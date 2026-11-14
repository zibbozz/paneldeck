//! Die Zeilen der Ergebnislisten und Karten: ein Ticket mit allem, was Liste und Board zeigen (Typ, Status, Priorität, Story Points,
//! Eltern). Typ-, Status- und Wertdaten werden gesammelt je Seite nachgeladen (keine Abfrage pro Zeile).

use super::error::SRes;
use super::Ctx;
use crate::planning::fields::parse_options;
use crate::planning::names::display_name;
use crate::planning::statuses::StatusCategory;
use crate::planning::tickets::POINTS_FIELD_KEY;
use rusqlite::{params_from_iter, Connection};
use std::collections::HashMap;

/// Schlüssel des Dropdown-Felds der Priorität (Standardfeld).
pub const PRIORITY_FIELD_KEY: &str = "priority";

/// Spalten der Ticket-Abfrage (Reihenfolge wie in [`TicketRow::from_core`]).
pub(super) const CORE_COLS: &str = "t.id, t.project_id, p.key, t.number, t.title, t.type_id, t.status_id, t.parent_id, t.flagged, t.starred, t.rank, t.updated_at";

/// Ein Ticket in einer Liste oder auf einer Karte.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketRow {
    pub id: i64,
    pub project_id: i64,
    pub project_key: String,
    pub number: i64,
    /// Anzeigeschlüssel, z. B. `PAN-12`.
    pub key: String,
    pub title: String,
    pub type_id: i64,
    pub type_name: String,
    pub type_color: String,
    pub type_icon: Option<String>,
    pub status_id: Option<i64>,
    pub status_name: Option<String>,
    pub status_color: Option<String>,
    pub status_category: Option<StatusCategory>,
    /// Anzeigename der Priorität (leer, wenn der Typ keine hat oder sie nicht gesetzt ist).
    pub priority: Option<String>,
    pub priority_color: Option<String>,
    pub story_points: Option<f64>,
    pub parent_id: Option<i64>,
    pub parent_key: Option<String>,
    /// Markiert („Flag“, Impediment).
    pub flagged: bool,
    /// Favorit (Stern).
    pub starred: bool,
    pub rank: i64,
    pub updated_at: String,
}

impl TicketRow {
    fn from_core(r: &rusqlite::Row) -> rusqlite::Result<TicketRow> {
        let project_key: String = r.get(2)?;
        let number: i64 = r.get(3)?;
        Ok(TicketRow {
            id: r.get(0)?,
            project_id: r.get(1)?,
            key: format!("{project_key}-{number}"),
            project_key,
            number,
            title: r.get(4)?,
            type_id: r.get(5)?,
            type_name: String::new(),
            type_color: String::new(),
            type_icon: None,
            status_id: r.get(6)?,
            status_name: None,
            status_color: None,
            status_category: None,
            priority: None,
            priority_color: None,
            story_points: None,
            parent_id: r.get(7)?,
            parent_key: None,
            flagged: r.get::<_, i64>(8)? != 0,
            starred: r.get::<_, i64>(9)? != 0,
            rank: r.get(10)?,
            updated_at: r.get(11)?,
        })
    }
}

/// Führt `sql` für Teilmengen der Kennungen aus (`{IN}` wird durch die Platzhalter ersetzt).
fn by_ids<T>(conn: &Connection, sql: &str, ids: &[i64], mut f: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>) -> SRes<Vec<T>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(", ");
        let mut st = conn.prepare(&sql.replace("{IN}", &ph))?;
        let rows = st.query_map(params_from_iter(chunk.iter()), &mut f)?;
        for r in rows {
            out.push(r?);
        }
    }
    Ok(out)
}

