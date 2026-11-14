//! Schnellmenü: Pflichtfelder eines Statuswechsels. Hat der Übergang Pflichtfelder, die am Ticket noch leer sind, zeigt das
//! Menü ein Formular mit genau diesen Feldern (dieselben Eingabebausteine wie der Pflichtfeld-Dialog der Ticket-Ansicht) und
//! führt den Wechsel danach mit den Werten aus. Bei mehreren Tickets gilt die Vereinigung der fehlenden Felder; jedes Ticket
//! bekommt nur die Werte, die zu seinem Typ gehören (siehe `quick_exec::run_transition_with`).
//!
//! Version, Meilenstein und Terminal werden hier selbst gezeichnet (Auswahlliste bzw. Textfeld), weil die allgemeine Eingabe
//! dafür noch ein Platzhalter ist.

use super::ticket_draft::Draft;
use super::ticket_inputs::{field_block, InCx};
use super::widgets;
use super::Cx;
use crate::planning::fields::{get_field, Field, FieldKind};
use crate::planning::sprints::releases::{list_releases, ReleaseKind};
use crate::planning::statuses::get_status;
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::get_ticket;
use crate::planning::transition::plan_transition;
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Id, RichText, Ui, Vec2};
use rusqlite::Connection;
use serde_json::Value;

/// Das Formular für die fehlenden Pflichtfelder.
pub struct Fill {
    pub to: i64,
    pub to_name: String,
    pub project_id: i64,
    /// Fehlende Felder (Vereinigung über alle Tickets) mit ihrem Eingabepuffer.
    pub fields: Vec<(Field, Draft)>,
    /// Bekannte Labels des Projekts (Vorschläge für Label-Felder).
    pub labels: Vec<String>,
    pub error: Option<String>,
}

impl Fill {
    /// Enter bestätigt nur, wenn kein Feld mehrzeilig ist (dann gilt Strg+Enter).
    pub fn enter_submits(&self) -> bool {
        !self.fields.iter().any(|(f, _)| f.kind == FieldKind::Rich)
    }
}

/// Prüft, ob der Wechsel nach `to` für eines der Tickets Pflichtfelder verlangt. `Ok(None)` = Wechsel ohne Formular ausführen
/// (auch wenn er aus anderen Gründen scheitert: diese Fehler meldet der Lauf je Ticket).
pub fn prepare(conn: &Connection, ids: &[i64], to: i64, lang: Lang) -> Result<Option<Fill>, String> {
    let mut need: Vec<i64> = Vec::new();
    let mut project_id = 0;
    for id in ids {
        let Ok(t) = get_ticket(conn, *id) else { continue };
        project_id = t.project_id;
        if let Err(TicketError::TransitionNeedsFields(missing)) = plan_transition(conn, &t, to, &[]) {
            for m in missing {
                if !need.contains(&m.id) {
                    need.push(m.id);
                }
            }
        }
    }
    if need.is_empty() {
        return Ok(None);
    }
    let mut fields = Vec::new();
    for fid in need {
        let f = get_field(conn, fid).map_err(|e| e.message(lang))?;
        let d = Draft::from_value(&f, None);
        fields.push((f, d));
    }
    let to_name = get_status(conn, to).map(|s| s.display(lang)).unwrap_or_default();
    let labels = super::ticket_data::label_suggestions(conn, project_id);
    Ok(Some(Fill { to, to_name, project_id, fields, labels, error: None }))
}

/// Werte aus den Eingabepuffern (leere entfallen). Fehler in einem Feld stehen am Feld; dann `None`.
pub fn collect(fill: &mut Fill, lang: Lang) -> Option<Vec<(i64, Value)>> {
    let mut values = Vec::new();
    let mut bad = false;
    for (f, d) in &mut fill.fields {
        d.err = None;
        match d.to_value(f) {
            Ok(v) if !crate::planning::values::is_empty(Some(&v)) => values.push((f.id, v)),
            Ok(_) => {}
            Err(e) => {
                d.err = Some(e.message(lang));
                bad = true;
            }
        }
    }
    (!bad).then_some(values)
}

