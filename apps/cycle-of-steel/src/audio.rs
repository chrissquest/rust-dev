//! Playing the sounds the world asks for, and the music.
//!
//! The simulation only says what happened ([`Sound`] events); this plays them
//! through the default output device with `rodio`. Clips are Ogg Vorbis
//! (or WAV) files in `audio/`, read the first time they are needed. Short
//! sounds are decoded once and kept; looping ones (the music, the laser, the
//! footsteps) are decoded as they play, since decoded they would take tens of
//! megabytes. A machine with no sound device simply plays nothing.
//!
//! Ogg goes through rodio's `lewton` decoder: its `symphonia` one gives no
//! samples at all for short Vorbis files such as the gunshot.
//!
//! As Unity's `AudioSource`s do: a one-off sound starts afresh each time and
//! overlaps the last; the laser's and the footsteps' sounds start over on
//! `Play` and pause on `Pause`; the Dungeon scene's music loops from the start.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use rodio::source::Buffered;
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

use crate::assets::AssetSource;
use crate::data::{SoundFile, SoundsFile};
use crate::sim::Sound;
use crate::sim::defs::Defs;

/// A clip's file, shared by everything playing it.
type Bytes = Cursor<Arc<[u8]>>;
/// A short clip, decoded.
type Clip = Buffered<Decoder<Bytes>>;

/// A clip as read: its file, and once played as a one-off, its samples.
struct Loaded {
    bytes: Arc<[u8]>,
    decoded: Option<Clip>,
}

pub struct Audio {
    stream: OutputStream,
    assets: AssetSource,
    /// Clips by stem. `None` for one that would not load, so it is tried once.
    clips: HashMap<String, Option<Loaded>>,
    sounds: SoundsFile,
    laser: Sink,
    walk: Sink,
    /// Kept so the music keeps playing.
    _music: Sink,
    /// While the game is paused: whether the laser and the footsteps were
    /// playing, to carry on with them after.
    paused: Option<(bool, bool)>,
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
        let mut audio = Self { stream, assets: assets.clone(), clips: HashMap::new(), sounds, laser, walk, _music: music, paused: None };
        if let Some(file) = audio.sounds.music.clone() {
            let bytes = audio.bytes(&file.clip);
            start(&audio._music, bytes, &file);
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
                    let bytes = self.bytes(&file.clip);
                    self.laser = Sink::connect_new(self.stream.mixer());
                    start(&self.laser, bytes, &file);
                }
                return;
            }
            Sound::LaserStop => {
                self.laser.pause();
                return;
            }
            Sound::WalkStart => {
                if let Some(file) = self.sounds.player.walk.clone() {
                    let bytes = self.bytes(&file.clip);
                    self.walk = Sink::connect_new(self.stream.mixer());
                    start(&self.walk, bytes, &file);
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
            && let Some(clip) = self.decoded(&file.clip)
        {
            self.stream.mixer().add(clip.amplify(file.volume));
        }
    }

    /// `PauseMenu.Pause` and `Resume`: the guns' and the player's sounds are
    /// muted while a menu is open; the music plays on.
    pub fn set_paused(&mut self, paused: bool) {
        let playing = |sink: &Sink| !sink.is_paused() && !sink.empty();
        match (paused, self.paused) {
            (true, None) => {
                self.paused = Some((playing(&self.laser), playing(&self.walk)));
                self.laser.pause();
                self.walk.pause();
            }
            (false, Some((laser, walk))) => {
                self.paused = None;
                if laser {
                    self.laser.play();
                }
                if walk {
                    self.walk.play();
                }
            }
            _ => {}
        }
    }

    /// A clip's file by stem, `.ogg` or `.wav`, read the first time.
    fn bytes(&mut self, stem: &str) -> Option<Arc<[u8]>> {
        self.loaded(stem).map(|loaded| loaded.bytes.clone())
    }

    /// A short clip's samples, decoded the first time.
    fn decoded(&mut self, stem: &str) -> Option<Clip> {
        let loaded = self.loaded(stem)?;
        if loaded.decoded.is_none() {
            match Decoder::new(Cursor::new(loaded.bytes.clone())) {
                Ok(decoder) => loaded.decoded = Some(decoder.buffered()),
                Err(error) => log::warn!("could not decode audio/{stem}: {error}"),
            }
        }
        loaded.decoded.clone()
    }

    fn loaded(&mut self, stem: &str) -> Option<&mut Loaded> {
        if !self.clips.contains_key(stem) {
            let bytes = ["ogg", "wav"].iter().find_map(|extension| self.assets.read(&format!("audio/{stem}.{extension}")).ok());
            if bytes.is_none() {
                log::warn!("no clip audio/{stem}");
            }
            let loaded = bytes.map(|bytes| Loaded { bytes: bytes.into(), decoded: None });
            self.clips.insert(stem.to_string(), loaded);
        }
        self.clips.get_mut(stem)?.as_mut()
    }
}

/// Starts a clip on `sink` at `file`'s volume, decoding as it plays, looping
/// if it loops.
fn start(sink: &Sink, bytes: Option<Arc<[u8]>>, file: &SoundFile) {
    let Some(bytes) = bytes else {
        return;
    };
    sink.set_volume(file.volume);
    let appended = if file.looping {
        Decoder::new_looped(Cursor::new(bytes)).map(|decoder| sink.append(decoder))
    } else {
        Decoder::new(Cursor::new(bytes)).map(|decoder| sink.append(decoder))
    };
    match appended {
        Ok(()) => sink.play(),
        Err(error) => log::warn!("could not decode audio/{}: {error}", file.clip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every clip gives sound, not just a header: rodio's symphonia decoder
    /// opened the short gunshot Ogg and gave no samples from it.
    #[test]
    fn every_clip_decodes_to_sound() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/audio");
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let decoder = Decoder::new(Cursor::new(bytes));
            assert!(decoder.is_ok(), "{} does not decode: {:?}", path.display(), decoder.err());
            let decoder = decoder.unwrap();
            let loudest = decoder.fold(0.0f32, |high, sample| high.max(sample.abs()));
            assert!(loudest > 0.01, "{} is silent", path.display());
        }
    }
}
