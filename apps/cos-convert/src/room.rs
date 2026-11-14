//! One Unity room prefab -> one room file.
//!
//! A room prefab is a root object holding a `Grid` of tilemaps plus loose objects:
//! decoration sprites, lights, nested prefabs (enemies, chests, hazards), barriers
//! at the doors and the odd scripted object. Everything is written out in room
//! space: x right, y up, origin at the room's bottom-left corner, in world units.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Result, bail};
use serde_json::{Map, Value as Json, json};

use crate::json::{number, pair};
use crate::project::{Project, SpriteInfo, TileCollider};
use crate::yaml::{Document, Value};

/// A dungeon room is 32 by 18 cells: `RoomInfo.width/height` of 16 by 9 at a
/// half-unit cell. The dungeon generator places rooms on that grid, so a room with
/// a room script is exactly this size whatever was painted outside it.
pub const COLUMNS: i64 = 32;
pub const ROWS: i64 = 18;

/// The URP `Light2D` script. It lives in a package, so its GUID names no project file.
const LIGHT_2D: &str = "073797afb82c5a1438f328866b10b3f0";
/// The physics layer named "Ledges": blocks walkers, but not bullets or flyers.
const LEDGES_LAYER: i64 = 9;

/// Property overrides on a nested prefab that only restate where it is, or that
/// Unity recomputes by itself.
const PLACEMENT_PROPERTIES: [&str; 10] = [
    "m_LocalBounds",
    "m_LocalPosition",
    "m_LocalRotation",
    "m_LocalEulerAnglesHint",
    "m_LocalScale",
    "m_Name",
    "m_RootOrder",
    "m_Anchor",
    "m_SizeDelta",
    "m_Pivot",
];

pub struct ConvertedRoom {
    pub json: Json,
    /// Every sprite the room refers to, for the shared catalogue.
    pub sprites: Vec<SpriteInfo>,
    pub warnings: Vec<String>,
    pub summary: Summary,
}

#[derive(Default)]
pub struct Summary {
    pub layers: usize,
    pub tiles: usize,
    pub decorations: usize,
    pub objects: usize,
    pub lights: usize,
}

/// Position, rotation about z and scale: all a 2D game's transforms carry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub(crate) position: [f32; 2],
    /// Counter-clockwise, in radians.
    pub(crate) rotation: f32,
    pub(crate) scale: [f32; 2],
}

impl Placement {
    pub(crate) const IDENTITY: Self = Self {
        position: [0.0, 0.0],
        rotation: 0.0,
        scale: [1.0, 1.0],
    };

    pub(crate) fn from_transform(transform: &Value) -> Self {
        let position = transform.get("m_LocalPosition");
        let rotation = transform.get("m_LocalRotation");
        let scale = transform.get("m_LocalScale");
        Self {
            position: [position.get("x").f32(), position.get("y").f32()],
            rotation: z_angle(rotation.get("z").f32(), rotation.get("w").f32()),
            scale: if scale.has("x") {
                [scale.get("x").f32(), scale.get("y").f32()]
            } else {
                [1.0, 1.0]
            },
        }
    }

    /// Maps a point from this object's local space to its parent's.
    pub(crate) fn apply(&self, point: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rotation.sin_cos();
        let x = point[0] * self.scale[0];
        let y = point[1] * self.scale[1];
        [
            self.position[0] + x * cos - y * sin,
            self.position[1] + x * sin + y * cos,
        ]
    }

    /// `self` as the parent of `child`: the child's placement in `self`'s parent space.
    pub(crate) fn then(&self, child: &Placement) -> Placement {
        Placement {
            position: self.apply(child.position),
            rotation: self.rotation + child.rotation,
            scale: [self.scale[0] * child.scale[0], self.scale[1] * child.scale[1]],
        }
    }
}

/// The z rotation a quaternion with only z and w set describes.
fn z_angle(z: f32, w: f32) -> f32 {
    if z == 0.0 && w == 0.0 { 0.0 } else { 2.0 * z.atan2(w) }
}

/// The objects of one prefab file, indexed by file id.
pub(crate) struct Prefab<'a> {
    documents: &'a [Document],
    by_id: HashMap<i64, usize>,
}

impl<'a> Prefab<'a> {
    pub(crate) fn new(documents: &'a [Document]) -> Self {
        let by_id = documents
            .iter()
            .enumerate()
            .map(|(index, document)| (document.file_id, index))
            .collect();
        Self { documents, by_id }
    }

