//! Weltuhren (#164): Stadt/Zeitzone, Uhrzeit, Datum, Abstand zur lokalen Zeit; analog oder digital.
//! Zeitzonen aus der IANA-Datenbank (`chrono-tz`), Namen wie `America/New_York`.

use crate::state::Lang;
use chrono::{DateTime, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;

pub const MAX_LABEL_CHARS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldClock {
    pub id: i64,
    /// Eigene Beschriftung (z. B. „Büro NY“); leer = Stadt aus dem Zonennamen.
    pub label: String,
    /// IANA-Name, z. B. `Asia/Tokyo`.
    pub zone: String,
    pub analog: bool,
}

impl WorldClock {
    pub fn new(label: &str, zone: &str) -> WorldClock {
        WorldClock { id: 0, label: clean_label(label), zone: zone.to_string(), analog: false }
    }

    pub fn tz(&self) -> Option<Tz> {
        parse_zone(&self.zone)
    }

    pub fn title(&self) -> String {
        if self.label.trim().is_empty() { city_of(&self.zone) } else { self.label.clone() }
    }
}

/// Zone nach Namen (genau oder ohne Groß-/Kleinschreibung, Leerzeichen = `_`).
pub fn parse_zone(s: &str) -> Option<Tz> {
    let s = s.trim();
    if let Ok(t) = s.parse::<Tz>() {
        return Some(t);
    }
    let k = s.replace(' ', "_").to_lowercase();
    chrono_tz::TZ_VARIANTS.iter().copied().find(|t| t.name().to_lowercase() == k)
}

/// Alle bekannten Zonen.
pub fn all_zones() -> &'static [Tz] {
    &chrono_tz::TZ_VARIANTS
}

/// Zonen, deren Name `query` enthält (ohne Groß-/Kleinschreibung, Leerzeichen = `_`), höchstens `max`.
pub fn search_zones(query: &str, max: usize) -> Vec<Tz> {
    let q = query.trim().replace(' ', "_").to_lowercase();
    all_zones().iter().copied().filter(|t| q.is_empty() || t.name().to_lowercase().contains(&q)).take(max).collect()
}

/// Stadt aus dem Zonennamen: `America/New_York` → „New York“, `America/Argentina/Buenos_Aires` → „Buenos Aires“.
pub fn city_of(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

/// Was eine Uhr zeigt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockView {
    pub local: NaiveDateTime,
    /// Abstand der Zone zu UTC in Minuten.
    pub utc_offset_min: i32,
    /// Abstand der Zone zur lokalen Zeit in Minuten (positiv = dort später).
    pub offset_to_local_min: i32,
    /// Kürzel der Zone (z. B. `CET`, `EST`).
    pub abbrev: String,
}

pub fn view<L: TimeZone>(zone: Tz, now_ms: i64, local: &L) -> Option<ClockView> {
    let utc: DateTime<Utc> = Utc.timestamp_millis_opt(now_ms).single()?;
    let there = utc.with_timezone(&zone);
    let off = there.offset().fix().local_minus_utc() / 60;
    let here = local.offset_from_utc_datetime(&utc.naive_utc()).fix().local_minus_utc() / 60;
    Some(ClockView { local: there.naive_local(), utc_offset_min: off, offset_to_local_min: off - here, abbrev: there.format("%Z").to_string() })
}

/// Abstand als Text: „+1 h“, „−5:30 h“, „gleiche Zeit“.
pub fn fmt_offset(min: i32, lang: Lang) -> String {
    if min == 0 {
        return lang.t("gleiche Zeit", "same time").into();
    }
    let sign = if min > 0 { "+" } else { "−" };
    let a = min.abs();
    if a % 60 == 0 { format!("{sign}{} h", a / 60) } else { format!("{sign}{}:{:02} h", a / 60, a % 60) }
}

/// UTC-Abstand als `UTC+05:30`.
pub fn fmt_utc(min: i32) -> String {
    let sign = if min < 0 { '-' } else { '+' };
    format!("UTC{sign}{:02}:{:02}", min.abs() / 60, min.abs() % 60)
}

pub fn clean_label(s: &str) -> String {
    super::alarm::clean_name(s).chars().take(MAX_LABEL_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::Europe::Berlin;

    fn ms(y: i32, mo: u32, d: u32, h: u32) -> i64 {
        Utc.from_utc_datetime(&NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, 0, 0).unwrap()).timestamp_millis()
    }

    #[test]
    fn offsets_follow_daylight_saving_on_both_sides() {
        let ny = parse_zone("America/New_York").unwrap();
        // Winter: Berlin UTC+1, New York UTC−5 → −6 h
        let v = view(ny, ms(2026, 1, 15, 12), &Berlin).unwrap();
        assert_eq!((v.utc_offset_min, v.offset_to_local_min), (-300, -360));
        assert_eq!(v.local.format("%H:%M").to_string(), "07:00");
        // Zwischen den Umstellungen (USA schon Sommerzeit, Europa noch nicht): −5 h
        let v = view(ny, ms(2026, 3, 20, 12), &Berlin).unwrap();
        assert_eq!(v.offset_to_local_min, -300);
        let kol = parse_zone("asia/kolkata").unwrap();
        let v = view(kol, ms(2026, 7, 1, 0), &Berlin).unwrap();
        assert_eq!(v.offset_to_local_min, 210);
        assert_eq!(fmt_offset(210, Lang::En), "+3:30 h");
        assert_eq!(fmt_offset(-360, Lang::De), "−6 h");
        assert_eq!(fmt_offset(0, Lang::De), "gleiche Zeit");
        assert_eq!(fmt_utc(330), "UTC+05:30");
        assert_eq!(fmt_utc(-300), "UTC-05:00");
    }

    #[test]
    fn zone_names_cities_and_search() {
        assert_eq!(city_of("America/Argentina/Buenos_Aires"), "Buenos Aires");
        assert_eq!(parse_zone("Europe/Berlin"), Some(Berlin));
        assert_eq!(parse_zone("new york"), None, "nur volle Namen");
        assert_eq!(parse_zone("america/new york"), Some(chrono_tz::America::New_York));
        assert!(parse_zone("Mars/Olympus").is_none());
        assert!(search_zones("tokyo", 10).iter().any(|t| t.name() == "Asia/Tokyo"));
        assert_eq!(search_zones("", 5).len(), 5);
        assert_eq!(WorldClock::new("", "Asia/Tokyo").title(), "Tokyo");
        assert_eq!(WorldClock::new("Büro", "Asia/Tokyo").title(), "Büro");
    }
}
