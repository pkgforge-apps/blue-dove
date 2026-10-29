#!/bin/sh
# Installs what the Arch build container needs beyond the setup action.
#
# Usage: sh packaging/appimage/get-dependencies.sh
#
# Runs in the Arch build container, before
# `scripts/package-appimage.sh`. The setup action installs the winit
# libraries; this adds the toolchain, the GitHub CLI that attaches the
# image to the release, and what an audio app needs: cpal links libasound,
# so alsa-lib is needed to build the app at all, and the audio servers
# quick-sharun deploys plugins for must be installed before it traces the
# binary.
#
# In CI `anylinux-setup-action` has already synced the keyring and the
# databases. Run by hand in a bare image they are missing, so this syncs
# them when they are.
set -eu

# A pacman transaction is retried, not just re-run once.
#
# libalpm aborts a transfer that drops under a byte a second for ten
# seconds, and a stalled `.sig` cannot fail over to the next server, since
# the signature payload carries no server list. The mirror behind
# ghcr.mirror.archlinux.pkgforge.dev stalls under load, so a single
# transaction fails on its own account rather than because anything is
# wrong with the packages. See pkgforge-dev/mirror-archlinux#76.
# `--needed` makes a retry after a partial install cheap.
retries=3
pacman_retry() {
    attempt=0
    while ! pacman "$@"; do
        attempt=$((attempt + 1))
        if [ "$attempt" -ge "$retries" ]; then
            echo "pacman $* failed $attempt times" >&2
            return 1
        fi
        echo "pacman $* failed, retrying ($attempt/$retries)" >&2
        sleep 10
    done
}

if ! pacman -Si alsa-lib > /dev/null 2>&1; then
    pacman-key --init
    pacman_retry -Syy --noconfirm archlinux-keyring
fi

# rustup: the toolchain the setup action leaves out.
# github-cli: attaches the image to the release.
# alsa-lib: cpal links libasound.
# pulseaudio: libpulse, which the ALSA pulse plugin loads, and the server
# the plugin talks to.
# pulseaudio-alsa: that plugin, and the 99-pulseaudio-default.conf that
# points the default PCM at it.
pacman_retry -S --noconfirm --needed \
    rustup github-cli alsa-lib pulseaudio pulseaudio-alsa
