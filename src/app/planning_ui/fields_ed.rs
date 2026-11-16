//! Editor „Felder“: je Typ eine Tabelle (Name, Feldtyp, Pflicht, Standardwert, Abschnitt, „auf Karte anzeigen“, Reihenfolge)
//! und ein Dialog je Feld mit den typspezifischen Optionen (Werteliste mit Farben, Zahl, Textlänge, Zeilen, Ziel-Typen).

use super::delete::Target;
use super::helpers::{color_of, kind_label, move_id};
use super::values;
use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::fields::{create_field, get_field, reorder_fields, update_field, Field, FieldKind, FieldOptions, NewField, OptionValue, UpdateField};
use crate::planning::summary::{ProjectSummary, TypeSummary};
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2, S3};
use eframe::egui::{self, Id, RichText, Ui, Vec2};
use serde_json::{json, Value};

#[derive(Default)]
pub(super) struct State {
    pub ty: Option<i64>,
    dlg: Option<FieldDlg>,
}

/// Eingaben des Felddialogs (neu oder bearbeiten).
struct FieldDlg {
    id: Option<i64>,
    type_id: i64,
    builtin: bool,
    default_named: bool,
    initial_name: String,
    name: String,
    kind: FieldKind,
    required: bool,
    help: String,
    section: String,
    show_on_card: bool,
    values: Vec<OptionValue>,
    min: Option<f64>,
    max: Option<f64>,
    decimals: u8,
    unit: String,
    max_len: Option<u32>,
    rows: u32,
    targets: Vec<i64>,
    /// Standardwert für Text, Rich, Checkbox, Zahl, Datum, Datum+Uhrzeit, Labels (`Null` = keiner).
    def: Value,
    /// Dropdown: Index des Standardwerts in `values`.
    def_idx: Option<usize>,
    /// Mehrfachauswahl: Haken je Wert.
    def_multi: Vec<bool>,
    err: Option<String>,
}

impl FieldDlg {
    fn new(type_id: i64) -> FieldDlg {
        FieldDlg {
            id: None,
            type_id,
            builtin: false,
            default_named: false,
            initial_name: String::new(),
            name: String::new(),
            kind: FieldKind::Text,
            required: false,
            help: String::new(),
            section: String::new(),
            show_on_card: false,
            values: Vec::new(),
            min: None,
            max: None,
            decimals: 0,
            unit: String::new(),
            max_len: None,
            rows: 6,
            targets: Vec::new(),
            def: Value::Null,
            def_idx: None,
            def_multi: Vec::new(),
            err: None,
        }
    }

    fn from_field(f: &Field, lang: Lang) -> FieldDlg {
        let o = &f.options;
        let mut d = FieldDlg::new(f.type_id);
        d.id = Some(f.id);
        d.builtin = f.builtin;
        d.default_named = f.key.is_some() && f.name.is_none();
        d.initial_name = f.display(lang);
        d.name = d.initial_name.clone();
        d.kind = f.kind;
        d.required = f.required;
        d.help = f.help.clone();
        d.section = f.section.clone();
        d.show_on_card = f.show_on_card;
        d.values = o.values.clone();
        d.min = o.min;
        d.max = o.max;
        d.decimals = o.decimals.unwrap_or(0);
        d.unit = o.unit.clone().unwrap_or_default();
        d.max_len = o.max_length;
        d.rows = o.rows.unwrap_or(6);
        d.targets = o.target_types.clone();
        match f.kind {
            FieldKind::Dropdown => d.def_idx = f.default_value.as_ref().and_then(Value::as_i64).and_then(|id| o.values.iter().position(|v| v.id == id)),
            FieldKind::Multi => {
                let ids: Vec<i64> = f.default_value.as_ref().and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                d.def_multi = o.values.iter().map(|v| ids.contains(&v.id)).collect();
            }
            _ => d.def = f.default_value.clone().unwrap_or(Value::Null),
        }
        d
    }

    fn options(&self) -> FieldOptions {
        let mut o = FieldOptions::default();
        match self.kind {
            FieldKind::Dropdown | FieldKind::Multi => o.values = self.values.clone(),
            FieldKind::Number => {
                o.min = self.min;
                o.max = self.max;
                o.decimals = Some(self.decimals);
                o.unit = Some(self.unit.trim().to_string()).filter(|u| !u.is_empty());
            }
            FieldKind::Text => o.max_length = self.max_len,
            FieldKind::Rich => o.rows = Some(self.rows),
            FieldKind::Ticket => o.target_types = self.targets.clone(),
            _ => {}
        }
        o
    }

