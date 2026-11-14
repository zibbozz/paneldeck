//! Burndown, Sprint-Bericht und Velocity aus den echten Ereignissen der Datenbank.
//!
//! # Wie aus Zeilen Ereignisse werden
//!
//! Jede Zeile von `sprint_tickets` (ein Ticket im Sprint von `added_at` bis `removed_at`) wird zu einer Folge von
//! [`ScopeEvent`]s für `calc::burndown`:
//!
//! * **Hinzugefügt** (`Added`) bei `added_at` – außer das Ticket war da schon erledigt (Status-Kategorie „Done“, aus dem Status-Verlauf
//!   zum Zeitpunkt `added_at` abgelesen): Bereits erledigte Arbeit gehört nicht zum Sprint und zählt weder zum Umfang noch zum Erledigten.
//! * Jeder Wechsel des Status (`history.field = 'status'`) in eine Done-Kategorie ergibt **Erledigt** (`Completed`), jeder Wechsel
//!   heraus **Wieder geöffnet** (`Reopened`); wird ein beim Hinzufügen schon erledigtes Ticket wieder geöffnet, ist das neuer Umfang (`Added`).
//! * **Entfernt** (`Removed`) bei `removed_at`, aber nur, wenn das Ticket dann noch offen ist (so verlangt es `calc::burndown`).
//! * Bei einem **abgeschlossenen** Sprint zählt der Verlauf nur bis `completed_at`; unerledigte Tickets bleiben dort offen
//!   (sie sind beim Abschließen nicht „entfernt“ worden) – der Rest erreicht dann nicht 0. Was danach im nächsten Sprint geschieht, gehört nicht mehr hierher.
//!
//! **Punkte:** Alle Ereignisse eines Tickets in einem Sprint tragen die Punkte zum Zeitpunkt des Hinzufügens (`points_at_add`);
//! so heben sich Hinzufügen, Erledigen und Entfernen genau auf, auch wenn die Punkte später geändert werden.
//!
//! **Startumfang:** Was bis `started_at` (der Zusage beim Start) hinzukam, gehört zum geplanten Umfang: Diese Ereignisse werden vor den
//! ersten Tag des Sprints gelegt, auch wenn sie am Starttag selbst oder (bei spätem Start) danach geschahen. Ein noch nicht gestarteter
//! Sprint zählt alles dazu. Zeitpunkte nach dem Start gelten als Umfangsänderung.
//!
//! Der Bericht ([`sprint_report`]) und die Velocity ([`velocity_for_project`]) nutzen dieselben Ereignisse und sind deshalb mit dem
//! Burndown stimmig.

use super::sprint::{get_sprint, Sprint, SprintState};
use super::{sprint_project, Figure, SRes, SprintError};
use crate::calc::burndown::{compute_burndown, velocity, BurndownSeries, ScopeEvent, SprintResult, SprintSpec, VelocityPoint};
use crate::calc::calendar::WorkCalendar;
use crate::calc::progress::Basis;
use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

const TS_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

fn ts(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, TS_FORMAT).unwrap_or_default()
}

/// Eine Zeile aus `sprint_tickets`.
struct Row {
    ticket_id: i64,
    added_at: String,
    removed_at: Option<String>,
    points: Option<f64>,
}

/// Ein Eintrag des Status-Verlaufs eines Tickets (Kennungen als Zahl, `None` = kein Status).
struct StatusChange {
    at: String,
    old: Option<i64>,
    new: Option<i64>,
}

