//! Tests der Versionen und Meilensteine: Verwaltung, Fortschritt, Löschen in Benutzung, Prüfung der Feldwerte, Optionalität.

use super::releases::*;
use super::tests_sprint::{d, S};
use super::*;
use crate::planning::fields::{get_field, Field, FieldKind, FieldOptions};
use crate::planning::history::Source;
use crate::planning::integrity::{self, DeleteMode};
use crate::planning::projects::{update_project, UpdateProject};
use crate::planning::tickets::{create_ticket, get_ticket, set_field, NewTicket, Ticket};
use crate::planning::ticket_error::TicketError;
use crate::planning::values::{get_value, normalize_value, RefCtx};
use crate::planning::PlanError;
use crate::state::Lang;
use serde_json::json;

fn ver(s: &S, name: &str) -> i64 {
    create_version(s.c(), &NewRelease::new(s.p(), name)).unwrap()
}

fn mile(s: &S, name: &str) -> i64 {
    create_milestone(s.c(), &NewRelease::new(s.p(), name)).unwrap()
}

/// Je ein Versions- und ein Meilenstein-Feld an den Entwickler-Aufgaben.
fn fields(s: &S) -> (Field, Field) {
    (
        s.f.add_field(s.f.task, "Fix version", FieldKind::Version, FieldOptions::default(), false),
        s.f.add_field(s.f.task, "Target milestone", FieldKind::Milestone, FieldOptions::default(), false),
    )
}

fn assign(s: &S, t: &Ticket, f: &Field, id: i64) {
    set_field(s.c(), t.id, f.id, json!(id), &Source::ui()).unwrap();
}

// ------------------------------------------------------------------ Verwaltung

#[test]
fn versions_are_created_at_the_end_and_listed_in_order() {
    let s = S::new();
    let (a, b) = (ver(&s, "  1.0 "), ver(&s, "2.0"));
    let l = list_versions(s.c(), s.p()).unwrap();
    assert_eq!(l.iter().map(|v| (v.id, v.name.as_str(), v.position)).collect::<Vec<_>>(), vec![(a, "1.0", 0), (b, "2.0", 1)]);
    let v = get_version(s.c(), a).unwrap();
    assert_eq!((v.done, v.done_at, v.due_date, v.kind), (false, None, None, ReleaseKind::Version));
    assert!(get_version(s.c(), 9999).unwrap_err().is_not_found());
}

#[test]
fn versions_and_milestones_live_in_separate_lists() {
    let s = S::new();
    let v = ver(&s, "1.0");
    let m = mile(&s, "1.0");
    assert_eq!(list_versions(s.c(), s.p()).unwrap().len(), 1);
    assert_eq!(list_milestones(s.c(), s.p()).unwrap().len(), 1);
    assert_eq!(get_milestone(s.c(), m).unwrap().kind, ReleaseKind::Milestone);
    // Dieselbe Kennung kann in beiden Tabellen vorkommen; sie bleiben getrennt.
    assert_eq!(v, m);
    delete_version(s.c(), v, None).unwrap();
    assert!(get_version(s.c(), v).is_err());
    assert!(get_milestone(s.c(), m).is_ok());
}

#[test]
fn names_are_unique_per_project_and_kind() {
    let s = S::new();
    ver(&s, "Alpha");
    assert_eq!(create_version(s.c(), &NewRelease::new(s.p(), "ALPHA")), Err(SprintError::Plan(PlanError::NameTaken)));
    assert_eq!(create_version(s.c(), &NewRelease::new(s.p(), " ")), Err(SprintError::Plan(PlanError::NameEmpty)));
    assert!(matches!(create_version(s.c(), &NewRelease::new(s.p(), &"x".repeat(81))), Err(SprintError::Plan(PlanError::NameTooLong { .. }))));
    mile(&s, "Alpha"); // anderer Bereich
    let other = s.other("OTH");
    assert!(create_version(s.c(), &NewRelease::new(other, "Alpha")).is_ok(), "anderes Projekt");
}

