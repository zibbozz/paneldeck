//! Reiter „Zeit“ (#164, ab 3.2.0) ohne Oberfläche: Wecker, Timer, Stoppuhren, Weltuhren, Geburtstage, die Benachrichtigungen
//! (`notify`), die mitgelieferten Klänge (`sounds`), die Einstellungen (`settings`), die Speicherung in `paneldeck.db`
//! (`store`) und die MCP-Werkzeuge (`agent`). Die Oberfläche liegt in `app/time_ui/`; der Kalender kommt als eigener Teil dazu.
//!
//! Zeiten werden als Unix-Zeit gespeichert (Wecker in Sekunden, Timer und Stoppuhren in Millisekunden). Alles, was von der
//! Uhr abhängt, bekommt „jetzt“ und die Zeitzone als Parameter (in der Anwendung `chrono::Local`, in Tests eine feste Zone
//! wie `Europe/Berlin`) – so lassen sich Sommerzeit, 29. Februar und „nach einem Neustart“ ohne echte Uhr prüfen.
//!
//! Es gibt keinen Hintergrunddienst: Wecker klingeln, solange Paneldeck läuft (auch minimiert oder im Tray). Was in der Zeit
//! dazwischen fällig war (Programm beendet, Rechner im Ruhezustand), meldet [`engine::tick`] beim nächsten Mal als verpasst.

pub mod agent;
pub mod alarm;
pub mod birthday;
pub mod engine;
pub mod notify;
pub mod settings;
pub mod sounds;
pub mod stopwatch;
pub mod store;
pub mod timer;
pub mod worldclock;

pub use alarm::Alarm;
pub use birthday::Birthday;
pub use stopwatch::Stopwatch;
pub use timer::{Preset, Timer, TimerState};
pub use worldclock::WorldClock;

use crate::state::Lang;

/// Alle Einträge des Reiters (im Speicher; jede Änderung geht sofort über `store` in die Datenbank).
#[derive(Clone, Debug, Default)]
pub struct TimeData {
    pub alarms: Vec<Alarm>,
    pub timers: Vec<Timer>,
    pub presets: Vec<Preset>,
    pub stopwatches: Vec<Stopwatch>,
    pub clocks: Vec<WorldClock>,
    pub birthdays: Vec<Birthday>,
    /// Nächster Termin je Wecker (nur im Speicher, `engine`): Kennung → (Prüfwert der Einstellungen, berechnet um, Termin).
    /// Die lokale Zeitzone abzufragen ist teuer (unter Windows je Aufruf eine Systemfunktion) – ohne Zwischenspeicher
    /// kosteten 1000 Wecker bei jedem Takt einen spürbaren Teil eines Bilds.
    pub next_cache: std::collections::HashMap<i64, (u64, i64, Option<i64>)>,
}

impl TimeData {
    pub fn alarm(&self, id: i64) -> Option<&Alarm> {
        self.alarms.iter().find(|a| a.id == id)
    }
    pub fn alarm_mut(&mut self, id: i64) -> Option<&mut Alarm> {
        self.alarms.iter_mut().find(|a| a.id == id)
    }
    pub fn timer(&self, id: i64) -> Option<&Timer> {
        self.timers.iter().find(|t| t.id == id)
    }
    pub fn timer_mut(&mut self, id: i64) -> Option<&mut Timer> {
        self.timers.iter_mut().find(|t| t.id == id)
    }
    pub fn stopwatch(&self, id: i64) -> Option<&Stopwatch> {
        self.stopwatches.iter().find(|s| s.id == id)
    }
    pub fn stopwatch_mut(&mut self, id: i64) -> Option<&mut Stopwatch> {
        self.stopwatches.iter_mut().find(|s| s.id == id)
    }
    pub fn clock(&self, id: i64) -> Option<&WorldClock> {
        self.clocks.iter().find(|c| c.id == id)
    }
    pub fn birthday(&self, id: i64) -> Option<&Birthday> {
        self.birthdays.iter().find(|b| b.id == id)
    }
}

