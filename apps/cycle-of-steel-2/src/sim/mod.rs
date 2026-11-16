//! The game itself: world state and one fixed-rate step.
//!
//! Rules carried over from Cycle of Steel 1 because they paid off there: no
//! window, GPU, wall-clock time or system randomness in here (randomness comes
//! from [`rng`] only), one step is [`DT`] seconds, and nothing allocates while
//! playing. A world and a list of inputs always replay to the same game.
//!
//! For now the world is a player walking an empty floor: the base the parts
//! system will be built on.

pub mod rng;

use crystal_engine::glam::Vec2;

/// Steps per second.
pub const RATE: u32 = 60;
/// Seconds per step.
pub const DT: f32 = 1.0 / RATE as f32;

/// What one step is asked to do. Built by the app from whatever device is in use.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// Desired movement, length 0 to 1.
    pub movement: Vec2,
}

#[derive(Clone, Debug)]
pub struct Player {
    /// Centre, in tiles.
    pub position: Vec2,
    /// Where `position` was before the last step, for the view to blend from.
    pub previous: Vec2,
    pub velocity: Vec2,
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
    /// Steps taken since the world began.
    pub tick: u64,
    pub rng: rng::Rng,
    pub player: Player,
}

impl World {
    pub fn new(seed: u32) -> Self {
        Self {
            tick: 0,
            rng: rng::Rng::new(seed),
            player: Player {
                position: Vec2::ZERO,
                previous: Vec2::ZERO,
                velocity: Vec2::ZERO,
            },
        }
    }

    /// Advances the world by [`DT`].
    pub fn step(&mut self, input: &Input) {
        let player = &mut self.player;
        player.previous = player.position;
        let wanted = input.movement.clamp_length_max(1.0) * Player::SPEED;
        // Exponential approach: the same feel at any step rate.
        let kept = (-Player::RESPONSE * DT).exp();
        player.velocity = wanted + (player.velocity - wanted) * kept;
        player.position += player.velocity * DT;
        self.tick += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(world: &mut World, movement: Vec2, steps: u32) {
        for _ in 0..steps {
            world.step(&Input { movement });
        }
    }

    #[test]
    fn the_player_reaches_top_speed_quickly_and_stops_quickly() {
        let mut world = World::new(1);
        walk(&mut world, Vec2::X, RATE / 4);
        assert!(world.player.velocity.x > Player::SPEED * 0.99);
        assert!(world.player.position.x > 0.0);
        walk(&mut world, Vec2::ZERO, RATE / 4);
        assert!(world.player.velocity.length() < Player::SPEED * 0.01);
        assert_eq!(world.tick, u64::from(RATE / 2));
    }

    #[test]
    fn diagonal_movement_is_no_faster() {
        let mut world = World::new(1);
        walk(&mut world, Vec2::ONE * 5.0, RATE);
        assert!(world.player.velocity.length() <= Player::SPEED + 1e-4);
    }

    #[test]
    fn the_same_inputs_give_the_same_world() {
        let mut a = World::new(9);
        let mut b = World::new(9);
        walk(&mut a, Vec2::new(0.3, -1.0), 90);
        walk(&mut b, Vec2::new(0.3, -1.0), 90);
        assert_eq!(a.player.position, b.player.position);
        assert_eq!(a.rng.unit(), b.rng.unit());
    }
}
