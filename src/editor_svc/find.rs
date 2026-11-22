//! Suche in Dateien (#54): projektweit im geöffneten Ordner des Editors, ohne egui.
//!
//! * Durchlaufen mit der Crate `ignore` (MIT/Unlicense, dieselbe wie in ripgrep): `.gitignore` (auch verschachtelte und
//!   die der Elternordner), `.git/info/exclude`, die globale Git-Ausschlussdatei und `.ignore` gelten – auch außerhalb
//!   eines Git-Repositorys. Mit *Ignorierte einbeziehen* fallen diese Regeln weg. Versteckte Dateien werden durchsucht,
//!   die Ordner `.git`, `.hg` und `.svn` nie. Eigene Ausschlussmuster (gitignore-Syntax) gelten immer.
//! * Binärdateien (NUL-Byte in den ersten 8 KB) und Dateien über der Größengrenze (Schwelle für große Dateien) werden
//!   übersprungen und gezählt. Text: UTF-8 (BOM erlaubt), UTF-16 mit BOM; sonst UTF-8 mit Ersatzzeichen.
//! * Suche zeilenweise: Text oder regulärer Ausdruck (Crate `regex`), Groß-/Kleinschreibung, ganzes Wort.
//! * Ergebnisse kommen gestreamt aus einem Hintergrund-Thread ([`start`] → [`Job`]), abbrechbar und gedeckelt
//!   ([`MAX_MATCHES`], [`MAX_FILES`], [`MAX_PER_FILE`]). [`Results`] ist das Modell für das Ergebnis-Panel.

use regex::{Regex, RegexBuilder};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// Höchstzahl Treffer insgesamt; danach endet die Suche mit `truncated`.
pub const MAX_MATCHES: u64 = 20_000;
/// Höchstzahl Dateien mit Treffern.
pub const MAX_FILES: u64 = 5_000;
/// Höchstzahl Trefferzeilen je Datei (weitere zählen, werden aber nicht übertragen: `FileHits::more`).
pub const MAX_PER_FILE: usize = 1_000;
/// Länge der Vorschau einer Trefferzeile (Zeichen).
pub const PREVIEW_CHARS: usize = 240;
/// Längstes Suchmuster.
pub const MAX_PATTERN: usize = 1_000;
/// So viele Bytes werden auf NUL geprüft.
const BINARY_PROBE: usize = 8 * 1024;
/// Ordner, die nie durchsucht werden.
pub const ALWAYS_SKIPPED: [&str; 3] = [".git", ".hg", ".svn"];

/// Was gesucht wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// `.gitignore` & Co. nicht beachten.
    pub include_ignored: bool,
    /// Eigene Ausschlussmuster (gitignore-Syntax).
    pub excludes: Vec<String>,
    /// Größere Dateien überspringen (Bytes).
    pub max_file_bytes: u64,
}

impl Default for Query {
    fn default() -> Self {
        Query { pattern: String::new(), regex: false, case_sensitive: false, whole_word: false, include_ignored: false, excludes: Vec::new(), max_file_bytes: 20 * 1024 * 1024 }
    }
}

/// Eine Zeile mit Treffern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineHit {
    /// Zeilennummer (ab 1).
    pub line: usize,
    /// Spalte des ersten Treffers in Zeichen (ab 1), bezogen auf die ganze Zeile.
    pub col: usize,
    /// Vorschau der Zeile (gekürzt um den ersten Treffer herum, ohne Zeilenende).
    pub preview: String,
    /// Byte-Bereiche der Treffer in `preview` (nur die sichtbaren).
    pub ranges: Vec<(usize, usize)>,
    /// Zahl der Treffer in dieser Zeile (auch nicht sichtbare).
    pub count: usize,
}

/// Treffer einer Datei.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHits {
    pub path: PathBuf,
    /// Pfad relativ zum Suchordner, mit `/`.
    pub rel: String,
    pub lines: Vec<LineHit>,
    /// Treffer insgesamt in dieser Datei.
    pub matches: u64,
    /// Es gab mehr Trefferzeilen als übertragen ([`MAX_PER_FILE`]).
    pub more: bool,
}

/// Abschluss einer Suche.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub files_scanned: u64,
    pub files_with_matches: u64,
    pub matches: u64,
    pub skipped_binary: u64,
    pub skipped_large: u64,
    pub errors: u64,
    /// Eine Obergrenze wurde erreicht.
    pub truncated: bool,
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

