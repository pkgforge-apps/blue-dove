#!/bin/sh
# Installs what the shared AppImage script leaves out for an audio app.
#
# Usage: sh packaging/appimage/get-dependencies.sh
#
# Runs in the Arch build container, before
# `scripts/package-appimage.sh`. The setup action installs the winit
# libraries; cpal links libasound, so alsa-lib is needed to build the app
# at all, and the audio servers quick-sharun deploys plugins for must be
# installed before it traces the binary.
#
# In CI `anylinux-setup-action` has already synced the keyring and the
# databases. Run by hand in a bare image they are missing, so this syncs
# them when they are.
set -eu

if ! pacman -Si alsa-lib > /dev/null 2>&1; then
    pacman-key --init
    pacman -Syy --noconfirm archlinux-keyring
fi

# alsa-lib: cpal links libasound. pulseaudio: libpulse, which the ALSA
# pulse plugin loads, and the server the plugin talks to. pulseaudio-alsa:
# that plugin, and the 99-pulseaudio-default.conf that points the default
# PCM at it.
pacman -S --noconfirm --needed \
    alsa-lib pulseaudio pulseaudio-alsa
