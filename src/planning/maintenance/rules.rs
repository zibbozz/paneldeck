//! Eigene Prüfregeln: gespeicherte Abfragen der Abfragesprache mit Name, Schwere und vorgeschlagener Aktion (Status setzen,
//! Feld setzen, archivieren). Je Projekt, aus, bis jemand eine anlegt; jede Regel lässt sich einzeln abschalten. Treffer der
//! Abfrage sind Befunde der Kategorie „Eigene Regeln“. Die Regeln werden mit dem Projekt exportiert (`backup_doc`).

use super::{Check, Finding, Fix, MRes, MaintError, RunOpts, Severity, Target, TicketRef};
use crate::planning::projects::Project;
use crate::planning::search::{self, Page, Scope};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Höchstzahl Treffer je Regel im Bericht.
pub const MAX_HITS_PER_RULE: i64 = 500;
pub const MAX_NAME_CHARS: usize = 120;

/// Vorgeschlagene Aktion einer Regel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleAction {
    /// Nur melden.
    #[default]
    None,
    /// In diesen Status wechseln (Workflow-Regeln gelten).
    Status { status: i64 },
    /// Feld setzen (Wert im Format der Datenschicht, z. B. Zahl, Text, Datum `JJJJ-MM-TT`).
    Field { field: i64, value: Value },
    Archive,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub jql: String,
    pub severity: Severity,
    pub action: RuleAction,
    pub enabled: bool,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewRule {
    pub name: String,
    pub jql: String,
    pub severity: Severity,
    pub action: RuleAction,
    pub enabled: bool,
}

const COLS: &str = "id, project_id, name, jql, severity, action, enabled, position";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Rule> {
    Ok(Rule {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        jql: r.get(3)?,
        severity: Severity::parse(&r.get::<_, String>(4)?).unwrap_or(Severity::Warning),
        action: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        enabled: r.get::<_, i64>(6)? != 0,
        position: r.get(7)?,
    })
}

