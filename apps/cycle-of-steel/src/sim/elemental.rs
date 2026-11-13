//! Being frozen, wet or in lava: Unity's `ElementalEffectHandler`.
//!
//! Only the enemies and the player carry the handler in Unity; anything else,
//! the hub's test dummy included, is untouched by elemental bullets. So a target
//! holds an [`Elemental`] only if its Unity object had the script.
//!
//! What is here: freezing, which one bullet applies (`Bullet_Pistol_2`, fired by
//! `pistol_2`, `smg_1` and `minigun_1`), and water and lava, which floor grates
//! (`EffectGrate`) apply to whatever stands on them. Water doubles the body's
//! drag, so it wades; lava takes 2 off it, so it hurries. Each lasts while the
//! body stands there and wears off a moment after it leaves. In Unity the player
//! has no handler, so only enemies are affected. What is not here, and why:
//! - **Burning.** No prefab in the Unity project carries `Bullet_Burn`, and the
//!   handler's burn could not hurt anyway: it damages when
//!   `burnTimer % burnSpeed == 0`, which never holds for a timer counted down
//!   from 5 in steps of 0.1 in single precision (see the test below). It is left
//!   out rather than ported as code that does nothing.

/// A handler refuses a freeze that would take it past this
/// (`maxFreezeEffect`, 20 on every prefab).
const MAX_FREEZE: f32 = 20.0;
/// How much freeze wears off each step. Unity takes 0.1 off every rendered
/// frame, so its thaw depends on the frame rate; one step here stands for one
/// frame at 60 frames a second.
const THAW_PER_STEP: f32 = 0.1;

/// What lava takes off the body's drag (`ElementalEffectHandler.lavaStrength`).
const LAVA_STRENGTH: f32 = 2.0;
/// How much water and lava timers fall each frame, like the thaw.
const TIMER_PER_STEP: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elemental {
    /// Unity's `freezeStrength`: what freezing has added to the body's drag.
    freeze: f32,
    /// `waterTimer` and `lavaTimer`, counted down 0.1 a frame.
    water_timer: f32,
    lava_timer: f32,
    /// What water has multiplied the drag by, and lava added to it. Unity edits
    /// the body's drag in place; these record the same edits.
    water_factor: f32,
    lava_added: f32,
}

impl Default for Elemental {
    fn default() -> Self {
        Self { freeze: 0.0, water_timer: 0.0, lava_timer: 0.0, water_factor: 1.0, lava_added: 0.0 }
    }
}

impl Elemental {
    /// `SetFreezeEffect`: freezes stack, but one that would pass the cap is
    /// ignored altogether rather than topped up to it.
    /// It ends lava first.
    pub fn freeze(&mut self, strength: f32) {
        self.end_lava();
        if self.freeze + strength <= MAX_FREEZE {
            self.freeze += strength;
        }
    }

    /// `SetWaterEffect`: ends lava, doubles the drag if the water timer is at 0,
    /// and tops the timer up to `duration`.
    pub fn wet(&mut self, duration: f32) {
        self.end_lava();
        if self.water_timer == 0.0 {
            self.water_factor *= 2.0;
        }
        if self.water_timer < duration {
            self.water_timer = duration;
        }
    }

    /// `SetLavaEffect`: ends water and freezing, takes 2 off the drag if the lava
    /// timer is at 0, and tops the timer up to `duration`.
    pub fn burn_in_lava(&mut self, duration: f32) {
        self.end_water();
        // `EndFreezeEffect` gives back what freezing added.
        self.freeze = self.freeze.min(0.0);
        if self.lava_timer == 0.0 {
            self.lava_added -= LAVA_STRENGTH;
        }
        if self.lava_timer < duration {
            self.lava_timer = duration;
        }
    }

    /// `EndWaterEffect`: only while the timer runs.
    fn end_water(&mut self) {
        if self.water_timer > 0.0 {
            self.water_factor /= 2.0;
            self.water_timer = 0.0;
        }
    }

    /// `EndLavaEffect`: only while the timer runs.
    fn end_lava(&mut self) {
        if self.lava_timer > 0.0 {
            self.lava_added += LAVA_STRENGTH;
            self.lava_timer = 0.0;
        }
    }

