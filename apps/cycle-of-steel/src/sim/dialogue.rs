//! The player's thoughts: lines shown one at a time at the bottom of the screen.
//!
//! As Unity's `PlayerUI.QueueDialogue` and `ProgressDialogue`: lines queue up,
//! interact shows the next, and while any shows the player can neither move nor
//! shoot (`PlayerInput.Update`); the rest of the world carries on. Most lines
//! belong to a [`PlayerEvent`] and are shown once per save
//! (`Player.DialogueEvent`); the exit room's are shown once per floor.

use std::collections::VecDeque;
use std::sync::Arc;

use crystal_engine::glam::Vec2;

/// Unity's `PlayerEvent`, in its order: what has been said, kept in the save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerEvent {
    Intro,
    Secret,
    Secret2,
    GunPickup,
    Desk,
    ShootButton,
    /// Declared in Unity but never raised.
    PreBoss,
}

impl PlayerEvent {
    pub const ALL: [PlayerEvent; 7] = [
        PlayerEvent::Intro,
        PlayerEvent::Secret,
        PlayerEvent::Secret2,
        PlayerEvent::GunPickup,
        PlayerEvent::Desk,
        PlayerEvent::ShootButton,
        PlayerEvent::PreBoss,
    ];

    /// The number a `DialogueEvent`'s `playerEvent` field holds.
    pub fn from_number(number: usize) -> Option<Self> {
        Self::ALL.get(number).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            PlayerEvent::Intro => "Intro",
            PlayerEvent::Secret => "Secret",
            PlayerEvent::Secret2 => "Secret2",
            PlayerEvent::GunPickup => "GunPickup",
            PlayerEvent::Desk => "Desk",
            PlayerEvent::ShootButton => "ShootButton",
            PlayerEvent::PreBoss => "PreBoss",
        }
    }

    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.name() == name)
    }
}

/// `HubRespawn.FinishAnim`: on waking in the hub. Unity's second line has a
/// broken character after "Core AI"; a comma reads as meant.
pub const INTRO: [&str; 2] = [
    "The tyranny of the knights must end...",
    "The Core AI, their god must be destroyed. I must escape this factory using any means necessary.",
];
/// `DebugRoom.OnInteract` and `DebugRoomExit.OnInteract`.
pub const SECRET: [&str; 2] = ["How did you know...", "Well, have at it"];
pub const SECRET2: [&str; 1] = ["Farewell, you seem well equipped now."];
/// `ExitRoomDialogue`.
pub const EXIT_ROOM: [&str; 2] = ["The Core AI is close...", "[Go down the stairs to exit or continue forth for a challege.]"];


/// A box that says something the first time the player walks into it
/// (`DialogueEvent`, `ExitRoomDialogue`).
#[derive(Clone, Debug)]
pub struct DialogueTrigger {
    pub center: Vec2,
    pub half: Vec2,
    /// The event it belongs to, said once per save; `None` for one said once
    /// for as long as the trigger exists.
    pub event: Option<PlayerEvent>,
    pub lines: Arc<[Arc<str>]>,
    pub said: bool,
}

/// The lines waiting to be shown, the first of them showing, and which
/// events have been said.
#[derive(Clone, Debug, Default)]
pub struct Dialogue {
    queue: VecDeque<Arc<str>>,
    pub said: [bool; PlayerEvent::ALL.len()],
}

impl Dialogue {
    /// The line showing, if any.
    pub fn current(&self) -> Option<&str> {
        self.queue.front().map(|line| &**line)
    }

    pub fn is_showing(&self) -> bool {
        !self.queue.is_empty()
    }

    /// `PlayerUI.QueueDialogue`: after whatever is already queued.
    pub fn queue(&mut self, lines: impl IntoIterator<Item = Arc<str>>) {
        self.queue.extend(lines);
    }

    /// `Player.DialogueEvent`: the lines, unless the event has been said.
    pub fn event(&mut self, event: PlayerEvent, lines: impl IntoIterator<Item = Arc<str>>) {
        let said = &mut self.said[event as usize];
        if !*said {
            *said = true;
            self.queue(lines);
        }
    }

    /// `PlayerUI.ProgressDialogue`: on to the next line, or done.
    pub fn advance(&mut self) {
        self.queue.pop_front();
    }

    /// Drops what is queued, keeping what has been said.
    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

/// Fixed lines as the queue holds them.
pub fn lines(texts: &[&str]) -> impl Iterator<Item = Arc<str>> {
    texts.iter().map(|&text| Arc::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_said_once_and_its_lines_show_in_turn() {
        let mut dialogue = Dialogue::default();
        dialogue.event(PlayerEvent::Secret, lines(&SECRET));
        dialogue.event(PlayerEvent::Secret, lines(&SECRET));
        dialogue.queue(lines(&SECRET2));
        assert_eq!(dialogue.current(), Some(SECRET[0]));
        dialogue.advance();
        assert_eq!(dialogue.current(), Some(SECRET[1]));
        dialogue.advance();
        assert_eq!(dialogue.current(), Some(SECRET2[0]), "queued after, not twice");
        dialogue.advance();
        assert!(!dialogue.is_showing());
        assert!(dialogue.said[PlayerEvent::Secret as usize]);
    }

    #[test]
    fn events_go_by_unitys_numbers_and_names() {
        assert_eq!(PlayerEvent::from_number(4), Some(PlayerEvent::Desk));
        assert_eq!(PlayerEvent::from_number(5), Some(PlayerEvent::ShootButton));
        assert_eq!(PlayerEvent::named("GunPickup"), Some(PlayerEvent::GunPickup));
        assert_eq!(PlayerEvent::from_number(7), None);
    }
}
