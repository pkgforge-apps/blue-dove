#!/bin/sh
# Copies the shared files from pkgforge-apps/.github into this app:
#
#   agents/GUIDELINES.md  into the marked block of AGENTS.md
#   agents/skills/<name>  into .agents/skills/<name>
#   app/<path>            to <path>: the shared scripts and the workflows
#                         that call the shared reusable workflows
#
# The workflows are pinned to the revision synced from. Shared skills and
# files the previous sync added and the source no longer has are removed.
# Every skill in .agents/skills, the app's own included, gets a symlink in
# .claude/skills, the only place Claude Code looks for project skills.
# What came from where is recorded in .agents/shared-guidelines.lock.
#
# Usage: scripts/sync-guidelines.sh
#
# PKGFORGE_SHARED_REF picks the branch, tag or commit to sync from
# (default main). PKGFORGE_SHARED names a local checkout to sync from
# instead, for trying out a change to the shared files before it lands.
set -eu
cd "$(dirname "$0")/.."

repo=pkgforge-apps/.github
lock=.agents/shared-guidelines.lock
begin='<!-- BEGIN SHARED GUIDELINES'
end='<!-- END SHARED GUIDELINES'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if [ -n "${PKGFORGE_SHARED:-}" ]; then
    src=$PKGFORGE_SHARED
    sha=$(git -C "$src" rev-parse HEAD 2>/dev/null || echo main)
    origin="local checkout at $sha, not a published revision"
else
    ref=${PKGFORGE_SHARED_REF:-main}
    if printf '%s' "$ref" | grep -Eq '^[0-9a-f]{40}$'; then
        sha=$ref
    else
        sha=$(curl -fsSL -H 'Accept: application/vnd.github.sha' \
            "https://api.github.com/repos/$repo/commits/$ref")
    fi
    mkdir "$work/src"
    curl -fsSL "https://codeload.github.com/$repo/tar.gz/$sha" |
        tar -xzf - -C "$work/src" --strip-components=1
    src=$work/src
    origin="$repo@$sha"
fi

if [ ! -f "$src/agents/GUIDELINES.md" ] || [ ! -d "$src/agents/skills" ] ||
    [ ! -d "$src/app" ]; then
    echo "$src has no agents/GUIDELINES.md, agents/skills and app" >&2
    exit 1
fi
if ! grep -q "^$begin" AGENTS.md || ! grep -q "^$end" AGENTS.md; then
    echo "AGENTS.md has no shared guidelines markers" >&2
    exit 1
fi

# Every file is written beside its target and renamed over it, so a file
# that is running, this script included, keeps reading its old copy.
place() {
    mkdir -p "$(dirname "$2")"
    cp "$1" "$2.sync"
    mv "$2.sync" "$2"
}

awk -v begin="$begin" -v end="$end" -v shared="$src/agents/GUIDELINES.md" '
    index($0, begin) == 1 {
        print
        print ""
        while ((getline line < shared) > 0) print line
        print ""
        skipping = 1
        next
    }
    index($0, end) == 1 { skipping = 0 }
    !skipping { print }
' AGENTS.md > "$work/AGENTS.md"
place "$work/AGENTS.md" AGENTS.md

if [ -f "$lock" ]; then
    sed -n 's/^skill //p' "$lock" | while read -r name; do
        if [ ! -d "$src/agents/skills/$name" ]; then
            rm -rf ".agents/skills/$name"
            if [ -L ".claude/skills/$name" ]; then
                rm ".claude/skills/$name"
            fi
        fi
    done
    sed -n 's/^file //p' "$lock" | while read -r path; do
        if [ ! -f "$src/app/$path" ]; then
            rm -f "$path"
        fi
    done
fi

mkdir -p .agents/skills .claude/skills
{
    echo "# Written by scripts/sync-guidelines.sh. Do not edit."
    echo "source $origin"
} > "$work/lock"

for dir in "$src"/agents/skills/*/; do
    name=$(basename "$dir")
    rm -rf ".agents/skills/$name"
    cp -R "$dir" ".agents/skills/$name"
    echo "skill $name" >> "$work/lock"
done

for dir in .agents/skills/*/; do
    name=$(basename "$dir")
    if [ ! -e ".claude/skills/$name" ] && [ ! -L ".claude/skills/$name" ]; then
        ln -s "../../.agents/skills/$name" ".claude/skills/$name"
    fi
done

(cd "$src/app" && find . -type f | sed 's|^\./||' | LC_ALL=C sort) > "$work/files"
while read -r path; do
    case $path in
    .github/workflows/*)
        sed "s|@__SHARED_REF__|@$sha|g" "$src/app/$path" > "$work/file"
        place "$work/file" "$path"
        ;;
    *)
        place "$src/app/$path" "$path"
        ;;
    esac
    chmod "$(if [ -x "$src/app/$path" ]; then echo 755; else echo 644; fi)" "$path"
    echo "file $path" >> "$work/lock"
done < "$work/files"

place "$work/lock" "$lock"
echo "synced from $origin"
