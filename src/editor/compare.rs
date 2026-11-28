//! Core of the file comparison (#385, 3.2.0): line diff with Patience (Myers inside and as fallback, time limit),
//! *Ignore whitespace*, intra-line changes (words, refined to characters for short words), alignment rows for the
//! side-by-side and unified views (filler lines are the missing side of a row), collapsing unchanged regions with
//! context, block edits (apply a block from one side to the other as one minimal edit), git-compatible patches and the
//! format differences shown in the info bar (line endings, encoding/BOM, final newline). No egui.
//!
//! Lines are compared **without** their line break; whether a text ends with a line break is kept separately
//! ([`Lines::final_newline`]) so that a missing final newline does not mark the last line as changed (the info bar says
//! it instead; patches still carry `\ No newline at end of file`).

use super::document::TextEdit;
use similar::{Algorithm, DiffOp};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Time limit while typing (live update of the diff tab, change markers): afterwards the result is coarser but correct.
pub const LIVE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Time limit for a comparison in the background (big files, with progress and *Cancel*).
pub const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(8);
/// Longer lines are not split into words (marked as a whole).
pub const INTRA_MAX_CHARS: usize = 2000;
/// Changed share above which a line is marked as a whole instead of word by word.
const INTRA_WHOLE_RATIO: f32 = 0.65;
/// Words up to this length are refined to characters.
const SHORT_WORD: usize = 12;

/// Lines of a text without line breaks (`\n`); a final line break does not make an extra empty line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lines<'a> {
    pub lines: Vec<&'a str>,
    /// The text ends with a line break (or is empty: `false`).
    pub final_newline: bool,
}

/// Split a text (with `\n` line breaks, like the editor's documents) into lines.
pub fn split_lines(text: &str) -> Lines<'_> {
    if text.is_empty() {
        return Lines { lines: Vec::new(), final_newline: false };
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    let final_newline = text.ends_with('\n');
    if final_newline {
        lines.pop();
    }
    Lines { lines, final_newline }
}

/// Join lines again (inverse of [`split_lines`]).
pub fn join_lines(lines: &[&str], final_newline: bool) -> String {
    let mut s = lines.join("\n");
    if final_newline && !lines.is_empty() {
        s.push('\n');
    }
    s
}

/// A changed block: lines `old` of the left (old) text became lines `new` of the right (new) text. One of the ranges
/// may be empty (pure insertion or deletion); its start is then the position between two lines.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

impl Hunk {
    pub fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (_, true) => HunkKind::Deleted,
            _ => HunkKind::Modified,
        }
    }
}

/// Kind of a block (for markers and the block list of `editor_diff`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HunkKind {
    Added,
    Deleted,
    Modified,
}

impl HunkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HunkKind::Added => "added",
            HunkKind::Deleted => "deleted",
            HunkKind::Modified => "modified",
        }
    }
}

/// Options of a comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Compare lines without any whitespace (like `git diff -w`).
    pub ignore_whitespace: bool,
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options { ignore_whitespace: false, timeout: LIVE_TIMEOUT }
    }
}

/// Progress (0–100) and cancelling of a comparison in the background.
#[derive(Debug, Default)]
pub struct Progress {
    pub cancel: AtomicBool,
    pub percent: AtomicU32,
}

