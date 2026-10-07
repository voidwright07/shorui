"""Plate 01: the Shorui mark as a specimen. Writes shorui-mark-plate.png (2400 x 3394, 1 : √2).

Run from this folder:  python plate.py
"""
import math
import os

from PIL import Image, ImageDraw, ImageFont

import mark

HERE = os.path.dirname(os.path.abspath(__file__))
ASSETS = os.path.join(HERE, "..", "..", "crates", "shorui", "assets", "fonts")
W, H = 2400, 3394
X = 2  # supersampling
M = 160

GROUND = (237, 235, 229)
INK = (28, 27, 26)
SHU = mark.SHU


def mix(a, t):
    return tuple(round(GROUND[i] * (1 - t) + a[i] * t) for i in range(3))


TEXT = mix(INK, 0.86)
LABEL = mix(INK, 0.58)
FAINT = mix(INK, 0.40)
RULE = mix(INK, 0.22)
GRID = mix(INK, 0.075)
GRID4 = mix(INK, 0.14)
GUIDE = mix(INK, 0.30)


def font(name, size, index=0):
    path = os.path.join(ASSETS, name) if name.startswith("IBM") else os.path.join("C:/Windows/Fonts", name)
    return ImageFont.truetype(path, int(size * X), index=index)


MONO = lambda s: font("IBMPlexMono-Regular.ttf", s)
SANS = lambda s: font("IBMPlexSans-Regular.ttf", s)
KAN = lambda s: font("YuGothL.ttc", s)

im = Image.new("RGB", (W * X, H * X), GROUND)
d = ImageDraw.Draw(im)


def p(v):
    return round(v * X)


def line(x0, y0, x1, y1, c=RULE, w=1):
    d.line([p(x0), p(y0), p(x1), p(y1)], fill=c, width=max(1, p(w)))


def text_w(s, f, track=0):
    return sum(f.getlength(ch) for ch in s) + p(track) * (len(s) - 1)


def text(x, y, s, f, c=LABEL, track=0, anchor="l"):
    """Draws s with its baseline at y. anchor l, r or c on x."""
    w = text_w(s, f, track) / X
    x = x - w if anchor == "r" else x - w / 2 if anchor == "c" else x
    cx = p(x)
    for ch in s:
        d.text((cx, p(y)), ch, font=f, fill=c, anchor="ls")
        cx += f.getlength(ch) + p(track)


def vtext(x, y, s, f, c=TEXT, step=1.0):
    """Japanese set top to bottom, each character centred on x, starting at y."""
    size = f.size / X
    for i, ch in enumerate(s):
        d.text((p(x), p(y + size * step * i)), ch, font=f, fill=c, anchor="mt")


def paste(img, x, y):
    im.paste(img, (p(x), p(y)), img)


# Header.
mono = MONO(21)
text(M, 214, "PLATE 01", mono, LABEL, track=4.2)
text(M + 190, 214, "MARK", mono, FAINT, track=4.2)
text(W - M, 214, "SHORUI  ·  2026", mono, LABEL, track=4.2, anchor="r")
line(M, 246, W - M, 246, RULE)

# Fig. 1: the mark.
HERO = 1240
hx, hy = (W - HERO) / 2, 360
paste(mark.draw_vector(HERO * X, glyph=True, edge=False, ss=2), hx, hy)
text(M, hy + 18, "fig. 1", mono, LABEL, track=1)
text(M, hy + 52, "the mark", mono, FAINT, track=1)

kan = KAN(132)
vtext(W - M - 66, hy - 10, "書類", kan, TEXT, step=1.12)
small = MONO(19)
# A rotated gloss beside the characters, the way Latin text runs in a vertical line.
gloss = "shorui  —  documents, papers"
gw = text_w(gloss, small, 3.2)
layer = Image.new("RGBA", (int(gw) + 8, p(30)), (0, 0, 0, 0))
ld = ImageDraw.Draw(layer)
cx = 0
for ch in gloss:
    ld.text((cx, p(22)), ch, font=small, fill=LABEL, anchor="ls")
    cx += small.getlength(ch) + p(3.2)
layer = layer.rotate(-90, expand=True)
im.paste(layer, (p(W - M - 66 - 118), p(hy)), layer)

line(M, 1720, W - M, 1720, RULE)

# Fig. 2: construction, drawn as a technical sheet on the 32-unit grid.
CX, CY, CS = 270, 1880, 840
k = CS / 32
u = lambda v: v * k
text(M, 1812, "fig. 2", mono, LABEL, track=1)
text(M + 104, 1812, "construction, 32-unit grid", mono, FAINT, track=1)
for i in range(33):
    c = GRID4 if i % 4 == 0 else GRID
    line(CX + u(i), CY, CX + u(i), CY + CS, c)
    line(CX, CY + u(i), CX + CS, CY + u(i), c)