/// Meldungen des Such-Threads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Hits(FileHits),
    /// Fortschritt: so viele Dateien sind durchsucht.
    Progress(u64),
    Done(Summary),
}

/// Ungültige Eingabe (Muster leer/zu lang, regulärer Ausdruck fehlerhaft, Ausschlussmuster fehlerhaft, Ordner fehlt).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError(pub String);

/// Muster in einen regulären Ausdruck übersetzen.
pub fn matcher(q: &Query) -> Result<Regex, QueryError> {
    let lang = crate::state::lang();
    if q.pattern.is_empty() {
        return Err(QueryError(lang.t("Suchtext fehlt.", "The search text is missing.").into()));
    }
    if q.pattern.len() > MAX_PATTERN {
        return Err(QueryError(lang.t("Der Suchtext ist zu lang (höchstens 1000 Zeichen).", "The search text is too long (at most 1000 characters).").into()));
    }
    let body = if q.regex { q.pattern.clone() } else { regex::escape(&q.pattern) };
    let body = if q.whole_word { format!(r"\b(?:{body})\b") } else { body };
    RegexBuilder::new(&body)
        .case_insensitive(!q.case_sensitive)
        .size_limit(8 * 1024 * 1024)
        .build()
        .map_err(|e| QueryError(format!("{} {e}", lang.t("Ungültiger regulärer Ausdruck:", "Invalid regular expression:"))))
}

/// Binärdatei? (NUL-Byte am Anfang; UTF-16 mit BOM zählt nicht als binär.)
pub fn is_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    bytes[..bytes.len().min(BINARY_PROBE)].contains(&0)
}

/// Bytes als Text: BOM für UTF-8/UTF-16 beachten, sonst UTF-8 mit Ersatzzeichen.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) }).collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(b) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(b).into_owned()
    } else if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(b, true)
    } else if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(b, false)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Vorschau einer Zeile um den ersten Treffer herum (höchstens [`PREVIEW_CHARS`] Zeichen) mit den sichtbaren Bereichen.
fn preview(line: &str, ranges: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
    let chars = line.chars().count();
    if chars <= PREVIEW_CHARS {
        return (line.to_string(), ranges.to_vec());
    }
    // Fenster in Bytes: etwas Kontext vor dem ersten Treffer.
    let first = ranges.first().map(|r| r.0).unwrap_or(0);
    let before = line[..first].chars().count();
    let start_char = before.saturating_sub(40);
    let start = line.char_indices().nth(start_char).map(|(i, _)| i).unwrap_or(0);
    let end = line[start..].char_indices().nth(PREVIEW_CHARS).map(|(i, _)| start + i).unwrap_or(line.len());
    let lead = if start > 0 { "…" } else { "" };
    let tail = if end < line.len() { "…" } else { "" };
    let text = format!("{lead}{}{tail}", &line[start..end]);
    let shift = lead.len();
    let vis = ranges
        .iter()
        .filter(|(a, _)| *a >= start && *a < end)
        .map(|(a, b)| (a - start + shift, (*b).min(end) - start + shift))
        .collect();
    (text, vis)
}

/// Treffer in einem Text.
#[derive(Debug, Default)]
pub struct TextHits {
    /// Trefferzeilen (höchstens so viele wie verlangt).
    pub lines: Vec<LineHit>,
    /// Treffer insgesamt.
    pub total: u64,
    /// Zeilen mit Treffern insgesamt (auch die nicht übertragenen).
    pub matched_lines: u64,
    /// Abgebrochen (`cancel`).
    pub stopped: bool,
}