impl Progress {
    fn set(&self, p: u32) {
        self.percent.store(p.min(100), Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Result of [`compare`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub hunks: Vec<Hunk>,
    /// The time limit was reached: the result is correct but coarser than possible.
    pub coarse: bool,
    pub cancelled: bool,
}

impl Outcome {
    /// Lines only on the right / only on the left.
    pub fn counts(&self) -> (usize, usize) {
        self.hunks.iter().fold((0, 0), |(a, d), h| (a + h.new.len(), d + h.old.len()))
    }
}

/// Line without whitespace (key for *Ignore whitespace*).
fn ws_key(line: &str) -> std::borrow::Cow<'_, str> {
    if line.chars().any(char::is_whitespace) { line.chars().filter(|c| !c.is_whitespace()).collect::<String>().into() } else { line.into() }
}

/// Compare two line lists. Lines are interned first (fast equality), common head and tail are skipped, the rest is
/// diffed with Patience (which uses Myers between its unique anchors and where there are none); after
/// `opts.timeout` the algorithm gives up and the result gets coarser (`coarse`), it is still a correct diff.
pub fn compare(old: &[&str], new: &[&str], opts: &Options, progress: Option<&Progress>) -> Outcome {
    let p = |v: u32| {
        if let Some(p) = progress {
            p.set(v);
        }
    };
    let cancelled = || progress.is_some_and(Progress::cancelled);
    // Zeilen in Nummern übersetzen (gleiche Zeile = gleiche Nummer): danach vergleicht der Algorithmus nur Zahlen.
    let mut ids: HashMap<std::borrow::Cow<'_, str>, u32> = HashMap::with_capacity((old.len() + new.len()).min(1 << 20));
    let total = (old.len() + new.len()).max(1);
    let mut a: Vec<u32> = Vec::with_capacity(old.len());
    let mut b: Vec<u32> = Vec::with_capacity(new.len());
    for (i, l) in old.iter().chain(new.iter()).enumerate() {
        let k = if opts.ignore_whitespace { ws_key(l) } else { std::borrow::Cow::Borrowed(*l) };
        let n = ids.len() as u32;
        let id = *ids.entry(k).or_insert(n);
        if i < old.len() { a.push(id) } else { b.push(id) }
        if i % 65536 == 0 {
            p((i * 40 / total) as u32);
            if cancelled() {
                return Outcome { cancelled: true, ..Default::default() };
            }
        }
    }
    drop(ids);
    p(40);
    // Gemeinsamer Anfang und gemeinsames Ende: schneller und eindeutig.
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let deadline = Instant::now() + opts.timeout;
    let ops = if ma.is_empty() || mb.is_empty() {
        let mut v = Vec::new();
        if !ma.is_empty() {
            v.push(DiffOp::Delete { old_index: 0, old_len: ma.len(), new_index: 0 });
        }
        if !mb.is_empty() {
            v.push(DiffOp::Insert { old_index: 0, new_index: 0, new_len: mb.len() });
        }
        v
    } else {
        similar::capture_diff_slices_deadline(Algorithm::Patience, ma, mb, Some(deadline))
    };
    let coarse = Instant::now() >= deadline;
    p(95);
    let hunks = hunks_from_ops(&ops, pre);
    p(100);
    Outcome { hunks, coarse, cancelled: false }
}

/// Changed blocks from the operations of `similar` (adjacent changes form one block), shifted by `offset` lines.
fn hunks_from_ops(ops: &[DiffOp], offset: usize) -> Vec<Hunk> {
    // Nur die Längen zählen: Die Positionen eines Einfügens nach einem Löschen meldet Patience nicht immer passend
    // zueinander; die Reihenfolge der Schritte stimmt.
    let mut out: Vec<Hunk> = Vec::new();
    let (mut i, mut j) = (offset, offset);
    for op in ops {
        let (a, b) = match *op {
            DiffOp::Equal { len, .. } => {
                i += len;
                j += len;
                continue;
            }
            DiffOp::Delete { old_len, .. } => (old_len, 0),
            DiffOp::Insert { new_len, .. } => (0, new_len),
            DiffOp::Replace { old_len, new_len, .. } => (old_len, new_len),
        };
        let (o, n) = (i..i + a, j..j + b);
        i += a;
        j += b;
        if let Some(last) = out.last_mut()
            && last.old.end == o.start
            && last.new.end == n.start
        {
            last.old.end = o.end;
            last.new.end = n.end;
            continue;
        }
        out.push(Hunk { old: o, new: n });
    }
    out
}

/// Compare two texts (`\n` line breaks).
pub fn compare_texts(old: &str, new: &str, opts: &Options) -> Outcome {
    compare(&split_lines(old).lines, &split_lines(new).lines, opts, None)
}

// ------------------------------------------------------------------------------------------------ Intra-line

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokClass {
    Word,
    Space,
    Other,
}

fn tok_class(c: char) -> TokClass {
    if c.is_alphanumeric() || c == '_' {
        TokClass::Word
    } else if c.is_whitespace() {
        TokClass::Space
    } else {
        TokClass::Other
    }
}

/// Tokens of a line: runs of word characters, runs of whitespace, single other characters (char ranges).
fn tokens(s: &str) -> Vec<(TokClass, Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut start_b = 0usize;
    let mut start_c = 0usize;
    let mut cur: Option<TokClass> = None;
    let mut ci = 0usize;
    for (bi, c) in s.char_indices() {
        let k = tok_class(c);
        let joins = cur == Some(k) && k != TokClass::Other;
        if !joins {
            if let Some(kc) = cur {
                out.push((kc, start_c..ci, &s[start_b..bi]));
            }
            cur = Some(k);
            start_b = bi;
            start_c = ci;
        }
        ci += 1;
    }
    if let Some(kc) = cur {
        out.push((kc, start_c..ci, &s[start_b..]));
    }
    out
}

fn push_range(v: &mut Vec<Range<usize>>, r: Range<usize>) {
    if r.is_empty() {
        return;
    }
    if let Some(last) = v.last_mut()
        && last.end >= r.start
    {
        last.end = last.end.max(r.end);
        return;
    }
    v.push(r);
}

/// Direkt aufeinanderfolgendes Löschen und Einfügen an derselben Stelle als Ersetzen (`similar` liefert sie getrennt).
fn pair_ops(ops: Vec<DiffOp>) -> Vec<DiffOp> {
    let mut out: Vec<DiffOp> = Vec::with_capacity(ops.len());
    for op in ops {
        if let (Some(DiffOp::Delete { old_index, old_len, new_index }), DiffOp::Insert { new_len, .. }) = (out.last().copied(), op) {
            out.pop();
            out.push(DiffOp::Replace { old_index, old_len, new_index, new_len });
            continue;
        }
        if let (Some(DiffOp::Insert { old_index, new_index, new_len }), DiffOp::Delete { old_len, .. }) = (out.last().copied(), op) {
            out.pop();
            out.push(DiffOp::Replace { old_index, old_len, new_index, new_len });
            continue;
        }
        out.push(op);
    }
    out
}

/// Changed parts of a line pair (char ranges in `a` and in `b`): word by word, short words refined to characters.
/// `None` = mark the lines as a whole (very many changes, or very long lines).
pub fn intraline(a: &str, b: &str, ignore_ws: bool) -> Option<(Vec<Range<usize>>, Vec<Range<usize>>)> {
    let (la, lb) = (a.chars().count(), b.chars().count());
    if la + lb > INTRA_MAX_CHARS * 2 || la > INTRA_MAX_CHARS || lb > INTRA_MAX_CHARS {
        return None;
    }
    if a == b {
        return Some((Vec::new(), Vec::new()));
    }
    let ta = tokens(a);
    let tb = tokens(b);
    let sa: Vec<&str> = ta.iter().map(|t| t.2).collect();
    let sb: Vec<&str> = tb.iter().map(|t| t.2).collect();
    let ops = pair_ops(similar::capture_diff_slices(Algorithm::Myers, &sa, &sb));
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    let mark = |v: &mut Vec<Range<usize>>, t: &(TokClass, Range<usize>, &str)| {
        if !(ignore_ws && t.0 == TokClass::Space) {
            push_range(v, t.1.clone());
        }
    };
    for op in &ops {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { old_index, old_len, .. } => ta[old_index..old_index + old_len].iter().for_each(|t| mark(&mut ra, t)),
            DiffOp::Insert { new_index, new_len, .. } => tb[new_index..new_index + new_len].iter().for_each(|t| mark(&mut rb, t)),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let (x, y) = (&ta[old_index], &tb[new_index]);
                let short = old_len == 1 && new_len == 1 && x.0 == TokClass::Word && y.0 == TokClass::Word && x.1.len() <= SHORT_WORD && y.1.len() <= SHORT_WORD;
                if short {
                    // Kurze Wörter: auf Zeichen verfeinern („color“ → „colour“ markiert nur das „u“).
                    let ca: Vec<char> = x.2.chars().collect();
                    let cb: Vec<char> = y.2.chars().collect();
                    for cop in pair_ops(similar::capture_diff_slices(Algorithm::Myers, &ca, &cb)) {
                        match cop {
                            DiffOp::Equal { .. } => {}
                            DiffOp::Delete { old_index: i, old_len: n, .. } => push_range(&mut ra, x.1.start + i..x.1.start + i + n),
                            DiffOp::Insert { new_index: i, new_len: n, .. } => push_range(&mut rb, y.1.start + i..y.1.start + i + n),
                            DiffOp::Replace { old_index: i, old_len: n, new_index: j, new_len: m } => {
                                push_range(&mut ra, x.1.start + i..x.1.start + i + n);
                                push_range(&mut rb, y.1.start + j..y.1.start + j + m);
                            }
                        }
                    }
                } else {
                    ta[old_index..old_index + old_len].iter().for_each(|t| mark(&mut ra, t));
                    tb[new_index..new_index + new_len].iter().for_each(|t| mark(&mut rb, t));
                }
            }
        }
    }
    let marked = |v: &[Range<usize>]| v.iter().map(|r| r.len()).sum::<usize>();
    let changed = (marked(&ra) + marked(&rb)) as f32 / (la + lb).max(1) as f32;
    if changed > INTRA_WHOLE_RATIO {
        return None;
    }
    Some((ra, rb))
}

