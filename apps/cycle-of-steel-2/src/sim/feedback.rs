//! What makes a hit feel like a hit: damage numbers, sparks, screen shake and
//! hit-stop.
//!
//! These live in the simulation, not the view, so they replay with the game
//! and tests can check them. All of it sits in pools reserved up front; when a
//! pool is full the oldest entry makes room.

use crystal_engine::glam::Vec2;

use super::rng::Rng;

/// Seconds a damage number lives.
pub const NUMBER_LIFE: f32 = 0.9;
pub const MAX_NUMBERS: usize = 128;
pub const MAX_SPARKS: usize = 512;
/// Steps the whole world freezes for on a critical hit.
pub const CRIT_HITSTOP: u32 = 3;
/// Shake added per hit and per critical hit; trauma runs 0 to 1.
pub const HIT_SHAKE: f32 = 0.1;
pub const CRIT_SHAKE: f32 = 0.45;
/// Trauma lost per second.
const SHAKE_DECAY: f32 = 1.5;
/// The camera's largest shake, tiles, reached at full trauma.
const SHAKE_MAX: f32 = 0.5;
/// Pull on damage numbers as they arc, tiles per second squared.
const NUMBER_GRAVITY: f32 = -14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberKind {
    Normal,
    Crit,
}

#[derive(Clone, Debug)]
pub struct DamageNumber {
    pub position: Vec2,
    pub previous: Vec2,
    pub velocity: Vec2,
    pub value: u32,
    pub kind: NumberKind,
    pub age: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SparkKind {
    /// A bullet hitting a target.
    Impact,
    /// A critical hit: bigger, brighter.
    Crit,
    /// A bullet hitting a wall.
    Wall,
}

#[derive(Clone, Debug)]
pub struct Spark {
    pub position: Vec2,
    pub previous: Vec2,
    pub velocity: Vec2,
    pub kind: SparkKind,
    pub age: f32,
    pub life: f32,
}

#[derive(Clone, Debug)]
pub struct Feedback {
    pub numbers: Vec<DamageNumber>,
    pub sparks: Vec<Spark>,
    /// Screen shake, 0 to 1; the camera moves by its square.
    pub trauma: f32,
    /// Steps left of hit-stop.
    pub hitstop: u32,
}

impl Default for Feedback {
    fn default() -> Self {
        Self {
            numbers: Vec::with_capacity(MAX_NUMBERS),
            sparks: Vec::with_capacity(MAX_SPARKS),
            trauma: 0.0,
            hitstop: 0,
        }
    }
}

impl Feedback {
    /// A number that pops up from `at` and arcs away, slightly to one side.
    pub fn number(&mut self, at: Vec2, value: u32, kind: NumberKind, rng: &mut Rng) {
        if self.numbers.len() >= MAX_NUMBERS {
            self.numbers.remove(0);
        }
        let up = if kind == NumberKind::Crit { 7.0 } else { 5.5 };
        self.numbers.push(DamageNumber {
            position: at,
            previous: at,
            velocity: Vec2::new(rng.range(-1.5, 1.5), up),
            value,
            kind,
            age: 0.0,
        });
    }

    /// `count` sparks thrown from `at`, mostly along `direction`.
    pub fn burst(&mut self, at: Vec2, direction: Vec2, count: u32, kind: SparkKind, rng: &mut Rng) {
        let base = direction.y.atan2(direction.x);
        for _ in 0..count {
            if self.sparks.len() >= MAX_SPARKS {
                self.sparks.swap_remove(0);
            }
            let angle = base + rng.range(-1.1, 1.1);
            let speed = rng.range(3.0, 9.0);
            self.sparks.push(Spark {
                position: at,
                previous: at,
                velocity: Vec2::from_angle(angle) * speed,
                kind,
                age: 0.0,
                life: rng.range(0.12, 0.3),
            });
        }
    }

    pub fn shake(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount).min(1.0);
    }

    pub fn step(&mut self, dt: f32) {
        self.trauma = (self.trauma - SHAKE_DECAY * dt).max(0.0);
        for number in &mut self.numbers {
            number.previous = number.position;
            number.velocity.y += NUMBER_GRAVITY * dt;
            number.velocity.x *= 1.0 - 3.0 * dt;
            number.position += number.velocity * dt;
            number.age += dt;
        }
        self.numbers.retain(|number| number.age < NUMBER_LIFE);
        for spark in &mut self.sparks {
            spark.previous = spark.position;
            spark.velocity *= 1.0 - 8.0 * dt;
            spark.position += spark.velocity * dt;
            spark.age += dt;
        }
        self.sparks.retain(|spark| spark.age < spark.life);
    }

    /// Where the camera sits off its target this step. Smooth noise from the
    /// step count, so it is the same on every replay.
    pub fn shake_offset(&self, tick: u64) -> Vec2 {
        let strength = self.trauma * self.trauma * SHAKE_MAX;
        let t = tick as f32 * 0.9;
        Vec2::new((t * 1.3).sin() + (t * 3.1).sin() * 0.5, (t * 1.7).cos() + (t * 2.3).sin() * 0.5) * (strength / 1.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_rise_then_expire() {
        let mut feedback = Feedback::default();
        let mut rng = Rng::new(1);
        feedback.number(Vec2::ZERO, 12, NumberKind::Normal, &mut rng);
        feedback.step(0.1);
        assert!(feedback.numbers[0].position.y > 0.0);
        for _ in 0..60 {
            feedback.step(1.0 / 60.0);
        }
        assert!(feedback.numbers.is_empty());
    }

    #[test]
    fn full_pools_drop_the_oldest_and_never_grow() {
        let mut feedback = Feedback::default();
        let mut rng = Rng::new(1);
        for value in 0..(MAX_NUMBERS as u32 + 10) {
            feedback.number(Vec2::ZERO, value, NumberKind::Normal, &mut rng);
        }
        assert_eq!(feedback.numbers.len(), MAX_NUMBERS);
        assert_eq!(feedback.numbers[0].value, 10);
        assert_eq!(feedback.numbers.capacity(), MAX_NUMBERS);
        feedback.burst(Vec2::ZERO, Vec2::X, MAX_SPARKS as u32 + 5, SparkKind::Impact, &mut rng);
        assert_eq!(feedback.sparks.capacity(), MAX_SPARKS);
    }

    #[test]
    fn shake_fades() {
        let mut feedback = Feedback::default();
        feedback.shake(CRIT_SHAKE);
        assert!(feedback.shake_offset(7).length() > 0.0);
        for _ in 0..60 {
            feedback.step(1.0 / 60.0);
        }
        assert_eq!(feedback.shake_offset(7), Vec2::ZERO);
    }
}
