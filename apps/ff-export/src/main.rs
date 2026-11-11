//! `ff-export` command line: inspect a ROM, unpack archives, diff against a
//! known-good corpus.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use ff_export::{anim, catalog, gltf_out, gpu, manifest, mar, mcm, mesh, names, nds, texture};

const USAGE: &str = "\
ff-export - Fossil Fighters (NDS) asset extraction

USAGE:
    ff-export <command> [arguments]

COMMANDS:
    info    <rom>                       ROM header, file and archive counts
    list    <rom> [substring]           list ROM files (id, size, path)
    unpack  <rom> <path> <outdir>       decompress one MAR archive into NNNN.bin
    verify  <rom> <path> <refdir>       diff that output against a reference tree
    manifest <rom> [limit]              creature -> mesh/animation/texture mapping
    names   <rom> [all]                 the 116 creature id -> name pairs
    mesh    <rom> <creature> [clip]     parse one creature mesh and report its geometry
    obj     <rom> <creature> [clip] <file.obj>   write the decoded mesh as Wavefront OBJ
    export  <rom> <creature> [clip] <file.glb>   write mesh, skeleton and clips as .glb (all clips unless one is given)
    textures <rom> <creature> <outdir>           write a creature's texture images out as loose PNGs
    export-all <rom> <outdir>                    export every creature: glb/ + textures/ + manifest.json
    anim    <rom> <creature> <clip>              inspect one clip's animation block
    hexdump <rom> <path> <entry> [len]  raw stored bytes of one entry, for analysis
    help                                show this text

