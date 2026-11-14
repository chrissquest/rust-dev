//! Sounds: Unity `AudioSource`s -> `{clip, volume, loop}`, and `sounds.json`.
//!
//! Things that make a sound in Unity hold an `AudioSource` a script plays: a
//! gun's fire, an explosion's bang, an enemy body's death noise. Those are
//! written beside the thing (in `guns.json`, `enemies.json`). The player's
//! sounds and the dungeon's music have no such home, so they go in
//! `sounds.json`. The clips themselves go to `audio/` under their own names
//! as Ogg Vorbis, about a tenth of WAV's size: WAVs are encoded with ffmpeg
//! (`FFMPEG` names it if it is not on the path), Oggs are copied. Without
//! ffmpeg the WAVs are copied as they are, which the game also plays. A sound
//! names its clip by the file's stem.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value as Json, json};

use crate::json::number;
use crate::project::Project;
use crate::room::script_guid;
use crate::yaml::{Document, Value};

const PLAYER: &str = "Prefabs/Player.prefab";
const DUNGEON_SCENE: &str = "Scenes/Dungeon.unity";

/// An `AudioSource`'s clip, volume and whether it loops; `None` without a clip.
pub fn sound_of(project: &Project, source: &Value) -> Option<Json> {
    let clip = project.stem_of(source.get("m_audioClip").get("guid").str())?;
    Some(json!({
        "clip": clip,
        "volume": number(source.get("m_Volume").f32()),
        "loop": source.get("Loop").bool(),
    }))
}

/// The `AudioSource` a script field points at, in the same file.
pub fn sound_at(project: &Project, documents: &[Document], reference: &Value) -> Option<Json> {
    let id = reference.get("fileID").i64();
    let source = documents.iter().find(|document| document.file_id == id && document.class == "AudioSource")?;
    sound_of(project, &source.body)
}

/// `sounds.json`: the player's sounds (`PlayerData`, `PlayerAnimations`) and the
/// music the Dungeon scene plays on its own.
pub fn convert(project: &Project) -> Result<Json> {
    let documents = project.documents(Path::new(PLAYER))?;
    let mut player = serde_json::Map::new();
    for (scripts, fields) in [
        (&["PlayerData"][..], &[("hurt", "playerHurt"), ("heal", "playerHeal"), ("pickup", "playerPickup")][..]),
        (&["PlayerAnimations"][..], &[("walk", "playerWalk")][..]),
    ] {
        let body = documents
            .iter()
            .find(|document| {
                document.class == "MonoBehaviour"
                    && project.stem_of(script_guid(document)).is_some_and(|stem| scripts.contains(&stem.as_str()))
            })
            .map(|document| &document.body);
        for (key, field) in fields {
            if let Some(sound) = body.and_then(|body| sound_at(project, &documents, body.get(field))) {
                player.insert(key.to_string(), sound);
            }
        }
    }
    let scene = project.documents(Path::new(DUNGEON_SCENE))?;
    let music = scene
        .iter()
        .find(|document| {
            document.class == "AudioSource" && document.body.get("m_PlayOnAwake").bool() && document.body.get("Loop").bool()
        })
        .and_then(|source| sound_of(project, &source.body));
    Ok(json!({ "player": player, "music": music }))
}

/// Vorbis quality for encoded clips: ffmpeg's `-q:a`, about 128 kbit/s.
const VORBIS_QUALITY: &str = "4";

/// Writes every clip a written file names (any `clip` in it) from the project's
/// audio into `audio/`, emptied first, and returns how many.
pub fn copy_clips(project: &Project, output: &Path, written: &[&Json]) -> Result<usize> {
    let mut stems = BTreeSet::new();
    for json in written {
        collect_clips(json, &mut stems);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    find_audio(&project.assets_dir().join("Audio"), &mut files)?;
    let folder = output.join("audio");
    if folder.exists() {
        std::fs::remove_dir_all(&folder).with_context(|| format!("could not empty {}", folder.display()))?;
    }
    std::fs::create_dir_all(&folder).with_context(|| format!("could not create {}", folder.display()))?;
    let ffmpeg = std::env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let mut have_ffmpeg = true;
    let mut copied = 0;
    for stem in &stems {
        let Some(file) = files.iter().find(|file| file.file_stem().is_some_and(|name| name.to_string_lossy() == *stem)) else {
            continue;
        };
        let is_ogg = file.extension().is_some_and(|extension| extension == "ogg");
        if !is_ogg && have_ffmpeg {
            let target = folder.join(format!("{stem}.ogg"));
            let run = std::process::Command::new(&ffmpeg)
                .args(["-v", "error", "-y", "-i"])
                .arg(file)
                .args(["-c:a", "libvorbis", "-q:a", VORBIS_QUALITY])
                .arg(&target)
                .status();
            match run {
                Ok(status) if status.success() => {
                    copied += 1;
                    continue;
                }
                Ok(status) => anyhow::bail!("ffmpeg could not encode {} ({status})", file.display()),
                Err(error) => {
                    println!("    warning: ffmpeg not found ({error}); copying WAVs as they are");
                    have_ffmpeg = false;
                }
            }
        }
        let name = file.file_name().expect("a file");
        crate::copy_writable(file, &folder.join(name))
            .with_context(|| format!("could not copy {}", file.display()))?;
        copied += 1;
    }
    Ok(copied)
}

fn collect_clips(json: &Json, stems: &mut BTreeSet<String>) {
    match json {
        Json::Object(fields) => {
            for (key, value) in fields {
                match (key.as_str(), value) {
                    ("clip", Json::String(stem)) => {
                        stems.insert(stem.clone());
                    }
                    _ => collect_clips(value, stems),
                }
            }
        }
        Json::Array(items) => items.iter().for_each(|item| collect_clips(item, stems)),
        _ => {}
    }
}

fn find_audio(folder: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(folder).with_context(|| format!("could not read {}", folder.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            find_audio(&path, files)?;
        } else if path.extension().is_some_and(|extension| matches!(extension.to_str(), Some("wav" | "ogg"))) {
            files.push(path);
        }
    }
    Ok(())
}
