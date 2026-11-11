//! What to reopen when `ff-view` is started without arguments.
//!
//! One setting today - the last export folder - stored as JSON next to the
//! user's other application data: `%APPDATA%\ff-view\settings.json` on Windows,
//! `$XDG_CONFIG_HOME/ff-view/settings.json` elsewhere. `serde_json` is already a
//! dependency, so no config format had to be added for this.
//!
//! Saves try a few locations in turn (roaming data, local data, the home
//! directory, and finally a portable file beside the executable), because a
//! locked-down profile, an antivirus rule or a confined terminal can make the
//! first ones unwritable; the one that worked is printed so the file is never a
//! mystery. `FF_VIEW_CONFIG` pins the location outright, which is useful for
//! tests and for a portable copy of the viewer.
//!
//! Losing this file is harmless: it only decides what the empty window opens on.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "settings.json";
/// Used beside the executable when no profile location can be written.
const PORTABLE_FILE_NAME: &str = "ff-view-settings.json";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_folder: Option<PathBuf>,
}

impl Settings {
    pub fn load() -> Self {
        Self::candidates()
            .iter()
            .find_map(|path| Self::read_if_present(path))
            .unwrap_or_default()
    }

    /// Writes the settings. Returns the file it wrote, or `None` after saying on
    /// stderr why it could not: not being able to remember a folder is not a
    /// reason to refuse to show it, but it should never be silent either.
    ///
    /// More than one location is tried, because `%APPDATA%` is not always
    /// writable - a locked-down profile, an antivirus rule, a process started
    /// with a stripped environment. The first one that works wins, and its path
    /// is reported so it is never a mystery where the setting went.
    pub fn save(&self) -> Option<PathBuf> {
        let candidates = Self::candidates();
        if candidates.is_empty() {
            eprintln!(
                "warning: no place to save settings (APPDATA and HOME are both unset); \
                 set FF_VIEW_CONFIG to a file path to choose one"
            );
            return None;
        }

        let mut failures = Vec::new();
        for path in &candidates {
            match self.write(path) {
                Ok(()) => return Some(path.clone()),
                Err(error) => failures.push((path.clone(), error)),
            }
        }

        // Name every place that was refused: one line saying "somewhere" was
        // useless when the location itself was the problem.
        eprintln!("warning: could not save settings; every location was refused:");
        for (path, error) in &failures {
            eprintln!("    {}: {error}", path.display());
        }
        eprintln!("    set FF_VIEW_CONFIG to a writable file path to choose one");
        None
    }

    /// Where settings are looked for and written, in order of preference.
    fn candidates() -> Vec<PathBuf> {
        if let Some(path) = std::env::var_os("FF_VIEW_CONFIG") {
            return vec![PathBuf::from(path)];
        }

        let mut paths = Vec::new();
        let profile = std::env::var_os("USERPROFILE").map(PathBuf::from);

        // `APPDATA` is the normal answer; deriving it from `USERPROFILE` covers
        // processes started with a stripped environment (some IDE run
        // configurations do this), which used to mean nothing was ever saved.
        let roaming = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| profile.as_ref().map(|home| home.join("AppData").join("Roaming")));
        if let Some(base) = roaming {
            paths.push(base.join("ff-view").join(FILE_NAME));
        }

        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| profile.as_ref().map(|home| home.join("AppData").join("Local")));
        if let Some(base) = local {
            paths.push(base.join("ff-view").join(FILE_NAME));
        }

        // The profile itself, then the Unix conventions. A read-only roaming
        // profile is a real thing, and `.ff-view` in the home directory is still
        // a place a portable copy can write.
        if let Some(home) = &profile {
            paths.push(home.join(".ff-view").join(FILE_NAME));
        }
        if let Some(base) = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        {
            paths.push(base.join("ff-view").join(FILE_NAME));
        }

        // Nothing in the profile was writable - a sandboxed terminal, an EDR
        // rule, an AppContainer. `cargo run` puts the binary in the build
        // directory, which is writable in a dev tree, so a "portable" settings
        // file beside it keeps the folder remembered there too. The path is
        // printed when it is used, and `cargo clean` forgets it.
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            paths.push(dir.join(PORTABLE_FILE_NAME));
        }

        paths
    }

    fn read_if_present(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&text) {
            Ok(settings) => Some(settings),
            Err(error) => {
                eprintln!("warning: ignoring {}: {error}", path.display());
                None
            }
        }
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

/// The remembered folder, if it is still there.
pub fn last_folder() -> Option<PathBuf> {
    Settings::load().last_folder.filter(|folder| folder.is_dir())
}

/// The remembered folder as stored, whether or not it still exists. Used by
/// `--settings` to explain why a launch did not reopen anything.
pub fn remembered_folder() -> Option<PathBuf> {
    Settings::load().last_folder
}

/// Where the settings file is: the first existing candidate, or the place a save
/// would go. `--settings` prints this.
pub fn path() -> Option<PathBuf> {
    let candidates = Settings::candidates();
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .or_else(|| candidates.first().cloned())
}

/// Remembers `folder` as what the next argument-less launch should open, and
/// returns the file it was written to.
pub fn remember_folder(folder: &Path) -> Option<PathBuf> {
    let mut settings = Settings::load();
    settings.last_folder = Some(folder.to_path_buf());
    settings.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ff-view-settings-{label}-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_missing_file_reads_as_default() {
        let path = scratch("missing");
        let settings = Settings::read_if_present(&path).unwrap_or_default();
        assert!(settings.last_folder.is_none());
    }

    #[test]
    fn settings_round_trip_through_json() {
        let path = scratch("round-trip");
        let settings = Settings {
            last_folder: Some(PathBuf::from(r"C:\models\externals")),
        };
        settings.write(&path).expect("write settings");

        let restored = Settings::read_if_present(&path).expect("read settings");
        assert_eq!(restored.last_folder, settings.last_folder);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_corrupt_file_is_ignored() {
        let path = scratch("corrupt");
        std::fs::write(&path, "{ not json").expect("write corrupt settings");

        assert!(Settings::read_if_present(&path).is_none());

        let _ = std::fs::remove_file(&path);
    }
}
