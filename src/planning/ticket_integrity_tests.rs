//! Tests der Integritätsregel mit **echten** Tickets (die Tests in `usage_tests.rs` simulieren die Benutzung nur): Zähler,
//! `InUse`, Umhängen (`Migrate`) und Verwerfen (`Discard`) für Status, Typen, Felder, Dropdown-Werte, Verknüpfungstypen, Arten
//! der Arbeit und Projekte.

use super::comments::add_comment;
use super::fields::{delete_field, get_field, update_field, FieldKind, UpdateField};
use super::history::{history_of, HistoryField, Source};
use super::integrity::{self, DeleteMode, TypeDeleteMode, TypeMigration};
use super::link_types::{create_link_type, delete_link_type, link_type_by_key, NewLinkType, TargetKind};
use super::links::{add_link, links_of, Target};
use super::projects::{delete_impact, delete_project, project_by_key};
use super::statuses::{create_status, delete_status, get_status, NewStatus, StatusCategory};
use super::ticket_error::TicketError;
use super::ticket_list::{list_tickets, TicketFilter};
use super::tickets::fixture::Fx;
use super::tickets::{create_ticket, get_ticket, set_field, NewTicket};
use super::time_entries::{entries_of, log_time, tests_now};
use super::transition::transition_ticket;
use super::types::{delete_type, get_type, list_types, parents_of, set_parents, suggest_field_map};
use super::values::get_value;
use super::work_kinds::{create_work_kind, delete_work_kind, list_work_kinds};
use super::workflow::{set_workflow, set_workflow_statuses, workflow_statuses};
use super::PlanError;
use crate::state::Lang;
use serde_json::json;

fn ui() -> Source {
    Source::ui()
}

#[test]
fn counts_come_from_the_real_tables() {
    let f = Fx::new();
    let story = f.mk(f.story, "S");
    let t1 = f.child(&story, f.task, "T1");
    let t2 = f.child(&story, f.task, "T2");
    transition_ticket(&f.c, t2.id, f.progress, &[], &ui()).unwrap();
    let sp = f.field(f.task, "story_points");
    set_field(&f.c, t1.id, sp.id, json!(3), &ui()).unwrap();
    set_field(&f.c, t2.id, sp.id, json!(5), &ui()).unwrap();
    let c = &f.c;
    assert_eq!(integrity::usage_count_type(c, f.task).unwrap(), 2);
    assert_eq!(integrity::usage_count_type(c, f.story).unwrap(), 1);
    assert_eq!(integrity::usage_count_type(c, f.defect).unwrap(), 0);
    assert_eq!(integrity::usage_count_status(c, f.todo).unwrap(), 2, "Story und T1");
    assert_eq!(integrity::usage_count_status(c, f.progress).unwrap(), 1);
    assert_eq!(integrity::usage_count_status(c, f.done).unwrap(), 0);
    assert_eq!(integrity::usage_count_type_workflow(c, f.task).unwrap(), 2);
    assert_eq!(integrity::usage_count_type_status(c, f.task, f.progress).unwrap(), 1);
    assert_eq!(integrity::usage_count_type_status(c, f.task, f.done).unwrap(), 0);
    assert_eq!(integrity::usage_count_parent_rule(c, f.task, f.story).unwrap(), 2);
    assert_eq!(integrity::usage_count_parent_rule(c, f.task, f.epic).unwrap(), 0);
    assert_eq!(integrity::usage_count_field(c, sp.id).unwrap(), 2);
    assert_eq!(integrity::usage_count_project(c, f.project).unwrap(), 3);
    assert_eq!(delete_impact(c, f.project).unwrap(), 3);
    assert_eq!(super::statuses::usage(c, f.todo).unwrap(), 2);
    assert_eq!(super::types::usage(c, f.task).unwrap(), 2);
    assert_eq!(super::fields::usage(c, sp.id).unwrap(), 2);
}

