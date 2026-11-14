//! Typen (z. B. Epic, Userstory) eines Projekts: Name, Icon, Farbe, Workflow ja/nein und die Hierarchie-Regeln
//! (welche Typen unter welchen hängen dürfen). Workflow-Status und Übergänge: `workflow.rs`, Felder: `fields.rs`.

use super::common::{self, atomic, bool_i, clean_name, ensure_name_free, normalize_color, rename_value, validate_ident};
use super::integrity::{self, TypeDeleteMode, TypeMigration};
use super::names;
use super::projects::editable;
use super::{icons, PlanError, Res};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct TicketType {
    pub id: i64,
    pub project_id: i64,
    pub key: Option<String>,
    pub name: Option<String>,
    /// Name aus dem festen Icon-Satz (`icons::ICONS`), optional.
    pub icon: Option<String>,
    pub color: String,
    /// `false`: der Typ hat kein Statusfeld und erscheint nicht im Board.
    pub workflow: bool,
    pub system: bool,
    pub position: i64,
}

impl TicketType {
    pub fn display(&self, lang: Lang) -> String {
        names::display_name(self.key.as_deref(), self.name.as_deref(), lang)
    }
}

#[derive(Debug, Clone)]
pub struct NewType {
    pub project_id: i64,
    pub key: Option<String>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub color: String,
    pub workflow: bool,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateType {
    /// Umbenennen; leer = Standardname (nur mit Schlüssel).
    pub name: Option<String>,
    /// `Some(None)` entfernt das Icon.
    pub icon: Option<Option<String>>,
    pub color: Option<String>,
}

const COLS: &str = "id, project_id, key, name, icon, color, workflow, system, position";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<TicketType> {
    Ok(TicketType {
        id: r.get(0)?,
        project_id: r.get(1)?,
        key: r.get(2)?,
        name: r.get(3)?,
        icon: r.get(4)?,
        color: r.get(5)?,
        workflow: r.get::<_, i64>(6)? != 0,
        system: r.get::<_, i64>(7)? != 0,
        position: r.get(8)?,
    })
}

pub fn get_type(conn: &Connection, id: i64) -> Res<TicketType> {
    conn.query_row(&format!("SELECT {COLS} FROM ticket_types WHERE id = ?1"), [id], from_row).optional()?.ok_or(PlanError::NotFound)
}

pub fn list_types(conn: &Connection, project_id: i64) -> Res<Vec<TicketType>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM ticket_types WHERE project_id = ?1 ORDER BY position, id"))?;
    Ok(st.query_map([project_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Der Typ, wenn er änderbar ist (kein System-Typ, Projekt kein System-Projekt).
pub fn editable_type(conn: &Connection, id: i64) -> Res<TicketType> {
    let t = get_type(conn, id)?;
    if t.system {
        return Err(PlanError::SystemProtected);
    }
    editable(conn, t.project_id)?;
    Ok(t)
}

fn check_icon(icon: &Option<String>) -> Res<()> {
    match icon {
        Some(i) if !icons::is_valid(i) => Err(PlanError::IconInvalid),
        _ => Ok(()),
    }
}

pub fn create_type(conn: &Connection, n: &NewType) -> Res<i64> {
    editable(conn, n.project_id)?;
    let color = normalize_color(&n.color)?;
    check_icon(&n.icon)?;
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
        ensure_name_free(conn, "ticket_types", "project_id = ?1", &[n.project_id], n.key.as_deref(), name.as_deref(), None)?;
        let pos = common::next_position(conn, "ticket_types", "project_id = ?1", &[n.project_id])?;
        conn.execute(
            "INSERT INTO ticket_types (project_id, key, name, icon, color, workflow, system, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
            params![n.project_id, n.key, name, n.icon, color, bool_i(n.workflow), pos],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_type(conn: &Connection, id: i64, u: &UpdateType) -> Res<()> {
    let cur = editable_type(conn, id)?;
    let name = match &u.name {
        Some(s) => rename_value(cur.key.as_deref(), s)?,
        None => cur.name.clone(),
    };
    let color = match &u.color {
        Some(c) => normalize_color(c)?,
        None => cur.color.clone(),
    };
    let icon = match &u.icon {
        Some(i) => {
            check_icon(i)?;
            i.clone()
        }
        None => cur.icon.clone(),
    };
    atomic(conn, || {
        ensure_name_free(conn, "ticket_types", "project_id = ?1", &[cur.project_id], cur.key.as_deref(), name.as_deref(), Some(id))?;
        conn.execute("UPDATE ticket_types SET name = ?1, icon = ?2, color = ?3 WHERE id = ?4", params![name, icon, color, id])?;
        Ok(())
    })
}

pub fn reorder_types(conn: &Connection, project_id: i64, ids: &[i64]) -> Res<()> {
    editable(conn, project_id)?;
    common::reorder(conn, "ticket_types", "project_id = ?1", &[project_id], ids)
}

/// Tickets dieses Typs (bis zum Ticket-Schritt 0).
pub fn usage(conn: &Connection, id: i64) -> Res<i64> {
    get_type(conn, id)?;
    integrity::usage_count_type(conn, id)
}

/// Löscht einen Typ samt Feldern, Workflow-Zuordnung, Übergängen und Hierarchie-Regeln. Wird er benutzt, ist ein Modus nötig:
/// `Migrate` hängt die Tickets auf den Ersatz-Typ um und überträgt die Feldwerte gemäß [`TypeMigration`], `Discard`
/// löscht die Tickets des Typs. Verweise anderer Ticket-Felder auf diesen Typ als Ziel werden entfernt.
pub fn delete_type(conn: &Connection, id: i64, mode: Option<TypeDeleteMode>) -> Res<()> {
    let cur = editable_type(conn, id)?;
    let count = integrity::usage_count_type(conn, id)?;
    integrity::require_mode(count, mode.is_some())?;
    if let Some(TypeDeleteMode::Migrate(m)) = &mode {
        check_migration(conn, &cur, m)?;
    }
    atomic(conn, || {
        integrity::migrate_tickets_type(conn, id, match &mode {
            Some(TypeDeleteMode::Migrate(m)) => Some(m),
            _ => None,
        })?;
        conn.execute("DELETE FROM ticket_types WHERE id = ?1", [id])?;
        scrub_target_type(conn, cur.project_id, id)?;
        Ok(())
    })
}

/// Entfernt `type_id` aus den Ziel-Typen aller Ticket-Felder des Projekts.
pub(crate) fn scrub_target_type(conn: &Connection, project_id: i64, type_id: i64) -> Res<()> {
    let rows: Vec<(i64, String)> = {
        let mut st = conn.prepare("SELECT f.id, f.options FROM fields f JOIN ticket_types t ON t.id = f.type_id WHERE t.project_id = ?1 AND f.kind = 'ticket'")?;
        st.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    for (fid, json) in rows {
        let mut o = super::fields::parse_options(&json);
        if o.target_types.contains(&type_id) {
            o.target_types.retain(|t| *t != type_id);
            conn.execute("UPDATE fields SET options = ?1 WHERE id = ?2", params![super::fields::options_json(&o), fid])?;
        }
    }
    Ok(())
}

fn check_migration(conn: &Connection, from: &TicketType, m: &TypeMigration) -> Res<()> {
    let to = get_type(conn, m.to_type).map_err(|_| PlanError::ReplacementInvalid)?;
    if to.id == from.id || to.project_id != from.project_id || to.system {
        return Err(PlanError::ReplacementInvalid);
    }
    let from_fields = super::fields::list_fields(conn, from.id)?;
    let to_fields = super::fields::list_fields(conn, to.id)?;
    let mut seen = HashSet::new();
    for e in &m.fields {
        let f = from_fields.iter().find(|f| f.id == e.from_field).ok_or(PlanError::ReplacementInvalid)?;
        if !seen.insert(e.from_field) {
            return Err(PlanError::ReplacementInvalid);
        }
        if let Some(t) = e.to_field {
            let t = to_fields.iter().find(|x| x.id == t).ok_or(PlanError::ReplacementInvalid)?;
            if t.kind != f.kind {
                return Err(PlanError::ReplacementInvalid);
            }
        }
    }
    // Zwei Felder dürfen nicht im selben Zielfeld landen.
    let targets: Vec<i64> = m.fields.iter().filter_map(|e| e.to_field).collect();
    if targets.iter().collect::<HashSet<_>>().len() != targets.len() {
        return Err(PlanError::ReplacementInvalid);
    }
    Ok(())
}

/// Vorschlag für die Feld-Zuordnung beim Typ-Wechsel: gleiche Felder (Schlüssel, sonst gleicher Name) mit gleichem Feldtyp
/// werden zugeordnet, alle anderen `None` (Wert verwerfen).
pub fn suggest_field_map(conn: &Connection, from_type: i64, to_type: i64) -> Res<Vec<integrity::FieldMapEntry>> {
    let from = super::fields::list_fields(conn, from_type)?;
    let to = super::fields::list_fields(conn, to_type)?;
    let mut used: HashSet<i64> = HashSet::new();
    let mut out = Vec::new();
    for f in &from {
        let hit = to.iter().find(|t| !used.contains(&t.id) && t.kind == f.kind && common::same_label(f.key.as_deref(), f.name.as_deref(), t.key.as_deref(), t.name.as_deref()));
        if let Some(t) = hit {
            used.insert(t.id);
        }
        out.push(integrity::FieldMapEntry { from_field: f.id, to_field: hit.map(|t| t.id) });
    }
    Ok(out)
}

// ------------------------------------------------------------------ Hierarchie

/// Typen, unter denen dieser Typ hängen darf (aufsteigend nach Id).
pub fn parents_of(conn: &Connection, type_id: i64) -> Res<Vec<i64>> {
    let mut st = conn.prepare("SELECT parent_type_id FROM type_parents WHERE type_id = ?1 ORDER BY parent_type_id")?;
    Ok(st.query_map([type_id], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
}

/// Typen, die unter diesem Typ hängen dürfen.
pub fn children_of(conn: &Connection, type_id: i64) -> Res<Vec<i64>> {
    let mut st = conn.prepare("SELECT type_id FROM type_parents WHERE parent_type_id = ?1 ORDER BY type_id")?;
    Ok(st.query_map([type_id], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
}

/// Darf ein Ticket vom Typ `child` unter einem Ticket vom Typ `parent` hängen?
pub fn may_be_child(conn: &Connection, child: i64, parent: i64) -> Res<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM type_parents WHERE type_id = ?1 AND parent_type_id = ?2", [child, parent], |r| r.get(0))?;
    Ok(n > 0)
}

/// Setzt die erlaubten Eltern-Typen des Typs (ersetzt die bisherigen). Eltern müssen zum selben Projekt gehören, ein Typ
/// kann nicht unter sich selbst hängen und die Regeln dürfen keinen Kreis ergeben. Wird eine Regel entfernt, nach der
/// schon Tickets hängen, liefert die Funktion `InUse`.
pub fn set_parents(conn: &Connection, type_id: i64, parents: &[i64]) -> Res<()> {
    let t = editable_type(conn, type_id)?;
    let mut want: Vec<i64> = parents.to_vec();
    want.sort_unstable();
    want.dedup();
    for p in &want {
        let pt = get_type(conn, *p).map_err(|_| PlanError::HierarchyForeign)?;
        if pt.project_id != t.project_id || pt.system {
            return Err(PlanError::HierarchyForeign);
        }
        if *p == type_id {
            return Err(PlanError::HierarchyCycle);
        }
    }
    // Kreise: Wäre einer der neuen Eltern selbst (indirekt) ein Kind dieses Typs?
    for p in &want {
        if descendants(conn, type_id)?.contains(p) {
            return Err(PlanError::HierarchyCycle);
        }
    }
    let have = parents_of(conn, type_id)?;
    let mut used = 0;
    for r in have.iter().filter(|h| !want.contains(h)) {
        used += integrity::usage_count_parent_rule(conn, type_id, *r)?;
    }
    if used > 0 {
        return Err(PlanError::InUse { count: used });
    }
    atomic(conn, || {
        conn.execute("DELETE FROM type_parents WHERE type_id = ?1", [type_id])?;
        for p in &want {
            conn.execute("INSERT INTO type_parents (type_id, parent_type_id) VALUES (?1, ?2)", [type_id, *p])?;
        }
        Ok(())
    })
}

/// Alle Typen, die (auch indirekt) unter `type_id` hängen dürfen.
fn descendants(conn: &Connection, type_id: i64) -> Res<HashSet<i64>> {
    let mut seen = HashSet::new();
    let mut todo = vec![type_id];
    while let Some(t) = todo.pop() {
        for c in children_of(conn, t)? {
            if seen.insert(c) {
                todo.push(c);
            }
        }
    }
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::testutil::db;

    fn proj(c: &Connection, key: &str) -> i64 {
        create_project(c, &NewProject::new(key, "P", "#112233")).unwrap()
    }

    fn ty(project_id: i64, name: &str) -> NewType {
        NewType { project_id, key: None, name: Some(name.into()), icon: None, color: "#445566".into(), workflow: true }
    }

    #[test]
    fn creates_updates_and_orders_types() {
        let c = db();
        let p = proj(&c, "AA");
        let a = create_type(&c, &NewType { icon: Some("bug".into()), ..ty(p, "Bug") }).unwrap();
        let b = create_type(&c, &ty(p, "Task")).unwrap();
        let k = create_type(&c, &NewType { project_id: p, key: Some("epic".into()), name: None, icon: None, color: "#b77fe0".into(), workflow: false }).unwrap();
        let l = list_types(&c, p).unwrap();
        assert_eq!(l.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a, b, k]);
        assert_eq!(l[0].icon.as_deref(), Some("bug"));
        assert!(l[0].workflow && !l[2].workflow && !l[0].system);
        assert_eq!((l[2].display(Lang::De), l[2].display(Lang::En)), ("Epic".into(), "Epic".into()));
        update_type(&c, a, &UpdateType { name: Some("Fehler".into()), color: Some("#ff0000".into()), icon: Some(Some("fire".into())) }).unwrap();
        let t = get_type(&c, a).unwrap();
        assert_eq!((t.display(Lang::En).as_str(), t.color.as_str(), t.icon.as_deref()), ("Fehler", "#FF0000", Some("fire")));
        update_type(&c, a, &UpdateType { icon: Some(None), ..Default::default() }).unwrap();
        assert_eq!(get_type(&c, a).unwrap().icon, None);
        assert_eq!(update_type(&c, a, &UpdateType { icon: Some(Some("nope".into())), ..Default::default() }), Err(PlanError::IconInvalid));
        assert_eq!(create_type(&c, &NewType { icon: Some("nope".into()), ..ty(p, "X") }), Err(PlanError::IconInvalid));
        reorder_types(&c, p, &[k, a, b]).unwrap();
        assert_eq!(list_types(&c, p).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![k, a, b]);
        assert_eq!(reorder_types(&c, p, &[k, a]), Err(PlanError::ReorderMismatch));
    }

    #[test]
    fn validates_types_and_protects_the_system_type() {
        let c = db();
        let p = proj(&c, "AA");
        create_type(&c, &ty(p, "Bug")).unwrap();
        assert_eq!(create_type(&c, &ty(p, "bug")), Err(PlanError::NameTaken));
        assert_eq!(create_type(&c, &ty(p, "")), Err(PlanError::NameEmpty));
        assert_eq!(create_type(&c, &NewType { color: "x".into(), ..ty(p, "Y") }), Err(PlanError::ColorInvalid));
        assert_eq!(create_type(&c, &ty(999, "Z")), Err(PlanError::NotFound));
        let todo = crate::planning::projects::project_by_key(&c, "TODO").unwrap();
        assert_eq!(create_type(&c, &ty(todo.id, "Z")), Err(PlanError::SystemProtected));
        let sys = list_types(&c, todo.id).unwrap().remove(0);
        assert!(sys.system && !sys.workflow);
        assert_eq!(update_type(&c, sys.id, &UpdateType { name: Some("x".into()), ..Default::default() }), Err(PlanError::SystemProtected));
        assert_eq!(delete_type(&c, sys.id, None), Err(PlanError::SystemProtected));
        assert_eq!(set_parents(&c, sys.id, &[]), Err(PlanError::SystemProtected));
        assert_eq!(sys.display(Lang::De), "Todolist");
    }

    #[test]
    fn hierarchy_rules() {
        let c = db();
        let p = proj(&c, "AA");
        let q = proj(&c, "BB");
        let epic = create_type(&c, &ty(p, "Epic")).unwrap();
        let story = create_type(&c, &ty(p, "Story")).unwrap();
        let task = create_type(&c, &ty(p, "Task")).unwrap();
        let foreign = create_type(&c, &ty(q, "Foreign")).unwrap();
        set_parents(&c, story, &[epic]).unwrap();
        set_parents(&c, task, &[story, epic, epic]).unwrap();
        assert_eq!(parents_of(&c, task).unwrap(), vec![epic, story]);
        assert_eq!(children_of(&c, epic).unwrap(), vec![story, task]);
        assert!(may_be_child(&c, task, story).unwrap());
        assert!(!may_be_child(&c, epic, story).unwrap());
        // Fehler
        assert_eq!(set_parents(&c, story, &[story]), Err(PlanError::HierarchyCycle), "nicht unter sich selbst");
        assert_eq!(set_parents(&c, epic, &[task]), Err(PlanError::HierarchyCycle), "indirekter Kreis epic > story > task > epic");
        assert_eq!(set_parents(&c, epic, &[story]), Err(PlanError::HierarchyCycle));
        assert_eq!(set_parents(&c, story, &[foreign]), Err(PlanError::HierarchyForeign));
        assert_eq!(set_parents(&c, story, &[9999]), Err(PlanError::HierarchyForeign));
        assert_eq!(parents_of(&c, story).unwrap(), vec![epic], "fehlgeschlagene Änderung ändert nichts");
        // ersetzen und leeren
        set_parents(&c, task, &[story]).unwrap();
        assert_eq!(parents_of(&c, task).unwrap(), vec![story]);
        set_parents(&c, task, &[]).unwrap();
        assert!(parents_of(&c, task).unwrap().is_empty());
        // Typ löschen entfernt seine Regeln (auch als Eltern anderer)
        delete_type(&c, story, None).unwrap();
        assert!(children_of(&c, epic).unwrap().is_empty());
    }

    #[test]
    fn deleting_a_type_cleans_up_and_validates_the_migration() {
        use crate::planning::fields::*;
        let c = db();
        let p = proj(&c, "AA");
        let a = create_type(&c, &ty(p, "A")).unwrap();
        let b = create_type(&c, &ty(p, "B")).unwrap();
        let f = |t: i64, name: &str, kind: FieldKind| create_field(&c, &NewField::new(t, name, kind)).unwrap();
        let fa_text = f(a, "Text", FieldKind::Text);
        let fa_num = f(a, "Zahl", FieldKind::Number);
        let fb_text = f(b, "Text", FieldKind::Text);
        let fb_rich = f(b, "Rich", FieldKind::Rich);
        // ein Ticket-Feld in B zeigt auf A
        let mut link = NewField::new(b, "Verweis", FieldKind::Ticket);
        link.options.target_types = vec![a, b];
        let link = create_field(&c, &link).unwrap();
        // Zuordnung: Vorschlag verbindet gleiche Namen/Typen
        let map = suggest_field_map(&c, a, b).unwrap();
        assert_eq!(map, vec![integrity::FieldMapEntry { from_field: fa_text, to_field: Some(fb_text) }, integrity::FieldMapEntry { from_field: fa_num, to_field: None }]);
        // ungültige Migrationen
        let bad = |m: TypeMigration| delete_type(&c, a, Some(TypeDeleteMode::Migrate(m)));
        assert_eq!(bad(TypeMigration { to_type: a, fields: vec![] }), Err(PlanError::ReplacementInvalid));
        assert_eq!(bad(TypeMigration { to_type: 999, fields: vec![] }), Err(PlanError::ReplacementInvalid));
        assert_eq!(bad(TypeMigration { to_type: b, fields: vec![integrity::FieldMapEntry { from_field: fa_text, to_field: Some(fb_rich) }] }), Err(PlanError::ReplacementInvalid), "Feldtypen müssen passen");
        assert_eq!(bad(TypeMigration { to_type: b, fields: vec![integrity::FieldMapEntry { from_field: fb_text, to_field: None }] }), Err(PlanError::ReplacementInvalid), "Quellfeld gehört nicht zum Typ");
        assert_eq!(
            bad(TypeMigration { to_type: b, fields: vec![integrity::FieldMapEntry { from_field: fa_text, to_field: Some(fb_text) }, integrity::FieldMapEntry { from_field: fa_text, to_field: None }] }),
            Err(PlanError::ReplacementInvalid),
            "ein Quellfeld nur einmal"
        );
        assert!(get_type(&c, a).is_ok());
        // gültig
        delete_type(&c, a, Some(TypeDeleteMode::Migrate(TypeMigration { to_type: b, fields: map }))).unwrap();
        assert_eq!(get_type(&c, a), Err(PlanError::NotFound));
        assert!(list_fields(&c, a).unwrap().is_empty(), "Felder kaskadiert");
        assert_eq!(get_field(&c, link).unwrap().options.target_types, vec![b], "A ist kein Ziel-Typ mehr");
        assert!(get_field(&c, fa_text).is_err());
    }
}
