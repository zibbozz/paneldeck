//! Tickets: Anlegen, Ändern, Löschen, Markierung/Favorit, Eltern/Kinder mit Fortschritt und Reihenfolge (Backlog-Rang).
//! Reine Funktionen auf `&Connection` ohne Oberfläche.
//!
//! * Der Schlüssel `<KEY>-<n>` kommt aus `projects.next_number` (atomar, nie wiederverwendet).
//! * Jede schreibende Funktion nimmt eine [`Source`] und schreibt Verlaufseinträge mit gemeinsamer `batch`-Nummer; die
//!   meisten liefern eine [`TicketOp`], die sich mit `undo::History` rückgängig machen und wiederholen lässt (`ops.rs`).
//! * Die „rohen“ Schreibfunktionen (`raw_*`) prüfen nichts und gehören dem Rückgängig-Modul sowie den Abläufen, die ihre
//!   Eingaben vorher selbst geprüft haben.
//! * Statuswechsel mit Regeln: `transition.rs`; Klonen, Typ ändern, Verschieben: `clone_move.rs`; Listen und Suche:
//!   `ticket_list.rs`.

use super::fields::{list_fields, Field};
use super::history::{self, json_text, HistoryField, Source};
use super::ops::{FieldChange, MarkKind, TicketOp, TicketSnapshot};
use super::projects::get_project;
use super::statuses::{get_status, StatusCategory};
use super::ticket_error::{atomic, FieldLabel, TRes, TicketError};
use super::types::{get_type, may_be_child};
use super::values::{self, normalize_value, RefCtx};
use super::workflow::default_status;
use super::PlanError;
use crate::calc::progress::{parent_progress, Basis, Progress};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;

/// Längster Titel in Zeichen.
pub const MAX_TITLE_CHARS: usize = 255;

/// Schlüssel des Zahlenfelds, das für den Fortschritt (Story Points) gelesen wird.
pub const POINTS_FIELD_KEY: &str = "story_points";

#[derive(Debug, Clone, PartialEq)]
pub struct Ticket {
    pub id: i64,
    pub project_id: i64,
    /// Schlüssel des Projekts (z. B. `PAN`).
    pub project_key: String,
    pub number: i64,
    pub type_id: i64,
    pub title: String,
    pub status_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub rank: i64,
    pub flagged: bool,
    pub starred: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Archiviert (Wartung, ab Schema 10): aus Listen, Boards und Suche ausgeblendet, jederzeit wiederherstellbar.
    pub archived: bool,
}

impl Ticket {
    /// Anzeigeschlüssel, z. B. `PAN-423`.
    pub fn key(&self) -> String {
        format!("{}-{}", self.project_key, self.number)
    }
}

pub(crate) const COLS: &str = "t.id, t.project_id, p.key, t.number, t.type_id, t.title, t.status_id, t.parent_id, t.rank, t.flagged, t.starred, t.created_at, t.updated_at, t.archived";
pub(crate) const FROM: &str = "FROM tickets t JOIN projects p ON p.id = t.project_id";

pub(crate) fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Ticket> {
    Ok(Ticket {
        id: r.get(0)?,
        project_id: r.get(1)?,
        project_key: r.get(2)?,
        number: r.get(3)?,
        type_id: r.get(4)?,
        title: r.get(5)?,
        status_id: r.get(6)?,
        parent_id: r.get(7)?,
        rank: r.get(8)?,
        flagged: r.get::<_, i64>(9)? != 0,
        starred: r.get::<_, i64>(10)? != 0,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        archived: r.get::<_, i64>(13)? != 0,
    })
}

pub fn get_ticket(conn: &Connection, id: i64) -> TRes<Ticket> {
    conn.query_row(&format!("SELECT {COLS} {FROM} WHERE t.id = ?1"), [id], from_row).optional()?.ok_or_else(TicketError::not_found)
}

/// Zerlegt einen Schlüssel wie `PAN-423` (Groß-/Kleinschreibung egal) in Projektschlüssel und Nummer.
pub fn parse_key(key: &str) -> Option<(String, i64)> {
    let (k, n) = key.trim().rsplit_once('-')?;
    let n: i64 = n.trim().parse().ok().filter(|n| *n > 0)?;
    let k = k.trim().to_ascii_uppercase();
    if k.is_empty() { None } else { Some((k, n)) }
}

/// Ticket über seinen Schlüssel (`PAN-423`) finden.
pub fn ticket_by_key(conn: &Connection, key: &str) -> TRes<Ticket> {
    let (k, n) = parse_key(key).ok_or_else(TicketError::not_found)?;
    conn.query_row(&format!("SELECT {COLS} {FROM} WHERE p.key = ?1 AND t.number = ?2"), params![k, n], from_row).optional()?.ok_or_else(TicketError::not_found)
}

/// Titel bereinigen: Leerraum (auch Zeilenumbrüche) zusammenfassen; leer → `TitleEmpty`, zu lang → `TitleTooLong`.
pub fn clean_title(s: &str) -> TRes<String> {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return Err(TicketError::TitleEmpty);
    }
    if t.chars().count() > MAX_TITLE_CHARS {
        return Err(TicketError::TitleTooLong { max: MAX_TITLE_CHARS });
    }
    Ok(t)
}

pub(crate) fn touch(conn: &Connection, id: i64) -> TRes<()> {
    conn.execute("UPDATE tickets SET updated_at = ?1 WHERE id = ?2", params![super::common::now(), id])?;
    Ok(())
}

// ------------------------------------------------------------------ Hierarchie prüfen

/// Darf ein Ticket des Typs `child_type` (im Projekt `project_id`, ggf. mit der Kennung `child_id`) unter `parent` hängen?
pub(crate) fn check_parent(conn: &Connection, project_id: i64, child_type: i64, child_id: Option<i64>, parent: i64) -> TRes<()> {
    let p = get_ticket(conn, parent)?;
    if Some(p.id) == child_id {
        return Err(TicketError::ParentSelf);
    }
    if p.project_id != project_id {
        return Err(TicketError::ParentOtherProject);
    }
    if !may_be_child(conn, child_type, p.type_id)? {
        return Err(TicketError::ParentNotAllowed);
    }
    if let Some(me) = child_id {
        for a in ancestors(conn, p.id)? {
            if a == me {
                return Err(TicketError::ParentCycle);
            }
        }
    }
    Ok(())
}

/// Kennungen der Vorfahren (zuerst der direkte Elternteil), Kreise werden abgebrochen.
pub fn ancestors(conn: &Connection, id: i64) -> TRes<Vec<i64>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = get_ticket(conn, id)?.parent_id;
    while let Some(p) = cur {
        if !seen.insert(p) {
            break;
        }
        out.push(p);
        cur = conn.query_row("SELECT parent_id FROM tickets WHERE id = ?1", [p], |r| r.get(0)).optional()?.flatten();
    }
    Ok(out)
}

// ------------------------------------------------------------------ Anlegen

