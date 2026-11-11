# rust-dev

A monorepo for small Rust apps about rendering, game dev and asset extraction.
All of them share one Cargo workspace, one lockfile and one `target/`.

## Libraries

| Crate | What it is |
| --- | --- |
| [`crates/crystal-engine`](crates/crystal-engine) | General-purpose rendering engine on wgpu: scenes, skinning, animation, glTF loading |

## Apps

| App | Package | What it is |
| --- | --- | --- |
| [`apps/ff-export`](apps/ff-export) | `ff-export` | Fossil Fighters (NDS) -> glTF/GLB asset exporter (CLI and library) |
| [`apps/ff-export-gui`](apps/ff-export-gui) | `ff-export-gui` | Slint window for `ff-export`: pick a ROM, export everything |
| [`apps/ff-view`](apps/ff-view) | `ff-view` | Low-poly glTF model viewer and browser, built on `crystal-engine` |

Each folder under `apps/` and `crates/` is named after the package it holds.

`ff-export` and `crystal-engine` (then `rust-wgpu`/`chris-wgpu`) used to be their own
repositories (`chrissquest/ff-export`, `chrissquest/rust-wgpu`). Their full commit history
was merged in here, so `git log --follow apps/ff-export/src/lib.rs` goes all the way back.

## Building

```powershell
cargo build                                    # everything except the Slint GUI
cargo test                                     # unit tests for the same set
cargo run -p ff-export -- --help
cargo run -p ff-export-gui                     # the exporter's window
cargo run -p ff-view
cargo run -p crystal-engine --example triangle
```

## Layout

```text
Cargo.toml       workspace: members, default-members, release profile
apps/<name>/     programs you run
crates/<name>/   libraries apps build on
```

To add an app, run `cargo new apps/<name>` and add it to `members` (and to
`default-members`, unless it is slow to build). To use the engine from it:

```toml
[dependencies]
crystal-engine = { path = "../../crates/crystal-engine" }
```

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
