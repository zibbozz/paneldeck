#!/usr/bin/env python
"""Changelog.md nach Keep a Changelog 1.1.0 (https://keepachangelog.com/de/1.1.0/) pflegen und prüfen.

Abweichung (Entscheidung des Nutzers): Es gibt keinen Abschnitt `[Unreleased]`. Noch nicht veröffentlichte Änderungen stehen
unter der Nummer des nächsten Releases mit `WIP` statt Datum: `## [1.1.0] - WIP`.

  python tools/changelog.py check [<version>]   Format prüfen (Aufbau, Reihenfolge, Datum/WIP, Typen, Links); mit <version>
                                                zusätzlich: dieser Abschnitt existiert, ist nicht leer, ist NICHT mehr WIP und
                                                trägt das heutige Datum (= Datum des Git-Tags)
  python tools/changelog.py new <version>       Leeren WIP-Abschnitt `## [<version>] - WIP` an der richtigen Stelle anlegen
  python tools/changelog.py release <version>   `WIP` durch das heutige Datum ersetzen und den Vergleichslink auf den Tag umstellen
  python tools/changelog.py section <version>   Text des Abschnitts (ohne Überschrift) ausgeben

Arbeitsweise: Während der Entwicklung Änderungen unter `## [x.y.z] - WIP` eintragen (nur Fehlerbehebungen → nächste PATCH-Version,
neue Funktionen → nächste MINOR-Version; kommt später eine Funktion dazu, wird der Abschnitt umbenannt). Beim Release (auf einem
Vorbereitungs-Branch von release/x.y) `release <version>`.
"""
import datetime
import re
import sys

PATH = "Changelog.md"
REPO = "https://github.com/zibbozz/paneldeck"
TYPES = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"]  # Reihenfolge laut Keep a Changelog
VER = r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"
WIP = "WIP"


def read():
    return open(PATH, encoding="utf-8").read()


def write(s):
    open(PATH, "w", encoding="utf-8", newline="\n").write(s)


def semver_key(v):
    core, _, pre = v.partition("-")
    nums = tuple(int(x) for x in core.split("."))
    # Ein Pre-Release ist kleiner als die Release-Version (SemVer Punkt 11)
    return nums + ((0,) if pre else (1,)), pre


def body_of(s):
    """Text vor den Link-Definitionen am Dateiende."""
    return re.sub(r"(?ms)^\[[^\]]+\]: \S+[ \t]*\n(?:\[[^\]]+\]: \S+[ \t]*\n?)*\Z", "", s)


def sections(s):
    """[(Version, Datum|'WIP'|None, Textblock)] in Dateireihenfolge."""
    out = []
    parts = re.split(r"(?m)^(?=## \[)", body_of(s))
    for p in parts[1:]:
        m = re.match(r"## \[([^\]]+)\](?: - (\d{4}-\d{2}-\d{2}|WIP))?( \[YANKED\])?[ \t]*\n", p)
        if not m:
            out.append((p.split("\n", 1)[0], None, p))
            continue
        out.append((m.group(1), m.group(2), p[m.end():]))
    return out


def link_for(version, prev, wip):
    if wip:
        return f"[{version}]: {REPO}/compare/v{prev}...HEAD" if prev else f"[{version}]: {REPO}/commits"
    return f"[{version}]: {REPO}/compare/v{prev}...v{version}" if prev else f"[{version}]: {REPO}/releases/tag/v{version}"


