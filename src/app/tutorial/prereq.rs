//! Voraussetzungen einzelner Schritte (#143): Claude Code und Ollama werden nur im PATH gesucht – nie gestartet.

use super::steps::Need;
use std::ffi::OsStr;
use std::path::Path;

/// Namen der ausführbaren Dateien je Voraussetzung (Windows mit Endungen, sonst ohne).
fn names(need: Need) -> &'static [&'static str] {
    match (need, cfg!(windows)) {
        (Need::Claude, true) => &["claude.exe", "claude.cmd", "claude.bat"],
        (Need::Claude, false) => &["claude"],
        (Need::Ollama, true) => &["ollama.exe"],
        (Need::Ollama, false) => &["ollama"],
    }
}

/// Liegt eine der Dateien `names` in einem Ordner von `path` (Inhalt einer PATH-Variablen)?
pub(super) fn in_path(path: &OsStr, names: &[&str]) -> bool {
    std::env::split_paths(path).any(|d| names.iter().any(|n| is_file(&d.join(n))))
}

fn is_file(p: &Path) -> bool {
    p.is_file()
}

/// Fehlt die Voraussetzung? `servers` = Zahl der eingerichteten LLM-Server (dann gilt Ollama als vorhanden).
pub(super) fn missing(need: Need, servers: usize) -> bool {
    if need == Need::Ollama && servers > 0 {
        return false;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    !in_path(&path, names(need))
}
