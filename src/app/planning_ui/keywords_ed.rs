//! Editor für JQL-Schlüsselwörter: benannte Prädikate (`IS blocked`, `IS NOT favorite`) und Sortierschlüssel (`ORDER BY favorites`).
//!
//! Eingebaute Schlüsselwörter (`blocked`, `favorite`, `favorites`, `flagged`) tragen ein Schloss: nur die Beschreibung ist änderbar,
//! löschen geht nicht. Eigene bestehen aus einer Bedingung (Feld, Vergleich, Wert; UND/ODER-Gruppen bis zur erlaubten Tiefe) bzw. einem
//! Sortierfeld mit Richtung. Verschwindet ein Feld, ist das Schlüsselwort „kaputt“ (rot markiert, dem Validator nicht mehr angeboten),
//! bis es repariert oder gelöscht wird. Die Datenschicht ist `crate::planning::views::keywords`; hier ist nur die Oberfläche.
//!
//! Der Dialog liegt im Zustand [`State`]; wer ihn öffnen will (Abfrage-Leiste, Reiter „Felder“), ruft [`State::open`] und zeichnet
//! [`keywords_dialog`]. Gibt der Dialog `true` zurück, haben sich die Schlüsselwörter geändert (Schemas der Leisten neu bauen).

use super::helpers::{op_label, op_needs_value, ops_for};
use super::values;
use super::widgets::{self, hint};
use crate::planning::fields::{FieldKind, FieldOptions, list_fields};
use crate::planning::projects::list_projects;
use crate::planning::types::list_types;
use crate::planning::views::keywords::{
    BuiltinId, Cond, Definition, Keyword, KeywordKind, MAX_COND_DEPTH, MAX_COND_NODES, MAX_KEYWORD_CHARS, NewKeyword, SortDir, UpdateKeyword, broken_keywords, create_keyword, delete_keyword, list_keywords,
    update_keyword, validate_name,
};
use crate::planning::workflow::ConditionOp;
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2, S3};
use eframe::egui::{self, Color32, Id, RichText, Ui};
use rusqlite::Connection;
use serde_json::Value;

#[allow(non_snake_case)]
fn RED() -> Color32 {
    crate::style::ui().error
}

/// Ein wählbares Feld (aus allen Projekten, weil Schlüsselwörter global sind).
#[derive(Clone, Debug)]
pub struct FieldChoice {
    pub id: i64,
    /// „PAN · Epic · Priority“
    pub label: String,
    pub kind: FieldKind,
    pub options: FieldOptions,
}

/// Alle Felder aller Projekte (ohne das System-Projekt), mit „Projekt · Typ · Feld“ als Bezeichnung.
pub fn all_field_choices(conn: &Connection, lang: Lang) -> Vec<FieldChoice> {
    let mut out = Vec::new();
    for p in list_projects(conn).unwrap_or_default().into_iter().filter(|p| !p.system) {
        for t in list_types(conn, p.id).unwrap_or_default() {
            for f in list_fields(conn, t.id).unwrap_or_default() {
                out.push(FieldChoice { id: f.id, label: format!("{} · {} · {}", p.key, t.display(lang), f.display(lang)), kind: f.kind, options: f.options.clone() });
            }
        }
    }
    out
}

fn find(fields: &[FieldChoice], id: i64) -> Option<&FieldChoice> {
    fields.iter().find(|f| f.id == id)
}

/// Kann nach diesem Feld sortiert werden? (nicht nach Mehrfachwerten und Rich-Text)
pub fn sortable(k: FieldKind) -> bool {
    !matches!(k, FieldKind::Multi | FieldKind::Labels | FieldKind::Rich)
}

// ------------------------------------------------------------------ Reine Hilfen

/// Einträge und Tiefe einer Bedingung: (Knoten, tiefste Ebene; die Wurzel liegt auf Ebene 1).
pub fn cond_stats(c: &Cond) -> (usize, usize) {
    match c {
        Cond::Compare { .. } => (1, 1),
        Cond::And { items } | Cond::Or { items } => {
            let mut nodes = 1;
            let mut depth = 1;
            for i in items {
                let (n, d) = cond_stats(i);
                nodes += n;
                depth = depth.max(d + 1);
            }
            (nodes, depth)
        }
    }
}

/// Gruppen mit nur einem Eintrag werden zu diesem Eintrag (die Datenschicht erlaubt beides, so bleibt es kurz).
pub fn normalize(c: Cond) -> Cond {
    match c {
        Cond::And { items } | Cond::Or { items } if items.len() == 1 => normalize(items.into_iter().next().unwrap_or(Cond::And { items: Vec::new() })),
        Cond::And { items } => Cond::And { items: items.into_iter().map(normalize).collect() },
        Cond::Or { items } => Cond::Or { items: items.into_iter().map(normalize).collect() },
        c => c,
    }
}

/// Wurzel für den Editor: immer eine UND-Gruppe (ein einzelner Vergleich wird eingepackt).
pub fn as_root(c: Cond) -> Cond {
    match c {
        Cond::Compare { .. } => Cond::And { items: vec![c] },
        c => c,
    }
}

