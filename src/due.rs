//! Fälligkeit („Due date“) von Tickets und Todo-Karten: Eingabe lesen, gespeicherte Form, Anzeige und die Markierung nach
//! Schwellen samt Tooltip-Text. Reine Logik ohne Oberfläche und ohne Datenbank.
//!
//! * **Gespeicherte Form:** `JJJJ-MM-TT` (nur Datum) oder `JJJJ-MM-TTTHH:MM[:SS]` (mit Uhrzeit), lokale Zeit.
//! * **Nur Datum** zählt als **Ende dieses Tages** (lokal): fällig bis 23:59:59, überfällig ab 00:00 des Folgetags.
//! * **Schwellen:** eine Liste aus Dauer, Markierung und „Zeile/Karte tönen“. Es gilt die **kleinste** Schwelle, deren Dauer
//!   größer ist als die verbleibende Zeit (überfällig = negative Zeit, also unter jeder Schwelle). Liegt die verbleibende Zeit
//!   über allen Schwellen, gibt es keine Markierung.
//! * **Markierung** = Rahmen (keiner, Dreieck, Quadrat, Kreis, umgedrehtes Dreieck, Sechseck, Achteck) + Symbol (keins oder
//!   ein Icon aus `planning::icons::ICONS`) + Farbe (+ optionaler Name). Standard (Projekte mit den mitgelieferten Typen und
//!   die einfache Todo-Liste): gelbes Dreieck mit Ausrufezeichen unter 1 Woche, rotes unter 1 Tag (auch überfällig), ohne
//!   Tönung.

use crate::state::Lang;
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};

/// Höchstens so viele Schwellen je Liste.
pub const MAX_THRESHOLDS: usize = 20;
/// Längste Schwelle: 10 Jahre in Minuten.
pub const MAX_MINUTES: i64 = 10 * 366 * 24 * 60;
/// Längster Name einer Markierung.
pub const MAX_NAME_CHARS: usize = 40;

pub const MINUTES_PER_HOUR: i64 = 60;
pub const MINUTES_PER_DAY: i64 = 24 * 60;
pub const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;

/// Farbe der Standard-Markierung „bald“ (gelb) und „sehr bald/überfällig“ (rot), aus den Standard-Paletten.
pub const YELLOW: &str = "#D8B94A";
pub const RED: &str = "#E06C75";
/// Symbol der Standard-Markierungen (Ausrufezeichen).
pub const ALERT_SYMBOL: &str = "alert";

/// Rahmen einer Markierung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MarkerFrame {
    #[default]
    None,
    Triangle,
    Square,
    Circle,
    InvertedTriangle,
    Hexagon,
    Octagon,
}

impl MarkerFrame {
    pub const ALL: [MarkerFrame; 7] = [MarkerFrame::None, MarkerFrame::Triangle, MarkerFrame::Square, MarkerFrame::Circle, MarkerFrame::InvertedTriangle, MarkerFrame::Hexagon, MarkerFrame::Octagon];

    pub fn as_str(self) -> &'static str {
        match self {
            MarkerFrame::None => "none",
            MarkerFrame::Triangle => "triangle",
            MarkerFrame::Square => "square",
            MarkerFrame::Circle => "circle",
            MarkerFrame::InvertedTriangle => "inverted_triangle",
            MarkerFrame::Hexagon => "hexagon",
            MarkerFrame::Octagon => "octagon",
        }
    }

    pub fn parse(s: &str) -> Option<MarkerFrame> {
        MarkerFrame::ALL.into_iter().find(|m| m.as_str().eq_ignore_ascii_case(s.trim()))
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            MarkerFrame::None => lang.t("Kein Rahmen", "No frame"),
            MarkerFrame::Triangle => lang.t("Dreieck", "Triangle"),
            MarkerFrame::Square => lang.t("Quadrat", "Square"),
            MarkerFrame::Circle => lang.t("Kreis", "Circle"),
            MarkerFrame::InvertedTriangle => lang.t("Umgedrehtes Dreieck", "Inverted triangle"),
            MarkerFrame::Hexagon => lang.t("Sechseck", "Hexagon"),
            MarkerFrame::Octagon => lang.t("Achteck", "Octagon"),
        }
    }
}

fn default_color() -> String {
    YELLOW.to_string()
}

