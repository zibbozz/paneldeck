//! Hilfetext der Kommandozeile (`paneldeck.exe --help`), Englisch und Deutsch.

use crate::state::Lang;

pub fn text(l: Lang) -> String {
    let body = match l {
        Lang::En => EN,
        Lang::De => DE,
    };
    format!("Paneldeck {}\n\n{body}", env!("CARGO_PKG_VERSION"))
}

const EN: &str = r#"Usage:
  paneldeck.exe [options]
  paneldeck.exe mcp [--data-dir <folder>] [--profile <name>] [--lang en|de]

Options:
  --open <folder>         Open a new session in <folder> (a file opens its folder).
  --claude                With --open: start Claude Code in the new session.
  --tray                  Start minimized in the tray.
  --no-tray               No tray icon for this start.
  --data-dir <folder>     Use <folder> as the data folder.
  --profile <name>        Use the profile <name> (its own data folder).
  --backup-dir <folder>   Write database backups to <folder> for this start.
  --lang en|de            Interface language for this start.
  --reset-settings        Reset the settings (a backup copy is kept).
  --version               Print the version and exit.
  --help, -h, -?          Print this help and exit.
  mcp                     Agent interface (MCP server over stdin/stdout).

Data folder (first match wins):
  1. --data-dir <folder>
  2. environment variable PANELDECK_DATA
  3. portable folder "paneldeck-data" next to paneldeck.exe (if it can be written)
  4. %APPDATA%\Paneldeck
  5. the current directory (fallback)
  --profile <name> uses the subfolder "profiles\<name>" of that folder.

Notes:
  - Profiles run side by side: each data folder has its own instance, settings,
    sessions and database. Only one Paneldeck runs per data folder (instance lock);
    a second start with the same folder hands over to the running one.
  - --lang, --backup-dir and --no-tray apply only to this start and are never
    written to settings.json. They (and --reset-settings) only take effect when this
    start opens the instance: if Paneldeck already runs with that data folder, the
    start only hands --open/--claude over (--reset-settings then fails).
  - A single folder without an option (e.g. dropped onto paneldeck.exe) works
    like --open <folder>; a file opens its folder.
  - Exit codes: 0 = ok, 1 = error (e.g. unusable data folder), 2 = invalid command line.

Details and examples:

  --open <folder>
    Opens a new terminal session in <folder> in the running Paneldeck (or starts it).
    If <folder> is a file, its folder is used. The Explorer context menu uses this.
      paneldeck.exe --open "C:\Dev\my-app"

  --claude
    Together with --open: the new session starts Claude Code (needs Claude mode).
      paneldeck.exe --open "C:\Dev\my-app" --claude

  --tray
    Starts minimized to the tray icon (used by "Start with Windows"). If Paneldeck
    already runs, it is shown instead.
      paneldeck.exe --tray

  --no-tray
    Starts without a tray icon. "Close to tray" and "Minimize to tray" then behave
    like a normal window (close quits, minimize uses the taskbar).
      paneldeck.exe --no-tray

  --data-dir <folder>
    Uses <folder> for settings, sessions, database and everything else. The folder
    is created if needed; if it cannot be created or written, Paneldeck stops with
    an error (no silent fallback). A relative path is relative to the current directory.
      paneldeck.exe --data-dir "D:\PaneldeckData"

  --profile <name>
    Uses "<data folder>\profiles\<name>" and runs as its own instance next to the
    default one. Names: 1-40 characters from A-Z, a-z, 0-9, _ and -.
    Can be combined with --data-dir or PANELDECK_DATA.
      paneldeck.exe --profile work
      paneldeck.exe --data-dir "D:\PaneldeckData" --profile customer-a

  --backup-dir <folder>
    Writes database backups to <folder> for this start (the folder in
    Settings > Data & backup is then locked).
      paneldeck.exe --backup-dir "E:\Backups\Paneldeck"

  --lang en|de
    Interface and message language for this start; the saved language stays.
      paneldeck.exe --lang de

  --reset-settings
    Renames settings.json to "settings.json.bak-<YYYYMMDD-HHMMSS>" and starts with
    default settings. Sessions, prompts, themes and the database stay untouched.
    Not possible while Paneldeck runs with the same data folder.
      paneldeck.exe --reset-settings
      paneldeck.exe --profile work --reset-settings

  --version
    Prints the version number.
      paneldeck.exe --version

  --help, -h, -?
    Prints this help (in the language of --lang or of the saved settings).
      paneldeck.exe --help > paneldeck-help.txt

  mcp
    Runs the MCP server for AI agents over stdin/stdout and talks to the running
    Paneldeck. It finds the hub in this order: --data-dir/--profile, the Paneldeck
    terminal it was started from, then the data folder lookup above. The agent
    interface must be enabled in Settings > Agents.
      claude mcp add paneldeck --scope user -- "C:\Program Files\Paneldeck\paneldeck.exe" mcp
      paneldeck.exe mcp --profile work
