#!/bin/sh
# Build and install Purpamail.
#   ./install.sh              installs into ~/.local
#   PREFIX=/usr/local ./install.sh   (as root) installs system-wide
set -e
PREFIX="${PREFIX:-$HOME/.local}"
cargo build --release
install -Dm755 target/release/purpamail "$PREFIX/bin/purpamail"
install -Dm644 data/org.purpamail.Purpamail.desktop "$PREFIX/share/applications/org.purpamail.Purpamail.desktop"
install -Dm644 data/org.purpamail.Purpamail.svg "$PREFIX/share/icons/hicolor/scalable/apps/org.purpamail.Purpamail.svg"
install -Dm644 data/org.purpamail.Purpamail.metainfo.xml "$PREFIX/share/metainfo/org.purpamail.Purpamail.metainfo.xml"
command -v update-desktop-database >/dev/null && update-desktop-database "$PREFIX/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$PREFIX/share/icons/hicolor" || true
echo "Purpamail installed to $PREFIX/bin/purpamail"
echo "To make it your default mail app: xdg-mime default org.purpamail.Purpamail.desktop x-scheme-handler/mailto"
