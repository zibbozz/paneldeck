#!/bin/bash
# Verwendung: tools/release-prep.sh <version> "<englischer Titel>" [--pr]
#
# Bereitet ein Release vor: legt von origin/release/<x.y> den Branch release-prep/<version> an und committet dort
#   Cargo.toml/Cargo.lock-Version, Changelog (`WIP` -> heutiges Datum, = späteres Tag-Datum) und den Titel in tools/titles.txt.
# Vorher muss docs/release-notes/<version>.en.md (englischer Release-Text) im Branch stehen bzw. hier hinzugefügt sein.
# Mit --pr wird der Branch gepusht und ein Pull Request nach release/<x.y> geöffnet. Gemergt wird nur auf GitHub durch den Projektinhaber;
# danach auf release/<x.y> (neu gezogen, Datum muss noch „heute“ sein): tools/release.sh <version> --publish.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/common.sh
V="${1:?Version fehlt}"; TITLE="${2:?Englischer Titel fehlt}"; shift 2
PR=0; [ "${1:-}" = "--pr" ] && PR=1
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Ungültige Version: $V"; exit 1; }
REL="release/${V%.*}"; BR="release-prep/$V"
[ -z "$(git status --porcelain)" ] || { echo "Arbeitsbaum nicht sauber"; exit 1; }
git fetch -q origin "$REL"
git switch -q -c "$BR" "origin/$REL"
sed -i "0,/^version = \".*\"/s//version = \"$V\"/" Cargo.toml
"$PY" - "$V" <<'PYEOF'
import re, sys
v = sys.argv[1]
t = open("Cargo.lock", encoding="utf-8", newline="").read()
t2 = re.sub(r'(name = "paneldeck"\r?\nversion = )"[^"]*"', lambda m: m.group(1) + f'"{v}"', t, count=1)
open("Cargo.lock", "w", encoding="utf-8", newline="").write(t2)
PYEOF
"$PY" tools/changelog.py release "$V"
grep -q "^$V|" tools/titles.txt && sed -i "/^$V|/d" tools/titles.txt
printf '%s|%s\n' "$V" "$TITLE" >> tools/titles.txt
[ -f "docs/release-notes/$V.en.md" ] || echo "ACHTUNG: docs/release-notes/$V.en.md fehlt noch (englischer Release-Text) – vor dem Commit anlegen!"
"$PY" tools/gen_licenses.py --check
git add -A
git commit -q -F - <<MSGEOF
Release-Vorbereitung $V

${ATTRIBUTION:-}
MSGEOF
git log --oneline -1
if [ "$PR" = 1 ]; then
  git push -u origin "$BR"
  "$GH" pr create --base "$REL" --head "$BR" --title "Release $V vorbereiten" \
    --body "Setzt Version, Changelog-Datum und Titel für **$V**. Nach dem Merge: \`tools/release.sh $V --publish\` auf \`$REL\` (Datum im Changelog muss dem Tag-Datum entsprechen)."
fi
