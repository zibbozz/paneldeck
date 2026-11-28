//! Change markers in the gutter (#385, 3.2.0): which lines of a document are added, modified or have lines removed
//! above them – against the git index (**unstaged** changes) and at the same time against HEAD (**staged** changes,
//! the lines where the buffer equals the index but the index differs from HEAD), each kind in its own colour. Outside a
//! repository (or for files git does not know) against the file on disk (unsaved changes). No egui; the UI computes
//! this in a background thread ~250 ms after typing.

use super::compare::{self, Hunk, HunkKind, Options};
use std::ops::Range;

/// A marked block of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkHunk {
    /// Lines of the document. Empty = lines were removed here (marker at the top edge of line `lines.start`, or below
    /// the last line).
    pub lines: Range<usize>,
    pub kind: HunkKind,
    /// Staged change (index ↔ HEAD); otherwise unstaged (document ↔ index) or unsaved (document ↔ disk).
    pub staged: bool,
    /// The lines of the base (index, HEAD or disk) that the block replaced – shown in the popup, used by *Revert*.
    pub old: Vec<String>,
    /// *Revert* is possible (a staged change whose lines were partly changed again cannot be reverted as a whole).
    pub revertable: bool,
}

/// All markers of a document, sorted by line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineMarks {
    pub hunks: Vec<MarkHunk>,
    /// Time limit reached (coarser markers).
    pub coarse: bool,
}

impl LineMarks {
    /// Block on line `line` (a block containing it; else a deletion marker at its top edge).
    pub fn at(&self, line: usize) -> Option<usize> {
        let i = self.hunks.partition_point(|h| h.lines.end <= line && !(h.lines.is_empty() && h.lines.start == line));
        let mut found = None;
        for (k, h) in self.hunks.iter().enumerate().skip(i.saturating_sub(1)) {
            if h.lines.start > line {
                break;
            }
            if h.lines.contains(&line) {
                return Some(k);
            }
            if h.lines.is_empty() && h.lines.start == line {
                found = Some(k);
            }
        }
        found
    }

    /// Next block after line `line` (`forward`) or before it, wrapping around. Liefert den Index.
    pub fn step(&self, line: usize, forward: bool) -> Option<usize> {
        if self.hunks.is_empty() {
            return None;
        }
        let cur = self.at(line);
        if forward {
            let i = self.hunks.iter().position(|h| h.lines.start > line && Some(h) != cur.map(|c| &self.hunks[c]));
            Some(i.unwrap_or(0))
        } else {
            let i = self.hunks.iter().rposition(|h| h.lines.start < line && Some(h) != cur.map(|c| &self.hunks[c]));
            Some(i.unwrap_or(self.hunks.len() - 1))
        }
    }
}

/// What the document is compared against.
#[derive(Clone, Copy, Debug)]
pub enum Base<'a> {
    /// Git: the file in the index (`None` = not in the index → no markers; the caller then compares with the disk) and
    /// in HEAD (`None` = no commit yet or new file: everything in the index counts as staged addition).
    Git { index: Option<&'a str>, head: Option<&'a str> },
    /// The file on disk (unsaved changes).
    Disk(&'a str),
}

fn mark(h: &Hunk, base: &[&str], staged: bool) -> MarkHunk {
    MarkHunk { lines: h.new.clone(), kind: h.kind(), staged, old: base[h.old.clone()].iter().map(|s| s.to_string()).collect(), revertable: true }
}

