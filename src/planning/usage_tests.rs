//! Tests der Integritätsregel mit simulierter Benutzung: Die Ticket-Tabellen gibt es noch nicht, deshalb meldet eine
//! Test-Tabelle `fake` je Bereich Verwendungen (über `integrity::test_hook`). So lässt sich prüfen, dass `InUse`
//! geliefert wird, wenn etwas benutzt ist, und dass `Migrate`/`Discard` es freigeben.

use super::fields::*;
use super::integrity::{test_hook, DeleteMode, FieldMapEntry, TypeDeleteMode, TypeMigration};
use super::link_types::{create_link_type, delete_link_type, link_type_by_key, NewLinkType, TargetKind};
use super::projects::{create_project, delete_impact, delete_project, NewProject};
use super::statuses::*;
use super::testutil::db;
use super::types::*;
use super::work_kinds::{create_work_kind, delete_work_kind};
use super::workflow::*;
use super::PlanError;
use rusqlite::Connection;

fn fake_db() -> Connection {
    let c = db();
    c.execute_batch("CREATE TABLE fake (kind TEXT, a INTEGER, b INTEGER, n INTEGER);").unwrap();
    for k in ["status", "type", "type_workflow", "field", "link_type", "work_kind", "project"] {
        test_hook::add(k, &format!("SELECT COALESCE(SUM(n), 0) FROM fake WHERE kind = '{k}' AND a = ?1"));
    }
    for k in ["type_status", "parent_rule", "option"] {
        test_hook::add(k, &format!("SELECT COALESCE(SUM(n), 0) FROM fake WHERE kind = '{k}' AND a = ?1 AND b = ?2"));
    }
    c
}

