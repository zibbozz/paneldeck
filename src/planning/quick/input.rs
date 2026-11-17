//! Eingabeformen des Quick-Menüs: Welche Eingabe zeigt die Menüzeile nach Enter für ein Feld ([`InputKind`]), und wie wird der
//! eingegebene Text geprüft und in den gespeicherten Wert verwandelt ([`parse_input`])?
//!
//! Der Wert hat das Format der Ticket-Felder (`fields.rs`): Text/Rich = String, Checkbox = Bool, Zahl = Zahl, Dropdown =
//! Kennung des Werts, Mehrfachauswahl = Liste von Kennungen, Labels = Liste von Strings, Datum = `JJJJ-MM-TT`,
//! Datum+Uhrzeit = `JJJJ-MM-TTTHH:MM[:SS]`. Eine **leere Eingabe** bedeutet „Feld leeren“ und ergibt `Value::Null` (nur die
//! Checkbox braucht immer eine Antwort). Verweis-Felder (Ticket, Terminal, Version, Meilenstein) werden in der Oberfläche
//! aus einer Liste gewählt; ihr Text lässt sich nicht in einen Wert verwandeln ([`InputError::PickOnly`]).

use super::matcher::fold;
use crate::planning::fields::{Field, FieldKind};
use crate::state::Lang;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use serde_json::{json, Value};

/// Höchstlänge eines Rich-Text-Wertes.
pub const MAX_RICH_CHARS: usize = 100_000;
/// Standard-Höchstlänge eines Text-Feldes ohne eigene Grenze.
pub const MAX_TEXT_CHARS: usize = 10_000;
/// Höchstlänge/-zahl bei Labels.
pub const MAX_LABEL_CHARS: usize = 40;
pub const MAX_LABELS: usize = 50;
/// Längste akzeptierte Eingabe überhaupt (Schutz gegen Einfügen riesiger Texte).
const MAX_INPUT_CHARS: usize = 200_000;

/// Ein auswählbarer Wert (Dropdown, Mehrfachauswahl).
#[derive(Debug, Clone, PartialEq)]
pub struct InputOption {
    pub id: i64,
    /// Anzeigename in der aktuellen Sprache.
    pub label: String,
    /// Weitere Namen, die ebenfalls passen (der Name in der anderen Sprache).
    pub aliases: Vec<String>,
    pub color: Option<String>,
}

/// Welche Eingabe die Menüzeile nach Enter zeigt.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    Text { max_length: Option<u32> },
    Number { min: Option<f64>, max: Option<f64>, decimals: Option<u8>, unit: Option<String> },
    Dropdown { values: Vec<InputOption> },
    MultiSelect { values: Vec<InputOption> },
    /// Freie Marken, mit Komma getrennt; neue entstehen beim Eingeben.
    Labels,
    Date,
    /// Datum+Uhrzeit; `time_optional` = ein Wert darf auch nur ein Datum sein (die eingebaute Fälligkeit, `date_only_ok`).
    DateTime { time_optional: bool },
    Checkbox,
    /// Suche nach Ticketschlüssel/Titel; `target_types` = erlaubte Typen (leer = jeder).
    TicketSearch { target_types: Vec<i64> },
    TerminalPick,
    VersionPick,
    MilestonePick,
    /// Größeres Eingabefeld (Markdown).
    RichTextEditor { rows: Option<u32> },
}

impl InputKind {
    /// Die Eingabeform für ein Feld (Namen der Werte in der Sprache `lang`, der Name der anderen Sprache als Alias).
    pub fn for_field(f: &Field, lang: Lang) -> InputKind {
        let other = match lang {
            Lang::De => Lang::En,
            Lang::En => Lang::De,
        };
        let values = || -> Vec<InputOption> {
            f.options
                .values
                .iter()
                .map(|v| {
                    let label = v.display(lang);
                    let alt = v.display(other);
                    InputOption { id: v.id, aliases: if alt != label { vec![alt] } else { Vec::new() }, label, color: v.color.clone() }
                })
                .collect()
        };
        match f.kind {
            FieldKind::Text => InputKind::Text { max_length: f.options.max_length },
            FieldKind::Rich => InputKind::RichTextEditor { rows: f.options.rows },
            FieldKind::Checkbox => InputKind::Checkbox,
            FieldKind::Dropdown => InputKind::Dropdown { values: values() },
            FieldKind::Number => InputKind::Number { min: f.options.min, max: f.options.max, decimals: f.options.decimals, unit: f.options.unit.clone() },
            FieldKind::Ticket => InputKind::TicketSearch { target_types: f.options.target_types.clone() },
            FieldKind::Terminal => InputKind::TerminalPick,
            FieldKind::Version => InputKind::VersionPick,
            FieldKind::Milestone => InputKind::MilestonePick,
            FieldKind::Date => InputKind::Date,
            FieldKind::Datetime => InputKind::DateTime { time_optional: f.options.date_only_ok() },
            FieldKind::Multi => InputKind::MultiSelect { values: values() },
            FieldKind::Labels => InputKind::Labels,
        }
    }

    /// Lässt sich der eingegebene Text direkt in einen Wert verwandeln? (Sonst wählt die Oberfläche aus einer Liste.)
    pub fn is_text_input(&self) -> bool {
        !matches!(self, InputKind::TicketSearch { .. } | InputKind::TerminalPick | InputKind::VersionPick | InputKind::MilestonePick)
    }

