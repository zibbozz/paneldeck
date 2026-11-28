//! Startbefehle für Integrationen (Autostart, Explorer-Kontextmenü, MCP-Registrierung) – reine Funktionen ohne
//! Registry oder Dateizugriff, damit sie sich testen lassen.
//!
//! Eine Instanz mit eigenem Datenordner (`--data-dir`) oder Profil (`--profile`) trägt diese Argumente in alle
//! Integrationen ein, die sie anlegt. Die Standardinstanz (keine Argumente) erzeugt byte-genau dieselben Einträge
//! wie bisher (`"<exe>" --tray`, MCP-Server `paneldeck` mit `["mcp"]`).
//!
//! Namen je Instanz:
//! - nur `--profile work` → Run-Wert `Paneldeck (work)`, MCP-Server `paneldeck-work`;
//! - nur `--data-dir D` → `Paneldeck (<h>)` / `paneldeck-<h>`, `<h>` = 8 Hex-Zeichen FNV-1a des normalisierten Ordners
//!   (Kleinbuchstaben, `/` → `\`, ohne abschließenden `\`), stabil über Neustarts und Versionen;
//! - beides → `Paneldeck (work-<h>)` / `paneldeck-work-<h>` (gleiche Profilnamen in verschiedenen Ordnern kollidieren nicht).
//!
//! Profilnamen werden für Namen kleingeschrieben (Ordner unter Windows: `Work` = `work`).

use std::path::Path;

/// Startargumente dieser Instanz (`--data-dir …`, `--profile …`); leer für die Standardinstanz.
/// Einzige Quelle für Integrationen (`integration`, Einstellungen → Agenten → Einrichten & Systemprompt).
pub fn instance_args() -> Vec<String> {
    crate::paths::instance_args()
}

/// Identität einer Instanz, aus ihren Startargumenten gelesen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Instance {
    pub data_dir: Option<String>,
    pub profile: Option<String>,
}

impl Instance {
    /// Aus den Instanz-Argumenten (`["--data-dir", "<abs>", "--profile", "work"]`, auch `--x=wert`) lesen.
    pub fn from_args(args: &[String]) -> Instance {
        let mut inst = Instance::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let (key, inline) = match a.split_once('=') {
                Some((k, v)) if k.starts_with("--") => (k, Some(v.to_string())),
                _ => (a.as_str(), None),
            };
            let slot = match key {
                "--data-dir" => &mut inst.data_dir,
                "--profile" => &mut inst.profile,
                _ => continue,
            };
            let v = inline.or_else(|| it.next().cloned());
            *slot = v.filter(|s| !s.trim().is_empty());
        }
        inst
    }

    /// Kennung für Namen: `None` für die Standardinstanz.
    fn tag(&self) -> Option<String> {
        let hash = self.data_dir.as_deref().map(dir_hash);
        let profile = self.profile.as_deref().map(|p| p.trim().to_lowercase());
        match (profile.as_deref(), hash) {
            (None, None) => None,
            (Some(p), None) => Some(p.to_string()),
            (None, Some(h)) => Some(h),
            (Some(p), Some(h)) => Some(format!("{p}-{h}")),
        }
    }
}

/// Pfad so normalisieren, wie Windows vergleicht: `/` → `\`, Präfix `\\?\` (bzw. `\\?\UNC\` → `\\`) weg,
/// Leerraum außen und abschließende Trenner weg, Kleinbuchstaben.
pub fn norm_path(p: &str) -> String {
    let s = p.trim().replace('/', "\\");
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    };
    s.trim_end_matches('\\').to_lowercase()
}

/// Stabiler Kurz-Hash eines Ordners (8 Hex-Zeichen, FNV-1a 64 Bit) über `norm_path` – unabhängig von
/// Groß-/Kleinschreibung, Schrägstrich-Richtung, `\\?\`-Präfix und abschließendem Trenner.
pub fn dir_hash(dir: &str) -> String {
    let norm = norm_path(dir);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in norm.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", (h >> 32) as u32 ^ h as u32)
}

