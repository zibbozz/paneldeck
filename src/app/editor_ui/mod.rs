//! Reiter „Editor“ (#54): ein schlichter Texteditor als eigener Hauptbereich – Ordnerbaum, Reiter, freie Teilungen mit
//! eigener Reiterleiste, Suchen/Ersetzen, Gehe zu Zeile, Statusleiste, Hinweisleisten, Hot Exit.
//!
//! Aufteilung:
//! - `panes`: Teilungen und Reiter (reine Logik),
//! - `doc`: offene Dokumente – dünne Schicht über dem Editor-Kern (`crate::editor`),
//! - `text`: das Textfeld (nur sichtbare Zeilen, Cursor, Auswahl, Maus, Tastatur, Minimap),
//! - `view`: Bereich zeichnen (Baum, Teilungen, Reiterleisten, Hinweisleisten, Statusleiste, Dialoge),
//! - `find`: Suchen/Ersetzen-Leiste und „Gehe zu Zeile“,
//! - `menu`: Menüleiste je Teilung (#381, *Datei · Bearbeiten · Ansicht · Format*),
//! - `format`: JSON/XML formatieren im Hintergrund, Fortschritt, Meldungen (#381; formatiert wird nie von allein),
//! - `tree`: Ordnerbaum, `persist`: `editor-state.json`,
//! - `diff_tab`, `diff_menu`, `marks`: Vergleichs-Reiter, seine Menüeinträge/Revisionsauswahl und die Änderungsmarken am
//!   Rand (#385, 3.2.0).
//!
//! Der Zustand ist für alle Fenster gemeinsam (`App::ed`, nicht in `view_fields!`): Ein weiteres Fenster im Editor-Reiter
//! zeigt dieselben Dateien und Teilungen. Ein Dokument gibt es genau einmal (`docs`), auch wenn es in mehreren Teilungen
//! offen ist; jeder Reiter hat seine eigene Ansicht (Cursor, Bildlauf).

mod diff_menu;
mod diff_tab;
mod doc;
pub(crate) use doc::DiskJobDone;
mod find;
pub(crate) mod format;
mod hscroll;
#[cfg(test)]
mod hscroll_tests;
mod menu;
mod marks;
mod panes;
mod persist;
pub(crate) mod smart;
mod smart_ui;
mod text;
mod tree;
mod view;
#[cfg(test)]
mod diff_tests;
#[cfg(test)]
mod multi_tests;
#[cfg(test)]
mod tests;

pub(crate) use doc::DocInfo;
pub(crate) use doc::ENCODINGS as ENCODING_CHOICES;
pub(crate) use view::lang_label;
use doc::DocEntry;
use panes::Panes;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;
use text::View;

/// Einstellungen, die der Editor liest (Einstellungen → Editor; Standardwerte, solange es dort nichts gibt).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditorPrefs {
    /// Schriftgröße; Schrift ist die des Terminals (Monospace), solange nichts anderes gewählt ist.
    pub font_size: f32,
    /// Ab dieser Größe (Bytes) sind Minimap und Umbruch aus („große Datei“).
    pub large_file_bytes: u64,
    /// Automatisch speichern so lange nach der letzten Änderung (`None` = aus, Standard). Nur Dateien mit Pfad.
    pub autosave: Option<std::time::Duration>,
    /// Syntax-Modus (#312): Standard-Modus, Modus je Sprache, eigene Zuordnungen.
    pub syntax: SyntaxPrefs,
    /// Menüleiste je Teilung (#381, Standard an).
    pub menu_bar: bool,
    /// Tastenkürzel für die Menüs (Kennung → Kürzel wie in `keys::DEFAULTS`, z. B. `ed_format` → `Alt+Shift+F`).
    pub keys: BTreeMap<String, String>,
    /// Änderungsmarken am Rand (#385; Standard aus) und ihre Grundlage.
    pub markers: bool,
    pub marker_base: crate::editor_svc::settings::MarkerBase,
    /// Vorgaben neuer Vergleichs-Reiter: Kontextzeilen, Darstellung, Leerraum ignorieren.
    pub diff_context: usize,
    pub diff_layout: crate::editor_svc::settings::DiffLayout,
    pub diff_ignore_ws: bool,
    /// Mehrere Cursor (#394; alles standardmäßig aus).
    pub multi: MultiPrefs,
    /// Smart-Modus (#367): Vervollständigung öffnet sich von selbst.
    pub smart_auto: bool,
}

/// Schalter der mehreren Cursor (#394, *Einstellungen → Editor*): Kürzel und Befehle, Alt+Klick, mittlere Maustaste.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MultiPrefs {
    pub on: bool,
    pub alt_click: bool,
    pub middle: bool,
}

/// Kennungen der Kürzel, die die Menüleiste anzeigt.
pub(crate) const MENU_KEYS: [&str; 19] = [
    "split_r",
    "split_d",
    "palette",
    "ed_format",
    "ed_minify",
    "ed_validate",
    "ed_sort_keys",
    "save_no_actions",
    "ed_next_change",
    "ed_prev_change",
    "ed_next_occurrence",
    "ed_all_occurrences",
    "ed_soft_undo",
    "ed_toggle_comment",
    "ed_toggle_comment_alt",
    "ed_complete",
    "ed_problems",
    "ed_next_problem",
    "ed_prev_problem",
];

impl Default for EditorPrefs {
    fn default() -> Self {
        let keys = MENU_KEYS.iter().filter_map(|id| crate::keys::DEFAULTS.iter().find(|(i, _)| i == id).map(|(i, d)| (i.to_string(), d.to_string()))).collect();
        EditorPrefs {
            font_size: 14.0,
            large_file_bytes: crate::editor::DEFAULT_LARGE_FILE_THRESHOLD,
            autosave: None,
            syntax: SyntaxPrefs::default(),
            menu_bar: true,
            keys,
            markers: false,
            marker_base: Default::default(),
            diff_context: crate::editor_svc::settings::DEFAULT_DIFF_CONTEXT as usize,
            diff_layout: Default::default(),
            diff_ignore_ws: false,
            multi: MultiPrefs::default(),
            smart_auto: true,
        }
    }
}

/// Befehl des Editors für die Befehlspalette (#385): von außen undurchsichtig, ausgeführt mit
/// [`super::App::editor_run_action`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditorAction(view::Cmd);

/// Einstellungen des Syntax-Modus (#312) aus *Einstellungen → Editor* (`settings.json` → `editor`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SyntaxPrefs {
    /// Modus einer Datei, für deren Sprache nichts eingestellt ist (Standard: Text).
    pub default_mode: crate::editor::syntax::Mode,
    /// Modus je Sprache.
    pub language_modes: BTreeMap<String, crate::editor::syntax::Mode>,
    /// Eigene Zuordnungen Dateiname/Endung → Sprache.
    pub user_map: crate::editor::syntax::UserMap,
    /// Kennung der Zuordnungen: Ändern sie sich, wird neu erkannt.
    pub map_gen: u64,
}

impl SyntaxPrefs {
    pub fn from_settings(e: &crate::editor_svc::settings::EditorSettings) -> SyntaxPrefs {
        use std::hash::{Hash, Hasher};
        let user_map = e.user_map();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        user_map.hash(&mut h);
        SyntaxPrefs { default_mode: e.default_mode, language_modes: e.language_modes.clone(), user_map, map_gen: h.finish() }
    }

    pub fn mode_for(&self, lang: Option<&str>) -> crate::editor::syntax::Mode {
        lang.and_then(|l| self.language_modes.get(l).copied()).unwrap_or(self.default_mode)
    }
}

