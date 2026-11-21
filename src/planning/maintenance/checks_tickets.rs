//! Ticket-Prüfungen der Wartung: liegengeblieben (in Arbeit / nie begonnen), überfällig, offen in abgeschlossenen Sprints,
//! Versionen oder Meilensteinen, kaputte Struktur (verwaiste Unteraufgaben, Verweise auf gelöschte Tickets, Eltern erledigt
//! bei offenen Kindern, Zyklen in Verknüpfungen, fehlende Pflichtfelder, ungültige Werte) und mögliche Duplikate.
//! Archivierte Tickets werden nicht geprüft. Nur lesend.

use super::similar;
use super::{days_since, load_tickets, Check, Finding, Fix, MRes, RunOpts, Settings, TRow, Target, TicketRef};
use crate::planning::fields::{list_fields, Field, FieldKind};
use crate::planning::projects::Project;
use crate::state::Lang;
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Höchstzahl offener Tickets für die Duplikat-Suche (Paare wachsen quadratisch).
pub const MAX_DUPLICATE_TICKETS: usize = 4000;

fn finding(check: Check, id: String, t: Option<&TRow>, target: Target, message: String, fixes: Vec<Fix>, detail: serde_json::Value) -> Finding {
    Finding { id, check, severity: check.severity(), ticket: t.map(TRow::tref), other: None, target, message, fixes, detail }
}

/// Alle Ticket-Prüfungen; das `bool` meldet, dass die Duplikat-Suche an ihrer Obergrenze vorzeitig endete.
pub fn run(conn: &Connection, project: &Project, s: &Settings, o: &RunOpts) -> MRes<(Vec<Finding>, bool)> {
    let lang = o.lang;
    let tickets = load_tickets(conn, project)?;
    let by_id: HashMap<i64, &TRow> = tickets.iter().map(|t| (t.id, t)).collect();
    let mut out = Vec::new();
    stale_and_overdue(conn, project, s, o, &tickets, &mut out)?;
    open_in_finished(conn, project, lang, &tickets, &by_id, &mut out)?;
    structure(conn, project, lang, &tickets, &by_id, &mut out)?;
    values(conn, project, lang, &tickets, &mut out)?;
    let mut truncated = false;
    if o.duplicates && s.enabled(Check::PossibleDuplicate) {
        truncated = duplicates(conn, s, o, &tickets, &mut out)?;
    }
    Ok((out, truncated))
}

// ------------------------------------------------------------------ liegengeblieben, überfällig

