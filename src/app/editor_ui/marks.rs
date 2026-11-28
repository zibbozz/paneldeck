//! Änderungsmarken am linken Rand des normalen Editors (#385, 3.2.0; Standard aus): dünner Streifen zwischen
//! Zeilennummer und Text (hinzugefügt, geändert, Dreieck für entfernte Zeilen), dazu Marken in der Bildlaufleiste und in
//! der Minimap. Nicht gestagte (bzw. ungespeicherte) und gestagte Änderungen haben eigene Farben des Themes.
//!
//! Grundlage: der Index **und** HEAD zugleich (Einstellung `marker_base` = `git`), außerhalb eines Repositorys oder für
//! Dateien, die git nicht kennt, die Datei auf der Platte; mit `disk` immer die Platte. Gelesen und verglichen wird im
//! Hintergrund: die Stände in git/auf der Platte alle 2 s (nur, wenn sich `.git/index`, `HEAD` bzw. die Datei geändert
//! haben) sowie nach Speichern und Fensterfokus, die Marken selbst ~250 ms nach dem Tippen. In großen Dateien gibt es
//! keine Marken.
//!
//! Ein Klick auf eine Marke öffnet ein kleines Fenster unter der Zeile: die alten Zeilen, *Zurücksetzen* (ein
//! Rückgängig-Schritt), vorige/nächste Änderung, *Im Vergleich öffnen*.

use super::doc::DocEntry;
use crate::editor::compare::HunkKind;
use crate::editor::linemarks::{self, Base, LineMarks, MarkHunk};
use crate::editor::TextEncoding;
use crate::editor_svc::git_base::{self, Blob, RepoFile, Rev};
use crate::editor_svc::settings::MarkerBase;
use eframe::egui::Color32;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(2);

/// Gelesene Grundlage.
#[derive(Clone, Debug, Default)]
pub(crate) struct BaseTexts {
    /// Index und HEAD (git) – `index` `None`: Datei nicht im Index.
    pub git: Option<(Option<String>, Option<String>)>,
    /// Datei auf der Platte (wenn nicht über git verglichen wird).
    pub disk: Option<String>,
}

impl BaseTexts {
    /// Wird gegen git verglichen (dann gibt es gestagte Marken)?
    pub(crate) fn is_git(&self) -> bool {
        self.git.as_ref().is_some_and(|(i, _)| i.is_some())
    }
}

enum BaseLoad {
    Unchanged(Option<Option<RepoFile>>),
    New { texts: BaseTexts, stamp: u64, repo: Option<Option<RepoFile>> },
}

/// Marken-Zustand eines Dokuments.
#[derive(Default)]
pub(crate) struct Marks {
    pub result: Option<Arc<LineMarks>>,
    result_rev: Option<(u64, u64)>,
    job: Option<mpsc::Receiver<((u64, u64), LineMarks)>>,
    changed_at: Option<Instant>,
    pub base: Option<Arc<BaseTexts>>,
    base_version: u64,
    base_job: Option<mpsc::Receiver<BaseLoad>>,
    base_stamp: Option<u64>,
    base_kind: Option<(MarkerBase, Option<PathBuf>)>,
    checked: Option<Instant>,
    seen_gen: u64,
    repo: Option<Option<RepoFile>>,
}

impl Marks {
    /// Alles vergessen (Marken aus, andere Grundlage, große Datei).
    pub(crate) fn clear(&mut self) {
        *self = Marks::default();
    }

    /// Passen die Marken zum Text mit diesem Stand (`DocEntry::revision`) und läuft keine Neuberechnung?
    pub(crate) fn fresh_for(&self, rev: Option<u64>) -> bool {
        rev.is_some() && self.result_rev.map(|r| r.0) == rev && self.job.is_none()
    }

    /// Sofort neu berechnen (ohne die Verzögerung nach dem Tippen).
    pub(crate) fn recompute(&mut self) {
        self.result_rev = None;
        self.changed_at = None;
    }
}

