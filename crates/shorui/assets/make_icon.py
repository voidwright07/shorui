"""Draws the app icon: a sheet of paper on a graphite tile. Standard library only.

Run from this folder:  python make_icon.py
Writes icon.png (256 px) and icon.ico (256, 64, 48, 32, 16 px).
"""
import math
import struct
import zlib

STOCK = (244, 242, 236)
INK = (16, 17, 19)


def seg_dist(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def rounded_rect(px, py, size, radius):
    qx = abs(px - size / 2) - (size / 2 - radius)
    qy = abs(py - size / 2) - (size / 2 - radius)
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - radius


def cover(d, soft=0.7):
    return max(0.0, min(1.0, 0.5 - d / soft))


def draw(size):
    k = size / 24.0
    pts = [(7.5, 4.5), (14, 4.5), (17.5, 8), (17.5, 19.5), (7.5, 19.5), (7.5, 4.5)]
    fold = [(14, 4.5), (14, 8), (17.5, 8)]
    lines = [((10, 12), (15, 12)), ((10, 15.5), (15, 15.5))]
    stroke = max(1.0, 1.7 * k)
    rows = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            cx, cy = x + 0.5, y + 0.5
            tile = cover(rounded_rect(cx, cy, size, size * 0.22))
            d = 1e9
            for a, b in zip(pts, pts[1:]):
                d = min(d, seg_dist(cx, cy, a[0] * k, a[1] * k, b[0] * k, b[1] * k))
            for a, b in zip(fold, fold[1:]):
                d = min(d, seg_dist(cx, cy, a[0] * k, a[1] * k, b[0] * k, b[1] * k))
            if size >= 32:
                for a, b in lines:
                    d = min(d, seg_dist(cx, cy, a[0] * k, a[1] * k, b[0] * k, b[1] * k))
            ink = cover(d - stroke / 2)
            r, g, b = (round(STOCK[i] * (1 - ink) + INK[i] * ink) for i in range(3))
            row += bytes((r, g, b, round(255 * tile)))
        rows.append(bytes(row))
    return rows


def png(size):
    raw = b"".join(b"\x00" + row for row in draw(size))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


sizes = [256, 64, 48, 32, 16]
images = [png(s) for s in sizes]
open("icon.png", "wb").write(images[0])
header = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
entries = b""
for s, data in zip(sizes, images):
    entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
open("icon.ico", "wb").write(header + entries + b"".join(images))
print("icon.png", len(images[0]), "bytes; icon.ico", offset, "bytes")
