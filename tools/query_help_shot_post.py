#!/usr/bin/env python3
"""Schneidet einen Fenster-Screenshot der Suche (tools/query-help-shots.ps1) auf das Bild der Abfrage-Hilfe zu.

Das Ergebnis besteht aus zwei Streifen des Fensters, untereinander gesetzt:
  1. die Abfrage-Leiste (Abfragetext, „Search“, Fragezeichen),
  2. Trefferzahl und Ergebnistabelle bis zur letzten Zeile (bzw. der Hinweis „No ticket matches the query.“).
Die Knopfzeile dazwischen (History, Saved filters …) und alles darüber (Reiter, Hinweistext) fallen weg. Die Streifen werden am
Bild erkannt (Blöcke ab SEARCH_TOP: Abfragefeld – ein- oder zweizeilig –, Knopfzeile, dann Trefferzahl und Tabelle); das gilt für
die Fenstergröße des Skripts (960 px breit, Projekt „Web shop“). Danach eine 256-Farben-Palette (die Bilder werden per
include_bytes! in die .exe eingebettet).

Aufruf:  python -I tools/query_help_shot_post.py <fenster.png> <ziel.png> [--max-kb 120]
Benötigt Pillow (`pip install pillow`).
"""
import argparse
import os
import sys

from PIL import Image

SEARCH_TOP = 222  # oberhalb des Abfragefelds (unter „Search in“)
SIDE = 8          # Rand links/rechts
PAD = 8           # Abstand über/unter den Streifen


def differs(p, bg, tol=12):
    return sum(abs(a - b) for a, b in zip(p, bg)) > tol


def blocks(im, top, bg):
    """Zusammenhängende Zeilenbereiche (von, bis) ab `top`, die sich vom Hintergrund unterscheiden (ohne Fensterrand unten)."""
    w, h = im.size
    px = im.load()
    out, start = [], None
    for y in range(top, h - 5):
        busy = any(differs(px[x, y], bg) for x in range(SIDE + 4, w - SIDE - 4, 2))
        if busy and start is None:
            start = y
        elif not busy and start is not None:
            out.append((start, y))
            start = None
    if start is not None:
        out.append((start, h - 5))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--max-kb", type=int, default=120)
    a = ap.parse_args()

    win = Image.open(a.src).convert("RGB")
    w, h = win.size
    bg = win.getpixel((w // 2, h - 20))
    b = blocks(win, SEARCH_TOP, bg)
    if len(b) < 3:
        print(f"FEHLER: {a.src}: Abfragefeld, Knopfzeile und Ergebnis nicht gefunden: {b}", file=sys.stderr)
        sys.exit(1)
    bottom = b[-1][1] + PAD
    if bottom >= h - 6:
        print(f"FEHLER: {a.src}: die Ergebnisse reichen bis zum Fensterrand (Fenster höher machen)", file=sys.stderr)
        sys.exit(1)
    bar = win.crop((SIDE, b[0][0] - PAD, w - SIDE, b[0][1] + PAD))
    res = win.crop((SIDE, b[2][0] - PAD, w - SIDE, bottom))
    out = Image.new("RGB", (w - 2 * SIDE, bar.height + res.height), bg)
    out.paste(bar, (0, 0))
    out.paste(res, (0, bar.height))
    out.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE).save(a.dst, optimize=True)
    kb = os.path.getsize(a.dst) / 1024
    print(f"{a.dst}: {out.size[0]}x{out.size[1]}, {kb:.0f} KB")
    if kb > a.max_kb:
        print("WARNUNG: größer als erlaubt", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
