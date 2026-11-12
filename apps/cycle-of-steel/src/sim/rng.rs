//! The simulation's random numbers.
//!
//! A small deterministic generator owned by the world, so that the same seed and
//! the same inputs always give the same game. Never use the system's randomness
//! inside `sim`: replays and tests depend on this being the only source.

/// xorshift32. Plenty for spread angles and loot rolls.
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        // The all-zero state is the one xorshift can never leave.
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// Uniform in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        // The top 24 bits fit an f32 exactly.
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform between `low` and `high`.
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    /// True with the given probability; never for 0, always for 1 or more.
    pub fn chance(&mut self, probability: f32) -> bool {
        self.unit() < probability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_numbers_and_all_in_range() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            let value = a.unit();
            assert_eq!(value, b.unit());
            assert!((0.0..1.0).contains(&value));
        }
        assert_ne!(Rng::new(1).unit(), Rng::new(2).unit());
    }

    #[test]
    fn ranges_and_chances_respect_their_limits() {
        let mut rng = Rng::new(0);
        for _ in 0..1000 {
            let value = rng.range(-20.0, 20.0);
            assert!((-20.0..=20.0).contains(&value));
            assert!(!rng.chance(0.0));
            assert!(rng.chance(1.0));
        }
    }
}
