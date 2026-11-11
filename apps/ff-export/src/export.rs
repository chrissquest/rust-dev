//! The export-all pipeline, callable from a terminal *and* from a window.
//!
//! This used to live in `main.rs`, where every step printed as it went: right for a CLI, useless for
//! a GUI. It now reports [`Progress`] and returns an [`ExportReport`], and `main.rs` turns those back
//! into the table it has always printed.
//!
//! A run that has per-clip failures still returns `Ok`: the report carries them, and the caller
//! decides whether that is fatal (the CLI exits non-zero; a window just shows them).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::catalog;
use crate::gpu::{self, UvOrientation};
use crate::manifest;
use crate::{anim, gltf_out, mar, mesh, nds, texture};

/// What to export.
pub struct ExportRequest {
    pub rom: PathBuf,
    pub out_dir: PathBuf,
    pub orientation: UvOrientation,
}

/// One creature's row in the summary.
#[derive(Clone, Debug)]
pub struct CreatureRow {
    pub id: usize,
    pub name: String,
    pub triangles: usize,
    pub clips: usize,
    pub bones: usize,
    pub sheets: usize,
    pub bytes: usize,
    pub labels: Vec<String>,
}

/// What one run produced.
#[derive(Clone, Debug, Default)]
pub struct ExportReport {
    pub creatures: usize,
    pub clips: usize,
    pub sheets: usize,
    pub bytes: usize,
    /// Files a previous run left behind, now removed.
    pub stale: usize,
    /// Creatures that share a name (the game's table repeats two of them).
    pub repeats: Vec<String>,
    /// One line per clip that did not build, verify or write.
    pub failures: Vec<String>,
    pub manifest: PathBuf,
    pub elapsed: Duration,
}

/// How far along a run is.
pub enum Progress {
    /// The manifest is parsed, and `creatures` of them will be visited.
    Started { creatures: usize },
    /// A creature is about to be exported.
    Creature {
        index: usize,
        total: usize,
        id: usize,
        name: String,
    },
    /// One of its clips is about to be built (`index` is 0-based within the creature).
    Clip {
        index: usize,
        total: usize,
        label: String,
    },
    /// A creature finished; `row` is what the summary table shows.
    CreatureDone(CreatureRow),
}

/// One clip's animation, as a summary.
#[derive(Clone, Debug)]
pub struct ClipSummary {
    pub name: String,
    pub frames: usize,
}

/// One material group, as an export reports it.
#[derive(Clone, Debug)]
pub struct MaterialSummary {
    pub name: String,
    /// The image it uses: width, height, and whether that image carries a colour key.
    pub image: Option<(usize, usize, bool)>,
}

/// Everything one creature's export produced: the file, and what went into it.
pub struct CreatureExport {
    /// The `.glb`, ready to write.
    pub bytes: Vec<u8>,
    pub bones: usize,
    /// Vertices before the per-corner UV split, i.e. what the mesh's own vertex list holds.
    pub authored_vertices: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub groups: usize,
    /// Whether the winding had to be reversed so front faces point outwards.
    pub reversed: bool,
    pub bounds: Option<([f64; 3], [f64; 3])>,
    pub uv_bounds: Option<([f64; 2], [f64; 2])>,
    pub clips: Vec<ClipSummary>,
    pub materials: Vec<MaterialSummary>,
    /// The creature's texture set, decoded: the loose PNGs and the catalog are built from it.
    pub textures: texture::Textures,
}

/// The name a clip carries in the glb.
fn clip_label(slot: usize) -> String {
    let label = manifest::clip_label(slot);
    if label == "unused" {
        format!("clip-{slot}")
    } else {
        label.to_string()
    }
}