/// Zeitzone des Rechners als IANA-Zone (`chrono-tz`, reines Rust), höchstens eine Minute alt; `None`, wenn der Name nicht
/// bekannt ist (dann gilt `chrono::Local`). Für den Takt: `chrono::Local` fragt unter Windows bei jeder Umrechnung das
/// System, was bei 1000 Weckern spürbar dauert.
pub fn system_tz() -> Option<chrono_tz::Tz> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Option<chrono_tz::Tz>)>> = Mutex::new(None);
    let mut c = CACHE.lock().ok()?;
    if let Some((at, tz)) = *c
        && at.elapsed() < Duration::from_secs(60)
    {
        return tz;
    }
    let tz = iana_time_zone::get_timezone().ok().and_then(|n| worldclock::parse_zone(&n));
    *c = Some((Instant::now(), tz));
    tz
}

/// Takt mit der schnellen Zone, falls bekannt (siehe [`system_tz`]).
pub fn tick_local(data: &mut TimeData, now_ms: i64) -> (Vec<engine::Event>, engine::Changed) {
    match system_tz() {
        Some(tz) => engine::tick(data, now_ms, &tz),
        None => engine::tick(data, now_ms, &chrono::Local),
    }
}

/// Nächster Weckzeitpunkt mit der schnellen Zone, falls bekannt.
pub fn next_wake_local(data: &TimeData, now_ms: i64) -> Option<i64> {
    match system_tz() {
        Some(tz) => engine::next_wake(data, now_ms, &tz),
        None => engine::next_wake(data, now_ms, &chrono::Local),
    }
}

/// Jetzt als Unix-Zeit in Millisekunden.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Dauer als `H:MM:SS` bzw. `M:SS` (unter einer Stunde); negative Werte zählen als 0. `tenths`: mit Zehntelsekunden.
pub fn fmt_hms(ms: i64, tenths: bool) -> String {
    let ms = ms.max(0);
    let s = ms / 1000;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    let base = if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") };
    if tenths { format!("{base}.{}", (ms % 1000) / 100) } else { base }
}

/// Kurze Dauer in Worten für Meldungen: „4 min“, „1 h 30 min“, „45 s“.
pub fn fmt_duration_short(ms: i64) -> String {
    let s = (ms.max(0) + 500) / 1000;
    if s < 60 {
        return format!("{s} s");
    }
    let m = (s + 30) / 60;
    if m < 60 {
        return format!("{m} min");
    }
    if m % 60 == 0 { format!("{} h", m / 60) } else { format!("{} h {} min", m / 60, m % 60) }
}

/// Liest eine Dauer: `90` (Sekunden), `4m`, `4 min`, `1h 30m`, `1:30` (M:SS), `1:00:00` (H:MM:SS), `25s`. Ergebnis in
/// Millisekunden, höchstens 100 Stunden; `None` bei unlesbarer oder leerer Eingabe bzw. 0.
pub fn parse_duration_ms(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let max = 100 * 3600 * 1000;
    let ok = |ms: i64| (ms > 0 && ms <= max).then_some(ms);
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        let nums: Option<Vec<i64>> = parts.iter().map(|p| p.trim().parse::<i64>().ok().filter(|n| *n >= 0)).collect();
        let nums = nums?;
        let secs = match nums.as_slice() {
            [m, sec] if *sec < 60 => m * 60 + sec,
            [h, m, sec] if *m < 60 && *sec < 60 => h * 3600 + m * 60 + sec,
            _ => return None,
        };
        return ok(secs.checked_mul(1000)?);
    }
    if let Ok(n) = s.parse::<i64>() {
        return ok(n.checked_mul(1000)?);
    }
    // Zahl + Einheit, mehrfach: „1h 30m“, „1h30m“, „4 min“, „2,5 min“ nicht (nur ganze Zahlen)
    let mut total: i64 = 0;
    let mut num = String::new();
    let mut unit = String::new();
    let mut any = false;
    let flush = |num: &mut String, unit: &mut String, total: &mut i64| -> Option<()> {
        if num.is_empty() {
            return if unit.trim().is_empty() { Some(()) } else { None };
        }
        let n: i64 = num.parse().ok()?;
        let mul = match unit.trim() {
            "h" | "hr" | "hrs" | "hour" | "hours" | "std" | "stunde" | "stunden" => 3600,
            "m" | "min" | "mins" | "minute" | "minutes" | "minuten" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" | "sek" | "sekunde" | "sekunden" => 1,
            _ => return None,
        };
        *total = total.checked_add(n.checked_mul(mul)?)?;
        num.clear();
        unit.clear();
        Some(())
    };
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !unit.trim().is_empty() {
                flush(&mut num, &mut unit, &mut total)?;
            }
            unit.clear();
            num.push(c);
            any = true;
        } else if c.is_alphabetic() {
            if num.is_empty() {
                return None;
            }
            unit.push(c);
        } else if c.is_whitespace() {
            if !unit.is_empty() {
                unit.push(' ');
            }
        } else {
            return None;
        }
    }
    flush(&mut num, &mut unit, &mut total)?;
    if !any {
        return None;
    }
    ok(total.checked_mul(1000)?)
}

