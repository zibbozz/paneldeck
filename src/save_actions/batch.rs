//! cmd-Befehle der Aktionen beim Speichern über eine Batch-Datei (#383): Nur so ersetzt cmd `%PANELDECK_FILE%` und die
//! geheimen Variablen der Regel (`%TOKEN%`) beim Ausführen aus der Umgebung des Laufs. Über die Befehlszeile geht das
//! nicht: Die Zeile kommt als Inhalt einer Umgebungsvariable an `cmd /c` (`procs::launch_spec`), und den ersetzt cmd
//! kein zweites Mal; `call` für einen zweiten Durchgang würde auch Werte (Dateinamen mit `%` oder `&`) erneut
//! auswerten. Geheime Werte stehen nie in der Datei und nie in der Befehlszeile (die sehen andere Programme).
//!
//! Die Datei liegt in einem eigenen Ordner im Temp-Ordner des Nutzers, hat einen zufälligen Namen und wird gelöscht,
//! sobald der Lauf vorbei ist ([`TempBatch`] beim Verwerfen); Reste eines abgestürzten Laufs räumt das nächste Anlegen
//! nach einem Tag weg. Kodierung: reiner ASCII-Text wie er ist (passt in jede OEM-Codepage), sonst UTF-8 mit
//! `chcp 65001` davor (cmd liest Batch-Dateien in der Codepage der Konsole; die Konsole gehört nur diesem Lauf).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Ordner im Temp-Ordner des Nutzers.
const DIR: &str = "paneldeck-onsave";
/// Reste (abgestürzter Lauf) ab diesem Alter löschen.
const STALE: Duration = Duration::from_secs(24 * 3600);

/// Inhalt der Batch-Datei für die Befehlszeile `line` (eine Zeile, aus `vars::cmd_batch_line`).
pub fn content(line: &str) -> Vec<u8> {
    let mut s = String::from("@echo off\r\n");
    if !line.is_ascii() {
        s.push_str("@\"%SystemRoot%\\System32\\chcp.com\" 65001 >nul\r\n");
    }
    s.push_str(line);
    s.push_str("\r\n");
    s.into_bytes()
}

/// Angelegte Batch-Datei; wird beim Verwerfen gelöscht.
#[derive(Debug)]
pub struct TempBatch {
    path: PathBuf,
}

impl TempBatch {
    /// Datei mit `content` anlegen (im Ordner [`DIR`] unter `base`, sonst im Temp-Ordner des Nutzers).
    pub fn create(content: &[u8]) -> std::io::Result<TempBatch> {
        Self::create_in(&std::env::temp_dir().join(DIR), content)
    }

    fn create_in(dir: &Path, content: &[u8]) -> std::io::Result<TempBatch> {
        std::fs::create_dir_all(dir)?;
        sweep(dir);
        let mut r = [0u8; 16];
        getrandom::fill(&mut r).map_err(|e| std::io::Error::other(e.to_string()))?;
        let name: String = r.iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("pd-{name}.cmd"));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        let guard = TempBatch { path };
        f.write_all(content)?;
        f.sync_all()?;
        Ok(guard)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Wert für die Umgebungsvariable, über die cmd die Datei startet: der Pfad in Anführungszeichen (`&` oder `^` im
    /// Pfad bleiben so wörtlich; ein `"` kann in Windows-Pfaden nicht vorkommen).
    pub fn command(&self) -> String {
        format!("\"{}\"", self.path.display())
    }
}

impl Drop for TempBatch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Alte Reste löschen (nur eigene Dateien `pd-*.cmd`).
fn sweep(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !(name.starts_with("pd-") && name.ends_with(".cmd")) {
            continue;
        }
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age >= STALE);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_ascii_or_utf8_with_chcp() {
        assert_eq!(content("type \"%PANELDECK_FILE%\""), b"@echo off\r\ntype \"%PANELDECK_FILE%\"\r\n");
        let u = content("echo \"Grüße ✓\"");
        let s = String::from_utf8(u).unwrap();
        assert!(s.starts_with("@echo off\r\n@\"%SystemRoot%\\System32\\chcp.com\" 65001 >nul\r\necho \"Grüße ✓\"\r\n"), "{s}");
    }

    #[test]
    fn file_is_private_random_and_removed_on_drop() {
        let dir = std::env::temp_dir().join(format!("pd-batch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = TempBatch::create_in(&dir, b"@echo off\r\n").unwrap();
        let b = TempBatch::create_in(&dir, b"@echo off\r\n").unwrap();
        assert_ne!(a.path(), b.path(), "zufälliger Name");
        assert!(a.path().is_file() && a.command().starts_with('"') && a.command().ends_with(".cmd\""));
        let p = a.path().to_path_buf();
        drop(a);
        assert!(!p.exists(), "nach dem Lauf gelöscht");
        drop(b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Echter Lauf mit cmd: `%PANELDECK_FILE%` und eine geheime Variable werden zur Laufzeit ersetzt, `%` und `&` in
    /// Werten bleiben wörtlich (kein zweiter Durchgang), der Exit-Code kommt an.
    #[cfg(windows)]
    #[test]
    fn cmd_expands_runtime_variables_from_the_batch_file_once() {
        use crate::save_actions::vars::{cmd_batch_line, Ctx, Target};
        let dir = std::env::temp_dir().join(format!("pd-batch-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("a %OS% & b");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("x 100%.txt");
        std::fs::write(&file, "INHALT-OK").unwrap();
        let ctx = Ctx { file: file.clone(), rule_folder: sub.clone(), workspace: None };
        let secret = |n: &str| n.eq_ignore_ascii_case("TOKEN");
        let defined = |n: &str| ["OS", "PANELDECK_FILE", "TOKEN"].iter().any(|x| x.eq_ignore_ascii_case(n));
        let tpl = "type \"%PANELDECK_FILE%\" && echo \"[%TOKEN%]\" [%OS%] [%NOPE%] 50% && type ${file} && exit /b 7";
        let line = cmd_batch_line(tpl, &ctx, &Target::test(crate::procs::Flavor::Cmd), &defined, &|_| None, &secret).unwrap();
        assert!(!line.contains("geheim"), "geheimer Wert nie in der Datei");
        let b = TempBatch::create_in(&dir.join("bat"), &content(&line)).unwrap();
        let out = std::process::Command::new("cmd.exe")
            .args(["/d", "/v:off", "/c", "%PANELDECK_PROCESS_COMMAND%"])
            .env("PANELDECK_PROCESS_COMMAND", b.command())
            .env("PANELDECK_FILE", &file)
            .env("TOKEN", "geheim&wert%OS%")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert_eq!(out.status.code(), Some(7), "{text}");
        assert_eq!(text.matches("INHALT-OK").count(), 2, "%PANELDECK_FILE% und ${{file}}: {text}");
        assert!(text.contains("[geheim&wert%OS%]"), "Wert einmal ersetzt, & und % darin wörtlich: {text}");
        assert!(text.contains("[Windows_NT] [%NOPE%] 50%"), "wie auf der Befehlszeile: {text}");
        drop(b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
