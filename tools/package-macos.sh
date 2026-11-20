#!/bin/bash
# Verwendung: tools/package-macos.sh <version> [--no-build]
#
# macOS-Pakete (#150, experimentell, unsigniert; Entscheidungen vom 2026-11-16/20): baut paneldeck für Apple Silicon und
# Intel (macOS 12+), fügt beides mit lipo zu einem Universal-Binary zusammen, legt Paneldeck.app an (ad-hoc signiert –
# Apple Silicon startet kein völlig unsigniertes Programm; das ist KEINE Developer-ID-Signatur, Gatekeeper fragt trotzdem)
# und packt nach dist/macos-<version>/:
#   Paneldeck-<v>-macos-universal.dmg   Disk-Image mit Paneldeck.app, Verknüpfung „Applications“ und Hinweis-Datei
#   paneldeck-<v>-macos-universal.zip   Paneldeck.app als ZIP (auch für das Selbst-Update der App)
#   SHA256SUMS-macos.txt                Prüfsummen (Format wie sha256sum; release.sh übernimmt sie in SHA256SUMS.txt)
# Läuft in der CI auf einem macOS-Runner (kostenlos im öffentlichen Repository) und lokal auf einem Mac mit Xcode-
# Kommandozeilenwerkzeugen. Nichts wird veröffentlicht.
set -euo pipefail
cd "$(dirname "$0")/.."

V="${1:?Version fehlt}"; shift
BUILD=1
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build) BUILD=0 ;;
    *) echo "Unbekannte Option: $1"; exit 1 ;;
  esac
  shift
done
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "Ungültige Version: $V"; exit 1; }
[ "$(uname -s)" = Darwin ] || { echo "Nur unter macOS"; exit 1; }
grep -q "^version = \"$V\"" Cargo.toml || { echo "Cargo.toml hat nicht Version $V"; exit 1; }
for t in lipo codesign ditto hdiutil iconutil sips plutil shasum python3; do
  command -v "$t" >/dev/null || { echo "$t fehlt (Xcode-Kommandozeilenwerkzeuge?)"; exit 1; }
done

export MACOSX_DEPLOYMENT_TARGET=12.0
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ "$BUILD" = 1 ]; then
  rustup target add "${TARGETS[@]}" >/dev/null
  # Keine Pfade des Build-Rechners im Programm (wie tools/common.sh unter Windows).
  export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=/rustup --remap-path-prefix=$PWD=/paneldeck"
  for t in "${TARGETS[@]}"; do
    cargo build --release --locked --target "$t" 2>&1 | grep -E "^error|Finished"
  done
fi
for t in "${TARGETS[@]}"; do
  [ -x "$TARGET_DIR/$t/release/paneldeck" ] || { echo "$TARGET_DIR/$t/release/paneldeck fehlt"; exit 1; }
done

OUT="dist/macos-$V"
rm -rf "$OUT"; mkdir -p "$OUT"
STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT

lipo -create -output "$STAGE/paneldeck" "$TARGET_DIR/aarch64-apple-darwin/release/paneldeck" "$TARGET_DIR/x86_64-apple-darwin/release/paneldeck"
if [ "$BUILD" = 1 ] && grep -a -q "$PWD/" "$STAGE/paneldeck"; then
  echo "Pfad des Build-Rechners ($PWD) im Programm – --remap-path-prefix greift nicht"; exit 1
fi

# --- Paneldeck.app -------------------------------------------------------------------------------------------------------
APP="$STAGE/app/Paneldeck.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/licenses"
install -m755 "$STAGE/paneldeck" "$APP/Contents/MacOS/paneldeck"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Paneldeck</string>
    <key>CFBundleDisplayName</key>
    <string>Paneldeck</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.zibbozz.paneldeck</string>
    <key>CFBundleExecutable</key>
    <string>paneldeck</string>
    <key>CFBundleIconFile</key>
    <string>paneldeck</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$V</string>
    <key>CFBundleVersion</key>
    <string>$V</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>MIT License – https://github.com/zibbozz/paneldeck</string>
