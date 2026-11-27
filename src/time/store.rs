//! Speicherung des Reiters „Zeit“ (#164) in `paneldeck.db`: eigene Tabellen (Migration [`MIGRATION`], angehängt an
//! `db::MIGRATIONS`), Laden aller Einträge und Speichern/Löschen je Eintrag. Das Schema ist intern (nicht öffentlich);
//! Sicherungen und Wiederherstellung der Datenbank (`backup`) enthalten die Tabellen von selbst, ebenso die Rettung einer
//! beschädigten Datenbank (`planning::maintenance`), die alle Tabellen des aktuellen Schemas übernimmt.
//!
//! Zeiten: Wecker in Unix-Sekunden, Timer und Stoppuhren in Unix-Millisekunden.

use super::{Alarm, Birthday, Preset, Stopwatch, TimeData, Timer, TimerState, WorldClock};
use rusqlite::{params, Connection, OptionalExtension};

/// Tabellen des Reiters „Zeit“ (Schema-Version 11).
pub const MIGRATION: &str = "
CREATE TABLE time_alarms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',
    hour INTEGER NOT NULL,
    minute INTEGER NOT NULL,
    days INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    sound TEXT NOT NULL DEFAULT '',
    snooze_minutes INTEGER NOT NULL DEFAULT 5,
    snooze_max INTEGER NOT NULL DEFAULT 10,
    armed_from INTEGER NOT NULL,
    snoozed_until INTEGER,
    snooze_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE time_timers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'idle',
    ends_at INTEGER,
    remaining_ms INTEGER,
    expired_at INTEGER,
    sound TEXT NOT NULL DEFAULT '',
    ticket TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE time_timer_presets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE time_stopwatches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',
    started_at INTEGER,
    accumulated_ms INTEGER NOT NULL DEFAULT 0,
    laps TEXT NOT NULL DEFAULT '[]',
    ticket TEXT NOT NULL DEFAULT '',
    booked_ms INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE time_clocks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL DEFAULT '',
    zone TEXT NOT NULL,
    analog INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE TABLE time_birthdays (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',
    month INTEGER NOT NULL,
    day INTEGER NOT NULL,
    year INTEGER,
    remind INTEGER NOT NULL DEFAULT 0,
    reminded_year INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);";

/// Länge des aktuellen Timer-Laufs (Schema-Version 14): Die Dauer darf sich während eines Laufs ändern, die
/// Zeiterfassung rechnet mit der Länge beim Start. `NULL` (ältere Bestände) = die Dauer.
pub const MIGRATION_TIMER_RUN: &str = "ALTER TABLE time_timers ADD COLUMN run_ms INTEGER;";

type R<T> = rusqlite::Result<T>;

fn now_text() -> String {
    crate::tasks::now()
}

