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
use crate::audio::Audio;
use crate::data::{DungeonFile, EnemiesFile, GunsFile, LootFile, RoomFile};
use crate::sim::dungeon;
use crate::hud::MenuLayout;
use crate::input::{Action, Bindings, Gamepads, InputState, MenuInput, StickVisual, TouchControls};
use crate::menu::{Choice, Menu, MenuKind};
use crate::sim::save::Save;
use crate::stats::FrameStats;
use crate::storage;
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
    /// Where the save is kept. `None` plays without one. Only a run (the hub
    /// and its dungeon) loads and saves; a single room never does.
    pub save: Option<PathBuf>,
    /// Print load time, frame times and memory when the game closes.
    pub measure: bool,
}

/// Runs the game on `event_loop` until its window closes.
///
/// The caller builds the event loop because Android's needs the handle the system
/// passes to `android_main`; on desktop it is a plain `EventLoop::new()`.
pub fn run(launch: Launch, event_loop: EventLoop<()>) -> Result<()> {
    let started = Instant::now();
    event_loop.set_control_flow(ControlFlow::Poll);
    crate::stats::log_memory("start");
    let world = new_game(&launch.assets, &launch.room, &launch.give, launch.save.as_deref())?;
    crate::stats::log_memory("loading the world");
    let save = launch.save.filter(|_| world.floor.is_some());
    let saves_written = world.saves;
    // The game opens on the start menu (Unity's `StartMenu` scene), the hub
    // waiting behind it; a room or the dungeon asked for by name opens on it.
    let menu = (launch.room == DEFAULT_ROOM).then(Menu::start);

    let audio = Audio::new(&launch.assets);
    crate::stats::log_memory("opening the sound device and the music");
    let mut app = App {
        audio,
        room: launch.room,
        give: launch.give,
        exit_requested: false,
        stats: FrameStats::new(launch.measure),
        show_stats: false,
        stats_text: String::new(),
        started,
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
        save,
        saves_written,
        menu,
        accumulator: 0.0,
        last_frame: None,
        graphics: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    if launch.measure {
        print!("{}", app.stats.summary());
    }
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
    /// The world's `room_version` the view last uploaded.
    room_version: u64,
}

struct App {
    /// `None` on a machine with no sound device.
    audio: Option<Audio>,
    /// What a new game starts in and with, for Play.
    room: String,
    give: Vec<String>,
    /// A menu chose to leave the game.
    exit_requested: bool,
    /// Frame times and memory; whether F3 shows them, and their line.
    stats: FrameStats,
    show_stats: bool,
    stats_text: String,
    /// When `run` began, for the load time.
    started: Instant,
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
    /// Where the save is written, and the world's save count last written.
    save: Option<PathBuf>,
    saves_written: u64,
    /// The menu open, if any. The world does not step while there is one.
    menu: Option<Menu>,
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

        crate::stats::log_memory("creating the window");
        let surface = pollster::block_on(WindowSurface::new(window.clone(), self.backends))?;
        crate::stats::log_memory("opening the GPU");
        log::info!("drawing with {} ({:?})", surface.adapter_name(), surface.backend());

        let mut renderer = Renderer2d::new(surface.device(), surface.queue(), surface.format());
        let mut view = View::new(&mut renderer, &self.assets, &self.world.defs)?;
        view.set_room(&mut renderer, &self.world.room)?;
        crate::stats::log_memory("loading sprites and the room onto the GPU");

        Ok(Graphics {
            window,
            surface,
            renderer,
            view,
            room_version: self.world.room_version,
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
        let interval = self.last_frame.map(|last| now.duration_since(last).as_secs_f32()).unwrap_or(0.0);
        let frame_seconds = interval.min(MAX_FRAME_SECONDS);
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

        let menu_input = self.input.take_menu_input();
        if self.menu.is_some() {
            // Paused: presses go to the menu, and none is left for the world
            // to act on once it closes.
            self.input.take_sim_input();
            self.steer_menu(menu_input);
            self.accumulator = 0.0;
        }
        // `DeathMenu`: counted in real time, as the game stands still.
        if let Some(Menu { kind: MenuKind::Death { seconds_left }, .. }) = &mut self.menu {
            *seconds_left -= frame_seconds;
            if *seconds_left <= 0.0 {
                self.respawn();
            }
        }
        if self.input.take_pause_request() {
            self.pause();
        }
        self.accumulator += frame_seconds;
        if self.menu.is_none() && self.accumulator >= sim::DT {
            // Presses are handed over once, to the first step that can act on them.
            let mut input = self.input.take_sim_input();
            let stepping = Instant::now();
            let mut steps = 0;
            while self.accumulator >= sim::DT {
                self.world.step(&input);
                input = input.held_only();
                self.accumulator -= sim::DT;
                steps += 1;
            }
            self.stats.steps(steps, stepping.elapsed().as_secs_f32());
        }
        let blend = self.accumulator / sim::DT;

        self.write_save();
        let menu_view = self.menu.map(|menu| menu.view(&self.world));
        let Some(graphics) = self.graphics.as_mut() else {
            return;
        };
        let defs = self.world.defs.clone();
        for event in self.world.drain_events() {
            match event {
                Event::Shake(shake) => graphics.view.start_shake(shake),
                Event::Sound(sound) => {
                    if let Some(audio) = &mut self.audio {
                        audio.play(sound, &defs);
                    }
                }
                Event::OpenUpgrades => self.menu = Some(Menu::upgrades()),
                Event::Died => self.menu = Some(Menu::death()),
            }
        }
        if let Some(audio) = &mut self.audio {
            audio.set_paused(self.menu.is_some());
        }
        if self.menu.is_some() {
            // Fingers on the sticks lift onto the menu, which takes their touches.
            self.touch.release();
            self.mouse_held = false;
        }
        if self.input.take_map_request() {
            graphics.view.toggle_map();
        }
        // A new floor rebuilt the room: upload it again.
        if graphics.room_version != self.world.room_version {
            if let Err(error) = graphics.view.set_room(&mut graphics.renderer, &self.world.room) {
                log::error!("could not redraw the room: {error:#}");
            }
            graphics.room_version = self.world.room_version;
        }
        let waiting = Instant::now();
        let frame = match graphics.surface.acquire() {
            Ok(frame) => frame,
            // Nothing to draw into this time (minimised, mid-resize): try next frame.
            Err(_) => return,
        };
        // Waiting for the screen to take a frame is not work.
        let waited = waiting.elapsed();
        if self.show_stats {
            self.stats.line(&mut self.stats_text);
        }
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
            menu_view.as_ref(),
            self.show_stats.then_some(self.stats_text.as_str()),
        );
        graphics.surface.present(frame, encoder);
        self.stats.frame(interval, (now.elapsed() - waited).as_secs_f32());
        if self.stats.load.is_none() {
            self.stats.load = Some(self.started.elapsed());
        }
    }

    /// Acts on the presses meant for the open menu.
    fn steer_menu(&mut self, input: MenuInput) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        menu.move_by(input.moves);
        if input.back {
            self.back();
        } else if input.choose {
            let row = menu.selected;
            self.choose(row);
        }
    }

    /// Chooses a row of the open menu.
    fn choose(&mut self, row: usize) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        menu.selected = row;
        match menu.choice(row) {
            Choice::Nothing => {}
            Choice::Buy(upgrade) => {
                self.world.buy(upgrade);
            }
            Choice::Close => self.menu = None,
            Choice::Play => self.play(),
            Choice::Controls => self.menu = Some(Menu::new(MenuKind::Controls)),
            Choice::BackToStart => self.menu = Some(Menu::start()),
            Choice::DeleteSave => {
                if let MenuKind::Start { confirm_delete } = &mut menu.kind {
                    if *confirm_delete {
                        // `MainMenu.DeleteSave`.
                        if let Some(path) = &self.save
                            && let Err(error) = std::fs::remove_file(path)
                            && error.kind() != std::io::ErrorKind::NotFound
                        {
                            log::error!("could not delete {}: {error}", path.display());
                        }
                        *confirm_delete = false;
                    } else {
                        *confirm_delete = true;
                    }
                }
            }
            Choice::Exit => self.exit_requested = true,
            Choice::ToStartMenu => {
                // `PauseMenu.Menu`.
                self.world.save_progress();
                self.write_save();
                self.menu = Some(Menu::start());
            }
            Choice::Respawn => self.respawn(),
        }
    }

    /// Escape, or a pad's B: out of a menu that can be left.
    fn back(&mut self) {
        match self.menu {
            Some(menu) if menu.can_go_back() => {
                self.menu = match menu.kind {
                    MenuKind::Controls => Some(Menu::start()),
                    _ => None,
                }
            }
            _ => {}
        }
    }

    /// `PauseMenu.OnEscape`: pause, or leave the menu that is open.
    fn pause(&mut self) {
        if self.menu.is_some() {
            self.back();
        } else {
            self.menu = Some(Menu::new(MenuKind::Pause));
        }
    }

    /// `DeathMenu`'s end or its Respawn button.
    fn respawn(&mut self) {
        self.world.respawn();
        self.menu = None;
    }

    /// `MainMenu.PlayGame`: the Dungeon scene afresh, from the save.
    fn play(&mut self) {
        match new_game(&self.assets, &self.room, &self.give, self.save.as_deref()) {
            Ok(world) => {
                self.world = world;
                self.saves_written = self.world.saves;
                self.accumulator = 0.0;
                self.menu = None;
                if let Some(graphics) = &mut self.graphics {
                    // A different world: its room is uploaded afresh.
                    graphics.room_version = u64::MAX;
                }
            }
            Err(error) => log::error!("could not start a game: {error:#}"),
        }
    }

    /// A tap or click at `point` on the open menu chooses the row under it.
    fn press_menu(&mut self, point: Vec2) {
        let Some(menu) = &self.menu else {
            return;
        };
        if let Some(row) = MenuLayout::new(self.screen_size(), menu.row_count()).row_at(point) {
            self.choose(row);
        }
    }

    /// Writes the world's newest save, if it has one not yet written.
    fn write_save(&mut self) {
        if self.world.saves == self.saves_written {
            return;
        }
        self.saves_written = self.world.saves;
        if let (Some(path), Some(save)) = (&self.save, &self.world.saved)
            && let Err(error) = storage::write_save(path, save)
        {
            log::error!("could not save: {error:#}");
        }
    }

    fn command(&mut self, action: Action) {
        match action {
            Action::Pause => self.pause(),
            Action::ToggleStats => self.show_stats = !self.show_stats,
            Action::ToggleMap => self.input.request_map(),
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
                    self.command(action);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Vec2::new(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                if self.menu.is_some() {
                    if pressed {
                        self.press_menu(self.cursor);
                    }
                } else {
                    self.mouse_held = pressed;
                }
            }
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
                if self.menu.is_some() {
                    if touch.phase == TouchPhase::Started {
                        self.press_menu(position);
                    }
                    return;
                }
                let screen = self.screen_size();
                let can_interact = self.world.can_interact();
                self.touch
                    .touch(&mut self.input, touch.id, touch.phase, position, screen, can_interact);
            }
            WindowEvent::RedrawRequested => {
                self.frame();
                if self.exit_requested {
                    event_loop.exit();
                }
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

/// A world ready to play: `room` loaded, the save at `save` (or a new one) in
/// a run, `give`'s guns added, and the player waking in the hub.
fn new_game(assets: &AssetSource, room: &str, give: &[String], save: Option<&Path>) -> Result<World> {
    let mut world = load_world(assets, room, clock_seed())?;
    if world.floor.is_some()
        && let Some(path) = save
    {
        // `PlayerData.Start` loads the save, or starts a new one.
        let loaded = storage::read_save(path).unwrap_or_else(|| Save::new_game(world.player.max_health));
        world.load(&loaded);
    }
    give_guns(&mut world, give)?;
    if room == DEFAULT_ROOM {
        world.wake_in_hub();
    }
    Ok(world)
}

/// Loads the game's definitions and a room, and builds the world in it.
/// The room name that loads a generated dungeon floor instead of one room.
pub const DUNGEON: &str = "dungeon";

fn load_world(assets: &AssetSource, room: &str, seed: u32) -> Result<World> {
    let defs = Arc::new(Defs::from_file(&GunsFile::load(assets)?)?.with_loot(&LootFile::load(assets)?)?.with_enemies(&EnemiesFile::load(assets)?)?);
    // The hub is the start of a run: a floor waits beside it. `dungeon` starts
    // the run inside the floor's spawn room.
    if room == DEFAULT_ROOM || room == DUNGEON {
        let settings = DungeonFile::load(assets)?;
        let mut rooms = std::collections::HashMap::new();
        for name in dungeon::RunAssets::room_names(&settings) {
            let file = RoomFile::load(assets, &name)?;
            rooms.insert(name, file);
        }
        let hub = RoomFile::load(assets, DEFAULT_ROOM)?;
        let mut world = World::new_run(dungeon::RunAssets { settings, rooms, hub }, defs, seed)?;
        if room == DUNGEON {
            world.enter_dungeon();
        }
        return Ok(world);
    }
    Ok(World::new(Room::from_file(RoomFile::load(assets, room)?), defs, seed))
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
    /// A menu to draw over the frame.
    pub menu: Option<Menu>,
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
        options.menu.map(|menu| menu.view(&world)).as_ref(),
        None,
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
        let start_x = world.floor.as_ref().expect("a floor waits beside the hub").start_x;
        let hub_targets = world.combat.targets.iter().filter(|target| target.origin.x < start_x).count();
        assert_eq!(hub_targets, 2, "the test dummy and the gun room's EnemyTank");
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
        let start_x = world.floor.as_ref().unwrap().start_x;
        assert!(world.combat.targets.iter().filter(|target| target.origin.x < start_x).all(|target| target.enemy.is_none()));
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
    fn a_real_dungeon_floor_loads_and_starts_in_its_spawn_room() {
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        for seed in [1, 2, 3, 99] {
            let world = load_world(&assets, DUNGEON, seed).expect("the floor builds");
            assert!(!world.room.box_hits_solid(world.player.position, HALF_SIZE), "seed {seed}");
            // A floor is whole rooms: 32 by 18 cells each.
            assert_eq!((world.room.size().x % 16.0, world.room.size().y % 9.0), (0.0, 0.0));
            assert!(world.combat.targets.iter().any(|target| target.enemy.is_some()), "seed {seed} has enemies");
        }
    }

    #[test]
    fn a_room_with_enemies_seals_when_entered_and_opens_when_cleared() {
        use crate::sim::enemy::block_of;
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        let mut world = load_world(&assets, DUNGEON, 1).expect("the floor builds");
        let floor = world.floor.as_ref().unwrap();
        // A room with enemies and a door, beside a room the player can stand in.
        let index = floor
            .rooms
            .iter()
            .position(|room| {
                // A door tile in its left doorway, the room's first column.
                room.seal.iter().any(|tile| tile.cell == (room.block.0 * 32, room.block.1 * 18 + 9))
                    && world.combat.targets.iter().any(|target| target.enemy.as_ref().is_some_and(|enemy| enemy.counted && enemy.home == room.block))
            })
            .expect("seed 1 has a room with enemies and a left door");
        let block = floor.rooms[index].block;
        // Step in through its left doorway.
        let doorway = Vec2::new(block.0 as f32 * 16.0 + 0.1, block.1 as f32 * 9.0 + 4.75);
        world.player.position = doorway - Vec2::new(0.3, 0.0);
        world.player.previous_position = world.player.position;
        world.step(&Input::default());
        world.player.position = doorway + Vec2::new(0.2, 0.0);
        world.step(&Input::default());
        let floor = world.floor.as_ref().unwrap();
        assert!(floor.rooms[index].sealed, "entering sealed it");
        assert!(world.player.position.x > doorway.x + 0.9, "and moved the player further in");
        // The door's cells are the room's first column, from row 8 to 10.
        assert!(world.room.box_hits_solid(doorway, Vec2::splat(0.05)), "its left door blocks");

        for target in &mut world.combat.targets {
            if target.enemy.as_ref().is_some_and(|enemy| enemy.home == block) {
                target.health = 0.0;
            }
        }
        world.step(&Input::default());
        world.step(&Input::default());
        assert!(!world.floor.as_ref().unwrap().rooms[index].sealed, "cleared, it opened");
        assert!(!world.room.box_hits_solid(doorway, Vec2::splat(0.05)));
        assert_eq!(block_of(world.player.position), block);
    }

    #[test]
    fn walking_out_of_the_hub_enters_the_dungeon_and_the_exit_leads_back() {
        use crate::sim::combat::Interaction;
        let mut world = hub();
        let start_x = world.floor.as_ref().unwrap().start_x;
        // `ExitHub`: the hub's right-hand block is the way out.
        let (entrance, _) = world.combat.entrances[0];
        world.player.position = entrance;
        world.step(&Input::default());
        assert!(world.player.position.x > start_x, "in the dungeon");
        assert!(!world.room.box_hits_solid(world.player.position, HALF_SIZE));

        // The exit room's stairs: interact on them.
        let (stairs, _) = world.combat.exits[0];
        world.teleport(stairs);
        assert!(matches!(world.interaction(), Some(Interaction::Exit(_))));
        world.player.health = 300.0;
        let version = world.room_version;
        interact(&mut world);
        assert!(world.player.position.x < start_x, "back in the hub");
        assert_eq!(world.player.health, world.player.max_health, "healed");
        assert_eq!(world.room_version, version, "an uncleared floor stays");
        assert_eq!(world.difficulty, 1.0);

        // Cleared all but two rooms: the next floor is new and harder.
        world.floor.as_mut().unwrap().rooms_cleared = world.floor.as_ref().unwrap().rooms.len() - 2;
        world.teleport(world.combat.exits[0].0);
        interact(&mut world);
        assert_eq!(world.room_version, version + 1, "a new floor");
        assert_eq!(world.difficulty, 2.0);
        assert!(world.combat.targets.iter().any(|target| target.origin.x > start_x && target.enemy.is_some()));
    }

    #[test]
    fn a_new_player_starts_dies_upgrades_and_continues_from_the_save() {
        use crate::sim::save::Upgrade;
        let mut world = hub();
        world.load(&Save::new_game(world.player.max_health));
        let pistol = world.defs.gun_named("pistol_1").unwrap();
        assert_eq!(world.inventory[0].as_ref().map(|gun| gun.def), Some(pistol), "a new save starts with the pistol");

        // `ExitHub` saves on the way in, guns and all.
        let saves = world.saves;
        world.player.position = world.combat.entrances[0].0;
        world.step(&Input::default());
        assert_eq!(world.saves, saves + 1);
        assert_eq!(world.saved.as_ref().unwrap().inventory[0].as_deref(), Some("pistol_1"));

        // A gun and data cells found in the dungeon, then death: the gun is
        // lost with the reload, the cells are kept.
        world.give(world.defs.gun_named("laser_2").unwrap());
        world.data_cells = 40;
        world.player.health = 0.0;
        world.step(&Input::default());
        world.respawn();
        // Off the rack, then read through the intro.
        for _ in 0..91 {
            world.step(&Input::default());
        }
        interact(&mut world);
        interact(&mut world);
        assert!(!world.dialogue.is_showing() && !world.is_waking());
        assert!(world.player.position.x < world.floor.as_ref().unwrap().start_x, "back in the hub");
        assert!(world.inventory[1].is_none(), "the laser was not saved");
        assert_eq!(world.data_cells, 40);

        // The bench: interacting opens its menu; buying spends the cells.
        let (bench, _) = world.combat.benches[0];
        world.teleport(bench);
        assert!(matches!(world.interaction(), Some(Interaction::Bench(_))));
        world.drain_events().for_each(drop);
        interact(&mut world);
        assert!(world.drain_events().any(|event| event == Event::OpenUpgrades));
        assert!(world.buy(Upgrade::Health), "10 cells");
        assert!(world.buy(Upgrade::Crit), "5 cells");
        assert!(world.buy(Upgrade::Move), "6 cells");
        assert!(world.buy(Upgrade::Health), "15 cells");
        assert_eq!(world.data_cells, 4);
        assert!(!world.buy(Upgrade::Crit), "7 cells are not there");
        assert_eq!(world.player.max_health, 1400.0);
        assert_eq!(world.player.health, 1400.0, "more health heals fully");
        assert_eq!(world.player.crit_chance, 0.05);
        assert_eq!(world.player.move_speed, 12.0);

        // Out through the dungeon's exit, which saves, and into a new launch.
        world.teleport(world.combat.exits[0].0);
        interact(&mut world);
        let text = world.saved.as_ref().unwrap().to_json();
        let mut next = hub();
        next.load(&Save::from_json(&text).unwrap());
        assert_eq!(next.player.max_health, 1400.0);
        assert_eq!(next.player.crit_chance, 0.05);
        assert_eq!(next.player.move_speed, 12.0);
        assert_eq!(next.data_cells, 4);
        assert_eq!(next.upgrades.level(Upgrade::Health), 2);
        assert_eq!(next.upgrades.price(Upgrade::Health), 20);
        assert_eq!(next.inventory[0].as_ref().map(|gun| gun.def), Some(pistol));

        // Faster: `moveSpeed` 12 tops out 1.2 times as fast.
        next.teleport(next.room.spawn);
        let right = Input { movement: Vec2::X, ..Input::default() };
        // Ten steps in, short of any wall, on the same curve as at 10.
        for _ in 0..10 {
            next.step(&right);
        }
        let curve = 1.0 - (-crate::sim::player::RESPONSE * 10.0 * sim::DT).exp();
        let expected = crate::sim::player::TOP_SPEED * 1.2 * curve;
        assert!((next.player.velocity.x - expected).abs() < 1e-3, "{} against {expected}", next.player.velocity.x);
    }

    #[test]
    fn dying_in_the_dungeon_wakes_in_the_hub_with_a_new_floor() {
        let mut world = load_world(&AssetSource::folder(None).unwrap(), DUNGEON, 1).unwrap();
        let start_x = world.floor.as_ref().unwrap().start_x;
        let version = world.room_version;
        world.player.health = 0.0;
        world.drain_events().for_each(drop);
        world.step(&Input::default());
        // `PlayerData.Die`: everything stands still for the death screen.
        assert!(world.dead);
        assert!(world.drain_events().any(|event| event == Event::Died));
        let (tick, position) = (world.tick, world.player.position);
        world.step(&Input { movement: Vec2::X, ..Input::default() });
        assert_eq!((world.tick, world.player.position), (tick, position));
        world.respawn();
        assert!(!world.dead);
        // Woken on the hub's rack: carried right, then free.
        assert!(world.is_waking());
        assert_eq!(world.player.position, world.room.spawn);
        for _ in 0..91 {
            world.step(&Input { movement: Vec2::X, ..Input::default() });
        }
        assert!(!world.is_waking());
        assert!((world.player.position.x - world.room.spawn.x - (4.092 - 3.676)).abs() < 1e-3, "carried the rack's 0.416, no further");
        assert!(!world.room.box_hits_solid(world.player.position, HALF_SIZE));
        assert!(world.player.position.x < start_x);
        assert_eq!(world.player.health, world.player.max_health);
        assert_eq!(world.room_version, version + 1);
    }

    #[test]
    fn the_boss_room_shuts_the_player_in_until_the_boss_dies() {
        use crate::sim::dungeon::RoomKind;
        let mut world = load_world(&AssetSource::folder(None).unwrap(), DUNGEON, 1).unwrap();
        let floor = world.floor.as_ref().unwrap();
        let boss = floor.rooms.iter().position(|room| room.kind == RoomKind::Boss).unwrap();
        let block = floor.rooms[boss].block;
        assert!(floor.rooms[boss].seal.iter().any(|tile| tile.solid), "its wall closes the way it was entered");
        let middle = Vec2::new(block.0 as f32 * 16.0 + 8.0, block.1 as f32 * 9.0 + 2.0);
        world.teleport(world.room.nearest_open_point(middle) - Vec2::new(0.0, 9.0));
        world.player.position = world.room.nearest_open_point(middle);
        world.step(&Input::default());
        assert!(world.floor.as_ref().unwrap().rooms[boss].sealed);
        for target in &mut world.combat.targets {
            if target.enemy.as_ref().is_some_and(|enemy| enemy.home == block) {
                target.health = 0.0;
            }
        }
        world.step(&Input::default());
        world.step(&Input::default());
        let floor = world.floor.as_ref().unwrap();
        assert!(!floor.rooms[boss].sealed, "open again once the boss is dead");
        assert_eq!(floor.rooms_cleared, 1);
    }

    #[test]
    fn the_minimap_shows_the_spawn_its_neighbours_and_rooms_once_entered() {
        use crate::sim::dungeon::MapMark;
        let mut world = load_world(&AssetSource::folder(None).unwrap(), DUNGEON, 1).unwrap();
        world.step(&Input::default());
        let floor = world.floor.as_ref().unwrap();
        let marks: Vec<_> = floor.map().collect();
        let spawn = marks.iter().find(|(_, mark)| *mark == MapMark::Spawn).expect("the spawn shows").0;
        assert!(marks.iter().filter(|(_, mark)| *mark != MapMark::Spawn).all(|&(block, mark)| {
            mark == MapMark::Unknown && (block.0 - spawn.0).abs() + (block.1 - spawn.1).abs() == 1
        }), "only the spawn's neighbours, as unknown: {marks:?}");
        let neighbour = marks.iter().find(|(_, mark)| *mark == MapMark::Unknown).expect("a neighbour").0;
        let shown_before = marks.len();

        let inside = world.room.nearest_open_point(Vec2::new(neighbour.0 as f32 * 16.0 + 8.0, neighbour.1 as f32 * 9.0 + 4.5));
        world.teleport(inside);
        world.step(&Input::default());
        let marks: Vec<_> = world.floor.as_ref().unwrap().map().collect();
        let entered = marks.iter().find(|(block, _)| *block == neighbour).unwrap().1;
        assert!(matches!(entered, MapMark::Explored | MapMark::Exit | MapMark::Boss), "{entered:?}");
        assert!(marks.len() >= shown_before);
    }

    #[test]
    fn a_floor_keeps_every_rooms_lights_where_its_room_went() {
        let world = load_world(&AssetSource::folder(None).unwrap(), DUNGEON, 1).unwrap();
        let lights = &world.room.file.as_ref().unwrap().lights;
        let start_x = world.floor.as_ref().unwrap().start_x;
        let hub = RoomFile::load(&AssetSource::folder(None).unwrap(), DEFAULT_ROOM).unwrap();
        // The hub's lights stay where they were; the floor's are beyond it.
        assert!(lights.iter().filter(|light| light.position[0] < start_x).count() >= hub.lights.len());
        let floor_lights = lights.iter().filter(|light| light.position[0] >= start_x).count();
        assert!(floor_lights > 20, "{floor_lights}");
        let size = world.room.size();
        for light in lights {
            for point in &light.path {
                assert!(point[0] >= -0.01 && point[1] >= -0.01 && point[0] <= size.x + 0.01 && point[1] <= size.y + 0.01);
            }
        }
    }

    #[test]
    fn real_enemies_explosions_and_bullets_carry_their_unity_lights() {
        let world = hub();
        let defs = &world.defs;
        // `Warrior.prefab`'s Light 2D: red, 1.5, radius 1, on Entities, just off centre.
        let warrior = &defs.enemy(defs.enemy_named("Warrior").unwrap()).lights[0];
        assert_eq!((warrior.intensity, warrior.outer_radius), (1.5, 1.0));
        assert_eq!(warrior.layers, ["Entities"]);
        assert!(Vec2::from(warrior.position).length() < 0.1);
        // `Explosion 1`: a freeform square lighting every layer around the blast.
        let pistol = defs.gun(defs.gun_named("bazooka_1").unwrap());
        let missile = defs.bullet(pistol.bullet.unwrap());
        let blast = defs.effect(missile.effect.unwrap());
        assert_eq!(blast.lights.len(), 1);
        assert_eq!(blast.lights[0].path.len(), 4);
        assert!(!missile.lights.is_empty(), "the missile glows too");
    }

    #[test]
    fn the_world_asks_for_the_sounds_unity_plays() {
        use crate::sim::{Event, Sound};
        let mut world = hub();
        let pistol = world.defs.gun_named("pistol_1").unwrap();
        world.give(pistol);
        world.drain_events().count();
        world.step(&Input { movement: Vec2::X, aim: Some(Vec2::X), ..Input::default() });
        let heard: Vec<Event> = world.drain_events().collect();
        assert!(heard.contains(&Event::Sound(Sound::Fire(pistol))), "{heard:?}");
        assert!(heard.contains(&Event::Sound(Sound::WalkStart)));
        world.step(&Input::default());
        assert!(world.drain_events().any(|event| event == Event::Sound(Sound::WalkStop)));
        // The real files: the pistol's shot, and sounds.json's music and footsteps.
        assert!(world.defs.gun(pistol).sound.is_some());
        let sounds = crate::data::SoundsFile::load(&AssetSource::folder(None).unwrap()).unwrap();
        assert!(sounds.music.as_ref().is_some_and(|music| music.looping));
        assert!(sounds.player.walk.as_ref().is_some_and(|walk| walk.looping));
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

        // `DebugRoom`: two lines on the way in, read through with interact.
        assert_eq!(world.dialogue.current(), Some(crate::sim::dialogue::SECRET[0]));
        interact(&mut world);
        interact(&mut world);
        assert!(!world.dialogue.is_showing());

        // Stand on the way out and interact.
        let exit = 1 - entrance;
        world.player.position = world.combat.teleports[exit].position;
        interact(&mut world);
        let outside = world.player.position;
        assert!(!world.room.box_hits_solid(outside, HALF_SIZE), "left into a wall at {outside}");
        assert!(outside.y < inside.y);
        assert_eq!(world.dialogue.current(), Some(crate::sim::dialogue::SECRET2[0]));
    }

    #[test]
    fn the_hub_says_its_lines_once_per_save_and_they_hold_the_player() {
        use crate::sim::dialogue::{INTRO, PlayerEvent};
        let mut world = hub();
        world.load(&Save::new_game(world.player.max_health));
        world.wake_in_hub();
        // `HubRespawn`: the intro, 1.5 seconds in.
        for _ in 0..90 {
            world.step(&Input::default());
        }
        assert!(!world.dialogue.is_showing());
        world.step(&Input::default());
        assert_eq!(world.dialogue.current(), Some(INTRO[0]));

        // A line holds the player still; interact reads on.
        let before = world.player.position;
        for _ in 0..10 {
            world.step(&Input { movement: Vec2::X, ..Input::default() });
        }
        assert_eq!(world.player.position, before, "held while a line shows");
        interact(&mut world);
        interact(&mut world);
        assert!(!world.dialogue.is_showing());

        // The desk's box: its line once, and remembered by the save.
        let desk = world.combat.dialogues.iter().find(|trigger| trigger.event == Some(PlayerEvent::Desk)).unwrap();
        world.teleport(desk.center);
        world.step(&Input::default());
        assert_eq!(world.dialogue.current(), Some("And my desk... here I can perform upgrades on my shell"));
        interact(&mut world);
        world.player.position = world.combat.entrances[0].0;
        world.step(&Input::default());
        let saved = world.saved.clone().unwrap();
        assert!(saved.events.iter().any(|name| name == "Intro") && saved.events.iter().any(|name| name == "Desk"));

        // A new launch from that save says neither again.
        let mut next = hub();
        next.load(&saved);
        for _ in 0..120 {
            next.step(&Input::default());
        }
        let desk = next.combat.dialogues.iter().find(|trigger| trigger.event == Some(PlayerEvent::Desk)).unwrap();
        next.teleport(desk.center);
        next.step(&Input::default());
        assert!(!next.dialogue.is_showing());
    }

    #[test]
    fn the_exit_room_speaks_on_the_way_in() {
        let mut world = hub();
        let trigger = world.combat.dialogues.iter().find(|trigger| trigger.event.is_none()).expect("the exit room's box");
        world.teleport(trigger.center);
        world.step(&Input::default());
        assert_eq!(world.dialogue.current(), Some(crate::sim::dialogue::EXIT_ROOM[0]));
    }
}
