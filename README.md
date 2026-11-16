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

The panel and the dock in `shell/` are [waybar](https://github.com/Alexays/Waybar)
bars; the launcher is fuzzel, the wallpaper swaybg and notifications mako.

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

## From other programs

`nekoland msg WORD` talks to the running compositor: `overview`, `workspace-add`,
`workspace-next`, `workspace-previous`, `quit`.

## Not there yet

- One GPU and one output; displays are not hot-plugged.
- No screen capture, screen lock, or idle handling.
- X11 windows have no title bars, and X11 and Wayland programs do not share a clipboard.
- Windows are not blurred behind translucent panels.
- The overview does not show previews of the other desktops, and windows cannot be dragged between desktops.
- Application icons in the overview are PNG only.

## Origin

The starting point was `smallvil`, the minimal example compositor of Smithay
0.7.0, used under its MIT license (`licenses/SMITHAY-MIT.txt`).
