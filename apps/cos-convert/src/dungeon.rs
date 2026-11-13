//! The dungeon generator's settings -> `dungeon.json`.
//!
//! Unity's `DungeonGenerator` lives in `Scenes/Dungeon.unity`. Its inspector
//! lists, for every set of doors a room can have, the room prefabs that fit,
//! in the order it tries them; each room prefab's `RoomScript` says how far from
//! the spawn it may be placed at the least (`distanceFromSpawn`). The grid size
//! and room count are constants in the script itself.
//!
//! It also writes the tiles `RoomScript.SealRoom` puts in a room's doorways while
//! the player fights there, and whether each blocks: every room uses the same
//! three, so they are read from the first room that names them.
//!
//! And the minimap's pictures, from the `MapMenu` on `Prefabs/Player View.prefab`:
//! a room not yet entered, the spawn, an entered room, the exit, the boss room,
//! and the frame over the room the player is in.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value as Json, json};

use crate::guns::root_script;
use crate::project::{Project, SpriteInfo, TileCollider};
use crate::room::Prefab;
use crate::slug;

const SCENE: &str = "Scenes/Dungeon.unity";
const PLAYER_VIEW: &str = "Prefabs/Player View.prefab";
const ROOMS_FOLDER: &str = "Prefabs/Generatable Rooms";
/// `DungeonGenerator.dungeonSize` and `dungeonRooms`: constants, not fields.
const GRID_SIZE: u32 = 10;
const ROOM_COUNT: u32 = 8;
/// The door sets the generator lists, as its fields name them.
const DOOR_SETS: [&str; 15] =
    ["NSEW", "NSE", "NEW", "NSW", "SEW", "NE", "NS", "NW", "SE", "EW", "SW", "N", "E", "S", "W"];

pub fn convert(project: &Project) -> Result<(Json, Vec<SpriteInfo>)> {
    let documents = project.documents(Path::new(SCENE))?;
    let generator = documents
        .iter()
        .find(|document| {
            document.class == "MonoBehaviour"
                && project.stem_of(crate::room::script_guid(document)).as_deref() == Some("DungeonGenerator")
        })
        .with_context(|| format!("no DungeonGenerator in {SCENE}"))?;
    let body = &generator.body;
    let room = |field: &str| project.stem_of(body.get(field).get("guid").str()).map(|name| slug(&name));

    let mut by_doors = BTreeMap::new();
    for doors in DOOR_SETS {
        let names: Vec<String> = body
            .get(&format!("roomDoors{doors}"))
            .seq()
            .iter()
            .filter_map(|entry| project.stem_of(entry.get("guid").str()))
            .map(|name| slug(&name))
            .collect();
        by_doors.insert(doors.to_string(), names);
    }

    let mut distance = BTreeMap::new();
    let mut door_tiles = None;
    let mut sprites = Vec::new();
    for path in project.prefabs_under(ROOMS_FOLDER)? {
        let documents = project.documents(&path)?;
        let prefab = Prefab::new(&documents);
        if let Some((_, _, script)) = root_script(project, &prefab, &["RoomScript", "SpawnRoom"]) {
            let name = path.file_stem().map(|stem| slug(&stem.to_string_lossy())).unwrap_or_default();
            distance.insert(name, script.get("distanceFromSpawn").i64());
            if door_tiles.is_none() {
                let mut tiles = serde_json::Map::new();
                for (key, field) in [("top", "doorHorzTop"), ("bottom", "doorHorzBot"), ("vertical", "doorVert")] {
                    if let Some((sprite, solid)) = tile(project, script.get(field).get("guid").str())? {
                        tiles.insert(key.into(), json!({ "sprite": sprite.name, "solid": solid }));
                        sprites.push(sprite);
                    }
                }
                if tiles.len() == 3 {
                    door_tiles = Some(Json::Object(tiles));
                }
            }
        }
    }

    let mut map = serde_json::Map::new();
    let view = project.documents(Path::new(PLAYER_VIEW))?;
    if let Some(menu) = view.iter().find(|document| {
        document.class == "MonoBehaviour" && project.stem_of(crate::room::script_guid(document)).as_deref() == Some("MapMenu")
    }) {
        for (key, field) in [
            ("spawn", "mapSpawn"),
            ("explored", "mapExplored"),
            ("exit", "mapExit"),
            ("boss", "mapBoss"),
            ("highlight", "mapHighlight"),
        ] {
            if let Some(sprite) = project.sprite(menu.body.get(field))? {
                map.insert(key.into(), json!(sprite.name));
                sprites.push(sprite);
            }
        }
        // `mapUnknown` is a prefab: its picture.
        if let Some(documents) = project.prefab(menu.body.get("mapUnknown").get("guid").str()) {
            let prefab = Prefab::new(&documents);
            if let Some(root) = prefab.root_transform().and_then(|transform| prefab.object_of(transform))
                && let Some(sprite) = crate::loot::renderer_sprite_info(project, &prefab, root)?
            {
                map.insert("unknown".into(), json!(sprite.name));
                sprites.push(sprite);
            }
        }
    }

    let spawn = body.get("spawnCoord");
    Ok((json!({
        "door_tiles": door_tiles,
        "map": map,
        "grid_size": GRID_SIZE,
        "rooms": ROOM_COUNT,
        "spawn": [spawn.get("x").i64(), spawn.get("y").i64()],
        "open_room": room("openRoom"),
        "spawn_room": room("spawnRoom"),
        "exit_room": room("exitRoom"),
        "boss_room": room("bossRoom"),
        "by_doors": by_doors,
        "distance": distance,
    }), sprites))
}

/// A Tile asset's sprite and whether it blocks.
fn tile(project: &Project, guid: &str) -> Result<Option<(SpriteInfo, bool)>> {
    let Some(path) = project.path_of(guid).map(Path::to_path_buf) else {
        return Ok(None);
    };
    let documents = project.documents(&path)?;
    let Some(document) = documents.iter().find(|document| document.body.has("m_Sprite")) else {
        return Ok(None);
    };
    let solid = project.tile_collider(guid) == TileCollider::Solid;
    Ok(project.sprite(document.body.get("m_Sprite"))?.map(|sprite| (sprite, solid)))
}
