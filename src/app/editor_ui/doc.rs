//! Offene Dokumente des Editors (#54): dünne Schicht über dem Editor-Kern (`crate::editor`). Hier liegt nur, was die
//! Oberfläche zusätzlich braucht: Pfad/Titel, verzögertes Laden, Hinweisleisten-Zustand, Hot-Exit-Schlüssel, Übersetzung
//! der Tastenbefehle in Kern-Aufrufe.

use crate::editor::syntax::{self, DocHighlight, Mode};
use crate::editor::{self as core, Document, EditOrigin, Selections};
use crate::state::Lang;
use std::path::{Path, PathBuf};

/// Kodierungen für das Menü der Statusleiste (Kennung des Kerns, Anzeige).
pub(crate) const ENCODINGS: &[(&str, &str)] = &[("utf-8", "UTF-8"), ("utf-8-bom", "UTF-8 BOM"), ("utf-16le", "UTF-16 LE"), ("utf-16be", "UTF-16 BE"), ("windows-1252", "Windows-1252")];

/// Angaben zu einem offenen Dokument (für `editor_list`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DocInfo {
    pub id: u64,
    pub path: Option<PathBuf>,
    pub title: String,
    pub dirty: bool,
    /// Von einem Agenten geöffnet oder geändert.
    pub by_agent: bool,
    /// Aktiver Reiter der fokussierten Teilung.
    pub active: bool,
    pub agent_edited: bool,
    pub opened_by_agent: bool,
    /// Kennung des Kerns: `utf-8`, `utf-8-bom`, `utf-16le`, `utf-16be`, `windows-1252`.
    pub encoding: String,
    /// `lf` oder `crlf` (Format beim Speichern); `mixed`, wenn die Datei beim Öffnen gemischte Zeilenenden hatte.
    pub eol: String,
    pub lines: usize,
    pub bytes: usize,
    pub large: bool,
    /// Datei ist schreibgeschützt.
    pub read_only: bool,
    /// Auf der Platte geändert, während es hier Änderungen gibt (Hinweisleiste offen).
    pub disk_changed: bool,
    /// Datei auf der Platte gelöscht.
    pub disk_deleted: bool,
    /// Schon gelesen (sonst stehen Kodierung/Zeilen noch nicht fest).
    pub loaded: bool,
}

/// Anzeige der Statusleiste.
pub(crate) struct StatusInfo {
    pub encoding: String,
    pub encoding_key: &'static str,
    pub crlf: bool,
    pub indent: String,
}

/// Syntax-Modus eines Dokuments für die Statusleiste (#312).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyntaxStatus {
    /// Sprache (Id) – gewählt oder erkannt; `None` = reiner Text.
    pub lang: Option<String>,
    /// Anzeigename der Sprache.
    pub lang_name: String,
    /// Die Sprache ist im Dokument gewählt (nicht erkannt).
    pub lang_chosen: bool,
    /// Gewählter bzw. eingestellter Modus.
    pub mode: Mode,
    /// Der Modus ist im Dokument gewählt (nicht aus den Einstellungen).
    pub mode_chosen: bool,
    /// Farben sind gerade an (Modus Syntax, Sprache mit Grammatik, keine große Datei).
    pub active: bool,
    /// Große Datei: Syntaxfarben bleiben aus.
    pub large: bool,
}

/// Zustand des Syntax-Modus je Dokument (#312): eigene Wahl (Statusleiste, gemerkt in `editor-state.json`),
/// erkannte Sprache und die Syntaxfarben. Gilt für alle Reiter/Teilungen des Dokuments gemeinsam.
#[derive(Default)]
pub(crate) struct SyntaxDoc {
    /// Sprache in der Statusleiste gewählt (`None` = erkennen).
    pub lang: Option<String>,
    /// Modus in der Statusleiste gewählt (`None` = aus den Einstellungen).
    pub mode: Option<Mode>,
    /// Gemerkter Modus, den diese Version nicht kennt (z. B. `smart` einer neueren): bleibt beim Speichern erhalten.
    pub mode_unknown: Option<String>,
    /// Erkannte Sprache und wofür erkannt wurde (Pfad, Zuordnungen, Dokument gelesen).
    detected: Option<(DetectKey, Option<String>)>,
    pub(super) hl: Option<DocHighlight>,
}

#[derive(Clone, Debug, PartialEq)]
struct DetectKey {
    path: Option<PathBuf>,
    map_gen: u64,
    loaded: bool,
    /// Unbenannte Datei: ihre Shebang-Zeile (neu erkennen, sobald eine dazukommt oder sich ändert).
    shebang: Option<String>,
}

/// Reaktion auf eine Änderung auf der Platte.
pub(crate) enum DiskReaction {
    Reloaded(String),
    Asked,
    Nothing,
}

/// Befehle aus Tastatur und Kontextmenü, die den Text ändern (oder die Auswahl über den Text bestimmen).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Op {
    Type(String),
    Paste(String),
    PasteFromClipboard,
    Copy,
    Cut,
    Backspace(bool),
    Delete(bool),
    Newline,
    Indent,
    Outdent,
    SelectAll,
    Undo,
    Redo,
    SelectNextOccurrence,
    SelectAllOccurrences,
    SelectLine,
    DeleteLine,
    MoveLines(bool),
}

