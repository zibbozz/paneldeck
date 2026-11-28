//! Cron-Ausdrücke für Wiederholungen im Kalender (#164) – eigener Parser und Auswerter (keine Crate deckt `#`, die
//! Kurzformen und die Sommerzeit-Regeln des Issues zusammen ab).
//!
//! **Umfang:** die fünf Standardfelder Minute (0–59), Stunde (0–23), Tag (1–31), Monat (1–12 oder `JAN`–`DEC`),
//! Wochentag (0–7 oder `SUN`–`SAT`, 0 und 7 = Sonntag) mit `*`, Listen (`1,15`), Bereichen (`1-5`), Schritten (`*/15`,
//! `0-30/10`, `5/15`); die Kurzformen `@yearly`/`@annually`, `@monthly`, `@weekly`, `@daily`/`@midnight`, `@hourly`; und
//! die Erweiterung `#` für den n-ten Wochentag im Monat (`MON#1` = erster Montag, 1–5).
//!
//! **Tag und Wochentag** (wie Vixie-Cron/cronie): Sind beide eingeschränkt (beginnen nicht mit `*`), reicht es, wenn
//! **eines** passt (ODER-Fall, [`Cron::or_case`] – der Baukasten warnt davor und erzeugt ihn nie). Sonst müssen beide passen.
//!
//! **Auswertung** in lokaler Wanduhrzeit (Minutenraster): [`Cron::next_after`] liefert die nächste passende Wanduhrzeit;
//! die Sommerzeit-Regeln (Lücke → nächste gültige Zeit, doppelte Stunde einmal) wendet [`Cron::next_in`] mit einer
//! [`Zone`] an.

use super::zone::Zone;
use crate::state::Lang;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// Längster Ausdruck (Zeichen).
pub const MAX_LEN: usize = 200;
/// So viele Tage sucht [`Cron::next_after`] höchstens voraus (gut 28 Jahre: so selten kommt z. B. `0 0 * 2 MON#5`).
pub const MAX_SEARCH_DAYS: i64 = 366 * 29;

/// Fehler beim Lesen eines Ausdrucks (zweisprachig).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronError {
    pub de: String,
    pub en: String,
}

impl CronError {
    fn new(de: impl Into<String>, en: impl Into<String>) -> CronError {
        CronError { de: de.into(), en: en.into() }
    }
    pub fn message(&self, lang: Lang) -> &str {
        match lang {
            Lang::De => &self.de,
            Lang::En => &self.en,
        }
    }
}

impl std::fmt::Display for CronError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.en)
    }
}

/// Ein gelesener Cron-Ausdruck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    /// Der Ausdruck in Normalform (Kurzformen bleiben stehen, Leerraum vereinheitlicht).
    pub text: String,
    minutes: u64,
    hours: u32,
    /// Bits 1..=31.
    dom: u32,
    /// Bits 1..=12.
    months: u16,
    /// Bits 0..=6 (Sonntag = 0).
    dow: u8,
    /// `#`-Einträge: (Wochentag 0..=6, n 1..=5).
    nth: Vec<(u8, u8)>,
    dom_star: bool,
    dow_star: bool,
}

const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
const DAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Minute,
    Hour,
    Dom,
    Month,
    Dow,
}

impl Field {
    fn range(self) -> (u32, u32) {
        match self {
            Field::Minute => (0, 59),
            Field::Hour => (0, 23),
            Field::Dom => (1, 31),
            Field::Month => (1, 12),
            Field::Dow => (0, 7),
        }
    }
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Field::Minute => ("Minute", "minute"),
            Field::Hour => ("Stunde", "hour"),
            Field::Dom => ("Tag", "day of month"),
            Field::Month => ("Monat", "month"),
            Field::Dow => ("Wochentag", "day of week"),
        }
    }
    fn err(self, de: &str, en: &str, item: &str) -> CronError {
        let (fd, fe) = self.names();
        CronError::new(format!("Feld {fd}: {de} („{item}“)"), format!("{fe} field: {en} (“{item}”)"))
    }
}

