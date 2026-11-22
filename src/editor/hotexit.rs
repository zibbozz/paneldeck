//! Hot exit: unsaved buffers survive a restart. Each buffer is stored as plain UTF-8 text (`\n` line breaks) in a
//! folder chosen by the caller (e.g. `<data folder>/editor-hotexit/`), with a small JSON index. Internal format,
//! not public API.

use super::document::Document;
use super::encoding::{LineEnding, TextEncoding};
use super::file::{DiskSnapshot, DiskState, OpenOptions, atomic_write, check_disk};
use super::selection::{Selection, Selections};
use super::text::LineCol;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const INDEX: &str = "index.json";
const VERSION: u32 = 1;

/// One stored buffer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotExitEntry {
    /// Caller's id for the buffer (tab id); only `[A-Za-z0-9_-]` are kept.
    pub id: String,
    /// File of the buffer (`None` = untitled).
    pub path: Option<PathBuf>,
    /// Title of an untitled buffer ("Untitled 2").
    #[serde(default)]
    pub title: Option<String>,
    pub encoding: TextEncoding,
    pub eol: LineEnding,
    /// Primary cursor.
    pub cursor: LineCol,
    /// Anchor of the primary selection.
    #[serde(default)]
    pub anchor: Option<LineCol>,
    /// The file on disk when the buffer was last loaded/saved (to notice changes made while Paneldeck was closed).
    #[serde(default)]
    pub disk: Option<DiskSnapshot>,
    /// Milliseconds since 1970.
    pub saved_at_ms: i64,
    /// Name of the text file in the folder.
    pub text_file: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    version: u32,
    entries: Vec<HotExitEntry>,
}

/// A restored buffer.
#[derive(Debug)]
pub struct Restored {
    pub entry: HotExitEntry,
    pub doc: Document,
    pub selections: Selections,
    /// The file changed (or vanished) on disk since the buffer was stored: show the "changed on disk" bar.
    pub disk_state: DiskState,
}

/// The hot-exit folder.
#[derive(Clone, Debug)]
pub struct HotExitStore {
    dir: PathBuf,
}