/// Markierung an einer fälligen Karte: Rahmen + Symbol + Farbe (+ Name). Ohne Rahmen und ohne Symbol wird nichts gezeichnet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DueMarker {
    #[serde(default)]
    pub frame: MarkerFrame,
    /// Icon-Name aus `planning::icons::ICONS`; `None` = kein Symbol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// „#RRGGBB“
    #[serde(default = "default_color")]
    pub color: String,
    /// Optionaler Name (z. B. „Bald fällig“), sonst leer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl Default for DueMarker {
    fn default() -> Self {
        DueMarker { frame: MarkerFrame::None, symbol: None, color: default_color(), name: String::new() }
    }
}

impl DueMarker {
    /// Dreieck mit Ausrufezeichen in der Farbe.
    pub fn warning(color: &str) -> Self {
        DueMarker { frame: MarkerFrame::Triangle, symbol: Some(ALERT_SYMBOL.to_string()), color: color.to_string(), name: String::new() }
    }

    /// Wird nichts gezeichnet (kein Rahmen, kein Symbol)?
    pub fn is_none(&self) -> bool {
        self.frame == MarkerFrame::None && self.symbol.is_none()
    }
}

/// Eine Schwelle: verbleibt weniger als `under_minutes` bis zur Fälligkeit, gilt `marker`; `tint` tönt Zeile/Karte zusätzlich.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DueThreshold {
    pub under_minutes: i64,
    #[serde(default)]
    pub marker: DueMarker,
    #[serde(default)]
    pub tint: bool,
}

/// Die Standard-Schwellen: unter 1 Woche gelbes Warndreieck, unter 1 Tag (auch überfällig) rotes, ohne Tönung.
pub fn default_thresholds() -> Vec<DueThreshold> {
    vec![
        DueThreshold { under_minutes: MINUTES_PER_WEEK, marker: DueMarker::warning(YELLOW), tint: false },
        DueThreshold { under_minutes: MINUTES_PER_DAY, marker: DueMarker::warning(RED), tint: false },
    ]
}

/// JSON der Standard-Schwellen (für die SQL-Migration; ein Test prüft, dass es [`default_thresholds`] entspricht).
pub const DEFAULT_THRESHOLDS_JSON: &str = r##"[{"under_minutes":10080,"marker":{"frame":"triangle","symbol":"alert","color":"#D8B94A"},"tint":false},{"under_minutes":1440,"marker":{"frame":"triangle","symbol":"alert","color":"#E06C75"},"tint":false}]"##;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdProblem {
    /// Mehr als [`MAX_THRESHOLDS`] Einträge.
    TooMany,
    /// Dauer nicht positiv oder größer als [`MAX_MINUTES`].
    Duration,
    /// Zwei Schwellen mit derselben Dauer.
    Duplicate,
    /// Farbe ist kein „#RRGGBB“.
    Color,
    /// Symbol ist kein bekanntes Icon.
    Symbol,
    /// Name zu lang.
    Name,
}

impl ThresholdProblem {
    pub const ALL: [ThresholdProblem; 6] = [ThresholdProblem::TooMany, ThresholdProblem::Duration, ThresholdProblem::Duplicate, ThresholdProblem::Color, ThresholdProblem::Symbol, ThresholdProblem::Name];

    pub fn message(self, lang: Lang) -> &'static str {
        match self {
            ThresholdProblem::TooMany => lang.t("Höchstens 20 Schwellen.", "At most 20 thresholds."),
            ThresholdProblem::Duration => lang.t("Eine Dauer muss größer als 0 und höchstens 10 Jahre sein.", "A duration must be greater than 0 and at most 10 years."),
            ThresholdProblem::Duplicate => lang.t("Zwei Schwellen haben dieselbe Dauer.", "Two thresholds have the same duration."),
            ThresholdProblem::Color => lang.t("Die Farbe einer Markierung muss die Form #RRGGBB haben.", "A marker's color must look like #RRGGBB."),
            ThresholdProblem::Symbol => lang.t("Dieses Symbol gibt es nicht.", "This symbol does not exist."),
            ThresholdProblem::Name => lang.t("Der Name einer Markierung ist zu lang (höchstens 40 Zeichen).", "A marker's name is too long (at most 40 characters)."),
        }
    }
}

fn clean_color(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    (h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("#{}", h.to_ascii_uppercase()))
}

