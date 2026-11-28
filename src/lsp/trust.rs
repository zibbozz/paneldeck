//! Vertrauenswürdige Ordner für den Smart-Modus (#367): Ein Language Server liest und führt Projektdateien aus
//! (Build-Skripte, Makros, Plug-ins). Darum startet er nur für Dateien in einem Ordner, dem der Nutzer vertraut –
//! bis dahin bleibt der Syntax-Modus. Gespeichert in `trust.json` im Datenordner (internes Format, keine öffentliche
//! API); Agenten können es nie ändern (es gibt kein Werkzeug dafür). Ein Ordner gilt mit allen Unterordnern.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const FILE: &str = "trust.json";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trust {
    folders: Vec<PathBuf>,
}

/// Vergleichsform eines Pfads (Windows: ohne Groß/Klein, `\`, ohne Schrägstrich am Ende).
fn key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.trim_end_matches('\\');
    if cfg!(windows) { s.to_lowercase() } else { s.replace('\\', "/") }
}

impl Trust {
    /// Tolerant lesen: fehlt die Datei oder ist sie kaputt, vertraut Paneldeck keinem Ordner.
    pub fn load(data: &Path) -> Trust {
        let v: Value = std::fs::read_to_string(data.join(FILE)).ok().and_then(|s| serde_json::from_str(crate::state::strip_bom(&s)).ok()).unwrap_or(Value::Null);
        let folders = v.get("folders").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(PathBuf::from).filter(|p| p.is_absolute()).take(1000).collect()).unwrap_or_default();
        Trust { folders }
    }

    /// Atomar speichern (temporäre Datei, dann umbenennen).
    pub fn save(&self, data: &Path) -> std::io::Result<()> {
        let v = json!({"version": 1, "folders": self.folders.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>()});
        let tmp = data.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&v).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, data.join(FILE))
    }

    /// Ist der Ordner `path` vertrauenswürdig? Lokal zählen auch Unterordner eines vertrauten Ordners; UNC- und
    /// WSL-Pfade (`\\server\…`, `\\wsl.localhost\…`) nur als genau dieser Ordner (Entscheidung zu #367).
    pub fn trusted(&self, path: &Path) -> bool {
        let p = key(path);
        let sep = if cfg!(windows) { '\\' } else { '/' };
        let unc = p.starts_with("\\\\") || p.starts_with("//");
        self.folders.iter().any(|f| {
            let f = key(f);
            !f.is_empty() && (p == f || (!unc && p.starts_with(&format!("{f}{sep}"))))
        })
    }

    /// Ordner vertrauen (schon enthalten = nichts).
    pub fn add(&mut self, folder: &Path) -> bool {
        if !folder.is_absolute() || self.folders.iter().any(|f| key(f) == key(folder)) {
            return false;
        }
        self.folders.push(folder.to_path_buf());
        true
    }

    /// Vertrauen entziehen: `folder` selbst und alle Einträge darunter (liefert, ob sich etwas geändert hat).
    pub fn remove(&mut self, folder: &Path) -> bool {
        let k = key(folder);
        let n = self.folders.len();
        self.folders.retain(|f| {
            let fk = key(f);
            !(fk == k || fk.starts_with(&format!("{k}\\")) || fk.starts_with(&format!("{k}/")))
        });
        n != self.folders.len()
    }

    pub fn folders(&self) -> &[PathBuf] {
        &self.folders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_count_with_subfolders_survive_a_restart_and_can_be_revoked() {
        let dir = std::env::temp_dir().join(format!("pd-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = Trust::load(&dir);
        assert!(!t.trusted(Path::new(r"C:\work\a\x.rs")), "ohne Datei: nichts vertraut");
        let root = if cfg!(windows) { PathBuf::from(r"C:\Work\A") } else { PathBuf::from("/work/a") };
        assert!(t.add(&root));
        assert!(!t.add(&root), "doppelt");
        assert!(!t.add(Path::new("relative")), "nur absolute Pfade");
        let inner = root.join("src").join("x.rs");
        assert!(t.trusted(&inner));
        let sibling = if cfg!(windows) { PathBuf::from(r"C:\Work\AB\x.rs") } else { PathBuf::from("/work/ab/x.rs") };
        assert!(!t.trusted(&sibling), "Nachbarordner mit gleichem Anfang");
        if cfg!(windows) {
            assert!(t.trusted(Path::new(r"c:\work\a\src\x.rs")), "ohne Groß/Klein");
        }
        t.save(&dir).unwrap();
        let back = Trust::load(&dir);
        assert_eq!(back, t);
        let mut b = back;
        assert!(b.remove(&root));
        assert!(!b.trusted(&inner));
        // UNC/WSL: nur genau der Ordner
        let mut u = Trust::default();
        if cfg!(windows) {
            assert!(u.add(Path::new(r"\\wsl.localhost\Ubuntu\home\u\proj")));
            assert!(u.trusted(Path::new(r"\\wsl.localhost\Ubuntu\home\u\proj")));
            assert!(!u.trusted(Path::new(r"\\wsl.localhost\Ubuntu\home\u\proj\sub")), "kein Unterordner");
        }
        std::fs::write(dir.join(FILE), b"{kaputt").unwrap();
        assert!(Trust::load(&dir).folders().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
