//! Geburtstage (#164): eigene Liste mit Name und Datum, das Jahr ist optional.
//!
//! Jeder Eintrag zeigt das aktuelle Alter (nur mit Jahr) und die Zeit bis zum nächsten Geburtstag im Format
//! „<neues Alter> in <Tage> und <Monate>“ (Entscheidung zu #164): Monate entfallen, wenn keine mehr übrig sind; die Tage
//! sind der Rest nach vollen Monaten (nie mehr als 31). Ohne Jahr nur der Countdown („in 5 Tagen und 2 Monaten“), am Tag
//! selbst „Heute! 🎂“. Ein 29. Februar wird in Nicht-Schaltjahren am 28. Februar gefeiert. Eine Erinnerung je Eintrag ist
//! standardmäßig aus.

use crate::state::Lang;
use chrono::{Datelike, Months, NaiveDate};

pub const MAX_NAME_CHARS: usize = 100;
/// Erinnerungen kommen am Tag selbst ab dieser Stunde (lokal).
pub const REMIND_HOUR: u32 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Birthday {
    pub id: i64,
    pub name: String,
    pub month: u32,
    pub day: u32,
    pub year: Option<i32>,
    /// Erinnerung am Tag selbst (Standard aus).
    pub remind: bool,
    /// Jahr der letzten Erinnerung (damit sie nur einmal kommt).
    pub reminded_year: Option<i32>,
}

impl Birthday {
    pub fn new(name: &str, month: u32, day: u32, year: Option<i32>) -> Birthday {
        Birthday { id: 0, name: super::alarm::clean_name(name).chars().take(MAX_NAME_CHARS).collect(), month, day, year, remind: false, reminded_year: None }
    }

    /// Tag des Geburtstags im Jahr `y` (29. Februar → 28. Februar in Nicht-Schaltjahren).
    pub fn in_year(&self, y: i32) -> Option<NaiveDate> {
        in_year(self.month, self.day, y)
    }

    /// Nächster Geburtstag ab `today` (heute zählt).
    pub fn next(&self, today: NaiveDate) -> Option<NaiveDate> {
        let this = self.in_year(today.year())?;
        if this >= today { Some(this) } else { self.in_year(today.year() + 1) }
    }

    pub fn is_today(&self, today: NaiveDate) -> bool {
        self.in_year(today.year()) == Some(today)
    }

    /// Alter am Tag `on` (nur mit Jahr; vor der Geburt `None`).
    pub fn age_on(&self, on: NaiveDate) -> Option<i32> {
        let y = self.year?;
        let had = self.in_year(on.year()).is_some_and(|d| on >= d);
        let age = on.year() - y - if had { 0 } else { 1 };
        (age >= 0).then_some(age)
    }

    /// Alter am nächsten Geburtstag.
    pub fn next_age(&self, today: NaiveDate) -> Option<i32> {
        self.age_on(self.next(today)?)
    }

    /// Tage bis zum nächsten Geburtstag (0 = heute).
    pub fn days_until(&self, today: NaiveDate) -> Option<i64> {
        Some((self.next(today)? - today).num_days())
    }

    /// Text des Countdowns (siehe Moduldoku).
    pub fn describe(&self, today: NaiveDate, lang: Lang) -> String {
        let Some(next) = self.next(today) else { return String::new() };
        if next == today {
            return lang.t("Heute! 🎂", "Today! 🎂").into();
        }
        let (months, days) = countdown(today, next);
        let span = span_text(months, days, lang);
        match self.age_on(next) {
            Some(age) => format!("{age} {span}"),
            None => span,
        }
    }

    /// Datum als Text: `TT.MM.JJJJ` bzw. `TT.MM.` (Deutsch), `YYYY-MM-DD` bzw. `MM-DD` (Englisch).
    pub fn date_text(&self, lang: Lang) -> String {
        match (lang, self.year) {
            (Lang::De, Some(y)) => format!("{:02}.{:02}.{y}", self.day, self.month),
            (Lang::De, None) => format!("{:02}.{:02}.", self.day, self.month),
            (Lang::En, Some(y)) => format!("{y}-{:02}-{:02}", self.month, self.day),
            (Lang::En, None) => format!("{:02}-{:02}", self.month, self.day),
        }
    }