def check(version=None):
    s = read()
    errs = []
    if not s.startswith("# Changelog\n"):
        errs.append("Datei muss mit '# Changelog' beginnen")
    if "keepachangelog.com" not in s or "semver.org" not in s:
        errs.append("Einleitung muss auf Keep a Changelog und Semantic Versioning verweisen")
    secs = sections(s)
    if re.search(r"(?m)^## \[Unreleased\]", s):
        errs.append("Kein '[Unreleased]' mehr: noch nicht veröffentlichte Änderungen stehen unter '## [x.y.z] - WIP'")
    vers = [n for n, _, _ in secs]
    for n, d, body in secs:
        if not re.fullmatch(VER, n):
            errs.append(f"Ungültige Überschrift/Version: [{n}]")
            continue
        if d is None:
            errs.append(f"[{n}]: Datum (JJJJ-MM-TT) oder WIP fehlt")
        elif d != WIP:
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
        if not heads and d != WIP:
            errs.append(f"[{n}]: keine Änderungen (### Added/Changed/…) eingetragen")
    keys = [semver_key(v) for v in vers if re.fullmatch(VER, v)]
    if keys != sorted(keys, reverse=True):
        errs.append("Versionen müssen absteigend sortiert sein (neueste zuerst)")
    if len(set(vers)) != len(vers):
        errs.append("Version doppelt")
    # Vergleichslinks
    links = dict(re.findall(r"(?m)^\[([^\]]+)\]: (\S+)\s*$", s))
    for n in vers:
        if n not in links:
            errs.append(f"Link-Definition '[{n}]: …' am Dateiende fehlt")
    if version:
        sec = [x for x in secs if x[0] == version]
        if not sec:
            errs.append(f"Abschnitt '## [{version}] - JJJJ-MM-TT' fehlt (vorher: python tools/changelog.py release {version})")
        else:
            _, d, body = sec[0]
            if not body.strip():
                errs.append(f"Abschnitt [{version}] ist leer")
            if d == WIP:
                errs.append(f"[{version}] steht noch auf WIP (vorher: python tools/changelog.py release {version})")
            elif d != datetime.date.today().isoformat():
                errs.append(f"[{version}]: Datum {d} ist nicht heute – das Datum muss dem Tag-Datum entsprechen (Release-Vorbereitung am Release-Tag wiederholen)")
    return errs


def new(version):
    if not re.fullmatch(VER, version):
        sys.exit(f"Ungültige Version: {version}")
    s = read()
    secs = sections(s)
    if any(n == version for n, _, _ in secs):
        sys.exit(f"[{version}] steht schon im Changelog")
    older = [n for n, _, _ in secs if semver_key(n) < semver_key(version)]
    prev = older[0] if older else None
    anchor = next((n for n, _, _ in secs if semver_key(n) < semver_key(version)), None)
    block = f"## [{version}] - {WIP}\n\n"
    if anchor:
        s = s.replace(f"## [{anchor}]", block + f"## [{anchor}]", 1)
    else:
        m = re.search(r"(?m)^\[[^\]]+\]: \S+[ \t]*$", s)
        pos = m.start() if m else len(s)
        s = s[:pos].rstrip("\n") + "\n\n" + block + s[pos:]
    # Link-Definition an der passenden Stelle (Reihenfolge wie die Abschnitte: neueste zuerst)
    link = link_for(version, prev, True)
    newer = [n for n, _, _ in secs if semver_key(n) > semver_key(version)]
    if newer:
        s = re.sub(rf"(?m)^(\[{re.escape(newer[-1])}\]: .*)$", lambda mm: mm.group(1) + "\n" + link, s, count=1)
    else:
        m = re.search(r"(?m)^\[[^\]]+\]: \S+[ \t]*$", s)
        s = s[: m.start()] + link + "\n" + s[m.start():] if m else s.rstrip("\n") + "\n\n" + link + "\n"
    write(s)
    print(f"Changelog: [{version}] - {WIP} angelegt")


def release(version):
    if not re.fullmatch(VER, version):
        sys.exit(f"Ungültige Version: {version}")
    s = read()
    secs = sections(s)
    sec = [x for x in secs if x[0] == version]
    if not sec:
        sys.exit(f"[{version}] steht nicht im Changelog (vorher: python tools/changelog.py new {version})")
    if sec[0][1] != WIP:
        sys.exit(f"[{version}] hat schon ein Datum ({sec[0][1]})")
    if not sec[0][2].strip():
        sys.exit(f"[{version}] hat noch keine Einträge")
    today = datetime.date.today().isoformat()
    s = s.replace(f"## [{version}] - {WIP}", f"## [{version}] - {today}", 1)
    older = [n for n, _, _ in secs if semver_key(n) < semver_key(version)]
    s = re.sub(rf"(?m)^\[{re.escape(version)}\]: .*$", link_for(version, older[0] if older else None, False), s, count=1)
    write(s)
    print(f"Changelog: [{version}] - {today}")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    arg = sys.argv[2] if len(sys.argv) > 2 else None
    if cmd == "check":
        errs = check(arg)
        for e in errs:
            print("Changelog:", e)
        if errs:
            sys.exit(1)
        print("Changelog: Format ok (Keep a Changelog 1.1.0, WIP-Variante)")
    elif cmd == "new" and arg:
        new(arg)
        errs = check()
        for e in errs:
            print("Changelog:", e)
        sys.exit(1 if errs else 0)
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
