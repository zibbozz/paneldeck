//! Tests der Projektsicherung: Rundreise, Kollision/Umnummerierung, Atomarität, Fehlerstellen, große Projekte.

use super::*;
use crate::planning::comments::add_comment;
use crate::planning::fields::{FieldKind, FieldOptions, OptionValue};
use crate::planning::history::{describe, history_of, Source};
use crate::planning::integrity::DeleteMode;
use crate::planning::link_types::{create_link_type, delete_link_type, link_type_by_key, NewLinkType, TargetKind};
use crate::planning::links::{add_link, Target};
use crate::calc::calendar::WorkCalendar;
use crate::calc::progress::Basis;
use crate::planning::projects::{project_by_key, update_project, UpdateProject};
use crate::planning::sprints::releases::{create_milestone, create_version, set_milestone_reached, set_version_released, NewRelease};
use crate::planning::sprints::sprint::{complete_sprint, create_sprint, list_sprints, move_tickets_to_backlog, move_tickets_to_sprint, start_sprint, Leftover, NewSprint};
use crate::planning::sprints::{burndown_for_sprint, clock, sprint_report, velocity_for_project, SprintError};
use crate::planning::template::export_types;
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::{create_child, create_ticket, delete_ticket, fixture::Fx, set_flagged, set_starred, update_ticket, NewTicket, Ticket, TicketUpdate};
use crate::planning::time_entries::{log_time, StartInput};
use crate::planning::transition::transition_ticket;
use crate::planning::types::{create_type, NewType};
use crate::planning::views::boards::{create_board, default_board_for};
use crate::planning::views::filters::{create_filter, NewFilter};
use crate::planning::work_kinds::list_work_kinds;
use crate::planning::PlanError;
use crate::state::Lang;
use rusqlite::Connection;
use serde_json::json;

/// Die Testumgebung: ein Projekt „PAN“ mit allen Arten von Inhalt.
#[allow(dead_code)]
struct Rich {
    fx: Fx,
    epic: Ticket,
    story: Ticket,
    task1: Ticket,
    task2: Ticket,
    sub: Ticket,
    defect: Ticket,
    plain: Ticket,
    sprint1: i64,
}

