//! Dauer-Eingabe: Text wie `1h 30m`, `1d 2h 15m`, `1.5h`, `0,5h` oder nur `2` in Minuten umrechnen – und wieder ausgeben.
//!
//! # Eingabe
//!
//! ```text
//! dauer   = ws teil { ws teil } ws          (* mehrere Teile hintereinander, auch ohne Leerzeichen: 1h30m *)
//!         | ws zahl ws                      (* eine bloße Zahl bedeutet STUNDEN: 2 = 2 h, 1.5 = 1,5 h *)
//! teil    = zahl ws einheit
//! zahl    = ziffern [ ( "." | "," ) ziffern ] | ( "." | "," ) ziffern     (* Dezimalpunkt oder -komma *)
//! einheit = m | min | mins | minute | minutes
//!         | h | hr | hrs | hour | hours | std
//!         | d | day | days | t | tag | tage                                (* Groß-/Kleinschreibung egal *)
//! ```
//!
//! Ein Tag (`d`) sind [`DEFAULT_WORK_HOURS_PER_DAY`] = 8 Arbeitsstunden, konfigurierbar über [`parse_duration_with`].
//! Jede Einheit darf nur einmal vorkommen (`1h 2h` ist ein Fehler). Negative Werte gibt es nicht.
//! Dezimalzahlen werden erst addiert und dann **einmal auf ganze Minuten gerundet** (kaufmännisch, .5 nach oben):
//! `0.25m 0.25m` ergibt 1 Minute, `0.1h` ergibt 6 Minuten.
//! Fehler tragen die Position (0-basierter Zeichenindex im Eingabetext) und eine Meldung in beiden Sprachen.
//!
//! # Ausgabe
//!
//! [`format_duration`] schreibt kurz und in beiden Sprachen gleich `d h m` (z. B. `1h 30m`, `2d 3h`, `45m`, `0m`):
//! Einheiten ohne Wert entfallen, die Tageslänge ist wieder die Arbeitstag-Länge. So lässt sich jede Ausgabe wieder einlesen
//! (`parse_duration_with(&format_duration(x, h, lang), h) == x`). [`format_hours`] gibt dezimale Stunden aus (`1.5 h` / `1,5 h`).

use crate::state::Lang;

/// Dauer in ganzen Minuten.
pub type Minutes = u32;

/// Länge eines Arbeitstags in Stunden, wenn nichts anderes eingestellt ist.
pub const DEFAULT_WORK_HOURS_PER_DAY: f64 = 8.0;

/// Obergrenze für eine Dauer (damit nichts überläuft).
pub const MAX_MINUTES: Minutes = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    Minute,
    Hour,
    Day,
}

