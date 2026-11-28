//! Timer (#164): mehrere benannte Countdowns gleichzeitig, Pause/Weiter/Neustart, gespeicherte Vorlagen („Tee 4 min“).
//!
//! Ein laufender Timer speichert seine **Endzeit** (`ends_at`, Unix-ms), ein angehaltener die Restzeit. So übersteht er
//! Neustart und Ruhezustand: Ist die Endzeit beim nächsten Blick vorbei, gilt er als abgelaufen (`Expired { at }`, in der
//! Oberfläche „abgelaufen um 14:32“). Klingeln gibt es nur, wenn das Ende höchstens `grace` zurückliegt (sonst lief es ab,
//! während Paneldeck zu war oder der Rechner schlief).

use crate::state::Lang;

pub const DEFAULT_SOUND: &str = "builtin:beep";
pub const MAX_NAME_CHARS: usize = 100;
/// Längste Dauer: 100 Stunden.
pub const MAX_DURATION_MS: i64 = 100 * 3600 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerState {
    /// Noch nicht gestartet bzw. zurückgesetzt.
    Idle,
    Running { ends_at: i64 },
    Paused { remaining_ms: i64 },
    /// Abgelaufen um `at` (Unix-ms).
    Expired { at: i64 },
}

impl TimerState {
    pub fn key(self) -> &'static str {
        match self {
            TimerState::Idle => "idle",
            TimerState::Running { .. } => "running",
            TimerState::Paused { .. } => "paused",
            TimerState::Expired { .. } => "expired",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timer {
    pub id: i64,
    pub name: String,
    pub duration_ms: i64,
    pub state: TimerState,
    pub sound: String,
    /// Ticket für die Zeiterfassung (z. B. `PAN-12`), leer = keine (Standard).
    pub ticket: String,
    /// Länge des aktuellen Laufs in ms (Dauer beim Start). Die Dauer darf sich während des Laufs ändern (gilt erst ab dem
    /// nächsten Start); die Zeiterfassung rechnet mit dieser Länge. Ältere Datenbestände ohne Wert: die Dauer.
    pub run_ms: i64,
}

impl Timer {
    pub fn new(name: &str, duration_ms: i64) -> Timer {
        let duration_ms = clamp_duration(duration_ms);
        Timer { id: 0, name: clean_name(name), duration_ms, state: TimerState::Idle, sound: DEFAULT_SOUND.into(), ticket: String::new(), run_ms: duration_ms }
    }

    pub fn display_name(&self, lang: Lang) -> String {
        if self.name.trim().is_empty() { lang.t("Timer", "Timer").to_string() } else { self.name.clone() }
    }

    /// Restzeit in ms zum Zeitpunkt `now`.
    pub fn remaining(&self, now: i64) -> i64 {
        match self.state {
            TimerState::Idle => self.duration_ms,
            TimerState::Running { ends_at } => (ends_at - now).max(0),
            TimerState::Paused { remaining_ms } => remaining_ms.max(0),
            TimerState::Expired { .. } => 0,
        }
    }

    /// Bisher abgelaufene Zeit dieses Laufs (für die Zeiterfassung) – gemessen an der Länge des Laufs, nicht an der
    /// (inzwischen evtl. geänderten) Dauer.
    pub fn worked(&self, now: i64) -> i64 {
        let run = self.run_ms.max(0);
        match self.state {
            TimerState::Idle => 0,
            TimerState::Expired { .. } => run,
            TimerState::Running { .. } | TimerState::Paused { .. } => (run - self.remaining(now)).clamp(0, run),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, TimerState::Running { .. })
    }

    /// Starten bzw. fortsetzen: aus „bereit“ oder „abgelaufen“ mit der vollen Dauer, aus „angehalten“ mit der Restzeit.
    /// Läuft er schon, bleibt alles, wie es ist.
    pub fn start(&mut self, now: i64) {
        self.state = match self.state {
            TimerState::Idle | TimerState::Expired { .. } => {
                self.run_ms = self.duration_ms;
                TimerState::Running { ends_at: now + self.duration_ms }
            }
            TimerState::Paused { remaining_ms } => TimerState::Running { ends_at: now + remaining_ms },
            s @ TimerState::Running { .. } => s,
        };
    }

    pub fn pause(&mut self, now: i64) {
        if let TimerState::Running { ends_at } = self.state {
            self.state = TimerState::Paused { remaining_ms: (ends_at - now).max(0) };
        }
    }

    /// Von vorn mit der vollen Dauer.
    pub fn restart(&mut self, now: i64) {
        self.run_ms = self.duration_ms;
        self.state = TimerState::Running { ends_at: now + self.duration_ms };
    }

    /// Zurück auf „bereit“.
    pub fn reset(&mut self) {
        self.state = TimerState::Idle;
    }

    /// Neue Dauer; ein laufender oder angehaltener Timer behält seine Restzeit (die neue Dauer gilt ab dem nächsten Start).
    pub fn set_duration(&mut self, ms: i64) {
        self.duration_ms = clamp_duration(ms);
    }

    /// Prüft das Ende zum Zeitpunkt `now`: `Some` beim Übergang nach „abgelaufen“ (`ring` = höchstens `grace_ms` her).
    pub fn check(&mut self, now: i64, grace_ms: i64) -> Option<Expiry> {
        if let TimerState::Running { ends_at } = self.state
            && ends_at <= now {
                self.state = TimerState::Expired { at: ends_at };
                return Some(Expiry { at: ends_at, ring: now - ends_at <= grace_ms });
            }
        None
    }
}

/// Übergang nach „abgelaufen“.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expiry {
    pub at: i64,
    pub ring: bool,
}

/// Gespeicherte Vorlage („Tee 4 min“): ein Klick legt einen Timer an und startet ihn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: i64,
    pub name: String,
    pub duration_ms: i64,
}

