//! The catalog: the `manifest.json` that describes everything an export produced.
//!
//! This file is the index a browser or an engine reads instead of walking a directory tree - one entry
//! per creature, each naming its `.glb`, its clips, its texture sheets and its extents. It is written
//! next to the files it describes, so the paths inside it are relative to the export directory and a
//! dump of that directory can be read anywhere.
//!
//! Naming lives here too, in [`slug`], [`glb_path`] and [`texture_dir`], because the names in the
//! catalog and the names on disk have to agree and only one place should decide them.

use serde::Serialize;

/// A filesystem-safe form of a creature's own name: `T-Rex` becomes `trex`, `Daspleto` stays
/// `daspleto`.
///
/// The names come from the game's text and contain letters, digits and the occasional hyphen. They end
/// up in file names, so everything else is dropped rather than trusted - and the creature's id is always
/// part of the file name too, which is what keeps `Slug`-collisions from being possible.
pub fn slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    if slug.is_empty() {
        "vivosaur".to_string()
    } else {
        slug
    }
}

/// Where one clip's `.glb` goes, relative to the export directory: `glb/breme_030_attack.glb`.
///
/// A clip is the unit because a clip is the unit in the game's own data: each of the creature's clips
/// names its own mesh, and those meshes differ - different bind poses for the same bones, and for two
/// creatures a different number of bones - so a mesh and its animation only make sense together.
pub fn glb_path(id: usize, name: &str, clip: &str) -> String {
    format!("glb/{}_{:03}_{}.glb", slug(name), id, clip)
}

/// Where a creature's loose texture sheets go: `textures/breme_030`.
///
/// The textures are per creature: all of a creature's clips share one texture set.
pub fn texture_dir(id: usize, name: &str) -> String {
    format!("textures/{}_{:03}", slug(name), id)
}

/// Where one sheet goes inside [`texture_dir`].
pub fn texture_path(id: usize, name: &str, image: &str) -> String {
    format!("{}/{}.png", texture_dir(id, name), image)
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub generator: String,
    pub source: Source,
    pub units: Units,
    /// How the files on disk are arranged, for whoever reads this.
    pub layout: Layout,
    pub creatures: Vec<Creature>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    /// The ROM the export came from, as it was named on disk.
    pub rom: String,
    /// The archive inside it that holds every creature.
    pub archive: String,
    pub creature_count: usize,
    /// Clips, i.e. `.glb` files: one per clip, because one mesh goes with one animation.
    pub clip_count: usize,
}

/// How to read the numbers in an entry.
#[derive(Debug, Clone, Serialize)]
pub struct Units {
    pub length: String,
    pub mesh_header_scale: f64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Layout {
    pub glb: String,
    pub textures: String,
}

impl Catalog {
    /// The layout note, in one place.
    pub fn layout() -> Layout {
        Layout {
            glb: "one file per clip, `glb/<name>_<id>_<clip>.glb`. The game stores one mesh per clip, \
                  each with its own bind pose - the same creature's meshes differ by up to 14 units in \
                  this game - so a mesh and its animation are one asset and cannot be merged into a \
                  single file without changing what the animation does."
                .to_string(),
            textures: "the creature's sheets as PNGs, grouped by creature: every clip of a creature \
                       shares one texture set, and the same images are embedded in each of its glbs."
                .to_string(),
        }
    }
    /// The scale documentation, in one place.
    pub fn units() -> Units {
        Units {
            length: "game units, as authored".to_string(),
            // every mesh in this game has this word, and it is not applied to the vertices
            mesh_header_scale: 32.0,
            note: "Vertex positions are exported exactly as the game stores them - 4.12 fixed point \
                   divided by 4096 - with no extra scaling. A creature is roughly 4 by 9 by 10 units, \
                   and each entry's `bounds` gives its own extents so a viewer can frame it. Every \
                   mesh carries a header scale word of 32.0, which the reference notes as `for ff1, \
                   always 32`; it describes how the game's own matrices were authored and is \
                   deliberately not applied here. Nothing maps these units to real-world lengths."
                .to_string(),
        }
    }

    /// The catalog as JSON, ready to write to `manifest.json`.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

/// One creature, as the catalog describes it: its sheets, and one entry per clip.
#[derive(Debug, Clone, Serialize)]
pub struct Creature {
    /// 1..=116, the game's own numbering, which is also the creature name table's order.
    pub id: usize,
    pub name: String,
    pub slug: String,
    /// The sheets every clip of this creature uses.
    pub textures: Vec<Texture>,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub size: [f64; 3],
}

/// One clip: one `.glb`, holding that clip's mesh, skeleton and animation.
#[derive(Debug, Clone, Serialize)]
pub struct Clip {
    /// The clip's slot in the game's data, 1..=7.
    pub slot: usize,
    /// The name it carries in the glb, e.g. `attack`.
    pub name: String,
    pub frames: usize,
    pub seconds: f64,
    /// The `.glb`, relative to the export directory.
    pub glb: String,
    /// The archive entries this clip was built from, so anything can be traced back to the ROM.
    pub mesh_entry: u32,
    pub animation_entry: u32,
    /// The sheet set, which is the creature's - repeated here so one clip describes itself.
    pub texture_entry: u32,
    pub bones: usize,
    /// Vertices in the glb, i.e. after the per-corner UV split.
    pub vertices: usize,
    pub triangles: usize,
    pub material_groups: usize,
    /// Bind-pose extents of this clip's mesh, in the same units as the vertices.
    pub bounds: Bounds,
}

#[derive(Debug, Clone, Serialize)]
pub struct Texture {
    /// The image's own name from the texture block, e.g. `din030_a`.
    pub name: String,
    /// The loose PNG, relative to the export directory.
    pub file: String,
    pub width: usize,
    pub height: usize,
    /// `eight-bit`, `four-bit`, and so on.
    pub format: String,
    /// Whether the image carries the DS colour key, i.e. whether its pixels have transparency.
    pub transparent: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_plain_lowercase_slugs() {
        assert_eq!(slug("Breme"), "breme");
        assert_eq!(slug("T-Rex"), "trex");
        assert_eq!(slug("S-Raptor"), "sraptor");
        assert_eq!(slug("Daspleto"), "daspleto");
        // nothing usable left, so it falls back rather than producing an empty file name
        assert_eq!(slug("!!!"), "vivosaur");
    }

    #[test]
    fn the_id_and_the_clip_are_always_in_the_file_name() {
        // two creatures can share a name - Frigi and Igno appear twice in the game's own table - so the
        // file name has to carry the id as well
        assert_eq!(glb_path(108, "Frigi", "attack"), "glb/frigi_108_attack.glb");
        assert_eq!(glb_path(115, "Frigi", "attack"), "glb/frigi_115_attack.glb");
        assert_eq!(
            glb_path(30, "Breme", "hurt-critical"),
            "glb/breme_030_hurt-critical.glb"
        );
        assert_eq!(
            texture_path(30, "Breme", "din030_a"),
            "textures/breme_030/din030_a.png"
        );
    }

    #[test]
    fn the_units_block_documents_the_scale() {
        let units = Catalog::units();
        assert_eq!(units.mesh_header_scale, 32.0);
        assert!(units.note.contains("not applied"));
    }

    #[test]
    fn the_layout_block_explains_the_one_file_per_clip() {
        let layout = Catalog::layout();
        assert!(layout.glb.contains("one file per clip"));
        assert!(layout.textures.contains("one texture set"));
    }
}