"#;

const DE: &str = r#"Aufruf:
  paneldeck.exe [Optionen]
  paneldeck.exe mcp [--data-dir <ordner>] [--profile <name>] [--lang en|de]

Optionen:
  --open <ordner>         Neue Sitzung in <ordner> öffnen (eine Datei öffnet ihren Ordner).
  --claude                Mit --open: Claude Code in der neuen Sitzung starten.
  --tray                  Minimiert im Tray starten.
  --no-tray               Kein Tray-Symbol für diesen Start.
  --data-dir <ordner>     <ordner> als Datenordner verwenden.
  --profile <name>        Profil <name> verwenden (eigener Datenordner).
  --backup-dir <ordner>   Datenbank-Sicherungen für diesen Start nach <ordner> schreiben.
  --lang en|de            Sprache der Oberfläche für diesen Start.
  --reset-settings        Einstellungen zurücksetzen (eine Sicherungskopie bleibt).
  --version               Version ausgeben und beenden.
  --help, -h, -?          Diese Hilfe ausgeben und beenden.
  mcp                     Agenten-Schnittstelle (MCP-Server über stdin/stdout).

Datenordner (der erste Treffer gilt):
  1. --data-dir <ordner>
  2. Umgebungsvariable PANELDECK_DATA
  3. portabler Ordner „paneldeck-data“ neben paneldeck.exe (wenn beschreibbar)
  4. %APPDATA%\Paneldeck
  5. das aktuelle Verzeichnis (Notlösung)
  --profile <name> nutzt den Unterordner „profiles\<name>“ dieses Ordners.

Hinweise:
  - Profile laufen nebeneinander: Jeder Datenordner hat eine eigene Instanz, eigene
    Einstellungen, Sitzungen und Datenbank. Pro Datenordner läuft nur ein Paneldeck
    (Instanzsperre); ein zweiter Start mit demselben Ordner übergibt an das laufende.
  - --lang, --backup-dir und --no-tray gelten nur für diesen Start und werden nie
    in settings.json gespeichert. Sie (und --reset-settings) wirken nur, wenn dieser
    Start die Instanz öffnet: Läuft Paneldeck mit diesem Datenordner schon, übergibt
    der Start nur --open/--claude (--reset-settings schlägt dann fehl).
  - Ein einzelner Ordner ohne Option (z. B. auf paneldeck.exe gezogen) wirkt wie
    --open <ordner>; eine Datei öffnet ihren Ordner.
  - Exit-Codes: 0 = ok, 1 = Fehler (z. B. unbrauchbarer Datenordner), 2 = ungültige Kommandozeile.

