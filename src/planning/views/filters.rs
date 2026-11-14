//! Gespeicherte Filter: eine benannte JQL-Abfrage, entweder global (`project_id = NULL`) oder je Projekt. Der Name ist je
//! Bereich eindeutig (ohne Groß-/Kleinschreibung); ein Projekt-Filter und ein globaler Filter dürfen gleich heißen.
//! Die Abfrage wird hier nur als Text gespeichert (Länge begrenzt); geprüft wird sie vom Validator der Abfragesprache.
//!
//! Beim Löschen eines Filters wird er aus den Quellen der Boards entfernt (`boards::prune_board_refs`).

use super::boards::{prune_all_board_refs, prune_board_refs};
use super::{atomic, clean_name, clean_text, ensure_name_free, Res, ViewError, MAX_JQL_CHARS};
use crate::planning::common;
use crate::planning::projects::editable;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq)]
pub struct SavedFilter {
    pub id: i64,
    /// `None` = globaler Filter (für alle Projekte).
    pub project_id: Option<i64>,
    pub name: String,
    pub jql: String,
    pub position: i64,
}

#[derive(Debug, Clone)]
pub struct NewFilter {
    pub project_id: Option<i64>,
    pub name: String,
    pub jql: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateFilter {
    pub name: Option<String>,
    pub jql: Option<String>,
}

const COLS: &str = "id, project_id, name, jql, position";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<SavedFilter> {
    Ok(SavedFilter { id: r.get(0)?, project_id: r.get(1)?, name: r.get(2)?, jql: r.get(3)?, position: r.get(4)? })
}

/// `WHERE`-Teil und Parameter des Bereichs (Projekt oder global).
fn scope(project_id: Option<i64>) -> (&'static str, Vec<i64>) {
    match project_id {
        Some(p) => ("project_id = ?1", vec![p]),
        None => ("project_id IS NULL", vec![]),
    }
}

pub fn get_filter(conn: &Connection, id: i64) -> Res<SavedFilter> {
    conn.query_row(&format!("SELECT {COLS} FROM saved_filters WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(ViewError::not_found)
}

/// Die Filter genau dieses Bereichs (`None` = nur die globalen).
pub fn list_filters(conn: &Connection, project_id: Option<i64>) -> Res<Vec<SavedFilter>> {
    let (w, p) = scope(project_id);
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM saved_filters WHERE {w} ORDER BY position, id"))?;
    Ok(st.query_map(rusqlite::params_from_iter(p.iter()), from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Alle im Projekt wählbaren Filter: erst die des Projekts, dann die globalen.
pub fn list_filters_for_project(conn: &Connection, project_id: i64) -> Res<Vec<SavedFilter>> {
    let mut out = list_filters(conn, Some(project_id))?;
    out.extend(list_filters(conn, None)?);
    Ok(out)
}

fn clean_jql(s: &str) -> Res<String> {
    clean_text(s, MAX_JQL_CHARS)
}

fn check_scope(conn: &Connection, project_id: Option<i64>) -> Res<()> {
    if let Some(p) = project_id {
        editable(conn, p)?;
    }
    Ok(())
}

pub fn create_filter(conn: &Connection, n: &NewFilter) -> Res<i64> {
    check_scope(conn, n.project_id)?;
    let name = clean_name(&n.name)?;
    let jql = clean_jql(&n.jql)?;
    atomic(conn, || {
        let (w, p) = scope(n.project_id);
        ensure_name_free(conn, "saved_filters", w, &p, &name, None)?;
        let pos = common::next_position(conn, "saved_filters", w, &p)?;
        conn.execute("INSERT INTO saved_filters (project_id, name, jql, position) VALUES (?1, ?2, ?3, ?4)", params![n.project_id, name, jql, pos])?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_filter(conn: &Connection, id: i64, u: &UpdateFilter) -> Res<()> {
    let cur = get_filter(conn, id)?;
    let name = match &u.name {
        Some(s) => clean_name(s)?,
        None => cur.name.clone(),
    };
    let jql = match &u.jql {
        Some(s) => clean_jql(s)?,
        None => cur.jql.clone(),
    };
    atomic(conn, || {
        let (w, p) = scope(cur.project_id);
        ensure_name_free(conn, "saved_filters", w, &p, &name, Some(id))?;
        conn.execute("UPDATE saved_filters SET name = ?1, jql = ?2 WHERE id = ?3", params![name, jql, id])?;
        Ok(())
    })
}

/// Neue Reihenfolge der Filter eines Bereichs; `ids` muss genau dessen Filter enthalten.
pub fn reorder_filters(conn: &Connection, project_id: Option<i64>, ids: &[i64]) -> Res<()> {
    let (w, p) = scope(project_id);
    Ok(common::reorder(conn, "saved_filters", w, &p, ids)?)
}

/// Löscht den Filter und entfernt ihn aus den Quellen der Boards (bei einem globalen Filter in allen Projekten).
pub fn delete_filter(conn: &Connection, id: i64) -> Res<()> {
    let f = get_filter(conn, id)?;
    atomic(conn, || {
        conn.execute("DELETE FROM saved_filters WHERE id = ?1", [id])?;
        match f.project_id {
            Some(p) => prune_board_refs(conn, p)?,
            None => prune_all_board_refs(conn)?,
        };
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::delete_project;
    use crate::planning::views::testutil::{db, project};
    use crate::planning::PlanError;

    fn new(p: Option<i64>, name: &str, jql: &str) -> NewFilter {
        NewFilter { project_id: p, name: name.into(), jql: jql.into() }
    }

    #[test]
    fn creates_reads_lists_and_orders() {
        let c = db();
        let p = project(&c, "AA", false);
        let a = create_filter(&c, &new(Some(p), "  Offene   Bugs ", "  type = Defect AND status != Done ")).unwrap();
        let b = create_filter(&c, &new(Some(p), "Meine", "")).unwrap();
        let g = create_filter(&c, &new(None, "Global", "favorite")).unwrap();
        let f = get_filter(&c, a).unwrap();
        assert_eq!((f.name.as_str(), f.jql.as_str(), f.project_id, f.position), ("Offene Bugs", "type = Defect AND status != Done", Some(p), 0));
        assert_eq!(list_filters(&c, Some(p)).unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(list_filters(&c, None).unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![g]);
        assert_eq!(list_filters_for_project(&c, p).unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![a, b, g]);
        assert_eq!(get_filter(&c, 9999), Err(ViewError::not_found()));
    }

    #[test]
    fn names_are_unique_per_scope() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        create_filter(&c, &new(Some(p), "Alle", "")).unwrap();
        assert_eq!(create_filter(&c, &new(Some(p), "alle", "")), Err(PlanError::NameTaken.into()));
        assert!(create_filter(&c, &new(Some(q), "Alle", "")).is_ok(), "anderes Projekt");
        assert!(create_filter(&c, &new(None, "Alle", "")).is_ok(), "global ist ein eigener Bereich");
        assert_eq!(create_filter(&c, &new(None, "ALLE", "")), Err(PlanError::NameTaken.into()));
        assert!(create_filter(&c, &new(Some(p), "Über", "")).is_ok());
        assert_eq!(create_filter(&c, &new(Some(p), "ÜBER", "")), Err(PlanError::NameTaken.into()));
    }

    #[test]
    fn validates_input() {
        let c = db();
        let p = project(&c, "AA", false);
        assert_eq!(create_filter(&c, &new(Some(p), "  ", "x")), Err(PlanError::NameEmpty.into()));
        assert_eq!(create_filter(&c, &new(Some(p), &"n".repeat(81), "x")), Err(PlanError::NameTooLong { max: 80 }.into()));
        assert_eq!(create_filter(&c, &new(Some(p), "A", &"j".repeat(MAX_JQL_CHARS + 1))), Err(ViewError::TextTooLong { max: MAX_JQL_CHARS }));
        assert!(create_filter(&c, &new(Some(p), "A", &"j".repeat(MAX_JQL_CHARS))).is_ok());
        assert_eq!(create_filter(&c, &new(Some(9999), "A", "")), Err(ViewError::not_found()));
        let sys = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(create_filter(&c, &new(Some(sys), "A", "")), Err(PlanError::SystemProtected.into()));
    }

    #[test]
    fn updates_name_and_query() {
        let c = db();
        let p = project(&c, "AA", false);
        let a = create_filter(&c, &new(Some(p), "A", "x")).unwrap();
        let b = create_filter(&c, &new(Some(p), "B", "y")).unwrap();
        update_filter(&c, a, &UpdateFilter { jql: Some(" z ".into()), ..Default::default() }).unwrap();
        assert_eq!(get_filter(&c, a).unwrap().jql, "z");
        assert_eq!(get_filter(&c, a).unwrap().name, "A");
        update_filter(&c, a, &UpdateFilter { name: Some("a".into()), ..Default::default() }).unwrap();
        assert_eq!(update_filter(&c, a, &UpdateFilter { name: Some("b".into()), ..Default::default() }), Err(PlanError::NameTaken.into()));
        assert_eq!(get_filter(&c, a).unwrap().name, "a");
        assert_eq!(get_filter(&c, b).unwrap().name, "B");
        assert_eq!(update_filter(&c, 9999, &UpdateFilter::default()), Err(ViewError::not_found()));
    }

    #[test]
    fn reorders_per_scope() {
        let c = db();
        let p = project(&c, "AA", false);
        let a = create_filter(&c, &new(Some(p), "A", "")).unwrap();
        let b = create_filter(&c, &new(Some(p), "B", "")).unwrap();
        let g1 = create_filter(&c, &new(None, "G1", "")).unwrap();
        let g2 = create_filter(&c, &new(None, "G2", "")).unwrap();
        reorder_filters(&c, Some(p), &[b, a]).unwrap();
        reorder_filters(&c, None, &[g2, g1]).unwrap();
        assert_eq!(list_filters(&c, Some(p)).unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(list_filters(&c, None).unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![g2, g1]);
        assert_eq!(reorder_filters(&c, Some(p), &[a, g1]), Err(PlanError::ReorderMismatch.into()));
        assert_eq!(reorder_filters(&c, None, &[g1]), Err(PlanError::ReorderMismatch.into()));
    }

    #[test]
    fn deletes_and_cascades_with_the_project() {
        let c = db();
        let p = project(&c, "AA", false);
        let a = create_filter(&c, &new(Some(p), "A", "")).unwrap();
        let g = create_filter(&c, &new(None, "G", "")).unwrap();
        delete_filter(&c, a).unwrap();
        assert_eq!(delete_filter(&c, a), Err(ViewError::not_found()));
        let b = create_filter(&c, &new(Some(p), "B", "")).unwrap();
        delete_project(&c, p).unwrap();
        assert_eq!(get_filter(&c, b), Err(ViewError::not_found()));
        assert!(get_filter(&c, g).is_ok(), "globale Filter bleiben");
    }
}