    /// Kurzer Hinweistext für die Menüzeile (Platzhalter im Eingabefeld).
    pub fn hint(&self, lang: Lang) -> String {
        let t = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        match self {
            InputKind::Text { max_length: Some(m) } => match lang {
                Lang::De => format!("Text (höchstens {m} Zeichen)"),
                Lang::En => format!("Text (at most {m} characters)"),
            },
            InputKind::Text { max_length: None } => t("Text", "Text"),
            InputKind::Number { min, max, unit, .. } => {
                let range = match (min, max) {
                    (Some(a), Some(b)) => format!(" ({} – {})", num_text(*a), num_text(*b)),
                    (Some(a), None) => format!(" (≥ {})", num_text(*a)),
                    (None, Some(b)) => format!(" (≤ {})", num_text(*b)),
                    (None, None) => String::new(),
                };
                let unit = unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default();
                format!("{}{range}{unit}", lang.t("Zahl", "Number"))
            }
            InputKind::Dropdown { .. } => t("Wert wählen (Name oder Nummer)", "Choose a value (name or number)"),
            InputKind::MultiSelect { .. } => t("Werte wählen (mit Komma getrennt)", "Choose values (comma separated)"),
            InputKind::Labels => t("Labels (mit Komma getrennt)", "Labels (comma separated)"),
            InputKind::Date => t("Datum (JJJJ-MM-TT, TT.MM.JJJJ, heute)", "Date (YYYY-MM-DD, DD.MM.YYYY, today)"),
            InputKind::DateTime { time_optional: false } => t("Datum und Uhrzeit (z. B. 14.11.2026 09:30)", "Date and time (e.g. 2026-11-14 09:30)"),
            InputKind::DateTime { time_optional: true } => {
                t("Datum, Uhrzeit optional (z. B. 14.11.2026 oder 14.11.2026 09:30)", "Date, time optional (e.g. 2026-11-14 or 2026-11-14 09:30)")
            }
            InputKind::Checkbox => t("Ja oder Nein", "Yes or no"),
            InputKind::TicketSearch { .. } => t("Ticket suchen (Schlüssel oder Titel)", "Search a ticket (key or title)"),
            InputKind::TerminalPick => t("Terminal wählen", "Choose a terminal"),
            InputKind::VersionPick => t("Version wählen", "Choose a version"),
            InputKind::MilestonePick => t("Meilenstein wählen", "Choose a milestone"),
            InputKind::RichTextEditor { .. } => t("Text (Markdown)", "Text (Markdown)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputError {
    /// Die Checkbox braucht eine Antwort.
    Empty,
    /// Dieses Feld wird aus einer Liste gewählt, nicht getippt.
    PickOnly,
    TooLong { max: usize },
    /// Steuerzeichen im Text.
    InvalidChars,
    NotANumber,
    OutOfRange { min: Option<f64>, max: Option<f64> },
    NotADate,
    NotADateTime,
    NotYesNo,
    /// Kein Wert dieses Namens.
    NoMatch(String),
    /// Mehrere Werte passen.
    Ambiguous(String),
    TooManyLabels { max: usize },
}

impl InputError {
    pub fn message(&self, lang: Lang) -> String {
        let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
        let range = |min: &Option<f64>, max: &Option<f64>| match (min, max) {
            (Some(a), Some(b)) => format!("{} – {}", num_text(*a), num_text(*b)),
            (Some(a), None) => format!("≥ {}", num_text(*a)),
            (None, Some(b)) => format!("≤ {}", num_text(*b)),
            (None, None) => String::new(),
        };
        match self {
            InputError::Empty => s("Bitte Ja oder Nein eingeben.", "Please enter yes or no."),
            InputError::PickOnly => s("Dieses Feld wird aus einer Liste gewählt.", "This field is chosen from a list."),
            InputError::TooLong { max } => match lang {
                Lang::De => format!("Der Text ist zu lang (höchstens {max} Zeichen)."),
                Lang::En => format!("The text is too long (at most {max} characters)."),
            },
            InputError::InvalidChars => s("Der Text enthält Zeichen, die hier nicht erlaubt sind.", "The text contains characters that are not allowed here."),
            InputError::NotANumber => s("Das ist keine Zahl (erlaubt: 12, 3,5 oder 3.5).", "That is not a number (allowed: 12, 3.5 or 3,5)."),
            InputError::OutOfRange { min, max } => match lang {
                Lang::De => format!("Die Zahl muss zwischen {} liegen.", range(min, max)),
                Lang::En => format!("The number must be in the range {}.", range(min, max)),
            },
            InputError::NotADate => s("Kein gültiges Datum (z. B. 2026-11-14 oder 14.11.2026).", "Not a valid date (e.g. 2026-11-14 or 14.11.2026)."),
            InputError::NotADateTime => s("Kein gültiges Datum mit Uhrzeit (z. B. 14.11.2026 09:30).", "Not a valid date and time (e.g. 2026-11-14 09:30)."),
            InputError::NotYesNo => s("Bitte Ja oder Nein eingeben.", "Please enter yes or no."),
            InputError::NoMatch(t) => match lang {
                Lang::De => format!("Kein Wert passt zu {}.", lang.q(t)),
                Lang::En => format!("No value matches {}.", lang.q(t)),
            },
            InputError::Ambiguous(t) => match lang {
                Lang::De => format!("{} ist nicht eindeutig.", lang.q(t)),
                Lang::En => format!("{} is ambiguous.", lang.q(t)),
            },
            InputError::TooManyLabels { max } => match lang {
                Lang::De => format!("Zu viele Labels (höchstens {max})."),
                Lang::En => format!("Too many labels (at most {max})."),
            },
        }
    }
}

fn num_text(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

/// Prüft den Text und verwandelt ihn in den gespeicherten Wert. Für Schlüsselwörter wie „heute“ wird die lokale Uhr benutzt;
/// [`parse_input_at`] nimmt die Zeit als Parameter (rein, testbar).
pub fn parse_input(kind: &InputKind, text: &str) -> Result<Value, InputError> {
    parse_input_at(kind, text, chrono::Local::now().naive_local())
}

pub fn parse_input_at(kind: &InputKind, text: &str, now: NaiveDateTime) -> Result<Value, InputError> {
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err(InputError::TooLong { max: MAX_INPUT_CHARS });
    }
    let t = text.trim();
    if !kind.is_text_input() {
        return Err(InputError::PickOnly);
    }
    if t.is_empty() {
        return if matches!(kind, InputKind::Checkbox) { Err(InputError::Empty) } else { Ok(Value::Null) };
    }
    match kind {
        InputKind::Text { max_length } => {
            if t.chars().any(char::is_control) {
                return Err(InputError::InvalidChars);
            }
            let max = max_length.map(|m| m as usize).unwrap_or(MAX_TEXT_CHARS);
            if t.chars().count() > max {
                return Err(InputError::TooLong { max });
            }
            Ok(json!(t))
        }
        InputKind::RichTextEditor { .. } => {
            let norm = text.trim_end().replace("\r\n", "\n").replace('\r', "\n");
            if norm.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
                return Err(InputError::InvalidChars);
            }
            if norm.chars().count() > MAX_RICH_CHARS {
                return Err(InputError::TooLong { max: MAX_RICH_CHARS });
            }
            Ok(json!(norm.trim_start_matches('\n')))
        }
        InputKind::Number { min, max, decimals, unit } => parse_number(t, *min, *max, *decimals, unit.as_deref()),
        InputKind::Checkbox => parse_bool(t).map(Value::Bool).ok_or(InputError::NotYesNo),
        InputKind::Date => parse_date(t, now.date()).map(|d| json!(d.format("%Y-%m-%d").to_string())).ok_or(InputError::NotADate),
        InputKind::DateTime { time_optional } => parse_datetime(t, now)
            .map(|d| {
                // Ohne Uhrzeit (keine Zeitangabe mit `:`, nicht „jetzt“) bleibt eine Fälligkeit ein reines Datum (= Ende des
                // Tages), nicht Mitternacht (#214).
                let time_given = t.contains(':') || matches!(fold(t).as_str(), "now" | "jetzt");
                if *time_optional && !time_given { json!(d.date().format("%Y-%m-%d").to_string()) } else { json!(format_datetime(d)) }
            })
            .ok_or(InputError::NotADateTime),
        InputKind::Dropdown { values } => Ok(json!(match_option(values, t)?.id)),
        InputKind::MultiSelect { values } => {
            let mut ids: Vec<i64> = Vec::new();
            for part in t.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
                let id = match_option(values, part)?.id;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            Ok(json!(ids))
        }
        InputKind::Labels => parse_labels(t),
        InputKind::TicketSearch { .. } | InputKind::TerminalPick | InputKind::VersionPick | InputKind::MilestonePick => Err(InputError::PickOnly),
    }
}

// ------------------------------------------------------------------ Zahl

fn parse_number(t: &str, min: Option<f64>, max: Option<f64>, decimals: Option<u8>, unit: Option<&str>) -> Result<Value, InputError> {
    let mut s = t.to_string();
    // Einheit am Ende („3 h“) darf mitgetippt werden.
    if let Some(u) = unit.map(str::trim).filter(|u| !u.is_empty()) {
        let n = u.chars().count();
        if s.to_lowercase().ends_with(&u.to_lowercase()) && s.chars().count() >= n {
            if let Some((cut, _)) = s.char_indices().rev().nth(n - 1) {
                s.truncate(cut);
            }
        }
    }
    let s: String = s.chars().filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '\'').collect();
    let (sign, digits) = match s.strip_prefix('-') {
        Some(r) => (-1.0, r),
        None => (1.0, s.strip_prefix('+').unwrap_or(&s)),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') || !digits.chars().any(|c| c.is_ascii_digit()) {
        return Err(InputError::NotANumber);
    }
    // Dezimaltrenner: bei „1.234,5“ bzw. „1,234.5“ der letzte, der andere ist Tausendertrenner; ein einzelner Trenner ist dezimal.
    let last_comma = digits.rfind(',');
    let last_dot = digits.rfind('.');
    let normalized = match (last_comma, last_dot) {
        (Some(c), Some(d)) => {
            let (dec, thou) = if c > d { (',', '.') } else { ('.', ',') };
            if digits.matches(dec).count() != 1 {
                return Err(InputError::NotANumber);
            }
            digits.replace(thou, "").replace(dec, ".")
        }
        (Some(_), None) | (None, Some(_)) => {
            let sep = if last_comma.is_some() { ',' } else { '.' };
            if digits.matches(sep).count() != 1 {
                return Err(InputError::NotANumber);
            }
            digits.replace(sep, ".")
        }
        (None, None) => digits.to_string(),
    };
    if normalized.starts_with('.') && normalized.len() == 1 || normalized.ends_with('.') && normalized.len() == 1 {
        return Err(InputError::NotANumber);
    }
    let mut x: f64 = normalized.parse().map_err(|_| InputError::NotANumber)?;
    if !x.is_finite() {
        return Err(InputError::NotANumber);
    }
    x *= sign;
    let places = decimals.unwrap_or(6).min(6) as i32;
    let f = 10f64.powi(places);
    x = (x * f).round() / f;
    if x == 0.0 {
        x = 0.0; // kein „-0“
    }
    if min.is_some_and(|m| x < m) || max.is_some_and(|m| x > m) {
        return Err(InputError::OutOfRange { min, max });
    }
    if x.fract() == 0.0 && x.abs() < 9e15 { Ok(json!(x as i64)) } else { Ok(json!(x)) }
}

// ------------------------------------------------------------------ Checkbox

fn parse_bool(t: &str) -> Option<bool> {
    match fold(t).as_str() {
        "x" | "1" | "true" | "yes" | "y" | "ja" | "j" | "an" | "on" | "wahr" | "✓" | "✔" | "check" | "checked" => Some(true),
        "0" | "false" | "no" | "n" | "nein" | "aus" | "off" | "falsch" | "-" | "unchecked" => Some(false),
        _ => None,
    }
}

// ------------------------------------------------------------------ Datum

fn two_digit_year(y: i32) -> i32 {
    2000 + y
}

/// `JJJJ-MM-TT`, `JJJJ/MM/TT`, `TT.MM.JJJJ` (auch `T.M.JJ`).
fn split_date(t: &str) -> Option<NaiveDate> {
    let num = |s: &str, max_len: usize| -> Option<i32> {
        if s.is_empty() || s.len() > max_len || !s.bytes().all(|b| b.is_ascii_digit()) { None } else { s.parse().ok() }
    };
    if t.contains('.') {
        let p: Vec<&str> = t.trim_end_matches('.').split('.').collect();
        if p.len() != 3 {
            return None;
        }
        let (d, m, y) = (num(p[0], 2)?, num(p[1], 2)?, num(p[2], 4)?);
        let y = match p[2].len() {
            4 => y,
            2 => two_digit_year(y),
            _ => return None,
        };
        return NaiveDate::from_ymd_opt(y, m as u32, d as u32);
    }
    let sep = if t.contains('-') { '-' } else if t.contains('/') { '/' } else { return None };
    let p: Vec<&str> = t.split(sep).collect();
    if p.len() != 3 || p[0].len() != 4 {
        return None;
    }
    NaiveDate::from_ymd_opt(num(p[0], 4)?, num(p[1], 2)? as u32, num(p[2], 2)? as u32)
}

fn parse_date(t: &str, today: NaiveDate) -> Option<NaiveDate> {
    match fold(t).as_str() {
        "today" | "heute" => Some(today),
        "tomorrow" | "morgen" => today.succ_opt(),
        "yesterday" | "gestern" => today.pred_opt(),
        _ => split_date(t),
    }
}

fn parse_time(t: &str) -> Option<NaiveTime> {
    let t = t.trim().trim_end_matches(|c: char| c.is_alphabetic() || c == '.').trim(); // „Uhr“
    let p: Vec<&str> = t.split(':').collect();
    if !(2..=3).contains(&p.len()) || p.iter().any(|x| x.is_empty() || x.len() > 2 || !x.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let n = |i: usize| p.get(i).and_then(|x| x.parse::<u32>().ok());
    NaiveTime::from_hms_opt(n(0)?, n(1)?, n(2).unwrap_or(0))
}

fn parse_datetime(t: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let f = fold(t);
    if f == "now" || f == "jetzt" {
        return Some(now.with_second(0)?.with_nanosecond(0)?);
    }
    // Datum und Uhrzeit trennt `T` oder Leerraum; nur Uhrzeit = heute.
    let mut prev_digit = false;
    let mut split = None;
    for (i, c) in t.char_indices() {
        if c == ' ' || (c == 'T' && prev_digit) {
            split = Some(i);
            break;
        }
        prev_digit = c.is_ascii_digit();
    }
    let (date_part, time_part) = match split {
        Some(i) => (t[..i].trim(), Some(t[i + 1..].trim())),
        None => (t, None),
    };
    if time_part.is_none() && date_part.contains(':') {
        return Some(NaiveDateTime::new(now.date(), parse_time(date_part)?));
    }
    let date = parse_date(date_part, now.date())?;
    let time = match time_part {
        Some(tp) if !tp.is_empty() => parse_time(tp)?,
        Some(_) => return None,
        None => NaiveTime::from_hms_opt(0, 0, 0)?,
    };
    Some(NaiveDateTime::new(date, time))
}

fn format_datetime(d: NaiveDateTime) -> String {
    if d.second() == 0 { d.format("%Y-%m-%dT%H:%M").to_string() } else { d.format("%Y-%m-%dT%H:%M:%S").to_string() }
}

// ------------------------------------------------------------------ Auswahl

/// Findet den Wert zu einer Eingabe: Name (exakt), Nummer (1-basiert), eindeutiger Namensanfang, eindeutiger Teilstring.
fn match_option<'a>(values: &'a [InputOption], t: &str) -> Result<&'a InputOption, InputError> {
    let q = fold(t);
    let names = |v: &InputOption| -> Vec<String> { std::iter::once(&v.label).chain(v.aliases.iter()).map(|n| fold(n)).collect() };
    if let Some(v) = values.iter().find(|v| names(v).contains(&q)) {
        return Ok(v);
    }
    if q.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(n) = q.parse::<usize>() {
            if (1..=values.len()).contains(&n) {
                return Ok(&values[n - 1]);
            }
        }
    }
    for tier in [0, 1] {
        let hits: Vec<&InputOption> = values.iter().filter(|v| names(v).iter().any(|n| if tier == 0 { n.starts_with(&q) } else { n.contains(&q) })).collect();
        match hits.len() {
            0 => {}
            1 => return Ok(hits[0]),
            _ => return Err(InputError::Ambiguous(t.to_string())),
        }
    }
    Err(InputError::NoMatch(t.to_string()))
}

