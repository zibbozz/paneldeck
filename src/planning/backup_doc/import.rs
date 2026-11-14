//! Import einer Projektsicherung als **neues** Projekt (siehe Moduldoku in `mod.rs`).

use super::refmap::{translate_history, IdMap, Ref};
use super::{valid_timestamp, BackupError, DocLinkType, DocRelease, DocTarget, DocTicket, ProjectDoc, Problem, RefKind, Warning, Where, FORMAT, VERSION};
use crate::planning::comments::MAX_COMMENT_CHARS;
use crate::planning::common::{self, normalize_key, validate_key};
use crate::planning::fields::{list_fields, Field, FieldKind};
use crate::planning::history::next_batch;
use crate::planning::link_types::{create_link_type, get_link_type, list_link_types, LinkType, NewLinkType, TargetKind};
use crate::planning::links::{clean_target, Target};
use crate::planning::names;
use crate::planning::projects::{create_project, project_by_key, NewProject};
use crate::planning::sprints::{ensure_name_free, fmt_date, parse_date, SprintError, MAX_DESCRIPTION_CHARS, MAX_GOAL_CHARS, MAX_SPRINT_DAYS};
use crate::planning::statuses::list_statuses;
use crate::planning::template::import_types;
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::{check_parent, clean_title};
use crate::planning::time_entries::MAX_TIME_COMMENT_CHARS;
use crate::planning::types::list_types;
use crate::planning::values::{normalize_value, write_value, RefCtx};
use crate::planning::views::transfer::import_views;
use crate::planning::work_kinds::list_work_kinds;
use crate::planning::workflow::workflow_statuses;
use crate::planning::PlanError;
use crate::state::Lang;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

/// Was ein Import angelegt hat.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub statuses: usize,
    pub types: usize,
    pub fields: usize,
    pub work_kinds: usize,
    pub quick_actions: usize,
    pub boards: usize,
    pub filters: usize,
    pub tickets: usize,
    pub sprints: usize,
    pub versions: usize,
    pub milestones: usize,
    pub comments: usize,
    pub links: usize,
    pub time_entries: usize,
    pub history: usize,
    /// Was bewusst nicht (oder anders) übernommen wurde.
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    /// Das neue Projekt.
    pub project_id: i64,
    /// Das Kürzel des neuen Projekts.
    pub key: String,
    /// Die Tickets wurden von 1 an neu nummeriert (neues Kürzel).
    pub renumbered: bool,
    pub report: ImportReport,
}

/// Prüft ein (neues) Projektkürzel für den Import: bringt es in Großbuchstaben, prüft die Form und dass es frei ist.
/// Dient auch der Live-Prüfung im Eingabefeld (`KeyTaken` = vergeben, `Plan(KeyInvalid)` = falsche Form).
pub fn check_new_key(conn: &Connection, key: &str) -> Result<String, BackupError> {
    let k = normalize_key(key);
    validate_key(&k)?;
    match project_by_key(conn, &k) {
        Ok(_) => Err(BackupError::KeyTaken(k)),
        Err(PlanError::NotFound) => Ok(k),
        Err(e) => Err(e.into()),
    }
}

/// Importiert die Sicherung als neues Projekt. `new_key`: das Kürzel des neuen Projekts; `None` = das des Dokuments (ist es
/// vergeben, kommt `KeyTaken` und der Aufrufer fragt ein neues Kürzel ab). Ein anderes Kürzel als das des Dokuments
/// nummeriert die Tickets von 1 an neu. Atomar: bei einem Fehler bleibt die Datenbank unverändert.
pub fn import_project(conn: &Connection, doc: &ProjectDoc, new_key: Option<String>) -> Result<ImportResult, BackupError> {
    if doc.format != FORMAT {
        return Err(BackupError::Format);
    }
    if doc.version > VERSION {
        return Err(BackupError::Version { found: doc.version, supported: VERSION });
    }
    let doc_key = normalize_key(&doc.project.key);
    let (key, renumber) = match new_key {
        Some(k) => {
            let k = check_new_key(conn, &k)?;
            let r = k != doc_key;
            (k, r)
        }
        None => (check_new_key(conn, &doc_key).map_err(|e| match e {
            BackupError::Plan(p) => BackupError::AtPlan(Where::Project, p),
            o => o,
        })?, false),
    };
    conn.execute_batch("SAVEPOINT backup_import")?;
    match run(conn, doc, &key, renumber) {
        Ok(v) => {
            conn.execute_batch("RELEASE backup_import")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO backup_import; RELEASE backup_import");
            Err(e)
        }
    }
}

