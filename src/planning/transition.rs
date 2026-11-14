//! Statuswechsel eines Tickets mit allen Regeln des Workflows – **eine** Funktion für Oberfläche, Quick-Menü und Agenten.
//!
//! Reihenfolge der Prüfung in [`transition_ticket`]:
//! 1. Der Typ hat einen Workflow und der Zielstatus gehört dazu (sonst `NoWorkflow` / `StatusNotInWorkflow`).
//! 2. Das Ziel ist laut Übergangsmatrix erlaubt (ohne definierte Übergänge frei; sonst `TransitionNotAllowed`).
//! 3. Die Bedingungen des Übergangs ([`evaluate_conditions`]) erlauben ihn – geprüft gegen die aktuellen Werte, überlagert
//!    mit den mitgegebenen Werten (`ConditionsNotMet`).
//! 4. Die „Setze-Werte“ des Übergangs werden angewendet (sie überschreiben mitgegebene Werte).
//! 5. Die Pflichtfelder des Übergangs müssen danach gefüllt sein; fehlen welche, liefert die Funktion
//!    `TransitionNeedsFields` mit der Liste – die Oberfläche öffnet damit einen Dialog und ruft die Funktion mit den Werten
//!    erneut auf (die Werte lassen sich also in derselben Operation mitgeben).
//!
//! Status und Werte werden atomar geschrieben, der Verlauf bekommt eine gemeinsame `batch`-Nummer.

use super::fields::list_fields;
use super::history::{self, Source};
use super::ops::TicketOp;
use super::ticket_error::{atomic, FieldLabel, TRes, TicketError};
use super::tickets::{get_ticket, raw_apply_fields, raw_set_status, Ticket};
use super::values::{self, is_empty, normalize_value, RefCtx};
use super::workflow::{allowed_targets, evaluate_conditions, get_transition, is_transition_allowed, workflow_statuses, Transition};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;

/// Was ein Statuswechsel schreiben würde (nach allen Prüfungen).
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionPlan {
    pub from: Option<i64>,
    pub to: i64,
    /// Regeln des Übergangs, falls einer definiert ist.
    pub transition: Option<Transition>,
    /// Tatsächliche Wertänderungen (mitgegebene Werte und Setze-Werte), `None` = Feld wird geleert.
    pub changes: Vec<(i64, Option<Value>)>,
}

/// Ein möglicher Zielstatus für die Oberfläche (Quick-Menü, Board-Ziehen).
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionOption {
    pub status_id: i64,
    /// Eine Bedingung sperrt den Wechsel gerade.
    pub blocked: bool,
    /// Pflichtfelder, die dafür noch zu füllen sind (Dialog).
    pub needs_fields: Vec<FieldLabel>,
}

