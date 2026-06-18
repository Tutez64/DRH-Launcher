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
- Linux desktop app ID: `io.github.Tutez64.DRHLauncher`
- Linux desktop icon name: `DRH-Launcher`
- Steam entry for playing: `Dungeon Rampage Haxe`

Steam should point to the launcher, not directly to the game executable.

## Application and Managed Data Layout

DRH Launcher itself is installed as a platform application:

- Linux: an AppImage installed for the current user by DRHL itself
- Windows: a current-user NSIS installation
- macOS: `DRH Launcher.app`, normally copied from a DMG

On Linux, the downloaded AppImage is both the bootstrap and portable package.
When it is launched outside DRHL's managed location, the launcher offers to:

```text
copy itself atomically to ~/Applications/DRH-Launcher.AppImage
install its icon under $XDG_DATA_HOME/icons/
install its desktop entry under $XDG_DATA_HOME/applications/
remove the downloaded AppImage when possible
restart from the managed AppImage
```

This installation is per-user and does not require administrator privileges.
Choosing `Not now` keeps the current AppImage portable. Native distribution
packages may be added later, but are not a first-release requirement.

The configured `install_dir` is not the launcher executable location. It is the
launcher-managed content root containing DRH, downloads, logs and install
metadata. Keeping application files and managed content separate lets the
launcher update itself without touching the game rollback directories and lets
the game update without replacing the launcher. DRH Launcher uses a fixed
default install directory for the current platform and persists it in
`config.json` on first startup. The UI shows this path for diagnostics, but the
user does not choose a different install root in the first release.

The intended managed content layout is:

