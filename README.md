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

Package the current platform:

```bash
cargo install cargo-packager --version 0.11.8 --locked
cargo packager --release
```

Release packaging, signing and self-update setup are documented in
[docs/packaging.md](docs/packaging.md).

The Linux release is an AppImage. On first launch it can install itself for the
current user and add DRH Launcher to the desktop application menu without
requiring administrator access.

## Current Scope

- Slint-based desktop UI
- Local JSON configuration
- Initial home screen
- GitHub release discovery
- Verified archive downloads
- Native launcher packages and signed launcher self-updates

## Architecture

See [docs/architecture.md](docs/architecture.md) for the intended product and technical architecture.
