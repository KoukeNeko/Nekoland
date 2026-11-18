#!/usr/bin/env python3
"""Draws Nekoland's boot splash for one screen size, as a Plymouth script theme.

The splash is GRUB's picture continued: the same glow, cat and name (see ../grub/make-theme.py,
whose drawing it reuses; the task being done is written bottom left by the script), the cat's eyes closed at first, then opening and blinking, and a
short bar going back and forth under the name, since a boot has no known length.

    make-theme.py WIDTH HEIGHT OUTPUT_DIR

writes nekoland.plymouth, nekoland.script and their pictures into OUTPUT_DIR.
"""

import importlib.util
import sys
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).parent
spec = importlib.util.spec_from_file_location("grub_theme", HERE.parent / "grub" / "make-theme.py")
grub = importlib.util.module_from_spec(spec)
spec.loader.exec_module(grub)

# Sizes for a 1080-line screen, scaled by the screen's own height.
CAT_SIZE = 168
TRACK = (220, 3)
THUMB_WIDTH = 70
TEXT_SIZE = 17
SUPERSAMPLE = grub.SUPERSAMPLE
EYE_SEARCH = (30, 80, 162, 135)  # where the logo's two eyes are, in the logo's own pixels


def find_eyes(coverage):
    """Each eye's (center x, center y, half width, half height) in the logo's pixels, read
    from the holes the logo has for them."""
    left, top, right, bottom = EYE_SEARCH
    holes = coverage.point(lambda value: 255 if value < 128 else 0).crop(EYE_SEARCH)
    middle = coverage.width // 2 - left
    eyes = []
    for part, offset in ((holes.crop((0, 0, middle, holes.height)), 0),
                         (holes.crop((middle, 0, holes.width, holes.height)), middle)):
        x0, y0, x1, y1 = part.getbbox()
        eyes.append(((x0 + x1) / 2 + offset + left, (y0 + y1) / 2 + top, (x1 - x0) / 2, (y1 - y0) / 2))
    return eyes


def paper_shape(coverage_image):
    image = Image.new("RGBA", coverage_image.size, grub.PAPER + (0,))
    image.putalpha(coverage_image)
    return image


def draw_cat(factor):
    coverage = Image.open(grub.LOGO).convert("L")
    eyes = find_eyes(coverage)
    pen = ImageDraw.Draw(coverage)
    for cx, cy, rx, ry in eyes:  # the cat closed its eyes: the holes are filled
        pen.ellipse((cx - rx - 1, cy - ry - 1, cx + rx + 1, cy + ry + 1), fill=255)
    size = round(coverage.width * factor)
    return paper_shape(coverage.resize((size, size), Image.LANCZOS)), eyes


def draw_eye(rx, ry):
    width, height = max(2, round(rx * 2)), max(2, round(ry * 2))
    big = Image.new("L", (width * SUPERSAMPLE, height * SUPERSAMPLE), 0)
    ImageDraw.Draw(big).ellipse((0, 0, big.width - 1, big.height - 1), fill=255)
    coverage = big.resize((width, height), Image.LANCZOS)
    eye = Image.new("RGBA", coverage.size, grub.INK + (0,))
    eye.putalpha(coverage)
    return eye


def draw_name(scale):
    size = grub.scaled(grub.NAME_SIZE, scale) * SUPERSAMPLE
    font = grub.load_name_font(size)
    tracking = grub.NAME_TRACKING * size
    advances = [font.getlength(letter) for letter in grub.NAME]
    width = round(sum(advances) + tracking * (len(grub.NAME) - 1))
    top = font.getbbox(grub.NAME)[1]
    height = font.getbbox(grub.NAME)[3] - top
    big = Image.new("L", (width, height), 0)
    pen = ImageDraw.Draw(big)
    left = 0.0
    for letter, advance in zip(grub.NAME, advances):
        pen.text((left, -top), letter, font=font, fill=255)
        left += advance + tracking
    return paper_shape(big.resize((max(1, width // SUPERSAMPLE), max(1, height // SUPERSAMPLE)), Image.LANCZOS))


def draw_bar(width, height, color):
    big = Image.new("L", (width * SUPERSAMPLE, height * SUPERSAMPLE), 0)
    ImageDraw.Draw(big).rounded_rectangle((0, 0, big.width - 1, big.height - 1), radius=big.height // 2, fill=255)
    bar = Image.new("RGBA", (width, height), color + (0,))
    bar.putalpha(big.resize((width, height), Image.LANCZOS))
    return bar


def main(arguments):
    if len(arguments) != 3:
        raise SystemExit(__doc__)
    width, height, output = int(arguments[0]), int(arguments[1]), Path(arguments[2])
    scale = height / grub.BASE_HEIGHT
    output.mkdir(parents=True, exist_ok=True)

    grub.draw_glow((width, height)).save(output / "bg.png")
    factor = grub.scaled(CAT_SIZE, scale) / 192
    cat, eyes = draw_cat(factor)
    cat.save(output / "cat.png")
    (left_eye, right_eye) = eyes
    draw_eye(left_eye[2] * factor, left_eye[3] * factor).save(output / "eye.png")
    name = draw_name(scale)
    name.save(output / "name.png")
    track_width, track_height = grub.scaled(TRACK[0], scale), grub.scaled(TRACK[1], scale)
    draw_bar(track_width, track_height, grub.TRACK).save(output / "track.png")
    draw_bar(grub.scaled(THUMB_WIDTH, scale), track_height, grub.PAPER).save(output / "thumb.png")

    script = (HERE / "nekoland.script").read_text(encoding="utf-8")
    for key, value in {
        "LEFT_EYE_X": left_eye[0] * factor, "RIGHT_EYE_X": right_eye[0] * factor,
        "EYE_Y": left_eye[1] * factor,
        "MARGIN_X": 56 * scale, "MARGIN_BOTTOM": 48 * scale, "TEXT_SIZE": TEXT_SIZE * scale,
        "NAME_GAP": 44 * scale, "TRACK_GAP": 72 * scale, "RISE": 10 * scale,
    }.items():
        script = script.replace(f"@{key}@", f"{value:.1f}")
    (output / "nekoland.script").write_text(script, encoding="utf-8")
    (output / "nekoland.plymouth").write_text(
        "[Plymouth Theme]\nName=Nekoland\nDescription=Nekoland's boot splash\nModuleName=script\n\n"
        "[script]\nImageDir=/usr/share/plymouth/themes/nekoland\n"
        "ScriptFile=/usr/share/plymouth/themes/nekoland/nekoland.script\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main(sys.argv[1:])
