//! Integritätsregel beim Löschen: Was benutzt wird (Status, Typ, Feld, Verknüpfungstyp, Art der Arbeit, Projekt), lässt sich
//! nur löschen, wenn man einen Ersatz angibt ([`DeleteMode::Migrate`]) oder die Verwendungen bewusst verwirft
//! ([`DeleteMode::Discard`]). Ohne Modus liefert die API [`PlanError::InUse`] mit der Zahl der Verwendungen.
//!
//! **Hier ist die Nahtstelle für den Ticket-Schritt:** Die `usage_count_*`-Funktionen zählen über Listen von SQL-Abfragen
//! (`*_USAGE`, je mit den Platzhaltern `?1…`), die heute leer sind, weil es `tickets`/`ticket_values` noch nicht gibt. Der
//! Ticket-Schritt trägt dort die Zählabfragen ein und füllt die `migrate_tickets_*`-Funktionen (Werte umhängen bzw. verwerfen).
//! Die Löschfunktionen der Konfiguration rufen diese Stellen bereits an der richtigen Stelle (vor dem Löschen, atomar) auf.

use super::{PlanError, Res};
use rusqlite::{params_from_iter, Connection};

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
/// gemäß `fields` (Felder ohne Eintrag werden verworfen). Die Status der Tickets setzt der Ticket-Schritt anhand des
/// Workflows des Ersatz-Typs um (gleicher Status bleibt, sonst der Anfangsstatus).
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

// ------------------------------------------------------------------ Zählabfragen (Ticket-Schritt trägt sie ein)

/// Tickets mit diesem Status. Später: `SELECT COUNT(*) FROM tickets WHERE status_id = ?1`
const STATUS_USAGE: &[&str] = &[];
/// Tickets dieses Typs. Später: `SELECT COUNT(*) FROM tickets WHERE type_id = ?1`
const TYPE_USAGE: &[&str] = &[];
/// Tickets des Typs mit gesetztem Status (?1 = Typ). Später: `... FROM tickets WHERE type_id = ?1 AND status_id IS NOT NULL`
const TYPE_WORKFLOW_USAGE: &[&str] = &[];
/// Tickets des Typs (?1) mit Status (?2). Später: `... WHERE type_id = ?1 AND status_id = ?2`
const TYPE_STATUS_USAGE: &[&str] = &[];
/// Tickets des Typs (?1) mit einem Eltern-Ticket des Typs (?2).
const PARENT_RULE_USAGE: &[&str] = &[];
/// Gesetzte Werte dieses Feldes. Später: `SELECT COUNT(*) FROM ticket_values WHERE field_id = ?1`
const FIELD_USAGE: &[&str] = &[];
/// Werte des Feldes (?1) mit dieser Dropdown-Option (?2).
const OPTION_USAGE: &[&str] = &[];
/// Verknüpfungen dieses Typs. Später: `SELECT COUNT(*) FROM links WHERE link_type_id = ?1`
const LINK_TYPE_USAGE: &[&str] = &[];
/// Zeit-Einträge mit dieser Art der Arbeit. Später: `SELECT COUNT(*) FROM time_entries WHERE work_kind_id = ?1`
const WORK_KIND_USAGE: &[&str] = &[];
/// Tickets des Projekts. Später: `SELECT COUNT(*) FROM tickets WHERE project_id = ?1`
const PROJECT_USAGE: &[&str] = &[];

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

// ------------------------------------------------------------------ Umhängen/Verwerfen (Ticket-Schritt füllt sie)

/// Tickets mit Status `from` bekommen `to` (`None`: Status wird geleert).
pub(crate) fn migrate_tickets_status(_conn: &Connection, _from: i64, _to: Option<i64>) -> Res<()> {
    Ok(())
}

/// Tickets des Typs `from` werden zu `migration.to_type` (Felder gemäß Zuordnung) bzw. gelöscht (`None`).
pub(crate) fn migrate_tickets_type(_conn: &Connection, _from: i64, _migration: Option<&TypeMigration>) -> Res<()> {
    Ok(())
}

/// Werte des Feldes `from` wandern ins Feld `to` bzw. werden verworfen.
pub(crate) fn migrate_values_field(_conn: &Connection, _from: i64, _to: Option<i64>) -> Res<()> {
    Ok(())
}

/// Werte mit der Dropdown-Option `option_id` des Feldes werden verworfen.
pub(crate) fn discard_values_option(_conn: &Connection, _field_id: i64, _option_id: i64) -> Res<()> {
    Ok(())
}

/// Status aller Tickets des Typs werden geleert (Workflow abgeschaltet).
pub(crate) fn clear_type_status(_conn: &Connection, _type_id: i64) -> Res<()> {
    Ok(())
}

/// Tickets des Typs mit Status `status_id` werden auf den Anfangsstatus gesetzt (Status aus dem Workflow genommen).
pub(crate) fn reset_type_status(_conn: &Connection, _type_id: i64, _status_id: i64) -> Res<()> {
    Ok(())
}

/// Verknüpfungen des Typs `from` wandern auf `to` bzw. werden gelöscht.
pub(crate) fn migrate_links(_conn: &Connection, _from: i64, _to: Option<i64>) -> Res<()> {
    Ok(())
}

/// Zeit-Einträge mit Art der Arbeit `from` wandern auf `to` bzw. verlieren die Angabe.
pub(crate) fn migrate_work_kind(_conn: &Connection, _from: i64, _to: Option<i64>) -> Res<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::testutil::db;

    #[test]
    fn nothing_is_in_use_yet() {
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
