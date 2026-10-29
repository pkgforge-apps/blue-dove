#!/bin/sh
# Packages a release build into dist/.
#
# Usage: scripts/package.sh <rust-target> <platform> <version>
#   scripts/package.sh x86_64-unknown-linux-gnu x86_64-linux 0.1.0
#
# Produces, in dist/:
#   Linux    <app>-<platform>.tar.xz  binary, desktop entry and icons
#   macOS    <app>-<platform>.dmg     the .app and an Applications link
#   Windows  <app>-<platform>.exe     the binary itself
# Build the release binary for <rust-target> first.
set -eu
cd "$(dirname "$0")/.."

target=$1
platform=$2
version=$3
app=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
title=$(sed -n 's/^Name=//p' packaging/linux/app.desktop | head -n 1)
bin=target/$target/release/$app
name=$app-$platform
stage=dist/$name

rm -rf "$stage"
mkdir -p "$stage"

case $target in
*-linux-*)
    cp LICENSE README.md "$bin" "$stage/"
    mkdir -p "$stage/share/applications"
    cp packaging/linux/app.desktop "$stage/share/applications/$app.desktop"
    # A vector icon ships as one SVG; pixel art ships as the rasters only,
    # since an SVG of it would just be the same pixels scaled up.
    if [ -f assets/icon.svg ]; then
        mkdir -p "$stage/share/icons/hicolor/scalable/apps"
        cp assets/icon.svg "$stage/share/icons/hicolor/scalable/apps/$app.svg"
    fi
    for size in 16 24 32 48 64 128 256 512; do
        dir=$stage/share/icons/hicolor/${size}x$size/apps
        mkdir -p "$dir"
        cp "assets/icons/$size.png" "$dir/$app.png"
    done
    tar -C dist -cJf "$stage.tar.xz" "$name"
    ;;
*-apple-darwin)
    contents=$stage/$title.app/Contents
    mkdir -p "$contents/MacOS" "$contents/Resources"
    cp "$bin" "$contents/MacOS/$app"
    sed "s/__VERSION__/$version/g" packaging/macos/Info.plist > "$contents/Info.plist"
    plutil -lint "$contents/Info.plist"
    iconset=dist/icon.iconset
    rm -rf "$iconset"
    mkdir -p "$iconset"
    for size in 16 32 128 256 512; do
        cp "assets/icons/$size.png" "$iconset/icon_${size}x$size.png"
        cp "assets/icons/$((size * 2)).png" "$iconset/icon_${size}x$size@2x.png"
    done
    iconutil -c icns "$iconset" -o "$contents/Resources/icon.icns"
    rm -rf "$iconset"
    ln -s /Applications "$stage/Applications"
    # hdiutil on CI runners now and then fails with "Resource busy".
    attempt=1
    until hdiutil create -volname "$title" -srcfolder "$stage" -ov \
        -format UDZO "dist/$name.dmg"; do
        if [ "$attempt" -ge 3 ]; then
            exit 1
        fi
        attempt=$((attempt + 1))
        sleep 5
    done
    ;;
*-windows-*)
    cp "$bin.exe" "dist/$name.exe"
    ;;
*)
    echo "unsupported target: $target" >&2
    exit 1
    ;;
esac

rm -rf "$stage"
ls dist/"$name".*
