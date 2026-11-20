//! Shell-Integration für zsh und fish (#57) und Erkennung der Login-Shell von WSL-Distributionen.
//!
//! - **zsh:** Paneldeck setzt `ZDOTDIR` auf einen eigenen Ordner im Datenordner (`shell-integration/zsh`). Dessen
//!   `.zshenv` stellt sofort das `ZDOTDIR` des Nutzers wieder her (aus `PANELDECK_ZSH_ZDOTDIR`, sonst ungesetzt = `$HOME`)
//!   und lädt dessen `.zshenv`; danach liest zsh `.zprofile`/`.zshrc`/`.zlogin` des Nutzers wie gewohnt. Für interaktive
//!   Shells kommen `precmd`/`preexec`-Hooks dazu. Die Dateien des Nutzers werden nie verändert.
//! - **fish:** `--init-command 'source $PANELDECK_FISH_INIT'` (läuft nach `config.fish`), Ereignis-Funktionen für
//!   `fish_prompt`, `fish_preexec`, `fish_postexec`.
//! - **WSL:** Die Login-Shell der Distribution (`getent passwd`) wird im Hintergrund ermittelt und je Programmstart im
//!   Speicher gehalten. Bis sie bekannt ist (oder wenn die Abfrage scheitert), startet die Sitzung sofort mit der
//!   Integration für bash **und** zsh (beides nur Umgebungsvariablen, die andere Shells nicht beachten).
//!
//! Gesendet wird wie bei bash: `OSC 133;D;<exit>` (Befehl beendet), `OSC 9;9;"<ordner>"`, `OSC 133;A` (Prompt) und
//! `OSC 133;C` (Befehl startet, für die Dauer). Plattformneutral: unter Linux/macOS gilt dasselbe für zsh/fish als
//! Programm (zsh ist die Standard-Shell von macOS).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Art der Shell-Integration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Pwsh,
    Cmd,
    Bash,
    Zsh,
    Fish,
}

impl Flavor {
    /// Integration einer Unix-Shell nach ihrem Dateinamen (`/usr/bin/zsh`, `-zsh`, `zsh.exe`).
    pub fn of_unix_shell(path: &str) -> Option<Flavor> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_ascii_lowercase();
        let name = name.strip_prefix('-').unwrap_or(&name);
        let name = name.strip_suffix(".exe").unwrap_or(name);
        match name {
            "bash" => Some(Flavor::Bash),
            "zsh" => Some(Flavor::Zsh),
            "fish" => Some(Flavor::Fish),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ Login-Shell (WSL)

/// Was über die Login-Shell einer WSL-Distribution bekannt ist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    /// Abfrage läuft.
    Pending,
    /// Pfad der Login-Shell (geprüft, siehe [`valid_shell_path`]).
    Known(String),
    /// Abfrage gescheitert (Zeitlimit, kein `getent`, unlesbare Ausgabe): Rückfall auf die bash-Integration.
    Failed,
}

fn cache() -> &'static Mutex<HashMap<String, Login>> {
    static C: OnceLock<Mutex<HashMap<String, Login>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Bekannter Stand der Login-Shell von `distro` (`""` = Standard-Distribution), ohne eine Abfrage zu starten.
pub fn wsl_login(distro: &str) -> Option<Login> {
    cache().lock().ok().and_then(|c| c.get(distro).cloned())
}

/// Wie [`wsl_login`]; ist noch nichts bekannt, startet die Abfrage im Hintergrund (Ergebnis: [`Login::Pending`]).
pub fn ensure_wsl_login(distro: &str) -> Login {
    {
        let Ok(mut c) = cache().lock() else { return Login::Failed };
        if let Some(l) = c.get(distro) {
            return l.clone();
        }
        c.insert(distro.to_string(), Login::Pending);
    }
    let d = distro.to_string();
    let spawned = std::thread::Builder::new().name("wsl-login-shell".into()).spawn(move || {
        let r = crate::wsl::login_shell(&d).map(Login::Known).unwrap_or(Login::Failed);
        if let Ok(mut c) = cache().lock() {
            c.insert(d, r);
        }
    });
    if spawned.is_err() {
        set_login(distro, Login::Failed);
        return Login::Failed;
    }
    Login::Pending
}

pub(crate) fn set_login(distro: &str, l: Login) {
    if let Ok(mut c) = cache().lock() {
        c.insert(distro.to_string(), l);
    }
}

/// Beim Programmstart: Login-Shells der schon laufenden Distributionen ermitteln (im Hintergrund; startet keine VM).
/// Andere Distributionen fragt Paneldeck erst, wenn eine Sitzung darin startet.
pub fn detect_running_wsl() {
    if !cfg!(windows) {
        return;
    }
    let _ = std::thread::Builder::new().name("wsl-running".into()).spawn(|| {
        for d in crate::wsl::running_distros().unwrap_or_default() {
            if !crate::wsl::is_hidden_distro(&d) {
                ensure_wsl_login(&d);
            }
        }
    });
}

/// Nur plausible Shell-Pfade übernehmen: absolut, kurz, nur `A-Z a-z 0-9 / . _ - +` (er kann als `wsl.exe -e <pfad>`
/// auf die Kommandozeile kommen).
pub fn valid_shell_path(p: &str) -> bool {
    p.starts_with('/') && p.len() <= 256 && !p.contains("//") && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-+".contains(&b))
}

/// Login-Shell aus einer Zeile von `getent passwd <name>` (7. Feld).
pub fn parse_passwd_shell(out: &str) -> Option<String> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    let shell = line.split(':').nth(6)?.trim();
    valid_shell_path(shell).then(|| shell.to_string())
}

