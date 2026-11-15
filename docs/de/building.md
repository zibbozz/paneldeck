[← README](../../README.de.md) · [English](../en/building.md) · **Deutsch**

# Bauen

Paneldeck ist in Rust geschrieben (egui/eframe, ConPTY via `portable-pty`, Terminal-Emulation via `alacritty_terminal`).

## Unter Windows (nativ, MSVC)
Rust (rustup, Toolchain `stable-x86_64-pc-windows-msvc`) und die Visual-Studio-Build-Tools genügen:

    cargo build --release        # -> target\release\paneldeck.exe
    cargo test --release

Eine selbst gebaute `paneldeck.exe` ist portabel: Sie legt `paneldeck-data` neben sich an (hier unter `target\release\`). Mit `--data-dir` oder `--profile` hältst du Testdaten von deinen echten Daten getrennt (siehe [Kommandozeile](command-line.md)).

## Cross-Build unter Linux
Möglich, wird aber nicht regelmäßig getestet:

    rustup target add x86_64-pc-windows-gnu && apt install gcc-mingw-w64-x86-64
    cargo build --release --target x86_64-pc-windows-gnu

## Release-Dateien
Releases entstehen auf einem Branch `release/x.y` mit `tools/release.sh <version>` (Ablauf siehe `CLAUDE.md`, Branching- und Release-Strategie) – unter Windows aus Git Bash; braucht Python 3 und für den Installer NSIS (`makensis`). Es führt die Tests aus, baut Programm und Installer, prüft beide mit Microsoft Defender (`tools/defender-scan.ps1`), führt die Ende-zu-Ende-Tests aus und packt drei Release-Dateien:

| Datei | Inhalt |
|---|---|
| `Paneldeck-Setup-<Version>.exe` | Installer (pro Benutzer, ohne Administratorrechte) |
| `paneldeck-<Version>-windows-x64.zip` | portables ZIP: Ordner `paneldeck-<Version>/` mit `paneldeck.exe`, Lizenzen, README und Changelog |
| `SHA256SUMS.txt` | SHA-256-Prüfsummen beider Dateien (für das [Selbst-Update](updates.md)) |

Mit `--publish` wird zusätzlich das Tag gepusht und das GitHub-Release angelegt (GitHub CLI `gh`, angemeldet), mit `--no-git` nur gebaut und gepackt.

## Tests
- `cargo test --release` – Unit-Tests und Oberflächen-Tests ohne Fenster.
- `tools/smoke-mcp.ps1` – Ende-zu-Ende-Test der Agenten-Schnittstelle gegen eine Kopie der gebauten `paneldeck.exe` mit eigenem temporärem Datenordner.
- `tools/update-test.ps1` – installiert eine ältere Version in einen Temp-Ordner mit Schein-Benutzerprofil und aktualisiert sie mit dem neuen Installer.
- `tools/update-feed.sh <version> <ordner>` – legt ein lokales Schein-Release an (mit `latest.json`, ZIP, Installer und `SHA256SUMS.txt`), um das Selbst-Update mit einem Debug-Build zu testen; veröffentlicht wird nichts.

Der manuelle Testplan ist `TESTPLAN.md` (Deutsch).