/// Ein neuer Vergleich für das erste passende Feld (oder für `field_id`).
pub fn new_compare(fields: &[FieldChoice], prefer: Option<i64>) -> Option<Cond> {
    let f = prefer.and_then(|id| find(fields, id)).or_else(|| fields.first())?;
    Some(compare_for(f))
}

fn compare_for(f: &FieldChoice) -> Cond {
    let op = ops_for(f.kind)[0];
    Cond::Compare { field_id: f.id, op, value: if op_needs_value(op) { values::blank(f.kind, &f.options) } else { Value::Null } }
}

/// Lesbare Zusammenfassung einer Bedingung („Priority is High AND Done is Yes“).
pub fn describe_cond(c: &Cond, fields: &[FieldChoice], lang: Lang) -> String {
    match c {
        Cond::Compare { field_id, op, value } => {
            let Some(f) = find(fields, *field_id) else { return lang.t("(Feld fehlt)", "(field missing)").to_string() };
            let name = f.label.rsplit(" · ").next().unwrap_or(&f.label);
            if op_needs_value(*op) {
                format!("{name} {} {}", op_label(*op, lang), values::show_value(f.kind, &f.options, value, lang))
            } else {
                format!("{name} {}", op_label(*op, lang))
            }
        }
        Cond::And { items } | Cond::Or { items } => {
            let join = if matches!(c, Cond::And { .. }) { lang.t(" UND ", " AND ") } else { lang.t(" ODER ", " OR ") };
            let parts: Vec<String> = items
                .iter()
                .map(|i| {
                    let s = describe_cond(i, fields, lang);
                    if matches!(i, Cond::Compare { .. }) { s } else { format!("({s})") }
                })
                .collect();
            parts.join(join)
        }
    }
}

/// Zusammenfassung der Definition und wie man das Schlüsselwort benutzt.
pub fn describe(k: &Keyword, fields: &[FieldChoice], lang: Lang) -> String {
    match &k.definition {
        Definition::Builtin { id } => match id {
            BuiltinId::Blocked | BuiltinId::Favorite => format!("IS {0} · IS NOT {0}", k.name),
            BuiltinId::Favorites | BuiltinId::Flagged => format!("ORDER BY {}", k.name),
        },
        Definition::Predicate { condition } => format!("IS {0} · IS NOT {0}  ⇔  {1}", k.name, describe_cond(condition, fields, lang)),
        Definition::Sort { field_id, dir } => {
            let f = find(fields, *field_id).map(|f| f.label.clone()).unwrap_or_else(|| lang.t("(Feld fehlt)", "(field missing)").to_string());
            let d = match dir {
                SortDir::Asc => lang.t("aufsteigend", "ascending"),
                SortDir::Desc => lang.t("absteigend", "descending"),
            };
            format!("ORDER BY {}  ⇔  {f} ({d})", k.name)
        }
    }
}

// ------------------------------------------------------------------ Zustand

/// Formular für ein neues oder bestehendes Schlüsselwort.
pub struct Form {
    id: Option<i64>,
    builtin: bool,
    name: String,
    description: String,
    kind: KeywordKind,
    cond: Cond,
    sort_field: i64,
    sort_dir: SortDir,
    err: Option<String>,
}

impl Form {
    fn new(fields: &[FieldChoice]) -> Form {
        let cond = new_compare(fields, None).map(|c| Cond::And { items: vec![c] }).unwrap_or(Cond::And { items: Vec::new() });
        let sort_field = fields.iter().find(|f| sortable(f.kind)).map(|f| f.id).unwrap_or(0);
        Form { id: None, builtin: false, name: String::new(), description: String::new(), kind: KeywordKind::Predicate, cond, sort_field, sort_dir: SortDir::Asc, err: None }
    }

    fn from_keyword(k: &Keyword, fields: &[FieldChoice]) -> Form {
        let mut f = Form::new(fields);
        f.id = Some(k.id);
        f.builtin = k.builtin;
        f.name = k.name.clone();
        f.description = k.description.clone().unwrap_or_default();
        f.kind = k.kind;
        match &k.definition {
            Definition::Predicate { condition } => f.cond = as_root(condition.clone()),
            Definition::Sort { field_id, dir } => {
                f.sort_field = *field_id;
                f.sort_dir = *dir;
            }
            Definition::Builtin { .. } => {}
        }
        f
    }

    /// Die Definition, wie sie gespeichert wird.
    fn definition(&self) -> Definition {
        match self.kind {
            KeywordKind::Predicate => Definition::Predicate { condition: normalize(self.cond.clone()) },
            KeywordKind::Sort => Definition::Sort { field_id: self.sort_field, dir: self.sort_dir },
        }
    }
}

enum Mode {
    List,
    Edit(Box<Form>),
}

/// Zustand des Schlüsselwort-Dialogs.
pub struct State {
    pub open: bool,
    mode: Mode,
    list: Vec<Keyword>,
    broken: Vec<i64>,
    fields: Vec<FieldChoice>,
    loaded: bool,
    err: Option<String>,
    delete: Option<i64>,
}