/// Wartende Rückfrage „Änderungen speichern?“ beim Schließen.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CloseAsk {
    /// Zu schließende Reiter (Gruppe, Dokument); die Rückfrage gilt dem ersten mit ungespeicherten Änderungen.
    pub tabs: Vec<(u64, u64)>,
    /// „Speichern“ gewählt, das Schreiben läuft noch im Hintergrund (WSL, #392): statt der Rückfrage ein Hinweis
    /// „wird gespeichert …“; geschlossen wird mit dem Ergebnis (`DiskJobDone::Saved`).
    pub saving: bool,
    /// Speichern ist gescheitert: die Rückfrage zeigt den Fehler.
    pub error: Option<String>,
}

impl CloseAsk {
    pub(super) fn new(tabs: Vec<(u64, u64)>) -> Self {
        CloseAsk { tabs, saving: false, error: None }
    }
}

/// Gerade gezogener Reiter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TabDrag {
    pub group: u64,
    pub index: usize,
    pub doc: u64,
}

#[derive(Default)]
pub(crate) struct EditorUi {
    pub(super) docs: BTreeMap<u64, DocEntry>,
    pub(super) panes: Panes<View>,
    next_doc: u64,
    next_untitled: u32,
    pub(super) root: Option<PathBuf>,
    pub(super) tree: tree::Tree,
    pub(super) tree_open: bool,
    pub(super) tree_width: f32,
    pub(super) recent_folders: Vec<String>,
    pub(super) recent_files: Vec<String>,
    /// Weicher Umbruch / Minimap (Ansicht; bei großen Dateien immer aus).
    pub(super) wrap: bool,
    pub(super) minimap: bool,
    pub(super) find: find::FindBar,
    pub(super) goto: Option<find::GotoLine>,
    pub(super) close_ask: Option<CloseAsk>,
    pub(super) drag: Option<TabDrag>,
    /// Kurzer Hinweis (z. B. „neu geladen“), unten in der Statusleiste.
    pub(super) hint: Option<(String, Instant)>,
    /// Im nächsten Bild das Textfeld der fokussierten Gruppe fokussieren.
    pub(super) want_focus: bool,
    /// Ein Agent hat das gezeigte Dokument gewechselt: Hat ein Textfeld des Editors die Tastatur, gibt es sie im nächsten
    /// Bild ab (Aufrufe von Agenten nehmen nie die Tastatur, [`EditorUi::end_agent`]).
    pub(super) blur: bool,
    /// Zählt, wie oft ein Agent das gezeigte Dokument gewechselt hat: Lag so ein Wechsel in der Zeit, in der das Fenster
    /// inaktiv oder minimiert war, bekommt das Textfeld die Tastatur danach nicht zurück
    /// (`view::keep_focus_across_deactivation`).
    pub(super) agent_switches: u64,
    /// Flächen der Gruppen im letzten Bild (Fokus per Strg+Alt+Pfeil, Ablage gezogener Reiter).
    pub(super) group_rects: HashMap<u64, eframe::egui::Rect>,
    /// Reiterleisten im letzten Bild (Gruppe → Fläche, Reiter-Flächen).
    pub(super) bar_rects: HashMap<u64, (eframe::egui::Rect, Vec<eframe::egui::Rect>)>,
    /// Ist der Zustand geladen (`restore`)? Erst dann wird gespeichert.
    loaded: bool,
    /// Suchtext der Sprachwahl in der Statusleiste (#312) und ob das Suchfeld die Tastatur bekommen soll.
    pub(super) lang_search: String,
    pub(super) lang_search_focus: bool,
    /// Knöpfe Sprache / Modus der Statusleiste im letzten Bild (Tests).
    pub(super) status_rects: (Option<eframe::egui::Rect>, Option<eframe::egui::Rect>),
    /// Formatieren (#381): laufende Läufe, Meldungen, Rückfrage.
    pub(super) fmt: format::FmtState,
    /// Menüleiste je Teilung im letzten Bild: Fläche und ob sie zusammengeklappt ist (Tests, Tutorial).
    pub(super) menu_rects: HashMap<u64, (eframe::egui::Rect, bool)>,
    /// Aktionen beim Speichern (#383): Stand für Statusleiste und Reiter, Klicks zurück an die App.
    pub(crate) on_save: super::save_actions_ui::OnSaveView,
    /// Vergleichs-Reiter (#385), Kennung → Zustand.
    pub(super) diffs: BTreeMap<u64, diff_tab::DiffTab>,
    next_diff: u64,
    /// Offenes Fenster einer Änderungsmarke.
    pub(super) mark_popup: Option<marks::MarkPopup>,
    /// Smart-Modus (#367): Language Server, Vertrauen, Diagnosen, Vervollständigen.
    pub(crate) smart: smart::SmartUi,
    /// Fenster „Vergleichen mit Revision …“.
    pub(super) rev_picker: Option<diff_menu::RevPicker>,
    /// Wächst nach dem Speichern und wenn das Fenster den Fokus bekommt: Vergleiche und Marken lesen die Stände in
    /// git und auf der Platte neu.
    pub(super) generation: u64,
    pub(super) was_active: bool,
    /// Zu dieser Zeile springen, sobald das (im Hintergrund geladene) Dokument da ist (#392: Strg+Klick auf
    /// `datei:12` aus WSL).
    pub(super) pending_goto: Option<(u64, usize)>,
    /// Speichern im Hintergrund (WSL, #392), dessen Formatieren/Text-Aktionen bei einem Fehler zurückzunehmen sind.
    pub(super) pending_saves: HashMap<u64, format::PendingSave>,
    /// *Speichern unter* auf eine Datei, die in einem anderen Reiter offen ist, schreibt im Hintergrund (WSL, #392):
    /// gespeichertes Dokument → anderes Dokument, das nach dem Schreiben neu lädt (bzw. die Leiste zeigt).
    pub(super) reload_after_save: HashMap<u64, u64>,
}

impl EditorUi {
    /// Gemerkten Zustand laden (Reiter, Teilungen, Ordner) und ungespeicherte Puffer aus dem Hot-Exit-Speicher holen.
    /// Dateien werden erst gelesen, wenn ihr Reiter zum ersten Mal sichtbar ist.
    pub(crate) fn restore(dir: &Path) -> Self {
        let s = persist::load(dir);
        let mut ed = EditorUi { loaded: true, ..Default::default() };
        ed.apply_saved(s, dir);
        ed
    }

    fn apply_saved(&mut self, s: persist::Saved, dir: &Path) {
        // WSL-Ordner und -Dateien (#392) beim Start nie prüfen (das könnte die WSL-VM starten und den Start blockieren);
        // ein Fehler zeigt sich erst beim Lesen im Hintergrund.
        self.root = s.root.map(PathBuf::from).filter(|p| doc::is_wsl_path(p) || p.is_dir());
        self.tree.expanded = s.expanded.into_iter().map(PathBuf::from).collect();
        self.recent_folders = s.recent_folders;
        self.recent_files = s.recent_files;
        self.tree_open = s.tree_open;
        self.tree_width = if s.tree_width.is_finite() { s.tree_width.clamp(160.0, 600.0) } else { 240.0 };
        self.wrap = s.wrap;
        self.minimap = s.minimap;
        self.next_untitled = s.next_untitled.max(1);
        let (store, entries) = doc::hot_entries(dir);
        for d in s.docs {
            let id = d.key.max(1);
            if self.docs.contains_key(&id) {
                continue;
            }
            let path = d.path.map(PathBuf::from);
            // Datei weg und nichts Ungespeichertes: Reiter nicht wiederherstellen.
            let hot = d.hot.and_then(|k| entries.iter().find(|e| e.id == k).cloned()).map(|e| (store.clone(), e));
            if path.as_ref().is_some_and(|p| !doc::is_wsl_path(p) && !p.is_file()) && hot.is_none() {
                continue;
            }
            if path.is_none() && hot.is_none() {
                continue; // leere unbenannte Datei
            }
            let mut entry = DocEntry::pending(path, d.untitled, hot);
            entry.syntax.lang = d.language.filter(|l| crate::editor::syntax::languages::is_known(l));
            entry.syntax.mode = d.mode.as_deref().and_then(crate::editor::syntax::Mode::from_id);
            entry.syntax.mode_unknown = d.mode.filter(|m| crate::editor::syntax::Mode::from_id(m).is_none() && !m.trim().is_empty() && m.len() <= 40);
            self.docs.insert(id, entry);
            self.next_doc = self.next_doc.max(id + 1);
        }
        for sd in &s.diffs {
            if let Some(dt) = diff_tab::DiffTab::from_saved(sd, &self.docs) {
                let id = sd.id.max(1);
                self.diffs.insert(id, dt);
                self.next_diff = self.next_diff.max(id + 1);
            }
        }
        for g in s.groups {
            let tabs: Vec<panes::Tab<View>> = g
                .tabs
                .into_iter()
                .filter(|t| self.docs.contains_key(&t.doc) && t.diff.is_none_or(|d| self.diffs.contains_key(&d)) && t.extra.is_none_or(|e| self.docs.contains_key(&e)))
                .map(|t| panes::Tab { doc: t.doc, view: View::restored(t.line, t.col, t.top), diff: t.diff, extra: t.extra.filter(|_| t.diff.is_some()) })
                .collect();
            self.panes.insert_group(g.id.max(1), panes::Group { tabs, active: g.active });
        }
        self.panes.layout = s.layout;
        self.panes.focused = s.focused;
        self.panes.normalize();
        // Dokumente und Vergleiche ohne Reiter wieder vergessen.
        let shown = self.panes.docs();
        self.docs.retain(|id, _| shown.contains(id));
        let diffs = self.panes.diffs();
        self.diffs.retain(|id, _| diffs.contains(id));
    }

