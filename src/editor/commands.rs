//! Editing commands that work on every selection of a view (multi-cursor): typing, deleting, newline with
//! auto-indent, indent/outdent, line operations, clipboard, cursor creation, go to line.

use super::change::{Assoc, ChangeSet};
use super::document::Document;
use super::encoding::normalize_eol;
use super::history::{EditKind, EditOrigin};
use super::indent::IndentStyle;
use super::search::{Direction, Search, SearchQuery};
use super::selection::{Selection, Selections};
use super::text::{self, next_grapheme, prev_grapheme};
use std::ops::Range;

/// Text for the clipboard from [`Document::copy`] / [`Document::cut`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardText {
    /// All parts joined with `\n` (convert to CRLF for the system clipboard if wanted).
    pub text: String,
    /// One part per selection (for pasting back into the same number of cursors).
    pub parts: Vec<String>,
    /// Whole lines were copied (empty selections); paste then inserts above the cursor's line.
    pub whole_lines: bool,
}

/// One replacement made for a selection; `anchor`/`head` are offsets from the start of the replacement in the new
/// text (may point past the inserted text into the following original text).
struct SelEdit {
    range: Range<usize>,
    text: String,
    anchor: usize,
    head: usize,
}

impl SelEdit {
    fn replace(range: Range<usize>, text: String) -> Self {
        let n = text.chars().count();
        Self { range, text, anchor: n, head: n }
    }

    fn delete(range: Range<usize>) -> Self {
        Self { range, text: String::new(), anchor: 0, head: 0 }
    }
}

/// Characters that open a block (newline between them and their partner indents).
const OPENERS: [(char, char); 3] = [('{', '}'), ('[', ']'), ('(', ')')];

impl Document {
    /// Apply one optional edit per selection (index = selection index) as one undo step.
    fn apply_sel_edits(&mut self, sels: &mut Selections, edits: Vec<Option<SelEdit>>, origin: EditOrigin, kind: EditKind) -> bool {
        let mut list: Vec<(usize, SelEdit)> = edits.into_iter().enumerate().filter_map(|(i, e)| e.map(|e| (i, e))).collect();
        list.sort_by_key(|(_, e)| (e.range.start, e.range.end));
        let mut kept: Vec<(usize, SelEdit)> = Vec::with_capacity(list.len());
        for (i, e) in list {
            if let Some((_, last)) = kept.last_mut() {
                let same_insert = e.range.is_empty() && last.range.is_empty() && e.range.start == last.range.start;
                if e.range.start < last.range.end || same_insert {
                    if e.text.is_empty() && last.text.is_empty() {
                        last.range = last.range.start.min(e.range.start)..last.range.end.max(e.range.end);
                    }
                    continue;
                }
            }
            kept.push((i, e));
        }
        let Ok(cs) = ChangeSet::from_edits(self.rope(), kept.iter().map(|(_, e)| (e.range.clone(), e.text.clone())).collect()) else {
            return false;
        };
        if cs.is_empty() {
            return false;
        }
        let mut by_sel: Vec<Option<&SelEdit>> = vec![None; sels.len()];
        for (i, e) in &kept {
            by_sel[*i] = Some(e);
        }
        let after: Vec<Selection> = sels
            .iter()
            .zip(by_sel)
            .map(|(s, e)| match e {
                Some(e) => {
                    let base = cs.map_pos(e.range.start, Assoc::Before);
                    Selection { anchor: base + e.anchor, head: base + e.head, goal: None }
                }
                None => {
                    let a = cs.map_pos(s.anchor, Assoc::After);
                    let h = cs.map_pos(s.head, Assoc::After);
                    Selection { anchor: a, head: h, goal: s.goal }
                }
            })
            .collect();
        let after = Selections::from_vec(after, sels.primary_index());
        self.commit(sels, cs, after, origin, kind)
    }

    /// Typed text at every cursor (replaces selections). Consecutive typing merges into one undo step.
    pub fn type_text(&mut self, sels: &mut Selections, text: &str, origin: EditOrigin) -> bool {
        self.sync(sels);
        let text = normalize_eol(text).0;
        if text.is_empty() {
            return false;
        }
        let kind = if text.contains('\n') { EditKind::Other } else { EditKind::Typing };
        let edits = sels.iter().map(|s| Some(SelEdit::replace(s.range(), text.clone()))).collect();
        self.apply_sel_edits(sels, edits, origin, kind)
    }

    /// Insert text at every cursor as its own undo step (no merging; e.g. snippets, drop, agent).
    pub fn insert_text(&mut self, sels: &mut Selections, text: &str, origin: EditOrigin) -> bool {
        self.sync(sels);
        let text = normalize_eol(text).0;
        let edits = sels.iter().map(|s| Some(SelEdit::replace(s.range(), text.clone()))).collect();
        self.apply_sel_edits(sels, edits, origin, EditKind::Other)
    }

