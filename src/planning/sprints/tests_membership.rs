//! Tests von Rückgängig/Wiederholen für die Sprint-Zugehörigkeit (`membership`): Zustand der Zeilen, Burndown, Bericht und
//! Velocity nach Undo/Redo, und wann es nicht mehr geht.

use super::membership::{move_tickets_to_backlog_tracked, move_tickets_to_sprint_tracked, SprintChange};
use super::sprint::*;
use super::tests_sprint::{d, S};
use super::*;
use crate::calc::burndown::BurndownSeries;
use crate::planning::ops::TicketOp;
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::Ticket;
use crate::state::Lang;
use crate::undo::History;

fn series(s: &S, sp: i64) -> BurndownSeries {
    burndown_for_sprint(s.c(), sp, Basis::Points, s.cal.clone(), d(2026, 11, 13)).unwrap()
}

fn actual(b: &BurndownSeries) -> Vec<f64> {
    b.actual.iter().map(|v| v.expect("Ist-Wert")).collect()
}

/// Alle Zeilen von `sprint_tickets` in fester Reihenfolge (für „sieht aus wie vorher“).
fn dump(c: &Connection) -> Vec<(i64, i64, i64, String, Option<String>, Option<f64>)> {
    let mut st = c.prepare("SELECT id, sprint_id, ticket_id, added_at, removed_at, points_at_add FROM sprint_tickets ORDER BY id").unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

/// Gestarteter Sprint „S“ (2.–13.11.) mit den Tickets (Punkte), Uhr auf dem Start.
fn running(s: &S, points: &[Option<f64>]) -> (i64, Vec<Ticket>) {
    let sp = s.dated("S");
    let ts: Vec<Ticket> = points.iter().enumerate().map(|(i, p)| s.task(&format!("T{i}"), *p)).collect();
    s.at("2026-10-30T10:00:00");
    move_tickets_to_sprint(s.c(), sp, &ts.iter().map(|t| t.id).collect::<Vec<_>>()).unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    (sp, ts)
}

fn op(change: Option<SprintChange>) -> TicketOp {
    TicketOp::Sprint(change.expect("etwas hat sich bewegt"))
}

#[test]
fn adding_to_the_running_sprint_leaves_no_trace_after_undo_and_comes_back_with_redo() {
    let s = S::new();
    let (sp, _t) = running(&s, &[Some(10.0)]);
    let extra = s.task("Nachzügler", Some(4.0));
    let (before_rows, before_series, before_report) = (dump(s.c()), series(&s, sp), sprint_report(s.c(), sp).unwrap());
    s.at("2026-11-04T10:00:00");
    let (moved, change) = move_tickets_to_sprint_tracked(s.c(), sp, &[extra.id]).unwrap();
    assert_eq!(moved, 1);
    let (after_rows, after_series) = (dump(s.c()), series(&s, sp));
    assert_eq!((after_series.scope_added, sprint_report(s.c(), sp).unwrap().added.tickets), (4.0, 1));
    let mut h: History<TicketOp> = History::new(10);
    h.push(op(change));
    // Rückgängig: kein hinzugefügt-und-entfernt-Paar, Zeilen und Burndown wie vorher
    s.at("2026-11-09T09:00:00");
    assert!(h.undo(s.c()).unwrap());
    assert_eq!(dump(s.c()), before_rows, "die Zeile ist ganz verschwunden, nicht nur als entfernt markiert");
    let b = series(&s, sp);
    assert_eq!((b.scope_added, b.scope_removed, b.scope_total_start), (0.0, 0.0, 10.0));
    assert_eq!(actual(&b), actual(&before_series));
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!((r.added.tickets, r.removed.tickets), (0, 0));
    assert_eq!(r, before_report);
    // Wiederholen: dieselben Zeilen mit dem ursprünglichen Zeitstempel (4.11.), nicht dem von jetzt (9.11.)
    assert!(h.redo(s.c()).unwrap());
    assert_eq!(dump(s.c()), after_rows);
    let b = series(&s, sp);
    assert_eq!(actual(&b), actual(&after_series));
    assert_eq!(b.scope_added, 4.0);
    // und noch einmal hin und her
    assert!(h.undo(s.c()).unwrap());
    assert!(h.redo(s.c()).unwrap());
    assert_eq!(dump(s.c()), after_rows);
}

#[test]
fn removing_from_the_running_sprint_is_restored_exactly_including_the_old_points() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(8.0), Some(2.0)]);
    let before_rows = dump(s.c());
    s.at("2026-11-06T09:00:00");
    let (moved, change) = move_tickets_to_backlog_tracked(s.c(), &[t[1].id]).unwrap();
    assert_eq!(moved, 1);
    let removed_rows = dump(s.c());
    let removed_series = series(&s, sp);
    assert_eq!(removed_series.scope_removed, 2.0);
    // die Punkte ändern sich zwischendurch: Rückgängig nimmt trotzdem den alten Wert (points_at_add) wieder auf
    s.set_points(&t[1], 20.0);
    let mut h: History<TicketOp> = History::new(10);
    h.push(op(change));
    s.at("2026-11-10T09:00:00");
    assert!(h.undo(s.c()).unwrap());
    assert_eq!(dump(s.c()), before_rows);
    let b = series(&s, sp);
    assert_eq!((b.scope_removed, b.scope_added), (0.0, 0.0));
    assert_eq!(sprint_content(s.c(), sp).unwrap().items.len(), 2);
    assert!(h.redo(s.c()).unwrap());
    assert_eq!(dump(s.c()), removed_rows);
    assert_eq!(series(&s, sp).scope_removed, 2.0);
}

