//! Enemies and the shots they fire.
//!
//! An enemy is a [`Target`] with an [`Enemy`] attached: bullets, beams and blasts
//! find it like any target, and this module moves it and lets it fight. Each runs
//! one of Unity's AI scripts:
//!
//! - `Warrior` (the Warrior and the EnemyTank), below.
//! - `RangedAI` (the Crab): wanders, follows the player once it has seen them,
//!   and inside `attackRange` stands and fires a bullet at them every
//!   `fireInterval`. Its push is scaled by the frame time, so unlike the
//!   Warrior's its speed does not depend on Unity's frame rate.
//! - `RangedAITurret`: never moves by itself; while the player is within
//!   `attackRange` it fires from every muzzle at once every `fireInterval`, sight
//!   or no sight.
//! - `RangedAI_Laser` (the LASERCrab): follows like the Crab, drawing a green
//!   sight line to the player. Every `fireInterval` of following it attacks for
//!   `maxFireTime`: the first second the line turns yellow as a warning, then a
//!   red beam sweeps towards the player at `laserRotSpeed` and hurts them every
//!   `damageRate` it touches them. It stands still while attacking.
//!
//! - `DashAIDrone`: hovers about at random; once the player is within
//!   `followRange` it rises (Unity grows it over its first 11 frames) and drifts
//!   towards them. It flies over ledges. Touching the player makes it die on the
//!   spot (`ExplosionTrigger`), and like every explosive enemy it then explodes.
//!
//! Some also bite whoever touches them (`BasicAIAttack`). Their shots
//! (`EnemyBullet`) fly straight, hurt the player and stop on them or on a wall,
//! leaving a splash; they fly over ledges and through enemies and chests.
//!
//! The warrior wanders, picking a new direction every `wanderInterval`. Once the
//! player is within `followRange` and in plain sight it follows them for
//! `spottingLength` seconds, seeing them again renewing that. When the player
//! touches its reach it stops and swings: the blow lands 0.5 s in and hurts
//! during the next 0.4 s, and it swings at most every `damageCooldown`.

use crystal_engine::glam::Vec2;

use super::DT;
use super::collide::{move_box, move_flyer};
use super::combat::{Effect, Target, UNITY_STEP, boxes_overlap, bullet_hits_wall, drag_kept, ray_hits_box};
use super::defs::{
    Ai, Animation, BulletId, Defs, Direction, EnemyDef, EnemyId, OffsetBox, RangedDef, TouchDef, TurretDef, WarriorDef,
};
use crate::data::{DroneFile, LaserFile};
use super::player::{self, Player};
use super::rng::Rng;
use super::room::{CAMERA_BLOCK, Room};

/// `Warrior.Update` and `Wander` push with `moveSpeed` every rendered frame,
/// without scaling by the frame time, so Unity's speed depends on the frame
/// rate. As for drops, one Unity frame is taken to be 1/60 s. A Warrior then
/// tops out near 0.36 units/s, an EnemyTank near 0.5.
const UNITY_FRAME_RATE: f32 = 60.0;
/// `PlayerData.difficulty`: 1 on a new save. It scales how hard enemies push.
const DIFFICULTY: f32 = 1.0;
/// `Warrior.SwingHitCheck`: the blow lands this long after the swing starts...
const SWING_WINDUP: f64 = 0.5;
/// ...and hurts for this long (`hittingCooldown`), at most once.
const SWING_HITTING: f64 = 0.4;
/// Running animations change only above this speed (`Warrior.Update`).
const ANIMATION_SPEED: f32 = 0.2;
/// `RangedAI.Attack` sets its bullet's speed to this.
const RANGED_SHOT_SPEED: f32 = 3.0;
/// `RangedAITurret.Attack` sets each bullet's velocity to its muzzle's unit `right`.
const TURRET_SHOT_SPEED: f32 = 1.0;
/// Room for this many enemy shots before the oldest makes way.
pub const SHOT_CAPACITY: usize = 256;

/// How far a LASERCrab's sight line reaches (`RangedAI_Laser.PlayerInLineOfSight`).
const LASER_SIGHT_RANGE: f32 = 100.0;

/// What a LASERCrab's line shows, as its `LineRenderer` colours it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeamPhase {
    /// Following: a sight line to what it sees, green.
    Sight,
    /// The attack's first second: yellow, still where the sight line last was.
    Warning,
    /// Firing: red, and it hurts.
    Firing,
}

/// A LASERCrab's line this step, from `origin` to `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnemyBeam {
    pub origin: Vec2,
    pub end: Vec2,
    pub phase: BeamPhase,
}

/// The LASERCrab's timers (`RangedAI_Laser`), all counting down in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LaserState {
    /// Seconds of the current attack left; 0 when not attacking.
    fire_time: f32,
    /// Seconds of following before the next attack.
    next_fire_time: f32,
    /// Seconds before the beam can hurt again.
    next_damage_tick: f32,
    /// Where the beam points, radians.
    angle: f32,
    /// Where the player was last seen from it, radians (`quatRot`).
    sight_angle: f32,
    /// Where the sight line last ended (`hit.point`).
    sight_end: Vec2,
}

/// A bullet an enemy fired (`EnemyBullet`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub def: BulletId,
    pub position: Vec2,
    pub previous_position: Vec2,
    pub velocity: Vec2,
    /// Which way the sprite points, counter-clockwise radians.
    pub rotation: f32,
    pub time_left: f32,
    pub damage: f32,
}