/// Name des Werts unter `HKCU\…\Run`: `Paneldeck` bzw. `Paneldeck (<kennung>)`.
pub fn run_value_name(inst: &Instance) -> String {
    match inst.tag() {
        None => "Paneldeck".to_string(),
        Some(t) => format!("Paneldeck ({})", t.chars().filter(|c| !c.is_control()).collect::<String>()),
    }
}

/// Ob ein Run-Wert-Name zu einer Nicht-Standardinstanz gehört (`Paneldeck (…)`).
pub fn is_instance_run_value(name: &str) -> bool {
    name.starts_with("Paneldeck (") && name.ends_with(')')
}

/// Name des MCP-Servers: `paneldeck` bzw. `paneldeck-<kennung>`. Claude Code erlaubt nur `[A-Za-z0-9_-]`; andere
/// Zeichen werden zu `_`, und dann hängt ein Hash des Originals an, damit verschiedene Namen nicht zusammenfallen.
pub fn mcp_server_name(inst: &Instance) -> String {
    let Some(tag) = inst.tag() else { return "paneldeck".to_string() };
    let clean: String = tag.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if clean == tag {
        format!("paneldeck-{clean}")
    } else {
        format!("paneldeck-{clean}-{}", dir_hash(&tag))
    }
}

/// Ob ein `mcpServers`-Name von Paneldeck stammen kann (`paneldeck` oder `paneldeck-*`).
pub fn is_our_server_name(name: &str) -> bool {
    name == "paneldeck" || name.starts_with("paneldeck-")
}

/// Argumente des MCP-Servers: `["mcp", …Instanz-Argumente]` (`mcp` muss vorn stehen, so erkennt `main` den Modus).
pub fn mcp_args(instance_args: &[String]) -> Vec<String> {
    std::iter::once("mcp".to_string()).chain(instance_args.iter().cloned()).collect()
}

/// Zeichen, die ein Argument nie enthalten darf: `"` (in Windows-Pfaden unmöglich, würde aber jede Quotierung
/// aufbrechen), `%` (Explorer ersetzt `%1`/`%V`…, cmd.exe Umgebungsvariablen) und Steuerzeichen (Zeilenumbrüche).
pub fn check_arg(a: &str) -> Result<(), String> {
    check_chars(a, false)
}

fn check_chars(a: &str, program: bool) -> Result<(), String> {
    if let Some(c) = a.chars().find(|c| *c == '"' || *c == '%' || c.is_control()) {
        let shown = if c.is_control() { format!("U+{:04X}", c as u32) } else { c.to_string() };
        return Err(match (crate::state::lang(), program) {
            (crate::state::Lang::De, false) => format!("Ungültiges Zeichen {shown} im Startargument: {a}"),
            (crate::state::Lang::En, false) => format!("Invalid character {shown} in launch argument: {a}"),
            (crate::state::Lang::De, true) => format!("Ungültiges Zeichen {shown} im Programmpfad (Integrationen nicht möglich): {a}"),
            (crate::state::Lang::En, true) => format!("Invalid character {shown} in the program path (integrations not possible): {a}"),
        });
    }
    Ok(())
}

/// Ein Argument für eine Windows-Kommandozeile quotieren (Regeln von `CommandLineToArgvW`/MSVC). Quotiert wird bei
/// Leerraum, leerem Argument und bei Zeichen, die cmd.exe deuten würde (`&|<>^()!,;=`), damit die Zeile auch über
/// `cmd.exe /c` sicher ist. Backslashes vor dem schließenden `"` werden verdoppelt (`C:\` → `"C:\\"`).
pub fn quote_arg(a: &str) -> Result<String, String> {
    check_arg(a)?;
    let needs = a.is_empty() || a.chars().any(|c| c == ' ' || c == '\t' || "&|<>^()!,;=".contains(c));
    if !needs {
        return Ok(a.to_string());
    }
    let trailing = a.len() - a.trim_end_matches('\\').len();
    Ok(format!("\"{a}{}\"", "\\".repeat(trailing)))
}

