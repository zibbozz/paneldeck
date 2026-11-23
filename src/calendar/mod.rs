//! Kalender (#164, 3.2.0) – Logik ohne egui: eigene Termine mit Wiederholung (Cron), Quellen aus der Planung
//! (Fälligkeiten, Sprints, Versionen/Meilensteine, gebuchte Zeit), iCal-Import/-Export, Erinnerungen und die
//! MCP-Werkzeuge. Die Oberfläche liegt in `app/calendar_ui/`.
//!
//! * [`zone`]: Zeitzonen und Sommerzeit-Regeln (Lücke → nächste gültige Zeit, doppelte Stunde einmal).
//! * [`cron`]: Cron-Ausdrücke (5 Felder, Kurzformen, `#`), [`builder`]: Baukasten für eindeutige Ausdrücke.
//! * [`store`]: Tabellen `cal_events`/`cal_settings` in `paneldeck.db` (Migration in `db.rs`), Optionen.
//! * [`sources`]: Einträge aus der Planung für einen Zeitraum.
//! * [`ics`]: iCal-Dateien (RRULE ↔ Cron). **Der iCal-Export ist öffentliche API.**
//! * [`remind`]: fällige Erinnerungen als reine Funktion.
//! * [`agent`]: MCP-Werkzeuge `list_calendar_events`, `create_calendar_event`, `update_calendar_event`.
//!
//! Alle Zeiten sind lokale Wanduhrzeit (`NaiveDateTime`); die Zeitzone kommt nur bei Wiederholungen (Sommerzeit),
//! Erinnerungen und iCal ins Spiel.

pub mod agent;
pub mod builder;
pub mod cron;
pub mod ics;
// Erinnerungen: verdrahtet der Benachrichtiger des Zeit-Reiters (bis dahin nur in Tests aufgerufen).
#[allow(dead_code)]
pub mod remind;
pub mod sources;
pub mod store;
pub mod zone;

#[cfg(test)]
mod tests;

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use cron::Cron;
use zone::Zone;

/// Längster Titel (Zeichen).
pub const MAX_TITLE: usize = 255;
/// Längste Notiz (Zeichen).
pub const MAX_NOTE: usize = 20_000;
/// Längste Dauer: 366 Tage in Minuten.
pub const MAX_MINUTES: i64 = 366 * 24 * 60;
/// Höchste Anzahl bei „endet nach N Mal“.
pub const MAX_COUNT: u32 = 10_000;
/// Höchstens so viele Ausnahmen je Termin.
pub const MAX_EXCEPTIONS: usize = 1000;
/// Längste Erinnerung: 4 Wochen vorher.
pub const MAX_REMINDER: i64 = 4 * 7 * 24 * 60;
/// Höchstens so viele Vorkommen eines Termins in einem Zeitraum (z. B. „jede Minute“ in der Monatsansicht).
pub const MAX_OCC_PER_RANGE: usize = 2000;
/// Erlaubte Jahre für alle Eingaben (Datumsrechnung bleibt damit weit weg von den Grenzen von `chrono`).
pub const MIN_YEAR: i32 = 1900;
pub const MAX_YEAR: i32 = 2200;

/// Liegt der Tag im erlaubten Bereich ([`MIN_YEAR`]..=[`MAX_YEAR`])?
pub fn year_ok(d: NaiveDate) -> bool {
    use chrono::Datelike;
    (MIN_YEAR..=MAX_YEAR).contains(&d.year())
}

/// `t + minutes`, an den Grenzen von `chrono` gesättigt (nie Panik).
pub fn add_minutes(t: NaiveDateTime, minutes: i64) -> NaiveDateTime {
    match Duration::try_minutes(minutes).and_then(|d| t.checked_add_signed(d)) {
        Some(x) => x,
        None if minutes < 0 => NaiveDateTime::MIN,
        None => NaiveDateTime::MAX,
    }
}

/// `d + days`, gesättigt (nie Panik).
pub fn add_days(d: NaiveDate, days: i64) -> NaiveDate {
    match Duration::try_days(days).and_then(|x| d.checked_add_signed(x)) {
        Some(x) => x,
        None if days < 0 => NaiveDate::MIN,
        None => NaiveDate::MAX,
    }
}

/// Wiederholung eines Termins.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recurrence {
    /// Cron-Ausdruck (siehe [`cron`]).
    pub cron: String,
    /// Letzter Tag (einschließlich), an dem ein Vorkommen beginnen darf.
    pub until: Option<NaiveDate>,
    /// Höchstens so viele Vorkommen ab dem Beginn (Ausnahmen zählen mit, wie bei iCal `COUNT`/`EXDATE`).
    pub count: Option<u32>,
    /// Ausgelassene Tage (das Vorkommen an diesem Tag fällt aus), aufsteigend ohne Doppelte.
    pub except: Vec<NaiveDate>,
}