fn use_it(c: &Connection, kind: &str, a: i64, b: i64, n: i64) {
    c.execute("INSERT INTO fake (kind, a, b, n) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![kind, a, b, n]).unwrap();
}

fn proj(c: &Connection, key: &str) -> i64 {
    create_project(c, &NewProject::new(key, "P", "#112233")).unwrap()
}

fn status(c: &Connection, p: i64, name: &str, cat: StatusCategory) -> i64 {
    create_status(c, &NewStatus { project_id: p, key: None, name: Some(name.into()), color: "#445566".into(), category: cat }).unwrap()
}

fn ty(c: &Connection, p: i64, name: &str) -> i64 {
    create_type(c, &NewType { project_id: p, key: None, name: Some(name.into()), icon: None, color: "#445566".into(), workflow: true }).unwrap()
}

#[test]
fn used_status_needs_a_mode() {
    let c = fake_db();
    let p = proj(&c, "AA");
    let a = status(&c, p, "A", StatusCategory::Todo);
    let b = status(&c, p, "B", StatusCategory::Done);
    use_it(&c, "status", a, 0, 3);
    assert_eq!(super::statuses::usage(&c, a).unwrap(), 3);
    assert_eq!(delete_status(&c, a, None), Err(PlanError::InUse { count: 3 }));
    assert!(get_status(&c, a).is_ok(), "nichts gelöscht");
    assert_eq!(delete_status(&c, a, Some(DeleteMode::Migrate { to: a })), Err(PlanError::ReplacementInvalid));
    delete_status(&c, a, Some(DeleteMode::Migrate { to: b })).unwrap();
    assert!(get_status(&c, a).is_err());
    let d = status(&c, p, "D", StatusCategory::Todo);
    use_it(&c, "status", d, 0, 1);
    assert_eq!(delete_status(&c, d, None), Err(PlanError::InUse { count: 1 }));
    delete_status(&c, d, Some(DeleteMode::Discard)).unwrap();
    assert!(get_status(&c, d).is_err());
}

#[test]
fn used_type_needs_a_mode_and_a_field_contract_for_migration() {
    let c = fake_db();
    let p = proj(&c, "AA");
    let a = ty(&c, p, "A");
    let b = ty(&c, p, "B");
    let fa = create_field(&c, &NewField::new(a, "Text", FieldKind::Text)).unwrap();
    let fb = create_field(&c, &NewField::new(b, "Text", FieldKind::Text)).unwrap();
    use_it(&c, "type", a, 0, 5);
    assert_eq!(delete_type(&c, a, None), Err(PlanError::InUse { count: 5 }));
    assert!(get_type(&c, a).is_ok());
    // Migration mit Zuordnung ungültig: Zielfeld gehört nicht zum Ersatz-Typ
    let bad = TypeMigration { to_type: b, fields: vec![FieldMapEntry { from_field: fa, to_field: Some(fa) }] };
    assert_eq!(delete_type(&c, a, Some(TypeDeleteMode::Migrate(bad))), Err(PlanError::ReplacementInvalid));
    let good = TypeMigration { to_type: b, fields: vec![FieldMapEntry { from_field: fa, to_field: Some(fb) }] };
    delete_type(&c, a, Some(TypeDeleteMode::Migrate(good))).unwrap();
    assert!(get_type(&c, a).is_err());
    let d = ty(&c, p, "D");
    use_it(&c, "type", d, 0, 2);
    delete_type(&c, d, Some(TypeDeleteMode::Discard)).unwrap();
}

#[test]
fn used_field_link_type_work_kind_and_project() {
    let c = fake_db();
    let p = proj(&c, "AA");
    let t = ty(&c, p, "T");
    let f1 = create_field(&c, &NewField::new(t, "A", FieldKind::Text)).unwrap();
    let f2 = create_field(&c, &NewField::new(t, "B", FieldKind::Text)).unwrap();
    use_it(&c, "field", f1, 0, 4);
    assert_eq!(delete_field(&c, f1, None), Err(PlanError::InUse { count: 4 }));
    delete_field(&c, f1, Some(DeleteMode::Migrate { to: f2 })).unwrap();
    use_it(&c, "field", f2, 0, 1);
    delete_field(&c, f2, Some(DeleteMode::Discard)).unwrap();
    // Verknüpfungstyp
    let lt = create_link_type(&c, &NewLinkType { name_out: "klont".into(), name_in: Some("ist Klon".into()), symmetric: false, target_kind: TargetKind::Ticket }).unwrap();
    use_it(&c, "link_type", lt, 0, 2);
    assert_eq!(delete_link_type(&c, lt, None), Err(PlanError::InUse { count: 2 }));
    let blocks = link_type_by_key(&c, "blocks").unwrap().id;
    delete_link_type(&c, lt, Some(DeleteMode::Migrate { to: blocks })).unwrap();
    use_it(&c, "link_type", blocks, 0, 7);
    assert_eq!(delete_link_type(&c, blocks, None), Err(PlanError::InUse { count: 7 }));
    delete_link_type(&c, blocks, Some(DeleteMode::Discard)).unwrap();
    // terminal bleibt auch mit Verwendungen und Modus geschützt
    let term = link_type_by_key(&c, "terminal").unwrap().id;
    use_it(&c, "link_type", term, 0, 9);
    assert_eq!(delete_link_type(&c, term, Some(DeleteMode::Discard)), Err(PlanError::SystemProtected));
    // Art der Arbeit
    let w1 = create_work_kind(&c, p, None, Some("A")).unwrap();
    let w2 = create_work_kind(&c, p, None, Some("B")).unwrap();
    use_it(&c, "work_kind", w1, 0, 1);
    assert_eq!(delete_work_kind(&c, w1, None), Err(PlanError::InUse { count: 1 }));
    delete_work_kind(&c, w1, Some(DeleteMode::Migrate { to: w2 })).unwrap();
    // Projekt: der Zähler meldet, wie viele Tickets mitgelöscht werden
    use_it(&c, "project", p, 0, 12);
    assert_eq!(delete_impact(&c, p).unwrap(), 12);
    assert_eq!(delete_project(&c, p).unwrap(), 12);
}

#[test]
fn workflow_changes_respect_usage() {
    let c = fake_db();
    let p = proj(&c, "AA");
    let a = status(&c, p, "A", StatusCategory::Todo);
    let b = status(&c, p, "B", StatusCategory::Done);
    let t = ty(&c, p, "T");
    set_workflow_statuses(&c, t, &[a, b]).unwrap();
    // Status aus dem Workflow nehmen, an dem Tickets hängen
    use_it(&c, "type_status", t, b, 2);
    assert_eq!(set_workflow_statuses(&c, t, &[a]), Err(PlanError::InUse { count: 2 }));
    assert_eq!(workflow_statuses(&c, t).unwrap(), vec![a, b]);
    set_workflow_statuses(&c, t, &[a, b]).unwrap();
    // Workflow abschalten, obwohl Tickets einen Status haben
    use_it(&c, "type_workflow", t, 0, 6);
    assert_eq!(set_workflow(&c, t, false, false), Err(PlanError::InUse { count: 6 }));
    assert!(get_type(&c, t).unwrap().workflow);
    set_workflow(&c, t, false, true).unwrap();
    assert!(!get_type(&c, t).unwrap().workflow);
    assert!(workflow_statuses(&c, t).unwrap().is_empty());
}

#[test]
fn hierarchy_and_dropdown_values_respect_usage() {
    let c = fake_db();
    let p = proj(&c, "AA");
    let epic = ty(&c, p, "Epic");
    let story = ty(&c, p, "Story");
    set_parents(&c, story, &[epic]).unwrap();
    use_it(&c, "parent_rule", story, epic, 3);
    assert_eq!(set_parents(&c, story, &[]), Err(PlanError::InUse { count: 3 }));
    assert_eq!(parents_of(&c, story).unwrap(), vec![epic]);
    set_parents(&c, story, &[epic]).unwrap(); // unverändert ist erlaubt
    // Dropdown-Wert, der benutzt wird
    let mut n = NewField::new(story, "D", FieldKind::Dropdown);
    n.options.values = vec![OptionValue::new("a"), OptionValue::new("b")];
    let f = create_field(&c, &n).unwrap();
    use_it(&c, "option", f, 1, 2);
    let mut o = get_field(&c, f).unwrap().options;
    o.values.remove(0);
    assert_eq!(update_field(&c, f, &UpdateField { options: Some(o.clone()), ..Default::default() }), Err(PlanError::InUse { count: 2 }));
    assert_eq!(get_field(&c, f).unwrap().options.values.len(), 2);
    // Der unbenutzte Wert („b“, Kennung 2) darf entfernt werden
    let mut o2 = get_field(&c, f).unwrap().options;
    o2.values.remove(1);
    update_field(&c, f, &UpdateField { options: Some(o2), ..Default::default() }).unwrap();
    assert_eq!(get_field(&c, f).unwrap().options.values.len(), 1);
}
