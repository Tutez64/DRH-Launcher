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
- Mocked install, launch and update actions

## Architecture

See [docs/architecture.md](docs/architecture.md) for the intended product and technical architecture.
