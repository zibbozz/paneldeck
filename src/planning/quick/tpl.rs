//! Quick-Actions im Typ-Vorlagen-Export/-Import (`planning::template`).
//!
//! * **Export** ([`export_actions`]): Die Aktionen eines Typs mit **Vorlagen-Nummern** statt Datenbank-Ids (Ziel-Typ, Status,
//!   Felder; dieselben Nummern wie im Rest des Dokuments). Verknüpfungstypen sind global und stehen nicht im Dokument:
//!   Standardtypen werden über ihren Schlüssel, eigene über ihren Hin-Namen angesprochen. Ungültige Aktionen fehlen.
//!   Standardwerte für Versions- und Meilenstein-Felder entfallen, weil deren Kennungen zu einem Projekt gehören.
//! * **Import** ([`import_actions`]): bildet die Nummern auf die echten Ids des Zielprojekts ab und prüft jede Aktion wie
//!   beim Anlegen. Verweist die Aktion auf einen Typ oder Status, den das Dokument nicht enthält, ist das Dokument
//!   inkonsistent (Fehler, der ganze Import bleibt wirkungslos). Fehlt dagegen nur etwas Nebensächliches (Verknüpfungstyp,
//!   Feld eines schon vorhandenen gleichnamigen Typs, Wert, der dort nicht mehr passt), wird es weggelassen und die Aktion
//!   im Bericht als unvollständig gemeldet.

use super::actions::{create_quick_action, list_quick_actions, repair_params, NewQuickAction};
use super::params::{ActionParams, ChangeStatusParams, CreateTicketParams, DefaultValue, FieldRef, LinkDirection, RelationSpec, StartSessionParams};
use super::QRes;
use crate::planning::common::same_label;
use crate::planning::fields::{get_field, FieldKind};
use crate::planning::link_types::{get_link_type, link_type_by_key, list_link_types};
use crate::planning::types::get_type;
use crate::planning::workflow::SetValue;
use crate::planning::{PlanError, Res, TemplateProblem};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Verweis auf einen Verknüpfungstyp im Dokument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplRelation {
    /// Schlüssel eines Standardtyps (`blocks`, `relates`, `duplicates`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_key: Option<String>,
    /// Hin-Name eines eigenen Typs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_name: Option<String>,
    pub direction: LinkDirection,
}

/// Standardwert im Dokument; `field` ist `"title"` oder die Vorlagen-Nummer des Feldes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplDefault {
    pub field: FieldRef,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplSetValue {
    pub field: i64,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplCreate {
    /// Vorlagen-Nummer des Ziel-Typs.
    pub ticket_type: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<TplRelation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defaults: Vec<TplDefault>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplChange {
    /// Vorlagen-Nummer des Ziel-Status.
    pub to_status: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set_values: Vec<TplSetValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "params", rename_all = "snake_case")]
pub enum TplParams {
    CreateTicket(TplCreate),
    ChangeStatus(TplChange),
    StartSession(StartSessionParams),
}

/// Eine Aktion im Vorlagen-Dokument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplQuickAction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub params: TplParams,
}

/// Abbildung Datenbank-Id → Vorlagen-Nummer (Export) bzw. Vorlagen-Nummer → Datenbank-Id (Import).
pub struct Maps<'a> {
    pub types: &'a HashMap<i64, i64>,
    pub statuses: &'a HashMap<i64, i64>,
    pub fields: &'a HashMap<i64, i64>,
}

// ------------------------------------------------------------------ Export