#[test]
fn deleting_a_used_status_needs_a_mode_and_moves_or_clears_the_tickets() {
    let f = Fx::new();
    let a = f.mk(f.task, "A");
    let b = f.mk(f.defect, "B");
    transition_ticket(&f.c, a.id, f.progress, &[], &ui()).unwrap();
    transition_ticket(&f.c, b.id, f.progress, &[], &ui()).unwrap();
    assert_eq!(delete_status(&f.c, f.progress, None), Err(PlanError::InUse { count: 2 }));
    assert!(get_status(&f.c, f.progress).is_ok());
    // Ersatz im gleichen Pool: Tickets wandern, Workflows und Übergänge werden umgeleitet
    delete_status(&f.c, f.progress, Some(DeleteMode::Migrate { to: f.done })).unwrap();
    assert_eq!(get_ticket(&f.c, a.id).unwrap().status_id, Some(f.done));
    assert_eq!(get_ticket(&f.c, b.id).unwrap().status_id, Some(f.done));
    assert!(!workflow_statuses(&f.c, f.task).unwrap().contains(&f.progress));
    // Verwerfen: Status der Tickets wird geleert
    let c = f.mk(f.task, "C");
    assert_eq!(c.status_id, Some(f.todo));
    assert_eq!(delete_status(&f.c, f.todo, None), Err(PlanError::InUse { count: 1 }));
    delete_status(&f.c, f.todo, Some(DeleteMode::Discard)).unwrap();
    assert_eq!(get_ticket(&f.c, c.id).unwrap().status_id, None);
    assert_eq!(get_ticket(&f.c, a.id).unwrap().status_id, Some(f.done), "andere bleiben");
}

#[test]
fn migrating_a_status_outside_the_workflow_uses_the_start_status() {
    let f = Fx::new();
    let extra = create_status(&f.c, &NewStatus { project_id: f.project, key: None, name: Some("Review".into()), color: "#445566".into(), category: StatusCategory::Progress }).unwrap();
    let other = create_status(&f.c, &NewStatus { project_id: f.project, key: None, name: Some("Anderes".into()), color: "#445566".into(), category: StatusCategory::Progress }).unwrap();
    let t = f.mk(f.task, "T");
    f.c.execute("UPDATE tickets SET status_id = ?1 WHERE id = ?2", [extra, t.id]).unwrap();
    integrity::migrate_tickets_status(&f.c, extra, Some(other)).unwrap();
    assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.todo), "Ziel nicht im Workflow: Startstatus");
    f.c.execute("UPDATE tickets SET status_id = ?1 WHERE id = ?2", [extra, t.id]).unwrap();
    integrity::migrate_tickets_status(&f.c, extra, None).unwrap();
    assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, None);
}

