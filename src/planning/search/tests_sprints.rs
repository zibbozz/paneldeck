//! Prüfungen von Sprints, Versionen und Meilensteinen in der Abfrage (Anschluss an `planning::sprints`, siehe `pending.rs`):
//! alle Operatoren, offene/abgeschlossene/entfernte Zugehörigkeit, `activeSprint()`, Sortierung, Schalter aus, mehrere Projekte.

use super::tests::{ks, Sx};
use super::*;
use crate::calc::calendar::WorkCalendar;
use crate::planning::fields::{FieldKind, FieldOptions};
use crate::planning::history::Source;
use crate::planning::projects::{update_project, UpdateProject};
use crate::planning::sprints::releases::{create_milestone, create_version, reorder_versions, set_version_released, NewRelease};
use crate::planning::sprints::sprint::{complete_sprint, create_sprint, move_tickets_to_backlog, move_tickets_to_sprint, start_sprint, Leftover, NewSprint};
use crate::planning::tickets::set_field;
use crate::query::FieldType;
use serde_json::json;

fn on(s: &Sx, sprints: bool, versions: bool) {
    update_project(&s.f.c, s.f.project, &UpdateProject { use_sprints: Some(sprints), use_versions: Some(versions), ..Default::default() }).unwrap();
}

fn sprint(s: &Sx, name: &str) -> i64 {
    create_sprint(&s.f.c, &NewSprint::new(s.f.project, name), &WorkCalendar::default()).unwrap()
}

fn put(s: &Sx, sprint_id: i64, tickets: &[u32]) {
    let ids: Vec<i64> = tickets.iter().map(|n| s.ids[*n as usize - 1]).collect();
    move_tickets_to_sprint(&s.f.c, sprint_id, &ids).unwrap();
}

/// Projekt PAN mit Sprints: „Sprint 1“ abgeschlossen (PAN-2 erledigt, PAN-9 erledigt, PAN-5 unerledigt → Sprint 3),
/// „Sprint 2“ aktiv (PAN-3, PAN-4, PAN-6; PAN-6 wird wieder entfernt), „Sprint 3“ geplant (PAN-5, PAN-7, PAN-1).
fn with_sprints() -> Sx {
    let s = Sx::new();
    on(&s, true, false);
    let (s1, s2, s3) = (sprint(&s, "Sprint 1"), sprint(&s, "Sprint 2"), sprint(&s, "Sprint 3"));
    put(&s, s1, &[2, 9, 5]);
    start_sprint(&s.f.c, s1, &WorkCalendar::default()).unwrap();
    complete_sprint(&s.f.c, s1, Leftover::ToSprint(s3)).unwrap();
    put(&s, s2, &[3, 4, 6]);
    start_sprint(&s.f.c, s2, &WorkCalendar::default()).unwrap();
    move_tickets_to_backlog(&s.f.c, &[s.ids[5]]).unwrap(); // PAN-6 verlässt den aktiven Sprint (Zeile mit `removed_at`)
    put(&s, s3, &[7, 1]);
    s
}

// ---------------------------------------------------------------------------------------------
// Schema und Schalter
// ---------------------------------------------------------------------------------------------

#[test]
fn the_sprint_field_exists_only_with_sprints_and_lists_their_names() {
    let s = Sx::new();
    assert!(schema_for(&s.f.c, s.scope(), Lang::En).unwrap().field("sprint").is_none());
    assert!(matches!(s.err("sprint IS EMPTY"), SearchError::Invalid { .. }));
    let s = with_sprints();
    let f = schema_for(&s.f.c, s.scope(), Lang::En).unwrap();
    assert_eq!(f.field("sprint").unwrap().ty, FieldType::Option(vec!["Sprint 1".into(), "Sprint 2".into(), "Sprint 3".into()]));
    on(&s, false, false);
    assert!(schema_for(&s.f.c, s.scope(), Lang::En).unwrap().field("sprint").is_none());
    assert!(matches!(s.err("sprint = activeSprint()"), SearchError::Invalid { .. }));
    on(&s, true, false);
    assert_eq!(s.q("sprint = activeSprint()"), ks(&[3, 4]), "die Daten bleiben beim Ausschalten erhalten");
}

