//! What the game's things are: guns, bullets and hit effects, chests, loot pools
//! and drops, as loaded data.
//!
//! The asset files name things by string. Here they are resolved once into dense
//! tables, so the simulation refers to a gun or a bullet by a small index and
//! never looks a name up while playing.

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::data::{
    AnimationFile, BulletFile, DropKind, EffectFile, EnemiesFile, ExplosionFile, GunFile, GunKind, GunsFile,
    LootEntryFile, LootFile, MuzzleFile, OffsetBoxFile, ShakeFile,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GunId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BulletId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChestId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PoolId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DropId(pub u16);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EnemyId(pub u16);

/// The four ways something faces or swings, in the order animations are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Right,
    Left,
    Up,
    Down,
}

/// What an enemy's Animator can be playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Animation {
    /// The controller the Animator starts with, if it has one.
    Idle,
    Run(Direction),
    Attack(Direction),
}

impl Animation {
    pub const ALL: [Animation; 9] = [
        Animation::Idle,
        Animation::Run(Direction::Right),
        Animation::Run(Direction::Left),
        Animation::Run(Direction::Up),
        Animation::Run(Direction::Down),
        Animation::Attack(Direction::Right),
        Animation::Attack(Direction::Left),
        Animation::Attack(Direction::Up),
        Animation::Attack(Direction::Down),
    ];

    /// Its place in [`EnemyDef::animations`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&animation| animation == self).expect("listed")
    }

    /// Its key in `enemies.json`. Unity calls running up "back" and down "front".
    fn key(self) -> &'static str {
        match self {
            Animation::Idle => "idle",
            Animation::Run(Direction::Right) => "run_right",
            Animation::Run(Direction::Left) => "run_left",
            Animation::Run(Direction::Up) => "run_back",
            Animation::Run(Direction::Down) => "run_front",
            Animation::Attack(Direction::Right) => "attack_right",
            Animation::Attack(Direction::Left) => "attack_left",
            Animation::Attack(Direction::Up) => "attack_up",
            Animation::Attack(Direction::Down) => "attack_down",
        }
    }
}

/// A box relative to its owner's position: half its size and its centre's offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffsetBox {
    pub half: [f32; 2],
    pub offset: [f32; 2],
}

impl From<OffsetBoxFile> for OffsetBox {
    fn from(file: OffsetBoxFile) -> Self {
        Self { half: [file.size[0] * 0.5, file.size[1] * 0.5], offset: file.offset }
    }
}

#[derive(Clone, Debug)]
pub struct WarriorDef {
    pub spotting_length: f32,
    pub follow_range: f32,
    pub move_speed: f32,
    pub wander_interval: f32,
    pub damage: f32,
    pub damage_cooldown: f32,
    pub reach: OffsetBox,
    /// Indexed like [`Direction`].
    pub swings: [OffsetBox; 4],
}

#[derive(Clone, Debug)]
pub struct RangedDef {
    pub follow_range: f32,
    pub attack_range: f32,
    pub move_speed: f32,
    pub wander_interval: f32,
    pub fire_interval: f32,
    pub damage: f32,
    pub spotting_length: f32,
    pub bullet: BulletId,
}

#[derive(Clone, Debug)]
pub struct TurretDef {
    pub attack_range: f32,
    pub fire_interval: f32,
    pub damage: f32,
    pub bullet: BulletId,
    pub muzzles: Vec<MuzzleFile>,
}

#[derive(Clone, Copy, Debug)]
pub struct TouchDef {
    pub damage: f32,
    pub damage_cooldown: f32,
    pub reach: OffsetBox,
}

/// How an enemy behaves: one of Unity's AI scripts.
#[derive(Clone, Debug)]
pub enum Ai {
    Warrior(WarriorDef),
    Ranged(RangedDef),
    Turret(TurretDef),
}

