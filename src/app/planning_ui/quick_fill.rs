//! Schnellmenü: Pflichtfelder von Statuswechsel und Anlegen. Verlangt ein Statuswechsel (Statuszeile oder Schnellaktion „Status
//! ändern“) Pflichtfelder, die am Ticket noch leer sind, oder fehlen einem Ticket, das eine Schnellaktion „Ticket anlegen“
//! erzeugen würde, Pflichtfelder ohne Vorgabe, zeigt das Menü ein Formular mit genau diesen Feldern (dieselben Eingabebausteine
//! wie die Ticket-Ansicht: `ticket_inputs::field_block`, auch für Version, Meilenstein und Terminal) und führt die Zeile danach
//! mit den Werten aus – für alle Tickets in **einem** Schritt (ein Rückgängig). Bei mehreren Tickets gilt die Vereinigung der
//! fehlenden Felder; jedes Ticket bekommt nur die Werte, die zu seinem Typ gehören (siehe `quick_exec::run_transition_with` und
//! `run_action_with`).

use super::quick_exec::resolve_context;
use super::ticket_draft::Draft;
use super::ticket_inputs::{field_block, InCx};
use super::widgets;
use super::Cx;
use crate::planning::fields::{get_field, list_fields, Field, FieldKind};
use crate::planning::quick::actions::get_quick_action;
use crate::planning::quick::params::ActionParams;
use crate::planning::quick::resolve::resolve_create_defaults;
use crate::planning::statuses::get_status;
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::get_ticket;
use crate::planning::transition::plan_transition;
use crate::planning::values::is_empty;
use crate::state::Lang;
use crate::style::{S1, S2};
use chrono::NaiveDateTime;
use eframe::egui::{self, Id, RichText, Ui};
use rusqlite::Connection;
use serde_json::Value;

/// Was nach dem Ausfüllen ausgeführt wird.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Statuszeile: Wechsel nach `to`.
    Status { to: i64 },
    /// Schnellaktion („Status ändern“ oder „Ticket anlegen“); `title` ist der vorher erfragte Titel des neuen Tickets.
    Action { id: i64, title: Option<String> },
}

/// Das Formular für die fehlenden Pflichtfelder.
pub struct Fill {
    pub run: Run,
    /// `true`: ein neues Ticket wird angelegt (sonst: Statuswechsel).
    pub create: bool,
    /// Zielstatus bei einem Wechsel, sonst der Name der Aktion.
    pub name: String,
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

    /// Setzt den vorher erfragten Titel des neuen Tickets (nur beim Anlegen).
    pub fn with_title(mut self, title: &str) -> Fill {
        if let Run::Action { title: t, .. } = &mut self.run {
            *t = Some(title.to_string());
        }
        self
    }
}

/// Baut das Formular für die Felder `need` (Kennungen, ohne Doppelte); keine Felder = kein Formular.
fn build(conn: &Connection, need: Vec<i64>, run: Run, create: bool, name: String, project_id: i64, lang: Lang) -> Result<Option<Fill>, String> {
    if need.is_empty() {
        return Ok(None);
    }
    let mut fields = Vec::new();
    for fid in need {
        let f = get_field(conn, fid).map_err(|e| e.message(lang))?;
        let d = Draft::from_value(&f, None);
        fields.push((f, d));
    }
    let labels = super::ticket_data::label_suggestions(conn, project_id);
    Ok(Some(Fill { run, create, name, project_id, fields, labels, error: None }))
}

fn add_missing(need: &mut Vec<i64>, ids: impl IntoIterator<Item = i64>) {
    for id in ids {
        if !need.contains(&id) {
            need.push(id);
        }
    }
}

