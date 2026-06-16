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
    parser.add_argument(
        "--repository",
        help="GitHub repository in owner/name form. URLs point to the matching release tag.",
    )
    parser.add_argument(
        "--base-url",
        help="Base URL serving the artifact directory. URLs point to files under this directory.",
    )
    parser.add_argument("--linux-base-url", help="Base URL for Linux updater artifacts.")
    parser.add_argument("--windows-base-url", help="Base URL for Windows updater artifacts.")
    parser.add_argument("--macos-base-url", help="Base URL for macOS updater artifacts.")
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


def github_download_url(repository: str, version: str, artifact: Path) -> str:
    encoded_name = quote(artifact.name)
    return f"https://github.com/{repository}/releases/download/v{version}/{encoded_name}"


def base_url_download_url(base_url: str, artifact: Path) -> str:
    encoded_name = quote(artifact.name)
    return f"{base_url.rstrip('/')}/{encoded_name}"


def platform_base_url(args: argparse.Namespace, platform: str) -> str:
    urls = {
        "linux": args.linux_base_url or args.base_url,
        "windows": args.windows_base_url or args.base_url,
        "macos": args.macos_base_url or args.base_url,
    }
    return urls[platform]


def download_url(args: argparse.Namespace, version: str, artifact: Path, platform: str) -> str:
    if args.repository:
        return github_download_url(args.repository, version, artifact)
    return base_url_download_url(platform_base_url(args, platform), artifact)


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
    if args.repository and not re.fullmatch(r"[^/]+/[^/]+", args.repository):
        raise SystemExit(f"Invalid GitHub repository: {args.repository}")
    platform_specific_urls = [args.linux_base_url, args.windows_base_url, args.macos_base_url]
    if args.repository and (args.base_url or any(platform_specific_urls)):
        raise SystemExit("--repository cannot be combined with artifact base URLs")
    if not args.repository:
        missing_urls = [
            name
            for name, value in (
                ("linux", args.linux_base_url or args.base_url),
                ("windows", args.windows_base_url or args.base_url),
                ("macos", args.macos_base_url or args.base_url),
            )
            if not value
        ]
        if missing_urls:
            raise SystemExit(
                "Missing artifact base URL for platforms: " + ", ".join(missing_urls)
            )
    for url in [args.base_url, *platform_specific_urls]:
        if url and not re.fullmatch(r"https?://.+", url):
            raise SystemExit(f"Invalid artifact base URL: {url}")

    linux = require_one(args.dist_dir, f"DRH-Launcher_{version}_x86_64.AppImage")
    windows = require_one(args.dist_dir, f"DRH-Launcher_{version}_x64-setup.exe")
    macos = require_one(args.dist_dir, "DRH Launcher.app.tar.gz")

    platforms = {
        "linux-x86_64": {
            "url": download_url(args, version, linux, "linux"),
            "signature": signature_for(linux),
            "format": "appimage",
        },
        "windows-x86_64": {
            "url": download_url(args, version, windows, "windows"),
            "signature": signature_for(windows),
            "format": "nsis",
        },
        "macos-x86_64": {
            "url": download_url(args, version, macos, "macos"),
            "signature": signature_for(macos),
            "format": "app",
        },
        "macos-aarch64": {
            "url": download_url(args, version, macos, "macos"),
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
