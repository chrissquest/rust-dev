//! Enemy prefabs -> `enemies.json`.
//!
//! An enemy is a prefab under `Prefabs/Enemies` with a `Killable` (health, loot,
//! the body it leaves) and one AI script: `Warrior`, `RangedAI`,
//! `RangedAITurret` or `RangedAI_Laser` so far, each written under its own key. Other AIs are added
//! here when their step needs them. A `BasicAIAttack` on a child (a bite on
//! touch) is written as `touch`. The bullets ranged enemies fire are written to
//! `guns.json` with the players' (see `guns.rs`).
//!
//! Enemies are the first things here with animations. A Unity animation is an
//! `AnimatorController` whose one state plays a clip; the clip keys the sprite.
//! Each becomes `{"fps", "frames"}`: the sprites in order, shown evenly.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde_json::{Map, Value as Json, json};

use crate::guns::root_script;
use crate::json::{number, pair};
use crate::loot::renderer_sprite_info;
use crate::project::{Project, SpriteInfo};
use crate::room::{Prefab, color_of, script_guid};
use crate::yaml::{Document, Value};

const ENEMIES_FOLDER: &str = "Prefabs/Enemies";

pub struct ConvertedEnemies {
    pub json: Json,
    pub sprites: Vec<SpriteInfo>,
    pub warnings: Vec<String>,
    pub enemies: usize,
}

#[derive(Default)]
struct Out {
    sprites: Vec<SpriteInfo>,
    warnings: Vec<String>,
}