    pub(crate) fn get(&self, file_id: i64) -> Option<&'a Document> {
        self.by_id.get(&file_id).map(|&index| &self.documents[index])
    }

    /// The components attached to a GameObject.
    pub(crate) fn components(&self, object: &'a Document) -> impl Iterator<Item = &'a Document> + '_ {
        object
            .body
            .get("m_Component")
            .seq()
            .iter()
            .filter_map(|entry| self.get(entry.get("component").get("fileID").i64()))
    }

    pub(crate) fn component(&self, object: &'a Document, class: &str) -> Option<&'a Document> {
        self.components(object).find(|component| component.class == class)
    }

    pub(crate) fn transform_of(&self, object: &'a Document) -> Option<&'a Document> {
        self.components(object)
            .find(|component| component.class == "Transform" || component.class == "RectTransform")
    }

    pub(crate) fn object_of(&self, component: &Document) -> Option<&'a Document> {
        self.get(component.body.get("m_GameObject").get("fileID").i64())
    }

    pub(crate) fn parent_transform(&self, transform: &Document) -> Option<&'a Document> {
        self.get(transform.body.get("m_Father").get("fileID").i64())
    }

    /// A transform's placement in room space: its own, under all its ancestors'.
    pub(crate) fn world(&self, transform: &Document) -> Placement {
        let local = Placement::from_transform(&transform.body);
        match self.parent_transform(transform) {
            Some(parent) if !parent.stripped => self.world(parent).then(&local),
            _ => local,
        }
    }

    pub(crate) fn world_of_object(&self, object: &'a Document) -> Placement {
        self.transform_of(object)
            .map(|transform| self.world(transform))
            .unwrap_or(Placement::IDENTITY)
    }

    /// Whether an object and all its ancestors are switched on.
    pub(crate) fn is_active(&self, object: &'a Document) -> bool {
        if !object.body.get("m_IsActive").bool() {
            return false;
        }
        let parent = self
            .transform_of(object)
            .and_then(|transform| self.parent_transform(transform))
            .filter(|parent| !parent.stripped)
            .and_then(|parent| self.object_of(parent));
        match parent {
            Some(parent) => self.is_active(parent),
            None => true,
        }
    }

    /// The name of the object a transform belongs to.
    pub(crate) fn name_of_transform(&self, transform: &Document) -> String {
        self.object_of(transform)
            .map(|object| object.body.get("m_Name").str().to_string())
            .unwrap_or_default()
    }

    /// The root transform: the one with no parent.
    pub(crate) fn root_transform(&self) -> Option<&'a Document> {
        self.documents.iter().find(|document| {
            !document.stripped
                && (document.class == "Transform" || document.class == "RectTransform")
                && document.body.get("m_Father").get("fileID").i64() == 0
        })
    }
}

/// Sprites used by a room, in first-use order. Tiles refer to them by index.
#[derive(Default)]
struct Palette {
    sprites: Vec<SpriteInfo>,
    index: HashMap<String, usize>,
}

impl Palette {
    fn add(&mut self, sprite: SpriteInfo) -> usize {
        if let Some(&index) = self.index.get(&sprite.name) {
            return index;
        }
        let index = self.sprites.len();
        self.index.insert(sprite.name.clone(), index);
        self.sprites.push(sprite);
        index
    }
}

