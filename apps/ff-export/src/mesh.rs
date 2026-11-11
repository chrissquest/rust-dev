//! The `Mesh_FF1` container: a vivosaur's geometry plus its skeleton.
//!
//! ```text
//! 0x00  4  f64  scale (20.12)        always 32.0 in this game
//! 0x04  4  u32  commands offset      0x28
//! 0x08  4  u32  commands length
//! 0x0C  4  u32  bone table length    (0 when absent)
//! 0x10  4  u32  model names length   (0 when absent)
//! 0x14  4  u32  unknown2
//! 0x18  4  u32  unknown3
//! 0x1C  4  u32  unknown4
//! 0x20  4  u32  keyframe count
//! 0x24  4  u32  unknown6
//! 0x28     the mesh command stream
//!          then the bone table:  u32 count, then count x (16 byte name + 12 word matrix)
//!          then the model names: u32 count, then count x 16 byte name
//! ```
//!
//! This is the entry the `3CL` manifest calls the *mesh* - for Breme that is `arcdin` entry 384
//! (12,852 bytes), not the much larger animation entry 385. Geometry is not stored as arrays: it
//! lives inside the command stream, which [`crate::gpu`] interprets.

use anyhow::{Result, bail};

use crate::bytes::Reader;
use crate::fixed;
use crate::matrix::Matrix4x3;

pub const HEADER_LEN: usize = 0x28;

#[derive(Debug, Clone)]
pub struct Bone {
    /// The game's own bone name, e.g. `waist`, `l_forearm`, `tail1`.
    pub name: String,
    /// Bind pose, as a transform of the bone's local space.
    pub matrix: Matrix4x3,
}

#[derive(Debug, Clone)]
pub struct Mesh {
    /// The header's first word: `32.0` in every mesh of this game.
    pub scale: f64,
    pub unknown2: u32,
    pub unknown3: u32,
    pub unknown4: u32,
    /// Frames the mesh was authored with; the animation entry carries the real clip lengths.
    pub keyframe_count: u32,
    pub unknown6: u32,
    /// Raw mesh command stream, interpreted by [`crate::gpu`].
    pub commands: Vec<u8>,
    pub bones: Vec<Bone>,
    /// The mesh's own part names, e.g. `din030_a`.
    pub model_names: Vec<String>,
}

pub fn parse(data: &[u8]) -> Result<Mesh> {
    if data.len() < HEADER_LEN {
        bail!(
            "mesh is shorter than its {HEADER_LEN:#x} byte header ({} bytes)",
            data.len()
        );
    }

    let mut reader = Reader::new(data);
    let scale = fixed::from_2012(reader.u32()?);
    let commands_offset = reader.u32()? as usize;
    let commands_length = reader.u32()? as usize;
    let bone_table_length = reader.u32()? as usize;
    let model_names_length = reader.u32()? as usize;
    let unknown2 = reader.u32()?;
    let unknown3 = reader.u32()?;
    let unknown4 = reader.u32()?;
    let keyframe_count = reader.u32()?;
    let unknown6 = reader.u32()?;

    let commands = reader.slice(commands_offset, commands_length)?.to_vec();

    let mut bones = Vec::new();
    if bone_table_length != 0 {
        let mut table = Reader::at(data, commands_offset + commands_length);
        let count = table.u32()? as usize;
        bones.reserve(count);
        for index in 0..count {
            let name = table.ascii_nul(16)?;
            let matrix = Matrix4x3::read(&mut table)?;
            if !matrix.is_finite() {
                bail!("bone {index} (`{name}`) has a non-finite bind matrix");
            }
            bones.push(Bone { name, matrix });
        }
    }

    let mut model_names = Vec::new();
    if model_names_length != 0 {
        let mut names = Reader::at(data, commands_offset + commands_length + bone_table_length);
        let count = names.u32()? as usize;
        model_names.reserve(count);
        for _ in 0..count {
            model_names.push(names.ascii_nul(16)?);
        }
    }

    Ok(Mesh {
        scale,
        unknown2,
        unknown3,
        unknown4,
        keyframe_count,
        unknown6,
        commands,
        bones,
        model_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(data: &mut [u8], at: usize, value: u32) {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_name(data: &mut [u8], at: usize, name: &str) {
        data[at..at + name.len()].copy_from_slice(name.as_bytes());
    }

    /// A header, a 4-byte command stream, one bone and one model name.
    fn sample() -> Vec<u8> {
        let commands_offset = HEADER_LEN;
        let commands_length = 4;
        let bone_table_length = 4 + 16 + 48; // count + name + 12-word matrix
        let model_names_length = 4 + 16;

        let mut file =
            vec![0u8; commands_offset + commands_length + bone_table_length + model_names_length];

        put_u32(&mut file, 0x00, fixed::to_2012(32.0));
        put_u32(&mut file, 0x04, commands_offset as u32);
        put_u32(&mut file, 0x08, commands_length as u32);
        put_u32(&mut file, 0x0C, bone_table_length as u32);
        put_u32(&mut file, 0x10, model_names_length as u32);
        put_u32(&mut file, 0x20, 257); // keyframe count
        file[commands_offset..commands_offset + commands_length].copy_from_slice(&[1, 2, 3, 4]);

        let bones = commands_offset + commands_length;
        put_u32(&mut file, bones, 1);
        put_name(&mut file, bones + 4, "waist");
        let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        for (index, value) in identity.iter().enumerate() {
            put_u32(&mut file, bones + 20 + index * 4, fixed::to_2012(*value));
        }

        let names = bones + bone_table_length;
        put_u32(&mut file, names, 1);
        put_name(&mut file, names + 4, "din030_a");

        file
    }

    #[test]
    fn reads_the_header_commands_bone_table_and_names() {
        let mesh = parse(&sample()).unwrap();
        assert_eq!(mesh.scale, 32.0);
        assert_eq!(mesh.keyframe_count, 257);
        assert_eq!(mesh.commands, vec![1, 2, 3, 4]);
        assert_eq!(mesh.bones.len(), 1);
        assert_eq!(mesh.bones[0].name, "waist");
        assert_eq!(mesh.bones[0].matrix, Matrix4x3::IDENTITY);
        assert_eq!(mesh.model_names, vec!["din030_a"]);
    }

    #[test]
    fn tolerates_absent_optional_tables() {
        let mut file = sample();
        put_u32(&mut file, 0x0C, 0); // no bone table
        put_u32(&mut file, 0x10, 0); // no model names
        let mesh = parse(&file).unwrap();
        assert!(mesh.bones.is_empty());
        assert!(mesh.model_names.is_empty());
    }

    #[test]
    fn rejects_a_truncated_header() {
        assert!(parse(&[0u8; 8]).is_err());
    }

    #[test]
    fn rejects_a_command_stream_that_runs_off_the_end() {
        let mut file = sample();
        put_u32(&mut file, 0x08, 10_000);
        assert!(parse(&file).is_err());
    }
}
