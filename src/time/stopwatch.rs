//! Stoppuhren (#164): mehrere gleichzeitig, mit Runden (Start/Stopp/Runde/Zurücksetzen, Rundenliste).
//!
//! Eine laufende Stoppuhr speichert ihren **Startzeitpunkt** (`started_at`, Unix-ms) und die bis dahin gesammelte Zeit
//! (`accumulated_ms`); so läuft sie über Neustart und Ruhezustand weiter. Runden sind die Gesamtzeiten beim Drücken von
//! „Runde“ (die Rundenzeit ist die Differenz zur vorigen).

use crate::state::Lang;

pub const MAX_NAME_CHARS: usize = 100;
/// Höchstens so viele Runden je Stoppuhr (danach zählt „Runde“ nicht mehr).
pub const MAX_LAPS: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Stopwatch {
    pub id: i64,
    pub name: String,
    /// Läuft seit (Unix-ms); `None` = angehalten.
    pub started_at: Option<i64>,
    pub accumulated_ms: i64,
    /// Gesamtzeit bei jeder Runde (aufsteigend).
    pub laps: Vec<i64>,
    /// Ticket für die Zeiterfassung, leer = keine (Standard).
    pub ticket: String,
    /// Schon gebuchte Zeit (die nächste Frage bietet nur den Rest an).
    pub booked_ms: i64,
}

impl Stopwatch {
    pub fn new(name: &str) -> Stopwatch {
        Stopwatch { name: super::timer::clean_name(name).chars().take(MAX_NAME_CHARS).collect(), ..Default::default() }
    }

    pub fn display_name(&self, lang: Lang) -> String {
        if self.name.trim().is_empty() { lang.t("Stoppuhr", "Stopwatch").to_string() } else { self.name.clone() }
    }

    pub fn running(&self) -> bool {
        self.started_at.is_some()
    }

    pub fn elapsed(&self, now: i64) -> i64 {
        self.accumulated_ms + self.started_at.map(|s| (now - s).max(0)).unwrap_or(0)
    }

    pub fn start(&mut self, now: i64) {
        if self.started_at.is_none() {
            self.started_at = Some(now);
        }
    }

    pub fn stop(&mut self, now: i64) {
        if self.started_at.is_some() {
            self.accumulated_ms = self.elapsed(now);
            self.started_at = None;
        }
    }

    /// Runde nehmen (nur während sie läuft); `false`, wenn sie steht oder die Liste voll ist.
    pub fn lap(&mut self, now: i64) -> bool {
        if !self.running() || self.laps.len() >= MAX_LAPS {
            return false;
        }
        self.laps.push(self.elapsed(now));
        true
    }

    pub fn reset(&mut self) {
        self.started_at = None;
        self.accumulated_ms = 0;
        self.laps.clear();
        self.booked_ms = 0;
    }

    /// Runden als (Nummer ab 1, Rundenzeit, Gesamtzeit), neueste zuerst.
    pub fn lap_rows(&self) -> Vec<(usize, i64, i64)> {
        let mut prev = 0;
        let mut v: Vec<(usize, i64, i64)> = self
            .laps
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let r = (i + 1, t - prev, t);
                prev = t;
                r
            })
            .collect();
        v.reverse();
        v
    }

    /// Noch nicht gebuchte Zeit.
    pub fn unbooked(&self, now: i64) -> i64 {
        (self.elapsed(now) - self.booked_ms).max(0)
    }
}

pub fn laps_to_json(laps: &[i64]) -> String {
    serde_json::to_string(laps).unwrap_or_else(|_| "[]".into())
}

/// Tolerant: Unlesbares ergibt eine leere Liste, Werte werden aufsteigend und nicht negativ gemacht.
pub fn laps_from_json(s: &str) -> Vec<i64> {
    let mut v: Vec<i64> = serde_json::from_str::<Vec<serde_json::Value>>(s).unwrap_or_default().iter().filter_map(|x| x.as_i64()).filter(|x| *x >= 0).take(MAX_LAPS).collect();
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_stop_lap_reset() {
        let mut s = Stopwatch::new("Lauf");
        assert!(!s.lap(0), "steht: keine Runde");
        s.start(1_000);
        assert_eq!(s.elapsed(11_000), 10_000);
        assert!(s.lap(11_000));
        assert!(s.lap(26_000));
        s.stop(30_000);
        assert_eq!(s.elapsed(99_000), 29_000, "angehalten zählt nicht weiter");
        s.start(40_000);
        assert_eq!(s.elapsed(41_000), 30_000);
        assert_eq!(s.lap_rows(), vec![(2, 15_000, 25_000), (1, 10_000, 10_000)]);
        s.reset();
        assert_eq!((s.elapsed(50_000), s.laps.len(), s.running()), (0, 0, false));
    }

    /// „Neustart“: Startzeit und gesammelte Zeit stehen in der Datenbank, die Uhr läuft weiter.
    #[test]
    fn survives_restart_by_start_time() {
        let mut s = Stopwatch::new("");
        s.start(0);
        s.stop(60_000);
        s.start(100_000);
        let restored = Stopwatch { laps: laps_from_json(&laps_to_json(&s.laps)), ..s.clone() };
        assert_eq!(restored.elapsed(3_700_000), 60_000 + 3_600_000);
        assert_eq!(laps_from_json("kaputt"), Vec::<i64>::new());
        assert_eq!(laps_from_json("[30, -1, 10, \"x\"]"), vec![10, 30]);
    }

    #[test]
    fn booking_offers_only_the_rest() {
        let mut s = Stopwatch::new("");
        s.start(0);
        s.stop(47 * 60_000);
        assert_eq!(s.unbooked(0), 47 * 60_000);
        s.booked_ms = s.elapsed(0);
        s.start(50 * 60_000);
        s.stop(60 * 60_000);
        assert_eq!(s.unbooked(0), 10 * 60_000);
    }
}
