//! Pfade für die jeweilige Shell aufbereiten (Quoting, WSL-/Git-Bash-Umrechnung).

fn drive_split(p: &str) -> Option<(char, &str)> {
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some((b[0] as char, &p[2..]))
    } else {
        None
    }
}

/// Netzwerk- oder Gerätepfad (UNC, `//server/…`, `\\?\UNC\…`, `\??\UNC\…`, `\\.\GLOBALROOT\Device\Mup\…` usw.), der kein
/// lokaler Laufwerks- oder WSL-Pfad ist? Schon das bloße Prüfen eines solchen Pfads baut eine SMB-Verbindung auf (NTLM-Hash-
/// Leak, Hänger). Gerätepfade (`\\?\`, `\\.\`, `\??\`) gelten nur mit Laufwerksbuchstaben dahinter als lokal (`\\?\C:\…`),
/// andere `\\…`-Pfade nur als WSL-Freigabe (`\\wsl$\…`, `\\wsl.localhost\…`) – alles andere (auch `\\.\pipe\…`) gilt als
/// entfernt. Übrige Pfade (`C:\…`, relativ, `/home/…` aus WSL) sind lokal.
pub fn is_remote_unc(p: &str) -> bool {
    let s = p.replace('/', "\\").to_ascii_lowercase();
    let dev = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\\.\")).or_else(|| s.strip_prefix(r"\??\"));
    if let Some(rest) = dev {
        let b = rest.as_bytes();
        return !(b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\'));
    }
    if !s.starts_with(r"\\") {
        return false;
    }
    // `\\wsl$`/`\\wsl.localhost` sind nur mit installiertem WSL lokal (#392) – sonst wäre es ein Rechner im Netz.
    !((s.starts_with(r"\\wsl$\") || s.starts_with(r"\\wsl.localhost\")) && wsl_installed())
}

/// Ist WSL installiert (`%SystemRoot%\System32\wsl.exe`; einmal geprüft, lokal)? Außerhalb von Windows: `true`
/// (dort gibt es keine UNC-Freigaben).
pub fn wsl_installed() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *C.get_or_init(|| {
        if !cfg!(windows) {
            return true;
        }
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        std::path::Path::new(&root).join("System32").join("wsl.exe").is_file()
    })
}

/// Netzwerkpfad im weiteren Sinn: [`is_remote_unc`] oder ein Pfad auf einem verbundenen Netzlaufwerk (`Z:\…`,
/// `GetDriveTypeW` = `DRIVE_REMOTE`; die Abfrage selbst geht nicht ins Netz). Für Prüfungen, die den UI-Thread
/// blockieren könnten (#216).
pub fn is_remote(p: &str) -> bool {
    is_remote_unc(p) || remote_drive(p)
}

fn remote_drive(p: &str) -> bool {
    if !crate::platform::fs::DRIVE_LETTERS {
        return false;
    }
    const DRIVE_REMOTE: u32 = 4;
    let s = p.replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\\.\")).or_else(|| s.strip_prefix(r"\??\")).unwrap_or(&s);
    let Some((d, _)) = drive_split(s) else { return false };
    crate::platform::fs::drive_type(d) == Some(DRIVE_REMOTE)
}

/// Gibt es den Ordner? Ohne den Aufrufer (oft den UI-Thread) lange zu blockieren (#216): Ein `\\wsl$`-Pfad wird nie
/// geprüft (das könnte die WSL-VM starten) – `None`; ein Netzwerkpfad ([`is_remote`], auch Netzlaufwerk) wird in einem Hilfsthread
/// geprüft, auf den höchstens `wait` gewartet wird – kommt die Antwort nicht rechtzeitig, `None` (der Thread läuft
/// allein zu Ende). Lokale Pfade direkt.
pub fn dir_exists_quick(path: &str, wait: std::time::Duration) -> Option<bool> {
    if crate::wsl::is_wsl_unc(path) {
        return None;
    }
    if !is_remote(path) {
        return Some(std::path::Path::new(path).is_dir());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let p = path.to_string();
    std::thread::Builder::new().name("dir-check".into()).spawn(move || {
        let _ = tx.send(std::path::Path::new(&p).is_dir());
    }).ok()?;
    rx.recv_timeout(wait).ok()
}

/// Pfad für die Shell umrechnen (ohne Quoting), nach der Kennung der Shell (`wsl:<D>`, `gitbash`; andere bleiben).
/// Umrechnung über `targetpath` (#392): `C:\x` → `/mnt/c/x` bzw. `/c/x`, `\\wsl.localhost\<D>\x` → `/x`. Lässt sich ein
/// Pfad nicht umrechnen (andere Distribution, Netzwerkfreigabe), bleibt er beim Linux-Teil bzw. mit `/` statt `\`.
pub fn convert(shell_id: &str, path: &str) -> String {
    use crate::targetpath::PathKind;
    let kind = if let Some(d) = shell_id.strip_prefix("wsl:") {
        PathKind::Wsl { distro: Some(d.to_string()).filter(|d| crate::wsl::valid_distro(d)) }
    } else if shell_id.starts_with("wsl") {
        PathKind::Wsl { distro: None }
    } else if shell_id == "gitbash" {
        PathKind::Msys { root: None }
    } else {
        return path.to_string();
    };
    match kind.to_target(path) {
        Ok(p) => p,
        Err(_) => match crate::wsl::unc_to_linux(path) {
            Some((_, l)) if matches!(kind, PathKind::Wsl { .. }) => l,
            _ => match drive_split(path) {
                Some((d, rest)) if matches!(kind, PathKind::Wsl { .. }) => format!("/mnt/{}{}", d.to_ascii_lowercase(), rest.replace('\\', "/")),
                _ => path.replace('\\', "/"),
            },
        },
    }
}

/// Pfad so formatieren, dass er in der Shell der Sitzung direkt eingefügt werden kann. Gequotet wird nach der Shell, die
/// dort wirklich läuft (`shells::quote_kind`: fish maskiert in `'…'` anders als bash, #57). Umgerechnet nach der Pfadart
/// der Shell (`targetpath`, auch eigene Profile); ein Pfad, der sich nicht umrechnen lässt (andere WSL-Distribution),
/// bleibt unverändert – Aufrufer, die das melden wollen, nehmen `targetpath::for_insert`.
pub fn for_shell(shell: &crate::shells::ShellSpec, path: &str) -> String {
    crate::targetpath::for_insert(shell, path).unwrap_or_else(|_| crate::quote::quote_if_needed(crate::shells::quote_kind(shell), path))
}

/// Pfad, den eine Shell meldet (OSC 7/9;9, Hooks), als Windows-Pfad: Linux-Pfad einer WSL-Distribution →
/// `\\wsl.localhost\<Distro>\…` (mit 9P-Zeichen; leere Distribution = die bekannte Standard-Distribution),
/// `/mnt/c/x` → `C:\x`, Git Bash `/c/x` → `C:\x`. Andere Pfade (und solche, die sich nicht umrechnen lassen) bleiben.
pub fn from_wsl(shell_id: &str, path: &str) -> String {
    use crate::targetpath::PathKind;
    let kind = PathKind::of_id(shell_id);
    if !kind.is_posix() || matches!(kind, PathKind::Native) || !path.starts_with('/') {
        return path.to_string();
    }
    kind.to_windows(path).unwrap_or_else(|_| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(id: &str, program: &str) -> crate::shells::ShellSpec {
        crate::shells::ShellSpec { id: id.into(), label: id.into(), program: program.into(), args: vec![] }
    }

    /// Wie eine POSIX-Shell bzw. fish ein Wort liest (nur `'…'`, `\x` außerhalb, Klartext); `None` bei einem Zeichen,
    /// das ein Wort beendet oder etwas auslöst (`;`, Leerzeichen, `$`, …), also bei einer Einschleusung.
    fn read_word(s: &str, fish: bool) -> Option<String> {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '\'' => loop {
                    match it.next()? {
                        '\'' => break,
                        '\\' if fish && matches!(it.peek(), Some('\'') | Some('\\')) => out.push(it.next()?),
                        x => out.push(x),
                    }
                },
                '\\' => out.push(it.next()?),
                c if c.is_whitespace() || ";&|$`#<>()\"".contains(c) => return None,
                c => out.push(c),
            }
        }
        Some(out)
    }

    #[test]
    fn file_names_cannot_inject_commands_in_any_shell() {
        // #57: ein Dateiname, der in POSIX-Quoting für fish eine Einschleusung wäre (`\'` beendet dort nicht das Quoting)
        let name = r"/tmp/x\';touch pwned;#";
        let cases = [
            (sp("sh", "/bin/sh"), false),
            (sp("bash", "/bin/bash"), false),
            (sp("zsh", "/usr/bin/zsh"), false),
            (sp("fish", "/usr/bin/fish"), true),
        ];
        for (s, fish) in cases {
            let q = for_shell(&s, name);
            assert_eq!(read_word(&q, fish).as_deref(), Some(name), "{}: {q}", s.id);
        }
        // fish liest POSIX-Quoting anders: dort wäre es eine Einschleusung (Gegenprobe des Tests)
        assert_ne!(read_word(&crate::quote::quote(crate::quote::Kind::Posix, name), true).as_deref(), Some(name));
        // PowerShell und cmd: ein Argument in Anführungszeichen ohne Ende darin
        let pw = for_shell(&sp("pwsh", "pwsh.exe"), r"C:\x\';touch pwned;#");
        assert_eq!(pw, r"'C:\x\'';touch pwned;#'");
        let cm = for_shell(&sp("cmd", "cmd.exe"), r"C:\x\';touch pwned;#");
        assert_eq!(cm, "\"C:\\x\\';touch pwned;#\"");
        // WSL mit fish als Login-Shell: Quoting für fish
        crate::shellinit::set_login("pf-fish", crate::shellinit::Login::Known(crate::shellinit::LoginInfo { shell: "/usr/bin/fish".into(), ..Default::default() }));
        let w = crate::shells::ShellSpec { id: "wsl:pf-fish".into(), label: "x".into(), program: "wsl.exe".into(), args: vec!["-d".into(), "pf-fish".into()] };
        // (für WSL werden `\` eines Windows-Pfads zu `/`; ein `'` bleibt und wird für fish maskiert)
        let fname = "/tmp/x';touch pwned;#";
        assert_eq!(read_word(&for_shell(&w, fname), true).as_deref(), Some(fname));
        assert_eq!(for_shell(&w, fname), r"'/tmp/x\';touch pwned;#'");
    }

    #[test]
    fn conversions() {
        assert_eq!(for_shell(&sp("powershell", "powershell.exe"), r"C:\Users\Max\a.png"), r"C:\Users\Max\a.png");
        assert_eq!(for_shell(&sp("cmd", "cmd.exe"), r"C:\Program Files\x.txt"), "\"C:\\Program Files\\x.txt\"");
        assert_eq!(for_shell(&sp("wsl:Ubuntu", "wsl.exe"), r"C:\Users\Max\a.png"), "/mnt/c/Users/Max/a.png");
        assert_eq!(for_shell(&sp("gitbash", r"C:\Program Files\Git\bin\bash.exe"), r"D:\Dev\my app\f.rs"), "'/d/Dev/my app/f.rs'");
        assert_eq!(for_shell(&sp("sh", "/bin/sh"), "/home/u/a b"), "'/home/u/a b'");
        // #392: wsl.localhost statt wsl$, /mnt/c als Laufwerk, 9P-Zeichen, UNC beim Einfügen als Linux-Pfad
        assert_eq!(from_wsl("wsl:Ubuntu", "/home/max/.claude/x.jsonl"), r"\\wsl.localhost\Ubuntu\home\max\.claude\x.jsonl");
        assert_eq!(from_wsl("wsl:Ubuntu", "/mnt/c/Users/x"), r"C:\Users\x");
        assert_eq!(from_wsl("wsl:Ubuntu", "/a:b"), "\\\\wsl.localhost\\Ubuntu\\a\u{F03A}b");
        assert_eq!(from_wsl("gitbash", "/c/x"), r"C:\x");
        assert_eq!(from_wsl("powershell", "/home/x"), "/home/x");
        assert_eq!(from_wsl("wsl:Ubuntu", "relative"), "relative");
        assert_eq!(convert("wsl:Ubuntu", r"\\wsl.localhost\Ubuntu\home\u\a"), "/home/u/a");
        assert_eq!(convert("wsl", r"\\wsl$\Ubuntu\srv"), "/srv", "Distribution unbekannt: Linux-Teil");
        assert_eq!(convert("gitbash", r"\\wsl.localhost\Ubuntu\home"), "//wsl.localhost/Ubuntu/home");
        assert_eq!(for_shell(&sp("wsl:Ubuntu", "wsl.exe"), r"\\wsl.localhost\Ubuntu\home\u\my file"), "'/home/u/my file'");
        assert!(is_remote_unc(r"\\evil\share\x.txt"));
        assert!(is_remote_unc("//evil/share/x.txt"));
        assert!(is_remote_unc(r"\\wsl.attacker.com\s\x.txt"));
        assert!(is_remote_unc(r"\\?\UNC\evil\s"));
        assert!(!is_remote_unc(r"\\wsl$\Ubuntu\home"));
        assert!(!is_remote_unc(r"\\wsl.localhost\Ubuntu\home"));
        assert!(!is_remote_unc(r"\\?\C:\x"));
        assert!(!is_remote_unc(r"C:\Users\x"));
        assert!(!is_remote_unc("/home/x"));
        // Geräte- und NT-Pfade, die auf Netzwerkfreigaben zeigen können: nie lokal.
        for p in [r"\\.\GLOBALROOT\Device\Mup\evil\s", r"\??\UNC\evil\s", r"\\.\UNC\evil\s", r"\\.\pipe\x", r"\\?\Volume{x}\a", "//?/UNC/evil/s", r"\??\GLOBALROOT\x"] {
            assert!(is_remote_unc(p), "{p}");
        }
        for p in [r"\\.\C:\x", r"\??\C:\x", "//?/c:/x", r"\\?\D:", r"\home\x", r"rel\a"] {
            assert!(!is_remote_unc(p), "{p}");
        }
    }

    #[test]
    fn quick_dir_check_never_waits_long() {
        let ms = std::time::Duration::from_millis;
        let here = std::env::current_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(dir_exists_quick(&here, ms(10)), Some(true));
        assert_eq!(dir_exists_quick(&format!("{here}\\gibt-es-nicht-xyz"), ms(10)), Some(false));
        // WSL-Freigabe: nie geprüft
        assert_eq!(dir_exists_quick(r"\\wsl.localhost\Ubuntu\home", ms(10)), None);
        assert_eq!(dir_exists_quick(r"\\wsl$\Ubuntu\home", ms(10)), None);
        // „Entfernter“ Pfad (laut is_remote_unc): nur im Hilfsthread, höchstens so lange wie erlaubt (Pipe-Namensraum, kein Netz)
        let t0 = std::time::Instant::now();
        let r = dir_exists_quick(r"\\.\pipe\paneldeck-gibt-es-nicht", ms(200));
        assert!(t0.elapsed() < ms(2000), "{:?}", t0.elapsed());
        assert_ne!(r, Some(true));
        // Lokales Laufwerk ist kein Netzlaufwerk (nur mit Laufwerksbuchstaben)
        #[cfg(windows)]
        {
            let drive = &here[..3];
            assert!(!is_remote(drive) && !is_remote(&here) && !is_remote(&format!(r"\\?\{here}")));
        }
        assert!(is_remote(r"\\server\share"));
    }
}
