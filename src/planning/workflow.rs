//! Workflow eines Typs: ein/aus, gewählte Status aus dem Projekt-Pool, Anfangsstatus und die Übergangsmatrix mit optionalen
//! Regeln je Übergang (Pflichtfelder, Bedingungen, Werte, die beim Übergang gesetzt werden). Alles ist optional und
//! unabhängig voneinander.
//!
//! * **Ein Typ ohne Workflow** (`ticket_types.workflow = 0`) hat kein Statusfeld: keine `type_statuses`, keine `transitions`.
//! * **Ohne definierte Übergänge** darf ein Ticket frei zwischen den Status seines Workflows wechseln; sobald mindestens ein
//!   Übergang eingetragen ist, gilt nur noch die Matrix ([`is_transition_allowed`]).
//! * **Bedingungen** werden der Reihe nach geprüft: Trifft eine zu, entscheidet ihr `allow`; trifft keine zu, ist der Übergang
//!   erlaubt – außer es gibt mindestens eine „erlauben“-Bedingung, dann ist er gesperrt (die erlaubenden Bedingungen wirken
//!   als Liste). Siehe [`evaluate_conditions`].

use super::common::atomic;
use super::fields::{self, check_compare_value, check_value, FieldKind};
use super::integrity;
use super::statuses::{get_status, list_statuses, StatusCategory};
use super::types::{editable_type, get_type};
use super::{PlanError, Res, RuleProblem};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    In,
    NotIn,
    IsEmpty,
    IsNotEmpty,
}

impl ConditionOp {
    fn is_order(self) -> bool {
        matches!(self, ConditionOp::Gt | ConditionOp::Ge | ConditionOp::Lt | ConditionOp::Le)
    }
}

/// Bedingung eines Übergangs: „wenn Feld `op` Wert, dann erlaubt (`allow`) / gesperrt“.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub field_id: i64,
    pub op: ConditionOp,
    /// Vergleichswert (bei `In`/`NotIn` eine Liste; bei `IsEmpty`/`IsNotEmpty` ignoriert).
    #[serde(default)]
    pub value: Value,
    pub allow: bool,
}

/// Wert, der beim Übergang in ein Feld geschrieben wird.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetValue {
    pub field_id: i64,
    pub value: Value,
}

/// Ein erlaubter Übergang samt Regeln.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TransitionDef {
    pub from_status_id: i64,
    pub to_status_id: i64,
    /// Felder, die beim Übergang ausgefüllt sein müssen (Dialog).
    #[serde(default)]
    pub required_fields: Vec<i64>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub set_values: Vec<SetValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub id: i64,
    pub type_id: i64,
    pub def: TransitionDef,
}

fn json_of<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

pub(crate) fn transition_from_row(r: &rusqlite::Row) -> rusqlite::Result<Transition> {
    let (req, cond, set): (String, String, String) = (r.get(4)?, r.get(5)?, r.get(6)?);
    Ok(Transition {
        id: r.get(0)?,
        type_id: r.get(1)?,
        def: TransitionDef {
            from_status_id: r.get(2)?,
            to_status_id: r.get(3)?,
            required_fields: serde_json::from_str(&req).unwrap_or_default(),
            conditions: serde_json::from_str(&cond).unwrap_or_default(),
            set_values: serde_json::from_str(&set).unwrap_or_default(),
        },
    })
}

// ------------------------------------------------------------------ Workflow ein/aus, Status wählen