    /// Soll jetzt erinnert werden? (eingeschaltet, heute, ab [`REMIND_HOUR`], dieses Jahr noch nicht)
    pub fn wants_reminder(&self, today: NaiveDate, hour: u32) -> bool {
        self.remind && self.is_today(today) && hour >= REMIND_HOUR && self.reminded_year != Some(today.year())
    }
}

/// Tag im Jahr `y`; 29. Februar → 28. Februar in Nicht-Schaltjahren. Ungültiges Datum: `None`.
pub fn in_year(month: u32, day: u32, y: i32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(y, month, day).or_else(|| (month == 2 && day == 29).then(|| NaiveDate::from_ymd_opt(y, 2, 28)).flatten())
}

/// Gültig, wenn es den Tag (im Schaltjahr) gibt und das Jahr vierstellig ist.
pub fn valid(month: u32, day: u32, year: Option<i32>) -> bool {
    let y = year.unwrap_or(2000);
    (1000..=9999).contains(&y) && NaiveDate::from_ymd_opt(if year.is_some() { y } else { 2000 }, month, day).is_some()
}

/// Volle Monate und Resttage von `from` bis `to` (`to` ≥ `from`, weniger als ein Jahr). Höchstens 11 Monate: vom
/// 29.02. eines Schaltjahrs bis zum 28.02. des Folgejahrs sind es „30 Tage und 11 Monate“, nicht „12 Monate“.
pub fn countdown(from: NaiveDate, to: NaiveDate) -> (u32, u32) {
    let mut months = 0u32;
    while months < 11
        && let Some(d) = from.checked_add_months(Months::new(months + 1))
    {
        if d <= to {
            months += 1;
        } else {
            break;
        }
    }
    let base = from.checked_add_months(Months::new(months)).unwrap_or(from);
    (months, (to - base).num_days().max(0) as u32)
}

