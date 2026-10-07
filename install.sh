#!/bin/sh
# Installs Shorui on macOS (Apple Silicon or Intel) or Linux (x86_64 or arm64):
#
#   curl -fsSL https://raw.githubusercontent.com/voidwright07/shorui/main/install.sh | sh
#
# Options go after `sh -s --`:
#   --version v0.2.0   install that release instead of the latest
#   --uninstall        remove Shorui (your settings are kept)
#
# Nothing needs administrator rights. On macOS the app goes to /Applications (or
# ~/Applications when that is not writable). On Linux it goes to ~/.local, with a menu entry.
set -eu

repo="voidwright07/shorui"
version="latest"
uninstall=0

say() { printf '%s\n' "$*"; }
fail() { printf 'Shorui install: %s\n' "$*" >&2; exit 1; }
usage() {
    say "Installs Shorui on macOS or Linux."
    say "  curl -fsSL https://raw.githubusercontent.com/$repo/main/install.sh | sh"
    say "  ... | sh -s -- --version v0.2.0    install that release instead of the latest"
    say "  ... | sh -s -- --uninstall         remove Shorui (settings are kept)"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || fail "--version needs a value, such as v0.1.0"; version="$2"; shift 2 ;;
        --version=*) version="${1#--version=}"; shift ;;
        --uninstall) uninstall=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) fail "unknown option: $1" ;;
    esac
done

if [ "$version" = "latest" ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    case "$version" in v*) ;; *) version="v$version" ;; esac
    base="https://github.com/$repo/releases/download/$version"
fi

download() {
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif command -v wget > /dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        fail "needs curl or wget to download."
    fi
}

