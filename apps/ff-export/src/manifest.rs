//! The `3CL` creature manifest.
//!
//! Entry 0 of `model/battle/arcdin` is a `3CL` file mapping each vivosaur to the archive entries
//! holding its mesh, animation and texture:
//!
//! ```text
//! 0x00  4  "3CL\0"
//! 0x04  4  u32 slot count          (117: slot 0 is unused, 1..=116 are creatures)
//! 0x08  4  u32 offset of slot table (0x0C), from the file start
//! 0x0C     slotCount x u32 slot-record offsets, from the file start
//!
//! slot record (at its offset):
//! 0x00  4  u32 clip slot count     (8 here: slot 0 unused, 1..=7 are clips)
//! 0x04  4  u32 offset of clip table (0x08), from the slot-record start
//! 0x08     clipSlotCount x u32 clip-record offsets, from the slot-record start
//!
//! clip record (at its offset):
//! 0x00  4  u32 valid flag          (0 = this clip slot is empty)
//! 0x04  4  u32 mesh entry index
//! 0x08  4  u32 mesh table name offset, from the clip-record start
//! 0x0C  4  u32 animation entry index
//! 0x10  4  u32 animation table name offset
//! 0x14  4  u32 texture entry index
//! 0x18  4  u32 texture table name offset
//!          the three NUL-terminated names, 4-byte aligned
//! ```
//!
//! A creature's clip slots are sparse: each creature has between 4 and 7 of the 7 usable slots
//! filled, which is why the exported clip files are named `vivosaur N animation M` for an
//! irregular set of `M`. Creature *names* are not in this file; they come from the game's text.

use anyhow::{Result, bail};

use crate::bytes::u32_at;

/// One named archive table a clip points into, e.g. `arcdin` entry 384.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableEntry {
    pub index: u32,
    pub table_name: String,
}

/// One animation group: which mesh, animation and texture this clip uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// The clip slot number, 1..=7. This is the `M` in `vivosaur N animation M`.
    pub slot: usize,
    pub mesh: TableEntry,
    pub animation: TableEntry,
    pub texture: TableEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Creature {
    /// 1..=116, matching the game's own numbering and the name table.
    pub id: usize,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub creatures: Vec<Creature>,
}

impl Manifest {
    pub fn clip_count(&self) -> usize {
        self.creatures.iter().map(|c| c.clips.len()).sum()
    }

    pub fn find(&self, id: usize) -> Option<&Creature> {
        self.creatures.iter().find(|c| c.id == id)
    }
}

/// What each clip slot is, established by comparing the exported clips against in-game footage.
///
/// Slot 1 is the full attack animation; the game truncates it at run time to present a one, two or
/// three hit attack, so what we export is the complete animation. Slot 2 is a distinct secondary
/// attack - only 36 of the 116 creatures have it - and it plays fully, unlike the combo in slot 1.
pub fn clip_label(slot: usize) -> &'static str {
    match slot {
        1 => "attack",
        2 => "attack-secondary",
        3 => "roar",
        4 => "cheer",
        5 => "hurt",
        6 => "hurt-critical",
        7 => "idle",
        _ => "unused",
    }
}

