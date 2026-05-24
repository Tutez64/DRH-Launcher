# DRH Launcher

Launcher and updater for [Dungeon Rampage Haxe](https://github.com/Tutez64/Dungeon-Rampage-Haxe).

This project is in early development. The first goal is to provide a small desktop application that can install, update and launch DRH. Mod management, Steam shortcut integration and advanced launch options will be added later.

## Development

Requirements:

- [Rust](https://rust-lang.org/fr/)
- [Slint](https://slint.dev/) LSP / IDE plugin

Run:

```bash
cargo run
```

Debug builds use the [fixture releases](https://github.com/Tutez64/DRHL-Release-Fixtures) by default. Release builds use the real DRH repository by default.

Override the release source explicitly:

```bash
DRHL_RELEASE_SOURCE=drh cargo run
DRHL_RELEASE_SOURCE=fixtures cargo run
```

Check:

```bash
cargo check
```

Build:

```bash
cargo build --release
```

## Current Scope

- Slint-based desktop UI
- Local JSON configuration
- Initial home screen
- GitHub release discovery
- Verified archive downloads

## Architecture

See [docs/architecture.md](docs/architecture.md) for the intended product and technical architecture.
