//! Beispieldaten anlegen: aus einer [`DemoSpec`] (Beschreibung der Hilfe-Beispiele) entsteht in einer **frischen Datenbank** ein
//! Projekt mit Typen, Status, Feldern und Tickets samt Feldwerten, Sternen, Markierungen, Eltern und Terminal-Verknüpfungen.
//!
//! Regeln:
//!
//! * Namen, die den englischen Standardnamen entsprechen („To Do“, „User story“, „Highest“, „Story points“ …), bekommen den
//!   Standard-Schlüssel (und übersetzen sich dadurch wie echte Standard-Inhalte); alle anderen sind eigene Namen.
//! * Typen: alle mit Workflow und allen Status; ein Typ darf unter allen Typen stehen, die vor ihm in der Liste kommen.
//! * Die Schlüssel der Tickets bleiben erhalten (`WEB-12` wird die Nummer 12); die Nummern müssen zum Projektschlüssel passen.
//! * `changed_by` (Quelle der letzten Änderung): alle Schreibvorgänge eines Tickets laufen mit dieser Quelle; ohne Angabe wird der
//!   Verlauf des Tickets am Ende gelöscht (dann ist `changedBy` leer, wie in der Referenz-Auswertung).
//! * **Sprints, Versionen, Meilensteine:** Die Sprints der Beschreibung entstehen in ihrer Reihenfolge (geplant), die Tickets kommen
//!   hinein, danach werden die abgeschlossenen gestartet und abgeschlossen (unerledigte Tickets bleiben im Sprint stehen) und der
//!   aktive gestartet – alles über `planning::sprints`. Versionen (mit „erschienen“) und Meilensteine (die Werte der Meilenstein-Felder)
//!   werden vor den Tickets angelegt; Feldwerte verweisen auf ihre Kennung. Die Projektschalter `use_sprints`/`use_versions` stehen
//!   dann an.
//! * Das Datum „heute“ löst relative Angaben (`@-3`) auf, siehe [`DemoSpec::resolved`].

use crate::planning::fields::{self, create_field, Field, FieldKind, FieldOptions, NewField, OptionValue};
use crate::planning::history::Source;
use crate::planning::link_types::link_type_by_key;
use crate::planning::links::{add_link, Target};
use crate::planning::names::translate;
use crate::calc::calendar::WorkCalendar;
use crate::planning::projects::{create_project, NewProject};
use crate::planning::sprints::releases::{create_milestone, create_version, set_version_released, NewRelease};
use crate::planning::sprints::sprint::{complete_sprint, create_sprint, move_tickets_to_sprint, start_sprint, Leftover, NewSprint};
use crate::planning::sprints::{parse_date, SprintError};
use crate::planning::query_help::{DemoSpec, DemoTicket, DemoValue, SprintState as DemoSprintState};
use crate::planning::statuses::{create_status, NewStatus, StatusCategory};
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::{create_ticket, parse_key, raw_set_status, set_field, set_flagged, set_parent, set_starred, NewTicket};
use crate::planning::transition::transition_ticket;
use crate::planning::types::{create_type, set_parents, NewType};
use crate::planning::workflow::set_workflow_statuses;
use crate::planning::PlanError;
use crate::state::Lang;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum SeedError {
    /// Die Beschreibung widerspricht sich (`DemoSpec::check`).
    Spec(Vec<String>),
    /// Das Projekt gibt es schon (nur frische Datenbanken).
    NotFresh,
    /// Ein Schlüssel passt nicht zum Projektschlüssel oder ist keine Nummer.
    BadKey(String),
    Db(String),
}

impl SeedError {
    pub fn message(&self, lang: Lang) -> String {
        match self {
            SeedError::Spec(p) => format!("{}: {}", lang.t("Die Beispieldaten widersprechen sich", "The example data contradicts itself"), p.join("; ")),
            SeedError::NotFresh => lang.t("Die Datenbank ist nicht frisch (das Projekt gibt es schon).", "The database is not fresh (the project already exists).").to_string(),
            SeedError::BadKey(k) => format!("{}: {k}", lang.t("Ungültiger Ticket-Schlüssel", "Invalid ticket key")),
            SeedError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
        }
    }
}

