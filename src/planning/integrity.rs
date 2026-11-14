//! Integritätsregel beim Löschen: Was benutzt wird (Status, Typ, Feld, Verknüpfungstyp, Art der Arbeit, Projekt), lässt sich
//! nur löschen, wenn man einen Ersatz angibt ([`DeleteMode::Migrate`]) oder die Verwendungen bewusst verwirft
//! ([`DeleteMode::Discard`]). Ohne Modus liefert die API [`PlanError::InUse`] mit der Zahl der Verwendungen.
//!
//! Die `usage_count_*`-Funktionen zählen über Listen von SQL-Abfragen (`*_USAGE`, je mit den Platzhaltern `?1…`) auf den
//! Tabellen der Tickets (`tickets`, `ticket_values`, `links`, `time_entries`); die `migrate_tickets_*`-Funktionen hängen
//! Werte um bzw. verwerfen sie (bei Typen mit Verlaufseinträgen der Quelle „Oberfläche / migration“). Die Löschfunktionen
//! der Konfiguration rufen diese Stellen an der richtigen Stelle (vor dem Löschen, atomar) auf.

use super::history::Source;
use super::ticket_error::TicketError;
use super::{PlanError, Res};
use rusqlite::{params, params_from_iter, Connection};

/// Was mit den Verwendungen eines zu löschenden Eintrags geschieht.
#[derive(Debug, Clone, PartialEq)]
pub enum DeleteMode {
    /// Verwendungen werden entfernt (Wert/Status wird geleert).
    Discard,
    /// Verwendungen werden auf den Ersatz `to` umgehängt (Status/Feld/Verknüpfungstyp/Art der Arbeit).
    Migrate { to: i64 },
}

/// Zuordnung eines Feldes des alten Typs auf ein Feld des Ersatz-Typs (`None` = Wert verwerfen).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldMapEntry {
    pub from_field: i64,
    pub to_field: Option<i64>,
}

/// **Vertrag für die Typ-Migration:** Tickets des gelöschten Typs werden zum Typ `to_type`; die Werte ihrer Felder wandern
/// gemäß `fields` (Felder ohne Eintrag werden verworfen). Die Status der Tickets werden wie beim
/// Typwechsel (`clone_move.rs`) anhand des Workflows des Ersatz-Typs umgesetzt (gleicher Status, sonst gleiche Bezeichnung
/// oder Kategorie, sonst der Anfangsstatus).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeMigration {
    pub to_type: i64,
    pub fields: Vec<FieldMapEntry>,
}

/// Löschmodus für Typen.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeDeleteMode {
    Discard,
    Migrate(TypeMigration),
}

// ------------------------------------------------------------------ Zählabfragen