/// Eingaben für ein neues Ticket.
#[derive(Debug, Clone)]
pub struct NewTicket {
    pub project_id: i64,
    pub type_id: i64,
    pub title: String,
    pub parent_id: Option<i64>,
    /// Feldwerte (Feld-Id, Wert); `null` = bewusst leer (dann gilt auch kein Standardwert).
    pub values: Vec<(i64, Value)>,
}

impl NewTicket {
    pub fn new(project_id: i64, type_id: i64, title: &str) -> Self {
        NewTicket { project_id, type_id, title: title.to_string(), parent_id: None, values: Vec::new() }
    }
}

/// Legt ein Ticket an: Schlüssel aus dem Projekt-Zähler, Standardwerte der Felder, Pflichtfelder, Startstatus aus dem
/// Workflow (nur wenn der Typ einen hat), Hierarchie-Regel, Rang am Ende, Verlaufseintrag.
pub fn create_ticket(conn: &Connection, n: &NewTicket, source: &Source) -> TRes<(Ticket, TicketOp)> {
    create_ticket_noted(conn, n, source, None)
}

/// Wie [`create_ticket`], mit einer Anmerkung im „angelegt“-Eintrag des Verlaufs (z. B. `clone:PAN-3`).
pub(crate) fn create_ticket_noted(conn: &Connection, n: &NewTicket, source: &Source, note: Option<String>) -> TRes<(Ticket, TicketOp)> {
    atomic(conn, || {
        let project = get_project(conn, n.project_id)?;
        if project.system {
            return Err(PlanError::SystemProtected.into());
        }
        let ty = get_type(conn, n.type_id)?;
        if ty.system {
            return Err(PlanError::SystemProtected.into());
        }
        if ty.project_id != project.id {
            return Err(TicketError::TypeNotInProject);
        }
        let title = clean_title(&n.title)?;
        if let Some(p) = n.parent_id {
            check_parent(conn, project.id, ty.id, None, p)?;
        }
        let ctx = RefCtx { project_id: project.id, ticket_id: None };
        let fields = list_fields(conn, ty.id)?;
        let mut explicit: Vec<(i64, Option<Value>)> = Vec::new();
        for (fid, v) in &n.values {
            let f = fields.iter().find(|f| f.id == *fid).ok_or(TicketError::FieldNotInType { field_id: *fid })?;
            explicit.retain(|(i, _)| i != fid);
            explicit.push((*fid, normalize_value(conn, &ctx, f, v)?));
        }
        let mut final_values: Vec<(i64, Value)> = Vec::new();
        let mut missing: Vec<FieldLabel> = Vec::new();
        for f in &fields {
            let value = match explicit.iter().find(|(i, _)| *i == f.id) {
                Some((_, v)) => v.clone(),
                None => f.default_value.as_ref().and_then(|d| normalize_value(conn, &ctx, f, d).ok().flatten()),
            };
            match value {
                Some(v) => final_values.push((f.id, v)),
                None if f.required => missing.push(FieldLabel::of(f)),
                None => {}
            }
        }
        if !missing.is_empty() {
            return Err(TicketError::RequiredMissing(missing));
        }
        let status = default_status(conn, ty.id)?;
        let number = project.next_number;
        conn.execute("UPDATE projects SET next_number = ?1 WHERE id = ?2", [number + 1, project.id])?;
        let rank: i64 = conn.query_row("SELECT COALESCE(MAX(rank) + 1, 0) FROM tickets WHERE project_id = ?1", [project.id], |r| r.get(0))?;
        let now = super::common::now();
        conn.execute(
            "INSERT INTO tickets (project_id, number, type_id, title, status_id, parent_id, rank, flagged, starred, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8, ?8)",
            params![project.id, number, ty.id, title, status, n.parent_id, rank, now],
        )?;
        let id = conn.last_insert_rowid();
        for (fid, v) in &final_values {
            let f = fields.iter().find(|f| f.id == *fid).expect("Feld stammt aus der Liste");
            values::write_value(conn, id, f, Some(v))?;
        }
        let batch = history::next_batch(conn)?;
        history::log(conn, id, source, batch, &HistoryField::Created, None, note.as_deref())?;
        let ticket = get_ticket(conn, id)?;
        let op = TicketOp::Created { snapshot: TicketSnapshot { ticket: ticket.clone(), values: final_values }, present: true, source: source.clone() };
        Ok((ticket, op))
    })
}

/// Legt ein Kind unter `parent_id` an (Projekt und Hierarchie-Regel werden geprüft).
pub fn create_child(conn: &Connection, parent_id: i64, type_id: i64, title: &str, values: Vec<(i64, Value)>, source: &Source) -> TRes<(Ticket, TicketOp)> {
    let parent = get_ticket(conn, parent_id)?;
    create_ticket(conn, &NewTicket { project_id: parent.project_id, type_id, title: title.to_string(), parent_id: Some(parent_id), values }, source)
}

// ------------------------------------------------------------------ Rohe Schreibfunktionen

/// Titel setzen (bereinigt, nicht leer). `true`, wenn er sich geändert hat.
pub(crate) fn raw_set_title(conn: &Connection, id: i64, new: &str, source: &Source, batch: i64) -> TRes<Option<String>> {
    let new = clean_title(new)?;
    let old = get_ticket(conn, id)?.title;
    if old == new {
        return Ok(None);
    }
    conn.execute("UPDATE tickets SET title = ?1 WHERE id = ?2", params![new, id])?;
    history::log(conn, id, source, batch, &HistoryField::Title, Some(&old), Some(&new))?;
    touch(conn, id)?;
    Ok(Some(old))
}

/// Feldwerte setzen (bereits geprüft/normalisiert; `None` leert). Liefert nur die tatsächlichen Änderungen.
pub(crate) fn raw_apply_fields(conn: &Connection, id: i64, changes: &[(i64, Option<Value>)], source: &Source, batch: i64) -> TRes<Vec<FieldChange>> {
    let mut done = Vec::new();
    for (fid, new) in changes {
        let field: Field = super::fields::get_field(conn, *fid)?;
        let old = values::get_value(conn, id, &field)?;
        if old == *new {
            continue;
        }
        values::write_value(conn, id, &field, new.as_ref())?;
        history::log(conn, id, source, batch, &HistoryField::Custom(*fid), json_text(old.as_ref()).as_deref(), json_text(new.as_ref()).as_deref())?;
        done.push(FieldChange { field_id: *fid, before: old, after: new.clone() });
    }
    if !done.is_empty() {
        touch(conn, id)?;
    }
    Ok(done)
}

/// Status setzen (ohne Regeln). Liefert den alten Status, wenn er sich geändert hat.
pub(crate) fn raw_set_status(conn: &Connection, id: i64, new: Option<i64>, source: &Source, batch: i64) -> TRes<Option<Option<i64>>> {
    let old = get_ticket(conn, id)?.status_id;
    if old == new {
        return Ok(None);
    }
    conn.execute("UPDATE tickets SET status_id = ?1 WHERE id = ?2", params![new, id])?;
    history::log(conn, id, source, batch, &HistoryField::Status, old.map(|s| s.to_string()).as_deref(), new.map(|s| s.to_string()).as_deref())?;
    touch(conn, id)?;
    Ok(Some(old))
}

