<h1 align="center"><img src="docs/images/de/banner.png" alt="Paneldeck" width="100%"></h1>

<p align="center"><a href="README.md">English</a> · <strong>Deutsch</strong></p>

Zentrale für viele Terminals – gebaut für Claude Code, nutzbar als allgemeiner Terminal-Manager.
Für Windows 10 (1809+) / 11 (x64): wahlweise als Installer (pro Benutzer, ohne Administratorrechte) oder als portables ZIP mit einer einzelnen `paneldeck.exe`.
Paneldeck arbeitet **offline**: Es meldet sich nur bei GitHub, wenn du selbst nach Updates suchst oder die automatische Update-Prüfung einschaltest, und Chats sprechen nur mit den lokalen Modell-Servern, die du einrichtest (siehe [Datenschutz](#datenschutz-privacy)).

> Unabhängiges Open-Source-Projekt – nicht mit Anthropic verbunden und nicht von Anthropic unterstützt oder geprüft. „Claude“ und „Claude Code“ sind Marken von Anthropic, PBC. Details unter [Rechtliches](#rechtliches).

<p align="center"><img src="docs/images/de/app.png" alt="Paneldeck mit geteilten Terminals, Freigabe-Anfrage und Seitenleiste" width="100%"><br><sub>Mehrere Sitzungen in Projekten und Ordnern, geteilte Bereiche, „Wartet auf dich“-Posteingang – alles in einem Fenster.</sub></p>

## Funktionen
- **Terminals** mit voller Emulation (ConPTY): PowerShell 7, Windows PowerShell, CMD, Git Bash, WSL-Distributionen – geordnet in Projekten, mit frei teilbaren Bereichen, mehreren Fenstern und einer Befehls-Palette. Moderne Tastaturprotokolle (kitty, win32-input-mode), Links aus Programmen (OSC 8, Strg+Klick), Fortschritt und Benachrichtigungen von Programmen, jede installierte Festbreitenschrift (z. B. Nerd Fonts), WSL-Sitzungen, die in Linux-Ordnern starten, und ein mitgelieferter, neuerer Konsolen-Host von Microsoft.
- **Hintergrundprogramme:** Die Shell jedes Terminals läuft in einem eigenen Windows-Job-Object; Programme, die aus einer Sitzung gestartet werden, gehören dazu, auch losgelöst gestartet. Das gilt auch für langlebige Programme wie `code .`, einen Browser oder `wt`. Laufen solche Programme noch, wenn du eine Sitzung schließt oder neu lädst oder Paneldeck beendest, listet Paneldeck sie auf und fragt, ob sie weiterlaufen oder alle beendet werden sollen. „Alle beenden“ beendet sie sofort, ohne zu speichern. Programme in einer Sitzung können ihren Job nicht verlassen (`CREATE_BREAKAWAY_FROM_JOB` schlägt fehl); das ist gewollt.
- **Claude-Modus** (abschaltbar): Statusanzeige per Hooks (arbeitet / Freigabe? / bereit), Benachrichtigungen, Token- und Kostenschätzung, Verlauf mit exaktem Fortsetzen, Startoptionen (Modell, Effort, Permission-Mode, Agent und mehr), Neu laden und Einstellungen je Sitzung.
- **Todo-Liste und Tickets:** eine einfache Todo-Liste, die im *Expertenmodus* zu einem kleinen Jira wird – Projekte, Tickettypen, Workflows, Boards, eine Abfragesprache, Sprints und Dashboards, alles lokal gespeichert. Jedes Ticket hat eine Beschreibung, eine Fälligkeit mit einstellbaren Markierungen und Labels; Icons aus dem Lucide-Satz und eine bearbeitbare Farbpalette.
- **Chats mit lokalen Modellen:** mit Ollama, LM Studio, llama.cpp oder vLLM auf diesem Rechner oder im eigenen Netz sprechen, direkt neben deinen Terminals – das Modell kann Paneldecks Werkzeuge nutzen, in Grenzen, die du je Modell festlegst.
- **Agenten-Schnittstelle (MCP):** KI-Agenten können Sitzungen sehen, Terminals lesen, Befehle ausführen und mit deiner Todo-Liste und deinen Tickets arbeiten – standardmäßig aus, mit einstellbarem Systemprompt.
- **Kommandozeile und Profile:** getrennte Datenordner und Profile, die nebeneinander laufen, jedes mit eigenem Autostart- und MCP-Eintrag.
- **Selbst-Update** aus den GitHub-Releases auf Wunsch, geprüft gegen `SHA256SUMS.txt`.
- **Englisch und Deutsch** (Standard: Englisch).

<p align="center"><img src="docs/images/de/todo-dashboard.png" alt="Ein Dashboard mit Burndown-Diagramm, Sprint-Fortschritt und Diagrammen" width="100%"><br><sub>Expertenmodus: ein Dashboard mit Burndown-Diagramm, Sprint-Fortschritt, Tickets nach Status und Priorität und erfasster Zeit.</sub></p>

<p align="center"><img src="docs/images/de/kosten.png" alt="Kosten-Übersicht" width="85%"><br><sub>Kosten-Übersicht: heute, 7 Tage, Gesamt, Tagesbudget und teuerste Sitzungen.</sub></p>

## Download
Paneldeck gibt es nur über **GitHub Releases**: https://github.com/zibbozz/paneldeck/releases – je Release ein Installer (`Paneldeck-Setup-<Version>.exe`), ein portables ZIP (`paneldeck-<Version>-windows-x64.zip`) und ab 2.1.0 `SHA256SUMS.txt` mit den Prüfsummen beider Dateien, alles aus diesem Repository gebaut.

**Linux** (ab 2.5.0, x86_64; Ubuntu 22.04 / Debian 12 / Fedora 39 oder neuer): eine AppImage (aktualisiert sich selbst), ein `.deb`, ein `.rpm` und ein Tarball, gebaut von GitHub Actions; ihre Prüfsummen stehen in derselben `SHA256SUMS.txt`. Installation, Datenordner und Desktop-Integration: [Linux](docs/de/linux.md).

**macOS** (ab 2.5.0, **experimentell**, macOS 12 oder neuer, Apple Silicon und Intel): eine Universal-`Paneldeck.app` als `.dmg` und `.zip` (die App aktualisiert sich selbst), gebaut von GitHub Actions; Prüfsummen in derselben `SHA256SUMS.txt`. Die App ist **nicht signiert** – der erste Start braucht Rechtsklick → *Öffnen* (macOS 15: *Systemeinstellungen → Datenschutz & Sicherheit → Dennoch öffnen*) oder `xattr -d com.apple.quarantine /Applications/Paneldeck.app`. Details: [macOS](docs/de/macos.md).

### Hinweis: Programm ist (noch) nicht signiert
Installer und `paneldeck.exe` sind **nicht code-signiert**. Ein Zertifikat kostet Geld, und Paneldeck soll ein kostenloses Hobbyprojekt bleiben. Deshalb gilt bis auf Weiteres:
- Beim ersten Start zeigt **Windows SmartScreen** evtl. „Der Computer wurde durch Windows geschützt“. Auf **„Weitere Informationen“ → „Trotzdem ausführen“** klicken.
- Manche Virenscanner (auch Microsoft Defender) schlagen bei neuen, unsignierten Programmen mit Tray-Symbol und Autostart gelegentlich fälschlich an. Der Quelltext ist offen einsehbar, und du kannst Paneldeck selbst bauen (siehe [Bauen](docs/de/building.md)); siehe auch [Fehlersuche](docs/de/troubleshooting.md#fehlalarme-von-virenscannern).

## Erste Schritte
1. **Installer** `Paneldeck-Setup-<Version>.exe` ausführen – oder das **portable ZIP** entpacken und `paneldeck.exe` starten.
2. Paneldeck startet mit einem leeren Terminal Hub: **Strg+Umschalt+T** öffnet eine neue Sitzung, **Strg+Umschalt+P** die Befehls-Palette, **Strg+Umschalt+D/E** teilt einen Bereich.
3. Für Claude: Button mit dem Funken-Symbol, Ordner wählen, fertig.
4. **Strg+2** (oder der Reiter *Todo-Liste* in der Fensterleiste) öffnet die Todo-Liste, **Strg+1** führt zurück zum Terminal Hub. **Strg+Klick** auf einen Reiter öffnet ihn in einem neuen Fenster.

Alle Kürzel lassen sich unter *Einstellungen → Tastenkürzel* ändern.

## Dokumentation
Die Anleitungen liegen in [`docs/de/`](docs/de/) (Englisch: [`docs/en/`](docs/en/)) und werden mit dem Code versioniert:

- [Terminals und Sitzungen](docs/de/terminals.md) – Sitzungen, Projekte, Teilen, Palette, Tastenkürzel
- [Mehrere Fenster](docs/de/windows.md) – Strg+Klick auf einen Reiter, der leere Terminal Hub
- [Claude Code](docs/de/claude-code.md) – Claude-Modus, Status, Kosten, Verlauf
- [Todo-Liste und Tickets](docs/de/todo-and-tickets.md) – einfacher und Expertenmodus, Boards, Sprints, Dashboards, Schnellaktionen
- [Abfragesprache](docs/de/query-language.md) – Tickets suchen, Beispiele, Referenz
- [Chats mit lokalen Modellen](docs/de/chat.md) – Ollama und OpenAI-kompatible Server, Modelle, Werkzeuge und Freigaben, Sicherheit
- [Agenten-Schnittstelle (MCP)](docs/de/agent-interface.md) – Stufen, Werkzeuge, Sicherheit, Systemprompt
- [Kommandozeile](docs/de/command-line.md) – alle Optionen, Suche des Datenordners, Profile, Exit-Codes
- [Datenordner und Sicherungen](docs/de/data-and-backups.md) – Dateien, Sicherungen, portabel oder installiert
- [Windows-Integration](docs/de/windows-integration.md) – Tray, Autostart, Explorer-Kontextmenü
- [Linux](docs/de/linux.md) – Pakete, Datenordner, Tray, Autostart, Dateimanager, API-Schlüssel, Selbst-Update der AppImage
- [macOS](docs/de/macos.md) (experimentell) – die unsignierte App installieren, Datenordner, Menüleiste, Autostart, Schlüsselbund, Selbst-Update
- [Updates](docs/de/updates.md) – Selbst-Update, Prüfsummen, Datenschutz
- [Fehlersuche](docs/de/troubleshooting.md) – häufige Probleme, Fehlalarme von Virenscannern
- [Bauen](docs/de/building.md) – aus dem Quelltext bauen, Release-Dateien

Den Changelog gibt es auf Deutsch ([`Changelog.de.md`](Changelog.de.md)) und Englisch ([`Changelog.md`](Changelog.md)); die Entwickler-Notizen (`CLAUDE.md`, `TESTPLAN.md`) sind auf Deutsch.

## Datenschutz (Privacy)
Dieses Programm überträgt keine Informationen an andere Systeme im Netzwerk, außer der Nutzer oder die Person, die es installiert oder bedient, verlangt das ausdrücklich. (This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.)

- Paneldeck sendet keine Telemetrie. Es liest lokal Claude-Transkripte (`~/.claude`) für Verlauf und Kostenschätzung; alles bleibt auf deinem Rechner, auch deine Todo-Liste und deine Tickets (lokale Datenbank `paneldeck.db`).
- **Update-Prüfung:** Paneldeck fragt GitHub nur dann nach dem neuesten Release, wenn du auf *Nach Updates suchen* klickst (*Einstellungen → Info*) oder dort die automatische Prüfung einschaltest (standardmäßig aus; beim Start, höchstens einmal am Tag). Die Anfrage geht an die öffentliche GitHub-API (`api.github.com`), Downloads kommen von `github.com` und den Download-Servern von GitHub – alles über HTTPS, ohne Anmeldung. GitHub sieht dabei deine IP-Adresse und die Paneldeck-Version (im `User-Agent`, z. B. `Paneldeck/2.1.0`); weitere Daten werden nicht gesendet. Siehe [Updates](docs/de/updates.md).
- Die Agenten-Schnittstelle (MCP) ist standardmäßig aus und nur vom eigenen Rechner aus erreichbar (`127.0.0.1`).
- **Chats mit lokalen Modellen** verbinden sich nur mit den Modell-Servern, die du selbst einrichtest, und nur auf diesem Rechner oder in privaten Netzen (siehe [Chats](docs/de/chat.md#datenschutz)).
- Links in Terminals und Tickets öffnen sich nur auf Klick im Browser.

## Issues und Mitarbeit
Paneldeck ist noch ein junges Projekt. Issues und Beiträge werden für alle geöffnet, sobald das Projekt einen Stand erreicht hat, mit dem ich zufrieden bin und bei dem ich mich wohlfühle, die Öffentlichkeit mitarbeiten zu lassen. Bis dahin bitte ich um Geduld – danke für das Verständnis.

## Rechtliches
- **Lizenz:** MIT (`LICENSE`). Die Software wird ohne jede Gewährleistung bereitgestellt; die Nutzung erfolgt auf eigene Gefahr – das gilt besonders für die Agenten-Schnittstelle, die KI-Agenten in deine Terminals schreiben lässt.
- **Keine Verbindung zu Anthropic:** Paneldeck ist ein unabhängiges Projekt. „Claude“ und „Claude Code“ sind Marken von Anthropic, PBC und werden hier nur genannt, um zu beschreiben, mit welchem Programm Paneldeck zusammenarbeitet. Paneldeck enthält keinen Code und keine Logos von Anthropic.
- **Datenschutz:** siehe [Datenschutz](#datenschutz-privacy).
- **Drittanbieter-Software:** Lizenzen und Hinweise stehen in `THIRD_PARTY.md` und `THIRD_PARTY_LICENSES.txt` (liegen Installer und ZIP bei). Die eingebetteten Schriften Cascadia Mono und Noto Color Emoji stehen unter der SIL Open Font License 1.1 (`assets/fonts/LICENSE-*.txt`), die eingebetteten Lucide-Icons unter der ISC-Lizenz (`assets/fonts/LICENSE-Lucide.txt`). Installer und ZIP enthalten den Konsolen-Host ConPTY von Microsoft (`conpty.dll`, `OpenConsole.exe`, unverändert, MIT-Lizenz; `licenses/LICENSE-ConPTY.txt`).
