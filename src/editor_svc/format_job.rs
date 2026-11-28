//! Ein Formatier-Lauf im Hintergrund (#381, 3.2.0) – ohne egui, ohne App. Gemeinsam für die Menüleiste des Editors,
//! Tastenkürzel, Befehls-Palette und das MCP-Werkzeug `format_file` (nie von allein: Formatiert wird nur auf Anforderung).
//!
//! Ablauf: Der Aufrufer nimmt einen Klon des Ropes (billig) und optional einen Zeichenbereich (Auswahl) und startet
//! [`Job::start_with`]. Der Hilfsthread baut den Text, erkennt die Art genau ([`format::detect`] mit dem ganzen Text), ruft
//! [`format::run`] und rechnet das Ergebnis in Änderungen um ([`Done::Changed`], Zeichenbereiche im Text beim Start): beim
//! ganzen Dokument bis [`DIFF_MAX_BYTES`] zeilenweise minimal (Cursor und Markierungen der übrigen Zeilen bleiben), darüber
//! ein einziger Ersatz. Der Aufrufer wendet sie im UI-Thread als **einen** Rückgängig-Schritt an – nur, wenn sich das
//! Dokument seitdem nicht geändert hat (`revision`). Fehler tragen absolute Positionen (1-basiert) im Dokument.
//!
//! Auswahl: Formatiert wird nur ihr Text ohne Leerraum am Anfang und Ende (der bleibt stehen); beim Einrücken bekommen
//! die Folgezeilen die Einrückung der Zeile, in der die Auswahl beginnt. Ist die Auswahl kein vollständiger Wert bzw.
//! kein vollständiges Element, kommt der Fehler des Prüfens zurück und nichts wird geändert.

use crate::editor::format::{self, FormatError, Indent, Kind, Op, Options, Outcome};
use crate::editor::{IndentSettings, IndentStyle, TextEdit};
use ropey::Rope;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

/// Bis zu dieser Größe (ganzes Dokument) werden die Änderungen zeilenweise minimal berechnet, darüber ersetzt ein
/// einziger Bereich den ganzen Text (der Zeilenvergleich wäre zu teuer).
pub const DIFF_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Frist eines Laufs für MCP `format_file` (die Brücke wartet 30 s auf die Antwort).
pub const AGENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

/// Was zu tun ist.
#[derive(Clone, Debug)]
pub struct Request {
    pub op: Op,
    /// Datei (für die Erkennung über Endung und Dateinamen).
    pub path: Option<PathBuf>,
    /// In der Statusleiste gewählte Sprache (Id; `None` = nicht gewählt, dann erkennt der Lauf die Art selbst).
    pub lang: Option<String>,
    pub rope: Rope,
    /// Zeichenbereich der Auswahl; `None` = ganzes Dokument.
    pub range: Option<Range<usize>>,
    pub opts: Options,
    /// JSONC *Minify* wurde bestätigt (Kommentare fallen weg); sonst kommt [`Done::NeedsConfirm`].
    pub confirmed: bool,
    /// Bis zu dieser Größe (Bytes) zeilenweise minimale Änderungen, darüber ein einziger Ersatz
    /// ([`DIFF_MAX_BYTES`]).
    pub diff_max: usize,
}

/// Ergebnis eines Laufs.
#[derive(Clone, Debug, PartialEq)]
pub enum Done {
    /// Änderungen (Zeichenbereiche im Text beim Start, aufsteigend, ohne Überlappung). `whole` = ein einziger Ersatz
    /// des ganzen Dokuments (der Aufrufer setzt den Cursor dann selbst auf die alte Zeile/Spalte).
    Changed { kind: Kind, edits: Vec<TextEdit>, whole: bool },
    /// Schon so formatiert – nichts zu tun.
    Unchanged(Kind),
    /// *Prüfen* war erfolgreich.
    Valid(Kind),
    /// Ungültig (absolute Position im Dokument, 1-basiert) – nichts wurde verändert.
    Invalid(Kind, FormatError),
    /// Weder JSON noch XML.
    NotSupported,
    /// *Schlüssel sortieren* gibt es für XML nicht.
    Unsupported(Kind),
    /// JSONC *Minify* entfernt Kommentare: erst nachfragen.
    NeedsConfirm(Kind),
    Cancelled,
}

