//! Arbeitstage: welche Wochentage zählen, welche Feiertage fallen aus, wie viele Arbeitstage liegen zwischen zwei Daten.
//!
//! Alle Funktionen rechnen mit reinen Kalenderdaten ([`NaiveDate`], keine Zeitzone, keine Uhrzeit) und lesen nie die Systemuhr.
//! Die Laufzeit ist proportional zur Zahl der betrachteten Tage (es wird Tag für Tag gezählt – für Zeiträume von Jahren sofort).
//!
//! # Grenzen
//!
//! * [`WorkCalendar::work_days_between`]: `von` **eingeschlossen**, `bis` **ausgeschlossen** (halboffen `[von, bis)`), wie bei Rust-Bereichen.
//!   Montag bis Montag der Folgewoche sind 5 Arbeitstage.
//! * [`WorkCalendar::work_days_inclusive`]: beide Enden **eingeschlossen** `[von, bis]`. Montag bis Freitag sind 5 Arbeitstage.
//! * Ist `bis` vor `von` (bzw. bei „halboffen“ nicht nach `von`), ist das Ergebnis 0.
//! * [`iter_days`]: beide Enden eingeschlossen.
//!
//! # „Wochenenden ausschließen“
//!
//! Das ist ein Schalter des Aufrufers: [`WorkCalendar::with_weekend_switch`]`(true)` ergibt Montag bis Freitag (Standard),
//! `(false)` zählt alle sieben Wochentage als Arbeitstage ([`WorkCalendar::every_day`]). Feiertage gelten in beiden Fällen.

use chrono::{Datelike, NaiveDate};

/// Welche Tage zählen. `work_days[0]` ist Montag … `work_days[6]` ist Sonntag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkCalendar {
    pub work_days: [bool; 7],
    /// Einzelne freie Tage (auch auf Arbeitstagen); Reihenfolge und Doppelte egal.
    pub holidays: Vec<NaiveDate>,
}

impl Default for WorkCalendar {
    /// Montag bis Freitag, keine Feiertage.
    fn default() -> Self {
        WorkCalendar { work_days: [true, true, true, true, true, false, false], holidays: Vec::new() }
    }
}

impl WorkCalendar {
    /// Alle sieben Wochentage sind Arbeitstage (Wochenenden nicht ausgeschlossen).
    pub fn every_day() -> Self {
        WorkCalendar { work_days: [true; 7], holidays: Vec::new() }
    }

    /// Schalter „Wochenenden ausschließen“: `true` = Montag bis Freitag, `false` = alle Tage.
    pub fn with_weekend_switch(exclude_weekends: bool) -> Self {
        if exclude_weekends { Self::default() } else { Self::every_day() }
    }

    /// Feiertage setzen (sortiert, ohne Doppelte).
    pub fn with_holidays(mut self, mut holidays: Vec<NaiveDate>) -> Self {
        holidays.sort();
        holidays.dedup();
        self.holidays = holidays;
        self
    }

    /// Gibt es überhaupt einen Wochentag, der zählt?
    pub fn has_work_days(&self) -> bool {
        self.work_days.iter().any(|w| *w)
    }

    pub fn is_holiday(&self, day: NaiveDate) -> bool {
        self.holidays.contains(&day)
    }

    /// Zählt der Tag als Arbeitstag (Wochentag erlaubt und kein Feiertag)?
    pub fn is_work_day(&self, day: NaiveDate) -> bool {
        self.work_days[day.weekday().num_days_from_monday() as usize] && !self.is_holiday(day)
    }

    /// Arbeitstage in `[from, to)`: `from` eingeschlossen, `to` ausgeschlossen. 0, wenn `to <= from`.
    pub fn work_days_between(&self, from: NaiveDate, to: NaiveDate) -> u32 {
        from.iter_days().take_while(|d| *d < to).filter(|d| self.is_work_day(*d)).count() as u32
    }

    /// Arbeitstage in `[from, to]`: beide Enden eingeschlossen. 0, wenn `to < from`.
    pub fn work_days_inclusive(&self, from: NaiveDate, to: NaiveDate) -> u32 {
        iter_days(from, to).filter(|d| self.is_work_day(*d)).count() as u32
    }

