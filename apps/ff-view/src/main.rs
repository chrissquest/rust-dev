//! `ff-view` - a low-poly model viewer.
//!
//! Loads a glTF/GLB file, plays its first animation clip, and renders it with a
//! skinned wgpu pipeline. Everything except the window and the renderer is
//! GPU-free, so `--info` works over SSH and in tests.
//!
//! Given an export folder instead of a file - or no path at all, and then a drop
//! - the same window grows a side panel for picking the creature and the clip,
//! and remembers that folder for next time (see `ff_view::settings`).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use ff_view::library::Catalog;
use crystal_engine::loader;
use ff_view::settings;
use ff_view::ui::{self, Initial, Launch};

const USAGE: &str = "\
ff-view - low-poly model viewer

USAGE:
    ff-view [model.gltf|model.glb | export-folder] [options]

    With no path, opens the picker window: drop a model or an export folder on it.
    An export folder - passed, remembered or dropped - lists its creatures and
    clips in a side panel, starting each creature on its `idle` animation. The
    last folder opened is reopened next time.

OPTIONS:
    --info                  print the scene's (or folder's) contents and exit
    --settings              print where the last folder is remembered and exit
    --clip <index|name>     start on a specific animation clip
    --wireframe             start in wireframe mode (needs POLYGON_MODE_LINE)
    --bind-pose             ignore animation clips and show the rest pose
    --backend <name>        gl | vulkan | dx12 | metal | all (default: all)
    --frames <n>            render n frames, then exit (smoke test)
    --screenshot <file.png> render one frame off-screen, save it, and exit
    -h, --help              show this text

CONTROLS:
    left drag               orbit
    right / middle drag     pan
    wheel                   zoom
    space                   pause / resume the clip
    n / p                   next / previous clip in this file
    up / down               previous / next creature (folder panel)
    left / right            previous / next clip (folder panel)
    w                       toggle wireframe
    f                       toggle camera follow (on by default)
    r                       reset the camera angle
    drop a file or folder   load it (a folder opens the panel and is remembered)
    esc                     quit
";

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        // wgpu's GLES backend emits a performance hint per buffer upload at
        // warn level; useful once, then noise on every launch.
        env_logger::Env::default().default_filter_or("warn,wgpu_hal::gles=error"),
    )
    .init();

    let Some(options) = parse_args()? else {
        print!("{USAGE}");
        return Ok(());
    };

    if options.settings_only {
        print_settings();
        return Ok(());
    }

    let initial = load_initial(&options)?;

    if options.info_only {
        let rows = match &initial {
            Initial::Scene(scene) => scene.summary(),
            Initial::Catalog(catalog) => catalog.summary(),
            Initial::Empty => bail!("--info needs a model or an export folder"),
        };
        for (label, value) in rows {
            if !value.is_empty() {
                println!("{label:<12}: {value}");
            }
        }
        return Ok(());
    }

    let mut launch = Launch::default();
    launch.initial = initial;
    launch.backends = options.backends;
    launch.clip = options.clip;
    launch.wireframe = options.wireframe;
    launch.bind_pose = options.bind_pose;
    launch.frames = options.frames;
    launch.screenshot = options.screenshot;

    ui::run(launch)
}

/// `--settings`: what will be reopened, and where that is written down.
fn print_settings() {
    match std::env::current_exe() {
        Ok(exe) => println!("exe         : {}", exe.display()),
        Err(_) => println!("exe         : (unknown)"),
    }

    match settings::path() {
        Some(path) => println!("file        : {}", path.display()),
        None => println!("file        : (no config directory in the environment)"),
    }

    match settings::remembered_folder() {
        Some(folder) => {
            let state = if folder.is_dir() { "exists" } else { "missing" };
            println!("last folder : {} ({state})", folder.display());
        }
        None => println!("last folder : (none yet - open one and it is saved)"),
    }
}

