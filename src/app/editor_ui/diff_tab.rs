//! Vergleichs-Reiter des Editors (#385, 3.2.0) – angelehnt an Sublime Text/Sublime Merge: zwei Seiten nebeneinander
//! (in schmalen Teilungen unter ~700 px untereinander, umschaltbar), synchron rollend mit schraffierten Füllzeilen,
//! Änderungen innerhalb der Zeile (Wörter, kurze Wörter auf Zeichen verfeinert), unveränderte Bereiche eingeklappt (3
//! Kontextzeilen, aufklappbar), Zeilennummern beider Seiten, Syntaxfarben, Blockpfeile in beide Richtungen.
//!
//! Eine Seite ist entweder ein **Dokument** des Editors (dasselbe wie im normalen Reiter: gemeinsames Rückgängig,
//! Ungespeichert-Punkt, Strg+S – jeder Blockpfeil ist ein Rückgängig-Schritt) oder ein **fester Text**: die Datei auf der
//! Platte (gespeicherter Stand), ein Stand in git (HEAD, Index, Commit/Zweig/Tag – nur lokal) oder die Zwischenablage;
//! feste Texte sind schreibgeschützt. Der Vergleich läuft im Hintergrund (~250 ms nach dem Tippen, Fortschritt und
//! *Abbrechen* bei großen Dateien, Zeitgrenze mit gröberem Ergebnis); git-Stände werden nach dem Speichern, bei
//! Fensterfokus und bei Änderungen an `.git/index`/`HEAD` neu gelesen.
//!
//! Ab 3.3.0 (#391) lässt sich in einer bearbeitbaren Seite direkt tippen: ein Cursor (eine Auswahl) im Dokument der
//! Seite, Füllzeilen überspringt er, Bearbeiten in einem eingeklappten Bereich klappt ihn auf. Der Text erscheint sofort
//! (`DiffTab::live_update`: neuer Stand der Seite, Blöcke über `compare::edit_hunks` verschoben bzw. zusammengelegt),
//! der richtige Vergleich folgt ~300 ms nach der letzten Taste. Tippen und Blockpfeile teilen das Rückgängig des
//! Dokuments.

use super::doc::{DocEntry, Op};
use super::view::Cmd;
use crate::editor::compare::{self, Facts, Hunk, Item, Row, RowKind};
use crate::editor::syntax::{DocHighlight, Span};
use crate::editor::text as et;
use crate::editor::{Document, Selection, Selections, TextEncoding};
use crate::editor_svc::git_base::{self, Blob, RepoFile, Rev};
use crate::editor_svc::settings::DiffLayout;
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Align2, Color32, Event, FontId, Id, Key, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// So lange nach der letzten Änderung (Tipp-Pause, #391) wird neu verglichen.
pub(super) const LIVE_DELAY: Duration = Duration::from_millis(300);
/// So oft wird geprüft, ob sich ein fester Stand (Platte, git) geändert hat.
const REFRESH: Duration = Duration::from_secs(2);
/// Schmaler als das: automatisch untereinander.
pub(super) const AUTO_UNIFIED_W: f32 = 700.0;
pub(super) const TOOLBAR_H: f32 = 30.0;
const INFO_H: f32 = 24.0;
const CENTER_W: f32 = 34.0;
const ACTION_W: f32 = 34.0;
const SCROLLBAR_W: f32 = 10.0;
/// Fortschritt erst zeigen, wenn der Vergleich so lange dauert.
const SHOW_PROGRESS_AFTER: Duration = Duration::from_millis(300);
/// Höchstzahl Zeilenpaare, für die Änderungen in der Zeile berechnet werden.
const INTRA_BUDGET: usize = 20_000;
/// Aufklappen in Schritten von so vielen Zeilen.
pub(super) const EXPAND_STEP: u32 = 20;

/// Woher ein fester Text kommt.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    /// Die Datei auf der Platte (gespeicherter Stand).
    Disk(PathBuf),
    /// Ein Stand in git.
    Git { path: PathBuf, rev: Rev },
    /// Die Zwischenablage (beim Öffnen gelesen).
    Clipboard,
}

/// Womit ein Dokument verglichen werden soll (Menüs, Palette, Kontextmenüs).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Target {
    /// Gespeicherter Stand auf der Platte.
    Disk,
    Head,
    Index,
    Rev(String),
    Clipboard,
    File(PathBuf),
    /// Datei im Dialog wählen.
    PickFile,
    /// Ein anderes offenes Dokument.
    Doc(u64),
}

/// Ein fester (schreibgeschützter) Text einer Seite.
pub(crate) struct StaticSide {
    pub source: Source,
    text: Arc<String>,
    facts: Facts,
    raw_len: u64,
    raw_sha: Option<String>,
    binary: bool,
    /// Hinweis (z. B. „nicht in HEAD“) oder Fehler beim Lesen.
    note: Option<String>,
    version: u64,
    stamp: Option<u64>,
    repo: Option<Option<RepoFile>>,
    load: Option<mpsc::Receiver<Loaded>>,
    loaded: bool,
    checked: Option<Instant>,
    seen_gen: u64,
    hl: Option<StaticHl>,
}

struct StaticHl {
    lang: String,
    version: u64,
    doc: Document,
    hl: DocHighlight,
}

/// Ergebnis des Lesens eines festen Stands.
enum Loaded {
    Unchanged { repo: Option<Option<RepoFile>> },
    Data { text: String, facts: Facts, raw_len: u64, raw_sha: String, binary: bool, stamp: u64, note: Option<String>, repo: Option<Option<RepoFile>> },
}

impl StaticSide {
    pub(crate) fn new(source: Source) -> StaticSide {
        StaticSide { source, text: Arc::new(String::new()), facts: Facts::default(), raw_len: 0, raw_sha: None, binary: false, note: None, version: 1, stamp: None, repo: None, load: None, loaded: false, checked: None, seen_gen: 0, hl: None }
    }

    /// Zwischenablage (sofort gelesen; ohne Kodierung und Zeilenenden, die gibt es dort nicht).
    pub(crate) fn clipboard(text: &str) -> StaticSide {
        let (t, _) = crate::editor::encoding::normalize_eol(text);
        let mut s = StaticSide::new(Source::Clipboard);
        s.raw_len = text.len() as u64;
        s.raw_sha = Some(sha_hex(text.as_bytes()));
        s.facts = Facts { encoding: None, eol: None, final_newline: t.ends_with('\n') };
        s.text = Arc::new(t);
        s.loaded = true;
        s
    }

    fn label(&self, lang: Lang) -> String {
        match &self.source {
            Source::Disk(p) => format!("{} ({})", file_name(p), lang.t("gespeichert", "saved")),
            Source::Git { path, rev } => format!("{} ({})", file_name(path), match rev {
                Rev::Index => lang.t("Index", "index").to_string(),
                r => r.label(),
            }),
            Source::Clipboard => lang.t("Zwischenablage", "Clipboard").to_string(),
        }
    }

    fn path(&self) -> Option<&Path> {
        match &self.source {
            Source::Disk(p) | Source::Git { path: p, .. } => Some(p),
            Source::Clipboard => None,
        }
    }

    /// Neu lesen, wenn nötig (erstes Mal, alle [`REFRESH`], nach Speichern/Fokus/git-Änderung). `enc` = Kodierung des
    /// zugehörigen Dokuments (für den gespeicherten Stand).
    fn tick(&mut self, now: Instant, generation: u64, max: u64, enc: Option<TextEncoding>) -> bool {
        if let Some(rx) = &self.load {
            match rx.try_recv() {
                Ok(Loaded::Unchanged { repo }) => {
                    self.load = None;
                    self.repo = repo.or(self.repo.take());
                }
                Ok(Loaded::Data { text, facts, raw_len, raw_sha, binary, stamp, note, repo }) => {
                    self.load = None;
                    self.text = Arc::new(text);
                    self.facts = facts;
                    self.raw_len = raw_len;
                    self.raw_sha = Some(raw_sha);
                    self.binary = binary;
                    self.stamp = Some(stamp);
                    self.note = note;
                    self.repo = repo.or(self.repo.take());
                    self.version += 1;
                    self.loaded = true;
                }
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.load = None;
                    self.loaded = true;
                }
            }
            return false;
        }
        if self.source == Source::Clipboard {
            return false;
        }
        let due = !self.loaded || generation != self.seen_gen || self.checked.is_none_or(|c| now.duration_since(c) >= REFRESH);
        if !due {
            return false;
        }
        self.seen_gen = generation;
        self.checked = Some(now);
        let (tx, rx) = mpsc::channel();
        let (source, prev, repo) = (self.source.clone(), if self.loaded { self.stamp } else { None }, self.repo.clone());
        std::thread::spawn(move || {
            let _ = tx.send(load_source(&source, prev, repo, max, enc));
        });
        self.load = Some(rx);
        true
    }
}

fn e_title(docs: &BTreeMap<u64, DocEntry>, d: u64) -> String {
    docs.get(&d).map(|e| e.title(Lang::En)).unwrap_or_default()
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string())
}

pub(super) fn sha_hex(b: &[u8]) -> String {
    hex(&crate::app::update::sha256::digest(b))
}

fn hex(d: &[u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 einer Datei in Blöcken (auch sehr große Dateien).
fn sha_file(p: &Path) -> Option<(u64, String)> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = crate::app::update::sha256::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf).ok()?;
        if k == 0 {
            break;
        }
        n += k as u64;
        h.update(&buf[..k]);
    }
    Some((n, hex(&h.finish())))
}

fn decoded(bytes: &[u8], enc: Option<TextEncoding>) -> (String, Facts, bool) {
    facts_of(crate::editor::encoding::decode(bytes, enc), bytes)
}

/// Ein Stand in git: reiner ASCII-Inhalt in der Kodierung des Dokuments (keine falsche Kodierungs-Meldung).
fn decoded_git(bytes: &[u8], enc: Option<TextEncoding>) -> (String, Facts, bool) {
    facts_of(git_base::decode_like(bytes, enc), bytes)
}

fn facts_of(d: crate::editor::encoding::Decoded, bytes: &[u8]) -> (String, Facts, bool) {
    let facts = Facts { encoding: Some(d.encoding.label().to_string()), eol: compare::eol_label(&d.eol_stats), final_newline: d.text.ends_with('\n') };
    let binary = compare::looks_binary(bytes);
    (d.text, facts, binary)
}

/// Einen festen Stand lesen (Hilfsthread). `prev` = Stempel des letzten Lesens: gleich → `Unchanged`.
fn load_source(source: &Source, prev: Option<u64>, repo: Option<Option<RepoFile>>, max: u64, enc: Option<TextEncoding>) -> Loaded {
    let t = crate::state::t;
    match source {
        Source::Clipboard => Loaded::Unchanged { repo },
        Source::Disk(p) => {
            let stamp = git_base::file_stamp(p);
            if prev == Some(stamp) {
                return Loaded::Unchanged { repo };
            }
            let len = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            if len > max {
                let (n, sha) = sha_file(p).unwrap_or((len, String::new()));
                return Loaded::Data { text: String::new(), facts: Facts::default(), raw_len: n, raw_sha: sha, binary: false, stamp, note: Some(t("Zu groß zum Vergleichen", "Too large to compare").into()), repo };
            }
            match std::fs::read(p) {
                Ok(b) => {
                    let (text, facts, binary) = decoded(&b, enc);
                    Loaded::Data { text, facts, raw_len: b.len() as u64, raw_sha: sha_hex(&b), binary, stamp, note: None, repo }
                }
                Err(_) => Loaded::Data { text: String::new(), facts: Facts::default(), raw_len: 0, raw_sha: sha_hex(b""), binary: false, stamp, note: Some(t("Datei auf der Platte nicht gefunden", "File not found on disk").into()), repo },
            }
        }
        Source::Git { path, rev } => {
            let repo = repo.unwrap_or_else(|| git_base::locate(path));
            let Some(r) = repo.clone() else {
                return Loaded::Data { text: String::new(), facts: Facts::default(), raw_len: 0, raw_sha: sha_hex(b""), binary: false, stamp: 0, note: Some(t("Die Datei liegt in keinem git-Repository (oder git fehlt).", "The file is not in a git repository (or git is missing).").into()), repo: Some(None) };
            };
            let stamp = git_base::stamp(&r.git_dir);
            if prev == Some(stamp) {
                return Loaded::Unchanged { repo: Some(repo) };
            }
            match git_base::blobs(&r, std::slice::from_ref(rev), max) {
                Ok(v) => match v.into_iter().next() {
                    Some(Blob::Bytes(b)) => {
                        let (text, facts, binary) = decoded_git(&b, enc);
                        Loaded::Data { text, facts, raw_len: b.len() as u64, raw_sha: sha_hex(&b), binary, stamp, note: None, repo: Some(repo) }
                    }
                    Some(Blob::TooLarge(n)) => Loaded::Data { text: String::new(), facts: Facts::default(), raw_len: n, raw_sha: String::new(), binary: false, stamp, note: Some(t("Zu groß zum Vergleichen", "Too large to compare").into()), repo: Some(repo) },
                    _ => {
                        let lang = crate::state::lang();
                        let note = if lang == Lang::De { format!("Die Datei gibt es in {} nicht (leer gezeigt).", rev.label()) } else { format!("The file does not exist in {} (shown empty).", rev.label()) };
                        Loaded::Data { text: String::new(), facts: Facts { encoding: None, eol: None, final_newline: false }, raw_len: 0, raw_sha: sha_hex(b""), binary: false, stamp, note: Some(note), repo: Some(repo) }
                    }
                },
                Err(e) => Loaded::Data { text: String::new(), facts: Facts::default(), raw_len: 0, raw_sha: String::new(), binary: false, stamp, note: Some(e), repo: Some(repo) },
            }
        }
    }
}

/// Eine Seite des Vergleichs.
pub(crate) enum Side {
    /// Ein Dokument des Editors (bearbeitbar).
    Doc(u64),
    /// Ein fester Text (schreibgeschützt).
    Text(Box<StaticSide>),
}

impl Side {
    pub(crate) fn doc(&self) -> Option<u64> {
        match self {
            Side::Doc(d) => Some(*d),
            Side::Text(_) => None,
        }
    }

    fn label(&self, docs: &BTreeMap<u64, DocEntry>, lang: Lang) -> String {
        match self {
            Side::Doc(d) => docs.get(d).map(|e| e.title(lang)).unwrap_or_else(|| "?".into()),
            Side::Text(s) => s.label(lang),
        }
    }