Einzelheiten und Beispiele:

  --open <ordner>
    Öffnet im laufenden Paneldeck (oder nach dem Start) eine neue Terminal-Sitzung in
    <ordner>. Ist <ordner> eine Datei, gilt ihr Ordner. Das Explorer-Kontextmenü nutzt das.
      paneldeck.exe --open "C:\Dev\meine-app"

  --claude
    Zusammen mit --open: Die neue Sitzung startet Claude Code (Claude-Modus nötig).
      paneldeck.exe --open "C:\Dev\meine-app" --claude

  --tray
    Startet minimiert im Tray (so startet „Mit Windows starten“). Läuft Paneldeck
    schon, wird es stattdessen angezeigt.
      paneldeck.exe --tray

  --no-tray
    Startet ohne Tray-Symbol. „Schließen → Tray“ und „Minimieren → Tray“ verhalten sich
    dann wie bei einem normalen Fenster (Schließen beendet, Minimieren in die Taskleiste).
      paneldeck.exe --no-tray

  --data-dir <ordner>
    Nutzt <ordner> für Einstellungen, Sitzungen, Datenbank und alles Weitere. Der
    Ordner wird bei Bedarf angelegt; lässt er sich nicht anlegen oder beschreiben,
    bricht Paneldeck mit einer Fehlermeldung ab (kein stiller Rückfall). Ein relativer
    Pfad gilt relativ zum aktuellen Verzeichnis.
      paneldeck.exe --data-dir "D:\PaneldeckDaten"

  --profile <name>
    Nutzt „<Datenordner>\profiles\<name>“ und läuft als eigene Instanz neben der
    normalen. Namen: 1–40 Zeichen aus A–Z, a–z, 0–9, _ und -.
    Kombinierbar mit --data-dir oder PANELDECK_DATA.
      paneldeck.exe --profile arbeit
      paneldeck.exe --data-dir "D:\PaneldeckDaten" --profile kunde-a

  --backup-dir <ordner>
    Schreibt Datenbank-Sicherungen für diesen Start nach <ordner> (die Ordnerwahl unter
    Einstellungen > Daten & Sicherung ist dann gesperrt).
      paneldeck.exe --backup-dir "E:\Sicherungen\Paneldeck"

  --lang en|de
    Sprache von Oberfläche und Meldungen für diesen Start; die gespeicherte bleibt.
      paneldeck.exe --lang en

  --reset-settings
    Benennt settings.json in „settings.json.bak-<JJJJMMTT-HHMMSS>“ um und startet mit
    Standard-Einstellungen. Sitzungen, Prompts, Themes und Datenbank bleiben unberührt.
    Nicht möglich, solange Paneldeck mit demselben Datenordner läuft.
      paneldeck.exe --reset-settings
      paneldeck.exe --profile arbeit --reset-settings

  --version
    Gibt die Versionsnummer aus.
      paneldeck.exe --version

  --help, -h, -?
    Gibt diese Hilfe aus (in der Sprache von --lang bzw. der gespeicherten Einstellungen).
      paneldeck.exe --help > paneldeck-hilfe.txt

  mcp
    Startet den MCP-Server für KI-Agenten über stdin/stdout und spricht mit dem
    laufenden Paneldeck. Den Hub sucht er in dieser Reihenfolge: --data-dir/--profile,
    das Paneldeck-Terminal, aus dem er gestartet wurde, dann die Datenordner-Suche oben.
    Die Agenten-Schnittstelle muss unter Einstellungen > Agenten eingeschaltet sein.
      claude mcp add paneldeck --scope user -- "C:\Program Files\Paneldeck\paneldeck.exe" mcp
      paneldeck.exe mcp --profile arbeit
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_option_in_both_languages() {
        let opts = ["--open", "--claude", "--tray", "--no-tray", "--data-dir", "--profile", "--backup-dir", "--lang", "--reset-settings", "--version", "--help", "-h", "-?", "mcp", "PANELDECK_DATA", "%APPDATA%", "paneldeck-data"];
        for l in [Lang::En, Lang::De] {
            let t = text(l);
            assert!(t.contains(env!("CARGO_PKG_VERSION")));
            for o in opts {
                assert!(t.contains(o), "{o} fehlt ({l:?})");
                // Überblick + ausführliche Erklärung + mindestens ein Beispiel
                if o.starts_with("--") || o == "mcp" {
                    assert!(t.matches(&format!("\n  {o}")).count() >= 2, "{o}: Überblick und Erklärung ({l:?})");
                    let example = t.lines().map(str::trim).any(|s| s.starts_with("paneldeck.exe ") && s.split_whitespace().any(|w| w == o));
                    assert!(example, "{o}: Beispiel ({l:?})");
                }
            }
        }
        // jedes Beispiel ist eine gültige Kommandozeile
        for l in [Lang::En, Lang::De] {
            for line in text(l).lines().map(str::trim).filter(|s| s.starts_with("paneldeck.exe ") && !s.contains('[')) {
                let mut args = vec!["paneldeck.exe".to_string()];
                let rest = line.trim_start_matches("paneldeck.exe ");
                let rest = rest.split(" > ").next().unwrap_or(rest);
                let mut cur = String::new();
                let mut quoted = false;
                for ch in rest.chars() {
                    match ch {
                        '"' => quoted = !quoted,
                        ' ' if !quoted => {
                            if !cur.is_empty() {
                                args.push(std::mem::take(&mut cur));
                            }
                        }
                        c => cur.push(c),
                    }
                }
                if !cur.is_empty() {
                    args.push(cur);
                }
                assert!(crate::cli::parse(&args, &|s| std::path::PathBuf::from(s), &|_| Some(true)).is_ok(), "{line}");
            }
        }
        assert_ne!(text(Lang::En), text(Lang::De));
    }
}