#[derive(Clone, Debug)]
pub struct EnemyDef {
    pub name: String,
    pub max_health: f32,
    pub sprite: String,
    pub scale: [f32; 2],
    pub color: [f32; 4],
    /// Half its solid box, and where that box sits from its position.
    pub half: [f32; 2],
    pub offset: [f32; 2],
    pub mass: f32,
    pub drag: f32,
    pub freezable: bool,
    pub loot: Vec<LootEntry>,
    pub dead_sprites: Vec<String>,
    pub ai: Ai,
    pub touch: Option<TouchDef>,
    /// Indexed by [`Animation::index`]; `None` where it has no such animation.
    pub animations: [Option<AnimationFile>; 9],
}

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
    /// The force steering it at a target, as Unity's `AddForce` takes it; 0 flies straight.
    pub homing: f32,
    /// Unity's linear drag, per second.
    pub drag: f32,
    /// How hard it freezes a target that can be frozen; 0 does not.
    pub freeze: f32,
}

#[derive(Clone, Debug)]
pub struct EffectDef {
    pub sprite: Option<String>,
    pub color: [f32; 4],
    pub scale: [f32; 2],
    pub time_to_live: f32,
    /// Set if the effect hurts what it touches when it appears.
    pub explosion: Option<ExplosionFile>,
}

