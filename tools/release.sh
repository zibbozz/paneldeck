#!/bin/bash
# Verwendung: tools/release.sh <version> [--publish] [--skip-smoke] [--no-git] [--ignore-defender]
#
# Release-Lauf für den Branch release/x.y (siehe CLAUDE.md, „Branching- und Release-Strategie“). Dieses Skript committet NICHT:
# Versionsnummer, Changelog-Datum und Titel kommen vorher über einen Pull Request in release/x.y (tools/release-prep.sh).
#   (ohne Optionen)  prüft, testet, baut, packt nach dist/ und setzt lokal das Tag v<version> auf den Stand von release/x.y
#   --publish        zusätzlich: nur das Tag pushen und das GitHub-Release anlegen (braucht `gh`, angemeldet); wartet dafür auf
#                    die Linux- und macOS-Pakete aus GitHub Actions (release-build.yml, Jobs `linux`/`macos`) und hängt sie mit
#                    an (#149, #150)
#   --no-git         nur bauen und packen (Probelauf auf jedem Branch; kein Tag, kein Push)
#   --skip-smoke     Smoke-Test und Update-Test (tools/smoke-mcp.ps1, tools/update-test.ps1; nur Windows) überspringen
#   --ignore-defender  trotz Defender-Treffer weitermachen (nur nach Rücksprache mit dem Nutzer; Fehlalarm vorher bei Microsoft melden)
# Defender-Prüfung (Windows, Vorgabe des Nutzers): paneldeck.exe und Installer werden vor Tag/Veröffentlichung mit
#   tools/defender-scan.ps1 geprüft; bei einem Treffer bricht das Skript ab und gibt die Werte für das Microsoft-Formular aus.
# Prüfungen (außer mit --no-git): Branch ist release/<x.y> passend zur Version · Arbeitsbaum sauber · Stand == origin/release/<x.y> ·
#   Cargo.toml-Version · Changelog-Abschnitt mit heutigem Datum (= Tag-Datum) in Changelog.md und Changelog.de.md · englischer Text docs/release-notes/<v>.en.md · Titel.
# Das GitHub-Release ist ein normales Release (Pre-release nur, wenn die Version ein „-“ enthält; Release Candidates nutzen wir nicht).
# Windows: nativer MSVC-Build (target/release), Installer mit NSIS (makensis). Linux: Cross-Build nach windows-gnu.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/common.sh

V="${1:?Version fehlt}"; shift
PUBLISH=0; NOGIT=0; SKIP_SMOKE=0; IGNORE_DEFENDER=0
for a in "$@"; do
  case "$a" in
    --publish) PUBLISH=1 ;;
    --ignore-defender) IGNORE_DEFENDER=1 ;;
    --no-git) NOGIT=1 ;;
    --skip-smoke) SKIP_SMOKE=1 ;;
    *) echo "Unbekannte Option: $a"; exit 1 ;;
  esac
done
[ "$PUBLISH" = 1 ] && [ "$NOGIT" = 1 ] && { echo "--publish und --no-git passen nicht zusammen"; exit 1; }
[ "$PUBLISH" = 1 ] && [ -z "$GH" ] && { echo "gh (GitHub CLI) nicht gefunden: winget install GitHub.cli"; exit 1; }
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "Ungültige Version (erwartet x.y.z): $V"; exit 1; }

if [ "$NOGIT" = 0 ]; then
  BRANCH_WANT="release/${V%.*}"
  BRANCH="$(git symbolic-ref --short -q HEAD || true)"
  [ "$BRANCH" = "$BRANCH_WANT" ] || { echo "Releases entstehen nur auf $BRANCH_WANT (aktuell: ${BRANCH:-detached HEAD})"; exit 1; }
  [ -z "$(git status --porcelain)" ] || { echo "Arbeitsbaum nicht sauber – alles muss per Pull Request in $BRANCH_WANT gelandet sein"; git status --short; exit 1; }
  git fetch -q origin "$BRANCH_WANT" "+refs/tags/*:refs/tags/*" 2>/dev/null || git fetch -q origin "$BRANCH_WANT"
  [ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$BRANCH_WANT")" ] || { echo "Lokaler Stand ≠ origin/$BRANCH_WANT (git pull --ff-only bzw. ungemergte Änderungen)"; exit 1; }
  ! git rev-parse -q --verify "refs/tags/v$V" >/dev/null || { echo "Tag v$V existiert schon – veröffentlichte Versionen werden nie verändert (nächste PATCH-Version nehmen)"; exit 1; }
