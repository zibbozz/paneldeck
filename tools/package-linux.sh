#!/bin/bash
# Verwendung: tools/package-linux.sh <version> [--no-build] [--skip <appimage|deb|rpm|tar>]...
#
# Linux-Pakete (#149, Entscheidungen vom 2026-11-16/20): baut paneldeck (x86_64, Release) und packt nach dist/linux-<version>/:
#   paneldeck-<v>-linux-x86_64.tar.gz   Tarball (Ordner paneldeck-<v>/ mit Programm, Desktop-Datei, Symbol, Lizenzen, Doku)
#   paneldeck_<v>_amd64.deb             Debian/Ubuntu (dpkg-deb)
#   paneldeck-<v>-1.x86_64.rpm          Fedora/openSUSE (rpmbuild)
#   Paneldeck-<v>-x86_64.AppImage       AppImage (appimagetool + type2-runtime, beide in fester Version mit SHA-256)
#   SHA256SUMS-linux.txt                Prüfsummen (Format wie sha256sum; release.sh übernimmt sie in SHA256SUMS.txt)
# Läuft in der CI auf ubuntu-22.04 (glibc 2.35 → Ubuntu 22.04 / Debian 12 / Fedora 39 als Minimum) und lokal unter Linux/WSL.
# Nichts wird veröffentlicht; Werkzeuge werden nur in dist/linux-tools/ geladen (nie systemweit installiert).
set -euo pipefail
cd "$(dirname "$0")/.."

V="${1:?Version fehlt}"; shift
BUILD=1; SKIP=" "
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build) BUILD=0 ;;
    --skip) SKIP="$SKIP$2 "; shift ;;
    *) echo "Unbekannte Option: $1"; exit 1 ;;
  esac
  shift
done
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "Ungültige Version: $V"; exit 1; }
[ "$(uname -s)" = Linux ] || { echo "Nur unter Linux"; exit 1; }
[ "$(uname -m)" = x86_64 ] || { echo "Nur x86_64"; exit 1; }
grep -q "^version = \"$V\"" Cargo.toml || { echo "Cargo.toml hat nicht Version $V"; exit 1; }
skip() { [[ "$SKIP" == *" $1 "* ]]; }

# Feste Werkzeug-Versionen (bei einem Wechsel Version und SHA-256 hier anpassen).
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/1.9.0/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA="46fdd785094c7f6e545b61afcfb0f3d98d8eab243f644b4b17698c01d06083d1"
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64"
RUNTIME_SHA="2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
BIN="$TARGET_DIR/release/paneldeck"
if [ "$BUILD" = 1 ]; then
  # Keine Pfade des Build-Rechners im Programm (wie tools/common.sh unter Windows).
  export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=/rustup --remap-path-prefix=$PWD=/paneldeck"
  cargo build --release --locked 2>&1 | grep -E "^error|Finished"
fi
[ -x "$BIN" ] || { echo "$BIN fehlt"; exit 1; }
if [ "$BUILD" = 1 ] && grep -a -q "$PWD/" "$BIN"; then
  echo "Pfad des Build-Rechners ($PWD) im Programm – --remap-path-prefix greift nicht"; exit 1
fi

OUT="dist/linux-$V"
rm -rf "$OUT"; mkdir -p "$OUT"
STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT

DESKTOP_FILE="[Desktop Entry]
Type=Application
Name=Paneldeck
GenericName=Terminal Manager
Comment=Terminal manager with sessions, Claude Code integration and an agent interface (MCP)
Comment[de]=Terminal-Manager mit Sitzungen, Claude-Code-Anbindung und Agenten-Schnittstelle (MCP)
Exec=paneldeck %F
Icon=paneldeck
Terminal=false
Categories=System;TerminalEmulator;Development;
Keywords=terminal;shell;claude;mcp;
StartupWMClass=paneldeck
"

# Gemeinsamer Dateibaum unter einem Präfix: bin/paneldeck, share/applications, share/icons, share/doc/paneldeck.
install_tree() { # install_tree <root> <prefix-im-root>
  local root="$1" pre="$2"
  install -Dm755 "$BIN" "$root$pre/bin/paneldeck"
  install -d "$root$pre/share/applications" "$root$pre/share/icons/hicolor/64x64/apps" "$root$pre/share/doc/paneldeck/licenses"
  printf '%s' "$DESKTOP_FILE" > "$root$pre/share/applications/paneldeck.desktop"
  install -m644 assets/icon64.png "$root$pre/share/icons/hicolor/64x64/apps/paneldeck.png"
  install -m644 LICENSE THIRD_PARTY.md THIRD_PARTY_LICENSES.txt README.md README.de.md Changelog.md Changelog.de.md "$root$pre/share/doc/paneldeck/"
  install -m644 assets/fonts/LICENSE-*.txt "$root$pre/share/doc/paneldeck/licenses/"
}

# --- Tarball -------------------------------------------------------------------------------------------------------------
if ! skip tar; then
  T="$STAGE/tar/paneldeck-$V"
  mkdir -p "$T"
  install -m755 "$BIN" "$T/paneldeck"
  printf '%s' "$DESKTOP_FILE" > "$T/paneldeck.desktop"
  install -m644 assets/icon64.png "$T/paneldeck.png"
  install -m644 LICENSE THIRD_PARTY.md THIRD_PARTY_LICENSES.txt README.md README.de.md Changelog.md Changelog.de.md "$T/"
  mkdir -p "$T/licenses" && install -m644 assets/fonts/LICENSE-*.txt "$T/licenses/"
  tar -C "$STAGE/tar" --owner=0 --group=0 --sort=name -czf "$OUT/paneldeck-$V-linux-x86_64.tar.gz" "paneldeck-$V"