// ------------------------------------------------------------------------------------------------ Rows

/// Kind of a row of the diff view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Equal,
    /// Changed line pair (side by side: left and right differ).
    Change,
    /// Only on the left (removed); side by side the right side is a filler line.
    Delete,
    /// Only on the right (added); side by side the left side is a filler line.
    Insert,
}

/// A row of the diff view: line on the left / right (0-based; `None` = filler line in side by side, or the other
/// column in unified), and the block it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub left: Option<u32>,
    pub right: Option<u32>,
    pub kind: RowKind,
    pub hunk: Option<u32>,
}

/// Rows for `n_old`/`n_new` lines and the blocks. Side by side: a block with `a` lines left and `b` right gets
/// `max(a, b)` rows (pairs first, then filler lines on the shorter side). Unified: the removed lines, then the added.
pub fn rows(hunks: &[Hunk], n_old: usize, n_new: usize, unified: bool) -> Vec<Row> {
    let mut out = Vec::with_capacity(n_old.max(n_new) + 16);
    let (mut i, mut j) = (0usize, 0usize);
    let eq = |out: &mut Vec<Row>, i: &mut usize, j: &mut usize, until_old: usize| {
        while *i < until_old {
            out.push(Row { left: Some(*i as u32), right: Some(*j as u32), kind: RowKind::Equal, hunk: None });
            *i += 1;
            *j += 1;
        }
    };
    for (k, h) in hunks.iter().enumerate() {
        eq(&mut out, &mut i, &mut j, h.old.start);
        let k = Some(k as u32);
        let (a, b) = (h.old.len(), h.new.len());
        if unified {
            for x in h.old.clone() {
                out.push(Row { left: Some(x as u32), right: None, kind: RowKind::Delete, hunk: k });
            }
            for y in h.new.clone() {
                out.push(Row { left: None, right: Some(y as u32), kind: RowKind::Insert, hunk: k });
            }
        } else {
            for r in 0..a.max(b) {
                let l = (r < a).then(|| (h.old.start + r) as u32);
                let rr = (r < b).then(|| (h.new.start + r) as u32);
                let kind = match (l, rr) {
                    (Some(_), Some(_)) => RowKind::Change,
                    (Some(_), None) => RowKind::Delete,
                    _ => RowKind::Insert,
                };
                out.push(Row { left: l, right: rr, kind, hunk: k });
            }
        }
        i = h.old.end;
        j = h.new.end;
    }
    eq(&mut out, &mut i, &mut j, n_old);
    let _ = n_new; // nach dem letzten Block sind beide Seiten gleich lang
    out
}

/// An entry of the shown list: a row, or a collapsed run of unchanged rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Row(usize),
    Fold { rows: Range<usize> },
}