#[test]
fn updates_change_name_description_and_due_date() {
    let s = S::new();
    let a = ver(&s, "1.0");
    ver(&s, "2.0");
    update_version(s.c(), a, &UpdateRelease { name: Some(" 1.1 ".into()), description: Some("  Erste Fassung ".into()), due_date: Some(Some(d(2026, 12, 1))), ..Default::default() }).unwrap();
    let v = get_version(s.c(), a).unwrap();
    assert_eq!((v.name.as_str(), v.description.as_str(), v.due_date), ("1.1", "Erste Fassung", Some(d(2026, 12, 1))));
    update_version(s.c(), a, &UpdateRelease { due_date: Some(None), ..Default::default() }).unwrap();
    assert_eq!(get_version(s.c(), a).unwrap().due_date, None);
    assert_eq!(update_version(s.c(), a, &UpdateRelease { name: Some("2.0".into()), ..Default::default() }), Err(SprintError::Plan(PlanError::NameTaken)));
    assert!(update_version(s.c(), a, &UpdateRelease { name: Some("1.1".into()), ..Default::default() }).is_ok(), "der eigene Name ist frei");
    assert!(update_milestone(s.c(), 9999, &UpdateRelease::default()).unwrap_err().is_not_found());
}

#[test]
fn descriptions_are_limited() {
    let s = S::new();
    let mut n = NewRelease::new(s.p(), "1.0");
    n.description = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
    assert_eq!(create_version(s.c(), &n), Err(SprintError::TextTooLong { max: MAX_DESCRIPTION_CHARS }));
    n.description = "x".repeat(MAX_DESCRIPTION_CHARS);
    assert!(create_version(s.c(), &n).is_ok());
}

#[test]
fn the_order_can_be_changed_but_must_list_every_entry() {
    let s = S::new();
    let (a, b, c) = (ver(&s, "A"), ver(&s, "B"), ver(&s, "C"));
    reorder_versions(s.c(), s.p(), &[c, a, b]).unwrap();
    assert_eq!(list_versions(s.c(), s.p()).unwrap().iter().map(|v| v.id).collect::<Vec<_>>(), vec![c, a, b]);
    assert_eq!(reorder_versions(s.c(), s.p(), &[a, b]), Err(SprintError::Plan(PlanError::ReorderMismatch)));
    assert_eq!(reorder_versions(s.c(), s.p(), &[a, a, b]), Err(SprintError::Plan(PlanError::ReorderMismatch)));
    let m = (mile(&s, "M1"), mile(&s, "M2"));
    reorder_milestones(s.c(), s.p(), &[m.1, m.0]).unwrap();
    assert_eq!(list_milestones(s.c(), s.p()).unwrap()[0].id, m.1);
    assert_eq!(list_versions(s.c(), s.p()).unwrap()[0].id, c, "die Versionen bleiben unberührt");
}

#[test]
fn released_and_reached_are_set_with_a_time() {
    let s = S::new();
    let v = ver(&s, "1.0");
    s.at("2026-12-01T10:00:00");
    set_version_released(s.c(), v, true).unwrap();
    let r = get_version(s.c(), v).unwrap();
    assert_eq!((r.done, r.done_at.as_deref()), (true, Some("2026-12-01T10:00:00")));
    s.at("2026-12-05T10:00:00");
    set_version_released(s.c(), v, true).unwrap();
    assert_eq!(get_version(s.c(), v).unwrap().done_at.as_deref(), Some("2026-12-01T10:00:00"), "unverändert lässt den Zeitpunkt stehen");
    set_version_released(s.c(), v, false).unwrap();
    let r = get_version(s.c(), v).unwrap();
    assert_eq!((r.done, r.done_at), (false, None));
    let m = mile(&s, "M");
    set_milestone_reached(s.c(), m, true).unwrap();
    assert!(get_milestone(s.c(), m).unwrap().done);
    assert!(!get_version(s.c(), v).unwrap().done);
}