/// Reads a NUL-terminated name at `offset`.
fn name_at(data: &[u8], offset: usize) -> Result<String> {
    if offset >= data.len() {
        bail!("name offset {:#x} is past the end of the manifest", offset);
    }
    let rest = &data[offset..];
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

pub fn parse(data: &[u8]) -> Result<Manifest> {
    if data.len() < 12 {
        bail!("3CL manifest is too short ({} bytes)", data.len());
    }
    if &data[0..3] != b"3CL" {
        bail!("not a 3CL manifest (magic {:02X?})", &data[0..3]);
    }

    let slot_count = u32_at(data, 4)? as usize;
    let slot_table = u32_at(data, 8)? as usize;

    if slot_table + slot_count * 4 > data.len() {
        bail!(
            "3CL slot table ({:#x} + {} x 4) does not fit in {} bytes",
            slot_table,
            slot_count,
            data.len()
        );
    }

    let mut creatures = Vec::new();

    for slot in 0..slot_count {
        let record = u32_at(data, slot_table + slot * 4)? as usize;
        if record + 8 > data.len() {
            bail!("3CL slot {} record at {:#x} is out of range", slot, record);
        }

        let clip_count = u32_at(data, record)? as usize;
        let clip_table = u32_at(data, record + 4)? as usize;

        if record + clip_table + clip_count * 4 > data.len() {
            bail!("3CL slot {} clip table does not fit", slot);
        }

        let mut clips = Vec::new();
        for index in 0..clip_count {
            let clip_offset = record + u32_at(data, record + clip_table + index * 4)? as usize;
            if clip_offset + 0x1C > data.len() {
                bail!("3CL slot {} clip {} record is out of range", slot, index);
            }

            // Validity is explicit rather than implied by a zero offset.
            if u32_at(data, clip_offset)? == 0 {
                continue;
            }

            let mesh_index = u32_at(data, clip_offset + 0x04)?;
            let mesh_name = u32_at(data, clip_offset + 0x08)? as usize;
            let animation_index = u32_at(data, clip_offset + 0x0C)?;
            let animation_name = u32_at(data, clip_offset + 0x10)? as usize;
            let texture_index = u32_at(data, clip_offset + 0x14)?;
            let texture_name = u32_at(data, clip_offset + 0x18)? as usize;

            clips.push(Clip {
                slot: index,
                mesh: TableEntry {
                    index: mesh_index,
                    table_name: name_at(data, clip_offset + mesh_name)?,
                },
                animation: TableEntry {
                    index: animation_index,
                    table_name: name_at(data, clip_offset + animation_name)?,
                },
                texture: TableEntry {
                    index: texture_index,
                    table_name: name_at(data, clip_offset + texture_name)?,
                },
            });
        }

        // Slot 0 is unused, and a slot with no clips is not a creature.
        if !clips.is_empty() {
            creatures.push(Creature { id: slot, clips });
        }
    }

    Ok(Manifest { creatures })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(data: &mut [u8], at: usize, value: u32) {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Builds a manifest with two slots: slot 0 has no valid clips (so it must be dropped) and
    /// slot 1 has one empty clip slot followed by a valid one. The three table names differ, which
    /// proves each name offset is read from the right place.
    fn sample() -> Vec<u8> {
        let mut file = Vec::new();
        file.extend_from_slice(b"3CL\0");
        file.extend_from_slice(&2u32.to_le_bytes()); // slot count
        file.extend_from_slice(&0x0Cu32.to_le_bytes()); // slot table offset
        file.extend_from_slice(&[0u8; 8]); // slot table, patched below

        // slot 0: one clip slot, empty
        let slot0 = file.len();
        file.extend_from_slice(&[0u8; 12]);
        let slot0_clip = file.len();
        file.extend_from_slice(&[0u8; 28]);

        // slot 1: two clip slots, the second valid
        let slot1 = file.len();
        file.extend_from_slice(&[0u8; 16]);
        let slot1_clip0 = file.len();
        file.extend_from_slice(&[0u8; 28]);
        let slot1_clip1 = file.len();
        file.extend_from_slice(&[0u8; 28]);

        let names = file.len();
        file.extend_from_slice(b"arcdin\0\0"); // mesh table
        file.extend_from_slice(b"motion\0\0"); // animation table
        file.extend_from_slice(b"image\0\0\0"); // texture table

        put_u32(&mut file, 0x0C, slot0 as u32);
        put_u32(&mut file, 0x10, slot1 as u32);

        put_u32(&mut file, slot0, 1); // clip slot count
        put_u32(&mut file, slot0 + 4, 8); // clip table offset
        put_u32(&mut file, slot0 + 8, (slot0_clip - slot0) as u32);

        put_u32(&mut file, slot1, 2);
        put_u32(&mut file, slot1 + 4, 8);
        put_u32(&mut file, slot1 + 8, (slot1_clip0 - slot1) as u32);
        put_u32(&mut file, slot1 + 12, (slot1_clip1 - slot1) as u32);

        put_u32(&mut file, slot1_clip1, 1); // valid
        put_u32(&mut file, slot1_clip1 + 4, 384); // mesh index
        put_u32(&mut file, slot1_clip1 + 8, (names - slot1_clip1) as u32);
        put_u32(&mut file, slot1_clip1 + 12, 385); // animation index
        put_u32(&mut file, slot1_clip1 + 16, (names + 8 - slot1_clip1) as u32);
        put_u32(&mut file, slot1_clip1 + 20, 386); // texture index
        put_u32(&mut file, slot1_clip1 + 24, (names + 16 - slot1_clip1) as u32);

        file
    }

    #[test]
    fn reads_clips_and_table_names() {
        let manifest = parse(&sample()).unwrap();
        assert_eq!(manifest.creatures.len(), 1, "the empty slot must be dropped");

        let creature = &manifest.creatures[0];
        assert_eq!(creature.id, 1);
        assert_eq!(creature.clips.len(), 1, "the invalid clip slot must be skipped");

        let clip = &creature.clips[0];
        assert_eq!(clip.slot, 1);
        assert_eq!(clip.mesh, TableEntry { index: 384, table_name: "arcdin".into() });
        assert_eq!(clip.animation, TableEntry { index: 385, table_name: "motion".into() });
        assert_eq!(clip.texture, TableEntry { index: 386, table_name: "image".into() });
    }

    #[test]
    fn rejects_a_wrong_magic() {
        let mut file = sample();
        file[0] = b'X';
        assert!(parse(&file).is_err());
    }

    #[test]
    fn rejects_a_slot_table_that_does_not_fit() {
        let mut file = sample();
        put_u32(&mut file, 4, 10_000);
        assert!(parse(&file).is_err());
    }

    #[test]
    fn rejects_a_clip_record_out_of_range() {
        let mut file = sample();
        let slot1 = u32::from_le_bytes(file[0x10..0x14].try_into().unwrap()) as usize;
        put_u32(&mut file, slot1 + 12, 10_000); // second clip offset
        assert!(parse(&file).is_err());
    }
}
