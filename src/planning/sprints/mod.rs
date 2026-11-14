//! Sprints, Versionen und Meilensteine: reine Datenschicht auf `&Connection` ohne Oberfläche (Stil wie `tickets.rs`).
//!
//! * `sprint` – Sprints (anlegen mit Dauer-Vorlage, ändern, löschen, starten, abschließen), Zugehörigkeit der Tickets
//!   (verschieben, Backlog, Inhalt mit Summen), `sprint_of` und `sprint_ids_active` (Haken für die Suche, z. B. `activeSprint()`).
//! * `analysis` – Burndown aus den echten Ereignissen (Zugehörigkeit + Status-Verlauf), Sprint-Bericht, Velocity.
//! * `releases` – Versionen und Meilensteine (Verwaltung, Fortschritt, Löschen mit Ersatz) und die Prüfung der Feldwerte.
//! * `error` – [`SprintError`] (zweisprachig).
//!
//! # Alles ist optional
//!
//! Die Projekteinstellungen `use_sprints` und `use_versions` (Versionen **und** Meilensteine) schalten die Schicht je Projekt
//! ein. Ist ein Schalter aus, lehnen alle Funktionen, die ein Projekt oder einen Sprint/eine Version bearbeiten oder abfragen,
//! mit [`SprintError::SprintsDisabled`] bzw. [`SprintError::VersionsDisabled`] ab; die Daten bleiben erhalten und sind nach dem
//! Wiedereinschalten wieder da. Ausgenommen sind die einfachen Leser (`get_sprint`, `get_version`, `get_milestone`) – damit lassen sich
//! vorhandene Feldwerte weiter anzeigen –, `sprint_of` und die Suchhaken (`active_sprint`, `sprint_ids_active`), die ohne Sprints
//! nichts liefern.
//!
//! # Zeit
//!
//! Zeitpunkte („jetzt“) stammen aus `common::now()` (lokale Zeit als ISO-Text, wie überall in `planning`); in Tests lässt sich die
//! Uhr über [`clock`] festlegen. Die Berechnungen in `analysis` bekommen „heute“ als Parameter.

// Die Schicht ist vollständig; Oberfläche und Suche (andere Schritte) nutzen sie erst nach und nach.
#![allow(dead_code)]

pub mod analysis;
pub mod error;
pub mod releases;
pub mod schema;
pub mod sprint;

#[cfg(test)]
mod tests_analysis;
#[cfg(test)]
mod tests_releases;
#[cfg(test)]
mod tests_sprint;

// Wiederausgabe für Oberfläche und Suche, die später darauf zugreifen.
#[allow(unused_imports)]
pub use analysis::{burndown_for_sprint, sprint_report, velocity_for_project, ReportEntry, SprintReport};
pub use error::{SRes, SprintError};
#[allow(unused_imports)]
pub use releases::{Release, ReleaseKind, ReleaseProgress};
#[allow(unused_imports)]
pub use sprint::{Leftover, Sprint, SprintContent, SprintItem, SprintState, SprintTotals};

use crate::calc::progress::Basis;
use crate::planning::projects::{get_project, Project};
use chrono::NaiveDate;
use rusqlite::Connection;

/// Format der Datumsangaben in der Datenbank.
pub const DATE_FORMAT: &str = "%Y-%m-%d";

/// Standarddauer eines Sprints in Arbeitstagen (zwei Wochen), wenn nichts anderes angegeben ist.
pub const DEFAULT_SPRINT_DAYS: u32 = 10;

/// Höchste Dauer-Vorlage in Arbeitstagen.
pub const MAX_SPRINT_DAYS: u32 = 366;

/// Längster Text für das Sprintziel in Zeichen.
pub const MAX_GOAL_CHARS: usize = 500;

/// Längste Beschreibung einer Version/eines Meilensteins in Zeichen.
pub const MAX_DESCRIPTION_CHARS: usize = 2000;

/// Umfang einer Menge von Tickets: Anzahl und Story Points. Welche Zahl zählt, bestimmt die [`Basis`] (`weight`), die
/// Punkte-Summe zählt nur Tickets **mit** Punkten (`without_points` sind die übrigen).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Figure {
    /// Zahl der Tickets.
    pub tickets: u32,
    /// Summe der Story Points der Tickets, die welche haben.
    pub points: f64,
    /// Tickets ohne Story Points.
    pub without_points: u32,
}

impl Figure {
    /// Ein Ticket dazuzählen. Ungültige Punkte (negativ, keine Zahl) gelten wie bei [`Basis`] als „keine Punkte“.
    pub fn add(&mut self, points: Option<f64>) {
        self.tickets += 1;
        match points.filter(|p| p.is_finite() && *p >= 0.0) {
            Some(p) => self.points += p,
            None => self.without_points += 1,
        }
    }

