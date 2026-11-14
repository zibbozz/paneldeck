//! Export einer Projektsicherung (siehe Moduldoku in `mod.rs`).

use super::refmap::{translate_history, IdMap, Ref};
use super::{BackupError, DocComment, DocHistory, DocLink, DocLinkType, DocProject, DocTarget, DocTicket, DocTime, DocValue, ProjectDoc, FORMAT, VERSION};
use crate::planning::fields::FieldKind;
use crate::planning::link_types::get_link_type;
use crate::planning::projects::editable;
use crate::planning::summary::project_summary;
use crate::planning::template::export_types;
use crate::planning::tickets::Ticket;
use crate::planning::values::{decode, Row};
use crate::planning::views::transfer::export_views;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};

/// Zuordnung Datenbank-Id → Dokument-Nummer.
struct ExportMap<'a> {
    conn: &'a Connection,
    status: HashMap<i64, i64>,
    types: HashMap<i64, i64>,
    fields: HashMap<i64, i64>,
    field_kinds: HashMap<i64, FieldKind>,
    work_kinds: HashMap<i64, i64>,
    tickets: HashMap<i64, i64>,
    link_type_no: HashMap<i64, i64>,
    link_types: Vec<DocLinkType>,
}

impl ExportMap<'_> {
    /// Nummer des Verknüpfungstyps; legt den Eintrag beim ersten Gebrauch an. `None` = gibt es nicht mehr.
    fn link_type(&mut self, id: i64) -> Option<i64> {
        if let Some(n) = self.link_type_no.get(&id) {
            return Some(*n);
        }
        let lt = get_link_type(self.conn, id).ok()?;
        let n = self.link_types.len() as i64 + 1;
        self.link_types.push(DocLinkType { id: n, key: lt.key, name_out: lt.name_out, name_in: lt.name_in, symmetric: lt.symmetric, target_kind: lt.target_kind.as_str().to_string() });
        self.link_type_no.insert(id, n);
        Some(n)
    }
}

impl IdMap for ExportMap<'_> {
    fn map(&mut self, r: Ref, id: i64) -> i64 {
        match r {
            Ref::Status => self.status.get(&id).copied(),
            Ref::Type => self.types.get(&id).copied(),
            Ref::Field => self.fields.get(&id).copied(),
            Ref::Ticket => self.tickets.get(&id).copied(),
            Ref::WorkKind => self.work_kinds.get(&id).copied(),
            Ref::LinkType => self.link_type(id),
        }
        .unwrap_or(0)
    }

    fn field_kind(&self, source_field: i64) -> Option<FieldKind> {
        self.field_kinds.get(&source_field).copied()
    }
}

