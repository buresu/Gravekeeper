#!/bin/sh
# Installs (or with --uninstall removes) Gravekeeper for the current user, without packages:
#   binary       ~/.local/bin/gravekeeper
#   menu entry   ~/.local/share/applications/gravekeeper.desktop
#   icons        ~/.local/share/icons/hicolor/<size>/apps/gravekeeper.png
#
# usage: packaging/linux/install.sh [path/to/gravekeeper] [--uninstall]
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
sizes="16 32 48 64 128 256 512"

if [ "${1:-}" = "--uninstall" ] || [ "${2:-}" = "--uninstall" ]; then
    rm -f "$bin_dir/gravekeeper" "$data_dir/applications/gravekeeper.desktop"
    for s in $sizes; do rm -f "$data_dir/icons/hicolor/${s}x${s}/apps/gravekeeper.png"; done
    echo "Removed Gravekeeper. Your configuration and data were kept."
    exit 0
fi

binary="${1:-$here/../../target/release/gravekeeper}"
[ -x "$binary" ] || { echo "no executable at $binary (build with: cargo build --release)" >&2; exit 1; }

install -Dm755 "$binary" "$bin_dir/gravekeeper"
for s in $sizes; do
    install -Dm644 "$here/../icons/${s}x${s}.png" "$data_dir/icons/hicolor/${s}x${s}/apps/gravekeeper.png"
done
mkdir -p "$data_dir/applications"
cat > "$data_dir/applications/gravekeeper.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Gravekeeper
Comment=Supervise and operate applications
Exec=$bin_dir/gravekeeper
Icon=gravekeeper
Categories=Utility;
Terminal=false
StartupWMClass=gravekeeper
Keywords=supervisor;process;monitor;restart;
DESKTOP
command -v update-desktop-database >/dev/null && update-desktop-database "$data_dir/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$data_dir/icons/hicolor" 2>/dev/null || true
echo "Installed Gravekeeper to $bin_dir/gravekeeper."