fi

grep -q "^version = \"$V\"" Cargo.toml || { echo "Cargo.toml hat nicht Version $V"; exit 1; }
if [ "$NOGIT" = 0 ]; then
  "$PY" tools/changelog.py check "$V"   # Abschnitt [$V] existiert, nicht leer, nicht WIP, Datum == heute (= Tag-Datum)
else
  "$PY" tools/changelog.py check        # Probelauf: nur Format
fi
"$PY" tools/check_docs_images.py       # README/Doku: keine offenen Screenshot-Platzhalter, alle Bilder da, EN/DE gleich (tools/docs-shots.ps1)
"$PY" tools/gen_licenses.py --check   # Lizenztexte der Abhängigkeiten passen zu Cargo.lock (sonst: python tools/gen_licenses.py)
grep -q "^$V|" tools/titles.txt || { echo "tools/titles.txt hat keinen (englischen) Titel für $V"; exit 1; }
[ -f "docs/release-notes/$V.en.md" ] || { echo "docs/release-notes/$V.en.md fehlt (englischer Release-Text)"; exit 1; }

# Früher Start der Linux-/macOS-Pakete (nur --publish): release-build.yml per workflow_dispatch auf genau diesem Commit von
# release/x.y, damit die CI-Builds (macOS ca. 19 min, ohne Cache) parallel zum lokalen Teil laufen. Das Tag wird weiterhin
# erst nach allen lokalen Prüfungen (Tests, Build, Defender) gepusht – nie vorher, ein gepushtes Tag wird nie verschoben.
# Herkunftsnachweis dieser Pakete: gleicher Workflow, Ref release/x.y und genau der Commit des Tags (--source-digest).
# Klappt der frühe Start nicht (oder hat das Tag einen anderen Commit), wird wie bisher auf den Build des Tags gewartet.
EARLY_SHA=""; EARLY_RUN=""
if [ "$PUBLISH" = 1 ]; then
  EARLY_SHA="$(git rev-parse HEAD)"
  EARLY_SINCE="$(date -u -d '-2 min' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%SZ)"
  if "$GH" workflow run release-build.yml --ref "$BRANCH_WANT" >/dev/null 2>&1; then
    for _ in $(seq 1 24); do
      EARLY_RUN="$("$GH" run list --workflow release-build.yml --event workflow_dispatch --limit 10 --json databaseId,headSha,createdAt \
        --jq ".[] | select(.headSha==\"$EARLY_SHA\" and .createdAt >= \"$EARLY_SINCE\") | .databaseId" | head -1)"
      [ -n "$EARLY_RUN" ] && break
      sleep 5
    done
  fi
  if [ -n "$EARLY_RUN" ]; then
    echo "Linux-/macOS-Pakete früh gestartet (Lauf $EARLY_RUN, Commit $EARLY_SHA)"
  else
    echo "Hinweis: früher Start der Linux-/macOS-Pakete nicht möglich – sie werden nach dem Push des Tags gebaut"
  fi
fi

fetch_conpty   # dist/conpty/{conpty.dll,OpenConsole.exe} für Tests, Installer und ZIP (geprüft per SHA-256)
# Tests auch gegen die ausgelieferte ConPTY (conpty::tests::pty_runs_on_selected_host startet eine Sitzung darauf).
PANELDECK_TEST_CONPTY_DIR="$(abspath dist/conpty)" cargo test --release --quiet  # bricht bei fehlschlagenden Tests ab (set -e)
cargo build --release ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"} 2>&1 | grep -E "^error|Finished"
mkdir -p dist
cp "$EXE_SRC" "dist/paneldeck-$V.exe"
# Statische C-Laufzeit (.cargo/config.toml, tools/common.sh): keine VCRUNTIME140.dll, die vor main() gesucht würde.
if [ "$IS_WIN" = 1 ] && grep -a -q -i 'vcruntime140' "dist/paneldeck-$V.exe"; then
  echo "Programmdatei braucht VCRUNTIME140.dll – die C-Laufzeit muss statisch eingebunden sein (+crt-static)"; exit 1
