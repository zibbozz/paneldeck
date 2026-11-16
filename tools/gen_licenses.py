#!/usr/bin/env python3
"""Sammelt die Lizenztexte aller Abhängigkeiten (cargo metadata) in THIRD_PARTY_LICENSES.txt.

Verwendung:  python tools/gen_licenses.py          -> schreibt THIRD_PARTY_LICENSES.txt
             python tools/gen_licenses.py --check  -> bricht ab, wenn die Datei veraltet ist (Cargo.lock geändert)

MIT, Apache-2.0, BSD u. a. verlangen, dass beim Weitergeben der fertigen Programmdatei die Lizenztexte mitgeliefert werden.
Gleiche Texte (gleicher Inhalt) werden nur einmal ausgegeben, davor steht, für welche Pakete sie gelten.
"""
import hashlib, json, pathlib, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY_LICENSES.txt"
EXTRA = ROOT / "tools" / "license-extra"   # Originaltexte aus den Upstream-Repos für Pakete, deren Crate-Archiv keine Lizenzdatei enthält
EXTRA_MAP = {
    "egui-MIT.txt egui-Apache.txt": "ecolor eframe egui egui-wgpu egui-winit egui_glow emath epaint",
    "egui-MIT.txt egui-Apache.txt egui-fonts-OFL.txt egui-fonts-UFL.txt": "epaint_default_fonts",
    "wgpu-MIT.txt wgpu-Apache.txt": "naga wgpu wgpu-core wgpu-core-deps-windows-linux-android wgpu-hal wgpu-types",
    "profiling-MIT.txt": "profiling",
    "glrs-Apache.txt": "gl_generator khronos_api",
    "clipboardwin-BSL.txt": "clipboard-win",
    "hexf-CC0.txt": "hexf-parse",
}
EXTRA_BY_PKG = {pkg: files.split() for files, pkgs in EXTRA_MAP.items() for pkg in pkgs.split()}
# Mitgelieferte Programmdateien, die keine Crates sind (Version aus dem Fetch-Skript).
FETCH_CONPTY = ROOT / "tools" / "fetch-conpty.ps1"
WANT = ("license", "licence", "copying", "copyright", "notice", "unlicense")


def collect():
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", "x86_64-pc-windows-msvc"],
        cwd=ROOT, capture_output=True, text=True, check=True, encoding="utf-8").stdout)
    own = {p["id"] for p in meta["packages"] if p["name"] == "paneldeck"}
    entries = []  # (name, version, license, [(dateiname, text)])
    for p in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
        if p["id"] in own:
            continue
        d = pathlib.Path(p["manifest_path"]).parent
        texts = []
        for f in sorted(d.iterdir()):
            if f.is_file() and f.name.lower().startswith(WANT) and f.suffix.lower() in ("", ".txt", ".md"):
                try:
                    texts.append((f.name, f.read_text(encoding="utf-8").strip()))
                except (UnicodeDecodeError, OSError):
                    pass
        if not texts:
            for fn in EXTRA_BY_PKG.get(p["name"], []):
                texts.append((fn, (EXTRA / fn).read_text(encoding="utf-8").strip()))
        entries.append([p["name"], p["version"], p.get("license") or "(siehe Datei)", texts, p.get("repository")])
    # Pakete ohne eigene Lizenzdatei (Workspaces): Texte eines Geschwister-Pakets aus demselben Repository übernehmen.
    by_repo = {}
    for e in entries:
        if e[3] and e[4]:
            by_repo.setdefault(e[4], e[3])
    for e in entries:
        if not e[3] and e[4] in by_repo:
            e[3] = by_repo[e[4]]
    return [tuple(e[:4]) for e in entries]


def bundled():
    """(Name, Version, Lizenz, Lizenztext) der mitgelieferten Programmdateien."""
    import re
    m = re.search(r"^\$Version = '([^']+)'", FETCH_CONPTY.read_text(encoding="utf-8"), re.M)
    if not m:
        sys.exit("tools/fetch-conpty.ps1: $Version nicht gefunden")
    return [("Microsoft.Windows.Console.ConPTY (conpty.dll, OpenConsole.exe)", m.group(1), "MIT",
             (EXTRA / "conpty-MIT.txt").read_text(encoding="utf-8").strip())]


def render():
    entries = collect()
    by_text = {}   # sha -> (text, [pakete])
    missing = []
    for name, ver, lic, texts in entries:
        if not texts:
            missing.append(f"{name} {ver} ({lic})")
        for _fn, t in texts:
            h = hashlib.sha256(" ".join(t.split()).encode()).hexdigest()
            by_text.setdefault(h, (t, []))[1].append(f"{name} {ver}")
    out = ["Lizenztexte der Abhängigkeiten von Paneldeck", "=" * 46, "",
           "Paneldeck selbst steht unter der MIT-Lizenz (Datei LICENSE). Die folgenden Pakete werden in die Programmdatei",
           "eingebunden; ihre Lizenztexte stehen unten (gleiche Texte nur einmal). Die eingebetteten Schriften haben eigene",
           "Lizenzdateien im Ordner assets/fonts/ (SIL Open Font License 1.1).", "",
           "Pakete und ihre Lizenz-Ausdrücke:", ""]
    for name, ver, lic, _ in entries:
        out.append(f"  {name} {ver} - {lic}")
    if missing:
        out += ["", "Pakete ohne eigene Lizenzdatei im Quellpaket (Lizenz laut Cargo.toml oben):"] + [f"  {m}" for m in missing]
    out += ["", "Mitgelieferte Programmdateien (neben paneldeck.exe, unverändert):", ""]
    extra = bundled()
    for name, ver, lic, _ in extra:
        out.append(f"  {name} {ver} - {lic}")
    out.append("")
    for i, (t, pk) in enumerate(sorted(by_text.values(), key=lambda v: v[1][0]), 1):
        out += ["", "-" * 78, f"Text {i} – gilt für: " + ", ".join(sorted(set(pk))), "-" * 78, "", t, ""]
    for name, ver, _lic, t in extra:
        out += ["", "-" * 78, f"Mitgeliefert: {name} {ver}", "-" * 78, "", t, ""]
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    text = render()
    if "--check" in sys.argv:
        old = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        if old.replace("\r\n", "\n") != text:
            sys.exit("THIRD_PARTY_LICENSES.txt ist veraltet: python tools/gen_licenses.py")
        print("THIRD_PARTY_LICENSES.txt aktuell")
    else:
        OUT.write_bytes(text.encode("utf-8"))
        print(f"geschrieben: {OUT.name} ({len(text) // 1024} KB)")
