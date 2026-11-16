//! Gebündeltes Laden für die Oberfläche: ein Projekt mit Status, Typen (samt Feldern, Hierarchie, Workflow und Übergängen)
//! und Arten der Arbeit in wenigen Abfragen statt je Typ eine.

use super::fields::{self, Field};
use super::link_types::{list_link_types, LinkType};
use super::projects::{get_project, project_by_key, Project};
use super::statuses::{list_statuses, Status};
use super::types::{self, TicketType};
use super::work_kinds::{list_work_kinds, WorkKind};
use super::workflow::{self, Transition};
use super::Res;
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct TypeSummary {
    pub ticket_type: TicketType,
    /// Typen, unter denen der Typ hängen darf (aufsteigend nach Id).
    pub parents: Vec<i64>,
    /// Status des Workflows in Reihenfolge (leer ohne Workflow).
    pub workflow_statuses: Vec<i64>,
    pub transitions: Vec<Transition>,
    /// Felder in Anzeigereihenfolge.
    pub fields: Vec<Field>,
}

impl TypeSummary {
    pub fn field(&self, id: i64) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    pub fn field_by_key(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key.as_deref() == Some(key))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSummary {
    pub project: Project,
    pub statuses: Vec<Status>,
    pub types: Vec<TypeSummary>,
    pub work_kinds: Vec<WorkKind>,
}

impl ProjectSummary {
    pub fn status(&self, id: i64) -> Option<&Status> {
        self.statuses.iter().find(|s| s.id == id)
    }

    pub fn type_by_id(&self, id: i64) -> Option<&TypeSummary> {
        self.types.iter().find(|t| t.ticket_type.id == id)
    }

    pub fn type_by_key(&self, key: &str) -> Option<&TypeSummary> {
        self.types.iter().find(|t| t.ticket_type.key.as_deref() == Some(key))
    }
}

/// Lädt alles zu einem Projekt.
pub fn project_summary(conn: &Connection, id: i64) -> Res<ProjectSummary> {
    let project = get_project(conn, id)?;
    build(conn, project)
}

/// Wie [`project_summary`], über den Projektschlüssel.
pub fn project_summary_by_key(conn: &Connection, key: &str) -> Res<ProjectSummary> {
    let project = project_by_key(conn, key)?;
    build(conn, project)
}

fn build(conn: &Connection, project: Project) -> Res<ProjectSummary> {
    let pid = project.id;
    let types_list = types::list_types(conn, pid)?;
    let mut parents: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT p.type_id, p.parent_type_id FROM type_parents p JOIN ticket_types t ON t.id = p.type_id WHERE t.project_id = ?1 ORDER BY p.type_id, p.parent_type_id")?;
        for row in st.query_map([pid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (t, p) = row?;
            parents.entry(t).or_default().push(p);
        }
    }
    let mut wf: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT s.type_id, s.status_id FROM type_statuses s JOIN ticket_types t ON t.id = s.type_id WHERE t.project_id = ?1 ORDER BY s.type_id, s.position, s.status_id")?;
        for row in st.query_map([pid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (t, s) = row?;
            wf.entry(t).or_default().push(s);
        }
    }
    let mut trs: HashMap<i64, Vec<Transition>> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT x.id, x.type_id, x.from_status_id, x.to_status_id, x.required_fields, x.conditions, x.set_values FROM transitions x JOIN ticket_types t ON t.id = x.type_id
             WHERE t.project_id = ?1 ORDER BY x.type_id, x.from_status_id, x.to_status_id",
        )?;
        for row in st.query_map([pid], workflow::transition_from_row)? {
            let t = row?;
            trs.entry(t.type_id).or_default().push(t);
        }
    }
    let mut flds: HashMap<i64, Vec<Field>> = HashMap::new();
    {
        let mut st = conn.prepare(&format!("SELECT {} FROM fields WHERE type_id IN (SELECT id FROM ticket_types WHERE project_id = ?1) ORDER BY type_id, position, id", fields::COLS))?;
        for row in st.query_map([pid], fields::from_row)? {
            let f = row?;
            flds.entry(f.type_id).or_default().push(f);
        }
    }
    let types = types_list
        .into_iter()
        .map(|t| {
            let id = t.id;
            TypeSummary {
                ticket_type: t,
                parents: parents.remove(&id).unwrap_or_default(),
                workflow_statuses: wf.remove(&id).unwrap_or_default(),
                transitions: trs.remove(&id).unwrap_or_default(),
                fields: flds.remove(&id).unwrap_or_default(),
            }
        })
        .collect();
    Ok(ProjectSummary { statuses: list_statuses(conn, pid)?, work_kinds: list_work_kinds(conn, pid)?, types, project })
}

/// Alle Projekte (System-Projekt zuerst) samt der globalen Verknüpfungstypen – die Grundlage der Projektauswahl und Editoren.
#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub projects: Vec<ProjectSummary>,
    pub link_types: Vec<LinkType>,
}

pub fn overview(conn: &Connection) -> Res<Overview> {
    let mut projects = Vec::new();
    for p in super::projects::list_projects(conn)? {
        projects.push(build(conn, p)?);
    }
    Ok(Overview { projects, link_types: list_link_types(conn)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::testutil::db;

    #[test]
    fn bundles_everything_and_matches_the_single_queries() {
        let c = db();
        let mut np = NewProject::new("AA", "A", "#112233");
        np.with_defaults = true;
        np.use_versions = true;
        let a = create_project(&c, &np).unwrap();
        create_project(&c, &NewProject::new("BB", "B", "#112233")).unwrap();
        let s = project_summary(&c, a).unwrap();
        assert_eq!(s.project.key, "AA");
        assert_eq!(s.statuses.len(), 3);
        assert_eq!(s.work_kinds.len(), 4);
        assert_eq!(s.types.len(), 5);
        for t in &s.types {
            let id = t.ticket_type.id;
            assert_eq!(t.fields, fields::list_fields(&c, id).unwrap());
            assert_eq!(t.parents, types::parents_of(&c, id).unwrap());
            assert_eq!(t.workflow_statuses, workflow::workflow_statuses(&c, id).unwrap());
            assert_eq!(t.transitions, workflow::list_transitions(&c, id).unwrap());
        }
        let us = s.type_by_key("userstory").unwrap();
        assert_eq!(us.fields.len(), 6, "mit den eingebauten Feldern Beschreibung und Fälligkeit");
        assert!(us.field_by_key("epic").is_some() && us.field(us.fields[0].id).is_some());
        assert!(s.status(s.statuses[0].id).is_some() && s.type_by_id(us.ticket_type.id).is_some());
        assert_eq!(project_summary_by_key(&c, "aa").unwrap(), s);
        // Typen eines anderen Projekts mischen sich nicht ein
        assert!(project_summary(&c, a).unwrap().types.iter().all(|t| t.ticket_type.project_id == a));
        assert!(project_summary(&c, 999).is_err());
    }

    #[test]
    fn overview_lists_projects_and_link_types() {
        let c = db();
        create_project(&c, &NewProject::new("AA", "A", "#112233")).unwrap();
        let o = overview(&c).unwrap();
        assert_eq!(o.projects.iter().map(|p| p.project.key.as_str()).collect::<Vec<_>>(), vec!["TODO", "AA"]);
        assert_eq!(o.link_types.len(), 4);
        let todo = &o.projects[0];
        assert_eq!(todo.types.len(), 1);
        assert!(todo.types[0].fields.is_empty() && todo.types[0].workflow_statuses.is_empty());
        assert!(o.projects[1].types.is_empty(), "leeres Projekt");
    }
}
