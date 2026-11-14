#!/bin/bash
# Verwendung: tools/backport.sh <x.y> <name> <commit>... [--pr]
#
# Überträgt Änderungen aus develop per Cherry-Pick auf release/<x.y>, ohne dort direkt zu committen:
#   legt von origin/release/<x.y> den Branch backport/<x.y>/<name> an und übernimmt die Commits (git cherry-pick -x, Merge-Commits mit -m 1).
# <commit> sind die Commits, die der Pull Request auf develop erzeugt hat (am einfachsten „Squash and merge“ → genau ein Commit je PR).
# Mit --pr wird der Branch gepusht und ein Pull Request nach release/<x.y> geöffnet; gemergt wird nur auf GitHub durch den Projektinhaber.
# Bei Konflikten bricht das Skript ab: dann von Hand lösen (git status; Dateien bearbeiten; git add; git cherry-pick --continue).
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/common.sh
X="${1:?x.y fehlt}"; NAME="${2:?Name fehlt}"; shift 2
PR=0; COMMITS=()
for a in "$@"; do if [ "$a" = "--pr" ]; then PR=1; else COMMITS+=("$a"); fi; done
[ "${#COMMITS[@]}" -gt 0 ] || { echo "Mindestens ein Commit nötig"; exit 1; }
REL="release/$X"; BR="backport/$X/$NAME"
[ -z "$(git status --porcelain)" ] || { echo "Arbeitsbaum nicht sauber"; exit 1; }
git fetch -q origin "$REL" develop
git switch -q -c "$BR" "origin/$REL"
for c in "${COMMITS[@]}"; do
  if [ "$(git rev-list --parents -n1 "$c" | wc -w)" -gt 2 ]; then M=(-m 1); else M=(); fi
  git -c core.hooksPath=tools/git-hooks cherry-pick -x ${M[@]+"${M[@]}"} "$c" || { echo "Konflikt bei $c – von Hand lösen (siehe Kopfkommentar)"; exit 1; }
done
git log --oneline "origin/$REL..HEAD"
if [ "$PR" = 1 ]; then
  git push -u origin "$BR"
  "$GH" pr create --base "$REL" --head "$BR" --title "Backport to $REL: $NAME" \
    --body "Cherry-pick (\`-x\`) from \`develop\` into \`$REL\`: ${COMMITS[*]}"
fi
