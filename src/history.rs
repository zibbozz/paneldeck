//! Verlauf früherer Claude-Konversationen (~/.claude/projects/*/*.jsonl) zum Fortsetzen.

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct HistoryItem {
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    pub modified: SystemTime,
    pub path: PathBuf,
}

pub fn projects_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".claude").join("projects"))
}

fn text_of(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(|b| (b["type"] == "text").then(|| b["text"].as_str().map(|s| s.to_string())).flatten()),
        _ => None,
    }
}

/// Liest nur den Anfang der Datei (Titel = erster echter Prompt, cwd).
pub fn read_item(path: &Path) -> Option<HistoryItem> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(256 * 1024).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let (mut title, mut cwd) = (String::new(), String::new());
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if cwd.is_empty() {
            if let Some(c) = v["cwd"].as_str() {
                cwd = c.to_string();
            }
        }
        if title.is_empty() && v["type"] == "user" && v["isMeta"] != true {
            if let Some(t) = text_of(&v["message"]["content"]) {
                let t = t.trim();
                if !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:") {
                    title = t.replace(['\n', '\r'], " ");
                }
            }
        }
        if !title.is_empty() && !cwd.is_empty() {
            break;
        }
    }
    if title.is_empty() {
        return None; // leere Sitzung
    }
    Some(HistoryItem { session_id, title: title.chars().take(140).collect(), cwd, modified, path: path.to_path_buf() })
}

pub fn scan(dir: &Path, limit: usize) -> Vec<HistoryItem> {
    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    for proj in rd.flatten() {
        let Ok(inner) = std::fs::read_dir(proj.path()) else { continue };
        for f in inner.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "jsonl") {
                if let Ok(m) = f.metadata().and_then(|m| m.modified()) {
                    files.push((m, p));
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(limit).filter_map(|(_, p)| read_item(&p)).collect()
}

/// Scannt im Hintergrund und liefert das Ergebnis über den Kanal.
pub fn scan_async(ctx: eframe::egui::Context) -> Receiver<Vec<HistoryItem>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let items = projects_dir().map(|d| scan(&d, 400)).unwrap_or_default();
        let _ = tx.send(items);
        ctx.request_repaint();
    });
    rx
}

/// Textausschnitt um die erste Fundstelle (`query` bereits klein geschrieben).
pub fn snippet(text: &str, query: &str, radius: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let lower: String = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let pos = lower.find(query)?;
    let start_c = lower[..pos].chars().count();
    let qlen = query.chars().count();
    let from = start_c.saturating_sub(radius);
    let to = (start_c + qlen + radius).min(chars.len());
    let mut out: String = chars[from..to].iter().map(|c| if c.is_control() { ' ' } else { *c }).collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!("{}{}{}", if from > 0 { "… " } else { "" }, out, if to < chars.len() { " …" } else { "" }))
}

fn all_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Volltextsuche in den Transkripten (Nutzer- und Claude-Texte) der neuesten `max_files` Konversationen.
pub fn search(dir: &Path, query: &str, max_files: usize) -> Vec<(HistoryItem, String)> {
    use std::io::{BufRead, BufReader};
    let q = query.trim().to_lowercase();
    if q.chars().count() < 2 {
        return vec![];
    }
    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    for proj in rd.flatten() {
        let Ok(inner) = std::fs::read_dir(proj.path()) else { continue };
        for f in inner.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "jsonl") {
                if let Ok(m) = f.metadata() {
                    if m.len() <= 100 * 1024 * 1024 {
                        if let Ok(t) = m.modified() {
                            files.push((t, p));
                        }
                    }
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut hits = Vec::new();
    for (_, path) in files.into_iter().take(max_files) {
        let Ok(f) = std::fs::File::open(&path) else { continue };
        let mut found: Option<String> = None;
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if !line.to_lowercase().contains(&q) {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] != "user" && v["type"] != "assistant" {
                continue;
            }
            if let Some(sn) = snippet(&all_text(&v["message"]["content"]), &q, 70) {
                found = Some(sn);
                break;
            }
        }
        if let Some(sn) = found {
            if let Some(item) = read_item(&path) {
                hits.push((item, sn));
                if hits.len() >= 100 {
                    break;
                }
            }
        }
    }
    hits
}

pub fn search_async(ctx: eframe::egui::Context, query: String) -> Receiver<Vec<(HistoryItem, String)>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = projects_dir().map(|d| search(&d, &query, 800)).unwrap_or_default();
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

pub fn age_label(t: SystemTime, de: bool) -> String {
    let secs = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    let (n, unit_de, unit_en) = match secs {
        0..=59 => (secs, "s", "s"),
        60..=3599 => (secs / 60, "min", "min"),
        3600..=86399 => (secs / 3600, "Std", "h"),
        _ => (secs / 86400, "Tg", "d"),
    };
    if de { format!("vor {n} {unit_de}") } else { format!("{n}{unit_en} ago") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_and_cwd_skip_meta_lines() {
        // Eigenes Verzeichnis je Test: die Tests laufen parallel und scannen sonst gegenseitig ihre Dateien.
        let dir = std::env::temp_dir().join(format!("hist-title-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        let p = dir.join("proj").join("abc-123.jsonl");
        std::fs::write(
            &p,
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"<command-name>/clear</command-name>\"}}\n",
                "{\"type\":\"attachment\",\"cwd\":\"C:\\\\Dev\\\\app\"}\n",
                "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Baue eine\\nLogin-Seite\"}]}}\n"
            ),
        )
        .unwrap();
        let items = scan(&dir, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].session_id, "abc-123");
        assert_eq!(items[0].title, "Baue eine Login-Seite");
        assert_eq!(items[0].cwd, "C:\\Dev\\app");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    #[test]
    fn snippet_and_search() {
        let sn = snippet("Hallo Welt, das ist ein ÄRGERLICHER Fehler im Code", "ärgerlicher", 5).unwrap();
        assert!(sn.contains("ÄRGERLICHER") && sn.starts_with("… "));
        assert!(snippet("abc", "x", 5).is_none());
        let dir = std::env::temp_dir().join(format!("hist-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("p")).unwrap();
        std::fs::write(
            dir.join("p/s1.jsonl"),
            "{\"type\":\"user\",\"cwd\":\"/x\",\"message\":{\"content\":\"Baue den Frobnicator\"}}\n{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Der Frobnicator ist fertig\"}]}}\n",
        )
        .unwrap();
        let r = search(&dir, "frobnicator", 10);
        assert_eq!(r.len(), 1);
        assert!(r[0].1.contains("Frobnicator"));
        assert!(search(&dir, "nichtda", 10).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
