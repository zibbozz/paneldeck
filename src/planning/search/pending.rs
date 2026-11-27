//! Sprints, Versionen und Meilensteine in der Abfrage: Anschluss an die Datenschicht `planning::sprints`.
//!
//! Die Datei heißt noch `pending` (früher die Einhängepunkte); die Funktionen hier liefern jetzt die SQL-Bausteine für `compile.rs`
//! und die Namen für das Schema.
//!
//! # Entscheidungen
//!
//! * **Schalter:** Nutzt kein Projekt des Bereichs Sprints (`use_sprints`), gibt es das Feld `sprint` nicht im Schema; ebenso fehlen
//!   Felder der Art Version/Meilenstein, wenn kein Projekt des Bereichs `use_versions` eingeschaltet hat (die Abfrage meldet dann
//!   den üblichen Fehler „unbekanntes Feld“). Im Bereich „alle Projekte“ gelten die Schalter je Projekt: Tickets eines Projekts mit
//!   ausgeschaltetem Schalter haben ein leeres Feld (die gespeicherten Daten bleiben erhalten).
//! * **`sprint = X`:** Das Ticket hat eine Zugehörigkeitszeile im Sprint X, die **nicht entfernt** wurde (`removed_at IS NULL`).
//!   Das gilt für geplante, aktive **und abgeschlossene** Sprints (beim Abschließen bleiben die Zeilen stehen, auch die der
//!   unerledigten Tickets – wie in `sprint_content`). Entfernte Tickets (aus einem aktiven Sprint genommen) zählen nicht mehr. Ein
//!   Ticket kann so in mehreren Sprints „sein“ (abgeschlossener Sprint und der geplante, in den es verschoben wurde).
//! * **`sprint IS EMPTY`:** das **Backlog** im Sinn von `sprints::backlog`: das Ticket steckt in keinem **offenen** (geplanten oder
//!   aktiven) Sprint. Ein erledigtes Ticket nur aus einem abgeschlossenen Sprint ist also „leer“ (und auch noch `sprint = <alter
//!   Sprint>`); `IS NOT EMPTY` heißt „liegt in einem offenen Sprint“.
//! * **Namen:** Sprint- und Versionsnamen werden ohne Beachtung der Schreibweise verglichen. Im Bereich „alle Projekte“ trifft ein
//!   Name alle Sprints/Versionen dieses Namens in ihren Projekten (die Ticket-Zugehörigkeit bleibt je Projekt).
//! * **`activeSprint()`:** am Feld `sprint` der aktive Sprint **jedes** Projekts des Bereichs (genau über die Kennung, auch wenn ein
//!   anderes Projekt einen gleichnamigen abgeschlossenen Sprint hat). An anderen Feldern (z. B. `summary = activeSprint()`) der
//!   Name; ohne aktiven Sprint ergibt die Funktion keinen Wert (nichts trifft, `!=` trifft alles).
//! * **`ORDER BY sprint`:** nach dem jüngsten Sprint (größte Kennung = zuletzt angelegt), in dem das Ticket (nicht entfernt) steckt;
//!   Tickets ohne Sprint stehen zuletzt. Versionen/Meilensteine sortieren nach der eingestellten Reihenfolge (`position`), leere zuletzt.
//! * **Board „nur aktiver Sprint“:** Projekt mit Sprints und aktivem Sprint: dessen Tickets; mit Sprints, aber ohne aktiven Sprint:
//!   keine Karten; Projekt **ohne** Sprints: keine Einschränkung (der Schalter ist dort wirkungslos).

use super::compile::{Cx, Mode};
use super::error::SRes;
use super::Scope;
use crate::query::CmpOp;
use rusqlite::{params, Connection};

/// Marke eines Werts aus `activeSprint()` am Feld `sprint`: gefolgt von der Sprint-Kennung (siehe [`active_sprint_name`]).
const ACTIVE_MARK: &str = "\u{0}active:";

fn scope_id(scope: Scope) -> Option<i64> {
    match scope {
        Scope::Project(p) => Some(p),
        Scope::All => None,
    }
}

fn push_unique(list: &mut Vec<String>, name: String) {
    if !list.iter().any(|x| x.to_lowercase() == name.to_lowercase()) {
        list.push(name);
    }
}