impl Default for State {
    fn default() -> Self {
        State { open: false, mode: Mode::List, list: Vec::new(), broken: Vec::new(), fields: Vec::new(), loaded: false, err: None, delete: None }
    }
}

impl State {
    /// Dialog öffnen (Liste neu lesen).
    pub fn open(&mut self) {
        self.open = true;
        self.mode = Mode::List;
        self.loaded = false;
        self.err = None;
        self.delete = None;
    }

    fn reload(&mut self, conn: &Connection, lang: Lang) {
        self.list = list_keywords(conn).unwrap_or_default();
        self.broken = broken_keywords(conn).unwrap_or_default();
        self.fields = all_field_choices(conn, lang);
        self.loaded = true;
    }
}

// ------------------------------------------------------------------ Dialog

/// Den Dialog zeichnen. `true`, wenn sich Schlüsselwörter geändert haben.
pub fn keywords_dialog(ctx: &egui::Context, st: &mut State, conn: &Connection, lang: Lang) -> bool {
    if !st.open {
        return false;
    }
    if !st.loaded {
        st.reload(conn, lang);
    }
    let mut changed = false;
    let mut close = false;
    let mut start_edit: Option<Option<i64>> = None;
    let mut ask_delete: Option<i64> = None;
    let mut save: Option<()> = None;
    let mut back = false;
    let m = crate::style::modal(Id::new("keywords_dialog")).show(ctx, |ui| {
        ui.set_width(widgets::modal_width(ctx, 760.0));
        match &mut st.mode {
            Mode::List => {
                widgets::dialog_title(ui, lang.t("JQL-Schlüsselwörter", "JQL keywords"));
                hint(ui, lang.t(
                    "Schlüsselwörter machen Abfragen kürzer: ein Prädikat (IS blocked, IS NOT favorite) prüft eine Bedingung, ein Sortierschlüssel (ORDER BY favorites) sortiert. Eingebaute (Schloss) lassen sich nicht löschen; bei ihnen ist nur die Beschreibung änderbar.",
                    "Keywords make queries shorter: a predicate (IS blocked, IS NOT favorite) checks a condition, a sort key (ORDER BY favorites) sorts. Built-in ones (lock) cannot be deleted; only their description can be changed.",
                ));
                ui.add_space(S2);
                if widgets::primary(ui, &format!("+ {}", lang.t("Neues Schlüsselwort", "New keyword"))).clicked() {
                    start_edit = Some(None);
                }
                ui.add_space(S2);
                if let Some(e) = &st.err {
                    widgets::error(ui, e);
                    ui.add_space(S1);
                }
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 320.0).clamp(140.0, 460.0)).auto_shrink([false, true]).show(ui, |ui| {
                    for k in &st.list {
                        let is_broken = st.broken.contains(&k.id);
                        widgets::card(ui, |ui| {
                            ui.horizontal(|ui| {
                                if k.builtin {
                                    widgets::lock_icon(ui, lang.t("Eingebaut: nicht löschbar, nur die Beschreibung ist änderbar", "Built in: cannot be deleted, only the description can be changed"));
                                } else if is_broken {
                                    ui.label(RichText::new("▲").color(RED())).on_hover_text(lang.t("Kaputt: ein Feld der Definition gibt es nicht mehr", "Broken: a field of the definition no longer exists"));
                                }
                                ui.label(RichText::new(&k.name).monospace().strong());
                                let (txt, tip) = match k.kind {
                                    KeywordKind::Predicate => (lang.t("Prädikat", "Predicate"), lang.t("Benutzung: IS name / IS NOT name", "Use as: IS name / IS NOT name")),
                                    KeywordKind::Sort => (lang.t("Sortierung", "Sort key"), lang.t("Benutzung: ORDER BY name", "Use as: ORDER BY name")),
                                };
                                ui.label(RichText::new(txt).small().color(style::accent())).on_hover_text(tip);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if !k.builtin && style::icon_btn_small(ui, SIcon::Trash, lang.t("Schlüsselwort löschen …", "Delete keyword …")).clicked() {
                                        ask_delete = Some(k.id);
                                    }
                                    if ui.button(lang.t("Bearbeiten", "Edit")).clicked() {
                                        start_edit = Some(Some(k.id));
                                    }
                                });
                            });
                            let d = k.description_text(lang);
                            if !d.is_empty() {
                                ui.label(RichText::new(d).color(style::dim(ui)));
                            }
                            ui.add(egui::Label::new(RichText::new(describe(k, &st.fields, lang)).monospace().size(11.5).color(style::dim(ui))).wrap());
                            if is_broken {
                                ui.label(RichText::new(lang.t(
                                    "Kaputt: Ein Feld dieser Definition gibt es nicht mehr. Das Schlüsselwort wird in Abfragen nicht angeboten – repariere es oder lösche es.",
                                    "Broken: a field of this definition no longer exists. The keyword is not offered in queries – repair or delete it.",
                                )).small().color(RED()));
                            }
                        });
                        ui.add_space(S1);
                    }
                });
                ui.add_space(S3);
                if widgets::secondary(ui, lang.t("Schließen", "Close")).clicked() {
                    close = true;
                }
            }
            Mode::Edit(f) => {
                form_ui(ui, f, &st.fields, lang, &mut save, &mut back);
            }
        }
    });
    if m.should_close() && st.delete.is_none() {
        match st.mode {
            Mode::List => close = true,
            Mode::Edit(_) => back = true,
        }
    }
    if let Some(which) = start_edit {
        st.err = None;
        st.mode = Mode::Edit(Box::new(match which.and_then(|id| st.list.iter().find(|k| k.id == id)) {
            Some(k) => Form::from_keyword(k, &st.fields),
            None => Form::new(&st.fields),
        }));
    }
    if let Some(id) = ask_delete {
        st.delete = Some(id);
    }
    if save.is_some() {
        if let Mode::Edit(f) = &mut st.mode {
            match save_form(conn, f) {
                Ok(()) => {
                    changed = true;
                    back = true;
                }
                Err(e) => f.err = Some(e.message(lang)),
            }
        }
    }
    if back {
        st.mode = Mode::List;
        st.reload(conn, lang);
    }
    if let Some(id) = st.delete {
        match st.list.iter().find(|k| k.id == id).map(|k| k.name.clone()) {
            Some(name) => {
                let (mut yes, mut no) = (false, false);
                let m = crate::style::modal(Id::new("keywords_delete")).show(ctx, |ui| {
                    ui.set_width(widgets::modal_width(ctx, 420.0));
                    widgets::dialog_title(ui, lang.t("Schlüsselwort löschen?", "Delete keyword?"));
                    ui.label(match lang {
                        Lang::De => format!("Das Schlüsselwort {} wird endgültig gelöscht.", lang.q(&name)),
                        Lang::En => format!("The keyword {} will be deleted for good.", lang.q(&name)),
                    });
                    ui.add_space(S1);
                    hint(ui, lang.t(
                        "Gespeicherte Filter, die es benutzen, melden danach ein unbekanntes Schlüsselwort.",
                        "Saved filters that use it will report an unknown keyword afterwards.",
                    ));
                    ui.add_space(S3);
                    ui.horizontal(|ui| {
                        if widgets::danger(ui, lang.t("Löschen", "Delete"), 0).clicked() {
                            yes = true;
                        }
                        if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                            no = true;
                        }
                    });
                });
                if m.should_close() {
                    no = true;
                }
                if yes {
                    match delete_keyword(conn, id) {
                        Ok(()) => {
                            changed = true;
                            st.err = None;
                        }
                        Err(e) => st.err = Some(e.message(lang)),
                    }
                    st.delete = None;
                    st.reload(conn, lang);
                } else if no {
                    st.delete = None;
                }
            }
            None => st.delete = None,
        }
    }
    if close {
        st.open = false;
    }
    changed
}