    /// Zustand für `editor-state.json` und ungespeicherte Puffer in den Hot-Exit-Speicher schreiben.
    pub(crate) fn save(&self, dir: &Path) {
        if !self.loaded {
            return;
        }
        let mut docs = Vec::new();
        for (id, d) in self.docs.iter() {
            let hot = d.store_hot(dir, &format!("doc{id}"), crate::state::Lang::En);
            docs.push(persist::SavedDoc { key: *id, path: d.path.as_ref().map(|p| p.to_string_lossy().to_string()), untitled: d.untitled, hot, language: d.syntax.lang.clone(), mode: d.syntax.mode.map(|m| m.id().to_string()).or_else(|| d.syntax.mode_unknown.clone()) });
        }
        let groups = self
            .panes
            .group_ids()
            .into_iter()
            .map(|g| {
                let grp = &self.panes.groups[&g];
                persist::SavedGroup {
                    id: g,
                    active: grp.active,
                    tabs: grp
                        .tabs
                        .iter()
                        .map(|t| {
                            let (line, col) = self.docs.get(&t.doc).map(|d| t.view.cursor_linecol(d)).unwrap_or(t.view.restore_pos());
                            persist::SavedTab { doc: t.doc, line, col, top: t.view.top_f32(), diff: t.diff, extra: t.extra }
                        })
                        .collect(),
                }
            })
            .collect();
        let s = persist::Saved {
            version: persist::VERSION,
            root: self.root.as_ref().map(|p| p.to_string_lossy().to_string()),
            expanded: self.tree.expanded.iter().filter(|p| self.root.as_ref().is_some_and(|r| p.starts_with(r))).map(|p| p.to_string_lossy().to_string()).collect(),
            recent_folders: self.recent_folders.clone(),
            recent_files: self.recent_files.clone(),
            layout: self.panes.layout.clone(),
            focused: self.panes.focused,
            groups,
            docs,
            tree_open: self.tree_open,
            tree_width: self.tree_width,
            wrap: self.wrap,
            minimap: self.minimap,
            next_untitled: self.next_untitled,
            diffs: self.diffs.iter().filter_map(|(id, d)| d.to_saved(*id)).collect(),
        };
        persist::save(dir, &s);
        doc::prune_hot(dir, &self.docs.values().filter_map(|d| d.hot_key()).collect::<Vec<_>>());
    }

    /// Dokument zu einem Pfad (schon offen?).
    pub(super) fn doc_for_path(&self, path: &Path) -> Option<u64> {
        let key = doc::path_key(path);
        self.docs.iter().find(|(_, d)| d.path.as_deref().is_some_and(|p| doc::path_key(p) == key)).map(|(id, _)| *id)
    }

    /// Datei öffnen (oder ihren Reiter zeigen). `side`: in einer neuen Teilung rechts. Liefert das Dokument.
    pub(super) fn open_path(&mut self, path: &Path, side: bool) -> u64 {
        // `.`/`..` lexikalisch auflösen (wie Windows selbst; Wege über das IPC `--edit`, Strg+Klick, Agenten, Dialoge):
        // Pfad im Reiter und für Vertrauen/Projektwurzel ist dann der echte Ort (#367, Review).
        let clean;
        let path = if cfg!(windows) && path.components().any(|c| matches!(c, std::path::Component::CurDir | std::path::Component::ParentDir)) {
            clean = crate::lsp::trust::normalize_lexical(path);
            clean.as_path()
        } else {
            path
        };
        let id = match self.doc_for_path(path) {
            Some(id) => id,
            None => {
                let id = self.alloc_doc();
                self.docs.insert(id, DocEntry::pending(Some(path.to_path_buf()), 0, None));
                id
            }
        };
        persist::push_recent(&mut self.recent_files, &path.to_string_lossy());
        if let Some(root) = self.root.clone()
            && path.starts_with(&root)
        {
            self.tree.reveal(&root, path);
        }
        self.show_doc(id, side);
        id
    }

    /// Dokument zeigen: in der fokussierten Gruppe (oder rechts daneben) aktivieren.
    pub(super) fn show_doc(&mut self, id: u64, side: bool) {
        if side {
            let g = self.panes.ensure_group();
            let has_tab = self.panes.groups.get(&g).is_some_and(|grp| !grp.tabs.is_empty());
            // Die neue Teilung zeigt erst die Kopie des alten Reiters – durch das Dokument ersetzen.
            if has_tab
                && let Some(ng) = self.panes.split(g, crate::layout::Dir::Horizontal)
                && let Some(grp) = self.panes.groups.get_mut(&ng)
            {
                grp.tabs.clear();
                grp.active = 0;
            }
        }
        self.panes.open(id);
        self.want_focus = true;
    }

    fn alloc_doc(&mut self) -> u64 {
        let id = self.next_doc.max(self.docs.keys().next_back().map(|k| k + 1).unwrap_or(1)).max(1);
        self.next_doc = id + 1;
        id
    }

    /// Neue unbenannte Datei (optional mit Inhalt) in der fokussierten Gruppe.
    pub(super) fn new_untitled(&mut self, text: Option<&str>) -> u64 {
        let id = self.alloc_doc();
        let n = self.next_untitled.max(1);
        self.next_untitled = n + 1;
        self.docs.insert(id, DocEntry::untitled(n, text));
        self.show_doc(id, false);
        id
    }

    /// Fertige Hintergrund-Läufe (Speichern/Neu laden von WSL-Dateien, #392) übernehmen: (Titel, Ergebnis). `busy`: es
    /// läuft noch einer.
    pub(super) fn poll_disk_jobs(&mut self, lang: crate::state::Lang) -> (Vec<(u64, String, doc::DiskJobDone)>, bool) {
        let mut out = Vec::new();
        let mut busy = false;
        for (id, d) in self.docs.iter_mut() {
            if let Some(r) = d.poll_disk_job(lang) {
                out.push((*id, d.title(lang), r));
            }
            busy |= d.disk_busy();
        }
        // Geschlossene Dokumente: nichts mehr zurückzunehmen.
        let docs = &self.docs;
        self.pending_saves.retain(|id, _| docs.contains_key(id));
        self.reload_after_save.retain(|id, _| docs.contains_key(id));
        (out, busy)
    }

