//! Kleine Hilfen: Gewichtung von Einträgen (`Basis`), Fortschritt eines übergeordneten Eintrags aus seinen Kindern,
//! Summen von Zeiteinträgen.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use super::duration::Minutes;

/// Womit ein Eintrag zählt, wenn Fortschritt oder Restumfang berechnet wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Basis {
    /// Nur die Punkte; ein Eintrag ohne Punkte zählt 0.
    Points,
    /// Jeder Eintrag zählt 1, Punkte werden ignoriert.
    TicketCount,
    /// Punkte; ein Eintrag **ohne** Punkte (`None`) zählt 1. Ausdrücklich gesetzte `Some(0.0)` zählen 0. (Standard.)
    #[default]
    PointsWithFallbackOne,
}

impl Basis {
    /// Gewicht eines Eintrags. Ungültige Punkte (negativ, NaN, unendlich) gelten als „keine Punkte“.
    pub fn weight(self, points: Option<f64>) -> f64 {
        let points = points.filter(|p| p.is_finite() && *p >= 0.0);
        match self {
            Basis::Points => points.unwrap_or(0.0),
            Basis::TicketCount => 1.0,
            Basis::PointsWithFallbackOne => points.unwrap_or(1.0),
        }
    }
}

/// Fortschritt eines übergeordneten Eintrags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// Gewicht der erledigten Kinder.
    pub done: f64,
    /// Gewicht aller Kinder.
    pub total: f64,
    /// `done / total` zwischen 0 und 1; 0, wenn `total` 0 ist (keine Kinder oder nur Kinder ohne Gewicht).
    pub fraction: f64,
}

impl Progress {
    /// Ganze Prozent (gerundet) für die Anzeige.
    pub fn percent(&self) -> u32 {
        (self.fraction * 100.0).round() as u32
    }
}

/// Fortschritt aus den Kindern: je Kind `(erledigt, Punkte)`. Gewichtung nach `basis`.
pub fn parent_progress(children: &[(bool, Option<f64>)], basis: Basis) -> Progress {
    let mut done = 0.0;
    let mut total = 0.0;
    for (is_done, points) in children {
        let w = basis.weight(*points);
        total += w;
        if *is_done {
            done += w;
        }
    }
    let fraction = if total > 0.0 { (done / total).clamp(0.0, 1.0) } else { 0.0 };
    Progress { done, total, fraction }
}

/// Summe von Zeiteinträgen (in Minuten); läuft nie über, sondern bleibt bei `u32::MAX`.
pub fn sum_minutes(entries: &[Minutes]) -> Minutes {
    entries.iter().fold(0, |acc: Minutes, m| acc.saturating_add(*m))
}

/// Zeiteinträge `(Tag, Minuten)` je Tag summieren, aufsteigend nach Datum.
pub fn sum_by_day(entries: &[(NaiveDate, Minutes)]) -> BTreeMap<NaiveDate, Minutes> {
    let mut map: BTreeMap<NaiveDate, Minutes> = BTreeMap::new();
    for (day, m) in entries {
        let slot = map.entry(*day).or_insert(0);
        *slot = slot.saturating_add(*m);
    }
    map
}

/// Restschätzung: Schätzung minus bereits gebuchte Zeit, nie unter 0. `None`, wenn es keine Schätzung gibt.
pub fn remaining_minutes(estimate: Option<Minutes>, logged: Minutes) -> Option<Minutes> {
    estimate.map(|e| e.saturating_sub(logged))
}