/// Prüft eine Schwellen-Liste und bereinigt sie (Farbe, Symbol, Name). Die Reihenfolge bleibt, wie der Nutzer sie festgelegt
/// hat – für die Auswertung zählt nur die Dauer.
pub fn validate_thresholds(list: &[DueThreshold]) -> Result<Vec<DueThreshold>, ThresholdProblem> {
    if list.len() > MAX_THRESHOLDS {
        return Err(ThresholdProblem::TooMany);
    }
    let mut out: Vec<DueThreshold> = Vec::with_capacity(list.len());
    for t in list {
        if t.under_minutes <= 0 || t.under_minutes > MAX_MINUTES {
            return Err(ThresholdProblem::Duration);
        }
        if out.iter().any(|o| o.under_minutes == t.under_minutes) {
            return Err(ThresholdProblem::Duplicate);
        }
        let color = clean_color(&t.marker.color).ok_or(ThresholdProblem::Color)?;
        let symbol = match t.marker.symbol.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) if crate::planning::icons::is_valid(s) => Some(s.to_string()),
            Some(_) => return Err(ThresholdProblem::Symbol),
        };
        let name = t.marker.name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(ThresholdProblem::Name);
        }
        out.push(DueThreshold { under_minutes: t.under_minutes, marker: DueMarker { frame: t.marker.frame, symbol, color, name }, tint: t.tint });
    }
    Ok(out)
}

/// Liest gespeichertes JSON; Unlesbares oder Ungültiges ergibt eine leere Liste (keine Markierung).
pub fn parse_thresholds(json: &str) -> Vec<DueThreshold> {
    serde_json::from_str::<Vec<DueThreshold>>(json).ok().and_then(|v| validate_thresholds(&v).ok()).unwrap_or_default()
}

pub fn thresholds_json(list: &[DueThreshold]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".to_string())
}

// ------------------------------------------------------------------ Datum lesen und schreiben

fn year_ok(d: NaiveDate) -> bool {
    (1000..=9999).contains(&d.year())
}

fn read_date(s: &str) -> Option<NaiveDate> {
    ["%Y-%m-%d", "%d.%m.%Y", "%Y/%m/%d"].iter().find_map(|f| NaiveDate::parse_from_str(s.trim(), f).ok()).filter(|d| year_ok(*d))
}

fn read_time(s: &str) -> Option<NaiveTime> {
    let t = s.trim();
    let t = t.strip_suffix("Uhr").unwrap_or(t).trim();
    ["%H:%M:%S", "%H:%M"].iter().find_map(|f| NaiveTime::parse_from_str(t, f).ok())
}

/// Liest eine Eingabe (`2026-11-20`, `20.11.2026`, jeweils optional mit Uhrzeit `14:30`, getrennt durch Leerzeichen oder `T`)
/// und liefert die gespeicherte Form (`2026-11-20` bzw. `2026-11-20T14:30`). Leer → `Ok(None)`, unlesbar → `Err(())`.
#[allow(clippy::result_unit_err)]
pub fn parse_input(s: &str) -> Result<Option<String>, ()> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let mut prev_digit = false;
    let mut split = None;
    for (i, c) in t.char_indices() {
        if c == ' ' || (c == 'T' && prev_digit) {
            split = Some(i);
            break;
        }
        prev_digit = c.is_ascii_digit();
    }
    match split {
        None => read_date(t).map(|d| Some(d.format("%Y-%m-%d").to_string())).ok_or(()),
        Some(i) => {
            let date = read_date(&t[..i]).ok_or(())?;
            let rest = t[i + 1..].trim();
            if rest.is_empty() {
                return Ok(Some(date.format("%Y-%m-%d").to_string()));
            }
            let time = read_time(rest).ok_or(())?;
            Ok(Some(NaiveDateTime::new(date, time).format("%Y-%m-%dT%H:%M").to_string()))
        }
    }
}

/// Ist der Text eine gültige gespeicherte Fälligkeit (`JJJJ-MM-TT` oder `JJJJ-MM-TTTHH:MM[:SS]`)?
pub fn is_valid(stored: &str) -> bool {
    parse_stored(stored).is_some()
}

/// Gespeicherte Form → (Datum, Uhrzeit falls angegeben).
pub fn parse_stored(stored: &str) -> Option<(NaiveDate, Option<NaiveTime>)> {
    let s = stored.trim();
    if s.len() == 10 {
        return NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().filter(|d| year_ok(*d)).map(|d| (d, None));
    }
    let dt = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")).ok()?;
    year_ok(dt.date()).then_some((dt.date(), Some(dt.time())))
}

