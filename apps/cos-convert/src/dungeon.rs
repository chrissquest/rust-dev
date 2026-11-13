//! The dungeon generator's settings -> `dungeon.json`.
//!
//! Unity's `DungeonGenerator` lives in `Scenes/Dungeon.unity`. Its inspector
//! lists, for every set of doors a room can have, the room prefabs that fit,
//! in the order it tries them; each room prefab's `RoomScript` says how far from
//! the spawn it may be placed at the least (`distanceFromSpawn`). The grid size
//! and room count are constants in the script itself.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value as Json, json};

use crate::guns::root_script;
use crate::project::Project;
use crate::room::Prefab;
use crate::slug;

const SCENE: &str = "Scenes/Dungeon.unity";
const ROOMS_FOLDER: &str = "Prefabs/Generatable Rooms";
/// `DungeonGenerator.dungeonSize` and `dungeonRooms`: constants, not fields.
const GRID_SIZE: u32 = 10;
const ROOM_COUNT: u32 = 8;
/// The door sets the generator lists, as its fields name them.
const DOOR_SETS: [&str; 15] =
    ["NSEW", "NSE", "NEW", "NSW", "SEW", "NE", "NS", "NW", "SE", "EW", "SW", "N", "E", "S", "W"];

pub fn convert(project: &Project) -> Result<Json> {
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
    for path in project.prefabs_under(ROOMS_FOLDER)? {
        let documents = project.documents(&path)?;
        let prefab = Prefab::new(&documents);
        if let Some((_, _, script)) = root_script(project, &prefab, &["RoomScript", "SpawnRoom"]) {
            let name = path.file_stem().map(|stem| slug(&stem.to_string_lossy())).unwrap_or_default();
            distance.insert(name, script.get("distanceFromSpawn").i64());
        }
    }

    let spawn = body.get("spawnCoord");
    Ok(json!({
        "grid_size": GRID_SIZE,
        "rooms": ROOM_COUNT,
        "spawn": [spawn.get("x").i64(), spawn.get("y").i64()],
        "open_room": room("openRoom"),
        "spawn_room": room("spawnRoom"),
        "exit_room": room("exitRoom"),
        "boss_room": room("bossRoom"),
        "by_doors": by_doors,
        "distance": distance,
    }))
}
