"""Draws the app icon from the mark in docs/brand/mark.py: a document sheet on a graphite tile, with
columns of ink read right to left and a vermilion seal. Needs Pillow.

Run:  python make_icon.py
Writes, next to this script, icon.png (256 px) and icon.ico (256, 64, 48, 32, 24, 16 px), plus
icon-macos.png: 1024 px with the tile inset to 824 px, the margin macOS app icons keep, for the .icns
made at release time.
"""
import struct
import sys
from io import BytesIO
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2] / "docs" / "brand"))
import mark  # noqa: E402


def png(img):
    buf = BytesIO()
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue()


mark.render(256).save(HERE / "icon.png", optimize=True)

sizes = [256, 64, 48, 32, 24, 16]
images = [png(mark.render(s)) for s in sizes]
header = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
entries = b""
for s, data in zip(sizes, images):
    entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
(HERE / "icon.ico").write_bytes(header + entries + b"".join(images))
print("icon.png and icon.ico", offset, "bytes")

inner, canvas = 824, 1024
mac = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
tile = mark.draw_vector(inner, glyph=True, edge=True)
mac.paste(tile, ((canvas - inner) // 2,) * 2, tile)
mac.save(HERE / "icon-macos.png", optimize=True)
print("icon-macos.png written")
