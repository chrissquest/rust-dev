//! The player: movement, collision and which way they face.
//!
//! The numbers reproduce the Unity player. There, `PlayerInput` added a force of
//! `move * moveSpeed * deltaTime * 100` every frame to a body with mass 1 and
//! linear drag 9 (`moveSpeed` 10). Summed over a 50 Hz physics step that is an
//! acceleration of 20 units/s^2, and Unity's 2D drag (Box2D's) divides velocity
//! by `1 + drag * dt` = 1.18 per step. The player therefore tops out at about
//! 2.22 units/s and keeps 1 / 1.18 of their speed every 0.02 s when input stops.
//!
//! Here the same curve is integrated exactly, so it does not depend on the step
//! rate: velocity decays towards `input * TOP_SPEED` at `RESPONSE` per second.

use crystal_engine::glam::Vec2;

use super::collide::move_box;
use super::room::Room;
use super::{DT, Input};

/// Speed reached while holding a direction, in world units per second.
/// Unity: the fixed point of `v = (v + 0.4) / 1.18`.
pub const TOP_SPEED: f32 = 0.4 / 0.18;
/// How quickly velocity closes on its target, per second. Unity: `ln(1.18) / 0.02`.
pub const RESPONSE: f32 = 8.275_722;

/// Half the collision box. Unity's player has a 0.19 x 0.48 capsule; this is its
/// bounding box, so corners are squarer than they were.
pub const HALF_SIZE: Vec2 = Vec2::new(0.095, 0.24);

/// Where the gun arm attaches and where its hands sit, all relative: the pivot to
/// the player's position, the hands to the pivot (before the arm is turned).
///
/// These come from the Unity player prefab and its three pivot animations. The
/// body sprites have no arms; the arm is drawn separately, so it has to follow
/// the shoulder as the body turns side-on and bobs with each stride.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arm {
    pub pivot: Vec2,
    pub left_hand: Vec2,
    pub right_hand: Vec2,
}

/// Standing still: the prefab's own placement.
const ARM_STANDING: Arm = Arm {
    pivot: Vec2::new(0.0, 0.15),
    left_hand: Vec2::new(-0.125, 0.025),
    right_hand: Vec2::new(-0.066, -0.068),
};
/// Running towards or away from the camera (`player_pivot.anim`).
const ARM_RUNNING_FACING: Arm = Arm {
    pivot: Vec2::new(0.0, 0.12),
    ..ARM_STANDING
};
/// Running left (`player_pivot_left.anim`): the shoulder is off-centre on a
/// side-on body and the hands sit closer in.
const ARM_RUNNING_LEFT: Arm = Arm {
    pivot: Vec2::new(-0.088, 0.10),
    left_hand: Vec2::new(-0.089, 0.02),
    right_hand: Vec2::new(-0.034, -0.027),
};
/// Running right (`player_pivot_right.anim`).
const ARM_RUNNING_RIGHT: Arm = Arm {
    pivot: Vec2::new(0.078, 0.10),
    ..ARM_RUNNING_LEFT
};
/// How far the shoulder rises and falls over a stride: one value per animation
/// frame, repeating every half second.
const STRIDE_BOB: [f32; 6] = [0.0, 0.02, 0.01, 0.0, -0.02, -0.01];

/// Bullets leave from this far along the aim from the pivot.
/// `PlayerData.maxHealth` on `Prefabs/Player.prefab`.
pub const MAX_HEALTH: f32 = 1000.0;
/// `PlayerData.damagedInvulnerableTime` on `Prefabs/Player.prefab`, in seconds.
pub const INVULNERABLE_TIME: f64 = 0.5;

