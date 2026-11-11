//! The Unity project as a whole: where assets live and what sprites they hold.
//!
//! Unity refers to assets by GUID (stored in each asset's `.meta` file) and to an
//! object inside an asset by file id. This module turns those references back into
//! paths, sprite rectangles and tile settings.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, bail};

use crate::yaml::{self, Document, Value};

/// The file id Unity gives the one sprite of a texture imported in "Single" mode.
const SINGLE_SPRITE_FILE_ID: i64 = 21300000;

/// One sprite: a rectangle of a texture, and how it sits in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteInfo {
    /// Unique across the project's used sprites; the key in `sprites.json`.
    pub name: String,
    /// The PNG, relative to the Unity `Assets` folder.
    pub texture: PathBuf,
    /// `[x, y, width, height]` in pixels, origin at the texture's top-left.
    pub rect: [u32; 4],
    /// The point of the sprite placed at its position: (0, 0) bottom-left, (1, 1) top-right.
    pub pivot: [f32; 2],
    pub pixels_per_unit: f32,
}

/// What a Tile asset says about collision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileCollider {
    None,
    /// Unity's "Sprite" (the sprite's physics outline) or "Grid" (the whole cell).
    /// Both are treated as the whole cell.
    Solid,
}

pub struct Project {
    /// The Unity project's `Assets` folder.
    assets: PathBuf,
    guid_to_path: HashMap<String, PathBuf>,
    textures: RefCell<HashMap<String, Rc<TextureSprites>>>,
    tiles: RefCell<HashMap<String, TileCollider>>,
    prefabs: RefCell<HashMap<String, Rc<Vec<Document>>>>,
    /// Sorting layer id -> (draw position, name), from the project's tag settings.
    sorting_layers: HashMap<u32, (usize, String)>,
}

struct TextureSprites {
    by_file_id: HashMap<i64, SpriteInfo>,
}

impl Project {
    /// Indexes every `.meta` file under `<root>/Assets`.
    pub fn open(root: &Path) -> Result<Self> {
        let assets = root.join("Assets");
        if !assets.is_dir() {
            bail!("{} is not a Unity project: it has no Assets folder", root.display());
        }

        let mut guid_to_path = HashMap::new();
        let mut pending = vec![assets.clone()];
        while let Some(folder) = pending.pop() {
            let entries =
                std::fs::read_dir(&folder).with_context(|| format!("could not list {}", folder.display()))?;
            for entry in entries {
                let path = entry?.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "meta") {
                    // The guid is on the second line; no need to parse the whole file.
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    if let Some(guid) = text.lines().find_map(|line| line.strip_prefix("guid: ")) {
                        let asset = path.with_extension("");
                        if let Ok(relative) = asset.strip_prefix(&assets) {
                            guid_to_path.insert(guid.trim().to_string(), relative.to_path_buf());
                        }
                    }
                }
            }
        }

        let mut sorting_layers = HashMap::new();
        let tags_path = root.join("ProjectSettings").join("TagManager.asset");
        let tags_text = std::fs::read_to_string(&tags_path)
            .with_context(|| format!("could not read {}", tags_path.display()))?;
        let tags = yaml::parse_documents(&tags_text)
            .with_context(|| format!("could not parse {}", tags_path.display()))?;
        for document in &tags {
            for (position, layer) in document.body.get("m_SortingLayers").seq().iter().enumerate() {
                sorting_layers.insert(
                    layer.get("uniqueID").i64() as u32,
                    (position, layer.get("name").str().to_string()),
                );
            }
        }

