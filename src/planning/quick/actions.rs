//! Quick-Actions eines Tickettyps: Anlegen, Ändern, Löschen, Ordnen, Duplizieren, Prüfung gegen die Konfiguration und
//! Aufräumen verwaister Verweise.
//!
//! * Jede Aktion hängt an einem Typ (`type_id`) und erscheint im Ticket dieses Typs. System-Typen/-Projekte (Todolist) haben keine.
//! * **Prüfung** ([`validate_params`]): Ziel-Typ gehört zum Projekt (kein System-Typ), Felder gehören zum Ziel-Typ, Werte
//!   passen über `fields::check_value`, Status gehört zum Workflow des Typs, Link-Typ existiert und verbindet Tickets,
//!   Platzhalter sind bekannt. Gespeichert wird nur Geprüftes.
//! * **Aufräumen** ([`prune_quick_action_refs`]): Die Löschfunktionen der Konfiguration rufen es später nach dem Löschen eines
//!   Typs/Status/Feldes/Verknüpfungstyps auf. Entfernbares (Standardwert eines gelöschten Feldes, gesetzter Wert, Verknüpfung)
//!   entfällt, was nicht reparierbar ist (Ziel-Typ oder Ziel-Status fehlt), markiert die Aktion als **ungültig**
//!   (`invalid`). Eine ungültige Aktion bleibt sichtbar und editierbar, lässt sich aber nicht ausführen und fehlt im Menü.
//!   Nach einer gültigen Änderung (`update_quick_action`) verschwindet die Markierung.

use super::params::{ActionKind, ActionParams, ChangeStatusParams, CreateTicketParams, DefaultValue, FieldRef, LinkDirection, StartSessionParams};
use super::resolve::{check_placeholders, placeholders_in, MAX_EXPANDED_CHARS, MAX_TITLE_CHARS, SOURCE_REF};
use super::{atomic, FieldLabel, FieldProblem, QRes, QuickError};
use crate::planning::common::{self, clean_name, ensure_name_free, rename_value, validate_ident, MAX_NAME_CHARS};
use crate::planning::fields::{check_value, list_fields, Field, FieldKind};
use crate::planning::link_types::{get_link_type, TargetKind};
use crate::planning::names;
use crate::planning::types::{editable_type, get_type, list_types, TicketType};
use crate::planning::workflow::workflow_statuses;
use crate::planning::PlanError;
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;

/// Höchstzahl der Aktionen je Typ.
pub const MAX_ACTIONS_PER_TYPE: usize = 50;
/// Höchstzahl der Standardwerte bzw. zu setzenden Werte je Aktion.
pub const MAX_ENTRIES: usize = 100;
/// Höchstlänge einer Shell-Kennung.
const MAX_SHELL_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct QuickAction {
    pub id: i64,
    pub type_id: i64,
    pub key: Option<String>,
    /// Knopftext (eigener Name, gilt für beide Sprachen).
    pub name: Option<String>,
    pub params: ActionParams,
    pub position: i64,
    /// Verweise stimmen nicht mehr (oder die Parameter sind nicht lesbar): nicht ausführbar, nicht im Menü.
    pub invalid: bool,
}

impl QuickAction {
    pub fn display(&self, lang: Lang) -> String {
        names::display_name(self.key.as_deref(), self.name.as_deref(), lang)
    }

    pub fn kind(&self) -> ActionKind {
        self.params.kind()
    }
}

#[derive(Debug, Clone)]
pub struct NewQuickAction {
    pub type_id: i64,
    pub key: Option<String>,
    pub name: Option<String>,
    pub params: ActionParams,
}

impl NewQuickAction {
    /// Eigene Aktion mit Knopftext.
    pub fn new(type_id: i64, name: &str, params: ActionParams) -> Self {
        NewQuickAction { type_id, key: None, name: Some(name.to_string()), params }
    }
}

/// Änderungen an einer Aktion; `None` = unverändert. Die Art lässt sich nicht ändern.
#[derive(Debug, Clone, Default)]
pub struct UpdateQuickAction {
    /// Umbenennen; leer = Standardname (nur mit Schlüssel).
    pub name: Option<String>,
    pub params: Option<ActionParams>,
}

const COLS: &str = "id, type_id, key, name, action, params, position, invalid";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<QuickAction> {
    let action: String = r.get(4)?;
    let json: String = r.get(5)?;
    let kind = ActionKind::parse(&action).unwrap_or(ActionKind::StartSession);
    // Nicht lesbare Parameter (kaputt, neuere Version) machen die Aktion ungültig, aber nicht die ganze Liste unlesbar.
    let (params, unreadable) = match ActionParams::from_json(kind, &json) {
        Ok(p) => (p, false),
        Err(_) => (ActionParams::empty(kind), true),
    };
    Ok(QuickAction { id: r.get(0)?, type_id: r.get(1)?, key: r.get(2)?, name: r.get(3)?, params, position: r.get(6)?, invalid: unreadable || r.get::<_, i64>(7)? != 0 })
}

pub fn get_quick_action(conn: &Connection, id: i64) -> QRes<QuickAction> {
    conn.query_row(&format!("SELECT {COLS} FROM quick_actions WHERE id = ?1"), [id], from_row).optional()?.ok_or(QuickError::Plan(PlanError::NotFound))
}

