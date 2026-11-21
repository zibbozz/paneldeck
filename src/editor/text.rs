//! Positions and small text helpers (no egui).
//!
//! All positions in the editor core are **char indices** into the document text (Unicode scalar values, not bytes,
//! not UTF-16 units). The text inside a [`Document`](super::Document) always uses `\n` as the only line break; the
//! file's line ending is applied when saving. Columns are char columns unless a function says "visual" (tabs expanded
//! to the next tab stop, wide East Asian characters count 2).

use ropey::{Rope, RopeSlice};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// 0-based line and char column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LineCol {
    pub line: usize,
    pub col: usize,
}

impl LineCol {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// Character classes for word movement and "select word".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharClass {
    Word,
    Space,
    Punct,
    Newline,
}

/// Letters, digits and `_` belong to words.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn char_class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if is_word_char(c) {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// Length of a line in chars without its trailing `\n`.
pub fn line_len(rope: &Rope, line: usize) -> usize {
    let s = rope.line(line);
    let n = s.len_chars();
    if n > 0 && s.char(n - 1) == '\n' { n - 1 } else { n }
}

/// The line without its trailing `\n` (no copy).
pub fn line_slice(rope: &Rope, line: usize) -> RopeSlice<'_> {
    let s = rope.line(line);
    let n = line_len(rope, line);
    s.slice(..n)
}

/// Char index of the end of `line` (before its `\n`).
pub fn line_end(rope: &Rope, line: usize) -> usize {
    rope.line_to_char(line) + line_len(rope, line)
}

/// Position → line/column.
pub fn pos_to_linecol(rope: &Rope, pos: usize) -> LineCol {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    LineCol { line, col: pos - rope.line_to_char(line) }
}

/// Line/column → position, clamped to the document and to the line length.
pub fn linecol_to_pos(rope: &Rope, lc: LineCol) -> usize {
    let line = lc.line.min(rope.len_lines().saturating_sub(1));
    rope.line_to_char(line) + lc.col.min(line_len(rope, line))
}

/// Display width of one char at visual column `vcol` (tabs go to the next tab stop, control chars count 1).
pub fn char_width(c: char, vcol: usize, tab_width: usize) -> usize {
    if c == '\t' {
        let t = tab_width.max(1);
        t - vcol % t
    } else {
        c.width().unwrap_or(1)
    }
}

/// Visual column of char column `col` in `line`.
pub fn visual_col(rope: &Rope, line: usize, col: usize, tab_width: usize) -> usize {
    let mut v = 0;
    for c in line_slice(rope, line).chars().take(col) {
        v += char_width(c, v, tab_width);
    }
    v
}

/// Char column in `line` for the visual column `vcol` (the char that covers `vcol`, rounded to the nearer edge;
/// past the end → line length).
pub fn col_for_visual(rope: &Rope, line: usize, vcol: usize, tab_width: usize) -> usize {
    let mut v = 0;
    for (i, c) in line_slice(rope, line).chars().enumerate() {
        let w = char_width(c, v, tab_width);
        if vcol < v + w {
            return if w > 1 && vcol - v > (w - 1) / 2 { i + 1 } else { i };
        }
        v += w;
    }
    line_len(rope, line)
}

/// Visual width of a whole line (tabs expanded).
pub fn line_visual_width(rope: &Rope, line: usize, tab_width: usize) -> usize {
    let mut v = 0;
    for c in line_slice(rope, line).chars() {
        v += char_width(c, v, tab_width);
    }
    v
}

/// Window size (chars) used to find grapheme boundaries without copying whole (possibly huge) lines.
const GRAPHEME_WINDOW: usize = 64;

/// Next grapheme cluster boundary after `pos` (`pos` itself at the end of the text).
pub fn next_grapheme(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    if pos >= len {
        return len;
    }
    let end = (pos + GRAPHEME_WINDOW).min(len);
    let s: String = rope.slice(pos..end).chars().collect();
    match s.grapheme_indices(true).nth(1) {
        Some((b, _)) => pos + s[..b].chars().count(),
        None => end,
    }
}

/// Previous grapheme cluster boundary before `pos` (0 at the start).
pub fn prev_grapheme(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    if pos == 0 {
        return 0;
    }
    // Start the window at a line start if it is near, so the segmentation starts at a real boundary.
    let line_start = rope.line_to_char(rope.char_to_line(pos - 1));
    let start = if pos - line_start <= GRAPHEME_WINDOW { line_start } else { pos - GRAPHEME_WINDOW };
    let s: String = rope.slice(start..pos).chars().collect();
    match s.grapheme_indices(true).last() {
        Some((b, _)) => start + s[..b].chars().count(),
        None => pos - 1,
    }
}