/// Exportiert das Projekt als Sicherung. System-Projekte (Todolist) lassen sich nicht exportieren (`SystemProtected`).
pub fn export_project(conn: &Connection, project_id: i64) -> Result<ProjectDoc, BackupError> {
    let project = editable(conn, project_id)?;
    let structure = export_types(conn, project_id)?;
    let views = export_views(conn, project_id).map_err(BackupError::View)?;
    let sum = project_summary(conn, project_id)?;

    let mut map = ExportMap {
        conn,
        status: HashMap::new(),
        types: HashMap::new(),
        fields: HashMap::new(),
        field_kinds: HashMap::new(),
        work_kinds: HashMap::new(),
        tickets: HashMap::new(),
        link_type_no: HashMap::new(),
        link_types: Vec::new(),
    };
    // Die Vorlagen-Nummern stammen aus dem Struktur-Dokument; die Reihenfolge der Zusammenfassung ist dieselbe.
    for (s, d) in sum.statuses.iter().zip(&structure.statuses) {
        map.status.insert(s.id, d.id);
    }
    let types: Vec<_> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
    for (t, d) in types.iter().zip(&structure.types) {
        map.types.insert(t.ticket_type.id, d.id);
        for (f, df) in t.fields.iter().zip(&d.fields) {
            map.fields.insert(f.id, df.id);
            map.field_kinds.insert(f.id, f.kind);
        }
    }
    for (i, w) in sum.work_kinds.iter().enumerate() {
        map.work_kinds.insert(w.id, i as i64 + 1);
    }
    if map.status.len() != structure.statuses.len() || map.types.len() != structure.types.len() {
        return Err(BackupError::Db("structure numbering".to_string()));
    }

    // Tickets (aufsteigend nach Nummer)
    let tickets: Vec<Ticket> = {
        let mut st = conn.prepare(&format!("SELECT {} {} WHERE t.project_id = ?1 ORDER BY t.number", crate::planning::tickets::COLS, crate::planning::tickets::FROM))?;
        let rows = st.query_map([project_id], crate::planning::tickets::from_row)?.collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for t in &tickets {
        map.tickets.insert(t.id, t.number);
    }

    // Feldwerte
    let mut values: HashMap<i64, BTreeMap<i64, (FieldKind, Vec<Row>)>> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT v.ticket_id, v.field_id, f.kind, v.text, v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
             WHERE t.project_id = ?1 ORDER BY v.ticket_id, v.field_id, v.pos",
        )?;
        let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<f64>>(4)?)))?;
        for row in rows {
            let (tid, fid, kind, text, num) = row?;
            let kind = FieldKind::parse(&kind).unwrap_or(FieldKind::Text);
            values.entry(tid).or_default().entry(fid).or_insert_with(|| (kind, Vec::new())).1.push((text, num));
        }
    }
    let mut comments: HashMap<i64, Vec<DocComment>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT c.ticket_id, c.body, c.created_at, c.updated_at FROM comments c JOIN tickets t ON t.id = c.ticket_id WHERE t.project_id = ?1 ORDER BY t.number, c.id")?;
        for row in st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, DocComment { body: r.get(1)?, created_at: r.get(2)?, updated_at: r.get(3)? })))? {
            let (tid, c) = row?;
            comments.entry(tid).or_default().push(c);
        }
    }
    let mut times: HashMap<i64, Vec<DocTime>> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT e.ticket_id, e.start, e.minutes, e.work_kind_id, e.comment, e.created_at FROM time_entries e JOIN tickets t ON t.id = e.ticket_id
             WHERE t.project_id = ?1 ORDER BY t.number, e.start, e.id",
        )?;
        let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)))?;
        for row in rows {
            let (tid, start, minutes, kind, comment, created_at) = row?;
            let work_kind = kind.and_then(|k| map.work_kinds.get(&k).copied());
            times.entry(tid).or_default().push(DocTime { start, minutes: minutes.clamp(0, u32::MAX as i64) as u32, work_kind, comment, created_at });
        }
    }
    // Verknüpfungen (ausgehend von Tickets des Projekts)
    let mut links: HashMap<i64, Vec<DocLink>> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT l.from_ticket_id, l.link_type_id, l.to_kind, l.to_ticket_id, l.to_text, l.created_at FROM links l JOIN tickets t ON t.id = l.from_ticket_id
             WHERE t.project_id = ?1 ORDER BY t.number, l.id",
        )?;
        let rows = st
            .query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        for (from, lt, kind, to_ticket, to_text, created_at) in rows {
            let Some(link_type) = map.link_type(lt) else { continue };
            let text = to_text.unwrap_or_default();
            let target = match kind.as_str() {
                "ticket" => {
                    let Some(to) = to_ticket else { continue };
                    match map.tickets.get(&to) {
                        Some(n) => DocTarget::Ticket { number: *n },
                        None => match crate::planning::tickets::get_ticket(conn, to) {
                            Ok(t) => DocTarget::ExternalTicket { key: t.key() },
                            Err(_) => continue,
                        },
                    }
                }
                "terminal" => DocTarget::Terminal { text },
                "url" => DocTarget::Url { text },
                _ => {
                    let (repo, name) = text.split_once('\n').unwrap_or(("", &text));
                    DocTarget::Branch { repo: repo.to_string(), name: name.to_string() }
                }
            };
            links.entry(from).or_default().push(DocLink { link_type, target, created_at });
        }
    }
    // Verlauf
    let mut hist_rows: HashMap<i64, Vec<(String, String, Option<String>, String, Option<String>, Option<String>, i64)>> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT h.ticket_id, h.at, h.source, h.source_detail, h.field, h.old, h.new, h.batch FROM history h JOIN tickets t ON t.id = h.ticket_id
             WHERE t.project_id = ?1 ORDER BY t.number, h.id",
        )?;
        let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))))?;
        for row in rows {
            let (tid, h) = row?;
            hist_rows.entry(tid).or_default().push(h);
        }
    }
    // Kein Feld des Projekts: Felder anderer Projekte (verschobene Tickets) sind für den Verlauf unbekannt (→ 0).

    let mut batches: HashMap<i64, i64> = HashMap::new();
    let mut out_tickets = Vec::with_capacity(tickets.len());
    for t in &tickets {
        let mut vals = Vec::new();
        for (fid, (kind, rows)) in values.remove(&t.id).unwrap_or_default() {
            let Some(no) = map.fields.get(&fid).copied() else { continue };
            let Some(mut v) = decode(kind, &rows) else { continue };
            if kind == FieldKind::Ticket {
                let Some(n) = v.as_i64().and_then(|id| map.tickets.get(&id).copied()) else { continue };
                v = serde_json::Value::from(n);
            }
            vals.push(DocValue { field: no, value: v });
        }
        vals.sort_by_key(|v| v.field);
        let mut history = Vec::new();
        for (at, source, source_detail, field, old, new, batch) in hist_rows.remove(&t.id).unwrap_or_default() {
            let (field, old, new) = translate_history(&field, old.as_deref(), new.as_deref(), &mut map);
            let next = batches.len() as i64 + 1;
            let batch = *batches.entry(batch).or_insert(next);
            history.push(DocHistory { at, source, source_detail, field, old, new, batch });
        }
        out_tickets.push(DocTicket {
            number: t.number,
            type_id: map.types.get(&t.type_id).copied().unwrap_or(0),
            title: t.title.clone(),
            status: t.status_id.and_then(|s| map.status.get(&s).copied()),
            parent: t.parent_id.and_then(|p| map.tickets.get(&p).copied()),
            rank: t.rank,
            flagged: t.flagged,
            starred: t.starred,
            created_at: t.created_at.clone(),
            updated_at: t.updated_at.clone(),
            values: vals,
            comments: comments.remove(&t.id).unwrap_or_default(),
            history,
            links: links.remove(&t.id).unwrap_or_default(),
            time_entries: times.remove(&t.id).unwrap_or_default(),
        });
    }

    Ok(ProjectDoc {
        format: FORMAT.to_string(),
        version: VERSION,
        exported_at: crate::planning::common::now(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        project: DocProject {
            key: project.key,
            name: project.name,
            description: project.description,
            color: project.color,
            work_dir: project.work_dir,
            use_sprints: project.use_sprints,
            use_versions: project.use_versions,
            tag_color_mode: project.tag_color_mode,
            next_number: project.next_number,
            created_at: project.created_at,
        },
        structure,
        link_types: map.link_types,
        views,
        tickets: out_tickets,
    })
}
