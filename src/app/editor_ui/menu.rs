//! Menüleiste je Teilung (#381, 3.2.0): *Datei · Bearbeiten · Ansicht · Format* (wie Notepad++/Sublime Text) über der
//! Reiterleiste jeder Teilung. Die Einträge rufen die vorhandenen Befehle des Editors auf; Kürzel stehen rechtsbündig,
//! nicht passende Einträge sind ausgegraut und sagen beim Überfahren warum. In schmalen Teilungen klappen die vier Menüs
//! zu einem einzigen *Menü ▾* zusammen. Abschaltbar unter *Einstellungen → Editor* (`menu_bar`, Standard an).
//!
//! Aufbau: [`menus`] liefert das Modell (rein, testbar: Texte, Kürzel, ausgegraut + Grund, Häkchen, Befehle),
//! [`menu_bar`] zeichnet es.

use super::doc::Op as EdOp;
use super::view::Cmd;
use super::EditorUi;
use crate::editor::format::{Kind, Op as FmtOp};
use crate::editor::syntax::Mode;
use crate::layout::Dir;
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Rect, Ui};

#[cfg(test)]
thread_local! {
    /// Gezeichnete Menütitel und Einträge (Kennung, Fläche, aktiv) – für die Tests ohne Fenster.
    pub(super) static SEEN: std::cell::RefCell<Vec<(&'static str, Rect, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Höhe der Menüleiste.
pub(super) const MENU_H: f32 = 26.0;

/// Ein Eintrag eines Menüs.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    Item(Item),
    Sep,
    /// Untermenü *Sprache* (eingebaute Sprachen, *Automatisch erkennen*).
    Languages { doc: u64, current: Option<String>, chosen: bool },
}

/// Ein Menüpunkt.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Item {
    /// Feste Kennung (Tests, Tutorial).
    pub id: &'static str,
    pub label: String,
    /// Kürzel (rechtsbündig), leer = keins.
    pub shortcut: String,
    pub enabled: bool,
    /// Grund für ausgegraut bzw. Erklärung.
    pub tip: Option<String>,
    /// Schalter: an/aus (`None` = kein Schalter).
    pub checked: Option<bool>,
    /// Befehle beim Klick (vorher wird die Teilung fokussiert).
    pub cmds: Vec<Cmd>,
}

/// Ein Menü der Leiste.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Menu {
    pub id: &'static str,
    pub title: &'static str,
    pub entries: Vec<Entry>,
}

impl Menu {
    /// Eintrag mit Kennung (Tests).
    #[cfg(test)]
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.entries.iter().find_map(|e| match e {
            Entry::Item(i) if i.id == id => Some(i),
            _ => None,
        })
    }
}

/// Kürzel aus den Einstellungen (Anzeigetext in der Sprache der Oberfläche; leer = keins).
pub(super) fn key(prefs: &super::EditorPrefs, lang: Lang, id: &str) -> String {
    prefs.keys.get(id).map(|k| crate::keys::pretty(k, lang == Lang::De)).unwrap_or_default()
}

/// Festes Kürzel des Editors in der Sprache der Oberfläche (Strg/Umschalt).
fn fixed(k: &str, lang: Lang) -> String {
    crate::keys::pretty(k, lang == Lang::De)
}

fn item(id: &'static str, label: impl Into<String>, shortcut: String, cmds: Vec<Cmd>) -> Item {
    Item { id, label: label.into(), shortcut, enabled: true, tip: None, checked: None, cmds }
}

impl Item {
    fn off(mut self, why: Option<String>) -> Self {
        if let Some(w) = why {
            self.enabled = false;
            self.tip = Some(w);
        }
        self
    }

    fn check(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }
}

