//! Change sets: one atomic edit of the text (one or more non-overlapping replacements).

use ropey::Rope;
use std::ops::Range;

/// Replace `removed` (starting at `start`, char index in the text *before* the change set) by `inserted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub start: usize,
    pub removed: String,
    pub inserted: String,
    removed_len: usize,
    inserted_len: usize,
}

impl Change {
    fn new(start: usize, removed: String, inserted: String) -> Self {
        let removed_len = removed.chars().count();
        let inserted_len = inserted.chars().count();
        Self { start, removed, inserted, removed_len, inserted_len }
    }

    /// End of the removed range (old coordinates).
    pub fn end(&self) -> usize {
        self.start + self.removed_len
    }

    pub fn removed_len(&self) -> usize {
        self.removed_len
    }

    pub fn inserted_len(&self) -> usize {
        self.inserted_len
    }
}

/// How a position exactly at an insertion point moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assoc {
    /// Stay before inserted text.
    Before,
    /// Move behind inserted text.
    After,
}

/// Error for edits that cannot be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// A range ends before it starts or reaches past the end of the text.
    OutOfRange { start: usize, end: usize, len: usize },
    /// Two edits overlap.
    Overlap,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::OutOfRange { start, end, len } => write!(f, "range {start}..{end} outside text of length {len}"),
            EditError::Overlap => write!(f, "edits overlap"),
        }
    }
}

impl std::error::Error for EditError {}

/// Sorted, non-overlapping changes, all in the coordinates of the text before the set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    changes: Vec<Change>,
}

impl ChangeSet {
    /// Build from `(range, replacement)` pairs (any order) against `rope`. No-op pairs are dropped. Several pure
    /// insertions at the same position are kept in the given order.
    pub fn from_edits(rope: &Rope, edits: Vec<(Range<usize>, String)>) -> Result<Self, EditError> {
        let len = rope.len_chars();
        let mut edits: Vec<(usize, Range<usize>, String)> = edits.into_iter().enumerate().map(|(i, (r, t))| (i, r, t)).collect();
        for (_, r, _) in &edits {
            if r.start > r.end || r.end > len {
                return Err(EditError::OutOfRange { start: r.start, end: r.end, len });
            }
        }
        edits.sort_by_key(|(i, r, _)| (r.start, r.end, *i));
        let mut changes: Vec<Change> = Vec::with_capacity(edits.len());
        for (_, r, text) in edits {
            if r.is_empty() && text.is_empty() {
                continue;
            }
            if let Some(last) = changes.last_mut() {
                if r.start < last.end() {
                    return Err(EditError::Overlap);
                }
                // Two pure insertions at one position: join them (in the given order).
                if r.is_empty() && last.removed_len == 0 && r.start == last.start {
                    let mut ins = std::mem::take(&mut last.inserted);
                    ins.push_str(&text);
                    *last = Change::new(last.start, String::new(), ins);
                    continue;
                }
            }
            let removed: String = rope.slice(r.clone()).chars().collect();
            changes.push(Change::new(r.start, removed, text));
        }
        Ok(Self { changes })
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Apply to the rope (the rope must be the text the set was built against).
    pub fn apply(&self, rope: &mut Rope) {
        for c in self.changes.iter().rev() {
            if c.removed_len > 0 {
                rope.remove(c.start..c.end());
            }
            if c.inserted_len > 0 {
                rope.insert(c.start, &c.inserted);
            }
        }
    }

    /// The set that undoes this one (in the coordinates of the text after this set).
    pub fn invert(&self) -> ChangeSet {
        let mut delta: isize = 0;
        let mut out = Vec::with_capacity(self.changes.len());
        for c in &self.changes {
            let start = (c.start as isize + delta) as usize;
            out.push(Change {
                start,
                removed: c.inserted.clone(),
                inserted: c.removed.clone(),
                removed_len: c.inserted_len,
                inserted_len: c.removed_len,
            });
            delta += c.inserted_len as isize - c.removed_len as isize;
        }
        ChangeSet { changes: out }
    }

    /// Ranges of the inserted texts in the coordinates after the set (same order as `changes()`).
    pub fn inserted_ranges(&self) -> Vec<Range<usize>> {
        let mut delta: isize = 0;
        let mut out = Vec::with_capacity(self.changes.len());
        for c in &self.changes {
            let s = (c.start as isize + delta) as usize;
            out.push(s..s + c.inserted_len);
            delta += c.inserted_len as isize - c.removed_len as isize;
        }
        out
    }

    /// Where `pos` (old coordinates) ends up after the set. Positions inside a replaced range go to its start
    /// (`Before`) or behind the inserted text (`After`).
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        let mut delta: isize = 0;
        for c in &self.changes {
            if pos < c.start {
                break;
            }
            let new_start = (c.start as isize + delta) as usize;
            if c.removed_len == 0 {
                if pos == c.start {
                    return match assoc {
                        Assoc::Before => new_start,
                        Assoc::After => new_start + c.inserted_len,
                    };
                }
            } else if pos == c.start {
                return new_start;
            } else if pos < c.end() {
                return match assoc {
                    Assoc::Before => new_start,
                    Assoc::After => new_start + c.inserted_len,
                };
            }
            delta += c.inserted_len as isize - c.removed_len as isize;
        }
        (pos as isize + delta) as usize
    }

