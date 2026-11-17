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
    !(s.starts_with(r"\\wsl$\") || s.starts_with(r"\\wsl.localhost\"))
}

/// Pfad für die Shell umrechnen (ohne Quoting).
pub fn convert(shell_id: &str, path: &str) -> String {
    if shell_id.starts_with("wsl") {
        match drive_split(path) {
            Some((d, rest)) => format!("/mnt/{}{}", d.to_ascii_lowercase(), rest.replace('\\', "/")),
            None => path.replace('\\', "/"),
        }
    } else if shell_id == "gitbash" {
        match drive_split(path) {
            Some((d, rest)) => format!("/{}{}", d.to_ascii_lowercase(), rest.replace('\\', "/")),
            None => path.replace('\\', "/"),
        }
    } else {
        path.to_string()
    }
}

/// Pfad so formatieren, dass er in der Shell `shell_id` direkt eingefügt werden kann.
pub fn for_shell(shell_id: &str, path: &str) -> String {
    crate::quote::quote_if_needed(crate::quote::kind_of_id(shell_id), &convert(shell_id, path))
}

/// Linux-Pfad aus einer WSL-Distribution als Windows-UNC-Pfad (`\\wsl$\<Distro>\…`).
/// Andere Pfade bleiben unverändert.
pub fn from_wsl(shell_id: &str, path: &str) -> String {
    match shell_id.strip_prefix("wsl:") {
        Some(distro) if path.starts_with('/') => format!(r"\\wsl$\{distro}{}", path.replace('/', "\\")),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(for_shell("powershell", r"C:\Users\Max\a.png"), r"C:\Users\Max\a.png");
        assert_eq!(for_shell("cmd", r"C:\Program Files\x.txt"), "\"C:\\Program Files\\x.txt\"");
        assert_eq!(for_shell("wsl:Ubuntu", r"C:\Users\Max\a.png"), "/mnt/c/Users/Max/a.png");
        assert_eq!(for_shell("gitbash", r"D:\Dev\my app\f.rs"), "'/d/Dev/my app/f.rs'");
        assert_eq!(for_shell("sh", "/home/u/a b"), "'/home/u/a b'");
        assert_eq!(from_wsl("wsl:Ubuntu", "/home/max/.claude/x.jsonl"), r"\\wsl$\Ubuntu\home\max\.claude\x.jsonl");
        assert_eq!(from_wsl("powershell", "/home/x"), "/home/x");
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
}
