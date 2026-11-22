#!/usr/bin/env python3
"""Prüft die Bilder in README und Anleitungen (Release-Vorbereitung, CI, release.sh).

Aufruf:  python tools/check_docs_images.py [--root <repo>]

Fehler (Exit 1), wenn
  * in README*.md oder docs/en|de/*.md noch ein Platzhalter `<!-- screenshot: … -->` steht (nur
    `<!-- screenshot-manual: … -->` ist erlaubt: Bilder, die tools/docs-shots.ps1 nicht erzeugen kann, etwa das
    Explorer-Kontextmenü – sie werden von Hand gepflegt und nicht als Bild verlangt),
  * ein dort eingebundenes Bild (Markdown `![…](…)` oder `<img src="…">`, nur lokale Pfade) fehlt,
  * die Bildsätze docs/images/*.png (Englisch) und docs/images/de/*.png (Deutsch) nicht dieselben Namen haben,
  * eine englische Seite andere Bilder (Dateinamen) oder andere manuelle Platzhalter einbindet als ihre deutsche Fassung
    (README.md <-> README.de.md, docs/en/X.md <-> docs/de/X.md).
"""
import argparse
import os
import re
import sys

MARKER = re.compile(r"<!--\s*screenshot\s*:(.*?)-->", re.S)
MANUAL = re.compile(r"<!--\s*screenshot-manual\s*:(.*?)-->", re.S)
MD_IMG = re.compile(r"!\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
HTML_IMG = re.compile(r"<img\b[^>]*?\bsrc\s*=\s*[\"']([^\"']+)[\"']", re.I)


def pages(root):
    """(Sprache, Schlüssel, Pfad) aller geprüften Seiten; der Schlüssel verbindet die beiden Fassungen einer Seite."""
    out = []
    for name, lang in (("README.md", "en"), ("README.de.md", "de")):
        p = os.path.join(root, name)
        if os.path.isfile(p):
            out.append((lang, "README", p))
    for lang in ("en", "de"):
        d = os.path.join(root, "docs", lang)
        if os.path.isdir(d):
            for f in sorted(os.listdir(d)):
                if f.endswith(".md"):
                    out.append((lang, f, os.path.join(d, f)))
    return out


def images_of(text):
    """Lokale Bildpfade einer Seite (ohne http/https/data)."""
    refs = MD_IMG.findall(text) + HTML_IMG.findall(text)
    return [r.split("#")[0].split("?")[0] for r in refs if not re.match(r"^(https?:|data:|//)", r)]


def png_names(d):
    return sorted(f for f in os.listdir(d) if f.lower().endswith(".png")) if os.path.isdir(d) else []


def check(root):
    errors = []
    per_page = {}
    for lang, key, path in pages(root):
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        text = open(path, encoding="utf-8").read()
        for m in MARKER.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            errors.append(f"{rel}:{line}: Platzhalter ohne Bild: <!-- screenshot:{m.group(1).strip()[:70]} -->")
        names = []
        for ref in images_of(text):
            target = os.path.normpath(os.path.join(os.path.dirname(path), ref))
            if not os.path.isfile(target):
                errors.append(f"{rel}: Bild fehlt: {ref}")
            names.append(os.path.basename(ref))
        per_page.setdefault(key, {})[lang] = (rel, sorted(set(names)), len(MANUAL.findall(text)))
    # Gleiche Bilder in beiden Sprachen
    for key, langs in sorted(per_page.items()):
        if "en" in langs and "de" in langs:
            (re_, ne, me), (rd, nd, md) = langs["en"], langs["de"]
            if ne != nd:
                errors.append(f"{re_} <-> {rd}: verschiedene Bilder: nur EN {sorted(set(ne) - set(nd))}, nur DE {sorted(set(nd) - set(ne))}")
            if me != md:
                errors.append(f"{re_} <-> {rd}: verschieden viele manuelle Platzhalter ({me} <-> {md})")
    en = png_names(os.path.join(root, "docs", "images"))
    de = png_names(os.path.join(root, "docs", "images", "de"))
    if en != de:
        errors.append(f"docs/images <-> docs/images/de: verschiedene Bildsätze: nur EN {sorted(set(en) - set(de))}, nur DE {sorted(set(de) - set(en))}")
    return errors, len(en)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    a = ap.parse_args()
    errors, n = check(os.path.abspath(a.root))
    for e in errors:
        print("FEHLER:", e)
    if errors:
        print(f"{len(errors)} Fehler - Bilder neu erzeugen: powershell -ExecutionPolicy Bypass -File tools\\docs-shots.ps1")
        sys.exit(1)
    print(f"Doku-Bilder in Ordnung ({n} Bilder je Sprache, keine offenen Platzhalter).")


if __name__ == "__main__":
    main()
