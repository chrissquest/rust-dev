//! The game simulation: plain state advanced by a fixed step.
//!
//! Everything here runs at [`TICK_RATE`] steps per second regardless of how fast
//! frames are drawn. The frame loop calls [`World::step`] zero or more times per
//! frame and the view blends between the last two steps, so motion is smooth at
//! any refresh rate and identical on every machine.

pub mod collide;
pub mod player;
pub mod room;

use crystal_engine::glam::Vec2;

use player::Player;
use room::Room;

/// Simulation steps per second.
pub const TICK_RATE: u32 = 60;
/// Seconds per simulation step.
pub const DT: f32 = 1.0 / TICK_RATE as f32;

/// What the player is asking for during one step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Direction to move in, at most length 1. Zero means stand still.
    pub movement: Vec2,
}

pub struct World {
    pub room: Room,
    pub player: Player,
    /// Steps taken since the world was created.
    pub tick: u64,
}

impl World {
    pub fn new(room: Room) -> Self {
        let player = Player::new(room.spawn);
        Self {
            room,
            player,
            tick: 0,
        }
    }

    /// Advances the world by [`DT`].
    pub fn step(&mut self, input: &Input) {
        self.player.step(input, &self.room);
        self.tick += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(world: &mut World, movement: Vec2, seconds: f32) {
        let input = Input { movement };
        for _ in 0..(seconds * TICK_RATE as f32) as u32 {
            world.step(&input);
        }
    }

    #[test]
    fn the_same_inputs_give_the_same_world() {
        let mut a = World::new(Room::test_room());
        let mut b = World::new(Room::test_room());
        for world in [&mut a, &mut b] {
            run(world, Vec2::new(1.0, 0.0), 0.7);
            run(world, Vec2::new(-0.6, 0.8), 1.3);
            run(world, Vec2::ZERO, 0.2);
        }
        assert_eq!(a.player.position, b.player.position);
        assert_eq!(a.tick, b.tick);
    }

    #[test]
    fn walking_into_a_wall_stops_at_the_wall() {
        let mut world = World::new(Room::test_room());
        // Far longer than it takes to cross the room.
        run(&mut world, Vec2::new(-1.0, 0.0), 20.0);

        let left_edge = world.player.position.x - player::HALF_SIZE.x;
        // The test room's outer wall is one cell thick.
        assert!((left_edge - room::CELL).abs() < 1e-3, "left edge at {left_edge}");
        assert!(!world.room.box_hits_solid(world.player.position, player::HALF_SIZE));
    }
}
