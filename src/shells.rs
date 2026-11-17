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

/// Bash: wird per Umgebungsvariable PROMPT_COMMAND aktiviert (kein Eingriff in .bashrc).
const BASH_PC: &str = r#"__hub_ec=$?; __hub_pc(){ printf '\033]133;D;%s\007\033]9;9;"%s"\007\033]133;A\007' "$1" "$(cygpath -w "$PWD" 2>/dev/null || printf %s "$PWD")"; }; __hub_pc "$__hub_ec"; (exit $__hub_ec)"#;
/// Bash PS0 wird vor der Ausführung eines Befehls ausgegeben: Marke „Befehl startet“ (OSC 133;C) für die Dauermessung.
const BASH_PS0: &str = "\x1b]133;C\x07";

/// Zusätzliche Startargumente und Umgebungsvariablen für die Shell-Integration (Befehlsmarken, Ordner-Tracking).
pub fn integration(shell: &ShellSpec) -> (Vec<String>, Vec<(String, String)>) {
    let prog = std::path::Path::new(&shell.program)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match prog.as_str() {
        "pwsh" | "powershell" => {
            // Hat das Profil/der Nutzer schon eigene Befehls-/Skriptargumente, nichts hinzufügen (zwei -Command wären ein Fehler).
            let has = shell.args.iter().any(|a| {
                let l = a.to_lowercase();
                matches!(l.as_str(), "-file" | "-f" | "-command" | "-c" | "-encodedcommand" | "-e" | "-ec" | "-commandwithargs" | "-cwa" | "-noexit" | "-i")
            });
            if has { (vec![], vec![]) } else { (vec!["-NoExit".into(), "-Command".into(), PS_INIT.into()], vec![]) }
        }
        "cmd" => (vec![], vec![("PROMPT".into(), "$E]9;9;$P$E\\$P$G".into())]),
        "bash" | "wsl" => (vec![], vec![("PROMPT_COMMAND".into(), BASH_PC.into()), ("PS0".into(), BASH_PS0.into())]),
        _ => (vec![], vec![]),
    }
}

/// Ist das eine WSL-Shell (Kennung `wsl:…` oder Programm `wsl.exe`)?
pub fn is_wsl(shell: &ShellSpec) -> bool {
    shell.id.starts_with("wsl")
        || std::path::Path::new(&shell.program).file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("wsl"))
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
        assert!(!integration(&spec("pwsh.exe", &["-NoLogo"])).0.is_empty());
        assert!(integration(&spec("pwsh.exe", &["-File", "x.ps1"])).0.is_empty());
        assert!(integration(&spec("powershell.exe", &["-Command", "ssh prod"])).0.is_empty());
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
    }

    #[test]
    fn bash_hook_keeps_exit_status_first() {
        assert!(BASH_PC.starts_with("__hub_ec=$?;"));
        assert!(BASH_PC.ends_with("(exit $__hub_ec)"));
        assert!(!BASH_PC.contains('\u{1b}'));
    }
}
