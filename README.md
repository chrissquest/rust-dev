# rust-dev

A monorepo for small Rust apps about rendering, game dev and asset extraction.
All of them share one Cargo workspace, one lockfile and one `target/`.

| App | Package | What it is |
| --- | --- | --- |
| [`apps/ff-export`](apps/ff-export) | `ff-export`, `ff-export-gui` | Fossil Fighters (NDS) -> glTF/GLB asset exporter, CLI plus a Slint window |
| [`apps/ff-view`](apps/ff-view) | `ff-view` | Low-poly glTF model viewer and browser on wgpu |
| [`apps/rust-wgpu`](apps/rust-wgpu) | `chris-wgpu` | wgpu + winit rendering playground |

`ff-export` and `rust-wgpu` used to be their own repositories
(`chrissquest/ff-export`, `chrissquest/rust-wgpu`). Their full commit history was
merged in here, so `git log --follow apps/ff-export/src/lib.rs` goes all the way back.

## Building

```powershell
cargo build                      # every app except the Slint GUI
cargo run -p ff-export -- --help
cargo run -p ff-export-gui       # the exporter's window
cargo run -p ff-view
cargo run -p chris-wgpu
```

## Layout

```text
Cargo.toml     workspace: members, default-members, release profile
apps/<name>/   one directory per app
crates/<name>/ shared libraries, once two apps need the same code
```

To add an app, create `apps/<name>` with `cargo new apps/<name>` and add it to
`members` (and to `default-members`, unless it is slow to build).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
