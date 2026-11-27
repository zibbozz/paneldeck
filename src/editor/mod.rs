//! Core of the Editor tab (#54): document model and all editing logic, without egui.
//!
//! - [`Document`]: rope text (crate `ropey`) with `\n` line breaks only, undo/redo ([`EditOrigin`] marks agent
//!   edits), file format ([`TextEncoding`], [`LineEnding`]), indentation, dirty state, disk snapshot, large flag.
//! - [`Selections`]: the cursors of one view (multi-cursor); every editing function takes them and syncs them first,
//!   so several views (splits) can share one document.
//! - Commands (`commands.rs`, `movement.rs`): typing, delete, newline with auto-indent, indent/outdent, line
//!   operations, clipboard, add cursors, next occurrence, column selection, go to line, cursor motions.
//! - [`Search`]: find/replace with regex, case, whole word; ranges for highlighting.
//! - Files (`file.rs`): open with detection and `.editorconfig`, atomic save, change detection on disk, conflict choice.
//! - [`HotExitStore`]: unsaved buffers across restarts.
//! - [`syntax`]: Syntax mode (#312) – language detection and incremental syntax colours behind a small interface.
//! - [`compare`], [`linemarks`]: file comparison (#385) – Patience diff, intra-line changes, alignment rows, block
//!   edits, patches; change markers in the gutter (unstaged/staged against git, unsaved against disk).
//!
//! Positions are char indices into the text; see `text.rs` for line/column helpers. API overview for the UI and
//! services: `notes/editor-core-api.md` in the test folder (kept up to date with this module).

// Ein Teil der API ist (mit Tests) schon da, wird aber erst von späteren Funktionen benutzt (Gehe zu Symbol und
// Sprachunterstützung #312, Hex-Modus #301, Zeilen-Befehle, Speichern unter mit anderer Kodierung, Hot-Exit-Verwaltung).
// Nur außerhalb der Tests erlaubt: Im Test-Build muss jedes Stück wenigstens von einem Test benutzt werden.
#![cfg_attr(not(test), allow(dead_code))]

pub mod change;
pub mod commands;
pub mod compare;
pub mod diff;
pub mod document;
pub mod editorconfig;
pub mod encoding;
pub mod file;
pub mod format;
pub mod history;
pub mod hotexit;
pub mod indent;
pub mod linemarks;
pub mod movement;
pub mod search;
pub mod selection;
pub mod syntax;
pub mod text;

#[allow(unused_imports)]
pub use change::{Assoc, Change, ChangeSet, EditError};
#[allow(unused_imports)]
pub use commands::{ClipboardText, parse_goto};
#[allow(unused_imports)]
pub use diff::{DiffKind, DiffLine, DiffSummary, diff_lines, diff_summary, unified_diff};
#[allow(unused_imports)]
pub use document::{DEFAULT_LARGE_FILE_THRESHOLD, Document, FileFormat, HeavyFeatures, LoadInfo, TextEdit};
#[allow(unused_imports)]
pub use editorconfig::EditorConfig;
#[allow(unused_imports)]
pub use encoding::{EolStats, LineEnding, TextEncoding, Unencodable};
#[allow(unused_imports)]
pub use file::{
    ConflictChoice, ConflictOutcome, ContentHasher, DiskChangeAction, DiskSnapshot, DiskState, OpenError, OpenNote,
    OpenOptions, OpenReport, SaveError, SaveReport, atomic_write, check_disk,
};
#[allow(unused_imports)]
pub use history::{EditKind, EditOrigin, HistoryLimits};
#[allow(unused_imports)]
pub use hotexit::{HotExitEntry, HotExitStore, Restored};
#[allow(unused_imports)]
pub use indent::{IndentSettings, IndentSource, IndentStyle, detect_indent};
#[allow(unused_imports)]
pub use movement::Motion;
#[allow(unused_imports)]
pub use search::{Direction, ReplaceResult, Search, SearchError, SearchQuery};
#[allow(unused_imports)]
pub use selection::{Selection, Selections};
#[allow(unused_imports)]
pub use text::LineCol;

/// Fresh temp folder for tests (under `PANELDECK_TEST_ROOT` if set, else the system temp folder).
#[cfg(test)]
pub(crate) fn test_dir(tag: &str) -> std::path::PathBuf {
    let root = std::env::var_os("PANELDECK_TEST_ROOT").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = root.join(format!(
        "pd-editor-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}