fn rich() -> Rich {
    let fx = Fx::new();
    let c = &fx.c;
    update_project(c, fx.project, &UpdateProject { description: Some("Ein Projekt\nmit Zeilenumbruch".into()), work_dir: Some("C:\\Work\\pan".into()), use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
    // zusätzliche Felder am Devtask
    let o = |names: &[&str]| FieldOptions { values: names.iter().map(|n| OptionValue::new(n)).collect(), ..Default::default() };
    let tags = fx.add_field(fx.task, "Tags", FieldKind::Labels, FieldOptions::default(), false);
    let areas = fx.add_field(fx.task, "Areas", FieldKind::Multi, o(&["Backend", "Frontend", "Docs"]), false);
    let reviewed = fx.add_field(fx.task, "Reviewed", FieldKind::Checkbox, FieldOptions::default(), false);
    let hours = fx.add_field(fx.task, "Hours", FieldKind::Number, FieldOptions { min: Some(0.0), max: Some(100.0), decimals: Some(1), unit: Some("h".into()), ..Default::default() }, false);
    let due = fx.add_field(fx.task, "Due", FieldKind::Date, FieldOptions::default(), false);
    let start_at = fx.add_field(fx.task, "Start at", FieldKind::Datetime, FieldOptions::default(), false);
    let note = fx.add_field(fx.task, "Note", FieldKind::Text, FieldOptions::default(), false);
    let session = fx.add_field(fx.task, "Session", FieldKind::Terminal, FieldOptions::default(), false);
    let related = fx.add_field(fx.task, "Related", FieldKind::Ticket, FieldOptions::default(), false);
    // ein Typ ohne Workflow
    let plain_type = create_type(c, &NewType { project_id: fx.project, key: None, name: Some("Note".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
    let src = Source::ui();

    let epic = fx.mk(fx.epic, "Epic one");
    let story = fx.child(&epic, fx.story, "Story one");
    update_ticket(c, story.id, &TicketUpdate { title: None, values: vec![(fx.field(fx.story, "epic").id, json!(epic.id))] }, &src).unwrap();
    let task1 = fx.child(&story, fx.task, "Task A");
    let task2 = fx.child(&story, fx.task, "Task B");
    let sub = fx.child(&task1, fx.subtask, "Sub");
    let defect = fx.mk(fx.defect, "Defect — äöü \"Quotes\", Komma; Semikolon");
    update_ticket(c, defect.id, &TicketUpdate { title: None, values: vec![(fx.field(fx.defect, "description").id, json!("Zeile 1\nZeile 2\r\nZeile 3"))] }, &src).unwrap();
    let plain = create_ticket(c, &NewTicket::new(fx.project, plain_type, "Plain thing"), &src).unwrap().0;
    update_ticket(
        c,
        task1.id,
        &TicketUpdate {
            title: None,
            values: vec![
                (tags.id, json!(["x|y", "z\\w"])),
                (areas.id, json!([1, 3])),
                (reviewed.id, json!(true)),
                (hours.id, json!(2.5)),
                (due.id, json!("2026-03-01")),
                (start_at.id, json!("2026-03-01T09:30:00")),
                (note.id, json!("=SUM(A1)")),
                (session.id, json!("session-7")),
                (related.id, json!(task2.id)),
                (fx.field(fx.task, "description").id, json!("# Titel\n\nText mit **fett**")),
            ],
        },
        &src,
    )
    .unwrap();
    transition_ticket(c, task1.id, fx.progress, &[], &src).unwrap();
    transition_ticket(c, task1.id, fx.done, &[], &src).unwrap();
    transition_ticket(c, task2.id, fx.progress, &[], &Source::agent("Sitzung 1")).unwrap();
    update_ticket(c, task2.id, &TicketUpdate { title: Some("Task B renamed".into()), values: vec![] }, &Source::quick()).unwrap();
    set_flagged(c, task2.id, true, &src).unwrap();
    set_starred(c, epic.id, true, &src).unwrap();
    crate::planning::tickets::set_parent(c, defect.id, Some(story.id), &src).unwrap();
    crate::planning::tickets::set_parent(c, defect.id, Some(epic.id), &src).unwrap();
    add_comment(c, task1.id, "Erster Kommentar", &src).unwrap();
    add_comment(c, task1.id, "Zweiter\nmit Umbruch und PAN-2", &src).unwrap();
    // Verknüpfungen
    let custom = create_link_type(c, &NewLinkType { name_out: "tests".into(), name_in: Some("is tested by".into()), symmetric: false, target_kind: TargetKind::Ticket }).unwrap();
    let lt = |k: &str| link_type_by_key(c, k).unwrap().id;
    add_link(c, task1.id, lt("blocks"), Target::Ticket(task2.id), &src).unwrap();
    add_link(c, story.id, lt("relates"), Target::Ticket(defect.id), &src).unwrap();
    add_link(c, story.id, custom, Target::Ticket(task2.id), &src).unwrap();
    add_link(c, task1.id, lt("terminal"), Target::Terminal("session-42".into()), &src).unwrap();
    // Zeiteinträge
    let wk = list_work_kinds(c, fx.project).unwrap();
    log_time(c, task1.id, "1h 30m", StartInput::parse("2026-03-01T09:00:00"), Some(wk[0].id), "Arbeit; fertig", chrono::Local::now().naive_local(), &src).unwrap();
    log_time(c, task1.id, "45m", StartInput::parse("2026-03-02"), None, "", chrono::Local::now().naive_local(), &src).unwrap();
    // Lücke in den Nummern: ein Ticket anlegen und wieder löschen
    let gone = fx.mk(fx.defect, "weg");
    delete_ticket(c, gone.id, &src).unwrap();
    // Versionen und Meilensteine mit Feldwerten
    let d = |m: u32, day: u32| chrono::NaiveDate::from_ymd_opt(2026, m, day).unwrap();
    let v1 = create_version(c, &NewRelease { project_id: fx.project, name: "1.0".into(), description: "Erste Version".into(), due_date: Some(d(12, 1)) }).unwrap();
    let v2 = create_version(c, &NewRelease::new(fx.project, "2.0")).unwrap();
    set_version_released(c, v1, true).unwrap();
    let m1 = create_milestone(c, &NewRelease { project_id: fx.project, name: "Alpha".into(), description: String::new(), due_date: Some(d(11, 20)) }).unwrap();
    create_milestone(c, &NewRelease::new(fx.project, "Beta")).unwrap();
    set_milestone_reached(c, m1, true).unwrap();
    let rel_field = fx.add_field(fx.story, "Release", FieldKind::Version, FieldOptions::default(), false);
    let goal_field = fx.add_field(fx.story, "Goal", FieldKind::Milestone, FieldOptions::default(), false);
    update_ticket(c, story.id, &TicketUpdate { title: None, values: vec![(rel_field.id, json!(v1)), (goal_field.id, json!(m1))] }, &src).unwrap();
    update_ticket(c, story.id, &TicketUpdate { title: None, values: vec![(rel_field.id, json!(v2))] }, &src).unwrap();
    // Story Points und feste Zeiten im Status-Verlauf (für Burndown und Bericht)
    let sp = fx.field(fx.task, "story_points");
    update_ticket(c, task1.id, &TicketUpdate { title: None, values: vec![(sp.id, json!(3))] }, &src).unwrap();
    update_ticket(c, task2.id, &TicketUpdate { title: None, values: vec![(sp.id, json!(5))] }, &src).unwrap();
    c.execute("UPDATE history SET at = '2026-11-04T10:00:00' WHERE field = 'status' AND ticket_id = ?1", [task1.id]).unwrap();
    c.execute("UPDATE history SET at = '2026-11-03T10:00:00' WHERE field = 'status' AND ticket_id = ?1", [task2.id]).unwrap();
    // Sprints: einer abgeschlossen, einer aktiv (mit Umfangsänderung), einer geplant
    let cal = WorkCalendar::default();
    clock::set("2026-10-01T09:00:00");
    let s0 = create_sprint(c, &NewSprint { project_id: fx.project, name: "S0".into(), goal: "Alt".into(), start: Some(d(10, 5)), end: Some(d(10, 16)), default_days: None }, &cal).unwrap();
    move_tickets_to_sprint(c, s0, &[epic.id]).unwrap();
    start_sprint(c, s0, &cal).unwrap();
    clock::set("2026-10-16T17:00:00");
    complete_sprint(c, s0, Leftover::ToBacklog).unwrap();
    clock::set("2026-10-30T10:00:00");
    let sprint1 = create_sprint(c, &NewSprint { project_id: fx.project, name: "S1".into(), goal: "Mitte".into(), start: Some(d(11, 2)), end: Some(d(11, 13)), default_days: Some(8) }, &cal).unwrap();
    move_tickets_to_sprint(c, sprint1, &[task1.id, task2.id, sub.id]).unwrap();
    clock::set("2026-11-02T08:00:00");
    start_sprint(c, sprint1, &cal).unwrap();
    clock::set("2026-11-05T10:00:00");
    move_tickets_to_sprint(c, sprint1, &[defect.id]).unwrap();
    clock::set("2026-11-06T09:00:00");
    move_tickets_to_backlog(c, &[defect.id]).unwrap();
    let s2 = create_sprint(c, &NewSprint::new(fx.project, "S2"), &cal).unwrap();
    move_tickets_to_sprint(c, s2, &[defect.id, story.id]).unwrap();
    // Ansichten
    create_filter(c, &NewFilter { project_id: Some(fx.project), name: "Epics".into(), jql: "type = Epic".into() }).unwrap();
    let b = default_board_for(c, fx.project, Lang::En).unwrap();
    create_board(c, &b).unwrap();
    Rich { fx, epic, story, task1, task2, sub, defect, plain, sprint1 }
}

fn fresh() -> Connection {
    crate::db::open_memory()
}

fn norm(mut d: ProjectDoc) -> ProjectDoc {
    d.exported_at = String::new();
    d
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

const TABLES: [&str; 12] = ["projects", "statuses", "ticket_types", "fields", "tickets", "ticket_values", "links", "comments", "history", "time_entries", "boards", "saved_filters"];

fn snapshot(c: &Connection) -> Vec<i64> {
    TABLES.iter().map(|t| count(c, t)).collect()
}

fn export(r: &Rich) -> ProjectDoc {
    export_project(&r.fx.c, r.fx.project).unwrap()
}

fn import(c: &Connection, d: &ProjectDoc) -> ImportResult {
    import_project(c, d, None).unwrap()
}

/// Der Import schlägt fehl und lässt eine frische Datenbank unverändert.
fn fails(d: &ProjectDoc) -> BackupError {
    let c = fresh();
    let before = snapshot(&c);
    let e = import_project(&c, d, None).unwrap_err();
    assert_eq!(snapshot(&c), before, "atomar: nichts bleibt zurück ({e:?})");
    e
}

fn sp(d: &mut ProjectDoc, i: usize) -> &mut DocSprint {
    &mut d.sprints[i]
}

fn ticket_mut<'a>(d: &'a mut ProjectDoc, title: &str) -> &'a mut DocTicket {
    d.tickets.iter_mut().find(|t| t.title == title).unwrap_or_else(|| panic!("Ticket {title}"))
}

fn titles(c: &Connection, project: i64) -> Vec<(i64, String)> {
    let mut st = c.prepare("SELECT number, title FROM tickets WHERE project_id = ?1 ORDER BY number").unwrap();
    st.query_map([project], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|x| x.unwrap()).collect()
}

// ------------------------------------------------------------------ Export

#[test]
fn export_has_format_version_and_project_settings() {
    let r = rich();
    let d = export(&r);
    assert_eq!((d.format.as_str(), d.version), (FORMAT, VERSION));
    assert!(valid_timestamp(&d.exported_at));
    assert_eq!(d.app_version, env!("CARGO_PKG_VERSION"));
    let p = &d.project;
    assert_eq!((p.key.as_str(), p.name.as_str(), p.color.as_str(), p.work_dir.as_str()), ("PAN", "Paneldeck", "#112233", "C:\\Work\\pan"));
    assert_eq!(p.description, "Ein Projekt\nmit Zeilenumbruch");
    assert!(p.use_sprints && p.use_versions);
    assert!(valid_timestamp(&p.created_at));
    // Zähler: ein Ticket wurde gelöscht → next_number liegt über der höchsten Nummer
    let max = d.tickets.iter().map(|t| t.number).max().unwrap();
    assert_eq!(p.next_number, max + 2);
    assert_eq!(d.tickets.len(), 7);
    assert!(d.tickets.windows(2).all(|w| w[0].number < w[1].number), "aufsteigend nach Nummer");
}

#[test]
fn export_uses_template_numbers_not_database_ids() {
    let r = rich();
    let d = export(&r);
    let nums: Vec<i64> = d.structure.types.iter().map(|t| t.id).collect();
    assert_eq!(nums, (1..=6).collect::<Vec<_>>());
    for t in &d.tickets {
        assert!(nums.contains(&t.type_id));
        if let Some(s) = t.status {
            assert!((1..=3).contains(&s));
        }
    }
    let field_nos: Vec<i64> = d.structure.types.iter().flat_map(|t| t.fields.iter().map(|f| f.id)).collect();
    for t in &d.tickets {
        assert!(t.values.iter().all(|v| field_nos.contains(&v.field)));
    }
    // Eltern und Ticket-Felder nennen Ticketnummern
    let t1 = d.tickets.iter().find(|t| t.title == "Task A").unwrap();
    assert_eq!(t1.parent, Some(r.story.number));
    let story = d.tickets.iter().find(|t| t.title == "Story one").unwrap();
    assert_eq!(story.parent, Some(r.epic.number));
}

#[test]
fn export_is_deterministic() {
    let r = rich();
    assert_eq!(norm(export(&r)), norm(export(&r)));
}

#[test]
fn export_contains_everything() {
    let r = rich();
    let d = export(&r);
    let t1 = d.tickets.iter().find(|t| t.title == "Task A").unwrap();
    assert_eq!(t1.comments.len(), 2);
    assert_eq!(t1.time_entries.len(), 2);
    assert_eq!(t1.links.len(), 2, "blockiert + Terminal");
    assert!(t1.links.iter().any(|l| matches!(&l.target, DocTarget::Terminal { text } if text == "session-42")));
    assert!(t1.values.len() >= 10);
    assert!(!t1.history.is_empty());
    let mut times = t1.time_entries.clone();
    times.sort_by(|a, b| a.start.cmp(&b.start));
    assert_eq!(times[0].comment, "Arbeit; fertig");
    assert_eq!(times[0].work_kind, Some(1));
    assert_eq!(times[1].work_kind, None);
    let t2 = d.tickets.iter().find(|t| t.title == "Task B renamed").unwrap();
    assert!(t2.flagged && !t2.starred);
    assert!(d.tickets.iter().find(|t| t.title == "Epic one").unwrap().starred);
    assert_eq!(d.views.boards.len(), 1);
    assert_eq!(d.views.filters.len(), 1);
    assert_eq!(d.link_types.len(), 4, "blocks, relates, tests und terminal – je nach Gebrauch nummeriert");
    assert!(d.link_types.iter().any(|l| l.key.is_none() && l.name_out.as_deref() == Some("tests")));
    // Quellen im Verlauf bleiben erhalten
    let all: Vec<&DocHistory> = d.tickets.iter().flat_map(|t| &t.history).collect();
    assert!(all.iter().any(|h| h.source == "agent" && h.source_detail.as_deref() == Some("Sitzung 1")));
    assert!(all.iter().any(|h| h.source == "quick"));
    let plain = d.tickets.iter().find(|t| t.title == "Plain thing").unwrap();
    assert_eq!(plain.status, None);
}

#[test]
fn export_history_batches_are_dense_and_in_order_of_appearance() {
    let r = rich();
    let d = export(&r);
    let mut seen: Vec<i64> = Vec::new();
    for h in d.tickets.iter().flat_map(|t| &t.history) {
        if !seen.contains(&h.batch) {
            seen.push(h.batch);
        }
    }
    assert_eq!(seen, (1..=seen.len() as i64).collect::<Vec<_>>());
    assert!(seen.len() > 5);
}

#[test]
fn export_refuses_system_and_unknown_projects() {
    let c = fresh();
    let todo = project_by_key(&c, "TODO").unwrap().id;
    assert_eq!(export_project(&c, todo).unwrap_err(), BackupError::Plan(PlanError::SystemProtected));
    assert_eq!(export_project(&c, 999).unwrap_err(), BackupError::Plan(PlanError::NotFound));
}

#[test]
fn export_of_an_empty_project() {
    let c = fresh();
    let id = crate::planning::projects::create_project(&c, &crate::planning::projects::NewProject::new("EM", "Empty", "#010203")).unwrap();
    let d = export_project(&c, id).unwrap();
    assert!(d.tickets.is_empty() && d.link_types.is_empty() && d.structure.types.is_empty());
    assert_eq!(d.project.next_number, 1);
    let c2 = fresh();
    let r = import(&c2, &d);
    assert_eq!(r.report.tickets, 0);
    assert_eq!(norm(export_project(&c2, r.project_id).unwrap()), norm(d));
}

// ------------------------------------------------------------------ Rundreise

#[test]
fn roundtrip_into_an_empty_database_gives_the_same_document() {
    let r = rich();
    let a = export(&r);
    let c = fresh();
    let res = import(&c, &a);
    assert!(!res.renumbered);
    assert_eq!(res.key, "PAN");
    let b = export_project(&c, res.project_id).unwrap();
    // Teile einzeln, damit ein Fehler lesbar bleibt
    assert_eq!(a.project, b.project, "Projekt (inkl. Zeitstempel und Zähler)");
    assert_eq!(a.structure, b.structure);
    assert_eq!(a.link_types, b.link_types);
    assert_eq!(a.views, b.views);
    assert_eq!(a.tickets.len(), b.tickets.len());
    for (x, y) in a.tickets.iter().zip(&b.tickets) {
        assert_eq!(x, y, "Ticket {}", x.number);
    }
    assert_eq!(norm(a), norm(b));
}

#[test]
fn roundtrip_through_json_text() {
    let r = rich();
    let a = export(&r);
    let text = a.to_json();
    let back = ProjectDoc::from_json(&text).unwrap();
    assert_eq!(back, a);
    let c = fresh();
    let res = import(&c, &back);
    assert_eq!(norm(export_project(&c, res.project_id).unwrap()), norm(a));
    // mit BOM
    assert_eq!(ProjectDoc::from_json(&format!("\u{feff}{text}")).unwrap(), back);
}

#[test]
fn roundtrip_twice_is_stable() {
    let r = rich();
    let a = export(&r);
    let c1 = fresh();
    let b = export_project(&c1, import(&c1, &a).project_id).unwrap();
    let c2 = fresh();
    let d = export_project(&c2, import(&c2, &b).project_id).unwrap();
    assert_eq!(norm(a), norm(d));
}

#[test]
fn roundtrip_restores_the_data_itself() {
    let r = rich();
    let a = export(&r);
    let c = fresh();
    let res = import(&c, &a);
    let pid = res.project_id;
    let rep = &res.report;
    assert_eq!((rep.statuses, rep.types, rep.tickets, rep.comments, rep.time_entries), (3, 6, 7, 2, 2));
    assert_eq!(rep.boards, 1);
    assert_eq!(rep.filters, 1);
    assert_eq!(rep.links, 4);
    assert_eq!(rep.history, a.tickets.iter().map(|t| t.history.len()).sum::<usize>());
    // Hierarchie und Felder in der Datenbank
    let t1 = crate::planning::tickets::ticket_by_key(&c, &format!("PAN-{}", r.task1.number)).unwrap();
    let story = crate::planning::tickets::ticket_by_key(&c, &format!("PAN-{}", r.story.number)).unwrap();
    assert_eq!(t1.parent_id, Some(story.id));
    assert_eq!(t1.project_id, pid);
    let vals = crate::planning::values::fields_with_values(&c, t1.id, t1.type_id).unwrap();
    let get = |name: &str| vals.iter().find(|v| v.field.name.as_deref() == Some(name)).and_then(|v| v.value.clone());
    assert_eq!(get("Tags"), Some(json!(["x|y", "z\\w"])));
    assert_eq!(get("Hours"), Some(json!(2.5)));
    assert_eq!(get("Reviewed"), Some(json!(true)));
    assert_eq!(get("Due"), Some(json!("2026-03-01")));
    assert_eq!(get("Session"), Some(json!("session-7")));
    let t2 = crate::planning::tickets::ticket_by_key(&c, &format!("PAN-{}", r.task2.number)).unwrap();
    assert_eq!(get("Related"), Some(json!(t2.id)), "Ticket-Feld zeigt auf das neue Ticket");
    assert!(t2.flagged);
    assert_eq!(crate::planning::comments::comments_of(&c, t1.id).unwrap().len(), 2);
    assert_eq!(crate::planning::time_entries::entries_of(&c, t1.id).unwrap().len(), 2);
    assert_eq!(crate::planning::links::links_of(&c, t1.id, Lang::En).unwrap().len(), 2);
    let done = crate::planning::statuses::list_statuses(&c, pid).unwrap().into_iter().find(|s| s.key.as_deref() == Some("done")).unwrap().id;
    assert_eq!(t1.status_id, Some(done));
    // Zähler setzt fort
    let nt = create_ticket(&c, &NewTicket::new(pid, crate::planning::types::list_types(&c, pid).unwrap()[0].id, "neu"), &Source::ui()).unwrap().0;
    assert_eq!(nt.number, a.project.next_number);
}

#[test]
fn history_is_readable_after_import_into_a_database_with_other_ids() {
    let r = rich();
    let a = export(&r);
    // Zielbank mit einem anderen Projekt, damit alle Ids anders sind
    let c = fresh();
    let other = Fx::new();
    let od = export_project(&other.c, other.project).unwrap();
    import_project(&c, &od, Some("ZZ".into())).unwrap();
    let p = project_by_key(&c, "ZZ").unwrap().id;
    let ty = crate::planning::types::list_types(&c, p).unwrap()[0].id;
    for i in 0..5 {
        create_ticket(&c, &NewTicket::new(p, ty, &format!("zz {i}")), &Source::ui()).unwrap();
    }
    let res = import(&c, &a);
    for lang in [Lang::En, Lang::De] {
        for t in &a.tickets {
            let old = crate::planning::tickets::ticket_by_key(&r.fx.c, &format!("PAN-{}", t.number)).unwrap();
            let new = crate::planning::tickets::ticket_by_key(&c, &format!("PAN-{}", t.number)).unwrap();
            assert_eq!(new.project_id, res.project_id);
            let before: Vec<String> = history_of(&r.fx.c, old.id).unwrap().iter().map(|e| describe(&r.fx.c, e, lang)).collect();
            let after: Vec<String> = history_of(&c, new.id).unwrap().iter().map(|e| describe(&c, e, lang)).collect();
            assert_eq!(before, after, "{} ({lang:?})", t.title);
            assert!(after.iter().all(|s| !s.contains("(deleted") && !s.contains("(gelöscht") && !s.contains("(Gelöscht")), "{after:?}");
        }
    }
}

#[test]
fn quick_actions_and_structure_survive() {
    let r = rich();
    let c = &r.fx.c;
    use crate::planning::quick::actions::{create_quick_action, NewQuickAction};
    use crate::planning::quick::params::{ActionParams, ChangeStatusParams};
    create_quick_action(c, &NewQuickAction::new(r.fx.story, "Finish", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: r.fx.done, set_values: vec![] }))).unwrap();
    let a = export(&r);
    assert_eq!(a.structure.types.iter().map(|t| t.quick_actions.len()).sum::<usize>(), 1);
    let c2 = fresh();
    let res = import(&c2, &a);
    assert_eq!(res.report.quick_actions, 1);
    assert_eq!(export_types(&c2, res.project_id).unwrap(), export_types(c, r.fx.project).unwrap());
}

#[test]
fn project_settings_roundtrip() {
    let r = rich();
    let a = export(&r);
    let c = fresh();
    let res = import(&c, &a);
    let p = crate::planning::projects::get_project(&c, res.project_id).unwrap();
    assert_eq!((p.name.as_str(), p.color.as_str(), p.work_dir.as_str(), p.description.as_str()), ("Paneldeck", "#112233", "C:\\Work\\pan", "Ein Projekt\nmit Zeilenumbruch"));
    assert!(p.use_sprints && p.use_versions && !p.system);
    assert_eq!(p.created_at, a.project.created_at);
}

// ------------------------------------------------------------------ Kürzel-Kollision und Umnummerierung

#[test]
fn same_key_asks_for_a_new_key_and_changes_nothing() {
    let r = rich();
    let a = export(&r);
    let before = snapshot(&r.fx.c);
    let e = import_project(&r.fx.c, &a, None).unwrap_err();
    assert_eq!(e, BackupError::KeyTaken("PAN".into()));
    assert!(e.message(Lang::De).contains("PAN") && e.message(Lang::En).contains("PAN"));
    assert_eq!(snapshot(&r.fx.c), before);
    // auch ausdrücklich übergeben, in beliebiger Schreibweise
    assert_eq!(import_project(&r.fx.c, &a, Some("pan".into())).unwrap_err(), BackupError::KeyTaken("PAN".into()));
    assert_eq!(snapshot(&r.fx.c), before);
}

#[test]
fn new_key_renumbers_and_keeps_all_internal_references() {
    let r = rich();
    let a = export(&r);
    let before = norm(export(&r));
    let res = import_project(&r.fx.c, &a, Some("neu".into())).unwrap();
    let c = &r.fx.c;
    assert!(res.renumbered);
    assert_eq!(res.key, "NEU");
    assert!(matches!(res.report.warnings.first(), Some(Warning::Renumbered { from_key, to_key }) if from_key == "PAN" && to_key == "NEU"));
    let n = titles(c, res.project_id);
    assert_eq!(n.iter().map(|x| x.0).collect::<Vec<_>>(), (1..=7).collect::<Vec<_>>(), "von 1 an ohne Lücke");
    // Reihenfolge der alten Nummern bleibt
    let old = titles(c, r.fx.project);
    assert_eq!(n.iter().map(|x| x.1.clone()).collect::<Vec<_>>(), old.iter().map(|x| x.1.clone()).collect::<Vec<_>>());
    assert_eq!(crate::planning::projects::get_project(c, res.project_id).unwrap().next_number, 8);
    // Eltern
    let by = |t: &str| crate::planning::tickets::ticket_by_key(c, &format!("NEU-{}", n.iter().find(|x| x.1 == t).unwrap().0)).unwrap();
    assert_eq!(by("Task A").parent_id, Some(by("Story one").id));
    assert_eq!(by("Sub").parent_id, Some(by("Task A").id));
    assert_eq!(by("Story one").parent_id, Some(by("Epic one").id));
    // Ticket-Felder „Epic“ und „Related“
    let story_vals = crate::planning::values::load_values(c, by("Story one").id).unwrap();
    assert!(story_vals.values().any(|v| *v == json!(by("Epic one").id)));
    let t1_vals = crate::planning::values::load_values(c, by("Task A").id).unwrap();
    assert!(t1_vals.values().any(|v| *v == json!(by("Task B renamed").id)));
    // Verknüpfungen
    let links = crate::planning::links::links_of(c, by("Task A").id, Lang::En).unwrap();
    assert!(links.iter().any(|l| matches!(&l.target, crate::planning::links::LinkTarget::Ticket(t) if t.id == by("Task B renamed").id)));
    // Verlauf zeigt die neuen Schlüssel (Eltern-Wechsel)
    let dname = "Defect — äöü \"Quotes\", Komma; Semikolon";
    let hist: Vec<String> = history_of(c, by(dname).id).unwrap().iter().map(|e| describe(c, e, Lang::En)).collect();
    assert!(hist.iter().any(|s| s.contains("Parent") && s.contains(&by("Story one").key()) && s.contains(&by("Epic one").key())), "{hist:?}");
    assert!(by(dname).key().starts_with("NEU-"));
    // Schlüssel in Texten bleiben unverändert
    let comments = crate::planning::comments::comments_of(c, by("Task A").id).unwrap();
    assert!(comments.iter().any(|x| x.body.contains("PAN-2")));
    // Das Original ist unverändert
    assert_eq!(before, norm(export(&r)));
    let b = export_project(c, res.project_id).unwrap();
    assert_eq!(b.project.key, "NEU");
    assert_eq!(b.tickets.len(), a.tickets.len());
    assert_eq!(b.structure, a.structure);
    assert_eq!(b.views, a.views);
}

#[test]
fn a_renumbered_project_can_be_backed_up_and_restored_again() {
    let r = rich();
    let a = export(&r);
    let c = fresh();
    let first = import_project(&c, &a, Some("AAA".into())).unwrap();
    let b = export_project(&c, first.project_id).unwrap();
    let c2 = fresh();
    let second = import_project(&c2, &b, None).unwrap();
    assert!(!second.renumbered);
    let again = export_project(&c2, second.project_id).unwrap();
    assert_eq!(norm(b), norm(again));
}

#[test]
fn new_key_equal_to_the_document_key_does_not_renumber() {
    let r = rich();
    let a = export(&r);
    let c = fresh();
    let res = import_project(&c, &a, Some("PAN".into())).unwrap();
    assert!(!res.renumbered);
    assert!(res.report.warnings.iter().all(|w| !matches!(w, Warning::Renumbered { .. })));
    assert_eq!(titles(&c, res.project_id).iter().map(|x| x.0).max(), Some(a.tickets.iter().map(|t| t.number).max().unwrap()));
}

#[test]
fn key_is_checked_live() {
    let r = rich();
    assert_eq!(check_new_key(&r.fx.c, " neu "), Ok("NEU".to_string()));
    assert_eq!(check_new_key(&r.fx.c, "pan"), Err(BackupError::KeyTaken("PAN".into())));
    assert_eq!(check_new_key(&r.fx.c, "1x"), Err(BackupError::Plan(PlanError::KeyInvalid)));
    assert_eq!(check_new_key(&r.fx.c, ""), Err(BackupError::Plan(PlanError::KeyInvalid)));
    assert_eq!(check_new_key(&r.fx.c, "TODO"), Err(BackupError::KeyTaken("TODO".into())), "das System-Projekt zählt");
    assert_eq!(import_project(&r.fx.c, &export(&r), Some("A".into())).unwrap_err(), BackupError::Plan(PlanError::KeyInvalid));
}

#[test]
fn import_never_changes_existing_data() {
    let r = rich();
    let c = &r.fx.c;
    let a = export(&r);
    let before = snapshot(c);
    let types_before = export_types(c, r.fx.project).unwrap();
    let link_types_before = count(c, "link_types");
    import_project(c, &a, Some("TWO".into())).unwrap();
    import_project(c, &a, Some("THREE".into())).unwrap();
    assert_eq!(norm(export(&r)), norm(a), "das Original ist unverändert");
    assert_eq!(export_types(c, r.fx.project).unwrap(), types_before);
    assert_eq!(count(c, "link_types"), link_types_before, "vorhandene Verknüpfungstypen werden mitbenutzt");
    assert_eq!(count(c, "projects"), 4);
    assert!(snapshot(c)[4] > before[4]);
    // die drei Projekte sind unabhängig voneinander
    assert_eq!(titles(c, project_by_key(c, "TWO").unwrap().id), titles(c, project_by_key(c, "THREE").unwrap().id));
}

// ------------------------------------------------------------------ Fehler und Atomarität

#[test]
fn rejects_foreign_and_newer_documents() {
    let r = rich();
    let mut a = export(&r);
    let c = fresh();
    let before = snapshot(&c);
    a.format = "something".into();
    assert_eq!(import_project(&c, &a, None).unwrap_err(), BackupError::Format);
    let mut a = export(&r);
    a.version = VERSION + 1;
    assert_eq!(import_project(&c, &a, None).unwrap_err(), BackupError::Version { found: VERSION + 1, supported: VERSION });
    assert_eq!(snapshot(&c), before);
    // JSON
    assert_eq!(ProjectDoc::from_json("[]").unwrap_err(), BackupError::Format);
    assert_eq!(ProjectDoc::from_json("{\"format\":\"paneldeck-type-template\",\"version\":1}").unwrap_err(), BackupError::Format);
    assert!(matches!(ProjectDoc::from_json("{"), Err(BackupError::Json(_))));
    assert!(matches!(ProjectDoc::from_json(&format!("{{\"format\":\"{FORMAT}\",\"version\":1}}")), Err(BackupError::Json(_))), "ohne Projekt und Struktur");
    assert_eq!(ProjectDoc::from_json(&format!("{{\"format\":\"{FORMAT}\",\"version\":{}}}", VERSION + 3)).unwrap_err(), BackupError::Version { found: VERSION + 3, supported: VERSION });
}

#[test]
fn unknown_sections_are_ignored_and_missing_optional_ones_are_empty() {
    let r = rich();
    let mut v: serde_json::Value = serde_json::from_str(&export(&r).to_json()).unwrap();
    v["future_section"] = json!([{"name": "x"}]);
    v.as_object_mut().unwrap().remove("link_types");
    v.as_object_mut().unwrap().remove("tickets");
    v.as_object_mut().unwrap().remove("sprints");
    let d = ProjectDoc::from_json(&v.to_string()).unwrap();
    assert!(d.tickets.is_empty() && d.link_types.is_empty());
    let c = fresh();
    assert_eq!(import(&c, &d).report.tickets, 0);
}

#[test]
fn errors_name_the_location_in_both_languages() {
    let r = rich();
    let mut a = export(&r);
    ticket_mut(&mut a, "Task A").status = Some(999);
    let e = fails(&a);
    let n = r.task1.number;
    assert_eq!(e, BackupError::At(Where::Ticket(n), Problem::UnknownRef(RefKind::Status, 999)));
    assert!(e.message(Lang::En).starts_with(&format!("Ticket {n}:")));
    assert!(e.message(Lang::De).contains("fehlt") && e.message(Lang::En).contains("does not contain"));
}

#[test]
fn rejects_elements_that_break_the_rules_of_the_data_layer() {
    let r = rich();
    let good = export(&r);
    let n = |t: &str| good.tickets.iter().find(|x| x.title == t).unwrap().number;
    let field_no = |type_key: &str, key: &str| -> i64 { good.structure.types.iter().find(|t| t.key.as_deref() == Some(type_key)).unwrap().fields.iter().find(|f| f.key.as_deref() == Some(key)).unwrap().id };
    let field_named = |name: &str| -> i64 { good.structure.types.iter().flat_map(|t| &t.fields).find(|f| f.name.as_deref() == Some(name)).unwrap().id };
    let defect = "Defect — äöü \"Quotes\", Komma; Semikolon";

    // Typ unbekannt
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").type_id = 999;
    assert_eq!(fails(&d), BackupError::At(Where::Ticket(n("Task A")), Problem::UnknownRef(RefKind::Type, 999)));
    // Titel leer
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").title = "  ".into();
    assert_eq!(fails(&d), BackupError::AtTicket(Where::Ticket(n("Task A")), TicketError::TitleEmpty));
    // doppelte und ungültige Nummer
    let mut d = good.clone();
    d.tickets[1].number = d.tickets[0].number;
    assert!(matches!(fails(&d), BackupError::At(_, Problem::TicketNumber(_))));
    let mut d = good.clone();
    d.tickets[0].number = 0;
    assert!(matches!(fails(&d), BackupError::At(Where::Ticket(0), Problem::TicketNumber(0))));
    // Status gehört nicht zum Workflow (Typ ohne Workflow)
    let mut d = good.clone();
    ticket_mut(&mut d, "Plain thing").status = Some(1);
    assert_eq!(fails(&d), BackupError::At(Where::Ticket(n("Plain thing")), Problem::StatusNotAllowed));
    // Eltern: unbekannt, sich selbst, nicht erlaubt
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").parent = Some(999);
    assert_eq!(fails(&d), BackupError::At(Where::Ticket(n("Task A")), Problem::UnknownRef(RefKind::Ticket, 999)));
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").parent = Some(n("Task A"));
    assert_eq!(fails(&d), BackupError::AtTicket(Where::Ticket(n("Task A")), TicketError::ParentSelf));
    let mut d = good.clone();
    ticket_mut(&mut d, "Epic one").parent = Some(n("Task A"));
    assert_eq!(fails(&d), BackupError::AtTicket(Where::Ticket(n("Epic one")), TicketError::ParentNotAllowed));
    // Werte: Feld gehört nicht zum Typ, unbekanntes Feld, falscher Wert, Verweis ins Leere
    let mut d = good.clone();
    ticket_mut(&mut d, defect).values.push(DocValue { field: field_named("Hours"), value: json!(1) });
    assert!(matches!(fails(&d), BackupError::At(Where::Value(_, _), Problem::FieldNotInType)));
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").values.push(DocValue { field: 9999, value: json!(1) });
    assert!(matches!(fails(&d), BackupError::At(Where::Value(_, 9999), Problem::UnknownRef(RefKind::Field, 9999))));
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").values.iter_mut().find(|v| v.field == field_named("Hours")).unwrap().value = json!("viel");
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Value(_, _), TicketError::ValueInvalid(_))));
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").values.iter_mut().find(|v| v.field == field_named("Hours")).unwrap().value = json!(500);
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Value(_, _), TicketError::ValueInvalid(_))), "über dem Maximum");
    let mut d = good.clone();
    ticket_mut(&mut d, "Task A").values.iter_mut().find(|v| v.field == field_named("Related")).unwrap().value = json!(4242);
    assert!(matches!(fails(&d), BackupError::At(Where::Value(_, _), Problem::UnknownRef(RefKind::Ticket, 4242))));
    // Ticket-Feld „Epic“ der Userstory darf nur auf einen Epic zeigen
    let mut d = good.clone();
    let epic_field = field_no("userstory", "epic");
    ticket_mut(&mut d, "Story one").values.iter_mut().find(|v| v.field == epic_field).unwrap().value = json!(n("Task A"));
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Value(_, _), TicketError::RefInvalid(_))));
    // Dropdown-Wert, den es nicht gibt
    let mut d = good.clone();
    let prio = field_no("devtask", "priority");
    ticket_mut(&mut d, "Task B renamed").values.push(DocValue { field: prio, value: json!(77) });
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Value(_, _), TicketError::ValueInvalid(_))));
}

