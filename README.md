# Nekoland

A Wayland compositor written in Rust on [Smithay](https://github.com/Smithay/smithay),
with the pieces of a desktop around it: a menu bar and an overview of windows and
desktops after the Mac's, a dock and a launcher.

Started on a bare console it drives the display itself through DRM/KMS, with input
from libinput. Started inside another Wayland or X11 session it runs as a window
there, which is handy while working on it.

## Build and run

```
cargo run
```

It needs the development files of Wayland, libinput, libseat, libudev, GBM, EGL,
xkbcommon, Cairo, Pango and gdk-pixbuf, and at run time `Xwayland` for X11 programs
and a D-Bus session bus for the menu bar.

With no configuration it starts a terminal. `-c COMMAND` starts something else.

## Configuration

`~/.config/nekoland/config.toml`; see `shell/config.toml` for one that starts the
dock, a wallpaper, notifications and an input method. Every key is optional:

| Key | Meaning |
| --- | --- |
| `mode` | Display mode as `WIDTHxHEIGHT`; the display's preferred one otherwise |
| `autostart` | Shell commands started once the compositor is up |
| `terminal`, `launcher` | Commands behind the shortcuts below |
| `launcher_search` | What the launcher takes before the text to search for |
| `volume_up`, `volume_down`, `volume_mute`, `brightness_up`, `brightness_down` | Commands behind the media keys |
| `glass` | Layer surfaces drawn on glass, by namespace, with the corner radius they draw: `{ waybar = 18, notifications = 14, launcher = 16 }` unless set |

The dock in `shell/` is a [waybar](https://github.com/Alexays/Waybar) bar; the
launcher is fuzzel, the wallpaper swaybg and notifications mako. The example also
starts Fcitx5 for input methods.

Programs learn whether to be dark from `xdg-desktop-portal`. With
`xdg-desktop-portal-gtk` installed, copy `shell/nekoland-portals.conf` to
`~/.config/xdg-desktop-portal/`; the GSetting
`org.gnome.desktop.interface color-scheme` then switches them.

## Menu bar

Nekoland draws the bar along the top itself, as the Mac does:

- The cat on the left opens the system menu: sleep, restart, shut down and log out.
  The last three ask first, in a dialog (`nekoland-confirm`, from `ui/`).
- Then the active application's menu, by its name, and its File, Window and Help
  menus. They are built from the actions the application exports on D-Bus
  (About, Settings, Keyboard Shortcuts, New Window, Help, Quit), plus what Nekoland
  does itself: hide, minimize, zoom, tile, move to another desktop, list its windows.
- On the right, the status items programs register as StatusNotifierItems, with their
  menus, and the clock.

A click opens a menu; then moving across the bar opens the others without clicking. A
press dragged onto an entry and let go chooses it. Ctrl+F2 opens the first menu for
the keyboard: the arrows move, Return chooses, Escape closes. It follows the desktop's
dark or light setting.

## Glass

Nekoland's glass is called Catseye（貓眼）. The menu bar and its menus, and a
dock, notification or launcher named in `glass`, are drawn over a blur of what is behind it, a little more saturated,
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
| Ctrl+F2 | Menu bar |
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

- The menu bar does not show an application's own menus yet (GTK menubars over
  `gtk_shell1`, Qt's over the KDE appmenu protocol, X11 ones over the AppMenu
  registrar), has no Help search, and shows no network or volume of its own: those
  come as status items, from programs such as nm-applet.
- One GPU and one output; displays are not hot-plugged.
- No screen capture, screen lock, or idle handling.
- X11 windows have no title bars, and X11 and Wayland programs do not share a clipboard.
- The overview does not show previews of the other desktops, and windows cannot be dragged between desktops.
- Application icons in the overview are PNG only.

## Origin

The starting point was `smallvil`, the minimal example compositor of Smithay
0.7.0, used under its MIT license (`licenses/SMITHAY-MIT.txt`).
