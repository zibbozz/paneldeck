#!/bin/bash
# Verwendung: tools/release-prep.sh <version> "<englischer Titel>" [--notes <datei>] [--pick <commit>…] [--pr]
#
# Bereitet ein Release vor: legt von origin/release/<x.y> den Branch release-prep/<version> an und committet dort
#   Cargo.toml/Cargo.lock-Version, beide Changelogs (Changelog.md + Changelog.de.md: `WIP` -> heutiges Datum, = späteres Tag-Datum)
#   und den Titel in tools/titles.txt.
#   --notes <datei>   übernimmt den englischen Release-Text als docs/release-notes/<version>.en.md in denselben Commit
#                     (der Text muss den englischen Changelog-Abschnitt der Version selbst enthalten; release_notes.py hängt
#                     nur den deutschen an). Ohne --notes muss die Datei schon im Release-Branch stehen.
#   --pick <commit>…  (PATCH-Releases) überträgt vorher die Fix-Commits aus develop per `git cherry-pick -x` – Backport und
#                     Release-Vorbereitung in einem Pull Request statt zweien (spart eine CI-Runde). Bei einem Konflikt hält
#                     das Skript an: von Hand lösen (Changelogs: Abschnitt [<version>] aus develop übernehmen), dann
#                     `git cherry-pick --continue` und das Skript mit denselben Argumenten ohne die schon übertragenen Commits
#                     erneut aufrufen (der Branch release-prep/<version> wird dann weiterverwendet).
# Mit --pr wird der Branch gepusht und ein Pull Request nach release/<x.y> geöffnet. Gemergt wird nach grüner CI;
# danach auf release/<x.y> (neu gezogen, Datum muss noch „heute“ sein): tools/release.sh <version> --publish.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/common.sh
V="${1:?Version fehlt}"; TITLE="${2:?Englischer Titel fehlt}"; shift 2
PR=0; NOTES=""; PICKS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --pr) PR=1 ;;
    --notes) NOTES="${2:?--notes braucht eine Datei}"; shift ;;
    --pick) shift; while [ $# -gt 0 ] && [[ "$1" != --* ]]; do PICKS+=("$1"); shift; done; continue ;;
    *) echo "Unbekannte Option: $1"; exit 1 ;;
  esac
  shift
done
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Ungültige Version: $V"; exit 1; }
[ -z "$NOTES" ] || [ -f "$NOTES" ] || { echo "Release-Text nicht gefunden: $NOTES"; exit 1; }
[ -z "$NOTES" ] || NOTES="$(cd "$(dirname "$NOTES")" && pwd)/$(basename "$NOTES")"
REL="release/${V%.*}"; BR="release-prep/$V"
[ -z "$(git status --porcelain)" ] || { echo "Arbeitsbaum nicht sauber"; exit 1; }
git fetch -q origin "$REL"
if [ "$(git symbolic-ref --short -q HEAD || true)" = "$BR" ]; then
  echo "Weiter auf $BR (nach gelöstem Konflikt)"
else
  git switch -q -c "$BR" "origin/$REL"
fi
if [ ${#PICKS[@]} -gt 0 ]; then
  git fetch -q origin develop
  for c in "${PICKS[@]}"; do
    git cherry-pick -x "$c" || { echo "Konflikt bei $c – von Hand lösen, git cherry-pick --continue, dann das Skript ohne die schon übertragenen Commits erneut aufrufen"; exit 1; }
  done
fi
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
[ -z "$NOTES" ] || cp "$NOTES" "docs/release-notes/$V.en.md"
[ -f "docs/release-notes/$V.en.md" ] || echo "ACHTUNG: docs/release-notes/$V.en.md fehlt noch (englischer Release-Text) – vor dem Commit anlegen!"
grep -q '^### ' "docs/release-notes/$V.en.md" 2>/dev/null || echo "ACHTUNG: docs/release-notes/$V.en.md enthält keinen englischen Changelog-Abschnitt (### Added/Fixed …)"
"$PY" tools/gen_licenses.py --check
git add -A
git commit -q -F - <<MSGEOF
Prepare release $V

${ATTRIBUTION:-}
MSGEOF
git log --oneline -1
if [ "$PR" = 1 ]; then
  git push -u origin "$BR"
  BODY="Sets the version, changelog date, title and English release text for **$V**."
  [ ${#PICKS[@]} -eq 0 ] || BODY="$BODY Also backports from develop (cherry-pick -x): ${PICKS[*]}."
  BODY="$BODY After the merge: \`tools/release.sh $V --publish\` on \`$REL\` (the changelog date must equal the tag date, i.e. today)."
  "$GH" pr create --base "$REL" --head "$BR" --title "Prepare release $V" --body "$BODY"
fi