/// Statuskennungen des Workflows in Reihenfolge.
pub fn workflow_statuses(conn: &Connection, type_id: i64) -> Res<Vec<i64>> {
    let mut st = conn.prepare("SELECT status_id FROM type_statuses WHERE type_id = ?1 ORDER BY position, status_id")?;
    Ok(st.query_map([type_id], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
}

/// Schaltet den Workflow des Typs ein oder aus. Aus entfernt Status-Zuordnung und Übergänge. Haben Tickets des Typs einen
/// Status, ist `discard_values` nötig (der Status der Tickets wird dann geleert), sonst `InUse`.
pub fn set_workflow(conn: &Connection, type_id: i64, enabled: bool, discard_values: bool) -> Res<()> {
    let t = editable_type(conn, type_id)?;
    if t.workflow == enabled {
        return Ok(());
    }
    if !enabled {
        let used = integrity::usage_count_type_workflow(conn, type_id)?;
        integrity::require_mode(used, discard_values)?;
    }
    atomic(conn, || {
        if !enabled {
            integrity::clear_type_status(conn, type_id)?;
            conn.execute("DELETE FROM transitions WHERE type_id = ?1", [type_id])?;
            conn.execute("DELETE FROM type_statuses WHERE type_id = ?1", [type_id])?;
        }
        conn.execute("UPDATE ticket_types SET workflow = ?1 WHERE id = ?2", [enabled as i64, type_id])?;
        Ok(())
    })
}

/// Wählt die Status des Workflows (Reihenfolge = Reihenfolge der Liste). Die Status müssen zum Projekt gehören; der Typ
/// braucht einen eingeschalteten Workflow. Status, die wegfallen, verlieren ihre Übergänge; hängen noch Tickets des Typs
/// daran, liefert die Funktion `InUse`.
pub fn set_workflow_statuses(conn: &Connection, type_id: i64, status_ids: &[i64]) -> Res<()> {
    let t = editable_type(conn, type_id)?;
    if !t.workflow {
        return Err(PlanError::WorkflowOff);
    }
    let mut seen = HashSet::new();
    for s in status_ids {
        let st = get_status(conn, *s).map_err(|_| PlanError::StatusNotInWorkflow)?;
        if st.project_id != t.project_id || !seen.insert(*s) {
            return Err(PlanError::StatusNotInWorkflow);
        }
    }
    let have = workflow_statuses(conn, type_id)?;
    let mut used = 0;
    for gone in have.iter().filter(|h| !status_ids.contains(h)) {
        used += integrity::usage_count_type_status(conn, type_id, *gone)?;
    }
    if used > 0 {
        return Err(PlanError::InUse { count: used });
    }
    atomic(conn, || {
        for gone in have.iter().filter(|h| !status_ids.contains(h)) {
            integrity::reset_type_status(conn, type_id, *gone)?;
            conn.execute("DELETE FROM transitions WHERE type_id = ?1 AND (from_status_id = ?2 OR to_status_id = ?2)", [type_id, *gone])?;
        }
        conn.execute("DELETE FROM type_statuses WHERE type_id = ?1", [type_id])?;
        for (i, s) in status_ids.iter().enumerate() {
            conn.execute("INSERT INTO type_statuses (type_id, status_id, position) VALUES (?1, ?2, ?3)", [type_id, *s, i as i64])?;
        }
        Ok(())
    })
}

/// Startstatus neuer Tickets: der erste Status der Kategorie To Do im Workflow, sonst der erste Status überhaupt; ohne
/// Workflow `None`.
pub fn default_status(conn: &Connection, type_id: i64) -> Res<Option<i64>> {
    let t = get_type(conn, type_id)?;
    if !t.workflow {
        return Ok(None);
    }
    let ids = workflow_statuses(conn, type_id)?;
    let mut first = None;
    for id in &ids {
        let s = get_status(conn, *id)?;
        if s.category == StatusCategory::Todo {
            return Ok(Some(*id));
        }
        first.get_or_insert(*id);
    }
    Ok(first)
}

// ------------------------------------------------------------------ Übergänge

pub fn list_transitions(conn: &Connection, type_id: i64) -> Res<Vec<Transition>> {
    let mut st = conn.prepare("SELECT id, type_id, from_status_id, to_status_id, required_fields, conditions, set_values FROM transitions WHERE type_id = ?1 ORDER BY from_status_id, to_status_id")?;
    Ok(st.query_map([type_id], transition_from_row)?.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_transition(conn: &Connection, type_id: i64, from: i64, to: i64) -> Res<Option<Transition>> {
    Ok(conn
        .query_row(
            "SELECT id, type_id, from_status_id, to_status_id, required_fields, conditions, set_values FROM transitions WHERE type_id = ?1 AND from_status_id = ?2 AND to_status_id = ?3",
            [type_id, from, to],
            transition_from_row,
        )
        .optional()?)
}

/// Prüft einen Übergang gegen den Typ: gleiche Status verboten, beide im Workflow, Felder des Typs, Werte passend.
fn check_def(conn: &Connection, type_id: i64, d: &TransitionDef) -> Res<()> {
    if d.from_status_id == d.to_status_id {
        return Err(PlanError::TransitionSame);
    }
    let in_wf = workflow_statuses(conn, type_id)?;
    if !in_wf.contains(&d.from_status_id) || !in_wf.contains(&d.to_status_id) {
        return Err(PlanError::StatusNotInWorkflow);
    }
    let flds = fields::list_fields(conn, type_id)?;
    let find = |id: i64| flds.iter().find(|f| f.id == id).ok_or(PlanError::Rule(RuleProblem::FieldNotInType));
    for f in &d.required_fields {
        find(*f)?;
    }
    for c in &d.conditions {
        let f = find(c.field_id)?;
        if c.op.is_order() && !matches!(f.kind, FieldKind::Number | FieldKind::Date | FieldKind::Datetime) {
            return Err(PlanError::Rule(RuleProblem::Operator));
        }
        let bad_value = || PlanError::Rule(RuleProblem::Value);
        match c.op {
            ConditionOp::IsEmpty | ConditionOp::IsNotEmpty => {}
            ConditionOp::In | ConditionOp::NotIn => {
                let list = c.value.as_array().filter(|a| !a.is_empty()).ok_or_else(bad_value)?;
                for v in list {
                    check_compare_value(f.kind, &f.options, v).map_err(|_| bad_value())?;
                }
            }
            _ => check_compare_value(f.kind, &f.options, &c.value).map_err(|_| bad_value())?,
        }
    }
    let mut targets = HashSet::new();
    for s in &d.set_values {
        let f = find(s.field_id)?;
        if !targets.insert(s.field_id) {
            return Err(PlanError::Rule(RuleProblem::Value));
        }
        check_value(f.kind, &f.options, &s.value, true).map_err(|_| PlanError::Rule(RuleProblem::Value))?;
    }
    Ok(())
}

fn dedup(v: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    for x in v {
        if !out.contains(x) {
            out.push(*x);
        }
    }
    out
}

fn write_transition(conn: &Connection, type_id: i64, d: &TransitionDef) -> Res<i64> {
    let req = dedup(&d.required_fields);
    conn.execute(
        "INSERT INTO transitions (type_id, from_status_id, to_status_id, required_fields, conditions, set_values) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (type_id, from_status_id, to_status_id) DO UPDATE SET required_fields = excluded.required_fields, conditions = excluded.conditions, set_values = excluded.set_values",
        params![type_id, d.from_status_id, d.to_status_id, json_of(&req), json_of(&d.conditions), json_of(&d.set_values)],
    )?;
    Ok(conn.query_row("SELECT id FROM transitions WHERE type_id = ?1 AND from_status_id = ?2 AND to_status_id = ?3", [type_id, d.from_status_id, d.to_status_id], |r| r.get(0))?)
}

/// Legt einen erlaubten Übergang an oder ersetzt dessen Regeln.
pub fn set_transition(conn: &Connection, type_id: i64, d: &TransitionDef) -> Res<i64> {
    let t = editable_type(conn, type_id)?;
    if !t.workflow {
        return Err(PlanError::WorkflowOff);
    }
    check_def(conn, type_id, d)?;
    atomic(conn, || write_transition(conn, type_id, d))
}

/// Entfernt einen Übergang (nicht vorhanden: kein Fehler).
pub fn remove_transition(conn: &Connection, type_id: i64, from: i64, to: i64) -> Res<()> {
    editable_type(conn, type_id)?;
    conn.execute("DELETE FROM transitions WHERE type_id = ?1 AND from_status_id = ?2 AND to_status_id = ?3", [type_id, from, to])?;
    Ok(())
}

/// Ersetzt die gesamte Übergangsmatrix des Typs (für den Matrix-Editor und den Vorlagen-Import).
pub fn replace_transitions(conn: &Connection, type_id: i64, defs: &[TransitionDef]) -> Res<()> {
    let t = editable_type(conn, type_id)?;
    if !t.workflow && !defs.is_empty() {
        return Err(PlanError::WorkflowOff);
    }
    for d in defs {
        check_def(conn, type_id, d)?;
    }
    atomic(conn, || {
        conn.execute("DELETE FROM transitions WHERE type_id = ?1", [type_id])?;
        for d in defs {
            write_transition(conn, type_id, d)?;
        }
        Ok(())
    })
}

/// Trägt alle Übergänge zwischen den Status des Workflows ein (jeder darf zu jedem), ohne Regeln – bestehende Regeln bleiben.
pub fn allow_all_transitions(conn: &Connection, type_id: i64) -> Res<()> {
    let t = editable_type(conn, type_id)?;
    if !t.workflow {
        return Err(PlanError::WorkflowOff);
    }
    let ids = workflow_statuses(conn, type_id)?;
    atomic(conn, || {
        for f in &ids {
            for to in &ids {
                if f != to {
                    conn.execute("INSERT OR IGNORE INTO transitions (type_id, from_status_id, to_status_id) VALUES (?1, ?2, ?3)", [type_id, *f, *to])?;
                }
            }
        }
        Ok(())
    })
}

/// Ziel-Status, die von `from` aus erlaubt sind (in Workflow-Reihenfolge). Ohne definierte Übergänge: alle anderen Status des Workflows.
pub fn allowed_targets(conn: &Connection, type_id: i64, from: i64) -> Res<Vec<i64>> {
    let wf = workflow_statuses(conn, type_id)?;
    if !wf.contains(&from) {
        return Ok(Vec::new());
    }
    let tr = list_transitions(conn, type_id)?;
    if tr.is_empty() {
        return Ok(wf.into_iter().filter(|s| *s != from).collect());
    }
    Ok(wf.into_iter().filter(|s| tr.iter().any(|t| t.def.from_status_id == from && t.def.to_status_id == *s)).collect())
}

/// Ist der Statuswechsel laut Matrix erlaubt? (Die Bedingungen des Übergangs prüft [`evaluate_conditions`] mit den Feldwerten
/// des Tickets – das geschieht in `transition.rs`.)
pub fn is_transition_allowed(conn: &Connection, type_id: i64, from: i64, to: i64) -> Res<bool> {
    Ok(allowed_targets(conn, type_id, from)?.contains(&to))
}

// ------------------------------------------------------------------ Bedingungen auswerten

fn is_empty_value(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    }
}

/// Vergleich zweier Zeitangaben wie in der Abfragesprache (Fälligkeit, #214): `actual` ist der Feldwert (`JJJJ-MM-TT` oder
/// `JJJJ-MM-TTTHH:MM[:SS]`), `wanted` der Wert der Bedingung. Ein reines Datum in der Bedingung vergleicht nur den Tag; mit
/// Uhrzeit den Zeitpunkt, wobei ein reines Datum als Feldwert das Ende des Tages (23:59:59) ist. `None`, wenn eines kein Datum ist.
fn date_cmp(actual: &str, wanted: &str) -> Option<std::cmp::Ordering> {
    let (ad, at) = crate::due::parse_stored(actual)?;
    let (wd, wt) = crate::due::parse_stored(wanted)?;
    Some(match wt {
        None => ad.cmp(&wd),
        Some(wt) => {
            let end_of_day = chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(chrono::NaiveTime::MIN);
            ad.and_time(at.unwrap_or(end_of_day)).cmp(&wd.and_time(wt))
        }
    })
}

/// `due` = das Feld der Bedingung ist die eingebaute Fälligkeit: dann gilt [`date_cmp`], sonst der bisherige Vergleich.
fn num_or_str_cmp(a: &Value, b: &Value, due: bool) -> Option<std::cmp::Ordering> {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x.partial_cmp(&y),
        _ => match (a.as_str(), b.as_str()) {
            (Some(x), Some(y)) => Some(due.then(|| date_cmp(x, y)).flatten().unwrap_or_else(|| x.cmp(y))),
            _ => None,
        },
    }
}

fn same(a: &Value, b: &Value, due: bool) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ if due && a.is_string() && b.is_string() => match date_cmp(a.as_str().unwrap_or_default(), b.as_str().unwrap_or_default()) {
            Some(o) => o.is_eq(),
            None => a == b,
        },
        _ => a == b,
    }
}

