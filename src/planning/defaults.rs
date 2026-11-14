//! Standard-Inhalte eines neuen Projekts („mit Standards“): Status To Do / In Progress / Done, die fünf Typen Epic, Userstory,
//! Devtask, Subtask und Defect mit Workflow, Hierarchie und Feldern sowie die Liste der Arten der Arbeit. Alle Namen sind
//! übersetzbar (`key` ohne `name`), solange der Nutzer sie nicht umbenennt.
//!
//! * Workflow je Typ: To Do ↔ In Progress ↔ Done (+ To Do → Done).
//! * Hierarchie: Userstory/Devtask/Defect unter Epic; Devtask/Defect/Subtask unter Userstory; Subtask unter Devtask/Defect.
//! * Felder: Description (rich) und Priority (Dropdown, Standard Medium) bei jedem Typ; Userstory zusätzlich Epic (Ticket-Feld,
//!   Ziel-Typ Epic), Release version (nur wenn das Projekt Versionen nutzt) und Story points; Devtask zusätzlich Story points;
//!   Defect zusätzlich Steps to reproduce. Story points sind optional (nicht Pflicht) und lassen sich löschen.

use super::fields::{create_field, FieldKind, FieldOptions, NewField, OptionValue};
use super::statuses::{create_status, NewStatus, StatusCategory};
use super::types::{create_type, set_parents, NewType};
use super::workflow::{replace_transitions, set_workflow_statuses, TransitionDef};
use super::work_kinds::{create_work_kind, DEFAULT_KEYS};
use super::Res;
use rusqlite::Connection;
use serde_json::json;

/// (Schlüssel, Farbe, Kategorie)
const STATUSES: [(&str, &str, StatusCategory); 3] = [("todo", "#97A3B0", StatusCategory::Todo), ("progress", "#58A6E6", StatusCategory::Progress), ("done", "#7FBF6A", StatusCategory::Done)];

/// (Schlüssel, Farbe, Icon)
const TYPES: [(&str, &str, &str); 5] = [
    ("epic", "#B77FE0", "bolt"),
    ("userstory", "#7FBF6A", "bookmark"),
    ("devtask", "#58A6E6", "code"),
    ("subtask", "#4FB8A8", "subtask"),
    ("defect", "#E06C75", "bug"),
];

/// (Kind-Typ, Eltern-Typen)
const PARENTS: [(&str, &[&str]); 5] = [("epic", &[]), ("userstory", &["epic"]), ("devtask", &["epic", "userstory"]), ("subtask", &["userstory", "devtask", "defect"]), ("defect", &["epic", "userstory"])];

/// (Schlüssel, Farbe) der Prioritäts-Werte in Reihenfolge; der dritte (`medium`) ist der Standardwert.
const PRIORITIES: [(&str, &str); 5] = [("priority.highest", "#E06C75"), ("priority.high", "#E5A04B"), ("priority.medium", "#D8B94A"), ("priority.low", "#58A6E6"), ("priority.lowest", "#97A3B0")];

fn field(type_id: i64, key: &str, kind: FieldKind) -> NewField {
    NewField { type_id, key: Some(key.to_string()), name: None, kind, options: FieldOptions::default(), required: false, default_value: None, help: String::new(), section: String::new(), show_on_card: false, builtin: true }
}

