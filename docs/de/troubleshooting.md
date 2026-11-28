[← README](../../README.de.md) · [English](../en/troubleshooting.md) · **Deutsch**

# Fehlersuche

## Fehlerberichte
*Einstellungen → Daten & Sicherung → Diagnose → Diagnose kopieren* legt Systeminfos in die Zwischenablage (Version, Datenordner und seine Herkunft, Profil, aktive Startoptionen …) – bitte bei Fehlerberichten mitschicken, zusammen mit `crash.log` aus dem Datenordner, falls vorhanden.

## Windows SmartScreen
Die Releases sind noch nicht code-signiert (siehe [Hinweis in der README](../../README.de.md#hinweis-programm-ist-noch-nicht-signiert)). Beim ersten Start zeigt SmartScreen evtl. „Der Computer wurde durch Windows geschützt“: auf **„Weitere Informationen“ → „Trotzdem ausführen“** klicken.

## Fehlalarme von Virenscannern
Unsignierte Builds von Paneldeck werden ab und zu von Microsoft Defender oder anderen Virenscannern als Schadsoftware erkannt (bisher z. B. `Trojan:Win32/Bearfoos.A!ml` – eine Erkennung per maschinellem Lernen, die von Build zu Build schwankt). Paneldeck enthält keine Schadsoftware: Es ist Open Source, wird aus diesem Repository gebaut, ist nicht gepackt und geht nur für die [Update-Prüfung](updates.md) ins Netz, wenn du es verlangst.

Was du tun kannst:
- Nur von der Seite [GitHub Releases](https://github.com/zibbozz/paneldeck/releases) herunterladen und die Prüfsumme mit `SHA256SUMS.txt` vergleichen (siehe [Updates](updates.md#integrität)).
- Den Fehlalarm an Microsoft melden: https://www.microsoft.com/en-us/wdsi/filesubmission („Incorrectly detected as malware/malicious“).
- Wenn du der Datei vertraust: sie aus der Quarantäne wiederherstellen und in der Windows-Sicherheit eine Ausnahme für den Paneldeck-Ordner anlegen – nur für diesen Ordner.
- Oder Paneldeck selbst bauen (siehe [Bauen](building.md)).

Jedes Release wird vor der Veröffentlichung mit Microsoft Defender geprüft (`tools/defender-scan.ps1`).

## Start und Kommandozeile
- **„Unbekannte Option: …“** beim Start: Seit 2.1.0 sind unbekannte Optionen und überzählige Argumente ein Fehler (Exit-Code 2). Alte Verknüpfungen und Skripte prüfen; `paneldeck.exe --help` listet alle Optionen ([Kommandozeile](command-line.md)).
- **„Der Datenordner lässt sich nicht anlegen oder beschreiben: …“**: Der mit `--data-dir` oder `PANELDECK_DATA` angegebene Ordner (oder der Profilordner) lässt sich nicht anlegen oder beschreiben. Paneldeck weicht in diesem Fall nicht auf einen anderen Ordner aus (Exit-Code 1).
- **Eine Option wirkt nicht:** `--lang`, `--backup-dir`, `--no-tray` und `--no-update-check` gelten nur, wenn der Start die Instanz öffnet. Läuft Paneldeck mit diesem Datenordner schon, vorher beenden (siehe [Was übergeben wird](command-line.md#was-an-eine-laufende-instanz-übergeben-wird)). `--reset-settings` schlägt fehl, solange es läuft.
- **Falsche Daten:** *Einstellungen → Info → Version* zeigt, welcher Datenordner aktiv ist und warum; vielleicht wirkt ein Profil oder `PANELDECK_DATA`.

## Datenbank
- „Die Datenbank hat die Schema-Version …“ beim Start: Der Datenordner wurde von einem neueren Paneldeck benutzt. Paneldeck aktualisieren (oder einen anderen Datenordner nehmen); die Datenbank bleibt unverändert. Das passiert auch, wenn du von 2.3 zu 2.2 zurückgehst (Schema-Version 9) – siehe [Zurück zu 2.2](data-and-backups.md#neue-felder-in-230).
- Eine beschädigte Datenbank lässt sich über *Einstellungen → Daten & Sicherung → Wiederherstellen …* ersetzen oder indem man bei geschlossenem Paneldeck eine Datei aus `backups/` über `paneldeck.db` kopiert (siehe [Datenordner und Sicherungen](data-and-backups.md)).

## Claude-Modus
- Status/Kosten erscheinen nicht? Prüfen, ob `claude` installiert ist und ob in *Einstellungen → Claude → Hooks* Hooks aktiv sind. `--safe-mode` (*Weitere Claude-Optionen*) schaltet die Hooks ab, Paneldeck sieht den Status einer solchen Sitzung dann nicht.
- Eine Startoption fehlt in einer Sitzung, die ein Agent gestartet hat: Zusätzliche Ordner, erlaubte Werkzeuge, eine MCP-Konfiguration und `--safe-mode` aus deinen Standards gelten nie für Sitzungen, die Agenten anlegen (siehe [Claude Code](claude-code.md#startoptionen)).

## Agenten-Schnittstelle
- `mcp_disabled`: unter *Einstellungen → Agenten → Zugriff* *Nur lesen* oder *Steuern* einschalten.
- `hub_not_running`: Für den Datenordner, in dem die Brücke gesucht hat, läuft kein Paneldeck (die Meldung nennt ihn). Bei Profilen den eigenen Servernamen der Instanz registrieren (`paneldeck-<profil>`) oder `--profile` an `mcp` übergeben (siehe [Agenten-Schnittstelle](agent-interface.md)).
- Nach dem Umschalten des Expertenmodus die Werkzeugliste des Agenten neu laden (`/mcp` in Claude Code).

## Chats mit lokalen Modellen
- Server nicht erreichbar, Modell nicht gefunden, Kontext zu klein, ein Modell nutzt keine Werkzeuge: siehe [Chats – Fehlersuche](chat.md#fehlersuche).

## Updates
- „Prüfsumme stimmt nicht – der Download wurde verworfen.“: Die Datei passte nicht zu `SHA256SUMS.txt`; später erneut versuchen oder von Hand herunterladen.
- „Diese Version lässt sich nicht automatisch installieren“: Das Release hat keine Prüfsummen-Datei `SHA256SUMS.txt` (2.0.0 und älter) oder keine passende Datei – von der Release-Seite herunterladen und von Hand aktualisieren.
- Nach einem Update wird `paneldeck.exe.old` neben einer portablen `paneldeck.exe` gelöscht, sobald die neue Version gestartet ist (ihr erstes Bild gezeichnet hat). Startet die neue Version gar nicht: Paneldeck beenden, `paneldeck.exe` löschen und `paneldeck.exe.old` wieder in `paneldeck.exe` umbenennen.
- „Die Sperrprüfung des Zertifikats war nicht möglich …“: Eine Firewall, ein Proxy oder ein Virenscanner blockiert den Server für Sperrlisten. Aus Sicherheitsgründen wird nichts geladen; später erneut versuchen oder Firewall/Proxy prüfen – oder von Hand aktualisieren.

## Anzeige
- Sonderzeichen fehlen? Windows-Systemschriften werden als Fallback genutzt (Segoe UI Symbol, Microsoft YaHei, Yu Gothic, Malgun Gothic …). Für Powerline- oder Nerd-Font-Symbole eine solche Schrift unter *Einstellungen → Darstellung → Schrift & Skalierung → Terminalschrift* wählen.

## Terminals
- **Eine Sitzung startet nicht, oder ein Hinweis meldet, dass der mitgelieferte Konsolen-Host fehlschlug:** *Einstellungen → Terminal → Verhalten → Mitgelieferten Konsolen-Host verwenden (empfohlen)* ausschalten; neue Sitzungen nutzen dann den Konsolen-Host von Windows. Paneldeck weicht auch von selbst darauf aus, wenn `conpty.dll` oder `OpenConsole.exe` fehlen oder fehlschlagen.
- **Umschalt+Enter führt in PowerShell den Befehl aus:** *Einstellungen → Terminal → Verhalten → Umschalt+Enter = neue Zeile in PowerShell* einschalten.
- **Strg+Rück löscht in bash nur ein Zeichen:** Es sendet `0x08` wie Windows Terminal; in PowerShell löscht es das vorige Wort.
- **„Mehrere Zeilen einfügen?“** erscheint, wenn das Programm kein Bracketed Paste nutzt; die Shell könnte sonst jede Zeile sofort ausführen. Abschalten unter *Einstellungen → Terminal → Zwischenablage & Einfügen → Vor dem Einfügen mehrerer Zeilen nachfragen*.
- **Ein Link öffnet sich nicht:** Links öffnen mit **Strg+Klick**; manche von Programmen gesetzte Links fragen erst nach, manche werden aus Sicherheitsgründen nie geöffnet (siehe [Links, Zwischenablage und Einfügen](terminals.md#links-zwischenablage-und-einfügen)).
- **Beim Schließen fragt Paneldeck nach Programmen, die weiterlaufen:** Das sind Hintergrundprogramme, die aus der Sitzung gestartet wurden (z. B. `code .`, ein Browser). *Weiterlaufen lassen* lässt sie offen; *Alle beenden* beendet sie sofort, ohne zu speichern (siehe [Sitzungen schließen](terminals.md#sitzungen-schließen-und-hintergrundprogramme)).
