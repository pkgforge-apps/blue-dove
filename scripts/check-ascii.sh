#!/bin/sh
# Fails when a tracked text file holds a byte outside printable ASCII.
# Write glyphs in Rust strings as \u{..} escapes instead.
set -eu
cd "$(dirname "$0")/.."

if LC_ALL=C grep -rnI '[^[:print:][:space:]]' . \
    --exclude-dir=.git --exclude-dir=.jj --exclude-dir=target \
    --exclude-dir=node_modules --exclude-dir=dist --exclude-dir=cache; then
    echo "non-ASCII bytes found; see the lines above" >&2
    exit 1
fi