impl From<PlanError> for SeedError {
    fn from(e: PlanError) -> Self {
        SeedError::Db(e.message(Lang::En))
    }
}

impl From<TicketError> for SeedError {
    fn from(e: TicketError) -> Self {
        SeedError::Db(e.message(Lang::En))
    }
}

impl From<SprintError> for SeedError {
    fn from(e: SprintError) -> Self {
        SeedError::Db(e.message(Lang::En))
    }
}

impl From<rusqlite::Error> for SeedError {
    fn from(e: rusqlite::Error) -> Self {
        SeedError::Db(e.to_string())
    }
}

/// Was angelegt wurde.
#[derive(Debug, Clone, PartialEq)]
pub struct SeededDemo {
    pub project_id: i64,
    /// (Schlüssel, Kennung) in der Reihenfolge der Beschreibung.
    pub tickets: Vec<(String, i64)>,
    /// Das aktuelle Terminal der Suche (für `Ctx::terminals`); leer ohne Angabe.
    pub terminals: Vec<String>,
    /// Das aufgelöste „heute“ (`JJJJ-MM-TT`).
    pub today: String,
    /// Was nicht angelegt wurde und warum (seit der Anbindung von Sprints und Versionen immer leer; bleibt für die Schnittstelle).
    pub skipped: Vec<String>,
}

impl SeededDemo {
    pub fn ticket_id(&self, key: &str) -> Option<i64> {
        self.tickets.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, id)| *id)
    }
}

/// Standard-Schlüssel eines Namens, wenn er dem englischen Standardnamen eines der `candidates` entspricht.
fn default_key(name: &str, candidates: &[&'static str]) -> Option<&'static str> {
    candidates.iter().copied().find(|k| translate(k, Lang::En).is_some_and(|en| en.eq_ignore_ascii_case(name.trim())))
}

const STATUS_KEYS: [&str; 3] = ["todo", "progress", "done"];
const TYPE_KEYS: [&str; 5] = ["epic", "userstory", "devtask", "subtask", "defect"];
const PRIORITY_KEYS: [&str; 5] = ["priority.highest", "priority.high", "priority.medium", "priority.low", "priority.lowest"];
const FIELD_KEYS: [&str; 6] = ["description", "priority", "story_points", "release_version", "milestone", "steps"];

fn status_color(c: StatusCategory) -> &'static str {
    match c {
        StatusCategory::Todo => "#97A3B0",
        StatusCategory::Progress => "#58A6E6",
        StatusCategory::Done => "#7FBF6A",
    }
}

fn type_style(key: Option<&str>, index: usize) -> (&'static str, Option<&'static str>) {
    match key {
        Some("epic") => ("#B77FE0", Some("bolt")),
        Some("userstory") => ("#7FBF6A", Some("bookmark")),
        Some("devtask") => ("#58A6E6", Some("code")),
        Some("subtask") => ("#4FB8A8", Some("subtask")),
        Some("defect") => ("#E06C75", Some("bug")),
        _ => (["#D8B94A", "#E5A04B", "#97A3B0"][index % 3], None),
    }
}

/// Ein angelegtes Feld eines Typs.
struct Made {
    field: Field,
}

struct Seeder<'a> {
    conn: &'a Connection,
    spec: DemoSpec,
    project_id: i64,
    status_ids: HashMap<String, i64>,
    type_ids: Vec<(String, i64)>,
    /// (Typ-Kennung, Feldname klein) → Feld
    fields: HashMap<(i64, String), Made>,
    skipped: Vec<String>,
    /// Name (klein) → Kennung der angelegten Versionen und Meilensteine
    version_ids: HashMap<String, i64>,
    milestone_ids: HashMap<String, i64>,
}

