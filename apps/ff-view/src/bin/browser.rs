//! `ff-browser` - the folder panel, kept as its own binary for muscle memory.
//!
//! It is the same window as `ff-view` (see `ff_view::ui`): the only differences
//! are that the panel is always visible, and that it starts on the remembered
//! folder, or on `apps\ff-export\externals` when run from the repository root.

use std::path::PathBuf;

use anyhow::Result;

use ff_view::library::Catalog;
use ff_view::settings;
use ff_view::ui::{self, Initial, Launch};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let initial = default_folder()
        .and_then(|folder| match Catalog::open(&folder) {
            Ok(catalog) => Some(Initial::Catalog(catalog)),
            Err(error) => {
                eprintln!("warning: could not open {}: {error:#}", folder.display());
                None
            }
        })
        .unwrap_or_default();

    ui::run(Launch::browser(initial))
}

fn default_folder() -> Option<PathBuf> {
    settings::last_folder().or_else(|| {
        let local = PathBuf::from("apps").join("ff-export").join("externals");
        local.is_dir().then_some(local)
    })
}