pub fn list_rules(conn: &Connection, project_id: i64) -> rusqlite::Result<Vec<Rule>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM maintenance_rules WHERE project_id = ?1 ORDER BY position, id"))?;
    let v = st.query_map([project_id], from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn get_rule(conn: &Connection, id: i64) -> MRes<Rule> {
    conn.query_row(&format!("SELECT {COLS} FROM maintenance_rules WHERE id = ?1"), [id], from_row).optional()?.ok_or(MaintError::NotFound)
}

/// Prüft Name, Abfrage (gegen das Schema des Projekts) und Aktion.
pub fn validate(conn: &Connection, project_id: i64, n: &NewRule, lang: Lang) -> MRes<NewRule> {
    let name = n.name.trim().to_string();
    if name.is_empty() {
        return Err(MaintError::refused("Die Regel braucht einen Namen.", "The rule needs a name."));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(MaintError::refused(format!("Name zu lang (höchstens {MAX_NAME_CHARS} Zeichen)."), format!("Name too long (at most {MAX_NAME_CHARS} characters).")));
    }
    let jql = n.jql.trim().to_string();
    if jql.is_empty() {
        return Err(MaintError::refused("Die Regel braucht eine Abfrage.", "The rule needs a query."));
    }
    if let Err(e) = search::prepare(conn, &search::Ctx::today_local(lang), Scope::Project(project_id), &jql) {
        return Err(MaintError::Refused { de: e.message(Lang::De), en: e.message(Lang::En) });
    }
    match &n.action {
        RuleAction::Status { status } => {
            let ok: Option<i64> = conn.query_row("SELECT id FROM statuses WHERE id = ?1 AND project_id = ?2", params![status, project_id], |r| r.get(0)).optional()?;
            if ok.is_none() {
                return Err(MaintError::refused("Der Status gehört nicht zum Projekt.", "The status does not belong to the project."));
            }
        }
        RuleAction::Field { field, value } => {
            let f = crate::planning::fields::get_field(conn, *field).map_err(|_| MaintError::refused("Das Feld gibt es nicht.", "The field does not exist."))?;
            let proj: Option<i64> = conn.query_row("SELECT project_id FROM ticket_types WHERE id = ?1", [f.type_id], |r| r.get(0)).optional()?;
            if proj != Some(project_id) {
                return Err(MaintError::refused("Das Feld gehört nicht zum Projekt.", "The field does not belong to the project."));
            }
            crate::planning::fields::check_value(f.kind, &f.options, value, true).map_err(MaintError::from)?;
        }
        RuleAction::None | RuleAction::Archive => {}
    }
    Ok(NewRule { name, jql, severity: n.severity, action: n.action.clone(), enabled: n.enabled })
}

pub fn create_rule(conn: &Connection, project_id: i64, n: &NewRule, lang: Lang) -> MRes<i64> {
    super::maint_project(conn, project_id)?;
    let n = validate(conn, project_id, n, lang)?;
    let pos: i64 = conn.query_row("SELECT COALESCE(MAX(position) + 1, 0) FROM maintenance_rules WHERE project_id = ?1", [project_id], |r| r.get(0))?;
    conn.execute(
        "INSERT INTO maintenance_rules (project_id, name, jql, severity, action, enabled, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project_id, n.name, n.jql, n.severity.as_str(), serde_json::to_string(&n.action).unwrap_or_else(|_| "{}".into()), n.enabled as i64, pos],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_rule(conn: &Connection, id: i64, n: &NewRule, lang: Lang) -> MRes<()> {
    let cur = get_rule(conn, id)?;
    let n = validate(conn, cur.project_id, n, lang)?;
    conn.execute(
        "UPDATE maintenance_rules SET name = ?1, jql = ?2, severity = ?3, action = ?4, enabled = ?5 WHERE id = ?6",
        params![n.name, n.jql, n.severity.as_str(), serde_json::to_string(&n.action).unwrap_or_else(|_| "{}".into()), n.enabled as i64, id],
    )?;
    Ok(())
}

pub fn set_enabled(conn: &Connection, id: i64, on: bool) -> MRes<()> {
    conn.execute("UPDATE maintenance_rules SET enabled = ?1 WHERE id = ?2", params![on as i64, id])?;
    Ok(())
}

pub fn delete_rule(conn: &Connection, id: i64) -> MRes<()> {
    conn.execute("DELETE FROM maintenance_rules WHERE id = ?1", [id])?;
    Ok(())
}

/// Text der vorgeschlagenen Aktion.
pub fn action_text(conn: &Connection, a: &RuleAction, lang: Lang) -> String {
    match a {
        RuleAction::None => lang.t("nur melden", "report only").into(),
        RuleAction::Archive => lang.t("archivieren", "archive").into(),
        RuleAction::Status { status } => {
            let s = crate::planning::statuses::get_status(conn, *status).map(|s| s.display(lang)).unwrap_or_else(|_| lang.t("(gelöscht)", "(deleted)").into());
            format!("{} {}", lang.t("Status setzen:", "set status:"), lang.q(&s))
        }
        RuleAction::Field { field, value } => {
            let f = crate::planning::fields::get_field(conn, *field).map(|f| f.display(lang)).unwrap_or_else(|_| lang.t("(gelöscht)", "(deleted)").into());
            format!("{} {} = {}", lang.t("Feld setzen:", "set field:"), lang.q(&f), value)
        }
    }
}

/// Befunde aller eingeschalteten Regeln. Eine Regel mit kaputter Abfrage (z. B. Feld gelöscht) ergibt selbst einen Befund.
pub fn run(conn: &Connection, project: &Project, o: &RunOpts) -> MRes<Vec<Finding>> {
    let mut out = Vec::new();
    let ctx = search::Ctx::new(&o.now.format("%Y-%m-%d").to_string(), o.lang);
    for r in list_rules(conn, project.id)?.into_iter().filter(|r| r.enabled) {
        let fixes = if r.action == RuleAction::None { vec![] } else { vec![Fix::RuleAction] };
        let action = action_text(conn, &r.action, o.lang);
        match search::run_query(conn, &ctx, Scope::Project(project.id), &r.jql, Page::first(MAX_HITS_PER_RULE)) {
            Ok(page) => {
                for row in page.rows {
                    let message = match o.lang {
                        Lang::De => format!("Regel {}: trifft zu (Vorschlag: {action}).", o.lang.q(&r.name)),
                        Lang::En => format!("Rule {}: matches (suggestion: {action}).", o.lang.q(&r.name)),
                    };
                    out.push(Finding {
                        id: format!("custom_rule:r{}:{}", r.id, row.key),
                        check: Check::CustomRule,
                        severity: r.severity,
                        ticket: Some(TicketRef { id: row.id, key: row.key.clone(), title: row.title.clone() }),
                        other: None,
                        target: Target::Rule(r.id),
                        message,
                        fixes: fixes.clone(),
                        detail: json!({"rule": r.name, "rule_id": r.id}),
                    });
                }
            }
            Err(e) => {
                let message = match o.lang {
                    Lang::De => format!("Regel {} lässt sich nicht ausführen: {}", o.lang.q(&r.name), e.message(o.lang)),
                    Lang::En => format!("Rule {} cannot run: {}", o.lang.q(&r.name), e.message(o.lang)),
                };
                out.push(Finding {
                    id: format!("custom_rule:r{}:broken", r.id),
                    check: Check::CustomRule,
                    severity: Severity::Error,
                    ticket: None,
                    other: None,
                    target: Target::Rule(r.id),
                    message,
                    fixes: vec![],
                    detail: json!({"rule": r.name, "rule_id": r.id, "broken": true}),
                });
            }
        }
    }
    Ok(out)
}