    /// Wartenden Sprung zur Zeile ausführen, sobald das Dokument geladen ist und sein Reiter vorn steht.
    pub(super) fn apply_pending_goto(&mut self) {
        let Some((id, line)) = self.pending_goto else { return };
        match self.docs.get(&id) {
            None => self.pending_goto = None,
            Some(d) if d.doc.is_some() => {
                let active = self.panes.groups.get(&self.panes.focused).and_then(|g| g.active_tab()).map(|t| t.doc);
                if active == Some(id) {
                    self.goto_line(line, 0);
                }
                self.pending_goto = None;
            }
            Some(d) if d.load_error.is_some() => self.pending_goto = None,
            Some(_) => {}
        }
    }

    /// Ordner als Wurzel des Baums öffnen.
    pub(super) fn set_root(&mut self, dir: &Path) {
        self.root = Some(dir.to_path_buf());
        self.tree.refresh();
        self.tree_open = true;
        persist::push_recent(&mut self.recent_folders, &dir.to_string_lossy());
    }

    /// Dokument, das der Nutzer gerade bearbeitet (aktiver Reiter der fokussierten Gruppe).
    pub(super) fn active_doc(&self) -> Option<u64> {
        self.panes.groups.get(&self.panes.focused).and_then(|g| g.active_tab()).map(|t| t.doc)
    }

    /// Reiter freigeben: das Dokument verschwindet, wenn es in keinem Reiter mehr offen ist (Hot-Exit-Daten mit).
    pub(super) fn release_doc(&mut self, doc: u64, dir: &Path) {
        if !self.panes.shows(doc) {
            self.cancel_format(doc);
            self.fmt.notes.remove(&doc);
            self.fmt.ask.retain(|a| a.doc != doc);
            if let Some(mut d) = self.docs.remove(&doc) {
                d.drop_hot(dir);
            }
            if self.mark_popup.as_ref().is_some_and(|p| p.doc == doc) {
                self.mark_popup = None;
            }
        }
    }

    // ---- Vergleich (#385)

    fn alloc_diff(&mut self) -> u64 {
        let id = self.next_diff.max(self.diffs.keys().next_back().map(|k| k + 1).unwrap_or(1)).max(1);
        self.next_diff = id + 1;
        id
    }

    /// Vergleich als Reiter der fokussierten Gruppe öffnen (hinter dem aktiven Reiter). Ist derselbe Vergleich schon
    /// offen, wird sein Reiter aktiv. Liefert die Kennung.
    pub(super) fn open_diff(&mut self, dt: diff_tab::DiffTab) -> Option<u64> {
        let (doc, extra) = dt.tab_docs()?;
        if let Some(id) = self.diffs.iter().find(|(_, d)| d.same_sides(&dt)).map(|(id, _)| *id)
            && let Some((g, i)) = self.panes.diff_tab(id)
        {
            self.panes.focused = g;
            if let Some(grp) = self.panes.groups.get_mut(&g) {
                grp.active = i;
            }
            self.want_focus = true;
            return Some(id);
        }
        let id = self.alloc_diff();
        self.diffs.insert(id, dt);
        self.panes.open_diff(id, doc, extra);
        self.want_focus = true;
        Some(id)
    }

    /// Neues Dokument zu einem Pfad ohne Reiter (Tests).
    #[cfg(test)]
    pub(super) fn alloc_doc_for_test(&mut self, path: &Path) -> u64 {
        let id = self.alloc_doc();
        self.docs.insert(id, DocEntry::pending(Some(path.to_path_buf()), 0, None));
        id
    }

    /// Dokument zu einer Datei: schon offen, sonst neu (noch ohne Reiter – der Vergleich hält es offen).
    fn doc_for_file(&mut self, path: &Path) -> u64 {
        match self.doc_for_path(path) {
            Some(id) => id,
            None => {
                let id = self.alloc_doc();
                self.docs.insert(id, DocEntry::pending(Some(path.to_path_buf()), 0, None));
                id
            }
        }
    }

    /// Dokument `doc` mit `target` vergleichen: das Ziel links, das Dokument rechts. `Ok(None)`, wenn erst ein Dialog
    /// gebraucht wird (Datei, Revision – das erledigt der Aufrufer).
    pub(super) fn compare(&mut self, doc: u64, target: diff_tab::Target, prefs: &EditorPrefs, lang: crate::state::Lang) -> Result<Option<u64>, String> {
        use crate::editor_svc::git_base::Rev;
        use diff_tab::{Side, Source, StaticSide, Target};
        let path = self.docs.get(&doc).ok_or_else(|| lang.t("Kein solches Dokument", "No such document").to_string())?.path.clone();
        let need = || path.clone().ok_or_else(|| lang.t("Die Datei ist noch nicht gespeichert – es gibt keinen gespeicherten Stand.", "The file has not been saved yet – there is no saved state.").to_string());
        let git = |rev: Rev| -> Result<Side, String> { Ok(Side::Text(Box::new(StaticSide::new(Source::Git { path: need()?, rev })))) };
        let left = match target {
            Target::Disk => Side::Text(Box::new(StaticSide::new(Source::Disk(need()?)))),
            Target::Head => git(Rev::Head)?,
            Target::Index => git(Rev::Index)?,
            Target::Rev(r) => {
                crate::editor_svc::git_base::check_ref(&r)?;
                git(Rev::Commit(r))?
            }
            Target::Clipboard => {
                let t = crate::platform::clipboard::get_text().filter(|t| !t.is_empty()).ok_or_else(|| lang.t("Die Zwischenablage enthält keinen Text.", "The clipboard contains no text.").to_string())?;
                Side::Text(Box::new(StaticSide::clipboard(&t)))
            }
            Target::File(p) => {
                // WSL-Dateien (#392) ungeprüft: sie laden im Hintergrund, ein Fehler erscheint im Vergleich.
                if !doc::is_wsl_path(&p) && !p.is_file() {
                    return Err(format!("{}: {}", lang.t("Datei nicht gefunden", "File not found"), p.display()));
                }
                let id = self.doc_for_file(&p);
                if id == doc {
                    return Err(lang.t("Eine Datei mit sich selbst vergleichen? Wähle eine andere.", "Compare a file with itself? Choose another one.").to_string());
                }
                Side::Doc(id)
            }
            Target::Doc(o) if self.docs.contains_key(&o) && o != doc => Side::Doc(o),
            Target::Doc(_) => return Err(lang.t("Kein solches Dokument", "No such document").to_string()),
            Target::PickFile => return Ok(None),
        };
        Ok(self.open_diff(diff_tab::DiffTab::new(left, diff_tab::Side::Doc(doc), prefs)))
    }

    /// Zwei Dateien vergleichen (Ordnerbaum): `a` links, `b` rechts.
    pub(super) fn compare_paths(&mut self, a: &Path, b: &Path, prefs: &EditorPrefs) -> Option<u64> {
        let file = |p: &Path| doc::is_wsl_path(p) || p.is_file();
        if !file(a) || !file(b) {
            return None;
        }
        let (x, y) = (self.doc_for_file(a), self.doc_for_file(b));
        if x == y {
            return None;
        }
        self.open_diff(diff_tab::DiffTab::new(diff_tab::Side::Doc(x), diff_tab::Side::Doc(y), prefs))
    }

    /// Vergleichs-Reiter schließen. Ein Dokument, das nur dort offen war und ungespeicherte Änderungen hat (z. B. durch
    /// einen Blockpfeil), bleibt als normaler Reiter offen – nichts geht verloren.
    pub(super) fn close_diff(&mut self, g: u64, diff: u64, dir: &Path) {
        let Some(i) = self.panes.groups.get(&g).and_then(|grp| grp.find_diff(diff)) else { return };
        let docs: Vec<u64> = {
            let t = &self.panes.groups[&g].tabs[i];
            std::iter::once(t.doc).chain(t.extra).collect()
        };
        self.panes.close_tab(g, i);
        self.diffs.remove(&diff);
        for d in docs {
            if self.panes.shows(d) {
                continue;
            }
            if self.docs.get(&d).is_some_and(|e| e.dirty()) {
                if self.panes.groups.contains_key(&g) {
                    self.panes.focused = g;
                }
                self.panes.open(d);
            } else {
                self.release_doc(d, dir);
            }
        }
        self.want_focus = true;
    }

