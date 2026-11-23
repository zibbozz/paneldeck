//! Speicher des Kalenders (#164) in `paneldeck.db`: eigene Termine (`cal_events`) und Einstellungen (`cal_settings`:
//! Optionen der Quellen, Zeitpunkt der letzten Erinnerungs-Prüfung). Das Schema ist intern (nicht öffentlich), es kommt
//! über die Migration [`MIGRATION`] (`db.rs`) und ist damit Teil jeder Sicherung (vollständige Kopie der Datenbank).
//!
//! Die Optionen liegen als JSON in `cal_settings` (Schlüssel `options`) und werden tolerant gelesen: Fehlendes gilt als
//! Standard (Fälligkeiten an, Sprints/Versionen/gebuchte Zeit aus, Erinnerungen aus).

use super::{Event, Recurrence, MAX_COUNT, MAX_EXCEPTIONS, MAX_MINUTES, MAX_NOTE, MAX_REMINDER, MAX_TITLE};
use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// SQL der Kalender-Migration (eigene Tabellen mit Präfix `cal_`, unabhängig von anderen Migrationen).
/// Eingefroren: nach dem Erscheinen nie ändern, spätere Änderungen kommen als eigene Migration.
pub const MIGRATION: &str = "
CREATE TABLE cal_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uid TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '',
    all_day INTEGER NOT NULL DEFAULT 0,
    start TEXT NOT NULL,
    minutes INTEGER NOT NULL DEFAULT 60 CHECK (minutes >= 0),
    cron TEXT,
    rec_until TEXT,
    rec_count INTEGER CHECK (rec_count IS NULL OR rec_count > 0),
    rec_end TEXT,
    exceptions TEXT NOT NULL DEFAULT '[]',
    reminder INTEGER CHECK (reminder IS NULL OR reminder >= 0),
    project_id INTEGER REFERENCES projects (id) ON DELETE SET NULL,
    origin TEXT NOT NULL DEFAULT 'local',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX cal_events_start ON cal_events (start);
CREATE INDEX cal_events_cron ON cal_events (cron) WHERE cron IS NOT NULL;
CREATE TABLE cal_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
";

/// Fehler beim Speichern (zweisprachig) oder in der Datenbank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Invalid { de: String, en: String },
    NotFound,
    Db(String),
}

impl StoreError {
    pub fn invalid(de: impl Into<String>, en: impl Into<String>) -> StoreError {
        StoreError::Invalid { de: de.into(), en: en.into() }
    }
    pub fn message(&self, lang: crate::state::Lang) -> String {
        match self {
            StoreError::Invalid { de, en } => if lang == crate::state::Lang::De { de.clone() } else { en.clone() },
            StoreError::NotFound => lang.t("Termin nicht gefunden.", "Event not found.").into(),
            StoreError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e.to_string())
    }
}

pub type SRes<T> = Result<T, StoreError>;

/// Eingaben für einen neuen oder geänderten Termin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventInput {
    pub title: String,
    pub note: String,
    pub color: String,
    pub all_day: bool,
    pub start: NaiveDateTime,
    pub minutes: i64,
    pub recur: Option<Recurrence>,
    pub reminder: Option<i64>,
    pub project_id: Option<i64>,
}

impl EventInput {
    pub fn new(title: &str, start: NaiveDateTime, minutes: i64) -> EventInput {
        EventInput { title: title.into(), note: String::new(), color: String::new(), all_day: false, start, minutes, recur: None, reminder: None, project_id: None }
    }

    pub fn from_event(e: &Event) -> EventInput {
        EventInput { title: e.title.clone(), note: e.note.clone(), color: e.color.clone(), all_day: e.all_day, start: e.start, minutes: e.minutes, recur: e.recur.clone(), reminder: e.reminder, project_id: e.project_id }
    }

