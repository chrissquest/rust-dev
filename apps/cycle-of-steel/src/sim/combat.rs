//! Everything that happens between pulling a trigger and something being hit:
//! bullets in flight, the marks they leave, things that can be shot, and guns
//! lying on the floor.
//!
//! Bullets and effects live in plain vectors that are reserved once and reused, so
//! nothing is allocated while playing however much is fired.

use crystal_engine::glam::Vec2;

use super::defs::{BulletId, Defs, EffectId, GunDef};
use super::guns::Gun;
use super::rng::Rng;
use super::room::{Cell, Room};
use super::{DT, Event};
use crate::data::GunKind;

/// Room for this many bullets and effects before the vectors would grow.
const BULLET_CAPACITY: usize = 512;
const EFFECT_CAPACITY: usize = 256;

/// How far a laser reaches when nothing is in its way (`LaserGun.updateFire`).
pub const LASER_RANGE: f32 = 20.0;
/// A crit does this many times the gun's damage.
const CRIT_MULTIPLIER: f32 = 2.0;

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

/// Something bullets hit and hurt.
#[derive(Clone, Debug)]
pub struct Target {
    /// Stays the same for the target's life, unlike its place in the list.
    pub id: u32,
    pub position: Vec2,
    pub half: Vec2,
    pub health: f32,
    pub max_health: f32,
    /// When it was last hit, in world seconds, for the view to flash it.
    pub last_hit: f64,
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
}

#[derive(Default)]
pub struct Combat {
    pub bullets: Vec<Bullet>,
    pub effects: Vec<Effect>,
    pub targets: Vec<Target>,
    pub pickups: Vec<Pickup>,
    pub teleports: Vec<Teleport>,
    /// The player's laser beam, while it fires.
    pub beam: Option<Beam>,
}

impl Combat {
    pub fn new() -> Self {
        Self {
            bullets: Vec::with_capacity(BULLET_CAPACITY),
            effects: Vec::with_capacity(EFFECT_CAPACITY),
            targets: Vec::new(),
            pickups: Vec::new(),
            teleports: Vec::new(),
            beam: None,
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
            let damage = if rng.chance(crit_chance) { def.damage * CRIT_MULTIPLIER } else { def.damage };
            let target = &mut self.targets[index];
            target.health -= damage;
            target.last_hit = now;
        }
    }

    /// Moves bullets, resolves what they hit, and ages effects.
    pub fn step(&mut self, defs: &Defs, room: &Room, now: f64) {
        let mut index = 0;
        while index < self.bullets.len() {
            let bullet = &mut self.bullets[index];
            let def = defs.bullet(bullet.def);
            bullet.previous_position = bullet.position;
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
                    target.health -= damage;
                    target.last_hit = now;
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
                    if self.effects.len() == self.effects.capacity() {
                        self.effects.remove(0);
                    }
                    self.effects.push(Effect {
                        def: effect,
                        position: bullet.position,
                        rotation: bullet.rotation,
                        color: def.color,
                        time_left: defs.effect(effect).time_to_live,
                    });
                }
                self.bullets.swap_remove(index);
            } else if bullet.time_left <= 0.0 {
                self.bullets.swap_remove(index);
            } else {
                index += 1;
            }
        }

        self.targets.retain(|target| target.health > 0.0);

        self.effects.retain_mut(|effect| {
            effect.time_left -= DT;
            effect.time_left > 0.0
        });
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

        pickups
            .chain(teleports)
            .filter(|(_, position, half)| boxes_overlap(player, player_half, *position, *half))
            .min_by(|(_, a, _), (_, b, _)| a.distance_squared(player).total_cmp(&b.distance_squared(player)))
            .map(|(interaction, _, _)| interaction)
    }
}

/// Bullets fly over ledges; only walls stop them.
fn bullet_hits_wall(room: &Room, center: Vec2, half: Vec2) -> bool {
    let (min, max) = room.cell_range(center, half);
    (min.1..=max.1).any(|y| (min.0..=max.0).any(|x| room.cell_at(x, y) == Cell::Wall))
}

/// How far along a ray from `origin` in the unit vector `direction` a box is first
/// touched. A ray starting inside the box touches it at once.
fn ray_hits_box(origin: Vec2, direction: Vec2, center: Vec2, half: Vec2) -> Option<f32> {
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

fn boxes_overlap(a: Vec2, a_half: Vec2, b: Vec2, b_half: Vec2) -> bool {
    let gap = (a - b).abs();
    gap.x < a_half.x + b_half.x && gap.y < a_half.y + b_half.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::defs::tests::test_defs;

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
            last_hit: f64::NEG_INFINITY,
        }
    }

    fn run(combat: &mut Combat, defs: &Defs, room: &Room, steps: u32) {
        for step in 0..steps {
            combat.step(defs, room, step as f64 * DT as f64);
        }
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
