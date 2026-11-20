//! Shell-Integration für zsh und fish (#57) und Erkennung der Login-Shell von WSL-Distributionen.
//!
//! - **zsh:** Paneldeck setzt `ZDOTDIR` auf einen eigenen Ordner im Datenordner (`shell-integration/zsh`). Dessen
//!   `.zshenv` stellt sofort das `ZDOTDIR` des Nutzers wieder her (aus `PANELDECK_ZSH_ZDOTDIR`, sonst ungesetzt = `$HOME`)
//!   und lädt dessen `.zshenv`; danach liest zsh `.zprofile`/`.zshrc`/`.zlogin` des Nutzers wie gewohnt. Für interaktive
//!   Shells kommen `precmd`/`preexec`-Hooks dazu. Die Dateien des Nutzers werden nie verändert.
//! - **fish:** `--init-command` (läuft nach `config.fish`), Ereignis-Funktionen für `fish_prompt`, `fish_preexec`,
//!   `fish_postexec`. Solange die Login-Shell einer WSL-Distribution noch nicht bekannt ist, bekommt fish die
//!   Integration über `XDG_DATA_DIRS` (`fish/vendor_conf.d`, die Datei nimmt sich danach selbst wieder heraus).
//! - **WSL:** Die Login-Shell der Distribution (`getent passwd`), die Liste `/etc/shells` und ob WSL die Hilfsdateien
//!   lesen kann (`test -r`, Pfad über `WSLENV …/p`), ermittelt Paneldeck im Hintergrund und hält es je Programmstart im
//!   Speicher; eine gescheiterte Abfrage wird nach [`RETRY`] wiederholt. `ZDOTDIR` setzt Paneldeck nur, wenn WSL die
//!   `.zshenv` lesen kann – sonst fände zsh die Dateien des Nutzers nicht.
//!
//! Gesendet wird wie bei bash: `OSC 133;D;<exit>` (Befehl beendet), `OSC 7` und `OSC 9;9;"<ordner>"`, `OSC 133;A`
//! (Prompt), `OSC 133;B` (Ende des Prompts) und `OSC 133;C` (Befehl startet). Plattformneutral: unter Linux/macOS gilt
//! dasselbe für zsh/fish als Programm (zsh ist die Standard-Shell von macOS).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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

/// Ergebnis der Abfrage in einer WSL-Distribution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoginInfo {
    /// Pfad der Login-Shell (geprüft, siehe [`valid_shell_path`]).
    pub shell: String,
    /// WSL kann die `.zshenv` des eigenen `ZDOTDIR` lesen (nur dann wird `ZDOTDIR` gesetzt).
    pub zsh_ok: bool,
    /// Einträge aus `/etc/shells` (geprüft), für die Shell-Auswahl im Profil.
    pub shells: Vec<String>,
}

/// Was über die Login-Shell einer WSL-Distribution bekannt ist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    /// Abfrage läuft.
    Pending,
    Known(LoginInfo),
    /// Abfrage gescheitert (Zeitlimit, kein `getent`, unlesbare Ausgabe) zu diesem Zeitpunkt: Rückfall auf die
    /// bash-Integration; nach [`RETRY`] fragt Paneldeck erneut.
    Failed(Instant),
}

/// Nach dieser Zeit wird eine gescheiterte Abfrage wiederholt (Zeitlimit beim Hochfahren der VM ist vorübergehend).
pub const RETRY: Duration = Duration::from_secs(45);

fn cache() -> &'static Mutex<HashMap<String, Login>> {
    static C: OnceLock<Mutex<HashMap<String, Login>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Schlüssel: Distributionsnamen sind in WSL ohne Groß-/Kleinschreibung (`-d ubuntu` = `Ubuntu`).
fn key(distro: &str) -> String {
    distro.to_lowercase()
}

/// Datenordner für die Hilfsdateien der Abfrage (siehe [`init`]).
fn data_dir() -> &'static OnceLock<PathBuf> {
    static D: OnceLock<PathBuf> = OnceLock::new();
    &D
}

/// Datenordner merken (beim Programmstart): die Abfrage legt die Hilfsdateien an und prüft, ob WSL sie lesen kann.
pub fn init(data: &Path) {
    let _ = data_dir().set(data.to_path_buf());
}

/// Bekannter Stand der Login-Shell von `distro` (`""` = Standard-Distribution), ohne eine Abfrage zu starten.
pub fn wsl_login(distro: &str) -> Option<Login> {
    cache().lock().ok().and_then(|c| c.get(&key(distro)).cloned())
}