/// Fehlende Pflichtfelder der Tickets für einen Wechsel nach `to` (`provided`: Werte, die schon feststehen) und das Projekt.
fn missing_for_transition(conn: &Connection, ids: &[i64], to: i64, provided: &[(i64, Value)]) -> (Vec<i64>, i64) {
    let mut need = Vec::new();
    let mut project_id = 0;
    for id in ids {
        let Ok(t) = get_ticket(conn, *id) else { continue };
        project_id = t.project_id;
        if let Err(TicketError::TransitionNeedsFields(missing)) = plan_transition(conn, &t, to, provided) {
            add_missing(&mut need, missing.iter().map(|m| m.id));
        }
    }
    (need, project_id)
}

/// Prüft, ob der Wechsel nach `to` für eines der Tickets Pflichtfelder verlangt. `Ok(None)` = Wechsel ohne Formular ausführen
/// (auch wenn er aus anderen Gründen scheitert: diese Fehler meldet der Lauf je Ticket).
pub fn prepare(conn: &Connection, ids: &[i64], to: i64, lang: Lang) -> Result<Option<Fill>, String> {
    let (need, project_id) = missing_for_transition(conn, ids, to, &[]);
    let name = get_status(conn, to).map(|s| s.display(lang)).unwrap_or_default();
    build(conn, need, Run::Status { to }, false, name, project_id, lang)
}

/// Pflichtfelder eines neuen Tickets, die weder die Aktion noch der Standardwert des Felds füllt. Ein ausdrücklich leerer
/// Wert der Aktion zählt als fehlend (das Datenmodell nimmt dann auch den Standardwert nicht).
fn missing_for_create(conn: &Connection, type_id: i64, values: &[(i64, Value)]) -> Vec<i64> {
    let Ok(fields) = list_fields(conn, type_id) else { return Vec::new() };
    fields
        .iter()
        .filter(|f| f.required)
        .filter(|f| match values.iter().rev().find(|(i, _)| *i == f.id) {
            Some((_, v)) => is_empty(Some(v)),
            None => f.default_value.is_none(),
        })
        .map(|f| f.id)
        .collect()
}

/// Wie [`prepare`] für eine Schnellaktion: „Status ändern“ (Pflichtfelder des Übergangs, soweit die Setze-Werte der Aktion sie
/// nicht füllen) und „Ticket anlegen“ (Pflichtfelder des neuen Tickets ohne Vorgabe). Sitzungsaktionen haben nie ein Formular.
/// Beim Anlegen ohne Titelvorgabe fragt das Menü zuerst den Titel und ruft danach [`Fill::with_title`] auf.
pub fn prepare_action(conn: &Connection, ids: &[i64], action_id: i64, now: NaiveDateTime, lang: Lang) -> Result<Option<Fill>, String> {
    let Ok(action) = get_quick_action(conn, action_id) else { return Ok(None) };
    if action.invalid {
        return Ok(None);
    }
    // Nur Tickets des Typs der Aktion zählen; die anderen meldet der Lauf als Fehler.
    let own: Vec<_> = ids.iter().filter_map(|i| get_ticket(conn, *i).ok()).filter(|t| t.type_id == action.type_id).collect();
    let run = Run::Action { id: action_id, title: None };
    match &action.params {
        ActionParams::ChangeStatus(c) => {
            let provided: Vec<(i64, Value)> = c.set_values.iter().map(|s| (s.field_id, s.value.clone())).collect();
            let own_ids: Vec<i64> = own.iter().map(|t| t.id).collect();
            let (need, project_id) = missing_for_transition(conn, &own_ids, c.to_status_id, &provided);
            let name = get_status(conn, c.to_status_id).map(|s| s.display(lang)).unwrap_or_default();
            build(conn, need, run, false, name, project_id, lang)
        }
        ActionParams::CreateTicket(p) => {
            let today = now.format("%Y-%m-%d").to_string();
            let mut need = Vec::new();
            let mut project_id = 0;
            for t in &own {
                project_id = t.project_id;
                let Ok(ctx) = resolve_context(conn, t, lang, &today) else { continue };
                let Ok(resolved) = resolve_create_defaults(conn, &action, &ctx) else { continue };
                add_missing(&mut need, missing_for_create(conn, p.ticket_type_id, &resolved.values));
            }
            build(conn, need, run, true, action.display(lang), project_id, lang)
        }
        ActionParams::StartSession(_) => Ok(None),
    }
}

