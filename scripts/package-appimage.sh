#!/bin/sh
# Packs the native release build into an Anylinux AppImage in dist/.
#
# Usage: scripts/package-appimage.sh <platform> <version>
#   scripts/package-appimage.sh x86_64-linux 0.1.0
#
# Produces dist/<app>-<platform>.AppImage and, when GITHUB_REPOSITORY is
# set, the .zsync its self-updater reads. Runs as root on Arch with the
# Anylinux tools and the debloated packages installed, after `cargo build
# --release --locked`; the release workflow sets that up. The image
# carries glibc, the loader, Mesa and every library the binary loads, all
# from the build host.
set -eu
cd "$(dirname "$0")/.."

platform=$1
version=$2
app=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)

# quick-sharun traces the binary where it is installed.
install -Dm755 "target/release/$app" "/usr/bin/$app"

export APPDIR="$PWD/dist/AppDir"
export OUTPATH="$PWD/dist"
export OUTNAME="$app-$platform.AppImage"
export VERSION="$version"
export DESKTOP="$PWD/packaging/linux/app.desktop"
export ICON="$PWD/assets/icons/256.png"

# Detection alone bundles GLX and gallium but no libEGL, no dri/ and no
# gbm, which leaves no Wayland path and no driver behind the GL it does
# carry.
export DEPLOY_OPENGL=1

if [ -n "${GITHUB_REPOSITORY:-}" ]; then
    export UPINFO="gh-releases-zsync|${GITHUB_REPOSITORY%/*}|${GITHUB_REPOSITORY#*/}|latest|*$platform.AppImage.zsync"
    export ADD_HOOKS=self-updater.hook
fi

rm -rf "$APPDIR"
quick-sharun "/usr/bin/$app"

# A host with Mesa makes up for a bundle without it, so no test run would
# catch these missing.
if [ -z "$(find "$APPDIR" -name libEGL.so.1)" ] ||
    [ -z "$(find "$APPDIR" -type d -name dri)" ]; then
    echo "the AppDir has no libEGL.so.1 or no dri/" >&2
    exit 1
fi

quick-sharun --make-appimage
rm -rf "$APPDIR" dist/appinfo
ls "dist/$OUTNAME"
