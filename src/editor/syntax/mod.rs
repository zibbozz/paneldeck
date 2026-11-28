//! Syntax mode of the editor (#312, 3.1.0): language detection and syntax colours, without egui.
//!
//! - [`Mode`]: how a document is shown – *Text* (plain) or *Syntax* (colours). *Smart* (language server) follows in a
//!   later version; settings that name it fall back to the default.
//! - [`languages`]: the built-in language table (stable ids such as `rust`, names, extensions, file names, shebang
//!   interpreters, modeline names) plus every other grammar of the bundled set (id = its scope, e.g. `source.haskell`).
//! - [`detect`]: file name / extension (with the user's own mappings first), then shebang and modeline.
//! - [`Highlighter`]: the small internal interface the editor draws with. Today there is one engine ([`engine`]:
//!   syntect with bat's grammars from two-face, plus our own grammars in `grammars/`); tree-sitter could be added later
//!   for single languages behind the same interface.
//! - [`Tok`]: the colour classes a grammar scope maps to ([`scopes`]); the UI takes the colours from the theme.

pub mod detect;
pub mod engine;
pub mod languages;
pub mod scopes;

use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::time::Instant;

pub use detect::{UserMap, detect};
pub use languages::{LangInfo, all_languages, language_name};

/// Display mode of a document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Plain text editor (default of a new file).
    #[default]
    Text,
    /// Language detected, syntax colours.
    Syntax,
    /// Syntax colours plus a language server (#367, 3.3.0): completion and diagnostics; only in trusted folders, else
    /// like `Syntax`.
    Smart,
}

impl Mode {
    /// Id in `settings.json` (`text`, `syntax`).
    pub fn id(self) -> &'static str {
        match self {
            Mode::Text => "text",
            Mode::Syntax => "syntax",
            Mode::Smart => "smart",
        }
    }

    /// Read an id leniently: unknown values (from newer versions) → `None`.
    pub fn from_id(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Mode::Text),
            "syntax" => Some(Mode::Syntax),
            "smart" => Some(Mode::Smart),
            _ => None,
        }
    }

    /// All modes for the pickers.
    pub const ALL: [Mode; 3] = [Mode::Text, Mode::Syntax, Mode::Smart];

    /// Syntax colours (Syntax and Smart).
    pub fn colours(self) -> bool {
        self != Mode::Text
    }
}

/// Colour class of a piece of text. The UI maps each class to a theme colour (`code_*` tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tok {
    Comment,
    String,
    Number,
    Constant,
    Keyword,
    Type,
    Function,
    Operator,
    Tag,
    Attribute,
    Preprocessor,
    Escape,
    Heading,
    Variable,
    Invalid,
    Inserted,
    Deleted,
}

impl Tok {
    pub const ALL: [Tok; 17] = [
        Tok::Comment,
        Tok::String,
        Tok::Number,
        Tok::Constant,
        Tok::Keyword,
        Tok::Type,
        Tok::Function,
        Tok::Operator,
        Tok::Tag,
        Tok::Attribute,
        Tok::Preprocessor,
        Tok::Escape,
        Tok::Heading,
        Tok::Variable,
        Tok::Invalid,
        Tok::Inserted,
        Tok::Deleted,
    ];

