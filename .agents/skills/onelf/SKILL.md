---
name: onelf
description: Inspect and debug a pkgforge-apps app's portable Linux bundle built with onelf (https://github.com/QaidVoid/onelf). Covers the recipe in onelf.toml, how the runtime chooses between bundled and host libraries, onelf's inspection commands and runtime diagnostics, and the failures seen so far. Use when the bundle fails to start or draw on some host, when deciding what onelf.toml should bundle, or when reading onelf output.
---

# onelf bundles

An app's Linux bundle is one executable: onelf's runtime followed by a
compressed image of a directory holding the binary, glibc and every
library the app loads. `scripts/package-onelf.sh` builds it from
`onelf.toml`. Reference documentation: https://onelf.qaidvoid.dev
and the `docs/` directory of the onelf repository.

## How a launch works

1. **Mode.** The runtime picks how to expose the image: `memfd` for a
   lone static binary, a FUSE mount in a private user namespace, a
   `fusermount3` mount, a runtime directory extraction, or the
   persistent cache. A host without FUSE falls back to extraction and
   prints `onelf-rt: fuse: ...` lines on the way; those are not errors.
2. **Resolver.** For libraries the host must supply, mainly GPU drivers,
   the runtime builds a link farm of chosen host libraries under
   `$XDG_RUNTIME_DIR/resolve-*`. It compares a library present on both
   sides by symbol versions and keeps the newer; equal sets keep the
   bundled copy. No host directory is ever searched, so a library the
   bundle lacks and the resolver did not choose fails by name.
3. **Host index.** The resolver learns the host's libraries from
   `/etc/ld.so.cache`. A host without one, every musl distribution,
   looks like a host with no libraries, which is why apps bundle Mesa.

## The recipe

The parts of `onelf.toml` an app changes:

| Key | Meaning |
|-----|---------|
| `[compression] payload = "stipple"` | the stipplefs image, smaller than the default blocks; keep it |
| `[update]` | never set here: packaging adds it, see the `release` skill's self-update section |
| `[bundle] gl`, `dri` | bundle Mesa from the build host, where the release job puts onelf-pkgs/mesa; keep both on for egui apps |
| `scan-dlopen` | scan the binary's strings for sonames it opens at runtime |
| `include` | sonames to bundle that the scan cannot see, such as the X11 libraries winit opens by name |
| `exclude` | soname prefixes to leave out |
| `no-gtk` | keep GTK out; winit never uses it |
| `strip` | strip bundled libraries |

The full schema is the onelf docs' recipe reference. Packing warns when
a bundled library reads a compiled-in data path the bundle does not
carry; read that list before shipping.

## Inspect a bundle

```sh
onelf info <bundle>                  # format, payload kind, package id
onelf list <bundle> | grep '\.so'    # what it carries
onelf verify <bundle>                # recompute every hash
onelf extract <bundle> -o <dir>      # unpack it to read files
```

Runtime flags, passed to the bundle itself:

```sh
./<bundle> --onelf-doctor     # mode, and which host libraries it would use
./<bundle> --onelf-version
./<bundle> --onelf-extract <dir>
```

`--onelf-doctor` is the first stop: it lists every library the resolver
compared, the ones it takes from the host, and the link farm's path,
without starting the app.

## Debug a failure

1. Run `--onelf-doctor` on the failing host.
2. Run the app with `LD_DEBUG=libs` and find the first
   `find library=<name>` that ends without a hit. That soname is missing
   from both the bundle and the farm.
3. `ONELF_NO_RESOLVER=1` launches with nothing from the host. When that
   fixes the failure, a host library the resolver chose is the cause.
4. `ONELF_MODE=cache` (or `rundir`, `fuse`) forces one mode and fails
   instead of falling back, to tell a mode problem from a library one.
5. Patching the farm is a quick causal test: symlink the host library
   into the farm the doctor names, rerun, then delete the farm's parent
   `resolve-*` directory so the next launch rebuilds it.

## Failures seen so far

| Symptom | Cause | Fix |
|---------|-------|-----|
| `NoGlutinConfigs`, `kind: NotFound`, `this host has no GL stack` | no usable host GL: musl host, minimal VM or container | bundle Mesa (`gl`, `dri`) |
| `Library libxkbcommon-x11.so could not be loaded` | opened by name, invisible to the scan | add it to `include` |
| a 17 MB bundle full of GTK, cairo and pango | libdecor's GTK plugin pulled in | `no-gtk = true`, `exclude = ["libdecor"]` |

Add a row when you diagnose something new, through the shared copy in
pkgforge-apps/.github.

## Upgrading onelf

The release workflow pins `ONELF_VERSION` and the binaries' SHA-256.
The `release` skill has the procedure. After an upgrade, run the
`test-linux-bundle` skill before merging.
