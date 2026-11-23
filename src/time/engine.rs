//! Takt des Reiters „Zeit“ (#164): prüft Wecker, Timer und Geburtstags-Erinnerungen und meldet, was passiert ist.
//! Ohne egui und ohne Benachrichtigungen – die Anwendung macht aus den [`Event`]s Klang, Toast und Hinweise und speichert
//! die geänderten Einträge ([`Changed`]).

use super::TimeData;
use chrono::{Datelike, TimeZone, Timelike};

/// Ein Wecker oder Timer klingelt nur, wenn sein Termin höchstens so lange zurückliegt (Sekunden); ältere gelten als
/// verpasst (Programm beendet, Rechner im Ruhezustand).
pub const GRACE_SECS: i64 = 120;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Wecker klingelt (`at` = Termin in Unix-Sekunden; `snoozed` = Ende eines Schlummerns).
    AlarmRing { id: i64, at: i64, snoozed: bool },
    /// Wecker-Termin verpasst (Unix-Sekunden; `snoozed` = Ende eines Schlummerns).
    AlarmMissed { id: i64, at: i64, snoozed: bool },
    /// Timer abgelaufen, klingelt (`at` = Ende in Unix-ms).
    TimerDone { id: i64, at: i64 },
    /// Timer lief ab, während Paneldeck zu war bzw. der Rechner schlief (kein Klingeln).
    TimerExpiredAway { id: i64, at: i64 },
    /// Geburtstags-Erinnerung (heute).
    Birthday { id: i64 },
}

/// Kennungen der geänderten Einträge (zum Speichern).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub alarms: Vec<i64>,
    pub timers: Vec<i64>,
    pub birthdays: Vec<i64>,
}

impl Changed {
    pub fn is_empty(&self) -> bool {
        self.alarms.is_empty() && self.timers.is_empty() && self.birthdays.is_empty()
    }
}

/// Wie lange ein zwischengespeicherter nächster Termin gilt (Sekunden); danach wird er neu berechnet (z. B. nach einem
/// Wechsel der Zeitzone des Systems).
const CACHE_SECS: i64 = 600;

/// Prüfwert der Einstellungen eines Weckers, von denen sein nächster Termin abhängt.
fn alarm_key(a: &super::Alarm) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (a.armed_from, a.hour, a.minute, a.days, a.enabled, a.snoozed_until).hash(&mut h);
    h.finish()
}

/// Wurde die Systemuhr zurückgestellt (siehe `Alarm::fix_clock_back`)?
fn clock_set_back(a: &super::Alarm, now_s: i64) -> bool {
    a.armed_from > now_s + super::alarm::CLOCK_BACK_TOLERANCE || a.snoozed_until.is_some_and(|s| s > now_s + 3600 + super::alarm::CLOCK_BACK_TOLERANCE)
}

/// Ein Takt zum Zeitpunkt `now_ms` in der Zone `tz`.
pub fn tick<Tz: TimeZone>(data: &mut TimeData, now_ms: i64, tz: &Tz) -> (Vec<Event>, Changed) {
    let mut ev = Vec::new();
    let mut ch = Changed::default();
    let now_s = now_ms.div_euclid(1000);
    let TimeData { alarms, next_cache, .. } = data;
    for a in alarms.iter_mut() {
        // Zwischenspeicher: steht der nächste Termin fest und liegt in der Zukunft, ist nichts zu tun
        let k = alarm_key(a);
        let snooze_due = a.snoozed_until.is_some_and(|s| s <= now_s);
        if !snooze_due
            && !clock_set_back(a, now_s)
            && let Some(&(ck, at, next)) = next_cache.get(&a.id)
            && ck == k
            && now_s - at < CACHE_SECS
            && now_s >= at
            && next.is_none_or(|n| n > now_s)
        {
            continue;
        }
        let due = a.check(now_s, tz, GRACE_SECS);
        let next = if a.enabled { a.next_after(a.armed_from.max(now_s - 1), tz) } else { None };
        next_cache.insert(a.id, (alarm_key(a), now_s, next));
        if let Some(due) = due {
            ch.alarms.push(a.id);
            for at in due.missed {
                ev.push(Event::AlarmMissed { id: a.id, at, snoozed: false });
            }
            if let Some(at) = due.missed_snooze {
                ev.push(Event::AlarmMissed { id: a.id, at, snoozed: true });
            }
            if let Some(at) = due.ring {
                ev.push(Event::AlarmRing { id: a.id, at, snoozed: due.snoozed });
            }
        }
    }
    let TimeData { timers, birthdays, .. } = data;
    for t in timers.iter_mut() {
        if let Some(x) = t.check(now_ms, GRACE_SECS * 1000) {
            ch.timers.push(t.id);
            ev.push(if x.ring { Event::TimerDone { id: t.id, at: x.at } } else { Event::TimerExpiredAway { id: t.id, at: x.at } });
        }
    }
    if let Some(local) = tz.timestamp_millis_opt(now_ms).single().map(|d| d.naive_local()) {
        let today = local.date();
        for b in birthdays.iter_mut() {
            if b.wants_reminder(today, local.hour()) {
                b.reminded_year = Some(today.year());
                ch.birthdays.push(b.id);
                ev.push(Event::Birthday { id: b.id });
            }
        }
    }
    (ev, ch)
}