/// „in 5 Tagen und 2 Monaten“ / „in 5 days and 2 months“; Monate entfallen ohne volle Monate, Tage ohne Rest.
pub fn span_text(months: u32, days: u32, lang: Lang) -> String {
    let d = match (lang, days) {
        (Lang::De, 1) => "1 Tag".to_string(),
        (Lang::De, n) => format!("{n} Tagen"),
        (Lang::En, 1) => "1 day".to_string(),
        (Lang::En, n) => format!("{n} days"),
    };
    let m = match (lang, months) {
        (Lang::De, 1) => "1 Monat".to_string(),
        (Lang::De, n) => format!("{n} Monaten"),
        (Lang::En, 1) => "1 month".to_string(),
        (Lang::En, n) => format!("{n} months"),
    };
    let and = lang.t("und", "and");
    match (months, days) {
        (0, _) => format!("in {d}"),
        (_, 0) => format!("in {m}"),
        _ => format!("in {d} {and} {m}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn format_with_and_without_year() {
        let b = Birthday::new("Anna", 1, 20, Some(1990));
        let today = d(2026, 11, 15);
        // 15.11. → 20.01.: 2 volle Monate (15.01.), dann 5 Tage
        assert_eq!(b.describe(today, Lang::En), "37 in 5 days and 2 months");
        assert_eq!(b.describe(today, Lang::De), "37 in 5 Tagen und 2 Monaten");
        assert_eq!(b.age_on(today), Some(36));
        let no_year = Birthday::new("Ben", 1, 20, None);
        assert_eq!(no_year.describe(today, Lang::En), "in 5 days and 2 months");
        assert_eq!(no_year.age_on(today), None);
        // weniger als ein Monat: nur Tage; Einzahl
        assert_eq!(Birthday::new("", 11, 16, Some(2000)).describe(today, Lang::En), "26 in 1 day");
        assert_eq!(Birthday::new("", 11, 16, None).describe(today, Lang::De), "in 1 Tag");
        assert_eq!(Birthday::new("", 12, 20, None).describe(today, Lang::En), "in 5 days and 1 month");
        assert_eq!(Birthday::new("", 12, 20, None).describe(today, Lang::De), "in 5 Tagen und 1 Monat");
        // genau volle Monate: nur Monate
        assert_eq!(Birthday::new("", 1, 15, None).describe(today, Lang::En), "in 2 months");
        // am Tag selbst
        assert_eq!(Birthday::new("", 11, 15, Some(2000)).describe(today, Lang::En), "Today! 🎂");
        assert_eq!(Birthday::new("", 11, 15, None).describe(today, Lang::De), "Heute! 🎂");
        assert_eq!(Birthday::new("", 11, 15, Some(2000)).age_on(today), Some(26), "am Tag zählt das neue Alter");
        // gestern: fast ein Jahr
        let y = Birthday::new("", 11, 14, None);
        assert_eq!(y.days_until(today), Some(364));
        assert_eq!(y.describe(today, Lang::En), "in 30 days and 11 months");
    }

    #[test]
    fn remainder_days_never_exceed_31() {
        let start = d(2026, 1, 1);
        for off in 0..(366 * 3) {
            let today = start + chrono::Duration::days(off);
            for (m, day) in [(1, 31), (2, 29), (3, 31), (3, 1), (12, 31), (5, 30), (2, 28)] {
                let b = Birthday::new("", m, day, Some(1980));
                let next = b.next(today).unwrap();
                let (months, days) = countdown(today, next);
                assert!(days <= 31 && months <= 11, "{today} → {next}: {months} m {days} d");
                // Rückweg: Monate + Tage ergeben genau das Ziel
                assert_eq!(today.checked_add_months(Months::new(months)).unwrap() + chrono::Duration::days(days as i64), next);
            }
        }
    }

    #[test]
    fn leap_day_birthdays() {
        let b = Birthday::new("Leap", 2, 29, Some(2000));
        assert_eq!(b.in_year(2027), Some(d(2027, 2, 28)));
        assert_eq!(b.in_year(2028), Some(d(2028, 2, 29)));
        assert!(b.is_today(d(2027, 2, 28)));
        assert!(!b.is_today(d(2028, 2, 28)));
        assert_eq!(b.describe(d(2027, 2, 28), Lang::En), "Today! 🎂");
        assert_eq!(b.age_on(d(2027, 2, 27)), Some(26));
        assert_eq!(b.age_on(d(2027, 2, 28)), Some(27));
        assert_eq!(b.describe(d(2027, 2, 27), Lang::En), "27 in 1 day");
        // im Schaltjahr am 28.: noch ein Tag
        assert_eq!(b.describe(d(2028, 2, 28), Lang::En), "28 in 1 day");
        // vom Schalttag bis zum 28.02. des Folgejahrs: 11 Monate und 30 Tage, nie „12 Monate“
        assert_eq!(Birthday::new("", 2, 28, None).describe(d(2028, 2, 29), Lang::En), "in 30 days and 11 months");
        assert!(valid(2, 29, None) && valid(2, 29, Some(2000)) && !valid(2, 29, Some(2001)) && !valid(2, 30, None) && !valid(13, 1, None) && !valid(1, 1, Some(99)));
    }

    #[test]
    fn reminders_once_a_year_from_the_reminder_hour() {
        let mut b = Birthday::new("", 3, 4, None);
        let day = d(2026, 3, 4);
        assert!(!b.wants_reminder(day, 9), "standardmäßig aus");
        b.remind = true;
        assert!(!b.wants_reminder(day, 7));
        assert!(b.wants_reminder(day, 8));
        b.reminded_year = Some(2026);
        assert!(!b.wants_reminder(day, 12));
        assert!(!b.wants_reminder(d(2026, 3, 5), 12));
    }

    #[test]
    fn date_texts_and_birth_in_the_future() {
        let b = Birthday::new("", 7, 3, Some(1985));
        assert_eq!(b.date_text(Lang::De), "03.07.1985");
        assert_eq!(b.date_text(Lang::En), "1985-07-03");
        assert_eq!(Birthday::new("", 7, 3, None).date_text(Lang::De), "03.07.");
        let unborn = Birthday::new("", 7, 3, Some(2030));
        assert_eq!(unborn.age_on(d(2026, 1, 1)), None);
    }
}