/// Zeitpunkt, ab dem die Karte überfällig ist: mit Uhrzeit genau dieser, nur Datum = Beginn des Folgetags (Ende des Tages).
pub fn deadline(stored: &str) -> Option<NaiveDateTime> {
    let (d, t) = parse_stored(stored)?;
    match t {
        Some(t) => Some(NaiveDateTime::new(d, t)),
        None => d.succ_opt().map(|n| n.and_time(NaiveTime::MIN)),
    }
}

/// Text für die Anzeige: `2026-11-20` / `20.11.2026`, mit Uhrzeit `… 14:30`.
pub fn display(stored: &str, lang: Lang) -> String {
    let Some((d, t)) = parse_stored(stored) else { return stored.to_string() };
    let date = match lang {
        Lang::De => d.format("%d.%m.%Y").to_string(),
        Lang::En => d.format("%Y-%m-%d").to_string(),
    };
    match t {
        Some(t) => format!("{date} {}", t.format("%H:%M")),
        None => date,
    }
}

/// Text für ein Eingabefeld (immer ISO, Uhrzeit mit Leerzeichen): `2026-11-20` bzw. `2026-11-20 14:30`.
pub fn edit_text(stored: &str) -> String {
    match parse_stored(stored) {
        Some((d, Some(t))) => format!("{} {}", d.format("%Y-%m-%d"), t.format("%H:%M")),
        Some((d, None)) => d.format("%Y-%m-%d").to_string(),
        None => stored.to_string(),
    }
}

// ------------------------------------------------------------------ Auswerten

/// Ergebnis der Auswertung: die zutreffende Schwelle (falls eine) und wie viele Minuten noch bleiben (negativ = überfällig).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueState {
    pub threshold: Option<DueThreshold>,
    pub minutes_left: i64,
}

impl DueState {
    pub fn overdue(&self) -> bool {
        self.minutes_left < 0
    }

    /// Die zu zeichnende Markierung (`None`, wenn keine Schwelle zutrifft oder sie nichts zeichnet).
    pub fn marker(&self) -> Option<&DueMarker> {
        self.threshold.as_ref().map(|t| &t.marker).filter(|m| !m.is_none())
    }

    /// Farbe zum Tönen von Zeile/Karte (nur wenn die Schwelle es verlangt).
    pub fn tint(&self) -> Option<&str> {
        self.threshold.as_ref().filter(|t| t.tint).map(|t| t.marker.color.as_str())
    }

    /// Wird irgendetwas angezeigt (Markierung oder Tönung)? Dann zählt der Eintrag als „markiert“.
    pub fn flagged(&self) -> bool {
        self.marker().is_some() || self.tint().is_some()
    }
}

/// Verbleibende Minuten bis zur Fälligkeit (abgerundet zur Null hin, negativ = überfällig).
pub fn minutes_left(stored: &str, now: NaiveDateTime) -> Option<i64> {
    let dl = deadline(stored)?;
    let secs = (dl - now).num_seconds();
    // überfällig zählt ab der ersten Sekunde nach der Frist
    Some(if secs < 0 { -((-secs + 59) / 60) } else { secs / 60 })
}

/// Auswertung einer Fälligkeit nach den Schwellen. `None`, wenn der Text keine gültige Fälligkeit ist.
pub fn evaluate(thresholds: &[DueThreshold], stored: &str, now: NaiveDateTime) -> Option<DueState> {
    let secs = (deadline(stored)? - now).num_seconds();
    let left = minutes_left(stored, now)?;
    let threshold = thresholds.iter().filter(|t| secs < t.under_minutes * 60).min_by_key(|t| t.under_minutes).cloned();
    Some(DueState { threshold, minutes_left: left })
}

/// Zählung für die Übersicht: (überfällig, markiert) über die Fälligkeiten offener Einträge.
pub fn count<I: IntoIterator<Item = S>, S: AsRef<str>>(thresholds: &[DueThreshold], dues: I, now: NaiveDateTime) -> (usize, usize) {
    let (mut overdue, mut marked) = (0, 0);
    for d in dues {
        if let Some(st) = evaluate(thresholds, d.as_ref(), now) {
            overdue += usize::from(st.overdue());
            marked += usize::from(st.flagged());
        }
    }
    (overdue, marked)
}