/// An enemy's state beside its [`Target`].
#[derive(Clone, Debug, PartialEq)]
pub struct Enemy {
    pub def: EnemyId,
    /// The rigidbody's, which knockback impulses are divided by.
    pub mass: f32,
    pub velocity: Vec2,
    /// Where it stood one step ago, for the view to blend from.
    pub previous_origin: Vec2,
    /// Seconds it keeps following after last seeing the player.
    spotting_timer: f32,
    /// Where it is heading: towards the player, or a wander direction.
    heading: Vec2,
    last_wander: f64,
    last_swing: f64,
    hitting_until: f64,
    last_fire: f64,
    last_touch: f64,
    laser: LaserState,
    /// How many frames a Drone has risen so far (`DashAIDrone.firstSight`).
    pub risen: u8,
    /// A Drone's smoothed wander direction (`targetGrav`).
    drift: Vec2,
    /// A LASERCrab's line, while it shows.
    pub beam: Option<EnemyBeam>,
    /// The swing in progress: its direction and when it started.
    pub swing: Option<(Direction, f64)>,
    /// What its Animator plays and since when. `None` until it first moves.
    pub animation: Option<(Animation, f64)>,
}

impl Enemy {
    pub fn new(def: EnemyId, mass: f32, origin: Vec2) -> Self {
        Self {
            def,
            mass,
            velocity: Vec2::ZERO,
            previous_origin: origin,
            spotting_timer: 0.0,
            heading: Vec2::ZERO,
            // Unity compares these with `Time.time`, which counts from the game's
            // start, long before a room's enemies meet the player: they never hold
            // the first wander or swing back. So they start in the distant past.
            last_wander: f64::NEG_INFINITY,
            last_swing: f64::NEG_INFINITY,
            hitting_until: f64::NEG_INFINITY,
            last_fire: f64::NEG_INFINITY,
            last_touch: f64::NEG_INFINITY,
            laser: LaserState {
                fire_time: 0.0,
                next_fire_time: 0.0,
                next_damage_tick: 0.0,
                angle: 0.0,
                sight_angle: 0.0,
                sight_end: origin,
            },
            beam: None,
            risen: 0,
            drift: Vec2::ZERO,
            swing: None,
            animation: None,
        }
    }

    /// Unity's Animator keeps playing a controller that is set again; only a
    /// different one starts from its first frame.
    fn play(&mut self, animation: Animation, now: f64) {
        if self.animation.map(|(playing, _)| playing) != Some(animation) {
            self.animation = Some((animation, now));
        }
    }
}

/// A target that is an enemy, standing at `origin`.
pub fn spawn(defs: &Defs, enemy: EnemyId, origin: Vec2, id: u32) -> Target {
    let def = defs.enemy(enemy);
    let mut state = Enemy::new(enemy, def.mass, origin);
    // An Animator given a controller in the prefab plays it from the start.
    if def.animations[Animation::Idle.index()].is_some() {
        state.animation = Some((Animation::Idle, 0.0));
    }
    if let Ai::Laser(laser) = &def.ai {
        // `Start`: the first attack waits `maxFireTime`, not `fireInterval`.
        state.laser.next_fire_time = laser.max_fire_time;
    }
    Target {
        id,
        position: origin + Vec2::from(def.offset),
        half: Vec2::from(def.half),
        origin,
        chest: None,
        enemy: Some(state),
        health: def.max_health,
        max_health: def.max_health,
        last_hit: f64::NEG_INFINITY,
        elemental: def.freezable.then(Default::default),
    }
}

/// One step of every enemy among `targets`: think, fight, move. Shots fired go
/// into `shots`.
#[allow(clippy::too_many_arguments)]
pub fn step_enemies(
    targets: &mut [Target],
    shots: &mut Vec<Shot>,
    defs: &Defs,
    room: &Room,
    player: &mut Player,
    now: f64,
    rng: &mut Rng,
) {
    for index in 0..targets.len() {
        let Some(enemy) = &targets[index].enemy else {
            continue;
        };
        let def = defs.enemy(enemy.def);
        let push = match &def.ai {
            Ai::Warrior(warrior) => {
                let heading = think(targets, index, warrior, room, player.position, now, rng);
                swing(&mut targets[index], warrior, player, now);
                heading * warrior_acceleration(warrior, def)
            }
            Ai::Ranged(ranged) => {
                let heading = think_ranged(targets, index, ranged, defs, room, player.position, now, rng, shots);
                heading * ranged_acceleration(ranged, def)
            }
            Ai::Turret(turret) => {
                fire_turret(&mut targets[index], turret, defs, player.position, now, shots);
                Vec2::ZERO
            }
            Ai::Laser(laser) => {
                let heading = think_laser(targets, index, laser, room, player, now, rng);
                heading * laser.move_speed * UNITY_STEP / def.mass
            }
            Ai::Drone(drone) => think_drone(&mut targets[index], drone, def, player.position, now, rng),
        };
        if let Some(area) = def.detonate {
            let target = &mut targets[index];
            if touches(target.origin, grown(area, target, def), player.position) {
                // `ExplosionTrigger` calls `Killable.Die` at once.
                target.health = 0.0;
            }
        }
        if let Some(touch) = def.touch {
            bite(&mut targets[index], touch, player, now);
        }
        move_enemy(targets, index, def, room, push);
    }
}

