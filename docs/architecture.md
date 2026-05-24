# DRH Launcher Architecture

This document captures the current intended architecture for DRH Launcher.

It is a planning document, not a promise that every feature already exists. Most sections are expected to evolve as implementation, packaging and release constraints become clearer.

## Goals

DRH Launcher is the recommended entry point for Dungeon Rampage Haxe.

It should:

- install DRH from GitHub releases
- check for updates automatically
- launch DRH with user-configurable options
- provide a simple place for settings, changelogs, diagnostics and links
- eventually manage mods and Steam shortcuts

DRH itself should remain launchable directly for debugging, recovery and advanced users, but direct launch is not the default user flow.

## Technology

- Language: Rust
- GUI: Slint
- License: GPL-3.0-or-later

The launcher is deliberately separate from the Haxe/OpenFL game runtime. This keeps update and recovery logic available even if the game installation is broken.

## Supported Platforms

The intended platform identifiers are:

```text
linux-x64
windows-x64
macos-universal
```

These identifiers should be used consistently in manifests, logs, update decisions and user-facing diagnostics. Archive names may keep their public release naming convention, but internal code should use stable platform identifiers.

## Public Names

- Cargo package: `drh_launcher`
- Executable: `DRH-Launcher`
- Window title: `DRH Launcher`
- User-facing launcher name: `DRH Launcher`
- Steam entry for playing: `Dungeon Rampage Haxe`

Steam should point to the launcher, not directly to the game executable.

## Installation Layout

The intended installed layout is:

```text
<install-dir>/
  DRH-Launcher
  launcher-data/
    config.json
    logs/
    cache/
    downloads/
  game/
    version.json
    Dungeon Rampage Haxe
    Resources/
    DbConfiguration/
  mods/
```

The exact executable names and native libraries vary by platform.

## Install State

The launcher should model installation state explicitly instead of relying on scattered booleans.

Expected states:

```text
NotInstalled
Installed
UpdateAvailable
Updating
BrokenInstall
LaunchableButMaybeOutdated
```

`UpdateAvailable` means a newer compatible version exists and the launcher can offer or start an update.

`LaunchableButMaybeOutdated` means the installed game appears runnable, but the launcher cannot confirm that it is up to date or cannot update it automatically right now. Examples include offline mode, GitHub check failure, failed download, missing manifest data, or an older version that is still accepted. The primary action can remain `Play`, with a warning or secondary update action. The UI should expose the concrete reason when available, such as "Could not check GitHub releases" or "Download failed".

`BrokenInstall` means the configured install directory exists but required files are missing or inconsistent.

## Main UI

The default visual direction is a dark theme with orange and red accents, matching the broad color mood of Dungeon Rampage Haxe.

Future versions may add more custom visual elements inspired by the game, but this is not a priority for the first implementation. The UI should stay clear, readable and practical before becoming more decorative.

An optional light theme can be added later if Slint makes it straightforward to maintain both themes without duplicating too much UI code.

The home screen should show:

- a prominent `Install DRH` button when the game is missing
- a prominent `Play` button after installation
- installed version
- latest known version
- update status
- manual `Check for updates` action
- compact Discord and GitHub links, visible but not dominant

DRH Launcher checks for updates automatically at startup. Manual checks are still available.

Discord and GitHub links can live in a header corner, footer or menu. They should be easy to find without taking space away from the primary install/play flow. The full Info page can provide the same links with more context.

Useful secondary actions:

- open the install folder
- verify or repair the installed game
- open recent logs

## Launch Modes

The launcher should support at least two modes:

```text
DRH-Launcher
DRH-Launcher --play
```

`DRH-Launcher` opens the full UI.

`DRH-Launcher --play` is intended for Steam and shortcuts. It should quickly check required state, apply or prompt for important updates when needed, then launch DRH without forcing the full UI when everything is ready.

## Launch Options

Launch options are split into two categories.

Pre-executable command:

```text
mangohud
gamescope
prime-run
```

These commands are advanced user input and are executed before the game executable.

Game arguments:

```text
Dungeon Rampage Haxe <game-args>
```

Known game arguments should eventually be described by release metadata rather than parsed from source code at runtime. The UI can then offer:

- default arguments from the release manifest
- recommended arguments maintained by the project
- advanced custom arguments

The source of truth in DRH is currently the constructor of `src/brain/utils/FeatureFlags.hx`, where feature flags and their default `true` / `false` values are listed.

The preferred long-term flow is:

1. DRH extracts feature flag metadata during its own build.
2. The generated metadata is shipped with the DRH release.
3. DRH Launcher reads that metadata from the installed release or release manifest.
4. DRH Launcher presents known flags as user-facing options.

This avoids runtime source parsing in DRH Launcher and keeps each launcher version compatible with the DRH version it installs.

## Updates

The first implementation should use full archives from GitHub releases.

```text
Release name: Dungeon Rampage Haxe V1
Tag: V1

Dungeon.Rampage.Haxe.V1.Linux.tar.gz
Dungeon.Rampage.Haxe.V1.Windows.zip
Dungeon.Rampage.Haxe.V1.macOS.zip
```

Manifest support should be implemented fairly early, but existing DRH releases should not need to be republished just to add one. When a release has no manifest, DRH Launcher should fall back to the simple release and asset naming rules described above.

GitHub now exposes a SHA-256 digest for uploaded release assets in the release UI and REST API. DRH Launcher can use that digest for archive verification, but it does not replace release metadata such as target platform, install layout, game feature flags or pack definitions.

