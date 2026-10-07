#!/bin/sh
# Builds Shorui for Linux on this machine's architecture and packs it as
#   dist/shorui-linux-<x86_64|aarch64>.tar.gz   (what install.sh downloads)
#
# Run from anywhere in the repository:  sh installer/linux/package.sh
# Build packages needed on Debian or Ubuntu:
#   sudo apt-get install build-essential pkg-config libfontconfig-dev libfreetype-dev \
#     libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-dev libx11-xcb-dev \
#     libxcb1-dev libvulkan-dev
set -eu
cd "$(dirname "$0")/../.."

case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) echo "No Linux package is made for $(uname -m)." >&2; exit 1 ;;
esac

cargo build --release --locked -p shorui
cargo build --release --locked -p shorui-core --bin shorui-cli

rm -rf dist
stage=dist/shorui
mkdir -p "$stage"
cp target/release/shorui target/release/shorui-cli "$stage/"
cp installer/linux/shorui.desktop "$stage/"
cp crates/shorui/assets/icon.png "$stage/shorui.png"
cp crates/shorui/assets/fonts/OFL.txt "$stage/"
tar -C dist -czf "dist/shorui-linux-$arch.tar.gz" shorui
rm -rf "$stage"
echo "Wrote dist/shorui-linux-$arch.tar.gz"