/// Kurztext für die Übersicht: „3 überfällig“ / „3 overdue“, sonst „2 markiert“ / „2 marked“, sonst leer.
pub fn count_text(overdue: usize, marked: usize, lang: Lang) -> String {
    match (overdue, marked) {
        (0, 0) => String::new(),
        (0, m) => format!("{m} {}", lang.t("markiert", "marked")),
        (o, _) => format!("{o} {}", lang.t("überfällig", "overdue")),
    }
}

/// Dauer als kurzer Text: Tage ab einem Tag, sonst Stunden ab einer Stunde, sonst Minuten.
fn span_text(minutes: i64, lang: Lang) -> String {
    let m = minutes.abs();
    if m >= MINUTES_PER_DAY {
        let d = m / MINUTES_PER_DAY;
        match lang {
            Lang::De => format!("{d} {}", if d == 1 { "Tag" } else { "Tagen" }),
            Lang::En => format!("{d} {}", if d == 1 { "day" } else { "days" }),
        }
    } else if m >= MINUTES_PER_HOUR {
        format!("{} h", m / MINUTES_PER_HOUR)
    } else {
        format!("{m} min")
    }
}

/// Tooltip: „fällig in 3 Tagen“ / „due in 3 days“, „überfällig seit 2 h“ / „overdue by 2 h“.
pub fn tooltip(minutes_left: i64, lang: Lang) -> String {
    let span = span_text(minutes_left, lang);
    match (minutes_left < 0, lang) {
        (true, Lang::De) => format!("überfällig seit {span}"),
        (true, Lang::En) => format!("overdue by {span}"),
        (false, Lang::De) => format!("fällig in {span}"),
        (false, Lang::En) => format!("due in {span}"),
    }
}

/// Dauer einer Schwelle für die Anzeige („1 Woche“, „3 Tage“, „12 h“, „30 min“), in der größten glatten Einheit.
pub fn duration_label(minutes: i64, lang: Lang) -> String {
    let (n, unit) = split_duration(minutes);
    format!("{n} {}", unit.label(n, lang))
}

/// Einheit im Schwellen-Editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    Minutes,
    Hours,
    Days,
    Weeks,
}

impl DurationUnit {
    pub const ALL: [DurationUnit; 4] = [DurationUnit::Minutes, DurationUnit::Hours, DurationUnit::Days, DurationUnit::Weeks];

    pub fn minutes(self) -> i64 {
        match self {
            DurationUnit::Minutes => 1,
            DurationUnit::Hours => MINUTES_PER_HOUR,
            DurationUnit::Days => MINUTES_PER_DAY,
            DurationUnit::Weeks => MINUTES_PER_WEEK,
        }
    }

    pub fn label(self, n: i64, lang: Lang) -> &'static str {
        let one = n == 1;
        match (self, lang) {
            (DurationUnit::Minutes, _) => "min",
            (DurationUnit::Hours, _) => "h",
            (DurationUnit::Days, Lang::De) => {
                if one {
                    "Tag"
                } else {
                    "Tage"
                }
            }
            (DurationUnit::Days, Lang::En) => {
                if one {
                    "day"
                } else {
                    "days"
                }
            }
            (DurationUnit::Weeks, Lang::De) => {
                if one {
                    "Woche"
                } else {
                    "Wochen"
                }
            }
            (DurationUnit::Weeks, Lang::En) => {
                if one {
                    "week"
                } else {
                    "weeks"
                }
            }
        }
    }
}