fn save_form(conn: &Connection, f: &Form) -> Result<(), crate::planning::views::ViewError> {
    match (f.id, f.builtin) {
        (Some(id), true) => update_keyword(conn, id, &UpdateKeyword { name: None, description: Some(f.description.clone()), definition: None }),
        (Some(id), false) => update_keyword(conn, id, &UpdateKeyword { name: Some(f.name.clone()), description: Some(f.description.clone()), definition: Some(f.definition()) }),
        (None, _) => create_keyword(conn, &NewKeyword { name: f.name.clone(), description: f.description.clone(), definition: f.definition() }).map(|_| ()),
    }
}

// ------------------------------------------------------------------ Formular

fn form_ui(ui: &mut Ui, f: &mut Form, fields: &[FieldChoice], lang: Lang, save: &mut Option<()>, back: &mut bool) {
    let title = match (f.id, f.builtin) {
        (None, _) => lang.t("Neues Schlüsselwort", "New keyword"),
        (Some(_), true) => lang.t("Eingebautes Schlüsselwort", "Built-in keyword"),
        (Some(_), false) => lang.t("Schlüsselwort bearbeiten", "Edit keyword"),
    };
    widgets::dialog_title(ui, title);
    egui::ScrollArea::vertical().max_height(520.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.label(RichText::new(lang.t("Name", "Name")).color(style::dim(ui)));
        ui.add_enabled(!f.builtin, egui::TextEdit::singleline(&mut f.name).desired_width(280.0).char_limit(MAX_KEYWORD_CHARS).margin(egui::Margin::symmetric(8, 5)));
        if !f.builtin {
            match validate_name(&f.name) {
                Err(e) if !f.name.trim().is_empty() => widgets::error(ui, &e.message(lang)),
                _ => hint(ui, lang.t("Buchstaben, Ziffern und _, nicht mit einer Ziffer beginnen, keine reservierten Wörter wie AND oder ORDER.", "Letters, digits and _, must not start with a digit, no reserved words like AND or ORDER.")),
            }
        }
        ui.add_space(S2);
        ui.label(RichText::new(lang.t("Beschreibung (optional)", "Description (optional)")).color(style::dim(ui)));
        ui.add(egui::TextEdit::singleline(&mut f.description).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
        if f.builtin {
            hint(ui, lang.t("Leer lassen für den mitgelieferten, übersetzten Text.", "Leave empty for the supplied, translated text."));
        } else {
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Art", "Kind")).color(style::dim(ui)));
            ui.horizontal(|ui| {
                widgets::radio(ui, &mut f.kind, KeywordKind::Predicate, lang.t("Prädikat (IS name)", "Predicate (IS name)"));
                widgets::radio(ui, &mut f.kind, KeywordKind::Sort, lang.t("Sortierung (ORDER BY name)", "Sort key (ORDER BY name)"));
            });
            ui.add_space(S2);
            match f.kind {
                KeywordKind::Predicate => {
                    ui.label(RichText::new(lang.t("Bedingung", "Condition")).color(style::dim(ui)));
                    hint(ui, lang.t("Ein Ticket erfüllt das Prädikat, wenn die Bedingung zutrifft.", "A ticket satisfies the predicate when the condition matches."));
                    ui.add_space(S1);
                    let (nodes, _) = cond_stats(&f.cond);
                    if fields.is_empty() {
                        hint(ui, lang.t("Es gibt noch keine Felder – lege zuerst Typen und Felder in einem Projekt an.", "There are no fields yet – create types and fields in a project first."));
                    } else {
                        let mut n = nodes;
                        group_editor(ui, &mut f.cond, fields, lang, 1, &mut n, Id::new("kw_cond"), true);
                    }
                }
                KeywordKind::Sort => {
                    ui.label(RichText::new(lang.t("Sortierfeld", "Sort field")).color(style::dim(ui)));
                    let sortable_fields: Vec<&FieldChoice> = fields.iter().filter(|c| sortable(c.kind)).collect();
                    let sel = find(fields, f.sort_field).map(|c| c.label.clone()).unwrap_or_else(|| lang.t("(Feld wählen)", "(choose a field)").to_string());
                    egui::ComboBox::from_id_salt("kw_sort_field").selected_text(sel).width(320.0).show_ui(ui, |ui| {
                        for c in &sortable_fields {
                            ui.selectable_value(&mut f.sort_field, c.id, &c.label);
                        }
                    });
                    if find(fields, f.sort_field).is_none() {
                        widgets::error(ui, lang.t("Das Feld gibt es nicht (mehr) – wähle ein anderes.", "The field does not exist (any more) – choose another one."));
                    }
                    ui.horizontal(|ui| {
                        widgets::radio(ui, &mut f.sort_dir, SortDir::Asc, lang.t("Aufsteigend", "Ascending"));
                        widgets::radio(ui, &mut f.sort_dir, SortDir::Desc, lang.t("Absteigend", "Descending"));
                    });
                }
            }
        }
        if let Some(e) = &f.err {
            ui.add_space(S2);
            widgets::error(ui, e);
        }
    });
    ui.add_space(S3);
    ui.horizontal(|ui| {
        let ok = f.builtin || validate_name(&f.name).is_ok();
        if widgets::primary_enabled(ui, ok, lang.t("Speichern", "Save")).clicked() {
            *save = Some(());
        }
        if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
            *back = true;
        }
    });
}