impl DurationUnit {
    /// Kurzzeichen der Ausgabe.
    pub fn short(self) -> &'static str {
        match self {
            DurationUnit::Minute => "m",
            DurationUnit::Hour => "h",
            DurationUnit::Day => "d",
        }
    }

    fn index(self) -> usize {
        match self {
            DurationUnit::Minute => 0,
            DurationUnit::Hour => 1,
            DurationUnit::Day => 2,
        }
    }

    /// Einheit aus einem (kleingeschriebenen) Wort.
    fn from_word(w: &str) -> Option<DurationUnit> {
        match w {
            "m" | "min" | "mins" | "minute" | "minutes" => Some(DurationUnit::Minute),
            "h" | "hr" | "hrs" | "hour" | "hours" | "std" => Some(DurationUnit::Hour),
            "d" | "day" | "days" | "t" | "tag" | "tage" => Some(DurationUnit::Day),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurationErrorKind {
    /// Leere Eingabe (oder nur Leerraum).
    Empty,
    /// Hier hätte eine Zahl stehen müssen, gefunden wurde dieses Zeichen.
    ExpectedNumber(char),
    /// Zahl ohne Einheit (nur eine einzelne Zahl allein gilt als Stunden).
    MissingUnit,
    /// Unbekannte Einheit.
    UnknownUnit(String),
    /// Dieselbe Einheit zweimal.
    DuplicateUnit(DurationUnit),
    /// Kaputte Dezimalzahl (`1.2.3`, `1.`).
    BadNumber,
    /// Mehr als [`MAX_MINUTES`].
    TooLarge,
}

/// Fehler beim Lesen einer Dauer. `pos` ist der 0-basierte **Zeichen**index im Eingabetext (nicht Byte-Offset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationError {
    pub kind: DurationErrorKind,
    pub pos: usize,
}

impl DurationError {
    /// Meldung für Nutzer, mit 1-basierter Position („Position 3: …“).
    pub fn message(&self, lang: Lang) -> String {
        let p = self.pos + 1;
        let units = "m, h, d";
        match &self.kind {
            DurationErrorKind::Empty => lang
                .t("Bitte eine Dauer eingeben, z. B. „1h 30m“.", "Please enter a duration, e.g. “1h 30m”.")
                .to_string(),
            DurationErrorKind::ExpectedNumber(c) => {
                let c = lang.q(&c.to_string());
                match lang {
                    Lang::De => format!("Position {p}: Hier wurde eine Zahl erwartet, gefunden: {c}."),
                    Lang::En => format!("Position {p}: A number was expected here, found: {c}."),
                }
            }
            DurationErrorKind::MissingUnit => match lang {
                Lang::De => format!("Position {p}: Zahl ohne Einheit ({units}). Nur eine einzelne Zahl allein gilt als Stunden."),
                Lang::En => format!("Position {p}: Number without a unit ({units}). Only a single number on its own counts as hours."),
            },
            DurationErrorKind::UnknownUnit(w) => {
                let w = lang.q(w);
                match lang {
                    Lang::De => format!("Position {p}: Unbekannte Einheit {w}. Erlaubt: {units}."),
                    Lang::En => format!("Position {p}: Unknown unit {w}. Allowed: {units}."),
                }
            }
            DurationErrorKind::DuplicateUnit(u) => {
                let u = lang.q(u.short());
                match lang {
                    Lang::De => format!("Position {p}: Die Einheit {u} kommt mehrfach vor."),
                    Lang::En => format!("Position {p}: The unit {u} appears more than once."),
                }
            }
            DurationErrorKind::BadNumber => match lang {
                Lang::De => format!("Position {p}: Ungültige Zahl (erlaubt sind z. B. 2, 1.5 oder 1,5)."),
                Lang::En => format!("Position {p}: Invalid number (allowed are e.g. 2, 1.5 or 1,5)."),
            },
            DurationErrorKind::TooLarge => match lang {
                Lang::De => format!("Position {p}: Die Dauer ist zu groß."),
                Lang::En => format!("Position {p}: The duration is too large."),
            },
        }
    }
}

impl std::fmt::Display for DurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message(Lang::En))
    }
}

impl std::error::Error for DurationError {}

/// Minuten pro Arbeitstag. Ungültige Werte (≤ 0, NaN, unendlich) ergeben den Standard (8 h), mehr als 24 h werden auf 24 h gekappt;
/// mindestens 1 Minute.
pub fn minutes_per_day(work_hours_per_day: f64) -> u32 {
    let h = if work_hours_per_day.is_finite() && work_hours_per_day > 0.0 {
        work_hours_per_day.min(24.0)
    } else {
        DEFAULT_WORK_HOURS_PER_DAY
    };
    ((h * 60.0).round() as u32).max(1)
}

/// Dauer lesen, Tag = 8 Arbeitsstunden. Siehe Moduldoku für die Grammatik.
pub fn parse_duration(input: &str) -> Result<Minutes, DurationError> {
    parse_duration_with(input, DEFAULT_WORK_HOURS_PER_DAY)
}

/// Dauer lesen mit eigener Arbeitstag-Länge (in Stunden, z. B. 7.5).
pub fn parse_duration_with(input: &str, work_hours_per_day: f64) -> Result<Minutes, DurationError> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let per_day = minutes_per_day(work_hours_per_day) as f64;
    let err = |kind, pos| DurationError { kind, pos };

    let mut i = skip_ws(&chars, 0);
    if i >= len {
        return Err(err(DurationErrorKind::Empty, 0));
    }
    let mut total = 0.0f64;
    let mut parts = 0usize;
    let mut seen = [false; 3];
    loop {
        let num_start = i;
        let (value, after) = read_number(&chars, i)?;
        i = skip_ws(&chars, after);
        if i >= len || !chars[i].is_alphabetic() {
            // Keine Einheit: nur eine einzelne Zahl allein bedeutet Stunden.
            if parts == 0 && i >= len {
                total = value * 60.0;
                if total.round() > MAX_MINUTES as f64 {
                    return Err(err(DurationErrorKind::TooLarge, num_start));
                }
                break;
            }
            return Err(err(DurationErrorKind::MissingUnit, num_start));
        }
        let unit_start = i;
        while i < len && chars[i].is_alphabetic() {
            i += 1;
        }
        let word: String = chars[unit_start..i].iter().collect();
        let unit = DurationUnit::from_word(&word.to_lowercase()).ok_or_else(|| err(DurationErrorKind::UnknownUnit(word.clone()), unit_start))?;
        if seen[unit.index()] {
            return Err(err(DurationErrorKind::DuplicateUnit(unit), unit_start));
        }
        seen[unit.index()] = true;
        let factor = match unit {
            DurationUnit::Minute => 1.0,
            DurationUnit::Hour => 60.0,
            DurationUnit::Day => per_day,
        };
        total += value * factor;
        parts += 1;
        if total.round() > MAX_MINUTES as f64 {
            return Err(err(DurationErrorKind::TooLarge, num_start));
        }
        i = skip_ws(&chars, i);
        if i >= len {
            break;
        }
    }
    Ok(total.round() as Minutes)
}