fi
if [ -n "$MAKENSIS" ]; then
  "$MAKENSIS" -V1 -DVERSION="$V" -DEXE="$(abspath "dist/paneldeck-$V.exe")" -DCONPTY_DIR="$(abspath dist/conpty)" \
    -DOUTFILE="$(abspath "dist/Paneldeck-Setup-$V.exe")" installer/paneldeck.nsi
else
  echo "Hinweis: makensis nicht gefunden – kein Installer (Windows: winget install NSIS.NSIS)"
fi
sha256sum "dist/paneldeck-$V.exe" | tee "dist/paneldeck-$V.sha256"
# Keine Pfade/Namen des Build-Rechners in der Programmdatei
if [ "$IS_WIN" = 1 ]; then
  n="$(grep -a -c -i -E 'C:\\Users|E:\\claude|E:\\paneldeck' "dist/paneldeck-$V.exe" || true)"
  [ "$n" = 0 ] || { echo "Programmdatei enthält Pfade des Build-Rechners ($n Treffer)"; exit 1; }
fi
# Der lokale Test-Feed des Selbst-Updates (Cargo-Feature update-test-feed) darf nie im Release stecken.
if grep -a -q 'PANELDECK_UPDATE_TEST_FEED' "dist/paneldeck-$V.exe"; then
  echo "Programmdatei enthält den Update-Test-Feed (Feature update-test-feed) – so nicht veröffentlichen"; exit 1
fi
# Ebenso die Test-Schnittstelle des Tutorials (Cargo-Feature tutorial-test, #143).
if grep -a -q 'PANELDECK_TUTORIAL_TEST' "dist/paneldeck-$V.exe"; then
  echo "Programmdatei enthält die Test-Schnittstelle des Tutorials (Feature tutorial-test) – so nicht veröffentlichen"; exit 1
fi
# Ebenso die Startansichten für die Doku-Screenshots (Cargo-Feature readme-shots, tools/docs-shots.ps1).
if grep -a -q 'PANELDECK_README_SHOTS_BUILD' "dist/paneldeck-$V.exe"; then
  echo "Programmdatei enthält die Screenshot-Startansichten (Feature readme-shots) – so nicht veröffentlichen"; exit 1
fi

# Defender-Prüfung auf Fehlalarme (Windows): paneldeck.exe (steckt unverändert auch im ZIP) und der Installer.
if [ "$IS_WIN" = 1 ]; then
  echo "== Defender-Prüfung (tools/defender-scan.ps1)"
  DEF_FILES=("$(abspath "dist/paneldeck-$V.exe")")
  [ -f "dist/Paneldeck-Setup-$V.exe" ] && DEF_FILES+=("$(abspath "dist/Paneldeck-Setup-$V.exe")")
  set +e
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/defender-scan.ps1 "${DEF_FILES[@]}"
  DEF_RC=$?
  set -e
  if [ "$DEF_RC" = 1 ]; then
    if [ "$IGNORE_DEFENDER" = 1 ]; then
      echo "WARNUNG: Defender meldet einen Treffer – weiter wegen --ignore-defender."
    else
      echo "Defender meldet einen Treffer: Release angehalten. Fehlalarm bei Microsoft melden (Werte siehe oben),"
      echo "dann neu prüfen oder nach Rücksprache mit dem Nutzer mit --ignore-defender fortfahren."
      exit 1
    fi
  elif [ "$DEF_RC" != 0 ]; then
    echo "WARNUNG: Defender-Prüfung nicht möglich (Exit $DEF_RC) – bitte von Hand prüfen: tools/defender-scan.ps1 <Dateien>"
  fi
fi