/// Die Bedingungs-Gruppe (UND/ODER) mit ihren Einträgen. `depth`: Ebene dieser Gruppe (die Wurzel ist 1). Gibt `true` zurück,
/// wenn die Gruppe entfernt werden soll (nicht bei der Wurzel).
fn group_editor(ui: &mut Ui, c: &mut Cond, fields: &[FieldChoice], lang: Lang, depth: usize, nodes: &mut usize, salt: Id, root: bool) -> bool {
    let mut remove_self = false;
    let is_and = matches!(c, Cond::And { .. });
    let mut want_and = is_and;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(salt.with("mode")).selected_text(if is_and { lang.t("Alle müssen zutreffen (UND)", "All must match (AND)") } else { lang.t("Eine muss zutreffen (ODER)", "Any must match (OR)") }).width(230.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut want_and, true, lang.t("Alle müssen zutreffen (UND)", "All must match (AND)"));
            ui.selectable_value(&mut want_and, false, lang.t("Eine muss zutreffen (ODER)", "Any must match (OR)"));
        });
        if !root && style::icon_btn_small(ui, SIcon::Trash, lang.t("Gruppe entfernen", "Remove group")).clicked() {
            remove_self = true;
        }
    });
    if want_and != is_and {
        let items = match std::mem::replace(c, Cond::And { items: Vec::new() }) {
            Cond::And { items } | Cond::Or { items } => items,
            other => vec![other],
        };
        *c = if want_and { Cond::And { items } } else { Cond::Or { items } };
    }
    let (Cond::And { items } | Cond::Or { items }) = c else { return remove_self };
    let mut remove: Option<usize> = None;
    let line_x = ui.cursor().min.x + 6.0;
    let top = ui.cursor().min.y;
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.vertical(|ui| {
            for (i, item) in items.iter_mut().enumerate() {
                let s = salt.with(i);
                match item {
                    Cond::Compare { .. } => {
                        if compare_editor(ui, item, fields, lang, s) {
                            remove = Some(i);
                        }
                    }
                    _ => {
                        if group_editor(ui, item, fields, lang, depth + 1, nodes, s, false) {
                            remove = Some(i);
                        }
                    }
                }
                ui.add_space(2.0);
            }
            if items.is_empty() {
                widgets::error(ui, lang.t("Eine Gruppe braucht mindestens einen Eintrag.", "A group needs at least one entry."));
            }
            ui.horizontal(|ui| {
                let room_one = *nodes < MAX_COND_NODES;
                let room_two = *nodes + 2 <= MAX_COND_NODES;
                if ui.add_enabled(depth < MAX_COND_DEPTH && room_one, egui::Button::new(format!("+ {}", lang.t("Bedingung", "Condition")))).clicked() {
                    let prefer = items.iter().rev().find_map(|i| if let Cond::Compare { field_id, .. } = i { Some(*field_id) } else { None });
                    if let Some(n) = new_compare(fields, prefer) {
                        items.push(n);
                        *nodes += 1;
                    }
                }
                if ui.add_enabled(depth + 1 < MAX_COND_DEPTH && room_two, egui::Button::new(format!("+ {}", lang.t("Gruppe", "Group")))).on_hover_text(lang.t("Eine verschachtelte UND-/ODER-Gruppe", "A nested AND/OR group")).clicked() {
                    if let Some(n) = new_compare(fields, None) {
                        items.push(Cond::Or { items: vec![n] });
                        *nodes += 2;
                    }
                }
            });
        });
    });
    let bottom = ui.min_rect().bottom();
    ui.painter().vline(line_x, top..=bottom - 2.0, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    if let Some(i) = remove {
        items.remove(i);
    }
    remove_self
}

