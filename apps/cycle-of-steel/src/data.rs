//! The asset files' formats: rooms and the sprite catalogue.
//!
//! These files were first written by `cos-convert` from the Unity project and are
//! now the source of truth. Everything is in room space: x right, y up, origin at
//! the room's bottom-left corner, in world units.
//!
//! Only what the game uses so far is read. A room file also lists `lights` and
//! `barriers`; those are skipped here until the steps that need them.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::assets::AssetSource;

/// `sprites.json`: every sprite by name.
#[derive(Debug, Deserialize)]
pub struct SpriteCatalogue {
    pub sprites: HashMap<String, SpriteEntry>,
}

/// One sprite: a rectangle of a texture and how it sits in the world.
#[derive(Clone, Debug, Deserialize)]
pub struct SpriteEntry {
    /// `sprites/<texture>.png`.
    pub texture: String,
    /// `[x, y, width, height]` in pixels, origin at the texture's top-left.
    pub rect: [u32; 4],
    /// The point placed at the sprite's position: (0, 0) bottom-left, (1, 1) top-right.
    pub pivot: [f32; 2],
    pub pixels_per_unit: f32,
}

/// `font.json`: the pixel font, one glyph sprite per character.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FontFile {
    /// From one line's baseline to the next, in ems.
    pub line_height: f32,
    pub glyphs: HashMap<char, GlyphFile>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GlyphFile {
    /// A key of `sprites.json`. Its pivot is the pen position on the baseline and
    /// its size is in ems. The picture is upside down, as Unity's atlas stores it:
    /// draw it flipped top to bottom.
    pub sprite: String,
    /// How far the pen moves on, in ems.
    pub advance: f32,
}

impl FontFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("font.json")?;
        serde_json::from_slice(&bytes).context("could not parse font.json")
    }
}

impl SpriteCatalogue {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("sprites.json")?;
        serde_json::from_slice(&bytes).context("could not parse sprites.json")
    }
}