# Check the download against the release's SHA256SUMS.txt.
verify() {
    file="$1"
    name=$(basename "$file")
    if ! download "$base/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt" 2> /dev/null; then
        say "  (no checksum list in this release; skipping the check)"
        return
    fi
    want=$(grep " \*\{0,1\}$name\$" "$tmp/SHA256SUMS.txt" | awk '{print $1}')
    [ -n "$want" ] || fail "$name is not in the release's checksum list."
    if command -v sha256sum > /dev/null 2>&1; then
        got=$(sha256sum "$file" | awk '{print $1}')
    else
        got=$(shasum -a 256 "$file" | awk '{print $1}')
    fi
    [ "$want" = "$got" ] || fail "the download of $name is damaged (checksum mismatch). Please try again."
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

# ------------------------------------------------------------------ macOS
macos() {
    if [ "$uninstall" = 1 ]; then
        rm -rf "/Applications/Shorui.app" "$HOME/Applications/Shorui.app" "$HOME/.local/bin/shorui-cli"
        say "Shorui is removed. Its settings stay in ~/Library/Application Support/Shorui."
        return
    fi
    asset=shorui-macos-universal.tar.gz
    say "Downloading Shorui ($version) for macOS..."
    download "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
    verify "$tmp/$asset"
    tar -xzf "$tmp/$asset" -C "$tmp"

    if [ -w /Applications ]; then dest=/Applications; else dest="$HOME/Applications"; mkdir -p "$dest"; fi
    rm -rf "$dest/Shorui.app"
    mv "$tmp/Shorui.app" "$dest/"
    # Downloaded this way the app carries no quarantine flag; clear it in case one was added.
    xattr -dr com.apple.quarantine "$dest/Shorui.app" 2> /dev/null || true

    mkdir -p "$HOME/.local/bin"
    ln -sf "$dest/Shorui.app/Contents/MacOS/shorui-cli" "$HOME/.local/bin/shorui-cli"

    say ""
    say "Shorui is installed in $dest."
    say "Open it from Launchpad or Spotlight, or run:  open -a Shorui"
}

# ------------------------------------------------------------------ Linux
linux() {
    prefix="$HOME/.local"
    data="${XDG_DATA_HOME:-$prefix/share}"
    if [ "$uninstall" = 1 ]; then
        rm -rf "$prefix/lib/shorui"
        rm -f "$prefix/bin/shorui" "$prefix/bin/shorui-cli" "$data/applications/shorui.desktop" "$data/icons/hicolor/256x256/apps/shorui.png"
        command -v update-desktop-database > /dev/null 2>&1 && update-desktop-database "$data/applications" 2> /dev/null || true
        say "Shorui is removed. Its settings stay in ${XDG_CONFIG_HOME:-$HOME/.config}/Shorui."
        return
    fi
    case "$(uname -m)" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) fail "there is no Linux build for $(uname -m) yet." ;;
    esac
    asset="shorui-linux-$arch.tar.gz"
    say "Downloading Shorui ($version) for Linux $arch..."
    download "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
    verify "$tmp/$asset"
    tar -xzf "$tmp/$asset" -C "$tmp"

    app="$prefix/lib/shorui"
    mkdir -p "$app" "$prefix/bin" "$data/applications" "$data/icons/hicolor/256x256/apps"
    for f in shorui shorui-cli OFL.txt; do
        cp "$tmp/shorui/$f" "$app/$f"
    done
    chmod 755 "$app/shorui" "$app/shorui-cli"
    ln -sf "$app/shorui" "$prefix/bin/shorui"
    ln -sf "$app/shorui-cli" "$prefix/bin/shorui-cli"
    sed "s|^Exec=.*|Exec=$app/shorui %F|" "$tmp/shorui/shorui.desktop" > "$data/applications/shorui.desktop"
    cp "$tmp/shorui/shorui.png" "$data/icons/hicolor/256x256/apps/shorui.png"
    command -v update-desktop-database > /dev/null 2>&1 && update-desktop-database "$data/applications" 2> /dev/null || true
    command -v gtk-update-icon-cache > /dev/null 2>&1 && gtk-update-icon-cache -q "$data/icons/hicolor" 2> /dev/null || true

    say ""
    say "Shorui is installed. Open it from your applications menu, or run:  shorui"
    case ":$PATH:" in
        *":$prefix/bin:"*) ;;
        *) say "To run it from a terminal, add ~/.local/bin to your PATH." ;;
    esac

    # The window needs Vulkan and a few desktop libraries. Most desktops have them.
    ldconfig_bin=$(command -v ldconfig 2> /dev/null || echo /sbin/ldconfig)
    if [ -x "$ldconfig_bin" ]; then
        missing=""
        for lib in libvulkan.so.1 libxkbcommon.so.0 libxkbcommon-x11.so.0 libwayland-client.so.0 libfontconfig.so.1 libfreetype.so.6; do
            "$ldconfig_bin" -p 2> /dev/null | grep -q "$lib" || missing="$missing $lib"
        done
        if [ -n "$missing" ]; then
            say ""
            say "Shorui needs some libraries this system does not have:$missing"
            if command -v apt-get > /dev/null 2>&1; then
                say "  sudo apt-get install libvulkan1 mesa-vulkan-drivers libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libfontconfig1 libfreetype6"
            elif command -v dnf > /dev/null 2>&1; then
                say "  sudo dnf install vulkan-loader mesa-vulkan-drivers libxkbcommon libxkbcommon-x11 libwayland-client fontconfig freetype"
            elif command -v pacman > /dev/null 2>&1; then
                say "  sudo pacman -S vulkan-icd-loader libxkbcommon libxkbcommon-x11 wayland fontconfig freetype2  (plus the Vulkan driver for your GPU)"
            elif command -v zypper > /dev/null 2>&1; then
                say "  sudo zypper install libvulkan1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 fontconfig libfreetype6"
            fi
        fi
    fi
}

case "$(uname -s)" in
    Darwin) macos ;;
    Linux) linux ;;
    *) fail "this script is for macOS and Linux. On Windows, run in PowerShell:  irm https://raw.githubusercontent.com/$repo/main/install.ps1 | iex" ;;
esac