/// Tickets mit diesem Status.
const STATUS_USAGE: &[&str] = &["SELECT COUNT(*) FROM tickets WHERE status_id = ?1"];
/// Tickets dieses Typs.
const TYPE_USAGE: &[&str] = &["SELECT COUNT(*) FROM tickets WHERE type_id = ?1"];
/// Tickets des Typs mit gesetztem Status (?1 = Typ).
const TYPE_WORKFLOW_USAGE: &[&str] = &["SELECT COUNT(*) FROM tickets WHERE type_id = ?1 AND status_id IS NOT NULL"];
/// Tickets des Typs (?1) mit Status (?2).
const TYPE_STATUS_USAGE: &[&str] = &["SELECT COUNT(*) FROM tickets WHERE type_id = ?1 AND status_id = ?2"];
/// Tickets des Typs (?1) mit einem Eltern-Ticket des Typs (?2).
const PARENT_RULE_USAGE: &[&str] = &["SELECT COUNT(*) FROM tickets c JOIN tickets p ON p.id = c.parent_id WHERE c.type_id = ?1 AND p.type_id = ?2"];
/// Tickets, die einen Wert in diesem Feld haben (nicht die Zahl der Zeilen: ein Mehrfachwert zählt einmal).
const FIELD_USAGE: &[&str] = &["SELECT COUNT(DISTINCT ticket_id) FROM ticket_values WHERE field_id = ?1"];
/// Tickets, deren Feld (?1) diese Dropdown-/Mehrfachauswahl-Option (?2) enthält.
const OPTION_USAGE: &[&str] = &["SELECT COUNT(DISTINCT ticket_id) FROM ticket_values WHERE field_id = ?1 AND num = ?2"];
/// Verknüpfungen dieses Typs.
const LINK_TYPE_USAGE: &[&str] = &["SELECT COUNT(*) FROM links WHERE link_type_id = ?1"];
/// Tickets, deren Feld vom Typ „Version“ auf diese Version zeigt.
const VERSION_USAGE: &[&str] = &["SELECT COUNT(DISTINCT v.ticket_id) FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE f.kind = 'version' AND v.text = CAST(?1 AS TEXT)"];
/// Tickets, deren Feld vom Typ „Meilenstein“ auf diesen Meilenstein zeigt.
const MILESTONE_USAGE: &[&str] = &["SELECT COUNT(DISTINCT v.ticket_id) FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE f.kind = 'milestone' AND v.text = CAST(?1 AS TEXT)"];
/// Zeit-Einträge mit dieser Art der Arbeit.
const WORK_KIND_USAGE: &[&str] = &["SELECT COUNT(*) FROM time_entries WHERE work_kind_id = ?1"];
/// Tickets des Projekts.
const PROJECT_USAGE: &[&str] = &["SELECT COUNT(*) FROM tickets WHERE project_id = ?1"];

/// Zählt die Verwendungen über alle Abfragen des Bereichs `name` (in Tests zusätzlich über die dort registrierten).
fn sum(conn: &Connection, name: &str, queries: &[&str], ids: &[i64]) -> Res<i64> {
    let mut total = 0;
    for q in queries {
        let n: i64 = conn.query_row(q, params_from_iter(ids.iter()), |r| r.get(0))?;
        total += n;
    }
    #[cfg(test)]
    {
        total += test_hook::extra(conn, name, ids)?;
    }
    let _ = name;
    Ok(total)
}

/// Nur für Tests: simuliert die Ticket-Tabellen, indem zusätzliche Zählabfragen je Bereich registriert werden.
#[cfg(test)]
pub(crate) mod test_hook {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static EXTRA: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    }

    /// Registriert eine Zählabfrage für den Bereich (`status`, `type`, `field`, `link_type`, `work_kind`, `project`, …).
    pub fn add(name: &str, sql: &str) {
        EXTRA.with(|e| e.borrow_mut().push((name.to_string(), sql.to_string())));
    }

    pub fn extra(conn: &Connection, name: &str, ids: &[i64]) -> Res<i64> {
        let qs: Vec<String> = EXTRA.with(|e| e.borrow().iter().filter(|(n, _)| n == name).map(|(_, q)| q.clone()).collect());
        let mut total = 0;
        for q in qs {
            let n: i64 = conn.query_row(&q, params_from_iter(ids.iter()), |r| r.get(0))?;
            total += n;
        }
        Ok(total)
    }
}

pub fn usage_count_status(conn: &Connection, status_id: i64) -> Res<i64> {
    sum(conn, "status", STATUS_USAGE, &[status_id])
}

pub fn usage_count_type(conn: &Connection, type_id: i64) -> Res<i64> {
    sum(conn, "type", TYPE_USAGE, &[type_id])
}

/// Tickets des Typs, die einen Status haben (relevant, wenn der Workflow des Typs abgeschaltet wird).
pub fn usage_count_type_workflow(conn: &Connection, type_id: i64) -> Res<i64> {
    sum(conn, "type_workflow", TYPE_WORKFLOW_USAGE, &[type_id])
}

/// Tickets des Typs mit genau diesem Status (relevant, wenn der Status aus dem Workflow des Typs genommen wird).
pub fn usage_count_type_status(conn: &Connection, type_id: i64, status_id: i64) -> Res<i64> {
    sum(conn, "type_status", TYPE_STATUS_USAGE, &[type_id, status_id])
}

