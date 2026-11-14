#!/bin/bash
# Verwendung: tools/release.sh <version> "<commit-nachricht>" [--publish] [--final] [--no-git]
#   (ohne Optionen)  testet, baut, packt nach dist/, committet und setzt das Tag v<version>
#   --publish        zusätzlich: Branch + Tag pushen und das GitHub-Release anlegen (braucht `gh`, angemeldet)
#   --final          Release nicht als Pre-release markieren (Standard: Pre-release)
#   --no-git         nur bauen und packen; kein Commit, kein Tag, kein Push (zum Ausprobieren)
#   --skip-smoke     Smoke-Test der Agenten-Schnittstelle (tools/smoke-mcp.ps1) und Installer-Update-Test (tools/update-test.ps1), nur Windows, überspringen
# Voraussetzungen: vorher Cargo.toml-Version setzen, `python tools/changelog.py release <version>` ausführen (verschiebt [Unreleased]
# in den neuen Abschnitt, Format Keep a Changelog) und tools/titles.txt pflegen.
# Windows: nativer MSVC-Build (target/release), Installer mit NSIS (makensis). Linux: Cross-Build nach windows-gnu.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/common.sh

V="${1:?Version fehlt}"; MSG="${2:?Commit-Nachricht fehlt}"; shift 2
PUBLISH=0; FINAL=0; NOGIT=0; SKIP_SMOKE=0
for a in "$@"; do
  case "$a" in
    --publish) PUBLISH=1 ;;
    --final) FINAL=1 ;;
    --no-git) NOGIT=1 ;;
    --skip-smoke) SKIP_SMOKE=1 ;;
    *) echo "Unbekannte Option: $a"; exit 1 ;;
  esac
done
[ "$PUBLISH" = 1 ] && [ "$NOGIT" = 1 ] && { echo "--publish und --no-git passen nicht zusammen"; exit 1; }
[ "$PUBLISH" = 1 ] && [ -z "$GH" ] && { echo "gh (GitHub CLI) nicht gefunden: winget install GitHub.cli"; exit 1; }

grep -q "^version = \"$V\"" Cargo.toml || { echo "Cargo.toml hat nicht Version $V"; exit 1; }
"$PY" tools/changelog.py check "$V"   # Keep a Changelog: Format, Reihenfolge, Datum, Links, Abschnitt [$V] vorhanden
"$PY" tools/gen_licenses.py --check   # Lizenztexte der Abhängigkeiten passen zu Cargo.lock (sonst: python tools/gen_licenses.py)
grep -q "^$V|" tools/titles.txt || { echo "tools/titles.txt hat keinen Titel für $V"; exit 1; }

cargo test --release --quiet  # bricht bei fehlschlagenden Tests ab (set -e)
cargo build --release ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"} 2>&1 | grep -E "^error|Finished"
mkdir -p dist
cp "$EXE_SRC" "dist/paneldeck-$V.exe"
if [ -n "$MAKENSIS" ]; then
  "$MAKENSIS" -V1 -DVERSION="$V" -DEXE="$(abspath "dist/paneldeck-$V.exe")" \
    -DOUTFILE="$(abspath "dist/Paneldeck-Setup-$V.exe")" installer/paneldeck.nsi
else
  echo "Hinweis: makensis nicht gefunden – kein Installer (Windows: winget install NSIS.NSIS)"
fi
sha256sum "dist/paneldeck-$V.exe" | tee "dist/paneldeck-$V.sha256"

# Ende-zu-Ende-Test der gebauten .exe (Windows): startet sie in einem Temp-Ordner und prüft die Agenten-Schnittstelle.
if [ "$IS_WIN" = 1 ] && [ "$SKIP_SMOKE" = 0 ]; then
  echo "== Smoke-Test (tools/smoke-mcp.ps1)"
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/smoke-mcp.ps1 -Exe "$(abspath "dist/paneldeck-$V.exe")"
  # Update-Test des echten Installers (alte Version -> dieser Installer, ruhender und laufender Hub; Schein-Profil, räumt auf).
  if [ -f "dist/Paneldeck-Setup-$V.exe" ]; then
    echo "== Update-Test (tools/update-test.ps1)"
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/update-test.ps1 -New "$(abspath "dist/Paneldeck-Setup-$V.exe")"
  fi
fi

if [ "$NOGIT" = 0 ]; then
  git add -A
  git commit -q -F - <<MSGEOF
$MSG

${ATTRIBUTION:-}
MSGEOF
  git tag -a "v$V" -m "Paneldeck $V"
  git log --oneline -1
fi

tools/package.sh "$V"

if [ "$PUBLISH" = 1 ]; then
  git push origin HEAD "v$V"
  ASSETS=("dist/paneldeck-$V-windows-x64.zip")
  [ -f "dist/Paneldeck-Setup-$V.exe" ] && ASSETS+=("dist/Paneldeck-Setup-$V.exe")
  PRE=(--prerelease); [ "$FINAL" = 1 ] && PRE=()
  "$GH" release create "v$V" "${ASSETS[@]}" --verify-tag ${PRE[@]+"${PRE[@]}"} \
    --title "$(cat "dist/gh-title-$V.txt")" --notes-file "dist/gh-notes-$V.md"
fi
