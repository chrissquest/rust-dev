//! A window for `ff-export`: choose a ROM, choose a folder, export every creature.
//!
//! The work is `ff_export::export::export_all`, the same call the CLI makes, so the window shows the
//! same numbers the terminal does - it just reports them through [`Progress`] instead of printing.

// A console window behind the UI is noise; keep it in debug builds, where the log is useful.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod settings;
#[cfg(windows)]
mod drop_target;

use std::path::{Path, PathBuf};

use ff_export::export::{self, ExportReport, ExportRequest, Progress};
use ff_export::gpu::UvOrientation;

fn main() -> anyhow::Result<()> {
    let ui = AppWindow::new()?;

    connect_browse_rom(&ui);
    connect_browse_out(&ui);
    connect_start(&ui);

    // The native window exists by now, which is all the drop hook needs. It keeps
    // retrying for a moment in case the window manager is slower than the loop.
    ui.show()?;
    #[cfg(windows)]
    drop_target::install_when_ready(&ui);

    ui.run()?;
    Ok(())
}

/// The one place a ROM gets accepted, whether it came from a dialog or a drop.
pub(crate) fn accept_rom(ui: &AppWindow, path: &Path) {
    ui.set_rom_path(path.display().to_string().into());
    if let Some(parent) = path.parent() {
        settings::remember_rom(parent);
    }
    ui.set_status(format!("ROM: {}", file_name(path)).into());
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The first candidate that is an existing directory: the field's own path, then what
/// was remembered.
fn starting_directory(candidates: [Option<PathBuf>; 3]) -> Option<PathBuf> {
    candidates
        .into_iter()
        .flatten()
        .find(|candidate| candidate.is_dir())
}

fn pick_rom(ui: &AppWindow) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Choose a Fossil Fighters ROM")
        .add_filter("Nintendo DS ROM", &["nds"]);

    let current = PathBuf::from(ui.get_rom_path().as_str());
    let current_dir = current.parent().map(Path::to_path_buf);

    if let Some(directory) = starting_directory([current_dir, settings::rom_dir(), None]) {
        dialog = dialog.set_directory(directory);
    }

    dialog.pick_file()
}

/// Asks where the export should go, and remembers the answer.
fn pick_output(ui: &AppWindow) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Choose where to write the export");

    let current = PathBuf::from(ui.get_out_path().as_str());
    let current_dir = (!current.as_os_str().is_empty()).then_some(current);

    if let Some(directory) = starting_directory([
        current_dir,
        settings::output_dir(),
        settings::rom_dir(),
    ]) {
        dialog = dialog.set_directory(directory);
    }

    let folder = dialog.pick_folder()?;
    settings::remember_output(&folder);
    Some(folder)
}

fn connect_browse_rom(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_browse_rom(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };

        if let Some(path) = pick_rom(&ui) {
            accept_rom(&ui, &path);
        }
    });
}

fn connect_browse_out(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_browse_out(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };

        if let Some(folder) = pick_output(&ui) {
            ui.set_out_path(folder.display().to_string().into());
        }
    });
}

