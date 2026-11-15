#!/usr/bin/env python
"""Changelogs nach Keep a Changelog 1.1.0 pflegen und prüfen – zweisprachig:

  Changelog.md     Englisch (Hauptfassung)  https://keepachangelog.com/en/1.1.0/ · https://semver.org/spec/v2.0.0.html
  Changelog.de.md  Deutsch                  https://keepachangelog.com/de/1.1.0/ · https://semver.org/lang/de/

Beide Dateien haben dieselben Versionen in derselben Reihenfolge, dieselben Daten/WIP/[YANKED]-Marken, dieselben Typ-Überschriften
je Version (`### Added` usw. auch im Deutschen englisch) und dieselben Link-Definitionen am Ende; nur der Text ist übersetzt.
Abweichung (Entscheidung des Nutzers): Es gibt keinen Abschnitt `[Unreleased]`. Noch nicht veröffentlichte Änderungen stehen
unter der Nummer des nächsten Releases mit `WIP` statt Datum: `## [1.1.0] - WIP`.

  python tools/changelog.py check [<version>]   Beide Dateien prüfen (Aufbau, Einleitung, Reihenfolge, Datum/WIP, Typen, Links) und
                                                vergleichen (gleiche Versionen, Daten, Marken, Typen, Links); mit <version>
                                                zusätzlich: dieser Abschnitt existiert in beiden, ist nicht leer, ist NICHT mehr WIP
                                                und trägt das heutige Datum (= Datum des Git-Tags)
  python tools/changelog.py new <version>       Leeren WIP-Abschnitt `## [<version>] - WIP` in beiden Dateien an der richtigen Stelle anlegen
  python tools/changelog.py release <version>   In beiden Dateien `WIP` durch das heutige Datum ersetzen und den Vergleichslink auf den Tag umstellen
  python tools/changelog.py section <version> [--lang en|de]
                                                Text des Abschnitts (ohne Überschrift) ausgeben; Standard: Englisch (Changelog.md)

Arbeitsweise: Jede Änderung, die Nutzer betrifft, gleichzeitig in BEIDE Dateien unter `## [x.y.z] - WIP` eintragen (nur
Fehlerbehebungen → nächste PATCH-Version, neue Funktionen → nächste MINOR-Version; kommt später eine Funktion dazu, wird der
Abschnitt umbenannt – in beiden Dateien). Beim Release (auf einem Vorbereitungs-Branch von release/x.y) `release <version>`.
"""
import datetime
import re
import sys

FILES = {"en": "Changelog.md", "de": "Changelog.de.md"}  # Englisch zuerst (Hauptfassung)
PATH = FILES["en"]  # Rückwärtskompatibel: frühere Aufrufer kannten nur eine Datei
INTRO = {  # Pflicht-Verweise der Einleitung je Sprache
    "en": ("https://keepachangelog.com/en/1.1.0/", "https://semver.org/spec/v2.0.0.html"),
    "de": ("https://keepachangelog.com/de/1.1.0/", "https://semver.org/lang/de/"),
}
REPO = "https://github.com/zibbozz/paneldeck"
TYPES = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"]  # Reihenfolge laut Keep a Changelog
VER = r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"
WIP = "WIP"



def read(path=PATH):
    return open(path, encoding="utf-8").read()


def write(s, path=PATH):
    open(path, "w", encoding="utf-8", newline="\n").write(s)


def semver_key(v):
    core, _, pre = v.partition("-")
    nums = tuple(int(x) for x in core.split("."))
    # Ein Pre-Release ist kleiner als die Release-Version (SemVer Punkt 11)
    return nums + ((0,) if pre else (1,)), pre


def body_of(s):
    """Text vor den Link-Definitionen am Dateiende."""
    return re.sub(r"(?ms)^\[[^\]]+\]: \S+[ \t]*\n(?:\[[^\]]+\]: \S+[ \t]*\n?)*\Z", "", s)


HEAD = r"## \[([^\]]+)\](?: - (\d{4}-\d{2}-\d{2}|WIP))?( \[YANKED\])?[ \t]*\n"


def sections(s):
    """[(Version, Datum|'WIP'|None, Textblock)] in Dateireihenfolge."""
    return [(n, d, b) for n, d, _, b in sections_full(s)]


def sections_full(s):
    """[(Version, Datum|'WIP'|None, YANKED?, Textblock)] in Dateireihenfolge."""
    out = []
    parts = re.split(r"(?m)^(?=## \[)", body_of(s))
    for p in parts[1:]:
        m = re.match(HEAD, p)
        if not m:
            out.append((p.split("\n", 1)[0], None, False, p))
            continue
        out.append((m.group(1), m.group(2), bool(m.group(3)), p[m.end():]))
    return out


def link_defs(s):
    """Link-Definitionen `[x]: url` in Dateireihenfolge."""
    return re.findall(r"(?m)^\[([^\]]+)\]: (\S+)\s*$", s)


def link_for(version, prev, wip):
    if wip:
        return f"[{version}]: {REPO}/compare/v{prev}...HEAD" if prev else f"[{version}]: {REPO}/commits"
    return f"[{version}]: {REPO}/compare/v{prev}...v{version}" if prev else f"[{version}]: {REPO}/releases/tag/v{version}"


def check_file(s, lang, version=None):
    """Prüft eine Datei; liefert Fehlertexte ohne Dateinamen."""
    errs = []
    if not s.startswith("# Changelog\n"):
        errs.append("Datei muss mit '# Changelog' beginnen")
    kac, sv = INTRO[lang]
    if kac not in s or sv not in s:
        errs.append(f"Einleitung muss auf Keep a Changelog ({kac}) und Semantic Versioning ({sv}) verweisen")
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
    links = dict(link_defs(s))
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


