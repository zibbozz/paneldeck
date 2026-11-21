//! Opening and saving documents: encoding/EOL/.editorconfig on open, atomic save, change detection on disk,
//! and the "file changed on disk" choice (keep mine / load from disk / show difference).

use super::change::Assoc;
use super::diff::{self, DiffLine, DiffSummary};
use super::document::{DEFAULT_LARGE_FILE_THRESHOLD, Document, FileFormat, LoadInfo};
use super::editorconfig::{self, EditorConfig};
use super::encoding::{self, LineEnding, TextEncoding, Unencodable};
use super::history::{EditKind, EditOrigin};
use super::indent::{IndentSettings, IndentSource, detect_indent};
use super::selection::Selections;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Options for opening / creating documents (from the editor settings).
#[derive(Clone, Debug)]
pub struct OpenOptions {
    /// From this size (bytes) the document counts as large (heavy features off). Default 20 MB.
    pub large_threshold: u64,
    /// Indentation when neither `.editorconfig` nor detection decide.
    pub default_indent: IndentSettings,
    /// Line ending for new files / files without line breaks.
    pub default_eol: LineEnding,
    /// Read `.editorconfig` files.
    pub use_editorconfig: bool,
    /// Detect the indentation from the content.
    pub detect_indent: bool,
    /// Decode with this encoding instead of detecting.
    pub encoding: Option<TextEncoding>,
    /// Default for "trim trailing whitespace on save" (`.editorconfig` wins).
    pub trim_trailing_whitespace: bool,
    /// Default for "final newline on save" (`.editorconfig` wins; `None` = leave as is).
    pub insert_final_newline: Option<bool>,
    /// Refuse files larger than this (bytes); `None` = no limit.
    pub max_size: Option<u64>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            large_threshold: DEFAULT_LARGE_FILE_THRESHOLD,
            default_indent: IndentSettings::default(),
            default_eol: LineEnding::native(),
            use_editorconfig: true,
            detect_indent: true,
            encoding: None,
            trim_trailing_whitespace: false,
            insert_final_newline: None,
            max_size: None,
        }
    }
}

/// Something worth telling the user after opening (info bar / status bar).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenNote {
    /// The file mixes line endings; it will be saved with `chosen` (the majority).
    MixedLineEndings { stats: encoding::EolStats, chosen: LineEnding },
    /// Invalid bytes were replaced by U+FFFD; saving changes them.
    Lossy { encoding: TextEncoding },
    /// The file contains NUL bytes (probably binary).
    LooksBinary,
    /// Large document: heavy features are off.
    Large { size: u64, threshold: u64 },
    /// `.editorconfig` sets another line ending than found; saving converts.
    EolFromEditorConfig { found: LineEnding, configured: LineEnding },
    /// `.editorconfig` sets another charset that was applied for saving.
    CharsetFromEditorConfig { found: TextEncoding, configured: TextEncoding },
    /// `.editorconfig` sets a charset that does not fit the file's bytes; the detected one is kept.
    CharsetConflict { found: TextEncoding, configured: TextEncoding },
}

/// Notes from opening a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenReport {
    pub notes: Vec<OpenNote>,
}

#[derive(Debug)]
pub enum OpenError {
    Io(std::io::Error),
    /// The path is a folder or device, not a regular file.
    NotAFile,
    /// Larger than `OpenOptions::max_size`.
    TooLarge { size: u64, max: u64 },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Io(e) => write!(f, "{e}"),
            OpenError::NotAFile => write!(f, "not a regular file"),
            OpenError::TooLarge { size, max } => write!(f, "file too large ({size} bytes, limit {max})"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<std::io::Error> for OpenError {
    fn from(e: std::io::Error) -> Self {
        OpenError::Io(e)
    }
}

#[derive(Debug)]
pub enum SaveError {
    /// Untitled document: use `save_as`.
    NoPath,
    /// A char does not fit the chosen encoding (nothing was written).
    Unencodable(Unencodable),
    /// The target file is read-only.
    ReadOnly,
    Io(std::io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::NoPath => write!(f, "document has no file name"),
            SaveError::Unencodable(u) => write!(f, "character U+{:04X} at {} does not fit the encoding", u.ch as u32, u.pos),
            SaveError::ReadOnly => write!(f, "file is read-only"),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}

/// Result of a successful save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveReport {
    pub path: PathBuf,
    pub bytes: u64,
    /// Written via temp file + replace. `false` = the target could not be replaced (e.g. locked for deleting by
    /// another program) and was overwritten in place.
    pub atomic: bool,
    /// Trailing whitespace / final newline were fixed in the buffer before saving (undoable).
    pub fixed_on_save: bool,
}

// ---------------------------------------------------------------------------------------------------------------
// Disk snapshots
// ---------------------------------------------------------------------------------------------------------------

/// Size, modification time and content hash of a file, to notice changes made by other programs (e.g. agents).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiskSnapshot {
    pub len: u64,
    /// Modification time in nanoseconds since 1970 (`None` if the file system has none).
    pub mtime_ns: Option<i64>,
    /// Stable 64-bit content hash ([`ContentHasher`]).
    pub hash: u64,
}

impl DiskSnapshot {
    /// Read and hash the file.
    pub fn of_file(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let meta = std::fs::metadata(path)?;
        Ok(Self::from_parts(&bytes, &meta))
    }