/// Ein eigener Termin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: i64,
    /// Eindeutige Kennung (iCal `UID`), beim Anlegen erzeugt oder aus dem Import.
    pub uid: String,
    pub title: String,
    pub note: String,
    /// `#RRGGBB` oder leer (= Standardfarbe).
    pub color: String,
    /// Ganztägig: `start` ist 00:00, `minutes` ein Vielfaches von 1440 (mehrtägig ab 2 Tagen).
    pub all_day: bool,
    /// Beginn (lokale Wanduhrzeit); bei Wiederholungen der Beginn der Serie.
    pub start: NaiveDateTime,
    /// Dauer in Minuten (0 = Zeitpunkt).
    pub minutes: i64,
    pub recur: Option<Recurrence>,
    /// Erinnerung so viele Minuten vor dem Beginn (`None` = keine).
    pub reminder: Option<i64>,
    /// Zugehöriges Projekt (nur Anzeige/Filter).
    pub project_id: Option<i64>,
    /// Herkunft: `local`, `import`, `agent`.
    pub origin: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Event {
    /// Ende (ausschließlich) eines Vorkommens mit Beginn `start`.
    pub fn end_of(&self, start: NaiveDateTime) -> NaiveDateTime {
        add_minutes(start, self.minutes)
    }

    /// Ganztägige Tage (mindestens 1).
    pub fn days(&self) -> i64 {
        (self.minutes / 1440).max(1)
    }

    /// Vorkommen (Beginn, lokale Wanduhrzeit), die sich mit `[from, to)` überschneiden; Wiederholungen werden nur in
    /// diesem Zeitraum ausgerechnet. Ein ungültiger Cron-Ausdruck ergibt nur den Beginn. Höchstens
    /// [`MAX_OCC_PER_RANGE`] Vorkommen, die ab `from` beginnen (davor beginnende, hineinreichende zählen nicht mit).
    pub fn occurrences(&self, from: NaiveDateTime, to: NaiveDateTime, zone: &dyn Zone) -> Vec<NaiveDateTime> {
        let overlaps = |s: NaiveDateTime| {
            let e = self.end_of(s);
            s < to && (e > from || (self.minutes == 0 && s >= from))
        };
        let Some(rec) = &self.recur else {
            return if overlaps(self.start) { vec![self.start] } else { vec![] };
        };
        let Ok(cron) = Cron::parse(&rec.cron) else {
            return if overlaps(self.start) { vec![self.start] } else { vec![] };
        };
        let mut out = Vec::new();
        // Ohne Anzahl reicht es, kurz vor dem Zeitraum (minus Dauer) zu beginnen; mit Anzahl wird ab dem Beginn gezählt.
        let lower = if rec.count.is_some() { self.start } else { self.start.max(add_minutes(from, -self.minutes.max(0))) };
        let mut n: u32 = 0;
        let mut after_from = 0usize;
        let mut keep = |s: NaiveDateTime, out: &mut Vec<NaiveDateTime>| {
            if overlaps(s) && rec.except.binary_search(&s.date()).is_err() {
                out.push(s);
                if s >= from {
                    after_from += 1;
                }
            }
            after_from >= MAX_OCC_PER_RANGE
        };
        if self.all_day {
            let mut d = lower.date();
            let end = rec.until.map_or(to.date(), |u| u.min(to.date()));
            // mit Anzahl: vom Beginn an zählen
            if rec.count.is_some() {
                d = self.start.date();
            }
            while d <= end {
                if cron.day_matches(d) {
                    n += 1;
                    if rec.count.is_some_and(|c| n > c) {
                        break;
                    }
                    if keep(d.and_time(NaiveTime::MIN), &mut out) {
                        break;
                    }
                }
                let Some(nd) = d.succ_opt() else { break };
                d = nd;
            }
            return out;
        }
        let mut t = add_minutes(lower, -1);
        while let Some((wall, _)) = cron.next_in(t, zone) {
            if wall >= to || rec.until.is_some_and(|u| wall.date() > u) {
                break;
            }
            n += 1;
            if rec.count.is_some_and(|c| n > c) {
                break;
            }
            if keep(wall, &mut out) {
                break;
            }
            t = wall;
        }
        out
    }

    /// Die nächsten `n` Vorkommen ab (einschließlich) `from`.
    pub fn next_occurrences(&self, from: NaiveDateTime, n: usize, zone: &dyn Zone) -> Vec<NaiveDateTime> {
        if self.recur.is_none() {
            return if self.start >= from { vec![self.start] } else { vec![] };
        }
        // schrittweise größere Fenster, bis genug gefunden sind (höchstens gut 30 Jahre)
        let mut span = 31i64;
        loop {
            let to = add_minutes(from, span * 1440);
            let occ: Vec<NaiveDateTime> = self.occurrences(from, to, zone).into_iter().filter(|s| *s >= from).take(n).collect();
            if occ.len() >= n || span > 366 * 30 {
                return occ;
            }
            span *= 4;
        }
    }
}

/// Kurzform einer Uhrzeit für Listen.
pub fn fmt_time(t: NaiveDateTime) -> String {
    t.format("%H:%M").to_string()
}

/// Gespeicherte Form einer Wanduhrzeit (`JJJJ-MM-TTTHH:MM:SS`).
pub fn fmt_stored(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Liest `JJJJ-MM-TT`, `JJJJ-MM-TTTHH:MM` oder `JJJJ-MM-TTTHH:MM:SS` (auch mit Leerzeichen statt `T`). Liefert die Zeit
/// und ob nur ein Datum angegeben war. Nur Jahre [`MIN_YEAR`]..=[`MAX_YEAR`] (genau vier Ziffern).
pub fn parse_when(s: &str) -> Option<(NaiveDateTime, bool)> {
    let s = s.trim();
    if !four_digit_year(s) {
        return None;
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return year_ok(d).then(|| (d.and_time(NaiveTime::MIN), true));
    }
    for f in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(s, f) {
            return year_ok(t.date()).then_some((t, false));
        }
    }
    None
}

/// Beginnt der Text mit genau vier Ziffern und einem Bindestrich (kein `+262142-…`)?
fn four_digit_year(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 5 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-'
}

/// Liest ein Datum `JJJJ-MM-TT` (Jahre [`MIN_YEAR`]..=[`MAX_YEAR`]).
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    if !four_digit_year(s) {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().filter(|d| year_ok(*d))
}

/// Montag der Woche von `d`.
pub fn week_start(d: NaiveDate) -> NaiveDate {
    use chrono::Datelike;
    add_days(d, -(d.weekday().num_days_from_monday() as i64))
}