/// What a line of a loot table spawns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loot {
    /// A gun lying on the floor.
    Gun(GunId),
    Drop(DropId),
    /// A roll on a pool. Only chests name pools; pools never do.
    Pool(PoolId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LootEntry {
    pub loot: Loot,
    /// Out of 100.
    pub weight: f32,
}

#[derive(Clone, Debug)]
pub struct ChestDef {
    pub max_health: f32,
    pub sprite: String,
    pub damaged_sprite: Option<String>,
    pub dead_sprite: Option<String>,
    /// Half its solid box, and where that box sits from the chest's position.
    pub half: [f32; 2],
    pub offset: [f32; 2],
    pub loot: Vec<LootEntry>,
}

#[derive(Clone, Debug)]
pub struct PoolDef {
    pub multiple: bool,
    pub entries: Vec<LootEntry>,
}

#[derive(Clone, Debug)]
pub struct DropDef {
    pub kind: DropKind,
    pub value: u32,
    pub sprite: String,
    pub pickup_range: f32,
    pub pickup_speed: f32,
    pub drag: f32,
    pub radius: f32,
    pub offset: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Defs {
    pub guns: Vec<GunDef>,
    pub bullets: Vec<BulletDef>,
    pub effects: Vec<EffectDef>,
    pub chests: Vec<ChestDef>,
    pub pools: Vec<PoolDef>,
    pub drops: Vec<DropDef>,
    pub enemies: Vec<EnemyDef>,
    gun_by_name: HashMap<String, GunId>,
    bullet_by_name: HashMap<String, BulletId>,
    enemy_by_name: HashMap<String, EnemyId>,
    /// Loot pools by name, for enemies' loot to name them too.
    pool_by_name: HashMap<String, PoolId>,
    chest_by_name: HashMap<String, ChestId>,
    drop_by_name: HashMap<String, DropId>,
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
            let EffectFile { sprite, color, scale, time_to_live, explosion } = file.effects[*name].clone();
            defs.effects.push(EffectDef { sprite, color, scale, time_to_live, explosion });
        }

        let mut bullet_names: Vec<&String> = file.bullets.keys().collect();
        bullet_names.sort();
        let bullet_ids: HashMap<&str, BulletId> = bullet_names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), BulletId(index as u16)))
            .collect();
        defs.bullet_by_name = bullet_ids.iter().map(|(name, id)| (name.to_string(), *id)).collect();
        for name in &bullet_names {
            let BulletFile { sprite, color, scale, size, time_to_live, piercing, effect, homing, drag, freeze } =
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
                homing,
                drag,
                freeze,
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

    /// Adds the chests, pools and drops of a `loot.json`. Loot names guns, so this
    /// comes after [`Defs::from_file`]. Tables are filled in name order.
    pub fn with_loot(mut self, file: &LootFile) -> Result<Self> {
        let mut drop_names: Vec<&String> = file.drops.keys().collect();
        drop_names.sort();
        for name in drop_names {
            let drop = &file.drops[name];
            self.drop_by_name.insert(name.clone(), DropId(self.drops.len() as u16));
            self.drops.push(DropDef {
                kind: drop.kind,
                value: drop.value,
                sprite: drop.sprite.clone(),
                pickup_range: drop.pickup_range,
                pickup_speed: drop.pickup_speed,
                drag: drop.drag,
                radius: drop.radius,
                offset: drop.offset,
            });
        }

        let mut pool_names: Vec<&String> = file.pools.keys().collect();
        pool_names.sort();
        let pool_ids: HashMap<&str, PoolId> = pool_names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), PoolId(index as u16)))
            .collect();
        self.pool_by_name = pool_ids.iter().map(|(name, id)| (name.to_string(), *id)).collect();
        for name in &pool_names {
            let pool = &file.pools[*name];
            let entries = pool
                .entries
                .iter()
                .map(|entry| self.loot_entry(entry, None))
                .collect::<Result<Vec<_>>>()
                .with_context(|| format!("in loot pool `{name}`"))?;
            self.pools.push(PoolDef { multiple: pool.multiple, entries });
        }

        let mut chest_names: Vec<&String> = file.chests.keys().collect();
        chest_names.sort();
        for name in chest_names {
            let chest = &file.chests[name];
            let loot = chest
                .loot
                .iter()
                .map(|entry| self.loot_entry(entry, Some(&pool_ids)))
                .collect::<Result<Vec<_>>>()
                .with_context(|| format!("in chest `{name}`"))?;
            self.chest_by_name.insert(name.clone(), ChestId(self.chests.len() as u16));
            self.chests.push(ChestDef {
                max_health: chest.max_health,
                sprite: chest.sprite.clone(),
                damaged_sprite: chest.damaged_sprite.clone(),
                dead_sprite: chest.dead_sprite.clone(),
                half: [chest.size[0] * 0.5, chest.size[1] * 0.5],
                offset: chest.offset,
                loot,
            });
        }
        Ok(self)
    }

    /// Adds the enemies of an `enemies.json`. Their loot names drops and pools, so
    /// this comes after [`Defs::with_loot`].
    pub fn with_enemies(mut self, file: &EnemiesFile) -> Result<Self> {
        let mut names: Vec<&String> = file.enemies.keys().collect();
        names.sort();
        let pools: HashMap<&str, PoolId> = self.pool_by_name.iter().map(|(name, id)| (name.as_str(), *id)).collect();
        for name in names {
            let enemy = &file.enemies[name];
            let loot = enemy
                .loot
                .iter()
                .map(|entry| self.loot_entry(entry, Some(&pools)))
                .collect::<Result<Vec<_>>>()
                .with_context(|| format!("in enemy `{name}`"))?;
            let bullet = |name: &str| -> Result<BulletId> {
                self.bullet_by_name.get(name).copied().with_context(|| format!("enemy fires unknown bullet `{name}`"))
            };
            let mut animations: [Option<AnimationFile>; 9] = Default::default();
            for (slot, animation) in animations.iter_mut().zip(Animation::ALL) {
                if let Some(file) = enemy.animations.get(animation.key()) {
                    anyhow::ensure!(!file.frames.is_empty() && file.fps > 0.0, "enemy `{name}`: `{}` is empty", animation.key());
                    *slot = Some(file.clone());
                }
            }
            let ai = match (&enemy.warrior, &enemy.ranged, &enemy.turret) {
                (Some(warrior), None, None) => {
                    let swing = |key: &str| -> Result<OffsetBox> {
                        Ok((*warrior.swings.get(key).with_context(|| format!("enemy `{name}` has no `{key}` swing"))?).into())
                    };
                    for animation in &Animation::ALL[1..] {
                        anyhow::ensure!(
                            animations[animation.index()].is_some(),
                            "enemy `{name}` has no `{}` animation",
                            animation.key()
                        );
                    }
                    Ai::Warrior(WarriorDef {
                        spotting_length: warrior.spotting_length,
                        follow_range: warrior.follow_range,
                        move_speed: warrior.move_speed,
                        wander_interval: warrior.wander_interval,
                        damage: warrior.damage,
                        damage_cooldown: warrior.damage_cooldown,
                        reach: warrior.reach.into(),
                        swings: [swing("right")?, swing("left")?, swing("up")?, swing("down")?],
                    })
                }
                (None, Some(ranged), None) => Ai::Ranged(RangedDef {
                    follow_range: ranged.follow_range,
                    attack_range: ranged.attack_range,
                    move_speed: ranged.move_speed,
                    wander_interval: ranged.wander_interval,
                    fire_interval: ranged.fire_interval,
                    damage: ranged.damage,
                    spotting_length: ranged.spotting_length,
                    bullet: bullet(&ranged.bullet)?,
                }),
                (None, None, Some(turret)) => Ai::Turret(TurretDef {
                    attack_range: turret.attack_range,
                    fire_interval: turret.fire_interval,
                    damage: turret.damage,
                    bullet: bullet(&turret.bullet)?,
                    muzzles: turret.muzzles.clone(),
                }),
                _ => anyhow::bail!("enemy `{name}` must have exactly one AI"),
            };
            self.enemy_by_name.insert(name.clone(), EnemyId(self.enemies.len() as u16));
            self.enemies.push(EnemyDef {
                name: name.clone(),
                max_health: enemy.max_health,
                sprite: enemy.sprite.clone(),
                scale: enemy.scale,
                color: enemy.color,
                half: [enemy.size[0] * 0.5, enemy.size[1] * 0.5],
                offset: enemy.offset,
                mass: enemy.mass,
                drag: enemy.drag,
                freezable: enemy.freezable,
                loot,
                dead_sprites: enemy.dead_sprites.clone(),
                ai,
                touch: enemy.touch.map(|touch| TouchDef {
                    damage: touch.damage,
                    damage_cooldown: touch.damage_cooldown,
                    reach: touch.reach.into(),
                }),
                animations,
            });
        }
        Ok(self)
    }

    pub fn enemy(&self, id: EnemyId) -> &EnemyDef {
        &self.enemies[id.0 as usize]
    }

    pub fn enemy_named(&self, name: &str) -> Option<EnemyId> {
        self.enemy_by_name.get(name).copied()
    }

    /// Resolves a loot line's name: a gun, a drop, or (where `pools` is given) a pool.
    fn loot_entry(&self, entry: &LootEntryFile, pools: Option<&HashMap<&str, PoolId>>) -> Result<LootEntry> {
        let name = entry.drop.as_str();
        let loot = if let Some(gun) = self.gun_named(name) {
            Loot::Gun(gun)
        } else if let Some(drop) = self.drop_by_name.get(name) {
            Loot::Drop(*drop)
        } else if let Some(pool) = pools.and_then(|pools| pools.get(name)) {
            Loot::Pool(*pool)
        } else {
            anyhow::bail!("unknown loot `{name}`");
        };
        Ok(LootEntry { loot, weight: entry.weight })
    }

    pub fn chest(&self, id: ChestId) -> &ChestDef {
        &self.chests[id.0 as usize]
    }

    pub fn pool(&self, id: PoolId) -> &PoolDef {
        &self.pools[id.0 as usize]
    }

    pub fn drop_def(&self, id: DropId) -> &DropDef {
        &self.drops[id.0 as usize]
    }

    pub fn chest_named(&self, name: &str) -> Option<ChestId> {
        self.chest_by_name.get(name).copied()
    }

    pub fn drop_named(&self, name: &str) -> Option<DropId> {
        self.drop_by_name.get(name).copied()
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

    /// Test guns and a chest whose loot is a pistol or a rifle and a data cell.
    pub(crate) fn test_defs() -> Defs {
        let loot: LootFile = serde_json::from_str(
            r#"{
              "chests": {
                "chest": {"max_health": 500, "sprite": "c", "damaged_sprite": "cd", "dead_sprite": "cx",
                          "size": [0.5, 0.28], "offset": [0, -0.11],
                          "loot": [{"drop": "guns", "weight": 100}, {"drop": "goodies", "weight": 100}]}
              },
              "pools": {
                "guns": {"multiple": false, "entries": [{"drop": "pistol", "weight": 20}, {"drop": "rifle", "weight": 20}]},
                "goodies": {"multiple": true, "entries": [{"drop": "cell", "weight": 100}, {"drop": "medkit", "weight": 0}]}
              },
              "drops": {
                "cell": {"kind": "data", "value": 3, "sprite": "d", "pickup_range": 2, "pickup_speed": 7,
                         "drag": 3, "radius": 0.125, "offset": [0, -0.05]},
                "medkit": {"kind": "heal", "value": 100, "sprite": "h", "pickup_range": 1.5, "pickup_speed": 7,
                           "drag": 3, "radius": 0.125, "offset": [0, -0.05]}
              }
            }"#,
        )
        .unwrap();
        let enemies: EnemiesFile = serde_json::from_str(
            r#"{"enemies": {"brute": {
                "max_health": 500, "sprite": "idle", "size": [0.2, 0.5], "mass": 1, "drag": 20, "freezable": true,
                "loot": [{"drop": "cell", "weight": 30}], "dead_sprites": ["body_a", "body_b"],
                "warrior": {"spotting_length": 5, "follow_range": 4, "move_speed": 10, "wander_interval": 0.25,
                            "damage": 150, "damage_cooldown": 1.5,
                            "reach": {"size": [0.225, 0.52]},
                            "swings": {"right": {"size": [0.64, 0.81], "offset": [0.26, 0.15]},
                                       "left": {"size": [0.64, 0.81], "offset": [-0.23, 0.15]},
                                       "up": {"size": [0.59, 0.71], "offset": [0, 0.3]},
                                       "down": {"size": [0.66, 0.91], "offset": [-0.02, -0.18]}}},
                "animations": {
                  "run_right": {"fps": 12, "frames": ["rr0", "rr1"]}, "run_left": {"fps": 12, "frames": ["rl0"]},
                  "run_back": {"fps": 12, "frames": ["rb0"]}, "run_front": {"fps": 12, "frames": ["rf0"]},
                  "attack_right": {"fps": 12, "frames": ["ar0"]}, "attack_left": {"fps": 12, "frames": ["al0"]},
                  "attack_up": {"fps": 12, "frames": ["au0"]}, "attack_down": {"fps": 12, "frames": ["ad0"]}}
              },
              "shooter": {
                "max_health": 200, "sprite": "crab", "size": [0.2, 0.5], "mass": 1, "drag": 15, "freezable": true,
                "loot": [], "dead_sprites": ["crab_dead"],
                "ranged": {"follow_range": 50, "attack_range": 2.5, "move_speed": 1000, "wander_interval": 0.25,
                           "fire_interval": 1, "damage": 50, "spotting_length": 5, "bullet": "round"},
                "touch": {"damage": 100, "damage_cooldown": 1.5, "reach": {"size": [0.225, 0.52]}},
                "animations": {"idle": {"fps": 12, "frames": ["c0", "c1", "c2"]}}
              },
              "turret": {
                "max_health": 500, "sprite": "turret", "size": [0.52, 0.54], "mass": 1, "drag": 35, "freezable": false,
                "loot": [], "dead_sprites": [],
                "turret": {"attack_range": 3, "fire_interval": 1.3, "damage": 50, "bullet": "round",
                           "muzzles": [{"offset": [0.218, 0], "angle": 0}, {"offset": [0, 0.216], "angle": 90}]}
              }
            }}"#,
        )
        .unwrap();
        test_gun_defs().with_loot(&loot).unwrap().with_enemies(&enemies).unwrap()
    }

    /// A pistol-like gun, a shotgun-like gun and their bullets.
    fn test_gun_defs() -> Defs {
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
                           "damage": 100, "knockback": 1, "damage_delay": 0.02},
                "bazooka": {"id": 7, "kind": "single", "heat_capacity": 1, "heat_cooldown": 2,
                           "damage": 0, "knockback": 3, "fire_delay": 2, "bullet_speed": 7, "bullet": "missile"},
                "seeker": {"id": 14, "kind": "single", "heat_capacity": 12, "heat_cooldown": 2,
                           "damage": 100, "knockback": 5, "fire_delay": 0.2, "bullet_speed": 10, "bullet": "homing"},
                "zz_freezer": {"id": 3, "kind": "single", "heat_capacity": 14, "heat_cooldown": 1.5,
                           "damage": 75, "knockback": 2, "fire_delay": 0.1, "bullet_speed": 6, "bullet": "frost"}
              },
              "bullets": {
                "round":  {"sprite": "b", "time_to_live": 2, "effect": "splash"},
                "pellet": {"time_to_live": 0.25, "piercing": true},
                "frost": {"time_to_live": 2, "freeze": 5, "effect": "splash"},
                "missile": {"time_to_live": 3, "size": [0.25, 0.1151], "effect": "blast"},
                "homing": {"time_to_live": 2, "homing": 500, "drag": 3, "effect": "splash"}
              },
              "effects": {"splash": {"sprite": "s", "time_to_live": 0.2},
                          "blast": {"time_to_live": 0.3, "explosion": {"damage": 200, "radius": 0.4422}}}
            }"#,
        )
        .unwrap();
        Defs::from_file(&file).unwrap()
    }

    #[test]
    fn names_resolve_to_ids_in_name_order() {
        let defs = test_defs();
        assert_eq!(defs.gun_named("pistol"), Some(GunId(3)));
        assert_eq!(defs.gun_named("shotgun"), Some(GunId(6)));
        assert_eq!(defs.gun_named("nope"), None);

        let pistol = defs.gun(GunId(3));
        let round = defs.bullet(pistol.bullet.unwrap());
        assert_eq!(round.half, [0.0625, 0.0625], "default size is an eighth of a unit");
        assert!(round.effect.is_some());
        assert!((defs.gun(GunId(6)).spray_angle - 20f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn loot_names_resolve_to_guns_drops_and_pools() {
        let defs = test_defs();
        let chest = defs.chest(defs.chest_named("chest").unwrap());
        assert_eq!(chest.half, [0.25, 0.14]);
        let Loot::Pool(guns) = chest.loot[0].loot else { panic!("the chest names a pool") };
        assert_eq!(defs.pool(guns).entries[1].loot, Loot::Gun(defs.gun_named("rifle").unwrap()));
        let Loot::Pool(goodies) = chest.loot[1].loot else { panic!("the chest names a pool") };
        assert_eq!(defs.pool(goodies).entries[0].loot, Loot::Drop(defs.drop_named("cell").unwrap()));
    }

    #[test]
    fn a_pool_naming_a_pool_is_an_error() {
        let loot: LootFile = serde_json::from_str(
            r#"{"chests": {}, "drops": {},
                "pools": {"a": {"multiple": true, "entries": [{"drop": "b", "weight": 1}]},
                          "b": {"multiple": true, "entries": []}}}"#,
        )
        .unwrap();
        assert!(test_gun_defs().with_loot(&loot).is_err());
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
