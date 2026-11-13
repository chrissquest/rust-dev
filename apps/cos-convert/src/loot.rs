//! Chest, loot pool and drop prefabs -> `loot.json`.
//!
//! In Unity a chest is a `Killable` (the same script enemies use) whose `lootPool`
//! names prefabs to spawn when it breaks. Those are `Spawner` prefabs, the loot
//! pools, which in turn name guns or drops; a drop is a small pickup with a
//! `DataDrop` or `HealDrop` script. Here they become three flat tables keyed by
//! prefab name, and a loot entry names a pool, a drop or a gun by that name.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{Value as Json, json};

use crate::guns::root_script;
use crate::json::{number, pair};
use crate::project::{Project, SpriteInfo};
use crate::room::Prefab;
use crate::yaml::Document;

/// Where chests and drops live, relative to the Unity `Assets` folder.
const CHESTS_FOLDER: &str = "Prefabs/Room Objects";
const DROPS_FOLDER: &str = "Prefabs/Drops";

const GUN_SCRIPTS: [&str; 3] = ["SingleBulletGun", "SprayGun", "LaserGun"];

pub struct ConvertedLoot {
    pub json: Json,
    pub sprites: Vec<SpriteInfo>,
    pub warnings: Vec<String>,
    pub chests: usize,
    pub pools: usize,
    pub drops: usize,
}

#[derive(Default)]
struct Tables {
    chests: BTreeMap<String, Json>,
    pools: BTreeMap<String, Json>,
    drops: BTreeMap<String, Json>,
    sprites: Vec<SpriteInfo>,
    warnings: Vec<String>,
}

pub fn convert(project: &Project) -> Result<ConvertedLoot> {
    let mut tables = Tables::default();

    // Every drop, including those only ever placed in a room by hand.
    for path in project.prefabs_under(DROPS_FOLDER)? {
        let documents = project.documents(&path)?;
        drop(project, &stem(&path), &documents, &mut tables)?;
    }

    for path in project.prefabs_under(CHESTS_FOLDER)? {
        let name = stem(&path);
        let documents = project.documents(&path)?;
        let prefab = Prefab::new(&documents);
        let Some((root, _, body)) = root_script(project, &prefab, &["Killable"]) else {
            continue;
        };

        let mut chest = serde_json::Map::new();
        chest.insert("max_health".into(), number(body.get("maxHealth").f32()));
        match renderer_sprite(project, &prefab, root, &mut tables)? {
            Some(sprite) => {
                chest.insert("sprite".into(), json!(sprite));
            }
            None => tables.warnings.push(format!("chest `{name}` has no sprite")),
        }
        if let Some(sprite) = project.sprite(body.get("damagedSprite"))? {
            chest.insert("damaged_sprite".into(), json!(sprite.name));
            tables.sprites.push(sprite);
        }
        if let Some(sprite) = picture_prefab(project, body.get("deathObject").get("guid").str(), &mut tables)? {
            chest.insert("dead_sprite".into(), json!(sprite));
        }
        if let Some(collider) = prefab.component(root, "BoxCollider2D") {
            let (size, offset) = (collider.body.get("m_Size"), collider.body.get("m_Offset"));
            chest.insert("size".into(), pair(size.get("x").f32(), size.get("y").f32()));
            chest.insert("offset".into(), pair(offset.get("x").f32(), offset.get("y").f32()));
        }
        let mut loot = Vec::new();
        for entry in body.get("lootPool").seq() {
            match loot_name(project, entry.get("drop").get("guid").str(), &mut tables)? {
                Some(drop) => loot.push(json!({ "drop": drop, "weight": number(entry.get("weight").f32()) })),
                None => tables.warnings.push(format!("chest `{name}` drops something that is not loot")),
            }
        }
        chest.insert("loot".into(), Json::Array(loot));
        tables.chests.insert(name, Json::Object(chest));
    }

    Ok(ConvertedLoot {
        chests: tables.chests.len(),
        pools: tables.pools.len(),
        drops: tables.drops.len(),
        json: json!({ "chests": tables.chests, "pools": tables.pools, "drops": tables.drops }),
        sprites: tables.sprites,
        warnings: tables.warnings,
    })
}

