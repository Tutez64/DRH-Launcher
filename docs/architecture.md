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
  data/
    installed.json
    logs/
      launcher.log
    cache/
    downloads/
    staging/
  Dungeon Rampage Haxe/
    current/
      Dungeon Rampage Haxe
      Resources/
      DbConfiguration/
    previous/
      Dungeon Rampage Haxe
      Resources/
      DbConfiguration/
  mods/
```

The exact executable names and native libraries vary by platform.

`config.json` is launcher configuration and currently lives in the platform application config directory. It should not be confused with install-owned metadata under `<install-dir>/data/`.

`data/installed.json` is owned by DRH Launcher and records what DRHL believes is installed in `Dungeon Rampage Haxe/current/` and, when present, `Dungeon Rampage Haxe/previous/`. It should be written after a successful install, update or rollback. Installed release metadata can include the release `launch_options`; the Options page should use that local metadata so it matches the installed game even when a newer release exists.

`Dungeon Rampage Haxe/previous/` is reserved for a simple rollback path. It contains the previous `current/` directory after an update replaces it.

## Install State

The launcher should model installation state explicitly instead of relying on scattered booleans.

Expected states:

```text
NotInstalled
Installed
UpdateAvailable
Updating
Playing
BrokenInstall
LaunchableButMaybeOutdated
```

`UpdateAvailable` means a newer compatible version exists and the launcher can offer or start an update.

`Playing` means DRH was launched by DRH Launcher and the launcher still sees the child process running. While in this state, the primary action should become `Stop` instead of launching another instance. This state is process-based UI state, not installed metadata written to disk.

`LaunchableButMaybeOutdated` means the installed game appears runnable, but the launcher cannot confirm that it is up to date or cannot update it automatically right now. Examples include offline mode, GitHub check failure, failed download, missing manifest data, or an older version that is still accepted. The primary action can remain `Play`, with a warning or secondary update action. The UI should expose the concrete reason when available, such as "Could not check GitHub releases" or "Download failed".

`BrokenInstall` means the configured install directory exists but required files are missing or inconsistent.

## Main UI

The default visual direction is a dark theme with orange and red accents, matching the broad color mood of Dungeon Rampage Haxe.

Future versions may add more custom visual elements inspired by the game, but this is not a priority for the first implementation. The UI should stay clear, readable and practical before becoming more decorative.

An optional light theme can be added later if Slint makes it straightforward to maintain both themes without duplicating too much UI code.

The home screen should show:

- a prominent `Install DRH` button when the game is missing
- a prominent `Play` button after installation
- a prominent `Update` button when a newer compatible release is known
- a prominent `Stop` button while a DRH process launched by DRH Launcher is still running
- compact installed version / latest-version / update-progress support text near the primary action
- manual `Check for updates` action

DRH Launcher checks for updates automatically at startup. Manual checks are still available. Update checks should use the latest GitHub release metadata and do not need to download the release manifest; the manifest is only needed when preparing an install or update.

The main navigation is:

- `Home`
- `Options`
- `Mods`
- `Settings`

The home screen should stay focused on the primary install/play/update/stop flow. It should not permanently show long logs, install paths or diagnostic details. When extra context is useful, such as download progress, verification, or update availability, it should appear as compact support text near the primary action.

Discord and GitHub links can live in `Settings > About` or another secondary location. They should be easy to find without taking space away from the primary install/play flow.

Useful secondary actions:

- go to launch options
- check for updates
- verify or repair the installed game
- open recent logs

The home UI state should be derived from a small view model rather than scattered direct widget updates. This keeps installed state, latest-release state, process state and temporary progress messages easier to reason about as the launcher grows.

## Launch Modes

The launcher should support at least two modes:

```text
DRH-Launcher
DRH-Launcher --play
```

`DRH-Launcher` opens the full UI.

`DRH-Launcher --play` is intended for Steam and shortcuts. It should quickly check required state, apply or prompt for important updates when needed, then launch DRH without forcing the full UI when everything is ready.

When DRH is launched from the launcher UI, DRH Launcher keeps the child process handle and uses it to prevent multiple launches from the same launcher instance. If the process exits normally, the UI returns to the installed state. If the user presses `Stop`, DRH Launcher terminates the tracked process and returns to the installed state.

This tracking only covers processes started by the current launcher instance. Detecting a DRH process launched directly by the user or by another launcher instance can be added later if it proves useful, but it should be treated carefully to avoid killing an unrelated process by mistake.

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

DRH feature flags currently accept command-line values in this shape:

```text
--flag
--flag true
--flag false
```

Passing a flag without a value enables it. Passing `false` disables it, including flags whose game default is `true`.

Known game arguments should eventually be described by release metadata rather than parsed from source code at runtime. The UI currently exposes launch arguments as an explicit mode:

- `Game defaults`: launch DRH without extra launcher-provided game arguments.
- `DRHL recommended`: the launcher default, built from per-argument `recommended` values in the release manifest.
- `Custom`: use the manually entered argument string.

The source of truth in DRH is currently the constructor of `src/brain/utils/FeatureFlags.hx`, where feature flags and their default `true` / `false` values are listed.

The preferred long-term flow is:

1. DRH extracts feature flag metadata during its own build.
2. The generated metadata is shipped with the DRH release.
3. DRH Launcher reads that metadata from the installed release or release manifest.
4. DRH Launcher presents known flags as user-facing options.

This avoids runtime source parsing in DRH Launcher and keeps each launcher version compatible with the DRH version it installs.

When launch option metadata is not available for the installed release, the UI should say so in user-facing language and still allow manual custom arguments.

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

Debug builds use `Tutez64/DRHL-Release-Fixtures` by default. Release builds use the real DRH repository by default. `DRHL_RELEASE_SOURCE=fixtures` or `DRHL_RELEASE_SOURCE=drh` can override this behavior explicitly.

The fixtures repository is intended for testing manifests, missing assets and small downloadable archives without polluting real DRH releases.

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

The first installation implementation installs from `data/staging/extracted/` into `Dungeon Rampage Haxe/current/`, moving the previous active version to `Dungeon Rampage Haxe/previous/` when present.

After a successful replacement, DRH Launcher should write `data/installed.json` with the active release metadata and previous release metadata when available, such as:

```json
{
  "active": {
    "version": "V9",
    "platform": "linux-x64",
    "source": "Tutez64/Dungeon-Rampage-Haxe",
    "release_url": "https://github.com/Tutez64/Dungeon-Rampage-Haxe/releases/tag/V9",
    "archive": "Dungeon.Rampage.Haxe.V9.Linux.tar.gz",
    "archive_sha256": "...",
    "installed_at": "2026-05-25T12:34:56Z",
    "launch_options": {
      "game_arguments": []
    }
  },
  "previous": {
    "version": "V7",
    "platform": "linux-x64",
    "source": "Tutez64/Dungeon-Rampage-Haxe",
    "release_url": "https://github.com/Tutez64/Dungeon-Rampage-Haxe/releases/tag/V7",
    "archive": "Dungeon.Rampage.Haxe.V7.Linux.tar.gz",
    "archive_sha256": "...",
    "installed_at": "2026-05-20T10:00:00Z"
  }
}
```

DRH Launcher should derive installed metadata from the GitHub release and downloaded archive it used.

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
  },
  "launch_options": {
    "game_arguments": [
      {
        "name": "want-zoom",
        "flag": "--want-zoom",
        "default": false,
        "recommended": true
      },
      {
        "name": "use-hd-assets",
        "flag": "--experimental-use-hd-assets",
        "default": false,
        "config_key": "experimental_use_hd_assets"
      }
    ]
  }
}
```

