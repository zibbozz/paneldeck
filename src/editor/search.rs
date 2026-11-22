//! Find and replace (literal or regex, case, whole word) over the rope without copying the whole text.
//!
//! The text is searched in blocks of whole lines (about 1 MiB each). Matches never span two blocks, so a regex
//! that matches line breaks (`\n`, `\s+` …) can miss a match that crosses a block boundary; single-line matches
//! are always found. Empty matches (e.g. `^` alone) are skipped.

use super::change::{Assoc, ChangeSet};
use super::document::Document;
use super::history::{EditKind, EditOrigin};
use super::selection::{Selection, Selections};
use super::text::is_word_char;
use regex::{Regex, RegexBuilder};
use std::ops::Range;

/// Size of the search blocks in bytes.
const BLOCK_BYTES: usize = 1024 * 1024;

/// What to search for.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SearchQuery {
    pub pattern: String,
    /// `pattern` is a regular expression (Rust `regex` syntax; `^`/`$` match at line starts/ends).
    pub regex: bool,
    pub case_sensitive: bool,
    /// Only matches that are not part of a longer word.
    pub whole_word: bool,
}

impl SearchQuery {
    /// Case-sensitive literal search.
    pub fn literal(pattern: &str) -> Self {
        Self { pattern: pattern.into(), regex: false, case_sensitive: true, whole_word: false }
    }

    /// Case-sensitive regex search.
    pub fn regex(pattern: &str) -> Self {
        Self { pattern: pattern.into(), regex: true, case_sensitive: true, whole_word: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchError {
    Empty,
    /// The regex does not compile (message from the regex crate, English).
    InvalidRegex(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::Empty => write!(f, "empty search"),
            SearchError::InvalidRegex(m) => write!(f, "invalid regular expression: {m}"),
        }
    }
}

impl std::error::Error for SearchError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// A compiled search.
#[derive(Clone, Debug)]
pub struct Search {
    query: SearchQuery,
    re: Regex,
}

/// Result of [`Document::replace_next`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaceResult {
    /// The primary selection was a match and was replaced.
    pub replaced: bool,
    /// The match selected afterwards (`None` = no more matches).
    pub next: Option<Range<usize>>,
}

impl Search {
    pub fn new(query: &SearchQuery) -> Result<Self, SearchError> {
        if query.pattern.is_empty() {
            return Err(SearchError::Empty);
        }
        let pat = if query.regex { query.pattern.clone() } else { regex::escape(&query.pattern) };
        let re = RegexBuilder::new(&pat)
            .case_insensitive(!query.case_sensitive)
            .multi_line(true)
            .size_limit(64 * 1024 * 1024)
            .build()
            .map_err(|e| SearchError::InvalidRegex(e.to_string()))?;
        Ok(Self { query: query.clone(), re })
    }

    pub fn query(&self) -> &SearchQuery {
        &self.query
    }

    /// Matches overlapping `range` (e.g. the visible lines), at most `limit`.
    pub fn find_in_range(&self, doc: &Document, range: Range<usize>, limit: usize) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        let len = doc.len_chars();
        let (s, e) = (range.start.min(len), range.end.min(len));
        let first = doc.char_to_line(s);
        let last = doc.char_to_line(e);
        for lines in blocks_forward(doc, first, last + 1) {
            let go = self.for_each_in_block(doc, lines, |m| {
                if m.start > e || (s != e && m.start >= e) {
                    return false;
                }
                let hit = if s == e { m.start <= s && s < m.end } else { m.end > s };
                if hit {
                    out.push(m);
                }
                out.len() < limit
            });
            if !go {
                break;
            }
        }
        out
    }

