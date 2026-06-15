# Packaging and Releases

DRH Launcher and Dungeon Rampage Haxe are released independently. This document
covers DRH Launcher releases only.

## Release Artifacts

The release workflow builds:

| Platform | User package | Updater package |
| --- | --- | --- |
| Linux x64 | AppImage | the signed AppImage |
| Windows x64 | current-user NSIS installer | the signed NSIS installer |
| macOS universal | DMG | signed `.app.tar.gz` |

`latest.json` maps the updater's runtime platform identifiers to those signed
artifacts. `SHA256SUMS` is provided for manual verification.

## Linux User Installation

The AppImage is the only officially supported Linux package for the first
release. It can remain portable, but the normal user flow is:

1. Download and launch the AppImage.
2. Confirm `Install DRH Launcher`.
3. DRHL copies itself atomically to
   `~/Applications/DRH-Launcher.AppImage`.
4. DRHL installs its icon and desktop entry under `$XDG_DATA_HOME`.
5. DRHL removes the downloaded AppImage when possible.
6. DRHL restarts from the installed copy.

No root access or installation-directory choice is required. Automatic launcher
updates are enabled only for this managed copy, so DRHL never overwrites an
arbitrary AppImage in `Downloads`. Copying before deleting also works when the
download and application directories are on different file systems. Failure to
delete the original does not invalidate an otherwise successful installation.

The portable AppImage exposes only the 256 px launcher icon to the desktop.
Smaller variants are stored outside its icon theme and are copied into the
user's `hicolor` theme during installation. This prevents desktop environments
from selecting and enlarging a low-resolution icon on the first launch.

Once installed, AppImage management and uninstall are available under
`Settings > General > Advanced settings`. Uninstall removes the AppImage and
desktop integration while preserving DRH, settings, downloads and logs.

Native `.deb`, `.rpm` or distribution-repository packages may be added later.
They are intentionally outside the first-release scope because each package
family adds separate installation, update and validation responsibilities.

### Wayland Window Icon

Slint 1.16.1 uses winit 0.30.13, whose Wayland backend ignores the window icon
provided by Slint. Before desktop integration, a native Wayland compositor can
therefore show its generic Wayland icon. Once installed, the desktop entry uses
the absolute path of the installed 256 px icon so KWin can associate it with the
launcher window. This is independent from the taskbar icon exposed by the
AppImage itself.

winit implements the `xdg-toplevel-icon-v1` protocol in its 0.31 development
line. Revisit this limitation once Slint uses a stable winit release containing
that support; forcing DRHL through XWayland only for a decoration icon is not a
reasonable release tradeoff.

## One-Time Repository Setup

Install the pinned packager and generate a password-protected update key:

```bash
cargo install cargo-packager --version 0.11.8 --locked
cargo packager signer generate --path drhl-update.key
```

Do not commit either generated key file.

Configure the GitHub repository:

| Kind | Name | Value |
| --- | --- | --- |
| Actions secret | `CARGO_PACKAGER_SIGN_PRIVATE_KEY` | contents of `drhl-update.key` |
| Actions secret | `CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD` | key password |
| Actions variable | `DRHL_UPDATE_PUBLIC_KEY` | contents of `drhl-update.key.pub` |

The workflow intentionally fails when one of these values is absent. Every
published launcher must embed the public key and every updater artifact must be
signed.

Keep an offline backup of the private key and password. Key rotation requires a
release signed by the old key that embeds the new public key; losing the old key
breaks automatic updates for already installed launchers.

## Publishing

1. Update `package.version` in `Cargo.toml`.
2. Update `Cargo.lock`, run the checks, and commit.
3. Create and push the matching tag.

```bash
cargo test --all-targets --locked
cargo clippy --all-targets --all-features -- -D warnings
git tag v0.1.0
git push origin master v0.1.0
```

The tag must exactly equal `v` followed by the Cargo version. GitHub Actions
builds on native runners, creates `latest.json`, then creates a draft GitHub
Release containing all artifacts.

Pre-release Cargo versions such as `0.2.0-alpha` use matching tags such as
`v0.2.0-alpha`. The workflow marks these GitHub Releases as pre-releases.
GitHub does not expose drafts or pre-releases as `Latest`, so installed
launchers do not discover them through `/releases/latest/download/latest.json`.

Download and test the Linux, Windows and macOS artifacts from the draft. Once
they have been validated, publish the draft manually from GitHub. Only then does
it become the public `Latest` release and become visible to installed launchers
through `/releases/latest/download/latest.json`.

If a candidate is invalid, delete the draft and its tag, fix the issue, then
create the tag again. Published release tags and artifacts should be treated as
immutable; corrections after publication require a new version.

## Local Packaging

Build the native package for the current platform:

```bash
cargo build --release --locked
cargo packager --release --formats appimage
```

Replace `appimage` with `nsis` on Windows or `app,dmg` on macOS. Local packages
are not updater-signed unless the two `CARGO_PACKAGER_SIGN_*` environment
variables are set.

## Platform Trust

The updater signature verifies that a package was produced with the DRHL update
key. It does not provide operating-system publisher reputation.

- Windows Authenticode is needed to reduce SmartScreen warnings.
- Apple Developer ID signing and notarization are needed for normal Gatekeeper
  distribution.
- Until those credentials exist, release notes must state the expected first-run
  warnings and the project must not imply that the packages are OS-verified.
