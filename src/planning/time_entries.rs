//! Zeiterfassung an Tickets: Dauer (Eingabe wie `1h 30m`, `2d`, nur eine Zahl = Stunden – `calc::duration`), Start,
//! Art der Arbeit (Liste je Projekt) und Kommentar (Markdown). Es gibt kein Restaufwand-System.
//!
//! Startlogik von [`log_time`] (die Uhrzeit „jetzt“ wird als Parameter übergeben):
//! * ohne Start: `jetzt − Dauer` (der Eintrag endet jetzt);
//! * nur ein Datum, keine Uhrzeit: 08:00 Uhr an diesem Tag;
//! * Datum mit Uhrzeit: genau diese;
//! * das Ende ist immer Start + Dauer ([`TimeEntry::end_time`]).

use super::history::{self, HistoryField, Source};
use super::ops::TicketOp;
use super::ticket_error::{atomic, TRes, TicketError};
use super::tickets::{get_ticket, touch};
use super::work_kinds::{get_work_kind, list_work_kinds};
use crate::calc::duration::{parse_duration, Minutes};
use crate::calc::progress::{sum_by_day, sum_minutes};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::collections::BTreeMap;

/// Längster Kommentar eines Zeiteintrags in Zeichen.
pub const MAX_TIME_COMMENT_CHARS: usize = 10_000;

/// Vorgabe für den Start der Erfassung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartInput {
    /// Datum und Uhrzeit.
    DateTime(NaiveDateTime),
    /// Nur ein Datum: Beginn um 08:00 Uhr.
    Date(NaiveDate),
}

impl From<NaiveDateTime> for StartInput {
    fn from(d: NaiveDateTime) -> Self {
        StartInput::DateTime(d)
    }
}

impl From<NaiveDate> for StartInput {
    fn from(d: NaiveDate) -> Self {
        StartInput::Date(d)
    }
}

impl StartInput {
    /// Liest `JJJJ-MM-TT`, `JJJJ-MM-TT HH:MM[:SS]` oder `JJJJ-MM-TTTHH:MM[:SS]`. Leere oder unlesbare Eingabe: `None`.
    /// Das Jahr muss vierstellig sein (sonst würde „26-11-14“ als Jahr 26 gelesen).
    pub fn parse(s: &str) -> Option<StartInput> {
        let s = s.trim();
        let year_ok = |y: i32| (1000..=9999).contains(&y);
        for f in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
            if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
                return Some(StartInput::DateTime(d)).filter(|_| year_ok(d.year()));
            }
        }
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().filter(|d| year_ok(d.year())).map(StartInput::Date)
    }

    /// Der Zeitpunkt, den die Vorgabe meint.
    pub fn resolve(self) -> NaiveDateTime {
        match self {
            StartInput::DateTime(d) => d,
            StartInput::Date(d) => d.and_hms_opt(8, 0, 0).expect("08:00 existiert"),
        }
    }
}

/// Der Start laut Regeln der Moduldoku.
pub fn resolve_start(start: Option<StartInput>, minutes: Minutes, now: NaiveDateTime) -> TRes<NaiveDateTime> {
    match start {
        Some(s) => Ok(s.resolve()),
        None => now.checked_sub_signed(Duration::minutes(minutes as i64)).ok_or(TicketError::StartInvalid),
    }
}

const ISO: &str = "%Y-%m-%dT%H:%M:%S";

#[derive(Debug, Clone, PartialEq)]
pub struct TimeEntry {
    pub id: i64,
    pub ticket_id: i64,
    /// Start als ISO-Text `JJJJ-MM-TTTHH:MM:SS`.
    pub start: String,
    pub minutes: Minutes,
    pub work_kind_id: Option<i64>,
    /// Markdown.
    pub comment: String,
    pub created_at: String,
}

/// Der änderbare Teil eines Zeiteintrags (für Rückgängig/Wiederholen).
#[derive(Debug, Clone, PartialEq)]
pub struct TimeState {
    pub start: String,
    pub minutes: Minutes,
    pub work_kind_id: Option<i64>,
    pub comment: String,
}

impl TimeEntry {
    pub fn start_time(&self) -> Option<NaiveDateTime> {
        NaiveDateTime::parse_from_str(&self.start, ISO).ok()
    }