    pub fn index(self) -> usize {
        Tok::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// Coloured part of a line: char columns `start..end` (without the line break).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub tok: Tok,
}

impl Span {
    pub fn range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// Lines longer than this (chars) are not parsed: they stay plain and the parser state carries over unchanged (like
/// other editors do for minified files), so one huge line cannot stall the editor.
pub const MAX_PARSE_LINE: usize = 3_000;

/// Interface between the editor and a highlighting engine. One highlighter belongs to one document (all views/splits
/// of the document share it). The editor calls [`Highlighter::invalidate_from`] after edits and
/// [`Highlighter::prepare`] for the visible lines before drawing; [`Highlighter::spans`] then answers per line.
pub trait Highlighter: Send {
    /// Text from line `line` on has changed: results from there are stale.
    fn invalidate_from(&mut self, line: usize);
    /// Work until the lines `lines` are highlighted or `deadline` passes. `true` = all of them are ready.
    fn prepare(&mut self, rope: &Rope, lines: Range<usize>, deadline: Instant) -> bool;
    /// Spans of a prepared line (`None` = not ready, draw plain).
    fn spans(&self, line: usize) -> Option<&[Span]>;
    /// The engine gave up (grammar error); the document is drawn plain.
    fn failed(&self) -> bool {
        false
    }
}

/// Highlighter for a language id (`None` for unknown ids or plain text).
pub fn highlighter(lang: &str) -> Option<Box<dyn Highlighter>> {
    engine::SyntectHighlighter::new(lang).map(|h| Box::new(h) as Box<dyn Highlighter>)
}

/// Lines highlighted beyond the asked range (smooth scrolling).
const MARGIN: usize = 60;
/// Time slice of the worker between looks at new requests.
const SLICE: std::time::Duration = std::time::Duration::from_millis(15);

/// Request to the worker: text at revision `rev` (a rope clone is cheap), first changed line since the last request,
/// lines wanted.
struct Job {
    rev: u64,
    rope: Rope,
    invalidate: Option<usize>,
    want: Range<usize>,
}

enum Reply {
    Lines { rev: u64, lines: Vec<(usize, Box<[Span]>)> },
    Failed,
}

/// Syntax colours of one document for the editor. The parsing runs on a worker thread (one per highlighted document,
/// ends with the value), so neither compiling a grammar nor a slow line ever blocks drawing; the UI only sends the text
/// (cheap rope clone) and the wanted lines and takes the results. Follows edits through
/// [`crate::editor::Document::changed_since`]; until fresh colours arrive, lines keep their previous colours.
pub struct DocHighlight {
    pub lang: String,
    rev: u64,
    line_count: usize,
    /// Line → (revision the colours belong to, spans).
    cache: std::collections::HashMap<usize, (u64, Box<[Span]>)>,
    /// First changed line not yet sent to the worker.
    pending_invalidate: Option<usize>,
    asked: Option<(u64, Range<usize>)>,
    tx: std::sync::mpsc::Sender<Job>,
    rx: std::sync::mpsc::Receiver<Reply>,
    failed: bool,
}

impl DocHighlight {
    /// `None` for languages without a grammar.
    pub fn new(lang: &str, doc: &crate::editor::Document) -> Option<DocHighlight> {
        if !engine::has_grammar(lang) {
            return None;
        }
        let (tx, jobs) = std::sync::mpsc::channel::<Job>();
        let (out, rx) = std::sync::mpsc::channel::<Reply>();
        let l = lang.to_string();
        std::thread::Builder::new().name("pd-syntax".into()).spawn(move || worker(&l, jobs, out)).ok()?;
        Some(DocHighlight { lang: lang.to_string(), rev: doc.revision(), line_count: doc.len_lines(), cache: Default::default(), pending_invalidate: None, asked: None, tx, rx, failed: false })
    }

    /// Follow edits made since the last call (through any view, undo, agents, reload).
    fn sync(&mut self, doc: &crate::editor::Document) {
        if doc.revision() == self.rev {
            return;
        }
        if let Some(pos) = doc.changed_since(self.rev) {
            let line = doc.char_to_line(pos);
            self.pending_invalidate = Some(self.pending_invalidate.map_or(line, |p| p.min(line)));
            // Same number of lines: keep the old colours below until fresh ones arrive (no flicker); otherwise they
            // would sit on the wrong lines.
            if doc.len_lines() != self.line_count {
                self.cache.retain(|l, _| *l < line);
            }
        }
        self.rev = doc.revision();
        self.line_count = doc.len_lines();
    }