/// Collapse unchanged regions: around each block `context` rows stay, the rest of a run becomes a [`Item::Fold`]
/// (only if it hides at least two rows). `revealed` = right line ranges the user expanded; `show_all` shows everything.
pub fn collapse(rows: &[Row], context: usize, revealed: &[Range<u32>], show_all: bool) -> Vec<Item> {
    if show_all {
        return (0..rows.len()).map(Item::Row).collect();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    let n = rows.len();
    let shown = |r: &Row| r.right.is_some_and(|x| revealed.iter().any(|rg| rg.contains(&x)));
    while i < n {
        if rows[i].kind != RowKind::Equal {
            out.push(Item::Row(i));
            i += 1;
            continue;
        }
        let s = i;
        while i < n && rows[i].kind == RowKind::Equal {
            i += 1;
        }
        let e = i;
        let keep_top = if s > 0 { context.min(e - s) } else { 0 };
        let keep_bot = if e < n { context.min(e - s) } else { 0 };
        let mut k = s;
        while k < e {
            let visible = k < s + keep_top || k >= e.saturating_sub(keep_bot) || shown(&rows[k]);
            if visible {
                out.push(Item::Row(k));
                k += 1;
                continue;
            }
            let fs = k;
            while k < e && !(k < s + keep_top || k >= e.saturating_sub(keep_bot) || shown(&rows[k])) {
                k += 1;
            }
            if k - fs >= 2 {
                out.push(Item::Fold { rows: fs..k });
            } else {
                (fs..k).for_each(|x| out.push(Item::Row(x)));
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ Block edits

/// Edit (char range in `target`, new text) that replaces the lines `lines` of `target` with `src` – keeps whether
/// `target` ends with a line break. One minimal edit (common head and tail stay untouched), so the cursors of other
/// views stay where they are.
pub fn replace_lines_edit(target: &str, lines: Range<usize>, src: &[&str]) -> TextEdit {
    let t = split_lines(target);
    let n = t.lines.len();
    let (s, e) = (lines.start.min(n), lines.end.min(n).max(lines.start.min(n)));
    let mut new_lines: Vec<&str> = Vec::with_capacity(n - (e - s) + src.len());
    new_lines.extend_from_slice(&t.lines[..s]);
    new_lines.extend_from_slice(src);
    new_lines.extend_from_slice(&t.lines[e..]);
    // Eine leere Datei, die eine Zeile bekommt: so wie die Quelle mit Zeilenende (sonst bliebe sie ohne).
    let fnl = if n == 0 { !new_lines.is_empty() } else { t.final_newline };
    let new = join_lines(&new_lines, fnl);
    minimal_edit(target, &new)
}

/// One edit turning `old` into `new` (common head and tail kept), char range in `old`.
pub fn minimal_edit(old: &str, new: &str) -> TextEdit {
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut p = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(p) || !new.is_char_boundary(p) {
        p -= 1;
    }
    let max_s = ob.len().min(nb.len()) - p;
    let mut s = ob.iter().rev().zip(nb.iter().rev()).take(max_s).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(ob.len() - s) || !new.is_char_boundary(nb.len() - s) {
        s -= 1;
    }
    let start = old[..p].chars().count();
    let len = old[p..ob.len() - s].chars().count();
    TextEdit { range: start..start + len, text: new[p..nb.len() - s].to_string() }
}

// ------------------------------------------------------------------------------------------------ Patches

/// Blocks for a patch: like `hunks`, but a different final line break changes the last line on both sides (a patch
/// must say `\ No newline at end of file`).
pub fn patch_hunks(hunks: &[Hunk], old: &Lines, new: &Lines) -> Vec<Hunk> {
    let (no, nn) = (old.lines.len(), new.lines.len());
    // Gleiche Zeile `i` links ↔ `j` rechts ist nur dann wirklich gleich, wenn beide gleich enden: die letzte Zeile ohne
    // Zeilenende ist eine andere als dieselbe Zeile mit Zeilenende.
    let open_end = |l: &Lines, i: usize| !l.final_newline && i + 1 == l.lines.len();
    let mut extra: Vec<(usize, usize)> = Vec::new();
    if no > 0
        && !old.final_newline
        && let Some(j) = map_line(hunks, no - 1, true)
        && !open_end(new, j)
    {
        extra.push((no - 1, j));
    }
    if nn > 0
        && !new.final_newline
        && let Some(i) = map_line(hunks, nn - 1, false)
        && !open_end(old, i)
        && !extra.contains(&(i, nn - 1))
    {
        extra.push((i, nn - 1));
    }
    let mut v = hunks.to_vec();
    for (i, j) in extra {
        v.push(Hunk { old: i..i + 1, new: j..j + 1 });
    }
    v.sort_by_key(|h| (h.old.start, h.new.start));
    let mut out: Vec<Hunk> = Vec::with_capacity(v.len());
    for h in v {
        if let Some(last) = out.last_mut()
            && last.old.end == h.old.start
            && last.new.end == h.new.start
        {
            last.old.end = h.old.end;
            last.new.end = h.new.end;
            continue;
        }
        out.push(h);
    }
    out
}

/// Gleiche Zeile auf der anderen Seite: `from_old` = Zeile `i` links → Zeile rechts (sonst umgekehrt); `None`, wenn die
/// Zeile in einem Block liegt.
pub fn map_line(hunks: &[Hunk], i: usize, from_old: bool) -> Option<usize> {
    let mut off: isize = 0;
    for h in hunks {
        let (src, dst) = if from_old { (&h.old, &h.new) } else { (&h.new, &h.old) };
        if src.contains(&i) {
            return None;
        }
        if src.end <= i && !(src.is_empty() && src.start > i) {
            off += dst.len() as isize - src.len() as isize;
        } else {
            break;
        }
    }
    usize::try_from(i as isize + off).ok()
}

/// Blocks after an edit on one side, without comparing again (#391: typing in the diff tab shows the text at once, the
/// real comparison follows after a pause). The lines `edit` of the edited side (`on_new` = right/new side) became
/// `new_len` lines. Blocks touching the edit merge with it into one block (the other side keeps its lines), later
/// blocks move by the difference in lines. The result is a valid, not necessarily minimal, comparison: everything
/// outside the blocks is still equal. Also returns the indices of the old blocks that were merged (`start` is the index
/// of the merged block in the result, if there is one).
pub fn edit_hunks(hunks: &[Hunk], on_new: bool, edit: Range<usize>, new_len: usize) -> (Vec<Hunk>, Range<usize>) {
    let side = |h: &Hunk| if on_new { h.new.clone() } else { h.old.clone() };
    let other = |h: &Hunk| if on_new { h.old.clone() } else { h.new.clone() };
    let end = edit.end.max(edit.start);
    let (mut lo, mut hi) = (edit.start, end);
    let i0 = hunks.partition_point(|h| side(h).end < lo);
    let mut i1 = i0;
    while i1 < hunks.len() && side(&hunks[i1]).start <= hi {
        let s = side(&hunks[i1]);
        lo = lo.min(s.start);
        hi = hi.max(s.end);
        i1 += 1;
    }
    // Zwischen dem Block davor und `lo` ist alles gleich: die Zeilen entsprechen sich eins zu eins.
    let olo = match i0.checked_sub(1).map(|i| &hunks[i]) {
        Some(p) => other(p).end + (lo - side(p).end),
        None => lo,
    };
    let (s_len, o_len) = hunks[i0..i1].iter().fold((0usize, 0usize), |(a, b), h| (a + side(h).len(), b + other(h).len()));
    let ohi = olo + (hi - lo) - s_len + o_len;
    let delta = new_len as isize - (end - edit.start) as isize;
    let shift = |r: Range<usize>| (r.start as isize + delta) as usize..(r.end as isize + delta) as usize;
    let new_side = lo..(hi as isize + delta) as usize;
    let mk = |s: Range<usize>, o: Range<usize>| if on_new { Hunk { old: o, new: s } } else { Hunk { old: s, new: o } };
    let mut out: Vec<Hunk> = Vec::with_capacity(hunks.len() + 1);
    out.extend_from_slice(&hunks[..i0]);
    if !(new_side.is_empty() && olo == ohi) {
        out.push(mk(new_side, olo..ohi));
    }
    out.extend(hunks[i1..].iter().map(|h| mk(shift(side(h)), other(h))));
    (out, i0..i1)
}

/// Lines `a..b` of `old` became lines `a..a + n` of `new`: common head and tail lines (`a + tail <= min(len)`).
pub fn changed_lines<S: AsRef<str>, T: AsRef<str>>(old: &[S], new: &[T]) -> (Range<usize>, usize) {
    changed_lines_by(old.len(), new.len(), |i, j| old[i].as_ref() == new[j].as_ref())
}

/// Wie [`changed_lines`] mit `eq(i, j)` = Zeile `i` alt gleich Zeile `j` neu.
pub fn changed_lines_by(n_old: usize, n_new: usize, eq: impl Fn(usize, usize) -> bool) -> (Range<usize>, usize) {
    let max = n_old.min(n_new);
    let head = (0..max).take_while(|&i| eq(i, i)).count();
    let tail = (0..max - head).take_while(|&k| eq(n_old - 1 - k, n_new - 1 - k)).count();
    (head..n_old - tail, n_new - tail - head)
}

/// Unified diff (`diff -u`, with `git` also the `diff --git` header) of the blocks with `context` lines.
pub fn unified(old: &Lines, new: &Lines, hunks: &[Hunk], context: usize, old_name: &str, new_name: &str, git: bool) -> String {
    let hunks = patch_hunks(hunks, old, new);
    if hunks.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if git {
        out.push_str(&format!("diff --git {old_name} {new_name}\n"));
    }
    out.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
    for g in group_hunks(&hunks, context, old.lines.len()) {
        out.push_str(&hunk_block(old, new, &hunks[g], context));
    }
    out
}

/// Blocks closer than `2 * context` lines are printed together (one `@@` header).
fn group_hunks(hunks: &[Hunk], context: usize, n_old: usize) -> Vec<Range<usize>> {
    let _ = n_old;
    let mut groups: Vec<Range<usize>> = Vec::new();
    for (i, h) in hunks.iter().enumerate() {
        match groups.last_mut() {
            Some(g) if h.old.start.saturating_sub(hunks[g.end - 1].old.end) <= 2 * context => g.end = i + 1,
            _ => groups.push(i..i + 1),
        }
    }
    groups
}

/// One `@@` section for neighbouring blocks.
pub fn hunk_block(old: &Lines, new: &Lines, hs: &[Hunk], context: usize) -> String {
    let Some(first) = hs.first() else { return String::new() };
    let last = hs.last().expect("nicht leer");
    let o0 = first.old.start.saturating_sub(context);
    let n0 = first.new.start - (first.old.start - o0);
    let o1 = (last.old.end + context).min(old.lines.len());
    let n1 = last.new.end + (o1 - last.old.end);
    let n1 = n1.min(new.lines.len());
    let range = |s: usize, e: usize| if e == s { format!("{s},0") } else if e - s == 1 { format!("{}", s + 1) } else { format!("{},{}", s + 1, e - s) };
    let mut out = format!("@@ -{} +{} @@\n", range(o0, o1), range(n0, n1));
    let no_nl = "\\ No newline at end of file\n";
    let line = |out: &mut String, sign: char, l: &Lines, i: usize| {
        out.push(sign);
        out.push_str(l.lines[i]);
        out.push('\n');
        if i + 1 == l.lines.len() && !l.final_newline {
            out.push_str(no_nl);
        }
    };
    let (mut i, mut j) = (o0, n0);
    for h in hs {
        while i < h.old.start {
            line(&mut out, ' ', old, i);
            i += 1;
        }
        for x in h.old.clone() {
            line(&mut out, '-', old, x);
        }
        for y in h.new.clone() {
            line(&mut out, '+', new, y);
        }
        i = h.old.end;
        j = h.new.end;
    }
    while i < o1 && j < n1 {
        // Gleiche letzte Zeile ohne Zeilenende auf beiden Seiten: einmal der Hinweis (Kontextzeile).
        out.push(' ');
        out.push_str(old.lines[i]);
        out.push('\n');
        if i + 1 == old.lines.len() && !old.final_newline {
            out.push_str(no_nl);
        }
        i += 1;
        j += 1;
    }
    out
}

// ------------------------------------------------------------------------------------------------ Format

/// What the info bar compares besides the text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Encoding label (`UTF-8`, `UTF-8 BOM`, …); `None` = unknown/not applicable (clipboard).
    pub encoding: Option<String>,
    /// Line endings: `LF`, `CRLF`, `CR`, `mixed` (`None` = no line break or not applicable).
    pub eol: Option<String>,
    pub final_newline: bool,
}

/// A difference besides the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FactDiff {
    Eol(String, String),
    Encoding(String, String),
    FinalNewline(bool, bool),
}

/// Differences of line endings, encoding (incl. BOM) and final line break.
pub fn fact_diffs(l: &Facts, r: &Facts) -> Vec<FactDiff> {
    let mut v = Vec::new();
    if let (Some(a), Some(b)) = (&l.eol, &r.eol)
        && a != b
    {
        v.push(FactDiff::Eol(a.clone(), b.clone()));
    }
    if let (Some(a), Some(b)) = (&l.encoding, &r.encoding)
        && a != b
    {
        v.push(FactDiff::Encoding(a.clone(), b.clone()));
    }
    if l.final_newline != r.final_newline {
        v.push(FactDiff::FinalNewline(l.final_newline, r.final_newline));
    }
    v
}

/// Line-ending label from the counts of a file.
pub fn eol_label(st: &super::encoding::EolStats) -> Option<String> {
    let kinds = [(st.lf, "LF"), (st.crlf, "CRLF"), (st.cr, "CR")];
    let used: Vec<&str> = kinds.iter().filter(|(n, _)| *n > 0).map(|(_, l)| *l).collect();
    match used.len() {
        0 => None,
        1 => Some(used[0].to_string()),
        _ => Some("mixed".to_string()),
    }
}

/// Looks like a binary file (NUL in the first 8000 bytes, like git), unless it is UTF-16 with a BOM.
pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    bytes.iter().take(8000).any(|b| *b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply the blocks: the new text must come out (every diff must be correct).
    fn apply(old: &[&str], new: &[&str], hunks: &[Hunk]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut i = 0;
        for h in hunks {
            assert!(h.old.start >= i, "sortiert und ohne Überlappung: {hunks:?}");
            out.extend(old[i..h.old.start].iter().map(|s| s.to_string()));
            out.extend(new[h.new.clone()].iter().map(|s| s.to_string()));
            i = h.old.end;
        }
        out.extend(old[i..].iter().map(|s| s.to_string()));
        out
    }

    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *seed >> 33
    }

    #[test]
    fn split_and_join_keep_the_final_newline_apart() {
        assert_eq!(split_lines(""), Lines { lines: vec![], final_newline: false });
        assert_eq!(split_lines("a"), Lines { lines: vec!["a"], final_newline: false });
        assert_eq!(split_lines("a\n"), Lines { lines: vec!["a"], final_newline: true });
        assert_eq!(split_lines("\n"), Lines { lines: vec![""], final_newline: true });
        assert_eq!(split_lines("a\n\nb"), Lines { lines: vec!["a", "", "b"], final_newline: false });
        for t in ["", "a", "a\n", "\n", "a\n\n", "x\ny"] {
            let l = split_lines(t);
            assert_eq!(join_lines(&l.lines, l.final_newline), t);
        }
        // Nur das letzte Zeilenende verschieden: keine geänderte Zeile, aber ein Hinweis.
        let o = compare_texts("a\nb", "a\nb\n", &Options::default());
        assert!(o.hunks.is_empty());
        let f = fact_diffs(&Facts { final_newline: false, ..Default::default() }, &Facts { final_newline: true, ..Default::default() });
        assert_eq!(f, vec![FactDiff::FinalNewline(false, true)]);
    }

    #[test]
    fn patience_diffs_are_correct_and_use_unique_anchors() {
        let old = ["fn a() {", "    x();", "}", "", "fn b() {", "    y();", "}"];
        let new = ["fn a() {", "    x();", "}", "", "fn c() {", "    z();", "}", "", "fn b() {", "    y();", "}"];
        let o = compare(&old, &new, &Options::default(), None);
        assert!(!o.coarse && !o.cancelled);
        assert_eq!(apply(&old, &new, &o.hunks), new.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(o.counts(), (4, 0), "nur die neue Funktion ist eingefügt");
        assert!(o.hunks.iter().all(|h| h.kind() == HunkKind::Added));
        // Zufällige Texte (auch ohne eindeutige Zeilen, wo Patience auf Myers zurückfällt): immer korrekt.
        let mut seed = 7u64;
        for _ in 0..200 {
            let gen_lines = |seed: &mut u64| -> Vec<String> { (0..(lcg(seed) % 30)).map(|_| ["a", "b", "c", "}", ""][(lcg(seed) % 5) as usize].to_string()).collect() };
            let a = gen_lines(&mut seed);
            let b = gen_lines(&mut seed);
            let (ra, rb): (Vec<&str>, Vec<&str>) = (a.iter().map(String::as_str).collect(), b.iter().map(String::as_str).collect());
            let o = compare(&ra, &rb, &Options::default(), None);
            assert_eq!(apply(&ra, &rb, &o.hunks), b, "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn time_limit_gives_a_coarse_but_correct_result_and_cancel_stops() {
        let old: Vec<String> = (0..3000).map(|i| format!("{}", i % 7)).collect();
        let new: Vec<String> = (0..3000).map(|i| format!("{}", (i * 3) % 11)).collect();
        let (a, b): (Vec<&str>, Vec<&str>) = (old.iter().map(String::as_str).collect(), new.iter().map(String::as_str).collect());
        let o = compare(&a, &b, &Options { ignore_whitespace: false, timeout: Duration::ZERO }, None);
        assert!(o.coarse, "Zeitgrenze schon vorbei");
        assert_eq!(apply(&a, &b, &o.hunks), new);
        let p = Progress::default();
        p.cancel.store(true, Ordering::Relaxed);
        assert!(compare(&a, &b, &Options::default(), Some(&p)).cancelled);
        let p = Progress::default();
        compare(&a, &b, &Options::default(), Some(&p));
        assert_eq!(p.percent.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn ignore_whitespace_hides_indentation_changes() {
        let old = "fn a() {\n  x();\n}\n";
        let new = "fn a() {\n\tx( );\n}\n";
        assert_eq!(compare_texts(old, new, &Options::default()).hunks.len(), 1);
        assert!(compare_texts(old, new, &Options { ignore_whitespace: true, ..Default::default() }).hunks.is_empty());
        // Echte Änderungen bleiben sichtbar.
        assert_eq!(compare_texts(old, "fn a() {\n  y();\n}\n", &Options { ignore_whitespace: true, ..Default::default() }).hunks.len(), 1);
    }

    #[test]
    fn intraline_marks_words_and_refines_short_words_to_chars() {
        // Kurzes Wort: nur das eingefügte „u“.
        let (a, b) = intraline("let color = 1;", "let colour = 1;", false).unwrap();
        assert!(a.is_empty());
        assert_eq!(b, vec![8..9]);
        // Ganze Wörter (lang)
        let (a, b) = intraline("let value = call computeTotalAmountNow(x) + offset;", "let value = call computeTotalAmountLater(x) + offset;", false).unwrap();
        assert_eq!(a, vec![17..38]);
        assert_eq!(b, vec![17..40]);
        // Viele Änderungen: ganze Zeile
        assert_eq!(intraline("alpha beta gamma", "one two three", false), None);
        // Gleich: nichts markiert
        assert_eq!(intraline("same", "same", false), Some((vec![], vec![])));
        // Leerraum ignorieren: nur Leerzeichen geändert → nichts markiert
        let (a, b) = intraline("a  = 1", "a = 1", true).unwrap();
        assert!(a.is_empty() && b.is_empty());
        // Umlaute: Zeichen, nicht Bytes
        let (_, b) = intraline("grün", "grüne", false).unwrap();
        assert_eq!(b, vec![4..5]);
        // Sehr lange Zeilen: ganz
        assert_eq!(intraline(&"x".repeat(INTRA_MAX_CHARS + 1), "y", false), None);
    }

    #[test]
    fn rows_align_side_by_side_with_fillers_and_unified() {
        let hunks = vec![Hunk { old: 1..3, new: 1..2 }, Hunk { old: 4..4, new: 3..5 }];
        let r = rows(&hunks, 5, 6, false);
        let kinds: Vec<RowKind> = r.iter().map(|x| x.kind).collect();
        use RowKind::*;
        assert_eq!(kinds, [Equal, Change, Delete, Equal, Insert, Insert, Equal]);
        assert_eq!((r[2].left, r[2].right), (Some(2), None), "Füllzeile rechts");
        assert_eq!((r[4].left, r[4].right), (None, Some(3)), "Füllzeile links");
        assert_eq!((r[6].left, r[6].right), (Some(4), Some(5)));
        assert_eq!(r[1].hunk, Some(0));
        assert_eq!(r[5].hunk, Some(1));
        let u = rows(&hunks, 5, 6, true);
        let kinds: Vec<RowKind> = u.iter().map(|x| x.kind).collect();
        assert_eq!(kinds, [Equal, Delete, Delete, Insert, Equal, Insert, Insert, Equal]);
        // Alle Zeilen beider Seiten kommen genau einmal vor.
        for rs in [&r, &u] {
            let l: Vec<u32> = rs.iter().filter_map(|x| x.left).collect();
            let rr: Vec<u32> = rs.iter().filter_map(|x| x.right).collect();
            assert_eq!(l, (0..5).collect::<Vec<_>>());
            assert_eq!(rr, (0..6).collect::<Vec<_>>());
        }
    }

    #[test]
    fn collapse_keeps_context_and_expands() {
        let old: Vec<String> = (0..40).map(|i| format!("l{i}")).collect();
        let mut new = old.clone();
        new[20] = "changed".into();
        let (a, b): (Vec<&str>, Vec<&str>) = (old.iter().map(String::as_str).collect(), new.iter().map(String::as_str).collect());
        let o = compare(&a, &b, &Options::default(), None);
        let r = rows(&o.hunks, 40, 40, false);
        let items = collapse(&r, 3, &[], false);
        // Falte oben (0..17), drei Zeilen, Änderung, drei Zeilen, Falte unten (24..40)
        assert_eq!(items.first(), Some(&Item::Fold { rows: 0..17 }));
        assert_eq!(items.last(), Some(&Item::Fold { rows: 24..40 }));
        assert_eq!(items.len(), 2 + 3 + 1 + 3);
        // +20 nach oben aufgeklappt (rechte Zeilen 0..17 sichtbar): oben keine Falte mehr
        let items = collapse(&r, 3, &[0..17], false);
        assert!(matches!(items.first(), Some(Item::Row(0))));
        // Alles zeigen
        assert_eq!(collapse(&r, 3, &[], true).len(), 40);
        // Kontext 0
        assert_eq!(collapse(&r, 0, &[], false).len(), 3);
        // Eine einzelne versteckte Zeile wird gezeigt statt gefaltet.
        let small = rows(&o.hunks[..], 40, 40, false);
        let items = collapse(&small[16..25], 3, &[], false);
        assert!(items.iter().all(|i| matches!(i, Item::Row(_))));
        // Gleiche Texte: alles in einer Falte
        let same = rows(&[], 10, 10, false);
        assert_eq!(collapse(&same, 3, &[], false), vec![Item::Fold { rows: 0..10 }]);
    }

    #[test]
    fn block_edits_both_ways_keep_the_final_newline() {
        let apply_edit = |t: &str, e: &TextEdit| {
            let mut c: Vec<char> = t.chars().collect();
            c.splice(e.range.clone(), e.text.chars());
            c.into_iter().collect::<String>()
        };
        let left = "a\nb\nc\n";
        let right = "a\nB\nc";
        let o = compare_texts(left, right, &Options::default());
        assert_eq!(o.hunks, vec![Hunk { old: 1..2, new: 1..2 }]);
        let (l, r) = (split_lines(left), split_lines(right));
        // links → rechts: rechts bleibt ohne Zeilenende am Schluss
        let e = replace_lines_edit(right, o.hunks[0].new.clone(), &l.lines[o.hunks[0].old.clone()]);
        assert_eq!(e, TextEdit { range: 2..3, text: "b".into() }, "minimal");
        assert_eq!(apply_edit(right, &e), "a\nb\nc");
        // rechts → links
        let e = replace_lines_edit(left, o.hunks[0].old.clone(), &r.lines[o.hunks[0].new.clone()]);
        assert_eq!(apply_edit(left, &e), "a\nB\nc\n");
        // Löschen am Ende ohne Zeilenende: kein neues Zeilenende
        assert_eq!(apply_edit("a\nb\nc", &replace_lines_edit("a\nb\nc", 1..3, &[])), "a");
        assert_eq!(apply_edit("a\nb\nc\n", &replace_lines_edit("a\nb\nc\n", 1..3, &[])), "a\n");
        // Einfügen am Ende, am Anfang, in eine leere Datei
        assert_eq!(apply_edit("a", &replace_lines_edit("a", 1..1, &["x", "y"])), "a\nx\ny");
        assert_eq!(apply_edit("a\n", &replace_lines_edit("a\n", 0..0, &["x"])), "x\na\n");
        assert_eq!(apply_edit("", &replace_lines_edit("", 0..0, &["x"])), "x\n");
        assert_eq!(apply_edit("x\n", &replace_lines_edit("x\n", 0..1, &[])), "");
        // Umlaute: Bereich in Zeichen
        let e = replace_lines_edit("ä\nö\n", 1..2, &["ü"]);
        assert_eq!(e.range, 2..3);
        assert_eq!(minimal_edit("same", "same"), TextEdit { range: 4..4, text: String::new() });
    }

    #[test]
    fn unified_patches_are_git_compatible() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        let new = "1\n2\nthree\n4\n5\n6\n7\n8\n9\n10\neleven";
        let (l, r) = (split_lines(old), split_lines(new));
        let o = compare(&l.lines, &r.lines, &Options::default(), None);
        let p = unified(&l, &r, &o.hunks, 3, "a/x.txt", "b/x.txt", true);
        assert!(p.starts_with("diff --git a/x.txt b/x.txt\n--- a/x.txt\n+++ b/x.txt\n@@ -1,6 +1,6 @@\n"), "{p}");
        assert!(p.contains("-3\n+three\n"));
        assert!(p.contains("@@ -8,3 +8,4 @@\n 8\n 9\n 10\n+eleven\n\\ No newline at end of file\n"), "{p}");
        // Nur das Zeilenende am Schluss verschieden: trotzdem ein Patch
        let (l, r) = (split_lines("a\nb\n"), split_lines("a\nb"));
        let p = unified(&l, &r, &[], 3, "a/f", "b/f", false);
        assert_eq!(p, "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n+b\n\\ No newline at end of file\n");
        // Links ohne Zeilenende, rechts kommt eine Zeile dazu: die alte letzte Zeile ändert sich mit.
        let (l, r) = (split_lines("a\nb"), split_lines("a\nb\nc\n"));
        let o = compare(&l.lines, &r.lines, &Options::default(), None);
        assert_eq!(unified(&l, &r, &o.hunks, 3, "a/f", "b/f", false), "--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n a\n-b\n\\ No newline at end of file\n+b\n+c\n");
        assert_eq!(map_line(&o.hunks, 1, true), Some(1));
        assert_eq!(map_line(&o.hunks, 2, false), None);
        // Gleich: leer
        let s = split_lines("x\n");
        assert_eq!(unified(&s, &s, &[], 3, "a", "b", true), "");
        // Einfügen in eine leere Datei
        let (l, r) = (split_lines(""), split_lines("x\n"));
        let o = compare(&l.lines, &r.lines, &Options::default(), None);
        assert_eq!(unified(&l, &r, &o.hunks, 3, "a/f", "b/f", false), "--- a/f\n+++ b/f\n@@ -0,0 +1 @@\n+x\n");
    }

    #[test]
    fn facts_and_binary_detection() {
        let st = |lf, crlf, cr| super::super::encoding::EolStats { lf, crlf, cr };
        assert_eq!(eol_label(&st(3, 0, 0)).as_deref(), Some("LF"));
        assert_eq!(eol_label(&st(0, 2, 0)).as_deref(), Some("CRLF"));
        assert_eq!(eol_label(&st(1, 2, 0)).as_deref(), Some("mixed"));
        assert_eq!(eol_label(&st(0, 0, 0)), None);
        let a = Facts { encoding: Some("UTF-8".into()), eol: Some("CRLF".into()), final_newline: true };
        let b = Facts { encoding: Some("UTF-8 BOM".into()), eol: Some("LF".into()), final_newline: true };
        assert_eq!(fact_diffs(&a, &b), vec![FactDiff::Eol("CRLF".into(), "LF".into()), FactDiff::Encoding("UTF-8".into(), "UTF-8 BOM".into())]);
        assert!(fact_diffs(&a, &Facts { encoding: None, eol: None, final_newline: true }).is_empty(), "Zwischenablage: nur das Zeilenende am Schluss");
        assert!(looks_binary(b"PK\x03\x04\0\0"));
        assert!(!looks_binary(b"plain text"));
        assert!(!looks_binary(&[0xFF, 0xFE, b'a', 0]), "UTF-16 mit BOM ist Text");
    }

    #[test]
    fn edited_blocks_stay_a_correct_comparison_without_comparing_again() {
        // Tippen in einer gleichen Zeile: neuer Block genau dort, spätere Blöcke unverändert.
        let old = ["a", "b", "c", "d"];
        let new = ["a", "b", "c", "D"];
        let hunks = compare(&old, &new, &Options::default(), None).hunks;
        assert_eq!(hunks, vec![Hunk { old: 3..4, new: 3..4 }]);
        let (h, merged) = edit_hunks(&hunks, true, 1..2, 1);
        assert_eq!(h, vec![Hunk { old: 1..2, new: 1..2 }, Hunk { old: 3..4, new: 3..4 }]);
        assert_eq!(merged, 0..0);
        // Zeilen einfügen links: der Block rechts davon wandert links mit.
        let (h, _) = edit_hunks(&hunks, false, 0..0, 2);
        assert_eq!(h, vec![Hunk { old: 0..2, new: 0..0 }, Hunk { old: 5..6, new: 3..4 }]);
        // Am Block: verschmilzt mit ihm.
        let (h, merged) = edit_hunks(&hunks, true, 3..4, 3);
        assert_eq!(h, vec![Hunk { old: 3..4, new: 3..6 }]);
        assert_eq!(merged, 0..1);
        // Eingefügte Zeilen wieder entfernt: kein Block mehr.
        let ins = vec![Hunk { old: 2..2, new: 2..4 }];
        assert_eq!(edit_hunks(&ins, true, 2..4, 0).0, vec![]);
        assert_eq!(changed_lines(&["a", "b", "c"], &["a", "x", "y", "c"]), (1..2, 2));
        assert_eq!(changed_lines(&["a", "a"], &["a"]), (1..2, 0));
        assert_eq!(changed_lines::<&str, &str>(&[], &["a"]), (0..0, 1));
        // Zufällig: alte Blöcke + Änderung auf einer Seite ergeben immer einen korrekten Vergleich.
        let mut seed = 11u64;
        for _ in 0..400 {
            let gen_lines = |seed: &mut u64, n: u64| -> Vec<String> { (0..(lcg(seed) % n)).map(|_| ["a", "b", "c", "}", ""][(lcg(seed) % 5) as usize].to_string()).collect() };
            let a = gen_lines(&mut seed, 25);
            let b = gen_lines(&mut seed, 25);
            let (ar, br): (Vec<&str>, Vec<&str>) = (a.iter().map(|s| s.as_str()).collect(), b.iter().map(|s| s.as_str()).collect());
            let hunks = compare(&ar, &br, &Options::default(), None).hunks;
            let on_new = lcg(&mut seed) % 2 == 0;
            let side = if on_new { &br } else { &ar };
            let s = (lcg(&mut seed) as usize) % (side.len() + 1);
            let e = s + (lcg(&mut seed) as usize) % (side.len() - s + 1);
            let ins = gen_lines(&mut seed, 4);
            let mut edited: Vec<&str> = side[..s].to_vec();
            edited.extend(ins.iter().map(|x| x.as_str()));
            edited.extend_from_slice(&side[e..]);
            let (ed_range, n) = changed_lines(side, &edited);
            let (h, _) = edit_hunks(&hunks, on_new, ed_range, n);
            let (o2, n2) = if on_new { (ar.clone(), edited.clone()) } else { (edited.clone(), br.clone()) };
            assert_eq!(apply(&o2, &n2, &h), n2.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{a:?} {b:?} {on_new} {s}..{e} {ins:?}");
            for w in h.windows(2) {
                assert!(w[0].old.end <= w[1].old.start && w[0].new.end <= w[1].new.start, "{h:?}");
            }
            assert!(h.iter().all(|x| !(x.old.is_empty() && x.new.is_empty())));
            // Außerhalb der Blöcke sind die Zeilen gleich (nicht nur das Ergebnis): jede Zeile außerhalb passt zur anderen Seite.
            for i in 0..o2.len() {
                if let Some(j) = map_line(&h, i, true) {
                    assert_eq!(o2[i], n2[j], "{h:?}");
                }
            }
        }
    }
}
