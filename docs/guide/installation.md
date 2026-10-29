# Installation

Pick the file for your system. Every link points at the latest release.

<Downloads />

## Linux

The `.onelf` file is a single executable that carries its own libraries,
graphics drivers included, so it runs on any distribution, musl ones
like Alpine too:

```sh
chmod +x blue-dove-x86_64-linux.onelf
./blue-dove-x86_64-linux.onelf
```

Add it to your application menu with:

```sh
./blue-dove-x86_64-linux.onelf --onelf-integrate
```

The `.AppImage` is a single file too, with DwarFS compression and no
FUSE requirement: it falls back to namespaces, then to extracting
itself. Make it executable and run it:

```sh
chmod +x blue-dove-anylinux-x86_64.AppImage
./blue-dove-anylinux-x86_64.AppImage
```

Its self-updater is off when an AppImage manager such as `am` or `soar`
manages the file, which is how those managers want it.

The `.tar.xz` archive holds the plain binary with its desktop entry and
icons under `share/`. It uses the display and audio libraries your
system already has.

## macOS

Open the disk image and drag `Blue Dove` to the `Applications` link beside
it. The app is not notarized, so the first launch needs a right click and
**Open**.

## Windows

Run `blue-dove.exe`. It carries the C runtime, so nothing else has to be
installed.

## From source

```sh
cargo install --locked --git https://github.com/pkgforge-apps/blue-dove
```

Building needs the ALSA development headers, `alsa-lib` on Arch and
`libasound2-dev` on Debian and Ubuntu, because the audio backend links
them.
