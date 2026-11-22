//! Einträge des Kalenders für einen Zeitraum (#164): eigene Termine (Vorkommen der Serien nur im Zeitraum) und die
//! Quellen aus der Planung – Fälligkeiten von Tickets und Todo-Karten, Sprints, Versionen/Meilensteine, gebuchte Zeit.
//! Jede Quelle ist in den Optionen schaltbar, global und je Projekt (0 = einfache Todo-Liste). Nur lesend; archivierte
//! Tickets und System-Projekte bleiben außen vor, Sprints/Versionen nur in Projekten, die sie nutzen.

use super::store::{self, Options};
use super::zone::Zone;
use super::Event;
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use rusqlite::{params, Connection};

/// Art eines Eintrags (Name in MCP-Antworten: [`Kind::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Event,
    Due,
    Todo,
    Sprint,
    Version,
    Milestone,
    Time,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Event => "event",
            Kind::Due => "due",
            Kind::Todo => "todo",
            Kind::Sprint => "sprint",
            Kind::Version => "version",
            Kind::Milestone => "milestone",
            Kind::Time => "time",
        }
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Kind::Event => lang.t("Termin", "Event"),
            Kind::Due => lang.t("Fälligkeit (Ticket)", "Due date (ticket)"),
            Kind::Todo => lang.t("Fälligkeit (Todo)", "Due date (to-do)"),
            Kind::Sprint => lang.t("Sprint", "Sprint"),
            Kind::Version => lang.t("Version", "Version"),
            Kind::Milestone => lang.t("Meilenstein", "Milestone"),
            Kind::Time => lang.t("Gebuchte Zeit", "Booked time"),
        }
    }
}

/// Farbe der Todo-Fälligkeiten und Standardfarbe eigener Termine.
pub const TODO_COLOR: &str = "#E5A04B";
pub const EVENT_COLOR: &str = "#58A6E6";

/// Ein Eintrag im Kalender.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// Kennung in der Quelle (Termin, Ticket, Todo-Karte, Sprint, Version, Meilenstein, Zeiteintrag).
    pub id: i64,
    pub title: String,
    pub start: NaiveDateTime,
    /// Ende (ausschließlich); gleich `start` bei Zeitpunkten.
    pub end: NaiveDateTime,
    pub all_day: bool,
    pub color: String,
    pub project: Option<i64>,
    /// Ticket-Kennung (`PAN-12`), `TODO-3` oder Projektschlüssel (Sprints, Versionen).
    pub key: String,
    /// Erledigt (Ticket im Status „Erledigt“, abgehakte Karte, erschienene Version, abgeschlossener Sprint).
    pub done: bool,
    pub recurring: bool,
}

impl Item {
    /// Ganztägig über mehr als einen Tag (Balken)?
    pub fn multi_day(&self) -> bool {
        self.all_day && self.end - self.start > Duration::days(1)
    }

    /// Erster und letzter Tag (einschließlich).
    pub fn day_span(&self) -> (NaiveDate, NaiveDate) {
        let last = if self.end > self.start { (self.end - Duration::seconds(1)).date() } else { self.start.date() };
        (self.start.date(), last.max(self.start.date()))
    }

    /// Schlüssel für die Auswahl in der Oberfläche (Art, Kennung, Beginn).
    pub fn sel_key(&self) -> (Kind, i64, NaiveDateTime) {
        (self.kind, self.id, self.start)
    }
}

/// Einträge eines Termins im Zeitraum.
pub fn event_items(e: &Event, from: NaiveDateTime, to: NaiveDateTime, zone: &dyn Zone) -> Vec<Item> {
    let color = if e.color.is_empty() { EVENT_COLOR.to_string() } else { e.color.clone() };
    e.occurrences(from, to, zone)
        .into_iter()
        .map(|s| Item {
            kind: Kind::Event,
            id: e.id,
            title: e.title.clone(),
            start: s,
            end: e.end_of(s),
            all_day: e.all_day,
            color: color.clone(),
            project: e.project_id,
            key: String::new(),
            done: false,
            recurring: e.recur.is_some(),
        })
        .collect()
}

