//! Kommandozeilenargumente (handgeschrieben, ohne zusätzliche Crate).
//!
//! Start: `--open <ordner> [--claude]`, `--tray`, `--no-tray`, `--data-dir <ordner>`, `--profile <name>`,
//! `--backup-dir <ordner>`, `--lang en|de`, `--reset-settings`, `--no-update-check`, `--version`, `--help`/`-h`/`-?`.
//! Agenten-Brücke: `mcp [--data-dir <ordner>] [--profile <name>] [--lang en|de]`.
//! `--hook` wird vorher in `main` behandelt (eigene Argumente von Claude Code) und kommt hier nicht an;
//! `--uninstall-hooks` (Deinstallationsprogramm) ist ein bekanntes Flag.

use crate::state::Lang;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct Cli {
    pub open: Option<String>,
    pub claude: bool,
    /// `--edit <datei>` (#54, 3.0.0): Datei im Editor-Reiter öffnen, bereits absolut.
    pub edit: Option<String>,
    pub tray: bool,
    /// Unterbefehl `mcp` (stdio-Brücke für Agenten).
    pub mcp: bool,
    /// `--data-dir`, bereits absolut.
    pub data_dir: Option<PathBuf>,
    /// `--backup-dir`, bereits absolut (nur für diesen Start, wird nicht gespeichert).
    pub backup_dir: Option<PathBuf>,
    pub profile: Option<String>,
    /// `--lang` (nur für diesen Start, wird nicht gespeichert).
    pub lang: Option<Lang>,
    pub no_tray: bool,
    /// `--no-update-check`: keine automatische Update-Prüfung für diesen Start (die manuelle geht weiter).
    pub no_update_check: bool,
    pub reset_settings: bool,
    pub version: bool,
    pub help: bool,
    pub uninstall_hooks: bool,
}

/// Fehler beim Lesen der Kommandozeile (Exit-Code 2).
#[derive(Debug, Clone, PartialEq)]
pub enum CliError {
    Unknown(String),
    MissingValue(String),
    BadProfile(String),
    BadLang(String),
    Unexpected(String),
    NotWithMcp(String),
    /// `--edit` zusammen mit `--open`/`--claude` oder einem Ordner ohne Option.
    EditWithOpen,
}

impl CliError {
    pub fn message(&self, l: Lang) -> String {
        let body = match self {
            CliError::Unknown(o) => format!("{} {}", l.t("Unbekannte Option:", "Unknown option:"), o),
            CliError::MissingValue(o) => format!("{} {}", l.t("Wert fehlt für", "Missing value for"), o),
            CliError::BadProfile(n) => format!(
                "{} {} – {}",
                l.t("Ungültiger Profilname:", "Invalid profile name:"),
                l.q(n),
                l.t(
                    "erlaubt sind 1–40 Zeichen aus A–Z, a–z, 0–9, _ und -, keine Windows-Gerätenamen (CON, NUL, …)",
                    "allowed are 1–40 characters from A–Z, a–z, 0–9, _ and -, no Windows device names (CON, NUL, …)"
                )
            ),
            CliError::BadLang(v) => format!("{} {} – {}", l.t("Ungültige Sprache:", "Invalid language:"), l.q(v), l.t("erlaubt sind en und de", "allowed are en and de")),
            CliError::Unexpected(a) => format!("{} {}", l.t("Unerwartetes Argument:", "Unexpected argument:"), a),
            CliError::NotWithMcp(o) => format!("{} {}", l.t("Mit mcp sind nur --data-dir, --profile, --lang und --help erlaubt, nicht:", "With mcp only --data-dir, --profile, --lang and --help are allowed, not:"), o),
            CliError::EditWithOpen => l.t("--edit geht nicht zusammen mit --open, --claude oder einem Ordner.", "--edit cannot be combined with --open, --claude or a folder.").to_string(),
        };
        format!("Paneldeck: {body}\n{}", l.t("Hilfe: paneldeck.exe --help", "Help: paneldeck.exe --help"))
    }
}

