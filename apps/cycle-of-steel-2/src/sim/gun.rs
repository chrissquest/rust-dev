//! The one gun of the feel prototype, and its bullets.
//!
//! Every number here is a first guess to be tuned by playing; the parts system
//! (roadmap phase 3) will compute them from fitted parts instead.

use crystal_engine::glam::Vec2;

use super::rng::Rng;

/// Shots per second while the trigger is held.
pub const FIRE_RATE: f32 = 9.0;
/// Bullet speed, tiles per second.
pub const BULLET_SPEED: f32 = 22.0;
/// Seconds a bullet flies before it fizzles.
pub const BULLET_LIFE: f32 = 1.5;
/// Half the cone a shot can leave in, degrees.
pub const SPREAD_DEGREES: f32 = 3.5;
/// Damage of a normal hit, inclusive range: a little variety keeps numbers alive.
pub const DAMAGE: (u32, u32) = (9, 14);
pub const CRIT_CHANCE: f32 = 0.15;
pub const CRIT_MULTIPLIER: f32 = 2.5;
/// From the player's centre to the muzzle, tiles.
pub const BARREL: f32 = 0.8;
/// How hard each shot shoves the shooter back, tiles per second.
pub const RECOIL: f32 = 0.5;
/// Bullets alive at once; the pool is reserved up front.
pub const MAX_BULLETS: usize = 256;
/// Seconds the muzzle flash shows.
const FLASH: f32 = 0.05;

#[derive(Clone, Debug)]
pub struct Bullet {
    pub position: Vec2,
    pub previous: Vec2,
    pub velocity: Vec2,
    pub damage: u32,
    pub crit: bool,
    pub life: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Gun {
    /// Seconds until the next shot may leave.
    pub cooldown: f32,
    /// Visual recoil, 1 just after a shot and falling to 0.
    pub kick: f32,
    /// Seconds of muzzle flash left.
    pub flash: f32,
    pub shots: u32,
}

impl Gun {
    /// Advances the gun by `dt`; fires if the trigger is held and it is ready.
    /// Returns whether a shot left.
    pub fn step(&mut self, dt: f32, trigger: bool, origin: Vec2, aim: Vec2, rng: &mut Rng, bullets: &mut Vec<Bullet>) -> bool {
        self.cooldown = (self.cooldown - dt).max(0.0);
        self.kick = (self.kick - dt * 12.0).max(0.0);
        self.flash = (self.flash - dt).max(0.0);
        if !trigger || self.cooldown > 0.0 || bullets.len() >= MAX_BULLETS {
            return false;
        }
        self.cooldown = 1.0 / FIRE_RATE;
        self.kick = 1.0;
        self.flash = FLASH;
        self.shots += 1;

        let angle = aim.y.atan2(aim.x) + rng.range(-SPREAD_DEGREES, SPREAD_DEGREES).to_radians();
        let direction = Vec2::from_angle(angle);
        let crit = rng.chance(CRIT_CHANCE);
        let base = DAMAGE.0 + (rng.unit() * (DAMAGE.1 - DAMAGE.0 + 1) as f32) as u32;
        let damage = if crit { (base as f32 * CRIT_MULTIPLIER).round() as u32 } else { base };
        let position = origin + aim * BARREL;
        bullets.push(Bullet {
            position,
            previous: position,
            velocity: direction * BULLET_SPEED,
            damage,
            crit,
            life: BULLET_LIFE,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holding_the_trigger_fires_at_the_fire_rate() {
        let mut gun = Gun::default();
        let mut rng = Rng::new(3);
        let mut bullets = Vec::with_capacity(MAX_BULLETS);
        for _ in 0..60 {
            gun.step(1.0 / 60.0, true, Vec2::ZERO, Vec2::X, &mut rng, &mut bullets);
        }
        // One second of fire: the first shot is immediate.
        assert!((9..=10).contains(&bullets.len()), "{} shots", bullets.len());
        assert!(bullets.iter().all(|b| b.velocity.x > 0.0 && b.damage >= DAMAGE.0));
    }

    #[test]
    fn crits_hit_harder_than_any_normal_shot() {
        let mut gun = Gun::default();
        let mut rng = Rng::new(5);
        let mut bullets = Vec::with_capacity(MAX_BULLETS);
        for _ in 0..200 {
            gun.cooldown = 0.0;
            gun.step(0.0, true, Vec2::ZERO, Vec2::X, &mut rng, &mut bullets);
        }
        assert!(bullets.iter().any(|b| b.crit));
        for bullet in &bullets {
            assert_eq!(bullet.crit, bullet.damage > DAMAGE.1);
        }
    }
}