/// Converts one room prefab. `on_dungeon_grid` rooms are exactly one grid cell of
/// the dungeon (32 by 18 tiles); others are as large as what was painted.
pub fn convert(project: &Project, relative: &Path, name: &str, on_dungeon_grid: bool) -> Result<ConvertedRoom> {
    let documents = project.documents(relative)?;
    let prefab = Prefab::new(&documents);
    let mut warnings = Vec::new();
    let mut palette = Palette::default();
    let mut summary = Summary::default();

    if prefab.root_transform().is_none() {
        bail!("{} has no root object", relative.display());
    }

    let room_script = documents
        .iter()
        .find(|document| document.class == "MonoBehaviour" && document.body.has("doorTop"));
    let (columns, rows) = if on_dungeon_grid {
        (COLUMNS, ROWS)
    } else {
        painted_size(&documents)
    };

    let mut room = Map::new();
    room.insert("name".into(), json!(name));
    room.insert("source".into(), json!(relative.to_string_lossy().replace('\\', "/")));
    room.insert("size".into(), json!([columns, rows]));

    // The room script: which sides have doors, and what kind of room this is.
    if let Some(script) = room_script {
        let body = &script.body;
        room.insert(
            "doors".into(),
            json!({
                "top": body.get("doorTop").bool(),
                "bottom": body.get("doorBottom").bool(),
                "left": body.get("doorLeft").bool(),
                "right": body.get("doorRight").bool(),
            }),
        );
        let kind = match body.get("roomType").i64() {
            0 => "spawn",
            2 => "exit",
            3 => "boss",
            _ => "random",
        };
        room.insert("room_type".into(), json!(kind));
    }

    let mut cell = 0.5;
    let mut layers = Vec::new();
    let mut decorations = Vec::new();
    let mut lights = Vec::new();
    let mut objects = Vec::new();
    let mut barriers = Vec::new();
    let mut markers = Vec::new();

    for document in &documents {
        if document.stripped {
            continue;
        }
        match document.class.as_str() {
            "Tilemap" => {
                let Some(object) = prefab.object_of(document) else {
                    continue;
                };
                if let Some(grid) = prefab
                    .transform_of(object)
                    .and_then(|transform| prefab.parent_transform(transform))
                    .and_then(|parent| prefab.object_of(parent))
                    .and_then(|parent| prefab.component(parent, "Grid"))
                {
                    let size = grid.body.get("m_CellSize").get("x").f32();
                    if size > 0.0 {
                        cell = size;
                    }
                }
                let offset = prefab.world_of_object(object);
                if offset != Placement::IDENTITY {
                    warnings.push(format!(
                        "tilemap `{}` is moved, rotated or scaled; its tiles are written as if it were not",
                        object.body.get("m_Name").str()
                    ));
                }
                let (order, layer) =
                    tile_layer(project, &prefab, object, document, (columns, rows), &mut palette, &mut warnings)?;
                summary.tiles += layer.1;
                layers.push((order, layer.0));
            }
            "SpriteRenderer" => {
                let Some(object) = prefab.object_of(document) else {
                    continue;
                };
                if !document.body.get("m_Enabled").bool() || !prefab.is_active(object) {
                    continue;
                }
                let Some(sprite) = project.sprite(document.body.get("m_Sprite"))? else {
                    warnings.push(format!(
                        "sprite on `{}` could not be resolved and was left out",
                        object.body.get("m_Name").str()
                    ));
                    continue;
                };
                let placement = prefab.world_of_object(object);
                let (layer_position, layer_name) =
                    project.sorting_layer(document.body.get("m_SortingLayerID").i64());
                let order = document.body.get("m_SortingOrder").i64();
                let mut entry = Map::new();
                entry.insert("name".into(), json!(object.body.get("m_Name").str()));
                entry.insert("sprite".into(), json!(sprite.name));
                entry.insert("position".into(), pair(placement.position[0], placement.position[1]));
                entry.insert("sorting_layer".into(), json!(layer_name));
                entry.insert("order".into(), json!(order));
                insert_placement_extras(&mut entry, &placement);
                let flip = (document.body.get("m_FlipX").bool(), document.body.get("m_FlipY").bool());
                if flip != (false, false) {
                    entry.insert("flip".into(), json!([flip.0, flip.1]));
                }
                let color = color_of(document.body.get("m_Color"));
                if color != [1.0, 1.0, 1.0, 1.0] {
                    entry.insert("color".into(), Json::Array(color.iter().map(|&part| number(part)).collect()));
                }
                palette.add(sprite);
                decorations.push(((layer_position, order), Json::Object(entry)));
            }
            "MonoBehaviour" if script_guid(document) == LIGHT_2D => {
                let Some(object) = prefab.object_of(document) else {
                    continue;
                };
                if document.body.get("m_Enabled").bool() && prefab.is_active(object) {
                    lights.push(light(project, &prefab, object, document));
                }
            }
            "PrefabInstance" => match nested_prefab(project, &prefab, document, &mut warnings) {
                Nested::Object(object) => objects.push(object),
                Nested::Light(entry) => lights.push(entry),
            },
            "GameObject" => {
                if let Some(entry) = scripted_object(project, &prefab, document) {
                    match entry {
                        Scripted::Barrier(barrier) => barriers.push(barrier),
                        Scripted::Marker(marker) => markers.push(marker),
                    }
                }
            }
            _ => {}
        }
    }

    // Bottom layer first. Sorting is stable, so ties keep the prefab's own order.
    layers.sort_by_key(|(order, _)| *order);
    decorations.sort_by_key(|(order, _)| *order);
    summary.layers = layers.len();
    summary.decorations = decorations.len();
    summary.objects = objects.len();
    summary.lights = lights.len();

    room.insert("cell".into(), number(cell));
    room.insert(
        "palette".into(),
        Json::Array(palette.sprites.iter().map(|sprite| json!(sprite.name)).collect()),
    );
    room.insert("layers".into(), Json::Array(layers.into_iter().map(|(_, layer)| layer).collect()));
    room.insert(
        "decorations".into(),
        Json::Array(decorations.into_iter().map(|(_, entry)| entry).collect()),
    );
    room.insert("objects".into(), Json::Array(objects));
    room.insert("lights".into(), Json::Array(lights));
    room.insert("barriers".into(), Json::Array(barriers));
    room.insert("markers".into(), Json::Array(markers));

    Ok(ConvertedRoom {
        json: Json::Object(room),
        sprites: palette.sprites,
        warnings,
        summary,
    })
}