    /// Insert a different text at each cursor (`texts.len()` must equal the number of selections, else the texts are
    /// joined with `\n` and inserted everywhere).
    pub fn insert_per_selection(&mut self, sels: &mut Selections, texts: &[String], origin: EditOrigin) -> bool {
        self.sync(sels);
        if texts.len() != sels.len() {
            return self.insert_text(sels, &texts.join("\n"), origin);
        }
        let edits = sels.iter().zip(texts).map(|(s, t)| Some(SelEdit::replace(s.range(), normalize_eol(t).0))).collect();
        self.apply_sel_edits(sels, edits, origin, EditKind::Other)
    }

    /// Paste. With several cursors and as many lines (or `parts`) as cursors, each cursor gets one; `whole_lines`
    /// text (from copying without selection) goes above the cursor's line.
    pub fn paste(&mut self, sels: &mut Selections, clip: &ClipboardText, origin: EditOrigin) -> bool {
        self.sync(sels);
        let text = normalize_eol(&clip.text).0;
        if sels.is_multi() {
            if clip.parts.len() == sels.len() {
                return self.insert_per_selection(sels, &clip.parts, origin);
            }
            let body = if clip.whole_lines { text.strip_suffix('\n').unwrap_or(&text) } else { &text };
            let lines: Vec<String> = body.split('\n').map(str::to_string).collect();
            if lines.len() == sels.len() {
                return self.insert_per_selection(sels, &lines, origin);
            }
        }
        if clip.whole_lines && sels.iter().all(|s| s.is_empty()) {
            let n = text.chars().count();
            let mut seen_lines = std::collections::HashSet::new();
            let edits = sels
                .iter()
                .map(|s| {
                    let line = self.char_to_line(s.head);
                    if !seen_lines.insert(line) {
                        return None;
                    }
                    let ls = self.line_to_char(line);
                    let col = s.head - ls;
                    Some(SelEdit { range: ls..ls, text: text.clone(), anchor: n + col, head: n + col })
                })
                .collect();
            return self.apply_sel_edits(sels, edits, origin, EditKind::Other);
        }
        self.insert_text(sels, &text, origin)
    }

