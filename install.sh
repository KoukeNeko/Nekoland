#!/bin/sh
# Installs Nekoland on an Ubuntu without a desktop, from its console or over SSH: the
# packages it runs with, the compositor and its programs in /usr/local (CI's build of them
# where there is one for this Ubuntu and processor, else built here), the
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
    libadwaita-1-dev libfontconfig-dev libfreetype-dev libpam0g-dev"
# What it runs with: Mesa to draw, Xwayland, the wallpaper, terminal, portals, the network,
# sound, printing, languages, clipboard, fonts and icons, the input method, and the applications the example
# configuration keeps in the Dock.
run_packages="libgl1-mesa-dri libegl-mesa0 libgbm1 libseat1 libinput10 xwayland xsettingsd swaylock swaybg foot grim
    wl-clipboard xclip dbus-user-session dconf-gsettings-backend gsettings-desktop-schemas
    xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr
    network-manager wpasupplicant network-manager-openvpn bluez cups locales rsync
    pipewire pipewire-pulse wireplumber brightnessctl power-profiles-daemon orca
    grub-common python3-fonttools fonts-ubuntu fonts-noto-cjk fonts-noto-color-emoji adwaita-icon-theme yaru-theme-icon
    ubuntu-wallpapers fcitx5 fcitx5-chewing nautilus gnome-text-editor gnome-calculator
    file libimage-exiftool-perl ffmpeg poppler-utils libarchive-tools sqlite3 desktop-file-utils
    libharfbuzz-bin at-spi2-core python3-gi python3-pil gir1.2-gdkpixbuf-2.0 librsvg2-common"

say "Installing packages"
sudo true
sudo apt-get update -q
# shellcheck disable=SC2086 # the lists are meant to split into words
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends \
    git curl ca-certificates $run_packages

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

# What CI built of this very commit for this Ubuntu and processor, as $target, if it published
# one and the checkout is unchanged; NEKOLAND_BUILD=1 builds here regardless.
prebuilt() {
    [ -z "${NEKOLAND_BUILD:-}" ] || return 1
    [ -z "$(git -C "$src" status --porcelain --untracked-files=no 2>/dev/null)" ] || return 1
    name=nekoland-ubuntu-$(. /etc/os-release && echo "$VERSION_ID")-$(dpkg --print-architecture).tar.gz
    url=https://github.com/KoukeNeko/Nekoland/releases/download/continuous/$name
    download=$(mktemp -d)
    trap 'rm -rf "$download"' EXIT
    curl -fsSL -o "$download/$name" "$url" || return 1
    curl -fsSL -o "$download/$name.sha256" "$url.sha256" || return 1
    (cd "$download" && sha256sum -c --quiet "$name.sha256") || return 1
    tar -xzf "$download/$name" -C "$download" || return 1
    [ "$(cat "$download/COMMIT")" = "$(git -C "$src" rev-parse HEAD 2>/dev/null)" ] || return 1
    target=$download
}

if prebuilt; then
    say "Using the build CI made of $(cut -c1-7 "$target/COMMIT")"
else
    say "Installing what it is built with"
    # shellcheck disable=SC2086
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends $build_packages

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
fi

say "Installing into /usr/local"
sudo true
# Next to each other: the compositor finds its programs and libnekoland_menus.so where it is.
for program in "$target/release/nekoland" "$target/release/nekoland-"*; do
    case $program in *.d) continue ;; esac
    sudo install -m 755 "$program" /usr/local/bin/
done
sudo install -m 644 "$target/release/libnekoland_menus.so" /usr/local/bin/
sudo install -Dm 644 -t /usr/local/share/applications "$src"/shell/*.desktop
# GIO finds which program opens a kind of file from the folder's cache, not the files themselves.
sudo update-desktop-database -q /usr/local/share/applications
# Icons Files uses that Adwaita lacks; a theme that has its own still draws its own.
sudo install -Dm 644 -t /usr/local/share/icons/hicolor/scalable/actions "$src"/shell/icons/*.svg
# Nekoland's applications keep their icons under any theme; the other applications' icons it
# draws are its own theme, the one used until the person picks another.
sudo install -Dm 644 -t /usr/local/share/icons/hicolor/scalable/apps "$src"/shell/icons/apps/io.github.koukeneko.Nekoland.*.svg
sudo install -Dm 644 -t /usr/local/share/icons/Nekoland "$src/shell/icons/Nekoland/index.theme"
# The look's theme for GTK 3 programs, light and dark.
sudo install -Dm 644 -t /usr/local/share/themes/Nekoland "$src/shell/gtk3/Nekoland/index.theme"
sudo install -Dm 644 -t /usr/local/share/themes/Nekoland/gtk-3.0 "$src"/shell/gtk3/Nekoland/gtk-3.0/*.css
sudo install -Dm 644 -t /usr/local/share/icons/Nekoland/scalable/apps "$src"/shell/icons/apps/*.svg
sudo install -m 755 "$src/shell/conform-icons.py" /usr/local/bin/nekoland-conform-icons
sudo install -m 755 "$src/shell/hand-over-network.sh" /usr/local/bin/nekoland-hand-over-network
sudo install -Dm 644 -t /usr/local/share/dbus-1/services \
    "$src/shell/org.freedesktop.impl.portal.desktop.nekoland.service" \
    "$src/shell/org.freedesktop.FileManager1.service"
# xdg-desktop-portal reads its backends from its own folder only, not from /usr/local.
sudo install -Dm 644 -t /usr/share/xdg-desktop-portal/portals "$src/shell/nekoland.portal"
sudo install -Dm 644 -t /usr/share/xdg-desktop-portal "$src/shell/nekoland-portals.conf"
sudo install -Dm 644 "$src/shell/nekoland-lock.pam" /etc/pam.d/nekoland-lock

# The boot menu in Nekoland's look, where GRUB is what boots the computer; a board that boots
# some other way has no /etc/default/grub and is left as it is.
if [ -f /etc/default/grub ]; then
    say "Installing the GRUB theme"
    sh "$src/shell/grub/install-theme.sh"
fi

# Other apps' icons on Nekoland's tile, for this user; Settings runs it again on request.
say "Restyling app icons"
nekoland-conform-icons

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

# Settings' 網路 page is NetworkManager's, but Ubuntu Server's netplan leaves the Ethernet
# ports to systemd-networkd. Over SSH, applying it at once could cut the connection this runs
# over, and NetworkManager may be given another address, so then it waits for the next boot.
# Settings' 改用 NetworkManager runs the same.
network_note=
if command -v netplan >/dev/null && [ "$(sudo netplan get renderer)" != NetworkManager ]; then
    say "Handing the network to NetworkManager"
    if [ -n "${SSH_CONNECTION:-}" ]; then
        sudo nekoland-hand-over-network --at-next-boot
        network_note=" NetworkManager takes over the network as the computer starts next."
    else
        sudo nekoland-hand-over-network
    fi
fi

say "Done. Log in on the first console (Ctrl+Alt+F1) to start Nekoland, or run nekoland there.$network_note"
