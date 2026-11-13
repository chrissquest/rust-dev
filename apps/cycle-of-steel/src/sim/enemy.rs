//! Enemies: Unity's `Warrior` AI, which both the Warrior and the EnemyTank run.
//!
//! An enemy is a [`Target`] with an [`Enemy`] attached: bullets, beams and blasts
//! find it like any target, and this module moves it and lets it swing.
//!
//! The warrior wanders, picking a new direction every `wanderInterval`. Once the
//! player is within `followRange` and in plain sight it follows them for
//! `spottingLength` seconds, seeing them again renewing that. When the player
//! touches its reach it stops and swings: the blow lands 0.5 s in and hurts
//! during the next 0.4 s, and it swings at most every `damageCooldown`.

use crystal_engine::glam::Vec2;

use super::DT;
use super::collide::move_box;
use super::combat::{Target, UNITY_STEP, boxes_overlap, drag_kept, ray_hits_box};
use super::defs::{Animation, Defs, Direction, EnemyDef, EnemyId, OffsetBox};
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
    Target {
        id,
        position: origin + Vec2::from(def.offset),
        half: Vec2::from(def.half),
        origin,
        chest: None,
        enemy: Some(Enemy::new(enemy, def.mass, origin)),
        health: def.max_health,
        max_health: def.max_health,
        last_hit: f64::NEG_INFINITY,
        elemental: def.freezable.then(Default::default),
    }
}

/// One step of every enemy among `targets`: think, swing, move.
pub fn step_enemies(targets: &mut [Target], defs: &Defs, room: &Room, player: &mut Player, now: f64, rng: &mut Rng) {
    for index in 0..targets.len() {
        let Some(enemy) = &targets[index].enemy else {
            continue;
        };
        let def = defs.enemy(enemy.def);
        let force = think(targets, index, def, room, player.position, now, rng);
        swing(&mut targets[index], def, player, now);
        move_enemy(targets, index, def, room, force);
    }
}

/// `Warrior.Update`: the direction it pushes in this step, if any. It acts only
/// while the player is in the same screen-sized block of the world.
fn think(targets: &mut [Target], index: usize, def: &EnemyDef, room: &Room, player: Vec2, now: f64, rng: &mut Rng) -> Vec2 {
    let origin = targets[index].origin;
    let in_sight = |targets: &[Target]| in_sight(targets, index, room, player);
    let enemy = targets[index].enemy.as_ref().expect("an enemy");
    if block_of(origin) != block_of(player) || enemy.swing.is_some() {
        return Vec2::ZERO;
    }
    let warrior = &def.warrior;
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
fn swing(target: &mut Target, def: &EnemyDef, player: &mut Player, now: f64) {
    let warrior = &def.warrior;
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

/// Pushes the enemy by `force` (a unit direction or zero), applies drag, and
/// moves it through the room. Walls and ledges stop it, and so do chests.
fn move_enemy(targets: &mut [Target], index: usize, def: &EnemyDef, room: &Room, force: Vec2) {
    let (before, after) = targets.split_at_mut(index);
    let (target, after) = after.split_first_mut().expect("index is in range");
    let solids = before
        .iter()
        .chain(after.iter())
        .filter(|other| other.chest.is_some())
        .map(|other| (other.position, other.half));
    let added_drag = target.elemental.as_ref().map_or(0.0, |elemental| elemental.added_drag());
    let enemy = target.enemy.as_mut().expect("an enemy");

    enemy.previous_origin = target.origin;
    enemy.velocity += force * push_acceleration(def) * DT;
    enemy.velocity *= drag_kept(def.drag + added_drag);
    let half = Vec2::from(def.half);
    let center = target.origin + Vec2::from(def.offset);
    let moved = move_box(room, solids, center, half, enemy.velocity * DT);
    if moved.hit_x {
        enemy.velocity.x = 0.0;
    }
    if moved.hit_y {
        enemy.velocity.y = 0.0;
    }
    target.position = moved.position;
    target.origin = moved.position - Vec2::from(def.offset);
}

/// Units/s^2 from Unity's per-frame push of `moveSpeed * difficulty` on its mass,
/// summed over each 0.02 s physics step.
fn push_acceleration(def: &EnemyDef) -> f32 {
    def.warrior.move_speed * DIFFICULTY * UNITY_FRAME_RATE * UNITY_STEP / def.mass
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

    fn run(targets: &mut [Target], defs: &Defs, room: &Room, player: &mut Player, steps: u32) {
        let mut rng = Rng::new(5);
        for tick in 0..steps {
            step_enemies(targets, defs, room, player, tick as f64 * DT as f64, &mut rng);
        }
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
        for _ in 0..600 {
            speed = (speed + push_acceleration(def) * DT) * drag_kept(def.drag);
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
