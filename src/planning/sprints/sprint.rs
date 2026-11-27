//! Sprints: Lebenslauf (planen → starten → abschließen), Zugehörigkeit der Tickets, Backlog und Inhalt mit Summen.
//!
//! # Regeln
//!
//! * Je Projekt gibt es höchstens **einen aktiven** Sprint (auch im Schema abgesichert). Geplante Sprints darf es beliebig viele geben.
//! * Ein Ticket steckt höchstens in **einem offenen** (geplanten oder aktiven) Sprint. Wer es in einen anderen Sprint legt,
//!   nimmt es aus dem bisherigen heraus. Das Backlog sind die Tickets ohne offenen Sprint (nach `tickets.rank`).
//! * Die Zugehörigkeit (`sprint_tickets`) ist eine Historie: Aus einem **aktiven** Sprint entfernte Tickets behalten ihre
//!   Zeile (`removed_at`), damit Burndown und Bericht den Umfangswechsel kennen. In einem **geplanten** Sprint ist noch nichts
//!   zugesagt, dort wird die Zeile einfach gelöscht.
//! * Beim **Abschließen** bleiben alle Zeilen im Sprint stehen (der Bericht zeigt später, was erledigt und was offen war);
//!   unerledigte Tickets (Status-Kategorie nicht „Done“) wandern je nach [`Leftover`] ins Backlog oder in einen geplanten Sprint.
//! * Abgeschlossene Sprints sind schreibgeschützt.
//! * Die Dauer-Vorlage: Ende = der `default_days`-te Arbeitstag ab dem Start (der Starttag zählt mit, ist er ein Arbeitstag).

use super::{
    atomic, clock, ensure_name_free, fmt_date, parse_date, sprint_project, Figure, SRes, SprintError, DEFAULT_SPRINT_DAYS, MAX_GOAL_CHARS, MAX_SPRINT_DAYS, MAX_SPRINT_SPAN_DAYS,
};
use crate::calc::calendar::WorkCalendar;
use crate::planning::common;
use crate::planning::tickets::{self, Ticket, POINTS_FIELD_KEY};
use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SprintState {
    Planned,
    Active,
    Completed,
}