/// Tickets des Typs `type_id`, die unter einem Ticket des Typs `parent_type_id` hängen (relevant beim Entfernen einer Eltern-Regel).
pub fn usage_count_parent_rule(conn: &Connection, type_id: i64, parent_type_id: i64) -> Res<i64> {
    sum(conn, "parent_rule", PARENT_RULE_USAGE, &[type_id, parent_type_id])
}

pub fn usage_count_field(conn: &Connection, field_id: i64) -> Res<i64> {
    sum(conn, "field", FIELD_USAGE, &[field_id])
}

/// Werte des Feldes mit genau dieser Dropdown-Option (relevant, wenn ein Wert aus der Liste entfernt wird).
pub fn usage_count_option(conn: &Connection, field_id: i64, option_id: i64) -> Res<i64> {
    sum(conn, "option", OPTION_USAGE, &[field_id, option_id])
}

pub fn usage_count_link_type(conn: &Connection, link_type_id: i64) -> Res<i64> {
    sum(conn, "link_type", LINK_TYPE_USAGE, &[link_type_id])
}

/// Tickets, deren Versions-Feld auf die Version zeigt.
pub fn usage_count_version(conn: &Connection, version_id: i64) -> Res<i64> {
    sum(conn, "version", VERSION_USAGE, &[version_id])
}

/// Tickets, deren Meilenstein-Feld auf den Meilenstein zeigt.
pub fn usage_count_milestone(conn: &Connection, milestone_id: i64) -> Res<i64> {
    sum(conn, "milestone", MILESTONE_USAGE, &[milestone_id])
}

pub fn usage_count_work_kind(conn: &Connection, work_kind_id: i64) -> Res<i64> {
    sum(conn, "work_kind", WORK_KIND_USAGE, &[work_kind_id])
}

/// Tickets des Projekts – so viele gehen beim Löschen des Projekts verloren (für die Warnung der Oberfläche).
pub fn usage_count_project(conn: &Connection, project_id: i64) -> Res<i64> {
    sum(conn, "project", PROJECT_USAGE, &[project_id])
}

/// Ohne Modus darf nur gelöscht werden, was nicht benutzt wird.
pub(crate) fn require_mode(count: i64, has_mode: bool) -> Res<()> {
    if count > 0 && !has_mode { Err(PlanError::InUse { count }) } else { Ok(()) }
}

// ------------------------------------------------------------------ Umhängen/Verwerfen

/// Quelle der Verlaufseinträge, die beim Umhängen entstehen.
fn migration_source() -> Source {
    Source { kind: super::history::SourceKind::Ui, detail: Some("migration".into()) }
}

fn plan_err(e: TicketError) -> PlanError {
    match e {
        TicketError::Plan(p) => p,
        other => PlanError::Db(other.message(crate::state::lang())),
    }
}