/// Prüft den Wechsel und liefert, was er schreiben würde – ohne etwas zu ändern.
pub fn plan_transition(conn: &Connection, t: &Ticket, to: i64, provided: &[(i64, Value)]) -> TRes<TransitionPlan> {
    let ty = super::types::get_type(conn, t.type_id)?;
    if !ty.workflow {
        return Err(TicketError::NoWorkflow);
    }
    if !workflow_statuses(conn, ty.id)?.contains(&to) {
        return Err(TicketError::StatusNotInWorkflow);
    }
    // Ohne (gültigen) aktuellen Status gibt es keinen Übergang, der Regeln hätte: der Wechsel ist frei.
    let from = t.status_id.filter(|s| workflow_statuses(conn, ty.id).map(|w| w.contains(s)).unwrap_or(false));
    if let Some(f) = from {
        if !is_transition_allowed(conn, ty.id, f, to)? {
            return Err(TicketError::TransitionNotAllowed);
        }
    }
    let transition = match from {
        Some(f) => get_transition(conn, ty.id, f, to)?,
        None => None,
    };
    let fields = list_fields(conn, ty.id)?;
    let ctx = RefCtx { project_id: t.project_id, ticket_id: Some(t.id) };
    let current = values::load_values(conn, t.id)?;
    let mut effective: BTreeMap<i64, Value> = current.clone();
    let mut touched: Vec<i64> = Vec::new();
    for (fid, v) in provided {
        let f = fields.iter().find(|f| f.id == *fid).ok_or(TicketError::FieldNotInType { field_id: *fid })?;
        match normalize_value(conn, &ctx, f, v)? {
            Some(n) => {
                effective.insert(*fid, n);
            }
            None => {
                effective.remove(fid);
            }
        }
        touched.push(*fid);
    }
    if let Some(tr) = &transition {
        if !evaluate_conditions(&tr.def.conditions, &|id| effective.get(&id).cloned()) {
            return Err(TicketError::ConditionsNotMet);
        }
        for sv in &tr.def.set_values {
            let Some(f) = fields.iter().find(|f| f.id == sv.field_id) else { continue };
            match normalize_value(conn, &ctx, f, &sv.value)? {
                Some(n) => {
                    effective.insert(f.id, n);
                }
                None => {
                    effective.remove(&f.id);
                }
            }
            touched.push(f.id);
        }
        let missing: Vec<FieldLabel> = tr
            .def
            .required_fields
            .iter()
            .filter(|fid| is_empty(effective.get(fid)))
            .filter_map(|fid| fields.iter().find(|f| f.id == *fid))
            .map(FieldLabel::of)
            .collect();
        if !missing.is_empty() {
            return Err(TicketError::TransitionNeedsFields(missing));
        }
    }
    let mut changes: Vec<(i64, Option<Value>)> = Vec::new();
    for fid in touched {
        if changes.iter().any(|(i, _)| *i == fid) {
            continue;
        }
        let (old, new) = (current.get(&fid), effective.get(&fid));
        if old != new {
            changes.push((fid, new.cloned()));
        }
    }
    Ok(TransitionPlan { from, to, transition, changes })
}

/// Wechselt den Status des Tickets nach `to` und wendet die Regeln des Übergangs an (siehe Moduldoku). `values` sind
/// zusätzliche Feldwerte, die in derselben Operation gesetzt werden (z. B. für Pflichtfelder des Übergangs). Gleicher
/// Status = `Ok(None)`.
pub fn transition_ticket(conn: &Connection, ticket_id: i64, to: i64, values: &[(i64, Value)], source: &Source) -> TRes<Option<TicketOp>> {
    atomic(conn, || {
        let t = get_ticket(conn, ticket_id)?;
        if t.status_id == Some(to) {
            return Ok(None);
        }
        let plan = plan_transition(conn, &t, to, values)?;
        let batch = history::next_batch(conn)?;
        let mut ops = Vec::new();
        if let Some(before) = raw_set_status(conn, ticket_id, Some(to), source, batch)? {
            ops.push(TicketOp::Status { ticket: ticket_id, before, after: Some(to), source: source.clone() });
        }
        let done = raw_apply_fields(conn, ticket_id, &plan.changes, source, batch)?;
        if !done.is_empty() {
            ops.push(TicketOp::Fields { ticket: ticket_id, changes: done, source: source.clone() });
        }
        Ok(TicketOp::combine(ops))
    })
}

