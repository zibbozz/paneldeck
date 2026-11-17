[← README](../../README.de.md) · [English](../en/command-line.md) · **Deutsch**

# Kommandozeile

```
paneldeck.exe [Optionen]
paneldeck.exe mcp [--data-dir <ordner>] [--profile <name>] [--lang en|de]
```

`paneldeck.exe --help` gibt dieselben Informationen aus, mit einem Beispiel zu jeder Option. Hilfe, Version und Fehlermeldungen erscheinen in der Konsole des aufrufenden Terminals (cmd, PowerShell); ohne Konsole (z. B. Win+R oder eine Verknüpfung) zeigt ein Meldungsfenster den Text.

Optionen mit Wert lassen sich auch als `--option=wert` schreiben (`--profile=arbeit`). **Unbekannte Optionen und überzählige Argumente sind ein Fehler** (Exit-Code 2) – vor 2.1.0 wurden sie still ignoriert.

## Überblick
| Option | Zweck |
|---|---|
| `--open <ordner>` | Neue Sitzung in `<ordner>` öffnen (eine Datei öffnet ihren Ordner) |
| `--claude` | Mit `--open`: Claude Code in der neuen Sitzung starten |
| `--tray` | Minimiert im Tray starten |
| `--no-tray` | Kein Tray-Symbol für diesen Start |
| `--data-dir <ordner>` | `<ordner>` als Datenordner verwenden |
| `--profile <name>` | Profil `<name>` verwenden (eigener Datenordner) |
| `--backup-dir <ordner>` | Datenbank-Sicherungen für diesen Start nach `<ordner>` schreiben |
| `--lang en\|de` | Sprache der Oberfläche für diesen Start |
| `--reset-settings` | Einstellungen zurücksetzen (eine Sicherungskopie bleibt) |
| `--no-update-check` | Keine automatische Update-Prüfung für diesen Start |
| `--version` | Version ausgeben und beenden |
| `--help`, `-h`, `-?` | Hilfe ausgeben und beenden |
| `<ordner>` | Ein einzelner Ordner ohne Option wirkt wie `--open <ordner>` |
| `mcp` | Agenten-Schnittstelle (MCP-Server über stdin/stdout) |

`--hook` und `--uninstall-hooks` benutzen intern die Claude-Code-Hooks und das Deinstallationsprogramm.

## Optionen im Einzelnen

### --open \<ordner\>
Öffnet im laufenden Paneldeck (oder nach dem Start) eine neue Terminal-Sitzung in `<ordner>`. Ist `<ordner>` eine Datei, gilt ihr Ordner. Das Explorer-Kontextmenü nutzt das; Laufwerkswurzeln wie `"C:\"` funktionieren auch. Ein relativer Pfad gilt ab dem aktuellen Ordner des Aufrufers; Linux-Pfade (`/…`, `~`) werden für WSL unverändert weitergegeben.

    paneldeck.exe --open "C:\Dev\meine-app"

### --claude
Zusammen mit `--open`: Die neue Sitzung startet Claude Code (Claude-Modus nötig).

    paneldeck.exe --open "C:\Dev\meine-app" --claude

