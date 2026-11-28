//! Wecker (#164): Uhrzeit + Wochentage (oder einmalig) + Name, Klang, Schlummern.
//!
//! Regeln:
//! * Ein Wecker gilt in **lokaler Zeit**. Fällt die Uhrzeit in die Lücke der Zeitumstellung (z. B. 02:30 am letzten
//!   Sonntag im März), klingelt er zur nächsten gültigen Zeit (03:00); in der doppelten Stunde im Oktober klingelt er
//!   nur einmal (beim ersten Durchlauf).
//! * `armed_from` (Unix-Sekunden): Termine **nach** diesem Zeitpunkt stehen noch aus. Beim Anlegen, Einschalten und Ändern
//!   wird er auf „jetzt“ gesetzt – eine Änderung auf eine frühere Uhrzeit löst also nichts nachträglich aus.
//! * [`Alarm::check`]: Alle Termine zwischen `armed_from` und jetzt sind fällig. Der letzte klingelt, wenn er höchstens
//!   `grace` Sekunden zurückliegt; alle anderen (und ein älterer letzter) gelten als **verpasst** (Programm beendet,
//!   Rechner im Ruhezustand). Ein einmaliger Wecker schaltet sich danach aus.
//! * Schlummern: `snooze_minutes` (Standard 5) höchstens `snooze_max`-mal (Standard 10) je Klingeln; ein neuer regulärer
//!   Termin setzt den Zähler zurück.

use crate::state::Lang;
use chrono::{Datelike, Duration, LocalResult, NaiveDateTime, NaiveTime, TimeZone, Timelike};

/// Bit je Wochentag: Montag = Bit 0 … Sonntag = Bit 6. `0` = einmalig.
pub const ALL_DAYS: u8 = 0x7f;
pub const WORKDAYS: u8 = 0x1f;
pub const WEEKEND: u8 = 0x60;

pub const DEFAULT_SNOOZE_MINUTES: u32 = 5;
pub const DEFAULT_SNOOZE_MAX: u32 = 10;
pub const SNOOZE_MINUTES_RANGE: (u32, u32) = (1, 60);
pub const SNOOZE_MAX_RANGE: (u32, u32) = (0, 30);
pub const DEFAULT_SOUND: &str = "builtin:chime";
/// Höchstens so viele verpasste Termine je Wecker werden gemeldet (die jüngsten).
pub const MAX_MISSED: usize = 10;
/// Liegt `armed_from` (bzw. ein Schlummern) weiter als so viele Sekunden in der Zukunft, wurde die Systemuhr
/// zurückgestellt: Der Wecker wird ab jetzt neu scharf gemacht, statt stunden- oder tagelang zu schweigen.
pub const CLOCK_BACK_TOLERANCE: i64 = 300;
pub const MAX_NAME_CHARS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alarm {
    pub id: i64,
    pub name: String,
    pub hour: u8,
    pub minute: u8,
    /// Wochentage als Bits (siehe [`ALL_DAYS`]); 0 = einmalig (nächstes Vorkommen der Uhrzeit).
    pub days: u8,
    pub enabled: bool,
    /// Klang: `builtin:<name>` oder `file:<pfad>` (siehe `sounds`).
    pub sound: String,
    pub snooze_minutes: u32,
    pub snooze_max: u32,
    /// Unix-Sekunden, siehe Moduldoku.
    pub armed_from: i64,
    pub snoozed_until: Option<i64>,
    pub snooze_count: u32,
}

impl Alarm {
    /// Neuer, eingeschalteter Wecker (scharf ab `now`).
    pub fn new(name: &str, hour: u8, minute: u8, days: u8, now: i64) -> Alarm {
        Alarm {
            id: 0,
            name: clean_name(name),
            hour: hour.min(23),
            minute: minute.min(59),
            days: days & ALL_DAYS,
            enabled: true,
            sound: DEFAULT_SOUND.into(),
            snooze_minutes: DEFAULT_SNOOZE_MINUTES,
            snooze_max: DEFAULT_SNOOZE_MAX,
            armed_from: now,
            snoozed_until: None,
            snooze_count: 0,
        }
    }

    pub fn is_once(&self) -> bool {
        self.days & ALL_DAYS == 0
    }

