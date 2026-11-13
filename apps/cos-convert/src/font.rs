//! Converts the game's pixel font into glyph sprites and `font.json`.
//!
//! The Unity project ships Thaleah Fat twice: as a TTF with a TextMeshPro
//! distance-field asset, which is what its text components use, and as a ready
//! 16-pixel bitmap atlas (`ThaleahFat.fontsettings` beside `ThaleahFat.png`). The
//! atlas is taken: it is the same pixel shapes, and it needs no rasteriser.
//!
//! Each glyph becomes an ordinary sprite named `font_<code point>`, whose pivot is
//! the pen position on the baseline and whose size is in ems (16 pixels per unit),
//! so the game draws text with the sprite code it already has.
//!
//! The atlas holds every glyph upside down (Unity pairs the top of a glyph with
//! the lower edge of its rectangle). The picture is copied as it is, so the game
//! draws glyph sprites flipped top to bottom, and a glyph's pivot is given for
//! the picture as stored: measured from the glyph's top.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value as Json, json};

use crate::json::number;
use crate::project::{Project, SpriteInfo};

const SETTINGS: &str = "Thaleah_PixelFont/ThaleahFat.fontsettings";
const TEXTURE: &str = "Thaleah_PixelFont/Materials/ThaleahFat.png";

pub struct Converted {
    pub json: Json,
    pub sprites: Vec<SpriteInfo>,
    pub warnings: Vec<String>,
    pub glyphs: usize,
}

pub fn convert(project: &Project) -> Result<Converted> {
    let documents = project.documents(Path::new(SETTINGS))?;
    let font = &documents.first().context("the font settings file is empty")?.body;
    let (width, height) = png_size(&project.assets_dir().join(TEXTURE))?;
    let em = font.get("m_FontSize").f32();
    if em <= 0.0 {
        bail!("{SETTINGS} has no font size");
    }

    let mut glyphs = Map::new();
    let mut sprites = Vec::new();
    let mut rotated = String::new();
    for glyph in font.get("m_CharacterRects").seq() {
        let Some(character) = u32::try_from(glyph.get("index").i64()).ok().and_then(char::from_u32) else {
            continue;
        };
        if glyph.get("flipped").bool() {
            // Stored turned a quarter in the atlas, which a sprite cannot undo.
            rotated.push(character);
            continue;
        }
        // Unity gives both rectangles a negative height: `y` is the upper edge.
        // `vert` is in pixels from the pen position on the baseline, so `vert.y`
        // is how far the glyph's top is above it.
        let (uv, vert) = (glyph.get("uv"), glyph.get("vert"));
        let glyph_width = (uv.get("width").f32() * width as f32).round();
        let glyph_height = (-uv.get("height").f32() * height as f32).round();
        if glyph_width < 1.0 || glyph_height < 1.0 {
            continue;
        }
        let left = (uv.get("x").f32() * width as f32).round();
        let top = height as f32 - (uv.get("y").f32() * height as f32).round();

        let name = format!("font_{}", character as u32);
        sprites.push(SpriteInfo {
            name: name.clone(),
            texture: TEXTURE.into(),
            rect: [left as u32, top as u32, glyph_width as u32, glyph_height as u32],
            pivot: [-vert.get("x").f32() / glyph_width, vert.get("y").f32() / glyph_height],
            pixels_per_unit: em,
        });
        glyphs.insert(
            character.to_string(),
            json!({ "sprite": name, "advance": number(glyph.get("advance").f32() / em) }),
        );
    }

    let mut warnings = Vec::new();
    if !rotated.is_empty() {
        warnings.push(format!("font glyphs left out because the atlas stores them rotated: {rotated}"));
    }
    Ok(Converted {
        glyphs: glyphs.len(),
        json: json!({
            "line_height": number(font.get("m_LineSpacing").f32() / em),
            "glyphs": glyphs,
        }),
        sprites,
        warnings,
    })
}

/// A PNG's width and height, from its header.
fn png_size(path: &Path) -> Result<(u32, u32)> {
    let bytes = std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    if bytes.len() < 24 || &bytes[1..4] != b"PNG" {
        bail!("{} is not a PNG", path.display());
    }
    let field = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Ok((field(16), field(20)))
}