FLAGS:
    --uv-flip <none|v|u|swap>           texture coordinate orientation for `export` and `mesh`.
                                        `none` is the default: the DS samples from the top-left with
                                        V running down, exactly like glTF. The others exist to A/B
                                        against the reference tool, which flips V for OBJ and USD.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let result = match args.first().map(String::as_str) {
        Some("info") => cmd_info(&args[1..]),
        Some("list") => cmd_list(&args[1..]),
        Some("unpack") => cmd_unpack(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some("hexdump") => cmd_hexdump(&args[1..]),
        Some("manifest") => cmd_manifest(&args[1..]),
        Some("names") => cmd_names(&args[1..]),
        Some("mesh") => cmd_mesh(&args[1..]),
        Some("obj") => cmd_obj(&args[1..]),
        Some("export") => cmd_export(&args[1..]),
        Some("textures") => cmd_textures(&args[1..]),
        Some("export-all") => cmd_export_all(&args[1..]),
        Some("anim") => cmd_anim(&args[1..]),
        Some("help") | None | Some("-h") | Some("--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(anyhow::anyhow!("unknown command `{other}`\n\n{USAGE}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn arg(args: &[String], index: usize, name: &str) -> Result<String> {
    match args.get(index) {
        Some(value) => Ok(value.clone()),
        None => Err(anyhow::anyhow!("missing <{name}> argument\n\n{USAGE}")),
    }
}

/// Pulls `--uv-flip <mode>` (or `--uv-flip=<mode>`) out of the arguments, returning the rest.
///
/// Texture coordinate orientation is a diagnostic, not something every run needs, so it is a flag
/// rather than a positional argument - and it has to be removed before the positional parsing counts.
fn take_uv_flip(args: &[String]) -> Result<(Vec<String>, gpu::UvOrientation)> {
    let mut rest = Vec::with_capacity(args.len());
    let mut orientation = gpu::UvOrientation::Ds;
    let mut index = 0;

    while index < args.len() {
        let argument = args[index].as_str();

        let value = if let Some(value) = argument.strip_prefix("--uv-flip=") {
            value.to_string()
        } else if argument == "--uv-flip" {
            index += 1;
            args.get(index).context("--uv-flip needs a value")?.clone()
        } else {
            rest.push(args[index].clone());
            index += 1;
            continue;
        };

        orientation = gpu::UvOrientation::parse(&value)
            .with_context(|| format!("--uv-flip must be none, v, u or swap, not `{value}`"))?;
        index += 1;
    }

    Ok((rest, orientation))
}

fn is_mar(rom: &nds::Rom, file: &nds::RomFile) -> bool {
    match rom.magic(file) {
        Ok(magic) => &magic[0..3] == b"MAR",
        Err(_) => false,
    }
}

/// Reference corpora name entries either `12.bin` or `0012.bin`; accept both.
fn reference_path(ref_dir: &std::path::Path, index: usize) -> Option<PathBuf> {
    for name in [format!("{index}.bin"), format!("{index:04}.bin")] {
        let candidate = ref_dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn cmd_info(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let rom = nds::Rom::open(&rom_path)?;
    let header = rom.header();

    println!("rom          : {rom_path}");
    println!(
        "title        : {:<12} game code {:<4} maker code {}",
        header.title, header.game_code, header.maker_code
    );
    println!(
        "size         : {} bytes ({:.1} MiB)",
        header.file_size,
        header.file_size as f64 / (1024.0 * 1024.0)
    );
    println!(
        "FNT          : {:#010x} + {:#x}",
        header.fnt_offset, header.fnt_size
    );
    println!(
        "FAT          : {:#010x} + {:#x}",
        header.fat_offset, header.fat_size
    );
    println!(
        "arm9 / arm7  : {:#x}+{:#x} / {:#x}+{:#x}",
        header.arm9_offset, header.arm9_size, header.arm7_offset, header.arm7_size
    );
    println!("files        : {}", rom.files().len());

    let archives = rom.files().iter().filter(|f| is_mar(&rom, f)).count();
    let bytes: u64 = rom.files().iter().map(|f| f.size as u64).sum();
    println!("MAR archives : {archives}");
    println!(
        "file bytes   : {} ({:.1} MiB)",
        bytes,
        bytes as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}

fn cmd_list(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let filter = args.get(1).cloned().unwrap_or_default();
    let rom = nds::Rom::open(&rom_path)?;

    let mut shown = 0usize;
    for file in rom.files() {
        if !filter.is_empty() && !file.path.contains(&filter) {
            continue;
        }
        let magic = rom.magic(file).unwrap_or([0; 4]);
        let tag = if &magic[0..3] == b"MAR" { "MAR" } else { "" };
        println!("{:>5}  {:>9}  {:<4} {}", file.id, file.size, tag, file.path);
        shown += 1;
    }
    println!("--- {shown} of {} file(s) ---", rom.files().len());
    Ok(())
}

/// Opens a ROM and the archive at `path` inside it.
fn open_archive<'a>(rom: &'a nds::Rom, path: &str) -> Result<(mar::Archive<'a>, &'a nds::RomFile)> {
    let file = rom
        .find(path)
        .with_context(|| format!("`{path}` is not in the ROM"))?;
    let bytes = rom.bytes(file)?;
    let archive = mar::Archive::parse(bytes).with_context(|| format!("parsing `{path}`"))?;
    Ok((archive, file))
}

fn cmd_unpack(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let inner = arg(args, 1, "path")?;
    let out_dir = PathBuf::from(arg(args, 2, "outdir")?);

    let started = Instant::now();
    let rom = nds::Rom::open(&rom_path)?;
    let (archive, file) = open_archive(&rom, &inner)?;
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("creating {}", out_dir.display()))?;

    println!(
        "`{inner}`: {} entr{}, stored {} bytes",
        archive.len(),
        if archive.len() == 1 { "y" } else { "ies" },
        file.size
    );

    let mut codecs: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut written = 0usize;
    let mut produced: u64 = 0;
    let mut failures = 0usize;

    for index in 0..archive.len() {
        let stored = archive.stored(index)?;
        if let Ok(info) = mcm::info(stored) {
            for codec in info.codecs() {
                *codecs.entry(codec.name()).or_default() += 1;
            }
        }

        match archive.decompressed(index) {
            Ok(data) => {
                let path = out_dir.join(format!("{index:04}.bin"));
                std::fs::write(&path, &data)
                    .with_context(|| format!("writing {}", path.display()))?;
                produced += data.len() as u64;
                written += 1;
            }
            Err(error) => {
                failures += 1;
                if failures <= 5 {
                    eprintln!("  entry {index:04}: {error:#}");
                }
            }
        }
    }

    println!("wrote        : {written} file(s), {produced} bytes");
    if failures > 0 {
        println!("failed       : {failures}");
    }
    let codecs: Vec<String> = codecs.iter().map(|(k, v)| format!("{k} x{v}")).collect();
    println!("codecs       : {}", codecs.join(", "));
    println!("elapsed      : {:.2}s", started.elapsed().as_secs_f64());

    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let inner = arg(args, 1, "path")?;
    let ref_dir = PathBuf::from(arg(args, 2, "refdir")?);

    let started = Instant::now();
    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, &inner)?;

    let mut identical = 0usize;
    let mut differing = 0usize;
    let mut missing = 0usize;
    let mut failed = 0usize;

    for index in 0..archive.len() {
        let reference_path = match reference_path(&ref_dir, index) {
            Some(path) => path,
            None => {
                missing += 1;
                if missing <= 5 {
                    eprintln!("  entry {index:04}: no reference in {}", ref_dir.display());
                }
                continue;
            }
        };
        let reference = std::fs::read(&reference_path)
            .with_context(|| format!("reading {}", reference_path.display()))?;

        match archive.decompressed(index) {
            Ok(data) => {
                if data == reference {
                    identical += 1;
                } else {
                    differing += 1;
                    if differing <= 5 {
                        let first = data.iter().zip(reference.iter()).position(|(a, b)| a != b);
                        eprintln!(
                            "  entry {index:04}: differs at byte {:?} (ours {} bytes, reference {} bytes)",
                            first,
                            data.len(),
                            reference.len()
                        );
                    }
                }
            }
            Err(error) => {
                failed += 1;
                if failed <= 5 {
                    eprintln!("  entry {index:04}: {error:#}");
                }
            }
        }
    }

    println!("`{inner}`: {} entries", archive.len());
    println!("identical    : {identical}");
    println!("differing    : {differing}");
    println!("missing ref  : {missing}");
    println!("failed       : {failed}");
    println!("elapsed      : {:.2}s", started.elapsed().as_secs_f64());

    if differing == 0 && missing == 0 && failed == 0 {
        println!("RESULT: BYTE-IDENTICAL to the reference corpus");
    } else {
        println!("RESULT: MISMATCH");
    }

    Ok(())
}

/// Dumps the stored (still compressed) bytes of one entry, plus how its `MCM`
/// header parses. This is the tool for byte-level analysis when a header looks
/// wrong: it shows exactly what the index points at.
fn cmd_hexdump(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let inner = arg(args, 1, "path")?;
    let index: usize = arg(args, 2, "entry")?
        .parse()
        .context("entry index must be a number")?;
    let length: usize = match args.get(3) {
        Some(value) => value.parse().context("length must be a number")?,
        None => 0x40,
    };

    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, &inner)?;
    let stored = archive.stored(index)?;

    println!(
        "entry {index}: stored {} bytes, index declares {} decompressed",
        stored.len(),
        archive.entries[index].decompressed_size
    );

    match mcm::info(stored) {
        Ok(info) => println!(
            "  MCM header: decompressed={} maxChunk={:#x} chunks={} codec1={} codec2={}",
            info.decompressed_size,
            info.max_chunk_size,
            info.chunk_count,
            info.codec1.name(),
            info.codec2.name()
        ),
        Err(error) => println!("  MCM header: {error:#}"),
    }

    let length = length.min(stored.len());
    print!("  bytes:");
    for (i, byte) in stored[..length].iter().enumerate() {
        if i % 16 == 0 {
            print!("\n    ");
        }
        print!(" {byte:02X}");
    }
    println!();

    Ok(())
}

/// Prints the creature -> mesh/animation/texture mapping from the `3CL` manifest.
fn cmd_manifest(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let limit: usize = match args.get(1) {
        Some(value) => value.parse().context("limit must be a number")?,
        None => 8,
    };

    let rom = nds::Rom::open(&rom_path)?;
    let name_table = creature_names(&rom).ok();
    let (archive, _) = open_archive(&rom, "model/battle/arcdin")?;
    let data = archive.decompressed(0)?;
    let parsed = manifest::parse(&data)
        .context("parsing the 3CL manifest in model/battle/arcdin entry 0")?;

    println!(
        "manifest     : {} bytes, {} creatures, {} clips",
        data.len(),
        parsed.creatures.len(),
        parsed.clip_count()
    );

    let mut histogram: BTreeMap<usize, usize> = BTreeMap::new();
    for creature in &parsed.creatures {
        *histogram.entry(creature.clips.len()).or_default() += 1;
    }
    let summary: Vec<String> = histogram
        .iter()
        .map(|(clips, count)| format!("{clips} clips x{count}"))
        .collect();
    println!("clip counts  : {}", summary.join(", "));

    println!();
    println!(
        "{:>4}  {:<12}  {:<14}  mesh / animation / texture",
        "id", "name", "clip slots"
    );
    for creature in parsed.creatures.iter().take(limit) {
        let slots: Vec<String> = creature.clips.iter().map(|c| c.slot.to_string()).collect();
        let first = &creature.clips[0];
        let name = name_table
            .as_ref()
            .and_then(|names| names.get(creature.id - 1))
            .map(String::as_str)
            .unwrap_or("-");
        println!(
            "{:>4}  {:<12}  {:<14}  {}:{} / {}:{} / {}:{}",
            creature.id,
            name,
            slots.join(","),
            first.mesh.table_name,
            first.mesh.index,
            first.animation.table_name,
            first.animation.index,
            first.texture.table_name,
            first.texture.index
        );
    }
    if parsed.creatures.len() > limit {
        println!("      ... {} more", parsed.creatures.len() - limit);
    }

    Ok(())
}

/// Reads the creature name table out of the game's text archive.
fn creature_names(rom: &nds::Rom) -> Result<Vec<String>> {
    let (archive, _) = open_archive(rom, "text/japanese")?;
    let text = archive
        .decompressed(0)
        .context("decompressing text/japanese entry 0")?;
    names::creature_names(&text, names::CREATURE_COUNT)
}

/// Prints the creature id -> name table.
fn cmd_names(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let rom = nds::Rom::open(&rom_path)?;
    let all = creature_names(&rom)?;

    let show_all = args.get(1).map(String::as_str) == Some("all");
    let limit = if show_all { all.len() } else { 12 };

    println!("creatures    : {}", all.len());
    for (index, name) in all.iter().take(limit).enumerate() {
        println!("{:>4}  {}", index + 1, name);
    }
    if limit < all.len() {
        println!("      ... {} more (pass `all` to list every one)", all.len() - limit);
    }

    Ok(())
}

/// Parses one creature's mesh and reports the geometry inside it.
fn cmd_mesh(args: &[String]) -> Result<()> {
    let (arguments, orientation) = take_uv_flip(args)?;
    let args = arguments.as_slice();

    let rom_path = arg(args, 0, "rom")?;
    let creature_id: usize = arg(args, 1, "creature")?
        .parse()
        .context("creature id must be a number")?;
    let clip_slot: Option<usize> = match args.get(2) {
        Some(value) => Some(value.parse().context("clip slot must be a number")?),
        None => None,
    };

    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, "model/battle/arcdin")?;

    let parsed = manifest::parse(&archive.decompressed(0)?)?;
    let creature = parsed
        .find(creature_id)
        .with_context(|| format!("creature {creature_id} is not in the manifest"))?;

    let name = creature_names(&rom)
        .ok()
        .and_then(|all| all.get(creature_id - 1).cloned())
        .unwrap_or_else(|| format!("vivosaur {creature_id}"));

    let clip = match clip_slot {
        Some(slot) => creature.clips.iter().find(|clip| clip.slot == slot).with_context(|| {
            let available: Vec<usize> = creature.clips.iter().map(|clip| clip.slot).collect();
            format!("creature {creature_id} has no clip slot {slot}; it has {available:?}")
        })?,
        None => &creature.clips[0],
    };

    println!("creature     : {creature_id} {name}");
    println!("clip         : {} ({})", clip.slot, manifest::clip_label(clip.slot));
    println!("mesh entry   : {}:{}", clip.mesh.table_name, clip.mesh.index);

    let data = archive.decompressed(clip.mesh.index as usize)?;
    let parsed_mesh = mesh::parse(&data)?;
    println!("mesh bytes   : {}", data.len());
    println!("scale        : {}", parsed_mesh.scale);
    println!("keyframes    : {}", parsed_mesh.keyframe_count);
    println!("bones        : {}", parsed_mesh.bones.len());
    println!(
        "bone names   : {}",
        parsed_mesh
            .bones
            .iter()
            .map(|bone| bone.name.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("model names  : {}", parsed_mesh.model_names.join(" "));

    let stream = gpu::parse_stream(&parsed_mesh.commands)?;
    let world_root = stream.world_root_bone_count()?;
    let commands = stream.gpu_commands()?;
    let mut geometry = gpu::build_geometry(&parsed_mesh, world_root, commands)?;
    if orientation != gpu::UvOrientation::Ds {
        for uv in &mut geometry.uvs {
            *uv = orientation.apply(*uv);
        }
    }

    // What the stream asks of the matrix stack. A matrix in *texture* mode would transform every
    // texture coordinate at once, and one in position mode would move every vertex, so it is worth
    // showing - `matrixRestore` is not, since it runs once per bone. For this game every mesh issues
    // one scale, an identity and two mode switches, and never loads a matrix, which `m3_no_mesh_loads_a_matrix`
    // holds to across the roster.
    let mut modes = Vec::new();
    let mut scales = 0usize;
    let mut loads = 0usize;
    for command in commands {
        match command {
            gpu::Command::MatrixMode(mode) => modes.push(*mode),
            gpu::Command::MatrixScale(_) => scales += 1,
            gpu::Command::MatrixLoad4x3(_) => loads += 1,
            _ => {}
        }
    }
    println!("matrix cmds  : modes {modes:?}, {scales} scale, {loads} load4x3");

    println!();
    println!("vertices     : {}", geometry.positions.len());
    println!("uvs          : {}", geometry.uvs.len());
    println!("uv flip      : {}", orientation.name());
    if let Some((min, max)) = geometry.uv_bounds() {
        println!(
            "uv range     : u {:.4} .. {:.4}   v {:.4} .. {:.4}",
            min[0], max[0], min[1], max[1]
        );
    }
    println!("faces        : {}", geometry.face_count());
    println!("triangles    : {}", geometry.triangle_count());
    println!("corners      : {}", geometry.corner_count());
    println!("materials    : {}", geometry.groups.len());
    for group in &geometry.groups {
        let key = match group.palette_base {
            Some(value) => value.to_string(),
            None => "-".to_string(),
        };
        println!("  material {key:>10} : {} faces", group.polygons.len());
    }

    if let Some((min, max)) = geometry.bounds() {
        for (label, axis) in [("X", 0), ("Y", 1), ("Z", 2)] {
            println!(
                "bounds {label}     : {0:9.3} .. {1:9.3}   size {2:7.3}",
                min[axis],
                max[axis],
                max[axis] - min[axis]
            );
        }
    }

    Ok(())
}

/// Writes the decoded geometry to a Wavefront OBJ, so it can be looked at in any viewer.
///
/// The OBJ format carries per-corner texture coordinates, which is exactly how the geometry stores
/// them, so this needs no vertex splitting.
fn cmd_obj(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let creature_id: usize = arg(args, 1, "creature")?
        .parse()
        .context("creature id must be a number")?;

    let output = PathBuf::from(args.last().context("missing <file.obj>")?);
    // `obj <rom> <creature> <file>` or `obj <rom> <creature> <clip> <file>`
    let clip_slot: Option<usize> = match args.len() {
        3 => None,
        4 => Some(args[2].parse().context("clip slot must be a number")?),
        _ => return Err(anyhow::anyhow!("expected <rom> <creature> [clip] <file.obj>\n\n{USAGE}")),
    };

    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, "model/battle/arcdin")?;
    let parsed = manifest::parse(&archive.decompressed(0)?)?;
    let creature = parsed
        .find(creature_id)
        .with_context(|| format!("creature {creature_id} is not in the manifest"))?;
    let clip = match clip_slot {
        Some(slot) => creature
            .clips
            .iter()
            .find(|clip| clip.slot == slot)
            .with_context(|| format!("creature {creature_id} has no clip slot {slot}"))?,
        None => &creature.clips[0],
    };

    let decoded = mesh::parse(&archive.decompressed(clip.mesh.index as usize)?)?;
    let stream = gpu::parse_stream(&decoded.commands)?;
    let geometry = gpu::build_geometry(
        &decoded,
        stream.world_root_bone_count()?,
        stream.gpu_commands()?,
    )?;

    let mut out = String::new();
    out.push_str(&format!(
        "# ff-export: creature {creature_id}, clip {} ({})\n",
        clip.slot,
        manifest::clip_label(clip.slot)
    ));
    out.push_str(&format!(
        "# {} vertices, {} faces, {} corners\n",
        geometry.positions.len(),
        geometry.face_count(),
        geometry.corner_count()
    ));

    for position in &geometry.positions {
        out.push_str(&format!("v {} {} {}\n", position[0], position[1], position[2]));
    }
    // Wavefront OBJ's v axis runs upwards from a bottom-left origin, so writing our DS-space
    // coordinates unflipped would put every texture upside down in an OBJ viewer. glTF needs no such
    // flip (see `gpu::UvOrientation`), which is the whole reason this is done here and not up front.
    for uv in &geometry.uvs {
        out.push_str(&format!("vt {} {}\n", uv[0], 1.0 - uv[1]));
    }

    let mut triangles = 0usize;
    for group in &geometry.groups {
        let key = match group.palette_base {
            Some(value) => value.to_string(),
            None => "none".to_string(),
        };
        out.push_str(&format!("g material_{key}\n"));
        for polygon in &group.polygons {
            for index in 1..polygon.len().saturating_sub(1) {
                let fan = [&polygon[0], &polygon[index], &polygon[index + 1]];
                let mut line = String::from("f");
                for corner in fan {
                    match corner.uv {
                        Some(uv) => line.push_str(&format!(" {}/{}", corner.vertex + 1, uv + 1)),
                        None => line.push_str(&format!(" {}", corner.vertex + 1)),
                    }
                }
                out.push_str(&line);
                out.push('\n');
                triangles += 1;
            }
        }
    }

    std::fs::write(&output, out).with_context(|| format!("writing {}", output.display()))?;
    println!(
        "creature {creature_id}, clip {} ({}) -> {} triangles written to {}",
        clip.slot,
        manifest::clip_label(clip.slot),
        triangles,
        output.display()
    );

    Ok(())
}

/// One clip, as an export reports it.
struct ClipSummary {
    name: String,
    frames: usize,
}

/// One material group, as an export reports it.
struct MaterialSummary {
    name: String,
    /// The image it uses: width, height, and whether that image carries a colour key.
    image: Option<(usize, usize, bool)>,
}

/// Everything one creature's export produced: the file, and what went into it.
struct CreatureExport {
    /// The `.glb`, ready to write.
    bytes: Vec<u8>,
    bones: usize,
    /// Vertices before the per-corner UV split, i.e. what the mesh's own vertex list holds.
    authored_vertices: usize,
    vertices: usize,
    triangles: usize,
    groups: usize,
    /// Whether the winding had to be reversed so front faces point outwards.
    reversed: bool,
    bounds: Option<([f64; 3], [f64; 3])>,
    uv_bounds: Option<([f64; 2], [f64; 2])>,
    clips: Vec<ClipSummary>,
    materials: Vec<MaterialSummary>,
    /// The creature's texture set, decoded: the loose PNGs and the catalog are built from it.
    textures: texture::Textures,
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
fn build_creature(
    archive: &mar::Archive<'_>,
    creature: &manifest::Creature,
    name: &str,
    orientation: gpu::UvOrientation,
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

/// Writes a creature clip's bind-pose mesh and skeleton as a `.glb`.
fn cmd_export(args: &[String]) -> Result<()> {
    let (arguments, orientation) = take_uv_flip(args)?;
    let args = arguments.as_slice();

    let rom_path = arg(args, 0, "rom")?;
    let creature_id: usize = arg(args, 1, "creature")?
        .parse()
        .context("creature id must be a number")?;
    // The clip is required rather than optional: every clip names its own mesh, with its own bind pose,
    // so there is no one mesh to hang a creature's whole clip set on.
    let clip_slot: usize = arg(args, 2, "clip")?
        .parse()
        .context("clip slot must be a number")?;
    let output = PathBuf::from(arg(args, 3, "file.glb")?);

    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, "model/battle/arcdin")?;
    let parsed = manifest::parse(&archive.decompressed(0)?)?;
    let creature = parsed
        .find(creature_id)
        .with_context(|| format!("creature {creature_id} is not in the manifest"))?;

    let name = creature_names(&rom)
        .ok()
        .and_then(|all| all.get(creature_id - 1).cloned())
        .unwrap_or_else(|| format!("vivosaur_{creature_id:03}"));

    let clip = creature
        .clips
        .iter()
        .find(|clip| clip.slot == clip_slot)
        .with_context(|| {
            let slots: Vec<usize> = creature.clips.iter().map(|clip| clip.slot).collect();
            format!("creature {creature_id} has no clip slot {clip_slot}; it has {slots:?}")
        })?;
    let wanted = [clip];

    let export = build_creature(&archive, creature, &name, orientation, &wanted)?;
    std::fs::write(&output, &export.bytes)
        .with_context(|| format!("writing {}", output.display()))?;

    println!("creature     : {creature_id} {name}");
    println!(
        "vertices     : {} after UV splitting ({} before)",
        export.vertices, export.authored_vertices
    );
    println!("triangles    : {}", export.triangles);
    println!("uv flip      : {}", orientation.name());
    if let Some((min, max)) = export.uv_bounds {
        println!(
            "uv range     : u {:.4} .. {:.4}   v {:.4} .. {:.4}",
            min[0], max[0], min[1], max[1]
        );
    }
    println!("primitives   : {}", export.groups);
    println!("images       : {}", export.textures.images.len());
    for image in &export.textures.images {
        println!(
            "  {:<12} {}x{} {}{}",
            image.name,
            image.width,
            image.height,
            image.format.name(),
            if image.transparent {
                ", transparent key"
            } else {
                ""
            }
        );
    }
    println!("materials    : {}", export.materials.len());
    for material in &export.materials {
        match material.image {
            Some((width, height, key)) => println!(
                "  {:<12} {}x{}{}",
                material.name,
                width,
                height,
                if key { ", transparency key" } else { "" }
            ),
            None => println!("  {:<12} no matching image", material.name),
        }
    }
    println!("joints       : {}", export.bones);
    println!(
        "winding      : {}",
        if export.reversed {
            "reversed, so front faces point outwards"
        } else {
            "kept, already facing outwards"
        }
    );
    println!("clips        : {}", export.clips.len());
    for clip in &export.clips {
        println!(
            "  {:<14} {} frames  {:.3}s",
            clip.name,
            clip.frames,
            clip.frames as f64 / 60.0
        );
    }
    println!(
        "wrote        : {} bytes to {}",
        export.bytes.len(),
        output.display()
    );

    Ok(())
}

/// Exports every creature: a `.glb` each, its loose texture sheets, and the catalog that lists them.
///
/// Every file is imported again in memory before it is written, so "exported" also means "and the
/// result loads", and the catalog is written last so it only ever describes files that are on disk.
fn cmd_export_all(args: &[String]) -> Result<()> {
    let (arguments, orientation) = take_uv_flip(args)?;
    let args = arguments.as_slice();

    let rom_path = arg(args, 0, "rom")?;
    let out_dir = PathBuf::from(arg(args, 1, "outdir")?);

    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, "model/battle/arcdin")?;
    let parsed = manifest::parse(&archive.decompressed(0)?)?;
    let names = creature_names(&rom).context("the creature name table is needed to name the files")?;

    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let started = Instant::now();
    let mut creatures = Vec::with_capacity(parsed.creatures.len());
    let mut slugs: BTreeMap<String, usize> = BTreeMap::new();
    let mut repeats = Vec::new();
    let mut failures = Vec::new();
    let mut bytes_written = 0usize;
    let mut sheets = 0usize;
    let mut files = 0usize;

    println!(
        "{:<4} {:<12} {:>7} {:>6} {:>6} {:>7} {:>9}  clip slots",
        "id", "name", "tris", "clips", "bones", "sheets", "total"
    );

    for creature in &parsed.creatures {
        let name = names
            .get(creature.id - 1)
            .cloned()
            .unwrap_or_else(|| format!("vivosaur_{:03}", creature.id));

        // The game's own name table repeats two names (Frigi and Igno appear twice), which is why the
        // creature's id is part of every file name. Two creatures sharing a name is therefore worth
        // reporting rather than failing on.
        if let Some(previous) = slugs.insert(catalog::slug(&name), creature.id) {
            repeats.push(format!("creatures {previous} and {} are both `{name}`", creature.id));
        }

        let mut clips = Vec::with_capacity(creature.clips.len());
        let mut textures = Vec::new();
        let mut labels = Vec::with_capacity(creature.clips.len());
        let mut creature_bytes = 0usize;
        let mut first_triangles = 0usize;
        let mut first_bones = 0usize;

        for clip in &creature.clips {
            let wanted = [clip];
            let label = clip_label(clip.slot);
            let reported = format!("creature {} {name} clip {} ({label})", creature.id, clip.slot);
            labels.push(label.clone());

            let export = match build_creature(&archive, creature, &name, orientation, &wanted) {
                Ok(export) => export,
                Err(error) => {
                    failures.push(format!("{reported}: {error:#}"));
                    continue;
                }
            };

            // Imported again before it is kept, so "exported" also means "and it loads".
            match verify_creature(&export, &name)
                .and_then(|()| write_clip(&out_dir, creature.id, &name, &label, &export))
            {
                Ok(()) => {}
                Err(error) => {
                    failures.push(format!("{reported}: {error:#}"));
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
                if let Err(error) = write_textures(&out_dir, creature.id, &name, &export) {
                    failures.push(format!("creature {} {name} sheets: {error:#}", creature.id));
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

        println!(
            "{:<4} {:<12} {:>7} {:>6} {:>6} {:>7} {:>9}  {}",
            creature.id,
            name,
            first_triangles,
            clips.len(),
            first_bones,
            textures.len(),
            human_bytes(creature_bytes),
            labels.join(", ")
        );

        let slug = catalog::slug(&name);
        creatures.push(catalog::Creature {
            id: creature.id,
            name,
            slug,
            textures,
            clips,
        });
    }

    let catalog = catalog::Catalog {
        generator: "ff-export".to_string(),
        source: catalog::Source {
            rom: Path::new(&rom_path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| rom_path.clone()),
            archive: "model/battle/arcdin".to_string(),
            creature_count: creatures.len(),
            clip_count: files,
        },
        units: catalog::Catalog::units(),
        layout: catalog::Catalog::layout(),
        creatures,
    };
    let manifest = out_dir.join("manifest.json");
    // Anything a previous run left behind goes first, so the manifest describes the whole directory.
    let stale = prune_output(&out_dir, &catalog)?;
    std::fs::write(&manifest, catalog.to_json()?)
        .with_context(|| format!("writing {}", manifest.display()))?;

    println!();
    for repeat in &repeats {
        println!("note         : {repeat}");
    }
    if stale > 0 {
        println!("pruned       : {stale} files a previous run left behind");
    }
    println!(
        "exported     : {} creatures, {} clips, {} texture sheets, {} of glb",
        catalog.source.creature_count,
        files,
        sheets,
        human_bytes(bytes_written)
    );
    println!("manifest     : {}", manifest.display());
    println!("elapsed      : {:.1}s", started.elapsed().as_secs_f64());
    for failure in &failures {
        println!("failed       : {failure}");
    }
    if !failures.is_empty() {
        bail!("{} exports did not verify", failures.len());
    }

    Ok(())
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
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
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
        std::fs::write(&path, image.to_png()?).with_context(|| format!("writing {}", path.display()))?;
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
        bail!("`{name}` came back with {primitives} primitives, expected {}", export.groups);
    }

    let joints = document
        .skins()
        .next()
        .map(|skin| skin.joints().count())
        .unwrap_or(0);
    if joints != export.bones {
        bail!("`{name}` came back with {joints} joints, expected {}", export.bones);
    }

    Ok(())
}

/// A byte count a person can read at a glance.
fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

/// Writes a creature's texture images out as loose PNGs, so a sheet can be looked at on its own.
///
/// The same images are embedded in the creature's `.glb`; this exists so one sheet can be put beside a
/// model's UV layout without digging it out of the binary.
fn cmd_textures(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let creature_id: usize = arg(args, 1, "creature")?
        .parse()
        .context("creature id must be a number")?;
    let out_dir = PathBuf::from(arg(args, 2, "outdir")?);

    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, "model/battle/arcdin")?;
    let parsed = manifest::parse(&archive.decompressed(0)?)?;
    let creature = parsed
        .find(creature_id)
        .with_context(|| format!("creature {creature_id} is not in the manifest"))?;
    let clip = creature
        .clips
        .first()
        .with_context(|| format!("creature {creature_id} has no clips"))?;

    let textures = texture::parse(&archive.decompressed(clip.texture.index as usize)?)?;
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    for image in &textures.images {
        let path = out_dir.join(format!("{}.png", image.name));
        std::fs::write(&path, image.to_png()?)
            .with_context(|| format!("writing {}", path.display()))?;

        println!(
            "{:<12} {}x{} {}{} -> {}",
            image.name,
            image.width,
            image.height,
            image.format.name(),
            if image.transparent {
                ", transparent key"
            } else {
                ""
            },
            path.display()
        );
    }

    Ok(())
}

/// Reports one clip's animation block, and compares its first frame with the mesh's bind pose.
///
/// The comparison answers a structural question: if frame 0 is the bind pose, the keyframes are in
/// the same space as the bind matrices (all joints parented to the skeleton root), so they can drive
/// glTF joint nodes directly.
fn cmd_anim(args: &[String]) -> Result<()> {
    let rom_path = arg(args, 0, "rom")?;
    let creature_id: usize = arg(args, 1, "creature")?
        .parse()
        .context("creature id must be a number")?;
    let clip_slot: usize = arg(args, 2, "clip")?
        .parse()
        .context("clip slot must be a number")?;

    let rom = nds::Rom::open(&rom_path)?;
    let (archive, _) = open_archive(&rom, "model/battle/arcdin")?;
    let parsed = manifest::parse(&archive.decompressed(0)?)?;
    let creature = parsed
        .find(creature_id)
        .with_context(|| format!("creature {creature_id} is not in the manifest"))?;
    let clip = creature
        .clips
        .iter()
        .find(|clip| clip.slot == clip_slot)
        .with_context(|| format!("creature {creature_id} has no clip slot {clip_slot}"))?;

    let animation = anim::parse(&archive.decompressed(clip.animation.index as usize)?)?;
    let decoded = mesh::parse(&archive.decompressed(clip.mesh.index as usize)?)?;

    println!("creature     : {creature_id}");
    println!("clip         : {clip_slot} ({})", manifest::clip_label(clip_slot));
    println!(
        "animation    : {}:{}",
        clip.animation.table_name, clip.animation.index
    );
    println!(
        "bones        : {} (the mesh has {})",
        animation.bone_count,
        decoded.bones.len()
    );
    println!("frames       : {}", animation.frame_count);
    println!(
        "duration     : {:.3}s at 60 fps",
        animation.frame_count as f64 / 60.0
    );
    println!("models       : {:?}", animation.included_models);

    println!();
    println!("bone          bind translation              frame 0 translation             delta");
    for bone in 0..animation.bone_count.min(8) {
        let Some(pose) = animation.transform(bone, 0) else {
            continue;
        };
        let bind = &decoded.bones[bone].matrix;
        let delta = [
            pose.translation[0] - bind.translation[0],
            pose.translation[1] - bind.translation[1],
            pose.translation[2] - bind.translation[2],
        ];
        println!(
            "{:<12} ({:7.3},{:7.3},{:7.3})  ({:7.3},{:7.3},{:7.3})  ({:6.3},{:6.3},{:6.3})",
            decoded.bones[bone].name,
            bind.translation[0],
            bind.translation[1],
            bind.translation[2],
            pose.translation[0],
            pose.translation[1],
            pose.translation[2],
            delta[0],
            delta[1],
            delta[2]
        );
    }

    Ok(())
}