/// Minuten in (Anzahl, größte glatte Einheit) zerlegen: 10080 → (1, Wochen), 2880 → (2, Tage), 90 → (90, Minuten).
pub fn split_duration(minutes: i64) -> (i64, DurationUnit) {
    for u in [DurationUnit::Weeks, DurationUnit::Days, DurationUnit::Hours] {
        if minutes > 0 && minutes % u.minutes() == 0 {
            return (minutes / u.minutes(), u);
        }
    }
    (minutes, DurationUnit::Minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").unwrap()
    }

    /// Farbe der Markierung nach den Standard-Schwellen (`None` = keine).
    fn color(stored: &str, now: &str) -> Option<&'static str> {
        let st = evaluate(&default_thresholds(), stored, at(now)).unwrap();
        st.marker().map(|m| if m.color == YELLOW { YELLOW } else if m.color == RED { RED } else { "?" })
    }

    #[test]
    fn input_is_read_in_iso_and_german_form_with_optional_time() {
        assert_eq!(parse_input(""), Ok(None));
        assert_eq!(parse_input("   "), Ok(None));
        assert_eq!(parse_input("2026-11-20"), Ok(Some("2026-11-20".into())));
        assert_eq!(parse_input(" 20.11.2026 "), Ok(Some("2026-11-20".into())));
        assert_eq!(parse_input("2026-11-20 14:30"), Ok(Some("2026-11-20T14:30".into())));
        assert_eq!(parse_input("2026-11-20T14:30"), Ok(Some("2026-11-20T14:30".into())));
        assert_eq!(parse_input("2026-11-20T14:30:15"), Ok(Some("2026-11-20T14:30".into())));
        assert_eq!(parse_input("20.11.2026 9:05 Uhr"), Ok(Some("2026-11-20T09:05".into())));
        assert_eq!(parse_input("2026-11-20 "), Ok(Some("2026-11-20".into())));
        for bad in ["morgen", "2026-13-01", "2026-11-20 25:00", "20.11.26", "0999-01-01", "2026-11-20 x", "2026-02-30"] {
            assert_eq!(parse_input(bad), Err(()), "{bad}");
        }
    }

    #[test]
    fn stored_values_and_display() {
        assert!(is_valid("2026-11-20") && is_valid("2026-11-20T14:30") && is_valid("2026-11-20T14:30:15"));
        assert!(!is_valid("2026-11-20 14:30") && !is_valid("20.11.2026") && !is_valid("") && !is_valid("2026-11-20T"));
        assert_eq!(display("2026-11-20", Lang::De), "20.11.2026");
        assert_eq!(display("2026-11-20T14:30", Lang::De), "20.11.2026 14:30");
        assert_eq!(display("2026-11-20T14:30:15", Lang::En), "2026-11-20 14:30");
        assert_eq!(edit_text("2026-11-20T14:30:00"), "2026-11-20 14:30");
        assert_eq!(edit_text("2026-11-20"), "2026-11-20");
        // Eingabe → gespeichert → Eingabetext → wieder gleich
        for s in ["2026-11-20", "2026-11-20T14:30"] {
            assert_eq!(parse_input(&edit_text(s)), Ok(Some(s.to_string())));
        }
    }

    #[test]
    fn date_only_counts_as_end_of_the_local_day() {
        assert_eq!(deadline("2026-11-20"), Some(at("2026-11-21T00:00:00")));
        assert_eq!(deadline("2026-12-31"), Some(at("2027-01-01T00:00:00")));
        assert_eq!(deadline("2026-11-20T14:30"), Some(at("2026-11-20T14:30:00")));
        // am Fälligkeitstag kurz vor Mitternacht noch nicht überfällig, ab Mitternacht schon
        assert_eq!(minutes_left("2026-11-20", at("2026-11-20T23:59:00")), Some(1));
        assert_eq!(minutes_left("2026-11-20", at("2026-11-21T00:00:00")), Some(0));
        assert_eq!(minutes_left("2026-11-20", at("2026-11-21T00:00:01")), Some(-1));
        assert_eq!(minutes_left("2026-11-20", at("2026-11-21T02:00:00")), Some(-120));
    }

    #[test]
    fn default_thresholds_at_the_boundaries() {
        let y = Some(YELLOW);
        let r = Some(RED);
        let due = "2026-11-20T12:00";
        // ≥ 1 Woche: nichts
        assert_eq!(color(due, "2026-11-01T00:00:00"), None);
        assert_eq!(color(due, "2026-11-13T12:00:00"), None, "genau 1 Woche");
        // < 1 Woche und ≥ 1 Tag: gelb
        assert_eq!(color(due, "2026-11-13T12:00:01"), y, "eine Sekunde unter 1 Woche");
        assert_eq!(color(due, "2026-11-19T12:00:00"), y, "genau 1 Tag");
        // < 1 Tag: rot, auch überfällig
        assert_eq!(color(due, "2026-11-19T12:00:01"), r);
        assert_eq!(color(due, "2026-11-20T12:00:00"), r, "genau jetzt fällig");
        assert_eq!(color(due, "2026-12-24T08:00:00"), r, "lange überfällig");
        // nur Datum: Ende des Tages zählt
        assert_eq!(color("2026-11-20", "2026-11-14T00:00:00"), None, "genau 1 Woche vor Tagesende");
        assert_eq!(color("2026-11-20", "2026-11-14T00:00:01"), y);
        assert_eq!(color("2026-11-20", "2026-11-20T00:00:00"), y, "am Morgen des Tages: noch 24 h");
        assert_eq!(color("2026-11-20", "2026-11-20T00:00:01"), r);
        assert_eq!(color("2026-11-20", "2026-11-22T10:00:00"), r);
        // Standard: Dreieck mit Ausrufezeichen, ohne Tönung
        let st = evaluate(&default_thresholds(), due, at("2026-11-20T00:00:00")).unwrap();
        let m = st.marker().unwrap();
        assert_eq!((m.frame, m.symbol.as_deref()), (MarkerFrame::Triangle, Some(ALERT_SYMBOL)));
        assert_eq!(st.tint(), None);
        assert!(st.flagged() && !st.overdue());
        // ungültig → keine Auswertung
        assert_eq!(evaluate(&default_thresholds(), "kaputt", at("2026-11-20T00:00:00")), None);
    }

    #[test]
    fn empty_or_custom_threshold_lists_and_the_tint_flag() {
        let st = evaluate(&[], "2026-11-20", at("2026-12-20T00:00:00")).unwrap();
        assert!(st.marker().is_none() && st.overdue() && !st.flagged(), "ohne Schwellen nie eine Markierung");
        let blue = DueMarker { frame: MarkerFrame::Circle, symbol: Some("clock".into()), color: "#58A6E6".into(), name: "Bald".into() };
        let orange = DueMarker { frame: MarkerFrame::Hexagon, symbol: None, color: "#E5A04B".into(), name: String::new() };
        let custom = vec![
            DueThreshold { under_minutes: 30 * MINUTES_PER_DAY, marker: blue.clone(), tint: false },
            DueThreshold { under_minutes: 2 * MINUTES_PER_HOUR, marker: orange.clone(), tint: true },
            DueThreshold { under_minutes: 1, marker: DueMarker::default(), tint: false },
        ];
        let ev = |now: &str| evaluate(&custom, "2026-11-20T12:00", at(now)).unwrap();
        assert!(ev("2026-10-01T00:00:00").threshold.is_none());
        assert_eq!(ev("2026-11-01T00:00:00").marker(), Some(&blue));
        assert_eq!(ev("2026-11-01T00:00:00").tint(), None, "Tönung ist aus");
        let o = ev("2026-11-20T10:30:00");
        assert_eq!((o.marker(), o.tint()), (Some(&orange), Some("#E5A04B")), "Tönung an");
        let none = ev("2026-11-20T12:00:00");
        assert!(none.threshold.is_some() && none.marker().is_none() && !none.flagged(), "die kleinste Schwelle gewinnt, auch wenn sie nichts zeichnet");
        // Reihenfolge der Liste ist egal
        let mut rev = custom.clone();
        rev.reverse();
        assert_eq!(evaluate(&rev, "2026-11-20T12:00", at("2026-11-20T10:30:00")).unwrap().marker(), Some(&orange));
        // Zählung für die Übersicht
        let dues = ["2026-11-19", "2026-11-20T12:00", "2026-11-22", "kaputt", "2027-06-01"];
        assert_eq!(count(&default_thresholds(), dues, at("2026-11-20T11:00:00")), (1, 3));
        assert_eq!(count_text(1, 3, Lang::En), "1 overdue");
        assert_eq!(count_text(0, 3, Lang::De), "3 markiert");
        assert_eq!(count_text(0, 0, Lang::En), "");
    }

    #[test]
    fn threshold_lists_are_validated_and_cleaned() {
        let t = |m: i64, c: &str| DueThreshold { under_minutes: m, marker: DueMarker::warning(c), tint: false };
        let mut d = default_thresholds();
        assert_eq!(validate_thresholds(&d).unwrap(), d);
        d.reverse();
        assert_eq!(validate_thresholds(&d).unwrap(), d, "Reihenfolge bleibt");
        assert_eq!(validate_thresholds(&[t(0, RED)]), Err(ThresholdProblem::Duration));
        assert_eq!(validate_thresholds(&[t(-5, RED)]), Err(ThresholdProblem::Duration));
        assert_eq!(validate_thresholds(&[t(MAX_MINUTES + 1, RED)]), Err(ThresholdProblem::Duration));
        assert_eq!(validate_thresholds(&[t(60, RED), t(60, YELLOW)]), Err(ThresholdProblem::Duplicate));
        assert_eq!(validate_thresholds(&(1..=21).map(|i| t(i, RED)).collect::<Vec<_>>()), Err(ThresholdProblem::TooMany));
        assert_eq!(validate_thresholds(&[t(60, "rot")]), Err(ThresholdProblem::Color));
        let mut bad = t(60, RED);
        bad.marker.symbol = Some("gibtsnicht".into());
        assert_eq!(validate_thresholds(&[bad]), Err(ThresholdProblem::Symbol));
        let mut long = t(60, RED);
        long.marker.name = "x".repeat(41);
        assert_eq!(validate_thresholds(&[long]), Err(ThresholdProblem::Name));
        let mut messy = t(60, "e06c75");
        messy.marker.symbol = Some(" ".into());
        messy.marker.name = "  Sehr   bald ".into();
        let clean = validate_thresholds(&[messy]).unwrap();
        assert_eq!((clean[0].marker.color.as_str(), clean[0].marker.symbol.as_deref(), clean[0].marker.name.as_str()), ("#E06C75", None, "Sehr bald"));
        assert!(validate_thresholds(&[]).unwrap().is_empty());
        assert!(ThresholdProblem::ALL.iter().all(|p| p.message(Lang::De) != p.message(Lang::En)));
        // JSON: Standard (auch das der Migration), unbekannte/kaputte Texte → leer, fehlende Teile → Standardwerte
        assert_eq!(parse_thresholds(DEFAULT_THRESHOLDS_JSON), default_thresholds());
        assert_eq!(thresholds_json(&default_thresholds()), DEFAULT_THRESHOLDS_JSON);
        assert!(parse_thresholds("kaputt").is_empty());
        assert!(parse_thresholds(r#"[{"under_minutes":0}]"#).is_empty());
        let min = parse_thresholds(r#"[{"under_minutes":60}]"#);
        assert_eq!(min, vec![DueThreshold { under_minutes: 60, marker: DueMarker::default(), tint: false }]);
        assert!(min[0].marker.is_none());
        assert_eq!(parse_thresholds(r#"[{"under_minutes":60,"marker":{"frame":"octagon"},"tint":true}]"#)[0].marker.frame, MarkerFrame::Octagon);
    }

    #[test]
    fn tooltips_in_both_languages() {
        assert_eq!(tooltip(3 * MINUTES_PER_DAY + 5, Lang::En), "due in 3 days");
        assert_eq!(tooltip(3 * MINUTES_PER_DAY + 5, Lang::De), "fällig in 3 Tagen");
        assert_eq!(tooltip(MINUTES_PER_DAY, Lang::En), "due in 1 day");
        assert_eq!(tooltip(MINUTES_PER_DAY, Lang::De), "fällig in 1 Tag");
        assert_eq!(tooltip(-125, Lang::En), "overdue by 2 h");
        assert_eq!(tooltip(-125, Lang::De), "überfällig seit 2 h");
        assert_eq!(tooltip(45, Lang::En), "due in 45 min");
        assert_eq!(tooltip(-1, Lang::De), "überfällig seit 1 min");
        assert_eq!(tooltip(0, Lang::En), "due in 0 min");
        let st = evaluate(&default_thresholds(), "2026-11-20", at("2026-11-17T00:00:00")).unwrap();
        assert_eq!(tooltip(st.minutes_left, Lang::En), "due in 4 days");
    }

    #[test]
    fn durations_frames_and_names() {
        assert_eq!(split_duration(10080), (1, DurationUnit::Weeks));
        assert_eq!(split_duration(2880), (2, DurationUnit::Days));
        assert_eq!(split_duration(120), (2, DurationUnit::Hours));
        assert_eq!(split_duration(90), (90, DurationUnit::Minutes));
        assert_eq!(duration_label(10080, Lang::De), "1 Woche");
        assert_eq!(duration_label(1440, Lang::En), "1 day");
        assert_eq!(duration_label(4320, Lang::De), "3 Tage");
        assert!(MarkerFrame::ALL.iter().all(|f| MarkerFrame::parse(f.as_str()) == Some(*f) && f.label(Lang::De) != f.label(Lang::En)));
        assert_eq!(MarkerFrame::parse("INVERTED_TRIANGLE"), Some(MarkerFrame::InvertedTriangle));
        assert_eq!(MarkerFrame::parse("stern"), None);
        assert!(crate::planning::icons::is_valid(ALERT_SYMBOL), "das Standard-Symbol gibt es im Icon-Satz");
    }
}