    fn path(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<PathBuf> {
        match self {
            Side::Doc(d) => docs.get(d).and_then(|e| e.path.clone()),
            Side::Text(s) => s.path().map(Path::to_path_buf),
        }
    }

    /// Stand für den Vergleich: (Kennung, Version); `None` = noch nicht gelesen.
    fn version(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<(u64, u64)> {
        match self {
            Side::Doc(d) => docs.get(d).and_then(|e| e.revision()).map(|r| (*d, r)),
            Side::Text(s) => s.loaded.then_some((0, s.version)),
        }
    }

    fn facts(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<Facts> {
        match self {
            Side::Doc(d) => docs.get(d).and_then(|e| e.facts()),
            Side::Text(s) => s.loaded.then(|| s.facts.clone()),
        }
    }

    fn note(&self) -> Option<&str> {
        match self {
            Side::Text(s) => s.note.as_deref(),
            Side::Doc(_) => None,
        }
    }

    fn duplicate(&self) -> Side {
        match self {
            Side::Doc(d) => Side::Doc(*d),
            Side::Text(s) => {
                let mut n = StaticSide::new(s.source.clone());
                n.text = s.text.clone();
                n.facts = s.facts.clone();
                n.raw_len = s.raw_len;
                n.raw_sha = s.raw_sha.clone();
                n.binary = s.binary;
                n.note = s.note.clone();
                n.stamp = s.stamp;
                n.repo = s.repo.clone();
                n.loaded = s.loaded;
                Side::Text(Box::new(n))
            }
        }
    }
}

/// Text einer Seite zum Zeitpunkt des Vergleichs (Zeilenanfänge vorberechnet).
pub(crate) struct Snap {
    text: Arc<String>,
    starts: Vec<usize>,
    final_nl: bool,
}

impl Snap {
    fn new(text: Arc<String>) -> Snap {
        let mut starts = Vec::new();
        if !text.is_empty() {
            starts.push(0);
            starts.extend(text.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i + 1));
            if text.ends_with('\n') {
                starts.pop();
            }
        }
        let final_nl = text.ends_with('\n');
        Snap { text, starts, final_nl }
    }

    pub(crate) fn len(&self) -> usize {
        self.starts.len()
    }

    pub(crate) fn line(&self, i: usize) -> &str {
        let Some(&s) = self.starts.get(i) else { return "" };
        let e = match self.starts.get(i + 1) {
            Some(n) => n - 1,
            None if self.final_nl => self.text.len() - 1,
            None => self.text.len(),
        };
        &self.text[s..e]
    }

    fn lines(&self) -> compare::Lines<'_> {
        compare::Lines { lines: (0..self.len()).map(|i| self.line(i)).collect(), final_newline: self.final_nl }
    }
}

/// Wofür ein Ergebnis gilt.
#[derive(Clone, Debug, PartialEq, Eq)]
struct JobKey {
    l: (u64, u64),
    r: (u64, u64),
    ws: bool,
}

/// Größen und Prüfsummen, wenn nicht zeilenweise verglichen wird (Binärdatei, zu groß).
#[derive(Clone, Debug)]
pub(crate) struct BigInfo {
    pub binary: bool,
    pub left: (u64, String),
    pub right: (u64, String),
}

/// Ergebnis eines Vergleichs.
pub(crate) struct DiffOut {
    pub left: Arc<Snap>,
    pub right: Arc<Snap>,
    pub hunks: Vec<Hunk>,
    pub coarse: bool,
    intra: HashMap<(u32, u32), Option<(Vec<Range<usize>>, Vec<Range<usize>>)>>,
    pub big: Option<BigInfo>,
    key: JobKey,
    /// Vorläufig (#391): nach dem Tippen ohne neuen Vergleich fortgeschrieben, der richtige folgt nach der Pause.
    pub provisional: bool,
    /// Breite der längsten Zeile beider Seiten (Tabbreite, Spalten) – einmal gemessen, beim Tippen fortgeschrieben.
    cols: std::sync::OnceLock<(usize, usize)>,
}

impl DiffOut {
    /// Breite der längsten Zeile beider Seiten in Spalten (#389).
    fn max_cols(&self, tab: usize) -> usize {
        if let Some((t, c)) = self.cols.get()
            && *t == tab
        {
            return *c;
        }
        let c = [&self.left, &self.right].iter().map(|s| (0..s.len()).map(|i| str_cols(s.line(i), tab)).max().unwrap_or(0)).max().unwrap_or(0);
        let _ = self.cols.set((tab, c));
        c
    }
}

/// Breite einer Zeile in visuellen Spalten (sehr lange Zeilen: ein Zeichen = eine Spalte).
fn str_cols(line: &str, tab: usize) -> usize {
    if line.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return line.len();
    }
    if line.len() >= super::text::LONG_LINE {
        return line.chars().count();
    }
    let mut v = 0;
    for c in line.chars() {
        v += et::char_width(c, v, tab);
    }
    v
}

/// Visuelle Spalte der Zeichenspalte `col` in `line`.
fn str_vcol(line: &str, col: usize, tab: usize) -> usize {
    let mut v = 0;
    for c in line.chars().take(col) {
        v += et::char_width(c, v, tab);
    }
    v
}

/// Zeichenspalte zur visuellen Spalte `v` in `line` (zur näheren Kante gerundet; hinter dem Ende = Länge).
fn str_col_at(line: &str, v: usize, tab: usize) -> usize {
    let mut x = 0;
    let mut n = 0;
    for (i, c) in line.chars().enumerate() {
        let w = et::char_width(c, x, tab);
        if v < x + w {
            return if w > 1 && v - x > (w - 1) / 2 { i + 1 } else { i };
        }
        x += w;
        n = i + 1;
    }
    n
}

enum SideIn {
    Rope { rope: ropey::Rope, path: Option<PathBuf>, clean: bool, binary: bool },
    Text { text: Arc<String>, raw_len: u64, raw_sha: Option<String>, binary: bool },
}

struct Job {
    rx: mpsc::Receiver<Option<DiffOut>>,
    progress: Arc<compare::Progress>,
    started: Instant,
    key: JobKey,
}

/// Darstellung, Rollen und Fokus eines Vergleichs-Reiters plus die Hintergrund-Arbeit.
pub(crate) struct DiffTab {
    pub left: Side,
    pub right: Side,
    pub layout: DiffLayout,
    pub ignore_ws: bool,
    pub context: usize,
    /// Aufgeklappte Bereiche (Zeilen der rechten Seite).
    pub revealed: Vec<Range<u32>>,
    pub show_all: bool,
    /// Tastatur (Rückgängig, Speichern) gilt der linken Seite.
    pub focus_left: bool,
    /// Aktueller Block (F7).
    pub current: Option<usize>,
    pub scroll_y: f32,
    pub scroll_x: f32,
    /// Griff der waagerechten Leiste wird gezogen (#389): Abstand des Zeigers zum Griffanfang.
    hdrag: Option<f32>,
    /// Leiste und größter Bildlauf im letzten Bild (Tests).
    pub(super) hbar: Option<Rect>,
    pub(super) hmax: f32,
    goto_hunk: Option<usize>,
    job: Option<Job>,
    pub(super) out: Option<Arc<DiffOut>>,
    changed_at: Option<Instant>,
    force: bool,
    cancelled: Option<JobKey>,
    cache: Option<Cache>,
    last_press: Option<(f64, Pos2)>,
    /// Flächen im letzten Bild (Tests, Tutorial): Blockpfeile (Block, nach links?) und Falten.
    pub(super) arrows: Vec<(Rect, usize, bool)>,
    pub(super) folds: Vec<(Rect, Range<u32>)>,
    pub(super) unified_shown: bool,
    /// Cursor in einer bearbeitbaren Seite (#391); `None` = Pfeiltasten rollen wie bisher.
    pub(super) caret: Option<Caret>,
    /// Textflächen beider Seiten und Zeichenbreite im letzten Bild (Tests).
    text_geo: Option<([Rect; 2], f32, f32)>,
}

/// Cursor im Vergleichs-Reiter (#391): eine Auswahl im Dokument einer Seite (mehrere Cursor kommen später, #394).
#[derive(Clone, Debug)]
pub(crate) struct Caret {
    /// Linke Seite (sonst rechte).
    pub left: bool,
    pub sels: Selections,
    /// Ziehen mit der Maus ab dieser Position.
    drag: Option<usize>,
    blink: f64,
    preedit: String,
    /// Nach Tastatur/Bearbeitung in den sichtbaren Bereich rollen.
    reveal: bool,
}

impl Caret {
    fn new(left: bool, doc: &Document, pos: usize, time: f64) -> Caret {
        Caret { left, sels: doc.new_selections(pos), drag: None, blink: time, preedit: String::new(), reveal: false }
    }
}

/// Zeilen und Höhen für die aktuelle Darstellung.
struct Cache {
    key: (usize, bool, usize, bool, u64, Option<usize>, Option<usize>, usize),
    /// Breite der längsten Zeile beider Seiten in Spalten (einmal je Aufbau gemessen, #389).
    max_cols: usize,
    rows: Vec<Row>,
    items: Vec<Item>,
    /// Oberkante jedes Eintrags (Pixel ab dem Anfang) und Gesamthöhe am Ende.
    ys: Vec<f32>,
    /// Bildschirmzeilen je Eintrag (Umbruch).
    spans: Vec<u16>,
}

impl DiffTab {
    pub(crate) fn new(left: Side, right: Side, prefs: &super::EditorPrefs) -> DiffTab {
        DiffTab {
            left,
            right,
            layout: prefs.diff_layout,
            ignore_ws: prefs.diff_ignore_ws,
            context: prefs.diff_context,
            revealed: Vec::new(),
            show_all: false,
            focus_left: false,
            current: None,
            scroll_y: 0.0,
            scroll_x: 0.0,
            hdrag: None,
            hbar: None,
            hmax: 0.0,
            goto_hunk: None,
            job: None,
            out: None,
            changed_at: None,
            force: false,
            cancelled: None,
            cache: None,
            last_press: None,
            arrows: Vec::new(),
            folds: Vec::new(),
            unified_shown: false,
            caret: None,
            text_geo: None,
        }
    }

    /// Kopie für eine neue Teilung (eigener Bildlauf, gleiche Seiten).
    pub(crate) fn duplicate(&self) -> DiffTab {
        let mut d = DiffTab::new(self.left.duplicate(), self.right.duplicate(), &super::EditorPrefs::default());
        d.layout = self.layout;
        d.ignore_ws = self.ignore_ws;
        d.context = self.context;
        d.revealed = self.revealed.clone();
        d.show_all = self.show_all;
        d.scroll_y = self.scroll_y;
        d
    }

    /// Gleiche Seiten (dann wird der offene Reiter gezeigt statt eines zweiten). Die Zwischenablage ist nie gleich.
    pub(crate) fn same_sides(&self, o: &DiffTab) -> bool {
        let same = |a: &Side, b: &Side| match (a, b) {
            (Side::Doc(x), Side::Doc(y)) => x == y,
            (Side::Text(x), Side::Text(y)) => x.source == y.source && x.source != Source::Clipboard,
            _ => false,
        };
        same(&self.left, &o.left) && same(&self.right, &o.right)
    }

    /// Für `editor-state.json` (`None` mit der Zwischenablage: deren Inhalt wird nicht gemerkt).
    pub(crate) fn to_saved(&self, id: u64) -> Option<super::persist::SavedDiff> {
        let side = |s: &Side| -> Option<super::persist::SavedSide> {
            Some(match s {
                Side::Doc(d) => super::persist::SavedSide { doc: Some(*d), ..Default::default() },
                Side::Text(t) => match &t.source {
                    Source::Disk(p) => super::persist::SavedSide { disk: Some(p.to_string_lossy().to_string()), ..Default::default() },
                    Source::Git { path, rev } => super::persist::SavedSide { path: Some(path.to_string_lossy().to_string()), rev: Some(rev.to_saved()), ..Default::default() },
                    Source::Clipboard => return None,
                },
            })
        };
        let layout = match self.layout {
            DiffLayout::Auto => "auto",
            DiffLayout::Split => "split",
            DiffLayout::Unified => "unified",
        };
        Some(super::persist::SavedDiff { id, left: side(&self.left)?, right: side(&self.right)?, layout: layout.into(), ignore_ws: self.ignore_ws, context: self.context as u32, show_all: self.show_all })
    }

    /// Aus `editor-state.json` (Dokumente müssen in `docs` stehen; die festen Stände werden beim ersten Zeigen gelesen).
    pub(crate) fn from_saved(s: &super::persist::SavedDiff, docs: &BTreeMap<u64, DocEntry>) -> Option<DiffTab> {
        let side = |x: &super::persist::SavedSide| -> Option<Side> {
            if let Some(d) = x.doc {
                return docs.contains_key(&d).then_some(Side::Doc(d));
            }
            if let Some(p) = &x.disk {
                return Some(Side::Text(Box::new(StaticSide::new(Source::Disk(PathBuf::from(p))))));
            }
            let (p, r) = (x.path.as_ref()?, x.rev.as_ref()?);
            Some(Side::Text(Box::new(StaticSide::new(Source::Git { path: PathBuf::from(p), rev: Rev::from_saved(r)? }))))
        };
        let (left, right) = (side(&s.left)?, side(&s.right)?);
        if left.doc().is_none() && right.doc().is_none() {
            return None;
        }
        let mut d = DiffTab::new(left, right, &super::EditorPrefs::default());
        d.layout = DiffLayout::from_id(&s.layout).unwrap_or_default();
        d.ignore_ws = s.ignore_ws;
        d.context = (s.context.min(crate::editor_svc::settings::DIFF_CONTEXT_RANGE.1)) as usize;
        d.show_all = s.show_all;
        Some(d)
    }

    /// Dokumente der Seiten für den Reiter: (`doc`, `extra`).
    pub(crate) fn tab_docs(&self) -> Option<(u64, Option<u64>)> {
        match (self.left.doc(), self.right.doc()) {
            (l, Some(r)) => Some((r, l)),
            (Some(l), None) => Some((l, None)),
            (None, None) => None,
        }
    }

    /// „a.json ↔ b.json“.
    pub(crate) fn title(&self, docs: &BTreeMap<u64, DocEntry>, lang: Lang) -> String {
        format!("{} ↔ {}", self.left.label(docs, lang), self.right.label(docs, lang))
    }

    pub(crate) fn dirty(&self, docs: &BTreeMap<u64, DocEntry>) -> bool {
        [self.left.doc(), self.right.doc()].into_iter().flatten().any(|d| docs.get(&d).is_some_and(|e| e.dirty()))
    }

    /// Dokument, dem Tastatur-Befehle (Speichern, Rückgängig) gelten: die fokussierte Seite, sonst die andere.
    pub(crate) fn focused_doc(&self) -> Option<u64> {
        let (a, b) = if self.focus_left { (&self.left, &self.right) } else { (&self.right, &self.left) };
        a.doc().or(b.doc())
    }

    pub(crate) fn swap(&mut self) {
        std::mem::swap(&mut self.left, &mut self.right);
        self.focus_left = !self.focus_left;
        if let Some(c) = self.caret.as_mut() {
            c.left = !c.left;
        }
        self.out = None;
        self.cache = None;
        self.revealed.clear();
        self.current = None;
    }

    /// Den festen Stand der linken Seite ersetzen (z. B. andere Revision).
    pub(crate) fn set_left(&mut self, side: Side) {
        self.left = side;
        if self.caret.as_ref().is_some_and(|c| c.left) {
            self.caret = None;
        }
        self.out = None;
        self.cache = None;
        self.current = None;
    }

    /// Ergebnis passt zum aktuellen Stand beider Seiten, nichts läuft (Tests).
    #[cfg(test)]
    pub(super) fn settled(&self, docs: &BTreeMap<u64, DocEntry>) -> bool {
        self.job.is_none() && self.key(docs).is_some_and(|k| self.out.as_ref().is_some_and(|o| o.key == k && !o.provisional)) && [&self.left, &self.right].iter().all(|s| !matches!(s, Side::Text(t) if t.load.is_some()))
    }

    pub(crate) fn hunk_count(&self) -> usize {
        self.out.as_ref().map(|o| o.hunks.len()).unwrap_or(0)
    }

    /// Nächster bzw. vorheriger Block (F7 / Umschalt+F7), ringsum.
    pub(crate) fn step(&mut self, forward: bool) {
        let n = self.hunk_count();
        if n == 0 {
            return;
        }
        let next = match self.current {
            None => {
                if forward {
                    0
                } else {
                    n - 1
                }
            }
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
        };
        self.current = Some(next);
        self.goto_hunk = Some(next);
    }

    /// Neu vergleichen, sobald möglich (nach einem Blockpfeil sofort).
    pub(crate) fn recompute_now(&mut self) {
        self.force = true;
        self.cancelled = None;
    }