tiny = MONO(15)
for i in range(0, 33, 8):
    text(CX + u(i), CY - 16, str(i), tiny, FAINT, anchor="c")
    text(CX - 16, CY + u(i) + 5, str(i), tiny, FAINT, anchor="r")
d.polygon([(p(CX + x), p(CY + y)) for x, y in mark.superellipse(CS / 2, CS / 2, CS / 2)], outline=LABEL, width=p(1.25))

sx0, sy0, sx1, sy1 = mark.SHEET
d.rounded_rectangle([p(CX + u(sx0)), p(CY + u(sy0)), p(CX + u(sx1)), p(CY + u(sy1))], radius=p(u(mark.SHEET_R)), outline=TEXT, width=p(1.5))
# √2: the diagonal of the sheet's top square, swung down, gives the sheet's height.
line(CX + u(sx0), CY + u(sy0), CX + u(sx1), CY + u(sy0 + 16), GUIDE)
line(CX + u(sx0), CY + u(sy0 + 16), CX + u(sx1), CY + u(sy0 + 16), GUIDE)
r = u(16 * math.sqrt(2))
ox, oy = CX + u(sx0), CY + u(sy0)
d.arc([p(ox - r), p(oy - r), p(ox + r), p(oy + r)], start=45, end=90, fill=GUIDE, width=p(1))
for x, ya, yb in mark.COLUMNS:
    h = mark.STROKE / 2
    d.rounded_rectangle([p(CX + u(x - h)), p(CY + u(ya - h)), p(CX + u(x + h)), p(CY + u(yb + h))], radius=p(u(h)), outline=TEXT, width=p(1.5))
    line(CX + u(x), CY + u(ya - h - 0.6), CX + u(x), CY + u(yb + h + 0.6), GUIDE)
numer = KAN(21)
for (x, _, _), n in zip(mark.COLUMNS, "一二三"):
    d.text((p(CX + u(x)), p(CY + u(6.15))), n, font=numer, fill=TEXT, anchor="mm")
qx0, qy0, qx1, qy1 = mark.SEAL
d.rounded_rectangle([p(CX + u(qx0)), p(CY + u(qy0)), p(CX + u(qx1)), p(CY + u(qy1))], radius=p(u(mark.SEAL_R)), fill=SHU)
cell = (qx1 - qx0) / (mark.GLYPH_CELLS + 2 * mark.GLYPH_PAD)
for gx, gy in mark.glyph_cells():
    x0 = CX + u(qx0 + (mark.GLYPH_PAD + gx) * cell)
    y0 = CY + u(qy0 + (mark.GLYPH_PAD + gy) * cell)
    d.rectangle([p(x0), p(y0), p(x0 + u(cell)) - 1, p(y0 + u(cell)) - 1], fill=mark.PAPER)


def dim_h(x0, x1, y, label):
    gap = text_w(label, tiny) / X / 2 + 10
    mid = (x0 + x1) / 2
    line(x0, y, mid - gap, y, LABEL)
    line(mid + gap, y, x1, y, LABEL)
    for x in (x0, x1):
        line(x, y - 7, x, y + 7, LABEL)
    text(mid, y + 5, label, tiny, LABEL, anchor="c")


def dim_v(x, y0, y1, label):
    gap = 16
    mid = (y0 + y1) / 2
    line(x, y0, x, mid - gap, LABEL)
    line(x, mid + gap, x, y1, LABEL)
    for y in (y0, y1):
        line(x - 7, y, x + 7, y, LABEL)
    text(x, mid + 5, label, tiny, LABEL, anchor="c")


dim_h(CX + u(sx0), CX + u(sx1), CY + u(2.9), "16")
dim_v(CX + u(26.2), CY + u(sy0), CY + u(sy1), "16√2")
dim_h(CX + u(sx1 - 2.4), CX + u(sx1), CY + u(sy1 + 1.4), "2.4")
# The seal, named below the drawing.
lx = CX + u((qx0 + qx1) / 2)
line(lx, CY + u(qy1) + 6, lx, CY + CS + 30, LABEL)
d.ellipse([p(lx - 3), p(CY + CS + 30 - 3), p(lx + 3), p(CY + CS + 30 + 3)], fill=LABEL)
d.text((p(lx + 18), p(CY + CS + 31)), "印", font=KAN(24), fill=TEXT, anchor="lm")
text(lx + 54, CY + CS + 37, "seal", tiny, LABEL, track=1)
d.text((p(lx + 54 + text_w("seal", tiny, 1) / X + 14), p(CY + CS + 37)), "白文", font=KAN(17), fill=FAINT, anchor="ls")