pub fn convert(project: &Project) -> Result<ConvertedEnemies> {
    let mut out = Out::default();
    let mut enemies = BTreeMap::new();

    for path in project.prefabs_under(ENEMIES_FOLDER)? {
        let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let documents = project.documents(&path)?;
        let prefab = Prefab::new(&documents);
        let Some((root, script, ai_body)) = root_script(project, &prefab, &AI_SCRIPTS) else {
            continue;
        };
        let Some((_, _, killable)) = root_script(project, &prefab, &["Killable"]) else {
            out.warnings.push(format!("enemy `{name}` has no Killable"));
            continue;
        };

        let mut enemy = Map::new();
        enemy.insert("max_health".into(), number(killable.get("maxHealth").f32()));
        if let Some(sprite) = renderer_sprite_info(project, &prefab, root)? {
            enemy.insert("sprite".into(), json!(sprite.name));
            out.sprites.push(sprite);
        }
        let scale = prefab.world_of_object(root).scale;
        if scale != [1.0, 1.0] {
            enemy.insert("scale".into(), pair(scale[0], scale[1]));
        }
        if let Some(renderer) = prefab.component(root, "SpriteRenderer") {
            let color = color_of(renderer.body.get("m_Color"));
            if color != [1.0, 1.0, 1.0, 1.0] {
                enemy.insert("color".into(), Json::Array(color.iter().map(|&part| number(part)).collect()));
            }
        }
        if let Some((size, offset)) = box_of(&prefab, root) {
            enemy.insert("size".into(), size);
            enemy.insert("offset".into(), offset);
        }
        if let Some(body) = prefab.component(root, "Rigidbody2D") {
            enemy.insert("mass".into(), number(body.body.get("m_Mass").f32()));
            enemy.insert("drag".into(), number(body.body.get("m_LinearDrag").f32()));
        }
        let freezable = prefab.components(root).any(|component| {
            component.class == "MonoBehaviour"
                && project.stem_of(script_guid(component)).as_deref() == Some("ElementalEffectHandler")
        });
        enemy.insert("freezable".into(), json!(freezable));
        let mut loot = Vec::new();
        for entry in killable.get("lootPool").seq() {
            match project.stem_of(entry.get("drop").get("guid").str()) {
                Some(drop) => loot.push(json!({ "drop": drop, "weight": number(entry.get("weight").f32()) })),
                None => out.warnings.push(format!("enemy `{name}` drops a prefab that could not be found")),
            }
        }
        enemy.insert("loot".into(), Json::Array(loot));
        enemy.insert("dead_sprites".into(), json!(dead_sprites(project, killable.get("deathObject"), &mut out)?));

        let mut animations = Map::new();
        // The controller the Animator starts with, if any: Warriors have none and
        // show their plain sprite until they move.
        if let Some(animator) = prefab.component(root, "Animator")
            && let Some(idle) = animation(project, animator.body.get("m_Controller").get("guid").str(), &mut out)?
        {
            animations.insert("idle".into(), idle);
        }
        match script.as_str() {
            "Warrior" => {
                enemy.insert("warrior".into(), warrior(&name, &documents, &prefab, ai_body, &mut out));
                for (key, field) in [
                    ("run_right", "runRight"),
                    ("run_left", "runLeft"),
                    ("run_front", "runFront"),
                    ("run_back", "runBack"),
                    ("attack_right", "attackRight"),
                    ("attack_left", "attackLeft"),
                    ("attack_up", "attackUp"),
                    ("attack_down", "attackDown"),
                ] {
                    match animation(project, ai_body.get(field).get("guid").str(), &mut out)? {
                        Some(animation) => {
                            animations.insert(key.into(), animation);
                        }
                        None => out.warnings.push(format!("enemy `{name}`: animation `{field}` could not be read")),
                    }
                }
            }
            "RangedAI" => {
                let mut ai = numbers(
                    ai_body,
                    &[
                        ("follow_range", "followRange"),
                        ("attack_range", "attackRange"),
                        ("move_speed", "moveSpeed"),
                        ("wander_interval", "wanderInterval"),
                        ("fire_interval", "fireInterval"),
                        ("damage", "damage"),
                        ("spotting_length", "spottingLength"),
                    ],
                );
                ai.insert("bullet".into(), json!(project.stem_of(ai_body.get("bulletFab").get("guid").str())));
                enemy.insert("ranged".into(), Json::Object(ai));
            }
            "RangedAI_Laser" => {
                let mut ai = numbers(
                    ai_body,
                    &[
                        ("follow_range", "followRange"),
                        ("attack_range", "attackRange"),
                        ("move_speed", "moveSpeed"),
                        ("wander_interval", "wanderInterval"),
                        ("max_fire_time", "maxFireTime"),
                        ("fire_interval", "fireInterval"),
                        ("damage", "damage"),
                        ("damage_rate", "damageRate"),
                        ("spotting_length", "spottingLength"),
                        ("turn_speed", "laserRotSpeed"),
                    ],
                );
                // The beam is the root's LineRenderer; its colours are set in code.
                if let Some(line) = prefab.component(root, "LineRenderer") {
                    ai.insert("width".into(), number(line.body.get("m_Parameters").get("widthMultiplier").f32()));
                }
                enemy.insert("laser".into(), Json::Object(ai));
            }
            _ => {
                let mut ai = numbers(
                    ai_body,
                    &[("attack_range", "attackRange"), ("fire_interval", "fireInterval"), ("damage", "damage")],
                );
                ai.insert("bullet".into(), json!(project.stem_of(ai_body.get("bulletFab").get("guid").str())));
                let origin = prefab.world_of_object(root).position;
                let mut muzzles = Vec::new();
                for point in ai_body.get("bulletSpawnPoints").seq() {
                    if let Some(transform) = prefab.get(point.get("fileID").i64()) {
                        let placement = prefab.world(transform);
                        muzzles.push(json!({
                            "offset": pair(placement.position[0] - origin[0], placement.position[1] - origin[1]),
                            "angle": number(placement.rotation.to_degrees()),
                        }));
                    }
                }
                ai.insert("muzzles".into(), Json::Array(muzzles));
                enemy.insert("turret".into(), Json::Object(ai));
            }
        }
        if let Some((object, body)) = documents.iter().find_map(|document| {
            (document.class == "MonoBehaviour"
                // The class is `BasicAIAttack`; its file is `MeleeAttack.cs`.
                && project.stem_of(script_guid(document)).as_deref() == Some("MeleeAttack"))
            .then(|| prefab.object_of(document).map(|object| (object, &document.body)))
            .flatten()
        }) {
            let mut touch = numbers(body, &[("damage", "damage"), ("damage_cooldown", "damageCooldown")]);
            if let Some((size, offset)) = box_of(&prefab, object) {
                touch.insert("reach".into(), json!({ "size": size, "offset": offset }));
            }
            enemy.insert("touch".into(), Json::Object(touch));
        }
        enemy.insert("animations".into(), Json::Object(animations));
        enemies.insert(name, Json::Object(enemy));
    }

    Ok(ConvertedEnemies {
        enemies: enemies.len(),
        json: json!({ "enemies": enemies }),
        sprites: out.sprites,
        warnings: out.warnings,
    })
}

/// The AI scripts converted so far.
const AI_SCRIPTS: [&str; 4] = ["Warrior", "RangedAI", "RangedAITurret", "RangedAI_Laser"];

/// Numeric fields of a script as (key, Unity field) pairs.
fn numbers(body: &Value, fields: &[(&str, &str)]) -> Map<String, Json> {
    fields.iter().map(|(key, field)| (key.to_string(), number(body.get(field).f32()))).collect()
}