#[test]
fn deleting_a_used_type_migrates_tickets_with_their_values_and_status() {
    let f = Fx::new();
    let story = f.mk(f.story, "S");
    let t = f.child(&story, f.task, "T");
    let sub = f.child(&t, f.subtask, "Sub");
    let desc = f.field(f.task, "description");
    let sp = f.field(f.task, "story_points");
    set_field(&f.c, t.id, desc.id, json!("Text"), &ui()).unwrap();
    set_field(&f.c, t.id, sp.id, json!(8), &ui()).unwrap();
    transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
    assert_eq!(delete_type(&f.c, f.task, None), Err(PlanError::InUse { count: 1 }));
    let mapping = suggest_field_map(&f.c, f.task, f.defect).unwrap();
    delete_type(&f.c, f.task, Some(TypeDeleteMode::Migrate(TypeMigration { to_type: f.defect, fields: mapping }))).unwrap();
    assert!(get_type(&f.c, f.task).is_err());
    let now = get_ticket(&f.c, t.id).unwrap();
    assert_eq!((now.type_id, now.status_id, now.parent_id), (f.defect, Some(f.progress), Some(story.id)));
    assert_eq!(now.key(), "PAN-2", "Schlüssel bleibt");
    let desc_d = f.field(f.defect, "description");
    assert_eq!(get_value(&f.c, t.id, &desc_d).unwrap(), Some(json!("Text")));
    assert_eq!(get_ticket(&f.c, sub.id).unwrap().parent_id, Some(t.id), "Kinder bleiben, Regel erlaubt es");
    // nichts vom alten Typ übrig
    let n: i64 = f.c.query_row("SELECT COUNT(*) FROM ticket_values WHERE ticket_id = ?1 AND field_id = ?2", [t.id, sp.id], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    // der Verlauf nennt die Quelle „migration“
    let h = history_of(&f.c, t.id).unwrap();
    let ty = h.iter().find(|e| e.field == HistoryField::Type).unwrap();
    assert_eq!(ty.source.detail.as_deref(), Some("migration"));
}

#[test]
fn deleting_a_used_type_with_discard_removes_tickets_and_detaches_children() {
    let f = Fx::new();
    let story = f.mk(f.story, "S");
    let t1 = f.child(&story, f.task, "T1");
    let t2 = f.child(&story, f.task, "T2");
    let sub = f.child(&t1, f.subtask, "Sub");
    add_comment(&f.c, t1.id, "weg", &ui()).unwrap();
    assert_eq!(delete_type(&f.c, f.task, None), Err(PlanError::InUse { count: 2 }));
    delete_type(&f.c, f.task, Some(TypeDeleteMode::Discard)).unwrap();
    assert!(get_ticket(&f.c, t1.id).unwrap_err().is_not_found());
    assert!(get_ticket(&f.c, t2.id).unwrap_err().is_not_found());
    assert_eq!(get_ticket(&f.c, sub.id).unwrap().parent_id, None);
    assert!(get_ticket(&f.c, story.id).is_ok());
    let n: i64 = f.c.query_row("SELECT COUNT(*) FROM comments", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    assert_eq!(list_tickets(&f.c, Some(f.project), &TicketFilter::default()).unwrap().total, 2);
}

#[test]
fn deleting_a_used_field_moves_or_drops_values() {
    let f = Fx::new();
    let a = f.add_field(f.task, "Alt", FieldKind::Text, Default::default(), false);
    let b = f.add_field(f.task, "Neu", FieldKind::Text, Default::default(), false);
    let (t1, t2, t3) = (f.mk(f.task, "1"), f.mk(f.task, "2"), f.mk(f.task, "3"));
    set_field(&f.c, t1.id, a.id, json!("eins"), &ui()).unwrap();
    set_field(&f.c, t2.id, a.id, json!("zwei"), &ui()).unwrap();
    set_field(&f.c, t2.id, b.id, json!("schon da"), &ui()).unwrap();
    assert_eq!(delete_field(&f.c, a.id, None), Err(PlanError::InUse { count: 2 }));
    delete_field(&f.c, a.id, Some(DeleteMode::Migrate { to: b.id })).unwrap();
    assert_eq!(get_value(&f.c, t1.id, &b).unwrap(), Some(json!("eins")));
    assert_eq!(get_value(&f.c, t2.id, &b).unwrap(), Some(json!("schon da")), "vorhandener Wert bleibt");
    assert_eq!(get_value(&f.c, t3.id, &b).unwrap(), None);
    // Verwerfen
    delete_field(&f.c, b.id, Some(DeleteMode::Discard)).unwrap();
    let n: i64 = f.c.query_row("SELECT COUNT(*) FROM ticket_values WHERE field_id = ?1", [b.id], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    // ein Mehrfachwert zählt als ein Ticket
    let m = f.add_field(f.task, "Mehrfach", FieldKind::Multi, Fx::dropdown(&["a", "b", "c"]), false);
    set_field(&f.c, t1.id, m.id, json!([1, 2, 3]), &ui()).unwrap();
    assert_eq!(integrity::usage_count_field(&f.c, m.id).unwrap(), 1);
    assert_eq!(integrity::usage_count_option(&f.c, m.id, 2).unwrap(), 1);
    assert_eq!(integrity::usage_count_option(&f.c, m.id, 9).unwrap(), 0);
}

#[test]
fn dropdown_values_are_translated_when_a_field_is_migrated() {
    let f = Fx::new();
    let a = f.add_field(f.task, "Farbe alt", FieldKind::Dropdown, Fx::dropdown(&["Rot", "Grün"]), false);
    let b = f.add_field(f.task, "Farbe neu", FieldKind::Dropdown, Fx::dropdown(&["grün", "Blau"]), false);
    let (t1, t2) = (f.mk(f.task, "1"), f.mk(f.task, "2"));
    set_field(&f.c, t1.id, a.id, json!(2), &ui()).unwrap(); // Grün
    set_field(&f.c, t2.id, a.id, json!(1), &ui()).unwrap(); // Rot (gibt es im Ziel nicht)
    delete_field(&f.c, a.id, Some(DeleteMode::Migrate { to: b.id })).unwrap();
    assert_eq!(get_value(&f.c, t1.id, &b).unwrap(), Some(json!(1)));
    assert_eq!(get_value(&f.c, t2.id, &b).unwrap(), None);
}

#[test]
fn a_used_dropdown_option_cannot_be_removed_but_an_unused_one_can() {
    let f = Fx::new();
    let d = f.add_field(f.task, "Stufe", FieldKind::Dropdown, Fx::dropdown(&["a", "b", "c"]), false);
    let (t1, t2) = (f.mk(f.task, "1"), f.mk(f.task, "2"));
    set_field(&f.c, t1.id, d.id, json!(2), &ui()).unwrap();
    set_field(&f.c, t2.id, d.id, json!(2), &ui()).unwrap();
    let mut o = get_field(&f.c, d.id).unwrap().options;
    o.values.remove(1);
    assert_eq!(update_field(&f.c, d.id, &UpdateField { options: Some(o), ..Default::default() }), Err(PlanError::InUse { count: 2 }));
    let mut o = get_field(&f.c, d.id).unwrap().options;
    o.values.remove(2);
    update_field(&f.c, d.id, &UpdateField { options: Some(o), ..Default::default() }).unwrap();
    assert_eq!(get_value(&f.c, t1.id, &get_field(&f.c, d.id).unwrap()).unwrap(), Some(json!(2)));
}

#[test]
fn deleting_a_used_link_type_moves_or_removes_links() {
    let f = Fx::new();
    let l1 = create_link_type(&f.c, &NewLinkType { name_out: "klont".into(), name_in: Some("wird geklont von".into()), symmetric: false, target_kind: TargetKind::Ticket }).unwrap();
    let l2 = create_link_type(&f.c, &NewLinkType { name_out: "ersetzt".into(), name_in: Some("wird ersetzt von".into()), symmetric: false, target_kind: TargetKind::Ticket }).unwrap();
    let (a, b, c) = (f.mk(f.task, "A"), f.mk(f.task, "B"), f.mk(f.task, "C"));
    add_link(&f.c, a.id, l1, Target::Ticket(b.id), &ui()).unwrap();
    add_link(&f.c, a.id, l2, Target::Ticket(b.id), &ui()).unwrap();
    add_link(&f.c, a.id, l1, Target::Ticket(c.id), &ui()).unwrap();
    assert_eq!(delete_link_type(&f.c, l1, None), Err(PlanError::InUse { count: 2 }));
    delete_link_type(&f.c, l1, Some(DeleteMode::Migrate { to: l2 })).unwrap();
    let v = links_of(&f.c, a.id, Lang::De).unwrap();
    assert_eq!(v.len(), 2, "das Duplikat A -> B entfällt");
    assert!(v.iter().all(|x| x.link_type_id == l2 && x.name == "ersetzt"));
    delete_link_type(&f.c, l2, Some(DeleteMode::Discard)).unwrap();
    assert!(links_of(&f.c, a.id, Lang::De).unwrap().is_empty());
    // der System-Typ „terminal“ bleibt trotz Links geschützt
    let term = link_type_by_key(&f.c, "terminal").unwrap().id;
    add_link(&f.c, a.id, term, Target::Terminal("s".into()), &ui()).unwrap();
    assert_eq!(delete_link_type(&f.c, term, Some(DeleteMode::Discard)), Err(PlanError::SystemProtected));
    assert_eq!(integrity::usage_count_link_type(&f.c, term).unwrap(), 1);
}

#[test]
fn deleting_a_used_work_kind_moves_or_clears_the_time_entries() {
    let f = Fx::new();
    let t = f.mk(f.task, "T");
    let kinds = list_work_kinds(&f.c, f.project).unwrap();
    let extra = create_work_kind(&f.c, f.project, None, Some("Review")).unwrap();
    log_time(&f.c, t.id, "1h", None, Some(kinds[0].id), "", tests_now(), &ui()).unwrap();
    log_time(&f.c, t.id, "2h", None, Some(kinds[0].id), "", tests_now(), &ui()).unwrap();
    log_time(&f.c, t.id, "3h", None, Some(kinds[1].id), "", tests_now(), &ui()).unwrap();
    assert_eq!(integrity::usage_count_work_kind(&f.c, kinds[0].id).unwrap(), 2);
    assert_eq!(delete_work_kind(&f.c, kinds[0].id, None), Err(PlanError::InUse { count: 2 }));
    delete_work_kind(&f.c, kinds[0].id, Some(DeleteMode::Migrate { to: extra })).unwrap();
    assert_eq!(entries_of(&f.c, t.id).unwrap().iter().filter(|e| e.work_kind_id == Some(extra)).count(), 2);
    delete_work_kind(&f.c, kinds[1].id, Some(DeleteMode::Discard)).unwrap();
    let e = entries_of(&f.c, t.id).unwrap();
    assert_eq!(e.iter().filter(|e| e.work_kind_id.is_none()).count(), 1);
    assert_eq!(e.len(), 3, "Zeiten bleiben erhalten");
}

#[test]
fn deleting_a_project_reports_the_real_count_and_removes_everything() {
    let f = Fx::new();
    let other = f.other_project("OTH");
    let o_type = list_types(&f.c, other).unwrap()[0].id;
    let o_ticket = create_ticket(&f.c, &NewTicket::new(other, o_type, "O"), &ui()).unwrap().0;
    let epic = f.mk(f.epic, "E");
    f.child(&epic, f.story, "S");
    let t = f.mk(f.task, "T");
    add_comment(&f.c, t.id, "x", &ui()).unwrap();
    log_time(&f.c, t.id, "1h", None, None, "", tests_now(), &ui()).unwrap();
    add_link(&f.c, o_ticket.id, link_type_by_key(&f.c, "relates").unwrap().id, Target::Ticket(t.id), &ui()).unwrap();
    add_link(&f.c, t.id, link_type_by_key(&f.c, "terminal").unwrap().id, Target::Terminal("s1".into()), &ui()).unwrap();
    assert_eq!(delete_impact(&f.c, f.project).unwrap(), 3);
    assert_eq!(delete_project(&f.c, f.project).unwrap(), 3);
    let q = |sql: &str| -> i64 { f.c.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(q(&format!("SELECT COUNT(*) FROM tickets WHERE project_id = {}", f.project)), 0);
    assert_eq!(q("SELECT COUNT(*) FROM comments"), 0);
    assert_eq!(q("SELECT COUNT(*) FROM time_entries"), 0);
    assert_eq!(q(&format!("SELECT COUNT(*) FROM history WHERE ticket_id = {}", t.id)), 0);
    assert_eq!(q("SELECT COUNT(*) FROM links"), 0, "auch die Verknüpfung vom anderen Projekt aus");
    assert!(get_ticket(&f.c, o_ticket.id).is_ok(), "das andere Projekt bleibt");
    assert!(links_of(&f.c, o_ticket.id, Lang::En).unwrap().is_empty());
    // das System-Projekt hat nie Tickets und bleibt geschützt
    let todo = project_by_key(&f.c, "TODO").unwrap();
    assert_eq!(delete_project(&f.c, todo.id), Err(PlanError::SystemProtected));
}

#[test]
fn workflow_and_hierarchy_changes_respect_real_usage() {
    let f = Fx::new();
    let story = f.mk(f.story, "S");
    let t = f.child(&story, f.task, "T");
    // Eltern-Regel mit Tickets lässt sich nicht entfernen
    assert_eq!(set_parents(&f.c, f.task, &[f.epic]), Err(PlanError::InUse { count: 1 }));
    assert_eq!(parents_of(&f.c, f.task).unwrap(), vec![f.epic, f.story]);
    // Status mit Tickets lässt sich nicht aus dem Workflow nehmen
    assert_eq!(set_workflow_statuses(&f.c, f.task, &[f.progress, f.done]), Err(PlanError::InUse { count: 1 }));
    transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
    assert_eq!(set_workflow_statuses(&f.c, f.task, &[f.todo, f.done]), Err(PlanError::InUse { count: 1 }));
    // Workflow abschalten: Verwerfen leert die Status
    assert_eq!(set_workflow(&f.c, f.task, false, false), Err(PlanError::InUse { count: 1 }));
    set_workflow(&f.c, f.task, false, true).unwrap();
    assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, None);
    // und die Transition ist dann nicht mehr möglich
    assert_eq!(transition_ticket(&f.c, t.id, f.done, &[], &ui()).unwrap_err(), TicketError::NoWorkflow);
}

#[test]
fn deleting_a_ticket_type_cleans_ticket_field_targets_and_keeps_other_tickets() {
    let f = Fx::new();
    let epic_field = f.field(f.story, "epic");
    let e = f.mk(f.epic, "E");
    let s = f.mk(f.story, "S");
    set_field(&f.c, s.id, epic_field.id, json!(e.id), &ui()).unwrap();
    // Epic löschen (verwerfen): das Ticket-Feld der Story verliert seinen Verweis, die Story bleibt
    delete_type(&f.c, f.epic, Some(TypeDeleteMode::Discard)).unwrap();
    assert!(get_ticket(&f.c, e.id).unwrap_err().is_not_found());
    assert_eq!(get_value(&f.c, s.id, &get_field(&f.c, epic_field.id).unwrap()).unwrap(), None);
    assert!(get_field(&f.c, epic_field.id).unwrap().options.target_types.is_empty(), "Ziel-Typ entfernt");
    let h = history_of(&f.c, s.id).unwrap();
    assert!(h.iter().any(|x| x.field == HistoryField::Custom(epic_field.id) && x.new.is_none()));
}

#[test]
fn foreign_keys_stay_consistent_after_a_busy_scenario() {
    let f = Fx::new();
    let other = f.other_project("OTH");
    let o_task = list_types(&f.c, other).unwrap().iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
    let epic = f.mk(f.epic, "E");
    let story = f.child(&epic, f.story, "S");
    let t1 = f.child(&story, f.task, "T1");
    let t2 = f.child(&story, f.task, "T2");
    let relates = link_type_by_key(&f.c, "relates").unwrap().id;
    add_link(&f.c, t1.id, relates, Target::Ticket(t2.id), &ui()).unwrap();
    add_comment(&f.c, t1.id, "x", &ui()).unwrap();
    log_time(&f.c, t1.id, "1h", None, Some(list_work_kinds(&f.c, f.project).unwrap()[0].id), "", tests_now(), &ui()).unwrap();
    transition_ticket(&f.c, t2.id, f.progress, &[], &ui()).unwrap();
    super::clone_move::change_type(&f.c, t1.id, o_task, &suggest_field_map(&f.c, f.task, o_task).unwrap(), &[], &ui()).unwrap();
    super::clone_move::clone_ticket(&f.c, story.id, &super::clone_move::CloneOptions { include_children: true, ..Default::default() }, &ui()).unwrap();
    delete_status(&f.c, f.progress, Some(DeleteMode::Migrate { to: f.done })).unwrap();
    delete_type(&f.c, f.task, Some(TypeDeleteMode::Discard)).unwrap();
    delete_project(&f.c, other).unwrap();
    super::tickets::delete_ticket(&f.c, epic.id, &ui()).unwrap();
    let bad: i64 = f.c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
    assert_eq!(bad, 0, "Fremdschlüssel verletzt");
    let dangling: i64 = f.c.query_row("SELECT COUNT(*) FROM tickets WHERE parent_id IS NOT NULL AND parent_id NOT IN (SELECT id FROM tickets)", [], |r| r.get(0)).unwrap();
    assert_eq!(dangling, 0);
}