/// Zuordnung Dokument-Nummer → Datenbank-Id.
#[derive(Default)]
struct ImportMap {
    status: HashMap<i64, i64>,
    types: HashMap<i64, i64>,
    fields: HashMap<i64, i64>,
    field_kinds: HashMap<i64, FieldKind>,
    work_kinds: HashMap<i64, i64>,
    tickets: HashMap<i64, i64>,
    link_types: HashMap<i64, i64>,
    versions: HashMap<i64, i64>,
    milestones: HashMap<i64, i64>,
}

impl IdMap for ImportMap {
    fn map(&mut self, r: Ref, id: i64) -> i64 {
        match r {
            Ref::Status => self.status.get(&id),
            Ref::Type => self.types.get(&id),
            Ref::Field => self.fields.get(&id),
            Ref::Ticket => self.tickets.get(&id),
            Ref::WorkKind => self.work_kinds.get(&id),
            Ref::LinkType => self.link_types.get(&id),
            Ref::Version => self.versions.get(&id),
            Ref::Milestone => self.milestones.get(&id),
        }
        .copied()
        .unwrap_or(0)
    }

    fn field_kind(&self, source_field: i64) -> Option<FieldKind> {
        self.field_kinds.get(&source_field).copied()
    }
}

fn unknown(w: Where, k: RefKind, n: i64) -> BackupError {
    BackupError::At(w, Problem::UnknownRef(k, n))
}

fn check_ts(w: impl Fn() -> Where, s: &str) -> Result<(), BackupError> {
    if valid_timestamp(s) { Ok(()) } else { Err(BackupError::At(w(), Problem::Timestamp(s.to_string()))) }
}