/// Kurzname eines Wochentags (Montag = 0).
pub fn weekday_short(i: u32, lang: Lang) -> &'static str {
    const DE: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];
    const EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let i = (i % 7) as usize;
    if lang == Lang::De { DE[i] } else { EN[i] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_format_and_parse() {
        assert_eq!(fmt_hms(0, false), "0:00");
        assert_eq!(fmt_hms(65_000, false), "1:05");
        assert_eq!(fmt_hms(3_725_400, true), "1:02:05.4");
        assert_eq!(fmt_hms(-5, false), "0:00");
        assert_eq!(fmt_duration_short(240_000), "4 min");
        assert_eq!(fmt_duration_short(5_400_000), "1 h 30 min");
        assert_eq!(fmt_duration_short(3_600_000), "1 h");
        assert_eq!(fmt_duration_short(45_000), "45 s");
        assert_eq!(parse_duration_ms("90"), Some(90_000));
        assert_eq!(parse_duration_ms("4m"), Some(240_000));
        assert_eq!(parse_duration_ms("4 min"), Some(240_000));
        assert_eq!(parse_duration_ms("1h 30m"), Some(5_400_000));
        assert_eq!(parse_duration_ms("1h30m"), Some(5_400_000));
        assert_eq!(parse_duration_ms("1:30"), Some(90_000));
        assert_eq!(parse_duration_ms("1:00:00"), Some(3_600_000));
        assert_eq!(parse_duration_ms("25s"), Some(25_000));
        assert_eq!(parse_duration_ms("2 Minuten"), Some(120_000));
        for bad in ["", "0", "abc", "1:75", "-5", "5x", "m5", "101h", "1,5h"] {
            assert_eq!(parse_duration_ms(bad), None, "{bad}");
        }
    }

    /// Die schnelle Zone stimmt mit `chrono::Local` überein (sonst rechnete der Takt mit einer falschen Zone).
    #[test]
    fn system_zone_matches_local_time() {
        use chrono::{Offset, TimeZone};
        if let Some(tz) = system_tz() {
            for ms in [now_ms(), now_ms() + 90 * 86_400_000, now_ms() + 180 * 86_400_000] {
                let a = tz.timestamp_millis_opt(ms).unwrap().offset().fix();
                let b = *chrono::Local.timestamp_millis_opt(ms).unwrap().offset();
                assert_eq!(a.local_minus_utc(), b.local_minus_utc(), "{tz}");
            }
        }
    }

    #[test]
    fn weekday_names_in_both_languages() {
        assert_eq!(weekday_short(0, Lang::En), "Mon");
        assert_eq!(weekday_short(6, Lang::De), "So");
    }
}