/// Eine Zeile „Feld – Vergleich – Wert“. `true`, wenn sie entfernt werden soll.
fn compare_editor(ui: &mut Ui, c: &mut Cond, fields: &[FieldChoice], lang: Lang, salt: Id) -> bool {
    let Cond::Compare { field_id, op, value } = c else { return false };
    let mut remove = false;
    ui.horizontal_wrapped(|ui| {
        let before = *field_id;
        let cur = find(fields, *field_id);
        let sel = cur.map(|f| f.label.clone()).unwrap_or_else(|| lang.t("(Feld fehlt – neu wählen)", "(field missing – choose again)").to_string());
        let text = if cur.is_some() { RichText::new(sel) } else { RichText::new(sel).color(RED()) };
        egui::ComboBox::from_id_salt(salt.with("f")).selected_text(text).width(250.0).show_ui(ui, |ui| {
            for f in fields {
                ui.selectable_value(field_id, f.id, &f.label);
            }
        });
        let Some(fc) = find(fields, *field_id) else {
            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Eintrag entfernen", "Remove entry")).clicked() {
                remove = true;
            }
            return;
        };
        if *field_id != before {
            *op = ops_for(fc.kind)[0];
            *value = if op_needs_value(*op) { values::blank(fc.kind, &fc.options) } else { Value::Null };
        }
        let ops = ops_for(fc.kind);
        if !ops.contains(op) {
            *op = ops[0];
        }
        let before_op = *op;
        egui::ComboBox::from_id_salt(salt.with("o")).selected_text(op_label(*op, lang)).width(130.0).show_ui(ui, |ui| {
            for o in &ops {
                ui.selectable_value(op, *o, op_label(*o, lang));
            }
        });
        let list_op = matches!(*op, ConditionOp::In | ConditionOp::NotIn);
        if *op != before_op {
            let was_list = matches!(before_op, ConditionOp::In | ConditionOp::NotIn);
            if list_op && !was_list {
                *value = serde_json::json!([]);
            } else if !list_op && (was_list || !op_needs_value(before_op)) {
                *value = values::blank(fc.kind, &fc.options);
            }
        }
        if op_needs_value(*op) {
            if list_op {
                if !value.is_array() {
                    *value = serde_json::json!([]);
                }
                values::list_editor(ui, salt.with("v"), fc.kind, &fc.options, value, lang);
            } else {
                if value.is_null() || value.is_array() {
                    *value = values::blank(fc.kind, &fc.options);
                }
                values::element_editor(ui, salt.with("v"), fc.kind, &fc.options, value, lang);
            }
        } else {
            *value = Value::Null;
        }
        if style::icon_btn_small(ui, SIcon::Trash, lang.t("Eintrag entfernen", "Remove entry")).clicked() {
            remove = true;
        }
    });
    remove
}

#[cfg(test)]
mod tests {
    use super::super::helpers::kind_label;
    use super::*;
    use crate::planning::projects::{NewProject, create_project};
    use crate::planning::views::testutil::{db, project};
    use serde_json::json;

    fn setup() -> (Connection, i64, Vec<FieldChoice>) {
        let c = db();
        let p = project(&c, "AA", true);
        let f = all_field_choices(&c, Lang::En);
        (c, p, f)
    }

    fn field(f: &[FieldChoice], name: &str) -> FieldChoice {
        f.iter().find(|x| x.label.ends_with(&format!("· {name}"))).unwrap_or_else(|| panic!("kein Feld {name}: {:?}", f.iter().map(|x| &x.label).collect::<Vec<_>>())).clone()
    }

