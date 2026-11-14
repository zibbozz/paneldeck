#!/usr/bin/env python
"""Changelog.md nach Keep a Changelog 1.1.0 (https://keepachangelog.com/de/1.1.0/) pflegen und prüfen.

  python tools/changelog.py check [<version>]   Format prüfen (Aufbau, Reihenfolge, Datum, Typen, Links); mit <version>
                                                zusätzlich: dieser Abschnitt existiert und ist nicht leer
  python tools/changelog.py release <version>   Inhalt von [Unreleased] in einen neuen Abschnitt `## [<version>] - <heute>`
                                                verschieben, leeren [Unreleased] anlegen, Vergleichslinks nachziehen
  python tools/changelog.py section <version>   Text des Abschnitts (ohne Überschrift) ausgeben

Arbeitsweise: Während der Entwicklung Änderungen unter `## [Unreleased]` eintragen; vor dem Release `release <version>`.
"""
import datetime
import re
import sys

PATH = "Changelog.md"
REPO = "https://github.com/zibbozz/paneldeck"
TYPES = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"]  # Reihenfolge laut Keep a Changelog
VER = r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"


def read():
    return open(PATH, encoding="utf-8").read()


def write(s):
    open(PATH, "w", encoding="utf-8", newline="\n").write(s)


def semver_key(v):
    core, _, pre = v.partition("-")
    nums = tuple(int(x) for x in core.split("."))
    # Ein Pre-Release ist kleiner als die Release-Version (SemVer Punkt 11)
    return nums + ((0,) if pre else (1,)), pre


def sections(s):
    """[(Name, Datum|None, Textblock)] in Dateireihenfolge; Name ist 'Unreleased' oder die Versionsnummer."""
    out = []
    s = re.sub(r"(?ms)^\[Unreleased\]: .*\Z", "", s)  # Link-Definitionen am Dateiende gehören zu keinem Abschnitt
    parts = re.split(r"(?m)^(?=## \[)", s)
    for p in parts[1:]:
        m = re.match(r"## \[([^\]]+)\](?: - (\d{4}-\d{2}-\d{2}))?( \[YANKED\])?[ \t]*\n", p)
        if not m:
            out.append((p.split("\n", 1)[0], None, p))
            continue
        out.append((m.group(1), m.group(2), p[m.end():]))
    return out


def check(version=None):
    s = read()
    errs = []
    if not s.startswith("# Changelog\n"):
        errs.append("Datei muss mit '# Changelog' beginnen")
    if "keepachangelog.com" not in s or "semver.org" not in s:
        errs.append("Einleitung muss auf Keep a Changelog und Semantic Versioning verweisen")
    secs = sections(s)
    if not secs or secs[0][0] != "Unreleased":
        errs.append("Erster Abschnitt muss '## [Unreleased]' sein")
    vers = [n for n, _, _ in secs if n != "Unreleased"]
    for n, d, body in secs:
        if n == "Unreleased":
            continue
        if not re.fullmatch(VER, n):
            errs.append(f"Ungültige Überschrift/Version: [{n}]")
            continue
        if d is None:
            errs.append(f"[{n}]: Datum (JJJJ-MM-TT) fehlt")
        else:
            try:
                datetime.date.fromisoformat(d)
            except ValueError:
                errs.append(f"[{n}]: ungültiges Datum {d}")
        heads = re.findall(r"(?m)^### (.+?)\s*$", body)
        for h in heads:
            if h not in TYPES:
                errs.append(f"[{n}]: unbekannter Typ '### {h}' (erlaubt: {', '.join(TYPES)})")
        known = [h for h in heads if h in TYPES]
        if known != sorted(known, key=TYPES.index):
            errs.append(f"[{n}]: Typen nicht in der Reihenfolge {', '.join(TYPES)}")
        if len(set(heads)) != len(heads):
            errs.append(f"[{n}]: Typ doppelt")
        if not heads:
            errs.append(f"[{n}]: keine Änderungen (### Added/Changed/…) eingetragen")
    keys = [semver_key(v) for v in vers if re.fullmatch(VER, v)]
    if keys != sorted(keys, reverse=True):
        errs.append("Versionen müssen absteigend sortiert sein (neueste zuerst)")
    if len(set(vers)) != len(vers):
        errs.append("Version doppelt")
    # Vergleichslinks
    links = dict(re.findall(r"(?m)^\[([^\]]+)\]: (\S+)\s*$", s))
    for n in ["Unreleased"] + vers:
        if n not in links:
            errs.append(f"Link-Definition '[{n}]: …' am Dateiende fehlt")
    if version:
        sec = [x for x in secs if x[0] == version]
        if not sec:
            errs.append(f"Abschnitt '## [{version}] - JJJJ-MM-TT' fehlt (vorher: python tools/changelog.py release {version})")
        elif not sec[0][2].strip():
            errs.append(f"Abschnitt [{version}] ist leer")
    return errs


def release(version):
    if not re.fullmatch(VER, version):
        sys.exit(f"Ungültige Version: {version}")
    s = read()
    secs = sections(s)
    if any(n == version for n, _, _ in secs):
        sys.exit(f"[{version}] steht schon im Changelog")
    m = re.search(r"(?ms)^## \[Unreleased\][ \t]*\n(.*?)(?=^## \[)", s)
    if not m or not m.group(1).strip():
        sys.exit("Unter [Unreleased] sind keine Änderungen eingetragen")
    body = m.group(1).strip("\n")
    prev = next((n for n, _, _ in secs if n != "Unreleased"), None)
    today = datetime.date.today().isoformat()
    s = s[: m.start()] + f"## [Unreleased]\n\n## [{version}] - {today}\n\n{body}\n\n" + s[m.end():]
    s = re.sub(r"(?m)^\[Unreleased\]: .*$", f"[Unreleased]: {REPO}/compare/v{version}...HEAD", s, count=1)
    link = f"[{version}]: {REPO}/compare/v{prev}...v{version}" if prev else f"[{version}]: {REPO}/releases/tag/v{version}"
    s = re.sub(r"(?m)^(\[Unreleased\]: .*)$", lambda mm: mm.group(1) + "\n" + link, s, count=1)
    write(s)
    print(f"Changelog: [{version}] - {today} angelegt")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    arg = sys.argv[2] if len(sys.argv) > 2 else None
    if cmd == "check":
        errs = check(arg)
        for e in errs:
            print("Changelog:", e)
        if errs:
            sys.exit(1)
        print("Changelog: Format ok (Keep a Changelog 1.1.0)")
    elif cmd == "release" and arg:
        release(arg)
        errs = check(arg)
        for e in errs:
            print("Changelog:", e)
        sys.exit(1 if errs else 0)
    elif cmd == "section" and arg:
        sec = [x for x in sections(read()) if x[0] == arg]
        if not sec:
            sys.exit(f"Kein Abschnitt [{arg}]")
        print(sec[0][2].strip())
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
