#!/bin/sh
# Packs the native release build into a portable onelf bundle in dist/.
#
# Usage: scripts/package-onelf.sh <platform> <version>
#   scripts/package-onelf.sh x86_64-linux 0.1.0
#
# Produces dist/<app>-<platform>.onelf. Run `cargo build --release
# --locked` first. The bundle carries glibc, Mesa and every library the
# binary loads, all from the build host.
set -eu
cd "$(dirname "$0")/.."

platform=$1
version=$2
app=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
appdir=dist/AppDir

rm -rf "$appdir"
mkdir -p "$appdir/bin" "$appdir/.onelf/icons" "$appdir/.onelf/desktop"
cp "target/release/$app" "$appdir/bin/$app"
cp packaging/linux/app.desktop "$appdir/.onelf/desktop/default.desktop"

# onelf wants an SVG for the bundle's own icon, so a pixel-art app gets one
# that carries the largest raster. A vector app copies its SVG straight in.
if [ -f assets/icon.svg ]; then
    cp assets/icon.svg "$appdir/.onelf/icons/default.svg"
    mkdir -p "$appdir/share/icons/hicolor/scalable/apps"
    cp assets/icon.svg "$appdir/share/icons/hicolor/scalable/apps/$app.svg"
else
    {
        printf '<svg xmlns="http://www.w3.org/2000/svg" '
        printf 'xmlns:xlink="http://www.w3.org/1999/xlink" '
        printf 'viewBox="0 0 1024 1024" width="1024" height="1024">\n'
        printf '  <image width="1024" height="1024" '
        printf 'xlink:href="data:image/png;base64,'
        base64 -w 0 assets/icons/1024.png
        printf '"/>\n</svg>\n'
    } > "$appdir/.onelf/icons/default.svg"
fi
for size in 16 24 32 48 64 128 256 512; do
    dir=$appdir/share/icons/hicolor/${size}x$size/apps
    mkdir -p "$dir"
    cp "assets/icons/$size.png" "$dir/$app.png"
done
sed "s/^version = .*/version = \"$version\"/" onelf.toml > "$appdir/onelf.toml"

onelf build "$appdir"
onelf verify "$appdir/$app.onelf"
mv "$appdir/$app.onelf" "dist/$app-$platform.onelf"
rm -rf "$appdir"
ls "dist/$app-$platform.onelf"