/// The size, in cells, that covers every painted tile: at least one dungeon room.
fn painted_size(documents: &[Document]) -> (i64, i64) {
    let mut size = (COLUMNS, ROWS);
    for document in documents.iter().filter(|document| document.class == "Tilemap") {
        for tile in document.body.get("m_Tiles").seq() {
            let position = tile.get("first");
            size.0 = size.0.max(position.get("x").i64() + 1);
            size.1 = size.1.max(position.get("y").i64() + 1);
        }
    }
    size
}

pub(crate) fn script_guid(document: &Document) -> &str {
    document.body.get("m_Script").get("guid").str()
}

pub(crate) fn color_of(value: &Value) -> [f32; 4] {
    [
        value.get("r").f32(),
        value.get("g").f32(),
        value.get("b").f32(),
        value.get("a").f32(),
    ]
}

/// Adds `rotation` (degrees) and `scale` to an entry, when they are not the defaults.
fn insert_placement_extras(entry: &mut Map<String, Json>, placement: &Placement) {
    if placement.rotation.abs() > 1e-4 {
        entry.insert("rotation".into(), number(placement.rotation.to_degrees()));
    }
    if (placement.scale[0] - 1.0).abs() > 1e-4 || (placement.scale[1] - 1.0).abs() > 1e-4 {
        entry.insert("scale".into(), pair(placement.scale[0], placement.scale[1]));
    }
}

/// How a tile is turned within its cell: quarter turns counter-clockwise in the low
/// two bits, plus 4 when it is mirrored left-to-right before turning.
fn tile_transform(matrix: &Value) -> i64 {
    let (mut e00, e01) = (matrix.get("e00").f32(), matrix.get("e01").f32());
    let (mut e10, e11) = (matrix.get("e10").f32(), matrix.get("e11").f32());
    let mirrored = e00 * e11 - e01 * e10 < 0.0;
    if mirrored {
        // M = R * mirror, so R = M * mirror: negate the first column.
        e00 = -e00;
        e10 = -e10;
    }
    let quarter_turns = (e10.atan2(e00) / std::f32::consts::FRAC_PI_2).round() as i64;
    quarter_turns.rem_euclid(4) + if mirrored { 4 } else { 0 }
}

