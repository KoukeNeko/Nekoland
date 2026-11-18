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
# Plymouth quits once the system is up, before tty1 starts Nekoland, and its leaving the display
# would bring back the console, text or blank, until Nekoland draws. Where tty1 logs in by itself
# it is only deactivated then, letting go of the display while its last frame stays on it, for
# Nekoland to take over as GDM does; it quits a while later. Plymouth takes these requests only
# from root, so systemd sends them, not Nekoland. getty is kept from waiting for the quit, and
# from resetting tty1 to text mode, which systemd does by default. Without autologin all this is
# left as it was, or tty1's login prompt would be hidden.
quit_dropin=/etc/systemd/system/plymouth-quit.service.d/nekoland.conf
wait_dropin=/etc/systemd/system/plymouth-quit-wait.service.d/nekoland.conf
getty_dropin=/etc/systemd/system/getty@tty1.service.d/nekoland-splash.conf
if systemctl cat getty@tty1.service 2>/dev/null | grep -q -- '--autologin'; then
    sudo install -d "$(dirname "$quit_dropin")" "$(dirname "$wait_dropin")"
    sudo tee "$quit_dropin" >/dev/null <<'UNIT'
[Service]
ExecStart=
ExecStart=-/usr/bin/plymouth deactivate
ExecStart=-/usr/bin/systemd-run --no-block --on-active=15 --unit=nekoland-plymouth-quit /usr/bin/plymouth quit --retain-splash
UNIT
    printf '[Service]\nExecStart=\nExecStart=/bin/true\n' | sudo tee "$wait_dropin" >/dev/null
    printf '[Unit]\nAfter=plymouth-quit.service\n\n[Service]\nTTYReset=no\nTTYVHangup=no\nTTYVTDisallocate=no\n' |
        sudo tee "$getty_dropin" >/dev/null
else
    sudo rm -f "$quit_dropin" "$wait_dropin" "$getty_dropin"
fi
sudo systemctl daemon-reload

# The initramfs carries the theme, so it is rebuilt.
sudo update-initramfs -u
