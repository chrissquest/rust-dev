//! Playing the sounds the world asks for, and the music.
//!
//! The simulation only says what happened ([`Sound`] events); this plays them
//! through the default output device with `rodio`. Clips are read from
//! `audio/` and decoded the first time they are played, then kept. A machine
//! with no sound device simply plays nothing.
//!
//! As Unity's `AudioSource`s do: a one-off sound starts afresh each time and
//! overlaps the last; the laser's and the footsteps' sounds start over on
//! `Play` and pause on `Pause`; the Dungeon scene's music loops from the start.

use std::collections::HashMap;
use std::io::Cursor;

use rodio::source::Buffered;
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

use crate::assets::AssetSource;
use crate::data::{SoundFile, SoundsFile};
use crate::sim::Sound;
use crate::sim::defs::Defs;

type Clip = Buffered<Decoder<Cursor<Vec<u8>>>>;

pub struct Audio {
    stream: OutputStream,
    assets: AssetSource,
    /// Clips by stem. `None` for one that would not load, so it is tried once.
    clips: HashMap<String, Option<Clip>>,
    sounds: SoundsFile,
    laser: Sink,
    walk: Sink,
    /// Kept so the music keeps playing.
    _music: Sink,
}

impl Audio {
    /// Opens the default output device and starts the music. `None`, with a
    /// warning, when there is no device.
    pub fn new(assets: &AssetSource) -> Option<Self> {
        let mut stream = match OutputStreamBuilder::open_default_stream() {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("no sound: {error}");
                return None;
            }
        };
        stream.log_on_drop(false);
        let sounds = SoundsFile::load(assets).unwrap_or_else(|error| {
            log::warn!("no sounds.json: {error:#}");
            SoundsFile::default()
        });
        let laser = Sink::connect_new(stream.mixer());
        let walk = Sink::connect_new(stream.mixer());
        let music = Sink::connect_new(stream.mixer());
        let mut audio = Self { stream, assets: assets.clone(), clips: HashMap::new(), sounds, laser, walk, _music: music };
        if let Some(file) = audio.sounds.music.clone() {
            let clip = audio.clip(&file.clip);
            start(&audio._music, clip, &file);
        }
        Some(audio)
    }

    /// Plays what the world asked for.
    pub fn play(&mut self, sound: Sound, defs: &Defs) {
        let one_off = match sound {
            Sound::Fire(gun) => defs.gun(gun).sound.as_ref(),
            Sound::Effect(effect) => defs.effect(effect).sound.as_ref(),
            Sound::Death(enemy) => defs.enemy(enemy).death_sound.as_ref(),
            Sound::Hurt => self.sounds.player.hurt.as_ref(),
            Sound::Heal => self.sounds.player.heal.as_ref(),
            Sound::Pickup => self.sounds.player.pickup.as_ref(),
            Sound::LaserStart(gun) => {
                if let Some(file) = defs.gun(gun).sound.clone() {
                    let clip = self.clip(&file.clip);
                    self.laser = Sink::connect_new(self.stream.mixer());
                    start(&self.laser, clip, &file);
                }
                return;
            }
            Sound::LaserStop => {
                self.laser.pause();
                return;
            }
            Sound::WalkStart => {
                if let Some(file) = self.sounds.player.walk.clone() {
                    let clip = self.clip(&file.clip);
                    self.walk = Sink::connect_new(self.stream.mixer());
                    start(&self.walk, clip, &file);
                }
                return;
            }
            Sound::WalkStop => {
                self.walk.pause();
                return;
            }
        }
        .cloned();
        if let Some(file) = one_off
            && let Some(clip) = self.clip(&file.clip)
        {
            self.stream.mixer().add(clip.amplify(file.volume));
        }
    }

    /// A clip by stem, read and decoded the first time.
    fn clip(&mut self, stem: &str) -> Option<Clip> {
        if let Some(clip) = self.clips.get(stem) {
            return clip.clone();
        }
        let clip = ["wav", "ogg"].iter().find_map(|extension| {
            let bytes = self.assets.read(&format!("audio/{stem}.{extension}")).ok()?;
            match Decoder::new(Cursor::new(bytes)) {
                Ok(decoder) => Some(decoder.buffered()),
                Err(error) => {
                    log::warn!("could not decode audio/{stem}.{extension}: {error}");
                    None
                }
            }
        });
        if clip.is_none() {
            log::warn!("no clip audio/{stem}");
        }
        self.clips.insert(stem.to_string(), clip.clone());
        clip
    }
}

/// Starts `clip` on `sink` at `file`'s volume, looping if it loops.
fn start(sink: &Sink, clip: Option<Clip>, file: &SoundFile) {
    let Some(clip) = clip else {
        return;
    };
    sink.set_volume(file.volume);
    if file.looping {
        sink.append(clip.repeat_infinite());
    } else {
        sink.append(clip);
    }
    sink.play();
}
