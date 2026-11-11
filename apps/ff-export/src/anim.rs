//! The animation entry: one clip's per-bone transforms.
//!
//! ```text
//! 0x00  4  f64  scale (20.12)         always 32.0 in this game
//! 0x04  4  u32  data offset           0x28, or 0x2c when a trailing field is present
//! 0x08  4  u32  first array size      bytes
//! 0x0c  4  u32  included models size  bytes
//! 0x10  4  u32  third array size      bytes
//! 0x14  4  u32  keyframes size        bytes: 8 + boneCount * frameCount * 48
//! 0x18  4  u32  frame count
//! 0x1c  4  u32  unknown
//! 0x20  4  u32  unknown
//! 0x24  4  u32  unknown
//! 0x28     first array, then the included models, then the third array
//!          then the keyframes:
//!            u32 bone count
//!            u32 frame count
//!            boneCount * frameCount x 12 word 20.12 matrix
//! ```
//!
//! The keyframe block is **frame-major**: every bone of frame 0, then every bone of frame 1, and so
//! on, so a bone's transform for a frame is at `frame * boneCount + bone`.
//!
//! Breme's first clip is 21 bones by 257 frames: `21 * 257 * 48 = 259,056`, plus the 8 byte header is
//! 259,064 - and its entry is 259,248 bytes, which is that plus a 184 byte preamble. The arithmetic
//! pinning the layout to the file size is not a coincidence.
//!
//! The matrices are absolute: they are the same space as the mesh's bind matrices (all joints are
//! children of the skeleton root), so they can drive glTF joint nodes directly.

use anyhow::{Result, bail};

use crate::bytes::{Reader, u32_at};
use crate::fixed;
use crate::matrix::Matrix4x3;

/// The default entry header length, when the data offset is 0x28.
pub const HEADER_LEN: usize = 0x28;

/// Bytes per keyframe matrix: twelve 20.12 words.
pub const MATRIX_BYTES: usize = 48;

#[derive(Debug, Clone, Default)]
pub struct Animation {
    /// Frames in the clip.
    pub frame_count: usize,
    /// Bones the clip animates; this matches the mesh's bone table order.
    pub bone_count: usize,
    /// Models the clip expects to be present.
    pub included_models: Vec<u32>,
    /// Frame-major keyframes: `frame * bone_count + bone`.
    pub transforms: Vec<Matrix4x3>,
}

impl Animation {
    /// The transform of one bone at one frame.
    pub fn transform(&self, bone: usize, frame: usize) -> Option<&Matrix4x3> {
        if bone >= self.bone_count || frame >= self.frame_count {
            return None;
        }
        self.transforms.get(frame * self.bone_count + bone)
    }

    /// Frames expressed in seconds, at the 60 fps the game and the reference both use.
    pub fn times(&self) -> Vec<f32> {
        (0..self.frame_count)
            .map(|frame| frame as f32 / 60.0)
            .collect()
    }
}

pub fn parse(data: &[u8]) -> Result<Animation> {
    if data.len() < HEADER_LEN {
        bail!(
            "animation is shorter than its {HEADER_LEN:#x} byte header ({} bytes)",
            data.len()
        );
    }

    let scale = fixed::from_2012(u32_at(data, 0x00)?);
    let data_offset = u32_at(data, 0x04)? as usize;
    let first_array_size = u32_at(data, 0x08)? as usize;
    let included_models_size = u32_at(data, 0x0c)? as usize;
    let third_array_size = u32_at(data, 0x10)? as usize;
    let keyframes_size = u32_at(data, 0x14)? as usize;
    let declared_frames = u32_at(data, 0x18)? as usize;

    if data_offset != HEADER_LEN && data_offset != HEADER_LEN + 4 {
        bail!("animation data offset is {data_offset:#x}, expected {HEADER_LEN:#x} or {:#x}", HEADER_LEN + 4);
    }
    if scale.is_finite() && scale != 32.0 {
        // not fatal, but every clip in this game measures 32.0
        #[allow(clippy::print_stderr)]
        {
            eprintln!("note: animation scale is {scale}, expected 32");
        }
    }

    let mut included_models = Vec::new();
    let models_offset = data_offset + first_array_size;
    for index in 0..included_models_size / 4 {
        included_models.push(u32_at(data, models_offset + index * 4)?);
    }

    let mut animation = Animation {
        included_models,
        ..Default::default()
    };

    if keyframes_size == 0 {
        // a valid clip with no keyframes; nothing to animate
        animation.frame_count = declared_frames;
        return Ok(animation);
    }

    let keyframes_offset = data_offset + first_array_size + included_models_size + third_array_size;
    let mut reader = Reader::at(data, keyframes_offset);
    let bone_count = reader.u32()? as usize;
    let frame_count = reader.u32()? as usize;

    let expected = 8 + bone_count * frame_count * MATRIX_BYTES;
    if expected != keyframes_size {
        bail!(
            "keyframe block is {keyframes_size} bytes but {bone_count} bones by {frame_count} frames \
             needs exactly {expected}"
        );
    }

    let mut transforms = Vec::with_capacity(bone_count * frame_count);
    for index in 0..bone_count * frame_count {
        let matrix = Matrix4x3::read(&mut reader)?;
        if !matrix.is_finite() {
            bail!("keyframe {index} (bone {}, frame {}) is not finite", index % bone_count, index / bone_count);
        }
        transforms.push(matrix);
    }

    animation.transforms = transforms;
    animation.bone_count = bone_count;
    animation.frame_count = frame_count;

    Ok(animation)
}
