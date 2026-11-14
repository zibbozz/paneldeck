//! Tests von Burndown, Bericht und Velocity aus den echten Datenbank-Ereignissen (Zugehörigkeit + Status-Verlauf).
//!
//! Der Sprint läuft, wenn nichts anderes steht, von Mo 2026-11-02 bis Fr 2026-11-13 (12 Kalendertage, 10 Arbeitstage); die Reihen
//! haben 12 Werte (Index 0 = 2. 11., Index 11 = 13. 11.).

use super::sprint::*;
use super::tests_sprint::{d, S};
use super::*;
use crate::calc::burndown::BurndownSeries;
use crate::calc::calendar::WorkCalendar;
use crate::planning::tickets::Ticket;

fn near(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-9, "{got:?} vs {want:?}");
    }
}

fn actual(b: &BurndownSeries) -> Vec<f64> {
    b.actual.iter().map(|v| v.expect("Ist-Wert")).collect()
}

/// Sprint „S“ mit Tickets in der Planung (30.10. 10:00) und Start am 2.11. um 8:00; die Uhr steht danach auf dem Start.
fn running(s: &S, points: &[Option<f64>]) -> (i64, Vec<Ticket>) {
    let sp = s.dated("S");
    let ts: Vec<Ticket> = points.iter().enumerate().map(|(i, p)| s.task(&format!("T{i}"), *p)).collect();
    s.at("2026-10-30T10:00:00");
    move_tickets_to_sprint(s.c(), sp, &ts.iter().map(|t| t.id).collect::<Vec<_>>()).unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    (sp, ts)
}

fn series(s: &S, sp: i64, basis: Basis) -> BurndownSeries {
    burndown_for_sprint(s.c(), sp, basis, s.cal.clone(), d(2026, 11, 13)).unwrap()
}

#[test]
fn the_burndown_follows_the_completions_in_the_status_history() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), Some(5.0), Some(2.0)]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    s.status(&t[1], s.f.done, "2026-11-05T09:00:00");
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b), &[10.0, 7.0, 7.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0]);
    near(&b.ideal, &[9.0, 8.0, 7.0, 6.0, 5.0, 5.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.0]);
    assert_eq!((b.days[0], b.days[11]), (d(2026, 11, 2), d(2026, 11, 13)));
    assert_eq!((b.scope_total_start, b.completed, b.scope_added, b.scope_removed), (10.0, 8.0, 0.0, 0.0));
}

#[test]
fn work_in_progress_does_not_burn_down() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), Some(5.0)]);
    s.status(&t[0], s.f.progress, "2026-11-03T10:00:00");
    near(&actual(&series(&s, sp, Basis::Points)), &[8.0; 12]);
}

#[test]
fn tickets_added_after_the_start_raise_the_remaining_work() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(10.0)]);
    let extra = s.task("Nachzügler", Some(4.0));
    s.at("2026-11-04T10:00:00");
    move_tickets_to_sprint(s.c(), sp, &[extra.id]).unwrap();
    let b = series(&s, sp, Basis::Points);
    let mut want = vec![10.0, 10.0];
    want.extend([14.0; 10]);
    near(&actual(&b), &want);
    assert_eq!((b.scope_total_start, b.scope_added), (10.0, 4.0));
    let _ = t;
}

#[test]
fn tickets_removed_after_the_start_lower_the_remaining_work() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(8.0), Some(2.0)]);
    s.at("2026-11-06T09:00:00");
    move_tickets_to_backlog(s.c(), &[t[1].id]).unwrap();
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b), &[10.0, 10.0, 10.0, 10.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0]);
    assert_eq!((b.scope_removed, b.completed), (2.0, 0.0));
}

#[test]
fn a_reopened_ticket_comes_back_into_the_remaining_work() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), Some(5.0)]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    s.status(&t[0], s.f.progress, "2026-11-04T09:00:00");
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b), &[8.0, 5.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0]);
    assert_eq!(b.completed, 0.0, "erledigt minus wieder geöffnet");
    // Und wieder erledigt.
    s.status(&t[0], s.f.done, "2026-11-05T09:00:00");
    let b = series(&s, sp, Basis::Points);
    assert_eq!(actual(&b)[3], 5.0);
    assert_eq!(b.completed, 3.0);
}

