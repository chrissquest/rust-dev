//! Everything that happens between pulling a trigger and something being hit:
//! bullets in flight, the marks they leave, things that can be shot, and what
//! lies on the floor: guns, drops and broken chests.
//!
//! Bullets and effects live in plain vectors that are reserved once and reused, so
//! nothing is allocated while playing however much is fired.

use crystal_engine::glam::Vec2;

use super::defs::{BulletId, ChestId, Defs, EffectId, GunDef};
use super::elemental::Elemental;
use super::guns::Gun;
use super::player::Player;
use super::enemy::{Enemy, SHOT_CAPACITY, Shot};
use super::loot::{DROP_CAPACITY, Drop, PICKUP_CAPACITY, Remains, RemainsKind, Spawned, spill};
use super::rng::Rng;
use super::room::{Cell, Room};
use super::{DT, Event};
use crate::data::{ExplosionFile, GunKind};

/// Room for this many bullets and effects before the vectors would grow.
const BULLET_CAPACITY: usize = 512;
const EFFECT_CAPACITY: usize = 256;
const DAMAGE_NUMBER_CAPACITY: usize = 128;

/// How long a damage number stays (`DamageNumberScript.Start`).
pub const DAMAGE_NUMBER_LIFE: f32 = 1.4;
/// How far a damage number climbs each step. `DamageNumberScript.Update` adds
/// 0.001 every rendered frame, so Unity's speed depends on the frame rate; one
/// step here stands for one frame at 60 frames a second.
const DAMAGE_NUMBER_RISE: f32 = 0.001;
/// A damage number appears up to this far from its target's position on each
/// axis, so that several do not hide one another (`Killable.PopupDamageNumber`).
const DAMAGE_NUMBER_SCATTER: f32 = 0.2;

/// How far a laser reaches when nothing is in its way (`LaserGun.updateFire`).
pub const LASER_RANGE: f32 = 20.0;
/// A crit does this many times the gun's damage.
const CRIT_MULTIPLIER: f32 = 2.0;
/// Unity's physics step (`TimeManager.asset`, Fixed Timestep). Forces and drag
/// were tuned against it, so they are converted from it rather than from [`DT`].
pub(super) const UNITY_STEP: f32 = 0.02;

#[derive(Clone, Debug)]
pub struct Bullet {
    pub def: BulletId,
    pub position: Vec2,
    /// Position one step ago, for the view to blend from.
    pub previous_position: Vec2,
    pub velocity: Vec2,
    /// Which way the sprite points, counter-clockwise radians.
    pub rotation: f32,
    /// Seconds left before it disappears by itself.
    pub time_left: f32,
    pub damage: f32,
    pub knockback: f32,
    pub crit: bool,
    /// Targets a piercing bullet has already hit, so each is hit once.
    hit: Vec<u32>,
}

/// What a bullet leaves where it stops: a splash that fades after a moment.
#[derive(Clone, Debug)]
pub struct Effect {
    pub def: EffectId,
    pub position: Vec2,
    pub rotation: f32,
    /// Splashes take the colour of the bullet that made them.
    pub color: [f32; 4],
    pub time_left: f32,
}

/// The number that floats up from something when it is hurt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DamageNumber {
    pub position: Vec2,
    /// The damage dealt, crit doubling included.
    pub amount: f32,
    /// Crits are drawn larger and in another colour.
    pub crit: bool,
    pub time_left: f32,
}

/// A laser's beam as it is this step. It exists only while the laser fires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beam {
    /// The gun's muzzle.
    pub origin: Vec2,
    /// A unit vector.
    pub direction: Vec2,
    /// From the muzzle to whatever stops the beam, at most [`LASER_RANGE`].
    pub length: f32,
}

/// Something bullets hit and hurt: the hub's dummy, a chest or an enemy.
#[derive(Clone, Debug)]
pub struct Target {
    /// Stays the same for the target's life, unlike its place in the list.
    pub id: u32,
    /// The centre of its box.
    pub position: Vec2,
    pub half: Vec2,
    /// Where its Unity object stands, which its box may be offset from. Damage
    /// numbers, loot and remains appear here.
    pub origin: Vec2,
    /// Set for chests, which are solid and leave loot and remains.
    pub chest: Option<ChestId>,
    /// Set for enemies, which move, fight, and leave loot and a body.
    pub enemy: Option<Enemy>,
    pub health: f32,
    pub max_health: f32,
    /// When it was last hit, in world seconds, for the view to flash it.
    pub last_hit: f64,
    /// Set if it can be frozen: in Unity, if it has an `ElementalEffectHandler`.
    pub elemental: Option<Elemental>,
}

impl Target {
    /// `Killable.DamageKillable` swaps in the damaged sprite below 70% health.
    pub fn is_damaged(&self) -> bool {
        self.health < self.max_health * 0.7
    }
}

/// A gun lying on the floor.
#[derive(Clone, Debug)]
pub struct Pickup {
    pub gun: Gun,
    pub position: Vec2,
}

/// A spot that moves the player when they interact on it. The hub hides two: the
/// way into the gun room and the way back out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Teleport {
    pub position: Vec2,
    pub half: Vec2,
    /// Added to the player's position.
    pub offset: Vec2,
}

/// What pressing interact would act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interaction {
    /// An index into [`Combat::pickups`].
    Pickup(usize),
    /// An index into [`Combat::teleports`].
    Teleport(usize),
    /// An index into [`Combat::exits`]: the exit room's stairs out.
    Exit(usize),
}