/// One tilemap as a layer: `((sort key), (json, tile count))`.
#[allow(clippy::type_complexity)]
fn tile_layer(
    project: &Project,
    prefab: &Prefab,
    object: &Document,
    tilemap: &Document,
    (columns, rows): (i64, i64),
    palette: &mut Palette,
    warnings: &mut Vec<String>,
) -> Result<((usize, i64), (Json, usize))> {
    let name = object.body.get("m_Name").str();
    let body = &tilemap.body;

    let renderer = prefab.component(object, "TilemapRenderer");
    let (layer_position, layer_name) = project.sorting_layer(
        renderer
            .map(|renderer| renderer.body.get("m_SortingLayerID").i64())
            .unwrap_or(0),
    );
    let order = renderer
        .map(|renderer| renderer.body.get("m_SortingOrder").i64())
        .unwrap_or(0);
    let collides = prefab
        .component(object, "TilemapCollider2D")
        .is_some_and(|collider| collider.body.get("m_Enabled").bool() && !collider.body.get("m_IsTrigger").bool());

    let tile_assets = body.get("m_TileAssetArray").seq();
    let tile_sprites = body.get("m_TileSpriteArray").seq();
    let tile_matrices = body.get("m_TileMatrixArray").seq();

    // Resolve each sprite slot once; a layer reuses a handful of them many times.
    // `None` is a tile with no picture: Unity allows those, and they still collide.
    let mut sprite_slots: HashMap<i64, Option<usize>> = HashMap::new();
    let mut tiles = vec![vec![-1i64; columns as usize]; rows as usize];
    let mut solid = vec![vec![false; columns as usize]; rows as usize];
    let (mut count, mut outside, mut unresolved, mut any_solid) = (0, 0, 0, false);

    for tile in body.get("m_Tiles").seq() {
        let position = tile.get("first");
        let (x, y) = (position.get("x").i64(), position.get("y").i64());
        if !(0..columns).contains(&x) || !(0..rows).contains(&y) {
            outside += 1;
            continue;
        }
        let data = tile.get("second");
        // Rows are written top first, as the room looks on screen.
        let row = (rows - 1 - y) as usize;

        if collides {
            let asset = tile_assets
                .get(data.get("m_TileIndex").i64() as usize)
                .map(|entry| entry.get("m_Data").get("guid").str())
                .unwrap_or("");
            if project.tile_collider(asset) == TileCollider::Solid {
                solid[row][x as usize] = true;
                any_solid = true;
            }
        }

        let sprite_index = data.get("m_TileSpriteIndex").i64();
        let slot = match sprite_slots.get(&sprite_index) {
            Some(slot) => *slot,
            None => {
                let reference = tile_sprites
                    .get(sprite_index as usize)
                    .map(|entry| entry.get("m_Data"));
                let slot = match reference {
                    Some(reference) => {
                        let sprite = project.sprite(reference)?;
                        if sprite.is_none() && reference.get("fileID").i64() != 0 {
                            unresolved += 1;
                        }
                        sprite.map(|sprite| palette.add(sprite))
                    }
                    None => None,
                };
                sprite_slots.insert(sprite_index, slot);
                slot
            }
        };
        let Some(slot) = slot else {
            continue;
        };

        let transform = tile_matrices
            .get(data.get("m_TileMatrixIndex").i64() as usize)
            .map(|entry| tile_transform(entry.get("m_Data")))
            .unwrap_or(0);
        tiles[row][x as usize] = slot as i64 * 8 + transform;
        count += 1;
    }

    if outside > 0 {
        warnings.push(format!("layer `{name}`: {outside} tiles lie outside the room and were left out"));
    }
    if unresolved > 0 {
        warnings.push(format!("layer `{name}`: {unresolved} sprites could not be resolved; their tiles are not drawn"));
    }

    let mut layer = Map::new();
    layer.insert("name".into(), json!(name));
    layer.insert("sorting_layer".into(), json!(layer_name));
    layer.insert("order".into(), json!(order));
    // Switched-off layers are the optional wall sets a room swaps in to close a side.
    layer.insert("enabled".into(), json!(prefab.is_active(object)));
    if object.body.get("m_Layer").i64() == LEDGES_LAYER {
        layer.insert("ledge".into(), json!(true));
    }
    layer.insert(
        "tiles".into(),
        Json::Array(tiles.into_iter().map(|row| json!(row)).collect()),
    );
    if any_solid {
        let rows: Vec<String> = solid
            .iter()
            .map(|row| row.iter().map(|&cell| if cell { '#' } else { '.' }).collect())
            .collect();
        layer.insert("solid".into(), json!(rows));
    }

    Ok(((layer_position, order), (Json::Object(layer), count)))
}

/// The lights a prefab carries, placed from its root: what a thing spawned while
/// playing (a bullet, a chest, an enemy) brings with it. Switched-off lights and
/// lights on switched-off objects are left out.
pub(crate) fn prefab_lights(project: &Project, prefab: &Prefab, root: &Document, documents: &[Document]) -> Vec<Json> {
    let origin = prefab.world_of_object(root).position;
    let mut lights = Vec::new();
    for document in documents {
        if document.class != "MonoBehaviour" || script_guid(document) != LIGHT_2D || !document.body.get("m_Enabled").bool() {
            continue;
        }
        let Some(object) = prefab.object_of(document) else {
            continue;
        };
        if !prefab.is_active(object) {
            continue;
        }
        let mut entry = light(project, prefab, object, document);
        // From the root, not from wherever the prefab's root happened to stand.
        let shift = |point: &mut Json| {
            if let Some([x, y]) = point.as_array().and_then(|pair| {
                Some([pair.first()?.as_f64()? as f32, pair.get(1)?.as_f64()? as f32])
            }) {
                *point = crate::json::pair(x - origin[0], y - origin[1]);
            }
        };
        if let Some(position) = entry.get_mut("position") {
            shift(position);
        }
        if let Some(path) = entry.get_mut("path").and_then(Json::as_array_mut) {
            path.iter_mut().for_each(shift);
        }
        if let Some(fields) = entry.as_object_mut() {
            fields.remove("group");
        }
        lights.push(entry);
    }
    lights
}

fn light(project: &Project, prefab: &Prefab, object: &Document, light: &Document) -> Json {
    let group = prefab
        .transform_of(object)
        .and_then(|transform| prefab.parent_transform(transform))
        .map(|parent| prefab.name_of_transform(parent));
    light_entry(
        project,
        object.body.get("m_Name").str(),
        group,
        &light.body,
        &prefab.world_of_object(object),
    )
}

