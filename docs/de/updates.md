[← README](../../README.de.md) · [English](../en/updates.md) · **Deutsch**

# Updates

Ab 2.1.0 kann sich Paneldeck selbst aus den [GitHub-Releases](https://github.com/zibbozz/paneldeck/releases) aktualisieren. Nichts wird selbst gehostet: Prüfung und Download gehen beide an GitHub. Angeboten werden nur reguläre Releases (keine Pre-Releases), und Paneldeck stuft nie auf eine ältere Version herab.

## Nach Updates suchen
- *Einstellungen → Info → Updates* zeigt die installierte Version und ob diese Kopie *(installiert)* oder *(portabel)* ist, den Knopf **Nach Updates suchen**, den Zeitpunkt der letzten Prüfung und eine übersprungene Version (mit *Zurücksetzen*).
- **Automatisch prüfen (beim Start, höchstens einmal am Tag)** – **standardmäßig aus**; es gibt keine Frage beim ersten Start. Nach einer gescheiterten automatischen Prüfung (z. B. offline) versucht Paneldeck es frühestens eine Stunde später erneut.
- `--no-update-check` unterdrückt die automatische Prüfung für einen Start (siehe [Kommandozeile](command-line.md#--no-update-check)).
- Agenten (MCP) können kein Update auslösen.

## Wenn ein Update verfügbar ist
In der Fensterleiste erscheint rechts, direkt links vom Minimieren-Knopf, ein Knopf (Tooltip „Version X ist verfügbar“; beim Herunterladen zeigt er den Fortschritt). Ein Klick öffnet einen Dialog mit:
- der neuen und der installierten Version,
- **Was ist neu?** – öffnet die Release-Seite auf GitHub im Browser,
- **Jetzt aktualisieren**, **Später** (blendet Dialog und Knopf aus; *Details …* unter *Einstellungen → Info* öffnet ihn wieder) und **Diese Version überspringen** (die automatische Prüfung bietet sie nicht mehr an; eine Prüfung von Hand zeigt sie weiter, mit „(übersprungen)“).

Ein Sprung auf eine **neue Hauptversion** (z. B. 2.x → 3.0) zeigt eine deutliche Warnung, und vor dem Update wird automatisch eine Datenbank-Sicherung angelegt.

## Installieren
1. **Herunterladen** im Hintergrund, danach Prüfung gegen `SHA256SUMS.txt` (siehe unten). Der Download lässt sich abbrechen.
2. **Neustart bestätigen:** Für das Update startet Paneldeck neu. Laufen Terminals oder Claude-Sitzungen, nennt es ihre Anzahl; ihre Prozesse werden beendet, die Sitzungen nach dem Neustart wiederhergestellt. *Nicht jetzt* lässt einen Knopf *Neu starten* in der Fensterleiste stehen.
3. **Installieren:**
   - **Installierte Kopien** laden `Paneldeck-Setup-<Version>.exe` und führen es still in denselben Ordner aus. Deine Wahl bei Desktop-Verknüpfung und Kontextmenü bleibt erhalten.
   - **Portable Kopien** laden `paneldeck-<Version>-windows-x64.zip`, ersetzen `paneldeck.exe` (die alte bleibt als `paneldeck.exe.old` liegen und wird gelöscht, nachdem die neue Version gestartet ist) und starten neu.
4. Die neue Version startet mit **denselben Parametern** (Datenordner, Profil) und stellt deine Sitzungen wieder her.

Als *installiert* gilt eine Kopie, wenn der Deinstallations-Eintrag des Installers genau auf ihren Ordner zeigt; jede andere Kopie (auch eine portable neben einer Installation) ist *portabel*.

## Integrität
- Jedes Release ab 2.1.0 hat eine zusätzliche Datei **`SHA256SUMS.txt`** mit den SHA-256-Prüfsummen von Installer und ZIP (Format von `sha256sum`). Der Download wird dagegen geprüft und **bei Abweichung verworfen** („Prüfsumme stimmt nicht – der Download wurde verworfen.“) und direkt vor der Installation noch einmal geprüft.
- Releases **ohne** `SHA256SUMS.txt` (2.0.0 und älter) werden nur angekündigt, nie automatisch installiert.
- Downloads gehen nur über HTTPS an GitHub-Hosts (`api.github.com`, `github.com` und die Download-Server von GitHub) und nur für dieses Repository.
- Ohne Code-Signatur (siehe [Richtlinie zur Code-Signatur](../../README.de.md#richtlinie-zur-code-signatur-code-signing-policy)) schützt das vor beschädigten oder unvollständigen Downloads, **nicht** vor einem kompromittierten GitHub-Konto.

Eine von Hand heruntergeladene Datei kannst du selbst prüfen, z. B. in PowerShell: `Get-FileHash .\Paneldeck-Setup-2.1.0.exe` und mit der Zeile in `SHA256SUMS.txt` vergleichen; in Git Bash: `sha256sum -c SHA256SUMS.txt`.

## Datenschutz
Die Prüfung fragt die öffentliche GitHub-API nach dem neuesten Release – ohne Anmeldung und nur, wenn du auf *Nach Updates suchen* klickst oder die automatische Prüfung eingeschaltet hast. GitHub sieht dabei deine IP-Adresse und die Paneldeck-Version (`User-Agent: Paneldeck/<Version>`); weitere Daten werden nicht gesendet. Siehe [Datenschutz](../../README.de.md#datenschutz-privacy).

## Von Hand aktualisieren
Du kannst jederzeit ohne das eingebaute Update aktualisieren: den neuen Installer über die alte Version ausführen (auch während Paneldeck läuft) oder `paneldeck.exe` einer portablen Kopie ersetzen. Einstellungen, Sitzungen und Daten bleiben. Das Selbst-Update funktioniert erst beim Update **von** 2.1.0 auf eine spätere Version.