#[test]
fn rejects_bad_comments_links_times_and_history() {
    let r = rich();
    let good = export(&r);
    let t = "Task A";
    let mut d = good.clone();
    ticket_mut(&mut d, t).comments[0].body = " ".into();
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Comment(_, 0), TicketError::CommentEmpty)));
    let mut d = good.clone();
    ticket_mut(&mut d, t).comments[1].created_at = "gestern".into();
    assert!(matches!(fails(&d), BackupError::At(Where::Comment(_, 1), Problem::Timestamp(_))));
    // Verknüpfungen
    let mut d = good.clone();
    ticket_mut(&mut d, t).links[0].link_type = 99;
    assert!(matches!(fails(&d), BackupError::At(Where::Link(_, 0), Problem::UnknownRef(RefKind::LinkType, 99))));
    let mut d = good.clone();
    ticket_mut(&mut d, t).links[0].target = DocTarget::Url { text: "https://example.com".into() };
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Link(_, 0), TicketError::LinkTargetMismatch)));
    let mut d = good.clone();
    let me = ticket_mut(&mut d, t).number;
    ticket_mut(&mut d, t).links[0].target = DocTarget::Ticket { number: me };
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Link(_, 0), TicketError::LinkSelf)));
    let mut d = good.clone();
    let dup = ticket_mut(&mut d, t).links[0].clone();
    ticket_mut(&mut d, t).links.push(dup);
    assert!(matches!(fails(&d), BackupError::At(Where::Link(_, 2), Problem::LinkDuplicate)));
    let mut d = good.clone();
    ticket_mut(&mut d, t).links[0].target = DocTarget::Ticket { number: 777 };
    assert!(matches!(fails(&d), BackupError::At(Where::Link(_, 0), Problem::UnknownRef(RefKind::Ticket, 777))));
    let mut d = good.clone();
    let l = ticket_mut(&mut d, t).links.iter_mut().find(|l| matches!(l.target, DocTarget::Terminal { .. })).unwrap();
    l.target = DocTarget::Terminal { text: "zwei\nZeilen".into() };
    assert!(matches!(fails(&d), BackupError::AtTicket(Where::Link(_, _), TicketError::LinkTargetInvalid)));
    // Zeiteinträge
    let mut d = good.clone();
    ticket_mut(&mut d, t).time_entries[0].minutes = 0;
    assert!(matches!(fails(&d), BackupError::At(Where::Time(_, 0), Problem::Minutes)));
    let mut d = good.clone();
    ticket_mut(&mut d, t).time_entries[0].work_kind = Some(99);
    assert!(matches!(fails(&d), BackupError::At(Where::Time(_, 0), Problem::UnknownRef(RefKind::WorkKind, 99))));
    // Verlauf
    let mut d = good.clone();
    ticket_mut(&mut d, t).history[0].source = "robot".into();
    assert!(matches!(fails(&d), BackupError::At(Where::History(_, 0), Problem::Source(_))));
    let mut d = good.clone();
    ticket_mut(&mut d, t).history[0].at = "heute".into();
    assert!(matches!(fails(&d), BackupError::At(Where::History(_, 0), Problem::Timestamp(_))));
}

