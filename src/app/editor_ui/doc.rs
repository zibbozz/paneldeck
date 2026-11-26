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
    /// Auswahlen für Änderungen ohne Ansicht (Agent, Neu laden).
    scratch: Selections,
    /// Syntax-Modus (#312).
    pub syntax: SyntaxDoc,
}

/// Ab dieser Dateigröße wird im Hilfsthread gelesen (die Oberfläche bleibt bedienbar).
pub(super) const BACKGROUND_BYTES: u64 = 4 * 1024 * 1024;

impl DocEntry {
    fn blank(path: Option<PathBuf>, untitled: u32) -> Self {
        DocEntry { path, untitled, doc: None, load_error: None, pending_hot: None, initial_text: None, hot: Default::default(), opened_by_agent: false, agent_edited: false, agent_note_dismissed: false, large_dismissed: false, disk_conflict: false, disk_deleted: false, edited_at: None, touched: false, loading: None, scratch: Selections::default(), syntax: SyntaxDoc::default() }
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
        if let Some(p) = self.path.clone().filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() >= BACKGROUND_BYTES)) {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(Document::open(&p, &opts).map(|(d, _)| d).map_err(|e| e.to_string()));
            });
            self.loading = Some(rx);
            return self.load(prefs, wait);
        }
        let doc = match &self.path {
            Some(p) if p.is_file() => match Document::open(p, &opts) {
                Ok((d, _report)) => d,
                Err(e) => {
                    self.load_error = Some(e.to_string());
                    return;
                }
            },
            Some(p) => Document::new_file(Some(p), &opts),
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

    /// Speichern (unter `path`, sonst unter dem eigenen Pfad).
    pub fn save(&mut self, path: Option<PathBuf>, lang: Lang) -> Result<(), String> {
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
            Err(core::SaveError::Unencodable(u)) => Err(format!("{} U+{:04X}", lang.t("Zeichen passt nicht in die Kodierung:", "Character does not fit the encoding:"), u.ch as u32)),
            Err(core::SaveError::ReadOnly) => Err(lang.t("Die Datei ist schreibgeschützt.", "The file is read-only.").to_string()),
            Err(core::SaveError::NoPath) => Err(lang.t("Die Datei hat noch keinen Namen.", "The file has no name yet.").to_string()),
            Err(e) => Err(format!("{}: {e}", lang.t("Fehler beim Speichern", "Error while saving"))),
        }
    }

    /// Text-Aktionen beim Speichern (#383) vor dem Schreiben: Text-Änderungen als ein rückgängig machbarer Schritt,
    /// Zeilenenden/Kodierung als Format. Je Aktion `Ok(None)`, `Ok(Some(hinweis))` (nichts geändert, z. B. kein JSON)
    /// oder `Err(fehler)`. Große Dateien bekommen keine Text-Änderungen (nur Format).
    pub fn apply_save_builtins(&mut self, list: &[crate::save_actions::Builtin], prefs: &super::EditorPrefs, lang: Lang, keep_lines: &[usize]) -> Vec<(crate::save_actions::Builtin, Result<Option<String>, String>)> {
        use crate::save_actions::{builtin, Builtin};
        let large = self.is_large(prefs);
        let path = self.path.clone();
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
                let cx = builtin::TextCx { path: path.as_deref(), tab_width: d.tab_width(), indent, keep_lines };
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

    /// Unterschied Puffer → Datei als Zeilen ('+' nur auf der Platte, '-' nur hier, ' ' gleich).
    pub fn disk_diff(&self, _lang: Lang) -> Vec<(char, String)> {
        let Some(d) = self.doc.as_ref() else { return Vec::new() };
        let Some(p) = &self.path else { return Vec::new() };
        let Ok(bytes) = std::fs::read(p) else { return Vec::new() };
        let disk = core::encoding::decode(&bytes, Some(d.encoding())).text;
        core::diff::diff_lines(&d.text(), &disk, Some(3))
            .into_iter()
            .map(|l| {
                let k = match l.kind {
                    core::DiffKind::Insert => '-', // nur auf der Platte → im Fenster „− auf der Platte“
                    core::DiffKind::Delete => '+', // nur im Puffer → „+ im Editor“
                    core::DiffKind::Equal => ' ',
                };
                (k, l.text)
            })
            .collect()
    }

    /// Meldung der Überwachung verarbeiten.
    pub fn disk_changed(&mut self, prefs: &super::EditorPrefs, lang: Lang) -> DiskReaction {
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
