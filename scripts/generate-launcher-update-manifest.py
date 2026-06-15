#!/usr/bin/env python3

import argparse
import hashlib
import json
import re
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate cargo-packager-updater metadata for a DRH Launcher release."
    )
    parser.add_argument("--version", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--dist-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--checksums-output", type=Path, required=True)
    return parser.parse_args()


def require_one(dist_dir: Path, pattern: str) -> Path:
    matches = sorted(path for path in dist_dir.glob(pattern) if path.is_file())
    if len(matches) != 1:
        names = ", ".join(path.name for path in matches) or "none"
        raise SystemExit(f"Expected one {pattern} file in {dist_dir}, found: {names}")
    return matches[0]


def signature_for(artifact: Path) -> str:
    signature = artifact.with_name(f"{artifact.name}.sig")
    if not signature.is_file():
        raise SystemExit(f"Missing update signature: {signature}")
    value = signature.read_text(encoding="utf-8").strip()
    if not value:
        raise SystemExit(f"Empty update signature: {signature}")
    return value


def download_url(repository: str, version: str, artifact: Path) -> str:
    encoded_name = quote(artifact.name)
    return f"https://github.com/{repository}/releases/download/v{version}/{encoded_name}"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    args = parse_args()
    version = args.version.removeprefix("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", version):
        raise SystemExit(f"Invalid semantic version: {args.version}")
    if not re.fullmatch(r"[^/]+/[^/]+", args.repository):
        raise SystemExit(f"Invalid GitHub repository: {args.repository}")

    linux = require_one(args.dist_dir, f"DRH-Launcher_{version}_x86_64.AppImage")
    windows = require_one(args.dist_dir, f"DRH-Launcher_{version}_x64-setup.exe")
    macos = require_one(args.dist_dir, "DRH Launcher.app.tar.gz")

    platforms = {
        "linux-x86_64": {
            "url": download_url(args.repository, version, linux),
            "signature": signature_for(linux),
            "format": "appimage",
        },
        "windows-x86_64": {
            "url": download_url(args.repository, version, windows),
            "signature": signature_for(windows),
            "format": "nsis",
        },
        "macos-x86_64": {
            "url": download_url(args.repository, version, macos),
            "signature": signature_for(macos),
            "format": "app",
        },
        "macos-aarch64": {
            "url": download_url(args.repository, version, macos),
            "signature": signature_for(macos),
            "format": "app",
        },
    }
    manifest = {
        "version": version,
        "pub_date": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "platforms": platforms,
    }
    args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    checksum_files = sorted(
        path
        for path in args.dist_dir.iterdir()
        if path.is_file()
        and path != args.output
        and path != args.checksums_output
        and not path.name.endswith(".sig")
    )
    checksums = "".join(f"{sha256_file(path)}  {path.name}\n" for path in checksum_files)
    args.checksums_output.write_text(checksums, encoding="utf-8")


if __name__ == "__main__":
    main()