/// One light, from its `Light2D` fields and where it sits in the room.
fn light_entry(project: &Project, name: &str, group: Option<String>, body: &Value, placement: &Placement) -> Json {
    let kind = match body.get("m_LightType").i64() {
        0 => "parametric",
        1 => "freeform",
        2 => "sprite",
        3 => "point",
        _ => "global",
    };

    let mut entry = Map::new();
    entry.insert("name".into(), json!(name));
    entry.insert("kind".into(), json!(kind));
    if let Some(group) = group {
        entry.insert("group".into(), json!(group));
    }
    let color = color_of(body.get("m_Color"));
    entry.insert("color".into(), Json::Array(color[..3].iter().map(|&part| number(part)).collect()));
    entry.insert("intensity".into(), number(body.get("m_Intensity").f32()));
    entry.insert(
        "layers".into(),
        Json::Array(
            sorting_layer_ids(body.get("m_ApplyToSortingLayers"))
                .into_iter()
                .map(|id| json!(project.sorting_layer(id).1))
                .collect(),
        ),
    );
    if body.get("m_ShadowIntensityEnabled").bool() {
        entry.insert("shadow".into(), number(body.get("m_ShadowIntensity").f32()));
    }

    match kind {
        "global" => {}
        "point" => {
            let scale = placement.scale[0].abs().max(placement.scale[1].abs());
            entry.insert("position".into(), pair(placement.position[0], placement.position[1]));
            entry.insert("inner_radius".into(), number(body.get("m_PointLightInnerRadius").f32() * scale));
            entry.insert("outer_radius".into(), number(body.get("m_PointLightOuterRadius").f32() * scale));
            entry.insert("falloff".into(), number(body.get("m_FalloffIntensity").f32()));
            let (inner, outer) = (
                body.get("m_PointLightInnerAngle").f32(),
                body.get("m_PointLightOuterAngle").f32(),
            );
            if inner < 360.0 || outer < 360.0 {
                entry.insert("inner_angle".into(), number(inner));
                entry.insert("outer_angle".into(), number(outer));
                entry.insert("rotation".into(), number(placement.rotation.to_degrees()));
            }
        }
        _ => {
            entry.insert("position".into(), pair(placement.position[0], placement.position[1]));
            // The outline, already in room space.
            let path: Vec<Json> = body
                .get("m_ShapePath")
                .seq()
                .iter()
                .map(|point| {
                    let [x, y] = placement.apply([point.get("x").f32(), point.get("y").f32()]);
                    pair(x, y)
                })
                .collect();
            entry.insert("path".into(), Json::Array(path));
            entry.insert("falloff".into(), number(body.get("m_FalloffIntensity").f32()));
            entry.insert("falloff_size".into(), number(body.get("m_ShapeLightFalloffSize").f32()));
        }
    }
    Json::Object(entry)
}

/// `m_ApplyToSortingLayers` is a list of ids, which Unity writes either as a
/// sequence or as one run of little-endian hex.
fn sorting_layer_ids(value: &Value) -> Vec<i64> {
    match value {
        Value::Seq(items) => items.iter().map(Value::i64).collect(),
        other => {
            let hex = other.str();
            (0..hex.len() / 8)
                .filter_map(|index| u32::from_str_radix(&hex[index * 8..index * 8 + 8], 16).ok())
                .map(|big_endian_read| big_endian_read.swap_bytes() as i64)
                .collect()
        }
    }
}

enum Nested {
    /// An enemy, a chest, a hazard: something the game will spawn.
    Object(Json),
    /// A prefab that is nothing but a light, such as the hub's spotlights.
    Light(Json),
}

