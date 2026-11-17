#!/usr/bin/env python3
"""Draws Nekoland's GRUB theme for one screen size, at that size.

GRUB scales a background to the screen and draws its fonts one bit deep, so a theme made
for one size looks soft or ragged on another. This draws every picture at the screen's own
pixels and sizes the menu's font to match; the text that never changes, the name and the
keys at the bottom, is drawn here smoothly instead of by GRUB.

    make-theme.py WIDTH HEIGHT OUTPUT_DIR

writes theme.txt and its pictures into OUTPUT_DIR and prints the menu font's pixel size,
for grub-mkfont.
"""

import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFont

INK = (23, 23, 26)
GLOW = (35, 35, 39)
PAPER = (244, 243, 240)
MUTED = "#8b8990"
HINT_COLOR = (109, 107, 114)
TRACK = (48, 48, 52)

# Every size below is for a 1080-line screen, scaled by the screen's own height.
BASE_HEIGHT = 1080
MENU_FONT_SIZE = 20
ITEM_HEIGHT = 48
ITEM_PADDING = 24
ITEM_SPACING = 6
HINT_FONT_SIZE = 17
# GRUB makes the countdown bar as tall as a line of its font whatever the theme asks, so its
# pictures are that tall and keep only a thin stripe across the middle.
BAR_PICTURE_HEIGHT = 28
BAR_STRIPE = 4

# The cat, drawn once at 3840x2160 as coverage, and where it sits on that screen; and the
# name under it, in Ubuntu Light, its letters spread by an eighth of their size.
LOGO = Path(__file__).with_name("logo.png")
LOGO_SCREEN = (3840, 2160)
LOGO_ORIGIN = (1824, 384)
NAME = "Nekoland"
NAME_FONT = "/usr/share/fonts/truetype/ubuntu/Ubuntu[wdth,wght].ttf"
NAME_WEIGHT = 300
NAME_SIZE = 44
NAME_TRACKING = 0.12
NAME_TOP = 345

# The glow behind the logo: an ellipse 70% by 60% of the screen around this point, fading to
# ink by 70% of the way out.
GLOW_CENTER = (0.5, 0.34)
GLOW_RADII = (0.7, 0.6)
GLOW_REACH = 0.7
GLOW_SAMPLES = (480, 270)
DITHER_SIGMA = 1.2

HINT_TEXT = "↑ ↓  選擇        Enter  開機        E  編輯        C  命令列"
HINT_FONTS = [
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", "TC"),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", None),
]
SUPERSAMPLE = 8


def scaled(value, scale):
    return max(1, round(value * scale))


def draw_glow(size):
    width, height = size
    columns, rows = GLOW_SAMPLES
    reach = [GLOW_REACH * radius for radius in GLOW_RADII]
    falloff = Image.new("L", GLOW_SAMPLES)
    falloff.putdata([
        round(255 * min(1.0, (((column + 0.5) / columns - GLOW_CENTER[0]) ** 2 / reach[0] ** 2
                              + ((row + 0.5) / rows - GLOW_CENTER[1]) ** 2 / reach[1] ** 2) ** 0.5))
        for row in range(rows) for column in range(columns)
    ])
    mask = falloff.resize(size, Image.BICUBIC)
    glow = Image.composite(Image.new("RGB", size, INK), Image.new("RGB", size, GLOW), mask)
    # The glow spans only a dozen shades, so without noise it shows as rings.
    noise = Image.effect_noise(size, DITHER_SIGMA).convert("RGB")
    return ImageChops.add(glow, noise, 1, -128)


def draw_cat(background):
    factor = background.width / LOGO_SCREEN[0]
    coverage = Image.open(LOGO).convert("L")
    coverage = coverage.resize((round(coverage.width * factor), round(coverage.height * factor)), Image.LANCZOS)
    origin = (round(LOGO_ORIGIN[0] * factor), round(LOGO_ORIGIN[1] * factor))
    background.paste(Image.new("RGB", coverage.size, PAPER), origin, coverage)


def load_name_font(pixel_size):
    font = ImageFont.truetype(NAME_FONT, pixel_size)
    axes = font.get_variation_axes()
    font.set_variation_by_axes([NAME_WEIGHT if axis["name"] in (b"Weight", "Weight") else axis["default"] for axis in axes])
    return font


