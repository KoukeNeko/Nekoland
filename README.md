# Nekoland

A Wayland compositor written in Rust on [Smithay](https://github.com/Smithay/smithay),
with the pieces of a desktop around it, after the Mac's: a menu bar, a Dock, an
overview of windows and desktops, notifications and a launcher, all of them its own.

Started on a bare console it drives the display itself through DRM/KMS, with input
from libinput. Started inside another Wayland or X11 session it runs as a window
there, which is handy while working on it; in an xrdp session it is the whole session,
for another computer to use over RDP.

## Build and run

```
cargo run
```

It needs the development files of Wayland, libinput, libseat, libudev, GBM, EGL,
xkbcommon, Cairo, Pango and gdk-pixbuf, GTK 4 and libadwaita for the settings, and at
run time `Xwayland` for X11 programs, PAM for `nekoland-lock` to check passwords, and a D-Bus
session bus for the menu bar.

With no configuration it starts a terminal. `-c COMMAND` starts something else.

On an Ubuntu without a desktop, such as Ubuntu Server, `install.sh` does all of it, as the
user who will use Nekoland:

```
curl -fsSL https://raw.githubusercontent.com/KoukeNeko/Nekoland/main/install.sh | sh
```

or `./install.sh` from a checkout. It installs what Nekoland runs with, downloads the
build CI made of that commit for Ubuntu 26.10 on x86-64 or arm64, or installs those packages
and builds it where there is none (or with `NEKOLAND_BUILD=1`), puts its programs in `/usr/local/bin`, its desktop entries and portal files where
GTK and `xdg-desktop-portal` find them, a GRUB theme (`shell/grub`) for the boot menu, shown for five seconds at every boot, and a Plymouth splash (`shell/plymouth`) after it, both drawn at the screen's own size where GRUB boots the computer, the boot waiting at most ten seconds for the network to come up, `shell/config.toml` as the configuration unless
there is one, and a few lines at the end of `~/.profile` (and of `~/.zprofile` when the login shell is zsh) that start Nekoland on logging in
on the first console, tty1, in place of its shell, so that quitting it logs out; its log is
`~/.local/state/nekoland.log`, the one before `nekoland.log.old`. Where netplan leaves the
network to systemd-networkd, as on Ubuntu Server, it hands it to NetworkManager, whose
connections Settings' 網路 page sets; run over SSH, that waits for the next boot, as applying
it at once could cut the connection. Run it again to update. Without a checkout it clones into
`~/.local/src/nekoland`, or `$NEKOLAND_SRC`; with a Rust older than Cargo.toml's
`rust-version`, as on Ubuntu 24.04, it installs a newer one with rustup.

## Configuration

`~/.config/nekoland/config.toml`; see `shell/config.toml` for one that adds a
wallpaper and an input method. Every key is optional:

| Key | Meaning |
| --- | --- |
| `mode` | Display mode as `WIDTHxHEIGHT`, at its fastest rate, or `WIDTHxHEIGHT@HZ`; the display's preferred one otherwise |
| `scale` | How many pixels each point is drawn with: 2 on a display dense enough to be Retina (4K desktop displays, high-resolution laptop panels), 1 otherwise, and the host's own scale when running in a window |
| `rotation` | How far the picture is turned, clockwise, for a display stood on its side: 0 (the default), 90, 180 or 270 |
| `display_scales` | The scale of single displays, by name, as `{ "HDMI-A-1" = 1.5 }`, in place of `scale` for those |
| `mirror_displays` | Whether the displays show the same part of the desktop from its left edge, not side by side; off unless set |
| `display_order` | The displays' names, left to right; the first is the main display, with the menu bar and the Dock. Displays it leaves out follow in the order they were found |
| `show_fps` | Whether the frames drawn each second are counted in the top right corner; off unless set |
| `night_shift`, `night_shift_from`, `night_shift_to`, `night_shift_warmth` | Night Shift, warmer colours through the displays' gamma ramps: `off` (the default), `on`, `sunset` from the sun's setting to its rising where the time zone is, as worked out from its place in `zone1970.tab` (from `night_shift_from` to `night_shift_to` in a zone with none, as UTC), or `scheduled` from `22:00` to `07:00` unless set; and how warm, from 0 to 1, 0.7 unless set |
| `color_profile` | The path of an ICC profile whose calibration curves (its `vcgt` tag, as colord and DisplayCAL write it) the displays' gamma ramps follow, under Night Shift; none unless set |
| `autostart` | Shell commands started once the compositor is up |
| `terminal`, `launcher` | Commands behind the shortcuts below |
| `launcher_search` | What the launcher takes before the text to search for |
| `volume_up`, `volume_down`, `volume_mute`, `brightness_up`, `brightness_down` | Commands behind the media keys |
| `glass` | Other layer surfaces or windows drawn on glass, by namespace or application ID, with the corner radius they draw: `{ nekoland-launcher = 16 }` unless set |
| `dock` | The applications kept in the Dock, by desktop entry name; the Dock writes it back when they are rearranged |
| `stacks` | The folders at the Dock's right end, each `{ path, sort, show, view }`: `sort` is `name`, `added`, `modified`, `created` or `kind`, `show` is `stack` or `folder`, `view` is `fan`, `grid`, `list` or `auto`; the downloads folder unless set, written back by the Dock |
| `apps_view` | How the applications are shown: `overview` (the default), `launchpad`, `shelf` or `sidebar`; see Overview |
| `wallpaper`, `wallpaper_mode` | A picture drawn on the desktop by swaybg, and `fill` (the default), `fit` or `center` |
| `desktop_icons`, `desktop_sort`, `desktop_free` | Whether the files in the Desktop folder show on the desktop, on unless set; their order: `name` (the default), `added`, `modified`, `created` or `kind`; and whether they stay where they are dragged instead, off unless set |
| `hover_info`, `hover_delay`, `hover_style` | Whether resting on a file shows its panel, on unless set; after how many seconds, 0.1 unless set; and `full` (the default) or `tooltip`, only its kind, size and date |
| `hover_follow`, `hover_offset_x`, `hover_offset_y` | Whether the panel moves with the pointer, off unless set, and how far right of and below it, 15 pixels each unless set |
| `hover_auto_hide`, `hover_large_file_protection` | Whether the panel goes as soon as the pointer leaves its file, or stays until it rests on another, and whether text over 8 MB is left unread (256 MB without it); both on unless set |
| `hover_glass`, `hover_opacity` | Whether the panel is on glass, on unless set, or plain and this opaque, 0.7 to 1, 0.98 unless set |
| `hover_width`, `hover_font_size`, `hover_compact` | The panel's width, 300 to 600 pixels, 300 unless set; its text's size, 9 to 14 points, 11 unless set; and whether its spacing is tighter, off unless set |
| `hover_hide` | The parts of the panel left out, of `icon`, `type`, `source`, `dates`, `file`, `location`, `notes` and `tags`; a kind's part, by its name such as `照片`; or one field, as `照片/焦距` or `dates/建立`; `file/屬性修改日期` unless set |
| `clock_24h`, `clock_date`, `clock_weekday`, `clock_seconds` | How the menu bar's clock reads: 24 hours, the date and the weekday unless set, no seconds |
| `clock_flash` | Whether the colon between the clock's hours and minutes flashes each second; off unless set |
| `menu_bar_network`, `menu_bar_sound` | Whether the menu bar shows the network's and the sound's icons; both unless set |
| `menu_bar_vpn` | Whether the menu bar shows the VPN helper's profiles: each turned on and off from its menu, and Settings opened for one that asks for a password; on unless set |
| `menu_bar_battery`, `menu_bar_battery_percent` | Whether the menu bar shows the computer's battery, where it has one, and its percentage beside it; the battery without its percentage unless set |
| `menu_bar_order` | The order of the menu bar's icons, left to right, as dragging one with Ctrl held leaves it: `battery`, `network`, `vpn`, `sound`, and `item:` followed by an application's item's ID; ones not named go on the left |
| `menu_bar_background` | Whether the menu bar has its glass, or shows the desktop through it, its text dark or light as the wallpaper under it is; on unless set |
| `dock_glass` | Whether the Dock is on glass or on its tint alone; on unless set |
| `bar_opacity`, `bar_blur`, `glass_opacity`, `glass_blur` | How solid the tint over glass is, from 0 to 1, and how far its blur spreads what is behind it, from 0, none, to 1: of the menu bar and the Dock themselves, and of all other glass, menus, notifications, the Dock's popups, the overview, and behind windows, as GTK's menus and sidebars show it. 0.5 and 0.8, each blurred 0.5, unless set |
| `dock_size` | How big the Dock's icons are, from 24 to 96, the rest of it growing with them; 40 unless set, written back when the line between its icons is dragged |
| `dock_position` | The edge the Dock is at: `left`, `bottom` or `right`; at the bottom unless set |
| `dock_autohide` | Whether the Dock slides off its edge until the pointer reaches that edge, leaving windows the room; off unless set |
| `dock_magnify`, `dock_magnify_scale` | Whether the Dock's icons grow as the pointer passes over them, and how big the one under it grows: on, and 1.6 times, from 1 to 2, unless set |
| `dock_indicators`, `dock_bounce` | Whether a dot under an icon says its application runs, and whether an icon bounces when clicked: both on unless set |
| `dock_minimize_into_app` | Whether a minimized window goes into its application's icon instead of a tile of its own; off unless set |
| `natural_scroll`, `tap_to_click`, `pointer_speed` | For mice and touchpads: off, on, and 0, from -1 to 1, unless set |
| `scroll_speed` | How far scrolling goes, times what the mouse or touchpad sends: 1 unless set |
| `cursor_size` | How many points across Nekoland draws the cursor: 24 unless set |
| `zoom` | How many times the screen is magnified around the pointer, up to 8; 1 (off) unless set. Super Ctrl = and Super Ctrl - change it by 0.5 |
| `cursor_theme`, `icon_theme` | The cursor and icon themes Nekoland draws with, by folder name; a common one, as `$XCURSOR_THEME`, Adwaita or Yaru, unless set. Icons already drawn change when drawn again |
| `pointer_acceleration`, `left_handed`, `disable_while_typing` | Whether the pointer goes further the faster it moves, the buttons are swapped for the left hand, and a touchpad ignores touches while typing: on, off and on unless set |
| `font` | The family of Nekoland's own text, and of GTK's in Nekoland's look, with Noto Sans CJK TC for what it lacks; `Ubuntu Sans` unless set |
| `keyboard_layout`, `keyboard_variant`, `keyboard_options` | The XKB layout, variant and options keys are read with, as setxkbmap takes them; `us` and none unless set |
| `color_filter` | A filter the displays' colour matrix applies: `grayscale`, or the correction for `protanopia`, `deuteranopia` or `tritanopia`, so that it takes a display that has a colour matrix; `none` unless set |
| `invert_colors` | Whether the displays' colours are inverted, through their gamma ramps; off unless set |
| `slow_keys_ms` | Milliseconds a key is held before it counts; a key held that long is sent when it is let go; 0, which counts every press, unless set |
| `sticky_keys` | Whether a modifier key let go on its own stays on for the next key pressed; off unless set |
| `bounce_keys_ms` | Milliseconds after a key is let go within which pressing it again is ignored; 0, which ignores none, unless set |
| `key_repeat`, `repeat_delay`, `repeat_rate` | Whether a held key repeats, how many milliseconds it waits before it does, and how many times a second it then repeats; on, 200 and 25 unless set |
| `lock_wallpaper`, `lock_blur` | Whether the lock shows the wallpaper rather than dark paper, and blurs it; both on unless set |
| `lock_style` | The lock's layout: `quiet` (the time in the middle, a row of widgets under it), `poster` (the hours over the minutes, bold, on the left) or `dial` (a clock with hands, its widgets on its face); `quiet` unless set |
| `lock_clock_color`, `lock_clock_weight` | The lock's clock as `#rrggbb` or `rainbow` (through the colours), else a colour of the wallpaper's where the style has one; and `light`, `regular` or `bold`, else the style's |
| `lock_clock_text`, `lock_clock_image` | Words and a small picture laid under the lock's clock, as Good Lock's ClockFace has them |
| `lock_dial_numerals`, `lock_dial_hands` | The `dial` style's figures, `none`, `arabic` or `roman`, `none` unless set; and its hands, `classic`, `thin` or `bold`, `classic` unless set |
| `lock_clock_at`, `lock_notices_at`, `lock_message_at` | Where the lock's clock, notifications and message stand instead of where the style puts them: their middle as `[x, y]` shares of the screen |
| `lock_clock_font`, `lock_clock_size`, `lock_clock_align` | The lock's clock's family, `font`'s unless set; `small`, `medium` or `large`, `medium` unless set; and `left`, `center` or `right`, `center` unless set, but the poster's always left |
| `lock_clock_seconds`, `lock_show_date` | Whether the lock's clock shows seconds, off unless set, and the date, on unless set |
| `lock_widgets` | What the lock shows beside the time, of `battery`, `playing` (an MPRIS player's title, with buttons to go back, play or pause and go on), `calendar` (today's next event from the Calendar), `world_clock` and `weather`; `battery` and `playing` unless set |
| `lock_world_clock`, `lock_world_clock_name` | The `world_clock` widget's zone, as `Asia/Tokyo`, and the city it is shown as, else the zone's last part |
| `lock_wallpaper_subject` | A picture of the wallpaper's subject on transparency, framed as the wallpaper is, drawn over the time above the password field, as One UI's depth effect does; best with `lock_blur = false`. Settings › 鎖定畫面 › 前景主體 › 去背 has one cut out of the lock's wallpaper by the services AI 生成桌布 uses: Stability AI takes the background away; the others are asked for a mask of the subject. The picture goes to that service |
| `lock_weather_at`, `lock_weather_name` | The `weather` widget's place, as `[latitude, longitude]`, and the city it is shown as. The place is sent to MET Norway (api.met.no) every half hour while locked |
| `lock_notices` | The notifications that came in while locked: `hidden`, `count` (how many from each program) or `full`; `count` unless set |
| `lock_message`, `lock_show_name` | Words the lock shows, such as whom to call; and whether it shows the user's name and picture (`~/.face`), on unless set |
| `lock_message_size`, `lock_message_color`, `lock_message_opacity` | The message's size in points, 10 to 40, 13 unless set; its colour as `#rrggbb`; and how opaque it is, 10 to 100 percent, 100 unless set |
| `lock_lock_icon`, `lock_lock_icon_file`, `lock_lock_icon_color` | The sign at the top of the lock: `none`, `padlock`, `shield`, `key`, `heart`, `paw` or `picture` (the file's), `none` unless set, or `padlock` where the older `lock_show_lock_icon` is on; and its colour as `#rrggbb` |
| `lock_power_buttons` | Whether the lock has sleep, restart and power off buttons at its foot; on unless set |
| `lock_shortcuts`, `lock_shortcuts_layout`, `lock_shortcuts_unlocked` | Apps the lock opens, by desktop entry ID: two in the corners at its foot (`corners`) or six in a row (`row`), or six up the right edge (`column`), `corners` unless set; and whether they open over the lock without the password, off unless set |
| `lock_wallpaper_folder` | A folder whose pictures take turns as the lock's, in order of their names, the next each time the screen locks, as One UI's Dynamic Lock Screen; `lock_wallpaper_file` comes first, and `lock_wallpaper_subject` is not drawn over them |
| `lock_wallpaper_file`, `lock_dim` | A picture of the lock's own, the desktop's unless set; and how much darker the lock makes it, 0 to 80 percent, 0 unless set. Settings › 鎖定畫面 › AI 生成桌布 has one made from a description by OpenAI, Google Gemini or Stability AI with the person's API key, or by ChatGPT through the Codex CLI signed in with ChatGPT; the description goes to that service, and the keys are kept in `~/.config/nekoland/image-services`, readable by its owner alone |
| `lock_stickers` | Pictures placed on the lock, as `[{ file = "…", at = [x, y], size = 160 }]`: each one's centre as shares of the screen, `[0.78, 0.3]` unless set, and its longer side in points, 160 unless set. The older `lock_sticker`, `lock_sticker_at` and `lock_sticker_size` give one more |
| `lock_unlock_effect`, `lock_unlock_duration` | How the lock goes once the password is given: `lift`, `fade`, `slide`, `expand`, `spread`, `wave`, `warp`, `ripple`, `mosaic`, `wiper`, `paint`, `glitch` or `swirl`, `lift` unless set; and in how many milliseconds, 560 unless set |
| `lock_unlock_direction`, `lock_unlock_easing`, `lock_unlock_color` | The way `slide`, `wave`, `wiper` (`left` and `up` turn it the other way round), `paint` and `glitch` go: `left`, `right`, `up` or `down`, `right` unless set but `left` for `slide`; the pace, `out`, `inout` or `linear`, `out` unless set; and the colour of `spread`'s edge, `ripple`'s rings, `paint`'s brush and `glitch`'s bands as `#rrggbb`, the wallpaper's unless set |
| `lock_screen_off_after` | Seconds without input while locked after which the displays turn off, counted from locking at the soonest; as `screen_off_after` unless set |
| `lock_always_on` | Whether the locked displays dim to the time on black instead of turning off, after `lock_screen_off_after` (30 seconds unless set) |
| `lock_always_on_widgets`, `lock_always_on_stickers`, `lock_always_on_brightness` | Whether the always-on display also shows the widgets and the stickers, both off unless set; and how bright it is, 20 to 100 percent, 100 unless set |
| PIN | Settings › 使用者 › PIN sets six digits or more that unlock the lock screen as the password does, kept hashed (SHA-512 crypt, a million rounds) in `~/.local/share/nekoland/pin` for the user alone; five wrong ones in a lock leave only the password. Off until set |
| `lock_fingerprint` | Whether a finger enrolled with fprintd unlocks, as the password does (on unless set). Settings › 鎖定畫面 › 指紋 enrolls and removes fingers |
| `lock_fingerprint_effect` | What a finger on the reader shows around the fingerprint: `ripple`, `glow` or `none`, `ripple` unless set; in the wallpaper's colour when the reader knew it, red when not |
| `screen_off_after`, `sleep_after` | Seconds without input after which the displays turn off, and after which the computer sleeps, unless a program keeps the screen awake; never unless set |
| `power_button`, `lid_close` | What the power button and closing the lid do: `shutdown`, which asks first, `sleep`, `lock` or `ignore`; `shutdown` and `sleep` unless set |
| `notification_seconds` | How many seconds a notification's banner stays when its program leaves that to Nekoland: 5 unless set |
| `notification_previews` | Whether a notification's banner shows its text, or its title alone; its text unless set |
| `quiet_scheduled`, `quiet_from`, `quiet_to` | Whether Do Not Disturb also holds each day from `quiet_from` to `quiet_to`, `22:00` to `07:00` unless set; off unless set |
| `notifications_off` | The programs, by the name they send notifications under, whose notifications never show, as banners or in the Notification Center; Settings' 通知 lists those that have sent one, kept in `~/.local/state/nekoland/notifiers` |
| `focus_follows_pointer` | Whether the keyboard goes to the window the pointer moves into, without raising it, instead of the one clicked; off unless set |
| `edge_tiling` | Whether a window dragged to the display's edge fills the part that edge stands for, shown under it while it is held there: a side its half, a corner its quarter, the top all of it, the bottom the third the pointer is over; on unless set |
| `window_gap` | Points left between windows put side by side, and around them, up to 64; none unless set |
| `animations` | Whether windows, desktops and menus move to where they go, as under Motion below, rather than jump there; on unless set |
| `hot_corner_top_left`, `hot_corner_top_right`, `hot_corner_bottom_left`, `hot_corner_bottom_right` | What pointing into that corner of a display does: `none` (the default), `overview`, `apps` or `lock` |

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
- The line before the kept folders, and the one before the running applications, is
  dragged up or down, or across at a side, to size the Dock, as on the Mac; its right
  click turns magnification and hiding on or off, moves the Dock to the left, bottom or
  right edge, and opens Settings. At a side the Dock runs from the top down, what opens
  from it opens beside it, and folders open as a grid.
- A kept folder shows as its newest items piled up or as its own icon, and opens as a
  fan rising from it, a grid of pages to scroll, or a list with submenus for folders; a
  right click sets its order, how it shows and how it opens. A folder dragged in from a
  file manager is kept there, and folders are dragged to reorder or take them off. Files
  dropped on the Trash go to it, and files dropped on an application open in it; the
  Trash is emptied from its right-click menu after asking.
- Notifications are shown by Nekoland, which serves `org.freedesktop.Notifications`:
  banners on glass under the menu bar, which go when their time is up unless pointed
  at; a click does what the program offers for it. What is not transient is kept after
  its banner for the Notification Center, the newest 50 of them, until Nekoland
  restarts; one its program closes or replaces goes from there too.
- The Notification Center, as the Mac's, opens from the bell or the clock in the menu bar, on the
  menus' glass where the banners stand, and closes on a second click, a click outside it
  or Escape. Each program's notifications are stacked under its newest, saying how many,
  and spread out by a click; a notification shows its program's name, when it came, its
  title and its text. A click on one does what its banner would and takes it away; the ×
  on one or on a stack under the pointer, 清除 over a program's spread out and 清除全部
  clear them. It scrolls when taller than the screen.
- Settings, `nekoland-settings` from `ui/`, opened from the system menu, is a GTK and
  libadwaita application, as GNOME's settings are, so it looks as the system's GTK style
  says. Its desktop page has a group each for the applications, with their four ways to
  be shown, the Dock, the menu bar, how long notifications stay, the hot corners and the
  wallpaper; its windows page sets what clicking a GTK title bar does, edge tiling, the gap
  between tiled windows and focus following the pointer, and lists the shortcuts that arrange
  windows; its other pages set light or dark, the icon and
  cursor themes, the displays as the Mac's Displays (their order side by side, and for the main one its name from its EDID, the sizes it can look like, its brightness, refresh rate, rotation, a frame counter, a colour profile, and Night Shift with its sunset schedule), the
  fonts, the keyboard, the mouse and touchpad, the sound, the power, which sandboxed applications, as
  Flatpak's, may use the camera, the microphone, the speakers, the location, screenshots, background running and notifications, through the portals'
  permission store, the cursor size, high contrast and reduced motion (GTK's and, as `animations`, Nekoland's), the applications
  that open links and each kind of file, through GIO's defaults, and the terminal,
  whether GTK pastes with the middle button, blinks its text cursor and remembers recent
  files, what starts at login, and whether other computers may come in over RDP and
  which session they get, show what the computer is and how worn its batteries are, and
  show GTK's own widgets as the system style draws them.
  Its Wi-Fi and 網路 pages work through NetworkManager's `nmcli`: 網路 joins and leaves
  each Ethernet port and shows its IPv4 and IPv6 addresses; a connection's details, a
  Wi-Fi network's among them, set whether it comes up on its own, 低數據模式 (marked
  metered), IPv4 (from DHCP, by hand, or off, with DHCP's lease renewed), IPv6, DNS servers
  and search domains, the MTU, and for Wi-Fi the private address, and copy a kept
  network's password. A port netplan gives to systemd-networkd, as on Ubuntu Server,
  has 改用 NetworkManager, which runs `nekoland-hand-over-network` as the installer does.
  The VPN page has the OpenVPN and WireGuard profiles of Nekoland's VPN helper,
  `nekoland-vpnd.service` (Plaitway's daemon, built from `third_party/plaitway`), through
  its `nekoland-vpn` command: each turned on and off, dragged into the order that
  decides which holds a route, set, edited as text, removed, with its traffic, routes and
  log, and the helper's routing and diagnostics; the passwords profiles ask for are asked
  while Settings is open and kept in the keyring, under the Plaitway app's items. The
  installer moves an installed Plaitway's profiles into it once and turns Plaitway's helper
  off. Below them, NetworkManager's VPNs are turned on and off and removed; without the
  helper, a WireGuard `.conf` or OpenVPN `.ovpn` is added to NetworkManager there.
  Each change takes effect at once. The fonts page sets `font` and GNOME's interface font together,
  and the monospace font, text size and antialiasing that GNOME's settings give GTK and
  others. The sound page picks the output and input, and the sample rate and
  bit depth each sound card is opened with, apart from the others: a WirePlumber rule of
  its own in `~/.config/wireplumber/wireplumber.conf.d/`, which restarts WirePlumber and so
  stops the sound for a moment. It also makes a multi-output device that plays on the
  outputs chosen at once, as the Mac's does, through PipeWire's combine stream in
  `~/.config/pipewire/pipewire.conf.d/`, which restarts PipeWire; and lets PipeWire run at
  the sample rate of what plays, from 44.1 to 192 kHz, instead of always at 48. Its
  裝置模式 group sets each card's profile, as stereo, surround, Pro Audio or off.
  What is the system's, not Nekoland's, Settings changes as root through `sudo -A`, asking
  for the password with Nekoland's askpass: 使用者 changes the full name (`usermod`), the
  password (`chpasswd`) and the picture (`~/.face`), whether the first console logs in by
  itself (a getty drop-in), and adds, makes administrators of and removes other people;
  語言與地區 sets the language and the formats through `localectl`, shows a date, a time, a
  number and the currency as they then read, and adds a language with `locale-gen`; 印表機
  lists CUPS's printers, network ones it finds among them, sets the person's default with
  `lpoptions`, prints a test page, cancels jobs, and adds a printer by its address as IPP
  Everywhere; 軟體更新 counts what apt can update, opens Market's 更新, and turns
  unattended-upgrades on and off; 關於 renames the computer through `hostnamectl`. 通知 sets
  Do Not Disturb's daily hours, how long banners stay, and turns each program's off; the
  power page sets power-profiles-daemon's 電源模式, and 協助工具 turns Orca on and sets 螢幕縮放. 備份 copies the
  home folder with rsync to a folder elsewhere, a dated folder each time with unchanged files
  hard-linked to the last, now or daily through a systemd user timer
  (`nekoland-settings --backup`). Windows App installs Wine and keeps bottles of it, each a
  prefix with its own Windows folders and programs: it runs a Windows program in the one
  chosen, turns DXVK and VKD3D-Proton (Direct3D 8 to 12 on Vulkan) on and off and installs fonts and runtimes there through winetricks,
  has its programs open their windows on Wayland itself rather than through XWayland
  (Wine's Wayland driver), sets the Windows it says it is, and opens Wine's settings, registry editor, task manager
  and uninstaller for it; a bottle made there can run with the newest of Kron4ek's builds
  of Wine, which the page downloads, in place of Ubuntu's, and does unless told otherwise, and which uses the kernel's
  ntsync that `install.sh` loads at boot (Ubuntu's Wine 10.0 is too old for it). A bottle follows Nekoland's
  appearance: dark, it goes without Wine's theme, in Nekoland's colours or libadwaita's,
  and its programs are told Windows is dark; its DPI is set to X11's as that changes
  (`nekoland-settings --wine-dpi`), so that Wine draws at the display's scale; a Windows program opened from Files
  (`nekoland-settings --wine`) is given the look first. It also
  installs Windows 11 Enterprise's
  90-day evaluation in a dockur/windows container under rootless podman with KVM, and adds
  a program of it as an application that opens as its own window through FreeRDP's
  RemoteApp (`nekoland-settings --windows-app`), at the display's scale: the page has Windows list the programs in
  its Start menu, with their icons and the types of file they open, so that such a file
  opens in Windows from Files. One connection is kept for all of them, in which an agent
  of Nekoland's (`ui/src/windows/agent.ps1`) starts the programs and tells their windows
  apart, so that each is its own application in the Dock under Nekoland's title bar.
  Microsoft Store and what is installed from it are among the programs listed, known by
  their names there (`shell:AppsFolder\…`) rather than by paths that change as they update.
  A `.exe` or `.msi` opened in Files runs with Wine. Files' right-click menu has a
  Windows submenu (`nekoland-settings --windows-menu`) with what Windows' programs add to
  the file's menu there, as Windows last listed it for files of the kind, and the
  programs installed in Wine's bottles to open the file with, and what those add to the
  menu themselves, as 7-Zip does, which a Windows program built with MinGW
  (`ui/src/windows/shellmenu.c`) asks them for. Copy `shell/io.github.koukeneko.Nekoland.Settings.desktop` to
  `~/.local/share/applications/` to find it among the applications.
- The launcher, `nekoland-launcher` from `ui/`, opens with Alt+Space and closes with it
  again or when anything else is clicked. It finds applications by name, in the user's
  language too, its own commands, and files in the home folder, and takes an input
  method. It also answers sums, percentages and unit conversions (`5 km to mi`,
  `3 坪 換成 m2`) and offers web, file and menu-item searches for what was typed. Its
  commands lock, sleep, restart and shut down, toggle dark mode, put the window under it
  in a half, a quarter, a third, a fourth or a sixth of the screen, larger, smaller or against an edge, and open views of their own: clipboard
  history, emoji, files, open windows, snippets, quicklinks and notes. Ctrl+K lists what
  can be done with the chosen item, Ctrl+Return does the second of them, and an item
  can be given an alias to find it by. Snippets paste text with `{date}`, `{time}` and
  `{clipboard}` filled in; a quicklink opens its address with what follows its keyword
  (`g nekoland`) in place of `{query}`. They and the notes are kept in
  `$XDG_DATA_HOME/nekoland/launcher.json`, and the clipboard's history, the last 300
  texts copied and none a password manager marks secret, in `clipboard.json` beside it,
  which `wl-paste --watch` adds to while Nekoland runs. `nekoland-launcher --open=VIEW`
  opens a view directly: `clipboard`, `emoji`, `files`, `windows`, `snippets`,
  `quicklinks` or `notes`.

Programs learn whether to be dark from `xdg-desktop-portal`. With
`xdg-desktop-portal-gtk` installed, copy `shell/nekoland-portals.conf` to
`~/.config/xdg-desktop-portal/`; the GSetting
`org.gnome.desktop.interface color-scheme` then switches them. The portal runs in a
graphical session only: Nekoland starts systemd's `graphical-session.target` through
`nekoland-session.target`, which `install.sh` puts in `/usr/local/lib/systemd/user`, and
stops it as it ends, so the person's units wanted by that target run with it too.

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
- The Window menu, from Nekoland: minimize, zoom, move and resize, move to another desktop,
  and the program's windows. The Help menu searches every other menu as you type.
- On the right: the status items programs register as StatusNotifierItems, with their
  menus; the sound (through `wpctl`, when PipeWire runs) and the network; the control
  center; the clock; and at the very end the bell. The clock and the bell each open the
  Notification Center, and the bell is lit while it is open.
- The control center, as the Mac's, opens a panel on the menus' glass. At its top, what is
  recording, in the colours of the bar's dots. Then tiles: the network, whose list of
  Wi-Fi networks joins one or turns Wi-Fi on or off through `nmcli`; Bluetooth, whether
  there is an adapter (it lists no devices yet); Do Not Disturb, which keeps notifications
  from coming up (they are kept in the Notification Center); dark, which sets the GSetting
  `org.gnome.desktop.interface color-scheme`; Night Shift, warm gamma ramps on displays
  that take them (not in a window); and a screenshot of the whole screen into Pictures,
  through `grim`. Under them, the display's brightness, through `brightnessctl`, when
  there is a backlight; the volume and the microphone, each icon muting it; the sound
  output and input, lists of PipeWire's to choose the default from; and what plays, from
  the first MPRIS player, with its cover and buttons. A tile that is on is solid ink.

A click opens a menu; then moving across the bar opens the others without clicking. A
press dragged onto an entry and let go chooses it. Submenus open beside their entry,
and stay open while the pointer heads into them. Ctrl+F2 opens the first menu for the
keyboard: the arrows move, also into and out of submenus, Return chooses, Escape
closes. It follows the desktop's dark or light setting.

## Desktop

The files in the Desktop folder lie on the wallpaper, under the windows, in columns from
the top right corner down, as on the Mac: a click chooses one, a double click opens it.
Shift or Ctrl and a click adds to the choice, and dragging over the desktop draws a box
that chooses what it touches. What is chosen drags as files: into a folder on the
desktop, onto the Dock (the Trash, an application, a kept folder) or into a window.

With the desktop chosen, the arrows and a typed letter move the choice; Return or F2
renames in place (the name without its extension is chosen, and the input method
types into it); Super O opens, Delete or Super Backspace puts in the Trash, Super I
shows the file's properties in Files, and Super Shift N makes a new folder. Super C
copies the files for pasting in Files or a window, Super V pastes what was copied or
cut there into the Desktop folder, Super D makes a copy beside each, and Super A, with
something chosen, chooses everything; Ctrl does the same as Super. Space or Super Y shows the last
file chosen in Quick Look, a panel over the windows with the picture, the start of the
text or the kind, size and date, which follows the arrows and closes with Space or
Escape. A right click on a file opens its menu, with the same and the applications that
open it; on the desktop itself, to make a new folder, paste, sort the files or change
the wallpaper. Sorting by 無 keeps each file where it is dragged to, in the cell nearest,
remembered in `desktop-places.toml` beside the configuration.

Resting the pointer on a file on the desktop, in a stack opened from the Dock or in a
Nautilus window, shows
what is known of it on glass, as FinderHover does on the Mac: by kind, a photo's camera,
lens and exposure, a video's or song's streams, a document's pages, an archive's or
disk image's contents, a font's glyphs, a folder's git branch and changes, a program's
architecture, the lines of code, and so on; then where it was downloaded from, its
dates, its details, where it is with how full the disk is, its tags and a note, kept in
the `user.xdg.comment` attribute. Clicks go through the panel to what is under it;
holding Alt keeps it still to click a value and copy it, or the note to type in it. What is read comes from exiftool, ffprobe,
pdfinfo, bsdtar, sqlite3, git and file, and a font's sample from HarfBuzz's hb-view, each
left out when it is not installed. Which file in Nautilus is pointed at is read from its
accessibility tree, through at-spi2-core. Settings lists each kind's fields, beside the
panel as it would show, to leave any of them out.

## Overview

Super, or the overview's key, spreads the desktop's windows apart where they are, each
named under it, as Mission Control does. Where the menu bar was, a strip of the same
glass names the desktops, the current one marked, with a button at the right end that
adds one. Pointing at the strip grows it down, and the windows move down to make room:
each desktop is shown small, wallpaper and windows as they are, and the add button is a
smaller one of the wallpaper alone, under a plus. A desktop added comes out of that
button to the end of the row as the others move over for it. A click on a desktop
goes to it and leaves the overview. A window dragged onto a desktop goes there, onto the
add button to a new desktop; the bar opens up for it. A desktop pointed at shows a button at its corner
that takes it away, its windows going to the desktop before it.

Super+A, the Dock's first button, or typing in the overview shows the applications in
place of the windows, as GNOME does: the desktops' bar stays open above them, a search
field under it, and every application a page at a time, turned with the wheel, the
arrows or the dots. Typing narrows them down by name, keyword or command, and Return
starts the first. A click starts one on the desktop shown; one dragged onto a desktop
starts there, onto the add button on a new desktop, and onto the Dock is kept in it,
where the Dock makes room for it. The windows stay where they are behind the
applications, blurred by the glass these are on.

The search field and a folder's name take an input method: with Fcitx5 running, what
is being composed shows underlined in the field and its candidates in a row under it,
as Nekoland draws them.

`apps_view` shows them other ways instead, without the desktops' bar: `launchpad`,
pages of icons over the whole screen with the search field above; `shelf`, one long
page, the latest started first and the rest under the name of their kind; `sidebar`,
the kinds in a list on glass at the left, with the latest started, and the icons of
the one chosen beside it. On the long page the wheel scrolls, the arrows move from one
application to the next and Return starts it. On a display with room for more than
seven by five icons, the pages are shown larger to fill it, icons, names and all, as
Launchpad is.

The applications are arranged by dragging, as on the Mac's Launchpad: one dropped
between others goes there, the others making room; onto another, the two make a folder
named after its kind; onto a folder, it goes in. A dragged icon held at the grid's side
turns the page. A folder opens under its row, the rows parting for it, in as many rows as there is room for over the Dock and pages turned by scrolling for the rest; its applications are reordered there or
dragged out, and a click on its name changes it. The arrangement is kept in
`app-grid.toml` beside `config.toml`. A right click on the grid opens a menu to arrange
it or put it back as it was, except on an application that came with the system:
Nekoland's own, or one that runs in a terminal.

## Motion

What is on screen moves to where it goes on springs, as iOS moves what it shows, and it is
Nekoland that moves it, so a program has it whatever drew its window: GTK, Qt or X11.

- The applications, however `apps_view` shows them, come in from a little larger than
  they are, as a phone's icons come home, and leave the way they came.
- On their pages each icon is a picture the GPU keeps, so that none of this is drawn
  again as it moves: an icon grows under the pointer and gives under a press; a dragged
  one comes to the pointer from its place and flies home from where it is let go; the
  others glide apart for it and close up behind it; the pages slide a screen apart; and
  the icons shake, each turned a little, while they are arranged. The icons are drawn
  ahead of time, a few a frame while nothing is shown, so showing them waits for none.
- A desktop picked in the overview is gone to as the overview closes, without the
  desktops sliding past each other as well.
- A new window grows a little as it fades in, as the Mac's do, and so does one brought
  back from being minimized other than from the Dock. One whose application was started
  from its icon among the applications opens out of that icon.
- A window that closes leaves its picture where it was for a moment, shrinking as it fades.
- A window maximized, made fullscreen, put in a half or another part of the display, or
  put back, glides from where it was to where it goes, stretched until its program has
  drawn it at the new size.
- Going to another desktop slides the one left out to the side as the other slides in.
- A menu or a popover, an application's own among them, grows out of what it was opened
  from, and its glass with it.

A window minimized is drawn into the Dock as before, and the overview spreads the windows
apart. Inside GTK programs, Nekoland's look gives what is pressed its own spring; see GTK
applications. `animations = false`, or 減少動態效果 in Settings' 協助工具, which also sets
GNOME's `enable-animations` for what GTK moves itself, puts everything where it goes at once.

## Glass

Nekoland's glass is called Catseye（貓眼）. The menu bar and its menus, the Dock,
notifications, the overview's desktops and what `glass` names are drawn over a blur of what is behind it, a little more saturated,
with a thin line of light just inside its top edge, like the band of light across a
cat's-eye stone. What `glass` names draws its own colour on top, so it has to be
translucent for the glass to show; the styles in `shell/` are. Nekoland's own surfaces
take a tint, the same light and dark. The menu bar and the Dock have a tint and blur of
their own; all other glass, GTK's menus and sidebars among it, shares one, and Settings'
外觀 page sets both. Glass behind a bar that spans the whole output has square corners.

Behind every window is glass of what is under it, blurred, as Windows' Acrylic is: what a
window leaves see-through, as sidebars and menus in Nekoland's GTK look, shows the windows
behind it and the wallpaper. Each window has a blur as large as itself, drawn again only
when what changes under it is near enough to show. An application's menus float over the
windows on the same glass.

GTK 4's menus are as the Mac's: a submenu opens beside its menu, on the right or, without
room there, on the left, instead of sliding in over it, and no popover has an arrow. GTK
leaves both to each program, so `libnekoland_menus.so`, from `menus/`, is loaded into what
Nekoland starts through `LD_PRELOAD`, from beside the `nekoland` binary, and changes each
popover as it is made. A button a program opens a popover from itself, as a browser's site
menu, is solid ink while the popover shows, as GTK's own menu button is: the library marks
it, a plain button having no state a stylesheet could go by. It reaches programs linked with GTK 4.14 or later, not those that
load GTK while running, as Python and GJS programs do; others are left as they are.

Text is drawn as the Mac draws it on a dense display: unhinted, with grey edges, and
heavier than regular, where the Mac thickens every outline. Nekoland's own text, and
GTK's in Nekoland's look, is Ubuntu Sans, or the family `font` names, at Medium where it
names no weight; Noto Sans CJK, a CFF font, is thickened by 0.4px by FreeType, through
`FREETYPE_PROPERTIES` in what Nekoland starts, unless that is set already.

On a computer whose GPUs have no render node, such as a Hyper-V VM, GTK 4 programs that
Nekoland or D-Bus starts draw with Cairo, through `GSK_RENDERER=cairo`, unless that is set
already: GTK's OpenGL on llvmpipe costs each of them some 150 MB more.

## Screen lock and capture

The menu, Super+L and `nekoland msg lock` lock the screen, and so does sleeping and,
when Settings' 鎖定畫面 says so, or `lock_after` in seconds, a time without input, unless a
program such as a video player keeps the screen awake. A locker program locks it through
`ext-session-lock`, `nekoland-lock` unless `lock` names another, such as swaylock; while
it is locked only the locker is drawn and takes keys, except for switching virtual
terminals and quitting. A locker that goes away leaves the screen locked, and a key
starts it again.

`nekoland-lock` shows the wallpaper a little nearer and blurred, under ink, with the time,
the date, the user's name and a field for the password, in one of three styles — 靜, 海報
and 錶面 — with the clock's colour and weight, the widgets (the battery and what plays),
the notifications that came in while locked and a message as Settings' 鎖定畫面 makes
them, over a preview `nekoland-lock --preview FILE WIDTH HEIGHT` draws. The compositor
keeps those notifications in `$XDG_RUNTIME_DIR/nekoland-lock-notices` for it, emptied as
the screen locks. PAM checks the password through the
`nekoland-lock` service, `shell/nekoland-lock.pam` in `/etc/pam.d`. Enter checks it,
Escape clears it; a wrong one shakes the field red. The right one lifts the ink off the
wallpaper before the desktop shows.

Nekoland takes the power button and the lid from logind, through an inhibitor lock, and
does what `power_button` and `lid_close` say; closing the lid does nothing while another
display is connected. A time without input can also turn the displays off, until the next
input, and put the computer to sleep, locked as it wakes.

Screenshots and screen sharing go through `wlr-screencopy` and `ext-image-copy-capture`:
`grim` takes pictures, of a display or of one window, and `xdg-desktop-portal-wlr`, which
`shell/nekoland-portals.conf` picks, shares a display or a window for programs that ask
the portal, such as browsers and Discord. What to share is chosen in
`nekoland-screenshare`, a window of thumbnails of the windows and the displays; windows
are told apart by `ext-foreign-toplevel-list`. Copy and paste works
between X11 and Wayland programs.

While any program has a microphone or a camera open, a dot shows left of the clock, as on
the Mac: orange or green. While the screen is shared, a purple button shows there, whose
menu stops the sharing.

Programs in a sandbox such as Flatpak's connect through `security-context-v1` and are
kept from what reaches other programs: input methods, virtual keyboards, the clipboard
manager protocol, other programs' windows, screen capture and the lock.

## Remote desktop

Other computers come in over RDP through xrdp and xorgxrdp, whose X server Nekoland
covers as a window without a frame, following its size as the client's window changes.
Settings' 共享 page installs xrdp (with `xorgxrdp` and `pipewire-module-xrdp`, left off)
and removes it, its configuration too when that is chosen, turns the `xrdp` service on and off, through `sudo -A` and
`nekoland-askpass` (not from inside an xrdp session, which would end it), shows the
address and port to connect to, and gives xrdp sessions Nekoland by writing `~/.xsession`, which
xrdp's session script runs; one already there is kept in `~/.config/nekoland/xsession`
and put back when the session is set to the system's again.
The same page has 遠端登入, which turns OpenSSH's server on and off (`ssh.socket` where
there is one) and shows the `ssh` command to reach this computer with. 檔案共享 installs Samba
(left off) and removes it, with the share, the Samba password and what was set up for
them when that is chosen, and shares the Public folder over SMB as
`Public-USER`: turning it on asks for a Samba password, adds the person to `sambashare`,
turns on users' shares in `/etc/samba/smb.conf` and starts `smbd`.

As on a Mac, what is turned on there is let through the firewall (ufw, 網路's 防火牆)
from the LAN only, the private networks, on the ports its configuration has, and taken
out again as it is turned off or removed. A system path unit,
`nekoland-firewall-USER.path`, which `install.sh` and Settings set up, brings the rules
up to date (`nekoland-settings --firewall-sync USER`, as root) whenever SSH's, xrdp's or
Sunshine's configuration changes, a service is turned on or off, by hand too, or the
firewall is turned on, and at boot; ufw keeps them while it is off. A port changed in
`sshd_config` or `xrdp.ini` is put in effect then too, by restarting the service, and
the page shows the ports as they are each time it is shown.

遠端登入 also has Sunshine, for Moonlight clients. Settings installs it from LizardByte's
newest release, the package built for this Ubuntu (its SHA-256 checked), with a udev rule
that gives the person its virtual input devices and lets its ports through the firewall
from the LAN (it turns on and off without a password), and removes it again, with its
settings, paired devices and login when that is chosen. Turned on, it runs as a user service, either streaming the
desktop at the computer (`nekoland-sunshine.service`) or a session of its own
(`nekoland-remote-session.service`): `nekoland --headless`, a compositor with no display
of its own whose screen, `HEADLESS-1` (1920×1080 unless the configuration gives it a mode),
is drawn only into the frames Sunshine asks for over wlr-screencopy, which starts locked,
takes input only from Sunshine's devices, and keeps running after logging out (lingering).
The desktop at the computer then ignores those devices. Each client gets a screen of its
own size: Settings gives Sunshine prep commands (`global_prep_cmd`, unless it has its own)
that run `nekoland msg stream-mode WIDTHxHEIGHT@HZ` with the client's size and rate as it
connects and `nekoland msg stream-mode` as it leaves. `HEADLESS-1` takes that size and
rate exactly; the desktop at the computer puts its main display in the mode nearest it,
the client's own size when it has one, else the smallest that holds it. The page sets Sunshine's web
login, pairs a device that is waiting with the PIN Moonlight shows, removes paired ones,
and edits `sunshine.conf`; the login is kept in the keyring.

Nekoland knows the session by xrdp's `XRDP_SESSION` and, as on a console, names itself
to the portals and to the services D-Bus starts. Sleep, from the menu, the power button
or idleness, only locks the screen there, since the computer is elsewhere. Frames are
drawn only when something in them changed, and only that is sent to the X server, so
that xrdp encodes no more. The cursor is xrdp's, drawn by the client: the cursors
programs name show as they are, and one a program draws itself as the arrow. With
`wl-clipboard` and `xclip` installed, text copied on either side can be pasted on the
other; other kinds of content stay where they were copied. Sound needs xrdp's PipeWire
module, `pipewire-module-xrdp`.

xrdp draws in software, so glass is slower than on a console. All of the client's
monitors are one display to Nekoland. The user's D-Bus services serve one session at a
time: logged in on the console and over RDP at once, the services started last follow the
session that started last.

## GTK applications

Nekoland's design language reaches GTK 4 and libadwaita applications through the system's
style, as a Mac's controls change when the system does, without changing the applications.
`shell/gtk/nekoland.css` sets libadwaita's colours to Nekoland's paper and ink, with what
is selected in solid ink, light and dark, and windows to 14px corners; draws GTK's
buttons, switches, sliders, fields, segmented controls, menus, calendars, tabs, sidebar
selection, dialogs, toasts and tooltips as the design language has them; has what is pressed give and spring back, buttons, switches, sliders, cards and the tiles of a grid; and leaves sidebars see-through, on the glass Nekoland
draws behind every window. Settings' 設計語言 page shows all of it, after the design,
in this look whether or not it is turned on. Settings' 外觀 page
turns it on: it puts the file in `~/.config/gtk-4.0/nekoland/` and imports it from
`~/.config/gtk-4.0/gtk.css`, keeping what else that file has, and keeps it up to date with Settings.
It sets their text in Ubuntu Sans and Noto Sans CJK TC, as Nekoland's own is set.
Windows have minimize, maximize and close at the right of the title bar with the look or
without it: install.sh makes that GNOME's `button-layout` default in a Nekoland session, and
a layout chosen by the user is kept. GTK reads that file as an application starts, so others take the change when opened again;
Settings shows it at once, and turned off it opens again without it. It needs GTK 4.20
or later, the first to read CSS variables and `prefers-color-scheme` in a stylesheet.

GTK 3 programs get the same look as a theme, `Nekoland`, which install.sh puts in
`/usr/local/share/themes`: `shell/gtk3/Nekoland/` holds its light and dark variants, on
Adwaita's, with the colours of `shell/gtk/nekoland.css`. Turning the look on in Settings
makes it GNOME's `gtk-theme`, which GTK 3 programs on Wayland read, and turning it off puts
the default back. X11 programs are given that setting, the icon and cursor themes, the
font, the buttons of the title bar, whether to be dark, and the DPI, GDK scale and cursor
size of the display's scale by `xsettingsd`, which the
compositor starts with XWayland and writes the file of, in `$XDG_RUNTIME_DIR`, again as
Settings or the dark switches change them. Qt, Wine and programs that draw their own
controls keep their own look. On a scaled display XWayland draws at the display's pixels,
not enlarged, so X11 programs that follow that DPI are sharp. Files dragged from a Wayland
window or the desktop drop into X11 programs, a Windows program in Wine among them, and
what X11 programs drag drops into Wayland's, through XDND.

X11 programs that do not draw their own title bar get one from Nekoland: the window's title,
and minimize, maximize and close at its right, above the window. Its edges and the bar's top
resize it, as an X11 program's own request to do so does. Maximized, it sits under the menu
bar; fullscreen, it is not drawn. It is drawn as GTK's header bar is: in Nekoland's look
while Settings has that on, in Adwaita's otherwise. A Windows App program's window, which
has Windows' title bar in it, has the bar over that one.

`nekoland-calendar`, from `ui/`, is a small calendar of GTK's and libadwaita's own widgets
with no styling of its own: a month, the chosen day's events, kept in
`~/.local/share/nekoland/calendar.tsv`, a search, a menu and a dialog to add one. It is
there to try the look on an application that knows nothing of it;
`shell/io.github.koukeneko.Nekoland.Calendar.desktop` puts it among the applications.

`nekoland-store`, from `ui/`, is Market: an app store over apt, Snap, Flathub and Homebrew, and
over the vendors' own apt repositories. An app found in several shows once, with a row per
source giving its version, size, how it updates and how it is confined; the picked row is
the one 安裝 uses, and an installed one can be switched for another. It reads AppStream,
`apt-cache`, snapd's socket and the `flatpak` and `brew` commands, so a source whose
command is missing is left out. Flathub apps go to the user's installation; apt and Snap
need an administrator, which it asks for through `nekoland-askpass` and `sudo -A`, once
for all a change runs, so the user must be in the `sudo` group. Adding a vendor's
repository (Ookla, Microsoft, Google) asks first and writes its key to `/etc/apt/keyrings`
and a `.sources` file to `/etc/apt/sources.list.d`; it is then listed under 其他套件庫,
where it can be removed. Under 外觀 it lists icon and cursor themes, those installed and
the most downloaded on Pling (gnome-look.org), through Pling's OCS API. Installing one
unpacks its archive with `tar` (or Python's `zipfile`) into `~/.local/share/icons` and runs
nothing in it; what Market installed is kept in `~/.local/share/nekoland/market.json`, so
that it can be removed whole. A cursor theme's page fetches the pack to try its cursors on
before it is installed; an icon theme's page draws the Dock's applications in it. 套用 sets
`icon_theme` or `cursor_theme`, and GNOME's matching setting, as Settings does.
`shell/io.github.koukeneko.Nekoland.Store.desktop` puts it among the applications, as the
program that opens `.deb` files, which it is also given on its command line
(`nekoland-store app.deb`). A package file's page (`ui/src/store/deb.rs`) shows what
`dpkg-deb -f` reads from it, what `apt-get --simulate` would install or remove with it, and
whether the installed version is older, the same or newer; 安裝, 更新, 重新安裝 or 降級 runs
`apt-get install` on the file under the same `sudo -A`, so that its dependencies come from
the repositories. A package for another processor, or one whose dependencies apt cannot
find, says so in place of the button.

Nekoland comes with no web browser. Market's 瀏覽器 page lists the ones it can install on
this computer (`ui/src/store/browsers.rs`): each from Flathub, the Snap Store, Ubuntu or its
vendor's repository, whichever this computer has, with its icon fetched from Flathub; the
first one installed becomes the default for web links. At each login the compositor runs
`nekoland-store --choose-browser`, which ends at once if there is a browser or the choice
was skipped (`~/.local/state/nekoland/browser-choice-skipped`), and otherwise asks for one:
the same browsers in a new order each time, none picked, the one picked installed, made the
default and opened. While there is none, Settings' 預設應用程式 shows 網頁瀏覽器 as 未安裝,
with 取得 opening Market's 瀏覽器 page.

`nekoland-filechooser`, from `ui/`, is the window programs open and save files in when they
ask `xdg-desktop-portal`: browsers, Flatpak and Snap applications, GTK 4's file dialog,
Electron, and GTK 3's native one, as Nekoland sets `GTK_USE_PORTAL=1`; a GTK 3 program that
builds its own `GtkFileChooserDialog` keeps GTK's. The places are on the left (recent files, the home folders, the bookmarks Files
keeps and mounted drives), the folder as a list or as icons with Files' thumbnails, the
types the program asked for at the bottom, with what is not of the chosen type shown faded
rather than hidden. Saving shows only the name and the folder until it is unfolded into the
whole window, and asks before replacing a file. Ctrl+L, or `/` or `~`, types a path; Ctrl+H
shows hidden files. It remembers the view, hidden files and the unfolding in
`~/.config/nekoland/file-chooser.ini`. To use it, install it as
`/usr/local/bin/nekoland-filechooser` (or change the path in the `.service` file), copy
`shell/nekoland.portal` to `/usr/share/xdg-desktop-portal/portals/`,
`shell/org.freedesktop.impl.portal.desktop.nekoland.service` to
`~/.local/share/dbus-1/services/` and `shell/io.github.koukeneko.Nekoland.FileChooser.desktop`
to `~/.local/share/applications/`, and `shell/nekoland-portals.conf` as above; they take effect
once D-Bus reloads its configuration and the portal starts again, as at the next login. The window opens in the middle of the screen, not over the program
that asked, which needs xdg-foreign; it has no type-ahead in the list, no completion of typed
paths, and does not take files dropped on it.

`nekoland-fileop copy|move <folder> <path>...`, from `ui/`, copies or moves files and folders
into a folder; files dropped on a folder on the desktop or in the Dock are moved with it, or copied when the folder is on another drive, so it
must be on the `PATH`. Its window shows only when the work takes more than half a second, with
how far it is and the time left, and comes up at once when a name is taken there: 略過,
保留兩者 (「季報 2.pdf」) or 取代, which for two folders is 合併 and asks again for the files
inside, optionally for every later clash. A file and a folder of one name do not overwrite
each other; the one replaced goes to the trash. When something fails it says why and offers
略過 or 重試. Copy `shell/io.github.koukeneko.Nekoland.FileOperation.desktop` to
`~/.local/share/applications/` for its name and icon. Each piece of work has its own window.

`nekoland-files`, from `ui/`, is a file manager after the Mac's Finder: tabs, the places on
glass at the left (最近使用, the home folders, 垃圾桶, the bookmarks, drives with an eject
button, the network and the tags), and a folder seen as icons, as a list with folders that
unfold, as columns or as a gallery, with a preview at the right. A search looks in the folder
or in the home folder and mounted drives, by name or by tag, and narrows by kind and date. Space opens 快速查看 on
what is chosen; Ctrl+I opens 取得資訊, where the name, the tags, the application that opens it
and the permissions change. Tags are seven colours kept in the `user.xdg.tags` attribute, so
other programs see them. Several files are renamed at once by replacing or adding text or by a
numbered name; files compress into a `.zip`, a picture turns or becomes the wallpaper, and
連接伺服器 (Ctrl+K) mounts smb, sftp, ftp or WebDAV through GVfs. Copying and moving are
`nekoland-fileop`'s, a drag held over a folder opens it, and pictures, PDFs and videos show
thumbnails from Files' cache, made with `pdftoppm` and `ffmpeg` when missing. It answers
`org.freedesktop.FileManager1` when Nautilus does not hold it, so 在檔案中顯示 comes to it;
`install.sh` installs `shell/org.freedesktop.FileManager1.service` and
`shell/io.github.koukeneko.Nekoland.Files.desktop`. It remembers the view, the preview, hidden
files and the icon size in `~/.config/nekoland/files.ini`.

`nekoland-tasks`, from `ui/`, is the task manager: four meters across the top, the CPU,
memory, disks and network over the last minute, and below them each application with its
processes, then the user's other processes, read from `/proc` every second. Pressing the CPU,
記憶體 or 磁碟 meter sorts the table by it and draws each row's share as a bar. 結束 (Delete)
asks an application to quit as the Dock does, or sends a process SIGTERM; 強制結束 kills it
after asking. An application whose window does not answer the compositor's ping within three
seconds is marked 沒有回應. It asks the compositor through `nekoland msg tasks`, which lists
each window's process, application and whether it answers, and `quit-app` and
`force-quit-app`.

`nekoland-preview`, from `ui/`, is 預覽程式: PDFs and pictures, as the Mac's Preview opens
them, with its menus and keys (⌘ is Ctrl, ⌥ is Alt), one file or many in a window. The
sidebar shows thumbnails, a PDF's 目錄, its 標示重點和附註 and 書籤, or a 聯絡表 of them all;
pages are dragged in it to reorder them, and files dropped on it are added or inserted. A PDF
is drawn by poppler, scrolled as one column or shown a page or two at a time, sharp at any
zoom; its text is chosen, copied and found (Ctrl+F, Ctrl+G), its links and forms work, and
it is marked with 標示重點, 底線 and 刪除線, notes, text boxes, shapes, sketches and
signatures (drawn or made from a picture), and 塗黑, which takes the text under it out of
the file when it is saved. Pages are rotated, cropped, deleted, inserted from a file, a
scanner (`scanimage`) or as blank pages, and a PDF gets passwords and permissions or is made
smaller (`qpdf`). Pictures are turned, flipped, cropped, resized, colour-adjusted, given a
profile, chosen from with 矩形選取, 橢圓形選取, 套索, 智慧型套索 and 即時 Alpha, and drawn on.
Changes are saved when the window closes, the file kept first among 所有版本 to go back to;
輸出 writes PNG, JPEG, TIFF, BMP, PDF, HEIC (`heif-enc`), JPEG 2000 (`opj_compress`) or
OpenEXR (ImageMagick). 檢閱器 (Ctrl+I) shows a file's details, EXIF and keywords
(`exiftool`), a PDF's permissions and annotations. 從剪貼板新增, screenshots (`grim`,
`slurp`), 列印, the 放大鏡 and 幻燈片秀 are there too; formats gdk-pixbuf cannot read are
converted by ImageMagick, and an Illustrator `.ai` is read as the PDF it holds.

`nekoland-disk`, from `ui/`, is 磁碟用量: where a folder's space went. A place in its sidebar,
a folder chosen or dropped on it, or a folder opened with it is scanned on a thread of its
own, staying on its disk, counting the blocks each file takes and a file with several names
once, and leaving out the places listed under 排除的位置. It is drawn as a sunburst or a
treemap, beside a list that sorts by name, date or size, searches the folder shown or the
whole scan and filters by kind and size, and an inspector of what the pointer is on. A
folder's many smallest files are drawn as one item. 快速查看, 在「檔案」中顯示, 複製路徑 and
丟到垃圾桶 act on one item; 清理清單 gathers several to put in the trash together. Each scan is
kept in `~/.local/state/nekoland/disk`, five of each folder, for 最近的掃描 and for comparing
with a later one; 儲存掃描檔 writes one as a gzipped `.nekoscan` to open elsewhere.

`nekoland-disks`, from `ui/`, is 磁碟: the drives and what is on them. `lsblk` and `losetup`
are read every three seconds, and each drive's partitions and free space are drawn side by
side as wide as they are large, with how full each is, beside a list of them. Choosing one
shows what can be done to it: 掛載 and 卸載 through `udisksctl`, 停用 or 啟用 for swap, 調整大小
for ext4, Btrfs, XFS (larger only) and NTFS, 檢查檔案系統, 格式化 to ext4, Btrfs, XFS, exFAT, FAT32,
NTFS or swap, quick or written over with zeros, 新增分割區 in free space, and 建立分割表 on a
drive with none. What `udisksctl` may not do, and all the rest, is done as root through
`sudo -A`, asking for the password with `nekoland-askpass`; the partition the system runs from
is not unmounted or formatted. An external drive can be 退出; the menu reads its SMART data
(with `smartmontools`), times it, writes it to a `.img` or `.img.zst` and writes one back, and
清除磁碟 wipes it. 開啟映像檔 makes a loop drive of an image, shown under 映像檔. A job's
progress is shown in the bar over the buttons, with 取消.

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
| Alt+Space | Launcher |
| Super+Up / Down | Maximize / restore |
| Super+Left / Right | Left / right half; again, two thirds, then a third |
| Super+Alt+Up / Down | Top / bottom half, cycling as the sides do |
| Super+Alt+U / I / J / K | Top left / top right / bottom left / bottom right quarter |
| Super+Alt+D / F / G | First / center / last third |
| Super+Alt+E / T | First / last two thirds |
| Super+Alt+Return, Super+Alt+Shift+Up | Maximize, maximize height |
| Super+Alt+C | Center |
| Super+Alt+= / - | Larger / smaller |
| Super+Alt+Backspace | Restore |
| Dragging a window in a half or maximized | Back to its earlier size |
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

`nekoland msg WORDS` talks to the running compositor: `overview`, `apps`, `notification-center`, `lock`,
`sleep`, `workspace-add`, `workspace-next`, `workspace-previous`, `quit`; `'window COMMAND'` arranges the active window
as the launcher's window commands do, by their names in `src/arrange.rs` (`left`, `top-right`,
`first-third`, `larger`, `move-left` and so on); `reload` reads the configuration again,
`config` prints the settings in effect, `modes` the display's mode and then all it has,
`display` its mode, preferred mode, scale and the scale its density gives, `scales` the
scale each of its sizes is drawn at when the settings name none, `sun` today's
sunset and sunrise where the time zone is, `stream-mode WIDTHxHEIGHT@HZ` the size a
Sunshine client streams at, until `stream-mode` alone ends it,
and `'set KEY = VALUE'` changes one setting in the file, keeping the rest of it, and puts
it in effect, or says why not; `'unset KEY'` puts one back at its default.

## Not there yet

- In the menu bar, menus taller than the screen do not scroll, the Help search takes
  typed letters but not an input method's, the sound menu steps the volume instead of
  sliding it, and the network menu shows the connection without choosing one.
  GNOME's own programs export no menus, so they get the menus Nekoland makes.
- Settings gives every display the same mode and scale, and windows keep their size
  when a display's changes. A wallpaper started by `autostart` stays under one chosen
  in Settings until the session ends.
- One GPU. A second display takes the pointer and windows, but the menu bar, Dock and
  overview stay on the first, and displays are not hot-plugged.
- X11 programs that ignore the DPI and GDK scale they are given, as xclock does, are
  drawn at half their size on a scaled display.
- Screen capture copies through the CPU, except a whole display over wlr-screencopy,
  which is drawn into a dmabuf when the program gives one, as Sunshine does; and the
  screen does not turn off when idle.
- X11 windows have no title bars.
- Notifications show no buttons for their actions, and the Notification Center forgets
  them when Nekoland restarts.

## Origin

The starting point was `smallvil`, the minimal example compositor of Smithay
0.7.0, used under its MIT license (`licenses/SMITHAY-MIT.txt`).
`third_party/smithay` is Smithay as of its November 2025 drag and drop between Wayland
and X11 (commit 0d3d212), with Nekoland's fixes to it, among them handing a drag over
an X11 window's surface to XDND.
