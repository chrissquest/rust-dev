//! Keyboard actions and their (rebindable) keys.
//!
//! Every keyboard command is an [`Action`]; [`Bindings`] maps key chords to actions.
//! The window never matches on `KeyCode` itself, so a future rebinding popup only
//! has to edit a `Bindings` and save it - nothing else changes.
//!
//! Bindings are stored in the settings file as *overrides*: action name -> list of
//! chords, e.g. `"focus_cursor": ["KeyF"]` or `"quit": ["Escape", "Ctrl+KeyQ"]`.
//! An action that is missing from the file keeps its default key, so actions added
//! in later versions get their defaults instead of being unbound; an empty list
//! deliberately unbinds one.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use winit::keyboard::{KeyCode, ModifiersState};

/// Everything a key can do in the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Quit,
    /// Orbit around whatever is under the mouse cursor.
    FocusCursor,
    ResetCamera,
    /// Move the camera with the animated centre of the model.
    ToggleFollow,
    ToggleWireframe,
    TogglePause,
    NextClipInFile,
    PreviousClipInFile,
    /// Folder panel navigation.
    PreviousCreature,
    NextCreature,
    PreviousClip,
    NextClip,
}

impl Action {
    /// In the order a rebinding screen should list them.
    pub const ALL: [Action; 12] = [
        Action::FocusCursor,
        Action::ResetCamera,
        Action::ToggleFollow,
        Action::ToggleWireframe,
        Action::TogglePause,
        Action::NextClipInFile,
        Action::PreviousClipInFile,
        Action::PreviousCreature,
        Action::NextCreature,
        Action::PreviousClip,
        Action::NextClip,
        Action::Quit,
    ];

    /// Human-readable name, for help text and a future rebinding popup.
    pub fn label(self) -> &'static str {
        match self {
            Action::Quit => "Quit",
            Action::FocusCursor => "Focus on point under cursor",
            Action::ResetCamera => "Reset camera and restart clip",
            Action::ToggleFollow => "Toggle camera follow",
            Action::ToggleWireframe => "Toggle wireframe",
            Action::TogglePause => "Pause / resume",
            Action::NextClipInFile => "Next clip in this file",
            Action::PreviousClipInFile => "Previous clip in this file",
            Action::PreviousCreature => "Previous creature (folder)",
            Action::NextCreature => "Next creature (folder)",
            Action::PreviousClip => "Previous clip (folder)",
            Action::NextClip => "Next clip (folder)",
        }
    }

    /// Whether holding the key keeps firing. Only list walking does; a toggle that
    /// repeated would strobe.
    pub fn repeats(self) -> bool {
        matches!(self, Action::PreviousCreature | Action::NextCreature)
    }

    /// The settings-file name, e.g. `focus_cursor`.
    pub fn id(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn from_id(id: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(id.to_owned())).ok()
    }

    fn defaults(self) -> Vec<KeyChord> {
        let key = KeyChord::key;
        match self {
            Action::Quit => vec![key(KeyCode::Escape)],
            Action::FocusCursor => vec![key(KeyCode::KeyF)],
            Action::ResetCamera => vec![key(KeyCode::KeyR)],
            Action::ToggleFollow => vec![key(KeyCode::KeyT)],
            Action::ToggleWireframe => vec![key(KeyCode::KeyW)],
            Action::TogglePause => vec![key(KeyCode::Space)],
            Action::NextClipInFile => vec![key(KeyCode::KeyN)],
            Action::PreviousClipInFile => vec![key(KeyCode::KeyP)],
            Action::PreviousCreature => vec![key(KeyCode::ArrowUp)],
            Action::NextCreature => vec![key(KeyCode::ArrowDown)],
            Action::PreviousClip => vec![key(KeyCode::ArrowLeft)],
            Action::NextClip => vec![key(KeyCode::ArrowRight)],
        }
    }
}

/// A physical key plus the modifiers that must be held with it. Physical keys, so
/// a binding sits in the same place on every keyboard layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyChord {
    pub fn key(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// What was pressed: the key with whichever modifiers were down.
    pub fn pressed(key: KeyCode, modifiers: ModifiersState) -> Self {
        Self {
            key,
            ctrl: modifiers.control_key(),
            shift: modifiers.shift_key(),
            alt: modifiers.alt_key(),
        }
    }

    /// `KeyF`, `Ctrl+Shift+KeyS`: winit's key names with modifier prefixes.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut chord = Self::key(KeyCode::Escape);
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                _ => {
                    if key.is_some() {
                        return Err(format!("`{text}` names more than one key"));
                    }
                    key = Some(
                        serde_json::from_value::<KeyCode>(serde_json::Value::String(part.to_owned()))
                            .map_err(|_| format!("`{part}` is not a key name (try KeyF, Space, ArrowUp, F5)"))?,
                    );
                }
            }
        }
        chord.key = key.ok_or_else(|| format!("`{text}` has no key"))?;
        Ok(chord)
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        let name = serde_json::to_value(self.key)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{:?}", self.key));
        f.write_str(&name)
    }
}

/// The settings-file shape: action id -> chord strings. Only overrides are stored.
pub type BindingOverrides = BTreeMap<String, Vec<String>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    keys: BTreeMap<Action, Vec<KeyChord>>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            keys: Action::ALL
                .iter()
                .map(|&action| (action, action.defaults()))
                .collect(),
        }
    }
}

impl Bindings {
    /// The action a key press triggers. Modifiers must match exactly, so `Ctrl+F`
    /// can mean something other than `F`.
    pub fn action_for(&self, pressed: KeyChord) -> Option<Action> {
        // `ALL` order makes a conflict resolve the same way every time.
        Action::ALL
            .into_iter()
            .find(|action| self.chords(*action).contains(&pressed))
    }