fn load_rows(conn: &Connection, sprint_id: i64) -> SRes<Vec<Row>> {
    let mut st = conn.prepare("SELECT ticket_id, added_at, removed_at, points_at_add FROM sprint_tickets WHERE sprint_id = ?1 ORDER BY id")?;
    Ok(st
        .query_map([sprint_id], |r| Ok(Row { ticket_id: r.get(0)?, added_at: r.get(1)?, removed_at: r.get(2)?, points: r.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?)
}

fn load_status_history(conn: &Connection, ticket_id: i64) -> SRes<Vec<StatusChange>> {
    let mut st = conn.prepare("SELECT at, old, new FROM history WHERE ticket_id = ?1 AND field = 'status' ORDER BY at, id")?;
    let id = |s: Option<String>| s.and_then(|s| s.parse::<i64>().ok());
    Ok(st
        .query_map([ticket_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(at, old, new)| StatusChange { at, old: id(old), new: id(new) })
        .collect())
}

/// Der Status des Tickets zum Zeitpunkt `t`: der Eintrag davor, sonst der alte Wert des ersten späteren Eintrags, sonst der aktuelle.
fn status_at(hist: &[StatusChange], current: Option<i64>, t: &str) -> Option<i64> {
    if let Some(e) = hist.iter().filter(|e| e.at.as_str() <= t).next_back() {
        return e.new;
    }
    match hist.first() {
        Some(e) => e.old,
        None => current,
    }
}

/// Alles, was zur Bewertung der Zeilen eines Sprints gebraucht wird.
struct Timeline {
    sprint: Sprint,
    done_statuses: HashSet<i64>,
}

/// Was aus einer Zeile folgt.
struct RowFacts {
    events: Vec<ScopeEvent>,
    entry: ReportEntry,
}

/// Ein Ticket im Bericht eines Sprints.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportEntry {
    pub ticket_id: i64,
    /// Punkte zum Zeitpunkt des Hinzufügens.
    pub points: Option<f64>,
    /// Gehörte zum Startumfang.
    pub planned: bool,
    /// Kam nach dem Start dazu (oder wurde ein schon erledigtes wieder geöffnet).
    pub added: bool,
    /// Wurde nach dem Start unerledigt aus dem Sprint genommen.
    pub removed: bool,
    /// In diesem Sprint erledigt (und am Ende nicht wieder geöffnet).
    pub completed: bool,
    /// Am Ende noch offen und im Sprint.
    pub open: bool,
}

fn analyse_row(tl: &Timeline, row: &Row, hist: &[StatusChange], current: Option<i64>, burndown_cutoff: Option<NaiveDateTime>) -> RowFacts {
    let is_done = |s: Option<i64>| s.is_some_and(|s| tl.done_statuses.contains(&s));
    let cap = if tl.sprint.state == SprintState::Completed { tl.sprint.completed_at.as_deref() } else { None };
    let end = row.removed_at.as_deref().or(cap);
    let commit = tl.sprint.started_at.as_deref();
    let before_commit = |at: &str| commit.is_none_or(|c| at <= c);
    // Ereignisse aus der Planungsphase liegen vor dem ersten Tag (nur für das Burndown).
    let at_of = |at: &str, plan_phase: bool| match (plan_phase, burndown_cutoff) {
        (true, Some(cut)) => ts(at).min(cut),
        _ => ts(at),
    };
    let p = row.points;
    let a = row.added_at.as_str();
    let mut entry = ReportEntry { ticket_id: row.ticket_id, points: p, planned: false, added: false, removed: false, completed: false, open: false };
    let mut events = Vec::new();
    let pre_done = is_done(status_at(hist, current, a));
    let (mut done, mut counted, mut completed) = (pre_done, !pre_done, false);
    if !pre_done {
        events.push(ScopeEvent::Added { at: at_of(a, before_commit(a)), points: p });
        if before_commit(a) { entry.planned = true } else { entry.added = true }
    }
    for e in hist.iter().filter(|e| e.at.as_str() > a && end.is_none_or(|w| e.at.as_str() <= w)) {
        let now_done = is_done(e.new);
        if now_done && !done {
            done = true;
            if counted {
                events.push(ScopeEvent::Completed { at: ts(&e.at), points: p });
                counted = false;
                completed = true;
            }
        } else if !now_done && done {
            done = false;
            counted = true;
            if completed {
                events.push(ScopeEvent::Reopened { at: ts(&e.at), points: p });
                completed = false;
            } else {
                events.push(ScopeEvent::Added { at: at_of(&e.at, before_commit(&e.at)), points: p });
                if before_commit(&e.at) { entry.planned = true } else { entry.added = true }
            }
        }
    }
    if let Some(r) = row.removed_at.as_deref() {
        if counted {
            events.push(ScopeEvent::Removed { at: at_of(r, before_commit(r)), points: p });
            counted = false;
            if before_commit(r) { entry.planned = false } else { entry.removed = true }
        }
    }
    entry.completed = completed;
    entry.open = counted && row.removed_at.is_none();
    RowFacts { events, entry }
}

/// Ereignisse und Bericht-Einträge aller Zeilen des Sprints.
fn facts_for(conn: &Connection, sprint: Sprint, burndown_cutoff: Option<NaiveDateTime>) -> SRes<(Sprint, Vec<RowFacts>)> {
    let done_statuses: HashSet<i64> = {
        let mut st = conn.prepare("SELECT id FROM statuses WHERE project_id = ?1 AND category = 'done'")?;
        st.query_map([sprint.project_id], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let rows = load_rows(conn, sprint.id)?;
    let tl = Timeline { sprint, done_statuses };
    let mut histories: HashMap<i64, (Vec<StatusChange>, Option<i64>)> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        if !histories.contains_key(&row.ticket_id) {
            let current: Option<i64> = conn.query_row("SELECT status_id FROM tickets WHERE id = ?1", [row.ticket_id], |r| r.get(0)).optional()?.flatten();
            histories.insert(row.ticket_id, (load_status_history(conn, row.ticket_id)?, current));
        }
        let (hist, current) = &histories[&row.ticket_id];
        out.push(analyse_row(&tl, row, hist, *current, burndown_cutoff));
    }
    Ok((tl.sprint, out))
}

// ------------------------------------------------------------------ Burndown

/// Burndown des Sprints: Ideal-Linie über die Arbeitstage von `cal` und Ist-Linie bis `today`, aus den echten Ereignissen.
/// Der Sprint braucht beide Termine. Der Kalender bestimmt die Arbeitstage („Wochenenden ausschließen“ =
/// `WorkCalendar::with_weekend_switch(true)`, plus Feiertage); `basis` die Gewichtung (Punkte, Anzahl, Punkte mit „ohne = 1“).
pub fn burndown_for_sprint(conn: &Connection, sprint_id: i64, basis: Basis, cal: WorkCalendar, today: NaiveDate) -> SRes<BurndownSeries> {
    let sprint = get_sprint(conn, sprint_id)?;
    sprint_project(conn, sprint.project_id)?;
    let (Some(start), Some(end)) = (sprint.start_date, sprint.end_date) else {
        return Err(SprintError::DatesIncomplete);
    };
    let cutoff = start.pred_opt().and_then(|d| d.and_hms_opt(23, 59, 59)).unwrap_or_default();
    let (_, facts) = facts_for(conn, sprint, Some(cutoff))?;
    let events: Vec<ScopeEvent> = facts.into_iter().flat_map(|f| f.events).collect();
    Ok(compute_burndown(&SprintSpec { start, end, calendar: cal }, &events, basis, today))
}

// ------------------------------------------------------------------ Bericht

/// Der Bericht eines Sprints. Jede Zahl ist ein [`Figure`] (Anzahl und Punkte); `weight(basis)` wählt, was zählt.
#[derive(Debug, Clone, PartialEq)]
pub struct SprintReport {
    pub sprint: Sprint,
    /// Geplant: der zugesagte Startumfang.
    pub planned: Figure,
    /// Erledigt (im Sprint, am Ende nicht wieder geöffnet).
    pub completed: Figure,
    /// Nach dem Start hinzugekommen.
    pub added: Figure,
    /// Nach dem Start unerledigt entfernt.
    pub removed: Figure,
    /// Offen: noch im Sprint und nicht erledigt (bei einem abgeschlossenen Sprint die unerledigten).
    pub open: Figure,
    /// Die Tickets mit ihrer Einordnung (ein Ticket kann mehrfach vorkommen, wenn es entfernt und wieder hinzugefügt wurde).
    pub entries: Vec<ReportEntry>,
}

/// Bericht des Sprints: geplant, erledigt, hinzugefügt, entfernt, offen. Beim aktiven Sprint der Stand jetzt, beim abgeschlossenen
/// der Stand beim Abschließen.
pub fn sprint_report(conn: &Connection, sprint_id: i64) -> SRes<SprintReport> {
    let sprint = get_sprint(conn, sprint_id)?;
    sprint_project(conn, sprint.project_id)?;
    let (sprint, facts) = facts_for(conn, sprint, None)?;
    let entries: Vec<ReportEntry> = facts.into_iter().map(|f| f.entry).collect();
    let mut r = SprintReport {
        sprint,
        planned: Figure::default(),
        completed: Figure::default(),
        added: Figure::default(),
        removed: Figure::default(),
        open: Figure::default(),
        entries: Vec::new(),
    };
    for e in &entries {
        for (flag, fig) in [(e.planned, &mut r.planned), (e.completed, &mut r.completed), (e.added, &mut r.added), (e.removed, &mut r.removed), (e.open, &mut r.open)] {
            if flag {
                fig.add(e.points);
            }
        }
    }
    r.entries = entries;
    Ok(r)
}

// ------------------------------------------------------------------ Velocity

/// Geplant gegen erledigt für die **letzten** `last_n` abgeschlossenen Sprints des Projekts (alt nach neu, nach Zeitpunkt des
/// Abschließens). `basis` bestimmt, ob Punkte oder Tickets gezählt werden. `calc::burndown::average_velocity` bildet daraus den Durchschnitt.
pub fn velocity_for_project(conn: &Connection, project_id: i64, last_n: usize, basis: Basis) -> SRes<Vec<VelocityPoint>> {
    sprint_project(conn, project_id)?;
    let mut ids: Vec<i64> = {
        let mut st = conn.prepare("SELECT id FROM sprints WHERE project_id = ?1 AND state = 'completed' ORDER BY completed_at DESC, id DESC LIMIT ?2")?;
        st.query_map(rusqlite::params![project_id, last_n as i64], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    ids.reverse();
    let mut results = Vec::with_capacity(ids.len());
    for id in ids {
        let r = sprint_report(conn, id)?;
        results.push(SprintResult { label: r.sprint.name.clone(), planned: r.planned.weight(basis), completed: r.completed.weight(basis) });
    }
    Ok(velocity(&results))
}