/// Eltern-Ticket setzen (ohne Regeln). Liefert das alte, wenn es sich geändert hat.
pub(crate) fn raw_set_parent(conn: &Connection, id: i64, new: Option<i64>, source: &Source, batch: i64) -> TRes<Option<Option<i64>>> {
    let old = get_ticket(conn, id)?.parent_id;
    if old == new {
        return Ok(None);
    }
    conn.execute("UPDATE tickets SET parent_id = ?1 WHERE id = ?2", params![new, id])?;
    history::log(conn, id, source, batch, &HistoryField::Parent, old.map(|s| s.to_string()).as_deref(), new.map(|s| s.to_string()).as_deref())?;
    touch(conn, id)?;
    Ok(Some(old))
}

/// Markierung oder Stern setzen. Liefert den alten Zustand, wenn er sich geändert hat.
pub(crate) fn raw_set_mark(conn: &Connection, id: i64, kind: MarkKind, on: bool, source: &Source, batch: i64) -> TRes<Option<bool>> {
    let t = get_ticket(conn, id)?;
    let old = match kind {
        MarkKind::Flag => t.flagged,
        MarkKind::Star => t.starred,
    };
    if old == on {
        return Ok(None);
    }
    let (col, field) = match kind {
        MarkKind::Flag => ("flagged", HistoryField::Flagged),
        MarkKind::Star => ("starred", HistoryField::Starred),
    };
    conn.execute(&format!("UPDATE tickets SET {col} = ?1 WHERE id = ?2"), params![on as i64, id])?;
    history::log(conn, id, source, batch, &field, Some(if old { "1" } else { "0" }), Some(if on { "1" } else { "0" }))?;
    touch(conn, id)?;
    Ok(Some(old))
}

pub(crate) fn raw_set_ranks(conn: &Connection, ranks: &[(i64, i64)]) -> TRes<()> {
    for (id, rank) in ranks {
        conn.execute("UPDATE tickets SET rank = ?1 WHERE id = ?2", [*rank, *id])?;
    }
    Ok(())
}

// ------------------------------------------------------------------ Ändern

/// Änderungen an Titel und Feldwerten eines Tickets.
#[derive(Debug, Clone, Default)]
pub struct TicketUpdate {
    pub title: Option<String>,
    /// (Feld-Id, neuer Wert); `null` leert das Feld (bei einem Pflichtfeld nicht erlaubt).
    pub values: Vec<(i64, Value)>,
}

/// Ändert Titel und Felder (nur Felder des Typs). Verlauf mit altem und neuem Wert je Feld, `updated_at` wird gesetzt.
/// `None` = nichts hat sich geändert.
pub fn update_ticket(conn: &Connection, id: i64, u: &TicketUpdate, source: &Source) -> TRes<Option<TicketOp>> {
    atomic(conn, || {
        let t = get_ticket(conn, id)?;
        let fields = list_fields(conn, t.type_id)?;
        let ctx = RefCtx { project_id: t.project_id, ticket_id: Some(id) };
        let title = match &u.title {
            Some(s) => Some(clean_title(s)?),
            None => None,
        };
        let mut changes: Vec<(i64, Option<Value>)> = Vec::new();
        let mut missing = Vec::new();
        for (fid, v) in &u.values {
            let f = fields.iter().find(|f| f.id == *fid).ok_or(TicketError::FieldNotInType { field_id: *fid })?;
            let new = normalize_value(conn, &ctx, f, v)?;
            if new.is_none() && f.required && values::get_value(conn, id, f)?.is_some() {
                missing.push(FieldLabel::of(f));
            }
            changes.retain(|(i, _)| i != fid);
            changes.push((*fid, new));
        }
        if !missing.is_empty() {
            return Err(TicketError::RequiredMissing(missing));
        }
        let batch = history::next_batch(conn)?;
        let mut ops = Vec::new();
        if let Some(title) = &title {
            if let Some(before) = raw_set_title(conn, id, title, source, batch)? {
                ops.push(TicketOp::Title { ticket: id, before, after: title.clone(), source: source.clone() });
            }
        }
        let done = raw_apply_fields(conn, id, &changes, source, batch)?;
        if !done.is_empty() {
            ops.push(TicketOp::Fields { ticket: id, changes: done, source: source.clone() });
        }
        Ok(TicketOp::combine(ops))
    })
}

/// Ein einzelnes Feld setzen (`null` leert).
pub fn set_field(conn: &Connection, id: i64, field_id: i64, value: Value, source: &Source) -> TRes<Option<TicketOp>> {
    update_ticket(conn, id, &TicketUpdate { title: None, values: vec![(field_id, value)] }, source)
}

/// Eltern-Ticket setzen oder entfernen (Hierarchie-Regeln, gleiches Projekt, kein Kreis).
pub fn set_parent(conn: &Connection, id: i64, parent: Option<i64>, source: &Source) -> TRes<Option<TicketOp>> {
    atomic(conn, || {
        let t = get_ticket(conn, id)?;
        if let Some(p) = parent {
            check_parent(conn, t.project_id, t.type_id, Some(id), p)?;
        }
        let batch = history::next_batch(conn)?;
        Ok(raw_set_parent(conn, id, parent, source, batch)?.map(|before| TicketOp::Parent { ticket: id, before, after: parent, source: source.clone() }))
    })
}

/// Markierung („Flag“/Impediment) setzen oder entfernen.
pub fn set_flagged(conn: &Connection, id: i64, on: bool, source: &Source) -> TRes<Option<TicketOp>> {
    set_mark(conn, id, MarkKind::Flag, on, source)
}

/// Favoriten-Stern setzen oder entfernen.
pub fn set_starred(conn: &Connection, id: i64, on: bool, source: &Source) -> TRes<Option<TicketOp>> {
    set_mark(conn, id, MarkKind::Star, on, source)
}

fn set_mark(conn: &Connection, id: i64, kind: MarkKind, on: bool, source: &Source) -> TRes<Option<TicketOp>> {
    atomic(conn, || {
        get_ticket(conn, id)?;
        let batch = history::next_batch(conn)?;
        Ok(raw_set_mark(conn, id, kind, on, source, batch)?.map(|before| TicketOp::Mark { ticket: id, kind, before, after: on, source: source.clone() }))
    })
}

// ------------------------------------------------------------------ Löschen

/// Was beim endgültigen Löschen eines Tickets mit verschwindet oder sich ändert (für die Warnung).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeleteImpact {
    /// Kinder: bleiben bestehen, verlieren aber ihr Eltern-Ticket.
    pub children: i64,
    /// Verknüpfungen von und zu diesem Ticket (werden gelöscht).
    pub links: i64,
    pub comments: i64,
    pub time_entries: i64,
    pub history: i64,
    /// Tickets, deren Ticket-Feld auf dieses verweist (der Wert wird geleert).
    pub referenced_by: i64,
}

