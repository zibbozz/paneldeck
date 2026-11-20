#!/bin/bash
# Verwendung: tools/check-linux-packages.sh <version>
# Prüft die Pakete aus tools/package-linux.sh (CI, ohne Root und ohne Installation): Prüfsummen, Inhalt jedes Pakets,
# und dass das Programm aus Tarball und AppImage startet (`--version`, ohne Fenster).
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:?Version fehlt}"
OUT="dist/linux-$V"
fail() { echo "FEHLER: $*"; exit 1; }
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT

(cd "$OUT" && sha256sum -c --quiet SHA256SUMS-linux.txt) || fail "SHA256SUMS-linux.txt passt nicht"
[ "$(wc -l < "$OUT/SHA256SUMS-linux.txt")" -ge 1 ] || fail "keine Prüfsummen"

want_version() { # want_version <programm>
  local out; out="$("$1" --version 2>&1 || true)"
  [[ "$out" == *"$V"* ]] || fail "$1 --version: $out"
  echo "ok: $1 --version → $out"
}

if [ -f "$OUT/paneldeck-$V-linux-x86_64.tar.gz" ]; then
  tar -C "$W" -xzf "$OUT/paneldeck-$V-linux-x86_64.tar.gz"
  for f in paneldeck paneldeck.desktop paneldeck.png LICENSE THIRD_PARTY_LICENSES.txt README.md Changelog.md; do
    [ -e "$W/paneldeck-$V/$f" ] || fail "Tarball: $f fehlt"
  done
  want_version "$W/paneldeck-$V/paneldeck"
fi

if [ -f "$OUT/paneldeck_${V}_amd64.deb" ]; then
  dpkg-deb -I "$OUT/paneldeck_${V}_amd64.deb" | grep -q "Version: $V" || fail ".deb: Version"
  list="$(dpkg-deb -c "$OUT/paneldeck_${V}_amd64.deb")"
  for f in ./usr/bin/paneldeck ./usr/share/applications/paneldeck.desktop ./usr/share/icons/hicolor/64x64/apps/paneldeck.png ./usr/share/doc/paneldeck/copyright; do
    grep -q " $f\$" <<<"$list" || fail ".deb: $f fehlt"
  done
  echo "ok: .deb"
fi

if [ -f "$OUT/paneldeck-$V-1.x86_64.rpm" ] && command -v rpm >/dev/null; then
  rpm -qp --queryformat '%{VERSION}\n' "$OUT/paneldeck-$V-1.x86_64.rpm" | grep -qx "${V//-/\~}" || fail ".rpm: Version"
  rpm -qlp "$OUT/paneldeck-$V-1.x86_64.rpm" | grep -qx /usr/bin/paneldeck || fail ".rpm: /usr/bin/paneldeck fehlt"
  echo "ok: .rpm"
fi

if [ -f "$OUT/Paneldeck-$V-x86_64.AppImage" ]; then
  (cd "$W" && "$OLDPWD/$OUT/Paneldeck-$V-x86_64.AppImage" --appimage-extract >/dev/null) || fail "AppImage lässt sich nicht entpacken"
  [ -x "$W/squashfs-root/AppRun" ] || fail "AppImage: AppRun fehlt"
  want_version "$W/squashfs-root/AppRun"
  # Kennung für das Selbst-Update (`app::update::install::is_appimage`): ELF + „AI\x02“ ab Byte 8
  [ "$(head -c 11 "$OUT/Paneldeck-$V-x86_64.AppImage" | tail -c 3 | od -An -tx1 | tr -d ' ')" = "414902" ] || fail "AppImage: Kennung AI 02 fehlt"
fi
echo "Linux-Pakete geprüft."