/// Einrückung des Dokuments (aus `.editorconfig` bzw. der Erkennung) für den Formatierer.
pub fn indent_of(s: &IndentSettings) -> Indent {
    match s.style {
        IndentStyle::Tabs => Indent::Tab,
        IndentStyle::Spaces(n) => Indent::Spaces(n.clamp(1, 16)),
    }
}

/// Kurzname der Art für Meldungen und das MCP-Werkzeug (`json`, `jsonc`, `xml`).
pub fn kind_id(k: Kind) -> &'static str {
    match k {
        Kind::Json => "json",
        Kind::Jsonc => "jsonc",
        Kind::Xml => "xml",
    }
}

/// Anzeigename der Art („JSON“ auch für JSONC, wo es um die Sprache geht).
pub fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Json => "JSON",
        Kind::Jsonc => "JSONC",
        Kind::Xml => "XML",
    }
}

/// Art eines Dokuments für Menüs (billig: Pfad und Anfang des Textes; JSON/JSONC nur ungefähr – genau erkennt der Lauf
/// mit dem ganzen Text). `lang` = in der Statusleiste gewählte Sprache ([`resolve_kind`]).
pub fn quick_kind(path: Option<&std::path::Path>, rope: &Rope, lang: Option<&str>) -> Option<Kind> {
    let end = rope.len_chars().min(4096);
    let head = rope.slice(..end).to_string();
    resolve_kind(path, &head, lang)
}

/// Art aus Pfad, Text und der in der Statusleiste gewählten Sprache: gewählt `json` → JSON (JSONC bei Kommentaren bzw.
/// JSONC-Namen), `xml` → XML, eine andere Sprache → nichts (der Nutzer hat gesagt, was es ist); nicht gewählt →
/// [`format::detect`].
pub fn resolve_kind(path: Option<&std::path::Path>, text: &str, lang: Option<&str>) -> Option<Kind> {
    match lang {
        None => format::detect(path, text),
        Some("json") => Some(match format::detect(path, text) {
            Some(k @ (Kind::Json | Kind::Jsonc)) => k,
            _ if format::json::has_comments(text) => Kind::Jsonc,
            _ => Kind::Json,
        }),
        Some("xml") => Some(Kind::Xml),
        Some(_) => None,
    }
}

