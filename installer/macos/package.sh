#!/bin/sh
# Builds Shorui.app for macOS, one app for both Apple Silicon and Intel, and packs it as
#   dist/Shorui-macos-universal.dmg      (drag to Applications)
#   dist/shorui-macos-universal.tar.gz   (what install.sh downloads)
#
# Run on a Mac from anywhere in the repository:  sh installer/macos/package.sh
# Needs Xcode's command line tools (lipo, codesign, iconutil, sips, hdiutil).
set -eu
cd "$(dirname "$0")/../.."

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
version=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "shorui"))')
targets="aarch64-apple-darwin x86_64-apple-darwin"

for target in $targets; do
    rustup target add "$target"
    cargo build --release --locked --target "$target" -p shorui
    cargo build --release --locked --target "$target" -p shorui-core --bin shorui-cli
done

rm -rf dist
app=dist/Shorui.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# One binary holding both architectures.
for bin in shorui shorui-cli; do
    lipo -create -output "$app/Contents/MacOS/$bin" \
        "target/aarch64-apple-darwin/release/$bin" \
        "target/x86_64-apple-darwin/release/$bin"
done
sed "s/@VERSION@/$version/g" installer/macos/Info.plist > "$app/Contents/Info.plist"
cp crates/shorui/assets/fonts/OFL.txt "$app/Contents/Resources/"

# The icon, at every size macOS asks for.
iconset=$(mktemp -d)/Shorui.iconset
mkdir -p "$iconset"
source_icon=crates/shorui/assets/icon-macos.png
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$source_icon" --out "$iconset/icon_${size}x${size}.png" > /dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$source_icon" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Shorui.icns"

# Apple Silicon only runs signed code. Without a Developer ID this is an ad-hoc signature,
# which is enough to run; the install script and the DMG note cover Gatekeeper.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

tar -C dist -czf dist/shorui-macos-universal.tar.gz Shorui.app

# The disk image: the app beside a link to Applications, to drag across.
stage=$(mktemp -d)
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "Shorui $version" -srcfolder "$stage" -ov -format UDZO dist/Shorui-macos-universal.dmg > /dev/null

rm -rf "$app"
lipo -info "$stage/Shorui.app/Contents/MacOS/shorui"
echo "Wrote dist/Shorui-macos-universal.dmg and dist/shorui-macos-universal.tar.gz (Shorui $version)"
