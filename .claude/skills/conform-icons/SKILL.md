---
name: conform-icons
description: Nekoland's app icons - restyle the icons of installed apps so they match Nekoland's own, review and correct the result, or draw a new icon by hand to the same rules. Use when apps were installed, when icons in the Dock or app grid look out of place, when asked to make icons match, or when asked to design or redraw an icon.
---

Nekoland's icons are one icon theme, `Nekoland`, in two halves: the hand-drawn
ones in `shell/icons/apps` (installed to `/usr/local/share/icons/Nekoland`), and
the ones `shell/conform-icons.py` makes from other apps' own icons, in the
person's `~/.local/share/icons/Nekoland/scalable/apps`. The compositor looks in
it before Yaru; Settings › 外觀 has the switch and the list of both halves.
While the switch is on, the compositor runs the tool again (`--refresh`) whenever a
folder of desktop entries changes, so a newly installed app is restyled by itself.
Folders and other places come from Yaru as they are.

## The rules every icon follows

Settings › 設計語言 › 圖示 shows these as a drawing (`ui/src/icon-sheet.svg`);
its figures are the tool's, so change the two together.

- A 128 canvas; the tile is a superellipse (n = 5) 103 across, 12.5 from the edge.
- The tile is one colour, lit faintly from above: about 16% lighter at its top and
  9% darker at its foot, as a slope of its own colour.
- What stands on it is a layer, centred: 66 across when cut from a plate, 78 for a
  whole object. Layers may be frosted white or their own colour, and translucent
  ones may lie behind others.
- Light falls only along the top edge and, weaker, the bottom one, computed from
  the shape by the `edge` and `cast` filters. A layer casts one soft shadow.
- Never: grain, brushed metal, a vignette, a sheen across the top half, or a
  darker copy of the tile under it as thickness.

## Restyling installed apps' icons

The tool decides per icon by looking at its pixels; your job is to catch the
decisions that look wrong and correct them through the overrides file. Never
edit the generated SVGs.

1. Run it with a sheet, on a machine that has the apps installed:

       /usr/bin/python3 shell/conform-icons.py --sheet /tmp/conform-sheet.html --report

   It skips every icon that has a hand-drawn one, and prints one JSON entry per
   icon: `mode`, `tile`, `fill` (share of the bounding box covered), `light`
   (share of near-white pixels). `--remove` deletes what it wrote. The test VM
   (`ssh nekoland`, checkout `~/Nekoland`) has few apps: generate on the host
   with `--out DIR` and copy the SVGs into the VM's half of the theme.

2. Screenshot the sheet and look at every pair (app icon → restyled tile). The
   modes are `layers` (a plain background became the tile and the shape on it a
   layer), `plate` (the icon fills the tile whole) and `object` (a shape stands
   on a light or dark tile). Correct an icon when:
   - splitting took the body of the thing for background, leaving parts afloat
     (a camera's lens, a console's buttons): set `"layers": false`;
   - its own background shows as a smaller plate inside the tile, or its corners
     or rim show: set `"mode": "plate"`;
   - a shape (a gear, a stopwatch, a logo on transparency) was zoomed and
     cropped: set `"mode": "object"`;
   - it is hard to see on its tile, or the tile colour fights it: set `"tile"`
     to a colour from the icon itself, or `#232327` (dark) / `#f7f7f9` (light);
   - a plate's own ring or border still shows inside the tile: raise `"scale"`
     (1.2–1.3 for a disc) until it falls outside;
   - an object looks too large or too small next to its neighbours: set
     `"scale"` (1.0 is the default; stay within 0.8–1.15).

3. Write the corrections into `~/.config/nekoland/icon-overrides.json`, keyed by
   the `Icon=` name from the report, keeping entries that are already there:

       {"steam": {"mode": "plate", "tile": "#1b2838"}}

4. Run step 1 again and recheck only the icons you corrected. Stop when a pass
   needs no correction.

## Drawing an icon by hand

For Nekoland's own apps and the common ones worth more than a restyle. Each is
one SVG in `shell/icons/apps`, named as the app's `Icon=` names it.

- Show a few versions before replacing anything: render each at 280, 64, 40 and
  24 pixels beside the current icon, and let the person choose. An icon that is
  not readable at 24 is not finished.
- Take the tile, `drop`, `cast` and `edge` filters from the tool's `DEFS` (import
  the script as a module) rather than writing new ones, so a drawn icon and a
  restyled one cannot drift apart.
- Draw your own shapes. An app's official mark may be the idea (Fcitx's penguin
  with a brush) but not the artwork: do not copy another theme's icon or embed an
  app's own file in the repository.
- Text in an icon is set in `Ubuntu Sans, Noto Sans CJK TC`, which the installer
  provides.
- The Calendar's icon is drawn by the compositor from `src/calendar-today.svg`,
  with the day put in; change that file with the icon, and run
  `cargo test --bin nekoland the_calendar_icon`.

## Seeing the result

Install a hand-drawn icon on the VM with
`sudo install -Dm 644 -t /usr/local/share/icons/Nekoland/scalable/apps FILE`
(and into `~/.local/share/icons/hicolor/scalable/apps` for the five
`io.github.koukeneko.Nekoland.*` ones). The compositor keeps the icons it has
drawn; `NEKOLAND_SOCKET=/run/user/1000/nekoland-wayland-1.sock nekoland msg reload`
makes it draw them again, with no restart. Only the Calendar needs the
compositor rebuilt and restarted, which needs the user's OK.
