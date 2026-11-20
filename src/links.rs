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
    (if cfg!(windows) { std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) } else { std::env::var_os("HOME") }).map(PathBuf::from)
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
    let p = path.to_string_lossy().to_string();
    if crate::pathfmt::is_remote_unc(&p) {
        return None;
    }
    // Netzlaufwerke und `\\wsl$` nie im aufrufenden (UI-)Thread prüfen: das läuft bei jedem Bild mit Strg über dem Text
    // und kann dort hängen (#251). Bis die Antwort aus dem Hilfsthread da ist, gibt es keinen Link.
    let exists = if crate::pathfmt::is_remote(&p) || crate::wsl::is_wsl_unc(&p) { exists_cached(&p) == Some(true) } else { path.exists() };
    exists.then_some(p)
}

/// Prüfungen langsamer Pfade (Netzlaufwerk, `\\wsl$`), die gerade laufen oder kürzlich liefen (#251).
struct SlowCheck {
    result: Option<bool>,
    at: std::time::Instant,
}

static SLOW_CHECKS: std::sync::Mutex<Option<std::collections::HashMap<String, SlowCheck>>> = std::sync::Mutex::new(None);
/// So lange gilt eine Antwort; höchstens so viele Prüfungen gleichzeitig; so viele Einträge höchstens.
const SLOW_TTL: std::time::Duration = std::time::Duration::from_secs(5);
const SLOW_PARALLEL: usize = 4;
const SLOW_MAX: usize = 256;

/// Gibt es `path`? Antwort aus dem Zwischenspeicher; fehlt sie (oder ist sie alt), startet eine Prüfung im Hilfsthread
/// und das Ergebnis ist vorerst `None`. Hängt eine Prüfung, bleibt sie bis zu ihrem Ende als laufend stehen (kein neuer
/// Thread für denselben Pfad).
fn exists_cached(path: &str) -> Option<bool> {
    let mut g = SLOW_CHECKS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(Default::default);
    let now = std::time::Instant::now();
    if let Some(c) = map.get(path) {
        match c.result {
            None => return None,
            Some(r) if now.duration_since(c.at) < SLOW_TTL => return Some(r),
            Some(_) => {}
        }
    }
    if map.values().filter(|c| c.result.is_none()).count() >= SLOW_PARALLEL {
        return None;
    }
    if map.len() >= SLOW_MAX {
        map.retain(|_, c| c.result.is_none());
    }
    map.insert(path.to_string(), SlowCheck { result: None, at: now });
    let p = path.to_string();
    let spawned = std::thread::Builder::new().name("link-check".into()).spawn(move || {
        let r = Path::new(&p).exists();
        let mut g = SLOW_CHECKS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.get_or_insert_with(Default::default).get_mut(&p) {
            c.result = Some(r);
            c.at = std::time::Instant::now();
        }
    });
    if spawned.is_err() {
        map.remove(path);
    }
    None
}

/// Läuft gerade eine Pfadprüfung im Hilfsthread? Dann zeichnet das Terminal bald noch einmal, damit der Link erscheint.
pub fn checks_pending() -> bool {
    SLOW_CHECKS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.values().any(|c| c.result.is_none()))
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
    "reg", "jar", "cpl", "dll", "appx", "msix", "sh", "run", "appimage", "url", "gadget", "application", "msc", "chm", "xll",
    "appref-ms", "settingcontent-ms", "diagcab", "library-ms", "searchconnector-ms", "theme", "themepack", "rdp", "wsc", "sct", "inf",
    "scf", "iso", "img", "vhd", "vhdx", "website", "ws", "appinstaller", "msixbundle", "appxbundle", "mde", "mda", "ade", "adp",
    "ins", "isp", "jnlp", "ps1xml", "psc1", "psd1", "pssc", "cdxml", "xbap", "vsix", "msh", "msh1", "msh2", "mshxml",
    // Skriptsprachen, deren Standardzuordnung die Datei ausführt (#251)
    "py", "pyw", "pyz", "pyzw", "pyc", "pyo", "pl", "plx", "rb", "rbw", "ahk", "au3", "a3x", "tcl", "lua", "php", "wsb", "mof",
    // macOS (#150): Bündel, Terminal-Befehle, AppleScript, Installer, Internet-Verknüpfungen, Systemeinstellungen;
    // Shell-Skripte (auch unter Linux)
    "app", "command", "tool", "terminal", "workflow", "scpt", "scptd", "applescript", "pkg", "mpkg", "webloc", "inetloc",
    "fileloc", "prefpane", "zsh", "bash", "csh", "ksh", "fish",
];

/// Endungen aus `PATHEXT` (z. B. `.PY`, wenn Python sich dort eingetragen hat): was die Shell ohne Endung startet, gilt
/// ebenfalls als ausführbar (#251).
fn in_pathext(ext: &str) -> bool {
    std::env::var("PATHEXT").is_ok_and(|v| v.split(';').any(|e| e.trim().trim_start_matches('.').eq_ignore_ascii_case(ext)))
}