#[test]
fn rejects_bad_project_structure_and_link_types() {
    let r = rich();
    let good = export(&r);
    let mut d = good.clone();
    d.project.color = "grün".into();
    assert_eq!(fails(&d), BackupError::AtPlan(Where::Project, PlanError::ColorInvalid));
    let mut d = good.clone();
    d.project.key = "1x".into();
    assert_eq!(fails(&d), BackupError::AtPlan(Where::Project, PlanError::KeyInvalid));
    let mut d = good.clone();
    d.project.created_at = "vor langer Zeit".into();
    assert!(matches!(fails(&d), BackupError::At(Where::Project, Problem::Timestamp(_))));
    let mut d = good.clone();
    d.structure.statuses[0].color = "x".into();
    assert_eq!(fails(&d), BackupError::AtPlan(Where::Structure, PlanError::ColorInvalid));
    let mut d = good.clone();
    let mut dup = d.structure.statuses[0].clone();
    dup.id = 99;
    d.structure.statuses.push(dup);
    assert_eq!(fails(&d), BackupError::At(Where::Structure, Problem::StructureNames));
    let mut d = good.clone();
    d.link_types[0].target_kind = "hologram".into();
    assert!(matches!(fails(&d), BackupError::At(Where::LinkType(1), Problem::TargetKind(_))));
    let mut d = good.clone();
    let custom = d.link_types.iter_mut().find(|l| l.key.is_none()).unwrap();
    custom.name_out = None;
    assert!(matches!(fails(&d), BackupError::At(Where::LinkType(_), Problem::LinkTypeUnnamed)));
}

