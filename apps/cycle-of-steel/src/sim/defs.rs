//! What the game's things are: guns, bullets and hit effects, as loaded data.
//!
//! The asset files name things by string. Here they are resolved once into dense
//! tables, so the simulation refers to a gun or a bullet by a small index and
//! never looks a name up while playing.

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::data::{BulletFile, EffectFile, GunFile, GunKind, GunsFile, ShakeFile};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GunId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BulletId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(pub u16);

#[derive(Clone, Debug)]
pub struct GunDef {
    /// The key in `guns.json`, which is also what room files call it.
    pub name: String,
    pub kind: GunKind,
    pub sprite: Option<String>,
    /// Half the box the player must touch to pick the gun up.
    pub pickup_half: [f32; 2],
    pub heat_capacity: f32,
    pub heat_cooldown: f32,
    pub damage: f32,
    pub knockback: f32,
    pub shake: ShakeFile,
    pub fire_delay: f32,
    pub bullet_speed: f32,
    pub bullet_count: u32,
    /// Radians either side of the aim.
    pub spray_angle: f32,
    pub random_speed: f32,
    pub bullet: Option<BulletId>,
    /// Lasers: seconds between the beam's hits.
    pub damage_delay: f32,
    /// Lasers: the beam's start cap, middle stretch and end cap.
    pub laser_sprites: [Option<String>; 3],
}

#[derive(Clone, Debug)]
pub struct BulletDef {
    pub sprite: Option<String>,
    pub color: [f32; 4],
    pub scale: [f32; 2],
    /// Half the collision box.
    pub half: [f32; 2],
    pub time_to_live: f32,
    pub piercing: bool,
    pub effect: Option<EffectId>,
}