    #[test]
    fn field_choices_span_projects_and_skip_the_system_project() {
        let (c, _, f) = setup();
        assert!(f.iter().all(|x| x.label.starts_with("AA · ")), "{:?}", f.iter().map(|x| &x.label).collect::<Vec<_>>());
        assert!(f.len() > 3);
        let _q = create_project(&c, &NewProject::new("BB", "B", "#112233")).unwrap();
        assert_eq!(all_field_choices(&c, Lang::En).len(), f.len(), "ein leeres Projekt ändert nichts");
        let q = project(&c, "CC", true);
        assert!(q > 0);
        let all = all_field_choices(&c, Lang::De);
        assert!(all.iter().any(|x| x.label.starts_with("CC · ")));
        assert!(all.len() > f.len());
    }

    #[test]
    fn stats_count_nodes_and_depth() {
        let leaf = |id| Cond::Compare { field_id: id, op: ConditionOp::IsEmpty, value: Value::Null };
        assert_eq!(cond_stats(&leaf(1)), (1, 1));
        let c = Cond::And { items: vec![leaf(1), Cond::Or { items: vec![leaf(2), leaf(3)] }] };
        assert_eq!(cond_stats(&c), (5, 3));
    }

    #[test]
    fn normalize_unwraps_single_item_groups() {
        let leaf = Cond::Compare { field_id: 1, op: ConditionOp::IsEmpty, value: Value::Null };
        assert_eq!(normalize(Cond::And { items: vec![leaf.clone()] }), leaf);
        assert_eq!(normalize(Cond::Or { items: vec![Cond::And { items: vec![leaf.clone()] }] }), leaf);
        let two = Cond::And { items: vec![leaf.clone(), Cond::Or { items: vec![leaf.clone()] }] };
        assert_eq!(normalize(two), Cond::And { items: vec![leaf.clone(), leaf.clone()] });
        assert_eq!(as_root(leaf.clone()), Cond::And { items: vec![leaf.clone()] });
        assert_eq!(as_root(Cond::Or { items: vec![] }), Cond::Or { items: vec![] });
    }

    #[test]
    fn new_compare_is_valid_for_every_field() {
        let (c, _, f) = setup();
        for fc in f.iter().filter(|x| x.kind != FieldKind::Labels) {
            let cond = new_compare(&f, Some(fc.id)).unwrap();
            let r = create_keyword(&c, &NewKeyword { name: format!("kw{}", fc.id), description: String::new(), definition: Definition::Predicate { condition: cond.clone() } });
            // Verweis-Felder ohne Wert (nur „ist leer“) und alle anderen mit passendem Anfangswert müssen durchgehen
            assert!(r.is_ok(), "{}: {:?} -> {:?}", fc.label, cond, r);
        }
        assert!(new_compare(&[], None).is_none());
    }

    #[test]
    fn form_roundtrip_creates_edits_and_deletes_a_keyword() {
        let (c, _, f) = setup();
        let prio = field(&f, "Priority");
        let mut form = Form::new(&f);
        form.name = "urgent".into();
        form.description = "High priority".into();
        form.cond = Cond::And { items: vec![new_compare(&f, Some(prio.id)).unwrap()] };
        save_form(&c, &form).unwrap();
        let k = crate::planning::views::keywords::keyword_by_name(&c, "urgent").unwrap();
        assert_eq!(k.kind, KeywordKind::Predicate);
        assert!(matches!(&k.definition, Definition::Predicate { condition: Cond::Compare { .. } }), "Gruppe mit einem Eintrag wird zum Vergleich: {:?}", k.definition);
        // wieder ins Formular laden: Wurzel ist eine Gruppe
        let back = Form::from_keyword(&k, &f);
        assert!(matches!(back.cond, Cond::And { .. }));
        assert_eq!(back.description, "High priority");
        // ändern: Art Sortierung
        let mut edit = Form::from_keyword(&k, &f);
        edit.kind = KeywordKind::Sort;
        edit.sort_field = prio.id;
        edit.sort_dir = SortDir::Desc;
        save_form(&c, &edit).unwrap();
        let k2 = crate::planning::views::keywords::get_keyword(&c, k.id).unwrap();
        assert_eq!(k2.definition, Definition::Sort { field_id: prio.id, dir: SortDir::Desc });
        assert!(describe(&k2, &f, Lang::En).contains("descending"));
        assert!(describe(&k2, &f, Lang::De).contains("absteigend"));
        // Name vergeben / ungültig / reserviert → Fehlermeldung beider Sprachen
        let mut dup = Form::new(&f);
        dup.name = "URGENT".into();
        let e = save_form(&c, &dup).unwrap_err();
        assert!(!e.message(Lang::En).is_empty() && e.message(Lang::De) != e.message(Lang::En));
        dup.name = "AND".into();
        assert!(save_form(&c, &dup).is_err());
        dup.name = "1x".into();
        assert!(save_form(&c, &dup).is_err());
        delete_keyword(&c, k.id).unwrap();
    }

