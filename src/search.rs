//! Suche im Terminal-Verlauf (Scrollback + sichtbarer Bereich).

use crate::terminal::Listener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;

/// Treffer: `line` als Zeile des Rasters (0 = oberste Bildschirmzeile, negativ im Verlauf), `col`/`len` in Zellen
/// (ein breites Zeichen zählt zwei).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Match {
    pub line: i32,
    pub col: usize,
    pub len: usize,
}

/// Höchstens so viele Treffer; bei mehr bleiben die neuesten (#250).
pub const MAX_MATCHES: usize = 5000;

pub struct SearchState {
    pub query: String,
    pub matches: Vec<Match>,
    pub current: usize,
    pub focus_input: bool,
    pub last_query: String,
    /// Absolute Zeile von Rasterzeile 0 bei der Berechnung (`Session::hist_base` + Verlaufslänge), damit der aktuelle
    /// Treffer nach neuer Ausgabe wiedergefunden wird (#250).
    pub origin: i64,
    /// Stand des Inhalts bei der Berechnung (Ausgabe-Zeitstempel, Zeilen, Spalten, verworfene Zeilen); ändert er sich,
    /// werden die Treffer neu berechnet (#250).
    pub stamp: (u64, usize, usize, u64),
    /// Zeitpunkt (egui-Zeit) der letzten Neuberechnung, zum Drosseln bei laufender Ausgabe.
    pub refreshed: f64,
}

impl SearchState {
    pub fn new() -> Self {
        Self { query: String::new(), matches: vec![], current: 0, focus_input: true, last_query: String::new(), origin: 0, stamp: (0, 0, 0, 0), refreshed: f64::NEG_INFINITY }
    }

    /// Treffer für `query` neu berechnen; `base` = `Session::hist_base`, `activity` = `Session::output_stamp`.
    pub fn compute(&mut self, term: &Term<Listener>, base: u64, activity: u64) {
        self.matches = find_all(term, &self.query);
        self.origin = origin_of(term, base);
        self.stamp = stamp_of(term, base, activity);
    }

    /// Hat sich der Inhalt seit der Berechnung geändert?
    pub fn stale(&self, term: &Term<Listener>, base: u64, activity: u64) -> bool {
        !self.query.is_empty() && self.stamp != stamp_of(term, base, activity)
    }

    /// Nach neuer Ausgabe neu berechnen (#250): der aktuelle Treffer bleibt derselbe (gleiche absolute Zeile und Spalte),
    /// sonst der nächste darüber bzw. der oberste. Scrollt nicht.
    pub fn refresh(&mut self, term: &Term<Listener>, base: u64, activity: u64) {
        let old = self.matches.get(self.current).map(|m| (self.origin + m.line as i64, m.col));
        self.compute(term, base, activity);
        let n = self.matches.len();
        self.current = match old {
            _ if n == 0 => 0,
            None => n - 1,
            Some((abs, col)) => {
                let key = |m: &Match| (self.origin + m.line as i64, m.col);
                match self.matches.iter().position(|m| key(m) == (abs, col)) {
                    Some(i) => i,
                    None => self.matches.iter().rposition(|m| key(m) <= (abs, col)).unwrap_or(0),
                }
            }
        };
    }
}

fn origin_of(term: &Term<Listener>, base: u64) -> i64 {
    base as i64 + term.grid().total_lines().saturating_sub(term.grid().screen_lines()) as i64
}