/// Leading whitespace (spaces/tabs) of `line` in chars.
pub fn indent_len(rope: &Rope, line: usize) -> usize {
    line_slice(rope, line).chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Leading whitespace of `line` as text.
pub fn indent_text(rope: &Rope, line: usize) -> String {
    line_slice(rope, line).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// Is the line empty or whitespace only?
pub fn is_blank_line(rope: &Rope, line: usize) -> bool {
    line_slice(rope, line).chars().all(|c| c == ' ' || c == '\t')
}

/// Range of the word at `pos` (word chars, or a run of the same class). Empty range if `pos` touches nothing.
pub fn word_range_at(rope: &Rope, pos: usize) -> std::ops::Range<usize> {
    let len = rope.len_chars();
    let pos = pos.min(len);
    let at = |i: usize| if i < len { Some(rope.char(i)) } else { None };
    // Prefer the char at pos; fall back to the char before (cursor at a word end).
    let class = match at(pos).map(char_class) {
        Some(CharClass::Word) => CharClass::Word,
        _ if pos > 0 && char_class(rope.char(pos - 1)) == CharClass::Word => CharClass::Word,
        Some(c) if c != CharClass::Newline => c,
        _ if pos > 0 && char_class(rope.char(pos - 1)) != CharClass::Newline => char_class(rope.char(pos - 1)),
        _ => return pos..pos,
    };
    let mut s = pos;
    while s > 0 && char_class(rope.char(s - 1)) == class {
        s -= 1;
    }
    let mut e = pos;
    while e < len && char_class(rope.char(e)) == class {
        e += 1;
    }
    s..e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_helpers() {
        let r = Rope::from_str("ab\n\tc\n");
        assert_eq!(r.len_lines(), 3);
        assert_eq!(line_len(&r, 0), 2);
        assert_eq!(line_len(&r, 1), 2);
        assert_eq!(line_len(&r, 2), 0);
        assert_eq!(pos_to_linecol(&r, 4), LineCol::new(1, 1));
        assert_eq!(linecol_to_pos(&r, LineCol::new(1, 99)), 5);
        assert_eq!(linecol_to_pos(&r, LineCol::new(99, 0)), 6);
        assert_eq!(visual_col(&r, 1, 1, 4), 4);
        assert_eq!(col_for_visual(&r, 1, 4, 4), 1);
        assert_eq!(col_for_visual(&r, 1, 1, 4), 0);
        assert_eq!(col_for_visual(&r, 1, 3, 4), 1);
        assert_eq!(col_for_visual(&r, 1, 50, 4), 2);
        assert_eq!(indent_text(&r, 1), "\t");
    }

    #[test]
    fn ropey_breaks_only_on_lf() {
        // Unicode line separators and lone CR must not count as line breaks (feature `unicode_lines` is off).
        let r = Rope::from_str("a\u{2028}b\rc\u{85}d");
        assert_eq!(r.len_lines(), 1);
    }

    #[test]
    fn graphemes() {
        let r = Rope::from_str("e\u{301}x👍🏽y");
        assert_eq!(next_grapheme(&r, 0), 2);
        assert_eq!(next_grapheme(&r, 2), 3);
        assert_eq!(next_grapheme(&r, 3), 5);
        assert_eq!(prev_grapheme(&r, 5), 3);
        assert_eq!(prev_grapheme(&r, 2), 0);
        assert_eq!(prev_grapheme(&r, 0), 0);
        assert_eq!(next_grapheme(&r, 6), 6);
    }

    #[test]
    fn words() {
        let r = Rope::from_str("foo_bar  (x)\nnext");
        assert_eq!(word_range_at(&r, 2), 0..7);
        assert_eq!(word_range_at(&r, 7), 0..7);
        assert_eq!(word_range_at(&r, 8), 7..9);
        assert_eq!(word_range_at(&r, 10), 10..11);
    }

    #[test]
    fn wide_chars() {
        let r = Rope::from_str("日本x");
        assert_eq!(visual_col(&r, 0, 2, 4), 4);
        assert_eq!(col_for_visual(&r, 0, 4, 4), 2);
        assert_eq!(line_visual_width(&r, 0, 4), 5);
    }
}
