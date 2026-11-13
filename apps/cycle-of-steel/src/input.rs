//! Keys, mouse, gamepads and touch to game actions.
//!
//! The window never looks at a `KeyCode` itself: it hands events to
//! [`InputState`], which asks [`Bindings`] what they mean. The simulation never
//! sees a device at all, only the [`sim::Input`](crate::sim::Input) built here.
//! Bindings are data so that a rebinding screen can edit them later.
//!
//! Two kinds of input come out. *Held* input (move, aim) is read fresh each
//! frame. *One-shot* input (pick up, change slot) is queued when the button goes
//! down and handed over exactly once, by [`InputState::take_sim_input`].
//! While a menu is open the same presses steer it instead
//! ([`InputState::take_menu_input`]): up and down move, interact chooses.

use crystal_engine::glam::Vec2;
use winit::event::TouchPhase;
use winit::keyboard::KeyCode;

use crate::hud::HudLayout;
use crate::sim;

/// Everything a key or button can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    /// Shoot in a direction. Holding one fires.
    AimUp,
    AimDown,
    AimLeft,
    AimRight,
    Interact,
    Slot1,
    Slot2,
    Slot3,
    Slot4,
    NextSlot,
    PreviousSlot,
    ToggleFullscreen,
    /// Show or hide the minimap.
    ToggleMap,
    Quit,
}

impl Action {
    const COUNT: usize = 18;

    fn index(self) -> usize {
        self as usize
    }

    /// Actions the window itself carries out, once per press.
    pub fn is_window_command(self) -> bool {
        matches!(self, Action::ToggleFullscreen | Action::ToggleMap | Action::Quit)
    }
}

/// Which keys trigger which actions. Several keys may share an action.
#[derive(Clone, Debug)]
pub struct Bindings {
    keys: Vec<(KeyCode, Action)>,
}