fn stamp_of(term: &Term<Listener>, base: u64, activity: u64) -> (u64, usize, usize, u64) {
    (activity, term.grid().total_lines(), term.grid().columns(), base)
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Alle Treffer, älteste zuerst; bei mehr als `MAX_MATCHES` die neuesten (#250). Platzhalterzellen breiter Zeichen
/// zählen nicht als Zeichen, damit z. B. „日本語“ gefunden wird.
pub fn find_all(term: &Term<Listener>, query: &str) -> Vec<Match> {
    let q: Vec<char> = query.chars().map(lower).collect();
    if q.is_empty() {
        return vec![];
    }
    let grid = term.grid();
    let cols = grid.columns();
    let mut out = Vec::new();
    // je Zeichen: (Zeichen, Spalte, Breite in Zellen)
    let mut row: Vec<(char, usize, usize)> = Vec::with_capacity(cols);
    let mut hits: Vec<Match> = Vec::new();
    // von unten nach oben, damit bei der Grenze die neuesten bleiben
    for l in (grid.topmost_line().0..=grid.bottommost_line().0).rev() {
        row.clear();
        for c in 0..cols {
            let cell = &grid[Line(l)][Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let ch = cell.c;
            let w = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            row.push((if ch == '\0' { ' ' } else { lower(ch) }, c, w));
        }
        if row.len() < q.len() {
            continue;
        }
        hits.clear();
        let mut i = 0;
        while i + q.len() <= row.len() {
            if row[i..i + q.len()].iter().map(|r| r.0).eq(q.iter().copied()) {
                let (start, (_, last, w)) = (row[i].1, row[i + q.len() - 1]);
                hits.push(Match { line: l, col: start, len: last + w - start });
                i += q.len();
            } else {
                i += 1;
            }
        }
        for m in hits.drain(..).rev() {
            out.push(m);
            if out.len() >= MAX_MATCHES {
                out.reverse();
                return out;
            }
        }
    }
    out.reverse();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Listener;
    use alacritty_terminal::term::{test::TermSize, Config};
    use alacritty_terminal::vte::ansi::Processor;

    fn feed(t: &mut Term<Listener>, bytes: &[u8]) {
        let mut p: Processor = Processor::new();
        p.advance(t, bytes);
    }

    fn term(cols: usize, lines: usize, text: &str) -> Term<Listener> {
        let mut t = Term::new(Config::default(), &TermSize::new(cols, lines), Listener::for_tests());
        feed(&mut t, text.as_bytes());
        t
    }

    #[test]
    fn wide_characters_are_found_with_cell_columns() {
        let t = term(20, 3, "ab日本語c 本語");
        let m = find_all(&t, "日本語");
        assert_eq!(m, vec![Match { line: 0, col: 2, len: 6 }]);
        let m = find_all(&t, "語c");
        assert_eq!(m, vec![Match { line: 0, col: 6, len: 3 }]);
        assert_eq!(find_all(&t, "本語").len(), 2);
        assert_eq!(find_all(&t, "ab").len(), 1);
    }

    #[test]
    fn too_many_matches_keep_the_newest() {
        let mut s = String::new();
        for i in 0..(MAX_MATCHES + 50) {
            s.push_str(&format!("x{i}\r\n"));
        }
        let mut t = Term::new(Config { scrolling_history: MAX_MATCHES + 100, ..Config::default() }, &TermSize::new(20, 5), Listener::for_tests());
        feed(&mut t, s.as_bytes());
        let m = find_all(&t, "x");
        assert_eq!(m.len(), MAX_MATCHES);
        assert!(m.windows(2).all(|w| w[0].line < w[1].line), "älteste zuerst");
        assert_eq!(line_text(&t, m.last().unwrap().line), format!("x{}", MAX_MATCHES + 49), "die neueste ist dabei");
    }

    #[test]
    fn refresh_keeps_the_current_match_after_new_output() {
        let mut t = term(20, 4, "foo 1\r\nfoo 2\r\nbar\r\n");
        let mut st = SearchState::new();
        st.query = "foo".into();
        st.compute(&t, 0, 1);
        assert_eq!(st.matches.len(), 2);
        st.current = 0; // „foo 1“
        assert!(!st.stale(&t, 0, 1));
        feed(&mut t, b"foo 3\r\nx\r\ny\r\n");
        assert!(st.stale(&t, 0, 2));
        st.refresh(&t, 0, 2);
        assert_eq!(st.matches.len(), 3);
        assert_eq!(line_text(&t, st.matches[st.current].line), "foo 1", "derselbe Treffer, obwohl die Zeilen gewandert sind");
        assert!(!st.stale(&t, 0, 2));
    }
}