# Ende-zu-Ende-Test der gebauten .exe (Windows): startet sie in einem Temp-Ordner und prüft die Agenten-Schnittstelle.
# PANELDECK_TEST_ROOT (optional): Ordner für die Temp-Ordner beider Tests statt %TEMP% – z. B. ein Ordner mit Defender-Ausnahme,
# solange unsignierte Builds fälschlich erkannt werden (siehe CLAUDE.md, Defender-Prüfung).
if [ "$IS_WIN" = 1 ] && [ "$SKIP_SMOKE" = 0 ]; then
  ROOT_ARGS_SMOKE=(); ROOT_ARGS_UPD=()
  if [ -n "${PANELDECK_TEST_ROOT:-}" ]; then
    mkdir -p "$PANELDECK_TEST_ROOT"
    ROOT_ARGS_SMOKE=(-WorkRoot "$(abspath "$PANELDECK_TEST_ROOT")"); ROOT_ARGS_UPD=(-TempRoot "$(abspath "$PANELDECK_TEST_ROOT")")
  fi
  echo "== Smoke-Test (tools/smoke-mcp.ps1)"
  # Wie ausgeliefert: mit der geprüften ConPTY aus dist/conpty, und sie muss genutzt werden.
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/smoke-mcp.ps1 -Exe "$(abspath "dist/paneldeck-$V.exe")" \
    -ConPtyDir "$(abspath dist/conpty)" -ConPty bundled ${ROOT_ARGS_SMOKE[@]+"${ROOT_ARGS_SMOKE[@]}"}
  # Update-Test des echten Installers (alte Version -> dieser Installer, ruhender und laufender Hub; Schein-Profil, räumt auf).
  if [ -f "dist/Paneldeck-Setup-$V.exe" ]; then
    echo "== Update-Test (tools/update-test.ps1)"
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/update-test.ps1 -New "$(abspath "dist/Paneldeck-Setup-$V.exe")" -Exe "$(abspath "dist/paneldeck-$V.exe")" ${ROOT_ARGS_UPD[@]+"${ROOT_ARGS_UPD[@]}"}
  fi
fi

if [ "$NOGIT" = 0 ]; then
  git tag -a "v$V" -m "Paneldeck $V"
  git log --oneline -1 "v$V"
fi

tools/package.sh "$V"

# Prüfsummen der Release-Dateien für das Selbst-Update (Paneldeck lädt SHA256SUMS.txt und verwirft Downloads, die nicht passen).
# Format wie `sha256sum` (`<hex>  <datei>`); der Asset-Name muss genau SHA256SUMS.txt sein, daher ein eigener Ordner je Version.
SUMS_DIR="dist/sums-$V"
mkdir -p "$SUMS_DIR"
: > "$SUMS_DIR/SHA256SUMS.txt"
for f in "Paneldeck-Setup-$V.exe" "paneldeck-$V-windows-x64.zip"; do
  [ -f "dist/$f" ] || continue
  printf '%s  %s\n' "$(sha256sum "dist/$f" | cut -d' ' -f1)" "$f" >> "$SUMS_DIR/SHA256SUMS.txt"
done
(cd dist && sha256sum -c --quiet "sums-$V/SHA256SUMS.txt") || { echo "SHA256SUMS.txt passt nicht zu den Dateien in dist/"; exit 1; }
cat "$SUMS_DIR/SHA256SUMS.txt"

