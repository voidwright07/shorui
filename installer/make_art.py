"""Draws the installer's two bitmaps in Shorui's graphite style. Needs Pillow.

Run from this folder:  python make_art.py
Writes dialog.bmp (493 x 312, the welcome and finish pages) and banner.bmp (493 x 58, the
strip across the top of the other pages). Windows Installer draws its own black text on the
right of the dialog bitmap and on the left of the banner, so those areas stay light.
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).parent
ASSETS = HERE.parent / "crates" / "shorui" / "assets"
BED = (16, 17, 19)
RULE = (38, 40, 45)
TONER_2 = (164, 167, 173)
PAPER = (255, 255, 255)
EDGE = (222, 222, 218)


def font(name, size):
    return ImageFont.truetype(str(ASSETS / "fonts" / name), size)


def icon(size):
    return Image.open(ASSETS / "icon.png").convert("RGBA").resize((size, size), Image.LANCZOS)


def dialog():
    image = Image.new("RGB", (493, 312), PAPER)
    draw = ImageDraw.Draw(image)
    # The graphite side panel, as wide as the area the dialog leaves free of text.
    draw.rectangle([0, 0, 163, 311], fill=BED)
    draw.line([164, 0, 164, 311], fill=EDGE)
    mark = icon(72)
    image.paste(mark, (46, 72), mark)
    title = font("IBMPlexSans-SemiBold.ttf", 22)
    draw.text((82, 168), "Shorui", font=title, fill=(236, 237, 238), anchor="mm")
    small = font("IBMPlexSans-Regular.ttf", 11)
    for i, line in enumerate(["Offline PDF toolbox", "Nothing leaves your machine"]):
        draw.text((82, 194 + i * 16), line, font=small, fill=TONER_2, anchor="mm")
    image.save(HERE / "dialog.bmp")


def banner():
    image = Image.new("RGB", (493, 58), PAPER)
    draw = ImageDraw.Draw(image)
    draw.line([0, 57, 492, 57], fill=EDGE)
    mark = icon(40)
    image.paste(mark, (493 - 40 - 12, 9), mark)
    image.save(HERE / "banner.bmp")


if __name__ == "__main__":
    dialog()
    banner()
    print("wrote dialog.bmp and banner.bmp")