/// Lädt alles (Reihenfolge: Wecker nach Uhrzeit, sonst nach Anlage). Unlesbare Werte werden auf gültige gesetzt.
pub fn load(conn: &Connection) -> R<TimeData> {
    let mut d = TimeData::default();
    {
        let mut st = conn.prepare("SELECT id, name, hour, minute, days, enabled, sound, snooze_minutes, snooze_max, armed_from, snoozed_until, snooze_count FROM time_alarms ORDER BY hour, minute, id")?;
        d.alarms = st
            .query_map([], |r| {
                Ok(Alarm {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    hour: r.get::<_, i64>(2)?.clamp(0, 23) as u8,
                    minute: r.get::<_, i64>(3)?.clamp(0, 59) as u8,
                    days: (r.get::<_, i64>(4)? & 0x7f) as u8,
                    enabled: r.get::<_, i64>(5)? != 0,
                    sound: r.get(6)?,
                    snooze_minutes: r.get::<_, i64>(7)?.clamp(1, 60) as u32,
                    snooze_max: r.get::<_, i64>(8)?.clamp(0, 30) as u32,
                    armed_from: r.get(9)?,
                    snoozed_until: r.get(10)?,
                    snooze_count: r.get::<_, i64>(11)?.clamp(0, 1000) as u32,
                })
            })?
            .collect::<R<_>>()?;
    }
    {
        let mut st = conn.prepare("SELECT id, name, duration_ms, state, ends_at, remaining_ms, expired_at, sound, ticket, run_ms FROM time_timers ORDER BY id")?;
        d.timers = st
            .query_map([], |r| {
                let state: String = r.get(3)?;
                let (ends, rem, exp): (Option<i64>, Option<i64>, Option<i64>) = (r.get(4)?, r.get(5)?, r.get(6)?);
                let state = match (state.as_str(), ends, rem, exp) {
                    ("running", Some(e), _, _) => TimerState::Running { ends_at: e },
                    ("paused", _, Some(m), _) => TimerState::Paused { remaining_ms: m.max(0) },
                    ("expired", _, _, Some(a)) => TimerState::Expired { at: a },
                    _ => TimerState::Idle,
                };
                let duration_ms = super::timer::clamp_duration(r.get(2)?);
                let run_ms = r.get::<_, Option<i64>>(9)?.map(super::timer::clamp_duration).unwrap_or(duration_ms);
                Ok(Timer { id: r.get(0)?, name: r.get(1)?, duration_ms, state, sound: r.get(7)?, ticket: r.get(8)?, run_ms })
            })?
            .collect::<R<_>>()?;
    }
    {
        let mut st = conn.prepare("SELECT id, name, duration_ms FROM time_timer_presets ORDER BY id")?;
        d.presets = st.query_map([], |r| Ok(Preset { id: r.get(0)?, name: r.get(1)?, duration_ms: super::timer::clamp_duration(r.get(2)?) }))?.collect::<R<_>>()?;
    }
    {
        let mut st = conn.prepare("SELECT id, name, started_at, accumulated_ms, laps, ticket, booked_ms FROM time_stopwatches ORDER BY id")?;
        d.stopwatches = st
            .query_map([], |r| {
                Ok(Stopwatch {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    started_at: r.get(2)?,
                    accumulated_ms: r.get::<_, i64>(3)?.max(0),
                    laps: super::stopwatch::laps_from_json(&r.get::<_, String>(4)?),
                    ticket: r.get(5)?,
                    booked_ms: r.get::<_, i64>(6)?.max(0),
                })
            })?
            .collect::<R<_>>()?;
    }
    {
        let mut st = conn.prepare("SELECT id, label, zone, analog FROM time_clocks ORDER BY id")?;
        d.clocks = st.query_map([], |r| Ok(WorldClock { id: r.get(0)?, label: r.get(1)?, zone: r.get(2)?, analog: r.get::<_, i64>(3)? != 0 }))?.collect::<R<_>>()?;
    }
    {
        let mut st = conn.prepare("SELECT id, name, month, day, year, remind, reminded_year FROM time_birthdays ORDER BY id")?;
        d.birthdays = st
            .query_map([], |r| {
                Ok(Birthday {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    month: r.get::<_, i64>(2)?.clamp(1, 12) as u32,
                    day: r.get::<_, i64>(3)?.clamp(1, 31) as u32,
                    year: r.get(4)?,
                    remind: r.get::<_, i64>(5)? != 0,
                    reminded_year: r.get(6)?,
                })
            })?
            .collect::<R<_>>()?;
    }
    Ok(d)
}

/// Speichert einen Wecker (neu bei `id == 0`, dann wird `id` gesetzt).
pub fn save_alarm(conn: &Connection, a: &mut Alarm) -> R<()> {
    let now = now_text();
    if a.id == 0 {
        conn.execute(
            "INSERT INTO time_alarms (name, hour, minute, days, enabled, sound, snooze_minutes, snooze_max, armed_from, snoozed_until, snooze_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![a.name, a.hour, a.minute, a.days, a.enabled, a.sound, a.snooze_minutes, a.snooze_max, a.armed_from, a.snoozed_until, a.snooze_count, now],
        )?;
        a.id = conn.last_insert_rowid();
    } else {
        conn.execute(
            "UPDATE time_alarms SET name = ?1, hour = ?2, minute = ?3, days = ?4, enabled = ?5, sound = ?6, snooze_minutes = ?7, snooze_max = ?8,
             armed_from = ?9, snoozed_until = ?10, snooze_count = ?11, updated_at = ?12 WHERE id = ?13",
            params![a.name, a.hour, a.minute, a.days, a.enabled, a.sound, a.snooze_minutes, a.snooze_max, a.armed_from, a.snoozed_until, a.snooze_count, now, a.id],
        )?;
    }
    Ok(())
}

fn timer_cols(t: &Timer) -> (&'static str, Option<i64>, Option<i64>, Option<i64>) {
    match t.state {
        TimerState::Idle => ("idle", None, None, None),
        TimerState::Running { ends_at } => ("running", Some(ends_at), None, None),
        TimerState::Paused { remaining_ms } => ("paused", None, Some(remaining_ms), None),
        TimerState::Expired { at } => ("expired", None, None, Some(at)),
    }
}

