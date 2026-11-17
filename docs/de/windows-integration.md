[← README](../../README.de.md) · [English](../en/windows-integration.md) · **Deutsch**

# Windows-Integration

Alles, was Paneldeck in Windows einrichtet, gilt pro Benutzer (`HKCU`, ohne Administratorrechte) und lässt sich wieder ausschalten. Die Einstellungen stehen unter *Einstellungen → Fenster*, Abschnitt *Windows-Integration (nur für dich, ohne Administratorrechte)*.

## Tray
- **Schließen → Tray:** Das Fenster verschwindet im Tray, statt sich zu beenden (Beenden über das Tray-Menü).
- **Minimieren → Tray:** kein Taskleistenknopf, solange minimiert; ein Klick aufs Tray-Symbol holt das Fenster zurück.
- `--no-tray` startet ohne Tray-Symbol; beide Optionen verhalten sich dann für diesen Start wie bei einem normalen Fenster (siehe [Kommandozeile](command-line.md#--no-tray)).
- Weitere Fenster (Strg+Klick auf einen Reiter) bleiben offen und aktualisieren sich weiter, während das Hauptfenster im Tray ist.

## Mit Windows starten (Autostart)
Startet Paneldeck nach der Anmeldung minimiert im Tray (`"<exe>" --tray`, Run-Wert `Paneldeck` unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`).

**Je Instanz:** Eine Instanz, die mit `--profile` oder `--data-dir` gestartet wurde, bekommt einen eigenen Run-Wert mit ihren eigenen Parametern, sodass jede Instanz unabhängig mit Windows starten kann:

| Instanz | Run-Wert | Befehl |
|---|---|---|
| Standard | `Paneldeck` | `"<exe>" --tray` |
| `--profile arbeit` | `Paneldeck (arbeit)` | `"<exe>" --profile arbeit --tray` |
| `--data-dir D:\pd` | `Paneldeck (<hash>)` | `"<exe>" --data-dir "D:\pd" --tray` |
| beides | `Paneldeck (arbeit-<hash>)` | `"<exe>" --data-dir "D:\pd" --profile arbeit --tray` |

`<hash>` ist ein kurzer, stabiler Hash aus 8 Zeichen über den Datenordner. Schaltet eine Instanz den Autostart aus, entfernt sie nur ihren eigenen Wert.

## Explorer-Kontextmenü
Rechtsklick auf einen Ordner oder den leeren Hintergrund eines Ordnerfensters (auch einer Laufwerkswurzel wie `C:\`): **In Paneldeck öffnen** / **Mit Claude in Paneldeck starten**. Die Einträge gelten für Ordner (`Directory\shell`) und Ordner-Hintergründe (`Directory\Background\shell`), nicht für die Laufwerkssymbole selbst (z. B. unter *Dieser PC*). Die Befehle sind `"<exe>" [Instanz-Optionen] --open "%1"` und `"<exe>" [Instanz-Optionen] --claude --open "%1"` (`%V` für den Hintergrund).

*In Paneldeck öffnen* auf einem WSL-Ordner (`\\wsl.localhost\<Distro>\…`) öffnet eine WSL-Sitzung dieser Distribution im passenden Linux-Pfad (siehe [WSL-Startordner](terminals.md#wsl-startordner)).

Es gibt **nur ein Kontextmenü**: Die Instanz, die es zuletzt einschaltet, wird von ihm gestartet. Eine andere Instanz zeigt dann „→ anderes Paneldeck“ neben dem Schalter (der Befehl steht im Tooltip); schaltet man es dort ein, übernimmt sie es.

## Status in den Einstellungen
Für Autostart und Kontextmenü zeigt *Einstellungen → Fenster*, ob der Eintrag **aus** ist, **an** (genau mit dem Befehl dieser Instanz) oder auf ein **anderes Paneldeck** zeigt (eine andere `.exe` oder eine andere Instanz).

Enthält der Pfad von `paneldeck.exe` ein `%`, kann Paneldeck diese Befehle nicht sicher schreiben und lehnt sie mit einer Meldung ab.

## Installer und Deinstallation
- Der Installer bietet Desktop-Verknüpfung, Autostart und Kontextmenü nur für die **Standardinstanz** an. Ein stilles Update (auch das [Selbst-Update](updates.md)) behält deine Wahl: Eine gelöschte Desktop-Verknüpfung oder ein abgeschaltetes Kontextmenü kommen nicht zurück.
- Die Deinstallation entfernt die Standard-Einträge, die Autostart-Werte `Paneldeck (…)` von Profil-Instanzen, die auf das installierte Programm zeigen, die Claude-Code-Hooks und die MCP-Registrierungen dieses Paneldeck (siehe [Agenten-Schnittstelle](agent-interface.md#deinstallieren)). Fremde Einträge bleiben unberührt. Sie entfernt auch den mitgelieferten Konsolen-Host (`conpty.dll`, `OpenConsole.exe`) und fragt, ob der Datenordner gelöscht werden soll.

## Sprache
Die Beschriftungen des Kontextmenüs stehen in der gespeicherten Sprache der Instanz, die es eingeschaltet hat (Englisch oder Deutsch; `--lang` ändert sie nicht).