/// Builds one creature's `.glb`, plus everything worth reporting about it.
///
/// `wanted` is all of a creature's clips, or a single one. Both `export` and `export-all` come through
/// here, so a file written by one is byte for byte a file written by the other.
pub fn build_creature(
    archive: &mar::Archive<'_>,
    creature: &manifest::Creature,
    name: &str,
    orientation: UvOrientation,
    wanted: &[&manifest::Clip],
) -> Result<CreatureExport> {
    // The mesh and the sheets come from the first wanted clip rather than from the creature: every clip
    // in this game names its own mesh, and those meshes carry different bind poses - up to 14 units apart
    // between two clips of the same creature - so a mesh only makes sense with its own animation. Callers
    // pass one clip per call, which is also why the export writes one file per clip.
    let mesh_clip = wanted
        .first()
        .with_context(|| format!("creature {} has no clips to export", creature.id))?;

    let decoded = mesh::parse(&archive.decompressed(mesh_clip.mesh.index as usize)?)?;
    let stream = gpu::parse_stream(&decoded.commands)?;
    let commands = stream.gpu_commands()?;
    let mut geometry = gpu::build_geometry(&decoded, stream.world_root_bone_count()?, commands)?;

    // glTF wants the coordinates exactly as the DS had them, so this is a no-op unless `--uv-flip`
    // asks for one.
    if orientation != gpu::UvOrientation::Ds {
        for uv in &mut geometry.uvs {
            *uv = orientation.apply(*uv);
        }
    }

    let mut animations = Vec::with_capacity(wanted.len());
    let mut clips = Vec::with_capacity(wanted.len());
    for clip in wanted {
        let animation = anim::parse(&archive.decompressed(clip.animation.index as usize)?)?;
        clips.push(ClipSummary {
            name: clip_label(clip.slot),
            frames: animation.frame_count,
        });
        animations.push(animation);
    }

    let clip_sources: Vec<gltf_out::Clip<'_>> = clips
        .iter()
        .zip(&animations)
        .map(|(clip, animation)| gltf_out::Clip {
            name: clip.name.as_str(),
            animation,
        })
        .collect();

    // A group's palette base is the key the texture set's images are named by, so that is how a
    // material finds its image.
    let textures = texture::parse(&archive.decompressed(mesh_clip.texture.index as usize)?)?;
    let by_key = textures.by_material_key();

    let materials: Vec<MaterialSummary> = geometry
        .groups
        .iter()
        .map(|group| {
            let image = group.palette_base.and_then(|key| by_key.get(&key).copied());
            MaterialSummary {
                name: match image {
                    Some(image) => image.name.clone(),
                    None => match group.palette_base {
                        Some(key) => format!("material_{key}"),
                        None => "material_none".to_string(),
                    },
                },
                image: image.map(|image| (image.width, image.height, image.transparent)),
            }
        })
        .collect();

    let sources: Vec<gltf_out::MaterialSource<'_>> = materials
        .iter()
        .zip(&geometry.groups)
        .map(|(material, group)| gltf_out::MaterialSource {
            name: material.name.as_str(),
            image: group
                .palette_base
                .and_then(|key| by_key.get(&key).copied())
                .map(|image| gltf_out::ImageSource {
                    width: image.width as u32,
                    height: image.height as u32,
                    pixels: &image.pixels,
                    alpha_mask: image.transparent,
                }),
        })
        .collect();

    let (bytes, split) = gltf_out::build(&decoded, &geometry, name, &clip_sources, &sources)?;

    Ok(CreatureExport {
        bytes,
        bones: decoded.bones.len(),
        authored_vertices: geometry.positions.len(),
        vertices: split.positions.len(),
        triangles: split.groups.iter().map(Vec::len).sum::<usize>() / 3,
        groups: split.groups.len(),
        reversed: split.reversed,
        bounds: geometry.bounds(),
        uv_bounds: geometry.uv_bounds(),
        clips,
        materials,
        textures,
    })
}