pub fn save_timer(conn: &Connection, t: &mut Timer) -> R<()> {
    let now = now_text();
    let (state, ends, rem, exp) = timer_cols(t);
    if t.id == 0 {
        conn.execute(
            "INSERT INTO time_timers (name, duration_ms, state, ends_at, remaining_ms, expired_at, sound, ticket, run_ms, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?10, ?9, ?9)",
            params![t.name, t.duration_ms, state, ends, rem, exp, t.sound, t.ticket, now, t.run_ms],
        )?;
        t.id = conn.last_insert_rowid();
    } else {
        conn.execute(
            "UPDATE time_timers SET name = ?1, duration_ms = ?2, state = ?3, ends_at = ?4, remaining_ms = ?5, expired_at = ?6, sound = ?7, ticket = ?8, updated_at = ?9, run_ms = ?11 WHERE id = ?10",
            params![t.name, t.duration_ms, state, ends, rem, exp, t.sound, t.ticket, now, t.id, t.run_ms],
        )?;
    }
    Ok(())
}

pub fn save_preset(conn: &Connection, p: &mut Preset) -> R<()> {
    if p.id == 0 {
        conn.execute("INSERT INTO time_timer_presets (name, duration_ms, created_at) VALUES (?1, ?2, ?3)", params![p.name, p.duration_ms, now_text()])?;
        p.id = conn.last_insert_rowid();
    } else {
        conn.execute("UPDATE time_timer_presets SET name = ?1, duration_ms = ?2 WHERE id = ?3", params![p.name, p.duration_ms, p.id])?;
    }
    Ok(())
}

pub fn save_stopwatch(conn: &Connection, s: &mut Stopwatch) -> R<()> {
    let now = now_text();
    let laps = super::stopwatch::laps_to_json(&s.laps);
    if s.id == 0 {
        conn.execute(
            "INSERT INTO time_stopwatches (name, started_at, accumulated_ms, laps, ticket, booked_ms, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![s.name, s.started_at, s.accumulated_ms, laps, s.ticket, s.booked_ms, now],
        )?;
        s.id = conn.last_insert_rowid();
    } else {
        conn.execute(
            "UPDATE time_stopwatches SET name = ?1, started_at = ?2, accumulated_ms = ?3, laps = ?4, ticket = ?5, booked_ms = ?6, updated_at = ?7 WHERE id = ?8",
            params![s.name, s.started_at, s.accumulated_ms, laps, s.ticket, s.booked_ms, now, s.id],
        )?;
    }
    Ok(())
}

pub fn save_clock(conn: &Connection, c: &mut WorldClock) -> R<()> {
    if c.id == 0 {
        conn.execute("INSERT INTO time_clocks (label, zone, analog, created_at) VALUES (?1, ?2, ?3, ?4)", params![c.label, c.zone, c.analog, now_text()])?;
        c.id = conn.last_insert_rowid();
    } else {
        conn.execute("UPDATE time_clocks SET label = ?1, zone = ?2, analog = ?3 WHERE id = ?4", params![c.label, c.zone, c.analog, c.id])?;
    }
    Ok(())
}

pub fn save_birthday(conn: &Connection, b: &mut Birthday) -> R<()> {
    let now = now_text();
    if b.id == 0 {
        conn.execute(
            "INSERT INTO time_birthdays (name, month, day, year, remind, reminded_year, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![b.name, b.month, b.day, b.year, b.remind, b.reminded_year, now],
        )?;
        b.id = conn.last_insert_rowid();
    } else {
        conn.execute(
            "UPDATE time_birthdays SET name = ?1, month = ?2, day = ?3, year = ?4, remind = ?5, reminded_year = ?6, updated_at = ?7 WHERE id = ?8",
            params![b.name, b.month, b.day, b.year, b.remind, b.reminded_year, now, b.id],
        )?;
    }
    Ok(())
}

/// Welche Tabelle (für [`delete`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Alarms,
    Timers,
    Presets,
    Stopwatches,
    Clocks,
    Birthdays,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Table::Alarms => "time_alarms",
            Table::Timers => "time_timers",
            Table::Presets => "time_timer_presets",
            Table::Stopwatches => "time_stopwatches",
            Table::Clocks => "time_clocks",
            Table::Birthdays => "time_birthdays",
        }
    }
}

