Working changelog for the next release. Update it as work lands.\
At tag time, copy these sections into the GitHub release notes under `Changelog`.


### Added

- Added a card above **Play** that appears in the following situations 
(dots cycle when several apply; hovering shows the full detail):
  - Steam is missing or installed but not running.
  - This DRH version is based on an outdated Dungeon Rampage release.
    If a newer DRH is already available, the notice tells
    you to update or restore it; if you are already on the latest DRH, it
    tells you to wait for the next DRH release.
  - Dungeon Rampage is not in the local Steam library. A
    dismissible reminder explains that DRH needs an **owned** copy to connect.
    The game name opens the Steam store, with the web store as fallback.
    Dismissing it hides the reminder permanently;
    **Settings → General** can show it again.
- Added a live player count next to Discord on Home, which includes DRH.
  Hovering explains that, and that Steam refreshes the figure about every
  five minutes. The chip is hidden when the count cannot be fetched.
- **Settings → Logs**:
  - Added the ability to select and copy the text (click-and-drag,
    Shift-click, double-click a row, Ctrl+A / Cmd+A, Ctrl+C / Cmd+C, 
    context menu with  "Copy" and "Select all"). Wrapped lines are joined.
  - Added a footer with the logical line count (launcher logs are still limited to 24 KiB).
    For game sessions, the on-disk size was moved from
    the session list entries to the footer.
  - Game-session viewer header: added a **Hide Haxe warnings** setting
    that omits DRH's repeated Haxe iteratee FIXME lines (on by default).

### Fixed

- **Stop** fully covers DRH started through a pre-launch wrapper like
  `prime-run`. If the wrapper exits on SIGTERM while the game is still
  running (including when DRH ignores SIGTERM), DRHL waits for the rest
  of the launch tree, then force-stops it after a short timeout
  (fixes [#3](https://github.com/Tutez64/DRH-Launcher/issues/3)).
- After Restore or Stop, Home keeps **Update** when a newer DRH release is
  already known, instead of dropping back to **Play**.
- The Home Play button is now correctly centered.
- Home labels, buttons, and icons are sharper (no more blur from clipping the
  whole panel).
- Already installed DRH copies can fill in missing frame-rate options (V11+)
  and Steam BuildIDs (mapped for V10–V13, from the release manifest from V14)
  without a reinstall.
- Checking GitHub releases no longer resets unsaved edits on **Options**.
- Closing DRHL, updating it, or installing the AppImage just after DRH exits
  no longer leaves Home stuck on **Stop** or skips compressing the
  game-session log.

### Improved

- Home uses a 256px DRH icon instead of a 1024px one,
  which reduces the launcher binary by ~1.3 MiB (~2.6 MiB on macOS).
- Release binaries are stripped, and Slint's unused system-tray stack is
  left out, which reduces the launcher binary by ~6 MiB on Linux and macOS.
- Listing game sessions no longer decompresses every completed log.
  New archives store a small listing prefix.
  Older archives are rewritten with that prefix the first time
  **Settings → Logs** is opened, with a progress line so the window stays usable.
- Home leaves **Stop** much quicker (almost immediately) when DRH exits.
- **Versions**, **Mods**, and **Settings** no longer repeat the page name as
  an in-page heading.
- Internal: one install/repair replacement path, a structured Home view
    model, shared IEC size formatting, and some dead UI code removed.