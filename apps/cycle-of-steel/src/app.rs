//! The window, the frame loop, and the windowless screenshot mode.
//!
//! The frame loop is a fixed-timestep loop: real time is added to an accumulator,
//! the simulation steps once per `sim::DT` it holds, and the leftover fraction
//! tells the view how far to blend towards the newest step.
//!
//! Everything that needs a window lives in [`Graphics`], which exists only between
//! `resumed` and `suspended`. On desktop that is the whole run. On Android the
//! system takes the drawing surface away whenever the app leaves the screen, so
//! `Graphics` is dropped and rebuilt while the [`World`] carries on untouched.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crystal_engine::glam::Vec2;
use crystal_engine::render2d::{self, Offscreen, Renderer2d, Target};
use crystal_engine::surface::WindowSurface;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Fullscreen, Window, WindowId};

use crate::assets::AssetSource;
use crate::data::{EnemiesFile, GunsFile, LootFile, RoomFile};
use crate::input::{Action, Bindings, Gamepads, InputState, StickVisual, TouchControls};
use crate::sim::defs::Defs;
use crate::sim::room::Room;
use crate::sim::{self, Event, World};
use crate::view::View;

/// A frame longer than this is treated as this long. After a stall (a dragged
/// window, a breakpoint) the game slows down briefly instead of running hundreds
/// of steps to catch up.
const MAX_FRAME_SECONDS: f32 = 0.25;

/// The room the game starts in when none is asked for.
pub const DEFAULT_ROOM: &str = "hub_room";

/// How to start the game.
pub struct Launch {
    pub assets: AssetSource,
    /// The room to start in: a file name under `rooms/`, without `.json`.
    pub room: String,
    pub backends: wgpu::Backends,
    /// Draw this many frames, then exit. A smoke test.
    pub frames: Option<u32>,
    /// Guns to start with: names from `guns.json`.
    pub give: Vec<String>,
}