Die Sitzung nutzt deine Standard-[Claude-Startoptionen](claude-code.md#startoptionen). In einem Paneldeck-Terminal eingegeben, wendet `--open … --claude` deine Standards für zusätzliche Ordner, erlaubte Werkzeuge, eine MCP-Konfiguration oder `--safe-mode` nie an, und die neue Sitzung wird nur mit einer [Agenten-Kette](agent-interface.md#sicherheit) markiert, wenn dieses Terminal eine hat.

### --tray
Startet minimiert im Tray (so startet *Mit Windows starten*). Läuft Paneldeck schon, wird es stattdessen angezeigt.

    paneldeck.exe --tray

### --no-tray
Startet ohne Tray-Symbol. *Schließen → Tray* und *Minimieren → Tray* verhalten sich dann wie bei einem normalen Fenster (Schließen beendet, Minimieren in die Taskleiste); *Einstellungen → Fenster* zeigt einen Hinweis.

    paneldeck.exe --no-tray

### --data-dir \<ordner\>
Nutzt `<ordner>` für Einstellungen, Sitzungen, Datenbank und alles Weitere. Der Ordner wird bei Bedarf angelegt; lässt er sich nicht anlegen oder beschreiben, bricht Paneldeck mit einer Fehlermeldung ab, die den Ordner nennt (Exit-Code 1, kein stiller Rückfall). Ein relativer Pfad gilt relativ zum aktuellen Verzeichnis.

    paneldeck.exe --data-dir "D:\PaneldeckDaten"

### --profile \<name\>
Nutzt `<Datenordner>\profiles\<name>` und läuft als eigene Instanz neben der normalen. Namen: 1–40 Zeichen aus `A–Z`, `a–z`, `0–9`, `_` und `-`; Windows-Gerätenamen (`CON`, `NUL`, `COM1` …) sind nicht erlaubt. Kombinierbar mit `--data-dir` oder `PANELDECK_DATA`.

    paneldeck.exe --profile arbeit
    paneldeck.exe --data-dir "D:\PaneldeckDaten" --profile kunde-a

### --backup-dir \<ordner\>
Schreibt Datenbank-Sicherungen für diesen Start nach `<ordner>`. *Einstellungen → Daten & Sicherung* zeigt den Ordner mit „(festgelegt mit --backup-dir)“ und sperrt die Ordnerwahl; der gespeicherte Sicherungsordner bleibt unverändert.

    paneldeck.exe --backup-dir "E:\Sicherungen\Paneldeck"

### --lang en|de
Sprache von Oberfläche und Meldungen für diesen Start (auch für `--help` und Fehlermeldungen). Die gespeicherte Sprache bleibt; Texte, die Paneldeck speichert (z. B. Standardnamen), behalten die gespeicherte Sprache. Wählst du bei einem solchen Start unter *Einstellungen → Allgemein* eine Sprache, wird diese Wahl gespeichert.

    paneldeck.exe --lang en

### --reset-settings
Benennt `settings.json` in `settings.json.bak-<JJJJMMTT-HHMMSS>` um und startet mit Standard-Einstellungen; ein Hinweis nennt die Sicherung. Sitzungen, Prompts, Themes und Datenbank bleiben unberührt. Nicht möglich, solange Paneldeck mit demselben Datenordner läuft (Fehler, Exit-Code 1).

    paneldeck.exe --reset-settings
    paneldeck.exe --profile arbeit --reset-settings

### --no-update-check
Unterdrückt die automatische Update-Prüfung für diesen Start, auch wenn sie unter *Einstellungen → Info* eingeschaltet ist. Eine Prüfung von Hand bleibt möglich. Siehe [Updates](updates.md).

    paneldeck.exe --no-update-check

### --version
Gibt die Versionsnummer aus.

    paneldeck.exe --version

### --help, -h, -?
Gibt die Hilfe aus (in der Sprache von `--lang` bzw. der gespeicherten Einstellungen).

    paneldeck.exe --help > paneldeck-hilfe.txt

### Ein einzelner Ordner ohne Option
Genau ein Argument ohne Option, das ein vorhandener Ordner ist, wirkt wie `--open <ordner>`; eine vorhandene Datei öffnet ihren Ordner. Das passiert, wenn du einen Ordner oder eine Datei auf `paneldeck.exe` ziehst. Es lässt sich mit anderen Optionen kombinieren (z. B. `--profile`), aber nicht mit `--open` oder `mcp`; ein zweites solches Argument oder ein Pfad, den es nicht gibt, ist ein Fehler (Exit-Code 2).

    paneldeck.exe "C:\Dev\meine-app"

### mcp
Startet den MCP-Server für KI-Agenten über stdin/stdout und spricht mit dem laufenden Paneldeck. Den Hub sucht er in dieser Reihenfolge: `--data-dir`/`--profile`, das Paneldeck-Terminal, aus dem er gestartet wurde, dann die Suche des Datenordners unten. Nach `mcp` sind nur `--data-dir`, `--profile`, `--lang` und `--help` erlaubt. Die Agenten-Schnittstelle muss unter *Einstellungen → Agenten* eingeschaltet sein – siehe [Agenten-Schnittstelle](agent-interface.md).

    claude mcp add paneldeck --scope user -- "C:\Program Files\Paneldeck\paneldeck.exe" mcp
    paneldeck.exe mcp --profile arbeit

## Suche des Datenordners
Der Datenordner wird in dieser Reihenfolge gewählt (der erste Treffer gilt):
1. `--data-dir <ordner>`
2. die Umgebungsvariable **`PANELDECK_DATA`**
3. der portable Ordner `paneldeck-data` neben `paneldeck.exe` (wenn beschreibbar – dort liegen auch die Daten der Installation pro Benutzer)
4. `%APPDATA%\Paneldeck`
5. das aktuelle Verzeichnis (Notlösung)

`--profile <name>` nutzt dann den Unterordner `profiles\<name>` dieses Ordners. Zeigt `--data-dir` oder `PANELDECK_DATA` auf einen unbrauchbaren Ordner, bricht Paneldeck mit einer Fehlermeldung ab, statt auszuweichen.
*Einstellungen → Info* und *Diagnose kopieren* zeigen den Datenordner, seine Herkunft (Parameter, Umgebungsvariable, portabel, `%APPDATA%`), das Profil und die aktiven Startoptionen. Siehe [Datenordner und Sicherungen](data-and-backups.md).

## Profile und mehrere Instanzen
- **Pro Datenordner läuft nur ein Paneldeck** (Instanzsperre). Ein zweiter Start mit demselben Ordner übergibt an das laufende (und holt dessen Fenster nach vorn).
- Profile und andere Datenordner laufen **nebeneinander**: Jedes hat eine eigene Instanz, eigene Einstellungen, Sitzungen und Datenbank.
- Integrationen folgen der Instanz: Autostart, Explorer-Kontextmenü und MCP-Registrierung einer Profil-Instanz tragen ihr `--profile`/`--data-dir` (siehe [Windows-Integration](windows-integration.md) und [Agenten-Schnittstelle](agent-interface.md#instanzen-mit-eigenem-datenordner-oder-profil)).
- Ein weiteres *Fenster* (Strg+Klick auf einen Reiter) ist keine neue Instanz: Es gehört zum selben Prozess und Datenordner (siehe [Mehrere Fenster](windows.md)).

## Was an eine laufende Instanz übergeben wird
Läuft Paneldeck schon mit demselben Datenordner, übergibt ein neuer Start nur **`--open`/`--claude`** (bzw. zeigt bei `--tray` und einem schlichten Start das Fenster). Die Optionen, die nur den Start einer Instanz betreffen – **`--lang`, `--backup-dir`, `--no-tray`, `--no-update-check`** – wirken dann nicht, und **`--reset-settings`** schlägt mit einer Fehlermeldung fehl. Vorher Paneldeck beenden.

## Exit-Codes
| Code | Bedeutung |
|---|---|
| `0` | ok (auch bei `--help` und `--version`) |
| `1` | Fehler, z. B. unbrauchbarer Datenordner, `--reset-settings` bei laufendem Paneldeck |
| `2` | ungültige Kommandozeile (unbekannte Option, fehlender Wert, ungültiger Profilname oder ungültige Sprache, überzähliges Argument) |

## Umgebungsvariablen
| Variable | Bedeutung |
|---|---|
| `PANELDECK_DATA` | fester Datenordner (Schritt 2 der Suche) |

Paneldeck setzt in seinen Terminals weitere Variablen (z. B. damit Hooks und MCP-Brücke ihren Hub finden); sie sind intern und können sich ändern.
