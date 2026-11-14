<h1 align="center"><img src="docs/images/banner.png" alt="Paneldeck" width="100%"></h1>

<p align="center"><a href="README.md">English</a> · <strong>Deutsch</strong></p>

Zentrale für viele Terminals – gebaut für Claude Code, nutzbar als allgemeiner Terminal-Manager.
Für Windows 10 (1809+) / 11 (x64), **komplett offline lauffähig**: wahlweise als Installer (pro Benutzer, ohne Administratorrechte) oder als portables ZIP mit einer einzelnen `paneldeck.exe`.

> Unabhängiges Open-Source-Projekt – nicht mit Anthropic verbunden und nicht von Anthropic unterstützt oder geprüft. „Claude“ und „Claude Code“ sind Marken von Anthropic, PBC. Details unter [Rechtliches](#rechtliches).

<p align="center"><img src="docs/images/app.png" alt="Paneldeck mit geteilten Terminals, Freigabe-Anfrage und Seitenleiste" width="100%"><br><sub>Mehrere Sitzungen in Projekten und Ordnern, geteilte Bereiche, „Wartet auf dich“-Posteingang – alles in einem Fenster.</sub></p>

## Funktionen
- **Terminals** mit voller Emulation (ConPTY): PowerShell 7, Windows PowerShell, CMD, Git Bash, WSL-Distributionen; Farben, Alt-Screen, Maus, Unicode, farbige Emojis, CJK, lückenlose Rahmenzeichen.
- **Organisation**: Sitzungen in Projekten mit eigenem Layout, Gruppen, Farben, Tags, Filter, Notizen; frei teilbare Bereiche, Drag & Drop, Abdocken in eigene Fenster.
- **Claude-Modus** (abschaltbar): Statusanzeige per Hooks (arbeitet / Freigabe? / bereit), Benachrichtigungen, Token- und Kostenschätzung, Verlauf mit exaktem Fortsetzen, Start-Dialog mit Modell/Permission-Mode, Editor für `CLAUDE.md` und Claude-Settings.
- **Komfort**: Command Palette, Suche im Verlauf, Prompt-Bibliothek, Broadcast an mehrere Terminals, Dateibaum, Git-Info, Drag & Drop von Dateien, Bilder aus der Zwischenablage, Tray, Transparenz, weiche Animationen.
- **Agenten-Schnittstelle (MCP)**: KI-Agenten wie Claude Code können Sitzungen sehen, Terminals lesen, Befehle ausführen und mehrere Claude-Sitzungen koordinieren – standardmäßig aus, mit Stufen „Nur lesen“ / „Steuern“ (siehe unten).
- **Anpassung**: Tastenkürzel, eigene Themes (JSON), Terminalschrift, Englisch/Deutsch (Standard: Englisch; umstellbar unter *Einstellungen → Allgemein → Sprache*).

<p align="center"><img src="docs/images/kosten.png" alt="Kosten-Übersicht" width="85%"><br><sub>Kosten-Übersicht: heute, 7 Tage, Gesamt, Tagesbudget und teuerste Sitzungen.</sub></p>

## Erste Schritte
1. **Installer** `Paneldeck-Setup-<Version>.exe` ausführen – oder das **portable ZIP** entpacken und `paneldeck.exe` starten.
2. **Strg+Umschalt+T** öffnet eine neue Sitzung, **Strg+Umschalt+P** die Befehls-Palette, **Strg+Umschalt+D/E** teilt einen Bereich.
3. Für Claude: Button mit dem Funken-Symbol, Ordner wählen, fertig.

Alle Kürzel lassen sich unter *Einstellungen → Tastenkürzel* ändern.

### Hinweis: Programm ist (noch) nicht signiert
Installer und `paneldeck.exe` sind **nicht code-signiert**. Ein Zertifikat kostet Geld, und Paneldeck soll ein kostenloses Hobbyprojekt bleiben. Deshalb gilt bis auf Weiteres:
- Beim ersten Start zeigt **Windows SmartScreen** evtl. „Der Computer wurde durch Windows geschützt“. Auf **„Weitere Informationen“ → „Trotzdem ausführen“** klicken.
- Manche Virenscanner schlagen bei neuen, unsignierten Programmen mit Tray-Symbol und Autostart fälschlich an. Der Quelltext ist offen einsehbar, und du kannst Paneldeck selbst bauen (siehe [Bauen](#bauen)).
- **Geplant:** Ich versuche demnächst, ein kostenloses Signatur-Zertifikat für Open-Source-Projekte zu bekommen (z. B. über die SignPath Foundation). Ob und wann das klappt, ist offen; sobald es so weit ist, steht es hier und im Changelog.

## Agenten-Schnittstelle (MCP)
Paneldeck enthält einen MCP-Server, damit KI-Agenten (Claude Code, Claude Desktop, Cursor …) deine Sitzungen sehen und steuern können.

1. **Einschalten:** *Einstellungen → Agenten* → **Nur lesen** oder **Steuern**. Ohne diesen Schritt lehnt der Hub jede Anfrage ab (`mcp_disabled`).
2. **Registrieren:** im selben Tab auf *Registrieren* klicken (oder von Hand: `claude mcp add --scope user paneldeck -- "<Pfad>\paneldeck.exe" mcp`). Für andere Agenten liefert *JSON für andere Agenten* den `mcpServers`-Eintrag. Der Agent startet dann bei Bedarf `paneldeck.exe mcp`; Paneldeck muss laufen.
3. **Benutzen:** z. B. in Claude Code: „Liste meine Paneldeck-Sitzungen“, „Führe in Sitzung 3 `npm test` aus und fasse das Ergebnis zusammen“, „Warte, bis Sitzung 5 fertig ist“.

<p align="center"><img src="docs/images/agenten.png" alt="Einstellungen, Tab Agenten" width="85%"><br><sub>Einstellungen → Agenten: Stufe wählen und mit einem Klick in Claude Code registrieren.</sub></p>

| Werkzeug | Stufe | Zweck |
|---|---|---|
| `whoami`, `list_sessions`, `read_terminal`, `screenshot`, `get_usage`, `search_history` | Lesen | Wer bin ich (Sitzung des Agenten), Sitzungen, Terminaltext, Bild von Fenster/Sitzung, Kosten, Claude-Verlauf |
| `create_session`, `send_text`, `send_keys`, `run_command`, `queue_prompt`, `wait_for_status`, `rename_session`, `focus_session`, `close_session` | Steuern | Sitzungen anlegen/steuern, Befehle mit Ausgabe und Exit-Code, auf Claude-Status warten |

**Sicherheit:** Der Zugriff läuft nur lokal über die laufende Instanz (eigener Token). „Steuern“ lässt den Agenten in deine echten Terminals tippen – nutze es nur mit Agenten, denen du vertraust; Terminalinhalt kann Anweisungen enthalten, die ein Agent fälschlich befolgt. Ein Agent erkennt sich per `whoami` selbst, legt Kinder-Sitzungen unter seinem Ordner an und steuert sie; er schreibt nie in seine eigene Sitzung, nicht in Sitzungen mit offener Freigabe-Frage und schließt nur Sitzungen, die er selbst angelegt hat. Alle Aufrufe stehen im Aufrufprotokoll (Einstellungen → Agenten, `mcp-log.jsonl`). Der Screenshot braucht ein sichtbares (nicht minimiertes) Fenster.

## Daten (portabel)
Alles liegt im Ordner `paneldeck-data` neben der `.exe` (falls nicht beschreibbar: `%APPDATA%\Paneldeck`):

| Datei/Ordner | Inhalt |
|---|---|
| `settings.json` | Einstellungen (von Hand editierbar) |
| `state.json` | Sitzungen, Projekte, Layout (`state.defekt.json` = Sicherung, falls die Datei beschädigt war) |
| `scrollback/` | gespeicherter Terminalverlauf |
| `prompts.json` | Prompt-Bibliothek |
| `themes/*.json` | eigene Themes (Beispiel liegt bei) |
| `clipboard/` | Bilder aus der Zwischenablage (nach 14 Tagen gelöscht) |
| `claude-hooks*.json` | von Paneldeck erzeugte Hook-Konfiguration |
| `crash.log` | Absturzprotokoll (nur wenn etwas schiefging) |
| `mcp-log.jsonl` | Aufrufprotokoll der Agenten-Schnittstelle (nur wenn sie benutzt wurde) |

## Fehlersuche
- *Einstellungen → Allgemein → Diagnose kopieren* legt Systeminfos in die Zwischenablage – bitte bei Fehlerberichten mitschicken, zusammen mit `crash.log`, falls vorhanden.
- Status/Kosten erscheinen nicht? Prüfen, ob `claude` installiert ist und ob in *Einstellungen → Claude* Hooks aktiv sind.
- Sonderzeichen fehlen? Windows-Systemschriften werden als Fallback genutzt (Segoe UI Symbol, Microsoft YaHei, Yu Gothic, Malgun Gothic …).

## Bauen
**Unter Windows (nativ, MSVC)** – Rust (rustup, Toolchain `stable-x86_64-pc-windows-msvc`) und die Visual-Studio-Build-Tools genügen:

    cargo build --release        # -> target\release\paneldeck.exe
    cargo test --release

**Cross-Build unter Linux** (möglich, wird aber nicht regelmäßig getestet):

    rustup target add x86_64-pc-windows-gnu && apt install gcc-mingw-w64-x86-64
    cargo build --release --target x86_64-pc-windows-gnu

Release-Dateien (portables ZIP mit `paneldeck.exe` und Lizenzen, Installer) entstehen auf einem Branch `release/x.y` mit `tools/release.sh <version>` (Ablauf siehe `CLAUDE.md`, Branching- und Release-Strategie) – unter Windows aus Git Bash, braucht Python 3 und für den Installer NSIS (`makensis`).
Mit `--publish` wird zusätzlich das Tag gepusht und das GitHub-Release angelegt (GitHub CLI `gh`, angemeldet), mit `--no-git` nur gebaut und gepackt.

## Rechtliches
- **Lizenz:** MIT (`LICENSE`). Die Software wird ohne jede Gewährleistung bereitgestellt; die Nutzung erfolgt auf eigene Gefahr – das gilt besonders für die Agenten-Schnittstelle, die KI-Agenten in deine Terminals schreiben lässt.
- **Keine Verbindung zu Anthropic:** Paneldeck ist ein unabhängiges Projekt. „Claude“ und „Claude Code“ sind Marken von Anthropic, PBC und werden hier nur genannt, um zu beschreiben, mit welchem Programm Paneldeck zusammenarbeitet. Paneldeck enthält keinen Code und keine Logos von Anthropic.
- **Datenschutz:** Paneldeck baut keine Verbindungen ins Internet auf und sendet keine Telemetrie. Es liest lokal Claude-Transkripte (`~/.claude`) für Verlauf und Kostenschätzung; alles bleibt auf deinem Rechner.
- **Drittanbieter-Software:** Lizenzen und Hinweise stehen in `THIRD_PARTY.md` und `THIRD_PARTY_LICENSES.txt` (liegen Installer und ZIP bei). Die eingebetteten Schriften Cascadia Mono und Noto Color Emoji stehen unter der SIL Open Font License 1.1 (`assets/fonts/LICENSE-*.txt`).
