//! Keys and gamepads to game actions.
//!
//! The window never looks at a `KeyCode` itself: it hands key events to
//! [`InputState`], which asks [`Bindings`] what they mean. The simulation never
//! sees a device at all, only the [`sim::Input`](crate::sim::Input) built here once
//! per step. Bindings are data so that a rebinding screen can edit them later.

use crystal_engine::glam::Vec2;
use winit::keyboard::KeyCode;

use crate::sim;

/// Everything a key or button can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    ToggleFullscreen,
    Quit,
}

impl Action {
    const COUNT: usize = 6;

    fn index(self) -> usize {
        self as usize
    }

    /// Actions that happen once per press rather than while held.
    pub fn is_command(self) -> bool {
        matches!(self, Action::ToggleFullscreen | Action::Quit)
    }
}

/// Which keys trigger which actions. Several keys may share an action.
#[derive(Clone, Debug)]
pub struct Bindings {
    keys: Vec<(KeyCode, Action)>,
}

impl Default for Bindings {
    /// WASD and the arrow keys, as in the Unity build's input actions.
    fn default() -> Self {
        Self {
            keys: vec![
                (KeyCode::KeyW, Action::MoveUp),
                (KeyCode::KeyS, Action::MoveDown),
                (KeyCode::KeyA, Action::MoveLeft),
                (KeyCode::KeyD, Action::MoveRight),
                (KeyCode::ArrowUp, Action::MoveUp),
                (KeyCode::ArrowDown, Action::MoveDown),
                (KeyCode::ArrowLeft, Action::MoveLeft),
                (KeyCode::ArrowRight, Action::MoveRight),
                (KeyCode::F11, Action::ToggleFullscreen),
                (KeyCode::Escape, Action::Quit),
            ],
        }
    }
}

impl Bindings {
    pub fn action(&self, key: KeyCode) -> Option<Action> {
        self.keys
            .iter()
            .find(|(bound, _)| *bound == key)
            .map(|(_, action)| *action)
    }
}

/// Stick positions inside this radius count as centred.
const STICK_DEADZONE: f32 = 0.2;

/// What is held right now, across keyboard and gamepad.
#[derive(Default)]
pub struct InputState {
    /// How many keys bound to each action are down. A count, not a flag, so that
    /// releasing W while Up is still held keeps moving.
    held: [u8; Action::COUNT],
    /// Keys currently down, to ignore auto-repeat and unmatched releases.
    down: Vec<KeyCode>,
    /// The gamepad's contribution to movement, already dead-zoned.
    pad_movement: Vec2,
}

impl InputState {
    /// Feeds one key event. Returns the action when this press is a one-shot
    /// command (quit, fullscreen) that the window should act on.
    pub fn key(&mut self, bindings: &Bindings, key: KeyCode, pressed: bool) -> Option<Action> {
        let was_down = self.down.contains(&key);
        if pressed == was_down {
            return None;
        }
        if pressed {
            self.down.push(key);
        } else {
            self.down.retain(|held| *held != key);
        }

        let action = bindings.action(key)?;
        let count = &mut self.held[action.index()];
        *count = if pressed { count.saturating_add(1) } else { count.saturating_sub(1) };
        (pressed && action.is_command()).then_some(action)
    }

    /// Forgets every held key. Call when the window loses focus: the releases
    /// will never arrive.
    pub fn release_all(&mut self) {
        self.held = [0; Action::COUNT];
        self.down.clear();
        self.pad_movement = Vec2::ZERO;
    }

    pub fn set_pad_movement(&mut self, stick: Vec2) {
        self.pad_movement = apply_deadzone(stick);
    }

    fn is_held(&self, action: Action) -> bool {
        self.held[action.index()] > 0
    }

    /// The input for the next simulation step.
    pub fn sim_input(&self) -> sim::Input {
        let axis = |positive, negative| {
            (self.is_held(positive) as i8 - self.is_held(negative) as i8) as f32
        };
        let keys = Vec2::new(
            axis(Action::MoveRight, Action::MoveLeft),
            axis(Action::MoveUp, Action::MoveDown),
        );
        // Keys give a full-speed direction; the stick can also walk slowly.
        let movement = if keys != Vec2::ZERO {
            keys.normalize()
        } else {
            self.pad_movement
        };
        sim::Input { movement }
    }
}