    /// Eine Seite ließ sich nicht lesen (Datei fehlt, keine Rechte): Text der Meldung.
    pub(crate) fn side_error(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<String> {
        [&self.left, &self.right].iter().find_map(|s| match s {
            Side::Doc(d) => docs.get(d).and_then(|e| e.load_error.clone()).map(|e| format!("{}: {e}", e_title(docs, *d))),
            Side::Text(_) => None,
        })
    }

    fn key(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<JobKey> {
        Some(JobKey { l: self.left.version(docs)?, r: self.right.version(docs)?, ws: self.ignore_ws })
    }

    fn side_in(side: &Side, docs: &BTreeMap<u64, DocEntry>) -> Option<SideIn> {
        Some(match side {
            Side::Doc(d) => {
                let e = docs.get(d)?;
                let doc = e.doc.as_ref()?;
                SideIn::Rope { rope: doc.rope().clone(), path: e.path.clone(), clean: !e.dirty(), binary: doc.load_info().looks_binary }
            }
            Side::Text(s) => SideIn::Text { text: s.text.clone(), raw_len: s.raw_len, raw_sha: s.raw_sha.clone(), binary: s.binary },
        })
    }

    /// Je Bild: feste Stände lesen, Ergebnis abholen, bei Bedarf neu vergleichen. `generation` wächst nach Speichern
    /// und Fensterfokus. Liefert, ob noch etwas läuft (dann neu zeichnen).
    pub(crate) fn tick(&mut self, docs: &mut BTreeMap<u64, DocEntry>, prefs: &super::EditorPrefs, generation: u64, now: Instant) -> bool {
        let mut busy = false;
        for d in [self.left.doc(), self.right.doc()].into_iter().flatten() {
            if let Some(e) = docs.get_mut(&d) {
                e.ensure_loaded(prefs);
                busy |= e.is_loading();
            }
        }
        let enc = [self.left.doc(), self.right.doc()].into_iter().flatten().find_map(|d| docs.get(&d).and_then(|e| e.doc.as_ref()).map(|x| x.encoding()));
        for side in [&mut self.left, &mut self.right] {
            if let Side::Text(s) = side {
                busy |= s.tick(now, generation, prefs.large_file_bytes, enc);
            }
        }
        if let Some(j) = &self.job {
            match j.rx.try_recv() {
                Ok(Some(out)) => {
                    let n = out.hunks.len();
                    self.out = Some(Arc::new(out));
                    self.cache = None;
                    self.job = None;
                    if self.current.is_some_and(|c| c >= n) {
                        self.current = n.checked_sub(1);
                    }
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => self.job = None,
                Err(mpsc::TryRecvError::Empty) => busy = true,
            }
        }
        // Eine Seite ist nicht lesbar: nichts zu tun (die Meldung steht im Reiter), nicht ewig neu zeichnen.
        if self.side_error(docs).is_some() {
            return busy;
        }
        self.live_update(docs, now);
        let Some(key) = self.key(docs) else { return true };
        let stale = self.out.as_ref().is_none_or(|o| o.key != key || o.provisional);
        let running_same = self.job.as_ref().is_some_and(|j| j.key == key);
        if stale && !running_same && self.cancelled.as_ref() != Some(&key) {
            let since = *self.changed_at.get_or_insert(now);
            if self.out.is_none() || self.force || now.duration_since(since) >= LIVE_DELAY {
                if self.job.is_none() {
                    self.spawn(docs, prefs, key);
                    self.changed_at = None;
                    self.force = false;
                }
            }
            busy = true;
        }
        busy
    }

    /// Geänderte Dokumente sofort zeigen (#391), ohne neu zu vergleichen: der Text der Seite kommt aus dem Dokument, die
    /// Blöcke werden über [`compare::edit_hunks`] verschoben bzw. mit der Änderung zusammengelegt (ein gültiger, nicht
    /// unbedingt kleinster Vergleich – Blockpfeile bleiben richtig). Der richtige Vergleich folgt [`LIVE_DELAY`] nach der
    /// letzten Änderung; ein laufender für einen älteren Stand wird verworfen. Liefert, ob sich etwas geändert hat.
    pub(super) fn live_update(&mut self, docs: &BTreeMap<u64, DocEntry>, now: Instant) -> bool {
        let Some(out) = self.out.clone() else { return false };
        if out.big.is_some() {
            return false;
        }
        let mut w: Option<DiffOut> = None;
        for left in [true, false] {
            let Some(d) = (if left { self.left.doc() } else { self.right.doc() }) else { continue };
            let Some(doc) = docs.get(&d).and_then(|e| e.doc.as_ref()) else { continue };
            let cur = w.as_ref().unwrap_or(&out);
            let seen = if left { cur.key.l } else { cur.key.r };
            if seen.0 != d || seen.1 == doc.revision() {
                continue;
            }
            let old = if left { cur.left.clone() } else { cur.right.clone() };
            let new = Arc::new(Snap::new(Arc::new(doc.rope().to_string())));
            let (edit, n) = compare::changed_lines_by(old.len(), new.len(), |i, j| old.line(i) == new.line(j));
            let (hunks, merged) = compare::edit_hunks(&cur.hunks, !left, edit.clone(), n);
            let delta = n as isize - edit.len() as isize;
            // Änderungen in der Zeile: in den zusammengelegten Blöcken verworfen (der Vergleich folgt), danach verschoben.
            let side_of = |h: &Hunk| if left { h.old.clone() } else { h.new.clone() };
            let lo = cur.hunks[merged.clone()].first().map_or(edit.start, |h| side_of(h).start.min(edit.start));
            let hi = cur.hunks[merged.clone()].last().map_or(edit.end, |h| side_of(h).end.max(edit.end));
            let mut intra = HashMap::with_capacity(cur.intra.len());
            for (&(a, b), v) in &cur.intra {
                let s = if left { a } else { b } as usize;
                if (lo..hi).contains(&s) {
                    continue;
                }
                let moved = if s >= hi { (s as isize + delta) as u32 } else { s as u32 };
                intra.insert(if left { (moved, b) } else { (a, moved) }, v.clone());
            }
            let tab = doc.tab_width().max(1);
            let cols = std::sync::OnceLock::new();
            if let Some((t, c)) = cur.cols.get()
                && *t == tab
            {
                let fresh = (edit.start..edit.start + n).map(|i| str_cols(new.line(i), tab)).max().unwrap_or(0);
                let _ = cols.set((tab, (*c).max(fresh)));
            }
            let mut key = cur.key.clone();
            if left {
                key.l = (d, doc.revision());
            } else {
                key.r = (d, doc.revision());
            }
            let pushed = hunks.len() + merged.len() > cur.hunks.len();
            if let Some(c) = self.current {
                self.current = if c < merged.start {
                    Some(c)
                } else if c < merged.end {
                    pushed.then_some(merged.start)
                } else {
                    Some(c - merged.len() + pushed as usize)
                };
            }
            if !left {
                // Aufgeklappte Bereiche sind Zeilen der rechten Seite.
                let map = |p: u32| -> u32 {
                    let p = p as usize;
                    (if p >= edit.end {
                        (p as isize + delta) as usize
                    } else if p > edit.start {
                        (edit.start + n).min(p)
                    } else {
                        p
                    }) as u32
                };
                for r in self.revealed.iter_mut() {
                    *r = map(r.start)..map(r.end);
                }
                self.revealed.retain(|r| !r.is_empty());
            }
            let (l, r) = if left { (new, cur.right.clone()) } else { (cur.left.clone(), new) };
            w = Some(DiffOut { left: l, right: r, hunks, coarse: cur.coarse, intra, big: None, key, provisional: true, cols });
        }
        let Some(w) = w else { return false };
        if let Some(j) = self.job.take() {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
        self.out = Some(Arc::new(w));
        self.cache = None;
        self.changed_at = Some(now);
        true
    }

    /// Bearbeiten in einem eingeklappten Bereich klappt ihn auf (#391): Zeile `line` der Seite `left`.
    fn unfold_line(&mut self, left: bool, line: usize) {
        let Some(c) = &self.cache else { return };
        let side = |r: &Row| if left { r.left } else { r.right };
        for it in &c.items {
            if let Item::Fold { rows } = it {
                let (Some(a), Some(b)) = (side(&c.rows[rows.start]), side(&c.rows[rows.end - 1])) else { continue };
                if (a as usize..=b as usize).contains(&line) {
                    let (Some(ra), Some(rb)) = (c.rows[rows.start].right, c.rows[rows.end - 1].right) else { return };
                    self.revealed.push(ra..rb + 1);
                    self.cache = None;
                    return;
                }
            }
        }
    }

    /// Bearbeitbare Seite: ein geladenes Dokument.
    fn editable(&self, left: bool, docs: &BTreeMap<u64, DocEntry>) -> bool {
        let side = if left { &self.left } else { &self.right };
        side.doc().is_some_and(|d| docs.get(&d).is_some_and(|e| e.doc.is_some()))
    }

    /// Seite, in die untereinander getippt wird: die rechte, wenn sie bearbeitbar ist, sonst die linke.
    fn unified_side(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<bool> {
        if self.editable(false, docs) {
            Some(false)
        } else if self.editable(true, docs) {
            Some(true)
        } else {
            None
        }
    }

    /// Bildschirmpunkt in Zeile `line`, Zeichenspalte `col` einer Seite im letzten Bild (Tests; ohne Umbruch, nur ASCII).
    #[cfg(test)]
    pub(super) fn point_at(&self, left: bool, line: usize, col: usize) -> Option<Pos2> {
        let (texts, body_top, cw) = self.text_geo?;
        let c = self.cache.as_ref()?;
        let side = |r: &Row| if left { r.left } else { r.right };
        let i = c.items.iter().position(|it| matches!(it, Item::Row(r) if side(&c.rows[*r]) == Some(line as u32)))?;
        let t = texts[if left || self.unified_shown { 0 } else { 1 }];
        Some(Pos2::new(t.left() + (col as f32 + 0.3) * cw - self.scroll_x, body_top + c.ys[i] - self.scroll_y + 3.0))
    }

    /// Bildschirmpunkt der ersten Füllzeile einer Seite (Tests).
    #[cfg(test)]
    pub(super) fn filler_point(&self, left: bool) -> Option<Pos2> {
        let (texts, body_top, cw) = self.text_geo?;
        let c = self.cache.as_ref()?;
        let i = c.items.iter().position(|it| matches!(it, Item::Row(r) if (if left { c.rows[*r].left } else { c.rows[*r].right }).is_none()))?;
        let t = texts[if left { 0 } else { 1 }];
        Some(Pos2::new(t.left() + cw, body_top + c.ys[i] - self.scroll_y + 3.0))
    }

    /// Cursor (Seite links?, Zeile, Spalte; ab 0) – Tests.
    #[cfg(test)]
    pub(super) fn caret_linecol(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<(bool, usize, usize)> {
        let c = self.caret.as_ref()?;
        let d = if c.left { self.left.doc() } else { self.right.doc() }?;
        let doc = docs.get(&d)?.doc.as_ref()?;
        let lc = et::pos_to_linecol(doc.rope(), c.sels.primary().head.min(doc.len_chars()));
        Some((c.left, lc.line, lc.col))
    }

    fn spawn(&mut self, docs: &BTreeMap<u64, DocEntry>, prefs: &super::EditorPrefs, key: JobKey) {
        let (Some(l), Some(r)) = (Self::side_in(&self.left, docs), Self::side_in(&self.right, docs)) else { return };
        let progress = Arc::new(compare::Progress::default());
        let p2 = progress.clone();
        let (tx, rx) = mpsc::channel();
        let (k2, max, ws) = (key.clone(), prefs.large_file_bytes, self.ignore_ws);
        std::thread::spawn(move || {
            let _ = tx.send(run_job(l, r, ws, max, &p2, k2));
        });
        self.job = Some(Job { rx, progress, started: Instant::now(), key });
    }

    /// Laufenden Vergleich abbrechen (bis sich etwas ändert oder neu verglichen wird).
    pub(crate) fn cancel(&mut self) {
        if let Some(j) = self.job.take() {
            j.progress.cancel.store(true, Ordering::Relaxed);
            self.cancelled = Some(j.key);
        }
    }

    pub(crate) fn computing(&self) -> Option<(u32, Duration)> {
        self.job.as_ref().map(|j| (j.progress.percent.load(Ordering::Relaxed), j.started.elapsed()))
    }

    /// Block `k` auf die andere Seite übernehmen: `to_left` = rechts → links. Ein Rückgängig-Schritt im Zieldokument.
    pub(crate) fn apply(&mut self, k: usize, to_left: bool, docs: &mut BTreeMap<u64, DocEntry>, lang: Lang) -> Result<(), String> {
        let out = self.out.clone().ok_or_else(|| lang.t("Der Vergleich wird noch berechnet.", "The comparison is still being computed.").to_string())?;
        if self.key(docs).as_ref() != Some(&out.key) {
            self.recompute_now();
            return Err(lang.t("Der Vergleich wird gerade neu berechnet – gleich noch einmal.", "The comparison is being recomputed – try again in a moment.").to_string());
        }
        let h = out.hunks.get(k).ok_or("?")?.clone();
        let (target, src_snap, src_range, dst_snap, dst_range) = if to_left { (&self.left, &out.right, h.new.clone(), &out.left, h.old.clone()) } else { (&self.right, &out.left, h.old.clone(), &out.right, h.new.clone()) };
        let Some(id) = target.doc() else { return Err(lang.t("Diese Seite ist schreibgeschützt.", "This side is read-only.").to_string()) };
        let src: Vec<&str> = src_range.map(|i| src_snap.line(i)).collect();
        let edit = compare::replace_lines_edit(&dst_snap.text, dst_range, &src);
        let e = docs.get_mut(&id).ok_or("?")?;
        e.apply_user_edit(edit);
        self.recompute_now();
        Ok(())
    }

    /// Unified Diff (für *Diff kopieren* / *Als Patch speichern*): `git` mit Kopf `diff --git a/… b/…`. Immer ohne
    /// *Leerraum ignorieren* (ein Patch muss genau passen).
    pub(crate) fn patch(&self, docs: &BTreeMap<u64, DocEntry>, git: bool) -> Option<String> {
        let out = self.out.as_ref()?;
        if out.big.is_some() {
            return None;
        }
        let (l, r) = (out.left.lines(), out.right.lines());
        let hunks = if self.ignore_ws { compare::compare(&l.lines, &r.lines, &compare::Options::default(), None).hunks } else { out.hunks.clone() };
        let (a, b) = self.patch_names(docs);
        Some(compare::unified(&l, &r, &hunks, self.context, &a, &b, git))
    }

    /// Unified Diff nur des aktuellen Blocks.
    pub(crate) fn block_text(&self) -> Option<String> {
        let out = self.out.as_ref()?;
        let h = out.hunks.get(self.current?)?;
        let (l, r) = (out.left.lines(), out.right.lines());
        Some(compare::hunk_block(&l, &r, std::slice::from_ref(h), self.context))
    }

    /// Namen im Patch: derselbe Pfad im Repository → `a/<pfad>` `b/<pfad>`, sonst die Dateinamen.
    fn patch_names(&self, docs: &BTreeMap<u64, DocEntry>) -> (String, String) {
        let rel = |side: &Side| -> Option<String> {
            if let Side::Text(s) = side
                && let Some(Some(r)) = &s.repo
            {
                return Some(r.rel.clone());
            }
            None
        };
        let lp = self.left.path(docs);
        let rp = self.right.path(docs);
        if lp.is_some() && lp == rp
            && let Some(rel) = rel(&self.left).or_else(|| rel(&self.right))
        {
            return (format!("a/{rel}"), format!("b/{rel}"));
        }
        let name = |p: Option<PathBuf>, s: &Side| p.map(|p| file_name(&p)).unwrap_or_else(|| s.label(docs, Lang::En).replace(' ', "_"));
        (format!("a/{}", name(lp, &self.left)), format!("b/{}", name(rp, &self.right)))
    }

    /// Vorschlag für den Dateinamen des Patches.
    pub(crate) fn patch_file_name(&self, docs: &BTreeMap<u64, DocEntry>) -> String {
        let base = self.right.path(docs).or_else(|| self.left.path(docs)).map(|p| file_name(&p)).unwrap_or_else(|| "changes".into());
        format!("{base}.patch")
    }

    pub(crate) fn folder(&self, docs: &BTreeMap<u64, DocEntry>) -> Option<PathBuf> {
        self.right.path(docs).or_else(|| self.left.path(docs)).and_then(|p| p.parent().map(Path::to_path_buf))
    }

    /// Hinweise über dem Vergleich: Zeilenenden, Kodierung, Zeilenende am Schluss, gröberes Ergebnis, Lesefehler.
    pub(crate) fn notes(&self, docs: &BTreeMap<u64, DocEntry>, lang: Lang) -> Vec<String> {
        let mut v = Vec::new();
        for (side, which) in [(&self.left, lang.t("links", "left")), (&self.right, lang.t("rechts", "right"))] {
            if let Some(n) = side.note() {
                v.push(format!("{which}: {n}"));
            }
        }
        if let (Some(a), Some(b)) = (self.left.facts(docs), self.right.facts(docs)) {
            let yes_no = |x: bool| if x { lang.t("ja", "yes") } else { lang.t("nein", "no") };
            let parts: Vec<String> = compare::fact_diffs(&a, &b)
                .into_iter()
                .map(|f| match f {
                    compare::FactDiff::Eol(x, y) => format!("{} {x} ↔ {y}", lang.t("Zeilenenden:", "Line endings:")),
                    compare::FactDiff::Encoding(x, y) => format!("{} {x} ↔ {y}", lang.t("Kodierung:", "Encoding:")),
                    compare::FactDiff::FinalNewline(x, y) => format!("{} {} ↔ {}", lang.t("Zeilenende am Schluss:", "Final newline:"), yes_no(x), yes_no(y)),
                })
                .collect();
            if !parts.is_empty() {
                v.push(parts.join(" · "));
            }
        }
        if self.out.as_ref().is_some_and(|o| o.coarse) {
            v.push(lang.t("Großer Unterschied: wegen der Zeitgrenze gröber gezeigt.", "Large difference: shown coarser because of the time limit.").to_string());
        }
        v
    }

    /// Kurzfassung „+12 −4 · 3 Änderungen“.
    pub(crate) fn summary(&self, lang: Lang) -> String {
        let Some(o) = &self.out else { return lang.t("wird verglichen …", "comparing …").to_string() };
        if o.big.is_some() {
            return String::new();
        }
        if o.hunks.is_empty() {
            return if self.ignore_ws { lang.t("gleich (Leerraum ignoriert)", "identical (whitespace ignored)") } else { lang.t("gleich", "identical") }.to_string();
        }
        let (add, del) = o.hunks.iter().fold((0, 0), |(a, d), h| (a + h.new.len(), d + h.old.len()));
        let cur = self.current.map(|c| format!("{}/", c + 1)).unwrap_or_default();
        format!("+{add} −{del} · {cur}{} {}", o.hunks.len(), if o.hunks.len() == 1 { lang.t("Änderung", "change") } else { lang.t("Änderungen", "changes") })
    }

    // ---- Darstellung

    fn ensure_cache(&mut self, unified: bool, wrap: Option<(usize, usize)>, tab: usize) {
        let Some(out) = self.out.clone() else {
            self.cache = None;
            return;
        };
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.revealed.hash(&mut h);
        let key = (Arc::as_ptr(&out) as usize, unified, self.context, self.show_all, h.finish(), wrap.map(|w| w.0), wrap.map(|w| w.1), tab);
        if self.cache.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let rows = compare::rows(&out.hunks, out.left.len(), out.right.len(), unified);
        let items = compare::collapse(&rows, self.context, &self.revealed, self.show_all);
        let mut spans = Vec::with_capacity(items.len());
        for it in &items {
            let n = match (it, wrap) {
                (Item::Row(i), Some((lc, rc))) => {
                    let r = rows[*i];
                    let a = r.left.map(|l| wrap_starts(out.left.line(l as usize), lc, tab).len()).unwrap_or(1);
                    let b = r.right.map(|x| wrap_starts(out.right.line(x as usize), if unified { lc } else { rc }, tab).len()).unwrap_or(1);
                    a.max(b).min(u16::MAX as usize) as u16
                }
                _ => 1,
            };
            spans.push(n);
        }
        let max_cols = out.max_cols(tab);
        self.cache = Some(Cache { key, max_cols, rows, items, ys: Vec::new(), spans });
    }
}

/// Vergleich im Hilfsthread.
fn run_job(l: SideIn, r: SideIn, ws: bool, max: u64, progress: &compare::Progress, key: JobKey) -> Option<DiffOut> {
    let text_of = |s: &SideIn| -> (Arc<String>, bool, u64) {
        match s {
            SideIn::Rope { rope, binary, .. } => {
                let t = rope.to_string();
                let n = t.len() as u64;
                (Arc::new(t), *binary, n)
            }
            SideIn::Text { text, raw_len, binary, .. } => (text.clone(), *binary, *raw_len),
        }
    };
    let (lt, lb, ln) = text_of(&l);
    let (rt, rb, rn) = text_of(&r);
    if progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    if lb || rb || ln > max || rn > max {
        let sha = |s: &SideIn, t: &str| -> (u64, String) {
            match s {
                SideIn::Rope { path: Some(p), clean: true, .. } => sha_file(p).unwrap_or((t.len() as u64, sha_hex(t.as_bytes()))),
                SideIn::Rope { .. } => (t.len() as u64, sha_hex(t.as_bytes())),
                SideIn::Text { raw_len, raw_sha, .. } => (*raw_len, raw_sha.clone().unwrap_or_default()),
            }
        };
        let big = BigInfo { binary: lb || rb, left: sha(&l, &lt), right: sha(&r, &rt) };
        return Some(DiffOut { left: Arc::new(Snap::new(Arc::new(String::new()))), right: Arc::new(Snap::new(Arc::new(String::new()))), hunks: Vec::new(), coarse: false, intra: HashMap::new(), big: Some(big), key, provisional: false, cols: Default::default() });
    }
    let (ls, rs) = (Snap::new(lt), Snap::new(rt));
    let (ll, rl) = (ls.lines(), rs.lines());
    let big_input = ln + rn > 4 * 1024 * 1024;
    let opts = compare::Options { ignore_whitespace: ws, timeout: if big_input { compare::BACKGROUND_TIMEOUT } else { compare::LIVE_TIMEOUT } };
    let o = compare::compare(&ll.lines, &rl.lines, &opts, Some(progress));
    if o.cancelled {
        return None;
    }
    let mut intra = HashMap::new();
    let mut budget = INTRA_BUDGET;
    'outer: for h in &o.hunks {
        for k in 0..h.old.len().min(h.new.len()) {
            if budget == 0 {
                break 'outer;
            }
            budget -= 1;
            let (a, b) = (h.old.start + k, h.new.start + k);
            intra.insert((a as u32, b as u32), compare::intraline(ll.lines[a], rl.lines[b], ws));
        }
    }
    drop((ll, rl));
    Some(DiffOut { left: Arc::new(ls), right: Arc::new(rs), hunks: o.hunks, coarse: o.coarse, intra, big: None, key, provisional: false, cols: Default::default() })
}

/// Anfänge (Zeichenspalten) der Bildschirmzeilen bei Umbruch nach `cols` Spalten (bevorzugt nach Leerzeichen).
pub(super) fn wrap_starts(line: &str, cols: usize, tab: usize) -> Vec<usize> {
    if cols < 8 || line.len() <= cols / 2 {
        return vec![0];
    }
    let mut rows = vec![0];
    let (mut v, mut row_v) = (0usize, 0usize);
    let mut last_space: Option<(usize, usize)> = None;
    for (i, c) in line.chars().enumerate() {
        let w = et::char_width(c, v, tab);
        if v + w - row_v > cols && i > *rows.last().unwrap_or(&0) {
            match last_space.filter(|(col, _)| *col > *rows.last().unwrap_or(&0)) {
                Some((col, sv)) => {
                    rows.push(col);
                    row_v = sv;
                }
                None => {
                    rows.push(i);
                    row_v = v;
                }
            }
            last_space = None;
        }
        v += w;
        if c == ' ' || c == '\t' {
            last_space = Some((i + 1, v));
        }
    }
    rows
}

// ------------------------------------------------------------------------------------------------ Zeichnen

/// Gemeinsame Werte für das Zeichnen.
pub(super) struct DiffCx<'a> {
    pub id: Id,
    pub font: FontId,
    pub theme: &'a crate::theme::Theme,
    pub prefs: &'a super::EditorPrefs,
    pub lang: Lang,
    pub wrap: bool,
    pub request_focus: bool,
    pub diff: u64,
}

/// Ergebnis eines Bildes.
#[derive(Default)]
pub(super) struct DiffShowOut {
    pub focused: bool,
    pub clicked: bool,
}

/// Geometrie einer Seite (nebeneinander) bzw. des einen Textfelds (untereinander).
#[derive(Clone, Copy)]
struct Col {
    num: Rect,
    text: Rect,
}

/// Vergleichs-Reiter in `rect` zeichnen und bedienen.
pub(super) fn show(ui: &mut Ui, rect: Rect, dt: &mut DiffTab, docs: &mut BTreeMap<u64, DocEntry>, generation: u64, cx: &DiffCx, cmds: &mut Vec<Cmd>) -> DiffShowOut {
    let mut out = DiffShowOut::default();
    let lang = cx.lang;
    let u = style::ui();
    let now = Instant::now();
    if dt.tick(docs, cx.prefs, generation, now) {
        ui.ctx().request_repaint_after(Duration::from_millis(60));
    }
    // ---- Werkzeugleiste
    let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), TOOLBAR_H));
    toolbar(ui, bar, dt, docs, cx, cmds);
    let mut body = Rect::from_min_max(Pos2::new(rect.left(), bar.bottom()), rect.max);
    // ---- Hinweise
    let mut notes = dt.notes(docs, lang);
    let conflict: Vec<u64> = [dt.left.doc(), dt.right.doc()].into_iter().flatten().filter(|d| docs.get(d).is_some_and(|e| e.disk_conflict)).collect();
    if !conflict.is_empty() {
        notes.insert(0, lang.t("Die Datei wurde auf der Platte geändert, hier gibt es ungespeicherte Änderungen.", "The file changed on disk and has unsaved changes here.").to_string());
    }
    for (i, n) in notes.iter().enumerate() {
        let r = Rect::from_min_size(body.min, Vec2::new(body.width(), INFO_H));
        ui.painter().rect_filled(r, 0.0, u.info.gamma_multiply(0.12));
        ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0_f32, u.border));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(Vec2::new(10.0, 2.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if i == 0
            && let Some(&d) = conflict.first()
        {
            if child.small_button(lang.t("Von der Platte laden", "Load from disk")).clicked() {
                cmds.push(Cmd::Reload(d));
            }
            if child.small_button(lang.t("Meine behalten", "Keep mine")).clicked() {
                cmds.push(Cmd::KeepMine(d));
            }
        }
        child.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(n).small()).truncate());
        });
        body.min.y += INFO_H;
    }
    if body.height() < 20.0 {
        return out;
    }
    // ---- Eingaben
    let resp = ui.interact(body, cx.id, Sense::click_and_drag());
    if cx.request_focus {
        resp.request_focus();
    }
    if resp.clicked() || resp.drag_started() || resp.double_clicked() {
        resp.request_focus();
        out.clicked = true;
    }
    out.focused = resp.has_focus();
    if out.focused {
        // Mit Cursor rückt Tab ein (#391), sonst wandert der Fokus weiter.
        let tab = dt.caret.is_some();
        ui.memory_mut(|m| m.set_focus_lock_filter(cx.id, egui::EventFilter { tab, horizontal_arrows: true, vertical_arrows: true, escape: false }));
    }
    // ---- Fortschritt
    if let Some((pct, age)) = dt.computing()
        && age >= SHOW_PROGRESS_AFTER
    {
        let r = Rect::from_min_size(body.min + Vec2::new(10.0, 6.0), Vec2::new((body.width() - 20.0).max(60.0), 22.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.add(egui::ProgressBar::new(pct as f32 / 100.0).desired_width((r.width() - 120.0).max(40.0)).text(format!("{} {pct} %", lang.t("Vergleiche …", "Comparing …"))));
        if child.button(lang.t("Abbrechen", "Cancel")).clicked() {
            dt.cancel();
        }
        body.min.y += 30.0;
    }
    if let Some(e) = dt.side_error(docs) {
        ui.painter().text(body.center(), Align2::CENTER_CENTER, format!("{} {e}", lang.t("Nicht lesbar –", "Cannot be read –")), FontId::proportional(13.0), style::ui().error);
        return out;
    }
    // ---- Cursor und Tippen (#391): vor dem Aufbau der Zeilen, damit dieses Bild schon den neuen Text zeigt
    let (row_h, cw) = ui.fonts_mut(|f| (f.row_height(&cx.font), f.glyph_width(&cx.font, 'M')));
    let cw = cw.max(1.0);
    let unified = match dt.layout {
        DiffLayout::Unified => true,
        DiffLayout::Split => false,
        DiffLayout::Auto => body.width() < AUTO_UNIFIED_W,
    };
    sync_caret(dt, docs);
    if out.focused && dt.out.as_ref().is_some_and(|o| o.big.is_none()) {
        let page = ((body.height() / row_h).floor() as usize).saturating_sub(1).max(1);
        if edit_keys(ui, dt, docs, page, row_h, unified, lang, cmds) {
            dt.live_update(docs, now);
        }
    }
    let Some(o) = dt.out.clone() else {
        let msg = if dt.cancelled.is_some() { lang.t("Abgebrochen.", "Cancelled.") } else { lang.t("wird verglichen …", "comparing …") };
        ui.painter().text(body.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), style::dim(ui));
        if dt.cancelled.is_some() {
            let b = Rect::from_center_size(body.center() + Vec2::new(0.0, 28.0), Vec2::new(160.0, 24.0));
            if ui.put(b, egui::Button::new(lang.t("Neu vergleichen", "Compare again"))).clicked() {
                dt.recompute_now();
            }
        }
        return out;
    };
    if let Some(big) = &o.big {
        big_message(ui, body, big, dt, docs, lang);
        return out;
    }
    // ---- Geometrie
    dt.unified_shown = unified;
    let tab = [dt.left.doc(), dt.right.doc()].into_iter().flatten().find_map(|d| docs.get(&d).and_then(|e| e.doc.as_ref()).map(|x| x.tab_width())).unwrap_or(4).max(1);
    let digits = o.left.len().max(o.right.len()).max(1).to_string().len().max(3) as f32;
    let num_w = (digits + 1.0) * cw + 6.0;
    let mut inner = Rect::from_min_max(body.min, Pos2::new(body.right() - SCROLLBAR_W, body.bottom()));
    let (mut cols, mut center) = if unified {
        let num = Rect::from_min_max(Pos2::new(inner.left() + ACTION_W, inner.top()), Pos2::new(inner.left() + ACTION_W + num_w * 2.0, inner.bottom()));
        let text = Rect::from_min_max(Pos2::new(num.right() + 2.0 * cw, inner.top()), inner.max);
        ([Col { num, text }, Col { num, text }], Rect::from_min_max(inner.min, Pos2::new(inner.left() + ACTION_W, inner.bottom())))
    } else {
        let half = ((inner.width() - CENTER_W) / 2.0).max(40.0);
        let lnum = Rect::from_min_size(inner.min, Vec2::new(num_w, inner.height()));
        let ltext = Rect::from_min_max(Pos2::new(lnum.right() + 4.0, inner.top()), Pos2::new(inner.left() + half, inner.bottom()));
        let center = Rect::from_min_size(Pos2::new(ltext.right(), inner.top()), Vec2::new(CENTER_W, inner.height()));
        let rnum = Rect::from_min_size(Pos2::new(center.right(), inner.top()), Vec2::new(num_w, inner.height()));
        let rtext = Rect::from_min_max(Pos2::new(rnum.right() + 4.0, inner.top()), inner.max);
        ([Col { num: lnum, text: ltext }, Col { num: rnum, text: rtext }], center)
    };
    let wrap = cx.wrap.then(|| (((cols[0].text.width() / cw).floor() as usize).max(8), ((cols[1].text.width() / cw).floor() as usize).max(8)));
    dt.ensure_cache(unified, wrap, tab);
    // ---- Waagerechte Leiste (#389): eine für beide Seiten und die Einzelansicht, unter den Textflächen
    let mut hbar: Option<Rect> = None;
    dt.hmax = 0.0;
    let mut hview = cols[0].text.width();
    if wrap.is_none() {
        let content = super::hscroll::content_width(dt.cache.as_ref().map_or(0, |c| c.max_cols), cw);
        if !unified {
            hview = hview.min(cols[1].text.width());
        }
        let mut probe = Rect::from_min_size(cols[0].text.min, Vec2::new(hview, body.height()));
        let right = if unified { cols[0].text.right() } else { cols[1].text.right() };
        hbar = super::hscroll::layout(&mut probe, cols[0].text.left(), right, body.bottom(), content);
        dt.hmax = super::hscroll::max_scroll(content, hview);
        dt.scroll_x = dt.scroll_x.clamp(0.0, dt.hmax);
        if hbar.is_some() {
            body.max.y -= super::hscroll::BAR_H;
            inner.max.y -= super::hscroll::BAR_H;
            center.max.y -= super::hscroll::BAR_H;
            for c in cols.iter_mut() {
                c.num.max.y -= super::hscroll::BAR_H;
                c.text.max.y -= super::hscroll::BAR_H;
            }
        }
    }
    let fold_h = row_h + 8.0;
    let (total, n_items) = {
        let c = dt.cache.as_mut().expect("Cache");
        if c.ys.len() != c.items.len() + 1 {
            let mut y = 0.0;
            c.ys.clear();
            for (i, it) in c.items.iter().enumerate() {
                c.ys.push(y);
                y += match it {
                    Item::Row(_) => c.spans[i] as f32 * row_h,
                    Item::Fold { .. } => fold_h,
                };
            }
            c.ys.push(y);
        }
        (*c.ys.last().unwrap_or(&0.0), c.items.len())
    };
    // ---- Tastatur und Rad
    if out.focused {
        keyboard(ui, dt, docs, body, row_h, total, lang, cmds);
    }
    let scroll_x = if wrap.is_some() { 0.0 } else { dt.scroll_x };
    let geo = CaretGeo { body, cols, center, unified, wrap, tab, row_h, cw, scroll_x, hview };
    dt.text_geo = Some(([cols[0].text, cols[1].text], body.top(), cw));
    caret_mouse(ui, &resp, dt, docs, &geo);
    if ui.rect_contains_pointer(body) {
        let (dy, dx, shift) = ui.input(|i| (i.smooth_scroll_delta.y, i.smooth_scroll_delta.x, i.modifiers.shift));
        let (dy, dx) = if shift && dx == 0.0 { (0.0, dy) } else { (dy, dx) };
        dt.scroll_y -= dy;
        if wrap.is_none() {
            let content = super::hscroll::content_width(dt.cache.as_ref().map_or(0, |c| c.max_cols), cw);
            dt.scroll_x = (dt.scroll_x - dx).clamp(0.0, super::hscroll::max_scroll(content, hview));
        }
    }
    // Zum gewählten Block rollen (F7)
    if let Some(k) = dt.goto_hunk.take()
        && let Some(c) = &dt.cache
        && let Some(i) = c.items.iter().position(|it| matches!(it, Item::Row(r) if c.rows[*r].hunk == Some(k as u32)))
    {
        dt.scroll_y = c.ys[i] - body.height() / 3.0;
    }
    caret_reveal(dt, docs, &geo);
    dt.scroll_y = dt.scroll_y.clamp(0.0, (total - body.height() + row_h * 2.0).max(0.0));
    // ---- Zeilen
    let pal = super::text::code_palette(&u);
    let hl_left = side_highlight(&mut dt.left, docs, cx.prefs, dt.right.doc());
    let hl_right = side_highlight(&mut dt.right, docs, cx.prefs, dt.left.doc());
    let c = dt.cache.as_ref().expect("Cache");
    let first = c.ys.partition_point(|y| *y <= dt.scroll_y).saturating_sub(1);
    let mut visible: Vec<(usize, f32)> = Vec::new();
    let mut i = first;
    while i < n_items {
        let y = body.top() + c.ys[i] - dt.scroll_y;
        if y > body.bottom() {
            break;
        }
        visible.push((i, y));
        i += 1;
    }
    // Syntaxfarben der sichtbaren Zeilen vorbereiten
    let (mut lmin, mut lmax, mut rmin, mut rmax) = (usize::MAX, 0usize, usize::MAX, 0usize);
    for (i, _) in &visible {
        if let Item::Row(r) = &c.items[*i] {
            let row = c.rows[*r];
            if let Some(l) = row.left {
                lmin = lmin.min(l as usize);
                lmax = lmax.max(l as usize + 1);
            }
            if let Some(x) = row.right {
                rmin = rmin.min(x as usize);
                rmax = rmax.max(x as usize + 1);
            }
        }
    }
    let mut pending = false;
    let spans_l = prepare_hl(hl_left, &mut dt.left, docs, lmin..lmax, &mut pending);
    let spans_r = prepare_hl(hl_right, &mut dt.right, docs, rmin..rmax, &mut pending);
    if pending {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let c = dt.cache.as_ref().expect("Cache");
    let painter = ui.painter_at(body);
    painter.rect_filled(body, 0.0, cx.theme.bg);
    let fg = cx.theme.fg;
    let dim = style::dim(ui);
    let mut arrows: Vec<(Rect, usize, bool)> = Vec::new();
    let mut folds: Vec<(Rect, Range<u32>)> = Vec::new();
    let left_editable = dt.left.doc().is_some_and(|d| docs.get(&d).is_some_and(|e| e.doc.is_some()));
    let right_editable = dt.right.doc().is_some_and(|d| docs.get(&d).is_some_and(|e| e.doc.is_some()));
    let scroll_x = if wrap.is_some() { 0.0 } else { dt.scroll_x };
    // Cursor und Auswahl (#391); untereinander mit Cursor links zeigen gleiche Zeilen den linken Text.
    let docs_r: &BTreeMap<u64, DocEntry> = docs;
    let cp = caret_paint(dt, docs_r, ui, out.focused, &o);
    let uni_left = unified && dt.unified_side(docs_r) == Some(true);
    let mut caret_at: Option<Pos2> = None;
    for (i, y) in &visible {
        let y = *y;
        match &c.items[*i] {
            Item::Fold { rows } => {
                let a = c.rows[rows.start].right.unwrap_or(0);
                let b = c.rows[rows.end - 1].right.map(|x| x + 1).unwrap_or(a);
                let r = Rect::from_min_size(Pos2::new(inner.left(), y + 2.0), Vec2::new(inner.width(), fold_h - 4.0));
                painter.rect_filled(r, 3.0, u.surface);
                let n = rows.len();
                let label = format!("⋯ {n} {}", if n == 1 { lang.t("unveränderte Zeile", "unchanged line") } else { lang.t("unveränderte Zeilen", "unchanged lines") });
                painter.text(Pos2::new(r.left() + 12.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(12.0), dim);
                folds.push((r, a..b));
                // Cursor in einem eingeklappten Bereich: am Anfang des Balkens (Tippen klappt ihn auf).
                if let Some(cp) = &cp {
                    let side = |x: &Row| if cp.left { x.left } else { x.right };
                    if let (Some(fa), Some(fb)) = (side(&c.rows[rows.start]), side(&c.rows[rows.end - 1]))
                        && (fa as usize..=fb as usize).contains(&cp.line)
                    {
                        let at = Pos2::new(r.left() + 5.0, r.center().y - row_h / 2.0);
                        if cp.on {
                            painter.rect_filled(Rect::from_min_size(at, Vec2::new(2.0, row_h)), 0.0, cp.color);
                        }
                        caret_at = Some(at);
                    }
                }
            }
            Item::Row(ri) => {
                let row = c.rows[*ri];
                let h = c.spans[*i] as f32 * row_h;
                let current = row.hunk.is_some_and(|k| Some(k as usize) == dt.current);
                let pair = |l: Option<u32>, r: Option<u32>| -> Option<&Option<(Vec<Range<usize>>, Vec<Range<usize>>)>> {
                    let k = row.hunk? as usize;
                    let hk = o.hunks.get(k)?;
                    match (l, r) {
                        (Some(l), Some(r)) => o.intra.get(&(l, r)),
                        (Some(l), None) => {
                            let off = l as usize - hk.old.start;
                            (off < hk.new.len()).then(|| o.intra.get(&(l, (hk.new.start + off) as u32))).flatten()
                        }
                        (None, Some(r)) => {
                            let off = r as usize - hk.new.start;
                            (off < hk.old.len()).then(|| o.intra.get(&((hk.old.start + off) as u32, r))).flatten()
                        }
                        _ => None,
                    }
                };
                let intra = pair(row.left, row.right);
                if unified {
                    let col = cols[0];
                    let band = Rect::from_min_max(Pos2::new(col.num.left(), y), Pos2::new(inner.right(), y + h));
                    let (bg, sign, word, line, snap, spans, which) = match row.kind {
                        RowKind::Delete => (Some(u.diff_remove_bg), "−", u.diff_remove_word, row.left, &o.left, &spans_l, 0),
                        RowKind::Insert => (Some(u.diff_add_bg), "+", u.diff_add_word, row.right, &o.right, &spans_r, 1),
                        _ if uni_left => (None, " ", Color32::TRANSPARENT, row.left, &o.left, &spans_l, 0),
                        _ => (None, " ", Color32::TRANSPARENT, row.right, &o.right, &spans_r, 1),
                    };
                    if let Some(bg) = bg {
                        painter.rect_filled(band, 0.0, bg);
                    }
                    if let Some(l) = row.left {
                        painter.text(Pos2::new(col.num.left() + num_w - 6.0, y), Align2::RIGHT_TOP, (l + 1).to_string(), cx.font.clone(), dim.gamma_multiply(0.8));
                    }
                    if let Some(r) = row.right {
                        painter.text(Pos2::new(col.num.left() + num_w * 2.0 - 6.0, y), Align2::RIGHT_TOP, (r + 1).to_string(), cx.font.clone(), dim.gamma_multiply(0.8));
                    }
                    painter.text(Pos2::new(col.num.right() + cw * 0.5, y), Align2::LEFT_TOP, sign, cx.font.clone(), dim);
                    if let Some(ln) = line {
                        let ranges = intra.map(|p| p.as_ref().map(|(a, b)| if which == 0 { a.as_slice() } else { b.as_slice() }));
                        let tp = painter.with_clip_rect(col.text.intersect(body));
                        let spans = spans.as_ref().and_then(|s| s.get(&(ln as usize)).map(Vec::as_slice));
                        let mine = cp.as_ref().filter(|cp| cp.left == (which == 0));
                        let lw = LineGeo { area: col.text, y, h, wrap: wrap.map(|w| w.0), tab, cw, row_h, scroll_x };
                        if let Some(cp) = mine {
                            paint_selection(&tp, &lw, snap.line(ln as usize), ln as usize, cp);
                        }
                        paint_text(&tp, col.text, y, snap.line(ln as usize), wrap.map(|w| w.0), tab, cw, row_h, scroll_x, fg, &cx.font, spans.zip(Some(&pal)), ranges.filter(|_| bg.is_some()), word);
                        if let Some(cp) = mine
                            && let Some(at) = paint_caret(&tp, &lw, snap.line(ln as usize), ln as usize, snap.len(), cp)
                        {
                            caret_at = Some(at);
                        }
                    }
                    if current {
                        painter.rect_filled(Rect::from_min_size(Pos2::new(inner.left(), y), Vec2::new(3.0, h)), 0.0, style::accent());
                    }
                    if let Some(k) = row.hunk
                        && c.items.get(i.wrapping_sub(1)).is_none_or(|p| !matches!(p, Item::Row(pr) if c.rows[*pr].hunk == Some(k)))
                    {
                        arrow_buttons(&painter, center, y, row_h, k as usize, left_editable, right_editable, &mut arrows, ui);
                    }
                } else {
                    for (s, col) in cols.iter().enumerate() {
                        let line = if s == 0 { row.left } else { row.right };
                        let band = Rect::from_min_max(Pos2::new(col.num.left(), y), Pos2::new(col.text.right(), y + h));
                        let (bg, word) = match (row.kind, s) {
                            (RowKind::Equal, _) => (None, Color32::TRANSPARENT),
                            (_, 0) => (Some(u.diff_remove_bg), u.diff_remove_word),
                            _ => (Some(u.diff_add_bg), u.diff_add_word),
                        };
                        match line {
                            None => hatch(&painter, band, u.diff_filler),
                            Some(ln) => {
                                if let Some(bg) = bg {
                                    painter.rect_filled(band, 0.0, bg);
                                }
                                painter.text(Pos2::new(col.num.right() - 6.0, y), Align2::RIGHT_TOP, (ln + 1).to_string(), cx.font.clone(), dim.gamma_multiply(0.8));
                                let snap = if s == 0 { &o.left } else { &o.right };
                                let ranges = intra.map(|p| p.as_ref().map(|(a, b)| if s == 0 { a.as_slice() } else { b.as_slice() }));
                                let spans = if s == 0 { &spans_l } else { &spans_r };
                                let spans = spans.as_ref().and_then(|m| m.get(&(ln as usize)).map(Vec::as_slice));
                                let tp = painter.with_clip_rect(col.text.intersect(body));
                                let cols_w = wrap.map(|w| if s == 0 { w.0 } else { w.1 });
                                let mine = cp.as_ref().filter(|cp| cp.left == (s == 0));
                                let lw = LineGeo { area: col.text, y, h, wrap: cols_w, tab, cw, row_h, scroll_x };
                                if let Some(cp) = mine {
                                    paint_selection(&tp, &lw, snap.line(ln as usize), ln as usize, cp);
                                }
                                paint_text(&tp, col.text, y, snap.line(ln as usize), cols_w, tab, cw, row_h, scroll_x, fg, &cx.font, spans.zip(Some(&pal)), ranges.filter(|_| bg.is_some()), word);
                                if let Some(cp) = mine
                                    && let Some(at) = paint_caret(&tp, &lw, snap.line(ln as usize), ln as usize, snap.len(), cp)
                                {
                                    caret_at = Some(at);
                                }
                            }
                        }
                    }
                    if current {
                        painter.rect_filled(Rect::from_min_size(Pos2::new(inner.left(), y), Vec2::new(3.0, h)), 0.0, style::accent());
                    }
                    if let Some(k) = row.hunk
                        && c.items.get(i.wrapping_sub(1)).is_none_or(|p| !matches!(p, Item::Row(pr) if c.rows[*pr].hunk == Some(k)))
                    {
                        arrow_buttons(&painter, center, y, row_h, k as usize, left_editable, right_editable, &mut arrows, ui);
                    }
                }
            }
        }
    }
    if out.focused
        && let Some(at) = caret_at
    {
        let cursor_rect = Rect::from_min_size(at, Vec2::new(1.0, row_h));
        let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
        ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { rect: to_global * body, cursor_rect: to_global * cursor_rect }));
    }
    // Trennlinie zwischen den Seiten
    if !unified {
        painter.line_segment([center.center_top(), center.center_bottom()], Stroke::new(1.0_f32, u.border.gamma_multiply(0.6)));
    }
    // Falten: Knöpfe „↓ 20“, „↑ 20“, „Alle“
    let mut fold_cmd: Option<(Range<u32>, u8)> = None;
    for (r, lines) in &folds {
        let mut x = r.right() - 8.0;
        for (k, label) in [(2u8, lang.t("Alle zeigen", "Show all")), (1, "↑ 20"), (0, "↓ 20")] {
            let gal = painter.layout_no_wrap(label.to_string(), FontId::proportional(12.0), u.link);
            let br = Rect::from_min_size(Pos2::new(x - gal.size().x - 8.0, r.top() + 1.0), Vec2::new(gal.size().x + 8.0, r.height() - 2.0));
            x = br.left() - 6.0;
            let resp = ui.interact(br, cx.id.with(("fold", lines.start, k)), Sense::click());
            if resp.hovered() {
                painter.rect_filled(br, 3.0, u.hover_bg);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            painter.galley(Pos2::new(br.left() + 4.0, br.center().y - gal.size().y / 2.0), gal, u.link);
            if resp.clicked() {
                fold_cmd = Some((lines.clone(), k));
            }
        }
    }
    if let Some((lines, k)) = fold_cmd {
        match k {
            0 => dt.revealed.push(lines.start..(lines.start + EXPAND_STEP).min(lines.end)),
            1 => dt.revealed.push(lines.end.saturating_sub(EXPAND_STEP).max(lines.start)..lines.end),
            _ => dt.revealed.push(lines),
        }
        dt.cache = None;
    }
    // Blockpfeile
    let mut apply: Option<(usize, bool)> = None;
    for (r, k, to_left) in &arrows {
        let resp = ui.interact(*r, cx.id.with(("arrow", *k, *to_left)), Sense::click());
        let tip = if *to_left { lang.t("Block nach links übernehmen", "Copy block to the left") } else { lang.t("Block nach rechts übernehmen", "Copy block to the right") };
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.on_hover_text(tip).clicked() {
            apply = Some((*k, *to_left));
        }
    }
    dt.arrows = arrows;
    dt.folds = folds;
    if let Some((k, to_left)) = apply {
        dt.current = Some(k);
        if let Err(e) = dt.apply(k, to_left, docs, lang) {
            cmds.push(Cmd::Hint(e));
        }
    }
    // Klick: Block und Seite wählen; Doppelklick öffnet die Zeile im Datei-Reiter.
    if (resp.clicked() || resp.double_clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let c = dt.cache.as_ref().expect("Cache");
        let yy = p.y - body.top() + dt.scroll_y;
        let idx = c.ys.partition_point(|y| *y <= yy).saturating_sub(1);
        dt.focus_left = !unified && p.x < center.left();
        if let Some(Item::Row(r)) = c.items.get(idx) {
            let row = c.rows[*r];
            if let Some(k) = row.hunk {
                dt.current = Some(k as usize);
            }
            let time = ui.input(|i| i.time);
            let double = resp.double_clicked() || dt.last_press.is_some_and(|(t, lp)| time - t < 0.45 && (lp - p).length() < 6.0);
            dt.last_press = Some((time, p));
            if double {
                let (side, line) = if unified {
                    if row.right.is_some() { (&dt.right, row.right) } else { (&dt.left, row.left) }
                } else if dt.focus_left {
                    (&dt.left, row.left)
                } else {
                    (&dt.right, row.right)
                };
                if let (Some(d), Some(l)) = (side.doc(), line) {
                    cmds.push(Cmd::OpenAt(d, l as usize));
                }
            }
        }
    }
    // ---- Waagerechte Leiste zeichnen und bedienen
    dt.hbar = hbar;
    if let Some(area) = hbar {
        let content = super::hscroll::content_width(dt.cache.as_ref().map_or(0, |c| c.max_cols), cw);
        let (mut sx, mut drag) = (dt.scroll_x, dt.hdrag);
        super::hscroll::bar(ui, area, cx.id.with("hbar"), &mut sx, &mut drag, content, hview);
        dt.scroll_x = sx;
        dt.hdrag = drag;
    } else {
        dt.hdrag = None;
    }
    // ---- Bildlaufleiste mit Marken der Blöcke
    let track = Rect::from_min_max(Pos2::new(body.right() - SCROLLBAR_W, body.top()), body.max);
    scrollbar(ui, track, dt, total, body.height(), cx, &u);
    // Rahmen bei Fokus
    if out.focused {
        let side_rect = if unified { inner } else if dt.focus_left { Rect::from_min_max(cols[0].num.min, cols[0].text.max) } else { Rect::from_min_max(cols[1].num.min, cols[1].text.max) };
        ui.painter().rect_stroke(side_rect.shrink(0.5), 0.0, Stroke::new(1.0_f32, style::accent().gamma_multiply(0.35)), egui::StrokeKind::Inside);
    }
    out
}

/// Werkzeugleiste: Titel, Tauschen, Revision, Darstellung, Leerraum, Kontext, vorige/nächste Änderung, Menü.
fn toolbar(ui: &mut Ui, bar: Rect, dt: &mut DiffTab, docs: &BTreeMap<u64, DocEntry>, cx: &DiffCx, cmds: &mut Vec<Cmd>) {
    let lang = cx.lang;
    let u = style::ui();
    ui.painter().rect_filled(bar, 0.0, u.surface.gamma_multiply(0.6));
    ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0_f32, u.border));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(Vec2::new(8.0, 3.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let ui = &mut child;
    // Menü
    let more = ui.small_button("⋯").on_hover_text(lang.t("Weitere Aktionen", "More actions"));
    let diff = cx.diff;
    egui::Popup::menu(&more).show(|ui| {
        ui.set_min_width(220.0);
        if ui.button(lang.t("Diff kopieren", "Copy diff")).clicked() {
            if let Some(p) = dt.patch(docs, false) {
                ui.ctx().copy_text(p);
            }
            ui.close();
        }
        if ui.add_enabled(dt.current.is_some(), egui::Button::new(lang.t("Block kopieren", "Copy block"))).clicked() {
            if let Some(b) = dt.block_text() {
                ui.ctx().copy_text(b);
            }
            ui.close();
        }
        if ui.button(lang.t("Als Patch speichern …", "Save as patch …")).clicked() {
            cmds.push(Cmd::SavePatch(diff));
            ui.close();
        }
        ui.separator();
        if ui.button(if dt.show_all { lang.t("Unveränderte Bereiche einklappen", "Collapse unchanged regions") } else { lang.t("Alle Zeilen zeigen", "Show all lines") }).clicked() {
            dt.show_all = !dt.show_all;
            if !dt.show_all {
                dt.revealed.clear();
            }
            dt.cache = None;
            ui.close();
        }
        for (side, label) in [(&dt.left, lang.t("Linke Seite im Reiter öffnen", "Open left side in a tab")), (&dt.right, lang.t("Rechte Seite im Reiter öffnen", "Open right side in a tab"))] {
            if let Some(d) = side.doc()
                && ui.button(label).clicked()
            {
                cmds.push(Cmd::OpenAt(d, 0));
                ui.close();
            }
        }
    });
    // Nächste / vorige Änderung
    if style::icon_btn_small(ui, style::Icon::Down, lang.t("Nächste Änderung (F7)", "Next change (F7)")).clicked() {
        dt.step(true);
    }
    if style::icon_btn_small(ui, style::Icon::Up, lang.t("Vorige Änderung (Umschalt+F7)", "Previous change (Shift+F7)")).clicked() {
        dt.step(false);
    }
    // Kontextzeilen
    let mut ctxn = dt.context as u32;
    if ui
        .add(egui::DragValue::new(&mut ctxn).range(crate::editor_svc::settings::DIFF_CONTEXT_RANGE.0..=crate::editor_svc::settings::DIFF_CONTEXT_RANGE.1).speed(0.2))
        .on_hover_text(lang.t("Kontextzeilen um Änderungen", "Context lines around changes"))
        .changed()
    {
        dt.context = ctxn as usize;
        dt.cache = None;
    }
    if ui.checkbox(&mut dt.ignore_ws, lang.t("Leerraum ignorieren", "Ignore whitespace")).changed() {
        dt.recompute_now();
    }
    let layout_label = match dt.layout {
        DiffLayout::Auto => lang.t("Automatisch", "Automatic"),
        DiffLayout::Split => lang.t("Nebeneinander", "Side by side"),
        DiffLayout::Unified => lang.t("Untereinander", "Unified"),
    };
    let lb = ui.small_button(layout_label).on_hover_text(lang.t("Darstellung: nebeneinander, untereinander oder automatisch (unter 700 px untereinander)", "Layout: side by side, unified or automatic (unified below 700 px)"));
    egui::Popup::menu(&lb).show(|ui| {
        for (l, label) in [(DiffLayout::Auto, lang.t("Automatisch", "Automatic")), (DiffLayout::Split, lang.t("Nebeneinander", "Side by side")), (DiffLayout::Unified, lang.t("Untereinander", "Unified"))] {
            if ui.selectable_label(dt.layout == l, label).clicked() {
                dt.layout = l;
                dt.cache = None;
                ui.close();
            }
        }
    });
    if ui.small_button("⇄").on_hover_text(lang.t("Seiten tauschen", "Swap sides")).clicked() {
        cmds.push(Cmd::SwapDiff(diff));
    }
    let git_path = match (&dt.left, &dt.right) {
        (Side::Text(s), _) if matches!(s.source, Source::Git { .. }) => s.path().map(Path::to_path_buf),
        (_, Side::Doc(d)) | (Side::Doc(d), _) => docs.get(d).and_then(|e| e.path.clone()),
        _ => None,
    };
    if let Some(p) = git_path
        && ui.small_button(lang.t("Revision …", "Revision …")).on_hover_text(lang.t("Mit einem Stand in git vergleichen (HEAD, Index, Zweig, Tag, Commit)", "Compare with a state in git (HEAD, index, branch, tag, commit)")).clicked()
    {
        cmds.push(Cmd::PickRevision(p, Some(diff)));
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(dt.summary(lang)).small().color(style::dim(ui)));
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add(egui::Label::new(egui::RichText::new(dt.title(docs, lang)).strong()).truncate());
    });
}

