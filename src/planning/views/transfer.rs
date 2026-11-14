//! Export/Import der Boards und der gespeicherten Filter **eines Projekts** (Baustein der späteren Projekt-Sicherung).
//!
//! * [`export_views`] liefert ein [`ViewsDoc`] (JSON mit Kennung und Format-Version): die Projekt-Filter und alle Boards.
//!   Verweise auf Status, Typen und Felder stehen **nicht als Kennung**, sondern als Name ([`NameRef`]: Standard-Schlüssel und/oder
//!   Name), Felder zusätzlich mit ihrem Typ ([`FieldRef`]); die Quelle eines Boards verweist auf den Filter über seinen Namen.
//!   So lässt sich das Dokument in jedes Projekt einspielen, dessen Status/Typen/Felder gleich heißen (z. B. nach dem
//!   Typ-Vorlagen-Import). Globale Filter gehören nicht zum Projekt und werden nicht exportiert.
//! * [`import_views`] nimmt genau dieses Format und legt **immer Neues** an (Bestehendes bleibt unverändert, vergebene Namen
//!   bekommen „ (2)“ …). Verweise, die es im Zielprojekt nicht gibt, werden weggelassen und im [`ImportReport`] gemeldet; eine
//!   Spalte ohne auflösbaren Status entfällt. Atomar: bei einem Fehler bleibt das Projekt unverändert.

use super::boards::{create_board, list_boards, BoardColumn, BoardConfig, NewBoard};
use super::filters::{create_filter, get_filter, list_filters, NewFilter};
use super::{atomic, names_in, numbered_name, same_name, DocProblem, Res, ViewError};
use crate::planning::common::same_label;
use crate::planning::fields::list_fields;
use crate::planning::names;
use crate::planning::projects::editable;
use crate::planning::statuses::list_statuses;
use crate::planning::types::list_types;
use crate::state::Lang;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Kennung des Dokumentformats.
pub const FORMAT: &str = "paneldeck-project-views";
/// Aktuelle Format-Version.
pub const VERSION: u32 = 1;

/// Verweis auf einen Status oder Typ über Standard-Schlüssel und/oder Namen (wie im Projekt gespeichert).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NameRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl NameRef {
    fn label(&self, lang: Lang) -> String {
        names::display_name(self.key.as_deref(), self.name.as_deref(), lang)
    }

    fn matches(&self, key: Option<&str>, name: Option<&str>) -> bool {
        same_label(self.key.as_deref(), self.name.as_deref(), key, name)
    }
}

/// Verweis auf ein Feld: der Typ und das Feld darin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldRef {
    #[serde(rename = "type")]
    pub type_ref: NameRef,
    pub field: NameRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocFilter {
    pub name: String,
    #[serde(default)]
    pub jql: String,
}

/// Quelle eines Boards: der gespeicherte Filter mit diesem Namen (`global`: unter den globalen Filtern, sonst im Projekt).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub name: String,
    #[serde(default)]
    pub global: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocColumn {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub statuses: Vec<NameRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocBoard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub columns: Vec<DocColumn>,
    /// Typ-Filter (leer = alle Typen).
    #[serde(default)]
    pub types: Vec<NameRef>,
    #[serde(default)]
    pub source_filter: Option<SourceRef>,
    #[serde(default)]
    pub only_active_sprint: bool,
    #[serde(default = "yes")]
    pub show_key: bool,
    #[serde(default = "yes")]
    pub show_type_icon: bool,
    #[serde(default)]
    pub card_fields: Vec<FieldRef>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewsDoc {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub filters: Vec<DocFilter>,
    #[serde(default)]
    pub boards: Vec<DocBoard>,
}

impl ViewsDoc {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Liest ein Dokument und prüft Format und Version.
    pub fn from_json(text: &str) -> Res<ViewsDoc> {
        let text = text.trim_start_matches('\u{feff}');
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| ViewError::Doc(DocProblem::Json(e.to_string())))?;
        if v.get("format").and_then(serde_json::Value::as_str) != Some(FORMAT) {
            return Err(ViewError::Doc(DocProblem::Format));
        }
        let found = v.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
        if found > VERSION {
            return Err(ViewError::Doc(DocProblem::Version { found, supported: VERSION }));
        }
        serde_json::from_value(v).map_err(|e| ViewError::Doc(DocProblem::Json(e.to_string())))
    }
}