/// A prefab placed inside the room.
fn nested_prefab(project: &Project, prefab: &Prefab, instance: &Document, warnings: &mut Vec<String>) -> Nested {
    let source_guid = instance.body.get("m_SourcePrefab").get("guid").str();
    let modification = instance.body.get("m_Modification");
    let source = project.prefab(source_guid);
    let source_prefab = source.as_deref().map(|documents| Prefab::new(documents));
    let source_root = source_prefab.as_ref().and_then(Prefab::root_transform);

    let prefab_name = project.stem_of(source_guid).unwrap_or_else(|| {
        warnings.push(format!("a nested prefab's source ({source_guid}) is not in the project"));
        source_guid.to_string()
    });

    // Start from where the prefab's own root sits, then apply the room's overrides.
    let mut local = source_root
        .map(|root| Placement::from_transform(&root.body))
        .unwrap_or(Placement::IDENTITY);
    let mut rotation = source_root
        .map(|root| {
            let rotation = root.body.get("m_LocalRotation");
            (rotation.get("z").f32(), rotation.get("w").f32())
        })
        .unwrap_or((0.0, 1.0));
    let mut name = source_root
        .and_then(|root| source_prefab.as_ref().map(|prefab| prefab.name_of_transform(root)))
        .unwrap_or_default();
    let mut overrides = BTreeMap::new();

    // A light on the prefab's root object, with the room's overrides applied to it.
    let root_light = source_prefab.as_ref().and_then(|source| {
        let root_object = source.object_of(source_root?)?;
        source
            .components(root_object)
            .find(|component| component.class == "MonoBehaviour" && script_guid(component) == LIGHT_2D)
    });
    let mut light_body = root_light.map(|light| light.body.clone());

    for change in modification.get("m_Modifications").seq() {
        let path = change.get("propertyPath").str();
        let value = change.get("value");
        let target = change.get("target").get("fileID").i64();
        let on_root = source_root.is_none_or(|root| root.file_id == target);

        match path {
            "m_LocalPosition.x" if on_root => local.position[0] = value.f32(),
            "m_LocalPosition.y" if on_root => local.position[1] = value.f32(),
            "m_LocalRotation.z" if on_root => rotation.0 = value.f32(),
            "m_LocalRotation.w" if on_root => rotation.1 = value.f32(),
            "m_LocalScale.x" if on_root => local.scale[0] = value.f32(),
            "m_LocalScale.y" if on_root => local.scale[1] = value.f32(),
            "m_Name" => name = value.str().to_string(),
            _ if PLACEMENT_PROPERTIES.iter().any(|prefix| path.starts_with(prefix)) => {}
            _ if root_light.is_some_and(|light| light.file_id == target) => {
                if let Some(body) = light_body.as_mut() {
                    body.set_path(path, value.str());
                }
            }
            _ => {
                // Say what the override is on, e.g. `EffectGrate.lava`.
                let owner = source_prefab
                    .as_ref()
                    .and_then(|prefab| prefab.get(target))
                    .map(|document| match document.class.as_str() {
                        "MonoBehaviour" if script_guid(document) == LIGHT_2D => "Light2D".to_string(),
                        "MonoBehaviour" => project
                            .stem_of(script_guid(document))
                            .unwrap_or_else(|| "MonoBehaviour".to_string()),
                        class => class.to_string(),
                    })
                    .unwrap_or_else(|| "?".to_string());
                let reference = change.get("objectReference").get("guid").str();
                let text = if value.str().is_empty() && !reference.is_empty() {
                    format!("asset:{}", project.stem_of(reference).unwrap_or_else(|| reference.to_string()))
                } else {
                    value.str().to_string()
                };
                overrides.insert(format!("{owner}.{path}"), text);
            }
        }
    }
    local.rotation = z_angle(rotation.0, rotation.1);

    let parent = prefab.get(modification.get("m_TransformParent").get("fileID").i64());
    let placement = match parent {
        Some(parent) => prefab.world(parent).then(&local),
        None => local,
    };

    let group = parent.map(|parent| prefab.name_of_transform(parent));
    if let Some(body) = &light_body {
        return Nested::Light(light_entry(project, &name, group, body, &placement));
    }

    let mut entry = Map::new();
    entry.insert("prefab".into(), json!(prefab_name));
    entry.insert("name".into(), json!(name));
    if let Some(group) = group {
        entry.insert("group".into(), json!(group));
    }
    entry.insert("position".into(), pair(placement.position[0], placement.position[1]));
    insert_placement_extras(&mut entry, &placement);
    if !overrides.is_empty() {
        entry.insert("overrides".into(), json!(overrides));
    }
    Nested::Object(Json::Object(entry))
}

enum Scripted {
    /// A box at a door that stops everything except the player.
    Barrier(Json),
    /// Any other object carrying one of the game's own scripts.
    Marker(Json),
}