    pub fn time_text(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// Nächster Termin strikt nach `after` (Unix-Sekunden) in der Zone `tz`; `None`, wenn keiner in den nächsten acht Tagen
    /// liegt (kann bei gültigen Daten nicht vorkommen).
    pub fn next_after<Tz: TimeZone>(&self, after: i64, tz: &Tz) -> Option<i64> {
        let local = tz.timestamp_opt(after, 0).single().or_else(|| tz.timestamp_opt(after, 0).earliest())?.naive_local();
        let time = NaiveTime::from_hms_opt(self.hour.min(23) as u32, self.minute.min(59) as u32, 0)?;
        let mut date = local.date();
        for _ in 0..9 {
            let wd = date.weekday().num_days_from_monday();
            if (self.is_once() || self.days & (1 << wd) != 0)
                && let Some(t) = resolve_local(tz, date.and_time(time))
                    && t > after {
                        return Some(t);
                    }
            date = date.succ_opt()?;
        }
        None
    }

    /// Prüft den Wecker zum Zeitpunkt `now` (siehe Moduldoku). Ändert `armed_from`, Schlummern und – bei einmaligen
    /// Weckern – `enabled`; `Some` heißt: etwas ist passiert (der Wecker muss gespeichert werden).
    pub fn check<Tz: TimeZone>(&mut self, now: i64, tz: &Tz, grace: i64) -> Option<Due> {
        let fixed = self.fix_clock_back(now);
        let mut occ: Vec<i64> = Vec::new();
        if self.enabled {
            let mut t = self.armed_from;
            while let Some(n) = self.next_after(t, tz) {
                if n > now {
                    break;
                }
                occ.push(n);
                t = n;
                if self.is_once() || occ.len() >= 5000 {
                    break;
                }
            }
        }
        let snooze_due = self.snoozed_until.filter(|&s| s <= now);
        if occ.is_empty() && snooze_due.is_none() {
            return fixed.then(Due::default);
        }
        if !occ.is_empty() {
            self.armed_from = now;
        }
        let mut due = Due::default();
        if let Some(&last) = occ.last() {
            let n = occ.len();
            if now - last <= grace {
                due.ring = Some(last);
                due.missed = occ[..n - 1].to_vec();
            } else {
                due.missed = occ;
            }
            if due.missed.len() > MAX_MISSED {
                due.missed.drain(..due.missed.len() - MAX_MISSED);
            }
            if self.is_once() {
                self.enabled = false;
            }
        }
        if due.ring.is_some() {
            // ein neuer regulärer Termin ersetzt ein laufendes Schlummern
            self.snooze_count = 0;
            self.snoozed_until = None;
        } else if let Some(s) = snooze_due {
            self.snoozed_until = None;
            due.snoozed = true;
            if now - s <= grace {
                due.ring = Some(s);
            } else {
                // Schlummern lief ab, während Paneldeck zu war bzw. der Rechner schlief: als verpasst melden
                due.missed_snooze = Some(s);
            }
        }
        Some(due)
    }

    /// Systemuhr zurückgestellt (siehe [`CLOCK_BACK_TOLERANCE`]): `armed_from` auf jetzt, ein unplausibel fernes
    /// Schlummern auf jetzt + Schlummerdauer. `true`, wenn sich etwas geändert hat.
    pub fn fix_clock_back(&mut self, now: i64) -> bool {
        let mut changed = false;
        if self.armed_from > now + CLOCK_BACK_TOLERANCE {
            self.armed_from = now;
            changed = true;
        }
        let snooze = self.snooze_minutes.clamp(SNOOZE_MINUTES_RANGE.0, SNOOZE_MINUTES_RANGE.1) as i64 * 60;
        if let Some(s) = self.snoozed_until
            && s > now + snooze + CLOCK_BACK_TOLERANCE
        {
            self.snoozed_until = Some(now + snooze);
            changed = true;
        }
        changed
    }

    /// Schlummern ab `now`. `false`, wenn das Limit erreicht ist (dann bleibt alles, wie es ist).
    pub fn snooze(&mut self, now: i64) -> bool {
        if !self.can_snooze() {
            return false;
        }
        self.snooze_count += 1;
        self.snoozed_until = Some(now + self.snooze_minutes.clamp(SNOOZE_MINUTES_RANGE.0, SNOOZE_MINUTES_RANGE.1) as i64 * 60);
        true
    }

    pub fn can_snooze(&self) -> bool {
        self.snooze_count < self.snooze_max.min(SNOOZE_MAX_RANGE.1)
    }

    /// Klingeln beenden: kein Schlummern mehr, Zähler zurück.
    pub fn dismiss(&mut self) {
        self.snoozed_until = None;
        self.snooze_count = 0;
    }

    /// Ein- bzw. ausschalten; beim Einschalten ab `now` scharf.
    pub fn set_enabled(&mut self, on: bool, now: i64) {
        if on && !self.enabled {
            self.armed_from = now;
        }
        if !on {
            self.snoozed_until = None;
            self.snooze_count = 0;
        }
        self.enabled = on;
    }

    /// Uhrzeit/Wochentage geändert: ab `now` neu scharf (verpasst wird dadurch nichts).
    pub fn rearm(&mut self, now: i64) {
        self.armed_from = now;
    }

    /// Wochentage als Text: „Einmalig“, „Täglich“, „Mo–Fr“, „Sa, So“, „Mo, Mi, Fr“.
    pub fn days_text(&self, lang: Lang) -> String {
        days_text(self.days, lang)
    }

    pub fn display_name(&self, lang: Lang) -> String {
        if self.name.trim().is_empty() { lang.t("Wecker", "Alarm").to_string() } else { self.name.clone() }
    }
}

/// Wochentage als Text (siehe [`Alarm::days_text`]).
pub fn days_text(days: u8, lang: Lang) -> String {
    let days = days & ALL_DAYS;
    match days {
        0 => return lang.t("Einmalig", "Once").into(),
        ALL_DAYS => return lang.t("Täglich", "Every day").into(),
        WORKDAYS => return format!("{}–{}", super::weekday_short(0, lang), super::weekday_short(4, lang)),
        _ => {}
    }
    // zusammenhängende Läufe ab drei Tagen als Bereich
    let on: Vec<u32> = (0..7).filter(|i| days & (1 << i) != 0).collect();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < on.len() {
        let mut j = i;
        while j + 1 < on.len() && on[j + 1] == on[j] + 1 {
            j += 1;
        }
        if j - i >= 2 {
            parts.push(format!("{}–{}", super::weekday_short(on[i], lang), super::weekday_short(on[j], lang)));
        } else {
            for &d in &on[i..=j] {
                parts.push(super::weekday_short(d, lang).to_string());
            }
        }
        i = j + 1;
    }
    parts.join(", ")
}

/// Ergebnis von [`Alarm::check`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Due {
    /// Termin, der jetzt klingelt (Unix-Sekunden).
    pub ring: Option<i64>,
    /// `ring` bzw. `missed_snooze` ist das Ende eines Schlummerns.
    pub snoozed: bool,
    /// Ende eines Schlummerns, das zu lange zurückliegt, um noch zu klingeln (Unix-Sekunden).
    pub missed_snooze: Option<i64>,
    /// Verpasste Termine, älteste zuerst (höchstens [`MAX_MISSED`]).
    pub missed: Vec<i64>,
}