```text
<install-dir>/
  data/
    installed.json
    logs/
      launcher.log
      game/
        <active-session-timestamp>.log
        <completed-session-timestamp>.log.zst
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
When active install metadata and a matching cached archive are available, `Repair` should first reinstall the active version from the verified cached archive. If local repair is not possible, the launcher may fall back to the latest compatible release flow.

## Main UI

The default visual direction is a dark theme with orange and red accents, matching the broad color mood of Dungeon Rampage Haxe.

Slint UI colors should be defined in `ui/theme.slint` through the exported `Theme` global, then referenced by semantic names from `ui/app.slint`. New Slint components should not introduce raw color literals unless they are deliberately adding a new palette entry.

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
- `Versions`
- `Mods`
- `Settings`

The home screen should stay focused on the primary install/play/update/stop flow. It should not permanently show long logs, install paths or diagnostic details. When extra context is useful, such as download progress, verification, update availability, repair, reinstall, version install, or stop in progress, it should appear as compact support text near the primary action through `home_support_text()`.

Discord and GitHub links can live in `Settings > About` or another secondary location. They should be easy to find without taking space away from the primary install/play flow.

Useful secondary actions:

- check for updates
- go to options
- open a compact Help menu for recovery actions such as restore, reinstall and logs

The home UI state should be derived from a small view model rather than scattered direct widget updates. This keeps installed state, latest-release state, process state and temporary progress messages easier to reason about as the launcher grows. During install, repair, reinstall, or version install operations, the Home title should switch to `DRH is updating` while the support text shows the current step. Home error feedback should stay compact: show a short excerpt of the failure and point users to `Settings > Logs` for the full message.

### Version History UI

The `Versions` page shows DRH and DRH Launcher release history with changelogs
from GitHub releases. DRH entries can install a selected historical release when
a compatible package or manifest is available. Replacing an already installed
DRH version requires an explicit confirmation because older game releases may
not be playable anymore.

Changelog rendering is intentionally best-effort for now. Slint 1.16 provides
`StyledText` and Markdown parsing for inline styling and links, but it does not
cover the full GitHub Markdown surface needed by release notes, such as headings,
images, block quotes and fenced code blocks as a single runtime Markdown
document. DRH Launcher therefore keeps a small block-level adapter around
`pulldown-cmark`: block structure is handled locally, while inline spans are
rendered through Slint `StyledText`.

Revisit this once Slint 1.17 is released so DRHL can use the public runtime
`StyledText` Markdown API instead of Slint's private unstable Rust re-export.
Revisit it again if Slint later grows a fuller Markdown renderer; at that point
the local block adapter should be removed or reduced substantially.

## Launch Modes

The launcher should support at least two modes:

```text
DRH-Launcher
DRH-Launcher --play
```

`DRH-Launcher` opens the full UI.

`DRH-Launcher --play` is intended for Steam and shortcuts. It should quickly check required state, apply or prompt for important updates when needed, then launch DRH without forcing the full UI when everything is ready.

For the first release, when an update is available, `--play` opens the full UI with an explanatory message instead of updating silently. Steam shortcut integration itself is deferred until a later phase.

When DRH is launched from the launcher UI, DRH Launcher keeps the child process handle and uses it to prevent multiple launches from the same launcher instance. If the process exits normally, the UI returns to the installed state. If the user presses `Stop`, DRH Launcher terminates the tracked process and returns to the installed state. Stop requests, graceful shutdown, forced termination after timeout, and the final process result should all be written to `launcher.log` in addition to the game-session log.

Closing DRH Launcher while its tracked DRH process is still running requires
confirmation. Confirming first requests a normal application shutdown, then
falls back to forced termination after a short timeout. DRHL waits for the
process to exit, finalizes and compresses the game-session log, then closes. If
stopping DRH or finalizing the session fails, the launcher remains open and
reports the error.

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

Game updates and launcher updates are independent release streams. DRH uses tags
such as `V10` in the DRH repository. DRH Launcher uses semantic versions such as
`v0.1.0` in the DRH Launcher repository.

### Launcher Packaging and Self-Updates

DRH Launcher releases are built by GitHub Actions with pinned `cargo-packager`
tooling:

```text
linux-x64       AppImage
windows-x64     current-user NSIS installer
macos-universal universal .app in a DMG
```

The macOS update bundle contains the same universal app for both
`macos-x86_64` and `macos-aarch64`.

Each release includes:

- the user-facing platform packages
- a cargo-packager update bundle where needed
- Minisign signatures generated from the DRHL update key
- `latest.json` for `cargo-packager-updater`
- `SHA256SUMS` for manual verification

The update private key exists only in GitHub Actions secrets. Its public key is
embedded in release builds through `DRHL_UPDATE_PUBLIC_KEY`. Release builds must
fail rather than publish packages when either key is missing.

Release builds use GitHub's `Latest` release manifest by default. Test builds
can override the launcher update manifest endpoint at compile time with
`DRHL_UPDATE_ENDPOINT`; installed builds can also be redirected for a test run
with the same runtime environment variable. Official release builds leave the
compile-time value unset.

Pushing a matching version tag builds the native packages and creates a draft
GitHub Release. The generated artifacts are tested from that draft before it is
published manually. Draft releases are not exposed through GitHub's `latest`
release endpoint, so launcher clients only discover a version after validation.

DRHL checks for its own updates independently from the DRH release check. Only
an available launcher update is shown, as a contextual notification on `Home`.
Installing it is an explicit action and is refused while a game process tracked
by DRHL is running. The downloaded package is verified with the embedded public
key before it is installed, then DRHL restarts.

On Linux, automatic updates only replace the AppImage in DRHL's managed
location. A portable AppImage first offers installation and does not silently
replace itself in an arbitrary download directory. Development binaries and
other unsupported installation formats link to the latest GitHub release
instead.

`Settings > General` exposes Linux installation and repair prominently when
needed. Once installed, AppImage management moves under advanced settings.
Linux uninstall removes the managed AppImage, icon and desktop entry, but
preserves launcher configuration, DRH installations, downloaded archives and
logs. On Windows and macOS, launcher removal is handled by the platform
installer or by deleting the installed application bundle; DRHL does not need a
separate self-uninstall action on those platforms for the first release.

Minisign update signatures do not replace platform code signing:

- unsigned Windows builds can still trigger SmartScreen
- non-notarized macOS builds can still trigger Gatekeeper
- Apple Developer ID signing/notarization and Windows Authenticode should be
  enabled when credentials are available

The packaging and updater implementation is isolated from the game installer.
`cargo-packager` and `cargo-packager-updater` are pinned because their updater is
still described upstream as preview functionality.

### Game Updates

The first game update implementation uses full archives from GitHub releases.

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

1. Reuse a matching cached archive from `data/downloads/` when present.
2. Download to `data/downloads/` when no valid cached archive exists.
3. Verify size and SHA-256 before extraction.
4. Extract to a staging directory.
5. Validate expected files.
6. Replace the existing game directory.
7. Keep enough state for rollback or recovery.

Downloaded archives are cache entries. DRH Launcher records recently used archives in `data/downloads/cache.txt` and prunes older entries according to `config.json.download_cache_limit`, which defaults to `3`.

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
Repairing or reinstalling the active install replaces only `Dungeon Rampage Haxe/current/` and preserves `Dungeon Rampage Haxe/previous/` when present, so an existing rollback target is not lost during recovery.

The initial rollback model is intentionally simple:

```text
Dungeon Rampage Haxe/current/   active version
Dungeon Rampage Haxe/previous/  previous version, if available
```

For example, if a user updates from `V7` to `V9`, `Dungeon Rampage Haxe/previous/` should contain `V7`, and `installed.json.previous` should record that it is `V7`. This allows the launcher to offer a clear rollback target even if an intermediate release such as `V8` was skipped or known bad.

The UI always shows `Restore previous version` in the Help menu. The action is enabled only when `installed.json.previous` exists and `Dungeon Rampage Haxe/previous/` exists on disk. When rollback metadata exists but the directory is missing, the button stays visible and disabled, matching the reinstall control. Restoring swaps `current/` and `previous/`, so the version being replaced remains available as the next rollback target. The replaced version is also recorded as `blocked_update_version`: if the user restores from `V10` to `V9`, `V10` should not be proposed again automatically, but a later release such as `V11` should be offered normally. Full multi-version management can be added later if there is a real need.

Help-menu recovery actions such as restore and reinstall do not require an extra confirmation dialog in the first release. Restore is reversible in one click and both actions are labeled explicitly from installed metadata.

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

The launcher exposes a `Versions` screen above `Mods`. It has two sub-tabs:

- `Dungeon Rampage Haxe`
- `DRH Launcher`

Both tabs load GitHub release history and show the selected release changelog in
the launcher UI. Changelogs are parsed from GitHub release Markdown and rendered
as readable in-app blocks for headings, paragraphs, lists, quotes, code blocks
and separators.

The DRH tab lists releases from the configured DRH release source, marks whether
the current platform has an installable asset, and allows installing or
replacing the installed DRH version with a selected release. Before installing a
selected release, the launcher re-fetches that release by tag and uses manifest
metadata when available. Replacing an already installed version requires an
explicit confirmation action.

The DRH Launcher tab is informational for now. It shows published DRHL releases,
including changelogs and links to GitHub. Installing older launcher versions
from inside DRHL is intentionally out of scope because launcher self-updates use
the signed latest-release updater flow.

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

Steam integration is deferred for the first release. The README and UI should continue to treat it as a later phase.

When implemented, it will be best-effort because Steam does not expose a simple public desktop API for non-Steam shortcuts.

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

- the install directory, described as the folder that contains launcher data, downloads, logs and `Dungeon Rampage Haxe/`
- an action to open the install directory
- an `Application` card for Linux AppImage installation or repair when needed
- the active AppImage path and an action to open its folder
- installed Linux AppImage management under advanced settings
- `download_cache_limit`, with clear help text explaining that it controls how many verified release archives are kept in `data/downloads/` for faster reinstalls, repairs and rollbacks
- an advanced section, collapsed by default, for diagnostics-only details such as the `config.json` location

`Logs` should include:

- a recent in-app launcher log view
- a refresh action
- an action to open the logs directory

`About` should include:

- DRH Launcher version
- Discord and GitHub links
- license information
- technology credits

Full self-uninstall may require platform-specific packaging support.

## Logging

DRH Launcher should write logs under:

```text
data/logs/launcher.log
data/logs/game/<active-session-timestamp>.log
data/logs/game/<completed-session-timestamp>.log.zst
```

`launcher.log` contains DRH Launcher diagnostics. Each DRH process launched by the
launcher gets a separate game-session log containing launch metadata, the game's
standard output and standard error, and the final process result.

Game output is written directly to an uncompressed `.log` while the process is
running. When the session ends, the launcher appends its result and compresses
the complete file as an independent Zstandard archive at level 10. The original
`.log` is removed only after the `.log.zst` has been written successfully.
An uncompressed `.log` therefore represents an active session or a recoverable
session whose finalization was interrupted or failed. The viewer handles both
states. Sessions are not deleted based on their count. Session filenames use
sortable UTC timestamps so a specific play session can be identified and shared.

The launcher provides an in-app log viewer in `Settings > Logs` with separate
launcher and game-session views, plus actions to open a selected session or the
logs directory in the platform file manager. Game log lines use the five DRH
severity levels: `DEBUG`, `INFO`, `WARN`, `ERROR` and `FATAL`. Completed session
entries show the game version, duration and compressed file size. Opening a
compressed session externally creates an uncompressed copy in the system
temporary directory so it can be handled by a regular text editor.

The log viewer measures its available layout width using the bundled Hack
monospace font used for display, then splits logical lines into fixed-height
visual rows. Bundling the font keeps character measurement consistent across
platforms. Those rows stay virtualized, which keeps large files quick to load
and prevents the scrollbar geometry from changing while scrolling. Resizing the
viewer recalculates the segments without changing the underlying log file or
losing content.

Display wrapping prefers whitespace, then commas, semicolons or colons near the
right edge before falling back to a hard character boundary. The launcher keeps
the launcher log scroll position and a separate scroll position for every game
session file while navigating between screens and sessions.

Launcher log writes should be best-effort: failure to write diagnostics must not
break install, update or launch flows. A game-session log must be created before
DRH is launched so graphical launches do not silently lose game output.

When no install directory is configured yet, launcher diagnostics should still be
written under the default install directory path so startup and first-install
activity is not lost. On first startup, DRH Launcher should persist that default
install directory in `config.json`, create the managed logs directory, and use
the same path for log viewing and `Open logs folder`.

Install and update logging should follow a simple split:

- atomic file operations and metadata writes are logged from the installer layer as they happen
- the final user-facing outcome for install, repair, reinstall, and restore is logged once from the UI orchestration layer

Hash, size, extraction, and install failures should be written at `ERROR` level at the point of failure. Successful install outcomes should not be logged twice by both the orchestration layer and its caller.

Log entries and session metadata use readable UTC timestamps.

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