/// Je Bild für ein sichtbares Dokument mit eingeschalteten Marken: Grundlage lesen/prüfen, Marken neu berechnen. Liefert,
/// ob noch etwas läuft (neu zeichnen).
pub(crate) fn tick(d: &mut DocEntry, prefs: &super::EditorPrefs, generation: u64, now: Instant) -> bool {
    if !prefs.markers || d.is_large(prefs) || d.doc.is_none() {
        if d.marks.result.is_some() || d.marks.base.is_some() {
            d.marks.clear();
        }
        return false;
    }
    let Some(path) = d.path.clone() else {
        d.marks.clear();
        return false;
    };
    let kind = (prefs.marker_base, Some(path.clone()));
    if d.marks.base_kind.as_ref() != Some(&kind) {
        d.marks.clear();
        d.marks.base_kind = Some(kind);
    }
    let mut busy = false;
    // ---- Grundlage
    if let Some(rx) = &d.marks.base_job {
        match rx.try_recv() {
            Ok(BaseLoad::Unchanged(repo)) => {
                d.marks.base_job = None;
                d.marks.repo = repo.or(d.marks.repo.take());
            }
            Ok(BaseLoad::New { texts, stamp, repo }) => {
                d.marks.base_job = None;
                d.marks.base = Some(Arc::new(texts));
                d.marks.base_stamp = Some(stamp);
                d.marks.base_version += 1;
                d.marks.repo = repo.or(d.marks.repo.take());
            }
            Err(mpsc::TryRecvError::Empty) => busy = true,
            Err(mpsc::TryRecvError::Disconnected) => d.marks.base_job = None,
        }
    } else if d.marks.base.is_none() || generation != d.marks.seen_gen || d.marks.checked.is_none_or(|c| now.duration_since(c) >= REFRESH) {
        d.marks.seen_gen = generation;
        d.marks.checked = Some(now);
        let enc = d.doc.as_ref().map(|x| x.encoding());
        let (tx, rx) = mpsc::channel();
        let (prev, repo, base, max) = (if d.marks.base.is_some() { d.marks.base_stamp } else { None }, d.marks.repo.clone(), prefs.marker_base, prefs.large_file_bytes);
        std::thread::spawn(move || {
            let _ = tx.send(load_base(&path, enc, base, prev, repo, max));
        });
        d.marks.base_job = Some(rx);
        busy = true;
    }
    // ---- Marken
    if let Some(rx) = &d.marks.job {
        match rx.try_recv() {
            Ok((rev, m)) => {
                d.marks.job = None;
                d.marks.result = Some(Arc::new(m));
                d.marks.result_rev = Some(rev);
            }
            Err(mpsc::TryRecvError::Empty) => busy = true,
            Err(mpsc::TryRecvError::Disconnected) => d.marks.job = None,
        }
    }
    let Some(base) = d.marks.base.clone() else { return true };
    let rev = (d.revision().unwrap_or(0), d.marks.base_version);
    if d.marks.result_rev != Some(rev) && d.marks.job.is_none() {
        let since = *d.marks.changed_at.get_or_insert(now);
        // Erste Marken und eine neue Grundlage sofort, Tippen mit kurzer Verzögerung.
        let first = d.marks.result_rev.is_none_or(|(_, b)| b != rev.1);
        if first || now.duration_since(since) >= super::diff_tab::LIVE_DELAY {
            d.marks.changed_at = None;
            if let Some(rope) = d.doc.as_ref().map(|x| x.rope().clone()) {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let text = rope.to_string();
                    let b = match (&base.git, &base.disk) {
                        (Some((Some(i), h)), _) => Base::Git { index: Some(i.as_str()), head: h.as_deref() },
                        (_, Some(disk)) => Base::Disk(disk.as_str()),
                        _ => {
                            let _ = tx.send((rev, LineMarks::default()));
                            return;
                        }
                    };
                    let _ = tx.send((rev, linemarks::classify(&text, b, &crate::editor::compare::Options::default())));
                });
                d.marks.job = Some(rx);
            }
        }
        busy = true;
    }
    busy
}