The DRH GitHub repository should be represented as an internal configuration value from the beginning, even if it is not user-editable in the UI. This avoids scattering repository URLs across the codebase.

Each downloadable artifact must have:

- URL
- size
- SHA-256
- target platform
- version

The update flow should be defensive:

1. Download to a cache/downloads directory.
2. Verify SHA-256.
3. Extract to a staging directory.
4. Validate expected files.
5. Replace the existing game directory.
6. Keep enough state for rollback or recovery.

The launcher should not patch the running game process.

## Manifest Shape

When available, a release manifest should describe the release state explicitly. The exact schema may evolve, but a minimal manifest should look like this:

```json
{
  "version": "V3",
  "platforms": {
    "linux-x64": {
      "archive": "Dungeon.Rampage.Haxe.V3.Linux.tar.gz",
      "sha256": "...",
      "size": 123456
    },
    "windows-x64": {
      "archive": "Dungeon.Rampage.Haxe.V3.Windows.zip",
      "sha256": "...",
      "size": 123456
    },
    "macos-universal": {
      "archive": "Dungeon.Rampage.Haxe.V3.macOS.zip",
      "sha256": "...",
      "size": 123456
    }
  }
}
```

DRH Launcher resolves `archive` against the GitHub release assets. For the first implementation, manifests should not point to arbitrary external download URLs.

Later versions of the manifest can add:

- game feature flags
- recommended launch arguments
- pack definitions
- compatibility metadata
- release channels
- changelog metadata

If no manifest is present, DRH Launcher should use the GitHub release name, tag, asset naming rules and GitHub-provided asset digests as a fallback.

## Trust and Security

DRH Launcher downloads and installs executable code, so update behavior must be conservative.

Rules:

- verify SHA-256 before extracting or installing any archive
- never launch a downloaded artifact before verification
- treat GitHub releases from the configured DRH repository as the initial trusted source
- do not silently follow release metadata to arbitrary third-party download domains unless this is explicitly allowed later
- log enough detail to diagnose failed downloads, invalid hashes and extraction errors

Cryptographic signatures may be added later, but SHA-256 verification against GitHub release metadata is enough for the first implementation.

## Failure and Recovery

The update flow must tolerate partial failure.

Expected behavior:

- interrupted downloads stay in the cache and can be retried or discarded
- invalid hashes abort installation and keep the current game unchanged
- extraction happens in a staging directory, never directly over the current game
- replacement happens only after validation succeeds
- failed replacement should keep or restore the previous launchable game directory when possible
- updates should not proceed while the game is running
- partial or inconsistent installs should be reported as `BrokenInstall`

The launcher should prefer a clear recovery action over silent repair when data loss or unexpected deletion is possible.

Destructive or potentially surprising actions should require confirmation, including:

- reinstalling DRH
- uninstalling DRH
- replacing the installed game with an older version
- removing Steam shortcuts

## Partial Updates

GitHub does not provide practical partial downloads from release archives. To avoid downloading the full game for small updates, DRH Launcher should later support pack-based updates.

Example packs:

```text
core
resources-config
resources-ui
resources-audio
resources-art2d
resources-art3d
```

Each release manifest still describes a complete install state. The launcher downloads only packs whose hashes differ locally.

This is preferred over incremental `Vx -> Vx.y` patch chains because it keeps fresh installs, skipped versions and rollback easier to reason about.

## Version History

The launcher should show changelogs and allow installing older versions.

Older versions must be presented as potentially incompatible with:

- current saves
- current mods
- current servers or APIs
- current launcher assumptions

Installing an older version should be an explicit replacement action.

## Local User Data

Dungeon Rampage Haxe is currently entirely multiplayer and does not store local save data that DRH Launcher needs to preserve.

Even so, the launcher should avoid deleting unrelated files in the install directory. Reinstall and uninstall actions should operate on files and directories owned by DRH Launcher whenever possible.

## Steam Integration

Steam integration is best-effort because Steam does not expose a simple public desktop API for non-Steam shortcuts.

Target behavior:

- add a Steam shortcut named `Dungeon Rampage Haxe`
- point it to `DRH-Launcher --play`
- optionally remove shortcuts created by DRH Launcher

The UI should make this explicit and avoid treating Steam shortcut editing as guaranteed.

## Mods

Mods are planned for a later phase.

Initial intended layout:

```text
mods/
  SomeMod/
    mod.json
    Resources/
```

Expected future features:

- enable or disable mods
- load order
- compatibility with DRH versions
- open mods folder
- verify modified files

The game may need explicit support to load mods cleanly. Until then, the launcher should avoid destructive file overlays when possible.

## Settings

Settings should eventually include:

- install directory
- release channel
- pre-executable command
- game arguments
- Steam shortcut actions
- reinstall or uninstall DRH
- launcher logs and diagnostics
- uninstall guidance for DRH Launcher

Full self-uninstall may require platform-specific packaging support.

## Logging

DRH Launcher should write logs under:

```text
launcher-data/logs/
```

The launcher should eventually provide an in-app log viewer for recent logs, plus an action to open the logs directory in the platform file manager.

Logs should be useful for diagnosing:

- update checks
- downloads
- hash verification
- extraction
- install replacement
- game launch failures
- Steam shortcut actions

## Info Page

The info page should include:

- Discord link
- GitHub project links
- explanation of DRH and DRH Launcher
- license information
- technology credits
- installed DRH version
- DRH Launcher version