/// Erlaubte Profilnamen: `[A-Za-z0-9_-]{1,40}`, keine reservierten Windows-Gerätenamen.
pub fn valid_profile(name: &str) -> bool {
    if name.is_empty() || name.len() > 40 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return false;
    }
    let up = name.to_ascii_uppercase();
    let reserved = matches!(up.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((up.starts_with("COM") || up.starts_with("LPT")) && up.len() == 4 && up.as_bytes()[3].is_ascii_digit());
    !reserved
}

fn parse_lang(v: &str) -> Option<Lang> {
    match v.to_ascii_lowercase().as_str() {
        "en" => Some(Lang::En),
        "de" => Some(Lang::De),
        _ => None,
    }
}

/// `--open "C:\"` (Laufwerkswurzel): Windows liest `\"` als maskiertes Anführungszeichen, der Rest der
/// Kommandozeile landet im selben Argument (`C:" --claude`). Hier wieder auftrennen: Pfad + übrige Argumente.
fn split_quoted(v: &str) -> (String, Vec<String>) {
    match v.split_once('"') {
        Some((path, rest)) => {
            let path = format!("{path}\\").replace("\\\\", "\\");
            (path, split_args(rest))
        }
        None => (v.to_string(), Vec::new()),
    }
}

/// Argumente nach den Windows-Regeln zerlegen (wie `CommandLineToArgvW`: Leerraum trennt außerhalb von
/// Anführungszeichen, `"` schaltet um, `2n` Backslashes + `"` → `n` Backslashes + Umschalten, `2n+1` → `n` + `"`),
/// mit einer Ausnahme für Pfade: ein einzelner Backslash vor einem schließenden `"` am Argumentende (`"C:\"`,
/// `"D:\daten\"`) bleibt ein Backslash. So kommen `--open "C:\" --data-dir "D:\my data"` richtig an.
pub fn split_args(s: &str) -> Vec<String> {
    let ch: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut i = 0;
    while i < ch.len() {
        let c = ch[i];
        if c == '\\' {
            let mut n = 0;
            while i < ch.len() && ch[i] == '\\' {
                n += 1;
                i += 1;
            }
            if i < ch.len() && ch[i] == '"' {
                let at_end = ch.get(i + 1).is_none_or(|c| c.is_whitespace());
                if quoted && n % 2 == 1 && at_end {
                    // Pfadende: `"C:\"` → `C:\`, Anführungszeichen schließt
                    cur.extend(std::iter::repeat_n('\\', n));
                    quoted = false;
                    i += 1;
                } else {
                    cur.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 {
                        cur.push('"');
                        i += 1;
                    }
                }
            } else {
                cur.extend(std::iter::repeat_n('\\', n));
            }
            started = true;
            continue;
        }
        if c == '"' {
            if quoted && ch.get(i + 1) == Some(&'"') {
                // `""` innerhalb von Anführungszeichen = ein `"`
                cur.push('"');
                i += 2;
                continue;
            }
            quoted = !quoted;
            started = true;
        } else if c.is_whitespace() && !quoted {
            if started {
                out.push(std::mem::take(&mut cur));
                started = false;
            }
        } else {
            cur.push(c);
            started = true;
        }
        i += 1;
    }
    if started {
        out.push(cur);
    }
    out
}

/// Ganze Kommandozeile (`GetCommandLineW`) zerlegen: der Programmname folgt einfacheren Regeln (nur `"`).
pub fn split_command_line(s: &str) -> Vec<String> {
    let s = s.trim_start();
    let (prog, rest) = if let Some(r) = s.strip_prefix('"') {
        match r.find('"') {
            Some(e) => (r[..e].to_string(), &r[e + 1..]),
            None => (r.to_string(), ""),
        }
    } else {
        match s.find(char::is_whitespace) {
            Some(e) => (s[..e].to_string(), &s[e..]),
            None => (s.to_string(), ""),
        }
    };
    std::iter::once(prog).chain(split_args(rest)).collect()
}

/// `--hook`-Aufruf von Claude Code: wird vor dem Parser und unabhängig von weiteren Argumenten behandelt.
pub fn is_hook_invocation(args: &[String]) -> bool {
    args.iter().any(|a| a == "--hook")
}

