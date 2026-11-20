#!/bin/bash
# Verwendung: tools/check-macos-packages.sh <version>
# Prüft die Pakete aus tools/package-macos.sh (CI, ohne Installation): Prüfsummen, Aufbau von Paneldeck.app (Info.plist,
# Universal-Binary arm64 + x86_64, Mindestversion, ad-hoc-Signatur), dass das Programm aus ZIP und DMG startet
# (`--version`, ohne Fenster), dass das ZIP nur Einträge enthält, die das Selbst-Update annimmt (keine Symlinks, nur
# gespeichert/Deflate, kein ZIP64), und das Selbst-Update selbst: Entpacken und Tauschen des Bündels mit genau diesem ZIP
# (Rust-Test `macos_release_zip_installs`, sonst übersprungen).
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:?Version fehlt}"
OUT="dist/macos-$V"
ZIP="$OUT/paneldeck-$V-macos-universal.zip"
DMG="$OUT/Paneldeck-$V-macos-universal.dmg"
fail() { echo "FEHLER: $*"; exit 1; }
W="$(mktemp -d)"; MNT="$W/mnt"
cleanup() { hdiutil detach -quiet "$MNT" 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

(cd "$OUT" && shasum -a 256 -c --quiet SHA256SUMS-macos.txt) || fail "SHA256SUMS-macos.txt passt nicht"
[ "$(wc -l < "$OUT/SHA256SUMS-macos.txt")" -eq 2 ] || fail "zwei Prüfsummen erwartet"

check_app() { # check_app <Paneldeck.app>
  local app="$1" bin="$1/Contents/MacOS/paneldeck" out archs
  [ -x "$bin" ] || fail "$bin fehlt"
  for f in Info.plist Resources/paneldeck.icns Resources/LICENSE Resources/THIRD_PARTY_LICENSES.txt Resources/licenses; do
    [ -e "$app/Contents/$f" ] || fail "$app: Contents/$f fehlt"
  done
  [ "$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")" = "$V" ] || fail "Info.plist: Version"
  [ "$(plutil -extract CFBundleExecutable raw "$app/Contents/Info.plist")" = "paneldeck" ] || fail "Info.plist: CFBundleExecutable"
  [ "$(plutil -extract LSMinimumSystemVersion raw "$app/Contents/Info.plist")" = "12.0" ] || fail "Info.plist: LSMinimumSystemVersion"
  archs="$(lipo -archs "$bin")"
  [[ " $archs " == *" arm64 "* && " $archs " == *" x86_64 "* ]] || fail "kein Universal-Binary: $archs"
  for a in arm64 x86_64; do
    otool -l -arch "$a" "$bin" | grep -A4 -E "LC_BUILD_VERSION|LC_VERSION_MIN_MACOSX" | grep -qE "(minos|version) 12\.0" || fail "$a: Mindestversion nicht 12.0"
  done
  codesign --verify --deep --strict "$app" || fail "$app: Signatur ungültig"
  out="$("$bin" --version 2>&1 || true)"
  [[ "$out" == *"$V"* ]] || fail "$bin --version: $out"
  echo "ok: $app ($archs) → $out"
}

# --- ZIP ----------------------------------------------------------------------------------------------------------------
python3 -I - "$ZIP" <<'PY' || fail "ZIP-Einträge"
import stat, sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
names = z.namelist()
assert len(names) < 4096, "zu viele Einträge"
for i in z.infolist():
    assert i.filename.startswith("Paneldeck.app/") or i.filename.startswith("__MACOSX/"), i.filename
    assert i.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED), (i.filename, i.compress_type)
    assert i.file_size < 0xFFFFFFFF and i.compress_size < 0xFFFFFFFF, "ZIP64"
    mode = i.external_attr >> 16
    assert not stat.S_ISLNK(mode), "Symlink: " + i.filename
    assert ".." not in i.filename.split("/"), i.filename
assert "Paneldeck.app/Contents/MacOS/paneldeck" in names
assert z.getinfo("Paneldeck.app/Contents/MacOS/paneldeck").external_attr >> 16 & 0o111, "nicht ausführbar"
print("ok: ZIP-Einträge (%d)" % len(names))
PY
ditto -x -k "$ZIP" "$W/zip"
check_app "$W/zip/Paneldeck.app"

# Selbst-Update mit genau diesem ZIP (entpacken mit dem eigenen ZIP-Leser, Bündel tauschen)
PANELDECK_TEST_MACOS_ZIP="$PWD/$ZIP" cargo test --release --locked -- --ignored --exact app::update::install::macapp_tests::macos_release_zip_installs

# --- DMG ----------------------------------------------------------------------------------------------------------------
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$MNT" "$DMG"
[ -L "$MNT/Applications" ] && [ "$(readlink "$MNT/Applications")" = /Applications ] || fail "DMG: Verknüpfung Applications fehlt"
[ -f "$MNT/If macOS blocks Paneldeck.txt" ] || fail "DMG: Hinweis-Datei fehlt"
check_app "$MNT/Paneldeck.app"
hdiutil detach -quiet "$MNT"
echo "macOS-Pakete geprüft."