/// Nächster Zeitpunkt (Unix-ms), zu dem ein Takt etwas ändern kann (Wecker, Schlummern, Timer-Ende) – zum Wecken der
/// Anwendung. `None`, wenn nichts ansteht.
pub fn next_wake<Tz: TimeZone>(data: &TimeData, now_ms: i64, tz: &Tz) -> Option<i64> {
    let now_s = now_ms.div_euclid(1000);
    let alarms = data.alarms.iter().flat_map(|a| {
        // aus dem Zwischenspeicher des Takts, sonst ausrechnen
        let cached = data.next_cache.get(&a.id).filter(|(k, at, _)| *k == alarm_key(a) && now_s - at < CACHE_SECS && now_s >= *at && !clock_set_back(a, now_s));
        let reg = match cached {
            Some(&(_, _, next)) => next.map(|s| s * 1000),
            None => {
                // Systemuhr zurückgestellt: wie `Alarm::fix_clock_back` ab jetzt rechnen
                let mut a = a.clone();
                a.fix_clock_back(now_s);
                if a.enabled { a.next_after(a.armed_from.max(now_s - 1), tz).map(|s| s * 1000) } else { None }
            }
        };
        let mut snooze = a.snoozed_until;
        if clock_set_back(a, now_s) {
            let mut b = a.clone();
            b.fix_clock_back(now_s);
            snooze = b.snoozed_until;
        }
        [reg, snooze.map(|s| s * 1000)]
    });
    let timers = data.timers.iter().filter_map(|t| match t.state {
        super::TimerState::Running { ends_at } => Some(ends_at),
        _ => None,
    });
    alarms.flatten().chain(timers).min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::alarm::ALL_DAYS;
    use crate::time::{Alarm, Birthday, Timer};
    use chrono::NaiveDate;
    use chrono_tz::Europe::Berlin;

    fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Berlin.from_local_datetime(&NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, 0).unwrap()).earliest().unwrap().timestamp_millis()
    }

    /// Ablauf wie in der Anwendung: Einträge stehen in der Datenbank, das Programm wird „neu gestartet“ (neu geladen),
    /// der Takt meldet Verpasstes und Abgelaufenes einmal.
    #[test]
    fn restart_reports_missed_alarms_and_expired_timers_once() {
        let conn = crate::db::open_memory();
        let start = ms(2026, 11, 16, 22, 0);
        let mut a = Alarm::new("Aufstehen", 7, 0, ALL_DAYS, start / 1000);
        crate::time::store::save_alarm(&conn, &mut a).unwrap();
        let mut t = Timer::new("Wäsche", 3_600_000);
        t.start(start);
        crate::time::store::save_timer(&conn, &mut t).unwrap();
        let mut sw = crate::time::Stopwatch::new("Projekt");
        sw.start(start);
        crate::time::store::save_stopwatch(&conn, &mut sw).unwrap();
        // „Neustart“ am nächsten Morgen um 09:00
        let mut d = crate::time::store::load(&conn).unwrap();
        let now = ms(2026, 11, 17, 9, 0);
        let (ev, ch) = tick(&mut d, now, &Berlin);
        assert_eq!(ev, vec![Event::AlarmMissed { id: a.id, at: ms(2026, 11, 17, 7, 0) / 1000, snoozed: false }, Event::TimerExpiredAway { id: t.id, at: start + 3_600_000 }]);
        assert_eq!((ch.alarms.clone(), ch.timers.clone()), (vec![a.id], vec![t.id]));
        assert_eq!(d.stopwatches[0].elapsed(now), now - start, "die Stoppuhr lief weiter");
        // gespeichert und erneut geladen: nichts doppelt
        crate::time::store::save_alarm(&conn, d.alarm_mut(a.id).unwrap()).unwrap();
        crate::time::store::save_timer(&conn, d.timer_mut(t.id).unwrap()).unwrap();
        let mut d2 = crate::time::store::load(&conn).unwrap();
        let (ev, ch) = tick(&mut d2, now + 1000, &Berlin);
        assert!(ev.is_empty() && ch.is_empty(), "{ev:?}");
        // am nächsten Morgen klingelt er pünktlich
        let (ev, _) = tick(&mut d2, ms(2026, 11, 18, 7, 0) + 500, &Berlin);
        assert_eq!(ev, vec![Event::AlarmRing { id: a.id, at: ms(2026, 11, 18, 7, 0) / 1000, snoozed: false }]);
    }

    #[test]
    fn timer_rings_when_running_and_next_wake_points_at_the_earliest_event() {
        let mut d = TimeData::default();
        let now = ms(2026, 11, 18, 10, 0);
        let mut t = Timer::new("Tee", 240_000);
        t.id = 1;
        t.start(now);
        d.timers.push(t);
        let mut a = Alarm::new("", 10, 2, ALL_DAYS, now / 1000);
        a.id = 2;
        d.alarms.push(a);
        assert_eq!(next_wake(&d, now, &Berlin), Some(now + 120_000));
        let (ev, _) = tick(&mut d, now + 120_000, &Berlin);
        assert_eq!(ev, vec![Event::AlarmRing { id: 2, at: (now + 120_000) / 1000, snoozed: false }]);
        assert_eq!(next_wake(&d, now + 121_000, &Berlin), Some(now + 240_000));
        let (ev, _) = tick(&mut d, now + 240_000, &Berlin);
        assert_eq!(ev, vec![Event::TimerDone { id: 1, at: now + 240_000 }]);
        d.alarm_mut(2).unwrap().snooze((now + 240_000) / 1000);
        assert_eq!(next_wake(&d, now + 241_000, &Berlin), Some(now + 240_000 + 300_000));
    }

    #[test]
    fn birthday_reminder_fires_once_on_the_day() {
        let mut d = TimeData::default();
        let mut b = Birthday::new("Anna", 11, 18, None);
        b.id = 5;
        b.remind = true;
        d.birthdays.push(b);
        assert!(tick(&mut d, ms(2026, 11, 18, 7, 0), &Berlin).0.is_empty());
        assert_eq!(tick(&mut d, ms(2026, 11, 18, 8, 0), &Berlin).0, vec![Event::Birthday { id: 5 }]);
        assert!(tick(&mut d, ms(2026, 11, 18, 9, 0), &Berlin).0.is_empty());
        assert_eq!(d.birthdays[0].reminded_year, Some(2026));
    }

    /// 1000 Einträge je Art: ein Takt bleibt weit unter einem Bild.
    #[test]
    fn a_thousand_entries_tick_quickly() {
        let mut d = TimeData::default();
        let now = ms(2026, 11, 18, 10, 0);
        for i in 0..1000 {
            let mut a = Alarm::new("", (i % 24) as u8, (i % 60) as u8, ALL_DAYS, now / 1000);
            a.id = i;
            d.alarms.push(a);
            let mut t = Timer::new("", 3_600_000);
            t.id = i;
            t.start(now);
            d.timers.push(t);
            let mut b = Birthday::new("", 1 + (i % 12) as u32, 1 + (i % 28) as u32, None);
            b.id = i;
            b.remind = true;
            d.birthdays.push(b);
        }
        let t0 = std::time::Instant::now();
        for k in 0..10 {
            let _ = tick(&mut d, now + k * 1000, &Berlin);
            let _ = next_wake(&d, now + k * 1000, &Berlin);
        }
        assert!(t0.elapsed() < std::time::Duration::from_millis(1500), "{:?}", t0.elapsed());
    }
}