#[test]
fn a_release_is_overdue_when_due_in_the_past_and_not_done() {
    let s = S::new();
    let v = ver(&s, "1.0");
    update_version(s.c(), v, &UpdateRelease { due_date: Some(Some(d(2026, 11, 1))), ..Default::default() }).unwrap();
    let r = get_version(s.c(), v).unwrap();
    assert!(r.is_overdue(d(2026, 11, 2)));
    assert!(!r.is_overdue(d(2026, 11, 1)), "am Fälligkeitstag noch nicht");
    set_version_released(s.c(), v, true).unwrap();
    assert!(!get_version(s.c(), v).unwrap().is_overdue(d(2026, 11, 30)));
    let none = ver(&s, "2.0");
    assert!(!get_version(s.c(), none).unwrap().is_overdue(d(2030, 1, 1)), "ohne Fälligkeit nie überfällig");
}

// ------------------------------------------------------------------ Fortschritt

#[test]
fn the_progress_counts_tickets_by_status_category() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let other = ver(&s, "2.0");
    let (a, b, c, e) = (s.task("a", Some(3.0)), s.task("b", Some(5.0)), s.task("c", Some(2.0)), s.task("e", Some(9.0)));
    for t in [&a, &b, &c] {
        assign(&s, t, &fv, v);
    }
    assign(&s, &e, &fv, other);
    s.status(&a, s.f.done, "2026-10-30T10:00:00");
    s.status(&b, s.f.progress, "2026-10-30T10:00:00");
    let p = version_progress(s.c(), v, Basis::Points).unwrap();
    assert_eq!((p.todo, p.in_progress, p.done, p.total()), (1, 1, 1, 3));
    assert_eq!((p.progress.done, p.progress.total), (3.0, 10.0));
    assert_eq!(p.progress.percent(), 30);
    let by_count = version_progress(s.c(), v, Basis::TicketCount).unwrap();
    assert_eq!(by_count.progress.percent(), 33);
    assert_eq!(version_progress(s.c(), other, Basis::Points).unwrap().total(), 1);
}

#[test]
fn the_progress_falls_back_to_one_per_ticket_without_points() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let (a, b) = (s.task("a", Some(3.0)), s.f.mk(s.f.defect, "ohne Punkte"));
    // Defects haben kein Versions-Feld in dieser Testumgebung: eigenes Feld anlegen.
    let fd = s.f.add_field(s.f.defect, "Fix version", FieldKind::Version, FieldOptions::default(), false);
    assign(&s, &a, &fv, v);
    set_field(s.c(), b.id, fd.id, json!(v), &Source::ui()).unwrap();
    s.status(&b, s.f.done, "2026-10-30T10:00:00");
    let p = version_progress(s.c(), v, Basis::PointsWithFallbackOne).unwrap();
    assert_eq!((p.progress.done, p.progress.total), (1.0, 4.0));
    assert_eq!(version_progress(s.c(), v, Basis::Points).unwrap().progress.total, 3.0);
}

#[test]
fn an_empty_release_has_no_progress_and_tickets_without_a_status_count_as_to_do() {
    let s = S::new();
    let (fv, fm) = fields(&s);
    let v = ver(&s, "1.0");
    let p = version_progress(s.c(), v, Basis::Points).unwrap();
    assert_eq!((p.total(), p.progress.fraction), (0, 0.0));
    let t = s.task("t", None);
    assign(&s, &t, &fv, v);
    s.c().execute("UPDATE tickets SET status_id = NULL WHERE id = ?1", [t.id]).unwrap();
    assert_eq!(version_progress(s.c(), v, Basis::TicketCount).unwrap().todo, 1);
    // Der Meilenstein mit derselben Kennung ist davon nicht betroffen.
    let m = mile(&s, "M");
    assert_eq!(m, v);
    assert_eq!(milestone_progress(s.c(), m, Basis::TicketCount).unwrap().total(), 0);
    let _ = fm;
}