/// Zero inside the dead zone, then rescaled so movement starts from zero at its
/// edge instead of jumping, and never longer than 1.
fn apply_deadzone(stick: Vec2) -> Vec2 {
    let length = stick.length();
    if length <= STICK_DEADZONE {
        return Vec2::ZERO;
    }
    let scaled = ((length - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).min(1.0);
    stick / length * scaled
}

/// Connected gamepads. Missing controller support on a machine is not an error:
/// the game simply runs on the keyboard.
pub struct Gamepads {
    gilrs: Option<gilrs::Gilrs>,
}

impl Gamepads {
    pub fn new() -> Self {
        let gilrs = match gilrs::Gilrs::new() {
            Ok(gilrs) => Some(gilrs),
            Err(error) => {
                log::warn!("gamepads are unavailable: {error}");
                None
            }
        };
        Self { gilrs }
    }

    /// Reads the pads and stores their movement in `state`. Call once per frame.
    pub fn poll(&mut self, state: &mut InputState) {
        use gilrs::{Axis, Button};

        let Some(gilrs) = self.gilrs.as_mut() else {
            return;
        };
        // gilrs only updates its cached pad state as events are drained.
        while gilrs.next_event().is_some() {}

        // Whichever pad is pushed furthest wins, so any connected pad can play.
        let mut movement = Vec2::ZERO;
        for (_, pad) in gilrs.gamepads() {
            let stick = Vec2::new(pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
            let dpad = Vec2::new(
                (pad.is_pressed(Button::DPadRight) as i8 - pad.is_pressed(Button::DPadLeft) as i8) as f32,
                (pad.is_pressed(Button::DPadUp) as i8 - pad.is_pressed(Button::DPadDown) as i8) as f32,
            );
            let candidate = if dpad != Vec2::ZERO { dpad.normalize() } else { stick };
            if candidate.length_squared() > movement.length_squared() {
                movement = candidate;
            }
        }
        state.set_pad_movement(movement);
    }
}

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(state: &mut InputState, key: KeyCode) -> Option<Action> {
        state.key(&Bindings::default(), key, true)
    }

    fn release(state: &mut InputState, key: KeyCode) {
        state.key(&Bindings::default(), key, false);
    }

    #[test]
    fn diagonals_are_unit_length() {
        let mut state = InputState::default();
        press(&mut state, KeyCode::KeyW);
        press(&mut state, KeyCode::KeyD);
        let movement = state.sim_input().movement;
        assert!((movement.length() - 1.0).abs() < 1e-6);
        assert!(movement.x > 0.0 && movement.y > 0.0);
    }

    #[test]
    fn opposite_keys_cancel() {
        let mut state = InputState::default();
        press(&mut state, KeyCode::KeyA);
        press(&mut state, KeyCode::KeyD);
        assert_eq!(state.sim_input().movement, Vec2::ZERO);
    }

    #[test]
    fn two_keys_for_one_action_need_both_released() {
        let mut state = InputState::default();
        press(&mut state, KeyCode::KeyW);
        press(&mut state, KeyCode::ArrowUp);
        // Auto-repeat must not count as a second press.
        press(&mut state, KeyCode::KeyW);
        release(&mut state, KeyCode::KeyW);
        assert_eq!(state.sim_input().movement, Vec2::new(0.0, 1.0));
        release(&mut state, KeyCode::ArrowUp);
        assert_eq!(state.sim_input().movement, Vec2::ZERO);
    }

    #[test]
    fn commands_fire_once_per_press() {
        let mut state = InputState::default();
        assert_eq!(press(&mut state, KeyCode::Escape), Some(Action::Quit));
        assert_eq!(press(&mut state, KeyCode::Escape), None, "held key repeating");
        assert_eq!(press(&mut state, KeyCode::KeyW), None, "movement is not a command");
    }

    #[test]
    fn stick_deadzone_and_keyboard_priority() {
        let mut state = InputState::default();
        state.set_pad_movement(Vec2::new(0.1, 0.1));
        assert_eq!(state.sim_input().movement, Vec2::ZERO);

        state.set_pad_movement(Vec2::new(0.6, 0.0));
        let walk = state.sim_input().movement;
        assert!(walk.x > 0.0 && walk.x < 0.6, "rescaled from the dead zone edge: {walk}");

        state.set_pad_movement(Vec2::new(2.0, 0.0));
        assert_eq!(state.sim_input().movement, Vec2::new(1.0, 0.0));

        press(&mut state, KeyCode::KeyA);
        assert_eq!(state.sim_input().movement, Vec2::new(-1.0, 0.0));
    }

    #[test]
    fn losing_focus_releases_everything() {
        let mut state = InputState::default();
        press(&mut state, KeyCode::KeyW);
        state.release_all();
        assert_eq!(state.sim_input().movement, Vec2::ZERO);
        // The key can be pressed again afterwards.
        press(&mut state, KeyCode::KeyW);
        assert_eq!(state.sim_input().movement, Vec2::new(0.0, 1.0));
    }
}
