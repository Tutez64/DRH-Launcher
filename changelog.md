Working changelog for the next release. Update it as work lands.
At tag time, copy these sections into the GitHub release notes under `Changelog`.

### Added

- **Mods** page: the mods of the index in **Catalog**, the installed ones in **Installed**. Installing or enabling a mod installs and enables what it needs, the `api` included. Labels show what a mod builds on, whether the index reviewed it, which mods need it, and what may keep it from working; an action that installs or enables other mods, or would break one, asks first. After a game, the page shows how it went: which mods failed, and which ran interpreted. Before mods are first used, a short notice says what playing with them means. Play passes the enabled mods to DRH, unless the launch arguments name their own `--mods-dir`.
- Commands for modders, run from a terminal without opening the launcher: `--new-mod` starts a mod from the template, `--check-mod` checks it, `--run-mod` plays it with what it needs, `--pack-mod` builds its release, `--submit-mod` proposes it to the mod index. The index's CI runs `--check-entry` and `--build-index`. On Windows, `DRH-Launcher.com`, installed next to the launcher, lets a terminal wait for them and show their output.

### Fixed

- The Linux AppImage's `AppRun` is executable by every user (mode 755 instead of 744), so the AppImage also starts in sandboxes such as Firejail (https://github.com/AppImage/appimage.github.io/pull/9233).

### Improved

- Updated Slint, the UI toolkit, to 1.18:
  - **Versions**: inline code in release notes is shown in a monospace font with a background.
  - **Options**: the context menu of the **Pre-launch command** and **Extra arguments** fields has **Undo** and **Redo**.
  - Icons are slightly sharper; drop shadows are no longer redrawn on every frame; on Wayland, the first frame is drawn at the window's actual size.
