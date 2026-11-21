//! Welche Dateien ein Agent im Editor öffnen darf (#54, Entscheidung im Issue): jeder **lokale** Pfad, nie Netzwerk-
//! bzw. UNC-Pfade (`\\server\freigabe`, auch `\\wsl.localhost\…` und verbundene Netzlaufwerke) und nie Geräte
//! (`\\.\…`, `\\?\…`, `CON`, `NUL`, `COM1` …; unter Linux/macOS `/dev`, `/proc`, `/sys`, FIFOs, Sockets).
//!
//! Zwei Stufen: [`check_syntax`] prüft nur den Text (rein, getestet für beide Plattformen über [`windows_rules`] und
//! [`unix_rules`]); [`check_file`] löst den Pfad danach auf der Platte auf (`canonicalize` – ein Netzlaufwerk oder ein
//! Link auf eine Freigabe wird dabei zu `\\?\UNC\…` und abgelehnt) und verlangt eine gewöhnliche, vorhandene Datei.

use crate::state::t;
use std::path::{Path, PathBuf};

/// Fehler wie im Hub: (Code, Meldung).
pub type Refusal = (String, String);

fn bad(msg: impl Into<String>) -> Refusal {
    ("bad_args".into(), msg.into())
}

fn forbidden(msg: impl Into<String>) -> Refusal {
    ("forbidden".into(), msg.into())
}

fn network() -> Refusal {
    forbidden(t(
        "Netzwerk- und UNC-Pfade (\\\\server\\freigabe, \\\\wsl.localhost\\…, Netzlaufwerke) darf ein Agent nicht öffnen – nur lokale Dateien.",
        "Agents cannot open network or UNC paths (\\\\server\\share, \\\\wsl.localhost\\…, mapped network drives) – only local files.",
    ))
}

fn device() -> Refusal {
    forbidden(t(
        "Geräte und Gerätepfade (\\\\.\\…, \\\\?\\…, CON, NUL, COM1 …, /dev, /proc, /sys) darf ein Agent nicht öffnen – nur gewöhnliche Dateien.",
        "Agents cannot open devices or device paths (\\\\.\\…, \\\\?\\…, CON, NUL, COM1 …, /dev, /proc, /sys) – only regular files.",
    ))
}

/// Reservierte Gerätenamen unter Windows (auch mit Endung, z. B. `NUL.txt`, und mit hochgestellten Ziffern).
fn reserved_device(component: &str) -> bool {
    // Windows ignoriert Punkte und Leerzeichen am Ende; der Name vor dem ersten Punkt zählt.
    let c = component.trim_end_matches(['.', ' ']);
    let stem = c.split('.').next().unwrap_or("").trim_end().to_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$") {
        return true;
    }
    let mut ch = stem.chars();
    let head: String = ch.by_ref().take(3).collect();
    let rest: String = ch.collect();
    (head == "COM" || head == "LPT") && matches!(rest.as_str(), "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
}

/// Regeln für Windows-Pfade (rein): absolut mit Laufwerksbuchstabe, kein UNC/Gerät, keine Datenströme (`datei:strom`),
/// keine unzulässigen Zeichen, keine Gerätenamen.
pub fn windows_rules(raw: &str) -> Result<PathBuf, Refusal> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(bad(t("path fehlt", "path is missing")));
    }
    let n = s.replace('/', "\\");
    if let Some(rest) = n.strip_prefix("\\\\") {
        // `\\?\` und `\\.\` sind Gerätenamensräume, alles andere ist eine Freigabe im Netz (auch `\\localhost\c$`).
        return Err(if rest.starts_with("?\\") || rest.starts_with(".\\") || rest == "?" || rest == "." { device() } else { network() });
    }
    let b = n.as_bytes();
    if !(b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\') {
        return Err(bad(t(
            "Bitte einen absoluten Pfad mit Laufwerksbuchstaben angeben, z. B. C:\\Projekt\\README.md.",
            "Please give an absolute path with a drive letter, e.g. C:\\project\\README.md.",
        )));
    }
    let rest = &n[3..];
    if rest.contains(':') {
        return Err(bad(t("„:“ ist im Pfad nicht erlaubt (Datenströme wie datei.txt:strom gehen nicht).", "“:” is not allowed in the path (alternate data streams such as file.txt:stream are not supported).")));
    }
    if rest.chars().any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || c.is_control()) {
        return Err(bad(t("Der Pfad enthält unzulässige Zeichen (< > \" | ? * oder Steuerzeichen).", "The path contains characters that are not allowed (< > \" | ? * or control characters).")));
    }
    if rest.split('\\').any(reserved_device) {
        return Err(device());
    }
    Ok(PathBuf::from(n))
}

