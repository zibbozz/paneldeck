//! Burndown-Berechnung für einen Zeitraum (Sprint) und Velocity über mehrere Zeiträume.
//!
//! # Modell
//!
//! Ein Zeitraum ist [`SprintSpec`]: erster und letzter Tag (beide eingeschlossen) plus der Arbeitskalender.
//! Was darin passiert, kommt als Liste von [`ScopeEvent`]s (hinzugefügt, entfernt, erledigt, wieder geöffnet) mit Zeitpunkt und
//! optionalen Punkten. Wie ein Eintrag zählt, bestimmt die [`Basis`] (Standard: Punkte, ohne Punkte = 1).
//!
//! Der **Rest** ist die Summe der noch offenen Gewichte: `Added` und `Reopened` erhöhen ihn, `Removed` und `Completed` senken ihn.
//! Die Ereignisse haben bewusst keine Eintrags-Kennung – wer sie erzeugt, schickt `Removed` nur für noch offene Einträge
//! (ein bereits erledigter, dann entfernter Eintrag verändert den Rest nicht mehr). Der berechnete Rest wird nie negativ.
//!
//! * Ereignisse **vor dem ersten Tag** (Datum < `start`) zählen zum Startumfang ([`BurndownSeries::scope_total_start`]).
//! * Ereignisse **am oder nach dem ersten Tag** verändern den Ist-Verlauf; Umfangsänderungen werden nicht extra markiert,
//!   die Ist-Linie steigt oder fällt einfach. Ereignisse nach dem letzten Tag werden ignoriert.
//! * **Ist-Linie** ([`BurndownSeries::actual`]): Rest am **Ende** jedes Tages; Tage nach `today` sind `None`.
//! * **Ideal-Linie** ([`BurndownSeries::ideal`]): fällt gleichmäßig über die **Arbeitstage** vom Startrest auf 0; ebenfalls am Tagesende,
//!   an freien Tagen bleibt sie waagerecht. Am letzten Tag ist sie immer 0. (Gibt es keinen einzigen Arbeitstag, bleibt sie bis
//!   zum vorletzten Tag auf dem Startrest und fällt erst am letzten Tag auf 0.)
//! * Mehrere Ereignisse an einem Tag: nur der Stand am Tagesende zählt. Die Reihenfolge der Eingabe spielt keine Rolle.
//! * Ist `end < start`, sind alle Reihen leer und alle Summen 0.
//!
//! Die Zahlen werden auf 1e-9 gerundet, damit Fließkomma-Rauschen (`0.1 + 0.2 - 0.3`) nicht als 0.0000000001 im Diagramm landet.

use chrono::{NaiveDate, NaiveDateTime};

use super::calendar::{WorkCalendar, iter_days};
pub use super::progress::Basis;

/// Zeitraum samt Kalender. `start` und `end` sind beide eingeschlossen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SprintSpec {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub calendar: WorkCalendar,
}

/// Eine Änderung des Umfangs oder Fortschritts. `points` ist `None`, wenn der Eintrag keine Punkte hat
/// (er zählt dann je nach [`Basis`] 0 oder 1).
#[derive(Debug, Clone, PartialEq)]
pub enum ScopeEvent {
    /// Eintrag kommt zum Zeitraum dazu (Rest steigt).
    Added { at: NaiveDateTime, points: Option<f64> },
    /// Offener Eintrag verlässt den Zeitraum (Rest sinkt).
    Removed { at: NaiveDateTime, points: Option<f64> },
    /// Eintrag wird erledigt (Rest sinkt).
    Completed { at: NaiveDateTime, points: Option<f64> },
    /// Erledigter Eintrag wird wieder geöffnet (Rest steigt).
    Reopened { at: NaiveDateTime, points: Option<f64> },
}