#[test]
fn link_types_are_reused_created_or_rejected() {
    let r = rich();
    let a = export(&r);
    // Zielbank ohne den eigenen Typ: er wird angelegt und gemeldet
    let c = fresh();
    let base = count(&c, "link_types");
    let res = import(&c, &a);
    assert!(res.report.warnings.iter().any(|w| matches!(w, Warning::LinkTypeCreated { en, .. } if en == "tests")));
    assert_eq!(count(&c, "link_types"), base + 1);
    // zweiter Import: vorhanden, keine Meldung
    let res = import_project(&c, &a, Some("TWO".into())).unwrap();
    assert!(!res.report.warnings.iter().any(|w| matches!(w, Warning::LinkTypeCreated { .. })));
    assert_eq!(count(&c, "link_types"), base + 1);
    // Ein mitgelieferter Typ fehlt: mit seinem Schlüssel wieder angelegt
    let c = fresh();
    delete_link_type(&c, link_type_by_key(&c, "relates").unwrap().id, Some(DeleteMode::Discard)).unwrap();
    let res = import(&c, &a);
    assert!(res.report.warnings.iter().any(|w| matches!(w, Warning::LinkTypeCreated { en, .. } if en == "relates to")));
    let lt = link_type_by_key(&c, "relates").unwrap();
    assert!(lt.symmetric && !lt.system);
    assert_eq!(lt.names(Lang::De).0, "ist in Beziehung mit", "übersetzt sich weiter");
    // Gleicher Name, andere Richtung: Fehler
    let c = fresh();
    create_link_type(&c, &NewLinkType { name_out: "tests".into(), name_in: None, symmetric: true, target_kind: TargetKind::Ticket }).unwrap();
    let before = snapshot(&c);
    assert!(matches!(import_project(&c, &a, None).unwrap_err(), BackupError::At(Where::LinkType(_), Problem::LinkTypeDiffers)));
    assert_eq!(snapshot(&c), before);
}

#[test]
fn links_to_other_projects_and_terminals_are_reported() {
    let r = rich();
    let c = &r.fx.c;
    let other = r.fx.other_project("OTH");
    let oty = crate::planning::types::list_types(c, other).unwrap()[0].id;
    let o = create_ticket(c, &NewTicket::new(other, oty, "fremd"), &Source::ui()).unwrap().0;
    add_link(c, r.defect.id, link_type_by_key(c, "blocks").unwrap().id, Target::Ticket(o.id), &Source::ui()).unwrap();
    let a = export(&r);
    let d = a.tickets.iter().find(|t| t.title == r.defect.title).unwrap();
    assert!(d.links.iter().any(|l| matches!(&l.target, DocTarget::ExternalTicket { key } if key == "OTH-1")));
    let c2 = fresh();
    let res = import(&c2, &a);
    assert!(res.report.warnings.contains(&Warning::ExternalLinks(1)));
    assert!(res.report.warnings.contains(&Warning::TerminalLinks(1)));
    assert_eq!(res.report.links, 4, "die Verknüpfung ins andere Projekt entfällt");
    for w in &res.report.warnings {
        assert!(!w.message(Lang::De).is_empty() && !w.message(Lang::En).is_empty());
    }
    // Terminal-Ziel bleibt als Text
    let t1 = crate::planning::tickets::ticket_by_key(&c2, &format!("PAN-{}", r.task1.number)).unwrap();
    assert!(crate::planning::links::links_of(&c2, t1.id, Lang::En).unwrap().iter().any(|l| matches!(&l.target, crate::planning::links::LinkTarget::Terminal(s) if s == "session-42")));
}

#[test]
fn version_1_documents_still_import_but_their_version_values_are_skipped() {
    let r = rich();
    let mut a = export(&r);
    // So sah ein Dokument der Version 1 aus: keine Abschnitte, Werte von Version-/Meilenstein-Feldern ohne Objekte
    a.version = 1;
    a.versions.clear();
    a.milestones.clear();
    a.sprints.clear();
    let c2 = fresh();
    let res = import(&c2, &a);
    assert!(res.report.warnings.contains(&Warning::VersionValues(2)));
    assert_eq!((res.report.sprints, res.report.versions, res.report.milestones), (0, 0, 0));
    let b = export_project(&c2, res.project_id).unwrap();
    let nv = |d: &ProjectDoc| d.tickets.iter().find(|t| t.title == "Story one").unwrap().values.len();
    assert_eq!(nv(&b) + 2, nv(&a));
    // und auch ein JSON-Text der Version 1 ohne die neuen Abschnitte wird gelesen
    let mut v: serde_json::Value = serde_json::from_str(&a.to_json()).unwrap();
    for k in ["versions", "milestones", "sprints"] {
        v.as_object_mut().unwrap().remove(k);
    }
    assert_eq!(v["version"], 1);
    let d = ProjectDoc::from_json(&v.to_string()).unwrap();
    assert!(d.sprints.is_empty() && d.versions.is_empty());
}

