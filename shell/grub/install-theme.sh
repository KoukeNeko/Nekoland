#!/bin/sh
# Installs Nekoland's GRUB theme drawn for this computer's screen: the pictures at the
# screen's own pixels and the menu's font at a size to match, in /boot/grub/themes/nekoland;
# then points /etc/default/grub at it, at that screen size, makes the menu show for a few
# seconds on every boot, installs the boot splash (../plymouth), and rebuilds grub.cfg. Run it
# again to update, or after changing screens. NEKOLAND_GRUB_MODE=WIDTHxHEIGHT draws it for
# another size.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
dest=/boot/grub/themes/nekoland
font='/usr/share/fonts/truetype/ubuntu/Ubuntu[wdth,wght].ttf'
defaults=/etc/default/grub
fallback_mode=1920x1080

die() { echo "install-theme.sh: $*" >&2; exit 1; }

[ -f "$font" ] || die "needs fonts-ubuntu"
# The system's Python, which apt's python3-fonttools and python3-pil are for, not one first
# on PATH, as Homebrew's.
python=/usr/bin/python3
"$python" -c 'import fontTools, PIL' 2>/dev/null || die "needs python3-fonttools and python3-pil"
command -v grub-mkfont >/dev/null || die "needs grub-common"

# The preferred mode of the first connected display, as the kernel read it from the display;
# none on a computer used only over the network, which gets the fallback.
native_mode() {
    for status in /sys/class/drm/card*-*/status; do
        [ "$(cat "$status" 2>/dev/null)" = connected ] || continue
        mode=$(head -n 1 "${status%/status}/modes" 2>/dev/null)
        case $mode in [0-9]*x[0-9]*) echo "${mode%%[!0-9x]*}"; return ;; esac
    done
    echo "$fallback_mode"
}

mode=${NEKOLAND_GRUB_MODE:-$(native_mode)}
width=${mode%x*}
height=${mode#*x}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

font_size=$("$python" "$here/make-theme.py" "$width" "$height" "$work/theme")
# GRUB draws the menu's text from its own one-bit font: Ubuntu's Medium, the weight that
# holds up at one bit; the Ubuntu-M.ttf beside it is only a link to the variable font.
"$python" -m fontTools varLib.instancer "$font" wght=500 wdth=100 -o "$work/medium.ttf" --quiet
grub-mkfont -n Nekoland -s "$font_size" --no-hinting -o "$work/theme/nekoland.pf2" "$work/medium.ttf"

# Rebuilding grub.cfg takes a while, so it is left as it is when neither the theme nor what
# is set for it below changes; a theme not installed yet differs.
changed=false
if ! diff -rq "$work/theme" "$dest" >/dev/null 2>&1; then
    sudo rm -rf "$dest"
    sudo install -Dm 644 -t "$dest" "$work"/theme/*
    changed=true
fi
settings() { cat "$defaults" /etc/default/grub.d/99-nekoland.cfg 2>/dev/null | cksum; }
settings_before=$(settings)

# Set KEY=VALUE in /etc/default/grub, replacing any line (even a commented one) for KEY.
set_default() {
    if sudo grep -q "^#\?$1=" "$defaults"; then
        sudo sed -i "s|^#\?$1=.*|$1=$2|" "$defaults"
    else
        echo "$1=$2" | sudo tee -a "$defaults" >/dev/null
    fi
}
set_default GRUB_THEME "\"$dest/theme.txt\""
set_default GRUB_GFXMODE "\"$mode,auto\""
set_default GRUB_DISTRIBUTOR '"Nekoland"'

# Ubuntu hides the menu (GRUB_TIMEOUT_STYLE=hidden, timeout 0), and its cloud images fix the
# terminal to the console, which has no theme. Files in /etc/default/grub.d are read after
# /etc/default/grub, so this one has the last word. Plymouth needs quiet splash to show, and
# on a board whose firmware names a serial console it shows only text unless told to ignore it.
sudo install -d /etc/default/grub.d
sudo tee /etc/default/grub.d/99-nekoland.cfg >/dev/null <<'CFG'
# Nekoland: the boot menu is shown for 5 seconds, graphically, and the splash is on.
GRUB_TIMEOUT_STYLE=menu
GRUB_TIMEOUT=5
GRUB_RECORDFAIL_TIMEOUT=5
unset GRUB_TERMINAL GRUB_TERMINAL_OUTPUT
for option in quiet splash plymouth.ignore-serial-consoles; do
    case " $GRUB_CMDLINE_LINUX_DEFAULT " in
        *" $option "*) ;;
        *) GRUB_CMDLINE_LINUX_DEFAULT="$GRUB_CMDLINE_LINUX_DEFAULT $option" ;;
    esac
done
CFG

sh "$here/../plymouth/install-splash.sh" "$mode"
if $changed || [ "$(settings)" != "$settings_before" ]; then
    sudo update-grub
fi