impl ScopeEvent {
    pub fn at(&self) -> NaiveDateTime {
        match self {
            ScopeEvent::Added { at, .. } | ScopeEvent::Removed { at, .. } | ScopeEvent::Completed { at, .. } | ScopeEvent::Reopened { at, .. } => *at,
        }
    }

    pub fn points(&self) -> Option<f64> {
        match self {
            ScopeEvent::Added { points, .. }
            | ScopeEvent::Removed { points, .. }
            | ScopeEvent::Completed { points, .. }
            | ScopeEvent::Reopened { points, .. } => *points,
        }
    }

    /// Änderung des Rests durch dieses Ereignis.
    fn delta(&self, basis: Basis) -> f64 {
        let w = basis.weight(self.points());
        match self {
            ScopeEvent::Added { .. } | ScopeEvent::Reopened { .. } => w,
            ScopeEvent::Removed { .. } | ScopeEvent::Completed { .. } => -w,
        }
    }
}

/// Ergebnis von [`compute_burndown`]. `days`, `ideal` und `actual` sind gleich lang.
#[derive(Debug, Clone, PartialEq)]
pub struct BurndownSeries {
    /// Jeder Kalendertag von `start` bis `end` (auch freie Tage; die Diagrammachse entscheidet, ob sie sichtbar sind).
    pub days: Vec<NaiveDate>,
    /// Ideal-Rest am Ende jedes Tages.
    pub ideal: Vec<f64>,
    /// Ist-Rest am Ende jedes Tages; `None` für Tage nach `today`.
    pub actual: Vec<Option<f64>>,
    /// Offener Umfang zu Beginn des ersten Tages (alle Ereignisse davor); Startwert der Ideal-Linie.
    pub scope_total_start: f64,
    /// Summe der im Zeitraum (bis `today`) hinzugekommenen Gewichte.
    pub scope_added: f64,
    /// Summe der im Zeitraum (bis `today`) entfernten Gewichte.
    pub scope_removed: f64,
    /// Netto erledigt im Zeitraum bis `today`: erledigt minus wieder geöffnet, nie unter 0.
    pub completed: f64,
}

impl BurndownSeries {
    /// Für die Velocity: geplant = Startumfang, erledigt = netto erledigt.
    pub fn to_result(&self, label: impl Into<String>) -> SprintResult {
        SprintResult { label: label.into(), planned: self.scope_total_start, completed: self.completed }
    }
}

/// Rundet Rauschen weg und hält Werte ≥ 0.
fn clean(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let r = (v * 1e9).round() / 1e9;
    if r <= 0.0 { 0.0 } else { r }
}

/// Burndown berechnen. `today` ist der letzte Tag, für den es Ist-Werte gibt (Zeit kommt immer als Parameter).
pub fn compute_burndown(spec: &SprintSpec, events: &[ScopeEvent], basis: Basis, today: NaiveDate) -> BurndownSeries {
    if spec.end < spec.start {
        return BurndownSeries { days: vec![], ideal: vec![], actual: vec![], scope_total_start: 0.0, scope_added: 0.0, scope_removed: 0.0, completed: 0.0 };
    }
    let mut sorted: Vec<&ScopeEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.at()); // stabil: gleiche Zeitpunkte behalten ihre Reihenfolge

    // Alles vor dem ersten Tag ist Startumfang.
    let mut remaining = 0.0f64;
    let mut idx = 0;
    while idx < sorted.len() && sorted[idx].at().date() < spec.start {
        remaining += sorted[idx].delta(basis);
        idx += 1;
    }
    let scope_total_start = clean(remaining);

    let days: Vec<NaiveDate> = iter_days(spec.start, spec.end).collect();
    let total_work = days.iter().filter(|d| spec.calendar.is_work_day(**d)).count();

    let mut ideal = Vec::with_capacity(days.len());
    let mut actual = Vec::with_capacity(days.len());
    let (mut added, mut removed, mut done, mut reopened) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut work_so_far = 0usize;
    let last = days.len() - 1;
    for (i, day) in days.iter().enumerate() {
        if spec.calendar.is_work_day(*day) {
            work_so_far += 1;
        }
        let ideal_value = if i == last {
            0.0
        } else if total_work == 0 {
            scope_total_start
        } else {
            scope_total_start * (total_work - work_so_far) as f64 / total_work as f64
        };
        ideal.push(clean(ideal_value));

        if *day > today {
            actual.push(None);
            continue;
        }
        while idx < sorted.len() && sorted[idx].at().date() <= *day {
            let e = sorted[idx];
            remaining += e.delta(basis);
            let w = basis.weight(e.points());
            match e {
                ScopeEvent::Added { .. } => added += w,
                ScopeEvent::Removed { .. } => removed += w,
                ScopeEvent::Completed { .. } => done += w,
                ScopeEvent::Reopened { .. } => reopened += w,
            }
            idx += 1;
        }
        actual.push(Some(clean(remaining)));
    }
    BurndownSeries {
        days,
        ideal,
        actual,
        scope_total_start,
        scope_added: clean(added),
        scope_removed: clean(removed),
        completed: clean(done - reopened),
    }
}