    /// Alle Arbeitstage in `[from, to]` (beide eingeschlossen), aufsteigend.
    pub fn work_days_in(&self, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        iter_days(from, to).filter(|d| self.is_work_day(*d)).collect()
    }

    /// Um `n` Arbeitstage verschieben (negativ = zurück). Gezählt wird jeder Arbeitstag, auf dem man landet;
    /// der Starttag selbst zählt nie mit und darf auch ein freier Tag sein: Freitag + 1 = Montag, Samstag + 1 = Montag,
    /// Montag − 1 = Freitag. `n == 0` gibt `start` unverändert zurück. `None`, wenn es keine Arbeitstage gibt
    /// ([`has_work_days`](Self::has_work_days) ist falsch) oder der Datumsbereich verlassen würde.
    pub fn add_work_days(&self, start: NaiveDate, n: i64) -> Option<NaiveDate> {
        if n == 0 {
            return Some(start);
        }
        if !self.has_work_days() {
            return None;
        }
        let mut day = start;
        let mut left = n.unsigned_abs();
        while left > 0 {
            day = if n > 0 { day.succ_opt()? } else { day.pred_opt()? };
            if self.is_work_day(day) {
                left -= 1;
            }
        }
        Some(day)
    }

    /// Der erste Arbeitstag am oder nach `day` (ist `day` selbst einer, kommt `day` zurück).
    pub fn next_work_day_on_or_after(&self, day: NaiveDate) -> Option<NaiveDate> {
        if self.is_work_day(day) {
            return Some(day);
        }
        self.add_work_days(day, 1)
    }

    /// Der letzte Arbeitstag am oder vor `day`.
    pub fn prev_work_day_on_or_before(&self, day: NaiveDate) -> Option<NaiveDate> {
        if self.is_work_day(day) {
            return Some(day);
        }
        self.add_work_days(day, -1)
    }
}