#[test]
fn versions_milestones_and_sprints_roundtrip_with_all_details() {
    let r = rich();
    let a = export(&r);
    assert_eq!(a.version, 2);
    assert_eq!(a.versions.iter().map(|v| (v.name.as_str(), v.done, v.due_date.as_deref())).collect::<Vec<_>>(), vec![("1.0", true, Some("2026-12-01")), ("2.0", false, None)]);
    assert!(a.versions[0].done_at.is_some() && a.versions[1].done_at.is_none());
    assert_eq!(a.versions[0].description, "Erste Version");
    assert_eq!(a.milestones.iter().map(|v| (v.name.as_str(), v.done)).collect::<Vec<_>>(), vec![("Alpha", true), ("Beta", false)]);
    assert_eq!(a.sprints.iter().map(|s| (s.name.as_str(), s.state.as_str())).collect::<Vec<_>>(), vec![("S0", "completed"), ("S1", "active"), ("S2", "planned")]);
    let s1 = &a.sprints[1];
    assert_eq!((s1.goal.as_str(), s1.default_days, s1.start_date.as_deref(), s1.end_date.as_deref()), ("Mitte", 8, Some("2026-11-02"), Some("2026-11-13")));
    assert_eq!(s1.started_at.as_deref(), Some("2026-11-02T08:00:00"));
    assert_eq!(a.sprints[0].completed_at.as_deref(), Some("2026-10-16T17:00:00"));
    // Zugehörigkeit: drei Tickets vom Planen, ein nachträglich hinzugekommenes und wieder entferntes
    assert_eq!(s1.tickets.len(), 4);
    let late = s1.tickets.iter().find(|e| e.removed_at.is_some()).unwrap();
    assert_eq!((late.added_at.as_str(), late.removed_at.as_deref()), ("2026-11-05T10:00:00", Some("2026-11-06T09:00:00")));
    assert!(s1.tickets.iter().any(|e| e.points_at_add == Some(5.0)) && s1.tickets.iter().any(|e| e.points_at_add == Some(3.0)));
    // Feldwerte nennen die Nummern der Versionen/Meilensteine
    let story = a.tickets.iter().find(|t| t.title == "Story one").unwrap();
    let nos: Vec<i64> = story.values.iter().filter(|v| v.field >= 1).map(|v| v.value.as_i64().unwrap_or(0)).collect();
    assert!(nos.contains(&2) && nos.contains(&1), "Version 2.0 (Nr. 2), Meilenstein Alpha (Nr. 1): {nos:?}");
    // Rundreise in eine leere und in eine Datenbank mit verschobenen Ids
    let c = fresh();
    let res = import(&c, &a);
    assert_eq!((res.report.sprints, res.report.versions, res.report.milestones), (3, 2, 2));
    assert!(!res.report.warnings.iter().any(|w| matches!(w, Warning::VersionValues(_))));
    let b = export_project(&c, res.project_id).unwrap();
    assert_eq!((&a.versions, &a.milestones, &a.sprints), (&b.versions, &b.milestones, &b.sprints));
    assert_eq!(norm(a.clone()), norm(b));
    let shifted = import_project(&r.fx.c, &a, Some("TWO".into())).unwrap();
    let b2 = export_project(&r.fx.c, shifted.project_id).unwrap();
    assert_eq!((&a.versions, &a.milestones, &a.sprints), (&b2.versions, &b2.milestones, &b2.sprints));
    // Das Feld zeigt auf die neue Version, nicht auf die des Originals
    let c_ = &r.fx.c;
    let st = c_.query_row("SELECT id FROM tickets WHERE project_id = ?1 AND title = 'Story one'", [shifted.project_id], |x| x.get::<_, i64>(0)).unwrap();
    let vals = crate::planning::values::load_values(c_, st).unwrap();
    let vid = vals.values().filter_map(|v| v.as_i64()).find(|id| c_.query_row("SELECT name FROM versions WHERE id = ?1 AND project_id = ?2", [*id, shifted.project_id], |x| x.get::<_, String>(0)).is_ok_and(|n| n == "2.0"));
    assert!(vid.is_some(), "{vals:?}");
    // Verlauf des Version-Felds: neue Kennung im Text
    let hist: Vec<_> = history_of(c_, st).unwrap().into_iter().filter(|e| matches!(e.field, crate::planning::history::HistoryField::Custom(_)) && e.new.as_deref() == vid.map(|v| v.to_string()).as_deref()).collect();
    assert_eq!(hist.len(), 1, "der Wechsel auf 2.0 steht im Verlauf mit der neuen Kennung");
    assert_eq!(hist[0].old.as_deref().map(|o| o.parse::<i64>().is_ok()), Some(true));
}

#[test]
fn burndown_report_and_velocity_are_the_same_after_the_import() {
    let r = rich();
    let a = export(&r);
    let c = fresh();
    let res = import(&c, &a);
    let find = |c: &Connection, p: i64, name: &str| list_sprints(c, p).unwrap().into_iter().find(|s| s.name == name).unwrap();
    let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
    for name in ["S0", "S1", "S2"] {
        let (old, new) = (find(&r.fx.c, r.fx.project, name), find(&c, res.project_id, name));
        for basis in [Basis::Points, Basis::TicketCount, Basis::PointsWithFallbackOne].into_iter().filter(|_| name != "S2") {
            let x = burndown_for_sprint(&r.fx.c, old.id, basis, WorkCalendar::default(), today).unwrap();
            let y = burndown_for_sprint(&c, new.id, basis, WorkCalendar::default(), today).unwrap();
            assert_eq!(format!("{x:?}"), format!("{y:?}"), "Burndown {name} {basis:?}");
        }
        let (x, y) = (sprint_report(&r.fx.c, old.id).unwrap(), sprint_report(&c, new.id).unwrap());
        assert_eq!((x.planned, x.completed, x.added, x.removed, x.open), (y.planned, y.completed, y.added, y.removed, y.open), "Bericht {name}");
        assert_eq!((x.sprint.state, x.sprint.start_date, x.sprint.started_at.clone()), (y.sprint.state, y.sprint.start_date, y.sprint.started_at.clone()));
    }
    // S1 hat wirklich etwas zu berichten (Startumfang, Erledigtes, Hinzugefügtes)
    let rep = sprint_report(&c, find(&c, res.project_id, "S1").id).unwrap();
    assert_eq!(rep.planned.tickets, 3);
    assert_eq!(rep.added.tickets, 1);
    assert!(rep.completed.tickets >= 1);
    let (vx, vy) = (velocity_for_project(&r.fx.c, r.fx.project, 5, Basis::Points).unwrap(), velocity_for_project(&c, res.project_id, 5, Basis::Points).unwrap());
    assert_eq!(format!("{vx:?}"), format!("{vy:?}"));
    // Die offene Zugehörigkeit ist erhalten: Defect und Story liegen im geplanten S2
    let s2 = find(&c, res.project_id, "S2");
    let content = crate::planning::sprints::sprint::sprint_content(&c, s2.id).unwrap();
    let mut t: Vec<String> = content.items.iter().map(|i| i.ticket.title.clone()).collect();
    t.sort();
    assert_eq!(t, vec!["Defect — äöü \"Quotes\", Komma; Semikolon".to_string(), "Story one".to_string()]);
}

#[test]
fn renumbering_does_not_touch_sprints_versions_and_milestones() {
    let r = rich();
    let a = export(&r);
    let res = import_project(&r.fx.c, &a, Some("NEU".into())).unwrap();
    assert!(res.renumbered);
    let b = export_project(&r.fx.c, res.project_id).unwrap();
    assert_eq!((&a.versions, &a.milestones), (&b.versions, &b.milestones));
    // Sprints sind gleich, nur die Ticketnummern der Zugehörigkeit folgen der Umnummerierung (alte Nummern → 1…n)
    let order: Vec<i64> = a.tickets.iter().map(|t| t.number).collect();
    let new_no = |n: i64| order.iter().position(|x| *x == n).unwrap() as i64 + 1;
    for (x, y) in a.sprints.iter().zip(&b.sprints) {
        assert_eq!((&x.name, &x.state, &x.goal, &x.start_date, &x.end_date, &x.created_at, &x.started_at, &x.completed_at), (&y.name, &y.state, &y.goal, &y.start_date, &y.end_date, &y.created_at, &y.started_at, &y.completed_at));
        assert_eq!(x.tickets.iter().map(|e| (new_no(e.ticket), e.added_at.clone(), e.removed_at.clone(), e.points_at_add)).collect::<Vec<_>>(), y.tickets.iter().map(|e| (e.ticket, e.added_at.clone(), e.removed_at.clone(), e.points_at_add)).collect::<Vec<_>>());
    }
    // Derselbe Name in einem anderen Projekt ist kein Konflikt
    assert_eq!(list_sprints(&r.fx.c, res.project_id).unwrap().len(), 3);
    assert_eq!(list_sprints(&r.fx.c, r.fx.project).unwrap().len(), 3);
    let active = list_sprints(&r.fx.c, res.project_id).unwrap().into_iter().find(|s| s.name == "S1").unwrap();
    let content = crate::planning::sprints::sprint::sprint_content(&r.fx.c, active.id).unwrap();
    assert!(content.items.iter().all(|i| i.ticket.project_id == res.project_id), "Zugehörigkeit zeigt auf die neuen Tickets");
    assert_eq!(content.items.len(), 3);
    let _ = r.sprint1;
}

#[test]
fn data_of_switched_off_features_is_kept() {
    // Sprints und Versionen sind ausgeschaltet, ihre Daten bleiben (die Schicht lehnt dann nur Bearbeitung ab)
    let r = rich();
    update_project(&r.fx.c, r.fx.project, &UpdateProject { use_sprints: Some(false), use_versions: Some(false), ..Default::default() }).unwrap();
    let a = export(&r);
    assert!(!a.project.use_sprints && !a.project.use_versions);
    assert_eq!((a.sprints.len(), a.versions.len(), a.milestones.len()), (3, 2, 2));
    let c = fresh();
    let res = import(&c, &a);
    assert_eq!(res.report.sprints, 3);
    assert_eq!(norm(export_project(&c, res.project_id).unwrap()), norm(a));
}

