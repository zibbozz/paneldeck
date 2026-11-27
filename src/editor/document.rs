//! The document: rope text, undo history, file format, dirty state and agent marks.

use super::change::{Assoc, ChangeSet, EditError};
use super::editorconfig::EditorConfig;
use super::encoding::{EolStats, LineEnding, TextEncoding, normalize_eol};
use super::file::DiskSnapshot;
use super::history::{EditKind, EditOrigin, History, HistoryLimits};
use super::indent::IndentSettings;
use super::selection::{Selection, Selections};
use super::text::{self, LineCol};
use ropey::{Rope, RopeSlice};
use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default size from which heavy features are switched off (bytes of the file / text).
pub const DEFAULT_LARGE_FILE_THRESHOLD: u64 = 20 * 1024 * 1024;

/// How many change sets are kept for syncing other views of the same document.
const LOG_CAPACITY: usize = 1024;

/// One replacement for [`Document::apply_edits`]: replace the char range by `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
}

impl TextEdit {
    pub fn new(range: Range<usize>, text: impl Into<String>) -> Self {
        Self { range, text: text.into() }
    }

    pub fn insert(pos: usize, text: impl Into<String>) -> Self {
        Self { range: pos..pos, text: text.into() }
    }
}

/// Encoding and line ending used when the document is saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileFormat {
    pub encoding: TextEncoding,
    pub eol: LineEnding,
}

/// What was found when the text was loaded (for info bars / status bar).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadInfo {
    /// Line endings found in the file (mixed → `eol` is the majority).
    pub eol_stats: EolStats,
    /// Invalid byte sequences were replaced with U+FFFD.
    pub lossy: bool,
    /// The file contains NUL bytes (probably binary).
    pub looks_binary: bool,
    /// Size of the file in bytes (0 for new documents).
    pub file_size: u64,
}

/// Which heavy features the UI should switch off for a large document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeavyFeatures {
    pub minimap: bool,
    pub word_wrap: bool,
    /// Syntax colours (3.1.0).
    pub syntax: bool,
    /// Highlighting all occurrences of the word under the cursor / of search matches outside the viewport.
    pub occurrence_highlight: bool,
}

impl HeavyFeatures {
    /// All on for normal documents, all off for large ones.
    pub fn for_large(large: bool) -> Self {
        let on = !large;
        Self { minimap: on, word_wrap: on, syntax: on, occurrence_highlight: on }
    }
}

/// A text document. Positions are char indices; the text uses `\n` only (see [`FileFormat`] for the file).
///
/// Selections are not stored here: every view owns a [`Selections`] and passes it to the editing functions, which
/// first [`sync`](Document::sync) it with edits made through other views.
pub struct Document {
    rope: Rope,
    revision: u64,
    log: VecDeque<(u64, Arc<ChangeSet>)>,
    pub(crate) history: History,
    /// State id that matches the file on disk (`None` = never saved / restored from hot exit).
    saved_state: Option<u64>,
    /// Encoding or line ending changed since the last save.
    format_dirty: bool,
    pub(crate) path: Option<PathBuf>,
    pub(crate) format: FileFormat,
    /// The encoding was chosen by the user ("reopen with encoding"); reloads keep it instead of detecting.
    pub(crate) encoding_forced: bool,
    pub(crate) indent: IndentSettings,
    pub(crate) editorconfig: EditorConfig,
    /// Remove trailing whitespace when saving (from `.editorconfig` or settings).
    pub trim_trailing_whitespace: bool,
    /// Make sure the file ends with a line break when saving (`Some(false)` = remove a final line break).
    pub insert_final_newline: Option<bool>,
    pub(crate) disk: Option<DiskSnapshot>,
    pub(crate) load_info: LoadInfo,
    large: bool,
    agent_marks: Vec<Range<usize>>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("path", &self.path)
            .field("len_chars", &self.rope.len_chars())
            .field("revision", &self.revision)
            .field("format", &self.format)
            .field("dirty", &self.is_dirty())
            .finish()
    }
}