    /// Prüfen und vereinheitlichen (Titel getrimmt, ganztägig auf volle Tage, Ausnahmen sortiert).
    pub fn validate(mut self) -> SRes<EventInput> {
        self.title = self.title.trim().to_string();
        if self.title.is_empty() {
            return Err(StoreError::invalid("Der Titel fehlt.", "The title is missing."));
        }
        if self.title.chars().count() > MAX_TITLE {
            return Err(StoreError::invalid(format!("Der Titel ist länger als {MAX_TITLE} Zeichen."), format!("The title is longer than {MAX_TITLE} characters.")));
        }
        if self.title.contains(['\n', '\r']) {
            self.title = self.title.replace(['\r', '\n'], " ");
        }
        if self.note.chars().count() > MAX_NOTE {
            return Err(StoreError::invalid(format!("Die Notiz ist länger als {MAX_NOTE} Zeichen."), format!("The note is longer than {MAX_NOTE} characters.")));
        }
        self.color = self.color.trim().to_string();
        if !self.color.is_empty() && !is_hex_color(&self.color) {
            return Err(StoreError::invalid("Die Farbe muss #RRGGBB sein.", "The colour must be #RRGGBB."));
        }
        let years = || StoreError::invalid(format!("Datum außerhalb von {}–{}.", super::MIN_YEAR, super::MAX_YEAR), format!("Date outside {}-{}.", super::MIN_YEAR, super::MAX_YEAR));
        if !super::year_ok(self.start.date()) {
            return Err(years());
        }
        if self.minutes < 0 || self.minutes > MAX_MINUTES {
            return Err(StoreError::invalid("Die Dauer muss zwischen 0 und 366 Tagen liegen.", "The duration must be between 0 and 366 days."));
        }
        if self.all_day {
            self.start = self.start.date().and_time(chrono::NaiveTime::MIN);
            self.minutes = ((self.minutes + 1439) / 1440).max(1) * 1440;
        }
        if let Some(r) = self.reminder
            && !(0..=MAX_REMINDER).contains(&r) {
                return Err(StoreError::invalid("Die Erinnerung muss zwischen 0 Minuten und 4 Wochen vorher liegen.", "The reminder must be between 0 minutes and 4 weeks before."));
            }
        if let Some(rec) = &mut self.recur {
            let c = super::cron::Cron::parse(&rec.cron).map_err(|e| StoreError::invalid(format!("Wiederholung: {}", e.de), format!("Recurrence: {}", e.en)))?;
            rec.cron = c.text;
            if rec.count.is_some_and(|n| n == 0 || n > MAX_COUNT) {
                return Err(StoreError::invalid(format!("Die Anzahl muss zwischen 1 und {MAX_COUNT} liegen."), format!("The count must be between 1 and {MAX_COUNT}.")));
            }
            if rec.until.is_some_and(|u| !super::year_ok(u)) || rec.except.iter().any(|d| !super::year_ok(*d)) {
                return Err(years());
            }
            if rec.until.is_some_and(|u| u < self.start.date()) {
                return Err(StoreError::invalid("Das Ende der Wiederholung liegt vor dem Beginn.", "The end of the recurrence is before the start."));
            }
            rec.except.sort();
            rec.except.dedup();
            if rec.except.len() > MAX_EXCEPTIONS {
                return Err(StoreError::invalid(format!("Höchstens {MAX_EXCEPTIONS} Ausnahmen."), format!("At most {MAX_EXCEPTIONS} exceptions.")));
            }
        }
        Ok(self)
    }
}

/// `#RRGGBB`?
pub fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Neue eindeutige Kennung für einen Termin (iCal `UID`).
pub fn new_uid() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        let n = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u128;
        b = n.to_le_bytes();
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{hex}@paneldeck")
}

fn now_text() -> String {
    super::fmt_stored(chrono::Local::now().naive_local())
}

