# Drittanbieter-Software und Lizenzen

Paneldeck ist aus freier Software aufgebaut. Die wichtigsten Bestandteile und ihre Lizenzen:

## Bibliotheken (Rust)
| Bestandteil | Verwendung | Lizenz |
|---|---|---|
| egui / eframe | Oberfläche, Fenster, OpenGL-Darstellung | MIT oder Apache-2.0 |
| alacritty_terminal, vte | Terminal-Emulation | Apache-2.0 (vte: Apache-2.0 oder MIT) |
| portable-pty | Pseudo-Konsole (ConPTY unter Windows); als Kopie mit kleinem Patch in `vendor/portable-pty` (Laden der mitgelieferten ConPTY) | MIT |
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
| iana-time-zone | Name der Zeitzone für den iCal-Export des Kalenders (kam schon über chrono mit) | MIT oder Apache-2.0 |
| regex-automata | Regex-Suche des Chat-Werkzeugs `grep` (kam schon über alacritty_terminal mit) | MIT oder Apache-2.0 |
| ropey (mit str_indices) | Text des Editors (Rope, auch für sehr große Dateien) | MIT (str_indices: MIT oder Apache-2.0) |
| regex | Suchen/Ersetzen im Editor (reguläre Ausdrücke), `.editorconfig`-Muster | MIT oder Apache-2.0 |
| encoding_rs | Kodierungen im Editor (Windows-1252) | (Apache-2.0 oder MIT) und BSD-3-Clause (WHATWG-Daten) |
| similar | Unterschied Datei/Puffer im Editor | Apache-2.0 |
| unicode-segmentation, unicode-width | Graphem-Grenzen und Spaltenbreiten im Editor | MIT oder Apache-2.0 |
| ignore (mit globset, walkdir, same-file) | Suche in Dateien im Editor: Ordner durchlaufen mit `.gitignore`-Regeln | Unlicense oder MIT |
| chrono-tz (mit phf) | Weltuhren im Reiter „Zeit“: Zeitzonen aus der IANA-Datenbank | MIT oder Apache-2.0 |
| rodio, cpal, hound | Klänge im Reiter „Zeit“ (nur Windows): Wiedergabe über WASAPI, WAV lesen | MIT oder Apache-2.0 (hound, cpal: Apache-2.0) |
| symphonia (symphonia-core, -metadata, -bundle-mp3) | MP3 lesen für eigene Klangdateien im Reiter „Zeit“ (nur Windows) | MPL-2.0 |

Alle weiteren transitiven Abhängigkeiten stehen unter MIT, Apache-2.0, BSD, Zlib, ISC, BSL-1.0, Unicode, CC0 oder Unlicense (teils wahlweise); Ausnahmen sind `option-ext` und `symphonia` (mit seinen Teil-Crates) unter der MPL-2.0 (Copyleft nur für die Dateien selbst; Paneldeck bindet sie unverändert ein, der Quelltext liegt bei crates.io). `r-efi` bietet LGPL-2.1 nur als Wahloption neben MIT/Apache an. Die **vollständigen Lizenztexte** aller Pakete stehen in `THIRD_PARTY_LICENSES.txt` (erzeugt mit `python tools/gen_licenses.py`), die genaue Liste ergibt sich aus `Cargo.lock`.

## Schriften (eingebettet)
| Schrift | Lizenz |
|---|---|
| Cascadia Mono (Microsoft) | SIL Open Font License 1.1 |
| Noto Color Emoji (Google) | SIL Open Font License 1.1 |
| Lucide-Icons (`lucide.ttf` aus dem npm-Paket `lucide-static` 1.66.0, https://lucide.dev; Icon-Explorer, Icons `lucide:<name>`) | ISC (aus Feather Icons abgeleitete Icons: MIT) |
| Standardschriften von egui (Hack, Ubuntu-Light, Noto Emoji, emoji-icon-font) | siehe egui-Projekt: Hack (MIT/Bitstream Vera), Ubuntu Font Licence, SIL OFL |

Die Lizenztexte der eingebetteten Schriften liegen unter `assets/fonts/`.

## Mitgelieferte Programmdateien
| Bestandteil | Verwendung | Lizenz |
|---|---|---|
| ConPTY von Microsoft (`conpty.dll`, `OpenConsole.exe`; NuGet-Paket Microsoft.Windows.Console.ConPTY, x64, unverändert und von Microsoft signiert) | neuere Pseudo-Konsole neben `paneldeck.exe` (Installer und portables ZIP) | MIT |

Version und SHA-256 stehen in `tools/fetch-conpty.ps1`; der Lizenztext liegt als `licenses/LICENSE-ConPTY.txt` bei, die Hinweise zu Fremdcode in OpenConsole (NOTICE.md von microsoft/terminal, Tag v1.25.3131.0: u. a. {fmt}, wil, chromium/base/numerics, jsoncpp) als `licenses/NOTICE-ConPTY.md` (Quellen: `tools/license-extra/conpty-MIT.txt`, `conpty-NOTICE.md`); beides steht auch in `THIRD_PARTY_LICENSES.txt`. Quelltext: https://github.com/microsoft/terminal

## Installer
Das Setup wird mit NSIS erstellt (zlib/libpng-Lizenz).
