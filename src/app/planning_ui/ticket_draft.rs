//! Eingabepuffer eines Feldwerts für die Ticket-Oberfläche (reine Logik ohne egui-Zeichnen, gut testbar).
//!
//! Ein [`Draft`] hält, was der Nutzer gerade tippt (Text, Zahl, Datum als Text) bzw. die schon gewählten Werte (Auswahl,
//! Haken, Verweise). [`Draft::to_value`] macht daraus den JSON-Wert, den die Datenschicht erwartet (`null` = leer); die
//! eigentliche Prüfung gegen Optionen und Grenzen macht `values::normalize_value`. Zahl- und Datumseingaben, die gar nicht
//! lesbar sind, melden schon hier einen zweisprachigen Fehler.

use crate::markdown::FieldMode;
use crate::planning::fields::{Field, FieldKind};
use crate::planning::values::canon_number;
use crate::state::Lang;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Draft {
    /// Tipptext: Text, Rich-Text, Zahl, Datum(+Uhrzeit), Terminal; bei Labels/Tickets der Suchtext.
    pub text: String,
    /// Gewählter Wert: Haken, Auswahl, Liste (Mehrfachauswahl/Labels), Verweis.
    pub value: Value,
    /// Darstellung des Rich-Text-Felds.
    pub mode: FieldMode,
    /// Der Nutzer hat seit dem letzten Speichern etwas verändert.
    pub edited: bool,
    /// Meldung zum Feld (Fehler beim Speichern).
    pub err: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftError {
    NotANumber,
    BadDate,
    BadDatetime,
}

impl DraftError {
    pub fn message(self, lang: Lang) -> String {
        match self {
            DraftError::NotANumber => lang.t("Das ist keine Zahl.", "That is not a number.").to_string(),
            DraftError::BadDate => lang.t("Datum nicht lesbar – so geht es: 2026-11-14 oder 14.11.2026.", "Date not readable – use 2026-11-14 or 14.11.2026.").to_string(),
            DraftError::BadDatetime => lang.t("Datum/Uhrzeit nicht lesbar – so geht es: 2026-11-14 09:30.", "Date/time not readable – use 2026-11-14 09:30.").to_string(),
        }
    }
}

/// Zahl als Text ohne überflüssige Nachkommastellen (`5`, `2.5`).
pub fn number_text(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1.0e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// Zahl lesen: Punkt oder Komma als Dezimaltrenner; leer = `Ok(None)`.
pub fn parse_number(s: &str) -> Result<Option<f64>, DraftError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.replace(',', ".").parse::<f64>().ok().filter(|x| x.is_finite()).map(Some).ok_or(DraftError::NotANumber)
}

/// Datum lesen (`JJJJ-MM-TT`, `TT.MM.JJJJ`, `JJJJ/MM/TT`).
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let t = s.trim();
    ["%Y-%m-%d", "%d.%m.%Y", "%Y/%m/%d"].iter().find_map(|f| NaiveDate::parse_from_str(t, f).ok())
}

/// Datum mit Uhrzeit lesen (`JJJJ-MM-TT HH:MM[:SS]`, auch mit `T`; deutsches Datum erlaubt).
pub fn parse_datetime(s: &str) -> Option<NaiveDateTime> {
    let t = s.trim().replacen('T', " ", 1);
    let (d, tm) = t.split_once(' ')?;
    let date = parse_date(d)?;
    let tm = tm.trim();
    let time = ["%H:%M:%S", "%H:%M"].iter().find_map(|f| NaiveTime::parse_from_str(tm, f).ok())?;
    Some(NaiveDateTime::new(date, time))
}

/// Anzeigetext eines gespeicherten Datums mit Uhrzeit (`2026-11-14T10:00:00` → `2026-11-14 10:00`).
pub fn datetime_text(stored: &str) -> String {
    parse_datetime(stored).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| stored.to_string())
}

impl Draft {
    pub fn empty() -> Draft {
        Draft { text: String::new(), value: Value::Null, mode: FieldMode::Text, edited: false, err: None }
    }