fn ids(conn: &Connection, sql: &str, args: &[i64]) -> Res<Vec<i64>> {
    let mut st = conn.prepare(sql)?;
    Ok(st.query_map(params_from_iter(args.iter()), |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
}

/// Ersatzstatus im Workflow eines Typs: der erste To-Do-Status außer `exclude`, sonst irgendein anderer, sonst keiner.
fn fallback_status(conn: &Connection, type_id: i64, exclude: i64) -> Res<Option<i64>> {
    let flow = super::workflow::workflow_statuses(conn, type_id)?;
    let mut first = None;
    for s in flow.into_iter().filter(|s| *s != exclude) {
        if super::statuses::get_status(conn, s)?.category == super::statuses::StatusCategory::Todo {
            return Ok(Some(s));
        }
        first.get_or_insert(s);
    }
    Ok(first)
}

/// Tickets mit Status `from` bekommen `to` (`None`: Status wird geleert). Gehört `to` nicht zum Workflow des Typs eines
/// Tickets, bekommt es stattdessen den Ersatzstatus des Workflows (der erste To-Do-Status, sonst keiner).
pub(crate) fn migrate_tickets_status(conn: &Connection, from: i64, to: Option<i64>) -> Res<()> {
    let now = super::common::now();
    for ty in ids(conn, "SELECT DISTINCT type_id FROM tickets WHERE status_id = ?1", &[from])? {
        let target = match to {
            Some(t) if super::workflow::workflow_statuses(conn, ty)?.contains(&t) => Some(t),
            Some(_) => fallback_status(conn, ty, from)?,
            None => None,
        };
        conn.execute("UPDATE tickets SET status_id = ?1, updated_at = ?2 WHERE type_id = ?3 AND status_id = ?4", params![target, now, ty, from])?;
    }
    Ok(())
}

/// Tickets des Typs `from` werden zu `migration.to_type` (Felder gemäß Zuordnung, Status und Hierarchie wie beim Typwechsel)
/// bzw. gelöscht (`None`).
pub(crate) fn migrate_tickets_type(conn: &Connection, from: i64, migration: Option<&TypeMigration>) -> Res<()> {
    let src = migration_source();
    let tickets = ids(conn, "SELECT id FROM tickets WHERE type_id = ?1 ORDER BY id", &[from])?;
    for t in tickets {
        match migration {
            Some(m) => super::clone_move::migrate_ticket_type(conn, t, m.to_type, &m.fields, &src).map_err(plan_err)?,
            None => {
                super::tickets::delete_ticket(conn, t, &src).map_err(plan_err)?;
            }
        }
    }
    Ok(())
}

/// Werte des Feldes `from` wandern ins Feld `to` bzw. werden verworfen. Hat ein Ticket im Zielfeld schon einen Wert, bleibt
/// dieser; Dropdown-/Mehrfachauswahl-Werte werden über ihre Bezeichnung übersetzt (ohne Entsprechung entfallen sie).
pub(crate) fn migrate_values_field(conn: &Connection, from: i64, to: Option<i64>) -> Res<()> {
    if let Some(to) = to {
        let (src, dst) = (super::fields::get_field(conn, from)?, super::fields::get_field(conn, to)?);
        for ticket in ids(conn, "SELECT DISTINCT ticket_id FROM ticket_values WHERE field_id = ?1", &[from])? {
            let have: i64 = conn.query_row("SELECT COUNT(*) FROM ticket_values WHERE ticket_id = ?1 AND field_id = ?2", [ticket, to], |r| r.get(0))?;
            if have > 0 {
                continue;
            }
            let v = super::values::get_value(conn, ticket, &src).map_err(plan_err)?;
            if let Some(v) = v.and_then(|v| super::values::translate_value(&src, &dst, &v)) {
                super::values::write_value(conn, ticket, &dst, Some(&v)).map_err(plan_err)?;
            }
        }
    }
    conn.execute("DELETE FROM ticket_values WHERE field_id = ?1", [from])?;
    Ok(())
}

/// Werte mit der Dropdown-Option `option_id` des Feldes werden verworfen (bei Mehrfachauswahl nur diese Option).
pub(crate) fn discard_values_option(conn: &Connection, field_id: i64, option_id: i64) -> Res<()> {
    conn.execute("DELETE FROM ticket_values WHERE field_id = ?1 AND num = ?2", [field_id, option_id])?;
    Ok(())
}

/// Status aller Tickets des Typs werden geleert (Workflow abgeschaltet).
pub(crate) fn clear_type_status(conn: &Connection, type_id: i64) -> Res<()> {
    conn.execute("UPDATE tickets SET status_id = NULL, updated_at = ?2 WHERE type_id = ?1 AND status_id IS NOT NULL", params![type_id, super::common::now()])?;
    Ok(())
}

/// Tickets des Typs mit Status `status_id` werden auf den Ersatzstatus des Workflows gesetzt (Status aus dem Workflow
/// genommen): der erste To-Do-Status, sonst ein anderer, sonst keiner.
pub(crate) fn reset_type_status(conn: &Connection, type_id: i64, status_id: i64) -> Res<()> {
    let target = fallback_status(conn, type_id, status_id)?;
    conn.execute("UPDATE tickets SET status_id = ?1, updated_at = ?2 WHERE type_id = ?3 AND status_id = ?4", params![target, super::common::now(), type_id, status_id])?;
    Ok(())
}

/// Verknüpfungen des Typs `from` wandern auf `to` bzw. werden gelöscht (Duplikate entfallen).
pub(crate) fn migrate_links(conn: &Connection, from: i64, to: Option<i64>) -> Res<()> {
    if let Some(to) = to {
        conn.execute("UPDATE OR IGNORE links SET link_type_id = ?1 WHERE link_type_id = ?2", [to, from])?;
    }
    conn.execute("DELETE FROM links WHERE link_type_id = ?1", [from])?;
    Ok(())
}

/// Werte der Felder der Art `field_kind` (`version` oder `milestone`), die auf `from` zeigen, wandern auf `to` bzw. werden gelöscht.
pub(crate) fn migrate_values_release(conn: &Connection, field_kind: &str, from: i64, to: Option<i64>) -> Res<()> {
    let sel = "SELECT v.ticket_id FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE f.kind = ?1 AND v.text = ?2";
    conn.execute(&format!("UPDATE tickets SET updated_at = ?3 WHERE id IN ({sel})"), params![field_kind, from.to_string(), super::common::now()])?;
    let fields = "field_id IN (SELECT id FROM fields WHERE kind = ?1) AND text = ?2";
    match to {
        Some(to) => conn.execute(&format!("UPDATE ticket_values SET text = ?3 WHERE {fields}"), params![field_kind, from.to_string(), to.to_string()])?,
        None => conn.execute(&format!("DELETE FROM ticket_values WHERE {fields}"), params![field_kind, from.to_string()])?,
    };
    Ok(())
}

/// Zeit-Einträge mit Art der Arbeit `from` wandern auf `to` bzw. verlieren die Angabe.
pub(crate) fn migrate_work_kind(conn: &Connection, from: i64, to: Option<i64>) -> Res<()> {
    conn.execute("UPDATE time_entries SET work_kind_id = ?1 WHERE work_kind_id = ?2", params![to, from])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::testutil::db;

    #[test]
    fn an_empty_database_has_no_usage() {
        let c = db();
        assert_eq!(usage_count_status(&c, 1).unwrap(), 0);
        assert_eq!(usage_count_type(&c, 1).unwrap(), 0);
        assert_eq!(usage_count_field(&c, 1).unwrap(), 0);
        assert_eq!(usage_count_link_type(&c, 1).unwrap(), 0);
        assert_eq!(usage_count_work_kind(&c, 1).unwrap(), 0);
        assert_eq!(usage_count_project(&c, 1).unwrap(), 0);
        assert_eq!(usage_count_option(&c, 1, 1).unwrap(), 0);
        assert_eq!(usage_count_type_status(&c, 1, 1).unwrap(), 0);
        assert_eq!(usage_count_type_workflow(&c, 1).unwrap(), 0);
        assert_eq!(usage_count_parent_rule(&c, 1, 2).unwrap(), 0);
    }

    #[test]
    fn counting_sums_up_all_queries_and_binds_parameters() {
        let c = db();
        // zwei Abfragen mit Parametern: Summe, Bindung von ?1/?2
        let n = sum(&c, "x", &["SELECT COUNT(*) FROM projects WHERE system = ?1 AND ?2 = 1", "SELECT COUNT(*) FROM projects WHERE id = ?2 AND system = ?1"], &[1, 1]).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn mode_is_required_only_when_used() {
        assert_eq!(require_mode(0, false), Ok(()));
        assert_eq!(require_mode(3, true), Ok(()));
        assert_eq!(require_mode(3, false), Err(PlanError::InUse { count: 3 }));
    }
}
