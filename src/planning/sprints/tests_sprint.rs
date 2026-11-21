//! Tests der Sprints: Migration, Dauer-Vorlage, Lebenslauf, Zugehörigkeit der Tickets, Backlog, Optionalität.
//! Enthält auch die gemeinsame Testumgebung `S` der übrigen Tests dieser Schicht.

use super::sprint::*;
use super::*;
use crate::calc::calendar::WorkCalendar;
use crate::planning::fields::{FieldKind, FieldOptions};
use crate::planning::history::Source;
use crate::planning::projects::{update_project, UpdateProject};
use crate::planning::tickets::fixture::Fx;
use crate::planning::tickets::{create_ticket, delete_ticket, move_rank, set_field, NewTicket, Ticket};
use crate::planning::transition::transition_ticket;
use crate::planning::PlanError;
use crate::state::Lang;
use serde_json::json;

pub(super) fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// Testumgebung: Projekt „PAN“ mit Standard-Inhalten, Sprints und Versionen eingeschaltet, Uhr auf Freitag 2026-10-30 09:00.
pub(super) struct S {
    pub f: Fx,
    pub cal: WorkCalendar,
}

impl S {
    pub fn new() -> S {
        clock::set("2026-10-30T09:00:00");
        let f = Fx::new();
        update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
        S { f, cal: WorkCalendar::default() }
    }

    pub fn c(&self) -> &Connection {
        &self.f.c
    }

    pub fn p(&self) -> i64 {
        self.f.project
    }

    /// Setzt „jetzt“.
    pub fn at(&self, now: &str) {
        clock::set(now);
    }

    /// Entwickler-Aufgabe mit Story Points (`None` = ohne).
    pub fn task(&self, title: &str, points: Option<f64>) -> Ticket {
        let t = self.f.mk(self.f.task, title);
        if let Some(p) = points {
            self.set_points(&t, p);
        }
        crate::planning::tickets::get_ticket(self.c(), t.id).unwrap()
    }

    /// Setzt die Story Points eines vorhandenen Tickets.
    pub fn set_points(&self, t: &Ticket, p: f64) {
        let sp = self.f.field(self.f.task, "story_points");
        set_field(self.c(), t.id, sp.id, json!(p), &Source::ui()).unwrap();
    }

    /// Statuswechsel mit festgelegtem Zeitpunkt im Verlauf.
    pub fn status(&self, t: &Ticket, status: i64, at: &str) {
        transition_ticket(self.c(), t.id, status, &[], &Source::ui()).unwrap();
        self.c()
            .execute("UPDATE history SET at = ?1 WHERE id = (SELECT MAX(id) FROM history WHERE ticket_id = ?2 AND field = 'status')", rusqlite::params![at, t.id])
            .unwrap();
    }

    /// Geplanter Sprint ohne Termine.
    pub fn sprint(&self, name: &str) -> i64 {
        create_sprint(self.c(), &NewSprint::new(self.p(), name), &self.cal).unwrap()
    }

    /// Geplanter Sprint mit Terminen (Mo 2026-11-02 bis Fr 2026-11-13).
    pub fn dated(&self, name: &str) -> i64 {
        let mut n = NewSprint::new(self.p(), name);
        n.start = Some(d(2026, 11, 2));
        n.end = Some(d(2026, 11, 13));
        create_sprint(self.c(), &n, &self.cal).unwrap()
    }

    pub fn ids(&self, tickets: &[&Ticket]) -> Vec<i64> {
        tickets.iter().map(|t| t.id).collect()
    }