impl Document {
    /// Empty untitled document (UTF-8, native line ending).
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            revision: 0,
            log: VecDeque::new(),
            history: History::default(),
            saved_state: Some(0),
            format_dirty: false,
            path: None,
            format: FileFormat { encoding: TextEncoding::Utf8, eol: LineEnding::native() },
            encoding_forced: false,
            indent: IndentSettings::default(),
            editorconfig: EditorConfig::default(),
            trim_trailing_whitespace: false,
            insert_final_newline: None,
            disk: None,
            load_info: LoadInfo::default(),
            large: false,
            agent_marks: Vec::new(),
        }
    }

    /// Untitled document with this text (any line endings; the majority becomes the save format). Not dirty.
    pub fn from_text(text: &str) -> Self {
        let (t, stats) = normalize_eol(text);
        let mut d = Self::new();
        d.rope = Rope::from_str(&t);
        d.format.eol = stats.majority().unwrap_or_else(LineEnding::native);
        d.load_info.eol_stats = stats;
        if let Some(style) = super::indent::detect_indent(&d.rope) {
            d.indent.style = style;
            d.indent.source = super::indent::IndentSource::Detected;
        }
        d
    }

    /// Internal: build from already normalised text.
    pub(crate) fn from_normalized(text: String) -> Self {
        let mut d = Self::new();
        d.rope = Rope::from_str(&text);
        d
    }

    // ---- text access (no copies of the whole text) ----

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Whole text as a `String` (copies; avoid for large documents).
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Size of the text in UTF-8 bytes (with `\n` line breaks).
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// Number of lines (a text ending in `\n` has an empty last line, as editors show it).
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line `i` without its `\n`.
    pub fn line(&self, i: usize) -> RopeSlice<'_> {
        text::line_slice(&self.rope, i)
    }

    /// Line `i` without its `\n`, as a `String`.
    pub fn line_string(&self, i: usize) -> String {
        self.line(i).to_string()
    }

    /// Length of line `i` in chars (without `\n`).
    pub fn line_len(&self, i: usize) -> usize {
        text::line_len(&self.rope, i)
    }

    /// Part of line `i` by char columns (clamped), for very long lines.
    pub fn line_part(&self, i: usize, cols: Range<usize>) -> RopeSlice<'_> {
        let l = self.line(i);
        let n = l.len_chars();
        let s = cols.start.min(n);
        let e = cols.end.clamp(s, n);
        l.slice(s..e)
    }

    /// Lines `first..first + count` (clamped) for rendering a viewport: `(line index, line without \n)`.
    pub fn visible_lines(&self, first: usize, count: usize) -> impl Iterator<Item = (usize, RopeSlice<'_>)> + '_ {
        let total = self.len_lines();
        let first = first.min(total);
        let n = count.min(total - first);
        self.rope.lines_at(first).take(n).enumerate().map(move |(k, l)| {
            let len = l.len_chars();
            let l = if len > 0 && l.char(len - 1) == '\n' { l.slice(..len - 1) } else { l };
            (first + k, l)
        })
    }

    pub fn slice(&self, range: Range<usize>) -> RopeSlice<'_> {
        let len = self.len_chars();
        self.rope.slice(range.start.min(len)..range.end.min(len))
    }

    pub fn slice_string(&self, range: Range<usize>) -> String {
        self.slice(range).to_string()
    }

    pub fn char_at(&self, pos: usize) -> Option<char> {
        (pos < self.len_chars()).then(|| self.rope.char(pos))
    }

    pub fn char_to_line(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos.min(self.len_chars()))
    }

    /// First char of line `line` (clamped to the last line).
    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.len_lines().saturating_sub(1)))
    }

    pub fn char_to_byte(&self, pos: usize) -> usize {
        self.rope.char_to_byte(pos.min(self.len_chars()))
    }

    /// Byte offset (UTF-8, `\n` text) → char index; offsets inside a char round down.
    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte.min(self.len_bytes()))
    }

    pub fn pos_to_linecol(&self, pos: usize) -> LineCol {
        text::pos_to_linecol(&self.rope, pos)
    }

    pub fn linecol_to_pos(&self, lc: LineCol) -> usize {
        text::linecol_to_pos(&self.rope, lc)
    }

    /// Visual column (tabs expanded with the document's tab width) of `pos`.
    pub fn visual_col(&self, pos: usize) -> usize {
        let lc = self.pos_to_linecol(pos);
        text::visual_col(&self.rope, lc.line, lc.col, self.indent.tab_width)
    }

    /// Position for a click at visual column `vcol` of `line`.
    pub fn pos_at_visual(&self, line: usize, vcol: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        self.rope.line_to_char(line) + text::col_for_visual(&self.rope, line, vcol, self.indent.tab_width)
    }

    // ---- metadata ----

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// File name for tab titles (`None` for untitled documents).
    #[allow(dead_code)] // API (die Oberfläche bildet Titel selbst)
    pub fn file_name(&self) -> Option<String> {
        self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())
    }

    pub fn format(&self) -> FileFormat {
        self.format
    }

    pub fn encoding(&self) -> TextEncoding {
        self.format.encoding
    }

    pub fn eol(&self) -> LineEnding {
        self.format.eol
    }

    /// Format for saving plus its dirty flag – to put both back when a save fails after the actions on save changed them.
    pub fn format_state(&self) -> (FileFormat, bool) {
        (self.format, self.format_dirty)
    }

    /// Put back what [`Document::format_state`] returned.
    pub fn restore_format_state(&mut self, s: (FileFormat, bool)) {
        self.format = s.0;
        self.format_dirty = s.1;
    }

    /// Save with another encoding (marks the document dirty; the text is unchanged). To re-read the file in another
    /// encoding use [`Document::reload_with_encoding`].
    pub fn set_encoding(&mut self, enc: TextEncoding) {
        if self.format.encoding != enc {
            self.format.encoding = enc;
            self.format_dirty = true;
        }
    }

    /// Save with another line ending (marks the document dirty).
    pub fn set_eol(&mut self, eol: LineEnding) {
        if self.format.eol != eol {
            self.format.eol = eol;
            self.format_dirty = true;
        }
    }

    pub fn indent_settings(&self) -> IndentSettings {
        self.indent
    }

    pub fn set_indent_settings(&mut self, indent: IndentSettings) {
        self.indent = indent;
    }

    pub fn tab_width(&self) -> usize {
        self.indent.tab_width.max(1)
    }

    /// Properties from `.editorconfig` found when the document was opened.
    #[allow(dead_code)] // API (Anzeige der .editorconfig-Werte, später)
    pub fn editorconfig(&self) -> &EditorConfig {
        &self.editorconfig
    }

    pub fn load_info(&self) -> &LoadInfo {
        &self.load_info
    }

    /// Snapshot of the file on disk at the last load/save (for change detection).
    pub fn disk_snapshot(&self) -> Option<&DiskSnapshot> {
        self.disk.as_ref()
    }

    /// Accept the current file on disk as known (e.g. "Keep mine": no new prompt for the same change).
    #[allow(dead_code)] // API (die Oberfläche nutzt die Konfliktwahl in `file.rs`)
    pub fn acknowledge_disk(&mut self, snap: DiskSnapshot) {
        self.disk = Some(snap);
    }

    /// Is this a large document (heavy features off)?
    pub fn is_large(&self) -> bool {
        self.large
    }

    /// Recompute the large flag against `threshold` bytes (file size on load, text size later).
    pub fn update_large(&mut self, threshold: u64) -> bool {
        let size = (self.len_bytes() as u64).max(self.load_info.file_size);
        self.large = size >= threshold;
        self.large
    }

    pub fn heavy_features(&self) -> HeavyFeatures {
        HeavyFeatures::for_large(self.large)
    }

    // ---- dirty state / revisions ----

    /// Counter that increases with every change of the text (edits, undo, redo). For caches in the UI.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Id of the current text state; equal ids mean equal text (undo back to a state gives its id again).
    pub fn state_id(&self) -> u64 {
        self.history.state_id()
    }

    /// Unsaved changes (text or format)?
    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.history.state_id()) || self.format_dirty
    }

    /// The current text matches the file (after save or load).
    pub(crate) fn mark_saved(&mut self) {
        self.saved_state = Some(self.history.state_id());
        self.format_dirty = false;
        self.history.break_group();
    }

    /// The text state `state` (from [`Document::state_id`]) matches the file – after a save that ran in the background
    /// while the user may have kept typing (then the document stays dirty).
    pub(crate) fn mark_saved_state(&mut self, state: u64) {
        self.saved_state = Some(state);
        self.format_dirty = false;
        self.history.break_group();
    }

    /// Mark as unsaved (restored from hot exit).
    pub(crate) fn mark_unsaved(&mut self) {
        self.saved_state = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Origin of the step that undo would revert next.
    pub fn undo_origin(&self) -> Option<EditOrigin> {
        self.history.undo.last().map(|t| t.origin)
    }

    /// The next edit starts a new undo step (call when the cursor was moved by click/keys, focus changed …).
    pub fn break_undo_group(&mut self) {
        self.history.break_group();
    }

    /// Join all undo steps made since the state `mark` (a [`Document::state_id`]) into one, so a single undo takes them
    /// back together (the actions on save before writing).
    pub fn join_undo_since(&mut self, mark: u64) {
        self.history.join_since(mark);
    }

    pub fn set_history_limits(&mut self, limits: HistoryLimits) {
        self.history.limits = limits;
    }

    /// Forget undo/redo (e.g. after loading another file into the document).
    #[allow(dead_code)] // API
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    // ---- agent marks ----

    /// Text ranges inserted by agents (sorted, non-overlapping); the UI shows them as "by agent".
    pub fn agent_marks(&self) -> &[Range<usize>] {
        &self.agent_marks
    }

    #[allow(dead_code)] // API
    pub fn clear_agent_marks(&mut self) {
        self.agent_marks.clear();
    }

    /// Smallest char position changed since revision `rev` (`None` = nothing changed). Positions before it are the
    /// same text as at `rev`. When the change log no longer reaches back that far (or the text was replaced), `Some(0)`.
    /// For caches that follow the text, e.g. syntax colours (#312).
    pub fn changed_since(&self, rev: u64) -> Option<usize> {
        if rev >= self.revision {
            return None;
        }
        if !self.log.front().is_some_and(|(first, _)| *first <= rev + 1) {
            return Some(0);
        }
        let first = self.log.iter().filter(|(r, _)| *r > rev).filter_map(|(_, cs)| cs.changes().first().map(|c| c.start)).min();
        Some(first.unwrap_or(0))
    }

    // ---- syncing views ----

    /// Selections for a new view: a cursor at `pos`, synced to the current revision.
    pub fn new_selections(&self, pos: usize) -> Selections {
        let mut s = Selections::cursor(pos.min(self.len_chars()));
        s.rev = Some(self.revision);
        s
    }

    /// Bring a view's selections up to date with edits made since it last saw the document (through another
    /// view, an agent, undo …). Cheap if nothing changed. Call before using positions of a view.
    pub fn sync(&self, sels: &mut Selections) {
        if let Some(r) = sels.rev
            && r < self.revision
            && self.log.front().is_some_and(|(first, _)| *first <= r + 1)
        {
            for (rev, cs) in self.log.iter() {
                if *rev > r {
                    let cs = cs.clone();
                    sels.transform(|s| map_selection(&cs, s));
                }
            }
        }
        sels.clamp(self.len_chars());
        sels.rev = Some(self.revision);
    }

    // ---- applying edits ----

    fn apply_raw(&mut self, cs: Arc<ChangeSet>, origin: EditOrigin) {
        cs.apply(&mut self.rope);
        self.revision += 1;
        if self.log.len() >= LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back((self.revision, cs.clone()));
        // Agent marks follow the text; an agent edit adds its inserted ranges.
        let mut marks: Vec<Range<usize>> = self
            .agent_marks
            .iter()
            .map(|m| cs.map_pos(m.start, Assoc::After)..cs.map_pos(m.end, Assoc::Before))
            .filter(|m| m.end > m.start)
            .collect();
        if origin == EditOrigin::Agent {
            marks.extend(cs.inserted_ranges().into_iter().filter(|r| !r.is_empty()));
            marks.sort_by_key(|r| r.start);
            let mut merged: Vec<Range<usize>> = Vec::with_capacity(marks.len());
            for m in marks {
                match merged.last_mut() {
                    Some(last) if m.start <= last.end => last.end = last.end.max(m.end),
                    _ => merged.push(m),
                }
            }
            marks = merged;
        }
        self.agent_marks = marks;
    }

    /// Apply a change set as one (possibly merged) undo step and set the view's selections to `after`.
    pub(crate) fn commit(
        &mut self,
        sels: &mut Selections,
        cs: ChangeSet,
        mut after: Selections,
        origin: EditOrigin,
        kind: EditKind,
    ) -> bool {
        if cs.is_empty() {
            return false;
        }
        let before = sels.clone();
        let cs = Arc::new(cs);
        self.apply_raw(cs.clone(), origin);
        after.clamp(self.len_chars());
        after.rev = Some(self.revision);
        self.history.record(cs, origin, kind, &before, &after);
        *sels = after;
        true
    }

    /// Apply a change set and map the view's selections through it (`start`/`end` of non-empty selections with the
    /// given associations; cursors move behind inserted text).
    pub(crate) fn commit_mapped(
        &mut self,
        sels: &mut Selections,
        cs: ChangeSet,
        origin: EditOrigin,
        kind: EditKind,
        start_assoc: Assoc,
        end_assoc: Assoc,
    ) -> bool {
        let mut after = sels.clone();
        after.transform(|s| {
            if s.is_empty() {
                let p = cs.map_pos(s.head, Assoc::After);
                Selection { anchor: p, head: p, goal: None }
            } else {
                let a = cs.map_pos(s.start(), start_assoc);
                let b = cs.map_pos(s.end(), end_assoc).max(a);
                s.with_range(a, b)
            }
        });
        self.commit(sels, cs, after, origin, kind)
    }

    /// Apply arbitrary non-overlapping replacements as one undo step (MCP `editor_edit`, reload, tools). The view's
    /// selections are mapped through the edit.
    pub fn apply_edits(&mut self, sels: &mut Selections, edits: Vec<TextEdit>, origin: EditOrigin) -> Result<bool, EditError> {
        self.sync(sels);
        let edits = edits.into_iter().map(|e| (e.range, normalize_eol(&e.text).0)).collect();
        let cs = ChangeSet::from_edits(&self.rope, edits)?;
        Ok(self.commit_mapped(sels, cs, origin, EditKind::Other, Assoc::After, Assoc::Before))
    }

    /// Replace the whole text by `new` with a minimal line diff (keeps undo, marks and cursors as far as possible).
    /// Used for "Load from disk" and agent rewrites. Returns whether anything changed.
    pub fn replace_all_text(&mut self, sels: &mut Selections, new: &str, origin: EditOrigin) -> bool {
        self.sync(sels);
        let (new, _) = normalize_eol(new);
        let old = self.text();
        if old == new {
            return false;
        }
        let edits = super::diff::minimal_edits(&old, &new);
        let cs = ChangeSet::from_edits(&self.rope, edits.into_iter().map(|e| (e.range, e.text)).collect())
            .expect("diff edits are valid");
        self.commit_mapped(sels, cs, origin, EditKind::Other, Assoc::After, Assoc::Before)
    }

    /// Undo the last step. The view's selections go back to where they were before it.
    pub fn undo(&mut self, sels: &mut Selections) -> bool {
        self.sync(sels);
        let Some(t) = self.history.pop_undo() else { return false };
        for step in t.steps.iter().rev() {
            self.apply_raw(Arc::new(step.invert()), EditOrigin::User);
        }
        let mut s = t.sel_before.clone();
        s.clamp(self.len_chars());
        s.rev = Some(self.revision);
        *sels = s;
        self.history.push_redo(t);
        true
    }

    /// Redo the last undone step.
    pub fn redo(&mut self, sels: &mut Selections) -> bool {
        self.sync(sels);
        let Some(t) = self.history.pop_redo() else { return false };
        for step in &t.steps {
            self.apply_raw(step.clone(), t.origin);
        }
        let mut s = t.sel_after.clone();
        s.clamp(self.len_chars());
        s.rev = Some(self.revision);
        *sels = s;
        self.history.push_undo(t);
        true
    }

    /// Replace text and history completely (reload without undo). Selections of all views are clamped on next sync.
    pub(crate) fn reset_text(&mut self, text: String) {
        self.rope = Rope::from_str(&text);
        self.revision += 1;
        self.log.clear();
        self.history.clear();
        self.agent_marks.clear();
    }
}