/// Lokale Zeit → Unix-Sekunden: in der Lücke der Zeitumstellung die nächste gültige Minute, in der doppelten Stunde der
/// erste Durchlauf.
pub fn resolve_local<Tz: TimeZone>(tz: &Tz, t: NaiveDateTime) -> Option<i64> {
    let mut t = t;
    for _ in 0..=180 {
        match tz.from_local_datetime(&t) {
            LocalResult::Single(d) => return Some(d.timestamp()),
            LocalResult::Ambiguous(a, b) => return Some(a.timestamp().min(b.timestamp())),
            LocalResult::None => {
                // auf die nächste volle Minute, dann minutenweise weiter
                t = t.checked_add_signed(Duration::minutes(1))?.with_second(0)?;
            }
        }
    }
    None
}

/// „Verpasst: Wecker 07:00 (Mo)“ bzw. mit Namen „Verpasst: Wecker „Aufstehen“ 07:00 (Mo)“ – Zeit und Tag des Termins in `tz`.
pub fn missed_text<Tz: TimeZone>(a: &Alarm, at: i64, tz: &Tz, lang: Lang) -> String {
    let when = tz.timestamp_opt(at, 0).single().map(|d| d.naive_local());
    let (time, day) = match when {
        Some(d) => (format!("{:02}:{:02}", d.hour(), d.minute()), super::weekday_short(d.weekday().num_days_from_monday(), lang)),
        None => (a.time_text(), ""),
    };
    let name = if a.name.trim().is_empty() { String::new() } else { format!(" {}", lang.q(&a.name)) };
    format!("{} {}{name} {time} ({day})", lang.t("Verpasst:", "Missed:"), lang.t("Wecker", "alarm"))
}