fn parse_labels(t: &str) -> Result<Value, InputError> {
    let mut out: Vec<String> = Vec::new();
    for part in t.split([',', ';']).map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|p| !p.is_empty()) {
        if part.chars().any(char::is_control) {
            return Err(InputError::InvalidChars);
        }
        if part.chars().count() > MAX_LABEL_CHARS {
            return Err(InputError::TooLong { max: MAX_LABEL_CHARS });
        }
        if !out.iter().any(|o| o.to_lowercase() == part.to_lowercase()) {
            out.push(part);
        }
    }
    if out.len() > MAX_LABELS {
        return Err(InputError::TooManyLabels { max: MAX_LABELS });
    }
    if out.is_empty() { Ok(Value::Null) } else { Ok(json!(out)) }
}

/// Zerlegt einen Ticketschlüssel wie `pan-423` in (`PAN`, 423); für die Ticket-Suche des Menüs.
pub fn parse_ticket_ref(text: &str) -> Option<(String, i64)> {
    let t = text.trim();
    let (k, n) = t.rsplit_once('-')?;
    let key = k.trim().to_ascii_uppercase();
    let num: i64 = n.trim().parse().ok()?;
    let ok = (2..=10).contains(&key.chars().count()) && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && key.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    if ok && num > 0 { Some((key, num)) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{FieldOptions, OptionValue};
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(10, 20, 30).unwrap()
    }

    fn p(kind: &InputKind, text: &str) -> Result<Value, InputError> {
        parse_input_at(kind, text, now())
    }

    fn prio() -> InputKind {
        let opt = |id: i64, l: &str, a: &str| InputOption { id, label: l.into(), aliases: vec![a.into()], color: None };
        InputKind::Dropdown { values: vec![opt(1, "Highest", "Höchste"), opt(2, "High", "Hoch"), opt(3, "Medium", "Normal"), opt(4, "Low", "Niedrig"), opt(5, "Lowest", "Niedrigste")] }
    }

    fn num(min: Option<f64>, max: Option<f64>, decimals: Option<u8>, unit: Option<&str>) -> InputKind {
        InputKind::Number { min, max, decimals, unit: unit.map(str::to_string) }
    }

    fn field(kind: FieldKind, options: FieldOptions) -> Field {
        Field { id: 1, type_id: 1, key: None, name: Some("F".into()), kind, options, required: false, default_value: None, help: String::new(), section: String::new(), position: 0, show_on_card: false, builtin: false }
    }

    #[test]
    fn every_field_kind_has_an_input_form() {
        let opts = FieldOptions {
            values: vec![OptionValue { id: 1, key: Some("priority.high".into()), name: None, color: Some("#E5A04B".into()) }, OptionValue { id: 2, key: None, name: Some("Eigen".into()), color: None }],
            min: Some(0.0),
            max: Some(10.0),
            decimals: Some(1),
            unit: Some("h".into()),
            max_length: Some(30),
            rows: Some(6),
            target_types: vec![4, 5],
            time_optional: None,
        };
        let expect = |k: FieldKind| InputKind::for_field(&field(k, opts.clone()), Lang::En);
        assert_eq!(expect(FieldKind::Text), InputKind::Text { max_length: Some(30) });
        assert_eq!(expect(FieldKind::Rich), InputKind::RichTextEditor { rows: Some(6) });
        assert_eq!(expect(FieldKind::Checkbox), InputKind::Checkbox);
        assert_eq!(expect(FieldKind::Number), num(Some(0.0), Some(10.0), Some(1), Some("h")));
        assert_eq!(expect(FieldKind::Ticket), InputKind::TicketSearch { target_types: vec![4, 5] });
        assert_eq!(expect(FieldKind::Terminal), InputKind::TerminalPick);
        assert_eq!(expect(FieldKind::Version), InputKind::VersionPick);
        assert_eq!(expect(FieldKind::Milestone), InputKind::MilestonePick);
        assert_eq!(expect(FieldKind::Date), InputKind::Date);
        assert_eq!(expect(FieldKind::Datetime), InputKind::DateTime { time_optional: false });
        let due_opts = FieldOptions { time_optional: Some(true), ..FieldOptions::default() };
        assert_eq!(InputKind::for_field(&field(FieldKind::Datetime, due_opts), Lang::En), InputKind::DateTime { time_optional: true });
        assert_ne!(InputKind::DateTime { time_optional: true }.hint(Lang::En), InputKind::DateTime { time_optional: false }.hint(Lang::En));
        assert_eq!(expect(FieldKind::Labels), InputKind::Labels);
        let InputKind::Dropdown { values } = expect(FieldKind::Dropdown) else { panic!() };
        assert_eq!(values.len(), 2);
        assert_eq!((values[0].label.as_str(), values[0].aliases.clone(), values[0].color.as_deref()), ("High", vec!["Hoch".to_string()], Some("#E5A04B")));
        assert!(values[1].aliases.is_empty(), "eigener Name gilt in beiden Sprachen");
        assert!(matches!(expect(FieldKind::Multi), InputKind::MultiSelect { .. }));
        // Sprache wirkt auf die Namen
        let InputKind::Dropdown { values } = InputKind::for_field(&field(FieldKind::Dropdown, opts.clone()), Lang::De) else { panic!() };
        assert_eq!((values[0].label.as_str(), values[0].aliases.clone()), ("Hoch", vec!["High".to_string()]));
        for k in FieldKind::ALL {
            assert!(!expect(k).hint(Lang::En).is_empty() && !expect(k).hint(Lang::De).is_empty());
        }
    }

    #[test]
    fn hints_differ_by_language_and_mention_limits() {
        let n = num(Some(0.0), Some(100.0), None, Some("%"));
        assert_eq!(n.hint(Lang::En), "Number (0 – 100) %");
        assert_eq!(n.hint(Lang::De), "Zahl (0 – 100) %");
        assert_ne!(InputKind::Date.hint(Lang::De), InputKind::Date.hint(Lang::En));
        assert!(InputKind::Text { max_length: Some(30) }.hint(Lang::En).contains("30"));
    }

    #[test]
    fn text_input() {
        let k = InputKind::Text { max_length: Some(5) };
        assert_eq!(p(&k, "  hello "), Ok(json!("hello")));
        assert_eq!(p(&k, "hello!"), Err(InputError::TooLong { max: 5 }));
        assert_eq!(p(&k, "a\nb"), Err(InputError::InvalidChars));
        assert_eq!(p(&k, "   "), Ok(Value::Null), "leer = Feld leeren");
        assert_eq!(p(&InputKind::Text { max_length: None }, &"x".repeat(MAX_TEXT_CHARS + 1)), Err(InputError::TooLong { max: MAX_TEXT_CHARS }));
        assert_eq!(p(&InputKind::Text { max_length: None }, "Größe ✓"), Ok(json!("Größe ✓")));
    }

    #[test]
    fn rich_text_keeps_lines() {
        let k = InputKind::RichTextEditor { rows: None };
        assert_eq!(p(&k, "# Title\r\n\r\n- a\r\n- b\n"), Ok(json!("# Title\n\n- a\n- b")));
        assert_eq!(p(&k, "a\u{0000}b"), Err(InputError::InvalidChars));
        assert_eq!(p(&k, ""), Ok(Value::Null));
        assert_eq!(p(&k, &"x".repeat(MAX_RICH_CHARS + 1)), Err(InputError::TooLong { max: MAX_RICH_CHARS }));
    }

    #[test]
    fn numbers_with_comma_or_point() {
        let k = num(None, None, None, None);
        assert_eq!(p(&k, "5"), Ok(json!(5)));
        assert_eq!(p(&k, "3,5"), Ok(json!(3.5)));
        assert_eq!(p(&k, "3.5"), Ok(json!(3.5)));
        assert_eq!(p(&k, " -2,25 "), Ok(json!(-2.25)));
        assert_eq!(p(&k, "+7"), Ok(json!(7)));
        assert_eq!(p(&k, "1.234,5"), Ok(json!(1234.5)));
        assert_eq!(p(&k, "1,234.5"), Ok(json!(1234.5)));
        assert_eq!(p(&k, "1 000"), Ok(json!(1000)));
        assert_eq!(p(&k, ".5"), Ok(json!(0.5)));
        assert_eq!(p(&k, "5."), Ok(json!(5)));
        assert_eq!(p(&k, "-0"), Ok(json!(0)));
        assert_eq!(p(&k, "2,0"), Ok(json!(2)), "ganze Zahlen werden als ganze Zahlen gespeichert");
        assert_eq!(p(&k, ""), Ok(Value::Null));
    }

    #[test]
    fn bad_numbers_are_rejected() {
        let k = num(None, None, None, None);
        for bad in ["abc", "1,2,3", "1.2.3", "1e5", "--1", "-", ".", ",", "1,2.3,4", "NaN", "inf", "5 5x", "0x10", "١٢"] {
            assert_eq!(p(&k, bad), Err(InputError::NotANumber), "{bad}");
        }
    }

    #[test]
    fn number_limits_decimals_and_unit() {
        let k = num(Some(0.0), Some(10.0), Some(1), Some("h"));
        assert_eq!(p(&k, "2,56"), Ok(json!(2.6)), "auf die erlaubten Nachkommastellen gerundet");
        assert_eq!(p(&k, "2.54"), Ok(json!(2.5)));
        assert_eq!(p(&k, "3 h"), Ok(json!(3)));
        assert_eq!(p(&k, "3H"), Ok(json!(3)));
        assert_eq!(p(&k, "10"), Ok(json!(10)));
        assert_eq!(p(&k, "10,04"), Ok(json!(10)), "nach dem Runden im Bereich");
        assert_eq!(p(&k, "11"), Err(InputError::OutOfRange { min: Some(0.0), max: Some(10.0) }));
        assert_eq!(p(&k, "-0,04"), Ok(json!(0)), "rundet auf 0 und ist dann im Bereich");
        assert_eq!(p(&k, "-0,1"), Err(InputError::OutOfRange { min: Some(0.0), max: Some(10.0) }));
        assert_eq!(p(&k, "-1"), Err(InputError::OutOfRange { min: Some(0.0), max: Some(10.0) }));
        assert_eq!(p(&num(None, None, Some(0), None), "2,5"), Ok(json!(3)));
        assert_eq!(p(&num(None, None, None, None), "0,1234567"), Ok(json!(0.123457)), "höchstens sechs Stellen");
        assert!(p(&num(None, None, None, None), &"9".repeat(400)).is_err(), "unendlich = ungültig");
        assert_eq!(p(&num(None, None, Some(0), None), "123456789012345678"), Ok(json!(123456789012345680.0_f64)));
    }

    #[test]
    fn dropdown_by_name_number_prefix_and_alias() {
        let k = prio();
        assert_eq!(p(&k, "High"), Ok(json!(2)));
        assert_eq!(p(&k, "hoch"), Ok(json!(2)), "Name der anderen Sprache");
        assert_eq!(p(&k, "NORMAL"), Ok(json!(3)));
        assert_eq!(p(&k, "3"), Ok(json!(3)), "Nummer in der Liste");
        assert_eq!(p(&k, "5"), Ok(json!(5)));
        assert_eq!(p(&k, "med"), Ok(json!(3)), "eindeutiger Anfang");
        assert_eq!(p(&k, "hoe"), Err(InputError::NoMatch("hoe".into())));
        assert_eq!(p(&k, "höchs"), Ok(json!(1)));
        assert_eq!(p(&k, "niedrigs"), Ok(json!(5)));
        assert_eq!(p(&k, "ow"), Err(InputError::Ambiguous("ow".into())), "Teilstring von Low und Lowest");
    }

    #[test]
    fn dropdown_errors() {
        let k = prio();
        assert_eq!(p(&k, "nope"), Err(InputError::NoMatch("nope".into())));
        assert_eq!(p(&k, "6"), Err(InputError::NoMatch("6".into())));
        assert_eq!(p(&k, "0"), Err(InputError::NoMatch("0".into())));
        assert_eq!(p(&k, "Lo"), Err(InputError::Ambiguous("Lo".into())), "Low und Lowest");
        assert_eq!(p(&k, "h"), Err(InputError::Ambiguous("h".into())));
        assert_eq!(p(&k, ""), Ok(Value::Null));
        // eine exakte Namensübereinstimmung geht vor der Nummer
        let k2 = InputKind::Dropdown { values: vec![InputOption { id: 10, label: "A".into(), aliases: vec![], color: None }, InputOption { id: 11, label: "1".into(), aliases: vec![], color: None }] };
        assert_eq!(p(&k2, "1"), Ok(json!(11)));
        assert_eq!(p(&k2, "2"), Ok(json!(11)), "sonst Nummer");
        let empty = InputKind::Dropdown { values: vec![] };
        assert_eq!(p(&empty, "x"), Err(InputError::NoMatch("x".into())));
    }

    #[test]
    fn multi_select_lists_and_clears() {
        let InputKind::Dropdown { values } = prio() else { panic!() };
        let k = InputKind::MultiSelect { values };
        assert_eq!(p(&k, "High, Low"), Ok(json!([2, 4])));
        assert_eq!(p(&k, "1;2;3"), Ok(json!([1, 2, 3])));
        assert_eq!(p(&k, "hoch, high,2"), Ok(json!([2])), "doppelt genannt wird nur einmal gespeichert");
        assert_eq!(p(&k, "High,,"), Ok(json!([2])));
        assert_eq!(p(&k, "High, nope"), Err(InputError::NoMatch("nope".into())));
        assert_eq!(p(&k, ""), Ok(Value::Null));
    }

    #[test]
    fn labels_are_split_and_deduplicated() {
        assert_eq!(p(&InputKind::Labels, "ui, Backend ,ui;  new   tag"), Ok(json!(["ui", "Backend", "new tag"])));
        assert_eq!(p(&InputKind::Labels, ", ,"), Ok(Value::Null));
        assert_eq!(p(&InputKind::Labels, &"x".repeat(41)), Err(InputError::TooLong { max: MAX_LABEL_CHARS }));
        let many = (0..51).map(|i| format!("t{i}")).collect::<Vec<_>>().join(",");
        assert_eq!(p(&InputKind::Labels, &many), Err(InputError::TooManyLabels { max: MAX_LABELS }));
        assert_eq!(p(&InputKind::Labels, "a\u{0007}b"), Err(InputError::InvalidChars));
    }

    #[test]
    fn checkbox_words() {
        for yes in ["ja", "Ja", "yes", "Y", "x", "1", "true", "on", "an", "✓"] {
            assert_eq!(p(&InputKind::Checkbox, yes), Ok(json!(true)), "{yes}");
        }
        for no in ["nein", "No", "n", "0", "false", "off", "aus", "-"] {
            assert_eq!(p(&InputKind::Checkbox, no), Ok(json!(false)), "{no}");
        }
        assert_eq!(p(&InputKind::Checkbox, "vielleicht"), Err(InputError::NotYesNo));
        assert_eq!(p(&InputKind::Checkbox, "  "), Err(InputError::Empty));
    }

    #[test]
    fn dates_in_iso_german_and_keywords() {
        let d = |t: &str| p(&InputKind::Date, t);
        assert_eq!(d("2026-11-14"), Ok(json!("2026-11-14")));
        assert_eq!(d("14.11.2026"), Ok(json!("2026-11-14")));
        assert_eq!(d("1.2.2027"), Ok(json!("2027-02-01")));
        assert_eq!(d("1.2.27"), Ok(json!("2027-02-01")));
        assert_eq!(d("2026-1-5"), Ok(json!("2026-01-05")));
        assert_eq!(d("2026/11/14"), Ok(json!("2026-11-14")));
        assert_eq!(d("14.11.2026."), Ok(json!("2026-11-14")));
        assert_eq!(d(" Heute "), Ok(json!("2026-11-14")));
        assert_eq!(d("today"), Ok(json!("2026-11-14")));
        assert_eq!(d("morgen"), Ok(json!("2026-11-15")));
        assert_eq!(d("Tomorrow"), Ok(json!("2026-11-15")));
        assert_eq!(d("gestern"), Ok(json!("2026-11-13")));
        assert_eq!(d("29.2.2028"), Ok(json!("2028-02-29")));
        assert_eq!(d(""), Ok(Value::Null));
    }

    #[test]
    fn bad_dates_are_rejected() {
        for bad in ["2026-13-01", "2026-02-30", "31.04.2026", "29.2.2027", "14-11-2026", "26-11-14", "11/14/2026", "abc", "2026-11", "14.11", "1.2.3", "2026-11-14-1", "٢٠٢٦-١١-١٤", "0.1.2026"] {
            assert_eq!(p(&InputKind::Date, bad), Err(InputError::NotADate), "{bad}");
        }
    }

    #[test]
    fn date_times() {
        let d = |t: &str| p(&InputKind::DateTime { time_optional: false }, t);
        assert_eq!(d("2026-11-14 09:30"), Ok(json!("2026-11-14T09:30")));
        assert_eq!(d("2026-11-14T09:30"), Ok(json!("2026-11-14T09:30")));
        assert_eq!(d("14.11.2026 9:05"), Ok(json!("2026-11-14T09:05")));
        assert_eq!(d("14.11.2026 09:30:15"), Ok(json!("2026-11-14T09:30:15")));
        assert_eq!(d("14.11.2026 09:30 Uhr"), Ok(json!("2026-11-14T09:30")));
        assert_eq!(d("heute 14:00"), Ok(json!("2026-11-14T14:00")));
        assert_eq!(d("morgen 08:15"), Ok(json!("2026-11-15T08:15")));
        assert_eq!(d("14:45"), Ok(json!("2026-11-14T14:45")), "nur Uhrzeit = heute");
        assert_eq!(d("jetzt"), Ok(json!("2026-11-14T10:20")));
        assert_eq!(d("now"), Ok(json!("2026-11-14T10:20")));
        assert_eq!(d("2026-11-14"), Ok(json!("2026-11-14T00:00")), "nur Datum = Mitternacht");
        assert_eq!(d(""), Ok(Value::Null));
        for bad in ["2026-11-14 25:00", "2026-11-14 12:60", "2026-11-14 12", "2026-11-14 ", "14.11.2026 ab:cd", "xx 12:00", "2026-11-14 12:00:00:00"] {
            let r = d(bad);
            assert!(matches!(r, Err(InputError::NotADateTime)) || bad == "2026-11-14 " && r == Ok(json!("2026-11-14T00:00")), "{bad}: {r:?}");
        }
    }

    #[test]
    fn due_date_keeps_date_only_values() {
        // #214: die eingebaute Fälligkeit (Uhrzeit optional) speichert ein reines Datum, nicht Mitternacht.
        let d = |t: &str| p(&InputKind::DateTime { time_optional: true }, t);
        assert_eq!(d("2026-11-20"), Ok(json!("2026-11-20")));
        assert_eq!(d("20.11.2026"), Ok(json!("2026-11-20")));
        assert_eq!(d("today"), Ok(json!("2026-11-14")));
        assert_eq!(d("morgen"), Ok(json!("2026-11-15")));
        assert_eq!(d("Tomorrow"), Ok(json!("2026-11-15")));
        assert_eq!(d("2026-11-20 14:30"), Ok(json!("2026-11-20T14:30")));
        assert_eq!(d("2026-11-20T00:00"), Ok(json!("2026-11-20T00:00")), "ausdrücklich Mitternacht bleibt");
        assert_eq!(d("14:45"), Ok(json!("2026-11-14T14:45")));
        assert_eq!(d("now"), Ok(json!("2026-11-14T10:20")));
        assert_eq!(d(""), Ok(Value::Null));
        assert_eq!(d("xx"), Err(InputError::NotADateTime));
        // Jeder gespeicherte Wert ist eine gültige Fälligkeit.
        for t in ["2026-11-20", "today", "2026-11-20 14:30"] {
            let v = d(t).unwrap();
            assert!(crate::due::parse_stored(v.as_str().unwrap()).is_some(), "{t}");
        }
    }

    #[test]
    fn pick_fields_cannot_be_typed() {
        for k in [InputKind::TicketSearch { target_types: vec![] }, InputKind::TerminalPick, InputKind::VersionPick, InputKind::MilestonePick] {
            assert!(!k.is_text_input());
            assert_eq!(p(&k, "PAN-1"), Err(InputError::PickOnly));
            assert_eq!(p(&k, ""), Err(InputError::PickOnly));
        }
        assert!(prio().is_text_input() && InputKind::Date.is_text_input() && InputKind::Checkbox.is_text_input());
    }

    #[test]
    fn huge_input_is_refused() {
        let big = "9".repeat(MAX_INPUT_CHARS + 1);
        for k in [InputKind::Text { max_length: None }, InputKind::Labels, InputKind::Date, num(None, None, None, None)] {
            assert_eq!(p(&k, &big), Err(InputError::TooLong { max: MAX_INPUT_CHARS }));
        }
    }

    #[test]
    fn ticket_refs_are_split() {
        assert_eq!(parse_ticket_ref("pan-423"), Some(("PAN".into(), 423)));
        assert_eq!(parse_ticket_ref(" PAN - 7 "), Some(("PAN".into(), 7)));
        assert_eq!(parse_ticket_ref("A1B-1"), Some(("A1B".into(), 1)));
        for bad in ["", "PAN", "PAN-", "PAN-0", "PAN-x", "-5", "P-1", "1AB-2", "PAN-1-2x", "ÄB-1", "TOOLONGKEY1-1"] {
            assert_eq!(parse_ticket_ref(bad), None, "{bad}");
        }
    }

    #[test]
    fn errors_have_both_languages() {
        let all = vec![
            InputError::Empty,
            InputError::PickOnly,
            InputError::TooLong { max: 5 },
            InputError::InvalidChars,
            InputError::NotANumber,
            InputError::OutOfRange { min: Some(0.0), max: Some(10.0) },
            InputError::OutOfRange { min: Some(1.5), max: None },
            InputError::OutOfRange { min: None, max: Some(3.0) },
            InputError::NotADate,
            InputError::NotADateTime,
            InputError::NotYesNo,
            InputError::NoMatch("x".into()),
            InputError::Ambiguous("x".into()),
            InputError::TooManyLabels { max: 50 },
        ];
        for e in all {
            let (de, en) = (e.message(Lang::De), e.message(Lang::En));
            assert!(!de.is_empty() && !en.is_empty());
            if !matches!(e, InputError::Empty | InputError::NotYesNo) {
                assert_ne!(de, en, "{e:?}");
            }
        }
        let r = InputError::OutOfRange { min: Some(0.0), max: Some(10.0) };
        assert!(r.message(Lang::En).contains("0 – 10") && r.message(Lang::De).contains("0 – 10"));
        assert!(InputError::NoMatch("zzz".into()).message(Lang::De).contains("„zzz“"));
        assert!(InputError::NoMatch("zzz".into()).message(Lang::En).contains("“zzz”"));
    }
}