fn stale_and_overdue(conn: &Connection, project: &Project, s: &Settings, o: &RunOpts, tickets: &[TRow], out: &mut Vec<Finding>) -> MRes<()> {
    let lang = o.lang;
    for t in tickets.iter().filter(|t| t.open()) {
        let Some(days) = days_since(&t.updated_at, o.now) else { continue };
        if days < s.stale_days {
            continue;
        }
        let progress = t.category == Some(crate::planning::statuses::StatusCategory::Progress);
        let check = if progress { Check::StaleInProgress } else { Check::StaleNew };
        let message = match (lang, progress) {
            (Lang::De, true) => format!("In Arbeit, aber seit {days} Tagen unverändert (Schwelle {} Tage) – hängt es fest?", s.stale_days),
            (Lang::En, true) => format!("In progress but unchanged for {days} days (threshold {} days) – is it stuck?", s.stale_days),
            (Lang::De, false) => format!("Nie begonnen und seit {days} Tagen unverändert (Schwelle {} Tage).", s.stale_days),
            (Lang::En, false) => format!("Never started and unchanged for {days} days (threshold {} days).", s.stale_days),
        };
        out.push(finding(check, format!("{}:{}", check.as_str(), t.key), Some(t), Target::Ticket, message, vec![Fix::CloseWontDo, Fix::Archive, Fix::SetDue], json!({"days": days, "threshold": s.stale_days})));
    }
    // Fälligkeit (eingebautes Feld): vorbei und offen
    let mut st = conn.prepare(
        "SELECT v.ticket_id, v.text FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND f.builtin = 1 AND f.key = ?2 AND v.text IS NOT NULL",
    )?;
    let due: HashMap<i64, String> = st.query_map(params![project.id, crate::planning::builtin_fields::DUE_KEY], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for t in tickets.iter().filter(|t| t.open()) {
        let Some(stored) = due.get(&t.id) else { continue };
        let Some(deadline) = crate::due::deadline(stored) else { continue };
        if deadline >= o.now {
            continue;
        }
        let days = o.now.signed_duration_since(deadline).num_days();
        let shown = crate::due::display(stored, lang);
        let message = match lang {
            Lang::De => format!("Überfällig seit {shown} ({days} Tage) und noch offen."),
            Lang::En => format!("Overdue since {shown} ({days} days) and still open."),
        };
        out.push(finding(Check::Overdue, format!("overdue:{}", t.key), Some(t), Target::Ticket, message, vec![Fix::SetDue, Fix::CloseWontDo, Fix::Archive], json!({"due": stored, "days": days})));
    }
    Ok(())
}

// ------------------------------------------------------------------ offen in abgeschlossenen Sprints/Versionen/Meilensteinen

fn open_in_finished(conn: &Connection, project: &Project, lang: Lang, tickets: &[TRow], by_id: &HashMap<i64, &TRow>, out: &mut Vec<Finding>) -> MRes<()> {
    let _ = tickets;
    let mut hits: Vec<(i64, String, String)> = Vec::new(); // (ticket, art, name)
    let mut st = conn.prepare(
        "SELECT st.ticket_id, s.name FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id
         WHERE s.project_id = ?1 AND s.state = 'completed' AND st.removed_at IS NULL",
    )?;
    for r in st.query_map([project.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (t, name) = r?;
        hits.push((t, "sprint".into(), name));
    }
    for (kind, table, flag) in [("version", "versions", "released"), ("milestone", "milestones", "reached")] {
        let sql = format!(
            "SELECT v.ticket_id, r.name FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN {table} r ON CAST(r.id AS TEXT) = v.text
             WHERE f.kind = ?1 AND r.project_id = ?2 AND r.{flag} = 1"
        );
        let mut st = conn.prepare(&sql)?;
        for r in st.query_map(params![kind, project.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (t, name) = r?;
            hits.push((t, kind.into(), name));
        }
    }
    let mut seen = HashSet::new();
    for (tid, kind, name) in hits {
        let Some(t) = by_id.get(&tid).filter(|t| t.open()) else { continue };
        if !seen.insert((tid, kind.clone(), name.clone())) {
            continue;
        }
        let what = match (lang, kind.as_str()) {
            (Lang::De, "sprint") => "abgeschlossenen Sprint",
            (Lang::En, "sprint") => "completed sprint",
            (Lang::De, "version") => "veröffentlichten Version",
            (Lang::En, "version") => "released version",
            (Lang::De, _) => "erreichten Meilenstein",
            (Lang::En, _) => "reached milestone",
        };
        let message = match lang {
            Lang::De => format!("Noch offen, gehört aber zum {what} {}.", lang.q(&name)),
            Lang::En => format!("Still open but belongs to the {what} {}.", lang.q(&name)),
        };
        out.push(finding(Check::OpenInFinished, format!("open_in_finished:{}:{kind}:{name}", t.key), Some(t), Target::Ticket, message, vec![Fix::CloseWontDo, Fix::Archive], json!({"in": kind, "name": name})));
    }
    Ok(())
}

// ------------------------------------------------------------------ Struktur

fn structure(conn: &Connection, project: &Project, lang: Lang, tickets: &[TRow], by_id: &HashMap<i64, &TRow>, out: &mut Vec<Finding>) -> MRes<()> {
    // Alle Kennungen des Projekts samt archivierter: ein archiviertes Eltern-Ticket ist nicht „gelöscht“.
    let mut st = conn.prepare("SELECT id FROM tickets WHERE project_id = ?1")?;
    let all: HashSet<i64> = st.query_map([project.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut st = conn.prepare("SELECT id FROM tickets")?;
    let everywhere: HashSet<i64> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let _ = all;
    // verwaiste Unteraufgaben: Eltern-Ticket existiert nicht mehr (nur möglich, wenn Fremdschlüssel einmal aus waren)
    for t in tickets {
        if let Some(p) = t.parent_id.filter(|p| !everywhere.contains(p)) {
            let message = match lang {
                Lang::De => format!("Das Eltern-Ticket (Kennung {p}) gibt es nicht mehr."),
                Lang::En => format!("The parent ticket (id {p}) no longer exists."),
            };
            out.push(finding(Check::OrphanedSubtask, format!("orphaned_subtask:{}", t.key), Some(t), Target::Ticket, message, vec![Fix::DetachParent], json!({"parent_id": p})));
        }
    }
    // Eltern erledigt, Kinder offen
    let mut children: HashMap<i64, Vec<&TRow>> = HashMap::new();
    for t in tickets.iter().filter(|t| t.open()) {
        if let Some(p) = t.parent_id {
            children.entry(p).or_default().push(t);
        }
    }
    for (pid, kids) in &children {
        let Some(parent) = by_id.get(pid).filter(|p| p.done()) else { continue };
        let keys: Vec<String> = kids.iter().map(|k| k.key.clone()).collect();
        let message = match lang {
            Lang::De => format!("Erledigt, aber {} Unteraufgabe(n) sind noch offen: {}.", kids.len(), keys.join(", ")),
            Lang::En => format!("Done, but {} child ticket(s) are still open: {}.", kids.len(), keys.join(", ")),
        };
        out.push(finding(
            Check::ParentDoneChildrenOpen,
            format!("parent_done_children_open:{}", parent.key),
            Some(parent),
            Target::Tickets(kids.iter().map(|k| k.id).collect()),
            message,
            vec![Fix::CloseWontDo, Fix::Archive],
            json!({"children": keys}),
        ));
    }
    // Verweise auf gelöschte Tickets: Verknüpfungen und Ticket-Felder
    let mut broken: HashMap<i64, usize> = HashMap::new();
    let mut st = conn.prepare(
        "SELECT l.from_ticket_id, l.to_ticket_id FROM links l JOIN tickets t ON t.id = l.from_ticket_id
         WHERE t.project_id = ?1 AND l.to_kind = 'ticket'",
    )?;
    for r in st.query_map([project.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))? {
        let (from, to) = r?;
        if to.is_none_or(|to| !everywhere.contains(&to)) {
            *broken.entry(from).or_default() += 1;
        }
    }
    let mut st = conn.prepare(
        "SELECT v.ticket_id, v.text FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND f.kind = 'ticket' AND v.text IS NOT NULL",
    )?;
    for r in st.query_map([project.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (tid, text) = r?;
        if text.trim().parse::<i64>().map(|id| !everywhere.contains(&id)).unwrap_or(true) {
            *broken.entry(tid).or_default() += 1;
        }
    }
    for t in tickets {
        if let Some(n) = broken.get(&t.id) {
            let message = match lang {
                Lang::De => format!("{n} Verweis(e) zeigen auf gelöschte Tickets."),
                Lang::En => format!("{n} reference(s) point to deleted tickets."),
            };
            out.push(finding(Check::BrokenReference, format!("broken_reference:{}", t.key), Some(t), Target::Ticket, message, vec![Fix::ClearReference], json!({"count": n})));
        }
    }
    cycles(conn, project, lang, by_id, out)?;
    Ok(())
}

/// Zyklen je gerichtetem Verknüpfungstyp (z. B. A blockiert B blockiert A), über Tickets des Projekts.
fn cycles(conn: &Connection, project: &Project, lang: Lang, by_id: &HashMap<i64, &TRow>, out: &mut Vec<Finding>) -> MRes<()> {
    let mut st = conn.prepare(
        "SELECT l.link_type_id, l.from_ticket_id, l.to_ticket_id FROM links l JOIN link_types lt ON lt.id = l.link_type_id
         JOIN tickets a ON a.id = l.from_ticket_id JOIN tickets b ON b.id = l.to_ticket_id
         WHERE lt.symmetric = 0 AND l.to_kind = 'ticket' AND a.project_id = ?1 AND b.project_id = ?1 AND a.archived = 0 AND b.archived = 0",
    )?;
    let mut graphs: HashMap<i64, HashMap<i64, Vec<i64>>> = HashMap::new();
    for r in st.query_map([project.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
        let (lt, a, b) = r?;
        graphs.entry(lt).or_default().entry(a).or_default().push(b);
    }
    let mut lts: Vec<_> = graphs.keys().copied().collect();
    lts.sort_unstable();
    for lt in lts {
        let g = &graphs[&lt];
        let lt_name = crate::planning::link_types::get_link_type(conn, lt).map(|l| l.names(lang).0).unwrap_or_default();
        for comp in strongly_connected(g) {
            let mut keys: Vec<String> = comp.iter().filter_map(|id| by_id.get(id).map(|t| t.key.clone())).collect();
            keys.sort();
            let Some(first) = comp.iter().filter_map(|id| by_id.get(id)).min_by_key(|t| t.id) else { continue };
            let message = match lang {
                Lang::De => format!("Zyklus über {}: {}.", lang.q(&lt_name), keys.join(" → ")),
                Lang::En => format!("Cycle via {}: {}.", lang.q(&lt_name), keys.join(" → ")),
            };
            out.push(finding(Check::LinkCycle, format!("link_cycle:{lt}:{}", keys.join(",")), Some(first), Target::Tickets(comp.clone()), message, vec![], json!({"tickets": keys, "link_type": lt_name})));
        }
    }
    Ok(())
}

/// Starke Zusammenhangskomponenten mit mehr als einem Knoten (Tarjan, iterativ genug für Ticket-Graphen).
pub(crate) fn strongly_connected(g: &HashMap<i64, Vec<i64>>) -> Vec<Vec<i64>> {
    struct T<'a> {
        g: &'a HashMap<i64, Vec<i64>>,
        index: HashMap<i64, usize>,
        low: HashMap<i64, usize>,
        on: HashSet<i64>,
        stack: Vec<i64>,
        next: usize,
        out: Vec<Vec<i64>>,
    }
    fn visit(t: &mut T, v: i64, depth: usize) {
        if depth > 10_000 {
            return;
        }
        t.index.insert(v, t.next);
        t.low.insert(v, t.next);
        t.next += 1;
        t.stack.push(v);
        t.on.insert(v);
        for &w in t.g.get(&v).map(|x| x.as_slice()).unwrap_or(&[]) {
            if !t.index.contains_key(&w) {
                visit(t, w, depth + 1);
                let lw = t.low[&w];
                let lv = t.low.get_mut(&v).expect("v");
                *lv = (*lv).min(lw);
            } else if t.on.contains(&w) {
                let iw = t.index[&w];
                let lv = t.low.get_mut(&v).expect("v");
                *lv = (*lv).min(iw);
            }
        }
        if t.low[&v] == t.index[&v] {
            let mut comp = Vec::new();
            while let Some(w) = t.stack.pop() {
                t.on.remove(&w);
                comp.push(w);
                if w == v {
                    break;
                }
            }
            if comp.len() > 1 {
                comp.sort_unstable();
                t.out.push(comp);
            }
        }
    }
    let mut t = T { g, index: HashMap::new(), low: HashMap::new(), on: HashSet::new(), stack: Vec::new(), next: 0, out: Vec::new() };
    let mut nodes: Vec<i64> = g.keys().copied().collect();
    nodes.sort_unstable();
    for v in nodes {
        if !t.index.contains_key(&v) {
            visit(&mut t, v, 0);
        }
    }
    t.out.sort();
    t.out
}

// ------------------------------------------------------------------ Pflichtfelder und ungültige Werte

fn values(conn: &Connection, project: &Project, lang: Lang, tickets: &[TRow], out: &mut Vec<Finding>) -> MRes<()> {
    let types = crate::planning::types::list_types(conn, project.id)?;
    let mut fields_of: HashMap<i64, Vec<Field>> = HashMap::new();
    let mut workflow_of: HashMap<i64, (bool, HashSet<i64>)> = HashMap::new();
    for ty in &types {
        fields_of.insert(ty.id, list_fields(conn, ty.id)?);
        let statuses: HashSet<i64> = crate::planning::workflow::workflow_statuses(conn, ty.id)?.into_iter().collect();
        workflow_of.insert(ty.id, (ty.workflow, statuses));
    }
    // vorhandene Werte je (Ticket, Feld)
    let mut st = conn.prepare(
        "SELECT v.ticket_id, v.field_id, v.text, v.num FROM ticket_values v JOIN tickets t ON t.id = v.ticket_id WHERE t.project_id = ?1 AND t.archived = 0",
    )?;
    let mut present: HashSet<(i64, i64)> = HashSet::new();
    let mut rows: Vec<(i64, i64, Option<String>, Option<f64>)> = Vec::new();
    for r in st.query_map([project.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<f64>>(3)?)))? {
        let (t, f, text, num) = r?;
        if text.as_deref().is_some_and(|s| !s.trim().is_empty()) || num.is_some() {
            present.insert((t, f));
        }
        rows.push((t, f, text, num));
    }
    let versions = ids(conn, "SELECT id FROM versions WHERE project_id = ?1", project.id)?;
    let milestones = ids(conn, "SELECT id FROM milestones WHERE project_id = ?1", project.id)?;
    let field_by_id: HashMap<i64, &Field> = fields_of.values().flatten().map(|f| (f.id, f)).collect();
    let mut invalid: HashMap<i64, BTreeSet<String>> = HashMap::new();
    for (tid, fid, text, num) in &rows {
        let Some(f) = field_by_id.get(fid) else { continue };
        let bad = match f.kind {
            FieldKind::Dropdown | FieldKind::Multi => num.is_some_and(|n| !f.options.values.iter().any(|o| o.id as f64 == n.round())),
            FieldKind::Version => text.as_deref().is_some_and(|s| s.trim().parse::<i64>().map(|id| !versions.contains(&id)).unwrap_or(true)),
            FieldKind::Milestone => text.as_deref().is_some_and(|s| s.trim().parse::<i64>().map(|id| !milestones.contains(&id)).unwrap_or(true)),
            _ => false,
        };
        if bad {
            invalid.entry(*tid).or_default().insert(f.display(lang));
        }
    }
    for t in tickets {
        // Pflichtfelder
        let missing: Vec<String> = fields_of.get(&t.type_id).into_iter().flatten().filter(|f| f.required && !present.contains(&(t.id, f.id))).map(|f| f.display(lang)).collect();
        if !missing.is_empty() {
            let message = match lang {
                Lang::De => format!("Pflichtfeld(er) leer: {}.", missing.join(", ")),
                Lang::En => format!("Required field(s) empty: {}.", missing.join(", ")),
            };
            out.push(finding(Check::MissingRequired, format!("missing_required:{}", t.key), Some(t), Target::Ticket, message, vec![], json!({"fields": missing})));
        }
        // Status passt nicht zum Workflow des Typs
        let mut problems: Vec<String> = invalid.remove(&t.id).map(|s| s.into_iter().collect()).unwrap_or_default();
        let mut status_bad = false;
        if let Some((wf, statuses)) = workflow_of.get(&t.type_id) {
            match t.status_id {
                Some(s) if !*wf || !statuses.contains(&s) => status_bad = true,
                None if *wf => status_bad = true,
                _ => {}
            }
        }
        if status_bad {
            problems.insert(0, lang.t("Status", "Status").to_string());
        }
        if !problems.is_empty() {
            let message = match lang {
                Lang::De => format!("Ungültige Werte (z. B. gelöschter Auswahlwert): {}.", problems.join(", ")),
                Lang::En => format!("Invalid values (e.g. a deleted option): {}.", problems.join(", ")),
            };
            // Gelöschte Auswahlwerte/Versionen lassen sich eindeutig leeren; ein falscher Status braucht eine Entscheidung.
            let fixes = if problems.len() > usize::from(status_bad) { vec![Fix::ClearReference] } else { vec![] };
            out.push(finding(Check::InvalidValue, format!("invalid_value:{}", t.key), Some(t), Target::Ticket, message, fixes, json!({"fields": problems, "status": status_bad})));
        }
    }
    Ok(())
}

fn ids(conn: &Connection, sql: &str, p: i64) -> rusqlite::Result<HashSet<i64>> {
    let mut st = conn.prepare(sql)?;
    let v = st.query_map([p], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

// ------------------------------------------------------------------ mögliche Duplikate

/// Liefert `true`, wenn die Suche vorzeitig endete (mehr offene Tickets als [`MAX_DUPLICATE_TICKETS`] oder
/// [`RunOpts::duplicate_budget`] erreicht).
fn duplicates(conn: &Connection, s: &Settings, o: &RunOpts, tickets: &[TRow], out: &mut Vec<Finding>) -> MRes<bool> {
    let lang = o.lang;
    // Schon als Duplikat verknüpfte Paare (Typ `duplicates`) nicht noch einmal vorschlagen.
    let mut st = conn.prepare(
        "SELECT l.from_ticket_id, l.to_ticket_id FROM links l JOIN link_types lt ON lt.id = l.link_type_id WHERE lt.key = 'duplicates' AND l.to_ticket_id IS NOT NULL",
    )?;
    let linked: HashSet<(i64, i64)> = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flat_map(|(a, b)| [(a, b), (b, a)])
        .collect();
    let all_open = tickets.iter().filter(|t| t.open() || t.category.is_none()).count();
    let mut truncated = all_open > MAX_DUPLICATE_TICKETS;
    let open: Vec<&TRow> = tickets.iter().filter(|t| t.open() || t.category.is_none()).take(MAX_DUPLICATE_TICKETS).collect();
    let norm: Vec<(String, usize)> = open.iter().map(|t| {
        let n = similar::normalize(&t.title);
        let len = n.chars().count();
        (n, len)
    }).collect();
    let threshold = s.duplicate_percent.clamp(50, 100) as u32;
    let started = std::time::Instant::now();
    let mut compared = 0usize;
    'outer: for i in 0..open.len() {
        for j in (i + 1)..open.len() {
            let (a, b) = (open[i], open[j]);
            if similar::length_bound(norm[i].1, norm[j].1) < threshold || linked.contains(&(a.id, b.id)) {
                continue;
            }
            // Obergrenze: gezählt werden die teuren Vergleiche (Levenshtein); die Uhr nur alle 256 Vergleiche lesen.
            if let Some(bud) = o.duplicate_budget {
                if compared >= bud.max_pairs || (compared % 256 == 0 && compared > 0 && started.elapsed() >= bud.max_time) {
                    truncated = true;
                    break 'outer;
                }
            }
            compared += 1;
            let p = similar::percent_normalized(&norm[i].0, &norm[j].0);
            if p < threshold {
                continue;
            }
            let message = match lang {
                Lang::De => format!("Titel zu {p} % ähnlich wie {} – vielleicht ein Duplikat (nur ein Vorschlag).", b.key),
                Lang::En => format!("Title {p}% similar to {} – possibly a duplicate (only a suggestion).", b.key),
            };
            let mut f = finding(Check::PossibleDuplicate, format!("possible_duplicate:{}:{}", a.key, b.key), Some(a), Target::Ticket, message, vec![Fix::LinkDuplicate], json!({"percent": p, "other": b.key}));
            f.other = Some(TicketRef { id: b.id, key: b.key.clone(), title: b.title.clone() });
            out.push(f);
        }
    }
    Ok(truncated)
}