#[test]
fn the_weekend_stays_flat_on_the_ideal_line_unless_weekends_count() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(12.0)]);
    s.status(&t[0], s.f.done, "2026-11-13T09:00:00");
    let work = burndown_for_sprint(s.c(), sp, Basis::Points, WorkCalendar::with_weekend_switch(true), d(2026, 11, 13)).unwrap();
    near(&work.ideal, &[10.8, 9.6, 8.4, 7.2, 6.0, 6.0, 6.0, 4.8, 3.6, 2.4, 1.2, 0.0]);
    let every = burndown_for_sprint(s.c(), sp, Basis::Points, WorkCalendar::with_weekend_switch(false), d(2026, 11, 13)).unwrap();
    near(&every.ideal, &[11.0, 10.0, 9.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.0]);
    // Feiertage fallen aus der Ideal-Linie.
    let holiday = burndown_for_sprint(s.c(), sp, Basis::Points, WorkCalendar::default().with_holidays(vec![d(2026, 11, 4)]), d(2026, 11, 13)).unwrap();
    assert_eq!(holiday.ideal[2], holiday.ideal[1], "am freien Tag bleibt die Linie waagerecht");
    near(&actual(&work), &[12.0, 12.0, 12.0, 12.0, 12.0, 12.0, 12.0, 12.0, 12.0, 12.0, 12.0, 0.0]);
}

#[test]
fn the_basis_decides_between_points_and_ticket_count() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), None]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    let by = |b: Basis| series(&s, sp, b);
    assert_eq!(by(Basis::Points).scope_total_start, 3.0, "ohne Punkte zählt ein Ticket 0");
    assert_eq!(by(Basis::TicketCount).scope_total_start, 2.0);
    assert_eq!(by(Basis::PointsWithFallbackOne).scope_total_start, 4.0, "ohne Punkte zählt ein Ticket 1");
    near(&actual(&by(Basis::TicketCount))[..3], &[2.0, 1.0, 1.0]);
    near(&actual(&by(Basis::PointsWithFallbackOne))[..3], &[4.0, 1.0, 1.0]);
}

#[test]
fn work_that_was_done_before_joining_the_sprint_does_not_count() {
    let s = S::new();
    let sp = s.dated("S");
    let (done, open) = (s.task("schon fertig", Some(5.0)), s.task("offen", Some(3.0)));
    s.status(&done, s.f.done, "2026-10-29T09:00:00");
    s.at("2026-10-30T10:00:00");
    move_tickets_to_sprint(s.c(), sp, &[done.id, open.id]).unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    let b = series(&s, sp, Basis::Points);
    assert_eq!((b.scope_total_start, b.completed), (3.0, 0.0));
    near(&actual(&b)[..2], &[3.0, 3.0]);
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!((r.planned.tickets, r.planned.points), (1, 3.0));
    assert_eq!(r.completed.tickets, 0);
    let e = r.entries.iter().find(|e| e.ticket_id == done.id).unwrap();
    assert!(!e.planned && !e.added && !e.completed && !e.open && !e.removed);
    // Der Inhalt zeigt es trotzdem (als erledigt).
    assert_eq!(sprint_content(s.c(), sp).unwrap().totals.done.tickets, 1);
}

#[test]
fn reopening_a_ticket_that_joined_done_is_new_scope() {
    let s = S::new();
    let sp = s.dated("S");
    let (x, y) = (s.task("x", Some(5.0)), s.task("y", Some(3.0)));
    s.status(&x, s.f.done, "2026-10-29T09:00:00");
    s.at("2026-10-30T10:00:00");
    move_tickets_to_sprint(s.c(), sp, &[x.id, y.id]).unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    s.status(&x, s.f.progress, "2026-11-04T09:00:00");
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b)[..4], &[3.0, 3.0, 8.0, 8.0]);
    assert_eq!((b.scope_added, b.completed), (5.0, 0.0));
    let r = sprint_report(s.c(), sp).unwrap();
    let e = r.entries.iter().find(|e| e.ticket_id == x.id).unwrap();
    assert!(e.added && e.open && !e.planned);
}

