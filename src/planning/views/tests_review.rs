//! Reproduktionstests der Fehlersuche in Boards/Dashboards/Konfiguration: Jeder Test beschreibt einen Fehler, der vor dem Fix auftrat.

use super::boards::{create_board, get_board, update_board, BoardColumn, BoardConfig, NewBoard, UpdateBoard};
use super::testutil::{db, project};
use crate::planning::fields::{delete_field, list_fields};
use crate::planning::integrity::DeleteMode;
use crate::planning::statuses::{delete_status, list_statuses};
use crate::planning::types::{delete_type, list_types};
use crate::planning::fields::{create_field, get_field, update_field, FieldKind, FieldOptions, NewField, OptionValue, UpdateField};
use crate::planning::workflow::{get_transition, set_transition, workflow_statuses, Condition, ConditionOp, SetValue, TransitionDef};
use rusqlite::Connection;
use serde_json::json;

fn board(c: &Connection, p: i64, cfg: BoardConfig) -> i64 {
    create_board(c, &NewBoard { project_id: p, name: "B".into(), description: String::new(), config: cfg }).unwrap()
}

#[test]
fn deleting_a_status_removes_it_from_the_board_columns() {
    let c = db();
    let p = project(&c, "AA", true);
    let s: Vec<i64> = list_statuses(&c, p).unwrap().iter().map(|s| s.id).collect();
    let cols = vec![BoardColumn { title: String::new(), status_ids: vec![s[0]] }, BoardColumn { title: String::new(), status_ids: vec![s[1], s[2]] }];
    let id = board(&c, p, BoardConfig { columns: cols, ..Default::default() });
    delete_status(&c, s[1], Some(DeleteMode::Discard)).unwrap();
    let cfg = get_board(&c, id).unwrap().config;
    assert_eq!(cfg.status_ids(), vec![s[0], s[2]], "der gelöschte Status steht nicht mehr im Board");
    // das Board bleibt bearbeitbar (sonst scheitert jedes Speichern an einem unbekannten Status)
    update_board(&c, id, &UpdateBoard { config: Some(cfg), ..Default::default() }).unwrap();
}

#[test]
fn deleting_the_last_status_of_a_column_drops_the_column() {
    let c = db();
    let p = project(&c, "AA", true);
    let s: Vec<i64> = list_statuses(&c, p).unwrap().iter().map(|s| s.id).collect();
    let id = board(&c, p, BoardConfig { columns: vec![BoardColumn { title: String::new(), status_ids: vec![s[0]] }, BoardColumn { title: String::new(), status_ids: vec![s[1]] }], ..Default::default() });
    delete_status(&c, s[1], Some(DeleteMode::Discard)).unwrap();
    assert_eq!(get_board(&c, id).unwrap().config.columns.len(), 1);
}

#[test]
fn deleting_a_type_removes_it_from_the_board_type_filter() {
    let c = db();
    let p = project(&c, "AA", true);
    let types = list_types(&c, p).unwrap();
    let (a, b) = (types[0].id, types[1].id);
    let id = board(&c, p, BoardConfig { type_filter: vec![a, b], ..Default::default() });
    delete_type(&c, a, None).unwrap();
    assert_eq!(get_board(&c, id).unwrap().config.type_filter, vec![b]);
    // war es der einzige Typ im Filter, zeigt das Board danach wieder alle Typen (statt nichts)
    delete_type(&c, b, None).unwrap();
    assert!(get_board(&c, id).unwrap().config.type_filter.is_empty());
}

#[test]
fn deleting_a_field_removes_it_from_the_board_card_fields() {
    let c = db();
    let p = project(&c, "AA", true);
    let ty = list_types(&c, p).unwrap().into_iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
    let fields = list_fields(&c, ty).unwrap();
    let (f1, f2) = (fields[0].id, fields[1].id);
    let id = board(&c, p, BoardConfig { card_fields: vec![f1, f2], ..Default::default() });
    delete_field(&c, f1, Some(DeleteMode::Discard)).unwrap();
    assert_eq!(get_board(&c, id).unwrap().config.card_fields, vec![f2]);
}