/// Kommandozeile `"<exe>" <args…>`. Die .exe steht immer in Anführungszeichen (wie bisher); für den Programmnamen
/// gelten bei Windows keine Backslash-Regeln. Für ihn gilt dieselbe Zeichenregel wie für Argumente (kein `"`, `%`,
/// Steuerzeichen): `%` würde Explorer (`%1`…) und cmd.exe (Variablen) verwirren.
pub fn command_line(exe: &Path, args: &[String]) -> Result<String, String> {
    let e = exe.to_string_lossy();
    check_chars(&e, true)?;
    let mut s = format!("\"{e}\"");
    for a in args {
        s.push(' ');
        s.push_str(&quote_arg(a)?);
    }
    Ok(s)
}

/// Autostart-Befehl: `"<exe>" <instanz…> --tray`.
pub fn run_command(exe: &Path, instance_args: &[String]) -> Result<String, String> {
    Ok(format!("{} --tray", command_line(exe, instance_args)?))
}

/// Kontextmenü-Befehle: `"<exe>" <instanz…> [--claude] --open "%1"` (bzw. `%V`). Die Instanz-Argumente stehen vor
/// `--open`, weil `--open "C:\"` (Laufwerkswurzel) den Rest der Zeile verschluckt (siehe `cli::parse`).
pub fn context_command(exe: &Path, instance_args: &[String], claude: bool, placeholder: &str) -> Result<String, String> {
    Ok(format!("{}{} --open \"{placeholder}\"", command_line(exe, instance_args)?, if claude { " --claude" } else { "" }))
}

/// Befehl zum Registrieren in Claude Code: `claude mcp add --scope user <name> -- "<exe>" mcp <instanz…>`.
pub fn claude_add_command(exe: &Path, inst_args: &[String]) -> Result<String, String> {
    let name = mcp_server_name(&Instance::from_args(inst_args));
    Ok(format!("claude mcp add --scope user {} -- {}", quote_arg(&name)?, command_line(exe, &mcp_args(inst_args))?))
}

/// Argumentzeile für `cmd.exe` (per `raw_arg`), um `claude <args>` zu starten: `/d` (kein AutoRun aus der
/// Registry), `/v:off` (keine verzögerte Erweiterung mit `!`), `/c claude …`; jedes Argument über `quote_arg`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn claude_cmd_line(args: &[String]) -> Result<String, String> {
    let mut s = String::from("/d /v:off /c claude");
    for a in args {
        s.push(' ');
        s.push_str(&quote_arg(a)?);
    }
    Ok(s)
}

/// Argumente für `claude mcp add` (ohne `claude`): `mcp add --scope user <name> -- <exe> mcp <instanz…>`.
pub fn claude_add_args(exe: &Path, inst_args: &[String]) -> Vec<String> {
    let name = mcp_server_name(&Instance::from_args(inst_args));
    ["mcp", "add", "--scope", "user", &name, "--"].iter().map(|s| s.to_string()).chain(std::iter::once(exe.to_string_lossy().to_string())).chain(mcp_args(inst_args)).collect()
}

/// Erstes Token einer Kommandozeile (der Programmpfad), ohne Anführungszeichen.
pub fn program_of(cmdline: &str) -> &str {
    let s = cmdline.trim_start();
    match s.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => s.split([' ', '\t']).next().unwrap_or(""),
    }
}

/// Pfade vergleichen wie Windows (Groß-/Kleinschreibung und Schrägstrich egal).
pub fn same_path(a: &str, b: &str) -> bool {
    norm_path(a) == norm_path(b)
}