/// Moves enemy shots and resolves what they hit (`EnemyBullet`): the player is
/// hurt (if not just hurt) and the shot ends there; a wall ends it too. Either
/// way it leaves its splash in its own colour.
pub fn step_shots(
    shots: &mut Vec<Shot>,
    effects: &mut Vec<Effect>,
    defs: &Defs,
    room: &Room,
    player: &mut Player,
    now: f64,
) {
    shots.retain_mut(|shot| {
        let def = defs.bullet(shot.def);
        shot.previous_position = shot.position;
        shot.position += shot.velocity * DT;
        shot.time_left -= DT;
        let half = Vec2::from(def.half);
        let hit_player = boxes_overlap(shot.position, half, player.position, player::HALF_SIZE);
        if hit_player {
            player.hurt(shot.damage, now);
        }
        if hit_player || bullet_hits_wall(room, shot.position, half) {
            if let Some(effect) = def.effect {
                if effects.len() == effects.capacity() {
                    effects.remove(0);
                }
                effects.push(Effect {
                    def: effect,
                    position: shot.position,
                    rotation: shot.rotation,
                    color: def.color,
                    time_left: defs.effect(effect).time_to_live,
                });
            }
            return false;
        }
        shot.time_left > 0.0
    });
}

fn fire(shots: &mut Vec<Shot>, defs: &Defs, bullet: BulletId, position: Vec2, velocity: Vec2, rotation: f32, damage: f32) {
    if shots.len() == shots.capacity() {
        shots.remove(0);
    }
    shots.push(Shot {
        def: bullet,
        position,
        previous_position: position,
        velocity,
        rotation,
        time_left: defs.bullet(bullet).time_to_live,
        damage,
    });
}

/// `RangedAI.Update`: the direction it pushes in this step, if any, firing as it
/// decides to. It acts only while the player is in its screen-sized block.
#[allow(clippy::too_many_arguments)]
fn think_ranged(
    targets: &mut [Target],
    index: usize,
    ai: &RangedDef,
    defs: &Defs,
    room: &Room,
    player: Vec2,
    now: f64,
    rng: &mut Rng,
    shots: &mut Vec<Shot>,
) -> Vec2 {
    let origin = targets[index].origin;
    if block_of(origin) != block_of(player) {
        return Vec2::ZERO;
    }
    let distance = origin.distance(player);
    let (in_attack, in_follow) = (distance < ai.attack_range, distance < ai.follow_range);
    let spotting = targets[index].enemy.as_ref().expect("an enemy").spotting_timer;
    let attacking = in_attack && spotting > 0.0;
    let following = !attacking && in_follow && spotting > 0.0;
    // Wandering looks only when the player is near; the other two always look.
    let seen = (attacking || following || in_attack || in_follow) && in_sight(targets, index, room, player);

    let enemy = targets[index].enemy.as_mut().expect("an enemy");
    let push = if attacking {
        if now - enemy.last_fire > (ai.fire_interval / DIFFICULTY) as f64 {
            // `Attack`: straight at the player, the sprite turned to face back.
            let direction = (player - origin).normalize_or_zero();
            fire(shots, defs, ai.bullet, origin, direction * RANGED_SHOT_SPEED, (-direction).to_angle(), ai.damage);
            enemy.last_fire = now;
        }
        Vec2::ZERO
    } else if following {
        enemy.heading = player - origin;
        enemy.heading.normalize_or_zero()
    } else {
        if now - enemy.last_wander > ai.wander_interval as f64 {
            let mut pick = || (rng.unit() * 3.0).floor().min(2.0) - 1.0;
            enemy.heading = Vec2::new(pick(), pick());
            enemy.last_wander = now;
        }
        enemy.heading.normalize_or_zero()
    };
    if seen {
        enemy.spotting_timer = ai.spotting_length;
    }
    if attacking || following {
        enemy.spotting_timer -= DT;
    }
    push
}

/// What a ray from an enemy meets first: the player, a wall, or another target
/// (Unity's mask holds the `Default` and `Player` layers: walls, bodies, chests).
#[derive(Clone, Copy, Debug, PartialEq)]
enum RayHit {
    Player(f32),
    Blocked(f32),
    Nothing,
}

fn cast(targets: &[Target], index: usize, room: &Room, player: Vec2, direction: Vec2, range: f32) -> RayHit {
    let origin = targets[index].origin;
    let mut nearest = room.ray_hits_wall(origin, direction, range).map(RayHit::Blocked);
    let distance = |hit: Option<RayHit>| match hit {
        Some(RayHit::Player(distance) | RayHit::Blocked(distance)) => distance,
        _ => range,
    };
    for (other, target) in targets.iter().enumerate() {
        if other != index
            && let Some(hit) = ray_hits_box(origin, direction, target.position, target.half).filter(|&hit| hit < distance(nearest))
        {
            nearest = Some(RayHit::Blocked(hit));
        }
    }
    if let Some(hit) = ray_hits_box(origin, direction, player, player::HALF_SIZE).filter(|&hit| hit < distance(nearest)) {
        nearest = Some(RayHit::Player(hit));
    }
    nearest.unwrap_or(RayHit::Nothing)
}