#[test]
fn a_drop_into_and_out_of_a_planned_sprint_restores_the_rows_with_the_same_ids() {
    let s = S::new();
    s.at("2026-10-30T09:00:00");
    let sp = s.sprint("Plan");
    let (a, b) = (s.task("A", Some(3.0)), s.task("B", None));
    let (_, c1) = move_tickets_to_sprint_tracked(s.c(), sp, &[a.id, b.id]).unwrap();
    let both = dump(s.c());
    assert_eq!(both.len(), 2);
    s.at("2026-10-30T11:00:00");
    let (_, c2) = move_tickets_to_backlog_tracked(s.c(), &[a.id]).unwrap();
    assert_eq!(dump(s.c()).len(), 1, "im geplanten Sprint wird die Zeile gelöscht");
    let mut h: History<TicketOp> = History::new(10);
    h.push(op(c1));
    h.push(op(c2));
    assert!(h.undo(s.c()).unwrap());
    assert_eq!(dump(s.c()), both, "dieselbe Zeilen-Kennung, derselbe Zeitstempel, dieselben Punkte");
    assert_eq!(sprint_of(s.c(), a.id), Some(sp));
    assert!(h.undo(s.c()).unwrap());
    assert!(dump(s.c()).is_empty());
    assert_eq!(sprint_of(s.c(), a.id), None);
    assert!(h.redo(s.c()).unwrap() && h.redo(s.c()).unwrap());
    assert_eq!(dump(s.c()).len(), 1);
    assert_eq!(sprint_of(s.c(), b.id), Some(sp));
}

#[test]
fn moving_between_two_sprints_is_one_step() {
    let s = S::new();
    let (s1, s2) = (s.sprint("Eins"), s.sprint("Zwei"));
    let t = s.task("T", None);
    move_tickets_to_sprint(s.c(), s1, &[t.id]).unwrap();
    let rows = dump(s.c());
    let (_, change) = move_tickets_to_sprint_tracked(s.c(), s2, &[t.id]).unwrap();
    assert_eq!(sprint_of(s.c(), t.id), Some(s2));
    let mut h: History<TicketOp> = History::new(5);
    h.push(op(change));
    assert!(h.undo(s.c()).unwrap());
    assert_eq!(dump(s.c()), rows);
    assert_eq!(sprint_of(s.c(), t.id), Some(s1));
    assert!(h.redo(s.c()).unwrap());
    assert_eq!(sprint_of(s.c(), t.id), Some(s2));
}

#[test]
fn nothing_moved_means_no_operation() {
    let s = S::new();
    let sp = s.sprint("S");
    let t = s.task("T", None);
    assert!(move_tickets_to_backlog_tracked(s.c(), &[t.id]).unwrap().1.is_none(), "war schon im Backlog");
    move_tickets_to_sprint(s.c(), sp, &[t.id]).unwrap();
    let (moved, change) = move_tickets_to_sprint_tracked(s.c(), sp, &[t.id]).unwrap();
    assert_eq!(moved, 0);
    assert!(change.is_none(), "war schon im Sprint");
}

#[test]
fn the_inverse_of_the_inverse_is_the_original_and_touches_finds_the_tickets() {
    let s = S::new();
    let sp = s.sprint("S");
    let (a, b) = (s.task("A", None), s.task("B", None));
    let change = move_tickets_to_sprint_tracked(s.c(), sp, &[a.id]).unwrap().1.unwrap();
    let o = TicketOp::Sprint(change.clone());
    assert_eq!(crate::undo::Op::inverse(&crate::undo::Op::inverse(&o)), o);
    assert!(o.touches(&[a.id]) && !o.touches(&[b.id]));
}

