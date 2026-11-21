//! Line diffs (crate `similar`): minimal edits for reloads, and diff views for "file changed on disk".

use super::document::TextEdit;
use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};
use std::time::Duration;

/// Diffs of big texts give up after this time and fall back to a coarser result (still correct).
const DIFF_TIMEOUT: Duration = Duration::from_millis(1500);

fn line_diff<'a>(old: &'a str, new: &'a str) -> TextDiff<'a, 'a, 'a, str> {
    TextDiff::configure().algorithm(Algorithm::Myers).timeout(DIFF_TIMEOUT).diff_lines(old, new)
}

/// Edits (char ranges in `old`) that turn `old` into `new`, line by line.
pub fn minimal_edits(old: &str, new: &str) -> Vec<TextEdit> {
    let diff = line_diff(old, new);
    let old_lines: Vec<&str> = diff.old_slices().to_vec();
    let new_lines: Vec<&str> = diff.new_slices().to_vec();
    let mut starts = Vec::with_capacity(old_lines.len() + 1);
    let mut acc = 0usize;
    for l in &old_lines {
        starts.push(acc);
        acc += l.chars().count();
    }
    starts.push(acc);
    let mut out: Vec<TextEdit> = Vec::new();
    for op in diff.ops() {
        let (o, n) = match *op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete { old_index, old_len, new_index } => (old_index..old_index + old_len, new_index..new_index),
            DiffOp::Insert { old_index, new_index, new_len } => (old_index..old_index, new_index..new_index + new_len),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                (old_index..old_index + old_len, new_index..new_index + new_len)
            }
        };
        let text: String = new_lines[n].concat();
        let range = starts[o.start]..starts[o.end];
        // Join with a directly preceding edit (keeps the change set small).
        if let Some(last) = out.last_mut() {
            if last.range.end == range.start {
                last.range.end = range.end;
                last.text.push_str(&text);
                continue;
            }
        }
        out.push(TextEdit { range, text });
    }
    out
}

/// Kind of a diff line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Equal,
    /// Only in the new text (e.g. on disk).
    Insert,
    /// Only in the old text (e.g. the buffer).
    Delete,
}

/// One line of a side-by-side or inline diff view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// 0-based line in the old text.
    pub old_line: Option<usize>,
    /// 0-based line in the new text.
    pub new_line: Option<usize>,
    /// Line text without line break.
    pub text: String,
}

/// Counts for a short hint ("+3 −1 lines").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffSummary {
    pub inserted: usize,
    pub deleted: usize,
}

impl DiffSummary {
    pub fn is_empty(&self) -> bool {
        self.inserted == 0 && self.deleted == 0
    }
}

/// All lines of the diff `old` → `new`, `context` unchanged lines around changes (`None` = all lines).
pub fn diff_lines(old: &str, new: &str, context: Option<usize>) -> Vec<DiffLine> {
    let diff = line_diff(old, new);
    let mut out = Vec::new();
    let mut push_change = |c: similar::Change<&str>| {
        let kind = match c.tag() {
            ChangeTag::Equal => DiffKind::Equal,
            ChangeTag::Insert => DiffKind::Insert,
            ChangeTag::Delete => DiffKind::Delete,
        };
        let text = c.value().trim_end_matches('\n').trim_end_matches('\r').to_string();
        out.push(DiffLine { kind, old_line: c.old_index(), new_line: c.new_index(), text });
    };
    match context {
        None => diff.iter_all_changes().for_each(&mut push_change),
        Some(n) => {
            for group in diff.grouped_ops(n) {
                for op in group {
                    diff.iter_changes(&op).for_each(&mut push_change);
                }
            }
        }
    }
    out
}

/// Inserted/deleted line counts.
pub fn diff_summary(old: &str, new: &str) -> DiffSummary {
    let diff = line_diff(old, new);
    let mut s = DiffSummary::default();
    for c in diff.iter_all_changes() {
        match c.tag() {
            ChangeTag::Insert => s.inserted += 1,
            ChangeTag::Delete => s.deleted += 1,
            ChangeTag::Equal => {}
        }
    }
    s
}

/// Unified diff text (like `diff -u`) with `context` lines.
pub fn unified_diff(old: &str, new: &str, old_name: &str, new_name: &str, context: usize) -> String {
    line_diff(old, new).unified_diff().context_radius(context).header(old_name, new_name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn apply(old: &str, edits: &[TextEdit]) -> String {
        let mut r = Rope::from_str(old);
        for e in edits.iter().rev() {
            r.remove(e.range.clone());
            r.insert(e.range.start, &e.text);
        }
        r.to_string()
    }

    #[test]
    fn minimal_edits_round_trip() {
        let cases = [
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("a\nb\nc\n", "x\na\nb\nc\ny"),
            ("ä\nö\nü", "ä\nü"),
            ("", "new\n"),
            ("old\n", ""),
            ("same", "same"),
        ];
        for (old, new) in cases {
            let e = minimal_edits(old, new);
            assert_eq!(apply(old, &e), new, "{old:?} -> {new:?}");
        }
        assert_eq!(minimal_edits("a\nb\nc\n", "a\nB\nc\n"), vec![TextEdit::new(2..4, "B\n")]);
    }

    #[test]
    fn diff_views() {
        let old = "1\n2\n3\n4\n5\n6\n7\n";
        let new = "1\n2\n3\nfour\n5\n6\n7\n";
        let s = diff_summary(old, new);
        assert_eq!(s, DiffSummary { inserted: 1, deleted: 1 });
        let lines = diff_lines(old, new, Some(1));
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1], DiffLine { kind: DiffKind::Delete, old_line: Some(3), new_line: None, text: "4".into() });
        assert_eq!(lines[2].kind, DiffKind::Insert);
        assert_eq!(diff_lines(old, new, None).len(), 8);
        let u = unified_diff(old, new, "mine", "disk", 1);
        assert!(u.contains("--- mine") && u.contains("+++ disk") && u.contains("-4") && u.contains("+four"));
    }
}