impl SprintState {
    pub fn as_str(self) -> &'static str {
        match self {
            SprintState::Planned => "planned",
            SprintState::Active => "active",
            SprintState::Completed => "completed",
        }
    }

    pub fn parse(s: &str) -> SprintState {
        match s {
            "active" => SprintState::Active,
            "completed" => SprintState::Completed,
            _ => SprintState::Planned,
        }
    }

    /// Geplant oder aktiv.
    pub fn is_open(self) -> bool {
        self != SprintState::Completed
    }

    /// Anzeigename in beiden Sprachen.
    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            SprintState::Planned => lang.t("Geplant", "Planned"),
            SprintState::Active => lang.t("Aktiv", "Active"),
            SprintState::Completed => lang.t("Abgeschlossen", "Completed"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sprint {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub goal: String,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub state: SprintState,
    /// Dauer-Vorlage in Arbeitstagen.
    pub default_days: u32,
    pub created_at: String,
    /// Zeitpunkt des Starts (Zusage des Umfangs).
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

const COLS: &str = "id, project_id, name, goal, start_date, end_date, state, default_days, created_at, started_at, completed_at";

fn date_col(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Option<NaiveDate>> {
    Ok(r.get::<_, Option<String>>(i)?.and_then(|s| parse_date(&s).ok()))
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Sprint> {
    Ok(Sprint {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        goal: r.get(3)?,
        start_date: date_col(r, 4)?,
        end_date: date_col(r, 5)?,
        state: SprintState::parse(&r.get::<_, String>(6)?),
        default_days: r.get(7)?,
        created_at: r.get(8)?,
        started_at: r.get(9)?,
        completed_at: r.get(10)?,
    })
}

/// Ein Sprint (auch wenn das Projekt keine Sprints mehr nutzt).
pub fn get_sprint(conn: &Connection, id: i64) -> SRes<Sprint> {
    conn.query_row(&format!("SELECT {COLS} FROM sprints WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(SprintError::not_found)
}

/// Alle Sprints des Projekts in der Reihenfolge des Anlegens.
pub fn list_sprints(conn: &Connection, project_id: i64) -> SRes<Vec<Sprint>> {
    sprint_project(conn, project_id)?;
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM sprints WHERE project_id = ?1 ORDER BY id"))?;
    Ok(st.query_map([project_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Der aktive Sprint des Projekts. `None`, wenn keiner läuft **oder das Projekt keine Sprints nutzt**.
pub fn active_sprint(conn: &Connection, project_id: i64) -> Option<Sprint> {
    conn.query_row(
        &format!("SELECT {COLS} FROM sprints WHERE project_id = ?1 AND state = 'active' AND (SELECT use_sprints FROM projects WHERE id = ?1) = 1"),
        [project_id],
        from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Kennungen der aktiven Sprints (alle Projekte, die Sprints nutzen, oder nur eines). Haken für die Suche (`activeSprint()`).
pub fn sprint_ids_active(conn: &Connection, project: Option<i64>) -> Vec<i64> {
    let run = || -> rusqlite::Result<Vec<i64>> {
        let mut st = conn.prepare(
            "SELECT s.id FROM sprints s JOIN projects p ON p.id = s.project_id WHERE s.state = 'active' AND p.use_sprints = 1 AND (?1 IS NULL OR s.project_id = ?1) ORDER BY s.id",
        )?;
        let rows = st.query_map([project], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
        Ok(rows)
    };
    run().unwrap_or_default()
}

/// Der offene (geplante oder aktive) Sprint des Tickets; `None` im Backlog oder wenn das Projekt keine Sprints nutzt.
pub fn sprint_of(conn: &Connection, ticket_id: i64) -> Option<i64> {
    conn.query_row(
        "SELECT st.sprint_id FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id JOIN projects p ON p.id = s.project_id
         WHERE st.ticket_id = ?1 AND st.removed_at IS NULL AND s.state IN ('planned', 'active') AND p.use_sprints = 1",
        [ticket_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

// ------------------------------------------------------------------ Dauer-Vorlage

/// Ende aus Start und Dauer: der `work_days`-te Arbeitstag ab `start` (ist `start` ein Arbeitstag, zählt er als erster; sonst
/// beginnt die Zählung am nächsten Arbeitstag). Zwei Wochen ab Montag = 10 Arbeitstage = Freitag der zweiten Woche.
pub fn end_from_duration(start: NaiveDate, work_days: u32, cal: &WorkCalendar) -> SRes<NaiveDate> {
    if !(1..=MAX_SPRINT_DAYS).contains(&work_days) {
        return Err(SprintError::DaysInvalid { max: MAX_SPRINT_DAYS });
    }
    let first = cal.next_work_day_on_or_after(start).ok_or(SprintError::NoWorkDays)?;
    cal.add_work_days(first, work_days as i64 - 1).ok_or(SprintError::NoWorkDays)
}

/// Vorschlag für den nächsten Sprint: Start am ersten Arbeitstag nach dem Ende des zuletzt endenden Sprints des Projekts (frühestens
/// `today`), Ende aus der Dauer-Vorlage.
pub fn suggest_dates(conn: &Connection, project_id: i64, work_days: u32, cal: &WorkCalendar, today: NaiveDate) -> SRes<(NaiveDate, NaiveDate)> {
    sprint_project(conn, project_id)?;
    let last: Option<String> = conn.query_row("SELECT MAX(end_date) FROM sprints WHERE project_id = ?1", [project_id], |r| r.get(0))?;
    let after = last.and_then(|s| parse_date(&s).ok()).and_then(|d| d.succ_opt()).unwrap_or(today).max(today);
    let start = cal.next_work_day_on_or_after(after).ok_or(SprintError::NoWorkDays)?;
    Ok((start, end_from_duration(start, work_days, cal)?))
}

/// Vorschlag für den Namen des nächsten Sprints: „Sprint <n>“ (der erste freie Wert).
pub fn suggest_name(conn: &Connection, project_id: i64) -> SRes<String> {
    sprint_project(conn, project_id)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM sprints WHERE project_id = ?1", [project_id], |r| r.get(0))?;
    let mut n = count + 1;
    loop {
        let name = format!("Sprint {n}");
        if ensure_name_free(conn, "sprints", project_id, &name, None).is_ok() {
            return Ok(name);
        }
        n += 1;
    }
}

// ------------------------------------------------------------------ Anlegen, Ändern, Löschen

#[derive(Debug, Clone)]
pub struct NewSprint {
    pub project_id: i64,
    pub name: String,
    pub goal: String,
    pub start: Option<NaiveDate>,
    /// Ohne Ende, aber mit Start, wird das Ende aus der Dauer-Vorlage berechnet.
    pub end: Option<NaiveDate>,
    /// Dauer-Vorlage in Arbeitstagen (Standard: [`DEFAULT_SPRINT_DAYS`]).
    pub default_days: Option<u32>,
}

impl NewSprint {
    /// Geplanter Sprint ohne Ziel und ohne Termine.
    pub fn new(project_id: i64, name: &str) -> Self {
        NewSprint { project_id, name: name.to_string(), goal: String::new(), start: None, end: None, default_days: None }
    }
}

/// Änderungen an einem Sprint; `None` = unverändert (bei den Terminen `Some(None)` = löschen).
#[derive(Debug, Clone, Default)]
pub struct UpdateSprint {
    pub name: Option<String>,
    pub goal: Option<String>,
    pub start: Option<Option<NaiveDate>>,
    pub end: Option<Option<NaiveDate>>,
    pub default_days: Option<u32>,
}

fn clean_goal(s: &str) -> SRes<String> {
    let g = s.trim().to_string();
    if g.chars().count() > MAX_GOAL_CHARS {
        return Err(SprintError::TextTooLong { max: MAX_GOAL_CHARS });
    }
    Ok(g)
}

fn check_days(days: u32) -> SRes<()> {
    if (1..=MAX_SPRINT_DAYS).contains(&days) { Ok(()) } else { Err(SprintError::DaysInvalid { max: MAX_SPRINT_DAYS }) }
}

fn check_dates(start: Option<NaiveDate>, end: Option<NaiveDate>) -> SRes<()> {
    if end.is_some() && start.is_none() {
        return Err(SprintError::DatesIncomplete);
    }
    if let (Some(s), Some(e)) = (start, end) {
        if e < s {
            return Err(SprintError::DatesOrder);
        }
        if (e - s).num_days() > MAX_SPRINT_SPAN_DAYS {
            return Err(SprintError::DatesTooLong { max: MAX_SPRINT_SPAN_DAYS });
        }
    }
    Ok(())
}

/// Legt einen geplanten Sprint an. Mit Start, aber ohne Ende, rechnet die Dauer-Vorlage das Ende aus (`cal`).
pub fn create_sprint(conn: &Connection, n: &NewSprint, cal: &WorkCalendar) -> SRes<i64> {
    sprint_project(conn, n.project_id)?;
    let name = common::clean_name(&n.name)?;
    ensure_name_free(conn, "sprints", n.project_id, &name, None)?;
    let goal = clean_goal(&n.goal)?;
    let days = n.default_days.unwrap_or(DEFAULT_SPRINT_DAYS);
    check_days(days)?;
    check_dates(n.start, n.end)?;
    let end = match (n.start, n.end) {
        (Some(s), None) => Some(end_from_duration(s, days, cal)?),
        (_, e) => e,
    };
    conn.execute(
        "INSERT INTO sprints (project_id, name, goal, start_date, end_date, state, default_days, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'planned', ?6, ?7)",
        params![n.project_id, name, goal, n.start.map(fmt_date), end.map(fmt_date), days, clock::now()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Ändert Name, Ziel, Termine oder Dauer-Vorlage (nicht bei abgeschlossenen Sprints; ein aktiver Sprint behält beide Termine).
pub fn update_sprint(conn: &Connection, id: i64, u: &UpdateSprint) -> SRes<()> {
    let cur = get_sprint(conn, id)?;
    sprint_project(conn, cur.project_id)?;
    if cur.state == SprintState::Completed {
        return Err(SprintError::SprintCompleted);
    }
    let name = match &u.name {
        Some(n) => {
            let n = common::clean_name(n)?;
            ensure_name_free(conn, "sprints", cur.project_id, &n, Some(id))?;
            n
        }
        None => cur.name.clone(),
    };
    let goal = match &u.goal {
        Some(g) => clean_goal(g)?,
        None => cur.goal.clone(),
    };
    let days = u.default_days.unwrap_or(cur.default_days);
    check_days(days)?;
    let start = u.start.unwrap_or(cur.start_date);
    let end = u.end.unwrap_or(cur.end_date);
    check_dates(start, end)?;
    if cur.state == SprintState::Active && (start.is_none() || end.is_none()) {
        return Err(SprintError::DatesIncomplete);
    }
    conn.execute(
        "UPDATE sprints SET name = ?1, goal = ?2, start_date = ?3, end_date = ?4, default_days = ?5 WHERE id = ?6",
        params![name, goal, start.map(fmt_date), end.map(fmt_date), days, id],
    )?;
    Ok(())
}

/// Löscht einen **geplanten** Sprint; seine Tickets landen wieder im Backlog.
pub fn delete_sprint(conn: &Connection, id: i64) -> SRes<()> {
    let cur = get_sprint(conn, id)?;
    sprint_project(conn, cur.project_id)?;
    if cur.state != SprintState::Planned {
        return Err(SprintError::NotPlanned);
    }
    conn.execute("DELETE FROM sprints WHERE id = ?1", [id])?;
    Ok(())
}

// ------------------------------------------------------------------ Starten und Abschließen

/// Startet einen geplanten Sprint (höchstens einer je Projekt). Fehlt der Start, ist es heute; fehlt das Ende, kommt es aus der
/// Dauer-Vorlage. Die Tickets, die jetzt im Sprint sind, bilden den zugesagten Startumfang.
pub fn start_sprint(conn: &Connection, id: i64, cal: &WorkCalendar) -> SRes<Sprint> {
    atomic(conn, || {
        let cur = get_sprint(conn, id)?;
        sprint_project(conn, cur.project_id)?;
        if cur.state != SprintState::Planned {
            return Err(SprintError::NotPlanned);
        }
        let running: Option<i64> = conn.query_row("SELECT id FROM sprints WHERE project_id = ?1 AND state = 'active'", [cur.project_id], |r| r.get(0)).optional()?;
        if let Some(sprint_id) = running {
            return Err(SprintError::AnotherActive { sprint_id });
        }
        let start = cur.start_date.unwrap_or_else(clock::today);
        let end = match cur.end_date {
            Some(e) => e,
            None => end_from_duration(start, cur.default_days, cal)?,
        };
        check_dates(Some(start), Some(end))?;
        conn.execute(
            "UPDATE sprints SET state = 'active', start_date = ?1, end_date = ?2, started_at = ?3 WHERE id = ?4",
            params![fmt_date(start), fmt_date(end), clock::now(), id],
        )?;
        // Zugesagt sind die Punkte beim Start: Was nach dem Einplanen geschätzt oder geändert wurde, gilt für Burndown, Bericht und Velocity.
        let rows: Vec<(i64, i64)> = {
            let mut st = conn.prepare("SELECT id, ticket_id FROM sprint_tickets WHERE sprint_id = ?1 AND removed_at IS NULL")?;
            st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        for (row, ticket) in rows {
            conn.execute("UPDATE sprint_tickets SET points_at_add = ?1 WHERE id = ?2", params![ticket_points(conn, ticket)?, row])?;
        }
        get_sprint(conn, id)
    })
}

/// Wohin die unerledigten Tickets beim Abschließen gehen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leftover {
    /// Zurück ins Backlog (sie sind in keinem offenen Sprint mehr).
    ToBacklog,
    /// In diesen geplanten Sprint desselben Projekts.
    ToSprint(i64),
}

/// Was beim Abschließen geschehen ist.
#[derive(Debug, Clone, PartialEq)]
pub struct CompleteResult {
    pub sprint: Sprint,
    /// Erledigte Tickets (Status-Kategorie „Done“); sie bleiben im abgeschlossenen Sprint.
    pub done: Vec<i64>,
    /// Unerledigte Tickets (ohne Status gelten sie als unerledigt) und ihr Ziel gemäß [`Leftover`].
    pub leftover: Vec<i64>,
}

/// Schließt den aktiven Sprint ab. Erledigt = Status-Kategorie „Done“. Die Zugehörigkeitszeilen bleiben für den Bericht im
/// Sprint stehen; die unerledigten Tickets werden zusätzlich in den Zielsprint gelegt oder sind (ins Backlog) frei.
pub fn complete_sprint(conn: &Connection, id: i64, leftover: Leftover) -> SRes<CompleteResult> {
    atomic(conn, || {
        let cur = get_sprint(conn, id)?;
        sprint_project(conn, cur.project_id)?;
        if cur.state != SprintState::Active {
            return Err(SprintError::NotActive);
        }
        if let Leftover::ToSprint(to) = leftover {
            let target = get_sprint(conn, to).map_err(|e| if e.is_not_found() { SprintError::LeftoverInvalid } else { e })?;
            if target.id == id || target.project_id != cur.project_id || target.state != SprintState::Planned {
                return Err(SprintError::LeftoverInvalid);
            }
        }
        let rows: Vec<(i64, bool)> = {
            let mut st = conn.prepare(
                "SELECT st.ticket_id, COALESCE(s.category = 'done', 0) FROM sprint_tickets st JOIN tickets t ON t.id = st.ticket_id
                 LEFT JOIN statuses s ON s.id = t.status_id WHERE st.sprint_id = ?1 AND st.removed_at IS NULL AND t.archived = 0 ORDER BY t.rank, t.id",
            )?;
            // Archivierte Tickets (Wartung) bleiben im abgeschlossenen Sprint und wandern nicht mit (wie im Abschlussdialog).
            st.query_map([id], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)))?.collect::<Result<_, _>>()?
        };
        let now = clock::now();
        let (done, open): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, d)| *d);
        let done: Vec<i64> = done.into_iter().map(|(t, _)| t).collect();
        let open: Vec<i64> = open.into_iter().map(|(t, _)| t).collect();
        if let Leftover::ToSprint(to) = leftover {
            for t in &open {
                enter_sprint(conn, to, *t, &now)?;
            }
        }
        conn.execute("UPDATE sprints SET state = 'completed', completed_at = ?1 WHERE id = ?2", params![now, id])?;
        Ok(CompleteResult { sprint: get_sprint(conn, id)?, done, leftover: open })
    })
}

// ------------------------------------------------------------------ Zugehörigkeit der Tickets

/// Story Points des Tickets (Zahlenfeld mit dem Schlüssel `story_points`); `None`, wenn es keines hat.
pub fn ticket_points(conn: &Connection, ticket_id: i64) -> SRes<Option<f64>> {
    Ok(conn
        .query_row(
            "SELECT v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE v.ticket_id = ?1 AND f.key = ?2 AND f.kind = 'number' LIMIT 1",
            params![ticket_id, POINTS_FIELD_KEY],
            |r| r.get::<_, Option<f64>>(0),
        )
        .optional()?
        .flatten())
}

/// Zeile und Sprint, in dem das Ticket gerade (offen) steckt.
fn open_membership(conn: &Connection, ticket_id: i64) -> SRes<Option<(i64, i64, SprintState)>> {
    Ok(conn
        .query_row(
            "SELECT st.id, st.sprint_id, s.state FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id
             WHERE st.ticket_id = ?1 AND st.removed_at IS NULL AND s.state IN ('planned', 'active')",
            [ticket_id],
            |r| Ok((r.get(0)?, r.get(1)?, SprintState::parse(&r.get::<_, String>(2)?))),
        )
        .optional()?)
}

/// Nimmt das Ticket aus seinem offenen Sprint (geplant: Zeile löschen, aktiv: `removed_at` setzen). Liefert den Sprint, den es verlassen hat.
fn leave_open_sprint(conn: &Connection, ticket_id: i64, now: &str) -> SRes<Option<i64>> {
    Ok(match open_membership(conn, ticket_id)? {
        None => None,
        Some((row, sprint, state)) => {
            if state == SprintState::Planned {
                conn.execute("DELETE FROM sprint_tickets WHERE id = ?1", [row])?;
            } else {
                conn.execute("UPDATE sprint_tickets SET removed_at = ?1 WHERE id = ?2", params![now, row])?;
            }
            Some(sprint)
        }
    })
}

fn enter_sprint(conn: &Connection, sprint_id: i64, ticket_id: i64, now: &str) -> SRes<()> {
    let points = ticket_points(conn, ticket_id)?;
    conn.execute("INSERT INTO sprint_tickets (sprint_id, ticket_id, added_at, points_at_add) VALUES (?1, ?2, ?3, ?4)", params![sprint_id, ticket_id, now, points])?;
    Ok(())
}

/// Legt die Tickets (in dieser Reihenfolge, ohne Doppelte) in den offenen Sprint; aus einem anderen offenen Sprint werden sie
/// herausgenommen. Liefert, wie viele Tickets sich wirklich bewegt haben (im Sprint schon vorhandene zählen nicht). Alles oder nichts.
pub fn move_tickets_to_sprint(conn: &Connection, sprint_id: i64, ticket_ids: &[i64]) -> SRes<usize> {
    atomic(conn, || {
        let sp = get_sprint(conn, sprint_id)?;
        sprint_project(conn, sp.project_id)?;
        if sp.state == SprintState::Completed {
            return Err(SprintError::SprintCompleted);
        }
        let mut ids: Vec<i64> = Vec::new();
        for id in ticket_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        for id in &ids {
            if tickets::get_ticket(conn, *id)?.project_id != sp.project_id {
                return Err(SprintError::TicketOtherProject { ticket_id: *id });
            }
        }
        let now = clock::now();
        let mut moved = 0;
        for id in ids {
            if open_membership(conn, id)?.is_some_and(|(_, s, _)| s == sprint_id) {
                continue;
            }
            leave_open_sprint(conn, id, &now)?;
            enter_sprint(conn, sprint_id, id, &now)?;
            moved += 1;
        }
        Ok(moved)
    })
}

/// Nimmt die Tickets aus ihrem offenen Sprint heraus (zurück ins Backlog). Liefert die Zahl der Tickets, die in einem Sprint waren.
pub fn move_tickets_to_backlog(conn: &Connection, ticket_ids: &[i64]) -> SRes<usize> {
    atomic(conn, || {
        let mut ids: Vec<i64> = Vec::new();
        for id in ticket_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        for id in &ids {
            sprint_project(conn, tickets::get_ticket(conn, *id)?.project_id)?;
        }
        let now = clock::now();
        let mut n = 0;
        for id in ids {
            if leave_open_sprint(conn, id, &now)?.is_some() {
                n += 1;
            }
        }
        Ok(n)
    })
}

/// Aufräumen, wenn ein Ticket das Projekt wechselt: Es verlässt seinen offenen Sprint (der gehört zum alten Projekt). Zeilen
/// abgeschlossener Sprints bleiben als Historie.
pub(crate) fn on_ticket_left_project(conn: &Connection, ticket_id: i64) -> rusqlite::Result<()> {
    let now = clock::now();
    conn.execute(
        "DELETE FROM sprint_tickets WHERE ticket_id = ?1 AND removed_at IS NULL AND sprint_id IN (SELECT id FROM sprints WHERE state = 'planned')",
        [ticket_id],
    )?;
    conn.execute(
        "UPDATE sprint_tickets SET removed_at = ?2 WHERE ticket_id = ?1 AND removed_at IS NULL AND sprint_id IN (SELECT id FROM sprints WHERE state = 'active')",
        params![ticket_id, now],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ Backlog und Inhalt

pub(crate) fn points_sql() -> String {
    format!("(SELECT v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE v.ticket_id = t.id AND f.key = '{POINTS_FIELD_KEY}' AND f.kind = 'number' LIMIT 1)")
}

/// Das Backlog: Tickets des Projekts ohne offenen Sprint, nach `rank`. Mit `include_done = false` fehlen die erledigten Tickets
/// (Status-Kategorie „Done“; z. B. die aus abgeschlossenen Sprints).
pub fn backlog(conn: &Connection, project_id: i64, include_done: bool) -> SRes<Vec<Ticket>> {
    sprint_project(conn, project_id)?;
    let sql = format!(
        "SELECT {} {} LEFT JOIN statuses s ON s.id = t.status_id WHERE t.project_id = ?1 AND t.archived = 0
         AND NOT EXISTS (SELECT 1 FROM sprint_tickets st JOIN sprints sp ON sp.id = st.sprint_id
                         WHERE st.ticket_id = t.id AND st.removed_at IS NULL AND sp.state IN ('planned', 'active'))
         {} ORDER BY t.rank, t.id",
        tickets::COLS,
        tickets::FROM,
        if include_done { "" } else { "AND COALESCE(s.category <> 'done', 1)" }
    );
    let mut st = conn.prepare(&sql)?;
    Ok(st.query_map([project_id], tickets::from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Ein Ticket im Sprint.
#[derive(Debug, Clone, PartialEq)]
pub struct SprintItem {
    pub ticket: Ticket,
    /// Story Points (Zahlenfeld `story_points`); `None` ohne.
    pub points: Option<f64>,
    /// Status-Kategorie „Done“.
    pub done: bool,
    pub added_at: String,
}

/// Summen eines Sprints: alle Tickets und die erledigten.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SprintTotals {
    pub total: Figure,
    pub done: Figure,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SprintContent {
    pub sprint: Sprint,
    /// Nach `rank` des Tickets.
    pub items: Vec<SprintItem>,
    pub totals: SprintTotals,
}

/// Tickets, die jetzt im Sprint sind (ohne entfernte), mit Summen. Bei einem abgeschlossenen Sprint die, die am Ende noch drin waren.
pub fn sprint_content(conn: &Connection, sprint_id: i64) -> SRes<SprintContent> {
    let sprint = get_sprint(conn, sprint_id)?;
    sprint_project(conn, sprint.project_id)?;
    let sql = format!(
        "SELECT {}, {}, COALESCE(s.category = 'done', 0), st.added_at {} JOIN sprint_tickets st ON st.ticket_id = t.id
         LEFT JOIN statuses s ON s.id = t.status_id WHERE st.sprint_id = ?1 AND st.removed_at IS NULL AND t.archived = 0 ORDER BY t.rank, t.id",
        tickets::COLS,
        points_sql(),
        tickets::FROM
    );
    let mut st = conn.prepare(&sql)?;
    let items = st
        .query_map([sprint_id], |r| Ok(SprintItem { ticket: tickets::from_row(r)?, points: r.get(tickets::COLS_N)?, done: r.get::<_, i64>(tickets::COLS_N + 1)? != 0, added_at: r.get(tickets::COLS_N + 2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut totals = SprintTotals::default();
    for i in &items {
        totals.total.add(i.points);
        if i.done {
            totals.done.add(i.points);
        }
    }
    Ok(SprintContent { sprint, items, totals })
}

/// Nur die Summen (für die Anzeige am Sprint-Kopf im Backlog).
pub fn sprint_totals(conn: &Connection, sprint_id: i64) -> SRes<SprintTotals> {
    Ok(sprint_content(conn, sprint_id)?.totals)
}
