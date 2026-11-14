//! Rückgängig/Wiederholen für die Sprint-Zugehörigkeit (Fallenlassen im Backlog, „Verschieben nach …“).
//!
//! # Warum Zeilen wiederherstellen und keine Gegenereignisse schreiben
//!
//! Burndown, Bericht und Velocity (`analysis`) werden nicht aus einem eigenen Ereignisprotokoll gelesen, sondern aus den Zeilen
//! von `sprint_tickets` (`added_at`, `removed_at`, `points_at_add`) – die Zeilen **sind** die Ereignisse. Ein Rückgängig, das ein
//! Gegenereignis anhängt (Hinzufügen → „entfernt“), würde in einem aktiven Sprint eine Umfangsänderung (hinzugekommen und wieder
//! herausgenommen) vortäuschen, die der Nutzer gar nicht gewollt hat. Deshalb hält eine [`SprintChange`] die Zeilen vorher und
//! nachher fest:
//!
//! * Rückgängig stellt die Zeilen **exakt** wieder her (eine neu entstandene Zeile verschwindet, eine geänderte oder gelöschte
//!   kommt mit denselben Zeitstempeln und Punkten zurück) – Burndown und Bericht sehen aus, als wäre nichts geschehen;
//! * Wiederholen stellt den Zustand danach her, ebenfalls mit den ursprünglichen Zeitstempeln (so liegt das Ereignis im
//!   Burndown dort, wo die Aktion stattfand, und nicht beim Drücken von Strg+Y).
//!
//! # Wann es nicht mehr geht
//!
//! Geändert werden darf nur, was noch genau so aussieht wie beim Ausführen der Aktion: Die beteiligten Zeilen müssen unverändert
//! sein, die beteiligten Sprints im selben Zustand (geplant/aktiv; gestartet, abgeschlossen oder gelöscht heißt „zu spät“, damit
//! ein Rückgängig nie den zugesagten Umfang eines inzwischen gestarteten Sprints verfälscht) und das Ticket darf danach nicht in
//! zwei offenen Sprints stehen. Sonst gibt es [`TicketError::SprintChanged`], die Historie verwirft den Eintrag, und nichts ändert sich.

use super::sprint::{get_sprint, move_tickets_to_backlog, move_tickets_to_sprint, SprintState};
use super::{sprint_project, SRes};
use crate::planning::ticket_error::{TRes, TicketError};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

/// Eine Zeile aus `sprint_tickets`.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    pub id: i64,
    pub sprint_id: i64,
    pub ticket_id: i64,
    pub added_at: String,
    pub removed_at: Option<String>,
    pub points_at_add: Option<f64>,
}

const COLS: &str = "id, sprint_id, ticket_id, added_at, removed_at, points_at_add";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<MemberRow> {
    Ok(MemberRow { id: r.get(0)?, sprint_id: r.get(1)?, ticket_id: r.get(2)?, added_at: r.get(3)?, removed_at: r.get(4)?, points_at_add: r.get(5)? })
}

/// Änderung der Zugehörigkeit einiger Tickets: nur die Zeilen, die sich geändert haben, vorher und nachher.
#[derive(Debug, Clone, PartialEq)]
pub struct SprintChange {
    /// Die Zeilen, wie sie vor der Aktion waren (geänderte und gelöschte), nach Kennung.
    pub before: Vec<MemberRow>,
    /// Die Zeilen, wie sie nach der Aktion sind (geänderte und neue), nach Kennung.
    pub after: Vec<MemberRow>,
    /// Zustand der beteiligten Sprints beim Ausführen.
    pub sprints: Vec<(i64, SprintState)>,
    /// Die betroffenen Tickets (für die Bereinigung der Historie nach dem Löschen).
    pub tickets: Vec<i64>,
}

fn snapshot(conn: &Connection, ids: &[i64]) -> SRes<HashMap<i64, MemberRow>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM sprint_tickets WHERE ticket_id = ?1"))?;
    let mut out = HashMap::new();
    for id in ids {
        for row in st.query_map([id], from_row)? {
            let row = row?;
            out.insert(row.id, row);
        }
    }
    Ok(out)
}

fn current(conn: &Connection, id: i64) -> rusqlite::Result<Option<MemberRow>> {
    conn.query_row(&format!("SELECT {COLS} FROM sprint_tickets WHERE id = ?1"), [id], from_row).optional()
}

