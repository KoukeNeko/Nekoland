#!/bin/sh
# Installs Nekoland on an Ubuntu without a desktop, from its console or over SSH: the
# packages it is built and runs with, the compositor and its programs in /usr/local, the
# portal files, an example configuration, and a login on the first console (tty1) that
# starts it. Run it again to update. From a checkout:
#   ./install.sh
# or from nothing, which clones into ~/.local/src/nekoland:
#   curl -fsSL https://raw.githubusercontent.com/KoukeNeko/Nekoland/main/install.sh | sh
set -eu

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" != 0 ] || die "run it as the user who will use Nekoland, not as root; it asks sudo when it needs to"
command -v apt-get >/dev/null || die "it needs Ubuntu's apt-get"

# Build tools and the development files of what the compositor and ui/ link to.
build_packages="build-essential pkg-config clang libclang-dev git curl ca-certificates cargo rustc
    libwayland-dev libxkbcommon-dev libinput-dev libudev-dev libseat-dev libgbm-dev libdrm-dev
    libegl-dev libgles-dev libcairo2-dev libpango1.0-dev libgdk-pixbuf-2.0-dev libgtk-4-dev
    libadwaita-1-dev libfontconfig-dev libfreetype-dev"
# What it runs with: Mesa to draw, Xwayland, the locker, wallpaper, terminal, portals, the network,
# sound, clipboard, fonts and icons, the input method, and the applications the example
# configuration keeps in the Dock.
run_packages="libgl1-mesa-dri libegl-mesa0 libgbm1 xwayland swaylock swaybg foot grim
    wl-clipboard xclip dbus-user-session dconf-gsettings-backend gsettings-desktop-schemas
    xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr
    network-manager network-manager-openvpn
    pipewire pipewire-pulse wireplumber brightnessctl
    fonts-ubuntu fonts-noto-cjk fonts-noto-color-emoji adwaita-icon-theme yaru-theme-icon
    ubuntu-wallpapers fcitx5 fcitx5-chewing nautilus gnome-text-editor gnome-calculator
    file libimage-exiftool-perl ffmpeg poppler-utils libarchive-tools sqlite3
    libharfbuzz-bin at-spi2-core"

say "Installing packages"
sudo true
sudo apt-get update -q
# shellcheck disable=SC2086 # the lists are meant to split into words
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends \
    $build_packages $run_packages

# The source: this checkout, or a clone of its own.
case $0 in
*install.sh) src=$(cd "$(dirname "$0")" && pwd) ;;
*) src= ;;
esac
if [ -z "$src" ] || ! grep -q '^name = "nekoland"' "$src/Cargo.toml" 2>/dev/null; then
    src=${NEKOLAND_SRC:-$HOME/.local/src/nekoland}
    if [ -d "$src/.git" ]; then
        say "Updating $src"
        git -C "$src" pull --ff-only
    else
        say "Cloning into $src"
        git clone https://github.com/KoukeNeko/Nekoland.git "$src"
    fi
fi

# Cargo.toml's rust-version; rustup's when the system's is older, as on Ubuntu 24.04.
minimum_rust=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$src/Cargo.toml")
cargo=cargo
[ -x "$HOME/.cargo/bin/cargo" ] && cargo=$HOME/.cargo/bin/cargo
if ! dpkg --compare-versions "$($cargo --version | cut -d' ' -f2)" ge "$minimum_rust"; then
    say "Installing Rust with rustup, the system's being older than $minimum_rust"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    cargo=$HOME/.cargo/bin/cargo
fi

say "Building (the first build takes a while)"
target=${CARGO_TARGET_DIR:-$src/target}
(cd "$src" && CARGO_TARGET_DIR=$target "$cargo" build --release --locked --workspace)

say "Installing into /usr/local"
sudo true
# Next to each other: the compositor finds its programs and libnekoland_menus.so where it is.
for program in "$target/release/nekoland" "$target/release/nekoland-"*; do
    case $program in *.d) continue ;; esac
    sudo install -m 755 "$program" /usr/local/bin/
done
sudo install -m 644 "$target/release/libnekoland_menus.so" /usr/local/bin/
sudo install -Dm 644 -t /usr/local/share/applications "$src"/shell/*.desktop
# Icons Files uses that Adwaita lacks; a theme that has its own still draws its own.
sudo install -Dm 644 -t /usr/local/share/icons/hicolor/scalable/actions "$src"/shell/icons/*.svg
sudo install -Dm 644 -t /usr/local/share/dbus-1/services \
    "$src/shell/org.freedesktop.impl.portal.desktop.nekoland.service" \
    "$src/shell/org.freedesktop.FileManager1.service"
# xdg-desktop-portal reads its backends from its own folder only, not from /usr/local.
sudo install -Dm 644 -t /usr/share/xdg-desktop-portal/portals "$src/shell/nekoland.portal"
sudo install -Dm 644 -t /usr/share/xdg-desktop-portal "$src/shell/nekoland-portals.conf"

config=${XDG_CONFIG_HOME:-$HOME/.config}/nekoland/config.toml
if [ ! -e "$config" ]; then
    say "Writing $config"
    install -Dm 644 "$src/shell/config.toml" "$config"
fi

# Logging in on the first console starts Nekoland in place of the shell, so that quitting it,
# or its ending while the screen is locked, logs out rather than leaving a shell behind the
# lock. The last session's log is kept beside the new one. An older block is replaced.
marker="# Nekoland: started by logging in on the first console"
say "Starting Nekoland from ~/.profile on tty1"
touch "$HOME/.profile"
sed -i "/^$marker\$/,/^fi\$/d" "$HOME/.profile"
cat >>"$HOME/.profile" <<EOF
$marker
if [ -z "\${WAYLAND_DISPLAY:-}" ] && [ -z "\${DISPLAY:-}" ] && [ "\$(tty)" = /dev/tty1 ]; then
    mkdir -p "\$HOME/.local/state"
    mv -f "\$HOME/.local/state/nekoland.log" "\$HOME/.local/state/nekoland.log.old" 2>/dev/null
    exec nekoland >"\$HOME/.local/state/nekoland.log" 2>&1
fi
EOF

say "Done. Log in on the first console (Ctrl+Alt+F1) to start Nekoland, or run nekoland there"
