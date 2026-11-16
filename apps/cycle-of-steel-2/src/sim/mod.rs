//! The game itself: world state and one fixed-rate step.
//!
//! Rules carried over from Cycle of Steel 1 because they paid off there: no
//! window, GPU, wall-clock time or system randomness in here (randomness comes
//! from [`rng`] only), one step is [`DT`] seconds, and nothing allocates while
//! playing. A world and a list of inputs always replay to the same game.
//!
//! For now the world is the feel prototype (roadmap phase 2): one gun, a room
//! with training dummies, and the feedback that makes hits land.

pub mod feedback;
pub mod gun;
pub mod rng;
pub mod target;

use crystal_engine::glam::Vec2;

use feedback::{Feedback, NumberKind, SparkKind};
use gun::{Bullet, Gun};
use target::Target;

/// Steps per second.
pub const RATE: u32 = 60;
/// Seconds per step.
pub const DT: f32 = 1.0 / RATE as f32;
/// Half the test room's inside, tiles. Its walls stop the player and bullets.
pub const ROOM_HALF: Vec2 = Vec2::new(11.0, 7.0);

/// What one step is asked to do. Built by the app from whatever device is in use.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// Desired movement, length 0 to 1.
    pub movement: Vec2,
    /// Where to aim, any length; zero keeps the last aim.
    pub aim: Vec2,
    /// The trigger is held.
    pub fire: bool,
}

#[derive(Clone, Debug)]
pub struct Player {
    /// Centre, in tiles.
    pub position: Vec2,
    /// Where `position` was before the last step, for the view to blend from.
    pub previous: Vec2,
    pub velocity: Vec2,
    /// Unit vector the gun points along.
    pub aim: Vec2,
}

impl Player {
    /// Top speed in tiles per second. A first guess, to be tuned by feel: a
    /// little quicker than Cycle of Steel 1's so dodging reads as a skill.
    pub const SPEED: f32 = 6.0;
    /// How fast velocity closes on the desired velocity, per second. High,
    /// so the player starts and stops almost at once.
    pub const RESPONSE: f32 = 20.0;
    /// Half the player's width and height, in tiles.
    pub const HALF_SIZE: f32 = 0.35;
}

#[derive(Clone, Debug)]
pub struct World {
    /// Steps taken since the world began, hit-stop included.
    pub tick: u64,
    pub rng: rng::Rng,
    pub player: Player,
    pub gun: Gun,
    pub bullets: Vec<Bullet>,
    pub targets: Vec<Target>,
    pub feedback: Feedback,
}

impl World {
    pub fn new(seed: u32) -> Self {
        let targets = [(3.0, 2.5), (5.0, 0.0), (3.0, -2.5), (6.5, 4.0), (6.5, -4.0)]
            .into_iter()
            .map(|(x, y)| Target::new(Vec2::new(x, y)))
            .collect();
        Self {
            tick: 0,
            rng: rng::Rng::new(seed),
            player: Player {
                position: Vec2::new(-4.0, 0.0),
                previous: Vec2::new(-4.0, 0.0),
                velocity: Vec2::ZERO,
                aim: Vec2::X,
            },
            gun: Gun::default(),
            bullets: Vec::with_capacity(gun::MAX_BULLETS),
            targets,
            feedback: Feedback::default(),
        }
    }

    /// Advances the world by [`DT`].
    pub fn step(&mut self, input: &Input) {
        self.tick += 1;
        if self.feedback.hitstop > 0 {
            // Everything holds still; the view must not blend between steps.
            self.feedback.hitstop -= 1;
            self.hold_still();
            return;
        }

        let player = &mut self.player;
        player.previous = player.position;
        if input.aim.length_squared() > 1e-6 {
            player.aim = input.aim.normalize();
        }
        let wanted = input.movement.clamp_length_max(1.0) * Player::SPEED;
        // Exponential approach: the same feel at any step rate.
        let kept = (-Player::RESPONSE * DT).exp();
        player.velocity = wanted + (player.velocity - wanted) * kept;
        let limit = ROOM_HALF - Vec2::splat(Player::HALF_SIZE);
        player.position = (player.position + player.velocity * DT).clamp(-limit, limit);

        if self.gun.step(DT, input.fire, player.position, player.aim, &mut self.rng, &mut self.bullets) {
            player.velocity -= player.aim * gun::RECOIL;
        }
        self.move_bullets();
        for target in &mut self.targets {
            target.step(DT);
        }
        self.feedback.step(DT);
    }