    /// Nach dem Teilen: ein kopierter Vergleichs-Reiter bekommt einen eigenen Vergleich (eigener Bildlauf).
    pub(super) fn fix_split_diff(&mut self, g: u64) {
        let Some(t) = self.panes.groups.get(&g).and_then(|grp| grp.active_tab()) else { return };
        let Some(old) = t.diff else { return };
        let Some(copy) = self.diffs.get(&old).map(|d| d.duplicate()) else { return };
        let id = self.alloc_diff();
        self.diffs.insert(id, copy);
        if let Some(t) = self.panes.groups.get_mut(&g).and_then(|grp| grp.active_tab_mut()) {
            t.diff = Some(id);
        }
    }

    /// Seiten eines Vergleichs tauschen (der Reiter merkt sich die Dokumente in neuer Reihenfolge).
    pub(super) fn swap_diff(&mut self, diff: u64) {
        let Some(dt) = self.diffs.get_mut(&diff) else { return };
        dt.swap();
        let docs = dt.tab_docs();
        if let (Some((g, i)), Some((doc, extra))) = (self.panes.diff_tab(diff), docs)
            && let Some(t) = self.panes.groups.get_mut(&g).and_then(|grp| grp.tabs.get_mut(i))
        {
            t.doc = doc;
            t.extra = extra;
        }
    }

    /// Dokument im aktiven Reiter (im Vergleichs-Reiter: die fokussierte Seite, sonst die andere).
    pub(super) fn active_file_doc(&self) -> Option<u64> {
        let t = self.panes.groups.get(&self.panes.focused)?.active_tab()?;
        match t.diff {
            None => Some(t.doc),
            Some(d) => self.diffs.get(&d).and_then(|x| x.focused_doc()),
        }
    }

    /// Aktiver Vergleichs-Reiter der fokussierten Gruppe: (Gruppe, Kennung).
    pub(super) fn active_diff(&self) -> Option<(u64, u64)> {
        let g = self.panes.focused;
        self.panes.groups.get(&g)?.active_tab()?.diff.map(|d| (g, d))
    }

    /// Änderungsmarke in Zeile `line` zurücksetzen: die alten Zeilen wiederherstellen (ein Rückgängig-Schritt). Liefert, ob
    /// sich etwas geändert hat.
    /// Veraltete Marken (der Text hat sich seitdem geändert, die Neuberechnung läuft noch) setzen nichts zurück: Fehler
    /// mit Hinweis, die Marken werden sofort neu berechnet.
    pub(super) fn revert_mark(&mut self, doc: u64, line: usize, lang: crate::state::Lang) -> Result<bool, String> {
        let Some(d) = self.docs.get_mut(&doc) else { return Ok(false) };
        if !d.marks.fresh_for(d.revision()) {
            d.marks.recompute();
            return Err(lang.t("Die Änderungsmarken werden gerade neu berechnet – gleich noch einmal.", "The change markers are being recomputed – try again in a moment.").to_string());
        }
        let mut changed = false;
        if let Some(m) = d.marks.result.clone()
            && let Some(k) = m.at(line)
            && m.hunks[k].revertable
            && let Some(text) = d.text()
        {
            changed = d.apply_user_edit(crate::editor::linemarks::revert_edit(&text, &m.hunks[k]));
        }
        self.mark_popup = None;
        self.want_focus = true;
        Ok(changed)
    }

    /// Linke Seite eines Vergleichs ersetzen (andere Revision). Ein Dokument, das dadurch aus dem Vergleich fällt, bleibt
    /// mit ungespeicherten Änderungen als Datei-Reiter offen (nichts geht verloren, auch kein Hot-Exit-Inhalt), sonst
    /// wird es frei. Der Reiter merkt sich die neuen Dokumente.
    pub(super) fn set_diff_left(&mut self, diff: u64, side: diff_tab::Side, dir: &Path) {
        let Some(dt) = self.diffs.get_mut(&diff) else { return };
        let old = dt.left.doc();
        dt.set_left(side);
        let docs = dt.tab_docs();
        let place = self.panes.diff_tab(diff);
        if let (Some((g, i)), Some((doc, extra))) = (place, docs)
            && let Some(t) = self.panes.groups.get_mut(&g).and_then(|grp| grp.tabs.get_mut(i))
        {
            t.doc = doc;
            t.extra = extra;
        }
        if let Some(o) = old
            && !self.panes.shows(o)
        {
            if self.docs.get(&o).is_some_and(|e| e.dirty()) {
                if let Some((g, _)) = place {
                    self.panes.focused = g;
                }
                self.panes.open(o);
                // Der Vergleich bleibt der aktive Reiter.
                if let Some((g, i)) = self.panes.diff_tab(diff)
                    && let Some(grp) = self.panes.groups.get_mut(&g)
                {
                    grp.active = i;
                }
            } else {
                self.release_doc(o, dir);
            }
        }
    }

    /// Nächste bzw. vorige Änderung (F7 / Umschalt+F7): im Vergleichs-Reiter der nächste Block, im normalen Reiter die
    /// nächste Änderungsmarke (der Cursor springt dorthin). Liefert einen Hinweis, wenn es nichts gibt.
    pub(super) fn step_change(&mut self, forward: bool, prefs: &EditorPrefs, lang: crate::state::Lang) -> Option<String> {
        if let Some((_, d)) = self.active_diff() {
            if let Some(dt) = self.diffs.get_mut(&d) {
                dt.step(forward);
            }
            return None;
        }
        let g = self.panes.focused;
        let t = self.panes.groups.get(&g)?.active_tab()?;
        let d = self.docs.get(&t.doc)?;
        let Some(m) = d.marks.result.clone().filter(|_| prefs.markers) else {
            return Some(lang.t("Keine Änderungsmarken – Ansicht → Änderungsmarken schaltet sie ein.", "No change markers – View → Change markers turns them on.").to_string());
        };
        let (line, _) = t.view.cursor_linecol(d);
        let k = m.step(line, forward)?;
        let target = m.hunks[k].lines.start;
        self.goto_line(target, 0);
        if let Some(p) = self.mark_popup.as_mut().filter(|p| p.group == g) {
            p.line = target;
        }
        None
    }


    /// Sprache eines Dokuments wählen (`None` = wieder erkennen; #312).
    pub(super) fn set_doc_language(&mut self, id: u64, lang: Option<String>) {
        if let Some(d) = self.docs.get_mut(&id) {
            d.set_language(lang);
        }
    }

    /// Modus eines Dokuments wählen (`None` = aus den Einstellungen).
    pub(super) fn set_doc_mode(&mut self, id: u64, mode: Option<crate::editor::syntax::Mode>) {
        if let Some(d) = self.docs.get_mut(&id) {
            d.set_mode(mode);
        }
    }

    pub(super) fn set_hint(&mut self, text: String) {
        self.hint = Some((text, Instant::now()));
    }

    /// Vor einem Aufruf eines Agenten (MCP oder Chat): Stand merken für [`EditorUi::end_agent`].
    pub(super) fn begin_agent(&self) -> AgentMark {
        AgentMark { want_focus: self.want_focus, active: (self.panes.focused, self.active_doc()) }
    }

