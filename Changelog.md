# Changelog

Alle wichtigen Änderungen an diesem Projekt werden in dieser Datei dokumentiert.

Das Format basiert auf [Keep a Changelog](https://keepachangelog.com/de/1.1.0/),
und dieses Projekt hält sich an [Semantic Versioning](https://semver.org/lang/de/).

## [Unreleased]

## [1.0.0] - 2026-11-14

Erste öffentliche Version. Ab hier gilt die öffentliche API (MCP-Schnittstelle, Kommandozeile, Datenformate im Datenordner) im Sinne von Semantic Versioning als verbindlich; frühere Entwicklungsstände wurden nicht veröffentlicht.

### Added

- **Terminals** mit voller Emulation über ConPTY: PowerShell 7, Windows PowerShell, CMD, Git Bash und WSL-Distributionen; Farben, Alt-Screen, Maus, Unicode, farbige Emojis, CJK und lückenlose Rahmenzeichen.
- **Organisation:** Sitzungen in Projekten mit eigenem Layout, Gruppen/Ordner (Baumansicht), Farben, Tags, Filter und Notizen; frei teilbare Bereiche, Drag & Drop (auch von Ordnern), Abdocken in eigene Fenster, Git-Worktrees pro Sitzung.
- **Claude-Modus** (abschaltbar): Statusanzeige per Hooks (arbeitet / Freigabe? / bereit), Benachrichtigungen, Token- und Kostenschätzung mit Kosten-Übersicht, Tages- und Sitzungsbudget, Verlauf mit exaktem Fortsetzen, Start-Dialog (Modell, Permission-Mode), Editor für `CLAUDE.md` und Claude-Einstellungen.
- **Komfort:** Befehls-Palette, Suche im Verlauf, Prompt-Bibliothek, Broadcast an mehrere Terminals, Dateibaum, Git-Info, Dateien und Bilder aus der Zwischenablage einfügen, Tray, Transparenz, Windows-Benachrichtigungen.
- **Agenten-Schnittstelle (MCP)** mit 15 Werkzeugen (`whoami`, `list_sessions`, `read_terminal`, `screenshot`, `get_usage`, `search_history`, `create_session`, `send_text`, `send_keys`, `run_command`, `wait_for_status`, `queue_prompt`, `rename_session`, `focus_session`, `close_session`): standardmäßig aus, Stufen „Nur lesen“ und „Steuern“, Zugriffsprüfung im Hub, Aufrufprotokoll; Agenten erkennen sich per `whoami` selbst und legen Kinder-Sitzungen unter ihrer eigenen an.
- **Anpassung:** Tastenkürzel, eigene Themes (JSON), Terminalschrift, Englisch (Standard) und Deutsch.
- **Windows-Integration ohne Administratorrechte:** Installer (pro Benutzer, mit Lizenzseite) und portables ZIP; Explorer-Kontextmenü, optionaler Autostart, Einzelinstanz; die Deinstallation entfernt Hooks und MCP-Registrierung wieder. Updates über einen laufenden Hub sind möglich.
- MIT-Lizenz (`LICENSE`); die Lizenztexte der Abhängigkeiten liegen Installer und ZIP bei (`THIRD_PARTY_LICENSES.txt`).

### Security

- Paneldeck baut keine Verbindungen ins Internet auf und sendet keine Telemetrie; Hook- und MCP-Verkehr läuft nur lokal (`127.0.0.1`, mit Token).
- Alle `unwrap()`/`expect()` im Programmcode sind durch Fehlerbehandlung ersetzt (der Release-Build bricht bei einem Panic komplett ab); die `unsafe`-Blöcke sind begründet.
- Die Programmdatei enthält keine Pfade des Build-Rechners.

[Unreleased]: https://github.com/zibbozz/paneldeck/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/zibbozz/paneldeck/releases/tag/v1.0.0
