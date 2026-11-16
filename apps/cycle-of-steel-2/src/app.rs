//! The window, the frame loop, and the windowless screenshot mode.
//!
//! The same shape as Cycle of Steel 1's app: real time fills an accumulator,
//! the world steps once per `sim::DT` it holds, and the leftover fraction tells
//! the view how far to blend towards the newest step. Everything that needs a
//! window lives in [`Graphics`], which Android takes away whenever the app
//! leaves the screen; the world carries on untouched.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use crystal_engine::glam::Vec2;
use crystal_engine::render2d::{self, Offscreen, Renderer2d, Target};
use crystal_engine::surface::WindowSurface;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::sim::{self, Input, World};
use crate::view::View;

/// The most simulated time one frame may catch up on: after a long stall the
/// game slows down rather than running hundreds of steps at once.
const MAX_CATCH_UP: f32 = 0.25;

struct Graphics {
    window: Arc<Window>,
    surface: WindowSurface,
    renderer: Renderer2d,
    view: View,
}

/// Keys and mouse buttons held, and where the cursor is.
///
/// WASD moves. The mouse aims from the middle of the window (the camera
/// follows the player) and the left button or Space fires. The arrow keys aim
/// and fire at once, for playing on a keyboard alone.
#[derive(Default)]
struct Keys {
    /// Left, right, down, up, for movement and for the arrow keys.
    movement: [bool; 4],
    arrows: [bool; 4],
    space: bool,
    mouse_button: bool,
    /// Cursor relative to the window's centre, y up; `None` until it moves.
    cursor: Option<Vec2>,
}

fn axes(held: [bool; 4]) -> Vec2 {
    let axis = |negative: bool, positive: bool| f32::from(u8::from(positive)) - f32::from(u8::from(negative));
    Vec2::new(axis(held[0], held[1]), axis(held[2], held[3])).normalize_or_zero()
}

impl Keys {
    fn set(&mut self, code: KeyCode, down: bool) {
        match code {
            KeyCode::KeyA => self.movement[0] = down,
            KeyCode::KeyD => self.movement[1] = down,
            KeyCode::KeyS => self.movement[2] = down,
            KeyCode::KeyW => self.movement[3] = down,
            KeyCode::ArrowLeft => self.arrows[0] = down,
            KeyCode::ArrowRight => self.arrows[1] = down,
            KeyCode::ArrowDown => self.arrows[2] = down,
            KeyCode::ArrowUp => self.arrows[3] = down,
            KeyCode::Space => self.space = down,
            _ => {}
        }
    }

    fn input(&self) -> Input {
        let arrows = axes(self.arrows);
        let aim = if arrows != Vec2::ZERO { arrows } else { self.cursor.unwrap_or(Vec2::ZERO) };
        Input {
            movement: axes(self.movement),
            aim,
            fire: self.space || self.mouse_button || arrows != Vec2::ZERO,
        }
    }
}

struct App {
    backends: wgpu::Backends,
    world: World,
    keys: Keys,
    graphics: Option<Graphics>,
    last_frame: Option<Instant>,
    accumulator: f32,
    error: Option<anyhow::Error>,
}