fn skip_ws(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

fn is_sep(c: char) -> bool {
    c == '.' || c == ','
}

/// Zahl ab `start` lesen; liefert Wert und Index hinter der Zahl. `start` liegt immer innerhalb der Eingabe.
fn read_number(chars: &[char], start: usize) -> Result<(f64, usize), DurationError> {
    let len = chars.len();
    let mut i = start;
    let mut text = String::new();
    let mut int_digits = 0;
    while i < len && chars[i].is_ascii_digit() {
        text.push(chars[i]);
        i += 1;
        int_digits += 1;
    }
    if i < len && is_sep(chars[i]) {
        let sep = i;
        i += 1;
        text.push('.');
        let mut frac_digits = 0;
        while i < len && chars[i].is_ascii_digit() {
            text.push(chars[i]);
            i += 1;
            frac_digits += 1;
        }
        if frac_digits == 0 {
            // „.“ allein ist keine Zahl, „1.“ ist kaputt.
            let kind = if int_digits == 0 { DurationErrorKind::ExpectedNumber(chars[sep]) } else { DurationErrorKind::BadNumber };
            return Err(DurationError { kind, pos: sep });
        }
        if i < len && is_sep(chars[i]) {
            return Err(DurationError { kind: DurationErrorKind::BadNumber, pos: i });
        }
    } else if int_digits == 0 {
        return Err(DurationError { kind: DurationErrorKind::ExpectedNumber(chars[start]), pos: start });
    }
    let value: f64 = text.parse().map_err(|_| DurationError { kind: DurationErrorKind::BadNumber, pos: start })?;
    Ok((value, i))
}

/// Dauer kurz ausgeben: `d h m`, nur Einheiten mit Wert, `0m` bei null. Ein Tag sind `work_hours_per_day` Stunden.
/// Die Schreibweise ist in beiden Sprachen gleich (`lang` ist für spätere Abweichungen vorgesehen).
pub fn format_duration(minutes: Minutes, work_hours_per_day: f64, _lang: Lang) -> String {
    let per_day = minutes_per_day(work_hours_per_day);
    let days = minutes / per_day;
    let rest = minutes % per_day;
    let hours = rest / 60;
    let mins = rest % 60;
    let mut parts: Vec<String> = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 {
        parts.push(format!("{mins}m"));
    }
    if parts.is_empty() {
        "0m".to_string()
    } else {
        parts.join(" ")
    }
}

/// Dezimale Stunden mit höchstens zwei Nachkommastellen: `1.5 h` (Englisch) bzw. `1,5 h` (Deutsch).
pub fn format_hours(minutes: Minutes, lang: Lang) -> String {
    let hours = minutes as f64 / 60.0;
    let mut s = format!("{hours:.2}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if lang == Lang::De {
        s = s.replace('.', ",");
    }
    format!("{s} h")
}

/// Rundungsart für [`round_to_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// Zum nächsten Vielfachen, genau die Mitte nach oben.
    Nearest,
    Up,
    Down,
}

