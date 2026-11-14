//! Eingabe und Anzeige von Feldwerten (Standardwerte, Bedingungen und Setze-Werte der Übergänge) als JSON-Werte –
//! passend zum Feldtyp und seinen Optionen. Dropdown-Werte werden über ihre stabile Kennung gespeichert.

use super::helpers::parse_csv;
use super::widgets;
use crate::planning::fields::{FieldKind, FieldOptions};
use crate::state::Lang;
use eframe::egui::{self, Id, Ui};
use serde_json::{json, Value};

/// Ein frischer Wert für den Feldtyp (Einzelelement).
pub fn blank(kind: FieldKind, o: &FieldOptions) -> Value {
    match kind {
        FieldKind::Checkbox => json!(true),
        FieldKind::Dropdown | FieldKind::Multi => o.values.first().map(|v| json!(v.id)).unwrap_or(Value::Null),
        FieldKind::Number => json!(o.min.unwrap_or(0.0)),
        FieldKind::Date => json!(chrono::Local::now().format("%Y-%m-%d").to_string()),
        FieldKind::Datetime => json!(chrono::Local::now().format("%Y-%m-%dT%H:%M").to_string()),
        _ => json!(""),
    }
}

/// Ein frischer Wert in der „vollen“ Form (Mehrfachauswahl/Labels als leere Liste).
pub fn blank_full(kind: FieldKind, o: &FieldOptions) -> Value {
    match kind {
        FieldKind::Multi | FieldKind::Labels => json!([]),
        _ => blank(kind, o),
    }
}

fn option_name(o: &FieldOptions, id: i64, lang: Lang) -> String {
    o.values.iter().find(|v| v.id == id).map(|v| v.display(lang)).unwrap_or_else(|| format!("#{id}"))
}

/// Kurztext eines Werts (Tabellen, Zusammenfassungen).
pub fn show_value(kind: FieldKind, o: &FieldOptions, v: &Value, lang: Lang) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { lang.t("Ja", "Yes") } else { lang.t("Nein", "No") }.to_string(),
        Value::Number(n) => match kind {
            FieldKind::Dropdown | FieldKind::Multi => option_name(o, n.as_i64().unwrap_or(0), lang),
            _ => {
                let x = n.as_f64().unwrap_or(0.0);
                let dec = o.decimals.unwrap_or(if x.fract() == 0.0 { 0 } else { 2 }) as usize;
                let unit = o.unit.as_deref().filter(|u| !u.is_empty()).map(|u| format!(" {u}")).unwrap_or_default();
                format!("{x:.dec$}{unit}")
            }
        },
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(|x| show_value(kind, o, x, lang)).collect::<Vec<_>>().join(", "),
        Value::Object(_) => String::new(),
    }
}

/// Eingabe eines einzelnen Elements (Text, Datum, Zahl, Auswahl …). `true` bei Änderung.
pub fn element_editor(ui: &mut Ui, salt: Id, kind: FieldKind, o: &FieldOptions, v: &mut Value, lang: Lang) -> bool {
    let mut changed = false;
    match kind {
        FieldKind::Text | FieldKind::Rich | FieldKind::Date | FieldKind::Datetime | FieldKind::Labels => {
            let mut s = v.as_str().unwrap_or("").to_string();
            let hint = match kind {
                FieldKind::Date => lang.t("JJJJ-MM-TT", "YYYY-MM-DD"),
                FieldKind::Datetime => lang.t("JJJJ-MM-TTTHH:MM", "YYYY-MM-DDTHH:MM"),
                _ => "",
            };
            if ui.add(egui::TextEdit::singleline(&mut s).id(salt).desired_width(170.0).hint_text(hint).margin(egui::Margin::symmetric(8, 4))).changed() {
                *v = Value::String(s);
                changed = true;
            }
        }
        FieldKind::Checkbox => {
            let mut b = v.as_bool().unwrap_or(true);
            egui::ComboBox::from_id_salt(salt).selected_text(if b { lang.t("Angehakt", "Checked") } else { lang.t("Nicht angehakt", "Unchecked") }).width(130.0).show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut b, true, lang.t("Angehakt", "Checked")).changed();
                changed |= ui.selectable_value(&mut b, false, lang.t("Nicht angehakt", "Unchecked")).changed();
            });
            if changed {
                *v = json!(b);
            }
        }
        FieldKind::Dropdown | FieldKind::Multi => {
            if o.values.is_empty() {
                ui.label(lang.t("(keine Werte definiert)", "(no values defined)"));
                return false;
            }
            let cur = v.as_i64().unwrap_or(o.values[0].id);
            let mut sel = cur;
            egui::ComboBox::from_id_salt(salt).selected_text(option_name(o, cur, lang)).width(170.0).show_ui(ui, |ui| {
                for val in &o.values {
                    ui.selectable_value(&mut sel, val.id, val.display(lang));
                }
            });
            if sel != cur || v.as_i64().is_none() {
                *v = json!(sel);
                changed = sel != cur;
            }
        }
        FieldKind::Number => {
            let mut x = v.as_f64().unwrap_or(0.0);
            let mut dv = egui::DragValue::new(&mut x).speed(0.1).max_decimals(o.decimals.unwrap_or(2) as usize);
            if let (Some(lo), Some(hi)) = (o.min, o.max) {
                dv = dv.range(lo..=hi);
            } else if let Some(lo) = o.min {
                dv = dv.range(lo..=f64::MAX);
            } else if let Some(hi) = o.max {
                dv = dv.range(f64::MIN..=hi);
            }
            if let Some(u) = o.unit.as_deref().filter(|u| !u.is_empty()) {
                dv = dv.suffix(format!(" {u}"));
            }
            if ui.add(dv).changed() {
                *v = json!(x);
                changed = true;
            }
        }
        _ => {
            ui.label("–");
        }
    }
    changed
}