    fn from_parts(bytes: &[u8], meta: &std::fs::Metadata) -> Self {
        let mut h = ContentHasher::new();
        h.update(bytes);
        Self { len: bytes.len() as u64, mtime_ns: mtime_ns(meta), hash: h.finish() }
    }
}

fn mtime_ns(meta: &std::fs::Metadata) -> Option<i64> {
    let t = meta.modified().ok()?;
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).ok(),
        Err(e) => i64::try_from(e.duration().as_nanos()).ok().map(|n| -n),
    }
}

/// What happened to a file compared with a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskState {
    Unchanged,
    /// Time stamp changed but the content is the same (e.g. `touch`); store the new snapshot to avoid re-hashing.
    Touched(DiskSnapshot),
    /// Content changed.
    Modified(DiskSnapshot),
    /// The file is gone.
    Deleted,
}

/// Compare the file at `path` with `known`. Cheap (metadata only) when size and time are unchanged.
pub fn check_disk(path: &Path, known: &DiskSnapshot) -> std::io::Result<DiskState> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DiskState::Deleted),
        Err(e) => return Err(e),
    };
    if !meta.is_file() {
        return Ok(DiskState::Deleted);
    }
    if meta.len() == known.len && mtime_ns(&meta) == known.mtime_ns {
        return Ok(DiskState::Unchanged);
    }
    let snap = DiskSnapshot::of_file(path)?;
    if snap.len == known.len && snap.hash == known.hash {
        Ok(DiskState::Touched(snap))
    } else {
        Ok(DiskState::Modified(snap))
    }
}

/// What the UI should do after a change on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChangeAction {
    /// Nothing to do.
    None,
    /// No own edits: reload quietly (show a short hint with the diff summary).
    ReloadQuietly,
    /// Own unsaved edits: info bar with keep mine / load from disk / show difference.
    AskUser,
    /// The file was deleted: keep the buffer, mark it unsaved.
    Deleted,
}

/// The user's choice in the "changed on disk" info bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Keep the buffer; do not ask again for this version of the file.
    KeepMine,
    /// Replace the buffer by the file (undoable).
    LoadFromDisk,
    /// Compute the difference buffer → disk (nothing changes).
    ShowDiff,
}

/// Outcome of [`Document::resolve_conflict`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictOutcome {
    Kept,
    Loaded(DiffSummary),
    /// Lines of the diff from the buffer (old) to the file on disk (new).
    Diff(Vec<DiffLine>),
}

/// Stable 64-bit hash for change detection (not cryptographic; same result for any chunking of the input).
#[derive(Clone, Debug)]
pub struct ContentHasher {
    h: u64,
    buf: [u8; 8],
    n: usize,
    len: u64,
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentHasher {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;

    pub fn new() -> Self {
        Self { h: 0xCBF2_9CE4_8422_2325, buf: [0; 8], n: 0, len: 0 }
    }

    #[inline]
    fn word(&mut self, w: u64) {
        self.h = (self.h ^ w).wrapping_mul(Self::K).rotate_left(29);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if self.n > 0 {
            let take = (8 - self.n).min(data.len());
            self.buf[self.n..self.n + take].copy_from_slice(&data[..take]);
            self.n += take;
            data = &data[take..];
            if self.n < 8 {
                return;
            }
            let w = u64::from_le_bytes(self.buf);
            self.word(w);
            self.n = 0;
        }
        let mut chunks = data.chunks_exact(8);
        for c in &mut chunks {
            self.word(u64::from_le_bytes(c.try_into().expect("8 bytes")));
        }
        let rest = chunks.remainder();
        self.buf[..rest.len()].copy_from_slice(rest);
        self.n = rest.len();
    }

    pub fn finish(&self) -> u64 {
        let mut h = self.h;
        if self.n > 0 {
            let mut b = [0u8; 8];
            b[..self.n].copy_from_slice(&self.buf[..self.n]);
            h = (h ^ u64::from_le_bytes(b)).wrapping_mul(Self::K).rotate_left(29);
        }
        h ^= self.len;
        // fmix64 (MurmurHash3)
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^ (h >> 33)
    }
}

/// Writer that hashes and counts what passes through.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: ContentHasher,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Atomic write
// ---------------------------------------------------------------------------------------------------------------

/// Result of [`atomic_write`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub bytes: u64,
    pub snapshot: DiskSnapshot,
    pub atomic: bool,
}