/// Aktionen eines Typs in Anzeigereihenfolge.
pub fn list_quick_actions(conn: &Connection, type_id: i64) -> QRes<Vec<QuickAction>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM quick_actions WHERE type_id = ?1 ORDER BY position, id"))?;
    Ok(st.query_map([type_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Alle Aktionen aller Typen des Projekts (nach Typ, dann Reihenfolge).
pub fn list_project_quick_actions(conn: &Connection, project_id: i64) -> QRes<Vec<QuickAction>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM quick_actions WHERE type_id IN (SELECT id FROM ticket_types WHERE project_id = ?1) ORDER BY type_id, position, id"
    ))?;
    Ok(st.query_map([project_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

// ------------------------------------------------------------------ Prüfung

fn label_of(f: &Field) -> FieldLabel {
    FieldLabel::Field { key: f.key.clone(), name: f.name.clone() }
}

fn field_err(label: FieldLabel, problem: FieldProblem) -> QuickError {
    QuickError::Field { field: label, problem }
}

/// Prüft einen Standardwert gegen die Felder des Ziel-Typs. `owner_id` ist der Typ, der die Aktion trägt (= Typ des Ausgangstickets).
fn check_default(owner_id: i64, target_fields: &[Field], d: &DefaultValue) -> QRes<()> {
    match d.field_id {
        FieldRef::Title => {
            let bad = || field_err(FieldLabel::Title, FieldProblem::Value);
            let s = d.value.as_str().ok_or_else(bad)?;
            if s.trim().is_empty() || s.chars().count() > MAX_TITLE_CHARS {
                return Err(bad());
            }
            check_placeholders(s)
        }
        FieldRef::Field(id) => {
            let f = target_fields.iter().find(|f| f.id == id).ok_or_else(|| field_err(FieldLabel::Id(id), FieldProblem::NotInType))?;
            let bad = || field_err(label_of(f), FieldProblem::Value);
            let v = &d.value;
            match f.kind {
                FieldKind::Terminal => Err(field_err(label_of(f), FieldProblem::NoDefault)),
                // Ein Ticket-Verweis als Standardwert kann nur „das Ausgangsticket“ sein – und das nur, wenn der Typ erlaubt ist.
                FieldKind::Ticket => {
                    if v.as_str() == Some(SOURCE_REF) && (f.options.target_types.is_empty() || f.options.target_types.contains(&owner_id)) {
                        Ok(())
                    } else {
                        Err(bad())
                    }
                }
                // Versionen/Meilensteine sind Objekte des Projekts; hier genügt eine plausible Kennung.
                FieldKind::Version | FieldKind::Milestone => {
                    if v.as_i64().is_some_and(|i| i > 0) { Ok(()) } else { Err(bad()) }
                }
                FieldKind::Text => {
                    let s = v.as_str().ok_or_else(bad)?;
                    if placeholders_in(s).is_empty() { check_value(f.kind, &f.options, v, true).map_err(|_| bad()) } else { check_placeholders(s) }
                }
                FieldKind::Rich => {
                    let s = v.as_str().ok_or_else(bad)?;
                    if s.chars().count() > MAX_EXPANDED_CHARS {
                        return Err(bad());
                    }
                    check_placeholders(s)
                }
                FieldKind::Labels => {
                    check_value(f.kind, &f.options, v, true).map_err(|_| bad())?;
                    for x in v.as_array().into_iter().flatten() {
                        check_placeholders(x.as_str().unwrap_or_default())?;
                    }
                    Ok(())
                }
                _ => check_value(f.kind, &f.options, v, true).map_err(|_| bad()),
            }
        }
    }
}

fn target_type_of(conn: &Connection, owner: &TicketType, type_id: i64) -> QRes<TicketType> {
    let t = get_type(conn, type_id).map_err(|_| QuickError::TargetType)?;
    if t.project_id != owner.project_id || t.system { Err(QuickError::TargetType) } else { Ok(t) }
}

fn validate_create(conn: &Connection, owner: &TicketType, c: &CreateTicketParams) -> QRes<CreateTicketParams> {
    let target = target_type_of(conn, owner, c.ticket_type_id)?;
    let mut out = CreateTicketParams { ticket_type_id: target.id, relation: None, defaults: Vec::new() };
    if let Some(r) = &c.relation {
        let l = get_link_type(conn, r.link_type_id).map_err(|_| QuickError::Relation)?;
        if l.target_kind != TargetKind::Ticket {
            return Err(QuickError::Relation);
        }
        // Bei symmetrischen Typen gibt es keine Richtung; normalisiert wird auf „Out“.
        out.relation = Some(super::params::RelationSpec { link_type_id: l.id, direction: if l.symmetric { LinkDirection::Out } else { r.direction } });
    }
    if c.defaults.len() > MAX_ENTRIES {
        return Err(QuickError::TooMany { max: MAX_ENTRIES });
    }
    let fields = list_fields(conn, target.id)?;
    let mut seen: HashSet<FieldRef> = HashSet::new();
    for d in &c.defaults {
        if !seen.insert(d.field_id) {
            let label = match d.field_id {
                FieldRef::Title => FieldLabel::Title,
                FieldRef::Field(id) => fields.iter().find(|f| f.id == id).map(label_of).unwrap_or(FieldLabel::Id(id)),
            };
            return Err(field_err(label, FieldProblem::Duplicate));
        }
        check_default(owner.id, &fields, d)?;
        out.defaults.push(d.clone());
    }
    Ok(out)
}

fn check_set_value(fields: &[Field], s: &crate::planning::workflow::SetValue) -> QRes<()> {
    let f = fields.iter().find(|f| f.id == s.field_id).ok_or_else(|| field_err(FieldLabel::Id(s.field_id), FieldProblem::NotInType))?;
    if f.kind == FieldKind::Terminal {
        return Err(field_err(label_of(f), FieldProblem::NoDefault));
    }
    check_value(f.kind, &f.options, &s.value, true).map_err(|_| field_err(label_of(f), FieldProblem::Value))
}

fn validate_change(conn: &Connection, owner: &TicketType, c: &ChangeStatusParams) -> QRes<ChangeStatusParams> {
    if !owner.workflow {
        return Err(PlanError::WorkflowOff.into());
    }
    if !workflow_statuses(conn, owner.id)?.contains(&c.to_status_id) {
        return Err(PlanError::StatusNotInWorkflow.into());
    }
    if c.set_values.len() > MAX_ENTRIES {
        return Err(QuickError::TooMany { max: MAX_ENTRIES });
    }
    let fields = list_fields(conn, owner.id)?;
    let mut seen = HashSet::new();
    for s in &c.set_values {
        if !seen.insert(s.field_id) {
            let label = fields.iter().find(|f| f.id == s.field_id).map(label_of).unwrap_or(FieldLabel::Id(s.field_id));
            return Err(field_err(label, FieldProblem::Duplicate));
        }
        check_set_value(&fields, s)?;
    }
    Ok(c.clone())
}

fn validate_session(c: &StartSessionParams) -> QRes<StartSessionParams> {
    let mut out = c.clone();
    if let Some(s) = &c.shell {
        let t = s.trim();
        if t.is_empty() || t.chars().count() > MAX_SHELL_CHARS || t.chars().any(char::is_control) {
            return Err(QuickError::Shell);
        }
        out.shell = Some(t.to_string());
    }
    if c.prompt_from_ticket && !c.claude {
        return Err(QuickError::PromptNeedsClaude);
    }
    Ok(out)
}

fn validate_with_owner(conn: &Connection, owner: &TicketType, p: &ActionParams) -> QRes<ActionParams> {
    Ok(match p {
        ActionParams::CreateTicket(c) => ActionParams::CreateTicket(validate_create(conn, owner, c)?),
        ActionParams::ChangeStatus(c) => ActionParams::ChangeStatus(validate_change(conn, owner, c)?),
        ActionParams::StartSession(c) => ActionParams::StartSession(validate_session(c)?),
    })
}

/// Prüft die Parameter einer Aktion des Typs `type_id` gegen die Konfiguration und liefert sie normalisiert zurück.
pub fn validate_params(conn: &Connection, type_id: i64, p: &ActionParams) -> QRes<ActionParams> {
    let owner = editable_type(conn, type_id)?;
    validate_with_owner(conn, &owner, p)
}

/// Prüft eine gespeicherte Aktion erneut (z. B. für die Anzeige „Aktion ist ungültig“ mit Grund).
pub fn check_quick_action(conn: &Connection, id: i64) -> QRes<()> {
    let a = get_quick_action(conn, id)?;
    let owner = get_type(conn, a.type_id)?;
    validate_with_owner(conn, &owner, &a.params).map(|_| ())
}

// ------------------------------------------------------------------ Anlegen, ändern, löschen

fn count_for_type(conn: &Connection, type_id: i64) -> QRes<usize> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM quick_actions WHERE type_id = ?1", [type_id], |r| r.get(0))?;
    Ok(n as usize)
}

pub fn create_quick_action(conn: &Connection, n: &NewQuickAction) -> QRes<i64> {
    let owner = editable_type(conn, n.type_id)?;
    if let Some(k) = &n.key {
        validate_ident(k)?;
    }
    let name = match &n.name {
        Some(s) if !s.trim().is_empty() => Some(clean_name(s)?),
        _ => None,
    };
    if n.key.is_none() && name.is_none() {
        return Err(PlanError::NameEmpty.into());
    }
    let params = validate_with_owner(conn, &owner, &n.params)?;
    atomic(conn, || {
        if count_for_type(conn, n.type_id)? >= MAX_ACTIONS_PER_TYPE {
            return Err(QuickError::TooMany { max: MAX_ACTIONS_PER_TYPE });
        }
        ensure_name_free(conn, "quick_actions", "type_id = ?1", &[n.type_id], n.key.as_deref(), name.as_deref(), None)?;
        let pos = common::next_position(conn, "quick_actions", "type_id = ?1", &[n.type_id])?;
        conn.execute(
            "INSERT INTO quick_actions (type_id, key, name, action, params, position, invalid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![n.type_id, n.key, name, params.kind().as_str(), params.to_json(), pos],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_quick_action(conn: &Connection, id: i64, u: &UpdateQuickAction) -> QRes<()> {
    let cur = get_quick_action(conn, id)?;
    let owner = editable_type(conn, cur.type_id)?;
    let name = match &u.name {
        Some(s) => rename_value(cur.key.as_deref(), s)?,
        None => cur.name.clone(),
    };
    let new_params = match &u.params {
        Some(p) => {
            if p.kind() != cur.kind() {
                return Err(QuickError::WrongKind);
            }
            Some(validate_with_owner(conn, &owner, p)?)
        }
        None => None,
    };
    atomic(conn, || {
        ensure_name_free(conn, "quick_actions", "type_id = ?1", &[cur.type_id], cur.key.as_deref(), name.as_deref(), Some(id))?;
        conn.execute("UPDATE quick_actions SET name = ?1 WHERE id = ?2", params![name, id])?;
        // Die Parameter werden nur angefasst, wenn neue kommen (sonst bleibt auch ein nicht lesbarer Bestand unverändert).
        if let Some(p) = &new_params {
            conn.execute("UPDATE quick_actions SET params = ?1, invalid = 0 WHERE id = ?2", params![p.to_json(), id])?;
        }
        Ok(())
    })
}

pub fn delete_quick_action(conn: &Connection, id: i64) -> QRes<()> {
    let cur = get_quick_action(conn, id)?;
    editable_type(conn, cur.type_id)?;
    conn.execute("DELETE FROM quick_actions WHERE id = ?1", [id])?;
    Ok(())
}

/// Neue Reihenfolge der Aktionen eines Typs; `ids` muss genau die vorhandenen enthalten.
pub fn reorder_quick_actions(conn: &Connection, type_id: i64, ids: &[i64]) -> QRes<()> {
    editable_type(conn, type_id)?;
    Ok(common::reorder(conn, "quick_actions", "type_id = ?1", &[type_id], ids)?)
}

/// Dupliziert die Aktion direkt hinter sich. Der Name bekommt den Zusatz „(Kopie)“ bzw. „(copy)“ (bei Bedarf mit Zähler), ein
/// Standard-Schlüssel wird nicht mitkopiert. Auch eine ungültige Aktion lässt sich kopieren (die Kopie bleibt ungültig).
pub fn duplicate_quick_action(conn: &Connection, id: i64, lang: Lang) -> QRes<i64> {
    let cur = get_quick_action(conn, id)?;
    editable_type(conn, cur.type_id)?;
    let raw: (String, String, i64) = conn.query_row("SELECT action, params, invalid FROM quick_actions WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let suffix = lang.t("Kopie", "copy");
    let base: String = cur.display(lang).chars().take(MAX_NAME_CHARS - 12).collect();
    atomic(conn, || {
        if count_for_type(conn, cur.type_id)? >= MAX_ACTIONS_PER_TYPE {
            return Err(QuickError::TooMany { max: MAX_ACTIONS_PER_TYPE });
        }
        let mut n = 1;
        let name = loop {
            let candidate = if n == 1 { format!("{base} ({suffix})") } else { format!("{base} ({suffix} {n})") };
            match ensure_name_free(conn, "quick_actions", "type_id = ?1", &[cur.type_id], None, Some(&candidate), None) {
                Ok(()) => break candidate,
                Err(PlanError::NameTaken) => n += 1,
                Err(e) => return Err(e.into()),
            }
        };
        let pos = common::next_position(conn, "quick_actions", "type_id = ?1", &[cur.type_id])?;
        conn.execute("INSERT INTO quick_actions (type_id, key, name, action, params, position, invalid) VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6)", params![cur.type_id, name, raw.0, raw.1, pos, raw.2])?;
        let new_id = conn.last_insert_rowid();
        // direkt hinter das Original schieben
        let mut order: Vec<i64> = list_quick_actions(conn, cur.type_id)?.iter().map(|a| a.id).filter(|i| *i != new_id).collect();
        let at = order.iter().position(|i| *i == id).map(|p| p + 1).unwrap_or(order.len());
        order.insert(at, new_id);
        common::reorder(conn, "quick_actions", "type_id = ?1", &[cur.type_id], &order)?;
        Ok(new_id)
    })
}

// ------------------------------------------------------------------ Aufräumen

/// Welche Aktionen [`prune_quick_action_refs`] durchsieht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneScope {
    /// Nur die Aktionen dieses Typs (nach Änderungen an seinen Feldern, Status oder seinem Workflow).
    Type(i64),
    /// Alle Aktionen aller Typen des Projekts (nach dem Löschen eines Typs: andere Aktionen können ihn als Ziel haben).
    Project(i64),
    /// Alle Aktionen (nach dem Löschen eines Verknüpfungstyps, der projektübergreifend gilt).
    All,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PruneReport {
    /// Durchgesehene Aktionen.
    pub checked: usize,
    /// Aktionen, deren Parameter geändert wurden.
    pub repaired: usize,
    pub removed_defaults: usize,
    pub removed_set_values: usize,
    pub removed_relations: usize,
    /// Neu als ungültig markiert / wieder gültig geworden.
    pub invalidated: usize,
    pub revalidated: usize,
}

/// Was beim Reparieren entfernt wurde.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Repair {
    pub defaults: usize,
    pub set_values: usize,
    pub relations: usize,
}

impl Repair {
    pub fn any(&self) -> bool {
        self.defaults + self.set_values + self.relations > 0
    }
}

/// Entfernt aus den Parametern, was sich entfernen lässt, ohne die Aktion zu verändern (Standardwerte und gesetzte Werte
/// ohne Feld oder mit nicht mehr passendem Wert, eine Verknüpfung ohne Verknüpfungstyp, doppelte Einträge). Ziel-Typ und
/// Ziel-Status bleiben stehen: fehlen sie, macht erst [`validate_params`] die Aktion ungültig.
pub fn repair_params(conn: &Connection, owner: &TicketType, p: &ActionParams) -> QRes<(ActionParams, Repair)> {
    let mut rep = Repair::default();
    let fixed = match p {
        ActionParams::CreateTicket(c) => {
            let mut c = c.clone();
            if let Some(r) = &c.relation {
                let ok = get_link_type(conn, r.link_type_id).is_ok_and(|l| l.target_kind == TargetKind::Ticket);
                if !ok {
                    c.relation = None;
                    rep.relations += 1;
                }
            }
            if target_type_of(conn, owner, c.ticket_type_id).is_ok() {
                let fields = list_fields(conn, c.ticket_type_id)?;
                let mut seen = HashSet::new();
                let mut kept = Vec::new();
                for d in c.defaults.drain(..) {
                    if seen.contains(&d.field_id) || check_default(owner.id, &fields, &d).is_err() {
                        rep.defaults += 1;
                    } else {
                        seen.insert(d.field_id);
                        kept.push(d);
                    }
                }
                c.defaults = kept;
            }
            ActionParams::CreateTicket(c)
        }
        ActionParams::ChangeStatus(c) => {
            let mut c = c.clone();
            let fields = list_fields(conn, owner.id)?;
            let mut seen = HashSet::new();
            let mut kept = Vec::new();
            for s in c.set_values.drain(..) {
                if seen.contains(&s.field_id) || check_set_value(&fields, &s).is_err() {
                    rep.set_values += 1;
                } else {
                    seen.insert(s.field_id);
                    kept.push(s);
                }
            }
            c.set_values = kept;
            ActionParams::ChangeStatus(c)
        }
        ActionParams::StartSession(c) => ActionParams::StartSession(c.clone()),
    };
    Ok((fixed, rep))
}

/// Räumt verwaiste Verweise der Aktionen im Bereich auf (siehe Moduldoku): repariert, was sich entfernen lässt, und setzt
/// bzw. löscht die Markierung „ungültig“ je nach Ergebnis der Prüfung. Idempotent.
pub fn prune_quick_action_refs(conn: &Connection, scope: PruneScope) -> QRes<PruneReport> {
    let list = match scope {
        PruneScope::Type(t) => list_quick_actions(conn, t)?,
        PruneScope::Project(p) => list_project_quick_actions(conn, p)?,
        PruneScope::All => {
            let mut st = conn.prepare(&format!("SELECT {COLS} FROM quick_actions ORDER BY id"))?;
            st.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?
        }
    };
    atomic(conn, || {
        let mut rep = PruneReport::default();
        for a in list {
            rep.checked += 1;
            let owner = get_type(conn, a.type_id)?;
            let (fixed, r) = repair_params(conn, &owner, &a.params)?;
            let now_invalid = unreadable(conn, a.id)? || validate_with_owner(conn, &owner, &fixed).is_err();
            rep.removed_defaults += r.defaults;
            rep.removed_set_values += r.set_values;
            rep.removed_relations += r.relations;
            if fixed != a.params {
                rep.repaired += 1;
                conn.execute("UPDATE quick_actions SET params = ?1 WHERE id = ?2", params![fixed.to_json(), a.id])?;
            }
            if now_invalid != a.invalid {
                conn.execute("UPDATE quick_actions SET invalid = ?1 WHERE id = ?2", params![now_invalid as i64, a.id])?;
                if now_invalid {
                    rep.invalidated += 1;
                } else {
                    rep.revalidated += 1;
                }
            }
        }
        Ok(rep)
    })
}

/// Sind die gespeicherten Parameter der Aktion nicht lesbar (kaputtes JSON, neuere Version)? Dann bleibt sie ungültig.
fn unreadable(conn: &Connection, id: i64) -> QRes<bool> {
    let (action, json): (String, String) = conn.query_row("SELECT action, params FROM quick_actions WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(ActionKind::parse(&action).is_none_or(|k| ActionParams::from_json(k, &json).is_err()))
}

/// Alle Typen des Projekts mit ihren Aktionen (Hilfe für Typ-Editor und Menü).
pub fn actions_by_type(conn: &Connection, project_id: i64) -> QRes<Vec<(TicketType, Vec<QuickAction>)>> {
    let all = list_project_quick_actions(conn, project_id)?;
    Ok(list_types(conn, project_id)?
        .into_iter()
        .map(|t| {
            let mine = all.iter().filter(|a| a.type_id == t.id).cloned().collect();
            (t, mine)
        })
        .collect())
}

/// Wert-Hilfe für Tests und Oberfläche: der Standardwert einer Aktion für ein Feld.
pub fn default_of(p: &CreateTicketParams, field: FieldRef) -> Option<&Value> {
    p.defaults.iter().find(|d| d.field_id == field).map(|d| &d.value)
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use crate::planning::fields::{list_fields, Field};
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::statuses::list_statuses;
    use crate::planning::testutil::db;
    use crate::planning::types::list_types;

    /// Projekt mit den Standard-Inhalten (Epic, Userstory, Devtask, Subtask, Defect; To Do/In Progress/Done).
    pub struct Fx {
        pub c: Connection,
        pub project: i64,
        pub epic: i64,
        pub story: i64,
        pub dev: i64,
        pub sub: i64,
        pub defect: i64,
        pub todo: i64,
        pub progress: i64,
        pub done: i64,
    }

    pub fn fx() -> Fx {
        let c = db();
        let mut p = NewProject::new("PAN", "Paneldeck", "#112233");
        p.with_defaults = true;
        p.use_versions = true;
        let project = create_project(&c, &p).unwrap();
        let ty = list_types(&c, project).unwrap();
        let by = |k: &str| ty.iter().find(|t| t.key.as_deref() == Some(k)).unwrap().id;
        let st = list_statuses(&c, project).unwrap();
        Fx { project, epic: by("epic"), story: by("userstory"), dev: by("devtask"), sub: by("subtask"), defect: by("defect"), todo: st[0].id, progress: st[1].id, done: st[2].id, c }
    }

    impl Fx {
        pub fn field(&self, type_id: i64, key: &str) -> Field {
            list_fields(&self.c, type_id).unwrap().into_iter().find(|f| f.key.as_deref() == Some(key)).unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::planning::fields::{create_field, delete_field, update_field, FieldOptions, NewField, OptionValue, UpdateField};
    use crate::planning::link_types::{create_link_type, delete_link_type, link_type_by_key, NewLinkType};
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::statuses::delete_status;
    use crate::planning::types::{delete_type, list_types};
    use crate::planning::workflow::{set_workflow_statuses, SetValue};
    use serde_json::json;

    fn create_params(ty: i64) -> ActionParams {
        ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: ty, relation: None, defaults: Vec::new() })
    }

    fn with_defaults(ty: i64, defaults: Vec<(FieldRef, Value)>) -> ActionParams {
        ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: ty, relation: None, defaults: defaults.into_iter().map(|(f, v)| DefaultValue { field_id: f, value: v }).collect() })
    }

    fn status_params(to: i64) -> ActionParams {
        ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: to, set_values: Vec::new() })
    }

    fn mk(f: &Fx, ty: i64, name: &str, p: ActionParams) -> i64 {
        create_quick_action(&f.c, &NewQuickAction::new(ty, name, p)).unwrap()
    }

    #[test]
    fn migration_creates_table_and_cascades_with_the_type() {
        let f = fx();
        let cols: Vec<String> = f.c.prepare("PRAGMA table_info(quick_actions)").unwrap().query_map([], |r| r.get::<_, String>(1)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(cols, vec!["id", "type_id", "key", "name", "action", "params", "position", "invalid"]);
        let id = mk(&f, f.story, "Create Dev task", create_params(f.dev));
        assert!(get_quick_action(&f.c, id).is_ok());
        // Typ löschen (ohne Tickets) nimmt die Aktionen mit
        delete_type(&f.c, f.story, None).unwrap();
        assert_eq!(get_quick_action(&f.c, id), Err(QuickError::Plan(PlanError::NotFound)));
        let n: i64 = f.c.query_row("SELECT COUNT(*) FROM quick_actions", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn schema_rejects_bad_rows() {
        let f = fx();
        assert!(f.c.execute("INSERT INTO quick_actions (type_id, name, action) VALUES (?1, 'x', 'explode')", [f.story]).is_err());
        assert!(f.c.execute("INSERT INTO quick_actions (type_id, action) VALUES (?1, 'create_ticket')", [f.story]).is_err(), "weder Schlüssel noch Name");
        assert!(f.c.execute("INSERT INTO quick_actions (type_id, name, action) VALUES (99999, 'x', 'create_ticket')", []).is_err(), "Fremdschlüssel");
    }

    #[test]
    fn create_get_list_in_order() {
        let f = fx();
        let a = mk(&f, f.story, "Create Dev task", create_params(f.dev));
        let b = mk(&f, f.story, "Start work", status_params(f.progress));
        let c = mk(&f, f.story, "Open session", ActionParams::StartSession(StartSessionParams { claude: true, ..Default::default() }));
        let list = list_quick_actions(&f.c, f.story).unwrap();
        assert_eq!(list.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a, b, c]);
        assert_eq!(list.iter().map(|x| x.position).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(list[0].kind(), ActionKind::CreateTicket);
        assert_eq!(list[1].display(Lang::En), "Start work");
        assert!(list.iter().all(|x| !x.invalid));
        assert!(list_quick_actions(&f.c, f.dev).unwrap().is_empty(), "je Typ getrennt");
        assert_eq!(list_project_quick_actions(&f.c, f.project).unwrap().len(), 3);
    }

    #[test]
    fn names_are_cleaned_and_unique_per_type() {
        let f = fx();
        let id = mk(&f, f.story, "  Create   Dev  task ", create_params(f.dev));
        assert_eq!(get_quick_action(&f.c, id).unwrap().name.as_deref(), Some("Create Dev task"));
        let dup = create_quick_action(&f.c, &NewQuickAction::new(f.story, "create dev TASK", create_params(f.dev)));
        assert_eq!(dup, Err(QuickError::Plan(PlanError::NameTaken)));
        // anderer Typ darf denselben Namen haben
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.epic, "Create Dev task", create_params(f.dev))).is_ok());
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "  ", create_params(f.dev))), Err(QuickError::Plan(PlanError::NameEmpty)));
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, &"x".repeat(81), create_params(f.dev))), Err(QuickError::Plan(PlanError::NameTooLong { .. }))));
    }

    #[test]
    fn key_actions_translate_until_renamed() {
        let f = fx();
        let n = NewQuickAction { type_id: f.story, key: Some("todo".into()), name: None, params: status_params(f.todo) };
        let id = create_quick_action(&f.c, &n).unwrap();
        let a = get_quick_action(&f.c, id).unwrap();
        assert_eq!((a.display(Lang::De).as_str(), a.display(Lang::En).as_str()), ("Zu erledigen", "To Do"));
        update_quick_action(&f.c, id, &UpdateQuickAction { name: Some("Back to start".into()), ..Default::default() }).unwrap();
        let a = get_quick_action(&f.c, id).unwrap();
        assert_eq!((a.display(Lang::De).as_str(), a.key.as_deref()), ("Back to start", Some("todo")));
        update_quick_action(&f.c, id, &UpdateQuickAction { name: Some("".into()), ..Default::default() }).unwrap();
        assert_eq!(get_quick_action(&f.c, id).unwrap().display(Lang::De), "Zu erledigen");
        // doppelter Schlüssel im Typ
        assert!(create_quick_action(&f.c, &n).is_err());
        // ungültiger Schlüssel
        let bad = NewQuickAction { key: Some("Not Valid!".into()), ..n.clone() };
        assert_eq!(create_quick_action(&f.c, &bad), Err(QuickError::Plan(PlanError::KeyInvalid)));
    }

    #[test]
    fn system_types_and_unknown_types_have_no_actions() {
        let f = fx();
        let todo_project = crate::planning::projects::project_by_key(&f.c, "TODO").unwrap();
        let sys_type = list_types(&f.c, todo_project.id).unwrap()[0].id;
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(sys_type, "x", ActionParams::empty(ActionKind::StartSession))), Err(QuickError::Plan(PlanError::SystemProtected)));
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(99_999, "x", ActionParams::empty(ActionKind::StartSession))), Err(QuickError::Plan(PlanError::NotFound)));
    }

    #[test]
    fn reorder_needs_exactly_the_existing_actions() {
        let f = fx();
        let a = mk(&f, f.story, "A", create_params(f.dev));
        let b = mk(&f, f.story, "B", create_params(f.dev));
        let c = mk(&f, f.story, "C", create_params(f.dev));
        reorder_quick_actions(&f.c, f.story, &[c, a, b]).unwrap();
        assert_eq!(list_quick_actions(&f.c, f.story).unwrap().iter().map(|x| x.id).collect::<Vec<_>>(), vec![c, a, b]);
        assert_eq!(reorder_quick_actions(&f.c, f.story, &[a, b]), Err(QuickError::Plan(PlanError::ReorderMismatch)));
        assert_eq!(reorder_quick_actions(&f.c, f.story, &[a, a, b, c]), Err(QuickError::Plan(PlanError::ReorderMismatch)));
        // Aktion eines anderen Typs
        let other = mk(&f, f.epic, "O", create_params(f.dev));
        assert_eq!(reorder_quick_actions(&f.c, f.story, &[a, b, c, other]), Err(QuickError::Plan(PlanError::ReorderMismatch)));
    }

    #[test]
    fn duplicates_right_behind_the_original() {
        let f = fx();
        let a = mk(&f, f.story, "Create Dev task", with_defaults(f.dev, vec![(FieldRef::Title, json!("T"))]));
        let b = mk(&f, f.story, "Other", status_params(f.done));
        let copy = duplicate_quick_action(&f.c, a, Lang::En).unwrap();
        let again = duplicate_quick_action(&f.c, a, Lang::En).unwrap();
        let de = duplicate_quick_action(&f.c, b, Lang::De).unwrap();
        let list = list_quick_actions(&f.c, f.story).unwrap();
        assert_eq!(list.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a, again, copy, b, de]);
        assert_eq!(get_quick_action(&f.c, copy).unwrap().name.as_deref(), Some("Create Dev task (copy)"));
        assert_eq!(get_quick_action(&f.c, again).unwrap().name.as_deref(), Some("Create Dev task (copy 2)"));
        assert_eq!(get_quick_action(&f.c, de).unwrap().name.as_deref(), Some("Other (Kopie)"));
        assert_eq!(get_quick_action(&f.c, copy).unwrap().params, get_quick_action(&f.c, a).unwrap().params);
        assert_eq!(list.iter().map(|x| x.position).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn duplicate_drops_the_default_key() {
        let f = fx();
        let id = create_quick_action(&f.c, &NewQuickAction { type_id: f.story, key: Some("todo".into()), name: None, params: status_params(f.todo) }).unwrap();
        let copy = duplicate_quick_action(&f.c, id, Lang::En).unwrap();
        let c = get_quick_action(&f.c, copy).unwrap();
        assert_eq!((c.key, c.name.as_deref()), (None, Some("To Do (copy)")));
    }

    #[test]
    fn create_ticket_target_must_be_a_normal_type_of_the_same_project() {
        let f = fx();
        let mut other = NewProject::new("OTH", "Other", "#112233");
        other.with_defaults = true;
        let op = create_project(&f.c, &other).unwrap();
        let foreign = list_types(&f.c, op).unwrap()[0].id;
        let todo_project = crate::planning::projects::project_by_key(&f.c, "TODO").unwrap();
        let sys = list_types(&f.c, todo_project.id).unwrap()[0].id;
        for bad in [foreign, sys, 0, 99_999] {
            assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "x", create_params(bad))), Err(QuickError::TargetType), "{bad}");
        }
        // derselbe Typ als Ziel ist erlaubt (Unteraufgabe in Unteraufgabe …)
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "same", create_params(f.story))).is_ok());
    }

    #[test]
    fn relation_needs_a_ticket_link_type() {
        let f = fx();
        let blocks = link_type_by_key(&f.c, "blocks").unwrap();
        let relates = link_type_by_key(&f.c, "relates").unwrap();
        let terminal = link_type_by_key(&f.c, "terminal").unwrap();
        let mk_rel = |id: i64, d: LinkDirection| {
            ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: Some(super::super::params::RelationSpec { link_type_id: id, direction: d }), defaults: vec![] })
        };
        let id = mk(&f, f.story, "Blocking task", mk_rel(blocks.id, LinkDirection::In));
        let ActionParams::CreateTicket(c) = get_quick_action(&f.c, id).unwrap().params else { panic!() };
        assert_eq!(c.relation.unwrap().direction, LinkDirection::In);
        // symmetrisch: Richtung wird auf „Out“ normalisiert
        let id = mk(&f, f.story, "Related task", mk_rel(relates.id, LinkDirection::In));
        let ActionParams::CreateTicket(c) = get_quick_action(&f.c, id).unwrap().params else { panic!() };
        assert_eq!(c.relation.unwrap().direction, LinkDirection::Out);
        // Terminal-Verknüpfung und unbekannter Typ
        for bad in [terminal.id, 0, 99_999] {
            assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, &format!("bad{bad}"), mk_rel(bad, LinkDirection::Out))), Err(QuickError::Relation));
        }
        // URL-Verknüpfung (Beziehungspartner nicht Ticket)
        let url = create_link_type(&f.c, &NewLinkType { name_out: "see".into(), name_in: None, symmetric: false, target_kind: TargetKind::Url }).unwrap();
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "u", mk_rel(url, LinkDirection::Out))), Err(QuickError::Relation));
    }

    #[test]
    fn defaults_must_belong_to_the_target_type() {
        let f = fx();
        let desc = f.field(f.dev, "description");
        let steps = f.field(f.defect, "steps");
        let ok = with_defaults(f.dev, vec![(FieldRef::Title, json!("Work on {{source.key}}")), (FieldRef::Field(desc.id), json!("From {{source.title}}"))]);
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "ok", ok)).is_ok());
        let foreign = with_defaults(f.dev, vec![(FieldRef::Field(steps.id), json!("x"))]);
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "bad", foreign)), Err(QuickError::Field { field: FieldLabel::Id(steps.id), problem: FieldProblem::NotInType }));
        // Feld des Ausgangs-Typs (statt des Ziel-Typs) ist ebenfalls falsch
        let us_epic = f.field(f.story, "epic");
        let wrong = with_defaults(f.dev, vec![(FieldRef::Field(us_epic.id), json!("{{source}}"))]);
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "bad2", wrong)), Err(QuickError::Field { problem: FieldProblem::NotInType, .. })));
    }

    #[test]
    fn defaults_values_must_fit_their_fields() {
        let f = fx();
        let prio = f.field(f.dev, "priority");
        let sp = f.field(f.dev, "story_points");
        let try_it = |name: &str, field: &Field, v: Value| create_quick_action(&f.c, &NewQuickAction::new(f.story, name, with_defaults(f.dev, vec![(FieldRef::Field(field.id), v)])));
        assert!(try_it("a", &prio, json!(2)).is_ok());
        for (i, bad) in [json!(99), json!("High"), json!(null), json!([1])].into_iter().enumerate() {
            assert!(matches!(try_it(&format!("b{i}"), &prio, bad), Err(QuickError::Field { problem: FieldProblem::Value, .. })), "{i}");
        }
        assert!(try_it("c", &sp, json!(5)).is_ok());
        assert!(matches!(try_it("d", &sp, json!(-1)), Err(QuickError::Field { problem: FieldProblem::Value, .. })), "unter dem Minimum");
        assert!(matches!(try_it("e", &sp, json!("5")), Err(QuickError::Field { problem: FieldProblem::Value, .. })));
    }

    #[test]
    fn title_default_rules() {
        let f = fx();
        let t = |name: &str, v: Value| create_quick_action(&f.c, &NewQuickAction::new(f.story, name, with_defaults(f.dev, vec![(FieldRef::Title, v)])));
        assert!(t("a", json!("Plain")).is_ok());
        for (i, bad) in [json!(""), json!("   "), json!(5), json!(null), json!("x".repeat(256))].into_iter().enumerate() {
            assert_eq!(t(&format!("b{i}"), bad), Err(QuickError::Field { field: FieldLabel::Title, problem: FieldProblem::Value }), "{i}");
        }
        assert_eq!(t("c", json!("{{evil.thing}}")), Err(QuickError::Placeholder("evil.thing".into())));
    }

    #[test]
    fn duplicate_defaults_are_rejected() {
        let f = fx();
        let desc = f.field(f.dev, "description");
        let two_titles = with_defaults(f.dev, vec![(FieldRef::Title, json!("a")), (FieldRef::Title, json!("b"))]);
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "t", two_titles)), Err(QuickError::Field { field: FieldLabel::Title, problem: FieldProblem::Duplicate }));
        let two = with_defaults(f.dev, vec![(FieldRef::Field(desc.id), json!("a")), (FieldRef::Field(desc.id), json!("b"))]);
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "f", two)), Err(QuickError::Field { problem: FieldProblem::Duplicate, .. })));
    }

    #[test]
    fn reference_fields_have_restricted_defaults() {
        let f = fx();
        // Terminal-Feld: kein Standardwert
        let term = create_field(&f.c, &NewField::new(f.dev, "Session", FieldKind::Terminal)).unwrap();
        let p = with_defaults(f.dev, vec![(FieldRef::Field(term), json!("abc"))]);
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "t", p)), Err(QuickError::Field { problem: FieldProblem::NoDefault, .. })));
        // Ticket-Feld „Epic“ der Userstory (Ziel-Typ Epic): nur „{{source}}“, und nur von einem Epic aus
        let epic_field = f.field(f.story, "epic");
        let src = with_defaults(f.story, vec![(FieldRef::Field(epic_field.id), json!("{{source}}"))]);
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.epic, "Create story", src.clone())).is_ok());
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.dev, "Create story", src)), Err(QuickError::Field { problem: FieldProblem::Value, .. })), "ein Devtask ist kein erlaubter Epic-Typ");
        let fixed = with_defaults(f.story, vec![(FieldRef::Field(epic_field.id), json!(5))]);
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.epic, "Fixed", fixed)), Err(QuickError::Field { problem: FieldProblem::Value, .. })));
        // Version: positive Kennung
        let rv = f.field(f.story, "release_version");
        let v_ok = with_defaults(f.story, vec![(FieldRef::Field(rv.id), json!(3))]);
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.epic, "V", v_ok)).is_ok());
        for (i, bad) in [json!(0), json!(-2), json!("3"), json!(1.5)].into_iter().enumerate() {
            let p = with_defaults(f.story, vec![(FieldRef::Field(rv.id), bad)]);
            assert!(create_quick_action(&f.c, &NewQuickAction::new(f.epic, &format!("V{i}"), p)).is_err(), "{i}");
        }
    }

    #[test]
    fn text_defaults_with_placeholders_skip_the_length_check_but_not_the_name_check() {
        let f = fx();
        let mut nf = NewField::new(f.dev, "Short", FieldKind::Text);
        nf.options.max_length = Some(5);
        let short = create_field(&f.c, &nf).unwrap();
        let t = |name: &str, v: Value| create_quick_action(&f.c, &NewQuickAction::new(f.story, name, with_defaults(f.dev, vec![(FieldRef::Field(short), v)])));
        assert!(t("a", json!("12345")).is_ok());
        assert!(t("b", json!("123456")).is_err(), "ohne Platzhalter gilt die Länge");
        assert!(t("c", json!("{{source.title}}")).is_ok(), "mit Platzhalter wird erst beim Auflösen gekürzt");
        assert_eq!(t("d", json!("{{what}}")), Err(QuickError::Placeholder("what".into())));
    }

    #[test]
    fn labels_and_multi_defaults() {
        let f = fx();
        let labels = create_field(&f.c, &NewField::new(f.dev, "Tags", FieldKind::Labels)).unwrap();
        let mut multi = NewField::new(f.dev, "Areas", FieldKind::Multi);
        multi.options = FieldOptions { values: vec![OptionValue::new("UI"), OptionValue::new("DB")], ..Default::default() };
        let multi = create_field(&f.c, &multi).unwrap();
        let t = |name: &str, fid: i64, v: Value| create_quick_action(&f.c, &NewQuickAction::new(f.story, name, with_defaults(f.dev, vec![(FieldRef::Field(fid), v)])));
        assert!(t("a", labels, json!(["urgent", "{{project.key}}"])).is_ok());
        assert!(t("b", labels, json!(["a", "a"])).is_err(), "doppelt");
        assert!(t("c", labels, json!(["{{nope}}"])).is_err());
        assert!(t("d", labels, json!("urgent")).is_err());
        assert!(t("e", multi, json!([1, 2])).is_ok());
        assert!(t("f", multi, json!([1, 9])).is_err());
    }

    #[test]
    fn too_many_defaults_and_actions() {
        let f = fx();
        let many = with_defaults(f.dev, (0..=MAX_ENTRIES).map(|i| (FieldRef::Field(i as i64 + 1000), json!("x"))).collect());
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "many", many)), Err(QuickError::TooMany { max: MAX_ENTRIES }));
        for i in 0..MAX_ACTIONS_PER_TYPE {
            mk(&f, f.story, &format!("A{i}"), create_params(f.dev));
        }
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "one more", create_params(f.dev))), Err(QuickError::TooMany { max: MAX_ACTIONS_PER_TYPE }));
        let first = list_quick_actions(&f.c, f.story).unwrap()[0].id;
        assert_eq!(duplicate_quick_action(&f.c, first, Lang::En), Err(QuickError::TooMany { max: MAX_ACTIONS_PER_TYPE }));
    }

    #[test]
    fn change_status_must_target_a_status_of_the_workflow() {
        let f = fx();
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "Done", status_params(f.done))).is_ok());
        set_workflow_statuses(&f.c, f.story, &[f.todo, f.done]).unwrap();
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "Start", status_params(f.progress))), Err(QuickError::Plan(PlanError::StatusNotInWorkflow)));
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "None", status_params(99_999))), Err(QuickError::Plan(PlanError::StatusNotInWorkflow)));
        // Typ ohne Workflow
        let t = crate::planning::types::create_type(&f.c, &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Note".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(t, "Done", status_params(f.done))), Err(QuickError::Plan(PlanError::WorkflowOff)));
    }

    #[test]
    fn change_status_set_values_follow_the_types_fields() {
        let f = fx();
        let prio = f.field(f.story, "priority");
        let sp = f.field(f.story, "story_points");
        let foreign = f.field(f.defect, "steps");
        let mk_sv = |v: Vec<(i64, Value)>| ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.done, set_values: v.into_iter().map(|(field_id, value)| SetValue { field_id, value }).collect() });
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "ok", mk_sv(vec![(prio.id, json!(1)), (sp.id, json!(3))]))).is_ok());
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "x1", mk_sv(vec![(foreign.id, json!("x"))]))), Err(QuickError::Field { problem: FieldProblem::NotInType, .. })));
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "x2", mk_sv(vec![(prio.id, json!(77))]))), Err(QuickError::Field { problem: FieldProblem::Value, .. })));
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "x3", mk_sv(vec![(prio.id, json!(1)), (prio.id, json!(2))]))), Err(QuickError::Field { problem: FieldProblem::Duplicate, .. })));
        assert!(matches!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "x4", mk_sv(vec![(sp.id, json!(null))]))), Err(QuickError::Field { problem: FieldProblem::Value, .. })));
    }

    #[test]
    fn start_session_rules() {
        let f = fx();
        let s = |shell: Option<&str>, claude: bool, prompt: bool| {
            ActionParams::StartSession(StartSessionParams { shell: shell.map(str::to_string), claude, prompt_from_ticket: prompt, cwd_from_project: true })
        };
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "a", s(None, false, false))).is_ok());
        let id = mk(&f, f.story, "b", s(Some("  pwsh "), true, true));
        assert_eq!(get_quick_action(&f.c, id).unwrap().params, s(Some("pwsh"), true, true), "Shell-Kennung wird bereinigt");
        assert!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "c", s(Some("wsl:Ubuntu 22.04"), false, false))).is_ok());
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "d", s(None, false, true))), Err(QuickError::PromptNeedsClaude));
        for (i, bad) in ["", "  ", "pw\nsh", "a\u{0000}b"].into_iter().enumerate() {
            assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, &format!("e{i}"), s(Some(bad), false, false))), Err(QuickError::Shell), "{i}");
        }
        assert_eq!(create_quick_action(&f.c, &NewQuickAction::new(f.story, "f", s(Some(&"s".repeat(101)), false, false))), Err(QuickError::Shell));
    }

    #[test]
    fn update_keeps_or_replaces_params() {
        let f = fx();
        let id = mk(&f, f.story, "Create", create_params(f.dev));
        // nur umbenennen: Parameter bleiben
        update_quick_action(&f.c, id, &UpdateQuickAction { name: Some("Create task".into()), ..Default::default() }).unwrap();
        let a = get_quick_action(&f.c, id).unwrap();
        assert_eq!((a.name.as_deref(), a.params.clone()), (Some("Create task"), create_params(f.dev)));
        // neue Parameter
        let np = with_defaults(f.sub, vec![(FieldRef::Title, json!("Sub"))]);
        update_quick_action(&f.c, id, &UpdateQuickAction { params: Some(np.clone()), ..Default::default() }).unwrap();
        assert_eq!(get_quick_action(&f.c, id).unwrap().params, np);
        // falsche Art, ungültige Parameter, doppelter Name: nichts ändert sich
        assert_eq!(update_quick_action(&f.c, id, &UpdateQuickAction { params: Some(status_params(f.done)), ..Default::default() }), Err(QuickError::WrongKind));
        assert_eq!(update_quick_action(&f.c, id, &UpdateQuickAction { params: Some(create_params(99_999)), ..Default::default() }), Err(QuickError::TargetType));
        let other = mk(&f, f.story, "Other", create_params(f.dev));
        assert_eq!(update_quick_action(&f.c, id, &UpdateQuickAction { name: Some("other".into()), ..Default::default() }), Err(QuickError::Plan(PlanError::NameTaken)));
        assert_eq!(update_quick_action(&f.c, 99_999, &UpdateQuickAction::default()), Err(QuickError::Plan(PlanError::NotFound)));
        assert_eq!(get_quick_action(&f.c, id).unwrap().name.as_deref(), Some("Create task"));
        assert_eq!(get_quick_action(&f.c, other).unwrap().name.as_deref(), Some("Other"));
        // Umbenennen ohne Schlüssel auf leer geht nicht
        assert_eq!(update_quick_action(&f.c, id, &UpdateQuickAction { name: Some(" ".into()), ..Default::default() }), Err(QuickError::Plan(PlanError::NameEmpty)));
    }

    #[test]
    fn delete_removes_only_that_action() {
        let f = fx();
        let a = mk(&f, f.story, "A", create_params(f.dev));
        let b = mk(&f, f.story, "B", create_params(f.dev));
        delete_quick_action(&f.c, a).unwrap();
        assert_eq!(list_quick_actions(&f.c, f.story).unwrap().iter().map(|x| x.id).collect::<Vec<_>>(), vec![b]);
        assert_eq!(delete_quick_action(&f.c, a), Err(QuickError::Plan(PlanError::NotFound)));
        // nach dem Löschen bleibt die Position des nächsten Eintrags am Ende
        let c = mk(&f, f.story, "C", create_params(f.dev));
        assert_eq!(get_quick_action(&f.c, c).unwrap().position, 2);
    }

    #[test]
    fn unreadable_params_make_the_action_invalid_but_keep_the_list() {
        let f = fx();
        let good = mk(&f, f.story, "Good", create_params(f.dev));
        f.c.execute("INSERT INTO quick_actions (type_id, name, action, params, position) VALUES (?1, 'Broken', 'create_ticket', 'not json', 5)", [f.story]).unwrap();
        f.c.execute("INSERT INTO quick_actions (type_id, name, action, params, position) VALUES (?1, 'Future', 'start_session', '{\"version\": 9}', 6)", [f.story]).unwrap();
        let list = list_quick_actions(&f.c, f.story).unwrap();
        assert_eq!(list.len(), 3);
        assert!(!list[0].invalid && list[0].id == good);
        assert!(list[1].invalid && list[2].invalid);
        // aufräumen lässt sie ungültig und schreibt ihre Parameter nicht um
        prune_quick_action_refs(&f.c, PruneScope::Project(f.project)).unwrap();
        let raw: String = f.c.query_row("SELECT params FROM quick_actions WHERE name = 'Broken'", [], |r| r.get(0)).unwrap();
        assert_eq!(raw, "not json");
        assert!(list_quick_actions(&f.c, f.story).unwrap()[2].invalid);
    }

    #[test]
    fn prune_removes_defaults_of_deleted_fields() {
        let f = fx();
        let desc = f.field(f.dev, "description");
        let sp = f.field(f.dev, "story_points");
        let id = mk(&f, f.story, "Create", with_defaults(f.dev, vec![(FieldRef::Title, json!("T")), (FieldRef::Field(desc.id), json!("D")), (FieldRef::Field(sp.id), json!(3))]));
        delete_field(&f.c, sp.id, None).unwrap();
        let rep = prune_quick_action_refs(&f.c, PruneScope::Type(f.story)).unwrap();
        assert_eq!((rep.checked, rep.repaired, rep.removed_defaults, rep.invalidated), (1, 1, 1, 0));
        let a = get_quick_action(&f.c, id).unwrap();
        assert!(!a.invalid);
        let ActionParams::CreateTicket(c) = a.params else { panic!() };
        assert_eq!(c.defaults.len(), 2);
        // idempotent
        assert_eq!(prune_quick_action_refs(&f.c, PruneScope::Type(f.story)).unwrap(), PruneReport { checked: 1, ..Default::default() });
    }

    #[test]
    fn prune_drops_defaults_whose_value_no_longer_fits() {
        let f = fx();
        let prio = f.field(f.dev, "priority");
        let id = mk(&f, f.story, "Create", with_defaults(f.dev, vec![(FieldRef::Field(prio.id), json!(5))]));
        // Wert „Lowest“ (Kennung 5) aus dem Dropdown entfernen (nicht in Benutzung)
        let mut o = prio.options.clone();
        o.values.retain(|v| v.id != 5);
        update_field(&f.c, prio.id, &UpdateField { options: Some(o), ..Default::default() }).unwrap();
        let rep = prune_quick_action_refs(&f.c, PruneScope::Project(f.project)).unwrap();
        assert_eq!(rep.removed_defaults, 1);
        let ActionParams::CreateTicket(c) = get_quick_action(&f.c, id).unwrap().params else { panic!() };
        assert!(c.defaults.is_empty());
    }

    #[test]
    fn prune_marks_actions_with_a_deleted_target_type_invalid_and_back() {
        let f = fx();
        let id = mk(&f, f.story, "Create sub", create_params(f.sub));
        let other = mk(&f, f.story, "Create dev", create_params(f.dev));
        delete_type(&f.c, f.sub, None).unwrap();
        // Typ-Bereich sieht die Aktion des anderen Typs nicht; Projekt-Bereich schon
        assert_eq!(prune_quick_action_refs(&f.c, PruneScope::Type(f.epic)).unwrap().invalidated, 0);
        let rep = prune_quick_action_refs(&f.c, PruneScope::Project(f.project)).unwrap();
        assert_eq!((rep.checked, rep.invalidated), (2, 1));
        assert!(get_quick_action(&f.c, id).unwrap().invalid);
        assert!(!get_quick_action(&f.c, other).unwrap().invalid);
        assert_eq!(check_quick_action(&f.c, id), Err(QuickError::TargetType));
        // gültige Parameter heben die Markierung auf
        update_quick_action(&f.c, id, &UpdateQuickAction { params: Some(create_params(f.dev)), ..Default::default() }).unwrap();
        assert!(!get_quick_action(&f.c, id).unwrap().invalid);
    }

    #[test]
    fn prune_marks_actions_with_a_missing_status_invalid_and_revalidates() {
        let f = fx();
        let id = mk(&f, f.story, "Start", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.progress, set_values: vec![SetValue { field_id: f.field(f.story, "priority").id, value: json!(1) }] }));
        set_workflow_statuses(&f.c, f.story, &[f.todo, f.done]).unwrap();
        let rep = prune_quick_action_refs(&f.c, PruneScope::Type(f.story)).unwrap();
        assert_eq!((rep.invalidated, rep.revalidated), (1, 0));
        assert!(get_quick_action(&f.c, id).unwrap().invalid);
        assert_eq!(check_quick_action(&f.c, id), Err(QuickError::Plan(PlanError::StatusNotInWorkflow)));
        // Status wieder in den Workflow: das Aufräumen stellt die Aktion wieder her
        set_workflow_statuses(&f.c, f.story, &[f.todo, f.progress, f.done]).unwrap();
        let rep = prune_quick_action_refs(&f.c, PruneScope::All).unwrap();
        assert_eq!((rep.invalidated, rep.revalidated), (0, 1));
        assert!(!get_quick_action(&f.c, id).unwrap().invalid);
    }

    #[test]
    fn prune_after_deleting_a_status_and_a_set_value_field() {
        let f = fx();
        let prio = f.field(f.story, "priority");
        let id = mk(&f, f.story, "Finish", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.done, set_values: vec![SetValue { field_id: prio.id, value: json!(1) }] }));
        let id2 = mk(&f, f.dev, "Finish", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.progress, set_values: vec![] }));
        delete_field(&f.c, prio.id, None).unwrap();
        delete_status(&f.c, f.progress, None).unwrap();
        let rep = prune_quick_action_refs(&f.c, PruneScope::Project(f.project)).unwrap();
        assert_eq!((rep.removed_set_values, rep.invalidated), (1, 1));
        assert!(!get_quick_action(&f.c, id).unwrap().invalid, "Ziel-Status Done existiert noch");
        assert!(get_quick_action(&f.c, id2).unwrap().invalid);
    }

    #[test]
    fn prune_removes_a_relation_to_a_deleted_link_type() {
        let f = fx();
        let custom = create_link_type(&f.c, &NewLinkType { name_out: "spawns".into(), name_in: Some("spawned by".into()), symmetric: false, target_kind: TargetKind::Ticket }).unwrap();
        let p = ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: Some(super::super::params::RelationSpec { link_type_id: custom, direction: LinkDirection::Out }), defaults: vec![] });
        let id = mk(&f, f.story, "Spawn", p);
        delete_link_type(&f.c, custom, None).unwrap();
        let rep = prune_quick_action_refs(&f.c, PruneScope::All).unwrap();
        assert_eq!((rep.removed_relations, rep.invalidated), (1, 0));
        assert_eq!(get_quick_action(&f.c, id).unwrap().params, create_params(f.dev));
    }

    #[test]
    fn prune_scopes_are_limited() {
        let f = fx();
        let desc_dev = f.field(f.dev, "description");
        let a = mk(&f, f.story, "A", with_defaults(f.dev, vec![(FieldRef::Field(desc_dev.id), json!("x"))]));
        let b = mk(&f, f.epic, "B", with_defaults(f.dev, vec![(FieldRef::Field(desc_dev.id), json!("x"))]));
        delete_field(&f.c, desc_dev.id, None).unwrap();
        // Typ-Bereich: nur die Aktionen dieses Typs
        prune_quick_action_refs(&f.c, PruneScope::Type(f.story)).unwrap();
        let has = |id: i64| match get_quick_action(&f.c, id).unwrap().params {
            ActionParams::CreateTicket(c) => !c.defaults.is_empty(),
            _ => false,
        };
        assert!(!has(a) && has(b));
        // anderes Projekt bleibt unberührt
        let mut o = NewProject::new("OTH", "O", "#112233");
        o.with_defaults = true;
        let op = create_project(&f.c, &o).unwrap();
        assert_eq!(prune_quick_action_refs(&f.c, PruneScope::Project(op)).unwrap().checked, 0);
        assert_eq!(prune_quick_action_refs(&f.c, PruneScope::Project(f.project)).unwrap().repaired, 1);
        assert!(!has(b));
    }

    #[test]
    fn actions_by_type_groups_the_project() {
        let f = fx();
        mk(&f, f.story, "A", create_params(f.dev));
        mk(&f, f.story, "B", create_params(f.dev));
        mk(&f, f.dev, "C", status_params(f.done));
        let groups = actions_by_type(&f.c, f.project).unwrap();
        assert_eq!(groups.len(), 5);
        let counts: Vec<usize> = groups.iter().map(|(_, a)| a.len()).collect();
        assert_eq!(counts, vec![0, 2, 1, 0, 0]);
    }

    #[test]
    fn default_of_finds_values() {
        let c = CreateTicketParams { ticket_type_id: 1, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Title, value: json!("x") }] };
        assert_eq!(default_of(&c, FieldRef::Title), Some(&json!("x")));
        assert_eq!(default_of(&c, FieldRef::Field(1)), None);
    }
}
