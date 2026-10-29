#!/bin/sh
# Runs every gate CI runs and reports each one's own exit status.
set -u
cd "$(dirname "$0")/.." || exit 1

fail=0
gate() {
    label=$1
    shift
    if out=$("$@" 2>&1); then
        printf '  ok    %s\n' "$label"
    else
        printf '  FAIL  %s\n' "$label"
        printf '%s\n' "$out" | tail -n 25 | sed 's/^/          /'
        fail=1
    fi
}

gate "fmt" cargo fmt --all --check
gate "clippy" cargo clippy --locked --all-targets --all-features --quiet -- -D warnings
gate "test" cargo test --locked --all-targets --all-features --quiet
gate "doc" env RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --document-private-items --quiet
gate "ascii" ./scripts/check-ascii.sh
if [ -d docs/node_modules ]; then
    gate "docs" sh -c 'cd docs && bun run build'
fi

if [ "$fail" -eq 0 ]; then
    echo "all gates passed"
else
    echo "GATES FAILED"
fi
exit "$fail"
