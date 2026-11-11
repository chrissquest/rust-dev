//! Keys and gamepads to game actions.
//!
//! The window never looks at a `KeyCode` itself: it hands key events to
//! [`InputState`], which asks [`Bindings`] what they mean. The simulation never
//! sees a device at all, only the [`sim::Input`](crate::sim::Input) built here once
//! per step. Bindings are data so that a rebinding screen can edit them later.

use crystal_engine::glam::Vec2;
use winit::event::TouchPhase;
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
    /// The on-screen stick's contribution to movement.
    touch_movement: Vec2,
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
        self.touch_movement = Vec2::ZERO;
    }

    pub fn set_touch_movement(&mut self, movement: Vec2) {
        self.touch_movement = movement.clamp_length_max(1.0);
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
        // Keys give a full-speed direction; the sticks can also walk slowly.
        let movement = if keys != Vec2::ZERO {
            keys.normalize()
        } else if self.touch_movement != Vec2::ZERO {
            self.touch_movement
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

/// The on-screen stick's radius, as a fraction of the screen's height.
const TOUCH_STICK_RADIUS: f32 = 0.14;
/// Drags shorter than this fraction of the radius do not move the player, so a
/// resting thumb does not creep.
const TOUCH_STICK_DEADZONE: f32 = 0.12;

/// Where to draw the on-screen stick, in pixels from the screen's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickVisual {
    /// Where the finger first went down: the middle of the stick.
    pub base: Vec2,
    /// The knob, held within `radius` of the base.
    pub knob: Vec2,
    pub radius: f32,
}

/// A floating virtual stick for movement.
///
/// The first finger to land on the left half of the screen becomes the stick: where
/// it lands is the centre, and dragging away from there is the direction. Other
/// fingers are ignored, and the right half is left free for aiming later.
#[derive(Default)]
pub struct TouchStick {
    /// The finger driving the stick: its id, where it landed and where it is now,
    /// in pixels from the top-left.
    held: Option<(u64, Vec2, Vec2)>,
}

impl TouchStick {
    /// Feeds one touch event. `position` and `screen` are in pixels.
    pub fn touch(&mut self, finger: u64, phase: TouchPhase, position: Vec2, screen: Vec2) {
        match phase {
            TouchPhase::Started => {
                if self.held.is_none() && position.x < screen.x * 0.5 {
                    self.held = Some((finger, position, position));
                }
            }
            TouchPhase::Moved => {
                if let Some((held, _, current)) = self.held.as_mut()
                    && *held == finger
                {
                    *current = position;
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.held.is_some_and(|(held, _, _)| held == finger) {
                    self.held = None;
                }
            }
        }
    }

    /// Lets go of the stick, e.g. when the app is sent to the background.
    pub fn release(&mut self) {
        self.held = None;
    }

    /// The movement the stick asks for: length 0..=1, y up.
    pub fn movement(&self, screen: Vec2) -> Vec2 {
        let Some((_, base, current)) = self.held else {
            return Vec2::ZERO;
        };
        let radius = stick_radius(screen);
        // Screen y grows downwards; the game's y grows upwards.
        let drag = Vec2::new(current.x - base.x, base.y - current.y) / radius;
        let length = drag.length();
        if length <= TOUCH_STICK_DEADZONE {
            return Vec2::ZERO;
        }
        let scaled = ((length - TOUCH_STICK_DEADZONE) / (1.0 - TOUCH_STICK_DEADZONE)).min(1.0);
        drag / length * scaled
    }

    /// Where to draw the stick, or `None` while no finger holds it.
    pub fn visual(&self, screen: Vec2) -> Option<StickVisual> {
        let (_, base, current) = self.held?;
        let radius = stick_radius(screen);
        Some(StickVisual {
            base,
            knob: base + (current - base).clamp_length_max(radius),
            radius,
        })
    }
}

fn stick_radius(screen: Vec2) -> f32 {
    (screen.y * TOUCH_STICK_RADIUS).max(1.0)
}

/// Connected gamepads. Missing controller support on a machine is not an error:
/// the game simply runs on the keyboard.
pub struct Gamepads {
    #[cfg(not(target_os = "android"))]
    gilrs: Option<gilrs::Gilrs>,
}

#[cfg(not(target_os = "android"))]
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

/// gilrs has no Android backend. Controllers there will come through Android's own
/// key and stick events; until that is wired up, Android plays by touch.
#[cfg(target_os = "android")]
impl Gamepads {
    pub fn new() -> Self {
        Self {}
    }

    pub fn poll(&mut self, _state: &mut InputState) {}
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

    const SCREEN: Vec2 = Vec2::new(2000.0, 1000.0);

    #[test]
    fn touch_stick_starts_on_the_left_half_only() {
        let mut stick = TouchStick::default();
        stick.touch(1, TouchPhase::Started, Vec2::new(1500.0, 500.0), SCREEN);
        assert_eq!(stick.visual(SCREEN), None, "right half is not the stick");

        stick.touch(2, TouchPhase::Started, Vec2::new(300.0, 700.0), SCREEN);
        assert_eq!(stick.movement(SCREEN), Vec2::ZERO, "no drag yet");
        assert!(stick.visual(SCREEN).is_some());
    }

    #[test]
    fn dragging_up_and_right_moves_up_and_right() {
        let mut stick = TouchStick::default();
        stick.touch(7, TouchPhase::Started, Vec2::new(300.0, 700.0), SCREEN);
        // Radius is 140 px. Screen y shrinks going up.
        stick.touch(7, TouchPhase::Moved, Vec2::new(370.0, 630.0), SCREEN);
        let movement = stick.movement(SCREEN);
        assert!(movement.x > 0.0 && movement.y > 0.0);
        assert!((movement.x - movement.y).abs() < 1e-6);
        assert!(movement.length() < 1.0, "a partial drag walks slower");

        // Far past the rim is still full speed, and the knob stays on the rim.
        stick.touch(7, TouchPhase::Moved, Vec2::new(300.0, 100.0), SCREEN);
        assert_eq!(stick.movement(SCREEN), Vec2::new(0.0, 1.0));
        let visual = stick.visual(SCREEN).unwrap();
        assert_eq!(visual.knob, Vec2::new(300.0, 700.0 - visual.radius));
    }

    #[test]
    fn only_the_holding_finger_moves_or_releases_the_stick() {
        let mut stick = TouchStick::default();
        stick.touch(1, TouchPhase::Started, Vec2::new(300.0, 700.0), SCREEN);
        stick.touch(1, TouchPhase::Moved, Vec2::new(440.0, 700.0), SCREEN);
        // A second finger elsewhere changes nothing, whatever it does.
        stick.touch(2, TouchPhase::Started, Vec2::new(100.0, 100.0), SCREEN);
        stick.touch(2, TouchPhase::Moved, Vec2::new(100.0, 900.0), SCREEN);
        stick.touch(2, TouchPhase::Ended, Vec2::new(100.0, 900.0), SCREEN);
        assert_eq!(stick.movement(SCREEN), Vec2::new(1.0, 0.0));

        stick.touch(1, TouchPhase::Ended, Vec2::new(440.0, 700.0), SCREEN);
        assert_eq!(stick.movement(SCREEN), Vec2::ZERO);
        assert_eq!(stick.visual(SCREEN), None);
    }

    #[test]
    fn touch_beats_the_pad_and_keys_beat_touch() {
        let mut state = InputState::default();
        state.set_pad_movement(Vec2::new(1.0, 0.0));
        state.set_touch_movement(Vec2::new(0.0, 0.5));
        assert_eq!(state.sim_input().movement, Vec2::new(0.0, 0.5));
        press(&mut state, KeyCode::KeyA);
        assert_eq!(state.sim_input().movement, Vec2::new(-1.0, 0.0));
    }
}
