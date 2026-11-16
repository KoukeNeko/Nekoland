# Nekoland

A Wayland compositor written in Rust on [Smithay](https://github.com/Smithay/smithay),
with the pieces of a desktop around it, after the Mac's: a menu bar, a Dock, an
overview of windows and desktops, notifications and a launcher, all of them its own.

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

`~/.config/nekoland/config.toml`; see `shell/config.toml` for one that adds a
wallpaper and an input method. Every key is optional:

| Key | Meaning |
| --- | --- |
| `mode` | Display mode as `WIDTHxHEIGHT`; the display's preferred one otherwise |
| `autostart` | Shell commands started once the compositor is up |
| `terminal`, `launcher` | Commands behind the shortcuts below |
| `launcher_search` | What the launcher takes before the text to search for |
| `volume_up`, `volume_down`, `volume_mute`, `brightness_up`, `brightness_down` | Commands behind the media keys |
| `glass` | Other layer surfaces or windows drawn on glass, by namespace or application ID, with the corner radius they draw: `{ nekoland-launcher = 16 }` unless set |
| `dock` | The applications kept in the Dock, by desktop entry name; the Dock writes it back when they are rearranged |
| `stacks` | The folders at the Dock's right end, each `{ path, sort, show, view }`: `sort` is `name`, `added`, `modified`, `created` or `kind`, `show` is `stack` or `folder`, `view` is `fan`, `grid`, `list` or `auto`; the downloads folder unless set, written back by the Dock |
| `apps_view` | How the applications are shown: `overview` (the default), `launchpad`, `shelf` or `sidebar`; see Overview |
| `wallpaper`, `wallpaper_mode` | A picture drawn on the desktop by swaybg, and `fill` (the default), `fit` or `center` |
| `clock_24h`, `clock_weekday`, `clock_seconds` | How the menu bar's clock reads: 24 hours and the weekday unless set, no seconds |
| `dock_glass` | Whether the Dock is on glass or on its tint alone; on unless set |
| `natural_scroll`, `tap_to_click`, `pointer_speed` | For mice and touchpads: off, on, and 0, from -1 to 1, unless set |

`nekoland msg reload` reads the file again once it is changed.

The example's wallpaper is swaybg's; it also starts Fcitx5 for input methods.

## Dock, notifications and launcher

- The Dock has a button for all applications, the ones kept in it, after a line the
  others with windows open, and after another the kept folders, the minimized windows,
  each a live picture to click back, and the Trash. Icons are trimmed of the margins their theme draws and shown
  at one size, as the Mac's are. A dot marks those that run, a longer one the active
  one. Pointing at an icon lifts it and names it; a click starts the application, which
  breathes until its window shows, or brings its windows forward, from another desktop
  or minimized too. Resting on a running application shows a live picture of each of
  its windows, to go to one or close it. A right click lists its windows and offers a
  new window, keeping it in the Dock, hiding and quitting. Kept icons are dragged to
  reorder them, out of the Dock to take them off, and a running one dragged among them
  is kept. A minimized window goes to the Dock's right end, next to the Trash, drawn in
  with the Mac's Genie effect, and comes back out the same way.
- A kept folder shows as its newest items piled up or as its own icon, and opens as a
  fan rising from it, a grid of pages to scroll, or a list with submenus for folders; a
  right click sets its order, how it shows and how it opens. A folder dragged in from a
  file manager is kept there, and folders are dragged to reorder or take them off. Files
  dropped on the Trash go to it, and files dropped on an application open in it; the
  Trash is emptied from its right-click menu after asking.
- Notifications are shown by Nekoland, which serves `org.freedesktop.Notifications`:
  banners on glass under the menu bar, which go when their time is up unless pointed
  at; a click does what the program offers for it.
- The launcher, `nekoland-launcher` from `ui/`, finds applications by name, in the
  user's language too, and takes an input method. Clicking the search field of the
  overview's applications opens it with what was typed there. It closes when anything
  else is clicked.

Programs learn whether to be dark from `xdg-desktop-portal`. With
`xdg-desktop-portal-gtk` installed, copy `shell/nekoland-portals.conf` to
`~/.config/xdg-desktop-portal/`; the GSetting
`org.gnome.desktop.interface color-scheme` then switches them.

## Menu bar

Nekoland draws the bar along the top itself, as the Mac does:

- The cat on the left opens the system menu: sleep, restart, shut down and log out.
  The last three ask first, in a dialog (`nekoland-confirm`, from `ui/`).
- Then the active application's menu, by its name, built from the actions it
  exports on D-Bus (About, Settings, Keyboard Shortcuts, Quit) and what Nekoland
  does for any program (hide, hide others, show all).
- Then the application's own menus, when it exports them: Qt programs over KDE's
  appmenu protocol, GTK programs over `gtk_shell1`, X11 programs through the
  `com.canonical.AppMenu.Registrar` service Nekoland provides (Qt needs it to
  export its menus at all). Their dialogs keep the menus of their program.
- For a program without menus, File, Edit, View and Help menus as the Mac gives
  every application: the Edit menu's entries press the usual shortcuts for it.
- The Window menu, from Nekoland: minimize, zoom, tile, move to another desktop,
  and the program's windows. The Help menu searches every other menu as you type.
- On the right: the status items programs register as StatusNotifierItems, with their
  menus; the sound (through `wpctl`, when PipeWire runs) and the network; the clock.

A click opens a menu; then moving across the bar opens the others without clicking. A
press dragged onto an entry and let go chooses it. Submenus open beside their entry,
and stay open while the pointer heads into them. Ctrl+F2 opens the first menu for the
keyboard: the arrows move, also into and out of submenus, Return chooses, Escape
closes. It follows the desktop's dark or light setting.

## Overview

Super, or the overview's key, spreads the desktop's windows apart where they are, each
named under it, as Mission Control does. Where the menu bar was, a strip of the same
glass names the desktops, the current one marked, with a button at the right end that
adds one. Pointing at the strip grows it down, and the windows move down to make room:
each desktop is shown small, wallpaper and windows as they are. A click on a desktop
goes to it and leaves the overview. A window dragged onto a desktop goes there, onto the
add button to a new desktop; the bar opens up for it. A desktop pointed at shows a button at its corner
that takes it away, its windows going to the desktop before it.

Super+A, the Dock's first button, or typing in the overview shows the applications in
place of the windows, as GNOME does: the desktops' bar stays open above them, a search
field under it, and every application a page at a time, turned with the wheel, the
arrows or the dots. Typing narrows them down by name, keyword or command, and Return
starts the first. A click starts one on the desktop shown; one dragged onto a desktop
starts there, onto the add button on a new desktop, and onto the Dock is kept in it,
where the Dock makes room for it.

The search field and a folder's name take an input method: with Fcitx5 running, what
is being composed shows underlined in the field and its candidates in a row under it,
as Nekoland draws them.

`apps_view` shows them other ways instead, without the desktops' bar: `launchpad`,
pages of icons over the whole screen with the search field above; `shelf`, one long
page, the latest started first and the rest under the name of their kind; `sidebar`,
the kinds in a list on glass at the left, with the latest started, and the icons of
the one chosen beside it. On the long page the wheel scrolls, the arrows move from one
application to the next and Return starts it.

The applications are arranged by dragging, as on the Mac's Launchpad: one dropped
between others goes there, the others making room; onto another, the two make a folder
named after its kind; onto a folder, it goes in. A dragged icon held at the grid's side
turns the page. A folder opens over the grid; its applications are reordered there or
dragged out, and a click on its name changes it. The arrangement is kept in
`app-grid.toml` beside `config.toml`.

## Glass

Nekoland's glass is called Catseye（貓眼）. The menu bar and its menus, the Dock,
notifications, the overview's desktops and what `glass` names are drawn over a blur of what is behind it, a little more saturated,
with a thin line of light just inside its top edge, like the band of light across a
cat's-eye stone. The surface's own colour goes on top, so it has to be translucent
for the glass to show; the styles in `shell/` are. Glass behind a bar that spans the
whole output has square corners.

## Shortcuts

| Keys | Action |
| --- | --- |
| Super, or Ctrl+Alt+Up | Overview |
| Typing in the overview | Applications, searched |
| Ctrl+Alt+Left / Right, Super+PageUp / PageDown | Previous / next desktop |
| Super+A | Applications |
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
`workspace-next`, `workspace-previous`, `quit`; `reload` reads the configuration again,
`config` prints the settings in effect, `modes` the display's mode and then all it has,
and `'set KEY = VALUE'` changes one setting in the file, keeping the rest of it, and puts
it in effect, or says why not.

## Not there yet

- In the menu bar, menus taller than the screen do not scroll, the Help search takes
  typed letters but not an input method's, the sound menu steps the volume instead of
  sliding it, and the network menu shows the connection without choosing one.
  GNOME's own programs export no menus, so they get the menus Nekoland makes.
- One GPU and one output; displays are not hot-plugged.
- No screen capture, screen lock, or idle handling.
- X11 windows have no title bars, and X11 and Wayland programs do not share a clipboard.
- Notifications keep no history and show no buttons for
  their actions; the launcher finds applications only.

## Origin

The starting point was `smallvil`, the minimal example compositor of Smithay
0.7.0, used under its MIT license (`licenses/SMITHAY-MIT.txt`).
