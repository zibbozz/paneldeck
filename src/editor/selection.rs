//! Cursors and selections (multi-cursor model).
//!
//! A [`Selection`] is an anchor and a head (char positions); an empty selection is a plain cursor. [`Selections`]
//! holds one or more of them, sorted and non-overlapping, with one *primary* selection (the one the view follows).
//! Each view (split) of a document owns its own `Selections`; [`Document::sync`](super::Document::sync) maps them
//! through edits made by other views.

use std::ops::Range;

/// One cursor or selection. `goal` is the visual column kept while moving up/down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    pub goal: Option<usize>,
}

impl Selection {
    /// An empty selection (cursor) at `pos`.
    pub fn cursor(pos: usize) -> Self {
        Self { anchor: pos, head: pos, goal: None }
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head, goal: None }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Head before anchor (selected "backwards").
    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// Same direction as `self`, covering `start..end`.
    pub fn with_range(&self, start: usize, end: usize) -> Self {
        if self.is_reversed() { Self::new(end, start) } else { Self::new(start, end) }
    }

    fn clamp(&mut self, len: usize) {
        self.anchor = self.anchor.min(len);
        self.head = self.head.min(len);
    }
}

/// All selections of one view: sorted by position, non-overlapping, never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selections {
    ranges: Vec<Selection>,
    primary: usize,
    /// Document revision these positions belong to (`None` = not yet synced; taken as current).
    pub(crate) rev: Option<u64>,
}

impl Default for Selections {
    fn default() -> Self {
        Self::single(Selection::cursor(0))
    }
}

impl Selections {
    /// One cursor at `pos`.
    pub fn cursor(pos: usize) -> Self {
        Self::single(Selection::cursor(pos))
    }

    pub fn single(sel: Selection) -> Self {
        Self { ranges: vec![sel], primary: 0, rev: None }
    }

    /// From several selections; `primary` is an index into `ranges`. Sorts and merges overlaps.
    /// An empty list gives a cursor at 0.
    pub fn from_vec(ranges: Vec<Selection>, primary: usize) -> Self {
        if ranges.is_empty() {
            return Self::default();
        }
        let primary = primary.min(ranges.len() - 1);
        let mut s = Self { ranges, primary, rev: None };
        s.normalize();
        s
    }

    /// Selections in document order.
    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.ranges.iter()
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.ranges
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// Always false (there is at least one selection); for clippy's `len_without_is_empty`.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn primary(&self) -> Selection {
        self.ranges[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// More than one cursor?
    pub fn is_multi(&self) -> bool {
        self.ranges.len() > 1
    }

    /// Replace the primary selection (others stay).
    pub fn set_primary(&mut self, sel: Selection) {
        self.ranges[self.primary] = sel;
        self.normalize();
    }

    /// Add a selection and make it primary.
    pub fn push(&mut self, sel: Selection) {
        self.ranges.push(sel);
        self.primary = self.ranges.len() - 1;
        self.normalize();
    }

    /// Keep only the primary selection (Esc with several cursors).
    pub fn collapse_to_primary(&mut self) {
        let p = self.primary();
        self.ranges = vec![p];
        self.primary = 0;
    }

    /// Change every selection with `f`, then sort/merge again.
    pub fn transform(&mut self, mut f: impl FnMut(Selection) -> Selection) {
        for s in &mut self.ranges {
            *s = f(*s);
        }
        self.normalize();
    }

    /// Replace all selections (`primary` = index into the new list).
    pub fn replace(&mut self, ranges: Vec<Selection>, primary: usize) {
        let rev = self.rev;
        *self = Self::from_vec(ranges, primary);
        self.rev = rev;
    }

    pub(crate) fn clamp(&mut self, len: usize) {
        for s in &mut self.ranges {
            s.clamp(len);
        }
        self.normalize();
    }

    /// Sort by start and merge overlapping selections (and cursors at the same spot). Touching non-empty
    /// selections stay separate.
    pub(crate) fn normalize(&mut self) {
        if self.ranges.len() <= 1 {
            self.primary = 0;
            return;
        }
        let mut items: Vec<(Selection, bool)> =
            self.ranges.iter().enumerate().map(|(i, s)| (*s, i == self.primary)).collect();
        items.sort_by_key(|(s, _)| (s.start(), s.end()));
        let mut out: Vec<(Selection, bool)> = Vec::with_capacity(items.len());
        for (s, p) in items {
            if let Some((last, lp)) = out.last_mut() {
                let overlap = s.start() < last.end() || (s.start() == last.end() && (s.is_empty() || last.is_empty()));
                if overlap {
                    let start = last.start();
                    let end = last.end().max(s.end());
                    // Keep the direction of the primary one, else of the first.
                    let dir = if p { s } else { *last };
                    let mut merged = dir.with_range(start, end);
                    if merged.is_empty() {
                        merged.goal = dir.goal;
                    }
                    *last = merged;
                    *lp = *lp || p;
                    continue;
                }
            }
            out.push((s, p));
        }
        self.primary = out.iter().position(|(_, p)| *p).unwrap_or(0);
        self.ranges = out.into_iter().map(|(s, _)| s).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_merges_and_keeps_primary() {
        let s = Selections::from_vec(
            vec![Selection::new(5, 8), Selection::cursor(1), Selection::new(7, 10), Selection::cursor(1)],
            2,
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s.as_slice()[0], Selection::cursor(1));
        assert_eq!(s.as_slice()[1].range(), 5..10);
        assert_eq!(s.primary_index(), 1);
    }

    #[test]
    fn touching_selections_stay_separate() {
        let s = Selections::from_vec(vec![Selection::new(0, 3), Selection::new(3, 5)], 0);
        assert_eq!(s.len(), 2);
        let s = Selections::from_vec(vec![Selection::new(0, 3), Selection::cursor(3)], 1);
        assert_eq!(s.len(), 1);
        assert_eq!(s.primary().range(), 0..3);
    }

    #[test]
    fn reversed_direction_kept() {
        let s = Selections::from_vec(vec![Selection::new(9, 4)], 0);
        assert!(s.primary().is_reversed());
        assert_eq!(s.primary().with_range(1, 2), Selection::new(2, 1));
    }
}