#[test]
fn what_was_added_before_the_start_belongs_to_the_planned_scope_even_on_the_start_day() {
    let s = S::new();
    let sp = s.dated("S");
    let (a, b) = (s.task("a", Some(5.0)), s.task("b", Some(3.0)));
    s.at("2026-11-02T07:00:00");
    move_tickets_to_sprint(s.c(), sp, &[a.id]).unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    s.at("2026-11-02T09:00:00");
    move_tickets_to_sprint(s.c(), sp, &[b.id]).unwrap();
    let bd = series(&s, sp, Basis::Points);
    assert_eq!((bd.scope_total_start, bd.scope_added), (5.0, 3.0));
    assert_eq!(actual(&bd)[0], 8.0, "am Ende des ersten Tages ist alles drin");
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!((r.planned.points, r.added.points), (5.0, 3.0));
}

#[test]
fn a_late_start_still_counts_earlier_additions_as_planned() {
    let s = S::new();
    let sp = s.dated("S");
    let a = s.task("a", Some(5.0));
    s.at("2026-11-03T09:00:00"); // schon im Sprintzeitraum, aber noch nicht gestartet
    move_tickets_to_sprint(s.c(), sp, &[a.id]).unwrap();
    s.at("2026-11-04T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    let b = series(&s, sp, Basis::Points);
    assert_eq!(b.scope_total_start, 5.0);
    assert_eq!(actual(&b)[0], 5.0);
    assert_eq!(sprint_report(s.c(), sp).unwrap().added.tickets, 0);
}

#[test]
fn a_sprint_that_has_not_started_counts_everything_as_planned() {
    let s = S::new();
    let sp = s.dated("S");
    let a = s.task("a", Some(5.0));
    s.at("2026-11-05T09:00:00");
    move_tickets_to_sprint(s.c(), sp, &[a.id]).unwrap();
    let b = series(&s, sp, Basis::Points);
    assert_eq!(b.scope_total_start, 5.0);
    assert_eq!(sprint_report(s.c(), sp).unwrap().planned.tickets, 1);
}

#[test]
fn the_burndown_needs_both_dates() {
    let s = S::new();
    let sp = s.sprint("ohne Termine");
    assert_eq!(burndown_for_sprint(s.c(), sp, Basis::Points, s.cal.clone(), d(2026, 11, 2)).map(|_| ()), Err(SprintError::DatesIncomplete));
    assert!(burndown_for_sprint(s.c(), 9999, Basis::Points, s.cal.clone(), d(2026, 11, 2)).unwrap_err().is_not_found());
}

#[test]
fn days_after_today_have_no_actual_value_and_later_events_are_ignored() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(4.0)]);
    s.status(&t[0], s.f.done, "2026-11-16T09:00:00"); // nach dem Sprintende
    let b = series(&s, sp, Basis::Points);
    assert_eq!(actual(&b), vec![4.0; 12]);
    let early = burndown_for_sprint(s.c(), sp, Basis::Points, s.cal.clone(), d(2026, 11, 3)).unwrap();
    assert_eq!(early.actual.iter().filter(|v| v.is_some()).count(), 2);
    let before = burndown_for_sprint(s.c(), sp, Basis::Points, s.cal.clone(), d(2026, 11, 1)).unwrap();
    assert!(before.actual.iter().all(|v| v.is_none()));
}

#[test]
fn the_ticket_points_at_adding_time_keep_the_lines_consistent() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0)]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    s.at("2026-11-04T10:00:00");
    s.set_points(&t[0], 8.0);
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b)[..3], &[3.0, 0.0, 0.0]);
    assert_eq!(b.completed, 3.0);
}

#[test]
fn removing_a_finished_ticket_does_not_change_the_remaining_work() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), Some(5.0)]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    s.at("2026-11-05T09:00:00");
    move_tickets_to_backlog(s.c(), &[t[0].id]).unwrap();
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b), &[8.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0]);
    assert_eq!((b.scope_removed, b.completed), (0.0, 3.0));
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!((r.removed.tickets, r.completed.tickets), (0, 1), "das erledigte Ticket gilt weiter als erledigt");
}