/// Wie [`wsl_login`]; ist noch nichts bekannt (oder die letzte Abfrage länger als [`RETRY`] gescheitert), startet die
/// Abfrage im Hintergrund (Ergebnis dann: [`Login::Pending`]).
pub fn ensure_wsl_login(distro: &str) -> Login {
    {
        let Ok(mut c) = cache().lock() else { return Login::Failed(Instant::now()) };
        let cur = c.get(&key(distro));
        if !needs_query(cur) {
            return cur.cloned().unwrap_or(Login::Pending);
        }
        c.insert(key(distro), Login::Pending);
    }
    let d = distro.to_string();
    let spawned = std::thread::Builder::new().name("wsl-login-shell".into()).spawn(move || {
        let r = probe(&d).map(Login::Known).unwrap_or_else(|| Login::Failed(Instant::now()));
        set_login(&d, r);
    });
    if spawned.is_err() {
        set_login(distro, Login::Failed(Instant::now()));
        return Login::Failed(Instant::now());
    }
    Login::Pending
}

/// Neue Abfrage nötig: noch nie gefragt oder vor mehr als [`RETRY`] gescheitert.
fn needs_query(l: Option<&Login>) -> bool {
    match l {
        None => true,
        Some(Login::Failed(t)) => t.elapsed() >= RETRY,
        _ => false,
    }
}

/// Abfrage in der Distribution: Hilfsdateien anlegen, dann Login-Shell, `/etc/shells` und Lesbarkeit prüfen.
fn probe(distro: &str) -> Option<LoginInfo> {
    let zdir = data_dir().get().and_then(|d| {
        let z = zsh_dir(d);
        write_private(&z.join(".zshenv"), ZSH_ENV).ok().map(|_| win_path(&z))
    });
    let out = crate::wsl::login_probe(distro, zdir.as_deref())?;
    parse_probe(&out)
}

pub(crate) fn set_login(distro: &str, l: Login) {
    if let Ok(mut c) = cache().lock() {
        c.insert(key(distro), l);
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
    let line = out.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("PD"))?;
    let shell = line.split(':').nth(6)?.trim();
    valid_shell_path(shell).then(|| shell.to_string())
}

/// Ausgabe von [`crate::wsl::login_probe`]: Zeile aus `passwd`, `PDSHELL=<pfad>` je Eintrag in `/etc/shells`,
/// `PDZ=1`, wenn die `.zshenv` lesbar ist.
pub fn parse_probe(out: &str) -> Option<LoginInfo> {
    let shell = parse_passwd_shell(out)?;
    let mut shells: Vec<String> = Vec::new();
    for l in out.lines().map(str::trim) {
        if let Some(p) = l.strip_prefix("PDSHELL=") {
            if valid_shell_path(p) && !shells.iter().any(|s| s == p) && shells.len() < 64 {
                shells.push(p.to_string());
            }
        }
    }
    Some(LoginInfo { shell, zsh_ok: out.lines().any(|l| l.trim() == "PDZ=1"), shells })
}

/// Text hinter dem Namen der Distribution in Auswahllisten: erkannte Shell, „Shell unbekannt“ (Abfrage gescheitert,
/// bash-Integration) oder „ohne Integration“ (andere Shell, z. B. nushell). Leer, solange die Abfrage läuft.
pub fn login_suffix(login: Option<&Login>, lang: crate::state::Lang) -> String {
    match login {
        Some(Login::Known(i)) => shell_suffix(&i.shell, lang),
        Some(Login::Failed(_)) => format!(" ({})", lang.t("Shell unbekannt", "shell unknown")),
        _ => String::new(),
    }
}

/// „ (zsh)“ bzw. „ (nu, ohne Integration)“.
pub fn shell_suffix(path: &str, lang: crate::state::Lang) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if Flavor::of_unix_shell(path).is_some() {
        format!(" ({name})")
    } else {
        format!(" ({name}, {})", lang.t("ohne Integration", "without integration"))
    }
}

// ------------------------------------------------------------------ Hilfsdateien

/// Unterordner im Datenordner für die Hilfsdateien (beim Beenden gelöscht).
pub const DIR: &str = "shell-integration";
/// Variable mit dem ursprünglichen `ZDOTDIR` des Nutzers (nur gesetzt, wenn er eins hatte).
pub const ENV_ZDOTDIR: &str = "PANELDECK_ZSH_ZDOTDIR";
/// Variable mit dem Pfad der fish-Datei (für `--init-command`).
pub const ENV_FISH: &str = "PANELDECK_FISH_INIT";
/// Unser Eintrag in `XDG_DATA_DIRS` (fish im Rückfall); die Datei entfernt ihn wieder.
pub const ENV_XDG: &str = "PANELDECK_XDG";
/// Wert von `--init-command`: ohne Anführungszeichen (`wsl.exe` zerlegt seine Kommandozeile selbst); fish teilt den Wert
/// einer Variablen nie an Leerzeichen. Fehlt die Variable oder die Datei, passiert nichts.
pub const FISH_INIT_COMMAND: &str = "set -q PANELDECK_FISH_INIT; and test -r $PANELDECK_FISH_INIT; and source $PANELDECK_FISH_INIT";