#[test]
fn removing_a_dropdown_option_removes_it_from_transition_rules() {
    let c = db();
    let p = project(&c, "AA", true);
    let ty = list_types(&c, p).unwrap().into_iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
    let mut n = NewField::new(ty, "Severity", FieldKind::Dropdown);
    n.options = FieldOptions { values: vec![OptionValue::new("Low"), OptionValue::new("High")], ..Default::default() };
    let fid = create_field(&c, &n).unwrap();
    let f = get_field(&c, fid).unwrap();
    let (low, high) = (f.options.values[0].id, f.options.values[1].id);
    let wf = workflow_statuses(&c, ty).unwrap();
    let def = TransitionDef {
        from_status_id: wf[0],
        to_status_id: wf[1],
        required_fields: vec![fid],
        conditions: vec![Condition { field_id: fid, op: ConditionOp::Eq, value: json!(high), allow: true }, Condition { field_id: fid, op: ConditionOp::In, value: json!([low, high]), allow: false }],
        set_values: vec![SetValue { field_id: fid, value: json!(high) }],
    };
    set_transition(&c, ty, &def).unwrap();
    // „High“ aus der Liste nehmen (kein Ticket benutzt es)
    let opts = FieldOptions { values: vec![f.options.values[0].clone()], ..Default::default() };
    update_field(&c, fid, &UpdateField { options: Some(opts), ..Default::default() }).unwrap();
    let t = get_transition(&c, ty, wf[0], wf[1]).unwrap().unwrap().def;
    assert_eq!(t.required_fields, vec![fid], "das Pflichtfeld bleibt");
    assert!(t.set_values.is_empty(), "Setzen einer gelöschten Option entfällt: {:?}", t.set_values);
    assert_eq!(t.conditions.len(), 1, "die Bedingung auf die gelöschte Option entfällt, aus der Liste verschwindet sie: {:?}", t.conditions);
    assert_eq!(t.conditions[0].value, json!([low]));
}

#[test]
fn deleting_a_version_cleans_the_transition_rules_that_point_at_it() {
    use crate::planning::history::Source;
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::releases::{create_version, delete_version, NewRelease};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::transition::{available_transitions, transition_ticket};
    let f = Fx::new();
    update_project(&f.c, f.project, &UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
    let fid = create_field(&f.c, &NewField::new(f.task, "Fix version", FieldKind::Version)).unwrap();
    let (v1, v2, v3) = (
        create_version(&f.c, &NewRelease::new(f.project, "1.0")).unwrap(),
        create_version(&f.c, &NewRelease::new(f.project, "2.0")).unwrap(),
        create_version(&f.c, &NewRelease::new(f.project, "3.0")).unwrap(),
    );
    // „Beim Erledigen die Version setzen; nicht erlaubt, solange schon die Version 1.0 gesetzt ist“
    let def = TransitionDef {
        from_status_id: f.progress,
        to_status_id: f.done,
        conditions: vec![Condition { field_id: fid, op: ConditionOp::Eq, value: json!(v1), allow: false }],
        set_values: vec![SetValue { field_id: fid, value: json!(v1) }],
        ..Default::default()
    };
    set_transition(&f.c, f.task, &def).unwrap();
    let t = f.mk(f.task, "T");
    transition_ticket(&f.c, t.id, f.progress, &[], &Source::ui()).unwrap();
    delete_version(&f.c, v1, None).unwrap();
    let rules = get_transition(&f.c, f.task, f.progress, f.done).unwrap().unwrap().def;
    assert!(rules.set_values.is_empty() && rules.conditions.is_empty(), "{rules:?}");
    // sonst scheitert die Liste der möglichen Ziele (und damit jedes Ziehen auf dem Board) an der gelöschten Version
    let targets = available_transitions(&f.c, t.id).unwrap();
    assert!(targets.iter().any(|o| o.status_id == f.done && !o.blocked));
    // mit Ersatz zeigen die Regeln auf den Ersatz
    set_transition(&f.c, f.task, &TransitionDef { set_values: vec![SetValue { field_id: fid, value: json!(v2) }], ..def }).unwrap();
    delete_version(&f.c, v2, Some(DeleteMode::Migrate { to: v3 })).unwrap();
    let rules = get_transition(&f.c, f.task, f.progress, f.done).unwrap().unwrap().def;
    assert_eq!(rules.set_values, vec![SetValue { field_id: fid, value: json!(v3) }]);
}

#[test]
fn a_template_export_leaves_out_rules_with_project_specific_references() {
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::releases::{create_version, NewRelease};
    use crate::planning::template::export_types;
    use crate::planning::tickets::fixture::Fx;
    let f = Fx::new();
    update_project(&f.c, f.project, &UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
    let vf = create_field(&f.c, &NewField::new(f.task, "Fix version", FieldKind::Version)).unwrap();
    let v = create_version(&f.c, &NewRelease::new(f.project, "1.0")).unwrap();
    let def = TransitionDef {
        from_status_id: f.progress,
        to_status_id: f.done,
        conditions: vec![Condition { field_id: vf, op: ConditionOp::Eq, value: json!(v), allow: false }, Condition { field_id: vf, op: ConditionOp::IsNotEmpty, value: json!(null), allow: false }],
        set_values: vec![SetValue { field_id: vf, value: json!(v) }],
        ..Default::default()
    };
    set_transition(&f.c, f.task, &def).unwrap();
    let doc = export_types(&f.c, f.project).unwrap();
    let task = doc.types.iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap();
    let tr = task.transitions.iter().find(|x| x.conditions.len() + x.set_values.len() > 0).expect("die Regel mit dem leeren-Vergleich bleibt");
    assert!(tr.set_values.is_empty(), "eine Versions-Kennung ist in anderen Projekten sinnlos: {:?}", tr.set_values);
    assert_eq!(tr.conditions.len(), 1, "nur der Vergleich ohne Kennung bleibt: {:?}", tr.conditions);
    assert_eq!(tr.conditions[0].op, ConditionOp::IsNotEmpty);
}