/// Runs the game on `event_loop` until its window closes.
///
/// The caller builds the event loop because Android's needs the handle the system
/// passes to `android_main`; on desktop it is a plain `EventLoop::new()`.
pub fn run(launch: Launch, event_loop: EventLoop<()>) -> Result<()> {
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut world = load_world(&launch.assets, &launch.room, clock_seed())?;
    give_guns(&mut world, &launch.give)?;

    let mut app = App {
        assets: launch.assets,
        backends: launch.backends,
        frames_left: launch.frames,
        bindings: Bindings::default(),
        input: InputState::default(),
        gamepads: Gamepads::new(),
        touch: TouchControls::default(),
        cursor: Vec2::ZERO,
        mouse_held: false,
        world,
        accumulator: 0.0,
        last_frame: None,
        graphics: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Everything that exists only while there is a surface to draw on.
struct Graphics {
    window: Arc<Window>,
    surface: WindowSurface,
    renderer: Renderer2d,
    view: View,
}

struct App {
    assets: AssetSource,
    backends: wgpu::Backends,
    frames_left: Option<u32>,

    bindings: Bindings,
    input: InputState,
    gamepads: Gamepads,
    touch: TouchControls,
    /// Where the mouse cursor is, in pixels from the window's top-left, and
    /// whether its fire button is down.
    cursor: Vec2,
    mouse_held: bool,

    world: World,
    /// Real time not yet turned into simulation steps, in seconds.
    accumulator: f32,
    last_frame: Option<Instant>,

    graphics: Option<Graphics>,
    /// Why the loop was asked to exit, if it was a failure.
    error: Option<anyhow::Error>,
}

impl App {
    fn create_graphics(&self, event_loop: &ActiveEventLoop) -> Result<Graphics> {
        // Android ignores the title and sizes: the window is the whole screen.
        let attributes = Window::default_attributes()
            .with_title("Cycle of Steel")
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(320.0, 180.0));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("could not create the window")?,
        );

        let surface = pollster::block_on(WindowSurface::new(window.clone(), self.backends))?;
        log::info!("drawing with {} ({:?})", surface.adapter_name(), surface.backend());

        let mut renderer = Renderer2d::new(surface.device(), surface.queue(), surface.format());
        let mut view = View::new(&mut renderer, &self.assets, &self.world.defs)?;
        view.set_room(&mut renderer, &self.world.room)?;

        Ok(Graphics {
            window,
            surface,
            renderer,
            view,
        })
    }

    /// The size of what is being drawn into, in pixels.
    fn screen_size(&self) -> Vec2 {
        match &self.graphics {
            Some(graphics) => {
                let (width, height) = graphics.surface.size();
                Vec2::new(width as f32, height as f32)
            }
            None => Vec2::ONE,
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let frame_seconds = self
            .last_frame
            .map(|last| now.duration_since(last).as_secs_f32())
            .unwrap_or(0.0)
            .min(MAX_FRAME_SECONDS);
        self.last_frame = Some(now);

        let screen = self.screen_size();
        self.gamepads.poll(&mut self.input);
        self.touch.apply(&mut self.input, screen);
        // The mouse aims from the gun arm towards the point under the cursor.
        let mouse_aim = match (&self.graphics, self.mouse_held) {
            (Some(graphics), true) => {
                Some(graphics.view.screen_to_world(self.cursor, screen) - self.world.player.pivot())
            }
            _ => None,
        };
        self.input.set_mouse_aim(mouse_aim);

        self.accumulator += frame_seconds;
        if self.accumulator >= sim::DT {
            // Presses are handed over once, to the first step that can act on them.
            let mut input = self.input.take_sim_input();
            while self.accumulator >= sim::DT {
                self.world.step(&input);
                input = input.held_only();
                self.accumulator -= sim::DT;
            }
        }
        let blend = self.accumulator / sim::DT;

        let Some(graphics) = self.graphics.as_mut() else {
            return;
        };
        for event in self.world.drain_events() {
            match event {
                Event::Shake(shake) => graphics.view.start_shake(shake),
            }
        }
        let frame = match graphics.surface.acquire() {
            Ok(frame) => frame,
            // Nothing to draw into this time (minimised, mid-resize): try next frame.
            Err(_) => return,
        };
        let mut encoder = graphics
            .surface
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        graphics.view.draw(
            &mut graphics.renderer,
            &mut encoder,
            Target {
                view: &frame.view,
                width: frame.width,
                height: frame.height,
            },
            &self.world,
            blend,
            frame_seconds,
            &stick_visuals(&self.touch, screen),
        );
        graphics.surface.present(frame, encoder);
    }

    fn command(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        match action {
            Action::Quit => event_loop.exit(),
            Action::ToggleFullscreen => {
                if let Some(graphics) = &self.graphics {
                    let fullscreen = match graphics.window.fullscreen() {
                        Some(_) => None,
                        None => Some(Fullscreen::Borderless(None)),
                    };
                    graphics.window.set_fullscreen(fullscreen);
                }
            }
            _ => {}
        }
    }

    /// Lets go of everything held. The releases never arrive once the window has
    /// lost focus or the app has left the screen.
    fn release_input(&mut self) {
        self.input.release_all();
        self.touch.release();
        self.mouse_held = false;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.graphics.is_some() {
            return;
        }
        match self.create_graphics(event_loop) {
            Ok(graphics) => {
                self.graphics = Some(graphics);
                // Time spent away is not game time.
                self.last_frame = None;
                event_loop.set_control_flow(ControlFlow::Poll);
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        // The surface is about to become invalid: drop everything built on it. The
        // world stays, so the game is exactly where it was on return.
        self.graphics = None;
        self.release_input();
        // Nothing to draw and nothing to simulate: sleep until the system wakes us.
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(graphics) = self.graphics.as_mut() {
                    graphics.surface.resize(size);
                }
            }
            WindowEvent::Focused(false) => self.release_input(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(key) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                if let Some(action) = self.input.key(&self.bindings, key, pressed) {
                    self.command(action, event_loop);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Vec2::new(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.mouse_held = state == ElementState::Pressed,
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                // Scrolling up goes to the previous slot, as in the Unity build.
                if lines > 0.0 {
                    self.input.press(Action::PreviousSlot);
                } else if lines < 0.0 {
                    self.input.press(Action::NextSlot);
                }
            }
            WindowEvent::Touch(touch) => {
                let position = Vec2::new(touch.location.x as f32, touch.location.y as f32);
                let screen = self.screen_size();
                let can_interact = self.world.can_interact();
                self.touch
                    .touch(&mut self.input, touch.id, touch.phase, position, screen, can_interact);
            }
            WindowEvent::RedrawRequested => {
                self.frame();
                if let Some(left) = self.frames_left.as_mut() {
                    *left = left.saturating_sub(1);
                    if *left == 0 {
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(graphics) = &self.graphics {
            graphics.window.request_redraw();
        }
    }
}

/// Puts the named guns into the inventory, for `--give`.
fn give_guns(world: &mut World, names: &[String]) -> Result<()> {
    for name in names {
        let gun = world
            .defs
            .gun_named(name)
            .with_context(|| format!("`{name}` is not a gun in guns.json"))?;
        world.give(gun);
    }
    Ok(())
}

/// Loads the game's definitions and a room, and builds the world in it.
fn load_world(assets: &AssetSource, room: &str, seed: u32) -> Result<World> {
    let defs = Arc::new(Defs::from_file(&GunsFile::load(assets)?)?.with_loot(&LootFile::load(assets)?)?.with_enemies(&EnemiesFile::load(assets)?)?);
    let room = Room::from_file(RoomFile::load(assets, room)?);
    Ok(World::new(room, defs, seed))
}

/// A seed that differs from launch to launch. The simulation itself never reads
/// the clock; this is the one place randomness comes in from outside.
fn clock_seed() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() ^ elapsed.as_secs() as u32)
        .unwrap_or(1)
}

/// The on-screen sticks that fingers are holding.
fn stick_visuals(touch: &TouchControls, screen: Vec2) -> Vec<StickVisual> {
    [touch.movement.visual(screen), touch.aim.visual(screen)]
        .into_iter()
        .flatten()
        .collect()
}

/// What `--screenshot` renders.
pub struct Screenshot {
    pub path: PathBuf,
    pub assets: AssetSource,
    /// The room to draw: a file name under `rooms/`, without `.json`.
    pub room: String,
    pub backends: wgpu::Backends,
    pub width: u32,
    pub height: u32,
    /// Simulation steps to run before the picture is taken.
    pub ticks: u32,
    /// The movement input held during those steps.
    pub movement: Vec2,
    /// The direction the trigger is held towards during those steps.
    pub aim: Option<Vec2>,
    /// Guns to start with in the inventory: keys of `guns.json`.
    pub give: Vec<String>,
    /// A finger held on the screen during those steps: where it went down and
    /// where it was dragged to, in pixels from the top-left. Drives the on-screen
    /// sticks exactly as a real touch would.
    pub touch: Option<(Vec2, Vec2)>,
    /// Frame the whole room instead of following the player.
    pub whole_room: bool,
}

/// Runs the game for a while with no window, draws one frame off-screen and saves
/// it as a PNG. Needs a GPU or a software rasteriser, but no display.
pub fn screenshot(options: Screenshot) -> Result<()> {
    let screen = Vec2::new(options.width as f32, options.height as f32);

    let mut input = InputState::default();
    let mut touch = TouchControls::default();
    if let Some((down, dragged)) = options.touch {
        touch.touch(&mut input, 0, TouchPhase::Started, down, screen, false);
        touch.touch(&mut input, 0, TouchPhase::Moved, dragged, screen, false);
    }
    touch.apply(&mut input, screen);
    input.set_pad(options.movement, Vec2::ZERO);
    input.set_mouse_aim(options.aim);
    let sim_input = input.sim_input();

    // A fixed seed: the same command always gives the same picture.
    let mut world = load_world(&options.assets, &options.room, 1)?;
    give_guns(&mut world, &options.give)?;
    for _ in 0..options.ticks {
        world.step(&sim_input);
    }

    let (device, queue) = pollster::block_on(render2d::headless_device(options.backends))?;
    let mut renderer = Renderer2d::new(&device, &queue, render2d::OFFSCREEN_FORMAT);
    let mut view = View::new(&mut renderer, &options.assets, &world.defs)?;
    view.set_room(&mut renderer, &world.room)?;
    view.set_overview(options.whole_room);

    let offscreen = Offscreen::new(&device, options.width, options.height);
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("screenshot") });
    view.draw(
        &mut renderer,
        &mut encoder,
        offscreen.target(),
        &world,
        1.0,
        0.0,
        &stick_visuals(&touch, screen),
    );
    queue.submit(std::iter::once(encoder.finish()));

    let pixels = offscreen.read_rgba(&device, &queue)?;
    save_png(&options.path, options.width, options.height, &pixels)?;

    let position = world.player.position;
    println!(
        "wrote {} ({}x{}), tick {}, player at ({:.3}, {:.3}), {} bullets, {} effects",
        options.path.display(),
        options.width,
        options.height,
        world.tick,
        position.x,
        position.y,
        world.combat.bullets.len(),
        world.combat.effects.len()
    );
    Ok(())
}

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("could not create {}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(rgba))
        .with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Input;
    use crate::sim::combat::Interaction;
    use crate::sim::player::HALF_SIZE;

    fn hub() -> World {
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        load_world(&assets, DEFAULT_ROOM, 1).expect("the hub loads")
    }

    fn interact(world: &mut World) {
        world.step(&Input {
            interact: true,
            ..Input::default()
        });
    }

    #[test]
    fn the_hub_has_guns_a_target_and_a_free_spawn() {
        let world = hub();
        assert!(world.combat.pickups.len() >= 10, "the gun room's guns: {}", world.combat.pickups.len());
        assert_eq!(world.combat.targets.len(), 2, "the test dummy and the gun room's EnemyTank");
        assert!(world.combat.targets[0].enemy.is_none());
        let tank = world.combat.targets[1].enemy.as_ref().expect("an enemy");
        assert_eq!(world.defs.enemy(tank.def).name, "EnemyTank");
        assert!(!world.room.box_hits_solid(world.player.position, HALF_SIZE));
    }

    #[test]
    fn the_real_lasers_shine_a_beam_and_have_their_sprites() {
        for name in ["laser_2", "debug_laser"] {
            let mut world = hub();
            let gun = world.defs.gun_named(name).expect(name);
            let def = world.defs.gun(gun);
            assert!(def.damage_delay > 0.0, "{name}");
            assert!(def.laser_sprites.iter().all(Option::is_some), "{name}");
            world.give(gun);
            let input = sim::Input { aim: Some(Vec2::X), ..Default::default() };
            world.step(&input);
            let beam = world.combat.beam.expect(name);
            assert!(beam.length > 0.1 && beam.length < 16.0, "{name}: stops at the hub's wall, {beam:?}");
        }
    }

    /// Fires the named gun once from the spawn, `turned` radians off the line to
    /// the hub's test dummy, and returns the health the dummy lost in two seconds.
    fn shoot_at_the_dummy(name: &str, turned: f32) -> f32 {
        let mut world = hub();
        // The gun room's EnemyTank is nearer the spawn than the dummy, so homing
        // shots would chase it, as in Unity. These tests are about the dummy.
        world.combat.targets.retain(|target| target.enemy.is_none());
        let gun = world.defs.gun_named(name).expect(name);
        world.give(gun);
        let before = world.combat.targets[0].health;
        let toward = (world.combat.targets[0].position - world.player.position).normalize();
        world.step(&sim::Input { aim: Some(Vec2::from_angle(turned).rotate(toward)), ..Default::default() });
        assert_eq!(world.combat.bullets.len(), 1, "{name} fired");
        for _ in 0..120 {
            world.step(&sim::Input::default());
        }
        before - world.combat.targets.first().map_or(0.0, |target| target.health)
    }

    #[test]
    fn the_real_bazooka_hurts_only_through_its_explosion() {
        let world = hub();
        let bazooka = world.defs.gun(world.defs.gun_named("bazooka_1").unwrap());
        let missile = world.defs.bullet(bazooka.bullet.unwrap());
        let explosion = world.defs.effect(missile.effect.unwrap()).explosion.expect("Explosion 1 explodes");
        // `Explosion 1.prefab`: Damage 200, CircleCollider2D radius 0.44217265.
        assert_eq!(explosion.damage, 200.0);
        assert!((explosion.radius - 0.4422).abs() < 1e-4);
        assert_eq!(bazooka.damage, 0.0);

        assert_eq!(shoot_at_the_dummy("bazooka_1", 0.0), 200.0);
    }

    #[test]
    fn the_real_homing_guns_bend_their_shots_onto_the_dummy() {
        for name in ["rifle_3", "lmg_red"] {
            let world = hub();
            let gun = world.defs.gun(world.defs.gun_named(name).unwrap());
            let bullet = world.defs.bullet(gun.bullet.unwrap());
            // `Bullet_Homing.prefab`: homingPower 500, Rigidbody2D linear drag 3.
            assert_eq!((bullet.homing, bullet.drag), (500.0, 3.0), "{name}");
        }
        // Fired 25 degrees wide of the dummy, the rifle's shot still lands. (The
        // lmg sprays at random, so it is left to the unit tests.)
        assert!(shoot_at_the_dummy("rifle_3", 25f32.to_radians()) > 0.0);
        // An ordinary bullet fired the same way misses.
        assert_eq!(shoot_at_the_dummy("rifle_1", 25f32.to_radians()), 0.0);
    }

    #[test]
    fn the_real_freezing_guns_freeze_but_not_the_dummy() {
        let mut world = hub();
        for name in ["pistol_2", "smg_1", "minigun_1"] {
            let gun = world.defs.gun(world.defs.gun_named(name).unwrap());
            // `Bullet_Pistol_2.prefab`: Bullet_Freeze, freezeStrength 5.
            assert_eq!(world.defs.bullet(gun.bullet.unwrap()).freeze, 5.0, "{name}");
        }
        // No other bullet freezes.
        assert_eq!(world.defs.bullets.iter().filter(|bullet| bullet.freeze > 0.0).count(), 1);
        // Unity's test dummy has no ElementalEffectHandler: hit, but never frozen.
        assert_eq!(world.combat.targets[0].elemental, None);
        assert!(shoot_at_the_dummy("pistol_2", 0.0) > 0.0);

        // Something that can be frozen, shot with the same real gun, is.
        world.combat.targets[0].elemental = Some(Default::default());
        world.give(world.defs.gun_named("pistol_2").unwrap());
        let toward = (world.combat.targets[0].position - world.player.position).normalize();
        world.step(&sim::Input { aim: Some(toward), ..Default::default() });
        let mut frozen = false;
        for _ in 0..120 {
            world.step(&sim::Input::default());
            frozen |= world.combat.targets[0].elemental.unwrap().is_frozen();
        }
        assert!(frozen);
    }

    #[test]
    fn the_real_font_can_write_any_damage_number() {
        let assets = AssetSource::folder(None).unwrap();
        let font = crate::data::FontFile::load(&assets).unwrap();
        let catalogue = crate::data::SpriteCatalogue::load(&assets).unwrap();
        for character in "0123456789.".chars() {
            let glyph = font.glyphs.get(&character).unwrap_or_else(|| panic!("no glyph for {character}"));
            let sprite = catalogue.sprites.get(&glyph.sprite).expect("the glyph's sprite is in sprites.json");
            // Thaleah at 16 pixels: one em is one unit.
            assert_eq!(sprite.pixels_per_unit, 16.0);
            assert!(glyph.advance > 0.0 && glyph.advance <= 0.5, "{character}: {}", glyph.advance);
        }
        // `ThaleahFat.fontsettings`: "0" is 9 by 9 with its 1-pixel border, advance 8 of 16.
        let zero = &font.glyphs[&'0'];
        assert_eq!(zero.advance, 0.5);
        let zero = &catalogue.sprites[&zero.sprite];
        assert_eq!(zero.rect[2..], [9, 9]);
        // Stored upside down: the baseline is 8 of the 9 rows up from the stored bottom.
        assert!((zero.pivot[1] - 8.0 / 9.0).abs() < 1e-3);
    }

    #[test]
    fn shooting_the_real_dummy_floats_a_number() {
        let mut world = hub();
        world.give(world.defs.gun_named("pistol_1").unwrap());
        let toward = (world.combat.targets[0].position - world.player.position).normalize();
        world.step(&sim::Input { aim: Some(toward), ..Default::default() });
        let mut shown = None;
        for _ in 0..120 {
            world.step(&sim::Input::default());
            shown = shown.or(world.combat.damage_numbers.first().copied());
        }
        let shown = shown.expect("a number appeared");
        let pistol = world.defs.gun(world.defs.gun_named("pistol_1").unwrap());
        assert!(shown.amount == pistol.damage || (shown.crit && shown.amount == pistol.damage * 2.0));
        assert!(world.combat.damage_numbers.is_empty(), "and left after its 1.4 s");
    }

    #[test]
    fn every_real_chest_breaks_into_loot_and_remains() {
        use crate::sim::defs::Loot;
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        for room in ["n1", "ns1", "e1"] {
            let mut world = load_world(&assets, room, 1).expect("the room loads");
            let chests: Vec<_> = world.combat.targets.iter().filter_map(|target| target.chest.map(|chest| (chest, target.origin))).collect();
            assert!(!chests.is_empty(), "{room} has chests");
            assert_eq!(world.combat.solids().count(), chests.len());
            for target in &mut world.combat.targets {
                target.health = 0.0;
            }
            world.step(&Input::default());
            let broken = world.combat.remains.iter().filter(|remains| matches!(remains.kind, crate::sim::loot::RemainsKind::Chest(_)));
            assert_eq!(broken.count(), chests.len(), "{room}");
            for (chest, origin) in chests {
                let def = world.defs.chest(chest);
                let gives_gun = def.loot.iter().any(|entry| match entry.loot {
                    Loot::Pool(pool) => world.defs.pool(pool).entries.iter().any(|line| matches!(line.loot, Loot::Gun(_))),
                    _ => false,
                });
                let at_chest = |position: Vec2| position == origin;
                if gives_gun {
                    assert!(world.combat.pickups.iter().any(|pickup| at_chest(pickup.position)), "{room}: a weapon chest leaves a gun");
                } else {
                    assert!(world.combat.drops.iter().any(|drop| at_chest(drop.position)), "{room}: a data chest leaves drops");
                }
            }
        }
    }

    #[test]
    fn the_real_enemy_tank_swings_for_350_and_dies_into_a_body() {
        let mut world = hub();
        let tank = world.combat.targets.iter().position(|target| target.enemy.is_some()).unwrap();
        let def = world.defs.enemy(world.combat.targets[tank].enemy.as_ref().unwrap().def);
        // `EnemyTank.prefab`: Killable maxHealth 1000; Warrior damage 350, followRange 5.
        let crate::sim::defs::Ai::Warrior(warrior) = &def.ai else { panic!("the tank is a Warrior") };
        assert_eq!((def.max_health, warrior.damage, warrior.follow_range), (1000.0, 350.0, 5.0));
        let run_right = crate::sim::defs::Animation::Run(crate::sim::defs::Direction::Right);
        assert_eq!(def.animations[run_right.index()].as_ref().unwrap().frames.len(), 12, "run_right has 12 frames");
        // Stand right beside it in the gun room, without its health packs healing.
        world.combat.drops.clear();
        world.player.position = world.combat.targets[tank].origin + Vec2::new(0.3, 0.0);
        world.player.previous_position = world.player.position;
        for _ in 0..120 {
            world.step(&Input::default());
        }
        assert_eq!(world.player.health, 650.0, "one swing landed within two seconds");

        world.combat.targets[tank].health = 0.0;
        world.step(&Input::default());
        assert!(world.combat.targets.iter().all(|target| target.enemy.is_none()));
        assert!(matches!(world.combat.remains[0].kind, crate::sim::loot::RemainsKind::Body(..)));
    }

    #[test]
    fn shooting_real_chests_breaks_them_into_a_gun_or_data_cells() {
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        for (chest, gives_gun) in [("Common Weapon Chest", true), ("Common Data Chest", false)] {
            let mut world = load_world(&assets, "n1", 1).expect("n1 loads");
            world.combat.targets.retain(|target| target.enemy.is_none());
            world.give(world.defs.gun_named("shotgun_1").unwrap());
            let id = world.defs.chest_named(chest).unwrap();
            let target = world.combat.targets.iter().find(|target| target.chest == Some(id)).unwrap().clone();
            // Stand just to its right, on free floor, and shoot left.
            world.player.position = target.position + Vec2::new(1.15, 0.0);
            world.player.previous_position = world.player.position;
            assert!(!world.room.box_hits_solid(world.player.position, HALF_SIZE), "{chest}");
            for _ in 0..600 {
                world.step(&Input { aim: Some(-Vec2::X), ..Input::default() });
            }
            assert!(world.combat.targets.iter().all(|other| other.id != target.id), "{chest} broke");
            if gives_gun {
                assert!(world.combat.pickups.iter().any(|pickup| pickup.position == target.origin), "a gun lies there");
            } else {
                // Its cells land beside the player and fly straight in.
                assert!(world.data_cells > 0, "the data cells were collected");
            }
        }
    }

    #[test]
    fn the_real_crab_and_turret_shoot_unitys_bullets() {
        use crate::sim::defs::Ai;
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        let mut world = load_world(&assets, "e1", 1).expect("e1 loads");
        let kind = |world: &World, name: &str| world.defs.enemy_named(name).unwrap();
        let (crab, turret) = (kind(&world, "Crab"), kind(&world, "multi-turret_1"));
        let Ai::Ranged(ranged) = &world.defs.enemy(crab).ai else { panic!("the Crab is ranged") };
        // `Crab.prefab`: RangedAI attackRange 2.5, fireInterval 1, damage 50; Killable 200.
        assert_eq!((ranged.attack_range, ranged.fire_interval, ranged.damage), (2.5, 1.0, 50.0));
        assert_eq!(world.defs.enemy(crab).max_health, 200.0);
        assert_eq!(world.defs.enemy(crab).touch.map(|touch| touch.damage), Some(100.0), "BasicAIAttack bites for 100");
        let Ai::Laser(laser) = &world.defs.enemy(kind(&world, "LASERCrab")).ai else { panic!("the LASERCrab has a laser") };
        // `LASERCrab.prefab`: attackRange 4, maxFireTime 3, damage 50 every 0.5 s, turning 50 degrees/s.
        assert_eq!((laser.attack_range, laser.max_fire_time, laser.damage, laser.damage_rate, laser.turn_speed), (4.0, 3.0, 50.0, 0.5, 50.0));
        let Ai::Turret(muzzles) = &world.defs.enemy(turret).ai else { panic!("the turret is a turret") };
        assert_eq!(muzzles.muzzles.len(), 8);
        assert_eq!(world.defs.bullet(muzzles.bullet).time_to_live, 2.0, "`Enemy Bullet` lives 2 s");

        // Stand 1 unit beside the turret: within a second its eight shots are out.
        let at = world.combat.targets.iter().find(|target| target.enemy.as_ref().is_some_and(|enemy| enemy.def == turret)).unwrap().origin;
        world.combat.targets.retain(|target| target.enemy.as_ref().is_some_and(|enemy| enemy.def == turret) && target.origin == at);
        world.player.position = at + Vec2::new(1.0, 0.0);
        world.player.previous_position = world.player.position;
        world.step(&Input::default());
        assert_eq!(world.combat.shots.len(), 8);
    }

    #[test]
    fn a_real_drone_that_touches_the_player_explodes_for_200() {
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        let mut world = load_world(&assets, "e1", 1).expect("e1 loads");
        let drone = world.defs.enemy_named("Drone").unwrap();
        let def = world.defs.enemy(drone);
        // `Drone.prefab`: on the FlyingEnemy layer, explosive; `Explosion.prefab`:
        // Damage 200, radius 0.4422 at scale 2.
        assert!(def.flying);
        let blast = world.defs.effect(def.explosion.unwrap()).explosion.unwrap();
        assert_eq!(blast.damage, 200.0);
        assert!((blast.radius - 0.8843).abs() < 1e-3);

        let at = world.combat.targets.iter().find(|target| target.enemy.as_ref().is_some_and(|enemy| enemy.def == drone)).unwrap().origin;
        world.combat.targets.retain(|target| target.origin == at);
        world.combat.drops.clear();
        world.player.position = at;
        world.player.previous_position = at;
        world.step(&Input::default());
        assert!(world.combat.targets.is_empty(), "it died at once");
        // Its explosion notices the player on the next step, as Unity's trigger does.
        world.step(&Input::default());
        assert_eq!(world.player.last_hurt, crate::sim::DT as f64, "its explosion hurt the player");
        // 200 off, unless the health pack it can drop landed on the player too.
        assert!(world.player.health == 800.0 || world.player.health == 900.0, "{}", world.player.health);
    }

    #[test]
    fn real_lava_grates_hurry_a_warrior_standing_on_them() {
        use crate::sim::combat::GrateKind;
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        let mut world = load_world(&assets, "n1", 1).expect("n1 loads");
        let lava = *world.combat.grates.iter().find(|grate| grate.kind == GrateKind::Lava).expect("n1 has lava");
        assert_eq!(lava.duration, 3.0);
        let warrior = world.combat.targets.iter().position(|target| target.enemy.is_some()).unwrap();
        world.combat.targets[warrior].origin = lava.position;
        world.combat.targets[warrior].position = lava.position;
        world.step(&Input::default());
        let def = world.defs.enemy(world.combat.targets[warrior].enemy.as_ref().unwrap().def);
        let drag = world.combat.targets[warrior].elemental.unwrap().drag(def.drag);
        assert_eq!(drag, def.drag - 2.0, "lava takes 2 off its drag");
    }

    #[test]
    fn the_real_priest_is_the_archbishop() {
        use crate::sim::defs::Ai;
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        let world = load_world(&assets, "boss_room", 1).expect("the boss room loads");
        let priest = world.combat.targets.iter().find_map(|target| target.enemy.as_ref()).expect("the Priest stands there");
        let def = world.defs.enemy(priest.def);
        // `Priest.prefab`: Killable maxHealth 50000, enemyName Archbishop, a boss bar;
        // BossAI bulletDamage 100; `BossLaser.prefab`: 20 every 0.1 s.
        assert_eq!((def.max_health, def.boss_bar.as_deref()), (50000.0, Some("Archbishop")));
        let Ai::Boss(boss) = &def.ai else { panic!("the Priest runs BossAI") };
        assert_eq!((boss.file.bullet_damage, boss.file.laser.damage, boss.file.laser.damage_delay), (100.0, 20.0, 0.1));
        assert!(world.defs.bullet(boss.homing_bullet).homing > 0.0, "its ring homes in");
    }

    #[test]
    fn the_hubs_drops_fly_to_the_player_and_count() {
        use crate::data::DropKind;
        let mut world = hub();
        assert_eq!(world.combat.drops.len(), 6, "three health packs and three data cells");
        let cell = world
            .combat
            .drops
            .iter()
            .position(|drop| world.defs.drop_def(drop.def).kind == DropKind::Data)
            .unwrap();
        world.combat.drops[cell].position = world.player.position + Vec2::new(1.0, 0.0);
        world.player.health = 950.0;
        let pack = world.combat.drops.iter().position(|drop| world.defs.drop_def(drop.def).kind == DropKind::Heal).unwrap();
        world.combat.drops[pack].position = world.player.position + Vec2::new(-1.0, 0.0);
        for _ in 0..120 {
            world.step(&Input::default());
        }
        assert_eq!(world.data_cells, 10, "the hub sets its `Data Drop 2`s to 10");
        assert_eq!(world.player.health, 1000.0, "healed by 500 but not past the top");
        assert_eq!(world.combat.drops.len(), 4);
    }

    #[test]
    fn the_hidden_spot_leads_into_the_gun_room_and_back_out() {
        let mut world = hub();
        assert_eq!(world.combat.teleports.len(), 2);
        let entrance = world
            .combat
            .teleports
            .iter()
            .position(|teleport| teleport.offset.x > 0.0)
            .expect("the way in moves the player right");

        // Stand on the way in and interact.
        world.player.position = world.combat.teleports[entrance].position;
        assert_eq!(world.interaction(), Some(Interaction::Teleport(entrance)));
        interact(&mut world);
        let inside = world.player.position;
        assert!(!world.room.box_hits_solid(inside, HALF_SIZE), "arrived inside a wall at {inside}");
        // The guns are laid out in this room.
        let nearest_gun = world
            .combat
            .pickups
            .iter()
            .map(|pickup| pickup.position.distance(inside))
            .fold(f32::INFINITY, f32::min);
        assert!(nearest_gun < 6.0, "no gun within reach of the gun room: nearest is {nearest_gun} away");

        // Stand on the way out and interact.
        let exit = 1 - entrance;
        world.player.position = world.combat.teleports[exit].position;
        interact(&mut world);
        let outside = world.player.position;
        assert!(!world.room.box_hits_solid(outside, HALF_SIZE), "left into a wall at {outside}");
        assert!(outside.y < inside.y);
    }
}
