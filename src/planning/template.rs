//! Vorlagen-Export/-Import der Typ-Struktur eines Projekts (nur Datenstruktur, keine Oberfläche).
//!
//! * [`export_types`] liefert ein [`TemplateDoc`] (JSON mit Format-Version): Status-Pool, Typen mit Hierarchie, Workflow
//!   (gewählte Status, Übergänge samt Regeln) und Feldern sowie die Arten der Arbeit. Die Kennungen im Dokument sind
//!   fortlaufende Vorlagen-Nummern (1, 2, …) und unabhängig von den Datenbank-Ids – zwei gleich aufgebaute Projekte ergeben
//!   dasselbe Dokument. Verweise (Eltern-Typen, Workflow-Status, Felder in Übergangsregeln, Ziel-Typen von Ticket-Feldern)
//!   benutzen diese Nummern. Dropdown-Werte behalten ihre Kennung im Feld.
//! * [`import_types`] nimmt genau dieses Format, legt fehlende Status, Typen, Felder, Workflows und Arten der Arbeit im
//!   Zielprojekt an und **ändert nie Bestehendes**: heißt etwas (in einer der Sprachen) schon so, wird es nicht
//!   umbenannt, sondern im [`ImportReport`] gemeldet und übersprungen (ein gleichnamiger Status wird für die Workflows der
//!   neuen Typen mitbenutzt, ein gleichnamiger Typ samt seinen Feldern/Regeln nicht angefasst). Der Import ist atomar:
//!   bei einem Fehler bleibt das Projekt unverändert.
//! * `quick_actions` enthält die Quick-Actions des Typs (`quick/tpl.rs`) mit Vorlagen-Nummern statt Datenbank-Ids; sie werden
//!   zusammen mit dem Typ angelegt (bei gleichnamigen, übersprungenen Typen nicht).

