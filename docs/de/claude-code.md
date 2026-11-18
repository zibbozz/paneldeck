[← README](../../README.de.md) · [English](../en/claude-code.md) · **Deutsch**

# Claude Code

Paneldeck ist dafür gebaut, viele [Claude-Code](https://docs.anthropic.com/en/docs/claude-code)-Sitzungen nebeneinander zu betreiben. Die Claude-Funktionen bilden den **Claude-Modus** (*Einstellungen → Claude*, standardmäßig an); ohne ihn ist Paneldeck ein schlichter Terminal-Manager.

> Paneldeck ist ein unabhängiges Projekt und nicht mit Anthropic verbunden. Es startet das `claude`-Programm, das du selbst installiert hast, und liest dessen lokale Dateien; es enthält keinen Code von Anthropic.

## Eine Claude-Sitzung starten
- Auf den **Funken-Knopf** klicken (oder *Neue Claude-Sitzung …* in der Seitenleiste bzw. im leeren Terminal Hub), Ordner wählen, fertig. Der Dialog bietet auch die [Startoptionen](#startoptionen) an.
- Rechtsklick auf einen Ordner im Explorer → *Mit Claude in Paneldeck starten* (siehe [Windows-Integration](windows-integration.md)) oder `paneldeck.exe --open <ordner> --claude` (siehe [Kommandozeile](command-line.md)).
- Eine Schnellaktion eines Tickets kann eine mit dem Ticket verknüpfte Claude-Sitzung starten (siehe [Todo-Liste und Tickets](todo-and-tickets.md#schnellaktionen-und-schnellmenü)).

## Startoptionen
Claude-Sitzungen lassen sich mit Optionen für Claude Code starten. Du setzt sie an drei Stellen: als **Standard** unter *Einstellungen → Claude → Standards für neue Claude-Sitzungen*, je Sitzung im Dialog **Neue Sitzung** (er wählt deine letzte Auswahl vor) und später in den **Einstellungen der Sitzung** (Kontextmenü der Seitenleiste → *Einstellungen …*; eine Änderung fragt *Jetzt neu laden?*). Ein leeres Feld heißt „Standard von Claude Code“.

- **Modell** (ein Alias wie `fable`, `opus`, `sonnet`, `haiku` oder ein voller Modellname), **Effort** (`low`, `medium`, `high`, `xhigh`, `max`), **Permission-Mode** (`acceptEdits`, `auto`, `plan`, `default`, `dontAsk`) und **Agent** (der Name eines Claude-Code-Agenten, den du angelegt hast). Namen dürfen nur Buchstaben, Ziffern und `. _ : -` enthalten (höchstens 100 Zeichen).
- **bypassPermissions** geht nur als ausdrückliche Wahl für eine Sitzung, mit Warnung – nie als Standard, nie als Vorauswahl beim nächsten Mal und nie für Agenten. Ein `bypassPermissions`-Standard aus einer älteren Einstellungsdatei wird mit Hinweis verworfen; alte `--dangerously-*`-Argumente einer Sitzung fallen ebenfalls weg.
- **Konversation** (im Dialog „Neue Sitzung“, eine Auswahl): *Neu – beim Neu laden/Neustart fortsetzen* (Standard), *Letzte Konversation im Ordner fortsetzen* (`--continue` einmal beim ersten Start, danach wie der Standard) oder *Bei jedem Start eine neue*. In den Einstellungen der Sitzung ist das der Haken *Konversation dieser Sitzung beim Neu laden/Neustart fortsetzen*.
- **Weitere Claude-Optionen** (aufklappbar, im Dialog, in den Einstellungen der Sitzung und bei den Standards):
  - **Schalter** ohne Wert: `--chrome` / `--no-chrome`, `--ide`, `--verbose`, `--brief`, `--restricted`, `--disable-slash-commands` und `--safe-mode`. `--safe-mode` schaltet Hooks und MCP-Server ab – Paneldeck erkennt dann Status und Freigabe-Fragen nicht – und zeigt eine Warnung.
  - **Zusätzliche Ordner** (`--add-dir`, einer je Zeile), **Erlaubte Werkzeuge** und **Verbotene Werkzeuge**, eine **MCP-Konfiguration** (JSON-Datei), **System-Prompt ergänzen**, ein **Ersatzmodell**, **Budget (USD)** und **Auto-Komprimierung** (`auto` oder eine Tokenzahl, z. B. `200k`).
  - Optionen, die Zugriffe erweitern (zusätzliche Ordner, erlaubte Werkzeuge, eine MCP-Konfiguration), zeigen eine Warnung. Pfade müssen lokal sein; Netzwerkfreigaben werden nie berührt, auch nicht über Verknüpfungen. Ordner oder Dateien, die beim Start fehlen, werden mit Hinweis weggelassen. Alle Werte werden geprüft und für die Shell gequotet.
- **Für Sitzungen, die Agenten anlegen** (MCP), und für `paneldeck --open --claude` aus einem Hub-Terminal gelten deine Standards für zusätzliche Ordner, erlaubte Werkzeuge, die MCP-Konfiguration und `--safe-mode` nie; Agenten können nur harmlose Schalter ergänzen (siehe [Agenten-Schnittstelle](agent-interface.md#claude-startoptionen)).
- Die Optionen stehen je Sitzung in `state.json` (`claude_opts`, `claude_continue`) und als Standard in `settings.json` (`claude_defaults`); siehe [Datenordner](data-and-backups.md#neue-felder-in-230).

## Status, Benachrichtigungen und Posteingang
- Paneldeck richtet **Hooks** für Claude Code ein (*Einstellungen → Claude*; erzeugte Konfiguration `claude-hooks*.json` im Datenordner, auf Wunsch global in `~/.claude/settings.json` installiert). Damit zeigt jede Sitzung ihren Status: **arbeitet**, **Freigabe?** oder **bereit**. Hook-Meldungen zählen nur für den laufenden Start der Sitzung (sie tragen `PANELDECK_SESSION_KEY`): Ein in der Sitzung gestartetes `claude -p`, übrig gebliebene Prozesse oder andere Terminals ändern Status und Konversation einer Sitzung nicht. Mit `--safe-mode` gestartete Sitzungen ignorieren Hook-Meldungen. Zeigen die globalen Hooks in `~/.claude/settings.json` auf eine andere oder ältere `paneldeck.exe` (zweite Installation), zählen ihre Meldungen nur für den Status; Paneldeck zeigt einen Hinweis, und *Einstellungen → Claude → Hooks* bietet an, sie neu einzutragen. Alte Claude-Argumente einer Sitzung, die Paneldeck kennt (weitere Ordner, Werkzeuge, MCP-Konfiguration, Ersatzmodell, Budget, Zusatz zum System-Prompt), werden beim ersten Start zu Claude-Optionen; die übrigen nennt einmalig ein Hinweis (Vorlagen übernehmen aus alten Argumenten nie weitere Ordner, erlaubte Werkzeuge oder eine MCP-Konfiguration).
- Sitzungen, die auf dich warten, stehen im Posteingang **„Wartet auf dich“** in der Seitenleiste.
- **Benachrichtigungen** (*Einstellungen → Benachrichtigungen*): bei Freigabe oder wenn Claude fertig ist, während das Fenster im Hintergrund ist – Toast, Ton, blinkender Taskleistenknopf.

## Kosten
- Token- und **Kostenschätzung** je Sitzung und insgesamt, auf Basis der lokalen Transkripte in `~/.claude` (nur eine Schätzung; unbekannte Modelle werden wie Sonnet gerechnet). Die Preise pro Million Token lassen sich unter *Einstellungen → Claude* ändern.
- Die **Kosten-Übersicht** (Seitenleiste) zeigt heute, 7 Tage, Gesamt, das Tagesbudget und die teuersten Sitzungen.
- **Tagesbudget** und **Sitzungsbudget** (USD, 0 = aus): ein Hinweis, wenn die geschätzten Kosten den Betrag überschreiten.

<p align="center"><img src="../images/de/kosten.png" alt="Kosten-Übersicht" width="85%"></p>

## Verlauf und Fortsetzen
- **Claude-Verlauf** (Seitenleiste): frühere Unterhaltungen durchsuchen und exakt fortsetzen.
- *Claude-Sitzungen beim Start mit --resume wiederherstellen* (*Einstellungen → Claude*): Nach einem Neustart von Paneldeck (auch nach einem Selbst-Update) machen Claude-Sitzungen dort weiter, wo sie waren – außer *Konversation fortsetzen* ist bei einer Sitzung aus.
- **Neu laden** (Kopf des Bereichs, Kontextmenü der Seitenleiste, Palette) startet Claude Code in der Sitzung mit ihren Startoptionen neu: Es setzt die Konversation fort oder beginnt eine neue, je nach Einstellung der Sitzung (siehe [Terminals – Neu laden](terminals.md#neu-laden-und-einstellungen-der-sitzung)).
- **Regeln fürs Fortsetzen:** Eine Sitzung setzt nur **ihre eigene** Konversation fort (`--resume <id>` mit der ID, die Paneldeck ihr beim Start gegeben hat), nie die neueste im Ordner, und nur, wenn es sie gibt – unter Windows prüft Paneldeck das Transkript in `~/.claude/projects` (geht auch ohne Hooks), in WSL verlässt es sich auf die Hooks. Sonst beginnt Claude eine neue Konversation. Auch ein geänderter Arbeitsordner oder eine andere Shell in den Einstellungen der Sitzung beginnt eine neue Konversation. `--continue` gibt es nur bei *Letzte Konversation im Ordner fortsetzen*. Alte gespeicherte Argumente wie `--resume`, `--continue` oder `--session-id` fallen weg.
- *Fortsetzen* im Claude-Verlauf öffnet die Konversation immer in einer Windows-Shell (der Verlauf kommt aus dem Windows-`~/.claude`); ist sie schon in einer Sitzung offen, wechselt Paneldeck dorthin.

## Claude-Dateien
- **Strg+Umschalt+K** öffnet einen Editor für `CLAUDE.md` und die Claude-Einstellungsdateien des Sitzungsordners (und die globalen).
- **Prompt-Bibliothek**, **Prompt-Composer** (**Strg+Umschalt+M**) und **Warteschlange** (**Strg+Umschalt+Q**: Prompts, die nacheinander gesendet werden, sobald Claude bereit ist) helfen bei längeren Arbeiten.

## Claude und die Agenten-Schnittstelle
Claude Code kann Paneldeck über die [Agenten-Schnittstelle (MCP)](agent-interface.md) auch *steuern*: Sitzungen auflisten, Terminals lesen, Kinder-Sitzungen starten und mehrere Claude-Sitzungen koordinieren oder mit deiner Todo-Liste und deinen Tickets arbeiten.

## Wenn Status oder Kosten fehlen
Prüfen, ob `claude` installiert ist und ob in *Einstellungen → Claude* Hooks aktiv sind – siehe [Fehlersuche](troubleshooting.md).
