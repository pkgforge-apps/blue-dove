---
name: add-dependency
description: Vet a Rust crate before adding it to a pkgforge-apps app, decide whether to inline the functionality instead, and add it with cargo add using minimal features. Use whenever a change needs a new crate, a new feature on an existing crate, or a replacement for an unmaintained one.
---

# Add a dependency

Every crate is weight the app carries forever: build time, binary size,
supply chain and upkeep. Run through these steps in order and stop at
the first that settles it.

## 1. Can it be inlined?

If the app needs a small slice of the crate (a parser for one format, a
helper, a single algorithm) and that slice is a few dozen lines, write it
in the app with tests instead. Examples that should usually be inlined:
hex encoding, a ring buffer, simple glob matching, a debounce, a semver
compare, reading one well-specified binary header.

Keep a crate when correctness is hard and the stakes are real:
cryptography, compression, image codecs, Unicode segmentation, TLS,
complex text formats.

## 2. Is it maintained?

```sh
crate=<name>
curl -s -A pkgforge-apps "https://crates.io/api/v1/crates/$crate" \
  | jq '.crate | {max_stable_version, updated_at, downloads, repository}'
```

Then look at the repository:

```sh
gh repo view <owner>/<repo> --json pushedAt,isArchived,openIssues
gh issue list -R <owner>/<repo> --state open --limit 20
gh pr list -R <owner>/<repo> --state open --limit 20
```

- A release or commit within the last year: fine.
- Older than a year: acceptable only when the crate is clearly complete
  and quiet, meaning few open issues, no unanswered bug reports and no
  stale PRs fixing real bugs. Otherwise look for a maintained
  alternative or inline it.
- Archived, or with a notice saying it is deprecated or unmaintained:
  no.
- Check `cargo audit`'s advisory database for it when in doubt.

## 3. What does it pull in?

Add it, then look:

```sh
cargo add <crate> --no-default-features --features <only,what,is,used>
cargo tree -e normal -i <crate>
cargo tree -e normal --depth 1 -p <crate>
```

- Prefer pure Rust. A crate that links a C library adds a system
  dependency on every platform and complicates the onelf bundle.
- A large tree for a small feature is a reason to back out with
  `cargo remove <crate>` and go back to step 1.
- No async runtime unless the app genuinely needs one.
- Platform-only crates go under a target table:
  `cargo add <crate> --target 'cfg(target_os = "linux")'`.

## 4. Commit

Never edit dependency entries in `Cargo.toml` by hand, never add git
dependencies or `[patch]` sections. Commit the manifest and `Cargo.lock`
together with the code that uses the crate, and run `scripts/check.sh`
first.