fn distinct(v: impl Iterator<Item = i64>) -> Vec<i64> {
    let mut v: Vec<i64> = v.collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Führt die Ticket-Abfrage aus (`sql` wählt [`CORE_COLS`]) und ergänzt die Zeilen.
pub(super) fn load_rows(conn: &Connection, ctx: &Ctx, sql: &str, params: &[rusqlite::types::Value]) -> SRes<Vec<TicketRow>> {
    let mut st = conn.prepare(sql)?;
    // nicht verwendete Nummern am Ende abschneiden (siehe `compile`: manche Bausteine verwerfen ihre Parameter)
    let n = st.parameter_count().min(params.len());
    let mut rows: Vec<TicketRow> = st.query_map(params_from_iter(params[..n].iter()), TicketRow::from_core)?.collect::<Result<_, _>>()?;
    drop(st);
    enrich(conn, ctx, &mut rows)?;
    Ok(rows)
}

/// Ergänzt Typ, Status, Priorität, Story Points und Eltern-Schlüssel.
pub(super) fn enrich(conn: &Connection, ctx: &Ctx, rows: &mut [TicketRow]) -> SRes<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let lang = ctx.lang;
    // Typen
    let type_ids = distinct(rows.iter().map(|r| r.type_id));
    let types: HashMap<i64, (String, String, Option<String>)> = by_ids(conn, "SELECT id, key, name, color, icon FROM ticket_types WHERE id IN ({IN})", &type_ids, |r| {
        let (key, name): (Option<String>, Option<String>) = (r.get(1)?, r.get(2)?);
        Ok((r.get::<_, i64>(0)?, (display_name(key.as_deref(), name.as_deref(), lang), r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?)))
    })?
    .into_iter()
    .collect();
    // Status
    let status_ids = distinct(rows.iter().filter_map(|r| r.status_id));
    let statuses: HashMap<i64, (String, String, StatusCategory)> = by_ids(conn, "SELECT id, key, name, color, category FROM statuses WHERE id IN ({IN})", &status_ids, |r| {
        let (key, name): (Option<String>, Option<String>) = (r.get(1)?, r.get(2)?);
        Ok((r.get::<_, i64>(0)?, (display_name(key.as_deref(), name.as_deref(), lang), r.get::<_, String>(3)?, StatusCategory::parse(&r.get::<_, String>(4)?))))
    })?
    .into_iter()
    .collect();
    // Eltern
    let parent_ids = distinct(rows.iter().filter_map(|r| r.parent_id));
    let parents: HashMap<i64, String> = by_ids(conn, "SELECT t.id, p.key, t.number FROM tickets t JOIN projects p ON p.id = t.project_id WHERE t.id IN ({IN})", &parent_ids, |r| {
        Ok((r.get::<_, i64>(0)?, format!("{}-{}", r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
    })?
    .into_iter()
    .collect();
    // Priorität und Story Points
    let ticket_ids = distinct(rows.iter().map(|r| r.id));
    struct Val {
        ticket: i64,
        key: String,
        num: Option<f64>,
        options: String,
    }
    let vals: Vec<Val> = by_ids(
        conn,
        &format!("SELECT v.ticket_id, f.key, v.num, f.options FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE v.ticket_id IN ({{IN}}) AND f.key IN ('{PRIORITY_FIELD_KEY}', '{POINTS_FIELD_KEY}')"),
        &ticket_ids,
        |r| Ok(Val { ticket: r.get(0)?, key: r.get(1)?, num: r.get(2)?, options: r.get(3)? }),
    )?;
    let mut prio_cache: HashMap<String, Vec<(i64, String, Option<String>)>> = HashMap::new();
    let mut by_ticket: HashMap<i64, Vec<&Val>> = HashMap::new();
    for v in &vals {
        by_ticket.entry(v.ticket).or_default().push(v);
    }
    for row in rows.iter_mut() {
        if let Some((name, color, icon)) = types.get(&row.type_id) {
            row.type_name = name.clone();
            row.type_color = color.clone();
            row.type_icon = icon.clone();
        }
        if let Some((name, color, cat)) = row.status_id.and_then(|s| statuses.get(&s)) {
            row.status_name = Some(name.clone());
            row.status_color = Some(color.clone());
            row.status_category = Some(*cat);
        }
        row.parent_key = row.parent_id.and_then(|p| parents.get(&p).cloned());
        for v in by_ticket.get(&row.id).map(Vec::as_slice).unwrap_or(&[]) {
            if v.key == POINTS_FIELD_KEY {
                row.story_points = v.num;
            } else if let Some(id) = v.num {
                let list = prio_cache.entry(v.options.clone()).or_insert_with(|| {
                    parse_options(&v.options).values.iter().map(|o| (o.id, o.display(lang), o.color.clone())).collect()
                });
                if let Some((_, name, color)) = list.iter().find(|(oid, _, _)| *oid as f64 == id) {
                    row.priority = Some(name.clone());
                    row.priority_color = color.clone();
                }
            }
        }
    }
    Ok(())
}
