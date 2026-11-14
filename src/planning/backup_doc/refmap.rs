//! Übersetzung der Kennungen im Verlauf zwischen Datenbank-Ids und Dokument-Nummern (und zurück).
//!
//! Der Verlauf speichert Rohwerte: Status, Typ und Eltern als Zahl-Text, `field:<Feld-Id>`, Werte von Ticket-Feldern als
//! JSON-Zahl, in Verknüpfungen die Id des Verknüpfungstyps und in Zeiteinträgen die Id der Art der Arbeit. Dieselbe
//! Funktion übersetzt in beide Richtungen; die Gegenseite entscheidet [`IdMap`]. `0` heißt „gibt es dort nicht“ – so
//! zeigt die Anzeige weiterhin „(gelöscht)“, ohne auf eine fremde Kennung zu zeigen.

use crate::planning::fields::FieldKind;
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Ref {
    Status,
    Type,
    Field,
    Ticket,
    LinkType,
    WorkKind,
    Version,
    Milestone,
}

pub(super) trait IdMap {
    /// Kennung auf der Gegenseite; `0` = gibt es dort nicht.
    fn map(&mut self, r: Ref, id: i64) -> i64;
    /// Feldtyp eines Felds (Kennung der Quellseite).
    fn field_kind(&self, source_field: i64) -> Option<FieldKind>;
}

fn id_text(m: &mut dyn IdMap, r: Ref, s: Option<&str>) -> Option<String> {
    s.map(|s| match s.trim().parse::<i64>() {
        Ok(i) => m.map(r, i).to_string(),
        Err(_) => s.to_string(),
    })
}

/// Schlüssel `key` eines JSON-Objekts (Text) mit `r` übersetzen; nicht lesbarer Text bleibt unverändert.
fn json_id(m: &mut dyn IdMap, r: Ref, key: &str, s: Option<&str>, zero_is_null: bool) -> Option<String> {
    let s = s?;
    let Ok(mut v) = serde_json::from_str::<Value>(s) else { return Some(s.to_string()) };
    if let Some(obj) = v.as_object_mut() {
        if let Some(id) = obj.get(key).and_then(Value::as_i64) {
            let to = m.map(r, id);
            obj.insert(key.to_string(), if to == 0 && zero_is_null { Value::Null } else { Value::from(to) });
        }
    }
    Some(v.to_string())
}