/// Zwei kleine Pfeile am ersten Zeilenpaar eines Blocks: ← (rechts nach links) und → (links nach rechts).
#[allow(clippy::too_many_arguments)]
fn arrow_buttons(p: &egui::Painter, center: Rect, y: f32, row_h: f32, k: usize, left_ok: bool, right_ok: bool, out: &mut Vec<(Rect, usize, bool)>, ui: &Ui) {
    let s = (row_h).clamp(12.0, 18.0);
    let cy = y + row_h / 2.0;
    let u = style::ui();
    let mut draw = |cx: f32, to_left: bool, enabled: bool| {
        let r = Rect::from_center_size(Pos2::new(cx, cy), Vec2::splat(s));
        let hovered = enabled && ui.rect_contains_pointer(r);
        let col = if !enabled { style::dim(ui).gamma_multiply(0.35) } else if hovered { style::accent() } else { u.text_dim };
        if hovered {
            p.rect_filled(r, 3.0, u.hover_bg);
        }
        let c = r.center();
        let d = if to_left { -1.0 } else { 1.0 };
        let st = Stroke::new(1.6_f32, col);
        p.line_segment([c + Vec2::new(-4.0 * d, 0.0), c + Vec2::new(4.0 * d, 0.0)], st);
        p.line_segment([c + Vec2::new(4.0 * d, 0.0), c + Vec2::new(1.0 * d, -3.0)], st);
        p.line_segment([c + Vec2::new(4.0 * d, 0.0), c + Vec2::new(1.0 * d, 3.0)], st);
        if enabled {
            out.push((r, k, to_left));
        }
    };
    draw(center.center().x - s / 2.0 - 1.0, true, left_ok);
    draw(center.center().x + s / 2.0 + 1.0, false, right_ok);
}

