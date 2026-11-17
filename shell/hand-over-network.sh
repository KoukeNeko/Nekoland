#!/bin/sh
# Gives the network ports netplan leaves to systemd-networkd, as Ubuntu Server does, to
# NetworkManager, whose connections Settings' 網路 page sets; run as root. With
# --at-next-boot it is set and takes effect as the computer starts, so that a connection it
# runs over, as SSH, is not cut.
set -eu

# dracut's initramfs leaves systemd-networkd a file taking every port, which would keep
# them from NetworkManager, which then only watches them; masked, networkd keeps only what
# netplan gives it.
DRACUT_DEFAULT=/etc/systemd/network/zzzz-dracut-default.network

netplan set --origin-hint 90-nekoland-networkmanager renderer=NetworkManager
mkdir -p "$(dirname "$DRACUT_DEFAULT")"
ln -sf /dev/null "$DRACUT_DEFAULT"
if [ "${1:-}" != --at-next-boot ]; then
    netplan apply
fi