/// Write a file atomically: `write` fills a temp file in the same folder, which then replaces `path` (symbolic links
/// are followed, so the link stays). Permissions of an existing file are kept; read-only files are refused.
/// On error the temp file is removed and `path` is untouched.
pub fn atomic_write(path: &Path, write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>) -> Result<WriteResult, SaveError> {
    let target = resolve_link(path);
    let existing = std::fs::metadata(&target).ok();
    if let Some(m) = &existing {
        if !m.is_file() {
            return Err(SaveError::Io(std::io::Error::other("not a regular file")));
        }
        if m.permissions().readonly() {
            return Err(SaveError::ReadOnly);
        }
    }
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let (tmp, file) = create_temp(dir, &name)?;
    let result = (|| -> std::io::Result<(u64, u64)> {
        let mut w = HashingWriter { inner: std::io::BufWriter::with_capacity(256 * 1024, file), hasher: ContentHasher::new() };
        write(&mut w)?;
        w.flush()?;
        let hash = w.hasher.finish();
        let len = w.hasher.len;
        let file = w.inner.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        if let Some(m) = &existing {
            // Unix: Modus übernehmen. Windows: nur das Schreibschutz-Bit (ReplaceFileW übernimmt den Rest).
            let _ = std::fs::set_permissions(&tmp, m.permissions());
        }
        Ok((len, hash))
    })();
    let (len, hash) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(SaveError::Io(e));
        }
    };
    let mut atomic = true;
    if let Err(e) = crate::platform::fs::replace_file(&tmp, &target) {
        // Replacing can fail when another program holds the file open without delete sharing: overwrite in place.
        let fallback = std::fs::copy(&tmp, &target);
        let _ = std::fs::remove_file(&tmp);
        match fallback {
            Ok(_) => atomic = false,
            Err(_) => return Err(SaveError::Io(e)),
        }
    }
    let meta = std::fs::metadata(&target)?;
    Ok(WriteResult { bytes: len, snapshot: DiskSnapshot { len, mtime_ns: mtime_ns(&meta), hash }, atomic })
}