/// Trifft die Bedingung auf den aktuellen Wert des Feldes zu? Bei Listenwerten (Mehrfachauswahl, Labels) gilt `Eq`/`In` als
/// „enthält“ und `Ne`/`NotIn` als „enthält nicht“. `due` = das Feld ist die eingebaute Fälligkeit (Vergleich wie in der
/// Abfragesprache, #214).
pub fn condition_matches(c: &Condition, actual: Option<&Value>, due: bool) -> bool {
    let contains_any = |wanted: &dyn Fn(&Value) -> bool| match actual {
        Some(Value::Array(a)) => a.iter().any(wanted),
        Some(v) if !v.is_null() => wanted(v),
        _ => false,
    };
    match c.op {
        ConditionOp::IsEmpty => is_empty_value(actual),
        ConditionOp::IsNotEmpty => !is_empty_value(actual),
        ConditionOp::Eq => contains_any(&|v| same(v, &c.value, due)),
        ConditionOp::Ne => !contains_any(&|v| same(v, &c.value, due)),
        ConditionOp::In => {
            let list = c.value.as_array().cloned().unwrap_or_default();
            contains_any(&|v| list.iter().any(|w| same(v, w, due)))
        }
        ConditionOp::NotIn => {
            let list = c.value.as_array().cloned().unwrap_or_default();
            !contains_any(&|v| list.iter().any(|w| same(v, w, due)))
        }
        ConditionOp::Gt | ConditionOp::Ge | ConditionOp::Lt | ConditionOp::Le => match actual.and_then(|a| num_or_str_cmp(a, &c.value, due)) {
            Some(o) => match c.op {
                ConditionOp::Gt => o.is_gt(),
                ConditionOp::Ge => o.is_ge(),
                ConditionOp::Lt => o.is_lt(),
                _ => o.is_le(),
            },
            None => false,
        },
    }
}

