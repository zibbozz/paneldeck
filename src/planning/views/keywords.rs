//! JQL-Schlüsselwörter: benannte Prädikate (`IS blocked`, `IS NOT favorite`) und Sortierschlüssel (`ORDER BY favorites`).
//!
//! * **Eingebaut** (nicht löschbar, Definition und Name nicht änderbar, nur die Beschreibung): Prädikate `blocked`
//!   (Ticket ist markiert/Impediment) und `favorite` (Ticket hat einen Stern), Sortierschlüssel `favorites` (Sterne zuerst)
//!   und `flagged` (Markierte zuerst). Die Auswertung (Flag/Stern am Ticket) liegt in der Abfrage-Schicht, hier steht nur die Kennung.
//! * **Eigene**: ein Prädikat mit einer [`Cond`] auf Felder (Vergleich `{field_id, op, value}` oder UND/ODER-Kombination)
//!   bzw. ein Sortierschlüssel mit Feld und Richtung.
//! * Namen: Buchstaben, Ziffern und `_`, nicht mit einer Ziffer beginnend, höchstens 40 Zeichen, nicht reserviert
//!   ([`RESERVED`]) und – ohne Groß-/Kleinschreibung – unter Prädikaten und Sortierschlüsseln zusammen eindeutig.
//! * Felder gehören zu einem Typ eines Projekts; ein Schlüsselwort ist aber global. Verschwindet ein Feld, ist das Schlüsselwort
//!   „kaputt“ ([`broken_keywords`]) und wird dem Validator nicht mehr angeboten ([`schema_keywords`]); der Nutzer repariert oder löscht es.
//! * [`schema_keywords`] / [`apply_to_schema`] speisen den Validator (`crate::query::Schema::with_predicate/with_sort_keyword`).

use super::{atomic, clean_text, ensure_name_free, ConditionProblem, Res, ViewError, MAX_DESCRIPTION_CHARS};
use crate::planning::common;
use crate::planning::fields::{self, check_compare_value, FieldKind};
use crate::planning::workflow::ConditionOp;
use crate::planning::PlanError;
use crate::query::Schema;
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Reservierte Wörter der Abfragesprache – kein Schlüsselwort darf so heißen.
pub const RESERVED: [&str; 12] = ["AND", "OR", "NOT", "IN", "IS", "EMPTY", "NULL", "ORDER", "SORT", "BY", "ASC", "DESC"];
/// Längster Name eines Schlüsselworts.
pub const MAX_KEYWORD_CHARS: usize = 40;
/// Höchstzahl der Knoten (Vergleiche, UND, ODER) einer Bedingung.
pub const MAX_COND_NODES: usize = 30;
/// Tiefste Verschachtelung einer Bedingung.
pub const MAX_COND_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeywordKind {
    Predicate,
    Sort,
}

impl KeywordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            KeywordKind::Predicate => "predicate",
            KeywordKind::Sort => "sort",
        }
    }

    fn parse(s: &str) -> KeywordKind {
        if s == "sort" { KeywordKind::Sort } else { KeywordKind::Predicate }
    }
}

/// Kennung eines eingebauten Schlüsselworts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinId {
    /// Prädikat: Ticket ist markiert (Impediment).
    Blocked,
    /// Prädikat: Ticket hat einen Stern.
    Favorite,
    /// Sortierschlüssel: Tickets mit Stern zuerst.
    Favorites,
    /// Sortierschlüssel: markierte Tickets zuerst.
    Flagged,
}

impl BuiltinId {
    pub fn kind(self) -> KeywordKind {
        match self {
            BuiltinId::Blocked | BuiltinId::Favorite => KeywordKind::Predicate,
            BuiltinId::Favorites | BuiltinId::Flagged => KeywordKind::Sort,
        }
    }

    /// Mitgelieferte Beschreibung.
    pub fn description(self, lang: Lang) -> &'static str {
        match self {
            BuiltinId::Blocked => lang.t("Das Ticket ist markiert (Impediment).", "The ticket is flagged (impediment)."),
            BuiltinId::Favorite => lang.t("Das Ticket hat einen Stern.", "The ticket has a star."),
            BuiltinId::Favorites => lang.t("Tickets mit Stern zuerst.", "Starred tickets first."),
            BuiltinId::Flagged => lang.t("Markierte Tickets zuerst.", "Flagged tickets first."),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    Asc,
    Desc,
}

/// Bedingung eines eigenen Prädikats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Cond {
    /// „Feld `op` Wert“ (bei `In`/`NotIn` eine Liste; bei `IsEmpty`/`IsNotEmpty` ohne Wert).
    Compare {
        field_id: i64,
        op: ConditionOp,
        #[serde(default)]
        value: Value,
    },
    /// Alle Einträge müssen zutreffen.
    And { items: Vec<Cond> },
    /// Mindestens ein Eintrag muss zutreffen.
    Or { items: Vec<Cond> },
}