</dict>
</plist>
EOF
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# Symbol: das 256-px-PNG aus assets/icon.ico (alle Größen dort sind PNG), daraus ein .iconset und paneldeck.icns.
python3 -I - "$STAGE/icon256.png" <<'PY'
import struct, sys
d = open("assets/icon.ico", "rb").read()
n = struct.unpack("<H", d[4:6])[0]
best = max((struct.unpack("<BBBBHHII", d[6 + 16 * i:22 + 16 * i]) for i in range(n)), key=lambda e: e[0] or 256)
w, h, _, _, _, _, size, off = best
assert d[off:off + 4] == b"\x89PNG", "icon.ico: größtes Bild ist kein PNG"
open(sys.argv[1], "wb").write(d[off:off + size])
PY
ICONSET="$STAGE/paneldeck.iconset"; mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$STAGE/icon256.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" "$STAGE/icon256.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/paneldeck.icns"

install -m644 LICENSE THIRD_PARTY.md THIRD_PARTY_LICENSES.txt README.md README.de.md Changelog.md Changelog.de.md "$APP/Contents/Resources/"
install -m644 assets/fonts/LICENSE-*.txt "$APP/Contents/Resources/licenses/"

# Ad-hoc-Signatur des ganzen Bündels (kostenlos, ohne Apple-Konto); danach darf am Bündel nichts mehr geändert werden.
codesign --force --deep --sign - --timestamp=none "$APP"
codesign --verify --deep --strict "$APP"

# --- ZIP (ditto: Unix-Rechte bleiben, keine Ressourcen-Gabeln/Attribute – das Selbst-Update liest es selbst) --------------
(cd "$STAGE/app" && ditto -c -k --norsrc --noextattr --noacl --keepParent Paneldeck.app "$OLDPWD/$OUT/paneldeck-$V-macos-universal.zip")

# --- DMG ----------------------------------------------------------------------------------------------------------------
DMG="$STAGE/dmg"
mkdir -p "$DMG"
ditto --norsrc --noextattr "$APP" "$DMG/Paneldeck.app"
ln -s /Applications "$DMG/Applications"
cat > "$DMG/If macOS blocks Paneldeck.txt" <<'EOF'
Paneldeck for macOS is EXPERIMENTAL and NOT SIGNED with an Apple Developer ID.

1. Drag Paneldeck.app onto "Applications".
2. Open it once: right-click Paneldeck.app in Applications -> Open -> Open.
   macOS 15 (Sequoia) and newer: double-click it, then go to
   System Settings -> Privacy & Security -> "Paneldeck" was blocked -> Open Anyway.
3. Or in Terminal:  xattr -d com.apple.quarantine /Applications/Paneldeck.app

Only do this for a file you downloaded from https://github.com/zibbozz/paneldeck/releases
and whose SHA-256 matches SHA256SUMS.txt of that release.
Feedback and problems: https://github.com/zibbozz/paneldeck/issues

---

Paneldeck für macOS ist EXPERIMENTELL und NICHT mit einer Apple-Developer-ID signiert.

1. Paneldeck.app auf "Applications" (Programme) ziehen.
2. Einmal öffnen: Rechtsklick auf Paneldeck.app in Programme -> Öffnen -> Öffnen.
   Ab macOS 15 (Sequoia): Doppelklick, dann Systemeinstellungen -> Datenschutz & Sicherheit ->
   "Paneldeck" wurde blockiert -> Dennoch öffnen.
3. Oder im Terminal:  xattr -d com.apple.quarantine /Applications/Paneldeck.app

Nur für eine Datei von https://github.com/zibbozz/paneldeck/releases, deren SHA-256 zu
SHA256SUMS.txt dieses Releases passt. Rückmeldungen: https://github.com/zibbozz/paneldeck/issues
EOF
hdiutil create -quiet -volname "Paneldeck $V" -srcfolder "$DMG" -fs HFS+ -format UDZO -ov "$OUT/Paneldeck-$V-macos-universal.dmg"

(cd "$OUT" && shasum -a 256 -- *.dmg *.zip > SHA256SUMS-macos.txt && cat SHA256SUMS-macos.txt)