fi

# --- .deb ----------------------------------------------------------------------------------------------------------------
if ! skip deb; then
  D="$STAGE/deb"
  install_tree "$D" /usr
  mkdir -p "$D/DEBIAN"
  SIZE_KB=$(du -sk "$D/usr" | cut -f1)
  cat > "$D/DEBIAN/control" <<EOF
Package: paneldeck
Version: $V
Section: utils
Priority: optional
Architecture: amd64
Installed-Size: $SIZE_KB
Depends: libc6 (>= 2.35), libgcc-s1, libgl1, libegl1, libx11-6, libxcursor1, libxrandr2, libxi6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0
Recommends: fontconfig, xdg-utils
Maintainer: Paneldeck <https://github.com/zibbozz/paneldeck/issues>
Homepage: https://github.com/zibbozz/paneldeck
Description: Terminal manager with sessions and an agent interface
 Paneldeck manages terminal sessions (bash, zsh, fish, …) in tabs and split
 panes, starts Claude Code with hooks, keeps to-dos and tickets per project
 and offers a local MCP interface for agents.
EOF
  cp LICENSE "$D/usr/share/doc/paneldeck/copyright"
  dpkg-deb --root-owner-group --build "$D" "$OUT/paneldeck_${V}_amd64.deb" >/dev/null
fi

# --- .rpm ----------------------------------------------------------------------------------------------------------------
if ! skip rpm; then
  command -v rpmbuild >/dev/null || { echo "rpmbuild fehlt (CI: apt-get install rpm) – oder --skip rpm"; exit 1; }
  R="$STAGE/rpm"
  mkdir -p "$R"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,root}
  install_tree "$R/root" /usr
  RV="${V//-/~}"   # rpm erlaubt kein „-“ in der Version
  cat > "$R/SPECS/paneldeck.spec" <<EOF
Name: paneldeck
Version: $RV
Release: 1
Summary: Terminal manager with sessions and an agent interface
License: MIT
URL: https://github.com/zibbozz/paneldeck
BuildArch: x86_64
AutoReqProv: yes
Requires: libEGL.so.1()(64bit), libGL.so.1()(64bit), libX11.so.6()(64bit), libXcursor.so.1()(64bit), libXrandr.so.2()(64bit), libXi.so.6()(64bit), libxkbcommon.so.0()(64bit), libxkbcommon-x11.so.0()(64bit), libwayland-client.so.0()(64bit)
Recommends: fontconfig, xdg-utils

%description
Paneldeck manages terminal sessions (bash, zsh, fish, ...) in tabs and split
panes, starts Claude Code with hooks, keeps to-dos and tickets per project and
offers a local MCP interface for agents.

%install
cp -a $R/root/. %{buildroot}/

%files
/usr/bin/paneldeck
/usr/share/applications/paneldeck.desktop
/usr/share/icons/hicolor/64x64/apps/paneldeck.png
%doc /usr/share/doc/paneldeck
EOF
  rpmbuild --quiet --define "_topdir $R" --define "_build_id_links none" --define "__strip /bin/true" -bb "$R/SPECS/paneldeck.spec"
  cp "$R"/RPMS/x86_64/paneldeck-*.rpm "$OUT/paneldeck-$V-1.x86_64.rpm"
fi

# --- AppImage ------------------------------------------------------------------------------------------------------------
if ! skip appimage; then
  # appimagetool prüft die Desktop-Datei und bricht ohne desktop-file-validate ab (CI: apt-get install desktop-file-utils).
  command -v desktop-file-validate >/dev/null || { echo "desktop-file-validate fehlt (desktop-file-utils) – oder --skip appimage"; exit 1; }
  TOOLS="dist/linux-tools"; mkdir -p "$TOOLS"
  fetch() { # fetch <url> <sha256> <datei>
    if [ ! -f "$3" ] || ! echo "$2  $3" | sha256sum -c --quiet - 2>/dev/null; then
      curl -sSfL -o "$3.part" "$1" && mv "$3.part" "$3"
    fi
    echo "$2  $3" | sha256sum -c --quiet - || { echo "SHA-256 von $3 passt nicht"; rm -f "$3"; exit 1; }
    chmod +x "$3"
  }
  fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA" "$TOOLS/appimagetool-1.9.0"
  fetch "$RUNTIME_URL" "$RUNTIME_SHA" "$TOOLS/runtime-20251108"
  A="$STAGE/Paneldeck.AppDir"
  install_tree "$A" /usr
  printf '%s' "$DESKTOP_FILE" > "$A/paneldeck.desktop"
  cp assets/icon64.png "$A/paneldeck.png"
  cat > "$A/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/paneldeck" "$@"
EOF
  chmod 755 "$A/AppRun"
  # appimagetool ist selbst eine AppImage: ohne FUSE (CI, Container) entpackt ausführen.
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOLS/appimagetool-1.9.0" --no-appstream --runtime-file "$TOOLS/runtime-20251108" "$A" "$OUT/Paneldeck-$V-x86_64.AppImage" >/dev/null
  chmod 755 "$OUT/Paneldeck-$V-x86_64.AppImage"
fi

(cd "$OUT" && sha256sum -- * | grep -v SHA256SUMS > SHA256SUMS-linux.txt && cat SHA256SUMS-linux.txt)