    /// Standardwert und ob er sich auf einen noch nicht gespeicherten Wert (Kennung 0) bezieht (dann im zweiten Schritt setzen).
    fn default_value(&self) -> (Option<Value>, bool) {
        match self.kind {
            FieldKind::Dropdown => match self.def_idx.and_then(|i| self.values.get(i)) {
                Some(v) if v.id > 0 => (Some(json!(v.id)), false),
                Some(_) => (None, true),
                None => (None, false),
            },
            FieldKind::Multi => {
                let chosen: Vec<&OptionValue> = self.values.iter().zip(self.def_multi.iter()).filter(|(_, on)| **on).map(|(v, _)| v).collect();
                if chosen.is_empty() {
                    (None, false)
                } else if chosen.iter().any(|v| v.id == 0) {
                    (None, true)
                } else {
                    (Some(json!(chosen.iter().map(|v| v.id).collect::<Vec<_>>())), false)
                }
            }
            FieldKind::Text | FieldKind::Rich | FieldKind::Date | FieldKind::Datetime => match self.def.as_str() {
                Some(s) if !s.trim().is_empty() => (Some(Value::String(s.to_string())), false),
                _ => (None, false),
            },
            FieldKind::Labels => match self.def.as_array() {
                Some(a) if !a.is_empty() => (Some(self.def.clone()), false),
                _ => (None, false),
            },
            FieldKind::Checkbox | FieldKind::Number => match &self.def {
                Value::Null => (None, false),
                v => (Some(v.clone()), false),
            },
            _ => (None, false),
        }
    }
}

