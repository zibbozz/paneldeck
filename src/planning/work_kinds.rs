//! „Art der Arbeit“ je Projekt (Entwicklung, Test, Planung, Schärfung – übersetzbare Standardliste, frei änderbar).

use super::common::{self, atomic, clean_name, ensure_name_free, rename_value, validate_ident};
use super::integrity::{self, DeleteMode};
use super::names;
use super::projects::editable;
use super::{PlanError, Res};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq)]
pub struct WorkKind {
    pub id: i64,
    pub project_id: i64,
    pub key: Option<String>,
    pub name: Option<String>,
    pub position: i64,
}

impl WorkKind {
    pub fn display(&self, lang: Lang) -> String {
        names::display_name(self.key.as_deref(), self.name.as_deref(), lang)
    }
}

/// Die mitgelieferte Liste (Schlüssel, in dieser Reihenfolge). Der erste Eintrag ist der Standard im Zeit-Dialog.
pub const DEFAULT_KEYS: [&str; 4] = ["development", "testing", "planning", "refinement"];

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<WorkKind> {
    Ok(WorkKind { id: r.get(0)?, project_id: r.get(1)?, key: r.get(2)?, name: r.get(3)?, position: r.get(4)? })
}

pub fn get_work_kind(conn: &Connection, id: i64) -> Res<WorkKind> {
    conn.query_row("SELECT id, project_id, key, name, position FROM work_kinds WHERE id = ?1", [id], from_row).optional()?.ok_or(PlanError::NotFound)
}