    /// Change of the text length in chars.
    pub fn len_delta(&self) -> isize {
        self.changes.iter().map(|c| c.inserted_len as isize - c.removed_len as isize).sum()
    }

    /// Rough memory use (for the history budget).
    pub fn heap_size(&self) -> usize {
        self.changes.iter().map(|c| c.removed.len() + c.inserted.len() + std::mem::size_of::<Change>()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edits(v: &[(usize, usize, &str)]) -> Vec<(Range<usize>, String)> {
        v.iter().map(|(a, b, t)| (*a..*b, t.to_string())).collect()
    }

    #[test]
    fn apply_and_invert() {
        let mut r = Rope::from_str("hello world");
        let cs = ChangeSet::from_edits(&r, edits(&[(6, 11, "there"), (0, 0, ">> "), (5, 5, ",")])).unwrap();
        cs.apply(&mut r);
        assert_eq!(r.to_string(), ">> hello, there");
        assert_eq!(cs.inserted_ranges(), vec![0..3, 8..9, 10..15]);
        cs.invert().apply(&mut r);
        assert_eq!(r.to_string(), "hello world");
    }

    #[test]
    fn overlap_and_range_errors() {
        let r = Rope::from_str("abc");
        assert_eq!(ChangeSet::from_edits(&r, edits(&[(0, 2, "x"), (1, 3, "y")])), Err(EditError::Overlap));
        assert!(matches!(ChangeSet::from_edits(&r, edits(&[(2, 9, "x")])), Err(EditError::OutOfRange { .. })));
    }

    #[test]
    fn joins_insertions_at_same_spot() {
        let mut r = Rope::from_str("ab");
        let cs = ChangeSet::from_edits(&r, edits(&[(1, 1, "X"), (1, 1, "Y")])).unwrap();
        assert_eq!(cs.changes().len(), 1);
        cs.apply(&mut r);
        assert_eq!(r.to_string(), "aXYb");
    }

    #[test]
    fn map_positions() {
        let r = Rope::from_str("0123456789");
        let cs = ChangeSet::from_edits(&r, edits(&[(2, 2, "ab"), (5, 7, "")])).unwrap();
        assert_eq!(cs.map_pos(1, Assoc::After), 1);
        assert_eq!(cs.map_pos(2, Assoc::Before), 2);
        assert_eq!(cs.map_pos(2, Assoc::After), 4);
        assert_eq!(cs.map_pos(5, Assoc::After), 7);
        assert_eq!(cs.map_pos(6, Assoc::Before), 7);
        assert_eq!(cs.map_pos(7, Assoc::Before), 7);
        assert_eq!(cs.map_pos(9, Assoc::Before), 9);
    }
}