    /// Nach einem Aufruf eines Agenten: Agenten dürfen Reiter zeigen und in den Editor-Reiter wechseln, nehmen aber nie
    /// die Tastatur – sonst landeten Tasten des Nutzers in einer Datei, die der Agent gerade geöffnet hat. Ein vom Aufruf
    /// gesetztes `want_focus` fällt weg; ist danach eine andere Teilung aktiv oder zeigt sie ein anderes Dokument, gibt
    /// ein Textfeld des Editors, das die Tastatur hat, sie im nächsten Bild ab.
    pub(super) fn end_agent(&mut self, m: AgentMark) {
        self.want_focus = m.want_focus;
        if (self.panes.focused, self.active_doc()) != m.active {
            self.blur = true;
            // Auch nach einem Fensterwechsel bekommt das Textfeld die Tastatur dann nicht zurück.
            self.agent_switches += 1;
        }
    }
}

// ------------------------------------------------------------------------------------------------ Tutorial (#143)

/// Was die Tour vom Editor sieht (Erkennung der Aufgaben).
pub(crate) struct EditorProbe {
    /// Offene Dokumente.
    pub docs: usize,
    /// Teilungen mit mindestens einem Reiter.
    pub groups: usize,
    /// Suchleiste offen.
    pub find: bool,
    /// Cursor (Auswahlen) im aktiven Reiter.
    pub cursors: usize,
}

impl super::App {
    pub(crate) fn editor_probe(&self) -> EditorProbe {
        EditorProbe {
            docs: self.ed.docs.len(),
            groups: self.ed.panes.groups.values().filter(|g| !g.tabs.is_empty()).count(),
            find: self.ed.find.visible,
            cursors: self.ed.focused_view_and_doc().map_or(0, |(v, _)| v.selection_count()),
        }
    }

    /// Ordnerbaum einblenden.
    pub(crate) fn editor_show_tree(&mut self) {
        self.ed.tree_open = true;
    }

    /// Suchleiste öffnen (mit Ersetzen) bzw. schließen.
    pub(crate) fn editor_find_bar(&mut self, open: bool) {
        if open && !self.ed.find.visible {
            self.ed.find.open(true);
        } else if !open && self.ed.find.visible {
            self.ed.find.visible = false;
        }
    }

    /// Ist diese Datei im Editor offen? `Some(ungespeichert)`.
    pub(crate) fn editor_path_state(&self, path: &Path) -> Option<bool> {
        self.ed.doc_for_path(path).and_then(|id| self.ed.docs.get(&id)).map(|d| d.dirty())
    }

    /// Alle Reiter einer Datei schließen, ohne zu fragen (`discard`: auch mit ungespeicherten Änderungen, die dann
    /// verloren sind). `false`, wenn sie ungespeichert ist und `discard` fehlt.
    pub(crate) fn editor_close_path(&mut self, path: &Path, discard: bool) -> bool {
        let Some(id) = self.ed.doc_for_path(path) else { return true };
        if !discard && self.ed.docs.get(&id).is_some_and(|d| d.dirty()) {
            return false;
        }
        while let Some((g, i)) = self.ed.panes.tabs_of(id).first().copied() {
            if self.ed.panes.close_tab(g, i).is_none() {
                break;
            }
        }
        let dir = self.dir.clone();
        self.ed.release_doc(id, &dir);
        true
    }

    /// Hat ein Textfeld des Editors gerade die Tastatur (dann braucht der Editor Esc selbst)?
    pub(crate) fn editor_text_focused(&self, ctx: &eframe::egui::Context) -> bool {
        ctx.memory(|m| m.focused()).is_some_and(|f| self.ed.panes.groups.keys().any(|g| view::text_id(*g) == f))
    }
}

/// Stand vor einem Aufruf eines Agenten ([`EditorUi::begin_agent`]).
#[must_use]
pub(super) struct AgentMark {
    want_focus: bool,
    /// Fokussierte Teilung und ihr aktives Dokument.
    active: (u64, Option<u64>),
}

// ------------------------------------------------------------------------------------------------ Schnittstelle (App)

// Schnittstelle für MCP-Werkzeuge, Einstellungen und Explorer-Kontextmenü (`app::editor_services`). Beschreibung:
// `notes/editor-ui-api.md` im Testordner.
impl super::App {
    /// Einstellungen des Editors (Einstellungen → Editor). Solange es dort nichts gibt: Standardwerte mit der Schriftgröße
    /// des Terminals.
    pub(crate) fn editor_prefs(&self) -> EditorPrefs {
        // Einstellungen → Editor (Services, `app::editor_services`).
        self.editor_prefs_from_settings()
    }