impl DeleteImpact {
    /// Summe aller betroffenen Einträge.
    pub fn total(&self) -> i64 {
        self.children + self.links + self.comments + self.time_entries + self.history + self.referenced_by
    }
}

/// Ticket-Felder, die auf `?1` verweisen; mit `?2` (nicht NULL) nur von Tickets, die nicht in diesem Projekt liegen.
const REFS_SQL: &str = "SELECT v.ticket_id, v.field_id FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets rt ON rt.id = v.ticket_id
    WHERE f.kind = 'ticket' AND v.text = ?1 AND (?2 IS NULL OR rt.project_id <> ?2)";

pub fn delete_impact(conn: &Connection, id: i64) -> TRes<DeleteImpact> {
    get_ticket(conn, id)?;
    let n = |sql: &str| -> TRes<i64> { Ok(conn.query_row(sql, [id], |r| r.get(0))?) };
    Ok(DeleteImpact {
        children: n("SELECT COUNT(*) FROM tickets WHERE parent_id = ?1")?,
        links: n("SELECT COUNT(*) FROM links WHERE from_ticket_id = ?1 OR to_ticket_id = ?1")?,
        comments: n("SELECT COUNT(*) FROM comments WHERE ticket_id = ?1")?,
        time_entries: n("SELECT COUNT(*) FROM time_entries WHERE ticket_id = ?1")?,
        history: n("SELECT COUNT(*) FROM history WHERE ticket_id = ?1")?,
        referenced_by: conn.query_row(&format!("SELECT COUNT(*) FROM ({REFS_SQL})"), params![id.to_string(), None::<i64>], |r| r.get(0))?,
    })
}