impl Default for Bindings {
    /// The Unity build's keys: WASD to move, arrows to shoot, space to interact,
    /// 1 to 4 for the inventory.
    fn default() -> Self {
        Self {
            keys: vec![
                (KeyCode::KeyW, Action::MoveUp),
                (KeyCode::KeyS, Action::MoveDown),
                (KeyCode::KeyA, Action::MoveLeft),
                (KeyCode::KeyD, Action::MoveRight),
                (KeyCode::ArrowUp, Action::AimUp),
                (KeyCode::ArrowDown, Action::AimDown),
                (KeyCode::ArrowLeft, Action::AimLeft),
                (KeyCode::ArrowRight, Action::AimRight),
                (KeyCode::Space, Action::Interact),
                (KeyCode::Enter, Action::Interact),
                (KeyCode::Digit1, Action::Slot1),
                (KeyCode::Digit2, Action::Slot2),
                (KeyCode::Digit3, Action::Slot3),
                (KeyCode::Digit4, Action::Slot4),
                (KeyCode::KeyE, Action::NextSlot),
                (KeyCode::KeyQ, Action::PreviousSlot),
                (KeyCode::F11, Action::ToggleFullscreen),
                // Unity's map key; the hub's floor says "Map (tab)".
                (KeyCode::Tab, Action::ToggleMap),
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
/// The aim stick fires once pushed past this, so a resting thumb does not shoot.
const AIM_DEADZONE: f32 = 0.35;

/// What is held right now and what has been pressed since last asked, across
/// keyboard, mouse, gamepad and touch.
#[derive(Default)]
pub struct InputState {
    /// How many keys bound to each action are down. A count, not a flag, so that
    /// releasing one of two keys bound to the same action keeps it held.
    held: [u8; Action::COUNT],
    /// Keys currently down, to ignore auto-repeat and unmatched releases.
    down: Vec<KeyCode>,
    /// The minimap was asked for: the window acts on it, not the simulation.
    map_requested: bool,

    /// The gamepad's sticks, already dead-zoned.
    pad_movement: Vec2,
    pad_aim: Option<Vec2>,
    /// The on-screen sticks.
    touch_movement: Vec2,
    touch_aim: Option<Vec2>,
    /// From the player towards the mouse cursor, while its button is held.
    mouse_aim: Option<Vec2>,

    /// One-shot requests not yet handed to the simulation.
    interact: bool,
    select_slot: Option<usize>,
    cycle_slot: i32,

    /// The same presses, for a menu.
    menu: MenuInput,
}

/// What was pressed for a menu since it was last asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuInput {
    /// Rows to move the highlight by: negative is up.
    pub moves: i32,
    /// Choose the highlighted row.
    pub choose: bool,
    /// Leave the menu.
    pub back: bool,
}

impl InputState {
    /// Feeds one key event. Returns the action when this press is something the
    /// window should act on (quit, fullscreen).
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
        if !pressed {
            return None;
        }
        self.press(action);
        action.is_window_command().then_some(action)
    }

    /// Queues what an action does at the moment it is pressed, if anything.
    pub fn press(&mut self, action: Action) {
        match action {
            Action::MoveUp | Action::AimUp => self.menu.moves -= 1,
            Action::MoveDown | Action::AimDown => self.menu.moves += 1,
            _ => {}
        }
        match action {
            Action::Interact => {
                self.interact = true;
                self.menu.choose = true;
            }
            Action::Slot1 => self.select_slot = Some(0),
            Action::Slot2 => self.select_slot = Some(1),
            Action::Slot3 => self.select_slot = Some(2),
            Action::Slot4 => self.select_slot = Some(3),
            Action::NextSlot => self.cycle_slot += 1,
            Action::PreviousSlot => self.cycle_slot -= 1,
            _ => {}
        }
    }

    /// Asks to leave a menu, from a pad's back button.
    pub fn press_back(&mut self) {
        self.menu.back = true;
    }

    /// The menu presses since the last call. Taken every frame, menu or not, so
    /// that a press made in play does not reach a menu opened by it.
    pub fn take_menu_input(&mut self) -> MenuInput {
        std::mem::take(&mut self.menu)
    }

    /// Queues a jump to an inventory slot, e.g. from a tap on it.
    pub fn select_slot(&mut self, slot: usize) {
        self.select_slot = Some(slot);
    }

    /// Asks for the minimap to be shown or hidden, from a key, a button or a tap.
    pub fn request_map(&mut self) {
        self.map_requested = true;
    }

    /// Whether the minimap was asked for since the last call.
    pub fn take_map_request(&mut self) -> bool {
        std::mem::take(&mut self.map_requested)
    }

    /// Forgets everything held. Call when the window loses focus: the releases
    /// will never arrive.
    pub fn release_all(&mut self) {
        self.held = [0; Action::COUNT];
        self.down.clear();
        self.pad_movement = Vec2::ZERO;
        self.pad_aim = None;
        self.touch_movement = Vec2::ZERO;
        self.touch_aim = None;
        self.mouse_aim = None;
    }

    pub fn set_pad(&mut self, movement: Vec2, aim: Vec2) {
        self.pad_movement = apply_deadzone(movement, STICK_DEADZONE);
        self.pad_aim = (aim.length() > AIM_DEADZONE).then_some(aim);
    }

    pub fn set_touch(&mut self, movement: Vec2, aim: Option<Vec2>) {
        self.touch_movement = movement.clamp_length_max(1.0);
        self.touch_aim = aim;
    }

    /// `Some` while the mouse button is held: the direction from the player to the cursor.
    pub fn set_mouse_aim(&mut self, aim: Option<Vec2>) {
        self.mouse_aim = aim;
    }

    fn is_held(&self, action: Action) -> bool {
        self.held[action.index()] > 0
    }

    fn axis(&self, positive: Action, negative: Action) -> f32 {
        (self.is_held(positive) as i8 - self.is_held(negative) as i8) as f32
    }

    /// The input for the next simulation step, without taking the one-shots.
    pub fn sim_input(&self) -> sim::Input {
        let move_keys = Vec2::new(
            self.axis(Action::MoveRight, Action::MoveLeft),
            self.axis(Action::MoveUp, Action::MoveDown),
        );
        // Keys give a full-speed direction; the sticks can also walk slowly.
        let movement = if move_keys != Vec2::ZERO {
            move_keys.normalize()
        } else if self.touch_movement != Vec2::ZERO {
            self.touch_movement
        } else {
            self.pad_movement
        };

        let aim_keys = Vec2::new(
            self.axis(Action::AimRight, Action::AimLeft),
            self.axis(Action::AimUp, Action::AimDown),
        );
        let aim = if aim_keys != Vec2::ZERO {
            Some(aim_keys)
        } else {
            self.mouse_aim.or(self.touch_aim).or(self.pad_aim)
        };

        sim::Input {
            movement,
            aim,
            interact: self.interact,
            select_slot: self.select_slot,
            cycle_slot: self.cycle_slot,
        }
    }

    /// The input for the next simulation step, handing over the queued one-shots.
    /// Call only when a step is about to run, or the presses are lost.
    pub fn take_sim_input(&mut self) -> sim::Input {
        let input = self.sim_input();
        self.interact = false;
        self.select_slot = None;
        self.cycle_slot = 0;
        input
    }
}

/// Zero inside the dead zone, then rescaled so the value starts from zero at its
/// edge instead of jumping, and never longer than 1.
fn apply_deadzone(stick: Vec2, deadzone: f32) -> Vec2 {
    let length = stick.length();
    if length <= deadzone {
        return Vec2::ZERO;
    }
    let scaled = ((length - deadzone) / (1.0 - deadzone)).min(1.0);
    stick / length * scaled
}

/// The on-screen sticks' radius, as a fraction of the screen's height.
const TOUCH_STICK_RADIUS: f32 = 0.14;
/// Drags shorter than this fraction of the radius do not count, so a resting
/// thumb does not creep.
const TOUCH_STICK_DEADZONE: f32 = 0.12;

/// Where to draw an on-screen stick, in pixels from the screen's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickVisual {
    /// Where the finger first went down: the middle of the stick.
    pub base: Vec2,
    /// The knob, held within `radius` of the base.
    pub knob: Vec2,
    pub radius: f32,
}

/// Which half of the screen a stick claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A floating virtual stick.
///
/// The first finger to land on the stick's half of the screen becomes the stick:
/// where it lands is the centre, and dragging away from there is the direction.
pub struct TouchStick {
    side: Side,
    /// The finger driving the stick: its id, where it landed and where it is now,
    /// in pixels from the top-left.
    held: Option<(u64, Vec2, Vec2)>,
}

impl TouchStick {
    pub fn new(side: Side) -> Self {
        Self { side, held: None }
    }