    /// Jedes Bild im Hauptdurchgang, auch außerhalb des Editor-Reiters: fertige Formatier-Läufe übernehmen (#381),
    /// automatisches Speichern (Auslöser „automatisch“ für die Aktionen beim Speichern, #383).
    pub(crate) fn editor_tick(&mut self, ctx: &eframe::egui::Context) {
        self.ed.poll_formats(self.settings.language);
        let prefs = self.editor_prefs();
        // Smart-Modus (#367): Server abfragen, Dokumente senden, Diagnosen übernehmen.
        let smart = self.settings.editor.smart.clone();
        let dir = self.dir.clone();
        if self.ed.smart_tick(&prefs, &smart, &dir) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else if self.ed.smart.servers.running() > 0 {
            // Antworten der Server kommen in Hilfsfäden an: ruhig nachsehen, solange einer läuft.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        let Some(delay) = prefs.autosave else { return };
        let mut next: Option<std::time::Duration> = None;
        let mut due = Vec::new();
        for (id, d) in self.ed.docs.iter() {
            let Some(at) = d.edited_at else { continue };
            if d.path.is_none() || !d.dirty() || d.disk_conflict {
                continue;
            }
            // WSL (#392): Speichern/Lesen läuft noch im Hintergrund – nicht jetzt (scheiterte mit „wird noch
            // gespeichert“), aber `edited_at` bleibt, damit es danach erneut versucht wird.
            if d.disk_busy() {
                let rest = std::time::Duration::from_millis(250);
                next = Some(next.map_or(rest, |n| n.min(rest)));
                continue;
            }
            let age = at.elapsed();
            if age >= delay {
                due.push(*id);
            } else {
                let rest = delay - age;
                next = Some(next.map_or(rest, |n| n.min(rest)));
            }
        }
        let dir = self.dir.clone();
        for id in due {
            let ok = self.editor_save_with(id, None, crate::save_actions::SaveOrigin::Autosave).is_ok();
            if let Some(d) = self.ed.docs.get_mut(&id) {
                // WSL (#392): Speichern läuft noch im Hintergrund – die Hot-Exit-Kopie bleibt, bis es geschrieben ist.
                if ok && !d.disk_busy() {
                    d.drop_hot(&dir);
                }
                d.edited_at = None;
            }
        }
        if let Some(n) = next {
            ctx.request_repaint_after(n);
        }
    }

    /// Geöffneter Ordner des Editors (Wurzel des Baums; für „In Dateien suchen“).
    pub(crate) fn editor_root(&self) -> Option<PathBuf> {
        self.ed.root.clone()
    }

    /// Datei öffnen und den Cursor auf Zeile/Spalte (ab 0) setzen (Treffer der Suche in Dateien).
    pub(crate) fn editor_open_at(&mut self, path: &Path, line: usize, col: usize) -> Result<u64, String> {
        let id = self.editor_open(path, true, false)?;
        self.ed.goto_line(line, col);
        Ok(id)
    }

    /// Dateien, die der Editor seit dem letzten Aufruf selbst gelesen, gespeichert oder neu geladen hat (die Überwachung
    /// setzt für sie den Vergleichsstand neu, `watch.baseline`).
    pub(crate) fn editor_take_touched(&mut self) -> Vec<PathBuf> {
        self.ed.docs.values_mut().filter_map(|d| if std::mem::take(&mut d.touched) { d.path.clone() } else { None }).collect()
    }

    /// Änderung eines Agenten anwenden: `edits` = (Bereich in **Bytes** des Textes mit `\n`-Zeilenenden wie
    /// `editor_text`, neuer Text), alle bezogen auf den aktuellen Text, ohne Überlappung. Eine rückgängig machbare
    /// Änderung, ungespeichert, als „von einem Agenten“ markiert; die Cursor aller Ansichten folgen (sie gleichen sich
    /// beim nächsten Zeichnen ab). Ein Bereich, der nicht auf Zeichengrenzen liegt, ist ein Fehler.
    pub(crate) fn editor_apply_agent_edit_bytes(&mut self, id: u64, edits: Vec<(std::ops::Range<usize>, String)>) -> Result<(), String> {
        let lang = self.settings.language;
        let prefs = self.editor_prefs();
        let d = self.ed.docs.get_mut(&id).ok_or_else(|| lang.t("Kein solches Dokument", "No such document").to_string())?;
        d.ensure_loaded_now(&prefs);
        let doc = d.doc.as_ref().ok_or_else(|| lang.t("Datei konnte nicht gelesen werden.", "The file could not be read.").to_string())?;
        let len = doc.len_bytes();
        let mut chars = Vec::with_capacity(edits.len());
        for (r, t) in edits {
            let ok = |b: usize| b <= len && doc.char_to_byte(doc.byte_to_char(b)) == b;
            if r.start > r.end || !ok(r.start) || !ok(r.end) {
                return Err(lang.t("Bereich liegt nicht auf Zeichengrenzen", "Range is not on character boundaries").to_string());
            }
            chars.push((doc.byte_to_char(r.start)..doc.byte_to_char(r.end), t));
        }
        d.apply_agent_edit(chars, lang)
    }

    /// Datei im Editor öffnen (oder ihren Reiter zeigen). `show`: in den Editor-Reiter wechseln. `by_agent`: der Reiter
    /// wird als „von einem Agenten geöffnet“ markiert und bekommt **nicht** die Tastatur (nur Aktionen des Nutzers
    /// fokussieren das Textfeld, [`EditorUi::end_agent`]). Liefert die Dokument-Id; Fehler, wenn der Pfad keine lesbare
    /// Datei ist (Text für Menschen in der eingestellten Sprache).
    /// Datei für den Nutzer öffnen (#392: Strg+Klick auf einen Pfad, `--edit`), optional an Zeile `line` (ab 1).
    /// WSL-Dateien (`\\wsl.localhost\…`) werden nie im UI-Thread geprüft oder gelesen (das könnte die WSL-VM
    /// starten): Sie laden im Hintergrund, der Sprung zur Zeile folgt dann.
    pub(crate) fn editor_open_user(&mut self, path: &Path, line: Option<usize>) -> Result<u64, String> {
        let lang = self.settings.language;
        let wsl = crate::wsl::is_wsl_unc(&path.to_string_lossy());
        if !wsl && !path.is_file() {
            return Err(lang.t("Datei nicht gefunden", "File not found").to_string());
        }
        let id = self.ed.open_path(path, false);
        let prefs = self.editor_prefs();
        if let Some(d) = self.ed.docs.get_mut(&id) {
            d.ensure_loaded(&prefs);
            if let Some(e) = &d.load_error {
                return Err(e.clone());
            }
        }
        self.set_mode(super::titlebar::Mode::Editor);
        if let Some(l) = line.filter(|l| *l > 0) {
            self.ed.pending_goto = Some((id, l - 1));
            self.ed.apply_pending_goto();
        }
        Ok(id)
    }

    pub(crate) fn editor_open(&mut self, path: &Path, show: bool, by_agent: bool) -> Result<u64, String> {
        if by_agent {
            let mark = self.ed.begin_agent();
            let r = self.editor_open_inner(path, show, true);
            self.ed.end_agent(mark);
            return r;
        }
        self.editor_open_inner(path, show, false)
    }

    fn editor_open_inner(&mut self, path: &Path, show: bool, by_agent: bool) -> Result<u64, String> {
        let lang = self.settings.language;
        if !path.is_file() {
            return Err(lang.t("Datei nicht gefunden", "File not found").to_string());
        }
        let id = self.ed.open_path(path, false);
        let prefs = self.editor_prefs();
        if let Some(d) = self.ed.docs.get_mut(&id) {
            d.ensure_loaded_now(&prefs);
            if let Some(e) = &d.load_error {
                return Err(e.clone());
            }
            if by_agent {
                d.opened_by_agent = true;
            }
        }
        if show {
            self.set_mode(super::titlebar::Mode::Editor);
        }
        Ok(id)
    }

    /// Neue unbenannte Datei mit Inhalt (z. B. von einem Agenten; dann ohne Tastatur wie bei [`App::editor_open`]).
    /// Liefert die Dokument-Id.
    pub(crate) fn editor_new(&mut self, text: &str, show: bool, by_agent: bool) -> u64 {
        let mark = by_agent.then(|| self.ed.begin_agent());
        let id = self.ed.new_untitled(Some(text));
        if let Some(d) = self.ed.docs.get_mut(&id) {
            d.opened_by_agent = by_agent;
            if by_agent {
                d.agent_edited = true;
            }
        }
        if show {
            self.set_mode(super::titlebar::Mode::Editor);
        }
        if let Some(m) = mark {
            self.ed.end_agent(m);
        }
        id
    }

    /// Offene Dokumente (für `editor_list`).
    pub(crate) fn editor_list(&self) -> Vec<DocInfo> {
        let active = self.ed.active_doc();
        self.ed.panes.docs().into_iter().filter_map(|id| self.ed.docs.get(&id).map(|d| d.info(id, active == Some(id), &self.editor_prefs()))).collect()
    }

    /// Text eines offenen Dokuments (lädt es bei Bedarf). `None`, wenn es die Id nicht gibt oder die Datei nicht lesbar ist.
    /// Cursor und Auswahlen des Nutzers in einem Dokument (#394, `editor_read`): die Ansicht der fokussierten Teilung,
    /// sonst die erste, die es zeigt; Hauptcursor zuerst. (Zeile, Spalte) ab 0.
    pub(crate) fn editor_selections(&mut self, id: u64) -> Option<Vec<((usize, usize), (usize, usize))>> {
        let doc = self.ed.docs.get(&id)?.doc.as_ref()?;
        let focused = self.ed.panes.focused;
        let mut views: Vec<(bool, &text::View)> = Vec::new();
        for (g, grp) in &self.ed.panes.groups {
            for (i, t) in grp.tabs.iter().enumerate() {
                if t.doc == id && t.diff.is_none() {
                    views.push((*g == focused && grp.active == i, &t.view));
                }
            }
        }
        views.sort_by_key(|(f, _)| !*f);
        let mut sels = views.first()?.1.sels.clone()?;
        doc.sync(&mut sels);
        let lc = |p: usize| {
            let l = crate::editor::text::pos_to_linecol(doc.rope(), p.min(doc.len_chars()));
            (l.line, l.col)
        };
        let p = sels.primary_index();
        let mut v: Vec<((usize, usize), (usize, usize))> = vec![(lc(sels.primary().anchor), lc(sels.primary().head))];
        v.extend(sels.iter().enumerate().filter(|(i, _)| *i != p).map(|(_, s)| (lc(s.anchor), lc(s.head))));
        Some(v)
    }

    pub(crate) fn editor_text(&mut self, id: u64) -> Option<String> {
        let prefs = self.editor_prefs();
        let d = self.ed.docs.get_mut(&id)?;
        // WSL-Dateien (#392) nie im UI-Thread abwarten: noch nicht geladen = `None` (später erneut).
        if d.path.as_deref().is_some_and(doc::is_wsl_path) {
            d.ensure_loaded(&prefs);
        } else {
            d.ensure_loaded_now(&prefs);
        }
        d.text()
    }

    /// Reiter eines Dokuments zeigen (wechselt in den Editor-Reiter) – für Agenten (`editor_open` mit `show`): die
    /// Teilung des Reiters wird die aktive, die Tastatur bekommt sein Textfeld aber nicht ([`EditorUi::end_agent`]).
    pub(crate) fn editor_focus(&mut self, id: u64) -> bool {
        if !self.ed.docs.contains_key(&id) {
            return false;
        }
        let mark = self.ed.begin_agent();
        if let Some((g, i)) = self.ed.panes.file_tabs_of(id).first().copied().or_else(|| self.ed.panes.tabs_of(id).first().copied()) {
            self.ed.panes.focused = g;
            if let Some(grp) = self.ed.panes.groups.get_mut(&g) {
                grp.active = i;
            }
        } else {
            self.ed.panes.open(id);
        }
        self.set_mode(super::titlebar::Mode::Editor);
        self.ed.end_agent(mark);
        true
    }

    /// Hat der aktive Reiter genau eine nicht leere Auswahl (dann formatiert ein Befehl nur sie)?
    pub(crate) fn editor_active_selection(&self) -> bool {
        self.ed.focused_view_and_doc().is_some_and(|(v, d)| {
            let (Some(s), Some(doc)) = (v.sels.as_ref(), d.doc.as_ref()) else { return false };
            let mut s = s.clone();
            doc.sync(&mut s);
            !s.is_multi() && !s.primary().is_empty()
        })
    }

    /// Gibt es einen aktiven Reiter (für die Einträge der Befehls-Palette)?
    pub(crate) fn ed_has_active_doc(&self) -> bool {
        self.ed.active_doc().is_some()
    }

    /// Formatieren über die Palette bzw. von außen (#381): Befehl auf den aktiven Reiter der fokussierten Teilung.
    pub(crate) fn editor_format_active(&mut self, ctx: &eframe::egui::Context, op: crate::editor::format::Op) {
        let g = self.ed.panes.focused;
        let lang = self.settings.language;
        let c = ctx.clone();
        self.ed.start_format(g, op, false, move || c.request_repaint(), lang);
    }

    /// Art des aktiven Dokuments für Formatier-Befehle (Palette; `None` = keins oder weder JSON noch XML).
    pub(crate) fn editor_active_format_kind(&self) -> Option<crate::editor::format::Kind> {
        self.ed.active_doc().and_then(|id| self.ed.docs.get(&id)).and_then(|d| d.format_kind())
    }

    /// Lauf für MCP `format_file` starten (#381; der Aufrufer hat Stufe, Schalter und Dokument geprüft).
    pub(crate) fn editor_start_agent_format(&mut self, ctx: &eframe::egui::Context, doc: u64, op: crate::editor::format::Op, confirmed: bool) -> Result<crate::editor_svc::format_job::Job, String> {
        let prefs = self.editor_prefs();
        let c = ctx.clone();
        self.ed.start_agent_format(doc, op, confirmed, &prefs, move || c.request_repaint())
    }

    /// Ergebnis eines Agenten-Laufs anwenden: markiert, ungespeichert, ein Rückgängig-Schritt, nie mit Tastatur
    /// ([`EditorUi::end_agent`]).
    pub(crate) fn editor_apply_agent_format(&mut self, doc: u64, revision: u64, edits: Vec<crate::editor::TextEdit>) -> Result<(), (&'static str, &'static str)> {
        let mark = self.ed.begin_agent();
        let r = self.ed.apply_agent_format(doc, revision, edits);
        self.ed.end_agent(mark);
        r
    }

    /// Dokument speichern (für `editor_save` und Chats; der Aufrufer prüft die Regeln, z. B. nur vorhandene Dateien).
    /// Auslöser „Agent“ für die Aktionen beim Speichern (#383).
    pub(crate) fn editor_save_doc(&mut self, id: u64) -> Result<(), String> {
        self.editor_save_with(id, None, crate::save_actions::SaveOrigin::Agent)
    }

    /// Pfade der offenen Dateien (für die Überwachung auf Änderungen).
    pub(crate) fn editor_open_paths(&self) -> Vec<PathBuf> {
        self.ed.docs.values().filter_map(|d| d.path.clone()).collect()
    }

    /// Eine offene Datei wurde auf der Platte geändert (meldet die Überwachung): ohne eigene Änderungen leise neu laden (mit
    /// kurzem Hinweis), sonst die Hinweisleiste *Meine behalten* / *Von der Platte laden* / *Unterschied anzeigen*.
    pub(crate) fn editor_disk_changed(&mut self, path: &Path) {
        let lang = self.settings.language;
        let prefs = self.editor_prefs();
        let Some(id) = self.ed.doc_for_path(path) else { return };
        let Some(d) = self.ed.docs.get_mut(&id) else { return };
        match d.disk_changed(&prefs, lang) {
            doc::DiskReaction::Reloaded(summary) => {
                let name = d.title(lang);
                let extra = if summary.is_empty() { String::new() } else { format!(" ({summary})") };
                self.ed.set_hint(format!("{name}: {}{extra}", lang.t("von der Platte neu geladen", "reloaded from disk")));
            }
            doc::DiskReaction::Asked | doc::DiskReaction::Nothing => {}
        }
    }
}

impl super::App {
    /// Einträge der Befehlspalette im Editor-Reiter (#385): Vergleichen mit …, Änderungsmarken, nächste/vorige Änderung.
    pub(crate) fn editor_palette_items(&self) -> Vec<(String, String, EditorAction)> {
        let prefs = self.editor_prefs();
        let (next, prev) = (self.binding_text("ed_next_change"), self.binding_text("ed_prev_change"));
        let mut v = diff_menu::palette_entries(&self.ed, &prefs, self.settings.language, &next, &prev);
        // Auswahl und Kommentar (#394)
        v.extend(menu::palette_entries(&self.ed, &prefs, self.settings.language));
        v.into_iter().map(|(l, h, c)| (l, h, EditorAction(c))).collect()
    }