/// Opens the window and runs the game until it is closed.
pub fn run(backends: wgpu::Backends, event_loop: EventLoop<()>) -> Result<()> {
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        backends,
        world: World::new(1),
        keys: Keys::default(),
        graphics: None,
        last_frame: None,
        accumulator: 0.0,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

impl App {
    fn create_graphics(&self, event_loop: &ActiveEventLoop) -> Result<Graphics> {
        // Android ignores the title and sizes: the window is the whole screen.
        let attributes = Window::default_attributes()
            .with_title("Cycle of Steel 2")
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(320.0, 180.0));
        let window = Arc::new(event_loop.create_window(attributes).context("could not create the window")?);
        let surface = pollster::block_on(WindowSurface::new(window.clone(), self.backends))?;
        log::info!("drawing with {} ({:?})", surface.adapter_name(), surface.backend());
        let mut renderer = Renderer2d::new(surface.device(), surface.queue(), surface.format());
        let view = View::new(&mut renderer)?;
        Ok(Graphics {
            window,
            surface,
            renderer,
            view,
        })
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let elapsed = self.last_frame.map_or(0.0, |last| (now - last).as_secs_f32());
        self.last_frame = Some(now);
        self.accumulator = (self.accumulator + elapsed).min(MAX_CATCH_UP);
        let input = self.keys.input();
        while self.accumulator >= sim::DT {
            self.world.step(&input);
            self.accumulator -= sim::DT;
        }
        let blend = self.accumulator / sim::DT;

        let Some(graphics) = self.graphics.as_mut() else { return };
        let frame = match graphics.surface.acquire() {
            Ok(frame) => frame,
            // Nothing to draw into this time (minimised, mid-resize): try next frame.
            Err(_) => return,
        };
        let mut encoder = graphics
            .surface
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let target = Target {
            view: &frame.view,
            width: frame.width,
            height: frame.height,
        };
        graphics.view.draw(&mut graphics.renderer, &mut encoder, target, &self.world, blend);
        graphics.surface.present(frame, encoder);
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
        // The surface is about to become invalid: drop everything built on it.
        self.graphics = None;
        self.keys = Keys::default();
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
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        ..
                    },
                ..
            } => {
                if code == KeyCode::Escape && state == ElementState::Pressed {
                    event_loop.exit();
                }
                self.keys.set(code, state == ElementState::Pressed);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(graphics) = &self.graphics {
                    let size = graphics.window.inner_size();
                    let center = Vec2::new(size.width as f32, size.height as f32) * 0.5;
                    self.keys.cursor = Some(Vec2::new(position.x as f32 - center.x, center.y - position.y as f32));
                }
            }
            // Touch, for phones until there are proper sticks: hold to aim at
            // the finger and fire.
            WindowEvent::Touch(touch) => {
                if let Some(graphics) = &self.graphics {
                    let size = graphics.window.inner_size();
                    let center = Vec2::new(size.width as f32, size.height as f32) * 0.5;
                    let at = Vec2::new(touch.location.x as f32 - center.x, center.y - touch.location.y as f32);
                    let down = matches!(touch.phase, TouchPhase::Started | TouchPhase::Moved);
                    self.keys.cursor = Some(at);
                    self.keys.mouse_button = down;
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.keys.mouse_button = state == ElementState::Pressed,
            // Released keys stop counting when the window loses focus.
            WindowEvent::Focused(false) => self.keys = Keys::default(),
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(graphics) = &self.graphics {
            graphics.window.request_redraw();
        }
    }
}

pub struct Screenshot {
    pub path: PathBuf,
    pub backends: wgpu::Backends,
    pub width: u32,
    pub height: u32,
    /// Simulation steps to run before the picture is taken.
    pub ticks: u32,
    /// The movement held during those steps.
    pub movement: Vec2,
    /// The aim held during those steps, and whether the trigger is.
    pub aim: Vec2,
    pub fire: bool,
}

/// Steps the world with no window, draws one frame off-screen and saves it as
/// a PNG. Needs a GPU or a software rasteriser, but no display.
pub fn screenshot(options: Screenshot) -> Result<()> {
    let mut world = World::new(1);
    let input = Input {
        movement: options.movement,
        aim: options.aim,
        fire: options.fire,
    };
    for _ in 0..options.ticks {
        world.step(&input);
    }

    let (device, queue) = pollster::block_on(render2d::headless_device(options.backends))?;
    let mut renderer = Renderer2d::new(&device, &queue, render2d::OFFSCREEN_FORMAT);
    let mut view = View::new(&mut renderer)?;
    let offscreen = Offscreen::new(&device, options.width, options.height);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("screenshot") });
    view.draw(&mut renderer, &mut encoder, offscreen.target(), &world, 1.0);
    queue.submit(std::iter::once(encoder.finish()));

    let pixels = offscreen.read_rgba(&device, &queue)?;
    save_png(&options.path, options.width, options.height, &pixels)?;
    let position = world.player.position;
    println!(
        "wrote {} ({}x{}), tick {}, player at ({:.3}, {:.3}), {} bullets, {} numbers",
        options.path.display(),
        options.width,
        options.height,
        world.tick,
        position.x,
        position.y,
        world.bullets.len(),
        world.feedback.numbers.len()
    );
    Ok(())
}

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file = std::fs::File::create(path).with_context(|| format!("could not create {}", path.display()))?;
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

    #[test]
    fn opposite_keys_cancel_and_arrows_aim_and_fire() {
        let mut keys = Keys::default();
        keys.set(KeyCode::KeyA, true);
        keys.set(KeyCode::KeyD, true);
        assert_eq!(keys.input().movement, Vec2::ZERO);
        assert!(!keys.input().fire);
        keys.cursor = Some(Vec2::new(0.0, 50.0));
        assert_eq!(keys.input().aim, Vec2::new(0.0, 50.0));
        keys.set(KeyCode::ArrowLeft, true);
        let input = keys.input();
        assert!(input.fire);
        assert_eq!(input.aim, Vec2::new(-1.0, 0.0));
    }
}
