#!/bin/sh
# Smoke-runs a Linux bundle on the hosts users have: this desktop under
# Wayland and X11, then containers covering a host with no GL at all,
# current Arch, Debian stable, Ubuntu 22.04's older Mesa, and Alpine's
# musl. Each check launches the bundle with --smoke, so the app has to
# open a window and draw.
#
# Usage: test-bundle.sh <bundle>
#
# Needs podman or docker for the container checks; CONTAINER_ENGINE
# picks one. Checks that cannot run here are reported as skipped. Exits
# non-zero when any check fails.
set -u

if [ $# -ne 1 ] || [ ! -f "$1" ]; then
    echo "usage: $0 <bundle>" >&2
    exit 2
fi
bundle=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
name=$(basename "$bundle")
logs=$(mktemp -d)
failed=0

engine=${CONTAINER_ENGINE:-}
if [ -z "$engine" ]; then
    for candidate in podman docker; do
        if command -v "$candidate" > /dev/null 2>&1; then
            engine=$candidate
            break
        fi
    done
fi

report() {
    label=$1
    status=$2
    log=$logs/$label.log
    if [ "$status" -eq 0 ]; then
        printf '  ok    %s\n' "$label"
    else
        printf '  FAIL  %s (exit %s)\n' "$label" "$status"
        grep -v 'xkbcommon' "$log" | tail -n 8 | sed 's/^/          /'
        failed=1
    fi
}

skip() {
    printf '  skip  %s: %s\n' "$1" "$2"
}

host() {
    label=$1
    shift
    env "$@" timeout 120 "$bundle" --smoke > "$logs/$label.log" 2>&1
    report "$label" $?
}

# Runs the bundle in a fresh container. $3 installs what the image needs
# and must leave an `xvfb-run` on PATH, or be empty to draw on this
# desktop's X server instead.
container() {
    label=$1
    image=$2
    setup=$3
    if [ -z "$engine" ]; then
        skip "$label" "no podman or docker"
        return
    fi
    if [ -n "$setup" ]; then
        # The trailing exit keeps sh as PID 1: xvfb-run as PID 1 never
        # sees the signal Xvfb sends when it is ready, and waits forever.
        set -- sh -c "{ $setup; } >/dev/null 2>&1 &&
            timeout 300 xvfb-run -a timeout 120 /b/$name --smoke; exit \$?"
        "$engine" run --rm --security-opt label=disable \
            -v "$(dirname "$bundle")":/b:ro "$image" "$@" > "$logs/$label.log" 2>&1
    else
        if [ -z "${DISPLAY:-}" ] || [ ! -d /tmp/.X11-unix ]; then
            skip "$label" "no X server on this desktop"
            return
        fi
        set -- --network none -e DISPLAY="$DISPLAY" -v /tmp/.X11-unix:/tmp/.X11-unix:ro
        if [ -n "${XAUTHORITY:-}" ] && [ -f "$XAUTHORITY" ]; then
            set -- "$@" -e XAUTHORITY=/xauth -v "$XAUTHORITY":/xauth:ro
        fi
        "$engine" run --rm --security-opt label=disable "$@" \
            -v "$(dirname "$bundle")":/b:ro "$image" timeout 120 "/b/$name" --smoke \
            > "$logs/$label.log" 2>&1
    fi
    report "$label" $?
}

apt_xvfb='export DEBIAN_FRONTEND=noninteractive; apt-get update -qq &&
    apt-get install -y -qq --no-install-recommends xvfb xauth \
    libgl1 libglx-mesa0 libegl1 libgl1-mesa-dri'

# The Arch mirrors drop transactions under load, so retry the sync.
# shellcheck disable=SC2016 # the container expands $p, not this shell
arch_xvfb='p=0
until pacman -Sy --noconfirm --needed xorg-server-xvfb xorg-xauth mesa; do
    p=$((p + 1))
    [ "$p" -lt 3 ] || exit 1
    sleep 10
done'

echo "testing $name"

if [ -n "${WAYLAND_DISPLAY:-}" ]; then
    host host-wayland
else
    skip host-wayland "no Wayland session"
fi
if [ -n "${DISPLAY:-}" ]; then
    host host-x11 -u WAYLAND_DISPLAY
else
    skip host-x11 "no X server"
fi

container no-gl docker.io/library/debian:stable-slim ''
if [ "$(uname -m)" = x86_64 ]; then
    container arch docker.io/library/archlinux:latest "$arch_xvfb"
else
    skip arch "the archlinux image is x86_64 only"
fi
container debian docker.io/library/debian:stable-slim "$apt_xvfb"
container ubuntu-22.04 docker.io/library/ubuntu:22.04 "$apt_xvfb"
container alpine docker.io/library/alpine:latest 'apk add -q xvfb-run mesa-dri-gallium'

echo "logs: $logs"
exit "$failed"
