//! Cycle of Steel.
//!
//! The crate is split so that the game itself can run without a window or a GPU:
//!
//! - `sim` is the game: world state and one fixed-rate `step`. It knows nothing
//!   about winit, wgpu or wall-clock time, so tests can step it directly.
//! - `input` turns keys and gamepads into the `sim::Input` a step consumes.
//! - `view` turns world state into sprites and owns the camera.
//! - `assets` finds and loads the files under `assets/`.
//! - `app` is the window, the frame loop and the screenshot mode.

pub mod app;
pub mod assets;
pub mod input;
pub mod sim;
pub mod view;