/// Alle Status, in die das Ticket laut Matrix wechseln darf, mit dem Hinweis, ob eine Bedingung sie gerade sperrt oder noch
/// Pflichtfelder fehlen (in Workflow-Reihenfolge). Ein Typ ohne Workflow hat keine.
pub fn available_transitions(conn: &Connection, ticket_id: i64) -> TRes<Vec<TransitionOption>> {
    let t = get_ticket(conn, ticket_id)?;
    let ty = super::types::get_type(conn, t.type_id)?;
    if !ty.workflow {
        return Ok(Vec::new());
    }
    let targets = match t.status_id {
        Some(s) => allowed_targets(conn, ty.id, s)?,
        None => workflow_statuses(conn, ty.id)?,
    };
    let mut out = Vec::new();
    for to in targets {
        match plan_transition(conn, &t, to, &[]) {
            Ok(_) => out.push(TransitionOption { status_id: to, blocked: false, needs_fields: Vec::new() }),
            Err(TicketError::ConditionsNotMet) => out.push(TransitionOption { status_id: to, blocked: true, needs_fields: Vec::new() }),
            Err(TicketError::TransitionNeedsFields(v)) => out.push(TransitionOption { status_id: to, blocked: false, needs_fields: v }),
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::FieldKind;
    use crate::planning::history::{history_of, HistoryField, SourceKind};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::set_field;
    use crate::planning::workflow::{replace_transitions, set_transition, set_workflow, set_workflow_statuses, Condition, ConditionOp, SetValue, TransitionDef};
    use crate::state::Lang;
    use serde_json::json;

    fn ui() -> Source {
        Source::ui()
    }

    #[test]
    fn plain_transition_changes_status_and_logs() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let op = transition_ticket(&f.c, t.id, f.progress, &[], &Source::agent("S")).unwrap();
        assert!(op.is_some());
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.progress));
        let h = history_of(&f.c, t.id).unwrap();
        let e = h.iter().find(|e| e.field == HistoryField::Status).unwrap();
        assert_eq!((e.old.clone(), e.new.clone()), (Some(f.todo.to_string()), Some(f.progress.to_string())));
        assert_eq!(e.source.kind, SourceKind::Agent);
        assert!(transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap().is_none(), "gleicher Status");
    }

    #[test]
    fn matrix_forbids_missing_transitions() {
        let f = Fx::new();
        // Standard: progress -> done erlaubt, done -> todo nicht
        let t = f.mk(f.task, "T");
        transition_ticket(&f.c, t.id, f.done, &[], &ui()).unwrap();
        assert_eq!(transition_ticket(&f.c, t.id, f.todo, &[], &ui()).unwrap_err(), TicketError::TransitionNotAllowed);
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.done));
        // ohne Matrix ist alles frei
        replace_transitions(&f.c, f.task, &[]).unwrap();
        transition_ticket(&f.c, t.id, f.todo, &[], &ui()).unwrap();
        // Status nicht im Workflow / Typ ohne Workflow
        let extra = crate::planning::statuses::create_status(
            &f.c,
            &crate::planning::statuses::NewStatus { project_id: f.project, key: None, name: Some("Review".into()), color: "#445566".into(), category: crate::planning::statuses::StatusCategory::Progress },
        )
        .unwrap();
        assert_eq!(transition_ticket(&f.c, t.id, extra, &[], &ui()).unwrap_err(), TicketError::StatusNotInWorkflow);
        let plain = crate::planning::types::create_type(
            &f.c,
            &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Notiz".into()), icon: None, color: "#445566".into(), workflow: false },
        )
        .unwrap();
        let n = f.mk(plain, "N");
        assert_eq!(transition_ticket(&f.c, n.id, f.done, &[], &ui()).unwrap_err(), TicketError::NoWorkflow);
        assert!(transition_ticket(&f.c, 9999, f.done, &[], &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn required_fields_need_values_that_can_be_passed_in_the_same_call() {
        let f = Fx::new();
        let reason = f.add_field(f.task, "Lösung", FieldKind::Text, Default::default(), false);
        set_transition(&f.c, f.task, &TransitionDef { from_status_id: f.progress, to_status_id: f.done, required_fields: vec![reason.id], ..Default::default() }).unwrap();
        let t = f.mk(f.task, "T");
        transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
        let err = transition_ticket(&f.c, t.id, f.done, &[], &ui()).unwrap_err();
        match &err {
            TicketError::TransitionNeedsFields(v) => assert_eq!(v.iter().map(|l| l.id).collect::<Vec<_>>(), vec![reason.id]),
            other => panic!("{other:?}"),
        }
        assert!(err.message(Lang::De).contains("Lösung"));
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.progress), "nichts geschrieben");
        // leerer Wert zählt nicht
        assert!(matches!(transition_ticket(&f.c, t.id, f.done, &[(reason.id, json!("  "))], &ui()), Err(TicketError::TransitionNeedsFields(_))));
        // Wert mitgeben
        let op = transition_ticket(&f.c, t.id, f.done, &[(reason.id, json!("behoben"))], &ui()).unwrap().unwrap();
        assert!(matches!(op, TicketOp::Batch(_)));
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.done));
        assert_eq!(values::get_value(&f.c, t.id, &reason).unwrap(), Some(json!("behoben")));
        let h = history_of(&f.c, t.id).unwrap();
        let st = h.iter().filter(|e| e.field == HistoryField::Status).last().unwrap();
        let fv = h.iter().find(|e| e.field == HistoryField::Custom(reason.id)).unwrap();
        assert_eq!(st.batch, fv.batch, "gleiche Aktion");
        // schon gefüllt: kein Dialog nötig
        let t2 = f.mk(f.task, "T2");
        transition_ticket(&f.c, t2.id, f.progress, &[], &ui()).unwrap();
        set_field(&f.c, t2.id, reason.id, json!("x"), &ui()).unwrap();
        transition_ticket(&f.c, t2.id, f.done, &[], &ui()).unwrap();
        // Wert für ein fremdes Feld / ungültiger Wert
        let t3 = f.mk(f.task, "T3");
        transition_ticket(&f.c, t3.id, f.progress, &[], &ui()).unwrap();
        let steps = f.field(f.defect, "steps");
        assert_eq!(transition_ticket(&f.c, t3.id, f.done, &[(steps.id, json!("x"))], &ui()).unwrap_err(), TicketError::FieldNotInType { field_id: steps.id });
    }

    #[test]
    fn conditions_block_or_allow_using_current_and_provided_values() {
        let f = Fx::new();
        let ok = f.add_field(f.task, "Getestet", FieldKind::Checkbox, Default::default(), false);
        set_transition(
            &f.c,
            f.task,
            &TransitionDef {
                from_status_id: f.progress,
                to_status_id: f.done,
                conditions: vec![Condition { field_id: ok.id, op: ConditionOp::Eq, value: json!(true), allow: true }],
                ..Default::default()
            },
        )
        .unwrap();
        let t = f.mk(f.task, "T");
        transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
        assert_eq!(transition_ticket(&f.c, t.id, f.done, &[], &ui()).unwrap_err(), TicketError::ConditionsNotMet);
        assert_eq!(transition_ticket(&f.c, t.id, f.done, &[(ok.id, json!(false))], &ui()).unwrap_err(), TicketError::ConditionsNotMet);
        // Der mitgegebene Wert erfüllt die Bedingung und wird mit geschrieben
        transition_ticket(&f.c, t.id, f.done, &[(ok.id, json!(true))], &ui()).unwrap();
        assert_eq!(values::get_value(&f.c, t.id, &ok).unwrap(), Some(json!(true)));
        // „sperren“-Bedingung
        let blocked = f.add_field(f.task, "Blockiert", FieldKind::Checkbox, Default::default(), false);
        set_transition(
            &f.c,
            f.task,
            &TransitionDef {
                from_status_id: f.todo,
                to_status_id: f.progress,
                conditions: vec![Condition { field_id: blocked.id, op: ConditionOp::Eq, value: json!(true), allow: false }],
                ..Default::default()
            },
        )
        .unwrap();
        let t2 = f.mk(f.task, "T2");
        set_field(&f.c, t2.id, blocked.id, json!(true), &ui()).unwrap();
        assert_eq!(transition_ticket(&f.c, t2.id, f.progress, &[], &ui()).unwrap_err(), TicketError::ConditionsNotMet);
        set_field(&f.c, t2.id, blocked.id, json!(false), &ui()).unwrap();
        assert!(transition_ticket(&f.c, t2.id, f.progress, &[], &ui()).unwrap().is_some());
    }

    #[test]
    fn set_values_are_written_and_can_satisfy_required_fields() {
        let f = Fx::new();
        let res = f.add_field(f.task, "Resolution", FieldKind::Dropdown, Fx::dropdown(&["Fixed", "Won't fix"]), false);
        let closed = f.add_field(f.task, "Erledigt am", FieldKind::Date, Default::default(), false);
        set_transition(
            &f.c,
            f.task,
            &TransitionDef {
                from_status_id: f.progress,
                to_status_id: f.done,
                required_fields: vec![res.id],
                set_values: vec![SetValue { field_id: res.id, value: json!(1) }, SetValue { field_id: closed.id, value: json!("2026-11-14") }],
                ..Default::default()
            },
        )
        .unwrap();
        let t = f.mk(f.task, "T");
        transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
        // Setze-Wert füllt das Pflichtfeld: kein Dialog nötig
        transition_ticket(&f.c, t.id, f.done, &[], &ui()).unwrap();
        assert_eq!(values::get_value(&f.c, t.id, &res).unwrap(), Some(json!(1)));
        assert_eq!(values::get_value(&f.c, t.id, &closed).unwrap(), Some(json!("2026-11-14")));
        // Setze-Werte überschreiben mitgegebene Werte
        let t2 = f.mk(f.task, "T2");
        transition_ticket(&f.c, t2.id, f.progress, &[], &ui()).unwrap();
        transition_ticket(&f.c, t2.id, f.done, &[(res.id, json!(2))], &ui()).unwrap();
        assert_eq!(values::get_value(&f.c, t2.id, &res).unwrap(), Some(json!(1)));
    }

    #[test]
    fn rules_are_identical_for_every_source() {
        let f = Fx::new();
        let reason = f.add_field(f.task, "Lösung", FieldKind::Text, Default::default(), false);
        set_transition(&f.c, f.task, &TransitionDef { from_status_id: f.todo, to_status_id: f.done, required_fields: vec![reason.id], ..Default::default() }).unwrap();
        for s in [Source::ui(), Source::quick(), Source::agent("A"), Source::import()] {
            let t = f.mk(f.task, "T");
            assert!(matches!(transition_ticket(&f.c, t.id, f.done, &[], &s), Err(TicketError::TransitionNeedsFields(_))), "{:?}", s.kind);
            assert!(transition_ticket(&f.c, t.id, f.done, &[(reason.id, json!("ok"))], &s).unwrap().is_some());
            let h = history_of(&f.c, t.id).unwrap();
            assert!(h.iter().filter(|e| e.field != HistoryField::Created).all(|e| e.source == s));
        }
    }

    #[test]
    fn available_transitions_report_blocks_and_missing_fields() {
        let f = Fx::new();
        let reason = f.add_field(f.task, "Lösung", FieldKind::Text, Default::default(), false);
        let flag = f.add_field(f.task, "Frei", FieldKind::Checkbox, Default::default(), false);
        set_transition(&f.c, f.task, &TransitionDef { from_status_id: f.todo, to_status_id: f.done, required_fields: vec![reason.id], ..Default::default() }).unwrap();
        set_transition(
            &f.c,
            f.task,
            &TransitionDef {
                from_status_id: f.todo,
                to_status_id: f.progress,
                conditions: vec![Condition { field_id: flag.id, op: ConditionOp::Eq, value: json!(true), allow: true }],
                ..Default::default()
            },
        )
        .unwrap();
        let t = f.mk(f.task, "T");
        let opts = available_transitions(&f.c, t.id).unwrap();
        assert_eq!(opts.len(), 2);
        let prog = opts.iter().find(|o| o.status_id == f.progress).unwrap();
        assert!(prog.blocked);
        let done = opts.iter().find(|o| o.status_id == f.done).unwrap();
        assert!(!done.blocked && done.needs_fields.len() == 1);
        set_field(&f.c, t.id, flag.id, json!(true), &ui()).unwrap();
        let opts = available_transitions(&f.c, t.id).unwrap();
        assert!(!opts.iter().find(|o| o.status_id == f.progress).unwrap().blocked);
        // ohne Workflow: leer
        let plain = crate::planning::types::create_type(
            &f.c,
            &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Notiz".into()), icon: None, color: "#445566".into(), workflow: false },
        )
        .unwrap();
        assert!(available_transitions(&f.c, f.mk(plain, "N").id).unwrap().is_empty());
    }

    #[test]
    fn ticket_without_status_may_enter_any_workflow_status() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        f.c.execute("UPDATE tickets SET status_id = NULL WHERE id = ?1", [t.id]).unwrap();
        assert_eq!(available_transitions(&f.c, t.id).unwrap().len(), 3);
        transition_ticket(&f.c, t.id, f.done, &[], &ui()).unwrap();
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.done));
    }

    #[test]
    fn workflow_changes_keep_tickets_consistent() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        // Status mit Tickets lässt sich nicht aus dem Workflow nehmen (Integritätsregel greift wirklich)
        assert_eq!(set_workflow_statuses(&f.c, f.task, &[f.progress, f.done]), Err(crate::planning::PlanError::InUse { count: 1 }));
        // Workflow abschalten braucht Verwerfen und leert die Status
        assert_eq!(set_workflow(&f.c, f.task, false, false), Err(crate::planning::PlanError::InUse { count: 1 }));
        set_workflow(&f.c, f.task, false, true).unwrap();
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, None);
    }
}