/// `RangedAI_Laser.Update`, one step: the direction it pushes in, if any. Its
/// line is left in `enemy.beam` for the view.
fn think_laser(
    targets: &mut [Target],
    index: usize,
    ai: &LaserFile,
    room: &Room,
    player: &mut Player,
    now: f64,
    rng: &mut Rng,
) -> Vec2 {
    let origin = targets[index].origin;
    if block_of(origin) != block_of(player.position) {
        return Vec2::ZERO;
    }
    let distance = origin.distance(player.position);
    let (in_attack, in_follow) = (distance < ai.attack_range, distance < ai.follow_range);

    // `PlayerInLineOfSight`, which also remembers where it looked and what it hit.
    let look = |targets: &mut [Target]| {
        let direction = (player.position - origin).normalize_or_zero();
        let hit = cast(targets, index, room, player.position, direction, LASER_SIGHT_RANGE);
        let state = targets[index].enemy.as_mut().expect("an enemy");
        state.laser.sight_angle = direction.to_angle();
        if let RayHit::Player(distance) | RayHit::Blocked(distance) = hit {
            state.laser.sight_end = origin + direction * distance;
        }
        if matches!(hit, RayHit::Player(_)) {
            state.spotting_timer = ai.spotting_length;
        }
    };

    let enemy = targets[index].enemy.as_mut().expect("an enemy");
    if enemy.laser.next_damage_tick > 0.0 {
        enemy.laser.next_damage_tick -= DT;
    }
    let mut push = Vec2::ZERO;
    if enemy.spotting_timer <= 0.0 {
        enemy.laser.fire_time = 0.0;
        enemy.laser.next_fire_time = ai.fire_interval;
        enemy.beam = None;
        if now - enemy.last_wander > ai.wander_interval as f64 {
            let mut pick = || (rng.unit() * 3.0).floor().min(2.0) - 1.0;
            enemy.heading = Vec2::new(pick(), pick());
            enemy.last_wander = now;
        }
        push = enemy.heading.normalize_or_zero();
        enemy.laser.angle = enemy.laser.sight_angle;
        if in_attack || in_follow {
            look(targets);
        }
    } else if in_attack && enemy.laser.fire_time > 0.0 {
        look(targets);
        let enemy = targets[index].enemy.as_mut().expect("an enemy");
        if enemy.laser.fire_time <= ai.max_fire_time - 1.0 {
            fire_laser(targets, index, ai, room, player, now);
        } else if let Some(beam) = &mut enemy.beam {
            // The warning recolours the line where the sight line last left it.
            beam.phase = BeamPhase::Warning;
        }
        let enemy = targets[index].enemy.as_mut().expect("an enemy");
        enemy.laser.fire_time -= DT;
        enemy.laser.next_fire_time = ai.fire_interval;
        enemy.spotting_timer -= DT;
    } else if in_follow {
        look(targets);
        let enemy = targets[index].enemy.as_mut().expect("an enemy");
        enemy.beam = Some(EnemyBeam { origin, end: enemy.laser.sight_end, phase: BeamPhase::Sight });
        enemy.heading = player.position - origin;
        push = enemy.heading.normalize_or_zero();
        enemy.spotting_timer -= DT;
        enemy.laser.angle = enemy.laser.sight_angle;
        enemy.laser.fire_time = 0.0;
    }
    let enemy = targets[index].enemy.as_mut().expect("an enemy");
    if enemy.laser.next_fire_time > 0.0 {
        enemy.laser.next_fire_time -= DT;
    } else {
        enemy.laser.fire_time = ai.max_fire_time;
    }
    push
}

/// `RangedAI_Laser.Attack`: turn the beam towards the player at the turn speed,
/// shine it up to the attack range, and hurt the player if it reaches them first.
fn fire_laser(targets: &mut [Target], index: usize, ai: &LaserFile, room: &Room, player: &mut Player, now: f64) {
    let origin = targets[index].origin;
    let enemy = targets[index].enemy.as_ref().expect("an enemy");
    let wanted = (player.position - origin).to_angle();
    let step = ai.turn_speed.to_radians() * DT;
    let turn = wrap_angle(wanted - enemy.laser.angle).clamp(-step, step);
    let angle = enemy.laser.angle + turn;
    let direction = Vec2::from_angle(angle);
    let hit = cast(targets, index, room, player.position, direction, ai.attack_range);
    let length = match hit {
        RayHit::Player(distance) | RayHit::Blocked(distance) => distance,
        RayHit::Nothing => ai.attack_range,
    };
    let enemy = targets[index].enemy.as_mut().expect("an enemy");
    enemy.laser.angle = angle;
    enemy.beam = Some(EnemyBeam { origin, end: origin + direction * length, phase: BeamPhase::Firing });
    if matches!(hit, RayHit::Player(_)) && enemy.laser.next_damage_tick <= 0.0 {
        player.hurt(ai.damage, now);
        enemy.laser.next_damage_tick = ai.damage_rate;
    }
}

/// How many frames a Drone grows for once it first sees the player, and by how
/// much each frame (`DashAIDrone.Update`: `firstSight <= 10`, `+= 0.05`).
const DRONE_RISE_FRAMES: u8 = 11;
const DRONE_RISE_STEP: f32 = 0.05;
/// `DashAIDrone.Follow` pushes with this times difficulty, every frame.
const DRONE_FOLLOW_FORCE: f32 = 7.0;
/// `DashAIDrone.Wander` eases its direction towards the new one at this many
/// units per second.
const DRONE_DRIFT_RATE: f32 = 4.0;