/// Markers of `buffer` (the document text, `\n` line breaks) against `base`.
pub fn classify(buffer: &str, base: Base, opts: &Options) -> LineMarks {
    let buf = compare::split_lines(buffer);
    let (index, head) = match base {
        Base::Disk(d) => {
            let d = compare::split_lines(d);
            let o = compare::compare(&d.lines, &buf.lines, opts, None);
            return LineMarks { hunks: o.hunks.iter().map(|h| mark(h, &d.lines, false)).collect(), coarse: o.coarse };
        }
        Base::Git { index: None, .. } => return LineMarks::default(),
        Base::Git { index: Some(i), head } => (i, head),
    };
    let idx = compare::split_lines(index);
    let a = compare::compare(&idx.lines, &buf.lines, opts, None);
    let mut hunks: Vec<MarkHunk> = a.hunks.iter().map(|h| mark(h, &idx.lines, false)).collect();
    let mut coarse = a.coarse;
    let head = compare::split_lines(head.unwrap_or(""));
    let b = compare::compare(&head.lines, &idx.lines, opts, None);
    coarse |= b.coarse;
    let unstaged_hits = |r: &Range<usize>, hunks: &[MarkHunk]| hunks.iter().any(|h| !h.staged && (h.lines.start < r.end.max(r.start + 1) && r.start < h.lines.end.max(h.lines.start + 1)));
    let mut staged: Vec<MarkHunk> = Vec::new();
    for hb in &b.hunks {
        let old: Vec<String> = head.lines[hb.old.clone()].iter().map(|s| s.to_string()).collect();
        if hb.new.is_empty() {
            // Im Index entfernte Zeilen: Stelle im Dokument (liegt sie in einer neuen Änderung, zeigt die schon etwas).
            let p = hb.new.start;
            if a.hunks.iter().any(|h| h.old.contains(&p)) {
                continue;
            }
            let q = map_pos(&a.hunks, p);
            let r = q..q;
            if !unstaged_hits(&r, &hunks) {
                staged.push(MarkHunk { lines: r, kind: HunkKind::Deleted, staged: true, old, revertable: true });
            }
            continue;
        }
        // Zeilen des Index auf das Dokument abbilden; geänderte (ungestagte) fallen heraus.
        let mapped: Vec<Option<usize>> = hb.new.clone().map(|j| compare::map_line(&a.hunks, j, true)).collect();
        let whole = mapped.iter().all(Option::is_some) && mapped.windows(2).all(|w| w[1] == w[0].map(|x| x + 1));
        let mut runs: Vec<Range<usize>> = Vec::new();
        for m in mapped.iter().flatten() {
            match runs.last_mut() {
                Some(r) if r.end == *m => r.end += 1,
                _ => runs.push(*m..*m + 1),
            }
        }
        for r in runs {
            if unstaged_hits(&r, &hunks) {
                continue;
            }
            staged.push(MarkHunk { lines: r, kind: hb.kind(), staged: true, old: old.clone(), revertable: whole });
        }
    }
    hunks.extend(staged);
    hunks.sort_by_key(|h| (h.lines.start, h.lines.end));
    LineMarks { hunks, coarse }
}

/// Position (zwischen Zeilen) `p` des Index im Dokument.
fn map_pos(hunks: &[Hunk], p: usize) -> usize {
    let mut off: isize = 0;
    for h in hunks {
        if h.old.end <= p {
            off += h.new.len() as isize - h.old.len() as isize;
        } else {
            break;
        }
    }
    (p as isize + off).max(0) as usize
}

