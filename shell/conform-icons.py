#!/usr/bin/python3
"""Restyle installed apps' own icons as Nekoland tiles.

Each app's icon is read from where the app installed it and laid on the
same thick, grained squircle tile the shipped Nekoland icons use. Nothing
is downloaded: the result is a local, per-user restyle of what is already
on the machine.

    conform-icons.py [--out DIR] [--overrides FILE] [--report] [--sheet HTML] [--ai] [ICON ...]

With no ICON names, every app with a .desktop file is processed; icons in
--out that this tool did not write (the hand-drawn ones) are left alone.
--report prints the decision for each icon as JSON and --sheet writes a
before/after page. An agent (or a person) screenshots the sheet, and for
each icon that came out wrong writes a correction into the overrides file,
then runs the tool again:

    {"steam": {"mode": "plate"}, "gimp": {"tile": "#2b2f36", "scale": 0.9}}

The overrides file defaults to ~/.config/nekoland/icon-overrides.json.

--ai lets a local Stable Diffusion 1.5 (stable-diffusion.cpp's sd-cli and
a model file under ~/.local/share/nekoland-icon-ai) paint relief and
lighting on each app's original icon before it goes on the tile, a few
seconds an icon. With control-canny-fp16.safetensors next to the model, the
icon is repainted with its outlines locked; without it only the model's
light is laid over the icon's own pixels. Either way a logo keeps its shape.

mode   "plate"  the icon already has its own background. When that is a plain
                colour with a shape on it, the two are split: the colour
                becomes the tile and the shape a layer on it ("layers" in
                the report); "layers": false keeps the icon whole instead
       "object" the icon is a shape: it stands on the tile as a layer
Every layer gets the same size grid, shadow and lit edge, which is what
makes the set look like one family.
tile   tile colour (#rrggbb)
scale  size relative to the default (1.0); above 1 crops a plate's own rim
"""
import argparse, base64, configparser, glob, io, json, math, os, subprocess, sys, tempfile

import gi
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import GdkPixbuf, GLib

HOME = os.path.expanduser("~")
OUT = f"{HOME}/.local/share/icons/hicolor/scalable/apps"
OVERRIDES = f"{os.environ.get('XDG_CONFIG_HOME', HOME + '/.config')}/nekoland/icon-overrides.json"
HAND_DRAWN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "icons", "apps")
DATA_DIRS = [f"{HOME}/.local/share", f"{HOME}/.local/share/flatpak/exports/share", "/var/lib/flatpak/exports/share",
             *os.environ.get("XDG_DATA_DIRS", "/usr/local/share:/usr/share").split(":")]
SHIPPED = "io.github.koukeneko.Nekoland."
LIGHT, DARK = "#eeeeec", "#22262c"
SIZE = 256  # analysis and embedding resolution
AI_HOME = f"{HOME}/.local/share/nekoland-icon-ai"
AI_PROMPT = ("app icon, glossy 3d rendered object on a rounded square tile, embossed, realistic materials, "
             "soft studio lighting from above, specular highlights, subtle shadow, highly detailed, sharp")
AI_NEGATIVE = "text, watermark, blurry, distorted, deformed, noisy, photo, people, flat, long shadow"
MARK = "<!-- conform-icons -->"  # only files carrying this are ours to overwrite


def squircle(a=51.5, n=5, steps=160):
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        pts.append(f"{64 + a * math.copysign(abs(c) ** (2 / n), c):.2f} {64 + a * math.copysign(abs(s) ** (2 / n), s):.2f}")
    return "M" + "L".join(pts) + "Z"
SQ = squircle()