#[derive(Clone, Debug)]
pub struct EffectDef {
    pub sprite: Option<String>,
    pub color: [f32; 4],
    pub scale: [f32; 2],
    pub time_to_live: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Defs {
    pub guns: Vec<GunDef>,
    pub bullets: Vec<BulletDef>,
    pub effects: Vec<EffectDef>,
    gun_by_name: HashMap<String, GunId>,
}

impl Defs {
    /// Resolves the names in a `guns.json`. Tables are filled in name order, so
    /// ids do not depend on how a hash map happened to iterate.
    pub fn from_file(file: &GunsFile) -> Result<Self> {
        let mut defs = Defs::default();

        let mut effect_names: Vec<&String> = file.effects.keys().collect();
        effect_names.sort();
        let effect_ids: HashMap<&str, EffectId> = effect_names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), EffectId(index as u16)))
            .collect();
        for name in &effect_names {
            let EffectFile { sprite, color, scale, time_to_live } = file.effects[*name].clone();
            defs.effects.push(EffectDef { sprite, color, scale, time_to_live });
        }

        let mut bullet_names: Vec<&String> = file.bullets.keys().collect();
        bullet_names.sort();
        let bullet_ids: HashMap<&str, BulletId> = bullet_names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), BulletId(index as u16)))
            .collect();
        for name in &bullet_names {
            let BulletFile { sprite, color, scale, size, time_to_live, piercing, effect } =
                file.bullets[*name].clone();
            let effect = match effect {
                Some(effect) => Some(
                    *effect_ids
                        .get(effect.as_str())
                        .with_context(|| format!("bullet `{name}` leaves unknown effect `{effect}`"))?,
                ),
                None => None,
            };
            defs.bullets.push(BulletDef {
                sprite,
                color,
                scale,
                half: [size[0] * 0.5, size[1] * 0.5],
                time_to_live,
                piercing,
                effect,
            });
        }

        let mut gun_names: Vec<&String> = file.guns.keys().collect();
        gun_names.sort();
        for name in gun_names {
            let gun: &GunFile = &file.guns[name];
            let bullet = match &gun.bullet {
                Some(bullet) => Some(
                    *bullet_ids
                        .get(bullet.as_str())
                        .with_context(|| format!("gun `{name}` fires unknown bullet `{bullet}`"))?,
                ),
                None => None,
            };
            defs.gun_by_name.insert(name.clone(), GunId(defs.guns.len() as u16));
            defs.guns.push(GunDef {
                name: name.clone(),
                kind: gun.kind,
                sprite: gun.sprite.clone(),
                pickup_half: [gun.pickup_size[0] * 0.5, gun.pickup_size[1] * 0.5],
                heat_capacity: gun.heat_capacity,
                heat_cooldown: gun.heat_cooldown,
                damage: gun.damage,
                knockback: gun.knockback,
                shake: gun.shake,
                fire_delay: gun.fire_delay,
                bullet_speed: gun.bullet_speed,
                bullet_count: gun.bullet_count,
                spray_angle: gun.spray_angle.to_radians(),
                random_speed: gun.random_speed,
                bullet,
                damage_delay: gun.damage_delay,
                laser_sprites: [gun.laser_start.clone(), gun.laser_middle.clone(), gun.laser_end.clone()],
            });
        }

        Ok(defs)
    }

    pub fn gun(&self, id: GunId) -> &GunDef {
        &self.guns[id.0 as usize]
    }

    pub fn bullet(&self, id: BulletId) -> &BulletDef {
        &self.bullets[id.0 as usize]
    }

    pub fn effect(&self, id: EffectId) -> &EffectDef {
        &self.effects[id.0 as usize]
    }

    pub fn gun_named(&self, name: &str) -> Option<GunId> {
        self.gun_by_name.get(name).copied()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A pistol-like gun, a shotgun-like gun and a bullet, for tests across `sim`.
    pub(crate) fn test_defs() -> Defs {
        let file: GunsFile = serde_json::from_str(
            r#"{
              "guns": {
                "pistol": {"id": 0, "kind": "single", "sprite": "gun", "heat_capacity": 1, "heat_cooldown": 1,
                           "damage": 100, "knockback": 1, "fire_delay": 1, "bullet_speed": 6, "bullet": "round"},
                "rifle":  {"id": 6, "kind": "single", "heat_capacity": 15, "heat_cooldown": 2,
                           "damage": 75, "knockback": 5, "fire_delay": 0.4, "bullet_speed": 10, "bullet": "round"},
                "shotgun": {"id": 1, "kind": "spray", "heat_capacity": 1, "heat_cooldown": 1.5,
                           "damage": 150, "knockback": 4, "fire_delay": 1.5, "bullet_speed": 5,
                           "bullet_count": 3, "spray_angle": 20, "random_speed": 1.2, "bullet": "pellet"},
                "laser":  {"id": 8, "kind": "laser", "heat_capacity": 20, "heat_cooldown": 10,
                           "damage": 100, "knockback": 1, "damage_delay": 0.1,
                           "shake": {"time": 0.05, "amplitude": 0.25, "frequency": 1}},
                "endless_laser": {"id": 13, "kind": "laser", "heat_capacity": 0, "heat_cooldown": 0,
                           "damage": 100, "knockback": 1, "damage_delay": 0.02}
              },
              "bullets": {
                "round":  {"sprite": "b", "time_to_live": 2, "effect": "splash"},
                "pellet": {"time_to_live": 0.25, "piercing": true}
              },
              "effects": {"splash": {"sprite": "s", "time_to_live": 0.2}}
            }"#,
        )
        .unwrap();
        Defs::from_file(&file).unwrap()
    }

    #[test]
    fn names_resolve_to_ids_in_name_order() {
        let defs = test_defs();
        assert_eq!(defs.gun_named("pistol"), Some(GunId(2)));
        assert_eq!(defs.gun_named("shotgun"), Some(GunId(4)));
        assert_eq!(defs.gun_named("nope"), None);

        let pistol = defs.gun(GunId(2));
        let round = defs.bullet(pistol.bullet.unwrap());
        assert_eq!(round.half, [0.0625, 0.0625], "default size is an eighth of a unit");
        assert!(round.effect.is_some());
        assert!((defs.gun(GunId(4)).spray_angle - 20f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn a_gun_naming_a_missing_bullet_is_an_error() {
        let file: GunsFile = serde_json::from_str(
            r#"{"guns": {"g": {"id": 0, "kind": "single", "heat_capacity": 0, "heat_cooldown": 0,
                               "damage": 1, "knockback": 0, "bullet": "missing"}},
                "bullets": {}, "effects": {}}"#,
        )
        .unwrap();
        assert!(Defs::from_file(&file).is_err());
    }
}