/// Übersetzt einen Verlaufseintrag (`field`-Spalte samt `old`/`new`).
pub(super) fn translate_history(col: &str, old: Option<&str>, new: Option<&str>, m: &mut dyn IdMap) -> (String, Option<String>, Option<String>) {
    match col {
        "status" => (col.to_string(), id_text(m, Ref::Status, old), id_text(m, Ref::Status, new)),
        "type" => (col.to_string(), id_text(m, Ref::Type, old), id_text(m, Ref::Type, new)),
        "parent" => (col.to_string(), id_text(m, Ref::Ticket, old), id_text(m, Ref::Ticket, new)),
        "link" => (col.to_string(), json_id(m, Ref::LinkType, "type", old, false), json_id(m, Ref::LinkType, "type", new, false)),
        "time" => (col.to_string(), json_id(m, Ref::WorkKind, "kind", old, true), json_id(m, Ref::WorkKind, "kind", new, true)),
        c => match c.strip_prefix("field:").and_then(|n| n.parse::<i64>().ok()) {
            Some(src) => {
                let to = m.map(Ref::Field, src);
                let target = match m.field_kind(src) {
                    Some(FieldKind::Ticket) => Some(Ref::Ticket),
                    Some(FieldKind::Version) => Some(Ref::Version),
                    Some(FieldKind::Milestone) => Some(Ref::Milestone),
                    _ => None,
                };
                match target {
                    Some(r) => (format!("field:{to}"), id_text(m, r, old), id_text(m, r, new)),
                    None => (format!("field:{to}"), old.map(str::to_string), new.map(str::to_string)),
                }
            }
            None => (c.to_string(), old.map(str::to_string), new.map(str::to_string)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verdoppelt jede Kennung (Hin) bzw. halbiert sie (zurück); 99 gibt es nicht.
    struct Double;

    impl IdMap for Double {
        fn map(&mut self, _r: Ref, id: i64) -> i64 {
            if id == 99 { 0 } else { id * 2 }
        }

        fn field_kind(&self, f: i64) -> Option<FieldKind> {
            match f {
                7 => Some(FieldKind::Ticket),
                8 => Some(FieldKind::Text),
                9 => Some(FieldKind::Version),
                10 => Some(FieldKind::Milestone),
                _ => None,
            }
        }
    }

    #[test]
    fn translates_ids_and_leaves_texts_alone() {
        let mut m = Double;
        assert_eq!(translate_history("status", Some("3"), Some("4"), &mut m), ("status".into(), Some("6".into()), Some("8".into())));
        assert_eq!(translate_history("status", None, Some("99"), &mut m).2.as_deref(), Some("0"), "Unbekanntes wird 0");
        assert_eq!(translate_history("parent", Some("5"), None, &mut m), ("parent".into(), Some("10".into()), None));
        assert_eq!(translate_history("type", Some("1"), Some("2"), &mut m).1.as_deref(), Some("2"));
        // Felder: Ticket-Felder übersetzen auch den Wert
        assert_eq!(translate_history("field:7", Some("3"), Some("4"), &mut m), ("field:14".into(), Some("6".into()), Some("8".into())));
        assert_eq!(translate_history("field:8", Some("\"3\""), Some("\"x\""), &mut m), ("field:16".into(), Some("\"3\"".into()), Some("\"x\"".into())));
        assert_eq!(translate_history("field:99", Some("1"), None, &mut m).0, "field:0");
        // Version-/Meilenstein-Felder übersetzen den Wert ebenfalls (unbekannte werden 0)
        assert_eq!(translate_history("field:9", Some("2"), Some("99"), &mut m), ("field:18".into(), Some("4".into()), Some("0".into())));
        assert_eq!(translate_history("field:10", None, Some("5"), &mut m), ("field:20".into(), None, Some("10".into())));
        // Texte bleiben
        assert_eq!(translate_history("title", Some("a"), Some("b"), &mut m), ("title".into(), Some("a".into()), Some("b".into())));
        assert_eq!(translate_history("moved", Some("OLD-1"), Some("NEW-2"), &mut m).1.as_deref(), Some("OLD-1"));
        assert_eq!(translate_history("created", None, Some("clone:PAN-3"), &mut m).2.as_deref(), Some("clone:PAN-3"));
        assert_eq!(translate_history("status", Some("abc"), None, &mut m).1.as_deref(), Some("abc"), "Nicht-Zahlen bleiben");
        assert_eq!(translate_history("future", Some("1"), Some("2"), &mut m).1.as_deref(), Some("1"));
    }

    #[test]
    fn translates_ids_in_link_and_time_json() {
        let mut m = Double;
        let link = r#"{"dir":"out","kind":"ticket","target":"PAN-4","type":3}"#;
        let (_, old, new) = translate_history("link", None, Some(link), &mut m);
        assert!(old.is_none());
        let v: Value = serde_json::from_str(&new.unwrap()).unwrap();
        assert_eq!(v["type"], 6);
        assert_eq!(v["target"], "PAN-4", "Schlüssel in Texten bleiben");
        let time = r#"{"comment":"x","kind":5,"minutes":30,"start":"2026-01-01T08:00:00"}"#;
        let (_, _, new) = translate_history("time", None, Some(time), &mut m);
        assert_eq!(serde_json::from_str::<Value>(&new.unwrap()).unwrap()["kind"], 10);
        // Art der Arbeit gibt es nicht mehr → null; ohne Art bleibt null
        let (_, _, gone) = translate_history("time", None, Some(r#"{"kind":99,"minutes":5}"#), &mut m);
        assert_eq!(serde_json::from_str::<Value>(&gone.unwrap()).unwrap()["kind"], Value::Null);
        let (_, _, none) = translate_history("time", None, Some(r#"{"kind":null,"minutes":5}"#), &mut m);
        assert_eq!(serde_json::from_str::<Value>(&none.unwrap()).unwrap()["kind"], Value::Null);
        // kein JSON → unverändert
        assert_eq!(translate_history("link", Some("kaputt"), None, &mut m).1.as_deref(), Some("kaputt"));
    }
}