/// `rooms/<name>.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct RoomFile {
    pub name: String,
    /// Columns and rows of cells.
    pub size: [u32; 2],
    /// World size of one cell.
    pub cell: f32,
    /// Sprite names; tiles refer to them by position in this list.
    pub palette: Vec<String>,
    /// Bottom layer first.
    pub layers: Vec<LayerFile>,
    /// Loose sprites, in draw order.
    #[serde(default)]
    pub decorations: Vec<DecorationFile>,
    /// Prefabs placed in the room: guns to pick up, enemies, chests, hazards.
    #[serde(default)]
    pub objects: Vec<ObjectFile>,
    #[serde(default)]
    pub markers: Vec<MarkerFile>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LayerFile {
    pub name: String,
    /// Unity's sorting layer: `Background`, `Default`, `Entities`, `Bullets` or
    /// `Foreground`. Everything up to `Entities` is drawn under the characters.
    pub sorting_layer: String,
    pub order: i32,
    /// Switched-off layers are optional wall sets, swapped in to close a side.
    pub enabled: bool,
    /// A ledge blocks walkers but not bullets or flyers.
    #[serde(default)]
    pub ledge: bool,
    /// One row per line, top row first. -1 is empty; otherwise
    /// `palette index * 8 + transform`, where the transform's low two bits are
    /// quarter turns counter-clockwise and 4 means mirrored left-to-right first.
    pub tiles: Vec<Vec<i32>>,
    /// Which cells block movement: one string per row, top first, `#` for solid.
    #[serde(default)]
    pub solid: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DecorationFile {
    pub sprite: String,
    pub position: [f32; 2],
    pub sorting_layer: String,
    /// Counter-clockwise, in degrees.
    #[serde(default)]
    pub rotation: f32,
    #[serde(default = "one")]
    pub scale: [f32; 2],
    /// Mirror left-to-right, top-to-bottom.
    #[serde(default)]
    pub flip: [bool; 2],
    #[serde(default = "white")]
    pub color: [f32; 4],
}

/// A prefab placed in the room. What it is depends on its name: a key of
/// `guns.json` is a gun lying on the floor, and so on.
#[derive(Clone, Debug, Deserialize)]
pub struct ObjectFile {
    pub prefab: String,
    pub position: [f32; 2],
}

/// An object that carried one of the Unity game's scripts.
#[derive(Clone, Debug, Deserialize)]
pub struct MarkerFile {
    pub name: String,
    pub scripts: Vec<String>,
    pub position: [f32; 2],
    /// Its box collider, if it had one.
    #[serde(default, rename = "box")]
    pub bounds: Option<BoxFile>,
    /// Settings typed into the Unity inspector, as `Script.field` -> text.
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct BoxFile {
    pub center: [f32; 2],
    pub size: [f32; 2],
}

impl MarkerFile {
    pub fn has_script(&self, script: &str) -> bool {
        self.scripts.iter().any(|name| name == script)
    }

    /// A numeric setting, e.g. `number("Killable.maxHealth")`.
    pub fn number(&self, field: &str) -> Option<f32> {
        self.fields.get(field)?.parse().ok()
    }
}

/// `guns.json`: every gun, the bullets they fire and the effects those leave.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GunsFile {
    pub guns: HashMap<String, GunFile>,
    pub bullets: HashMap<String, BulletFile>,
    pub effects: HashMap<String, EffectFile>,
}

impl GunsFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("guns.json")?;
        serde_json::from_slice(&bytes).context("could not parse guns.json")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GunKind {
    /// One bullet per shot, straight along the aim.
    Single,
    /// Several bullets per shot, each off-aim by a random angle and speed.
    Spray,
    /// A continuous beam.
    Laser,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GunFile {
    /// The number save files know the gun by.
    pub id: i32,
    pub kind: GunKind,
    #[serde(default)]
    pub sprite: Option<String>,
    /// The box the player must touch to pick the gun up, in world units.
    #[serde(default = "quarter")]
    pub pickup_size: [f32; 2],
    /// Shots (or laser ticks) until the gun overheats. 0 with a 0 cooldown means
    /// it never does.
    pub heat_capacity: f32,
    /// Seconds an overheated gun is locked while it cools.
    pub heat_cooldown: f32,
    pub damage: f32,
    pub knockback: f32,
    #[serde(default)]
    pub shake: ShakeFile,
    /// Seconds between shots.
    #[serde(default)]
    pub fire_delay: f32,
    /// World units per second.
    #[serde(default)]
    pub bullet_speed: f32,
    #[serde(default = "one_bullet")]
    pub bullet_count: u32,
    /// Spray guns: each bullet leaves up to this many degrees either side of the aim.
    #[serde(default)]
    pub spray_angle: f32,
    /// Spray guns: each bullet's speed is multiplied by a random 1..=this.
    #[serde(default = "unit")]
    pub random_speed: f32,
    /// A key of `bullets`. Lasers have none.
    #[serde(default)]
    pub bullet: Option<String>,
    /// Lasers: seconds between the beam's hits.
    #[serde(default)]
    pub damage_delay: f32,
    /// Lasers: the sprites of the beam's two caps and of the stretch between them.
    #[serde(default)]
    pub laser_start: Option<String>,
    #[serde(default)]
    pub laser_middle: Option<String>,
    #[serde(default)]
    pub laser_end: Option<String>,
}

/// A camera shake: how long, how far and how fast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct ShakeFile {
    pub time: f32,
    pub amplitude: f32,
    pub frequency: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BulletFile {
    #[serde(default)]
    pub sprite: Option<String>,
    #[serde(default = "white")]
    pub color: [f32; 4],
    #[serde(default = "one")]
    pub scale: [f32; 2],
    /// Collision box, in world units.
    #[serde(default = "eighth")]
    pub size: [f32; 2],
    /// Seconds before it disappears by itself.
    pub time_to_live: f32,
    /// Carries on through what it hits instead of stopping at the first.
    #[serde(default)]
    pub piercing: bool,
    /// A key of `effects`: what is left where it hits.
    #[serde(default)]
    pub effect: Option<String>,
    /// Unity's `HomingBullet.homingPower`: the force that steers it. 0 flies straight.
    #[serde(default)]
    pub homing: f32,
    /// The bullet's `Rigidbody2D` linear drag.
    #[serde(default)]
    pub drag: f32,
    /// `Bullet_Freeze.freezeStrength`: how hard it freezes what it hits. 0 does not.
    #[serde(default)]
    pub freeze: f32,
}

/// An effect that hurts everything it touches when it appears (`ExplosionScript`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct ExplosionFile {
    pub damage: f32,
    /// World units, from the prefab's `CircleCollider2D`.
    pub radius: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EffectFile {
    #[serde(default)]
    pub sprite: Option<String>,
    #[serde(default = "white")]
    pub color: [f32; 4],
    #[serde(default = "one")]
    pub scale: [f32; 2],
    pub time_to_live: f32,
    #[serde(default)]
    pub explosion: Option<ExplosionFile>,
}

fn quarter() -> [f32; 2] {
    [0.25, 0.25]
}

fn eighth() -> [f32; 2] {
    [0.125, 0.125]
}

fn one_bullet() -> u32 {
    1
}

fn unit() -> f32 {
    1.0
}

fn one() -> [f32; 2] {
    [1.0, 1.0]
}

fn white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

impl RoomFile {
    pub fn load(assets: &AssetSource, name: &str) -> Result<Self> {
        let path = format!("rooms/{name}.json");
        let bytes = assets.read(&path)?;
        let room: RoomFile = serde_json::from_slice(&bytes).with_context(|| format!("could not parse {path}"))?;
        room.check().with_context(|| format!("{path} is not a usable room"))?;
        Ok(room)
    }

    /// Rejects files whose grids do not match their stated size, so that nothing
    /// later has to bounds-check a tile lookup.
    fn check(&self) -> Result<()> {
        let [columns, rows] = self.size;
        if columns == 0 || rows == 0 || self.cell <= 0.0 {
            bail!("size {columns}x{rows} with cell {} is empty", self.cell);
        }
        for layer in &self.layers {
            if layer.tiles.len() != rows as usize || layer.tiles.iter().any(|row| row.len() != columns as usize) {
                bail!("layer `{}` is not {columns}x{rows} tiles", layer.name);
            }
            for &tile in layer.tiles.iter().flatten() {
                if tile >= 0 && (tile / 8) as usize >= self.palette.len() {
                    bail!("layer `{}` uses sprite {} but the palette has {}", layer.name, tile / 8, self.palette.len());
                }
            }
            if let Some(solid) = &layer.solid
                && (solid.len() != rows as usize || solid.iter().any(|row| row.len() != columns as usize))
            {
                bail!("layer `{}` has a solid grid that is not {columns}x{rows}", layer.name);
            }
        }
        Ok(())
    }
}

/// Unity's sorting layers, in draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SortingLayer {
    Background,
    Default,
    Entities,
    Bullets,
    Foreground,
}

impl SortingLayer {
    /// Unknown names draw as `Default`, which is what Unity does with a missing layer.
    pub fn from_name(name: &str) -> Self {
        match name {
            "Background" => Self::Background,
            "Entities" => Self::Entities,
            "Bullets" => Self::Bullets,
            "Foreground" => Self::Foreground,
            _ => Self::Default,
        }
    }

    /// Whether things on this layer are drawn over the characters.
    pub fn is_above_entities(self) -> bool {
        self > Self::Entities
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room_json(tiles: &str) -> String {
        format!(
            r##"{{"name": "t", "size": [2, 2], "cell": 0.5, "palette": ["a"],
                "layers": [{{"name": "Walls", "sorting_layer": "Default", "order": 0, "enabled": true,
                             "tiles": {tiles}, "solid": ["#.", ".."]}}]}}"##
        )
    }

    #[test]
    fn a_room_parses_with_defaults_for_what_is_missing() {
        let room: RoomFile = serde_json::from_str(&room_json("[[0, -1], [5, -1]]")).unwrap();
        room.check().unwrap();
        assert_eq!(room.size, [2, 2]);
        assert!(room.decorations.is_empty());
        assert!(!room.layers[0].ledge);
    }

    #[test]
    fn mismatched_grids_and_bad_palette_indices_are_rejected() {
        let short: RoomFile = serde_json::from_str(&room_json("[[0, -1]]")).unwrap();
        assert!(short.check().is_err());
        // 8 is palette index 1; the palette has one sprite.
        let bad: RoomFile = serde_json::from_str(&room_json("[[8, -1], [-1, -1]]")).unwrap();
        assert!(bad.check().is_err());
    }

    #[test]
    fn sorting_layers_order_as_unity_draws_them() {
        assert!(SortingLayer::from_name("Background") < SortingLayer::from_name("Default"));
        assert!(!SortingLayer::from_name("Entities").is_above_entities());
        assert!(SortingLayer::from_name("Foreground").is_above_entities());
        assert_eq!(SortingLayer::from_name("HUD Items"), SortingLayer::Default);
    }
}