pub(crate) struct DocEntry {
    pub path: Option<PathBuf>,
    pub untitled: u32,
    pub doc: Option<Document>,
    pub load_error: Option<String>,
    /// Gespeicherter ungespeicherter Inhalt, der beim Laden die Datei ersetzt.
    pending_hot: Option<(core::HotExitStore, core::HotExitEntry)>,
    /// Inhalt einer neuen unbenannten Datei bis zum Laden.
    initial_text: Option<String>,
    /// Schlüssel im Hot-Exit-Speicher und Stand (`state_id`), der dort liegt.
    hot: std::cell::RefCell<Option<(String, u64)>>,
    pub opened_by_agent: bool,
    pub agent_edited: bool,
    pub agent_note_dismissed: bool,
    pub large_dismissed: bool,
    /// Datei auf der Platte geändert, hier gibt es eigene Änderungen (Hinweisleiste).
    pub disk_conflict: bool,
    /// Datei auf der Platte gelöscht (Puffer bleibt, gilt als ungespeichert).
    pub disk_deleted: bool,
    /// Zeitpunkt der letzten eigenen Änderung (automatisches Speichern).
    pub edited_at: Option<std::time::Instant>,
    /// Selbst gelesen/gespeichert/neu geladen seit `editor_take_touched`.
    pub touched: bool,
    /// Lesen im Hintergrund (große Dateien).
    loading: Option<std::sync::mpsc::Receiver<Result<Document, String>>>,
    /// Auswahlen für Änderungen ohne Ansicht (Agent, Neu laden, Formatieren ohne Ansicht).
    pub(super) scratch: Selections,
    /// Syntax-Modus (#312).
    pub syntax: SyntaxDoc,
    /// Änderungsmarken am Rand (#385).
    pub marks: super::marks::Marks,
    /// Breite der längsten Zeile für die waagerechte Bildlaufleiste (#389), für alle Ansichten.
    pub(super) width: super::hscroll::DocWidth,
    /// Speichern bzw. Neu laden einer WSL-Datei im Hilfsthread (#392; langsames Dateisystem, nie im UI-Thread).
    disk_job: Option<DiskJob>,
}

/// Lesen bzw. Schreiben einer WSL-Datei im Hilfsthread (#392).
enum DiskJob {
    Save { rx: std::sync::mpsc::Receiver<Result<core::WriteResult, core::SaveError>>, path: PathBuf, state: u64 },
    /// `force`: „Von der Platte laden“ (immer übernehmen); sonst Meldung der Überwachung (wie [`DocEntry::disk_changed`]).
    Reload { rx: std::sync::mpsc::Receiver<std::io::Result<(Vec<u8>, std::fs::Metadata)>>, force: bool },
}

/// Ergebnis eines fertigen [`DiskJob`] für die Oberfläche.
#[derive(Debug, PartialEq)]
pub(crate) enum DiskJobDone {
    Saved,
    Reloaded(String),
    Conflict,
    Deleted,
    Unchanged,
    Failed(String),
}

/// Liegt die Datei in WSL (`\\wsl.localhost\…`)? Dann nie im UI-Thread lesen, schreiben oder prüfen.
pub(super) fn is_wsl_path(p: &std::path::Path) -> bool {
    crate::wsl::is_wsl_unc(&p.to_string_lossy())
}

/// Datei öffnen, wenn es sie gibt; nur „nicht gefunden“ ergibt ein neues, leeres Dokument – jeder andere Fehler (kein
/// Zugriff, Netzwerk, WSL antwortet nicht) ist ein Ladefehler, damit Speichern die Datei nie mit einem leeren Puffer
/// überschreibt.
fn open_or_new(p: &std::path::Path, opts: &core::OpenOptions) -> Result<Document, String> {
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() => Document::open(p, opts).map(|(d, _)| d).map_err(|e| e.to_string()),
        Ok(_) => Err(crate::state::t("Kein reguläre Datei (Ordner?).", "Not a regular file (a folder?).").to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Document::new_file(Some(p), opts)),
        Err(e) => Err(e.to_string()),
    }
}

/// Meldung zu einem Fehler beim Speichern.
fn save_error_text(e: core::SaveError, lang: Lang) -> String {
    match e {
        core::SaveError::Unencodable(u) => format!("{} U+{:04X}", lang.t("Zeichen passt nicht in die Kodierung:", "Character does not fit the encoding:"), u.ch as u32),
        core::SaveError::ReadOnly => lang.t("Die Datei ist schreibgeschützt.", "The file is read-only.").to_string(),
        core::SaveError::NoPath => lang.t("Die Datei hat noch keinen Namen.", "The file has no name yet.").to_string(),
        e => format!("{}: {e}", lang.t("Fehler beim Speichern", "Error while saving")),
    }
}

#[cfg(test)]
mod wsl_io_tests {
    use super::*;