/// `.zshenv` im eigenen `ZDOTDIR`. Exit-Status zuerst sichern (unsere Funktion steht vorn in `precmd_functions`); das
/// Ende des Prompts (133;B) hängt eine Funktion an, die immer als letzte läuft (Themes setzen PS1 in precmd neu).
/// PROMPT_SP: zsh markiert eine Ausgabe ohne Zeilenende (`%` und Leerzeichen bis zum Rand) erst nach precmd. Das tun wir
/// vor dem Prompt-Anfang selbst, damit die Marke auf der Zeile des Prompts steht und `run_command` die Ausgabe
/// vollständig bekommt (das eigene PROMPT_SP von zsh überschreibt danach nur noch die Prompt-Zeile).
pub const ZSH_ENV: &str = r#"# Paneldeck shell integration for zsh (#57). Generated by Paneldeck and removed when it exits - do not edit.
# Paneldeck starts zsh with ZDOTDIR pointing here. This file restores your own ZDOTDIR at once, so zsh then reads your
# .zshenv, .zprofile, .zshrc and .zlogin as usual; interactive shells get prompt hooks (OSC 133, OSC 7, OSC 9;9).
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
    builtin local __pd_ec=$? __pd_p=${PWD//[[:cntrl:]]/} __pd_u
    __pd_u=${__pd_p//\%/%25}
    __pd_u=${__pd_u// /%20}
    if [[ -o prompt_sp ]]; then
      builtin print -nP -- "${PROMPT_EOL_MARK-%B%S%#%s%b}"
      builtin printf '%*s\r' $(( COLUMNS > 1 ? COLUMNS - 1 : 0 )) ''
    fi
    builtin printf '\033]133;D;%s\007\033]7;file://%s%s\007\033]9;9;"%s"\007\033]133;A\007' "$__pd_ec" "${HOST//[[:cntrl:]]/}" "$__pd_u" "$__pd_p"
  }
  __paneldeck_prompt_end() {
    [[ $PS1 == *$'\e]133;B\a'* ]] || PS1+=$'%{\e]133;B\a%}'
  }
  __paneldeck_preexec() {
    builtin printf '\033]133;C\007'
    precmd_functions=(${precmd_functions:#__paneldeck_prompt_end} __paneldeck_prompt_end)
  }
  typeset -ga precmd_functions preexec_functions
  precmd_functions=(__paneldeck_precmd ${precmd_functions:#__paneldeck_precmd} __paneldeck_prompt_end)
  preexec_functions=(${preexec_functions:#__paneldeck_preexec} __paneldeck_preexec)
fi
"#;

/// fish-Hooks (für `--init-command` und für den Rückfall über `XDG_DATA_DIRS`). Das Ende des Prompts (133;B): beim
/// ersten Prompt wird `fish_prompt` umhüllt (dann ist auch ein in `config.fish` definierter Prompt da).
pub const FISH_INIT: &str = r#"# Paneldeck shell integration for fish (#57). Generated by Paneldeck and removed when it exits - do not edit.
# Loaded with `fish --init-command` (after config.fish); your own files stay untouched.
set -e PANELDECK_FISH_INIT
if status is-interactive; and not set -q __paneldeck_hooks
    set -g __paneldeck_hooks 1
    function __paneldeck_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end
    function __paneldeck_prompt --on-event fish_prompt
        set -l p (string replace -ra '[[:cntrl:]]' '' -- $PWD)
        set -l u (string replace -a '%' '%25' -- $p | string replace -a ' ' '%20')
        printf '\e]7;file://%s%s\a\e]9;9;"%s"\a\e]133;A\a' (string replace -ra '[[:cntrl:]]' '' -- $hostname) "$u" "$p"
        if not set -q __paneldeck_wrapped; and functions -q fish_prompt; and not functions -q __paneldeck_orig_prompt
            set -g __paneldeck_wrapped 1
            functions -c fish_prompt __paneldeck_orig_prompt
            function fish_prompt
                __paneldeck_orig_prompt
                printf '\e]133;B\a'
            end
        end
    end
    function __paneldeck_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end
end
"#;

/// Vorspann der Datei in `XDG_DATA_DIRS/fish/vendor_conf.d` (Rückfall, Login-Shell noch unbekannt): unseren Eintrag
/// wieder aus `XDG_DATA_DIRS` nehmen (wie Ghostty); war es nur unserer, die Variable wieder entfernen.
pub const FISH_XDG_PRE: &str = r#"# Paneldeck shell integration for fish (#57), loaded through XDG_DATA_DIRS while the login shell was not known yet.
if set -q PANELDECK_XDG
    set -l __pd_keep
    for d in (string split : -- "$XDG_DATA_DIRS")
        test "$d" = "$PANELDECK_XDG"; or set -a __pd_keep $d
    end
    if test (count $__pd_keep) -gt 0
        set -gx XDG_DATA_DIRS (string join : -- $__pd_keep)
    else
        set -e XDG_DATA_DIRS
    end
    set -e PANELDECK_XDG
end
"#;

/// Zum Einfügen auf entfernten Rechnern (Einstellungen → Terminal): nur die Befehlsmarken, kein Ordner (ein Pfad des
/// anderen Rechners gehört nicht in den Dateibaum).
pub const REMOTE_BASH: &str = r#"# Paneldeck shell integration (bash) - add to ~/.bashrc on the remote machine
__paneldeck_prompt() { local ec=$?; printf '\033]133;D;%s\007\033]133;A\007' "$ec"; [[ $PS1 == *'133;B'* ]] || PS1="$PS1"'\[\e]133;B\a\]'; return $ec; }
PROMPT_COMMAND="__paneldeck_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
PS0=$'\e]133;C\a'
"#;

/// Wie [`REMOTE_BASH`] für zsh (`~/.zshrc`).
pub const REMOTE_ZSH: &str = r#"# Paneldeck shell integration (zsh) - add to ~/.zshrc on the remote machine
__paneldeck_precmd() { local ec=$?; builtin printf '\033]133;D;%s\007\033]133;A\007' "$ec"; }
__paneldeck_preexec() { builtin printf '\033]133;C\007'; }
typeset -ga precmd_functions preexec_functions
precmd_functions=(__paneldeck_precmd ${precmd_functions:#__paneldeck_precmd})
preexec_functions+=(__paneldeck_preexec)
"#;

/// Wie [`REMOTE_BASH`] für fish (`~/.config/fish/conf.d/paneldeck.fish`).
pub const REMOTE_FISH: &str = r#"# Paneldeck shell integration (fish) - save as ~/.config/fish/conf.d/paneldeck.fish on the remote machine
function __paneldeck_postexec --on-event fish_postexec; printf '\e]133;D;%s\a' $status; end
function __paneldeck_prompt --on-event fish_prompt; printf '\e]133;A\a'; end
function __paneldeck_preexec --on-event fish_preexec; printf '\e]133;C\a'; end
"#;

fn zsh_dir(data: &Path) -> PathBuf {
    data.join(DIR).join("zsh")
}

fn fish_file(data: &Path) -> PathBuf {
    data.join(DIR).join("paneldeck.fish")
}

fn xdg_dir(data: &Path) -> PathBuf {
    data.join(DIR).join("xdg")
}

/// Ordner (0700 unter Unix) und Datei (0600) anlegen; nur schreiben, wenn sich der Inhalt unterscheidet.
fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent"))?;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Alle Ordner bis einschließlich `shell-integration` schützen.
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

/// Pfad als Text, ohne `\\?\`-Vorsatz. Für WSL bleibt es der Windows-Pfad: WSL rechnet ihn über `WSLENV <name>/p`
/// nach seiner eigenen Einhänge-Konfiguration um.
fn win_path(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

// ------------------------------------------------------------------ Startplan

/// Zusätzliche Argumente/Variablen für die Integration einer Sitzung.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Angewandte Integration (`None` = keine). WSL mit noch unbekannter Login-Shell: `Bash` (bash, dazu fish über
    /// `XDG_DATA_DIRS`).
    pub flavor: Option<Flavor>,
    /// Variablen, die als Pfad nach WSL gehen (`WSLENV <name>/p`).
    pub path_vars: Vec<String>,
}

/// Was eine WSL-Kommandozeile startet.
#[derive(Debug, PartialEq, Eq)]
pub enum WslTarget {
    /// Die Login-Shell (nur Optionen wie `-d`, `--cd`, `~`).
    Login,
    /// `-e <programm> …`: dieses Programm, Index des Programms in den Argumenten.
    Exec(usize),
    /// Ein Befehl über die Login-Shell (`wsl ls`, `wsl -- ls`) oder ein anderer Benutzer (`-u`): Integration wie bisher
    /// (bash), ohne Abfrage.
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

/// Woher ein bisheriges `ZDOTDIR` des Nutzers kommt.
pub struct EnvCtx<'a> {
    /// Schon gesetzte Variablen der Sitzung (Profil).
    pub env: &'a [(String, String)],
    /// Die Sitzung erbt die Umgebung des Hubs (Einstellung „Umgebung von Paneldeck erben“): nur dann zählt ein
    /// `ZDOTDIR` aus der Umgebung des Hubs.
    pub inherit: bool,
}

/// Variablen für zsh: eigenes `ZDOTDIR`, dazu das bisherige des Nutzers. `None`, wenn die Datei nicht geschrieben
/// werden konnte.
fn zsh_env(data: &Path, wsl: bool, ctx: &EnvCtx) -> Option<Plan> {
    let dir = zsh_dir(data);
    write_private(&dir.join(".zshenv"), ZSH_ENV).ok()?;
    let mut p = Plan { env: vec![("ZDOTDIR".to_string(), win_path(&dir))], flavor: Some(Flavor::Zsh), ..Default::default() };
    if wsl {
        p.path_vars.push("ZDOTDIR".into());
    }
    let user = ctx.env.iter().rev().find(|(k, _)| k == "ZDOTDIR").map(|(_, v)| v.clone()).or_else(|| if wsl || !ctx.inherit { None } else { std::env::var("ZDOTDIR").ok() });
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        p.env.push((ENV_ZDOTDIR.to_string(), u));
    }
    Some(p)
}

/// Variable für fish (`--init-command` lädt die Datei).
fn fish_plan(data: &Path, wsl: bool) -> Option<Plan> {
    let f = fish_file(data);
    write_private(&f, FISH_INIT).ok()?;
    Some(Plan {
        args: vec!["--init-command".into(), FISH_INIT_COMMAND.into()],
        env: vec![(ENV_FISH.to_string(), win_path(&f))],
        flavor: Some(Flavor::Fish),
        path_vars: if wsl { vec![ENV_FISH.into()] } else { vec![] },
    })
}

/// bash-Integration (PROMPT_COMMAND/PS0).
fn bash_plan() -> Plan {
    Plan { env: vec![("PROMPT_COMMAND".into(), crate::shells::BASH_PC.into()), ("PS0".into(), crate::shells::BASH_PS0.into())], flavor: Some(Flavor::Bash), ..Default::default() }
}

/// Integration für eine Unix-Shell (`flavor`), die Paneldeck selbst mit `args` startet. In WSL nur mit bestätigter
/// `.zshenv` (`zsh_ok`).
pub fn unix_plan(flavor: Option<Flavor>, args: &[String], data: &Path, wsl: bool, zsh_ok: bool, ctx: &EnvCtx) -> Plan {
    match flavor {
        Some(Flavor::Bash) => bash_plan(),
        Some(Flavor::Zsh) if !zsh_has_command(args) && (zsh_ok || !wsl) => zsh_env(data, wsl, ctx).unwrap_or_default(),
        Some(Flavor::Fish) if !fish_has_command(args) => fish_plan(data, wsl).unwrap_or_default(),
        _ => Plan::default(),
    }
}

/// Login-Shell in WSL noch unbekannt oder Abfrage gescheitert: bash wie bisher (räumt die fish-Variablen weg), fish über
/// `XDG_DATA_DIRS`. zsh bekommt kein `ZDOTDIR`, bevor die Abfrage bestätigt hat, dass WSL die Datei lesen kann.
fn wsl_fallback(data: &Path) -> Plan {
    let mut p = Plan { env: vec![("PROMPT_COMMAND".into(), crate::shells::BASH_PC_XDG.into()), ("PS0".into(), crate::shells::BASH_PS0.into())], flavor: Some(Flavor::Bash), ..Default::default() };
    let dir = xdg_dir(data);
    if write_private(&dir.join("fish").join("vendor_conf.d").join("paneldeck.fish"), &format!("{FISH_XDG_PRE}{FISH_INIT}")).is_ok() {
        let w = win_path(&dir);
        p.env.push(("XDG_DATA_DIRS".into(), w.clone()));
        p.env.push((ENV_XDG.into(), w));
        p.path_vars.extend(["XDG_DATA_DIRS".to_string(), ENV_XDG.to_string()]);
    }
    p
}

/// Integration einer WSL-Sitzung (`args` = Argumente von `wsl.exe`, `login` = Stand der Login-Shell der Distribution).
pub fn wsl_plan(args: &[String], login: Option<Login>, data: &Path, ctx: &EnvCtx) -> Plan {
    let zsh_ok = matches!(&login, Some(Login::Known(i)) if i.zsh_ok);
    match wsl_target(args) {
        WslTarget::Exec(i) => unix_plan(Flavor::of_unix_shell(&args[i]), &args[i + 1..], data, true, zsh_ok, ctx),
        WslTarget::Other => bash_plan(),
        WslTarget::Login => match login {
            Some(Login::Known(info)) => match Flavor::of_unix_shell(&info.shell) {
                Some(Flavor::Fish) => {
                    // fish bekommt `--init-command` nur als Argument: die Login-Shell selbst starten (`-l` wie WSL).
                    let mut plan = unix_plan(Some(Flavor::Fish), &[], data, true, zsh_ok, ctx);
                    if plan.flavor.is_some() {
                        let mut a = vec!["-e".to_string(), info.shell.clone(), "-l".to_string()];
                        a.append(&mut plan.args);
                        plan.args = a;
                    }
                    plan
                }
                f @ Some(_) => unix_plan(f, &[], data, true, zsh_ok, ctx),
                None => Plan::default(),
            },
            _ => wsl_fallback(data),
        },
    }
}

/// Eigene Shell des Profils (#57, „Shell in WSL“) als `-e <pfad> -l` an eine WSL-Kommandozeile hängen, die sonst die
/// Login-Shell startet. Ungültige Pfade und Kommandozeilen mit eigenem Programm bleiben unverändert.
pub fn apply_login_override(args: &mut Vec<String>, path: &str) -> bool {
    if path.is_empty() || !valid_shell_path(path) || wsl_target(args) != WslTarget::Login {
        return false;
    }
    args.extend(["-e".to_string(), path.to_string(), "-l".to_string()]);
    true
}

/// Mit zsh: die Markierung einer Ausgabe ohne Zeilenende (PROMPT_EOL_MARK, `%` bzw. `#` als root, danach Leerzeichen
/// bis zum Rand) aus der letzten Ausgabezeile entfernen. Nur wenn diese Zeile in die Prompt-Zeile umbricht
/// (`wrapped`) – eine echte Ausgabe „100%“ mit Zeilenende bleibt.
pub fn strip_eol_mark(lines: &mut Vec<String>, wrapped: bool) {
    if !wrapped {
        return;
    }
    if let Some(last) = lines.last_mut() {
        let t = last.trim_end();
        if let Some(rest) = t.strip_suffix('%').or_else(|| t.strip_suffix('#')) {
            *last = rest.to_string();
        }
        if last.is_empty() {
            lines.pop();
        }
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

    const NO_ENV: EnvCtx = EnvCtx { env: &[], inherit: false };

    fn known(shell: &str, zsh_ok: bool) -> Option<Login> {
        Some(Login::Known(LoginInfo { shell: shell.into(), zsh_ok, shells: vec![] }))
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
    fn probe_output_is_parsed() {
        let out = "max:x:1000:1000::/home/max:/usr/bin/zsh\nPDSHELL=# /etc/shells: valid login shells\nPDSHELL=/bin/sh\nPDSHELL=/usr/bin/zsh\nPDSHELL=/usr/bin/fish\nPDSHELL=/bin/sh\nPDSHELL=/bin/x y\nPDZ=1\n";
        let i = parse_probe(out).unwrap();
        assert_eq!(i, LoginInfo { shell: "/usr/bin/zsh".into(), zsh_ok: true, shells: s(&["/bin/sh", "/usr/bin/zsh", "/usr/bin/fish"]) });
        // Ohne PDZ=1: WSL kann die Datei nicht lesen
        assert!(!parse_probe("max:x:1:1::/h:/usr/bin/zsh\n").unwrap().zsh_ok);
        assert_eq!(parse_probe("PDZ=1\n"), None);
    }

    #[test]
    fn failed_detection_is_retried_later() {
        // Frisch gescheitert: keine neue Abfrage (Schlüssel ohne Groß-/Kleinschreibung)
        set_login("Retry-Test", Login::Failed(Instant::now()));
        assert!(matches!(ensure_wsl_login("retry-test"), Login::Failed(_)));
        assert!(!needs_query(wsl_login("RETRY-TEST").as_ref()));
        // Abgelaufen: wieder fragen; bekannt oder laufend: nicht
        assert!(needs_query(Some(&Login::Failed(Instant::now() - RETRY - Duration::from_secs(1)))));
        assert!(needs_query(None));
        assert!(!needs_query(Some(&Login::Pending)));
        assert!(!needs_query(known("/bin/bash", false).as_ref()));
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
    fn profile_shell_override() {
        let mut a = s(&["-d", "U"]);
        assert!(apply_login_override(&mut a, "/usr/bin/fish"));
        assert_eq!(a, s(&["-d", "U", "-e", "/usr/bin/fish", "-l"]));
        let mut b = s(&["-d", "U", "-e", "htop"]);
        assert!(!apply_login_override(&mut b, "/usr/bin/zsh"), "eigenes Programm bleibt");
        let mut c = s(&["-d", "U"]);
        assert!(!apply_login_override(&mut c, "/bin/zsh; rm"), "nur geprüfte Pfade");
        assert!(!apply_login_override(&mut c, ""));
    }

    #[test]
    fn zsh_wrapper_restores_zdotdir_before_loading_user_files() {
        let restore = ZSH_ENV.find("builtin unset ZDOTDIR").unwrap();
        let user = ZSH_ENV.find("builtin source -- \"${ZDOTDIR-$HOME}/.zshenv\"").unwrap();
        let hooks = ZSH_ENV.find("precmd_functions=(__paneldeck_precmd").unwrap();
        assert!(restore < user && user < hooks);
        // Exit-Status als Erstes, unsere precmd-Funktion vorn, das Prompt-Ende hinten
        assert!(ZSH_ENV.contains("__paneldeck_precmd() {\n    builtin local __pd_ec=$?"));
        assert!(ZSH_ENV.contains("${precmd_functions:#__paneldeck_precmd} __paneldeck_prompt_end)"));
        for m in ["133;D;%s", "]7;file://%s%s", "9;9;\"%s\"", "133;A", "133;B", "133;C", "[[:cntrl:]]"] {
            assert!(ZSH_ENV.contains(m), "{m}");
        }
        // OSC 7 vor OSC 9;9 (der Windows-Pfad bzw. Linux-Pfad aus 9;9 gilt zuletzt), PROMPT_SP vor dem Prompt-Anfang
        assert!(ZSH_ENV.find("]7;file").unwrap() < ZSH_ENV.find("9;9;").unwrap());
        assert!(ZSH_ENV.find("PROMPT_EOL_MARK").unwrap() < ZSH_ENV.find("133;A").unwrap());
        // Nur interaktive Shells, nur einmal
        assert!(ZSH_ENV.contains("[[ -o interactive && -z \"${__paneldeck_hooks-}\" ]]"));
        // Zeilenenden LF (rustc normalisiert CRLF der Quelldatei), sonst liefe zsh nicht
        for t in [ZSH_ENV, FISH_INIT, FISH_XDG_PRE, REMOTE_BASH, REMOTE_ZSH, REMOTE_FISH] {
            assert!(!t.contains('\r'));
        }
    }

    #[test]
    fn fish_init_hooks_events_and_cleans_up() {
        assert!(FISH_INIT.starts_with("# Paneldeck"));
        assert!(FISH_INIT.contains("set -e PANELDECK_FISH_INIT"));
        for ev in ["--on-event fish_postexec", "--on-event fish_prompt", "--on-event fish_preexec", "]133;B", "]7;file://", "[[:cntrl:]]"] {
            assert!(FISH_INIT.contains(ev), "{ev}");
        }
        assert!(FISH_INIT.contains("printf '\\e]133;D;%s\\a' $status"));
        // Für wsl.exe: keine Anführungszeichen und kein Backslash im Argument; ohne Variable oder Datei nichts
        assert!(!FISH_INIT_COMMAND.contains(['"', '\'', '\\']));
        assert!(FISH_INIT_COMMAND.starts_with("set -q PANELDECK_FISH_INIT; and "));
        // XDG-Rückfall: entfernt sich selbst
        assert!(FISH_XDG_PRE.contains("set -e PANELDECK_XDG") && FISH_XDG_PRE.contains("set -e XDG_DATA_DIRS"));
    }

    #[test]
    fn remote_snippets_send_marks_only() {
        for (t, ec) in [(REMOTE_BASH, "local ec=$?"), (REMOTE_ZSH, "local ec=$?"), (REMOTE_FISH, "$status")] {
            for m in ["133;A", "133;C", "133;D;%s"] {
                assert!(t.contains(m), "{m} in {t}");
            }
            assert!(t.contains(ec));
            assert!(!t.contains("9;9") && !t.contains("]7;"), "kein Ordner eines fremden Rechners");
        }
    }

    #[test]
    fn zsh_plan_writes_wrapper_and_keeps_user_zdotdir() {
        let d = tmp("zsh");
        let ctx = EnvCtx { env: &[("ZDOTDIR".into(), "/home/max/.config/zsh".into())], inherit: false };
        let p = unix_plan(Some(Flavor::Zsh), &[], &d, false, false, &ctx);
        assert_eq!(p.flavor, Some(Flavor::Zsh));
        let z = p.env.iter().find(|(k, _)| k == "ZDOTDIR").unwrap().1.clone();
        assert_eq!(std::fs::read_to_string(Path::new(&z).join(".zshenv")).unwrap(), ZSH_ENV);
        assert!(p.env.contains(&(ENV_ZDOTDIR.to_string(), "/home/max/.config/zsh".to_string())));
        assert!(p.path_vars.is_empty(), "nativ kein WSLENV");
        // Eigener Befehl oder `-f`: keine Integration
        assert_eq!(unix_plan(Some(Flavor::Zsh), &s(&["-c", "ls"]), &d, false, false, &NO_ENV).flavor, None);
        assert_eq!(unix_plan(Some(Flavor::Zsh), &s(&["-f"]), &d, false, false, &NO_ENV).flavor, None);
        assert_eq!(unix_plan(Some(Flavor::Zsh), &s(&["-l"]), &d, false, false, &NO_ENV).flavor, Some(Flavor::Zsh));
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
    fn hub_zdotdir_only_with_inherited_environment() {
        if cfg!(windows) {
            return; // nativ zsh nur unter Linux/macOS; die Logik prüft der WSL-Test
        }
        let d = tmp("inherit");
        // SAFETY: kein anderer Test liest oder schreibt ZDOTDIR.
        unsafe { std::env::set_var("ZDOTDIR", "/from/hub") };
        let off = unix_plan(Some(Flavor::Zsh), &[], &d, false, false, &EnvCtx { env: &[], inherit: false });
        assert!(!off.env.iter().any(|(k, _)| k == ENV_ZDOTDIR));
        let on = unix_plan(Some(Flavor::Zsh), &[], &d, false, false, &EnvCtx { env: &[], inherit: true });
        assert!(on.env.contains(&(ENV_ZDOTDIR.to_string(), "/from/hub".to_string())));
        // SAFETY: wie oben.
        unsafe { std::env::remove_var("ZDOTDIR") };
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fish_plan_uses_init_command() {
        let d = tmp("fish");
        let p = unix_plan(Some(Flavor::Fish), &s(&["-l"]), &d, false, false, &NO_ENV);
        assert_eq!(p.args, s(&["--init-command", FISH_INIT_COMMAND]));
        let f = &p.env.iter().find(|(k, _)| k == ENV_FISH).unwrap().1;
        assert_eq!(std::fs::read_to_string(f).unwrap(), FISH_INIT);
        assert_eq!(unix_plan(Some(Flavor::Fish), &s(&["script.fish"]), &d, false, false, &NO_ENV), Plan::default());
        assert_eq!(unix_plan(Some(Flavor::Fish), &s(&["-c", "ls"]), &d, false, false, &NO_ENV), Plan::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wsl_plans_follow_the_login_shell() {
        let d = tmp("wsl");
        let args = s(&["--cd", "/tmp", "-d", "U"]);
        let names = |p: &Plan| p.env.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        // Unbekannt (läuft noch / gescheitert): bash, fish über XDG_DATA_DIRS – kein ZDOTDIR
        for l in [None, Some(Login::Pending), Some(Login::Failed(Instant::now()))] {
            let p = wsl_plan(&args, l, &d, &NO_ENV);
            assert_eq!(names(&p), s(&["PROMPT_COMMAND", "PS0", "XDG_DATA_DIRS", ENV_XDG]));
            assert_eq!(p.path_vars, s(&["XDG_DATA_DIRS", ENV_XDG]));
            assert!(p.args.is_empty());
            let vendor = d.join(DIR).join("xdg").join("fish").join("vendor_conf.d").join("paneldeck.fish");
            assert!(std::fs::read_to_string(vendor).unwrap().starts_with(FISH_XDG_PRE));
        }
        // zsh: ZDOTDIR als Windows-Pfad mit WSLENV …/p – nur, wenn WSL die Datei lesen kann
        let p = wsl_plan(&args, known("/usr/bin/zsh", true), &d, &NO_ENV);
        assert_eq!(names(&p), s(&["ZDOTDIR"]));
        assert_eq!(p.path_vars, s(&["ZDOTDIR"]));
        assert!(!p.env[0].1.starts_with("/mnt/"), "WSL rechnet selbst um: {}", p.env[0].1);
        assert_eq!(wsl_plan(&args, known("/usr/bin/zsh", false), &d, &NO_ENV), Plan::default());
        assert_eq!(wsl_plan(&args, known("/bin/bash", false), &d, &NO_ENV).flavor, Some(Flavor::Bash));
        // fish: Login-Shell selbst mit --init-command
        let p = wsl_plan(&args, known("/usr/bin/fish", false), &d, &NO_ENV);
        assert_eq!(p.args, s(&["-e", "/usr/bin/fish", "-l", "--init-command", FISH_INIT_COMMAND]));
        assert_eq!((p.env[0].0.as_str(), p.path_vars.clone()), (ENV_FISH, s(&[ENV_FISH])));
        // Andere Shell: ohne Integration
        assert_eq!(wsl_plan(&args, known("/usr/bin/nu", true), &d, &NO_ENV), Plan::default());
        // Eigenes -e im Profil
        let p = wsl_plan(&s(&["-d", "U", "-e", "fish"]), known("/bin/bash", false), &d, &NO_ENV);
        assert_eq!(p.args, s(&["--init-command", FISH_INIT_COMMAND]));
        assert_eq!(wsl_plan(&s(&["-d", "U", "-e", "zsh"]), known("/bin/bash", true), &d, &NO_ENV).flavor, Some(Flavor::Zsh));
        assert_eq!(wsl_plan(&s(&["-d", "U", "-e", "zsh"]), None, &d, &NO_ENV).flavor, None, "ohne Bestätigung kein ZDOTDIR");
        // Befehl über die Login-Shell: bash wie bisher, ohne XDG
        assert_eq!(names(&wsl_plan(&s(&["-d", "U", "ls"]), None, &d, &NO_ENV)), s(&["PROMPT_COMMAND", "PS0"]));
        // Ein ZDOTDIR des Profils wird zum ZDOTDIR des Nutzers
        let ctx = EnvCtx { env: &[("ZDOTDIR".into(), "/home/u/z".into())], inherit: true };
        let p = wsl_plan(&args, known("/usr/bin/zsh", true), &d, &ctx);
        assert!(p.env.contains(&(ENV_ZDOTDIR.to_string(), "/home/u/z".to_string())));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn eol_mark_is_stripped_only_from_a_wrapped_last_line() {
        let mut l = s(&["x%"]);
        strip_eol_mark(&mut l, true);
        assert_eq!(l, s(&["x"]));
        let mut l = s(&["a", "b#   "]);
        strip_eol_mark(&mut l, true);
        assert_eq!(l, s(&["a", "b"]));
        // Echte Ausgabe mit Zeilenende bleibt
        let mut l = s(&["100%"]);
        strip_eol_mark(&mut l, false);
        assert_eq!(l, s(&["100%"]));
        // Nur die Markierung: Zeile fällt weg
        let mut l = s(&["%"]);
        strip_eol_mark(&mut l, true);
        assert!(l.is_empty());
    }

    #[test]
    fn suffix_names_the_shell() {
        use crate::state::Lang;
        assert_eq!(login_suffix(known("/usr/bin/zsh", true).as_ref(), Lang::En), " (zsh)");
        assert_eq!(login_suffix(known("/usr/bin/nu", true).as_ref(), Lang::En), " (nu, without integration)");
        assert_eq!(login_suffix(Some(&Login::Failed(Instant::now())), Lang::De), " (Shell unbekannt)");
        assert_eq!(login_suffix(Some(&Login::Pending), Lang::En), "");
        assert_eq!(login_suffix(None, Lang::En), "");
    }
}
