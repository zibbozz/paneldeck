# Hinweise für Claude

Paneldeck ist ein Terminal-Manager für Windows (Rust, egui/eframe, ConPTY via `portable-pty`, Emulation via `alacritty_terminal`).
Funktionen und Datenordner: `README.md` (Englisch, Hauptseite) / `README.de.md` (Deutsch; beide synchron halten) · Änderungen: `Changelog.md` · manuelle Windows-Tests: `TESTPLAN.md` · Lizenzen: `THIRD_PARTY.md` (Projektlizenz: MIT, `LICENSE`).

## ⚠ Verbindliche Projektregeln (Vorgaben des Nutzers – immer einhalten)

1. **Versionierung: Semantic Versioning 2.0.0** – https://semver.org/lang/de/ (Details und öffentliche API: Abschnitt „Versionierung“ unten).
2. **Changelog: Keep a Changelog 1.1.0** – https://keepachangelog.com/de/1.1.0/ (Details: Abschnitt „Changelog“ unten). **Jede Änderung, die Nutzer betrifft, wird sofort unter `## [Unreleased]` eingetragen** – nicht erst beim Release.
3. **Nichts einbauen, was Geld kostet** (kein GitHub-Actions-CI, keine Zertifikate/Dienste). Tests laufen lokal.
4. **Jedes neue Feature testen – über MCP UND per Maus/Tastatur** (Abschnitt „Neue Features testen“).
5. **Push, Tags und Releases nur auf ausdrücklichen Wunsch** des Nutzers.

## Entwicklungsumgebung

- **Entwickelt wird nativ unter Windows 11** (Rust stable, Host `x86_64-pc-windows-msvc`). Früher wurde unter Linux nach `x86_64-pc-windows-gnu` cross-kompiliert;
  `.cargo/config.toml` (mingw-Linker) ist ein Überbleibsel davon und wird nur für diesen Cross-Build gebraucht.
- Bauen/Testen: `cargo build --release` → `target\release\paneldeck.exe` (ca. 1 min); `cargo test --release` (alle Tests laufen parallel stabil).
  Die Windows-Teile (Tray, Hooks, ConPTY, Installer) lassen sich hier direkt ausprobieren – `TESTPLAN.md` Abschnitt 30/31.
- Shells: PowerShell ist die Haupt-Shell, das Bash-Tool ist Git Bash (`sha256sum`, `file` usw. vorhanden). `python3` ist hier nur der Store-Stub, `python` (3.11) funktioniert –
  die Release-Skripte finden das selbst. Python-Skripte immer mit `PYTHONUTF8=1` (sonst cp1252-Fehler an Umlauten). Mehrzeilige Python-Heredocs im Bash-Tool können an Anführungszeichen scheitern → lieber Skript-Datei schreiben und mit `python -I` starten.
- Installiert (winget): GitHub CLI `gh` (angemeldet), NSIS 3.13 (`C:\Program Files (x86)\NSIS\makensis.exe`). In bereits laufenden Shells fehlen sie evtl. im PATH → volle Pfade
  (`C:\Program Files\GitHub CLI\gh.exe`); die Skripte unter `tools/` suchen sie selbst.
