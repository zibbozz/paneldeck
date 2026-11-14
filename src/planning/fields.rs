//! Felder eines Typs: Feldtyp mit typspezifischen Optionen (Dropdown-Werte mit Farbe, Zahl min/max/Stellen/Einheit,
//! Textlänge, Zeilen für Rich-Text, Ziel-Typen für Ticket-Felder), Pflichtfeld, Standardwert, Hilfetext, Abschnitt,
//! Reihenfolge und „auf Karte anzeigen“.
//!
//! * Dropdown-/Mehrfachauswahl-Werte tragen eine **stabile Kennung** (`OptionValue::id`, im Feld eindeutig); Standardwerte,
//!   Bedingungen und später die Ticket-Werte verweisen darauf, ein Umbenennen ändert sie nicht.
//! * Werte (Standardwert, Wert in Übergangsregeln) als JSON: Text/Rich = String, Checkbox = Bool, Zahl = Zahl, Dropdown =
//!   Kennung, Mehrfachauswahl = Liste von Kennungen, Labels = Liste von Strings, Datum = `JJJJ-MM-TT`, Datum+Uhrzeit =
//!   `JJJJ-MM-TTTHH:MM[:SS]`. Verweis-Felder (Ticket, Terminal, Version, Meilenstein) haben keinen Standardwert.
//! * Der Feldtyp lässt sich nach dem Anlegen nicht mehr ändern (Löschen mit Migration und neu anlegen).

use super::common::{self, atomic, bool_i, clean_name, ensure_name_free, normalize_color, rename_value, validate_ident, MAX_NAME_CHARS};
use super::integrity::{self, DeleteMode};
use super::names;
use super::projects;
use super::types::editable_type;
use super::{OptionsProblem, PlanError, Res};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Rich,
    Checkbox,
    Dropdown,
    Number,
    Ticket,
    Terminal,
    Version,
    Milestone,
    Date,
    Datetime,
    Multi,
    Labels,
}

impl FieldKind {
    pub const ALL: [FieldKind; 13] = [
        FieldKind::Text,
        FieldKind::Rich,
        FieldKind::Checkbox,
        FieldKind::Dropdown,
        FieldKind::Number,
        FieldKind::Ticket,
        FieldKind::Terminal,
        FieldKind::Version,
        FieldKind::Milestone,
        FieldKind::Date,
        FieldKind::Datetime,
        FieldKind::Multi,
        FieldKind::Labels,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Rich => "rich",
            FieldKind::Checkbox => "checkbox",
            FieldKind::Dropdown => "dropdown",
            FieldKind::Number => "number",
            FieldKind::Ticket => "ticket",
            FieldKind::Terminal => "terminal",
            FieldKind::Version => "version",
            FieldKind::Milestone => "milestone",
            FieldKind::Date => "date",
            FieldKind::Datetime => "datetime",
            FieldKind::Multi => "multi",
            FieldKind::Labels => "labels",
        }
    }

    pub fn parse(s: &str) -> Option<FieldKind> {
        FieldKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Verweist der Wert auf ein anderes Objekt (Ticket, Terminal-Sitzung, Version, Meilenstein)? Dann gibt es keinen Standardwert.
    pub fn is_reference(self) -> bool {
        matches!(self, FieldKind::Ticket | FieldKind::Terminal | FieldKind::Version | FieldKind::Milestone)
    }

    /// Hat das Feld eine Werteliste (Dropdown, Mehrfachauswahl)?
    pub fn has_values(self) -> bool {
        matches!(self, FieldKind::Dropdown | FieldKind::Multi)
    }
}

/// Ein Wert eines Dropdowns bzw. einer Mehrfachauswahl.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionValue {
    /// Stabile Kennung im Feld; 0 = beim Speichern vergeben.
    #[serde(default)]
    pub id: i64,
    /// Standard-Schlüssel für die Übersetzung (z. B. `priority.medium`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Eigener Name (gilt für beide Sprachen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// „#RRGGBB“
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl OptionValue {
    pub fn new(name: &str) -> Self {
        OptionValue { name: Some(name.to_string()), ..Default::default() }
    }

    pub fn display(&self, lang: Lang) -> String {
        names::display_name(self.key.as_deref(), self.name.as_deref(), lang)
    }
}

/// Typspezifische Optionen eines Feldes (nicht Passendes wird beim Speichern verworfen).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldOptions {
    /// Dropdown/Mehrfachauswahl: die Werte in Anzeigereihenfolge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<OptionValue>,
    /// Zahl: Untergrenze/Obergrenze, Nachkommastellen (0–6), Einheit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Text: höchste Länge in Zeichen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    /// Rich-Text: Höhe des Eingabefeldes in Zeilen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u32>,
    /// Ticket-Feld: erlaubte Typen der Ziel-Tickets (leer = jeder Typ).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_types: Vec<i64>,
}