/// Namen der Sprints im Bereich (nur Projekte mit eingeschalteten Sprints), in der Reihenfolge Projekt, Anlegen; ohne Doppelte.
/// `only`: Sicht eines Agenten (#386) – nur diese Projekte.
pub(super) fn sprint_names(conn: &Connection, scope: Scope, only: Option<&[i64]>) -> SRes<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT s.name, p.id FROM sprints s JOIN projects p ON p.id = s.project_id
         WHERE p.system = 0 AND p.use_sprints = 1 AND (?1 IS NULL OR p.id = ?1) ORDER BY p.position, p.id, s.id",
    )?;
    let rows = st.query_map(params![scope_id(scope)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (n, pid) in rows {
        if only.is_none_or(|o| o.contains(&pid)) {
            push_unique(&mut out, n);
        }
    }
    Ok(out)
}

/// Die aktiven Sprints im Bereich (Kennung, Name) – nur Projekte mit eingeschalteten Sprints.
pub(super) fn active_sprints(conn: &Connection, scope: Scope, only: Option<&[i64]>) -> SRes<Vec<(i64, String)>> {
    let mut st = conn.prepare(
        "SELECT s.id, s.name, p.id FROM sprints s JOIN projects p ON p.id = s.project_id
         WHERE s.state = 'active' AND p.system = 0 AND p.use_sprints = 1 AND (?1 IS NULL OR p.id = ?1) ORDER BY s.id",
    )?;
    let rows = st.query_map(params![scope_id(scope)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter(|(_, _, p)| only.is_none_or(|o| o.contains(p))).map(|(i, n, _)| (i, n)).collect())
}

/// Nutzt mindestens ein Projekt des Bereichs Versionen/Meilensteine?
pub(super) fn versions_enabled(conn: &Connection, scope: Scope, only: Option<&[i64]>) -> SRes<bool> {
    let mut st = conn.prepare("SELECT p.id FROM projects p WHERE p.system = 0 AND p.use_versions = 1 AND (?1 IS NULL OR p.id = ?1)")?;
    let ids = st.query_map(params![scope_id(scope)], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ids.iter().any(|p| only.is_none_or(|o| o.contains(p))))
}

/// Namen der Versionen bzw. Meilensteine im Bereich (nur Projekte mit eingeschalteten Versionen), nach Projekt und Reihenfolge; ohne Doppelte.
pub(super) fn version_names(conn: &Connection, scope: Scope, milestones: bool, only: Option<&[i64]>) -> SRes<Vec<String>> {
    let table = if milestones { "milestones" } else { "versions" };
    let mut st = conn.prepare(&format!(
        "SELECT r.name, p.id FROM {table} r JOIN projects p ON p.id = r.project_id
         WHERE p.system = 0 AND p.use_versions = 1 AND (?1 IS NULL OR p.id = ?1) ORDER BY p.position, p.id, r.position, r.id"
    ))?;
    let rows = st.query_map(params![scope_id(scope)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (n, pid) in rows {
        if only.is_none_or(|o| o.contains(&pid)) {
            push_unique(&mut out, n);
        }
    }
    Ok(out)
}

/// Unterabfrage „das Ticket hat eine nicht entfernte Zeile in einem Sprint seines Projekts“ (ohne `EXISTS`, ohne Zusatzbedingung).
const MEMBERSHIP: &str = "SELECT 1 FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id
     WHERE st.ticket_id = t.id AND st.removed_at IS NULL AND s.project_id = t.project_id";

/// SQL (immer 0/1, nie NULL) für `sprint <op> werte`, mit `Eq` als „eines der Werte“. `None` als Operator = `IS EMPTY` (der Aufrufer
/// verneint für `IS NOT EMPTY`). Die Werte sind Namen oder Marken von `activeSprint()`.
pub(super) fn sprint_condition(cx: &mut Cx<'_>, op: Option<CmpOp>, values: &[String]) -> SRes<String> {
    let Some(op) = op else {
        // Backlog: in keinem offenen Sprint (oder das Projekt nutzt keine Sprints)
        return Ok(format!("NOT (p.use_sprints = 1 AND EXISTS ({MEMBERSHIP} AND s.state IN ('planned', 'active')))"));
    };
    let mode = match op {
        CmpOp::Eq => Mode::Eq,
        CmpOp::Contains => Mode::Contains,
        _ => return Ok("0".to_string()),
    };
    let mut names: Vec<String> = Vec::new();
    let mut ids: Vec<i64> = Vec::new();
    for v in values {
        match v.strip_prefix(ACTIVE_MARK).and_then(|i| i.parse::<i64>().ok()) {
            Some(id) => {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            None => names.push(v.clone()),
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !names.is_empty() {
        parts.push(cx.ci_any("s.name", &names, mode));
    }
    if !ids.is_empty() {
        parts.push(cx.ids_clause("s.id", &ids));
    }
    if parts.is_empty() {
        return Ok("0".to_string());
    }
    Ok(format!("(p.use_sprints = 1 AND EXISTS ({MEMBERSHIP} AND ({})))", parts.join(" OR ")))
}

/// Sortier-Ausdruck für `ORDER BY sprint` (kleiner = früher); `NULL` ohne Sprint.
pub(super) fn sprint_order(_cx: &mut Cx<'_>) -> SRes<String> {
    Ok("(SELECT MAX(s.id) FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id
        WHERE st.ticket_id = t.id AND st.removed_at IS NULL AND s.project_id = t.project_id AND p.use_sprints = 1)"
        .to_string())
}

/// Werte von `activeSprint()`: am Feld `sprint` (`for_sprint_field`) die Marken der aktiven Sprints (so trifft die Abfrage genau diese
/// Sprints, nicht gleichnamige anderer Projekte), sonst deren Namen. Ohne aktiven Sprint: keine Werte.
pub(super) fn active_sprint_name(cx: &mut Cx<'_>, for_sprint_field: bool) -> SRes<Vec<String>> {
    let list = &cx.s.active_sprints;
    if for_sprint_field {
        return Ok(list.iter().map(|(id, _)| format!("{ACTIVE_MARK}{id}")).collect());
    }
    let mut out = Vec::new();
    for (_, n) in list {
        push_unique(&mut out, n.clone());
    }
    Ok(out)
}

/// Bedingung (SQL, 0/1) „Ticket gehört zum aktiven Sprint des Projekts“ (Board-Schalter). Projekte ohne Sprints: keine Einschränkung.
pub(crate) fn active_sprint_condition(cx: &mut Cx<'_>, project_id: i64) -> SRes<String> {
    let pid = cx.int(project_id);
    Ok(format!(
        "(p.use_sprints = 0 OR EXISTS (SELECT 1 FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id
          WHERE st.ticket_id = t.id AND st.removed_at IS NULL AND s.state = 'active' AND s.project_id = {pid}))"
    ))
}

/// Tabelle der Einträge.
fn release_table(milestone: bool) -> &'static str {
    if milestone { "milestones" } else { "versions" }
}

/// SQL (0/1) für ein Feld der Art Version/Meilenstein (`None` als Operator = `IS EMPTY`). Im Feld steht die Kennung des Eintrags
/// (`ticket_values.text`); verglichen wird über den Namen des Eintrags im Projekt des Tickets. Projekte ohne `use_versions` zählen als leer.
pub(super) fn version_condition(cx: &mut Cx<'_>, field_ids: &[i64], milestone: bool, op: Option<CmpOp>, values: &[String]) -> SRes<String> {
    let table = release_table(milestone);
    let Some(op) = op else {
        let has = cx.exists_v(field_ids, "COALESCE(v.text, '') <> ''");
        return Ok(format!("NOT (p.use_versions = 1 AND {has})"));
    };
    let mode = match op {
        CmpOp::Eq => Mode::Eq,
        CmpOp::Contains => Mode::Contains,
        _ => return Ok("0".to_string()),
    };
    if values.is_empty() {
        return Ok("0".to_string());
    }
    let by_name = cx.ci_any("r.name", values, mode);
    let cond = format!("v.text IN (SELECT CAST(r.id AS TEXT) FROM {table} r WHERE r.project_id = t.project_id AND {by_name})");
    let has = cx.exists_v(field_ids, &cond);
    Ok(format!("(p.use_versions = 1 AND {has})"))
}

/// Sortier-Ausdrücke für `ORDER BY <Versionsfeld>`: nach der Reihenfolge der Einträge im Projekt (danach Kennung); `NULL` ohne Wert.
pub(super) fn version_order(cx: &mut Cx<'_>, field_ids: &[i64], milestone: bool) -> SRes<Vec<String>> {
    let table = release_table(milestone);
    let f = cx.ids_clause("+v.field_id", field_ids);
    let one = |col: &str| {
        format!(
            "(SELECT MIN(r.{col}) FROM ticket_values v JOIN {table} r ON CAST(r.id AS TEXT) = v.text AND r.project_id = t.project_id
              WHERE v.ticket_id = t.id AND {f} AND p.use_versions = 1)"
        )
    };
    Ok(vec![one("position"), one("id")])
}