/// Der eingebaute Formatierer: ganzes Dokument über [`format::run`]; eine XML-Auswahl (`fragment`) über
/// `xml::run_with(…, fragment = true, …)` – sie darf mehrere oder keine Elemente auf oberster Ebene haben.
pub fn builtin(kind: Kind, op: Op, text: &str, opts: Options, fragment: bool, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Outcome {
    if fragment && kind == Kind::Xml {
        format::xml::run_with(op, text, opts, true, cancel, progress)
    } else {
        format::run(kind, op, text, opts, cancel, progress)
    }
}

/// Signatur des Formatierers (in Tests ersetzbar).
/// Argumente: Art, Befehl, Text, Optionen, `fragment` (Text ist eine Auswahl, kein ganzes Dokument), Abbruch, Fortschritt.
pub type Runner = dyn Fn(Kind, Op, &str, Options, bool, &AtomicBool, &dyn Fn(f32)) -> Outcome + Send + Sync;
/// Wie [`Runner`], ohne `Send` (für [`compute_with`]).
pub type RunRef = dyn Fn(Kind, Op, &str, Options, bool, &AtomicBool, &dyn Fn(f32)) -> Outcome + Sync;

/// Den Lauf synchron ausführen (Hilfsthread, Tests) mit einem Formatierer.
pub fn compute_with(req: &Request, cancel: &AtomicBool, progress: &dyn Fn(f32), run: &RunRef) -> Done {
    let len = req.rope.len_chars();
    let range = req.range.clone().map(|r| r.start.min(len)..r.end.min(len)).filter(|r| r.start < r.end);
    let full = req.rope.to_string();
    let Some(kind) = resolve_kind(req.path.as_deref(), &full, req.lang.as_deref()) else {
        return Done::NotSupported;
    };
    if !format::supports(kind, req.op) {
        return Done::Unsupported(kind);
    }
    if kind == Kind::Jsonc && req.op == Op::Minify && !req.confirmed {
        return Done::NeedsConfirm(kind);
    }
    if cancel.load(Ordering::Relaxed) {
        return Done::Cancelled;
    }
    match range {
        None => {
            let out = run(kind, req.op, &full, req.opts, false, cancel, progress);
            match out {
                Outcome::Cancelled => Done::Cancelled,
                Outcome::Invalid(e) => Done::Invalid(kind, e),
                Outcome::Text(_) if req.op == Op::Validate => Done::Valid(kind),
                Outcome::Text(t) if t == full => Done::Unchanged(kind),
                Outcome::Text(t) => {
                    if full.len() <= req.diff_max {
                        Done::Changed { kind, edits: crate::editor::diff::minimal_edits(&full, &t), whole: false }
                    } else {
                        Done::Changed { kind, edits: vec![TextEdit::new(0..len, t)], whole: true }
                    }
                }
            }
        }
        Some(r) => {
            let sel = req.rope.slice(r.clone()).to_string();
            // Leerraum am Anfang und Ende der Auswahl bleibt stehen.
            let core = sel.trim_matches(|c: char| c.is_whitespace());
            if core.is_empty() {
                return if req.op == Op::Validate { Done::Invalid(kind, FormatError { line: line_of(&req.rope, r.start) + 1, column: col_of(&req.rope, r.start) + 1, message: "empty selection".into() }) } else { Done::Unchanged(kind) };
            }
            let lead_bytes = sel.len() - sel.trim_start_matches(|c: char| c.is_whitespace()).len();
            let lead = &sel[..lead_bytes];
            let trail = &sel[lead_bytes + core.len()..];
            let core_start = r.start + lead.chars().count();
            let opts = Options { final_newline: false, ..req.opts };
            match run(kind, req.op, core, opts, true, cancel, progress) {
                Outcome::Cancelled => Done::Cancelled,
                Outcome::Invalid(e) => Done::Invalid(kind, absolute(&req.rope, core_start, e)),
                Outcome::Text(_) if req.op == Op::Validate => Done::Valid(kind),
                Outcome::Text(t) => {
                    let t = t.trim_end_matches('\n');
                    // Einrückung der Zeile, in der der Wert beginnt (nicht die Spalte 0 einer ganzen markierten Zeile
                    // oder das Ende der Zeile davor).
                    let body = if req.op == Op::Minify { t.to_string() } else { reindent(t, &base_indent(&req.rope, core_start)) };
                    let new = format!("{lead}{body}{trail}");
                    if new == sel { Done::Unchanged(kind) } else { Done::Changed { kind, edits: vec![TextEdit::new(r, new)], whole: false } }
                }
            }
        }
    }
}

fn line_of(rope: &Rope, pos: usize) -> usize {
    rope.char_to_line(pos.min(rope.len_chars()))
}

fn col_of(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    pos - rope.line_to_char(rope.char_to_line(pos))
}

/// Fehler im Auswahltext (ab `start`) → Position im Dokument.
pub fn absolute(rope: &Rope, start: usize, e: FormatError) -> FormatError {
    let (l0, c0) = (line_of(rope, start), col_of(rope, start));
    let line = l0 + e.line.max(1);
    let column = if e.line <= 1 { c0 + e.column.max(1) } else { e.column.max(1) };
    FormatError { line, column, message: e.message }
}

/// Einrückung (Leerraum am Anfang) der Zeile, in der `pos` liegt – höchstens bis `pos`.
fn base_indent(rope: &Rope, pos: usize) -> String {
    let line = line_of(rope, pos);
    let start = rope.line_to_char(line);
    rope.slice(start..pos.min(rope.len_chars())).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// Folgezeilen (nicht leer) um `base` einrücken.
fn reindent(text: &str, base: &str) -> String {
    if base.is_empty() || !text.contains('\n') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + base.len() * 16);
    for (i, l) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if !l.is_empty() {
                out.push_str(base);
            }
        }
        out.push_str(l);
    }
    out
}

/// Laufender Lauf im Hilfsthread.
pub struct Job {
    pub op: Op,
    /// Stand des Dokuments beim Start (`Document::revision`).
    pub revision: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    rx: Receiver<Done>,
}

impl Job {
    /// Lauf starten mit dem Formatierer `run` (meist [`builtin`]; Tests: Ersatz für Fortschritt und *Abbrechen*);
    /// `wake` wird nach Fortschritt (grob) und am Ende gerufen (z. B. `request_repaint`).
    pub fn start_with(req: Request, revision: u64, wake: impl Fn() + Send + Sync + 'static, run: Arc<Runner>) -> Job {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
        let (tx, rx) = std::sync::mpsc::channel();
        let (c2, p2) = (cancel.clone(), progress.clone());
        let op = req.op;
        std::thread::spawn(move || {
            let last = AtomicU32::new(0);
            let report = |f: f32| {
                let f = f.clamp(0.0, 1.0);
                p2.store(f.to_bits(), Ordering::Relaxed);
                // Höchstens jedes Prozent wecken.
                let pct = (f * 100.0) as u32;
                if last.swap(pct, Ordering::Relaxed) != pct {
                    wake();
                }
            };
            let done = compute_with(&req, &c2, &report, &*run);
            let _ = tx.send(done);
            wake();
        });
        Job { op, revision, cancel, progress, rx }
    }