#[test]
fn rejects_bad_sprints_versions_and_milestones() {
    let r = rich();
    let good = export(&r);
    // Name doppelt (ohne Beachtung der Groß-/Kleinschreibung)
    let mut d = good.clone();
    sp(&mut d, 1).name = "s0".into();
    assert_eq!(fails(&d), BackupError::AtSprint(Where::Sprint(2), SprintError::Plan(crate::planning::PlanError::NameTaken)));
    // Name leer
    let mut d = good.clone();
    sp(&mut d, 0).name = " ".into();
    assert_eq!(fails(&d), BackupError::AtPlan(Where::Sprint(1), crate::planning::PlanError::NameEmpty));
    // Zustand, zwei aktive, Termine
    let mut d = good.clone();
    sp(&mut d, 2).state = "paused".into();
    assert!(matches!(fails(&d), BackupError::At(Where::Sprint(3), Problem::SprintState(_))));
    let mut d = good.clone();
    sp(&mut d, 0).state = "active".into();
    assert!(matches!(fails(&d), BackupError::At(Where::Sprint(2), Problem::SecondActiveSprint)));
    let mut d = good.clone();
    sp(&mut d, 1).end_date = Some("2026-10-01".into());
    assert_eq!(fails(&d), BackupError::AtSprint(Where::Sprint(2), SprintError::DatesOrder));
    let mut d = good.clone();
    sp(&mut d, 1).end_date = None;
    assert_eq!(fails(&d), BackupError::AtSprint(Where::Sprint(2), SprintError::DatesIncomplete));
    let mut d = good.clone();
    sp(&mut d, 1).start_date = Some("2.11.2026".into());
    assert_eq!(fails(&d), BackupError::AtSprint(Where::Sprint(2), SprintError::DateInvalid));
    let mut d = good.clone();
    sp(&mut d, 2).default_days = 0;
    assert!(matches!(fails(&d), BackupError::AtSprint(Where::Sprint(3), SprintError::DaysInvalid { .. })));
    let mut d = good.clone();
    sp(&mut d, 2).goal = "x".repeat(501);
    assert!(matches!(fails(&d), BackupError::AtSprint(Where::Sprint(3), SprintError::TextTooLong { .. })));
    let mut d = good.clone();
    sp(&mut d, 1).started_at = Some("gestern".into());
    assert!(matches!(fails(&d), BackupError::At(Where::Sprint(2), Problem::Timestamp(_))));
    // Zugehörigkeit
    let mut d = good.clone();
    sp(&mut d, 1).tickets[0].ticket = 999;
    assert_eq!(fails(&d), BackupError::At(Where::SprintEntry(2, 0), Problem::UnknownRef(RefKind::Ticket, 999)));
    let mut d = good.clone();
    sp(&mut d, 1).tickets[0].removed_at = Some("2020-01-01T00:00:00".into());
    assert_eq!(fails(&d), BackupError::At(Where::SprintEntry(2, 0), Problem::RemovedBeforeAdded));
    let mut d = good.clone();
    sp(&mut d, 1).tickets[0].points_at_add = Some(-1.0);
    assert_eq!(fails(&d), BackupError::At(Where::SprintEntry(2, 0), Problem::Points));
    let mut d = good.clone();
    let dup = sp(&mut d, 1).tickets[0].clone();
    sp(&mut d, 1).tickets.push(dup.clone());
    assert_eq!(fails(&d), BackupError::At(Where::SprintEntry(2, 4), Problem::SprintTicketTwice(dup.ticket)));
    // dasselbe Ticket in zwei offenen Sprints
    let mut d = good.clone();
    let first = sp(&mut d, 1).tickets[0].clone();
    sp(&mut d, 2).tickets.push(first.clone());
    assert_eq!(fails(&d), BackupError::At(Where::SprintEntry(3, 2), Problem::TicketInTwoSprints(first.ticket)));
    // Versionen und Meilensteine
    let mut d = good.clone();
    d.versions[1].name = "1.0".into();
    assert_eq!(fails(&d), BackupError::AtSprint(Where::Version(2), SprintError::Plan(crate::planning::PlanError::NameTaken)));
    let mut d = good.clone();
    d.milestones[0].due_date = Some("irgendwann".into());
    assert_eq!(fails(&d), BackupError::AtSprint(Where::Milestone(1), SprintError::DateInvalid));
    let mut d = good.clone();
    d.milestones[0].description = "y".repeat(2001);
    assert!(matches!(fails(&d), BackupError::AtSprint(Where::Milestone(1), SprintError::TextTooLong { .. })));
    let mut d = good.clone();
    d.versions[0].done_at = Some("kürzlich".into());
    assert!(matches!(fails(&d), BackupError::At(Where::Version(1), Problem::Timestamp(_))));
    // Feldwert verweist auf eine Version, die es im Dokument nicht gibt
    let mut d = good.clone();
    let rel = d.structure.types.iter().flat_map(|t| &t.fields).find(|f| f.name.as_deref() == Some("Release")).unwrap().id;
    ticket_mut(&mut d, "Story one").values.iter_mut().find(|v| v.field == rel).unwrap().value = json!(9);
    assert!(matches!(fails(&d), BackupError::At(Where::Value(_, _), Problem::UnknownRef(RefKind::Version, 9))));
    let goal = d.structure.types.iter().flat_map(|t| &t.fields).find(|f| f.name.as_deref() == Some("Goal")).unwrap().id;
    let mut d = good.clone();
    ticket_mut(&mut d, "Story one").values.iter_mut().find(|v| v.field == goal).unwrap().value = json!(0);
    assert!(matches!(fails(&d), BackupError::At(Where::Value(_, _), Problem::UnknownRef(RefKind::Milestone, 0))));
    // Texte sind zweisprachig
    let e = fails(&{
        let mut d = good.clone();
        sp(&mut d, 1).tickets[0].removed_at = Some("2020-01-01T00:00:00".into());
        d
    });
    assert!(e.message(Lang::En).starts_with("Sprint 2, ticket entry 1:") && e.message(Lang::De).starts_with("Sprint 2, Ticket-Zeile 1:"));
}

#[test]
fn views_boards_and_filters_come_along() {
    let r = rich();
    let a = export(&r);
    let c = fresh();
    let res = import(&c, &a);
    let boards = crate::planning::views::boards::list_boards(&c, res.project_id).unwrap();
    assert_eq!(boards.len(), 1);
    assert!(!boards[0].config.columns.is_empty());
    let filters = crate::planning::views::filters::list_filters(&c, Some(res.project_id)).unwrap();
    assert_eq!(filters.iter().map(|f| (f.name.as_str(), f.jql.as_str())).collect::<Vec<_>>(), vec![("Epics", "type = Epic")]);
}

// ------------------------------------------------------------------ Vorschau und Sonderfälle

