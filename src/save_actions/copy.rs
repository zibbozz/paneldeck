//! Aktion *Datei kopieren* (#383): an einen anderen **lokalen** Ort, Pfad relativ zum Regelordner beibehalten (oder nur
//! der Dateiname), fehlende Ordner werden angelegt, ein vorhandenes Ziel wird ohne Rückfrage atomar ersetzt.
//! Abgelehnt: Netzwerk-/UNC-Ziele und Netzlaufwerke (Paneldeck greift selbst nie aufs Netz zu), Geräte, ein Ziel gleich
//! der Quelle oder im Regelordner (sonst kopierte sich die Datei beim nächsten Speichern immer weiter).

use super::{is_inside, path_key, relative};
use crate::state::Lang;
use std::path::{Path, PathBuf};

/// Warum nicht kopiert wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyRefusal {
    NoTarget,
    NotAbsolute,
    Network,
    Device,
    /// Ziel liegt im Regelordner (oder ist er).
    InsideRuleFolder,
    SameFile,
}

impl CopyRefusal {
    pub fn text(&self, lang: Lang) -> &'static str {
        match self {
            CopyRefusal::NoTarget => lang.t("Kein Zielordner angegeben.", "No target folder given."),
            CopyRefusal::NotAbsolute => lang.t("Der Zielordner muss ein absoluter lokaler Pfad sein.", "The target folder must be an absolute local path."),
            CopyRefusal::Network => lang.t("Netzwerkziele (UNC-Pfade, Netzlaufwerke) gehen nicht – Paneldeck kopiert nur lokal. Für Uploads einen eigenen Befehl nehmen (z. B. scp).", "Network targets (UNC paths, network drives) are not supported – Paneldeck copies locally only. Use your own command for uploads (e.g. scp)."),
            CopyRefusal::Device => lang.t("Geräte sind als Ziel nicht erlaubt.", "Devices are not allowed as a target."),
            CopyRefusal::InsideRuleFolder => lang.t("Das Ziel liegt im Regelordner – die Kopie löste sonst beim nächsten Speichern wieder Aktionen aus.", "The target is inside the rule folder – the copy would trigger actions again on the next save."),
            CopyRefusal::SameFile => lang.t("Ziel und Quelle sind dieselbe Datei.", "Target and source are the same file."),
        }
    }
}

/// Prüft nur den Zielordner (für den Regel-Dialog), ohne Datei.
pub fn check_target(target: &str, rule_folder: &Path) -> Result<PathBuf, CopyRefusal> {
    let t = target.trim();
    if t.is_empty() {
        return Err(CopyRefusal::NoTarget);
    }
    if crate::pathfmt::is_remote(t) {
        return Err(CopyRefusal::Network);
    }
    match crate::editor_svc::agent_path::check_syntax(t) {
        Ok(_) => {}
        Err((code, msg)) if code == "forbidden" && msg.to_lowercase().contains("netw") => return Err(CopyRefusal::Network),
        Err((code, _)) if code == "forbidden" => return Err(CopyRefusal::Device),
        Err(_) => return Err(CopyRefusal::NotAbsolute),
    }
    let tp = normalize(Path::new(t));
    let real = resolve(&tp)?;
    let (rf, rf_real) = rule_folders(rule_folder);
    if is_inside(&tp, &rf) || is_inside(&real, &rf_real) {
        return Err(CopyRefusal::InsideRuleFolder);
    }
    Ok(real)
}

/// `.` und `..` auflösen (nur der Text, ohne die Platte).
pub fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            c => out.push(c),
        }
    }
    out.iter().collect()
}

/// Kanonischer Pfad ohne `\\?\`; `\\?\UNC\…` und andere Freigaben = Netzwerk. `None` = gibt es nicht.
fn canonical(p: &Path) -> Result<Option<PathBuf>, CopyRefusal> {
    let Ok(c) = std::fs::canonicalize(p) else { return Ok(None) };
    let s = c.to_string_lossy().to_string();
    if s.to_ascii_lowercase().starts_with(r"\\?\unc\") {
        return Err(CopyRefusal::Network);
    }
    let plain = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    if crate::pathfmt::is_remote(&plain) {
        return Err(CopyRefusal::Network);
    }
    Ok(Some(PathBuf::from(plain)))
}