/// Loads whatever was named on the command line - or the remembered folder when
/// nothing was - into what the window starts with.
fn load_initial(options: &Options) -> Result<Initial> {
    match options.model.clone() {
        Some(path) if path.is_dir() => {
            // `Catalog`'s errors already name the folder or the file inside it.
            Ok(Initial::Catalog(Catalog::open(&path)?))
        }
        Some(path) => {
            let scene = loader::load(&path)
                .with_context(|| format!("could not load {}", path.display()))?;
            Ok(Initial::Scene(scene))
        }
        None => Ok(settings::last_folder()
            .and_then(|folder| match Catalog::open(&folder) {
                Ok(catalog) => Some(Initial::Catalog(catalog)),
                Err(error) => {
                    // A remembered folder that has since been moved is a warning,
                    // not a failure: the window still opens and can take a drop.
                    eprintln!("warning: could not reopen {}: {error:#}", folder.display());
                    None
                }
            })
            .unwrap_or_default()),
    }
}

struct Options {
    /// `None` when the viewer was started bare, to be used as a drop target.
    model: Option<PathBuf>,
    info_only: bool,
    settings_only: bool,
    wireframe: bool,
    bind_pose: bool,
    frames: Option<u32>,
    screenshot: Option<PathBuf>,
    backends: wgpu::Backends,
    clip: Option<String>,
}

fn parse_args() -> Result<Option<Options>> {
    let mut args = std::env::args().skip(1);
    let mut model: Option<PathBuf> = None;
    let mut info_only = false;
    let mut settings_only = false;
    let mut wireframe = false;
    let mut bind_pose = false;
    let mut frames = None;
    let mut screenshot = None;
    let mut backends = backends_from_env();
    let mut clip = None;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--info" => info_only = true,
            "--settings" => settings_only = true,
            "--wireframe" => wireframe = true,
            "--bind-pose" => bind_pose = true,
            "--clip" => {
                clip = Some(args.next().context("--clip needs an index or a name")?);
            }
            "--backend" => {
                let value = args.next().context("--backend needs a value")?;
                backends = parse_backends(&value)?;
            }
            "--frames" => {
                let value = args.next().context("--frames needs a count")?;
                frames = Some(
                    value
                        .parse::<u32>()
                        .with_context(|| format!("`{value}` is not a frame count"))?,
                );
            }
            "--screenshot" => {
                screenshot = Some(PathBuf::from(
                    args.next().context("--screenshot needs a path")?,
                ));
            }
            other if other.starts_with('-') => bail!("unknown option `{other}`\n\n{USAGE}"),
            other => {
                if model.is_some() {
                    bail!("only one model can be given at a time");
                }
                model = Some(PathBuf::from(other));
            }
        }
    }

    Ok(Some(Options {
        model,
        info_only,
        settings_only,
        wireframe,
        bind_pose,
        frames: frames.filter(|&count| count > 0),
        screenshot,
        backends,
        clip,
    }))
}

fn backends_from_env() -> wgpu::Backends {
    match std::env::var("FF_VIEW_BACKEND") {
        Ok(value) => parse_backends(&value).unwrap_or(wgpu::Backends::all()),
        Err(_) => wgpu::Backends::all(),
    }
}

fn parse_backends(value: &str) -> Result<wgpu::Backends> {
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "gl" | "opengl" | "gles" => wgpu::Backends::GL,
        "vulkan" | "vk" => wgpu::Backends::VULKAN,
        "dx12" | "d3d12" => wgpu::Backends::DX12,
        "metal" => wgpu::Backends::METAL,
        "all" | "" => wgpu::Backends::all(),
        other => bail!("unknown backend `{other}` (try gl, vulkan, dx12, metal or all)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_argument_is_a_catalog_not_a_scene() {
        // A dedicated empty folder, so the answer cannot depend on whatever else
        // is in the system temp directory.
        let folder = std::env::temp_dir().join(format!(
            "ff-view-empty-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("create scratch folder");

        let options = Options {
            model: Some(folder.clone()),
            info_only: false,
            settings_only: false,
            wireframe: false,
            bind_pose: false,
            frames: None,
            screenshot: None,
            backends: wgpu::Backends::all(),
            clip: None,
        };

        // It has no models, so this fails - but it has to fail as a folder, not
        // by trying to parse the folder as glTF.
        let error = load_initial(&options).unwrap_err().to_string();
        assert!(
            error.contains("no manifest.json") || error.contains("is not a folder"),
            "unexpected error: {error}"
        );

        let _ = std::fs::remove_dir_all(&folder);
    }
}