#[derive(Default)]
pub struct Combat {
    pub bullets: Vec<Bullet>,
    pub effects: Vec<Effect>,
    pub targets: Vec<Target>,
    pub pickups: Vec<Pickup>,
    pub teleports: Vec<Teleport>,
    pub drops: Vec<Drop>,
    pub remains: Vec<Remains>,
    /// The player's laser beam, while it fires.
    pub beam: Option<Beam>,
    pub damage_numbers: Vec<DamageNumber>,
    /// Bullets enemies fired.
    pub shots: Vec<Shot>,
    /// Explosions while they can still hurt.
    pub blasts: Vec<Blast>,
    /// The room's water and lava pools.
    pub grates: Vec<Grate>,
    /// The boxes at doorways that keep enemies in their room, as (centre, half size).
    pub barriers: Vec<(Vec2, Vec2)>,
    /// Sounds asked for this step, for the world to pass on.
    pub sounds: Vec<super::Sound>,
    /// Interact here to leave the dungeon (`ExitDungeon`), as (centre, half size).
    pub exits: Vec<(Vec2, Vec2)>,
    /// Walk in here to enter the dungeon (`ExitHub`), as (centre, half size).
    pub entrances: Vec<(Vec2, Vec2)>,
}

/// A floor grate over water or lava (`EffectGrate`): a trigger box that wets or
/// heats enemies standing on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grate {
    pub position: Vec2,
    pub half: Vec2,
    pub kind: GrateKind,
    /// How long the effect lasts once set, in Unity's 0.1-a-frame units.
    pub duration: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrateKind {
    Water,
    Lava,
}

/// Room for this many live explosions before the oldest makes way.
const BLAST_CAPACITY: usize = 32;
/// How many targets one explosion remembers being inside it.
const BLAST_MEMORY: usize = 32;

/// An explosion while its trigger lasts (`ExplosionScript`, the effect's
/// `TimeToLive`). Anything that comes into it is hurt and pushed once; leaving
/// and coming back in hurts again, as `OnTriggerEnter2D` would.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blast {
    pub center: Vec2,
    explosion: ExplosionFile,
    time_left: f32,
    /// Ids of the targets inside it now.
    inside: [u32; BLAST_MEMORY],
    inside_count: usize,
    player_inside: bool,
}

impl Combat {
    pub fn new() -> Self {
        Self {
            bullets: Vec::with_capacity(BULLET_CAPACITY),
            effects: Vec::with_capacity(EFFECT_CAPACITY),
            targets: Vec::new(),
            pickups: Vec::with_capacity(PICKUP_CAPACITY),
            teleports: Vec::new(),
            drops: Vec::with_capacity(DROP_CAPACITY),
            remains: Vec::new(),
            beam: None,
            damage_numbers: Vec::with_capacity(DAMAGE_NUMBER_CAPACITY),
            shots: Vec::with_capacity(SHOT_CAPACITY),
            blasts: Vec::with_capacity(BLAST_CAPACITY),
            grates: Vec::new(),
            barriers: Vec::new(),
            sounds: Vec::with_capacity(32),
            exits: Vec::new(),
            entrances: Vec::new(),
        }
    }

    /// Fires `gun` once from `muzzle` along the unit vector `aim`.
    ///
    /// `carrier_velocity` is added to each bullet, as in Unity: shots fired while
    /// running forwards fly faster. `crit_chance` is 0..=1.
    #[allow(clippy::too_many_arguments)]
    pub fn fire(
        &mut self,
        defs: &Defs,
        gun: &GunDef,
        muzzle: Vec2,
        aim: Vec2,
        carrier_velocity: Vec2,
        crit_chance: f32,
        rng: &mut Rng,
        events: &mut Vec<Event>,
    ) {
        if gun.shake.time > 0.0 {
            events.push(Event::Shake(gun.shake));
        }
        let Some(bullet) = gun.bullet else {
            return;
        };
        let def = defs.bullet(bullet);

        let count = if gun.kind == GunKind::Spray { gun.bullet_count } else { 1 };
        for _ in 0..count {
            let (direction, speed) = if gun.kind == GunKind::Spray {
                let angle = rng.range(-gun.spray_angle, gun.spray_angle);
                // Unity's Random.Range(1, randomSpeed), whichever way round they are.
                (Vec2::from_angle(angle).rotate(aim), gun.bullet_speed * rng.range(1.0, gun.random_speed))
            } else {
                (aim, gun.bullet_speed)
            };
            // Bullet sprites are drawn pointing left, so they are turned to face
            // away from where they are going.
            let rotation = (-direction).to_angle();

            if self.bullets.len() == self.bullets.capacity() {
                // Full: the oldest bullet makes way rather than the list growing.
                self.bullets.remove(0);
            }
            self.bullets.push(Bullet {
                def: bullet,
                position: muzzle,
                previous_position: muzzle,
                velocity: carrier_velocity + direction * speed,
                rotation,
                time_left: def.time_to_live,
                damage: gun.damage,
                knockback: gun.knockback,
                crit: rng.chance(crit_chance),
                hit: Vec::new(),
            });
        }
    }

    /// Shines `gun`'s laser from `muzzle` along the unit vector `aim` for one step,
    /// as Unity's `LaserGun.updateFire` does every frame the trigger is held.
    ///
    /// The beam stops at the first wall or target; ledges do not stop it. While it
    /// touches either, it heats the gun every `damage_delay`, and hurts the target
    /// if that is what it touches. The caller has checked the gun is not locked.
    #[allow(clippy::too_many_arguments)]
    pub fn fire_laser(
        &mut self,
        gun: &mut Gun,
        def: &GunDef,
        room: &Room,
        muzzle: Vec2,
        aim: Vec2,
        crit_chance: f32,
        now: f64,
        rng: &mut Rng,
        events: &mut Vec<Event>,
    ) {
        if def.shake.time > 0.0 {
            events.push(Event::Shake(def.shake));
        }
        let mut length = room.ray_hits_wall(muzzle, aim, LASER_RANGE);
        let mut touched = None;
        for (index, target) in self.targets.iter().enumerate() {
            let limit = length.unwrap_or(LASER_RANGE);
            if let Some(distance) = ray_hits_box(muzzle, aim, target.position, target.half).filter(|&distance| distance < limit) {
                length = Some(distance);
                touched = Some(index);
            }
        }
        self.beam = Some(Beam { origin: muzzle, direction: aim, length: length.unwrap_or(LASER_RANGE) });
        if length.is_none() || !gun.try_laser_hit(def, now) {
            return;
        }
        if let Some(index) = touched {
            // Unity rolls the crit on every hit of the beam, not once per trigger pull.
            let crit = rng.chance(crit_chance);
            let damage = if crit { def.damage * CRIT_MULTIPLIER } else { def.damage };
            // `LaserGun.updateFire` pushes along the beam.
            let knockback = aim * def.knockback;
            hurt(&mut self.targets[index], &mut self.damage_numbers, damage, crit, knockback, now, rng);
        }
    }