/// Wirklicher Ort eines (vielleicht noch nicht vorhandenen) Pfads: der nächste vorhandene Vorfahr kanonisch (Links,
/// Junctions, Laufwerke), der Rest angehängt. Netzwerk → Fehler.
pub fn resolve(p: &Path) -> Result<PathBuf, CopyRefusal> {
    let p = normalize(p);
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = Some(p.as_path());
    while let Some(c) = cur {
        if let Some(real) = canonical(c)? {
            let mut out = real;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return Ok(out);
        }
        match c.file_name() {
            Some(n) => rest.push(n.to_os_string()),
            None => break,
        }
        cur = c.parent();
    }
    Ok(p)
}

/// Regelordner als Text (normalisiert) und – wenn lokal – sein wirklicher Ort (ein Netzwerkordner wird nicht angefasst).
fn rule_folders(rule_folder: &Path) -> (PathBuf, PathBuf) {
    let rf = normalize(rule_folder);
    let real = if crate::pathfmt::is_remote(&rf.to_string_lossy()) { rf.clone() } else { resolve(&rf).unwrap_or_else(|_| rf.clone()) };
    (rf, real)
}

/// Zielpfad für `src`: `target` + (Pfad relativ zum Regelordner bzw. Dateiname).
pub fn destination(src: &Path, rule_folder: &Path, target: &str, keep_structure: bool) -> Result<PathBuf, CopyRefusal> {
    let t = check_target(target, rule_folder)?;
    let rel = if keep_structure { relative(src, rule_folder).filter(|r| !r.as_os_str().is_empty()) } else { None };
    let dest = normalize(&match rel {
        Some(r) => t.join(r),
        None => t.join(src.file_name().unwrap_or_default()),
    });
    let real_src = resolve(src).unwrap_or_else(|_| normalize(src));
    if path_key(&dest) == path_key(src) || path_key(&dest) == path_key(&real_src) {
        return Err(CopyRefusal::SameFile);
    }
    // Das fertige Ziel darf nie im Regelordner liegen (z. B. Ziel = Ordner über dem Regelordner mit Ordnerstruktur).
    let (rf, rf_real) = rule_folders(rule_folder);
    if is_inside(&dest, &rf) || is_inside(&dest, &rf_real) {
        return Err(CopyRefusal::InsideRuleFolder);
    }
    Ok(dest)
}

