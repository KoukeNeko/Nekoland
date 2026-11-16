# Nekoland

A Wayland compositor written in Rust on [Smithay](https://github.com/Smithay/smithay),
with the pieces of a desktop around it: a panel, a dock, a launcher and an overview
of windows and desktops after Mission Control.

Started on a bare console it drives the display itself through DRM/KMS, with input
from libinput. Started inside another Wayland or X11 session it runs as a window
there, which is handy while working on it.

## Build and run

```
cargo run
```

It needs the development files of Wayland, libinput, libseat, libudev, GBM, EGL,
xkbcommon, Cairo and Pango, and at run time `Xwayland` for X11 programs.

With no configuration it starts a terminal. `-c COMMAND` starts something else.

## Configuration

`~/.config/nekoland/config.toml`; see `shell/config.toml` for one that starts the
panel, the dock, a wallpaper and notifications. Every key is optional:

| Key | Meaning |
| --- | --- |
| `mode` | Display mode as `WIDTHxHEIGHT`; the display's preferred one otherwise |
| `autostart` | Shell commands started once the compositor is up |
| `terminal`, `launcher` | Commands behind the shortcuts below |
| `launcher_search` | What the launcher takes before the text to search for |
| `volume_up`, `volume_down`, `volume_mute`, `brightness_up`, `brightness_down` | Commands behind the media keys |
| `glass` | Layer surfaces drawn on glass, by namespace, with the corner radius they draw: `{ waybar = 18, notifications = 14, launcher = 16 }` unless set |

The panel and the dock in `shell/` are [waybar](https://github.com/Alexays/Waybar)
bars; the launcher is fuzzel, the wallpaper swaybg and notifications mako. The
example also starts Fcitx5 for input methods.

Programs learn whether to be dark from `xdg-desktop-portal`. With
`xdg-desktop-portal-gtk` installed, copy `shell/nekoland-portals.conf` to
`~/.config/xdg-desktop-portal/`; the GSetting
`org.gnome.desktop.interface color-scheme` then switches them.

## Glass

Nekoland's glass is called Catseye（貓眼）. A panel, dock, notification or launcher
named in `glass` is drawn over a blur of what is behind it, a little more saturated,
with a thin line of light just inside its top edge, like the band of light across a
cat's-eye stone. The surface's own colour goes on top, so it has to be translucent
for the glass to show; the styles in `shell/` are. Glass behind a bar that spans the
whole output has square corners.

## Shortcuts

| Keys | Action |
| --- | --- |
| Super, or Ctrl+Alt+Up | Overview |
| Typing in the overview | Search applications |
| Ctrl+Alt+Left / Right, Super+PageUp / PageDown | Previous / next desktop |
| Super+A | Launcher |
| Ctrl+Alt+T, Super+Return | Terminal |
| Alt+Tab, Super+Tab | Next window |
| Super+Up / Down | Maximize / restore |
| Super+Left / Right | Tile to a half |
| Super+H | Minimize |
| F11 | Fullscreen |
| Alt+F4, Super+Q | Close |
| Super+drag | Move a window |
| Ctrl+Alt+F1…F12 | Switch virtual terminal |
| Ctrl+Alt+Backspace | Quit |

## Widgets

`ui/` holds Nekoland's own widgets, written in [Slint](https://slint.dev): the
colours and sizes in `ui/ui/theme.slint`, the widgets in `ui/ui/widgets.slint`.
`cargo run -p nekoland-ui` opens a window with all of them. It follows the
system's dark or light setting; `NEKOLAND_THEME=dark` or `light` pins one.

## From other programs

`nekoland msg WORD` talks to the running compositor: `overview`, `workspace-add`,
`workspace-next`, `workspace-previous`, `quit`.

## Not there yet

- One GPU and one output; displays are not hot-plugged.
- No screen capture, screen lock, or idle handling.
- X11 windows have no title bars, and X11 and Wayland programs do not share a clipboard.
- The overview does not show previews of the other desktops, and windows cannot be dragged between desktops.
- Application icons in the overview are PNG only.

## Origin

The starting point was `smallvil`, the minimal example compositor of Smithay
0.7.0, used under its MIT license (`licenses/SMITHAY-MIT.txt`).