    /// One frame of the handler's `Update`. A timer that has gone below zero
    /// stops its effect; one above zero counts down. As in Unity, single
    /// precision decides where a countdown from 3 in steps of 0.1 lands.
    pub fn step(&mut self) {
        if self.water_timer < 0.0 {
            self.water_factor /= 2.0;
            self.water_timer = 0.0;
        } else if self.water_timer > 0.0 {
            self.water_timer -= TIMER_PER_STEP;
        }
        if self.lava_timer < 0.0 {
            self.lava_added += LAVA_STRENGTH;
            self.lava_timer = 0.0;
        } else if self.lava_timer > 0.0 {
            self.lava_timer -= TIMER_PER_STEP;
        }
        if self.freeze > 0.0 {
            self.freeze -= THAW_PER_STEP;
        }
    }

    /// The body's linear drag given its prefab's `base`. Unity edits one drag
    /// value in place, so the order effects came in can shift it slightly; here
    /// water's doubling applies to the rest.
    pub fn drag(&self, base: f32) -> f32 {
        (base + self.freeze + self.lava_added) * self.water_factor
    }

    /// Unity tints a frozen sprite (0.3, 1, 1) while this holds.
    pub fn is_frozen(&self) -> bool {
        self.freeze > 0.0
    }

    /// What to add to the body's linear drag: this is how freezing slows things.
    ///
    /// It can rest a little below zero. Unity takes its 0.1 off whenever the
    /// strength is above zero, so the last step usually overshoots, and the drag
    /// it took with it is never given back: a thawed enemy is very slightly
    /// quicker than one never frozen. Kept, since it is what Unity does.
    pub fn added_drag(&self) -> f32 {
        self.freeze
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_doubles_drag_while_stood_in_and_wears_off_after() {
        let mut elemental = Elemental::default();
        for _ in 0..10 {
            elemental.wet(3.0);
            elemental.step();
        }
        assert_eq!(elemental.drag(20.0), 40.0);
        let mut steps = 0;
        while elemental.drag(20.0) != 20.0 && steps < 100 {
            elemental.step();
            steps += 1;
        }
        // 3 in steps of 0.1 runs out after about 30 frames, half a second.
        assert!((29..=33).contains(&steps), "{steps}");
    }

    #[test]
    fn lava_takes_two_off_and_ends_water_and_freezing() {
        let mut elemental = Elemental::default();
        elemental.wet(3.0);
        elemental.freeze(5.0);
        elemental.burn_in_lava(3.0);
        assert_eq!(elemental.drag(20.0), 18.0);
        elemental.wet(3.0);
        assert_eq!(elemental.drag(20.0), 40.0, "water ends lava again");
    }

    #[test]
    fn freezing_stacks_up_to_the_cap_and_wears_off() {
        let mut elemental = Elemental::default();
        assert!(!elemental.is_frozen());
        // `Bullet_Pistol_2`'s freezeStrength is 5: four fit under the cap of 20.
        for _ in 0..4 {
            elemental.freeze(5.0);
        }
        assert_eq!(elemental.added_drag(), 20.0);
        elemental.freeze(5.0);
        assert_eq!(elemental.added_drag(), 20.0, "a fifth is refused");

        // One freeze of 5 wears off at 0.1 a step: about 50 steps, under a second.
        let mut elemental = Elemental::default();
        elemental.freeze(5.0);
        for _ in 0..45 {
            elemental.step();
        }
        assert!(elemental.is_frozen());
        for _ in 0..10 {
            elemental.step();
        }
        assert!(!elemental.is_frozen());
        // The overshoot Unity leaves behind: no more than one step's worth.
        assert!(elemental.added_drag() <= 0.0 && elemental.added_drag() > -THAW_PER_STEP);
        let rest = elemental.added_drag();
        elemental.step();
        assert_eq!(elemental.added_drag(), rest, "thawed, it stays put");
    }

    /// Why burning is not ported: Unity's own arithmetic, in the same single
    /// precision, never finds a frame on which to hurt.
    #[test]
    fn unitys_burn_never_lands_on_a_damage_frame() {
        let (max_burn_time, burn_speed) = (5.0_f32, 0.05_f32);
        let mut burn_timer = max_burn_time;
        let mut frames = 0;
        while burn_timer > 0.0 {
            burn_timer -= 0.1;
            frames += 1;
            assert_ne!(burn_timer % burn_speed, 0.0, "frame {frames}");
        }
        assert_eq!(frames, 51);
    }
}
