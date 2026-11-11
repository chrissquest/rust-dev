//! crystal-engine: a general-purpose rendering engine on wgpu.
//!
//! The modules are layered, and each knows about as little as it can:
//!
//! - `math` is [`glam`] plus the few things it lacks (bounding boxes, NaN-safe helpers);
//! - `scene` and `anim` are plain data: nodes, meshes, materials, skins and clips;
//! - `camera` is an orbit camera fitted from a scene's bounds;
//! - `loader` is the only module that knows about glTF;
//! - `terrain` loads heightmap + splat-map folders into the scene;
//! - `renderer` is the only module that knows about wgpu.
//!
//! None of them opens a window or owns an event loop: an app does that and hands
//! the engine a window to draw into.

pub mod anim;
pub mod camera;
pub mod loader;
pub mod math;
pub mod renderer;
pub mod scene;
pub mod terrain;

/// Re-exported so apps use the same `glam` version as the engine.
pub use glam;
