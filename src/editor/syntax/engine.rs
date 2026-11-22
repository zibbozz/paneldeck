//! Highlighting engine of the Syntax mode (#312): syntect with bat's grammars (crate `two-face`, pure-Rust regexes via
//! `fancy-regex`, no Oniguruma) plus our own small grammars for languages bat does not cover (`grammars/`).
//!
//! Both grammar sets are loaded lazily on first use (bat's set is a ready-made dump of about 1 MB, ~5 ms; ours are a
//! few kB of YAML). The grammars' regexes compile on demand while parsing.
//!
//! Incremental highlighting: the parser state is kept at a checkpoint every [`STEP`] lines; colours are only computed
//! for the lines that are drawn (plus a few around them), within a time budget per frame, and an edit only invalidates
//! from its first changed line on.

use super::languages::{Grammar, LANGUAGES, language};
use super::scopes::{is_key_scope, tok_for_scope};
use super::{Highlighter, MAX_PARSE_LINE, Span, Tok};
use ropey::Rope;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;
use std::time::Instant;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet, SyntaxSetBuilder};

/// Our own grammars (MIT, part of Paneldeck): name → YAML.
pub const OWN_GRAMMARS: &[(&str, &str)] = &[
    ("AutoIt", include_str!("grammars/autoit.sublime-syntax")),
    ("Inno Setup", include_str!("grammars/innosetup.sublime-syntax")),
    ("Visual Basic", include_str!("grammars/visualbasic.sublime-syntax")),
    ("PowerShell", include_str!("grammars/powershell.sublime-syntax")),
];

/// Parser state is kept for every `STEP`-th line.
pub const STEP: usize = 16;
/// Ready lines kept per document before the cache is trimmed to the area around the view.
const CACHE_MAX: usize = 8192;

struct Registry {
    bat: SyntaxSet,
    own: SyntaxSet,
    /// Scope name → index of the visible grammars of bat's set (ids of the grammars outside the table).
    bat_scopes: HashMap<String, usize>,
}

fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(|| {
        let bat = two_face::syntax::extra_newlines();
        let bat_scopes = bat.syntaxes().iter().enumerate().filter(|(_, s)| !s.hidden && s.name != "Plain Text").map(|(i, s)| (s.scope.build_string(), i)).collect();
        Registry { bat, own: build_own().unwrap_or_default(), bat_scopes }
    })
}

/// Build the set of our own grammars. Errors name the grammar (tests make sure there are none).
pub fn build_own() -> Result<SyntaxSet, String> {
    let mut b = SyntaxSetBuilder::new();
    for (name, yaml) in OWN_GRAMMARS {
        let def = SyntaxDefinition::load_from_str(yaml, true, Some(name)).map_err(|e| format!("{name}: {e}"))?;
        b.add(def);
    }
    Ok(b.build())
}

/// Visible grammars of bat's set (for the language picker): not hidden, with file extensions, not plain text.
pub fn bat_syntaxes() -> impl Iterator<Item = &'static SyntaxReference> {
    registry().bat.syntaxes().iter().filter(|s| !s.hidden && !s.file_extensions.is_empty() && s.name != "Plain Text")
}

/// Language id for a grammar of bat's set: the table id if the table uses it, else its scope name.
fn bat_id(s: &SyntaxReference) -> String {
    LANGUAGES
        .iter()
        .find(|l| matches!(l.grammar, Grammar::Bat(n) if n == s.name))
        .map(|l| l.id.to_string())
        .unwrap_or_else(|| s.scope.build_string())
}

/// Language by a file name or extension in bat's set (`Makefile.am`, `go.mod`, `hs`).
pub fn bat_by_extension(ext: &str) -> Option<String> {
    let s = registry().bat.find_syntax_by_extension(ext)?;
    (!s.hidden && s.name != "Plain Text").then(|| bat_id(s))
}

/// Language by the first-line patterns of bat's grammars (`<?xml`, `<?php`, shebangs).
pub fn bat_by_first_line(first: &str) -> Option<String> {
    let first: String = first.chars().take(500).collect();
    if first.trim().is_empty() {
        return None;
    }
    let s = registry().bat.find_syntax_by_first_line(&first)?;
    (!s.hidden && s.name != "Plain Text").then(|| bat_id(s))
}

/// Grammar of a language id: (set, index in the set, variables with sigil). `None` = plain text or unknown.
fn resolve(id: &str) -> Option<(&'static SyntaxSet, usize, bool)> {
    let reg = registry();
    let find = |set: &'static SyntaxSet, f: &dyn Fn(&SyntaxReference) -> bool| set.syntaxes().iter().position(f).map(|i| (set, i));
    if let Some(l) = language(id) {
        let (set, i) = match l.grammar {
            Grammar::Bat("Plain Text") => return None,
            Grammar::Bat(n) => find(&reg.bat, &|s| s.name == n)?,
            Grammar::Own(n) => find(&reg.own, &|s| s.name == n)?,
        };
        return Some((set, i, l.sigil_vars));
    }
    reg.bat_scopes.get(id).map(|i| (&reg.bat, *i, false))
}

/// Does a grammar exist for this language id (and is it not plain text)?
pub fn has_grammar(id: &str) -> bool {
    resolve(id).is_some()
}

/// Colour class and "key" flag of a scope, memoised per highlighter.
#[derive(Clone, Copy)]
struct ScopeInfo {
    tok: Option<Tok>,
    key: bool,
}