impl PlanUi {
    pub(super) fn fields_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        let list: Vec<&TypeSummary> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
        if !self.fields.ty.is_some_and(|s| list.iter().any(|t| t.ticket_type.id == s)) {
            self.fields.ty = list.first().map(|t| t.ticket_type.id);
        }
        hint(ui, lang.t(
            "Felder gehören zu einem Typ. Sie bestimmen, was ein Ticket dieses Typs speichert und was auf der Karte erscheint.",
            "Fields belong to a type. They decide what a ticket of that type stores and what shows on the card.",
        ));
        ui.add_space(S2);
        if list.is_empty() {
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Es gibt noch keinen Typ. Lege zuerst im Reiter „Typen“ einen an.", "There is no type yet. Create one in the “Types” tab first.")).color(style::dim(ui)));
            });
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(lang.t("Typ:", "Type:")).color(style::dim(ui)));
            for t in &list {
                let tt = &t.ticket_type;
                if style::pill(ui, &tt.display(lang), Some(t.fields.len()), color_of(&tt.color), self.fields.ty == Some(tt.id)).clicked() {
                    self.fields.ty = Some(tt.id);
                }
            }
        });
        ui.add_space(S2);
        let Some(t) = self.fields.ty.and_then(|s| list.iter().find(|t| t.ticket_type.id == s)).copied() else { return };
        ui.horizontal(|ui| {
            if widgets::primary(ui, &format!("+ {}", lang.t("Neues Feld", "New field"))).clicked() {
                self.fields.dlg = Some(FieldDlg::new(t.ticket_type.id));
            }
            // Einhängepunkt Schlüsselwort-Editor: eigene JQL-Schlüsselwörter bauen auf diesen Feldern auf.
            if ui.button(format!("{} …", lang.t("JQL-Schlüsselwörter", "JQL keywords"))).on_hover_text(lang.t("Eigene Schlüsselwörter für die Abfragesprache verwalten", "Manage your own keywords for the query language")).clicked() {
                self.kw.open();
            }
        });
        ui.add_space(S2);
        if t.fields.is_empty() {
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Dieser Typ hat noch keine Felder.", "This type has no fields yet.")).color(style::dim(ui)));
            });
            return;
        }
        let order: Vec<i64> = t.fields.iter().map(|f| f.id).collect();
        egui::Grid::new(("fields_grid", t.ticket_type.id)).num_columns(8).spacing(Vec2::new(14.0, 8.0)).striped(true).show(ui, |ui| {
            for h in [lang.t("Reihenfolge", "Order"), lang.t("Name", "Name"), lang.t("Feldtyp", "Field type"), lang.t("Pflicht", "Required"), lang.t("Standardwert", "Default"), lang.t("Abschnitt", "Section"), lang.t("Auf Karte", "On card"), ""] {
                ui.label(RichText::new(h).small().color(style::dim(ui)));
            }
            ui.end_row();
            // Eingebaut: der Titel (kein Feld, immer Pflicht)
            builtin_badge(ui, lang);
            ui.label(RichText::new(lang.t("Titel", "Title")).strong());
            ui.label(kind_label(FieldKind::Text, lang));
            ui.label(lang.t("ja", "yes"));
            for _ in 0..3 {
                ui.label(RichText::new("–").color(style::dim(ui)));
            }
            ui.label("");
            ui.end_row();
            for f in &t.fields {
                let rc = row_controls(f);
                if !rc.movable {
                    builtin_badge(ui, lang);
                } else {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, f.id, -1) {
                            let r = reorder_fields(cx.conn, t.ticket_type.id, &o);
                            self.run(cx, r);
                        }
                    }
                    if style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, f.id, 1) {
                            let r = reorder_fields(cx.conn, t.ticket_type.id, &o);
                            self.run(cx, r);
                        }
                    }
                });
                }
                let name = f.display(lang);
                if !rc.editable {
                    ui.label(RichText::new(&name).strong());
                } else if ui.add(egui::Label::new(RichText::new(&name).strong()).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.fields.dlg = Some(FieldDlg::from_field(f, lang));
                }
                ui.label(if f.is_due() { lang.t("Datum, Uhrzeit optional", "Date, time optional") } else { kind_label(f.kind, lang) });
                let mut req = f.required;
                if !rc.editable {
                    ui.label(RichText::new("–").color(style::dim(ui)));
                } else if widgets::check(ui, &mut req, "").changed() {
                    let r = update_field(cx.conn, f.id, &UpdateField { required: Some(req), ..Default::default() });
                    self.run(cx, r);
                }
                let dv = f.default_value.as_ref().map(|v| values::show_value(f.kind, &f.options, v, lang)).unwrap_or_default();
                let dv: String = if dv.chars().count() > 22 { dv.chars().take(21).chain(std::iter::once('…')).collect() } else { dv };
                ui.label(RichText::new(if dv.is_empty() { "–".to_string() } else { dv }).color(style::dim(ui)));
                if let Some(v) = widgets::commit_text(ui, Id::new(("field_section", f.id)), &f.section, 120.0, lang.t("kein Abschnitt", "no section")) {
                    let r = update_field(cx.conn, f.id, &UpdateField { section: Some(v), ..Default::default() });
                    self.run(cx, r);
                }
                let mut card = f.show_on_card;
                if widgets::check(ui, &mut card, "").changed() {
                    let r = update_field(cx.conn, f.id, &UpdateField { show_on_card: Some(card), ..Default::default() });
                    self.run(cx, r);
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if rc.editable && ui.small_button(lang.t("Bearbeiten …", "Edit …")).clicked() {
                        self.fields.dlg = Some(FieldDlg::from_field(f, lang));
                    }
                    if rc.deletable && style::icon_btn_small(ui, SIcon::Trash, lang.t("Feld löschen …", "Delete field …")).clicked() {
                        self.ask_delete(cx, Target::Field(f.id));
                    }
                });
                ui.end_row();
            }
        });
        if t.fields.iter().any(|f| f.key.is_some() && f.name.is_none()) {
            ui.add_space(S2);
            hint(ui, lang.t("Standardnamen werden automatisch übersetzt, bis du sie umbenennst (dann gilt dein Text in beiden Sprachen).", "Default names are translated automatically until you rename them (then your text applies to both languages)."));
        }
    }

    // ------------------------------------------------------------------ Dialog
}

/// Was eine Zeile der Feldliste anbietet: Reihenfolge, Bearbeiten (Dialog, Pflicht), Löschen. Eingebaute Felder (Beschreibung,
/// Fälligkeit) stehen fest oben und lassen sich weder bearbeiten noch löschen; Abschnitt und „Auf Karte“ bleiben änderbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RowControls {
    pub movable: bool,
    pub editable: bool,
    pub deletable: bool,
}