/// Um wie viele Minuten die gebuchte Zeit die Schätzung übersteigt (0, wenn nicht oder keine Schätzung).
pub fn overrun_minutes(estimate: Option<Minutes>, logged: Minutes) -> Minutes {
    estimate.map(|e| logged.saturating_sub(e)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn default_basis_is_points_with_fallback() {
        assert_eq!(Basis::default(), Basis::PointsWithFallbackOne);
    }

    #[test]
    fn weights_per_basis() {
        assert_eq!(Basis::Points.weight(Some(3.0)), 3.0);
        assert_eq!(Basis::Points.weight(None), 0.0);
        assert_eq!(Basis::TicketCount.weight(Some(3.0)), 1.0);
        assert_eq!(Basis::TicketCount.weight(None), 1.0);
        assert_eq!(Basis::PointsWithFallbackOne.weight(Some(3.0)), 3.0);
        assert_eq!(Basis::PointsWithFallbackOne.weight(None), 1.0);
    }

    #[test]
    fn explicit_zero_points_stay_zero_even_with_fallback() {
        assert_eq!(Basis::PointsWithFallbackOne.weight(Some(0.0)), 0.0);
    }

    #[test]
    fn invalid_points_count_as_missing() {
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(Basis::Points.weight(Some(bad)), 0.0);
            assert_eq!(Basis::PointsWithFallbackOne.weight(Some(bad)), 1.0);
            assert_eq!(Basis::TicketCount.weight(Some(bad)), 1.0);
        }
    }

    #[test]
    fn fractional_points() {
        assert_eq!(Basis::Points.weight(Some(0.5)), 0.5);
    }

    #[test]
    fn progress_by_points() {
        let children = [(true, Some(3.0)), (false, Some(5.0)), (true, Some(2.0))];
        let p = parent_progress(&children, Basis::Points);
        assert_eq!((p.done, p.total), (5.0, 10.0));
        assert!(approx(p.fraction, 0.5));
        assert_eq!(p.percent(), 50);
    }

    #[test]
    fn progress_by_count_ignores_points() {
        let children = [(true, Some(100.0)), (false, Some(1.0)), (false, None), (false, None)];
        let p = parent_progress(&children, Basis::TicketCount);
        assert_eq!((p.done, p.total), (1.0, 4.0));
        assert!(approx(p.fraction, 0.25));
        assert_eq!(p.percent(), 25);
    }

    #[test]
    fn progress_with_fallback() {
        let children = [(true, None), (false, Some(3.0))];
        let p = parent_progress(&children, Basis::PointsWithFallbackOne);
        assert_eq!((p.done, p.total), (1.0, 4.0));
        assert!(approx(p.fraction, 0.25));
    }

    #[test]
    fn progress_points_basis_without_points_is_zero() {
        let children = [(true, None), (true, None)];
        let p = parent_progress(&children, Basis::Points);
        assert_eq!((p.done, p.total, p.fraction), (0.0, 0.0, 0.0));
    }

    #[test]
    fn progress_without_children_is_zero_not_nan() {
        let p = parent_progress(&[], Basis::default());
        assert_eq!((p.done, p.total, p.fraction), (0.0, 0.0, 0.0));
        assert_eq!(p.percent(), 0);
    }

    #[test]
    fn progress_all_done_and_none_done() {
        let all = parent_progress(&[(true, Some(1.0)), (true, Some(2.0))], Basis::Points);
        assert_eq!(all.fraction, 1.0);
        assert_eq!(all.percent(), 100);
        let none = parent_progress(&[(false, Some(1.0)), (false, Some(2.0))], Basis::Points);
        assert_eq!(none.fraction, 0.0);
    }

    #[test]
    fn percent_rounds() {
        let p = parent_progress(&[(true, Some(1.0)), (false, Some(2.0))], Basis::Points);
        assert_eq!(p.percent(), 33);
        let p = parent_progress(&[(true, Some(2.0)), (false, Some(1.0))], Basis::Points);
        assert_eq!(p.percent(), 67);
    }

    #[test]
    fn summing_minutes() {
        assert_eq!(sum_minutes(&[]), 0);
        assert_eq!(sum_minutes(&[30, 45, 15]), 90);
        assert_eq!(sum_minutes(&[u32::MAX, 5]), u32::MAX);
    }

    #[test]
    fn summing_per_day() {
        let entries = [(d(2026, 11, 3), 60), (d(2026, 11, 2), 30), (d(2026, 11, 3), 15)];
        let m = sum_by_day(&entries);
        assert_eq!(m.len(), 2);
        assert_eq!(m[&d(2026, 11, 2)], 30);
        assert_eq!(m[&d(2026, 11, 3)], 75);
        // aufsteigend
        assert_eq!(m.keys().next(), Some(&d(2026, 11, 2)));
        assert!(sum_by_day(&[]).is_empty());
        let big = sum_by_day(&[(d(2026, 11, 2), u32::MAX), (d(2026, 11, 2), 1)]);
        assert_eq!(big[&d(2026, 11, 2)], u32::MAX);
    }

    #[test]
    fn remaining_and_overrun() {
        assert_eq!(remaining_minutes(Some(120), 45), Some(75));
        assert_eq!(remaining_minutes(Some(120), 120), Some(0));
        assert_eq!(remaining_minutes(Some(120), 200), Some(0));
        assert_eq!(remaining_minutes(None, 50), None);
        assert_eq!(overrun_minutes(Some(120), 200), 80);
        assert_eq!(overrun_minutes(Some(120), 100), 0);
        assert_eq!(overrun_minutes(None, 100), 0);
    }
}
