"""Erzeugt die Release-Texte für GitHub (zweisprachig: Englisch oben, Deutsch darunter).

Englisch: docs/release-notes/<version>.en.md (von Hand geschrieben, Pflicht)   Deutsch: Abschnitt [<version>] aus Changelog.md
Titel: tools/titles.txt (englisch).   Ausgabe: dist/gh-title-<v>.txt, dist/gh-notes-<v>.md, dist/release-notes-<v>.md
"""
import os, subprocess, sys

v = sys.argv[1]


def rd(p):
    return open(p, encoding="utf-8").read()


def wr(p, t):
    open(p, "w", encoding="utf-8", newline="\n").write(t)


titles = dict(l.strip().split("|", 1) for l in open("tools/titles.txt", encoding="utf-8") if "|" in l)
sys.path.insert(0, "tools")
import changelog  # Keep a Changelog: Abschnitt [<v>] lesen

sec = [x for x in changelog.sections(rd("Changelog.md")) if x[0] == v]
if not sec:
    sys.exit(f"Changelog.md hat keinen Abschnitt [{v}]")
de = sec[0][2].strip()
en_path = f"docs/release-notes/{v}.en.md"
if not os.path.exists(en_path):
    sys.exit(f"{en_path} fehlt (englischer Release-Text, Pflicht seit 1.0.0)")
en = rd(en_path).strip()
title = titles.get(v, "Paneldeck " + v)
sha = subprocess.run(["git", "rev-list", "-n", "1", f"v{v}"], capture_output=True, text=True).stdout.strip()
body = f"**English** · [Deutsch](#deutsch)\n\n{en}\n\n---\n\n## Deutsch\n\n{de}\n"
wr(f"dist/gh-title-{v}.txt", title)
wr(f"dist/gh-notes-{v}.md", body)
wr(f"dist/release-notes-{v}.md", f"# Release v{v}\n\n**Tag:** `v{v}` (Commit `{sha}`)\n\n**Title:** {title}\n\n{body}")