#[test]
fn version_fields_exist_only_with_versions_and_list_their_names() {
    let s = Sx::new();
    s.f.add_field(s.f.epic, "Release version", FieldKind::Version, FieldOptions::default(), false);
    assert!(schema_for(&s.f.c, s.scope(), Lang::En).unwrap().field("release version").is_none(), "Schalter aus: kein Feld");
    assert!(matches!(s.err(r#""Release version" IS EMPTY"#), SearchError::Invalid { .. }));
    on(&s, false, true);
    create_version(&s.f.c, &NewRelease::new(s.f.project, "1.0")).unwrap();
    create_version(&s.f.c, &NewRelease::new(s.f.project, "2.0")).unwrap();
    let f = schema_for(&s.f.c, s.scope(), Lang::En).unwrap();
    assert_eq!(f.field("release version").unwrap().ty, FieldType::Option(vec!["1.0".into(), "2.0".into()]));
}

// ---------------------------------------------------------------------------------------------
// Sprint: Operatoren und Zugehörigkeit
// ---------------------------------------------------------------------------------------------

#[test]
fn sprint_equals_covers_planned_active_and_completed_sprints() {
    let s = with_sprints();
    assert_eq!(s.q(r#"sprint = "Sprint 1""#), ks(&[2, 5, 9]), "abgeschlossen: auch das unerledigte Ticket, das weitergereicht wurde");
    assert_eq!(s.q(r#"sprint = "Sprint 2""#), ks(&[3, 4]), "PAN-6 wurde entfernt");
    assert_eq!(s.q(r#"sprint = "Sprint 3""#), ks(&[1, 5, 7]));
    assert_eq!(s.q(r#"sprint = "sprint 2""#), ks(&[3, 4]), "ohne Beachtung der Schreibweise");
}

#[test]
fn sprint_in_and_not_equals_and_not_in() {
    let s = with_sprints();
    assert_eq!(s.q(r#"sprint IN ("Sprint 1", "Sprint 2")"#), ks(&[2, 3, 4, 5, 9]));
    assert_eq!(s.q(r#"sprint != "Sprint 2""#), ks(&[1, 2, 5, 6, 7, 8, 9, 10, 11]), "`!=` trifft auch Tickets ohne Sprint");
    assert_eq!(s.q(r#"sprint NOT IN ("Sprint 1", "Sprint 3")"#), ks(&[3, 4, 6, 8, 10, 11]));
    assert_eq!(s.q(r#"sprint IN ("Sprint 3", NULL)"#), ks(&[1, 2, 5, 6, 7, 8, 9, 10, 11]), "NULL = leer (Backlog)");
}

#[test]
fn sprint_is_empty_is_the_backlog_without_an_open_sprint() {
    let s = with_sprints();
    // offen: Sprint 2 (PAN-3, 4), Sprint 3 (PAN-1, 5, 7); PAN-2 und PAN-9 stecken nur im abgeschlossenen Sprint 1
    assert_eq!(s.q("sprint IS EMPTY"), ks(&[2, 6, 8, 9, 10, 11]));
    assert_eq!(s.q("sprint IS NOT EMPTY"), ks(&[1, 3, 4, 5, 7]));
    assert_eq!(s.q("sprint = NULL"), ks(&[2, 6, 8, 9, 10, 11]));
    assert_eq!(s.q("sprint != EMPTY"), ks(&[1, 3, 4, 5, 7]));
    assert_eq!(s.q("sprint IS EMPTY AND type != Epic AND status != Done"), ks(&[6, 8, 10, 11]));
}

#[test]
fn removed_tickets_do_not_count_any_more() {
    let s = with_sprints();
    assert!(!s.q(r#"sprint = "Sprint 2""#).contains(&"PAN-6".to_string()));
    assert!(s.q("sprint IS EMPTY").contains(&"PAN-6".to_string()));
    // zurück in den aktiven Sprint: eine neue Zeile, die alte bleibt als Historie
    let s2: i64 = s.f.c.query_row("SELECT id FROM sprints WHERE name = 'Sprint 2'", [], |r| r.get(0)).unwrap();
    put(&s, s2, &[6]);
    assert_eq!(s.q(r#"sprint = "Sprint 2""#), ks(&[3, 4, 6]));
}

#[test]
fn a_ticket_can_be_in_the_completed_and_the_next_sprint() {
    let s = with_sprints();
    let q = s.q(r#"sprint = "Sprint 1" AND sprint = "Sprint 3""#);
    assert_eq!(q, ks(&[5]));
}

#[test]
fn active_sprint_resolves_to_the_active_sprint_only() {
    let s = with_sprints();
    assert_eq!(s.q("sprint = activeSprint()"), ks(&[3, 4]));
    assert_eq!(s.q("sprint != activeSprint()"), ks(&[1, 2, 5, 6, 7, 8, 9, 10, 11]));
    assert_eq!(s.q(r#"sprint IN (activeSprint(), "Sprint 3")"#), ks(&[1, 3, 4, 5, 7]));
    assert_eq!(s.q("sprint NOT IN (activeSprint())"), ks(&[1, 2, 5, 6, 7, 8, 9, 10, 11]));
}

#[test]
fn without_an_active_sprint_active_sprint_matches_nothing() {
    let s = with_sprints();
    let s2: i64 = s.f.c.query_row("SELECT id FROM sprints WHERE name = 'Sprint 2'", [], |r| r.get(0)).unwrap();
    complete_sprint(&s.f.c, s2, Leftover::ToBacklog).unwrap();
    assert_eq!(s.q("sprint = activeSprint()"), Vec::<String>::new());
    assert_eq!(s.q("sprint != activeSprint()").len(), 11, "alle Tickets");
    assert_eq!(s.q(r#"sprint = "Sprint 2""#), ks(&[3, 4]), "der abgeschlossene Sprint gilt weiter für seinen Namen");
}

#[test]
fn active_sprint_works_on_other_fields_as_its_name() {
    let s = with_sprints();
    assert_eq!(s.q("summary = activeSprint()"), Vec::<String>::new());
    s.f.mk(s.f.task, "Sprint 2");
    assert_eq!(s.q("summary = activeSprint()").len(), 1);
}

#[test]
fn sprint_sorting_follows_the_newest_sprint_and_puts_empty_last() {
    let s = with_sprints();
    assert_eq!(s.q("sprint IS NOT EMPTY ORDER BY sprint"), ks(&[3, 4, 1, 5, 7]));
    assert_eq!(s.q("sprint IS NOT EMPTY ORDER BY sprint DESC"), ks(&[1, 5, 7, 3, 4]));
    // PAN-2 und PAN-9 stecken nur im (älteren) abgeschlossenen Sprint 1
    let all = s.q("ORDER BY sprint");
    assert_eq!(all, ks(&[2, 9, 3, 4, 1, 5, 7, 6, 8, 10, 11]));
    let desc = s.q("ORDER BY sprint DESC");
    assert_eq!(desc, ks(&[1, 5, 7, 3, 4, 2, 9, 6, 8, 10, 11]), "leere stehen auch absteigend zuletzt");
}

#[test]
fn sprint_names_with_special_characters_are_compared_literally() {
    let s = Sx::new();
    on(&s, true, false);
    for (name, ticket) in [("100%", 1), ("1000", 2), ("a_b", 3), ("axb", 4)] {
        let id = sprint(&s, name);
        put(&s, id, &[ticket]);
    }
    assert_eq!(s.q(r#"sprint = "100%""#), ks(&[1]));
    assert_eq!(s.q(r#"sprint = "1000""#), ks(&[2]));
    assert_eq!(s.q(r#"sprint = "a_b""#), ks(&[3]));
    assert_eq!(s.q(r#"sprint IN ("a_b", axb)"#), ks(&[3, 4]));
}

#[test]
fn sprints_of_other_projects_and_the_all_scope() {
    let s = with_sprints();
    let other = s.f.other_project("OTH");
    update_project(&s.f.c, other, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
    // gleichnamige Sprints: bei OTH ist „Sprint 2“ abgeschlossen, „Sprint 9“ aktiv
    let o_types = crate::planning::types::list_types(&s.f.c, other).unwrap();
    let mk = |title: &str| crate::planning::tickets::create_ticket(&s.f.c, &crate::planning::tickets::NewTicket::new(other, o_types[0].id, title), &Source::ui()).unwrap().0;
    let (t1, t2, t3) = (mk("O1"), mk("O2"), mk("O3"));
    let cal = WorkCalendar::default();
    let o2 = create_sprint(&s.f.c, &NewSprint::new(other, "Sprint 2"), &cal).unwrap();
    let o9 = create_sprint(&s.f.c, &NewSprint::new(other, "Sprint 9"), &cal).unwrap();
    move_tickets_to_sprint(&s.f.c, o2, &[t1.id, t2.id]).unwrap();
    start_sprint(&s.f.c, o2, &cal).unwrap();
    complete_sprint(&s.f.c, o2, Leftover::ToBacklog).unwrap();
    move_tickets_to_sprint(&s.f.c, o9, &[t3.id]).unwrap();
    start_sprint(&s.f.c, o9, &cal).unwrap();
    let all = |jql: &str| run_query(&s.f.c, &s.ctx, Scope::All, jql, Page::ALL).unwrap().rows.into_iter().map(|r| r.key).collect::<Vec<_>>();
    // `activeSprint()` trifft die aktiven Sprints jedes Projekts – nicht den gleichnamigen abgeschlossenen von OTH
    assert_eq!(all("sprint = activeSprint()"), vec!["PAN-3", "PAN-4", "OTH-3"]);
    // der Name trifft alle Sprints dieses Namens in ihren Projekten
    assert_eq!(all(r#"sprint = "Sprint 2""#), vec!["PAN-3", "PAN-4", "OTH-1", "OTH-2"]);
    // im Projekt-Bereich sind die Namen die des Projekts
    assert_eq!(s.q(r#"sprint = "Sprint 2""#), ks(&[3, 4]));
    assert!(matches!(s.err(r#"sprint = "Sprint 9""#), SearchError::Invalid { .. }));
    // Schalter je Projekt: OTH ohne Sprints → leer, die Daten bleiben
    update_project(&s.f.c, other, &UpdateProject { use_sprints: Some(false), ..Default::default() }).unwrap();
    assert_eq!(all("sprint = activeSprint()"), vec!["PAN-3", "PAN-4"]);
    assert!(all("sprint IS EMPTY").contains(&"OTH-3".to_string()));
    assert!(!all("sprint IS NOT EMPTY").contains(&"OTH-3".to_string()));
}

#[test]
fn a_moved_ticket_keeps_no_sprint_of_its_old_project() {
    let s = with_sprints();
    let other = s.f.other_project("OTH");
    update_project(&s.f.c, other, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
    // PAN-3 steckt im aktiven Sprint 2; eine Zeile für ein Ticket eines anderen Projekts zählt nicht (Sprint und Ticket müssen zusammenpassen)
    let t = s.ids[2];
    s.f.c.execute("UPDATE tickets SET project_id = ?1 WHERE id = ?2", [other, t]).unwrap();
    let all = run_query(&s.f.c, &s.ctx, Scope::All, "sprint = activeSprint()", Page::ALL).unwrap();
    assert!(all.rows.iter().all(|r| r.id != t), "{:?}", all.rows.iter().map(|r| r.key.clone()).collect::<Vec<_>>());
}

#[test]
fn sprint_and_other_conditions_combine() {
    let s = with_sprints();
    assert_eq!(s.q("sprint = activeSprint() AND status = \"In Progress\""), ks(&[4]));
    assert_eq!(s.q("NOT sprint = activeSprint() AND type = Epic"), ks(&[1, 2]));
    assert_eq!(s.q("sprint = activeSprint() ORDER BY priority"), ks(&[3, 4]));
    assert_eq!(count(&s.f.c, &s.ctx, s.scope(), "sprint = activeSprint()").unwrap(), 2);
}

// ---------------------------------------------------------------------------------------------
// Versionen und Meilensteine
// ---------------------------------------------------------------------------------------------

/// PAN mit Versionen „1.0“ (erschienen), „1.1“, „2.0“ und Meilensteinen „Beta“, „Launch“; Felder „Release version“ (Epic, Story) und
/// „Target“ (Meilenstein, Epic): PAN-1 = 2.0, PAN-2 = 1.0, PAN-3 (Story) = 1.1, PAN-4 (Story) ohne.
fn with_versions() -> Sx {
    let s = Sx::new();
    on(&s, false, true);
    let v10 = create_version(&s.f.c, &NewRelease::new(s.f.project, "1.0")).unwrap();
    let v11 = create_version(&s.f.c, &NewRelease::new(s.f.project, "1.1")).unwrap();
    let v20 = create_version(&s.f.c, &NewRelease::new(s.f.project, "2.0")).unwrap();
    set_version_released(&s.f.c, v10, true).unwrap();
    let beta = create_milestone(&s.f.c, &NewRelease::new(s.f.project, "Beta")).unwrap();
    let launch = create_milestone(&s.f.c, &NewRelease::new(s.f.project, "Launch")).unwrap();
    let rv_epic = s.f.add_field(s.f.epic, "Release version", FieldKind::Version, FieldOptions::default(), false);
    let rv_story = s.f.add_field(s.f.story, "Release version", FieldKind::Version, FieldOptions::default(), false);
    let target = s.f.add_field(s.f.epic, "Target", FieldKind::Milestone, FieldOptions::default(), false);
    let set = |t: usize, f: i64, v: i64| set_field(&s.f.c, s.ids[t - 1], f, json!(v), &Source::ui()).unwrap();
    set(1, rv_epic.id, v20);
    set(2, rv_epic.id, v10);
    set(3, rv_story.id, v11);
    set(1, target.id, launch);
    set(2, target.id, beta);
    s
}

#[test]
fn version_equals_in_not_and_empty() {
    let s = with_versions();
    assert_eq!(s.q(r#""Release version" = 2.0"#), ks(&[1]));
    assert_eq!(s.q(r#""Release version" = 1.0"#), ks(&[2]));
    assert_eq!(s.q(r#""Release version" IN (1.0, 1.1)"#), ks(&[2, 3]), "das Feld gilt für Epics und Stories");
    assert_eq!(s.q(r#""Release version" != 2.0"#), ks(&[2, 3, 4, 5, 6, 7, 8, 9, 10, 11]), "`!=` trifft auch leere");
    assert_eq!(s.q(r#""Release version" NOT IN (1.0, 1.1)"#), ks(&[1, 4, 5, 6, 7, 8, 9, 10, 11]));
    assert_eq!(s.q(r#""Release version" IS NOT EMPTY"#), ks(&[1, 2, 3]));
    assert_eq!(s.q(r#""Release version" IS EMPTY"#), ks(&[4, 5, 6, 7, 8, 9, 10, 11]));
    assert_eq!(s.q(r#"type = Epic AND "Release version" IS EMPTY"#), Vec::<String>::new());
    assert_eq!(s.q(r#""Release version" = NULL"#), ks(&[4, 5, 6, 7, 8, 9, 10, 11]));
}

#[test]
fn milestones_work_like_versions_with_their_own_names() {
    let s = with_versions();
    assert_eq!(s.q("target = Beta"), ks(&[2]));
    assert_eq!(s.q("target IN (Beta, Launch)"), ks(&[1, 2]));
    assert_eq!(s.q("target IS EMPTY AND type = Epic"), Vec::<String>::new());
    assert_eq!(s.q("target IS NOT EMPTY"), ks(&[1, 2]));
    assert!(matches!(s.err("target = 1.0"), SearchError::Invalid { .. }), "ein Versionsname ist kein Meilenstein");
}

#[test]
fn version_sorting_follows_the_order_of_the_versions() {
    let s = with_versions();
    assert_eq!(s.q(r#""Release version" IS NOT EMPTY ORDER BY "Release version""#), ks(&[2, 3, 1]));
    assert_eq!(s.q(r#""Release version" IS NOT EMPTY ORDER BY "Release version" DESC"#), ks(&[1, 3, 2]));
    let ids: Vec<i64> = s.f.c.prepare("SELECT id FROM versions ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    reorder_versions(&s.f.c, s.f.project, &[ids[2], ids[0], ids[1]]).unwrap();
    assert_eq!(s.q(r#""Release version" IS NOT EMPTY ORDER BY "Release version""#), ks(&[1, 2, 3]));
    let all = s.q(r#"ORDER BY "Release version""#);
    assert_eq!(&all[..3], ks(&[1, 2, 3]).as_slice());
    assert_eq!(all.len(), 11, "Tickets ohne Version stehen zuletzt");
    assert_eq!(s.q("target IS NOT EMPTY ORDER BY target")[..2], ks(&[2, 1])[..]);
}

#[test]
fn versions_of_projects_with_the_switch_off_count_as_empty() {
    let s = with_versions();
    let other = s.f.other_project("OTH");
    update_project(&s.f.c, other, &UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
    on(&s, false, false);
    // PAN: Schalter aus; OTH: an, aber ohne Feld → im Bereich „alle“ gibt es das Feld noch nicht
    let all = |jql: &str| run_query(&s.f.c, &s.ctx, Scope::All, jql, Page::ALL).map(|p| p.rows.into_iter().map(|r| r.key).collect::<Vec<_>>());
    assert_eq!(all(r#""Release version" IS NOT EMPTY"#).unwrap(), Vec::<String>::new(), "PAN zählt als leer, solange der Schalter aus ist");
    on(&s, false, true);
    assert_eq!(all(r#""Release version" IS NOT EMPTY"#).unwrap(), vec!["PAN-1", "PAN-2", "PAN-3"]);
    // Feldnamen sind im Projekt-Bereich mit Schalter aus unbekannt
    on(&s, false, false);
    assert!(matches!(s.err(r#""Release version" = 1.0"#), SearchError::Invalid { .. }));
}

#[test]
fn version_names_are_compared_per_project_in_the_all_scope() {
    let s = with_versions();
    let other = s.f.other_project("OTH");
    update_project(&s.f.c, other, &UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
    let o_types = crate::planning::types::list_types(&s.f.c, other).unwrap();
    let f = s.f.add_field(o_types[0].id, "Release version", FieldKind::Version, FieldOptions::default(), false);
    let ov = create_version(&s.f.c, &NewRelease::new(other, "2.0")).unwrap();
    let t = crate::planning::tickets::create_ticket(&s.f.c, &crate::planning::tickets::NewTicket::new(other, o_types[0].id, "O"), &Source::ui()).unwrap().0;
    set_field(&s.f.c, t.id, f.id, json!(ov), &Source::ui()).unwrap();
    let all = |jql: &str| run_query(&s.f.c, &s.ctx, Scope::All, jql, Page::ALL).unwrap().rows.into_iter().map(|r| r.key).collect::<Vec<_>>();
    assert_eq!(all(r#""Release version" = 2.0 ORDER BY key"#), vec!["OTH-1", "PAN-1"]);
}

#[test]
fn unknown_version_values_are_reported_in_both_languages() {
    // die Beispiele der Hilfe mit Sprints und Versionen laufen in `tests_seed` (Kreuzprobe)
    let s = with_versions();
    for lang in [Lang::En, Lang::De] {
        let ctx = Ctx::new("2026-11-16", lang);
        let e = run_query(&s.f.c, &ctx, s.scope(), r#""Release version" = 9.9"#, Page::ALL).unwrap_err();
        assert!(matches!(e, SearchError::Invalid { .. }));
        assert!(e.message(lang).contains("9.9"));
    }
}