/// Löscht einen Eintrag (nur aus der Oberfläche; Agenten können nichts löschen).
pub fn delete(conn: &Connection, t: Table, id: i64) -> R<bool> {
    Ok(conn.execute(&format!("DELETE FROM {} WHERE id = ?1", t.name()), [id])? > 0)
}

/// Anzahl der Einträge einer Tabelle (Tests).
#[cfg(test)]
pub fn count(conn: &Connection, t: Table) -> R<i64> {
    conn.query_row(&format!("SELECT COUNT(*) FROM {}", t.name()), [], |r| r.get(0))
}

/// Gibt es die Tabellen schon (Schema ≥ 11)?
pub fn available(conn: &Connection) -> bool {
    conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'time_alarms'", [], |_| Ok(())).optional().ok().flatten().is_some()
}

/// Mehrere Einträge auf einmal speichern (eine Transaktion; z. B. alle Wecker nach dem Prüfen beim Start).
pub fn in_tx<T>(conn: &Connection, f: impl FnOnce() -> R<T>) -> R<T> {
    conn.execute_batch("SAVEPOINT time_tx")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("RELEASE time_tx")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO time_tx; RELEASE time_tx");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::alarm::WORKDAYS;

    #[test]
    fn round_trip_every_type() {
        let conn = crate::db::open_memory();
        assert!(available(&conn));
        let mut a = Alarm::new("Aufstehen", 7, 15, WORKDAYS, 1000);
        a.snoozed_until = Some(2000);
        a.sound = "file:C:\\x.mp3".into();
        save_alarm(&conn, &mut a).unwrap();
        assert!(a.id > 0);
        let mut t = Timer::new("Tee", 240_000);
        t.start(5_000);
        t.ticket = "PAN-12".into();
        save_timer(&conn, &mut t).unwrap();
        let mut t2 = Timer::new("", 60_000);
        t2.start(0);
        t2.pause(10_000);
        save_timer(&conn, &mut t2).unwrap();
        let mut t3 = Timer::new("", 60_000);
        t3.state = TimerState::Expired { at: 77 };
        save_timer(&conn, &mut t3).unwrap();
        let mut p = Preset { id: 0, name: "Tee".into(), duration_ms: 240_000 };
        save_preset(&conn, &mut p).unwrap();
        let mut s = Stopwatch::new("Lauf");
        s.start(0);
        s.lap(1000);
        save_stopwatch(&conn, &mut s).unwrap();
        let mut c = WorldClock::new("NY", "America/New_York");
        c.analog = true;
        save_clock(&conn, &mut c).unwrap();
        let mut b = Birthday::new("Anna", 2, 29, Some(2000));
        b.remind = true;
        save_birthday(&conn, &mut b).unwrap();
        let d = load(&conn).unwrap();
        assert_eq!(d.alarms, vec![a.clone()]);
        assert_eq!(d.timers, vec![t.clone(), t2.clone(), t3.clone()]);
        assert_eq!(d.presets, vec![p.clone()]);
        assert_eq!(d.stopwatches, vec![s.clone()]);
        assert_eq!(d.clocks, vec![c.clone()]);
        assert_eq!(d.birthdays, vec![b.clone()]);
        // Ändern und Löschen
        a.name = "Neu".into();
        save_alarm(&conn, &mut a).unwrap();
        assert_eq!(load(&conn).unwrap().alarms[0].name, "Neu");
        assert!(delete(&conn, Table::Alarms, a.id).unwrap());
        assert!(!delete(&conn, Table::Alarms, a.id).unwrap());
        assert_eq!(count(&conn, Table::Alarms).unwrap(), 0);
        assert_eq!(count(&conn, Table::Timers).unwrap(), 3);
    }

    #[test]
    fn broken_values_are_clamped_on_load() {
        let conn = crate::db::open_memory();
        conn.execute("INSERT INTO time_alarms (name, hour, minute, days, enabled, armed_from, snooze_minutes, snooze_max, created_at, updated_at) VALUES ('', 99, -3, 255, 1, 0, 0, 999, '', '')", []).unwrap();
        conn.execute("INSERT INTO time_timers (name, duration_ms, state, ends_at, created_at, updated_at) VALUES ('', 0, 'running', NULL, '', '')", []).unwrap();
        conn.execute("INSERT INTO time_stopwatches (name, laps, created_at, updated_at) VALUES ('', 'kaputt', '', '')", []).unwrap();
        let d = load(&conn).unwrap();
        let a = &d.alarms[0];
        assert_eq!((a.hour, a.minute, a.days, a.snooze_minutes, a.snooze_max), (23, 0, 0x7f, 1, 30));
        assert_eq!(d.timers[0].state, TimerState::Idle, "running ohne Endzeit → bereit");
        assert_eq!(d.timers[0].duration_ms, 1000);
        assert_eq!(d.timers[0].run_ms, 1000, "ohne Länge des Laufs (ältere Bestände): die Dauer");
        assert!(d.stopwatches[0].laps.is_empty());
    }

    /// Länge des Laufs übersteht Speichern/Laden, auch wenn die Dauer währenddessen geändert wurde; Bestände von vor
    /// Schema-Version 14 (ohne Spalte) werden ergänzt und laden mit der Dauer.
    #[test]
    fn timer_run_length_survives_and_old_rows_load() {
        let conn = crate::db::open_memory();
        let mut t = Timer::new("Fokus", 600_000);
        t.start(0);
        t.set_duration(1_800_000);
        save_timer(&conn, &mut t).unwrap();
        let back = load(&conn).unwrap().timers.remove(0);
        assert_eq!((back.duration_ms, back.run_ms, back.worked(240_000)), (1_800_000, 600_000, 240_000));
        t.pause(240_000);
        save_timer(&conn, &mut t).unwrap();
        assert_eq!(load(&conn).unwrap().timers[0].worked(0), 240_000);
        // Stand vor der Migration: Tabelle ohne `run_ms`
        let dir = std::env::temp_dir().join(format!("pd-time-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let n = crate::db::migration_index(MIGRATION_TIMER_RUN);
        {
            let old = crate::db::open_prefix(&dir, &dir.join("backups"), n).unwrap();
            old.conn.execute("INSERT INTO time_timers (name, duration_ms, state, ends_at, created_at, updated_at) VALUES ('alt', 600000, 'running', 300000, '', '')", []).unwrap();
        }
        let db = crate::db::open(&dir, &dir.join("backups")).unwrap();
        let old = load(&db.conn).unwrap().timers.remove(0);
        assert_eq!((old.run_ms, old.worked(0)), (600_000, 300_000));
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transaction_helper_rolls_back_on_error() {
        let conn = crate::db::open_memory();
        let r: R<()> = in_tx(&conn, || {
            save_clock(&conn, &mut WorldClock::new("", "UTC"))?;
            conn.execute("INSERT INTO gibts_nicht VALUES (1)", [])?;
            Ok(())
        });
        assert!(r.is_err());
        assert_eq!(count(&conn, Table::Clocks).unwrap(), 0);
        in_tx(&conn, || save_clock(&conn, &mut WorldClock::new("", "UTC"))).unwrap();
        assert_eq!(count(&conn, Table::Clocks).unwrap(), 1);
    }

    /// 1000 Einträge je Art: Speichern in einer Transaktion und Laden bleiben schnell.
    #[test]
    fn a_thousand_entries_each_load_quickly() {
        let conn = crate::db::open_memory();
        let t0 = std::time::Instant::now();
        in_tx(&conn, || {
            for i in 0..1000 {
                save_alarm(&conn, &mut Alarm::new(&format!("A{i}"), (i % 24) as u8, (i % 60) as u8, WORKDAYS, 0))?;
                save_timer(&conn, &mut Timer::new(&format!("T{i}"), 60_000 + i))?;
                save_preset(&conn, &mut Preset { id: 0, name: format!("P{i}"), duration_ms: 60_000 })?;
                save_stopwatch(&conn, &mut Stopwatch::new(&format!("S{i}")))?;
                save_clock(&conn, &mut WorldClock::new("", "Europe/Berlin"))?;
                save_birthday(&conn, &mut Birthday::new(&format!("B{i}"), 1 + (i % 12) as u32, 1 + (i % 28) as u32, Some(1950 + (i % 60) as i32)))?;
            }
            Ok(())
        })
        .unwrap();
        let t1 = std::time::Instant::now();
        let d = load(&conn).unwrap();
        let load_time = t1.elapsed();
        assert_eq!((d.alarms.len(), d.timers.len(), d.presets.len(), d.stopwatches.len(), d.clocks.len(), d.birthdays.len()), (1000, 1000, 1000, 1000, 1000, 1000));
        assert!(load_time < std::time::Duration::from_secs(2), "Laden dauerte {load_time:?}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(20));
    }
}