/// Alle Tage von `from` bis `to`, **beide eingeschlossen**; leer, wenn `to < from`.
pub fn iter_days(from: NaiveDate, to: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    from.iter_days().take_while(move |d| *d <= to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // Referenzwoche: Mo 2026-11-02 … So 2026-11-08, Mo 2026-11-09 …
    fn mon() -> NaiveDate {
        let m = d(2026, 11, 2);
        assert_eq!(m.weekday(), Weekday::Mon);
        m
    }

    #[test]
    fn default_is_monday_to_friday() {
        let c = WorkCalendar::default();
        let days: Vec<bool> = (0..7).map(|i| c.is_work_day(mon() + chrono::TimeDelta::days(i))).collect();
        assert_eq!(days, [true, true, true, true, true, false, false]);
    }

    #[test]
    fn every_day_counts_weekends() {
        let c = WorkCalendar::every_day();
        assert!(c.is_work_day(d(2026, 11, 7)));
        assert!(c.is_work_day(d(2026, 11, 8)));
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 11, 8)), 7);
    }

    #[test]
    fn weekend_switch() {
        assert_eq!(WorkCalendar::with_weekend_switch(true), WorkCalendar::default());
        assert_eq!(WorkCalendar::with_weekend_switch(false), WorkCalendar::every_day());
    }

    #[test]
    fn custom_week_e_g_sunday_to_thursday() {
        let c = WorkCalendar { work_days: [true, true, true, true, false, false, true], holidays: vec![] };
        assert!(c.is_work_day(d(2026, 11, 8))); // Sonntag
        assert!(!c.is_work_day(d(2026, 11, 6))); // Freitag
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 11, 8)), 5);
    }

    #[test]
    fn holidays_remove_work_days() {
        let c = WorkCalendar::default().with_holidays(vec![d(2026, 11, 4)]);
        assert!(!c.is_work_day(d(2026, 11, 4)));
        assert!(c.is_holiday(d(2026, 11, 4)));
        assert!(c.is_work_day(d(2026, 11, 5)));
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 11, 6)), 4);
    }

    #[test]
    fn holiday_on_weekend_changes_nothing() {
        let c = WorkCalendar::default().with_holidays(vec![d(2026, 11, 7)]);
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 11, 8)), 5);
    }

    #[test]
    fn holidays_apply_to_every_day_calendar_too() {
        let c = WorkCalendar::every_day().with_holidays(vec![d(2026, 11, 8)]);
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 11, 8)), 6);
    }

    #[test]
    fn with_holidays_sorts_and_deduplicates() {
        let c = WorkCalendar::default().with_holidays(vec![d(2026, 12, 25), d(2026, 1, 1), d(2026, 12, 25)]);
        assert_eq!(c.holidays, vec![d(2026, 1, 1), d(2026, 12, 25)]);
    }

    #[test]
    fn between_is_half_open() {
        let c = WorkCalendar::default();
        // Mo bis Mo der Folgewoche: 5 Arbeitstage (Mo … Fr), der Endtag zählt nicht.
        assert_eq!(c.work_days_between(mon(), d(2026, 11, 9)), 5);
        // Mo bis Fr: Mo, Di, Mi, Do = 4.
        assert_eq!(c.work_days_between(mon(), d(2026, 11, 6)), 4);
        assert_eq!(c.work_days_between(mon(), mon()), 0);
        assert_eq!(c.work_days_between(mon(), d(2026, 11, 3)), 1);
    }

    #[test]
    fn inclusive_counts_both_ends() {
        let c = WorkCalendar::default();
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 11, 6)), 5);
        assert_eq!(c.work_days_inclusive(mon(), mon()), 1);
        assert_eq!(c.work_days_inclusive(d(2026, 11, 7), d(2026, 11, 7)), 0); // Samstag allein
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 11, 13)), 10);
    }

    #[test]
    fn reversed_ranges_are_zero() {
        let c = WorkCalendar::default();
        assert_eq!(c.work_days_between(d(2026, 11, 9), mon()), 0);
        assert_eq!(c.work_days_inclusive(d(2026, 11, 9), mon()), 0);
        assert!(c.work_days_in(d(2026, 11, 9), mon()).is_empty());
    }

    #[test]
    fn work_days_in_lists_dates() {
        let c = WorkCalendar::default();
        let v = c.work_days_in(d(2026, 11, 5), d(2026, 11, 10));
        assert_eq!(v, vec![d(2026, 11, 5), d(2026, 11, 6), d(2026, 11, 9), d(2026, 11, 10)]);
    }

    #[test]
    fn counts_across_year_boundary_and_leap_day() {
        let c = WorkCalendar::default();
        // 2027-12-31 ist ein Freitag, 2028-01-03 ein Montag.
        assert_eq!(c.work_days_inclusive(d(2027, 12, 31), d(2028, 1, 3)), 2);
        // 2028 ist ein Schaltjahr: 2028-02-28 (Mo) bis 2028-03-01 (Mi) = 3 Tage.
        assert_eq!(c.work_days_inclusive(d(2028, 2, 28), d(2028, 3, 1)), 3);
        assert_eq!(c.work_days_inclusive(d(2028, 2, 29), d(2028, 2, 29)), 1);
    }

    #[test]
    fn a_whole_year_has_about_260_work_days() {
        let c = WorkCalendar::default();
        assert_eq!(c.work_days_inclusive(d(2026, 1, 1), d(2026, 12, 31)), 261);
    }

    #[test]
    fn add_work_days_forward() {
        let c = WorkCalendar::default();
        assert_eq!(c.add_work_days(mon(), 1), Some(d(2026, 11, 3)));
        assert_eq!(c.add_work_days(mon(), 4), Some(d(2026, 11, 6)));
        assert_eq!(c.add_work_days(mon(), 5), Some(d(2026, 11, 9))); // über das Wochenende
        assert_eq!(c.add_work_days(d(2026, 11, 6), 1), Some(d(2026, 11, 9))); // Fr + 1 = Mo
        assert_eq!(c.add_work_days(mon(), 10), Some(d(2026, 11, 16)));
    }

    #[test]
    fn add_work_days_from_a_free_day() {
        let c = WorkCalendar::default();
        assert_eq!(c.add_work_days(d(2026, 11, 7), 1), Some(d(2026, 11, 9))); // Sa + 1 = Mo
        assert_eq!(c.add_work_days(d(2026, 11, 8), 1), Some(d(2026, 11, 9))); // So + 1 = Mo
        assert_eq!(c.add_work_days(d(2026, 11, 7), -1), Some(d(2026, 11, 6))); // Sa − 1 = Fr
    }

    #[test]
    fn add_work_days_backward() {
        let c = WorkCalendar::default();
        assert_eq!(c.add_work_days(d(2026, 11, 9), -1), Some(d(2026, 11, 6)));
        assert_eq!(c.add_work_days(d(2026, 11, 9), -5), Some(mon()));
        assert_eq!(c.add_work_days(mon(), -1), Some(d(2026, 10, 30)));
    }

    #[test]
    fn add_zero_returns_start_even_on_free_day() {
        let c = WorkCalendar::default();
        assert_eq!(c.add_work_days(d(2026, 11, 7), 0), Some(d(2026, 11, 7)));
    }

    #[test]
    fn add_work_days_skips_holidays() {
        let c = WorkCalendar::default().with_holidays(vec![d(2026, 11, 3), d(2026, 11, 4)]);
        assert_eq!(c.add_work_days(mon(), 1), Some(d(2026, 11, 5)));
        assert_eq!(c.add_work_days(d(2026, 11, 5), -1), Some(mon()));
    }

    #[test]
    fn add_and_count_are_consistent() {
        let c = WorkCalendar::default().with_holidays(vec![d(2026, 11, 3)]);
        for n in 1..40 {
            let end = c.add_work_days(mon(), n).unwrap();
            assert!(c.is_work_day(end));
            // Zwischen Start (ausgeschlossen) und Ende (eingeschlossen) liegen genau n Arbeitstage.
            assert_eq!(c.work_days_inclusive(mon() + chrono::TimeDelta::days(1), end), n as u32, "n = {n}");
            // Und zurück.
            assert_eq!(c.add_work_days(end, -n), c.prev_work_day_on_or_before(mon()));
        }
    }

    #[test]
    fn no_work_days_means_none() {
        let c = WorkCalendar { work_days: [false; 7], holidays: vec![] };
        assert!(!c.has_work_days());
        assert_eq!(c.add_work_days(mon(), 3), None);
        assert_eq!(c.next_work_day_on_or_after(mon()), None);
        assert_eq!(c.work_days_inclusive(mon(), d(2026, 12, 31)), 0);
        assert_eq!(c.add_work_days(mon(), 0), Some(mon()));
    }

    #[test]
    fn date_range_limits_give_none() {
        let c = WorkCalendar::every_day();
        assert_eq!(c.add_work_days(NaiveDate::MAX, 1), None);
        assert_eq!(c.add_work_days(NaiveDate::MIN, -1), None);
    }

    #[test]
    fn next_and_prev_work_day() {
        let c = WorkCalendar::default();
        assert_eq!(c.next_work_day_on_or_after(d(2026, 11, 7)), Some(d(2026, 11, 9)));
        assert_eq!(c.next_work_day_on_or_after(mon()), Some(mon()));
        assert_eq!(c.prev_work_day_on_or_before(d(2026, 11, 8)), Some(d(2026, 11, 6)));
        assert_eq!(c.prev_work_day_on_or_before(d(2026, 11, 6)), Some(d(2026, 11, 6)));
    }

    #[test]
    fn iter_days_is_inclusive() {
        let v: Vec<NaiveDate> = iter_days(d(2026, 11, 30), d(2026, 12, 2)).collect();
        assert_eq!(v, vec![d(2026, 11, 30), d(2026, 12, 1), d(2026, 12, 2)]);
        assert_eq!(iter_days(mon(), mon()).count(), 1);
        assert_eq!(iter_days(d(2026, 11, 3), mon()).count(), 0);
        assert_eq!(iter_days(d(2028, 2, 28), d(2028, 3, 1)).count(), 3);
    }

    #[test]
    fn iter_days_near_the_end_of_time_terminates() {
        // chrono liefert den allerletzten darstellbaren Tag nicht mehr aus; wichtig ist nur, dass es endet.
        assert!(iter_days(NaiveDate::MAX - chrono::TimeDelta::days(2), NaiveDate::MAX).count() >= 2);
    }
}
