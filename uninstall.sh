#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Removes what install.sh installed.
#
#   ./uninstall.sh                # keeps your channels and settings
#   ./uninstall.sh --purge        # also deletes ~/.local/share/tvmaxe
#   PREFIX=/usr ./uninstall.sh    # if it was installed with another prefix
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
PREFIX="${PREFIX%/}"
PURGE=0

for arg in "$@"; do
    case "$arg" in
    --purge) PURGE=1 ;;
    -h | --help)
        sed -n '4,8p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        echo "Unknown option: $arg (use --purge or --help)" >&2
        exit 2
        ;;
    esac
done

if [[ ${EUID} -eq 0 ]]; then
    echo "Run this script as your normal desktop user, not as root." >&2
    echo "It asks for sudo only to remove the files from $PREFIX." >&2
    exit 1
fi

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/tvmaxe"
ICON_DIR="$PREFIX/share/icons/hicolor"
FILES=(
    "$PREFIX/bin/tvmaxe"
    "$PREFIX/share/applications/TvMaxe.desktop"
    "$ICON_DIR/scalable/apps/TvMaxe.svg"
    # Older manual installs.
    "$PREFIX/share/applications/io.condor2.TvMaxe.desktop"
    "$ICON_DIR/scalable/apps/io.condor2.TvMaxe.svg"
)

found=()
for f in "${FILES[@]}"; do
    [[ -e "$f" ]] && found+=("$f")
done

if [[ ${#found[@]} -eq 0 ]]; then
    echo "TV-Maxe is not installed in $PREFIX."
else
    if pgrep -x tvmaxe >/dev/null 2>&1; then
        echo "Note: TV-Maxe is running; it keeps working until you close it."
    fi
    echo "Removing:"
    printf '  %s\n' "${found[@]}"
    sudo rm -f "${found[@]}"

    if command -v update-desktop-database >/dev/null 2>&1; then
        sudo update-desktop-database -q "$PREFIX/share/applications" || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ -f "$ICON_DIR/index.theme" ]]; then
        sudo gtk-update-icon-cache -q -t -f "$ICON_DIR" || true
    fi
fi

if [[ $PURGE -eq 1 ]]; then
    if [[ -d "$DATA_DIR" ]]; then
        echo "Removing your channels and settings: $DATA_DIR"
        rm -rf "$DATA_DIR"
    fi
elif [[ -d "$DATA_DIR" ]]; then
    echo
    echo "Your channels and settings were kept in $DATA_DIR"
    echo "(run ./uninstall.sh --purge to delete them too)."
fi

echo
echo "TV-Maxe was removed."
