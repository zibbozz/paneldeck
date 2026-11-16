//! WSL-Hilfen (#172): installierte Distributionen, Pfadumrechnung (`\\wsl.localhost\…` ↔ Linux, `C:\…` ↔ `/mnt/c/…`),
//! Prüfung von Linux-Pfaden und sichere `wsl.exe`-Aufrufe (Argumente einzeln, nie als Shell-Zeichenkette).
//!
//! Alles hier ist ohne egui; die Oberfläche dazu ist `app::folder_picker`.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Höchstlänge eines Linux-Pfads (PATH_MAX).
pub const MAX_PATH_LEN: usize = 4096;
/// Zuletzt benutzte WSL-Ordner: so viele je Distribution …
pub const RECENT_MAX: usize = 10;
/// … und so viele Distributionen.
pub const RECENT_DISTROS_MAX: usize = 16;

/// Zeitlimit für die Distributionsliste und das Home-Verzeichnis (der erste Aufruf startet ggf. die WSL-VM).
pub const SLOW_TIMEOUT: Duration = Duration::from_secs(15);
/// Zeitlimit für Vervollständigung und Ordnerprüfung.
pub const FAST_TIMEOUT: Duration = Duration::from_secs(5);

/// Ausgabe von `wsl.exe` dekodieren: die eigenen Meldungen (z. B. `-l -q`) kommen als UTF-16LE (mit oder ohne BOM),
/// mit `WSL_UTF8=1` als UTF-8. UTF-8-Text enthält nie ein Nullbyte, UTF-16 mit ASCII-Namen immer.
pub fn decode_output(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.contains(&0);
    let s = if utf16 {
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    s.replace('\u{feff}', "")
}

/// Namen aus `wsl.exe -l -q` (eine Distribution je Zeile).
pub fn parse_distro_list(bytes: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in decode_output(bytes).lines() {
        let name = l.trim().trim_matches('\0').trim();
        if valid_distro(name) && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// Standard-Distribution aus `wsl.exe -l -v` (Zeile mit `*` vorn; Spaltenköpfe sind übersetzt, `*` nicht).
/// `names` aus `-l -q` erlaubt Namen mit Leerzeichen; ohne Treffer zählt das erste Wort.
pub fn parse_default_distro(bytes: &[u8], names: &[String]) -> Option<String> {
    let text = decode_output(bytes);
    let line = text.lines().map(|l| l.trim_matches('\0').trim()).find(|l| l.starts_with('*'))?;
    let rest = line.trim_start_matches('*').trim_start();
    let hit = names
        .iter()
        .filter(|n| rest.starts_with(n.as_str()) && rest[n.len()..].chars().next().is_none_or(char::is_whitespace))
        .max_by_key(|n| n.len());
    match hit {
        Some(n) => Some(n.clone()),
        None => rest.split_whitespace().next().filter(|n| valid_distro(n)).map(str::to_string),
    }
}

/// Standard-Distribution nach vorn, sonst Reihenfolge von `wsl.exe`.
pub fn order_distros(mut names: Vec<String>, default: Option<&str>) -> Vec<String> {
    if let Some(i) = default.and_then(|d| names.iter().position(|n| n == d)) {
        let d = names.remove(i);
        names.insert(0, d);
    }
    names
}

/// Plausibler Distributionsname für `wsl.exe -d <name>`: nicht leer, keine Steuerzeichen, keine Pfadtrenner,
/// beginnt nicht mit `-` (wäre eine Option).
pub fn valid_distro(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.starts_with('-') && !name.chars().any(|c| c.is_control() || matches!(c, '\\' | '/' | '"'))
}

/// Fehler bei einem Linux-Pfad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    Empty,
    NotAbsolute,
    Control,
    TooLong,
}

impl PathError {
    /// Meldung in beiden Sprachen.
    pub fn message(self, de: bool) -> &'static str {
        match (self, de) {
            (PathError::Empty, true) => "Kein Pfad angegeben.",
            (PathError::Empty, false) => "No path given.",
            (PathError::NotAbsolute, true) => "Linux-Pfad muss mit / beginnen (oder ~ sein).",
            (PathError::NotAbsolute, false) => "A Linux path must start with / (or be ~).",
            (PathError::Control, true) => "Der Pfad enthält Steuerzeichen.",
            (PathError::Control, false) => "The path contains control characters.",
            (PathError::TooLong, true) => "Der Pfad ist zu lang (höchstens 4096 Bytes).",
            (PathError::TooLong, false) => "The path is too long (at most 4096 bytes).",
        }
    }
}

/// Gültiger Startordner für `wsl.exe --cd`: absoluter POSIX-Pfad oder `~` (Home), ohne Steuerzeichen, höchstens
/// [`MAX_PATH_LEN`] Bytes. Ob der Ordner existiert, prüft erst [`is_dir`] (braucht die Distribution).
pub fn validate_linux_path(p: &str) -> Result<(), PathError> {
    if p.is_empty() {
        return Err(PathError::Empty);
    }
    if p.len() > MAX_PATH_LEN {
        return Err(PathError::TooLong);
    }
    if p.chars().any(char::is_control) {
        return Err(PathError::Control);
    }
    if p != "~" && !p.starts_with('/') {
        return Err(PathError::NotAbsolute);
    }
    Ok(())
}

/// Sieht der Text wie ein Linux-Pfad aus (`/…`, `~`, `~/…`)?
pub fn looks_linux(p: &str) -> bool {
    p.starts_with('/') || p == "~" || p.starts_with("~/")
}

/// Lokaler WSL-UNC-Pfad (`\\wsl.localhost\<Distro>\…`, `\\wsl$\<Distro>\…`, auch mit `/` oder `\\?\UNC\…`) →
/// (Distribution, Linux-Pfad). Andere Pfade: `None`.
pub fn unc_to_linux(p: &str) -> Option<(String, String)> {
    let s = p.replace('/', "\\");
    let lower = s.to_ascii_lowercase();
    // Präfixe sind ASCII: gleiche Bytelänge in `s` und `lower`.
    let rest_at = [r"\\?\unc\wsl.localhost\", r"\\?\unc\wsl$\", r"\\wsl.localhost\", r"\\wsl$\"]
        .iter()
        .find(|pre| lower.starts_with(*pre))
        .map(|pre| pre.len())?;
    let rest = &s[rest_at..];
    let (distro, tail) = rest.split_once('\\').unwrap_or((rest, ""));
    if !valid_distro(distro) {
        return None;
    }
    let parts: Vec<&str> = tail.split('\\').filter(|c| !c.is_empty()).collect();
    Some((distro.to_string(), format!("/{}", parts.join("/"))))
}

/// Linux-Pfad als Windows-Pfad über `\\wsl.localhost\<Distro>\…` (für den Ordnerdialog).
pub fn linux_to_unc(distro: &str, linux: &str) -> String {
    let parts: Vec<&str> = linux.split('/').filter(|c| !c.is_empty()).collect();
    if parts.is_empty() {
        format!(r"\\wsl.localhost\{distro}\")
    } else {
        format!(r"\\wsl.localhost\{distro}\{}", parts.join("\\"))
    }
}

/// Windows-Pfad → Linux-Pfad in WSL: `C:\x` → `/mnt/c/x` (Standard-Einhängepunkt), `\\wsl.localhost\D\x` → `/x`.
/// Relative Pfade und Netzwerkfreigaben: `None`.
pub fn windows_to_linux(p: &str) -> Option<String> {
    if let Some((_, l)) = unc_to_linux(p) {
        return Some(l);
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        let parts: Vec<&str> = p[2..].split(['\\', '/']).filter(|c| !c.is_empty()).collect();
        let d = (b[0] as char).to_ascii_lowercase();
        return Some(if parts.is_empty() { format!("/mnt/{d}") } else { format!("/mnt/{d}/{}", parts.join("/")) });
    }
    None
}

/// Linux-Pfad → Windows-Pfad: `/mnt/c/x` → `C:\x`, sonst `\\wsl.localhost\<Distro>\…`. `~` und relative Pfade: `None`.
pub fn linux_to_windows(distro: &str, p: &str) -> Option<String> {
    if !p.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
    if parts.len() >= 2 && parts[0] == "mnt" && parts[1].len() == 1 && parts[1].as_bytes()[0].is_ascii_alphabetic() {
        let d = parts[1].to_ascii_uppercase();
        return Some(format!(r"{d}:\{}", parts[2..].join("\\")));
    }
    if distro.is_empty() {
        return None;
    }
    Some(linux_to_unc(distro, p))
}

/// `~` / `~/x` mit dem Home-Verzeichnis auflösen; absolute Pfade bleiben, sonst `None`.
pub fn expand_home(p: &str, home: Option<&str>) -> Option<String> {
    if p.starts_with('/') {
        return Some(p.to_string());
    }
    let home = home.filter(|h| h.starts_with('/'))?;
    if p == "~" {
        return Some(home.to_string());
    }
    let rest = p.strip_prefix("~/")?;
    Some(format!("{}/{rest}", home.trim_end_matches('/')))
}

/// Pfad für die Ablage: abschließenden `/` entfernen (außer bei `/`), `~/x` mit bekanntem Home auflösen
/// (`wsl.exe --cd` kennt nur `~` allein).
pub fn normalize(p: &str, home: Option<&str>) -> String {
    let p = p.trim();
    let p = if p.len() > 1 { p.trim_end_matches('/') } else { p };
    let p = if p.is_empty() { "/" } else { p };
    if p.starts_with("~/") {
        if let Some(e) = expand_home(p, home) {
            return e;
        }
    }
    p.to_string()
}

/// Text im Eingabefeld → (Ordner, der aufgelistet wird, getippter Namensanfang). `/home/u/pr` → (`/home/u/`, `pr`),
/// `~/x` mit Home `/home/u` → (`/home/u/`, `x`). Ohne absoluten Pfad: `None`.
pub fn split_for_completion(text: &str, home: Option<&str>) -> Option<(String, String)> {
    let t = if text == "~" { "~/" } else { text };
    let full = expand_home(t, home)?;
    let cut = full.rfind('/')? + 1;
    Some((full[..cut].to_string(), full[cut..].to_string()))
}

/// Ausgabe von `ls -1 -p -A` → Unterordner (mit `/` am Ende), die mit `prefix` beginnen. Versteckte Ordner nur,
/// wenn der Anfang mit `.` beginnt. Sortiert, ohne Steuerzeichen, höchstens `max`.
pub fn parse_ls(stdout: &[u8], prefix: &str, max: usize) -> Vec<String> {
    let text = String::from_utf8_lossy(stdout);
    let mut v: Vec<String> = text
        .lines()
        .filter(|l| l.ends_with('/') && l.len() > 1)
        .filter(|l| !l.chars().any(char::is_control))
        .filter(|l| l.starts_with(prefix))
        .filter(|l| !l.starts_with('.') || prefix.starts_with('.'))
        .map(str::to_string)
        .collect();
    v.sort_by_key(|a| a.to_lowercase());
    v.truncate(max);
    v
}

/// `wsl.exe`-Argumente: Home-Verzeichnis ausgeben. Die Shell-Zeichenkette ist fest; die Distribution ist ein eigenes Argument.
pub fn home_args(distro: &str) -> Vec<String> {
    ["-d", distro, "-e", "sh", "-c", "printf '%s' \"$HOME\""].iter().map(|s| s.to_string()).collect()
}

/// `wsl.exe`-Argumente: Ordner auflisten (`-e` = ohne Shell, jedes Argument kommt unverändert in Linux an).
pub fn list_args(distro: &str, dir: &str) -> Vec<String> {
    ["-d", distro, "-e", "ls", "-1", "-p", "-A", "--", dir].iter().map(|s| s.to_string()).collect()
}

/// `wsl.exe`-Argumente: existiert der Ordner?
pub fn isdir_args(distro: &str, dir: &str) -> Vec<String> {
    ["-d", distro, "-e", "test", "-d", dir].iter().map(|s| s.to_string()).collect()
}

/// Startordner einer WSL-Sitzung: `--cd <pfad>` vor die Argumente setzen, wenn `cwd` ein Linux-Pfad ist
/// (leer = `~`, die Vorgabe aus #172). Liefert `true`, wenn der Windows-Startordner dann egal ist.
/// Eigene `--cd`-Angaben eines Profils bleiben unangetastet; Windows-Pfade übersetzt `wsl.exe` selbst.
pub fn apply_cd(args: &mut Vec<String>, cwd: &str) -> bool {
    if args.iter().any(|a| a == "--cd" || a == "~") {
        return false;
    }
    let cwd = cwd.trim();
    let target = if cwd.is_empty() {
        "~".to_string()
    } else if cwd.starts_with("~/") {
        // Ohne Home-Auflösung kennt `--cd` nur `~` allein.
        "~".to_string()
    } else if looks_linux(cwd) && validate_linux_path(cwd).is_ok() {
        cwd.to_string()
    } else {
        return false;
    };
    args.splice(0..0, ["--cd".to_string(), target]);
    true
}

/// Ordner vorn in die Liste der zuletzt benutzten WSL-Ordner der Distribution setzen (ohne Doppel, begrenzt).
pub fn push_recent(map: &mut BTreeMap<String, Vec<String>>, distro: &str, path: &str) {
    if !valid_distro(distro) || validate_linux_path(path).is_err() {
        return;
    }
    if !map.contains_key(distro) && map.len() >= RECENT_DISTROS_MAX {
        if let Some(k) = map.keys().next().cloned() {
            map.remove(&k);
        }
    }
    let v = map.entry(distro.to_string()).or_default();
    v.retain(|p| p != path);
    v.insert(0, path.to_string());
    v.truncate(RECENT_MAX);
}

/// Gelesene Liste bereinigen (settings.json ist eine Vertrauensgrenze): ungültige Einträge weg, Grenzen einhalten.
pub fn sanitize_recent(map: &mut BTreeMap<String, Vec<String>>) {
    map.retain(|d, _| valid_distro(d));
    for v in map.values_mut() {
        v.retain(|p| validate_linux_path(p).is_ok());
        v.truncate(RECENT_MAX);
    }
    map.retain(|_, v| !v.is_empty());
    while map.len() > RECENT_DISTROS_MAX {
        let Some(k) = map.keys().next().cloned() else { break };
        map.remove(&k);
    }
}

/// Zugriff auf WSL – im Programm über `wsl.exe` ([`WslExe`]), in Tests ersetzbar. Alle Aufrufe blockieren und
/// laufen deshalb nur in Hintergrund-Threads.
pub trait Backend: Send + Sync {
    /// Installierte Distributionen, Standard zuerst.
    fn distros(&self) -> Vec<String>;
    /// Home-Verzeichnis des Standardnutzers.
    fn home(&self, distro: &str) -> Option<String>;
    /// Ausgabe von `ls -1 -p -A -- <dir>`; `None` = nicht lesbar.
    fn list(&self, distro: &str, dir: &str) -> Option<Vec<u8>>;
    /// Existiert der Ordner?
    fn is_dir(&self, distro: &str, path: &str) -> bool;
}

/// Echte Abfragen über `wsl.exe`, Distributionen und Home-Verzeichnisse zwischengespeichert.
pub struct WslExe;

fn distro_cache() -> &'static Mutex<Option<Vec<String>>> {
    static C: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn home_cache() -> &'static Mutex<BTreeMap<String, String>> {
    static C: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(BTreeMap::new()))
}

impl Backend for WslExe {
    fn distros(&self) -> Vec<String> {
        if let Some(v) = distro_cache().lock().ok().and_then(|c| c.clone()) {
            return v;
        }
        let names = run(&["-l".into(), "-q".into()], SLOW_TIMEOUT).filter(|(ok, _)| *ok).map(|(_, out)| parse_distro_list(&out)).unwrap_or_default();
        let default = if names.len() > 1 {
            run(&["-l".into(), "-v".into()], SLOW_TIMEOUT).and_then(|(_, out)| parse_default_distro(&out, &names))
        } else {
            None
        };
        let v = order_distros(names, default.as_deref());
        if let Ok(mut c) = distro_cache().lock() {
            *c = Some(v.clone());
        }
        v
    }

    fn home(&self, distro: &str) -> Option<String> {
        if !valid_distro(distro) {
            return None;
        }
        if let Some(h) = home_cache().lock().ok().and_then(|c| c.get(distro).cloned()) {
            return Some(h);
        }
        let (ok, out) = run(&home_args(distro), SLOW_TIMEOUT)?;
        let h = String::from_utf8_lossy(&out).trim().to_string();
        if !ok || validate_linux_path(&h).is_err() || h == "~" {
            return None;
        }
        if let Ok(mut c) = home_cache().lock() {
            c.insert(distro.to_string(), h.clone());
        }
        Some(h)
    }

    fn list(&self, distro: &str, dir: &str) -> Option<Vec<u8>> {
        if !valid_distro(distro) || !dir.starts_with('/') || validate_linux_path(dir).is_err() {
            return None;
        }
        run(&list_args(distro, dir), FAST_TIMEOUT).filter(|(ok, _)| *ok).map(|(_, out)| out)
    }

    fn is_dir(&self, distro: &str, path: &str) -> bool {
        if !valid_distro(distro) || !path.starts_with('/') || validate_linux_path(path).is_err() {
            return false;
        }
        run(&isdir_args(distro, path), FAST_TIMEOUT).is_some_and(|(ok, _)| ok)
    }
}

/// `wsl.exe` mit einzelnen Argumenten starten (kein Fenster, keine Eingabe), Ausgabe lesen (höchstens 1 MB);
/// nach `timeout` wird der Prozess beendet. `None` = nicht startbar oder Zeit überschritten.
#[cfg(windows)]
fn run(args: &[String], timeout: Duration) -> Option<(bool, Vec<u8>)> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut child = Command::new("wsl.exe")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stdout).take(1 << 20).read_to_end(&mut buf);
        buf
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    status.map(|s| (s.success(), out))
}