/// Grundlage lesen (Hilfsthread): git (Index und HEAD in einem Aufruf), sonst die Platte.
fn load_base(path: &Path, enc: Option<TextEncoding>, base: MarkerBase, prev: Option<u64>, repo: Option<Option<RepoFile>>, max: u64) -> BaseLoad {
    let decode = |b: &[u8]| crate::editor::encoding::decode(b, enc).text;
    let disk = |repo: Option<Option<RepoFile>>, extra: u64| -> BaseLoad {
        let stamp = git_base::file_stamp(path) ^ extra;
        if prev == Some(stamp) {
            return BaseLoad::Unchanged(repo);
        }
        let text = std::fs::metadata(path).ok().filter(|m| m.len() <= max).and_then(|_| std::fs::read(path).ok()).map(|b| decode(&b));
        BaseLoad::New { texts: BaseTexts { git: None, disk: text }, stamp, repo }
    };
    if base == MarkerBase::Disk {
        return disk(repo, 0);
    }
    let repo = repo.unwrap_or_else(|| git_base::locate(path));
    let Some(r) = repo.clone() else { return disk(Some(None), 0) };
    let gstamp = git_base::stamp(&r.git_dir);
    if prev == Some(gstamp) {
        return BaseLoad::Unchanged(Some(repo));
    }
    match git_base::blobs(&r, &[Rev::Index, Rev::Head], max) {
        Ok(v) => {
            let text = |b: &Blob| match b {
                Blob::Bytes(x) => Some(git_base::decode_like(x, enc).text),
                _ => None,
            };
            let (index, head) = (text(&v[0]), v.get(1).and_then(text));
            if index.is_none() {
                // Nicht im Index (neu, ignoriert): gegen die Platte; der Stempel umfasst auch den von git.
                return disk(Some(repo), gstamp);
            }
            BaseLoad::New { texts: BaseTexts { git: Some((index, head)), disk: None }, stamp: gstamp, repo: Some(repo) }
        }
        Err(_) => disk(Some(repo), gstamp),
    }
}

/// Farbe einer Marke.
pub(crate) fn color(h: &MarkHunk, u: &crate::theme::tokens::UiColors) -> Color32 {
    match (h.kind, h.staged) {
        (HunkKind::Added, false) => u.marker_added,
        (HunkKind::Modified, false) => u.marker_modified,
        (HunkKind::Deleted, false) => u.marker_deleted,
        (HunkKind::Added, true) => u.marker_staged_added,
        (HunkKind::Modified, true) => u.marker_staged_modified,
        (HunkKind::Deleted, true) => u.marker_staged_deleted,
    }
}

/// Offenes Fenster einer Marke.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MarkPopup {
    pub group: u64,
    pub doc: u64,
    /// Zeile, an der die Marke steht (Anker des Fensters).
    pub line: usize,
}

/// Womit „Im Vergleich öffnen“ vergleicht: gestagte Änderungen mit HEAD, andere mit dem Index (git) bzw. der Platte.
pub(crate) fn diff_target(h: &MarkHunk, base: Option<&BaseTexts>) -> super::diff_tab::Target {
    match base {
        Some(b) if b.is_git() && h.staged => super::diff_tab::Target::Head,
        Some(b) if b.is_git() => super::diff_tab::Target::Index,
        _ => super::diff_tab::Target::Disk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_follow_the_disk_and_switch_off_for_large_files() {
        let root = std::env::var_os("PANELDECK_TEST_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!("pd-marks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("m.txt");
        std::fs::write(&p, "a\nb\nc\n").unwrap();
        let prefs = super::super::EditorPrefs { markers: true, marker_base: MarkerBase::Disk, ..Default::default() };
        let mut d = DocEntry::pending(Some(p.clone()), 0, None);
        d.ensure_loaded_now(&prefs);
        d.apply_user_edit(crate::editor::compare::minimal_edit("a\nb\nc\n", "a\nB\nc\nd\n"));
        let t0 = Instant::now();
        let mut n = 0;
        while d.marks.result.is_none() || d.marks.result_rev.map(|r| r.0) != d.revision() {
            tick(&mut d, &prefs, 0, Instant::now());
            std::thread::sleep(Duration::from_millis(5));
            n += 1;
            assert!(t0.elapsed() < Duration::from_secs(10), "Marken kommen nicht ({n})");
        }
        let m = d.marks.result.clone().unwrap();
        let kinds: Vec<(std::ops::Range<usize>, HunkKind, bool)> = m.hunks.iter().map(|h| (h.lines.clone(), h.kind, h.staged)).collect();
        assert_eq!(kinds, vec![(1..2, HunkKind::Modified, false), (3..4, HunkKind::Added, false)]);
        let u = crate::theme::tokens::ui_colors(&crate::theme::all()[0]);
        assert_eq!(color(&m.hunks[0], &u), u.marker_modified);
        assert_eq!(diff_target(&m.hunks[0], d.marks.base.as_deref()), super::super::diff_tab::Target::Disk);
        // Aus: Marken weg
        let off = super::super::EditorPrefs { markers: false, ..prefs.clone() };
        tick(&mut d, &off, 0, Instant::now());
        assert!(d.marks.result.is_none());
        // Große Datei: keine Marken
        let tiny = super::super::EditorPrefs { large_file_bytes: 4, ..prefs };
        assert!(!tick(&mut d, &tiny, 0, Instant::now()));
        assert!(d.marks.result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