/// Führt `f` aus und hält fest, was sich an der Zugehörigkeit der `ids` geändert hat (`None`, wenn nichts).
fn track(conn: &Connection, ids: &[i64], f: impl FnOnce() -> SRes<usize>) -> SRes<(usize, Option<SprintChange>)> {
    let mut tickets: Vec<i64> = Vec::new();
    for id in ids {
        if !tickets.contains(id) {
            tickets.push(*id);
        }
    }
    let before = snapshot(conn, &tickets)?;
    let moved = f()?;
    let after = snapshot(conn, &tickets)?;
    let mut b: Vec<MemberRow> = before.iter().filter(|(id, r)| after.get(id) != Some(r)).map(|(_, r)| r.clone()).collect();
    let mut a: Vec<MemberRow> = after.iter().filter(|(id, r)| before.get(id) != Some(r)).map(|(_, r)| r.clone()).collect();
    if b.is_empty() && a.is_empty() {
        return Ok((moved, None));
    }
    b.sort_by_key(|r| r.id);
    a.sort_by_key(|r| r.id);
    let mut sprint_ids: Vec<i64> = b.iter().chain(a.iter()).map(|r| r.sprint_id).collect();
    sprint_ids.sort_unstable();
    sprint_ids.dedup();
    let mut sprints = Vec::with_capacity(sprint_ids.len());
    for id in sprint_ids {
        sprints.push((id, get_sprint(conn, id)?.state));
    }
    let mut touched: Vec<i64> = b.iter().chain(a.iter()).map(|r| r.ticket_id).collect();
    touched.sort_unstable();
    touched.dedup();
    Ok((moved, Some(SprintChange { before: b, after: a, sprints, tickets: touched })))
}

/// Wie [`move_tickets_to_sprint`], dazu die Änderung für Strg+Z (`None`, wenn sich nichts bewegt hat).
pub fn move_tickets_to_sprint_tracked(conn: &Connection, sprint_id: i64, ticket_ids: &[i64]) -> SRes<(usize, Option<SprintChange>)> {
    track(conn, ticket_ids, || move_tickets_to_sprint(conn, sprint_id, ticket_ids))
}

/// Wie [`move_tickets_to_backlog`], dazu die Änderung für Strg+Z (`None`, wenn sich nichts bewegt hat).
pub fn move_tickets_to_backlog_tracked(conn: &Connection, ticket_ids: &[i64]) -> SRes<(usize, Option<SprintChange>)> {
    track(conn, ticket_ids, || move_tickets_to_backlog(conn, ticket_ids))
}

impl SprintChange {
    /// Die Gegenänderung (vorher und nachher getauscht).
    pub fn inverse(&self) -> SprintChange {
        SprintChange { before: self.after.clone(), after: self.before.clone(), sprints: self.sprints.clone(), tickets: self.tickets.clone() }
    }

    pub fn touches(&self, ids: &[i64]) -> bool {
        self.tickets.iter().any(|t| ids.contains(t))
    }

    /// Stellt den Zustand `after` her (siehe Moduldoku für die Voraussetzungen). Der Aufrufer macht das atomar.
    pub fn apply(&self, conn: &Connection) -> TRes<()> {
        for (sid, state) in &self.sprints {
            let sp = get_sprint(conn, *sid).map_err(|_| TicketError::SprintChanged)?;
            if sp.state != *state || sprint_project(conn, sp.project_id).is_err() {
                return Err(TicketError::SprintChanged);
            }
        }
        for r in &self.before {
            if current(conn, r.id)?.as_ref() != Some(r) {
                return Err(TicketError::SprintChanged);
            }
        }
        for r in &self.after {
            if !self.before.iter().any(|b| b.id == r.id) && current(conn, r.id)?.is_some() {
                return Err(TicketError::SprintChanged);
            }
        }
        for r in &self.before {
            if !self.after.iter().any(|a| a.id == r.id) {
                conn.execute("DELETE FROM sprint_tickets WHERE id = ?1", [r.id])?;
            }
        }
        // Entfernte Zeilen zuerst: Die offene Zeile eines Tickets in einem Sprint ist eindeutig (Teil-Unique-Index).
        let mut put: Vec<&MemberRow> = self.after.iter().collect();
        put.sort_by_key(|r| (r.removed_at.is_none(), r.id));
        for r in put {
            conn.execute(
                "INSERT INTO sprint_tickets (id, sprint_id, ticket_id, added_at, removed_at, points_at_add) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET sprint_id = excluded.sprint_id, ticket_id = excluded.ticket_id, added_at = excluded.added_at,
                                               removed_at = excluded.removed_at, points_at_add = excluded.points_at_add",
                params![r.id, r.sprint_id, r.ticket_id, r.added_at, r.removed_at, r.points_at_add],
            )
            .map_err(|_| TicketError::SprintChanged)?;
        }
        // Ein Ticket steckt höchstens in einem offenen Sprint (sonst hat jemand zwischendurch ohne Historie umgeräumt).
        for t in &self.tickets {
            let open: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id WHERE st.ticket_id = ?1 AND st.removed_at IS NULL AND s.state IN ('planned', 'active')",
                [t],
                |r| r.get(0),
            )?;
            if open > 1 {
                return Err(TicketError::SprintChanged);
            }
        }
        Ok(())
    }
}