/// Die Aktionen des Typs im Vorlagen-Format. Aktionen, die auf etwas verweisen, das nicht im Dokument steht, entfallen.
pub fn export_actions(conn: &Connection, type_id: i64, maps: &Maps) -> QRes<Vec<TplQuickAction>> {
    let mut out = Vec::new();
    'actions: for a in list_quick_actions(conn, type_id)? {
        if a.invalid {
            continue;
        }
        let params = match &a.params {
            ActionParams::CreateTicket(c) => {
                let Some(ticket_type) = maps.types.get(&c.ticket_type_id).copied() else { continue 'actions };
                let relation = match &c.relation {
                    Some(r) => match get_link_type(conn, r.link_type_id) {
                        Ok(l) => Some(TplRelation { link_key: l.key.clone(), link_name: if l.key.is_none() { l.name_out.clone() } else { None }, direction: r.direction }),
                        Err(_) => None,
                    },
                    None => None,
                };
                let mut defaults = Vec::new();
                for d in &c.defaults {
                    match d.field_id {
                        FieldRef::Title => defaults.push(TplDefault { field: FieldRef::Title, value: d.value.clone() }),
                        FieldRef::Field(id) => {
                            let Some(no) = maps.fields.get(&id) else { continue };
                            // Versionen/Meilensteine gehören zum Projekt: ihre Kennungen gelten woanders nicht.
                            if get_field(conn, id).is_ok_and(|f| matches!(f.kind, FieldKind::Version | FieldKind::Milestone)) {
                                continue;
                            }
                            defaults.push(TplDefault { field: FieldRef::Field(*no), value: d.value.clone() });
                        }
                    }
                }
                TplParams::CreateTicket(TplCreate { ticket_type, relation, defaults })
            }
            ActionParams::ChangeStatus(c) => {
                let Some(to_status) = maps.statuses.get(&c.to_status_id).copied() else { continue 'actions };
                let set_values = c.set_values.iter().filter_map(|s| maps.fields.get(&s.field_id).map(|no| TplSetValue { field: *no, value: s.value.clone() })).collect();
                TplParams::ChangeStatus(TplChange { to_status, set_values })
            }
            ActionParams::StartSession(c) => TplParams::StartSession(c.clone()),
        };
        out.push(TplQuickAction { key: a.key.clone(), name: a.name.clone(), params });
    }
    Ok(out)
}

// ------------------------------------------------------------------ Import

/// Ergebnis des Imports für einen Typ.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportOutcome {
    pub created: usize,
    /// (Schlüssel, Name) der Aktionen, die nicht vollständig übernommen werden konnten.
    pub incomplete: Vec<(Option<String>, Option<String>)>,
}

fn reference(what: &str, no: i64) -> PlanError {
    PlanError::Template(TemplateProblem::Reference(format!("{what} {no}")))
}

fn find_link(conn: &Connection, r: &TplRelation) -> Res<Option<i64>> {
    if let Some(k) = &r.link_key {
        return Ok(link_type_by_key(conn, k).ok().map(|l| l.id));
    }
    let Some(name) = &r.link_name else { return Ok(None) };
    Ok(list_link_types(conn)?.into_iter().find(|l| l.key.is_none() && same_label(None, Some(name), None, l.name_out.as_deref())).map(|l| l.id))
}

