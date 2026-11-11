//! crystal-engine: a general-purpose rendering engine on wgpu.
//!
//! The modules are layered, and each knows about as little as it can:
//!
//! - `math` is [`glam`] plus the few things it lacks (bounding boxes, NaN-safe helpers);
//! - `scene` and `anim` are plain data: nodes, meshes, materials, skins and clips;
//! - `camera` is an orbit camera fitted from a scene's bounds;
//! - `loader` is the only module that knows about glTF;
//! - `terrain` loads heightmap + splat-map folders into the scene;
//! - `renderer` draws 3D scenes with wgpu;
//! - `sprite`, `tilemap` and `camera2d` are the plain-data half of the 2D layer:
//!   textured quads, tile grids and a straight-down camera;
//! - `render2d` draws sprites with wgpu.
//!
//! - `surface` is a window's swapchain and device with no pipelines attached, for
//!   apps that draw only through `render2d`.
//!
//! `renderer`, `render2d` and `surface` are the only modules that know about wgpu.
//!
//! None of them opens a window or owns an event loop: an app does that and hands
//! the engine a window to draw into.

pub mod anim;
pub mod camera;
pub mod camera2d;
pub mod loader;
pub mod math;
pub mod render2d;
pub mod renderer;
pub mod scene;
pub mod sprite;
pub mod surface;
pub mod terrain;
pub mod tilemap;

/// Re-exported so apps use the same `glam` version as the engine.
pub use glam;
