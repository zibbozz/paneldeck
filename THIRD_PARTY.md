# Drittanbieter-Software und Lizenzen

Paneldeck ist aus freier Software aufgebaut. Die wichtigsten Bestandteile und ihre Lizenzen:

## Bibliotheken (Rust)
| Bestandteil | Verwendung | Lizenz |
|---|---|---|
| egui / eframe | Oberfläche, Fenster, OpenGL-Darstellung | MIT oder Apache-2.0 |
| alacritty_terminal, vte | Terminal-Emulation | Apache-2.0 (vte: Apache-2.0 oder MIT) |
| portable-pty | Pseudo-Konsole (ConPTY unter Windows) | MIT |
| ttf-parser | Auslesen der Emoji-Bitmaps und Ligaturen | MIT oder Apache-2.0 |
| image | PNG-Dekodierung, Skalierung | MIT oder Apache-2.0 |
| serde, serde_json | Einstellungen und Zustand | MIT oder Apache-2.0 |
| tauri-winrt-notification | Windows-Benachrichtigungen | MIT oder Apache-2.0 |
| tray-icon | Tray-Symbol | MIT oder Apache-2.0 |
| rfd | Datei-/Ordnerdialoge | MIT |
| arboard | Zwischenablage (Bilder) | MIT oder Apache-2.0 |
| windows-sys, raw-window-handle | Windows-Fensterfunktionen, WinHTTP (Update-Prüfung) | MIT oder Apache-2.0 |
| miniz_oxide | Entpacken des portablen ZIPs beim Selbst-Update | MIT, Zlib oder Apache-2.0 |
| embed-resource | Icon/Versionsinfo in der .exe | MIT |
| rusqlite (mit eingebautem SQLite) | lokale Datenbank und Sicherungen | MIT (SQLite selbst: gemeinfrei/Public Domain) |
| chrono | Datum und Uhrzeit | MIT oder Apache-2.0 |

Alle weiteren transitiven Abhängigkeiten stehen unter MIT, Apache-2.0, BSD, Zlib, ISC, BSL-1.0, Unicode, CC0 oder Unlicense (teils wahlweise); eine Ausnahme ist `option-ext` unter der MPL-2.0 (Copyleft nur für die Datei selbst; Paneldeck bindet sie unverändert ein, der Quelltext liegt bei crates.io). `r-efi` bietet LGPL-2.1 nur als Wahloption neben MIT/Apache an. Die **vollständigen Lizenztexte** aller Pakete stehen in `THIRD_PARTY_LICENSES.txt` (erzeugt mit `python tools/gen_licenses.py`), die genaue Liste ergibt sich aus `Cargo.lock`.

## Schriften (eingebettet)
| Schrift | Lizenz |
|---|---|
| Cascadia Mono (Microsoft) | SIL Open Font License 1.1 |
| Noto Color Emoji (Google) | SIL Open Font License 1.1 |
| Lucide-Icons (`lucide.ttf` aus dem npm-Paket `lucide-static` 1.66.0, https://lucide.dev; Icon-Explorer, Icons `lucide:<name>`) | ISC (aus Feather Icons abgeleitete Icons: MIT) |
| Standardschriften von egui (Hack, Ubuntu-Light, Noto Emoji, emoji-icon-font) | siehe egui-Projekt: Hack (MIT/Bitstream Vera), Ubuntu Font Licence, SIL OFL |

Die Lizenztexte der eingebetteten Schriften liegen unter `assets/fonts/`.

## Installer
Das Setup wird mit NSIS erstellt (zlib/libpng-Lizenz).
