//! Fossil Fighters (NDS) asset extraction.
//!
//! The pipeline, outermost format first:
//!
//! ```text
//! NDS ROM
//!   file      ->  "MAR\0" container                        (mar)
//!     entry   ->  "MCM\0" chunk wrapper                    (mcm)
//!       chunk ->  codec: none | LZ10 | ...                 (codec)
//!         payload -> mesh / animation / texture / 3CL manifest
//! ```
//!
//! Every layer is validated against the corpora produced earlier in the companion
//! `chris-wgpu` project, so "done" is a byte comparison rather than an opinion.
//!
//! Note on provenance: this is an independent implementation. The formats were
//! derived from byte-level analysis of this ROM, from the Nintendo DS hardware
//! documentation, and from the format notes in `externals/documentation`.
//! No source code from other fan tools is copied here.

pub mod anim;
pub mod bytes;
pub mod catalog;
pub mod codec;
pub mod fixed;
pub mod gltf_out;
pub mod gpu;
pub mod manifest;
pub mod mar;
pub mod matrix;
pub mod mcm;
pub mod mesh;
pub mod names;
pub mod nds;
pub mod texture;

/// Magic bytes that mark an archive this crate can unpack.
pub const MAR_MAGIC: &[u8; 3] = b"MAR";
/// Magic bytes that mark the per-entry wrapper inside an archive.
pub const MCM_MAGIC: &[u8; 3] = b"MCM";
