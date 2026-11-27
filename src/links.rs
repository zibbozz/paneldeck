//! Klickbare Links und Pfade im Terminal (Strg+Klick).

use crate::targetpath::PathKind;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// Spaltenbereich im Text (`start..end`)
    pub start: usize,
    pub end: usize,
    /// Was geöffnet wird: URL oder Dateipfad (Windows-Schreibweise).
    pub target: String,
    /// Zeile aus `datei:12[:5]` (ab 1), wenn angegeben.
    pub line: Option<usize>,
    /// Stand der Pfad im Terminal in Linux-Schreibweise (`/…`, `~`, Git Bash `/c/…`, #392)? Dann öffnet eine Textdatei
    /// im Editor von Paneldeck an der Zeile.
    pub linux: bool,
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

/// Zeile aus `datei:12` bzw. `datei:12:5` (ab 1).
fn line_of(s: &str) -> Option<usize> {
    let stripped = strip_line_col(s);
    let tail = s.get(stripped.len()..)?.strip_prefix(':')?;
    tail.split(':').next()?.parse().ok().filter(|n| *n > 0)
}

fn home() -> Option<PathBuf> {
    (if cfg!(windows) { std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) } else { std::env::var_os("HOME") }).map(PathBuf::from)
}

/// Pfad im Terminaltext → (Windows-Pfad, in Linux-Schreibweise geschrieben). Linux-Pfade, `~`, `/mnt/c` und Git Bash
/// `/c/` werden nach der Pfadart der Sitzung aufgelöst (#392, `targetpath`); `~` in WSL über das bekannte Home.
fn resolve(token: &str, cwd: &str, kind: &PathKind) -> Option<(String, bool)> {
    let t = strip_line_col(token);
    let b = t.as_bytes();
    let is_drive = b.len() >= 3 && b[1] == b':' && b[0].is_ascii_alphabetic() && (b[2] == b'\\' || b[2] == b'/');
    // UNC-Pfade (außer lokalen WSL-Freigaben) nicht anfassen: schon das Prüfen würde eine SMB-Verbindung aufbauen
    // (NTLM-Hash-Leak, UI-Hänger) – allein durch Text im Terminal.
    if crate::pathfmt::is_remote_unc(t) {
        return None;
    }
    let home_form = t.starts_with("~/") || t.starts_with("~\\");
    let mut linux = false;
    let path: PathBuf = if is_drive || t.starts_with("\\\\") {
        PathBuf::from(t)
    } else if (t.starts_with('/') && t.len() > 1) || home_form {
        match kind {
            PathKind::Native if home_form => home()?.join(&t[2..]),
            PathKind::Native => PathBuf::from(t),
            _ => match kind.to_windows(t) {
                Ok(w) => {
                    linux = kind.is_posix() || !w.eq_ignore_ascii_case(t);
                    PathBuf::from(w)
                }
                // Windows-Shell: `/x` wie bisher (Laufwerk des Programms).
                Err(_) if !kind.is_posix() && t.starts_with('/') => PathBuf::from(t),
                Err(_) => return None,
            },
        }
    } else if t.contains('/') || t.contains('\\') || t.contains('.') {
        let base = if kind.is_posix() && !matches!(kind, PathKind::Native) && (cwd.starts_with('/') || cwd.starts_with('~')) { kind.to_windows(cwd).ok()? } else { cwd.to_string() };
        linux = kind.is_posix() && !matches!(kind, PathKind::Native) && !t.contains('\\');
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
    exists.then_some((p, linux))
}

/// Wünsche „im Editor öffnen“ aus dem Terminal (Pfad, Zeile ab 1); holt die App je Bild ab.
static EDITOR_REQUESTS: std::sync::Mutex<Vec<(String, Option<usize>)>> = std::sync::Mutex::new(Vec::new());

/// Wünsche „im Editor öffnen“ abholen (App, je Bild).
pub fn take_editor_requests() -> Vec<(String, Option<usize>)> {
    std::mem::take(&mut *EDITOR_REQUESTS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Strg+Klick auf einen Link (#392): Ein in Linux-Schreibweise genannter Pfad öffnet als Textdatei im Editor von
/// Paneldeck an der Zeile (Ausführbares nie, Ordner im Explorer); sonst wie bisher mit dem Standardprogramm.
pub fn activate(link: &Link) {
    // WSL-/Netzwerkpfade nie im UI-Thread prüfen (#392): Ordner oder Datei aus der Prüfung, die den Link gefunden hat.
    let slow = crate::wsl::is_wsl_unc(&link.target) || crate::pathfmt::is_remote(&link.target);
    let is_dir = if slow { dir_cached(&link.target) } else { Some(Path::new(&link.target).is_dir()) };
    if link.linux && !link.target.starts_with("http") && !is_executable_like(&link.target) && is_dir == Some(false) {
        EDITOR_REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).push((link.target.clone(), link.line));
        return;
    }
    open(&link.target);
}

/// Prüfungen langsamer Pfade (Netzlaufwerk, `\\wsl$`), die gerade laufen oder kürzlich liefen (#251).
struct SlowCheck {
    result: Option<bool>,
    /// Ist es ein Ordner (mit dem Ergebnis gemessen)?
    dir: bool,
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
    map.insert(path.to_string(), SlowCheck { result: None, dir: false, at: now });
    let p = path.to_string();
    let spawned = std::thread::Builder::new().name("link-check".into()).spawn(move || {
        let m = std::fs::metadata(&p);
        let (r, dir) = (m.is_ok(), m.is_ok_and(|m| m.is_dir()));
        let mut g = SLOW_CHECKS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.get_or_insert_with(Default::default).get_mut(&p) {
            c.result = Some(r);
            c.dir = dir;
            c.at = std::time::Instant::now();
        }
    });
    if spawned.is_err() {
        map.remove(path);
    }
    None
}

/// Ordner laut der letzten Prüfung im Hilfsthread (`None`: nicht geprüft oder läuft noch). Fragt nie selbst.
fn dir_cached(path: &str) -> Option<bool> {
    let g = SLOW_CHECKS.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(path)).and_then(|c| c.result.map(|_| c.dir))
}

/// Läuft gerade eine Pfadprüfung im Hilfsthread? Dann zeichnet das Terminal bald noch einmal, damit der Link erscheint.
pub fn checks_pending() -> bool {
    SLOW_CHECKS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.values().any(|c| c.result.is_none()))
}

/// Link unter Spalte `col` der Zeile `chars` finden (URL oder existierender Pfad); Pfadart nach der Kennung der Shell.
#[cfg_attr(not(test), allow(dead_code))]
pub fn find(chars: &[char], col: usize, cwd: &str, shell_id: &str) -> Option<Link> {
    find_kind(chars, col, cwd, &PathKind::of_id(shell_id))
}

/// Wie [`find`] mit der Pfadart der Sitzung (`targetpath::PathKind::of`, auch eigene Profile).
pub fn find_kind(chars: &[char], col: usize, cwd: &str, kind: &PathKind) -> Option<Link> {
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
        return Some(Link { start, end, target: token, line: None, linux: false });
    }
    if lower.starts_with("www.") && token.len() > 6 {
        return Some(Link { start, end, target: format!("https://{token}"), line: None, linux: false });
    }
    // `file:///home/u/x` (Linux) bzw. `file:///C:/x` (Windows)
    let token = match token.strip_prefix("file:///") {
        Some(t) if kind.is_posix() && !t.as_bytes().get(1).is_some_and(|c| *c == b':') => format!("/{t}"),
        Some(t) => t.to_string(),
        None => token,
    };
    let line = line_of(&token);
    resolve(&token, cwd, kind).map(|(target, linux)| Link { start, end, target, line, linux })
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

    /// #392: Linux-Pfade aus WSL und Git Bash werden zu Windows-Pfaden (`/mnt/c`, `/c/`, `~`), mit Zeile.
    #[test]
    fn linux_paths_resolve_for_the_sessions_path_kind() {
        let dir = std::env::temp_dir().join(format!("links-lin-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "x").unwrap();
        let win = dir.to_string_lossy().to_string();
        if let Some((d, parts)) = crate::wsl::drive_parts(&win) {
            let rest = parts.join("/");
            // WSL: /mnt/<lw>/… (Standard-Einhängepunkt) – Linux-Schreibweise, Zeile 12
            let tok = format!("/mnt/{d}/{rest}/src/main.rs:12:3");
            let k = find_kind(&chars(&tok), 3, "", &PathKind::Wsl { distro: Some("Links-Test-Distro".into()) }).unwrap();
            assert!(k.target.to_lowercase().ends_with("src\\main.rs") && k.linux && k.line == Some(12), "{k:?}");
            // Git Bash: /<lw>/…
            let tok = format!("/{d}/{rest}/src/main.rs:7");
            let k = find_kind(&chars(&tok), 3, "", &PathKind::Msys { root: None }).unwrap();
            assert!(k.linux && k.line == Some(7), "{k:?}");
            // relativ zu einem Linux-Arbeitsordner (WSL) und file:///
            let cwd = format!("/mnt/{d}/{rest}");
            let k = find_kind(&chars("src/main.rs:5"), 2, &cwd, &PathKind::Wsl { distro: Some("Links-Test-Distro".into()) }).unwrap();
            assert!(k.linux && k.line == Some(5), "{k:?}");
            let tok = format!("file:///mnt/{d}/{rest}/src/main.rs");
            assert!(find_kind(&chars(&tok), 10, "", &PathKind::Wsl { distro: Some("Links-Test-Distro".into()) }).is_some_and(|k| k.linux));
            // Windows-Pfad in einer Windows-Sitzung: wie bisher, nicht im Editor
            let abs = dir.join("src").join("main.rs").to_string_lossy().to_string();
            let k = find(&chars(&abs), 3, "", "powershell").unwrap();
            assert!(!k.linux && k.line.is_none());
        }
        // ~ in WSL ohne bekanntes Home: kein Link (und keine Prüfung)
        assert!(find_kind(&chars("~/x.txt"), 2, "", &PathKind::Wsl { distro: Some("Unbekannt-Distro".into()) }).is_none());
        assert_eq!(line_of("a.rs:10:3"), Some(10));
        assert_eq!(line_of("a.rs"), None);
        assert_eq!(line_of(r"C:\x\a.rs:0"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_suffix() {
        assert_eq!(strip_line_col("a.rs:10:3"), "a.rs");
        assert_eq!(strip_line_col("C:\\x\\a.rs:10"), "C:\\x\\a.rs");
        assert_eq!(strip_line_col("C:\\x"), "C:\\x");
    }
}