    /// Fortschritt 0.0–1.0.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Fertig? (`Some` genau einmal.) Ein abgestürzter Thread gilt als abgebrochen.
    pub fn poll(&self) -> Option<Done> {
        match self.rx.try_recv() {
            Ok(d) => Some(d),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Done::Cancelled),
        }
    }

    /// Auf das Ende warten (Tests).
    #[cfg(test)]
    pub fn wait(&self) -> Done {
        self.rx.recv().unwrap_or(Done::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ersatz-Formatierer: rückt jedes `,` mit Zeilenumbruch ein, `!` ist ungültig (an seiner Stelle).
    fn fake(_k: Kind, op: Op, t: &str, o: Options, _f: bool, _c: &AtomicBool, _p: &dyn Fn(f32)) -> Outcome {
        if let Some(i) = t.find('!') {
            let before = &t[..i];
            let line = before.matches('\n').count() + 1;
            let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
            return Outcome::Invalid(FormatError { line, column: col, message: "bang".into() });
        }
        let ind = match o.indent {
            Indent::Spaces(n) => " ".repeat(n as usize),
            Indent::Tab => "\t".into(),
        };
        let mut s = match op {
            Op::Minify => t.replace([' ', '\n'], ""),
            Op::Validate => return Outcome::Text(t.to_string()),
            _ => t.trim().replace(",", &format!(",\n{ind}")),
        };
        if o.final_newline && !s.ends_with('\n') {
            s.push('\n');
        }
        Outcome::Text(s)
    }

    fn req(text: &str, op: Op, range: Option<Range<usize>>) -> Request {
        Request { op, path: Some(PathBuf::from("a.json")), lang: None, rope: Rope::from_str(text), range, opts: Options::default(), confirmed: false, diff_max: DIFF_MAX_BYTES }
    }

    fn run(r: &Request) -> Done {
        compute_with(r, &AtomicBool::new(false), &|_| {}, &fake)
    }

    fn apply(text: &str, d: &Done) -> String {
        let Done::Changed { edits, .. } = d else { panic!("{d:?}") };
        let mut r = Rope::from_str(text);
        for e in edits.iter().rev() {
            r.remove(e.range.clone());
            r.insert(e.range.start, &e.text);
        }
        r.to_string()
    }

    #[test]
    fn whole_document_gives_minimal_edits_and_unchanged_stays() {
        let text = "keep\n[1,2]\nkeep\n";
        let d = run(&req(text, Op::Format, None));
        assert_eq!(apply(text, &d), "keep\n[1,\n  2]\nkeep\n");
        let Done::Changed { edits, whole, .. } = &d else { unreachable!() };
        assert!(!whole && edits.iter().all(|e| e.range.start >= 5), "erste Zeile bleibt unberührt: {edits:?}");
        assert_eq!(run(&req("[1]\n", Op::Format, None)), Done::Unchanged(Kind::Json));
        assert_eq!(run(&req("[1]\n", Op::Validate, None)), Done::Valid(Kind::Json));
    }

    #[test]
    fn selection_is_formatted_alone_keeps_surrounding_space_and_indents() {
        let text = "{\n    \"a\": [1,2] \n}\n";
        let start = text.find('[').unwrap();
        let end = text.find(']').unwrap() + 2; // mit Leerzeichen
        let d = run(&req(text, Op::Format, Some(start..end)));
        assert_eq!(apply(text, &d), "{\n    \"a\": [1,\n      2] \n}\n", "Folgezeile mit der Einrückung der Zeile");
        // Fehler in der Auswahl → absolute Position im Dokument
        let text = "{\n  \"a\": [1,!]\n}";
        let s = text.find('[').unwrap();
        let d = run(&req(text, Op::Format, Some(s..s + 5)));
        assert_eq!(d, Done::Invalid(Kind::Json, FormatError { line: 2, column: 11, message: "bang".into() }));
        // Leere Auswahl (nur Leerraum)
        assert_eq!(run(&req("[ ]", Op::Format, Some(1..2))), Done::Unchanged(Kind::Json));
    }

    #[test]
    fn invalid_unsupported_and_jsonc_minify_need_attention() {
        assert!(matches!(run(&req("[1,!]", Op::Format, None)), Done::Invalid(Kind::Json, FormatError { line: 1, column: 4, .. })));
        let mut r = req("hello", Op::Format, None);
        r.path = Some(PathBuf::from("a.txt"));
        assert_eq!(run(&r), Done::NotSupported);
        let mut r = req("<a/>", Op::SortKeys, None);
        r.path = Some(PathBuf::from("a.xml"));
        assert_eq!(run(&r), Done::Unsupported(Kind::Xml));
        let mut r = req("{}\n", Op::Minify, None);
        r.path = Some(PathBuf::from("tsconfig.json"));
        assert_eq!(run(&r), Done::NeedsConfirm(Kind::Jsonc));
        r.confirmed = true;
        assert_eq!(run(&r), Done::Unchanged(Kind::Jsonc));
        // Gewählte Sprache geht vor: `json` formatiert auch ohne passende Endung, eine andere Sprache nie.
        let mut r = req("1,2", Op::Format, None);
        r.path = None;
        assert_eq!(run(&r), Done::NotSupported);
        r.lang = Some("json".into());
        assert!(matches!(run(&r), Done::Changed { kind: Kind::Json, .. }));
        let mut r = req("[1,2]", Op::Format, None);
        r.lang = Some("python".into());
        assert_eq!(run(&r), Done::NotSupported, "gewählte andere Sprache");
        // Über `diff_max` ein einziger Ersatz
        let mut r = req("keep\n[1,2]\n", Op::Format, None);
        r.diff_max = 4;
        assert!(matches!(run(&r), Done::Changed { whole: true, .. }));
    }

    /// Review #381: ganze Zeilen ab Spalte 0 (Umschalt+Pfeil runter) bzw. ab dem Ende der Zeile davor markiert – die
    /// Folgezeilen bekommen die Einrückung der Zeile, in der der Wert beginnt.
    #[test]
    fn selection_of_whole_lines_keeps_the_value_indentation() {
        let text = "{\n    \"a\": 1,\n    [1,2]\n}\n";
        // ganze Zeile 3 samt Zeilenende, ab Spalte 0
        let s = text.find("    [").unwrap();
        let e = text.find("]\n").unwrap() + 2;
        let d = run(&req(text, Op::Format, Some(s..e)));
        assert_eq!(apply(text, &d), "{\n    \"a\": 1,\n    [1,\n      2]\n}\n");
        // ab dem Ende der Zeile davor
        let s = text.find(",\n    [").unwrap() + 1;
        let e = text.find(']').unwrap() + 1;
        let d = run(&req(text, Op::Format, Some(s..e)));
        assert_eq!(apply(text, &d), "{\n    \"a\": 1,\n    [1,\n      2]\n}\n");
    }

    /// Review #381: Inhalt erkennt nur bei Dateien ohne Endung bzw. unbenannten Tabs.
    #[test]
    fn content_sniffing_only_without_extension() {
        use std::path::Path;
        let k = |name: Option<&str>, text: &str| resolve_kind(name.map(Path::new), text, None);
        assert_eq!(k(Some("Cargo.toml"), "[package]\nname = \"x\"\n"), None);
        assert_eq!(k(Some("setup.ini"), "[core]\n"), None);
        assert_eq!(k(Some("build.ps1"), "[CmdletBinding()]\nparam()\n"), None);
        assert_eq!(k(Some("README.md"), "[![CI](x)](y)\n"), None);
        assert_eq!(k(Some(".gitconfig"), "[user]\n"), None);
        assert_eq!(k(Some("a.txt"), "{\"a\": 1}"), None);
        assert_eq!(k(Some("data.json"), "{}"), Some(Kind::Json));
        assert_eq!(k(Some("tsconfig.json"), "{}"), Some(Kind::Jsonc));
        assert_eq!(k(Some(".babelrc"), "{}"), Some(Kind::Json));
        assert_eq!(k(Some("app.csproj"), "<Project/>"), Some(Kind::Xml));
        assert_eq!(k(Some("Web.config"), "<configuration/>"), Some(Kind::Xml));
        // ohne Endung bzw. unbenannt: am Inhalt
        assert_eq!(k(Some("LICENSE"), "[1, 2]"), Some(Kind::Json));
        assert_eq!(k(None, "{\"a\": 1}"), Some(Kind::Json));
        assert_eq!(k(None, "<?xml version=\"1.0\"?><a/>"), Some(Kind::Xml));
        assert_eq!(k(None, "[section]\nkey = 1"), None, "INI ist kein JSON");
        assert_eq!(k(None, "[![badge](x)](y)"), None, "Markdown ist kein JSON");
        assert_eq!(k(None, "[true, null]"), Some(Kind::Json));
        assert_eq!(k(None, "[tool.poetry]"), None);
        // gewählte Sprache
        assert_eq!(resolve_kind(Some(Path::new("a.json")), "{}", Some("rust")), None);
        assert_eq!(resolve_kind(Some(Path::new("notes.txt")), "<a/>", Some("xml")), Some(Kind::Xml));
        assert_eq!(resolve_kind(Some(Path::new("tsconfig.json")), "{}", Some("json")), Some(Kind::Jsonc));
    }

    /// Review #381: Anwenden eines einzigen Ersatzes bei sehr großen Dateien (~200 MB) – Zeit messen
    /// (`cargo test --release -- --ignored large_whole_replacement --nocapture`).
    #[test]
    #[ignore]
    fn large_whole_replacement_apply_time() {
        let line = "{\"id\": 123456, \"name\": \"some value here\", \"tags\": [1, 2, 3]},\n";
        let n = 200 * 1024 * 1024 / line.len();
        let old: String = std::iter::once("[\n").chain(std::iter::repeat_n(line, n)).chain(std::iter::once("{}]\n")).collect();
        let new = old.replace(", ", ",\n  ");
        let mut doc = crate::editor::Document::from_text(&old);
        let mut sels = doc.new_selections(0);
        let len = doc.len_chars();
        let t0 = std::time::Instant::now();
        doc.break_undo_group();
        assert!(doc.apply_edits(&mut sels, vec![TextEdit::new(0..len, new)], crate::editor::EditOrigin::User).unwrap());
        let apply = t0.elapsed();
        let t1 = std::time::Instant::now();
        assert!(doc.undo(&mut sels));
        eprintln!("LARGE {} MB: apply {:?}, undo {:?}", old.len() / (1024 * 1024), apply, t1.elapsed());
    }

    #[test]
    fn job_runs_in_the_background_reports_progress_and_cancels() {
        let slow: Arc<Runner> = Arc::new(|_k: Kind, _op: Op, t: &str, _o: Options, _f: bool, c: &AtomicBool, p: &dyn Fn(f32)| {
            for i in 0..2000 {
                if c.load(Ordering::Relaxed) {
                    return Outcome::Cancelled;
                }
                p(i as f32 / 2000.0);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Outcome::Text(format!("{t} "))
        });
        let job = Job::start_with(req("[1]", Op::Format, None), 7, || {}, slow);
        assert_eq!(job.revision, 7);
        let t0 = std::time::Instant::now();
        while job.progress() == 0.0 && t0.elapsed().as_secs() < 5 {
            std::thread::yield_now();
        }
        assert!(job.progress() > 0.0, "Fortschritt kommt an");
        job.cancel();
        assert_eq!(job.wait(), Done::Cancelled);
        // Ohne Abbruch: Ergebnis
        let job = Job::start_with(req("[1]\n", Op::Validate, None), 1, || {}, Arc::new(builtin) as Arc<Runner>);
        assert_eq!(job.wait(), Done::Valid(Kind::Json));
    }

    #[test]
    fn indent_and_kind_helpers() {
        assert_eq!(indent_of(&IndentSettings { style: IndentStyle::Tabs, ..Default::default() }), Indent::Tab);
        assert_eq!(indent_of(&IndentSettings { style: IndentStyle::Spaces(4), ..Default::default() }), Indent::Spaces(4));
        assert_eq!(kind_id(Kind::Jsonc), "jsonc");
        assert_eq!(quick_kind(Some(std::path::Path::new("a.xml")), &Rope::from_str("<a/>"), None), Some(Kind::Xml));
        assert_eq!(quick_kind(None, &Rope::from_str("hello"), Some("json")), Some(Kind::Json));
        assert_eq!(quick_kind(Some(std::path::Path::new("a.txt")), &Rope::from_str("hello"), None), None);
        assert_eq!(reindent("a\n\nb", "  "), "a\n\n  b");
    }
}
