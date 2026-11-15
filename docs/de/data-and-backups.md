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
| `chats/` | Verläufe der [Chats](chat.md), je Chat eine `<id>.json` (siehe [Chatverläufe](#chatverläufe)) |
| `prompts.json` | Prompt-Bibliothek |
| `themes/*.json` | eigene Themes (Beispiel liegt bei) |
| `system-prompt.md` | deine eigene Vorlage des [Systemprompts für Agenten](agent-interface.md#systemprompt) (nur wenn du eine gespeichert hast); `system-prompt.md.bak` = die vorige Vorlage nach *Auf Standard zurücksetzen* |
| `clipboard/` | Bilder aus der Zwischenablage (nach 14 Tagen gelöscht) |
| `claude-hooks*.json` | von Paneldeck erzeugte Hook-Konfiguration |
| `crash.log` | Absturzprotokoll (nur wenn etwas schiefging) |
| `mcp-log.jsonl` | Aufrufprotokoll der Agenten-Schnittstelle (nur wenn sie benutzt wurde) |
| `profiles/<name>/` | Datenordner der [Profile](command-line.md#profile-und-mehrere-instanzen), jeweils mit demselben Aufbau |
| `ipc.json` | Port und Token der laufenden Instanz (intern) |

`settings.json`, `state.json`, `prompts.json`, `themes/*.json`, `system-prompt.md`, `chats/*.json` und `mcp-log.jsonl` sind stabile Formate: Neuere Versionen lesen ältere Dateien weiter, neue Felder sind optional.

## Chatverläufe
Jeder [Chat](chat.md) speichert seinen Verlauf in `chats/<id>.json` (`<id>` = Kennung der Sitzung; Formatversion 1, seit 2.2.0):

```json
{
  "version": 1,
  "context_start": 2,
  "messages": [
    { "role": "user", "content": "Which sessions are open?", "ts": 1763200000 },
    { "role": "assistant", "content": "", "ts": 1763200001,
      "tool_calls": [ { "id": "c1", "name": "list_sessions", "arguments": {} } ] },
    { "role": "tool", "content": "{\"sessions\":[]}", "ts": 1763200002,
      "tool_call_id": "c1", "tool_name": "list_sessions", "status": "ok" },
    { "role": "assistant", "content": "There are no sessions.", "ts": 1763200003 },
    { "role": "assistant", "content": "", "ts": 1763200010, "error": "connection refused" }
  ]
}
```

- `role` ist `user`, `assistant`, `tool` oder `system`; `ts` = Sekunden seit 1970 (UTC). Eine Antwort kann `tool_calls`, einen Fehler `error` (angezeigt mit *Wiederholen*) oder einen Hinweis `note` tragen (z. B. „an der Token-Grenze abgeschnitten“; geht nicht an das Modell). Ein Werkzeug-Ergebnis hat `tool_call_id`, `tool_name` und `status` (`ok`, `error`, `denied`). `context_start` = die erste Nachricht, die das Modell bei der letzten Anfrage noch gesehen hat (fehlt, solange nichts weggelassen wurde). Die mit *Immer für diesen Chat* erlaubten Werkzeuge stehen ebenfalls in der Datei.
- Die Datei wird **atomar** geschrieben (temporäre Datei, dann umbenennen). Lesen ist tolerant: BOM erlaubt, unbekannte Felder werden ignoriert, eine einzelne ungültige Nachricht wird übersprungen. Eine beschädigte oder zu große Datei (über 8 MB) ergibt einen leeren Chat mit Hinweis; die Datei wird vorher als `chats/<id>.defekt-<Zeit>.json` gesichert. Beim Speichern verliert ein Verlauf über 8 MB seine ältesten Nachrichten.
- Eine Verlaufsdatei, die zu keiner Sitzung gehört (z. B. nach einer zurückgesetzten `state.json`), wird nie in einen neuen Chat geladen, sondern als `chats/<id>.orphan-<Zeit>.json` beiseitegelegt.
- **Schließen eines Chats löscht seine Datei** (samt ihrer `.defekt-…`-Sicherungen). *Verlauf als Datei speichern …* im Kontextmenü des Chats exportiert sie vorher als Markdown.

## Chat-Felder in settings.json und state.json
- `settings.json` → `llm_servers`: die Server aus *Einstellungen → Modelle*, je mit `id`, `name`, `kind` (`ollama`/`openai`), `url`, optional `api_key` (Base64 eines Windows-DPAPI-Blobs – nie Klartext; nur derselbe Windows-Benutzer auf demselben Rechner kann ihn entschlüsseln), optional `receive_timeout_secs`, `models` (Einstellungen je Modell: `name`, `level`, `disabled_tools`, `ask_write`, `max_steps`, optional `tools`, `context_length`, `temperature`) und `last_models` (Ergebnis des letzten *Verbinden*). Ein Server-Eintrag, der sich nicht lesen lässt (z. B. ein unbekannter `kind` einer neueren Version), bleibt unverändert erhalten und wird nur lesbar mit *Entfernen* angezeigt.
- `settings.json` → `last_chat_model`: `{ "server": "<id>", "model": "<name>" }`, die letzte Wahl im Dialog *Neuer Chat*.
- `state.json` → je Sitzung `kind` (`"chat"`; fehlt = Terminal) und `chat` (`{ "server": "<id>", "model": "<name>" }`). Ein älteres Paneldeck (2.1), das diese Datei öffnet, zeigt Chats als Terminals und lässt `chats/*.json` in Ruhe.

## Sicherungen
*Einstellungen → Daten & Sicherung* steuert die automatischen Sicherungen von `paneldeck.db`: Standard ist eine Kopie alle 24 Stunden, behalten werden zwei (zusätzlich zur Datenbank selbst). War der Rechner zur fälligen Zeit aus, wird die Sicherung beim nächsten Start nachgeholt; mehrere verpasste ergeben nur eine. Du kannst einen anderen Sicherungsordner wählen, ihn öffnen, mit **Jetzt sichern** eine Kopie anlegen (manuelle Kopien werden nie automatisch gelöscht) und jede Kopie aus der Liste **wiederherstellen** – dabei wird zuerst der aktuelle Stand gesichert, danach beendet sich Paneldeck und spielt die Wiederherstellung beim nächsten Start ein. Vor jedem Datenbank-Update (Migration) legt Paneldeck ebenfalls eine Kopie an und fasst eine Datenbank einer neueren Version nie an.

- `--backup-dir <ordner>` schreibt die Sicherungen eines Starts in einen anderen Ordner (die Ordnerwahl ist dann gesperrt; siehe [Kommandozeile](command-line.md#--backup-dir-ordner)).
- Vor einem Update auf eine **neue Hauptversion** legt das Selbst-Update zuerst eine Datenbank-Sicherung an (siehe [Updates](updates.md)).
- Der **Rückgängig-Verlauf** der Todo-Liste (3 bis 100 Schritte, Standard 25) wird im selben Tab eingestellt.

## Daten verschieben oder kopieren
Paneldeck beenden, dann den ganzen Datenordner kopieren. Um ihn von einem anderen Ort zu nutzen, Paneldeck mit `--data-dir <ordner>` starten oder `PANELDECK_DATA` setzen.

API-Schlüssel von [Chat-Servern](chat.md#api-schlüssel) sind für deinen Windows-Benutzer verschlüsselt: Unter einem anderen Benutzerkonto oder auf einem anderen Rechner lassen sie sich nicht entschlüsseln – dann in *Einstellungen → Modelle* neu eingeben.