/// Löscht ein Ticket endgültig. Kinder verlieren ihr Eltern-Ticket (mit Verlaufseintrag), Verknüpfungen, Kommentare,
/// Zeiteinträge und der Verlauf des Tickets werden mitgelöscht, Ticket-Felder anderer Tickets, die darauf verweisen,
/// geleert. Gibt zurück, was betroffen war.
pub fn delete_ticket(conn: &Connection, id: i64, source: &Source) -> TRes<DeleteImpact> {
    atomic(conn, || {
        let impact = delete_impact(conn, id)?;
        let batch = history::next_batch(conn)?;
        let children: Vec<i64> = {
            let mut st = conn.prepare("SELECT id FROM tickets WHERE parent_id = ?1 ORDER BY id")?;
            st.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for c in children {
            raw_set_parent(conn, c, None, source, batch)?;
        }
        clear_refs_to(conn, id, None, source, batch)?;
        conn.execute("DELETE FROM tickets WHERE id = ?1", [id])?;
        Ok(impact)
    })
}

/// Leert alle Ticket-Felder, die auf `target` verweisen (mit Verlauf bei den betroffenen Tickets); mit `other_than_project`
/// nur die von Tickets außerhalb dieses Projekts.
pub(crate) fn clear_refs_to(conn: &Connection, target: i64, other_than_project: Option<i64>, source: &Source, batch: i64) -> TRes<()> {
    let refs: Vec<(i64, i64)> = {
        let mut st = conn.prepare(REFS_SQL)?;
        st.query_map(params![target.to_string(), other_than_project], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    for (ticket, field) in refs {
        conn.execute("DELETE FROM ticket_values WHERE ticket_id = ?1 AND field_id = ?2", [ticket, field])?;
        history::log(conn, ticket, source, batch, &HistoryField::Custom(field), Some(&target.to_string()), None)?;
        touch(conn, ticket)?;
    }
    Ok(())
}

/// Vor dem Löschen eines Projekts: Ticket-Felder anderer Projekte, die auf dessen Tickets verweisen, werden geleert.
pub(crate) fn before_project_delete(conn: &Connection, project_id: i64) -> TRes<()> {
    conn.execute(
        "DELETE FROM ticket_values WHERE field_id IN (SELECT id FROM fields WHERE kind = 'ticket') AND text IN (SELECT CAST(id AS TEXT) FROM tickets WHERE project_id = ?1)",
        [project_id],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ Kinder und Fortschritt

/// Direkte Kinder in Rang-Reihenfolge.
pub fn children_of(conn: &Connection, id: i64) -> TRes<Vec<Ticket>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} {FROM} WHERE t.parent_id = ?1 ORDER BY t.rank, t.id"))?;
    Ok(st.query_map([id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Fortschritt aus den direkten Kindern: erledigt = Status der Kategorie „Done“; Punkte aus dem Zahlenfeld mit dem Schlüssel
/// [`POINTS_FIELD_KEY`] (fehlt es beim Kind, hat es keine Punkte). Gewichtung nach `basis` (`calc::progress`).
pub fn progress_of(conn: &Connection, id: i64, basis: Basis) -> TRes<Progress> {
    get_ticket(conn, id)?;
    let mut st = conn.prepare(
        "SELECT s.category, (SELECT v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE v.ticket_id = t.id AND f.key = ?2 AND f.kind = 'number' LIMIT 1)
         FROM tickets t LEFT JOIN statuses s ON s.id = t.status_id WHERE t.parent_id = ?1",
    )?;
    let rows = st
        .query_map(params![id, POINTS_FIELD_KEY], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<f64>>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let children: Vec<(bool, Option<f64>)> = rows.into_iter().map(|(cat, pts)| (cat.as_deref().map(StatusCategory::parse) == Some(StatusCategory::Done), pts)).collect();
    Ok(parent_progress(&children, basis))
}

/// Ist das Ticket erledigt (Status der Kategorie „Done“)? Ohne Status: nein.
pub fn is_done(conn: &Connection, t: &Ticket) -> TRes<bool> {
    match t.status_id {
        Some(s) => Ok(get_status(conn, s)?.category == StatusCategory::Done),
        None => Ok(false),
    }
}

// ------------------------------------------------------------------ Reihenfolge

/// Setzt das Ticket vor `before` (`None` = ans Ende) in der Backlog-Reihenfolge seines Projekts und nummeriert dicht neu.
/// `None` = die Reihenfolge ändert sich nicht. `before` muss im selben Projekt liegen.
pub fn move_rank(conn: &Connection, id: i64, before: Option<i64>) -> TRes<Option<TicketOp>> {
    atomic(conn, || {
        let t = get_ticket(conn, id)?;
        let old: Vec<(i64, i64)> = {
            let mut st = conn.prepare("SELECT id, rank FROM tickets WHERE project_id = ?1 ORDER BY rank, id")?;
            st.query_map([t.project_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        if before == Some(id) {
            return Ok(None);
        }
        let mut order: Vec<i64> = old.iter().map(|(i, _)| *i).filter(|i| *i != id).collect();
        let at = match before {
            Some(b) => order.iter().position(|x| *x == b).ok_or_else(TicketError::not_found)?,
            None => order.len(),
        };
        order.insert(at, id);
        if order.iter().copied().eq(old.iter().map(|(i, _)| *i)) {
            return Ok(None);
        }
        let mut changes = Vec::new();
        for (new_rank, tid) in order.iter().enumerate() {
            let old_rank = old.iter().find(|(i, _)| i == tid).map(|(_, r)| *r).unwrap_or(0);
            if old_rank != new_rank as i64 {
                changes.push((*tid, old_rank, new_rank as i64));
            }
        }
        raw_set_ranks(conn, &changes.iter().map(|(i, _, a)| (*i, *a)).collect::<Vec<_>>())?;
        Ok(Some(TicketOp::Rank { changes }))
    })
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Gemeinsame Testumgebung der Ticket-Schicht: ein Projekt „PAN“ mit den Standard-Typen und ein paar zusätzlichen Feldern.

    use super::*;
    use crate::planning::fields::{create_field, FieldKind, FieldOptions, NewField, OptionValue};
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::statuses::list_statuses;
    use crate::planning::types::list_types;

    pub struct Fx {
        pub c: Connection,
        pub project: i64,
        pub epic: i64,
        pub story: i64,
        pub task: i64,
        pub subtask: i64,
        pub defect: i64,
        pub todo: i64,
        pub progress: i64,
        pub done: i64,
    }

    impl Fx {
        pub fn new() -> Fx {
            let c = crate::db::open_memory();
            let mut np = NewProject::new("PAN", "Paneldeck", "#112233");
            np.with_defaults = true;
            let project = create_project(&c, &np).unwrap();
            let types = list_types(&c, project).unwrap();
            let t = |k: &str| types.iter().find(|t| t.key.as_deref() == Some(k)).unwrap().id;
            let st = list_statuses(&c, project).unwrap();
            let s = |k: &str| st.iter().find(|s| s.key.as_deref() == Some(k)).unwrap().id;
            Fx { project, epic: t("epic"), story: t("userstory"), task: t("devtask"), subtask: t("subtask"), defect: t("defect"), todo: s("todo"), progress: s("progress"), done: s("done"), c }
        }

        /// Feld des Typs über seinen Standard-Schlüssel.
        pub fn field(&self, type_id: i64, key: &str) -> Field {
            list_fields(&self.c, type_id).unwrap().into_iter().find(|f| f.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("Feld {key} fehlt"))
        }

        /// Eigenes Feld mit Namen am Typ anlegen.
        pub fn add_field(&self, type_id: i64, name: &str, kind: FieldKind, options: FieldOptions, required: bool) -> Field {
            let mut n = NewField::new(type_id, name, kind);
            n.options = options;
            n.required = required;
            let id = create_field(&self.c, &n).unwrap();
            crate::planning::fields::get_field(&self.c, id).unwrap()
        }

        pub fn dropdown(names: &[&str]) -> FieldOptions {
            FieldOptions { values: names.iter().map(|n| OptionValue::new(n)).collect(), ..Default::default() }
        }

        pub fn mk(&self, type_id: i64, title: &str) -> Ticket {
            create_ticket(&self.c, &NewTicket::new(self.project, type_id, title), &Source::ui()).unwrap().0
        }

        pub fn child(&self, parent: &Ticket, type_id: i64, title: &str) -> Ticket {
            create_child(&self.c, parent.id, type_id, title, vec![], &Source::ui()).unwrap().0
        }

        /// Zweites Projekt mit Standard-Inhalten.
        pub fn other_project(&self, key: &str) -> i64 {
            let mut np = NewProject::new(key, "Other", "#223344");
            np.with_defaults = true;
            create_project(&self.c, &np).unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::Fx;
    use super::*;
    use crate::planning::fields::{FieldKind, FieldOptions};
    use crate::planning::history::{history_of, SourceKind};
    use crate::state::Lang;
    use serde_json::json;

    fn ui() -> Source {
        Source::ui()
    }

    #[test]
    fn keys_are_per_project_sequential_and_never_reused() {
        let f = Fx::new();
        let a = f.mk(f.epic, "A");
        let b = f.mk(f.epic, "B");
        assert_eq!((a.key().as_str(), b.key().as_str()), ("PAN-1", "PAN-2"));
        delete_ticket(&f.c, b.id, &ui()).unwrap();
        let c = f.mk(f.epic, "C");
        assert_eq!(c.key(), "PAN-3", "Nummer 2 wird nie wiederverwendet");
        let other = f.other_project("OTH");
        let o = create_ticket(&f.c, &NewTicket::new(other, crate::planning::types::list_types(&f.c, other).unwrap()[0].id, "O"), &ui()).unwrap().0;
        assert_eq!(o.key(), "OTH-1", "eigener Zähler je Projekt");
        assert_eq!(ticket_by_key(&f.c, "pan-3").unwrap().id, c.id);
        assert!(ticket_by_key(&f.c, "PAN-2").unwrap_err().is_not_found());
        assert!(ticket_by_key(&f.c, "nonsense").unwrap_err().is_not_found());
        assert_eq!(parse_key(" pan-12 "), Some(("PAN".into(), 12)));
        assert_eq!(parse_key("PAN-0"), None);
        assert_eq!(parse_key("-5"), None);
    }

    #[test]
    fn title_is_cleaned_and_required() {
        let f = Fx::new();
        assert_eq!(f.mk(f.epic, "  a \n b  ").title, "a b");
        for bad in ["", "   ", "\n\t"] {
            assert_eq!(create_ticket(&f.c, &NewTicket::new(f.project, f.epic, bad), &ui()).unwrap_err(), TicketError::TitleEmpty);
        }
        assert_eq!(create_ticket(&f.c, &NewTicket::new(f.project, f.epic, &"x".repeat(256)), &ui()).unwrap_err(), TicketError::TitleTooLong { max: 255 });
        assert!(create_ticket(&f.c, &NewTicket::new(f.project, f.epic, &"x".repeat(255)), &ui()).is_ok());
        let n: i64 = f.c.query_row("SELECT next_number FROM projects WHERE id = ?1", [f.project], |r| r.get(0)).unwrap();
        assert_eq!(n, 3, "Fehlschläge verbrauchen keine Nummer");
    }

    #[test]
    fn type_must_belong_to_the_project_and_not_be_system() {
        let f = Fx::new();
        let other = f.other_project("OTH");
        let other_type = crate::planning::types::list_types(&f.c, other).unwrap()[0].id;
        assert_eq!(create_ticket(&f.c, &NewTicket::new(f.project, other_type, "x"), &ui()).unwrap_err(), TicketError::TypeNotInProject);
        // System-Projekt „Todolist“ ist nur lesbar
        let todo_project = crate::planning::projects::project_by_key(&f.c, "TODO").unwrap();
        let todo_type = crate::planning::types::list_types(&f.c, todo_project.id).unwrap()[0].id;
        assert_eq!(create_ticket(&f.c, &NewTicket::new(todo_project.id, todo_type, "x"), &ui()).unwrap_err(), TicketError::Plan(PlanError::SystemProtected));
        assert!(create_ticket(&f.c, &NewTicket::new(9999, f.epic, "x"), &ui()).unwrap_err().is_not_found());
        assert!(create_ticket(&f.c, &NewTicket::new(f.project, 9999, "x"), &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn defaults_workflow_start_status_and_rank() {
        let f = Fx::new();
        let a = f.mk(f.epic, "A");
        assert_eq!(a.status_id, Some(f.todo), "Startstatus aus dem Workflow");
        assert_eq!(a.rank, 0);
        let b = f.mk(f.epic, "B");
        assert_eq!(b.rank, 1, "Rang am Ende");
        let prio = f.field(f.epic, "priority");
        let v = values::get_value(&f.c, a.id, &prio).unwrap();
        assert_eq!(v, Some(json!(3)), "Standardwert Medium");
        // explizit leer = kein Standardwert
        let (c, _) = create_ticket(&f.c, &NewTicket { values: vec![(prio.id, json!(null))], ..NewTicket::new(f.project, f.epic, "C") }, &ui()).unwrap();
        assert_eq!(values::get_value(&f.c, c.id, &prio).unwrap(), None);
        // explizit gesetzt
        let (d, _) = create_ticket(&f.c, &NewTicket { values: vec![(prio.id, json!(1))], ..NewTicket::new(f.project, f.epic, "D") }, &ui()).unwrap();
        assert_eq!(values::get_value(&f.c, d.id, &prio).unwrap(), Some(json!(1)));
        // ein Typ ohne Workflow bekommt keinen Status
        let plain = crate::planning::types::create_type(
            &f.c,
            &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Notiz".into()), icon: None, color: "#445566".into(), workflow: false },
        )
        .unwrap();
        assert_eq!(f.mk(plain, "N").status_id, None);
    }

    #[test]
    fn fields_must_belong_to_the_type_and_values_are_checked() {
        let f = Fx::new();
        let sp = f.field(f.story, "story_points");
        let bug_steps = f.field(f.defect, "steps");
        let bad = |v: Vec<(i64, Value)>, t: i64| create_ticket(&f.c, &NewTicket { values: v, ..NewTicket::new(f.project, t, "x") }, &ui()).unwrap_err();
        assert_eq!(bad(vec![(bug_steps.id, json!("s"))], f.story), TicketError::FieldNotInType { field_id: bug_steps.id });
        assert!(matches!(bad(vec![(sp.id, json!(-3))], f.story), TicketError::ValueInvalid(l) if l.id == sp.id));
        assert!(matches!(bad(vec![(sp.id, json!("viel"))], f.story), TicketError::ValueInvalid(_)));
        let ok = create_ticket(&f.c, &NewTicket { values: vec![(sp.id, json!(5))], ..NewTicket::new(f.project, f.story, "ok") }, &ui()).unwrap().0;
        assert_eq!(values::get_value(&f.c, ok.id, &sp).unwrap(), Some(json!(5)));
        // ein misslungenes Anlegen hinterlässt nichts
        let n: i64 = f.c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn required_fields_are_enforced_on_create_and_update() {
        let f = Fx::new();
        let must = f.add_field(f.epic, "Ziel", FieldKind::Text, FieldOptions::default(), true);
        let err = create_ticket(&f.c, &NewTicket::new(f.project, f.epic, "A"), &ui()).unwrap_err();
        match &err {
            TicketError::RequiredMissing(v) => assert_eq!(v.iter().map(|l| l.id).collect::<Vec<_>>(), vec![must.id]),
            other => panic!("{other:?}"),
        }
        assert!(err.message(Lang::De).contains("Ziel"));
        let ok = create_ticket(&f.c, &NewTicket { values: vec![(must.id, json!("x"))], ..NewTicket::new(f.project, f.epic, "A") }, &ui()).unwrap().0;
        // Pflichtfeld lässt sich nicht leeren, ändern schon
        assert!(matches!(set_field(&f.c, ok.id, must.id, json!(null), &ui()), Err(TicketError::RequiredMissing(_))));
        assert!(matches!(set_field(&f.c, ok.id, must.id, json!("  "), &ui()), Err(TicketError::RequiredMissing(_))));
        assert!(set_field(&f.c, ok.id, must.id, json!("y"), &ui()).unwrap().is_some());
        // Titeländerung eines Tickets mit leerem Pflichtfeld (Altbestand) bleibt möglich
        let legacy = f.mk(f.story, "L");
        let later = f.add_field(f.story, "Später", FieldKind::Text, FieldOptions::default(), true);
        assert!(update_ticket(&f.c, legacy.id, &TicketUpdate { title: Some("L2".into()), values: vec![] }, &ui()).unwrap().is_some());
        assert_eq!(values::get_value(&f.c, legacy.id, &later).unwrap(), None);
    }

    #[test]
    fn hierarchy_rules_apply_on_create() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "E");
        let story = f.child(&epic, f.story, "S");
        assert_eq!(story.parent_id, Some(epic.id));
        // Epic unter Story: nicht erlaubt; Defect unter Subtask: nicht erlaubt
        let r = create_ticket(&f.c, &NewTicket { parent_id: Some(story.id), ..NewTicket::new(f.project, f.epic, "x") }, &ui());
        assert_eq!(r.unwrap_err(), TicketError::ParentNotAllowed);
        // Subtask direkt unter Epic: nicht erlaubt
        assert_eq!(create_child(&f.c, epic.id, f.subtask, "x", vec![], &ui()).unwrap_err(), TicketError::ParentNotAllowed);
        // anderes Projekt
        let other = f.other_project("OTH");
        let other_type = crate::planning::types::list_types(&f.c, other).unwrap().iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        let r = create_ticket(&f.c, &NewTicket { parent_id: Some(epic.id), ..NewTicket::new(other, other_type, "x") }, &ui());
        assert_eq!(r.unwrap_err(), TicketError::ParentOtherProject);
        assert!(create_child(&f.c, 9999, f.story, "x", vec![], &ui()).unwrap_err().is_not_found());
        // Typ ohne Eltern-Regeln darf kein Kind sein
        assert_eq!(create_child(&f.c, story.id, f.epic, "x", vec![], &ui()).unwrap_err(), TicketError::ParentNotAllowed);
    }

    #[test]
    fn set_parent_checks_rules_cycles_and_logs() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "E");
        let story = f.mk(f.story, "S");
        let task = f.mk(f.task, "T");
        assert!(set_parent(&f.c, story.id, Some(epic.id), &ui()).unwrap().is_some());
        assert!(set_parent(&f.c, story.id, Some(epic.id), &ui()).unwrap().is_none(), "unverändert");
        assert_eq!(set_parent(&f.c, story.id, Some(story.id), &ui()).unwrap_err(), TicketError::ParentSelf);
        assert_eq!(set_parent(&f.c, task.id, Some(task.id), &ui()).unwrap_err(), TicketError::ParentSelf);
        // Ein Epic darf laut Regeln unter keinem Typ hängen
        assert_eq!(set_parent(&f.c, epic.id, Some(story.id), &ui()).unwrap_err(), TicketError::ParentNotAllowed);
        assert!(set_parent(&f.c, story.id, None, &ui()).unwrap().is_some());
        assert_eq!(get_ticket(&f.c, story.id).unwrap().parent_id, None);
        let h = history_of(&f.c, story.id).unwrap();
        assert_eq!(h.iter().filter(|e| e.field == HistoryField::Parent).count(), 2);
    }

    #[test]
    fn parent_cycle_is_detected_with_a_self_nesting_free_rule_set() {
        // Die Regeln selbst lassen keine Kreise zu; ein Kreis in den Tickets ergibt sich nur, wenn eine Regel nachträglich
        // (hier: direkt in der Datenbank) so geändert wird, dass Epic unter Devtask hängen dürfte.
        let f = Fx::new();
        let epic = f.mk(f.epic, "E");
        let story = f.child(&epic, f.story, "S");
        let task = f.child(&story, f.task, "T");
        f.c.execute("INSERT INTO type_parents (type_id, parent_type_id) VALUES (?1, ?2)", [f.epic, f.task]).unwrap();
        assert_eq!(set_parent(&f.c, epic.id, Some(task.id), &ui()).unwrap_err(), TicketError::ParentCycle);
        assert_eq!(ancestors(&f.c, task.id).unwrap(), vec![story.id, epic.id]);
    }

    #[test]
    fn update_changes_title_and_fields_with_history() {
        let f = Fx::new();
        let t = f.mk(f.story, "Alt");
        let sp = f.field(f.story, "story_points");
        let prio = f.field(f.story, "priority");
        let before = get_ticket(&f.c, t.id).unwrap().updated_at;
        let op = update_ticket(&f.c, t.id, &TicketUpdate { title: Some(" Neu ".into()), values: vec![(sp.id, json!(8)), (prio.id, json!(1))] }, &Source::agent("S1")).unwrap();
        assert!(matches!(op, Some(TicketOp::Batch(_))));
        let now = get_ticket(&f.c, t.id).unwrap();
        assert_eq!(now.title, "Neu");
        assert!(now.updated_at >= before);
        let h = history_of(&f.c, t.id).unwrap();
        assert_eq!(h.len(), 4, "angelegt, Titel, 2 Felder");
        let title = h.iter().find(|e| e.field == HistoryField::Title).unwrap();
        assert_eq!((title.old.as_deref(), title.new.as_deref()), (Some("Alt"), Some("Neu")));
        assert_eq!(title.source.kind, SourceKind::Agent);
        assert_eq!(title.source.detail.as_deref(), Some("S1"));
        let sp_e = h.iter().find(|e| e.field == HistoryField::Custom(sp.id)).unwrap();
        assert_eq!((sp_e.old.as_deref(), sp_e.new.as_deref()), (None, Some("8")));
        let pr_e = h.iter().find(|e| e.field == HistoryField::Custom(prio.id)).unwrap();
        assert_eq!((pr_e.old.as_deref(), pr_e.new.as_deref()), (Some("3"), Some("1")));
        let batch: HashSet<i64> = h.iter().skip(1).map(|e| e.batch).collect();
        assert_eq!(batch.len(), 1, "alles gehört zu einer Aktion");
        assert_ne!(h[0].batch, h[1].batch);
        // nichts geändert
        assert!(update_ticket(&f.c, t.id, &TicketUpdate { title: Some("Neu".into()), values: vec![(sp.id, json!(8))] }, &ui()).unwrap().is_none());
        assert_eq!(history_of(&f.c, t.id).unwrap().len(), 4);
        // Feld leeren
        assert!(set_field(&f.c, t.id, sp.id, json!(null), &ui()).unwrap().is_some());
        assert_eq!(values::get_value(&f.c, t.id, &sp).unwrap(), None);
    }

    #[test]
    fn update_rejects_foreign_fields_and_bad_values_atomically() {
        let f = Fx::new();
        let t = f.mk(f.story, "T");
        let steps = f.field(f.defect, "steps");
        let sp = f.field(f.story, "story_points");
        let r = update_ticket(&f.c, t.id, &TicketUpdate { title: Some("Neu".into()), values: vec![(sp.id, json!(3)), (steps.id, json!("x"))] }, &ui());
        assert_eq!(r.unwrap_err(), TicketError::FieldNotInType { field_id: steps.id });
        let r = update_ticket(&f.c, t.id, &TicketUpdate { title: Some("Neu".into()), values: vec![(sp.id, json!("x"))] }, &ui());
        assert!(matches!(r, Err(TicketError::ValueInvalid(_))));
        assert_eq!(get_ticket(&f.c, t.id).unwrap().title, "T", "nichts wurde geändert");
        assert_eq!(update_ticket(&f.c, t.id, &TicketUpdate { title: Some("  ".into()), values: vec![] }, &ui()).unwrap_err(), TicketError::TitleEmpty);
        assert!(update_ticket(&f.c, 9999, &TicketUpdate::default(), &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn ticket_fields_reference_tickets_of_the_same_project() {
        let f = Fx::new();
        let epic_field = f.field(f.story, "epic");
        let epic = f.mk(f.epic, "E");
        let story = f.mk(f.story, "S");
        let other_story = f.mk(f.story, "S2");
        assert!(set_field(&f.c, story.id, epic_field.id, json!(epic.id), &ui()).unwrap().is_some());
        // nur Epics erlaubt
        assert!(matches!(set_field(&f.c, story.id, epic_field.id, json!(other_story.id), &ui()), Err(TicketError::RefInvalid(_))));
        // nicht existierend
        assert!(matches!(set_field(&f.c, story.id, epic_field.id, json!(9999), &ui()), Err(TicketError::RefInvalid(_))));
        // anderes Projekt
        let other = f.other_project("OTH");
        let other_epic_type = crate::planning::types::list_types(&f.c, other).unwrap().iter().find(|t| t.key.as_deref() == Some("epic")).unwrap().id;
        let foreign = create_ticket(&f.c, &NewTicket::new(other, other_epic_type, "F"), &ui()).unwrap().0;
        assert!(matches!(set_field(&f.c, story.id, epic_field.id, json!(foreign.id), &ui()), Err(TicketError::RefInvalid(_))));
        // beim Löschen des Ziels wird der Verweis geleert
        let impact = delete_ticket(&f.c, epic.id, &Source::agent("A")).unwrap();
        assert_eq!(impact.referenced_by, 1);
        assert_eq!(values::get_value(&f.c, story.id, &epic_field).unwrap(), None);
        let h = history_of(&f.c, story.id).unwrap();
        assert!(h.iter().any(|e| e.field == HistoryField::Custom(epic_field.id) && e.new.is_none() && e.source.kind == SourceKind::Agent));
    }

    #[test]
    fn flag_and_star_are_logged() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        assert!(set_flagged(&f.c, t.id, true, &ui()).unwrap().is_some());
        assert!(set_flagged(&f.c, t.id, true, &ui()).unwrap().is_none());
        assert!(set_starred(&f.c, t.id, true, &Source::quick()).unwrap().is_some());
        let now = get_ticket(&f.c, t.id).unwrap();
        assert!(now.flagged && now.starred);
        assert!(set_flagged(&f.c, t.id, false, &ui()).unwrap().is_some());
        let h = history_of(&f.c, t.id).unwrap();
        let flags: Vec<_> = h.iter().filter(|e| e.field == HistoryField::Flagged).map(|e| e.new.clone().unwrap()).collect();
        assert_eq!(flags, vec!["1", "0"]);
        assert_eq!(h.iter().find(|e| e.field == HistoryField::Starred).unwrap().source.kind, SourceKind::Quick);
        assert!(set_starred(&f.c, 9999, true, &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn delete_reports_impact_and_orphans_children() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "E");
        let s1 = f.child(&epic, f.story, "S1");
        let s2 = f.child(&epic, f.story, "S2");
        let other = f.mk(f.epic, "E2");
        crate::planning::comments::add_comment(&f.c, epic.id, "Hallo", &ui()).unwrap();
        crate::planning::comments::add_comment(&f.c, epic.id, "Welt", &ui()).unwrap();
        let rel = crate::planning::link_types::link_type_by_key(&f.c, "relates").unwrap().id;
        crate::planning::links::add_link(&f.c, epic.id, rel, crate::planning::links::Target::Ticket(other.id), &ui()).unwrap();
        crate::planning::time_entries::log_time(&f.c, epic.id, "1h", None, None, "", crate::planning::time_entries::tests_now(), &ui()).unwrap();
        let impact = delete_impact(&f.c, epic.id).unwrap();
        assert_eq!((impact.children, impact.links, impact.comments, impact.time_entries), (2, 1, 2, 1));
        assert!(impact.history >= 5);
        assert_eq!(impact.total(), impact.children + impact.links + impact.comments + impact.time_entries + impact.history + impact.referenced_by);
        let done = delete_ticket(&f.c, epic.id, &ui()).unwrap();
        assert_eq!(done, impact);
        assert!(get_ticket(&f.c, epic.id).unwrap_err().is_not_found());
        assert_eq!(get_ticket(&f.c, s1.id).unwrap().parent_id, None);
        assert_eq!(get_ticket(&f.c, s2.id).unwrap().parent_id, None);
        let n: i64 = f.c.query_row("SELECT (SELECT COUNT(*) FROM comments) + (SELECT COUNT(*) FROM time_entries) + (SELECT COUNT(*) FROM links) + (SELECT COUNT(*) FROM history WHERE ticket_id = ?1)", [epic.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "alles kaskadiert");
        assert!(history_of(&f.c, s1.id).unwrap().iter().any(|e| e.field == HistoryField::Parent && e.new.is_none()));
        assert!(delete_ticket(&f.c, epic.id, &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn progress_counts_done_children() {
        let f = Fx::new();
        let story = f.mk(f.story, "S");
        let t1 = f.child(&story, f.task, "T1");
        let t2 = f.child(&story, f.task, "T2");
        let _t3 = f.child(&story, f.task, "T3");
        let p = progress_of(&f.c, story.id, Basis::TicketCount).unwrap();
        assert_eq!((p.done, p.total, p.percent()), (0.0, 3.0, 0));
        crate::planning::transition::transition_ticket(&f.c, t1.id, f.done, &[], &ui()).unwrap();
        let p = progress_of(&f.c, story.id, Basis::TicketCount).unwrap();
        assert_eq!((p.done, p.total, p.percent()), (1.0, 3.0, 33));
        // Punkte: T1 = 5 (erledigt), T2 = 3
        let sp = f.field(f.task, "story_points");
        set_field(&f.c, t1.id, sp.id, json!(5), &ui()).unwrap();
        set_field(&f.c, t2.id, sp.id, json!(3), &ui()).unwrap();
        let p = progress_of(&f.c, story.id, Basis::Points).unwrap();
        assert_eq!((p.done, p.total), (5.0, 8.0));
        let p = progress_of(&f.c, story.id, Basis::PointsWithFallbackOne).unwrap();
        assert_eq!((p.done, p.total), (5.0, 9.0), "T3 ohne Punkte zählt 1");
        // ohne Kinder
        let p = progress_of(&f.c, t1.id, Basis::Points).unwrap();
        assert_eq!((p.total, p.fraction), (0.0, 0.0));
        assert!(progress_of(&f.c, 9999, Basis::Points).unwrap_err().is_not_found());
        assert_eq!(children_of(&f.c, story.id).unwrap().len(), 3);
        assert!(is_done(&f.c, &get_ticket(&f.c, t1.id).unwrap()).unwrap());
        assert!(!is_done(&f.c, &get_ticket(&f.c, t2.id).unwrap()).unwrap());
    }

    #[test]
    fn move_rank_renumbers_densely() {
        let f = Fx::new();
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let c = f.mk(f.task, "C");
        let d = f.mk(f.task, "D");
        let order = |f: &Fx| -> Vec<i64> {
            let mut v = crate::planning::ticket_list::list_tickets(&f.c, Some(f.project), &crate::planning::ticket_list::TicketFilter::default()).unwrap().items;
            v.sort_by_key(|t| (t.rank, t.id));
            v.iter().map(|t| t.id).collect()
        };
        assert_eq!(order(&f), vec![a.id, b.id, c.id, d.id]);
        // D vor B
        assert!(move_rank(&f.c, d.id, Some(b.id)).unwrap().is_some());
        assert_eq!(order(&f), vec![a.id, d.id, b.id, c.id]);
        let ranks: Vec<i64> = [a.id, d.id, b.id, c.id].iter().map(|i| get_ticket(&f.c, *i).unwrap().rank).collect();
        assert_eq!(ranks, vec![0, 1, 2, 3]);
        // A ans Ende
        move_rank(&f.c, a.id, None).unwrap().unwrap();
        assert_eq!(order(&f), vec![d.id, b.id, c.id, a.id]);
        // keine Änderung: schon vor dem Nachbarn / ans Ende, wenn schon am Ende / vor sich selbst
        assert!(move_rank(&f.c, d.id, Some(b.id)).unwrap().is_none());
        assert!(move_rank(&f.c, a.id, None).unwrap().is_none());
        assert!(move_rank(&f.c, a.id, Some(a.id)).unwrap().is_none());
        // Ziel aus anderem Projekt / unbekannt
        let other = f.other_project("OTH");
        let ot = crate::planning::types::list_types(&f.c, other).unwrap()[0].id;
        let o = create_ticket(&f.c, &NewTicket::new(other, ot, "O"), &ui()).unwrap().0;
        assert!(move_rank(&f.c, a.id, Some(o.id)).unwrap_err().is_not_found());
        assert!(move_rank(&f.c, a.id, Some(9999)).unwrap_err().is_not_found());
        assert_eq!(get_ticket(&f.c, o.id).unwrap().rank, 0, "andere Projekte bleiben unberührt");
    }
}