    /// #392: Nur „nicht gefunden“ ergibt ein neues Dokument; jeder andere Fehler ist ein Ladefehler (sonst überschriebe
    /// Speichern eine nicht lesbare Datei mit einem leeren Puffer).
    #[test]
    fn only_missing_files_become_new_documents() {
        let dir = std::env::temp_dir().join(format!("pd-open-or-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = core::OpenOptions::default();
        assert!(open_or_new(&dir.join("neu.txt"), &opts).is_ok_and(|d| d.text().is_empty()));
        std::fs::write(dir.join("a.txt"), "inhalt").unwrap();
        assert_eq!(open_or_new(&dir.join("a.txt"), &opts).map(|d| d.text()), Ok("inhalt".to_string()));
        assert!(open_or_new(&dir, &opts).is_err(), "Ordner");
        // Eine Datei als Ordner im Pfad: kein NotFound unter Windows/Linux → Ladefehler, kein leeres Dokument
        let r = open_or_new(&dir.join("a.txt").join("x.txt"), &opts);
        if let Err(e) = &r {
            assert!(!e.is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(is_wsl_path(std::path::Path::new(r"\\wsl.localhost\U\x")) && !is_wsl_path(std::path::Path::new(r"C:\x")));
    }
}

/// Ab dieser Dateigröße wird im Hilfsthread gelesen (die Oberfläche bleibt bedienbar).
pub(super) const BACKGROUND_BYTES: u64 = 4 * 1024 * 1024;

impl DocEntry {
    fn blank(path: Option<PathBuf>, untitled: u32) -> Self {
        DocEntry { path, untitled, doc: None, load_error: None, pending_hot: None, initial_text: None, hot: Default::default(), opened_by_agent: false, agent_edited: false, agent_note_dismissed: false, large_dismissed: false, disk_conflict: false, disk_deleted: false, edited_at: None, touched: false, loading: None, scratch: Selections::default(), syntax: SyntaxDoc::default(), marks: Default::default(), width: Default::default(), disk_job: None }
    }

    /// Noch nicht gelesenes Dokument (Datei und/oder Hot-Exit-Inhalt).
    pub fn pending(path: Option<PathBuf>, untitled: u32, hot: Option<(core::HotExitStore, core::HotExitEntry)>) -> Self {
        DocEntry { pending_hot: hot, ..Self::blank(path, untitled) }
    }

    /// Neue unbenannte Datei, optional mit Inhalt.
    pub fn untitled(n: u32, text: Option<&str>) -> Self {
        DocEntry { initial_text: text.map(str::to_string), ..Self::blank(None, n) }
    }

    #[cfg(test)]
    pub fn is_loaded(&self) -> bool {
        self.doc.is_some()
    }

    /// Lesen, falls noch nicht geschehen (ungespeicherter Inhalt aus dem Hot-Exit-Speicher, sonst die Datei). Dateien ab
    /// [`BACKGROUND_BYTES`] werden im Hilfsthread gelesen; bis dahin bleibt `doc` leer („wird geladen …“) und jeder
    /// weitere Aufruf holt das Ergebnis ab.
    pub fn ensure_loaded(&mut self, prefs: &super::EditorPrefs) {
        self.load(prefs, false);
    }

    /// Wie [`DocEntry::ensure_loaded`], wartet aber auf ein Lesen im Hintergrund (Schnittstelle für Agenten).
    pub fn ensure_loaded_now(&mut self, prefs: &super::EditorPrefs) {
        self.load(prefs, true);
    }

    /// Wird die Datei gerade im Hintergrund gelesen?
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    fn load(&mut self, prefs: &super::EditorPrefs, wait: bool) {
        if self.doc.is_some() || self.load_error.is_some() {
            return;
        }
        if let Some(rx) = &self.loading {
            let r = if wait { rx.recv().map_err(|_| ()) } else { rx.try_recv().map_err(|_| ()) };
            match r {
                Ok(Ok(doc)) => {
                    self.loading = None;
                    self.scratch = doc.new_selections(0);
                    self.doc = Some(doc);
                    self.touched = true;
                }
                Ok(Err(e)) => {
                    self.loading = None;
                    self.load_error = Some(e);
                }
                Err(()) if !wait && matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) => {}
                Err(()) => {
                    self.loading = None;
                    self.load_error = Some("loading failed".into());
                }
            }
            return;
        }
        let opts = open_options(prefs);
        // Ungespeicherter Inhalt aus dem letzten Lauf (Hot Exit) geht vor der Datei.
        if let Some((store, entry)) = self.pending_hot.take()
            && let Ok(r) = store.restore(&entry, &opts)
        {
            self.disk_conflict = matches!(r.disk_state, core::DiskState::Modified(_));
            self.disk_deleted = matches!(r.disk_state, core::DiskState::Deleted) && self.path.is_some();
            *self.hot.get_mut() = Some((entry.id.clone(), u64::MAX));
            self.scratch = r.selections;
            self.doc = Some(r.doc);
            self.touched = self.path.is_some();
            return;
        }
        // WSL-Dateien (#392) immer im Hilfsthread (schon das Prüfen kann die WSL-VM starten und blockieren); eine noch
        // nicht vorhandene Datei wird dort ein neues, leeres Dokument.
        if let Some(p) = self.path.clone().filter(|p| crate::wsl::is_wsl_unc(&p.to_string_lossy())) {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(open_or_new(&p, &opts));
            });
            self.loading = Some(rx);
            return self.load(prefs, wait);
        }
        if let Some(p) = self.path.clone().filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() >= BACKGROUND_BYTES)) {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(Document::open(&p, &opts).map(|(d, _)| d).map_err(|e| e.to_string()));
            });
            self.loading = Some(rx);
            return self.load(prefs, wait);
        }
        let doc = match &self.path {
            Some(p) => match open_or_new(p, &opts) {
                Ok(d) => d,
                Err(e) => {
                    self.load_error = Some(e);
                    return;
                }
            },
            None => match self.initial_text.take() {
                Some(t) => Document::from_text(&t),
                None => Document::new_file(None, &opts),
            },
        };
        self.scratch = doc.new_selections(0);
        self.doc = Some(doc);
        self.touched = self.path.is_some();
    }

    /// „Unbenannt-3“ bzw. Dateiname.
    pub fn title(&self, lang: Lang) -> String {
        match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string()),
            None => format!("{}-{}", lang.t("Unbenannt", "Untitled"), self.untitled.max(1)),
        }
    }

    pub fn dirty(&self) -> bool {
        match &self.doc {
            Some(d) => d.is_dirty() || (self.path.is_none() && d.len_chars() > 0),
            None => self.pending_hot.is_some() || self.initial_text.as_ref().is_some_and(|t| !t.is_empty()),
        }
    }

    pub fn is_large(&self, prefs: &super::EditorPrefs) -> bool {
        self.doc.as_ref().is_some_and(|d| d.is_large() || d.len_bytes() as u64 >= prefs.large_file_bytes)
    }

    pub fn line_count(&self) -> usize {
        self.doc.as_ref().map(|d| d.len_lines()).unwrap_or(1)
    }

    pub fn text(&self) -> Option<String> {
        self.doc.as_ref().map(|d| d.text())
    }

    pub fn status(&self, _prefs: &super::EditorPrefs, lang: Lang) -> Option<StatusInfo> {
        let d = self.doc.as_ref()?;
        let enc = d.encoding();
        let key = ENCODINGS.iter().find(|(k, _)| *k == enc.id()).map(|(k, _)| *k).unwrap_or("utf-8");
        let ind = d.indent_settings();
        let indent = match ind.style {
            core::IndentStyle::Tabs => format!("{}: {}", lang.t("Tabs", "Tabs"), ind.tab_width),
            core::IndentStyle::Spaces(n) => format!("{}: {n}", lang.t("Leerzeichen", "Spaces")),
        };
        Some(StatusInfo { encoding: enc.label().to_string(), encoding_key: key, crlf: d.eol() == core::LineEnding::CrLf, indent })
    }

    pub fn info(&self, id: u64, active: bool, prefs: &super::EditorPrefs) -> DocInfo {
        let (encoding, eol, lines, bytes) = match &self.doc {
            Some(d) => {
                let eol = if d.load_info().eol_stats.is_mixed() && !d.is_dirty() { "mixed" } else if d.eol() == core::LineEnding::CrLf { "crlf" } else { "lf" };
                (d.encoding().id().to_string(), eol.to_string(), d.len_lines(), d.len_bytes())
            }
            None => (String::new(), String::new(), 0, 0),
        };
        let read_only = self.path.as_ref().and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| m.permissions().readonly());
        DocInfo {
            id,
            path: self.path.clone(),
            title: self.title(Lang::En),
            dirty: self.dirty(),
            by_agent: self.opened_by_agent || self.agent_edited,
            agent_edited: self.agent_edited,
            opened_by_agent: self.opened_by_agent,
            active,
            encoding,
            eol,
            lines,
            bytes,
            large: self.is_large(prefs),
            read_only,
            disk_changed: self.disk_conflict,
            disk_deleted: self.disk_deleted,
            loaded: self.doc.is_some(),
        }
    }

    // ---- Syntax-Modus (#312) ----

    /// Sprache des Dokuments: gewählt, sonst erkannt (Dateiname, Shebang, Modeline; erst nach dem Lesen auch nach dem
    /// Inhalt). `None` = reiner Text.
    pub fn language(&mut self, prefs: &super::EditorPrefs) -> Option<String> {
        if let Some(l) = &self.syntax.lang {
            return Some(l.clone());
        }
        let shebang = if self.path.is_none() {
            self.doc.as_ref().map(|d| d.line_part(0, 0..200).to_string()).filter(|l| l.starts_with("#!"))
        } else {
            None
        };
        let key = DetectKey { path: self.path.clone(), map_gen: prefs.syntax.map_gen, loaded: self.doc.is_some(), shebang };
        if let Some((k, l)) = &self.syntax.detected
            && *k == key
        {
            return l.clone();
        }
        let (head, tail) = self.doc.as_ref().map(|d| syntax::detect::head_tail(d.rope())).unwrap_or_default();
        let l = syntax::detect(self.path.as_deref(), &head, &tail, &prefs.syntax.user_map);
        self.syntax.detected = Some((key, l.clone()));
        l
    }

    /// Modus: gewählt, sonst die Einstellung der Sprache, sonst der Standard (Text).
    pub fn mode(&mut self, prefs: &super::EditorPrefs) -> Mode {
        if let Some(m) = self.syntax.mode {
            return m;
        }
        let l = self.language(prefs);
        prefs.syntax.mode_for(l.as_deref())
    }

    /// Stand für die Statusleiste.
    pub fn syntax_status(&mut self, prefs: &super::EditorPrefs, lang: Lang) -> SyntaxStatus {
        let l = self.language(prefs);
        let mode = self.mode(prefs);
        let large = self.is_large(prefs);
        let lang_name = match &l {
            Some(id) => syntax::language_name(id).map(str::to_string).unwrap_or_else(|| id.clone()),
            None => lang.t("Reiner Text", "Plain text").to_string(),
        };
        let has_grammar = l.as_deref().is_some_and(syntax::engine::has_grammar) && !self.syntax.hl.as_ref().is_some_and(|h| h.failed());
        SyntaxStatus { lang: l, lang_name, lang_chosen: self.syntax.lang.is_some(), mode, mode_chosen: self.syntax.mode.is_some(), active: mode == Mode::Syntax && has_grammar && !large, large }
    }

    /// Sprache wählen (`None` = wieder erkennen).
    pub fn set_language(&mut self, lang: Option<String>) {
        self.syntax.lang = lang;
    }

    /// Modus wählen (`None` = wieder aus den Einstellungen).
    pub fn set_mode(&mut self, mode: Option<Mode>) {
        self.syntax.mode = mode;
        self.syntax.mode_unknown = None;
    }

    /// Syntaxfarben bereitstellen, wenn sie an sind (Modus Syntax, Sprache mit Grammatik, keine große Datei), sonst
    /// verwerfen. Danach liegen sie in `self.syntax` für das Zeichnen.
    pub fn ensure_highlight(&mut self, prefs: &super::EditorPrefs) {
        let want = if self.doc.is_some() && !self.is_large(prefs) && self.mode(prefs) == Mode::Syntax { self.language(prefs) } else { None };
        match (want, &self.syntax.hl) {
            (Some(l), Some(h)) if h.lang == l => {}
            (Some(l), _) => self.syntax.hl = self.doc.as_ref().and_then(|d| DocHighlight::new(&l, d)),
            (None, _) => self.syntax.hl = None,
        }
    }

    /// Nach einer Änderung des Nutzers.
    pub fn on_user_edit(&mut self) {
        self.edited_at = Some(std::time::Instant::now());
    }

    /// Läuft gerade Speichern/Neu laden einer WSL-Datei im Hintergrund?
    pub fn disk_busy(&self) -> bool {
        self.disk_job.is_some()
    }

    /// Speichern (unter `path`, sonst unter dem eigenen Pfad). WSL-Dateien (#392) im Hilfsthread: `Ok` heißt dann
    /// „gestartet“, das Ergebnis liefert [`DocEntry::poll_disk_job`].
    pub fn save(&mut self, path: Option<PathBuf>, lang: Lang) -> Result<(), String> {
        let target = path.clone().or_else(|| self.path.clone());
        if target.as_deref().is_some_and(is_wsl_path) {
            return self.save_in_background(target.unwrap_or_default(), lang);
        }
        let Some(d) = self.doc.as_mut() else { return Ok(()) };
        let r = match &path {
            Some(p) => d.save_as(&mut self.scratch, p),
            None => d.save(&mut self.scratch),
        };
        match r {
            Ok(rep) => {
                self.path = Some(rep.path);
                self.disk_conflict = false;
                self.disk_deleted = false;
                self.touched = true;
                self.edited_at = None;
                self.agent_edited = false;
                Ok(())
            }
            Err(e) => Err(save_error_text(e, lang)),
        }
    }

    /// Text-Aktionen beim Speichern (#383) vor dem Schreiben: Text-Änderungen als ein rückgängig machbarer Schritt,
    /// Zeilenenden/Kodierung als Format. Je Aktion `Ok(None)`, `Ok(Some(hinweis))` (nichts geändert, z. B. kein JSON)
    /// oder `Err(fehler)`. Große Dateien bekommen keine Text-Änderungen (nur Format). `path` = Ziel des Speicherns (bei
    /// *Speichern unter* der neue Name – danach richtet sich *Formatieren*). Aufruf nur über [`super::EditorUi::save_doc`].
    pub fn apply_save_builtins(&mut self, list: &[crate::save_actions::Builtin], path: Option<&std::path::Path>, prefs: &super::EditorPrefs, lang: Lang, keep_lines: &[usize]) -> Vec<(crate::save_actions::Builtin, Result<Option<String>, String>)> {
        use crate::save_actions::{builtin, Builtin};
        let large = self.is_large(prefs);
        let Some(d) = self.doc.as_mut() else { return Vec::new() };
        let mut out = Vec::new();
        let text_list: Vec<Builtin> = list.iter().filter(|b| !matches!(b, Builtin::LineEndings(_) | Builtin::Encoding(_))).cloned().collect();
        if !text_list.is_empty() {
            if large {
                for b in text_list {
                    out.push((b, Ok(Some(lang.t("Große Datei – Text nicht geändert", "Large file – text not changed").to_string()))));
                }
            } else {
                let ind = d.indent_settings();
                let indent = match ind.style {
                    core::IndentStyle::Tabs => builtin::Indent::Tab,
                    core::IndentStyle::Spaces(n) => builtin::Indent::Spaces(n),
                };
                let cx = builtin::TextCx { path, tab_width: d.tab_width(), indent, keep_lines };
                let a = builtin::apply_text(&d.text(), &text_list, cx, lang);
                if !a.changed.is_empty() {
                    d.replace_all_text(&mut self.scratch, &a.text, EditOrigin::User);
                }
                for b in text_list {
                    let r = match (a.errors.iter().find(|(x, _)| *x == b), a.notes.iter().find(|(x, _)| *x == b)) {
                        (Some((_, e)), _) => Err(e.clone()),
                        (None, Some((_, n))) => Ok(Some(n.clone())),
                        _ => Ok(None),
                    };
                    out.push((b, r));
                }
            }
        }
        for b in list {
            match b {
                Builtin::LineEndings(e) => {
                    d.set_eol(*e);
                    out.push((b.clone(), Ok(None)));
                }
                Builtin::Encoding(e) => match core::encoding::first_unencodable(d.rope(), *e) {
                    Some(u) => out.push((b.clone(), Err(format!("{} U+{:04X} – {}", lang.t("Zeichen passt nicht in die Kodierung:", "Character does not fit the encoding:"), u.ch as u32, lang.t("Kodierung unverändert", "encoding unchanged"))))),
                    None => {
                        d.set_encoding(*e);
                        out.push((b.clone(), Ok(None)));
                    }
                },
                _ => {}
            }
        }
        out
    }

    fn save_in_background(&mut self, path: PathBuf, lang: Lang) -> Result<(), String> {
        if self.disk_job.is_some() {
            return Err(lang.t("Die Datei wird gerade noch gespeichert oder gelesen – bitte gleich noch einmal.", "The file is still being saved or read – please try again in a moment.").to_string());
        }
        let Some(d) = self.doc.as_mut() else { return Ok(()) };
        let job = d.begin_save(&mut self.scratch, Some(&path)).map_err(|e| save_error_text(e, lang))?;
        let (tx, rx) = std::sync::mpsc::channel();
        let state = job.state;
        std::thread::spawn(move || {
            let _ = tx.send(job.write());
        });
        self.disk_job = Some(DiskJob::Save { rx, path, state });
        Ok(())
    }

    /// Fertigen Hintergrund-Lauf übernehmen (je Bild). `None`: keiner oder läuft noch.
    pub fn poll_disk_job(&mut self, lang: Lang) -> Option<DiskJobDone> {
        use std::sync::mpsc::TryRecvError;
        let job = self.disk_job.take()?;
        match job {
            DiskJob::Save { rx, path, state } => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.disk_job = Some(DiskJob::Save { rx, path, state });
                    None
                }
                Err(TryRecvError::Disconnected) => Some(DiskJobDone::Failed(lang.t("Speichern abgebrochen.", "Saving was interrupted.").to_string())),
                Ok(Err(e)) => Some(DiskJobDone::Failed(save_error_text(e, lang))),
                Ok(Ok(w)) => {
                    if let Some(d) = self.doc.as_mut() {
                        d.finish_save(&path, state, w);
                    }
                    self.path = Some(path);
                    self.disk_conflict = false;
                    self.disk_deleted = false;
                    self.touched = true;
                    self.edited_at = None;
                    self.agent_edited = false;
                    Some(DiskJobDone::Saved)
                }
            },
            DiskJob::Reload { rx, force } => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.disk_job = Some(DiskJob::Reload { rx, force });
                    None
                }
                Err(TryRecvError::Disconnected) => Some(DiskJobDone::Unchanged),
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound && !force => {
                    if let Some(d) = self.doc.as_mut() {
                        d.mark_deleted_on_disk();
                    }
                    self.disk_deleted = true;
                    Some(DiskJobDone::Deleted)
                }
                Ok(Err(e)) => Some(if force { DiskJobDone::Failed(e.to_string()) } else { DiskJobDone::Unchanged }),
                Ok(Ok((bytes, meta))) => {
                    let Some(d) = self.doc.as_mut() else { return Some(DiskJobDone::Unchanged) };
                    if !force && !d.differs_from_disk(&bytes, &meta) {
                        return Some(DiskJobDone::Unchanged);
                    }
                    if !force && d.is_dirty() {
                        self.disk_conflict = true;
                        return Some(DiskJobDone::Conflict);
                    }
                    match d.reload_with_bytes(&mut self.scratch, bytes, &meta) {
                        Ok(s) => {
                            self.disk_conflict = false;
                            self.disk_deleted = false;
                            self.touched = true;
                            Some(DiskJobDone::Reloaded(if s.is_empty() { String::new() } else { format!("+{} −{} {}", s.inserted, s.deleted, lang.t("Zeilen", "lines")) }))
                        }
                        Err(e) => Some(DiskJobDone::Failed(e.to_string())),
                    }
                }
            },
        }
    }

    /// WSL-Datei im Hilfsthread lesen (#392); das Ergebnis übernimmt [`DocEntry::poll_disk_job`].
    fn reload_in_background(&mut self, force: bool) {
        let Some(p) = self.path.clone() else { return };
        if self.disk_job.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = std::fs::read(&p).and_then(|b| std::fs::metadata(&p).map(|m| (b, m)));
            let _ = tx.send(r);
        });
        self.disk_job = Some(DiskJob::Reload { rx, force });
    }

    /// Kodierung für das nächste Speichern setzen.
    pub fn set_encoding(&mut self, key: &str, _lang: Lang) -> Result<(), String> {
        if let (Some(d), Some(enc)) = (self.doc.as_mut(), core::TextEncoding::from_id(key)) {
            d.set_encoding(enc);
        }
        Ok(())
    }

    pub fn set_eol(&mut self, crlf: bool) {
        if let Some(d) = self.doc.as_mut() {
            d.set_eol(if crlf { core::LineEnding::CrLf } else { core::LineEnding::Lf });
        }
    }

    /// Änderungen verwerfen (Schließen ohne Speichern).
    pub fn discard_changes(&mut self) {
        self.pending_hot = None;
    }

    /// Datei neu lesen (Hinweisleiste „Von der Platte laden“).
    pub fn reload_from_disk(&mut self, _prefs: &super::EditorPrefs) -> Option<core::DiffSummary> {
        if self.path.as_deref().is_some_and(is_wsl_path) {
            // WSL (#392): im Hilfsthread; das Ergebnis kommt über `poll_disk_job`.
            self.reload_in_background(true);
            return None;
        }
        let d = self.doc.as_mut()?;
        let r = d.reload_from_disk(&mut self.scratch).ok();
        self.disk_conflict = false;
        self.disk_deleted = false;
        self.touched = true;
        r
    }

    /// „Meine behalten“: nicht noch einmal für diesen Stand der Datei fragen.
    pub fn keep_mine(&mut self) {
        if let Some(d) = self.doc.as_mut() {
            let _ = d.resolve_conflict(&mut self.scratch, core::ConflictChoice::KeepMine);
        }
        self.disk_conflict = false;
    }

    /// Stand des Textes (wächst mit jeder Änderung, auch Rückgängig) – `None`, solange nicht gelesen.
    pub fn revision(&self) -> Option<u64> {
        self.doc.as_ref().map(|d| d.revision())
    }

    /// Kodierung, Zeilenende (Format beim Speichern) und Zeilenende am Schluss – für die Hinweisleiste des Vergleichs
    /// (#385).
    pub fn facts(&self) -> Option<core::compare::Facts> {
        let d = self.doc.as_ref()?;
        let n = d.len_chars();
        let final_newline = n > 0 && d.char_at(n - 1) == Some('\n');
        Some(core::compare::Facts { encoding: Some(d.encoding().label().to_string()), eol: Some(if d.eol() == core::LineEnding::CrLf { "CRLF" } else { "LF" }.to_string()), final_newline })
    }

    /// Änderung des Nutzers außerhalb des Textfelds (Blockpfeil im Vergleich, *Zurücksetzen* einer Marke): ein
    /// Rückgängig-Schritt, die Cursor der Ansichten folgen. Liefert, ob sich etwas geändert hat.
    pub fn apply_user_edit(&mut self, edit: core::TextEdit) -> bool {
        let Some(d) = self.doc.as_mut() else { return false };
        d.break_undo_group();
        let changed = d.apply_edits(&mut self.scratch, vec![edit], EditOrigin::User).unwrap_or(false);
        d.break_undo_group();
        if changed {
            self.on_user_edit();
        }
        changed
    }

    /// Rückgängig / Wiederholen außerhalb des Textfelds (Vergleichs-Reiter).
    pub fn undo_user(&mut self) -> bool {
        self.doc.as_mut().is_some_and(|d| d.undo(&mut self.scratch))
    }

    pub fn redo_user(&mut self) -> bool {
        self.doc.as_mut().is_some_and(|d| d.redo(&mut self.scratch))
    }

    /// Meldung der Überwachung verarbeiten.
    pub fn disk_changed(&mut self, prefs: &super::EditorPrefs, lang: Lang) -> DiskReaction {
        if self.path.as_deref().is_some_and(is_wsl_path) {
            // WSL (#392): Prüfen und still neu laden im Hilfsthread (`poll_disk_job`).
            if self.doc.is_some() {
                self.reload_in_background(false);
            }
            return DiskReaction::Nothing;
        }
        let Some(d) = self.doc.as_mut() else { return DiskReaction::Nothing };
        let Ok(state) = d.check_disk() else { return DiskReaction::Nothing };
        match d.disk_change_action(&state) {
            core::DiskChangeAction::None => DiskReaction::Nothing,
            core::DiskChangeAction::ReloadQuietly => match self.reload_from_disk(prefs) {
                Some(s) if !s.is_empty() => DiskReaction::Reloaded(format!("+{} −{} {}", s.inserted, s.deleted, lang.t("Zeilen", "lines"))),
                Some(_) => DiskReaction::Reloaded(String::new()),
                None => DiskReaction::Nothing,
            },
            core::DiskChangeAction::AskUser => {
                self.disk_conflict = true;
                DiskReaction::Asked
            }
            core::DiskChangeAction::Deleted => {
                d.mark_deleted_on_disk();
                self.disk_deleted = true;
                DiskReaction::Asked
            }
        }
    }

    /// Änderung eines Agenten (eine rückgängig machbare Änderung, Herkunft Agent).
    pub fn apply_agent_edit(&mut self, edits: Vec<(std::ops::Range<usize>, String)>, lang: Lang) -> Result<(), String> {
        let Some(d) = self.doc.as_mut() else { return Err(lang.t("Datei konnte nicht gelesen werden.", "The file could not be read.").to_string()) };
        let edits = edits.into_iter().map(|(r, t)| core::TextEdit::new(r, t)).collect();
        match d.apply_edits(&mut self.scratch, edits, EditOrigin::Agent) {
            Ok(changed) => {
                if changed {
                    self.agent_edited = true;
                    self.agent_note_dismissed = false;
                }
                Ok(())
            }
            Err(e) => Err(format!("{}: {e}", lang.t("Ungültige Änderung", "Invalid edit"))),
        }
    }

    // ---- Hot Exit ----

    pub fn hot_key(&self) -> Option<String> {
        self.hot.borrow().as_ref().map(|(k, _)| k.clone()).or(self.pending_hot.as_ref().map(|(_, e)| e.id.clone()))
    }

    /// Ungespeicherten Inhalt in den Hot-Exit-Speicher (`<dir>/editor-hotexit`) schreiben – nur bei Änderungen seit dem
    /// letzten Mal; ohne Änderungen dort löschen. `key` = Schlüssel für dieses Dokument. Liefert den Schlüssel, wenn es
    /// dort etwas gibt.
    pub fn store_hot(&self, dir: &Path, key: &str, lang: Lang) -> Option<String> {
        let store = core::HotExitStore::new(dir.join(HOT_DIR));
        let Some(d) = self.doc.as_ref() else { return self.pending_hot.as_ref().map(|(_, e)| e.id.clone()) };
        if !self.dirty() {
            if let Some((k, _)) = self.hot.borrow_mut().take() {
                let _ = store.remove(&k);
            }
            return None;
        }
        let state = d.state_id();
        if let Some((k, s)) = self.hot.borrow().as_ref()
            && *s == state
        {
            return Some(k.clone());
        }
        let key = self.hot.borrow().as_ref().map(|(k, _)| k.clone()).unwrap_or_else(|| key.to_string());
        let title = self.path.is_none().then(|| self.title(lang));
        match store.save(&key, d, &self.scratch, title.as_deref()) {
            Ok(e) => {
                *self.hot.borrow_mut() = Some((e.id.clone(), state));
                Some(e.id)
            }
            Err(_) => None,
        }
    }

    /// Hot-Exit-Daten löschen (nach Speichern oder Schließen).
    pub fn drop_hot(&mut self, dir: &Path) {
        if let Some(k) = self.hot_key() {
            let _ = core::HotExitStore::new(dir.join(HOT_DIR)).remove(&k);
        }
        *self.hot.get_mut() = None;
        self.pending_hot = None;
    }
}

