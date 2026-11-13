//! Gun, bullet and hit-effect prefabs -> `guns.json`.
//!
//! In Unity a gun is a prefab with one of three scripts (`SingleBulletGun`,
//! `SprayGun`, `LaserGun`) pointing at a bullet prefab, which points at the effect
//! it leaves when it hits. Here those become three flat tables keyed by prefab
//! name, so a gun's numbers can be read and tuned in one place.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{Map, Value as Json, json};

use crate::json::{number, pair};
use crate::project::{Project, SpriteInfo};
use crate::room::{Placement, Prefab, color_of, script_guid};
use crate::yaml::{Document, Value};

/// Where the gun prefabs live, relative to the Unity `Assets` folder.
const GUNS_FOLDER: &str = "Prefabs/Guns";

pub struct ConvertedGuns {
    pub json: Json,
    pub sprites: Vec<SpriteInfo>,
    pub warnings: Vec<String>,
    pub guns: usize,
    pub bullets: usize,
    pub effects: usize,
}

#[derive(Default)]
struct Tables {
    guns: BTreeMap<String, Json>,
    bullets: BTreeMap<String, Json>,
    effects: BTreeMap<String, Json>,
    sprites: Vec<SpriteInfo>,
    warnings: Vec<String>,
}

pub fn convert(project: &Project) -> Result<ConvertedGuns> {
    let mut tables = Tables::default();

    for path in project.prefabs_under(GUNS_FOLDER)? {
        let documents = project.documents(&path)?;
        let prefab = Prefab::new(&documents);
        let Some((root, script, body)) = root_script(project, &prefab, &["SingleBulletGun", "SprayGun", "LaserGun"])
        else {
            continue;
        };
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut gun = Map::new();
        gun.insert("id".into(), json!(body.get("ID").i64()));
        gun.insert(
            "kind".into(),
            json!(match script.as_str() {
                "SprayGun" => "spray",
                "LaserGun" => "laser",
                _ => "single",
            }),
        );
        insert_sprite(project, &prefab, root, &mut gun, &mut tables, &name)?;
        if let Some(collider) = prefab.component(root, "BoxCollider2D") {
            let size = collider.body.get("m_Size");
            gun.insert("pickup_size".into(), pair(size.get("x").f32(), size.get("y").f32()));
        }
        gun.insert("heat_capacity".into(), number(body.get("heatCapacity").f32()));
        gun.insert("heat_cooldown".into(), number(body.get("heatCooldownTime").f32()));
        gun.insert("damage".into(), number(body.get("damage").f32()));
        gun.insert("knockback".into(), number(body.get("knockback").f32()));
        let shake = body.get("shake");
        gun.insert(
            "shake".into(),
            json!({
                "time": number(shake.get("x").f32()),
                "amplitude": number(shake.get("y").f32()),
                "frequency": number(shake.get("z").f32()),
            }),
        );
        if let Some(clip) = prefab
            .component(root, "AudioSource")
            .and_then(|audio| project.stem_of(audio.body.get("m_audioClip").get("guid").str()))
        {
            gun.insert("sound".into(), json!(clip));
        }

        if script == "LaserGun" {
            gun.insert("damage_delay".into(), number(body.get("damageDelay").f32()));
            for (key, field) in [("start", "laserStart"), ("middle", "laserMiddle"), ("end", "laserEnd")] {
                if let Some(sprite) = part_sprite(project, body.get(field), &mut tables)? {
                    gun.insert(format!("laser_{key}"), json!(sprite));
                }
            }
        } else {
            gun.insert("fire_delay".into(), number(body.get("fireDelay").f32()));
            gun.insert("bullet_speed".into(), number(body.get("bulletSpeed").f32()));
            if script == "SprayGun" {
                gun.insert("bullet_count".into(), json!(body.get("bulletCount").i64()));
                gun.insert("spray_angle".into(), number(body.get("sprayAngle").f32()));
                gun.insert("random_speed".into(), number(body.get("randomSpeed").f32()));
            }
            match bullet(project, body.get("bulletFab").get("guid").str(), &mut tables)? {
                Some(bullet) => {
                    gun.insert("bullet".into(), json!(bullet));
                }
                None => tables.warnings.push(format!("gun `{name}` has no usable bullet prefab")),
            }
        }

        tables.guns.insert(name, Json::Object(gun));
    }

    Ok(ConvertedGuns {
        guns: tables.guns.len(),
        bullets: tables.bullets.len(),
        effects: tables.effects.len(),
        json: json!({ "guns": tables.guns, "bullets": tables.bullets, "effects": tables.effects }),
        sprites: tables.sprites,
        warnings: tables.warnings,
    })
}

/// The prefab's root object and the first of `scripts` attached to it, with that
/// script's fields.
fn root_script<'a>(
    project: &Project,
    prefab: &Prefab<'a>,
    scripts: &[&str],
) -> Option<(&'a Document, String, &'a Value)> {
    let root = prefab.object_of(prefab.root_transform()?)?;
    prefab.components(root).find_map(|component| {
        if component.class != "MonoBehaviour" {
            return None;
        }
        let script = project.stem_of(script_guid(component))?;
        scripts
            .contains(&script.as_str())
            .then_some((root, script, &component.body))
    })
}