    /// Next match starting at or after `from` (`Forward`), or the last match starting before `from` (`Backward`).
    /// With `wrap`, continue at the other end of the document.
    pub fn find_next(&self, doc: &Document, from: usize, dir: Direction, wrap: bool) -> Option<Range<usize>> {
        let len = doc.len_chars();
        let from = from.min(len);
        let total = doc.len_lines();
        let line = doc.char_to_line(from);
        match dir {
            Direction::Forward => {
                if let Some(m) = self.first_match(doc, line, total, |m| m.start >= from) {
                    return Some(m);
                }
                if wrap { self.first_match(doc, 0, line + 1, |m| m.start < from) } else { None }
            }
            Direction::Backward => {
                if let Some(m) = self.last_match(doc, 0, line + 1, |m| m.start < from) {
                    return Some(m);
                }
                if wrap { self.last_match(doc, line, total, |m| m.start >= from) } else { None }
            }
        }
    }

    /// All matches (at most `limit`); `bool` = there were more.
    pub fn find_all(&self, doc: &Document, limit: usize) -> (Vec<Range<usize>>, bool) {
        let mut out = Vec::new();
        let mut more = false;
        for lines in blocks_forward(doc, 0, doc.len_lines()) {
            let go = self.for_each_in_block(doc, lines, |m| {
                if out.len() >= limit {
                    more = true;
                    return false;
                }
                out.push(m);
                true
            });
            if !go {
                break;
            }
        }
        (out, more)
    }

    /// Number of matches, counting up to `limit` (`bool` = limit reached).
    pub fn count(&self, doc: &Document, limit: usize) -> (usize, bool) {
        let mut n = 0;
        let mut capped = false;
        for lines in blocks_forward(doc, 0, doc.len_lines()) {
            let go = self.for_each_in_block(doc, lines, |_| {
                if n >= limit {
                    capped = true;
                    return false;
                }
                n += 1;
                true
            });
            if !go {
                break;
            }
        }
        (n, capped)
    }

    /// Is `range` exactly a match?
    pub fn is_match(&self, doc: &Document, range: &Range<usize>) -> bool {
        if range.is_empty() {
            return false;
        }
        self.find_in_range(doc, range.clone(), 64).iter().any(|m| m == range)
    }

    /// Text that replaces the match `range` (regex: `$1`, `${name}`, `$$`, and `\n`, `\t`, `\\` in `replacement`).
    pub fn replacement_for(&self, doc: &Document, range: &Range<usize>, replacement: &str) -> String {
        if !self.query.regex {
            return replacement.to_string();
        }
        let first = doc.char_to_line(range.start);
        let last = doc.char_to_line(range.end);
        let start_char = doc.line_to_char(first);
        let end_char = line_end_incl(doc, last + 1);
        let block = doc.slice_string(start_char..end_char);
        let base = doc.char_to_byte(start_char);
        let at = doc.char_to_byte(range.start) - base;
        let repl = unescape_replacement(replacement);
        match self.re.captures_at(&block, at) {
            Some(c) if c.get(0).is_some_and(|m| m.start() == at) => {
                let mut dst = String::new();
                c.expand(&repl, &mut dst);
                dst
            }
            _ => repl,
        }
    }

    // ---- internals ----

    /// Run `f` for every (filtered) match in the block of `lines`; `f` returns `false` to stop. Returns `false` if
    /// stopped.
    fn for_each_in_block(&self, doc: &Document, lines: Range<usize>, mut f: impl FnMut(Range<usize>) -> bool) -> bool {
        let start_char = doc.line_to_char(lines.start);
        let end_char = line_end_incl(doc, lines.end);
        if end_char <= start_char {
            return true;
        }
        let block = doc.slice_string(start_char..end_char);
        let base = doc.char_to_byte(start_char);
        for m in self.re.find_iter(&block) {
            if m.start() == m.end() {
                continue;
            }
            let s = doc.byte_to_char(base + m.start());
            let e = doc.byte_to_char(base + m.end());
            if self.query.whole_word && !self.word_bounded(doc, s, e) {
                continue;
            }
            if !f(s..e) {
                return false;
            }
        }
        true
    }

    fn word_bounded(&self, doc: &Document, s: usize, e: usize) -> bool {
        let first = doc.char_at(s).is_some_and(is_word_char);
        let last = e > s && doc.char_at(e - 1).is_some_and(is_word_char);
        let before = s > 0 && doc.char_at(s - 1).is_some_and(is_word_char);
        let after = doc.char_at(e).is_some_and(is_word_char);
        !(first && before) && !(last && after)
    }