    /// Moves bullets, resolves what they hit, and ages effects.
    ///
    /// `player` is where the player stands: homing bullets chase the target
    /// nearest the player, not the one nearest themselves (`HomingBullet.Update`
    /// compares `DistanceToPlayer`), so every homing bullet in flight agrees on
    /// one target. Ties go to the earlier target, as Unity's loop does.
    pub fn step(&mut self, defs: &Defs, room: &Room, player: &mut Player, now: f64, rng: &mut Rng) {
        let player_position = player.position;
        let chased = self
            .targets
            .iter()
            .min_by(|a, b| {
                a.position.distance_squared(player_position).total_cmp(&b.position.distance_squared(player_position))
            })
            .map(|target| target.position);

        let mut index = 0;
        while index < self.bullets.len() {
            let bullet = &mut self.bullets[index];
            let def = defs.bullet(bullet.def);
            bullet.previous_position = bullet.position;
            if let (true, Some(chased)) = (def.homing > 0.0, chased) {
                let toward = (chased - bullet.position).normalize_or_zero();
                bullet.velocity += toward * homing_acceleration(def.homing) * DT;
                if toward != Vec2::ZERO {
                    // Unity points a homing bullet at its target, not along its
                    // flight (`Quaternion.FromToRotation(Vector3.left, target)`).
                    bullet.rotation = (-toward).to_angle();
                }
            }
            bullet.velocity *= drag_kept(def.drag);
            bullet.position += bullet.velocity * DT;
            bullet.time_left -= DT;

            let half = Vec2::from(def.half);
            let mut stopped = bullet_hits_wall(room, bullet.position, half);

            if !stopped {
                for target in &mut self.targets {
                    if !boxes_overlap(bullet.position, half, target.position, target.half)
                        || bullet.hit.contains(&target.id)
                    {
                        continue;
                    }
                    let damage = if bullet.crit { bullet.damage * CRIT_MULTIPLIER } else { bullet.damage };
                    // `Bullet.OnTriggerEnter2D` pushes along the bullet's flight.
                    let knockback = bullet.velocity.normalize_or_zero() * bullet.knockback;
                    hurt(target, &mut self.damage_numbers, damage, bullet.crit, knockback, now, rng);
                    // `Bullet_Freeze.OnTriggerEnter2D`, beside the bullet's own hit.
                    if let (true, Some(elemental)) = (def.freeze > 0.0, target.elemental.as_mut()) {
                        elemental.freeze(def.freeze);
                    }
                    if def.piercing {
                        bullet.hit.push(target.id);
                    } else {
                        stopped = true;
                        break;
                    }
                }
            }

            if stopped {
                if let Some(effect) = def.effect {
                    let effect_def = defs.effect(effect);
                    if self.effects.len() == self.effects.capacity() {
                        self.effects.remove(0);
                    }
                    self.effects.push(Effect {
                        def: effect,
                        position: bullet.position,
                        rotation: bullet.rotation,
                        color: def.color,
                        time_left: effect_def.time_to_live,
                    });
                    if let Some(explosion) = effect_def.explosion {
                        start_blast(&mut self.blasts, bullet.position, explosion, effect_def.time_to_live);
                    }
                    if effect_def.sound.is_some() {
                        self.sounds.push(super::Sound::Effect(effect));
                    }
                }
                self.bullets.swap_remove(index);
            } else if bullet.time_left <= 0.0 {
                self.bullets.swap_remove(index);
            } else {
                index += 1;
            }
        }

        self.step_blasts(player, now, rng);
        // Blasts from deaths here first hurt next step: Unity's trigger notices
        // what is inside it on the physics step after it appears.
        self.break_dead_targets(defs, rng);
        for target in &mut self.targets {
            if let (Some(elemental), Some(enemy)) = (target.elemental.as_mut(), target.enemy.as_ref()) {
                // `EffectGrate.OnTriggerStay2D`, every step a walker's box touches one.
                // Ledges are on the grates' physics layer, which flyers pass.
                if !defs.enemy(enemy.def).flying {
                    for grate in &self.grates {
                        if boxes_overlap(target.position, target.half, grate.position, grate.half) {
                            match grate.kind {
                                GrateKind::Water => elemental.wet(grate.duration),
                                GrateKind::Lava => elemental.burn_in_lava(grate.duration),
                            }
                        }
                    }
                }
            }
        }
        for elemental in self.targets.iter_mut().filter_map(|target| target.elemental.as_mut()) {
            elemental.step();
        }

        self.damage_numbers.retain_mut(|number| {
            number.position.y += DAMAGE_NUMBER_RISE;
            number.time_left -= DT;
            number.time_left > 0.0
        });

        self.effects.retain_mut(|effect| {
            effect.time_left -= DT;
            effect.time_left > 0.0
        });
    }

