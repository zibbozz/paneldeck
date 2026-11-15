#!/bin/bash
# Verwendung: tools/update-feed.sh <version> <feed-ordner> [<paneldeck.exe>]
#
# Legt ein lokales Schein-Release für den Ende-zu-Ende-Test des Selbst-Updates an (nichts wird veröffentlicht):
#   <feed-ordner>/latest.json                         Antwort wie von https://api.github.com/repos/zibbozz/paneldeck/releases/latest
#   <feed-ordner>/paneldeck-<v>-windows-x64.zip       portables ZIP (Ordner paneldeck-<v>/ mit paneldeck.exe)
#   <feed-ordner>/Paneldeck-Setup-<v>.exe             Installer (nur wenn makensis vorhanden)
#   <feed-ordner>/SHA256SUMS.txt                      Prüfsummen wie in tools/release.sh
# Genutzt wird der Feed nur von einem DEBUG-Build (`cargo build` ohne --release) mit PANELDECK_UPDATE_TEST_FEED=<feed-ordner>;
# Release-Builds enthalten diesen Testweg nicht. Die „neue“ Programmdatei ist standardmäßig der Release-Build
# ($CARGO_TARGET_DIR bzw. target/release/paneldeck.exe). Optionen über Umgebungsvariablen:
#   NO_SUMS=1  kein SHA256SUMS.txt (prüft „nur ankündigen“)   BAD_SUM=1  falsche Prüfsumme für das ZIP (prüft das Verwerfen)
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/common.sh
V="${1:?Version fehlt (z. B. 9.9.9)}"
FEED="${2:?Feed-Ordner fehlt}"
EXE="${3:-${CARGO_TARGET_DIR:-target}/release/paneldeck.exe}"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Ungültige Version: $V"; exit 1; }
[ -f "$EXE" ] || { echo "Programmdatei fehlt: $EXE"; exit 1; }
mkdir -p "$FEED"
rm -f "$FEED"/latest.json "$FEED"/SHA256SUMS.txt "$FEED"/paneldeck-*-windows-x64.zip "$FEED"/Paneldeck-Setup-*.exe

"$PY" -I -c "
import zipfile,sys
v,exe,out=sys.argv[1:4]; d=f'paneldeck-{v}/'
z=zipfile.ZipFile(out,'w',zipfile.ZIP_DEFLATED,compresslevel=9)
z.write(exe, d+'paneldeck.exe')
for f in ('LICENSE','README.md'): z.write(f, d+f)
z.close()" "$V" "$EXE" "$FEED/paneldeck-$V-windows-x64.zip"

if [ -n "$MAKENSIS" ]; then
  "$MAKENSIS" -V1 -DVERSION="$V" -DEXE="$(abspath "$EXE")" -DOUTFILE="$(abspath "$FEED")\\Paneldeck-Setup-$V.exe" installer/paneldeck.nsi
fi

ASSETS=()
for f in "Paneldeck-Setup-$V.exe" "paneldeck-$V-windows-x64.zip"; do
  [ -f "$FEED/$f" ] || continue
  ASSETS+=("$f")
  if [ "${NO_SUMS:-0}" != 1 ]; then
    h="$(sha256sum "$FEED/$f" | cut -d' ' -f1)"
    [ "${BAD_SUM:-0}" = 1 ] && [[ "$f" == *.zip ]] && h="$(printf '%064d' 0)"
    printf '%s  %s\n' "$h" "$f" >> "$FEED/SHA256SUMS.txt"
  fi
done
[ -f "$FEED/SHA256SUMS.txt" ] && ASSETS+=("SHA256SUMS.txt")

json_assets=""
for f in "${ASSETS[@]}"; do
  size="$(wc -c < "$FEED/$f" | tr -d ' ')"
  [ -n "$json_assets" ] && json_assets+=","
  json_assets+="{\"name\":\"$f\",\"size\":$size,\"browser_download_url\":\"https://github.com/zibbozz/paneldeck/releases/download/v$V/$f\"}"
done
printf '{"tag_name":"v%s","name":"Paneldeck %s (local test feed)","draft":false,"prerelease":false,"html_url":"https://github.com/zibbozz/paneldeck/releases/tag/v%s","assets":[%s]}\n' "$V" "$V" "$V" "$json_assets" > "$FEED/latest.json"
ls -la "$FEED"