/// The `Warrior` script's numbers and its reach and swing boxes.
fn warrior(name: &str, documents: &[Document], prefab: &Prefab, body: &Value, out: &mut Out) -> Json {
    let mut ai = numbers(
        body,
        &[
            ("spotting_length", "spottingLength"),
            ("follow_range", "followRange"),
            ("move_speed", "moveSpeed"),
            ("wander_interval", "wanderInterval"),
            ("damage", "damage"),
            ("damage_cooldown", "damageCooldown"),
        ],
    );
    // The always-on trigger that starts a swing is the child named `AttackCollider`.
    match documents
        .iter()
        .find(|document| document.class == "GameObject" && document.body.get("m_Name").str() == "AttackCollider")
    {
        Some(object) => {
            if let Some((size, offset)) = box_of(prefab, object) {
                ai.insert("reach".into(), json!({ "size": size, "offset": offset }));
            }
        }
        None => out.warnings.push(format!("enemy `{name}` has no AttackCollider")),
    }
    let mut swings = Map::new();
    for (key, field) in [
        ("right", "colliderAttackRight"),
        ("left", "colliderAttackLeft"),
        ("up", "colliderAttackUp"),
        ("down", "colliderAttackDown"),
    ] {
        if let Some((size, offset)) =
            prefab.get(body.get(field).get("fileID").i64()).and_then(|object| box_of(prefab, object))
        {
            swings.insert(key.into(), json!({ "size": size, "offset": offset }));
        }
    }
    ai.insert("swings".into(), Json::Object(swings));
    Json::Object(ai)
}

/// An object's `BoxCollider2D` as (`size`, `offset`) in world units, scaled as
/// the prefab scales it. Every collider so far sits on an object at its root's
/// position, so only the scale is applied.
fn box_of(prefab: &Prefab, object: &Document) -> Option<(Json, Json)> {
    let collider = prefab.component(object, "BoxCollider2D")?;
    let (size, offset) = (collider.body.get("m_Size"), collider.body.get("m_Offset"));
    let [x, y] = prefab.world_of_object(object).scale;
    Some((
        pair(size.get("x").f32() * x, size.get("y").f32() * y),
        pair(offset.get("x").f32() * x, offset.get("y").f32() * y),
    ))
}

/// The bodies a `deathObject` can show: `RandomBodyScript.possibleSprites`, one
/// picked at random when it appears, or else its renderer's one sprite.
fn dead_sprites(project: &Project, reference: &Value, out: &mut Out) -> Result<Vec<String>> {
    let guid = reference.get("guid").str();
    let Some(documents) = project.prefab(guid) else {
        return Ok(Vec::new());
    };
    let prefab = Prefab::new(&documents);
    let Some(root) = prefab.root_transform().and_then(|transform| prefab.object_of(transform)) else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    if let Some((_, _, body)) = root_script(project, &prefab, &["RandomBodyScript"]) {
        for sprite in body.get("possibleSprites").seq() {
            if let Some(sprite) = project.sprite(sprite)? {
                names.push(sprite.name.clone());
                out.sprites.push(sprite);
            }
        }
    }
    if names.is_empty()
        && let Some(sprite) = renderer_sprite_info(project, &prefab, root)?
    {
        names.push(sprite.name.clone());
        out.sprites.push(sprite);
    }
    Ok(names)
}

/// The animation an `AnimatorController` plays: its first state's clip.
fn animation(project: &Project, controller_guid: &str, out: &mut Out) -> Result<Option<Json>> {
    if controller_guid.is_empty() {
        return Ok(None);
    }
    let Some(controller) = project.path_of(controller_guid).map(Path::to_path_buf) else {
        return Ok(None);
    };
    let documents = project.documents(&controller)?;
    let Some(clip_guid) = documents
        .iter()
        .filter(|document| document.class == "AnimatorState")
        .map(|state| state.body.get("m_Motion").get("guid").str())
        .find(|guid| !guid.is_empty())
    else {
        return Ok(None);
    };
    let Some(clip) = project.path_of(clip_guid).map(Path::to_path_buf) else {
        return Ok(None);
    };
    let documents = project.documents(&clip)?;
    let Some(clip) = documents.iter().find(|document| document.class == "AnimationClip") else {
        return Ok(None);
    };
    let fps = clip.body.get("m_SampleRate").f32();
    let Some(curve) = clip
        .body
        .get("m_PPtrCurves")
        .seq()
        .iter()
        .find(|curve| curve.get("attribute").str() == "m_Sprite")
    else {
        return Ok(None);
    };
    let mut frames = Vec::new();
    for (index, key) in curve.get("curve").seq().iter().enumerate() {
        // Every clip so far keys one sprite per sample; anything else would need
        // per-frame times.
        let expected = index as f32 / fps;
        if (key.get("time").f32() - expected).abs() > 1e-3 {
            out.warnings.push(format!("{}: frames are not one per sample", clip.body.get("m_Name").str()));
        }
        match project.sprite(key.get("value"))? {
            Some(sprite) => {
                frames.push(json!(sprite.name));
                out.sprites.push(sprite);
            }
            None => out.warnings.push(format!("{}: a frame's sprite could not be resolved", clip.body.get("m_Name").str())),
        }
    }
    Ok(Some(json!({ "fps": number(fps), "frames": frames })))
}
