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

The draft GitHub Release notes start with a prominent `Download DRH Launcher`
section using simple operating-system labels: Linux, Windows and macOS. These
links point to the three user-facing packages only, followed by a bold Markdown
link to the README installation instructions and a `Changelog` section for the
generated release notes.

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
Release containing all artifacts. The generated release notes include a
prominent download section that points users to the Linux AppImage, Windows
installer and macOS DMG so they do not need to choose among updater and checksum
files.

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

## GitHub Test Packages

Use the manual `Test Packages` workflow to generate packages for all platforms without 
having to install Rust, Python or cargo-packager in the test VMs. This
workflow only uploads Actions artifacts; it does not create tags or GitHub
Releases.

Inputs:

| Input | Purpose |
| --- | --- |
| `linux_update_base_url` | Linux HTTP(S) directory that will serve the generated `update-server` artifact. |
| `windows_update_base_url` | Windows HTTP(S) directory that will serve the generated `update-server` artifact. |
| `macos_update_base_url` | macOS HTTP(S) directory that will serve the generated `update-server` artifact. |
| `build_current` | Also builds installable current-version packages for full current-to-update tests. |
| `current_version` | Optional version for the current packages; blank uses `Cargo.toml`. |
| `update_version` | Optional version for the update packages; blank uses the next patch test version. |

The workflow always creates `drhl-test-update-server`, containing signed update
artifacts, `latest.json` and `SHA256SUMS`. When `build_current` is enabled, it
also creates `drhl-test-current-linux`, `drhl-test-current-windows` and
`drhl-test-current-macos`.

Typical full update test:

1. Run `Test Packages` with `build_current` enabled.
2. Set each platform update URL to an address reachable from that OS.
3. Download and extract the `drhl-test-update-server` artifact.
4. Serve that directory with any static HTTP server.
5. Install the matching `drhl-test-current-*` package on the target OS.
6. Start DRHL and check for launcher updates.

### VM Update Server Networking

Serve the extracted `drhl-test-update-server` directory from the host. Python's
default `http.server` binding is usually enough; `--bind 0.0.0.0` only makes the
intent explicit when the guest reaches the host through a VM network address:

```bash
python3 -m http.server --bind 0.0.0.0 --directory /path/to/update-server 8000
```

Use the matching host address for each platform URL when triggering the
workflow. The defaults below match the current maintainer test setup, but VM
network addresses are environment-specific and may differ on another machine.

| VM network mode | Workflow URL |
| --- | --- |
| Linux on the same host as the server | `linux_update_base_url=http://127.0.0.1:8000` |
| virt-manager/libvirt NAT on `virbr0` | `windows_update_base_url=http://192.168.100.1:8000` |
| OSX-KVM scripts using QEMU user networking | `macos_update_base_url=http://10.0.2.2:8000` |
| VM bridged to the LAN | `*_update_base_url=http://<host-lan-ip>:8000` |

The local loopback address is per-machine. `http://127.0.0.1:8000` only works
when the HTTP server runs inside the same VM as DRHL.

Before testing DRHL itself, verify connectivity from the guest:

```powershell
curl.exe http://192.168.100.1:8000/latest.json
```

```bash
curl -f http://10.0.2.2:8000/latest.json
```

Once real releases exist, `build_current` can often stay disabled. In that
case, install the existing release and launch it with a runtime endpoint
override:

```bash
DRHL_UPDATE_ENDPOINT="http://192.168.1.42:8000/latest.json" \
~/Applications/DRH-Launcher.AppImage
```

On Windows, run the installed launcher from PowerShell with:

```powershell
$env:DRHL_UPDATE_ENDPOINT = "http://192.168.1.42:8000/latest.json"
& "$env:LOCALAPPDATA\DRH Launcher\DRH Launcher.exe"
```

On macOS, launch the app executable directly so the environment variable is
inherited:

```bash
DRHL_UPDATE_ENDPOINT="http://192.168.1.42:8000/latest.json" \
/Applications/DRH\ Launcher.app/Contents/MacOS/DRH-Launcher
```

The runtime override is only a test hook. Update packages are still verified
with the public key embedded in the launcher, so redirecting the endpoint does
not bypass update signing.