pub fn parse_options(json: &str) -> FieldOptions {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn options_json(o: &FieldOptions) -> String {
    serde_json::to_string(o).unwrap_or_else(|_| "{}".to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub id: i64,
    pub type_id: i64,
    pub key: Option<String>,
    pub name: Option<String>,
    pub kind: FieldKind,
    pub options: FieldOptions,
    pub required: bool,
    pub default_value: Option<Value>,
    pub help: String,
    /// Abschnitt (Gruppe) im Ticket; leer = ohne Abschnitt.
    pub section: String,
    pub position: i64,
    pub show_on_card: bool,
    /// Mitgeliefertes Standardfeld (nur Kennzeichnung, ändert nichts an den Rechten).
    pub builtin: bool,
}

impl Field {
    pub fn display(&self, lang: Lang) -> String {
        names::display_name(self.key.as_deref(), self.name.as_deref(), lang)
    }
}

#[derive(Debug, Clone)]
pub struct NewField {
    pub type_id: i64,
    pub key: Option<String>,
    pub name: Option<String>,
    pub kind: FieldKind,
    pub options: FieldOptions,
    pub required: bool,
    pub default_value: Option<Value>,
    pub help: String,
    pub section: String,
    pub show_on_card: bool,
    pub builtin: bool,
}

impl NewField {
    /// Eigenes Feld mit Namen und Feldtyp, alles andere leer/aus.
    pub fn new(type_id: i64, name: &str, kind: FieldKind) -> Self {
        NewField {
            type_id,
            key: None,
            name: Some(name.to_string()),
            kind,
            options: FieldOptions::default(),
            required: false,
            default_value: None,
            help: String::new(),
            section: String::new(),
            show_on_card: false,
            builtin: false,
        }
    }
}

/// Änderungen an einem Feld; `None` = unverändert.
#[derive(Debug, Clone, Default)]
pub struct UpdateField {
    /// Umbenennen; leer = Standardname (nur mit Schlüssel).
    pub name: Option<String>,
    pub options: Option<FieldOptions>,
    pub required: Option<bool>,
    /// `Some(None)` entfernt den Standardwert.
    pub default_value: Option<Option<Value>>,
    pub help: Option<String>,
    pub section: Option<String>,
    pub show_on_card: Option<bool>,
}

pub(crate) const COLS: &str = "id, type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin";
const MAX_HELP_CHARS: usize = 1000;

pub(crate) fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Field> {
    let kind: String = r.get(4)?;
    let options: String = r.get(5)?;
    let default: Option<String> = r.get(7)?;
    Ok(Field {
        id: r.get(0)?,
        type_id: r.get(1)?,
        key: r.get(2)?,
        name: r.get(3)?,
        kind: FieldKind::parse(&kind).unwrap_or(FieldKind::Text),
        options: parse_options(&options),
        required: r.get::<_, i64>(6)? != 0,
        default_value: default.and_then(|d| serde_json::from_str(&d).ok()),
        help: r.get(8)?,
        section: r.get(9)?,
        position: r.get(10)?,
        show_on_card: r.get::<_, i64>(11)? != 0,
        builtin: r.get::<_, i64>(12)? != 0,
    })
}

pub fn get_field(conn: &Connection, id: i64) -> Res<Field> {
    conn.query_row(&format!("SELECT {COLS} FROM fields WHERE id = ?1"), [id], from_row).optional()?.ok_or(PlanError::NotFound)
}

pub fn list_fields(conn: &Connection, type_id: i64) -> Res<Vec<Field>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM fields WHERE type_id = ?1 ORDER BY position, id"))?;
    Ok(st.query_map([type_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

// ------------------------------------------------------------------ Optionen und Werte prüfen

/// Prüft und bereinigt die Optionen für den Feldtyp (nicht Passendes entfällt, Farben werden normalisiert, fehlende
/// Wert-Kennungen vergeben) und stellt sicher, dass Ziel-Typen zum Projekt gehören.
pub fn validate_options(conn: &Connection, project_id: i64, kind: FieldKind, o: &mut FieldOptions) -> Res<()> {
    let src = std::mem::take(o);
    let mut out = FieldOptions::default();
    let bad = |p: OptionsProblem| Err(PlanError::Options(p));
    match kind {
        FieldKind::Dropdown | FieldKind::Multi => {
            let mut ids: Vec<i64> = Vec::new();
            for v in &src.values {
                if v.id < 0 {
                    return bad(OptionsProblem::DuplicateId);
                }
                if v.id > 0 {
                    if ids.contains(&v.id) {
                        return bad(OptionsProblem::DuplicateId);
                    }
                    ids.push(v.id);
                }
            }
            let mut next = ids.iter().copied().max().unwrap_or(0);
            let mut seen_labels: Vec<String> = Vec::new();
            for v in &src.values {
                let key = match v.key.as_deref().map(str::trim) {
                    Some(k) if !k.is_empty() => {
                        validate_ident(k)?;
                        Some(k.to_string())
                    }
                    _ => None,
                };
                let name = match v.name.as_deref() {
                    Some(n) if !n.trim().is_empty() => Some(clean_name(n).map_err(|_| PlanError::Options(OptionsProblem::EmptyValueName))?),
                    _ => None,
                };
                if key.is_none() && name.is_none() {
                    return bad(OptionsProblem::EmptyValueName);
                }
                let color = match v.color.as_deref() {
                    Some(c) => Some(normalize_color(c).map_err(|_| PlanError::Options(OptionsProblem::ValueColor))?),
                    None => None,
                };
                let labels = common::labels(key.as_deref(), name.as_deref());
                if labels.iter().any(|l| seen_labels.contains(l)) {
                    return bad(OptionsProblem::DuplicateValue);
                }
                seen_labels.extend(labels);
                let id = if v.id > 0 {
                    v.id
                } else {
                    next += 1;
                    next
                };
                out.values.push(OptionValue { id, key, name, color });
            }
        }
        FieldKind::Number => {
            for x in [src.min, src.max].into_iter().flatten() {
                if !x.is_finite() {
                    return bad(OptionsProblem::MinGreaterMax);
                }
            }
            if let (Some(a), Some(b)) = (src.min, src.max) {
                if a > b {
                    return bad(OptionsProblem::MinGreaterMax);
                }
            }
            if src.decimals.is_some_and(|d| d > 6) {
                return bad(OptionsProblem::Decimals);
            }
            let unit = src.unit.as_deref().map(|u| u.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|u| !u.is_empty());
            if unit.as_ref().is_some_and(|u| u.chars().count() > 16) {
                return bad(OptionsProblem::Unit);
            }
            out.min = src.min;
            out.max = src.max;
            out.decimals = src.decimals;
            out.unit = unit;
        }
        FieldKind::Text => {
            if src.max_length.is_some_and(|l| l == 0 || l > 100_000) {
                return bad(OptionsProblem::TextLength);
            }
            out.max_length = src.max_length;
        }
        FieldKind::Rich => {
            if src.rows.is_some_and(|r| !(1..=50).contains(&r)) {
                return bad(OptionsProblem::Rows);
            }
            out.rows = src.rows;
        }
        FieldKind::Ticket => {
            let mut targets = src.target_types.clone();
            targets.sort_unstable();
            targets.dedup();
            for t in &targets {
                let pid: Option<i64> = conn.query_row("SELECT project_id FROM ticket_types WHERE id = ?1", [t], |r| r.get(0)).optional()?;
                if pid != Some(project_id) {
                    return bad(OptionsProblem::TargetType);
                }
            }
            out.target_types = targets;
        }
        FieldKind::Checkbox | FieldKind::Terminal | FieldKind::Version | FieldKind::Milestone | FieldKind::Date | FieldKind::Datetime | FieldKind::Labels => {}
    }
    *o = out;
    Ok(())
}

fn date_ok(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

fn datetime_ok(s: &str) -> bool {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok() || chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").is_ok()
}

fn number_ok(o: &FieldOptions, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) if x.is_finite() => o.min.is_none_or(|m| x >= m) && o.max.is_none_or(|m| x <= m),
        _ => false,
    }
}

/// Passt ein einzelner Wert zum Feld? Bei Mehrfachwerten (Mehrfachauswahl, Labels) gilt das für ein Element.
fn single_ok(kind: FieldKind, o: &FieldOptions, v: &Value, refs_ok: bool) -> bool {
    match kind {
        FieldKind::Text => v.as_str().is_some_and(|s| o.max_length.is_none_or(|m| s.chars().count() <= m as usize)),
        FieldKind::Rich => v.is_string(),
        FieldKind::Checkbox => v.is_boolean(),
        FieldKind::Dropdown | FieldKind::Multi => v.as_i64().is_some_and(|i| o.values.iter().any(|x| x.id == i)),
        FieldKind::Labels => v.as_str().is_some_and(|s| !s.trim().is_empty()),
        FieldKind::Number => number_ok(o, v),
        FieldKind::Date => v.as_str().is_some_and(date_ok),
        FieldKind::Datetime => v.as_str().is_some_and(datetime_ok),
        FieldKind::Ticket | FieldKind::Version | FieldKind::Milestone => refs_ok && v.as_i64().is_some(),
        FieldKind::Terminal => refs_ok && v.is_string(),
    }
}

/// Passt der Wert zum Feldtyp und seinen Optionen? `refs_ok`: Verweise (Ticket, Terminal, Version, Meilenstein) sind
/// erlaubt (für Werte in Übergangsregeln); für Standardwerte `false`.
pub fn check_value(kind: FieldKind, o: &FieldOptions, v: &Value, refs_ok: bool) -> Res<()> {
    let ok = match kind {
        FieldKind::Multi | FieldKind::Labels => match v.as_array() {
            Some(a) => {
                let mut dedup: Vec<&Value> = Vec::new();
                for x in a {
                    if dedup.contains(&x) {
                        return Err(PlanError::ValueInvalid);
                    }
                    dedup.push(x);
                }
                a.iter().all(|x| single_ok(kind, o, x, refs_ok))
            }
            None => false,
        },
        _ => single_ok(kind, o, v, refs_ok),
    };
    if ok { Ok(()) } else { Err(PlanError::ValueInvalid) }
}

/// Wie [`check_value`], aber für den Vergleichswert einer Bedingung: bei Mehrfachwerten genügt ein Element.
pub fn check_compare_value(kind: FieldKind, o: &FieldOptions, v: &Value) -> Res<()> {
    if single_ok(kind, o, v, true) { Ok(()) } else { Err(PlanError::ValueInvalid) }
}

fn check_default(kind: FieldKind, o: &FieldOptions, v: &Option<Value>) -> Res<()> {
    match v {
        None => Ok(()),
        Some(v) => {
            if kind.is_reference() {
                return Err(PlanError::ValueInvalid);
            }
            check_value(kind, o, v, false)
        }
    }
}

fn clean_text(s: &str, max: usize) -> Res<String> {
    let t = s.trim().to_string();
    if t.chars().count() > max { Err(PlanError::NameTooLong { max }) } else { Ok(t) }
}

// ------------------------------------------------------------------ Anlegen, ändern, löschen

pub fn create_field(conn: &Connection, n: &NewField) -> Res<i64> {
    let t = editable_type(conn, n.type_id)?;
    if let Some(k) = &n.key {
        validate_ident(k)?;
    }
    let name = match &n.name {
        Some(s) if !s.trim().is_empty() => Some(clean_name(s)?),
        _ => None,
    };
    if n.key.is_none() && name.is_none() {
        return Err(PlanError::NameEmpty);
    }
    let mut options = n.options.clone();
    validate_options(conn, t.project_id, n.kind, &mut options)?;
    check_default(n.kind, &options, &n.default_value)?;
    let help = clean_text(&n.help, MAX_HELP_CHARS)?;
    let section = clean_text(&n.section, MAX_NAME_CHARS)?;
    atomic(conn, || {
        ensure_name_free(conn, "fields", "type_id = ?1", &[n.type_id], n.key.as_deref(), name.as_deref(), None)?;
        let pos = common::next_position(conn, "fields", "type_id = ?1", &[n.type_id])?;
        conn.execute(
            "INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                n.type_id,
                n.key,
                name,
                n.kind.as_str(),
                options_json(&options),
                bool_i(n.required),
                n.default_value.as_ref().map(|v| v.to_string()),
                help,
                section,
                pos,
                bool_i(n.show_on_card),
                bool_i(n.builtin)
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_field(conn: &Connection, id: i64, u: &UpdateField) -> Res<()> {
    let cur = get_field(conn, id)?;
    let t = editable_type(conn, cur.type_id)?;
    let name = match &u.name {
        Some(s) => rename_value(cur.key.as_deref(), s)?,
        None => cur.name.clone(),
    };
    let mut options = u.options.clone().unwrap_or_else(|| cur.options.clone());
    validate_options(conn, t.project_id, cur.kind, &mut options)?;
    let default = match &u.default_value {
        Some(d) => d.clone(),
        None => cur.default_value.clone(),
    };
    check_default(cur.kind, &options, &default)?;
    let help = match &u.help {
        Some(h) => clean_text(h, MAX_HELP_CHARS)?,
        None => cur.help.clone(),
    };
    let section = match &u.section {
        Some(s) => clean_text(s, MAX_NAME_CHARS)?,
        None => cur.section.clone(),
    };
    // Entfernte Dropdown-Werte dürfen nicht in Benutzung sein.
    let mut used = 0;
    if cur.kind.has_values() {
        for old in cur.options.values.iter().filter(|o| !options.values.iter().any(|n| n.id == o.id)) {
            used += integrity::usage_count_option(conn, id, old.id)?;
        }
    }
    if used > 0 {
        return Err(PlanError::InUse { count: used });
    }
    atomic(conn, || {
        ensure_name_free(conn, "fields", "type_id = ?1", &[cur.type_id], cur.key.as_deref(), name.as_deref(), Some(id))?;
        conn.execute(
            "UPDATE fields SET name = ?1, options = ?2, required = ?3, default_value = ?4, help = ?5, section = ?6, show_on_card = ?7 WHERE id = ?8",
            params![name, options_json(&options), bool_i(u.required.unwrap_or(cur.required)), default.as_ref().map(|v| v.to_string()), help, section, bool_i(u.show_on_card.unwrap_or(cur.show_on_card)), id],
        )?;
        Ok(())
    })
}

/// Abschnitt (Gruppe) eines Feldes setzen; leer = ohne Abschnitt.
pub fn set_section(conn: &Connection, id: i64, section: &str) -> Res<()> {
    update_field(conn, id, &UpdateField { section: Some(section.to_string()), ..Default::default() })
}

pub fn reorder_fields(conn: &Connection, type_id: i64, ids: &[i64]) -> Res<()> {
    editable_type(conn, type_id)?;
    common::reorder(conn, "fields", "type_id = ?1", &[type_id], ids)
}

/// Wie oft das Feld mit einem Wert belegt ist (bis zum Ticket-Schritt 0).
pub fn usage(conn: &Connection, id: i64) -> Res<i64> {
    get_field(conn, id)?;
    integrity::usage_count_field(conn, id)
}

/// Löscht ein Feld. Ist es belegt, ist ein Modus nötig: `Migrate { to }` überträgt die Werte in ein Feld **desselben Typs
/// mit gleichem Feldtyp**, `Discard` verwirft sie. Verweise in Übergangsregeln (Pflichtfeld, Bedingung, Setzen) entfallen.
pub fn delete_field(conn: &Connection, id: i64, mode: Option<DeleteMode>) -> Res<()> {
    let cur = get_field(conn, id)?;
    editable_type(conn, cur.type_id)?;
    let count = integrity::usage_count_field(conn, id)?;
    integrity::require_mode(count, mode.is_some())?;
    let to = match &mode {
        Some(DeleteMode::Migrate { to }) => {
            let t = get_field(conn, *to).map_err(|_| PlanError::ReplacementInvalid)?;
            if t.id == id || t.type_id != cur.type_id || t.kind != cur.kind {
                return Err(PlanError::ReplacementInvalid);
            }
            Some(t.id)
        }
        _ => None,
    };
    atomic(conn, || {
        integrity::migrate_values_field(conn, id, to)?;
        super::workflow::scrub_field_refs(conn, cur.type_id, id)?;
        conn.execute("DELETE FROM fields WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// Projekt, zu dem das Feld gehört (Hilfe für Aufrufer).
pub fn project_of(conn: &Connection, field_id: i64) -> Res<projects::Project> {
    let f = get_field(conn, field_id)?;
    let t = super::types::get_type(conn, f.type_id)?;
    projects::get_project(conn, t.project_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::testutil::db;
    use crate::planning::types::{create_type, NewType};
    use serde_json::json;

    fn setup(c: &Connection) -> (i64, i64) {
        let p = create_project(c, &NewProject::new("AA", "P", "#112233")).unwrap();
        let t = create_type(c, &NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        (p, t)
    }

    fn opts(values: &[&str]) -> FieldOptions {
        FieldOptions { values: values.iter().map(|n| OptionValue::new(n)).collect(), ..Default::default() }
    }

    #[test]
    fn kinds_roundtrip_through_text() {
        assert_eq!(FieldKind::ALL.len(), 13);
        for k in FieldKind::ALL {
            assert_eq!(FieldKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{}\"", k.as_str()));
        }
        assert_eq!(FieldKind::parse("nope"), None);
    }

    #[test]
    fn dropdown_values_get_ids_and_must_be_unique() {
        let c = db();
        let (p, t) = setup(&c);
        let mut o = opts(&["Low", "High"]);
        o.values[1].color = Some("#ff0000".into());
        validate_options(&c, p, FieldKind::Dropdown, &mut o).unwrap();
        assert_eq!(o.values.iter().map(|v| v.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(o.values[1].color.as_deref(), Some("#FF0000"));
        // vorhandene Kennungen bleiben, neue werden hinter der größten vergeben
        let mut o2 = o.clone();
        o2.values.remove(0);
        o2.values.push(OptionValue::new("Mid"));
        validate_options(&c, p, FieldKind::Dropdown, &mut o2).unwrap();
        assert_eq!(o2.values.iter().map(|v| v.id).collect::<Vec<_>>(), vec![2, 3]);
        let check = |vals: Vec<OptionValue>| {
            let mut o = FieldOptions { values: vals, ..Default::default() };
            validate_options(&c, p, FieldKind::Dropdown, &mut o)
        };
        assert_eq!(check(vec![OptionValue::new("A"), OptionValue::new("a")]), Err(PlanError::Options(OptionsProblem::DuplicateValue)));
        assert_eq!(check(vec![OptionValue::new("A"), OptionValue::new(" A ")]), Err(PlanError::Options(OptionsProblem::DuplicateValue)));
        assert_eq!(check(vec![OptionValue::default()]), Err(PlanError::Options(OptionsProblem::EmptyValueName)));
        assert_eq!(check(vec![OptionValue::new("  ")]), Err(PlanError::Options(OptionsProblem::EmptyValueName)));
        assert_eq!(check(vec![OptionValue { color: Some("rot".into()), ..OptionValue::new("A") }]), Err(PlanError::Options(OptionsProblem::ValueColor)));
        assert_eq!(check(vec![OptionValue { id: 5, ..OptionValue::new("A") }, OptionValue { id: 5, ..OptionValue::new("B") }]), Err(PlanError::Options(OptionsProblem::DuplicateId)));
        // Standardwert „Medium“ kollidiert mit eigenem Namen „Normal“ in der anderen Sprache
        let key_val = OptionValue { key: Some("priority.medium".into()), ..Default::default() };
        assert_eq!(check(vec![key_val.clone(), OptionValue::new("Normal")]), Err(PlanError::Options(OptionsProblem::DuplicateValue)));
        assert_eq!(check(vec![key_val, OptionValue::new("Medium")]), Err(PlanError::Options(OptionsProblem::DuplicateValue)));
        // Leere Liste ist erlaubt
        assert!(check(vec![]).is_ok());
        let _ = t;
    }

    #[test]
    fn number_options_are_validated() {
        let c = db();
        let (p, _) = setup(&c);
        let run = |o: FieldOptions| {
            let mut o = o;
            validate_options(&c, p, FieldKind::Number, &mut o).map(|_| o)
        };
        assert!(run(FieldOptions { min: Some(0.0), max: Some(10.0), decimals: Some(2), unit: Some(" h ".into()), ..Default::default() }).is_ok());
        assert_eq!(run(FieldOptions { unit: Some(" h ".into()), ..Default::default() }).unwrap().unit.as_deref(), Some("h"));
        assert!(run(FieldOptions { min: Some(5.0), max: Some(5.0), ..Default::default() }).is_ok());
        assert_eq!(run(FieldOptions { min: Some(6.0), max: Some(5.0), ..Default::default() }), Err(PlanError::Options(OptionsProblem::MinGreaterMax)));
        assert_eq!(run(FieldOptions { min: Some(f64::NAN), ..Default::default() }), Err(PlanError::Options(OptionsProblem::MinGreaterMax)));
        assert_eq!(run(FieldOptions { decimals: Some(7), ..Default::default() }), Err(PlanError::Options(OptionsProblem::Decimals)));
        assert_eq!(run(FieldOptions { unit: Some("x".repeat(17)), ..Default::default() }), Err(PlanError::Options(OptionsProblem::Unit)));
        assert_eq!(run(FieldOptions { unit: Some("  ".into()), ..Default::default() }).unwrap().unit, None);
    }

    #[test]
    fn text_rich_and_irrelevant_options() {
        let c = db();
        let (p, _) = setup(&c);
        let mut o = FieldOptions { max_length: Some(0), ..Default::default() };
        assert_eq!(validate_options(&c, p, FieldKind::Text, &mut o), Err(PlanError::Options(OptionsProblem::TextLength)));
        let mut o = FieldOptions { max_length: Some(200_000), ..Default::default() };
        assert_eq!(validate_options(&c, p, FieldKind::Text, &mut o), Err(PlanError::Options(OptionsProblem::TextLength)));
        let mut o = FieldOptions { rows: Some(0), ..Default::default() };
        assert_eq!(validate_options(&c, p, FieldKind::Rich, &mut o), Err(PlanError::Options(OptionsProblem::Rows)));
        let mut o = FieldOptions { rows: Some(51), ..Default::default() };
        assert_eq!(validate_options(&c, p, FieldKind::Rich, &mut o), Err(PlanError::Options(OptionsProblem::Rows)));
        let mut o = FieldOptions { rows: Some(8), ..Default::default() };
        validate_options(&c, p, FieldKind::Rich, &mut o).unwrap();
        assert_eq!(o.rows, Some(8));
        // Nicht Passendes wird verworfen
        let mut o = FieldOptions { rows: Some(8), min: Some(1.0), max_length: Some(5), values: vec![OptionValue::new("x")], target_types: vec![1], ..Default::default() };
        validate_options(&c, p, FieldKind::Checkbox, &mut o).unwrap();
        assert_eq!(o, FieldOptions::default());
        let mut o = FieldOptions { min: Some(1.0), rows: Some(3), ..Default::default() };
        validate_options(&c, p, FieldKind::Text, &mut o).unwrap();
        assert_eq!(o, FieldOptions::default());
    }

    #[test]
    fn ticket_targets_must_exist_in_the_project() {
        let c = db();
        let (p, t) = setup(&c);
        let q = create_project(&c, &NewProject::new("BB", "Q", "#112233")).unwrap();
        let other = create_type(&c, &NewType { project_id: q, key: None, name: Some("O".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let mut o = FieldOptions { target_types: vec![t, t], ..Default::default() };
        validate_options(&c, p, FieldKind::Ticket, &mut o).unwrap();
        assert_eq!(o.target_types, vec![t], "doppelte entfallen");
        let mut o = FieldOptions { target_types: vec![t, other], ..Default::default() };
        assert_eq!(validate_options(&c, p, FieldKind::Ticket, &mut o), Err(PlanError::Options(OptionsProblem::TargetType)));
        let mut o = FieldOptions { target_types: vec![9999], ..Default::default() };
        assert_eq!(validate_options(&c, p, FieldKind::Ticket, &mut o), Err(PlanError::Options(OptionsProblem::TargetType)));
        let mut o = FieldOptions::default();
        validate_options(&c, p, FieldKind::Ticket, &mut o).unwrap(); // leer = jeder Typ
    }

    #[test]
    fn checks_values_per_kind() {
        let dd = FieldOptions { values: vec![OptionValue { id: 1, ..OptionValue::new("a") }, OptionValue { id: 2, ..OptionValue::new("b") }], ..Default::default() };
        let num = FieldOptions { min: Some(0.0), max: Some(10.0), ..Default::default() };
        let txt = FieldOptions { max_length: Some(3), ..Default::default() };
        let ok = |k, o: &FieldOptions, v: Value| check_value(k, o, &v, false).is_ok();
        let e = FieldOptions::default();
        assert!(ok(FieldKind::Text, &txt, json!("abc")) && !ok(FieldKind::Text, &txt, json!("abcd")) && !ok(FieldKind::Text, &txt, json!(1)));
        assert!(ok(FieldKind::Rich, &e, json!("# x")) && !ok(FieldKind::Rich, &e, json!(true)));
        assert!(ok(FieldKind::Checkbox, &e, json!(false)) && !ok(FieldKind::Checkbox, &e, json!("true")));
        assert!(ok(FieldKind::Dropdown, &dd, json!(2)) && !ok(FieldKind::Dropdown, &dd, json!(3)) && !ok(FieldKind::Dropdown, &dd, json!("a")));
        assert!(ok(FieldKind::Multi, &dd, json!([1, 2])) && ok(FieldKind::Multi, &dd, json!([])) && !ok(FieldKind::Multi, &dd, json!([1, 1])) && !ok(FieldKind::Multi, &dd, json!([3])) && !ok(FieldKind::Multi, &dd, json!(1)));
        assert!(ok(FieldKind::Labels, &e, json!(["a", "b"])) && !ok(FieldKind::Labels, &e, json!([""])) && !ok(FieldKind::Labels, &e, json!("a")));
        assert!(ok(FieldKind::Number, &num, json!(5)) && ok(FieldKind::Number, &num, json!(0.5)) && !ok(FieldKind::Number, &num, json!(11)) && !ok(FieldKind::Number, &num, json!(-1)) && !ok(FieldKind::Number, &num, json!("5")));
        assert!(ok(FieldKind::Date, &e, json!("2026-02-28")) && !ok(FieldKind::Date, &e, json!("2026-02-30")) && !ok(FieldKind::Date, &e, json!("28.02.2026")));
        assert!(ok(FieldKind::Datetime, &e, json!("2026-02-28T13:05")) && ok(FieldKind::Datetime, &e, json!("2026-02-28T13:05:09")) && !ok(FieldKind::Datetime, &e, json!("2026-02-28")));
        // Verweise: nur mit refs_ok
        for (k, v) in [(FieldKind::Ticket, json!(5)), (FieldKind::Version, json!(1)), (FieldKind::Milestone, json!(1)), (FieldKind::Terminal, json!("sess"))] {
            assert!(check_value(k, &e, &v, false).is_err());
            assert!(check_value(k, &e, &v, true).is_ok());
        }
        assert!(check_compare_value(FieldKind::Multi, &dd, &json!(2)).is_ok());
        assert!(check_compare_value(FieldKind::Multi, &dd, &json!(7)).is_err());
    }

    #[test]
    fn creates_and_updates_fields() {
        let c = db();
        let (_p, t) = setup(&c);
        let mut n = NewField::new(t, "Priorität", FieldKind::Dropdown);
        n.options = opts(&["Low", "High"]);
        n.default_value = Some(json!(1));
        n.required = true;
        n.help = "  Wie wichtig?  ".into();
        n.section = " Allgemein ".into();
        n.show_on_card = true;
        let id = create_field(&c, &n).unwrap();
        let f = get_field(&c, id).unwrap();
        assert_eq!((f.kind, f.required, f.show_on_card, f.builtin), (FieldKind::Dropdown, true, true, false));
        assert_eq!((f.help.as_str(), f.section.as_str()), ("Wie wichtig?", "Allgemein"));
        assert_eq!(f.default_value, Some(json!(1)));
        assert_eq!(f.options.values.len(), 2);
        // ändern
        update_field(&c, id, &UpdateField { name: Some("Wichtigkeit".into()), required: Some(false), help: Some("".into()), show_on_card: Some(false), default_value: Some(Some(json!(2))), ..Default::default() }).unwrap();
        let f = get_field(&c, id).unwrap();
        assert_eq!((f.display(Lang::En).as_str(), f.required, f.show_on_card, f.help.as_str(), f.default_value.clone()), ("Wichtigkeit", false, false, "", Some(json!(2))));
        set_section(&c, id, "Details").unwrap();
        assert_eq!(get_field(&c, id).unwrap().section, "Details");
        set_section(&c, id, "").unwrap();
        assert_eq!(get_field(&c, id).unwrap().section, "");
        update_field(&c, id, &UpdateField { default_value: Some(None), ..Default::default() }).unwrap();
        assert_eq!(get_field(&c, id).unwrap().default_value, None);
        // Standardwert muss zu den (neuen) Optionen passen
        let mut o = get_field(&c, id).unwrap().options;
        update_field(&c, id, &UpdateField { default_value: Some(Some(json!(2))), ..Default::default() }).unwrap();
        o.values.retain(|v| v.id != 2);
        assert_eq!(update_field(&c, id, &UpdateField { options: Some(o), ..Default::default() }), Err(PlanError::ValueInvalid), "der Standardwert zeigt auf einen entfernten Wert");
        assert_eq!(get_field(&c, id).unwrap().options.values.len(), 2, "nichts verändert");
    }

    #[test]
    fn rejects_bad_fields() {
        let c = db();
        let (_p, t) = setup(&c);
        let ok = create_field(&c, &NewField::new(t, "Titel", FieldKind::Text)).unwrap();
        assert_eq!(create_field(&c, &NewField::new(t, "titel", FieldKind::Number)), Err(PlanError::NameTaken));
        assert_eq!(create_field(&c, &NewField::new(t, " ", FieldKind::Text)), Err(PlanError::NameEmpty));
        let mut n = NewField::new(t, "X", FieldKind::Number);
        n.default_value = Some(json!("x"));
        assert_eq!(create_field(&c, &n), Err(PlanError::ValueInvalid));
        let mut n = NewField::new(t, "X", FieldKind::Number);
        n.options.min = Some(1.0);
        n.default_value = Some(json!(0));
        assert_eq!(create_field(&c, &n), Err(PlanError::ValueInvalid), "Standardwert unter dem Minimum");
        let mut n = NewField::new(t, "X", FieldKind::Ticket);
        n.default_value = Some(json!(3));
        assert_eq!(create_field(&c, &n), Err(PlanError::ValueInvalid), "Verweisfelder haben keinen Standardwert");
        let mut n = NewField::new(t, "X", FieldKind::Number);
        n.options.min = Some(2.0);
        n.options.max = Some(1.0);
        assert_eq!(create_field(&c, &n), Err(PlanError::Options(OptionsProblem::MinGreaterMax)));
        let mut n = NewField::new(t, "X", FieldKind::Text);
        n.help = "x".repeat(1001);
        assert!(matches!(create_field(&c, &n), Err(PlanError::NameTooLong { .. })));
        assert_eq!(create_field(&c, &NewField::new(999, "X", FieldKind::Text)), Err(PlanError::NotFound));
        assert_eq!(list_fields(&c, t).unwrap().len(), 1, "nur das gültige Feld existiert");
        // Schlüssel eindeutig im Typ, Standardname bleibt übersetzbar
        let mut k = NewField::new(t, "", FieldKind::Rich);
        k.name = None;
        k.key = Some("description".into());
        let kid = create_field(&c, &k).unwrap();
        assert_eq!(get_field(&c, kid).unwrap().display(Lang::De), "Beschreibung");
        assert_eq!(create_field(&c, &k), Err(PlanError::NameTaken));
        assert_eq!(update_field(&c, ok, &UpdateField { name: Some("Beschreibung".into()), ..Default::default() }), Err(PlanError::NameTaken), "auch in der anderen Sprache");
        // der Feldtyp ändert sich beim Aktualisieren nie
        update_field(&c, ok, &UpdateField { options: Some(opts(&["x"])), ..Default::default() }).unwrap();
        assert_eq!(get_field(&c, ok).unwrap().options, FieldOptions::default(), "Text-Feld behält nur passende Optionen");
        // System-Typ: keine Felder
        let todo_type = list_types_of_todo(&c);
        assert_eq!(create_field(&c, &NewField::new(todo_type, "X", FieldKind::Text)), Err(PlanError::SystemProtected));
    }

    fn list_types_of_todo(c: &Connection) -> i64 {
        let todo = crate::planning::projects::project_by_key(c, "TODO").unwrap();
        crate::planning::types::list_types(c, todo.id).unwrap().remove(0).id
    }

    #[test]
    fn orders_fields() {
        let c = db();
        let (_p, t) = setup(&c);
        let a = create_field(&c, &NewField::new(t, "A", FieldKind::Text)).unwrap();
        let b = create_field(&c, &NewField::new(t, "B", FieldKind::Text)).unwrap();
        let d = create_field(&c, &NewField::new(t, "C", FieldKind::Text)).unwrap();
        assert_eq!(list_fields(&c, t).unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![a, b, d]);
        reorder_fields(&c, t, &[d, a, b]).unwrap();
        assert_eq!(list_fields(&c, t).unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![d, a, b]);
        assert_eq!(reorder_fields(&c, t, &[a, b]), Err(PlanError::ReorderMismatch));
        assert_eq!(reorder_fields(&c, t, &[a, b, 999]), Err(PlanError::ReorderMismatch));
    }

    #[test]
    fn deleting_fields_with_modes() {
        let c = db();
        let (_p, t) = setup(&c);
        let u = create_type(&c, &NewType { project_id: _p, key: None, name: Some("U".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let a = create_field(&c, &NewField::new(t, "A", FieldKind::Text)).unwrap();
        let b = create_field(&c, &NewField::new(t, "B", FieldKind::Text)).unwrap();
        let n = create_field(&c, &NewField::new(t, "N", FieldKind::Number)).unwrap();
        let other = create_field(&c, &NewField::new(u, "O", FieldKind::Text)).unwrap();
        assert_eq!(usage(&c, a).unwrap(), 0);
        assert_eq!(delete_field(&c, a, Some(DeleteMode::Migrate { to: a })), Err(PlanError::ReplacementInvalid));
        assert_eq!(delete_field(&c, a, Some(DeleteMode::Migrate { to: n })), Err(PlanError::ReplacementInvalid), "anderer Feldtyp");
        assert_eq!(delete_field(&c, a, Some(DeleteMode::Migrate { to: other })), Err(PlanError::ReplacementInvalid), "anderer Typ");
        assert_eq!(delete_field(&c, a, Some(DeleteMode::Migrate { to: 999 })), Err(PlanError::ReplacementInvalid));
        assert!(get_field(&c, a).is_ok());
        delete_field(&c, a, Some(DeleteMode::Migrate { to: b })).unwrap();
        delete_field(&c, b, Some(DeleteMode::Discard)).unwrap();
        delete_field(&c, n, None).unwrap();
        assert_eq!(delete_field(&c, n, None), Err(PlanError::NotFound));
        assert!(list_fields(&c, t).unwrap().is_empty());
        assert_eq!(list_fields(&c, u).unwrap().len(), 1);
    }

    #[test]
    fn removing_used_dropdown_values_hits_the_integrity_hook() {
        // Solange es keine Tickets gibt, ist nichts „in Benutzung“: das Entfernen klappt und vergibt Kennungen nie neu.
        let c = db();
        let (_p, t) = setup(&c);
        let mut n = NewField::new(t, "D", FieldKind::Dropdown);
        n.options = opts(&["a", "b"]);
        let id = create_field(&c, &n).unwrap();
        let mut o = get_field(&c, id).unwrap().options;
        o.values.remove(0);
        o.values.push(OptionValue::new("c"));
        update_field(&c, id, &UpdateField { options: Some(o), ..Default::default() }).unwrap();
        assert_eq!(get_field(&c, id).unwrap().options.values.iter().map(|v| v.id).collect::<Vec<_>>(), vec![2, 3]);
    }
}