    /// Zweites Projekt mit eingeschalteten Sprints und Versionen.
    pub fn other(&self, key: &str) -> i64 {
        let o = self.f.other_project(key);
        update_project(self.c(), o, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
        o
    }
}

fn open_rows(c: &Connection, sprint: i64) -> i64 {
    c.query_row("SELECT COUNT(*) FROM sprint_tickets WHERE sprint_id = ?1 AND removed_at IS NULL", [sprint], |r| r.get(0)).unwrap()
}

fn all_rows(c: &Connection, sprint: i64) -> i64 {
    c.query_row("SELECT COUNT(*) FROM sprint_tickets WHERE sprint_id = ?1", [sprint], |r| r.get(0)).unwrap()
}

// ------------------------------------------------------------------ Migration

#[test]
fn migration_creates_the_tables_on_a_fresh_database() {
    let c = crate::db::open_memory();
    for t in ["sprints", "sprint_tickets", "versions", "milestones"] {
        c.prepare(&format!("SELECT * FROM {t}")).unwrap();
    }
    assert!(crate::db::latest_version() >= 7, "die Sprint-Migration ist die siebte");
    let sql: String = c.query_row("SELECT group_concat(sql, ' ') FROM sqlite_master WHERE tbl_name = 'sprints'", [], |r| r.get(0)).unwrap();
    assert!(sql.contains("sprints_one_active"));
}

#[test]
fn migration_applies_on_top_of_version_6_and_keeps_the_data() {
    let d = std::env::temp_dir().join(format!("paneldeck-test-sprints-mig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let bdir = d.join("backups");
    let (project, ticket) = {
        let db = crate::db::open(&d, &bdir).unwrap();
        let mut np = crate::planning::projects::NewProject::new("PAN", "Paneldeck", "#112233");
        np.with_defaults = true;
        let project = crate::planning::projects::create_project(&db.conn, &np).unwrap();
        let ty = crate::planning::types::list_types(&db.conn, project).unwrap()[0].id;
        let (t, _) = create_ticket(&db.conn, &NewTicket::new(project, ty, "Alt"), &Source::ui()).unwrap();
        // Zurück auf den Stand von Version 6: die neuen Tabellen entfernen.
        db.conn.execute_batch("DROP TABLE maintenance_log; DROP TABLE maintenance_rules; DROP TABLE maintenance_ignored; DROP TABLE maintenance_settings; DROP INDEX tickets_archived; ALTER TABLE tickets DROP COLUMN archived; DROP TABLE sprint_tickets; DROP TABLE sprints; DROP TABLE versions; DROP TABLE milestones; DROP INDEX todo_items_due; ALTER TABLE todo_items DROP COLUMN due; ALTER TABLE projects DROP COLUMN due_thresholds; PRAGMA user_version = 6;").unwrap();
        (project, t.id)
    };
    let db = crate::db::open(&d, &bdir).unwrap();
    let v: u32 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(v, crate::db::latest_version());
    assert_eq!(crate::planning::tickets::get_ticket(&db.conn, ticket).unwrap().title, "Alt");
    assert_eq!(crate::planning::projects::get_project(&db.conn, project).unwrap().key, "PAN");
    for t in ["sprints", "sprint_tickets", "versions", "milestones"] {
        db.conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
    }
    assert_eq!(crate::backup::list(&bdir).len(), 1, "vor der Migration entsteht eine Sicherung");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_schema_allows_only_one_active_sprint_per_project() {
    let s = S::new();
    let (a, b) = (s.dated("A"), s.dated("B"));
    s.c().execute("UPDATE sprints SET state = 'active' WHERE id = ?1", [a]).unwrap();
    assert!(s.c().execute("UPDATE sprints SET state = 'active' WHERE id = ?1", [b]).is_err());
}

// ------------------------------------------------------------------ Dauer-Vorlage

#[test]
fn the_duration_template_ends_on_the_nth_work_day() {
    let cal = WorkCalendar::default();
    let mon = d(2026, 11, 2);
    assert_eq!(end_from_duration(mon, 10, &cal).unwrap(), d(2026, 11, 13), "zwei Wochen");
    assert_eq!(end_from_duration(mon, 5, &cal).unwrap(), d(2026, 11, 6));
    assert_eq!(end_from_duration(mon, 1, &cal).unwrap(), mon, "ein Tag endet am Starttag");
    assert_eq!(end_from_duration(mon, 6, &cal).unwrap(), d(2026, 11, 9), "das Wochenende zählt nicht");
}

#[test]
fn the_duration_template_starts_counting_on_the_next_work_day() {
    let cal = WorkCalendar::default();
    assert_eq!(end_from_duration(d(2026, 11, 7), 5, &cal).unwrap(), d(2026, 11, 13), "Samstag: gezählt wird ab Montag");
    assert_eq!(end_from_duration(d(2026, 11, 7), 1, &cal).unwrap(), d(2026, 11, 9));
}

#[test]
fn the_duration_template_respects_holidays_and_the_weekend_switch() {
    let cal = WorkCalendar::default().with_holidays(vec![d(2026, 11, 4)]);
    assert_eq!(end_from_duration(d(2026, 11, 2), 5, &cal).unwrap(), d(2026, 11, 9), "ein freier Mittwoch schiebt das Ende");
    let every = WorkCalendar::with_weekend_switch(false);
    assert_eq!(end_from_duration(d(2026, 11, 2), 7, &every).unwrap(), d(2026, 11, 8), "ohne Wochenend-Ausschluss zählen alle Tage");
}

#[test]
fn the_duration_template_needs_work_days_and_a_sane_length() {
    let none = WorkCalendar { work_days: [false; 7], holidays: vec![] };
    assert_eq!(end_from_duration(d(2026, 11, 2), 5, &none), Err(SprintError::NoWorkDays));
    let cal = WorkCalendar::default();
    assert_eq!(end_from_duration(d(2026, 11, 2), 0, &cal), Err(SprintError::DaysInvalid { max: 366 }));
    assert_eq!(end_from_duration(d(2026, 11, 2), 367, &cal), Err(SprintError::DaysInvalid { max: 366 }));
    assert!(end_from_duration(d(2026, 11, 2), 366, &cal).is_ok());
}

#[test]
fn suggestions_continue_after_the_last_sprint() {
    let s = S::new();
    let today = d(2026, 10, 30);
    // Ohne Sprints: der nächste Arbeitstag ab heute (Freitag), zwei Wochen.
    assert_eq!(suggest_dates(s.c(), s.p(), 10, &s.cal, today).unwrap(), (d(2026, 10, 30), d(2026, 11, 12)));
    assert_eq!(suggest_name(s.c(), s.p()).unwrap(), "Sprint 1");
    s.dated("Sprint 1");
    // Danach: der Tag nach dem Ende (Fr 13.11. -> Sa 14.11. -> Montag 16.11.).
    assert_eq!(suggest_dates(s.c(), s.p(), 5, &s.cal, today).unwrap(), (d(2026, 11, 16), d(2026, 11, 20)));
    assert_eq!(suggest_name(s.c(), s.p()).unwrap(), "Sprint 2");
    s.sprint("Sprint 2");
    s.sprint("sprint 3");
    assert_eq!(suggest_name(s.c(), s.p()).unwrap(), "Sprint 4", "der erste freie Name, Groß-/Kleinschreibung egal");
    // Liegt das Ende in der Vergangenheit, zählt heute.
    assert_eq!(suggest_dates(s.c(), s.p(), 5, &s.cal, d(2027, 1, 4)).unwrap().0, d(2027, 1, 4));
}

// ------------------------------------------------------------------ Anlegen, Ändern, Löschen

#[test]
fn creates_a_planned_sprint_with_defaults() {
    let s = S::new();
    let id = s.sprint("  Sprint   1 ");
    let sp = get_sprint(s.c(), id).unwrap();
    assert_eq!(sp.name, "Sprint 1");
    assert_eq!((sp.state, sp.goal.as_str(), sp.start_date, sp.end_date), (SprintState::Planned, "", None, None));
    assert_eq!(sp.default_days, DEFAULT_SPRINT_DAYS);
    assert_eq!(sp.created_at, "2026-10-30T09:00:00");
    assert_eq!((sp.started_at.clone(), sp.completed_at.clone()), (None, None));
    assert_eq!(list_sprints(s.c(), s.p()).unwrap(), vec![sp]);
}

#[test]
fn a_start_without_an_end_uses_the_duration_template() {
    let s = S::new();
    let mut n = NewSprint::new(s.p(), "S");
    n.start = Some(d(2026, 11, 2));
    n.default_days = Some(5);
    let sp = get_sprint(s.c(), create_sprint(s.c(), &n, &s.cal).unwrap()).unwrap();
    assert_eq!(sp.end_date, Some(d(2026, 11, 6)));
    assert_eq!(sp.default_days, 5);
    // Standarddauer ohne Angabe: zehn Arbeitstage.
    let mut n = NewSprint::new(s.p(), "T");
    n.start = Some(d(2026, 11, 2));
    assert_eq!(get_sprint(s.c(), create_sprint(s.c(), &n, &s.cal).unwrap()).unwrap().end_date, Some(d(2026, 11, 13)));
}

#[test]
fn explicit_dates_win_over_the_template_and_are_validated() {
    let s = S::new();
    let mut n = NewSprint::new(s.p(), "S");
    n.start = Some(d(2026, 11, 2));
    n.end = Some(d(2026, 11, 3));
    let sp = get_sprint(s.c(), create_sprint(s.c(), &n, &s.cal).unwrap()).unwrap();
    assert_eq!(sp.end_date, Some(d(2026, 11, 3)));
    let mut bad = NewSprint::new(s.p(), "B");
    bad.start = Some(d(2026, 11, 5));
    bad.end = Some(d(2026, 11, 4));
    assert_eq!(create_sprint(s.c(), &bad, &s.cal), Err(SprintError::DatesOrder));
    let mut only_end = NewSprint::new(s.p(), "E");
    only_end.end = Some(d(2026, 11, 4));
    assert_eq!(create_sprint(s.c(), &only_end, &s.cal), Err(SprintError::DatesIncomplete));
    let mut days = NewSprint::new(s.p(), "D");
    days.default_days = Some(0);
    assert_eq!(create_sprint(s.c(), &days, &s.cal), Err(SprintError::DaysInvalid { max: 366 }));
    assert_eq!(list_sprints(s.c(), s.p()).unwrap().len(), 1, "Fehlschläge legen nichts an");
}

#[test]
fn names_are_cleaned_and_unique_per_project() {
    let s = S::new();
    s.sprint("Alpha");
    assert_eq!(create_sprint(s.c(), &NewSprint::new(s.p(), "ALPHA"), &s.cal), Err(SprintError::Plan(PlanError::NameTaken)));
    assert_eq!(create_sprint(s.c(), &NewSprint::new(s.p(), "   "), &s.cal), Err(SprintError::Plan(PlanError::NameEmpty)));
    assert!(matches!(create_sprint(s.c(), &NewSprint::new(s.p(), &"x".repeat(81)), &s.cal), Err(SprintError::Plan(PlanError::NameTooLong { .. }))));
    // In einem anderen Projekt ist derselbe Name erlaubt.
    let other = s.other("OTH");
    assert!(create_sprint(s.c(), &NewSprint::new(other, "Alpha"), &s.cal).is_ok());
}

#[test]
fn the_goal_is_trimmed_and_limited() {
    let s = S::new();
    let mut n = NewSprint::new(s.p(), "S");
    n.goal = "  Release vorbereiten \n".into();
    assert_eq!(get_sprint(s.c(), create_sprint(s.c(), &n, &s.cal).unwrap()).unwrap().goal, "Release vorbereiten");
    n.name = "T".into();
    n.goal = "x".repeat(MAX_GOAL_CHARS + 1);
    assert_eq!(create_sprint(s.c(), &n, &s.cal), Err(SprintError::TextTooLong { max: MAX_GOAL_CHARS }));
    n.goal = "x".repeat(MAX_GOAL_CHARS);
    assert!(create_sprint(s.c(), &n, &s.cal).is_ok());
}

#[test]
fn updates_change_fields_and_validate() {
    let s = S::new();
    let id = s.dated("Alt");
    s.sprint("Andere");
    update_sprint(s.c(), id, &UpdateSprint { name: Some(" Neu ".into()), goal: Some("Ziel".into()), default_days: Some(7), ..Default::default() }).unwrap();
    let sp = get_sprint(s.c(), id).unwrap();
    assert_eq!((sp.name.as_str(), sp.goal.as_str(), sp.default_days), ("Neu", "Ziel", 7));
    assert_eq!(sp.start_date, Some(d(2026, 11, 2)), "Termine bleiben, wenn sie nicht genannt werden");
    update_sprint(s.c(), id, &UpdateSprint { start: Some(Some(d(2026, 11, 9))), end: Some(Some(d(2026, 11, 20))), ..Default::default() }).unwrap();
    assert_eq!(get_sprint(s.c(), id).unwrap().end_date, Some(d(2026, 11, 20)));
    assert_eq!(update_sprint(s.c(), id, &UpdateSprint { end: Some(Some(d(2026, 11, 1))), ..Default::default() }), Err(SprintError::DatesOrder));
    assert_eq!(update_sprint(s.c(), id, &UpdateSprint { name: Some("andere".into()), ..Default::default() }), Err(SprintError::Plan(PlanError::NameTaken)));
    assert!(update_sprint(s.c(), id, &UpdateSprint { name: Some("NEU".into()), ..Default::default() }).is_ok(), "der eigene Name zählt nicht als belegt");
    // Ein geplanter Sprint darf die Termine verlieren, ein Ende ohne Start geht nicht.
    update_sprint(s.c(), id, &UpdateSprint { start: Some(None), end: Some(None), ..Default::default() }).unwrap();
    assert_eq!(get_sprint(s.c(), id).unwrap().start_date, None);
    assert_eq!(update_sprint(s.c(), id, &UpdateSprint { end: Some(Some(d(2026, 11, 20))), ..Default::default() }), Err(SprintError::DatesIncomplete));
    assert!(update_sprint(s.c(), 9999, &UpdateSprint::default()).unwrap_err().is_not_found());
}

#[test]
fn completed_sprints_are_read_only_and_active_ones_keep_their_dates() {
    let s = S::new();
    let id = s.dated("S");
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), id, &s.cal).unwrap();
    assert_eq!(update_sprint(s.c(), id, &UpdateSprint { end: Some(None), ..Default::default() }), Err(SprintError::DatesIncomplete));
    assert!(update_sprint(s.c(), id, &UpdateSprint { goal: Some("neu".into()), ..Default::default() }).is_ok());
    complete_sprint(s.c(), id, Leftover::ToBacklog).unwrap();
    assert_eq!(update_sprint(s.c(), id, &UpdateSprint { goal: Some("x".into()), ..Default::default() }), Err(SprintError::SprintCompleted));
}

#[test]
fn only_planned_sprints_can_be_deleted_and_their_tickets_return_to_the_backlog() {
    let s = S::new();
    let a = s.dated("A");
    let t = s.task("T", Some(3.0));
    move_tickets_to_sprint(s.c(), a, &[t.id]).unwrap();
    assert_eq!(sprint_of(s.c(), t.id), Some(a));
    delete_sprint(s.c(), a).unwrap();
    assert!(get_sprint(s.c(), a).unwrap_err().is_not_found());
    assert_eq!(sprint_of(s.c(), t.id), None);
    assert_eq!(backlog(s.c(), s.p(), true).unwrap().len(), 1);
    assert_eq!(all_rows(s.c(), a), 0, "die Zeilen verschwinden mit dem Sprint");
    let b = s.dated("B");
    start_sprint(s.c(), b, &s.cal).unwrap();
    assert_eq!(delete_sprint(s.c(), b), Err(SprintError::NotPlanned));
    complete_sprint(s.c(), b, Leftover::ToBacklog).unwrap();
    assert_eq!(delete_sprint(s.c(), b), Err(SprintError::NotPlanned));
    assert!(delete_sprint(s.c(), 9999).unwrap_err().is_not_found());
}

// ------------------------------------------------------------------ Starten und Abschließen

#[test]
fn starting_activates_the_sprint_and_records_the_time() {
    let s = S::new();
    let id = s.dated("S");
    s.at("2026-11-02T08:15:00");
    let sp = start_sprint(s.c(), id, &s.cal).unwrap();
    assert_eq!(sp.state, SprintState::Active);
    assert_eq!(sp.started_at.as_deref(), Some("2026-11-02T08:15:00"));
    assert_eq!(active_sprint(s.c(), s.p()).unwrap().id, id);
    assert_eq!(start_sprint(s.c(), id, &s.cal), Err(SprintError::NotPlanned), "ein aktiver Sprint lässt sich nicht noch einmal starten");
}

#[test]
fn starting_fills_in_missing_dates() {
    let s = S::new();
    let id = s.sprint("S");
    s.at("2026-11-04T10:00:00"); // Mittwoch
    let sp = start_sprint(s.c(), id, &s.cal).unwrap();
    assert_eq!(sp.start_date, Some(d(2026, 11, 4)), "ohne Start gilt heute");
    assert_eq!(sp.end_date, Some(d(2026, 11, 17)), "zehn Arbeitstage ab Mittwoch");
    // Start vorhanden, Ende fehlt: aus der Dauer-Vorlage.
    complete_sprint(s.c(), id, Leftover::ToBacklog).unwrap();
    let mut n = NewSprint::new(s.p(), "T");
    n.default_days = Some(3);
    let t = create_sprint(s.c(), &n, &s.cal).unwrap();
    s.c().execute("UPDATE sprints SET start_date = '2026-11-02' WHERE id = ?1", [t]).unwrap();
    assert_eq!(start_sprint(s.c(), t, &s.cal).unwrap().end_date, Some(d(2026, 11, 4)));
}

#[test]
fn only_one_sprint_can_be_active_per_project() {
    let s = S::new();
    let (a, b) = (s.dated("A"), s.dated("B"));
    start_sprint(s.c(), a, &s.cal).unwrap();
    assert_eq!(start_sprint(s.c(), b, &s.cal), Err(SprintError::AnotherActive { sprint_id: a }));
    assert_eq!(get_sprint(s.c(), b).unwrap().state, SprintState::Planned, "der Fehlversuch ändert nichts");
    complete_sprint(s.c(), a, Leftover::ToBacklog).unwrap();
    assert!(start_sprint(s.c(), b, &s.cal).is_ok());
    // In einem anderen Projekt darf gleichzeitig einer laufen.
    let other = s.other("OTH");
    let o = create_sprint(s.c(), &NewSprint::new(other, "O"), &s.cal).unwrap();
    assert!(start_sprint(s.c(), o, &s.cal).is_ok());
}

#[test]
fn completing_needs_the_active_sprint() {
    let s = S::new();
    let id = s.dated("S");
    assert_eq!(complete_sprint(s.c(), id, Leftover::ToBacklog), Err(SprintError::NotActive));
    start_sprint(s.c(), id, &s.cal).unwrap();
    complete_sprint(s.c(), id, Leftover::ToBacklog).unwrap();
    assert_eq!(complete_sprint(s.c(), id, Leftover::ToBacklog), Err(SprintError::NotActive));
    assert!(complete_sprint(s.c(), 9999, Leftover::ToBacklog).unwrap_err().is_not_found());
}

/// Drei Tickets im aktiven Sprint: eines erledigt, eines in Arbeit, eines offen.
fn active_with_three(s: &S) -> (i64, [Ticket; 3]) {
    let sp = s.dated("S1");
    let done = s.task("erledigt", Some(3.0));
    let wip = s.task("in Arbeit", Some(5.0));
    let open = s.task("offen", None);
    move_tickets_to_sprint(s.c(), sp, &[done.id, wip.id, open.id]).unwrap();
    start_sprint(s.c(), sp, &s.cal).unwrap();
    s.status(&done, s.f.done, "2026-11-03T10:00:00");
    s.status(&wip, s.f.progress, "2026-11-03T11:00:00");
    (sp, [done, wip, open])
}

#[test]
fn completing_to_the_backlog_frees_the_unfinished_tickets() {
    let s = S::new();
    let (sp, [done, wip, open]) = active_with_three(&s);
    s.at("2026-11-13T17:00:00");
    let r = complete_sprint(s.c(), sp, Leftover::ToBacklog).unwrap();
    assert_eq!(r.sprint.state, SprintState::Completed);
    assert_eq!(r.sprint.completed_at.as_deref(), Some("2026-11-13T17:00:00"));
    assert_eq!(r.done, vec![done.id]);
    assert_eq!(r.leftover, vec![wip.id, open.id], "nach Rang");
    for t in [&done, &wip, &open] {
        assert_eq!(sprint_of(s.c(), t.id), None, "kein offener Sprint mehr");
    }
    let b: Vec<i64> = backlog(s.c(), s.p(), false).unwrap().iter().map(|t| t.id).collect();
    assert_eq!(b, vec![wip.id, open.id], "ohne die erledigten");
    assert_eq!(backlog(s.c(), s.p(), true).unwrap().len(), 3);
    assert!(active_sprint(s.c(), s.p()).is_none());
    // Die Zeilen bleiben für den Bericht.
    assert_eq!(open_rows(s.c(), sp), 3);
}

#[test]
fn completing_to_a_sprint_moves_the_unfinished_tickets_there() {
    let s = S::new();
    let (sp, [done, wip, open]) = active_with_three(&s);
    let next = s.sprint("S2");
    s.at("2026-11-13T17:00:00");
    let r = complete_sprint(s.c(), sp, Leftover::ToSprint(next)).unwrap();
    assert_eq!(r.leftover, vec![wip.id, open.id]);
    assert_eq!(sprint_of(s.c(), wip.id), Some(next));
    assert_eq!(sprint_of(s.c(), open.id), Some(next));
    assert_eq!(sprint_of(s.c(), done.id), None, "Erledigtes bleibt im alten Sprint");
    let c = sprint_content(s.c(), next).unwrap();
    assert_eq!(c.items.len(), 2);
    assert_eq!(c.items[0].added_at, "2026-11-13T17:00:00");
    assert_eq!(c.items[0].points, Some(5.0), "die Punkte werden beim Hinzufügen festgehalten");
    // Der alte Sprint zeigt weiter seinen Stand beim Abschluss.
    assert_eq!(sprint_content(s.c(), sp).unwrap().items.len(), 3);
}

#[test]
fn the_leftover_target_must_be_a_planned_sprint_of_the_same_project() {
    let s = S::new();
    let (sp, _) = active_with_three(&s);
    let other_project = s.other("OTH");
    let foreign = create_sprint(s.c(), &NewSprint::new(other_project, "F"), &s.cal).unwrap();
    let finished = s.dated("Fertig");
    // „Fertig“ wird direkt auf abgeschlossen gesetzt, damit der aktive Sprint nicht berührt wird.
    s.c().execute("UPDATE sprints SET state = 'completed' WHERE id = ?1", [finished]).unwrap();
    for bad in [sp, foreign, finished, 9999] {
        assert_eq!(complete_sprint(s.c(), sp, Leftover::ToSprint(bad)), Err(SprintError::LeftoverInvalid), "{bad}");
    }
    assert_eq!(get_sprint(s.c(), sp).unwrap().state, SprintState::Active, "der Fehlversuch ändert nichts");
    assert_eq!(open_rows(s.c(), foreign), 0);
}

#[test]
fn tickets_without_a_status_count_as_unfinished() {
    let s = S::new();
    let sp = s.dated("S");
    let t = s.task("x", None);
    move_tickets_to_sprint(s.c(), sp, &[t.id]).unwrap();
    s.c().execute("UPDATE tickets SET status_id = NULL WHERE id = ?1", [t.id]).unwrap();
    start_sprint(s.c(), sp, &s.cal).unwrap();
    let r = complete_sprint(s.c(), sp, Leftover::ToBacklog).unwrap();
    assert_eq!((r.done.len(), r.leftover), (0, vec![t.id]));
}

// ------------------------------------------------------------------ Zugehörigkeit der Tickets

#[test]
fn tickets_move_into_and_out_of_a_sprint() {
    let s = S::new();
    let sp = s.dated("S");
    let (a, b) = (s.task("A", Some(2.0)), s.task("B", None));
    assert_eq!(move_tickets_to_sprint(s.c(), sp, &s.ids(&[&a, &b])).unwrap(), 2);
    assert_eq!((sprint_of(s.c(), a.id), sprint_of(s.c(), b.id)), (Some(sp), Some(sp)));
    assert!(backlog(s.c(), s.p(), true).unwrap().is_empty());
    assert_eq!(move_tickets_to_backlog(s.c(), &[a.id]).unwrap(), 1);
    assert_eq!(sprint_of(s.c(), a.id), None);
    assert_eq!(backlog(s.c(), s.p(), true).unwrap()[0].id, a.id);
    assert_eq!(move_tickets_to_backlog(s.c(), &[a.id]).unwrap(), 0, "ein Ticket im Backlog bewegt sich nicht");
}

#[test]
fn a_ticket_is_in_at_most_one_open_sprint() {
    let s = S::new();
    let (x, y) = (s.dated("X"), s.dated("Y"));
    let t = s.task("T", Some(1.0));
    move_tickets_to_sprint(s.c(), x, &[t.id]).unwrap();
    assert_eq!(move_tickets_to_sprint(s.c(), y, &[t.id]).unwrap(), 1);
    assert_eq!(sprint_of(s.c(), t.id), Some(y));
    assert_eq!((open_rows(s.c(), x), open_rows(s.c(), y)), (0, 1), "im geplanten Sprint X ist die Zeile weg");
    let open_total: i64 = s
        .c()
        .query_row(
            "SELECT COUNT(*) FROM sprint_tickets st JOIN sprints sp ON sp.id = st.sprint_id WHERE st.ticket_id = ?1 AND st.removed_at IS NULL AND sp.state IN ('planned', 'active')",
            [t.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(open_total, 1);
}

#[test]
fn moving_between_sprints_keeps_the_history_of_an_active_sprint() {
    let s = S::new();
    let (x, y) = (s.dated("X"), s.dated("Y"));
    let t = s.task("T", Some(4.0));
    move_tickets_to_sprint(s.c(), x, &[t.id]).unwrap();
    start_sprint(s.c(), x, &s.cal).unwrap();
    s.at("2026-11-04T09:00:00");
    move_tickets_to_sprint(s.c(), y, &[t.id]).unwrap();
    let removed: Option<String> = s.c().query_row("SELECT removed_at FROM sprint_tickets WHERE sprint_id = ?1", [x], |r| r.get(0)).unwrap();
    assert_eq!(removed.as_deref(), Some("2026-11-04T09:00:00"), "aus dem aktiven Sprint entfernt: Zeile mit Zeitpunkt");
    assert_eq!(sprint_content(s.c(), x).unwrap().items.len(), 0);
    assert_eq!(sprint_content(s.c(), y).unwrap().items.len(), 1);
    // Zurück in den ersten Sprint: eine neue Zeile.
    move_tickets_to_backlog(s.c(), &[t.id]).unwrap();
    s.at("2026-11-05T09:00:00");
    move_tickets_to_sprint(s.c(), x, &[t.id]).unwrap();
    assert_eq!(all_rows(s.c(), x), 2);
    assert_eq!(open_rows(s.c(), x), 1);
}

#[test]
fn moving_several_tickets_is_all_or_nothing() {
    let s = S::new();
    let sp = s.dated("S");
    let (a, b) = (s.task("A", None), s.task("B", None));
    let other = s.f.other_project("OTH");
    let ty = crate::planning::types::list_types(s.c(), other).unwrap()[0].id;
    let (foreign, _) = create_ticket(s.c(), &NewTicket::new(other, ty, "F"), &Source::ui()).unwrap();
    let r = move_tickets_to_sprint(s.c(), sp, &[a.id, foreign.id, b.id]);
    assert_eq!(r, Err(SprintError::TicketOtherProject { ticket_id: foreign.id }));
    assert_eq!(open_rows(s.c(), sp), 0, "auch das erste Ticket ist nicht drin");
    assert!(move_tickets_to_sprint(s.c(), sp, &[a.id, 9999]).unwrap_err().is_not_found());
    assert_eq!(open_rows(s.c(), sp), 0);
}

#[test]
fn moving_ignores_duplicates_and_tickets_already_there() {
    let s = S::new();
    let sp = s.dated("S");
    let (a, b) = (s.task("A", None), s.task("B", None));
    assert_eq!(move_tickets_to_sprint(s.c(), sp, &[a.id, a.id, b.id, a.id]).unwrap(), 2);
    assert_eq!(move_tickets_to_sprint(s.c(), sp, &[a.id, b.id]).unwrap(), 0);
    assert_eq!(all_rows(s.c(), sp), 2);
    assert_eq!(move_tickets_to_sprint(s.c(), sp, &[]).unwrap(), 0);
}

#[test]
fn completed_sprints_take_no_tickets() {
    let s = S::new();
    let sp = s.dated("S");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    complete_sprint(s.c(), sp, Leftover::ToBacklog).unwrap();
    let t = s.task("T", None);
    assert_eq!(move_tickets_to_sprint(s.c(), sp, &[t.id]), Err(SprintError::SprintCompleted));
    assert!(move_tickets_to_sprint(s.c(), 9999, &[t.id]).unwrap_err().is_not_found());
}

#[test]
fn removing_from_a_planned_sprint_leaves_no_trace_but_from_an_active_one_does() {
    let s = S::new();
    let sp = s.dated("S");
    let (a, b) = (s.task("A", None), s.task("B", None));
    move_tickets_to_sprint(s.c(), sp, &[a.id, b.id]).unwrap();
    move_tickets_to_backlog(s.c(), &[a.id]).unwrap();
    assert_eq!(all_rows(s.c(), sp), 1);
    start_sprint(s.c(), sp, &s.cal).unwrap();
    s.at("2026-11-03T12:00:00");
    move_tickets_to_backlog(s.c(), &[b.id]).unwrap();
    assert_eq!((all_rows(s.c(), sp), open_rows(s.c(), sp)), (1, 0));
}

#[test]
fn the_backlog_follows_the_rank_and_excludes_sprint_tickets() {
    let s = S::new();
    let (a, b, c) = (s.task("A", None), s.task("B", None), s.task("C", None));
    let ids = |s: &S| backlog(s.c(), s.p(), true).unwrap().iter().map(|t| t.id).collect::<Vec<_>>();
    assert_eq!(ids(&s), vec![a.id, b.id, c.id]);
    move_rank(s.c(), c.id, Some(a.id)).unwrap();
    assert_eq!(ids(&s), vec![c.id, a.id, b.id], "die Reihenfolge ist der Rang der Tickets");
    let sp = s.dated("S");
    move_tickets_to_sprint(s.c(), sp, &[a.id]).unwrap();
    assert_eq!(ids(&s), vec![c.id, b.id]);
    move_tickets_to_backlog(s.c(), &[a.id]).unwrap();
    assert_eq!(ids(&s), vec![c.id, a.id, b.id], "das Ticket kehrt an seinen Rang zurück");
}

#[test]
fn the_backlog_can_hide_finished_tickets() {
    let s = S::new();
    let (a, b) = (s.task("A", None), s.task("B", None));
    s.status(&a, s.f.done, "2026-10-30T10:00:00");
    assert_eq!(backlog(s.c(), s.p(), false).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![b.id]);
    assert_eq!(backlog(s.c(), s.p(), true).unwrap().len(), 2);
    // Ein Ticket ohne Status ist nicht erledigt.
    s.c().execute("UPDATE tickets SET status_id = NULL WHERE id = ?1", [a.id]).unwrap();
    assert_eq!(backlog(s.c(), s.p(), false).unwrap().len(), 2);
}

#[test]
fn the_content_sums_points_and_falls_back_to_the_ticket_count() {
    let s = S::new();
    let sp = s.dated("S");
    let (a, b, c) = (s.task("A", Some(3.0)), s.task("B", Some(5.0)), s.task("C", None));
    move_tickets_to_sprint(s.c(), sp, &[a.id, b.id, c.id]).unwrap();
    s.status(&b, s.f.done, "2026-10-30T10:00:00");
    let content = sprint_content(s.c(), sp).unwrap();
    assert_eq!(content.items.iter().map(|i| (i.ticket.id, i.points, i.done)).collect::<Vec<_>>(), vec![(a.id, Some(3.0), false), (b.id, Some(5.0), true), (c.id, None, false)]);
    let t = content.totals;
    assert_eq!((t.total.tickets, t.total.points, t.total.without_points), (3, 8.0, 1));
    assert_eq!((t.done.tickets, t.done.points, t.done.without_points), (1, 5.0, 0));
    assert_eq!(t.total.weight(Basis::Points), 8.0);
    assert_eq!(t.total.weight(Basis::TicketCount), 3.0);
    assert_eq!(t.total.weight(Basis::PointsWithFallbackOne), 9.0, "ein Ticket ohne Punkte zählt 1");
    assert_eq!(sprint_totals(s.c(), sp).unwrap(), t);
}

#[test]
fn story_points_are_optional_everywhere() {
    // Epics und Defects haben kein Story-Points-Feld: alles funktioniert und zählt über die Anzahl.
    let s = S::new();
    let sp = s.dated("S");
    let e = s.f.mk(s.f.epic, "Epic");
    let defect = s.f.mk(s.f.defect, "Defect");
    move_tickets_to_sprint(s.c(), sp, &[e.id, defect.id]).unwrap();
    let t = sprint_content(s.c(), sp).unwrap().totals;
    assert_eq!((t.total.tickets, t.total.points, t.total.without_points), (2, 0.0, 2));
    assert_eq!(t.total.weight(Basis::PointsWithFallbackOne), 2.0);
    assert_eq!(ticket_points(s.c(), e.id).unwrap(), None);
}

#[test]
fn the_active_sprint_hooks_for_the_search() {
    let s = S::new();
    assert!(sprint_ids_active(s.c(), None).is_empty());
    let a = s.dated("A");
    assert!(sprint_ids_active(s.c(), Some(s.p())).is_empty(), "geplant ist nicht aktiv");
    start_sprint(s.c(), a, &s.cal).unwrap();
    assert_eq!(sprint_ids_active(s.c(), None), vec![a]);
    assert_eq!(sprint_ids_active(s.c(), Some(s.p())), vec![a]);
    assert!(sprint_ids_active(s.c(), Some(9999)).is_empty());
    let other = s.other("OTH");
    let o = create_sprint(s.c(), &NewSprint::new(other, "O"), &s.cal).unwrap();
    start_sprint(s.c(), o, &s.cal).unwrap();
    assert_eq!(sprint_ids_active(s.c(), None), vec![a, o]);
    assert_eq!(sprint_ids_active(s.c(), Some(other)), vec![o]);
    complete_sprint(s.c(), a, Leftover::ToBacklog).unwrap();
    assert_eq!(sprint_ids_active(s.c(), None), vec![o]);
}

// ------------------------------------------------------------------ Optionalität

#[test]
fn everything_refuses_a_project_without_sprints() {
    let s = S::new();
    let sp = s.dated("S");
    let t = s.task("T", Some(2.0));
    move_tickets_to_sprint(s.c(), sp, &[t.id]).unwrap();
    update_project(s.c(), s.p(), &UpdateProject { use_sprints: Some(false), ..Default::default() }).unwrap();
    let off = Err(SprintError::SprintsDisabled);
    assert_eq!(create_sprint(s.c(), &NewSprint::new(s.p(), "N"), &s.cal).map(|_| ()), off);
    assert_eq!(update_sprint(s.c(), sp, &UpdateSprint::default()).map(|_| ()), off);
    assert_eq!(delete_sprint(s.c(), sp).map(|_| ()), off);
    assert_eq!(start_sprint(s.c(), sp, &s.cal).map(|_| ()), off);
    assert_eq!(complete_sprint(s.c(), sp, Leftover::ToBacklog).map(|_| ()), off);
    assert_eq!(move_tickets_to_sprint(s.c(), sp, &[t.id]).map(|_| ()), off);
    assert_eq!(move_tickets_to_backlog(s.c(), &[t.id]).map(|_| ()), off);
    assert_eq!(list_sprints(s.c(), s.p()).map(|_| ()), off);
    assert_eq!(backlog(s.c(), s.p(), true).map(|_| ()), off);
    assert_eq!(sprint_content(s.c(), sp).map(|_| ()), off);
    assert_eq!(suggest_name(s.c(), s.p()).map(|_| ()), off);
    assert_eq!(suggest_dates(s.c(), s.p(), 5, &s.cal, d(2026, 11, 2)).map(|_| ()), off);
    assert_eq!(sprint_report(s.c(), sp).map(|_| ()), off);
    assert_eq!(burndown_for_sprint(s.c(), sp, Basis::default(), s.cal.clone(), d(2026, 11, 2)).map(|_| ()), off);
    assert_eq!(velocity_for_project(s.c(), s.p(), 3, Basis::default()).map(|_| ()), off);
    // Einfache Leser und die Suchhaken bleiben still.
    assert_eq!(get_sprint(s.c(), sp).unwrap().name, "S");
    assert_eq!(sprint_of(s.c(), t.id), None);
    assert!(active_sprint(s.c(), s.p()).is_none());
    assert!(sprint_ids_active(s.c(), None).is_empty());
}

#[test]
fn the_data_comes_back_when_sprints_are_switched_on_again() {
    let s = S::new();
    let sp = s.dated("S");
    let t = s.task("T", Some(2.0));
    move_tickets_to_sprint(s.c(), sp, &[t.id]).unwrap();
    start_sprint(s.c(), sp, &s.cal).unwrap();
    update_project(s.c(), s.p(), &UpdateProject { use_sprints: Some(false), ..Default::default() }).unwrap();
    update_project(s.c(), s.p(), &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
    assert_eq!(active_sprint(s.c(), s.p()).unwrap().id, sp);
    assert_eq!(sprint_of(s.c(), t.id), Some(sp));
    assert_eq!(sprint_content(s.c(), sp).unwrap().items.len(), 1);
}

#[test]
fn the_system_project_never_has_sprints() {
    let s = S::new();
    let todo = crate::planning::projects::project_by_key(s.c(), "TODO").unwrap();
    assert_eq!(create_sprint(s.c(), &NewSprint::new(todo.id, "S"), &s.cal), Err(SprintError::SprintsDisabled));
}

// ------------------------------------------------------------------ Aufräumen

#[test]
fn deleting_a_ticket_removes_its_rows() {
    let s = S::new();
    let sp = s.dated("S");
    let t = s.task("T", None);
    move_tickets_to_sprint(s.c(), sp, &[t.id]).unwrap();
    delete_ticket(s.c(), t.id, &Source::ui()).unwrap();
    assert_eq!(all_rows(s.c(), sp), 0);
    assert!(sprint_content(s.c(), sp).unwrap().items.is_empty());
}

#[test]
fn deleting_a_project_removes_its_sprints_versions_and_milestones() {
    let s = S::new();
    let other = s.other("OTH");
    let sp = create_sprint(s.c(), &NewSprint::new(other, "S"), &s.cal).unwrap();
    releases::create_version(s.c(), &releases::NewRelease::new(other, "1.0")).unwrap();
    releases::create_milestone(s.c(), &releases::NewRelease::new(other, "M1")).unwrap();
    crate::planning::projects::delete_project(s.c(), other).unwrap();
    assert!(get_sprint(s.c(), sp).unwrap_err().is_not_found());
    for t in ["sprints", "versions", "milestones"] {
        let n: i64 = s.c().query_row(&format!("SELECT COUNT(*) FROM {t} WHERE project_id = ?1"), [other], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "{t}");
    }
}

fn devtask_of(c: &Connection, project: i64) -> i64 {
    crate::planning::types::list_types(c, project).unwrap().into_iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id
}

#[test]
fn a_ticket_that_changes_the_project_leaves_its_open_sprint() {
    let s = S::new();
    let sp = s.dated("S");
    let t = s.task("T", Some(3.0));
    move_tickets_to_sprint(s.c(), sp, &[t.id]).unwrap();
    let other = s.f.other_project("OTH");
    crate::planning::clone_move::change_type(s.c(), t.id, devtask_of(s.c(), other), &[], &[], &Source::ui()).unwrap();
    assert_eq!(crate::planning::tickets::get_ticket(s.c(), t.id).unwrap().project_id, other);
    assert_eq!(sprint_of(s.c(), t.id), None);
    assert_eq!(open_rows(s.c(), sp), 0);
}

#[test]
fn a_ticket_leaving_an_active_sprint_by_project_change_keeps_a_removal_row() {
    let s = S::new();
    let sp = s.dated("S");
    let t = s.task("T", Some(3.0));
    move_tickets_to_sprint(s.c(), sp, &[t.id]).unwrap();
    start_sprint(s.c(), sp, &s.cal).unwrap();
    s.at("2026-11-04T09:00:00");
    let other = s.f.other_project("OTH");
    crate::planning::clone_move::change_type(s.c(), t.id, devtask_of(s.c(), other), &[], &[], &Source::ui()).unwrap();
    let removed: Option<String> = s.c().query_row("SELECT removed_at FROM sprint_tickets WHERE sprint_id = ?1", [sp], |r| r.get(0)).unwrap();
    assert_eq!(removed.as_deref(), Some("2026-11-04T09:00:00"));
}

// ------------------------------------------------------------------ Zweisprachigkeit und Kleinkram

#[test]
fn state_labels_exist_in_both_languages() {
    for st in [SprintState::Planned, SprintState::Active, SprintState::Completed] {
        assert_ne!(st.label(Lang::De), st.label(Lang::En), "{st:?}");
        assert_eq!(SprintState::parse(st.as_str()), st);
    }
    assert!(SprintState::Active.is_open() && SprintState::Planned.is_open() && !SprintState::Completed.is_open());
    assert_eq!(SprintState::parse("nonsense"), SprintState::Planned);
}

#[test]
fn points_are_read_only_from_the_number_field_story_points() {
    let s = S::new();
    let t = s.task("T", None);
    let text = s.f.add_field(s.f.task, "Punkte als Text", FieldKind::Text, FieldOptions::default(), false);
    set_field(s.c(), t.id, text.id, json!("8"), &Source::ui()).unwrap();
    assert_eq!(ticket_points(s.c(), t.id).unwrap(), None);
    s.set_points(&t, 8.0);
    assert_eq!(ticket_points(s.c(), t.id).unwrap(), Some(8.0));
}

/// Ein Tippfehler im Jahr (0026 statt 2026) darf keinen Sprint mit hunderttausenden Tagen ergeben (Burndown, Diagramm).
#[test]
fn an_absurdly_long_period_is_refused() {
    let s = S::new();
    let mut n = NewSprint::new(s.p(), "Lang");
    n.start = Some(d(26, 11, 2));
    n.end = Some(d(2026, 11, 3));
    assert_eq!(create_sprint(s.c(), &n, &s.cal), Err(SprintError::DatesTooLong { max: MAX_SPRINT_SPAN_DAYS }));
    n.start = Some(d(2026, 11, 3) - chrono::Duration::days(MAX_SPRINT_SPAN_DAYS));
    let id = create_sprint(s.c(), &n, &s.cal).expect("genau die Obergrenze geht");
    assert_eq!(update_sprint(s.c(), id, &UpdateSprint { start: Some(Some(d(1000, 1, 1))), ..Default::default() }), Err(SprintError::DatesTooLong { max: MAX_SPRINT_SPAN_DAYS }));
    assert!(SprintError::DatesTooLong { max: MAX_SPRINT_SPAN_DAYS }.message(Lang::De).contains("4000"));
}