    fn move_bullets(&mut self) {
        let feedback = &mut self.feedback;
        let rng = &mut self.rng;
        let targets = &mut self.targets;
        self.bullets.retain_mut(|bullet| {
            bullet.previous = bullet.position;
            bullet.position += bullet.velocity * DT;
            bullet.life -= DT;
            let direction = bullet.velocity.normalize_or_zero();
            if let Some(target) = targets.iter_mut().find(|target| target.contains(bullet.position)) {
                let push = direction * if bullet.crit { 4.0 } else { 1.5 };
                target.hit(bullet.damage, push);
                let top = target.position + Vec2::new(0.0, target::HALF_SIZE.y);
                if bullet.crit {
                    feedback.number(top, bullet.damage, NumberKind::Crit, rng);
                    feedback.burst(bullet.position, -direction, 10, SparkKind::Crit, rng);
                    feedback.shake(feedback::CRIT_SHAKE);
                    feedback.hitstop = feedback.hitstop.max(feedback::CRIT_HITSTOP);
                } else {
                    feedback.number(top, bullet.damage, NumberKind::Normal, rng);
                    feedback.burst(bullet.position, -direction, 4, SparkKind::Impact, rng);
                    feedback.shake(feedback::HIT_SHAKE);
                }
                return false;
            }
            let outside = bullet.position.abs().cmpgt(ROOM_HALF).any();
            if outside {
                let at = bullet.position.clamp(-ROOM_HALF, ROOM_HALF);
                feedback.burst(at, -direction, 3, SparkKind::Wall, rng);
                return false;
            }
            bullet.life > 0.0
        });
    }

    fn hold_still(&mut self) {
        self.player.previous = self.player.position;
        for bullet in &mut self.bullets {
            bullet.previous = bullet.position;
        }
        for target in &mut self.targets {
            target.previous = target.position;
        }
        for number in &mut self.feedback.numbers {
            number.previous = number.position;
        }
        for spark in &mut self.feedback.sparks {
            spark.previous = spark.position;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(world: &mut World, input: Input, steps: u32) {
        for _ in 0..steps {
            world.step(&input);
        }
    }

    #[test]
    fn the_player_reaches_top_speed_quickly_and_stops_quickly() {
        let mut world = World::new(1);
        run(&mut world, Input { movement: Vec2::X, ..Input::default() }, RATE / 4);
        assert!(world.player.velocity.x > Player::SPEED * 0.99);
        run(&mut world, Input::default(), RATE / 4);
        assert!(world.player.velocity.length() < Player::SPEED * 0.01);
    }

    #[test]
    fn diagonal_movement_is_no_faster_and_walls_hold() {
        let mut world = World::new(1);
        run(&mut world, Input { movement: Vec2::ONE * 5.0, ..Input::default() }, RATE * 5);
        assert!(world.player.velocity.length() <= Player::SPEED + 1e-4);
        assert!(world.player.position.x <= ROOM_HALF.x && world.player.position.y <= ROOM_HALF.y);
    }

    #[test]
    fn shooting_a_dummy_makes_numbers_sparks_and_shake() {
        let mut world = World::new(2);
        // The middle dummy is straight ahead of the start.
        let aim = world.targets[1].home - world.player.position;
        let mut most_shake: f32 = 0.0;
        for _ in 0..RATE {
            world.step(&Input { aim, fire: true, ..Input::default() });
            most_shake = most_shake.max(world.feedback.trauma);
        }
        let target = &world.targets[1];
        assert!(target.combo_hits > 0, "no hits");
        assert!(!world.feedback.numbers.is_empty());
        assert!(!world.feedback.sparks.is_empty());
        assert!(most_shake > 0.0);
        assert!(world.bullets.capacity() == gun::MAX_BULLETS);
    }

    #[test]
    fn a_crit_freezes_the_world_briefly() {
        let mut world = World::new(2);
        world.feedback.hitstop = feedback::CRIT_HITSTOP;
        world.player.velocity = Vec2::X;
        let before = world.player.position;
        run(&mut world, Input { movement: Vec2::X, ..Input::default() }, feedback::CRIT_HITSTOP);
        assert_eq!(world.player.position, before);
        world.step(&Input { movement: Vec2::X, ..Input::default() });
        assert!(world.player.position.x > before.x);
    }

    #[test]
    fn the_same_inputs_give_the_same_world() {
        let input = Input { movement: Vec2::new(0.3, -1.0), aim: Vec2::new(1.0, 0.2), fire: true };
        let mut a = World::new(9);
        let mut b = World::new(9);
        run(&mut a, input, 120);
        run(&mut b, input, 120);
        assert_eq!(a.player.position, b.player.position);
        assert_eq!(a.feedback.numbers.len(), b.feedback.numbers.len());
        assert_eq!(a.rng.unit(), b.rng.unit());
    }
}