`config_key` is optional. It records the matching DRH JSON configuration key when it differs from `name`; DRH Launcher still launches with `flag`.
`recommended` is optional on each game argument. When omitted, DRH Launcher treats the recommended value as equal to `default`.

DRH Launcher resolves `archive` against the GitHub release assets. For the first implementation, manifests should not point to arbitrary external download URLs.

Later versions of the manifest can add:

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
- extraction happens in `data/staging/`, never directly over the current game
- replacement happens only after validation succeeds
- failed replacement should keep or restore the previous launchable game directory when possible
- a successful replacement should move the old `Dungeon Rampage Haxe/current/` directory to `Dungeon Rampage Haxe/previous/`
- an existing `Dungeon Rampage Haxe/previous/` can be removed before creating a new rollback copy
- updates should not proceed while the game is running
- partial or inconsistent installs should be reported as `BrokenInstall`

The launcher should prefer a clear recovery action over silent repair when data loss or unexpected deletion is possible.

The initial rollback model is intentionally simple:

```text
Dungeon Rampage Haxe/current/   active version
Dungeon Rampage Haxe/previous/  previous version, if available
```

For example, if a user updates from `V7` to `V9`, `Dungeon Rampage Haxe/previous/` should contain `V7`, and `installed.json.previous` should record that it is `V7`. This allows the launcher to offer a clear rollback target even if an intermediate release such as `V8` was skipped or known bad.

The UI can later expose `Restore previous version` when `Dungeon Rampage Haxe/previous/` exists. Full multi-version management can be added later if there is a real need.

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

`Settings` is the launcher-specific area. It is split into sub-sections:

- `General`
- `Logs`
- `About`

`General` should include:

- install directory
- config file path
- release channel
- reinstall or uninstall DRH
- uninstall guidance for DRH Launcher

`Logs` should include:

- a recent in-app launcher log view
- a refresh action
- an action to open the logs directory

`About` should include:

- DRH Launcher version
- active release source
- Discord and GitHub links
- license information
- technology credits

Full self-uninstall may require platform-specific packaging support.

## Logging

DRH Launcher should write logs under:

```text
data/logs/launcher.log
```

The launcher provides an in-app recent log viewer in `Settings > Logs`, plus an action to open the logs directory in the platform file manager. Log writes should be best-effort: failure to write diagnostics must not break install, update or launch flows.

Log entries use readable UTC timestamps and severity levels.

Logs should be useful for diagnosing:

- update checks
- downloads
- hash verification
- extraction
- install replacement
- game process start, exit and stop actions
- game launch failures
- Steam shortcut actions

## About Page

The `Settings > About` page should include:

- Discord link
- GitHub project links
- license information
- technology credits
- DRH Launcher version
- active release source
