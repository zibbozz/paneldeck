//! Zeitzonen für den Kalender (#164): Abbildung lokaler Wanduhrzeit auf Zeitpunkte (UTC-Sekunden) mit den Regeln der
//! Sommer-/Winterzeit.
//!
//! * [`SystemZone`] ist die Zeitzone des Rechners (über `chrono::Local`, Name über `iana-time-zone`).
//! * [`RuleZone`] ist eine feste Zone mit Normal-Versatz und optionaler Sommerzeit-Regel (EU oder USA) – für Tests mit
//!   fester Uhr und fester Zeitzone und für die Zonen aus importierten `.ics`-Dateien.
//!
//! **Sommerzeit (Entscheidung im Issue):** gerechnet wird in lokaler Zeit. Eine Zeit in der Lücke (Uhr springt vor)
//! rückt auf die nächste gültige Zeit (das Ende der Lücke), die doppelte Stunde (Uhr springt zurück) zählt einmal – es
//! gilt der frühere Zeitpunkt.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike, Weekday};

/// Wie eine lokale Wanduhrzeit auf die Zeitachse fällt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTime {
    /// Eindeutig, mit Versatz zu UTC in Sekunden (lokal = UTC + Versatz).
    One(i32),
    /// Doppelt (Rückstellung): früherer und späterer Versatz.
    Two(i32, i32),
    /// In der Lücke (Vorstellung): erste gültige lokale Zeit danach und ihr Versatz.
    Gap(NaiveDateTime, i32),
}

/// Eine Zeitzone: lokale Zeit ↔ Zeitpunkt.
pub trait Zone {
    /// Lokale Wanduhrzeit einordnen.
    fn local(&self, t: NaiveDateTime) -> LocalTime;
    /// Versatz zu UTC (Sekunden) zum Zeitpunkt `utc` (Unix-Sekunden).
    fn offset_at(&self, utc: i64) -> i32;
    /// Name für die iCal-Ausgabe (`TZID`), z. B. `Europe/Berlin`.
    fn name(&self) -> String;
    /// Eigene Kopie für einen Hilfsthread (iCal-Import im Hintergrund).
    fn boxed(&self) -> Box<dyn Zone + Send>;

    /// Lokale Wanduhrzeit zum Zeitpunkt `utc`.
    fn to_local(&self, utc: i64) -> NaiveDateTime {
        naive_from_secs(utc + self.offset_at(utc) as i64)
    }

    /// Zeitpunkt (Unix-Sekunden) einer lokalen Zeit und die tatsächlich geltende lokale Zeit nach den Regeln oben: Lücke →
    /// Ende der Lücke, doppelte Stunde → früherer Zeitpunkt.
    fn resolve(&self, t: NaiveDateTime) -> (NaiveDateTime, i64) {
        match self.local(t) {
            LocalTime::One(off) => (t, secs(t) - off as i64),
            LocalTime::Two(early, _) => (t, secs(t) - early as i64),
            LocalTime::Gap(next, off) => (next, secs(next) - off as i64),
        }
    }

    /// Zeitpunkt einer lokalen Zeit (siehe [`Zone::resolve`]).
    fn instant(&self, t: NaiveDateTime) -> i64 {
        self.resolve(t).1
    }
}

/// Unix-Sekunden einer Wanduhrzeit, als wäre sie UTC.
pub fn secs(t: NaiveDateTime) -> i64 {
    t.and_utc().timestamp()
}

/// Umkehrung von [`secs`].
pub fn naive_from_secs(s: i64) -> NaiveDateTime {
    chrono::DateTime::from_timestamp(s, 0).map(|d| d.naive_utc()).unwrap_or_default()
}

/// Die Zeitzone des Rechners.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemZone;

impl Zone for SystemZone {
    fn boxed(&self) -> Box<dyn Zone + Send> {
        Box::new(SystemZone)
    }