#[test]
fn milestone_progress_reads_milestone_fields() {
    let s = S::new();
    let (_, fm) = fields(&s);
    let m = mile(&s, "M1");
    let (a, b) = (s.task("a", Some(1.0)), s.task("b", Some(1.0)));
    assign(&s, &a, &fm, m);
    assign(&s, &b, &fm, m);
    s.status(&a, s.f.done, "2026-10-30T10:00:00");
    let p = milestone_progress(s.c(), m, Basis::TicketCount).unwrap();
    assert_eq!((p.done, p.todo, p.progress.percent()), (1, 1, 50));
    assert_eq!(milestone_tickets(s.c(), m).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![a.id, b.id]);
}

#[test]
fn the_tickets_of_a_release_come_in_rank_order() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let (a, b, c) = (s.task("a", None), s.task("b", None), s.task("c", None));
    for t in [&c, &a] {
        assign(&s, t, &fv, v);
    }
    assert_eq!(version_tickets(s.c(), v).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![a.id, c.id]);
    let _ = b;
}

// ------------------------------------------------------------------ Löschen in Benutzung

#[test]
fn an_unused_release_can_be_deleted_without_a_mode() {
    let s = S::new();
    let v = ver(&s, "1.0");
    assert_eq!(version_usage(s.c(), v).unwrap(), 0);
    delete_version(s.c(), v, None).unwrap();
    assert!(get_version(s.c(), v).unwrap_err().is_not_found());
    assert!(delete_version(s.c(), v, None).unwrap_err().is_not_found());
}

#[test]
fn a_used_version_cannot_be_deleted_without_a_mode() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let (a, b) = (s.task("a", None), s.task("b", None));
    assign(&s, &a, &fv, v);
    assign(&s, &b, &fv, v);
    assert_eq!(version_usage(s.c(), v).unwrap(), 2);
    assert_eq!(integrity::usage_count_version(s.c(), v).unwrap(), 2);
    assert_eq!(delete_version(s.c(), v, None), Err(SprintError::Plan(PlanError::InUse { count: 2 })));
    assert!(get_version(s.c(), v).is_ok());
    assert!(SprintError::Plan(PlanError::InUse { count: 2 }).message(Lang::En).contains('2'));
}

#[test]
fn discarding_clears_the_fields_that_pointed_to_the_version() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let a = s.task("a", None);
    assign(&s, &a, &fv, v);
    delete_version(s.c(), v, Some(DeleteMode::Discard)).unwrap();
    assert_eq!(get_value(s.c(), a.id, &get_field(s.c(), fv.id).unwrap()).unwrap(), None);
    assert!(get_version(s.c(), v).is_err());
}

#[test]
fn migrating_moves_the_fields_to_the_replacement() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let (old, new) = (ver(&s, "1.0"), ver(&s, "1.1"));
    let a = s.task("a", None);
    assign(&s, &a, &fv, old);
    let before = get_ticket(s.c(), a.id).unwrap().updated_at;
    delete_version(s.c(), old, Some(DeleteMode::Migrate { to: new })).unwrap();
    assert_eq!(get_value(s.c(), a.id, &fv).unwrap(), Some(json!(new)));
    assert_eq!(version_usage(s.c(), new).unwrap(), 1);
    assert!(get_ticket(s.c(), a.id).unwrap().updated_at >= before);
}

