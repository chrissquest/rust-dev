//! `cos-convert` - turns the Cycle of Steel Unity project into the Rust port's assets.
//!
//! It reads Unity's text files directly, so Unity does not need to be installed.
//! It is meant to be run once per kind of asset: after that, the files it wrote are
//! the source of truth and are edited directly (or by the room editor, later).

mod json;
mod project;
mod room;
mod yaml;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;

use project::{Project, SpriteInfo};

const USAGE: &str = "\
cos-convert - Cycle of Steel Unity project -> Rust port assets

USAGE:
    cos-convert <unity-project> <assets-folder>

    <unity-project>   the folder holding the Unity project's Assets/ and ProjectSettings/
    <assets-folder>   where to write, e.g. apps/cycle-of-steel/assets

WRITES:
    rooms/<room>.json     one file per prefab under Assets/Prefabs/Generatable Rooms
    sprites.json          every sprite those rooms use: texture, rectangle, pivot
    sprites/<name>.png    the textures themselves, copied
";

/// Where the room prefabs live, relative to the Unity `Assets` folder.
const ROOMS_FOLDER: &str = "Prefabs/Generatable Rooms";
/// The one room the dungeon generator never places. It is larger than a dungeon
/// room: the hub, the exit corridor beside it and the debug room above it.
const OFF_GRID_ROOM: &str = "Hub Room";

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [unity_root, output] = arguments.as_slice() else {
        print!("{USAGE}");
        if arguments.iter().any(|argument| argument == "-h" || argument == "--help") {
            return Ok(());
        }
        bail!("expected a Unity project folder and an output folder");
    };
    let output = PathBuf::from(output);

    let project = Project::open(Path::new(unity_root))?;
    let room_paths = find_prefabs(&project.assets_dir().join(ROOMS_FOLDER), project.assets_dir())?;
    if room_paths.is_empty() {
        bail!("no room prefabs under {ROOMS_FOLDER}");
    }

    let rooms_dir = output.join("rooms");
    std::fs::create_dir_all(&rooms_dir).with_context(|| format!("could not create {}", rooms_dir.display()))?;

    // Sprite name -> sprite, over every room. A BTreeMap so the catalogue is sorted.
    let mut catalogue: BTreeMap<String, SpriteInfo> = BTreeMap::new();
    let mut file_names: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut warning_count = 0;

    println!("{:<16} {:>6} {:>6} {:>6} {:>7} {:>6}", "room", "layers", "tiles", "decor", "objects", "lights");
    for relative in &room_paths {
        let name = relative
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file_name = slug(&name);
        if let Some(other) = file_names.insert(file_name.clone(), relative.clone()) {
            bail!(
                "{} and {} would both be written as {file_name}.json",
                other.display(),
                relative.display()
            );
        }

        let converted = room::convert(&project, relative, &name, name != OFF_GRID_ROOM)
            .with_context(|| format!("could not convert {}", relative.display()))?;
        let path = rooms_dir.join(format!("{file_name}.json"));
        std::fs::write(&path, json::to_string(&converted.json))
            .with_context(|| format!("could not write {}", path.display()))?;

        let summary = &converted.summary;
        println!(
            "{:<16} {:>6} {:>6} {:>6} {:>7} {:>6}",
            file_name, summary.layers, summary.tiles, summary.decorations, summary.objects, summary.lights
        );
        for warning in &converted.warnings {
            println!("    warning: {warning}");
        }
        warning_count += converted.warnings.len();

        for sprite in converted.sprites {
            match catalogue.get(&sprite.name) {
                Some(existing) if *existing != sprite => bail!(
                    "two different sprites are both named `{}` ({} and {})",
                    sprite.name,
                    existing.texture.display(),
                    sprite.texture.display()
                ),
                Some(_) => {}
                None => {
                    catalogue.insert(sprite.name.clone(), sprite);
                }
            }
        }
    }

    let texture_count = write_sprites(&project, &output, &catalogue)?;
    println!(
        "\n{} rooms, {} sprites from {} textures, {} warnings -> {}",
        room_paths.len(),
        catalogue.len(),
        texture_count,
        warning_count,
        output.display()
    );
    Ok(())
}

/// Every `.prefab` under `folder`, relative to `assets`, in a stable order.
fn find_prefabs(folder: &Path, assets: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries =
            std::fs::read_dir(&current).with_context(|| format!("could not list {}", current.display()))?;
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "prefab") {
                found.push(path.strip_prefix(assets).unwrap_or(&path).to_path_buf());
            }
        }
    }
    found.sort();
    Ok(found)
}

/// `Hub Room` -> `hub_room`: a file name with no spaces or capitals to trip over.
fn slug(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Writes `sprites.json` and copies the textures it names. Returns how many.
fn write_sprites(project: &Project, output: &Path, catalogue: &BTreeMap<String, SpriteInfo>) -> Result<usize> {
    let sprites_dir = output.join("sprites");
    std::fs::create_dir_all(&sprites_dir)
        .with_context(|| format!("could not create {}", sprites_dir.display()))?;

    // Textures are flattened into one folder, so two sharing a file name would clash.
    let mut copied: BTreeMap<String, &Path> = BTreeMap::new();
    let mut entries = serde_json::Map::new();
    for sprite in catalogue.values() {
        let texture_name = sprite
            .texture
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        match copied.get(&texture_name) {
            Some(other) if *other != sprite.texture => bail!(
                "{} and {} would both be copied as {texture_name}.png",
                other.display(),
                sprite.texture.display()
            ),
            Some(_) => {}
            None => {
                let from = project.assets_dir().join(&sprite.texture);
                let to = sprites_dir.join(format!("{texture_name}.png"));
                std::fs::copy(&from, &to)
                    .with_context(|| format!("could not copy {} to {}", from.display(), to.display()))?;
                copied.insert(texture_name.clone(), &sprite.texture);
            }
        }
        entries.insert(
            sprite.name.clone(),
            json!({
                "texture": texture_name,
                "rect": sprite.rect,
                "pivot": json::pair(sprite.pivot[0], sprite.pivot[1]),
                "pixels_per_unit": json::number(sprite.pixels_per_unit),
            }),
        );
    }

    let path = output.join("sprites.json");
    std::fs::write(&path, json::to_string(&json!({ "sprites": entries })))
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(copied.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lowercase_with_no_spaces() {
        assert_eq!(slug("Hub Room"), "hub_room");
        assert_eq!(slug("4-1"), "4-1");
        assert_eq!(slug("NSE1"), "nse1");
    }
}