    /// Puffer aus dem gespeicherten Wert (`None` = leer).
    pub fn from_value(f: &Field, v: Option<&Value>) -> Draft {
        let mut d = Draft::empty();
        match f.kind {
            FieldKind::Text | FieldKind::Rich | FieldKind::Terminal => d.text = v.and_then(Value::as_str).unwrap_or("").to_string(),
            FieldKind::Date => d.text = v.and_then(Value::as_str).unwrap_or("").to_string(),
            FieldKind::Datetime => d.text = v.and_then(Value::as_str).map(datetime_text).unwrap_or_default(),
            FieldKind::Number => d.text = v.and_then(Value::as_f64).map(number_text).unwrap_or_default(),
            FieldKind::Checkbox => d.value = Value::Bool(v.and_then(Value::as_bool).unwrap_or(false)),
            FieldKind::Dropdown | FieldKind::Ticket | FieldKind::Version | FieldKind::Milestone => d.value = v.cloned().unwrap_or(Value::Null),
            FieldKind::Multi | FieldKind::Labels => d.value = v.filter(|x| x.is_array()).cloned().unwrap_or_else(|| json!([])),
        }
        if f.kind == FieldKind::Rich {
            d.mode = if d.text.trim().is_empty() { FieldMode::Text } else { FieldMode::Visual };
        }
        d
    }

    /// Der Wert für die Datenschicht; `Value::Null` heißt „leer“.
    pub fn to_value(&self, f: &Field) -> Result<Value, DraftError> {
        Ok(match f.kind {
            FieldKind::Text | FieldKind::Rich | FieldKind::Terminal => Value::String(self.text.clone()),
            FieldKind::Number => match parse_number(&self.text)? {
                Some(x) => canon_number(x),
                None => Value::Null,
            },
            FieldKind::Date => {
                if self.text.trim().is_empty() {
                    Value::Null
                } else {
                    Value::String(parse_date(&self.text).ok_or(DraftError::BadDate)?.format("%Y-%m-%d").to_string())
                }
            }
            FieldKind::Datetime => {
                if self.text.trim().is_empty() {
                    Value::Null
                } else {
                    Value::String(parse_datetime(&self.text).ok_or(DraftError::BadDatetime)?.format("%Y-%m-%dT%H:%M").to_string())
                }
            }
            FieldKind::Checkbox => Value::Bool(self.value.as_bool().unwrap_or(false)),
            FieldKind::Labels => {
                // Ein noch nicht mit Enter bestätigtes Label zählt mit.
                let mut list: Vec<Value> = self.value.as_array().cloned().unwrap_or_default();
                let pending = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !pending.is_empty() && !list.iter().any(|x| x.as_str().is_some_and(|s| s.eq_ignore_ascii_case(&pending))) {
                    list.push(Value::String(pending));
                }
                Value::Array(list)
            }
            _ => self.value.clone(),
        })
    }

    /// Fügt ein Label (bereinigt, ohne Doppelte) hinzu und leert den Suchtext.
    pub fn add_label(&mut self, label: &str) {
        let l = label.split_whitespace().collect::<Vec<_>>().join(" ");
        self.text.clear();
        if l.is_empty() {
            return;
        }
        let mut list = self.value.as_array().cloned().unwrap_or_default();
        if !list.iter().any(|x| x.as_str().is_some_and(|s| s.to_lowercase() == l.to_lowercase())) {
            list.push(Value::String(l));
        }
        self.value = Value::Array(list);
        self.edited = true;
    }

    pub fn remove_label(&mut self, label: &str) {
        let mut list = self.value.as_array().cloned().unwrap_or_default();
        list.retain(|x| x.as_str() != Some(label));
        self.value = Value::Array(list);
        self.edited = true;
    }

    /// Schaltet einen Wert der Mehrfachauswahl um (Reihenfolge wie in `order`).
    pub fn toggle_option(&mut self, id: i64, order: &[i64]) {
        let mut ids: Vec<i64> = self.value.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
        if ids.contains(&id) {
            ids.retain(|i| *i != id);
        } else {
            ids.push(id);
        }
        self.value = Value::Array(order.iter().filter(|o| ids.contains(o)).map(|o| json!(o)).collect());
        self.edited = true;
    }
}