/// Legt die Aktionen im (neu angelegten) Typ `type_id` an; `maps` bildet Vorlagen-Nummern auf Ids des Zielprojekts ab.
pub fn import_actions(conn: &Connection, type_id: i64, actions: &[TplQuickAction], maps: &Maps) -> Res<ImportOutcome> {
    let owner = get_type(conn, type_id)?;
    let mut out = ImportOutcome::default();
    for a in actions {
        let mut incomplete = false;
        let params = match &a.params {
            TplParams::CreateTicket(c) => {
                let ticket_type_id = maps.types.get(&c.ticket_type).copied().ok_or_else(|| reference("type", c.ticket_type))?;
                let relation = match &c.relation {
                    Some(r) => match find_link(conn, r)? {
                        Some(id) => Some(RelationSpec { link_type_id: id, direction: r.direction }),
                        None => {
                            incomplete = true;
                            None
                        }
                    },
                    None => None,
                };
                let mut defaults = Vec::new();
                for d in &c.defaults {
                    match d.field {
                        FieldRef::Title => defaults.push(DefaultValue { field_id: FieldRef::Title, value: d.value.clone() }),
                        FieldRef::Field(no) => match maps.fields.get(&no) {
                            Some(id) => defaults.push(DefaultValue { field_id: FieldRef::Field(*id), value: d.value.clone() }),
                            None => incomplete = true,
                        },
                    }
                }
                ActionParams::CreateTicket(CreateTicketParams { ticket_type_id, relation, defaults })
            }
            TplParams::ChangeStatus(c) => {
                let to_status_id = maps.statuses.get(&c.to_status).copied().ok_or_else(|| reference("status", c.to_status))?;
                let mut set_values = Vec::new();
                for s in &c.set_values {
                    let id = maps.fields.get(&s.field).copied().ok_or_else(|| reference("field", s.field))?;
                    set_values.push(SetValue { field_id: id, value: s.value.clone() });
                }
                ActionParams::ChangeStatus(ChangeStatusParams { to_status_id, set_values })
            }
            TplParams::StartSession(c) => ActionParams::StartSession(c.clone()),
        };
        // Was nicht (mehr) passt, entfällt und wird gemeldet; ein Rest, der immer noch nicht gültig ist, bricht den Import ab.
        let (fixed, rep) = repair_params(conn, &owner, &params)?;
        incomplete |= rep.any();
        create_quick_action(conn, &NewQuickAction { type_id, key: a.key.clone(), name: a.name.clone(), params: fixed })?;
        out.created += 1;
        if incomplete {
            out.incomplete.push((a.key.clone(), a.name.clone()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::actions::fixture::*;
    use super::*;
    use crate::planning::fields::{create_field, list_fields, NewField};
    use crate::planning::link_types::{create_link_type, delete_link_type, NewLinkType, TargetKind};
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::statuses::list_statuses;
    use crate::planning::template::{export_types, import_types, TemplateDoc};
    use crate::planning::types::list_types;
    use serde_json::json;

    fn add(f: &Fx, ty: i64, name: &str, p: ActionParams) -> i64 {
        create_quick_action(&f.c, &NewQuickAction::new(ty, name, p)).unwrap()
    }

    /// Ein Projekt mit je einer Aktion jeder Art, Verknüpfung (Standard und eigener Typ), Titel, Feld- und Versions-Default.
    fn rich(f: &Fx) -> i64 {
        let blocks = link_type_by_key(&f.c, "blocks").unwrap().id;
        let desc = f.field(f.dev, "description");
        let prio = f.field(f.dev, "priority");
        let rv = f.field(f.story, "release_version");
        let epic_field = f.field(f.story, "epic");
        let create = ActionParams::CreateTicket(CreateTicketParams {
            ticket_type_id: f.dev,
            relation: Some(RelationSpec { link_type_id: blocks, direction: LinkDirection::In }),
            defaults: vec![
                DefaultValue { field_id: FieldRef::Title, value: json!("Task for {{source.key}}") },
                DefaultValue { field_id: FieldRef::Field(desc.id), value: json!("From {{source.title}}") },
                DefaultValue { field_id: FieldRef::Field(prio.id), value: json!(2) },
            ],
        });
        add(f, f.story, "Create Dev task", create);
        add(f, f.story, "Start", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.progress, set_values: vec![SetValue { field_id: f.field(f.story, "priority").id, value: json!(1) }] }));
        add(f, f.story, "Work in Claude", ActionParams::StartSession(StartSessionParams { shell: Some("pwsh".into()), claude: true, prompt_from_ticket: true, cwd_from_project: true }));
        // Epic → Userstory: „Epic“-Feld mit dem Ausgangsticket, Version fest (entfällt beim Export)
        add(
            f,
            f.epic,
            "Create story",
            ActionParams::CreateTicket(CreateTicketParams {
                ticket_type_id: f.story,
                relation: None,
                defaults: vec![
                    DefaultValue { field_id: FieldRef::Field(epic_field.id), value: json!("{{source}}") },
                    DefaultValue { field_id: FieldRef::Field(rv.id), value: json!(7) },
                ],
            }),
        );
        f.project
    }

    fn empty_project(c: &Connection, key: &str) -> i64 {
        let mut p = NewProject::new(key, "Empty", "#445566");
        p.with_defaults = false;
        p.use_versions = true;
        create_project(c, &p).unwrap()
    }

    #[test]
    fn json_shape_is_stable() {
        let a = TplQuickAction {
            key: None,
            name: Some("Go".into()),
            params: TplParams::CreateTicket(TplCreate {
                ticket_type: 2,
                relation: Some(TplRelation { link_key: Some("blocks".into()), link_name: None, direction: LinkDirection::Out }),
                defaults: vec![TplDefault { field: FieldRef::Title, value: json!("T") }, TplDefault { field: FieldRef::Field(4), value: json!(1) }],
            }),
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(
            v,
            json!({"name": "Go", "action": "create_ticket", "params": {"ticket_type": 2, "relation": {"link_key": "blocks", "direction": "out"}, "defaults": [{"field": "title", "value": "T"}, {"field": 4, "value": 1}]}})
        );
        assert_eq!(serde_json::from_value::<TplQuickAction>(v).unwrap(), a);
        let s: TplQuickAction = serde_json::from_value(json!({"name": "S", "action": "start_session", "params": {"claude": true}})).unwrap();
        assert!(matches!(s.params, TplParams::StartSession(ref p) if p.claude && !p.prompt_from_ticket));
        assert!(serde_json::from_value::<TplQuickAction>(json!({"name": "S", "action": "explode", "params": {}})).is_err());
    }

    #[test]
    fn old_documents_with_empty_quick_actions_still_load() {
        let f = fx();
        let mut doc = export_types(&f.c, f.project).unwrap();
        assert!(doc.types.iter().all(|t| t.quick_actions.is_empty()));
        let text = doc.to_json();
        assert!(text.contains("\"quick_actions\": []"));
        doc = TemplateDoc::from_json(&text).unwrap();
        let p = empty_project(&f.c, "OLD");
        let rep = import_types(&f.c, p, &doc).unwrap();
        assert_eq!(rep.created_quick_actions, 0);
    }

    #[test]
    fn export_uses_template_numbers_not_database_ids() {
        let f = fx();
        rich(&f);
        let doc = export_types(&f.c, f.project).unwrap();
        let story = doc.types.iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap();
        let epic = doc.types.iter().find(|t| t.key.as_deref() == Some("epic")).unwrap();
        let dev = doc.types.iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap();
        assert_eq!(story.quick_actions.len(), 3);
        let TplParams::CreateTicket(c) = &story.quick_actions[0].params else { panic!() };
        assert_eq!(c.ticket_type, dev.id, "Vorlagen-Nummer des Devtask-Typs");
        assert_eq!(c.relation.as_ref().unwrap().link_key.as_deref(), Some("blocks"));
        assert_eq!(c.defaults[0], TplDefault { field: FieldRef::Title, value: json!("Task for {{source.key}}") });
        let desc_no = dev.fields.iter().find(|x| x.key.as_deref() == Some("description")).unwrap().id;
        assert_eq!(c.defaults[1].field, FieldRef::Field(desc_no));
        let TplParams::ChangeStatus(s) = &story.quick_actions[1].params else { panic!() };
        assert_eq!(s.to_status, doc.statuses[1].id);
        assert_eq!(s.set_values[0].field, story.fields.iter().find(|x| x.key.as_deref() == Some("priority")).unwrap().id);
        // Epic → Story: der Versions-Default entfällt, der Ticket-Verweis auf das Ausgangsticket bleibt
        let TplParams::CreateTicket(c) = &epic.quick_actions[0].params else { panic!() };
        assert_eq!(c.defaults.len(), 1);
        assert_eq!(c.defaults[0].value, json!("{{source}}"));
        // das ganze Dokument enthält keine Aktion mit Datenbank-Ids (kleine Nummern 1..): Gegenprobe über Wiederholbarkeit
        assert_eq!(doc, export_types(&f.c, f.project).unwrap());
    }

    #[test]
    fn roundtrip_into_an_empty_project_gives_the_same_actions() {
        let f = fx();
        rich(&f);
        let doc = export_types(&f.c, f.project).unwrap();
        let text = doc.to_json();
        let p2 = empty_project(&f.c, "TWO");
        let rep = import_types(&f.c, p2, &TemplateDoc::from_json(&text).unwrap()).unwrap();
        assert_eq!(rep.created_quick_actions, 4);
        assert!(!rep.has_conflicts(), "{:?}", rep.conflicts);
        let doc2 = export_types(&f.c, p2).unwrap();
        for (a, b) in doc.types.iter().zip(&doc2.types) {
            assert_eq!(a.quick_actions, b.quick_actions, "{:?}", a.key);
        }
        assert_eq!(doc, doc2, "auch das ganze Dokument ist gleich");
    }

    #[test]
    fn imported_actions_point_at_the_new_projects_ids() {
        let f = fx();
        rich(&f);
        let doc = export_types(&f.c, f.project).unwrap();
        let p2 = empty_project(&f.c, "TWO");
        import_types(&f.c, p2, &doc).unwrap();
        let types2 = list_types(&f.c, p2).unwrap();
        let by = |k: &str| types2.iter().find(|t| t.key.as_deref() == Some(k)).unwrap().id;
        let statuses2 = list_statuses(&f.c, p2).unwrap();
        let story2 = by("userstory");
        let acts = list_quick_actions(&f.c, story2).unwrap();
        assert_eq!(acts.iter().map(|a| a.display(crate::state::Lang::En)).collect::<Vec<_>>(), vec!["Create Dev task", "Start", "Work in Claude"]);
        let ActionParams::CreateTicket(c) = &acts[0].params else { panic!() };
        assert_eq!(c.ticket_type_id, by("devtask"));
        assert_ne!(c.ticket_type_id, f.dev);
        let dev_desc = list_fields(&f.c, by("devtask")).unwrap().into_iter().find(|x| x.key.as_deref() == Some("description")).unwrap().id;
        assert_eq!(c.defaults[1].field_id, FieldRef::Field(dev_desc));
        assert_eq!(c.relation.as_ref().unwrap().link_type_id, link_type_by_key(&f.c, "blocks").unwrap().id);
        let ActionParams::ChangeStatus(s) = &acts[1].params else { panic!() };
        assert_eq!(s.to_status_id, statuses2[1].id);
        assert!(acts.iter().all(|a| !a.invalid && super::super::actions::check_quick_action(&f.c, a.id).is_ok()));
        // die Aktion des Ausgangsprojekts ist unverändert
        assert_eq!(list_quick_actions(&f.c, f.story).unwrap().len(), 3);
    }

    #[test]
    fn invalid_actions_are_not_exported() {
        let f = fx();
        let id = add(&f, f.story, "Create sub", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.sub, relation: None, defaults: vec![] }));
        add(&f, f.story, "Fine", ActionParams::StartSession(StartSessionParams::default()));
        f.c.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [id]).unwrap();
        let doc = export_types(&f.c, f.project).unwrap();
        let story = doc.types.iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap();
        assert_eq!(story.quick_actions.len(), 1);
        assert_eq!(story.quick_actions[0].name.as_deref(), Some("Fine"));
    }

    #[test]
    fn custom_link_types_travel_by_name_and_missing_ones_are_reported() {
        let f = fx();
        let spawn = create_link_type(&f.c, &NewLinkType { name_out: "spawns".into(), name_in: Some("spawned by".into()), symmetric: false, target_kind: TargetKind::Ticket }).unwrap();
        let p = ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: Some(RelationSpec { link_type_id: spawn, direction: LinkDirection::Out }), defaults: vec![] });
        add(&f, f.story, "Spawn", p);
        let doc = export_types(&f.c, f.project).unwrap();
        let story = doc.types.iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap();
        let TplParams::CreateTicket(c) = &story.quick_actions[0].params else { panic!() };
        assert_eq!(c.relation, Some(TplRelation { link_key: None, link_name: Some("spawns".into()), direction: LinkDirection::Out }));
        // gleiche Datenbank: der eigene Typ wird über den Namen gefunden
        let p2 = empty_project(&f.c, "TWO");
        let rep = import_types(&f.c, p2, &doc).unwrap();
        assert!(!rep.has_conflicts());
        let story2 = list_types(&f.c, p2).unwrap().into_iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        let ActionParams::CreateTicket(c2) = &list_quick_actions(&f.c, story2).unwrap()[0].params else { panic!() };
        assert_eq!(c2.relation.as_ref().unwrap().link_type_id, spawn);
        // in einer Datenbank ohne den Typ wird die Verknüpfung weggelassen und gemeldet
        delete_link_type(&f.c, spawn, None).unwrap();
        let p3 = empty_project(&f.c, "THR");
        let rep = import_types(&f.c, p3, &doc).unwrap();
        assert_eq!(rep.created_quick_actions, 1);
        assert_eq!(rep.conflicts.len(), 1);
        assert_eq!(rep.conflicts[0].name.as_deref(), Some("Spawn"));
        assert!(rep.conflicts[0].message(crate::state::Lang::En).contains("Spawn") && rep.conflicts[0].message(crate::state::Lang::De).contains("Spawn"));
        let story3 = list_types(&f.c, p3).unwrap().into_iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        let ActionParams::CreateTicket(c3) = &list_quick_actions(&f.c, story3).unwrap()[0].params else { panic!() };
        assert_eq!(c3.relation, None);
    }

    #[test]
    fn broken_references_abort_the_import_without_a_trace() {
        let f = fx();
        rich(&f);
        let doc = export_types(&f.c, f.project).unwrap();
        let mut bad_type = doc.clone();
        let story = bad_type.types.iter_mut().find(|t| t.key.as_deref() == Some("userstory")).unwrap();
        if let TplParams::CreateTicket(c) = &mut story.quick_actions[0].params {
            c.ticket_type = 99;
        }
        let mut bad_status = doc.clone();
        let story = bad_status.types.iter_mut().find(|t| t.key.as_deref() == Some("userstory")).unwrap();
        if let TplParams::ChangeStatus(c) = &mut story.quick_actions[1].params {
            c.to_status = 99;
        }
        let mut bad_field = doc.clone();
        let story = bad_field.types.iter_mut().find(|t| t.key.as_deref() == Some("userstory")).unwrap();
        if let TplParams::ChangeStatus(c) = &mut story.quick_actions[1].params {
            c.set_values[0].field = 99;
        }
        let mut bad_value = doc.clone();
        let story = bad_value.types.iter_mut().find(|t| t.key.as_deref() == Some("userstory")).unwrap();
        if let TplParams::StartSession(c) = &mut story.quick_actions[2].params {
            c.prompt_from_ticket = true;
            c.claude = false;
        }
        for (i, d) in [bad_type, bad_status, bad_field, bad_value].into_iter().enumerate() {
            let p = empty_project(&f.c, &format!("BA{i}"));
            let before: i64 = f.c.query_row("SELECT COUNT(*) FROM quick_actions", [], |r| r.get(0)).unwrap();
            let r = import_types(&f.c, p, &d);
            assert!(matches!(r, Err(PlanError::Template(_))), "{i}: {r:?}");
            let after: i64 = f.c.query_row("SELECT COUNT(*) FROM quick_actions", [], |r| r.get(0)).unwrap();
            assert_eq!(before, after, "{i}");
            assert!(list_types(&f.c, p).unwrap().is_empty(), "{i}: auch Typen und Status bleiben weg");
        }
    }

    #[test]
    fn defaults_for_fields_of_a_kept_type_are_mapped_by_name_or_dropped() {
        // Quellprojekt: Devtask hat „Aufwand“; Userstory-Aktion legt Devtasks mit „Aufwand“ an.
        let f = fx();
        let mut nf = NewField::new(f.dev, "Effort", FieldKind::Text);
        nf.options.max_length = Some(20);
        let effort = create_field(&f.c, &nf).unwrap();
        let nf2 = NewField::new(f.dev, "Extra", FieldKind::Text);
        let extra = create_field(&f.c, &nf2).unwrap();
        add(
            &f,
            f.story,
            "Create Dev task",
            ActionParams::CreateTicket(CreateTicketParams {
                ticket_type_id: f.dev,
                relation: None,
                defaults: vec![
                    DefaultValue { field_id: FieldRef::Field(effort), value: json!("big") },
                    DefaultValue { field_id: FieldRef::Field(extra), value: json!("x") },
                ],
            }),
        );
        let doc = export_types(&f.c, f.project).unwrap();
        // Zielprojekt hat schon einen Devtask – mit „Effort“, aber ohne „Extra“
        let p2 = empty_project(&f.c, "TWO");
        let dev2 = crate::planning::types::create_type(&f.c, &crate::planning::types::NewType { project_id: p2, key: Some("devtask".into()), name: None, icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let mut ef = NewField::new(dev2, "Effort", FieldKind::Text);
        ef.options.max_length = Some(20);
        let effort2 = create_field(&f.c, &ef).unwrap();
        let rep = import_types(&f.c, p2, &doc).unwrap();
        assert!(rep.conflicts.iter().any(|c| c.kind == crate::planning::template::ConflictKind::Type), "der vorhandene Devtask bleibt unberührt");
        assert_eq!(rep.created_quick_actions, 1);
        let q = rep.conflicts.iter().find(|c| c.kind == crate::planning::template::ConflictKind::QuickAction).expect("unvollständig gemeldet");
        assert_eq!(q.name.as_deref(), Some("Create Dev task"));
        let story2 = list_types(&f.c, p2).unwrap().into_iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        let ActionParams::CreateTicket(c) = &list_quick_actions(&f.c, story2).unwrap()[0].params else { panic!() };
        assert_eq!(c.ticket_type_id, dev2);
        assert_eq!(c.defaults, vec![DefaultValue { field_id: FieldRef::Field(effort2), value: json!("big") }]);
        // die Aktionen des vorhandenen Typs wurden nicht angefasst
        assert!(list_quick_actions(&f.c, dev2).unwrap().is_empty());
    }

    #[test]
    fn importing_twice_does_not_duplicate_actions() {
        let f = fx();
        rich(&f);
        let doc = export_types(&f.c, f.project).unwrap();
        let p2 = empty_project(&f.c, "TWO");
        import_types(&f.c, p2, &doc).unwrap();
        let rep = import_types(&f.c, p2, &doc).unwrap();
        assert_eq!(rep.created_quick_actions, 0, "gleichnamige Typen werden samt Aktionen übersprungen");
        let n: i64 = f.c.query_row("SELECT COUNT(*) FROM quick_actions WHERE type_id IN (SELECT id FROM ticket_types WHERE project_id = ?1)", [p2], |r| r.get(0)).unwrap();
        assert_eq!(n, 4);
    }
}
