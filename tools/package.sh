#!/bin/bash
# Verwendung: tools/package.sh <version>
#   -> dist/paneldeck-<version>-windows-x64.zip  (portabel: paneldeck.exe + Lizenzen + README, in einem Ordner paneldeck-<version>/)
#   Der Installer (dist/Paneldeck-Setup-<version>.exe, mit Lizenzen) wird von release.sh gebaut und ist das zweite Release-Asset.
# Erzeugt außerdem dist/release-notes-<version>.md sowie dist/gh-title-<version>.txt und dist/gh-notes-<version>.md (Titel/Text für das GitHub-Release).
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/common.sh
V="${1:?Version fehlt}"
"$PY" tools/release_notes.py "$V"
rm -f "dist/paneldeck-$V-windows-x64.zip"
"$PY" -c "
import zipfile,sys
v=sys.argv[1]; d=f'paneldeck-{v}/'
z=zipfile.ZipFile(f'dist/paneldeck-{v}-windows-x64.zip','w',zipfile.ZIP_DEFLATED,compresslevel=9)
z.write(f'dist/paneldeck-{v}.exe', d+'paneldeck.exe')   # im Ordner heißt die Datei paneldeck.exe (Hooks/MCP-Eintrag verweisen auf den Namen)
for f in ('LICENSE','THIRD_PARTY.md','THIRD_PARTY_LICENSES.txt','README.md','README.de.md','Changelog.md'): z.write(f, d+f)
for f in ('LICENSE-Cascadia.txt','LICENSE-NotoColorEmoji.txt'): z.write('assets/fonts/'+f, d+'licenses/'+f)
z.close()" "$V"
ls -la "dist/paneldeck-$V-windows-x64.zip"