use super::fields::{create_field, list_fields, FieldKind, FieldOptions, NewField};
use super::names;
use super::projects::editable;
use super::quick::tpl as quick_tpl;
use super::statuses::{create_status, NewStatus, StatusCategory};
use super::summary::project_summary;
use super::types::{create_type, set_parents, NewType};
use super::work_kinds::{create_work_kind, list_work_kinds};
use super::workflow::{replace_transitions, set_workflow_statuses, TransitionDef};
use super::{common, PlanError, Res, TemplateProblem};
use crate::state::Lang;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Kennung des Dokumentformats.
pub const FORMAT: &str = "paneldeck-type-template";
/// Aktuelle Format-Version.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateDoc {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub statuses: Vec<TplStatus>,
    #[serde(default)]
    pub types: Vec<TplType>,
    #[serde(default)]
    pub work_kinds: Vec<TplWorkKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplStatus {
    pub id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub color: String,
    pub category: StatusCategory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplType {
    pub id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub color: String,
    pub workflow: bool,
    /// Vorlagen-Nummern der erlaubten Eltern-Typen.
    #[serde(default)]
    pub parents: Vec<i64>,
    /// Vorlagen-Nummern der Workflow-Status in Reihenfolge.
    #[serde(default)]
    pub workflow_statuses: Vec<i64>,
    /// Übergänge mit Vorlagen-Nummern für Status und Felder.
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
    #[serde(default)]
    pub fields: Vec<TplField>,
    /// Quick-Actions des Typs (Vorlagen-Nummern statt Datenbank-Ids).
    #[serde(default)]
    pub quick_actions: Vec<quick_tpl::TplQuickAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplField {
    pub id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub kind: FieldKind,
    #[serde(default)]
    pub options: FieldOptions,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub section: String,
    #[serde(default)]
    pub show_on_card: bool,
    #[serde(default)]
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TplWorkKind {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl TemplateDoc {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Liest ein Dokument und prüft Format und Version.
    pub fn from_json(text: &str) -> Res<TemplateDoc> {
        let text = text.trim_start_matches('\u{feff}');
        let v: Value = serde_json::from_str(text).map_err(|e| PlanError::Template(TemplateProblem::Json(e.to_string())))?;
        if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err(PlanError::Template(TemplateProblem::Format));
        }
        let found = v.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
        if found > VERSION {
            return Err(PlanError::Template(TemplateProblem::Version { found, supported: VERSION }));
        }
        serde_json::from_value(v).map_err(|e| PlanError::Template(TemplateProblem::Json(e.to_string())))
    }
}

// ------------------------------------------------------------------ Export

/// Exportiert Status, Typen, Felder, Workflows, Hierarchie und Arten der Arbeit des Projekts. System-Typen entfallen.
pub fn export_types(conn: &Connection, project_id: i64) -> Res<TemplateDoc> {
    let s = project_summary(conn, project_id)?;
    let status_no: HashMap<i64, i64> = s.statuses.iter().enumerate().map(|(i, st)| (st.id, i as i64 + 1)).collect();
    let types: Vec<_> = s.types.iter().filter(|t| !t.ticket_type.system).collect();
    let type_no: HashMap<i64, i64> = types.iter().enumerate().map(|(i, t)| (t.ticket_type.id, i as i64 + 1)).collect();
    let mut field_no: HashMap<i64, i64> = HashMap::new();
    for t in &types {
        for f in &t.fields {
            let n = field_no.len() as i64 + 1;
            field_no.insert(f.id, n);
        }
    }
    // Verweise auf Tickets, Versionen, Meilensteine und Sitzungen gelten nur in diesem Projekt: Werte und Vergleiche darauf gehören nicht in die Vorlage
    let ref_fields: std::collections::HashSet<i64> = types.iter().flat_map(|t| t.fields.iter()).filter(|f| f.kind.is_reference()).map(|f| f.id).collect();
    let map_all = |ids: &[i64], m: &HashMap<i64, i64>| ids.iter().filter_map(|i| m.get(i).copied()).collect::<Vec<_>>();
    let mut quick: HashMap<i64, Vec<quick_tpl::TplQuickAction>> = HashMap::new();
    {
        let maps = quick_tpl::Maps { types: &type_no, statuses: &status_no, fields: &field_no };
        for t in &types {
            quick.insert(t.ticket_type.id, quick_tpl::export_actions(conn, t.ticket_type.id, &maps)?);
        }
    }
    let out_types = types
        .iter()
        .map(|t| {
            let tt = &t.ticket_type;
            TplType {
                id: type_no[&tt.id],
                key: tt.key.clone(),
                name: tt.name.clone(),
                icon: tt.icon.clone(),
                color: tt.color.clone(),
                workflow: tt.workflow,
                parents: map_all(&t.parents, &type_no),
                workflow_statuses: map_all(&t.workflow_statuses, &status_no),
                transitions: t
                    .transitions
                    .iter()
                    .map(|x| TransitionDef {
                        from_status_id: status_no[&x.def.from_status_id],
                        to_status_id: status_no[&x.def.to_status_id],
                        required_fields: map_all(&x.def.required_fields, &field_no),
                        conditions: x
                            .def
                            .conditions
                            .iter()
                            .filter(|c| !ref_fields.contains(&c.field_id) || matches!(c.op, super::workflow::ConditionOp::IsEmpty | super::workflow::ConditionOp::IsNotEmpty))
                            .filter_map(|c| field_no.get(&c.field_id).map(|n| super::workflow::Condition { field_id: *n, ..c.clone() }))
                            .collect(),
                        set_values: x.def.set_values.iter().filter(|c| !ref_fields.contains(&c.field_id)).filter_map(|c| field_no.get(&c.field_id).map(|n| super::workflow::SetValue { field_id: *n, value: c.value.clone() })).collect(),
                    })
                    .collect(),
                fields: t
                    .fields
                    .iter()
                    .map(|f| {
                        let mut options = f.options.clone();
                        options.target_types = map_all(&options.target_types, &type_no);
                        TplField {
                            id: field_no[&f.id],
                            key: f.key.clone(),
                            name: f.name.clone(),
                            kind: f.kind,
                            options,
                            required: f.required,
                            default_value: f.default_value.clone(),
                            help: f.help.clone(),
                            section: f.section.clone(),
                            show_on_card: f.show_on_card,
                            builtin: f.builtin,
                        }
                    })
                    .collect(),
                quick_actions: quick.remove(&tt.id).unwrap_or_default(),
            }
        })
        .collect();
    Ok(TemplateDoc {
        format: FORMAT.to_string(),
        version: VERSION,
        statuses: s.statuses.iter().map(|st| TplStatus { id: status_no[&st.id], key: st.key.clone(), name: st.name.clone(), color: st.color.clone(), category: st.category }).collect(),
        types: out_types,
        work_kinds: s.work_kinds.iter().map(|w| TplWorkKind { key: w.key.clone(), name: w.name.clone() }).collect(),
    })
}

// ------------------------------------------------------------------ Import

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    Status,
    Type,
    WorkKind,
    /// Eine Quick-Action wurde ohne Teile übernommen, auf die sie verweist (z. B. Verknüpfungstyp, Feld eines vorhandenen Typs).
    QuickAction,
}

/// Etwas aus der Vorlage gibt es im Projekt schon (gleicher Schlüssel oder Name in irgendeiner Sprache) und wurde nicht angefasst.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportConflict {
    pub kind: ConflictKind,
    pub key: Option<String>,
    pub name: Option<String>,
}

impl ImportConflict {
    pub fn message(&self, lang: Lang) -> String {
        let label = names::display_name(self.key.as_deref(), self.name.as_deref(), lang);
        let q = lang.q(&label);
        match (self.kind, lang) {
            (ConflictKind::Status, Lang::De) => format!("Der Status {q} gibt es schon – nicht importiert."),
            (ConflictKind::Status, Lang::En) => format!("The status {q} already exists – not imported."),
            (ConflictKind::Type, Lang::De) => format!("Den Typ {q} gibt es schon – nicht importiert (samt Feldern und Workflow)."),
            (ConflictKind::Type, Lang::En) => format!("The type {q} already exists – not imported (including its fields and workflow)."),
            (ConflictKind::WorkKind, Lang::De) => format!("Die Art der Arbeit {q} gibt es schon – nicht importiert."),
            (ConflictKind::WorkKind, Lang::En) => format!("The kind of work {q} already exists – not imported."),
            (ConflictKind::QuickAction, Lang::De) => format!("Die Schnellaktion {q} wurde ohne einige Verweise importiert, die es hier nicht gibt."),
            (ConflictKind::QuickAction, Lang::En) => format!("The quick action {q} was imported without some references that do not exist here."),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub created_statuses: usize,
    pub created_types: usize,
    pub created_fields: usize,
    pub created_work_kinds: usize,
    pub created_quick_actions: usize,
    pub conflicts: Vec<ImportConflict>,
}

impl ImportReport {
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }
}

fn reference(what: &str, id: i64) -> PlanError {
    PlanError::Template(TemplateProblem::Reference(format!("{what} {id}")))
}

fn insert_unique(map: &mut HashMap<i64, i64>, local: i64, real: i64, what: &str) -> Res<()> {
    if map.insert(local, real).is_some() {
        return Err(PlanError::Template(TemplateProblem::Invalid(format!("{what} {local} (doppelt)"))));
    }
    Ok(())
}

/// Importiert die Vorlage ins Projekt (siehe Moduldoku): legt an, was fehlt, meldet Gleichnamiges, ändert nichts Bestehendes.
pub fn import_types(conn: &Connection, project_id: i64, doc: &TemplateDoc) -> Res<ImportReport> {
    editable(conn, project_id)?;
    if doc.format != FORMAT {
        return Err(PlanError::Template(TemplateProblem::Format));
    }
    if doc.version > VERSION {
        return Err(PlanError::Template(TemplateProblem::Version { found: doc.version, supported: VERSION }));
    }
    common::atomic(conn, || {
        let mut rep = ImportReport::default();
        // 1. Status (bestehende gleichen Namens werden für die Workflows der neuen Typen mitbenutzt)
        let mut status_map: HashMap<i64, i64> = HashMap::new();
        for s in &doc.statuses {
            let existing = super::statuses::list_statuses(conn, project_id)?.into_iter().find(|e| common::same_label(s.key.as_deref(), s.name.as_deref(), e.key.as_deref(), e.name.as_deref()));
            let real = match existing {
                Some(e) => {
                    rep.conflicts.push(ImportConflict { kind: ConflictKind::Status, key: s.key.clone(), name: s.name.clone() });
                    e.id
                }
                None => {
                    rep.created_statuses += 1;
                    create_status(conn, &NewStatus { project_id, key: s.key.clone(), name: s.name.clone(), color: s.color.clone(), category: s.category })?
                }
            };
            insert_unique(&mut status_map, s.id, real, "status")?;
        }
        // 2. Typen anlegen (gleichnamige bleiben unberührt und werden samt ihrem Inhalt übersprungen)
        let mut type_map: HashMap<i64, i64> = HashMap::new();
        let mut created: Vec<(&TplType, i64)> = Vec::new();
        for t in &doc.types {
            let existing = super::types::list_types(conn, project_id)?.into_iter().find(|e| common::same_label(t.key.as_deref(), t.name.as_deref(), e.key.as_deref(), e.name.as_deref()));
            match existing {
                Some(e) => {
                    rep.conflicts.push(ImportConflict { kind: ConflictKind::Type, key: t.key.clone(), name: t.name.clone() });
                    insert_unique(&mut type_map, t.id, e.id, "type")?;
                }
                None => {
                    let id = create_type(conn, &NewType { project_id, key: t.key.clone(), name: t.name.clone(), icon: t.icon.clone(), color: t.color.clone(), workflow: t.workflow })?;
                    rep.created_types += 1;
                    insert_unique(&mut type_map, t.id, id, "type")?;
                    created.push((t, id));
                }
            }
        }
        // 3. Felder der neuen Typen
        let mut field_map: HashMap<i64, i64> = HashMap::new();
        for (t, type_id) in &created {
            // eingebaute Felder (Beschreibung, Fälligkeit, Labels) gibt es im neuen Typ schon: zuordnen statt anlegen – wie beim
            // Update auch gleichnamige eigene Felder älterer Vorlagen/Sicherungen
            let targets = super::builtin_fields::builtin_targets(&t.fields.iter().map(|f| (f.key.as_deref(), f.name.as_deref(), f.kind)).collect::<Vec<_>>());
            for (fi, f) in t.fields.iter().enumerate() {
                let fixed_key = f.key.as_deref().filter(|k| super::builtin_fields::FIXED_KEYS.contains(k));
                let mut key = f.key.clone();
                let mut name = f.name.clone();
                if let Some(k) = targets[fi] {
                    {
                        let id = super::builtin_fields::field_id(conn, *type_id, k)?.ok_or(PlanError::NotFound)?;
                        // ein eigener Anzeigename eines Feldes mit Schlüssel bleibt (z. B. umbenannte Beschreibung)
                        if let (true, Some(n)) = (f.key.is_some(), f.name.as_deref().map(str::trim).filter(|n| !n.is_empty())) {
                            conn.execute("UPDATE fields SET name = ?1 WHERE id = ?2", rusqlite::params![common::clean_name(n)?, id])?;
                        }
                        super::fields::update_field(conn, id, &super::fields::UpdateField { help: Some(f.help.clone()), section: Some(f.section.clone()), show_on_card: Some(f.show_on_card), ..Default::default() })?;
                        insert_unique(&mut field_map, f.id, id, "field")?;
                        rep.created_fields += 1;
                        continue;
                    }
                }
                if let Some(k) = fixed_key {
                    // anderer Feldtyp: bleibt ein eigenes Feld ohne Schlüssel
                    key = None;
                    if name.is_none() {
                        name = Some(format!("{} (custom)", super::names::display_name(Some(k), None, crate::state::Lang::En)));
                    }
                }
                // ein eigenes Feld, das wie ein eingebautes heißt (z. B. „Due date“), bekommt wie beim Update den Zusatz „(custom)“
                if let Some(n) = name.as_deref().filter(|n| key.is_none() && super::builtin_fields::is_builtin_label(n)) {
                    name = Some(format!("{} (custom)", n.trim()));
                }
                let mut options = f.options.clone();
                options.target_types = options.target_types.iter().map(|x| type_map.get(x).copied().ok_or_else(|| reference("type", *x))).collect::<Res<Vec<_>>>()?;
                let id = create_field(
                    conn,
                    &NewField {
                        type_id: *type_id,
                        key,
                        name,
                        kind: f.kind,
                        options,
                        required: f.required,
                        default_value: f.default_value.clone(),
                        help: f.help.clone(),
                        section: f.section.clone(),
                        show_on_card: f.show_on_card,
                        builtin: f.builtin,
                    },
                )?;
                rep.created_fields += 1;
                insert_unique(&mut field_map, f.id, id, "field")?;
            }
        }
        // Felder schon vorhandener gleichnamiger Typen: für Verweise der Quick-Actions (Zuordnung über den Namen)
        for t in &doc.types {
            let Some(real) = type_map.get(&t.id) else { continue };
            if created.iter().any(|(_, id)| id == real) {
                continue;
            }
            let have = list_fields(conn, *real)?;
            for f in &t.fields {
                if let Some(e) = have.iter().find(|e| common::same_label(f.key.as_deref(), f.name.as_deref(), e.key.as_deref(), e.name.as_deref())) {
                    field_map.entry(f.id).or_insert(e.id);
                }
            }
        }
        // 4. Hierarchie-Regeln der neuen Typen (Eltern: neue oder bereits vorhandene Typen)
        for (t, type_id) in &created {
            if t.parents.is_empty() {
                continue;
            }
            let parents = t.parents.iter().map(|p| type_map.get(p).copied().ok_or_else(|| reference("type", *p))).collect::<Res<Vec<_>>>()?;
            set_parents(conn, *type_id, &parents)?;
        }
        // 5. Workflows der neuen Typen
        for (t, type_id) in &created {
            if !t.workflow {
                if !t.workflow_statuses.is_empty() || !t.transitions.is_empty() {
                    return Err(PlanError::Template(TemplateProblem::Invalid(format!("type {} (Workflow ohne Workflow)", t.id))));
                }
                continue;
            }
            let wf = t.workflow_statuses.iter().map(|s| status_map.get(s).copied().ok_or_else(|| reference("status", *s))).collect::<Res<Vec<_>>>()?;
            set_workflow_statuses(conn, *type_id, &wf)?;
            let st = |id: i64| status_map.get(&id).copied().ok_or_else(|| reference("status", id));
            let fl = |id: i64| field_map.get(&id).copied().ok_or_else(|| reference("field", id));
            let mut defs = Vec::new();
            for x in &t.transitions {
                defs.push(TransitionDef {
                    from_status_id: st(x.from_status_id)?,
                    to_status_id: st(x.to_status_id)?,
                    required_fields: x.required_fields.iter().map(|f| fl(*f)).collect::<Res<Vec<_>>>()?,
                    conditions: x.conditions.iter().map(|c| Ok(super::workflow::Condition { field_id: fl(c.field_id)?, ..c.clone() })).collect::<Res<Vec<_>>>()?,
                    set_values: x.set_values.iter().map(|c| Ok(super::workflow::SetValue { field_id: fl(c.field_id)?, value: c.value.clone() })).collect::<Res<Vec<_>>>()?,
                });
            }
            replace_transitions(conn, *type_id, &defs)?;
        }
        // 5b. Quick-Actions der neuen Typen (brauchen Typen, Felder und Workflows)
        let maps = quick_tpl::Maps { types: &type_map, statuses: &status_map, fields: &field_map };
        for (t, type_id) in &created {
            if t.quick_actions.is_empty() {
                continue;
            }
            let out = quick_tpl::import_actions(conn, *type_id, &t.quick_actions, &maps)?;
            rep.created_quick_actions += out.created;
            for (key, name) in out.incomplete {
                rep.conflicts.push(ImportConflict { kind: ConflictKind::QuickAction, key, name });
            }
        }
        // 6. Arten der Arbeit
        for w in &doc.work_kinds {
            let exists = list_work_kinds(conn, project_id)?.iter().any(|e| common::same_label(w.key.as_deref(), w.name.as_deref(), e.key.as_deref(), e.name.as_deref()));
            if exists {
                rep.conflicts.push(ImportConflict { kind: ConflictKind::WorkKind, key: w.key.clone(), name: w.name.clone() });
            } else {
                create_work_kind(conn, project_id, w.key.as_deref(), w.name.as_deref())?;
                rep.created_work_kinds += 1;
            }
        }
        Ok(rep)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{list_fields, update_field, UpdateField};
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::statuses::{list_statuses, update_status, UpdateStatus};
    use crate::planning::testutil::db;
    use crate::planning::types::list_types;
    use crate::planning::workflow::{Condition, ConditionOp, SetValue};
    use serde_json::json;

    fn project(c: &Connection, key: &str, defaults: bool, versions: bool) -> i64 {
        let mut p = NewProject::new(key, "P", "#112233");
        p.with_defaults = defaults;
        p.use_versions = versions;
        create_project(c, &p).unwrap()
    }

    /// Das Standard-Projekt mit zusätzlichen Eigenheiten: umbenannter Status, Regeln in einer Übergangsmatrix, eigenes Feld.
    fn rich_project(c: &Connection, key: &str) -> i64 {
        let p = project(c, key, true, true);
        let s = list_statuses(c, p).unwrap();
        update_status(c, s[1].id, &UpdateStatus { name: Some("Entwicklung".into()), ..Default::default() }).unwrap();
        let ty = list_types(c, p).unwrap();
        let dev = ty.iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
        let f = list_fields(c, dev).unwrap();
        let prio = f.iter().find(|x| x.key.as_deref() == Some("priority")).unwrap().clone();
        let sp = f.iter().find(|x| x.key.as_deref() == Some("story_points")).unwrap().id;
        let mut num = NewField::new(dev, "Aufwand", FieldKind::Number);
        num.options.min = Some(0.5);
        num.options.max = Some(40.0);
        num.options.decimals = Some(1);
        num.options.unit = Some("h".into());
        num.section = "Planung".into();
        num.help = "Geschätzte Stunden".into();
        create_field(c, &num).unwrap();
        update_field(c, sp, &UpdateField { required: Some(true), ..Default::default() }).unwrap();
        let mut o = NewField::new(dev, "Tags", FieldKind::Labels);
        o.show_on_card = true;
        create_field(c, &o).unwrap();
        // Regeln auf dem Übergang In Progress -> Done
        crate::planning::workflow::set_transition(
            c,
            dev,
            &TransitionDef {
                from_status_id: s[1].id,
                to_status_id: s[2].id,
                required_fields: vec![sp],
                conditions: vec![Condition { field_id: prio.id, op: ConditionOp::NotIn, value: json!([5]), allow: false }],
                set_values: vec![SetValue { field_id: prio.id, value: json!(2) }],
            },
        )
        .unwrap();
        // Hierarchie ändern: Devtask nur noch unter Userstory
        let us = ty.iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        crate::planning::types::set_parents(c, dev, &[us]).unwrap();
        p
    }

    #[test]
    fn roundtrip_export_empty_project_import_gives_the_same_structure() {
        let c = db();
        let a = rich_project(&c, "AA");
        let doc = export_types(&c, a).unwrap();
        assert_eq!((doc.format.as_str(), doc.version), (FORMAT, VERSION));
        assert_eq!(doc.statuses.len(), 3);
        assert_eq!(doc.types.len(), 5);
        assert_eq!(doc.work_kinds.len(), 4);
        // über JSON hin und zurück
        let doc = TemplateDoc::from_json(&doc.to_json()).unwrap();
        let b = project(&c, "BB", false, true);
        let rep = import_types(&c, b, &doc).unwrap();
        assert!(!rep.has_conflicts(), "{:?}", rep.conflicts);
        assert_eq!((rep.created_statuses, rep.created_types, rep.created_work_kinds), (3, 5, 4));
        assert_eq!(rep.created_fields, doc.types.iter().map(|t| t.fields.len()).sum::<usize>());
        let again = export_types(&c, b).unwrap();
        assert_eq!(again, doc, "Export des Ziels ist dem Quell-Dokument gleich");
        assert_eq!(export_types(&c, a).unwrap(), again);
        // Struktur im Ziel: Typen, Regeln und eigene Namen sind angekommen
        let tys = list_types(&c, b).unwrap();
        let dev = tys.iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
        let fields = list_fields(&c, dev).unwrap();
        assert!(fields.iter().any(|f| f.name.as_deref() == Some("Aufwand") && f.options.unit.as_deref() == Some("h") && f.section == "Planung"));
        assert_eq!(crate::planning::workflow::list_transitions(&c, dev).unwrap().iter().filter(|t| !t.def.conditions.is_empty()).count(), 1);
        let tr = crate::planning::workflow::list_transitions(&c, dev).unwrap();
        let ruled = tr.iter().find(|t| !t.def.conditions.is_empty()).unwrap();
        assert_eq!(ruled.def.required_fields.len(), 1);
        assert!(fields.iter().any(|f| f.id == ruled.def.required_fields[0] && f.key.as_deref() == Some("story_points")), "Pflichtfeld verweist auf das neue Feld des Ziels");
        assert!(fields.iter().any(|f| f.id == ruled.def.conditions[0].field_id && f.key.as_deref() == Some("priority")));
        assert_eq!(list_statuses(&c, b).unwrap()[1].name.as_deref(), Some("Entwicklung"));
        // Ticket-Feld „Epic“ zeigt auf den Epic-Typ des Ziels
        let us = tys.iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        let epic = tys.iter().find(|t| t.key.as_deref() == Some("epic")).unwrap().id;
        let ef = list_fields(&c, us).unwrap().into_iter().find(|f| f.key.as_deref() == Some("epic")).unwrap();
        assert_eq!(ef.options.target_types, vec![epic]);
        assert_eq!(crate::planning::types::parents_of(&c, dev).unwrap(), vec![us]);
        // Das Quellprojekt ist unverändert
        assert_eq!(list_types(&c, a).unwrap().len(), 5);
    }

    #[test]
    fn export_of_default_projects_is_identical() {
        // Zwei Projekte mit denselben Standards ergeben dasselbe Dokument (Kennungen sind Vorlagen-Nummern).
        let c = db();
        let a = project(&c, "AA", true, true);
        let b = project(&c, "BB", true, true);
        assert_eq!(export_types(&c, a).unwrap(), export_types(&c, b).unwrap());
        let e = export_types(&c, project(&c, "CC", false, false)).unwrap();
        assert!(e.statuses.is_empty() && e.types.is_empty() && e.work_kinds.is_empty());
        let todo = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert!(export_types(&c, todo).unwrap().types.is_empty(), "der System-Typ wird nicht exportiert");
        assert_eq!(export_types(&c, 999), Err(PlanError::NotFound));
    }

    #[test]
    fn import_reports_conflicts_and_never_changes_existing_data() {
        let c = db();
        let src = rich_project(&c, "AA");
        let doc = export_types(&c, src).unwrap();
        // Ziel: hat schon „Epic“ (anders gestaltet), einen gleichnamigen Status und eine Art der Arbeit
        let t = project(&c, "BB", false, false);
        let st = create_status(&c, &NewStatus { project_id: t, key: None, name: Some("To Do".into()), color: "#010203".into(), category: StatusCategory::Todo }).unwrap();
        let epic = create_type(&c, &NewType { project_id: t, key: None, name: Some("Epic".into()), icon: Some("fire".into()), color: "#AA0000".into(), workflow: false }).unwrap();
        create_field(&c, &NewField::new(epic, "Eigenes", FieldKind::Text)).unwrap();
        create_work_kind(&c, t, None, Some("Testing")).unwrap();
        let before = export_types(&c, t).unwrap();
        let rep = import_types(&c, t, &doc).unwrap();
        // Konflikte: Status „To Do“, Typ „Epic“, Art der Arbeit „Testing“
        assert_eq!(rep.conflicts.iter().map(|x| x.kind).collect::<Vec<_>>(), vec![ConflictKind::Status, ConflictKind::Type, ConflictKind::WorkKind]);
        assert_eq!((rep.created_statuses, rep.created_types, rep.created_work_kinds), (2, 4, 3));
        for l in [Lang::De, Lang::En] {
            for x in &rep.conflicts {
                assert!(!x.message(l).is_empty());
            }
        }
        assert!(rep.conflicts[0].message(Lang::En).contains("To Do") && rep.conflicts[1].message(Lang::De).contains("Epic"));
        // Bestehendes ist unverändert
        let e = crate::planning::types::get_type(&c, epic).unwrap();
        assert_eq!((e.icon.as_deref(), e.color.as_str(), e.workflow), (Some("fire"), "#AA0000", false));
        assert_eq!(list_fields(&c, epic).unwrap().iter().filter(|f| !f.is_fixed()).count(), 1);
        assert!(crate::planning::workflow::workflow_statuses(&c, epic).unwrap().is_empty());
        assert_eq!(crate::planning::statuses::get_status(&c, st).unwrap().color, "#010203");
        let after = export_types(&c, t).unwrap();
        assert_eq!(after.statuses[0], before.statuses[0]);
        // Der vorhandene Status wird in den Workflows der neuen Typen mitbenutzt
        let us = list_types(&c, t).unwrap().into_iter().find(|x| x.key.as_deref() == Some("userstory")).unwrap();
        assert!(crate::planning::workflow::workflow_statuses(&c, us.id).unwrap().contains(&st));
        // „Userstory“ liegt jetzt unter dem bestehenden Epic
        assert_eq!(crate::planning::types::parents_of(&c, us.id).unwrap(), vec![epic]);
        // Zweiter Import derselben Vorlage: nur noch Konflikte, nichts Neues
        let n_types = list_types(&c, t).unwrap().len();
        let rep2 = import_types(&c, t, &doc).unwrap();
        assert_eq!((rep2.created_statuses, rep2.created_types, rep2.created_fields, rep2.created_work_kinds), (0, 0, 0, 0));
        assert_eq!(rep2.conflicts.len(), 3 + 5 + 4);
        assert_eq!(list_types(&c, t).unwrap().len(), n_types);
    }

    #[test]
    fn import_is_atomic_and_validates_the_document() {
        let c = db();
        let src = project(&c, "AA", true, true);
        let good = export_types(&c, src).unwrap();
        let target = project(&c, "BB", false, false);
        let count = |c: &Connection| (list_statuses(c, target).unwrap().len(), list_types(c, target).unwrap().len());
        // Verweis auf einen fehlenden Status
        let mut bad = good.clone();
        bad.types[2].workflow_statuses.push(99);
        assert!(matches!(import_types(&c, target, &bad), Err(PlanError::Template(TemplateProblem::Reference(_)))));
        assert_eq!(count(&c), (0, 0), "nichts bleibt zurück");
        // fehlender Eltern-Typ
        let mut bad = good.clone();
        bad.types[1].parents = vec![42];
        assert!(matches!(import_types(&c, target, &bad), Err(PlanError::Template(TemplateProblem::Reference(_)))));
        // ungültige Farbe
        let mut bad = good.clone();
        bad.statuses[0].color = "grün".into();
        assert_eq!(import_types(&c, target, &bad), Err(PlanError::ColorInvalid));
        // Kreis in der Hierarchie
        let mut bad = good.clone();
        bad.types[0].parents = vec![bad.types[1].id];
        assert_eq!(import_types(&c, target, &bad), Err(PlanError::HierarchyCycle));
        // doppelte Kennung
        let mut bad = good.clone();
        bad.statuses[1].id = bad.statuses[0].id;
        assert!(matches!(import_types(&c, target, &bad), Err(PlanError::Template(TemplateProblem::Invalid(_)))));
        // Workflow-Daten bei Typ ohne Workflow
        let mut bad = good.clone();
        bad.types[0].workflow = false;
        assert!(matches!(import_types(&c, target, &bad), Err(PlanError::Template(TemplateProblem::Invalid(_)))));
        // Übergangsregel mit unbekanntem Feld
        let mut bad = good.clone();
        bad.types[2].transitions[0].required_fields = vec![9999];
        assert!(matches!(import_types(&c, target, &bad), Err(PlanError::Template(TemplateProblem::Reference(_)))));
        // Optionen ungültig
        let mut bad = good.clone();
        bad.types[0].fields[3].options.values[0].color = Some("x".into());
        assert_eq!(import_types(&c, target, &bad), Err(PlanError::Options(crate::planning::OptionsProblem::ValueColor)));
        assert_eq!(count(&c), (0, 0));
        // System-Projekt und unbekanntes Projekt
        let todo = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(import_types(&c, todo, &good), Err(PlanError::SystemProtected));
        assert_eq!(import_types(&c, 999, &good), Err(PlanError::NotFound));
        // Der unveränderte Import klappt danach
        import_types(&c, target, &good).unwrap();
        assert_eq!(count(&c), (3, 5));
    }

    #[test]
    fn json_reading_checks_format_and_version() {
        let doc = TemplateDoc { format: FORMAT.into(), version: VERSION, statuses: vec![], types: vec![], work_kinds: vec![] };
        assert_eq!(TemplateDoc::from_json(&doc.to_json()).unwrap(), doc);
        assert_eq!(TemplateDoc::from_json(&format!("\u{feff}{}", doc.to_json())).unwrap(), doc, "BOM wird toleriert");
        assert!(matches!(TemplateDoc::from_json("{"), Err(PlanError::Template(TemplateProblem::Json(_)))));
        assert_eq!(TemplateDoc::from_json("{\"format\":\"x\",\"version\":1}"), Err(PlanError::Template(TemplateProblem::Format)));
        assert_eq!(TemplateDoc::from_json("[]"), Err(PlanError::Template(TemplateProblem::Format)));
        assert_eq!(
            TemplateDoc::from_json(&format!("{{\"format\":\"{FORMAT}\",\"version\":{}}}", VERSION + 1)),
            Err(PlanError::Template(TemplateProblem::Version { found: VERSION + 1, supported: VERSION }))
        );
        // fehlende Listen sind leer; unbekannter Feldtyp ist ein JSON-Fehler
        let min = TemplateDoc::from_json(&format!("{{\"format\":\"{FORMAT}\",\"version\":1}}")).unwrap();
        assert!(min.types.is_empty());
        let bad_kind = format!("{{\"format\":\"{FORMAT}\",\"version\":1,\"types\":[{{\"id\":1,\"key\":\"x\",\"color\":\"#112233\",\"workflow\":false,\"fields\":[{{\"id\":1,\"name\":\"f\",\"kind\":\"hologram\"}}]}}]}}");
        assert!(matches!(TemplateDoc::from_json(&bad_kind), Err(PlanError::Template(TemplateProblem::Json(_)))));
        // Version 1 liest ein Dokument ohne die optionalen Felder
        let ok = format!("{{\"format\":\"{FORMAT}\",\"version\":1,\"types\":[{{\"id\":1,\"name\":\"X\",\"color\":\"#112233\",\"workflow\":false,\"fields\":[{{\"id\":1,\"name\":\"f\",\"kind\":\"text\"}}]}}]}}");
        let d = TemplateDoc::from_json(&ok).unwrap();
        let c = db();
        let p = project(&c, "AA", false, false);
        let rep = import_types(&c, p, &d).unwrap();
        assert_eq!((rep.created_types, rep.created_fields), (1, 1));
    }
}

#[cfg(test)]
mod lucide_tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::types::{create_type, list_types, NewType};

    #[test]
    fn lucide_icons_roundtrip_through_the_template() {
        let c = crate::db::open_memory();
        let a = create_project(&c, &NewProject::new("AA", "A", "#112233")).unwrap();
        create_type(&c, &NewType { project_id: a, key: None, name: Some("Alarm".into()), icon: Some("lucide:alarm-clock".into()), color: "#445566".into(), workflow: false }).unwrap();
        create_type(&c, &NewType { project_id: a, key: None, name: Some("Future".into()), icon: Some("lucide:not-yet-known".into()), color: "#445566".into(), workflow: false }).unwrap();
        let doc = TemplateDoc::from_json(&export_types(&c, a).unwrap().to_json()).unwrap();
        let b = create_project(&c, &NewProject::new("BB", "B", "#112233")).unwrap();
        import_types(&c, b, &doc).unwrap();
        let icons: Vec<Option<String>> = list_types(&c, b).unwrap().into_iter().map(|t| t.icon).collect();
        assert!(icons.contains(&Some("lucide:alarm-clock".into())) && icons.contains(&Some("lucide:not-yet-known".into())));
        // Ein altes Dokument ohne Icon-Feld lädt weiter (Serde-Standard).
        let mut v: serde_json::Value = serde_json::from_str(&doc.to_json()).unwrap();
        for t in v["types"].as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("icon");
        }
        let old = TemplateDoc::from_json(&v.to_string()).unwrap();
        assert!(old.types.iter().all(|t| t.icon.is_none()));
    }
}