fn source_of(t: &DemoTicket) -> Source {
    match t.changed_by.as_deref() {
        Some("agent") => Source::agent("demo"),
        Some("import") => Source::import(),
        Some("quick") => Source::quick(),
        _ => Source::ui(),
    }
}

/// Legt die Beispieldaten in der (frischen) Datenbank an. `today` ist `JJJJ-MM-TT`.
pub fn seed_demo(conn: &Connection, spec: &DemoSpec, today: &str) -> Result<SeededDemo, SeedError> {
    let problems = spec.check();
    if !problems.is_empty() {
        return Err(SeedError::Spec(problems));
    }
    conn.execute_batch("SAVEPOINT seed_demo")?;
    match seed_inner(conn, spec, today) {
        Ok(v) => {
            conn.execute_batch("RELEASE seed_demo")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO seed_demo; RELEASE seed_demo");
            Err(e)
        }
    }
}

fn seed_inner(conn: &Connection, spec: &DemoSpec, today: &str) -> Result<SeededDemo, SeedError> {
    let spec = spec.resolved(today);
    let key_taken: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE key = ?1", [spec.project_key.to_ascii_uppercase()], |r| r.get(0))?;
    if key_taken > 0 {
        return Err(SeedError::NotFresh);
    }
    let skipped = Vec::new();
    let mut np = NewProject::new(&spec.project_key, &spec.project_name, "#4C8BF5");
    np.with_defaults = false;
    // die Schalter stehen, damit die Felder `sprint`/Versionen im Schema erscheinen
    np.use_sprints = !spec.sprints.is_empty();
    np.use_versions = !spec.versions.is_empty() || spec.fields.iter().any(|f| matches!(f.kind, FieldKind::Version | FieldKind::Milestone));
    let project_id = create_project(conn, &np)?;
    let mut s = Seeder {
        conn,
        spec,
        project_id,
        status_ids: HashMap::new(),
        type_ids: Vec::new(),
        fields: HashMap::new(),
        skipped,
        version_ids: HashMap::new(),
        milestone_ids: HashMap::new(),
    };
    s.releases()?;
    s.config()?;
    let tickets = s.tickets()?;
    s.sprints(&tickets)?;
    Ok(SeededDemo {
        project_id,
        tickets,
        terminals: s.spec.current_terminal.iter().cloned().collect(),
        today: today.to_string(),
        skipped: s.skipped,
    })
}

