//! Verknüpfungstypen (global, nicht an ein Projekt gebunden): „blockiert / ist blockiert durch“, „ist in Beziehung mit“,
//! „ist Duplikat von / hat Duplikat“ und der System-Typ `terminal` (Ziel ist eine Terminal-Sitzung, keine Rückrichtung,
//! nie löschbar). Eigene Typen lassen sich anlegen, ändern und löschen; die drei Standardtypen sind ebenfalls löschbar.

use super::common::{self, atomic, bool_i, clean_name, same_label};
use super::integrity::{self, DeleteMode};
use super::names;
use super::{PlanError, Res};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};

/// Was am anderen Ende einer Verknüpfung steht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Ticket,
    Terminal,
    Url,
    Branch,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Ticket => "ticket",
            TargetKind::Terminal => "terminal",
            TargetKind::Url => "url",
            TargetKind::Branch => "branch",
        }
    }

    pub fn parse(s: &str) -> TargetKind {
        match s {
            "terminal" => TargetKind::Terminal,
            "url" => TargetKind::Url,
            "branch" => TargetKind::Branch,
            _ => TargetKind::Ticket,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkType {
    pub id: i64,
    pub key: Option<String>,
    pub name_out: Option<String>,
    pub name_in: Option<String>,
    pub symmetric: bool,
    /// Nur `terminal`: nie löschbar.
    pub system: bool,
    pub target_kind: TargetKind,
    pub position: i64,
}

impl LinkType {
    /// (Hinrichtung, Rückrichtung) in der Sprache; ohne Rückrichtung (Terminal, URL, Branch) `None`; bei symmetrischen
    /// Typen derselbe Text für beide Seiten.
    pub fn names(&self, lang: Lang) -> (String, Option<String>) {
        names::link_names(self.key.as_deref(), self.name_out.as_deref(), self.name_in.as_deref(), self.symmetric, self.target_kind == TargetKind::Ticket, lang)
    }
}

#[derive(Debug, Clone)]
pub struct NewLinkType {
    pub name_out: String,
    /// Rückrichtung; bei symmetrischen Typen und Nicht-Ticket-Zielen nicht nötig.
    pub name_in: Option<String>,
    pub symmetric: bool,
    pub target_kind: TargetKind,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateLinkType {
    /// Umbenennen; leer = Standardname (nur bei Standardtypen).
    pub name_out: Option<String>,
    pub name_in: Option<String>,
    pub symmetric: Option<bool>,
}

const COLS: &str = "id, key, name_out, name_in, symmetric, system, target_kind, position";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<LinkType> {
    Ok(LinkType {
        id: r.get(0)?,
        key: r.get(1)?,
        name_out: r.get(2)?,
        name_in: r.get(3)?,
        symmetric: r.get::<_, i64>(4)? != 0,
        system: r.get::<_, i64>(5)? != 0,
        target_kind: TargetKind::parse(&r.get::<_, String>(6)?),
        position: r.get(7)?,
    })
}

pub fn get_link_type(conn: &Connection, id: i64) -> Res<LinkType> {
    conn.query_row(&format!("SELECT {COLS} FROM link_types WHERE id = ?1"), [id], from_row).optional()?.ok_or(PlanError::NotFound)
}

pub fn link_type_by_key(conn: &Connection, key: &str) -> Res<LinkType> {
    conn.query_row(&format!("SELECT {COLS} FROM link_types WHERE key = ?1"), [key], from_row).optional()?.ok_or(PlanError::NotFound)
}

pub fn list_link_types(conn: &Connection) -> Res<Vec<LinkType>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM link_types ORDER BY position, id"))?;
    Ok(st.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Ist der Hinrichtungs-Name (in einer der Sprachen) schon vergeben?
fn out_name_taken(conn: &Connection, key: Option<&str>, name_out: Option<&str>, exclude: Option<i64>) -> Res<bool> {
    let mine = key.map(|k| format!("link.{k}.out"));
    for o in list_link_types(conn)? {
        if Some(o.id) == exclude {
            continue;
        }
        let theirs = o.key.as_deref().map(|k| format!("link.{k}.out"));
        if same_label(mine.as_deref(), name_out, theirs.as_deref(), o.name_out.as_deref()) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn create_link_type(conn: &Connection, n: &NewLinkType) -> Res<i64> {
    let out = clean_name(&n.name_out)?;
    let directed = n.target_kind == TargetKind::Ticket && !n.symmetric;
    if n.symmetric && n.target_kind != TargetKind::Ticket {
        return Err(PlanError::LinkTypeInvalid);
    }
    let name_in = match (&n.name_in, directed) {
        (Some(s), true) if !s.trim().is_empty() => Some(clean_name(s)?),
        (_, true) => return Err(PlanError::LinkTypeInvalid),
        _ => None,
    };
    atomic(conn, || {
        if out_name_taken(conn, None, Some(&out), None)? {
            return Err(PlanError::NameTaken);
        }
        let pos = common::next_position(conn, "link_types", "1 = 1", &[])?;
        conn.execute(
            "INSERT INTO link_types (key, name_out, name_in, symmetric, system, target_kind, position) VALUES (NULL, ?1, ?2, ?3, 0, ?4, ?5)",
            params![out, name_in, bool_i(n.symmetric), n.target_kind.as_str(), pos],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_link_type(conn: &Connection, id: i64, u: &UpdateLinkType) -> Res<()> {
    let cur = get_link_type(conn, id)?;
    let symmetric = u.symmetric.unwrap_or(cur.symmetric);
    if symmetric != cur.symmetric {
        // Die Richtung eines System-Typs oder eines Nicht-Ticket-Typs ändert sich nie; bei benutzten Typen auch nicht.
        if cur.system || cur.target_kind != TargetKind::Ticket {
            return Err(PlanError::SystemProtected);
        }
        let used = integrity::usage_count_link_type(conn, id)?;
        if used > 0 {
            return Err(PlanError::InUse { count: used });
        }
    }
    let name_out = match &u.name_out {
        Some(s) => common::rename_value(cur.key.as_deref(), s)?,
        None => cur.name_out.clone(),
    };
    let directed = cur.target_kind == TargetKind::Ticket && !symmetric;
    let name_in = if !directed {
        None
    } else {
        match &u.name_in {
            Some(s) if s.trim().is_empty() => {
                if cur.key.is_some() {
                    None
                } else {
                    return Err(PlanError::LinkTypeInvalid);
                }
            }
            Some(s) => Some(clean_name(s)?),
            // Von symmetrisch auf gerichtet ohne neuen Namen geht nur mit Standardnamen.
            None => cur.name_in.clone(),
        }
    };
    if directed && name_in.is_none() && cur.key.is_none() {
        return Err(PlanError::LinkTypeInvalid);
    }
    atomic(conn, || {
        if out_name_taken(conn, cur.key.as_deref(), name_out.as_deref(), Some(id))? {
            return Err(PlanError::NameTaken);
        }
        conn.execute("UPDATE link_types SET name_out = ?1, name_in = ?2, symmetric = ?3 WHERE id = ?4", params![name_out, name_in, bool_i(symmetric), id])?;
        Ok(())
    })
}

pub fn reorder_link_types(conn: &Connection, ids: &[i64]) -> Res<()> {
    common::reorder(conn, "link_types", "1 = 1", &[], ids)
}

/// Verknüpfungen dieses Typs (bis zum Ticket-Schritt 0).
pub fn usage(conn: &Connection, id: i64) -> Res<i64> {
    get_link_type(conn, id)?;
    integrity::usage_count_link_type(conn, id)
}

/// Löscht einen Verknüpfungstyp. `terminal` (System) ist nie löschbar. Ein benutzter Typ braucht einen Modus: `Migrate`
/// hängt die Verknüpfungen auf einen Typ mit gleichem Ziel und gleicher Richtungsart um, `Discard` löscht sie.
pub fn delete_link_type(conn: &Connection, id: i64, mode: Option<DeleteMode>) -> Res<()> {
    let cur = get_link_type(conn, id)?;
    if cur.system {
        return Err(PlanError::SystemProtected);
    }
    let count = integrity::usage_count_link_type(conn, id)?;
    integrity::require_mode(count, mode.is_some())?;
    let to = match &mode {
        Some(DeleteMode::Migrate { to }) => {
            let t = get_link_type(conn, *to).map_err(|_| PlanError::ReplacementInvalid)?;
            if t.id == id || t.target_kind != cur.target_kind || t.symmetric != cur.symmetric {
                return Err(PlanError::ReplacementInvalid);
            }
            Some(t.id)
        }
        _ => None,
    };
    atomic(conn, || {
        integrity::migrate_links(conn, id, to)?;
        conn.execute("DELETE FROM link_types WHERE id = ?1", [id])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::testutil::db;

    fn nl(out: &str, inn: Option<&str>, sym: bool, kind: TargetKind) -> NewLinkType {
        NewLinkType { name_out: out.into(), name_in: inn.map(str::to_string), symmetric: sym, target_kind: kind }
    }

    #[test]
    fn system_link_types_exist_with_translations() {
        let c = db();
        let l = list_link_types(&c).unwrap();
        assert_eq!(l.iter().map(|t| t.key.clone().unwrap()).collect::<Vec<_>>(), vec!["blocks", "relates", "duplicates", "terminal"]);
        let by = |k: &str| link_type_by_key(&c, k).unwrap();
        assert_eq!(by("blocks").names(Lang::En), ("blocks".into(), Some("is blocked by".into())));
        assert_eq!(by("blocks").names(Lang::De), ("blockiert".into(), Some("ist blockiert durch".into())));
        assert_eq!(by("relates").names(Lang::En), ("relates to".into(), Some("relates to".into())));
        assert_eq!(by("relates").names(Lang::De), ("ist in Beziehung mit".into(), Some("ist in Beziehung mit".into())));
        assert_eq!(by("duplicates").names(Lang::En), ("duplicates".into(), Some("is duplicated by".into())));
        assert_eq!(by("duplicates").names(Lang::De), ("ist Duplikat von".into(), Some("hat Duplikat".into())));
        let t = by("terminal");
        assert!(t.system && !t.symmetric && t.target_kind == TargetKind::Terminal);
        assert_eq!(t.names(Lang::En), ("Terminal".into(), None), "keine Rückrichtung");
        assert!(by("relates").symmetric);
    }

    #[test]
    fn terminal_is_never_deletable_but_standard_types_are() {
        let c = db();
        let t = link_type_by_key(&c, "terminal").unwrap();
        assert_eq!(delete_link_type(&c, t.id, None), Err(PlanError::SystemProtected));
        assert_eq!(delete_link_type(&c, t.id, Some(DeleteMode::Discard)), Err(PlanError::SystemProtected));
        assert!(link_type_by_key(&c, "terminal").is_ok());
        let b = link_type_by_key(&c, "blocks").unwrap();
        delete_link_type(&c, b.id, None).unwrap();
        assert_eq!(link_type_by_key(&c, "blocks"), Err(PlanError::NotFound));
        assert_eq!(list_link_types(&c).unwrap().len(), 3);
        assert_eq!(delete_link_type(&c, b.id, None), Err(PlanError::NotFound));
    }

    #[test]
    fn creates_custom_types() {
        let c = db();
        let id = create_link_type(&c, &nl("klont", Some("ist Klon von"), false, TargetKind::Ticket)).unwrap();
        let t = get_link_type(&c, id).unwrap();
        assert_eq!(t.names(Lang::En), ("klont".into(), Some("ist Klon von".into())));
        assert!(!t.system && t.key.is_none());
        // symmetrisch: keine Rückrichtung nötig
        let s = create_link_type(&c, &nl("hängt zusammen mit", None, true, TargetKind::Ticket)).unwrap();
        assert_eq!(get_link_type(&c, s).unwrap().names(Lang::De), ("hängt zusammen mit".into(), Some("hängt zusammen mit".into())));
        // Nicht-Ticket-Ziele: nur Hinrichtung
        let u = create_link_type(&c, &nl("siehe", None, false, TargetKind::Url)).unwrap();
        assert_eq!(get_link_type(&c, u).unwrap().names(Lang::En), ("siehe".into(), None));
        // Fehler
        assert_eq!(create_link_type(&c, &nl("x", None, false, TargetKind::Ticket)), Err(PlanError::LinkTypeInvalid), "gerichtet braucht die Rückrichtung");
        assert_eq!(create_link_type(&c, &nl("x", Some(" "), false, TargetKind::Ticket)), Err(PlanError::LinkTypeInvalid));
        assert_eq!(create_link_type(&c, &nl("x", None, true, TargetKind::Url)), Err(PlanError::LinkTypeInvalid), "URL kann nicht symmetrisch sein");
        assert_eq!(create_link_type(&c, &nl(" ", Some("y"), false, TargetKind::Ticket)), Err(PlanError::NameEmpty));
        assert_eq!(create_link_type(&c, &nl("Blocks", Some("y"), false, TargetKind::Ticket)), Err(PlanError::NameTaken), "gleich einem Standardnamen");
        assert_eq!(create_link_type(&c, &nl("blockiert", Some("y"), false, TargetKind::Ticket)), Err(PlanError::NameTaken), "auch in der anderen Sprache");
        assert_eq!(create_link_type(&c, &nl("KLONT", Some("y"), false, TargetKind::Ticket)), Err(PlanError::NameTaken));
        assert_eq!(list_link_types(&c).unwrap().len(), 7);
        // Reihenfolge
        let ids: Vec<i64> = list_link_types(&c).unwrap().iter().map(|t| t.id).rev().collect();
        reorder_link_types(&c, &ids).unwrap();
        assert_eq!(list_link_types(&c).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), ids);
        assert_eq!(reorder_link_types(&c, &ids[1..]), Err(PlanError::ReorderMismatch));
    }

    #[test]
    fn updates_names_and_resets_to_defaults() {
        let c = db();
        let b = link_type_by_key(&c, "blocks").unwrap();
        update_link_type(&c, b.id, &UpdateLinkType { name_out: Some("hält auf".into()), name_in: Some("wird aufgehalten von".into()), ..Default::default() }).unwrap();
        let t = get_link_type(&c, b.id).unwrap();
        assert_eq!(t.names(Lang::De), ("hält auf".into(), Some("wird aufgehalten von".into())));
        assert_eq!(t.names(Lang::En).0, "hält auf", "eigener Name gilt für beide Sprachen");
        assert_eq!(t.key.as_deref(), Some("blocks"), "der Schlüssel bleibt");
        update_link_type(&c, b.id, &UpdateLinkType { name_out: Some("".into()), name_in: Some("".into()), ..Default::default() }).unwrap();
        assert_eq!(get_link_type(&c, b.id).unwrap().names(Lang::En), ("blocks".into(), Some("is blocked by".into())));
        // Umbenennen auf vergebenen Namen
        assert_eq!(update_link_type(&c, b.id, &UpdateLinkType { name_out: Some("relates to".into()), ..Default::default() }), Err(PlanError::NameTaken));
        // System-Typ terminal darf umbenannt, aber nicht umgebaut werden
        let t = link_type_by_key(&c, "terminal").unwrap();
        update_link_type(&c, t.id, &UpdateLinkType { name_out: Some("Sitzung".into()), ..Default::default() }).unwrap();
        assert_eq!(get_link_type(&c, t.id).unwrap().names(Lang::En), ("Sitzung".into(), None));
        assert_eq!(update_link_type(&c, t.id, &UpdateLinkType { symmetric: Some(true), ..Default::default() }), Err(PlanError::SystemProtected));
        // eigener Typ: Richtung wechseln
        let id = create_link_type(&c, &nl("a", Some("b"), false, TargetKind::Ticket)).unwrap();
        update_link_type(&c, id, &UpdateLinkType { symmetric: Some(true), ..Default::default() }).unwrap();
        let t = get_link_type(&c, id).unwrap();
        assert!(t.symmetric && t.name_in.is_none());
        assert_eq!(update_link_type(&c, id, &UpdateLinkType { symmetric: Some(false), ..Default::default() }), Err(PlanError::LinkTypeInvalid), "gerichtet ohne Rückrichtung");
        update_link_type(&c, id, &UpdateLinkType { symmetric: Some(false), name_in: Some("c".into()), ..Default::default() }).unwrap();
        assert_eq!(get_link_type(&c, id).unwrap().names(Lang::En), ("a".into(), Some("c".into())));
        assert_eq!(update_link_type(&c, id, &UpdateLinkType { name_out: Some("".into()), ..Default::default() }), Err(PlanError::NameEmpty));
    }

    #[test]
    fn delete_with_replacement_rules() {
        let c = db();
        let a = create_link_type(&c, &nl("a", Some("a'"), false, TargetKind::Ticket)).unwrap();
        let sym = create_link_type(&c, &nl("s", None, true, TargetKind::Ticket)).unwrap();
        let url = create_link_type(&c, &nl("u", None, false, TargetKind::Url)).unwrap();
        let blocks = link_type_by_key(&c, "blocks").unwrap().id;
        assert_eq!(delete_link_type(&c, a, Some(DeleteMode::Migrate { to: a })), Err(PlanError::ReplacementInvalid));
        assert_eq!(delete_link_type(&c, a, Some(DeleteMode::Migrate { to: sym })), Err(PlanError::ReplacementInvalid), "Richtungsart muss passen");
        assert_eq!(delete_link_type(&c, a, Some(DeleteMode::Migrate { to: url })), Err(PlanError::ReplacementInvalid), "Ziel muss passen");
        assert_eq!(delete_link_type(&c, a, Some(DeleteMode::Migrate { to: 999 })), Err(PlanError::ReplacementInvalid));
        assert!(get_link_type(&c, a).is_ok());
        delete_link_type(&c, a, Some(DeleteMode::Migrate { to: blocks })).unwrap();
        delete_link_type(&c, sym, Some(DeleteMode::Discard)).unwrap();
        assert_eq!(usage(&c, url).unwrap(), 0);
        assert_eq!(usage(&c, 999), Err(PlanError::NotFound));
    }
}