pub(super) fn row_controls(f: &crate::planning::fields::Field) -> RowControls {
    let free = !f.is_fixed();
    RowControls { movable: free, editable: free, deletable: free }
}

/// Kennzeichen „Eingebaut“ in der ersten Spalte (Schloss, Tooltip).
fn builtin_badge(ui: &mut Ui, lang: Lang) {
    ui.label(RichText::new(lang.t("Eingebaut", "Built-in")).small().color(style::dim(ui))).on_hover_text(lang.t(
        "Titel, Beschreibung und Fälligkeit hat jedes Ticket. Sie lassen sich nicht löschen.",
        "Every ticket has a title, a description and a due date. They cannot be deleted.",
    ));
}

impl PlanUi {

    pub(super) fn fields_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.fields.dlg.take() else { return };
        let lang = cx.lang;
        let sum = self.sum.clone();
        let mut save = false;
        let mut close = false;
        let m = egui::Modal::new(Self::modal_id("field")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 600.0));
            widgets::dialog_title(ui, if d.id.is_some() { lang.t("Feld bearbeiten", "Edit field") } else { lang.t("Neues Feld", "New field") });
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 250.0).max(260.0)).auto_shrink([true, true]).show(ui, |ui| {
                field_form(ui, &mut d, sum.as_deref(), lang);
            });
            ui.add_space(S2);
            if let Some(e) = &d.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, !d.name.trim().is_empty(), lang.t("Speichern", "Save")).clicked() {
                    save = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if save {
            match self.save_field(cx, &d) {
                Ok(()) => close = true,
                Err(e) => d.err = Some(e),
            }
        }
        if !close {
            self.fields.dlg = Some(d);
        }
    }

    /// Feld anlegen oder ändern. Bezieht sich der Standardwert auf einen neuen Listenwert, wird er im zweiten Schritt gesetzt
    /// (erst beim Speichern bekommt der Wert seine Kennung).
    fn save_field(&mut self, cx: &Cx, d: &FieldDlg) -> Result<(), String> {
        let lang = cx.lang;
        let opts = d.options();
        let (default, deferred) = d.default_value();
        let msg = |e: crate::planning::PlanError| e.message(lang);
        let id = match d.id {
            Some(id) => {
                let name = if d.name.trim() != d.initial_name { Some(d.name.clone()) } else { None };
                let u = UpdateField {
                    name,
                    options: Some(opts),
                    required: Some(d.required),
                    default_value: Some(default),
                    help: Some(d.help.clone()),
                    section: Some(d.section.clone()),
                    show_on_card: Some(d.show_on_card),
                };
                update_field(cx.conn, id, &u).map_err(msg)?;
                id
            }
            None => {
                let n = NewField {
                    type_id: d.type_id,
                    key: None,
                    name: Some(d.name.clone()),
                    kind: d.kind,
                    options: opts,
                    required: d.required,
                    default_value: default,
                    help: d.help.clone(),
                    section: d.section.clone(),
                    show_on_card: d.show_on_card,
                    builtin: false,
                };
                create_field(cx.conn, &n).map_err(msg)?
            }
        };
        if deferred {
            // Kennungen sind jetzt vergeben: Standardwert über die Reihenfolge der Werte setzen.
            let f = get_field(cx.conn, id).map_err(msg)?;
            let dv = match d.kind {
                FieldKind::Dropdown => d.def_idx.and_then(|i| f.options.values.get(i)).map(|v| json!(v.id)),
                _ => {
                    let ids: Vec<i64> = f.options.values.iter().zip(d.def_multi.iter()).filter(|(_, on)| **on).map(|(v, _)| v.id).collect();
                    if ids.is_empty() { None } else { Some(json!(ids)) }
                }
            };
            update_field(cx.conn, id, &UpdateField { default_value: Some(dv), ..Default::default() }).map_err(msg)?;
        }
        self.dirty = true;
        self.msg = None;
        Ok(())
    }
}

