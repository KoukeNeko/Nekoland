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
# Without a label plugin Plymouth cannot draw text, and its script plugin then crashes at the
# first frame (ply_console_viewer_hide on a viewer it never made), which stops the boot.
ls /usr/lib/*/plymouth/label-*.so >/dev/null 2>&1 || { echo "install-splash.sh: needs plymouth-label" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 "$here/make-theme.py" "${mode%x*}" "${mode#*x}" "$work/theme"

sudo rm -rf "$dest"
sudo install -Dm 644 -t "$dest" "$work"/theme/*
# Plymouth reads the theme from plymouthd.conf, and dracut, which builds Ubuntu's initramfs,
# copies the theme it names; the default.plymouth alternative is for what reads that instead.
printf '[Daemon]\nTheme=nekoland\n' | sudo tee /etc/plymouth/plymouthd.conf >/dev/null
sudo update-alternatives --install /usr/share/plymouth/themes/default.plymouth default.plymouth \
    "$dest/nekoland.plymouth" 150
sudo update-alternatives --set default.plymouth "$dest/nekoland.plymouth"
# Plymouth quits once the system is up, before tty1 starts Nekoland, and would hand the screen
# back to the console's text in between. Retained, its last frame stays and the console is left
# in graphics mode, drawing none of the login's text, as long as starting tty1's getty does not
# reset the console to text mode, which systemd does by default. Only where tty1 logs in by
# itself: otherwise its login prompt would be hidden.
quit_dropin=/etc/systemd/system/plymouth-quit.service.d/nekoland.conf
getty_dropin=/etc/systemd/system/getty@tty1.service.d/nekoland-splash.conf
if systemctl cat getty@tty1.service 2>/dev/null | grep -q -- '--autologin'; then
    sudo install -d "$(dirname "$quit_dropin")"
    printf '[Service]\nExecStart=\nExecStart=-/usr/bin/plymouth quit --retain-splash\n' |
        sudo tee "$quit_dropin" >/dev/null
    printf '[Service]\nTTYReset=no\nTTYVHangup=no\nTTYVTDisallocate=no\n' |
        sudo tee "$getty_dropin" >/dev/null
else
    sudo rm -f "$quit_dropin" "$getty_dropin"
fi
sudo systemctl daemon-reload

# The initramfs carries the theme, so it is rebuilt.
sudo update-initramfs -u