/// Entscheidet, ob die Bedingungen eines Übergangs ihn erlauben (Regel siehe Moduldoku). `value_of` liefert den aktuellen
/// Wert eines Feldes des Tickets; `due_field` ist die Kennung der eingebauten Fälligkeit des Typs (falls vorhanden).
pub fn evaluate_conditions(conditions: &[Condition], value_of: &dyn Fn(i64) -> Option<Value>, due_field: Option<i64>) -> bool {
    for c in conditions {
        if condition_matches(c, value_of(c.field_id).as_ref(), due_field == Some(c.field_id)) {
            return c.allow;
        }
    }
    !conditions.iter().any(|c| c.allow)
}

// ------------------------------------------------------------------ Aufräumen und Prüfen

/// Entfernt ein gelöschtes Feld aus allen Übergangsregeln des Typs.
pub(crate) fn scrub_field_refs(conn: &Connection, type_id: i64, field_id: i64) -> Res<()> {
    for t in list_transitions(conn, type_id)? {
        let mut d = t.def.clone();
        d.required_fields.retain(|f| *f != field_id);
        d.conditions.retain(|c| c.field_id != field_id);
        d.set_values.retain(|s| s.field_id != field_id);
        if d != t.def {
            conn.execute("UPDATE transitions SET required_fields = ?1, conditions = ?2, set_values = ?3 WHERE id = ?4", params![json_of(&d.required_fields), json_of(&d.conditions), json_of(&d.set_values), t.id])?;
        }
    }
    Ok(())
}

/// Schreibt Zahlen-Verweise (Option, Version, Meilenstein) in den Bedingungen und Setze-Werten der Übergänge um: `map(id)` liefert
/// `Some(Some(neu))` zum Ersetzen, `Some(None)` zum Entfernen (eine Bedingung oder ein Wert ohne Rest entfällt ganz) und `None`,
/// wenn der Verweis bleibt. `fields` sind die Felder, deren Werte gemeint sind. Ohne das bliebe z. B. eine „erlauben“-Bedingung auf
/// eine nicht mehr vorhandene Option für immer bestehen, und ein Setze-Wert auf eine gelöschte Version ließe jeden Wechsel in den
/// Status mit einem Fehler scheitern.
fn rewrite_value_refs(conn: &Connection, type_id: i64, fields: &HashSet<i64>, map: &dyn Fn(i64) -> Option<Option<i64>>) -> Res<()> {
    let rewrite = |v: &Value| -> Option<Value> {
        match v {
            Value::Array(a) => {
                let rest: Vec<Value> = a
                    .iter()
                    .filter_map(|x| match x.as_i64().and_then(map) {
                        Some(Some(n)) => Some(Value::from(n)),
                        Some(None) => None,
                        None => Some(x.clone()),
                    })
                    .collect();
                if rest.is_empty() { None } else { Some(Value::Array(rest)) }
            }
            other => match other.as_i64().and_then(map) {
                Some(Some(n)) => Some(Value::from(n)),
                Some(None) => None,
                None => Some(other.clone()),
            },
        }
    };
    for t in list_transitions(conn, type_id)? {
        let mut d = t.def.clone();
        d.conditions = d
            .conditions
            .into_iter()
            .filter_map(|mut c| {
                if !fields.contains(&c.field_id) || matches!(c.op, ConditionOp::IsEmpty | ConditionOp::IsNotEmpty) {
                    return Some(c);
                }
                c.value = rewrite(&c.value)?;
                Some(c)
            })
            .collect();
        d.set_values = d
            .set_values
            .into_iter()
            .filter_map(|mut s| {
                if !fields.contains(&s.field_id) {
                    return Some(s);
                }
                s.value = rewrite(&s.value)?;
                Some(s)
            })
            .collect();
        if d != t.def {
            conn.execute("UPDATE transitions SET conditions = ?1, set_values = ?2 WHERE id = ?3", params![json_of(&d.conditions), json_of(&d.set_values), t.id])?;
        }
    }
    Ok(())
}

/// Entfernt gelöschte Dropdown-/Mehrfachauswahl-Optionen (`removed`) des Feldes aus den Übergangsregeln des Typs.
pub(crate) fn scrub_option_refs(conn: &Connection, type_id: i64, field_id: i64, removed: &[i64]) -> Res<()> {
    rewrite_value_refs(conn, type_id, &HashSet::from([field_id]), &|i| removed.contains(&i).then_some(None))
}