    /// Feeds one touch event. `position` and `screen` are in pixels.
    pub fn touch(&mut self, finger: u64, phase: TouchPhase, position: Vec2, screen: Vec2) {
        match phase {
            TouchPhase::Started => {
                let on_side = match self.side {
                    Side::Left => position.x < screen.x * 0.5,
                    Side::Right => position.x >= screen.x * 0.5,
                };
                if self.held.is_none() && on_side {
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

    /// How far the stick is pushed: length 0..=1, y up. Zero inside the dead zone.
    pub fn value(&self, screen: Vec2) -> Vec2 {
        let Some((_, base, current)) = self.held else {
            return Vec2::ZERO;
        };
        let radius = stick_radius(screen);
        // Screen y grows downwards; the game's y grows upwards.
        let drag = Vec2::new(current.x - base.x, base.y - current.y) / radius;
        apply_deadzone(drag, TOUCH_STICK_DEADZONE)
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

/// All touch input: a movement stick on the left, an aim-and-fire stick on the
/// right, and taps on the HUD.
pub struct TouchControls {
    pub movement: TouchStick,
    pub aim: TouchStick,
}

impl Default for TouchControls {
    fn default() -> Self {
        Self {
            movement: TouchStick::new(Side::Left),
            aim: TouchStick::new(Side::Right),
        }
    }
}

impl TouchControls {
    /// Feeds one touch event. A finger landing on a HUD control presses it and is
    /// not given to a stick; `can_interact` says whether the pick-up button is showing.
    pub fn touch(
        &mut self,
        state: &mut InputState,
        finger: u64,
        phase: TouchPhase,
        position: Vec2,
        screen: Vec2,
        can_interact: bool,
    ) {
        if phase == TouchPhase::Started {
            let hud = HudLayout::new(screen);
            if let Some(slot) = hud.slot_at(position) {
                state.select_slot(slot);
                return;
            }
            if hud.map_button.contains(position) {
                state.request_map();
                return;
            }
            if can_interact && hud.interact_contains(position) {
                state.press(Action::Interact);
                return;
            }
        }
        self.movement.touch(finger, phase, position, screen);
        self.aim.touch(finger, phase, position, screen);
    }

    pub fn release(&mut self) {
        self.movement.release();
        self.aim.release();
    }

    /// Writes the sticks' current values into `state`. Call once per frame.
    pub fn apply(&self, state: &mut InputState, screen: Vec2) {
        let aim = self.aim.value(screen);
        // Any push past the dead zone aims and fires; how far does not matter.
        state.set_touch(self.movement.value(screen), (aim != Vec2::ZERO).then_some(aim));
    }
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

    /// Reads the pads into `state`: sticks as held input, button presses as
    /// one-shots. Call once per frame.
    pub fn poll(&mut self, state: &mut InputState) {
        use gilrs::{Axis, Button, EventType};

        let Some(gilrs) = self.gilrs.as_mut() else {
            return;
        };
        // gilrs only updates its cached pad state as events are drained.
        while let Some(event) = gilrs.next_event() {
            if let EventType::ButtonPressed(button, _) = event.event {
                match button {
                    Button::South | Button::West => state.press(Action::Interact),
                    Button::RightTrigger => state.press(Action::NextSlot),
                    Button::LeftTrigger => state.press(Action::PreviousSlot),
                    Button::Select => state.request_map(),
                    // Only menus listen to these.
                    Button::DPadUp => state.press(Action::MoveUp),
                    Button::DPadDown => state.press(Action::MoveDown),
                    Button::East => state.press_back(),
                    _ => {}
                }
            }
        }

        // Whichever pad is pushed furthest wins, so any connected pad can play.
        let mut movement = Vec2::ZERO;
        let mut aim = Vec2::ZERO;
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
            let right = Vec2::new(pad.value(Axis::RightStickX), pad.value(Axis::RightStickY));
            if right.length_squared() > aim.length_squared() {
                aim = right;
            }
        }
        state.set_pad(movement, aim);
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
    fn auto_repeat_is_not_a_second_press() {
        let mut state = InputState::default();
        press(&mut state, KeyCode::KeyW);
        press(&mut state, KeyCode::KeyW);
        release(&mut state, KeyCode::KeyW);
        assert_eq!(state.sim_input().movement, Vec2::ZERO);
    }

    #[test]
    fn arrows_aim_and_fire_while_held() {
        let mut state = InputState::default();
        assert_eq!(state.sim_input().aim, None);
        press(&mut state, KeyCode::ArrowLeft);
        press(&mut state, KeyCode::ArrowUp);
        assert_eq!(state.sim_input().aim, Some(Vec2::new(-1.0, 1.0)));
        release(&mut state, KeyCode::ArrowLeft);
        release(&mut state, KeyCode::ArrowUp);
        assert_eq!(state.sim_input().aim, None);
    }

    #[test]
    fn aim_sources_rank_keys_then_mouse_then_touch_then_pad() {
        let mut state = InputState::default();
        state.set_pad(Vec2::ZERO, Vec2::new(0.0, 1.0));
        assert_eq!(state.sim_input().aim, Some(Vec2::new(0.0, 1.0)));
        state.set_touch(Vec2::ZERO, Some(Vec2::new(1.0, 0.0)));
        assert_eq!(state.sim_input().aim, Some(Vec2::new(1.0, 0.0)));
        state.set_mouse_aim(Some(Vec2::new(-3.0, 0.0)));
        assert_eq!(state.sim_input().aim, Some(Vec2::new(-3.0, 0.0)));
        press(&mut state, KeyCode::ArrowDown);
        assert_eq!(state.sim_input().aim, Some(Vec2::new(0.0, -1.0)));

        // A barely-nudged aim stick does not fire.
        let mut state = InputState::default();
        state.set_pad(Vec2::ZERO, Vec2::new(0.2, 0.0));
        assert_eq!(state.sim_input().aim, None);
    }

    #[test]
    fn one_shots_are_handed_over_once() {
        let mut state = InputState::default();
        press(&mut state, KeyCode::Space);
        press(&mut state, KeyCode::Digit3);
        press(&mut state, KeyCode::KeyE);
        press(&mut state, KeyCode::KeyE);
        release(&mut state, KeyCode::KeyE);

        let first = state.take_sim_input();
        assert!(first.interact);
        assert_eq!(first.select_slot, Some(2));
        assert_eq!(first.cycle_slot, 1, "the held key repeating is one press");

        let second = state.take_sim_input();
        assert!(!second.interact && second.select_slot.is_none() && second.cycle_slot == 0);
    }

    #[test]
    fn window_commands_fire_once_per_press() {
        let mut state = InputState::default();
        assert_eq!(press(&mut state, KeyCode::Escape), Some(Action::Quit));
        assert_eq!(press(&mut state, KeyCode::Escape), None, "held key repeating");
        assert_eq!(press(&mut state, KeyCode::Space), None, "the game's own actions are not the window's");
    }

    #[test]
    fn stick_deadzone_and_keyboard_priority() {
        let mut state = InputState::default();
        state.set_pad(Vec2::new(0.1, 0.1), Vec2::ZERO);
        assert_eq!(state.sim_input().movement, Vec2::ZERO);

        state.set_pad(Vec2::new(0.6, 0.0), Vec2::ZERO);
        let walk = state.sim_input().movement;
        assert!(walk.x > 0.0 && walk.x < 0.6, "rescaled from the dead zone edge: {walk}");

        state.set_pad(Vec2::new(2.0, 0.0), Vec2::ZERO);
        assert_eq!(state.sim_input().movement, Vec2::new(1.0, 0.0));

        state.set_touch(Vec2::new(0.0, 0.5), None);
        assert_eq!(state.sim_input().movement, Vec2::new(0.0, 0.5), "touch beats the pad");

        press(&mut state, KeyCode::KeyA);
        assert_eq!(state.sim_input().movement, Vec2::new(-1.0, 0.0), "keys beat touch");
    }

    #[test]
    fn losing_focus_releases_everything_held() {
        let mut state = InputState::default();
        press(&mut state, KeyCode::KeyW);
        state.set_mouse_aim(Some(Vec2::X));
        state.release_all();
        let input = state.sim_input();
        assert_eq!((input.movement, input.aim), (Vec2::ZERO, None));
        // The key can be pressed again afterwards.
        press(&mut state, KeyCode::KeyW);
        assert_eq!(state.sim_input().movement, Vec2::new(0.0, 1.0));
    }

    const SCREEN: Vec2 = Vec2::new(2000.0, 1000.0);

    #[test]
    fn each_stick_claims_its_own_half() {
        let mut controls = TouchControls::default();
        let mut state = InputState::default();
        controls.touch(&mut state, 1, TouchPhase::Started, Vec2::new(1500.0, 700.0), SCREEN, false);
        assert!(controls.movement.visual(SCREEN).is_none());
        assert!(controls.aim.visual(SCREEN).is_some());

        controls.touch(&mut state, 2, TouchPhase::Started, Vec2::new(300.0, 700.0), SCREEN, false);
        assert!(controls.movement.visual(SCREEN).is_some());
    }

    #[test]
    fn dragging_up_and_right_moves_up_and_right() {
        let mut stick = TouchStick::new(Side::Left);
        stick.touch(7, TouchPhase::Started, Vec2::new(300.0, 700.0), SCREEN);
        assert_eq!(stick.value(SCREEN), Vec2::ZERO, "no drag yet");
        // Radius is 140 px. Screen y shrinks going up.
        stick.touch(7, TouchPhase::Moved, Vec2::new(370.0, 630.0), SCREEN);
        let value = stick.value(SCREEN);
        assert!(value.x > 0.0 && value.y > 0.0);
        assert!((value.x - value.y).abs() < 1e-6);
        assert!(value.length() < 1.0, "a partial drag is a partial push");

        // Far past the rim is still a full push, and the knob stays on the rim.
        stick.touch(7, TouchPhase::Moved, Vec2::new(300.0, 100.0), SCREEN);
        assert_eq!(stick.value(SCREEN), Vec2::new(0.0, 1.0));
        let visual = stick.visual(SCREEN).unwrap();
        assert_eq!(visual.knob, Vec2::new(300.0, 700.0 - visual.radius));
    }

    #[test]
    fn only_the_holding_finger_moves_or_releases_a_stick() {
        let mut stick = TouchStick::new(Side::Left);
        stick.touch(1, TouchPhase::Started, Vec2::new(300.0, 700.0), SCREEN);
        stick.touch(1, TouchPhase::Moved, Vec2::new(440.0, 700.0), SCREEN);
        // A second finger elsewhere changes nothing, whatever it does.
        stick.touch(2, TouchPhase::Started, Vec2::new(100.0, 100.0), SCREEN);
        stick.touch(2, TouchPhase::Moved, Vec2::new(100.0, 900.0), SCREEN);
        stick.touch(2, TouchPhase::Ended, Vec2::new(100.0, 900.0), SCREEN);
        assert_eq!(stick.value(SCREEN), Vec2::new(1.0, 0.0));

        stick.touch(1, TouchPhase::Ended, Vec2::new(440.0, 700.0), SCREEN);
        assert_eq!(stick.value(SCREEN), Vec2::ZERO);
        assert_eq!(stick.visual(SCREEN), None);
    }

    #[test]
    fn the_aim_stick_fires_once_dragged_and_stops_when_released() {
        let mut controls = TouchControls::default();
        let mut state = InputState::default();
        controls.touch(&mut state, 1, TouchPhase::Started, Vec2::new(1500.0, 700.0), SCREEN, false);
        controls.apply(&mut state, SCREEN);
        assert_eq!(state.sim_input().aim, None, "a resting thumb does not shoot");

        controls.touch(&mut state, 1, TouchPhase::Moved, Vec2::new(1500.0, 600.0), SCREEN, false);
        controls.apply(&mut state, SCREEN);
        let aim = state.sim_input().aim.expect("dragged up: firing");
        assert!(aim.y > 0.0 && aim.x == 0.0);

        controls.touch(&mut state, 1, TouchPhase::Ended, Vec2::new(1500.0, 600.0), SCREEN, false);
        controls.apply(&mut state, SCREEN);
        assert_eq!(state.sim_input().aim, None);
    }

    #[test]
    fn taps_on_the_hud_press_it_instead_of_starting_a_stick() {
        let hud = HudLayout::new(SCREEN);
        let mut controls = TouchControls::default();
        let mut state = InputState::default();

        controls.touch(&mut state, 1, TouchPhase::Started, hud.slots[1].center(), SCREEN, false);
        assert!(controls.movement.visual(SCREEN).is_none(), "the slot took the tap");
        assert_eq!(state.take_sim_input().select_slot, Some(1));

        // The pick-up button only exists while there is something to pick up.
        controls.touch(&mut state, 2, TouchPhase::Started, hud.interact_center, SCREEN, false);
        assert!(!state.take_sim_input().interact);
        assert!(controls.aim.visual(SCREEN).is_some(), "with no button there, it is the aim stick");
        controls.release();

        controls.touch(&mut state, 3, TouchPhase::Started, hud.interact_center, SCREEN, true);
        assert!(state.take_sim_input().interact);
        assert!(controls.aim.visual(SCREEN).is_none());
    }
}