fn resolve_link(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => match std::fs::read_link(path) {
            Ok(t) if t.is_absolute() => t,
            Ok(t) => path.parent().map(|p| p.join(&t)).unwrap_or(t),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

fn create_temp(dir: &Path, name: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let mut last_err = None;
    for _ in 0..16 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.{}-{n}.pdtmp", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("no temp file name")))
}

// ---------------------------------------------------------------------------------------------------------------
// Document: open / save / reload
// ---------------------------------------------------------------------------------------------------------------

impl Document {
    /// Open a file. Detects encoding (unless forced) and line endings, applies `.editorconfig` and indentation
    /// detection, sets the large flag. Safe to call off the UI thread.
    pub fn open(path: &Path, opts: &OpenOptions) -> Result<(Document, OpenReport), OpenError> {
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(OpenError::NotAFile);
        }
        if let Some(max) = opts.max_size {
            if meta.len() > max {
                return Err(OpenError::TooLarge { size: meta.len(), max });
            }
        }
        let bytes = std::fs::read(path)?;
        let meta = std::fs::metadata(path).unwrap_or(meta);
        let snap = DiskSnapshot::from_parts(&bytes, &meta);
        let (mut doc, report) = Document::from_bytes(&bytes, Some(path), opts);
        doc.disk = Some(snap);
        Ok((doc, report))
    }

    /// Build a document from file bytes (`path` for `.editorconfig` lookup and as the document's path).
    pub fn from_bytes(bytes: &[u8], path: Option<&Path>, opts: &OpenOptions) -> (Document, OpenReport) {
        let decoded = encoding::decode(bytes, opts.encoding);
        let mut notes = Vec::new();
        let ec = match (path, opts.use_editorconfig) {
            (Some(p), true) => editorconfig::resolve(p),
            _ => EditorConfig::default(),
        };
        let is_ascii = decoded.text.is_ascii();
        let mut doc = Document::from_normalized(decoded.text);
        doc.path = path.map(Path::to_path_buf);
        let found_eol = decoded.eol_stats.majority();
        let mut eol = found_eol.unwrap_or(opts.default_eol);
        if decoded.eol_stats.is_mixed() {
            notes.push(OpenNote::MixedLineEndings { stats: decoded.eol_stats, chosen: eol });
        }
        if let Some(ce) = ec.end_of_line {
            if let Some(f) = found_eol.filter(|f| *f != ce) {
                notes.push(OpenNote::EolFromEditorConfig { found: f, configured: ce });
            }
            eol = ce;
        }
        let mut enc = decoded.encoding;
        if let (Some(cs), None) = (ec.charset, opts.encoding) {
            use TextEncoding::*;
            let compatible = cs == enc
                || matches!((enc, cs), (Utf8, Utf8Bom) | (Utf8Bom, Utf8))
                || (cs == Windows1252 && enc == Utf8 && is_ascii)
                || (bytes.is_empty());
            if cs != enc {
                if compatible {
                    notes.push(OpenNote::CharsetFromEditorConfig { found: enc, configured: cs });
                    enc = cs;
                } else {
                    notes.push(OpenNote::CharsetConflict { found: enc, configured: cs });
                }
            }
        }
        if decoded.lossy {
            notes.push(OpenNote::Lossy { encoding: decoded.encoding });
        }
        if decoded.looks_binary {
            notes.push(OpenNote::LooksBinary);
        }
        doc.format = FileFormat { encoding: enc, eol };
        doc.encoding_forced = opts.encoding.is_some();
        doc.load_info = LoadInfo {
            eol_stats: decoded.eol_stats,
            lossy: decoded.lossy,
            looks_binary: decoded.looks_binary,
            file_size: bytes.len() as u64,
        };
        doc.apply_indent_and_save_rules(&ec, opts);
        doc.editorconfig = ec;
        if doc.update_large(opts.large_threshold) {
            notes.push(OpenNote::Large { size: bytes.len() as u64, threshold: opts.large_threshold });
        }
        (doc, OpenReport { notes })
    }

    /// New empty document; with `path` (a file that does not exist yet) `.editorconfig` defaults apply fully.
    pub fn new_file(path: Option<&Path>, opts: &OpenOptions) -> Document {
        let ec = match (path, opts.use_editorconfig) {
            (Some(p), true) => editorconfig::resolve(p),
            _ => EditorConfig::default(),
        };
        let mut doc = Document::new();
        doc.path = path.map(Path::to_path_buf);
        doc.format = FileFormat {
            encoding: ec.charset.or(opts.encoding).unwrap_or_default(),
            eol: ec.end_of_line.unwrap_or(opts.default_eol),
        };
        doc.apply_indent_and_save_rules(&ec, opts);
        doc.editorconfig = ec;
        if path.is_some() {
            // Not on disk yet: saving creates it.
            doc.mark_unsaved();
        }
        doc
    }

    fn apply_indent_and_save_rules(&mut self, ec: &EditorConfig, opts: &OpenOptions) {
        let mut ind = opts.default_indent;
        ind.source = IndentSource::Default;
        if let Some(style) = ec.indent_style {
            ind.style = style;
            ind.source = IndentSource::EditorConfig;
        } else if opts.detect_indent {
            if let Some(style) = detect_indent(self.rope()) {
                ind.style = style;
                ind.source = IndentSource::Detected;
            }
        }
        if let Some(tw) = ec.tab_width {
            ind.tab_width = tw;
        }
        self.indent = ind;
        self.trim_trailing_whitespace = ec.trim_trailing_whitespace.unwrap_or(opts.trim_trailing_whitespace);
        self.insert_final_newline = ec.insert_final_newline.or(opts.insert_final_newline);
    }

    /// Bytes the document would be saved as (encoding, BOM, line endings). For small documents and tests.
    pub fn encoded_bytes(&self) -> Result<Vec<u8>, SaveError> {
        if let Some(u) = encoding::first_unencodable(self.rope(), self.format.encoding) {
            return Err(SaveError::Unencodable(u));
        }
        Ok(encoding::encode_to_vec(self.rope(), self.format.encoding, self.format.eol)?)
    }

    /// First char that does not fit the current encoding (check before switching to Windows-1252).
    pub fn first_unencodable(&self) -> Option<Unencodable> {
        encoding::first_unencodable(self.rope(), self.format.encoding)
    }

    /// Apply the save rules (trim trailing whitespace, final newline) as an undoable edit. Returns whether
    /// something changed. `save` calls this itself.
    pub fn apply_save_fixes(&mut self, sels: &mut Selections) -> bool {
        self.sync(sels);
        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        if self.trim_trailing_whitespace {
            edits.extend(self.trailing_whitespace_ranges().into_iter().map(|r| (r, String::new())));
        }
        let len = self.len_chars();
        match self.insert_final_newline {
            Some(true) if len > 0 && self.char_at(len - 1) != Some('\n') => edits.push((len..len, "\n".into())),
            Some(false) => {
                // Remove all final line breaks.
                let mut s = len;
                while s > 0 && self.char_at(s - 1) == Some('\n') {
                    s -= 1;
                }
                if s < len && !edits.iter().any(|(r, _)| r.end > s) {
                    edits.push((s..len, String::new()));
                }
            }
            _ => {}
        }
        if edits.is_empty() {
            return false;
        }
        let Ok(cs) = super::change::ChangeSet::from_edits(self.rope(), edits) else { return false };
        self.commit_mapped(sels, cs, EditOrigin::User, EditKind::Other, Assoc::Before, Assoc::Before)
    }

    /// Ranges of trailing spaces/tabs on all lines.
    pub fn trailing_whitespace_ranges(&self) -> Vec<std::ops::Range<usize>> {
        let mut out = Vec::new();
        for (i, line) in self.rope().lines().enumerate() {
            let len = super::text::line_len(self.rope(), i);
            let mut k = len;
            while k > 0 {
                let c = line.char(k - 1);
                if c == ' ' || c == '\t' {
                    k -= 1;
                } else {
                    break;
                }
            }
            if k < len {
                let ls = self.rope().line_to_char(i);
                out.push(ls + k..ls + len);
            }
        }
        out
    }

    /// Save to the document's path.
    pub fn save(&mut self, sels: &mut Selections) -> Result<SaveReport, SaveError> {
        let path = self.path.clone().ok_or(SaveError::NoPath)?;
        self.save_to(sels, &path, true)
    }

    /// Save under a new path (becomes the document's path).
    pub fn save_as(&mut self, sels: &mut Selections, path: &Path) -> Result<SaveReport, SaveError> {
        self.save_to(sels, path, true)
    }

    /// Write a copy to `path` without changing the document (no save fixes, still dirty).
    pub fn save_copy(&self, path: &Path) -> Result<SaveReport, SaveError> {
        if let Some(u) = self.first_unencodable() {
            return Err(SaveError::Unencodable(u));
        }
        let (rope, fmt) = (self.rope(), self.format);
        let w = atomic_write(path, |out| encoding::write_encoded(rope, fmt.encoding, fmt.eol, out).map(|_| ()))?;
        Ok(SaveReport { path: path.to_path_buf(), bytes: w.bytes, atomic: w.atomic, fixed_on_save: false })
    }

    fn save_to(&mut self, sels: &mut Selections, path: &Path, fixes: bool) -> Result<SaveReport, SaveError> {
        if let Some(u) = self.first_unencodable() {
            return Err(SaveError::Unencodable(u));
        }
        let fixed = fixes && self.apply_save_fixes(sels);
        let (rope, fmt) = (self.rope().clone(), self.format);
        let w = atomic_write(path, |out| encoding::write_encoded(&rope, fmt.encoding, fmt.eol, out).map(|_| ()))?;
        self.path = Some(path.to_path_buf());
        self.disk = Some(w.snapshot);
        self.load_info.file_size = w.bytes;
        self.mark_saved();
        Ok(SaveReport { path: path.to_path_buf(), bytes: w.bytes, atomic: w.atomic, fixed_on_save: fixed })
    }

    /// Compare the file on disk with the snapshot from the last load/save. Untitled documents: `Unchanged`.
    pub fn check_disk(&self) -> std::io::Result<DiskState> {
        match (&self.path, &self.disk) {
            (Some(p), Some(s)) => check_disk(p, s),
            (Some(p), None) => Ok(if p.exists() { DiskState::Modified(DiskSnapshot::of_file(p)?) } else { DiskState::Unchanged }),
            _ => Ok(DiskState::Unchanged),
        }
    }

    /// Recommended reaction to `state` (quiet reload without own edits, info bar with them).
    pub fn disk_change_action(&self, state: &DiskState) -> DiskChangeAction {
        match state {
            DiskState::Unchanged | DiskState::Touched(_) => DiskChangeAction::None,
            DiskState::Deleted => DiskChangeAction::Deleted,
            DiskState::Modified(_) if self.is_dirty() => DiskChangeAction::AskUser,
            DiskState::Modified(_) => DiskChangeAction::ReloadQuietly,
        }
    }

    /// The file was deleted on disk: keep the text, mark it unsaved.
    pub fn mark_deleted_on_disk(&mut self) {
        self.disk = None;
        self.mark_unsaved();
    }

    /// Read the file and replace the buffer by it (undoable line diff; for large documents without undo).
    /// The document is clean afterwards. Returns a summary for the "reloaded" hint.
    pub fn reload_from_disk(&mut self, sels: &mut Selections) -> Result<DiffSummary, OpenError> {
        let enc = self.forced_encoding();
        self.reload_inner(sels, enc)
    }

    /// Re-read the file decoded as `enc` ("Reopen with encoding").
    pub fn reload_with_encoding(&mut self, sels: &mut Selections, enc: TextEncoding) -> Result<DiffSummary, OpenError> {
        let r = self.reload_inner(sels, Some(enc))?;
        self.encoding_forced = true;
        Ok(r)
    }

    fn forced_encoding(&self) -> Option<TextEncoding> {
        // Detect again, unless the user chose the encoding ("reopen with encoding"); a BOM still wins.
        self.encoding_forced.then_some(self.format.encoding)
    }

    fn reload_inner(&mut self, sels: &mut Selections, enc: Option<TextEncoding>) -> Result<DiffSummary, OpenError> {
        let path = self.path.clone().ok_or(OpenError::NotAFile)?;
        let bytes = std::fs::read(&path)?;
        let meta = std::fs::metadata(&path)?;
        let snap = DiskSnapshot::from_parts(&bytes, &meta);
        // A BOM in the new file wins over the remembered encoding.
        let bom = encoding::detect(&bytes);
        let enc = match bom {
            TextEncoding::Utf8Bom | TextEncoding::Utf16Le | TextEncoding::Utf16Be if bytes.len() >= 2 => {
                if has_bom(&bytes) { Some(bom) } else { enc }
            }
            _ => enc,
        };
        let decoded = encoding::decode(&bytes, enc);
        let summary;
        if self.is_large() || decoded.text.len() as u64 >= DEFAULT_LARGE_FILE_THRESHOLD {
            let old = self.text();
            summary = if old.len() < 4 * 1024 * 1024 { diff::diff_summary(&old, &decoded.text) } else { DiffSummary::default() };
            self.reset_text(decoded.text);
            self.sync(sels);
        } else {
            summary = diff::diff_summary(&self.text(), &decoded.text);
            self.replace_all_text(sels, &decoded.text, EditOrigin::User);
            self.break_undo_group();
        }
        self.format.encoding = decoded.encoding;
        if let Some(e) = decoded.eol_stats.majority() {
            if self.editorconfig.end_of_line.is_none() {
                self.format.eol = e;
            }
        }
        self.load_info = LoadInfo {
            eol_stats: decoded.eol_stats,
            lossy: decoded.lossy,
            looks_binary: decoded.looks_binary,
            file_size: bytes.len() as u64,
        };
        self.disk = Some(snap);
        self.mark_saved();
        Ok(summary)
    }

    /// Handle the user's choice in the "changed on disk" info bar.
    pub fn resolve_conflict(&mut self, sels: &mut Selections, choice: ConflictChoice) -> Result<ConflictOutcome, OpenError> {
        match choice {
            ConflictChoice::KeepMine => {
                if let Some(p) = &self.path {
                    if let Ok(s) = DiskSnapshot::of_file(p) {
                        self.disk = Some(s);
                    }
                }
                Ok(ConflictOutcome::Kept)
            }
            ConflictChoice::LoadFromDisk => Ok(ConflictOutcome::Loaded(self.reload_from_disk(sels)?)),
            ConflictChoice::ShowDiff => {
                let path = self.path.clone().ok_or(OpenError::NotAFile)?;
                let bytes = std::fs::read(&path)?;
                let disk = encoding::decode(&bytes, Some(self.format.encoding)).text;
                Ok(ConflictOutcome::Diff(diff::diff_lines(&self.text(), &disk, Some(3))))
            }
        }
    }
}

fn has_bom(b: &[u8]) -> bool {
    b.starts_with(&[0xEF, 0xBB, 0xBF]) || b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{IndentStyle, test_dir};

    #[test]
    fn hasher_is_chunking_independent() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut a = ContentHasher::new();
        a.update(&data);
        let mut b = ContentHasher::new();
        for c in data.chunks(3) {
            b.update(c);
        }
        assert_eq!(a.finish(), b.finish());
        let mut c = ContentHasher::new();
        c.update(&data[..999]);
        assert_ne!(a.finish(), c.finish());
    }

    #[test]
    fn open_keeps_encoding_and_eol_on_save() {
        let dir = test_dir("open-save");
        let cases: Vec<(&str, Vec<u8>, TextEncoding, LineEnding)> = vec![
            ("u8.txt", b"a\r\nb\r\n".to_vec(), TextEncoding::Utf8, LineEnding::CrLf),
            ("bom.txt", b"\xEF\xBB\xBFx\ny\n".to_vec(), TextEncoding::Utf8Bom, LineEnding::Lf),
            ("w.txt", b"caf\xe9\r\n".to_vec(), TextEncoding::Windows1252, LineEnding::CrLf),
            ("le.txt", [&[0xFF, 0xFE][..], &"hi\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>()].concat(), TextEncoding::Utf16Le, LineEnding::CrLf),
            ("be.txt", [&[0xFE, 0xFF][..], &"hi\n".encode_utf16().flat_map(|u| u.to_be_bytes()).collect::<Vec<u8>>()].concat(), TextEncoding::Utf16Be, LineEnding::Lf),
        ];
        for (name, bytes, enc, eol) in cases {
            let p = dir.join(name);
            std::fs::write(&p, &bytes).unwrap();
            let (mut d, _) = Document::open(&p, &OpenOptions::default()).unwrap();
            assert_eq!(d.encoding(), enc, "{name}");
            assert_eq!(d.eol(), eol, "{name}");
            assert!(!d.is_dirty());
            let mut s = d.new_selections(0);
            let r = d.save(&mut s).unwrap();
            assert!(r.atomic);
            assert_eq!(std::fs::read(&p).unwrap(), bytes, "{name}");
            assert_eq!(d.check_disk().unwrap(), DiskState::Unchanged);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switch_encoding_and_eol() {
        let dir = test_dir("switch");
        let p = dir.join("a.txt");
        std::fs::write(&p, "Grüße\n").unwrap();
        let (mut d, _) = Document::open(&p, &OpenOptions::default()).unwrap();
        let mut s = d.new_selections(0);
        d.set_encoding(TextEncoding::Utf16Le);
        d.set_eol(LineEnding::CrLf);
        assert!(d.is_dirty());
        d.save(&mut s).unwrap();
        assert!(!d.is_dirty());
        let (d2, _) = Document::open(&p, &OpenOptions::default()).unwrap();
        assert_eq!((d2.encoding(), d2.eol(), d2.text()), (TextEncoding::Utf16Le, LineEnding::CrLf, "Grüße\n".to_string()));
        // Unencodable chars are refused, the file stays.
        let before = std::fs::read(&p).unwrap();
        d.apply_edits(&mut s, vec![crate::editor::TextEdit::insert(0, "日")], EditOrigin::User).unwrap();
        d.set_encoding(TextEncoding::Windows1252);
        assert!(matches!(d.save(&mut s), Err(SaveError::Unencodable(Unencodable { pos: 0, ch: '日' }))));
        assert_eq!(std::fs::read(&p).unwrap(), before);
        // Reopen with another encoding.
        std::fs::write(&p, b"caf\xc3\xa9\n").unwrap();
        let (mut d3, _) = Document::open(&p, &OpenOptions::default()).unwrap();
        let mut s3 = d3.new_selections(0);
        assert_eq!(d3.text(), "café\n");
        d3.reload_with_encoding(&mut s3, TextEncoding::Windows1252).unwrap();
        assert_eq!(d3.text(), "cafÃ©\n");
        assert_eq!(d3.encoding(), TextEncoding::Windows1252);
        assert!(!d3.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mixed_eol_reported_and_majority_saved() {
        let (d, rep) = Document::from_bytes(b"a\r\nb\r\nc\n", None, &OpenOptions::default());
        assert_eq!(d.eol(), LineEnding::CrLf);
        assert!(rep.notes.iter().any(|n| matches!(n, OpenNote::MixedLineEndings { chosen: LineEnding::CrLf, .. })));
        assert_eq!(d.encoded_bytes().unwrap(), b"a\r\nb\r\nc\r\n");
    }

    #[test]
    fn editorconfig_applied_on_open_and_save() {
        let dir = test_dir("ec-open");
        std::fs::write(
            dir.join(".editorconfig"),
            "root = true\n[*.txt]\nindent_style = tab\ntab_width = 8\nend_of_line = lf\ntrim_trailing_whitespace = true\ninsert_final_newline = true\ncharset = utf-8-bom\n",
        )
        .unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, "x  \r\n    y\t\r\nz").unwrap();
        let (mut d, rep) = Document::open(&p, &OpenOptions::default()).unwrap();
        assert_eq!(d.indent_settings().style, IndentStyle::Tabs);
        assert_eq!(d.indent_settings().source, IndentSource::EditorConfig);
        assert_eq!(d.tab_width(), 8);
        assert_eq!(d.eol(), LineEnding::Lf);
        assert_eq!(d.encoding(), TextEncoding::Utf8Bom);
        assert!(rep.notes.contains(&OpenNote::EolFromEditorConfig { found: LineEnding::CrLf, configured: LineEnding::Lf }));
        let mut s = d.new_selections(0);
        let r = d.save(&mut s).unwrap();
        assert!(r.fixed_on_save);
        assert_eq!(std::fs::read(&p).unwrap(), b"\xEF\xBB\xBFx\n    y\nz\n");
        // The fixes are one undo step.
        d.undo(&mut s);
        assert_eq!(d.text(), "x  \n    y\t\nz");
        // Detection without editorconfig; conflicting charset is reported, not applied.
        let q = dir.join("b.md");
        std::fs::write(&q, "a\n  b\n    c\n").unwrap();
        let (d, _) = Document::open(&q, &OpenOptions::default()).unwrap();
        assert_eq!(d.indent_settings().style, IndentStyle::Spaces(2));
        assert_eq!(d.indent_settings().source, IndentSource::Detected);
        std::fs::write(dir.join(".editorconfig"), "root = true\n[*]\ncharset = utf-16le\n").unwrap();
        let (d, rep) = Document::open(&q, &OpenOptions::default()).unwrap();
        assert_eq!(d.encoding(), TextEncoding::Utf8);
        assert!(rep.notes.contains(&OpenNote::CharsetConflict { found: TextEncoding::Utf8, configured: TextEncoding::Utf16Le }));
        // New file: editorconfig fully.
        let n = Document::new_file(Some(&dir.join("new.txt")), &OpenOptions::default());
        assert_eq!(n.encoding(), TextEncoding::Utf16Le);
        assert!(n.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_changes_and_conflicts() {
        let dir = test_dir("disk");
        let p = dir.join("a.txt");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        let (mut d, _) = Document::open(&p, &OpenOptions::default()).unwrap();
        let mut s = d.new_selections(0);
        assert_eq!(d.check_disk().unwrap(), DiskState::Unchanged);
        std::fs::write(&p, "one\nTWO\nthree\n").unwrap();
        let st = d.check_disk().unwrap();
        assert!(matches!(st, DiskState::Modified(_)));
        assert_eq!(d.disk_change_action(&st), DiskChangeAction::ReloadQuietly);
        let sum = d.reload_from_disk(&mut s).unwrap();
        assert_eq!(sum, DiffSummary { inserted: 2, deleted: 1 });
        assert_eq!(d.text(), "one\nTWO\nthree\n");
        assert!(!d.is_dirty());
        // Reload is undoable (buffer gets dirty again then).
        d.undo(&mut s);
        assert_eq!(d.text(), "one\ntwo\n");
        assert!(d.is_dirty());
        d.redo(&mut s);
        // Own edit + change on disk → ask.
        d.apply_edits(&mut s, vec![crate::editor::TextEdit::insert(0, "# ")], EditOrigin::User).unwrap();
        std::fs::write(&p, "changed\n").unwrap();
        let st = d.check_disk().unwrap();
        assert_eq!(d.disk_change_action(&st), DiskChangeAction::AskUser);
        let ConflictOutcome::Diff(lines) = d.resolve_conflict(&mut s, ConflictChoice::ShowDiff).unwrap() else { panic!() };
        assert!(lines.iter().any(|l| l.kind == diff::DiffKind::Insert && l.text == "changed"));
        assert_eq!(d.resolve_conflict(&mut s, ConflictChoice::KeepMine).unwrap(), ConflictOutcome::Kept);
        assert_eq!(d.check_disk().unwrap(), DiskState::Unchanged);
        assert!(d.is_dirty());
        std::fs::write(&p, "again\n").unwrap();
        assert!(matches!(d.resolve_conflict(&mut s, ConflictChoice::LoadFromDisk).unwrap(), ConflictOutcome::Loaded(_)));
        assert_eq!(d.text(), "again\n");
        std::fs::remove_file(&p).unwrap();
        let st = d.check_disk().unwrap();
        assert_eq!(st, DiskState::Deleted);
        assert_eq!(d.disk_change_action(&st), DiskChangeAction::Deleted);
        d.mark_deleted_on_disk();
        assert!(d.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_save_keeps_old_file_on_error_and_refuses_read_only() {
        let dir = test_dir("atomic");
        let p = dir.join("a.txt");
        std::fs::write(&p, "old").unwrap();
        let r = atomic_write(&p, |w| {
            w.write_all(b"partial")?;
            Err(std::io::Error::other("boom"))
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "old");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".pdtmp")).collect();
        assert!(leftovers.is_empty());
        let w = atomic_write(&p, |w| w.write_all(b"new")).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(w.snapshot, DiskSnapshot::of_file(&p).unwrap());
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&p, perm.clone()).unwrap();
        assert!(matches!(atomic_write(&p, |w| w.write_all(b"x")), Err(SaveError::ReadOnly)));
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&p, perm).unwrap();
        // New file in an existing folder.
        let q = dir.join("new.txt");
        atomic_write(&q, |w| w.write_all(b"fresh")).unwrap();
        assert_eq!(std::fs::read_to_string(&q).unwrap(), "fresh");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_save_keeps_unix_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir("mode");
        let p = dir.join("run.sh");
        std::fs::write(&p, "echo").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o751)).unwrap();
        atomic_write(&p, |w| w.write_all(b"echo hi")).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o751);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn large_round_trip(target_bytes: usize) {
        let dir = test_dir(&format!("large-{target_bytes}"));
        let p = dir.join("big.log");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&p).unwrap());
            let mut n = 0;
            let mut i = 0u64;
            while n < target_bytes {
                let line = format!("{i:08} lorem ipsum dolor sit amet, consectetur adipiscing elit – ü\r\n");
                f.write_all(line.as_bytes()).unwrap();
                n += line.len();
                i += 1;
            }
        }
        let size = std::fs::metadata(&p).unwrap().len();
        let opts = OpenOptions { large_threshold: 20 * 1024 * 1024, ..Default::default() };
        let (mut d, rep) = Document::open(&p, &opts).unwrap();
        assert_eq!(d.eol(), LineEnding::CrLf);
        assert_eq!(d.is_large(), size >= opts.large_threshold);
        assert_eq!(rep.notes.iter().any(|n| matches!(n, OpenNote::Large { .. })), d.is_large());
        let lines = d.len_lines();
        let mid = lines / 2;
        let v: Vec<_> = d.visible_lines(mid, 40).collect();
        assert_eq!(v.len(), 40);
        assert!(v[0].1.to_string().starts_with(&format!("{mid:08}")));
        // Edit in the middle, find something near the end, undo, save.
        let mut s = d.new_selections(d.line_to_char(mid));
        d.type_text(&mut s, "EDIT ", EditOrigin::User);
        let q = crate::editor::SearchQuery::literal(&format!("{:08} lorem", lines - 3));
        let search = crate::editor::Search::new(&q).unwrap();
        let hit = search.find_next(&d, 0, crate::editor::Direction::Forward, true).unwrap();
        assert_eq!(d.char_to_line(hit.start), lines - 3);
        d.save(&mut s).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().len(), size + 5);
        d.undo(&mut s);
        d.save(&mut s).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().len(), size);
        assert_eq!(d.check_disk().unwrap(), DiskState::Unchanged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn large_file_small_variant() {
        large_round_trip(4 * 1024 * 1024);
    }

    /// ~100 MB generated at runtime (run with `cargo test --release -- --ignored large_file_100mb`).
    #[test]
    #[ignore]
    fn large_file_100mb() {
        let t = std::time::Instant::now();
        large_round_trip(100 * 1024 * 1024);
        eprintln!("100 MB round trip: {:?}", t.elapsed());
    }
}