/// Schraffur einer Füllzeile.
fn hatch(p: &egui::Painter, r: Rect, col: Color32) {
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    let step = 7.0;
    let mut x = r.left() - r.height();
    while x < r.right() {
        p.line_segment([Pos2::new(x, r.bottom()), Pos2::new(x + r.height(), r.top())], Stroke::new(1.0_f32, col));
        x += step;
    }
}

/// Text einer Zeile (mit Umbruch über mehrere Bildschirmzeilen), Syntaxfarben und Hervorhebungen in der Zeile.
#[allow(clippy::too_many_arguments)]
fn paint_text(p: &egui::Painter, area: Rect, y: f32, line: &str, wrap: Option<usize>, tab: usize, cw: f32, row_h: f32, scroll_x: f32, fg: Color32, font: &FontId, syntax: Option<(&[Span], &[Color32; 17])>, intra: Option<Option<&[Range<usize>]>>, word: Color32) {
    let starts = match wrap {
        Some(c) => wrap_starts(line, c, tab),
        None => vec![0],
    };
    let max_v = ((area.width() + scroll_x) / cw).ceil() as usize + 2;
    let first_v = (scroll_x / cw).floor() as usize;
    let mut si = 0usize;
    let mut color_at = |ci: usize| -> Color32 {
        let Some((spans, pal)) = syntax else { return fg };
        while si < spans.len() && (spans[si].end as usize) <= ci {
            si += 1;
        }
        match spans.get(si) {
            Some(s) if (s.start as usize) <= ci => pal[s.tok.index()],
            _ => fg,
        }
    };
    let whole = matches!(intra, Some(None));
    let ranges: &[Range<usize>] = match intra {
        Some(Some(r)) => r,
        _ => &[],
    };
    let mut row = 0usize;
    let mut v = 0usize;
    let mut row_v = 0usize;
    let mut run = String::new();
    let mut run_x = 0.0f32;
    let mut run_col = fg;
    let mut mark_from: Option<f32> = None;
    let x_of = |v: usize, row_v: usize| area.left() + (v - row_v) as f32 * cw - if wrap.is_some() { 0.0 } else { scroll_x };
    let ry = |row: usize| y + row as f32 * row_h;
    // Erst sammeln, dann zeichnen: die Hervorhebungen liegen unter dem Text.
    let mut texts: Vec<(Pos2, String, Color32)> = Vec::new();
    let mut marks: Vec<Rect> = Vec::new();
    let flush = |texts: &mut Vec<(Pos2, String, Color32)>, run: &mut String, x: f32, row: usize, col: Color32| {
        if !run.is_empty() {
            texts.push((Pos2::new(x, ry(row)), std::mem::take(run), col));
        }
    };
    let n_chars = line.chars().count();
    let mut ri = 0usize;
    for (ci, ch) in line.chars().enumerate() {
        if starts.get(row + 1) == Some(&ci) {
            // Umbruch: Markierung am Ende der Bildschirmzeile schließen und in der nächsten weiterführen.
            flush(&mut texts, &mut run, run_x, row, run_col);
            let marking = mark_from.take().map(|x0| marks.push(Rect::from_min_max(Pos2::new(x0, ry(row)), Pos2::new(x_of(v, row_v), ry(row) + row_h)))).is_some();
            row += 1;
            row_v = v;
            if marking {
                mark_from = Some(x_of(v, row_v));
            }
        }
        while ri < ranges.len() && ranges[ri].end <= ci {
            ri += 1;
        }
        let in_mark = whole || ranges.get(ri).is_some_and(|r| r.start <= ci);
        match (in_mark, mark_from) {
            (true, None) => mark_from = Some(x_of(v, row_v)),
            (false, Some(x0)) => {
                marks.push(Rect::from_min_max(Pos2::new(x0, ry(row)), Pos2::new(x_of(v, row_v), ry(row) + row_h)));
                mark_from = None;
            }
            _ => {}
        }
        let w = et::char_width(ch, v, tab);
        if wrap.is_none() && v > max_v {
            break;
        }
        let visible = wrap.is_some() || v + w >= first_v;
        if ch == '\t' || !visible {
            flush(&mut texts, &mut run, run_x, row, run_col);
        } else if (ch as u32) < 0x20 {
            flush(&mut texts, &mut run, run_x, row, run_col);
        } else if ch.is_ascii() {
            let col = color_at(ci);
            if col != run_col || run.is_empty() {
                flush(&mut texts, &mut run, run_x, row, run_col);
                run_col = col;
                run_x = x_of(v, row_v);
            }
            run.push(ch);
        } else {
            flush(&mut texts, &mut run, run_x, row, run_col);
            let col = color_at(ci);
            let mut b = [0u8; 4];
            texts.push((Pos2::new(x_of(v, row_v), ry(row)), ch.encode_utf8(&mut b).to_string(), col));
        }
        v += w;
    }
    flush(&mut texts, &mut run, run_x, row, run_col);
    if let Some(x0) = mark_from {
        let x1 = x_of(v, row_v) + if n_chars == 0 { cw * 0.6 } else { 0.0 };
        marks.push(Rect::from_min_max(Pos2::new(x0, ry(row)), Pos2::new(x1.max(x0 + 2.0), ry(row) + row_h)));
    }
    if whole && n_chars == 0 && marks.is_empty() {
        marks.push(Rect::from_min_size(Pos2::new(area.left(), y), Vec2::new(cw * 0.6, row_h)));
    }
    for r in marks {
        p.rect_filled(r, 2.0, word);
    }
    for (pos, t, col) in texts {
        p.text(pos, Align2::LEFT_TOP, t, font.clone(), col);
    }
}