    /// Ende = Start + Dauer.
    pub fn end_time(&self) -> Option<NaiveDateTime> {
        self.start_time()?.checked_add_signed(Duration::minutes(self.minutes as i64))
    }

    /// Der Tag des Starts (für die Tagessummen).
    pub fn day(&self) -> Option<NaiveDate> {
        self.start_time().map(|d| d.date())
    }

    pub fn state(&self) -> TimeState {
        TimeState { start: self.start.clone(), minutes: self.minutes, work_kind_id: self.work_kind_id, comment: self.comment.clone() }
    }
}

const COLS: &str = "id, ticket_id, start, minutes, work_kind_id, comment, created_at";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<TimeEntry> {
    Ok(TimeEntry { id: r.get(0)?, ticket_id: r.get(1)?, start: r.get(2)?, minutes: r.get::<_, i64>(3)?.clamp(0, u32::MAX as i64) as u32, work_kind_id: r.get(4)?, comment: r.get(5)?, created_at: r.get(6)? })
}

pub fn get_time_entry(conn: &Connection, id: i64) -> TRes<TimeEntry> {
    conn.query_row(&format!("SELECT {COLS} FROM time_entries WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(TicketError::not_found)
}

/// Die Zeiteinträge eines Tickets nach Start (dann Anlegen).
pub fn entries_of(conn: &Connection, ticket_id: i64) -> TRes<Vec<TimeEntry>> {
    get_ticket(conn, ticket_id)?;
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM time_entries WHERE ticket_id = ?1 ORDER BY start, id"))?;
    Ok(st.query_map([ticket_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Die vorgeschlagene Art der Arbeit eines Projekts (der erste Eintrag der Liste, z. B. „Entwicklung“).
pub fn default_work_kind(conn: &Connection, project_id: i64) -> TRes<Option<i64>> {
    Ok(list_work_kinds(conn, project_id)?.first().map(|k| k.id))
}

fn clean_comment(s: &str) -> TRes<String> {
    let c = s.trim();
    if c.chars().count() > MAX_TIME_COMMENT_CHARS {
        return Err(TicketError::CommentTooLong { max: MAX_TIME_COMMENT_CHARS });
    }
    Ok(c.to_string())
}

fn parse_minutes(input: &str) -> TRes<Minutes> {
    let m = parse_duration(input).map_err(TicketError::DurationInvalid)?;
    if m == 0 { Err(TicketError::DurationZero) } else { Ok(m) }
}

fn check_work_kind(conn: &Connection, project_id: i64, work_kind: Option<i64>) -> TRes<()> {
    if let Some(k) = work_kind {
        let wk = get_work_kind(conn, k).map_err(|_| TicketError::WorkKindInvalid)?;
        if wk.project_id != project_id {
            return Err(TicketError::WorkKindInvalid);
        }
    }
    Ok(())
}

fn json_of(minutes: Minutes, start: &str, kind: Option<i64>, comment: &str) -> String {
    json!({ "minutes": minutes, "start": start, "kind": kind, "comment": comment }).to_string()
}

pub(crate) fn raw_insert(conn: &Connection, e: &TimeEntry, source: &Source, batch: i64) -> TRes<i64> {
    if e.id > 0 {
        conn.execute(
            "INSERT INTO time_entries (id, ticket_id, start, minutes, work_kind_id, comment, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![e.id, e.ticket_id, e.start, e.minutes, e.work_kind_id, e.comment, e.created_at],
        )?;
    } else {
        conn.execute(
            "INSERT INTO time_entries (ticket_id, start, minutes, work_kind_id, comment, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![e.ticket_id, e.start, e.minutes, e.work_kind_id, e.comment, e.created_at],
        )?;
    }
    let id = if e.id > 0 { e.id } else { conn.last_insert_rowid() };
    history::log(conn, e.ticket_id, source, batch, &HistoryField::Time, None, Some(&json_of(e.minutes, &e.start, e.work_kind_id, &e.comment)))?;
    touch(conn, e.ticket_id)?;
    Ok(id)
}

pub(crate) fn raw_delete(conn: &Connection, e: &TimeEntry, source: &Source, batch: i64) -> TRes<()> {
    if conn.execute("DELETE FROM time_entries WHERE id = ?1", [e.id])? == 0 {
        return Err(TicketError::not_found());
    }
    history::log(conn, e.ticket_id, source, batch, &HistoryField::Time, Some(&json_of(e.minutes, &e.start, e.work_kind_id, &e.comment)), None)?;
    touch(conn, e.ticket_id)
}

/// Setzt den änderbaren Teil. Liefert den alten Zustand, wenn sich etwas geändert hat.
pub(crate) fn raw_set_state(conn: &Connection, id: i64, new: &TimeState, source: &Source, batch: i64) -> TRes<Option<TimeState>> {
    let cur = get_time_entry(conn, id)?;
    let old = cur.state();
    if old == *new {
        return Ok(None);
    }
    conn.execute("UPDATE time_entries SET start = ?1, minutes = ?2, work_kind_id = ?3, comment = ?4 WHERE id = ?5", params![new.start, new.minutes, new.work_kind_id, new.comment, id])?;
    history::log(
        conn,
        cur.ticket_id,
        source,
        batch,
        &HistoryField::Time,
        Some(&json_of(old.minutes, &old.start, old.work_kind_id, &old.comment)),
        Some(&json_of(new.minutes, &new.start, new.work_kind_id, &new.comment)),
    )?;
    touch(conn, cur.ticket_id)?;
    Ok(Some(old))
}

/// Erfasst Zeit an einem Ticket. `input` ist die Dauer (`1h 30m`, `2d`, `1,5h`; nur eine Zahl = Stunden), `start` siehe
/// Moduldoku, `work_kind` muss zum Projekt des Tickets gehören, `now` ist die aktuelle lokale Zeit.
pub fn log_time(conn: &Connection, ticket_id: i64, input: &str, start: Option<StartInput>, work_kind: Option<i64>, comment: &str, now: NaiveDateTime, source: &Source) -> TRes<(i64, TicketOp)> {
    atomic(conn, || {
        let t = get_ticket(conn, ticket_id)?;
        let minutes = parse_minutes(input)?;
        check_work_kind(conn, t.project_id, work_kind)?;
        let comment = clean_comment(comment)?;
        let start = resolve_start(start, minutes, now)?;
        let batch = history::next_batch(conn)?;
        let e = TimeEntry { id: 0, ticket_id, start: start.format(ISO).to_string(), minutes, work_kind_id: work_kind, comment, created_at: super::common::now() };
        let id = raw_insert(conn, &e, source, batch)?;
        Ok((id, TicketOp::Time { entry: get_time_entry(conn, id)?, present: true, source: source.clone() }))
    })
}

/// Änderungen an einem Zeiteintrag; `None` = unverändert.
#[derive(Debug, Clone, Default)]
pub struct TimeEdit {
    /// Neue Dauer als Eingabetext.
    pub duration: Option<String>,
    pub start: Option<StartInput>,
    /// `Some(None)` entfernt die Art der Arbeit.
    pub work_kind: Option<Option<i64>>,
    pub comment: Option<String>,
}

/// Ändert einen Zeiteintrag. `None` = nichts geändert.
pub fn edit_time_entry(conn: &Connection, id: i64, e: &TimeEdit, source: &Source) -> TRes<Option<TicketOp>> {
    atomic(conn, || {
        let cur = get_time_entry(conn, id)?;
        let t = get_ticket(conn, cur.ticket_id)?;
        let mut new = cur.state();
        if let Some(d) = &e.duration {
            new.minutes = parse_minutes(d)?;
        }
        if let Some(s) = e.start {
            new.start = s.resolve().format(ISO).to_string();
        }
        if let Some(k) = e.work_kind {
            check_work_kind(conn, t.project_id, k)?;
            new.work_kind_id = k;
        }
        if let Some(c) = &e.comment {
            new.comment = clean_comment(c)?;
        }
        let batch = history::next_batch(conn)?;
        Ok(raw_set_state(conn, id, &new, source, batch)?.map(|before| TicketOp::TimeEdit { ticket: cur.ticket_id, entry: id, before, after: new, source: source.clone() }))
    })
}

/// Löscht einen Zeiteintrag endgültig (die Oberfläche fragt vorher nach).
pub fn delete_time_entry(conn: &Connection, id: i64, source: &Source) -> TRes<()> {
    atomic(conn, || {
        let e = get_time_entry(conn, id)?;
        let batch = history::next_batch(conn)?;
        raw_delete(conn, &e, source, batch)
    })
}

/// Gesamtzeit des Tickets in Minuten.
pub fn time_total(conn: &Connection, ticket_id: i64) -> TRes<Minutes> {
    let all: Vec<Minutes> = entries_of(conn, ticket_id)?.iter().map(|e| e.minutes).collect();
    Ok(sum_minutes(&all))
}

/// Zeit des Tickets je Tag (Tag des Starts), aufsteigend.
pub fn time_by_day(conn: &Connection, ticket_id: i64) -> TRes<BTreeMap<NaiveDate, Minutes>> {
    let rows: Vec<(NaiveDate, Minutes)> = entries_of(conn, ticket_id)?.iter().filter_map(|e| e.day().map(|d| (d, e.minutes))).collect();
    Ok(sum_by_day(&rows))
}

/// Gebuchte Zeit je Tag über alle Tickets (oder eines Projekts) zwischen zwei Tagen einschließlich – z. B. für die
/// Zeiterfassungs-Kachel.
pub fn time_by_day_in(conn: &Connection, project_id: Option<i64>, from: NaiveDate, to: NaiveDate) -> TRes<BTreeMap<NaiveDate, Minutes>> {
    let mut st = conn.prepare(
        "SELECT e.start, e.minutes FROM time_entries e JOIN tickets t ON t.id = e.ticket_id
         WHERE (?1 IS NULL OR t.project_id = ?1) AND substr(e.start, 1, 10) BETWEEN ?2 AND ?3",
    )?;
    let rows = st
        .query_map(params![project_id, from.format("%Y-%m-%d").to_string(), to.format("%Y-%m-%d").to_string()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let pairs: Vec<(NaiveDate, Minutes)> = rows
        .into_iter()
        .filter_map(|(s, m)| NaiveDateTime::parse_from_str(&s, ISO).ok().map(|d| (d.date(), m.clamp(0, u32::MAX as i64) as u32)))
        .collect();
    Ok(sum_by_day(&pairs))
}

#[cfg(test)]
pub(crate) fn tests_now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::history::{describe, history_of};
    use crate::planning::tickets::delete_ticket;
    use crate::planning::tickets::fixture::Fx;
    use crate::state::Lang;

    fn ui() -> Source {
        Source::ui()
    }

    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
    }

    #[test]
    fn start_defaults_to_now_minus_duration() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let (id, _) = log_time(&f.c, t.id, "1h 30m", None, None, "", tests_now(), &ui()).unwrap();
        let e = get_time_entry(&f.c, id).unwrap();
        assert_eq!(e.minutes, 90);
        assert_eq!(e.start_time(), Some(dt(2026, 11, 14, 10, 30)));
        assert_eq!(e.end_time(), Some(tests_now()), "Ende = jetzt");
        assert_eq!(e.day(), Some(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()));
    }

    #[test]
    fn date_only_start_means_eight_o_clock_and_end_is_start_plus_duration() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let day = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let (id, _) = log_time(&f.c, t.id, "2d", Some(day.into()), None, "", tests_now(), &ui()).unwrap();
        let e = get_time_entry(&f.c, id).unwrap();
        assert_eq!(e.start, "2026-11-02T08:00:00");
        assert_eq!(e.minutes, 16 * 60);
        assert_eq!(e.end_time(), Some(dt(2026, 11, 3, 0, 0)));
        let (id, _) = log_time(&f.c, t.id, "45m", Some(dt(2026, 11, 3, 13, 15).into()), None, "", tests_now(), &ui()).unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().start, "2026-11-03T13:15:00");
        // Start über Mitternacht zurück
        let (id, _) = log_time(&f.c, t.id, "3h", None, None, "", dt(2026, 11, 14, 1, 0), &ui()).unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().start, "2026-11-13T22:00:00");
    }

    #[test]
    fn a_bare_number_means_hours_and_bad_input_is_reported() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let (id, _) = log_time(&f.c, t.id, "2", None, None, "", tests_now(), &ui()).unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().minutes, 120);
        let (id, _) = log_time(&f.c, t.id, "0,5", None, None, "", tests_now(), &ui()).unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().minutes, 30);
        for bad in ["", "abc", "1x", "1h 1h"] {
            match log_time(&f.c, t.id, bad, None, None, "", tests_now(), &ui()) {
                Err(TicketError::DurationInvalid(e)) => assert!(!e.message(Lang::De).is_empty()),
                other => panic!("{bad}: {other:?}"),
            }
        }
        assert_eq!(log_time(&f.c, t.id, "0", None, None, "", tests_now(), &ui()).unwrap_err(), TicketError::DurationZero);
        assert_eq!(log_time(&f.c, t.id, "0m", None, None, "", tests_now(), &ui()).unwrap_err(), TicketError::DurationZero);
        assert!(log_time(&f.c, 9999, "1h", None, None, "", tests_now(), &ui()).unwrap_err().is_not_found());
        assert_eq!(entries_of(&f.c, t.id).unwrap().len(), 2, "Fehler legen nichts an");
    }

    #[test]
    fn work_kind_must_belong_to_the_project_and_has_a_default() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let kinds = list_work_kinds(&f.c, f.project).unwrap();
        assert_eq!(default_work_kind(&f.c, f.project).unwrap(), Some(kinds[0].id));
        assert_eq!(kinds[0].display(Lang::En), "Development");
        assert_eq!(kinds[0].display(Lang::De), "Entwicklung");
        let (id, _) = log_time(&f.c, t.id, "1h", None, Some(kinds[1].id), "", tests_now(), &ui()).unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().work_kind_id, Some(kinds[1].id));
        let other = f.other_project("OTH");
        let foreign = list_work_kinds(&f.c, other).unwrap()[0].id;
        assert_eq!(log_time(&f.c, t.id, "1h", None, Some(foreign), "", tests_now(), &ui()).unwrap_err(), TicketError::WorkKindInvalid);
        assert_eq!(log_time(&f.c, t.id, "1h", None, Some(9999), "", tests_now(), &ui()).unwrap_err(), TicketError::WorkKindInvalid);
        // Projekt ohne Arten der Arbeit hat keinen Vorschlag
        let empty = crate::planning::projects::create_project(&f.c, &crate::planning::projects::NewProject::new("EMP", "E", "#112233")).unwrap();
        assert_eq!(default_work_kind(&f.c, empty).unwrap(), None);
    }

    #[test]
    fn edits_and_deletes_entries() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let (id, _) = log_time(&f.c, t.id, "1h", Some(dt(2026, 11, 10, 9, 0).into()), None, "alt", tests_now(), &ui()).unwrap();
        let kind = list_work_kinds(&f.c, f.project).unwrap()[2].id;
        let op = edit_time_entry(&f.c, id, &TimeEdit { duration: Some("2h".into()), start: Some(NaiveDate::from_ymd_opt(2026, 11, 11).unwrap().into()), work_kind: Some(Some(kind)), comment: Some("  neu **fett** ".into()) }, &ui()).unwrap();
        assert!(op.is_some());
        let e = get_time_entry(&f.c, id).unwrap();
        assert_eq!((e.minutes, e.start.as_str(), e.work_kind_id, e.comment.as_str()), (120, "2026-11-11T08:00:00", Some(kind), "neu **fett**"));
        assert!(edit_time_entry(&f.c, id, &TimeEdit::default(), &ui()).unwrap().is_none());
        assert!(edit_time_entry(&f.c, id, &TimeEdit { duration: Some("2h".into()), ..Default::default() }, &ui()).unwrap().is_none(), "gleiche Dauer");
        edit_time_entry(&f.c, id, &TimeEdit { work_kind: Some(None), ..Default::default() }, &ui()).unwrap().unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().work_kind_id, None);
        assert_eq!(edit_time_entry(&f.c, id, &TimeEdit { duration: Some("0".into()), ..Default::default() }, &ui()).unwrap_err(), TicketError::DurationZero);
        assert_eq!(edit_time_entry(&f.c, id, &TimeEdit { work_kind: Some(Some(9999)), ..Default::default() }, &ui()).unwrap_err(), TicketError::WorkKindInvalid);
        assert_eq!(get_time_entry(&f.c, id).unwrap().minutes, 120, "Fehler ändern nichts");
        delete_time_entry(&f.c, id, &ui()).unwrap();
        assert!(get_time_entry(&f.c, id).unwrap_err().is_not_found());
        assert!(delete_time_entry(&f.c, id, &ui()).unwrap_err().is_not_found());
        assert!(edit_time_entry(&f.c, id, &TimeEdit::default(), &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn sums_per_ticket_and_day() {
        let f = Fx::new();
        let (a, b) = (f.mk(f.task, "A"), f.mk(f.task, "B"));
        log_time(&f.c, a.id, "1h", Some(dt(2026, 11, 9, 9, 0).into()), None, "", tests_now(), &ui()).unwrap();
        log_time(&f.c, a.id, "30m", Some(dt(2026, 11, 9, 14, 0).into()), None, "", tests_now(), &ui()).unwrap();
        log_time(&f.c, a.id, "2h", Some(dt(2026, 11, 10, 9, 0).into()), None, "", tests_now(), &ui()).unwrap();
        log_time(&f.c, b.id, "4h", Some(dt(2026, 11, 10, 9, 0).into()), None, "", tests_now(), &ui()).unwrap();
        assert_eq!(time_total(&f.c, a.id).unwrap(), 210);
        assert_eq!(time_total(&f.c, b.id).unwrap(), 240);
        let by = time_by_day(&f.c, a.id).unwrap();
        assert_eq!(by.get(&NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()), Some(&90));
        assert_eq!(by.get(&NaiveDate::from_ymd_opt(2026, 11, 10).unwrap()), Some(&120));
        assert_eq!(by.len(), 2);
        let d = |day| NaiveDate::from_ymd_opt(2026, 11, day).unwrap();
        let all = time_by_day_in(&f.c, None, d(9), d(10)).unwrap();
        assert_eq!((all[&d(9)], all[&d(10)]), (90, 360));
        let only_first = time_by_day_in(&f.c, None, d(10), d(10)).unwrap();
        assert_eq!(only_first.len(), 1);
        let other = f.other_project("OTH");
        assert!(time_by_day_in(&f.c, Some(other), d(1), d(30)).unwrap().is_empty());
        assert_eq!(time_by_day_in(&f.c, Some(f.project), d(1), d(30)).unwrap().len(), 2);
        assert_eq!(time_total(&f.c, b.id).unwrap(), 240);
        delete_ticket(&f.c, b.id, &ui()).unwrap();
        assert!(time_total(&f.c, b.id).unwrap_err().is_not_found());
    }

    #[test]
    fn start_input_parses_dates_and_times() {
        assert_eq!(StartInput::parse("2026-11-14"), Some(StartInput::Date(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap())));
        assert_eq!(StartInput::parse(" 2026-11-14 09:30 "), Some(StartInput::DateTime(dt(2026, 11, 14, 9, 30))));
        assert_eq!(StartInput::parse("2026-11-14T09:30:15").unwrap().resolve().format(ISO).to_string(), "2026-11-14T09:30:15");
        assert_eq!(StartInput::parse("2026-11-14").unwrap().resolve(), dt(2026, 11, 14, 8, 0));
        for bad in ["", "gestern", "14.11.2026", "2026-13-01", "26-11-14", "0026-11-14 09:30", "10000-01-01"] {
            assert_eq!(StartInput::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn history_shows_time_with_kind_and_comment() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let kind = list_work_kinds(&f.c, f.project).unwrap()[0].id;
        log_time(&f.c, t.id, "1h 30m", None, Some(kind), "Spike **fertig**", tests_now(), &Source::agent("A")).unwrap();
        let e = history_of(&f.c, t.id).unwrap().into_iter().find(|e| e.field == HistoryField::Time).unwrap();
        assert_eq!(describe(&f.c, &e, Lang::En), "Time logged: 1h 30m (Development) – Spike **fertig**");
        assert_eq!(describe(&f.c, &e, Lang::De), "Zeit erfasst: 1h 30m (Entwicklung) – Spike **fertig**");
    }

    #[test]
    fn too_long_comments_are_rejected() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let long = "x".repeat(MAX_TIME_COMMENT_CHARS + 1);
        assert_eq!(log_time(&f.c, t.id, "1h", None, None, &long, tests_now(), &ui()).unwrap_err(), TicketError::CommentTooLong { max: MAX_TIME_COMMENT_CHARS });
    }
}
