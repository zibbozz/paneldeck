//! Erkennung der installierten Shells (PowerShell, CMD, WSL, Git Bash).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShellSpec {
    pub id: String,
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

impl ShellSpec {
    fn new(id: &str, label: &str, program: &str, args: &[&str]) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[cfg(windows)]
pub fn detect() -> Vec<ShellSpec> {
    let mut v = Vec::new();
    if let Some(p) = which("pwsh.exe") {
        v.push(ShellSpec::new(
            "pwsh",
            "PowerShell 7",
            &p.to_string_lossy(),
            &["-NoLogo"],
        ));
    }
    v.push(ShellSpec::new(
        "powershell",
        "Windows PowerShell",
        "powershell.exe",
        &["-NoLogo"],
    ));
    v.push(ShellSpec::new("cmd", "Eingabeaufforderung (CMD)", "cmd.exe", &[]));

    // Git Bash
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            let base = std::path::PathBuf::from(base);
            candidates.push(base.join("Git").join("bin").join("bash.exe"));
            candidates.push(base.join("Programs").join("Git").join("bin").join("bash.exe"));
        }
    }
    if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
        v.push(ShellSpec::new(
            "gitbash",
            "Git Bash",
            &p.to_string_lossy(),
            &["--login", "-i"],
        ));
    }

    // WSL-Distributionen
    v.extend(detect_wsl());
    v
}

#[cfg(windows)]
fn detect_wsl() -> Vec<ShellSpec> {
    // Mit Zeitlimit (läuft beim Start im UI-Thread, #221); wsl.exe gibt UTF-16LE aus (mit WSL_UTF8=1 UTF-8),
    // `wsl::parse_distro_list` kann beides.
    crate::wsl::list_distros(crate::wsl::FAST_TIMEOUT)
        .unwrap_or_default()
        .iter()
        .map(|name| {
            ShellSpec::new(
                &format!("wsl:{name}"),
                &format!("WSL: {name}"),
                "wsl.exe",
                &["-d", name],
            )
        })
        .collect()
}

#[cfg(not(windows))]
pub fn detect() -> Vec<ShellSpec> {
    // Nur für Entwicklung/Tests unter Linux.
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let _ = which("sh");
    vec![ShellSpec::new("sh", "Shell", &sh, &[])]
}

/// PowerShell-Init: umhüllt die vorhandene `prompt`-Funktion (Profile bleiben unangetastet) und sendet
/// Befehlsende (Exit-Code, Dauer) und aktuellen Ordner als OSC-Sequenzen. Ohne Anführungszeichen, damit das Quoting robust bleibt.
/// Exit-Code: nach einem Fehler (`$?` falsch) stammt er von einem Cmdlet (neues Objekt in `$Error[0]` → 1) oder von einem
/// nativen Programm (`$LASTEXITCODE`, auch wenn derselbe Wert schon beim vorigen Befehl stand).
const PS_INIT: &str = "$global:__hub_op=$function:prompt;function global:prompt{$ec=$?;$lec=$global:LASTEXITCODE;$er=if($Error.Count){$Error[0]}else{$null};$e=[char]27;$b=[char]7;$o='';$h=Get-History -Count 1;if($h -and $h.Id -ne $global:__hub_hid){$global:__hub_hid=$h.Id;$ms=[int](($h.EndExecutionTime-$h.StartExecutionTime).TotalMilliseconds);$o+=$e+']7777;dur='+$ms+$b};$c=0;if(-not $ec){if($er -and -not [object]::ReferenceEquals($er,$global:__hub_er)){$c=1}elseif($lec){$c=$lec}else{$c=1}};$global:__hub_er=$er;$o+=$e+']133;D;'+$c+$b+$e+']9;9;'+[char]34+$PWD.ProviderPath+[char]34+$b+$e+']133;A'+$b;$p=if($global:__hub_op){& $global:__hub_op}else{'PS '+$PWD+'> '};$o+$p+$e+']133;B'+$b}";