    pub fn chords(&self, action: Action) -> &[KeyChord] {
        self.keys.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Replaces an action's keys. An empty list unbinds it.
    pub fn set(&mut self, action: Action, chords: Vec<KeyChord>) {
        self.keys.insert(action, chords);
    }

    /// Chords bound to more than one action, with those actions - what a rebinding
    /// popup should warn about.
    pub fn conflicts(&self) -> Vec<(KeyChord, Vec<Action>)> {
        let mut users: Vec<(KeyChord, Vec<Action>)> = Vec::new();
        for action in Action::ALL {
            for &chord in self.chords(action) {
                match users.iter_mut().find(|(existing, _)| *existing == chord) {
                    Some((_, actions)) => actions.push(action),
                    None => users.push((chord, vec![action])),
                }
            }
        }
        users.retain(|(_, actions)| actions.len() > 1);
        users
    }

    /// "F" / "Ctrl+Q, Escape" / "unbound", for help text.
    pub fn describe(&self, action: Action) -> String {
        let chords = self.chords(action);
        if chords.is_empty() {
            return String::from("unbound");
        }
        chords.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    }

    /// Defaults with `overrides` applied. Bad entries are skipped and reported, so a
    /// typo in the settings file costs one binding, not all of them.
    pub fn with_overrides(overrides: &BindingOverrides) -> (Self, Vec<String>) {
        let mut bindings = Self::default();
        let mut problems = Vec::new();
        for (id, texts) in overrides {
            let Some(action) = Action::from_id(id) else {
                problems.push(format!("unknown action `{id}`"));
                continue;
            };
            let mut chords = Vec::new();
            for text in texts {
                match KeyChord::parse(text) {
                    Ok(chord) => chords.push(chord),
                    Err(error) => problems.push(format!("{id}: {error}")),
                }
            }
            bindings.set(action, chords);
        }
        (bindings, problems)
    }

    /// Only what differs from the defaults, for saving.
    pub fn overrides(&self) -> BindingOverrides {
        let defaults = Self::default();
        Action::ALL
            .into_iter()
            .filter(|&action| self.chords(action) != defaults.chords(action))
            .map(|action| {
                (
                    action.id(),
                    self.chords(action).iter().map(ToString::to_string).collect(),
                )
            })
            .collect()
    }

    /// One line per action, for `--help` and `--keys`.
    pub fn help_lines(&self) -> Vec<String> {
        Action::ALL
            .iter()
            .map(|&action| format!("    {:<24}{}", self.describe(action), action.label()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_default_and_none_collide() {
        let bindings = Bindings::default();
        for action in Action::ALL {
            assert!(!bindings.chords(action).is_empty(), "{action:?} has no default key");
        }
        assert!(bindings.conflicts().is_empty(), "{:?}", bindings.conflicts());
    }

    #[test]
    fn f_focuses_and_modifiers_must_match() {
        let bindings = Bindings::default();
        let f = KeyChord::key(KeyCode::KeyF);
        assert_eq!(bindings.action_for(f), Some(Action::FocusCursor));
        let ctrl_f = KeyChord::pressed(KeyCode::KeyF, ModifiersState::CONTROL);
        assert_eq!(bindings.action_for(ctrl_f), None);
    }

    #[test]
    fn chords_round_trip_through_text() {
        for text in ["KeyF", "Ctrl+Shift+KeyS", "Alt+F5", "Space", "ArrowUp"] {
            let chord = KeyChord::parse(text).unwrap();
            assert_eq!(chord.to_string(), text);
        }
        assert_eq!(KeyChord::parse("ctrl + keyq").unwrap_err().contains("keyq"), true);
        assert!(KeyChord::parse("Ctrl+").is_err());
        assert!(KeyChord::parse("KeyA+KeyB").is_err());
    }

    #[test]
    fn overrides_replace_only_what_they_name() {
        let mut overrides = BindingOverrides::new();
        overrides.insert("focus_cursor".into(), vec!["Ctrl+KeyF".into(), "KeyZ".into()]);
        overrides.insert("toggle_wireframe".into(), vec![]);
        overrides.insert("no_such_action".into(), vec!["KeyX".into()]);
        overrides.insert("quit".into(), vec!["Nonsense".into(), "KeyQ".into()]);

        let (bindings, problems) = Bindings::with_overrides(&overrides);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(bindings.describe(Action::FocusCursor), "Ctrl+KeyF, KeyZ");
        assert_eq!(bindings.describe(Action::ToggleWireframe), "unbound");
        assert_eq!(bindings.describe(Action::Quit), "KeyQ");
        assert_eq!(bindings.describe(Action::TogglePause), "Space");
        assert_eq!(bindings.action_for(KeyChord::key(KeyCode::KeyW)), None);
    }

    #[test]
    fn saving_keeps_only_the_differences() {
        let mut bindings = Bindings::default();
        assert!(bindings.overrides().is_empty());
        bindings.set(Action::ToggleFollow, vec![KeyChord::key(KeyCode::KeyL)]);
        let saved = bindings.overrides();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved["toggle_follow"], vec![String::from("KeyL")]);
        assert_eq!(Bindings::with_overrides(&saved).0, bindings);
    }

    #[test]
    fn conflicts_are_reported() {
        let mut bindings = Bindings::default();
        bindings.set(Action::ToggleFollow, vec![KeyChord::key(KeyCode::KeyF)]);
        let conflicts = bindings.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].1, vec![Action::FocusCursor, Action::ToggleFollow]);
    }
}
