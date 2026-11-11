//! Finding and loading the files under `assets/`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crystal_engine::scene::Texture;
use crystal_engine::sprite;

/// Sprites in the Unity project were imported at 64 pixels per world unit.
pub const PIXELS_PER_UNIT: f32 = 64.0;

/// The assets folder to use: an explicit choice, else `COS_ASSETS`, else `assets`
/// beside the executable (a shipped build), else the one in the source tree (a
/// `cargo run`).
pub fn find_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if !path.is_dir() {
            bail!("assets folder {} does not exist", path.display());
        }
        return Ok(path.to_path_buf());
    }

    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("COS_ASSETS") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(folder) = exe.parent()
    {
        candidates.push(folder.join("assets"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets"));

    match candidates.iter().find(|path| path.is_dir()) {
        Some(path) => Ok(path.clone()),
        None => bail!(
            "could not find the assets folder; looked in {}",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Loads `sprites/<name>.png` from the assets folder.
pub fn load_sprite(assets: &Path, name: &str) -> Result<Texture> {
    let path = assets.join("sprites").join(format!("{name}.png"));
    sprite::load_png(&path).with_context(|| format!("could not load sprite `{name}`"))
}