/// An object in the room that carries game scripts: a barrier, a trigger, a bench.
fn scripted_object(project: &Project, prefab: &Prefab, object: &Document) -> Option<Scripted> {
    if !prefab.is_active(object) {
        return None;
    }

    let mut scripts = Vec::new();
    let mut fields = BTreeMap::new();
    let mut lists = BTreeMap::new();
    for component in prefab.components(object) {
        if component.class != "MonoBehaviour" || !component.body.get("m_Enabled").bool() {
            continue;
        }
        // Only the game's own scripts: package scripts have no file in the project.
        let Some(path) = project.path_of(script_guid(component)) else {
            continue;
        };
        if !path.starts_with("Scripts") || component.body.has("doorTop") {
            continue;
        }
        let script = path.file_stem()?.to_string_lossy().into_owned();
        if script == "ShadowCaster" {
            continue;
        }
        // Plain settings typed into the inspector; references to other objects are
        // left out, since their ids mean nothing outside Unity.
        for (key, value) in component.body.entries() {
            if key.starts_with("m_") {
                continue;
            }
            match value {
                Value::Scalar(text) if !text.is_empty() => {
                    fields.insert(format!("{script}.{key}"), text.clone());
                }
                // Lists of plain text, such as `DialogueEvent.dialogue`'s lines.
                Value::Seq(items) if !items.is_empty() && items.iter().all(|item| matches!(item, Value::Scalar(_))) => {
                    let texts: Vec<&str> = items.iter().map(Value::str).collect();
                    lists.insert(format!("{script}.{key}"), texts);
                }
                _ => {}
            }
        }
        scripts.push(script);
    }
    if scripts.is_empty() {
        return None;
    }

    let placement = prefab.world_of_object(object);
    let collider = prefab.component(object, "BoxCollider2D").map(|collider| {
        let (size, offset) = (collider.body.get("m_Size"), collider.body.get("m_Offset"));
        let center = placement.apply([offset.get("x").f32(), offset.get("y").f32()]);
        let width = size.get("x").f32() * placement.scale[0].abs();
        let height = size.get("y").f32() * placement.scale[1].abs();
        (center, [width, height], collider.body.get("m_IsTrigger").bool())
    });

    let mut entry = Map::new();
    entry.insert("name".into(), json!(object.body.get("m_Name").str()));

    if scripts.iter().any(|script| script == "PlayerBarrierPassthrough") {
        let (center, size, _) = collider?;
        entry.insert("center".into(), pair(center[0], center[1]));
        entry.insert("size".into(), pair(size[0], size[1]));
        return Some(Scripted::Barrier(Json::Object(entry)));
    }

    entry.insert("scripts".into(), json!(scripts));
    entry.insert("position".into(), pair(placement.position[0], placement.position[1]));
    if let Some((center, size, trigger)) = collider {
        entry.insert(
            "box".into(),
            json!({ "center": pair(center[0], center[1]), "size": pair(size[0], size[1]), "trigger": trigger }),
        );
    }
    if !fields.is_empty() {
        entry.insert("fields".into(), json!(fields));
    }
    if !lists.is_empty() {
        entry.insert("lists".into(), json!(lists));
    }
    Some(Scripted::Marker(Json::Object(entry)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::parse_map;

    fn matrix(e00: f32, e01: f32, e10: f32, e11: f32) -> Value {
        parse_map(&format!("e00: {e00}\ne01: {e01}\ne10: {e10}\ne11: {e11}\n")).unwrap()
    }

    #[test]
    fn tile_matrices_become_quarter_turns_and_a_mirror_bit() {
        assert_eq!(tile_transform(&matrix(1.0, 0.0, 0.0, 1.0)), 0);
        // Unity's 90 degree counter-clockwise rotation matrix.
        assert_eq!(tile_transform(&matrix(0.0, -1.0, 1.0, 0.0)), 1);
        assert_eq!(tile_transform(&matrix(-1.0, 0.0, 0.0, -1.0)), 2);
        assert_eq!(tile_transform(&matrix(0.0, 1.0, -1.0, 0.0)), 3);
        // Mirrored left-to-right, not turned.
        assert_eq!(tile_transform(&matrix(-1.0, 0.0, 0.0, 1.0)), 4);
        // Mirrored top-to-bottom is a left-to-right mirror turned half way round.
        assert_eq!(tile_transform(&matrix(1.0, 0.0, 0.0, -1.0)), 6);
    }

    #[test]
    fn sorting_layer_ids_read_from_hex_or_a_list() {
        // 630060465 is the project's "Background" layer.
        assert_eq!(sorting_layer_ids(&Value::Scalar("b1f58d25".into())), [630060465]);
        assert_eq!(
            sorting_layer_ids(&Value::Scalar("b1f58d2500000000".into())),
            [630060465, 0]
        );
        let list = Value::Seq(vec![Value::Scalar("7".into()), Value::Scalar("-3".into())]);
        assert_eq!(sorting_layer_ids(&list), [7, -3]);
    }

    #[test]
    fn placements_compose_parent_then_child() {
        let parent = Placement {
            position: [10.0, 0.0],
            rotation: std::f32::consts::FRAC_PI_2,
            scale: [2.0, 2.0],
        };
        let child = Placement {
            position: [1.0, 0.0],
            ..Placement::IDENTITY
        };
        let world = parent.then(&child);
        // One unit along the parent's x, doubled, turned to point up.
        assert!((world.position[0] - 10.0).abs() < 1e-5 && (world.position[1] - 2.0).abs() < 1e-5);
        assert_eq!(world.scale, [2.0, 2.0]);
        assert!((z_angle(std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2) - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }
}