/// Eingabe einer Liste von Elementen (Mehrfachauswahl, „ist einer von“): Haken je Wert oder kommagetrennter Text.
pub fn list_editor(ui: &mut Ui, salt: Id, kind: FieldKind, o: &FieldOptions, v: &mut Value, lang: Lang) -> bool {
    let mut changed = false;
    match kind {
        FieldKind::Dropdown | FieldKind::Multi => {
            if o.values.is_empty() {
                ui.label(lang.t("(keine Werte definiert)", "(no values defined)"));
                return false;
            }
            let mut ids: Vec<i64> = v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
            ui.vertical(|ui| {
                for val in &o.values {
                    let mut on = ids.contains(&val.id);
                    if widgets::check(ui, &mut on, val.display(lang)).changed() {
                        if on {
                            ids.push(val.id);
                        } else {
                            ids.retain(|i| *i != val.id);
                        }
                        changed = true;
                    }
                }
            });
            if changed {
                let ordered: Vec<Value> = o.values.iter().filter(|x| ids.contains(&x.id)).map(|x| json!(x.id)).collect();
                *v = Value::Array(ordered);
            }
        }
        _ => {
            // kommagetrennter Text mit Puffer, damit ein Komma am Ende beim Tippen stehen bleibt
            let buf_id = salt.with("csv");
            let field_id = salt.with("csvfield");
            let current: Vec<String> = v
                .as_array()
                .map(|a| a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect())
                .unwrap_or_default();
            let focused = ui.memory(|m| m.has_focus(field_id));
            let mut text: String = if focused { ui.data_mut(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| current.join(", ")) } else { current.join(", ") };
            let hint = if kind == FieldKind::Number { lang.t("Zahlen, durch Komma getrennt", "numbers, comma-separated") } else { lang.t("Werte, durch Komma getrennt", "values, comma-separated") };
            if ui.add(egui::TextEdit::singleline(&mut text).id(field_id).desired_width(220.0).hint_text(hint).margin(egui::Margin::symmetric(8, 4))).changed() {
                ui.data_mut(|d| d.insert_temp(buf_id, text.clone()));
                let parts = parse_csv(&text);
                let list: Vec<Value> = if kind == FieldKind::Number { parts.iter().filter_map(|p| p.replace(',', ".").parse::<f64>().ok()).map(|x| json!(x)).collect() } else { parts.into_iter().map(Value::String).collect() };
                *v = Value::Array(list);
                changed = true;
            }
        }
    }
    changed
}

/// Eingabe in der Form, wie der Feldtyp den Wert speichert (Mehrfachauswahl/Labels als Liste, sonst Einzelwert).
pub fn full_editor(ui: &mut Ui, salt: Id, kind: FieldKind, o: &FieldOptions, v: &mut Value, lang: Lang) -> bool {
    match kind {
        FieldKind::Multi | FieldKind::Labels => list_editor(ui, salt, kind, o, v, lang),
        _ => element_editor(ui, salt, kind, o, v, lang),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::OptionValue;

    fn opts() -> FieldOptions {
        FieldOptions {
            values: vec![OptionValue { id: 1, name: Some("Rot".into()), ..Default::default() }, OptionValue { id: 2, key: Some("priority.high".into()), ..Default::default() }],
            min: Some(1.0),
            max: Some(10.0),
            decimals: Some(1),
            unit: Some("h".into()),
            ..Default::default()
        }
    }

    #[test]
    fn blank_values_fit_the_kind() {
        let o = opts();
        assert_eq!(blank(FieldKind::Checkbox, &o), json!(true));
        assert_eq!(blank(FieldKind::Dropdown, &o), json!(1));
        assert_eq!(blank(FieldKind::Text, &o), json!(""));
        assert_eq!(blank_full(FieldKind::Multi, &o), json!([]));
        assert_eq!(blank_full(FieldKind::Labels, &o), json!([]));
        assert!(blank(FieldKind::Date, &o).as_str().is_some_and(|s| s.len() == 10));
        assert_eq!(blank(FieldKind::Dropdown, &FieldOptions::default()), Value::Null);
        let n = blank(FieldKind::Number, &o).as_f64().unwrap();
        assert!(n >= 0.0, "Zahl-Startwert");
    }

    #[test]
    fn shows_values_readably() {
        let o = opts();
        assert_eq!(show_value(FieldKind::Dropdown, &o, &json!(1), Lang::En), "Rot");
        assert_eq!(show_value(FieldKind::Dropdown, &o, &json!(2), Lang::En), "High");
        assert_eq!(show_value(FieldKind::Dropdown, &o, &json!(2), Lang::De), "Hoch");
        assert_eq!(show_value(FieldKind::Multi, &o, &json!([1, 2]), Lang::En), "Rot, High");
        assert_eq!(show_value(FieldKind::Number, &o, &json!(2.5), Lang::En), "2.5 h");
        assert_eq!(show_value(FieldKind::Checkbox, &o, &json!(true), Lang::De), "Ja");
        assert_eq!(show_value(FieldKind::Checkbox, &o, &json!(false), Lang::En), "No");
        assert_eq!(show_value(FieldKind::Text, &o, &Value::Null, Lang::En), "");
        assert_eq!(show_value(FieldKind::Dropdown, &o, &json!(9), Lang::En), "#9", "unbekannte Kennung");
    }
}