/// Treffer in einem Text (rein), höchstens `max_lines` Zeilen.
pub fn search_text(text: &str, re: &Regex, max_lines: usize, cancel: &AtomicBool) -> TextHits {
    let mut h = TextHits::default();
    for (i, raw) in text.split('\n').enumerate() {
        if i % 4096 == 4095 && cancel.load(Ordering::Relaxed) {
            h.stopped = true;
            return h;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        // Leere Treffer (z. B. `^`) zählen nur in leeren Zeilen (`^$`).
        let ranges: Vec<(usize, usize)> = re.find_iter(line).filter(|m| m.end() > m.start() || line.is_empty()).map(|m| (m.start(), m.end())).collect();
        if ranges.is_empty() {
            continue;
        }
        h.total += ranges.len() as u64;
        h.matched_lines += 1;
        if h.lines.len() < max_lines {
            let col = line[..ranges[0].0].chars().count() + 1;
            let (preview, vis) = preview(line, &ranges);
            h.lines.push(LineHit { line: i + 1, col, preview, ranges: vis, count: ranges.len() });
        }
    }
    h
}

/// Durchlauf des Ordners mit den Regeln aus der Moduldoku.
fn walker(root: &Path, q: &Query) -> Result<ignore::Walk, QueryError> {
    let lang = crate::state::lang();
    let mut ov = ignore::overrides::OverrideBuilder::new(root);
    for pat in &q.excludes {
        let p = pat.trim();
        if p.is_empty() {
            continue;
        }
        // In Overrides heißt „!muster“ ausschließen.
        let neg = if let Some(rest) = p.strip_prefix('!') { rest.to_string() } else { format!("!{p}") };
        ov.add(&neg).map_err(|e| QueryError(format!("{} {p}: {e}", lang.t("Ungültiges Ausschlussmuster", "Invalid exclude pattern"))))?;
    }
    let ov = ov.build().map_err(|e| QueryError(e.to_string()))?;
    let honor = !q.include_ignored;
    let mut b = ignore::WalkBuilder::new(root);
    b.hidden(false)
        .git_ignore(honor)
        .git_exclude(honor)
        .ignore(honor)
        .parents(honor)
        // Tests bleiben unabhängig von der globalen Git-Ausschlussdatei des Rechners.
        .git_global(honor && !cfg!(test))
        .require_git(false)
        .follow_links(false)
        .overrides(ov)
        .filter_entry(|e| !ALWAYS_SKIPPED.iter().any(|s| e.file_name() == *s))
        .sort_by_file_name(|a, b| a.cmp(b));
    Ok(b.build())
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Die eigentliche Suche (blockierend; läuft im Thread von [`start`], in Tests direkt).
pub fn run(root: &Path, q: &Query, re: &Regex, cancel: &AtomicBool, mut emit: impl FnMut(Event)) -> Summary {
    let t0 = Instant::now();
    let mut s = Summary::default();
    let walk = match walker(root, q) {
        Ok(w) => w,
        Err(_) => {
            s.errors += 1;
            return s;
        }
    };
    let mut last_progress = Instant::now();
    for entry in walk {
        if cancel.load(Ordering::Relaxed) {
            s.cancelled = true;
            break;
        }
        let Ok(entry) = entry else {
            s.errors += 1;
            continue;
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if len > q.max_file_bytes {
            s.skipped_large += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            s.errors += 1;
            continue;
        };
        s.files_scanned += 1;
        if is_binary(&bytes) {
            s.skipped_binary += 1;
            continue;
        }
        let text = decode(&bytes);
        let room = (MAX_MATCHES - s.matches).min(MAX_PER_FILE as u64) as usize;
        let h = search_text(&text, re, room.max(1), cancel);
        if h.stopped {
            s.cancelled = true;
            break;
        }
        if h.total > 0 {
            s.files_with_matches += 1;
            s.matches += h.total;
            let more = (h.lines.len() as u64) < h.matched_lines;
            emit(Event::Hits(FileHits { path: path.to_path_buf(), rel: rel_of(root, path), lines: h.lines, matches: h.total, more }));
            if s.matches >= MAX_MATCHES || s.files_with_matches >= MAX_FILES {
                s.truncated = true;
                break;
            }
        }
        if last_progress.elapsed().as_millis() >= 100 {
            emit(Event::Progress(s.files_scanned));
            last_progress = Instant::now();
        }
    }
    s.elapsed_ms = t0.elapsed().as_millis() as u64;
    s
}

/// Laufende Suche.
pub struct Job {
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// Abbrechen (der Thread endet bei der nächsten Datei bzw. nach höchstens 4096 Zeilen).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Neue Meldungen (höchstens `max` auf einmal, damit ein Bild nie lange hängt).
    pub fn drain(&self, max: usize) -> Vec<Event> {
        self.rx.try_iter().take(max).collect()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Suche im Hintergrund starten. Fehler im Muster oder in den Ausschlussmustern kommen sofort zurück. `wake` wird nach
/// neuen Meldungen gerufen (z. B. `ctx.request_repaint()`).
pub fn start(root: PathBuf, q: Query, wake: impl Fn() + Send + 'static) -> Result<Job, QueryError> {
    let lang = crate::state::lang();
    if !root.is_dir() {
        return Err(QueryError(lang.t("Der Suchordner existiert nicht.", "The search folder does not exist.").into()));
    }
    let re = matcher(&q)?;
    walker(&root, &q)?; // Ausschlussmuster jetzt prüfen
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx): (Sender<Event>, Receiver<Event>) = std::sync::mpsc::channel();
    let c = cancel.clone();
    std::thread::Builder::new()
        .name("editor-find".into())
        .spawn(move || {
            let tx2 = tx.clone();
            let mut n = 0usize;
            let summary = run(&root, &q, &re, &c, |e| {
                let _ = tx2.send(e);
                n += 1;
                if n % 16 == 1 {
                    wake();
                }
            });
            let _ = tx.send(Event::Done(summary));
            wake();
        })
        .map_err(|e| QueryError(e.to_string()))?;
    Ok(Job { rx, cancel })
}

/// Modell des Ergebnis-Panels: gesammelte Treffer, Fortschritt, Abschluss, aufgeklappte Dateien, Auswahl.
#[derive(Default)]
pub struct Results {
    pub files: Vec<FileHits>,
    /// Durchsuchte Dateien bis jetzt.
    pub progress: u64,
    pub summary: Option<Summary>,
    /// Fehler beim Start (ungültiges Muster …).
    pub error: Option<String>,
    /// Zugeklappte Dateien (Index in `files`).
    pub collapsed: std::collections::BTreeSet<usize>,
    /// Ausgewählte Zeile: (Datei, Zeile in `lines`).
    pub selected: Option<(usize, usize)>,
    job: Option<Job>,
}

impl Results {
    /// Neue Suche starten (eine laufende wird abgebrochen).
    pub fn start(&mut self, root: PathBuf, q: Query, wake: impl Fn() + Send + 'static) {
        self.cancel();
        *self = Results::default();
        match start(root, q, wake) {
            Ok(j) => self.job = Some(j),
            Err(e) => self.error = Some(e.0),
        }
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    pub fn cancel(&mut self) {
        if let Some(j) = self.job.take() {
            j.cancel();
            // Der Thread schickt noch `Done` – ohne Empfänger egal. Abschluss hier vermerken.
            let mut s = self.summary.clone().unwrap_or_default();
            s.cancelled = true;
            s.files_scanned = s.files_scanned.max(self.progress);
            s.files_with_matches = self.files.len() as u64;
            s.matches = self.total_matches();
            self.summary = Some(s);
        }
    }

    /// Je Bild aufrufen; `true`, wenn sich etwas geändert hat.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &self.job else { return false };
        let events = job.drain(256);
        let changed = !events.is_empty();
        for e in events {
            match e {
                Event::Hits(h) => self.files.push(h),
                Event::Progress(n) => self.progress = n,
                Event::Done(s) => {
                    self.progress = s.files_scanned;
                    self.summary = Some(s);
                    self.job = None;
                }
            }
        }
        changed
    }

    pub fn total_matches(&self) -> u64 {
        self.files.iter().map(|f| f.matches).sum()
    }

    /// Ziel der ausgewählten Zeile: (Datei, Zeile ab 1, Spalte ab 1) – zum Öffnen im Editor.
    pub fn target(&self, file: usize, line: usize) -> Option<(PathBuf, usize, usize)> {
        let f = self.files.get(file)?;
        let l = f.lines.get(line)?;
        Some((f.path.clone(), l.line, l.col))
    }

    /// Nächste/vorige Trefferzeile ab der Auswahl (F4/Umschalt+F4), über Dateigrenzen hinweg.
    pub fn step(&mut self, forward: bool) -> Option<(PathBuf, usize, usize)> {
        let flat: Vec<(usize, usize)> = self.files.iter().enumerate().flat_map(|(fi, f)| (0..f.lines.len()).map(move |li| (fi, li))).collect();
        if flat.is_empty() {
            return None;
        }
        let pos = self.selected.and_then(|s| flat.iter().position(|x| *x == s));
        let next = match (pos, forward) {
            (None, true) => 0,
            (None, false) => flat.len() - 1,
            (Some(p), true) => (p + 1) % flat.len(),
            (Some(p), false) => (p + flat.len() - 1) % flat.len(),
        };
        self.selected = Some(flat[next]);
        self.collapsed.remove(&flat[next].0);
        self.target(flat[next].0, flat[next].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(p: &str) -> Query {
        Query { pattern: p.into(), ..Default::default() }
    }

    fn no() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn matcher_text_regex_case_and_word() {
        let re = matcher(&q("a.b")).unwrap();
        assert!(re.is_match("xA.Bx") && !re.is_match("axb"), "Text, ohne Groß-/Kleinschreibung");
        let re = matcher(&Query { regex: true, ..q("a.b") }).unwrap();
        assert!(re.is_match("axb"));
        let re = matcher(&Query { case_sensitive: true, ..q("Foo") }).unwrap();
        assert!(re.is_match("Foo") && !re.is_match("foo"));
        let re = matcher(&Query { whole_word: true, ..q("foo") }).unwrap();
        assert!(re.is_match("a foo b") && !re.is_match("foobar"));
        let re = matcher(&Query { whole_word: true, regex: true, ..q("fo+|bar") }).unwrap();
        assert!(re.is_match("x bar") && !re.is_match("xbar"), "Alternative im Wort-Rahmen geklammert");
        assert!(matcher(&Query { regex: true, ..q("(") }).unwrap_err().0.contains("regular expression") || crate::state::lang() == crate::state::Lang::De);
        assert!(matcher(&q("")).is_err());
        assert!(matcher(&q(&"x".repeat(MAX_PATTERN + 1))).is_err());
    }

    #[test]
    fn lines_columns_previews_and_caps() {
        let re = matcher(&q("needle")).unwrap();
        let text = "a\r\nfind the needle and another needle\nnone\nnéédle needle";
        let h = search_text(text, &re, 100, &no());
        let lines = h.lines;
        assert!(!h.stopped);
        assert_eq!((h.total, h.matched_lines), (3, 2));
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].line, lines[0].col, lines[0].count), (2, 10, 2));
        assert_eq!(&lines[0].preview[lines[0].ranges[0].0..lines[0].ranges[0].1], "needle");
        assert_eq!((lines[1].line, lines[1].col), (4, 8), "Spalte in Zeichen, nicht Bytes");
        // Vorschau langer Zeilen um den Treffer herum
        let long = format!("{}needle{}", "x".repeat(1000), "y".repeat(1000));
        let lines = search_text(&long, &re, 10, &no()).lines;
        let l = &lines[0];
        assert!(l.preview.starts_with('…') && l.preview.ends_with('…') && l.preview.chars().count() <= PREVIEW_CHARS + 2);
        assert_eq!(&l.preview[l.ranges[0].0..l.ranges[0].1], "needle");
        assert_eq!(l.col, 1001);
        // Obergrenze der Zeilen
        let many = "needle\n".repeat(50);
        let h = search_text(&many, &re, 10, &no());
        assert_eq!((h.lines.len(), h.total, h.matched_lines), (10, 50, 50));
        // Abbruch
        let big = "x\n".repeat(10_000);
        let stop = AtomicBool::new(true);
        assert!(search_text(&big, &re, 10, &stop).stopped);
    }

    #[test]
    fn binary_and_encodings() {
        assert!(is_binary(b"abc\0def"));
        assert!(!is_binary(b"plain text"));
        assert!(!is_binary(&[0xFF, 0xFE, b'a', 0]), "UTF-16 mit BOM ist Text");
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'h', b'i']), "hi");
        assert_eq!(decode(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode(&[0xFE, 0xFF, 0, b'h', 0, b'i']), "hi");
        assert_eq!(decode(b"caf\xe9"), "caf\u{FFFD}");
    }

    fn tree() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pd-find-{}-{}", std::process::id(), std::thread::current().name().unwrap_or("t").replace("::", "_")));
        let _ = std::fs::remove_dir_all(&d);
        for (p, c) in [
            ("src/main.rs", "fn main() { needle(); }\n"),
            ("src/lib.rs", "// no match here\n"),
            ("target/debug/out.txt", "needle in build output\n"),
            ("notes/.hidden.md", "hidden needle\n"),
            ("vendor/x.min.js", "needle()\n"),
            (".git/config", "needle in git\n"),
            ("bin.dat", "needle\0binary"),
            ("big.log", &"needle\n".repeat(400)),
            (".gitignore", "target/\n"),
            ("sub/.gitignore", "*.tmp\n"),
            ("sub/a.tmp", "needle tmp\n"),
            ("sub/b.txt", "needle b\n"),
        ] {
            let f = d.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        }
        d
    }

    fn rels(root: &Path, query: &Query) -> (Vec<String>, Summary) {
        let re = matcher(query).unwrap();
        let mut files = Vec::new();
        let s = run(root, query, &re, &no(), |e| {
            if let Event::Hits(h) = e {
                files.push(h.rel);
            }
        });
        (files, s)
    }

    #[test]
    fn walking_respects_gitignore_excludes_binaries_and_size() {
        let root = tree();
        let base = Query { max_file_bytes: 1000, ..q("needle") };
        let (files, s) = rels(&root, &base);
        assert_eq!(files, ["notes/.hidden.md", "src/main.rs", "sub/b.txt", "vendor/x.min.js"], "sortiert; .gitignore (auch verschachtelt), .git und Binärdateien fehlen");
        assert_eq!((s.skipped_binary, s.skipped_large, s.files_with_matches, s.matches), (1, 1, 4, 4));
        assert!(!s.truncated && !s.cancelled);
        // Ignorierte einbeziehen: target/ und *.tmp kommen dazu, .git nie.
        let (files, _) = rels(&root, &Query { include_ignored: true, ..base.clone() });
        assert!(files.contains(&"target/debug/out.txt".to_string()) && files.contains(&"sub/a.tmp".to_string()));
        assert!(!files.iter().any(|f| f.starts_with(".git/")));
        // Eigene Ausschlussmuster gelten immer.
        let (files, _) = rels(&root, &Query { excludes: vec!["*.min.js".into(), "notes/".into()], include_ignored: true, ..base.clone() });
        assert!(!files.iter().any(|f| f.ends_with(".min.js") || f.starts_with("notes/")));
        // Ohne Größengrenze wird die große Datei durchsucht.
        let (files, s) = rels(&root, &Query { max_file_bytes: u64::MAX, ..base.clone() });
        assert!(files.contains(&"big.log".to_string()) && s.matches == 404);
        // Ungültiges Ausschlussmuster und fehlender Ordner werden beim Start gemeldet.
        assert!(start(root.clone(), Query { excludes: vec!["a[".into()], ..base.clone() }, || {}).is_err());
        assert!(start(root.join("fehlt"), base.clone(), || {}).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn background_job_streams_and_the_panel_model_collects() {
        let root = tree();
        let mut r = Results::default();
        r.start(root.clone(), Query { max_file_bytes: u64::MAX, ..q("needle") }, || {});
        let t0 = Instant::now();
        while r.running() && t0.elapsed().as_secs() < 20 {
            r.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let s = r.summary.clone().expect("fertig");
        assert_eq!(s.files_with_matches, r.files.len() as u64);
        assert_eq!(r.total_matches(), s.matches);
        assert!(r.error.is_none());
        // Navigation über Dateigrenzen
        let first = r.step(true).unwrap();
        assert_eq!(first.0, r.files[0].path);
        let n = r.files.len() - 1;
        let last = r.step(false).unwrap();
        assert_eq!(last, r.target(n, r.files[n].lines.len() - 1).unwrap(), "vom ersten zurück = der letzte");
        assert_eq!(r.step(true).unwrap(), first, "und wieder vorwärts zum ersten");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn caps_stop_the_search() {
        let d = std::env::temp_dir().join(format!("pd-find-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), "x\n".repeat(MAX_MATCHES as usize + 10)).unwrap();
        std::fs::write(d.join("b.txt"), "x\n").unwrap();
        let (files, s) = rels(&d, &Query { max_file_bytes: u64::MAX, ..q("x") });
        assert_eq!(files, ["a.txt"]);
        assert!(s.truncated);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn invalid_regex_reports_an_error_in_the_model() {
        let mut r = Results::default();
        r.start(std::env::temp_dir(), Query { regex: true, ..q("(") }, || {});
        assert!(r.error.is_some() && !r.running());
    }
}