    /// Take results and ask the worker for `lines` (plus a margin). `true` = all of them have current colours.
    pub fn prepare(&mut self, doc: &crate::editor::Document, lines: Range<usize>) -> bool {
        self.sync(doc);
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Reply::Failed => {
                    self.failed = true;
                    self.cache.clear();
                }
                Reply::Lines { rev, lines } => {
                    // Results for an older text are still right above its first change since then.
                    let valid_below = if rev == self.rev { usize::MAX } else { doc.changed_since(rev).map(|p| doc.char_to_line(p)).unwrap_or(usize::MAX) };
                    for (l, s) in lines {
                        if l < valid_below {
                            self.cache.insert(l, (self.rev, s));
                        }
                    }
                }
            }
        }
        if self.failed {
            return true;
        }
        let total = doc.len_lines();
        let end = lines.end.min(total);
        let start = lines.start.min(end);
        let done = (start..end).all(|l| self.cache.get(&l).is_some_and(|(r, _)| *r == self.rev));
        if self.cache.len() > 4 * engine::STEP * 256 {
            let (a, b) = (start.saturating_sub(4 * MARGIN), end + 4 * MARGIN);
            self.cache.retain(|l, _| *l >= a && *l < b);
        }
        let want = start.saturating_sub(MARGIN)..(end + MARGIN).min(total);
        let asked = self.asked.as_ref().is_some_and(|(r, w)| *r == self.rev && w.start <= start && w.end >= end);
        if !done && !asked {
            let job = Job { rev: self.rev, rope: doc.rope().clone(), invalidate: self.pending_invalidate.take(), want: want.clone() };
            if self.tx.send(job).is_err() {
                self.failed = true;
                return true;
            }
            self.asked = Some((self.rev, want));
        }
        done
    }

    /// Colours of a line (possibly from before the last edit while fresh ones are on the way).
    pub fn spans(&self, line: usize) -> Option<&[Span]> {
        self.cache.get(&line).map(|(_, s)| &s[..])
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Wait (up to `timeout`) until `lines` have current colours – for tests and agents' sake, never in drawing.
    pub fn wait(&mut self, doc: &crate::editor::Document, lines: Range<usize>, timeout: std::time::Duration) -> bool {
        let until = Instant::now() + timeout;
        loop {
            if self.prepare(doc, lines.clone()) {
                return true;
            }
            if Instant::now() > until {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

/// The worker: one [`engine::SyntectHighlighter`], always working on the newest request, in time slices so a newer
/// request (edit, scrolling) is taken up quickly. Ends when the [`DocHighlight`] is dropped.
fn worker(lang: &str, jobs: std::sync::mpsc::Receiver<Job>, out: std::sync::mpsc::Sender<Reply>) {
    let Some(mut hl) = engine::SyntectHighlighter::new(lang) else {
        let _ = out.send(Reply::Failed);
        return;
    };
    let mut cur: Option<Job> = None;
    let mut sent: std::collections::HashSet<usize> = Default::default();
    let mut idle = true;
    loop {
        let mut take = |j: Job, cur: &mut Option<Job>, sent: &mut std::collections::HashSet<usize>| {
            if let Some(l) = j.invalidate {
                hl.invalidate_from(l);
            }
            if cur.as_ref().is_none_or(|c| c.rev != j.rev) {
                sent.clear();
            }
            *cur = Some(j);
        };
        if idle {
            match jobs.recv() {
                Ok(j) => take(j, &mut cur, &mut sent),
                Err(_) => return,
            }
        }
        loop {
            match jobs.try_recv() {
                Ok(j) => take(j, &mut cur, &mut sent),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
        let Some(job) = cur.as_ref() else {
            idle = true;
            continue;
        };
        let done = hl.prepare(&job.rope, job.want.clone(), Instant::now() + SLICE);
        if hl.failed() {
            let _ = out.send(Reply::Failed);
            return;
        }
        let lines: Vec<(usize, Box<[Span]>)> = job.want.clone().filter(|l| !sent.contains(l)).filter_map(|l| hl.spans(l).map(|s| (l, s.to_vec().into_boxed_slice()))).collect();
        sent.extend(lines.iter().map(|(l, _)| *l));
        if !lines.is_empty() && out.send(Reply::Lines { rev: job.rev, lines }).is_err() {
            return;
        }
        idle = done;
    }
}

#[cfg(test)]
mod tests;