/// Das Modell der Menüs für die Teilung `g`.
pub(super) fn menus(ed: &mut EditorUi, g: u64, prefs: &super::EditorPrefs, lang: Lang) -> Vec<Menu> {
    let tab = ed.panes.groups.get(&g).and_then(|grp| grp.active_tab()).map(|t| (t.doc, t.view.sels.clone()));
    // Vergleichs-Reiter (#385): *Datei* gilt dem Dokument der fokussierten Seite; Bearbeiten/Format gehen dort nicht.
    let in_diff = ed.panes.groups.get(&g).and_then(|grp| grp.active_tab()).and_then(|t| t.diff);
    let doc_id = match in_diff {
        Some(d) => ed.diffs.get(&d).and_then(|x| x.focused_doc()),
        None => tab.as_ref().map(|t| t.0),
    };
    let running = doc_id.is_some_and(|d| ed.fmt.job_for(d).is_some());
    let syn = doc_id.and_then(|id| ed.docs.get_mut(&id)).filter(|d| d.doc.is_some()).map(|d| d.syntax_status(prefs, lang));
    let d = doc_id.and_then(|id| ed.docs.get(&id));
    let loaded = d.and_then(|d| d.doc.as_ref());
    let kind = d.and_then(|d| d.format_kind());
    let selection = match (&tab, loaded) {
        (Some((_, Some(s))), Some(doc)) => {
            let mut s = s.clone();
            doc.sync(&mut s);
            !s.is_multi() && !s.primary().is_empty()
        }
        _ => false,
    };
    let any_dirty = ed.docs.values().any(|x| x.dirty() && x.path.is_some());
    let no_doc = || Some(lang.t("Keine Datei offen", "No file open").to_string());
    // Grund, warum ein Befehl für das aktive Dokument nicht geht.
    let need_doc = || -> Option<String> {
        match (doc_id, loaded) {
            (None, _) => no_doc(),
            (Some(_), None) => Some(lang.t("Die Datei wird noch geladen.", "The file is still loading.").to_string()),
            _ => None,
        }
    };
    // Grund für Befehle, die einen Text-Reiter brauchen (nicht im Vergleich).
    let need_text = || -> Option<String> { in_diff.map(|_| lang.t("Nicht im Vergleichs-Reiter", "Not in a comparison tab").to_string()).or_else(need_doc) };
    let f = |c: Cmd| vec![Cmd::FocusGroup(g), c];

    // ---- Datei
    let mut file = vec![
        Entry::Item(item("new", lang.t("Neue Datei", "New file"), fixed("Ctrl+N", lang), f(Cmd::NewFile))),
        Entry::Item(item("open", lang.t("Datei öffnen …", "Open file …"), fixed("Ctrl+O", lang), f(Cmd::OpenFileDialog))),
        Entry::Item(item("open_folder", lang.t("Ordner öffnen …", "Open folder …"), fixed("Ctrl+Shift+O", lang), f(Cmd::OpenFolderDialog))),
        Entry::Sep,
    ];
    match doc_id {
        Some(id) => {
            file.push(Entry::Item(item("save", lang.t("Speichern", "Save"), fixed("Ctrl+S", lang), f(Cmd::Save(id))).off(need_doc())));
            file.push(Entry::Item(item("save_as", lang.t("Speichern unter …", "Save as …"), fixed("Ctrl+Shift+S", lang), f(Cmd::SaveAs(id))).off(need_doc())));
        }
        None => {
            file.push(Entry::Item(item("save", lang.t("Speichern", "Save"), fixed("Ctrl+S", lang), vec![]).off(no_doc())));
            file.push(Entry::Item(item("save_as", lang.t("Speichern unter …", "Save as …"), fixed("Ctrl+Shift+S", lang), vec![]).off(no_doc())));
        }
    }
    file.push(Entry::Item(
        item("save_all", lang.t("Alle speichern", "Save all"), String::new(), f(Cmd::SaveAll)).off((!any_dirty).then(|| lang.t("Keine ungespeicherten Dateien", "No unsaved files").to_string())),
    ));
    file.push(Entry::Sep);
    file.extend(on_save_entries(doc_id, ed.on_save.paused, need_doc(), prefs, lang, f));
    // Vergleichen mit … (#385)
    let saved = doc_id.and_then(|d| ed.docs.get(&d)).is_some_and(|d| d.path.is_some());
    let why_cmp = need_doc().or_else(|| (!saved).then(|| lang.t("Die Datei ist noch nicht gespeichert.", "The file has not been saved yet.").to_string()));
    let cmp = super::diff_menu::file_entries_for(ed, doc_id, lang);
    diff_items(&mut file, &FILE_DIFF_IDS, cmp, |i| if i == 0 || i == 5 { need_doc() } else { why_cmp.clone() }, f);
    file.push(Entry::Sep);
    file.push(Entry::Item(match doc_id {
        Some(id) => item("close", lang.t("Reiter schließen", "Close tab"), fixed("Ctrl+W", lang), f(Cmd::Close(vec![(g, id)]))),
        None => item("close", lang.t("Reiter schließen", "Close tab"), fixed("Ctrl+W", lang), vec![]).off(no_doc()),
    }));

    // ---- Bearbeiten
    let op = |id: &'static str, label: &str, k: &str, o: EdOp| Entry::Item(item(id, label.to_string(), fixed(k, lang), f(Cmd::EditOp(g, o))).off(need_text()));
    let can_undo = loaded.is_some_and(|d| d.can_undo());
    let can_redo = loaded.is_some_and(|d| d.can_redo());
    let mut undo = item("undo", lang.t("Rückgängig", "Undo"), fixed("Ctrl+Z", lang), f(Cmd::EditOp(g, EdOp::Undo))).off(need_text());
    if undo.enabled && !can_undo {
        undo = undo.off(Some(lang.t("Nichts rückgängig zu machen", "Nothing to undo").to_string()));
    }
    let mut redo = item("redo", lang.t("Wiederholen", "Redo"), fixed("Ctrl+Y", lang), f(Cmd::EditOp(g, EdOp::Redo))).off(need_text());
    if redo.enabled && !can_redo {
        redo = redo.off(Some(lang.t("Nichts zu wiederholen", "Nothing to redo").to_string()));
    }
    let edit = vec![
        Entry::Item(undo),
        Entry::Item(redo),
        Entry::Sep,
        op("cut", lang.t("Ausschneiden", "Cut"), "Ctrl+X", EdOp::Cut),
        op("copy", lang.t("Kopieren", "Copy"), "Ctrl+C", EdOp::Copy),
        op("paste", lang.t("Einfügen", "Paste"), "Ctrl+V", EdOp::PasteFromClipboard),
        op("select_all", lang.t("Alles markieren", "Select all"), "Ctrl+A", EdOp::SelectAll),
        Entry::Sep,
        Entry::Item(item("find", lang.t("Suchen …", "Find …"), fixed("Ctrl+F", lang), f(Cmd::OpenFind(false))).off(need_text())),
        Entry::Item(item("replace", lang.t("Ersetzen …", "Replace …"), fixed("Ctrl+H", lang), f(Cmd::OpenFind(true))).off(need_text())),
        Entry::Item(item("find_files", lang.t("In Dateien suchen …", "Find in files …"), fixed("Ctrl+Shift+F", lang), f(Cmd::FindInFiles))),
        Entry::Item(item("goto", lang.t("Gehe zu Zeile …", "Go to line …"), fixed("Ctrl+G", lang), f(Cmd::GotoLineAsk)).off(need_text())),
        Entry::Sep,
        op("next_occurrence", lang.t("Nächstes Vorkommen dazu", "Add next occurrence"), "Ctrl+D", EdOp::SelectNextOccurrence),
        op("all_occurrences", lang.t("Alle Vorkommen markieren", "Select all occurrences"), "Ctrl+Shift+L", EdOp::SelectAllOccurrences),
        op("select_line", lang.t("Zeile markieren", "Select line"), "Ctrl+L", EdOp::SelectLine),
        op("delete_line", lang.t("Zeile löschen", "Delete line"), "Ctrl+Shift+K", EdOp::DeleteLine),
        op("line_up", lang.t("Zeile nach oben", "Move line up"), "Alt+Up", EdOp::MoveLines(false)),
        op("line_down", lang.t("Zeile nach unten", "Move line down"), "Alt+Down", EdOp::MoveLines(true)),
    ];

    // ---- Ansicht
    let mut view = vec![
        Entry::Item(item("wrap", lang.t("Zeilenumbruch", "Word wrap"), fixed("Alt+Z", lang), f(Cmd::ToggleWrap)).check(ed.wrap)),
        Entry::Item(item("minimap", lang.t("Minimap", "Minimap"), String::new(), f(Cmd::ToggleMinimap)).check(ed.minimap)),
        Entry::Item(item("tree", lang.t("Ordnerbaum", "Folder tree"), fixed("Ctrl+B", lang), f(Cmd::ToggleTree)).check(ed.tree_open)),
        Entry::Sep,
        Entry::Item(item("split_right", lang.t("Rechts teilen", "Split right"), key(prefs, lang, "split_r"), f(Cmd::Split(g, Dir::Horizontal)))),
        Entry::Item(item("split_down", lang.t("Unten teilen", "Split down"), key(prefs, lang, "split_d"), f(Cmd::Split(g, Dir::Vertical)))),
        Entry::Sep,
    ];
    match (doc_id, &syn) {
        (Some(id), Some(s)) => {
            view.push(Entry::Item(item("mode_text", lang.t("Modus: Text", "Mode: Text"), String::new(), f(Cmd::SetDocMode(id, Some(Mode::Text)))).check(s.mode == Mode::Text)));
            let large = s.large.then(|| lang.t("Große Datei: Syntaxfarben bleiben aus.", "Large file: syntax colours stay off.").to_string());
            let mut it = item("mode_syntax", lang.t("Modus: Syntax (Farben)", "Mode: Syntax (colours)"), String::new(), f(Cmd::SetDocMode(id, Some(Mode::Syntax)))).check(s.mode == Mode::Syntax);
            it.tip = large;
            view.push(Entry::Item(it));
            view.push(Entry::Languages { doc: id, current: s.lang.clone(), chosen: s.lang_chosen });
        }
        _ => {
            view.push(Entry::Item(item("mode_text", lang.t("Modus: Text", "Mode: Text"), String::new(), vec![]).off(need_doc())));
            view.push(Entry::Item(item("mode_syntax", lang.t("Modus: Syntax (Farben)", "Mode: Syntax (colours)"), String::new(), vec![]).off(need_doc())));
        }
    }
    // Änderungsmarken, nächste/vorige Änderung (#385)
    let marks = super::diff_menu::view_entries(prefs, lang, &key(prefs, lang, "ed_next_change"), &key(prefs, lang, "ed_prev_change"));
    diff_items(&mut view, &VIEW_DIFF_IDS, marks, |_| Some(lang.t("Die Änderungsmarken sind aus.", "Change markers are off.").to_string()), f);
    view.push(Entry::Sep);
    view.push(Entry::Item(item("palette", lang.t("Befehle (Palette) …", "Commands (palette) …"), key(prefs, lang, "palette"), f(Cmd::OpenPalette))));

    // ---- Format
    let why_fmt = need_text()
        .or_else(|| kind.is_none().then(|| lang.t("Weder JSON noch XML (erkannt an Endung, Dateiname oder Inhalt)", "Not JSON or XML (detected by extension, file name or content)").to_string()))
        .or_else(|| running.then(|| lang.t("Das Formatieren läuft – warten oder abbrechen", "Formatting is running – wait or cancel").to_string()));
    let fop = |id: &'static str, o: FmtOp, k: &str| {
        let mut it = item(id, super::format::op_label(o, kind, selection && o != FmtOp::Validate, lang), key(prefs, lang, k), f(Cmd::Format(g, o))).off(why_fmt.clone());
        if it.enabled && kind == Some(Kind::Xml) && o == FmtOp::SortKeys {
            it = it.off(Some(lang.t("Nur für JSON", "Only for JSON").to_string()));
        }
        if it.enabled && selection && o != FmtOp::Validate {
            it.tip = Some(lang.t("Nur die Auswahl; sie muss ein vollständiger Wert bzw. ein vollständiges Element sein.", "Only the selection; it must be a complete value or element.").to_string());
        }
        Entry::Item(it)
    };
    let mut format = vec![fop("format", FmtOp::Format, "ed_format"), fop("minify", FmtOp::Minify, "ed_minify"), fop("validate", FmtOp::Validate, "ed_validate"), fop("sort_keys", FmtOp::SortKeys, "ed_sort_keys"), Entry::Sep];
    // Formatieren nie von allein: nur auf Knopfdruck oder über eine Regel mit der Aktion *Formatieren* (#383). Der Eintrag
    // öffnet den Regel-Dialog für diese Datei (Muster und Aktion vorbelegt).
    let mut fos = item("format_on_save", lang.t("Beim Speichern formatieren …", "Format on save …"), String::new(), f(Cmd::OnSaveMenu(super::super::save_actions_dialog::OnSaveMenu::FormatRule(doc_id)))).off(need_text().or_else(|| kind.is_none().then(|| lang.t("Weder JSON noch XML", "Not JSON or XML").to_string())));
    if fos.enabled {
        fos.tip = Some(lang.t("Legt eine Regel „Aktionen beim Speichern“ mit der Aktion Formatieren für diese Datei an. Ohne Regel wird nie von allein formatiert.", "Creates an “Actions on save” rule with the Format action for this file. Without a rule nothing is ever formatted automatically.").to_string());
    }
    format.push(Entry::Item(fos));

    vec![
        Menu { id: "file", title: lang.t("Datei", "File"), entries: file },
        Menu { id: "edit", title: lang.t("Bearbeiten", "Edit"), entries: edit },
        Menu { id: "view", title: lang.t("Ansicht", "View"), entries: view },
        Menu { id: "format", title: lang.t("Format", "Format"), entries: format },
    ]
}