/// Kopieren: fehlende Ordner anlegen, dann atomar schreiben (Temp-Datei im Zielordner + Ersetzen). Liefert die Bytes.
pub fn copy_file(src: &Path, dest: &Path) -> Result<u64, String> {
    // Nach dem Auflösen von Links/Laufwerken weiterhin lokal? (Ein vorhandener Ordner kann ein Link ins Netz sein.)
    let mut probe = dest.parent();
    while let Some(p) = probe {
        if p.exists() {
            if let Ok(c) = std::fs::canonicalize(p) {
                let s = c.to_string_lossy();
                if crate::pathfmt::is_remote(&s) || s.to_ascii_lowercase().starts_with(r"\\?\unc\") {
                    return Err("network target".into());
                }
            }
            break;
        }
        probe = p.parent();
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let w = crate::editor::file::atomic_write(dest, |out| std::io::copy(&mut f, out).map(|_| ())).map_err(|e| e.to_string())?;
    Ok(w.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_checked() {
        let tmp = crate::editor::test_dir("sa-copy");
        let rule = tmp.join("proj");
        let src = rule.join("src").join("a.txt");
        let out = tmp.join("out");
        assert_eq!(check_target("", &rule), Err(CopyRefusal::NoTarget));
        assert_eq!(check_target("relative\\dir", &rule), Err(CopyRefusal::NotAbsolute));
        if cfg!(windows) {
            assert_eq!(check_target(r"\\server\share\x", &rule), Err(CopyRefusal::Network));
            assert_eq!(check_target(r"\\?\UNC\server\share", &rule), Err(CopyRefusal::Network));
            assert!(matches!(check_target(r"C:\x\NUL", &rule), Err(CopyRefusal::Device)));
        }
        assert_eq!(check_target(&rule.join("dist").to_string_lossy(), &rule), Err(CopyRefusal::InsideRuleFolder));
        assert_eq!(check_target(&rule.to_string_lossy(), &rule), Err(CopyRefusal::InsideRuleFolder));
        // Das Ziel ist der wirkliche Ort (kanonisch: Links aufgelöst, unter Windows lange statt 8.3-Namen wie
        // `RUNNER~1`, unter macOS `/private/var` statt `/var`) – verglichen wird deshalb mit dem kanonischen Testordner.
        let canon = std::fs::canonicalize(&tmp).unwrap().to_string_lossy().to_string();
        let real_out = PathBuf::from(canon.strip_prefix(r"\\?\").unwrap_or(&canon)).join("out");
        assert_eq!(destination(&src, &rule, &out.to_string_lossy(), true).unwrap(), real_out.join("src").join("a.txt"));
        assert_eq!(destination(&src, &rule, &out.to_string_lossy(), false).unwrap(), real_out.join("a.txt"));
        // Datei-Regel: Regelordner = Ordner der Datei, das Ziel darf nicht die Quelle sein.
        let r2 = src.parent().unwrap();
        assert_eq!(destination(&src, r2, &src.parent().unwrap().to_string_lossy(), false), Err(CopyRefusal::InsideRuleFolder));
    }

    /// `..`, Ordner über dem Regelordner mit Ordnerstruktur und Links/Junctions in den Regelordner.
    #[test]
    fn final_destination_is_checked_after_resolving() {
        let tmp = crate::editor::test_dir("sa-copy3");
        let rule = tmp.join("app");
        let src = rule.join("app").join("x.txt");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "x").unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        // `..` führt zurück in den Regelordner.
        let back = format!("{}{sep}out{sep}..{sep}app{sep}dist", tmp.display());
        assert_eq!(check_target(&back, &rule), Err(CopyRefusal::InsideRuleFolder));
        assert_eq!(normalize(Path::new(&back)), rule.join("dist"));
        // Ziel = Ordner über dem Regelordner, Ordnerstruktur `app\x.txt` → `<tmp>\app\x.txt` liegt im Regelordner.
        assert_eq!(destination(&src, &rule, &tmp.to_string_lossy(), true), Err(CopyRefusal::InsideRuleFolder));
        // Ohne Ordnerstruktur liegt `<tmp>\x.txt` daneben: erlaubt.
        assert!(destination(&src, &rule, &tmp.to_string_lossy(), false).is_ok());
        // Ein Link auf den Regelordner zählt wie der Regelordner.
        let link = tmp.join("link");
        let made = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&rule).output().map(|o| o.status.success()).unwrap_or(false)
        } else {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&rule, &link).is_ok()
            }
            #[cfg(not(unix))]
            {
                false
            }
        };
        if made {
            assert_eq!(check_target(&link.join("sub").to_string_lossy(), &rule), Err(CopyRefusal::InsideRuleFolder));
            assert_eq!(check_target(&link.to_string_lossy(), &rule), Err(CopyRefusal::InsideRuleFolder));
        }
    }

    #[test]
    fn copy_creates_folders_and_replaces_atomically() {
        let tmp = crate::editor::test_dir("sa-copy2");
        let rule = tmp.join("proj");
        let src = rule.join("src").join("a.txt");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "one").unwrap();
        let dest = destination(&src, &rule, &tmp.join("out").to_string_lossy(), true).unwrap();
        assert_eq!(copy_file(&src, &dest).unwrap(), 3);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "one");
        std::fs::write(&src, "two!").unwrap();
        copy_file(&src, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "two!");
        let leftovers: Vec<_> = std::fs::read_dir(dest.parent().unwrap()).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".pdtmp")).collect();
        assert!(leftovers.is_empty(), "keine Temp-Dateien übrig");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