pub fn is_executable_like(target: &str) -> bool {
    let lower = target.get(..8).unwrap_or(target).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return false;
    }
    // Windows ignoriert Punkte und Leerzeichen am Ende („a.exe. “ startet a.exe); ein Doppelpunkt hinter dem Laufwerk
    // (alternativer Datenstrom „a.txt:x.exe“) gilt ebenfalls als ausführbar.
    let t = target.trim_end_matches(['.', ' ']);
    if t.char_indices().any(|(i, c)| c == ':' && i > 2) {
        return true;
    }
    Path::new(t).extension().is_some_and(|e| {
        let e = e.to_string_lossy().to_lowercase();
        EXECUTABLE.contains(&e.as_str()) || in_pathext(&e)
    }) || unix_exec_bit(t)
}

/// Linux/macOS: eine Datei mit Ausführungsrecht (auch ohne Endung) wird nie per `open`/`xdg-open` gestartet.
#[cfg(unix)]
fn unix_exec_bit(t: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(t).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn unix_exec_bit(_t: &str) -> bool {
    false
}

/// Mit dem Standardprogramm öffnen (URL im Browser, Datei in der zugeordneten App, Ordner im Explorer).
/// Ausführbare Dateitypen werden stattdessen im Ordner angezeigt (Rückgabe `false` = nicht geöffnet, nur gezeigt).
pub fn open(target: &str) -> bool {
    if is_executable_like(target) {
        reveal(target);
        return false;
    }
    launch(target)
}

/// URI mit seinem Protokoll-Handler öffnen (OSC-8-Links nach der Rückfrage, `hyperlink::open`), ohne die Prüfung auf
/// ausführbare Dateiendungen (die gilt für Pfade, nicht für `mailto:`/`vscode:` usw.).
pub fn launch_uri(uri: &str) {
    launch(uri);
}

fn reveal(target: &str) {
    // Explorer liest Schalter aus dem Argument („/select,a,/root,…“): Kommas und Anführungszeichen nie durchreichen.
    if !reveal_safe(target) {
        return;
    }
    crate::platform::open::reveal(target);
}

fn launch(target: &str) -> bool {
    crate::platform::open::launch(target)
}

/// Darf der Pfad an `explorer.exe /select,<pfad>` gehen? (Keine Kommas/Anführungszeichen, keine Netzwerkpfade.)
fn reveal_safe(target: &str) -> bool {
    !target.contains([',', '"']) && !crate::pathfmt::is_remote_unc(target)
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
        // Punkte/Leerzeichen am Ende und alternative Datenströme täuschen die Endung nicht weg.
        assert!(is_executable_like(r"C:\x\setup.exe."));
        assert!(is_executable_like(r"C:\x\setup.exe . "));
        assert!(is_executable_like(r"C:\x\a.txt:evil.exe"));
        assert!(is_executable_like(r"C:\x\a.settingcontent-ms") && is_executable_like("/x/b.CHM") && is_executable_like("c.library-ms"));
        assert!(!is_executable_like(r"C:\x\notes.txt"));
        // Skripte mit ausführender Standardzuordnung (#251)
        for f in [r"C:\x\tool.py", "a.PYW", "b.pyz", "c.pl", "d.rb", "e.ahk", "f.wsb"] {
            assert!(is_executable_like(f), "{f}");
        }
        assert!(!is_executable_like("notes.md") && !is_executable_like("data.json"));
        // macOS (#150): Bündel und Dateien, die macOS beim Öffnen ausführt
        for f in ["/Applications/Calculator.app", "/x/Foo.app/", "a.command", "b.tool", "c.terminal", "d.workflow", "e.scpt", "f.scptd", "g.applescript", "h.pkg", "i.mpkg", "j.webloc", "k.inetloc", "l.fileloc", "m.prefPane", "n.zsh", "o.bash", "p.csh", "q.ksh", "r.fish"] {
            assert!(is_executable_like(f), "{f}");
        }
        assert!(reveal_safe(r"C:\x\a.exe"));
        assert!(!reveal_safe(r"C:\x\a,/root,\\evil\s\y.exe") && !reveal_safe(r#"C:\x\a"b.exe"#) && !reveal_safe(r"\\evil\s\a.exe"));
    }

    #[test]
    fn slow_paths_are_checked_off_the_calling_thread() {
        let dir = std::env::temp_dir().join(format!("links-slow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.to_string_lossy().to_string();
        assert_eq!(exists_cached(&p), None, "erst im Hilfsthread");
        let t0 = std::time::Instant::now();
        while exists_cached(&p).is_none() && t0.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(exists_cached(&p), Some(true));
        let missing = dir.join("fehlt").to_string_lossy().to_string();
        let _ = exists_cached(&missing);
        let t0 = std::time::Instant::now();
        while exists_cached(&missing).is_none() && t0.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(exists_cached(&missing), Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_suffix() {
        assert_eq!(strip_line_col("a.rs:10:3"), "a.rs");
        assert_eq!(strip_line_col("C:\\x\\a.rs:10"), "C:\\x\\a.rs");
        assert_eq!(strip_line_col("C:\\x"), "C:\\x");
    }
}
