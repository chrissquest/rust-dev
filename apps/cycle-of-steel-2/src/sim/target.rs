//! Training dummies: things to shoot that never die.
//!
//! A dummy is held to its spot by a spring, so hits knock it back and it wobbles
//! home. It keeps a running total of the hits in the current burst, which the
//! view shows above it: rapid fire reads as one climbing number.

use crystal_engine::glam::Vec2;

/// Half the dummy's width and height, tiles (12 by 18 pixels at 16 per tile).
pub const HALF_SIZE: Vec2 = Vec2::new(0.375, 0.5625);
/// Seconds without a hit before the running total resets.
pub const COMBO_WINDOW: f32 = 0.8;
/// Seconds a hit flashes the dummy white.
pub const FLASH: f32 = 0.06;
/// The spring home: stiffness and damping per second.
const STIFFNESS: f32 = 220.0;
const DAMPING: f32 = 16.0;

#[derive(Clone, Debug)]
pub struct Target {
    pub home: Vec2,
    pub position: Vec2,
    pub previous: Vec2,
    pub velocity: Vec2,
    /// Seconds of hit flash left.
    pub flash: f32,
    /// Damage and hits in the current burst.
    pub combo_total: u32,
    pub combo_hits: u32,
    /// Seconds left before the burst ends.
    pub combo_timer: f32,
    /// Seconds since the total last grew, for the view's pop.
    pub combo_age: f32,
}

impl Target {
    pub fn new(home: Vec2) -> Self {
        Self {
            home,
            position: home,
            previous: home,
            velocity: Vec2::ZERO,
            flash: 0.0,
            combo_total: 0,
            combo_hits: 0,
            combo_timer: 0.0,
            combo_age: 0.0,
        }
    }

    pub fn contains(&self, point: Vec2) -> bool {
        let offset = (point - self.position).abs();
        offset.x <= HALF_SIZE.x && offset.y <= HALF_SIZE.y
    }

    /// Takes a hit: flash, knockback (`push`, tiles per second) and the running total.
    pub fn hit(&mut self, damage: u32, push: Vec2) {
        self.flash = FLASH;
        self.velocity += push;
        self.combo_total += damage;
        self.combo_hits += 1;
        self.combo_timer = COMBO_WINDOW;
        self.combo_age = 0.0;
    }

    pub fn step(&mut self, dt: f32) {
        self.previous = self.position;
        let pull = (self.home - self.position) * STIFFNESS - self.velocity * DAMPING;
        self.velocity += pull * dt;
        self.position += self.velocity * dt;
        self.flash = (self.flash - dt).max(0.0);
        self.combo_age += dt;
        if self.combo_timer > 0.0 {
            self.combo_timer -= dt;
            if self.combo_timer <= 0.0 {
                self.combo_total = 0;
                self.combo_hits = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_knocks_back_then_the_spring_brings_it_home() {
        let mut target = Target::new(Vec2::new(5.0, 0.0));
        target.hit(10, Vec2::new(3.0, 0.0));
        target.step(1.0 / 60.0);
        assert!(target.position.x > 5.0);
        for _ in 0..120 {
            target.step(1.0 / 60.0);
        }
        assert!((target.position - target.home).length() < 0.01);
    }

    #[test]
    fn hits_in_a_burst_add_up_and_reset_after_a_pause() {
        let mut target = Target::new(Vec2::ZERO);
        target.hit(10, Vec2::ZERO);
        target.step(0.1);
        target.hit(25, Vec2::ZERO);
        assert_eq!((target.combo_total, target.combo_hits), (35, 2));
        for _ in 0..60 {
            target.step(1.0 / 60.0);
        }
        assert_eq!(target.combo_total, 0);
    }
}
