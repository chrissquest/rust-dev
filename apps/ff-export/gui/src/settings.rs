//! The two directories worth remembering between runs.
//!
//! Each picker keeps its own last location - ROMs and exports are rarely in the same
//! place - and both survive restarts in `%APPDATA%\ff-export\settings.json`.
//!
//! Everything here is touched from the UI thread only (callbacks and the drop hook),
//! which is why a thread-local is enough.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rom_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_dir: Option<PathBuf>,
}

thread_local! {
    static SETTINGS: RefCell<Settings> = RefCell::new(load());
}

/// Where the ROM picker should open.
pub fn rom_dir() -> Option<PathBuf> {
    SETTINGS.with(|settings| settings.borrow().rom_dir.clone())
}

/// Where the output picker should open.
pub fn output_dir() -> Option<PathBuf> {
    SETTINGS.with(|settings| settings.borrow().output_dir.clone())
}

/// Remembers the folder a ROM was picked or dropped from.
pub fn remember_rom(dir: &Path) {
    update(|settings| settings.rom_dir = Some(dir.to_path_buf()));
}

/// Remembers the folder chosen for the export.
pub fn remember_output(dir: &Path) {
    update(|settings| settings.output_dir = Some(dir.to_path_buf()));
}

fn update(change: impl FnOnce(&mut Settings)) {
    SETTINGS.with(|settings| {
        let mut settings = settings.borrow_mut();
        change(&mut settings);
        if let Some(path) = settings_path() {
            save_to(&path, &settings);
        }
    });
}

/// `FF_EXPORT_SETTINGS` overrides the location, which keeps a portable copy's
/// settings beside the app and gives the tests somewhere harmless to write.
fn settings_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FF_EXPORT_SETTINGS") {
        return Some(PathBuf::from(path));
    }

    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("ff-export").join("settings.json"))
}

fn load() -> Settings {
    settings_path()
        .map(|path| load_from(&path))
        .unwrap_or_default()
}

fn load_from(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
            // A corrupt settings file should never stop the app from starting.
            eprintln!("warning: ignoring {}: {error}", path.display());
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

fn save_to(path: &Path, settings: &Settings) {
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("warning: could not create {}: {error}", parent.display());
            return;
        }
    }

    match serde_json::to_string_pretty(settings) {
        Ok(text) => {
            if let Err(error) = std::fs::write(path, text) {
                eprintln!("warning: could not write {}: {error}", path.display());
            }
        }
        Err(error) => eprintln!("warning: could not serialise settings: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("ff-export-settings-{}-{name}", std::process::id()))
            .join("settings.json")
    }

    #[test]
    fn round_trips_both_directories() {
        let path = scratch("round-trip");
        let _ = std::fs::remove_file(&path);

        let settings = Settings {
            rom_dir: Some(PathBuf::from(r"B:\Emu\NDS Roms")),
            output_dir: Some(PathBuf::from(r"B:\Emu\FF Extractions")),
        };
        save_to(&path, &settings);

        let loaded = load_from(&path);
        assert_eq!(loaded.rom_dir.as_deref(), Some(Path::new(r"B:\Emu\NDS Roms")));
        assert_eq!(
            loaded.output_dir.as_deref(),
            Some(Path::new(r"B:\Emu\FF Extractions"))
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_reads_as_default() {
        let loaded = load_from(&scratch("absent"));
        assert!(loaded.rom_dir.is_none() && loaded.output_dir.is_none());
    }

    #[test]
    fn corrupt_file_reads_as_default() {
        let path = scratch("corrupt");
        save_to(&path, &Settings::default());
        std::fs::write(&path, "{ not json").expect("write a broken file");

        let loaded = load_from(&path);
        assert!(loaded.rom_dir.is_none() && loaded.output_dir.is_none());

        let _ = std::fs::remove_file(&path);
    }
}
