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
//! - `format`: JSON/XML formatieren im Hintergrund, Fortschritt, Meldungen, Formatieren beim Speichern (#381),
//! - `tree`: Ordnerbaum, `persist`: `editor-state.json`.
//!
//! Der Zustand ist für alle Fenster gemeinsam (`App::ed`, nicht in `view_fields!`): Ein weiteres Fenster im Editor-Reiter
//! zeigt dieselben Dateien und Teilungen. Ein Dokument gibt es genau einmal (`docs`), auch wenn es in mehreren Teilungen
//! offen ist; jeder Reiter hat seine eigene Ansicht (Cursor, Bildlauf).

mod doc;
mod find;
pub(crate) mod format;
mod hscroll;
#[cfg(test)]
mod hscroll_tests;
mod menu;
mod panes;
mod persist;
mod text;
mod tree;
mod view;
#[cfg(test)]
mod tests;

pub(crate) use doc::DocInfo;
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
    /// Formatieren beim Speichern (#381, Standard aus).
    pub format_on_save_json: bool,
    pub format_on_save_xml: bool,
    /// Tastenkürzel für die Menüs (Kennung → Kürzel wie in `keys::DEFAULTS`, z. B. `ed_format` → `Alt+Shift+F`).
    pub keys: BTreeMap<String, String>,
}

/// Kennungen der Kürzel, die die Menüleiste anzeigt.
pub(crate) const MENU_KEYS: [&str; 7] = ["split_r", "split_d", "palette", "ed_format", "ed_minify", "ed_validate", "ed_sort_keys"];

impl Default for EditorPrefs {
    fn default() -> Self {
        let keys = MENU_KEYS.iter().filter_map(|id| crate::keys::DEFAULTS.iter().find(|(i, _)| i == id).map(|(i, d)| (i.to_string(), d.to_string()))).collect();
        EditorPrefs {
            font_size: 14.0,
            large_file_bytes: crate::editor::DEFAULT_LARGE_FILE_THRESHOLD,
            autosave: None,
            syntax: SyntaxPrefs::default(),
            menu_bar: true,
            format_on_save_json: false,
            format_on_save_xml: false,
            keys,
        }
    }
}

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
    /// Fenster „Unterschied anzeigen“ (Datei auf der Platte gegen den Puffer) für dieses Dokument.
    pub(super) diff_doc: Option<u64>,
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
        self.root = s.root.map(PathBuf::from).filter(|p| p.is_dir());
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
            if path.as_ref().is_some_and(|p| !p.is_file()) && hot.is_none() {
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
        for g in s.groups {
            let tabs: Vec<panes::Tab<View>> = g.tabs.into_iter().filter(|t| self.docs.contains_key(&t.doc)).map(|t| panes::Tab { doc: t.doc, view: View::restored(t.line, t.col, t.top) }).collect();
            self.panes.insert_group(g.id.max(1), panes::Group { tabs, active: g.active });
        }
        self.panes.layout = s.layout;
        self.panes.focused = s.focused;
        self.panes.normalize();
        // Dokumente ohne Reiter wieder vergessen.
        let shown = self.panes.docs();
        self.docs.retain(|id, _| shown.contains(id));
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
                            persist::SavedTab { doc: t.doc, line, col, top: t.view.top_f32() }
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
            if self.diff_doc == Some(doc) {
                self.diff_doc = None;
            }
        }
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
    /// automatisches Speichern.
    pub(crate) fn editor_tick(&mut self, ctx: &eframe::egui::Context) {
        self.ed.poll_formats(self.settings.language);
        let prefs = self.editor_prefs();
        let Some(delay) = prefs.autosave else { return };
        let lang = self.settings.language;
        let mut next: Option<std::time::Duration> = None;
        for d in self.ed.docs.values_mut() {
            let Some(at) = d.edited_at else { continue };
            if d.path.is_none() || !d.dirty() || d.disk_conflict {
                continue;
            }
            let age = at.elapsed();
            if age >= delay {
                if d.save(None, lang).is_ok() {
                    d.drop_hot(&self.dir);
                }
                d.edited_at = None;
            } else {
                let rest = delay - age;
                next = Some(next.map_or(rest, |n| n.min(rest)));
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
    pub(crate) fn editor_text(&mut self, id: u64) -> Option<String> {
        let prefs = self.editor_prefs();
        let d = self.ed.docs.get_mut(&id)?;
        d.ensure_loaded_now(&prefs);
        d.text()
    }

    /// Reiter eines Dokuments zeigen (wechselt in den Editor-Reiter) – für Agenten (`editor_open` mit `show`): die
    /// Teilung des Reiters wird die aktive, die Tastatur bekommt sein Textfeld aber nicht ([`EditorUi::end_agent`]).
    pub(crate) fn editor_focus(&mut self, id: u64) -> bool {
        if !self.ed.docs.contains_key(&id) {
            return false;
        }
        let mark = self.ed.begin_agent();
        if let Some((g, i)) = self.ed.panes.tabs_of(id).first().copied() {
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

    /// Dokument speichern (für `editor_save`; der Aufrufer prüft die Regeln, z. B. nur vorhandene Dateien).
    pub(crate) fn editor_save_doc(&mut self, id: u64) -> Result<(), String> {
        let lang = self.settings.language;
        let d = self.ed.docs.get_mut(&id).ok_or_else(|| lang.t("Kein solches Dokument", "No such document").to_string())?;
        d.save(None, lang)
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
