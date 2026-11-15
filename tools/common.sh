#!/bin/bash
# Gemeinsame Helfer für release.sh / package.sh (wird mit `source` geladen).
# Läuft unter Windows (Git Bash, nativer MSVC-Build) und unter Linux (Cross-Build nach windows-gnu).

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) IS_WIN=1 ;;
  *) IS_WIN=0 ;;
esac

# UTF-8 für Python erzwingen (Windows nutzt sonst cp1252 und bricht an Umlauten ab).
export PYTHONUTF8=1

# Python finden. Unter Windows ist `python3` oft nur der Store-Stub → mit einem echten Aufruf prüfen.
PY=""
for c in python3 python; do
  if command -v "$c" >/dev/null 2>&1 && "$c" -c 'import sys' >/dev/null 2>&1; then PY="$c"; break; fi
done
[ -n "$PY" ] || { echo "Python nicht gefunden (python3/python)"; exit 1; }

# Build-Ziel: nativ unter Windows, sonst Cross-Build.
if [ "$IS_WIN" = 1 ]; then
  BUILD_ARGS=()
  EXE_SRC="target/release/paneldeck.exe"
else
  BUILD_ARGS=(--target x86_64-pc-windows-gnu)
  EXE_SRC="target/x86_64-pc-windows-gnu/release/paneldeck.exe"
fi

# Release-Builds sollen keine Pfade des Build-Rechners enthalten (Panik-Meldungen der Abhängigkeiten würden sonst
# den Benutzernamen aus dem Cargo-Pfad in die .exe schreiben): Pfade per rustc auf neutrale Namen abbilden.
_w() { if [ "$IS_WIN" = 1 ]; then cygpath -w "$1" 2>/dev/null || echo "$1"; else echo "$1"; fi; }
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$(_w "${CARGO_HOME:-$HOME/.cargo}")=/cargo --remap-path-prefix=$(_w "${RUSTUP_HOME:-$HOME/.rustup}")=/rustup --remap-path-prefix=$(_w "$PWD")=/paneldeck"

# Optionale Werkzeuge (auch wenn sie nicht im PATH der laufenden Shell stehen).
find_tool() { # find_tool <name> <fallback-pfad>...
  local n="$1"; shift
  if command -v "$n" >/dev/null 2>&1; then command -v "$n"; return; fi
  for p in "$@"; do [ -x "$p" ] && { echo "$p"; return; }; done
}
MAKENSIS="$(find_tool makensis "/c/Program Files (x86)/NSIS/makensis.exe" "/c/Program Files/NSIS/makensis.exe")"
GH="$(find_tool gh "/c/Program Files/GitHub CLI/gh.exe")"

# Absoluter Pfad in der Schreibweise des Zielprogramms (Windows-Programme wollen C:/..., nicht /c/...).
abspath() {
  # (NSIS' `File` findet Pfade mit Schrägstrichen nicht → unter Windows Backslashes liefern.)
  # Ordner direkt unter dem Laufwerk (E:\x): `pwd -W` liefert "E:/" → Schrägstrich am Ende abschneiden, sonst doppelter Trenner.
  if [ "$IS_WIN" = 1 ]; then local p; p="$(cd "$(dirname "$1")" && pwd -W)"; p="${p%/}/$(basename "$1")"; echo "${p//\//\\}"
  else local d; d="$(cd "$(dirname "$1")" && pwd)"; echo "${d%/}/$(basename "$1")"; fi
}
