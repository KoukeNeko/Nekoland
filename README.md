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
xkbcommon, Cairo, Pango and gdk-pixbuf, GTK 4 and libadwaita for the settings, and at
run time `Xwayland` for X11 programs, `swaylock` to lock the screen, and a D-Bus
session bus for the menu bar.

With no configuration it starts a terminal. `-c COMMAND` starts something else.

## Configuration

`~/.config/nekoland/config.toml`; see `shell/config.toml` for one that adds a
wallpaper and an input method. Every key is optional:

| Key | Meaning |
| --- | --- |
| `mode` | Display mode as `WIDTHxHEIGHT`; the display's preferred one otherwise |
| `scale` | How many pixels each point is drawn with: 2 on a display dense enough to be Retina (4K desktop displays, high-resolution laptop panels), 1 otherwise, and the host's own scale when running in a window |
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
| `glass_opacity`, `glass_blur` | How solid the tint over glass is, from 0 to 1, for menus, notifications and the Dock's popups, with bars 0.3 clearer; and how far glass spreads what is behind it, from 0, none, to 1: 0.8 and 0.5 unless set |
| `dock_magnify`, `dock_magnify_scale` | Whether the Dock's icons grow as the pointer passes over them, and how big the one under it grows: on, and 1.6 times, from 1 to 2, unless set |
| `natural_scroll`, `tap_to_click`, `pointer_speed` | For mice and touchpads: off, on, and 0, from -1 to 1, unless set |
| `keyboard_layout`, `keyboard_variant`, `keyboard_options` | The XKB layout, variant and options keys are read with, as setxkbmap takes them; `us` and none unless set |
| `repeat_delay`, `repeat_rate` | Milliseconds a held key waits before it repeats, and how many times a second it then repeats; 200 and 25 unless set |

The Settings application changes most of these while Nekoland runs (see below);
`nekoland msg reload` reads a file changed by hand.

The example's wallpaper is swaybg's; it also starts Fcitx5 for input methods.

## Dock, notifications and launcher

- The Dock has a button for all applications, the ones kept in it, after a line the
  others with windows open, and after another the kept folders, the minimized windows,
  each a live picture to click back, and the Trash. Icons are trimmed of the margins their theme draws and shown
  at one size, as the Mac's are. A dot marks those that run, a longer one the active
  one. As the pointer passes over the icons they grow, the nearest most, and the Dock
  widens for them, as the Mac's magnification does; with that off, pointing at an icon
  lifts it. Either way it is named; a click starts the application, which breathes until its window shows, or brings its windows forward, from another desktop
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
- Settings, `nekoland-settings` from `ui/`, opened from the system menu, is a GTK and
  libadwaita application, as GNOME's settings are, so it looks as the system's GTK style
  says. Its desktop page has a group each for the applications, with their four ways to
  be shown, the Dock, the menu bar and the wallpaper; its other pages set light or dark,
  the keyboard, the mouse and touchpad, the sound and what starts at login, show what the
  computer is, and show GTK's own widgets as the system style draws them. Each change
  takes effect at once. The sound page picks the output and input, and the sample rate and
  bit depth each sound card is opened with, apart from the others: a WirePlumber rule of
  its own in `~/.config/wireplumber/wireplumber.conf.d/`, which restarts WirePlumber and so
  stops the sound for a moment. Copy `shell/io.github.koukeneko.Nekoland.Settings.desktop` to
  `~/.local/share/applications/` to find it among the applications.
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

- The cat on the left opens the system menu: Settings, sleep, restart, shut down, lock
  the screen and log out. Restart, shut down and log out ask first, in a dialog (`nekoland-confirm`, from `ui/`).
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
cat's-eye stone. What `glass` names draws its own colour on top, so it has to be
translucent for the glass to show; the styles in `shell/` are. Nekoland's own surfaces
take a tint, as solid as `glass_opacity` says for menus, notifications, the Dock's popups
and GTK's menus in Nekoland's look, and 0.3 clearer for the menu bar, the Dock and the
overview's bar, the same light and dark; Settings' 外觀 page sets it and the blur. Glass
behind a bar that spans the whole output has square corners.

Behind every window is glass of what is under it, blurred, as Windows' Acrylic is: what a
window leaves see-through, as sidebars and menus in Nekoland's GTK look, shows the windows
behind it and the wallpaper. Each window has a blur as large as itself, drawn again only
when what changes under it is near enough to show. An application's menus float over the
windows on the same glass.

GTK 4's menus are as the Mac's: a submenu opens beside its menu, on the right or, without
room there, on the left, instead of sliding in over it, and no popover has an arrow. GTK
leaves both to each program, so `libnekoland_menus.so`, from `menus/`, is loaded into what
Nekoland starts through `LD_PRELOAD`, from beside the `nekoland` binary, and changes each
popover as it is made. It reaches programs linked with GTK 4.14 or later, not those that
load GTK while running, as Python and GJS programs do; others are left as they are.

