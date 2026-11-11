//! The window, the frame loop, and the windowless screenshot mode.
//!
//! The frame loop is a fixed-timestep loop: real time is added to an accumulator,
//! the simulation steps once per `sim::DT` it holds, and the leftover fraction
//! tells the view how far to blend towards the newest step.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use crystal_engine::glam::Vec2;
use crystal_engine::render2d::{self, Offscreen, Renderer2d, Target};
use crystal_engine::surface::WindowSurface;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Fullscreen, Window, WindowId};

use crate::assets;
use crate::input::{Action, Bindings, Gamepads, InputState};
use crate::sim::room::Room;
use crate::sim::{self, World};
use crate::view::View;

/// A frame longer than this is treated as this long. After a stall (a dragged
/// window, a breakpoint) the game slows down briefly instead of running hundreds
/// of steps to catch up.
const MAX_FRAME_SECONDS: f32 = 0.25;

/// How to start the game.
pub struct Launch {
    pub assets: Option<PathBuf>,
    pub backends: wgpu::Backends,
    /// Draw this many frames, then exit. A smoke test.
    pub frames: Option<u32>,
}

/// Opens the window and runs until it closes.
pub fn run(launch: Launch) -> Result<()> {
    let assets_dir = assets::find_dir(launch.assets.as_deref())?;
    let event_loop = EventLoop::new().context("could not create the event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        assets_dir,
        backends: launch.backends,
        frames_left: launch.frames,
        bindings: Bindings::default(),
        input: InputState::default(),
        gamepads: Gamepads::new(),
        world: World::new(Room::test_room()),
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

/// Everything that exists only while there is a window.
struct Graphics {
    window: Arc<Window>,
    surface: WindowSurface,
    renderer: Renderer2d,
    view: View,
}

struct App {
    assets_dir: PathBuf,
    backends: wgpu::Backends,
    frames_left: Option<u32>,

    bindings: Bindings,
    input: InputState,
    gamepads: Gamepads,

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
        let mut view = View::new(&mut renderer, &self.assets_dir)?;
        view.set_room(&mut renderer, &self.world.room);

        Ok(Graphics {
            window,
            surface,
            renderer,
            view,
        })
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let frame_seconds = self
            .last_frame
            .map(|last| now.duration_since(last).as_secs_f32())
            .unwrap_or(0.0)
            .min(MAX_FRAME_SECONDS);
        self.last_frame = Some(now);

        self.gamepads.poll(&mut self.input);
        let input = self.input.sim_input();

        self.accumulator += frame_seconds;
        while self.accumulator >= sim::DT {
            self.world.step(&input);
            self.accumulator -= sim::DT;
        }
        let blend = self.accumulator / sim::DT;

        let Some(graphics) = self.graphics.as_mut() else {
            return;
        };
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.graphics.is_some() {
            return;
        }
        match self.create_graphics(event_loop) {
            Ok(graphics) => self.graphics = Some(graphics),
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(graphics) = self.graphics.as_mut() {
                    graphics.surface.resize(size);
                }
            }
            WindowEvent::Focused(false) => self.input.release_all(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(key) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                if let Some(action) = self.input.key(&self.bindings, key, pressed) {
                    self.command(action, event_loop);
                }
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

/// What `--screenshot` renders.
pub struct Screenshot {
    pub path: PathBuf,
    pub assets: Option<PathBuf>,
    pub backends: wgpu::Backends,
    pub width: u32,
    pub height: u32,
    /// Simulation steps to run before the picture is taken.
    pub ticks: u32,
    /// The movement input held during those steps.
    pub movement: Vec2,
}

/// Runs the game for a while with no window, draws one frame off-screen and saves
/// it as a PNG. Needs a GPU or a software rasteriser, but no display.
pub fn screenshot(options: Screenshot) -> Result<()> {
    let assets_dir = assets::find_dir(options.assets.as_deref())?;

    let mut world = World::new(Room::test_room());
    let input = sim::Input {
        movement: options.movement.clamp_length_max(1.0),
    };
    for _ in 0..options.ticks {
        world.step(&input);
    }

    let (device, queue) = pollster::block_on(render2d::headless_device(options.backends))?;
    let mut renderer = Renderer2d::new(&device, &queue, render2d::OFFSCREEN_FORMAT);
    let mut view = View::new(&mut renderer, &assets_dir)?;
    view.set_room(&mut renderer, &world.room);

    let offscreen = Offscreen::new(&device, options.width, options.height);
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("screenshot") });
    view.draw(&mut renderer, &mut encoder, offscreen.target(), &world, 1.0, 0.0);
    queue.submit(std::iter::once(encoder.finish()));

    let pixels = offscreen.read_rgba(&device, &queue)?;
    save_png(&options.path, options.width, options.height, &pixels)?;

    let position = world.player.position;
    println!(
        "wrote {} ({}x{}), tick {}, player at ({:.3}, {:.3})",
        options.path.display(),
        options.width,
        options.height,
        world.tick,
        position.x,
        position.y
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