/// Legt die Standard-Inhalte im (leeren) Projekt an. Wird von `create_project` bei `with_defaults` aufgerufen.
pub(crate) fn apply(conn: &Connection, project_id: i64, use_versions: bool) -> Res<()> {
    // Status
    let mut status_ids = Vec::new();
    for (key, color, category) in STATUSES {
        status_ids.push(create_status(conn, &NewStatus { project_id, key: Some(key.into()), name: None, color: color.into(), category })?);
    }
    let (todo, progress, done) = (status_ids[0], status_ids[1], status_ids[2]);
    // Typen mit Workflow
    let mut type_ids: Vec<(&str, i64)> = Vec::new();
    for (key, color, icon) in TYPES {
        let id = create_type(conn, &NewType { project_id, key: Some(key.into()), name: None, icon: Some(icon.into()), color: color.into(), workflow: true })?;
        set_workflow_statuses(conn, id, &status_ids)?;
        let def = |f: i64, t: i64| TransitionDef { from_status_id: f, to_status_id: t, ..Default::default() };
        replace_transitions(conn, id, &[def(todo, progress), def(progress, todo), def(progress, done), def(done, progress), def(todo, done)])?;
        type_ids.push((key, id));
    }
    let tid = |key: &str| type_ids.iter().find(|(k, _)| *k == key).map(|(_, id)| *id).unwrap();
    // Hierarchie
    for (child, parents) in PARENTS {
        let ids: Vec<i64> = parents.iter().map(|p| tid(p)).collect();
        set_parents(conn, tid(child), &ids)?;
    }
    // Felder
    for (key, _, _) in TYPES {
        let t = tid(key);
        let mut d = field(t, "description", FieldKind::Rich);
        d.options.rows = Some(8);
        create_field(conn, &d)?;
        let mut p = field(t, "priority", FieldKind::Dropdown);
        p.options.values = PRIORITIES.iter().map(|(k, c)| OptionValue { id: 0, key: Some((*k).into()), name: None, color: Some((*c).into()) }).collect();
        p.default_value = Some(json!(3)); // „Medium“ (Normal) ist der dritte Wert, Kennung 3
        p.show_on_card = true;
        create_field(conn, &p)?;
        match key {
            "userstory" => {
                let mut e = field(t, "epic", FieldKind::Ticket);
                e.options.target_types = vec![tid("epic")];
                e.show_on_card = true;
                create_field(conn, &e)?;
                if use_versions {
                    create_field(conn, &field(t, "release_version", FieldKind::Version))?;
                }
                create_field(conn, &story_points(t))?;
            }
            "devtask" => {
                create_field(conn, &story_points(t))?;
            }
            "defect" => {
                let mut s = field(t, "steps", FieldKind::Rich);
                s.options.rows = Some(6);
                create_field(conn, &s)?;
            }
            _ => {}
        }
    }
    // Arten der Arbeit
    for k in DEFAULT_KEYS {
        create_work_kind(conn, project_id, Some(k), None)?;
    }
    Ok(())
}

