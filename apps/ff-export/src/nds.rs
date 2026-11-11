//! Nintendo DS ROM access: header, file name table (FNT) and file allocation table (FAT).
//!
//! This game's FNT is a non-standard variant of the usual Nitro layout. Instead of
//! `[type][nameLen][name][u16 id]` it stores:
//!
//! * `[type][name]` — for files: no id field at all, ids are implicit and
//!   sequential from that directory's `firstFileId`;
//! * `[type][name][u16 0xF000 | dirId]` — for directories.
//!
//! The low 7 bits of `type` are the name length and bit 7 marks a directory.
//! The layout was re-derived from this ROM's bytes and cross-checked against the
//! FAT (8,279 entries, 8 reserved, 8,271 real files across 59 directories), which
//! is why the consistency checks below are worth keeping.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::bytes::{u16_at, u32_at};

pub const HEADER_LEN: usize = 0x200;

#[derive(Debug, Clone)]
pub struct Header {
    pub title: String,
    pub game_code: String,
    pub maker_code: String,
    pub file_size: usize,
    pub arm9_offset: u32,
    pub arm9_size: u32,
    pub arm7_offset: u32,
    pub arm7_size: u32,
    pub fnt_offset: u32,
    pub fnt_size: u32,
    pub fat_offset: u32,
    pub fat_size: u32,
}

#[derive(Debug, Clone)]
pub struct RomFile {
    /// FAT index, which is also the id the FNT walk assigns implicitly.
    pub id: usize,
    /// Path inside the ROM, `/` separated, e.g. `model/battle/arcdin`.
    pub path: String,
    pub start: u32,
    pub size: u32,
}

pub struct Rom {
    data: Vec<u8>,
    pub header: Header,
    pub files: Vec<RomFile>,
}

fn ascii(data: &[u8], offset: usize, len: usize) -> String {
    let end = (offset + len).min(data.len());
    if offset >= end {
        return String::new();
    }
    let raw = &data[offset..end];
    let stop = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..stop]).into_owned()
}

impl Rom {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(data).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.len() < HEADER_LEN {
            bail!(
                "file is only {} bytes, too short to hold a {:#x} byte DS header",
                data.len(),
                HEADER_LEN
            );
        }

        let header = Header {
            title: ascii(&data, 0x00, 12),
            game_code: ascii(&data, 0x0C, 4),
            maker_code: ascii(&data, 0x10, 2),
            file_size: data.len(),
            arm9_offset: u32_at(&data, 0x20)?,
            arm9_size: u32_at(&data, 0x2C)?,
            arm7_offset: u32_at(&data, 0x30)?,
            arm7_size: u32_at(&data, 0x3C)?,
            fnt_offset: u32_at(&data, 0x40)?,
            fnt_size: u32_at(&data, 0x44)?,
            fat_offset: u32_at(&data, 0x48)?,
            fat_size: u32_at(&data, 0x4C)?,
        };

