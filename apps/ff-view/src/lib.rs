//! A model viewer on top of `crystal-engine`, as a library.
//!
//! Two binaries sit on top of this, and they are the same window:
//!
//! - `ff-view`, which opens a model file, an export folder, or nothing at all;
//! - `ff-browser`, which is that window with the folder panel always on.
//!
//! The engine does the scene, the glTF loading and the drawing. What is left
//! here is the app: `library` is the only module that knows what an export
//! folder looks like, `settings` remembers the last one, and `ui` is the only
//! one that opens a window.

pub mod library;
pub mod settings;
pub mod ui;