    #[test]
    fn builtin_form_changes_only_the_description() {
        let (c, _, f) = setup();
        let blocked = crate::planning::views::keywords::keyword_by_name(&c, "blocked").unwrap();
        let mut form = Form::from_keyword(&blocked, &f);
        assert!(form.builtin);
        form.name = "hacked".into();
        form.description = "Mein Text".into();
        save_form(&c, &form).unwrap();
        let again = crate::planning::views::keywords::keyword_by_name(&c, "blocked").unwrap();
        assert_eq!(again.description.as_deref(), Some("Mein Text"));
        assert!(crate::planning::views::keywords::keyword_by_name(&c, "hacked").is_err());
        // leer = mitgelieferter Text
        form.description.clear();
        save_form(&c, &form).unwrap();
        assert_eq!(crate::planning::views::keywords::keyword_by_name(&c, "blocked").unwrap().description, None);
        assert_eq!(describe(&again, &f, Lang::En), "IS blocked · IS NOT blocked");
        let favs = crate::planning::views::keywords::keyword_by_name(&c, "favorites").unwrap();
        assert_eq!(describe(&favs, &f, Lang::En), "ORDER BY favorites");
    }

    #[test]
    fn broken_keywords_are_found_and_described() {
        let (c, p, f) = setup();
        let prio = field(&f, "Priority");
        let k = create_keyword(&c, &NewKeyword { name: "hot".into(), description: String::new(), definition: Definition::Predicate { condition: Cond::Compare { field_id: prio.id, op: ConditionOp::IsNotEmpty, value: Value::Null } } }).unwrap();
        assert!(broken_keywords(&c).unwrap().is_empty());
        crate::planning::projects::delete_project(&c, p).unwrap();
        assert_eq!(broken_keywords(&c).unwrap(), vec![k]);
        let kw = crate::planning::views::keywords::get_keyword(&c, k).unwrap();
        let fields = all_field_choices(&c, Lang::En);
        assert!(describe(&kw, &fields, Lang::En).contains("field missing"));
        assert!(describe(&kw, &fields, Lang::De).contains("Feld fehlt"));
        // Reparatur im Formular: Feld fehlt → neues wählen
        let q = project(&c, "BB", true);
        assert!(q > 0);
        let fields = all_field_choices(&c, Lang::En);
        let mut form = Form::from_keyword(&kw, &fields);
        form.cond = Cond::And { items: vec![new_compare(&fields, None).unwrap()] };
        save_form(&c, &form).unwrap();
        assert!(broken_keywords(&c).unwrap().is_empty());
    }

    #[test]
    fn describes_conditions_in_both_languages() {
        let (_, _, f) = setup();
        let prio = field(&f, "Priority");
        let pr = &prio.options.values[0];
        let a = Cond::Compare { field_id: prio.id, op: ConditionOp::Eq, value: json!(pr.id) };
        let b = Cond::Compare { field_id: prio.id, op: ConditionOp::IsEmpty, value: Value::Null };
        let c = Cond::Or { items: vec![a.clone(), Cond::And { items: vec![a, b] }] };
        let en = describe_cond(&c, &f, Lang::En);
        let de = describe_cond(&c, &f, Lang::De);
        assert!(en.contains(" OR ") && en.contains(" AND ") && en.contains('('), "{en}");
        assert!(de.contains(" ODER ") && de.contains(" UND "), "{de}");
        assert!(en.starts_with("Priority is "), "{en}");
    }

    #[test]
    fn sortable_excludes_multi_values() {
        assert!(sortable(FieldKind::Number) && sortable(FieldKind::Dropdown) && sortable(FieldKind::Date));
        assert!(!sortable(FieldKind::Multi) && !sortable(FieldKind::Labels) && !sortable(FieldKind::Rich));
        for k in FieldKind::ALL {
            assert!(!kind_label(k, Lang::En).is_empty());
        }
    }

    #[test]
    fn dialog_draws_headless_in_every_mode() {
        let (c, _, f) = setup();
        let ctx = egui::Context::default();
        let mut st = State::default();
        st.open();
        let frame = |st: &mut State| {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))), ..Default::default() }, |ctx| {
                keywords_dialog(ctx, st, &c, Lang::De);
            });
        };
        frame(&mut st);
        assert!(st.loaded && st.list.len() == 4);
        // Bearbeiten: eingebaut, neu (Prädikat), neu (Sortierung), mit tiefer Bedingung
        st.mode = Mode::Edit(Box::new(Form::from_keyword(&st.list[0].clone(), &st.fields)));
        frame(&mut st);
        let mut f2 = Form::new(&f);
        f2.cond = Cond::And { items: vec![Cond::Or { items: vec![new_compare(&f, None).unwrap(), new_compare(&f, None).unwrap()] }, new_compare(&f, None).unwrap()] };
        st.mode = Mode::Edit(Box::new(f2));
        frame(&mut st);
        let mut f3 = Form::new(&f);
        f3.kind = KeywordKind::Sort;
        st.mode = Mode::Edit(Box::new(f3));
        frame(&mut st);
        st.delete = Some(st.list[0].id);
        st.mode = Mode::List;
        frame(&mut st);
        assert!(st.open);
    }
}