fn map_selection(cs: &ChangeSet, s: Selection) -> Selection {
    if s.is_empty() {
        let p = cs.map_pos(s.head, Assoc::After);
        Selection { anchor: p, head: p, goal: s.goal }
    } else {
        let a = cs.map_pos(s.start(), Assoc::After);
        let b = cs.map_pos(s.end(), Assoc::Before).max(a);
        s.with_range(a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_lines_without_copy() {
        let d = Document::from_text("a\nbb\r\nccc\n");
        assert_eq!(d.len_lines(), 4);
        let v: Vec<(usize, String)> = d.visible_lines(1, 10).map(|(i, l)| (i, l.to_string())).collect();
        assert_eq!(v, vec![(1, "bb".into()), (2, "ccc".into()), (3, "".into())]);
        assert_eq!(d.line_part(2, 1..9).to_string(), "cc");
        assert_eq!(d.visible_lines(10, 5).count(), 0);
        assert_eq!(d.format().eol, LineEnding::Lf); // 2 LF vs 1 CRLF
        assert!(d.load_info().eol_stats.is_mixed());
    }

    #[test]
    fn index_conversions() {
        let d = Document::from_text("ä€\nx");
        assert_eq!(d.char_to_byte(2), 5);
        assert_eq!(d.byte_to_char(5), 2);
        assert_eq!(d.byte_to_char(1), 0);
        assert_eq!(d.pos_to_linecol(3), LineCol::new(1, 0));
        assert_eq!(d.linecol_to_pos(LineCol::new(0, 5)), 2);
    }

    /// Mehrere Aktionen beim Speichern (#383): ein Rückgängig-Schritt.
    #[test]
    fn join_undo_since_makes_one_step() {
        let mut d = Document::from_text("a  
b");
        let mut s = d.new_selections(0);
        d.apply_edits(&mut s, vec![TextEdit::insert(0, "x")], EditOrigin::User).unwrap();
        let mark = d.state_id();
        d.replace_all_text(&mut s, "xa
b", EditOrigin::User);
        d.replace_all_text(&mut s, "xa
b
", EditOrigin::User);
        d.join_undo_since(mark);
        assert!(d.undo(&mut s));
        assert_eq!(d.text(), "xa  
b", "beide Schritte auf einmal zurück");
        assert_eq!(d.state_id(), mark);
        assert!(d.redo(&mut s));
        assert_eq!(d.text(), "xa
b
");
        // Unbekannte Marke bzw. nur ein Schritt danach: nichts zusammengelegt.
        let m2 = d.state_id();
        d.join_undo_since(9999);
        d.replace_all_text(&mut s, "y", EditOrigin::User);
        d.join_undo_since(m2);
        assert!(d.undo(&mut s));
        assert_eq!(d.text(), "xa
b
");
        // Vom Anfang an (Marke = Ausgangszustand ohne Schritte).
        let mut e = Document::from_text("q");
        let mut se = e.new_selections(0);
        let m0 = e.state_id();
        e.replace_all_text(&mut se, "q1", EditOrigin::User);
        e.replace_all_text(&mut se, "q12", EditOrigin::User);
        e.join_undo_since(m0);
        assert!(e.undo(&mut se) && e.text() == "q" && !e.can_undo());
        // Format: gesetzt und wieder hergestellt, auch das „ungespeichert“ des Formats.
        let st = e.format_state();
        e.set_eol(if st.0.eol == LineEnding::Lf { LineEnding::CrLf } else { LineEnding::Lf });
        assert!(e.format_state() != st);
        e.restore_format_state(st);
        assert!(e.format_state() == st);
    }

    #[test]
    fn apply_edits_undo_redo_and_dirty() {
        let mut d = Document::from_text("hello world");
        let mut s = d.new_selections(11);
        assert!(!d.is_dirty());
        d.apply_edits(&mut s, vec![TextEdit::new(0..5, "HELLO"), TextEdit::insert(11, "!")], EditOrigin::User).unwrap();
        assert_eq!(d.text(), "HELLO world!");
        assert!(d.is_dirty());
        assert!(d.undo(&mut s));
        assert_eq!(d.text(), "hello world");
        assert!(!d.is_dirty());
        assert_eq!(s.primary().head, 11);
        assert!(d.redo(&mut s));
        assert_eq!(d.text(), "HELLO world!");
        assert!(d.is_dirty());
        #[allow(clippy::reversed_empty_ranges)]
        let bad = TextEdit::new(3..1, "x");
        assert!(d.apply_edits(&mut s, vec![bad], EditOrigin::User).is_err());
    }

    #[test]
    fn agent_edits_are_marked_and_undoable() {
        let mut d = Document::from_text("abc\n");
        let mut s = d.new_selections(0);
        d.apply_edits(&mut s, vec![TextEdit::insert(1, "XY")], EditOrigin::Agent).unwrap();
        assert_eq!(d.agent_marks().to_vec(), vec![1..3]);
        assert_eq!(d.undo_origin(), Some(EditOrigin::Agent));
        // User typing before the mark shifts it.
        d.apply_edits(&mut s, vec![TextEdit::insert(0, "__")], EditOrigin::User).unwrap();
        assert_eq!(d.agent_marks().to_vec(), vec![3..5]);
        d.undo(&mut s);
        d.undo(&mut s);
        assert_eq!(d.text(), "abc\n");
        assert!(d.agent_marks().is_empty());
        d.redo(&mut s);
        assert_eq!(d.agent_marks().to_vec(), vec![1..3]);
    }

    #[test]
    fn second_view_is_synced() {
        let mut d = Document::from_text("one two three");
        let mut a = d.new_selections(4);
        let mut b = d.new_selections(8);
        d.apply_edits(&mut a, vec![TextEdit::insert(0, ">> ")], EditOrigin::User).unwrap();
        d.sync(&mut b);
        assert_eq!(b.primary().head, 11);
        d.undo(&mut a);
        d.sync(&mut b);
        assert_eq!(b.primary().head, 8);
    }

    #[test]
    fn replace_all_text_uses_diff() {
        let mut d = Document::from_text("a\nb\nc\nd\n");
        let mut s = d.new_selections(6); // line 3 ("d")
        assert!(d.replace_all_text(&mut s, "a\nB\nc\nd\n", EditOrigin::User));
        assert_eq!(d.text(), "a\nB\nc\nd\n");
        assert_eq!(s.primary().head, 6);
        assert!(!d.replace_all_text(&mut s, "a\r\nB\r\nc\r\nd\r\n", EditOrigin::User));
    }

    #[test]
    fn changed_since_reports_the_first_changed_position() {
        let mut d = Document::from_text("aaa\nbbb\nccc\n");
        let r0 = d.revision();
        assert_eq!(d.changed_since(r0), None);
        let mut s = d.new_selections(0);
        d.apply_edits(&mut s, vec![TextEdit::insert(9, "X")], EditOrigin::User).unwrap();
        let r1 = d.revision();
        d.apply_edits(&mut s, vec![TextEdit::insert(5, "Y")], EditOrigin::User).unwrap();
        assert_eq!(d.changed_since(r0), Some(5));
        assert_eq!(d.changed_since(r1), Some(5));
        d.undo(&mut s);
        assert_eq!(d.changed_since(r1), Some(5), "Rückgängig zählt als Änderung");
        d.reset_text("neu".into());
        assert_eq!(d.changed_since(r1), Some(0), "ohne Protokoll: alles");
    }

    #[test]
    fn large_flag() {
        let mut d = Document::from_text(&"x".repeat(100));
        assert!(!d.update_large(1000));
        assert!(d.update_large(100));
        assert!(!d.heavy_features().minimap);
    }
}
