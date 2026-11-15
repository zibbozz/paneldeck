[← README](../../README.de.md) · [English](../en/data-and-backups.md) · **Deutsch**

# Datenordner und Sicherungen

## Wo der Datenordner liegt
Paneldeck legt alles in einem Datenordner ab. Welcher es ist, entscheidet sich beim Start (der erste Treffer gilt – Einzelheiten unter [Kommandozeile](command-line.md#suche-des-datenordners)):
1. `--data-dir <ordner>`
2. die Umgebungsvariable `PANELDECK_DATA`
3. `paneldeck-data` neben `paneldeck.exe`, wenn beschreibbar
4. `%APPDATA%\Paneldeck`

Mit `--profile <name>` wird der Unterordner `profiles\<name>` dieses Ordners genutzt.
*Einstellungen → Info* zeigt den aktiven Datenordner, seine Herkunft und das Profil.

### Portabel oder installiert
- **Portables ZIP:** `paneldeck-data` entsteht neben `paneldeck.exe` – der ganze Ordner lässt sich verschieben oder auf einen USB-Stick kopieren. Ist der Ort nicht beschreibbar (z. B. ein schreibgeschützter Ordner), nutzt Paneldeck `%APPDATA%\Paneldeck`.
- **Installer:** installiert pro Benutzer nach `%LOCALAPPDATA%\Programs\Paneldeck`; der Datenordner ist auch hier `paneldeck-data` neben der installierten `paneldeck.exe`. Beim Deinstallieren fragt das Deinstallationsprogramm, ob `paneldeck-data` ebenfalls gelöscht werden soll.

## Was darin liegt
| Datei/Ordner | Inhalt |
|---|---|
| `settings.json` | Einstellungen (von Hand editierbar); `settings.json.bak-<Zeit>` = Kopie von `--reset-settings` |
| `state.json` | Sitzungen, Projekte, Layout, weitere Fenster (`state.defekt.json` = Sicherung, falls die Datei beschädigt war) |
| `paneldeck.db` | die lokale Datenbank (SQLite) mit Todo-Liste, Projekten und Tickets (`paneldeck.db-wal` / `-shm` gibt es nur, solange die App läuft) |
| `backups/` | automatische und manuelle Kopien der Datenbank (der Ordner ist änderbar) |
| `scrollback/` | gespeicherter Terminalverlauf |
| `prompts.json` | Prompt-Bibliothek |
| `themes/*.json` | eigene Themes (Beispiel liegt bei) |
| `system-prompt.md` | deine eigene Vorlage des [Systemprompts für Agenten](agent-interface.md#systemprompt) (nur wenn du eine gespeichert hast); `system-prompt.md.bak` = die vorige Vorlage nach *Auf Standard zurücksetzen* |
| `clipboard/` | Bilder aus der Zwischenablage (nach 14 Tagen gelöscht) |
| `claude-hooks*.json` | von Paneldeck erzeugte Hook-Konfiguration |
| `crash.log` | Absturzprotokoll (nur wenn etwas schiefging) |
| `mcp-log.jsonl` | Aufrufprotokoll der Agenten-Schnittstelle (nur wenn sie benutzt wurde) |
| `profiles/<name>/` | Datenordner der [Profile](command-line.md#profile-und-mehrere-instanzen), jeweils mit demselben Aufbau |
| `ipc.json` | Port und Token der laufenden Instanz (intern) |

`settings.json`, `state.json`, `prompts.json`, `themes/*.json`, `system-prompt.md` und `mcp-log.jsonl` sind stabile Formate: Neuere Versionen lesen ältere Dateien weiter, neue Felder sind optional.

## Sicherungen
*Einstellungen → Daten & Sicherung* steuert die automatischen Sicherungen von `paneldeck.db`: Standard ist eine Kopie alle 24 Stunden, behalten werden zwei (zusätzlich zur Datenbank selbst). War der Rechner zur fälligen Zeit aus, wird die Sicherung beim nächsten Start nachgeholt; mehrere verpasste ergeben nur eine. Du kannst einen anderen Sicherungsordner wählen, ihn öffnen, mit **Jetzt sichern** eine Kopie anlegen (manuelle Kopien werden nie automatisch gelöscht) und jede Kopie aus der Liste **wiederherstellen** – dabei wird zuerst der aktuelle Stand gesichert, danach beendet sich Paneldeck und spielt die Wiederherstellung beim nächsten Start ein. Vor jedem Datenbank-Update (Migration) legt Paneldeck ebenfalls eine Kopie an und fasst eine Datenbank einer neueren Version nie an.

- `--backup-dir <ordner>` schreibt die Sicherungen eines Starts in einen anderen Ordner (die Ordnerwahl ist dann gesperrt; siehe [Kommandozeile](command-line.md#--backup-dir-ordner)).
- Vor einem Update auf eine **neue Hauptversion** legt das Selbst-Update zuerst eine Datenbank-Sicherung an (siehe [Updates](updates.md)).
- Der **Rückgängig-Verlauf** der Todo-Liste (3 bis 100 Schritte, Standard 25) wird im selben Tab eingestellt.

## Daten verschieben oder kopieren
Paneldeck beenden, dann den ganzen Datenordner kopieren. Um ihn von einem anderen Ort zu nutzen, Paneldeck mit `--data-dir <ordner>` starten oder `PANELDECK_DATA` setzen.
