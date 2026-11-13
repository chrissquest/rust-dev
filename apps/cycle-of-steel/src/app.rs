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
use crate::data::{GunsFile, RoomFile};
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
}

/// Runs the game on `event_loop` until its window closes.
///
/// The caller builds the event loop because Android's needs the handle the system
/// passes to `android_main`; on desktop it is a plain `EventLoop::new()`.
pub fn run(launch: Launch, event_loop: EventLoop<()>) -> Result<()> {
    event_loop.set_control_flow(ControlFlow::Poll);
    let world = load_world(&launch.assets, &launch.room, clock_seed())?;

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

/// Loads the game's definitions and a room, and builds the world in it.
fn load_world(assets: &AssetSource, room: &str, seed: u32) -> Result<World> {
    let defs = Arc::new(Defs::from_file(&GunsFile::load(assets)?)?);
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
    for name in &options.give {
        let gun = world
            .defs
            .gun_named(name)
            .with_context(|| format!("`{name}` is not a gun in guns.json"))?;
        world.give(gun);
    }
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
        assert_eq!(world.combat.targets.len(), 1, "the test dummy");
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