# The tile, its thickness, grain, vignette and rim are the shipped icons' materials.
DEFS = f'''
    <filter id="drop" x="-20%" y="-20%" width="140%" height="150%"><feGaussianBlur in="SourceAlpha" stdDeviation="1.4"/><feOffset dy="1.4"/><feComponentTransfer><feFuncA type="linear" slope=".32"/></feComponentTransfer><feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge></filter>
    <filter id="cast" x="-40%" y="-40%" width="180%" height="200%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="4.6" result="a"/><feOffset in="a" dy="5.2" result="a2"/><feComponentTransfer in="a2" result="amb"><feFuncA type="linear" slope=".34"/></feComponentTransfer>
      <feGaussianBlur in="SourceAlpha" stdDeviation="1" result="c"/><feOffset in="c" dy="1.3" result="c2"/><feComponentTransfer in="c2" result="con"><feFuncA type="linear" slope=".5"/></feComponentTransfer>
      <feOffset in="SourceAlpha" dy="1.2" result="down"/><feComposite in="SourceAlpha" in2="down" operator="out" result="top"/><feGaussianBlur in="top" stdDeviation=".35" result="tops"/><feFlood flood-color="#fff" flood-opacity=".8"/><feComposite in2="tops" operator="in" result="hi0"/><feComposite in="hi0" in2="SourceAlpha" operator="in" result="hi"/>
      <feOffset in="SourceAlpha" dy="-1.4" result="up"/><feComposite in="SourceAlpha" in2="up" operator="out" result="bottom"/><feGaussianBlur in="bottom" stdDeviation=".5" result="bots"/><feFlood flood-color="#fff" flood-opacity=".5"/><feComposite in2="bots" operator="in" result="lo0"/><feComposite in="lo0" in2="SourceAlpha" operator="in" result="lo"/>
      <feMerge><feMergeNode in="amb"/><feMergeNode in="con"/><feMergeNode in="SourceGraphic"/><feMergeNode in="lo"/><feMergeNode in="hi"/></feMerge></filter>
    <filter id="grain" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="1.3" numOctaves="2" seed="7"/><feColorMatrix values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  .55 0 0 0 -.2"/></filter>
    <radialGradient id="vig" cx=".5" cy=".42" r=".75"><stop offset=".55" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity=".09"/></radialGradient>
    <linearGradient id="rim" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".9"/><stop offset=".24" stop-color="#fff" stop-opacity="0"/><stop offset=".74" stop-color="#fff" stop-opacity="0"/><stop offset="1" stop-color="#fff" stop-opacity=".55"/></linearGradient>
    <linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".3"/><stop offset=".45" stop-color="#fff" stop-opacity="0"/></linearGradient>
    <linearGradient id="floor" x1="0" y1="0" x2="0" y2="1"><stop offset=".5" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity=".12"/></linearGradient>
    <radialGradient id="spot" cx=".5" cy=".08" r=".7"><stop offset="0" stop-color="#fff" stop-opacity=".22"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient>
    <clipPath id="clip"><path d="{SQ}"/></clipPath>'''


def icon_id(name):
    """Icon= may be a path; the output is named after its file without .svg/.png."""
    base = os.path.basename(name)
    return base[:-4] if base.endswith((".svg", ".png")) else base


LIGHTING = f'<path d="{SQ}" fill="url(#spot)"/><path d="{SQ}" fill="url(#gloss)"/><path d="{SQ}" fill="url(#floor)"/>'


def mix(colour, other, amount):
    a, b = (tuple(int(c[i:i + 2], 16) for i in (1, 3, 5)) for c in (colour, other))
    return "#%02x%02x%02x" % tuple(round(a[i] + (b[i] - a[i]) * amount) for i in range(3))


def lit_layer(png):
    """A foreground layer shaded as a solid lit from above: brighter at its top, darker at its foot."""
    from PIL import Image, ImageChops
    layer = Image.open(io.BytesIO(png)).convert("RGBA")
    w, h = layer.size
    ramp = Image.new("L", (1, h))
    ramp.putdata([round(140 - 34 * y / max(h - 1, 1)) for y in range(h)])  # 128 is no change
    ramp = ramp.resize((w, h))
    shaded = ImageChops.add(layer.convert("RGB"), Image.merge("RGB", (ramp, ramp, ramp)), offset=-128)
    shaded.putalpha(layer.getchannel("A"))
    out = io.BytesIO()
    shaded.save(out, "png")
    return out.getvalue()


