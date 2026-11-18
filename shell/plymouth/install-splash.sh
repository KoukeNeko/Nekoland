#!/bin/sh
# Installs Nekoland's boot splash drawn for the screen's size, as Plymouth's default theme,
# and rebuilds the initramfs, which carries it. Run by grub/install-theme.sh, with its mode:
#
#     install-splash.sh WIDTHxHEIGHT
set -eu

here=$(cd "$(dirname "$0")" && pwd)
mode=${1:?usage: install-splash.sh WIDTHxHEIGHT}
dest=/usr/share/plymouth/themes/nekoland

[ -d /usr/share/plymouth/themes ] || { echo "install-splash.sh: needs plymouth" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 "$here/make-theme.py" "${mode%x*}" "${mode#*x}" "$work/theme"

sudo rm -rf "$dest"
sudo install -Dm 644 -t "$dest" "$work"/theme/*
# Ubuntu picks the theme through the default.plymouth alternative, and the initramfs
# carries it, so that is rebuilt.
sudo update-alternatives --install /usr/share/plymouth/themes/default.plymouth default.plymouth \
    "$dest/nekoland.plymouth" 150
sudo update-alternatives --set default.plymouth "$dest/nekoland.plymouth"
sudo update-initramfs -u
