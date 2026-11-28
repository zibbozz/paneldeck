[← README](../../README.de.md) · [English](../en/macos.md) · **Deutsch**

# macOS (experimentell)

Ab 2.5.0 läuft Paneldeck unter macOS 12 (Monterey) oder neuer, auf Macs mit Apple Silicon und Intel (eine Universal-App). **Die macOS-Version ist experimentell:** Sie wird von GitHub Actions unter macOS automatisch gebaut und getestet, aber noch nicht täglich auf einem echten Mac benutzt. Bitte alles, was nicht funktioniert, als [Issue](https://github.com/zibbozz/paneldeck/issues) melden. Die Seite *Info* in den Einstellungen weist ebenfalls darauf hin.

## Installieren
Jedes Release auf [GitHub Releases](https://github.com/zibbozz/paneldeck/releases) hat zwei macOS-Dateien, von GitHub Actions aus dem getaggten Quelltext gebaut, dazu `SHA256SUMS.txt`:

| Datei | Inhalt |
|---|---|
| `Paneldeck-<version>-macos-universal.dmg` | Disk-Image: `Paneldeck.app` auf *Programme* (Applications) ziehen |
| `paneldeck-<version>-macos-universal.zip` | `Paneldeck.app` als ZIP (entpacken, nach *Programme* verschieben) – auch das, was das Selbst-Update lädt |

Einen Download vor dem Öffnen selbst prüfen: `shasum -a 256 -c --ignore-missing SHA256SUMS.txt`.

### Die App ist nicht signiert – das erste Öffnen
Paneldeck hat keine Apple-Developer-ID (die kostet Geld; das Projekt bleibt kostenlos). Die App ist nur *ad hoc* signiert, deshalb blockiert Gatekeeper den ersten Start einer heruntergeladenen Kopie („kann nicht geöffnet werden, da Apple sie nicht auf Schadsoftware überprüfen kann“). Folgendes nur für eine Datei von der Release-Seite tun, deren Prüfsumme passt:
- **macOS 12–14:** in *Programme* Rechtsklick (oder Ctrl+Klick) auf `Paneldeck.app` → **Öffnen** → **Öffnen**. macOS merkt sich die Entscheidung.
- **macOS 15 (Sequoia) und neuer:** die App doppelklicken, die Meldung schließen, dann *Systemeinstellungen → Datenschutz & Sicherheit*, nach unten zu „Paneldeck wurde blockiert …“ → **Dennoch öffnen** und bestätigen.
- **Terminal (alle Versionen):** `xattr -d com.apple.quarantine /Applications/Paneldeck.app` entfernt die Download-Markierung; danach öffnet sich die App normal.

Aus *Programme* starten, nicht aus dem Disk-Image: Eine aus dem Disk-Image (oder aus einem Ordner unter Quarantäne, „App Translocation“) gestartete App läuft von einem schreibgeschützten Ort und kann sich nicht selbst aktualisieren.

## Datenordner
`~/Library/Application Support/Paneldeck`, nur für dich lesbar (Ordner `0700`, Dateien `0600`). **Portabler Modus:** Ein Ordner `paneldeck-data` neben dem Programm im Inneren der App ist unter macOS nicht praktikabel; stattdessen `--data-dir` oder `PANELDECK_DATA` nehmen. `--data-dir`, `PANELDECK_DATA` und `--profile` funktionieren wie überall (siehe [Kommandozeile](command-line.md)); das Programm selbst ist `/Applications/Paneldeck.app/Contents/MacOS/paneldeck`.

## Shells
Die Terminals nutzen die Shells aus `/etc/shells`; Standard ist deine Anmelde-Shell (`$SHELL`, normalerweise **zsh**). Sie starten als Login-Shell (`-l`), `PATH` und Profil sind also wie in Terminal.app eingerichtet und `claude` wird gefunden. zsh und fish bekommen die Shell-Integration (Befehlsmarken, Exit-Codes, Ordner-Verfolgung, exaktes `run_command`), ohne dass deine `.zshrc` geändert wird – siehe [Terminals](terminals.md).

## Desktop-Integration
Alle Schalter stehen unter *Einstellungen → Allgemein → Desktop-Integration*.
- **Menüleiste:** Statt eines Tray-Symbols zeigt Paneldeck ein Symbol in der Menüleiste (Klick = Fenster zeigen, Menü: *Paneldeck anzeigen* / *Beenden*). *Schließen → Tray* und *Minimieren → Tray* nutzen es.
- **Bei der Anmeldung starten** (standardmäßig aus): ein LaunchAgent `~/Library/LaunchAgents/io.github.zibbozz.paneldeck.plist` (Instanzen mit `--profile`/`--data-dir` bekommen eine eigene Datei), der Paneldeck bei der nächsten Anmeldung minimiert startet. macOS meldet „Hintergrundobjekt hinzugefügt“ und führt ihn unter *Systemeinstellungen → Allgemein → Anmeldeobjekte*. Ausschalten entfernt die Datei wieder (eine Datei ohne die Paneldeck-Markierung wird nie angefasst). Solange Paneldeck aus dem Disk-Image oder einem verschobenen Ort (App Translocation) läuft, lässt er sich nicht einschalten – die App zuerst nach *Programme* ziehen.
- **Benachrichtigungen** (standardmäßig an) laufen über `osascript`. Weil die App nicht signiert ist, erscheinen sie unter dem Namen **„Skript-Editor“**; erlauben (oder abschalten) unter *Systemeinstellungen → Mitteilungen → Skript-Editor*.
- **Dateidialoge** (Export, Import, Ordner wählen) sind die von macOS.
- **Finder: „In Paneldeck öffnen“** richtet Paneldeck in 2.5.0 nicht selbst ein. Als Schnellaktion lässt es sich in zwei Minuten anlegen:
  1. *Automator* öffnen → *Neues Dokument* → **Schnellaktion**.
  2. Oben: *Arbeitsablauf empfängt aktuelle* **Ordner** in **Finder**.
  3. Die Aktion **Shell-Skript ausführen** hinzufügen, *Eingabe übergeben:* **als Argumente**, und eintragen:
     ```sh
     for f in "$@"; do /Applications/Paneldeck.app/Contents/MacOS/paneldeck --open "$f"; done
     ```
     (für „Mit Claude starten“ stattdessen `--claude --open`).
  4. Als „In Paneldeck öffnen“ sichern. Es erscheint per Rechtsklick auf einen Ordner unter *Schnellaktionen* (bzw. *Dienste*). Ein laufendes Paneldeck öffnet den Ordner als neue Sitzung.

## API-Schlüssel von Chat-Servern
API-Schlüssel werden nie im Klartext gespeichert. Paneldeck legt einen zufälligen Schlüssel in deinem **Anmelde-Schlüsselbund** ab (Objekt „Paneldeck“, Account „api-key-encryption“) und speichert die API-Schlüssel damit verschlüsselt in `settings.json`. Weil die App nicht signiert ist, fragt macOS evtl. einmal, ob Paneldeck dieses Objekt verwenden darf – *Immer erlauben* wählen. Nach einem Update kann die Frage wiederkommen (die Signatur der neuen Version ist eine andere). Gibt es keinen nutzbaren Schlüsselbund, fragt Paneldeck beim ersten Zugriff auf einen Schlüssel (nie beim Start) nach einem **Master-Passwort**, wie unter Linux.

## Selbst-Update
Die App aktualisiert sich selbst: Das neue `paneldeck-<version>-macos-universal.zip` wird von GitHub geladen, **gegen `SHA256SUMS.txt` geprüft**, neben der App entpackt und an die Stelle der alten `Paneldeck.app` gesetzt (die alte bleibt versteckt als `.Paneldeck.app.old` liegen, bis die neue Version gestartet ist), danach startet Paneldeck neu. Dafür braucht es Schreibrechte im Ordner der App (für einen Admin-Benutzer in `/Applications` normal); sonst meldet Paneldeck die neue Version nur mit Link. Vor dem Tausch wird die entpackte App mit `codesign --verify --deep --strict` geprüft; ein beschädigtes Bündel wird verworfen.

Ehrlich gesagt: Die Prüfsumme schützt vor **Übertragungsfehlern und unvollständigen Downloads, nicht vor einem manipulierten Release** – eine eigene Signatur gibt es nicht. Die vom Selbst-Update geschriebene App hat **kein Quarantäne-Attribut**, Gatekeeper prüft Updates also gar nicht. Vertrauen entsteht allein über den Abgleich mit `SHA256SUMS.txt` des GitHub-Releases.

## Agenten-Schnittstelle (MCP)
Funktioniert wie unter Windows und Linux. Der Hub ermittelt den aufrufenden Prozess selbst (Besitzer der TCP-Verbindung über `libproc` und sein Prozessbaum). Ein nicht zuordenbarer Aufrufer wird **abgewiesen**. Zu einer Sitzung gehört ein Aufrufer nur, wenn er ihre Shell oder einer ihrer Nachfahren ist. macOS hat keinen „Subreaper“: Ein Programm, das sich nach dem Start des Hubs von einer Sitzung löst (seine Elternkette endet bei `launchd`), gilt als „gehört zu keiner nachweisbaren Sitzung“ und wird wie unter Windows eingeschränkt. Wie unter Windows und Linux ist das keine Grenze gegen feindliche Programme desselben Benutzers, siehe [Agenten-Schnittstelle](agent-interface.md).

Bekannte Umwege (wie WMI oder die Aufgabenplanung unter Windows): Eine Sitzung kann ein **schon laufendes** Programm etwas für sich starten lassen. Dieses Programm ist dann kein Nachfahre der Sitzung, und der Hub zählt es als externen Aufrufer. Beispiele:
- `open -a Terminal x.command` oder `open x.command`: Terminal.app führt das Skript in einem neuen Fenster aus;
- `osascript -e 'tell application "Terminal" to do script "…"'` (ebenso für iTerm2): ein Befehl, getippt in ein schon offenes Terminalfenster;
- ein vorher oder außerhalb der Sitzung gestarteter `tmux`-/`screen`-Server: `tmux new-window …`, `tmux send-keys …`;
- `launchctl`-Jobs oder Anmeldeobjekte.

Die Ketten-Regeln schützen vor gelenkten Agenten (Prompt Injection), nicht vor feindlichen lokalen Programmen. Ein Agent mit Stufe *Steuern*, der Shell-Befehle ausführen kann, kann diese Umwege nutzen. Wem das wichtig ist, lässt die Stufe auf *Aus* oder *Lesen*.

## Grenzen (2.5.0)
- **Experimentell**, nicht signiert, nicht notarisiert.
- Fenster: Paneldeck zeichnet seine eigene Titelleiste mit den Fensterknöpfen rechts (wie unter Windows), nicht die macOS-„Ampel“.
- Tastatur: App-Kürzel nutzen weiterhin **Ctrl** (wie unter Windows/Linux), nicht Cmd.
- Finder-Integration nur von Hand (siehe oben).
- Ein Klick auf einen Pfad im Terminal startet nie ein Programm: Apps (`.app`), `.command`/`.tool`/`.terminal`, AppleScripts, Installer (`.pkg`), `.webloc`/`.inetloc`/`.fileloc`, `.prefPane`, Shell-Skripte und jede Datei mit Ausführungsrecht werden nur im Finder gezeigt.
- Benachrichtigungen erscheinen als „Skript-Editor“.
- Installierte Festbreitenschriften werden aus `/System/Library/Fonts`, `/Library/Fonts` und `~/Library/Fonts` gelesen.