pub const MUZZLE_DISTANCE: f32 = 0.35;

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
    /// Where the trigger is being held towards, as a unit vector; `None` when it
    /// is not held.
    pub aim: Option<Vec2>,
    /// Whether the last sideways movement was to the right. It decides which side
    /// the gun is carried on while not aiming.
    pub moved_right: bool,
    /// Chance of a shot doing double damage, 0..=1. Raised by upgrades later.
    pub crit_chance: f32,
    /// Enemies take this down and health packs fill it back up to the top.
    pub health: f32,
    pub max_health: f32,
    /// When the player was last hurt, in world seconds.
    pub last_hurt: f64,
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
            aim: None,
            moved_right: false,
            crit_chance: 0.0,
            health: MAX_HEALTH,
            max_health: MAX_HEALTH,
            last_hurt: f64::NEG_INFINITY,
        }
    }

    /// Where the gun arm attaches right now, and where its hands sit on it.
    pub fn arm(&self) -> Arm {
        if !self.running {
            return ARM_STANDING;
        }
        let mut arm = match self.facing {
            Facing::Front | Facing::Back => ARM_RUNNING_FACING,
            Facing::Left => ARM_RUNNING_LEFT,
            Facing::Right => ARM_RUNNING_RIGHT,
        };
        let frame = (self.animation_time * ANIMATION_FPS) as usize % STRIDE_BOB.len();
        arm.pivot.y += STRIDE_BOB[frame];
        arm
    }

    /// The point the gun arm turns about, in world space.
    pub fn pivot(&self) -> Vec2 {
        self.position + self.arm().pivot
    }

    /// Where a bullet fired along `aim` starts.
    pub fn muzzle(&self, aim: Vec2) -> Vec2 {
        self.pivot() + aim * MUZZLE_DISTANCE
    }

    /// `PlayerData.DamagePlayer`: nothing lands within [`INVULNERABLE_TIME`] of the
    /// last blow that did. Returns whether this one did.
    pub fn hurt(&mut self, amount: f32, now: f64) -> bool {
        if now - self.last_hurt <= INVULNERABLE_TIME {
            return false;
        }
        self.health -= amount;
        self.last_hurt = now;
        true
    }

    /// `solids` are boxes (centre, half size) that block the player besides walls.
    pub fn step(&mut self, input: &Input, room: &Room, solids: impl Iterator<Item = (Vec2, Vec2)> + Clone) {
        self.previous_position = self.position;

        self.aim = input.aim.and_then(Vec2::try_normalize);
        let movement = input.movement.clamp_length_max(1.0);
        // Unity only re-evaluates the carry side while there is movement input.
        if movement != Vec2::ZERO {
            self.moved_right = movement.x > 0.0;
        }
        let target_velocity = movement * TOP_SPEED;
        self.velocity = target_velocity + (self.velocity - target_velocity) * (-RESPONSE * DT).exp();

        let moved = move_box(room, solids, self.position, HALF_SIZE, self.velocity * DT);
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
            player.step(&Input { movement, ..Input::default() }, room, std::iter::empty());
        }
    }

    #[test]
    fn reaches_unitys_top_speed_and_no_more() {
        let room = Room::test_room();
        // The middle rows of the test room are clear from wall to wall.
        let mut player = Player::new(Vec2::new(2.0, 4.5));
        step_for(&mut player, &room, Vec2::new(1.0, 0.0), 2.0);
        assert!((player.velocity.x - TOP_SPEED).abs() < 1e-3, "{}", player.velocity.x);
        assert!((TOP_SPEED - 2.222).abs() < 1e-3);
    }

    #[test]
    fn slows_like_unity_after_letting_go() {
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(2.0, 4.5));
        step_for(&mut player, &room, Vec2::new(1.0, 0.0), 2.0);
        let before = player.velocity.x;
        // 0.1 s is five of Unity's 0.02 s physics steps, each keeping 1 / 1.18 of the speed.
        step_for(&mut player, &room, Vec2::ZERO, 0.1);
        let expected = before * (1.0f32 / 1.18).powf(0.1 / 0.02);
        assert!((player.velocity.x - expected).abs() < 1e-3, "{} vs {expected}", player.velocity.x);
    }

    #[test]
    fn diagonal_input_is_not_faster() {
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(4.0, 2.0));
        step_for(&mut player, &room, Vec2::new(1.0, 1.0), 1.2);
        assert!((player.velocity.length() - TOP_SPEED).abs() < 1e-3);
    }

    #[test]
    fn the_arm_follows_the_shoulder_while_running_and_returns_when_standing() {
        let room = Room::test_room();
        let mut player = Player::new(Vec2::new(4.0, 4.5));
        assert_eq!(player.arm(), ARM_STANDING);

        // Running left the shoulder is left of centre, and it bobs with the stride.
        step_for(&mut player, &room, Vec2::new(-1.0, 0.0), 0.05);
        assert_eq!(player.arm().pivot, Vec2::new(-0.088, 0.10));
        step_for(&mut player, &room, Vec2::new(-1.0, 0.0), 0.1);
        assert_eq!(player.animation_frame(), 1);
        assert!((player.arm().pivot.y - 0.12).abs() < 1e-6);
        assert_eq!(player.arm().left_hand, Vec2::new(-0.089, 0.02));

        // Running up keeps the standing hands but lowers the shoulder.
        step_for(&mut player, &room, Vec2::new(0.0, 1.0), 0.05);
        assert_eq!(player.arm().pivot.x, 0.0);
        assert_eq!(player.arm().left_hand, ARM_STANDING.left_hand);

        step_for(&mut player, &room, Vec2::ZERO, 0.05);
        assert_eq!(player.arm(), ARM_STANDING);
        assert_eq!(player.pivot(), player.position + Vec2::new(0.0, 0.15));
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