const COLS: &str = "id, uid, title, note, color, all_day, start, minutes, cron, rec_until, rec_count, exceptions, reminder, project_id, origin, created_at, updated_at";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Event> {
    let start: String = r.get(6)?;
    let cron: Option<String> = r.get(8)?;
    let until: Option<String> = r.get(9)?;
    let count: Option<i64> = r.get(10)?;
    let exc: String = r.get(11)?;
    let recur = cron.filter(|c| !c.trim().is_empty()).map(|cron| Recurrence {
        cron,
        until: until.as_deref().and_then(super::parse_date),
        count: count.and_then(|n| u32::try_from(n).ok()),
        except: parse_exceptions(&exc),
    });
    Ok(Event {
        id: r.get(0)?,
        uid: r.get(1)?,
        title: r.get(2)?,
        note: r.get(3)?,
        color: r.get(4)?,
        all_day: r.get::<_, i64>(5)? != 0,
        start: super::parse_when(&start).map(|(t, _)| t).unwrap_or_default(),
        minutes: r.get(7)?,
        recur,
        reminder: r.get(12)?,
        project_id: r.get(13)?,
        origin: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

fn parse_exceptions(s: &str) -> Vec<NaiveDate> {
    let mut v: Vec<NaiveDate> = serde_json::from_str::<Vec<String>>(s).unwrap_or_default().iter().filter_map(|d| super::parse_date(d)).collect();
    v.sort();
    v.dedup();
    v
}

fn exceptions_json(v: &[NaiveDate]) -> String {
    serde_json::to_string(&v.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into())
}

/// Termin anlegen (Herkunft `origin`: `local`, `import`, `agent`); `uid` = `None` erzeugt eine neue Kennung.
pub fn create_event(conn: &Connection, input: EventInput, uid: Option<&str>, origin: &str) -> SRes<Event> {
    let i = input.validate()?;
    let uid = uid.map(str::to_string).unwrap_or_else(new_uid);
    let now = now_text();
    let (cron, until, count, exc) = rec_cols(&i.recur);
    let end = series_end(&i);
    let project = i.project_id.filter(|p| project_exists(conn, *p));
    conn.execute(
        "INSERT INTO cal_events (uid, title, note, color, all_day, start, minutes, cron, rec_until, rec_count, rec_end, exceptions, reminder, project_id, origin, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
        params![uid, i.title, i.note, i.color, i.all_day as i64, super::fmt_stored(i.start), i.minutes, cron, until, count, end, exc, i.reminder, project, origin, now],
    )?;
    get_event(conn, conn.last_insert_rowid())
}

/// Letzter Tag, an dem ein Vorkommen der Serie beginnen kann (`rec_end`): das Ende bzw. das letzte gezählte Vorkommen
/// (in UTC gerechnet, die Abfrage gibt einen Tag Spielraum); `None` = endlos. So fallen abgelaufene Serien schon in der
/// Datenbank-Abfrage weg und werden nicht bei jedem Neuladen neu gezählt.
fn series_end(i: &EventInput) -> Option<String> {
    let r = i.recur.as_ref()?;
    let by_count = r.count.and_then(|n| {
        let cron = super::cron::Cron::parse(&r.cron).ok()?;
        let utc = super::zone::RuleZone::fixed("UTC", 0);
        let mut last = i.start.date();
        if i.all_day {
            let mut d = i.start.date();
            let mut k = 0;
            // höchstens gut 30 Jahre suchen
            for _ in 0..(366 * 31) {
                if cron.day_matches(d) {
                    last = d;
                    k += 1;
                    if k >= n {
                        break;
                    }
                }
                d = super::add_days(d, 1);
            }
        } else {
            let mut t = super::add_minutes(i.start, -1);
            for _ in 0..n {
                let Some((wall, _)) = cron.next_in(t, &utc) else { break };
                last = wall.date();
                t = wall;
            }
        }
        Some(last)
    });
    let end = match (by_count, r.until) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    end.map(|d| d.format("%Y-%m-%d").to_string())
}

fn rec_cols(r: &Option<Recurrence>) -> (Option<String>, Option<String>, Option<i64>, String) {
    match r {
        Some(r) => (Some(r.cron.clone()), r.until.map(|d| d.format("%Y-%m-%d").to_string()), r.count.map(|n| n as i64), exceptions_json(&r.except)),
        None => (None, None, None, "[]".into()),
    }
}

fn project_exists(conn: &Connection, id: i64) -> bool {
    conn.query_row("SELECT 1 FROM projects WHERE id = ?1", [id], |_| Ok(())).optional().ok().flatten().is_some()
}

/// Termin ändern (alle Felder aus `input`).
pub fn update_event(conn: &Connection, id: i64, input: EventInput) -> SRes<Event> {
    let i = input.validate()?;
    let (cron, until, count, exc) = rec_cols(&i.recur);
    let end = series_end(&i);
    let project = i.project_id.filter(|p| project_exists(conn, *p));
    let n = conn.execute(
        "UPDATE cal_events SET title = ?2, note = ?3, color = ?4, all_day = ?5, start = ?6, minutes = ?7, cron = ?8, rec_until = ?9, rec_count = ?10,
         exceptions = ?11, reminder = ?12, project_id = ?13, updated_at = ?14, rec_end = ?15 WHERE id = ?1",
        params![id, i.title, i.note, i.color, i.all_day as i64, super::fmt_stored(i.start), i.minutes, cron, until, count, exc, i.reminder, project, now_text(), end],
    )?;
    if n == 0 {
        return Err(StoreError::NotFound);
    }
    get_event(conn, id)
}

/// Termin löschen (nur in der Oberfläche; Agenten können nicht löschen).
pub fn delete_event(conn: &Connection, id: i64) -> SRes<()> {
    if conn.execute("DELETE FROM cal_events WHERE id = ?1", [id])? == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

/// Ein Vorkommen einer Serie auslassen (Ausnahme für den Tag `day`).
pub fn skip_occurrence(conn: &Connection, id: i64, day: NaiveDate) -> SRes<Event> {
    let e = get_event(conn, id)?;
    let mut i = EventInput::from_event(&e);
    match &mut i.recur {
        Some(r) => r.except.push(day),
        None => return Err(StoreError::invalid("Der Termin wiederholt sich nicht.", "The event does not repeat.")),
    }
    update_event(conn, id, i)
}

pub fn get_event(conn: &Connection, id: i64) -> SRes<Event> {
    conn.query_row(&format!("SELECT {COLS} FROM cal_events WHERE id = ?1"), [id], from_row).optional()?.ok_or(StoreError::NotFound)
}

/// Gibt es einen Termin mit dieser `UID`?
pub fn uid_exists(conn: &Connection, uid: &str) -> SRes<bool> {
    Ok(conn.query_row("SELECT 1 FROM cal_events WHERE uid = ?1", [uid], |_| Ok(())).optional()?.is_some())
}

/// Alle Termine (für Export und Erinnerungen).
pub fn all_events(conn: &Connection) -> SRes<Vec<Event>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM cal_events ORDER BY start, id"))?;
    Ok(st.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Termine, die sich mit `[from, to)` überschneiden können: Serien (immer, sie werden später nur im Zeitraum
/// ausgerechnet) und Einzeltermine, die vor `to` beginnen und nicht vor `from` minus der längsten Dauer enden.
pub fn events_for_range(conn: &Connection, from: NaiveDateTime, to: NaiveDateTime) -> SRes<Vec<Event>> {
    let earliest = super::fmt_stored(super::add_minutes(from, -MAX_MINUTES));
    // Serien, deren letztes mögliches Vorkommen (plus Dauer und einen Tag Spielraum) vor dem Zeitraum liegt, fallen weg.
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM cal_events WHERE (cron IS NOT NULL AND start < ?2 AND (rec_end IS NULL OR date(rec_end, '+' || (minutes / 1440 + 2) || ' days') >= ?3))
         OR (cron IS NULL AND start < ?2 AND start >= ?1) ORDER BY start, id"
    ))?;
    let rows = st.query_map(params![earliest, super::fmt_stored(to), from.format("%Y-%m-%d").to_string()], from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter(|e| e.recur.is_some() || e.end_of(e.start) > from || (e.minutes == 0 && e.start >= from)).collect())
}

/// Termine mit Erinnerung (für `remind::poll`).
pub fn events_with_reminder(conn: &Connection) -> SRes<Vec<Event>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM cal_events WHERE reminder IS NOT NULL ORDER BY start, id"))?;
    Ok(st.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

// ---- Einstellungen

/// Quellen-Schalter (global oder je Projekt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sources {
    /// Fälligkeiten von Tickets und Todo-Karten.
    pub due: bool,
    pub sprints: bool,
    /// Versionen und Meilensteine.
    pub versions: bool,
    /// Gebuchte Zeit (Zeiteinträge).
    pub time: bool,
}

impl Default for Sources {
    fn default() -> Self {
        Sources { due: true, sprints: false, versions: false, time: false }
    }
}

/// Ansicht beim Öffnen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    #[default]
    Month,
    Week,
    Day,
    List,
}

/// Optionen des Kalenders (Fenster *Optionen*).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Options {
    /// Standard für alle Projekte.
    pub sources: Sources,
    /// Abweichungen je Projekt (Projekt-Kennung; 0 = einfache Todo-Liste).
    pub per_project: BTreeMap<i64, Sources>,
    /// Erinnerungen an Termine (Standard aus).
    pub reminders: bool,
    /// Zuletzt gewählte Ansicht.
    pub view: ViewKind,
}

impl Options {
    /// Geltende Schalter eines Projekts (0 = einfache Todo-Liste).
    pub fn for_project(&self, id: i64) -> Sources {
        self.per_project.get(&id).copied().unwrap_or(self.sources)
    }
}

fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM cal_settings WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
}

fn set_setting(conn: &Connection, key: &str, value: &str) -> SRes<()> {
    conn.execute("INSERT INTO cal_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value])?;
    Ok(())
}

/// Optionen laden (tolerant: Unlesbares gilt als Standard).
pub fn load_options(conn: &Connection) -> Options {
    setting(conn, "options").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_options(conn: &Connection, o: &Options) -> SRes<()> {
    set_setting(conn, "options", &serde_json::to_string(o).unwrap_or_else(|_| "{}".into()))
}

/// Zeitpunkt (Unix-Sekunden) der letzten Erinnerungs-Prüfung (für `remind::poll`).
pub fn last_reminder_check(conn: &Connection) -> Option<i64> {
    setting(conn, "reminder_check").and_then(|s| s.parse().ok())
}

pub fn set_last_reminder_check(conn: &Connection, t: i64) -> SRes<()> {
    set_setting(conn, "reminder_check", &t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        super::super::parse_when(s).unwrap().0
    }

    #[test]
    fn create_read_update_skip_delete() {
        let c = crate::db::open_memory();
        let mut i = EventInput::new("  Standup ", dt("2026-11-23T09:00"), 15);
        i.recur = Some(Recurrence { cron: "0  9 * * MON-FRI".into(), until: None, count: Some(10), except: vec![] });
        i.reminder = Some(5);
        i.color = "#61AFEF".into();
        let e = create_event(&c, i, None, "local").unwrap();
        assert_eq!(e.title, "Standup");
        assert!(e.uid.ends_with("@paneldeck") && e.uid.len() > 20);
        assert_eq!(e.recur.as_ref().unwrap().cron, "0 9 * * MON-FRI", "Normalform");
        assert_eq!(get_event(&c, e.id).unwrap(), e);
        let e2 = skip_occurrence(&c, e.id, NaiveDate::from_ymd_opt(2026, 11, 24).unwrap()).unwrap();
        assert_eq!(e2.recur.as_ref().unwrap().except, [NaiveDate::from_ymd_opt(2026, 11, 24).unwrap()]);
        let mut u = EventInput::from_event(&e2);
        u.recur = None;
        let e3 = update_event(&c, e.id, u).unwrap();
        assert!(e3.recur.is_none());
        assert!(skip_occurrence(&c, e.id, NaiveDate::from_ymd_opt(2026, 11, 25).unwrap()).is_err());
        assert!(uid_exists(&c, &e.uid).unwrap());
        delete_event(&c, e.id).unwrap();
        assert_eq!(get_event(&c, e.id), Err(StoreError::NotFound));
        assert_eq!(delete_event(&c, e.id), Err(StoreError::NotFound));
    }

    #[test]
    fn validation() {
        let c = crate::db::open_memory();
        let t = dt("2026-11-23T09:00");
        let bad = |f: &dyn Fn(&mut EventInput)| {
            let mut i = EventInput::new("x", t, 60);
            f(&mut i);
            create_event(&c, i, None, "local").unwrap_err()
        };
        assert!(matches!(bad(&|i| i.title = "   ".into()), StoreError::Invalid { .. }));
        assert!(matches!(bad(&|i| i.title = "x".repeat(256)), StoreError::Invalid { .. }));
        assert!(matches!(bad(&|i| i.color = "blue".into()), StoreError::Invalid { .. }));
        assert!(matches!(bad(&|i| i.minutes = -1), StoreError::Invalid { .. }));
        assert!(matches!(bad(&|i| i.reminder = Some(-5)), StoreError::Invalid { .. }));
        assert!(matches!(bad(&|i| i.recur = Some(Recurrence { cron: "nope".into(), ..Default::default() })), StoreError::Invalid { .. }));
        assert!(matches!(bad(&|i| i.recur = Some(Recurrence { cron: "@daily".into(), count: Some(0), ..Default::default() })), StoreError::Invalid { .. }));
        assert!(matches!(bad(&|i| i.recur = Some(Recurrence { cron: "@daily".into(), until: NaiveDate::from_ymd_opt(2026, 1, 1), ..Default::default() })), StoreError::Invalid { .. }));
        // ganztägig: auf volle Tage
        let mut i = EventInput::new("Urlaub", dt("2026-11-23T13:00"), 3000);
        i.all_day = true;
        let e = create_event(&c, i, None, "local").unwrap();
        assert_eq!((e.start, e.minutes, e.days()), (dt("2026-11-23"), 3 * 1440, 3));
        // unbekanntes Projekt wird nicht gespeichert
        let mut i = EventInput::new("x", t, 60);
        i.project_id = Some(999);
        assert_eq!(create_event(&c, i, None, "local").unwrap().project_id, None);
        let msg = StoreError::invalid("de", "en");
        assert_eq!(msg.message(crate::state::Lang::De), "de");
    }

    #[test]
    fn range_query_finds_overlapping_and_series() {
        let c = crate::db::open_memory();
        let mk = |t: &str, m: i64, cron: Option<&str>| {
            let mut i = EventInput::new(t, dt(t), m);
            i.recur = cron.map(|c| Recurrence { cron: c.into(), ..Default::default() });
            create_event(&c, i, None, "local").unwrap().id
        };
        let before = mk("2026-10-01T09:00", 60, None);
        let long = mk("2026-10-30T09:00", 5 * 1440, None);
        let inside = mk("2026-11-10T09:00", 0, None);
        let series = mk("2026-01-05T09:00", 30, Some("0 9 * * MON"));
        let after = mk("2026-12-05T09:00", 60, None);
        let ids: Vec<i64> = events_for_range(&c, dt("2026-11-01"), dt("2026-12-01")).unwrap().iter().map(|e| e.id).collect();
        assert!(ids.contains(&long) && ids.contains(&inside) && ids.contains(&series));
        assert!(!ids.contains(&before) && !ids.contains(&after));
    }

    #[test]
    fn options_default_and_tolerant() {
        let c = crate::db::open_memory();
        let o = load_options(&c);
        assert_eq!(o.sources, Sources { due: true, sprints: false, versions: false, time: false }, "beim ersten Öffnen: nur Fälligkeiten");
        assert!(!o.reminders, "Erinnerungen an Termine sind aus");
        let mut o2 = o.clone();
        o2.per_project.insert(5, Sources { due: false, sprints: true, versions: true, time: true });
        o2.view = ViewKind::Week;
        save_options(&c, &o2).unwrap();
        assert_eq!(load_options(&c), o2);
        assert!(o2.for_project(5).sprints);
        assert_eq!(o2.for_project(6), o2.sources);
        c.execute("UPDATE cal_settings SET value = '{\"reminders\": true, \"view\": \"bogus\"' WHERE key = 'options'", []).unwrap();
        assert_eq!(load_options(&c), Options::default(), "kaputt → Standard");
        c.execute("UPDATE cal_settings SET value = '{\"reminders\": true, \"extra\": 1}' WHERE key = 'options'", []).unwrap();
        assert!(load_options(&c).reminders, "Unbekanntes wird ignoriert");
        assert_eq!(last_reminder_check(&c), None);
        set_last_reminder_check(&c, 1234).unwrap();
        assert_eq!(last_reminder_check(&c), Some(1234));
    }
}
