//! Cursor movement for all selections of a view.

use super::document::Document;
use super::selection::{Selection, Selections};
use super::text::{CharClass, char_class, next_grapheme, prev_grapheme};

/// A cursor movement (keys: arrows, Ctrl+arrows, Home/End, Ctrl+Home/End, PageUp/Down = `Up(n)`/`Down(n)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// One grapheme cluster left.
    Left,
    Right,
    /// To the start of the previous word.
    WordLeft,
    /// To the end of the next word.
    WordRight,
    /// `n` lines up, keeping the visual column (goal).
    Up(usize),
    Down(usize),
    /// Column 0.
    LineStart,
    /// First non-blank char; again → column 0 (Home).
    SmartLineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

impl Document {
    /// Move every cursor; with `extend` the anchors stay (Shift). Collapsing a selection with Left/Right goes to
    /// its start/end.
    pub fn move_cursors(&self, sels: &mut Selections, motion: Motion, extend: bool) {
        self.sync(sels);
        sels.transform(|s| self.move_one(s, motion, extend));
    }

    /// Target of `motion` for one selection.
    pub fn move_one(&self, s: Selection, motion: Motion, extend: bool) -> Selection {
        if !extend && !s.is_empty() {
            match motion {
                Motion::Left => return Selection::cursor(s.start()),
                Motion::Right => return Selection::cursor(s.end()),
                _ => {}
            }
        }
        let rope = self.rope();
        let head = s.head.min(self.len_chars());
        let mut goal = None;
        let pos = match motion {
            Motion::Left => prev_grapheme(rope, head),
            Motion::Right => next_grapheme(rope, head),
            Motion::WordLeft => self.word_left(head),
            Motion::WordRight => self.word_right(head),
            Motion::Up(n) | Motion::Down(n) => {
                let g = s.goal.unwrap_or_else(|| self.visual_col(head));
                let line = self.char_to_line(head);
                let last = self.len_lines().saturating_sub(1);
                let up = matches!(motion, Motion::Up(_));
                if up && line == 0 {
                    0
                } else if !up && line >= last {
                    self.len_chars()
                } else {
                    goal = Some(g);
                    let target = if up { line.saturating_sub(n.max(1)) } else { (line + n.max(1)).min(last) };
                    self.pos_at_visual(target, g)
                }
            }
            Motion::LineStart => self.line_to_char(self.char_to_line(head)),
            Motion::SmartLineStart => {
                let line = self.char_to_line(head);
                let ls = self.line_to_char(line);
                let first = ls + super::text::indent_len(rope, line);
                if head == first { ls } else { first }
            }
            Motion::LineEnd => super::text::line_end(rope, self.char_to_line(head)),
            Motion::DocStart => 0,
            Motion::DocEnd => self.len_chars(),
        };
        if extend { Selection { anchor: s.anchor, head: pos, goal } } else { Selection { anchor: pos, head: pos, goal } }
    }

    /// Start of the word left of `pos` (skips blanks first; stops at line breaks).
    pub fn word_left(&self, pos: usize) -> usize {
        let r = self.rope();
        let mut p = pos.min(self.len_chars());
        while p > 0 && char_class(r.char(p - 1)) == CharClass::Space {
            p -= 1;
        }
        if p == 0 {
            return 0;
        }
        let c = char_class(r.char(p - 1));
        if c == CharClass::Newline {
            return if p == pos { p - 1 } else { p };
        }
        while p > 0 && char_class(r.char(p - 1)) == c {
            p -= 1;
        }
        p
    }

    /// End of the word right of `pos` (skips blanks first; stops at line breaks).
    pub fn word_right(&self, pos: usize) -> usize {
        let r = self.rope();
        let len = self.len_chars();
        let mut p = pos.min(len);
        while p < len && char_class(r.char(p)) == CharClass::Space {
            p += 1;
        }
        if p >= len {
            return len;
        }
        let c = char_class(r.char(p));
        if c == CharClass::Newline {
            return if p == pos { p + 1 } else { p };
        }
        while p < len && char_class(r.char(p)) == c {
            p += 1;
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_and_words() {
        let d = Document::from_text("foo bar.baz\n  x");
        let mut s = d.new_selections(0);
        d.move_cursors(&mut s, Motion::WordRight, false);
        assert_eq!(s.primary().head, 3);
        d.move_cursors(&mut s, Motion::WordRight, false);
        assert_eq!(s.primary().head, 7);
        d.move_cursors(&mut s, Motion::WordRight, true);
        assert_eq!(s.primary().range(), 7..8);
        d.move_cursors(&mut s, Motion::Left, false);
        assert_eq!(s.primary().head, 7);
        d.move_cursors(&mut s, Motion::WordLeft, false);
        assert_eq!(s.primary().head, 4);
        assert_eq!(d.word_right(11), 12);
        assert_eq!(d.word_left(12), 11);
        assert_eq!(d.word_left(14), 12);
    }

    #[test]
    fn vertical_keeps_goal_column() {
        let d = Document::from_text("long line here\nab\nanother long one\n");
        let mut s = d.new_selections(10);
        d.move_cursors(&mut s, Motion::Down(1), false);
        assert_eq!(d.pos_to_linecol(s.primary().head), crate::editor::LineCol::new(1, 2));
        d.move_cursors(&mut s, Motion::Down(1), false);
        assert_eq!(d.pos_to_linecol(s.primary().head), crate::editor::LineCol::new(2, 10));
        d.move_cursors(&mut s, Motion::Up(5), false);
        assert_eq!(s.primary().head, 10);
        d.move_cursors(&mut s, Motion::Up(1), false);
        assert_eq!(s.primary().head, 0);
        d.move_cursors(&mut s, Motion::DocEnd, false);
        assert_eq!(s.primary().head, d.len_chars());
    }

    #[test]
    fn home_toggles_and_cursors_merge() {
        let d = Document::from_text("    indented");
        let mut s = d.new_selections(8);
        d.move_cursors(&mut s, Motion::SmartLineStart, false);
        assert_eq!(s.primary().head, 4);
        d.move_cursors(&mut s, Motion::SmartLineStart, false);
        assert_eq!(s.primary().head, 0);
        d.move_cursors(&mut s, Motion::LineEnd, false);
        assert_eq!(s.primary().head, 12);
        let mut m = Selections::from_vec(vec![Selection::cursor(1), Selection::cursor(2)], 1);
        d.move_cursors(&mut m, Motion::LineStart, false);
        assert_eq!(m.len(), 1);
    }
}