/// Text hinter dem Namen der Distribution in Auswahllisten: erkannte Shell, „Shell unbekannt“ (Abfrage gescheitert,
/// bash-Integration) oder „ohne Integration“ (andere Shell, z. B. nushell). Leer, solange die Abfrage läuft.
pub fn login_suffix(login: Option<&Login>, lang: crate::state::Lang) -> String {
    match login {
        Some(Login::Known(p)) => {
            let name = p.rsplit('/').next().unwrap_or(p);
            if Flavor::of_unix_shell(p).is_some() {
                format!(" ({name})")
            } else {
                format!(" ({name}, {})", lang.t("ohne Integration", "without integration"))
            }
        }
        Some(Login::Failed) => format!(" ({})", lang.t("Shell unbekannt", "shell unknown")),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ Hilfsdateien

/// Unterordner im Datenordner für die Hilfsdateien (beim Beenden gelöscht).
pub const DIR: &str = "shell-integration";
/// Variable mit dem ursprünglichen `ZDOTDIR` des Nutzers (nur gesetzt, wenn er eins hatte).
pub const ENV_ZDOTDIR: &str = "PANELDECK_ZSH_ZDOTDIR";
/// Variable mit dem Pfad der fish-Datei (für `--init-command`).
pub const ENV_FISH: &str = "PANELDECK_FISH_INIT";
/// Wert von `--init-command`: ohne Anführungszeichen (`wsl.exe` zerlegt seine Kommandozeile selbst); fish teilt den Wert
/// einer Variablen nie an Leerzeichen.
pub const FISH_INIT_COMMAND: &str = "source $PANELDECK_FISH_INIT";

/// `.zshenv` im eigenen `ZDOTDIR`. Exit-Status zuerst sichern (unsere Funktion steht vorn in `precmd_functions`).
pub const ZSH_ENV: &str = r#"# Paneldeck shell integration for zsh (#57). Generated by Paneldeck and removed when it exits - do not edit.
# Paneldeck starts zsh with ZDOTDIR pointing here. This file restores your own ZDOTDIR at once, so zsh then reads your
# .zshenv, .zprofile, .zshrc and .zlogin as usual; interactive shells get prompt hooks (OSC 133, OSC 9;9).
if [[ -n "${PANELDECK_ZSH_ZDOTDIR+X}" ]]; then
  builtin export ZDOTDIR="$PANELDECK_ZSH_ZDOTDIR"
  builtin unset PANELDECK_ZSH_ZDOTDIR
else
  builtin unset ZDOTDIR
fi
if [[ -r "${ZDOTDIR-$HOME}/.zshenv" ]]; then
  builtin source -- "${ZDOTDIR-$HOME}/.zshenv"
fi
if [[ -o interactive && -z "${__paneldeck_hooks-}" ]]; then
  typeset -g __paneldeck_hooks=1
  __paneldeck_precmd() {
    builtin local __pd_ec=$?
    builtin printf '\033]133;D;%s\007\033]9;9;"%s"\007\033]133;A\007' "$__pd_ec" "$PWD"
  }
  __paneldeck_preexec() {
    builtin printf '\033]133;C\007'
  }
  typeset -ga precmd_functions preexec_functions
  precmd_functions=(__paneldeck_precmd ${precmd_functions:#__paneldeck_precmd})
  preexec_functions=(${preexec_functions:#__paneldeck_preexec} __paneldeck_preexec)
fi
"#;

/// fish-Datei für `--init-command`.
pub const FISH_INIT: &str = r#"# Paneldeck shell integration for fish (#57). Generated by Paneldeck and removed when it exits - do not edit.
# Loaded with `fish --init-command` (after config.fish); your own files stay untouched.
set -e PANELDECK_FISH_INIT
if status is-interactive; and not set -q __paneldeck_hooks
    set -g __paneldeck_hooks 1
    function __paneldeck_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end
    function __paneldeck_prompt --on-event fish_prompt
        printf '\e]9;9;"%s"\a\e]133;A\a' "$PWD"
    end
    function __paneldeck_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end
end
"#;

fn zsh_dir(data: &Path) -> PathBuf {
    data.join(DIR).join("zsh")
}

fn fish_file(data: &Path) -> PathBuf {
    data.join(DIR).join("paneldeck.fish")
}

/// Ordner (0700 unter Unix) und Datei (0600) anlegen; nur schreiben, wenn sich der Inhalt unterscheidet.
fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent"))?;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Auch den Oberordner `shell-integration` schützen.
        let mut d = Some(dir);
        while let Some(x) = d {
            std::fs::set_permissions(x, std::fs::Permissions::from_mode(0o700))?;
            if x.file_name().is_some_and(|n| n == DIR) {
                break;
            }
            d = x.parent();
        }
    }
    if std::fs::read_to_string(path).ok().as_deref() != Some(content) {
        std::fs::write(path, content)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Hilfsdateien entfernen (beim Beenden). Laufende Shells brauchen sie nicht mehr: zsh hat `ZDOTDIR` beim Start
/// wiederhergestellt, fish hat die Datei geladen.
pub fn remove_files(data: &Path) {
    let _ = std::fs::remove_dir_all(data.join(DIR));
}

/// Pfad im Datenordner so, wie ihn die Shell sieht: unter WSL `/mnt/<lw>/…` (nur lokale Laufwerke; Netzlaufwerke
/// hängt WSL nicht ein, ein unerreichbares `ZDOTDIR` ließe zsh ohne die Dateien des Nutzers starten).
fn shell_path(p: &Path, wsl: bool) -> Option<String> {
    let s = p.to_string_lossy().to_string();
    if !wsl {
        return Some(s);
    }
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let b = s.as_bytes();
    if b.len() < 3 || b[1] != b':' || !b[0].is_ascii_alphabetic() || !matches!(b[2], b'\\' | b'/') {
        return None;
    }
    if cfg!(windows) && !matches!(crate::platform::fs::drive_type(b[0] as char), Some(2 | 3)) {
        return None;
    }
    let linux = crate::pathfmt::convert("wsl", s);
    // Kommt in eine Umgebungsvariable; Steuerzeichen nie.
    (!linux.chars().any(char::is_control)).then_some(linux)
}

// ------------------------------------------------------------------ Startplan

/// Zusätzliche Argumente/Variablen für die Integration einer Sitzung.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Angewandte Integration (`None` = keine). Für WSL mit noch unbekannter Login-Shell `Bash` (bash und zsh aktiv).
    pub flavor: Option<Flavor>,
}

/// Was eine WSL-Kommandozeile startet.
#[derive(Debug, PartialEq, Eq)]
pub enum WslTarget {
    /// Die Login-Shell (nur Optionen wie `-d`, `--cd`, `~`).
    Login,
    /// `-e <programm> …`: dieses Programm, Index des Programms in den Argumenten.
    Exec(usize),
    /// Ein Befehl über die Login-Shell (`wsl ls`, `wsl -- ls`) oder ein anderer Benutzer (`-u`): Integration wie bisher
    /// (bash) und zsh, ohne Abfrage.
    Other,
}

/// Kommandozeile von `wsl.exe` einordnen.
pub fn wsl_target(args: &[String]) -> WslTarget {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-e" | "--exec" => return if i + 1 < args.len() { WslTarget::Exec(i + 1) } else { WslTarget::Other },
            "-u" | "--user" | "--" => return WslTarget::Other,
            "-d" | "--distribution" | "--cd" | "--shell-type" | "--distribution-id" => i += 2,
            "~" => i += 1,
            _ if a.starts_with('-') => i += 1,
            _ => return WslTarget::Other,
        }
    }
    WslTarget::Login
}

/// Hat fish schon eigene Befehls- oder Skriptargumente (dann nichts hinzufügen)?
fn fish_has_command(args: &[String]) -> bool {
    args.iter().any(|a| matches!(a.as_str(), "-c" | "--command" | "-C" | "--init-command" | "-n" | "--no-execute") || a.starts_with("--command=") || a.starts_with("--init-command=") || !a.starts_with('-'))
}

/// Hat zsh eigene Befehle/Skripte oder läuft es ohne Startdateien (`-f`)?
fn zsh_has_command(args: &[String]) -> bool {
    args.iter().any(|a| a == "-c" || a == "-f" || a == "--no-rcs" || !a.starts_with('-') || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains(['c', 'f'])))
}

/// Variablen für zsh: eigenes `ZDOTDIR`, dazu das bisherige des Nutzers (aus den Variablen des Profils bzw. der Umgebung
/// des Hubs). `None`, wenn die Datei nicht geschrieben werden konnte oder WSL den Ordner nicht sieht.
fn zsh_env(data: &Path, wsl: bool, env: &[(String, String)]) -> Option<Vec<(String, String)>> {
    let dir = zsh_dir(data);
    let p = shell_path(&dir, wsl)?;
    write_private(&dir.join(".zshenv"), ZSH_ENV).ok()?;
    let mut v = vec![("ZDOTDIR".to_string(), p)];
    let user = env.iter().rev().find(|(k, _)| k == "ZDOTDIR").map(|(_, v)| v.clone()).or_else(|| if wsl { None } else { std::env::var("ZDOTDIR").ok() });
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        v.push((ENV_ZDOTDIR.to_string(), u));
    }
    Some(v)
}

/// Variable für fish (`--init-command` lädt die Datei).
fn fish_env(data: &Path, wsl: bool) -> Option<(String, String)> {
    let f = fish_file(data);
    let p = shell_path(&f, wsl)?;
    write_private(&f, FISH_INIT).ok()?;
    Some((ENV_FISH.to_string(), p))
}

/// bash-Integration (PROMPT_COMMAND/PS0).
fn bash_env() -> Vec<(String, String)> {
    vec![("PROMPT_COMMAND".into(), crate::shells::BASH_PC.into()), ("PS0".into(), crate::shells::BASH_PS0.into())]
}

/// Integration für eine Unix-Shell (`flavor`), die Paneldeck selbst mit `args` startet.
pub fn unix_plan(flavor: Option<Flavor>, args: &[String], data: &Path, wsl: bool, env: &[(String, String)]) -> Plan {
    match flavor {
        Some(Flavor::Bash) => Plan { env: bash_env(), flavor: Some(Flavor::Bash), ..Default::default() },
        Some(Flavor::Zsh) if !zsh_has_command(args) => match zsh_env(data, wsl, env) {
            Some(e) => Plan { env: e, flavor: Some(Flavor::Zsh), ..Default::default() },
            None => Plan::default(),
        },
        Some(Flavor::Fish) if !fish_has_command(args) => match fish_env(data, wsl) {
            Some(e) => Plan { args: vec!["--init-command".into(), FISH_INIT_COMMAND.into()], env: vec![e], flavor: Some(Flavor::Fish) },
            None => Plan::default(),
        },
        _ => Plan::default(),
    }
}

/// Unbekannte Login-Shell in WSL: bash- und zsh-Integration zugleich (beides nur Variablen).
fn wsl_fallback(data: &Path, env: &[(String, String)]) -> Plan {
    let mut e = bash_env();
    e.extend(zsh_env(data, true, env).unwrap_or_default());
    Plan { env: e, flavor: Some(Flavor::Bash), ..Default::default() }
}

/// Integration einer WSL-Sitzung (`args` = Argumente von `wsl.exe`, `login` = Stand der Login-Shell der Distribution).
pub fn wsl_plan(args: &[String], login: Option<Login>, data: &Path, env: &[(String, String)]) -> Plan {
    match wsl_target(args) {
        WslTarget::Exec(i) => {
            let flavor = Flavor::of_unix_shell(&args[i]);
            unix_plan(flavor, &args[i + 1..], data, true, env)
        }
        WslTarget::Other => wsl_fallback(data, env),
        WslTarget::Login => match login {
            Some(Login::Known(p)) => match Flavor::of_unix_shell(&p) {
                Some(Flavor::Fish) => {
                    // fish bekommt `--init-command` nur als Argument: die Login-Shell selbst starten (`-l` wie WSL).
                    let mut plan = unix_plan(Some(Flavor::Fish), &[], data, true, env);
                    if plan.flavor.is_some() {
                        let mut a = vec!["-e".to_string(), p.clone(), "-l".to_string()];
                        a.append(&mut plan.args);
                        plan.args = a;
                    }
                    plan
                }
                f @ Some(_) => unix_plan(f, &[], data, true, env),
                None => Plan::default(),
            },
            _ => wsl_fallback(data, env),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pd-shellinit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn flavors_from_file_names() {
        assert_eq!(Flavor::of_unix_shell("/usr/bin/zsh"), Some(Flavor::Zsh));
        assert_eq!(Flavor::of_unix_shell("-zsh"), Some(Flavor::Zsh));
        assert_eq!(Flavor::of_unix_shell("/usr/local/bin/fish"), Some(Flavor::Fish));
        assert_eq!(Flavor::of_unix_shell("/bin/bash"), Some(Flavor::Bash));
        assert_eq!(Flavor::of_unix_shell("/usr/bin/nu"), None);
        assert_eq!(Flavor::of_unix_shell("/bin/sh"), None);
    }

    #[test]
    fn passwd_line_gives_a_checked_shell() {
        assert_eq!(parse_passwd_shell("max:x:1000:1000:Max M,,,:/home/max:/usr/bin/zsh\n").as_deref(), Some("/usr/bin/zsh"));
        assert_eq!(parse_passwd_shell("\nmax:x:1000:1000::/home/max:/usr/bin/fish").as_deref(), Some("/usr/bin/fish"));
        // Nichts, was auf eine Kommandozeile gehört: Leerzeichen, Anführungszeichen, relative Pfade
        assert_eq!(parse_passwd_shell("max:x:1:1::/h:/bin/zsh -c x"), None);
        assert_eq!(parse_passwd_shell("max:x:1:1::/h:/bin/\"zsh"), None);
        assert_eq!(parse_passwd_shell("max:x:1:1::/h:zsh"), None);
        assert_eq!(parse_passwd_shell("kaputt"), None);
        assert_eq!(parse_passwd_shell(""), None);
    }

    #[test]
    fn wsl_command_lines() {
        assert_eq!(wsl_target(&s(&["-d", "Ubuntu"])), WslTarget::Login);
        assert_eq!(wsl_target(&s(&["--cd", "/tmp", "-d", "Ubuntu"])), WslTarget::Login);
        assert_eq!(wsl_target(&s(&["~", "-d", "Ubuntu"])), WslTarget::Login);
        assert_eq!(wsl_target(&s(&["-d", "Ubuntu", "-e", "zsh"])), WslTarget::Exec(3));
        assert_eq!(wsl_target(&s(&["-d", "U", "--exec", "/usr/bin/fish", "-l"])), WslTarget::Exec(3));
        assert_eq!(wsl_target(&s(&["-d", "U", "ls"])), WslTarget::Other);
        assert_eq!(wsl_target(&s(&["-d", "U", "--", "ls"])), WslTarget::Other);
        assert_eq!(wsl_target(&s(&["-u", "root"])), WslTarget::Other);
        assert_eq!(wsl_target(&s(&["-e"])), WslTarget::Other);
    }

    #[test]
    fn zsh_wrapper_restores_zdotdir_before_loading_user_files() {
        let restore = ZSH_ENV.find("builtin unset ZDOTDIR").unwrap();
        let user = ZSH_ENV.find("builtin source -- \"${ZDOTDIR-$HOME}/.zshenv\"").unwrap();
        let hooks = ZSH_ENV.find("precmd_functions=(__paneldeck_precmd").unwrap();
        assert!(restore < user && user < hooks);
        // Exit-Status als Erstes, unsere precmd-Funktion vorn
        assert!(ZSH_ENV.contains("__paneldeck_precmd() {\n    builtin local __pd_ec=$?"));
        for m in ["133;D;%s", "9;9;\"%s\"", "133;A", "133;C"] {
            assert!(ZSH_ENV.contains(m), "{m}");
        }
        // Zeilenenden LF (rustc normalisiert CRLF der Quelldatei), sonst liefe zsh nicht
        assert!(!ZSH_ENV.contains('\r') && !FISH_INIT.contains('\r'));
        // Nur interaktive Shells, nur einmal
        assert!(ZSH_ENV.contains("[[ -o interactive && -z \"${__paneldeck_hooks-}\" ]]"));
    }

    #[test]
    fn fish_init_hooks_events_and_cleans_up() {
        assert!(FISH_INIT.starts_with("# Paneldeck"));
        assert!(FISH_INIT.contains("set -e PANELDECK_FISH_INIT"));
        for ev in ["--on-event fish_postexec", "--on-event fish_prompt", "--on-event fish_preexec"] {
            assert!(FISH_INIT.contains(ev), "{ev}");
        }
        assert!(FISH_INIT.contains("printf '\\e]133;D;%s\\a' $status"));
        // Für wsl.exe: keine Anführungszeichen und kein Backslash im Argument
        assert!(!FISH_INIT_COMMAND.contains(['"', '\'', '\\']));
    }

    #[test]
    fn zsh_plan_writes_wrapper_and_keeps_user_zdotdir() {
        let d = tmp("zsh");
        let p = unix_plan(Some(Flavor::Zsh), &[], &d, false, &[("ZDOTDIR".into(), "/home/max/.config/zsh".into())]);
        assert_eq!(p.flavor, Some(Flavor::Zsh));
        let z = p.env.iter().find(|(k, _)| k == "ZDOTDIR").unwrap().1.clone();
        assert_eq!(std::fs::read_to_string(Path::new(&z).join(".zshenv")).unwrap(), ZSH_ENV);
        assert!(p.env.contains(&(ENV_ZDOTDIR.to_string(), "/home/max/.config/zsh".to_string())));
        // Eigener Befehl oder `-f`: keine Integration
        assert_eq!(unix_plan(Some(Flavor::Zsh), &s(&["-c", "ls"]), &d, false, &[]).flavor, None);
        assert_eq!(unix_plan(Some(Flavor::Zsh), &s(&["-f"]), &d, false, &[]).flavor, None);
        assert_eq!(unix_plan(Some(Flavor::Zsh), &s(&["-l"]), &d, false, &[]).flavor, Some(Flavor::Zsh));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d.join(DIR)).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(Path::new(&z).join(".zshenv")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        remove_files(&d);
        assert!(!d.join(DIR).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fish_plan_uses_init_command() {
        let d = tmp("fish");
        let p = unix_plan(Some(Flavor::Fish), &s(&["-l"]), &d, false, &[]);
        assert_eq!(p.args, s(&["--init-command", FISH_INIT_COMMAND]));
        let f = &p.env.iter().find(|(k, _)| k == ENV_FISH).unwrap().1;
        assert_eq!(std::fs::read_to_string(f).unwrap(), FISH_INIT);
        assert_eq!(unix_plan(Some(Flavor::Fish), &s(&["script.fish"]), &d, false, &[]), Plan::default());
        assert_eq!(unix_plan(Some(Flavor::Fish), &s(&["-c", "ls"]), &d, false, &[]), Plan::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn wsl_plans_follow_the_login_shell() {
        let d = tmp("wsl");
        let args = s(&["--cd", "/tmp", "-d", "U"]);
        let names = |p: &Plan| p.env.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        // Unbekannt (läuft noch / gescheitert): bash und zsh zugleich
        for l in [None, Some(Login::Pending), Some(Login::Failed)] {
            let p = wsl_plan(&args, l, &d, &[]);
            assert_eq!(names(&p), s(&["PROMPT_COMMAND", "PS0", "ZDOTDIR"]));
            assert!(p.args.is_empty());
        }
        // ZDOTDIR als WSL-Pfad
        let p = wsl_plan(&args, Some(Login::Known("/usr/bin/zsh".into())), &d, &[]);
        assert_eq!(names(&p), s(&["ZDOTDIR"]));
        assert!(p.env[0].1.starts_with("/mnt/"), "{}", p.env[0].1);
        assert_eq!(wsl_plan(&args, Some(Login::Known("/bin/bash".into())), &d, &[]).flavor, Some(Flavor::Bash));
        // fish: Login-Shell selbst mit --init-command
        let p = wsl_plan(&args, Some(Login::Known("/usr/bin/fish".into())), &d, &[]);
        assert_eq!(p.args, s(&["-e", "/usr/bin/fish", "-l", "--init-command", FISH_INIT_COMMAND]));
        assert!(p.env[0].1.starts_with("/mnt/") && p.env[0].0 == ENV_FISH);
        // Andere Shell: ohne Integration
        assert_eq!(wsl_plan(&args, Some(Login::Known("/usr/bin/nu".into())), &d, &[]), Plan::default());
        // Eigenes -e im Profil
        let p = wsl_plan(&s(&["-d", "U", "-e", "fish"]), Some(Login::Known("/bin/bash".into())), &d, &[]);
        assert_eq!(p.args, s(&["--init-command", FISH_INIT_COMMAND]));
        // Netzwerkpfad als Datenordner: WSL sähe ihn nicht – kein ZDOTDIR
        assert_eq!(wsl_plan(&args, Some(Login::Known("/usr/bin/zsh".into())), Path::new(r"\\server\share\pd"), &[]), Plan::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn suffix_names_the_shell() {
        use crate::state::Lang;
        assert_eq!(login_suffix(Some(&Login::Known("/usr/bin/zsh".into())), Lang::En), " (zsh)");
        assert_eq!(login_suffix(Some(&Login::Known("/usr/bin/nu".into())), Lang::En), " (nu, without integration)");
        assert_eq!(login_suffix(Some(&Login::Failed), Lang::De), " (Shell unbekannt)");
        assert_eq!(login_suffix(Some(&Login::Pending), Lang::En), "");
        assert_eq!(login_suffix(None, Lang::En), "");
    }
}