pub fn list_work_kinds(conn: &Connection, project_id: i64) -> Res<Vec<WorkKind>> {
    let mut st = conn.prepare("SELECT id, project_id, key, name, position FROM work_kinds WHERE project_id = ?1 ORDER BY position, id")?;
    Ok(st.query_map([project_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Legt eine Art der Arbeit an: entweder mit eigenem Namen oder (Standardliste) mit Schlüssel.
pub fn create_work_kind(conn: &Connection, project_id: i64, key: Option<&str>, name: Option<&str>) -> Res<i64> {
    editable(conn, project_id)?;
    if let Some(k) = key {
        validate_ident(k)?;
    }
    let name = match name {
        Some(s) if !s.trim().is_empty() => Some(clean_name(s)?),
        _ => None,
    };
    if key.is_none() && name.is_none() {
        return Err(PlanError::NameEmpty);
    }
    atomic(conn, || {
        ensure_name_free(conn, "work_kinds", "project_id = ?1", &[project_id], key, name.as_deref(), None)?;
        let pos = common::next_position(conn, "work_kinds", "project_id = ?1", &[project_id])?;
        conn.execute("INSERT INTO work_kinds (project_id, key, name, position) VALUES (?1, ?2, ?3, ?4)", params![project_id, key, name, pos])?;
        Ok(conn.last_insert_rowid())
    })
}

/// Umbenennen; leer = Standardname (nur bei Einträgen mit Schlüssel).
pub fn rename_work_kind(conn: &Connection, id: i64, new_name: &str) -> Res<()> {
    let cur = get_work_kind(conn, id)?;
    editable(conn, cur.project_id)?;
    let name = rename_value(cur.key.as_deref(), new_name)?;
    atomic(conn, || {
        ensure_name_free(conn, "work_kinds", "project_id = ?1", &[cur.project_id], cur.key.as_deref(), name.as_deref(), Some(id))?;
        conn.execute("UPDATE work_kinds SET name = ?1 WHERE id = ?2", params![name, id])?;
        Ok(())
    })
}

pub fn reorder_work_kinds(conn: &Connection, project_id: i64, ids: &[i64]) -> Res<()> {
    editable(conn, project_id)?;
    common::reorder(conn, "work_kinds", "project_id = ?1", &[project_id], ids)
}

/// Zeit-Einträge mit dieser Art der Arbeit.
pub fn usage(conn: &Connection, id: i64) -> Res<i64> {
    get_work_kind(conn, id)?;
    integrity::usage_count_work_kind(conn, id)
}

/// Löscht eine Art der Arbeit; benutzte nur mit Modus (`Migrate` auf eine andere Art desselben Projekts oder `Discard`).
pub fn delete_work_kind(conn: &Connection, id: i64, mode: Option<DeleteMode>) -> Res<()> {
    let cur = get_work_kind(conn, id)?;
    editable(conn, cur.project_id)?;
    let count = integrity::usage_count_work_kind(conn, id)?;
    integrity::require_mode(count, mode.is_some())?;
    let to = match &mode {
        Some(DeleteMode::Migrate { to }) => {
            let t = get_work_kind(conn, *to).map_err(|_| PlanError::ReplacementInvalid)?;
            if t.id == id || t.project_id != cur.project_id {
                return Err(PlanError::ReplacementInvalid);
            }
            Some(t.id)
        }
        _ => None,
    };
    atomic(conn, || {
        integrity::migrate_work_kind(conn, id, to)?;
        conn.execute("DELETE FROM work_kinds WHERE id = ?1", [id])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::testutil::db;

    #[test]
    fn manages_work_kinds() {
        let c = db();
        let p = create_project(&c, &NewProject::new("AA", "P", "#112233")).unwrap();
        let q = create_project(&c, &NewProject::new("BB", "Q", "#112233")).unwrap();
        let a = create_work_kind(&c, p, Some("development"), None).unwrap();
        let b = create_work_kind(&c, p, None, Some("Review")).unwrap();
        let other = create_work_kind(&c, q, None, Some("Review")).unwrap();
        let l = list_work_kinds(&c, p).unwrap();
        assert_eq!(l.iter().map(|w| w.id).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!((l[0].display(Lang::De), l[0].display(Lang::En)), ("Entwicklung".into(), "Development".into()));
        // Fehler
        assert_eq!(create_work_kind(&c, p, None, Some("review")), Err(PlanError::NameTaken));
        assert_eq!(create_work_kind(&c, p, None, Some("Entwicklung")), Err(PlanError::NameTaken), "gleich der übersetzten Standardart");
        assert_eq!(create_work_kind(&c, p, None, Some(" ")), Err(PlanError::NameEmpty));
        assert_eq!(create_work_kind(&c, p, Some("Bad Key"), None), Err(PlanError::KeyInvalid));
        assert_eq!(create_work_kind(&c, 999, None, Some("X")), Err(PlanError::NotFound));
        let todo = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(create_work_kind(&c, todo, None, Some("X")), Err(PlanError::SystemProtected));
        // Umbenennen und zurücksetzen
        rename_work_kind(&c, a, "Coden").unwrap();
        assert_eq!(get_work_kind(&c, a).unwrap().display(Lang::De), "Coden");
        rename_work_kind(&c, a, "").unwrap();
        assert_eq!(get_work_kind(&c, a).unwrap().display(Lang::De), "Entwicklung");
        assert_eq!(rename_work_kind(&c, b, ""), Err(PlanError::NameEmpty));
        assert_eq!(rename_work_kind(&c, b, "development"), Err(PlanError::NameTaken));
        // Reihenfolge
        reorder_work_kinds(&c, p, &[b, a]).unwrap();
        assert_eq!(list_work_kinds(&c, p).unwrap().iter().map(|w| w.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(reorder_work_kinds(&c, p, &[b]), Err(PlanError::ReorderMismatch));
        // Löschen
        assert_eq!(delete_work_kind(&c, a, Some(DeleteMode::Migrate { to: a })), Err(PlanError::ReplacementInvalid));
        assert_eq!(delete_work_kind(&c, a, Some(DeleteMode::Migrate { to: other })), Err(PlanError::ReplacementInvalid), "anderes Projekt");
        delete_work_kind(&c, a, Some(DeleteMode::Migrate { to: b })).unwrap();
        delete_work_kind(&c, b, None).unwrap();
        assert!(list_work_kinds(&c, p).unwrap().is_empty());
        assert_eq!(list_work_kinds(&c, q).unwrap().len(), 1);
        assert_eq!(usage(&c, other).unwrap(), 0);
    }
}