/// Keep ids file-name safe.
fn clean_id(id: &str) -> String {
    let s: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect();
    if s.is_empty() { "buffer".into() } else { s }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl HotExitStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn read_index(&self) -> Index {
        let Ok(bytes) = std::fs::read(self.dir.join(INDEX)) else { return Index { version: VERSION, entries: vec![] } };
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        serde_json::from_str(text).unwrap_or(Index { version: VERSION, entries: vec![] })
    }

    fn write_index(&self, idx: &Index) -> std::io::Result<()> {
        if idx.entries.is_empty() {
            return match std::fs::remove_file(self.dir.join(INDEX)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let json = serde_json::to_vec_pretty(idx).map_err(std::io::Error::other)?;
        atomic_write(&self.dir.join(INDEX), |w| w.write_all(&json)).map(|_| ()).map_err(to_io)
    }

    /// Stored buffers (index order = order of saving).
    pub fn list(&self) -> Vec<HotExitEntry> {
        self.read_index().entries
    }

    /// Store (or update) a buffer. Writes the text first, then the index.
    pub fn save(&self, id: &str, doc: &Document, sels: &Selections, title: Option<&str>) -> std::io::Result<HotExitEntry> {
        std::fs::create_dir_all(&self.dir)?;
        let id = clean_id(id);
        let text_file = format!("{id}.txt");
        let rope = doc.rope().clone();
        atomic_write(&self.dir.join(&text_file), |w| {
            for chunk in rope.chunks() {
                w.write_all(chunk.as_bytes())?;
            }
            Ok(())
        })
        .map_err(to_io)?;
        let p = sels.primary();
        let entry = HotExitEntry {
            id: id.clone(),
            path: doc.path().map(Path::to_path_buf),
            title: title.map(str::to_string),
            encoding: doc.encoding(),
            eol: doc.eol(),
            cursor: doc.pos_to_linecol(p.head),
            anchor: (!p.is_empty()).then(|| doc.pos_to_linecol(p.anchor)),
            disk: doc.disk_snapshot().copied(),
            saved_at_ms: now_ms(),
            text_file,
        };
        let mut idx = self.read_index();
        idx.version = VERSION;
        idx.entries.retain(|e| e.id != id);
        idx.entries.push(entry.clone());
        self.write_index(&idx)?;
        Ok(entry)
    }

    /// Forget a buffer (after save or close without saving).
    pub fn remove(&self, id: &str) -> std::io::Result<()> {
        let id = clean_id(id);
        let mut idx = self.read_index();
        let before = idx.entries.len();
        idx.entries.retain(|e| e.id != id);
        let _ = std::fs::remove_file(self.dir.join(format!("{id}.txt")));
        if idx.entries.len() != before {
            self.write_index(&idx)?;
        }
        Ok(())
    }

    /// Keep only the given ids (drop buffers of tabs that no longer exist).
    pub fn retain(&self, ids: &[&str]) -> std::io::Result<()> {
        let keep: Vec<String> = ids.iter().map(|i| clean_id(i)).collect();
        let mut idx = self.read_index();
        let (k, drop): (Vec<_>, Vec<_>) = idx.entries.into_iter().partition(|e| keep.contains(&e.id));
        for e in &drop {
            let _ = std::fs::remove_file(self.dir.join(&e.text_file));
        }
        idx.entries = k;
        self.write_index(&idx)
    }

    /// Delete everything (cleanup window).
    pub fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Bytes used by the folder (for the cleanup window).
    pub fn size_on_disk(&self) -> u64 {
        std::fs::read_dir(&self.dir)
            .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
            .unwrap_or(0)
    }

    /// Restore one buffer: the document is dirty, keeps encoding/EOL of the entry and takes indentation and save
    /// rules from `.editorconfig`/detection as on open.
    pub fn restore(&self, entry: &HotExitEntry, opts: &OpenOptions) -> std::io::Result<Restored> {
        let file = self.dir.join(Path::new(&entry.text_file).file_name().unwrap_or_default());
        let bytes = std::fs::read(&file)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let (mut doc, _) = Document::from_bytes(text.as_bytes(), entry.path.as_deref(), opts);
        doc.format.encoding = entry.encoding;
        doc.format.eol = entry.eol;
        doc.disk = entry.disk;
        doc.load_info.file_size = 0;
        doc.update_large(opts.large_threshold);
        doc.mark_unsaved();
        let head = doc.linecol_to_pos(entry.cursor);
        let anchor = entry.anchor.map(|a| doc.linecol_to_pos(a)).unwrap_or(head);
        let mut selections = doc.new_selections(head);
        selections.set_primary(Selection::new(anchor, head));
        let disk_state = match (&entry.path, &entry.disk) {
            (Some(p), Some(s)) => check_disk(p, s).unwrap_or(DiskState::Unchanged),
            (Some(p), None) if p.exists() => DiskState::Modified(DiskSnapshot::of_file(p)?),
            _ => DiskState::Unchanged,
        };
        Ok(Restored { entry: entry.clone(), doc, selections, disk_state })
    }

    /// Restore all buffers (entries whose text file is missing are skipped).
    pub fn restore_all(&self, opts: &OpenOptions) -> Vec<Restored> {
        self.list().iter().filter_map(|e| self.restore(e, opts).ok()).collect()
    }
}

fn to_io(e: super::file::SaveError) -> std::io::Error {
    match e {
        super::file::SaveError::Io(e) => e,
        other => std::io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{EditOrigin, test_dir};

    #[test]
    fn store_restore_remove() {
        let base = test_dir("hotexit");
        let store = HotExitStore::new(base.join("editor-hotexit"));
        let file = base.join("a.txt");
        std::fs::write(&file, b"one\r\ntwo\r\n").unwrap();
        let (mut d, _) = Document::open(&file, &OpenOptions::default()).unwrap();
        let mut s = d.new_selections(5);
        d.type_text(&mut s, "X", EditOrigin::User);
        store.save("tab-1", &d, &s, None).unwrap();
        let mut u = Document::from_text("draft ü\n");
        u.set_encoding(TextEncoding::Utf16Le);
        let us = u.new_selections(3);
        store.save("tab/2", &u, &us, Some("Untitled 1")).unwrap();
        let list = store.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].id, "tab2");
        // Plain text on disk.
        assert_eq!(std::fs::read_to_string(store.dir().join("tab-1.txt")).unwrap(), "one\ntXwo\n");
        let r = store.restore(&list[0], &OpenOptions::default()).unwrap();
        assert_eq!(r.doc.text(), "one\ntXwo\n");
        assert_eq!(r.doc.eol(), LineEnding::CrLf);
        assert_eq!(r.doc.path(), Some(file.as_path()));
        assert!(r.doc.is_dirty());
        assert_eq!(r.selections.primary().head, 6);
        assert_eq!(r.disk_state, DiskState::Unchanged);
        let all = store.restore_all(&OpenOptions::default());
        assert_eq!(all[1].doc.encoding(), TextEncoding::Utf16Le);
        assert_eq!(all[1].entry.title.as_deref(), Some("Untitled 1"));
        // File changed while closed.
        std::fs::write(&file, b"changed\n").unwrap();
        let r = store.restore(&list[0], &OpenOptions::default()).unwrap();
        assert!(matches!(r.disk_state, DiskState::Modified(_)));
        // Update replaces, remove deletes the text.
        store.save("tab-1", &d, &s, None).unwrap();
        assert_eq!(store.list().len(), 2);
        store.remove("tab-1").unwrap();
        assert!(!store.dir().join("tab-1.txt").exists());
        store.retain(&[]).unwrap();
        assert!(store.list().is_empty());
        assert!(!store.dir().join(INDEX).exists());
        assert!(store.size_on_disk() == 0);
        store.clear().unwrap();
        assert!(!store.dir().exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