/// Ordner des Hot-Exit-Speichers im Datenordner.
/// (Gleicher Name wie im Aufräumen-Fenster, `cleanup::HOT_EXIT`.)
pub(crate) const HOT_DIR: &str = "hot-exit";

/// Gespeicherte Puffer (für das Wiederherstellen beim Start).
pub(super) fn hot_entries(dir: &Path) -> (core::HotExitStore, Vec<core::HotExitEntry>) {
    let store = core::HotExitStore::new(dir.join(HOT_DIR));
    let list = store.list();
    (store, list)
}

/// Hot-Exit-Daten ohne offenes Dokument entfernen.
pub(super) fn prune_hot(dir: &Path, keep: &[String]) {
    let store = core::HotExitStore::new(dir.join(HOT_DIR));
    if store.list().iter().any(|e| !keep.contains(&e.id)) {
        let k: Vec<&str> = keep.iter().map(String::as_str).collect();
        let _ = store.retain(&k);
    }
}

/// Öffnen-Optionen aus den Einstellungen.
fn open_options(prefs: &super::EditorPrefs) -> core::OpenOptions {
    core::OpenOptions { large_threshold: prefs.large_file_bytes, ..Default::default() }
}

/// Vergleichsschlüssel eines Pfads (unter Windows ohne Groß/Klein).
pub(super) fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    if cfg!(windows) { s.to_lowercase() } else { p.to_string_lossy().to_string() }
}

