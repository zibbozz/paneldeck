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
}

impl Mode {
    /// Id in `settings.json` (`text`, `syntax`).
    pub fn id(self) -> &'static str {
        match self {
            Mode::Text => "text",
            Mode::Syntax => "syntax",
        }
    }

    /// Read an id leniently: unknown values (also `smart` from newer versions) → `None`.
    pub fn from_id(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Mode::Text),
            "syntax" => Some(Mode::Syntax),
            _ => None,
        }
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
pub const MAX_PARSE_LINE: usize = 10_000;

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

/// Keeps a document's highlighter in step with its text: remembers the revision it last saw and invalidates from the
/// first changed line ([`crate::editor::Document::changed_since`]).
pub struct DocHighlight {
    pub lang: String,
    hl: Box<dyn Highlighter>,
    rev: u64,
}

impl DocHighlight {
    pub fn new(lang: &str, doc: &crate::editor::Document) -> Option<DocHighlight> {
        Some(DocHighlight { lang: lang.to_string(), hl: highlighter(lang)?, rev: doc.revision() })
    }

    /// Follow edits made since the last call (through any view, undo, agents, reload).
    pub fn sync(&mut self, doc: &crate::editor::Document) {
        if doc.revision() == self.rev {
            return;
        }
        if let Some(pos) = doc.changed_since(self.rev) {
            self.hl.invalidate_from(doc.char_to_line(pos));
        }
        self.rev = doc.revision();
    }

    pub fn prepare(&mut self, doc: &crate::editor::Document, lines: Range<usize>, deadline: Instant) -> bool {
        self.sync(doc);
        self.hl.prepare(doc.rope(), lines, deadline)
    }

    pub fn spans(&self, line: usize) -> Option<&[Span]> {
        self.hl.spans(line)
    }

    pub fn failed(&self) -> bool {
        self.hl.failed()
    }
}

#[cfg(test)]
mod tests;