    fn local(&self, t: NaiveDateTime) -> LocalTime {
        use chrono::offset::LocalResult;
        match chrono::Local.from_local_datetime(&t) {
            LocalResult::Single(d) => LocalTime::One(d.offset().local_minus_utc()),
            LocalResult::Ambiguous(a, b) => {
                let (x, y) = (a.offset().local_minus_utc(), b.offset().local_minus_utc());
                // früherer Zeitpunkt = größerer Versatz
                LocalTime::Two(x.max(y), x.min(y))
            }
            LocalResult::None => {
                // erste gültige Minute nach der Lücke (Lücken sind höchstens ein paar Stunden lang)
                let mut n = t.with_second(0).unwrap_or(t);
                for _ in 0..(26 * 60) {
                    n += Duration::minutes(1);
                    if let LocalResult::Single(d) = chrono::Local.from_local_datetime(&n) {
                        return LocalTime::Gap(n, d.offset().local_minus_utc());
                    }
                }
                LocalTime::One(0)
            }
        }
    }

    fn offset_at(&self, utc: i64) -> i32 {
        chrono::Local.timestamp_opt(utc, 0).single().map(|d| d.offset().local_minus_utc()).unwrap_or(0)
    }

    fn name(&self) -> String {
        iana_time_zone::get_timezone().ok().filter(|n| !n.trim().is_empty() && !n.contains(['\r', '\n', ';', ':', '"'])).unwrap_or_else(|| "UTC".into())
    }
}

/// Sommerzeit-Regel einer [`RuleZone`] (EU/USA für Tests und feste Zonen).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DstRule {
    /// Keine Sommerzeit.
    None,
    /// EU: letzter Sonntag im März 01:00 UTC bis letzter Sonntag im Oktober 01:00 UTC.
    Eu,
    /// USA: zweiter Sonntag im März 02:00 lokal bis erster Sonntag im November 02:00 lokal.
    Us,
    /// Allgemein (aus `VTIMEZONE`): Beginn und Ende als (Monat, n-ter Wochentag – negativ vom Monatsende –, Wochentag,
    /// lokale Uhrzeit in Sekunden vor dem Wechsel).
    Custom { start: Transition, end: Transition },
}

/// Ein jährlicher Wechsel: Monat, n-ter (1..5, -1 = letzter) Wochentag, Uhrzeit (lokal, Sekunden ab Mitternacht).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub month: u32,
    pub nth: i32,
    pub weekday: Weekday,
    pub at_secs: i32,
}

impl Transition {
    /// Der Tag dieses Wechsels im Jahr `year`.
    pub fn date(&self, year: i32) -> Option<NaiveDate> {
        nth_weekday(year, self.month, self.weekday, self.nth)
    }
}

/// `n`-ter Wochentag eines Monats (1..=5; -1 = letzter, -2 = vorletzter …); `None`, wenn es ihn nicht gibt.
pub fn nth_weekday(year: i32, month: u32, wd: Weekday, n: i32) -> Option<NaiveDate> {
    if !(1..=5).contains(&n.abs()) {
        return None;
    }
    if n > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let shift = (7 + wd.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64) % 7;
        let d = first + Duration::days(shift + 7 * (n as i64 - 1));
        (d.month() == month).then_some(d)
    } else if n < 0 {
        let last = last_day(year, month)?;
        let shift = (7 + last.weekday().num_days_from_monday() as i64 - wd.num_days_from_monday() as i64) % 7;
        let d = last - Duration::days(shift + 7 * (-n as i64 - 1));
        (d.month() == month).then_some(d)
    } else {
        None
    }
}

/// Letzter Tag eines Monats.
pub fn last_day(year: i32, month: u32) -> Option<NaiveDate> {
    let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).map(|d| d - Duration::days(1))
}

/// Feste Zone mit Normalversatz `std` (Sekunden), Sommerzeit-Versatz `dst` und Regel – reine Rechnung, für Tests und
/// importierte Zeitzonen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleZone {
    pub name: String,
    pub std: i32,
    pub dst: i32,
    pub rule: DstRule,
}

#[allow(dead_code)]
impl RuleZone {
    /// Mitteleuropa (CET/CEST).
    pub fn berlin() -> RuleZone {
        RuleZone { name: "Europe/Berlin".into(), std: 3600, dst: 7200, rule: DstRule::Eu }
    }
    /// New York (EST/EDT).
    pub fn new_york() -> RuleZone {
        RuleZone { name: "America/New_York".into(), std: -5 * 3600, dst: -4 * 3600, rule: DstRule::Us }
    }
    /// UTC.
    pub fn utc() -> RuleZone {
        RuleZone { name: "UTC".into(), std: 0, dst: 0, rule: DstRule::None }
    }
    /// Feste Zone ohne Sommerzeit.
    pub fn fixed(name: &str, offset: i32) -> RuleZone {
        RuleZone { name: name.into(), std: offset, dst: offset, rule: DstRule::None }
    }