/// Ergebnis eines abgeschlossenen (oder laufenden) Zeitraums für die Velocity.
#[derive(Debug, Clone, PartialEq)]
pub struct SprintResult {
    pub label: String,
    /// Geplanter Umfang zu Beginn.
    pub planned: f64,
    /// Tatsächlich erledigt.
    pub completed: f64,
}

/// Ein Punkt im Velocity-Diagramm.
#[derive(Debug, Clone, PartialEq)]
pub struct VelocityPoint {
    pub label: String,
    pub planned: f64,
    pub completed: f64,
    /// `completed / planned`; `None`, wenn nichts geplant war.
    pub completion_ratio: Option<f64>,
}

fn non_negative(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// Geplant gegen erledigt je Zeitraum, in der übergebenen Reihenfolge. Negative oder ungültige Zahlen zählen als 0.
pub fn velocity(sprints: &[SprintResult]) -> Vec<VelocityPoint> {
    sprints
        .iter()
        .map(|s| {
            let planned = non_negative(s.planned);
            let completed = non_negative(s.completed);
            VelocityPoint { label: s.label.clone(), planned, completed, completion_ratio: if planned > 0.0 { Some(completed / planned) } else { None } }
        })
        .collect()
}

/// Durchschnitt des Erledigten über die **letzten** `last_n` Zeiträume der Liste (die Liste ist von alt nach neu geordnet).
/// Gibt es weniger als `last_n`, zählen alle vorhandenen. `None` bei `last_n == 0` oder leerer Liste.
pub fn average_velocity(sprints: &[SprintResult], last_n: usize) -> Option<f64> {
    if last_n == 0 || sprints.is_empty() {
        return None;
    }
    let tail = &sprints[sprints.len().saturating_sub(last_n)..];
    let sum: f64 = tail.iter().map(|s| non_negative(s.completed)).sum();
    Some(clean(sum / tail.len() as f64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Weekday};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn at(y: i32, m: u32, day: u32, h: u32) -> NaiveDateTime {
        d(y, m, day).and_hms_opt(h, 0, 0).unwrap()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn assert_series(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!(approx(*a, *e), "{actual:?} vs {expected:?}");
        }
    }

    /// Zwei Wochen: Mo 2026-11-02 … Fr 2026-11-13 (14 Kalendertage, 10 Arbeitstage).
    fn two_weeks() -> SprintSpec {
        assert_eq!(d(2026, 11, 2).weekday(), Weekday::Mon);
        assert_eq!(d(2026, 11, 13).weekday(), Weekday::Fri);
        SprintSpec { start: d(2026, 11, 2), end: d(2026, 11, 13), calendar: WorkCalendar::default() }
    }

    fn added(day: u32, month: u32, h: u32, p: Option<f64>) -> ScopeEvent {
        ScopeEvent::Added { at: at(2026, month, day, h), points: p }
    }

    fn completed(month: u32, day: u32, h: u32, p: Option<f64>) -> ScopeEvent {
        ScopeEvent::Completed { at: at(2026, month, day, h), points: p }
    }

    fn actual_values(s: &BurndownSeries) -> Vec<Option<f64>> {
        s.actual.clone()
    }

    #[test]
    fn ideal_line_falls_over_work_days_only() {
        let events = [added(30, 10, 9, Some(10.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert_eq!(s.days.len(), 12);
        assert_eq!(s.days[0], d(2026, 11, 2));
        assert_eq!(s.days[11], d(2026, 11, 13));
        assert!(approx(s.scope_total_start, 10.0));
        // Mo..Fr der 1. Woche: 9 8 7 6 5, Sa/So waagerecht 5 5, dann 4 3 2 1 0.
        assert_series(&s.ideal, &[9.0, 8.0, 7.0, 6.0, 5.0, 5.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.0]);
    }

    #[test]
    fn actual_follows_completed_events() {
        let events = [added(30, 10, 9, Some(10.0)), completed(11, 3, 14, Some(2.0)), completed(11, 5, 9, Some(3.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        let expected = [10.0, 8.0, 8.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0];
        let got: Vec<f64> = s.actual.iter().map(|v| v.unwrap()).collect();
        assert_series(&got, &expected);
        assert!(approx(s.completed, 5.0));
    }

    #[test]
    fn future_days_are_none() {
        let events = [added(30, 10, 9, Some(10.0)), completed(11, 3, 14, Some(2.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 4)); // Mittwoch
        let v = actual_values(&s);
        assert_eq!(v.iter().filter(|x| x.is_some()).count(), 3);
        assert!(v[0].is_some() && v[1].is_some() && v[2].is_some());
        assert!(v[3..].iter().all(|x| x.is_none()));
        assert!(approx(v[2].unwrap(), 8.0));
        // Die Ideal-Linie gibt es trotzdem für alle Tage.
        assert_eq!(s.ideal.len(), 12);
    }

    #[test]
    fn today_before_start_gives_no_actual_values() {
        let events = [added(30, 10, 9, Some(10.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 1));
        assert!(s.actual.iter().all(|x| x.is_none()));
        assert!(approx(s.scope_total_start, 10.0));
    }

    #[test]
    fn today_after_end_fills_everything() {
        let events = [added(30, 10, 9, Some(4.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2027, 1, 1));
        assert!(s.actual.iter().all(|x| *x == Some(4.0)));
    }

    #[test]
    fn today_exactly_on_last_day_is_included() {
        let events = [added(30, 10, 9, Some(4.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(s.actual[11].is_some());
    }

    #[test]
    fn events_after_today_are_ignored() {
        let events = [added(30, 10, 9, Some(10.0)), completed(11, 10, 9, Some(10.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 4));
        assert!(approx(s.actual[2].unwrap(), 10.0));
        assert!(approx(s.completed, 0.0));
    }

    #[test]
    fn scope_added_during_the_period_raises_the_line() {
        let events = [added(30, 10, 9, Some(10.0)), added(4, 11, 10, Some(5.0)), completed(11, 3, 9, Some(2.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        // Mo 10, Di 8, Mi 13 (5 dazu), danach 13.
        assert_series(&[s.actual[0].unwrap(), s.actual[1].unwrap(), s.actual[2].unwrap(), s.actual[3].unwrap()], &[10.0, 8.0, 13.0, 13.0]);
        assert!(approx(s.scope_added, 5.0));
        // Die Ideal-Linie startet weiter beim Startumfang.
        assert!(approx(s.ideal[0], 9.0));
    }

    #[test]
    fn removed_scope_lowers_the_line() {
        let events = [added(30, 10, 9, Some(10.0)), ScopeEvent::Removed { at: at(2026, 11, 3, 9), points: Some(4.0) }];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.actual[0].unwrap(), 10.0));
        assert!(approx(s.actual[1].unwrap(), 6.0));
        assert!(approx(s.scope_removed, 4.0));
        assert!(approx(s.completed, 0.0));
    }

    #[test]
    fn reopening_raises_the_remainder_again() {
        let events = [
            added(30, 10, 9, Some(8.0)),
            completed(11, 3, 9, Some(3.0)),
            ScopeEvent::Reopened { at: at(2026, 11, 5, 9), points: Some(3.0) },
        ];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.actual[1].unwrap(), 5.0)); // Di
        assert!(approx(s.actual[2].unwrap(), 5.0)); // Mi
        assert!(approx(s.actual[3].unwrap(), 8.0)); // Do: wieder offen
        assert!(approx(s.completed, 0.0)); // netto nichts erledigt
    }

    #[test]
    fn complete_and_reopen_on_the_same_day_cancel_out() {
        let events = [
            added(30, 10, 9, Some(8.0)),
            completed(11, 3, 9, Some(3.0)),
            ScopeEvent::Reopened { at: at(2026, 11, 3, 15), points: Some(3.0) },
        ];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.actual[1].unwrap(), 8.0));
    }

    #[test]
    fn several_events_on_one_day_only_the_end_of_day_counts() {
        let events = [
            added(30, 10, 9, Some(10.0)),
            completed(11, 3, 9, Some(2.0)),
            completed(11, 3, 11, Some(1.0)),
            added(3, 11, 13, Some(4.0)),
            completed(11, 3, 17, Some(5.0)),
        ];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.actual[1].unwrap(), 10.0 - 2.0 - 1.0 + 4.0 - 5.0));
    }

    #[test]
    fn input_order_does_not_matter() {
        let mut events = vec![
            completed(11, 5, 9, Some(3.0)),
            added(30, 10, 9, Some(10.0)),
            completed(11, 3, 14, Some(2.0)),
            added(4, 11, 10, Some(1.0)),
        ];
        let a = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        events.reverse();
        let b = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert_eq!(a, b);
    }

    #[test]
    fn events_before_start_are_the_starting_scope() {
        let events = [
            added(1, 10, 9, Some(5.0)),
            added(20, 10, 9, Some(5.0)),
            ScopeEvent::Removed { at: at(2026, 10, 25, 9), points: Some(2.0) },
            completed(10, 28, 9, Some(1.0)),
            added(1, 11, 23, Some(3.0)), // Sonntag vor dem Start
        ];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.scope_total_start, 5.0 + 5.0 - 2.0 - 1.0 + 3.0));
        assert!(approx(s.actual[0].unwrap(), 10.0));
    }

    #[test]
    fn event_at_midnight_of_the_first_day_belongs_to_the_period() {
        let events = [ScopeEvent::Added { at: d(2026, 11, 2).and_hms_opt(0, 0, 0).unwrap(), points: Some(6.0) }];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.scope_total_start, 0.0));
        assert!(approx(s.actual[0].unwrap(), 6.0));
        assert!(approx(s.scope_added, 6.0));
    }

    #[test]
    fn event_at_the_very_end_of_the_last_day_counts() {
        let events = [
            added(30, 10, 9, Some(6.0)),
            ScopeEvent::Completed { at: d(2026, 11, 13).and_hms_opt(23, 59, 59).unwrap(), points: Some(6.0) },
        ];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert_eq!(s.actual[11], Some(0.0));
    }

    #[test]
    fn events_after_the_period_are_ignored() {
        let events = [added(30, 10, 9, Some(6.0)), completed(11, 14, 9, Some(6.0)), completed(12, 24, 9, Some(1.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2027, 1, 1));
        assert_eq!(s.actual[11], Some(6.0));
        assert!(approx(s.completed, 0.0));
    }

    #[test]
    fn empty_event_list_is_all_zero() {
        let s = compute_burndown(&two_weeks(), &[], Basis::default(), d(2026, 11, 13));
        assert_eq!(s.days.len(), 12);
        assert!(s.ideal.iter().all(|v| *v == 0.0));
        assert!(s.actual.iter().all(|v| *v == Some(0.0)));
        assert_eq!(s.scope_total_start, 0.0);
    }

    #[test]
    fn one_day_period() {
        let spec = SprintSpec { start: d(2026, 11, 2), end: d(2026, 11, 2), calendar: WorkCalendar::default() };
        let events = [added(1, 11, 9, Some(3.0)), completed(11, 2, 10, Some(1.0))];
        let s = compute_burndown(&spec, &events, Basis::Points, d(2026, 11, 2));
        assert_eq!(s.days, vec![d(2026, 11, 2)]);
        assert_eq!(s.ideal, vec![0.0]);
        assert_eq!(s.actual, vec![Some(2.0)]);
        assert!(approx(s.scope_total_start, 3.0));
    }

    #[test]
    fn one_day_period_on_a_free_day() {
        let spec = SprintSpec { start: d(2026, 11, 7), end: d(2026, 11, 7), calendar: WorkCalendar::default() };
        let s = compute_burndown(&spec, &[added(1, 11, 9, Some(3.0))], Basis::Points, d(2026, 11, 7));
        assert_eq!(s.ideal, vec![0.0]);
        assert_eq!(s.actual, vec![Some(3.0)]);
    }

    #[test]
    fn period_without_any_work_day_stays_flat_until_the_last_day() {
        // Sa–So
        let spec = SprintSpec { start: d(2026, 11, 7), end: d(2026, 11, 8), calendar: WorkCalendar::default() };
        let s = compute_burndown(&spec, &[added(1, 11, 9, Some(4.0))], Basis::Points, d(2026, 11, 8));
        assert_series(&s.ideal, &[4.0, 0.0]);
    }

    #[test]
    fn end_before_start_is_empty() {
        let spec = SprintSpec { start: d(2026, 11, 13), end: d(2026, 11, 2), calendar: WorkCalendar::default() };
        let s = compute_burndown(&spec, &[added(1, 11, 9, Some(4.0))], Basis::Points, d(2026, 11, 20));
        assert!(s.days.is_empty() && s.ideal.is_empty() && s.actual.is_empty());
        assert_eq!(s.scope_total_start, 0.0);
    }

    #[test]
    fn remainder_never_goes_negative() {
        let events = [completed(11, 3, 9, Some(5.0)), added(11, 4, 9, Some(2.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert_eq!(s.actual[1], Some(0.0));
        // Der Rohstand ist −5 + 2 = −3: auch danach 0, der Fehlbetrag wird nicht verdeckt „aufgefüllt“.
        assert_eq!(s.actual[2], Some(0.0));
        assert!(s.actual.iter().all(|v| v.unwrap() >= 0.0));
    }

    #[test]
    fn floating_point_noise_is_removed() {
        let events = [added(30, 10, 9, Some(0.1)), added(30, 10, 10, Some(0.2)), completed(11, 2, 9, Some(0.3))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert_eq!(s.actual[0], Some(0.0));
        assert!(approx(s.scope_total_start, 0.3));
    }

    #[test]
    fn basis_points_ignores_unpointed_items() {
        let events = [added(30, 10, 9, Some(5.0)), added(30, 10, 9, None), completed(11, 2, 9, None)];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.scope_total_start, 5.0));
        assert_eq!(s.actual[0], Some(5.0));
    }

    #[test]
    fn basis_ticket_count_counts_every_item_once() {
        let events = [added(30, 10, 9, Some(8.0)), added(30, 10, 9, None), added(30, 10, 9, Some(1.0)), completed(11, 2, 9, Some(8.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::TicketCount, d(2026, 11, 13));
        assert!(approx(s.scope_total_start, 3.0));
        assert_eq!(s.actual[0], Some(2.0));
    }

    #[test]
    fn basis_fallback_counts_missing_points_as_one() {
        let events = [added(30, 10, 9, Some(8.0)), added(30, 10, 9, None), added(30, 10, 9, Some(0.0)), completed(11, 2, 9, None)];
        let s = compute_burndown(&two_weeks(), &events, Basis::PointsWithFallbackOne, d(2026, 11, 13));
        assert!(approx(s.scope_total_start, 9.0));
        assert_eq!(s.actual[0], Some(8.0));
    }

    #[test]
    fn default_basis_is_fallback_one() {
        let events = [added(30, 10, 9, None)];
        let s = compute_burndown(&two_weeks(), &events, Basis::default(), d(2026, 11, 13));
        assert!(approx(s.scope_total_start, 1.0));
    }

    #[test]
    fn calendar_with_weekends_has_no_flat_stretch() {
        let spec = SprintSpec { start: d(2026, 11, 2), end: d(2026, 11, 8), calendar: WorkCalendar::every_day() };
        let s = compute_burndown(&spec, &[added(30, 10, 9, Some(7.0))], Basis::Points, d(2026, 11, 8));
        assert_series(&s.ideal, &[6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.0]);
    }

    #[test]
    fn holiday_makes_the_ideal_line_flat_that_day() {
        let cal = WorkCalendar::default().with_holidays(vec![d(2026, 11, 4)]);
        let spec = SprintSpec { start: d(2026, 11, 2), end: d(2026, 11, 6), calendar: cal };
        let s = compute_burndown(&spec, &[added(30, 10, 9, Some(8.0))], Basis::Points, d(2026, 11, 6));
        // 4 Arbeitstage: Mo Di (Mi frei) Do Fr -> 6 4 4 2 0
        assert_series(&s.ideal, &[6.0, 4.0, 4.0, 2.0, 0.0]);
    }

    #[test]
    fn period_starting_on_a_weekend_keeps_the_start_value_until_monday() {
        let spec = SprintSpec { start: d(2026, 11, 7), end: d(2026, 11, 11), calendar: WorkCalendar::default() };
        let s = compute_burndown(&spec, &[added(30, 10, 9, Some(3.0))], Basis::Points, d(2026, 11, 11));
        // Sa So Mo Di Mi: 3 Arbeitstage → 3 3 2 1 0
        assert_series(&s.ideal, &[3.0, 3.0, 2.0, 1.0, 0.0]);
    }

    #[test]
    fn period_ending_on_a_weekend_reaches_zero_on_the_last_day() {
        let spec = SprintSpec { start: d(2026, 11, 2), end: d(2026, 11, 8), calendar: WorkCalendar::default() };
        let s = compute_burndown(&spec, &[added(30, 10, 9, Some(5.0))], Basis::Points, d(2026, 11, 8));
        assert_series(&s.ideal, &[4.0, 3.0, 2.0, 1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn work_on_free_days_still_shows_in_actual() {
        let events = [added(30, 10, 9, Some(5.0)), completed(11, 7, 10, Some(2.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        assert!(approx(s.actual[4].unwrap(), 5.0)); // Fr
        assert!(approx(s.actual[5].unwrap(), 3.0)); // Sa
    }

    #[test]
    fn series_lengths_match() {
        let s = compute_burndown(&two_weeks(), &[], Basis::default(), d(2026, 11, 5));
        assert_eq!(s.days.len(), s.ideal.len());
        assert_eq!(s.days.len(), s.actual.len());
    }

    #[test]
    fn long_period_across_year_boundary() {
        let spec = SprintSpec { start: d(2026, 12, 21), end: d(2027, 1, 8), calendar: WorkCalendar::default().with_holidays(vec![d(2026, 12, 25), d(2027, 1, 1)]) };
        let s = compute_burndown(&spec, &[ScopeEvent::Added { at: at(2026, 12, 1, 9), points: Some(13.0) }], Basis::Points, d(2027, 1, 8));
        assert_eq!(s.days.len(), 19);
        // Arbeitstage: 3 Wochen minus 2 Feiertage = 15 − 2 = 13 → 13 Punkte fallen je Arbeitstag um 1.
        assert!(approx(s.ideal[0], 12.0));
        assert!(approx(*s.ideal.last().unwrap(), 0.0));
        assert!(approx(s.ideal[4], s.ideal[3])); // Fr 25.12. frei
    }

    #[test]
    fn to_result_feeds_velocity() {
        let events = [added(30, 10, 9, Some(10.0)), added(4, 11, 9, Some(2.0)), completed(11, 5, 9, Some(7.0))];
        let s = compute_burndown(&two_weeks(), &events, Basis::Points, d(2026, 11, 13));
        let r = s.to_result("Sprint 7");
        assert_eq!(r.label, "Sprint 7");
        assert!(approx(r.planned, 10.0));
        assert!(approx(r.completed, 7.0));
    }

    fn sr(label: &str, planned: f64, completed: f64) -> SprintResult {
        SprintResult { label: label.to_string(), planned, completed }
    }

    #[test]
    fn velocity_points() {
        let v = velocity(&[sr("A", 20.0, 15.0), sr("B", 10.0, 10.0), sr("C", 0.0, 3.0)]);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].label, "A");
        assert!(approx(v[0].completion_ratio.unwrap(), 0.75));
        assert!(approx(v[1].completion_ratio.unwrap(), 1.0));
        assert_eq!(v[2].completion_ratio, None);
        assert_eq!((v[2].planned, v[2].completed), (0.0, 3.0));
    }

    #[test]
    fn velocity_sanitizes_invalid_numbers() {
        let v = velocity(&[sr("A", -5.0, f64::NAN), sr("B", f64::INFINITY, 4.0)]);
        assert_eq!((v[0].planned, v[0].completed), (0.0, 0.0));
        assert_eq!((v[1].planned, v[1].completed), (0.0, 4.0));
        assert_eq!(v[1].completion_ratio, None);
    }

    #[test]
    fn velocity_of_nothing_is_empty() {
        assert!(velocity(&[]).is_empty());
    }

    #[test]
    fn overachieving_has_ratio_above_one() {
        let v = velocity(&[sr("A", 5.0, 10.0)]);
        assert!(approx(v[0].completion_ratio.unwrap(), 2.0));
    }

    #[test]
    fn average_of_the_last_n() {
        let list = [sr("1", 10.0, 2.0), sr("2", 10.0, 8.0), sr("3", 10.0, 10.0), sr("4", 10.0, 12.0)];
        assert!(approx(average_velocity(&list, 2).unwrap(), 11.0));
        assert!(approx(average_velocity(&list, 3).unwrap(), 10.0));
        assert!(approx(average_velocity(&list, 4).unwrap(), 8.0));
        assert!(approx(average_velocity(&list, 1).unwrap(), 12.0));
    }

    #[test]
    fn average_with_fewer_than_n_uses_all() {
        let list = [sr("1", 10.0, 4.0), sr("2", 10.0, 6.0)];
        assert!(approx(average_velocity(&list, 10).unwrap(), 5.0));
    }

    #[test]
    fn average_edge_cases() {
        assert_eq!(average_velocity(&[], 3), None);
        assert_eq!(average_velocity(&[sr("1", 1.0, 1.0)], 0), None);
        assert!(approx(average_velocity(&[sr("1", 1.0, f64::NAN), sr("2", 1.0, 4.0)], 2).unwrap(), 2.0));
    }

    #[test]
    fn scope_event_accessors() {
        let e = ScopeEvent::Reopened { at: at(2026, 11, 3, 9), points: Some(2.0) };
        assert_eq!(e.at(), at(2026, 11, 3, 9));
        assert_eq!(e.points(), Some(2.0));
    }
}