fn stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a loot entry's prefab is, by the name it has in `loot.json` or
/// `guns.json`: a pool (converted on first sight), a drop or a gun.
fn loot_name(project: &Project, guid: &str, tables: &mut Tables) -> Result<Option<String>> {
    let (Some(name), Some(documents)) = (project.stem_of(guid), project.prefab(guid)) else {
        return Ok(None);
    };
    let prefab = Prefab::new(&documents);
    let known = tables.drops.contains_key(&name) || tables.pools.contains_key(&name);
    if known || root_script(project, &prefab, &GUN_SCRIPTS).is_some() {
        return Ok(Some(name));
    }
    let Some((_, _, body)) = root_script(project, &prefab, &["Spawner"]) else {
        return Ok(None);
    };
    let mut entries = Vec::new();
    for spawn in body.get("pool").seq() {
        let guid = spawn.get("gameObject").get("guid").str();
        match project.stem_of(guid).zip(project.prefab(guid)) {
            Some((entry, documents)) => {
                let entry_prefab = Prefab::new(&documents);
                if root_script(project, &entry_prefab, &GUN_SCRIPTS).is_some() || tables.drops.contains_key(&entry) {
                    entries.push(json!({ "drop": entry, "weight": number(spawn.get("weight").f32()) }));
                } else {
                    tables.warnings.push(format!("pool `{name}` spawns `{entry}`, which is neither a gun nor a drop"));
                }
            }
            None => tables.warnings.push(format!("pool `{name}` spawns a prefab that could not be found")),
        }
    }
    tables.pools.insert(
        name.clone(),
        json!({ "multiple": body.get("multiplePoolDrops").bool(), "entries": entries }),
    );
    Ok(Some(name))
}

/// Adds a drop prefab to the table if it carries `DataDrop` or `HealDrop`.
fn drop(project: &Project, name: &str, documents: &[Document], tables: &mut Tables) -> Result<()> {
    let prefab = Prefab::new(documents);
    let Some((root, script, body)) = root_script(project, &prefab, &["DataDrop", "HealDrop"]) else {
        return Ok(());
    };
    let mut drop = serde_json::Map::new();
    drop.insert("kind".into(), json!(if script == "HealDrop" { "heal" } else { "data" }));
    drop.insert("value".into(), json!(body.get("value").i64()));
    drop.insert("pickup_range".into(), number(body.get("pickupRange").f32()));
    drop.insert("pickup_speed".into(), number(body.get("pickupSpeed").f32()));
    if let Some(sprite) = renderer_sprite(project, &prefab, root, tables)? {
        drop.insert("sprite".into(), json!(sprite));
    }
    if let Some(body) = prefab.component(root, "Rigidbody2D") {
        drop.insert("drag".into(), number(body.body.get("m_LinearDrag").f32()));
    }
    if let Some(collider) = prefab.component(root, "CircleCollider2D") {
        let offset = collider.body.get("m_Offset");
        drop.insert("radius".into(), number(collider.body.get("m_Radius").f32()));
        drop.insert("offset".into(), pair(offset.get("x").f32(), offset.get("y").f32()));
    }
    tables.drops.insert(name.to_string(), serde_json::Value::Object(drop));
    Ok(())
}

/// The sprite on `object`'s `SpriteRenderer`, added to the catalogue.
fn renderer_sprite(project: &Project, prefab: &Prefab, object: &Document, tables: &mut Tables) -> Result<Option<String>> {
    let Some(sprite) = renderer_sprite_info(project, prefab, object)? else {
        return Ok(None);
    };
    let name = sprite.name.clone();
    tables.sprites.push(sprite);
    Ok(Some(name))
}

/// The sprite on `object`'s `SpriteRenderer`, if it has one that resolves.
pub(crate) fn renderer_sprite_info(project: &Project, prefab: &Prefab, object: &Document) -> Result<Option<SpriteInfo>> {
    let Some(renderer) = prefab.component(object, "SpriteRenderer") else {
        return Ok(None);
    };
    project.sprite(renderer.body.get("m_Sprite"))
}

/// The sprite of a prefab that is only a picture, such as a broken chest.
fn picture_prefab(project: &Project, guid: &str, tables: &mut Tables) -> Result<Option<String>> {
    let Some(documents) = project.prefab(guid) else {
        return Ok(None);
    };
    let prefab = Prefab::new(&documents);
    let Some(root) = prefab.root_transform().and_then(|transform| prefab.object_of(transform)) else {
        return Ok(None);
    };
    renderer_sprite(project, &prefab, root, tables)
}
