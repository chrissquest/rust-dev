//! Where the save lives, and reading and writing it.
//!
//! Unity kept `save1.dat` in `Application.persistentDataPath`. Here it is
//! `save1.json` in the user's data folder (`%APPDATA%\CycleOfSteel` on
//! Windows, `~/.local/share/cycle-of-steel` on Linux, the app's internal
//! storage on Android). Unity saves are not carried over.

use std::path::Path;

use anyhow::{Context, Result};

use crate::sim::save::Save;

pub const SAVE_NAME: &str = "save1.json";

/// The save file on this desktop, if there is a home to put it in.
#[cfg(not(target_os = "android"))]
pub fn default_save_path() -> Option<std::path::PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(std::path::PathBuf::from);
    let folder = if cfg!(windows) {
        var("APPDATA")?.join("CycleOfSteel")
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support/CycleOfSteel")
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|home| home.join(".local/share")))?.join("cycle-of-steel")
    };
    Some(folder.join(SAVE_NAME))
}

/// Reads the save at `path`: `None` when there is none yet. A file that cannot
/// be read is set aside as `.bad`, so that the next save does not destroy it,
/// and the game starts afresh.
pub fn read_save(path: &Path) -> Option<Save> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!("could not read {}: {error}", path.display());
            return None;
        }
    };
    match Save::from_json(&text) {
        Ok(save) => Some(save),
        Err(error) => {
            let aside = path.with_extension("json.bad");
            log::warn!("{} is not a save this game can read ({error:#}); kept as {}", path.display(), aside.display());
            let _ = std::fs::rename(path, &aside);
            None
        }
    }
}

/// Writes `save` to `path`, through a temporary file so a crash mid-write
/// leaves the old save whole.
pub fn write_save(path: &Path, save: &Save) -> Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).with_context(|| format!("could not create {}", folder.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, save.to_json()).with_context(|| format!("could not write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("could not replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_survives_the_disk_and_a_broken_one_is_set_aside() {
        // Under target/, which the sandbox lets tests write to.
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-saves");
        let path = folder.join(SAVE_NAME);
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_save(&path), None);

        let mut save = Save::new_game(1000.0);
        save.data_cells = 42;
        write_save(&path, &save).unwrap();
        assert_eq!(read_save(&path), Some(save));

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_save(&path), None);
        assert!(!path.exists() && path.with_extension("json.bad").exists());
        let _ = std::fs::remove_dir_all(&folder);
    }
}