/// Auf ein Vielfaches von `step` Minuten runden (z. B. 15). `step == 0` lässt den Wert unverändert.
/// Aufrunden über den Zahlenbereich hinaus ergibt [`MAX_MINUTES`].
pub fn round_to_step(minutes: Minutes, step: Minutes, mode: Rounding) -> Minutes {
    if step == 0 {
        return minutes;
    }
    let m = minutes as u64;
    let s = step as u64;
    let rounded = match mode {
        Rounding::Down => m / s * s,
        Rounding::Up => m.div_ceil(s) * s,
        Rounding::Nearest => (m + s / 2) / s * s,
    };
    rounded.min(MAX_MINUTES as u64) as Minutes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Minutes {
        parse_duration(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    fn e(s: &str) -> DurationError {
        match parse_duration(s) {
            Ok(v) => panic!("{s:?} should fail, got {v}"),
            Err(e) => e,
        }
    }

    #[test]
    fn simple_units() {
        assert_eq!(p("30m"), 30);
        assert_eq!(p("2h"), 120);
        assert_eq!(p("1d"), 480);
        assert_eq!(p("0m"), 0);
    }

    #[test]
    fn combined_parts_with_and_without_spaces() {
        assert_eq!(p("1h 30m"), 90);
        assert_eq!(p("1h30m"), 90);
        assert_eq!(p("1d 2h 15m"), 480 + 120 + 15);
        assert_eq!(p("1d2h15m"), 480 + 120 + 15);
        assert_eq!(p("  1h   30m  "), 90);
        assert_eq!(p("1d\t2h\n"), 600);
    }

    #[test]
    fn any_order_is_fine() {
        assert_eq!(p("30m 1h"), 90);
        assert_eq!(p("15m 1d"), 495);
    }

    #[test]
    fn decimals_with_point_and_comma() {
        assert_eq!(p("1.5h"), 90);
        assert_eq!(p("0,5h"), 30);
        assert_eq!(p(".5h"), 30);
        assert_eq!(p(",25h"), 15);
        assert_eq!(p("0.5d"), 240);
        assert_eq!(p("1,5d 1h"), 720 + 60);
    }

    #[test]
    fn decimals_are_rounded_once_at_the_end() {
        assert_eq!(p("0.1h"), 6);
        assert_eq!(p("0.5m"), 1); // .5 nach oben
        assert_eq!(p("0.4m"), 0);
        assert_eq!(p("0.4m 0.005h"), 1); // 0,4 + 0,3 = 0,7 -> 1 (je Teil gerundet wäre es 0)
        assert_eq!(p("1.3333h"), 80);
    }

    #[test]
    fn bare_number_means_hours() {
        assert_eq!(p("2"), 120);
        assert_eq!(p("1.5"), 90);
        assert_eq!(p("0,5"), 30);
        assert_eq!(p("  8  "), 480);
        assert_eq!(p("0"), 0);
    }

    #[test]
    fn case_insensitive_and_aliases() {
        assert_eq!(p("1H 30M"), 90);
        assert_eq!(p("1D"), 480);
        assert_eq!(p("2 Std"), 120);
        assert_eq!(p("2 hours 5 minutes"), 125);
        assert_eq!(p("1 Tag"), 480);
        assert_eq!(p("2 Tage"), 960);
        assert_eq!(p("1t 1h"), 540);
        assert_eq!(p("45 min"), 45);
        assert_eq!(p("1 day"), 480);
    }

    #[test]
    fn space_between_number_and_unit() {
        assert_eq!(p("1 h 30 m"), 90);
        assert_eq!(p("1.5 h"), 90);
    }

    #[test]
    fn configurable_day_length() {
        assert_eq!(parse_duration_with("1d", 7.5).unwrap(), 450);
        assert_eq!(parse_duration_with("2d 1h", 6.0).unwrap(), 780);
        assert_eq!(parse_duration_with("1d", 24.0).unwrap(), 1440);
        // Ungültige Tageslängen fallen auf 8 h zurück, mehr als 24 h werden gekappt.
        assert_eq!(parse_duration_with("1d", 0.0).unwrap(), 480);
        assert_eq!(parse_duration_with("1d", -3.0).unwrap(), 480);
        assert_eq!(parse_duration_with("1d", f64::NAN).unwrap(), 480);
        assert_eq!(parse_duration_with("1d", 99.0).unwrap(), 1440);
    }

    #[test]
    fn empty_input_is_an_error() {
        assert_eq!(e("").kind, DurationErrorKind::Empty);
        assert_eq!(e("   ").kind, DurationErrorKind::Empty);
        assert_eq!(e("\t\n").pos, 0);
    }

    #[test]
    fn missing_unit_reports_position_of_the_number() {
        let err = e("1h 30");
        assert_eq!(err.kind, DurationErrorKind::MissingUnit);
        assert_eq!(err.pos, 3);
        let err = e("1h30");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::MissingUnit, 2));
        let err = e("2 30m");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::MissingUnit, 0));
    }

    #[test]
    fn unknown_unit_reports_position_and_word() {
        let err = e("5x");
        assert_eq!(err.kind, DurationErrorKind::UnknownUnit("x".into()));
        assert_eq!(err.pos, 1);
        let err = e("1h 5wochen");
        assert_eq!(err.kind, DurationErrorKind::UnknownUnit("wochen".into()));
        assert_eq!(err.pos, 4);
        let err = e("1hm");
        assert_eq!(err.kind, DurationErrorKind::UnknownUnit("hm".into()));
    }

    #[test]
    fn duplicate_unit_is_rejected() {
        let err = e("1h 2h");
        assert_eq!(err.kind, DurationErrorKind::DuplicateUnit(DurationUnit::Hour));
        assert_eq!(err.pos, 4);
        assert_eq!(e("1h 30m 15m").kind, DurationErrorKind::DuplicateUnit(DurationUnit::Minute));
        // „h“ und „std“ sind dieselbe Einheit.
        assert_eq!(e("1h 1std").kind, DurationErrorKind::DuplicateUnit(DurationUnit::Hour));
    }

    #[test]
    fn letters_or_signs_where_a_number_belongs() {
        assert_eq!(e("h").kind, DurationErrorKind::ExpectedNumber('h'));
        assert_eq!(e("abc").pos, 0);
        let err = e("-1h");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::ExpectedNumber('-'), 0));
        assert_eq!(e("+2h").kind, DurationErrorKind::ExpectedNumber('+'));
        let err = e("1h, 30m");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::ExpectedNumber(','), 2));
        let err = e("1h x");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::ExpectedNumber('x'), 3));
        assert_eq!(e(".").kind, DurationErrorKind::ExpectedNumber('.'));
    }

    #[test]
    fn broken_decimals() {
        let err = e("1.2.3h");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::BadNumber, 3));
        let err = e("1.h");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::BadNumber, 1));
        let err = e("1,2,3");
        assert_eq!(err.kind, DurationErrorKind::BadNumber);
        assert_eq!(e("1..5h").kind, DurationErrorKind::BadNumber);
    }

    #[test]
    fn exponent_notation_is_not_supported() {
        assert_eq!(e("1e3h").kind, DurationErrorKind::UnknownUnit("e".into()));
    }

    #[test]
    fn too_large_values_are_rejected() {
        assert_eq!(e("99999999999d").kind, DurationErrorKind::TooLarge);
        assert_eq!(e(&format!("{}h", "9".repeat(400))).kind, DurationErrorKind::TooLarge);
        assert_eq!(e("99999999999").kind, DurationErrorKind::TooLarge);
        assert_eq!(e("80000000h").kind, DurationErrorKind::TooLarge);
        // Genau die Obergrenze geht noch.
        assert_eq!(p("4294967295m"), u32::MAX);
        assert_eq!(e("4294967296m").kind, DurationErrorKind::TooLarge);
    }

    #[test]
    fn position_counts_characters_not_bytes() {
        // „ä“ ist 2 Byte, aber 1 Zeichen: die unbekannte Einheit „ä“ liegt an Zeichenposition 1.
        let err = e("1ä");
        assert_eq!(err.pos, 1);
        assert_eq!(err.kind, DurationErrorKind::UnknownUnit("ä".into()));
        let err = e("😀");
        assert_eq!((err.kind.clone(), err.pos), (DurationErrorKind::ExpectedNumber('😀'), 0));
    }

    #[test]
    fn non_ascii_digits_are_rejected() {
        assert_eq!(e("٣h").kind, DurationErrorKind::ExpectedNumber('٣'));
    }

    #[test]
    fn messages_are_bilingual_and_show_one_based_position() {
        let err = e("1h 5x");
        let en = err.message(Lang::En);
        let de = err.message(Lang::De);
        assert!(en.starts_with("Position 5:"), "{en}");
        assert!(en.contains("Unknown unit") && en.contains("“x”"), "{en}");
        assert!(de.starts_with("Position 5:"), "{de}");
        assert!(de.contains("Unbekannte Einheit") && de.contains("„x“"), "{de}");
        assert_ne!(en, de);
    }

    #[test]
    fn every_error_kind_has_two_different_messages() {
        let kinds = [
            DurationErrorKind::Empty,
            DurationErrorKind::ExpectedNumber('x'),
            DurationErrorKind::MissingUnit,
            DurationErrorKind::UnknownUnit("zz".into()),
            DurationErrorKind::DuplicateUnit(DurationUnit::Day),
            DurationErrorKind::BadNumber,
            DurationErrorKind::TooLarge,
        ];
        for kind in kinds {
            let err = DurationError { kind, pos: 2 };
            let (en, de) = (err.message(Lang::En), err.message(Lang::De));
            assert!(!en.is_empty() && !de.is_empty());
            assert_ne!(en, de, "{err:?}");
        }
    }

    #[test]
    fn display_is_english() {
        assert_eq!(e("").to_string(), e("").message(Lang::En));
        let boxed: Box<dyn std::error::Error> = Box::new(e("x"));
        assert!(boxed.to_string().contains("number was expected"));
    }

    #[test]
    fn format_short_form() {
        let f = |m| format_duration(m, 8.0, Lang::En);
        assert_eq!(f(0), "0m");
        assert_eq!(f(45), "45m");
        assert_eq!(f(60), "1h");
        assert_eq!(f(90), "1h 30m");
        assert_eq!(f(480), "1d");
        assert_eq!(f(480 + 180), "1d 3h");
        assert_eq!(f(2 * 480 + 3 * 60), "2d 3h");
        assert_eq!(f(480 + 60 + 15), "1d 1h 15m");
        assert_eq!(f(479), "7h 59m");
    }

    #[test]
    fn format_is_the_same_in_both_languages() {
        for m in [0, 1, 59, 60, 61, 479, 480, 481, 5000] {
            assert_eq!(format_duration(m, 8.0, Lang::De), format_duration(m, 8.0, Lang::En));
        }
    }

    #[test]
    fn format_with_other_day_length() {
        assert_eq!(format_duration(450, 7.5, Lang::En), "1d");
        assert_eq!(format_duration(450 + 30, 7.5, Lang::En), "1d 30m");
        assert_eq!(format_duration(1440, 24.0, Lang::En), "1d");
        assert_eq!(format_duration(500, 8.0, Lang::En), "1d 20m");
    }

    #[test]
    fn format_then_parse_round_trips() {
        for hours in [8.0, 7.5, 6.0, 24.0, 1.0] {
            for m in [0u32, 1, 15, 59, 60, 61, 90, 359, 360, 449, 450, 479, 480, 481, 1000, 2880, 100_000, 4_000_000_000] {
                let text = format_duration(m, hours, Lang::En);
                assert_eq!(parse_duration_with(&text, hours).unwrap(), m, "{text:?} @ {hours} h");
            }
        }
    }

    #[test]
    fn format_hours_decimal() {
        assert_eq!(format_hours(90, Lang::En), "1.5 h");
        assert_eq!(format_hours(90, Lang::De), "1,5 h");
        assert_eq!(format_hours(120, Lang::En), "2 h");
        assert_eq!(format_hours(100, Lang::En), "1.67 h");
        assert_eq!(format_hours(100, Lang::De), "1,67 h");
        assert_eq!(format_hours(0, Lang::En), "0 h");
        assert_eq!(format_hours(15, Lang::De), "0,25 h");
    }

    #[test]
    fn rounding_to_steps() {
        use Rounding::*;
        assert_eq!(round_to_step(7, 15, Nearest), 0);
        assert_eq!(round_to_step(8, 15, Nearest), 15);
        assert_eq!(round_to_step(22, 15, Nearest), 15);
        assert_eq!(round_to_step(23, 15, Nearest), 30);
        assert_eq!(round_to_step(30, 15, Nearest), 30);
        assert_eq!(round_to_step(31, 15, Up), 45);
        assert_eq!(round_to_step(30, 15, Up), 30);
        assert_eq!(round_to_step(0, 15, Up), 0);
        assert_eq!(round_to_step(44, 15, Down), 30);
        assert_eq!(round_to_step(14, 15, Down), 0);
        assert_eq!(round_to_step(100, 0, Up), 100);
        assert_eq!(round_to_step(100, 1, Nearest), 100);
    }

    #[test]
    fn rounding_up_saturates() {
        assert_eq!(round_to_step(u32::MAX - 1, 60, Rounding::Up), u32::MAX);
        assert_eq!(round_to_step(u32::MAX, 15, Rounding::Nearest), u32::MAX);
    }

    #[test]
    fn rounding_sum_of_parts() {
        // Erst summieren, dann runden: 3 × 20 Minuten = 60, nicht 3 × (auf 15 gerundet).
        let parts = [20u32, 20, 20];
        let total: Minutes = parts.iter().sum();
        assert_eq!(round_to_step(total, 15, Rounding::Nearest), 60);
        let per_part: Minutes = parts.iter().map(|m| round_to_step(*m, 15, Rounding::Nearest)).sum();
        assert_eq!(per_part, 45);
    }
}