#[test]
fn preview_counts_the_contents() {
    let r = rich();
    let a = export(&r);
    let p = a.preview();
    assert_eq!((p.key.as_str(), p.name.as_str()), ("PAN", "Paneldeck"));
    assert_eq!((p.statuses, p.types, p.tickets, p.comments, p.links, p.time_entries, p.boards, p.filters), (3, 6, 7, 2, 4, 2, 1, 1));
    assert_eq!((p.sprints, p.versions, p.milestones), (3, 2, 2));
    assert_eq!(p.history, a.tickets.iter().map(|t| t.history.len()).sum::<usize>());
    assert!(p.fields > 10);
    assert_eq!(p.app_version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_required_field_added_later_does_not_block_the_restore() {
    let r = rich();
    let c = &r.fx.c;
    let steps = crate::planning::fields::list_fields(c, r.fx.defect).unwrap().into_iter().find(|f| f.key.as_deref() == Some("steps")).unwrap();
    crate::planning::fields::update_field(c, steps.id, &crate::planning::fields::UpdateField { required: Some(true), ..Default::default() }).unwrap();
    let a = export(&r);
    let c2 = fresh();
    assert_eq!(import(&c2, &a).report.tickets, 7);
}

#[test]
fn history_of_deleted_things_stays_harmless() {
    let r = rich();
    let c = &r.fx.c;
    // Status „progress“ wird gelöscht (mit Ersatz): alte Verlaufseinträge verweisen dann auf eine fremde Kennung
    crate::planning::statuses::delete_status(c, r.fx.progress, Some(DeleteMode::Migrate { to: r.fx.todo })).unwrap();
    let a = export(&r);
    let c2 = fresh();
    let res = import(&c2, &a);
    assert_eq!(res.report.tickets, 7);
    let t1 = crate::planning::tickets::ticket_by_key(&c2, &format!("PAN-{}", r.task1.number)).unwrap();
    for lang in [Lang::En, Lang::De] {
        for e in history_of(&c2, t1.id).unwrap() {
            assert!(!describe(&c2, &e, lang).is_empty());
        }
    }
}

#[test]
fn big_project_with_2000_tickets_exports_and_imports_quickly() {
    let fx = Fx::new();
    let c = &fx.c;
    let src = Source::ui();
    let prio = fx.field(fx.task, "priority");
    let desc = fx.field(fx.task, "description");
    let epic = fx.mk(fx.epic, "Big epic");
    let story = fx.child(&epic, fx.story, "Big story");
    for i in 0..1998 {
        let t = create_child(c, story.id, fx.task, &format!("Task {i}"), vec![(prio.id, json!(1 + (i % 5))), (desc.id, json!(format!("Beschreibung {i}\nzweite Zeile")))], &src).unwrap().0;
        if i % 4 == 0 {
            add_comment(c, t.id, &format!("Kommentar {i}"), &src).unwrap();
        }
        if i % 10 == 0 {
            transition_ticket(c, t.id, fx.progress, &[], &src).unwrap();
        }
    }
    let t0 = std::time::Instant::now();
    let a = export_project(c, fx.project).unwrap();
    let t_export = t0.elapsed();
    assert_eq!(a.tickets.len(), 2000);
    let json = a.to_json();
    let back = ProjectDoc::from_json(&json).unwrap();
    let c2 = fresh();
    let t1 = std::time::Instant::now();
    let res = import(&c2, &back);
    let t_import = t1.elapsed();
    assert_eq!(res.report.tickets, 2000);
    assert_eq!(res.report.comments, 500);
    let b = export_project(&c2, res.project_id).unwrap();
    assert_eq!(norm(a), norm(b));
    eprintln!("2000 Tickets: Export {t_export:?}, Import {t_import:?}, JSON {} KB", json.len() / 1024);
    // Allein unter einer Sekunde; die Grenzen sind großzügig, weil die Tests parallel laufen.
    assert!(t_export.as_secs() < 60 && t_import.as_secs() < 90, "export {t_export:?}, import {t_import:?}");
    // mit Umnummerierung ebenso
    let c3 = fresh();
    let res = import_project(&c3, &back, Some("BIG".into())).unwrap();
    assert!(res.renumbered);
    assert_eq!(titles(&c3, res.project_id).len(), 2000);
}

// ------------------------------------------------------------------ Grenzen (Audit vor 2.0.0)

/// Ein Dokument mit Zählern nahe `i64::MAX` darf nicht angelegt werden: Beim Weiterzählen liefe `next_number + 1` über
/// (Release: wird negativ), und `MAX(rank) + 1` in SQLite würde zur Kommazahl, die sich nicht mehr als Ganzzahl lesen lässt.
#[test]
fn rejects_counters_and_ranks_that_would_overflow() {
    let r = rich();
    let good = export(&r);
    for n in [i64::MAX, MAX_TICKET_NUMBER + 1] {
        let mut d = good.clone();
        d.project.next_number = n;
        assert_eq!(fails(&d), BackupError::At(Where::Project, Problem::TicketNumber(n)), "next_number {n}");
    }
    for n in [i64::MAX, MAX_TICKET_NUMBER + 1] {
        let mut d = good.clone();
        d.tickets[0].number = n;
        assert_eq!(fails(&d), BackupError::At(Where::Ticket(n), Problem::TicketNumber(n)), "Ticketnummer {n}");
    }
    for rk in [i64::MAX, i64::MIN, MAX_RANK + 1, -MAX_RANK - 1] {
        let mut d = good.clone();
        let number = d.tickets[0].number;
        d.tickets[0].rank = rk;
        assert_eq!(fails(&d), BackupError::At(Where::Ticket(number), Problem::Rank(rk)), "rank {rk}");
    }
    // die Grenzwerte selbst gehen, und das Projekt bleibt benutzbar (Ticket anlegen, Liste lesen)
    let mut d = good.clone();
    d.project.next_number = MAX_TICKET_NUMBER;
    let last = d.tickets.len() - 1;
    d.tickets[last].rank = MAX_RANK;
    d.tickets[0].rank = -MAX_RANK;
    let c = fresh();
    let res = import_project(&c, &d, None).unwrap();
    let ty = crate::planning::types::list_types(&c, res.project_id).unwrap()[0].id;
    let (t, _) = create_ticket(&c, &NewTicket::new(res.project_id, ty, "Danach"), &Source::ui()).unwrap();
    assert_eq!(t.number, MAX_TICKET_NUMBER);
    let page = crate::planning::ticket_list::list_tickets(&c, Some(res.project_id), &Default::default()).unwrap();
    assert_eq!(page.items.len(), d.tickets.len() + 1);
}

#[test]
fn malicious_json_is_rejected_without_panic() {
    let r = rich();
    let good = export(&r).to_json();
    // zu tief verschachtelt (serde_json begrenzt die Tiefe, kein Stapelüberlauf)
    let deep = format!("{{\"format\":\"{FORMAT}\",\"version\":2,\"x\":{}{}}}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(matches!(ProjectDoc::from_json(&deep), Err(BackupError::Json(_))));
    // falsche Typen, Null, Zahlen als Text, abgeschnitten, nur BOM
    for bad in ["null", "\"x\"", "42", "{\"format\": 5}", "{\"format\":\"paneldeck-project\",\"version\":\"2\"}", "\u{feff}", "", "{\"format\":\"paneldeck-project\",\"version\":-1}", "{\"format\":\"paneldeck-project\",\"version\":99999999999}"] {
        assert!(ProjectDoc::from_json(bad).is_err(), "{bad:?}");
    }
    assert!(ProjectDoc::from_json(&good[..good.len() / 2]).is_err(), "abgeschnitten");
    // mit BOM ist das gute Dokument lesbar
    assert!(ProjectDoc::from_json(&format!("\u{feff}{good}")).is_ok());
}

/// Doppelte Verweise, Selbst- und Kreis-Eltern, Verweise ins Leere: der Import bricht ab und lässt die Datenbank unverändert.
#[test]
fn cycles_duplicates_and_dangling_references_roll_back() {
    let r = rich();
    let good = export(&r);
    let (a, b) = (good.tickets[0].number, good.tickets[1].number);
    // Eltern-Kreis a <-> b
    let mut d = good.clone();
    d.tickets[0].parent = Some(b);
    d.tickets[1].parent = Some(a);
    assert!(matches!(fails(&d), BackupError::AtTicket(_, _) | BackupError::At(_, _)));
    // Verknüpfung auf sich selbst
    let mut d = good.clone();
    let lt = d.link_types.iter().find(|l| l.target_kind == "ticket").unwrap().id;
    d.tickets[0].links.push(DocLink { link_type: lt, target: DocTarget::Ticket { number: a }, created_at: "2026-01-01T00:00:00".into() });
    assert!(matches!(fails(&d), BackupError::AtTicket(_, TicketError::LinkSelf)));
    // Verweis auf Sprint-Ticket, das es nicht gibt
    let mut d = good.clone();
    d.sprints[0].tickets.push(DocSprintTicket { ticket: 99_999, added_at: "2026-01-01T00:00:00".into(), removed_at: None, points_at_add: None });
    assert!(matches!(fails(&d), BackupError::At(Where::SprintEntry(..), Problem::UnknownRef(RefKind::Ticket, 99_999))));
    // Verlauf mit Zeitstempel-Müll und Quelle
    let mut d = good.clone();
    d.tickets[0].history.push(DocHistory { at: "gestern".into(), source: "ui".into(), source_detail: None, field: "title".into(), old: None, new: None, batch: 1 });
    assert!(matches!(fails(&d), BackupError::At(Where::History(..), Problem::Timestamp(_))));
}

#[test]
fn oversized_files_are_not_read() {
    let dir = std::env::temp_dir().join(format!("paneldeck-test-readlimit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let small = dir.join("klein.json");
    std::fs::write(&small, b"{}").unwrap();
    assert_eq!(read_limited(&small).unwrap().unwrap(), b"{}");
    let big = dir.join("gross.json");
    let f = std::fs::File::create(&big).unwrap();
    f.set_len(MAX_FILE_BYTES + 1).unwrap();
    drop(f);
    assert_eq!(read_limited(&big).unwrap().unwrap_err(), BackupError::TooLarge { max_mb: 128 });
    assert!(read_limited(&dir.join("gibt-es-nicht.json")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sprint_with_an_absurd_period_is_rejected_by_the_import() {
    let r = rich();
    let mut d = export(&r);
    d.sprints[0].start_date = Some("0026-10-05".into());
    assert!(matches!(fails(&d), BackupError::AtSprint(Where::Sprint(1), SprintError::DatesTooLong { .. })));
}

/// Warum `MAX_RANK` und `MAX_TICKET_NUMBER` nötig sind: SQLite rechnet über `i64::MAX` hinaus in Kommazahlen weiter.
#[test]
fn sqlite_turns_integer_overflow_into_a_float() {
    let c = fresh();
    let v: rusqlite::types::Value = c.query_row("SELECT 9223372036854775807 + 1", [], |r| r.get(0)).unwrap();
    assert!(matches!(v, rusqlite::types::Value::Real(_)), "{v:?}");
    c.execute_batch("CREATE TABLE t (rank INTEGER); INSERT INTO t VALUES (9223372036854775807);").unwrap();
    let n: rusqlite::Result<i64> = c.query_row("SELECT MAX(rank) + 1 FROM t", [], |r| r.get(0));
    assert!(n.is_err(), "die Zahl ist als Ganzzahl nicht mehr lesbar");
}

#[test]
fn lucide_icons_survive_the_backup_and_unknown_ones_do_not_stop_the_import() {
    use crate::planning::types::{list_types, update_type, UpdateType};
    let r = rich();
    let c = &r.fx.c;
    let note = list_types(c, r.fx.project).unwrap().into_iter().find(|t| t.name.as_deref() == Some("Note")).unwrap().id;
    update_type(c, note, &UpdateType { icon: Some(Some("lucide:alarm-clock".into())), ..Default::default() }).unwrap();
    // Ein Icon aus einer neueren Lucide-Version (formal richtig, hier unbekannt) wird gespeichert und bleibt erhalten.
    update_type(c, r.fx.task, &UpdateType { icon: Some(Some("lucide:icon-from-a-newer-version".into())), ..Default::default() }).unwrap();
    let a = export(&r);
    let icons: Vec<Option<&str>> = a.structure.types.iter().map(|t| t.icon.as_deref()).collect();
    assert!(icons.contains(&Some("lucide:alarm-clock")) && icons.contains(&Some("lucide:icon-from-a-newer-version")));
    let back = ProjectDoc::from_json(&a.to_json()).unwrap();
    let c2 = fresh();
    let res = import(&c2, &back);
    assert_eq!(norm(export_project(&c2, res.project_id).unwrap()), norm(a.clone()));
    // Formal falsche Namen lehnt der Import ab (atomar).
    let mut bad = a;
    bad.structure.types.iter_mut().find(|t| t.icon.as_deref() == Some("lucide:alarm-clock")).unwrap().icon = Some("lucide:Alarm Clock".into());
    let e = fails(&bad);
    assert!(format!("{e:?}").contains("IconInvalid"), "{e:?}");
}