    fn first_match(&self, doc: &Document, from_line: usize, to_line: usize, ok: impl Fn(&Range<usize>) -> bool) -> Option<Range<usize>> {
        for lines in blocks_forward(doc, from_line, to_line) {
            let mut found = None;
            self.for_each_in_block(doc, lines, |m| {
                if ok(&m) {
                    found = Some(m);
                    false
                } else {
                    true
                }
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn last_match(&self, doc: &Document, from_line: usize, to_line: usize, ok: impl Fn(&Range<usize>) -> bool) -> Option<Range<usize>> {
        for lines in blocks_backward(doc, from_line, to_line) {
            let mut found = None;
            self.for_each_in_block(doc, lines, |m| {
                if ok(&m) {
                    found = Some(m);
                }
                true
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }
}

/// Char index of the start of `line` (or the end of the text for `line >= len_lines`).
fn line_end_incl(doc: &Document, line: usize) -> usize {
    if line >= doc.len_lines() { doc.len_chars() } else { doc.rope().line_to_char(line) }
}

/// Line blocks `[a, b)` covering `from..to` (lines), about `BLOCK_BYTES` each, front to back.
fn blocks_forward(doc: &Document, from: usize, to: usize) -> impl Iterator<Item = Range<usize>> + '_ {
    let to = to.min(doc.len_lines());
    let mut a = from.min(to);
    std::iter::from_fn(move || {
        if a >= to {
            return None;
        }
        let r = doc.rope();
        let target = r.line_to_byte(a) + BLOCK_BYTES;
        let b = if target >= r.len_bytes() { to } else { (r.byte_to_line(target) + 1).clamp(a + 1, to) };
        let out = a..b;
        a = b;
        Some(out)
    })
}

/// Same blocks, back to front.
fn blocks_backward(doc: &Document, from: usize, to: usize) -> impl Iterator<Item = Range<usize>> + '_ {
    let to = to.min(doc.len_lines());
    let from = from.min(to);
    let mut b = to;
    std::iter::from_fn(move || {
        if b <= from {
            return None;
        }
        let r = doc.rope();
        let end_byte = if b >= r.len_lines() { r.len_bytes() } else { r.line_to_byte(b) };
        let a = if end_byte <= BLOCK_BYTES { from } else { r.byte_to_line(end_byte - BLOCK_BYTES).clamp(from, b - 1) };
        let out = a..b;
        b = a;
        Some(out)
    })
}

/// `\n`, `\t`, `\\` in a regex replacement.
fn unescape_replacement(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.peek() {
                Some('n') => {
                    out.push('\n');
                    it.next();
                }
                Some('t') => {
                    out.push('\t');
                    it.next();
                }
                Some('\\') => {
                    out.push('\\');
                    it.next();
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Document {
    /// Replace all matches (optionally only inside `within`) as one undo step. Returns the number replaced.
    pub fn replace_all(
        &mut self,
        sels: &mut Selections,
        search: &Search,
        replacement: &str,
        within: Option<Range<usize>>,
        origin: EditOrigin,
    ) -> usize {
        self.sync(sels);
        let matches = match &within {
            Some(r) => search
                .find_in_range(self, r.clone(), usize::MAX)
                .into_iter()
                .filter(|m| m.start >= r.start && m.end <= r.end)
                .collect(),
            None => search.find_all(self, usize::MAX).0,
        };
        if matches.is_empty() {
            return 0;
        }
        let n = matches.len();
        let edits: Vec<(Range<usize>, String)> = if search.query().regex {
            matches.into_iter().map(|m| {
                let t = search.replacement_for(self, &m, replacement);
                (m, t)
            }).collect()
        } else {
            matches.into_iter().map(|m| (m, replacement.to_string())).collect()
        };
        let edits = edits.into_iter().map(|(r, t)| (r, super::encoding::normalize_eol(&t).0)).collect();
        let Ok(cs) = ChangeSet::from_edits(self.rope(), edits) else { return 0 };
        self.commit_mapped(sels, cs, origin, EditKind::Other, Assoc::Before, Assoc::After);
        n
    }

    /// "Replace" button: if the primary selection is a match, replace it; then select the next match.
    pub fn replace_next(&mut self, sels: &mut Selections, search: &Search, replacement: &str, origin: EditOrigin) -> ReplaceResult {
        self.sync(sels);
        let p = sels.primary();
        let mut replaced = false;
        let mut from = p.end();
        if search.is_match(self, &p.range()) {
            let text = super::encoding::normalize_eol(&search.replacement_for(self, &p.range(), replacement)).0;
            let len = text.chars().count();
            if let Ok(cs) = ChangeSet::from_edits(self.rope(), vec![(p.range(), text)]) {
                let start = p.start();
                let after = Selections::cursor(start + len);
                self.commit(sels, cs, after, origin, EditKind::Other);
                replaced = true;
                from = start + len;
            }
        }
        let next = search.find_next(self, from, Direction::Forward, true);
        if let Some(m) = &next {
            *sels = self.new_selections(m.start);
            sels.set_primary(Selection::new(m.start, m.end));
        }
        ReplaceResult { replaced, next }
    }

    /// Select every match as its own selection (Alt+Enter in the find bar). Returns the count.
    pub fn select_all_matches(&self, sels: &mut Selections, search: &Search, limit: usize) -> usize {
        let (all, _) = search.find_all(self, limit);
        if all.is_empty() {
            return 0;
        }
        let n = all.len();
        let ranges: Vec<Selection> = all.into_iter().map(|m| Selection::new(m.start, m.end)).collect();
        sels.replace(ranges, 0);
        self.sync(sels);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(t: &str) -> Document {
        Document::from_text(t)
    }

    #[test]
    fn literal_case_and_whole_word() {
        let d = doc("Foo foo food\nfoo_bar FOO");
        let mut q = SearchQuery::literal("foo");
        let s = Search::new(&q).unwrap();
        assert_eq!(s.find_all(&d, 100).0, vec![4..7, 8..11, 13..16]);
        q.case_sensitive = false;
        assert_eq!(Search::new(&q).unwrap().count(&d, 100), (5, false));
        q.whole_word = true;
        assert_eq!(Search::new(&q).unwrap().find_all(&d, 100).0, vec![0..3, 4..7, 21..24]);
        assert_eq!(Search::new(&q).unwrap().count(&d, 2), (2, true));
        // Pattern with special chars is literal.
        let d2 = doc("a.b a+b (x)");
        assert_eq!(Search::new(&SearchQuery::literal("(x)")).unwrap().find_all(&d2, 10).0, vec![8..11]);
        assert_eq!(Search::new(&SearchQuery::literal("")).unwrap_err(), SearchError::Empty);
    }

    #[test]
    fn regex_find_next_wraps_both_ways() {
        let d = doc("x1 y22\nz333\nw4");
        let s = Search::new(&SearchQuery::regex(r"\d+")).unwrap();
        assert_eq!(s.find_next(&d, 0, Direction::Forward, false), Some(1..2));
        assert_eq!(s.find_next(&d, 2, Direction::Forward, false), Some(4..6));
        assert_eq!(s.find_next(&d, 14, Direction::Forward, false), None);
        assert_eq!(s.find_next(&d, 14, Direction::Forward, true), Some(1..2));
        assert_eq!(s.find_next(&d, 4, Direction::Backward, false), Some(1..2));
        assert_eq!(s.find_next(&d, 1, Direction::Backward, false), None);
        assert_eq!(s.find_next(&d, 1, Direction::Backward, true), Some(13..14));
        let anchored = Search::new(&SearchQuery::regex(r"^\w")).unwrap();
        assert_eq!(anchored.find_all(&d, 10).0, vec![0..1, 7..8, 12..13]);
        assert!(matches!(Search::new(&SearchQuery::regex("(")), Err(SearchError::InvalidRegex(_))));
    }

    #[test]
    fn incremental_range_api() {
        let text: String = (0..100).map(|i| format!("line {i} needle\n")).collect();
        let d = doc(&text);
        let s = Search::new(&SearchQuery::literal("needle")).unwrap();
        let from = d.line_to_char(10);
        let to = d.line_to_char(20);
        let m = s.find_in_range(&d, from..to, 1000);
        assert_eq!(m.len(), 10);
        assert_eq!(d.char_to_line(m[0].start), 10);
        assert_eq!(s.find_in_range(&d, from..to, 3).len(), 3);
    }

    #[test]
    fn regex_replace_all_with_groups_is_one_undo_step() {
        let mut d = doc("let a = 1;\nlet bb = 22;\n");
        let mut sels = d.new_selections(0);
        let s = Search::new(&SearchQuery::regex(r"let (\w+) = (\d+);")).unwrap();
        assert_eq!(d.replace_all(&mut sels, &s, "const $1: i32 = $2;\\t// ${2}", None, EditOrigin::User), 2);
        assert_eq!(d.text(), "const a: i32 = 1;\t// 1\nconst bb: i32 = 22;\t// 22\n");
        d.undo(&mut sels);
        assert_eq!(d.text(), "let a = 1;\nlet bb = 22;\n");
        // Literal replacement does not expand `$1`.
        let lit = Search::new(&SearchQuery::literal("a")).unwrap();
        assert_eq!(d.replace_all(&mut sels, &lit, "$1", None, EditOrigin::User), 1);
        assert_eq!(d.text(), "let $1 = 1;\nlet bb = 22;\n");
        // Within a range only.
        let mut d = doc("aaa\naaa\n");
        let mut sels = d.new_selections(0);
        assert_eq!(d.replace_all(&mut sels, &lit, "b", Some(4..6), EditOrigin::User), 2);
        assert_eq!(d.text(), "aaa\nbba\n");
    }

    #[test]
    fn replace_next_steps_through() {
        let mut d = doc("cat cat cat");
        let s = Search::new(&SearchQuery::literal("cat")).unwrap();
        let mut sels = d.new_selections(0);
        let r = d.replace_next(&mut sels, &s, "dog", EditOrigin::User);
        assert_eq!(r, ReplaceResult { replaced: false, next: Some(0..3) });
        let r = d.replace_next(&mut sels, &s, "dog", EditOrigin::User);
        assert_eq!(r, ReplaceResult { replaced: true, next: Some(4..7) });
        assert_eq!(d.text(), "dog cat cat");
        assert_eq!(sels.primary().range(), 4..7);
        d.replace_next(&mut sels, &s, "dog", EditOrigin::User);
        let r = d.replace_next(&mut sels, &s, "dog", EditOrigin::User);
        assert_eq!(r, ReplaceResult { replaced: true, next: None });
        assert_eq!(d.text(), "dog dog dog");
    }

    #[test]
    fn select_all_matches_makes_cursors() {
        let d = doc("x y x y x");
        let mut sels = d.new_selections(0);
        let s = Search::new(&SearchQuery::literal("x")).unwrap();
        assert_eq!(d.select_all_matches(&mut sels, &s, 100), 3);
        assert_eq!(sels.len(), 3);
    }

    #[test]
    fn blocks_cover_big_text() {
        // Many lines → several blocks; matches near block borders are found exactly once.
        let line = "abcdefghij needle klmnopqrstuvwxyz 0123456789\n";
        let n = (3 * BLOCK_BYTES) / line.len() + 7;
        let d = doc(&line.repeat(n));
        let s = Search::new(&SearchQuery::literal("needle")).unwrap();
        assert_eq!(s.count(&d, usize::MAX), (n, false));
        let last = s.find_next(&d, d.len_chars(), Direction::Backward, false).unwrap();
        assert_eq!(d.char_to_line(last.start), n - 1);
        let first_after = s.find_next(&d, d.line_to_char(n / 2) + 12, Direction::Forward, false).unwrap();
        assert_eq!(d.char_to_line(first_after.start), n / 2 + 1);
        let back = s.find_next(&d, d.line_to_char(n / 2), Direction::Backward, false).unwrap();
        assert_eq!(d.char_to_line(back.start), n / 2 - 1);
    }
}
