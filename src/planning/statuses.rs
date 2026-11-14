//! Status-Pool eines Projekts (Name, Farbe, Kategorie To Do / In Progress / Done). Welche Status ein Typ nutzt und welche
//! Übergänge erlaubt sind, steht in `workflow.rs`.

use super::common::{self, atomic, clean_name, ensure_name_free, normalize_color, rename_value, validate_ident};
use super::integrity::{self, DeleteMode};
use super::names;
use super::projects::editable;
use super::{PlanError, Res};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusCategory {
    Todo,
    Progress,
    Done,
}

impl StatusCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            StatusCategory::Todo => "todo",
            StatusCategory::Progress => "progress",
            StatusCategory::Done => "done",
        }
    }

    pub fn parse(s: &str) -> StatusCategory {
        match s {
            "progress" => StatusCategory::Progress,
            "done" => StatusCategory::Done,
            _ => StatusCategory::Todo,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub id: i64,
    pub project_id: i64,
    pub key: Option<String>,
    pub name: Option<String>,
    pub color: String,
    pub category: StatusCategory,
    pub position: i64,
}

impl Status {
    pub fn display(&self, lang: Lang) -> String {
        names::display_name(self.key.as_deref(), self.name.as_deref(), lang)
    }
}

#[derive(Debug, Clone)]
pub struct NewStatus {
    pub project_id: i64,
    /// Standard-Schlüssel für die Übersetzung (nur für mitgelieferte Status); sonst `None`.
    pub key: Option<String>,
    /// Eigener Name (Pflicht, wenn kein Schlüssel angegeben ist).
    pub name: Option<String>,
    pub color: String,
    pub category: StatusCategory,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateStatus {
    /// Umbenennen; ein leerer Text setzt auf den Standardnamen zurück (nur bei vorhandenem Schlüssel).
    pub name: Option<String>,
    pub color: Option<String>,
    pub category: Option<StatusCategory>,
}

const COLS: &str = "id, project_id, key, name, color, category, position";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Status> {
    Ok(Status { id: r.get(0)?, project_id: r.get(1)?, key: r.get(2)?, name: r.get(3)?, color: r.get(4)?, category: StatusCategory::parse(&r.get::<_, String>(5)?), position: r.get(6)? })
}

pub fn get_status(conn: &Connection, id: i64) -> Res<Status> {
    conn.query_row(&format!("SELECT {COLS} FROM statuses WHERE id = ?1"), [id], from_row).optional()?.ok_or(PlanError::NotFound)
}

pub fn list_statuses(conn: &Connection, project_id: i64) -> Res<Vec<Status>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM statuses WHERE project_id = ?1 ORDER BY position, id"))?;
    Ok(st.query_map([project_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

pub fn create_status(conn: &Connection, n: &NewStatus) -> Res<i64> {
    editable(conn, n.project_id)?;
    let color = normalize_color(&n.color)?;
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
    atomic(conn, || {
        ensure_name_free(conn, "statuses", "project_id = ?1", &[n.project_id], n.key.as_deref(), name.as_deref(), None)?;
        let pos = common::next_position(conn, "statuses", "project_id = ?1", &[n.project_id])?;
        conn.execute("INSERT INTO statuses (project_id, key, name, color, category, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![n.project_id, n.key, name, color, n.category.as_str(), pos])?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_status(conn: &Connection, id: i64, u: &UpdateStatus) -> Res<()> {
    let cur = get_status(conn, id)?;
    editable(conn, cur.project_id)?;
    let name = match &u.name {
        Some(s) => rename_value(cur.key.as_deref(), s)?,
        None => cur.name.clone(),
    };
    let color = match &u.color {
        Some(c) => normalize_color(c)?,
        None => cur.color.clone(),
    };
    atomic(conn, || {
        ensure_name_free(conn, "statuses", "project_id = ?1", &[cur.project_id], cur.key.as_deref(), name.as_deref(), Some(id))?;
        conn.execute("UPDATE statuses SET name = ?1, color = ?2, category = ?3 WHERE id = ?4", params![name, color, u.category.unwrap_or(cur.category).as_str(), id])?;
        Ok(())
    })
}

pub fn reorder_statuses(conn: &Connection, project_id: i64, ids: &[i64]) -> Res<()> {
    editable(conn, project_id)?;
    common::reorder(conn, "statuses", "project_id = ?1", &[project_id], ids)
}

/// Wie oft der Status benutzt wird (Tickets; bis zum Ticket-Schritt 0).
pub fn usage(conn: &Connection, id: i64) -> Res<i64> {
    get_status(conn, id)?;
    integrity::usage_count_status(conn, id)
}

/// Löscht einen Status. Wird er benutzt, ist ein `mode` nötig (sonst `InUse`). `Migrate { to }` hängt Tickets und die
/// Workflows der Typen auf den Ersatz um (Übergänge werden entsprechend umgeleitet), `Discard` verwirft sie.
pub fn delete_status(conn: &Connection, id: i64, mode: Option<DeleteMode>) -> Res<()> {
    let cur = get_status(conn, id)?;
    editable(conn, cur.project_id)?;
    let count = integrity::usage_count_status(conn, id)?;
    integrity::require_mode(count, mode.is_some())?;
    let to = match &mode {
        Some(DeleteMode::Migrate { to }) => {
            let t = get_status(conn, *to).map_err(|_| PlanError::ReplacementInvalid)?;
            if t.id == id || t.project_id != cur.project_id {
                return Err(PlanError::ReplacementInvalid);
            }
            Some(t.id)
        }
        _ => None,
    };
    atomic(conn, || {
        if let Some(to) = to {
            rewire_workflows(conn, id, to)?;
        }
        integrity::migrate_tickets_status(conn, id, to)?;
        conn.execute("DELETE FROM statuses WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// Hängt die Workflow-Zuordnung und die Übergänge von `from` auf `to` um (ohne Doppelte und ohne Übergang auf sich selbst).
fn rewire_workflows(conn: &Connection, from: i64, to: i64) -> Res<()> {
    // Typen, die schon `to` nutzen: die Zeile von `from` entfällt; die übrigen übernehmen die Position.
    conn.execute("DELETE FROM type_statuses WHERE status_id = ?1 AND type_id IN (SELECT type_id FROM type_statuses WHERE status_id = ?2)", [from, to])?;
    conn.execute("UPDATE type_statuses SET status_id = ?2 WHERE status_id = ?1", [from, to])?;
    // Übergänge: zuerst Übergänge von/nach `from` auf `to` umschreiben (bereits vorhandene bleiben unangetastet), Reste entfallen.
    conn.execute("UPDATE OR IGNORE transitions SET from_status_id = ?2 WHERE from_status_id = ?1", [from, to])?;
    conn.execute("UPDATE OR IGNORE transitions SET to_status_id = ?2 WHERE to_status_id = ?1", [from, to])?;
    conn.execute("DELETE FROM transitions WHERE from_status_id = ?1 OR to_status_id = ?1", [from])?;
    conn.execute("DELETE FROM transitions WHERE from_status_id = to_status_id", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::testutil::db;

    fn proj(c: &Connection, key: &str) -> i64 {
        create_project(c, &NewProject::new(key, "P", "#112233")).unwrap()
    }

    fn st(project_id: i64, name: &str, cat: StatusCategory) -> NewStatus {
        NewStatus { project_id, key: None, name: Some(name.into()), color: "#445566".into(), category: cat }
    }

    #[test]
    fn creates_updates_and_lists_in_order() {
        let c = db();
        let p = proj(&c, "AA");
        let a = create_status(&c, &st(p, "Backlog", StatusCategory::Todo)).unwrap();
        let b = create_status(&c, &st(p, "Review", StatusCategory::Progress)).unwrap();
        let d = create_status(&c, &NewStatus { project_id: p, key: Some("done".into()), name: None, color: "#7fbf6a".into(), category: StatusCategory::Done }).unwrap();
        let l = list_statuses(&c, p).unwrap();
        assert_eq!(l.iter().map(|s| s.id).collect::<Vec<_>>(), vec![a, b, d]);
        assert_eq!(l[2].display(Lang::De), "Erledigt");
        assert_eq!(l[2].display(Lang::En), "Done");
        assert_eq!(l[2].color, "#7FBF6A");
        // umbenennen: eigener Name gilt für beide Sprachen, Schlüssel bleibt
        update_status(&c, d, &UpdateStatus { name: Some("Fertig".into()), ..Default::default() }).unwrap();
        let s = get_status(&c, d).unwrap();
        assert_eq!((s.key.as_deref(), s.name.as_deref()), (Some("done"), Some("Fertig")));
        assert_eq!((s.display(Lang::De), s.display(Lang::En)), ("Fertig".into(), "Fertig".into()));
        // leerer Name = zurück auf den Standardnamen
        update_status(&c, d, &UpdateStatus { name: Some("".into()), ..Default::default() }).unwrap();
        assert_eq!(get_status(&c, d).unwrap().display(Lang::En), "Done");
        // ohne Schlüssel kein leerer Name
        assert_eq!(update_status(&c, a, &UpdateStatus { name: Some(" ".into()), ..Default::default() }), Err(PlanError::NameEmpty));
        update_status(&c, a, &UpdateStatus { color: Some("#010203".into()), category: Some(StatusCategory::Done), ..Default::default() }).unwrap();
        let s = get_status(&c, a).unwrap();
        assert_eq!((s.color.as_str(), s.category), ("#010203", StatusCategory::Done));
        // Reihenfolge
        reorder_statuses(&c, p, &[d, a, b]).unwrap();
        assert_eq!(list_statuses(&c, p).unwrap().iter().map(|s| s.id).collect::<Vec<_>>(), vec![d, a, b]);
        assert_eq!(reorder_statuses(&c, p, &[d, a]), Err(PlanError::ReorderMismatch));
    }

    #[test]
    fn validates_input() {
        let c = db();
        let p = proj(&c, "AA");
        let q = proj(&c, "BB");
        create_status(&c, &st(p, "Open", StatusCategory::Todo)).unwrap();
        assert_eq!(create_status(&c, &st(p, "open", StatusCategory::Todo)), Err(PlanError::NameTaken));
        create_status(&c, &st(q, "Open", StatusCategory::Todo)).unwrap(); // anderes Projekt: erlaubt
        assert_eq!(create_status(&c, &NewStatus { project_id: p, key: None, name: None, color: "#112233".into(), category: StatusCategory::Todo }), Err(PlanError::NameEmpty));
        assert_eq!(create_status(&c, &NewStatus { project_id: p, key: None, name: Some("  ".into()), color: "#112233".into(), category: StatusCategory::Todo }), Err(PlanError::NameEmpty));
        assert_eq!(create_status(&c, &NewStatus { project_id: p, key: None, name: Some("X".into()), color: "rot".into(), category: StatusCategory::Todo }), Err(PlanError::ColorInvalid));
        assert_eq!(create_status(&c, &NewStatus { project_id: p, key: Some("Bad Key".into()), name: None, color: "#112233".into(), category: StatusCategory::Todo }), Err(PlanError::KeyInvalid));
        // Standard-Status kollidiert mit gleichnamigem eigenen Namen in der anderen Sprache
        create_status(&c, &NewStatus { project_id: p, key: Some("todo".into()), name: None, color: "#112233".into(), category: StatusCategory::Todo }).unwrap();
        assert_eq!(create_status(&c, &st(p, "Zu erledigen", StatusCategory::Todo)), Err(PlanError::NameTaken));
        assert_eq!(create_status(&c, &st(999, "X", StatusCategory::Todo)), Err(PlanError::NotFound));
        // System-Projekt: nichts anlegbar
        let todo = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(create_status(&c, &st(todo, "X", StatusCategory::Todo)), Err(PlanError::SystemProtected));
        // Umbenennen auf einen vergebenen Namen
        let a = create_status(&c, &st(p, "A", StatusCategory::Todo)).unwrap();
        assert_eq!(update_status(&c, a, &UpdateStatus { name: Some("OPEN".into()), ..Default::default() }), Err(PlanError::NameTaken));
        update_status(&c, a, &UpdateStatus { name: Some("a".into()), ..Default::default() }).unwrap(); // eigener Name ist kein Konflikt
    }

    fn setup_workflow(c: &Connection) -> (i64, i64, i64, i64, i64) {
        use crate::planning::types::{create_type, NewType};
        use crate::planning::workflow::{set_transition, set_workflow_statuses, TransitionDef};
        let p = proj(c, "AA");
        let a = create_status(c, &st(p, "A", StatusCategory::Todo)).unwrap();
        let b = create_status(c, &st(p, "B", StatusCategory::Progress)).unwrap();
        let d = create_status(c, &st(p, "C", StatusCategory::Done)).unwrap();
        let t = create_type(c, &NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#112233".into(), workflow: true }).unwrap();
        set_workflow_statuses(c, t, &[a, b, d]).unwrap();
        for (f, to) in [(a, b), (b, a), (b, d), (a, d)] {
            set_transition(c, t, &TransitionDef { from_status_id: f, to_status_id: to, ..Default::default() }).unwrap();
        }
        (p, t, a, b, d)
    }

    #[test]
    fn deleting_without_usage_cascades_workflow_rows() {
        let c = db();
        let (_p, t, a, b, d) = setup_workflow(&c);
        delete_status(&c, b, None).unwrap();
        assert_eq!(crate::planning::workflow::workflow_statuses(&c, t).unwrap(), vec![a, d]);
        let tr = crate::planning::workflow::list_transitions(&c, t).unwrap();
        assert_eq!(tr.len(), 1, "nur A -> C bleibt");
        assert_eq!((tr[0].def.from_status_id, tr[0].def.to_status_id), (a, d));
        assert_eq!(get_status(&c, b), Err(PlanError::NotFound));
        assert_eq!(delete_status(&c, b, None), Err(PlanError::NotFound));
    }

    #[test]
    fn migrating_rewires_workflows() {
        let c = db();
        let (p, t, a, b, d) = setup_workflow(&c);
        // B -> A: Typ nutzt beide; Übergänge A->B, B->A, B->C, A->C ⇒ A->A entfällt, B->C wird A->C (schon vorhanden)
        delete_status(&c, b, Some(DeleteMode::Migrate { to: a })).unwrap();
        assert_eq!(crate::planning::workflow::workflow_statuses(&c, t).unwrap(), vec![a, d]);
        let tr = crate::planning::workflow::list_transitions(&c, t).unwrap();
        assert_eq!(tr.len(), 1);
        assert_eq!((tr[0].def.from_status_id, tr[0].def.to_status_id), (a, d));
        // Ersatz-Status ist neu im Workflow: bekommt die Position des alten
        let e = create_status(&c, &st(p, "E", StatusCategory::Progress)).unwrap();
        delete_status(&c, d, Some(DeleteMode::Migrate { to: e })).unwrap();
        assert_eq!(crate::planning::workflow::workflow_statuses(&c, t).unwrap(), vec![a, e]);
        let tr = crate::planning::workflow::list_transitions(&c, t).unwrap();
        assert_eq!((tr[0].def.from_status_id, tr[0].def.to_status_id), (a, e));
    }

    #[test]
    fn migrate_replacement_must_be_valid() {
        let c = db();
        let (_p, _t, a, b, _d) = setup_workflow(&c);
        let q = proj(&c, "BB");
        let other = create_status(&c, &st(q, "X", StatusCategory::Todo)).unwrap();
        assert_eq!(delete_status(&c, a, Some(DeleteMode::Migrate { to: a })), Err(PlanError::ReplacementInvalid));
        assert_eq!(delete_status(&c, a, Some(DeleteMode::Migrate { to: other })), Err(PlanError::ReplacementInvalid), "anderes Projekt");
        assert_eq!(delete_status(&c, a, Some(DeleteMode::Migrate { to: 9999 })), Err(PlanError::ReplacementInvalid));
        assert!(get_status(&c, a).is_ok() && get_status(&c, b).is_ok(), "nichts verändert");
        delete_status(&c, a, Some(DeleteMode::Discard)).unwrap();
    }

    #[test]
    fn in_use_requires_a_mode() {
        // Die Nutzungszähler kommen im Ticket-Schritt; hier prüfen wir die Regel an der Nahtstelle.
        assert_eq!(integrity::require_mode(2, false), Err(PlanError::InUse { count: 2 }));
        assert!(integrity::require_mode(2, true).is_ok());
        let c = db();
        let p = proj(&c, "AA");
        let a = create_status(&c, &st(p, "A", StatusCategory::Todo)).unwrap();
        assert_eq!(usage(&c, a).unwrap(), 0);
        assert_eq!(usage(&c, 999), Err(PlanError::NotFound));
    }
}
