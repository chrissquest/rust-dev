//! An exported folder: `manifest.json` plus one `.glb` per clip.
//!
//! `ff-export` writes one mesh per clip, so a model here is a (creature, clip)
//! pair, and a folder is a list of them. This module is the only part of the
//! crate that knows the manifest's shape, which keeps `ff-view` and `ff-browser`
//! agreeing about what "an export folder" is.
//!
//! A folder without a manifest is still accepted: every `.glb`/`.gltf` in it
//! becomes its own one-clip entry. That is what makes dropping an arbitrary
//! folder of models work instead of failing with "not an export folder".

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// How deep a manifest-less folder is searched, and how many files it may hold.
const SCAN_DEPTH: usize = 6;
const SCAN_LIMIT: usize = 4096;

/// The parts of `manifest.json` a viewer needs; the rest is ignored.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub creatures: Vec<Creature>,
}

#[derive(Debug, Deserialize)]
pub struct Creature {
    pub id: usize,
    pub name: String,
    #[serde(default)]
    pub clips: Vec<Clip>,
}

#[derive(Debug, Deserialize)]
pub struct Clip {
    pub name: String,
    #[serde(default)]
    pub seconds: f64,
    /// Path relative to the folder root, e.g. `glb/trex_001_attack.glb`.
    pub glb: String,
}

/// An opened export folder.
#[derive(Debug)]
pub struct Catalog {
    pub root: PathBuf,
    pub creatures: Vec<Creature>,
    /// True when the entries were found by scanning instead of read from a manifest.
    pub scanned: bool,
}

impl Catalog {
    /// Opens `folder`, using its `manifest.json` when there is one and scanning
    /// for models when there is not.
    pub fn open(folder: &Path) -> Result<Self> {
        // The root is displayed, used to resolve clips and remembered for next
        // launch, so it has to be absolute: `.\externals` means something else
        // from another working directory.
        let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
        let folder = folder.as_path();

        if !folder.is_dir() {
            bail!("{} is not a folder", folder.display());
        }

        let manifest = folder.join("manifest.json");
        if manifest.is_file() {
            return Self::from_manifest(folder, &manifest);
        }

        Self::scan(folder)
    }

    fn from_manifest(folder: &Path, manifest: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let data: Manifest = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", manifest.display()))?;

        if data.creatures.is_empty() {
            bail!("{} lists no creatures", manifest.display());
        }

        Ok(Self {
            root: folder.to_path_buf(),
            creatures: data.creatures,
            scanned: false,
        })
    }

    fn scan(folder: &Path) -> Result<Self> {
        let mut files = Vec::new();
        collect_models(folder, folder, 0, &mut files)?;

        if files.is_empty() {
            bail!(
                "{} has no manifest.json and no .glb or .gltf files",
                folder.display()
            );
        }
        files.sort();

        let creatures = files
            .iter()
            .enumerate()
            .map(|(index, relative)| {
                let name = relative
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| relative.display().to_string());
                Creature {
                    id: index + 1,
                    name: name.clone(),
                    clips: vec![Clip {
                        name,
                        seconds: 0.0,
                        glb: relative.to_string_lossy().into_owned(),
                    }],
                }
            })
            .collect();

        Ok(Self {
            root: folder.to_path_buf(),
            creatures,
            scanned: true,
        })
    }

    pub fn clip_count(&self) -> usize {
        self.creatures
            .iter()
            .map(|creature| creature.clips.len())
            .sum()
    }

    pub fn clip_path(&self, creature: usize, clip: usize) -> Option<PathBuf> {
        let clip = self.creatures.get(creature)?.clips.get(clip)?;
        Some(self.root.join(&clip.glb))
    }

    /// The first entry that actually has a clip, which is what a freshly opened
    /// folder shows.
    pub fn first_clip(&self) -> Option<(usize, usize)> {
        self.creatures
            .iter()
            .position(|creature| !creature.clips.is_empty())
            .map(|creature| (creature, 0))
    }

    /// `T-Rex - attack`, for window titles and error messages.
    pub fn label(&self, creature: usize, clip: usize) -> Option<String> {
        let creature = self.creatures.get(creature)?;
        let clip = creature.clips.get(clip)?;
        Some(format!("{} - {}", creature.name, clip.name))
    }

    /// The clip's own name, for carrying a clip choice between creatures.
    pub fn clip_name(&self, creature: usize, clip: usize) -> Option<&str> {
        Some(self.creatures.get(creature)?.clips.get(clip)?.name.as_str())
    }

    /// Where `name` sits in this creature's clip list, ignoring case.
    pub fn clip_index(&self, creature: usize, name: &str) -> Option<usize> {
        let needle = name.to_ascii_lowercase();
        self.creatures
            .get(creature)?
            .clips
            .iter()
            .position(|clip| clip.name.to_ascii_lowercase() == needle)
    }

    /// Rows for `--info`.
    pub fn summary(&self) -> Vec<(String, String)> {
        let creatures = if self.scanned {
            format!("{} (scanned)", self.creatures.len())
        } else {
            self.creatures.len().to_string()
        };

        let mut rows = vec![
            ("folder".to_string(), self.root.display().to_string()),
            ("creatures".to_string(), creatures),
            ("clips".to_string(), self.clip_count().to_string()),
        ];
        if let Some(label) = self
            .first_clip()
            .and_then(|(creature, clip)| self.label(creature, clip))
        {
            rows.push(("first".to_string(), label));
        }
        rows
    }
}