/// Eine gelöschte Version bzw. ein gelöschter Meilenstein (`field_kind`: `version` oder `milestone`) verschwindet aus den
/// Übergangsregeln aller Typen oder wird durch den Ersatz `to` ersetzt.
pub(crate) fn scrub_release_refs(conn: &Connection, field_kind: &str, from: i64, to: Option<i64>) -> Res<()> {
    let mut st = conn.prepare("SELECT id, type_id FROM fields WHERE kind = ?1")?;
    let rows: Vec<(i64, i64)> = st.query_map([field_kind], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut by_type: std::collections::BTreeMap<i64, HashSet<i64>> = std::collections::BTreeMap::new();
    for (field, ty) in rows {
        by_type.entry(ty).or_default().insert(field);
    }
    for (ty, fields) in by_type {
        rewrite_value_refs(conn, ty, &fields, &|i| (i == from).then_some(to))?;
    }
    Ok(())
}

/// Was an einem eingeschalteten Workflow noch fehlt („komplett definiert“). Nur Hinweise – gespeichert werden darf immer.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowProblem {
    /// Workflow an, aber kein Status gewählt.
    NoStatuses,
    /// Kein Status der Kategorie To Do (kein sinnvoller Anfangsstatus).
    NoTodoStatus,
    /// Kein Status der Kategorie Done (nichts gilt je als erledigt).
    NoDoneStatus,
    /// Übergänge sind definiert, aber in diesen Status führt keiner hinein (und es ist nicht der Anfangsstatus).
    Unreachable(i64),
}