/// `DashAIDrone.Update`: the acceleration it pushes with. Wandering sets its
/// velocity outright, so that is done here and no push is returned.
fn think_drone(target: &mut Target, ai: &DroneFile, def: &EnemyDef, player: Vec2, now: f64, rng: &mut Rng) -> Vec2 {
    let origin = target.origin;
    if block_of(origin) != block_of(player) {
        return Vec2::ZERO;
    }
    let enemy = target.enemy.as_mut().expect("an enemy");
    if origin.distance(player) <= ai.follow_range {
        if enemy.risen < DRONE_RISE_FRAMES {
            enemy.risen += 1;
        }
        let toward = (player - origin).normalize_or_zero();
        // A push every frame, not scaled by frame time, taken at 60 frames a second.
        return toward * DRONE_FOLLOW_FORCE * DIFFICULTY * UNITY_FRAME_RATE * UNITY_STEP / def.mass;
    }
    // A wander interval of 0 picks a new direction every frame.
    if now - enemy.last_wander > ai.wander_interval as f64 {
        let mut pick = || (rng.unit() * 3.0).floor().min(2.0) - 1.0;
        enemy.heading = Vec2::new(pick(), pick());
        enemy.last_wander = now;
    }
    let wanted = enemy.heading.normalize_or_zero();
    let gap = wanted - enemy.drift;
    let reach = DRONE_DRIFT_RATE * DT;
    enemy.drift = if gap.length() <= reach { wanted } else { enemy.drift + gap.normalize() * reach };
    enemy.velocity = enemy.drift * ai.move_speed;
    Vec2::ZERO
}

/// How much bigger than its prefab a risen Drone is: its scale went from the
/// prefab's to that plus 0.05 a frame, and its colliders grew with it.
fn growth(enemy: &Enemy, def: &EnemyDef) -> f32 {
    (def.scale[0] + DRONE_RISE_STEP * enemy.risen as f32) / def.scale[0]
}

/// A box of the enemy's, grown as the enemy has grown.
fn grown(area: OffsetBox, target: &Target, def: &EnemyDef) -> OffsetBox {
    let factor = target.enemy.as_ref().map_or(1.0, |enemy| growth(enemy, def));
    OffsetBox {
        half: [area.half[0] * factor, area.half[1] * factor],
        offset: [area.offset[0] * factor, area.offset[1] * factor],
    }
}

/// The scale an enemy is drawn at: its prefab's, plus what a Drone has risen.
pub fn drawn_scale(enemy: &Enemy, def: &EnemyDef) -> [f32; 2] {
    let factor = growth(enemy, def);
    [def.scale[0] * factor, def.scale[1] * factor]
}

/// An angle brought into -pi..=pi, for turning the short way round.
fn wrap_angle(angle: f32) -> f32 {
    let turn = std::f32::consts::TAU;
    angle - turn * ((angle + std::f32::consts::PI) / turn).floor()
}

/// `RangedAITurret.Update`: every muzzle fires at once while the player is near.
fn fire_turret(target: &mut Target, ai: &TurretDef, defs: &Defs, player: Vec2, now: f64, shots: &mut Vec<Shot>) {
    let origin = target.origin;
    let enemy = target.enemy.as_mut().expect("an enemy");
    if block_of(origin) != block_of(player)
        || origin.distance(player) >= ai.attack_range
        || now - enemy.last_fire <= (ai.fire_interval / DIFFICULTY) as f64
    {
        return;
    }
    for muzzle in &ai.muzzles {
        let angle = muzzle.angle.to_radians();
        // The bullet keeps its muzzle's rotation, so its sprite is not turned round
        // as the Crab's is.
        let velocity = Vec2::from_angle(angle) * TURRET_SHOT_SPEED;
        fire(shots, defs, ai.bullet, origin + Vec2::from(muzzle.offset), velocity, angle, ai.damage);
    }
    enemy.last_fire = now;
}

/// `BasicAIAttack.OnTriggerStay2D`: touching the box hurts, at most every
/// cooldown. The cooldown restarts even when the player was still invulnerable.
fn bite(target: &mut Target, touch: TouchDef, player: &mut Player, now: f64) {
    let origin = target.origin;
    let enemy = target.enemy.as_mut().expect("an enemy");
    if touches(origin, touch.reach, player.position) && now - enemy.last_touch > touch.damage_cooldown as f64 {
        player.hurt(touch.damage, now);
        enemy.last_touch = now;
    }
}

/// `Warrior.Update`: the direction it pushes in this step, if any. It acts only
/// while the player is in the same screen-sized block of the world.
#[allow(clippy::too_many_arguments)]
fn think(targets: &mut [Target], index: usize, warrior: &WarriorDef, room: &Room, player: Vec2, now: f64, rng: &mut Rng) -> Vec2 {
    let origin = targets[index].origin;
    let in_sight = |targets: &[Target]| in_sight(targets, index, room, player);
    let enemy = targets[index].enemy.as_ref().expect("an enemy");
    if block_of(origin) != block_of(player) || enemy.swing.is_some() {
        return Vec2::ZERO;
    }
    let in_range = origin.distance(player) < warrior.follow_range;
    let following = in_range && enemy.spotting_timer > 0.0;
    let seen = in_range && in_sight(targets);

    let enemy = targets[index].enemy.as_mut().expect("an enemy");
    if following {
        enemy.heading = player - origin;
        enemy.spotting_timer -= DT;
    } else if now - enemy.last_wander > warrior.wander_interval as f64 {
        // `Random.Range(0, 3) - 1` on each axis: a step left, none or right.
        let mut pick = || (rng.unit() * 3.0).floor().min(2.0) - 1.0;
        enemy.heading = Vec2::new(pick(), pick());
        enemy.last_wander = now;
    }
    if seen {
        enemy.spotting_timer = warrior.spotting_length;
    }

    if enemy.velocity.length() > ANIMATION_SPEED
        && let Some(direction) = within_45(enemy.velocity)
    {
        enemy.play(Animation::Run(direction), now);
    }
    enemy.heading.normalize_or_zero()
}