fn value(field: Field, s: &str) -> Result<u32, CronError> {
    let up = s.to_ascii_uppercase();
    let named = match field {
        Field::Month => MONTHS.iter().position(|m| *m == up).map(|i| i as u32 + 1),
        Field::Dow => DAYS.iter().position(|d| *d == up).map(|i| i as u32),
        _ => None,
    };
    let v = match named {
        Some(v) => v,
        None if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.len() <= 3 => s.parse::<u32>().map_err(|_| field.err("keine Zahl", "not a number", s))?,
        None => return Err(field.err("unbekannter Wert", "unknown value", s)),
    };
    let (lo, hi) = field.range();
    if v < lo || v > hi {
        return Err(field.err(&format!("erlaubt sind {lo}–{hi}"), &format!("allowed are {lo}-{hi}"), s));
    }
    Ok(v)
}

/// Bits eines Feldes, dazu (bei Wochentagen) die `#`-Einträge.
fn parse_field(field: Field, text: &str) -> Result<(u64, Vec<(u8, u8)>), CronError> {
    let (lo, hi) = field.range();
    let mut bits = 0u64;
    let mut nth = Vec::new();
    if text.is_empty() {
        return Err(field.err("leer", "empty", text));
    }
    for item in text.split(',') {
        if item.is_empty() {
            return Err(field.err("leerer Listeneintrag", "empty list item", text));
        }
        if let Some((day, n)) = item.split_once('#') {
            if field != Field::Dow {
                return Err(field.err("„#“ gibt es nur beim Wochentag", "“#” only works in the day-of-week field", item));
            }
            let d = value(field, day)? % 7;
            let n: u32 = n.parse().map_err(|_| field.err("nach „#“ steht 1–5", "“#” must be followed by 1-5", item))?;
            if !(1..=5).contains(&n) {
                return Err(field.err("nach „#“ steht 1–5", "“#” must be followed by 1-5", item));
            }
            if !nth.contains(&(d as u8, n as u8)) {
                nth.push((d as u8, n as u8));
            }
            continue;
        }
        let (range, step) = match item.split_once('/') {
            Some((r, s)) => {
                let st: u32 = s.parse().map_err(|_| field.err("Schritt ist keine Zahl", "step is not a number", item))?;
                if st == 0 || st > hi {
                    return Err(field.err("ungültiger Schritt", "invalid step", item));
                }
                (r, Some(st))
            }
            None => (item, None),
        };
        let (a, b) = if range == "*" {
            (lo, if field == Field::Dow { 6 } else { hi })
        } else if let Some((x, y)) = range.split_once('-') {
            let (x, y) = (value(field, x)?, value(field, y)?);
            if x > y {
                return Err(field.err("Bereich rückwärts", "range goes backwards", item));
            }
            (x, y)
        } else {
            let x = value(field, range)?;
            // „5/15“ = ab 5 bis zum Ende in Schritten
            (x, if step.is_some() { if field == Field::Dow { 6 } else { hi } } else { x })
        };
        let st = step.unwrap_or(1);
        let mut v = a;
        while v <= b {
            let bit = if field == Field::Dow { v % 7 } else { v };
            bits |= 1u64 << bit;
            v += st;
        }
    }
    Ok((bits, nth))
}