thread_local! {
    /// Zuletzt im Editor kopierter Text (mit Teilen je Cursor und „ganze Zeilen“), damit Einfügen ihn wie kopiert
    /// verteilt; egui liefert beim Einfügen nur den Text.
    static LAST_CLIP: std::cell::RefCell<Option<core::ClipboardText>> = const { std::cell::RefCell::new(None) };
}

/// Kopieren bzw. Ausschneiden; liefert den Text für die Zwischenablage und ob der Text geändert wurde.
pub(super) fn clip(doc: &mut Document, sels: &mut Selections, cut: bool) -> (String, bool) {
    let before = doc.revision();
    let c = if cut { doc.cut(sels, EditOrigin::User) } else { doc.copy(sels) };
    let text = c.text.clone();
    LAST_CLIP.with(|l| *l.borrow_mut() = Some(c));
    (text, doc.revision() != before)
}

/// Tastenbefehl ausführen. Liefert, ob der Text geändert wurde.
pub(super) fn run(doc: &mut Document, sels: &mut Selections, op: Op, _tab: usize) -> bool {
    let u = EditOrigin::User;
    match op {
        Op::Type(t) => doc.type_text(sels, &t, u),
        Op::Paste(t) => {
            let norm = |s: &str| s.replace("\r\n", "\n");
            let clip = LAST_CLIP.with(|l| l.borrow().clone()).filter(|c| norm(&c.text) == norm(&t)).unwrap_or(core::ClipboardText { text: t, parts: Vec::new(), whole_lines: false });
            doc.paste(sels, &clip, u)
        }
        Op::PasteFromClipboard | Op::Copy => false,
        Op::Cut => clip(doc, sels, true).1,
        Op::Backspace(false) => doc.backspace(sels, u),
        Op::Backspace(true) => doc.delete_word_left(sels, u),
        Op::Delete(false) => doc.delete_forward(sels, u),
        Op::Delete(true) => doc.delete_word_right(sels, u),
        Op::Newline => doc.newline(sels, u),
        Op::Indent => doc.indent(sels, u),
        Op::Outdent => doc.outdent(sels, u),
        Op::SelectAll => {
            doc.select_all(sels);
            false
        }
        Op::Undo => doc.undo(sels),
        Op::Redo => doc.redo(sels),
        Op::SelectNextOccurrence => {
            doc.add_next_occurrence(sels);
            false
        }
        Op::SelectAllOccurrences => {
            doc.select_all_occurrences(sels, 10_000);
            false
        }
        Op::SelectLine => {
            doc.select_lines(sels);
            false
        }
        Op::DeleteLine => doc.delete_lines(sels, u),
        Op::MoveLines(down) => doc.move_lines(sels, down, u),
    }
}

