#!/usr/bin/env python3
"""Rahmt einen Fenster-Screenshot für die README: Verlaufs-Hintergrund, weicher Schatten, abgerundete Ecken (Stil von docs/images/app.png).

Aufruf:  python -I tools/readme_shot_post.py <fenster.png> <ziel.png> [--margin 70] [--max-kb 400]
Benötigt Pillow (`pip install pillow`). Das Ergebnis wird bei Bedarf auf eine 256-Farben-Palette verkleinert, bis es unter --max-kb liegt.
"""
import argparse
import os
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter


def gradient(w, h):
    """Diagonaler Verlauf von dunkelblau nach dunkelviolett (wie app.png)."""
    c0, c1 = (14, 18, 32), (58, 35, 80)
    small = Image.new("RGB", (64, 64))
    px = small.load()
    for y in range(64):
        for x in range(64):
            t = (x + y) / 126.0
            px[x, y] = tuple(round(c0[i] + (c1[i] - c0[i]) * t) for i in range(3))
    return small.resize((w, h), Image.BILINEAR)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--margin", type=int, default=70)
    ap.add_argument("--radius", type=int, default=10)
    ap.add_argument("--max-kb", type=int, default=400)
    a = ap.parse_args()

    win = Image.open(a.src).convert("RGB")
    m = a.margin
    w, h = win.size
    bg = gradient(w + 2 * m, h + 2 * m + 20).convert("RGBA")
    # Schatten
    shadow = Image.new("L", bg.size, 0)
    ImageDraw.Draw(shadow).rounded_rectangle((m, m + 14, m + w, m + 14 + h), a.radius, fill=170)
    shadow = shadow.filter(ImageFilter.GaussianBlur(22))
    bg.paste(Image.new("RGBA", bg.size, (0, 0, 0, 255)), (0, 0), shadow)
    # Fenster mit abgerundeten Ecken
    mask = Image.new("L", (w * 4, h * 4), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, w * 4 - 1, h * 4 - 1), a.radius * 4, fill=255)
    mask = mask.resize((w, h), Image.LANCZOS)
    bg.paste(win, (m, m + 4), mask)
    # feiner heller Rand
    border = Image.new("L", bg.size, 0)
    d = ImageDraw.Draw(border)
    d.rounded_rectangle((m, m + 4, m + w - 1, m + 4 + h - 1), a.radius, outline=60, width=1)
    bg.paste(Image.new("RGBA", bg.size, (255, 255, 255, 255)), (0, 0), border)
    out = bg.convert("RGB")
    out.save(a.dst, optimize=True)
    if os.path.getsize(a.dst) > a.max_kb * 1024:
        out.quantize(colors=256, method=Image.Quantize.FASTOCTREE, dither=Image.Dither.NONE).save(a.dst, optimize=True)
    kb = os.path.getsize(a.dst) / 1024
    print(f"{a.dst}: {out.size[0]}x{out.size[1]}, {kb:.0f} KB")
    if kb > a.max_kb:
        print("WARNUNG: größer als erlaubt", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