        Ok(Self {
            assets,
            guid_to_path,
            sorting_layers,
            textures: RefCell::default(),
            tiles: RefCell::default(),
            prefabs: RefCell::default(),
        })
    }

    /// A sorting layer's name and its position in the draw order (0 is drawn first).
    /// Unknown ids fall back to Unity's built-in "Default".
    pub fn sorting_layer(&self, id: i64) -> (usize, &str) {
        match self.sorting_layers.get(&(id as u32)) {
            Some((position, name)) => (*position, name.as_str()),
            None => self
                .sorting_layers
                .values()
                .find(|(_, name)| name == "Default")
                .map(|(position, name)| (*position, name.as_str()))
                .unwrap_or((0, "Default")),
        }
    }

    pub fn assets_dir(&self) -> &Path {
        &self.assets
    }

    /// The asset a GUID names, relative to `Assets`. `None` for Unity's built-in
    /// and package assets, which have no file in the project.
    pub fn path_of(&self, guid: &str) -> Option<&Path> {
        self.guid_to_path.get(guid).map(PathBuf::as_path)
    }

    /// The file name without extension of the asset a GUID names.
    pub fn stem_of(&self, guid: &str) -> Option<String> {
        self.path_of(guid)?
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
    }

    /// Reads and parses a scene or prefab, relative to `Assets`.
    pub fn documents(&self, relative: &Path) -> Result<Vec<Document>> {
        let path = self.assets.join(relative);
        let text = std::fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
        yaml::parse_documents(&text).with_context(|| format!("could not parse {}", path.display()))
    }

    /// The parsed prefab a GUID names, cached: rooms place the same few prefabs
    /// hundreds of times.
    pub fn prefab(&self, guid: &str) -> Option<Rc<Vec<Document>>> {
        if let Some(found) = self.prefabs.borrow().get(guid) {
            return Some(found.clone());
        }
        let path = self.path_of(guid)?.to_path_buf();
        let documents = Rc::new(self.documents(&path).ok()?);
        self.prefabs.borrow_mut().insert(guid.to_string(), documents.clone());
        Some(documents)
    }

    /// Resolves a `{fileID, guid}` sprite reference.
    pub fn sprite(&self, reference: &Value) -> Result<Option<SpriteInfo>> {
        let guid = reference.get("guid").str();
        let file_id = reference.get("fileID").i64();
        if guid.is_empty() || file_id == 0 {
            return Ok(None);
        }

        if !self.textures.borrow().contains_key(guid) {
            let Some(texture) = self.path_of(guid) else {
                return Ok(None);
            };
            let sprites = self.read_texture_sprites(texture)?;
            self.textures.borrow_mut().insert(guid.to_string(), Rc::new(sprites));
        }
        Ok(self.textures.borrow()[guid].by_file_id.get(&file_id).cloned())
    }

    fn read_texture_sprites(&self, texture: &Path) -> Result<TextureSprites> {
        let full = self.assets.join(texture);
        let (texture_width, texture_height) = png_size(&full)?;

        let meta_path = PathBuf::from(format!("{}.meta", full.display()));
        let meta_text = std::fs::read_to_string(&meta_path)
            .with_context(|| format!("could not read {}", meta_path.display()))?;
        let meta = yaml::parse_map(&meta_text).with_context(|| format!("could not parse {}", meta_path.display()))?;
        let importer = meta.get("TextureImporter");
        let pixels_per_unit = match importer.get("spritePixelsToUnits").f32() {
            value if value > 0.0 => value,
            _ => 100.0,
        };
        let stem = texture
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut by_file_id = HashMap::new();
        // 1 is Single, 2 is Multiple (a sliced sheet).
        if importer.get("spriteMode").i64() == 2 {
            // Older metas keep the name -> id table apart from the sprite list.
            let table = importer.get("spriteSheet").get("nameFileIdTable");
            for sprite in importer.get("spriteSheet").get("sprites").seq() {
                let name = sprite.get("name").str();
                let file_id = match sprite.get("internalID").i64() {
                    0 => table.get(name).i64(),
                    id => id,
                };
                let rect = sprite.get("rect");
                let (x, y) = (rect.get("x").f32(), rect.get("y").f32());
                let (width, height) = (rect.get("width").f32(), rect.get("height").f32());
                // Unity measures a sprite's rectangle from the texture's bottom-left.
                let top = texture_height as f32 - y - height;
                by_file_id.insert(
                    file_id,
                    SpriteInfo {
                        name: name.to_string(),
                        texture: texture.to_path_buf(),
                        rect: [x.round() as u32, top.round().max(0.0) as u32, width.round() as u32, height.round() as u32],
                        pivot: pivot_of(sprite.get("alignment").i64(), sprite.get("pivot")),
                        pixels_per_unit,
                    },
                );
            }
        } else {
            by_file_id.insert(
                SINGLE_SPRITE_FILE_ID,
                SpriteInfo {
                    name: stem,
                    texture: texture.to_path_buf(),
                    rect: [0, 0, texture_width, texture_height],
                    pivot: pivot_of(importer.get("alignment").i64(), importer.get("spritePivot")),
                    pixels_per_unit,
                },
            );
        }

        Ok(TextureSprites { by_file_id })
    }

    /// Whether the Tile asset a GUID names blocks movement.
    pub fn tile_collider(&self, guid: &str) -> TileCollider {
        if let Some(found) = self.tiles.borrow().get(guid) {
            return *found;
        }
        let collider = self
            .path_of(guid)
            .and_then(|path| self.documents(path).ok())
            .and_then(|documents| {
                documents
                    .iter()
                    .find(|document| document.body.has("m_ColliderType"))
                    .map(|document| document.body.get("m_ColliderType").i64())
            })
            .map(|kind| if kind == 0 { TileCollider::None } else { TileCollider::Solid })
            .unwrap_or(TileCollider::None);
        self.tiles.borrow_mut().insert(guid.to_string(), collider);
        collider
    }
}

/// Unity's sprite alignment enum to a pivot. 9 is "Custom" and uses `custom`.
fn pivot_of(alignment: i64, custom: &Value) -> [f32; 2] {
    match alignment {
        1 => [0.0, 1.0],
        2 => [0.5, 1.0],
        3 => [1.0, 1.0],
        4 => [0.0, 0.5],
        5 => [1.0, 0.5],
        6 => [0.0, 0.0],
        7 => [0.5, 0.0],
        8 => [1.0, 0.0],
        9 => [custom.get("x").f32(), custom.get("y").f32()],
        _ => [0.5, 0.5],
    }
}

/// Reads a PNG's width and height from its header.
fn png_size(path: &Path) -> Result<(u32, u32)> {
    use std::io::Read;

    let mut header = [0u8; 24];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .with_context(|| format!("could not read {}", path.display()))?;
    if &header[..8] != b"\x89PNG\r\n\x1a\n" {
        bail!("{} is not a PNG", path.display());
    }
    let number = |at: usize| u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
    Ok((number(16), number(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_names_the_pivot() {
        let custom = yaml::parse_map("x: 0.25\ny: 0.75\n").unwrap();
        assert_eq!(pivot_of(0, &custom), [0.5, 0.5]);
        assert_eq!(pivot_of(6, &custom), [0.0, 0.0]);
        assert_eq!(pivot_of(7, &custom), [0.5, 0.0]);
        assert_eq!(pivot_of(9, &custom), [0.25, 0.75]);
    }
}
