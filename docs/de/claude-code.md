[← README](../../README.de.md) · [English](../en/claude-code.md) · **Deutsch**

# Claude Code

Paneldeck ist dafür gebaut, viele [Claude-Code](https://docs.anthropic.com/en/docs/claude-code)-Sitzungen nebeneinander zu betreiben. Die Claude-Funktionen bilden den **Claude-Modus** (*Einstellungen → Claude*, standardmäßig an); ohne ihn ist Paneldeck ein schlichter Terminal-Manager.

> Paneldeck ist ein unabhängiges Projekt und nicht mit Anthropic verbunden. Es startet das `claude`-Programm, das du selbst installiert hast, und liest dessen lokale Dateien; es enthält keinen Code von Anthropic.

## Eine Claude-Sitzung starten
- Auf den **Funken-Knopf** klicken (oder *Neue Claude-Sitzung …* in der Seitenleiste bzw. im leeren Terminal Hub), Ordner wählen, fertig. Der Start-Dialog bietet auch Modell und Permission-Mode an.
- Rechtsklick auf einen Ordner im Explorer → *Mit Claude in Paneldeck starten* (siehe [Windows-Integration](windows-integration.md)) oder `paneldeck.exe --open <ordner> --claude` (siehe [Kommandozeile](command-line.md)).
- Eine Schnellaktion eines Tickets kann eine mit dem Ticket verknüpfte Claude-Sitzung starten (siehe [Todo-Liste und Tickets](todo-and-tickets.md#schnellaktionen-und-schnellmenü)).

## Status, Benachrichtigungen und Posteingang
- Paneldeck richtet **Hooks** für Claude Code ein (*Einstellungen → Claude*; erzeugte Konfiguration `claude-hooks*.json` im Datenordner, auf Wunsch global in `~/.claude/settings.json` installiert). Damit zeigt jede Sitzung ihren Status: **arbeitet**, **Freigabe?** oder **bereit**.
- Sitzungen, die auf dich warten, stehen im Posteingang **„Wartet auf dich“** in der Seitenleiste.
- **Benachrichtigungen** (*Einstellungen → Benachrichtigungen*): bei Freigabe oder wenn Claude fertig ist, während das Fenster im Hintergrund ist – Toast, Ton, blinkender Taskleistenknopf.

## Kosten
- Token- und **Kostenschätzung** je Sitzung und insgesamt, auf Basis der lokalen Transkripte in `~/.claude` (nur eine Schätzung; unbekannte Modelle werden wie Sonnet gerechnet). Die Preise pro Million Token lassen sich unter *Einstellungen → Claude* ändern.
- Die **Kosten-Übersicht** (Seitenleiste) zeigt heute, 7 Tage, Gesamt, das Tagesbudget und die teuersten Sitzungen.
- **Tagesbudget** und **Sitzungsbudget** (USD, 0 = aus): ein Hinweis, wenn die geschätzten Kosten den Betrag überschreiten.

<p align="center"><img src="../images/de/kosten.png" alt="Kosten-Übersicht" width="85%"></p>

## Verlauf und Fortsetzen
- **Claude-Verlauf** (Seitenleiste): frühere Unterhaltungen durchsuchen und exakt fortsetzen.
- *Claude-Sitzungen beim Start mit --resume wiederherstellen* (*Einstellungen → Claude*): Nach einem Neustart von Paneldeck (auch nach einem Selbst-Update) machen Claude-Sitzungen dort weiter, wo sie waren.

## Claude-Dateien
- **Strg+Umschalt+K** öffnet einen Editor für `CLAUDE.md` und die Claude-Einstellungsdateien des Sitzungsordners (und die globalen).
- **Prompt-Bibliothek**, **Prompt-Composer** (**Strg+Umschalt+M**) und **Warteschlange** (**Strg+Umschalt+Q**: Prompts, die nacheinander gesendet werden, sobald Claude bereit ist) helfen bei längeren Arbeiten.

## Claude und die Agenten-Schnittstelle
Claude Code kann Paneldeck über die [Agenten-Schnittstelle (MCP)](agent-interface.md) auch *steuern*: Sitzungen auflisten, Terminals lesen, Kinder-Sitzungen starten und mehrere Claude-Sitzungen koordinieren oder mit deiner Todo-Liste und deinen Tickets arbeiten.

## Wenn Status oder Kosten fehlen
Prüfen, ob `claude` installiert ist und ob in *Einstellungen → Claude* Hooks aktiv sind – siehe [Fehlersuche](troubleshooting.md).
