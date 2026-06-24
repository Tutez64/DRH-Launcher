<p align="center">
  <img src="assets/icons/app-icon-128.png" width="96" alt="DRH Launcher" />
</p>
<h1 align="center">DRH Launcher</h1>

<p align="center">
Official launcher for <a href="https://github.com/Tutez64/Dungeon-Rampage-Haxe" target="_blank">Dungeon Rampage Haxe</a>.
Install, update, configure, and play DRH easily.</p>

## Installation

Download it for Windows, Linux, or macOS: [latest release](https://github.com/Tutez64/DRH-Launcher/releases/latest).

### Windows

Run the `.exe` installer.

If a **SmartScreen** window appears: choose **More info**, then **Run anyway**.

### Linux

Open the `.AppImage`. If needed, make it executable first:

```bash
chmod +x DRH-Launcher_*.AppImage
```

On first launch, choose **Install** when prompted.

### macOS

Open the `.dmg`, drag **DRH Launcher.app** to **Applications**.

**Gatekeeper** may block the `.dmg` or the app the first time. If that happens,
right-click the blocked item, choose **Open**, then confirm.\
On newer macOS versions, you may also need to allow it from
**System Settings > Privacy & Security**.

## Install and play DRH

1. Open **DRH Launcher**.
2. Click **Install DRH**.
3. Click **Play**.

## Features

- Install, update, repair, and launch Dungeon Rampage Haxe.
- Use recommended launch options by default or adjust them as you wish. 
- Optionally, use a pre-launch command and custom extra game arguments.
- Show launcher and game session logs.
- Browse DRH and DRH Launcher release history, read changelogs, and install older DRH versions.
- Update itself when a new release is available.
- ...more to come!

## Community

Join the [Discord server](https://discord.gg/VvWbNspZrQ) to discuss DRH and DRH Launcher,
get update notifications, and see occasional previews.

## Development

```bash
cargo run
cargo test
```

Debug builds use [fixture releases](https://github.com/Tutez64/DRHL-Release-Fixtures)
with the same archive layout as production DRH releases. Release builds use the
real DRH repository by default.

```bash
DRHL_RELEASE_SOURCE=drh cargo run
DRHL_RELEASE_SOURCE=fixtures cargo run
```

- Packaging and release workflows: [docs/packaging.md](docs/packaging.md)
- Architecture: [docs/architecture.md](docs/architecture.md)