// ------------------------------------------------------------------ Export

/// Exportiert die Projekt-Filter und alle Boards des Projekts.
pub fn export_views(conn: &Connection, project_id: i64) -> Res<ViewsDoc> {
    editable(conn, project_id)?;
    let statuses: HashMap<i64, NameRef> = list_statuses(conn, project_id)?.into_iter().map(|s| (s.id, NameRef { key: s.key, name: s.name })).collect();
    let types = list_types(conn, project_id)?;
    let type_refs: HashMap<i64, NameRef> = types.iter().map(|t| (t.id, NameRef { key: t.key.clone(), name: t.name.clone() })).collect();
    let mut field_refs: HashMap<i64, FieldRef> = HashMap::new();
    for t in &types {
        for f in list_fields(conn, t.id)? {
            field_refs.insert(f.id, FieldRef { type_ref: type_refs[&t.id].clone(), field: NameRef { key: f.key, name: f.name } });
        }
    }
    let filters = list_filters(conn, Some(project_id))?.into_iter().map(|f| DocFilter { name: f.name, jql: f.jql }).collect();
    let mut boards = Vec::new();
    for b in list_boards(conn, project_id)? {
        let c = &b.config;
        let source_filter = match c.source_filter_id {
            Some(id) => get_filter(conn, id).ok().map(|f| SourceRef { name: f.name, global: f.project_id.is_none() }),
            None => None,
        };
        boards.push(DocBoard {
            name: b.name,
            description: b.description,
            columns: c.columns.iter().map(|col| DocColumn { title: col.title.clone(), statuses: col.status_ids.iter().filter_map(|id| statuses.get(id).cloned()).collect() }).collect(),
            types: c.type_filter.iter().filter_map(|id| type_refs.get(id).cloned()).collect(),
            source_filter,
            only_active_sprint: c.only_active_sprint,
            show_key: c.show_key,
            show_type_icon: c.show_type_icon,
            card_fields: c.card_fields.iter().filter_map(|id| field_refs.get(id).cloned()).collect(),
        });
    }
    Ok(ViewsDoc { format: FORMAT.to_string(), version: VERSION, filters, boards })
}

// ------------------------------------------------------------------ Import

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Status,
    Type,
    Field,
    Filter,
}

/// Ein Verweis des Dokuments, den es im Zielprojekt nicht gibt (und der deshalb fehlt).
#[derive(Debug, Clone, PartialEq)]
pub struct Unresolved {
    pub kind: RefKind,
    /// Name/Schlüssel des Fehlenden (bei Feldern „Typ / Feld“ nicht aufgelöst: nur das Feld).
    pub target: NameRef,
    /// Name des Boards im Dokument.
    pub board: String,
}