/// Binärdateien oder zu große Dateien: Größe und SHA-256 beider Seiten.
fn big_message(ui: &mut Ui, body: Rect, big: &BigInfo, dt: &DiffTab, docs: &BTreeMap<u64, DocEntry>, lang: Lang) {
    let r = Rect::from_center_size(Pos2::new(body.center().x, body.top() + (body.height() * 0.35).max(90.0)), Vec2::new((body.width() - 40.0).clamp(200.0, 760.0), 160.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    let same = big.left.1 == big.right.1 && !big.left.1.is_empty();
    ui.heading(if same {
        lang.t("Die Dateien sind gleich.", "The files are identical.")
    } else if big.binary {
        lang.t("Binärdateien unterscheiden sich.", "Binary files differ.")
    } else {
        lang.t("Zu groß zum zeilenweisen Vergleichen (Grenze für große Dateien).", "Too large to compare line by line (large-file limit).")
    });
    ui.add_space(style::S2);
    for (side, (len, sha)) in [(&dt.left, &big.left), (&dt.right, &big.right)] {
        ui.label(egui::RichText::new(side.label(docs, lang)).strong());
        ui.label(egui::RichText::new(format!("{} · SHA-256 {}", human_size(*len, lang), if sha.is_empty() { "–" } else { sha })).monospace().small());
        ui.add_space(style::S1);
    }
}

pub(super) fn human_size(n: u64, lang: Lang) -> String {
    if n < 1024 {
        format!("{n} {}", lang.t("Bytes", "bytes"))
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Senkrechte Bildlaufleiste mit Marken der Blöcke.
fn scrollbar(ui: &mut Ui, track: Rect, dt: &mut DiffTab, total: f32, view_h: f32, cx: &DiffCx, u: &crate::theme::tokens::UiColors) {
    let p = ui.painter();
    if let (Some(c), Some(o)) = (&dt.cache, &dt.out)
        && total > 0.0
    {
        for (i, it) in c.items.iter().enumerate() {
            if let Item::Row(r) = it {
                let row = c.rows[*r];
                if row.hunk.is_none() || c.items.get(i.wrapping_sub(1)).is_some_and(|p| matches!(p, Item::Row(pr) if c.rows[*pr].hunk == row.hunk)) {
                    continue;
                }
                let k = row.hunk.unwrap_or(0) as usize;
                let col = match o.hunks.get(k).map(|h| h.kind()) {
                    Some(compare::HunkKind::Added) => u.marker_added,
                    Some(compare::HunkKind::Deleted) => u.marker_deleted,
                    _ => u.marker_modified,
                };
                let y = track.top() + c.ys[i] / total * track.height();
                p.rect_filled(Rect::from_min_size(Pos2::new(track.left() + 1.0, y), Vec2::new(track.width() - 2.0, 2.0)), 0.0, col);
            }
        }
    }
    if total <= view_h {
        return;
    }
    let h = (track.height() * (view_h / total)).clamp(24.0, track.height());
    let frac = dt.scroll_y / (total - view_h).max(1.0);
    let y = track.top() + (track.height() - h) * frac.clamp(0.0, 1.0);
    let thumb = Rect::from_min_size(Pos2::new(track.left() + 2.0, y), Vec2::new(track.width() - 4.0, h));
    let resp = ui.interact(track, cx.id.with("vscroll"), Sense::click_and_drag());
    let hot = resp.hovered() || resp.dragged();
    ui.painter().rect_filled(thumb, 4.0, if hot { u.text_dim.gamma_multiply(0.7) } else { u.text_dim.gamma_multiply(0.35) });
    if let Some(pp) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let f = ((pp.y - h / 2.0 - track.top()) / (track.height() - h).max(1.0)).clamp(0.0, 1.0);
        dt.scroll_y = f * (total - view_h);
    }
}

/// Tastatur im Vergleich: Rollen, Rückgängig/Wiederholen auf der fokussierten Seite, Blockpfeile mit Alt+←/→, Block
/// kopieren mit Strg+C.
#[allow(clippy::too_many_arguments)]
fn keyboard(ui: &mut Ui, dt: &mut DiffTab, docs: &mut BTreeMap<u64, DocEntry>, body: Rect, row_h: f32, total: f32, lang: Lang, cmds: &mut Vec<Cmd>) {
    let events = ui.input(|i| i.events.clone());
    // Mit Cursor (#391) bewegen Pfeile, Bild auf/ab, Pos1/Ende den Cursor, Strg+Z/Y und Kopieren einer Auswahl gehen an
    // `edit_keys`.
    let caret = dt.caret.is_some();
    let selected = dt.caret.as_ref().is_some_and(|c| !c.sels.primary().is_empty());
    for e in events {
        match e {
            egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                let ctrl = m.ctrl || m.command;
                match key {
                    Key::ArrowDown if !m.alt && !caret => dt.scroll_y += row_h * 3.0,
                    Key::ArrowUp if !m.alt && !caret => dt.scroll_y -= row_h * 3.0,
                    Key::PageDown if !caret => dt.scroll_y += body.height() - row_h * 2.0,
                    Key::PageUp if !caret => dt.scroll_y -= body.height() - row_h * 2.0,
                    Key::Home if ctrl && !caret => dt.scroll_y = 0.0,
                    Key::End if ctrl && !caret => dt.scroll_y = total,
                    // Nur Alt (AltGr meldet Strg+Alt und darf keine Blöcke übernehmen).
                    Key::ArrowLeft if m.alt && !ctrl => {
                        if let Some(k) = dt.current
                            && let Err(e) = dt.apply(k, true, docs, lang)
                        {
                            cmds.push(Cmd::Hint(e));
                        }
                    }
                    Key::ArrowRight if m.alt && !ctrl => {
                        if let Some(k) = dt.current
                            && let Err(e) = dt.apply(k, false, docs, lang)
                        {
                            cmds.push(Cmd::Hint(e));
                        }
                    }
                    Key::ArrowLeft if !ctrl && !caret => dt.scroll_x = (dt.scroll_x - 40.0).max(0.0),
                    Key::ArrowRight if !ctrl && !caret => dt.scroll_x += 40.0,
                    Key::Z | Key::Y if ctrl && !caret => {
                        let redo = key == Key::Y || m.shift;
                        if let Some(d) = dt.focused_doc().and_then(|d| docs.get_mut(&d)) {
                            if redo {
                                d.redo_user();
                            } else {
                                d.undo_user();
                            }
                            dt.recompute_now();
                        }
                    }
                    _ => {}
                }
            }
            egui::Event::Copy if !selected => {
                if let Some(b) = dt.block_text() {
                    ui.ctx().copy_text(b);
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------------------------------------ Cursor (#391)

/// Cursor mit dem Dokument abgleichen (Änderungen über andere Ansichten); ohne bearbeitbare Seite fällt er weg.
fn sync_caret(dt: &mut DiffTab, docs: &BTreeMap<u64, DocEntry>) {
    let Some(left) = dt.caret.as_ref().map(|c| c.left) else { return };
    let doc = (if left { dt.left.doc() } else { dt.right.doc() }).and_then(|d| docs.get(&d)).and_then(|e| e.doc.as_ref());
    match (doc, dt.caret.as_mut()) {
        (Some(doc), Some(c)) => {
            doc.sync(&mut c.sels);
            if c.sels.is_multi() {
                c.sels.collapse_to_primary();
            }
        }
        _ => dt.caret = None,
    }
}

/// Tippen und Bewegen in der Seite des Cursors: Text, Einfügen, Ausschneiden/Kopieren einer Auswahl, Rücktaste/Entf,
/// Eingabe, Tab, Pfeile/Pos1/Ende/Bild (mit Umschalt markieren), Strg+A/Z/Y, Esc. Bearbeiten in einem eingeklappten
/// Bereich klappt ihn auf. Mehrere Cursor gibt es hier nicht (Strg+D usw. bleiben ohne Wirkung). Liefert, ob sich der
/// Text geändert hat. Ohne Cursor meldet Tippen in einer schreibgeschützten Seite einen Hinweis.
#[allow(clippy::too_many_arguments)]
fn edit_keys(ui: &mut Ui, dt: &mut DiffTab, docs: &mut BTreeMap<u64, DocEntry>, page: usize, row_h: f32, unified: bool, lang: Lang, cmds: &mut Vec<Cmd>) -> bool {
    use super::text::{KeyAct, key_op, move_in_doc};
    let events = ui.input(|i| i.events.clone());
    let Some(mut caret) = dt.caret.take() else {
        let typed = events.iter().any(|e| matches!(e, Event::Text(_) | Event::Paste(_)));
        let side_left = if unified { dt.unified_side(docs) } else { Some(dt.focus_left) };
        if typed && side_left.is_none_or(|l| !dt.editable(l, docs)) {
            cmds.push(Cmd::Hint(lang.t("Diese Seite ist schreibgeschützt.", "This side is read-only.").to_string()));
        }
        return false;
    };
    let Some(id) = (if caret.left { dt.left.doc() } else { dt.right.doc() }) else { return false };
    let Some(e) = docs.get_mut(&id) else { return false };
    let Some(doc) = e.doc.as_mut() else { return false };
    let tab = doc.tab_width().max(1);
    let time = ui.input(|i| i.time);
    let (mut edited, mut undone) = (false, false);
    let mut scroll = 0i32;
    for ev in events {
        let mut moved = true;
        let op = match ev {
            Event::Text(t) => caret.preedit.is_empty().then_some(Op::Type(t)),
            Event::Paste(t) => Some(Op::Paste(t)),
            Event::Cut => (!caret.sels.primary().is_empty()).then_some(Op::Cut),
            Event::Copy => {
                if !caret.sels.primary().is_empty() {
                    let (t, _) = super::doc::clip(doc, &mut caret.sels, false);
                    ui.ctx().copy_text(if cfg!(windows) { t.replace('\n', "\r\n") } else { t });
                }
                moved = false;
                None
            }
            Event::Ime(egui::ImeEvent::Preedit(t)) => {
                caret.preedit = t;
                None
            }
            Event::Ime(egui::ImeEvent::Commit(t)) => {
                caret.preedit.clear();
                (!t.is_empty()).then_some(Op::Type(t))
            }
            Event::Ime(egui::ImeEvent::Disabled) => {
                caret.preedit.clear();
                None
            }
            Event::Key { key, pressed: true, modifiers: m, .. } => {
                let ctrl = m.ctrl || m.command;
                if m.alt && !ctrl && matches!(key, Key::ArrowLeft | Key::ArrowRight) {
                    // Blockpfeile (`keyboard`)
                    moved = false;
                    None
                } else {
                    match key_op(key, m, page) {
                        Some(KeyAct::Op(Op::SelectNextOccurrence | Op::SelectAllOccurrences)) | Some(KeyAct::AddCursor(_)) | None => {
                            moved = false;
                            None
                        }
                        Some(KeyAct::Op(op)) => Some(op),
                        Some(KeyAct::Move(mv, extend)) => {
                            move_in_doc(doc, &mut caret.sels, mv, extend, tab);
                            None
                        }
                        Some(KeyAct::ScrollLines(n)) => {
                            scroll += n;
                            moved = false;
                            None
                        }
                        Some(KeyAct::Escape) => {
                            let mut p = caret.sels.primary();
                            p.anchor = p.head;
                            caret.sels.set_primary(p);
                            None
                        }
                    }
                }
            }
            _ => {
                moved = false;
                None
            }
        };
        if let Some(op) = op {
            let undo = matches!(op, Op::Undo | Op::Redo);
            let changes = !matches!(op, Op::SelectAll | Op::SelectLine | Op::Undo | Op::Redo);
            if changes {
                let line = doc.char_to_line(caret.sels.primary().head.min(doc.len_chars()));
                dt.unfold_line(caret.left, line);
            }
            if super::doc::run(doc, &mut caret.sels, op, tab) {
                edited = true;
                undone |= undo;
            }
        }
        if moved {
            caret.reveal = true;
            caret.blink = time;
        }
    }
    if caret.sels.is_multi() {
        caret.sels.collapse_to_primary();
    }
    dt.scroll_y += scroll as f32 * row_h;
    dt.focus_left = caret.left;
    dt.caret = Some(caret);
    if edited {
        e.on_user_edit();
        if undone {
            // Rückgängig/Wiederholen wie bisher sofort neu vergleichen, Tippen erst nach der Pause.
            dt.recompute_now();
        }
    }
    edited
}

/// Geometrie für Cursor und Maus.
struct CaretGeo {
    body: Rect,
    cols: [Col; 2],
    center: Rect,
    unified: bool,
    wrap: Option<(usize, usize)>,
    tab: usize,
    row_h: f32,
    cw: f32,
    scroll_x: f32,
    hview: f32,
}

impl CaretGeo {
    fn text(&self, left: bool) -> Rect {
        if self.unified || left { self.cols[0].text } else { self.cols[1].text }
    }

    fn wrap_cols(&self, left: bool) -> Option<usize> {
        self.wrap.map(|w| if self.unified || left { w.0 } else { w.1 })
    }
}

/// Bildschirmzeilen-Anfänge (Zeichenspalten) einer Zeile.
fn starts_of(line: &str, wrap: Option<usize>, tab: usize) -> Vec<usize> {
    match wrap {
        Some(c) => wrap_starts(line, c, tab),
        None => vec![0],
    }
}

/// Zeichen-Position im Dokument der Seite unter dem Zeiger `p`. Füllzeilen: Ende der Zeile davor (sonst Anfang der
/// nächsten). Eingeklappte Bereiche: `None`.
fn pos_in_side(dt: &DiffTab, doc: &Document, left: bool, g: &CaretGeo, p: Pos2) -> Option<usize> {
    let c = dt.cache.as_ref()?;
    if c.items.is_empty() {
        return Some(0);
    }
    let side = |r: &Row| if left { r.left } else { r.right };
    let yy = (p.y - g.body.top() + dt.scroll_y).max(0.0);
    let idx = c.ys.partition_point(|y| *y <= yy).saturating_sub(1).min(c.items.len() - 1);
    let last = doc.len_lines().saturating_sub(1);
    let Item::Row(ri) = &c.items[idx] else { return None };
    let Some(ln) = side(&c.rows[*ri]) else {
        // Füllzeile: zur nächsten echten Zeile der Seite (zuerst nach oben).
        let line_of = |i: usize| match &c.items[i] {
            Item::Row(r) => side(&c.rows[*r]).map(|l| (l as usize).min(last)),
            Item::Fold { .. } => None,
        };
        if let Some(l) = (0..idx).rev().find_map(line_of) {
            return Some(et::line_end(doc.rope(), l));
        }
        return Some((idx..c.items.len()).find_map(line_of).map_or(0, |l| doc.line_to_char(l)));
    };
    let ln = (ln as usize).min(last);
    let line: String = et::line_slice(doc.rope(), ln).to_string();
    let starts = starts_of(&line, g.wrap_cols(left), g.tab);
    let row_top = g.body.top() + c.ys[idx] - dt.scroll_y;
    let sub = (((p.y - row_top) / g.row_h).floor().max(0.0) as usize).min(starts.len() - 1);
    let vstart = str_vcol(&line, starts[sub], g.tab);
    let v = (((p.x - g.text(left).left() + g.scroll_x) / g.cw).round().max(0.0) as usize) + vstart;
    let n = line.chars().count();
    let end = starts.get(sub + 1).copied().unwrap_or(n);
    let mut col = str_col_at(&line, v, g.tab).clamp(starts[sub], end);
    // Am Umbruch: das letzte Zeichen der Bildschirmzeile gehört schon zur nächsten.
    if col == end && end < n && col > starts[sub] {
        col -= 1;
    }
    Some(doc.line_to_char(ln) + col)
}

/// Maus im Text (#391): Klick setzt den Cursor in einer bearbeitbaren Seite (Umschalt+Klick dehnt die Auswahl), Ziehen
/// markiert. Ein Klick in eine schreibgeschützte Seite nimmt den Cursor weg (Pfeiltasten rollen dann wieder).
fn caret_mouse(ui: &Ui, resp: &egui::Response, dt: &mut DiffTab, docs: &mut BTreeMap<u64, DocEntry>, g: &CaretGeo) {
    let (pressed, down, pos, time, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.time, i.modifiers));
    let Some(p) = pos else { return };
    if pressed && resp.hovered() && g.body.contains(p) {
        if dt.arrows.iter().any(|(r, ..)| r.contains(p)) || dt.folds.iter().any(|(r, _)| r.contains(p)) {
            return;
        }
        let left = if g.unified {
            match dt.unified_side(docs) {
                Some(l) => l,
                None => {
                    dt.caret = None;
                    return;
                }
            }
        } else if p.x < g.center.left() {
            true
        } else if p.x > g.center.right() {
            false
        } else {
            return;
        };
        if !dt.editable(left, docs) {
            dt.caret = None;
            return;
        }
        let Some(id) = (if left { dt.left.doc() } else { dt.right.doc() }) else { return };
        let Some(doc) = docs.get_mut(&id).and_then(|e| e.doc.as_mut()) else { return };
        let Some(pos) = pos_in_side(dt, doc, left, g, p) else { return };
        doc.break_undo_group();
        match dt.caret.as_mut() {
            Some(c) if c.left == left && mods.shift => {
                let mut s = c.sels.primary();
                s.head = pos;
                s.goal = None;
                c.sels.replace(vec![s], 0);
                c.drag = Some(s.anchor);
                c.blink = time;
            }
            _ => {
                let mut c = Caret::new(left, doc, pos, time);
                c.drag = Some(pos);
                dt.caret = Some(c);
            }
        }
        dt.focus_left = left;
        return;
    }
    if !down {
        if let Some(c) = dt.caret.as_mut() {
            c.drag = None;
        }
        return;
    }
    let Some((left, anchor)) = dt.caret.as_ref().and_then(|c| c.drag.map(|a| (c.left, a))) else { return };
    // Ziehen über den Rand: rollen
    if p.y < g.body.top() {
        dt.scroll_y -= ((g.body.top() - p.y) * 0.3).clamp(1.0, g.row_h * 3.0);
        ui.ctx().request_repaint();
    } else if p.y > g.body.bottom() {
        dt.scroll_y += ((p.y - g.body.bottom()) * 0.3).clamp(1.0, g.row_h * 3.0);
        ui.ctx().request_repaint();
    }
    let Some(doc) = (if left { dt.left.doc() } else { dt.right.doc() }).and_then(|d| docs.get(&d)).and_then(|e| e.doc.as_ref()) else { return };
    let q = Pos2::new(p.x, p.y.clamp(g.body.top(), g.body.bottom() - 1.0));
    if let Some(pos) = pos_in_side(dt, doc, left, g, q)
        && let Some(c) = dt.caret.as_mut()
    {
        c.sels.replace(vec![Selection::new(anchor, pos)], 0);
    }
}

/// Nach Tastatur/Bearbeitung den Cursor in den sichtbaren Bereich holen (senkrecht und ohne Umbruch waagerecht).
fn caret_reveal(dt: &mut DiffTab, docs: &BTreeMap<u64, DocEntry>, g: &CaretGeo) {
    let Some(c) = dt.caret.as_mut() else { return };
    if !std::mem::take(&mut c.reveal) {
        return;
    }
    let left = c.left;
    let head = c.sels.primary().head;
    let Some(doc) = (if left { dt.left.doc() } else { dt.right.doc() }).and_then(|d| docs.get(&d)).and_then(|e| e.doc.as_ref()) else { return };
    let Some(cache) = dt.cache.as_ref() else { return };
    let lc = et::pos_to_linecol(doc.rope(), head.min(doc.len_chars()));
    let side = |r: &Row| if left { r.left } else { r.right };
    // Die leere Zeile nach dem letzten Zeilenende hat keine eigene Reihe: die der Zeile davor.
    let n_side = cache.rows.iter().rev().find_map(|r| side(r)).map_or(0, |l| l as usize + 1);
    let target = lc.line.min(n_side.saturating_sub(1));
    let Some(idx) = cache.items.iter().position(|it| match it {
        Item::Row(r) => side(&cache.rows[*r]).is_some_and(|l| l as usize == target),
        Item::Fold { rows } => matches!((side(&cache.rows[rows.start]), side(&cache.rows[rows.end - 1])), (Some(a), Some(b)) if (a as usize..=b as usize).contains(&target)),
    }) else {
        return;
    };
    let line: String = et::line_slice(doc.rope(), lc.line).to_string();
    let starts = starts_of(&line, g.wrap_cols(left), g.tab);
    let sub = if matches!(cache.items[idx], Item::Row(_)) && target == lc.line { starts.iter().rposition(|s| *s <= lc.col).unwrap_or(0) } else { 0 };
    let extra = if target < lc.line { cache.ys[idx + 1] - cache.ys[idx] } else { 0.0 };
    let y = cache.ys[idx] + sub as f32 * g.row_h + extra;
    let view_h = g.body.height();
    if y < dt.scroll_y {
        dt.scroll_y = y;
    } else if y + g.row_h > dt.scroll_y + view_h {
        dt.scroll_y = y + g.row_h - view_h;
    }
    if g.wrap.is_none() {
        let x = str_vcol(&line, lc.col, g.tab) as f32 * g.cw;
        let margin = g.cw * 2.0;
        if x < dt.scroll_x + margin {
            dt.scroll_x = (x - margin).max(0.0);
        } else if x > dt.scroll_x + g.hview - margin {
            dt.scroll_x = (x - g.hview + margin).clamp(0.0, dt.hmax.max(0.0));
        }
    }
}

/// Was für Cursor und Auswahl gezeichnet wird.
struct CaretPaint<'a> {
    left: bool,
    doc: &'a Document,
    sel: Selection,
    line: usize,
    col: usize,
    on: bool,
    color: Color32,
    sel_bg: Color32,
}

fn caret_paint<'a>(dt: &DiffTab, docs: &'a BTreeMap<u64, DocEntry>, ui: &Ui, focused: bool, _o: &DiffOut) -> Option<CaretPaint<'a>> {
    let c = dt.caret.as_ref()?;
    let d = if c.left { dt.left.doc() } else { dt.right.doc() }?;
    let doc = docs.get(&d)?.doc.as_ref()?;
    let head = c.sels.primary().head.min(doc.len_chars());
    let lc = et::pos_to_linecol(doc.rope(), head);
    let on = !focused || {
        let t = ui.input(|i| i.time) - c.blink;
        if t <= 30.0 {
            ui.ctx().request_repaint_after(Duration::from_millis(530));
        }
        t > 30.0 || ((t / 0.53) as u64).is_multiple_of(2)
    };
    let u = style::ui();
    let color = if focused { u.focus } else { style::dim(ui).gamma_multiply(0.6) };
    Some(CaretPaint { left: c.left, doc, sel: c.sels.primary(), line: lc.line, col: lc.col, on, color, sel_bg: u.selection_bg })
}

/// Lage einer gezeichneten Zeile.
struct LineGeo {
    area: Rect,
    y: f32,
    /// Höhe der Reihe (alle Bildschirmzeilen).
    h: f32,
    wrap: Option<usize>,
    tab: usize,
    cw: f32,
    row_h: f32,
    scroll_x: f32,
}

/// Auswahl in einer Zeile (unter dem Text).
fn paint_selection(p: &egui::Painter, g: &LineGeo, line: &str, ln: usize, cp: &CaretPaint) {
    let s = cp.sel;
    if s.is_empty() || ln >= cp.doc.len_lines() {
        return;
    }
    let base = cp.doc.line_to_char(ln);
    let n = line.chars().count();
    let (a, b) = (s.start(), s.end());
    if b < base || a > base + n {
        return;
    }
    let (ca, cb) = (a.saturating_sub(base).min(n), (b - base).min(n));
    let starts = starts_of(line, g.wrap, g.tab);
    for (r, &rs) in starts.iter().enumerate() {
        let re = starts.get(r + 1).copied().unwrap_or(n);
        let beyond = r + 1 == starts.len() && b > base + n;
        let (x0c, x1c) = (ca.max(rs), cb.min(re));
        if x1c < x0c || (x1c == x0c && !beyond) {
            continue;
        }
        let vs = str_vcol(line, rs, g.tab);
        let x = |col: usize| g.area.left() + (str_vcol(line, col, g.tab) - vs) as f32 * g.cw - g.scroll_x;
        let mut x1 = x(x1c);
        if beyond {
            x1 += g.cw * 0.6;
        }
        let y = g.y + r as f32 * g.row_h;
        p.rect_filled(Rect::from_min_max(Pos2::new(x(x0c), y), Pos2::new(x1, y + g.row_h)), 0.0, cp.sel_bg);
    }
}

/// Cursor in einer Zeile; die leere Zeile nach dem letzten Zeilenende (`side_len` = Zeilen der Seite) steht unter der
/// letzten. Liefert die Position (oben links), wenn er hier steht.
fn paint_caret(p: &egui::Painter, g: &LineGeo, line: &str, ln: usize, side_len: usize, cp: &CaretPaint) -> Option<Pos2> {
    let at = if cp.line == ln {
        let n = line.chars().count();
        let col = cp.col.min(n);
        let starts = starts_of(line, g.wrap, g.tab);
        let r = starts.iter().rposition(|s| *s <= col).unwrap_or(0);
        let x = g.area.left() + (str_vcol(line, col, g.tab) - str_vcol(line, starts[r], g.tab)) as f32 * g.cw - g.scroll_x - 1.0;
        Pos2::new(x, g.y + r as f32 * g.row_h)
    } else if cp.line == side_len && ln + 1 == side_len {
        Pos2::new(g.area.left() - g.scroll_x - 1.0, g.y + g.h)
    } else {
        return None;
    };
    if cp.on {
        p.rect_filled(Rect::from_min_size(at, Vec2::new(2.0, g.row_h)), 0.0, cp.color);
    }
    Some(at)
}

/// Sprache und Modus für die Syntaxfarben einer Seite: ein Dokument hat seine eigenen; ein fester Text nimmt die
/// Sprache und den Modus des Dokuments der anderen Seite (sonst die Erkennung nach dem Dateinamen). Liefert die Sprache,
/// wenn Farben an sind.
fn side_highlight(side: &mut Side, docs: &mut BTreeMap<u64, DocEntry>, prefs: &super::EditorPrefs, other: Option<u64>) -> Option<String> {
    match side {
        Side::Doc(d) => {
            let e = docs.get_mut(d)?;
            e.ensure_highlight(prefs);
            e.syntax.hl.as_ref().map(|h| h.lang.clone())
        }
        Side::Text(s) => {
            let lang = match other.and_then(|o| docs.get_mut(&o)) {
                Some(e) => {
                    let on = e.mode(prefs) == crate::editor::syntax::Mode::Syntax && !e.is_large(prefs);
                    if on { e.language(prefs) } else { None }
                }
                None => {
                    let l = crate::editor::syntax::detect(s.path(), "", "", &prefs.syntax.user_map);
                    l.filter(|l| prefs.syntax.mode_for(Some(l)) == crate::editor::syntax::Mode::Syntax)
                }
            }?;
            if !crate::editor::syntax::engine::has_grammar(&lang) || s.binary || s.raw_len > prefs.large_file_bytes {
                s.hl = None;
                return None;
            }
            if s.hl.as_ref().is_none_or(|h| h.lang != lang || h.version != s.version) {
                let doc = Document::from_text(&s.text);
                s.hl = DocHighlight::new(&lang, &doc).map(|hl| StaticHl { lang: lang.clone(), version: s.version, doc, hl });
            }
            s.hl.as_ref().map(|h| h.lang.clone())
        }
    }
}

/// Syntaxfarben der Zeilen `lines` einer Seite (Kopien, damit danach frei gezeichnet werden kann).
fn prepare_hl(lang: Option<String>, side: &mut Side, docs: &mut BTreeMap<u64, DocEntry>, lines: Range<usize>, pending: &mut bool) -> Option<HashMap<usize, Vec<Span>>> {
    lang?;
    if lines.start >= lines.end {
        return None;
    }
    let (doc, hl): (&Document, &mut DocHighlight) = match side {
        Side::Doc(d) => {
            let e = docs.get_mut(d)?;
            let doc = e.doc.as_ref()?;
            (doc, e.syntax.hl.as_mut()?)
        }
        Side::Text(s) => {
            let h = s.hl.as_mut()?;
            (&h.doc, &mut h.hl)
        }
    };
    let n = doc.len_lines();
    let r = lines.start.min(n)..lines.end.min(n);
    if !hl.prepare(doc, r.clone()) {
        *pending = true;
    }
    Some(r.filter_map(|l| hl.spans(l).map(|s| (l, s.to_vec()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_split_lines_like_the_core() {
        for t in ["", "a", "a\n", "a\nb", "a\nb\n", "\n\n", "ä\nö"] {
            let s = Snap::new(Arc::new(t.to_string()));
            let l = compare::split_lines(t);
            assert_eq!(s.lines(), l, "{t:?}");
            assert_eq!(s.len(), l.lines.len());
        }
        assert_eq!(Snap::new(Arc::new("x".into())).line(7), "");
    }

    #[test]
    fn wrap_starts_break_at_spaces() {
        assert_eq!(wrap_starts("short", 40, 4), vec![0]);
        let w = wrap_starts("alpha beta gamma delta epsilon", 12, 4);
        assert_eq!(w, vec![0, 11, 23]);
        assert_eq!(wrap_starts(&"x".repeat(30), 10, 4), vec![0, 10, 20]);
    }

    #[test]
    fn jobs_compare_texts_and_report_binary_or_large_files() {
        let p = compare::Progress::default();
        let key = JobKey { l: (0, 1), r: (0, 1), ws: false };
        let side = |t: &str| SideIn::Text { text: Arc::new(t.to_string()), raw_len: t.len() as u64, raw_sha: Some(sha_hex(t.as_bytes())), binary: false };
        let o = run_job(side("a\nb\nc\n"), side("a\nB\nc\n"), false, 1 << 20, &p, key.clone()).unwrap();
        assert_eq!(o.hunks, vec![Hunk { old: 1..2, new: 1..2 }]);
        assert!(o.intra.contains_key(&(1, 1)));
        assert!(o.big.is_none());
        // zu groß
        let o = run_job(side("abc"), side("abd"), false, 2, &p, key.clone()).unwrap();
        let big = o.big.unwrap();
        assert!(!big.binary);
        assert_eq!(big.left.0, 3);
        assert_eq!(big.left.1, sha_hex(b"abc"));
        // binär
        let bin = SideIn::Text { text: Arc::new("x".into()), raw_len: 4, raw_sha: Some("aa".into()), binary: true };
        assert!(run_job(bin, side("x"), false, 1 << 20, &p, key.clone()).unwrap().big.unwrap().binary);
        // abgebrochen
        let c = compare::Progress::default();
        c.cancel.store(true, Ordering::Relaxed);
        assert!(run_job(side("a"), side("b"), false, 1 << 20, &c, key).is_none());
        assert_eq!(human_size(10, Lang::En), "10 bytes");
        assert_eq!(human_size(2048, Lang::En), "2.0 KB");
    }

    #[test]
    fn clipboard_side_has_no_format_facts() {
        let s = StaticSide::clipboard("a\r\nb\r\n");
        assert_eq!(&*s.text, "a\nb\n");
        assert!(s.facts.encoding.is_none() && s.facts.eol.is_none() && s.facts.final_newline);
        assert_eq!(s.label(Lang::En), "Clipboard");
    }
}