# Fig. 3: optical sizes, at true pixel size. 32 px and smaller are fitted to the pixel grid by hand.
RX = 1300
text(RX, 1812, "fig. 3", mono, LABEL, track=1)
text(RX + 104, 1812, "optical sizes, actual pixels", mono, FAINT, track=1)
base = 2150
SIZES = (256, 128, 64, 48, 32, 24, 16)
gap = (W - M - RX - sum(SIZES)) / (len(SIZES) - 1)
# Pixel-exact figures are placed on the final image after downsampling, at whole-pixel positions.
late = []
x = RX
for s in SIZES:
    late.append((mark.render(s), round(x), base - s))
    text(round(x) + s / 2, base + 36, str(s), tiny, LABEL, anchor="c")
    x += s + gap

# Fig. 3b: the hand-fitted sizes, enlarged with their pixel grid and spread across the column.
ey = 2276
text(RX, ey, "fig. 3b", mono, LABEL, track=1)
text(RX + 118, ey, "16, 24 and 32, fitted to the pixel", mono, FAINT, track=1)
FIT = ((16, 14), (24, 9), (32, 7))
fgap = (W - M - RX - sum(s * sc for s, sc in FIT)) / (len(FIT) - 1)
ox = RX
for s, sc in FIT:
    n = s * sc
    big = Image.new("RGBA", (n + 1, n + 1), GROUND + (255,))
    icon = mark.render(s).resize((n, n), Image.NEAREST)
    big.paste(icon, (0, 0), icon)
    g = ImageDraw.Draw(big, "RGBA")
    for i in range(s + 1):
        c = RULE + (255,) if i in (0, s) else (128, 126, 122, 84)
        g.line([i * sc, 0, i * sc, n], fill=c)
        g.line([0, i * sc, n, i * sc], fill=c)
    oy = ey + 44
    late.append((big, round(ox), oy))
    text(round(ox), oy + 224 + 36, f"{s} px  ×{sc}", tiny, LABEL)
    ox += n + fgap

# Fig. 4: the four colours.
cy = 2648
text(RX, cy, "fig. 4", mono, LABEL, track=1)
text(RX + 104, cy, "colour", mono, FAINT, track=1)
SW = 138
swatches = [("鈍", "nibi", mark.TILE), ("墨", "sumi", mark.INK), ("生成", "kinari", mark.PAPER), ("朱", "shu", SHU)]
for i, (kj, name, col) in enumerate(swatches):
    x0 = RX + i * (SW + (W - M - RX - 4 * SW) / 3)
    y0 = cy + 44
    d.rectangle([p(x0), p(y0), p(x0 + SW), p(y0 + SW)], fill=col)
    if col == mark.PAPER:
        d.rectangle([p(x0), p(y0), p(x0 + SW), p(y0 + SW)], outline=RULE, width=p(1))
    d.text((p(x0), p(y0 + SW + 26)), kj, font=KAN(26), fill=TEXT, anchor="lt")
    text(x0 + text_w(kj, KAN(26)) / X + 12, y0 + SW + 48, name, tiny, LABEL, track=1)
    text(x0, y0 + SW + 82, "#%02X%02X%02X" % col, tiny, FAINT, track=1)

line(M, 2948, W - M, 2948, RULE)

# Lockup.
LK = 150
ly = 3024
paste(mark.draw_vector(LK * X, glyph=False, edge=False, ss=4), M, ly)
word = SANS(84)
text(M + LK + 48, ly + LK / 2 + 30, "Shorui", word, TEXT, track=1.5)
ww = text_w("Shorui", word, 1.5) / X
d.text((p(M + LK + 48 + ww + 36), p(ly + LK / 2 + 30)), "書類", font=KAN(40), fill=LABEL, anchor="ls")
text(W - M, ly + LK / 2 - 4, "read right to left", mono, LABEL, track=2, anchor="r")
text(W - M, ly + LK / 2 + 30, "一  二  三  印", KAN(21), FAINT, track=0, anchor="r")

out = im.resize((W, H), Image.LANCZOS)
for img, x, y in late:
    out.paste(img, (x, y), img)
out.save(os.path.join(HERE, "shorui-mark-plate.png"), optimize=True)
print("shorui-mark-plate.png")
