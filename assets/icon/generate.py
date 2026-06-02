#!/usr/bin/env python3
"""Generate Grove's placeholder app icon set.

Draws a rounded-square badge with a stylized branching "grove" glyph, renders
it at high resolution, then downsamples to the PNG sizes cargo-bundle expects.
Re-run after editing to regenerate: `python3 assets/icon/generate.py`.
"""
import os
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))

# Brand-ish palette: deep green badge, soft mint glyph.
BG_TOP = (24, 61, 45)      # deep forest
BG_BOT = (18, 43, 33)
GLYPH = (126, 217, 167)    # mint
GLYPH_DIM = (86, 168, 126)

SS = 1024  # master render size (supersampled)


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def rounded_mask(size, radius):
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return m


def render_master():
    img = Image.new("RGBA", (SS, SS), (0, 0, 0, 0))

    # Vertical gradient background.
    grad = Image.new("RGBA", (SS, SS))
    gp = grad.load()
    for y in range(SS):
        c = lerp(BG_TOP, BG_BOT, y / (SS - 1))
        for x in range(SS):
            gp[x, y] = (c[0], c[1], c[2], 255)

    mask = rounded_mask(SS, radius=int(SS * 0.22))
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    lw = int(SS * 0.040)

    def line(p1, p2, color, width):
        d.line([p1, p2], fill=color, width=width, joint="curve")

    def node(p, r, color):
        d.ellipse([p[0] - r, p[1] - r, p[0] + r, p[1] + r], fill=color)

    def blob(cx, cy, r, color):
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=color)
        d.ellipse([cx - int(r * 1.45), cy - int(r * 0.1),
                   cx - int(r * 0.15), cy + int(r * 1.15)], fill=color)
        d.ellipse([cx + int(r * 0.15), cy - int(r * 0.1),
                   cx + int(r * 1.45), cy + int(r * 1.15)], fill=color)
        d.ellipse([cx - int(r * 0.7), cy + int(r * 0.4),
                   cx + int(r * 0.7), cy + int(r * 1.5)], fill=color)

    def fork(cx, base, scale, color, leafy=False):
        """One Y-fork "worktree"; the center one grows a leafy canopy."""
        fk = base - int(SS * 0.22 * scale)
        l = (cx - int(SS * 0.13 * scale), fk - int(SS * 0.16 * scale))
        r = (cx + int(SS * 0.13 * scale), fk - int(SS * 0.16 * scale))
        w = int(lw * scale)
        line((cx, base), (cx, fk), color, w)
        line((cx, fk), l, color, w)
        line((cx, fk), r, color, w)
        nr = int(SS * 0.04 * scale)
        node((cx, fk), nr, color)
        if leafy:
            cr = int(SS * 0.115 * scale)
            cy = fk - int(SS * 0.16 * scale) - int(cr * 0.35)
            blob(cx, cy, cr, color)
        else:
            node(l, nr, color)
            node(r, nr, color)

    # Two bare flanking worktrees, one grown leafy tree at center.
    fork(int(SS * 0.345), int(SS * 0.76), 0.8, GLYPH_DIM)
    fork(int(SS * 0.655), int(SS * 0.76), 0.8, GLYPH_DIM)
    fork(int(SS * 0.50), int(SS * 0.80), 1.05, GLYPH, leafy=True)

    return img


def main():
    master = render_master()
    targets = {
        "32x32.png": 32,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "512x512.png": 512,
        "1024x1024.png": 1024,
    }
    for name, size in targets.items():
        out = master.resize((size, size), Image.LANCZOS)
        out.save(os.path.join(HERE, name))
        print("wrote", name)


if __name__ == "__main__":
    main()
