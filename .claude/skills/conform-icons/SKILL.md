---
name: conform-icons
description: Restyle the app icons installed on a Nekoland machine so they match Nekoland's own tiles, then review the result and correct the ones that came out wrong. Use when apps were installed, when icons in the Dock or app grid look out of place, or when asked to make icons match.
---

`shell/conform-icons.py` lays each installed app's own icon on Nekoland's tile.
It decides per icon by looking at its pixels; your job is to catch the decisions
that look wrong and correct them through the overrides file. Never edit the
generated SVGs or the hand-drawn icons in `shell/icons/apps`.

1. Run it with a sheet, on the machine whose icons you are restyling (over
   `ssh nekoland` for the VM; there the checkout is `~/Nekoland`):

       /usr/bin/python3 shell/conform-icons.py --sheet /tmp/conform-sheet.html --report

   Add `--ai` on a machine that has the local model (`~/.local/share/nekoland-icon-ai`
   holds `sd/sd-cli`, `models/sd15-fp16.safetensors` and, for a full repaint with
   outlines locked, `models/control-canny-fp16.safetensors`): it paints relief and
   lighting on each tile and keeps the icon's own pixels. The VM has no model;
   generate on the host with `--out DIR` and copy the SVGs over.

   It writes to `~/.local/share/icons/hicolor/scalable/apps` and prints one JSON
   entry per icon: `mode`, `tile`, `fill` (share of the bounding box covered),
   `light` (share of near-white pixels).

2. Screenshot the sheet and look at every pair (app icon → restyled tile).
   Correct an icon when:
   - its own background shows as a smaller plate inside the tile, or its corners
     or rim show: set `"mode": "plate"`;
   - a shape (a gear, a stopwatch, a logo on transparency) was zoomed and
     cropped: set `"mode": "object"`;
   - it is hard to see on its tile, or the tile colour fights it: set `"tile"`
     to a colour from the icon itself, or `#22262c` (dark) / `#eeeeec` (light);
   - a plate's own ring or border still shows inside the tile: raise `"scale"`
     (1.2–1.3 for a disc) until it falls outside;
   - an object looks too large or too small next to its neighbours: set
     `"scale"` (1.0 is the default; stay within 0.8–1.15).

3. Write the corrections into `~/.config/nekoland/icon-overrides.json`, keyed by
   the `Icon=` name from the report, keeping entries that are already there:

       {"steam": {"mode": "plate", "tile": "#1b2838"}}

4. Run step 1 again and recheck only the icons you corrected. Stop when a pass
   needs no correction. The Dock and app grid cache icons, so the compositor
   shows the new ones after it restarts; restarting it needs the user's OK.
