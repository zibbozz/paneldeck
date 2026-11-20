[← README](../../README.de.md) · [English](../en/linux.md) · **Deutsch**

# Linux

Ab 2.5.0 läuft Paneldeck unter Linux (x86_64). Mindestens: **Ubuntu 22.04, Debian 12, Fedora 39** oder eine gleich alte Distribution (glibc 2.35). X11 und Wayland funktionieren beide.

## Installieren
Jedes Release unter [GitHub Releases](https://github.com/zibbozz/paneldeck/releases) hat vier Linux-Dateien, von GitHub Actions aus dem getaggten Quelltext gebaut, dazu `SHA256SUMS.txt`:

| Datei | Für | Updates |
|---|---|---|
| `Paneldeck-<Version>-x86_64.AppImage` | jede Distribution, ohne Installation | **Selbst-Update** (siehe unten) |
| `paneldeck_<Version>_amd64.deb` | Debian, Ubuntu, Mint … | über das Paket (Paneldeck meldet neue Versionen nur) |
| `paneldeck-<Version>-1.x86_64.rpm` | Fedora, openSUSE … | über das Paket (nur Hinweis) |
| `paneldeck-<Version>-linux-x86_64.tar.gz` | irgendwo entpacken | von Hand (nur Hinweis) |

- **AppImage:** `chmod +x Paneldeck-*.AppImage`, dann starten. Ohne FUSE 2 (`libfuse2`) mit `--appimage-extract-and-run` starten oder das Paket `libfuse2`/`fuse-libs` installieren.
- **.deb:** `sudo apt install ./paneldeck_<Version>_amd64.deb` · **.rpm:** `sudo dnf install ./paneldeck-<Version>-1.x86_64.rpm`. Beide installieren `/usr/bin/paneldeck`, einen Menüeintrag und das Symbol.
- **Tarball:** entpacken, `paneldeck-<Version>/paneldeck` starten. Für einen Menüeintrag `paneldeck.desktop` nach `~/.local/share/applications/` kopieren (`Exec=` anpassen).
- Einen Download selbst prüfen: `sha256sum -c --ignore-missing SHA256SUMS.txt`.

## Datenordner
`~/.local/share/Paneldeck` (bzw. `$XDG_DATA_HOME/Paneldeck`). **Portabel:** einen Ordner `paneldeck-data` neben dem Programm (Tarball) bzw. neben der `.AppImage`-Datei anlegen – gibt es ihn, nimmt Paneldeck ihn. Anders als unter Windows wird er nie von selbst angelegt, damit ein entpacktes Archiv nicht unbemerkt zum Datenordner wird. `--data-dir`, `PANELDECK_DATA` und `--profile` gelten wie überall (siehe [Kommandozeile](command-line.md)).

## Shells
Die Terminals nutzen die Shells aus `/etc/shells`; Standard ist deine Anmelde-Shell (`$SHELL`).

## Desktop-Integration
Alle Schalter stehen unter *Einstellungen → Fenster*, Abschnitt *Desktop-Integration*. Geschrieben wird nur in deinen Benutzerordner (kein root), für jedes Paketformat gleich.
- **Tray:** ein StatusNotifierItem (KDE, Xfce, Cinnamon, MATE, Budgie …). **GNOME** braucht die Erweiterung *AppIndicator and KStatusNotifierItem Support*; ohne Tray läuft Paneldeck normal, nur *Schließen → Tray* und *Minimieren → Tray* wirken dann nicht.
- **Bei der Anmeldung starten** (standardmäßig aus): ein XDG-Autostart-Eintrag `~/.config/autostart/paneldeck.desktop` (Instanzen mit `--profile`/`--data-dir` bekommen eine eigene Datei), Start minimiert im Tray.
- **Dateimanager: „In Paneldeck öffnen“** (standardmäßig aus): Einschalten schreibt „In Paneldeck öffnen“ und „Mit Claude in Paneldeck starten“ für **Dolphin** (Dienstmenü, Rechtsklick auf einen Ordner), **Nemo** (Aktionen, Rechtsklick auf einen Ordner oder den Hintergrund) und **Nautilus** (Rechtsklick → *Skripte*). Ausschalten entfernt genau diese Dateien wieder (Dateien ohne Paneldeck-Markierung bleiben unberührt).
- **Benachrichtigungen** (standardmäßig an) laufen über den Benachrichtigungsdienst des Desktops (D-Bus).

## API-Schlüssel der Chat-Server
API-Schlüssel landen nie im Klartext in einer Datei:
- Mit einem **Schlüsselbund des Systems** (Secret Service: GNOME Keyring, KWallet, KeePassXC …) legt Paneldeck dort einen zufälligen Schlüssel ab („Paneldeck – API key encryption“) und speichert die API-Schlüssel damit verschlüsselt in `settings.json`. Der Schlüsselbund fragt evtl. nach dem Entsperren.
- **Ohne Schlüsselbund** fragt Paneldeck nach einem **Master-Passwort** – erst beim ersten Zugriff auf einen Schlüssel (Schlüssel speichern, Chat mit einem Server mit Schlüssel), nie beim Start; Autostart und Tray bleiben still. Das Passwort bleibt bis zum Beenden im Speicher und wird nirgends gespeichert. Ohne es lassen sich gespeicherte Schlüssel nicht mehr lesen (Schlüssel neu eingeben).

## Selbst-Update (AppImage)
Die AppImage aktualisiert sich wie die portable Windows-Version: Die neue `.AppImage` wird von GitHub geladen, **gegen `SHA256SUMS.txt` geprüft**, an die Stelle der alten gesetzt (die bleibt als `.old`, bis die neue Version gestartet ist) und Paneldeck startet neu. Ehrlich gesagt: Die Prüfsumme schützt vor **Übertragungsfehlern und unvollständigen Downloads, nicht vor einem manipulierten Release** – eine eigene Signatur gibt es nicht. `.deb`, `.rpm` und der Tarball melden eine neue Version nur mit Link; sie werden über die Paketverwaltung bzw. von Hand aktualisiert.

## Agenten-Schnittstelle (MCP)
Funktioniert wie unter Windows. Der Hub ermittelt den aufrufenden Prozess selbst (Besitzer der TCP-Verbindung aus `/proc/net/tcp` und `/proc/<pid>/fd`, sein Prozessbaum und statt der Job Objects von Windows eine Markierung je Sitzung in der Umgebung). Ein Aufrufer, der sich nicht bestimmen lässt, wird **abgewiesen**. Wie unter Windows ist das keine Grenze gegen feindliche Programme desselben Benutzers, siehe [Agenten-Schnittstelle](agent-interface.md).

## Grenzen (2.5.0)
- Dateidialoge (Export, Import, Ordner wählen) sind noch nicht nativ: Exporte landen im Datenordner, Ordner werden eingetippt oder in Paneldecks eigener Ordnerauswahl gewählt.
- Ein Bild aus der Zwischenablage einfügen (Strg+V ins Terminal) nutzt X11 (unter Wayland über XWayland).
- macOS folgt getrennt (experimentell, #150).