Text is drawn as the Mac draws it on a dense display: unhinted, with grey edges, and a
little heavier than regular, where the Mac thickens every outline. Nekoland's own text,
and GTK's in Nekoland's look, is Ubuntu Sans at weight 450; Noto Sans CJK, a CFF font, is
thickened by 0.4px by FreeType, through `FREETYPE_PROPERTIES` in what Nekoland starts,
unless that is set already.

## Screen lock and capture

The menu, Super+L and `nekoland msg lock` lock the screen, and so does sleeping and,
when Settings' 鎖定螢幕 says so, or `lock_after` in seconds, a time without input, unless a
program such as a video player keeps the screen awake. A locker program locks it through
`ext-session-lock`, swaylock in Nekoland's dark paper unless `lock` names another; while
it is locked only the locker is drawn and takes keys, except for switching virtual
terminals and quitting. A locker that goes away leaves the screen locked, and a key
starts it again.

Screenshots and screen sharing go through `wlr-screencopy`: `grim` takes pictures, and
`xdg-desktop-portal-wlr`, which `shell/nekoland-portals.conf` picks, takes them and shares
the screen for programs that ask the portal, such as browsers. Copy and paste works
between X11 and Wayland programs.

Programs in a sandbox such as Flatpak's connect through `security-context-v1` and are
kept from what reaches other programs: input methods, virtual keyboards, the clipboard
manager protocol, other programs' windows, screen capture and the lock.

## GTK applications

Nekoland's design language reaches GTK 4 and libadwaita applications through the system's
style, as a Mac's controls change when the system does, without changing the applications.
`shell/gtk/nekoland.css` sets libadwaita's colours to Nekoland's paper and ink, with what
is selected in solid ink, light and dark, and windows to 14px corners; draws GTK's
buttons, switches, sliders, fields, segmented controls, menus, calendars, tabs, sidebar
selection, dialogs, toasts and tooltips as the design language has them; and leaves sidebars see-through, on the glass Nekoland
draws behind every window. Settings' 設計語言 page shows all of it, after the design,
in this look whether or not it is turned on. Settings' 外觀 page
turns it on: it puts the file in `~/.config/gtk-4.0/nekoland/` and imports it from
`~/.config/gtk-4.0/gtk.css`, keeping what else that file has, and keeps it up to date with Settings.
It sets their text in Ubuntu Sans and Noto Sans CJK TC, as Nekoland's own is set.
It also gives windows minimize, maximize and close at the right of the title bar, through
GNOME's `button-layout`, and puts back the buttons they had when it is turned off. GTK reads
that file as an application starts, so others take the change when opened again;
Settings shows it at once, and turned off it opens again without it. It needs GTK 4.20
or later, the first to read CSS variables and `prefers-color-scheme` in a stylesheet.

`nekoland-calendar`, from `ui/`, is a small calendar of GTK's and libadwaita's own widgets
with no styling of its own: a month, the chosen day's events, kept in
`~/.local/share/nekoland/calendar.tsv`, a search, a menu and a dialog to add one. It is
there to try the look on an application that knows nothing of it;
`shell/io.github.koukeneko.Nekoland.Calendar.desktop` puts it among the applications.

It reaches what libadwaita draws from its variables: windows, header bars, sidebars,
cards, popovers, dialogs and the accent. Not GTK 3 or Qt programs, Flatpak applications,
which use their own runtime's libraries and do not read the host's `gtk.css`, nor what an
application draws itself.

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
| Super+L | Lock the screen |
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

`nekoland msg WORD` talks to the running compositor: `overview`, `lock`, `workspace-add`,
`workspace-next`, `workspace-previous`, `quit`; `reload` reads the configuration again,
`config` prints the settings in effect, `modes` the display's mode and then all it has,
and `'set KEY = VALUE'` changes one setting in the file, keeping the rest of it, and puts
it in effect, or says why not.

## Not there yet

- In the menu bar, menus taller than the screen do not scroll, the Help search takes
  typed letters but not an input method's, the sound menu steps the volume instead of
  sliding it, and the network menu shows the connection without choosing one.
  GNOME's own programs export no menus, so they get the menus Nekoland makes.
- Settings does not change the display's mode or the keyboard. A wallpaper started by
  `autostart` stays under one chosen in Settings until the session ends.
- One GPU. A second display takes the pointer and windows, but the menu bar, Dock and
  overview stay on the first, and displays are not hot-plugged.
- X11 programs are drawn at one pixel a point and enlarged on a scaled display, so
  they look soft there.
- Screen capture copies through the CPU, not dmabufs, and the screen does not turn off
  when idle.
- X11 windows have no title bars.
- Notifications keep no history and show no buttons for
  their actions; the launcher finds applications only.

## Origin

The starting point was `smallvil`, the minimal example compositor of Smithay
0.7.0, used under its MIT license (`licenses/SMITHAY-MIT.txt`).
