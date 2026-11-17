#!/bin/sh
# Installs Nekoland's GRUB theme: copies it to /boot/grub/themes/nekoland, makes the font it
# draws with from Ubuntu's own, points /etc/default/grub at it and rebuilds grub.cfg.
# Run it again to update.
set -eu

src=$(cd "$(dirname "$0")" && pwd)/nekoland
dest=/boot/grub/themes/nekoland
font=/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf
defaults=/etc/default/grub

[ -f "$font" ] || { echo "install-theme.sh: needs fonts-ubuntu" >&2; exit 1; }
command -v grub-mkfont >/dev/null || { echo "install-theme.sh: needs grub-common" >&2; exit 1; }

sudo install -Dm 644 -t "$dest" "$src"/theme.txt "$src"/*.png
sudo grub-mkfont -s 20 --no-hinting -o "$dest/ubuntu-20.pf2" "$font"

# Set KEY=VALUE in /etc/default/grub, replacing any line (even a commented one) for KEY.
set_default() {
    if sudo grep -q "^#\?$1=" "$defaults"; then
        sudo sed -i "s|^#\?$1=.*|$1=$2|" "$defaults"
    else
        echo "$1=$2" | sudo tee -a "$defaults" >/dev/null
    fi
}
set_default GRUB_THEME "\"$dest/theme.txt\""
set_default GRUB_GFXMODE '"1920x1080,auto"'
set_default GRUB_DISTRIBUTOR '"Nekoland"'
sudo grep -q '^GRUB_TERMINAL' "$defaults" && sudo sed -i 's|^GRUB_TERMINAL.*|#&|' "$defaults"

sudo update-grub