/// Edit for *Revert* of block `h` in `buffer` (one undo step for the caller).
pub fn revert_edit(buffer: &str, h: &MarkHunk) -> super::document::TextEdit {
    let old: Vec<&str> = h.old.iter().map(String::as_str).collect();
    compare::replace_lines_edit(buffer, h.lines.clone(), &old)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(m: &LineMarks) -> Vec<(Range<usize>, HunkKind, bool)> {
        m.hunks.iter().map(|h| (h.lines.clone(), h.kind, h.staged)).collect()
    }

    fn apply(t: &str, e: &crate::editor::TextEdit) -> String {
        let mut c: Vec<char> = t.chars().collect();
        c.splice(e.range.clone(), e.text.chars());
        c.into_iter().collect()
    }

    #[test]
    fn disk_base_marks_unsaved_changes() {
        let disk = "a\nb\nc\nd\n";
        let buf = "a\nB\nc\nnew\nd\n";
        let m = classify(buf, Base::Disk(disk), &Options::default());
        assert_eq!(kinds(&m), vec![(1..2, HunkKind::Modified, false), (3..4, HunkKind::Added, false)]);
        assert_eq!(m.hunks[0].old, vec!["b".to_string()]);
        let m = classify("a\nd\n", Base::Disk(disk), &Options::default());
        assert_eq!(kinds(&m), vec![(1..1, HunkKind::Deleted, false)]);
        assert_eq!(m.at(1), Some(0), "Löschmarke am oberen Rand von Zeile 2");
        assert_eq!(m.hunks[0].old, vec!["b".to_string(), "c".to_string()]);
        assert!(classify(disk, Base::Disk(disk), &Options::default()).hunks.is_empty());
    }

    #[test]
    fn git_base_tells_staged_from_unstaged() {
        let head = "one\ntwo\nthree\nfour\nfive\n";
        // Gestagt: „two“ → „TWO“, „four“ entfernt. Danach im Dokument: Zeile „six“ angehängt, „one“ geändert.
        let index = "one\nTWO\nthree\nfive\n";
        let buf = "ONE\nTWO\nthree\nfive\nsix\n";
        let m = classify(buf, Base::Git { index: Some(index), head: Some(head) }, &Options::default());
        assert_eq!(
            kinds(&m),
            vec![(0..1, HunkKind::Modified, false), (1..2, HunkKind::Modified, true), (3..3, HunkKind::Deleted, true), (4..5, HunkKind::Added, false)],
            "{m:?}"
        );
        let staged_mod = &m.hunks[1];
        assert_eq!(staged_mod.old, vec!["two".to_string()], "das Popup zeigt den Stand in HEAD");
        assert!(staged_mod.revertable);
        // Neue Datei im Index (kein HEAD): alles gestagt hinzugefügt.
        let m = classify("x\ny\n", Base::Git { index: Some("x\ny\n"), head: None }, &Options::default());
        assert_eq!(kinds(&m), vec![(0..2, HunkKind::Added, true)]);
        // Nicht im Index: keine Marken (der Aufrufer vergleicht dann mit der Platte).
        assert!(classify("x", Base::Git { index: None, head: Some("x") }, &Options::default()).hunks.is_empty());
        // Gestagte Änderung, danach teilweise wieder geändert: Teil bleibt gestagt, aber nicht als Ganzes umkehrbar.
        let head = "a\nb\nc\nd\n";
        let index = "a\nB1\nB2\nB3\nd\n";
        let buf = "a\nB1\nchanged\nB3\nd\n";
        let m = classify(buf, Base::Git { index: Some(index), head: Some(head) }, &Options::default());
        let staged: Vec<_> = m.hunks.iter().filter(|h| h.staged).collect();
        assert_eq!(staged.len(), 2);
        assert!(staged.iter().all(|h| !h.revertable));
        assert!(m.hunks.iter().any(|h| !h.staged && h.lines == (2..3)));
    }

    #[test]
    fn revert_restores_the_base_lines_and_steps_wrap() {
        let disk = "a\nb\nc\nd\ne\n";
        let buf = "a\nX\nc\ne\nextra\n";
        let m = classify(buf, Base::Disk(disk), &Options::default());
        assert_eq!(m.hunks.len(), 3);
        let mut t = buf.to_string();
        // Von hinten nach vorn umkehren: die Zeilen davor bleiben gültig.
        for h in m.hunks.iter().rev() {
            t = apply(&t, &revert_edit(&t, h));
        }
        assert_eq!(t, disk);
        assert_eq!(m.step(0, true), Some(0));
        assert_eq!(m.step(1, true), Some(1), "von einer Änderung zur nächsten");
        assert_eq!(m.step(4, true), Some(0), "am Ende wieder oben");
        assert_eq!(m.step(0, false), Some(2), "rückwärts am Anfang: die letzte");
        assert_eq!(LineMarks::default().step(3, true), None);
    }
}
