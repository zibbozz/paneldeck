//! Export einer Projektsicherung (siehe Moduldoku in `mod.rs`).

use super::refmap::{translate_history, IdMap, Ref};
use super::{BackupError, DocComment, DocHistory, DocLink, DocLinkType, DocProject, DocRelease, DocSprint, DocSprintTicket, DocTarget, DocTicket, DocTime, DocValue, ProjectDoc, FORMAT, VERSION};
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
    versions: HashMap<i64, i64>,
    milestones: HashMap<i64, i64>,
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
            Ref::Version => self.versions.get(&id).copied(),
            Ref::Milestone => self.milestones.get(&id).copied(),
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
        versions: HashMap::new(),
        milestones: HashMap::new(),
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

    // Versionen und Meilensteine (unabhängig von den Projektschaltern: die Daten bleiben auch bei ausgeschalteter Funktion erhalten)
    let versions = load_releases(conn, "versions", "released", "released_at", project_id, &mut map.versions)?;
    let milestones = load_releases(conn, "milestones", "reached", "reached_at", project_id, &mut map.milestones)?;

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
        let mut st = conn.prepare("SELECT c.ticket_id, c.body, c.created_at, c.updated_at, c.origin, c.origin_detail FROM comments c JOIN tickets t ON t.id = c.ticket_id WHERE t.project_id = ?1 AND c.trashed_at IS NULL ORDER BY t.number, c.id")?;
        let origin = |s: String| (s != "user").then_some(s);
        for row in st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, DocComment { body: r.get(1)?, created_at: r.get(2)?, updated_at: r.get(3)?, origin: origin(r.get(4)?), origin_detail: r.get(5)? })))? {
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
            let target = match kind {
                FieldKind::Ticket => Some(&map.tickets),
                FieldKind::Version => Some(&map.versions),
                FieldKind::Milestone => Some(&map.milestones),
                _ => None,
            };
            if let Some(numbers) = target {
                let Some(n) = v.as_i64().and_then(|id| numbers.get(&id).copied()) else { continue };
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
            archived: t.archived,
            private: t.private,
            origin: t.origin.is_system().then(|| t.origin.as_str().to_string()),
            origin_detail: t.origin_detail.clone(),
            created_at: t.created_at.clone(),
            updated_at: t.updated_at.clone(),
            values: vals,
            comments: comments.remove(&t.id).unwrap_or_default(),
            history,
            links: links.remove(&t.id).unwrap_or_default(),
            time_entries: times.remove(&t.id).unwrap_or_default(),
        });
    }

    let sprints = load_sprints(conn, project_id, &map.tickets)?;
    let maintenance = export_maintenance(conn, project_id, &map)?;

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
            due_thresholds: Some(crate::planning::projects::due_thresholds(conn, project.id)?),
        },
        structure,
        link_types: map.link_types,
        views,
        tickets: out_tickets,
        versions,
        milestones,
        sprints,
        maintenance,
    })
}

/// Wartungs-Einstellungen und eigene Regeln (#308); `None`, wenn das Projekt nichts davon hat.
fn export_maintenance(conn: &Connection, project_id: i64, map: &ExportMap) -> Result<Option<super::DocMaintenance>, BackupError> {
    use crate::planning::maintenance::{load_settings, rules::list_rules};
    let has_settings: bool = conn.query_row("SELECT COUNT(*) FROM maintenance_settings WHERE project_id = ?1", [project_id], |r| r.get::<_, i64>(0))? > 0;
    let rules = list_rules(conn, project_id)?;
    if !has_settings && rules.is_empty() {
        return Ok(None);
    }
    let s = load_settings(conn, project_id)?;
    let doc_rules = rules
        .iter()
        .map(|r| {
            let mut action = serde_json::to_value(&r.action).unwrap_or(serde_json::Value::Null);
            if let Some(s) = action.get("status").and_then(|v| v.as_i64()) {
                action["status"] = serde_json::json!(map.status.get(&s).copied().unwrap_or(0));
            }
            if let Some(f) = action.get("field").and_then(|v| v.as_i64()) {
                action["field"] = serde_json::json!(map.fields.get(&f).copied().unwrap_or(0));
            }
            super::DocRule { name: r.name.clone(), jql: r.jql.clone(), severity: r.severity.as_str().to_string(), action, enabled: r.enabled }
        })
        .collect();
    Ok(Some(super::DocMaintenance {
        wont_do: s.wont_do.and_then(|w| map.status.get(&w).copied()),
        stale_days: Some(s.stale_days),
        field_days: Some(s.field_days),
        duplicate_percent: Some(s.duplicate_percent),
        disabled: s.disabled.iter().map(|c| c.as_str().to_string()).collect(),
        rules: doc_rules,
    }))
}

/// Versionen bzw. Meilensteine des Projekts in der Reihenfolge der Konfiguration; trägt die Nummern (1, 2, …) in `numbers` ein.
fn load_releases(conn: &Connection, table: &str, done: &str, done_at: &str, project_id: i64, numbers: &mut HashMap<i64, i64>) -> Result<Vec<DocRelease>, BackupError> {
    let mut st = conn.prepare(&format!("SELECT id, name, description, due_date, {done}, {done_at} FROM {table} WHERE project_id = ?1 ORDER BY position, id"))?;
    let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, DocRelease { name: r.get(1)?, description: r.get(2)?, due_date: r.get(3)?, done: r.get::<_, i64>(4)? != 0, done_at: r.get(5)? })))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, d) = row?;
        out.push(d);
        numbers.insert(id, out.len() as i64);
    }
    Ok(out)
}

/// Sprints des Projekts (in der Reihenfolge des Anlegens) samt den Zeilen der Ticket-Zugehörigkeit.
fn load_sprints(conn: &Connection, project_id: i64, tickets: &HashMap<i64, i64>) -> Result<Vec<DocSprint>, BackupError> {
    let mut ids: Vec<i64> = Vec::new();
    let mut out: Vec<DocSprint> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id, name, goal, start_date, end_date, state, default_days, created_at, started_at, completed_at FROM sprints WHERE project_id = ?1 ORDER BY id")?;
        let rows = st.query_map([project_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                DocSprint {
                    name: r.get(1)?,
                    goal: r.get(2)?,
                    start_date: r.get(3)?,
                    end_date: r.get(4)?,
                    state: r.get(5)?,
                    default_days: r.get(6)?,
                    created_at: r.get(7)?,
                    started_at: r.get(8)?,
                    completed_at: r.get(9)?,
                    tickets: Vec::new(),
                },
            ))
        })?;
        for row in rows {
            let (id, s) = row?;
            ids.push(id);
            out.push(s);
        }
    }
    let mut st = conn.prepare(
        "SELECT st.sprint_id, st.ticket_id, st.added_at, st.removed_at, st.points_at_add FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id
         WHERE s.project_id = ?1 ORDER BY st.sprint_id, st.id",
    )?;
    let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, DocSprintTicket { ticket: 0, added_at: r.get(2)?, removed_at: r.get(3)?, points_at_add: r.get(4)? })))?;
    for row in rows {
        let (sprint, ticket, mut e) = row?;
        let (Some(i), Some(n)) = (ids.iter().position(|s| *s == sprint), tickets.get(&ticket)) else { continue };
        e.ticket = *n;
        out[i].tickets.push(e);
    }
    Ok(out)
}
