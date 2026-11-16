//! Feldwerte der Tickets: Ablage in `ticket_values` (typisiert für SQL/JQL), Prüfen und Vereinheitlichen eingegebener Werte.
//!
//! In der Schicht werden Werte als JSON ([`serde_json::Value`]) weitergereicht – dieselbe Darstellung wie bei Standardwerten
//! und Übergangsregeln (`fields.rs`): Text/Rich/Terminal = String, Checkbox = Bool, Zahl = Zahl, Dropdown = Kennung des Werts,
//! Mehrfachauswahl = Liste von Kennungen, Labels = Liste von Strings, Datum = `JJJJ-MM-TT`, Datum+Uhrzeit =
//! `JJJJ-MM-TTTHH:MM:SS`, Ticket/Version/Meilenstein = Kennung. „Leer“ (null, leerer Text, leere Liste) ist kein Wert: Es
//! gibt dann keine Zeile.
//!
//! Ablage je Zeile (`text`, `num`): siehe `ticket_schema.rs`. Zahlen werden vereinheitlicht (ganze Zahlen als Ganzzahl), damit
//! gleiche Werte auch als JSON gleich sind.

use super::fields::{check_value, list_fields, Field, FieldKind};
use super::sprints::releases::{exists_in_project, ReleaseKind};
use super::ticket_error::{FieldLabel, TRes, TicketError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

/// Eine Zeile in `ticket_values`: (`text`, `num`).
pub(crate) type Row = (Option<String>, Option<f64>);

/// Ein Feld des Typs mit dem aktuellen Wert des Tickets (`None` = leer).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldValue {
    pub field: Field,
    pub value: Option<Value>,
}

/// Kontext für die Prüfung von Verweisen: das Projekt des Tickets und – falls es schon existiert – das Ticket selbst.
#[derive(Debug, Clone, Copy)]
pub struct RefCtx {
    pub project_id: i64,
    pub ticket_id: Option<i64>,
}

/// Zahl in einheitliche JSON-Darstellung bringen (ganze Zahlen ohne Nachkomma).
pub fn canon_number(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 9.0e15 {
        Value::from(x as i64)
    } else {
        serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
    }
}

fn canon(v: &Value) -> Value {
    match v {
        Value::Number(n) => n.as_f64().map(canon_number).unwrap_or_else(|| v.clone()),
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        other => other.clone(),
    }
}

// ------------------------------------------------------------------ Zeilen ↔ Wert

pub(crate) fn encode(kind: FieldKind, v: &Value) -> Vec<Row> {
    let s = |v: &Value| Some(v.as_str().unwrap_or("").to_string());
    match kind {
        FieldKind::Text | FieldKind::Rich | FieldKind::Date | FieldKind::Datetime | FieldKind::Terminal => vec![(s(v), None)],
        FieldKind::Checkbox => vec![(None, Some(if v.as_bool() == Some(true) { 1.0 } else { 0.0 }))],
        FieldKind::Number | FieldKind::Dropdown => vec![(None, v.as_f64())],
        FieldKind::Multi => v.as_array().map(|a| a.iter().map(|x| (None, x.as_f64())).collect()).unwrap_or_default(),
        FieldKind::Labels => v.as_array().map(|a| a.iter().map(|x| (s(x), None)).collect()).unwrap_or_default(),
        FieldKind::Ticket | FieldKind::Version | FieldKind::Milestone => vec![(Some(v.as_i64().unwrap_or(0).to_string()), None)],
    }
}

pub(crate) fn decode(kind: FieldKind, rows: &[Row]) -> Option<Value> {
    if rows.is_empty() {
        return None;
    }
    let text = |r: &Row| r.0.clone().unwrap_or_default();
    let num = |r: &Row| r.1.unwrap_or(0.0);
    Some(match kind {
        FieldKind::Text | FieldKind::Rich | FieldKind::Date | FieldKind::Datetime | FieldKind::Terminal => Value::String(text(&rows[0])),
        FieldKind::Checkbox => Value::Bool(num(&rows[0]) != 0.0),
        FieldKind::Number | FieldKind::Dropdown => canon_number(num(&rows[0])),
        FieldKind::Multi => Value::Array(rows.iter().map(|r| canon_number(num(r))).collect()),
        FieldKind::Labels => Value::Array(rows.iter().map(|r| Value::String(text(r))).collect()),
        FieldKind::Ticket | FieldKind::Version | FieldKind::Milestone => canon_number(text(&rows[0]).parse::<i64>().unwrap_or(0) as f64),
    })
}