/// Befehle vergleichen (gespeicherter Registry-Wert gegen erwarteten): Groß-/Kleinschreibung egal, Leerraum außen egal.
pub fn same_command(stored: &str, expected: &str) -> bool {
    stored.trim().to_lowercase() == expected.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn instance_from_args() {
        assert!(Instance::from_args(&[]) == Instance::default());
        let i = Instance::from_args(&s(&["--data-dir", "D:\\x", "--profile", "work"]));
        assert_eq!(i.data_dir.as_deref(), Some("D:\\x"));
        assert_eq!(i.profile.as_deref(), Some("work"));
        let i = Instance::from_args(&s(&["--profile=work"]));
        assert_eq!(i.profile.as_deref(), Some("work"));
        assert!(i.data_dir.is_none());
        assert!(Instance::from_args(&s(&["--profile", ""])) == Instance::default());
        assert!(Instance::from_args(&s(&["--tray"])) == Instance::default());
    }

    #[test]
    fn names() {
        let d = Instance::default();
        assert_eq!(run_value_name(&d), "Paneldeck");
        assert_eq!(mcp_server_name(&d), "paneldeck");
        let w = Instance::from_args(&s(&["--profile", "work"]));
        assert_eq!(run_value_name(&w), "Paneldeck (work)");
        assert_eq!(mcp_server_name(&w), "paneldeck-work");
        let dd = Instance::from_args(&s(&["--data-dir", "D:\\Data\\Pd"]));
        let h = dir_hash("D:\\Data\\Pd");
        assert_eq!(h.len(), 8);
        assert_eq!(run_value_name(&dd), format!("Paneldeck ({h})"));
        assert_eq!(mcp_server_name(&dd), format!("paneldeck-{h}"));
        // stabil gegenüber Schreibweise
        assert_eq!(dir_hash("d:/data/pd/"), h);
        assert_ne!(dir_hash("D:\\Data\\Other"), h);
        let both = Instance::from_args(&s(&["--data-dir", "D:\\Data\\Pd", "--profile", "work"]));
        assert_eq!(mcp_server_name(&both), format!("paneldeck-work-{h}"));
        assert_eq!(run_value_name(&both), format!("Paneldeck (work-{h})"));
        // ungültige Zeichen für Claude Code
        let odd = Instance::from_args(&s(&["--profile", "my work"]));
        let n = mcp_server_name(&odd);
        assert!(n.starts_with("paneldeck-my_work-") && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{n}");
        assert_ne!(n, mcp_server_name(&Instance::from_args(&s(&["--profile", "my_work"]))));
        assert_eq!(run_value_name(&odd), "Paneldeck (my work)");
        assert!(is_instance_run_value("Paneldeck (work)"));
        assert!(!is_instance_run_value("Paneldeck"));
        assert!(!is_instance_run_value("PaneldeckX"));
        assert!(is_our_server_name("paneldeck") && is_our_server_name("paneldeck-work"));
        assert!(!is_our_server_name("paneldeckx") && !is_our_server_name("other"));
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_arg("work").unwrap(), "work");
        assert_eq!(quote_arg("").unwrap(), "\"\"");
        assert_eq!(quote_arg("C:\\My Data").unwrap(), "\"C:\\My Data\"");
        // abschließender Backslash vor dem schließenden Anführungszeichen wird verdoppelt
        assert_eq!(quote_arg("C:\\My Data\\").unwrap(), "\"C:\\My Data\\\\\"");
        assert_eq!(quote_arg("C:\\").unwrap(), "C:\\");
        assert_eq!(quote_arg("C:\\a&b").unwrap(), "\"C:\\a&b\"");
        assert_eq!(quote_arg("x^y").unwrap(), "\"x^y\"");
        assert!(quote_arg("a\"b").is_err());
        assert!(quote_arg("100%").is_err());
        assert!(quote_arg("a\nb").is_err());
        assert!(quote_arg("a\rb").is_err());
    }

    /// Nachbau von `CommandLineToArgvW` für die Argumente nach dem Programmnamen (Prüfung der Quotierung).
    fn split_args(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut chars = line.chars().peekable();
        loop {
            while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                chars.next();
            }
            if chars.peek().is_none() {
                return out;
            }
            let mut cur = String::new();
            let mut inq = false;
            let mut bs = 0usize;
            while let Some(&c) = chars.peek() {
                if c == '\\' {
                    bs += 1;
                    chars.next();
                    continue;
                }
                if c == '"' {
                    cur.push_str(&"\\".repeat(bs / 2));
                    if bs % 2 == 1 {
                        cur.push('"');
                    } else {
                        inq = !inq;
                    }
                    bs = 0;
                    chars.next();
                    continue;
                }
                cur.push_str(&"\\".repeat(bs));
                bs = 0;
                if !inq && (c == ' ' || c == '\t') {
                    break;
                }
                cur.push(c);
                chars.next();
            }
            cur.push_str(&"\\".repeat(bs));
            out.push(cur);
        }
    }

    #[test]
    fn quoting_roundtrip() {
        for a in ["plain", "C:\\My Data\\", "C:\\", "\\\\server\\share\\x y", "a b\\\\", "", "C:\\Tom & Jerry", "trail\\"] {
            let q = quote_arg(a).unwrap();
            assert_eq!(split_args(&q), vec![a.to_string()], "{a} -> {q}");
        }
        let args = s(&["--data-dir", "C:\\Users\\A B\\pd\\", "--profile", "work"]);
        let line = command_line(Path::new("C:\\Program Files\\Paneldeck\\paneldeck.exe"), &args).unwrap();
        let rest = line.split_once("paneldeck.exe\" ").unwrap().1;
        assert_eq!(split_args(rest), args);
    }

    #[test]
    fn commands_default_unchanged() {
        let exe = PathBuf::from("C:\\Users\\A B\\AppData\\Local\\Programs\\Paneldeck\\paneldeck.exe");
        let e = exe.display();
        assert_eq!(run_command(&exe, &[]).unwrap(), format!("\"{e}\" --tray"));
        assert_eq!(context_command(&exe, &[], false, "%1").unwrap(), format!("\"{e}\" --open \"%1\""));
        assert_eq!(context_command(&exe, &[], false, "%V").unwrap(), format!("\"{e}\" --open \"%V\""));
        assert_eq!(context_command(&exe, &[], true, "%1").unwrap(), format!("\"{e}\" --claude --open \"%1\""));
        assert_eq!(context_command(&exe, &[], true, "%V").unwrap(), format!("\"{e}\" --claude --open \"%V\""));
        assert_eq!(mcp_args(&[]), s(&["mcp"]));
        assert_eq!(claude_add_command(&exe, &[]).unwrap(), format!("claude mcp add --scope user paneldeck -- \"{e}\" mcp"));
    }

    #[test]
    fn commands_with_instance() {
        let exe = PathBuf::from("C:\\pd\\paneldeck.exe");
        let a = s(&["--profile", "work"]);
        assert_eq!(run_command(&exe, &a).unwrap(), "\"C:\\pd\\paneldeck.exe\" --profile work --tray");
        assert_eq!(context_command(&exe, &a, false, "%1").unwrap(), "\"C:\\pd\\paneldeck.exe\" --profile work --open \"%1\"");
        assert_eq!(context_command(&exe, &a, true, "%V").unwrap(), "\"C:\\pd\\paneldeck.exe\" --profile work --claude --open \"%V\"");
        assert_eq!(mcp_args(&a), s(&["mcp", "--profile", "work"]));
        assert_eq!(claude_add_command(&exe, &a).unwrap(), "claude mcp add --scope user paneldeck-work -- \"C:\\pd\\paneldeck.exe\" mcp --profile work");
        let d = s(&["--data-dir", "D:\\My Data\\"]);
        assert_eq!(run_command(&exe, &d).unwrap(), "\"C:\\pd\\paneldeck.exe\" --data-dir \"D:\\My Data\\\\\" --tray");
        assert!(run_command(&exe, &s(&["--data-dir", "D:\\100%"])).is_err());
        assert!(command_line(Path::new("C:\\a\"b.exe"), &[]).is_err());
    }

    #[test]
    fn profile_case_and_long_paths() {
        let a = Instance::from_args(&s(&["--profile", "Work"]));
        let b = Instance::from_args(&s(&["--profile", "work"]));
        assert_eq!(run_value_name(&a), "Paneldeck (work)");
        assert_eq!(mcp_server_name(&a), mcp_server_name(&b));
        assert_eq!(mcp_server_name(&a), "paneldeck-work");
        let h = dir_hash(r"D:\Data\Pd");
        assert_eq!(dir_hash(r"\\?\D:\Data\Pd\"), h);
        assert_eq!(dir_hash(r" D:\DATA\pd "), h);
        assert_eq!(dir_hash(r"\\?\UNC\srv\share\pd"), dir_hash(r"\\srv\share\pd"));
        assert!(same_path(r"\\?\C:\pd\paneldeck.exe", r"c:\PD\paneldeck.exe"));
    }

    #[test]
    fn exe_path_rule() {
        assert!(command_line(Path::new(r"C:\100%\paneldeck.exe"), &[]).is_err());
        assert!(run_command(Path::new(r"C:\100%\paneldeck.exe"), &[]).is_err());
        assert!(command_line(Path::new(r"C:\Tom & Jerry\paneldeck.exe"), &[]).is_ok());
    }

    #[test]
    fn claude_add_full_cmd_line() {
        let exe = PathBuf::from(r"C:\Program Files\Paneldeck\paneldeck.exe");
        let inst = s(&["--data-dir", r"D:\Tom & Jerry\pd data\", "--profile", "Work"]);
        let h = dir_hash(r"D:\Tom & Jerry\pd data");
        let line = claude_cmd_line(&claude_add_args(&exe, &inst)).unwrap();
        assert_eq!(
            line,
            format!(r#"/d /v:off /c claude mcp add --scope user paneldeck-work-{h} -- "C:\Program Files\Paneldeck\paneldeck.exe" mcp --data-dir "D:\Tom & Jerry\pd data\\" --profile Work"#)
        );
        // was cmd.exe nach `/c` an claude weiterreicht, zerlegt sich wieder in genau die Argumente
        let after = line.strip_prefix("/d /v:off /c claude ").unwrap();
        assert_eq!(split_args(after), claude_add_args(&exe, &inst));
        // Standardinstanz
        assert_eq!(claude_cmd_line(&claude_add_args(&exe, &[])).unwrap(), r#"/d /v:off /c claude mcp add --scope user paneldeck -- "C:\Program Files\Paneldeck\paneldeck.exe" mcp"#);
        assert!(claude_cmd_line(&s(&["mcp", "remove", "100%"])).is_err());
    }

    #[test]
    fn program_and_compare() {
        assert_eq!(program_of("\"C:\\pd\\paneldeck.exe\" --profile work --tray"), "C:\\pd\\paneldeck.exe");
        assert_eq!(program_of("C:\\pd\\paneldeck.exe --tray"), "C:\\pd\\paneldeck.exe");
        assert!(same_path("C:/PD/Paneldeck.exe", "c:\\pd\\paneldeck.exe"));
        assert!(same_command("\"C:\\PD\\paneldeck.exe\" --tray ", "\"c:\\pd\\paneldeck.exe\" --tray"));
        assert!(!same_command("\"C:\\pd\\paneldeck.exe\" --tray", "\"C:\\pd\\paneldeck.exe\" --profile work --tray"));
    }
}
