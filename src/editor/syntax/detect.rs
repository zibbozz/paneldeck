//! Language detection (#312): the user's own mappings first, then the file name / extension, then shebang and
//! modeline, last the first-line patterns of the bundled grammars. Always overridable per document (status bar).

use super::engine;
use super::languages::{LANGUAGES, Language};
use std::path::Path;

/// The user's own mappings (`settings.json` → `editor.language_map`): pattern → language id. A pattern is a file
/// name (`Jenkinsfile`, compared without case) or an extension / suffix (`*.tpl`, `.tpl`, `*.blade.php`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UserMap {
    pub entries: Vec<(String, String)>,
}

impl UserMap {
    /// Language for a file name from the user's mappings (exact names first, then the longest matching suffix).
    pub fn lookup(&self, file_name: &str) -> Option<&str> {
        let name = file_name.to_lowercase();
        if let Some((_, l)) = self.entries.iter().find(|(p, _)| !is_suffix_pattern(p) && p.trim().to_lowercase() == name) {
            return Some(l.as_str());
        }
        self.entries
            .iter()
            .filter_map(|(p, l)| suffix_of(p).map(|s| (s, l)))
            .filter(|(s, _)| name.len() > s.len() && name.ends_with(s.as_str()))
            .max_by_key(|(s, _)| s.len())
            .map(|(_, l)| l.as_str())
    }
}

fn is_suffix_pattern(p: &str) -> bool {
    let p = p.trim();
    p.starts_with('*') || p.starts_with('.')
}

/// `*.tpl` / `.tpl` → `.tpl` (lowercase); other patterns → `None`.
fn suffix_of(p: &str) -> Option<String> {
    let p = p.trim();
    let rest = p.strip_prefix('*').unwrap_or(p);
    (rest.starts_with('.') && rest.len() > 1).then(|| rest.to_lowercase())
}

/// Detect the language of a document: `path` (may be `None` for untitled documents), `head` = the first lines of the
/// text, `tail` = the last lines (modelines can be at the end). Returns a language id or `None` (plain text).
pub fn detect(path: Option<&Path>, head: &str, tail: &str, user: &UserMap) -> Option<String> {
    let name = path.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !name.is_empty() {
        if let Some(l) = user.lookup(&name) {
            return Some(l.to_string());
        }
        if let Some(l) = by_name(&name) {
            return Some(l.id.to_string());
        }
        // Ganze Namen aus bats Liste (`go.mod`, `requirements.txt`, `Jenkinsfile`) – nur mit Punkt oder großem
        // Anfangsbuchstaben, sonst träfe eine Datei `build` oder `conf` eine Endung.
        if (name.contains('.') || name.starts_with(|c: char| c.is_ascii_uppercase()))
            && let Some(id) = engine::bat_by_extension(&name)
        {
            return Some(id);
        }
        if let Some((_, ext)) = name.rsplit_once('.')
            && !ext.is_empty()
        {
            let ext = ext.to_lowercase();
            if let Some(id) = sniff(&ext, head) {
                return Some(id.to_string());
            }
            if let Some(l) = LANGUAGES.iter().find(|l| l.exts.contains(&ext.as_str())) {
                return Some(l.id.to_string());
            }
            if let Some(id) = engine::bat_by_extension(&ext) {
                return Some(id);
            }
        }
    }
    let first = head.lines().next().unwrap_or("");
    if let Some(i) = shebang_interpreter(first)
        && let Some(l) = LANGUAGES.iter().find(|l| l.interpreters.contains(&i.as_str()))
    {
        return Some(l.id.to_string());
    }
    if let Some(m) = modeline(head).or_else(|| modeline(tail))
        && let Some(id) = by_mode_name(&m)
    {
        return Some(id.to_string());
    }
    engine::bat_by_first_line(first)
}

/// Built-in language by exact file name (or prefix entry `Dockerfile.`).
fn by_name(name: &str) -> Option<&'static Language> {
    let lower = name.to_lowercase();
    LANGUAGES.iter().find(|l| {
        l.names.iter().any(|n| {
            let n = n.to_lowercase();
            if n.ends_with('.') { lower.starts_with(&n) && lower.len() > n.len() } else { lower == n }
        })
    })
}