/// Werte aus den Eingabepuffern (leere entfallen). Fehler in einem Feld stehen am Feld; dann `None`.
pub fn collect(fill: &mut Fill, lang: Lang) -> Option<Vec<(i64, Value)>> {
    let mut values = Vec::new();
    let mut bad = false;
    for (f, d) in &mut fill.fields {
        d.err = None;
        match d.to_value(f) {
            Ok(v) if !is_empty(Some(&v)) => values.push((f.id, v)),
            Ok(_) => {}
            Err(e) => {
                d.err = Some(e.message(lang));
                bad = true;
            }
        }
    }
    (!bad).then_some(values)
}

/// Zeichnet das Formular. Liefert (zurück, ausführen).
pub fn draw(ui: &mut Ui, cx: &Cx, ids: &[i64], prefixes: &[String], fill: &mut Fill) -> (bool, bool) {
    let lang = cx.lang;
    let (mut back, mut apply) = (false, false);
    let heading = match (fill.create, ids.len(), lang) {
        (true, 1, Lang::De) => format!("{} – Pflichtfelder", fill.name),
        (true, 1, Lang::En) => format!("{} – required fields", fill.name),
        (true, n, Lang::De) => format!("{} ({n} Tickets) – Pflichtfelder", fill.name),
        (true, n, Lang::En) => format!("{} ({n} tickets) – required fields", fill.name),
        (false, 1, Lang::De) => format!("Nach {} verschieben", lang.q(&fill.name)),
        (false, 1, Lang::En) => format!("Move to {}", lang.q(&fill.name)),
        (false, n, Lang::De) => format!("{n} Tickets nach {} verschieben", lang.q(&fill.name)),
        (false, n, Lang::En) => format!("Move {n} tickets to {}", lang.q(&fill.name)),
    };
    ui.label(RichText::new(heading).heading());
    ui.add_space(S1);
    let intro = if fill.create {
        lang.t("Für das neue Ticket müssen diese Felder ausgefüllt sein:", "The new ticket requires these fields to be filled in:")
    } else {
        lang.t("Für diesen Statuswechsel müssen diese Felder ausgefüllt sein:", "This status change requires these fields to be filled in:")
    };
    ui.label(intro);
    ui.add_space(S2);
    let project_id = fill.project_id;
    let salt = match &fill.run {
        Run::Status { to } => (0_u8, *to),
        Run::Action { id, .. } => (1_u8, *id),
    };
    let labels = std::mem::take(&mut fill.labels);
    // Beim Anlegen gibt es das Ticket noch nicht: nichts auszuschließen.
    let ticket_id = (ids.len() == 1 && !fill.create).then(|| ids[0]);
    egui::ScrollArea::vertical().id_salt("quick_fill").max_height(320.0).auto_shrink([true, true]).show(ui, |ui| {
        let ic = InCx { conn: cx.conn, lang, project_id, ticket_id, prefixes, labels: &labels, id: Id::new(("quick_fill", salt)) };
        for (f, d) in &mut fill.fields {
            let ev = field_block(ui, &ic, f, d, false, None);
            apply |= ev.enter;
        }
    });
    fill.labels = labels;
    if let Some(e) = &fill.error {
        ui.add_space(S1);
        ui.horizontal_wrapped(|ui| widgets::error(ui, e));
    }
    ui.add_space(S2);
    ui.horizontal(|ui| {
        let go = if fill.create { lang.t("Anlegen", "Create") } else { lang.t("Verschieben", "Move") };
        if widgets::primary(ui, go).on_hover_text("Ctrl+Enter").clicked() {
            apply = true;
        }
        if widgets::secondary(ui, lang.t("Zurück", "Back")).clicked() {
            back = true;
        }
    });
    (back, apply)
}