/// Der gesamte Inhalt des Felddialogs.
fn field_form(ui: &mut Ui, d: &mut FieldDlg, sum: Option<&ProjectSummary>, lang: Lang) {
    let w = ui.available_width();
    egui::Grid::new("field_form_grid").num_columns(2).spacing(Vec2::new(16.0, 10.0)).min_col_width(110.0).show(ui, |ui| {
        widgets::row(ui, lang.t("Name", "Name"), |ui| {
            let r = ui.add_sized([(w - 150.0).max(160.0), 28.0], egui::TextEdit::singleline(&mut d.name).margin(egui::Margin::symmetric(8, 5)));
            if d.id.is_none() && d.name.is_empty() {
                r.request_focus();
            }
        });
        widgets::row(ui, lang.t("Feldtyp", "Field type"), |ui| {
            if d.id.is_some() {
                ui.label(format!("{} ({})", kind_label(d.kind, lang), lang.t("nach dem Anlegen nicht änderbar", "cannot be changed after creation")));
            } else {
                let before = d.kind;
                egui::ComboBox::from_id_salt("field_kind").selected_text(kind_label(d.kind, lang)).width(240.0).height(420.0).show_ui(ui, |ui| {
                    for k in FieldKind::ALL {
                        ui.selectable_value(&mut d.kind, k, kind_label(k, lang));
                    }
                });
                if d.kind != before {
                    d.def = Value::Null;
                    d.def_idx = None;
                    d.def_multi.clear();
                }
            }
        });
        widgets::row(ui, lang.t("Pflichtfeld", "Required"), |ui| {
            style::toggle(ui, &mut d.required);
        });
        widgets::row(ui, lang.t("Auf der Karte anzeigen", "Show on card"), |ui| {
            style::toggle(ui, &mut d.show_on_card);
        });
        widgets::row(ui, lang.t("Abschnitt", "Section"), |ui| {
            ui.add_sized([260.0, 28.0], egui::TextEdit::singleline(&mut d.section).hint_text(lang.t("optional, gruppiert Felder im Ticket", "optional, groups fields in the ticket")).margin(egui::Margin::symmetric(8, 5)));
        });
        widgets::row(ui, lang.t("Hilfetext", "Help text"), |ui| {
            ui.add_sized([(w - 150.0).max(160.0), 56.0], egui::TextEdit::multiline(&mut d.help).margin(egui::Margin::symmetric(8, 5)));
        });
    });
    if d.default_named {
        hint(ui, lang.t("Standardname – er wird automatisch übersetzt, bis du ihn änderst (dann gilt dein Text in beiden Sprachen).", "Default name – it is translated automatically until you change it (then your text applies to both languages)."));
    }
    ui.add_space(S2);
    // Optionen je Feldtyp
    let has_opts = matches!(d.kind, FieldKind::Dropdown | FieldKind::Multi | FieldKind::Number | FieldKind::Text | FieldKind::Rich | FieldKind::Ticket);
    if has_opts {
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Optionen", "Options")).strong());
            ui.add_space(S1);
            match d.kind {
                FieldKind::Dropdown | FieldKind::Multi => values_editor(ui, d, lang),
                FieldKind::Number => number_options(ui, d, lang),
                FieldKind::Text => {
                    ui.horizontal(|ui| {
                        let mut on = d.max_len.is_some();
                        if widgets::check(ui, &mut on, lang.t("Maximale Länge", "Maximum length")).changed() {
                            d.max_len = if on { Some(200) } else { None };
                        }
                        if let Some(n) = &mut d.max_len {
                            ui.add(egui::DragValue::new(n).range(1..=100_000).speed(1.0).suffix(format!(" {}", lang.t("Zeichen", "characters"))));
                        }
                    });
                }
                FieldKind::Rich => {
                    ui.horizontal(|ui| {
                        ui.label(lang.t("Höhe des Eingabefelds:", "Height of the input:"));
                        ui.add(egui::DragValue::new(&mut d.rows).range(1..=50).speed(0.2).suffix(format!(" {}", lang.t("Zeilen", "lines"))));
                    });
                }
                FieldKind::Ticket => {
                    ui.label(lang.t("Erlaubte Typen der verknüpften Tickets (keiner gewählt = jeder Typ):", "Allowed types of the linked tickets (none selected = any type):"));
                    if let Some(sum) = sum {
                        for t in sum.types.iter().filter(|t| !t.ticket_type.system) {
                            let tid = t.ticket_type.id;
                            let mut on = d.targets.contains(&tid);
                            if widgets::check(ui, &mut on, t.ticket_type.display(lang)).changed() {
                                if on {
                                    d.targets.push(tid);
                                } else {
                                    d.targets.retain(|x| *x != tid);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        });
        ui.add_space(S2);
    }
    // Standardwert
    widgets::card(ui, |ui| {
        ui.label(RichText::new(lang.t("Standardwert", "Default value")).strong());
        ui.add_space(S1);
        default_editor(ui, d, lang);
    });
}

fn number_options(ui: &mut Ui, d: &mut FieldDlg, lang: Lang) {
    ui.horizontal(|ui| {
        let mut on = d.min.is_some();
        if widgets::check(ui, &mut on, lang.t("Minimum", "Minimum")).changed() {
            d.min = if on { Some(0.0) } else { None };
        }
        if let Some(v) = &mut d.min {
            ui.add(egui::DragValue::new(v).speed(0.5));
        }
        ui.add_space(S3);
        let mut on = d.max.is_some();
        if widgets::check(ui, &mut on, lang.t("Maximum", "Maximum")).changed() {
            d.max = if on { Some(100.0) } else { None };
        }
        if let Some(v) = &mut d.max {
            ui.add(egui::DragValue::new(v).speed(0.5));
        }
    });
    ui.horizontal(|ui| {
        ui.label(lang.t("Nachkommastellen:", "Decimal places:"));
        ui.add(egui::DragValue::new(&mut d.decimals).range(0..=6).speed(0.1));
        ui.add_space(S3);
        ui.label(lang.t("Einheit:", "Unit:"));
        ui.add(egui::TextEdit::singleline(&mut d.unit).desired_width(80.0).hint_text(lang.t("z. B. h, SP", "e.g. h, SP")).margin(egui::Margin::symmetric(8, 4)));
    });
}

/// Werteliste von Dropdown und Mehrfachauswahl: Name, Farbe, Reihenfolge, entfernen.
fn values_editor(ui: &mut Ui, d: &mut FieldDlg, lang: Lang) {
    let mut remove: Option<usize> = None;
    let mut swap: Option<(usize, usize)> = None;
    let n = d.values.len();
    if n == 0 {
        hint(ui, lang.t("Noch keine Werte – füge unten welche hinzu.", "No values yet – add some below."));
    }
    for i in 0..n {
        ui.horizontal(|ui| {
            let v = &mut d.values[i];
            let mut hex = v.color.clone().unwrap_or_else(|| "#97A3B0".to_string());
            if widgets::color_edit(ui, ("opt_color", i, v.id), &mut hex, lang) {
                v.color = Some(hex);
            }
            let mut text = v.name.clone().unwrap_or_else(|| v.display(lang));
            if ui.add(egui::TextEdit::singleline(&mut text).id(Id::new(("opt_name", i, v.id))).desired_width(220.0).margin(egui::Margin::symmetric(8, 4))).changed() {
                v.name = Some(text);
            }
            if style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() && i > 0 {
                swap = Some((i, i - 1));
            }
            if style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() && i + 1 < n {
                swap = Some((i, i + 1));
            }
            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Wert entfernen", "Remove value")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some((a, b)) = swap {
        d.values.swap(a, b);
        if d.def_multi.len() == d.values.len() {
            d.def_multi.swap(a, b);
        }
        d.def_idx = d.def_idx.map(|x| if x == a { b } else if x == b { a } else { x });
    }
    if let Some(i) = remove {
        d.values.remove(i);
        if i < d.def_multi.len() {
            d.def_multi.remove(i);
        }
        d.def_idx = match d.def_idx {
            Some(x) if x == i => None,
            Some(x) if x > i => Some(x - 1),
            other => other,
        };
    }
    if ui.button(format!("+ {}", lang.t("Wert hinzufügen", "Add value"))).clicked() {
        let color = crate::palette::nth(crate::palette::Area::Expert, d.values.len());
        d.values.push(OptionValue { id: 0, key: None, name: Some(format!("{} {}", lang.t("Wert", "Value"), d.values.len() + 1)), color: Some(color) });
    }
}

fn default_editor(ui: &mut Ui, d: &mut FieldDlg, lang: Lang) {
    let o = d.options();
    match d.kind {
        FieldKind::Ticket | FieldKind::Terminal | FieldKind::Version | FieldKind::Milestone => {
            hint(ui, lang.t("Verweis-Felder haben keinen Standardwert.", "Reference fields have no default value."));
        }
        FieldKind::Dropdown => {
            let sel = d.def_idx.and_then(|i| d.values.get(i)).map(|v| v.name.clone().unwrap_or_else(|| v.display(lang))).unwrap_or_else(|| lang.t("Kein Standardwert", "No default").to_string());
            egui::ComboBox::from_id_salt("field_def_dd").selected_text(sel).width(240.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut d.def_idx, None, lang.t("Kein Standardwert", "No default"));
                for (i, v) in d.values.iter().enumerate() {
                    ui.selectable_value(&mut d.def_idx, Some(i), v.name.clone().unwrap_or_else(|| v.display(lang)));
                }
            });
        }
        FieldKind::Multi => {
            d.def_multi.resize(d.values.len(), false);
            if d.values.is_empty() {
                hint(ui, lang.t("Füge zuerst Werte hinzu.", "Add values first."));
            }
            for (i, v) in d.values.iter().enumerate() {
                widgets::check(ui, &mut d.def_multi[i], v.name.clone().unwrap_or_else(|| v.display(lang)));
            }
        }
        FieldKind::Checkbox => {
            let sel = match d.def.as_bool() {
                Some(true) => lang.t("Angehakt", "Checked"),
                Some(false) => lang.t("Nicht angehakt", "Unchecked"),
                None => lang.t("Kein Standardwert", "No default"),
            };
            egui::ComboBox::from_id_salt("field_def_cb").selected_text(sel).width(200.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut d.def, Value::Null, lang.t("Kein Standardwert", "No default"));
                ui.selectable_value(&mut d.def, json!(true), lang.t("Angehakt", "Checked"));
                ui.selectable_value(&mut d.def, json!(false), lang.t("Nicht angehakt", "Unchecked"));
            });
        }
        FieldKind::Number => {
            ui.horizontal(|ui| {
                let mut on = !d.def.is_null();
                if widgets::check(ui, &mut on, lang.t("Standardwert setzen", "Set a default")).changed() {
                    d.def = if on { values::blank(FieldKind::Number, &o) } else { Value::Null };
                }
                if !d.def.is_null() {
                    values::element_editor(ui, Id::new("field_def_num"), FieldKind::Number, &o, &mut d.def, lang);
                }
            });
        }
        FieldKind::Rich => {
            let mut s = d.def.as_str().unwrap_or("").to_string();
            if ui.add(egui::TextEdit::multiline(&mut s).desired_width(ui.available_width()).desired_rows(3).hint_text(lang.t("leer = kein Standardwert", "empty = no default")).margin(egui::Margin::symmetric(8, 5))).changed() {
                d.def = Value::String(s);
            }
        }
        FieldKind::Labels => {
            if !d.def.is_array() {
                d.def = json!([]);
            }
            values::list_editor(ui, Id::new("field_def_labels"), FieldKind::Labels, &o, &mut d.def, lang);
            hint(ui, lang.t("Labels durch Komma trennen; leer = kein Standardwert.", "Separate labels with commas; empty = no default."));
        }
        FieldKind::Text | FieldKind::Date | FieldKind::Datetime => {
            let mut s = d.def.as_str().unwrap_or("").to_string();
            let hint_text = match d.kind {
                FieldKind::Date => lang.t("JJJJ-MM-TT, leer = kein Standardwert", "YYYY-MM-DD, empty = no default"),
                FieldKind::Datetime => lang.t("JJJJ-MM-TTTHH:MM, leer = kein Standardwert", "YYYY-MM-DDTHH:MM, empty = no default"),
                _ => lang.t("leer = kein Standardwert", "empty = no default"),
            };
            if ui.add(egui::TextEdit::singleline(&mut s).desired_width(280.0).hint_text(hint_text).margin(egui::Margin::symmetric(8, 5))).changed() {
                d.def = Value::String(s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dlg(kind: FieldKind) -> FieldDlg {
        let mut d = FieldDlg::new(1);
        d.kind = kind;
        d
    }

    #[test]
    fn options_only_carry_what_fits_the_kind() {
        let mut d = dlg(FieldKind::Number);
        d.min = Some(1.0);
        d.max = Some(9.0);
        d.decimals = 2;
        d.unit = " h ".into();
        d.max_len = Some(5);
        d.rows = 9;
        let o = d.options();
        assert_eq!((o.min, o.max, o.decimals, o.unit.as_deref()), (Some(1.0), Some(9.0), Some(2), Some("h")));
        assert!(o.max_length.is_none() && o.rows.is_none() && o.values.is_empty());
        d.kind = FieldKind::Text;
        let o = d.options();
        assert_eq!(o.max_length, Some(5));
        assert!(o.min.is_none());
        d.kind = FieldKind::Rich;
        assert_eq!(d.options().rows, Some(9));
        d.kind = FieldKind::Ticket;
        d.targets = vec![3];
        assert_eq!(d.options().target_types, vec![3]);
    }

    #[test]
    fn defaults_follow_the_kind() {
        let mut d = dlg(FieldKind::Text);
        assert_eq!(d.default_value(), (None, false));
        d.def = json!("  ");
        assert_eq!(d.default_value(), (None, false), "nur Leerraum = kein Standardwert");
        d.def = json!("hallo");
        assert_eq!(d.default_value(), (Some(json!("hallo")), false));
        let mut d = dlg(FieldKind::Checkbox);
        d.def = json!(false);
        assert_eq!(d.default_value(), (Some(json!(false)), false), "„nicht angehakt“ ist ein Standardwert");
        let mut d = dlg(FieldKind::Dropdown);
        d.values = vec![OptionValue { id: 4, name: Some("A".into()), ..Default::default() }, OptionValue { id: 0, name: Some("B".into()), ..Default::default() }];
        d.def_idx = Some(0);
        assert_eq!(d.default_value(), (Some(json!(4)), false));
        d.def_idx = Some(1);
        assert_eq!(d.default_value(), (None, true), "neuer Wert ohne Kennung → zweiter Schritt");
        let mut d = dlg(FieldKind::Multi);
        d.values = vec![OptionValue { id: 1, ..Default::default() }, OptionValue { id: 2, ..Default::default() }];
        d.def_multi = vec![true, true];
        assert_eq!(d.default_value(), (Some(json!([1, 2])), false));
        d.def_multi = vec![false, false];
        assert_eq!(d.default_value(), (None, false));
        let d = dlg(FieldKind::Version);
        assert_eq!(d.default_value(), (None, false));
    }

    #[test]
    fn saving_a_new_dropdown_default_in_two_steps_works_end_to_end() {
        use crate::planning::projects::{create_project, NewProject};
        use crate::planning::types::{create_type, NewType};
        let conn = crate::db::open_memory();
        let p = create_project(&conn, &NewProject::new("AB", "P", "#112233")).unwrap();
        let t = create_type(&conn, &NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let mut d = dlg(FieldKind::Dropdown);
        d.type_id = t;
        d.name = "Farbe".into();
        d.values = vec![OptionValue::new("Rot"), OptionValue::new("Blau")];
        d.def_idx = Some(1);
        let opts = d.options();
        let (default, deferred) = d.default_value();
        assert!(default.is_none() && deferred);
        let id = create_field(&conn, &NewField { type_id: t, key: None, name: Some(d.name.clone()), kind: d.kind, options: opts, required: false, default_value: default, help: String::new(), section: String::new(), show_on_card: false, builtin: false }).unwrap();
        let f = get_field(&conn, id).unwrap();
        let blue = f.options.values.iter().position(|v| v.name.as_deref() == Some("Blau")).unwrap();
        assert_eq!(blue, 1, "die Reihenfolge der Werte bleibt erhalten");
        let dv = d.def_idx.and_then(|i| f.options.values.get(i)).map(|v| json!(v.id));
        update_field(&conn, id, &UpdateField { default_value: Some(dv), ..Default::default() }).unwrap();
        let f = get_field(&conn, id).unwrap();
        assert_eq!(f.default_value, Some(json!(f.options.values[1].id)));
        // und zurück in den Dialog: der Standardwert findet seinen Index wieder
        let back = FieldDlg::from_field(&f, Lang::En);
        assert_eq!(back.def_idx, Some(1));
        assert_eq!(back.initial_name, "Farbe");
    }
}