fn connect_start(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_start(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_busy() {
            return;
        }

        let rom = PathBuf::from(ui.get_rom_path().as_str());
        if rom.as_os_str().is_empty() {
            ui.set_status("Drop a ROM on the window, or use Browse.".into());
            return;
        }
        if !rom.is_file() {
            ui.set_status(format!("No such file: {}", rom.display()).into());
            return;
        }

        // Where to write is a question for this moment rather than a field to fill in
        // first: ask here, and remember the answer for next time.
        let mut out_dir = PathBuf::from(ui.get_out_path().as_str());
        if out_dir.as_os_str().is_empty() {
            match pick_output(&ui) {
                Some(chosen) => {
                    ui.set_out_path(chosen.display().to_string().into());
                    out_dir = chosen;
                }
                None => {
                    ui.set_status("Export cancelled — no output folder chosen.".into());
                    return;
                }
            }
        }

        ui.set_busy(true);
        ui.set_progress(0.0);
        ui.set_log_text("".into());
        ui.set_status("Reading the manifest…".into());

        let worker = weak.clone();
        std::thread::spawn(move || {
            let request = ExportRequest {
                rom,
                out_dir,
                orientation: UvOrientation::Ds,
            };

            // The paths belong in the log: when a write fails, the first question
            // is always "to where, exactly?".
            let mut log = format!(
                "rom          : {}\noutput       : {}\n\n",
                request.rom.display(),
                request.out_dir.display()
            );

            // A panic in here would otherwise kill the thread and leave the window
            // stuck on "busy" forever, with nothing to read. Errors are values in
            // Rust, but panics are not, so they need catching separately.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                export::export_all(&request, &mut |progress| match progress {
                    Progress::Started { creatures } => {
                        publish(&worker, format!("Exporting {creatures} creatures…"), None);
                    }
                    Progress::Creature {
                        index, total, name, ..
                    } => {
                        let fraction = index as f32 / total.max(1) as f32;
                        publish(
                            &worker,
                            format!("{}/{}  {name}", index + 1, total),
                            Some(fraction),
                        );
                    }
                    Progress::Clip { label, .. } => {
                        publish(&worker, format!("building {label}"), None);
                    }
                    Progress::CreatureDone(row) => {
                        log.push_str(&format!(
                            "{:>4}  {:<14} {:>6} tris  {:>3} clips  {:>3} joints  {}\n",
                            row.id,
                            row.name,
                            row.triangles,
                            row.clips,
                            row.bones,
                            row.labels.join(", ")
                        ));
                        publish_log(&worker, &log);
                    }
                })
            }));

            let result = match outcome {
                Ok(result) => result,
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| String::from("unknown panic"));
                    Err(anyhow::anyhow!("the export panicked: {message}"))
                }
            };

            finish(&worker, result, log);
        });
    });
}

fn publish(ui: &slint::Weak<AppWindow>, status: String, progress: Option<f32>) {
    let ui = ui.clone();
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_status(status.into());
        if let Some(progress) = progress {
            ui.set_progress(progress);
        }
    });
}

fn publish_log(ui: &slint::Weak<AppWindow>, text: &str) {
    let ui = ui.clone();
    let text = text.to_string();
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_log_text(text.into()));
}

fn finish(ui: &slint::Weak<AppWindow>, result: anyhow::Result<ExportReport>, mut log: String) {
    let ui = ui.clone();
    let _ = ui.upgrade_in_event_loop(move |ui| match result {
        Ok(report) => {
            log.push_str(&format!(
                "\nexported     : {} creatures, {} clips, {} texture sheets, {} in {:.1}s\nmanifest     : {}",
                report.creatures,
                report.clips,
                report.sheets,
                export::human_bytes(report.bytes),
                report.elapsed.as_secs_f64(),
                report.manifest.display()
            ));
            for repeat in &report.repeats {
                log.push_str(&format!("\nnote         : {repeat}"));
            }
            for failure in &report.failures {
                log.push_str(&format!("\nfailed       : {failure}"));
            }

            let status = if report.failures.is_empty() {
                format!(
                    "Done: {} creatures, {} clips.",
                    report.creatures, report.clips
                )
            } else {
                format!("Finished with {} failures — see log.", report.failures.len())
            };

            ui.set_log_text(log.into());
            ui.set_status(status.into());
            ui.set_progress(1.0);
            ui.set_busy(false);
        }
        Err(error) => {
            // `{:#}` is anyhow's alternate Display: it walks the context chain and
            // gives "writing <path>: Access is denied. (os error 5)". Plain `{}`
            // would show only the outer context and hide the cause.
            log.push_str(&format!("failed\n\n{error:#}\n"));
            ui.set_log_text(log.into());
            ui.set_status(format!("Export failed: {}", summarise(&error)).into());
            ui.set_busy(false);
        }
    });
}

/// The full error chain, capped so a long one cannot swallow the window.
fn summarise(error: &anyhow::Error) -> String {
    let full = format!("{error:#}");
    if full.chars().count() > 160 {
        let mut short: String = full.chars().take(157).collect();
        short.push_str("...");
        short
    } else {
        full
    }
}