def outline(s):
    """Sprachunabhängiges Gerüst: je Version (Version, Datum/WIP, YANKED, Typ-Überschriften in Reihenfolge)."""
    return [(n, d, y, tuple(re.findall(r"(?m)^### (.+?)\s*$", b))) for n, d, y, b in sections_full(s)]


def compare(texts):
    """Prüft, dass alle Sprachfassungen dasselbe Gerüst und dieselben Link-Definitionen haben."""
    errs = []
    (la, sa), *rest = texts.items()
    oa = outline(sa)
    for lb, sb in rest:
        fa, fb = FILES[la], FILES[lb]
        ob = outline(sb)
        va, vb = [x[0] for x in oa], [x[0] for x in ob]
        if va != vb:
            errs.append(f"{fa} und {fb} haben nicht dieselben Versionen in derselben Reihenfolge: {', '.join(va)} ≠ {', '.join(vb)}")
        else:
            for (n, da, ya, ha), (_, db, yb, hb) in zip(oa, ob):
                if da != db:
                    errs.append(f"[{n}]: Datum/WIP unterschiedlich ({fa}: {da}, {fb}: {db})")
                if ya != yb:
                    errs.append(f"[{n}]: [YANKED] nur in {fa if ya else fb}")
                if ha != hb:
                    errs.append(f"[{n}]: Typ-Überschriften unterschiedlich ({fa}: {', '.join(ha) or '–'}; {fb}: {', '.join(hb) or '–'})")
        if link_defs(sa) != link_defs(sb):
            errs.append(f"Link-Definitionen am Dateiende unterscheiden sich zwischen {fa} und {fb}")
    return errs


def check(version=None):
    """Prüft beide Dateien und ihren Gleichlauf; Fehlertexte mit Dateinamen."""
    errs, texts = [], {}
    for lang, path in FILES.items():
        try:
            texts[lang] = read(path)
        except OSError as e:
            errs.append(f"{path}: nicht lesbar ({e})")
            continue
        errs += [f"{path}: {e}" for e in check_file(texts[lang], lang, version)]
    if len(texts) == len(FILES):
        errs += compare(texts)
    return errs


def add_wip(s, version):
    """Text mit neuem leerem WIP-Abschnitt (und Link) oder None, wenn es die Version schon gibt."""
    secs = sections(s)
    if any(n == version for n, _, _ in secs):
        return None
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
    return s


def new(version):
    if not re.fullmatch(VER, version):
        sys.exit(f"Ungültige Version: {version}")
    out = {}
    for path in FILES.values():  # erst beide vorbereiten, dann schreiben (nie nur eine Datei ändern)
        s = add_wip(read(path), version)
        if s is None:
            sys.exit(f"[{version}] steht schon in {path}")
        out[path] = s
    for path, s in out.items():
        write(s, path)
    print(f"Changelog: [{version}] - {WIP} angelegt ({', '.join(out)})")


def set_date(s, path, version, today):
    """Text mit `WIP` → heutiges Datum und Vergleichslink auf den Tag (bricht bei fehlendem/leerem/datiertem Abschnitt ab)."""
    secs = sections(s)
    sec = [x for x in secs if x[0] == version]
    if not sec:
        sys.exit(f"[{version}] steht nicht in {path} (vorher: python tools/changelog.py new {version})")
    if sec[0][1] != WIP:
        sys.exit(f"[{version}] hat in {path} schon ein Datum ({sec[0][1]})")
    if not sec[0][2].strip():
        sys.exit(f"[{version}] hat in {path} noch keine Einträge")
    s = s.replace(f"## [{version}] - {WIP}", f"## [{version}] - {today}", 1)
    older = [n for n, _, _ in secs if semver_key(n) < semver_key(version)]
    return re.sub(rf"(?m)^\[{re.escape(version)}\]: .*$", link_for(version, older[0] if older else None, False), s, count=1)


def release(version):
    if not re.fullmatch(VER, version):
        sys.exit(f"Ungültige Version: {version}")
    today = datetime.date.today().isoformat()
    out = {path: set_date(read(path), path, version, today) for path in FILES.values()}  # erst beide prüfen, dann schreiben
    for path, s in out.items():
        write(s, path)
    print(f"Changelog: [{version}] - {today} ({', '.join(out)})")


def report(errs):
    for e in errs:
        print("Changelog:", e)
    return 1 if errs else 0


def main():
    args = sys.argv[1:]
    lang = "en"
    if "--lang" in args:
        i = args.index("--lang")
        if i + 1 >= len(args) or args[i + 1] not in FILES:
            sys.exit(f"--lang braucht eine dieser Sprachen: {', '.join(FILES)}")
        lang = args[i + 1]
        del args[i:i + 2]
    cmd = args[0] if args else ""
    arg = args[1] if len(args) > 1 else None
    if cmd == "check":
        if report(check(arg)):
            sys.exit(1)
        print(f"Changelog: Format ok (Keep a Changelog 1.1.0, WIP-Variante; {' + '.join(FILES.values())} im Gleichlauf)")
    elif cmd == "new" and arg:
        new(arg)
        sys.exit(report(check()))
    elif cmd == "release" and arg:
        release(arg)
        sys.exit(report(check(arg)))
    elif cmd == "section" and arg:
        path = FILES[lang]
        sec = [x for x in sections(read(path)) if x[0] == arg]
        if not sec:
            sys.exit(f"Kein Abschnitt [{arg}] in {path}")
        print(sec[0][2].strip())
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