/// Regeln für Linux/macOS (rein): absolut, nicht unter `/dev`, `/proc`, `/sys`, kein NUL-Zeichen.
pub fn unix_rules(raw: &str) -> Result<PathBuf, Refusal> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(bad(t("path fehlt", "path is missing")));
    }
    if !s.starts_with('/') {
        return Err(bad(t("Bitte einen absoluten Pfad angeben, z. B. /home/ich/projekt/README.md.", "Please give an absolute path, e.g. /home/me/project/README.md.")));
    }
    if s.contains('\0') {
        return Err(bad(t("Der Pfad enthält ein NUL-Zeichen.", "The path contains a NUL character.")));
    }
    let first = s.split('/').find(|c| !c.is_empty()).unwrap_or("");
    if matches!(first, "dev" | "proc" | "sys") {
        return Err(device());
    }
    Ok(PathBuf::from(s))
}

/// Textprüfung für die laufende Plattform.
pub fn check_syntax(raw: &str) -> Result<PathBuf, Refusal> {
    if cfg!(windows) { windows_rules(raw) } else { unix_rules(raw) }
}

/// Ergebnis von `canonicalize` prüfen (rein): unter Windows `\\?\C:\…` → `C:\…`, `\\?\UNC\…` ist eine Freigabe
/// (Netzlaufwerk, Link), alles andere ein Gerät/Volume ohne Buchstaben. Danach gelten die Textregeln erneut.
pub fn canonical_rules(canon: &str, windows: bool) -> Result<PathBuf, Refusal> {
    if !windows {
        return unix_rules(canon);
    }
    let s = canon.replace('/', "\\");
    let plain = match s.strip_prefix("\\\\?\\") {
        Some(r) if r.len() >= 4 && r.as_bytes()[..4].eq_ignore_ascii_case(b"UNC\\") => return Err(network()),
        // Volume ohne Laufwerksbuchstaben (`\\?\Volume{…}\`) oder ein anderer Gerätename.
        Some(r) if !(r.len() >= 3 && r.as_bytes()[0].is_ascii_alphabetic() && &r.as_bytes()[1..3] == b":\\") => return Err(device()),
        Some(r) => r.to_string(),
        None => s,
    };
    windows_rules(&plain)
}

/// Vollständige Prüfung für `editor_open`: Text, dann auf der Platte (vorhanden, gewöhnliche Datei, kein Ordner, nach
/// dem Auflösen von Links/Laufwerken weiterhin lokal). Liefert den aufgelösten Pfad (ohne `\\?\`).
pub fn check_file(raw: &str) -> Result<PathBuf, Refusal> {
    let p = check_syntax(raw)?;
    // Verbundenes Netzlaufwerk schon am Buchstaben erkennen (fragt das Netz nicht an; `canonicalize` könnte an einer
    // getrennten Freigabe hängen).
    if let Some(letter) = p.to_string_lossy().chars().next().filter(|_| crate::platform::fs::DRIVE_LETTERS)
        && crate::platform::fs::drive_type(letter) == Some(4)
    {
        return Err(network());
    }
    let canon = match std::fs::canonicalize(&p) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((
                "not_found".into(),
                format!("{} {} – {}", t("Datei nicht gefunden:", "File not found:"), p.display(), t("für neuen Text editor_open mit untitled = true nehmen.", "for new text use editor_open with untitled = true.")),
            ))
        }
        Err(e) => return Err(bad(format!("{}: {e}", p.display()))),
    };
    let checked = canonical_rules(&canon.to_string_lossy(), cfg!(windows))?;
    let meta = std::fs::metadata(&checked).map_err(|e| bad(format!("{}: {e}", checked.display())))?;
    if meta.is_dir() {
        return Err(bad(t("Das ist ein Ordner – editor_open öffnet nur Dateien.", "That is a folder – editor_open only opens files.")));
    }
    if !meta.is_file() {
        return Err(device());
    }
    Ok(checked)
}