    /// Beginn und Ende der Sommerzeit im Jahr `year` als UTC-Sekunden.
    fn dst_window(&self, year: i32) -> Option<(i64, i64)> {
        // lokale Uhrzeit `s` (Sekunden ab Mitternacht) am Tag `d` mit Versatz `off` als UTC-Sekunden
        let local = |d: Option<NaiveDate>, s: i32, off: i32| -> Option<i64> { Some(secs(d?.and_hms_opt(0, 0, 0)?) + s as i64 - off as i64) };
        match self.rule {
            DstRule::None => None,
            DstRule::Eu => {
                let s = nth_weekday(year, 3, Weekday::Sun, -1)?;
                let e = nth_weekday(year, 10, Weekday::Sun, -1)?;
                Some((secs(s.and_hms_opt(1, 0, 0)?), secs(e.and_hms_opt(1, 0, 0)?)))
            }
            DstRule::Us => {
                let s = nth_weekday(year, 3, Weekday::Sun, 2);
                let e = nth_weekday(year, 11, Weekday::Sun, 1);
                Some((local(s, 2 * 3600, self.std)?, local(e, 2 * 3600, self.dst)?))
            }
            DstRule::Custom { start, end } => Some((local(start.date(year), start.at_secs, self.std)?, local(end.date(year), end.at_secs, self.dst)?)),
        }
    }

    fn is_dst_utc(&self, utc: i64) -> bool {
        if self.dst == self.std {
            return false;
        }
        let year = naive_from_secs(utc + self.std as i64).year();
        match self.dst_window(year) {
            // Nordhalbkugel: Sommerzeit zwischen Beginn und Ende; Südhalbkugel: außerhalb
            Some((s, e)) if s <= e => utc >= s && utc < e,
            Some((s, e)) => utc >= s || utc < e,
            None => false,
        }
    }
}

impl Zone for RuleZone {
    fn boxed(&self) -> Box<dyn Zone + Send> {
        Box::new(self.clone())
    }

    fn local(&self, t: NaiveDateTime) -> LocalTime {
        // Kandidaten: als Normalzeit und als Sommerzeit gelesen; gültig ist, was zum Versatz zu diesem Zeitpunkt passt.
        let fits = |t: NaiveDateTime| {
            let base = secs(t);
            (self.offset_at(base - self.std as i64) == self.std, self.offset_at(base - self.dst as i64) == self.dst)
        };
        match fits(t) {
            (true, true) if self.std != self.dst => LocalTime::Two(self.dst.max(self.std), self.dst.min(self.std)),
            (true, _) => LocalTime::One(self.std),
            (_, true) => LocalTime::One(self.dst),
            (false, false) => {
                // Lücke: erste gültige Minute danach (ohne Rekursion)
                let mut n = t.with_second(0).unwrap_or(t);
                for _ in 0..(26 * 60) {
                    n += Duration::minutes(1);
                    match fits(n) {
                        (true, _) => return LocalTime::Gap(n, self.std),
                        (_, true) => return LocalTime::Gap(n, self.dst),
                        _ => {}
                    }
                }
                LocalTime::One(self.std)
            }
        }
    }

