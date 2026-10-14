#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds TV-Maxe and installs it system-wide (default prefix: /usr/local).
#
#   ./install.sh                  # /usr/local/bin/tvmaxe + menu entry + icon
#   PREFIX=/usr ./install.sh      # another prefix
#
# Run it as your normal user: it asks for sudo only to copy the files.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-/usr/local}"
PREFIX="${PREFIX%/}"
MIN_RUST="1.88"
MIN_GTK="4.14"

BIN="$PREFIX/bin/tvmaxe"
DESKTOP="$PREFIX/share/applications/TvMaxe.desktop"
ICON_DIR="$PREFIX/share/icons/hicolor"
ICON="$ICON_DIR/scalable/apps/TvMaxe.svg"

if [[ ${EUID} -eq 0 ]]; then
    echo "Run this script as your normal desktop user, not as root." >&2
    echo "It asks for sudo only to copy the files into $PREFIX." >&2
    exit 1
fi

if ! command -v sudo >/dev/null 2>&1; then
    echo "sudo is required to install into $PREFIX." >&2
    exit 1
fi

hint_packages() {
    echo >&2
    if command -v pacman >/dev/null 2>&1; then
        echo "On Arch Linux install the build/runtime packages with:" >&2
        echo "  sudo pacman -S --needed base-devel rust gtk4 mpv" >&2
    elif command -v apt-get >/dev/null 2>&1; then
        echo "On Debian/Ubuntu install the build/runtime packages with:" >&2
        echo "  sudo apt install build-essential pkg-config libgtk-4-dev libmpv-dev" >&2
        echo "and a recent Rust toolchain from https://rustup.rs (the distribution's may be too old)." >&2
    elif command -v dnf >/dev/null 2>&1; then
        echo "On Fedora install the build/runtime packages with:" >&2
        echo "  sudo dnf install gcc pkgconf-pkg-config gtk4-devel mpv-libs-devel rust cargo" >&2
    fi
}

missing=0
for cmd in cargo rustc pkg-config cc; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Missing required command: $cmd" >&2
        missing=1
    fi
done

if command -v rustc >/dev/null 2>&1; then
    RUST_VERSION="$(rustc --version | awk '{print $2}')"
    if [[ "$(printf '%s\n%s\n' "$MIN_RUST" "$RUST_VERSION" | sort -V | head -n1)" != "$MIN_RUST" ]]; then
        echo "Rust $MIN_RUST or newer is required (found $RUST_VERSION)." >&2
        echo "Update it with 'rustup update' or your distribution's rust package." >&2
        missing=1
    else
        echo "Detected Rust: $RUST_VERSION"
    fi
fi

if command -v pkg-config >/dev/null 2>&1; then
    if pkg-config --exists "gtk4 >= $MIN_GTK"; then
        echo "Detected GTK: $(pkg-config --modversion gtk4)"
    else
        echo "GTK $MIN_GTK+ development files (gtk4.pc) were not found." >&2
        missing=1
    fi
    if pkg-config --exists mpv; then
        echo "Detected libmpv: $(pkg-config --modversion mpv)"
    else
        echo "libmpv development files (mpv.pc) were not found." >&2
        missing=1
    fi
fi

if [[ $missing -ne 0 ]]; then
    hint_packages
    exit 1
fi

echo "Building TV-Maxe..."
LOCKED=()
[[ -f "$ROOT_DIR/Cargo.lock" ]] && LOCKED=(--locked)
cargo build --release "${LOCKED[@]}" --manifest-path "$ROOT_DIR/Cargo.toml" -p tvmaxe

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
BUILT_BIN="$TARGET_DIR/release/tvmaxe"
if [[ ! -x "$BUILT_BIN" ]]; then
    echo "Build finished, but the executable was not found at $BUILT_BIN." >&2
    exit 1
fi

# The menu entry points to the installed binary with an absolute path, so it
# starts even when $PREFIX/bin is not in the PATH of the desktop session.
STAGING="$(mktemp -d)"
BIN_BACKUP=""
cleanup() {
    rm -rf "$STAGING"
    [[ -n "$BIN_BACKUP" ]] && rm -f "$BIN_BACKUP"
}
trap cleanup EXIT

sed "s|^Exec=tvmaxe |Exec=$BIN |" "$ROOT_DIR/data/TvMaxe.desktop" >"$STAGING/TvMaxe.desktop"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$STAGING/TvMaxe.desktop"
fi

# Keep the previous binary so a failed install can be rolled back.
if [[ -e "$BIN" ]]; then
    BIN_BACKUP="$(mktemp)"
    cp -p "$BIN" "$BIN_BACKUP"
fi

rollback() {
    echo "Installation failed; rolling back..." >&2
    if [[ -n "$BIN_BACKUP" && -f "$BIN_BACKUP" ]]; then
        sudo install -Dm755 "$BIN_BACKUP" "$BIN"
    else
        sudo rm -f "$BIN"
    fi
    sudo rm -f "$DESKTOP" "$ICON"
}

echo "Installing into $PREFIX..."
if ! {
    sudo install -Dm755 "$BUILT_BIN" "$BIN" &&
        sudo install -Dm644 "$STAGING/TvMaxe.desktop" "$DESKTOP" &&
        sudo install -Dm644 "$ROOT_DIR/data/icons/hicolor/scalable/apps/TvMaxe.svg" "$ICON"
}; then
    rollback
    exit 1
fi

# Files from earlier manual installs under the old reverse-DNS name.
for old in \
    "$PREFIX/share/applications/io.condor2.TvMaxe.desktop" \
    "$ICON_DIR/scalable/apps/io.condor2.TvMaxe.svg"; do
    [[ -e "$old" ]] && sudo rm -f "$old"
done
for old in \
    "${XDG_DATA_HOME:-$HOME/.local/share}/applications/io.condor2.TvMaxe.desktop" \
    "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/io.condor2.TvMaxe.svg"; do
    rm -f "$old"
done

# Menu and icon caches (optional tools; the menu also refreshes on its own).
if command -v update-desktop-database >/dev/null 2>&1; then
    sudo update-desktop-database -q "$PREFIX/share/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ -f "$ICON_DIR/index.theme" ]]; then
    sudo gtk-update-icon-cache -q -t -f "$ICON_DIR" || true
fi

if [[ -n "${XDG_DATA_DIRS:-}" && ":$XDG_DATA_DIRS:" != *":$PREFIX/share:"* && ":$XDG_DATA_DIRS:" != *":$PREFIX/share/:"* ]]; then
    echo
    echo "Warning: $PREFIX/share is not in XDG_DATA_DIRS, so the menu may not show TV-Maxe." >&2
    echo "         Log out and back in, or install with PREFIX=/usr." >&2
fi

echo
echo "Installed successfully:"
echo "  $BIN"
echo "  $DESKTOP"
echo "  $ICON"
echo
echo "Start it from the application menu (Multimedia → TV-Maxe) or with:"
echo "  tvmaxe"
echo
echo "The interface follows the system language (English, or Romanian on a Romanian system)."
echo "Your channels and settings live in ${XDG_DATA_HOME:-$HOME/.local/share}/tvmaxe/."
