---
name: app-icon
description: Design a pkgforge-apps app icon as a hand-written SVG and regenerate every raster, the Windows .ico and the docs favicons from it. Use when creating a new app's icon, changing it, or when the icons, favicon or .ico look wrong or stale.
---

# App icon

`assets/icon.svg` is the only source, or `assets/icon.png` when the icon is
pixel art. Everything else is generated. The script takes whichever of the
two exists, so an app switches by adding the PNG and deleting the SVG.

## Design rules

- Canvas `viewBox="0 0 1024 1024"`. The tile is a rounded square at
  `x=80 y=80 width=864 height=864 rx=196`, leaving the margin macOS
  expects and keeping the shape consistent across the organization.
- One idea, one glyph. The icon must read at 16 px: a bold silhouette,
  at most three or four shapes inside the tile, no text, no fine lines
  thinner than about 40 units.
- Each app gets its own palette. Use a two-stop linear gradient on the
  tile and one light glyph on top, or the inverse. Make sure the glyph
  has strong contrast against the tile in both light and dark
  surroundings.
- A subtle top sheen and a soft drop shadow under the glyph are the
  house style. Keep them subtle.
- Plain SVG only: paths, rects, circles, linear gradients. No filters,
  masks, embedded images or fonts, so every renderer draws it the same.
- A pixel-art icon is a square PNG instead, with the art on a square grid
  and a transparent margin. The script scales it nearest-neighbour to 16,
  24, 32, 48, 64, 128, 256, 512 and 1024 px: a size that is a whole
  multiple of the grid keeps every pixel square, any other comes out with
  uneven pixels. A 16 px grid is exact at every size but 24; a 32 px grid
  is also uneven at 48, and at 16 and 24 it loses pixels, which thins
  its lines. Leave the margin fully
  transparent, since a faint halo left over from scaling the art up shows
  as a white smudge on a dark desktop.
- ASCII only, like every file in the repository.
- When the full icon is too detailed for a browser tab, add a simplified
  `assets/favicon.svg`; the script prefers it for the favicon.

## Check it

Render previews outside the repository and look at them, at least at
512 and 32 px:

```sh
mkdir -p /tmp/icon-preview
resvg -w 512 assets/icon.svg /tmp/icon-preview/512.png
resvg -w 32 assets/icon.svg /tmp/icon-preview/32.png
```

Iterate until the 32 px render is still recognizable. For a pixel-art
icon, preview with `magick assets/icon.png -filter point -resize 32x32
/tmp/icon-preview/32.png` instead.

## Generate

```sh
scripts/icons.sh
```

Needs `resvg` (`cargo install resvg`). It writes:

- `assets/icons/{16..1024}.png`: window icon, hicolor theme icons for
  Linux, and the sources the release workflow turns into the macOS icns.
- `assets/icon.ico`: embedded into the Windows executable by `build.rs`.
- `docs/public/favicon.svg`, `logo.svg`, `apple-touch-icon.png`.

Commit the SVG and every generated file together. Then retune the docs
palette in `docs/.vitepress/theme/style.css` and the `accent` in
`docs/.vitepress/app.ts` to match the new icon.