pub fn workflow_problems(conn: &Connection, type_id: i64) -> Res<Vec<WorkflowProblem>> {
    let t = get_type(conn, type_id)?;
    let mut out = Vec::new();
    if !t.workflow {
        return Ok(out);
    }
    let ids = workflow_statuses(conn, type_id)?;
    if ids.is_empty() {
        out.push(WorkflowProblem::NoStatuses);
        return Ok(out);
    }
    let all = list_statuses(conn, t.project_id)?;
    let cat = |id: i64| all.iter().find(|s| s.id == id).map(|s| s.category);
    if !ids.iter().any(|i| cat(*i) == Some(StatusCategory::Todo)) {
        out.push(WorkflowProblem::NoTodoStatus);
    }
    if !ids.iter().any(|i| cat(*i) == Some(StatusCategory::Done)) {
        out.push(WorkflowProblem::NoDoneStatus);
    }
    let tr = list_transitions(conn, type_id)?;
    if !tr.is_empty() {
        let start = default_status(conn, type_id)?;
        for id in &ids {
            if Some(*id) != start && !tr.iter().any(|x| x.def.to_status_id == *id) {
                out.push(WorkflowProblem::Unreachable(*id));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{create_field, delete_field, FieldOptions, NewField, OptionValue};
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::statuses::{create_status, NewStatus};
    use crate::planning::testutil::db;
    use crate::planning::types::{create_type, NewType};
    use serde_json::json;

    struct W {
        p: i64,
        t: i64,
        a: i64,
        b: i64,
        d: i64,
    }

    fn setup(c: &Connection) -> W {
        let p = create_project(c, &NewProject::new("AA", "P", "#112233")).unwrap();
        let mk = |n: &str, cat| create_status(c, &NewStatus { project_id: p, key: None, name: Some(n.into()), color: "#445566".into(), category: cat }).unwrap();
        let a = mk("A", StatusCategory::Todo);
        let b = mk("B", StatusCategory::Progress);
        let d = mk("C", StatusCategory::Done);
        let t = create_type(c, &NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#445566".into(), workflow: true }).unwrap();
        W { p, t, a, b, d }
    }

    fn def(from: i64, to: i64) -> TransitionDef {
        TransitionDef { from_status_id: from, to_status_id: to, ..Default::default() }
    }

    #[test]
    fn workflow_on_off_and_status_selection() {
        let c = db();
        let w = setup(&c);
        assert!(workflow_statuses(&c, w.t).unwrap().is_empty());
        set_workflow_statuses(&c, w.t, &[w.d, w.a]).unwrap();
        assert_eq!(workflow_statuses(&c, w.t).unwrap(), vec![w.d, w.a], "Reihenfolge der Liste");
        // Status eines anderen Projekts / doppelt / unbekannt
        let q = create_project(&c, &NewProject::new("BB", "Q", "#112233")).unwrap();
        let foreign = create_status(&c, &NewStatus { project_id: q, key: None, name: Some("F".into()), color: "#445566".into(), category: StatusCategory::Todo }).unwrap();
        assert_eq!(set_workflow_statuses(&c, w.t, &[w.a, foreign]), Err(PlanError::StatusNotInWorkflow));
        assert_eq!(set_workflow_statuses(&c, w.t, &[w.a, w.a]), Err(PlanError::StatusNotInWorkflow));
        assert_eq!(set_workflow_statuses(&c, w.t, &[999]), Err(PlanError::StatusNotInWorkflow));
        assert_eq!(workflow_statuses(&c, w.t).unwrap(), vec![w.d, w.a], "Fehler ändern nichts");
        // abschalten räumt auf
        set_workflow_statuses(&c, w.t, &[w.a, w.b, w.d]).unwrap();
        set_transition(&c, w.t, &def(w.a, w.b)).unwrap();
        set_workflow(&c, w.t, false, false).unwrap();
        assert!(!get_type(&c, w.t).unwrap().workflow);
        assert!(workflow_statuses(&c, w.t).unwrap().is_empty());
        assert!(list_transitions(&c, w.t).unwrap().is_empty());
        assert_eq!(default_status(&c, w.t).unwrap(), None);
        assert_eq!(set_workflow_statuses(&c, w.t, &[w.a]), Err(PlanError::WorkflowOff));
        assert_eq!(set_transition(&c, w.t, &def(w.a, w.b)), Err(PlanError::WorkflowOff));
        assert_eq!(replace_transitions(&c, w.t, &[def(w.a, w.b)]), Err(PlanError::WorkflowOff));
        replace_transitions(&c, w.t, &[]).unwrap();
        assert!(workflow_problems(&c, w.t).unwrap().is_empty(), "ohne Workflow nichts zu bemängeln");
        // wieder an: leer
        set_workflow(&c, w.t, true, false).unwrap();
        assert!(workflow_statuses(&c, w.t).unwrap().is_empty());
        set_workflow(&c, w.t, true, false).unwrap(); // idempotent
    }

    #[test]
    fn removing_a_status_from_the_workflow_drops_its_transitions() {
        let c = db();
        let w = setup(&c);
        set_workflow_statuses(&c, w.t, &[w.a, w.b, w.d]).unwrap();
        replace_transitions(&c, w.t, &[def(w.a, w.b), def(w.b, w.d), def(w.a, w.d)]).unwrap();
        set_workflow_statuses(&c, w.t, &[w.a, w.d]).unwrap();
        let tr = list_transitions(&c, w.t).unwrap();
        assert_eq!(tr.len(), 1);
        assert_eq!((tr[0].def.from_status_id, tr[0].def.to_status_id), (w.a, w.d));
        // Der Status selbst bleibt im Pool.
        assert!(crate::planning::statuses::get_status(&c, w.b).is_ok());
    }

    #[test]
    fn default_status_is_the_first_todo_status() {
        let c = db();
        let w = setup(&c);
        assert_eq!(default_status(&c, w.t).unwrap(), None, "kein Status gewählt");
        set_workflow_statuses(&c, w.t, &[w.b, w.d, w.a]).unwrap();
        assert_eq!(default_status(&c, w.t).unwrap(), Some(w.a), "erster To-Do-Status, nicht der erste überhaupt");
        set_workflow_statuses(&c, w.t, &[w.b, w.d]).unwrap();
        assert_eq!(default_status(&c, w.t).unwrap(), Some(w.b), "ohne To-Do: der erste");
    }

    #[test]
    fn transitions_matrix_rules() {
        let c = db();
        let w = setup(&c);
        set_workflow_statuses(&c, w.t, &[w.a, w.b, w.d]).unwrap();
        // ohne definierte Übergänge: frei
        assert_eq!(allowed_targets(&c, w.t, w.a).unwrap(), vec![w.b, w.d]);
        assert!(is_transition_allowed(&c, w.t, w.d, w.a).unwrap());
        assert!(!is_transition_allowed(&c, w.t, w.a, w.a).unwrap());
        // definiert: nur noch die Matrix
        set_transition(&c, w.t, &def(w.a, w.b)).unwrap();
        set_transition(&c, w.t, &def(w.b, w.d)).unwrap();
        assert_eq!(allowed_targets(&c, w.t, w.a).unwrap(), vec![w.b]);
        assert!(!is_transition_allowed(&c, w.t, w.a, w.d).unwrap());
        assert!(allowed_targets(&c, w.t, w.d).unwrap().is_empty());
        assert!(allowed_targets(&c, w.t, 999).unwrap().is_empty(), "Status nicht im Workflow");
        // Upsert statt Doppelte
        let id1 = set_transition(&c, w.t, &def(w.a, w.b)).unwrap();
        let id2 = set_transition(&c, w.t, &TransitionDef { required_fields: vec![], ..def(w.a, w.b) }).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(list_transitions(&c, w.t).unwrap().len(), 2);
        // Fehler
        assert_eq!(set_transition(&c, w.t, &def(w.a, w.a)), Err(PlanError::TransitionSame));
        let q = create_project(&c, &NewProject::new("BB", "Q", "#112233")).unwrap();
        let foreign = create_status(&c, &NewStatus { project_id: q, key: None, name: Some("F".into()), color: "#445566".into(), category: StatusCategory::Todo }).unwrap();
        assert_eq!(set_transition(&c, w.t, &def(w.a, foreign)), Err(PlanError::StatusNotInWorkflow));
        set_workflow_statuses(&c, w.t, &[w.a, w.b]).unwrap();
        assert_eq!(set_transition(&c, w.t, &def(w.a, w.d)), Err(PlanError::StatusNotInWorkflow), "Status nicht im Workflow dieses Typs");
        // entfernen, auch Unbekanntes
        remove_transition(&c, w.t, w.a, w.b).unwrap();
        remove_transition(&c, w.t, w.a, w.b).unwrap();
        assert!(list_transitions(&c, w.t).unwrap().is_empty(), "b->d fiel mit dem Status weg");
        // alle erlauben
        allow_all_transitions(&c, w.t).unwrap();
        assert_eq!(list_transitions(&c, w.t).unwrap().len(), 2);
        allow_all_transitions(&c, w.t).unwrap();
        assert_eq!(list_transitions(&c, w.t).unwrap().len(), 2);
        // replace ist atomar: ein ungültiger Eintrag lässt alles unverändert
        assert_eq!(replace_transitions(&c, w.t, &[def(w.a, w.b), def(w.b, w.b)]), Err(PlanError::TransitionSame));
        assert_eq!(list_transitions(&c, w.t).unwrap().len(), 2);
        replace_transitions(&c, w.t, &[def(w.a, w.b)]).unwrap();
        assert_eq!(list_transitions(&c, w.t).unwrap().len(), 1);
    }

    #[test]
    fn transition_rules_are_validated_and_optional_independently() {
        let c = db();
        let w = setup(&c);
        set_workflow_statuses(&c, w.t, &[w.a, w.b, w.d]).unwrap();
        let mut n = NewField::new(w.t, "Zahl", FieldKind::Number);
        n.options.min = Some(0.0);
        let num = create_field(&c, &n).unwrap();
        let text = create_field(&c, &NewField::new(w.t, "Text", FieldKind::Text)).unwrap();
        let mut dd = NewField::new(w.t, "Wahl", FieldKind::Dropdown);
        dd.options = FieldOptions { values: vec![OptionValue::new("x"), OptionValue::new("y")], ..Default::default() };
        let dd = create_field(&c, &dd).unwrap();
        let other_type = create_type(&c, &NewType { project_id: w.p, key: None, name: Some("U".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let foreign = create_field(&c, &NewField::new(other_type, "F", FieldKind::Text)).unwrap();
        // nur Pflichtfelder
        set_transition(&c, w.t, &TransitionDef { required_fields: vec![text, text, num], ..def(w.a, w.b) }).unwrap();
        assert_eq!(get_transition(&c, w.t, w.a, w.b).unwrap().unwrap().def.required_fields, vec![text, num], "doppelte entfallen");
        // nur Bedingungen
        let cond = |field_id, op, value: Value, allow| Condition { field_id, op, value, allow };
        set_transition(&c, w.t, &TransitionDef { conditions: vec![cond(dd, ConditionOp::Eq, json!(1), false)], ..def(w.b, w.d) }).unwrap();
        // nur Setze-Werte
        set_transition(&c, w.t, &TransitionDef { set_values: vec![SetValue { field_id: num, value: json!(3) }], ..def(w.b, w.a) }).unwrap();
        // alles zusammen
        let full = TransitionDef {
            required_fields: vec![text],
            conditions: vec![cond(num, ConditionOp::Gt, json!(1), true), cond(dd, ConditionOp::In, json!([1, 2]), false), cond(text, ConditionOp::IsEmpty, Value::Null, false)],
            set_values: vec![SetValue { field_id: dd, value: json!(2) }],
            ..def(w.a, w.d)
        };
        set_transition(&c, w.t, &full).unwrap();
        assert_eq!(get_transition(&c, w.t, w.a, w.d).unwrap().unwrap().def, full, "Regeln werden verlustfrei gespeichert");
        // Fehler
        let bad = |d: TransitionDef| set_transition(&c, w.t, &d);
        assert_eq!(bad(TransitionDef { required_fields: vec![foreign], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::FieldNotInType)));
        assert_eq!(bad(TransitionDef { conditions: vec![cond(foreign, ConditionOp::IsEmpty, Value::Null, true)], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::FieldNotInType)));
        assert_eq!(bad(TransitionDef { conditions: vec![cond(text, ConditionOp::Gt, json!("a"), true)], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::Operator)));
        assert_eq!(bad(TransitionDef { conditions: vec![cond(num, ConditionOp::Eq, json!("viel"), true)], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::Value)));
        assert_eq!(bad(TransitionDef { conditions: vec![cond(dd, ConditionOp::Eq, json!(9), true)], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::Value)));
        assert_eq!(bad(TransitionDef { conditions: vec![cond(dd, ConditionOp::In, json!([]), true)], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::Value)));
        assert_eq!(bad(TransitionDef { conditions: vec![cond(dd, ConditionOp::In, json!(1), true)], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::Value)));
        assert_eq!(bad(TransitionDef { set_values: vec![SetValue { field_id: num, value: json!(-5) }], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::Value)), "unter dem Minimum");
        assert_eq!(bad(TransitionDef { set_values: vec![SetValue { field_id: num, value: json!(1) }, SetValue { field_id: num, value: json!(2) }], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::Value)), "ein Feld nur einmal");
        assert_eq!(bad(TransitionDef { set_values: vec![SetValue { field_id: foreign, value: json!("x") }], ..def(w.a, w.b) }), Err(PlanError::Rule(RuleProblem::FieldNotInType)));
        // Das Löschen eines Feldes entfernt es aus allen Regeln
        delete_field(&c, num, None).unwrap();
        let t = get_transition(&c, w.t, w.a, w.b).unwrap().unwrap().def;
        assert_eq!(t.required_fields, vec![text]);
        let t = get_transition(&c, w.t, w.b, w.a).unwrap().unwrap().def;
        assert!(t.set_values.is_empty());
        let t = get_transition(&c, w.t, w.a, w.d).unwrap().unwrap().def;
        assert_eq!(t.conditions.len(), 2, "die Bedingung auf das gelöschte Feld entfiel");
        assert!(t.conditions.iter().all(|c| c.field_id != num));
        assert_eq!(t.set_values.len(), 1);
    }

    #[test]
    fn condition_logic() {
        let cond = |op, value: Value, allow| Condition { field_id: 1, op, value, allow };
        let m = |c: &Condition, v: Option<Value>| condition_matches(c, v.as_ref(), false);
        // die eingebaute Fälligkeit (#214)
        let md = |c: &Condition, v: Option<Value>| condition_matches(c, v.as_ref(), true);
        assert!(m(&cond(ConditionOp::Eq, json!(2), true), Some(json!(2.0))));
        assert!(!m(&cond(ConditionOp::Eq, json!(2), true), None));
        assert!(m(&cond(ConditionOp::Ne, json!(2), true), None));
        assert!(m(&cond(ConditionOp::Gt, json!(2), true), Some(json!(3))));
        assert!(!m(&cond(ConditionOp::Gt, json!(2), true), Some(json!(2))) && m(&cond(ConditionOp::Ge, json!(2), true), Some(json!(2))));
        assert!(m(&cond(ConditionOp::Lt, json!("2026-02-01"), true), Some(json!("2026-01-31"))), "Datum als ISO-Text");
        // #214: Fälligkeit wie in der Abfragesprache (reines Datum = ganzer Tag bzw. Ende des Tages, Sekunden optional)
        assert!(md(&cond(ConditionOp::Eq, json!("2026-11-20"), true), Some(json!("2026-11-20T14:30"))), "Datum trifft den ganzen Tag");
        assert!(md(&cond(ConditionOp::Eq, json!("2026-11-20T14:30:00"), true), Some(json!("2026-11-20T14:30"))), "Sekunden optional");
        assert!(!md(&cond(ConditionOp::Lt, json!("2026-11-20"), true), Some(json!("2026-11-20T09:00"))), "am selben Tag nicht davor");
        assert!(md(&cond(ConditionOp::Le, json!("2026-11-20"), true), Some(json!("2026-11-20T09:00"))));
        assert!(md(&cond(ConditionOp::Gt, json!("2026-11-20T14:30"), true), Some(json!("2026-11-20"))), "reines Datum = Ende des Tages");
        assert!(!md(&cond(ConditionOp::Lt, json!("2026-11-20T14:30"), true), Some(json!("2026-11-20"))));
        assert!(md(&cond(ConditionOp::In, json!(["2026-11-19", "2026-11-20"]), true), Some(json!("2026-11-20T08:00"))));
        assert!(md(&cond(ConditionOp::Eq, json!("abc"), true), Some(json!("abc"))) && !md(&cond(ConditionOp::Eq, json!("abc"), true), Some(json!("abd"))), "Text bleibt Text");
        // andere Felder (eigenes Datum+Uhrzeit, Text) behalten den bisherigen Vergleich
        assert!(!m(&cond(ConditionOp::Eq, json!("2026-11-20"), true), Some(json!("2026-11-20T14:30"))));
        assert!(m(&cond(ConditionOp::Lt, json!("2026-11-20"), true), Some(json!("2026-11-19T23:00"))));
        // nur die Bedingung auf dem Fälligkeitsfeld vergleicht als Fälligkeit
        let on = |fid: i64| Condition { field_id: fid, op: ConditionOp::Eq, value: json!("2026-11-20"), allow: true };
        let v = |_id: i64| Some(json!("2026-11-20T14:30"));
        assert!(evaluate_conditions(&[on(1)], &v, Some(1)) && !evaluate_conditions(&[on(1)], &v, Some(2)) && !evaluate_conditions(&[on(1)], &v, None));
        assert!(!m(&cond(ConditionOp::Lt, json!(2), true), None), "kein Wert: Vergleich trifft nicht zu");
        assert!(m(&cond(ConditionOp::In, json!([1, 2]), true), Some(json!(2))));
        assert!(m(&cond(ConditionOp::NotIn, json!([1, 2]), true), Some(json!(3))));
        assert!(m(&cond(ConditionOp::IsEmpty, Value::Null, true), Some(json!(""))));
        assert!(m(&cond(ConditionOp::IsEmpty, Value::Null, true), Some(json!([]))));
        assert!(m(&cond(ConditionOp::IsNotEmpty, Value::Null, true), Some(json!(false))), "false ist ein Wert");
        // Listenwerte: enthält
        assert!(m(&cond(ConditionOp::Eq, json!(2), true), Some(json!([1, 2]))));
        assert!(m(&cond(ConditionOp::Ne, json!(3), true), Some(json!([1, 2]))));
        // Auswertung
        let vals = |v: Option<Value>| move |_id: i64| v.clone();
        assert!(evaluate_conditions(&[], &vals(None), None), "keine Bedingungen: erlaubt");
        let block = cond(ConditionOp::Eq, json!(1), false);
        assert!(evaluate_conditions(&[block.clone()], &vals(Some(json!(2))), None), "nur sperrende, keine trifft zu: erlaubt");
        assert!(!evaluate_conditions(&[block.clone()], &vals(Some(json!(1))), None));
        let allow = cond(ConditionOp::Eq, json!(5), true);
        assert!(evaluate_conditions(&[allow.clone()], &vals(Some(json!(5))), None));
        assert!(!evaluate_conditions(&[allow.clone()], &vals(Some(json!(6))), None), "erlaubende Bedingung ohne Treffer: gesperrt");
        // die erste zutreffende entscheidet
        assert!(!evaluate_conditions(&[block.clone(), allow.clone()], &vals(Some(json!(1))), None));
        let both = [cond(ConditionOp::Gt, json!(0), true), cond(ConditionOp::Eq, json!(7), false)];
        assert!(!evaluate_conditions(&[both[1].clone(), both[0].clone()], &vals(Some(json!(7))), None));
        assert!(evaluate_conditions(&both, &vals(Some(json!(7))), None), "Reihenfolge entscheidet");
    }

    #[test]
    fn workflow_problems_are_reported() {
        let c = db();
        let w = setup(&c);
        assert_eq!(workflow_problems(&c, w.t).unwrap(), vec![WorkflowProblem::NoStatuses]);
        set_workflow_statuses(&c, w.t, &[w.b]).unwrap();
        assert_eq!(workflow_problems(&c, w.t).unwrap(), vec![WorkflowProblem::NoTodoStatus, WorkflowProblem::NoDoneStatus]);
        set_workflow_statuses(&c, w.t, &[w.a, w.b, w.d]).unwrap();
        assert!(workflow_problems(&c, w.t).unwrap().is_empty());
        replace_transitions(&c, w.t, &[def(w.a, w.b)]).unwrap();
        assert_eq!(workflow_problems(&c, w.t).unwrap(), vec![WorkflowProblem::Unreachable(w.d)]);
        replace_transitions(&c, w.t, &[def(w.a, w.b), def(w.b, w.d)]).unwrap();
        assert!(workflow_problems(&c, w.t).unwrap().is_empty());
    }

    #[test]
    fn system_type_has_no_workflow() {
        let c = db();
        let todo = crate::planning::projects::project_by_key(&c, "TODO").unwrap();
        let t = crate::planning::types::list_types(&c, todo.id).unwrap().remove(0);
        assert_eq!(set_workflow(&c, t.id, true, false), Err(PlanError::SystemProtected));
        assert_eq!(set_workflow_statuses(&c, t.id, &[]), Err(PlanError::SystemProtected));
    }
}