#[test]
fn the_replacement_must_be_another_entry_of_the_same_kind_and_project() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let a = s.task("a", None);
    assign(&s, &a, &fv, v);
    let other = s.other("OTH");
    let foreign = create_version(s.c(), &NewRelease::new(other, "9.0")).unwrap();
    let m = mile(&s, "M"); // dieselbe Kennung wie die Version, aber ein Meilenstein
    let _ = m;
    for bad in [v, foreign, 9999] {
        assert_eq!(delete_version(s.c(), v, Some(DeleteMode::Migrate { to: bad })), Err(SprintError::Plan(PlanError::ReplacementInvalid)), "{bad}");
    }
    assert!(get_version(s.c(), v).is_ok());
    assert_eq!(get_value(s.c(), a.id, &fv).unwrap(), Some(json!(v)), "nichts geändert");
}

#[test]
fn a_milestone_is_counted_and_migrated_separately_from_versions() {
    let s = S::new();
    let (fv, fm) = fields(&s);
    let (m1, m2) = (mile(&s, "M1"), mile(&s, "M2"));
    let v = ver(&s, "V");
    let a = s.task("a", None);
    assign(&s, &a, &fm, m1);
    assign(&s, &a, &fv, v);
    assert_eq!(milestone_usage(s.c(), m1).unwrap(), 1);
    assert_eq!(integrity::usage_count_milestone(s.c(), m1).unwrap(), 1);
    assert_eq!(milestone_usage(s.c(), m2).unwrap(), 0);
    assert_eq!(delete_milestone(s.c(), m1, None), Err(SprintError::Plan(PlanError::InUse { count: 1 })));
    delete_milestone(s.c(), m1, Some(DeleteMode::Migrate { to: m2 })).unwrap();
    assert_eq!(get_value(s.c(), a.id, &fm).unwrap(), Some(json!(m2)));
    assert_eq!(get_value(s.c(), a.id, &fv).unwrap(), Some(json!(v)), "das Versionsfeld bleibt");
    delete_milestone(s.c(), m2, Some(DeleteMode::Discard)).unwrap();
    assert_eq!(get_value(s.c(), a.id, &fm).unwrap(), None);
    assert_eq!(get_value(s.c(), a.id, &fv).unwrap(), Some(json!(v)));
}

#[test]
fn a_ticket_with_two_fields_on_the_same_version_counts_once() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let second = s.f.add_field(s.f.task, "Found in", FieldKind::Version, FieldOptions::default(), false);
    let v = ver(&s, "1.0");
    let a = s.task("a", None);
    assign(&s, &a, &fv, v);
    assign(&s, &a, &second, v);
    assert_eq!(version_usage(s.c(), v).unwrap(), 1);
    let w = ver(&s, "2.0");
    delete_version(s.c(), v, Some(DeleteMode::Migrate { to: w })).unwrap();
    assert_eq!(get_value(s.c(), a.id, &fv).unwrap(), Some(json!(w)));
    assert_eq!(get_value(s.c(), a.id, &second).unwrap(), Some(json!(w)));
}

#[test]
fn deleting_the_project_removes_nothing_from_other_projects() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let a = s.task("a", None);
    assign(&s, &a, &fv, v);
    let other = s.other("OTH");
    create_version(s.c(), &NewRelease::new(other, "1.0")).unwrap();
    crate::planning::projects::delete_project(s.c(), other).unwrap();
    assert_eq!(version_usage(s.c(), v).unwrap(), 1);
}

// ------------------------------------------------------------------ Prüfung der Feldwerte

fn check(s: &S, f: &Field, project: i64, v: serde_json::Value) -> Result<Option<serde_json::Value>, TicketError> {
    normalize_value(s.c(), &RefCtx { project_id: project, ticket_id: None }, f, &v)
}