/// Adds the root object's `sprite`, and its `color` and `scale` when not plain.
fn insert_sprite(
    project: &Project,
    prefab: &Prefab,
    root: &Document,
    entry: &mut Map<String, Json>,
    tables: &mut Tables,
    owner: &str,
) -> Result<()> {
    let Some(renderer) = prefab.component(root, "SpriteRenderer") else {
        return Ok(());
    };
    match project.sprite(renderer.body.get("m_Sprite"))? {
        Some(sprite) => {
            entry.insert("sprite".into(), json!(sprite.name));
            tables.sprites.push(sprite);
        }
        None => tables.warnings.push(format!("`{owner}` has a sprite that could not be resolved")),
    }
    let color = color_of(renderer.body.get("m_Color"));
    if color != [1.0, 1.0, 1.0, 1.0] {
        entry.insert("color".into(), Json::Array(color.iter().map(|&part| number(part)).collect()));
    }
    let scale = prefab
        .transform_of(root)
        .map(|transform| Placement::from_transform(&transform.body).scale)
        .unwrap_or([1.0, 1.0]);
    if scale != [1.0, 1.0] {
        entry.insert("scale".into(), pair(scale[0], scale[1]));
    }
    Ok(())
}

/// The sprite of a prefab that is just a picture, such as a piece of a laser beam.
fn part_sprite(project: &Project, reference: &Value, tables: &mut Tables) -> Result<Option<String>> {
    let Some(documents) = project.prefab(reference.get("guid").str()) else {
        return Ok(None);
    };
    let prefab = Prefab::new(&documents);
    let Some(root) = prefab.root_transform().and_then(|transform| prefab.object_of(transform)) else {
        return Ok(None);
    };
    let Some(renderer) = prefab.component(root, "SpriteRenderer") else {
        return Ok(None);
    };
    let Some(sprite) = project.sprite(renderer.body.get("m_Sprite"))? else {
        return Ok(None);
    };
    let name = sprite.name.clone();
    tables.sprites.push(sprite);
    Ok(Some(name))
}

/// Converts the bullet prefab a GUID names, once, and returns its table key.
fn bullet(project: &Project, guid: &str, tables: &mut Tables) -> Result<Option<String>> {
    let (Some(name), Some(documents)) = (project.stem_of(guid), project.prefab(guid)) else {
        return Ok(None);
    };
    if tables.bullets.contains_key(&name) {
        return Ok(Some(name));
    }
    let prefab = Prefab::new(&documents);
    let Some((root, script, body)) = root_script(project, &prefab, &["Bullet", "HomingBullet", "EnemyBullet"]) else {
        return Ok(None);
    };

    let mut entry = Map::new();
    insert_sprite(project, &prefab, root, &mut entry, tables, &name)?;
    if let Some(collider) = prefab.component(root, "BoxCollider2D") {
        let size = collider.body.get("m_Size");
        entry.insert("size".into(), pair(size.get("x").f32(), size.get("y").f32()));
    } else if let Some(collider) = prefab.component(root, "CircleCollider2D") {
        let diameter = collider.body.get("m_Radius").f32() * 2.0;
        entry.insert("size".into(), pair(diameter, diameter));
    }
    entry.insert("time_to_live".into(), number(body.get("TimeToLive").f32()));
    entry.insert("piercing".into(), json!(body.get("EnemyPiercing").bool()));
    if script == "HomingBullet" {
        entry.insert("homing".into(), number(body.get("homingPower").f32()));
    }
    // Only the homing bullets have drag; without it they would never turn, only
    // drift sideways at full speed.
    let drag = prefab
        .component(root, "Rigidbody2D")
        .map(|body| body.body.get("m_LinearDrag").f32())
        .unwrap_or(0.0);
    if drag != 0.0 {
        entry.insert("drag".into(), number(drag));
    }
    if script == "EnemyBullet" {
        // An enemy's bullet carries its own damage; a player's takes the gun's.
        entry.insert("enemy_damage".into(), number(body.get("Damage").f32()));
    }

    // Elemental riders are separate scripts on the same object.
    for component in prefab.components(root).filter(|component| component.class == "MonoBehaviour") {
        match project.stem_of(script_guid(component)).as_deref() {
            Some("Bullet_Freeze") => {
                entry.insert("freeze".into(), number(component.body.get("freezeStrength").f32()));
            }
            Some("Bullet_Burn") => {
                entry.insert("burn".into(), number(component.body.get("burnStrength").f32()));
            }
            _ => {}
        }
    }

    if let Some(effect) = effect(project, body.get("Splash").get("guid").str(), tables)? {
        entry.insert("effect".into(), json!(effect));
    }

    tables.bullets.insert(name.clone(), Json::Object(entry));
    Ok(Some(name))
}

/// Converts the hit-effect prefab a GUID names (a splash or an explosion), once.
fn effect(project: &Project, guid: &str, tables: &mut Tables) -> Result<Option<String>> {
    let (Some(name), Some(documents)) = (project.stem_of(guid), project.prefab(guid)) else {
        return Ok(None);
    };
    if tables.effects.contains_key(&name) {
        return Ok(Some(name));
    }
    let prefab = Prefab::new(&documents);
    let Some((root, script, body)) = root_script(project, &prefab, &["BulletSplash", "ExplosionScript"]) else {
        return Ok(None);
    };

    let mut entry = Map::new();
    insert_sprite(project, &prefab, root, &mut entry, tables, &name)?;
    entry.insert("time_to_live".into(), number(body.get("TimeToLive").f32()));
    if script == "ExplosionScript" {
        let radius = prefab
            .component(root, "CircleCollider2D")
            .map(|collider| collider.body.get("m_Radius").f32())
            .unwrap_or(0.0);
        entry.insert(
            "explosion".into(),
            json!({ "damage": number(body.get("Damage").f32()), "radius": number(radius) }),
        );
    }

    tables.effects.insert(name.clone(), Json::Object(entry));
    Ok(Some(name))
}