fn run(conn: &Connection, doc: &ProjectDoc, key: &str, renumber: bool) -> Result<ImportResult, BackupError> {
    let mut report = ImportReport::default();
    let p = &doc.project;
    let pid = create_project(
        conn,
        &NewProject {
            key: key.to_string(),
            name: p.name.clone(),
            color: p.color.clone(),
            description: p.description.clone(),
            work_dir: p.work_dir.clone(),
            use_sprints: p.use_sprints,
            use_versions: p.use_versions,
            tag_color_mode: p.tag_color_mode,
            with_defaults: false,
        },
    )
    .map_err(|e| BackupError::AtPlan(Where::Project, e))?;
    if !valid_timestamp(&p.created_at) {
        return Err(BackupError::At(Where::Project, Problem::Timestamp(p.created_at.clone())));
    }

    // 1. Struktur: Status, Typen, Hierarchie, Felder, Workflows, Quick-Actions, Arten der Arbeit
    let rep = import_types(conn, pid, &doc.structure).map_err(|e| BackupError::AtPlan(Where::Structure, e))?;
    if rep.has_conflicts() {
        return Err(BackupError::At(Where::Structure, Problem::StructureNames));
    }
    report.statuses = rep.created_statuses;
    report.types = rep.created_types;
    report.fields = rep.created_fields;
    report.work_kinds = rep.created_work_kinds;
    report.quick_actions = rep.created_quick_actions;
    let mut map = ImportMap::default();
    let mut field_info: HashMap<i64, Field> = HashMap::new();
    {
        let st = list_statuses(conn, pid)?;
        let ty = list_types(conn, pid)?;
        let wk = list_work_kinds(conn, pid)?;
        let ds = &doc.structure;
        if st.len() != ds.statuses.len() || ty.len() != ds.types.len() || wk.len() != ds.work_kinds.len() {
            return Err(BackupError::At(Where::Structure, Problem::StructureMismatch));
        }
        for (s, d) in st.iter().zip(&ds.statuses) {
            map.status.insert(d.id, s.id);
        }
        for (i, w) in wk.iter().enumerate() {
            map.work_kinds.insert(i as i64 + 1, w.id);
        }
        for (t, d) in ty.iter().zip(&ds.types) {
            map.types.insert(d.id, t.id);
            let fs = list_fields(conn, t.id)?;
            if fs.len() != d.fields.len() {
                return Err(BackupError::At(Where::Structure, Problem::StructureMismatch));
            }
            for (f, df) in fs.into_iter().zip(&d.fields) {
                map.fields.insert(df.id, f.id);
                map.field_kinds.insert(df.id, f.kind);
                field_info.insert(df.id, f);
            }
        }
    }

    // 2. Verknüpfungstypen (vorhandene gleichen Namens werden benutzt, fehlende angelegt)
    let lt_real = resolve_link_types(conn, &doc.link_types, &mut report.warnings)?;
    map.link_types = lt_real.iter().map(|(k, v)| (*k, v.id)).collect();

    // 3. Boards und gespeicherte Filter
    let v = import_views(conn, pid, &doc.views).map_err(|e| BackupError::AtView(Where::Views, e))?;
    report.boards = v.created_boards;
    report.filters = v.created_filters;
    report.warnings.extend(v.unresolved.into_iter().map(Warning::View));

    // 3b. Versionen, Meilensteine und Sprints (vor den Tickets: Feldwerte und Zugehörigkeit verweisen darauf)
    report.versions = import_releases(conn, pid, "versions", "released", "released_at", &doc.versions, &mut map.versions, Where::Version)?;
    report.milestones = import_releases(conn, pid, "milestones", "reached", "reached_at", &doc.milestones, &mut map.milestones, Where::Milestone)?;
    let sprint_ids = import_sprints(conn, pid, doc)?;
    report.sprints = sprint_ids.len();

    // 4. Tickets anlegen (ohne Eltern: die brauchen alle Tickets)
    let mut order: Vec<&DocTicket> = doc.tickets.iter().collect();
    order.sort_by_key(|t| t.number);
    let mut seen = HashSet::new();
    let mut wf_cache: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut real_type: HashMap<i64, i64> = HashMap::new();
    let mut new_number: HashMap<i64, i64> = HashMap::new();
    for (i, t) in order.iter().enumerate() {
        let w = || Where::Ticket(t.number);
        if t.number <= 0 || !seen.insert(t.number) {
            return Err(BackupError::At(w(), Problem::TicketNumber(t.number)));
        }
        let type_id = *map.types.get(&t.type_id).ok_or_else(|| unknown(w(), RefKind::Type, t.type_id))?;
        let title = clean_title(&t.title).map_err(|e| BackupError::AtTicket(w(), e))?;
        let status = match t.status {
            None => None,
            Some(s) => {
                let id = *map.status.get(&s).ok_or_else(|| unknown(w(), RefKind::Status, s))?;
                if !wf_cache.contains_key(&type_id) {
                    wf_cache.insert(type_id, workflow_statuses(conn, type_id)?);
                }
                if !wf_cache[&type_id].contains(&id) {
                    return Err(BackupError::At(w(), Problem::StatusNotAllowed));
                }
                Some(id)
            }
        };
        check_ts(w, &t.created_at)?;
        check_ts(w, &t.updated_at)?;
        let number = if renumber { i as i64 + 1 } else { t.number };
        conn.prepare_cached(
            "INSERT INTO tickets (project_id, number, type_id, title, status_id, parent_id, rank, flagged, starred, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10)",
        )?
        .execute(params![pid, number, type_id, title, status, t.rank, t.flagged as i64, t.starred as i64, t.created_at, t.updated_at])?;
        let id = conn.last_insert_rowid();
        map.tickets.insert(t.number, id);
        real_type.insert(t.number, type_id);
        new_number.insert(t.number, number);
        report.tickets += 1;
    }
    let max_new = new_number.values().copied().max().unwrap_or(0);
    let next_number = if renumber { max_new + 1 } else { p.next_number.max(max_new + 1) };
    conn.execute("UPDATE projects SET next_number = ?1, created_at = ?2 WHERE id = ?3", params![next_number, p.created_at, pid])?;

    // 5. Eltern (mit den Regeln der Hierarchie; Kreise werden erkannt)
    for t in &order {
        let Some(parent) = t.parent else { continue };
        let w = || Where::Ticket(t.number);
        let parent_id = *map.tickets.get(&parent).ok_or_else(|| unknown(w(), RefKind::Ticket, parent))?;
        let id = map.tickets[&t.number];
        check_parent(conn, pid, real_type[&t.number], Some(id), parent_id).map_err(|e| BackupError::AtTicket(w(), e))?;
        conn.execute("UPDATE tickets SET parent_id = ?1 WHERE id = ?2", [parent_id, id])?;
    }

    // 6. Feldwerte (über die Prüfung der Datenschicht)
    for t in &order {
        let id = map.tickets[&t.number];
        for dv in &t.values {
            let w = || Where::Value(t.number, dv.field);
            let field = field_info.get(&dv.field).ok_or_else(|| unknown(w(), RefKind::Field, dv.field))?;
            if field.type_id != real_type[&t.number] {
                return Err(BackupError::At(w(), Problem::FieldNotInType));
            }
            let release = matches!(field.kind, FieldKind::Version | FieldKind::Milestone);
            if release && doc.version < 2 {
                // Version-1-Dokumente tragen Datenbank-Ids ohne die zugehörigen Objekte.
                count_version_value(&mut report.warnings);
                continue;
            }
            let mut value = dv.value.clone();
            if release || field.kind == FieldKind::Ticket {
                let n = value.as_i64().ok_or_else(|| BackupError::AtTicket(w(), TicketError::ValueInvalid(crate::planning::ticket_error::FieldLabel::of(field))))?;
                let (numbers, what) = match field.kind {
                    FieldKind::Version => (&map.versions, RefKind::Version),
                    FieldKind::Milestone => (&map.milestones, RefKind::Milestone),
                    _ => (&map.tickets, RefKind::Ticket),
                };
                let target = *numbers.get(&n).ok_or_else(|| unknown(w(), what, n))?;
                value = serde_json::Value::from(target);
            }
            let ctx = RefCtx { project_id: pid, ticket_id: Some(id) };
            match normalize_value(conn, &ctx, field, &value).map_err(|e| BackupError::AtTicket(w(), e))? {
                Some(v) => write_value(conn, id, field, Some(&v)).map_err(|e| BackupError::AtTicket(w(), e))?,
                None => {}
            }
        }
    }

    // 6b. Zugehörigkeit der Tickets zu den Sprints
    import_sprint_tickets(conn, doc, &sprint_ids, &map.tickets)?;

    // 7. Kommentare
    for t in &order {
        let id = map.tickets[&t.number];
        for (i, c) in t.comments.iter().enumerate() {
            let w = || Where::Comment(t.number, i);
            let body = c.body.trim();
            if body.is_empty() {
                return Err(BackupError::AtTicket(w(), TicketError::CommentEmpty));
            }
            if body.chars().count() > MAX_COMMENT_CHARS {
                return Err(BackupError::AtTicket(w(), TicketError::CommentTooLong { max: MAX_COMMENT_CHARS }));
            }
            check_ts(w, &c.created_at)?;
            check_ts(w, &c.updated_at)?;
            conn.prepare_cached("INSERT INTO comments (ticket_id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)")?.execute(params![id, body, c.created_at, c.updated_at])?;
            report.comments += 1;
        }
    }

    // 8. Verknüpfungen
    let mut link_seen: HashSet<(i64, i64, &'static str, Option<i64>, Option<String>)> = HashSet::new();
    let (mut external, mut terminal) = (0usize, 0usize);
    for t in &order {
        let from = map.tickets[&t.number];
        for (i, l) in t.links.iter().enumerate() {
            let w = || Where::Link(t.number, i);
            let lt = lt_real.get(&l.link_type).ok_or_else(|| unknown(w(), RefKind::LinkType, l.link_type))?;
            let target = match &l.target {
                DocTarget::ExternalTicket { .. } => {
                    external += 1;
                    continue;
                }
                DocTarget::Ticket { number } => Target::Ticket(*map.tickets.get(number).ok_or_else(|| unknown(w(), RefKind::Ticket, *number))?),
                DocTarget::Terminal { text } => Target::Terminal(text.clone()),
                DocTarget::Url { text } => Target::Url(text.clone()),
                DocTarget::Branch { repo, name } => Target::Branch { repo: repo.clone(), name: name.clone() },
            };
            if target.kind() != lt.target_kind {
                return Err(BackupError::AtTicket(w(), TicketError::LinkTargetMismatch));
            }
            let target = clean_target(&target).map_err(|e| BackupError::AtTicket(w(), e))?;
            check_ts(w, &l.created_at)?;
            let (to_ticket, to_text) = match &target {
                Target::Ticket(to) => {
                    if *to == from {
                        return Err(BackupError::AtTicket(w(), TicketError::LinkSelf));
                    }
                    (Some(*to), None)
                }
                Target::Terminal(s) | Target::Url(s) => (None, Some(s.clone())),
                Target::Branch { repo, name } => (None, Some(format!("{repo}\n{name}"))),
            };
            let kind = target.kind().as_str();
            let dup = !link_seen.insert((lt.id, from, kind, to_ticket, to_text.clone()))
                || (lt.symmetric && to_ticket.is_some_and(|to| link_seen.contains(&(lt.id, to, kind, Some(from), None))));
            if dup {
                return Err(BackupError::At(w(), Problem::LinkDuplicate));
            }
            if matches!(target, Target::Terminal(_)) {
                terminal += 1;
            }
            conn.prepare_cached("INSERT INTO links (link_type_id, from_ticket_id, to_kind, to_ticket_id, to_text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
                .execute(params![lt.id, from, kind, to_ticket, to_text, l.created_at])?;
            report.links += 1;
        }
    }
    if external > 0 {
        report.warnings.push(Warning::ExternalLinks(external));
    }
    if terminal > 0 {
        report.warnings.push(Warning::TerminalLinks(terminal));
    }

    // 9. Zeiteinträge
    for t in &order {
        let id = map.tickets[&t.number];
        for (i, e) in t.time_entries.iter().enumerate() {
            let w = || Where::Time(t.number, i);
            if e.minutes == 0 {
                return Err(BackupError::At(w(), Problem::Minutes));
            }
            check_ts(w, &e.start)?;
            check_ts(w, &e.created_at)?;
            if e.comment.chars().count() > MAX_TIME_COMMENT_CHARS {
                return Err(BackupError::AtTicket(w(), TicketError::CommentTooLong { max: MAX_TIME_COMMENT_CHARS }));
            }
            let kind = match e.work_kind {
                Some(n) => Some(*map.work_kinds.get(&n).ok_or_else(|| unknown(w(), RefKind::WorkKind, n))?),
                None => None,
            };
            conn.prepare_cached("INSERT INTO time_entries (ticket_id, start, minutes, work_kind_id, comment, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
                .execute(params![id, e.start, e.minutes, kind, e.comment, e.created_at])?;
            report.time_entries += 1;
        }
    }

    // 10. Verlauf (Kennungen übersetzt, Aktions-Nummern neu vergeben)
    let mut batches: HashMap<i64, i64> = HashMap::new();
    for t in &order {
        let id = map.tickets[&t.number];
        for (i, h) in t.history.iter().enumerate() {
            let w = || Where::History(t.number, i);
            check_ts(w, &h.at)?;
            if !matches!(h.source.as_str(), "ui" | "quick" | "agent" | "import") {
                return Err(BackupError::At(w(), Problem::Source(h.source.clone())));
            }
            let (field, old, new) = translate_history(&h.field, h.old.as_deref(), h.new.as_deref(), &mut map);
            let batch = match batches.get(&h.batch) {
                Some(b) => *b,
                None => {
                    let b = next_batch(conn).map_err(|e| BackupError::AtTicket(w(), e))?;
                    batches.insert(h.batch, b);
                    b
                }
            };
            conn.prepare_cached("INSERT INTO history (ticket_id, at, source, source_detail, field, old, new, batch) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")?
                .execute(params![id, h.at, h.source, h.source_detail, field, old, new, batch])?;
            report.history += 1;
        }
    }

    if renumber {
        report.warnings.insert(0, Warning::Renumbered { from_key: normalize_key(&p.key), to_key: key.to_string() });
    }
    Ok(ImportResult { project_id: pid, key: key.to_string(), renumbered: renumber, report })
}

/// Versionen bzw. Meilensteine anlegen (Name eindeutig, Texte und Termine geprüft). Trägt die Nummern (1, 2, …) → Ids ein.
fn import_releases(
    conn: &Connection,
    pid: i64,
    table: &str,
    done_col: &str,
    done_at_col: &str,
    items: &[DocRelease],
    numbers: &mut HashMap<i64, i64>,
    place: fn(i64) -> Where,
) -> Result<usize, BackupError> {
    for (i, d) in items.iter().enumerate() {
        let n = i as i64 + 1;
        let name = common::clean_name(&d.name).map_err(|e| BackupError::AtPlan(place(n), e))?;
        ensure_name_free(conn, table, pid, &name, None).map_err(|e| BackupError::AtSprint(place(n), e))?;
        let description = d.description.trim();
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(BackupError::AtSprint(place(n), SprintError::TextTooLong { max: MAX_DESCRIPTION_CHARS }));
        }
        let due = match &d.due_date {
            Some(s) => Some(fmt_date(parse_date(s).map_err(|e| BackupError::AtSprint(place(n), e))?)),
            None => None,
        };
        if let Some(t) = &d.done_at {
            check_ts(|| place(n), t)?;
        }
        conn.execute(
            &format!("INSERT INTO {table} (project_id, name, description, due_date, {done_col}, {done_at_col}, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
            params![pid, name, description, due, d.done as i64, d.done_at, i as i64],
        )?;
        numbers.insert(n, conn.last_insert_rowid());
    }
    Ok(items.len())
}

/// Sprints anlegen (ohne Zugehörigkeit); liefert die Ids in der Reihenfolge des Dokuments.
fn import_sprints(conn: &Connection, pid: i64, doc: &ProjectDoc) -> Result<Vec<i64>, BackupError> {
    let mut ids = Vec::with_capacity(doc.sprints.len());
    let mut active = false;
    for (i, s) in doc.sprints.iter().enumerate() {
        let n = i as i64 + 1;
        let w = || Where::Sprint(n);
        let at = |e: SprintError| BackupError::AtSprint(w(), e);
        let name = common::clean_name(&s.name).map_err(|e| BackupError::AtPlan(w(), e))?;
        ensure_name_free(conn, "sprints", pid, &name, None).map_err(at)?;
        let goal = s.goal.trim();
        if goal.chars().count() > MAX_GOAL_CHARS {
            return Err(at(SprintError::TextTooLong { max: MAX_GOAL_CHARS }));
        }
        if !(1..=MAX_SPRINT_DAYS).contains(&s.default_days) {
            return Err(at(SprintError::DaysInvalid { max: MAX_SPRINT_DAYS }));
        }
        let start = s.start_date.as_deref().map(parse_date).transpose().map_err(at)?;
        let end = s.end_date.as_deref().map(parse_date).transpose().map_err(at)?;
        if end.is_some() && start.is_none() {
            return Err(at(SprintError::DatesIncomplete));
        }
        if let (Some(a), Some(b)) = (start, end) {
            if b < a {
                return Err(at(SprintError::DatesOrder));
            }
        }
        let state = match s.state.as_str() {
            "planned" | "active" | "completed" => s.state.as_str(),
            other => return Err(BackupError::At(w(), Problem::SprintState(other.to_string()))),
        };
        if state != "planned" && (start.is_none() || end.is_none()) {
            return Err(at(SprintError::DatesIncomplete));
        }
        if state == "active" {
            if active {
                return Err(BackupError::At(w(), Problem::SecondActiveSprint));
            }
            active = true;
        }
        check_ts(w, &s.created_at)?;
        for t in [&s.started_at, &s.completed_at].into_iter().flatten() {
            check_ts(w, t)?;
        }
        conn.execute(
            "INSERT INTO sprints (project_id, name, goal, start_date, end_date, state, default_days, created_at, started_at, completed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![pid, name, goal, start.map(fmt_date), end.map(fmt_date), state, s.default_days, s.created_at, s.started_at, s.completed_at],
        )?;
        ids.push(conn.last_insert_rowid());
    }
    Ok(ids)
}

/// Zeilen der Sprint-Zugehörigkeit: ein Ticket steckt höchstens in einem offenen (geplanten/aktiven) Sprint.
fn import_sprint_tickets(conn: &Connection, doc: &ProjectDoc, sprint_ids: &[i64], tickets: &HashMap<i64, i64>) -> Result<(), BackupError> {
    let mut open_in: HashMap<i64, i64> = HashMap::new();
    let mut open_rows: HashSet<(i64, i64)> = HashSet::new();
    for (si, s) in doc.sprints.iter().enumerate() {
        let n = si as i64 + 1;
        for (j, e) in s.tickets.iter().enumerate() {
            let w = || Where::SprintEntry(n, j);
            let ticket = *tickets.get(&e.ticket).ok_or_else(|| unknown(w(), RefKind::Ticket, e.ticket))?;
            check_ts(w, &e.added_at)?;
            if let Some(r) = &e.removed_at {
                check_ts(w, r)?;
                if *r < e.added_at {
                    return Err(BackupError::At(w(), Problem::RemovedBeforeAdded));
                }
            }
            if e.points_at_add.is_some_and(|p| !p.is_finite() || p < 0.0) {
                return Err(BackupError::At(w(), Problem::Points));
            }
            if e.removed_at.is_none() {
                if !open_rows.insert((sprint_ids[si], ticket)) {
                    return Err(BackupError::At(w(), Problem::SprintTicketTwice(e.ticket)));
                }
                if s.state != "completed" && open_in.insert(ticket, n).is_some_and(|other| other != n) {
                    return Err(BackupError::At(w(), Problem::TicketInTwoSprints(e.ticket)));
                }
            }
            conn.prepare_cached("INSERT INTO sprint_tickets (sprint_id, ticket_id, added_at, removed_at, points_at_add) VALUES (?1, ?2, ?3, ?4, ?5)")?
                .execute(params![sprint_ids[si], ticket, e.added_at, e.removed_at, e.points_at_add])?;
        }
    }
    Ok(())
}

/// Zählt übersprungene Version-/Meilenstein-Werte in der Warnung (eine Warnung für alle).
fn count_version_value(w: &mut Vec<Warning>) {
    match w.iter_mut().find_map(|x| if let Warning::VersionValues(n) = x { Some(n) } else { None }) {
        Some(n) => *n += 1,
        None => w.push(Warning::VersionValues(1)),
    }
}

/// Ordnet die Verknüpfungstypen des Dokuments vorhandenen zu (Schlüssel, sonst Name) oder legt fehlende an.
fn resolve_link_types(conn: &Connection, docs: &[DocLinkType], warnings: &mut Vec<Warning>) -> Result<HashMap<i64, LinkType>, BackupError> {
    let mut out: HashMap<i64, LinkType> = HashMap::new();
    for d in docs {
        let w = || Where::LinkType(d.id);
        let tk = match d.target_kind.as_str() {
            "ticket" | "terminal" | "url" | "branch" => TargetKind::parse(&d.target_kind),
            other => return Err(BackupError::At(w(), Problem::TargetKind(other.to_string()))),
        };
        let existing = list_link_types(conn)?;
        let found = match d.key.as_deref() {
            Some(k) => existing.iter().find(|e| e.key.as_deref() == Some(k)),
            None => existing.iter().find(|e| e.key.is_none() && common::same_label(None, d.name_out.as_deref(), None, e.name_out.as_deref())),
        };
        let lt = match found {
            Some(e) => {
                if e.target_kind != tk || e.symmetric != d.symmetric {
                    return Err(BackupError::At(w(), Problem::LinkTypeDiffers));
                }
                e.clone()
            }
            None => {
                let id = create_missing_link_type(conn, d, tk).map_err(|e| match e {
                    BackupError::Plan(p) => BackupError::AtPlan(w(), p),
                    o => o,
                })?;
                let lt = get_link_type(conn, id)?;
                warnings.push(Warning::LinkTypeCreated { de: lt.names(Lang::De).0, en: lt.names(Lang::En).0 });
                lt
            }
        };
        out.insert(d.id, lt);
    }
    Ok(out)
}

fn create_missing_link_type(conn: &Connection, d: &DocLinkType, tk: TargetKind) -> Result<i64, BackupError> {
    // Mitgelieferter Typ, den es hier nicht mehr gibt (gelöscht): mit seinem Schlüssel (übersetzter Name) wieder anlegen.
    if let Some(k) = d.key.as_deref().filter(|k| *k != "terminal" && names::translate(&format!("link.{k}.out"), Lang::En).is_some()) {
        let pos = common::next_position(conn, "link_types", "1 = 1", &[])?;
        conn.execute(
            "INSERT INTO link_types (key, name_out, name_in, symmetric, system, target_kind, position) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
            params![k, d.name_out, d.name_in, d.symmetric as i64, tk.as_str(), pos],
        )?;
        return Ok(conn.last_insert_rowid());
    }
    let name = d.name_out.as_deref().map(str::trim).filter(|n| !n.is_empty()).ok_or(BackupError::At(Where::LinkType(d.id), Problem::LinkTypeUnnamed))?;
    Ok(create_link_type(conn, &NewLinkType { name_out: name.to_string(), name_in: d.name_in.clone(), symmetric: d.symmetric, target_kind: tk })?)
}