#[test]
fn a_version_field_accepts_only_versions_of_the_tickets_project() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    assert_eq!(check(&s, &fv, s.p(), json!(v)).unwrap(), Some(json!(v)));
    let other = s.other("OTH");
    let foreign = create_version(s.c(), &NewRelease::new(other, "9.0")).unwrap();
    assert!(matches!(check(&s, &fv, s.p(), json!(foreign)), Err(TicketError::RefInvalid(l)) if l.id == fv.id));
    assert!(matches!(check(&s, &fv, s.p(), json!(9999)), Err(TicketError::RefInvalid(_))), "unbekannte Kennung");
    assert!(matches!(check(&s, &fv, s.p(), json!(0)), Err(TicketError::ValueInvalid(_))));
    assert!(matches!(check(&s, &fv, s.p(), json!(-3)), Err(TicketError::ValueInvalid(_))));
    assert!(matches!(check(&s, &fv, s.p(), json!("1")), Err(TicketError::ValueInvalid(_))));
    assert_eq!(check(&s, &fv, other, json!(foreign)).unwrap(), Some(json!(foreign)), "im anderen Projekt ist sie gültig");
    assert_eq!(check(&s, &fv, s.p(), json!(null)).unwrap(), None, "leer bleibt erlaubt");
}

#[test]
fn a_version_field_does_not_accept_a_milestone() {
    let s = S::new();
    let (fv, fm) = fields(&s);
    let m = mile(&s, "M"); // Kennung 1, aber es gibt keine Version
    assert!(matches!(check(&s, &fv, s.p(), json!(m)), Err(TicketError::RefInvalid(_))));
    assert_eq!(check(&s, &fm, s.p(), json!(m)).unwrap(), Some(json!(m)));
    let v = ver(&s, "V");
    assert_eq!(check(&s, &fv, s.p(), json!(v)).unwrap(), Some(json!(v)));
    // Ein Meilenstein mit höherer Kennung ist im Versionsfeld nicht gültig.
    let m2 = mile(&s, "M2");
    assert!(matches!(check(&s, &fv, s.p(), json!(m2)), Err(TicketError::RefInvalid(_))));
}

#[test]
fn setting_a_version_through_the_ticket_api_is_checked() {
    let s = S::new();
    let (fv, fm) = fields(&s);
    let a = s.task("a", None);
    assert!(matches!(set_field(s.c(), a.id, fv.id, json!(5), &Source::ui()), Err(TicketError::RefInvalid(_))));
    let v = ver(&s, "1.0");
    set_field(s.c(), a.id, fv.id, json!(v), &Source::ui()).unwrap();
    assert_eq!(get_value(s.c(), a.id, &fv).unwrap(), Some(json!(v)));
    let other = s.other("OTH");
    let foreign = create_milestone(s.c(), &NewRelease::new(other, "M")).unwrap();
    assert!(set_field(s.c(), a.id, fm.id, json!(foreign), &Source::ui()).is_err());
    assert_eq!(get_value(s.c(), a.id, &fm).unwrap(), None);
}

#[test]
fn creating_a_ticket_with_a_foreign_version_fails_and_a_stale_default_is_ignored() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let other = s.other("OTH");
    let foreign = create_version(s.c(), &NewRelease::new(other, "9.0")).unwrap();
    let mut n = NewTicket::new(s.p(), s.f.task, "t");
    n.values = vec![(fv.id, json!(foreign))];
    assert!(matches!(create_ticket(s.c(), &n, &Source::ui()), Err(TicketError::RefInvalid(_))));
    // Ein Standardwert, der auf eine nicht mehr vorhandene Version zeigt, wird beim Anlegen übersprungen.
    s.c().execute("UPDATE fields SET default_value = '777' WHERE id = ?1", [fv.id]).unwrap();
    let (t, _) = create_ticket(s.c(), &NewTicket::new(s.p(), s.f.task, "u"), &Source::ui()).unwrap();
    assert_eq!(get_value(s.c(), t.id, &get_field(s.c(), fv.id).unwrap()).unwrap(), None);
}