        let files = read_files(&data, &header)?;
        Ok(Self {
            data,
            header,
            files,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// All files, in FAT id order.
    pub fn files(&self) -> &[RomFile] {
        &self.files
    }

    /// Finds a file by its exact ROM path.
    pub fn find(&self, path: &str) -> Option<&RomFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// All files whose path starts with `prefix`.
    pub fn files_under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a RomFile> {
        self.files.iter().filter(move |f| f.path.starts_with(prefix))
    }

    /// The raw bytes of a file.
    pub fn bytes(&self, file: &RomFile) -> Result<&[u8]> {
        let start = file.start as usize;
        let end = start
            .checked_add(file.size as usize)
            .ok_or_else(|| anyhow::anyhow!("{} has an overflowing size", file.path))?;
        if end > self.data.len() {
            bail!(
                "{} spans {:#x}..{:#x}, past the end of a {} byte ROM",
                file.path,
                start,
                end,
                self.data.len()
            );
        }
        Ok(&self.data[start..end])
    }

    /// The first four bytes of a file, for magic-byte filtering.
    pub fn magic(&self, file: &RomFile) -> Result<[u8; 4]> {
        let bytes = self.bytes(file)?;
        let mut magic = [0u8; 4];
        let take = bytes.len().min(4);
        magic[..take].copy_from_slice(&bytes[..take]);
        Ok(magic)
    }
}

/// Walks the FNT to recover every file's path and id, then pairs each with its FAT
/// range. Directory names are collected in a first pass (a parent's name can appear
/// after its children in the table) and resolved into full paths afterwards.
fn read_files(data: &[u8], header: &Header) -> Result<Vec<RomFile>> {
    let fnt_offset = header.fnt_offset as usize;
    let fnt_end = fnt_offset
        .checked_add(header.fnt_size as usize)
        .ok_or_else(|| anyhow::anyhow!("FNT size overflows"))?;
    if fnt_end > data.len() {
        bail!(
            "FNT spans {:#x}..{:#x}, past the end of a {} byte ROM",
            fnt_offset,
            fnt_end,
            data.len()
        );
    }
    let fnt = &data[fnt_offset..fnt_end];

    let dir_count = u16_at(fnt, 6)? as usize;

    let mut subtable_offset = vec![0usize; dir_count];
    let mut first_file_id = vec![0usize; dir_count];
    let mut parent = vec![0usize; dir_count];
    for d in 0..dir_count {
        subtable_offset[d] = u32_at(fnt, d * 8)? as usize;
        first_file_id[d] = u16_at(fnt, d * 8 + 4)? as usize;
        parent[d] = (u16_at(fnt, d * 8 + 6)? & 0x0FFF) as usize;
    }

    // The root's parent slot holds the directory count, so the main table ends
    // exactly where the first subtable begins.
    if let Some(first) = subtable_offset.first() {
        if *first != dir_count * 8 {
            bail!(
                "FNT main table is {} bytes but the first subtable is at {:#x}",
                dir_count * 8,
                first
            );
        }
    }

    let mut directory_name: Vec<Option<String>> = vec![None; dir_count];
    // (directory, file name, implicit id)
    let mut raw: Vec<(usize, String, usize)> = Vec::new();

    for d in 0..dir_count {
        let mut offset = subtable_offset[d];
        if offset + 1 > fnt.len() {
            continue;
        }
        let mut next_id = first_file_id[d];

        while offset + 1 <= fnt.len() {
            let kind = fnt[offset];
            if kind == 0 {
                break;
            }
            let name_len = (kind & 0x7F) as usize;
            if name_len == 0 || offset + 1 + name_len > fnt.len() {
                break;
            }
            let name = String::from_utf8_lossy(&fnt[offset + 1..offset + 1 + name_len]).into_owned();
            offset += 1 + name_len;

            if kind & 0x80 != 0 {
                if offset + 2 > fnt.len() {
                    break;
                }
                let child = (u16_at(fnt, offset)? & 0x0FFF) as usize;
                offset += 2;
                if child < dir_count && child != d {
                    directory_name[child] = Some(name);
                }
            } else {
                raw.push((d, name, next_id));
                next_id += 1;
            }
        }
    }

    // Resolve each directory's path from its parent chain.
    let mut directory_path: Vec<Option<String>> = vec![None; dir_count];
    if dir_count > 0 {
        directory_path[0] = Some(String::new());
    }
    for _ in 0..32 {
        let mut progress = false;
        for d in 1..dir_count {
            if directory_path[d].is_some() {
                continue;
            }
            let mut p = parent[d];
            if p >= dir_count {
                p = 0;
            }
            let resolved = match &directory_name[d] {
                None => Some(String::new()),
                Some(name) => {
                    if p == d {
                        Some(format!("{name}/"))
                    } else {
                        directory_path[p]
                            .as_ref()
                            .map(|parent_path| format!("{parent_path}{name}/"))
                    }
                }
            };
            if resolved.is_some() {
                directory_path[d] = resolved;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    let fat_offset = header.fat_offset as usize;
    let fat_end = fat_offset
        .checked_add(header.fat_size as usize)
        .ok_or_else(|| anyhow::anyhow!("FAT size overflows"))?;
    if fat_end > data.len() {
        bail!(
            "FAT spans {:#x}..{:#x}, past the end of a {} byte ROM",
            fat_offset,
            fat_end,
            data.len()
        );
    }
    let fat_count = header.fat_size as usize / 8;

    let mut files = Vec::with_capacity(raw.len());
    for (directory, name, id) in raw {
        if id >= fat_count {
            continue;
        }
        let start = u32_at(data, fat_offset + id * 8)?;
        let end = u32_at(data, fat_offset + id * 8 + 4)?;
        if end < start {
            bail!("FAT entry {} ends ({:#x}) before it starts ({:#x})", id, end, start);
        }
        let prefix = directory_path
            .get(directory)
            .and_then(|path| path.as_deref())
            .unwrap_or("");
        files.push(RomFile {
            id,
            path: format!("{prefix}{name}"),
            start,
            size: end - start,
        });
    }

    Ok(files)
}