    /// Gewicht nach `basis`: nur Punkte, Anzahl, oder Punkte mit „ohne Punkte = 1“.
    pub fn weight(&self, basis: Basis) -> f64 {
        match basis {
            Basis::Points => self.points,
            Basis::TicketCount => self.tickets as f64,
            Basis::PointsWithFallbackOne => self.points + self.without_points as f64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tickets == 0
    }
}

/// Datum aus der Datenbank bzw. Eingabe (`JJJJ-MM-TT`).
pub fn parse_date(s: &str) -> SRes<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), DATE_FORMAT).map_err(|_| SprintError::DateInvalid)
}

pub(crate) fn fmt_date(d: NaiveDate) -> String {
    d.format(DATE_FORMAT).to_string()
}

/// Das Projekt, falls es Sprints nutzt.
pub(crate) fn sprint_project(conn: &Connection, project_id: i64) -> SRes<Project> {
    let p = get_project(conn, project_id)?;
    if p.use_sprints { Ok(p) } else { Err(SprintError::SprintsDisabled) }
}

/// Das Projekt, falls es Versionen/Meilensteine nutzt.
pub(crate) fn release_project(conn: &Connection, project_id: i64) -> SRes<Project> {
    let p = get_project(conn, project_id)?;
    if p.use_versions { Ok(p) } else { Err(SprintError::VersionsDisabled) }
}

/// Führt `f` atomar aus (SAVEPOINT): bei einem Fehler wird alles zurückgenommen. Lässt sich verschachteln und in einer
/// äußeren Transaktion benutzen.
pub(crate) fn atomic<T>(conn: &Connection, f: impl FnOnce() -> SRes<T>) -> SRes<T> {
    conn.execute_batch("SAVEPOINT sprint_op")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("RELEASE sprint_op")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO sprint_op; RELEASE sprint_op");
            Err(e)
        }
    }
}

/// Stellt sicher, dass kein anderer Eintrag der Tabelle im Projekt (außer `exclude`) denselben Namen trägt (ohne Beachtung der Groß-/Kleinschreibung).
pub(crate) fn ensure_name_free(conn: &Connection, table: &str, project_id: i64, name: &str, exclude: Option<i64>) -> SRes<()> {
    let mut st = conn.prepare(&format!("SELECT id, name FROM {table} WHERE project_id = ?1"))?;
    let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let want = name.to_lowercase();
    if rows.iter().any(|(id, n)| Some(*id) != exclude && n.to_lowercase() == want) {
        return Err(SprintError::Plan(crate::planning::PlanError::NameTaken));
    }
    Ok(())
}

/// Zeitgeber der Schicht. In Tests lässt sich „jetzt“ festlegen, damit Zeitpunkte und Berechnungen deterministisch sind.
pub mod clock {
    use chrono::NaiveDate;

    #[cfg(test)]
    thread_local! {
        static FIXED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    /// Jetzt als `JJJJ-MM-TTTHH:MM:SS` (lokale Zeit).
    pub fn now() -> String {
        #[cfg(test)]
        if let Some(t) = FIXED.with(|f| f.borrow().clone()) {
            return t;
        }
        crate::planning::common::now()
    }

    /// Heute (aus [`now`]).
    pub fn today() -> NaiveDate {
        let n = now();
        NaiveDate::parse_from_str(&n[..10], super::DATE_FORMAT).unwrap_or_default()
    }

    /// Nur für Tests: legt „jetzt“ für diesen Thread fest.
    #[cfg(test)]
    pub fn set(t: &str) {
        FIXED.with(|f| *f.borrow_mut() = Some(t.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figure_adds_up_by_basis() {
        let mut f = Figure::default();
        assert!(f.is_empty());
        f.add(Some(3.0));
        f.add(Some(0.0));
        f.add(None);
        f.add(Some(-2.0));
        f.add(Some(f64::NAN));
        assert_eq!((f.tickets, f.without_points), (5, 3));
        assert_eq!(f.points, 3.0);
        assert_eq!(f.weight(Basis::Points), 3.0);
        assert_eq!(f.weight(Basis::TicketCount), 5.0);
        assert_eq!(f.weight(Basis::PointsWithFallbackOne), 6.0);
        assert!(!f.is_empty());
    }

    #[test]
    fn dates_parse_strictly() {
        assert_eq!(parse_date("2026-11-02").unwrap(), NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
        assert_eq!(parse_date(" 2026-11-02 ").unwrap(), NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
        for bad in ["", "2.11.2026", "2026-13-01", "2026-11-31", "heute"] {
            assert_eq!(parse_date(bad), Err(SprintError::DateInvalid), "{bad}");
        }
        assert_eq!(fmt_date(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()), "2026-01-05");
    }

    #[test]
    fn the_clock_can_be_fixed_in_tests() {
        clock::set("2026-11-02T09:30:00");
        assert_eq!(clock::now(), "2026-11-02T09:30:00");
        assert_eq!(clock::today(), NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
    }
}
