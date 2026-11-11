//! Fossil Fighters-specific glue: which archive holds the models, and what the creatures are called.
//!
//! Small enough to look pointless, but it is the piece every command needs and no format module
//! should own: `nds`, `mar` and friends know nothing about *this* game.

use anyhow::{Context, Result};

use crate::{mar, names, nds};

/// Opens a ROM and the archive at `path` inside it.
pub fn open_archive<'a>(
    rom: &'a nds::Rom,
    path: &str,
) -> Result<(mar::Archive<'a>, &'a nds::RomFile)> {
    let file = rom
        .find(path)
        .with_context(|| format!("`{path}` is not in the ROM"))?;
    let bytes = rom.bytes(file)?;
    let archive = mar::Archive::parse(bytes).with_context(|| format!("parsing `{path}`"))?;
    Ok((archive, file))
}

/// Reads the creature name table out of the game's text archive.
pub fn creature_names(rom: &nds::Rom) -> Result<Vec<String>> {
    let (archive, _) = open_archive(rom, "text/japanese")?;
    let text = archive
        .decompressed(0)
        .context("decompressing text/japanese entry 0")?;
    names::creature_names(&text, names::CREATURE_COUNT)
}