/// Incremental highlighter for one document (see module docs).
pub struct SyntectHighlighter {
    set: &'static SyntaxSet,
    sigil: bool,
    /// `checkpoints[k]` = parser state before line `k * STEP`; `[0]` always exists.
    checkpoints: Vec<(ParseState, ScopeStack)>,
    /// Highlighted lines.
    lines: HashMap<usize, Box<[Span]>>,
    memo: HashMap<Scope, ScopeInfo>,
    buf: String,
    failed: bool,
}

impl SyntectHighlighter {
    pub fn new(lang: &str) -> Option<SyntectHighlighter> {
        let (set, syntax, sigil) = resolve(lang)?;
        let start = ParseState::new(&set.syntaxes()[syntax]);
        Some(SyntectHighlighter { set, sigil, checkpoints: vec![(start, ScopeStack::new())], lines: HashMap::new(), memo: HashMap::new(), buf: String::new(), failed: false })
    }

    /// Number of stored parser states (for tests).
    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    fn info(&mut self, s: Scope) -> ScopeInfo {
        if let Some(i) = self.memo.get(&s) {
            return *i;
        }
        let name = s.build_string();
        let i = ScopeInfo { tok: tok_for_scope(&name, self.sigil), key: is_key_scope(&name) };
        self.memo.insert(s, i);
        i
    }

    /// Colour class at the top of the scope stack: the innermost scope with a class; a plain string inside a key
    /// scope (JSON/YAML keys) is a key.
    fn tok_of(&mut self, stack: &ScopeStack) -> Option<Tok> {
        let mut first = None;
        let mut found = false;
        let mut key = false;
        for s in stack.as_slice().iter().rev() {
            let i = self.info(*s);
            if !found && i.tok.is_some() {
                first = i.tok;
                found = true;
            }
            key |= i.key;
        }
        if key && matches!(first, None | Some(Tok::String)) { Some(Tok::Attribute) } else { first }
    }

    /// Parse one line (advancing the state) and return its spans.
    fn parse(&mut self, rope: &Rope, line: usize, ps: &mut ParseState, stack: &mut ScopeStack) -> Result<Box<[Span]>, ()> {
        let len = crate::editor::text::line_len(rope, line);
        if len > MAX_PARSE_LINE {
            return Ok(Box::new([]));
        }
        let mut buf = std::mem::take(&mut self.buf);
        buf.clear();
        for c in crate::editor::text::line_slice(rope, line).chunks() {
            buf.push_str(c);
        }
        let text_len = buf.len();
        buf.push('\n');
        let ops = ps.parse_line(&buf, self.set).map_err(|_| ())?;
        // Byte offset → char column (identity for ASCII lines).
        let ascii = buf.is_ascii();
        let mut col_of: Vec<u32> = Vec::new();
        if !ascii {
            col_of.resize(buf.len() + 1, 0);
            let mut c = 0u32;
            for (b, ch) in buf.char_indices() {
                for k in 0..ch.len_utf8() {
                    col_of[b + k] = c;
                }
                c += 1;
            }
            col_of[buf.len()] = c;
        }
        let to_col = |b: usize| if ascii { b as u32 } else { col_of[b.min(buf.len())] };
        let mut spans: Vec<Span> = Vec::new();
        let mut push = |a: usize, b: usize, tok: Option<Tok>| {
            let (a, b) = (a.min(text_len), b.min(text_len));
            let Some(tok) = tok else { return };
            if b <= a {
                return;
            }
            let (s, e) = (to_col(a), to_col(b));
            match spans.last_mut() {
                Some(last) if last.tok == tok && last.end == s => last.end = e,
                _ => spans.push(Span { start: s, end: e, tok }),
            }
        };
        let mut pos = 0usize;
        let mut cur = self.tok_of(stack);
        for (i, op) in ops {
            if i > pos {
                push(pos, i, cur);
                pos = i;
            }
            stack.apply(&op).map_err(|_| ())?;
            cur = self.tok_of(stack);
        }
        push(pos, text_len, cur);
        self.buf = buf;
        Ok(spans.into_boxed_slice())
    }
}

impl Highlighter for SyntectHighlighter {
    fn invalidate_from(&mut self, line: usize) {
        self.checkpoints.truncate((line / STEP + 1).max(1));
        self.lines.retain(|l, _| *l < line);
    }

    fn prepare(&mut self, rope: &Rope, lines: Range<usize>, deadline: Instant) -> bool {
        if self.failed {
            return true;
        }
        let total = rope.len_lines();
        let end = lines.end.min(total);
        let start = lines.start.min(end);
        let Some(first_missing) = (start..end).find(|l| !self.lines.contains_key(l)) else { return true };
        if self.lines.len() > CACHE_MAX {
            self.lines.retain(|l, _| *l + 2 * STEP >= start && *l < end + 2 * STEP);
        }
        let k = (first_missing / STEP).min(self.checkpoints.len() - 1);
        let (mut ps, mut stack) = self.checkpoints[k].clone();
        let mut line = k * STEP;
        let keep_from = start.saturating_sub(2 * STEP);
        while line < end {
            if line.is_multiple_of(STEP) && line / STEP == self.checkpoints.len() {
                self.checkpoints.push((ps.clone(), stack.clone()));
            }
            match self.parse(rope, line, &mut ps, &mut stack) {
                Ok(spans) => {
                    if line >= keep_from {
                        self.lines.insert(line, spans);
                    }
                }
                Err(()) => {
                    self.failed = true;
                    self.lines.clear();
                    return true;
                }
            }
            line += 1;
            if line < end && Instant::now() >= deadline {
                return false;
            }
        }
        true
    }

    fn spans(&self, line: usize) -> Option<&[Span]> {
        if self.failed {
            return None;
        }
        self.lines.get(&line).map(|s| &s[..])
    }

    fn failed(&self) -> bool {
        self.failed
    }
}