- Der Datenordner `paneldeck-data` liegt neben der `.exe` (also unter `target\release\`); er sowie `target/` und `dist/` sind git-ignoriert.
- `.gitattributes` erzwingt LF für `*.sh` (CRLF bricht Git Bash). Skripte sind im Index ausführbar (`git update-index --chmod=+x`), Windows meldet sonst Modus-Änderungen.

## Neue Features testen (Vorgabe des Nutzers)

**Jedes neue Feature wird getestet, bevor es released wird – über MCP UND per Maus/Tastatur**, nicht nur eines von beiden.
- **MCP / Ende-zu-Ende:** `tools/smoke-mcp.ps1 [-Visible]` (57 Prüfungen, mit `-Visible` mehr) startet eine Kopie der .exe in einem Temp-Ordner (eigener Datenordner) und spricht per JSON-RPC mit `paneldeck.exe mcp`.
  Neue Werkzeuge/Verhalten dort ergänzen. Kein Eingriff in Maus/Tastatur des Nutzers.
- **Installer-Update:** `tools/update-test.ps1 [-Old <Setup.exe>] [-New <Setup.exe>]` installiert eine alte Version (Standard 0.19.1) in einen Temp-Ordner mit Schein-Profil, legt Daten/Autostart/Kontextmenü an, spielt den neuen Installer darüber (ruhender und laufender Hub) und räumt auf (27 Prüfungen). Bricht ab, wenn Paneldeck real installiert ist.
- **Maus/Tastatur/Screenshot:** `. tools\gui-test.ps1` (Win32: `Focus`, `Top`, `Click`, `Drag`, `Wheel`, `Press 'ctrl+x'`, `TypeText`, `MoveAbs`, `ShotPW`). `ShotPW` nutzt PrintWindow und funktioniert auch bei verdecktem Fenster.
  Der Nutzer darf dabei nichts anfassen; Fenster mit `Top` nach vorn, danach `Untop`. Testinstanzen immer aus einer **Kopie** der .exe starten (Datenordner daneben), nie mit dem echten Datenordner.
- **Nie** die echte `~/.claude/settings.json`, `~/.claude.json` oder Registry-Einträge des Nutzers mit Tests verändern: `USERPROFILE` auf einen Schein-Ordner setzen (die Shell-Ordner von Windows folgen dem!), Installer-Läufe mit `/D=<Temp>` und Aufräumen.
- Ein UAC-Dialog (z. B. „Neu starten“ einer Admin-Sitzung) lässt sich nicht automatisch bedienen – das bleibt Handarbeit des Nutzers.
- Windows-PowerShell 5.1: `Set-Content -Encoding UTF8` und `Process.StandardInput` schreiben ein BOM; JSON-Leser im Projekt tolerieren es (`state::strip_bom`).

## Changelog: Keep a Changelog 1.1.0 (Vorgabe des Nutzers)

`Changelog.md` folgt https://keepachangelog.com/de/1.1.0/ und wird mit `tools/changelog.py` geprüft (`python tools/changelog.py check`; `release.sh` ruft es auf und bricht bei Formatfehlern ab).
- **Aufbau:** `# Changelog`, Einleitung mit Verweis auf Keep a Changelog und Semantic Versioning, dann `## [Unreleased]`, darunter die Versionen **neueste zuerst** als `## [X.Y.Z] - JJJJ-MM-TT` (ISO-Datum), am Dateiende je eine Link-Definition `[X.Y.Z]: …/compare/v<vorherige>...v<X.Y.Z>` (und `[Unreleased]: …/compare/v<neueste>...HEAD`). Zurückgezogene Versionen bleiben stehen und bekommen `[YANKED]` hinter dem Datum.
- **Datum** = Datum des Git-Tags der Version (Entscheidung des Nutzers), nicht das spätere Veröffentlichungsdatum des GitHub-Releases; `changelog.py release` setzt das heutige Datum, das beim Release dem Tag-Datum entspricht.
- **Änderungstypen** (englische Überschriften wie auf der Seite, genau in dieser Reihenfolge, leere weglassen): `### Added` (neue Funktionen) · `### Changed` (Änderungen an bestehenden Funktionen) · `### Deprecated` (bald entfernt) · `### Removed` (entfernt) · `### Fixed` (Fehlerbehebungen) · `### Security` (Sicherheitslücken/Härtung). Einträge deutsch, für Menschen geschrieben (kein Commit-Log), Veraltetes/Entferntes/Inkompatibles immer nennen.
- **Arbeitsweise:** Änderungen laufend unter `## [Unreleased]` eintragen. Vor dem Release `python tools/changelog.py release <version>` (verschiebt den Inhalt in `## [<version>] - <heute>`, legt einen leeren `[Unreleased]` an, zieht die Links nach), dann Version in `Cargo.toml` und `tools/titles.txt` setzen, dann `release.sh`. Der Text des GitHub-Releases ist der Abschnitt der Version (`tools/release_notes.py`).
- Typ passend zur Versionsnummer wählen (siehe SemVer): `Fixed`/`Security` allein → PATCH; `Added`/`Changed` abwärtskompatibel → MINOR; nicht abwärtskompatibel oder `Removed` an der öffentlichen API → MAJOR.

## Agenten-Schnittstelle (MCP)

`paneldeck.exe mcp` = stdio-Brücke zur laufenden Instanz (Port + Token aus `ipc.json` im Datenordner), Stufe in den Einstellungen: Aus (Standard) / Lesen / Steuern.
- `src/mcp.rs`: Protokoll (JSON-RPC, Versionen), Werkzeugliste (`tools()` mit Mindeststufe), Brücke `run_bridge`, `key_bytes`, `base64`, Protokoll-Hilfen, `unregister_if_ours` (Deinstallation). Kennt kein egui.
- `src/hooks.rs`: `McpJob`/`mcp_rx`, `handle_mcp` (Token + **Stufe im Hub prüfen**, nicht erst in der Brücke), `call_mcp`. Die Stufe steht in der globalen `mcp::LEVEL` (UI schreibt sie jedes Frame).
- `src/app/mcp.rs`: Ausführung in der UI-Schleife (`poll_mcp`, pro Frame; Wartendes wie `run_command`/`wait_for_status`/`screenshot` bleibt in `McpState::pending`), Einstellungs-Tab, Seitenleisten-Chip.
- `src/terminal.rs`: `tail_text`, `screen_text`, `lines_abs`, `send_text_agent` (ohne Broadcast), Zähler in `ShellState`, `quiet_for`. `src/osc.rs`: `feed_at` (Positionen) – der Datenblock wird an den Sequenzen geteilt, damit die Prompt-Zeile exakt stimmt.
- Sicherheitsregeln (nicht aufweichen): Aus als Standard; kein Schreiben in die eigene Sitzung des Agenten (`PANELDECK_SESSION` der Brücke); nichts bei offener Freigabe-Frage; Schließen nur selbst angelegter Sitzungen (außer Einstellung); nie Admin-Profile, nie eigene Programme/Argumente; Antworten kennzeichnen Terminalinhalt als nicht vertrauenswürdig.
- `run_command`-Grenzen: PowerShell/cmd liefern nur `quality: approx`; Exit-Code bei cmd `null`; PowerShell-Exit-Code kann nach einem nativen Programm veraltet sein; langsame Prompts (Git Bash) werden über die Echo-Zeile abgegrenzt.

## Versionierung: Semantic Versioning 2.0.0 (Vorgabe des Nutzers)

Wir halten uns **immer** an https://semver.org/lang/de/ (Version 2.0.0). Die Version steht in `Cargo.toml`, im Git-Tag (`v<MAJOR>.<MINOR>.<PATCH>`; das `v` ist nur Tag-Konvention, die Version selbst hat keins), im Changelog-Abschnitt und in `tools/titles.txt`.
- **Format:** `X.Y.Z` mit nichtnegativen ganzen Zahlen ohne führende Nullen, aufsteigend (`0.9.0 < 0.10.0`). Pre-Release nur als `1.0.0-rc.1` (nie als Namenszusatz im Titel wie „Release Candidate“ bei einer normalen Nummer); Build-Metadaten (`+…`) zählen nicht für die Rangfolge.
- **Öffentliche API** (SemVer verlangt, sie festzulegen):
  1. die **MCP-Schnittstelle**: Werkzeugnamen, Parameter, Felder der Antworten, Fehlercodes, Stufen;
  2. die **Kommandozeile**: `--open`, `--claude`, `--tray`, `mcp` (sowie `--hook`/`--uninstall-hooks` für Installer und Claude-Hooks);
  3. die **Datenformate** im Datenordner: `settings.json`, `state.json`, `prompts.json`, `themes/*.json`, `mcp-log.jsonl`.
  Nicht öffentlich (dürfen sich ändern): `ipc.json`, das interne IPC-Protokoll, Hook-Umgebungsvariablen, Oberfläche (Layout, Texte), Rust-Interna.
- **PATCH** (`Z`) nur für **abwärtskompatible Fehlerbehebungen** – keine neuen Funktionen. **MINOR** (`Y`) für abwärtskompatible neue Funktionen und für als veraltet markierte (deprecated) API-Teile; neue Funktionen auch dann MINOR, wenn sie klein sind (z. B. „Ordner ziehen“). **MAJOR** (`X`) für jede nicht abwärtskompatible Änderung der öffentlichen API (Werkzeug/Parameter/Feld entfernt oder umbenannt, Datenformat ohne Migration geändert, Argument entfernt). Beim Erhöhen einer Stelle gehen die rechten auf 0.
- **Veröffentlichte Versionen werden nie verändert:** kein Tag verschieben, keine Release-Dateien austauschen; ein Fehler wird mit der nächsten PATCH-Version behoben.
- **Veraltete API zuerst als „deprecated“ im Changelog** (in einer MINOR-Version) markieren und erst in der nächsten MAJOR-Version entfernen.
- **`0.y.z` ist „initiale Entwicklung“** (alles darf sich ändern) – das galt bis 0.20.0. Ab **1.0.0** ist die öffentliche API oben verbindlich; vor 1.0.0 trotzdem schon so versionieren (PATCH = nur Fehlerbehebungen).
- Bis einschließlich 0.20.0 gab es PATCH-Versionen mit neuen Funktionen (0.15.2–0.15.5, 0.17.1–0.17.3); das ist in der initialen Entwicklung erlaubt, wiederholt sich aber nicht.
- Pre-Release-Nummern (`1.0.0-rc.1`) funktionieren mit `release.sh` (Dateinamen enthalten die volle Version); den GitHub-Haken „Pre-release“ dann gesetzt lassen, `--final` nur für Versionen ohne `-`.

## Release

- **Es gibt genau zwei Release-Dateien, beide mit Version im Namen und beide mit den Lizenzen:** der Installer `Paneldeck-Setup-<version>.exe` (zeigt die LICENSE als Seite, legt LICENSE, THIRD_PARTY*, Schrift-Lizenzen ins Programmverzeichnis)
  und das portable ZIP `paneldeck-<version>-windows-x64.zip` (Ordner `paneldeck-<version>/` mit `paneldeck.exe`, LICENSE, THIRD_PARTY.md, THIRD_PARTY_LICENSES.txt, README, Changelog, `licenses/`). Eine nackte `.exe` wird nicht mehr veröffentlicht.
  (Die Programmdatei heißt immer `paneldeck.exe`, weil Autostart, Kontextmenü, Hooks und der MCP-Eintrag darauf verweisen; `dist/paneldeck-<version>.exe` ist nur das Zwischenprodukt des Release-Skripts.)
- **Lizenztexte der Abhängigkeiten:** `python tools/gen_licenses.py` erzeugt `THIRD_PARTY_LICENSES.txt` aus `Cargo.lock` (fehlende Crate-Lizenzen aus `tools/license-extra/`); `release.sh` prüft per `--check`, dass sie aktuell ist. Nach jeder Änderung an Abhängigkeiten neu erzeugen.
- **Keine Pfade des Build-Rechners in der .exe:** `tools/common.sh` setzt `RUSTFLAGS` mit `--remap-path-prefix` (Cargo-, Rustup- und Projektordner). Beim Release prüfen: `grep -a -c "C:\Users" dist/paneldeck-<v>.exe` muss 0 ergeben.
- Vorher: `python tools/changelog.py release <version>` (siehe „Changelog“), `Cargo.toml`-Version setzen, `tools/titles.txt` (`<version>|Paneldeck <version> – Titel`) ergänzen. Das Skript prüft alle drei (Changelog über `tools/changelog.py check`).
- **`tools/release.sh <version> "<Nachricht>" [--publish] [--final] [--no-git] [--skip-smoke]`** (aus Git Bash: `bash tools/release.sh …`) – läuft unter Windows und Linux (`tools/common.sh` erkennt die Umgebung):
  Tests → Release-Build → `dist/paneldeck-<v>.exe` → Installer (NSIS, Icon aus `assets\icon.ico` via `MUI_ICON`/`MUI_UNICON`) → SHA-256 → **Smoke-Test** (`tools/smoke-mcp.ps1` gegen die gebaute .exe, nur Windows) → **Update-Test** (`tools/update-test.ps1`) → Commit + Tag `v<v>` → ZIP + `release-notes-<v>.md`.
  - `--no-git`: nur bauen/packen (Probelauf, kein Commit/Tag/Push).
  - `--publish`: zusätzlich Branch + Tag pushen und GitHub-Release anlegen (`gh release create`, Titel/Text aus `dist/gh-title-<v>.txt` / `dist/gh-notes-<v>.md`, Assets: portables ZIP + Installer).
    Standard ist **Pre-release** wie bei allen bisherigen Releases; `--final` für ein normales Release (ab 1.0).
  - Commit-Nachricht endet mit der Attribution (`ATTRIBUTION="Co-Authored-By: …"` setzen).
- Remote `origin` = `github.com/zibbozz/paneldeck` (Branch `main`). Die Historie beginnt mit 1.0.0; die frühere Entwicklung (0.x, anderer Projektname) liegt in einem getrennten, privaten Archiv. Push, Tags und Releases nur auf ausdrücklichen Wunsch.
- Push, Tags und Releases sind nach außen sichtbar → **nur auf ausdrücklichen Wunsch** (`--publish` nicht von selbst setzen).

## Bekannte Stolpersteine

- NSIS findet Pfade mit `/` nicht → `abspath` in `tools/common.sh` liefert unter Windows Backslashes.
- `tools/release_notes.py`/`release_texts.py` lesen/schreiben jetzt explizit UTF-8; ohne das bricht Python unter Windows ab.
- Die Setup-`.exe` aus dem Probelauf enthält denselben Code wie die veröffentlichte, ist aber nicht byte-identisch (Build ist nicht reproduzierbar) – Release-Assets nur aus dem echten Lauf hochladen.
- Das Hub-Fenster ist transparent; Screenshots per Bildschirmkopie zeigen dann, was dahinter liegt → `ShotPW` oder das MCP-Werkzeug `screenshot` nehmen.
- Ein im Tray versteckter Hub ist „minimiert + ohne Taskleistenbutton“ (`IsWindowVisible` bleibt wahr; `IsIconic` prüfen).