if [ "$PUBLISH" = 1 ]; then
  git push origin "v$V"
  # Linux-Pakete (#149) und macOS-Pakete (#150): baut .github/workflows/release-build.yml (Jobs `linux` und `macos`) auf dem
  # gerade gepushten Tag. Hier auf beide warten, die Artefakte laden, ihre Prüfsummen und Herkunftsnachweise prüfen und die
  # Zeilen in SHA256SUMS.txt übernehmen (das Selbst-Update der AppImage und der macOS-App prüft gegen diese Datei). Scheitert
  # das, wird kein Release angelegt; das Tag ist dann schon gepusht – Ursache beheben (z. B. den Job in GitHub neu starten)
  # und das Release danach von Hand anlegen.
  SHA_TAG="$(git rev-parse "v$V^{commit}")"
  RUN=""; SRC_REF="refs/tags/v$V"
  if [ -n "$EARLY_RUN" ] && [ "$EARLY_SHA" = "$SHA_TAG" ]; then
    RUN="$EARLY_RUN"; SRC_REF="refs/heads/$BRANCH_WANT"
    echo "Nehme den früh gestarteten Lauf $RUN (Commit $SHA_TAG)"
  fi
  for _ in $(seq 1 60); do
    [ -n "$RUN" ] && break
    RUN="$("$GH" run list --workflow release-build.yml --event push --limit 20 --json databaseId,headSha,headBranch \
      --jq ".[] | select(.headSha==\"$SHA_TAG\" and .headBranch==\"v$V\") | .databaseId" | head -1)"
    [ -n "$RUN" ] && break
    sleep 10
  done
  [ -n "$RUN" ] || { echo "Release-Build für v$V nicht gefunden (GitHub Actions)"; exit 1; }
  # ci_assets <job> <ordner> <prüfsummen-datei> <datei>…: auf den Job warten, Artefakt `paneldeck-<v>-<job>` laden und prüfen.
  ci_assets() {
    local job="$1" dir="$2" sums="$3"; shift 3
    echo "Warte auf den Build \`$job\` (Lauf $RUN) …"
    local concl=""
    for _ in $(seq 1 360); do
      concl="$("$GH" run view "$RUN" --json jobs --jq ".jobs[] | select(.name==\"$job\") | .status + \":\" + (.conclusion // \"\")")"
      case "$concl" in
        completed:success) break ;;
        completed:*) echo "Build \`$job\` fehlgeschlagen ($concl) – siehe gh run view $RUN"; exit 1 ;;
      esac
      sleep 10
    done
    [ "$concl" = "completed:success" ] || { echo "Build \`$job\` nicht rechtzeitig fertig (Lauf $RUN)"; exit 1; }
    rm -rf "$dir"
    "$GH" run download "$RUN" -n "paneldeck-$V-$job" -D "$dir"
    (cd "$dir" && sha256sum -c --quiet "$sums") || { echo "Prüfsummen der Pakete von \`$job\` passen nicht"; exit 1; }
    for f in "$@"; do
      [ -f "$dir/$f" ] || { echo "Paket fehlt im Artefakt \`$job\`: $f"; exit 1; }
      # Herkunftsnachweis (actions/attest-build-provenance im Job): gebaut von diesem Repository in GitHub Actions.
      # Nur Nachweise aus release-build.yml für genau dieses Tag bzw. den früh gestarteten Lauf auf release/x.y, in beiden
      # Fällen für genau den Commit des Tags (kein anderer Workflow, Branch oder Commit des Repositorys).
      "$GH" attestation verify "$dir/$f" -R zibbozz/paneldeck \
        --signer-workflow zibbozz/paneldeck/.github/workflows/release-build.yml --source-ref "$SRC_REF" \
        --source-digest "$SHA_TAG" >/dev/null || { echo "Herkunftsnachweis von $f nicht bestätigt"; exit 1; }
      grep -q "  $f\$" "$SUMS_DIR/SHA256SUMS.txt" || grep "  $f\$" "$dir/$sums" >> "$SUMS_DIR/SHA256SUMS.txt"
    done
    (cd "$dir" && sha256sum -c --quiet --ignore-missing "../sums-$V/SHA256SUMS.txt") || { echo "SHA256SUMS.txt passt nicht zu den Paketen von \`$job\`"; exit 1; }
  }
  LINUX_DIR="dist/linux-ci-$V"
  LINUX_FILES=("Paneldeck-$V-x86_64.AppImage" "paneldeck_${V}_amd64.deb" "paneldeck-$V-1.x86_64.rpm" "paneldeck-$V-linux-x86_64.tar.gz")
  ci_assets linux "$LINUX_DIR" SHA256SUMS-linux.txt "${LINUX_FILES[@]}"
  MACOS_DIR="dist/macos-ci-$V"
  MACOS_FILES=("Paneldeck-$V-macos-universal.dmg" "paneldeck-$V-macos-universal.zip")
  ci_assets macos "$MACOS_DIR" SHA256SUMS-macos.txt "${MACOS_FILES[@]}"
  cat "$SUMS_DIR/SHA256SUMS.txt"
  ASSETS=("dist/paneldeck-$V-windows-x64.zip")
  [ -f "dist/Paneldeck-Setup-$V.exe" ] && ASSETS+=("dist/Paneldeck-Setup-$V.exe")
  for f in "${LINUX_FILES[@]}"; do ASSETS+=("$LINUX_DIR/$f"); done
  for f in "${MACOS_FILES[@]}"; do ASSETS+=("$MACOS_DIR/$f"); done
  ASSETS+=("$SUMS_DIR/SHA256SUMS.txt")
  PRE=(); [[ "$V" == *-* ]] && PRE=(--prerelease)
  "$GH" release create "v$V" "${ASSETS[@]}" --verify-tag ${PRE[@]+"${PRE[@]}"} \
    --title "$(cat "dist/gh-title-$V.txt")" --notes-file "dist/gh-notes-$V.md"
fi