    /// Removes every target out of health, as `Killable.Die`: a chest spills its
    /// loot where it stood and leaves its remains. Order is kept, as homing
    /// bullets break ties by it.
    fn break_dead_targets(&mut self, defs: &Defs, rng: &mut Rng) {
        let mut index = 0;
        while index < self.targets.len() {
            if self.targets[index].health > 0.0 {
                index += 1;
                continue;
            }
            let target = self.targets.remove(index);
            let mut out = Spawned { pickups: &mut self.pickups, drops: &mut self.drops };
            if let Some(chest) = target.chest {
                spill(&defs.chest(chest).loot, defs, target.origin, rng, &mut out);
                self.remains.push(Remains { kind: RemainsKind::Chest(chest), position: target.origin });
            }
            if let Some(enemy) = &target.enemy {
                let def = defs.enemy(enemy.def);
                spill(&def.loot, defs, target.origin, rng, &mut out);
                // `Killable.Die`: an explosive enemy leaves its explosion, which
                // hurts the player and whatever else is near, as any blast does.
                if let Some(effect) = def.explosion {
                    let effect_def = defs.effect(effect);
                    if self.effects.len() == self.effects.capacity() {
                        self.effects.remove(0);
                    }
                    self.effects.push(Effect {
                        def: effect,
                        position: target.origin,
                        rotation: 0.0,
                        color: effect_def.color,
                        time_left: effect_def.time_to_live,
                    });
                    if let Some(explosion) = effect_def.explosion {
                        start_blast(&mut self.blasts, target.origin, explosion, effect_def.time_to_live);
                    }
                    if effect_def.sound.is_some() {
                        self.sounds.push(super::Sound::Effect(effect));
                    }
                }
                if !def.dead_sprites.is_empty() {
                    // `RandomBodyScript`: `Random.Range(0, count)`.
                    let pick = ((rng.unit() * def.dead_sprites.len() as f32) as usize).min(def.dead_sprites.len() - 1);
                    self.remains.push(Remains { kind: RemainsKind::Body(enemy.def, pick as u8), position: target.origin });
                    if def.death_sound.is_some() {
                        self.sounds.push(super::Sound::Death(enemy.def));
                    }
                }
            }
        }
    }

    /// The boxes that stop walkers: chests, which in Unity have a solid collider
    /// on a body frozen in place.
    pub fn solids(&self) -> impl Iterator<Item = (Vec2, Vec2)> + Clone + '_ {
        self.targets
            .iter()
            .filter(|target| target.chest.is_some())
            .map(|target| (target.position, target.half))
    }

    /// What interacting would act on: the nearest thing whose box the player is
    /// touching, as Unity picked the closest interactable.
    pub fn interaction(&self, defs: &Defs, player: Vec2, player_half: Vec2) -> Option<Interaction> {
        let pickups = self.pickups.iter().enumerate().map(|(index, pickup)| {
            let half = Vec2::from(defs.gun(pickup.gun.def).pickup_half);
            (Interaction::Pickup(index), pickup.position, half)
        });
        let teleports = self
            .teleports
            .iter()
            .enumerate()
            .map(|(index, teleport)| (Interaction::Teleport(index), teleport.position, teleport.half));

        let exits = self.exits.iter().enumerate().map(|(index, &(center, half))| (Interaction::Exit(index), center, half));
        pickups
            .chain(teleports)
            .chain(exits)
            .filter(|(_, position, half)| boxes_overlap(player, player_half, *position, *half))
            .min_by(|(_, a, _), (_, b, _)| a.distance_squared(player).total_cmp(&b.distance_squared(player)))
            .map(|(interaction, _, _)| interaction)
    }
}

fn start_blast(blasts: &mut Vec<Blast>, center: Vec2, explosion: ExplosionFile, time_to_live: f32) {
    if blasts.len() == blasts.capacity() {
        blasts.remove(0);
    }
    blasts.push(Blast {
        center,
        explosion,
        time_left: time_to_live,
        inside: [0; BLAST_MEMORY],
        inside_count: 0,
        player_inside: false,
    });
}

impl Combat {
    /// One step of every live explosion (`ExplosionScript.OnTriggerEnter2D`):
    /// whatever has come into its circle since the last step is hurt and pushed.
    ///
    /// As in Unity the damage is the same anywhere inside the circle, walls do
    /// not shelter anything, and it is separate from the direct hit of the bullet
    /// that set it off (the bazooka's missile itself does 0). It hurts the player
    /// too, the bazooka's owner included, and pushes them, as
    /// `PlayerData.DamagePlayer` does: only when the blow lands.
    fn step_blasts(&mut self, player: &mut Player, now: f64, rng: &mut Rng) {
        for blast in &mut self.blasts {
            let explosion = blast.explosion;
            let push_radius = explosion.knockback_radius.unwrap_or(explosion.radius);
            let mut still_inside = [0; BLAST_MEMORY];
            let mut count = 0;
            for target in &mut self.targets {
                if !circle_overlaps_box(blast.center, explosion.radius, target.position, target.half) {
                    continue;
                }
                let was_inside = blast.inside[..blast.inside_count].contains(&target.id);
                if !was_inside {
                    // `ExplosionScript` reports its damage as a crit, so it shows as one,
                    // and pushes away from the centre by three times the offset over the radius.
                    let knockback = (target.origin - blast.center) / push_radius * 3.0;
                    hurt(target, &mut self.damage_numbers, explosion.damage, true, knockback, now, rng);
                }
                if count < BLAST_MEMORY {
                    still_inside[count] = target.id;
                    count += 1;
                }
            }
            blast.inside = still_inside;
            blast.inside_count = count;

            let player_inside = circle_overlaps_box(blast.center, explosion.radius, player.position, super::player::HALF_SIZE);
            if player_inside && !blast.player_inside {
                let knockback = (player.position - blast.center) / push_radius * 3.0;
                if player.hurt(explosion.damage, now) {
                    player.velocity += knockback;
                }
            }
            blast.player_inside = player_inside;
            blast.time_left -= DT;
        }
        self.blasts.retain(|blast| blast.time_left > 0.0);
    }
}