impl Preset {
    pub fn label(&self) -> String {
        format!("{} {}", self.name, super::fmt_duration_short(self.duration_ms))
    }
}

pub fn clamp_duration(ms: i64) -> i64 {
    ms.clamp(1000, MAX_DURATION_MS)
}

pub fn clean_name(s: &str) -> String {
    super::alarm::clean_name(s).chars().take(MAX_NAME_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_pause_resume_restart_reset() {
        let mut t = Timer::new("Tee", 240_000);
        assert_eq!(t.remaining(0), 240_000);
        t.start(1_000);
        assert_eq!(t.state, TimerState::Running { ends_at: 241_000 });
        assert_eq!(t.remaining(61_000), 180_000);
        t.pause(61_000);
        assert_eq!(t.state, TimerState::Paused { remaining_ms: 180_000 });
        assert_eq!(t.remaining(999_999), 180_000, "angehalten läuft nichts weiter");
        assert_eq!(t.worked(999_999), 60_000);
        t.start(100_000);
        assert_eq!(t.state, TimerState::Running { ends_at: 280_000 });
        t.start(120_000);
        assert_eq!(t.state, TimerState::Running { ends_at: 280_000 }, "Start auf einem laufenden Timer ändert nichts");
        t.restart(200_000);
        assert_eq!(t.state, TimerState::Running { ends_at: 440_000 });
        t.reset();
        assert_eq!(t.state, TimerState::Idle);
    }

    /// „Neustart“ des Programms: Der Timer wird aus Endzeit/Restzeit wiederhergestellt; abgelaufen während Paneldeck zu
    /// war → „abgelaufen um …“ ohne Klingeln, kurz davor → Klingeln.
    #[test]
    fn survives_restart_by_end_time_and_rings_only_within_grace() {
        let mut t = Timer::new("Pizza", 600_000);
        t.start(0);
        let mut restored = t.clone(); // so steht er in der Datenbank
        assert_eq!(restored.check(300_000, 120_000), None);
        assert_eq!(restored.remaining(300_000), 300_000);
        assert_eq!(restored.check(600_500, 120_000), Some(Expiry { at: 600_000, ring: true }));
        assert_eq!(restored.state, TimerState::Expired { at: 600_000 });
        assert_eq!(restored.check(700_000, 120_000), None, "nur einmal");
        let mut late = t.clone();
        assert_eq!(late.check(3_600_000, 120_000), Some(Expiry { at: 600_000, ring: false }));
        assert_eq!(late.worked(3_600_000), 600_000);
        late.start(4_000_000);
        assert_eq!(late.state, TimerState::Running { ends_at: 4_600_000 }, "abgelaufen → Start mit voller Dauer");
    }

    /// Dauer während des Laufs ändern: Restzeit bleibt, gebucht wird die tatsächlich gelaufene Zeit (Länge des Laufs
    /// minus Rest), nicht „neue Dauer minus Rest“.
    #[test]
    fn changing_the_duration_while_running_books_the_real_time() {
        // Erhöhen: 10 min gestartet, nach 4 min auf 30 min → 4 min gearbeitet (nicht 24)
        let mut t = Timer::new("Fokus", 600_000);
        t.start(0);
        t.set_duration(1_800_000);
        assert_eq!(t.remaining(240_000), 360_000, "Restzeit unverändert");
        assert_eq!(t.worked(240_000), 240_000);
        t.pause(240_000);
        assert_eq!(t.worked(999_999), 240_000);
        t.start(300_000);
        assert_eq!(t.worked(360_000), 300_000, "nach dem Fortsetzen weiter vom alten Lauf");
        assert_eq!(t.check(700_000, 1000).map(|e| e.at), Some(660_000));
        assert_eq!(t.worked(700_000), 600_000, "abgelaufen: der ganze Lauf");
        // Neuer Start: die neue Dauer gilt
        t.start(1_000_000);
        assert_eq!(t.worked(1_060_000), 60_000);
        assert_eq!(t.remaining(1_060_000), 1_740_000);
        // Senken: 30 min gestartet, nach 10 min auf 5 min → 10 min gearbeitet (nicht 0)
        let mut s = Timer::new("Lang", 1_800_000);
        s.start(0);
        s.set_duration(300_000);
        assert_eq!(s.worked(600_000), 600_000);
        s.pause(600_000);
        assert_eq!(s.worked(600_000), 600_000);
        s.restart(700_000);
        assert_eq!((s.run_ms, s.worked(760_000)), (300_000, 60_000));
        s.reset();
        assert_eq!(s.worked(800_000), 0, "bereit: nichts gelaufen");
    }

    #[test]
    fn duration_limits_and_names() {
        assert_eq!(Timer::new("", 0).duration_ms, 1000);
        assert_eq!(Timer::new("", i64::MAX).duration_ms, MAX_DURATION_MS);
        assert_eq!(Timer::new("", 1).display_name(Lang::En), "Timer");
        assert_eq!(Preset { id: 1, name: "Tea".into(), duration_ms: 240_000 }.label(), "Tea 4 min");
    }
}