fn special(kind: FieldKind) -> bool {
    matches!(kind, FieldKind::Version | FieldKind::Milestone | FieldKind::Terminal)
}

/// Eine Zeile für Version, Meilenstein oder Terminal.
fn special_block(ui: &mut Ui, cx: &Cx, project_id: i64, f: &Field, d: &mut Draft) {
    let lang = cx.lang;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(160.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(f.display(lang)).color(style::dim(ui)).strong()).truncate());
            ui.label(RichText::new("*").color(widgets::DANGER).strong()).on_hover_text(lang.t("Pflichtfeld", "Required field"));
        });
        match f.kind {
            FieldKind::Terminal => {
                ui.add(egui::TextEdit::singleline(&mut d.text).id(Id::new(("quick_fill_term", f.id))).hint_text(lang.t("Name des Terminals", "Terminal name")).desired_width(240.0));
            }
            kind => {
                let rk = if kind == FieldKind::Version { ReleaseKind::Version } else { ReleaseKind::Milestone };
                let list = list_releases(cx.conn, rk, project_id).unwrap_or_default();
                let cur = d.value.as_i64();
                let shown = cur.and_then(|i| list.iter().find(|r| r.id == i)).map(|r| r.name.clone()).unwrap_or_else(|| lang.t("(leer)", "(empty)").to_string());
                egui::ComboBox::from_id_salt(("quick_fill_rel", f.id)).selected_text(shown).width(240.0).show_ui(ui, |ui| {
                    if ui.selectable_label(cur.is_none(), lang.t("(leer)", "(empty)")).clicked() {
                        d.value = Value::Null;
                    }
                    for r in &list {
                        if ui.selectable_label(cur == Some(r.id), &r.name).clicked() {
                            d.value = Value::from(r.id);
                        }
                    }
                });
            }
        }
    });
    if let Some(e) = &d.err {
        widgets::error(ui, e);
    }
    ui.add_space(S2);
}

/// Zeichnet das Formular. Liefert (zurück, ausführen).
pub fn draw(ui: &mut Ui, cx: &Cx, ids: &[i64], prefixes: &[String], fill: &mut Fill) -> (bool, bool) {
    let lang = cx.lang;
    let (mut back, mut apply) = (false, false);
    let heading = match (ids.len(), lang) {
        (1, Lang::De) => format!("Nach {} verschieben", lang.q(&fill.to_name)),
        (1, Lang::En) => format!("Move to {}", lang.q(&fill.to_name)),
        (n, Lang::De) => format!("{n} Tickets nach {} verschieben", lang.q(&fill.to_name)),
        (n, Lang::En) => format!("Move {n} tickets to {}", lang.q(&fill.to_name)),
    };
    ui.label(RichText::new(heading).heading());
    ui.add_space(S1);
    ui.label(lang.t("Für diesen Statuswechsel müssen diese Felder ausgefüllt sein:", "This status change requires these fields to be filled in:"));
    ui.add_space(S2);
    let project_id = fill.project_id;
    let to = fill.to;
    let labels = std::mem::take(&mut fill.labels);
    egui::ScrollArea::vertical().id_salt("quick_fill").max_height(320.0).auto_shrink([true, true]).show(ui, |ui| {
        let ic = InCx { conn: cx.conn, lang, project_id, ticket_id: (ids.len() == 1).then(|| ids[0]), prefixes, labels: &labels, id: Id::new(("quick_fill", to)) };
        for (f, d) in &mut fill.fields {
            if special(f.kind) {
                special_block(ui, cx, project_id, f, d);
            } else {
                let ev = field_block(ui, &ic, f, d, false, None);
                apply |= ev.enter;
            }
        }
    });
    fill.labels = labels;
    if let Some(e) = &fill.error {
        ui.add_space(S1);
        ui.horizontal_wrapped(|ui| widgets::error(ui, e));
    }
    ui.add_space(S2);
    ui.horizontal(|ui| {
        if widgets::primary(ui, lang.t("Verschieben", "Move")).on_hover_text("Ctrl+Enter").clicked() {
            apply = true;
        }
        if widgets::secondary(ui, lang.t("Zurück", "Back")).clicked() {
            back = true;
        }
    });
    (back, apply)
}