/// Bash: wird per Umgebungsvariable PROMPT_COMMAND aktiviert (kein Eingriff in .bashrc). Sendet Befehlsende, OSC 7 und
/// OSC 9;9 (Ordner, ohne Steuerzeichen; 9;9 zuletzt, unter Git Bash als Windows-Pfad) und den Prompt-Anfang; das
/// Prompt-Ende (133;B) hängt es einmal an PS1 (#57).
pub(crate) const BASH_PC: &str = r#"__hub_ec=$?; __hub_pc(){ local p="${PWD//[[:cntrl:]]/}" u; u="${p//\%/%25}"; u="${u// /%20}"; printf '\033]133;D;%s\007\033]7;file://%s%s\007\033]9;9;"%s"\007\033]133;A\007' "$1" "${HOSTNAME//[[:cntrl:]]/}" "$u" "$(cygpath -w "$p" 2>/dev/null || printf %s "$p")"; [[ $PS1 == *'133;B'* ]] || PS1="$PS1"'\[\e]133;B\a\]'; }; __hub_pc "$__hub_ec"; (exit $__hub_ec)"#;
/// Wie [`BASH_PC`] für WSL mit noch unbekannter Login-Shell: entfernt die nur für fish gesetzten Variablen
/// (`XDG_DATA_DIRS` mit unserem Eintrag, `PANELDECK_XDG`) wieder aus der bash-Sitzung.
pub(crate) const BASH_PC_XDG: &str = r#"__hub_ec=$?; [ -n "$PANELDECK_XDG" ] && { [ "$XDG_DATA_DIRS" = "$PANELDECK_XDG" ] && unset XDG_DATA_DIRS; unset PANELDECK_XDG; }; __hub_pc(){ local p="${PWD//[[:cntrl:]]/}" u; u="${p//\%/%25}"; u="${u// /%20}"; printf '\033]133;D;%s\007\033]7;file://%s%s\007\033]9;9;"%s"\007\033]133;A\007' "$1" "${HOSTNAME//[[:cntrl:]]/}" "$u" "$(cygpath -w "$p" 2>/dev/null || printf %s "$p")"; [[ $PS1 == *'133;B'* ]] || PS1="$PS1"'\[\e]133;B\a\]'; }; __hub_pc "$__hub_ec"; (exit $__hub_ec)"#;
/// Bash PS0 wird vor der Ausführung eines Befehls ausgegeben: Marke „Befehl startet“ (OSC 133;C) für die Dauermessung.
pub(crate) const BASH_PS0: &str = "\x1b]133;C\x07";