/// `Warrior.PlayerInLineOfSight`: a ray from the enemy towards the player meets
/// the player before any wall or other solid body. Ledges, triggers and the enemy
/// itself do not stop it, as in Unity's ray settings.
fn in_sight(targets: &[Target], index: usize, room: &Room, player: Vec2) -> bool {
    let origin = targets[index].origin;
    let Some(direction) = (player - origin).try_normalize() else {
        return true;
    };
    let Some(to_player) = ray_hits_box(origin, direction, player, player::HALF_SIZE) else {
        return false;
    };
    if room.ray_hits_wall(origin, direction, to_player).is_some() {
        return false;
    }
    !targets.iter().enumerate().any(|(other, target)| {
        other != index && ray_hits_box(origin, direction, target.position, target.half).is_some_and(|d| d < to_player)
    })
}

/// `Warrior.AttackColliderRelay`. Touching the reach starts a swing, at most
/// every `damageCooldown`, aimed where the enemy was heading. The swing's box is
/// live from 0.5 s to 0.9 s and hurts the player at most once.
///
/// Unity picks the swing's direction only when the heading is less than 45°
/// from an axis; a heading exactly diagonal or zero (both common while
/// wandering) picks none, and the warrior then stands frozen for good. Here such
/// a swing is aimed at the player instead.
fn swing(target: &mut Target, warrior: &WarriorDef, player: &mut Player, now: f64) {
    let origin = target.origin;
    let enemy = target.enemy.as_mut().expect("an enemy");

    if let Some((direction, started)) = enemy.swing {
        let elapsed = now - started;
        if elapsed >= SWING_WINDUP + SWING_HITTING {
            enemy.swing = None;
        } else if elapsed >= SWING_WINDUP && now > enemy.hitting_until {
            let blow = warrior.swings[direction as usize];
            if touches(origin, blow, player.position) {
                player.hurt(warrior.damage, now);
                enemy.hitting_until = now + SWING_HITTING;
            }
        }
        return;
    }

    if touches(origin, warrior.reach, player.position) && now - enemy.last_swing > warrior.damage_cooldown as f64 {
        let direction = within_45(enemy.heading)
            .or_else(|| nearest_axis(player.position - origin))
            .unwrap_or(Direction::Down);
        enemy.play(Animation::Attack(direction), now);
        enemy.swing = Some((direction, now));
        enemy.velocity = Vec2::ZERO;
        enemy.last_swing = now;
    }
}

/// Pushes the enemy with acceleration `push`, applies drag, and moves it through
/// the room. Walls and ledges stop it, and so do chests.
fn move_enemy(targets: &mut [Target], index: usize, def: &EnemyDef, room: &Room, push: Vec2) {
    let (before, after) = targets.split_at_mut(index);
    let (target, after) = after.split_first_mut().expect("index is in range");
    let solids = before
        .iter()
        .chain(after.iter())
        .filter(|other| other.chest.is_some())
        .map(|other| (other.position, other.half));
    let added_drag = target.elemental.as_ref().map_or(0.0, |elemental| elemental.added_drag());
    let body = grown(OffsetBox { half: def.half, offset: def.offset }, target, def);
    let enemy = target.enemy.as_mut().expect("an enemy");

    enemy.previous_origin = target.origin;
    enemy.velocity += push * DT;
    enemy.velocity *= drag_kept(def.drag + added_drag);
    let half = Vec2::from(body.half);
    let center = target.origin + Vec2::from(body.offset);
    let delta = enemy.velocity * DT;
    let moved = if def.flying {
        move_flyer(room, solids, center, half, delta)
    } else {
        move_box(room, solids, center, half, delta)
    };
    if moved.hit_x {
        enemy.velocity.x = 0.0;
    }
    if moved.hit_y {
        enemy.velocity.y = 0.0;
    }
    target.position = moved.position;
    target.half = half;
    target.origin = moved.position - Vec2::from(body.offset);
}

/// Units/s^2 from the Warrior's per-frame push of `moveSpeed * difficulty` on
/// its mass, summed over each 0.02 s physics step.
fn warrior_acceleration(warrior: &WarriorDef, def: &EnemyDef) -> f32 {
    warrior.move_speed * DIFFICULTY * UNITY_FRAME_RATE * UNITY_STEP / def.mass
}

/// Units/s^2 from `RangedAI`'s push of `moveSpeed * deltaTime` each frame: summed
/// over a 0.02 s physics step that is `moveSpeed * 0.02`, whatever the frame
/// rate. The Crab's 1000 against its drag of 15 tops out near 0.93 units/s.
fn ranged_acceleration(ranged: &RangedDef, def: &EnemyDef) -> f32 {
    ranged.move_speed * UNITY_STEP / def.mass
}

/// Whether the player's box overlaps a box placed relative to `origin`.
fn touches(origin: Vec2, area: OffsetBox, player: Vec2) -> bool {
    boxes_overlap(origin + Vec2::from(area.offset), Vec2::from(area.half), player, player::HALF_SIZE)
}