    /// Backspace: deletes the selection, else the grapheme before the cursor; in leading spaces back to the previous
    /// indentation stop.
    pub fn backspace(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let rope = self.rope();
        let unit = match self.indent_settings().style {
            IndentStyle::Spaces(n) => Some(n.max(1) as usize),
            IndentStyle::Tabs => None,
        };
        let edits = sels
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    return Some(SelEdit::delete(s.range()));
                }
                let pos = s.head;
                if pos == 0 {
                    return None;
                }
                let line = rope.char_to_line(pos);
                let ls = rope.line_to_char(line);
                let col = pos - ls;
                if let Some(n) = unit
                    && col > 0
                    && rope.slice(ls..pos).chars().all(|c| c == ' ')
                {
                    let k = (col - 1) % n + 1;
                    return Some(SelEdit::delete(pos - k..pos));
                }
                Some(SelEdit::delete(prev_grapheme(rope, pos)..pos))
            })
            .collect();
        self.apply_sel_edits(sels, edits, origin, EditKind::DeleteBack)
    }

    /// Delete key: deletes the selection, else the grapheme after the cursor.
    pub fn delete_forward(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let rope = self.rope();
        let len = self.len_chars();
        let edits = sels
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    Some(SelEdit::delete(s.range()))
                } else if s.head < len {
                    Some(SelEdit::delete(s.head..next_grapheme(rope, s.head)))
                } else {
                    None
                }
            })
            .collect();
        self.apply_sel_edits(sels, edits, origin, EditKind::DeleteForward)
    }

    /// Ctrl+Backspace: delete to the start of the word (or the selection).
    pub fn delete_word_left(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let edits = sels
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    return Some(SelEdit::delete(s.range()));
                }
                let t = self.word_left(s.head);
                (t < s.head).then(|| SelEdit::delete(t..s.head))
            })
            .collect();
        self.apply_sel_edits(sels, edits, origin, EditKind::Other)
    }

    /// Ctrl+Delete: delete to the end of the word (or the selection).
    pub fn delete_word_right(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let edits = sels
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    return Some(SelEdit::delete(s.range()));
                }
                let t = self.word_right(s.head);
                (t > s.head).then(|| SelEdit::delete(s.head..t))
            })
            .collect();
        self.apply_sel_edits(sels, edits, origin, EditKind::Other)
    }

    /// Enter: line break at every cursor, keeping the line's indentation; after `{`, `[`, `(` one level more, and
    /// if the partner follows directly it moves to its own line.
    pub fn newline(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let rope = self.rope();
        let unit = self.indent_settings().style.unit();
        let edits = sels
            .iter()
            .map(|s| {
                let (start, end) = (s.start(), s.end());
                let line = rope.char_to_line(start);
                let ls = rope.line_to_char(line);
                let ind_len = text::indent_len(rope, line).min(start - ls);
                let indent: String = rope.slice(ls..ls + ind_len).chars().collect();
                let before = (ls..start).rev().map(|k| rope.char(k)).find(|c| *c != ' ' && *c != '\t');
                let after = (end < self.len_chars()).then(|| rope.char(end));
                let opener = before.and_then(|b| OPENERS.iter().find(|(o, _)| *o == b));
                let mut t = String::from("\n");
                t.push_str(&indent);
                match opener {
                    Some((_, close)) => {
                        t.push_str(&unit);
                        let cursor = t.chars().count();
                        if after == Some(*close) {
                            t.push('\n');
                            t.push_str(&indent);
                        }
                        Some(SelEdit { range: start..end, text: t, anchor: cursor, head: cursor })
                    }
                    None => Some(SelEdit::replace(start..end, t)),
                }
            })
            .collect();
        self.apply_sel_edits(sels, edits, origin, EditKind::Other)
    }

    /// Lines touched by a selection (a selection ending at column 0 of a later line does not include that line).
    pub fn selection_lines(&self, s: &Selection) -> (usize, usize) {
        let a = self.char_to_line(s.start());
        let mut b = self.char_to_line(s.end());
        if b > a && s.end() == self.line_to_char(b) {
            b -= 1;
        }
        (a, b)
    }

    /// Merged line blocks of all selections (overlapping or adjacent blocks join).
    fn line_blocks(&self, sels: &Selections) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for s in sels.iter() {
            let (a, b) = self.selection_lines(s);
            match blocks.last_mut() {
                Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
                _ => blocks.push((a, b)),
            }
        }
        blocks
    }

    /// Tab: with selections over several lines indent those lines, else insert one indentation unit at each cursor
    /// (spaces up to the next stop).
    pub fn indent(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let multi_line = sels.iter().any(|s| {
            let (a, b) = self.selection_lines(s);
            b > a
        });
        if !multi_line {
            let style = self.indent_settings().style;
            let tab = self.tab_width();
            let edits = sels
                .iter()
                .map(|s| {
                    let t = match style {
                        IndentStyle::Tabs => "\t".to_string(),
                        IndentStyle::Spaces(n) => {
                            let n = n.max(1) as usize;
                            let _ = tab;
                            let v = self.visual_col(s.start());
                            " ".repeat(n - v % n)
                        }
                    };
                    Some(SelEdit::replace(s.range(), t))
                })
                .collect();
            return self.apply_sel_edits(sels, edits, origin, EditKind::Other);
        }
        self.indent_lines(sels, origin)
    }

    /// Indent all lines touched by the selections by one level (blank lines are skipped).
    pub fn indent_lines(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let unit = self.indent_settings().style.unit();
        let mut edits = Vec::new();
        for (a, b) in self.line_blocks(sels) {
            for line in a..=b {
                if !text::is_blank_line(self.rope(), line) {
                    let ls = self.line_to_char(line);
                    edits.push((ls..ls, unit.clone()));
                }
            }
        }
        let Ok(cs) = ChangeSet::from_edits(self.rope(), edits) else { return false };
        self.commit_mapped(sels, cs, origin, EditKind::Other, Assoc::Before, Assoc::After)
    }

    /// Shift+Tab: remove one indentation level from all lines touched by the selections.
    pub fn outdent(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let n = self.indent_settings().unit_width();
        let mut edits = Vec::new();
        for (a, b) in self.line_blocks(sels) {
            for line in a..=b {
                let ls = self.line_to_char(line);
                let l = self.line(line);
                let first = l.chars().next();
                let k = match first {
                    Some('\t') => 1,
                    Some(' ') => {
                        let spaces = l.chars().take_while(|c| *c == ' ').count();
                        let k = if spaces % n == 0 { n } else { spaces % n };
                        // Remove a tab that follows the spaces too, if the spaces are fewer than one level.
                        k.min(spaces)
                    }
                    _ => 0,
                };
                if k > 0 {
                    edits.push((ls..ls + k, String::new()));
                }
            }
        }
        let Ok(cs) = ChangeSet::from_edits(self.rope(), edits) else { return false };
        self.commit_mapped(sels, cs, origin, EditKind::Other, Assoc::Before, Assoc::After)
    }

    /// Alt+Up / Alt+Down: move the lines of all selections one line up/down; the selections move along.
    pub fn move_lines(&mut self, sels: &mut Selections, down: bool, origin: EditOrigin) -> bool {
        self.sync(sels);
        let total = self.len_lines();
        let blocks = self.line_blocks(sels);
        let mut edits = Vec::new();
        let mut shifts: Vec<((usize, usize), isize)> = Vec::new();
        for (a, b) in blocks {
            if (!down && a == 0) || (down && b + 1 >= total) {
                continue;
            }
            let (ra, rb) = if down { (a, b + 1) } else { (a - 1, b) };
            let start = self.line_to_char(ra);
            let has_trailing_nl = rb + 1 < total;
            let end = if has_trailing_nl { self.line_to_char(rb + 1) } else { self.len_chars() };
            let mut lines: Vec<String> = (ra..=rb).map(|l| self.line_string(l)).collect();
            let moved_len;
            if down {
                let last = lines.pop().expect("block + next line");
                moved_len = last.chars().count() + 1;
                lines.insert(0, last);
            } else {
                let first = lines.remove(0);
                moved_len = first.chars().count() + 1;
                lines.push(first);
            }
            let mut t = lines.join("\n");
            if has_trailing_nl {
                t.push('\n');
            }
            edits.push((start..end, t));
            shifts.push(((a, b), if down { moved_len as isize } else { -(moved_len as isize) }));
        }
        if edits.is_empty() {
            return false;
        }
        let Ok(cs) = ChangeSet::from_edits(self.rope(), edits) else { return false };
        let after: Vec<Selection> = sels
            .iter()
            .map(|s| {
                let (a, _) = self.selection_lines(s);
                let d = shifts.iter().find(|((x, y), _)| a >= *x && a <= *y).map(|(_, d)| *d).unwrap_or(0);
                let m = |p: usize| (p as isize + d) as usize;
                Selection { anchor: m(s.anchor), head: m(s.head), goal: s.goal }
            })
            .collect();
        let after = Selections::from_vec(after, sels.primary_index());
        self.commit(sels, cs, after, origin, EditKind::Other)
    }

    /// Shift+Alt+Down / Up: duplicate the lines of all selections; with `down` the selections move to the copy.
    pub fn duplicate_lines(&mut self, sels: &mut Selections, down: bool, origin: EditOrigin) -> bool {
        self.sync(sels);
        let total = self.len_lines();
        let mut edits = Vec::new();
        let mut shifts: Vec<((usize, usize), usize)> = Vec::new();
        for (a, b) in self.line_blocks(sels) {
            let block: String = (a..=b).map(|l| self.line_string(l)).collect::<Vec<_>>().join("\n");
            let n = block.chars().count() + 1;
            if b + 1 < total {
                let at = self.line_to_char(b + 1);
                edits.push((at..at, format!("{block}\n")));
            } else {
                let at = self.len_chars();
                edits.push((at..at, format!("\n{block}")));
            }
            shifts.push(((a, b), n));
        }
        let Ok(cs) = ChangeSet::from_edits(self.rope(), edits) else { return false };
        let after: Vec<Selection> = sels
            .iter()
            .map(|s| {
                let (a, _) = self.selection_lines(s);
                let d = if down { shifts.iter().find(|((x, y), _)| a >= *x && a <= *y).map(|(_, d)| *d).unwrap_or(0) } else { 0 };
                let m = |p: usize| cs.map_pos(p, Assoc::Before) + d;
                Selection { anchor: m(s.anchor), head: m(s.head), goal: s.goal }
            })
            .collect();
        let after = Selections::from_vec(after, sels.primary_index());
        self.commit(sels, cs, after, origin, EditKind::Other)
    }

    /// Ctrl+Shift+K: delete the lines of all selections.
    pub fn delete_lines(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let total = self.len_lines();
        let edits: Vec<(Range<usize>, String)> = self
            .line_blocks(sels)
            .into_iter()
            .map(|(a, b)| {
                let start = self.line_to_char(a);
                if b + 1 < total {
                    (start..self.line_to_char(b + 1), String::new())
                } else if a > 0 {
                    (start - 1..self.len_chars(), String::new())
                } else {
                    (start..self.len_chars(), String::new())
                }
            })
            .collect();
        let Ok(cs) = ChangeSet::from_edits(self.rope(), edits) else { return false };
        // Cursors go to the start of the line that takes the deleted lines' place.
        let mut after = sels.clone();
        after.transform(|s| {
            let p = cs.map_pos(s.start(), Assoc::Before);
            Selection::cursor(p)
        });
        let len_after = (self.len_chars() as isize + cs.len_delta()) as usize;
        after.clamp(len_after);
        let r = self.commit(sels, cs, after, origin, EditKind::Other);
        if r {
            let rope = self.rope();
            sels.transform(|s| {
                let line = rope.char_to_line(s.head);
                Selection::cursor(rope.line_to_char(line))
            });
        }
        r
    }

    /// Ctrl+C: selected texts, or whole lines for empty selections.
    pub fn copy(&self, sels: &Selections) -> ClipboardText {
        if sels.iter().all(|s| s.is_empty()) {
            let mut parts = Vec::new();
            let mut last_line = None;
            for s in sels.iter() {
                let line = self.char_to_line(s.head);
                if last_line == Some(line) {
                    continue;
                }
                last_line = Some(line);
                parts.push(format!("{}\n", self.line_string(line)));
            }
            return ClipboardText { text: parts.concat(), parts, whole_lines: true };
        }
        let parts: Vec<String> = sels.iter().filter(|s| !s.is_empty()).map(|s| self.slice_string(s.range())).collect();
        ClipboardText { text: parts.join("\n"), parts, whole_lines: false }
    }

    /// Ctrl+X: copy, then delete the selections (whole lines for empty selections).
    pub fn cut(&mut self, sels: &mut Selections, origin: EditOrigin) -> ClipboardText {
        self.sync(sels);
        let clip = self.copy(sels);
        if clip.whole_lines {
            self.delete_lines(sels, origin);
        } else {
            let edits = sels.iter().map(|s| (!s.is_empty()).then(|| SelEdit::delete(s.range()))).collect();
            self.apply_sel_edits(sels, edits, origin, EditKind::Other);
        }
        clip
    }

    /// Ctrl+A.
    pub fn select_all(&self, sels: &mut Selections) {
        sels.replace(vec![Selection::new(0, self.len_chars())], 0);
        sels.rev = Some(self.revision());
    }

    /// Ctrl+L: extend every selection to whole lines (including the line break); again → one more line.
    pub fn select_lines(&self, sels: &mut Selections) {
        self.sync(sels);
        let total = self.len_lines();
        sels.transform(|s| {
            let (a, b) = self.selection_lines(&s);
            let start = self.line_to_char(a);
            let full_line_end = |l: usize| if l + 1 < total { self.line_to_char(l + 1) } else { self.len_chars() };
            let mut end = full_line_end(b);
            if s.start() == start && s.end() == end && !s.is_empty() {
                end = full_line_end((b + 1).min(total - 1));
            }
            Selection::new(start, end)
        });
    }

    /// Double click: the word (or run of punctuation/blanks) at `pos`.
    pub fn word_at(&self, pos: usize) -> Range<usize> {
        text::word_range_at(self.rope(), pos)
    }

    /// Ctrl+Alt+Up/Down: add a cursor on the line above the topmost (below the bottommost) cursor, same visual column.
    pub fn add_cursor_vertical(&self, sels: &mut Selections, below: bool) -> bool {
        self.sync(sels);
        let s = if below { *sels.as_slice().last().expect("non-empty") } else { sels.as_slice()[0] };
        let line = self.char_to_line(s.head);
        let target = if below {
            if line + 1 >= self.len_lines() {
                return false;
            }
            line + 1
        } else {
            match line.checked_sub(1) {
                Some(l) => l,
                None => return false,
            }
        };
        let goal = s.goal.unwrap_or_else(|| self.visual_col(s.head));
        let pos = self.pos_at_visual(target, goal);
        sels.push(Selection { anchor: pos, head: pos, goal: Some(goal) });
        true
    }

    /// Ctrl+D: with an empty primary cursor select the word there; else add the next occurrence of the primary
    /// selection's text (whole word if the selection is a whole word) as a new primary selection. Wraps around.
    pub fn add_next_occurrence(&self, sels: &mut Selections) -> bool {
        self.sync(sels);
        let p = sels.primary();
        if p.is_empty() {
            let r = self.word_at(p.head);
            if r.is_empty() || !self.slice(r.clone()).chars().any(text::is_word_char) {
                return false;
            }
            sels.set_primary(Selection::new(r.start, r.end));
            return true;
        }
        let Some(search) = self.occurrence_search(&p) else { return false };
        let mut from = p.end();
        for _ in 0..sels.len() + 1 {
            let Some(m) = search.find_next(self, from, Direction::Forward, true) else { return false };
            let taken = sels.iter().any(|s| s.start() < m.end && m.start < s.end());
            if !taken {
                sels.push(Selection::new(m.start, m.end));
                return true;
            }
            from = m.end;
        }
        false
    }

    /// Ctrl+Shift+L: select all occurrences of the primary selection (or of the word at the cursor).
    pub fn select_all_occurrences(&self, sels: &mut Selections, limit: usize) -> usize {
        self.sync(sels);
        let mut p = sels.primary();
        if p.is_empty() {
            let r = self.word_at(p.head);
            if r.is_empty() {
                return 0;
            }
            p = Selection::new(r.start, r.end);
        }
        let Some(search) = self.occurrence_search(&p) else { return 0 };
        let (all, _) = search.find_all(self, limit);
        if all.is_empty() {
            return 0;
        }
        let primary = all.iter().position(|m| *m == p.range()).unwrap_or(0);
        let n = all.len();
        sels.replace(all.into_iter().map(|m| Selection::new(m.start, m.end)).collect(), primary);
        n
    }

    fn occurrence_search(&self, p: &Selection) -> Option<Search> {
        let needle = self.slice_string(p.range());
        let mut q = SearchQuery::literal(&needle);
        q.whole_word = self.word_at(p.start()) == p.range() && needle.chars().all(text::is_word_char);
        Search::new(&q).ok()
    }

    /// Alt+drag / Shift+Alt+arrows: one selection per line from `anchor` to `head` (line, visual column). Lines that
    /// are too short for a non-empty box are skipped (the head line always stays).
    pub fn column_select(&self, sels: &mut Selections, anchor: (usize, usize), head: (usize, usize)) {
        let last = self.len_lines().saturating_sub(1);
        let (al, hl) = (anchor.0.min(last), head.0.min(last));
        let lines: Vec<usize> = if al <= hl { (al..=hl).collect() } else { (hl..=al).rev().collect() };
        let mut out = Vec::new();
        let mut primary = 0;
        for line in lines {
            let a = self.pos_at_visual(line, anchor.1);
            let h = self.pos_at_visual(line, head.1);
            let width = self.rope().line(line).len_chars();
            let too_short = anchor.1 != head.1 && a == h && text::line_visual_width(self.rope(), line, self.tab_width()) < anchor.1.min(head.1);
            if too_short && line != hl {
                continue;
            }
            let _ = width;
            if line == hl {
                primary = out.len();
            }
            out.push(Selection { anchor: a, head: h, goal: Some(head.1) });
        }
        sels.replace(out, primary);
        sels.rev = Some(self.revision());
    }

    /// Go to line `line` (1-based) and optional column `col` (1-based, chars); clamped. Returns the position.
    pub fn goto_line(&self, sels: &mut Selections, line: usize, col: Option<usize>) -> usize {
        let l = line.max(1) - 1;
        let pos = self.linecol_to_pos(text::LineCol::new(l, col.unwrap_or(1).max(1) - 1));
        *sels = self.new_selections(pos);
        pos
    }

    /// Remove trailing whitespace on all lines (one undo step).
    pub fn trim_trailing_whitespace_now(&mut self, sels: &mut Selections, origin: EditOrigin) -> bool {
        self.sync(sels);
        let edits = self.trailing_whitespace_ranges().into_iter().map(|r| (r, String::new())).collect();
        let Ok(cs) = ChangeSet::from_edits(self.rope(), edits) else { return false };
        self.commit_mapped(sels, cs, origin, EditKind::Other, Assoc::Before, Assoc::Before)
    }
}