/// Exports every creature: a `.glb` each, its loose texture sheets, and the catalog that lists them.
///
/// Every file is imported again in memory before it is written, so "exported" also means "and the
/// result loads", and the catalog is written last so it only ever describes files that are on disk.
pub fn export_all(
    request: &ExportRequest,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<ExportReport> {
    let rom = nds::Rom::open(&request.rom)?;
    let (archive, _) = crate::game::open_archive(&rom, "model/battle/arcdin")?;
    let parsed = manifest::parse(&archive.decompressed(0)?)?;
    let names = crate::game::creature_names(&rom)
        .context("the creature name table is needed to name the files")?;

    std::fs::create_dir_all(&request.out_dir)
        .with_context(|| format!("creating {}", request.out_dir.display()))?;

    let started = Instant::now();
    let mut report = ExportReport {
        manifest: request.out_dir.join("manifest.json"),
        ..ExportReport::default()
    };

    let mut creatures = Vec::with_capacity(parsed.creatures.len());
    let mut slugs: BTreeMap<String, usize> = BTreeMap::new();
    let mut bytes_written = 0usize;
    let mut sheets = 0usize;
    let mut files = 0usize;

    on_progress(Progress::Started {
        creatures: parsed.creatures.len(),
    });

    for (index, creature) in parsed.creatures.iter().enumerate() {
        let name = names
            .get(creature.id - 1)
            .cloned()
            .unwrap_or_else(|| format!("vivosaur_{:03}", creature.id));

        on_progress(Progress::Creature {
            index,
            total: parsed.creatures.len(),
            id: creature.id,
            name: name.clone(),
        });

        // The game's own name table repeats two names (Frigi and Igno appear twice), which is why the
        // creature's id is part of every file name. Two creatures sharing a name is therefore worth
        // reporting rather than failing on.
        if let Some(previous) = slugs.insert(catalog::slug(&name), creature.id) {
            report.repeats.push(format!(
                "creatures {previous} and {} are both `{name}`",
                creature.id
            ));
        }

        let mut clips = Vec::with_capacity(creature.clips.len());
        let mut textures = Vec::new();
        let mut labels = Vec::with_capacity(creature.clips.len());
        let mut creature_bytes = 0usize;
        let mut first_triangles = 0usize;
        let mut first_bones = 0usize;

        for (clip_index, clip) in creature.clips.iter().enumerate() {
            let wanted = [clip];
            let label = clip_label(clip.slot);
            let reported = format!(
                "creature {} {name} clip {} ({label})",
                creature.id, clip.slot
            );
            labels.push(label.clone());

            on_progress(Progress::Clip {
                index: clip_index,
                total: creature.clips.len(),
                label: label.clone(),
            });

            let export =
                match build_creature(&archive, creature, &name, request.orientation, &wanted) {
                    Ok(export) => export,
                    Err(error) => {
                        report.failures.push(format!("{reported}: {error:#}"));
                        continue;
                    }
                };

            // Imported again before it is kept, so "exported" also means "and it loads".
            match verify_creature(&export, &name)
                .and_then(|()| write_clip(&request.out_dir, creature.id, &name, &label, &export))
            {
                Ok(()) => {}
                Err(error) => {
                    report.failures.push(format!("{reported}: {error:#}"));
                    continue;
                }
            }

            // The sheets belong to the creature rather than to the clip, so they are written once.
            if textures.is_empty() {
                textures = export
                    .textures
                    .images
                    .iter()
                    .map(|image| catalog::Texture {
                        name: image.name.clone(),
                        file: catalog::texture_path(creature.id, &name, &image.name),
                        width: image.width,
                        height: image.height,
                        format: image.format.name().to_string(),
                        transparent: image.transparent,
                    })
                    .collect();
                if let Err(error) = write_textures(&request.out_dir, creature.id, &name, &export) {
                    report
                        .failures
                        .push(format!("creature {} {name} sheets: {error:#}", creature.id));
                    continue;
                }
                sheets += export.textures.images.len();
            }

            if clips.is_empty() {
                first_triangles = export.triangles;
                first_bones = export.bones;
            }

            bytes_written += export.bytes.len();
            creature_bytes += export.bytes.len();
            files += 1;

            clips.push(catalog::Clip {
                slot: clip.slot,
                name: label.clone(),
                frames: export.clips[0].frames,
                seconds: export.clips[0].frames as f64 / 60.0,
                glb: catalog::glb_path(creature.id, &name, &label),
                mesh_entry: clip.mesh.index,
                animation_entry: clip.animation.index,
                texture_entry: clip.texture.index,
                bones: export.bones,
                vertices: export.vertices,
                triangles: export.triangles,
                material_groups: export.groups,
                bounds: bounds_of(&export),
            });
        }

        let row = CreatureRow {
            id: creature.id,
            name: name.clone(),
            triangles: first_triangles,
            clips: clips.len(),
            bones: first_bones,
            sheets: textures.len(),
            bytes: creature_bytes,
            labels: labels.clone(),
        };

        let slug = catalog::slug(&name);
        creatures.push(catalog::Creature {
            id: creature.id,
            name,
            slug,
            textures,
            clips,
        });

        on_progress(Progress::CreatureDone(row));
    }

    let catalog = catalog::Catalog {
        generator: "ff-export".to_string(),
        source: catalog::Source {
            rom: request
                .rom
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| request.rom.display().to_string()),
            archive: "model/battle/arcdin".to_string(),
            creature_count: creatures.len(),
            clip_count: files,
        },
        units: catalog::Catalog::units(),
        layout: catalog::Catalog::layout(),
        creatures,
    };

    // Anything a previous run left behind goes first, so the manifest describes the whole directory.
    let stale = prune_output(&request.out_dir, &catalog)?;
    std::fs::write(&report.manifest, catalog.to_json()?)
        .with_context(|| format!("writing {}", report.manifest.display()))?;

    report.creatures = catalog.source.creature_count;
    report.clips = files;
    report.sheets = sheets;
    report.bytes = bytes_written;
    report.stale = stale;
    report.elapsed = started.elapsed();

    Ok(report)
}

