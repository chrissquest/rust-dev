//! Being frozen: Unity's `ElementalEffectHandler`, as far as the game uses it.
//!
//! Only the enemies and the player carry the handler in Unity; anything else,
//! the hub's test dummy included, is untouched by elemental bullets. So a target
//! holds an [`Elemental`] only if its Unity object had the script.
//!
//! What is here is freezing, which one bullet applies (`Bullet_Pistol_2`, fired
//! by `pistol_2`, `smg_1` and `minigun_1`). What is not, and why:
//! - **Burning.** No prefab in the Unity project carries `Bullet_Burn`, and the
//!   handler's burn could not hurt anyway: it damages when
//!   `burnTimer % burnSpeed == 0`, which never holds for a timer counted down
//!   from 5 in steps of 0.1 in single precision (see the test below). It is left
//!   out rather than ported as code that does nothing.
//! - **Water and lava** come from floor grates (`EffectGrate.cs`), which arrive
//!   with room objects.

/// A handler refuses a freeze that would take it past this
/// (`maxFreezeEffect`, 20 on every prefab).
const MAX_FREEZE: f32 = 20.0;
/// How much freeze wears off each step. Unity takes 0.1 off every rendered
/// frame, so its thaw depends on the frame rate; one step here stands for one
/// frame at 60 frames a second.
const THAW_PER_STEP: f32 = 0.1;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Elemental {
    /// Unity's `freezeStrength`: what freezing has added to the body's drag.
    freeze: f32,
}

impl Elemental {
    /// `SetFreezeEffect`: freezes stack, but one that would pass the cap is
    /// ignored altogether rather than topped up to it.
    pub fn freeze(&mut self, strength: f32) {
        if self.freeze + strength <= MAX_FREEZE {
            self.freeze += strength;
        }
    }

    /// One frame of the handler's `Update`.
    pub fn step(&mut self) {
        if self.freeze > 0.0 {
            self.freeze -= THAW_PER_STEP;
        }
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