pub(super) fn check_query(q: &super::find::Query) -> Option<String> {
    core::Search::new(&to_core(q)).err().map(|e| e.to_string())
}

fn to_core(q: &super::find::Query) -> core::SearchQuery {
    core::SearchQuery { pattern: q.text.clone(), regex: q.regex, case_sensitive: q.case, whole_word: q.word }
}

pub(super) fn count_matches(d: &Document, q: &super::find::Query, cap: usize) -> usize {
    core::Search::new(&to_core(q)).map(|s| s.count(d, cap).0).unwrap_or(0)
}

pub(super) fn find_in_range(d: &Document, q: &super::find::Query, range: std::ops::Range<usize>, limit: usize) -> Vec<std::ops::Range<usize>> {
    core::Search::new(&to_core(q)).map(|s| s.find_in_range(d, range, limit)).unwrap_or_default()
}

pub(super) fn find_next(d: &Document, q: &super::find::Query, from: usize, forward: bool) -> Option<std::ops::Range<usize>> {
    let s = core::Search::new(&to_core(q)).ok()?;
    s.find_next(d, from, if forward { core::Direction::Forward } else { core::Direction::Backward }, true)
}

pub(super) fn replace_all(d: &mut Document, sels: &mut Selections, q: &super::find::Query, repl: &str) -> usize {
    let Ok(s) = core::Search::new(&to_core(q)) else { return 0 };
    d.replace_all(sels, &s, repl, None, EditOrigin::User)
}

pub(super) fn replace_range(d: &mut Document, sels: &mut Selections, q: &super::find::Query, _m: std::ops::Range<usize>, repl: &str) -> bool {
    let Ok(s) = core::Search::new(&to_core(q)) else { return false };
    d.replace_next(sels, &s, repl, EditOrigin::User).replaced
}