impl Seeder<'_> {
    /// Versionen (mit „erschienen“) und Meilensteine (die Werte der Meilenstein-Felder).
    fn releases(&mut self) -> Result<(), SeedError> {
        let (conn, pid) = (self.conn, self.project_id);
        for v in self.spec.versions.clone() {
            let due = v.due.as_deref().and_then(|d| parse_date(d).ok());
            let id = create_version(conn, &NewRelease { project_id: pid, name: v.name.clone(), description: String::new(), due_date: due })?;
            if v.released {
                set_version_released(conn, id, true)?;
            }
            self.version_ids.insert(v.name.to_lowercase(), id);
        }
        for f in self.spec.fields.clone().iter().filter(|f| f.kind == FieldKind::Milestone) {
            for name in &f.options {
                if !self.milestone_ids.contains_key(&name.to_lowercase()) {
                    let id = create_milestone(conn, &NewRelease::new(pid, name))?;
                    self.milestone_ids.insert(name.to_lowercase(), id);
                }
            }
        }
        Ok(())
    }

    /// Sprints in der Reihenfolge der Beschreibung anlegen, Tickets hineinlegen, abgeschlossene und aktiven Sprint starten.
    fn sprints(&mut self, made: &[(String, i64)]) -> Result<(), SeedError> {
        let (conn, pid) = (self.conn, self.project_id);
        let cal = WorkCalendar::default();
        let mut ids: Vec<(String, i64, DemoSprintState)> = Vec::new();
        for sp in self.spec.sprints.clone() {
            let n = NewSprint { project_id: pid, name: sp.name.clone(), goal: String::new(), start: parse_date(&sp.start).ok(), end: parse_date(&sp.end).ok(), default_days: None };
            ids.push((sp.name.clone(), create_sprint(conn, &n, &cal)?, sp.state));
        }
        for (name, sprint_id, _) in &ids {
            let tickets: Vec<i64> = self
                .spec
                .tickets
                .iter()
                .zip(made)
                .filter(|(t, _)| t.sprint.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(name)))
                .map(|(_, (_, id))| *id)
                .collect();
            if !tickets.is_empty() {
                move_tickets_to_sprint(conn, *sprint_id, &tickets)?;
            }
        }
        // zuerst die abgeschlossenen (je Projekt läuft höchstens ein Sprint), dann der aktive
        for (_, sprint_id, _) in ids.iter().filter(|(_, _, st)| *st == DemoSprintState::Closed) {
            start_sprint(conn, *sprint_id, &cal)?;
            complete_sprint(conn, *sprint_id, Leftover::ToBacklog)?;
        }
        for (_, sprint_id, _) in ids.iter().filter(|(_, _, st)| *st == DemoSprintState::Active) {
            start_sprint(conn, *sprint_id, &cal)?;
        }
        Ok(())
    }

    /// Status, Typen, Hierarchie und Felder.
    fn config(&mut self) -> Result<(), SeedError> {
        let conn = self.conn;
        let pid = self.project_id;
        for st in &self.spec.statuses {
            let key = default_key(&st.name, &STATUS_KEYS);
            let id = create_status(
                conn,
                &NewStatus { project_id: pid, key: key.map(String::from), name: if key.is_some() { None } else { Some(st.name.clone()) }, color: status_color(st.category).into(), category: st.category },
            )?;
            self.status_ids.insert(st.name.to_lowercase(), id);
        }
        let all_status: Vec<i64> = self.spec.statuses.iter().filter_map(|s| self.status_ids.get(&s.name.to_lowercase()).copied()).collect();
        for (i, name) in self.spec.types.iter().enumerate() {
            let key = default_key(name, &TYPE_KEYS);
            let (color, icon) = type_style(key, i);
            let id = create_type(
                conn,
                &NewType { project_id: pid, key: key.map(String::from), name: if key.is_some() { None } else { Some(name.clone()) }, icon: icon.map(String::from), color: color.into(), workflow: true },
            )?;
            set_workflow_statuses(conn, id, &all_status)?;
            self.type_ids.push((name.to_lowercase(), id));
        }
        // ein Typ darf unter allen Typen stehen, die vor ihm kommen
        for i in 1..self.type_ids.len() {
            let parents: Vec<i64> = self.type_ids[..i].iter().map(|(_, id)| *id).collect();
            set_parents(conn, self.type_ids[i].1, &parents)?;
        }
        // Felder: zuerst die Priorität (an jedem Typ), dann die benutzerdefinierten
        if !self.spec.priorities.is_empty() {
            let values: Vec<OptionValue> = self
                .spec
                .priorities
                .iter()
                .map(|p| match default_key(p, &PRIORITY_KEYS) {
                    Some(k) => OptionValue { id: 0, key: Some(k.into()), name: None, color: None },
                    None => OptionValue::new(p),
                })
                .collect();
            for (_, tid) in self.type_ids.clone() {
                let mut n = NewField::new(tid, "Priority", FieldKind::Dropdown);
                n.key = Some("priority".into());
                n.name = None;
                n.options = FieldOptions { values: values.clone(), ..Default::default() };
                n.show_on_card = true;
                n.builtin = true;
                let fid = create_field(conn, &n)?;
                self.fields.insert((tid, "priority".into()), Made { field: fields::get_field(conn, fid)? });
            }
        }
        for f in self.spec.fields.clone() {
            let targets: Vec<i64> =
                if f.types.is_empty() { self.type_ids.iter().map(|(_, id)| *id).collect() } else { self.type_ids.iter().filter(|(n, _)| f.types.iter().any(|t| t.eq_ignore_ascii_case(n))).map(|(_, id)| *id).collect() };
            for tid in targets {
                // Beschreibung und Fälligkeit sind in jedem Typ eingebaut: deren Felder benutzen
                let builtin = match (f.name.to_lowercase().as_str(), f.kind) {
                    ("description", FieldKind::Rich | FieldKind::Text) => Some(crate::planning::builtin_fields::DESCRIPTION_KEY),
                    ("due", FieldKind::Date | FieldKind::Datetime) => Some(crate::planning::builtin_fields::DUE_KEY),
                    _ => None,
                };
                if let Some(fid) = builtin.map(|k| crate::planning::builtin_fields::field_id(conn, tid, k)).transpose()?.flatten() {
                    self.fields.insert((tid, f.name.to_lowercase()), Made { field: fields::get_field(conn, fid)? });
                    continue;
                }
                let key = default_key(&f.name, &FIELD_KEYS);
                let mut n = NewField::new(tid, &f.name, f.kind);
                if let Some(k) = key {
                    n.key = Some(k.into());
                    n.name = None;
                }
                if matches!(f.kind, FieldKind::Dropdown | FieldKind::Multi) {
                    n.options = FieldOptions { values: f.options.iter().map(|o| OptionValue::new(o)).collect(), ..Default::default() };
                }
                if f.kind == FieldKind::Number {
                    n.options.min = Some(0.0);
                    n.options.decimals = Some(0);
                }
                let fid = create_field(conn, &n)?;
                self.fields.insert((tid, f.name.to_lowercase()), Made { field: fields::get_field(conn, fid)? });
            }
        }
        Ok(())
    }

    fn type_id(&self, name: &str) -> i64 {
        self.type_ids.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, id)| *id).unwrap_or(0)
    }

    /// JSON-Wert für ein Feld; `None` = wird nicht gesetzt (Version/Meilenstein; Ticket-Verweise kommen später).
    fn json_value(&mut self, f: &Field, v: &DemoValue) -> Option<Value> {
        match (f.kind, v) {
            (FieldKind::Checkbox, DemoValue::Bool(b)) => Some(json!(b)),
            (FieldKind::Number, DemoValue::Number(n)) => Some(json!(n)),
            (FieldKind::Text | FieldKind::Rich | FieldKind::Terminal | FieldKind::Date | FieldKind::Datetime, DemoValue::Text(s)) => Some(json!(s)),
            (FieldKind::Labels, DemoValue::List(l)) => Some(json!(l)),
            (FieldKind::Dropdown, DemoValue::Text(s)) => f.options.values.iter().find(|o| o.display(Lang::En).eq_ignore_ascii_case(s)).map(|o| json!(o.id)),
            (FieldKind::Multi, DemoValue::List(l)) => {
                let ids: Vec<Value> = l.iter().filter_map(|s| f.options.values.iter().find(|o| o.display(Lang::En).eq_ignore_ascii_case(s)).map(|o| json!(o.id))).collect();
                Some(Value::Array(ids))
            }
            (FieldKind::Version, DemoValue::Text(s)) => self.version_ids.get(&s.to_lowercase()).map(|id| json!(id)),
            (FieldKind::Milestone, DemoValue::Text(s)) => self.milestone_ids.get(&s.to_lowercase()).map(|id| json!(id)),
            _ => None,
        }
    }

    fn tickets(&mut self) -> Result<Vec<(String, i64)>, SeedError> {
        let conn = self.conn;
        let pid = self.project_id;
        let spec = self.spec.clone();
        let mut made: Vec<(String, i64)> = Vec::new();
        // Phase 1: anlegen (Status, Werte, Stern, Markierung, Terminal-Verknüpfung)
        for t in &spec.tickets {
            let source = source_of(t);
            let (prefix, number) = parse_key(&t.key).ok_or_else(|| SeedError::BadKey(t.key.clone()))?;
            if !prefix.eq_ignore_ascii_case(&spec.project_key) {
                return Err(SeedError::BadKey(t.key.clone()));
            }
            conn.execute("UPDATE projects SET next_number = ?1 WHERE id = ?2", [number, pid])?;
            let tid = self.type_id(&t.ty);
            let mut values: Vec<(i64, Value)> = Vec::new();
            if let (Some(p), Some(m)) = (&t.priority, self.fields.get(&(tid, "priority".into()))) {
                if let Some(o) = m.field.options.values.iter().find(|o| o.display(Lang::En).eq_ignore_ascii_case(p)) {
                    values.push((m.field.id, json!(o.id)));
                }
            }
            let mut terminal_names: Vec<String> = Vec::new();
            for (name, v) in &t.values {
                let Some(def) = spec.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name)) else { continue };
                let Some(m) = self.fields.get(&(tid, name.to_lowercase())) else { continue };
                if def.kind == FieldKind::Ticket {
                    continue; // Phase 2
                }
                let field = m.field.clone();
                if let Some(j) = self.json_value(&field, v) {
                    if def.kind == FieldKind::Terminal {
                        if let DemoValue::Text(s) = v {
                            terminal_names.push(s.clone());
                        }
                    }
                    values.push((field.id, j));
                }
            }
            let (ticket, _) = create_ticket(conn, &NewTicket { project_id: pid, type_id: tid, title: t.summary.clone(), parent_id: None, values }, &source)?;
            match &t.status {
                Some(s) => {
                    let sid = self.status_ids.get(&s.to_lowercase()).copied().unwrap_or(0);
                    transition_ticket(conn, ticket.id, sid, &[], &source)?;
                }
                None => {
                    raw_set_status(conn, ticket.id, None, &source, 0)?;
                }
            }
            if t.favorite {
                set_starred(conn, ticket.id, true, &source)?;
            }
            if t.blocked {
                set_flagged(conn, ticket.id, true, &source)?;
            }
            if !terminal_names.is_empty() {
                let lt = link_type_by_key(conn, "terminal")?;
                for name in terminal_names {
                    add_link(conn, ticket.id, lt.id, Target::Terminal(name), &source)?;
                }
            }
            made.push((t.key.clone(), ticket.id));
        }
        // Phase 2: Eltern und Ticket-Verweise
        for (t, (_, id)) in spec.tickets.iter().zip(&made) {
            let source = source_of(t);
            if let Some(p) = &t.parent {
                let pid_ticket = made.iter().find(|(k, _)| k.eq_ignore_ascii_case(p)).map(|(_, i)| *i).unwrap_or(0);
                set_parent(conn, *id, Some(pid_ticket), &source)?;
            }
            let tid = self.type_id(&t.ty);
            for (name, v) in &t.values {
                let Some(def) = spec.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name)) else { continue };
                if def.kind != FieldKind::Ticket {
                    continue;
                }
                let (Some(m), DemoValue::Text(target)) = (self.fields.get(&(tid, name.to_lowercase())), v) else { continue };
                let target_id = made.iter().find(|(k, _)| k.eq_ignore_ascii_case(target)).map(|(_, i)| *i).unwrap_or(0);
                set_field(conn, *id, m.field.id, json!(target_id), &source)?;
            }
        }
        // ohne `changed_by`: kein Verlauf (`changedBy` ist dann leer)
        for (t, (_, id)) in spec.tickets.iter().zip(&made) {
            if t.changed_by.is_none() {
                conn.execute("DELETE FROM history WHERE ticket_id = ?1", [id])?;
            }
        }
        Ok(made)
    }
}