/// Felder nach Abschnitten gruppiert, in der Reihenfolge des ersten Auftretens; der Abschnitt „ohne Namen“ steht vorn.
pub fn group_sections(fields: &[Field]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let name = f.section.trim().to_string();
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => v.push(i),
            None => out.push((name, vec![i])),
        }
    }
    if let Some(p) = out.iter().position(|(n, _)| n.is_empty()) {
        let g = out.remove(p);
        out.insert(0, g);
    }
    out
}

/// Kurzer Hinweis zu den Grenzen eines Feldes (Zahlbereich, Textlänge, Format), leer wenn es nichts zu sagen gibt.
pub fn field_hint(f: &Field, lang: Lang) -> String {
    let o = &f.options;
    match f.kind {
        FieldKind::Number => {
            let unit = o.unit.as_deref().filter(|u| !u.is_empty()).map(|u| format!(" {u}")).unwrap_or_default();
            match (o.min, o.max) {
                (Some(a), Some(b)) => format!("{}{unit} – {}{unit}", number_text(a), number_text(b)),
                (Some(a), None) => format!("≥ {}{unit}", number_text(a)),
                (None, Some(b)) => format!("≤ {}{unit}", number_text(b)),
                (None, None) => unit.trim().to_string(),
            }
        }
        FieldKind::Text => o.max_length.map(|m| format!("max. {m} {}", lang.t("Zeichen", "characters"))).unwrap_or_default(),
        FieldKind::Date => lang.t("JJJJ-MM-TT", "YYYY-MM-DD").to_string(),
        FieldKind::Datetime => lang.t("JJJJ-MM-TT HH:MM", "YYYY-MM-DD HH:MM").to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::FieldOptions;

    fn field(kind: FieldKind) -> Field {
        Field {
            id: 1,
            type_id: 1,
            key: None,
            name: Some("F".into()),
            kind,
            options: FieldOptions::default(),
            required: false,
            default_value: None,
            help: String::new(),
            section: String::new(),
            position: 0,
            show_on_card: false,
            builtin: false,
        }
    }

    #[test]
    fn numbers_parse_with_point_or_comma() {
        assert_eq!(parse_number("5"), Ok(Some(5.0)));
        assert_eq!(parse_number(" 2,5 "), Ok(Some(2.5)));
        assert_eq!(parse_number("2.5"), Ok(Some(2.5)));
        assert_eq!(parse_number(""), Ok(None));
        assert_eq!(parse_number("abc"), Err(DraftError::NotANumber));
        assert_eq!(parse_number("inf"), Err(DraftError::NotANumber));
        assert_eq!(number_text(5.0), "5");
        assert_eq!(number_text(2.5), "2.5");
        assert_eq!(number_text(-3.0), "-3");
    }

    #[test]
    fn dates_accept_iso_and_german_notation() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 14).unwrap();
        assert_eq!(parse_date("2026-11-14"), Some(d));
        assert_eq!(parse_date("14.11.2026"), Some(d));
        assert_eq!(parse_date("2026/11/14"), Some(d));
        assert_eq!(parse_date("2026-13-40"), None);
        assert_eq!(parse_date(""), None);
        let dt = parse_datetime("2026-11-14T09:30:15").unwrap();
        assert_eq!(dt.format("%H:%M:%S").to_string(), "09:30:15");
        assert!(parse_datetime("2026-11-14 09:30").is_some());
        assert!(parse_datetime("14.11.2026 9:30").is_some());
        assert!(parse_datetime("2026-11-14").is_none(), "ohne Uhrzeit ist es kein Datum+Uhrzeit");
        assert_eq!(datetime_text("2026-11-14T10:00:00"), "2026-11-14 10:00");
    }

    #[test]
    fn drafts_roundtrip_through_values() {
        let n = field(FieldKind::Number);
        let d = Draft::from_value(&n, Some(&json!(8)));
        assert_eq!(d.text, "8");
        assert_eq!(d.to_value(&n), Ok(json!(8)));
        let mut d = Draft::from_value(&n, None);
        assert_eq!(d.to_value(&n), Ok(Value::Null));
        d.text = "x".into();
        assert_eq!(d.to_value(&n), Err(DraftError::NotANumber));
        let date = field(FieldKind::Date);
        let mut d = Draft::from_value(&date, Some(&json!("2026-11-14")));
        assert_eq!(d.to_value(&date), Ok(json!("2026-11-14")));
        d.text = "14.11.2026".into();
        assert_eq!(d.to_value(&date), Ok(json!("2026-11-14")), "deutsche Schreibweise wird vereinheitlicht");
        d.text = "morgen".into();
        assert_eq!(d.to_value(&date), Err(DraftError::BadDate));
        let dt = field(FieldKind::Datetime);
        let d = Draft::from_value(&dt, Some(&json!("2026-11-14T10:00:00")));
        assert_eq!(d.text, "2026-11-14 10:00");
        assert_eq!(d.to_value(&dt), Ok(json!("2026-11-14T10:00")));
        let c = field(FieldKind::Checkbox);
        assert_eq!(Draft::from_value(&c, None).to_value(&c), Ok(json!(false)));
        assert_eq!(Draft::from_value(&c, Some(&json!(true))).to_value(&c), Ok(json!(true)));
    }

    #[test]
    fn rich_fields_start_visual_when_filled() {
        let r = field(FieldKind::Rich);
        assert_eq!(Draft::from_value(&r, Some(&json!("hallo"))).mode, FieldMode::Visual);
        assert_eq!(Draft::from_value(&r, None).mode, FieldMode::Text);
    }

    #[test]
    fn labels_add_dedupe_and_include_pending_text() {
        let l = field(FieldKind::Labels);
        let mut d = Draft::from_value(&l, None);
        d.add_label("  Backend  ");
        d.add_label("backend");
        d.add_label("UI");
        assert_eq!(d.value, json!(["Backend", "UI"]));
        d.text = "neu".into();
        assert_eq!(d.to_value(&l), Ok(json!(["Backend", "UI", "neu"])), "ungetipptes Label zählt mit");
        d.remove_label("UI");
        assert_eq!(d.value, json!(["Backend"]));
        d.add_label("   ");
        assert!(d.text.is_empty());
    }

    #[test]
    fn multi_toggle_keeps_option_order() {
        let m = field(FieldKind::Multi);
        let mut d = Draft::from_value(&m, None);
        d.toggle_option(3, &[1, 2, 3]);
        d.toggle_option(1, &[1, 2, 3]);
        assert_eq!(d.value, json!([1, 3]));
        d.toggle_option(3, &[1, 2, 3]);
        assert_eq!(d.value, json!([1]));
    }

    #[test]
    fn sections_group_in_order_with_unnamed_first() {
        let mut fs: Vec<Field> = (0..5).map(|_| field(FieldKind::Text)).collect();
        for (f, s) in fs.iter_mut().zip(["Plan", "", "Plan", "Test", ""]) {
            f.section = s.into();
        }
        let g = group_sections(&fs);
        assert_eq!(g, vec![("".to_string(), vec![1, 4]), ("Plan".to_string(), vec![0, 2]), ("Test".to_string(), vec![3])]);
        assert!(group_sections(&[]).is_empty());
    }

    #[test]
    fn hints_name_the_limits() {
        let mut n = field(FieldKind::Number);
        n.options.min = Some(0.0);
        n.options.max = Some(100.0);
        n.options.unit = Some("h".into());
        assert_eq!(field_hint(&n, Lang::En), "0 h – 100 h");
        n.options.max = None;
        assert_eq!(field_hint(&n, Lang::En), "≥ 0 h");
        assert_ne!(field_hint(&field(FieldKind::Date), Lang::De), field_hint(&field(FieldKind::Date), Lang::En));
        assert!(DraftError::NotANumber.message(Lang::De) != DraftError::NotANumber.message(Lang::En));
    }
}