impl Cron {
    /// Liest einen Ausdruck (fünf Felder oder Kurzform).
    pub fn parse(expr: &str) -> Result<Cron, CronError> {
        let t = expr.trim();
        if t.is_empty() {
            return Err(CronError::new("Der Ausdruck ist leer.", "The expression is empty."));
        }
        if t.chars().count() > MAX_LEN {
            return Err(CronError::new(format!("Der Ausdruck ist länger als {MAX_LEN} Zeichen."), format!("The expression is longer than {MAX_LEN} characters.")));
        }
        let expanded = match t.to_ascii_lowercase().as_str() {
            "@yearly" | "@annually" => Some("0 0 1 1 *"),
            "@monthly" => Some("0 0 1 * *"),
            "@weekly" => Some("0 0 * * 0"),
            "@daily" | "@midnight" => Some("0 0 * * *"),
            "@hourly" => Some("0 * * * *"),
            s if s.starts_with('@') => {
                return Err(CronError::new(
                    format!("Unbekannte Kurzform „{t}“ (@yearly, @monthly, @weekly, @daily, @hourly)."),
                    format!("Unknown shortcut “{t}” (@yearly, @monthly, @weekly, @daily, @hourly)."),
                ))
            }
            _ => None,
        };
        let fields: Vec<&str> = expanded.unwrap_or(t).split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError::new(
                format!("Ein Cron-Ausdruck hat 5 Felder (Minute Stunde Tag Monat Wochentag), hier sind es {}.", fields.len()),
                format!("A cron expression has 5 fields (minute hour day month weekday), this one has {}.", fields.len()),
            ));
        }
        let (minutes, _) = parse_field(Field::Minute, fields[0])?;
        let (hours, _) = parse_field(Field::Hour, fields[1])?;
        let (dom, _) = parse_field(Field::Dom, fields[2])?;
        let (months, _) = parse_field(Field::Month, fields[3])?;
        let (dow, nth) = parse_field(Field::Dow, fields[4])?;
        let text = if expanded.is_some() { t.to_ascii_lowercase() } else { fields.join(" ") };
        Ok(Cron {
            text,
            minutes,
            hours: hours as u32,
            dom: dom as u32,
            months: months as u16,
            dow: dow as u8,
            nth,
            dom_star: fields[2].starts_with('*'),
            dow_star: fields[4].starts_with('*'),
        })
    }

    /// ODER-Fall: Tag und Wochentag sind beide eingeschränkt – es reicht, wenn einer passt (häufige Überraschung).
    pub fn or_case(&self) -> bool {
        !self.dom_star && !self.dow_star
    }

    /// Warnung zum ODER-Fall in der Sprache `lang` (sonst `None`).
    pub fn warning(&self, lang: Lang) -> Option<&'static str> {
        self.or_case().then(|| {
            lang.t(
                "Tag und Wochentag sind beide gesetzt: Cron wiederholt dann an jedem Tag, an dem EINES von beiden passt (z. B. „13 * FRI“ = jeder 13. und jeder Freitag).",
                "Day of month and day of week are both set: cron then repeats on every day where EITHER matches (e.g. “13 * FRI” = every 13th and every Friday).",
            )
        })
    }

    /// Mehr als ein Vorkommen an einem passenden Tag (mehrere Minuten oder Stunden)?
    pub fn several_per_day(&self) -> bool {
        self.minutes.count_ones() * self.hours.count_ones() > 1
    }

    /// Passt der Tag (Monat, Tag, Wochentag)?
    pub fn day_matches(&self, d: NaiveDate) -> bool {
        if self.months & (1 << d.month()) == 0 {
            return false;
        }
        let dom_ok = self.dom & (1 << d.day()) != 0;
        let wd = d.weekday().num_days_from_sunday() as u8;
        let n = ((d.day() - 1) / 7 + 1) as u8;
        let dow_ok = self.dow & (1 << wd) != 0 || self.nth.contains(&(wd, n));
        if self.dom_star || self.dow_star { dom_ok && dow_ok } else { dom_ok || dow_ok }
    }

    /// Passt die Wanduhrzeit (Sekunden werden ignoriert)? (Prüfhilfe der Tests.)
    #[cfg(test)]
    pub fn matches(&self, t: NaiveDateTime) -> bool {
        self.day_matches(t.date()) && self.hours & (1 << t.hour()) != 0 && self.minutes & (1 << t.minute()) != 0
    }

    /// Erste passende Minute des Tages ab Minute `from` (0..1440).
    fn first_time_from(&self, from: u32) -> Option<u32> {
        let (h0, m0) = (from / 60, from % 60);
        for h in h0..24 {
            if self.hours & (1 << h) == 0 {
                continue;
            }
            let start = if h == h0 { m0 } else { 0 };
            let mask = (self.minutes >> start) << start;
            if mask != 0 {
                return Some(h * 60 + mask.trailing_zeros());
            }
        }
        None
    }

    /// Nächste passende Wanduhrzeit **nach** `t` (ohne Zeitzone); `None`, wenn es in [`MAX_SEARCH_DAYS`] keine gibt
    /// (z. B. `0 0 30 2 *`).
    pub fn next_after(&self, t: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut date = t.date();
        let mut from = t.hour() * 60 + t.minute() + 1;
        for _ in 0..MAX_SEARCH_DAYS {
            if from < 1440 && self.day_matches(date)
                && let Some(m) = self.first_time_from(from) {
                    return date.and_hms_opt(m / 60, m % 60, 0);
                }
            date = date.succ_opt()?;
            from = 0;
        }
        None
    }

    /// Nächstes Vorkommen nach der Wanduhrzeit `after` in der Zone `zone`: eine Zeit in der Lücke rückt ans Ende der Lücke,
    /// die doppelte Stunde zählt einmal. Liefert (geltende Wanduhrzeit, Zeitpunkt). Fällt das verschobene Vorkommen auf
    /// `after` oder davor (mehrere Zeiten in derselben Lücke), wird es übersprungen.
    pub fn next_in(&self, after: NaiveDateTime, zone: &dyn Zone) -> Option<(NaiveDateTime, i64)> {
        let mut t = after;
        for _ in 0..2000 {
            let n = self.next_after(t)?;
            let (wall, inst) = zone.resolve(n);
            if wall > after {
                return Some((wall, inst));
            }
            t = n;
        }
        None
    }

    /// Die nächsten `n` Vorkommen ab (einschließlich) `from` in der Zone `zone` (Prüfhilfe; der Dialog rechnet über
    /// `Event::next_occurrences`, das auch Ende und Ausnahmen kennt).
    #[cfg(test)]
    pub fn preview(&self, from: NaiveDateTime, n: usize, zone: &dyn Zone) -> Vec<NaiveDateTime> {
        let mut out = Vec::with_capacity(n);
        let mut t = from - chrono::Duration::minutes(1);
        while out.len() < n {
            let Some((wall, _)) = self.next_in(t, zone) else { break };
            out.push(wall);
            t = wall;
        }
        out
    }

    // ---- Zugriff für die Übersetzung nach RRULE und den Baukasten

    /// Werte des Minutenfelds.
    pub fn minute_values(&self) -> Vec<u32> {
        (0..60).filter(|i| self.minutes & (1 << i) != 0).collect()
    }
    pub fn hour_values(&self) -> Vec<u32> {
        (0..24).filter(|i| self.hours & (1 << i) != 0).collect()
    }
    /// Tage (1..=31); bei `*` im Feld alle.
    pub fn dom_values(&self) -> Vec<u32> {
        (1..32).filter(|i| self.dom & (1 << i) != 0).collect()
    }
    pub fn month_values(&self) -> Vec<u32> {
        (1..13).filter(|i| self.months & (1 << i) != 0).collect()
    }
    /// Wochentage (0 = Sonntag) ohne die `#`-Einträge.
    pub fn dow_values(&self) -> Vec<u32> {
        (0..7).filter(|i| self.dow & (1 << i) != 0).collect()
    }
    /// `#`-Einträge (Wochentag 0 = Sonntag, n).
    pub fn nth_values(&self) -> &[(u8, u8)] {
        &self.nth
    }
    /// Feld Tag uneingeschränkt (`*` am Anfang)?
    pub fn dom_unrestricted(&self) -> bool {
        self.dom_star
    }
    pub fn dow_unrestricted(&self) -> bool {
        self.dow_star
    }
    /// Alle Monate?
    pub fn every_month(&self) -> bool {
        self.month_values().len() == 12
    }
    /// Tag-Feld effektiv ohne Wirkung (`*` mit allen Tagen)?
    pub fn every_dom(&self) -> bool {
        self.dom_star && self.dom_values().len() == 31
    }
    pub fn every_dow(&self) -> bool {
        self.dow_star && self.dow_values().len() == 7 && self.nth.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::zone::{RuleZone, SystemZone};
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }
    fn next(c: &str, from: &str) -> String {
        Cron::parse(c).unwrap().next_after(dt(from)).unwrap().format("%Y-%m-%d %H:%M").to_string()
    }
    fn prev5(c: &str, from: &str) -> Vec<String> {
        Cron::parse(c).unwrap().preview(dt(from), 5, &RuleZone::utc()).iter().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).collect()
    }

    #[test]
    fn fields_lists_ranges_steps_and_names() {
        assert_eq!(next("*/15 * * * *", "2026-11-22 10:07"), "2026-11-22 10:15");
        assert_eq!(next("*/15 * * * *", "2026-11-22 10:45"), "2026-11-22 11:00");
        assert_eq!(next("0 9 * * MON-FRI", "2026-11-21 08:00"), "2026-11-23 09:00", "Samstag → Montag");
        assert_eq!(next("30 8,12,18 * * *", "2026-11-22 12:30"), "2026-11-22 18:30", "strikt danach");
        assert_eq!(next("0 0-20/10 * * *", "2026-11-22 10:00"), "2026-11-22 20:00");
        assert_eq!(next("0 0-20/10 * * *", "2026-11-22 20:00"), "2026-11-23 00:00");
        assert_eq!(next("5/20 * * * *", "2026-11-22 10:06"), "2026-11-22 10:25");
        assert_eq!(next("0 12 1 jan,Jul *", "2026-02-01 00:00"), "2026-07-01 12:00", "Namen ohne Groß-/Kleinschreibung");
        assert_eq!(next("0 12 * * 7", "2026-11-22 13:00"), "2026-11-29 12:00", "7 = Sonntag");
        assert_eq!(next("0 12 * * 5-7", "2026-11-23 13:00"), "2026-11-27 12:00", "Fr–So");
        assert_eq!(next("0 0 * * SUN", "2026-11-22 00:00"), "2026-11-29 00:00");
        assert_eq!(Cron::parse("  0   9 *  * 1 ").unwrap().text, "0 9 * * 1", "Leerraum vereinheitlicht");
        let c = Cron::parse("0 9 * * MON-FRI").unwrap();
        assert!(c.matches(dt("2026-11-23 09:00")) && !c.matches(dt("2026-11-22 09:00")) && !c.matches(dt("2026-11-23 09:01")));
    }

    #[test]
    fn shortcuts() {
        assert_eq!(next("@daily", "2026-11-22 10:00"), "2026-11-23 00:00");
        assert_eq!(next("@midnight", "2026-11-22 10:00"), "2026-11-23 00:00");
        assert_eq!(next("@hourly", "2026-11-22 10:00"), "2026-11-22 11:00");
        assert_eq!(next("@weekly", "2026-11-22 10:00"), "2026-11-29 00:00");
        assert_eq!(next("@monthly", "2026-11-22 10:00"), "2026-12-01 00:00");
        assert_eq!(next("@yearly", "2026-11-22 10:00"), "2027-01-01 00:00");
        assert_eq!(next("@ANNUALLY", "2026-11-22 10:00"), "2027-01-01 00:00");
        assert!(Cron::parse("@reboot").is_err());
    }

    #[test]
    fn nth_weekday_extension() {
        assert_eq!(prev5("0 9 * * MON#1", "2026-11-22 00:00"), ["2026-12-07 09:00", "2027-01-04 09:00", "2027-02-01 09:00", "2027-03-01 09:00", "2027-04-05 09:00"]);
        assert_eq!(next("0 9 * * 5#3", "2026-11-01 00:00"), "2026-11-20 09:00", "dritter Freitag");
        assert_eq!(next("0 9 * * MON#5", "2026-11-01 00:00"), "2026-11-30 09:00");
        assert_eq!(next("0 9 * * MON#5", "2026-12-01 00:00"), "2027-03-29 09:00", "nur Monate mit fünf Montagen");
        assert_eq!(next("0 9 * * MON#1,FRI", "2026-11-28 00:00"), "2026-12-04 09:00", "Liste mit # und normalem Tag");
        assert_eq!(next("0 0 * 2 MON#5", "2026-01-01 00:00"), "2044-02-29 00:00", "fünfter Montag im Februar: selten");
        assert!(Cron::parse("0 9 MON#1 * *").is_err(), "# nur beim Wochentag");
        assert!(Cron::parse("0 9 * * MON#6").is_err());
        assert!(Cron::parse("0 9 * * MON#").is_err());
    }

    #[test]
    fn day_of_month_or_day_of_week() {
        let c = Cron::parse("0 0 13 * FRI").unwrap();
        assert!(c.or_case() && c.warning(Lang::En).is_some() && c.warning(Lang::De).is_some());
        // 13. November 2026 ist ein Freitag; davor: Fr 6.11.; danach: Fr 20.11.
        assert_eq!(next("0 0 13 * FRI", "2026-11-06 00:00"), "2026-11-13 00:00");
        assert_eq!(next("0 0 13 * FRI", "2026-11-13 00:00"), "2026-11-20 00:00");
        assert_eq!(next("0 0 13 * FRI", "2026-12-11 00:00"), "2026-12-13 00:00", "13. Dezember ist ein Sonntag – zählt trotzdem");
        // mit * am Anfang: UND
        let c = Cron::parse("0 0 */2 * MON").unwrap();
        assert!(!c.or_case());
        assert_eq!(next("0 0 */2 * MON", "2026-11-01 00:00"), "2026-11-09 00:00", "ungerader Tag UND Montag");
        assert!(!Cron::parse("0 0 13 * *").unwrap().or_case());
        assert!(!Cron::parse("0 0 * * FRI").unwrap().or_case());
        assert!(Cron::parse("0 0 1 * MON#1").unwrap().or_case(), "# zählt als eingeschränkt");
    }

    #[test]
    fn leap_years_and_impossible_dates() {
        assert_eq!(next("0 0 29 2 *", "2026-01-01 00:00"), "2028-02-29 00:00");
        assert_eq!(next("0 0 29 2 *", "2028-02-29 00:00"), "2032-02-29 00:00");
        assert_eq!(next("0 0 31 * *", "2026-11-01 00:00"), "2026-12-31 00:00", "November hat keinen 31.");
        assert!(Cron::parse("0 0 30 2 *").unwrap().next_after(dt("2026-01-01 00:00")).is_none());
        assert!(Cron::parse("0 0 31 4,6 *").unwrap().next_after(dt("2026-01-01 00:00")).is_none());
    }

    #[test]
    fn bad_expressions_are_explained() {
        for (e, part) in [
            ("", "empty"),
            ("* * * *", "5 fields"),
            ("60 * * * *", "0-59"),
            ("* 24 * * *", "0-23"),
            ("* * 0 * *", "1-31"),
            ("* * * 13 *", "1-12"),
            ("* * * * 8", "0-7"),
            ("*/0 * * * *", "step"),
            ("5-1 * * * *", "backwards"),
            ("a * * * *", "unknown"),
            ("1,,2 * * * *", "empty list"),
            ("@often", "shortcut"),
        ] {
            let err = Cron::parse(e).unwrap_err();
            assert!(err.en.contains(part), "{e}: {}", err.en);
            assert!(!err.de.is_empty() && err.de != err.en);
        }
        assert!(Cron::parse(&"1,".repeat(150)).is_err(), "zu lang");
    }

    #[test]
    fn dst_gap_moves_to_the_next_valid_time() {
        let z = RuleZone::berlin();
        let c = Cron::parse("30 2 * * *").unwrap();
        // 29.03.2026: 02:30 gibt es nicht → 03:00
        let (wall, inst) = c.next_in(dt("2026-03-28 12:00"), &z).unwrap();
        assert_eq!(wall, dt("2026-03-29 03:00"));
        assert_eq!(inst, super::super::zone::secs(dt("2026-03-29 01:00")));
        assert_eq!(c.next_in(wall, &z).unwrap().0, dt("2026-03-30 02:30"));
        // alle 15 Minuten: 02:00–02:45 fallen auf 03:00, das nur einmal kommt
        let c = Cron::parse("*/15 * * * *").unwrap();
        let p: Vec<String> = c.preview(dt("2026-03-29 01:45"), 4, &z).iter().map(|t| t.format("%H:%M").to_string()).collect();
        assert_eq!(p, ["01:45", "03:00", "03:15", "03:30"]);
    }

    #[test]
    fn dst_repeated_hour_fires_once() {
        let z = RuleZone::berlin();
        let c = Cron::parse("30 2 * * *").unwrap();
        let occ = c.preview(dt("2026-10-24 12:00"), 3, &z);
        assert_eq!(occ, [dt("2026-10-25 02:30"), dt("2026-10-26 02:30"), dt("2026-10-27 02:30")]);
        let (_, inst) = c.next_in(dt("2026-10-24 12:00"), &z).unwrap();
        assert_eq!(inst, super::super::zone::secs(dt("2026-10-25 00:30")), "die erste 02:30");
        let hourly = Cron::parse("0 * * * *").unwrap().preview(dt("2026-10-25 01:00"), 4, &z);
        assert_eq!(hourly.iter().map(|t| t.hour()).collect::<Vec<_>>(), [1, 2, 3, 4], "02:00 einmal");
        // USA: 01:30 doppelt am 01.11.2026
        let ny = RuleZone::new_york();
        let c = Cron::parse("30 1 * * *").unwrap();
        assert_eq!(c.preview(dt("2026-10-31 12:00"), 2, &ny), [dt("2026-11-01 01:30"), dt("2026-11-02 01:30")]);
    }

    #[test]
    fn preview_includes_the_start_and_works_in_the_system_zone() {
        assert_eq!(prev5("0 9 * * *", "2026-11-22 09:00")[0], "2026-11-22 09:00");
        assert_eq!(Cron::parse("0 9 * * *").unwrap().preview(dt("2026-06-01 00:00"), 5, &SystemZone).len(), 5);
    }
}