#[test]
fn a_ticket_that_leaves_and_returns_counts_twice_in_the_history() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(5.0), Some(1.0)]);
    s.at("2026-11-04T09:00:00");
    move_tickets_to_backlog(s.c(), &[t[0].id]).unwrap();
    s.at("2026-11-05T09:00:00");
    move_tickets_to_sprint(s.c(), sp, &[t[0].id]).unwrap();
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b)[..5], &[6.0, 6.0, 1.0, 6.0, 6.0]);
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!(r.entries.len(), 3);
    assert_eq!((r.planned.tickets, r.removed.tickets, r.added.tickets, r.open.tickets), (2, 1, 1, 2));
}

// ------------------------------------------------------------------ Bericht

/// Szenario: a(3), b(5), c(2) geplant; a erledigt (3.11.), d(4) kommt am 4.11. dazu, c wird am 5.11. entfernt, b erledigt (6.11.).
fn full_scenario(s: &S) -> (i64, [Ticket; 4]) {
    let (sp, t) = running(s, &[Some(3.0), Some(5.0), Some(2.0)]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    let d4 = s.task("d", Some(4.0));
    s.at("2026-11-04T10:00:00");
    move_tickets_to_sprint(s.c(), sp, &[d4.id]).unwrap();
    s.at("2026-11-05T09:00:00");
    move_tickets_to_backlog(s.c(), &[t[2].id]).unwrap();
    s.status(&t[1], s.f.done, "2026-11-06T09:00:00");
    (sp, [t[0].clone(), t[1].clone(), t[2].clone(), d4])
}

#[test]
fn the_report_counts_planned_completed_added_removed_and_open_work() {
    let s = S::new();
    let (sp, [a, b, c, dd]) = full_scenario(&s);
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!((r.planned.tickets, r.planned.points), (3, 10.0));
    assert_eq!((r.completed.tickets, r.completed.points), (2, 8.0));
    assert_eq!((r.added.tickets, r.added.points), (1, 4.0));
    assert_eq!((r.removed.tickets, r.removed.points), (1, 2.0));
    assert_eq!((r.open.tickets, r.open.points), (1, 4.0));
    assert_eq!(r.sprint.id, sp);
    let by = |id: i64| r.entries.iter().find(|e| e.ticket_id == id).unwrap().clone();
    assert!(by(a.id).planned && by(a.id).completed && !by(a.id).open);
    assert!(by(b.id).planned && by(b.id).completed);
    assert!(by(c.id).planned && by(c.id).removed && !by(c.id).open && !by(c.id).completed);
    assert!(by(dd.id).added && by(dd.id).open && !by(dd.id).planned);
    assert_eq!(r.planned.weight(Basis::TicketCount), 3.0);
}

#[test]
fn the_full_scenario_matches_the_burndown() {
    let s = S::new();
    let (sp, _) = full_scenario(&s);
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b), &[10.0, 7.0, 11.0, 9.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0]);
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!(b.scope_total_start, r.planned.weight(Basis::Points));
    assert_eq!(b.completed, r.completed.weight(Basis::Points));
    assert_eq!(b.scope_added, r.added.weight(Basis::Points));
    assert_eq!(b.scope_removed, r.removed.weight(Basis::Points));
    assert_eq!(*actual(&b).last().unwrap(), r.open.weight(Basis::Points));
}

#[test]
fn the_report_of_a_completed_sprint_is_frozen_at_completion() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), Some(5.0), Some(2.0)]);
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    s.at("2026-11-10T17:00:00");
    complete_sprint(s.c(), sp, Leftover::ToBacklog).unwrap();
    // Danach wird Ticket b erledigt: im abgeschlossenen Sprint zählt das nicht mehr.
    s.status(&t[1], s.f.done, "2026-11-11T10:00:00");
    let r = sprint_report(s.c(), sp).unwrap();
    assert_eq!((r.planned.tickets, r.completed.tickets, r.open.tickets, r.removed.tickets), (3, 1, 2, 0));
    assert_eq!(r.open.points, 7.0);
    let b = series(&s, sp, Basis::Points);
    near(&actual(&b), &[10.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0]);
    assert!(*actual(&b).last().unwrap() > 0.0, "unerledigte Tickets bleiben im Sprint offen");
}