## Local Update Testing

Release builds normally embed GitHub's latest-release manifest endpoint:

```text
https://github.com/Tutez64/DRH-Launcher/releases/latest/download/latest.json
```

For local update tests, either compile a release build with
`DRHL_UPDATE_ENDPOINT` pointing to a locally served manifest or launch an
existing build with `DRHL_UPDATE_ENDPOINT` set in the runtime environment:

```bash
DRHL_UPDATE_PUBLIC_KEY="$(cat drhl-update.key.pub)" \
DRHL_UPDATE_ENDPOINT="http://127.0.0.1:8000/latest.json" \
cargo build --release --locked
```

The compile-time endpoint is useful for packages that should work through normal
double-click launching. The runtime endpoint is useful for redirecting an
already installed release during a one-off update test. The release workflow
does not set the compile-time `DRHL_UPDATE_ENDPOINT`, so official packages keep
using GitHub `Latest` unless the runtime variable is present.

On Linux, the test build still needs to run as a managed AppImage before
automatic launcher updates are enabled. This mirrors production behavior and
prevents test updates from replacing an arbitrary AppImage in `Downloads`.

### Local Linux AppImage Update Test

This test uses two locally built AppImages:

- the currently installed AppImage, compiled with `DRHL_UPDATE_ENDPOINT` or
  launched with the runtime variable
- a newer signed AppImage, served from a local HTTP server

Generate an update key once if needed:

```bash
cargo packager signer generate --path drhl-update.key
```

Configure the shell used for both builds:

```bash
export DRHL_UPDATE_PUBLIC_KEY="$(cat drhl-update.key.pub)"
export CARGO_PACKAGER_SIGN_PRIVATE_KEY="$(cat drhl-update.key)"
export CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD='replace-with-key-password'
export DRHL_UPDATE_ENDPOINT="http://127.0.0.1:8000/latest.json"
```

Build the current version and install it from the generated AppImage:

```bash
rm -rf /tmp/drhl-current /tmp/drhl-server
mkdir -p /tmp/drhl-current /tmp/drhl-server

cargo build --release --locked
cargo packager --release --formats appimage --out-dir /tmp/drhl-current
```

Launch the AppImage from `/tmp/drhl-current`, confirm `Install DRH Launcher`,
then close the restarted launcher.

Create a temporary newer version by editing `package.version` in `Cargo.toml`
to a SemVer-compatible pre-release such as:

```toml
version = "0.1.1-test"
```

Build and sign the newer AppImage:

```bash
cargo build --release
cargo packager --release --formats appimage --out-dir /tmp/drhl-server

artifact="$(find /tmp/drhl-server -maxdepth 1 -name '*.AppImage' -print -quit)"
cargo packager signer sign "$artifact"
```

Create a local update manifest:

```bash
python3 - <<'PY'
import json
from datetime import datetime, timezone
from pathlib import Path

version = "0.1.1-test"
root = Path("/tmp/drhl-server")
artifact = next(root.glob("*.AppImage"))
signature = artifact.with_name(f"{artifact.name}.sig").read_text(encoding="utf-8").strip()

manifest = {
    "version": version,
    "pub_date": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
    "platforms": {
        "linux-x86_64": {
            "url": f"http://127.0.0.1:8000/{artifact.name}",
            "signature": signature,
            "format": "appimage",
        }
    },
}

(root / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
PY
```

Serve the update directory in a separate terminal:

```bash
python3 -m http.server 8000 --directory /tmp/drhl-server
```

Restart the installed launcher:

```bash
~/Applications/DRH-Launcher.AppImage
```

The launcher should detect the newer version, show the update banner, download
the local AppImage, verify its signature, replace the managed AppImage, and
restart.

After the test, restore `Cargo.toml` and `Cargo.lock` if they were changed for
the temporary version.

## Platform Trust

The updater signature verifies that a package was produced with the DRHL update
key. It does not provide operating-system publisher reputation.

- Windows Authenticode is needed to reduce SmartScreen warnings.
- Apple Developer ID signing and notarization are needed for normal Gatekeeper
  distribution.
- Until those credentials exist, release notes must state the expected first-run
  warnings and the project must not imply that the packages are OS-verified.