    fn offset_at(&self, utc: i64) -> i32 {
        if self.is_dst_utc(utc) { self.dst } else { self.std }
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

/// Die Wechsel einer Zone in einem Jahr (UTC-Sekunden, Versatz davor, Versatz danach), aufsteigend – gefunden durch
/// stundenweises Abtasten und Eingrenzen auf die Minute. Grundlage für das `VTIMEZONE` der iCal-Ausgabe.
pub fn transitions(zone: &dyn Zone, year: i32) -> Vec<(i64, i32, i32)> {
    let Some(start) = NaiveDate::from_ymd_opt(year, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)) else { return vec![] };
    let mut t = secs(start) - 14 * 3600;
    let end = t + 367 * 86400;
    let mut out = Vec::new();
    let mut prev = zone.offset_at(t);
    while t < end {
        let next = t + 3600;
        let off = zone.offset_at(next);
        if off != prev {
            // auf die Minute eingrenzen
            let (mut lo, mut hi) = (t, next);
            while hi - lo > 60 {
                let mid = lo + (hi - lo) / 2 / 60 * 60;
                let mid = if mid <= lo { lo + 60 } else { mid };
                if zone.offset_at(mid) == prev { lo = mid } else { hi = mid }
            }
            if zone.to_local(hi).year() == year {
                out.push((hi, prev, off));
            }
            prev = off;
        }
        t = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn nth_weekday_counts_from_both_ends() {
        assert_eq!(nth_weekday(2026, 3, Weekday::Sun, -1), NaiveDate::from_ymd_opt(2026, 3, 29));
        assert_eq!(nth_weekday(2026, 10, Weekday::Sun, -1), NaiveDate::from_ymd_opt(2026, 10, 25));
        assert_eq!(nth_weekday(2026, 11, Weekday::Mon, 1), NaiveDate::from_ymd_opt(2026, 11, 2));
        assert_eq!(nth_weekday(2026, 11, Weekday::Mon, 5), NaiveDate::from_ymd_opt(2026, 11, 30));
        assert_eq!(nth_weekday(2026, 2, Weekday::Mon, 5), None, "Februar 2026 hat nur vier Montage");
        assert_eq!(last_day(2028, 2), NaiveDate::from_ymd_opt(2028, 2, 29));
    }

    #[test]
    fn berlin_gap_and_repeated_hour() {
        let z = RuleZone::berlin();
        // 29.03.2026: 02:00 → 03:00
        assert_eq!(z.local(dt("2026-03-29 01:59")), LocalTime::One(3600));
        assert_eq!(z.local(dt("2026-03-29 02:30")), LocalTime::Gap(dt("2026-03-29 03:00"), 7200));
        assert_eq!(z.local(dt("2026-03-29 03:00")), LocalTime::One(7200));
        assert_eq!(z.resolve(dt("2026-03-29 02:30")).0, dt("2026-03-29 03:00"));
        // 25.10.2026: 03:00 → 02:00, 02:30 doppelt
        assert_eq!(z.local(dt("2026-10-25 02:30")), LocalTime::Two(7200, 3600));
        let (_, first) = z.resolve(dt("2026-10-25 02:30"));
        assert_eq!(first, secs(dt("2026-10-25 00:30")), "früherer Zeitpunkt = 00:30 UTC");
        assert_eq!(z.local(dt("2026-10-25 03:00")), LocalTime::One(3600));
        assert_eq!(z.to_local(secs(dt("2026-10-25 01:30"))), dt("2026-10-25 02:30"), "01:30 UTC ist die zweite 02:30");
    }

    #[test]
    fn new_york_rules() {
        let z = RuleZone::new_york();
        // 08.03.2026: 02:00 → 03:00; 01.11.2026: 02:00 → 01:00
        assert!(matches!(z.local(dt("2026-03-08 02:15")), LocalTime::Gap(t, _) if t == dt("2026-03-08 03:00")));
        assert_eq!(z.local(dt("2026-11-01 01:30")), LocalTime::Two(-4 * 3600, -5 * 3600));
        assert_eq!(z.local(dt("2026-07-01 12:00")), LocalTime::One(-4 * 3600));
        assert_eq!(z.local(dt("2026-12-01 12:00")), LocalTime::One(-5 * 3600));
    }

    #[test]
    fn transitions_are_found_to_the_minute() {
        let tr = transitions(&RuleZone::berlin(), 2026);
        assert_eq!(tr.len(), 2);
        assert_eq!(tr[0], (secs(dt("2026-03-29 01:00")), 3600, 7200));
        assert_eq!(tr[1], (secs(dt("2026-10-25 01:00")), 7200, 3600));
        assert!(transitions(&RuleZone::utc(), 2026).is_empty());
    }

    #[test]
    fn system_zone_answers_consistently() {
        let z = SystemZone;
        let t = dt("2026-06-15 12:00");
        let (_, inst) = z.resolve(t);
        assert_eq!(z.to_local(inst), t);
        assert!(!z.name().is_empty());
    }
}