#[test]
fn carried_over_tickets_form_the_planned_scope_of_the_next_sprint() {
    let s = S::new();
    let (sp, t) = running(&s, &[Some(3.0), Some(5.0), Some(2.0)]);
    let mut n = NewSprint::new(s.p(), "S2");
    n.start = Some(d(2026, 11, 16));
    n.end = Some(d(2026, 11, 27));
    let next = create_sprint(s.c(), &n, &s.cal).unwrap();
    s.status(&t[0], s.f.done, "2026-11-03T10:00:00");
    s.at("2026-11-10T17:00:00");
    complete_sprint(s.c(), sp, Leftover::ToSprint(next)).unwrap();
    s.at("2026-11-16T08:00:00");
    start_sprint(s.c(), next, &s.cal).unwrap();
    s.status(&t[1], s.f.done, "2026-11-17T09:00:00");
    let b = burndown_for_sprint(s.c(), next, Basis::Points, s.cal.clone(), d(2026, 11, 27)).unwrap();
    assert_eq!(b.scope_total_start, 7.0);
    near(&actual(&b)[..3], &[7.0, 2.0, 2.0]);
    assert_eq!(b.completed, 5.0);
    // Der erste Sprint bleibt davon unberührt.
    assert_eq!(sprint_report(s.c(), sp).unwrap().completed.points, 3.0);
}

#[test]
fn the_status_before_adding_is_read_from_the_whole_history() {
    // Mehrere Wechsel vor dem Hinzufügen: nur der letzte zählt; der Sprint-Beitritt in „In Arbeit“ ist nicht erledigt.
    let s = S::new();
    let sp = s.dated("S");
    let x = s.task("x", Some(4.0));
    s.status(&x, s.f.progress, "2026-10-27T09:00:00");
    s.status(&x, s.f.done, "2026-10-28T09:00:00");
    s.status(&x, s.f.progress, "2026-10-29T09:00:00");
    s.at("2026-10-30T10:00:00");
    move_tickets_to_sprint(s.c(), sp, &[x.id]).unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), sp, &s.cal).unwrap();
    s.status(&x, s.f.done, "2026-11-04T09:00:00");
    let b = series(&s, sp, Basis::Points);
    assert_eq!(b.scope_total_start, 4.0);
    near(&actual(&b)[..4], &[4.0, 4.0, 0.0, 0.0]);
}

// ------------------------------------------------------------------ Velocity

/// Zwei abgeschlossene Sprints und ein laufender dritter (siehe Kommentare).
fn three_sprints(s: &S) -> (i64, i64, i64) {
    let s1 = s.dated("S1");
    let mut n2 = NewSprint::new(s.p(), "S2");
    n2.start = Some(d(2026, 11, 16));
    n2.end = Some(d(2026, 11, 27));
    let s2 = create_sprint(s.c(), &n2, &s.cal).unwrap();
    let (x, y, z) = (s.task("x", Some(3.0)), s.task("y", Some(2.0)), s.task("z", Some(5.0)));
    s.at("2026-10-30T10:00:00");
    move_tickets_to_sprint(s.c(), s1, &[x.id, y.id]).unwrap();
    s.at("2026-11-02T08:00:00");
    start_sprint(s.c(), s1, &s.cal).unwrap();
    s.status(&x, s.f.done, "2026-11-03T10:00:00");
    s.at("2026-11-11T09:00:00");
    move_tickets_to_sprint(s.c(), s2, &[z.id]).unwrap();
    s.at("2026-11-13T17:00:00");
    complete_sprint(s.c(), s1, Leftover::ToSprint(s2)).unwrap();
    s.at("2026-11-16T08:00:00");
    start_sprint(s.c(), s2, &s.cal).unwrap();
    s.status(&y, s.f.done, "2026-11-17T09:00:00");
    s.status(&z, s.f.done, "2026-11-18T09:00:00");
    s.at("2026-11-27T17:00:00");
    complete_sprint(s.c(), s2, Leftover::ToBacklog).unwrap();
    let mut n3 = NewSprint::new(s.p(), "S3");
    n3.start = Some(d(2026, 11, 30));
    n3.end = Some(d(2026, 12, 11));
    let s3 = create_sprint(s.c(), &n3, &s.cal).unwrap();
    let w = s.task("w", Some(4.0));
    move_tickets_to_sprint(s.c(), s3, &[w.id]).unwrap();
    s.at("2026-11-30T08:00:00");
    start_sprint(s.c(), s3, &s.cal).unwrap();
    (s1, s2, s3)
}