impl Cond {
    /// Alle Felder, auf die die Bedingung verweist.
    pub fn field_ids(&self) -> Vec<i64> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<i64>) {
        match self {
            Cond::Compare { field_id, .. } => out.push(*field_id),
            Cond::And { items } | Cond::Or { items } => items.iter().for_each(|i| i.collect(out)),
        }
    }
}

/// Definition eines Schlüsselworts (so steht sie als JSON in der Datenbank).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Definition {
    Builtin { id: BuiltinId },
    Predicate { condition: Cond },
    Sort { field_id: i64, dir: SortDir },
}

impl Definition {
    pub fn kind(&self) -> KeywordKind {
        match self {
            Definition::Builtin { id } => id.kind(),
            Definition::Predicate { .. } => KeywordKind::Predicate,
            Definition::Sort { .. } => KeywordKind::Sort,
        }
    }

    pub fn field_ids(&self) -> Vec<i64> {
        match self {
            Definition::Builtin { .. } => Vec::new(),
            Definition::Predicate { condition } => condition.field_ids(),
            Definition::Sort { field_id, .. } => vec![*field_id],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keyword {
    pub id: i64,
    pub name: String,
    pub kind: KeywordKind,
    pub builtin: bool,
    /// Eigene Beschreibung; `None` = bei Eingebauten der mitgelieferte, übersetzte Text, sonst keine.
    pub description: Option<String>,
    pub definition: Definition,
    pub position: i64,
}

impl Keyword {
    /// Beschreibung in der Sprache: die eigene, sonst bei Eingebauten die mitgelieferte, sonst leer.
    pub fn description_text(&self, lang: Lang) -> String {
        match (&self.description, &self.definition) {
            (Some(d), _) if !d.trim().is_empty() => d.clone(),
            (_, Definition::Builtin { id }) => id.description(lang).to_string(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewKeyword {
    pub name: String,
    pub description: String,
    /// Eigene Definition (`Builtin` ist nicht erlaubt).
    pub definition: Definition,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateKeyword {
    pub name: Option<String>,
    /// Leerer Text entfernt die eigene Beschreibung (bei Eingebauten: zurück zum mitgelieferten Text).
    pub description: Option<String>,
    pub definition: Option<Definition>,
}

const COLS: &str = "id, name, kind, builtin, description, definition, position";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Keyword> {
    let def_text: String = r.get(5)?;
    // Eine unlesbare Definition (sollte nicht vorkommen) wird zu einer leeren Bedingung, die `broken_keywords` meldet.
    let definition = serde_json::from_str(&def_text).unwrap_or(Definition::Predicate { condition: Cond::And { items: Vec::new() } });
    Ok(Keyword { id: r.get(0)?, name: r.get(1)?, kind: KeywordKind::parse(&r.get::<_, String>(2)?), builtin: r.get::<_, i64>(3)? != 0, description: r.get(4)?, definition, position: r.get(6)? })
}

pub fn get_keyword(conn: &Connection, id: i64) -> Res<Keyword> {
    conn.query_row(&format!("SELECT {COLS} FROM jql_keywords WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(ViewError::not_found)
}

/// Schlüsselwort über den Namen (ohne Groß-/Kleinschreibung).
pub fn keyword_by_name(conn: &Connection, name: &str) -> Res<Keyword> {
    conn.query_row(&format!("SELECT {COLS} FROM jql_keywords WHERE name = ?1 COLLATE NOCASE"), [name], from_row).optional()?.ok_or_else(ViewError::not_found)
}

/// Alle Schlüsselwörter: Eingebaute und eigene in der eingestellten Reihenfolge.
pub fn list_keywords(conn: &Connection) -> Res<Vec<Keyword>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM jql_keywords ORDER BY position, id"))?;
    Ok(st.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

pub fn list_predicates(conn: &Connection) -> Res<Vec<Keyword>> {
    Ok(list_keywords(conn)?.into_iter().filter(|k| k.kind == KeywordKind::Predicate).collect())
}

pub fn list_sort_keys(conn: &Connection) -> Res<Vec<Keyword>> {
    Ok(list_keywords(conn)?.into_iter().filter(|k| k.kind == KeywordKind::Sort).collect())
}

// ------------------------------------------------------------------ Prüfen

/// Prüft den Namen eines Schlüsselworts und gibt ihn bereinigt (Ränder abgeschnitten) zurück.
pub fn validate_name(name: &str) -> Res<String> {
    let n = name.trim();
    let mut chars = n.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    if !first_ok || n.chars().count() > MAX_KEYWORD_CHARS || !n.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ViewError::KeywordNameInvalid);
    }
    if RESERVED.iter().any(|w| w.eq_ignore_ascii_case(n)) {
        return Err(ViewError::KeywordReserved);
    }
    Ok(n.to_string())
}

fn field_of(conn: &Connection, id: i64) -> Res<fields::Field> {
    fields::get_field(conn, id).map_err(|e| if e == PlanError::NotFound { ViewError::KeywordFieldMissing } else { e.into() })
}

fn check_compare(conn: &Connection, field_id: i64, op: ConditionOp, value: &Value) -> Res<()> {
    let f = field_of(conn, field_id)?;
    let is_order = matches!(op, ConditionOp::Gt | ConditionOp::Ge | ConditionOp::Lt | ConditionOp::Le);
    if is_order && !matches!(f.kind, FieldKind::Number | FieldKind::Date | FieldKind::Datetime) {
        return Err(ViewError::Condition(ConditionProblem::Operator));
    }
    let bad = || ViewError::Condition(ConditionProblem::Value);
    match op {
        ConditionOp::IsEmpty | ConditionOp::IsNotEmpty => Ok(()),
        ConditionOp::In | ConditionOp::NotIn => {
            let list = value.as_array().filter(|a| !a.is_empty()).ok_or_else(bad)?;
            list.iter().try_for_each(|v| check_compare_value(f.kind, &f.options, v).map_err(|_| bad()))
        }
        _ => check_compare_value(f.kind, &f.options, value).map_err(|_| bad()),
    }
}

fn check_cond(conn: &Connection, c: &Cond, depth: usize, nodes: &mut usize) -> Res<()> {
    *nodes += 1;
    if depth > MAX_COND_DEPTH || *nodes > MAX_COND_NODES {
        return Err(ViewError::Condition(ConditionProblem::TooLarge));
    }
    match c {
        Cond::Compare { field_id, op, value } => check_compare(conn, *field_id, *op, value),
        Cond::And { items } | Cond::Or { items } => {
            if items.is_empty() {
                return Err(ViewError::Condition(ConditionProblem::Empty));
            }
            items.iter().try_for_each(|i| check_cond(conn, i, depth + 1, nodes))
        }
    }
}

/// Prüft eine eigene Definition (Felder vorhanden, Vergleiche passend, Größe) und liefert die Art.
pub fn check_definition(conn: &Connection, d: &Definition) -> Res<KeywordKind> {
    match d {
        Definition::Builtin { .. } => return Err(ViewError::BuiltinProtected),
        Definition::Predicate { condition } => check_cond(conn, condition, 1, &mut 0)?,
        Definition::Sort { field_id, .. } => {
            field_of(conn, *field_id)?;
        }
    }
    Ok(d.kind())
}

fn definition_json(d: &Definition) -> String {
    serde_json::to_string(d).unwrap_or_else(|_| "{}".to_string())
}

fn description_value(s: &str) -> Res<Option<String>> {
    let t = clean_text(s, MAX_DESCRIPTION_CHARS)?;
    Ok(if t.is_empty() { None } else { Some(t) })
}

// ------------------------------------------------------------------ Anlegen, ändern, löschen

pub fn create_keyword(conn: &Connection, n: &NewKeyword) -> Res<i64> {
    let name = validate_name(&n.name)?;
    let description = description_value(&n.description)?;
    atomic(conn, || {
        let kind = check_definition(conn, &n.definition)?;
        ensure_name_free(conn, "jql_keywords", "1 = 1", &[], &name, None)?;
        let pos = common::next_position(conn, "jql_keywords", "1 = 1", &[])?;
        conn.execute("INSERT INTO jql_keywords (name, kind, builtin, description, definition, position) VALUES (?1, ?2, 0, ?3, ?4, ?5)", params![name, kind.as_str(), description, definition_json(&n.definition), pos])?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_keyword(conn: &Connection, id: i64, u: &UpdateKeyword) -> Res<()> {
    let cur = get_keyword(conn, id)?;
    if cur.builtin && (u.name.is_some() || u.definition.is_some()) {
        return Err(ViewError::BuiltinProtected);
    }
    let name = match &u.name {
        Some(s) => validate_name(s)?,
        None => cur.name.clone(),
    };
    let description = match &u.description {
        Some(s) => description_value(s)?,
        None => cur.description.clone(),
    };
    atomic(conn, || {
        let (kind, def) = match &u.definition {
            Some(d) => (check_definition(conn, d)?, d.clone()),
            None => (cur.kind, cur.definition.clone()),
        };
        ensure_name_free(conn, "jql_keywords", "1 = 1", &[], &name, Some(id))?;
        conn.execute("UPDATE jql_keywords SET name = ?1, kind = ?2, description = ?3, definition = ?4 WHERE id = ?5", params![name, kind.as_str(), description, definition_json(&def), id])?;
        Ok(())
    })
}

/// Löscht ein eigenes Schlüsselwort; Eingebaute sind geschützt.
pub fn delete_keyword(conn: &Connection, id: i64) -> Res<()> {
    let k = get_keyword(conn, id)?;
    if k.builtin {
        return Err(ViewError::BuiltinProtected);
    }
    conn.execute("DELETE FROM jql_keywords WHERE id = ?1", [id])?;
    Ok(())
}

/// Neue Reihenfolge aller Schlüsselwörter; `ids` muss genau alle enthalten.
pub fn reorder_keywords(conn: &Connection, ids: &[i64]) -> Res<()> {
    Ok(common::reorder(conn, "jql_keywords", "1 = 1", &[], ids)?)
}

// ------------------------------------------------------------------ Validator

/// Eigene Schlüsselwörter, deren Felder es nicht mehr gibt (oder deren Definition nicht lesbar ist), nach Kennung.
pub fn broken_keywords(conn: &Connection) -> Res<Vec<i64>> {
    let mut out = Vec::new();
    for k in list_keywords(conn)? {
        if k.builtin {
            continue;
        }
        let empty_cond = matches!(&k.definition, Definition::Predicate { condition: Cond::And { items } } if items.is_empty());
        let missing = k.definition.field_ids().iter().any(|f| fields::get_field(conn, *f).is_err());
        if empty_cond || missing {
            out.push(k.id);
        }
    }
    Ok(out)
}

fn builtin_names() -> (Vec<String>, Vec<String>) {
    (vec!["blocked".into(), "favorite".into()], vec!["favorites".into(), "flagged".into()])
}

/// Die Namen für den Validator: (Prädikate, Sortierschlüssel), eingebaute zuerst. Kaputte eigene fehlen. Ein Datenbankfehler
/// ergibt die eingebauten Namen (der Validator soll die Oberfläche nie blockieren).
pub fn schema_keywords(conn: &Connection) -> (Vec<String>, Vec<String>) {
    let (Ok(all), Ok(broken)) = (list_keywords(conn), broken_keywords(conn)) else {
        return builtin_names();
    };
    let mut preds = Vec::new();
    let mut sorts = Vec::new();
    for k in all.into_iter().filter(|k| !broken.contains(&k.id)) {
        match k.kind {
            KeywordKind::Predicate => preds.push(k.name),
            KeywordKind::Sort => sorts.push(k.name),
        }
    }
    (preds, sorts)
}

/// Fügt dem Schema des Validators alle gültigen Schlüsselwörter hinzu.
pub fn apply_to_schema(conn: &Connection, schema: Schema) -> Schema {
    let (preds, sorts) = schema_keywords(conn);
    let schema = preds.iter().fold(schema, |s, p| s.with_predicate(p));
    sorts.iter().fold(schema, |s, p| s.with_sort_keyword(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{create_field, delete_field, list_fields, NewField};
    use crate::planning::types::list_types;
    use crate::planning::views::testutil::{db, project};
    use serde_json::json;

    /// (Feld-Kennung der Priorität, Story Points, Beschreibung) der Userstory eines Standard-Projekts.
    fn us_fields(c: &Connection, p: i64) -> (i64, i64, i64) {
        let t = list_types(c, p).unwrap().into_iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        let f = list_fields(c, t).unwrap();
        let id = |k: &str| f.iter().find(|x| x.key.as_deref() == Some(k)).unwrap().id;
        (id("priority"), id("story_points"), id("description"))
    }

    fn pred(name: &str, cond: Cond) -> NewKeyword {
        NewKeyword { name: name.into(), description: String::new(), definition: Definition::Predicate { condition: cond } }
    }

    fn cmp(field_id: i64, op: ConditionOp, value: Value) -> Cond {
        Cond::Compare { field_id, op, value }
    }

    #[test]
    fn builtins_exist_after_migration() {
        let c = db();
        let all = list_keywords(&c).unwrap();
        assert_eq!(all.iter().map(|k| (k.name.as_str(), k.kind, k.builtin)).collect::<Vec<_>>(), vec![("blocked", KeywordKind::Predicate, true), ("favorite", KeywordKind::Predicate, true), ("favorites", KeywordKind::Sort, true), ("flagged", KeywordKind::Sort, true)]);
        assert_eq!(all[0].definition, Definition::Builtin { id: BuiltinId::Blocked });
        assert_eq!(list_predicates(&c).unwrap().len(), 2);
        assert_eq!(list_sort_keys(&c).unwrap().len(), 2);
        assert_eq!(keyword_by_name(&c, "FAVORITES").unwrap().name, "favorites");
        assert_eq!(keyword_by_name(&c, "nix"), Err(ViewError::not_found()));
        // Beschreibungen in beiden Sprachen
        for k in &all {
            let (en, de) = (k.description_text(Lang::En), k.description_text(Lang::De));
            assert!(!en.is_empty() && !de.is_empty() && en != de, "{}", k.name);
        }
    }

    #[test]
    fn validates_names_and_reserved_words() {
        for ok in ["urgent", "Ready_For_QA", "_x", "a1", "größe", "A"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        assert_eq!(validate_name("  trimmed ").unwrap(), "trimmed");
        for bad in ["", " ", "1abc", "a b", "a-b", "a.b", "a:b", "ein+aus", &"x".repeat(41)] {
            assert_eq!(validate_name(bad), Err(ViewError::KeywordNameInvalid), "{bad}");
        }
        for w in ["AND", "or", "Not", "IN", "is", "EMPTY", "null", "ORDER", "sort", "by", "ASC", "desc"] {
            assert_eq!(validate_name(w), Err(ViewError::KeywordReserved), "{w}");
        }
        assert_eq!(RESERVED.len(), 12);
        // ähnlich, aber nicht reserviert
        assert!(validate_name("order_by").is_ok());
        assert!(validate_name("android").is_ok());
    }

    #[test]
    fn every_reserved_word_is_rejected_when_creating() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, _, _) = us_fields(&c, p);
        for w in RESERVED {
            assert_eq!(create_keyword(&c, &pred(w, cmp(prio, ConditionOp::IsEmpty, Value::Null))), Err(ViewError::KeywordReserved), "{w}");
        }
    }

    #[test]
    fn creates_a_custom_predicate_and_sort_key() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, sp, _) = us_fields(&c, p);
        let big = create_keyword(&c, &NewKeyword { name: "big".into(), description: " groß ".into(), definition: Definition::Predicate { condition: cmp(sp, ConditionOp::Ge, json!(8)) } }).unwrap();
        let by_points = create_keyword(&c, &NewKeyword { name: "points".into(), description: String::new(), definition: Definition::Sort { field_id: sp, dir: SortDir::Desc } }).unwrap();
        let k = get_keyword(&c, big).unwrap();
        assert_eq!((k.kind, k.builtin, k.description.as_deref()), (KeywordKind::Predicate, false, Some("groß")));
        assert_eq!(k.position, 4);
        assert_eq!(get_keyword(&c, by_points).unwrap().kind, KeywordKind::Sort);
        assert_eq!(get_keyword(&c, by_points).unwrap().description, None);
        assert_eq!(get_keyword(&c, by_points).unwrap().definition, Definition::Sort { field_id: sp, dir: SortDir::Desc });
        // Priorität = Dropdown mit Kennungen
        assert!(create_keyword(&c, &pred("high", cmp(prio, ConditionOp::In, json!([1, 2])))).is_ok());
        assert_eq!(list_predicates(&c).unwrap().len(), 4);
        assert_eq!(list_sort_keys(&c).unwrap().len(), 3);
    }

    #[test]
    fn combines_conditions_with_and_or() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, sp, _) = us_fields(&c, p);
        let cond = Cond::And { items: vec![cmp(sp, ConditionOp::Ge, json!(5)), Cond::Or { items: vec![cmp(prio, ConditionOp::Eq, json!(1)), cmp(prio, ConditionOp::Eq, json!(2))] }] };
        let id = create_keyword(&c, &pred("urgent_big", cond.clone())).unwrap();
        assert_eq!(get_keyword(&c, id).unwrap().definition, Definition::Predicate { condition: cond.clone() });
        assert_eq!(cond.field_ids(), vec![sp, prio, prio]);
        // JSON-Form ist stabil
        let v = serde_json::to_value(&cond).unwrap();
        assert_eq!(v["type"], "and");
        assert_eq!(v["items"][1]["type"], "or");
        assert_eq!(v["items"][0], json!({"type": "compare", "field_id": sp, "op": "ge", "value": 5}));
    }

    #[test]
    fn rejects_bad_definitions() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, sp, desc) = us_fields(&c, p);
        let err = |cond| create_keyword(&c, &pred("k", cond));
        assert_eq!(err(cmp(99999, ConditionOp::Eq, json!(1))), Err(ViewError::KeywordFieldMissing));
        assert_eq!(err(cmp(desc, ConditionOp::Gt, json!(1))), Err(ViewError::Condition(ConditionProblem::Operator)), "größer bei Text");
        assert_eq!(err(cmp(sp, ConditionOp::Eq, json!("viel"))), Err(ViewError::Condition(ConditionProblem::Value)));
        assert_eq!(err(cmp(sp, ConditionOp::In, json!([]))), Err(ViewError::Condition(ConditionProblem::Value)));
        assert_eq!(err(cmp(sp, ConditionOp::In, json!(5))), Err(ViewError::Condition(ConditionProblem::Value)));
        assert_eq!(err(Cond::And { items: vec![] }), Err(ViewError::Condition(ConditionProblem::Empty)));
        assert_eq!(err(Cond::Or { items: vec![Cond::Or { items: vec![] }] }), Err(ViewError::Condition(ConditionProblem::Empty)));
        // IsEmpty braucht keinen Wert
        assert!(err(cmp(prio, ConditionOp::IsEmpty, Value::Null)).is_ok());
        // zu tief
        let mut deep = cmp(sp, ConditionOp::Eq, json!(1));
        for _ in 0..MAX_COND_DEPTH {
            deep = Cond::And { items: vec![deep] };
        }
        assert_eq!(create_keyword(&c, &pred("deep", deep)), Err(ViewError::Condition(ConditionProblem::TooLarge)));
        // zu viele Knoten
        let wide = Cond::Or { items: (0..MAX_COND_NODES).map(|_| cmp(sp, ConditionOp::Eq, json!(1))).collect() };
        assert_eq!(create_keyword(&c, &pred("wide", wide)), Err(ViewError::Condition(ConditionProblem::TooLarge)));
        // Sortierschlüssel mit fehlendem Feld
        assert_eq!(create_keyword(&c, &NewKeyword { name: "s".into(), description: String::new(), definition: Definition::Sort { field_id: 99999, dir: SortDir::Asc } }), Err(ViewError::KeywordFieldMissing));
        // eingebaute Definition lässt sich nicht neu anlegen
        assert_eq!(create_keyword(&c, &NewKeyword { name: "b".into(), description: String::new(), definition: Definition::Builtin { id: BuiltinId::Blocked } }), Err(ViewError::BuiltinProtected));
        assert_eq!(list_keywords(&c).unwrap().len(), 5, "nur das gültige „k“ (IsEmpty) ist dazugekommen, nichts von den abgelehnten");
    }

    #[test]
    fn names_are_unique_case_insensitively_across_kinds() {
        let c = db();
        let p = project(&c, "AA", true);
        let (_, sp, _) = us_fields(&c, p);
        let ok = pred("big", cmp(sp, ConditionOp::Ge, json!(8)));
        create_keyword(&c, &ok).unwrap();
        assert_eq!(create_keyword(&c, &pred("BIG", cmp(sp, ConditionOp::Ge, json!(1)))), Err(PlanError::NameTaken.into()));
        // gegen die Eingebauten
        assert_eq!(create_keyword(&c, &pred("Blocked", cmp(sp, ConditionOp::Ge, json!(1)))), Err(PlanError::NameTaken.into()));
        assert_eq!(create_keyword(&c, &NewKeyword { name: "FLAGGED".into(), description: String::new(), definition: Definition::Sort { field_id: sp, dir: SortDir::Asc } }), Err(PlanError::NameTaken.into()));
        // Umlaute
        create_keyword(&c, &pred("Größe", cmp(sp, ConditionOp::Ge, json!(1)))).unwrap();
        assert_eq!(create_keyword(&c, &pred("größe", cmp(sp, ConditionOp::Ge, json!(1)))), Err(PlanError::NameTaken.into()));
    }

    #[test]
    fn builtins_are_protected_but_description_is_editable() {
        let c = db();
        let k = keyword_by_name(&c, "blocked").unwrap();
        assert_eq!(delete_keyword(&c, k.id), Err(ViewError::BuiltinProtected));
        assert_eq!(update_keyword(&c, k.id, &UpdateKeyword { name: Some("x".into()), ..Default::default() }), Err(ViewError::BuiltinProtected));
        assert_eq!(update_keyword(&c, k.id, &UpdateKeyword { definition: Some(Definition::Builtin { id: BuiltinId::Favorite }), ..Default::default() }), Err(ViewError::BuiltinProtected));
        update_keyword(&c, k.id, &UpdateKeyword { description: Some("  Unser Impediment ".into()), ..Default::default() }).unwrap();
        let k2 = get_keyword(&c, k.id).unwrap();
        assert_eq!(k2.description_text(Lang::De), "Unser Impediment");
        assert_eq!(k2.description_text(Lang::En), "Unser Impediment");
        assert_eq!(k2.definition, k.definition);
        assert_eq!(k2.name, "blocked");
        // leer = zurück zum mitgelieferten Text
        update_keyword(&c, k.id, &UpdateKeyword { description: Some(" ".into()), ..Default::default() }).unwrap();
        assert_eq!(get_keyword(&c, k.id).unwrap().description_text(Lang::En), "The ticket is flagged (impediment).");
        assert_eq!(list_keywords(&c).unwrap().len(), 4);
    }

    #[test]
    fn updates_and_deletes_custom_keywords() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, sp, _) = us_fields(&c, p);
        let id = create_keyword(&c, &pred("big", cmp(sp, ConditionOp::Ge, json!(8)))).unwrap();
        update_keyword(&c, id, &UpdateKeyword { name: Some("huge".into()), definition: Some(Definition::Predicate { condition: cmp(sp, ConditionOp::Ge, json!(13)) }), ..Default::default() }).unwrap();
        let k = get_keyword(&c, id).unwrap();
        assert_eq!(k.name, "huge");
        assert_eq!(k.definition, Definition::Predicate { condition: cmp(sp, ConditionOp::Ge, json!(13)) });
        // Art wechseln: Prädikat -> Sortierschlüssel
        update_keyword(&c, id, &UpdateKeyword { definition: Some(Definition::Sort { field_id: prio, dir: SortDir::Asc }), ..Default::default() }).unwrap();
        assert_eq!(get_keyword(&c, id).unwrap().kind, KeywordKind::Sort);
        // Namenskonflikt und ungültige Änderung ändern nichts
        assert_eq!(update_keyword(&c, id, &UpdateKeyword { name: Some("Blocked".into()), ..Default::default() }), Err(PlanError::NameTaken.into()));
        assert_eq!(update_keyword(&c, id, &UpdateKeyword { name: Some("AND".into()), ..Default::default() }), Err(ViewError::KeywordReserved));
        assert_eq!(update_keyword(&c, id, &UpdateKeyword { definition: Some(Definition::Sort { field_id: 1234567, dir: SortDir::Asc }), ..Default::default() }), Err(ViewError::KeywordFieldMissing));
        assert_eq!(get_keyword(&c, id).unwrap().name, "huge");
        // eigenen Namen in anderer Schreibweise setzen ist erlaubt
        update_keyword(&c, id, &UpdateKeyword { name: Some("HUGE".into()), ..Default::default() }).unwrap();
        delete_keyword(&c, id).unwrap();
        assert_eq!(delete_keyword(&c, id), Err(ViewError::not_found()));
        assert_eq!(update_keyword(&c, id, &UpdateKeyword::default()), Err(ViewError::not_found()));
    }

    #[test]
    fn description_is_limited() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, _, _) = us_fields(&c, p);
        let mut n = pred("k", cmp(prio, ConditionOp::IsEmpty, Value::Null));
        n.description = "d".repeat(501);
        assert_eq!(create_keyword(&c, &n), Err(ViewError::TextTooLong { max: 500 }));
    }

    #[test]
    fn feeds_the_validator_schema() {
        let c = db();
        let p = project(&c, "AA", true);
        let (_, sp, _) = us_fields(&c, p);
        let (preds, sorts) = schema_keywords(&c);
        assert_eq!(preds, vec!["blocked", "favorite"]);
        assert_eq!(sorts, vec!["favorites", "flagged"]);
        create_keyword(&c, &pred("big", cmp(sp, ConditionOp::Ge, json!(8)))).unwrap();
        create_keyword(&c, &NewKeyword { name: "bypoints".into(), description: String::new(), definition: Definition::Sort { field_id: sp, dir: SortDir::Desc } }).unwrap();
        let (preds, sorts) = schema_keywords(&c);
        assert_eq!(preds, vec!["blocked", "favorite", "big"]);
        assert_eq!(sorts, vec!["favorites", "flagged", "bypoints"]);
        let schema = apply_to_schema(&c, Schema::new());
        assert!(schema.predicate("BIG").is_some() && schema.predicate("blocked").is_some());
        assert!(schema.sort_keyword("Favorites").is_some() && schema.sort_keyword("bypoints").is_some());
        assert!(schema.predicate("favorites").is_none(), "Sortierschlüssel ist kein Prädikat");
    }

    #[test]
    fn schema_is_accepted_by_the_query_validator() {
        let c = db();
        let schema = apply_to_schema(&c, Schema::new().with_field("status", crate::query::FieldType::Text));
        let q = crate::query::parse("IS favorite AND IS NOT blocked ORDER BY favorites, flagged").unwrap();
        let issues = crate::query::validate(&q, &schema);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn deleting_a_field_breaks_the_keyword_and_hides_it_from_the_validator() {
        let c = db();
        let p = project(&c, "AA", true);
        let (_, sp, _) = us_fields(&c, p);
        let id = create_keyword(&c, &pred("big", cmp(sp, ConditionOp::Ge, json!(8)))).unwrap();
        assert!(broken_keywords(&c).unwrap().is_empty());
        delete_field(&c, sp, None).unwrap();
        assert_eq!(broken_keywords(&c).unwrap(), vec![id]);
        let (preds, _) = schema_keywords(&c);
        assert_eq!(preds, vec!["blocked", "favorite"]);
        // die Definition bleibt lesbar, der Nutzer kann reparieren oder löschen
        assert!(get_keyword(&c, id).is_ok());
        delete_keyword(&c, id).unwrap();
        assert!(broken_keywords(&c).unwrap().is_empty());
    }

    #[test]
    fn unreadable_definition_counts_as_broken() {
        let c = db();
        let p = project(&c, "AA", true);
        let (_, sp, _) = us_fields(&c, p);
        let id = create_keyword(&c, &pred("big", cmp(sp, ConditionOp::Ge, json!(8)))).unwrap();
        c.execute("UPDATE jql_keywords SET definition = 'kaputt' WHERE id = ?1", [id]).unwrap();
        assert_eq!(broken_keywords(&c).unwrap(), vec![id]);
    }

    #[test]
    fn reorders_keywords() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, _, _) = us_fields(&c, p);
        let a = create_keyword(&c, &pred("a", cmp(prio, ConditionOp::IsEmpty, Value::Null))).unwrap();
        let mut ids: Vec<i64> = list_keywords(&c).unwrap().iter().map(|k| k.id).collect();
        assert_eq!(*ids.last().unwrap(), a);
        ids.rotate_right(1);
        reorder_keywords(&c, &ids).unwrap();
        assert_eq!(list_keywords(&c).unwrap()[0].id, a);
        assert_eq!(reorder_keywords(&c, &ids[1..]), Err(PlanError::ReorderMismatch.into()));
    }

    #[test]
    fn cascades_nothing_but_survives_project_deletion_as_broken() {
        let c = db();
        let p = project(&c, "AA", true);
        let (prio, _, _) = us_fields(&c, p);
        let id = create_keyword(&c, &pred("a", cmp(prio, ConditionOp::IsEmpty, Value::Null))).unwrap();
        crate::planning::projects::delete_project(&c, p).unwrap();
        assert!(get_keyword(&c, id).is_ok());
        assert_eq!(broken_keywords(&c).unwrap(), vec![id]);
        // neue Felder ändern daran nichts
        let q = project(&c, "BB", true);
        let t = list_types(&c, q).unwrap()[0].id;
        create_field(&c, &NewField::new(t, "n", FieldKind::Number)).unwrap();
        assert_eq!(broken_keywords(&c).unwrap(), vec![id]);
    }
}