/// One clip's `.glb`, at its catalog path.
fn write_clip(
    out_dir: &Path,
    id: usize,
    name: &str,
    clip: &str,
    export: &CreatureExport,
) -> Result<()> {
    let path = out_dir.join(catalog::glb_path(id, name, clip));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, &export.bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// A creature's texture sheets, as loose PNGs, written once for all of its clips.
fn write_textures(out_dir: &Path, id: usize, name: &str, export: &CreatureExport) -> Result<()> {
    let sheets = out_dir.join(catalog::texture_dir(id, name));
    std::fs::create_dir_all(&sheets).with_context(|| format!("creating {}", sheets.display()))?;
    for image in &export.textures.images {
        let path = sheets.join(format!("{}.png", image.name));
        std::fs::write(&path, image.to_png()?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Removes files under `glb/` and `textures/` that this run did not write, and reports how many.
///
/// An export should be exactly what `manifest.json` describes: re-running one after a name changed would
/// otherwise leave the earlier file behind, which matters when the output is zipped and shipped.
fn prune_output(out_dir: &Path, catalog: &catalog::Catalog) -> Result<usize> {
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for creature in &catalog.creatures {
        for clip in &creature.clips {
            expected.insert(clip.glb.clone());
        }
        for texture in &creature.textures {
            expected.insert(texture.file.clone());
        }
    }

    let mut removed = 0usize;
    for folder in ["glb", "textures"] {
        let mut files = Vec::new();
        files_under(&out_dir.join(folder), &mut files)?;
        for file in files {
            let relative = file
                .strip_prefix(out_dir)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            if !expected.contains(&relative) {
                std::fs::remove_file(&file)
                    .with_context(|| format!("removing {}", file.display()))?;
                removed += 1;
            }
        }
    }

    Ok(removed)
}

/// Every file under `dir`, recursively.
fn files_under(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            files_under(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// An export's extents as a catalog entry.
fn bounds_of(export: &CreatureExport) -> catalog::Bounds {
    match export.bounds {
        Some((min, max)) => catalog::Bounds {
            min,
            max,
            size: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
        },
        // a mesh with no vertices cannot be exported, but an entry still needs numbers
        None => catalog::Bounds {
            min: [0.0; 3],
            max: [0.0; 3],
            size: [0.0; 3],
        },
    }
}

/// Imports a built `.glb` again, so that "exported" means "and it loads".
///
/// This is the cheap half of what the Khronos validator checks: if the importer accepts the file and
/// the counts inside it match what went in, the export is structurally the one that was intended.
fn verify_creature(export: &CreatureExport, name: &str) -> Result<()> {
    let (document, _buffers, images) =
        gltf::import_slice(&export.bytes).with_context(|| format!("re-importing `{name}`"))?;

    let animations: Vec<&str> = document
        .animations()
        .map(|animation| animation.name().unwrap_or_default())
        .collect();
    let expected: Vec<&str> = export.clips.iter().map(|clip| clip.name.as_str()).collect();
    if animations != expected {
        bail!("`{name}` came back with animations {animations:?}, expected {expected:?}");
    }

    if images.len() != document.images().count() {
        bail!(
            "`{name}` came back with {} images for {} entries",
            images.len(),
            document.images().count()
        );
    }
    if images.is_empty() {
        bail!("`{name}` has no images at all");
    }
    // Matched by name, not by position: images appear in material order, and only the name says which
    // sheet an image is. Verifying the name too is the point - a mis-named image would be a bug the
    // browser would have to live with.
    for (image, data) in document.images().zip(&images) {
        let image_name = image.name().unwrap_or_default();
        let source = export
            .textures
            .images
            .iter()
            .find(|source| source.name == image_name)
            .with_context(|| {
                format!("`{image_name}` is in the glb but not in this creature's texture set")
            })?;
        if data.width as usize != source.width || data.height as usize != source.height {
            bail!(
                "`{name}`: `{image_name}` came back {}x{}, expected {}x{}",
                data.width,
                data.height,
                source.width,
                source.height
            );
        }
    }

    let mesh = document.meshes().next().context("the glb has no mesh")?;
    let primitives = mesh.primitives().count();
    if primitives != export.groups {
        bail!(
            "`{name}` came back with {primitives} primitives, expected {}",
            export.groups
        );
    }

    let joints = document
        .skins()
        .next()
        .map(|skin| skin.joints().count())
        .unwrap_or(0);
    if joints != export.bones {
        bail!(
            "`{name}` came back with {joints} joints, expected {}",
            export.bones
        );
    }

    Ok(())
}

/// A byte count a person can read at a glance.
pub fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}