/// Alle Einträge in `[from, to)` (Tage), sortiert nach Beginn: eigene Termine und eingeschaltete Quellen.
pub fn collect(conn: &Connection, opts: &Options, from: NaiveDate, to: NaiveDate, zone: &dyn Zone) -> Vec<Item> {
    let (f, t) = (from.and_time(NaiveTime::MIN), to.and_time(NaiveTime::MIN));
    let mut out: Vec<Item> = store::events_for_range(conn, f, t).unwrap_or_default().iter().flat_map(|e| event_items(e, f, t, zone)).collect();
    out.extend(source_items(conn, opts, from, to));
    sort(&mut out);
    out
}

/// Sortierung: Beginn, dann ganztägige zuerst, dann längere zuerst, dann Titel.
pub fn sort(v: &mut [Item]) {
    v.sort_by(|a, b| a.start.cmp(&b.start).then(b.all_day.cmp(&a.all_day)).then((b.end - b.start).cmp(&(a.end - a.start))).then(a.title.cmp(&b.title)).then(a.id.cmp(&b.id)));
}

fn day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Einträge aus der Planung in `[from, to)` nach den Optionen.
pub fn source_items(conn: &Connection, opts: &Options, from: NaiveDate, to: NaiveDate) -> Vec<Item> {
    let mut out = Vec::new();
    let (fs, ts) = (day(from), day(to));
    let on = |project: i64, pick: fn(&store::Sources) -> bool| pick(&opts.for_project(project));
    // Fälligkeiten von Tickets
    let tickets = conn.prepare(
        "SELECT t.id, p.id, p.key, t.number, t.title, v.text, p.color, COALESCE(s.category, '') = 'done'
         FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id JOIN projects p ON p.id = t.project_id
         LEFT JOIN statuses s ON s.id = t.status_id
         WHERE f.key = 'due' AND f.builtin = 1 AND v.text IS NOT NULL AND v.text >= ?1 AND v.text < ?2 AND t.archived = 0 AND p.system = 0",
    );
    if let Ok(mut st) = tickets {
        let rows = st.query_map(params![fs, ts], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, bool>(7)?))
        });
        for (id, pid, pkey, num, title, due, color, done) in rows.into_iter().flatten().flatten() {
            if !on(pid, |s| s.due) {
                continue;
            }
            if let Some(it) = due_item(Kind::Due, id, &format!("{pkey}-{num}"), &title, &due, &color, Some(pid), done) {
                out.push(it);
            }
        }
    }
    // Fälligkeiten der einfachen Todo-Liste (Projekt 0)
    if on(0, |s| s.due) {
        if let Ok(mut st) = conn.prepare("SELECT id, title, due, done FROM todo_items WHERE due IS NOT NULL AND due >= ?1 AND due < ?2") {
            let rows = st.query_map(params![fs, ts], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, bool>(3)?)));
            for (id, title, due, done) in rows.into_iter().flatten().flatten() {
                if let Some(it) = due_item(Kind::Todo, id, &format!("TODO-{id}"), &title, &due, TODO_COLOR, None, done) {
                    out.push(it);
                }
            }
        }
    }
    // Sprints (mit Datum) als mehrtägige Balken
    if let Ok(mut st) = conn.prepare(
        "SELECT s.id, p.id, p.key, s.name, s.start_date, s.end_date, s.state, p.color FROM sprints s JOIN projects p ON p.id = s.project_id
         WHERE p.use_sprints = 1 AND p.system = 0 AND s.start_date IS NOT NULL AND s.end_date IS NOT NULL AND s.start_date < ?2 AND s.end_date >= ?1",
    ) {
        let rows = st.query_map(params![fs, ts], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?))
        });
        for (id, pid, pkey, name, s, e, state, color) in rows.into_iter().flatten().flatten() {
            let (Some(s), Some(e)) = (super::parse_date(&s), super::parse_date(&e)) else { continue };
            if !on(pid, |x| x.sprints) {
                continue;
            }
            out.push(Item {
                kind: Kind::Sprint,
                id,
                title: name,
                start: s.and_time(NaiveTime::MIN),
                end: (e + Duration::days(1)).and_time(NaiveTime::MIN),
                all_day: true,
                color,
                project: Some(pid),
                key: pkey,
                done: state == "completed",
                recurring: false,
            });
        }
    }
    // Versionen und Meilensteine (Fälligkeitstag)
    for (kind, table, done_col) in [(Kind::Version, "versions", "released"), (Kind::Milestone, "milestones", "reached")] {
        let sql = format!(
            "SELECT r.id, p.id, p.key, r.name, r.due_date, r.{done_col}, p.color FROM {table} r JOIN projects p ON p.id = r.project_id
             WHERE p.use_versions = 1 AND p.system = 0 AND r.due_date IS NOT NULL AND r.due_date >= ?1 AND r.due_date < ?2"
        );
        let Ok(mut st) = conn.prepare(&sql) else { continue };
        let rows = st.query_map(params![fs, ts], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, bool>(5)?, r.get::<_, String>(6)?))
        });
        for (id, pid, pkey, name, due, done, color) in rows.into_iter().flatten().flatten() {
            let Some(d) = super::parse_date(&due) else { continue };
            if !on(pid, |x| x.versions) {
                continue;
            }
            out.push(Item { kind, id, title: name, start: d.and_time(NaiveTime::MIN), end: (d + Duration::days(1)).and_time(NaiveTime::MIN), all_day: true, color, project: Some(pid), key: pkey, done, recurring: false });
        }
    }
    // gebuchte Zeit (Zeiteinträge), etwas vor dem Zeitraum beginnend (Einträge über Mitternacht)
    let early = day(from - Duration::days(2));
    if let Ok(mut st) = conn.prepare(
        "SELECT e.id, p.id, p.key, t.number, t.title, e.start, e.minutes, p.color FROM time_entries e JOIN tickets t ON t.id = e.ticket_id
         JOIN projects p ON p.id = t.project_id WHERE p.system = 0 AND e.start >= ?1 AND e.start < ?2",
    ) {
        let rows = st.query_map(params![early, ts], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, i64>(6)?, r.get::<_, String>(7)?))
        });
        let f = from.and_time(NaiveTime::MIN);
        for (id, pid, pkey, num, title, start, minutes, color) in rows.into_iter().flatten().flatten() {
            let Some((s, _)) = super::parse_when(&start) else { continue };
            let e = s + Duration::minutes(minutes.max(0));
            if e <= f || !on(pid, |x| x.time) {
                continue;
            }
            out.push(Item { kind: Kind::Time, id, title, start: s, end: e, all_day: false, color, project: Some(pid), key: format!("{pkey}-{num}"), done: true, recurring: false });
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn due_item(kind: Kind, id: i64, key: &str, title: &str, due: &str, color: &str, project: Option<i64>, done: bool) -> Option<Item> {
    let (d, time) = crate::due::parse_stored(due)?;
    let (start, end, all_day) = match time {
        Some(t) => {
            let s = d.and_time(t);
            (s, s, false)
        }
        None => (d.and_time(NaiveTime::MIN), (d + Duration::days(1)).and_time(NaiveTime::MIN), true),
    };
    Some(Item { kind, id, title: title.to_string(), start, end, all_day, color: color.to_string(), project, key: key.to_string(), done, recurring: false })
}

/// Projekte für das Optionen-Fenster (Kennung, Schlüssel, Name; ohne System-Projekte).
pub fn projects(conn: &Connection) -> Vec<(i64, String, String)> {
    let Ok(mut st) = conn.prepare("SELECT id, key, name FROM projects WHERE system = 0 ORDER BY position, id") else { return vec![] };
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
}