/// Kennungen der Einträge aus [`super::diff_menu::file_entries`] bzw. [`super::diff_menu::view_entries`] (Tests, Tutorial).
const FILE_DIFF_IDS: [&str; 6] = ["compare_file", "compare_disk", "compare_head", "compare_index", "compare_revision", "compare_clipboard"];
const VIEW_DIFF_IDS: [&str; 5] = ["change_markers", "markers_git", "markers_disk", "next_change", "prev_change"];

/// Einträge des Vergleichs (#385, [`super::diff_menu`]) ins Modell übernehmen: Trennlinien, Häkchen, Kürzel; ausgegraute
/// sagen mit `why(index)` warum.
fn diff_items(out: &mut Vec<Entry>, ids: &[&'static str], entries: Vec<super::diff_menu::MenuEntry>, why: impl Fn(usize) -> Option<String>, f: impl Fn(Cmd) -> Vec<Cmd>) {
    for (i, e) in entries.into_iter().enumerate() {
        if e.separator && !matches!(out.last(), None | Some(Entry::Sep)) {
            out.push(Entry::Sep);
        }
        let mut it = item(ids.get(i).copied().unwrap_or("diff"), e.label, e.shortcut, f(e.cmd));
        it.checked = e.checked;
        if !e.enabled {
            it = it.off(why(i).or_else(|| Some(String::new())));
        }
        out.push(Entry::Item(it));
    }
}

/// Aktionen beim Speichern (#383) im Menü *Datei*: *Aktionen beim Speichern …*, *Ohne Aktionen speichern* (eigenes
/// Kürzel aus den Einstellungen), *Pausieren*/*Fortsetzen*. Ausgeführt von [`crate::app::App::on_save_menu_items`].
fn on_save_entries(doc: Option<u64>, paused: bool, need_doc: Option<String>, prefs: &super::EditorPrefs, lang: Lang, f: impl Fn(Cmd) -> Vec<Cmd>) -> Vec<Entry> {
    use super::super::save_actions_dialog::OnSaveMenu;
    let rules = item("on_save_rules", lang.t("Aktionen beim Speichern …", "Actions on save …"), String::new(), f(Cmd::OnSaveMenu(OnSaveMenu::Rules(doc))));
    let without = match doc {
        Some(d) => item("save_no_actions", lang.t("Ohne Aktionen speichern", "Save without actions"), key(prefs, lang, "save_no_actions"), f(Cmd::OnSaveMenu(OnSaveMenu::SaveWithout(d)))).off(need_doc),
        None => item("save_no_actions", lang.t("Ohne Aktionen speichern", "Save without actions"), key(prefs, lang, "save_no_actions"), vec![]).off(need_doc),
    };
    let pause = if paused { lang.t("Aktionen beim Speichern fortsetzen", "Resume actions on save") } else { lang.t("Aktionen beim Speichern pausieren (bis zum Neustart)", "Pause actions on save (until restart)") };
    vec![Entry::Item(rules), Entry::Item(without), Entry::Item(item("on_save_pause", pause, String::new(), f(Cmd::OnSaveMenu(OnSaveMenu::TogglePause))))]
}

impl EditorUi {
    /// Befehl des Textfelds aus dem Menü *Bearbeiten* auf den aktiven Reiter der Teilung `g` (wie Tastatur bzw.
    /// Kontextmenü); danach hat das Textfeld wieder die Tastatur.
    pub(super) fn menu_edit_op(&mut self, ctx: &egui::Context, g: u64, op: EdOp) {
        let Some(idx) = self.panes.groups.get(&g).map(|grp| grp.active) else { return };
        self.panes.focused = g;
        self.want_focus = true;
        let (Some(view), Some(d)) = self.view_doc_mut(g, idx) else { return };
        let Some(doc) = d.doc.as_mut() else { return };
        view.sync(doc);
        let Some(sels) = view.sels.as_mut() else { return };
        let tab = doc.tab_width().max(1);
        let edited = match op {
            EdOp::Copy | EdOp::Cut => {
                let (text, edited) = super::doc::clip(doc, sels, op == EdOp::Cut);
                if !text.is_empty() {
                    // Unter Windows mit CRLF, damit andere Programme die Zeilen erkennen (wie im Textfeld).
                    ctx.copy_text(if cfg!(windows) { text.replace('\n', "\r\n") } else { text });
                }
                edited
            }
            EdOp::PasteFromClipboard => match crate::platform::clipboard::get_text() {
                Some(t) => super::doc::run(doc, sels, EdOp::Paste(t), tab),
                None => false,
            },
            o => super::doc::run(doc, sels, o, tab),
        };
        view.reveal = true;
        if edited {
            d.on_user_edit();
        }
    }
}

/// Schrift der Menütitel.
fn title_font() -> egui::FontId {
    egui::FontId::proportional(12.5)
}

/// Passen die vier Menütitel in `width`? Sonst ein einziges *Menü ▾*.
pub(super) fn collapsed(ui: &Ui, width: f32, menus: &[Menu]) -> bool {
    let pad = ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x;
    let need: f32 = menus.iter().map(|m| ui.painter().layout_no_wrap(m.title.to_string(), title_font(), egui::Color32::PLACEHOLDER).size().x + pad).sum::<f32>() + 16.0;
    width < need
}

/// Menüleiste der Teilung `g` in `rect` zeichnen. Liefert, ob sie zusammengeklappt ist (Tests).
pub(super) fn menu_bar(ui: &mut Ui, rect: Rect, g: u64, ed: &mut EditorUi, prefs: &super::EditorPrefs, lang: Lang, cmds: &mut Vec<Cmd>) -> bool {
    let u = style::ui();
    ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
    ui.painter().line_segment([rect.left_bottom() - egui::vec2(0.0, 0.5), rect.right_bottom() - egui::vec2(0.0, 0.5)], egui::Stroke::new(1.0_f32, u.border.gamma_multiply(0.7)));
    let menus = menus(ed, g, prefs, lang);
    let narrow = collapsed(ui, rect.width(), &menus);
    let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("ed_menu_bar", g)).max_rect(rect.shrink2(egui::vec2(4.0, 1.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 2.0;
    let mut out: Vec<Cmd> = Vec::new();
    egui::MenuBar::new().ui(ui, |ui| {
        if narrow {
            ui.menu_button(egui::RichText::new(lang.t("Menü ▾", "Menu ▾")).font(title_font()), |ui| {
                for m in &menus {
                    ui.menu_button(m.title, |ui| entries_ui(ui, &m.entries, lang, &mut out));
                }
            });
        } else {
            for m in &menus {
                let _r = ui.menu_button(egui::RichText::new(m.title).font(title_font()), |ui| entries_ui(ui, &m.entries, lang, &mut out)).response;
                #[cfg(test)]
                SEEN.with(|s| s.borrow_mut().push((m.id, _r.rect, true)));
            }
        }
    });
    cmds.extend(out);
    narrow
}

/// Einträge eines Menüs zeichnen.
fn entries_ui(ui: &mut Ui, entries: &[Entry], lang: Lang, out: &mut Vec<Cmd>) {
    ui.set_min_width(240.0);
    for e in entries {
        match e {
            Entry::Sep => {
                ui.separator();
            }
            Entry::Item(it) => {
                let mut b = match it.checked {
                    Some(on) => egui::Button::selectable(on, it.label.as_str()),
                    None => egui::Button::new(it.label.as_str()),
                };
                if !it.shortcut.is_empty() {
                    b = b.shortcut_text(it.shortcut.as_str());
                }
                let r = ui.add_enabled(it.enabled, b);
                #[cfg(test)]
                SEEN.with(|s| s.borrow_mut().push((it.id, r.rect, it.enabled)));
                let r = match (&it.tip, it.enabled) {
                    (Some(t), false) => r.on_disabled_hover_text(t.as_str()),
                    (Some(t), true) => r.on_hover_text(t.as_str()),
                    (None, _) => r,
                };
                if r.clicked() {
                    out.extend(it.cmds.iter().cloned());
                    ui.close();
                }
            }
            Entry::Languages { doc, current, chosen } => {
                ui.menu_button(lang.t("Sprache", "Language"), |ui| {
                    ui.set_min_width(220.0);
                    if ui.selectable_label(!*chosen, lang.t("Automatisch erkennen", "Detect automatically")).clicked() {
                        out.push(Cmd::SetLanguage(*doc, None));
                        ui.close();
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for l in crate::editor::syntax::languages::LANGUAGES {
                            let sel = current.as_deref() == Some(l.id);
                            if ui.selectable_label(sel, super::view::lang_label(l.id, l.name, lang)).clicked() {
                                out.push(Cmd::SetLanguage(*doc, Some(l.id.to_string())));
                                ui.close();
                            }
                        }
                    });
                    ui.label(egui::RichText::new(lang.t("Alle Sprachen: Statusleiste", "All languages: status bar")).small().color(style::dim(ui)));
                });
            }
        }
    }
}