/// Extensions used by more than one language: decide by the content.
fn sniff(ext: &str, head: &str) -> Option<&'static str> {
    match ext {
        // VBA class modules vs. LaTeX classes.
        "cls" => {
            let t = head.trim_start();
            Some(if t.starts_with("VERSION ") || t.starts_with("Attribute VB_") || t.contains("\nAttribute VB_") { "vb" } else { "tex" })
        }
        _ => None,
    }
}

/// Interpreter of a shebang line: `#!/usr/bin/env -S python3 -u` → `python`, `#!/bin/bash` → `bash`.
pub fn shebang_interpreter(first_line: &str) -> Option<String> {
    let rest = first_line.strip_prefix("#!")?.trim();
    let mut parts = rest.split_whitespace();
    let mut prog = base_name(parts.next()?);
    if prog == "env" {
        prog = base_name(parts.find(|p| !p.starts_with('-'))?);
    }
    let p = prog.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').to_lowercase();
    (!p.is_empty()).then_some(p)
}

fn base_name(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

/// Mode name from a vim (`vim: set ft=python:`) or emacs (`-*- mode: python -*-`, `-*- python -*-`) modeline.
pub fn modeline(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(i) = line.find("-*-") {
            let rest = &line[i + 3..];
            if let Some(j) = rest.find("-*-") {
                let inner = rest[..j].trim();
                let mut found = None;
                for part in inner.split(';') {
                    match part.split_once(':') {
                        Some((k, v)) if k.trim().eq_ignore_ascii_case("mode") => found = Some(v.trim().to_lowercase()),
                        None if !part.trim().is_empty() && !inner.contains(':') => found = Some(part.trim().to_lowercase()),
                        _ => {}
                    }
                }
                if let Some(f) = found.filter(|f| !f.is_empty()) {
                    return Some(f);
                }
            }
        }
        let l = line.to_lowercase();
        for marker in ["vim:", "vi:", "ex:"] {
            let Some(i) = l.find(marker) else { continue };
            if i > 0 && !l[..i].ends_with(|c: char| c.is_whitespace()) {
                continue;
            }
            let rest = &l[i + marker.len()..];
            for key in ["filetype=", "ft=", "syntax="] {
                let mut from = 0;
                while let Some(k) = rest[from..].find(key) {
                    let at = from + k;
                    let ok = at == 0 || rest[..at].ends_with(|c: char| c.is_whitespace() || c == ':' || c == ',');
                    if ok {
                        let v: String = rest[at + key.len()..].chars().take_while(|c| c.is_alphanumeric() || matches!(c, '+' | '#' | '-' | '_')).collect();
                        if !v.is_empty() {
                            return Some(v);
                        }
                    }
                    from = at + key.len();
                }
            }
        }
    }
    None
}

/// Language id for a modeline name (modeline names, ids, then extensions of the table).
fn by_mode_name(m: &str) -> Option<&'static str> {
    let m = m.trim().to_lowercase();
    LANGUAGES
        .iter()
        .find(|l| l.modelines.contains(&m.as_str()))
        .or_else(|| LANGUAGES.iter().find(|l| l.id == m))
        .or_else(|| LANGUAGES.iter().find(|l| l.exts.contains(&m.as_str())))
        .map(|l| l.id)
}

/// Head (first lines) and tail (last lines) of a document for [`detect`] – a few lines each, no copy of the text.
pub fn head_tail(rope: &ropey::Rope) -> (String, String) {
    const LINES: usize = 5;
    const MAX: usize = 2000;
    let n = rope.len_lines();
    let take = |range: std::ops::Range<usize>| {
        let mut s = String::new();
        for i in range {
            let l = rope.line(i);
            s.extend(l.chars().take(MAX));
            if !s.ends_with('\n') {
                s.push('\n');
            }
        }
        s
    };
    let head = take(0..n.min(LINES));
    let tail = if n > LINES { take(n.saturating_sub(LINES).max(LINES)..n) } else { String::new() };
    (head, tail)
}
