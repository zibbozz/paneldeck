//! Erinnerungen an Termine (#164) als reine Funktion: Welche Erinnerungen fallen in `(last_check, now]`? Die Oberfläche
//! (bzw. der Benachrichtiger des Zeit-Reiters) ruft [`poll`] regelmäßig auf und zeigt, was zurückkommt. Erinnerungen an
//! Termine sind standardmäßig aus (Optionen → „Erinnerungen an Termine“).
//!
//! Gerechnet wird mit Zeitpunkten (UTC-Sekunden), damit die doppelte Stunde bei der Zeitumstellung nichts doppelt
//! auslöst. Lag `last_check` lange zurück (Paneldeck war geschlossen, Rechner im Ruhezustand), kommen die verpassten
//! Erinnerungen mit `missed = true` – sie lassen sich als Hinweis „Verpasst: …“ zeigen. Höchstens [`MAX_PER_CHECK`].

use super::zone::Zone;
use super::Event;
use chrono::{Duration, NaiveDateTime};

/// Höchstens so viele Erinnerungen je Prüfung (älteste verpasste fallen weg).
pub const MAX_PER_CHECK: usize = 50;
/// Verpasste Erinnerungen werden höchstens so weit zurück nachgeholt (7 Tage).
pub const MAX_CATCH_UP_SECS: i64 = 7 * 86_400;
/// Was mehr als so viele Sekunden vor `now` fällig war, gilt als verpasst.
pub const MISSED_AFTER_SECS: i64 = 120;

/// Eine fällige Erinnerung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    pub event_id: i64,
    pub title: String,
    /// Beginn des Vorkommens (lokale Wanduhrzeit).
    pub start: NaiveDateTime,
    /// Zeitpunkt der Erinnerung (UTC-Sekunden).
    pub fire_at: i64,
    pub minutes_before: i64,
    /// Lag deutlich vor `now` (Programm war zu).
    pub missed: bool,
}

impl Reminder {
    /// Text für Hinweis/Benachrichtigung.
    pub fn text(&self, lang: crate::state::Lang) -> String {
        let when = self.start.format(match lang {
            crate::state::Lang::De => "%d.%m. %H:%M",
            crate::state::Lang::En => "%b %d, %H:%M",
        });
        if self.missed {
            format!("{} {} ({when})", lang.t("Verpasst:", "Missed:"), self.title)
        } else if self.minutes_before == 0 {
            format!("{} – {}", self.title, lang.t("jetzt", "now"))
        } else {
            format!("{} – {when}", self.title)
        }
    }
}

/// Erinnerungen der Termine `events`, deren Zeitpunkt in `(last_check, now]` liegt (UTC-Sekunden), nach Zeitpunkt
/// sortiert. Reine Funktion (feste Uhr und Zone in Tests).
pub fn due_reminders(events: &[Event], now: i64, last_check: i64, zone: &dyn Zone) -> Vec<Reminder> {
    let last = last_check.max(now - MAX_CATCH_UP_SECS);
    if now <= last {
        return vec![];
    }
    let mut out = Vec::new();
    for e in events {
        let Some(before) = e.reminder else { continue };
        // Vorkommen, deren Beginn in (last + vorher, now + vorher] liegt; großzügiges Fenster in Wanduhrzeit (±1 Tag
        // wegen Zeitumstellung und Ganztägigem), danach genau nach Zeitpunkt gefiltert
        let lo = zone.to_local(last + before * 60) - Duration::days(1);
        let hi = zone.to_local(now + before * 60) + Duration::days(1);
        for s in e.occurrences(lo, hi, zone) {
            if s < lo {
                continue;
            }
            let fire = zone.instant(s) - before * 60;
            if fire > last && fire <= now {
                out.push(Reminder { event_id: e.id, title: e.title.clone(), start: s, fire_at: fire, minutes_before: before, missed: now - fire > MISSED_AFTER_SECS });
            }
        }
    }
    out.sort_by(|a, b| a.fire_at.cmp(&b.fire_at).then(a.event_id.cmp(&b.event_id)));
    if out.len() > MAX_PER_CHECK {
        out.drain(..out.len() - MAX_PER_CHECK);
    }
    out
}

/// Zeitpunkt der letzten Prüfung wird höchstens so oft in die Datenbank geschrieben (Sekunden), außer es kam eine
/// Erinnerung.
pub const PERSIST_EVERY_SECS: i64 = 300;

/// Prüft die Erinnerungen regelmäßig (z. B. alle 30 s aus dem Takt des Zeit-Reiters) – billig: die letzte Prüfung liegt
/// im Speicher, in die Datenbank kommt sie nur alle [`PERSIST_EVERY_SECS`] und sofort, wenn eine Erinnerung kam. So kann
/// nach einem Neustart nichts doppelt kommen: zwischen dem gespeicherten Zeitpunkt und dem Beenden gab es keine.
#[derive(Debug, Default)]
pub struct Poller {
    /// Letzte Prüfung (im Speicher); `None` = noch nicht aus der Datenbank geladen.
    last: Option<i64>,
    /// Zuletzt gespeicherter Zeitpunkt.
    persisted: i64,
    loaded: bool,
}

impl Poller {
    /// Erinnerungen in `(letzte Prüfung, now]`. Beim allerersten Aufruf ohne gespeicherten Zeitpunkt gibt es keine
    /// (nichts nachholen); danach kommen nach einem Neustart die verpassten mit `missed = true`. Sind Erinnerungen in
    /// den Optionen aus, kommt nichts zurück (die Prüfung rückt trotzdem vor, damit beim Einschalten nichts Altes kommt).
    pub fn poll(&mut self, conn: &rusqlite::Connection, now: i64, zone: &dyn Zone) -> Vec<Reminder> {
        if !self.loaded {
            self.loaded = true;
            self.last = super::store::last_reminder_check(conn);
            self.persisted = self.last.unwrap_or(i64::MIN);
        }
        let Some(last) = self.last else {
            self.last = Some(now);
            self.persist(conn, now);
            return vec![];
        };
        if now <= last {
            return vec![];
        }
        self.last = Some(now);
        let out = if super::store::load_options(conn).reminders {
            let events = super::store::events_with_reminder(conn).unwrap_or_default();
            due_reminders(&events, now, last, zone)
        } else {
            vec![]
        };
        if !out.is_empty() || now - self.persisted >= PERSIST_EVERY_SECS {
            self.persist(conn, now);
        }
        out
    }

    fn persist(&mut self, conn: &rusqlite::Connection, now: i64) {
        if super::store::set_last_reminder_check(conn, now).is_ok() {
            self.persisted = now;
        }
    }
}

/// Einmalige Prüfung ohne gemerkten Zustand (liest und schreibt die letzte Prüfung in der Datenbank). Für regelmäßige
/// Aufrufe [`Poller`] nehmen (die Anwendung nutzt den `Poller` im Takt des Zeit-Reiters; diese Form nur in Tests).
#[cfg(test)]
pub fn poll(conn: &rusqlite::Connection, now: i64, zone: &dyn Zone) -> Vec<Reminder> {
    let mut p = Poller::default();
    let r = p.poll(conn, now, zone);
    if p.persisted != now {
        p.persist(conn, now);
    }
    r
}