/// `--lang` aus den rohen Argumenten (auch wenn die Kommandozeile sonst fehlerhaft ist) – für Fehlermeldungen.
pub fn scan_lang(args: &[String]) -> Option<Lang> {
    let mut found = None;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "--lang" {
            if let Some(l) = it.next().and_then(|v| parse_lang(v)) {
                found = Some(l);
            }
        } else if let Some(v) = a.strip_prefix("--lang=") {
            if let Some(l) = parse_lang(v) {
                found = Some(l);
            }
        }
    }
    found
}

/// Ordner/Datei auf der Festplatte: `Some(true)` = Ordner, `Some(false)` = Datei, `None` = nicht vorhanden.
pub fn fs_kind(p: &std::path::Path) -> Option<bool> {
    std::fs::metadata(p).ok().map(|m| m.is_dir())
}

/// Liest die Argumente (`args[0]` = Programm). `abs` macht relative Pfade absolut, `kind` prüft, ob ein Pfad ein
/// Ordner oder eine Datei ist (beides in Tests eingespeist). Genau ein Argument ohne Option, das ein vorhandener
/// Ordner ist (eine Datei → ihr Ordner), gilt als `--open <pfad>` (Ordner auf die .exe gezogen, alte Verknüpfungen).
pub fn parse(args: &[String], abs: &dyn Fn(&str) -> PathBuf, kind: &dyn Fn(&std::path::Path) -> Option<bool>) -> Result<Cli, CliError> {
    let mut c = Cli::default();
    let mut positional = false;
    let mut open_opt = false;
    let mut rest: std::collections::VecDeque<String> = args.iter().skip(1).cloned().collect();
    if rest.front().map(|s| s.as_str()) == Some("mcp") {
        rest.pop_front();
        c.mcp = true;
    }
    while let Some(raw) = rest.pop_front() {
        // `--option=wert` ist gleichwertig zu `--option wert`.
        let (opt, inline) = match raw.split_once('=') {
            Some((o, v)) if o.starts_with("--") => (o.to_string(), Some(v.to_string())),
            _ => (raw.clone(), None),
        };
        let takes_value = matches!(opt.as_str(), "--open" | "--edit" | "--data-dir" | "--backup-dir" | "--profile" | "--lang");
        if inline.is_some() && !takes_value {
            return Err(CliError::Unknown(raw));
        }
        let value = |rest: &mut std::collections::VecDeque<String>| -> Result<String, CliError> {
            let v = match inline.clone() {
                Some(v) => v,
                None => match rest.front() {
                    // Eine folgende Option ist kein Wert („--data-dir --tray“).
                    Some(n) if !(n.starts_with("--") && n.len() > 2) => rest.pop_front().unwrap_or_default(),
                    _ => return Err(CliError::MissingValue(opt.clone())),
                },
            };
            if v.trim().is_empty() {
                return Err(CliError::MissingValue(opt.clone()));
            }
            Ok(v)
        };
        if c.mcp && !matches!(opt.as_str(), "--data-dir" | "--profile" | "--lang" | "--help" | "-h" | "-?" | "/?") {
            return Err(if opt.starts_with('-') { CliError::NotWithMcp(raw) } else { CliError::Unexpected(raw) });
        }
        match opt.as_str() {
            "--open" | "--edit" | "--data-dir" | "--backup-dir" => {
                let (v, more) = split_quoted(&value(&mut rest)?);
                for m in more.into_iter().rev() {
                    rest.push_front(m);
                }
                match opt.as_str() {
                    "--edit" => {
                        if c.edit.is_some() {
                            return Err(CliError::Unexpected(raw));
                        }
                        // Wie --open relativ zum Ordner dieses Aufrufs (die laufende Instanz hat einen anderen). Linux-Pfade
                        // (WSL, `~`) bleiben; der Hub löst sie nach der Sitzung auf, aus der der Aufruf kam (#392).
                        c.edit = Some(if crate::wsl::looks_linux(v.trim()) { v } else { abs(&v).display().to_string() });
                    }
                    "--open" => {
                        if positional {
                            return Err(CliError::Unexpected(raw));
                        }
                        open_opt = true;
                        // Relativ zum Ordner dieses Aufrufs, nicht zu dem des laufenden Hubs, an den `--open`
                        // weitergereicht wird (#221). Linux-Pfade (WSL) bleiben unverändert.
                        c.open = Some(if crate::wsl::looks_linux(v.trim()) { v } else { abs(&v).display().to_string() })
                    }
                    "--data-dir" => c.data_dir = Some(abs(&v)),
                    _ => c.backup_dir = Some(abs(&v)),
                }
            }
            "--profile" => {
                let v = value(&mut rest)?;
                if !valid_profile(&v) {
                    return Err(CliError::BadProfile(v));
                }
                c.profile = Some(v);
            }
            "--lang" => {
                let v = value(&mut rest)?;
                c.lang = Some(parse_lang(&v).ok_or(CliError::BadLang(v))?);
            }
            "--claude" => c.claude = true,
            "--tray" => c.tray = true,
            "--no-tray" => c.no_tray = true,
            "--no-update-check" => c.no_update_check = true,
            "--reset-settings" => c.reset_settings = true,
            "--version" => c.version = true,
            "--help" | "-h" | "-?" | "/?" => c.help = true,
            "--uninstall-hooks" => c.uninstall_hooks = true,
            o if o.starts_with('-') => return Err(CliError::Unknown(raw)),
            _ => {
                if positional || open_opt || c.mcp {
                    return Err(CliError::Unexpected(raw));
                }
                let (v, more) = split_quoted(&raw);
                let p = abs(&v);
                let dir = match kind(&p) {
                    Some(true) => p,
                    Some(false) => p.parent().map(|d| d.to_path_buf()).ok_or_else(|| CliError::Unexpected(raw.clone()))?,
                    None => return Err(CliError::Unexpected(raw)),
                };
                for m in more.into_iter().rev() {
                    rest.push_front(m);
                }
                positional = true;
                c.open = Some(dir.display().to_string());
            }
        }
    }
    if c.edit.is_some() && (c.open.is_some() || c.claude) {
        return Err(CliError::EditWithOpen);
    }
    Ok(c)
}