    /// Befehl der Palette ausführen.
    pub(crate) fn editor_run_action(&mut self, ctx: &eframe::egui::Context, a: EditorAction) {
        self.editor_cmd(ctx, a.0);
    }
}

// ------------------------------------------------------------------------------------------------ Bilder für die Doku

#[cfg(feature = "readme-shots")]
pub(crate) fn shot_target_disk() -> diff_tab::Target {
    diff_tab::Target::Disk
}

#[cfg(feature = "readme-shots")]
impl super::App {
    /// Bilder `editor-diff`/`editor-gutter` (#385): ungespeicherte Änderungen in der Beispieldatei `src/orders.rs`.
    pub(crate) fn editor_shot_edits(&mut self, id: u64) {
        let Some(d) = self.ed.docs.get_mut(&id) else { return };
        let Some(old) = d.text() else { return };
        let new = old
            .replace("//! Orders: create, find and cancel.", "//! Orders: create, find, cancel and refund.")
            .replace("    pub total_cents: u64,
    pub cancelled: bool,", "    pub total_cents: u64,
    pub currency: String,
    pub cancelled: bool,")
            .replace("total_cents: 1999, cancelled: false", "total_cents: 2499, currency: \"EUR\".into(), cancelled: false")
            .replace("    pub fn find_order(&self, id: u64) -> Option<&Order> {
        self.orders.get(&id)
    }

", "");
        d.apply_user_edit(crate::editor::compare::minimal_edit(&old, &new));
    }

    /// Fenster der Änderungsmarke unter Zeile `line` (ab 0) zeigen.
    pub(crate) fn editor_shot_popup(&mut self, id: u64, line: usize) {
        let g = self.ed.panes.focused;
        let line = self.ed.docs.get(&id).and_then(|d| d.marks.result.clone()).and_then(|m| m.hunks.iter().map(|h| h.lines.start).min_by_key(|s| s.abs_diff(line))).unwrap_or(line);
        self.ed.mark_popup = Some(marks::MarkPopup { group: g, doc: id, line });
    }
}
