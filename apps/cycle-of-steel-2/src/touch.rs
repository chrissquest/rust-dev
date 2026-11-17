//! Twin touch sticks for phones.
//!
//! The left half of the screen moves, the right half aims and fires. Both
//! sticks float: a stick starts wherever the thumb lands, so there is no
//! fixed spot to find without looking. Pushing the right stick past a small
//! dead zone fires; letting go stops.
//!
//! Positions here are window pixels with y down, as touch events give them.

use crystal_engine::glam::Vec2;

/// Stick reach as a share of the screen's shorter side.
const RADIUS_SHARE: f32 = 0.11;
/// Share of the reach the right stick must travel before it fires.
const FIRE_DEAD_ZONE: f32 = 0.25;
/// Share of the reach the left stick ignores, so a resting thumb stays still.
const MOVE_DEAD_ZONE: f32 = 0.08;

#[derive(Clone, Copy, Debug, Default)]
pub struct Stick {
    /// The finger holding it, if any.
    pub finger: Option<u64>,
    /// Where the finger landed, and where it is now.
    pub origin: Vec2,
    pub at: Vec2,
}

impl Stick {
    /// The push, length 0 to 1, y up.
    fn push(&self, radius: f32) -> Vec2 {
        if self.finger.is_none() || radius <= 0.0 {
            return Vec2::ZERO;
        }
        let offset = (self.at - self.origin) / radius;
        Vec2::new(offset.x, -offset.y).clamp_length_max(1.0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TouchSticks {
    pub left: Stick,
    pub right: Stick,
    /// Window size in pixels, for the halves and the reach.
    pub screen: Vec2,
    /// A touch has been seen: from then on the resting sticks are drawn.
    pub used: bool,
}

/// What the view draws for one stick, in window pixels with y down.
#[derive(Clone, Copy, Debug)]
pub struct StickVisual {
    pub base: Vec2,
    pub knob: Vec2,
    pub radius: f32,
    pub held: bool,
}

impl TouchSticks {
    pub fn radius(&self) -> f32 {
        self.screen.x.min(self.screen.y) * RADIUS_SHARE
    }

    pub fn start(&mut self, finger: u64, at: Vec2) {
        self.used = true;
        let stick = if at.x < self.screen.x * 0.5 { &mut self.left } else { &mut self.right };
        if stick.finger.is_none() {
            *stick = Stick { finger: Some(finger), origin: at, at };
        }
    }

    pub fn moved(&mut self, finger: u64, at: Vec2) {
        let radius = self.radius();
        for stick in [&mut self.left, &mut self.right] {
            if stick.finger == Some(finger) {
                stick.at = at;
                // Drag the base along when the thumb goes past the reach, so a
                // change of direction answers at once.
                let offset = at - stick.origin;
                if offset.length() > radius {
                    stick.origin = at - offset.normalize() * radius;
                }
            }
        }
    }

    pub fn end(&mut self, finger: u64) {
        for stick in [&mut self.left, &mut self.right] {
            if stick.finger == Some(finger) {
                stick.finger = None;
            }
        }
    }

    /// Lets go of everything, as when the app leaves the screen.
    pub fn release(&mut self) {
        self.left.finger = None;
        self.right.finger = None;
    }

    pub fn movement(&self) -> Vec2 {
        let push = self.left.push(self.radius());
        if push.length() < MOVE_DEAD_ZONE { Vec2::ZERO } else { push }
    }

    /// The aim while the right stick is pushed far enough to fire.
    pub fn firing_aim(&self) -> Option<Vec2> {
        let push = self.right.push(self.radius());
        (push.length() >= FIRE_DEAD_ZONE).then_some(push)
    }

    /// Both sticks, held ones where the thumbs are and idle ones at rest in
    /// the lower corners. Empty until the screen has been touched.
    pub fn visuals(&self) -> [Option<StickVisual>; 2] {
        if !self.used {
            return [None, None];
        }
        let radius = self.radius();
        let rest = |share: f32| Vec2::new(self.screen.x * share, self.screen.y - radius * 1.8);
        let visual = |stick: &Stick, home: Vec2| {
            let held = stick.finger.is_some();
            let base = if held { stick.origin } else { home };
            let knob = if held { base + (stick.at - base).clamp_length_max(radius) } else { base };
            Some(StickVisual { base, knob, radius, held })
        };
        [visual(&self.left, rest(0.15)), visual(&self.right, rest(0.85))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sticks() -> TouchSticks {
        TouchSticks { screen: Vec2::new(2000.0, 1000.0), ..TouchSticks::default() }
    }

    #[test]
    fn the_left_half_moves_and_the_right_half_aims_and_fires() {
        let mut touch = sticks();
        let reach = touch.radius();
        touch.start(1, Vec2::new(300.0, 700.0));
        touch.moved(1, Vec2::new(300.0 + reach, 700.0));
        touch.start(2, Vec2::new(1600.0, 600.0));
        assert_eq!(touch.movement(), Vec2::X);
        assert_eq!(touch.firing_aim(), None, "a resting thumb does not fire");
        touch.moved(2, Vec2::new(1600.0, 600.0 - reach));
        assert_eq!(touch.firing_aim(), Some(Vec2::Y), "screen up is world up");
        touch.end(2);
        assert_eq!(touch.firing_aim(), None);
        assert_eq!(touch.movement(), Vec2::X);
    }

    #[test]
    fn a_thumb_past_the_reach_drags_the_base_along() {
        let mut touch = sticks();
        let reach = touch.radius();
        touch.start(1, Vec2::new(300.0, 500.0));
        touch.moved(1, Vec2::new(300.0 + 3.0 * reach, 500.0));
        // Turning back answers at once instead of crossing the old centre first.
        touch.moved(1, Vec2::new(300.0 + reach, 500.0));
        assert!(touch.movement().x < 0.0);
    }

    #[test]
    fn a_second_finger_on_a_held_side_is_ignored() {
        let mut touch = sticks();
        touch.start(1, Vec2::new(300.0, 500.0));
        touch.start(2, Vec2::new(400.0, 500.0));
        touch.moved(2, Vec2::new(900.0, 500.0));
        assert_eq!(touch.movement(), Vec2::ZERO);
        assert!(touch.visuals()[0].is_some_and(|stick| stick.held));
    }
}