/// Takes `amount` from a target's health and floats the number up from it, as
/// `Killable.DamageKillable` does. A hit for nothing (the bazooka's missile)
/// shows no number. An enemy is pushed by the `knockback` impulse; chests are
/// frozen in place in Unity, and the dummy does not move.
#[allow(clippy::too_many_arguments)]
fn hurt(
    target: &mut Target,
    numbers: &mut Vec<DamageNumber>,
    amount: f32,
    crit: bool,
    knockback: Vec2,
    now: f64,
    rng: &mut Rng,
) {
    target.health -= amount;
    target.last_hit = now;
    if let Some(enemy) = &mut target.enemy {
        enemy.velocity += knockback / enemy.mass;
    }
    if amount <= 0.0 {
        return;
    }
    if numbers.len() == numbers.capacity() {
        numbers.remove(0);
    }
    let scatter = Vec2::new(
        rng.range(-DAMAGE_NUMBER_SCATTER, DAMAGE_NUMBER_SCATTER),
        rng.range(-DAMAGE_NUMBER_SCATTER, DAMAGE_NUMBER_SCATTER),
    );
    numbers.push(DamageNumber {
        position: target.origin + scatter,
        amount,
        crit,
        time_left: DAMAGE_NUMBER_LIFE,
    });
}

/// The acceleration, in units/s^2, of a homing bullet steered with force `homing`.
///
/// `HomingBullet.Update` calls `AddForce(direction * 500 * Time.deltaTime)` every
/// frame. Forces pile up until the next physics step, so one step sees a force of
/// `500 * UNITY_STEP` in total whatever the frame rate, and the bullets' mass is
/// 1: 10 units/s^2. (The script hard-codes 500 and ignores its own `homingPower`
/// field; both prefabs set that to 500 as well, so reading the field is the same.)
fn homing_acceleration(homing: f32) -> f32 {
    homing * UNITY_STEP
}

/// The share of its velocity a body keeps over one step of [`DT`] under Unity's
/// linear `drag`. Unity's 2D physics is Box2D, which after adding forces divides
/// velocity by `1 + drag * UNITY_STEP` each physics step; this is the same decay
/// spread over our shorter step. (Unity's 3D drag, `1 - drag * dt`, is not what
/// a `Rigidbody2D` does, and with the large drags enemies have, 15 to 35, the two
/// are far apart.) Homing bullets have drag 3 and so settle at about 3.3
/// units/s; other bullets have none.
pub(super) fn drag_kept(drag: f32) -> f32 {
    if drag == 0.0 {
        return 1.0;
    }
    (1.0 / (1.0 + drag * UNITY_STEP)).powf(DT / UNITY_STEP)
}

/// Bullets fly over ledges; only walls stop them.
pub(super) fn bullet_hits_wall(room: &Room, center: Vec2, half: Vec2) -> bool {
    let (min, max) = room.cell_range(center, half);
    (min.1..=max.1).any(|y| (min.0..=max.0).any(|x| room.cell_at(x, y) == Cell::Wall))
}

/// How far along a ray from `origin` in the unit vector `direction` a box is first
/// touched. A ray starting inside the box touches it at once.
pub(super) fn ray_hits_box(origin: Vec2, direction: Vec2, center: Vec2, half: Vec2) -> Option<f32> {
    let mut enter = 0.0_f32;
    let mut leave = f32::INFINITY;
    for axis in 0..2 {
        let (low, high) = (center[axis] - half[axis] - origin[axis], center[axis] + half[axis] - origin[axis]);
        if direction[axis] == 0.0 {
            if low > 0.0 || high < 0.0 {
                return None;
            }
            continue;
        }
        let (a, b) = (low / direction[axis], high / direction[axis]);
        enter = enter.max(a.min(b));
        leave = leave.min(a.max(b));
    }
    (enter <= leave).then_some(enter)
}

pub(super) fn circle_overlaps_box(center: Vec2, radius: f32, box_center: Vec2, half: Vec2) -> bool {
    let nearest = center.clamp(box_center - half, box_center + half);
    nearest.distance_squared(center) < radius * radius
}