#[test]
fn undo_is_refused_once_the_sprint_was_started_completed_or_deleted() {
    // gestartet: der Zusage-Umfang darf nicht nachträglich verändert werden
    let s = S::new();
    let sp = s.dated("S");
    let t = s.task("T", Some(2.0));
    s.at("2026-10-30T10:00:00");
    let change = move_tickets_to_sprint_tracked(s.c(), sp, &[t.id]).unwrap().1.unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    let rows = dump(s.c());
    assert_eq!(change.inverse().apply(s.c()), Err(TicketError::SprintChanged));
    assert_eq!(dump(s.c()), rows, "unverändert");
    // abgeschlossen
    complete_sprint(s.c(), sp, Leftover::ToBacklog).unwrap();
    assert_eq!(change.inverse().apply(s.c()), Err(TicketError::SprintChanged));
    // gelöscht
    let s = S::new();
    let sp = s.sprint("Weg");
    let t = s.task("T", None);
    let change = move_tickets_to_sprint_tracked(s.c(), sp, &[t.id]).unwrap().1.unwrap();
    delete_sprint(s.c(), sp).unwrap();
    assert_eq!(change.inverse().apply(s.c()), Err(TicketError::SprintChanged));
    assert_eq!(change.apply(s.c()), Err(TicketError::SprintChanged));
}

#[test]
fn undo_is_refused_when_the_rows_were_changed_in_the_meantime_without_history() {
    let s = S::new();
    let (s1, s2) = (s.sprint("Eins"), s.sprint("Zwei"));
    let t = s.task("T", None);
    let change = move_tickets_to_sprint_tracked(s.c(), s1, &[t.id]).unwrap().1.unwrap();
    // jemand (Agent, anderer Weg) schiebt das Ticket ohne Historie in den anderen Sprint
    move_tickets_to_sprint(s.c(), s2, &[t.id]).unwrap();
    let rows = dump(s.c());
    assert_eq!(change.inverse().apply(s.c()), Err(TicketError::SprintChanged));
    assert_eq!(dump(s.c()), rows);
    // über die Historie: Fehlermeldung in beiden Sprachen, Eintrag verworfen
    let mut h: History<TicketOp> = History::new(5);
    h.push(TicketOp::Sprint(change));
    let e = h.undo(s.c()).unwrap_err();
    assert_eq!(e, TicketError::SprintChanged.message(crate::state::lang()));
    assert!(!h.can_undo());
    for lang in [Lang::De, Lang::En] {
        assert!(!TicketError::SprintChanged.message(lang).is_empty());
    }
}

#[test]
fn a_failed_batch_leaves_the_membership_untouched() {
    let s = S::new();
    let sp = s.sprint("S");
    let t = s.task("T", None);
    let change = move_tickets_to_sprint_tracked(s.c(), sp, &[t.id]).unwrap().1.unwrap();
    // Rang-Änderung mit nicht vorhandenem Ticket nach der Zugehörigkeit: das ganze Batch scheitert, die Zeile bleibt weg/da wie vorher
    let batch = TicketOp::Batch(vec![TicketOp::Sprint(change.clone()), TicketOp::Rank { changes: vec![(99_999, 0, 1)] }]);
    let mut h: History<TicketOp> = History::new(5);
    h.push(batch);
    assert!(h.undo(s.c()).is_err());
    assert_eq!(sprint_of(s.c(), t.id), Some(sp), "nichts halb zurückgenommen");
}

#[test]
fn deleting_the_ticket_drops_its_entries_from_the_history() {
    let s = S::new();
    let sp = s.sprint("S");
    let t = s.task("T", None);
    let change = move_tickets_to_sprint_tracked(s.c(), sp, &[t.id]).unwrap().1.unwrap();
    let mut h: History<TicketOp> = History::new(5);
    h.push(TicketOp::Sprint(change));
    h.retain(|o| !o.touches(&[t.id]));
    assert!(!h.can_undo());
}

#[test]
fn velocity_is_unaffected_by_an_undone_drop_into_a_finished_sprint() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), Some(5.0)]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    let extra = s.task("Später", Some(13.0));
    s.at("2026-11-05T10:00:00");
    let change = move_tickets_to_sprint_tracked(s.c(), sp, &[extra.id]).unwrap().1.unwrap();
    let mut h: History<TicketOp> = History::new(5);
    h.push(TicketOp::Sprint(change));
    assert!(h.undo(s.c()).unwrap());
    s.at("2026-11-13T17:00:00");
    complete_sprint(s.c(), sp, Leftover::ToBacklog).unwrap();
    let v = velocity_for_project(s.c(), s.p(), 5, Basis::Points).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!((v[0].planned, v[0].completed), (8.0, 3.0), "die zurückgenommene 13 zählt nicht");
}