/// Parse "go to line" input: `12`, `12:5`, `12,5` (1-based). `None` if it is not a number.
pub fn parse_goto(input: &str) -> Option<(usize, Option<usize>)> {
    let s = input.trim();
    let (l, c) = match s.split_once([':', ',']) {
        Some((l, c)) => (l.trim(), Some(c.trim())),
        None => (s, None),
    };
    let line: usize = l.parse().ok()?;
    let col = match c {
        Some(c) if !c.is_empty() => Some(c.parse().ok()?),
        _ => None,
    };
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{IndentSettings, IndentSource};

    fn doc_with(text: &str, style: IndentStyle) -> Document {
        let mut d = Document::from_text(text);
        d.set_indent_settings(IndentSettings { style, tab_width: 4, source: IndentSource::User });
        d
    }

    fn cursors(d: &Document, pos: &[usize]) -> Selections {
        let mut s = Selections::from_vec(pos.iter().map(|p| Selection::cursor(*p)).collect(), 0);
        d.sync(&mut s);
        s
    }

    fn heads(s: &Selections) -> Vec<usize> {
        s.iter().map(|x| x.head).collect()
    }

    #[test]
    fn multi_cursor_typing_and_undo_grouping() {
        let mut d = Document::from_text("a\nb\nc\n");
        let mut s = cursors(&d, &[1, 3, 5]);
        for c in ["x", "y", "z"] {
            d.type_text(&mut s, c, EditOrigin::User);
        }
        assert_eq!(d.text(), "axyz\nbxyz\ncxyz\n");
        assert_eq!(heads(&s), vec![4, 9, 14]);
        // Typing merged into one step.
        d.undo(&mut s);
        assert_eq!(d.text(), "a\nb\nc\n");
        assert_eq!(heads(&s), vec![1, 3, 5]);
        d.redo(&mut s);
        assert_eq!(d.text(), "axyz\nbxyz\ncxyz\n");
        // The UI breaks the group when the cursor is moved by click/keys.
        d.type_text(&mut s, "1", EditOrigin::User);
        d.break_undo_group();
        d.type_text(&mut s, "2", EditOrigin::User);
        d.undo(&mut s);
        assert_eq!(d.text(), "axyz1\nbxyz1\ncxyz1\n");
        // Cursors that ended up elsewhere also start a new step (positions differ from the last step's end).
        let mut other = cursors(&d, &[0]);
        d.type_text(&mut other, "#", EditOrigin::User);
        d.undo(&mut other);
        assert_eq!(d.text(), "axyz1\nbxyz1\ncxyz1\n");
        d.sync(&mut s);
        // An agent edit never merges with user typing.
        d.type_text(&mut s, "u", EditOrigin::User);
        d.type_text(&mut s, "g", EditOrigin::Agent);
        d.undo(&mut s);
        assert_eq!(d.text(), "axyz1u\nbxyz1u\ncxyz1u\n");
    }

    #[test]
    fn typing_merge_window_respected() {
        let mut d = Document::from_text("");
        d.set_history_limits(crate::editor::HistoryLimits { merge_window: std::time::Duration::ZERO, ..Default::default() });
        let mut s = d.new_selections(0);
        d.type_text(&mut s, "a", EditOrigin::User);
        std::thread::sleep(std::time::Duration::from_millis(2));
        d.type_text(&mut s, "b", EditOrigin::User);
        d.undo(&mut s);
        assert_eq!(d.text(), "a");
    }

    #[test]
    fn backspace_delete_and_indent_stops() {
        let mut d = doc_with("      x\nab", IndentStyle::Spaces(4));
        let mut s = cursors(&d, &[6, 10]);
        d.backspace(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "    x\na");
        d.backspace(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "x\n");
        assert_eq!(heads(&s), vec![0, 2]);
        d.undo(&mut s);
        assert_eq!(d.text(), "      x\nab");
        let mut s = cursors(&d, &[0, 8]);
        d.delete_forward(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "     x\nb");
        // Overlapping word deletions merge instead of failing.
        let mut d = Document::from_text("foo bar");
        let mut s = cursors(&d, &[5, 7]);
        d.delete_word_left(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "foo ");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn newline_auto_indent() {
        let mut d = doc_with("    if x {}", IndentStyle::Spaces(4));
        let mut s = cursors(&d, &[10]);
        d.newline(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "    if x {\n        \n    }");
        assert_eq!(d.pos_to_linecol(s.primary().head), crate::editor::LineCol::new(1, 8));
        let mut d = doc_with("\tfoo", IndentStyle::Tabs);
        let mut s = cursors(&d, &[4]);
        d.newline(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "\tfoo\n\t");
        // Selection is replaced.
        let mut d = doc_with("ab cd", IndentStyle::Spaces(2));
        let mut s = Selections::single(Selection::new(2, 3));
        d.newline(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "ab\ncd");
    }

    #[test]
    fn indent_and_outdent() {
        let mut d = doc_with("a\n\nb\nc", IndentStyle::Spaces(2));
        let mut s = Selections::single(Selection::new(0, 5));
        d.indent(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "  a\n\n  b\nc");
        assert_eq!(s.primary().range(), 0..9);
        d.outdent(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "a\n\nb\nc");
        // Tab with a cursor inserts spaces to the next stop.
        let mut d = doc_with("abc", IndentStyle::Spaces(4));
        let mut s = cursors(&d, &[1]);
        d.indent(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "a   bc");
        let mut d = doc_with("\t\tx\n   y", IndentStyle::Tabs);
        let mut s = Selections::single(Selection::new(0, 8));
        d.outdent(&mut s, EditOrigin::User);
        assert_eq!(d.text(), "\tx\ny");
    }

    #[test]
    fn move_duplicate_delete_lines() {
        let mut d = Document::from_text("1\n2\n3");
        let mut s = cursors(&d, &[2]);
        d.move_lines(&mut s, false, EditOrigin::User);
        assert_eq!(d.text(), "2\n1\n3");
        assert_eq!(heads(&s), vec![0]);
        d.move_lines(&mut s, true, EditOrigin::User);
        d.move_lines(&mut s, true, EditOrigin::User);
        assert_eq!(d.text(), "1\n3\n2");
        assert_eq!(heads(&s), vec![4]);
        assert!(!d.move_lines(&mut s, true, EditOrigin::User));
        d.duplicate_lines(&mut s, true, EditOrigin::User);
        assert_eq!(d.text(), "1\n3\n2\n2");
        assert_eq!(heads(&s), vec![6]);
        let mut s2 = cursors(&d, &[0, 2]);
        d.duplicate_lines(&mut s2, false, EditOrigin::User);
        assert_eq!(d.text(), "1\n3\n1\n3\n2\n2");
        assert_eq!(heads(&s2), vec![0, 2]);
        let mut s3 = cursors(&d, &[0, 9]);
        d.delete_lines(&mut s3, EditOrigin::User);
        assert_eq!(d.text(), "3\n1\n3\n2");
        assert_eq!(heads(&s3), vec![0, 6]);
        d.undo(&mut s3);
        assert_eq!(d.text(), "1\n3\n1\n3\n2\n2");
    }

    #[test]
    fn clipboard_and_paste_distribution() {
        let mut d = Document::from_text("ab\ncd\n");
        let mut s = Selections::from_vec(vec![Selection::new(0, 1), Selection::new(3, 4)], 0);
        d.sync(&mut s);
        let clip = d.copy(&s);
        assert_eq!(clip.text, "a\nc");
        // Same number of cursors: one part each.
        let mut t = cursors(&d, &[2, 5]);
        d.paste(&mut t, &clip, EditOrigin::User);
        assert_eq!(d.text(), "aba\ncdc\n");
        // Whole-line copy and paste above.
        let s = cursors(&d, &[1]);
        let line = d.copy(&s);
        assert!(line.whole_lines);
        assert_eq!(line.text, "aba\n");
        let mut u = cursors(&d, &[5]);
        d.paste(&mut u, &line, EditOrigin::User);
        assert_eq!(d.text(), "aba\naba\ncdc\n");
        assert_eq!(heads(&u), vec![9]);
        let mut v = cursors(&d, &[0]);
        let cut = d.cut(&mut v, EditOrigin::User);
        assert_eq!(cut.text, "aba\n");
        assert_eq!(d.text(), "aba\ncdc\n");
        // CRLF from the clipboard becomes LF.
        let mut w = cursors(&d, &[0]);
        d.paste(&mut w, &ClipboardText { text: "x\r\ny".into(), parts: vec![], whole_lines: false }, EditOrigin::User);
        assert_eq!(d.text(), "x\nyaba\ncdc\n");
    }

    #[test]
    fn add_cursors_and_occurrences() {
        let d = Document::from_text("abc\nx\nabcdef\n");
        let mut s = cursors(&d, &[2]);
        assert!(d.add_cursor_vertical(&mut s, true));
        assert!(d.add_cursor_vertical(&mut s, true));
        assert_eq!(heads(&s), vec![2, 5, 8]);
        assert!(!d.add_cursor_vertical(&mut cursors(&d, &[0]), false));
        let d = Document::from_text("foo bar foo foobar foo");
        let mut s = cursors(&d, &[1]);
        assert!(d.add_next_occurrence(&mut s));
        assert_eq!(s.primary().range(), 0..3);
        assert!(d.add_next_occurrence(&mut s));
        assert!(d.add_next_occurrence(&mut s));
        // "foobar" is skipped (whole word), wraps to nothing new.
        assert_eq!(s.iter().map(|x| x.range()).collect::<Vec<_>>(), vec![0..3, 8..11, 19..22]);
        assert!(!d.add_next_occurrence(&mut s));
        let mut t = cursors(&d, &[9]);
        assert_eq!(d.select_all_occurrences(&mut t, 100), 3);
        assert_eq!(t.primary().range(), 8..11);
    }

    #[test]
    fn column_selection() {
        let d = Document::from_text("abcdef\nab\nabcdef\n\tx");
        let mut s = d.new_selections(0);
        d.column_select(&mut s, (0, 1), (2, 4));
        assert_eq!(s.iter().map(|x| x.range()).collect::<Vec<_>>(), vec![1..4, 8..9, 11..14]);
        assert_eq!(s.primary().range(), 11..14);
        let mut d2 = Document::from_text("abcdef\nab\nabcdef\n\tx");
        let mut s = d2.new_selections(0);
        d2.column_select(&mut s, (0, 4), (2, 5));
        assert_eq!(s.len(), 2); // line 1 is too short
        d2.type_text(&mut s, "#", EditOrigin::User);
        assert_eq!(d2.text(), "abcd#f\nab\nabcd#f\n\tx");
        // Tabs count by visual column.
        let mut s = d2.new_selections(0);
        d2.column_select(&mut s, (3, 4), (3, 4));
        assert_eq!(s.primary().head, d2.line_to_char(3) + 1);
    }

    #[test]
    fn goto_and_select_lines() {
        let d = Document::from_text("a\nbb\nccc");
        let mut s = d.new_selections(0);
        assert_eq!(d.goto_line(&mut s, 3, Some(2)), 6);
        assert_eq!(d.goto_line(&mut s, 99, None), 5);
        assert_eq!(d.goto_line(&mut s, 0, Some(99)), 1);
        assert_eq!(parse_goto(" 12:5 "), Some((12, Some(5))));
        assert_eq!(parse_goto("7,"), Some((7, None)));
        assert_eq!(parse_goto("x"), None);
        let mut s = d.new_selections(3);
        d.select_lines(&mut s);
        assert_eq!(s.primary().range(), 2..5);
        d.select_lines(&mut s);
        assert_eq!(s.primary().range(), 2..8);
        d.select_all(&mut s);
        assert_eq!(s.primary().range(), 0..8);
    }
}