pub fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect::<String>().trim().to_string()
}

/// Liest `HH:MM` (auch `H:MM`, `7`, `0700`).
pub fn parse_hhmm(s: &str) -> Option<(u8, u8)> {
    let s = s.trim();
    let (h, m) = if let Some((h, m)) = s.split_once(':') {
        (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?)
    } else if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        (s[..2].parse().ok()?, s[2..].parse().ok()?)
    } else {
        (s.parse::<u32>().ok()?, 0)
    };
    (h < 24 && m < 60).then_some((h as u8, m as u8))
}

/// Wochentage aus Namen (`mon`, `tue` … bzw. `mo`, `di` …; Groß-/Kleinschreibung egal). Unbekannter Name: `Err(name)`.
pub fn parse_days<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<u8, String> {
    let mut bits = 0u8;
    for n in names {
        let k = n.trim().to_lowercase();
        let i = match k.as_str() {
            "mon" | "monday" | "mo" | "montag" => 0,
            "tue" | "tuesday" | "di" | "dienstag" => 1,
            "wed" | "wednesday" | "mi" | "mittwoch" => 2,
            "thu" | "thursday" | "do" | "donnerstag" => 3,
            "fri" | "friday" | "fr" | "freitag" => 4,
            "sat" | "saturday" | "sa" | "samstag" => 5,
            "sun" | "sunday" | "so" | "sonntag" => 6,
            _ => return Err(n.to_string()),
        };
        bits |= 1 << i;
    }
    Ok(bits)
}