pub fn boxes_overlap(a: Vec2, a_half: Vec2, b: Vec2, b_half: Vec2) -> bool {
    let gap = (a - b).abs();
    gap.x < a_half.x + b_half.x && gap.y < a_half.y + b_half.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::defs::tests::test_defs;
    use crate::sim::loot::{Remains, RemainsKind};

    fn fire(combat: &mut Combat, defs: &Defs, gun: &str, from: Vec2, aim: Vec2) -> Vec<Event> {
        let mut events = Vec::new();
        let def = defs.gun(defs.gun_named(gun).unwrap());
        combat.fire(defs, def, from, aim, Vec2::ZERO, 0.0, &mut Rng::new(3), &mut events);
        events
    }

    fn target(id: u32, position: Vec2, health: f32) -> Target {
        Target {
            id,
            position,
            half: Vec2::new(0.1, 0.25),
            health,
            max_health: health,
            origin: position,
            chest: None,
            enemy: None,
            last_hit: f64::NEG_INFINITY,
            elemental: None,
        }
    }

    #[test]
    fn a_broken_chest_spills_its_loot_and_leaves_remains_where_it_stood() {
        let defs = test_defs();
        let room = Room::test_room();
        let chest = defs.chest_named("chest").unwrap();
        let mut combat = Combat::new();
        let origin = Vec2::new(8.0, 4.0);
        combat.targets.push(Target {
            origin,
            chest: Some(chest),
            position: origin + Vec2::new(0.0, -0.11),
            ..target(0, origin, 500.0)
        });
        assert_eq!(combat.solids().count(), 1);
        let mut rng = Rng::new(3);
        combat.targets[0].health = 0.0;
        combat.step(&defs, &room, &mut Player::new(Vec2::new(2.0, 2.0)), 0.0, &mut rng);
        assert!(combat.targets.is_empty());
        assert_eq!(combat.solids().count(), 0, "a broken chest no longer blocks");
        assert_eq!(combat.remains, vec![Remains { kind: RemainsKind::Chest(chest), position: origin }]);
        assert_eq!(combat.pickups.len(), 1);
        assert_eq!(combat.pickups[0].position, origin);
        assert_eq!(combat.drops.len(), 1);
    }

    #[test]
    fn a_blast_hurts_what_walks_into_it_once_while_it_lasts() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        let blast = ExplosionFile { damage: 200.0, radius: 0.44, knockback_radius: None };
        start_blast(&mut combat.blasts, Vec2::new(6.0, 4.5), blast, 0.3);
        // Outside at first, then inside for the rest of the blast's life.
        combat.targets.push(target(1, Vec2::new(7.0, 4.5), 1000.0));
        let mut rng = Rng::new(1);
        let mut player = Player::new(Vec2::new(2.0, 2.0));
        combat.step(&defs, &room, &mut player, 0.0, &mut rng);
        assert_eq!(combat.targets[0].health, 1000.0);
        combat.targets[0].position = Vec2::new(6.2, 4.5);
        for step in 1..20 {
            combat.step(&defs, &room, &mut player, step as f64 * DT as f64, &mut rng);
        }
        assert_eq!(combat.targets[0].health, 800.0, "hurt once on walking in");
        assert!(combat.blasts.is_empty(), "gone after its 0.3 s");
        assert_eq!(player.health, crate::sim::player::MAX_HEALTH, "the player was never in it");
    }

    #[test]
    fn a_chest_looks_damaged_below_seventy_percent() {
        let mut chest = target(0, Vec2::ZERO, 500.0);
        chest.health = 351.0;
        assert!(!chest.is_damaged());
        chest.health = 349.0;
        assert!(chest.is_damaged());
    }

    #[test]
    fn a_freezing_bullet_freezes_only_what_can_be_frozen() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        // The nearer target has Unity's handler; the one behind it does not.
        combat.targets.push(Target { elemental: Some(Elemental::default()), ..target(1, Vec2::new(9.0, 4.5), 1000.0) });
        combat.targets.push(target(2, Vec2::new(9.5, 4.5), 1000.0));
        fire(&mut combat, &defs, "zz_freezer", PLAYER, Vec2::X);
        run(&mut combat, &defs, &room, 12);
        let frozen = combat.targets[0].elemental.unwrap();
        assert!(frozen.is_frozen());
        assert_eq!(combat.targets[0].health, 925.0, "it still does its damage");
        assert!(frozen.added_drag() > 4.0 && frozen.added_drag() < 5.0, "5, already thawing");

        // It wears off by itself in under a second.
        run(&mut combat, &defs, &room, 60);
        assert!(!combat.targets[0].elemental.unwrap().is_frozen());

        // The same shot at the plain target does damage and nothing else, and an
        // ordinary bullet does not freeze.
        combat.targets.remove(0);
        fire(&mut combat, &defs, "zz_freezer", PLAYER, Vec2::X);
        combat.targets.push(Target { elemental: Some(Elemental::default()), ..target(3, Vec2::new(7.0, 4.5), 1000.0) });
        fire(&mut combat, &defs, "pistol", PLAYER, -Vec2::X);
        run(&mut combat, &defs, &room, 20);
        assert_eq!(combat.targets[0].elemental, None);
        assert_eq!(combat.targets[0].health, 925.0);
        assert_eq!(combat.targets[1].health, 900.0);
        assert!(!combat.targets[1].elemental.unwrap().is_frozen());
    }

    /// Where the player stands in these tests: the middle of the test room.
    const PLAYER: Vec2 = Vec2::new(8.0, 4.5);

    fn run(combat: &mut Combat, defs: &Defs, room: &Room, steps: u32) {
        let mut rng = Rng::new(9);
        let mut player = Player::new(PLAYER);
        for step in 0..steps {
            combat.step(defs, room, &mut player, step as f64 * DT as f64, &mut rng);
        }
    }

    #[test]
    fn being_hurt_floats_a_number_that_rises_and_goes() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        let at = Vec2::new(10.0, 4.5);
        combat.targets.push(target(1, at, 1000.0));
        fire(&mut combat, &defs, "pistol", PLAYER, Vec2::X);
        run(&mut combat, &defs, &room, 30);
        assert_eq!(combat.damage_numbers.len(), 1);
        let number = combat.damage_numbers[0];
        assert_eq!((number.amount, number.crit), (100.0, false));
        let offset = number.position - at;
        assert!(offset.x.abs() <= 0.2 && offset.y.abs() <= 0.2 + 0.03, "scattered within 0.2: {offset}");

        // It climbs 0.001 a step and is gone 1.4 s after the hit.
        run(&mut combat, &defs, &room, 30);
        assert!((combat.damage_numbers[0].position.y - number.position.y - 0.03).abs() < 1e-4);
        run(&mut combat, &defs, &room, 50);
        assert!(combat.damage_numbers.is_empty());
    }

    #[test]
    fn an_explosion_shows_as_a_crit_and_a_hit_for_nothing_shows_no_number() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        combat.targets.push(target(1, Vec2::new(10.0, 4.5), 1000.0));
        fire(&mut combat, &defs, "bazooka", PLAYER, Vec2::X);
        run(&mut combat, &defs, &room, 30);
        // The missile's own 0 is not shown; the blast's 200 is, as a crit.
        assert_eq!(combat.damage_numbers.len(), 1);
        assert_eq!((combat.damage_numbers[0].amount, combat.damage_numbers[0].crit), (200.0, true));
    }

    #[test]
    fn a_crit_bullet_shows_its_doubled_damage() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        combat.targets.push(target(1, Vec2::new(9.0, 4.5), 1000.0));
        let def = defs.gun(defs.gun_named("pistol").unwrap());
        combat.fire(&defs, def, PLAYER, Vec2::X, Vec2::ZERO, 1.0, &mut Rng::new(3), &mut Vec::new());
        run(&mut combat, &defs, &room, 20);
        assert_eq!((combat.damage_numbers[0].amount, combat.damage_numbers[0].crit), (200.0, true));
    }

    #[test]
    fn a_missile_explodes_and_hurts_everything_in_its_radius() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        // The missile hits the first target. The blast (radius 0.4422) reaches the
        // second, whose near edge is 0.35 above, but not the third, a unit away.
        combat.targets.push(target(1, Vec2::new(10.0, 4.5), 1000.0));
        combat.targets.push(target(2, Vec2::new(10.0, 5.1), 1000.0));
        combat.targets.push(target(3, Vec2::new(11.2, 4.5), 1000.0));
        fire(&mut combat, &defs, "bazooka", PLAYER, Vec2::X);
        run(&mut combat, &defs, &room, 30);
        assert!(combat.bullets.is_empty());
        assert_eq!(combat.effects.len(), 1, "the explosion is what the missile leaves");
        // The bazooka's own damage is 0: all 200 is the explosion's.
        assert_eq!(combat.targets[0].health, 800.0);
        assert_eq!(combat.targets[1].health, 800.0);
        assert_eq!(combat.targets[2].health, 1000.0);

        // It hurts once, not on every step of its 0.3 s.
        run(&mut combat, &defs, &room, 30);
        assert!(combat.effects.is_empty());
        assert_eq!(combat.targets[1].health, 800.0);
    }

    #[test]
    fn a_missile_exploding_on_a_wall_hurts_a_target_beside_it() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        // Out of the missile's path, 0.3 from the right wall's face at x = 15.5.
        combat.targets.push(target(1, Vec2::new(15.1, 4.9), 250.0));
        fire(&mut combat, &defs, "bazooka", PLAYER, Vec2::X);
        run(&mut combat, &defs, &room, 90);
        assert_eq!(combat.targets[0].health, 50.0);

        // A plain splash is not an explosion: the pistol's hurts nothing nearby.
        fire(&mut combat, &defs, "pistol", PLAYER, Vec2::X);
        run(&mut combat, &defs, &room, 90);
        assert_eq!(combat.targets[0].health, 50.0);
    }

    #[test]
    fn the_blast_circle_touches_boxes_by_their_nearest_point() {
        let half = Vec2::new(0.1, 0.25);
        assert!(circle_overlaps_box(Vec2::ZERO, 0.44, Vec2::new(0.5, 0.0), half));
        assert!(!circle_overlaps_box(Vec2::ZERO, 0.44, Vec2::new(0.6, 0.0), half));
        // Diagonally the corner is 0.5 away although each axis alone is within reach.
        assert!(!circle_overlaps_box(Vec2::ZERO, 0.44, Vec2::new(0.4, 0.65), half));
        assert!(circle_overlaps_box(Vec2::ZERO, 0.44, Vec2::ZERO, half), "a centre inside the box");
    }

    #[test]
    fn a_homing_bullet_curves_into_a_target_off_its_line() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        combat.targets.push(target(1, Vec2::new(10.0, 6.5), 1000.0));
        // Fired level, two units below the target: a straight bullet would miss.
        fire(&mut combat, &defs, "seeker", PLAYER, Vec2::X);
        run(&mut combat, &defs, &room, 1);
        let bullet = &combat.bullets[0];
        assert!(bullet.velocity.y > 0.0, "pulled up towards the target");
        // It points at the target, 45 degrees up and right, not along its flight.
        let facing = -Vec2::from_angle(bullet.rotation);
        assert!((facing - Vec2::ONE.normalize()).length() < 0.05, "{facing}");

        run(&mut combat, &defs, &room, 119);
        assert!(combat.bullets.is_empty());
        assert_eq!(combat.targets[0].health, 900.0);
    }

    #[test]
    fn homing_bullets_chase_the_target_nearest_the_player() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        // The bullet starts right beside the far target, yet turns for the one
        // nearest the player, as in Unity.
        combat.targets.push(target(1, Vec2::new(14.0, 7.0), 1000.0));
        combat.targets.push(target(2, Vec2::new(7.0, 4.5), 1000.0));
        fire(&mut combat, &defs, "seeker", Vec2::new(13.0, 6.0), Vec2::Y);
        run(&mut combat, &defs, &room, 1);
        assert!(combat.bullets[0].velocity.x < 0.0);
    }

    #[test]
    fn homing_speed_matches_unitys_force_and_drag() {
        // 500 * 0.02 s, mass 1.
        assert_eq!(homing_acceleration(500.0), 10.0);
        // Drag 3 keeps 1 / 1.06 per 0.02 s Unity step: over 0.1 s, that to the fifth.
        assert!((drag_kept(3.0).powi(6) - (1.0f32 / 1.06).powi(5)).abs() < 1e-4);
        assert_eq!(drag_kept(0.0), 1.0);

        // With nothing to chase a homing bullet only slows: 10 units/s falls
        // below 1 within a second, where a plain bullet keeps its speed.
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        fire(&mut combat, &defs, "seeker", Vec2::new(2.0, 4.5), Vec2::X);
        run(&mut combat, &defs, &room, 60);
        let speed = combat.bullets[0].velocity.length();
        assert!((speed - 10.0 * (1.0f32 / 1.06).powi(50)).abs() < 0.01, "{speed}");

        // Chasing a far target it settles where push and drag balance. In Unity
        // that is (v + 0.2) / 1.06 = v, about 3.33 units/s.
        // A fresh bullet, flown almost to the end of its 2 s.
        let mut combat = Combat::new();
        combat.targets.push(target(1, Vec2::new(14.0, 4.5), 1000.0));
        fire(&mut combat, &defs, "seeker", Vec2::new(2.0, 4.5), Vec2::X);
        run(&mut combat, &defs, &room, 115);
        let speed = combat.bullets[0].velocity.length();
        assert!((speed - 3.33).abs() < 0.06, "{speed}");
    }

    #[test]
    fn a_bullet_flies_straight_and_splashes_on_the_wall() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        fire(&mut combat, &defs, "pistol", Vec2::new(8.0, 4.5), Vec2::X);
        assert_eq!(combat.bullets.len(), 1);
        assert_eq!(combat.bullets[0].velocity, Vec2::new(6.0, 0.0));

        // The right wall's inner face is at x = 15.5: 7.5 units at 6 units/s.
        run(&mut combat, &defs, &room, 70);
        assert_eq!(combat.bullets.len(), 1, "still in flight after 1.17 s");
        run(&mut combat, &defs, &room, 10);
        assert!(combat.bullets.is_empty());
        assert_eq!(combat.effects.len(), 1);
        assert!(combat.effects[0].position.x > 15.4);

        // The splash fades after its 0.2 s.
        run(&mut combat, &defs, &room, 13);
        assert!(combat.effects.is_empty());
    }

    #[test]
    fn a_bullet_damages_and_stops_at_a_target() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        combat.targets.push(target(1, Vec2::new(10.0, 4.5), 250.0));
        fire(&mut combat, &defs, "pistol", Vec2::new(8.0, 4.5), Vec2::X);
        run(&mut combat, &defs, &room, 30);
        assert!(combat.bullets.is_empty());
        assert_eq!(combat.targets[0].health, 150.0);
        assert!(combat.targets[0].last_hit > 0.0);
        assert_eq!(combat.effects.len(), 1, "it splashes on the target too");
    }

    #[test]
    fn a_target_out_of_health_is_removed() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        combat.targets.push(target(1, Vec2::new(9.0, 4.5), 100.0));
        fire(&mut combat, &defs, "pistol", Vec2::new(8.0, 4.5), Vec2::X);
        run(&mut combat, &defs, &room, 30);
        assert!(combat.targets.is_empty());
    }

    #[test]
    fn a_spray_fires_several_bullets_inside_its_cone_and_they_pierce() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut combat = Combat::new();
        // Two targets in a row: a piercing pellet can hurt both, each once.
        combat.targets.push(target(1, Vec2::new(8.4, 4.5), 10_000.0));
        combat.targets.push(target(2, Vec2::new(8.8, 4.5), 10_000.0));
        fire(&mut combat, &defs, "shotgun", Vec2::new(8.0, 4.5), Vec2::X);
        assert_eq!(combat.bullets.len(), 3);
        for bullet in &combat.bullets {
            let angle = bullet.velocity.to_angle().abs();
            assert!(angle <= 20f32.to_radians() + 1e-4, "{angle} is outside the 20 degree cone");
            let speed = bullet.velocity.length();
            assert!((5.0..=6.0 + 1e-4).contains(&speed), "{speed} is outside 5..6");
        }

        // Pellets live 0.25 s: 15 steps, about 1.3 units.
        run(&mut combat, &defs, &room, 20);
        assert!(combat.bullets.is_empty(), "pellets expire by themselves");
        let near = 10_000.0 - combat.targets[0].health;
        assert!(near > 0.0 && near % 150.0 == 0.0 && near <= 450.0, "{near}");
    }

    #[test]
    fn bullets_cross_ledges_but_not_walls() {
        assert!(boxes_overlap(Vec2::ZERO, Vec2::ONE, Vec2::new(1.5, 0.0), Vec2::ONE));
        assert!(!boxes_overlap(Vec2::ZERO, Vec2::ONE, Vec2::new(2.0, 0.0), Vec2::ONE), "touching is not overlapping");

        let room = Room::test_room();
        // (0, 0) is a wall cell; (1, 1) is floor.
        assert!(bullet_hits_wall(&room, Vec2::new(0.25, 0.25), Vec2::splat(0.05)));
        assert!(!bullet_hits_wall(&room, Vec2::new(0.75, 0.75), Vec2::splat(0.05)));
    }

    #[test]
    fn the_nearest_touching_pickup_is_the_one_in_reach() {
        let defs = test_defs();
        let mut combat = Combat::new();
        let gun = || Gun::new(defs.gun_named("pistol").unwrap());
        combat.pickups.push(Pickup { gun: gun(), position: Vec2::new(5.0, 5.0) });
        combat.pickups.push(Pickup { gun: gun(), position: Vec2::new(1.1, 1.0) });
        combat.pickups.push(Pickup { gun: gun(), position: Vec2::new(1.05, 1.0) });
        let half = Vec2::new(0.095, 0.24);
        assert_eq!(combat.interaction(&defs, Vec2::new(1.0, 1.0), half), Some(Interaction::Pickup(2)));
        assert_eq!(combat.interaction(&defs, Vec2::new(3.0, 3.0), half), None);

        // A teleport spot competes on the same terms: nearest wins.
        combat.teleports.push(Teleport {
            position: Vec2::new(1.0, 1.0),
            half: Vec2::splat(0.1),
            offset: Vec2::new(7.0, 0.0),
        });
        assert_eq!(combat.interaction(&defs, Vec2::new(1.0, 1.0), half), Some(Interaction::Teleport(0)));
    }

    #[test]
    fn firing_reports_the_guns_camera_shake() {
        let defs = test_defs();
        let mut combat = Combat::new();
        // The test guns have no shake set, so nothing is reported...
        assert!(fire(&mut combat, &defs, "pistol", Vec2::ZERO, Vec2::X).is_empty());
        // ...and one with a shake reports it.
        let mut def = defs.gun(defs.gun_named("pistol").unwrap()).clone();
        def.shake = crate::data::ShakeFile { time: 0.05, amplitude: 0.4, frequency: 1.0 };
        let mut events = Vec::new();
        combat.fire(&defs, &def, Vec2::ZERO, Vec2::X, Vec2::ZERO, 0.0, &mut Rng::new(1), &mut events);
        assert_eq!(events, [Event::Shake(def.shake)]);
    }
}