#[test]
fn the_velocity_compares_planned_and_completed_work_of_finished_sprints() {
    let s = S::new();
    three_sprints(&s);
    let v = velocity_for_project(s.c(), s.p(), 10, Basis::Points).unwrap();
    assert_eq!(v.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), vec!["S1", "S2"], "alt nach neu, der laufende Sprint fehlt");
    assert_eq!((v[0].planned, v[0].completed), (5.0, 3.0));
    assert_eq!(v[0].completion_ratio, Some(0.6));
    assert_eq!((v[1].planned, v[1].completed), (7.0, 7.0), "Übertrag (2) und neu geplant (5)");
    assert_eq!(v[1].completion_ratio, Some(1.0));
}

#[test]
fn the_velocity_takes_only_the_last_sprints() {
    let s = S::new();
    three_sprints(&s);
    let one = velocity_for_project(s.c(), s.p(), 1, Basis::Points).unwrap();
    assert_eq!(one.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), vec!["S2"]);
    assert!(velocity_for_project(s.c(), s.p(), 0, Basis::Points).unwrap().is_empty());
    assert_eq!(velocity_for_project(s.c(), s.p(), 99, Basis::Points).unwrap().len(), 2);
}

#[test]
fn the_velocity_can_count_tickets_instead_of_points() {
    let s = S::new();
    three_sprints(&s);
    let v = velocity_for_project(s.c(), s.p(), 10, Basis::TicketCount).unwrap();
    assert_eq!((v[0].planned, v[0].completed), (2.0, 1.0));
    assert_eq!((v[1].planned, v[1].completed), (2.0, 2.0));
}

#[test]
fn the_velocity_of_a_project_without_finished_sprints_is_empty() {
    let s = S::new();
    assert!(velocity_for_project(s.c(), s.p(), 5, Basis::Points).unwrap().is_empty());
    s.dated("nur geplant");
    assert!(velocity_for_project(s.c(), s.p(), 5, Basis::Points).unwrap().is_empty());
}

#[test]
fn the_average_velocity_builds_on_the_velocity_points() {
    let s = S::new();
    three_sprints(&s);
    let v = velocity_for_project(s.c(), s.p(), 10, Basis::Points).unwrap();
    let results: Vec<crate::calc::burndown::SprintResult> = v.iter().map(|p| crate::calc::burndown::SprintResult { label: p.label.clone(), planned: p.planned, completed: p.completed }).collect();
    assert_eq!(crate::calc::burndown::average_velocity(&results, 2), Some(5.0));
    assert_eq!(crate::calc::burndown::average_velocity(&results, 1), Some(7.0));
}

#[test]
fn the_burndown_of_the_second_sprint_starts_with_the_carried_over_work() {
    let s = S::new();
    let (_, s2, s3) = three_sprints(&s);
    let b = burndown_for_sprint(s.c(), s2, Basis::Points, s.cal.clone(), d(2026, 11, 27)).unwrap();
    assert_eq!(b.scope_total_start, 7.0);
    near(&actual(&b)[..4], &[7.0, 5.0, 0.0, 0.0]);
    // Der dritte läuft: heute nach dem Start, Rest = 4.
    let b3 = burndown_for_sprint(s.c(), s3, Basis::Points, s.cal.clone(), d(2026, 11, 30)).unwrap();
    assert_eq!(b3.actual[0], Some(4.0));
    assert!(b3.actual[1].is_none());
}
