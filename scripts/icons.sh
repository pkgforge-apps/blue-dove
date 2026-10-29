#!/bin/sh
# Regenerates every raster icon from assets/icon.svg, or from assets/icon.png
# when the app's icon is pixel art.
#
# A PNG source is scaled with nearest-neighbour, so a pixel of the art stays a
# square block at every size. An SVG source is rendered at each size instead,
# which is what a vector icon wants.
#
# Outputs:
#   assets/icons/<size>.png           window icon, hicolor theme, icns sources
#   assets/icon.ico                   Windows executable icon
#   docs/public/favicon.svg           assets/favicon.svg when present
#   docs/public/logo.svg              docs nav logo
#   docs/public/apple-touch-icon.png
#
# Needs resvg (cargo install resvg), and ImageMagick for a PNG source.
set -eu
cd "$(dirname "$0")/.."

if [ -f assets/icon.png ]; then
    source_png=assets/icon.png
    if command -v magick >/dev/null 2>&1; then
        magick=magick
    else
        magick=convert
    fi
else
    source_png=
fi
svg=assets/icon.svg

# The source as an SVG, which is what resvg is given for the favicon and the
# touch icon. A pixel-art source is wrapped in one that carries the raster,
# kept outside the repository so a failed run leaves nothing behind.
wrapped=$(mktemp)
trap 'rm -f "$wrapped"' EXIT
if [ -n "$source_png" ]; then
    {
        printf '<svg xmlns="http://www.w3.org/2000/svg" '
        printf 'xmlns:xlink="http://www.w3.org/1999/xlink" '
        printf 'viewBox="0 0 1024 1024" width="1024" height="1024">\n'
        printf '  <image width="1024" height="1024" '
        printf 'xlink:href="data:image/png;base64,'
        base64 -w 0 "$source_png"
        printf '"/>\n</svg>\n'
    } > "$wrapped"
    svg=$wrapped
fi

favicon=assets/favicon.svg
[ -f "$favicon" ] || favicon=$svg

mkdir -p assets/icons docs/public
for size in 16 24 32 48 64 128 256 512 1024; do
    if [ -n "$source_png" ]; then
        "$magick" "$source_png" -filter point -resize "${size}x${size}" \
            "assets/icons/$size.png"
    else
        resvg -w "$size" -h "$size" "$svg" "assets/icons/$size.png"
    fi
done

byte() {
    # shellcheck disable=SC2059 # the format is the octal escape itself
    printf "\\$(printf '%03o' "$(($1 & 255))")"
}

le16() {
    byte "$1"
    byte "$(($1 >> 8))"
}

le32() {
    le16 "$(($1 & 65535))"
    le16 "$(($1 >> 16))"
}

# An ICO is a 6-byte header, one 16-byte entry per image, then the
# images. Vista and later read PNG-encoded entries directly.
ico_sizes="16 24 32 48 64 128 256"
count=0
for size in $ico_sizes; do
    count=$((count + 1))
done
offset=$((6 + 16 * count))
{
    le16 0
    le16 1
    le16 "$count"
    for size in $ico_sizes; do
        length=$(wc -c < "assets/icons/$size.png")
        byte "$size"
        byte "$size"
        byte 0
        byte 0
        le16 1
        le16 32
        le32 "$length"
        le32 "$offset"
        offset=$((offset + length))
    done
    for size in $ico_sizes; do
        cat "assets/icons/$size.png"
    done
} > assets/icon.ico

cp "$favicon" docs/public/favicon.svg
cp "$svg" docs/public/logo.svg
resvg --image-rendering pixelated -w 180 -h 180 "$svg" \
    docs/public/apple-touch-icon.png
echo "icons regenerated from ${source_png:-assets/icon.svg}"