/// Wochentage als Namen für die MCP-Antworten (`mon` … `sun`).
pub fn day_names(days: u8) -> Vec<&'static str> {
    const N: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    (0..7).filter(|i| days & (1 << i) != 0).map(|i| N[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::Europe::Berlin;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Berlin.from_local_datetime(&NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, 0).unwrap()).earliest().unwrap().timestamp()
    }

    fn local(t: i64) -> String {
        Berlin.timestamp_opt(t, 0).unwrap().format("%Y-%m-%d %H:%M %a").to_string()
    }

    #[test]
    fn next_occurrence_respects_weekdays_and_once() {
        // Mittwoch, 2026-11-18 10:00
        let now = ts(2026, 11, 18, 10, 0);
        let mut a = Alarm::new("x", 7, 0, WORKDAYS, now);
        assert_eq!(local(a.next_after(now, &Berlin).unwrap()), "2026-11-19 07:00 Thu");
        a.days = WEEKEND;
        assert_eq!(local(a.next_after(now, &Berlin).unwrap()), "2026-11-21 07:00 Sat");
        a.days = 0;
        a.hour = 11;
        assert_eq!(local(a.next_after(now, &Berlin).unwrap()), "2026-11-18 11:00 Wed", "einmalig: noch heute");
        a.hour = 9;
        assert_eq!(local(a.next_after(now, &Berlin).unwrap()), "2026-11-19 09:00 Thu", "einmalig: morgen");
    }

    #[test]
    fn dst_gap_moves_to_the_next_valid_time_and_repeated_hour_fires_once() {
        // Sommerzeit 2026: 29. März 02:00 → 03:00
        let a = Alarm::new("gap", 2, 30, ALL_DAYS, ts(2026, 3, 28, 12, 0));
        let n = a.next_after(ts(2026, 3, 28, 12, 0), &Berlin).unwrap();
        assert_eq!(local(n), "2026-03-29 03:00 Sun");
        // Winterzeit 2026: 25. Oktober 03:00 → 02:00 (02:30 gibt es zweimal)
        let mut a = Alarm::new("rep", 2, 30, ALL_DAYS, ts(2026, 10, 24, 12, 0));
        let first = a.next_after(a.armed_from, &Berlin).unwrap();
        assert_eq!(local(first), "2026-10-25 02:30 Sun");
        let second_pass = first + 3600; // dieselbe Uhrzeit eine Stunde später (Winterzeit)
        assert_eq!(local(second_pass), "2026-10-25 02:30 Sun");
        let due = a.check(first + 10, &Berlin, 120).unwrap();
        assert_eq!(due.ring, Some(first));
        assert_eq!(a.check(second_pass + 10, &Berlin, 120), None, "die doppelte Stunde klingelt nicht noch einmal");
        assert_eq!(local(a.next_after(second_pass, &Berlin).unwrap()), "2026-10-26 02:30 Mon");
    }

    #[test]
    fn check_rings_within_grace_and_reports_missed_occurrences() {
        let created = ts(2026, 11, 16, 20, 0); // Montag
        let mut a = Alarm::new("Aufstehen", 7, 0, ALL_DAYS, created);
        assert_eq!(a.check(ts(2026, 11, 17, 6, 59), &Berlin, 120), None);
        let due = a.check(ts(2026, 11, 17, 7, 1), &Berlin, 120).unwrap();
        assert_eq!(due.ring, Some(ts(2026, 11, 17, 7, 0)));
        assert!(due.missed.is_empty());
        // Programm drei Tage aus: Mi, Do, Fr verpasst; Fr 07:00 liegt zu weit zurück, um noch zu klingeln
        let due = a.check(ts(2026, 11, 20, 9, 0), &Berlin, 120).unwrap();
        assert_eq!(due.ring, None);
        assert_eq!(due.missed, vec![ts(2026, 11, 18, 7, 0), ts(2026, 11, 19, 7, 0), ts(2026, 11, 20, 7, 0)]);
        assert_eq!(missed_text(&a, due.missed[0], &Berlin, Lang::En), "Missed: alarm “Aufstehen” 07:00 (Wed)");
        let anon = Alarm::new("", 7, 0, ALL_DAYS, created);
        assert_eq!(missed_text(&anon, due.missed[0], &Berlin, Lang::De), "Verpasst: Wecker 07:00 (Mi)");
        assert_eq!(missed_text(&anon, ts(2026, 11, 16, 7, 0), &Berlin, Lang::En), "Missed: alarm 07:00 (Mon)");
        // nichts doppelt
        assert_eq!(a.check(ts(2026, 11, 20, 9, 1), &Berlin, 120), None);
        // zurückgekommen kurz nach dem Termin: klingelt, ältere sind verpasst
        let due = a.check(ts(2026, 11, 23, 7, 1), &Berlin, 120).unwrap();
        assert_eq!(due.ring, Some(ts(2026, 11, 23, 7, 0)));
        assert_eq!(due.missed.len(), 2);
        // sehr lange aus: höchstens MAX_MISSED Meldungen
        let due = a.check(ts(2027, 2, 1, 12, 0), &Berlin, 120).unwrap();
        assert_eq!(due.missed.len(), MAX_MISSED);
        assert_eq!(*due.missed.last().unwrap(), ts(2027, 2, 1, 7, 0));
    }

    #[test]
    fn once_alarm_disables_itself_and_disabled_alarms_never_ring() {
        let now = ts(2026, 11, 18, 10, 0);
        let mut a = Alarm::new("", 10, 30, 0, now);
        let due = a.check(ts(2026, 11, 18, 10, 30), &Berlin, 120).unwrap();
        assert!(due.ring.is_some());
        assert!(!a.enabled);
        assert_eq!(a.check(ts(2026, 11, 19, 10, 31), &Berlin, 120), None);
        let mut b = Alarm::new("", 10, 30, ALL_DAYS, now);
        b.set_enabled(false, now);
        assert_eq!(b.check(ts(2026, 11, 20, 10, 30), &Berlin, 120), None);
        // Einschalten: ab jetzt scharf, nichts nachträglich
        b.set_enabled(true, ts(2026, 11, 20, 11, 0));
        assert_eq!(b.check(ts(2026, 11, 20, 11, 1), &Berlin, 120), None);
        // Uhrzeit ändern auf vorhin: nichts
        let mut c = Alarm::new("", 12, 0, ALL_DAYS, now);
        c.hour = 9;
        c.rearm(now);
        assert_eq!(c.check(now + 60, &Berlin, 120), None);
    }

    #[test]
    fn snooze_up_to_the_limit_and_once_alarm_still_snoozes() {
        let now = ts(2026, 11, 18, 6, 0);
        let mut a = Alarm::new("", 7, 0, 0, now);
        a.snooze_max = 2;
        let t0 = ts(2026, 11, 18, 7, 0);
        assert!(a.check(t0, &Berlin, 120).unwrap().ring.is_some());
        assert!(a.snooze(t0));
        assert_eq!(a.snoozed_until, Some(t0 + 300));
        assert_eq!(a.check(t0 + 299, &Berlin, 120), None);
        let d = a.check(t0 + 300, &Berlin, 120).unwrap();
        assert_eq!((d.ring, d.snoozed), (Some(t0 + 300), true));
        assert!(a.snooze(t0 + 300));
        assert!(!a.can_snooze());
        assert!(!a.snooze(t0 + 600), "Limit erreicht");
        a.dismiss();
        assert!(a.can_snooze() && a.snoozed_until.is_none());
    }

    /// Systemuhr zurückgestellt: `armed_from` lag in der „Zukunft“ – der Wecker wird neu scharf und klingelt.
    #[test]
    fn clock_set_back_rearms_the_alarm() {
        let mut a = Alarm::new("", 7, 0, ALL_DAYS, ts(2026, 11, 20, 12, 0));
        // Uhr zwei Tage zurück: 2026-11-18 06:00
        let back = ts(2026, 11, 18, 6, 0);
        assert_eq!(a.check(back, &Berlin, 120), Some(Due::default()), "neu scharf (speichern)");
        assert_eq!(a.armed_from, back);
        let due = a.check(ts(2026, 11, 18, 7, 0), &Berlin, 120).unwrap();
        assert_eq!(due.ring, Some(ts(2026, 11, 18, 7, 0)));
        // kleine Abweichung (< 5 min) ändert nichts
        let mut b = Alarm::new("", 7, 0, ALL_DAYS, back + 200);
        assert_eq!(b.check(back, &Berlin, 120), None);
        // unplausibel fernes Schlummern wird auf jetzt + 5 min gesetzt
        let mut c = Alarm::new("", 7, 0, ALL_DAYS, back);
        c.snoozed_until = Some(back + 86_400);
        assert!(c.fix_clock_back(back));
        assert_eq!(c.snoozed_until, Some(back + 300));
    }

    /// Ein Schlummern, das während des Ruhezustands ablief, wird als verpasst gemeldet.
    #[test]
    fn snooze_that_ended_while_away_is_reported_as_missed() {
        let t0 = ts(2026, 11, 18, 7, 0);
        let mut a = Alarm::new("", 7, 0, 0, t0 - 3600);
        assert!(a.check(t0, &Berlin, 120).unwrap().ring.is_some());
        assert!(a.snooze(t0));
        let due = a.check(t0 + 3600, &Berlin, 120).unwrap();
        assert_eq!((due.ring, due.missed_snooze, due.snoozed), (None, Some(t0 + 300), true));
        assert_eq!(a.snoozed_until, None);
    }

    #[test]
    fn day_texts_and_parsers() {
        assert_eq!(days_text(0, Lang::En), "Once");
        assert_eq!(days_text(ALL_DAYS, Lang::De), "Täglich");
        assert_eq!(days_text(WORKDAYS, Lang::En), "Mon–Fri");
        assert_eq!(days_text(WEEKEND, Lang::De), "Sa, So");
        assert_eq!(days_text(0b0010101, Lang::En), "Mon, Wed, Fri");
        assert_eq!(days_text(0b0001111 | 0b1000000, Lang::En), "Mon–Thu, Sun");
        assert_eq!(parse_hhmm("7:05"), Some((7, 5)));
        assert_eq!(parse_hhmm("0700"), Some((7, 0)));
        assert_eq!(parse_hhmm("23"), Some((23, 0)));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_days(["Mon", "fr", "Sonntag"]), Ok(0b1010001));
        assert_eq!(parse_days(["xyz"]), Err("xyz".into()));
        assert_eq!(day_names(WEEKEND), vec!["sat", "sun"]);
    }
}
