#!/usr/bin/env sh
set -eu

app_id="io.github.Tutez64.DRHLauncher"
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
desktop_source="$repo_root/packaging/linux/$app_id.desktop"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
desktop_target="$data_home/applications/$app_id.desktop"
exe_path="$repo_root/target/debug/DRH-Launcher"
icon_path="$repo_root/assets/icons/app-icon-1024.png"

if [ ! -x "$exe_path" ]; then
    cargo build --manifest-path "$repo_root/Cargo.toml" --bin DRH-Launcher
fi

mkdir -p "$data_home/applications"
sed \
    -e "s#^Exec=.*#Exec=$exe_path#" \
    -e "s#^Icon=.*#Icon=$icon_path#" \
    "$desktop_source" > "$desktop_target"
chmod 755 "$desktop_target"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_home/applications" >/dev/null 2>&1 || true
fi

if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
elif command -v kbuildsycoca5 >/dev/null 2>&1; then
    kbuildsycoca5 --noincremental >/dev/null 2>&1 || true
fi

printf 'Installed %s desktop entry for local development.\n' "$app_id"