fn stem_of(program: &str) -> String {
    std::path::Path::new(program).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Distribution einer WSL-Shell als Schlüssel der Login-Shell-Erkennung (`""` = Standard-Distribution).
pub fn wsl_distro_key(shell: &ShellSpec) -> String {
    shell.id.strip_prefix("wsl:").filter(|d| !d.is_empty()).map(str::to_string).or_else(|| wsl_distro(&shell.args)).unwrap_or_default()
}

/// Zusätzliche Startargumente und Umgebungsvariablen für die Shell-Integration (Befehlsmarken, Ordner-Tracking).
/// `data` = Datenordner (Hilfsdateien für zsh/fish), `ctx` = schon gesetzte Variablen (Profil; ein eigenes `ZDOTDIR`
/// bleibt so erhalten) und ob die Umgebung des Hubs vererbt wird. In WSL startet die Abfrage der Login-Shell, falls sie
/// noch nicht bekannt ist (#57).
pub fn integration(shell: &ShellSpec, data: &std::path::Path, ctx: &crate::shellinit::EnvCtx) -> crate::shellinit::Plan {
    use crate::shellinit::{self as si, Flavor, Plan};
    if is_wsl(shell) {
        if is_system_bash(&shell.program) {
            // Der alte WSL-Starter `bash.exe` startet immer bash.
            return si::unix_plan(Some(Flavor::Bash), &[], data, true, false, ctx);
        }
        let login = match si::wsl_target(&shell.args) {
            si::WslTarget::Login | si::WslTarget::Exec(_) => Some(si::ensure_wsl_login(&wsl_distro_key(shell))),
            si::WslTarget::Other => None,
        };
        return si::wsl_plan(&shell.args, login, data, ctx);
    }
    match stem_of(&shell.program).as_str() {
        "pwsh" | "powershell" => {
            // Hat das Profil/der Nutzer schon eigene Befehls-/Skriptargumente, nichts hinzufügen (zwei -Command wären ein Fehler).
            let has = shell.args.iter().any(|a| {
                let l = a.to_lowercase();
                matches!(l.as_str(), "-file" | "-f" | "-command" | "-c" | "-encodedcommand" | "-e" | "-ec" | "-commandwithargs" | "-cwa" | "-noexit" | "-i")
            });
            if has { Plan::default() } else { Plan { args: vec!["-NoExit".into(), "-Command".into(), PS_INIT.into()], flavor: Some(Flavor::Pwsh), ..Default::default() } }
        }
        "cmd" => Plan { env: vec![("PROMPT".into(), "$E]9;9;$P$E\\$P$G".into())], flavor: Some(Flavor::Cmd), ..Default::default() },
        "bash" => si::unix_plan(Some(Flavor::Bash), &shell.args, data, false, false, ctx),
        // zsh/fish als eigenes Programm unter Linux/macOS (unter Windows nur in WSL; MSYS2-zsh gehört nicht dazu).
        s @ ("zsh" | "fish") if !cfg!(windows) => si::unix_plan(Flavor::of_unix_shell(s), &shell.args, data, false, false, ctx),
        _ => Plan::default(),
    }
}

/// Shell, die in der Sitzung tatsächlich läuft, soweit bekannt (WSL: erkannte Login-Shell oder `-e <programm>`); `None`
/// = unbekannt oder eine andere Shell. Startet keine Abfrage.
pub fn running_flavor(shell: &ShellSpec) -> Option<crate::shellinit::Flavor> {
    use crate::shellinit::{self as si, Flavor};
    if is_wsl(shell) {
        if is_system_bash(&shell.program) {
            return Some(Flavor::Bash);
        }
        return match si::wsl_target(&shell.args) {
            si::WslTarget::Exec(i) => Flavor::of_unix_shell(&shell.args[i]),
            si::WslTarget::Login => match si::wsl_login(&wsl_distro_key(shell)) {
                Some(si::Login::Known(i)) => Flavor::of_unix_shell(&i.shell),
                _ => None,
            },
            si::WslTarget::Other => None,
        };
    }
    match stem_of(&shell.program).as_str() {
        "pwsh" | "powershell" => Some(Flavor::Pwsh),
        "cmd" => Some(Flavor::Cmd),
        s => Flavor::of_unix_shell(s),
    }
}

/// Quoting für Befehle, die Paneldeck in diese Shell tippt (Claude-Start): nach der laufenden Shell, sonst nach dem
/// Programm (WSL mit unbekannter Login-Shell: vorsichtiges POSIX ohne `$'…'`).
pub fn quote_kind(shell: &ShellSpec) -> crate::quote::Kind {
    use crate::quote::Kind;
    use crate::shellinit::Flavor;
    match running_flavor(shell) {
        Some(Flavor::Pwsh) => Kind::Pwsh,
        Some(Flavor::Cmd) => Kind::Cmd,
        Some(Flavor::Bash | Flavor::Zsh) => Kind::Bash,
        Some(Flavor::Fish) => Kind::Fish,
        None if is_wsl(shell) => Kind::Posix,
        None => crate::quote::kind_of_program(&shell.program),
    }
}

/// Name in Auswahllisten: bei WSL-Distributionen mit der erkannten Login-Shell, z. B. „WSL: Ubuntu (zsh)“ (#57).
pub fn display_label(shell: &ShellSpec, lang: crate::state::Lang) -> String {
    if is_wsl(shell) && !is_system_bash(&shell.program) && crate::shellinit::wsl_target(&shell.args) == crate::shellinit::WslTarget::Login {
        let login = crate::shellinit::wsl_login(&wsl_distro_key(shell));
        return format!("{}{}", shell.label, crate::shellinit::login_suffix(login.as_ref(), lang));
    }
    shell.label.clone()
}

/// Sieht der Hub, ob im Vordergrund dieser Shell ein Programm läuft (`platform::process::shell_is_idle`)? Nur bei lokalen Shells,
/// deren Programme Windows-Prozesse an derselben Konsole sind: PowerShell, cmd, Git Bash. Nicht in WSL (Linux-Prozesse
/// sieht Windows nicht) und nicht bei eigenen Programmen wie `ssh.exe` (dort läuft alles auf der anderen Seite).
pub fn foreground_visible(shell: &ShellSpec) -> bool {
    let stem = std::path::Path::new(&shell.program).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    cfg!(windows) && !is_wsl(shell) && matches!(stem.as_str(), "pwsh" | "powershell" | "cmd" | "bash")
}

/// Ist das eine WSL-Shell (Kennung `wsl:…`, Programm `wsl.exe` oder das `bash.exe` aus System32)?
pub fn is_wsl(shell: &ShellSpec) -> bool {
    shell.id.starts_with("wsl") || is_wsl_program(&shell.program)
}

/// Startet das Programm WSL: `wsl.exe` oder der WSL-Starter `bash.exe` aus System32/Sysnative/WindowsApps (#261: eine
/// gemeinsame Erkennung für Pfade, Hooks, Umgebung und Status). Git Bash (`…\Git\bin\bash.exe`) ist kein WSL.
pub fn is_wsl_program(program: &str) -> bool {
    let prog = program.replace('/', "\\").to_ascii_lowercase();
    let file = prog.rsplit('\\').next().unwrap_or("");
    let stem = file.strip_suffix(".exe").unwrap_or(file);
    stem == "wsl" || (stem == "bash" && is_system_bash(&prog))
}

/// Der WSL-Starter `bash.exe` (kennt kein `--cd`, startet im Windows-Ordner).
pub fn is_system_bash(program: &str) -> bool {
    let prog = program.replace('/', "\\").to_ascii_lowercase();
    let file = prog.rsplit('\\').next().unwrap_or("");
    file.strip_suffix(".exe").unwrap_or(file) == "bash" && (prog.contains("\\system32\\") || prog.contains("\\sysnative\\") || prog.contains("\\windowsapps\\"))
}

/// Distribution aus `-d <Name>` / `--distribution <Name>` in den Argumenten.
pub fn wsl_distro(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-d" || a == "--distribution" {
            // Aus Profil-Argumenten (Nutzereingabe): nur plausible Namen (keine Option, keine Anführungszeichen …).
            return it.next().filter(|d| crate::wsl::valid_distro(d)).cloned();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(program: &str, args: &[&str]) -> ShellSpec {
        ShellSpec { id: "x".into(), label: "x".into(), program: program.into(), args: args.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn powershell_integration_skipped_with_own_command() {
        let d = std::env::temp_dir();
        assert!(!integration(&spec("pwsh.exe", &["-NoLogo"]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false }).args.is_empty());
        assert!(integration(&spec("pwsh.exe", &["-File", "x.ps1"]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false }).args.is_empty());
        assert!(integration(&spec("powershell.exe", &["-Command", "ssh prod"]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false }).args.is_empty());
    }

    #[test]
    fn powershell_init_is_quote_safe_and_tracks_cmdlet_errors() {
        // Es wird als -Command übergeben: keine doppelten Anführungszeichen (Quoting-Probleme unter Windows).
        assert!(!PS_INIT.contains('"'));
        // Exit-Code-Logik: neuer Fehler in $Error[0] = Cmdlet-Fehler (1), sonst $LASTEXITCODE des nativen Programms.
        assert!(PS_INIT.contains("ReferenceEquals($er,$global:__hub_er)"));
        assert!(PS_INIT.contains("$global:__hub_er=$er"));
    }

    #[test]
    fn wsl_detection() {
        assert!(is_wsl(&spec("wsl.exe", &["-d", "Ubuntu"])));
        assert_eq!(wsl_distro(&["-d".into(), "Ubuntu".into()]).as_deref(), Some("Ubuntu"));
        assert_eq!(wsl_distro(&["-d".into(), "-e".into()]), None);
        assert_eq!(wsl_distro(&["--distribution".into(), "U\" x".into()]), None);
        assert!(!is_wsl(&spec("pwsh.exe", &[])));
        // #261: der WSL-Starter bash.exe aus System32 ist WSL, Git Bash nicht – überall dieselbe Erkennung
        assert!(is_wsl(&spec("C:\\Windows\\System32\\bash.exe", &[])));
        assert!(is_wsl(&spec("c:/windows/sysnative/BASH.EXE", &[])));
        assert!(is_system_bash("C:\\Windows\\System32\\bash.exe") && !is_system_bash("C:\\Windows\\System32\\wsl.exe"));
        assert!(!is_wsl(&spec("C:\\Program Files\\Git\\bin\\bash.exe", &[])));
        assert!(!is_system_bash("C:\\Program Files\\Git\\bin\\bash.exe"));
        if cfg!(windows) {
            use crate::claude_opts::PathStyle;
            use crate::quote::Kind;
            assert_eq!(PathStyle::for_shell("custom:1", "C:\\Windows\\System32\\bash.exe", Kind::Posix), PathStyle::Wsl);
            assert_eq!(PathStyle::for_shell("custom:1", "C:\\Program Files\\Git\\bin\\bash.exe", Kind::Posix), PathStyle::Slash);
        }
    }

    #[test]
    fn only_local_shells_show_their_foreground_program() {
        let named = |id: &str, program: &str| ShellSpec { id: id.into(), label: "x".into(), program: program.into(), args: vec![] };
        if cfg!(windows) {
            assert!(foreground_visible(&named("pwsh", "C:\\Program Files\\PowerShell\\7\\pwsh.exe")));
            assert!(foreground_visible(&named("powershell", "powershell.exe")));
            assert!(foreground_visible(&named("cmd", "cmd.exe")));
            assert!(foreground_visible(&named("gitbash", "C:\\Program Files\\Git\\bin\\bash.exe")));
        }
        // WSL (auch über bash.exe aus System32 oder ein eigenes Profil) und fremde Programme nie
        assert!(!foreground_visible(&named("wsl:Ubuntu", "wsl.exe")));
        assert!(!foreground_visible(&named("custom:1", "C:\\Windows\\System32\\bash.exe")));
        assert!(!foreground_visible(&named("custom:2", "wsl.exe")));
        assert!(!foreground_visible(&named("custom:3", "ssh.exe")));
        assert!(!foreground_visible(&named("custom:4", "nu.exe")));
    }

    #[test]
    fn bash_hook_keeps_exit_status_first() {
        assert!(BASH_PC.starts_with("__hub_ec=$?;"));
        assert!(BASH_PC.ends_with("(exit $__hub_ec)"));
        assert!(!BASH_PC.contains('\u{1b}'));
        // #57: Ordner ohne Steuerzeichen, OSC 7 vor 9;9, Prompt-Ende 133;B einmal an PS1
        for pc in [BASH_PC, BASH_PC_XDG] {
            assert!(pc.starts_with("__hub_ec=$?;") && pc.ends_with("(exit $__hub_ec)"));
            assert!(pc.contains("${PWD//[[:cntrl:]]/}") && pc.contains("]7;file://%s%s") && pc.contains("133;B"));
            assert!(pc.find("]7;file").unwrap() < pc.find("9;9;").unwrap());
        }
        // Rückfall: entfernt nur unser XDG_DATA_DIRS
        assert!(BASH_PC_XDG.contains(r#"[ "$XDG_DATA_DIRS" = "$PANELDECK_XDG" ] && unset XDG_DATA_DIRS"#));
    }

    #[test]
    fn running_shell_decides_quoting_and_label() {
        use crate::quote::Kind;
        use crate::shellinit::{set_login, Flavor, Login};
        use crate::state::Lang;
        let wsl = |d: &str, args: &[&str]| ShellSpec { id: format!("wsl:{d}"), label: format!("WSL: {d}"), program: "wsl.exe".into(), args: args.iter().map(|s| s.to_string()).collect() };
        // Eigene Distributionsnamen je Fall (der Speicher ist global)
        set_login("qk-zsh", Login::Known(crate::shellinit::LoginInfo { shell: "/usr/bin/zsh".into(), zsh_ok: true, shells: vec![] }));
        set_login("qk-fish", Login::Known(crate::shellinit::LoginInfo { shell: "/usr/bin/fish".into(), zsh_ok: false, shells: vec![] }));
        set_login("qk-nu", Login::Known(crate::shellinit::LoginInfo { shell: "/usr/bin/nu".into(), zsh_ok: false, shells: vec![] }));
        set_login("qk-fail", Login::Failed(std::time::Instant::now()));
        set_login("qk-wait", Login::Pending);
        let z = wsl("qk-zsh", &["-d", "qk-zsh"]);
        assert_eq!(running_flavor(&z), Some(Flavor::Zsh));
        assert_eq!(quote_kind(&z), Kind::Bash);
        assert_eq!(display_label(&z, Lang::En), "WSL: qk-zsh (zsh)");
        let f = wsl("qk-fish", &["-d", "qk-fish"]);
        assert_eq!(quote_kind(&f), Kind::Fish);
        assert_eq!(display_label(&f, Lang::De), "WSL: qk-fish (fish)");
        assert_eq!(quote_kind(&wsl("qk-nu", &["-d", "qk-nu"])), Kind::Posix);
        assert_eq!(display_label(&wsl("qk-nu", &[]), Lang::En), "WSL: qk-nu (nu, without integration)");
        assert_eq!(display_label(&wsl("qk-fail", &[]), Lang::En), "WSL: qk-fail (shell unknown)");
        assert_eq!(quote_kind(&wsl("qk-fail", &[])), Kind::Posix, "unbekannt: kein $'…'");
        assert_eq!(display_label(&wsl("qk-wait", &[]), Lang::En), "WSL: qk-wait");
        // `-e <programm>` im Profil gewinnt über die Login-Shell
        let e = ShellSpec { id: "custom:1".into(), label: "x".into(), program: "wsl.exe".into(), args: vec!["-d".into(), "qk-zsh".into(), "-e".into(), "fish".into()] };
        assert_eq!(quote_kind(&e), Kind::Fish);
        assert_eq!(display_label(&e, Lang::En), "x");
        // Lokale Shells nach dem Programm
        let p = |program: &str| ShellSpec { id: "x".into(), label: "x".into(), program: program.into(), args: vec![] };
        assert_eq!(quote_kind(&p("pwsh.exe")), Kind::Pwsh);
        assert_eq!(quote_kind(&p("cmd.exe")), Kind::Cmd);
        assert_eq!(quote_kind(&p("/bin/zsh")), Kind::Bash);
        assert_eq!(quote_kind(&p("/usr/bin/fish")), Kind::Fish);
        assert_eq!(quote_kind(&p("ssh.exe")), Kind::Posix);
        assert_eq!(quote_kind(&p("C:\\Windows\\System32\\bash.exe")), Kind::Bash);
    }

    #[test]
    fn integration_per_shell() {
        use crate::shellinit::{set_login, Flavor, Login, ENV_FISH, FISH_INIT_COMMAND};
        let d = std::env::temp_dir().join(format!("pd-shells-int-{}", std::process::id()));
        let p = |id: &str, program: &str, args: &[&str]| ShellSpec { id: id.into(), label: "x".into(), program: program.into(), args: args.iter().map(|s| s.to_string()).collect() };
        let names = |pl: &crate::shellinit::Plan| pl.env.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        assert_eq!(integration(&p("cmd", "cmd.exe", &[]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false }).flavor, Some(Flavor::Cmd));
        let bash = if cfg!(windows) { "C:\\Program Files\\Git\\bin\\bash.exe" } else { "/usr/bin/bash" };
        let gb = integration(&p("gitbash", bash, &["--login", "-i"]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false });
        assert_eq!(names(&gb), ["PROMPT_COMMAND", "PS0"]);
        assert_eq!(integration(&p("x", "nu.exe", &[]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false }), crate::shellinit::Plan::default());
        if cfg!(windows) {
            // WSL: bekannte Login-Shell
            set_login("it-zsh", Login::Known(crate::shellinit::LoginInfo { shell: "/usr/bin/zsh".into(), zsh_ok: true, shells: vec![] }));
            assert_eq!(names(&integration(&p("wsl:it-zsh", "wsl.exe", &["-d", "it-zsh"]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false })), ["ZDOTDIR"]);
            set_login("it-fish", Login::Known(crate::shellinit::LoginInfo { shell: "/usr/bin/fish".into(), zsh_ok: false, shells: vec![] }));
            let f = integration(&p("wsl:it-fish", "wsl.exe", &["--cd", "~", "-d", "it-fish"]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false });
            assert_eq!(f.args, ["-e", "/usr/bin/fish", "-l", "--init-command", FISH_INIT_COMMAND]);
            assert_eq!(names(&f), [ENV_FISH]);
            // Alter WSL-Starter bash.exe: bash
            assert_eq!(names(&integration(&p("custom:1", "C:\\Windows\\System32\\bash.exe", &[]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false })), ["PROMPT_COMMAND", "PS0"]);
        } else {
            // Linux/macOS: zsh und fish als Programm
            assert_eq!(integration(&p("sh", "/bin/zsh", &["-l"]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false }).flavor, Some(Flavor::Zsh));
            assert_eq!(integration(&p("sh", "/usr/bin/fish", &[]), &d, &crate::shellinit::EnvCtx { env: &[], inherit: false }).args, ["--init-command", FISH_INIT_COMMAND]);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