def draw_name(background, scale):
    size = scaled(NAME_SIZE, scale) * SUPERSAMPLE
    font = load_name_font(size)
    tracking = NAME_TRACKING * size
    advances = [font.getlength(letter) for letter in NAME]
    width = round(sum(advances) + tracking * (len(NAME) - 1))
    top = font.getbbox(NAME)[1]
    height = font.getbbox(NAME)[3] - top
    big = Image.new("L", (width, height), 0)
    pen = ImageDraw.Draw(big)
    left = 0.0
    for letter, advance in zip(NAME, advances):
        pen.text((left, -top), letter, font=font, fill=255)
        left += advance + tracking
    coverage = big.resize((max(1, width // SUPERSAMPLE), max(1, height // SUPERSAMPLE)), Image.LANCZOS)
    origin = ((background.width - coverage.width) // 2, round(NAME_TOP * scale))
    background.paste(Image.new("RGB", coverage.size, PAPER), origin, coverage)


def draw_background(size, scale):
    background = draw_glow(size)
    draw_cat(background)
    draw_name(background, scale)
    return background


def draw_pill(height):
    """The chosen entry's paper, as the left cap, a slice of middle and the right cap."""
    big = height * SUPERSAMPLE
    pill = Image.new("RGBA", (big * 2 + SUPERSAMPLE, big), (0, 0, 0, 0))
    ImageDraw.Draw(pill).rounded_rectangle((0, 0, pill.width - 1, big - 1), radius=big // 2, fill=PAPER + (255,))
    pill = pill.resize((height * 2 + 1, height), Image.LANCZOS)
    cap = height // 2
    return {
        "w": pill.crop((0, 0, cap, height)),
        "c": pill.crop((cap, 0, cap + 1, height)),
        "e": pill.crop((pill.width - cap, 0, pill.width, height)),
    }


def draw_bar(scale, color):
    height = scaled(BAR_PICTURE_HEIGHT, scale)
    stripe = scaled(BAR_STRIPE, scale)
    bar = Image.new("RGBA", (1, height), (0, 0, 0, 0))
    bar.paste(color + (255,), (0, (height - stripe) // 2, 1, (height - stripe) // 2 + stripe))
    return {"c": bar}


def load_hint_font(pixel_size):
    for path, wanted in HINT_FONTS:
        if not Path(path).exists():
            continue
        for index in range(16):
            try:
                font = ImageFont.truetype(path, pixel_size, index=index)
            except OSError:
                break
            if wanted is None or wanted in font.getname()[0]:
                return font
    raise SystemExit("make-theme.py: needs fonts-noto-cjk for the keys at the bottom")


def draw_hint(scale):
    font = load_hint_font(scaled(HINT_FONT_SIZE, scale) * SUPERSAMPLE)
    left, top, right, bottom = font.getbbox(HINT_TEXT)
    big = Image.new("L", (right - left, bottom - top), 0)
    ImageDraw.Draw(big).text((-left, -top), HINT_TEXT, font=font, fill=255)
    coverage = big.resize((max(1, big.width // SUPERSAMPLE), max(1, big.height // SUPERSAMPLE)), Image.LANCZOS)
    hint = Image.new("RGBA", coverage.size, HINT_COLOR + (0,))
    hint.putalpha(coverage)
    return hint


def write_pieces(output, prefix, pieces):
    for side, image in pieces.items():
        image.save(output / f"{prefix}_{side}.png")


def write_theme_file(output, size, scale, hint):
    width, height = size
    font = f"Nekoland Regular {scaled(MENU_FONT_SIZE, scale)}"
    hint_left = (width - hint.width) // 2
    (output / "theme.txt").write_text(
        f"""# Nekoland's GRUB theme for {width}x{height}, drawn by make-theme.py; drawn again on reinstall.
title-text: ""
desktop-image: "background.png"
desktop-color: "#17171a"
terminal-font: "{font}"

+ boot_menu {{
  left = 35%
  width = 30%
  top = 52%
  height = 32%
  item_font = "{font}"
  selected_item_font = "{font}"
  item_color = "{MUTED}"
  selected_item_color = "#17171a"
  item_height = {scaled(ITEM_HEIGHT, scale)}
  item_padding = {scaled(ITEM_PADDING, scale)}
  item_spacing = {scaled(ITEM_SPACING, scale)}
  icon_width = 0
  icon_height = 0
  item_icon_space = 0
  selected_item_pixmap_style = "select_*.png"
  scrollbar = false
}}

+ progress_bar {{
  id = "__timeout__"
  left = 42%
  width = 16%
  top = 86%
  font = "{font}"
  show_text = false
  bar_style = "track_*.png"
  highlight_style = "fill_*.png"
}}

+ image {{
  left = {hint_left}
  top = 93%
  width = {hint.width}
  height = {hint.height}
  file = "hint.png"
}}
""",
        encoding="utf-8",
    )


def make_theme(size, output):
    scale = size[1] / BASE_HEIGHT
    output.mkdir(parents=True, exist_ok=True)
    draw_background(size, scale).save(output / "background.png")
    write_pieces(output, "select", draw_pill(scaled(ITEM_HEIGHT, scale)))
    write_pieces(output, "track", draw_bar(scale, TRACK))
    write_pieces(output, "fill", draw_bar(scale, PAPER))
    hint = draw_hint(scale)
    hint.save(output / "hint.png")
    write_theme_file(output, size, scale, hint)
    return scaled(MENU_FONT_SIZE, scale)


def main(arguments):
    if len(arguments) != 3:
        raise SystemExit(__doc__)
    width, height, output = int(arguments[0]), int(arguments[1]), Path(arguments[2])
    print(make_theme((width, height), output))


if __name__ == "__main__":
    main(sys.argv[1:])