#[test]
fn moving_a_ticket_to_another_project_drops_version_values_of_the_old_project() {
    let s = S::new();
    let (fv, _) = fields(&s);
    let v = ver(&s, "1.0");
    let a = s.task("a", None);
    assign(&s, &a, &fv, v);
    let other = s.f.other_project("OTH");
    let ty = crate::planning::types::list_types(s.c(), other).unwrap().into_iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
    let to_field = s.f.add_field(ty, "Fix version", FieldKind::Version, FieldOptions::default(), false);
    let mapping = [integrity::FieldMapEntry { from_field: fv.id, to_field: Some(to_field.id) }];
    crate::planning::clone_move::change_type(s.c(), a.id, ty, &mapping, &[], &Source::ui()).unwrap();
    assert_eq!(get_value(s.c(), a.id, &to_field).unwrap(), None, "die Version gehört dem alten Projekt");
    assert_eq!(version_usage(s.c(), v).unwrap(), 0);
}

// ------------------------------------------------------------------ Optionalität und Sprache

#[test]
fn everything_refuses_a_project_without_versions() {
    let s = S::new();
    let v = ver(&s, "1.0");
    let m = mile(&s, "M");
    update_project(s.c(), s.p(), &UpdateProject { use_versions: Some(false), ..Default::default() }).unwrap();
    let off = Err(SprintError::VersionsDisabled);
    assert_eq!(create_version(s.c(), &NewRelease::new(s.p(), "N")).map(|_| ()), off);
    assert_eq!(create_milestone(s.c(), &NewRelease::new(s.p(), "N")).map(|_| ()), off);
    assert_eq!(update_version(s.c(), v, &UpdateRelease::default()).map(|_| ()), off);
    assert_eq!(delete_version(s.c(), v, None).map(|_| ()), off);
    assert_eq!(delete_milestone(s.c(), m, None).map(|_| ()), off);
    assert_eq!(set_version_released(s.c(), v, true).map(|_| ()), off);
    assert_eq!(set_milestone_reached(s.c(), m, true).map(|_| ()), off);
    assert_eq!(reorder_versions(s.c(), s.p(), &[v]).map(|_| ()), off);
    assert_eq!(list_versions(s.c(), s.p()).map(|_| ()), off);
    assert_eq!(list_milestones(s.c(), s.p()).map(|_| ()), off);
    assert_eq!(version_progress(s.c(), v, Basis::Points).map(|_| ()), off);
    assert_eq!(milestone_tickets(s.c(), m).map(|_| ()), off);
    // Vorhandene Daten bleiben lesbar (Feldwerte lassen sich weiter anzeigen) und kehren zurück.
    assert_eq!(get_version(s.c(), v).unwrap().name, "1.0");
    update_project(s.c(), s.p(), &UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
    assert_eq!(list_versions(s.c(), s.p()).unwrap().len(), 1);
}

#[test]
fn sprints_and_versions_are_switched_independently() {
    let s = S::new();
    update_project(s.c(), s.p(), &UpdateProject { use_sprints: Some(false), ..Default::default() }).unwrap();
    assert!(create_version(s.c(), &NewRelease::new(s.p(), "1.0")).is_ok());
    update_project(s.c(), s.p(), &UpdateProject { use_sprints: Some(true), use_versions: Some(false), ..Default::default() }).unwrap();
    assert!(create_version(s.c(), &NewRelease::new(s.p(), "2.0")).is_err());
    assert!(s.c().prepare("SELECT 1").is_ok());
    assert!(super::sprint::create_sprint(s.c(), &super::sprint::NewSprint::new(s.p(), "S"), &s.cal).is_ok());
}

#[test]
fn labels_exist_in_both_languages() {
    for k in [ReleaseKind::Version, ReleaseKind::Milestone] {
        for done in [true, false] {
            assert_ne!(k.done_label(done, Lang::De), k.done_label(done, Lang::En), "{k:?} {done}");
        }
    }
    assert_eq!(ReleaseKind::Version.field_kind(), "version");
    assert_eq!(ReleaseKind::Milestone.field_kind(), "milestone");
    assert_ne!(ReleaseKind::Milestone.label(Lang::De), ReleaseKind::Milestone.label(Lang::En));
}
