[← README](../../README.de.md) · [English](../en/updates.md) · **Deutsch**

# Updates

Ab 2.1.0 kann sich Paneldeck selbst aus den [GitHub-Releases](https://github.com/zibbozz/paneldeck/releases) aktualisieren. Nichts wird selbst gehostet: Prüfung und Download gehen beide an GitHub. Angeboten werden nur reguläre Releases (keine Pre-Releases), und Paneldeck stuft nie auf eine ältere Version herab.

## Nach Updates suchen
- *Einstellungen → Info → Updates* zeigt die installierte Version und ob diese Kopie *(installiert)* oder *(portabel)* ist, den Knopf **Nach Updates suchen**, den Zeitpunkt der letzten Prüfung und eine übersprungene Version (mit *Zurücksetzen*).
- **Automatisch prüfen (beim Start, höchstens einmal am Tag)** – **standardmäßig aus**; es gibt keine Frage beim ersten Start. Nach einer gescheiterten automatischen Prüfung (z. B. offline) versucht Paneldeck es frühestens eine Stunde später erneut.
- `--no-update-check` unterdrückt die automatische Prüfung für einen Start (siehe [Kommandozeile](command-line.md#--no-update-check)).
- Agenten (MCP) können kein Update auslösen.

## Wenn ein Update verfügbar ist
In der Fensterleiste erscheint rechts, direkt links vom Minimieren-Knopf, ein Knopf. Er zeigt die neue Version (Tooltip „Version X ist verfügbar“), beim Herunterladen den Fortschritt in Prozent und **Neu starten**, sobald das Update bereit ist. Ein Klick öffnet einen Dialog mit:
- der neuen und der installierten Version,
- **Was ist neu? (Release-Seite auf GitHub)** – öffnet die Release-Seite im Browser,
- **Jetzt aktualisieren**, **Später** und **Diese Version überspringen** (die automatische Prüfung bietet sie nicht mehr an; eine Prüfung von Hand zeigt sie weiter, mit „(übersprungen)“; *Einstellungen → Info* zeigt die übersprungene Version mit *Zurücksetzen*).

**Später** blendet Dialog und Knopf aus; *Details …* unter *Einstellungen → Info* öffnet ihn wieder. Ein Klick auf *Später* während des Downloads lässt den Download im Hintergrund zu Ende laufen; danach bleibt nur der Knopf **Neu starten** in der Fensterleiste, ohne Nachfrage.

Eine automatische Prüfung (etwa 3 Sekunden nach dem Start) zeigt nur den Knopf; eine Prüfung von Hand öffnet zusätzlich den Dialog.

Ein Sprung auf eine **neue Hauptversion** (z. B. 2.x → 3.0) zeigt eine deutliche Warnung, und vor dem Update wird automatisch eine Datenbank-Sicherung angelegt.

## Installieren
1. **Herunterladen** im Hintergrund (*Download abbrechen* hält ihn an) in einen neuen Ordner unter `%LOCALAPPDATA%\Paneldeck\updates`, danach Prüfung gegen `SHA256SUMS.txt` (siehe unten). Ein ZIP wird zusätzlich probehalber entpackt, damit ein kaputtes Archiv vor der Neustart-Frage auffällt. Download-Ordner beendeter Instanzen räumt der nächste Start auf.
2. **Neustart bestätigen:** Paneldeck startet nie ohne Nachfrage neu. Ist der Download fertig, fragt es *Jetzt neu starten, um zu aktualisieren?* – auch wenn kein Terminal läuft (nicht gespeicherte Eingaben in offenen Dialogen gingen verloren). Laufen Terminals oder Claude-Sitzungen, nennt es ihre Anzahl; ihre Prozesse werden beendet, die Sitzungen nach dem Neustart wiederhergestellt. **Neu starten und aktualisieren** führt das Update aus; **Nicht jetzt** lässt den Knopf *Neu starten* in der Fensterleiste stehen (ein Klick darauf fragt erneut). Ein Klick auf *Neu starten und aktualisieren* im Update-Dialog gilt, wenn kein Terminal läuft, selbst als Bestätigung.
3. **Installieren:**
   - **Installierte Kopien** laden `Paneldeck-Setup-<Version>.exe` und führen es still in denselben Ordner aus. Deine Wahl bei Desktop-Verknüpfung und Kontextmenü bleibt erhalten.
   - **Portable Kopien** laden `paneldeck-<Version>-windows-x64.zip`, ersetzen `paneldeck.exe` (die alte bleibt als `paneldeck.exe.old` liegen und wird gelöscht, sobald die neue Version ihr erstes Bild gezeichnet hat) und starten neu. Startet eine neue Version gar nicht: Paneldeck beenden, `paneldeck.exe` löschen und `paneldeck.exe.old` wieder in `paneldeck.exe` umbenennen.
   - Seit 2.3 ersetzt ein Update `paneldeck.exe` und den [mitgelieferten Konsolen-Host](terminals.md#shells-und-emulation) (`conpty.dll`, `OpenConsole.exe`) gemeinsam, auch bei laufendem Paneldeck: Der Installer schreibt zuerst den Konsolen-Host und legt bei einem Schreibfehler die bisherige Datei zurück; das portable Update ersetzt alle Dateien oder keine. Übrig gebliebene `.old`-Dateien werden beim nächsten Start gelöscht.
4. Die neue Version startet mit **denselben Parametern** (Datenordner, Profil) und stellt deine Sitzungen wieder her.

Als *installiert* gilt eine Kopie, wenn der Deinstallations-Eintrag des Installers genau auf ihren Ordner zeigt; jede andere Kopie (auch eine portable neben einer Installation) ist *portabel*.

## Integrität
- Jedes Release ab 2.1.0 hat eine zusätzliche Datei **`SHA256SUMS.txt`** mit den SHA-256-Prüfsummen von Installer und ZIP (Format von `sha256sum`). Der Download wird dagegen geprüft und **bei Abweichung verworfen** („Prüfsumme stimmt nicht – der Download wurde verworfen.“) und direkt vor der Installation noch einmal geprüft.
- Releases **ohne** `SHA256SUMS.txt` (2.0.0 und älter) werden nur angekündigt, nie automatisch installiert.
- Downloads gehen nur über HTTPS an eine feste Liste von GitHub-Hosts (`api.github.com`, `github.com`, `objects.githubusercontent.com`, `release-assets.githubusercontent.com`; Weiterleitungen nur zwischen diesen, höchstens 5) und nur für dieses Repository. Eine Release-Datei darf höchstens 64 MiB groß sein.
- Der Sperrstatus des Zertifikats wird geprüft. Ist das Zertifikat widerrufen oder lässt sich die Sperrprüfung nicht durchführen (z. B. weil eine Firewall oder ein Proxy den Server für Sperrlisten blockiert), wird nichts geladen.
- Ohne Code-Signatur (siehe [Hinweis in der README](../../README.de.md#hinweis-programm-ist-noch-nicht-signiert)) schützt das vor beschädigten oder unvollständigen Downloads, **nicht** vor einem kompromittierten GitHub-Konto.
- **Linux (ab 2.5.0):** Nur die AppImage aktualisiert sich selbst (gleiche Prüfungen: `SHA256SUMS.txt`, GitHub-Hosts, Größengrenze; Zertifikate aus dem Zertifikatsspeicher des Systems). `.deb`, `.rpm` und Tarball melden eine neue Version nur. Siehe [Linux](linux.md#selbst-update-appimage).

Eine von Hand heruntergeladene Datei kannst du selbst prüfen, z. B. in PowerShell: `Get-FileHash .\Paneldeck-Setup-2.1.0.exe` und mit der Zeile in `SHA256SUMS.txt` vergleichen; in Git Bash: `sha256sum -c SHA256SUMS.txt`.

## Datenschutz
Die Prüfung fragt die öffentliche GitHub-API (`api.github.com`) nach dem neuesten Release – ohne Anmeldung und nur, wenn du auf *Nach Updates suchen* klickst oder die automatische Prüfung eingeschaltet hast; ein Download nutzt zusätzlich die Download-Server von GitHub. GitHub sieht dabei deine IP-Adresse und die Paneldeck-Version (`User-Agent: Paneldeck/<Version>`); weitere Daten werden nicht gesendet. Siehe [Datenschutz](../../README.de.md#datenschutz-privacy).

## Von Hand aktualisieren
Du kannst jederzeit ohne das eingebaute Update aktualisieren: den neuen Installer über die alte Version ausführen (auch während Paneldeck läuft) oder `paneldeck.exe` einer portablen Kopie ersetzen – ab 2.3 zusammen mit `conpty.dll` und `OpenConsole.exe` aus dem ZIP. Einstellungen, Sitzungen und Daten bleiben. Das Selbst-Update funktioniert erst beim Update **von** 2.1.0 auf eine spätere Version.

## Selbst-Update testen (Entwicklung)
Release-Builds sprechen nur mit GitHub. Für lokale Tests gibt es einen Test-Feed: mit dem Cargo-Feature `update-test-feed` bauen (`cargo build --release --features update-test-feed`), mit `tools/update-feed.sh <Version> <Feed-Ordner> [<paneldeck.exe>]` ein Schein-Release anlegen und die Testkopie mit der Umgebungsvariable `PANELDECK_UPDATE_TEST_FEED=<Feed-Ordner>` starten. Es wird nichts veröffentlicht; `tools/release.sh` prüft, dass Release-Builds diesen Weg nicht enthalten. Einzelheiten: `TESTPLAN.md`, Abschnitt 52.