impl Unresolved {
    pub fn message(&self, lang: Lang) -> String {
        let label = self.target.label(lang);
        let (q, b) = (lang.q(&label), lang.q(&self.board));
        match (self.kind, lang) {
            (RefKind::Status, Lang::De) => format!("Board {b}: Den Status {q} gibt es nicht – weggelassen."),
            (RefKind::Status, Lang::En) => format!("Board {b}: the status {q} does not exist – left out."),
            (RefKind::Type, Lang::De) => format!("Board {b}: Den Typ {q} gibt es nicht – weggelassen."),
            (RefKind::Type, Lang::En) => format!("Board {b}: the type {q} does not exist – left out."),
            (RefKind::Field, Lang::De) => format!("Board {b}: Das Feld {q} gibt es nicht – weggelassen."),
            (RefKind::Field, Lang::En) => format!("Board {b}: the field {q} does not exist – left out."),
            (RefKind::Filter, Lang::De) => format!("Board {b}: Den Filter {q} gibt es nicht – keine Quelle gesetzt."),
            (RefKind::Filter, Lang::En) => format!("Board {b}: the filter {q} does not exist – no source set."),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub created_filters: usize,
    pub created_boards: usize,
    /// Namen (wie im Dokument), die schon vergeben waren und mit „ (2)“ o. ä. angelegt wurden.
    pub renamed: Vec<String>,
    pub unresolved: Vec<Unresolved>,
}

impl ImportReport {
    pub fn has_problems(&self) -> bool {
        !self.unresolved.is_empty()
    }
}

/// Importiert das Dokument ins Projekt (siehe Moduldoku).
pub fn import_views(conn: &Connection, project_id: i64, doc: &ViewsDoc) -> Res<ImportReport> {
    editable(conn, project_id)?;
    if doc.format != FORMAT {
        return Err(ViewError::Doc(DocProblem::Format));
    }
    if doc.version > VERSION {
        return Err(ViewError::Doc(DocProblem::Version { found: doc.version, supported: VERSION }));
    }
    atomic(conn, || {
        let mut report = ImportReport::default();
        // Filter zuerst, damit die Boards darauf verweisen können (Dokument-Name, kleingeschrieben -> neue Kennung)
        let mut taken = names_in(conn, "saved_filters", "project_id = ?1", &[project_id])?;
        let mut new_filters: HashMap<String, i64> = HashMap::new();
        for f in &doc.filters {
            let name = numbered_name(&super::clean_name(&f.name)?, &taken);
            if name != f.name.trim() {
                report.renamed.push(f.name.clone());
            }
            let id = create_filter(conn, &NewFilter { project_id: Some(project_id), name: name.clone(), jql: f.jql.clone() })?;
            taken.push(name);
            new_filters.insert(f.name.trim().to_lowercase(), id);
            report.created_filters += 1;
        }
        let statuses = list_statuses(conn, project_id)?;
        let types = list_types(conn, project_id)?;
        let mut board_taken = names_in(conn, "boards", "project_id = ?1", &[project_id])?;
        for b in &doc.boards {
            let mut miss = |kind: RefKind, r: &NameRef| report.unresolved.push(Unresolved { kind, target: r.clone(), board: b.name.clone() });
            let mut used = std::collections::HashSet::new();
            let mut columns = Vec::new();
            for col in &b.columns {
                let mut ids = Vec::new();
                for r in &col.statuses {
                    match statuses.iter().find(|s| r.matches(s.key.as_deref(), s.name.as_deref())) {
                        Some(s) => {
                            if used.insert(s.id) {
                                ids.push(s.id);
                            }
                        }
                        None => miss(RefKind::Status, r),
                    }
                }
                if !ids.is_empty() {
                    columns.push(BoardColumn { title: col.title.clone(), status_ids: ids });
                }
            }
            let mut type_filter = Vec::new();
            for r in &b.types {
                match types.iter().find(|t| r.matches(t.key.as_deref(), t.name.as_deref())) {
                    Some(t) => type_filter.push(t.id),
                    None => miss(RefKind::Type, r),
                }
            }
            let mut card_fields = Vec::new();
            for r in &b.card_fields {
                let mut found = None;
                for t in types.iter().filter(|t| r.type_ref.matches(t.key.as_deref(), t.name.as_deref())) {
                    if let Some(f) = list_fields(conn, t.id)?.into_iter().find(|f| r.field.matches(f.key.as_deref(), f.name.as_deref())) {
                        found = Some(f.id);
                        break;
                    }
                }
                match found {
                    Some(id) => card_fields.push(id),
                    None => miss(RefKind::Field, &r.field),
                }
            }
            let source_filter_id = match &b.source_filter {
                None => None,
                Some(s) => {
                    let found = if s.global {
                        list_filters(conn, None)?.into_iter().find(|f| same_name(&f.name, &s.name)).map(|f| f.id)
                    } else {
                        new_filters.get(&s.name.trim().to_lowercase()).copied().or(list_filters(conn, Some(project_id))?.into_iter().find(|f| same_name(&f.name, &s.name)).map(|f| f.id))
                    };
                    if found.is_none() {
                        miss(RefKind::Filter, &NameRef { key: None, name: Some(s.name.clone()) });
                    }
                    found
                }
            };
            let name = numbered_name(&super::clean_name(&b.name)?, &board_taken);
            if name != b.name.trim() {
                report.renamed.push(b.name.clone());
            }
            let config = BoardConfig { columns, type_filter, source_filter_id, only_active_sprint: b.only_active_sprint, show_key: b.show_key, show_type_icon: b.show_type_icon, card_fields, ..BoardConfig::default() };
            create_board(conn, &NewBoard { project_id, name: name.clone(), description: b.description.clone(), config })?;
            board_taken.push(name);
            report.created_boards += 1;
        }
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::list_fields;
    use crate::planning::statuses::{list_statuses, update_status, UpdateStatus};
    use crate::planning::views::boards::{default_board_for, get_board, update_board, UpdateBoard};
    use crate::planning::views::filters::{delete_filter, get_filter};
    use crate::planning::views::testutil::{db, project};
    use crate::planning::PlanError;
    use serde_json::json;

    fn type_id(c: &Connection, p: i64, key: &str) -> i64 {
        list_types(c, p).unwrap().into_iter().find(|t| t.key.as_deref() == Some(key)).unwrap().id
    }

    fn field_id(c: &Connection, p: i64, ty: &str, key: &str) -> i64 {
        list_fields(c, type_id(c, p, ty)).unwrap().into_iter().find(|f| f.key.as_deref() == Some(key)).unwrap().id
    }

    /// Projekt mit einem Filter und zwei Boards (eines davon mit allen Einstellungen).
    fn sample(c: &Connection, key: &str) -> (i64, i64) {
        let p = project(c, key, true);
        let s: Vec<i64> = list_statuses(c, p).unwrap().iter().map(|s| s.id).collect();
        let flt = create_filter(c, &NewFilter { project_id: Some(p), name: "Offene".into(), jql: "status != Done".into() }).unwrap();
        create_board(c, &NewBoard { project_id: p, name: "Team".into(), description: "Das Team-Board".into(), config: BoardConfig {
            columns: vec![BoardColumn { title: "Start".into(), status_ids: vec![s[0], s[1]] }, BoardColumn { title: String::new(), status_ids: vec![s[2]] }],
            type_filter: vec![type_id(c, p, "userstory"), type_id(c, p, "defect")],
            source_filter_id: Some(flt),
            only_active_sprint: true,
            show_key: false,
            show_type_icon: true,
            card_fields: vec![field_id(c, p, "userstory", "priority"), field_id(c, p, "userstory", "epic")],
            ..BoardConfig::default()
        } }).unwrap();
        create_board(c, &default_board_for(c, p, Lang::En).unwrap().into_named("Standard")).unwrap();
        (p, flt)
    }

    impl NewBoard {
        fn into_named(mut self, n: &str) -> NewBoard {
            self.name = n.into();
            self
        }
    }

    #[test]
    fn export_has_names_instead_of_ids() {
        let c = db();
        let (p, _) = sample(&c, "AA");
        let doc = export_views(&c, p).unwrap();
        assert_eq!((doc.format.as_str(), doc.version), (FORMAT, VERSION));
        assert_eq!(doc.filters, vec![DocFilter { name: "Offene".into(), jql: "status != Done".into() }]);
        assert_eq!(doc.boards.len(), 2);
        let b = &doc.boards[0];
        assert_eq!((b.name.as_str(), b.description.as_str(), b.only_active_sprint, b.show_key, b.show_type_icon), ("Team", "Das Team-Board", true, false, true));
        assert_eq!(b.columns[0].title, "Start");
        assert_eq!(b.columns[0].statuses, vec![NameRef { key: Some("todo".into()), name: None }, NameRef { key: Some("progress".into()), name: None }]);
        assert_eq!(b.types.iter().map(|t| t.key.clone().unwrap()).collect::<Vec<_>>(), vec!["userstory", "defect"]);
        assert_eq!(b.source_filter, Some(SourceRef { name: "Offene".into(), global: false }));
        assert_eq!(b.card_fields[1], FieldRef { type_ref: NameRef { key: Some("userstory".into()), name: None }, field: NameRef { key: Some("epic".into()), name: None } });
        // JSON: keine Datenbank-Kennungen, leere Namen fehlen
        let v: serde_json::Value = serde_json::from_str(&doc.to_json()).unwrap();
        assert_eq!(v["boards"][0]["columns"][0]["statuses"][0], json!({"key": "todo"}));
        assert_eq!(v["boards"][0]["card_fields"][0]["type"], json!({"key": "userstory"}));
        assert_eq!(v["boards"][0]["source_filter"], json!({"name": "Offene", "global": false}));
        assert_eq!(ViewsDoc::from_json(&doc.to_json()).unwrap(), doc);
    }

    #[test]
    fn imports_into_a_similar_project_and_round_trips() {
        let c = db();
        let (p, _) = sample(&c, "AA");
        let q = project(&c, "BB", true);
        let doc = export_views(&c, p).unwrap();
        let rep = import_views(&c, q, &doc).unwrap();
        assert_eq!((rep.created_filters, rep.created_boards), (1, 2));
        assert!(rep.unresolved.is_empty() && rep.renamed.is_empty() && !rep.has_problems());
        // der Export des Zielprojekts ist derselbe (gleiche Namen, gleiche Struktur)
        assert_eq!(export_views(&c, q).unwrap(), doc);
        // das Board verweist auf Dinge des Zielprojekts
        let b = &list_boards(&c, q).unwrap()[0];
        assert!(b.config.status_ids().iter().all(|id| list_statuses(&c, q).unwrap().iter().any(|s| s.id == *id)));
        assert_eq!(b.config.type_filter, vec![type_id(&c, q, "userstory"), type_id(&c, q, "defect")]);
        assert_eq!(b.config.card_fields, vec![field_id(&c, q, "userstory", "priority"), field_id(&c, q, "userstory", "epic")]);
        let src = get_filter(&c, b.config.source_filter_id.unwrap()).unwrap();
        assert_eq!((src.project_id, src.name.as_str()), (Some(q), "Offene"));
        // die Quelle ist der neue Filter, nicht der aus AA
        assert_eq!(list_boards(&c, p).unwrap()[0].config.source_filter_id.map(|f| f != src.id), Some(true));
    }

    #[test]
    fn import_into_the_same_project_creates_numbered_copies() {
        let c = db();
        let (p, flt) = sample(&c, "AA");
        let doc = export_views(&c, p).unwrap();
        let rep = import_views(&c, p, &doc).unwrap();
        assert_eq!(rep.renamed, vec!["Offene".to_string(), "Team".to_string(), "Standard".to_string()]);
        let boards = list_boards(&c, p).unwrap();
        assert_eq!(boards.iter().map(|b| b.name.clone()).collect::<Vec<_>>(), vec!["Team", "Standard", "Team (2)", "Standard (2)"]);
        let names: Vec<String> = list_filters(&c, Some(p)).unwrap().iter().map(|f| f.name.clone()).collect();
        assert_eq!(names, vec!["Offene", "Offene (2)"]);
        // das neue Board nutzt den neuen Filter; das alte bleibt unverändert
        assert_eq!(boards[0].config.source_filter_id, Some(flt));
        assert_ne!(boards[2].config.source_filter_id, Some(flt));
        assert_eq!(boards[2].config.columns, boards[0].config.columns);
        // noch einmal: „(3)“
        import_views(&c, p, &doc).unwrap();
        assert!(list_boards(&c, p).unwrap().iter().any(|b| b.name == "Team (3)"));
    }

    #[test]
    fn unresolved_references_are_dropped_and_reported() {
        let c = db();
        let (p, _) = sample(&c, "AA");
        let doc = export_views(&c, p).unwrap();
        // Zielprojekt: „In Arbeit“ umbenannt, kein Defect-Typ, keine Epic-Feld
        let q = project(&c, "BB", true);
        let sq = list_statuses(&c, q).unwrap();
        update_status(&c, sq[1].id, &UpdateStatus { name: Some("Review".into()), ..Default::default() }).unwrap();
        c.execute("DELETE FROM ticket_types WHERE id = ?1", [type_id(&c, q, "defect")]).unwrap();
        c.execute("DELETE FROM fields WHERE id = ?1", [field_id(&c, q, "userstory", "epic")]).unwrap();
        let rep = import_views(&c, q, &doc).unwrap();
        let kinds: Vec<(RefKind, &str)> = rep.unresolved.iter().filter(|u| u.board == "Team").map(|u| (u.kind, u.target.key.as_deref().unwrap_or(""))).collect();
        assert!(kinds.contains(&(RefKind::Status, "progress")));
        assert!(kinds.contains(&(RefKind::Type, "defect")));
        assert!(kinds.contains(&(RefKind::Field, "epic")));
        assert!(rep.has_problems());
        let b = &list_boards(&c, q).unwrap()[0];
        assert_eq!(b.config.columns[0].status_ids, vec![sq[0].id], "die Spalte behält den auflösbaren Status");
        assert_eq!(b.config.type_filter, vec![type_id(&c, q, "userstory")]);
        // das Standard-Board hatte „progress“ als eigene Spalte: sie entfällt
        let std = &list_boards(&c, q).unwrap()[1];
        assert_eq!(std.config.columns.len(), 2);
        // Meldungen in beiden Sprachen
        for u in &rep.unresolved {
            let (en, de) = (u.message(Lang::En), u.message(Lang::De));
            assert_ne!(en, de);
            assert!(en.contains(&u.board) && de.contains(&u.board));
        }
    }

    #[test]
    fn matching_works_by_renamed_names_too() {
        let c = db();
        let (p, _) = sample(&c, "AA");
        // In AA „To Do“ umbenennen: im Dokument steht jetzt der Name, im Ziel (gleich umbenannt) passt er
        let s = list_statuses(&c, p).unwrap();
        update_status(&c, s[0].id, &UpdateStatus { name: Some("Backlog".into()), ..Default::default() }).unwrap();
        let doc = export_views(&c, p).unwrap();
        assert_eq!(doc.boards[0].columns[0].statuses[0], NameRef { key: Some("todo".into()), name: Some("Backlog".into()) });
        let q = project(&c, "BB", true);
        let sq = list_statuses(&c, q).unwrap();
        update_status(&c, sq[0].id, &UpdateStatus { name: Some("Backlog".into()), ..Default::default() }).unwrap();
        let rep = import_views(&c, q, &doc).unwrap();
        assert!(rep.unresolved.is_empty(), "{:?}", rep.unresolved);
        assert!(list_boards(&c, q).unwrap()[0].config.columns[0].status_ids.contains(&sq[0].id));
    }

    #[test]
    fn global_filters_are_referenced_by_name() {
        let c = db();
        let p = project(&c, "AA", true);
        let g = create_filter(&c, &NewFilter { project_id: None, name: "Global".into(), jql: "favorite".into() }).unwrap();
        create_board(&c, &NewBoard { project_id: p, name: "B".into(), description: String::new(), config: BoardConfig { source_filter_id: Some(g), ..Default::default() } }).unwrap();
        let doc = export_views(&c, p).unwrap();
        assert!(doc.filters.is_empty(), "globale Filter werden nicht mitexportiert");
        assert_eq!(doc.boards[0].source_filter, Some(SourceRef { name: "Global".into(), global: true }));
        let q = project(&c, "BB", true);
        let rep = import_views(&c, q, &doc).unwrap();
        assert!(rep.unresolved.is_empty());
        assert_eq!(list_boards(&c, q).unwrap()[0].config.source_filter_id, Some(g));
        // gibt es den globalen Filter nicht mehr, entfällt die Quelle mit Meldung
        delete_filter(&c, g).unwrap();
        let r = project(&c, "CC", true);
        let rep = import_views(&c, r, &doc).unwrap();
        assert_eq!(rep.unresolved.len(), 1);
        assert_eq!(rep.unresolved[0].kind, RefKind::Filter);
        assert_eq!(list_boards(&c, r).unwrap()[0].config.source_filter_id, None);
    }

    #[test]
    fn deleted_source_filter_is_exported_without_source() {
        let c = db();
        let (p, flt) = sample(&c, "AA");
        delete_filter(&c, flt).unwrap();
        let doc = export_views(&c, p).unwrap();
        assert!(doc.filters.is_empty());
        assert_eq!(doc.boards[0].source_filter, None);
    }

    #[test]
    fn import_is_atomic_and_never_touches_existing_data() {
        let c = db();
        let (p, _) = sample(&c, "AA");
        let before_boards = list_boards(&c, p).unwrap();
        let before_filters = list_filters(&c, Some(p)).unwrap();
        let mut doc = export_views(&c, p).unwrap();
        // zweites Board mit zu langem Namen: der Import bricht ab, auch das erste neue Board darf nicht bleiben
        doc.boards.push(DocBoard { name: "x".repeat(81), description: String::new(), columns: vec![], types: vec![], source_filter: None, only_active_sprint: false, show_key: true, show_type_icon: true, card_fields: vec![] });
        let q = project(&c, "BB", true);
        assert_eq!(import_views(&c, q, &doc), Err(PlanError::NameTooLong { max: 80 }.into()));
        assert!(list_boards(&c, q).unwrap().is_empty());
        assert!(list_filters(&c, Some(q)).unwrap().is_empty());
        // Import in AA ändert Bestehendes nicht
        doc.boards.pop();
        import_views(&c, p, &doc).unwrap();
        assert_eq!(&list_boards(&c, p).unwrap()[..2], &before_boards[..]);
        assert_eq!(&list_filters(&c, Some(p)).unwrap()[..1], &before_filters[..]);
    }

    #[test]
    fn checks_format_version_and_target() {
        let c = db();
        let (p, _) = sample(&c, "AA");
        let doc = export_views(&c, p).unwrap();
        let text = doc.to_json();
        assert!(ViewsDoc::from_json(&format!("\u{feff}{text}")).is_ok());
        assert_eq!(ViewsDoc::from_json("{}"), Err(ViewError::Doc(DocProblem::Format)));
        assert!(matches!(ViewsDoc::from_json("[1"), Err(ViewError::Doc(DocProblem::Json(_)))));
        let newer = text.replacen("\"version\": 1", "\"version\": 7", 1);
        assert_eq!(ViewsDoc::from_json(&newer), Err(ViewError::Doc(DocProblem::Version { found: 7, supported: VERSION })));
        let mut bad = doc.clone();
        bad.format = "x".into();
        assert_eq!(import_views(&c, p, &bad), Err(ViewError::Doc(DocProblem::Format)));
        bad.format = FORMAT.into();
        bad.version = 5;
        assert!(matches!(import_views(&c, p, &bad), Err(ViewError::Doc(DocProblem::Version { .. }))));
        // Ziel
        assert_eq!(import_views(&c, 9999, &doc), Err(ViewError::not_found()));
        let sys = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(import_views(&c, sys, &doc), Err(PlanError::SystemProtected.into()));
        assert_eq!(export_views(&c, sys), Err(PlanError::SystemProtected.into()));
        assert_eq!(export_views(&c, 9999), Err(ViewError::not_found()));
    }

    #[test]
    fn empty_project_exports_an_empty_document() {
        let c = db();
        let p = project(&c, "AA", false);
        let doc = export_views(&c, p).unwrap();
        assert!(doc.filters.is_empty() && doc.boards.is_empty());
        let q = project(&c, "BB", false);
        let rep = import_views(&c, q, &doc).unwrap();
        assert_eq!(rep, ImportReport::default());
    }

    #[test]
    fn imported_boards_pass_the_normal_validation() {
        let c = db();
        let (p, _) = sample(&c, "AA");
        let q = project(&c, "BB", true);
        import_views(&c, q, &export_views(&c, p).unwrap()).unwrap();
        for b in list_boards(&c, q).unwrap() {
            // die gespeicherte Konfiguration besteht die Prüfung erneut
            update_board(&c, b.id, &UpdateBoard { config: Some(b.config.clone()), ..Default::default() }).unwrap();
            assert_eq!(get_board(&c, b.id).unwrap().config, b.config);
        }
    }
}