/// Collects model paths relative to `root`. Recursion stops at `SCAN_DEPTH` and
/// `SCAN_LIMIT` so a dropped `C:\` cannot hang the window.
fn collect_models(root: &Path, folder: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<()> {
    if depth > SCAN_DEPTH || out.len() >= SCAN_LIMIT {
        return Ok(());
    }

    let entries = std::fs::read_dir(folder).with_context(|| format!("reading {}", folder.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", folder.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }

        // `file_type` does not follow symlinks, so a link loop cannot trap the walk.
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading {}", path.display()))?;

        if file_type.is_dir() {
            collect_models(root, &path, depth + 1, out)?;
        } else if is_model(&path) {
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }

        if out.len() >= SCAN_LIMIT {
            break;
        }
    }

    Ok(())
}

fn is_model(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("glb" | "gltf")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder that deletes itself, so the tests need no `tempfile`.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "ff-view-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create scratch folder");
            Self(path)
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create scratch parent");
            }
            std::fs::write(path, contents).expect("write scratch file");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn manifest_entries_resolve_against_the_folder() {
        let scratch = Scratch::new("manifest");
        scratch.write(
            "manifest.json",
            r#"{
                "creatures": [
                    {
                        "id": 7,
                        "name": "T-Rex",
                        "textures": [{"name": "din001_a", "file": "textures/trex_001/din001_a.png"}],
                        "clips": [
                            {"slot": 1, "name": "attack", "frames": 250, "seconds": 4.0, "glb": "glb/trex_001_attack.glb"},
                            {"slot": 3, "name": "roar", "frames": 90, "seconds": 1.5, "glb": "glb/trex_001_roar.glb"}
                        ]
                    }
                ]
            }"#,
        );

        let catalog = Catalog::open(&scratch.0).expect("open manifest folder");
        assert!(!catalog.scanned);
        assert_eq!(catalog.creatures.len(), 1);
        assert_eq!(catalog.clip_count(), 2);
        assert_eq!(catalog.first_clip(), Some((0, 0)));
        assert_eq!(catalog.label(0, 1).as_deref(), Some("T-Rex - roar"));
        assert_eq!(
            catalog.clip_path(0, 1).unwrap(),
            scratch.0.join("glb").join("trex_001_roar.glb")
        );
    }

    #[test]
    fn a_folder_without_a_manifest_lists_its_models() {
        let scratch = Scratch::new("scan");
        scratch.write("glb/trex_001_attack.glb", "");
        scratch.write("glb/trex_001_roar.glb", "");
        scratch.write("notes.txt", "not a model");

        let catalog = Catalog::open(&scratch.0).expect("scan folder");
        assert!(catalog.scanned);
        assert_eq!(catalog.creatures.len(), 2);
        assert_eq!(catalog.label(0, 0).as_deref(), Some("trex_001_attack - trex_001_attack"));
        assert_eq!(
            catalog.clip_path(0, 0).unwrap(),
            scratch.0.join("glb").join("trex_001_attack.glb")
        );
    }

    #[test]
    fn an_empty_folder_is_an_error() {
        let scratch = Scratch::new("empty");
        let error = Catalog::open(&scratch.0).unwrap_err().to_string();
        assert!(error.contains("no .glb"), "unexpected error: {error}");
    }

    #[test]
    fn a_file_is_not_a_folder() {
        let scratch = Scratch::new("file");
        scratch.write("model.glb", "");
        let error = Catalog::open(&scratch.0.join("model.glb"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("is not a folder"), "unexpected error: {error}");
    }
}
