//! Klickbare Links und Pfade im Terminal (Strg+Klick).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// Spaltenbereich im Text (`start..end`)
    pub start: usize,
    pub end: usize,
    /// Was geöffnet wird: URL oder Dateipfad (Windows-Schreibweise).
    pub target: String,
}

fn is_sep(c: char) -> bool {
    c.is_whitespace() || c == '\0' || "\"'<>()[]{}|,;`".contains(c)
}

fn trim_end_punct(chars: &[char], mut end: usize, start: usize) -> usize {
    while end > start && ".,:;!?".contains(chars[end - 1]) {
        end -= 1;
    }
    end
}

fn strip_line_col(s: &str) -> &str {
    // ":12" oder ":12:5" am Ende entfernen; "C:" (Laufwerk) bleibt unangetastet.
    let mut cur = s;
    for _ in 0..2 {
        if let Some(i) = cur.rfind(':') {
            let tail = &cur[i + 1..];
            if i > 1 && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                cur = &cur[..i];
                continue;
            }
        }
        break;
    }
    cur
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

fn resolve(token: &str, cwd: &str, shell_id: &str) -> Option<String> {
    let t = strip_line_col(token);
    let b = t.as_bytes();
    let is_drive = b.len() >= 3 && b[1] == b':' && b[0].is_ascii_alphabetic() && (b[2] == b'\\' || b[2] == b'/');
    // UNC-Pfade (außer lokalen WSL-Freigaben) nicht anfassen: schon das Prüfen würde eine SMB-Verbindung aufbauen
    // (NTLM-Hash-Leak, UI-Hänger) – allein durch Text im Terminal.
    if crate::pathfmt::is_remote_unc(t) {
        return None;
    }
    let path: PathBuf = if is_drive || t.starts_with("\\\\") {
        PathBuf::from(t)
    } else if shell_id.starts_with("wsl") && t.starts_with('/') {
        PathBuf::from(crate::pathfmt::from_wsl(shell_id, t))
    } else if t.starts_with('/') && t.len() > 1 {
        PathBuf::from(t)
    } else if let Some(rest) = t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
        if shell_id.starts_with("wsl") {
            return None;
        }
        home()?.join(rest)
    } else if t.contains('/') || t.contains('\\') || t.contains('.') {
        let base = if shell_id.starts_with("wsl") && cwd.starts_with('/') { crate::pathfmt::from_wsl(shell_id, cwd) } else { cwd.to_string() };
        Path::new(&base).join(t)
    } else {
        return None;
    };
    // Endgültigen Pfad nochmals prüfen (relativer Pfad auf einem UNC-Arbeitsordner, `//server/…` usw.).
    if crate::pathfmt::is_remote_unc(&path.to_string_lossy()) {
        return None;
    }
    if path.exists() {
        Some(path.to_string_lossy().to_string())
    } else {
        None
    }
}

/// Link unter Spalte `col` der Zeile `chars` finden (URL oder existierender Pfad).
pub fn find(chars: &[char], col: usize, cwd: &str, shell_id: &str) -> Option<Link> {
    if col >= chars.len() || is_sep(chars[col]) {
        return None;
    }
    let mut start = col;
    while start > 0 && !is_sep(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col + 1;
    while end < chars.len() && !is_sep(chars[end]) {
        end += 1;
    }
    let end = trim_end_punct(chars, end, start);
    if end <= col {
        return None;
    }
    let token: String = chars[start..end].iter().collect();
    let lower = token.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(Link { start, end, target: token });
    }
    if lower.starts_with("www.") && token.len() > 6 {
        return Some(Link { start, end, target: format!("https://{token}") });
    }
    let token = token.strip_prefix("file:///").map(|t| t.to_string()).unwrap_or(token);
    resolve(&token, cwd, shell_id).map(|target| Link { start, end, target })
}

/// Dateiendungen, die ausgeführt werden könnten: sie werden NIE per Klick gestartet, nur im Ordner angezeigt.
const EXECUTABLE: &[&str] = &[
    "exe", "com", "bat", "cmd", "msi", "msp", "lnk", "pif", "scr", "hta", "vbs", "vbe", "js", "jse", "wsf", "wsh", "ps1", "psm1",
    "reg", "jar", "cpl", "dll", "appx", "msix", "sh", "run", "appimage", "url", "gadget", "application",
];

pub fn is_executable_like(target: &str) -> bool {
    if target.starts_with("http://") || target.starts_with("https://") {
        return false;
    }
    Path::new(target).extension().is_some_and(|e| EXECUTABLE.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Mit dem Standardprogramm öffnen (URL im Browser, Datei in der zugeordneten App, Ordner im Explorer).
/// Ausführbare Dateitypen werden stattdessen im Ordner angezeigt (Rückgabe `false` = nicht geöffnet, nur gezeigt).
pub fn open(target: &str) -> bool {
    if is_executable_like(target) {
        crate::platform::open::reveal(target);
        return false;
    }
    crate::platform::open::launch(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn urls() {
        let l = chars("siehe https://example.com/a?b=1, danke");
        let k = find(&l, 12, "", "powershell").unwrap();
        assert_eq!(k.target, "https://example.com/a?b=1");
        assert_eq!((k.start, k.end), (6, 31));
        assert!(find(&l, 2, "", "powershell").is_none());
        assert_eq!(find(&chars("www.rust-lang.org"), 3, "", "x").unwrap().target, "https://www.rust-lang.org");
    }

    #[test]
    fn paths_with_line_numbers() {
        let dir = std::env::temp_dir().join(format!("links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "x").unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let l = chars("error at src/main.rs:12:5 here");
        let k = find(&l, 12, &cwd, "sh").unwrap();
        assert!(k.target.ends_with("main.rs"), "{}", k.target);
        assert!(find(&chars("nichts/dort.txt"), 3, &cwd, "sh").is_none());
        // absoluter Pfad
        let abs = dir.join("src").join("main.rs").to_string_lossy().to_string();
        assert_eq!(find(&chars(&abs), 5, "", "sh").unwrap().target, abs);
    }

    #[test]
    fn executables_are_never_launched_and_unc_is_ignored() {
        assert!(is_executable_like("C:\\x\\setup.BAT"));
        assert!(is_executable_like("/tmp/run.sh"));
        assert!(!is_executable_like("https://example.com/a.exe"));
        assert!(!is_executable_like("/tmp/readme.md"));
        assert!(find(&chars("\\\\evil\\share\\x.txt"), 4, "", "powershell").is_none());
    }

    #[test]
    fn strip_suffix() {
        assert_eq!(strip_line_col("a.rs:10:3"), "a.rs");
        assert_eq!(strip_line_col("C:\\x\\a.rs:10"), "C:\\x\\a.rs");
        assert_eq!(strip_line_col("C:\\x"), "C:\\x");
    }
}