fn story_points(type_id: i64) -> NewField {
    let mut f = field(type_id, "story_points", FieldKind::Number);
    f.options.min = Some(0.0);
    f.options.decimals = Some(0);
    f.show_on_card = true;
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{list_fields, Field};
    use crate::planning::projects::{create_project, NewProject, Project};
    use crate::planning::statuses::list_statuses;
    use crate::planning::testutil::db;
    use crate::planning::types::{list_types, parents_of, TicketType};
    use crate::planning::workflow::{default_status, list_transitions, workflow_problems, workflow_statuses};
    use crate::state::Lang;

    fn with_defaults(c: &Connection, key: &str, versions: bool) -> Project {
        let mut p = NewProject::new(key, "P", "#112233");
        p.with_defaults = true;
        p.use_versions = versions;
        let id = create_project(c, &p).unwrap();
        crate::planning::projects::get_project(c, id).unwrap()
    }

    fn by_key(types: &[TicketType], key: &str) -> TicketType {
        types.iter().find(|t| t.key.as_deref() == Some(key)).unwrap().clone()
    }

    fn keys(fields: &[Field]) -> Vec<String> {
        fields.iter().map(|f| f.key.clone().unwrap()).collect()
    }

    #[test]
    fn creates_exactly_the_described_defaults() {
        let c = db();
        let p = with_defaults(&c, "AA", true);
        // Status
        let st = list_statuses(&c, p.id).unwrap();
        assert_eq!(st.iter().map(|s| (s.key.clone().unwrap(), s.category)).collect::<Vec<_>>(), vec![("todo".into(), StatusCategory::Todo), ("progress".into(), StatusCategory::Progress), ("done".into(), StatusCategory::Done)]);
        assert!(st.iter().all(|s| s.name.is_none()));
        assert_eq!(st.iter().map(|s| s.display(Lang::En)).collect::<Vec<_>>(), vec!["To Do", "In Progress", "Done"]);
        assert_eq!(st.iter().map(|s| s.display(Lang::De)).collect::<Vec<_>>(), vec!["Zu erledigen", "In Arbeit", "Erledigt"]);
        // Typen
        let ty = list_types(&c, p.id).unwrap();
        assert_eq!(ty.iter().map(|t| t.key.clone().unwrap()).collect::<Vec<_>>(), vec!["epic", "userstory", "devtask", "subtask", "defect"]);
        assert!(ty.iter().all(|t| t.workflow && !t.system && t.icon.is_some() && t.name.is_none()));
        assert!(ty.iter().all(|t| crate::planning::icons::is_valid(t.icon.as_deref().unwrap())));
        assert_eq!(ty.iter().map(|t| t.display(Lang::De)).collect::<Vec<_>>(), vec!["Epic", "Userstory", "Dev-Aufgabe", "Unteraufgabe", "Fehler"]);
        assert_eq!(ty.iter().map(|t| t.display(Lang::En)).collect::<Vec<_>>(), vec!["Epic", "User story", "Dev task", "Subtask", "Defect"]);
        // Workflow je Typ: alle drei Status, fünf Übergänge, kein Hinweis
        let sids: Vec<i64> = st.iter().map(|s| s.id).collect();
        for t in &ty {
            assert_eq!(workflow_statuses(&c, t.id).unwrap(), sids);
            let tr: Vec<(i64, i64)> = list_transitions(&c, t.id).unwrap().iter().map(|x| (x.def.from_status_id, x.def.to_status_id)).collect();
            let mut want = vec![(sids[0], sids[1]), (sids[1], sids[0]), (sids[1], sids[2]), (sids[2], sids[1]), (sids[0], sids[2])];
            want.sort();
            assert_eq!(tr, want, "{:?}", t.key);
            assert_eq!(default_status(&c, t.id).unwrap(), Some(sids[0]));
            assert!(workflow_problems(&c, t.id).unwrap().is_empty());
        }
        // Hierarchie
        let id = |k: &str| by_key(&ty, k).id;
        let parents = |k: &str| {
            let mut v = parents_of(&c, id(k)).unwrap();
            v.sort();
            v
        };
        let sorted = |ks: &[&str]| {
            let mut v: Vec<i64> = ks.iter().map(|k| id(k)).collect();
            v.sort();
            v
        };
        assert!(parents("epic").is_empty());
        assert_eq!(parents("userstory"), sorted(&["epic"]));
        assert_eq!(parents("devtask"), sorted(&["epic", "userstory"]));
        assert_eq!(parents("defect"), sorted(&["epic", "userstory"]));
        assert_eq!(parents("subtask"), sorted(&["userstory", "devtask", "defect"]));
        // Felder
        let fields = |k: &str| list_fields(&c, id(k)).unwrap();
        assert_eq!(keys(&fields("epic")), vec!["description", "priority"]);
        assert_eq!(keys(&fields("userstory")), vec!["description", "priority", "epic", "release_version", "story_points"]);
        assert_eq!(keys(&fields("devtask")), vec!["description", "priority", "story_points"]);
        assert_eq!(keys(&fields("subtask")), vec!["description", "priority"]);
        assert_eq!(keys(&fields("defect")), vec!["description", "priority", "steps"]);
        let us = fields("userstory");
        let kind = |k: &str| us.iter().find(|f| f.key.as_deref() == Some(k)).unwrap().clone();
        assert_eq!(kind("description").kind, FieldKind::Rich);
        assert_eq!(kind("epic").kind, FieldKind::Ticket);
        assert_eq!(kind("epic").options.target_types, vec![id("epic")]);
        assert_eq!(kind("release_version").kind, FieldKind::Version);
        let sp = kind("story_points");
        assert_eq!(sp.kind, FieldKind::Number);
        assert!(!sp.required, "Story Points sind optional");
        assert!(us.iter().all(|f| !f.required && f.builtin && f.name.is_none()));
        // Priorität: fünf Werte, Standard Medium/„Normal“
        let pr = kind("priority");
        assert_eq!(pr.kind, FieldKind::Dropdown);
        assert_eq!(pr.options.values.iter().map(|v| v.display(Lang::En)).collect::<Vec<_>>(), vec!["Highest", "High", "Medium", "Low", "Lowest"]);
        assert_eq!(pr.options.values.iter().map(|v| v.display(Lang::De)).collect::<Vec<_>>(), vec!["Höchste", "Hoch", "Normal", "Niedrig", "Niedrigste"]);
        assert!(pr.options.values.iter().all(|v| v.color.is_some()));
        let def = pr.default_value.clone().unwrap().as_i64().unwrap();
        assert_eq!(pr.options.values.iter().find(|v| v.id == def).unwrap().key.as_deref(), Some("priority.medium"));
        assert_eq!(kind("description").display(Lang::De), "Beschreibung");
        assert_eq!(fields("defect")[2].display(Lang::De), "Schritte zur Reproduktion");
        // Arten der Arbeit
        let wk = crate::planning::work_kinds::list_work_kinds(&c, p.id).unwrap();
        assert_eq!(wk.iter().map(|w| w.display(Lang::En)).collect::<Vec<_>>(), vec!["Development", "Testing", "Planning", "Refinement"]);
        assert_eq!(wk.iter().map(|w| w.display(Lang::De)).collect::<Vec<_>>(), vec!["Entwicklung", "Test", "Planung", "Schärfung"]);
    }

    #[test]
    fn release_version_only_with_versions() {
        let c = db();
        let p = with_defaults(&c, "AA", false);
        let us = by_key(&list_types(&c, p.id).unwrap(), "userstory");
        assert_eq!(keys(&list_fields(&c, us.id).unwrap()), vec!["description", "priority", "epic", "story_points"]);
    }

    #[test]
    fn every_default_key_is_translated() {
        let c = db();
        let p = with_defaults(&c, "AA", true);
        let tr = |k: &str| {
            assert!(crate::planning::names::translate(k, Lang::De).is_some() && crate::planning::names::translate(k, Lang::En).is_some(), "Übersetzung für {k} fehlt");
        };
        for s in list_statuses(&c, p.id).unwrap() {
            tr(s.key.as_deref().unwrap());
        }
        for t in list_types(&c, p.id).unwrap() {
            tr(t.key.as_deref().unwrap());
            for f in list_fields(&c, t.id).unwrap() {
                tr(f.key.as_deref().unwrap());
                for v in &f.options.values {
                    tr(v.key.as_deref().unwrap());
                }
            }
        }
        for w in crate::planning::work_kinds::list_work_kinds(&c, p.id).unwrap() {
            tr(w.key.as_deref().unwrap());
        }
        for l in crate::planning::link_types::list_link_types(&c).unwrap() {
            let k = l.key.clone().unwrap();
            tr(&format!("link.{k}.out"));
            if l.target_kind == crate::planning::link_types::TargetKind::Ticket && !l.symmetric {
                tr(&format!("link.{k}.in"));
            }
        }
    }

    #[test]
    fn two_default_projects_are_independent() {
        let c = db();
        let a = with_defaults(&c, "AA", true);
        let b = with_defaults(&c, "BB", false);
        assert_eq!(list_statuses(&c, a.id).unwrap().len(), 3);
        assert_eq!(list_statuses(&c, b.id).unwrap().len(), 3);
        let ta = list_types(&c, a.id).unwrap();
        let tb = list_types(&c, b.id).unwrap();
        assert!(ta.iter().all(|x| tb.iter().all(|y| x.id != y.id)));
        // Ticket-Feld „Epic“ des zweiten Projekts zeigt auf dessen eigenen Epic-Typ
        let us = by_key(&tb, "userstory");
        let f = list_fields(&c, us.id).unwrap().into_iter().find(|f| f.key.as_deref() == Some("epic")).unwrap();
        assert_eq!(f.options.target_types, vec![by_key(&tb, "epic").id]);
    }

    #[test]
    fn defaults_can_be_edited_afterwards() {
        use crate::planning::fields::{delete_field, update_field, UpdateField};
        let c = db();
        let p = with_defaults(&c, "AA", true);
        let dev = by_key(&list_types(&c, p.id).unwrap(), "devtask");
        let sp = list_fields(&c, dev.id).unwrap().into_iter().find(|f| f.key.as_deref() == Some("story_points")).unwrap();
        delete_field(&c, sp.id, None).unwrap(); // Story Points sind nicht zwingend
        update_field(&c, list_fields(&c, dev.id).unwrap()[1].id, &UpdateField { name: Some("Wichtigkeit".into()), ..Default::default() }).unwrap();
        let f = &list_fields(&c, dev.id).unwrap()[1];
        assert_eq!((f.display(Lang::En).as_str(), f.key.as_deref()), ("Wichtigkeit", Some("priority")));
    }
}
