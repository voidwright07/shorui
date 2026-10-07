"""The Shorui mark: 書類, a document sheet. Columns of ink run top to bottom and are read right to left
(tategaki), and a vermilion seal sits where the sender signs.

Geometry is on a 32-unit tile. Sizes of 32 px and below use hand-placed pixel geometry instead.
"""
import math

from PIL import Image, ImageDraw

TILE = (22, 23, 26)
PAPER = (244, 242, 236)
INK = (28, 27, 26)
SHU = (214, 58, 38)
EDGE = (255, 255, 255, 18)

U = 32.0
SHEET = (8.0, 4.7, 24.0, 27.3)  # 16 x 22.6 units: A-series, 1 : √2
SHEET_R = 0.45
STROKE = 1.5
# Columns from right to left: a full line, a shorter one, a closing line, then the seal.
# Margins are 2.4 units at the sides and 2.95 at head and foot. Columns sit 3.5 apart (a 2-unit gap), the
# seal keeps the same gap to the column beside it, and its right edge lines up with the closing line.
COLUMNS = [(20.85, 8.4, 23.6), (17.35, 8.4, 20.6), (13.85, 8.4, 12.0)]
SEAL = (10.4, 20.15, 14.6, 24.35)  # 4.2 units square, its foot level with the first column's
SEAL_R = 0.35

# 書 in square seal script, on a 13-cell grid, knocked out of the seal (白文, white-character seal).
GLYPH = [
    ("h", 0, 2, 10), ("v", 10, 0, 4), ("h", 2, 2, 10), ("h", 4, 0, 12), ("v", 6, 0, 6), ("h", 6, 1, 11),
    ("h", 8, 3, 9), ("h", 10, 3, 9), ("h", 12, 3, 9), ("v", 3, 8, 12), ("v", 9, 8, 12),
]
GLYPH_CELLS = 13
GLYPH_PAD = 1.5  # cells of margin inside the seal


def superellipse(cx, cy, r, n=5.0, steps=1440):
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        pts.append((cx + r * math.copysign(abs(c) ** (2 / n), c), cy + r * math.copysign(abs(s) ** (2 / n), s)))
    return pts


def glyph_cells():
    cells = set()
    for kind, a, b, c in GLYPH:
        for k in range(b, c + 1):
            cells.add((k, a) if kind == "h" else (a, k))
    return cells


def draw_vector(size, glyph=True, edge=True, tile=True, ss=4):
    S = size * ss
    k = S / U
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    if tile:
        d.polygon(superellipse(S / 2, S / 2, S / 2), fill=TILE)
        if edge:
            inner = Image.new("L", (S, S), 0)
            ImageDraw.Draw(inner).polygon(superellipse(S / 2, S / 2, S / 2 - max(ss, k * 0.06)), fill=255)
            outer = Image.new("L", (S, S), 0)
            ImageDraw.Draw(outer).polygon(superellipse(S / 2, S / 2, S / 2), fill=255)
            ring = Image.composite(Image.new("L", (S, S), 0), outer, inner)
            im.paste(Image.new("RGBA", (S, S), EDGE[:3] + (255,)), (0, 0), ring.point(lambda v: v * EDGE[3] // 255))
    x0, y0, x1, y1 = SHEET
    d.rounded_rectangle([x0 * k, y0 * k, x1 * k, y1 * k], radius=SHEET_R * k, fill=PAPER)
    w = STROKE * k
    for x, ya, yb in COLUMNS:
        d.rectangle([(x - STROKE / 2) * k, ya * k, (x + STROKE / 2) * k, yb * k], fill=INK)
        d.ellipse([(x - STROKE / 2) * k, ya * k - w / 2, (x + STROKE / 2) * k, ya * k + w / 2], fill=INK)
        d.ellipse([(x - STROKE / 2) * k, yb * k - w / 2, (x + STROKE / 2) * k, yb * k + w / 2], fill=INK)
    sx0, sy0, sx1, sy1 = SEAL
    d.rounded_rectangle([sx0 * k, sy0 * k, sx1 * k, sy1 * k], radius=SEAL_R * k, fill=SHU)
    if glyph:
        cell = (sx1 - sx0) / (GLYPH_CELLS + 2 * GLYPH_PAD)
        for cx, cy in glyph_cells():
            gx = sx0 + (GLYPH_PAD + cx) * cell
            gy = sy0 + (GLYPH_PAD + cy) * cell
            d.rectangle([gx * k, gy * k, (gx + cell) * k - 1, (gy + cell) * k - 1], fill=PAPER)
    return im.resize((size, size), Image.LANCZOS)


# Pixel-fitted small sizes: (tile radius, sheet, columns [(x, w, y0, y1)], seal) in whole pixels.
PIXEL = {
    16: dict(sheet=(4, 2, 12, 15), cols=[(10, 1, 4, 13), (7, 1, 4, 8)], seal=(5, 10, 8, 13)),
    24: dict(sheet=(6, 3, 18, 21), cols=[(16, 1, 5, 19), (13, 1, 5, 16), (10, 1, 5, 8)], seal=(7, 15, 11, 19)),
    32: dict(sheet=(8, 5, 24, 27), cols=[(20, 2, 8, 24), (16, 2, 8, 21), (12, 2, 8, 12)], seal=(10, 20, 14, 24)),
}


def draw_pixel(size):
    g = PIXEL[size]
    im = draw_vector(size, glyph=False, edge=False)  # tile silhouette, antialiased
    base = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    base.paste(im, (0, 0))
    px = base.load()
    for y in range(size):
        for x in range(size):
            if px[x, y][3] > 0:
                px[x, y] = TILE + (px[x, y][3],)

    def fill(x0, y0, x1, y1, c):
        for y in range(y0, y1):
            for x in range(x0, x1):
                px[x, y] = c + (255,)

    fill(*g["sheet"], PAPER)
    for x, w, ya, yb in g["cols"]:
        fill(x, ya, x + w, yb, INK)
    fill(*g["seal"], SHU)
    return base


def render(size):
    if size in PIXEL:
        return draw_pixel(size)
    return draw_vector(size, glyph=size >= 512, edge=size >= 128)


if __name__ == "__main__":
    import sys

    out = sys.argv[1] if len(sys.argv) > 1 else "."
    for s in (1024, 512, 256, 128, 64, 48, 32, 24, 16):
        render(s).save(f"{out}/mark-{s}.png")