#[cfg(not(windows))]
fn run(_args: &[String], _timeout: Duration) -> Option<(bool, Vec<u8>)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str, bom: bool) -> Vec<u8> {
        let mut v = if bom { vec![0xFF, 0xFE] } else { Vec::new() };
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn distro_list_utf16_bom_and_utf8() {
        let want = vec!["Ubuntu-24.04".to_string(), "Debian".to_string(), "docker-desktop".to_string()];
        let text = "Ubuntu-24.04\r\nDebian\r\n\r\ndocker-desktop\r\n";
        assert_eq!(parse_distro_list(&utf16(text, false)), want);
        assert_eq!(parse_distro_list(&utf16(text, true)), want);
        assert_eq!(parse_distro_list(text.as_bytes()), want, "WSL_UTF8=1");
        // BOM vor jeder Zeile, Nullbytes, Doppel und Unsinn (Option, Steuerzeichen) fallen weg.
        assert_eq!(parse_distro_list(&utf16("\u{feff}Ubuntu\n\u{feff}Ubuntu\n-x\n", false)), vec!["Ubuntu".to_string()]);
        assert!(parse_distro_list(&[]).is_empty());
        // Ungerade Länge (abgeschnitten): kein Absturz.
        let mut odd = utf16("Arch\n", true);
        odd.push(b'X');
        assert_eq!(parse_distro_list(&odd), vec!["Arch".to_string()]);
    }

    #[test]
    fn default_distro_from_verbose_list() {
        let names = vec!["Ubuntu".to_string(), "Ubuntu-24.04".to_string(), "My Distro".to_string()];
        let v = "  NAME            STATE           VERSION\r\n  Ubuntu          Stopped         2\r\n* Ubuntu-24.04    Running         2\r\n";
        assert_eq!(parse_default_distro(&utf16(v, true), &names).as_deref(), Some("Ubuntu-24.04"));
        // Deutsche Spaltenköpfe, Name mit Leerzeichen
        let v = "  NAME      STATUS   VERSION\n* My Distro Gestoppt 2\n  Ubuntu Gestoppt 2\n";
        assert_eq!(parse_default_distro(v.as_bytes(), &names).as_deref(), Some("My Distro"));
        // Kein Stern: keine Standard-Distribution bekannt
        assert_eq!(parse_default_distro(b"  NAME STATE\n  Ubuntu Running 2\n", &names), None);
        // Unbekannter Name: erstes Wort
        assert_eq!(parse_default_distro(b"* Arch Running 2\n", &names).as_deref(), Some("Arch"));
        let ordered = order_distros(names.clone(), Some("Ubuntu-24.04"));
        assert_eq!(ordered, vec!["Ubuntu-24.04".to_string(), "Ubuntu".to_string(), "My Distro".to_string()]);
        assert_eq!(order_distros(names.clone(), Some("Missing")), names);
    }

    #[test]
    fn unc_to_linux_both_hosts_and_forms() {
        let l = |p: &str| unc_to_linux(p).map(|(d, p)| format!("{d}|{p}"));
        assert_eq!(l(r"\\wsl.localhost\Ubuntu-24.04\home\max\my project").as_deref(), Some("Ubuntu-24.04|/home/max/my project"));
        assert_eq!(l(r"\\wsl$\Ubuntu\home\max\").as_deref(), Some("Ubuntu|/home/max"));
        assert_eq!(l(r"\\WSL.LOCALHOST\Ubuntu").as_deref(), Some("Ubuntu|/"));
        assert_eq!(l(r"\\wsl$\Ubuntu\").as_deref(), Some("Ubuntu|/"));
        assert_eq!(l("//wsl.localhost/Debian/srv/a&b (1)/ä").as_deref(), Some("Debian|/srv/a&b (1)/ä"));
        assert_eq!(l(r"\\?\UNC\wsl.localhost\Ubuntu\tmp").as_deref(), Some("Ubuntu|/tmp"));
        assert_eq!(l(r"\\wsl.localhost\Ubuntu\a\\b").as_deref(), Some("Ubuntu|/a/b"));
        // Keine WSL-Pfade
        assert_eq!(l(r"\\wsl.attacker.com\share\x"), None);
        assert_eq!(l(r"\\server\share"), None);
        assert_eq!(l(r"C:\Users\x"), None);
        assert_eq!(l(r"\\wsl$\"), None);
        assert_eq!(l(r"\\wsl$\-bad\x"), None);
    }

    #[test]
    fn windows_and_linux_round_trip() {
        assert_eq!(windows_to_linux(r"C:\Users\Max\a b").as_deref(), Some("/mnt/c/Users/Max/a b"));
        assert_eq!(windows_to_linux(r"d:\").as_deref(), Some("/mnt/d"));
        assert_eq!(windows_to_linux("E:").as_deref(), Some("/mnt/e"));
        assert_eq!(windows_to_linux(r"C:/x/y/").as_deref(), Some("/mnt/c/x/y"));
        assert_eq!(windows_to_linux(r"C:\$Recycle.Bin\#1 'q'").as_deref(), Some("/mnt/c/$Recycle.Bin/#1 'q'"));
        assert_eq!(windows_to_linux(r"\\wsl$\Ubuntu\home\u").as_deref(), Some("/home/u"));
        assert_eq!(windows_to_linux(r"\\server\share"), None);
        assert_eq!(windows_to_linux("relative\\x"), None);
        assert_eq!(windows_to_linux("/home/u"), None);

        assert_eq!(linux_to_windows("Ubuntu", "/mnt/c/Users/Max/a b").as_deref(), Some(r"C:\Users\Max\a b"));
        assert_eq!(linux_to_windows("Ubuntu", "/mnt/d").as_deref(), Some(r"D:\"));
        assert_eq!(linux_to_windows("Ubuntu", "/mnt/wsl/x").as_deref(), Some(r"\\wsl.localhost\Ubuntu\mnt\wsl\x"));
        assert_eq!(linux_to_windows("Ubuntu", "/home/u/p q").as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\u\p q"));
        assert_eq!(linux_to_windows("Ubuntu", "~"), None);
        assert_eq!(linux_to_windows("", "/home/u"), None);
        assert_eq!(linux_to_unc("Ubuntu", "/"), r"\\wsl.localhost\Ubuntu\");
        // Hin und zurück
        for p in ["/home/u/a b", "/", "/srv/ä-ö"] {
            assert_eq!(unc_to_linux(&linux_to_unc("Deb", p)).map(|x| x.1).as_deref(), Some(p));
        }
        for p in [r"C:\Users\Max", r"Z:\a b\c"] {
            assert_eq!(linux_to_windows("D", &windows_to_linux(p).unwrap()).as_deref(), Some(p));
        }
    }

    #[test]
    fn linux_path_validation() {
        assert_eq!(validate_linux_path("/home/u/p"), Ok(()));
        assert_eq!(validate_linux_path("~"), Ok(()));
        assert_eq!(validate_linux_path("/a b/$(rm -rf)/`x`;'\""), Ok(()), "Sonderzeichen sind als Argument harmlos");
        assert_eq!(validate_linux_path(""), Err(PathError::Empty));
        assert_eq!(validate_linux_path("home/u"), Err(PathError::NotAbsolute));
        assert_eq!(validate_linux_path("~/x"), Err(PathError::NotAbsolute));
        assert_eq!(validate_linux_path(r"C:\x"), Err(PathError::NotAbsolute));
        assert_eq!(validate_linux_path("/a\nb"), Err(PathError::Control));
        assert_eq!(validate_linux_path("/a\0b"), Err(PathError::Control));
        assert_eq!(validate_linux_path(&format!("/{}", "a".repeat(MAX_PATH_LEN))), Err(PathError::TooLong));
        assert!(PathError::NotAbsolute.message(true).contains("Linux"));
        assert!(PathError::TooLong.message(false).contains("4096"));
    }

    #[test]
    fn home_expansion_and_normalizing() {
        assert_eq!(expand_home("~", Some("/home/u")).as_deref(), Some("/home/u"));
        assert_eq!(expand_home("~/p", Some("/home/u/")).as_deref(), Some("/home/u/p"));
        assert_eq!(expand_home("~/p", None), None);
        assert_eq!(expand_home("/x", None).as_deref(), Some("/x"));
        assert_eq!(expand_home("x", Some("/home/u")), None);
        assert_eq!(normalize("/home/u/p/", None), "/home/u/p");
        assert_eq!(normalize("/", None), "/");
        assert_eq!(normalize("~/p/", Some("/home/u")), "/home/u/p");
        assert_eq!(normalize("~", Some("/home/u")), "~");
        assert_eq!(normalize("~/p", None), "~/p");
    }

    #[test]
    fn completion_parsing() {
        assert_eq!(split_for_completion("/home/u/pr", None), Some(("/home/u/".into(), "pr".into())));
        assert_eq!(split_for_completion("/home/u/", None), Some(("/home/u/".into(), "".into())));
        assert_eq!(split_for_completion("/", None), Some(("/".into(), "".into())));
        assert_eq!(split_for_completion("~/pr", Some("/home/u")), Some(("/home/u/".into(), "pr".into())));
        assert_eq!(split_for_completion("~", Some("/home/u")), Some(("/home/u/".into(), "".into())));
        assert_eq!(split_for_completion("~", None), None);
        assert_eq!(split_for_completion("rel", None), None);
        let ls = b"project/\nproto/\nREADME.md\n.config/\nPrivate/\nlink\nweird\x1bname/\npr\xc3\xa4fix/\n";
        assert_eq!(parse_ls(ls, "pr", 10), vec!["project/".to_string(), "proto/".to_string(), "präfix/".to_string()]);
        assert_eq!(parse_ls(ls, "", 10), vec!["Private/".to_string(), "project/".to_string(), "proto/".to_string(), "präfix/".to_string()]);
        assert_eq!(parse_ls(ls, ".", 10), vec![".config/".to_string()]);
        assert_eq!(parse_ls(ls, "", 2).len(), 2);
        assert!(parse_ls(b"", "", 10).is_empty());
    }

    #[test]
    fn argv_keeps_user_text_as_single_arguments() {
        let evil = "/tmp/x; rm -rf ~ $(id) `id` 'q' \"d\" && echo";
        let a = list_args("Ubuntu", evil);
        assert_eq!(a, vec!["-d", "Ubuntu", "-e", "ls", "-1", "-p", "-A", "--", evil]);
        // `-e` = ohne Shell; der Pfad ist genau ein Argument und steht hinter `--`.
        assert_eq!(a.iter().filter(|x| x.as_str() == evil).count(), 1);
        assert_eq!(isdir_args("Ubuntu", evil), vec!["-d", "Ubuntu", "-e", "test", "-d", evil]);
        let h = home_args("Ubuntu-24.04");
        assert_eq!(&h[..5], &["-d", "Ubuntu-24.04", "-e", "sh", "-c"]);
        assert!(!h[5].contains("Ubuntu"), "nichts Veränderliches in der Shell-Zeichenkette");
        assert!(!valid_distro("-e"));
        assert!(!valid_distro("a\nb"));
        assert!(!valid_distro(r"..\x"));
        assert!(valid_distro("Ubuntu-24.04"));
    }

    #[test]
    fn cd_argument_for_sessions() {
        let mut a = vec!["-d".to_string(), "Ubuntu".to_string()];
        assert!(apply_cd(&mut a, "/home/u/my project"));
        assert_eq!(a, vec!["--cd", "/home/u/my project", "-d", "Ubuntu"]);
        let mut a = vec!["-d".to_string(), "Ubuntu".to_string()];
        assert!(apply_cd(&mut a, ""), "leer = Linux-Home");
        assert_eq!(a, vec!["--cd", "~", "-d", "Ubuntu"]);
        let mut a = vec![];
        assert!(apply_cd(&mut a, "~/x"));
        assert_eq!(a, vec!["--cd", "~"]);
        // Windows-Pfad: wie bisher über den Windows-Startordner
        let mut a = vec!["-d".to_string(), "Ubuntu".to_string()];
        assert!(!apply_cd(&mut a, r"C:\Users\u"));
        assert!(!apply_cd(&mut a, r"\\wsl$\Ubuntu\home"));
        assert_eq!(a.len(), 2);
        // Ungültig (Steuerzeichen) oder eigenes --cd des Profils: nichts anfügen
        assert!(!apply_cd(&mut a, "/a\nb"));
        let mut own = vec!["--cd".to_string(), "/srv".to_string()];
        assert!(!apply_cd(&mut own, "/home"));
        assert_eq!(own, vec!["--cd", "/srv"]);
    }

    #[test]
    fn recent_folders_are_capped_and_deduplicated() {
        let mut m = BTreeMap::new();
        for i in 0..15 {
            push_recent(&mut m, "Ubuntu", &format!("/p/{i}"));
        }
        push_recent(&mut m, "Ubuntu", "/p/5");
        let v = &m["Ubuntu"];
        assert_eq!(v.len(), RECENT_MAX);
        assert_eq!(v[0], "/p/5");
        assert_eq!(v.iter().filter(|p| *p == "/p/5").count(), 1);
        push_recent(&mut m, "Ubuntu", "relative");
        push_recent(&mut m, "-bad", "/x");
        assert_eq!(m.len(), 1);
        for i in 0..20 {
            push_recent(&mut m, &format!("D{i:02}"), "/x");
        }
        assert_eq!(m.len(), RECENT_DISTROS_MAX);
        assert_eq!(m["D19"], vec!["/x".to_string()]);
        let mut bad: BTreeMap<String, Vec<String>> = BTreeMap::new();
        bad.insert("Ubuntu".into(), vec!["/ok".into(), "nope".into(), "/a\u{7}".into()]);
        bad.insert("-x".into(), vec!["/ok".into()]);
        bad.insert("Empty".into(), vec!["rel".into()]);
        sanitize_recent(&mut bad);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad["Ubuntu"], vec!["/ok".to_string()]);
    }
}
