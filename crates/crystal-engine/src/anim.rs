//! Animation clips sampled on the CPU.
//!
//! A viewer this size does not need GPU skinning *evaluation*: the joint
//! matrices are small in number and recomputed once per frame, which keeps the
//! shader to a weighted sum of four matrices and keeps the sampling code
//! testable without a GPU.

use crate::math::{Quat, Vec3};
use crate::scene::Transform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    /// Hold the previous key until the next one.
    Step,
}

#[derive(Clone, Debug)]
pub enum Values {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub node: usize,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    pub values: Values,
}

#[derive(Clone, Debug, Default)]
pub struct Animation {
    pub name: String,
    pub channels: Vec<Channel>,
    pub duration: f32,
}

impl Animation {
    /// Writes this clip's pose at `time` over `pose`, leaving untouched nodes
    /// at whatever the rest pose had.
    pub fn apply(&self, time: f32, pose: &mut [Transform]) {
        let time = if self.duration > 0.0 {
            time.clamp(0.0, self.duration)
        } else {
            0.0
        };

        for channel in &self.channels {
            let Some(transform) = pose.get_mut(channel.node) else {
                continue;
            };
            channel.apply(time, transform);
        }
    }
}

impl Channel {
    fn apply(&self, time: f32, transform: &mut Transform) {
        let Some((first, second, alpha)) = self.keys(time) else {
            return;
        };

        match &self.values {
            Values::Translation(values) => {
                let a = values.get(first).copied().unwrap_or(Vec3::ZERO);
                let b = values.get(second).copied().unwrap_or(a);
                transform.translation = if self.interpolation == Interpolation::Step {
                    a
                } else {
                    a.lerp(b, alpha)
                };
            }
            Values::Rotation(values) => {
                let a = values.get(first).copied().unwrap_or(Quat::IDENTITY);
                let b = values.get(second).copied().unwrap_or(a);
                transform.rotation = if self.interpolation == Interpolation::Step {
                    a
                } else {
                    a.slerp(b, alpha)
                };
            }
            Values::Scale(values) => {
                let a = values.get(first).copied().unwrap_or(Vec3::ONE);
                let b = values.get(second).copied().unwrap_or(a);
                transform.scale = if self.interpolation == Interpolation::Step {
                    a
                } else {
                    a.lerp(b, alpha)
                };
            }
        }
    }

    /// The two keys bracketing `time` and how far between them it sits.
    fn keys(&self, time: f32) -> Option<(usize, usize, f32)> {
        if self.times.is_empty() {
            return None;
        }
        if self.times.len() == 1 {
            return Some((0, 0, 0.0));
        }

        let next = self.times.partition_point(|&key| key <= time);
        let first = next.saturating_sub(1).min(self.times.len() - 1);
        let second = (first + 1).min(self.times.len() - 1);

        let span = self.times[second] - self.times[first];
        let alpha = if span > f32::EPSILON {
            ((time - self.times[first]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };

        Some((first, second, alpha))
    }
}

/// Which clip is playing and where it is in its loop.
#[derive(Clone, Debug, Default)]
pub struct Playback {
    pub clip: usize,
    pub time: f32,
    pub paused: bool,
    pub speed: f32,
}

impl Playback {
    pub fn new() -> Self {
        Self {
            clip: 0,
            time: 0.0,
            paused: false,
            speed: 1.0,
        }
    }

    pub fn advance(&mut self, delta_seconds: f32, clips: &[Animation]) {
        if self.paused {
            return;
        }
        let Some(clip) = clips.get(self.clip) else {
            return;
        };
        if clip.duration <= 0.0 {
            return;
        }
        self.time = (self.time + delta_seconds * self.speed).rem_euclid(clip.duration);
    }

    pub fn next_clip(&mut self, clips: &[Animation]) {
        if clips.is_empty() {
            return;
        }
        self.clip = (self.clip + 1) % clips.len();
        self.time = 0.0;
    }

    pub fn previous_clip(&mut self, clips: &[Animation]) {
        if clips.is_empty() {
            return;
        }
        self.clip = (self.clip + clips.len() - 1) % clips.len();
        self.time = 0.0;
    }

    pub fn restart(&mut self) {
        self.time = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(values: Values, times: Vec<f32>) -> Channel {
        Channel {
            node: 0,
            interpolation: Interpolation::Linear,
            times,
            values,
        }
    }

    #[test]
    fn clamps_before_first_key() {
        let channel = channel(
            Values::Translation(vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)]),
            vec![0.0, 1.0],
        );
        let mut transform = Transform::default();
        channel.apply(-5.0, &mut transform);
        assert_eq!(transform.translation, Vec3::ZERO);
    }

    #[test]
    fn clamps_after_last_key() {
        let channel = channel(
            Values::Translation(vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)]),
            vec![0.0, 1.0],
        );
        let mut transform = Transform::default();
        channel.apply(9.0, &mut transform);
        assert_eq!(transform.translation, Vec3::new(10.0, 0.0, 0.0));
    }

    #[test]
    fn interpolates_between_keys() {
        let channel = channel(
            Values::Translation(vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)]),
            vec![0.0, 1.0],
        );
        let mut transform = Transform::default();
        channel.apply(0.25, &mut transform);
        assert!((transform.translation.x - 2.5).abs() < 1e-5);
    }

    #[test]
    fn step_holds_previous_value() {
        let mut channel = channel(
            Values::Translation(vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)]),
            vec![0.0, 1.0],
        );
        channel.interpolation = Interpolation::Step;
        let mut transform = Transform::default();
        channel.apply(0.9, &mut transform);
        assert_eq!(transform.translation, Vec3::ZERO);
    }

    #[test]
    fn playback_loops() {
        let clips = vec![Animation {
            name: String::from("clip"),
            channels: Vec::new(),
            duration: 1.0,
        }];
        let mut playback = Playback::new();
        playback.advance(1.25, &clips);
        assert!((playback.time - 0.25).abs() < 1e-5);
    }
}
