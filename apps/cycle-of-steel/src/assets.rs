//! Finding and loading the game's asset files.
//!
//! Assets are read through an [`AssetSource`], never straight from the filesystem:
//! on desktop they are files in a folder, on Android they are entries inside the
//! APK, and the rest of the game cannot tell the difference. Paths are always
//! relative to the assets root and use forward slashes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crystal_engine::scene::Texture;
use crystal_engine::sprite;

/// Sprites in the Unity project were imported at 64 pixels per world unit.
pub const PIXELS_PER_UNIT: f32 = 64.0;

/// Where asset bytes come from.
#[derive(Clone)]
pub enum AssetSource {
    /// A folder on disk.
    Folder(PathBuf),
    /// The `assets/` entries packed into the running APK.
    #[cfg(target_os = "android")]
    Apk(winit::platform::android::activity::AndroidApp),
}

impl AssetSource {
    /// The desktop source: an explicit folder, else `COS_ASSETS`, else `assets`
    /// beside the executable (a shipped build), else the one in the source tree
    /// (a `cargo run`).
    pub fn folder(explicit: Option<&Path>) -> Result<Self> {
        if let Some(path) = explicit {
            if !path.is_dir() {
                bail!("assets folder {} does not exist", path.display());
            }
            return Ok(Self::Folder(path.to_path_buf()));
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
            Some(path) => Ok(Self::Folder(path.clone())),
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

    /// Reads one asset whole. `path` is relative, e.g. `sprites/player.png`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        match self {
            Self::Folder(root) => {
                let full = root.join(path);
                std::fs::read(&full).with_context(|| format!("could not read {}", full.display()))
            }
            #[cfg(target_os = "android")]
            Self::Apk(app) => {
                use std::io::Read;

                let name = std::ffi::CString::new(path)
                    .with_context(|| format!("asset path `{path}` contains a NUL byte"))?;
                let mut asset = app
                    .asset_manager()
                    .open(&name)
                    .with_context(|| format!("asset `{path}` is not in the APK"))?;
                let mut bytes = Vec::new();
                asset
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("could not read asset `{path}` from the APK"))?;
                Ok(bytes)
            }
        }
    }

    /// Loads `sprites/<name>.png`.
    pub fn sprite(&self, name: &str) -> Result<Texture> {
        let bytes = self.read(&format!("sprites/{name}.png"))?;
        sprite::decode_png(name, &bytes).with_context(|| format!("could not decode sprite `{name}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_tree_assets_load() {
        let assets = AssetSource::folder(None).expect("assets folder beside the manifest");
        let tiles = assets.sprite("main_dark_compound").expect("tileset loads");
        assert_eq!((tiles.width, tiles.height), (256, 192));
        assert!(assets.read("sprites/no_such_file.png").is_err());
    }

    #[test]
    fn a_missing_explicit_folder_is_an_error() {
        assert!(AssetSource::folder(Some(Path::new("/definitely/not/here"))).is_err());
    }
}