// ------------------------------------------------------------------ Lesen und Schreiben

/// Alle gesetzten Werte eines Tickets nach Feld-Id.
pub fn load_values(conn: &Connection, ticket_id: i64) -> TRes<BTreeMap<i64, Value>> {
    let mut st = conn.prepare(
        "SELECT v.field_id, f.kind, v.text, v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE v.ticket_id = ?1 ORDER BY v.field_id, v.pos",
    )?;
    let rows = st
        .query_map([ticket_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<f64>>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut grouped: BTreeMap<i64, (FieldKind, Vec<Row>)> = BTreeMap::new();
    for (fid, kind, text, num) in rows {
        let kind = FieldKind::parse(&kind).unwrap_or(FieldKind::Text);
        grouped.entry(fid).or_insert_with(|| (kind, Vec::new())).1.push((text, num));
    }
    Ok(grouped.into_iter().filter_map(|(fid, (kind, rows))| decode(kind, &rows).map(|v| (fid, v))).collect())
}

/// Wert eines Feldes (`None` = leer).
pub fn get_value(conn: &Connection, ticket_id: i64, field: &Field) -> TRes<Option<Value>> {
    let mut st = conn.prepare("SELECT text, num FROM ticket_values WHERE ticket_id = ?1 AND field_id = ?2 ORDER BY pos")?;
    let rows = st.query_map([ticket_id, field.id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<f64>>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(decode(field.kind, &rows))
}

/// Alle Felder des Typs mit den Werten des Tickets, in der Reihenfolge der Felder.
pub fn fields_with_values(conn: &Connection, ticket_id: i64, type_id: i64) -> TRes<Vec<FieldValue>> {
    let mut values = load_values(conn, ticket_id)?;
    Ok(list_fields(conn, type_id)?.into_iter().map(|f| FieldValue { value: values.remove(&f.id), field: f }).collect())
}

/// Schreibt den Wert (ersetzt alle Zeilen des Feldes); `None` löscht. Der Wert muss schon geprüft sein.
pub(crate) fn write_value(conn: &Connection, ticket_id: i64, field: &Field, v: Option<&Value>) -> TRes<()> {
    conn.execute("DELETE FROM ticket_values WHERE ticket_id = ?1 AND field_id = ?2", [ticket_id, field.id])?;
    if let Some(v) = v {
        for (pos, (text, num)) in encode(field.kind, v).into_iter().enumerate() {
            conn.execute("INSERT INTO ticket_values (ticket_id, field_id, pos, text, num) VALUES (?1, ?2, ?3, ?4, ?5)", params![ticket_id, field.id, pos as i64, text, num])?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ Prüfen

/// Ist der Wert „leer“ im Sinne von Pflichtfeldern?
pub fn is_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    }
}

fn normalize_datetime(s: &str) -> String {
    if s.len() == 16 { format!("{s}:00") } else { s.to_string() }
}

/// Prüft einen eingegebenen Wert für das Feld und bringt ihn in die gespeicherte Form. `Ok(None)` heißt „leer“ (das Feld wird
/// geleert). Fehler nennen das Feld: `ValueInvalid` (passt nicht zum Feldtyp/zu den Optionen) bzw. `RefInvalid` (Verweis).
pub fn normalize_value(conn: &Connection, ctx: &RefCtx, field: &Field, v: &Value) -> TRes<Option<Value>> {
    let bad = || TicketError::ValueInvalid(FieldLabel::of(field));
    let mut v = v.clone();
    match &v {
        Value::Null => return Ok(None),
        Value::String(s) if s.trim().is_empty() => return Ok(None),
        Value::Array(a) if a.is_empty() => return Ok(None),
        _ => {}
    }
    match field.kind {
        FieldKind::Labels => {
            if let Value::Array(items) = &v {
                let mut out: Vec<Value> = Vec::new();
                for it in items {
                    let Some(s) = it.as_str() else { return Err(bad()) };
                    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
                    if t.is_empty() || out.iter().any(|o| o.as_str().is_some_and(|x| x.to_lowercase() == t.to_lowercase())) {
                        continue;
                    }
                    out.push(Value::String(t));
                }
                if out.is_empty() {
                    return Ok(None);
                }
                v = Value::Array(out);
            }
        }
        FieldKind::Datetime => {
            if let Some(s) = v.as_str() {
                // Uhrzeit optional (Fälligkeit): auch „2026-11-20 14:30“ und „20.11.2026“ verstehen
                let s = match (field.options.date_only_ok(), crate::due::parse_input(s)) {
                    (true, Ok(Some(n))) => n,
                    _ => s.to_string(),
                };
                v = Value::String(normalize_datetime(&s));
            }
        }
        FieldKind::Terminal => {
            if let Some(s) = v.as_str() {
                let t = s.trim();
                if t.chars().count() > 200 {
                    return Err(bad());
                }
                v = Value::String(t.to_string());
            }
        }
        _ => {}
    }
    check_value(field.kind, &field.options, &v, true).map_err(|_| bad())?;
    match field.kind {
        FieldKind::Ticket => {
            let id = v.as_i64().ok_or_else(bad)?;
            let target: Option<(i64, i64)> = conn.query_row("SELECT project_id, type_id FROM tickets WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            let ok = match target {
                Some((project, ty)) => project == ctx.project_id && Some(id) != ctx.ticket_id && (field.options.target_types.is_empty() || field.options.target_types.contains(&ty)),
                None => false,
            };
            if !ok {
                return Err(TicketError::RefInvalid(FieldLabel::of(field)));
            }
        }
        FieldKind::Version | FieldKind::Milestone => {
            let id = v.as_i64().filter(|i| *i > 0).ok_or_else(bad)?;
            // Die Kennung muss zu einer Version/einem Meilenstein desselben Projekts gehören.
            let kind = if field.kind == FieldKind::Version { ReleaseKind::Version } else { ReleaseKind::Milestone };
            if !exists_in_project(conn, kind, id, ctx.project_id)? {
                return Err(TicketError::RefInvalid(FieldLabel::of(field)));
            }
        }
        _ => {}
    }
    Ok(Some(canon(&v)))
}

/// Wert aus dem Feld `from` für das Feld `to` desselben Feldtyps übersetzen: Dropdown-/Mehrfachauswahl-Werte werden über
/// ihre Bezeichnung (Schlüssel oder Name, in beiden Sprachen) zugeordnet, alles andere bleibt gleich. `None` = ohne Entsprechung.
pub fn translate_value(from: &Field, to: &Field, v: &Value) -> Option<Value> {
    let map_one = |id: i64| -> Option<Value> {
        let src = from.options.values.iter().find(|o| o.id == id)?;
        let dst = to.options.values.iter().find(|o| super::common::same_label(src.key.as_deref(), src.name.as_deref(), o.key.as_deref(), o.name.as_deref()))?;
        Some(Value::from(dst.id))
    };
    match from.kind {
        FieldKind::Dropdown => map_one(v.as_i64()?),
        FieldKind::Multi => {
            let list: Vec<Value> = v.as_array()?.iter().filter_map(|x| x.as_i64().and_then(map_one)).collect();
            if list.is_empty() { None } else { Some(Value::Array(list)) }
        }
        _ => Some(v.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{create_field, get_field, FieldOptions, NewField, OptionValue};
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::testutil::db;
    use crate::planning::types::{create_type, NewType};
    use serde_json::json;

    fn setup(c: &Connection) -> (i64, i64) {
        let p = create_project(c, &NewProject::new("AA", "P", "#112233")).unwrap();
        let t = create_type(c, &NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        (p, t)
    }

    fn field(c: &Connection, t: i64, name: &str, kind: FieldKind, o: FieldOptions) -> Field {
        let mut n = NewField::new(t, name, kind);
        n.options = o;
        get_field(c, create_field(c, &n).unwrap()).unwrap()
    }

    fn dd(names: &[&str]) -> FieldOptions {
        FieldOptions { values: names.iter().map(|n| OptionValue::new(n)).collect(), ..Default::default() }
    }

    #[test]
    fn roundtrip_for_every_kind() {
        let c = db();
        let (p, t) = setup(&c);
        let ctx = RefCtx { project_id: p, ticket_id: None };
        // Versionen und Meilensteine müssen zum Projekt gehören (Kennung 1 jeweils).
        c.execute("INSERT INTO versions (project_id, name) VALUES (?1, 'v')", [p]).unwrap();
        c.execute("INSERT INTO milestones (project_id, name) VALUES (?1, 'm')", [p]).unwrap();
        let cases: Vec<(FieldKind, FieldOptions, Value)> = vec![
            (FieldKind::Text, FieldOptions::default(), json!("hallo")),
            (FieldKind::Rich, FieldOptions::default(), json!("# Titel\n\ntext")),
            (FieldKind::Checkbox, FieldOptions::default(), json!(true)),
            (FieldKind::Checkbox, FieldOptions::default(), json!(false)),
            (FieldKind::Dropdown, dd(&["a", "b"]), json!(2)),
            (FieldKind::Number, FieldOptions::default(), json!(3.5)),
            (FieldKind::Number, FieldOptions::default(), json!(7)),
            (FieldKind::Terminal, FieldOptions::default(), json!("term-1")),
            (FieldKind::Version, FieldOptions::default(), json!(1)),
            (FieldKind::Milestone, FieldOptions::default(), json!(1)),
            (FieldKind::Date, FieldOptions::default(), json!("2026-11-14")),
            (FieldKind::Datetime, FieldOptions::default(), json!("2026-11-14T09:30:15")),
            (FieldKind::Multi, dd(&["a", "b", "c"]), json!([3, 1])),
            (FieldKind::Labels, FieldOptions::default(), json!(["x", "y z"])),
        ];
        for (i, (kind, o, v)) in cases.into_iter().enumerate() {
            let f = field(&c, t, &format!("F{i}"), kind, o);
            let ticket = i as i64 + 100;
            c.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
            let n = normalize_value(&c, &ctx, &f, &v).unwrap().unwrap();
            assert_eq!(n, v, "{kind:?} normalisiert unverändert");
            write_value(&c, ticket, &f, Some(&n)).unwrap();
            assert_eq!(get_value(&c, ticket, &f).unwrap(), Some(v.clone()), "{kind:?}");
            assert_eq!(load_values(&c, ticket).unwrap().get(&f.id), Some(&v), "{kind:?}");
            write_value(&c, ticket, &f, None).unwrap();
            assert_eq!(get_value(&c, ticket, &f).unwrap(), None);
        }
    }

    #[test]
    fn multi_values_use_one_row_per_value() {
        let c = db();
        let (_, t) = setup(&c);
        let f = field(&c, t, "M", FieldKind::Multi, dd(&["a", "b", "c"]));
        c.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        write_value(&c, 1, &f, Some(&json!([2, 3]))).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM ticket_values WHERE ticket_id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        write_value(&c, 1, &f, Some(&json!([1]))).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM ticket_values WHERE ticket_id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "alte Zeilen werden ersetzt");
    }

    #[test]
    fn empty_values_mean_no_value() {
        let c = db();
        let (p, t) = setup(&c);
        let ctx = RefCtx { project_id: p, ticket_id: None };
        let f = field(&c, t, "T", FieldKind::Text, FieldOptions::default());
        for v in [json!(null), json!(""), json!("   "), json!([])] {
            assert_eq!(normalize_value(&c, &ctx, &f, &v).unwrap(), None, "{v}");
        }
        assert!(is_empty(None) && is_empty(Some(&json!(" "))) && is_empty(Some(&json!([]))));
        assert!(!is_empty(Some(&json!(false))) && !is_empty(Some(&json!(0))));
        // ein Text behält seine Form (kein Zuschneiden)
        assert_eq!(normalize_value(&c, &ctx, &f, &json!(" a ")).unwrap(), Some(json!(" a ")));
    }

    #[test]
    fn rejects_values_that_do_not_fit() {
        let c = db();
        let (p, t) = setup(&c);
        let ctx = RefCtx { project_id: p, ticket_id: None };
        let num = field(&c, t, "N", FieldKind::Number, FieldOptions { min: Some(0.0), max: Some(10.0), ..Default::default() });
        let drop = field(&c, t, "D", FieldKind::Dropdown, dd(&["a"]));
        let chk = field(&c, t, "C", FieldKind::Checkbox, FieldOptions::default());
        let date = field(&c, t, "Da", FieldKind::Date, FieldOptions::default());
        let txt = field(&c, t, "Tx", FieldKind::Text, FieldOptions { max_length: Some(3), ..Default::default() });
        for (f, v) in [
            (&num, json!(11)),
            (&num, json!(-1)),
            (&num, json!("x")),
            (&drop, json!(99)),
            (&drop, json!("a")),
            (&chk, json!("true")),
            (&date, json!("14.11.2026")),
            (&txt, json!("abcd")),
        ] {
            match normalize_value(&c, &ctx, f, &v) {
                Err(TicketError::ValueInvalid(l)) => assert_eq!(l.id, f.id),
                other => panic!("{v}: {other:?}"),
            }
        }
        assert_eq!(normalize_value(&c, &ctx, &num, &json!(10)).unwrap(), Some(json!(10)));
        assert_eq!(normalize_value(&c, &ctx, &num, &json!(2.0)).unwrap(), Some(json!(2)), "ganze Zahlen werden vereinheitlicht");
    }

    #[test]
    fn datetime_gets_seconds_and_labels_are_cleaned() {
        let c = db();
        let (p, t) = setup(&c);
        let ctx = RefCtx { project_id: p, ticket_id: None };
        let dt = field(&c, t, "Dt", FieldKind::Datetime, FieldOptions::default());
        assert_eq!(normalize_value(&c, &ctx, &dt, &json!("2026-11-14T09:30")).unwrap(), Some(json!("2026-11-14T09:30:00")));
        let lb = field(&c, t, "L", FieldKind::Labels, FieldOptions::default());
        assert_eq!(normalize_value(&c, &ctx, &lb, &json!([" a ", "A", "b  c", ""])).unwrap(), Some(json!(["a", "b c"])));
        assert_eq!(normalize_value(&c, &ctx, &lb, &json!(["", " "])).unwrap(), None);
        assert!(normalize_value(&c, &ctx, &lb, &json!([1])).is_err());
    }

    #[test]
    fn terminal_values_are_trimmed_and_limited() {
        let c = db();
        let (p, t) = setup(&c);
        let ctx = RefCtx { project_id: p, ticket_id: None };
        let f = field(&c, t, "Term", FieldKind::Terminal, FieldOptions::default());
        assert_eq!(normalize_value(&c, &ctx, &f, &json!(" s1 ")).unwrap(), Some(json!("s1")));
        assert!(normalize_value(&c, &ctx, &f, &json!("x".repeat(201))).is_err());
        assert!(normalize_value(&c, &ctx, &f, &json!(5)).is_err());
        let v = field(&c, t, "V", FieldKind::Version, FieldOptions::default());
        assert!(normalize_value(&c, &ctx, &v, &json!(0)).is_err());
        assert!(normalize_value(&c, &ctx, &v, &json!(3)).is_err(), "es gibt keine Version 3");
        c.execute("INSERT INTO versions (project_id, name) VALUES (?1, 'v')", [p]).unwrap();
        assert!(normalize_value(&c, &ctx, &v, &json!(1)).is_ok());
    }

    #[test]
    fn translates_dropdown_values_by_label() {
        let c = db();
        let (_, t) = setup(&c);
        let a = field(&c, t, "A", FieldKind::Dropdown, dd(&["Rot", "Grün", "Blau"]));
        let b = field(&c, t, "B", FieldKind::Dropdown, dd(&["blau", "Rot"]));
        assert_eq!(translate_value(&a, &b, &json!(1)), Some(json!(2)), "Rot");
        assert_eq!(translate_value(&a, &b, &json!(3)), Some(json!(1)), "Blau, Schreibweise egal");
        assert_eq!(translate_value(&a, &b, &json!(2)), None, "Grün gibt es dort nicht");
        let m1 = field(&c, t, "M1", FieldKind::Multi, dd(&["x", "y"]));
        let m2 = field(&c, t, "M2", FieldKind::Multi, dd(&["y", "z"]));
        assert_eq!(translate_value(&m1, &m2, &json!([1, 2])), Some(json!([1])));
        assert_eq!(translate_value(&m1, &m2, &json!([1])), None);
        let x = field(&c, t, "X", FieldKind::Text, FieldOptions::default());
        let y = field(&c, t, "Y", FieldKind::Text, FieldOptions::default());
        assert_eq!(translate_value(&x, &y, &json!("t")), Some(json!("t")));
    }
}