/// Gleicher Pfad unter Windows ohne Rücksicht auf Groß-/Kleinschreibung und Trenner (für die Zuordnung zu offenen Tabs).
pub fn same_file(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let n = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_start_matches("\\\\?\\").to_lowercase();
        n(a) == n(b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(r: Result<PathBuf, Refusal>) -> String {
        match r {
            Ok(_) => "ok".into(),
            Err((c, _)) => c,
        }
    }

    #[test]
    fn windows_paths_must_be_local_absolute_files() {
        let w = |s: &str| code(windows_rules(s));
        assert_eq!(w(r"C:\work\README.md"), "ok");
        assert_eq!(w("c:/work/a b/x.rs"), "ok");
        assert_eq!(windows_rules("c:/work/x.rs").unwrap(), PathBuf::from(r"c:\work\x.rs"));
        // Netzwerk und UNC (auch WSL und localhost-Freigaben)
        for s in [r"\\server\share\x.txt", "//server/share/x.txt", r"\\wsl.localhost\Ubuntu\home\a.txt", r"\\localhost\c$\x.txt", r"\\127.0.0.1\c$\x"] {
            let (c, m) = windows_rules(s).unwrap_err();
            assert_eq!(c, "forbidden", "{s}");
            assert!(m.contains("UNC") || m.contains("network"), "{m}");
        }
        // Gerätenamensräume
        for s in [r"\\.\PhysicalDrive0", r"\\?\C:\x.txt", r"\\?\UNC\srv\s\x", r"\\.\pipe\x", r"\\?\Volume{1234}\x", r"\\.", r"\\?"] {
            assert_eq!(w(s), "forbidden", "{s}");
        }
        // reservierte Gerätenamen, auch mit Endung, Punkt/Leerzeichen am Ende und in Unterordnern
        for s in [r"C:\NUL", r"C:\x\con.txt", r"C:\aux.", r"C:\COM1", r"C:\a\lpt9.log", r"C:\CONIN$", r"C:\x\COM¹", r"C:\prn .txt"] {
            assert_eq!(w(s), "forbidden", "{s}");
        }
        for s in [r"C:\console.txt", r"C:\nul_file.txt", r"C:\COM10", r"C:\x\com.txt", r"C:\LPT", r"C:\auxiliary"] {
            assert_eq!(w(s), "ok", "{s}");
        }
        // relativ, ohne Laufwerk, Laufwerk-relativ, Datenströme, unzulässige Zeichen
        for s in ["README.md", r"work\x", r"\work\x", "C:x.txt", "C:", r"C:\x.txt:secret", r"C:\a|b", r"C:\a*.txt", "", "  "] {
            assert_eq!(w(s), "bad_args", "{s:?}");
        }
    }

    #[test]
    fn unix_paths_must_be_absolute_and_not_devices() {
        let u = |s: &str| code(unix_rules(s));
        assert_eq!(u("/home/me/x.txt"), "ok");
        assert_eq!(u("/devices/x"), "ok");
        for s in ["/dev/sda", "/proc/self/environ", "/sys/kernel", "//dev/null"] {
            assert_eq!(u(s), "forbidden", "{s}");
        }
        for s in ["x.txt", "~/x.txt", "./a", "/a\0b", ""] {
            assert_eq!(u(s), "bad_args", "{s:?}");
        }
    }

    #[test]
    fn canonical_forms_catch_mapped_drives_and_links() {
        assert_eq!(canonical_rules(r"\\?\C:\work\a.txt", true).unwrap(), PathBuf::from(r"C:\work\a.txt"));
        assert_eq!(code(canonical_rules(r"\\?\UNC\server\share\a.txt", true)), "forbidden");
        assert_eq!(code(canonical_rules(r"\\?\unc\server\share\a.txt", true)), "forbidden");
        assert_eq!(code(canonical_rules(r"\\?\Volume{0000}\a.txt", true)), "forbidden");
        assert_eq!(code(canonical_rules("\\\\?\\äöü\\x", true)), "forbidden", "kein Absturz an Mehrbyte-Zeichen");
        assert_eq!(code(windows_rules("äöü")), "bad_args");
        assert_eq!(code(canonical_rules("/proc/1/mem", false)), "forbidden");
        assert_eq!(canonical_rules("/tmp/a.txt", false).unwrap(), PathBuf::from("/tmp/a.txt"));
    }

    #[test]
    fn check_file_needs_an_existing_regular_file() {
        let dir = std::env::temp_dir().join(format!("pd-agent-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let got = check_file(&f.to_string_lossy()).unwrap();
        assert!(same_file(&got, &std::fs::canonicalize(&f).unwrap()));
        assert!(!got.to_string_lossy().starts_with(r"\\?\"), "ohne Präfix: {}", got.display());
        assert_eq!(code(check_file(&dir.to_string_lossy())), "bad_args", "Ordner");
        assert_eq!(code(check_file(&dir.join("fehlt.txt").to_string_lossy())), "not_found");
        #[cfg(unix)]
        assert_eq!(code(check_file("/dev/null")), "forbidden");
        #[cfg(windows)]
        assert_eq!(code(check_file(r"C:\NUL")), "forbidden");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_file_ignores_case_and_prefix_on_windows() {
        if cfg!(windows) {
            assert!(same_file(Path::new(r"C:\A\b.TXT"), Path::new(r"\\?\c:\a\B.txt")));
            assert!(same_file(Path::new("C:/a/b"), Path::new(r"C:\a\b")));
        }
        assert!(!same_file(Path::new("/a/b"), Path::new("/a/c")));
    }
}
