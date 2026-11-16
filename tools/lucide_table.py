#!/usr/bin/env python3
"""Erzeugt aus den Lucide-Paketen die eingebetteten Dateien (aufgerufen von tools/update-lucide.ps1).

Verwendung:  python -I tools/lucide_table.py <lucide-static/package> <lucide-Quelle> <Version> <Repo-Wurzel>

  <lucide-static/package>  entpacktes npm-Paket lucide-static (font/lucide.ttf, font/info.json, tags.json, LICENSE)
  <lucide-Quelle>          entpackter Quell-Stand von GitHub zum selben Tag (icons/*.json mit Kategorien, categories/*.json)

Schreibt:
  assets/fonts/lucide.ttf               Schrift (unverändert)
  assets/fonts/LICENSE-Lucide.txt       Lizenz (ISC + MIT für die Feather-Icons)
  tools/license-extra/lucide-LICENSE.txt  dieselbe Lizenz für THIRD_PARTY_LICENSES.txt (tools/gen_licenses.py)
  assets/lucide/icons.tsv               Tabelle Name -> Zeichen, Kategorien, Stichwörter

Aufbau von icons.tsv (UTF-8, LF): Kommentarzeilen mit '#', Kategorien als '@<TAB>schluessel<TAB>Titel',
dann je Icon 'name<TAB>codepoint-hex<TAB>kat,kat<TAB>stichwort,stichwort' (nach Name sortiert).
Nur kanonische Namen (keine veralteten Aliase); jedes Zeichen muss in der Schrift vorhanden sein.
"""
import json, os, re, shutil, struct, sys


def cmap_of(data):
    """Unicode -> Glyph aus der cmap (Format 4 und 12)."""
    num = struct.unpack(">H", data[4:6])[0]
    tables = {}
    for i in range(num):
        tag, _cs, off, ln = struct.unpack(">4sIII", data[12 + 16 * i:28 + 16 * i])
        tables[tag.decode("latin-1")] = (off, ln)
    o = tables["cmap"][0]
    out = {}
    for i in range(struct.unpack(">H", data[o + 2:o + 4])[0]):
        _pid, _eid, sub = struct.unpack(">HHI", data[o + 4 + 8 * i:o + 12 + 8 * i])
        so = o + sub
        fmt = struct.unpack(">H", data[so:so + 2])[0]
        if fmt == 4:
            segx2 = struct.unpack(">H", data[so + 6:so + 8])[0]
            seg = segx2 // 2
            ends = struct.unpack(">%dH" % seg, data[so + 14:so + 14 + segx2])
            starts = struct.unpack(">%dH" % seg, data[so + 16 + segx2:so + 16 + 2 * segx2])
            deltas = struct.unpack(">%dh" % seg, data[so + 16 + 2 * segx2:so + 16 + 3 * segx2])
            ro = so + 16 + 3 * segx2
            ranges = struct.unpack(">%dH" % seg, data[ro:ro + segx2])
            for s in range(seg):
                for c in range(starts[s], ends[s] + 1):
                    if c == 0xFFFF:
                        continue
                    if ranges[s] == 0:
                        g = (c + deltas[s]) & 0xFFFF
                    else:
                        a = ro + 2 * s + ranges[s] + 2 * (c - starts[s])
                        g = struct.unpack(">H", data[a:a + 2])[0]
                        if g:
                            g = (g + deltas[s]) & 0xFFFF
                    if g:
                        out[c] = g
        elif fmt == 12:
            n = struct.unpack(">I", data[so + 12:so + 16])[0]
            for k in range(n):
                a, b, g = struct.unpack(">III", data[so + 16 + 12 * k:so + 28 + 12 * k])
                for c in range(a, b + 1):
                    out[c] = g + (c - a)
    return out


NAME = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")


def clean(word):
    w = " ".join(str(word).replace("\t", " ").replace(",", " ").split()).lower()
    return w


def categories_of(src, name, rd, depth=0):
    """Kategorien eines Icons; `$extends:<icon>` übernimmt die Kategorien eines anderen Icons."""
    path = os.path.join(src, "icons", name + ".json")
    if depth > 8 or not os.path.exists(path):
        return []
    out = []
    for c in rd(path).get("categories", []):
        found = categories_of(src, c[len("$extends:"):], rd, depth + 1) if c.startswith("$extends:") else [c]
        for x in found:
            if x not in out:
                out.append(x)
    return out


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    pkg, src, version, root = sys.argv[1:5]
    rd = lambda *p: json.load(open(os.path.join(*p), encoding="utf-8"))
    info = rd(pkg, "font", "info.json")
    tags = rd(pkg, "tags.json")
    font = open(os.path.join(pkg, "font", "lucide.ttf"), "rb").read()
    cmap = cmap_of(font)
    cats = {}
    for f in sorted(os.listdir(os.path.join(src, "categories"))):
        if f.endswith(".json"):
            cats[f[:-5]] = rd(src, "categories", f)["title"]
    rows = []
    for name in sorted(tags):
        if name not in info:
            continue
        if not NAME.match(name) or len(name) > 64:
            sys.exit(f"ungültiger Name: {name}")
        cp = int(info[name]["encodedCode"].lstrip("\\"), 16)
        if cp not in cmap:
            sys.exit(f"{name}: Zeichen {cp:X} fehlt in der Schrift")
        icats = categories_of(src, name, rd)
        for c in icats:
            if c not in cats:
                sys.exit(f"{name}: unbekannte Kategorie {c}")
        words = []
        for t in tags[name]:
            w = clean(t)
            if w and w not in words and w != name:
                words.append(w)
        rows.append((name, cp, icats, words))
    if len(rows) < 1000:
        sys.exit(f"zu wenige Icons: {len(rows)}")
    if len({r[1] for r in rows}) != len(rows):
        sys.exit("doppelte Zeichen")
    used = sorted({c for r in rows for c in r[2]})
    lines = [f"# Lucide {version} (npm lucide-static, ISC; categories from github.com/lucide-icons/lucide tag {version})",
             "# Generated by tools/update-lucide.ps1 - do not edit by hand.",
             f"# icons={len(rows)} categories={len(used)}"]
    lines += [f"@\t{c}\t{cats[c]}" for c in used]
    lines += [f"{n}\t{cp:x}\t{','.join(c)}\t{','.join(w)}" for n, cp, c, w in rows]
    os.makedirs(os.path.join(root, "assets", "lucide"), exist_ok=True)
    with open(os.path.join(root, "assets", "lucide", "icons.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    shutil.copyfile(os.path.join(pkg, "font", "lucide.ttf"), os.path.join(root, "assets", "fonts", "lucide.ttf"))
    lic = open(os.path.join(pkg, "LICENSE"), encoding="utf-8").read().replace("\r\n", "\n")
    for p in (os.path.join(root, "assets", "fonts", "LICENSE-Lucide.txt"), os.path.join(root, "tools", "license-extra", "lucide-LICENSE.txt")):
        with open(p, "w", encoding="utf-8", newline="\n") as f:
            f.write(lic)
    print(f"Lucide {version}: {len(rows)} Icons, {len(used)} Kategorien")


if __name__ == "__main__":
    main()
