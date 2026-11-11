//! The player: movement, collision and which way they face.
//!
//! The numbers reproduce the Unity player. There, `PlayerInput` added a force of
//! `move * moveSpeed * deltaTime * 100` every frame to a body with mass 1 and
//! linear drag 9 (`moveSpeed` 10). Summed over a 50 Hz physics step that is an
//! acceleration of 20 units/s^2, and Unity's drag multiplies velocity by
//! `1 - drag * dt` = 0.82 per step. The player therefore tops out at about
//! 1.82 units/s and loses 18% of their speed every 0.02 s when input stops.
//!
//! Here the same curve is integrated exactly, so it does not depend on the step
//! rate: velocity decays towards `input * TOP_SPEED` at `RESPONSE` per second.

use crystal_engine::glam::Vec2;

use super::collide::move_box;
use super::room::Room;
use super::{DT, Input};

/// Speed reached while holding a direction, in world units per second.
/// Unity: the fixed point of `v = (v + 0.4) * 0.82`.
pub const TOP_SPEED: f32 = 0.328 / 0.18;
/// How quickly velocity closes on its target, per second. Unity: `-ln(0.82) / 0.02`.
pub const RESPONSE: f32 = 9.922_546;

/// Half the collision box. Unity's player has a 0.19 x 0.48 capsule; this is its
/// bounding box, so corners are squarer than they were.
pub const HALF_SIZE: Vec2 = Vec2::new(0.095, 0.24);

/// The run animations play at 12 frames per second and have 12 frames.
pub const ANIMATION_FPS: f32 = 12.0;
pub const ANIMATION_FRAMES: u32 = 12;

/// Which run animation is showing. Named for the sprite sheets: `Front` is the
/// player seen from the front, i.e. walking down the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    Front,
    Back,
    Left,
    Right,
}

pub struct Player {
    pub position: Vec2,
    /// Position one step ago, for the view to blend from.
    pub previous_position: Vec2,
    pub velocity: Vec2,
    pub facing: Facing,
    /// Whether a direction is being held: the run animation plays only then.
    pub running: bool,
    /// Seconds into the current run animation.
    pub animation_time: f32,
}

impl Player {
    pub fn new(position: Vec2) -> Self {
        Self {
            position,
            previous_position: position,
            velocity: Vec2::ZERO,
            facing: Facing::Front,
            running: false,
            animation_time: 0.0,
        }
    }

    pub fn step(&mut self, input: &Input, room: &Room) {
        self.previous_position = self.position;

        let movement = input.movement.clamp_length_max(1.0);
        let target_velocity = movement * TOP_SPEED;
        self.velocity = target_velocity + (self.velocity - target_velocity) * (-RESPONSE * DT).exp();

        let moved = move_box(room, self.position, HALF_SIZE, self.velocity * DT);
        self.position = moved.position;
        if moved.hit_x {
            self.velocity.x = 0.0;
        }
        if moved.hit_y {
            self.velocity.y = 0.0;
        }

        self.animate(movement);
    }

    /// Unity swapped the animator whenever the move input changed: sideways input
    /// wins over vertical, and letting go freezes the sprite.
    fn animate(&mut self, movement: Vec2) {
        let facing = if movement.x > 0.0 {
            Some(Facing::Right)
        } else if movement.x < 0.0 {
            Some(Facing::Left)
        } else if movement.y > 0.0 {
            Some(Facing::Back)
        } else if movement.y < 0.0 {
            Some(Facing::Front)
        } else {
            None
        };

        match facing {
            Some(facing) => {
                if !self.running || facing != self.facing {
                    self.animation_time = 0.0;
                } else {
                    self.animation_time += DT;
                }
                self.facing = facing;
                self.running = true;
            }
            None => {
                self.running = false;
                self.animation_time = 0.0;
            }
        }
    }

    /// The frame of the facing's sprite sheet to show.
    pub fn animation_frame(&self) -> u32 {
        (self.animation_time * ANIMATION_FPS) as u32 % ANIMATION_FRAMES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::TICK_RATE;

    fn step_for(player: &mut Player, room: &Room, movement: Vec2, seconds: f32) {
        for _ in 0..(seconds * TICK_RATE as f32).round() as u32 {
            player.step(&Input { movement }, room);
        }
    }

    #[test]
    fn reaches_unitys_top_speed_and_no_more() {
        let room = Room::test_room();
        // The middle rows of the test room are clear from wall to wall.
        let mut player = Player::new(Vec2::new(2.0, 4.5));
        step_for(&mut player, &room, Vec2::new(1.0, 0.0), 2.0);
        assert!((player.velocity.x - TOP_SPEED).abs() < 1e-3, "{}", player.velocity.x);
        assert!((TOP_SPEED - 1.822).abs() < 1e-3);
    }

    #[test]
    fn slows_like_unity_after_letting_go() {
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(2.0, 4.5));
        step_for(&mut player, &room, Vec2::new(1.0, 0.0), 2.0);
        let before = player.velocity.x;
        // 0.1 s is five of Unity's 0.02 s physics steps, each keeping 82% of the speed.
        step_for(&mut player, &room, Vec2::ZERO, 0.1);
        let expected = before * 0.82f32.powf(0.1 / 0.02);
        assert!((player.velocity.x - expected).abs() < 1e-3, "{} vs {expected}", player.velocity.x);
    }

    #[test]
    fn diagonal_input_is_not_faster() {
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(4.0, 2.0));
        step_for(&mut player, &room, Vec2::new(1.0, 1.0), 2.0);
        assert!((player.velocity.length() - TOP_SPEED).abs() < 1e-3);
    }

    #[test]
    fn sideways_wins_the_facing_and_stopping_freezes_the_animation() {
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(4.0, 2.0));
        step_for(&mut player, &room, Vec2::new(-1.0, 1.0), 0.5);
        assert_eq!(player.facing, Facing::Left);
        assert!(player.running);
        assert_eq!(player.animation_frame(), 5, "0.5 s at 12 fps, minus the starting step");

        step_for(&mut player, &room, Vec2::ZERO, 0.1);
        assert_eq!(player.facing, Facing::Left, "keeps facing the way it was going");
        assert!(!player.running);
        assert_eq!(player.animation_frame(), 0);
    }
}