/// The axis `vector` is strictly within 45° of, as `Vector2.Angle(...) < 45`.
fn within_45(vector: Vec2) -> Option<Direction> {
    let (x, y) = (vector.x, vector.y);
    if x > y.abs() {
        Some(Direction::Right)
    } else if -x > y.abs() {
        Some(Direction::Left)
    } else if y > x.abs() {
        Some(Direction::Up)
    } else if -y > x.abs() {
        Some(Direction::Down)
    } else {
        None
    }
}

/// The axis nearest `vector`, ties going to the horizontal.
fn nearest_axis(vector: Vec2) -> Option<Direction> {
    if vector == Vec2::ZERO {
        return None;
    }
    Some(if vector.x.abs() >= vector.y.abs() {
        if vector.x > 0.0 { Direction::Right } else { Direction::Left }
    } else if vector.y > 0.0 {
        Direction::Up
    } else {
        Direction::Down
    })
}

/// Which screen-sized block of the world a point is in (`RoomInfo.width/height`).
fn block_of(point: Vec2) -> (i32, i32) {
    ((point.x / CAMERA_BLOCK.x).floor() as i32, (point.y / CAMERA_BLOCK.y).floor() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::defs::tests::test_defs;

    fn brute_at(defs: &Defs, origin: Vec2) -> Target {
        spawn(defs, defs.enemy_named("brute").unwrap(), origin, 0)
    }

    fn run(targets: &mut [Target], defs: &Defs, room: &Room, player: &mut Player, steps: u32) -> Vec<Shot> {
        let mut rng = Rng::new(5);
        let mut shots = Vec::with_capacity(SHOT_CAPACITY);
        let mut effects = Vec::with_capacity(8);
        for tick in 0..steps {
            let now = tick as f64 * DT as f64;
            step_enemies(targets, &mut shots, defs, room, player, now, &mut rng);
            step_shots(&mut shots, &mut effects, defs, room, player, now);
        }
        shots
    }

    #[test]
    fn a_crab_closes_in_then_stands_and_shoots() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(4.0, 3.0));
        let mut targets = vec![spawn(&defs, defs.enemy_named("shooter").unwrap(), Vec2::new(10.0, 3.0), 0)];
        run(&mut targets, &defs, &room, &mut player, 60 * 10);
        let distance = targets[0].origin.distance(player.position);
        assert!(distance < 2.6, "it came within its attack range: {distance}");
        assert!(player.health < player::MAX_HEALTH, "and its shots landed: {}", player.health);
        assert!(matches!(targets[0].enemy.as_ref().unwrap().animation, Some((Animation::Idle, _))));
    }

    #[test]
    fn a_crab_that_never_sees_the_player_only_wanders() {
        let defs = test_defs();
        let room = Room::test_room();
        // Out of its block: it does nothing at all.
        let mut player = Player::new(Vec2::new(20.0, 3.0));
        let mut targets = vec![spawn(&defs, defs.enemy_named("shooter").unwrap(), Vec2::new(10.0, 3.0), 0)];
        let shots = run(&mut targets, &defs, &room, &mut player, 120);
        assert!(shots.is_empty());
        assert_eq!(targets[0].origin, Vec2::new(10.0, 3.0));
    }

    #[test]
    fn a_turret_fires_every_muzzle_only_while_the_player_is_near() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut far = Player::new(Vec2::new(8.0, 3.0));
        let mut targets = vec![spawn(&defs, defs.enemy_named("turret").unwrap(), Vec2::new(4.0, 3.0), 0)];
        assert!(run(&mut targets, &defs, &room, &mut far, 120).is_empty(), "4 units is beyond its 3");

        let mut near = Player::new(Vec2::new(4.0, 5.0));
        let mut targets = vec![spawn(&defs, defs.enemy_named("turret").unwrap(), Vec2::new(4.0, 3.0), 0)];
        let shots = run(&mut targets, &defs, &room, &mut near, 1);
        assert_eq!(shots.len(), 2, "one per muzzle");
        assert_eq!(shots[0].velocity, Vec2::new(1.0, 0.0));
        assert!((shots[1].velocity - Vec2::new(0.0, 1.0)).length() < 1e-6);
        // At 1 unit per second the upward shot reaches a player 1 unit above.
        let mut close = Player::new(Vec2::new(4.0, 4.0));
        let mut targets = vec![spawn(&defs, defs.enemy_named("turret").unwrap(), Vec2::new(4.0, 3.0), 0)];
        run(&mut targets, &defs, &room, &mut close, 60);
        assert_eq!(close.health, player::MAX_HEALTH - 50.0);
    }

    #[test]
    fn a_laser_crab_warns_then_burns_the_player_it_sees() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(4.0, 2.0));
        let mut targets = vec![spawn(&defs, defs.enemy_named("laser_crab").unwrap(), Vec2::new(6.5, 2.0), 0)];
        let mut rng = Rng::new(5);
        let mut shots = Vec::new();
        let mut phases = Vec::new();
        for tick in 0..60 * 8 {
            let now = tick as f64 * DT as f64;
            // Keep the player in place, out of its bite.
            player.position = Vec2::new(4.0, 2.0);
            step_enemies(&mut targets, &mut shots, &defs, &room, &mut player, now, &mut rng);
            if let Some(beam) = targets[0].enemy.as_ref().unwrap().beam
                && phases.last() != Some(&beam.phase)
            {
                phases.push(beam.phase);
            }
        }
        assert_eq!(&phases[..3], &[BeamPhase::Sight, BeamPhase::Warning, BeamPhase::Firing], "{phases:?}");
        assert!(player.health < player::MAX_HEALTH, "the beam burned: {}", player.health);
    }

    #[test]
    fn a_drone_rises_drifts_in_and_blows_up_on_the_player() {
        let defs = test_defs();
        let room = Room::test_room();
        let drone = defs.enemy_named("drone").unwrap();
        let mut player = Player::new(Vec2::new(4.0, 2.0));
        let mut targets = vec![spawn(&defs, drone, Vec2::new(4.6, 2.0), 0)];
        let mut rng = Rng::new(5);
        let mut shots = Vec::new();
        step_enemies(&mut targets, &mut shots, &defs, &room, &mut player, 0.0, &mut rng);
        assert_eq!(targets[0].enemy.as_ref().unwrap().risen, 1, "it saw the player and began to rise");
        let mut tick = 1;
        while targets[0].health > 0.0 && tick < 60 * 30 {
            step_enemies(&mut targets, &mut shots, &defs, &room, &mut player, tick as f64 * DT as f64, &mut rng);
            tick += 1;
        }
        assert_eq!(targets[0].health, 0.0, "touching the player set it off");
        let enemy = targets[0].enemy.as_ref().unwrap();
        assert_eq!(enemy.risen, DRONE_RISE_FRAMES);
        assert!((drawn_scale(enemy, defs.enemy(drone))[0] - 1.05).abs() < 1e-5, "grown from 0.5 to 1.05");
    }

    #[test]
    fn a_drone_flies_over_ledges() {
        let defs = test_defs();
        assert!(defs.enemy(defs.enemy_named("drone").unwrap()).flying);
        assert!(!defs.enemy(defs.enemy_named("brute").unwrap()).flying);
    }

    #[test]
    fn angles_wrap_the_short_way_round() {
        assert!((wrap_angle(3.5 * std::f32::consts::PI) - (-0.5 * std::f32::consts::PI)).abs() < 1e-5);
        assert!((wrap_angle(0.25) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_bite_hurts_on_touch_at_most_every_cooldown() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(8.0, 3.0));
        let mut targets = vec![spawn(&defs, defs.enemy_named("shooter").unwrap(), Vec2::new(8.1, 3.0), 0)];
        // Shots and bites together, over 1.4 s: the bite lands at once, and the
        // player's 0.5 s of invulnerability spaces out the rest.
        run(&mut targets, &defs, &room, &mut player, 1);
        assert_eq!(player.health, player::MAX_HEALTH - 100.0);
    }

    #[test]
    fn a_warrior_that_sees_the_player_walks_up_and_hurts_them() {
        let defs = test_defs();
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(5.0, 3.0));
        let mut targets = vec![brute_at(&defs, Vec2::new(7.5, 3.0))];
        run(&mut targets, &defs, &room, &mut player, 60 * 12);
        assert!(targets[0].origin.x < 7.5 - 1.0, "it closed in: {}", targets[0].origin);
        assert!(player.health <= player::MAX_HEALTH - 150.0, "and swung: {}", player.health);
        assert!(matches!(targets[0].enemy.as_ref().unwrap().animation, Some((Animation::Attack(_), _)) | Some((Animation::Run(_), _))));
    }

    #[test]
    fn a_warrior_in_another_block_does_nothing() {
        let defs = test_defs();
        let room = Room::test_room();
        // The test room is one block; the player stands outside it.
        let mut player = Player::new(Vec2::new(20.0, 3.0));
        let mut targets = vec![brute_at(&defs, Vec2::new(8.0, 3.0))];
        run(&mut targets, &defs, &room, &mut player, 120);
        assert_eq!(targets[0].origin, Vec2::new(8.0, 3.0));
        assert_eq!(targets[0].enemy.as_ref().unwrap().animation, None, "still the idle picture");
    }

    #[test]
    fn its_top_speed_is_unitys_at_sixty_frames_a_second() {
        let defs = test_defs();
        let def = defs.enemy(defs.enemy_named("brute").unwrap());
        // v = (v + 10 * 1.2 * 0.02) * (1 - 20 * 0.02) settles at 0.36.
        let mut speed = 0.0_f32;
        let Ai::Warrior(warrior) = &def.ai else { panic!("a warrior") };
        for _ in 0..600 {
            speed = (speed + warrior_acceleration(warrior, def) * DT) * drag_kept(def.drag);
        }
        assert!((speed - 0.36).abs() < 0.03, "{speed}");
    }

    #[test]
    fn a_wall_hides_the_player() {
        let defs = test_defs();
        let room = Room::test_room();
        let targets = vec![brute_at(&defs, Vec2::new(4.0, 3.0))];
        assert!(in_sight(&targets, 0, &room, Vec2::new(6.0, 3.0)));
        // The test room's outer wall stands between these two.
        assert!(!in_sight(&targets, 0, &room, Vec2::new(-1.0, 3.0)));
    }

    #[test]
    fn diagonal_headings_pick_no_axis_as_in_unity() {
        assert_eq!(within_45(Vec2::new(1.0, 1.0)), None);
        assert_eq!(within_45(Vec2::new(1.0, 0.9)), Some(Direction::Right));
        assert_eq!(within_45(Vec2::new(-0.2, -1.0)), Some(Direction::Down));
        assert_eq!(nearest_axis(Vec2::new(1.0, 1.0)), Some(Direction::Right));
    }
}
