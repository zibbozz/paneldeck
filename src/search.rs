//! Suche im Terminal-Verlauf (Scrollback + sichtbarer Bereich).

use crate::terminal::Listener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Match {
    pub line: i32,
    pub col: usize,
    pub len: usize,
}

pub struct SearchState {
    pub query: String,
    pub matches: Vec<Match>,
    pub current: usize,
    pub focus_input: bool,
    pub last_query: String,
}

impl SearchState {
    pub fn new() -> Self {
        Self { query: String::new(), matches: vec![], current: 0, focus_input: true, last_query: String::new() }
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

pub fn find_all(term: &Term<Listener>, query: &str) -> Vec<Match> {
    let q: Vec<char> = query.chars().map(lower).collect();
    if q.is_empty() {
        return vec![];
    }
    let grid = term.grid();
    let cols = grid.columns();
    let mut out = Vec::new();
    let mut row: Vec<char> = Vec::with_capacity(cols);
    for l in grid.topmost_line().0..=grid.bottommost_line().0 {
        row.clear();
        for c in 0..cols {
            let cell = &grid[Line(l)][Column(c)];
            let ch = cell.c;
            row.push(if ch == '\0' { ' ' } else { lower(ch) });
        }
        if row.len() < q.len() {
            continue;
        }
        let mut i = 0;
        while i + q.len() <= row.len() {
            if row[i..i + q.len()] == q[..] {
                out.push(Match { line: l, col: i, len: q.len() });
                i += q.len();
                if out.len() >= 5000 {
                    return out;
                }
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Text einer Zeile (rechts bereinigt).
pub fn line_text(term: &Term<Listener>, line: i32) -> String {
    let grid = term.grid();
    let mut s = String::new();
    for c in 0..grid.columns() {
        let cell = &grid[Line(line)][Column(c)];
        if cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        s.push(if cell.c == '\0' { ' ' } else { cell.c });
    }
    s.trim_end().to_string()
}

/// Scrollt so, dass die Zeile des Treffers etwa mittig sichtbar ist.
pub fn scroll_to(term: &mut Term<Listener>, line: i32) {
    let rows = term.screen_lines() as i32;
    let history = (term.grid().total_lines() as i32 - rows).max(0);
    let target = (rows / 2 - line).clamp(0, history);
    let cur = term.grid().display_offset() as i32;
    term.scroll_display(Scroll::Delta(target - cur));
}
