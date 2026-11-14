#!/bin/bash
# Gemeinsame Prüfung der Hooks: auf geschützten Branches (develop, main, release/*) wird nicht direkt committet oder gemergt.
# Änderungen laufen über Feature-/Fix-Branches und Pull Requests; gemergt wird nur auf GitHub durch den Projektinhaber.
b="$(git symbolic-ref --short -q HEAD || true)"
case "$b" in
  develop|main|release/*|feature/next)
    echo "Gesperrt: direkter Commit/Merge auf '$b'." >&2
    echo "Arbeite auf einem Branch feature/…, fix/…, chore/… oder backport/… und öffne einen Pull Request (siehe CLAUDE.md, Branching-Strategie)." >&2
    exit 1 ;;
esac