def find_source(name):
    """The app's own icon: scalable first, then the largest bitmap. Our output dir is skipped."""
    if os.path.isabs(name):
        return name if os.path.exists(name) else None
    best, best_size = None, -1
    for data in DATA_DIRS:
        for theme in ("hicolor", "Yaru", "Adwaita"):
            # Exactly name.svg or name.png: "name.*" would also take name.Other-symbolic.svg.
            for path in glob.glob(f"{data}/icons/{theme}/*/apps/{glob.escape(name)}.[sp][vn]g"):
                if path.startswith(OUT) or os.path.dirname(path) == OUT_DIR:
                    continue
                size_dir = path.split("/")[-3]
                size = 10_000 if size_dir == "scalable" else int(size_dir.split("x")[0]) if "x" in size_dir else 0
                if path.endswith((".svg", ".png")) and size > best_size:
                    best, best_size = path, size
        if best_size < 0:
            for ext in ("svg", "png"):
                if os.path.exists(f"{data}/pixmaps/{name}.{ext}"):
                    best, best_size = f"{data}/pixmaps/{name}.{ext}", 0
    return best


def analyse(pixbuf):
    """Bounding box, how much of it is filled, and the colours of the shape."""
    if not pixbuf.get_has_alpha():
        pixbuf = pixbuf.add_alpha(False, 0, 0, 0)
    w, h, stride = pixbuf.get_width(), pixbuf.get_height(), pixbuf.get_rowstride()
    px = pixbuf.get_pixels()
    x0, y0, x1, y1, filled, lum, light = w, h, -1, -1, 0, 0.0, 0
    for y in range(h):
        row = y * stride
        for x in range(w):
            r, g, b, a = px[row + 4 * x: row + 4 * x + 4]
            if a > 128:
                filled += 1
                x0, y0, x1, y1 = min(x0, x), min(y0, y), max(x1, x), max(y1, y)
                l = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255
                lum += l
                light += l > 0.8
    if x1 < 0:
        return None
    bw, bh = x1 - x0 + 1, y1 - y0 + 1

    def at(fx, fy):
        x, y = x0 + int(fx * (bw - 1)), y0 + int(fy * (bh - 1))
        return px[y * stride + 4 * x: y * stride + 4 * x + 4]

    # Corners empty but edge midpoints solid: a rounded plate or a disc, not a free shape.
    edges = [at(.5, .06), at(.5, .94), at(.06, .5), at(.94, .5)]
    corners = [at(.03, .03), at(.97, .03), at(.03, .97), at(.97, .97)]
    square = 0.85 < bw / bh < 1.18
    fill = filled / (bw * bh)
    solid_edges = all(e[3] > 200 for e in edges)
    edge_rgb = [sum(e[i] for e in edges) // 4 for i in range(3)]
    uniform = all(abs(e[i] - edge_rgb[i]) < 40 for e in edges for i in range(3))
    corners_empty = all(c[3] < 60 for c in corners)
    # A single-colour disc reads as a plate too; a multicoloured one (Chrome) is an object.
    # Gears and stopwatches pass the edge test; only a true disc is solid all round its rim.
    ring = [at(.5 + .48 * math.cos(i * math.pi / 8), .5 + .48 * math.sin(i * math.pi / 8)) for i in range(16)]
    round_rim = all(p[3] > 200 and all(abs(p[i] - edge_rgb[i]) < 48 for i in range(3)) for p in ring)
    disc = square and solid_edges and uniform and corners_empty and round_rim and 0.74 < fill < 0.83
    plate = square and solid_edges and (fill > 0.86 or disc)
    return {
        "box": (x0, y0, bw, bh), "fill": round(fill, 3), "plate": plate,
        "corners_empty": corners_empty, "disc": disc,
        # A filled rectangle that is not square (GNOME's cards): a plate only if it splits into layers.
        "card": solid_edges and fill > 0.93 and not square and 0.6 < bw / bh < 1.7,
        "edge": "#%02x%02x%02x" % tuple(edge_rgb),
        "lum": round(lum / filled, 3), "light": round(light / filled, 3),
    }


def split_layers(png):
    """A plate icon as (top colour, bottom colour, foreground PNG), or None when it is not a
    plain background with a shape on it. This is what lets every icon get the same effects."""
    from PIL import Image, ImageChops
    icon = Image.open(io.BytesIO(png)).convert("RGBA")
    w, h = icon.size
    px = icon.load()
    # The background shows at the sides of each row; where the two sides differ it is not plain.
    # A second, deeper pair of columns gets past a border drawn down the sides (a film strip).
    for inset in (.07, .15):
        rows, disagree = {}, 0
        for y in range(h):
            sides = [px[x, y] for x in (int(w * inset), int(w * (1 - inset))) if px[x, y][3] > 250]
            if len(sides) == 2:
                disagree += max(abs(sides[0][i] - sides[1][i]) for i in range(3)) > 30
                rows[y] = tuple((sides[0][i] + sides[1][i]) // 2 for i in range(3))
        if len(rows) >= h * .4 and disagree <= len(rows) * .15:
            break
    else:
        return None
    column = Image.new("RGB", (1, h))
    for y in range(h):
        column.putpixel((0, y), rows[min(rows, key=lambda r: abs(r - y))])
    difference = ImageChops.difference(icon.convert("RGB"), column.resize((w, h)))
    r, g, b = difference.split()
    distance = ImageChops.lighter(ImageChops.lighter(r, g), b)
    mask = ImageChops.multiply(distance.point(lambda d: max(0, min(255, (d - 14) * 12))), icon.getchannel("A"))
    # A shape that reaches the plate's edge was cut by it; moved onto our tile the cut would show.
    inside = Image.new("L", (w, h), 0)
    inside.paste(255, (int(w * .1), int(h * .1), int(w * .9), int(h * .9)))
    # The bottom strip is exempt: a card's darker lower edge is its thickness, not a cut shape.
    sides = Image.new("L", (w, h), 255)
    sides.paste(0, (int(w * .1), int(h * .1), int(w * .9), h))
    edge = ImageChops.multiply(mask, sides)
    whole = sum(mask.histogram()[128:]) / (w * h)
    if sum(edge.histogram()[128:]) / (w * h) > .012 or not .03 < whole < .6:
        return None
    # Faint marks (grid lines, embossing) are part of the surface, not a shape standing on it.
    from PIL import ImageStat
    if ImageStat.Stat(distance, mask.point(lambda v: 255 * (v > 128))).mean[0] < 70:
        return None
    mask = ImageChops.multiply(mask, inside)
    icon.putalpha(mask)
    out = io.BytesIO()
    icon.crop(mask.getbbox()).save(out, "png")
    ys = sorted(rows)
    return "#%02x%02x%02x" % rows[ys[len(ys) // 10]], "#%02x%02x%02x" % rows[ys[-len(ys) // 10]], out.getvalue()


def png_bytes(pixbuf, box):
    x, y, w, h = box
    ok, buf = pixbuf.new_subpixbuf(x, y, w, h).save_to_bufferv("png", [], [])
    return buf


def render(name, source, override, ai=False):
    pixbuf = GdkPixbuf.Pixbuf.new_from_file_at_scale(source, SIZE, SIZE, True)
    if not pixbuf.get_has_alpha():
        pixbuf = pixbuf.add_alpha(False, 0, 0, 0)
    info = analyse(pixbuf)
    if info is None:
        return None
    mode = override.get("mode") or ("plate" if info["plate"] else "object")
    x, y, w, h = info["box"]
    art = png_bytes(pixbuf, info["box"])
    splittable = mode == "plate" or (info["card"] and "mode" not in override)
    layers = split_layers(art) if splittable and override.get("layers", True) else None
    if layers:
        # A plain background and a shape: the background becomes our tile, the shape a layer on it,
        # at the size and with the effects every other icon's layer gets.
        mode, (top, bottom, art) = "layers", layers
        tile = override.get("tile") or bottom
        from PIL import Image
        lw, lh = Image.open(io.BytesIO(art)).size
        k = 66 * override.get("scale", 1.0) / max(lw, lh)
        iw, ih = lw * k, lh * k
        data = base64.b64encode(lit_layer(ai_light(art) if ai else art)).decode()
        inner = f'<g filter="url(#cast)"><image x="{64 - iw / 2:.2f}" y="{64 - ih / 2:.2f}" width="{iw:.2f}" height="{ih:.2f}" href="data:image/png;base64,{data}"/></g>'
        strength = .1
        top = override.get("tile") or top
    else:
        top = None
        painted = ai_light(art) if ai else art
        data = base64.b64encode(painted if mode == "plate" else lit_layer(painted)).decode()
    if layers:
        pass
    elif mode == "plate":
        # Bleed past the squircle so the plate's own corners and rim fall outside.
        tile = override.get("tile") or info["edge"]
        # A disc must reach the squircle's corners (radius 63.4); a rounded plate only needs to bleed.
        side = 130 if info["disc"] else 103 * 1.06 if info["corners_empty"] else 103
        side *= override.get("scale", 1.0)
        k = side / max(w, h)
        iw, ih = w * k, h * k
        art = f'<image x="{64 - iw / 2:.2f}" y="{64 - ih / 2:.2f}" width="{iw:.2f}" height="{ih:.2f}" href="data:image/png;base64,{data}"/>'
        inner = art
        strength = .18
    else:
        # A mostly light shape would vanish on the light tile.
        tile = override.get("tile") or (DARK if info["light"] > 0.55 else LIGHT)
        side = 78 * override.get("scale", 1.0)
        k = side / max(w, h)
        iw, ih = w * k, h * k
        inner = f'<g filter="url(#cast)"><image x="{64 - iw / 2:.2f}" y="{62 - ih / 2:.2f}" width="{iw:.2f}" height="{ih:.2f}" href="data:image/png;base64,{data}"/></g>'
        strength = .22
    # Every tile is lit from above: lighter at the top, darker at the foot, whatever its colour.
    top, fill = top or tile, "url(#bg)"
    gradient = (f'\n    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{mix(top, "#ffffff", .14)}"/>'
                f'<stop offset="1" stop-color="{mix(tile, "#000000", .08)}"/></linearGradient>')
    svg = f'''{MARK}
<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">
  <defs>{DEFS}{gradient}
  </defs>
  <g filter="url(#drop)"><path d="{SQ}" fill="{fill}"/></g>
  <g clip-path="url(#clip)">
    {"" if mode == "plate" else LIGHTING}
    {inner}
    {LIGHTING if mode == "plate" else ""}
    <rect width="128" height="128" filter="url(#grain)" opacity="{strength}"/>
    <path d="{SQ}" fill="url(#vig)"/><path d="{SQ}" fill="none" stroke="#fff" stroke-opacity=".12" stroke-width="1.4"/><path d="{SQ}" fill="none" stroke="url(#rim)" stroke-width="2.6"/>
  </g>
</svg>
'''
    with open(f"{OUT_DIR}/{name}.svg", "w") as f:
        f.write(svg)
    return {"name": name, "source": source, "mode": mode, "tile": tile, **{k: info[k] for k in ("fill", "lum", "light")},
            "before": base64.b64encode(png_bytes(pixbuf, (0, 0, pixbuf.get_width(), pixbuf.get_height()))).decode(),
            "after": base64.b64encode(svg.encode()).decode()}


def write_sheet(path, report):
    cells = "".join(
        f'<figure><img src="data:image/png;base64,{r["before"]}"> → <img src="data:image/svg+xml;base64,{r["after"]}">'
        f'<figcaption>{r["name"]}<br>{r["mode"]} {r["tile"]}</figcaption></figure>' for r in report)
    with open(path, "w") as f:
        f.write(f'''<!doctype html><meta charset="utf-8"><title>conform-icons</title><style>
body{{margin:0;padding:20px;background:#8b8b8b;color:#fff;font:12px sans-serif;display:grid;grid-template-columns:repeat(6,1fr);gap:14px}}
figure{{margin:0;text-align:center}} img{{width:66px;height:66px;vertical-align:middle}}</style>{cells}''')


def ai_light(png):
    """The app's own icon (PNG bytes, with alpha) with the model's relief and lighting painted on."""
    from PIL import Image, ImageChops, ImageFilter
    icon = Image.open(io.BytesIO(png)).convert("RGBA")
    # The model wants 512x512 without alpha: the icon centred on grey.
    k = 448 / max(icon.size)
    fitted = icon.resize((round(icon.width * k), round(icon.height * k)), Image.LANCZOS)
    left, top = (512 - fitted.width) // 2, (512 - fitted.height) // 2
    flat = Image.new("RGB", (512, 512), "#8b8b8b")
    flat.paste(fitted, (left, top), fitted.getchannel("A"))
    # With the edge ControlNet the model may repaint far more: the icon's outlines hold its shapes.
    control = f"{AI_HOME}/models/control-canny-fp16.safetensors"
    locked = os.path.exists(control)
    with tempfile.TemporaryDirectory() as tmp:
        flat.save(f"{tmp}/in.png")
        subprocess.run(
            [f"{AI_HOME}/sd/sd-cli", "-m", f"{AI_HOME}/models/sd15-fp16.safetensors", "-i", f"{tmp}/in.png",
             "-o", f"{tmp}/out.png", "-p", AI_PROMPT, "-n", AI_NEGATIVE, "--strength", "0.62" if locked else "0.45",
             "--steps", "24", "--cfg-scale", "8", "-W", "512", "-H", "512", "--seed", "11",
             *(["--control-net", control, "--control-image", f"{tmp}/in.png", "--image-preprocess",
                "target=control,canny=true", "--control-strength", "0.9"] if locked else [])],
            check=True, capture_output=True, env={**os.environ, "LD_LIBRARY_PATH": f"{AI_HOME}/sd"})
        painted = Image.open(f"{tmp}/out.png").convert("RGB")
    # The model's shading and colour at coarse scale over the icon's own fine detail: it
    # repaints small features wrong (eyes, glyphs), so those never come from the model.
    # Without the ControlNet its shapes drift, so only a broader, grey light is trusted.
    blur = ImageFilter.GaussianBlur(4 if locked else 10)
    if not locked:
        painted = painted.convert("L").convert("RGB")
    grey = flat if locked else flat.convert("L").convert("RGB")
    change = ImageChops.add(painted.filter(blur), ImageChops.invert(grey.filter(blur)), scale=2)
    gain = 2.0 if locked else 3.2
    change = change.point(lambda v: max(0, min(255, int(128 + (v - 127.5) * gain))))
    lit = ImageChops.add(flat, change, offset=-128)
    lit = lit.crop((left, top, left + fitted.width, top + fitted.height))
    lit.putalpha(fitted.getchannel("A"))
    out = io.BytesIO()
    lit.save(out, "png")
    return out.getvalue()


def desktop_icons():
    names = set()
    for data in DATA_DIRS:
        for path in glob.glob(f"{data}/applications/*.desktop"):
            entry = configparser.ConfigParser(interpolation=None, strict=False)
            try:
                entry.read(path, encoding="utf-8")
                section = entry["Desktop Entry"]
            except (configparser.Error, KeyError, UnicodeDecodeError) as err:
                print(f"skip {path}: {err}", file=sys.stderr)
                continue
            if section.get("NoDisplay") == "true" or not section.get("Icon"):
                continue
            if os.path.isabs(section["Icon"]):
                continue  # a path bypasses the icon theme, so a restyle could never be found
            names.add(section["Icon"])
    return sorted(n for n in names if not n.startswith(SHIPPED))


def main():
    global OUT_DIR
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("icons", nargs="*")
    parser.add_argument("--out", default=OUT)
    parser.add_argument("--overrides", default=OVERRIDES)
    parser.add_argument("--report", action="store_true")
    parser.add_argument("--sheet")
    parser.add_argument("--ai", action="store_true")
    args = parser.parse_args()
    if args.ai and not os.path.exists(f"{AI_HOME}/sd/sd-cli"):
        sys.exit(f"--ai needs sd-cli in {AI_HOME}/sd and sd15-fp16.safetensors in {AI_HOME}/models")
    OUT_DIR = os.path.abspath(args.out)
    os.makedirs(OUT_DIR, exist_ok=True)
    overrides = json.load(open(args.overrides)) if os.path.exists(args.overrides) else {}
    report = []
    for name in args.icons or desktop_icons():
        target = f"{OUT_DIR}/{icon_id(name)}.svg"
        if os.path.exists(f"{HAND_DRAWN}/{icon_id(name)}.svg") or (
                os.path.exists(target) and MARK not in open(target).read(200)):
            continue  # a hand-drawn icon already matches
        source = find_source(name)
        if source is None:
            print(f"no icon found for {name}", file=sys.stderr)
            continue
        try:
            result = render(icon_id(name), source, overrides.get(name, {}), args.ai)
        except GLib.Error as err:
            print(f"cannot read {source}: {err.message}", file=sys.stderr)
            continue
        except subprocess.CalledProcessError as err:
            print(f"model failed on {name}: {err.stderr.decode()[-300:]}", file=sys.stderr)
            continue
        if result:
            report.append(result)
    if args.sheet:
        write_sheet(args.sheet, report)
    if args.report:
        json.dump([{k: v for k, v in r.items() if k not in ("before", "after")} for r in report], sys.stdout, indent=1)
        print()


OUT_DIR = OUT
if __name__ == "__main__":
    main()