impl Cli {
    /// Startoptionen, die Einstellungen nur für diesen Start überschreiben (für Info-Seite und Diagnose).
    pub fn overrides(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(l) = self.lang {
            v.push(format!("--lang {}", if l == Lang::De { "de" } else { "en" }));
        }
        if let Some(b) = &self.backup_dir {
            v.push(format!("--backup-dir {}", b.display()));
        }
        if self.no_tray {
            v.push("--no-tray".to_string());
        }
        if self.no_update_check {
            v.push("--no-update-check".to_string());
        }
        v
    }
}

static CLI: OnceLock<Cli> = OnceLock::new();

/// Relativen Pfad gegen das aktuelle Verzeichnis auflösen.
pub fn abs_from_cwd(s: &str) -> PathBuf {
    std::path::absolute(s).unwrap_or_else(|_| PathBuf::from(s))
}

/// Ergebnis von `parse` für den Rest des Prozesses ablegen (einmal, in `main`).
pub fn set(c: Cli) {
    let _ = CLI.set(c);
}

pub fn get() -> &'static Cli {
    CLI.get_or_init(Cli::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aktuelles Verzeichnis, ein vorhandener Ordner und eine Datei darin – je Plattform mit echten absoluten Pfaden.
    #[cfg(windows)]
    const CWD: &str = r"C:\cwd";
    #[cfg(not(windows))]
    const CWD: &str = "/cwd";
    #[cfg(windows)]
    const APP: &str = r"C:\Dev\app";
    #[cfg(not(windows))]
    const APP: &str = "/dev/app";
    #[cfg(windows)]
    const MAIN_RS: &str = r"C:\Dev\app\main.rs";
    #[cfg(not(windows))]
    const MAIN_RS: &str = "/dev/app/main.rs";

    fn p(args: &[&str]) -> Result<Cli, CliError> {
        let a: Vec<String> = std::iter::once("x.exe").chain(args.iter().copied()).map(|s| s.to_string()).collect();
        parse(
            &a,
            &|s| {
                let p = std::path::Path::new(s);
                if p.is_absolute() { p.to_path_buf() } else { PathBuf::from(CWD).join(s) }
            },
            &|p| match p.to_string_lossy().as_ref() {
                "C:\\Dev\\app" | "C:\\cwd\\rel" | "C:\\" | "/dev/app" | "/cwd/rel" => Some(true),
                "C:\\Dev\\app\\main.rs" | "/dev/app/main.rs" => Some(false),
                _ => None,
            },
        )
    }

    #[cfg(windows)] // nur für die Tests mit Windows-Pfaden
    fn line(s: &str) -> Result<Cli, CliError> {
        let a = split_command_line(s);
        parse(&a, &|s| PathBuf::from(s), &|_| Some(true))
    }

    #[test]
    fn windows_tokenizer() {
        let v = |s: &str| split_command_line(s);
        assert_eq!(v(r#""C:\Program Files\P\paneldeck.exe" --open "C:\" --claude"#), vec![r"C:\Program Files\P\paneldeck.exe", "--open", r"C:\", "--claude"]);
        assert_eq!(v(r#"paneldeck.exe --open "C:\" --data-dir "D:\my data""#), vec!["paneldeck.exe", "--open", r"C:\", "--data-dir", r"D:\my data"]);
        assert_eq!(v(r#"x --data-dir "D:\dir with space\" --tray"#), vec!["x", "--data-dir", r"D:\dir with space\", "--tray"]);
        assert_eq!(v(r"x C:\a\b  "), vec!["x", r"C:\a\b"]);
    }

    #[cfg(windows)] // Windows-Pfade (Laufwerksbuchstaben, `\`)
    #[test]
    fn installer_and_registry_forms() {
        // Autostart, Kontextmenü (%1 / %V, auch Laufwerkswurzel) und Deinstallation, wie in installer/paneldeck.nsi
        let c = line(r#""C:\P\paneldeck.exe" --tray"#).unwrap();
        assert!(c.tray && c.open.is_none());
        let c = line(r#""C:\P\paneldeck.exe" --open "C:\Users\me\proj""#).unwrap();
        assert_eq!(c.open.as_deref(), Some(r"C:\Users\me\proj"));
        let c = line(r#""C:\P\paneldeck.exe" --open "C:\""#).unwrap();
        assert_eq!(c.open.as_deref(), Some(r"C:\"));
        let c = line(r#""C:\P\paneldeck.exe" --claude --open "C:\""#).unwrap();
        assert!(c.claude);
        assert_eq!(c.open.as_deref(), Some(r"C:\"));
        let c = line(r#""C:\P\paneldeck.exe" --claude --open "D:\My Projects\a b""#).unwrap();
        assert!(c.claude);
        assert_eq!(c.open.as_deref(), Some(r"D:\My Projects\a b"));
        assert!(line(r#""C:\P\paneldeck.exe" --uninstall-hooks"#).unwrap().uninstall_hooks);
        // dieselben Formen, wie std::env::args sie liefert (Laufwerkswurzel: `C:" --claude` in einem Argument)
        let c = p(&["--claude", "--open", "C:\\\""]).unwrap();
        assert_eq!(c.open.as_deref(), Some("C:\\"));
        assert!(c.claude);
    }

    #[test]
    fn hook_invocation_is_detected_before_parsing() {
        let a: Vec<String> = ["x.exe", "--hook", "--whatever", "SessionStart", "stray"].iter().map(|s| s.to_string()).collect();
        assert!(is_hook_invocation(&a));
        // der Parser würde diese Argumente ablehnen – deshalb behandelt `main` `--hook` vorher
        assert!(parse(&a, &|s| PathBuf::from(s), &|_| None).is_err());
        assert!(!is_hook_invocation(&["x.exe".to_string(), "--tray".to_string()]));
    }

    #[cfg(windows)] // Windows-Pfade (Laufwerksbuchstaben, `\`)
    #[test]
    fn positional_folder_means_open() {
        assert_eq!(p(&["C:\\Dev\\app"]).unwrap().open.as_deref(), Some("C:\\Dev\\app"));
        assert_eq!(p(&["rel"]).unwrap().open.as_deref(), Some("C:\\cwd\\rel"));
        // Datei → ihr Ordner
        assert_eq!(p(&["C:\\Dev\\app\\main.rs"]).unwrap().open.as_deref(), Some("C:\\Dev\\app"));
        let c = p(&["C:\\Dev\\app", "--claude"]).unwrap();
        assert!(c.claude);
        // Laufwerkswurzel per Ziehen
        assert_eq!(p(&["C:\\\" --tray"]).unwrap().open.as_deref(), Some("C:\\"));
        // nicht vorhanden, zwei Stück, zusammen mit --open oder mcp: Fehler
        assert!(matches!(p(&["C:\\nope"]), Err(CliError::Unexpected(_))));
        assert!(matches!(p(&["C:\\Dev\\app", "rel"]), Err(CliError::Unexpected(_))));
        assert!(matches!(p(&["C:\\Dev\\app", "--open", "rel"]), Err(CliError::Unexpected(_))));
        assert!(matches!(p(&["--open", "rel", "C:\\Dev\\app"]), Err(CliError::Unexpected(_))));
        assert!(matches!(p(&["mcp", "C:\\Dev\\app"]), Err(CliError::Unexpected(_))));
    }

    #[cfg(windows)] // Windows-Pfade (Laufwerksbuchstaben, `\`)
    #[test]
    fn a_relative_open_path_becomes_absolute_before_forwarding() {
        assert_eq!(p(&["--open", "rel"]).unwrap().open.as_deref(), Some("C:\\cwd\\rel"));
        assert_eq!(p(&["--open", "sub\\dir"]).unwrap().open.as_deref(), Some("C:\\cwd\\sub\\dir"));
        assert_eq!(p(&["--open=.\\x"]).unwrap().open.as_deref(), Some("C:\\cwd\\.\\x"));
        assert_eq!(p(&["--open", "C:\\Dev\\app"]).unwrap().open.as_deref(), Some("C:\\Dev\\app"));
        assert_eq!(p(&["--open", r"\\wsl.localhost\Ubuntu\home"]).unwrap().open.as_deref(), Some(r"\\wsl.localhost\Ubuntu\home"));
        assert_eq!(p(&["--open", "/home/u"]).unwrap().open.as_deref(), Some("/home/u"));
        // #392: auch --edit gibt Linux-Pfade unverändert weiter (der Hub löst sie nach der Sitzung des Aufrufs auf)
        assert_eq!(p(&["--edit", "/home/u/a.rs"]).unwrap().edit.as_deref(), Some("/home/u/a.rs"));
        assert_eq!(p(&["--edit", "~/a.rs"]).unwrap().edit.as_deref(), Some("~/a.rs"));
        assert_eq!(p(&["--edit", "rel.txt"]).unwrap().edit.as_deref(), Some("C:\\cwd\\rel.txt"));
        // echte Umgebung: relativ zum aktuellen Ordner dieses Prozesses
        let a: Vec<String> = ["x.exe", "--open", "rel"].iter().map(|s| s.to_string()).collect();
        let c = parse(&a, &|s| abs_from_cwd(s), &|_| None).unwrap();
        assert_eq!(c.open.map(PathBuf::from), Some(std::env::current_dir().unwrap().join("rel")));
    }

    #[test]
    fn lang_prescan() {
        let a = |v: &[&str]| -> Vec<String> { std::iter::once("x").chain(v.iter().copied()).map(|s| s.to_string()).collect() };
        assert_eq!(scan_lang(&a(&["--bogus", "--lang", "de"])), Some(Lang::De));
        assert_eq!(scan_lang(&a(&["--lang=DE", "--x"])), Some(Lang::De));
        assert_eq!(scan_lang(&a(&["--lang", "xx"])), None);
        assert_eq!(scan_lang(&a(&["--bogus"])), None);
    }

    #[cfg(windows)] // Windows-Pfade (Laufwerksbuchstaben, `\`)
    #[test]
    fn parses_existing_options() {
        let c = p(&["--open", "C:\\Dev\\app", "--claude", "--tray"]).unwrap();
        assert_eq!(c.open.as_deref(), Some("C:\\Dev\\app"));
        assert!(c.claude && c.tray);
        assert_eq!(p(&[]).unwrap(), Cli::default());
        // Laufwerkswurzel: `--open "C:\" --claude` kommt als ein Argument `C:" --claude`
        let c = p(&["--open", "C:\\\" --claude"]).unwrap();
        assert_eq!(c.open.as_deref(), Some("C:\\"));
        assert!(c.claude && !c.tray);
        let c = p(&["--open", "D:\\\" --tray --profile work"]).unwrap();
        assert_eq!(c.open.as_deref(), Some("D:\\"));
        assert!(c.tray);
        assert_eq!(c.profile.as_deref(), Some("work"));
    }

    #[cfg(windows)] // Windows-Pfade (Laufwerksbuchstaben, `\`)
    #[test]
    fn parses_new_options() {
        let c = p(&["--data-dir", "D:\\pd", "--backup-dir", "bk", "--profile", "work_1-a", "--lang", "DE", "--no-tray", "--reset-settings"]).unwrap();
        assert_eq!(c.data_dir, Some(PathBuf::from("D:\\pd")));
        assert_eq!(c.backup_dir, Some(PathBuf::from("C:\\cwd\\bk")));
        assert_eq!(c.profile.as_deref(), Some("work_1-a"));
        assert_eq!(c.lang, Some(Lang::De));
        assert!(c.no_tray && c.reset_settings && !c.help && !c.version);
        assert_eq!(c.overrides(), vec!["--lang de".to_string(), "--backup-dir C:\\cwd\\bk".to_string(), "--no-tray".to_string()]);
        let c = p(&["--data-dir=rel\\dir", "--lang=en"]).unwrap();
        assert_eq!(c.data_dir, Some(PathBuf::from("C:\\cwd\\rel\\dir")));
        assert_eq!(c.lang, Some(Lang::En));
        for h in ["--help", "-h", "-?", "/?"] {
            assert!(p(&[h]).unwrap().help, "{h}");
        }
        assert!(p(&["--version"]).unwrap().version);
        assert!(p(&["--uninstall-hooks"]).unwrap().uninstall_hooks);
        // Pfad mit abschließendem Backslash in Anführungszeichen
        let c = p(&["--data-dir", "E:\\\" --no-tray"]).unwrap();
        assert_eq!(c.data_dir, Some(PathBuf::from("E:\\")));
        assert!(c.no_tray);
    }

    #[test]
    fn no_update_check_option() {
        let c = p(&["--no-update-check"]).unwrap();
        assert!(c.no_update_check && !c.no_tray);
        assert_eq!(c.overrides(), vec!["--no-update-check".to_string()]);
        assert!(!p(&[]).unwrap().no_update_check, "Standard: Prüfung erlaubt");
        let c = p(&["--profile", "work", "--no-update-check", "--tray"]).unwrap();
        assert!(c.no_update_check && c.tray);
        // streng: kein Wert, keine Abwandlung, nicht mit mcp
        assert_eq!(p(&["--no-update-check=1"]), Err(CliError::Unknown("--no-update-check=1".into())));
        assert_eq!(p(&["--no-update-checks"]), Err(CliError::Unknown("--no-update-checks".into())));
        assert!(matches!(p(&["mcp", "--no-update-check"]), Err(CliError::NotWithMcp(_))));
    }

    #[cfg(windows)] // Windows-Pfade (Laufwerksbuchstaben, `\`)
    #[test]
    fn mcp_subcommand() {
        let c = p(&["mcp"]).unwrap();
        assert!(c.mcp && c.profile.is_none());
        let c = p(&["mcp", "--profile", "p1"]).unwrap();
        assert!(c.mcp);
        assert_eq!(c.profile.as_deref(), Some("p1"));
        let c = p(&["mcp", "--data-dir", "D:\\x", "--lang", "de"]).unwrap();
        assert_eq!(c.data_dir, Some(PathBuf::from("D:\\x")));
        assert!(matches!(p(&["mcp", "--tray"]), Err(CliError::NotWithMcp(_))));
        assert!(matches!(p(&["mcp", "extra"]), Err(CliError::Unexpected(_))));
        // `mcp` nur als erstes Argument
        assert!(matches!(p(&["--tray", "mcp"]), Err(CliError::Unexpected(_))));
    }

    #[test]
    fn errors() {
        assert_eq!(p(&["--bogus"]), Err(CliError::Unknown("--bogus".into())));
        assert_eq!(p(&["--tray=1"]), Err(CliError::Unknown("--tray=1".into())));
        assert_eq!(p(&["--data-dir"]), Err(CliError::MissingValue("--data-dir".into())));
        assert_eq!(p(&["--data-dir", "--tray"]), Err(CliError::MissingValue("--data-dir".into())));
        assert_eq!(p(&["--profile="]), Err(CliError::MissingValue("--profile".into())));
        assert_eq!(p(&["--open"]), Err(CliError::MissingValue("--open".into())));
        assert_eq!(p(&["--lang", "fr"]), Err(CliError::BadLang("fr".into())));
        assert_eq!(p(&["--profile", "a b"]), Err(CliError::BadProfile("a b".into())));
        assert_eq!(p(&["stray"]), Err(CliError::Unexpected("stray".into())));
        assert!(p(&["--tray", "mcp"]).is_err());
        for e in [CliError::Unknown("--x".into()), CliError::BadProfile("ä".into()), CliError::NotWithMcp("--tray".into())] {
            assert!(e.message(Lang::En).contains("--help") && e.message(Lang::De).contains("--help"));
            assert_ne!(e.message(Lang::En), e.message(Lang::De));
        }
    }

    #[test]
    fn edit_option_opens_a_file_in_the_editor() {
        // absolut, relativ zum Aufruf, mit Instanz-Argumenten und in der Form `--edit=…`
        let in_cwd = |name: &str| PathBuf::from(CWD).join(name).display().to_string();
        assert_eq!(p(&["--edit", MAIN_RS]).unwrap().edit.as_deref(), Some(MAIN_RS));
        assert_eq!(p(&["--edit", "rel.txt"]).unwrap().edit, Some(in_cwd("rel.txt")));
        #[cfg(windows)]
        assert_eq!(p(&["--edit", "rel.txt"]).unwrap().edit.as_deref(), Some(r"C:\cwd\rel.txt"));
        #[cfg(not(windows))]
        assert_eq!(p(&["--edit", "rel.txt"]).unwrap().edit.as_deref(), Some("/cwd/rel.txt"));
        let c = p(&["--profile", "work", "--edit=notes.md", "--lang", "de"]).unwrap();
        assert_eq!((c.edit, c.profile.as_deref(), c.open.as_deref()), (Some(in_cwd("notes.md")), Some("work"), None));
        // Fehler: ohne Wert, doppelt, mit --open/--claude/Ordner, mit mcp
        assert_eq!(p(&["--edit"]), Err(CliError::MissingValue("--edit".into())));
        assert!(matches!(p(&["--edit", "a", "--edit", "b"]), Err(CliError::Unexpected(_))));
        assert_eq!(p(&["--edit", "a", "--open", APP]), Err(CliError::EditWithOpen));
        assert_eq!(p(&["--edit", "a", "--claude"]), Err(CliError::EditWithOpen));
        assert_eq!(p(&[APP, "--edit", "a"]), Err(CliError::EditWithOpen));
        assert!(matches!(p(&["mcp", "--edit", "a"]), Err(CliError::NotWithMcp(_))));
        let m = CliError::EditWithOpen;
        assert!(m.message(Lang::En).contains("--edit") && m.message(Lang::De).contains("--edit") && m.message(Lang::En) != m.message(Lang::De));
    }

    #[cfg(windows)]
    #[test]
    fn edit_option_from_the_explorer_entry() {
        // So ruft der Explorer-Eintrag (`integration::edit_context_command`) Paneldeck auf.
        let c = line(r#""C:\Program Files\Paneldeck\paneldeck.exe" --profile work --edit "C:\Users\A B\x y.txt""#).unwrap();
        assert_eq!((c.edit.as_deref(), c.profile.as_deref()), (Some(r"C:\Users\A B\x y.txt"), Some("work")));
    }

    #[test]
    fn profile_names() {
        let long_ok = "x".repeat(40);
        let long_bad = "x".repeat(41);
        for ok in ["a", "work", "Work_2", "a-b", long_ok.as_str(), "COM", "com10", "CONsole"] {
            assert!(valid_profile(ok), "{ok}");
        }
        for bad in ["", "a b", "ä", "a.b", "a/b", "..", long_bad.as_str(), "CON", "nul", "Com1", "lpt9", "aux"] {
            assert!(!valid_profile(bad), "{bad}");
        }
    }
}
