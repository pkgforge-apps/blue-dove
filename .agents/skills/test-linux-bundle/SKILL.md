---
name: test-linux-bundle
description: Smoke-run a pkgforge-apps onelf bundle on the Linux hosts users have (Wayland, X11, no GL, current Arch, Debian, Ubuntu 22.04's older Mesa, Alpine's musl) before it ships. Use after changing onelf.toml, bumping onelf, adding a dependency that links or opens a system library, adding a feature that reaches new parts of the system, or when a user reports the bundle failing on their distribution.
---

# Test a Linux bundle

A bundle that runs on the machine that built it proves little. Hosts
differ in exactly the ways that break bundles: no GL stack, a musl libc,
an older Mesa, X11 instead of Wayland. The script here launches the
bundle on each and reports per host.

## Build the bundle

```sh
cargo build --release --locked
sh scripts/package-onelf.sh x86_64-linux 0.0.0
```

The release workflow builds it on Arch, which is the reference. A bundle
built on another distribution carries that distribution's libraries and
can behave differently; when the result matters, build in an Arch
container the way the shared `app-release.yml` workflow in
pkgforge-apps/.github does.

## Run the checks

```sh
sh .agents/skills/test-linux-bundle/test-bundle.sh dist/<app>-x86_64-linux.onelf
```

Each check launches the bundle with `--smoke`, so the app must open a
window, draw and exit 0. It needs podman or docker for everything after
the two desktop checks, and pulls each image on first use.

| Check | What it stands for |
|-------|--------------------|
| `host-wayland` | this desktop's Wayland session |
| `host-x11` | the same desktop with `WAYLAND_DISPLAY` unset, so winit takes X11 (Xwayland) |
| `no-gl` | a bare Debian with no GL libraries at all, drawing on this desktop's X server |
| `arch` | current Arch under Xvfb, the release job's own smoke test |
| `debian` | Debian stable under Xvfb with Mesa |
| `ubuntu-22.04` | Mesa 23, whose per-driver `dri/*_dri.so` modules older distributions still ship |
| `alpine` | musl: a glibc bundle can use nothing from the host's GPU stack |

A check that cannot run, such as the desktop ones over SSH or in CI, is
reported as skipped. The script exits non-zero when any check fails and
prints the tail of its log; the full logs stay in the directory it
names at the end.

## Read a failure

- `NoGlutinConfigs` with `kind: NotFound`: no GL library could be
  loaded. The bundle is missing Mesa, or `gl`/`dri` were turned off.
- `NoGlutinConfigs` with `kind: BadConfig`: a GL library loaded but no
  driver behind it worked. Run the bundle with `LD_DEBUG=libs` and look
  for the library that failed to load.
- `Library libxkbcommon-x11.so could not be loaded` or a similar
  `dlopen` panic: the app opens a library by name that the bundle does
  not carry. Add its soname to `include` in `onelf.toml`.
- `xkbcommon: ERROR: ... Compose file` and `onelf-rt: fuse: ...` lines
  are warnings from containers without locale data or FUSE; on their own
  they do not fail a check.

The `onelf` skill covers inspecting the bundle and the runtime's
diagnostics.

## What it does not cover

- A display running only NVIDIA's proprietary driver. It needs the
  hardware; test on such a machine by hand when a change touches GL.
- aarch64. Run the script on an aarch64 machine with the aarch64 bundle;
  the `arch` check is skipped there because the image is x86_64 only.
- Real use. `--smoke` proves the app starts and draws, not that its
  features work; exercise those by hand on at least one host.
