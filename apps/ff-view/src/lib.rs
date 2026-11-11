//! Rendering a glTF scene with wgpu, as a library.
//!
//! Two binaries sit on top of this, and they are the same window:
//!
//! - `ff-view`, which opens a model file, an export folder, or nothing at all;
//! - `ff-browser`, which is that window with the folder panel always on.
//!
//! The modules below are deliberately layered: `math` and `scene` are plain data,
//! `loader` is the only one that knows about glTF, `library` is the only one that
//! knows what an export folder looks like, `renderer` is the only one that knows
//! about wgpu, and `ui` is the only one that opens a window.

pub mod anim;
pub mod camera;
pub mod library;
pub mod loader;
pub mod math;
pub mod renderer;
pub mod scene;
pub mod settings;
pub mod ui;
