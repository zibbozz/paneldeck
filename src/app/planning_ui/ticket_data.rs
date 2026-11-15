//! Laden der Daten für die Ticket-Oberfläche: Listenzeilen mit den Spalten Priorität und Story Points, das Detail eines
//! Tickets (Felder, Kinder, Verknüpfungen, Kommentare, Verlauf, Zeiten, erlaubte Statuswechsel) und kleine Suchhilfen.
//! Alles liest nur; geschrieben wird über die Datenschicht in `crate::planning`.

use super::quick_exec::{buttons_for, QuickButton};
use super::ticket_draft::Draft;
use super::ticket_state::Filters;
use crate::calc::duration::Minutes;
use crate::calc::progress::{Basis, Progress};
use crate::planning::comments::{comments_of, Comment};
use crate::planning::fields::FieldKind;
use crate::planning::history::{describe_with_source, history_of};
use crate::planning::link_types::{LinkType, TargetKind};
use crate::planning::links::{links_of, LinkView};
use crate::planning::summary::{project_summary, ProjectSummary, TypeSummary};
use crate::planning::ticket_error::{TRes, TicketError};
use crate::planning::ticket_list::{list_tickets, TicketFilter, TicketOrder};
use crate::planning::tickets::{children_of, get_ticket, is_done, progress_of, Ticket};
use crate::planning::time_entries::{entries_of, time_total, TimeEntry};
use crate::planning::transition::{available_transitions, TransitionOption};
use crate::planning::values::load_values;
use crate::state::Lang;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// So viele Verlaufseinträge zeigt ein Ticket höchstens (neueste zuerst).
pub const HISTORY_SHOW_MAX: usize = 200;

/// Eine Zeile der Liste: das Ticket und die Werte der Spalten „Priorität“ (Kennung der Auswahl) und „Story Points“.
#[derive(Clone, Debug)]
pub struct Row {
    pub ticket: Ticket,
    pub prio: Option<i64>,
    pub points: Option<f64>,
}

/// Lädt die Zeilen des Projekts nach dem Filter (alle Treffer; die Liste zeichnet nur den sichtbaren Ausschnitt).
/// Zweiter Wert: alle Tickets des Projekts ohne Filter.
pub fn load_rows(conn: &Connection, project_id: i64, f: &Filters) -> TRes<(Vec<Row>, usize)> {
    let page = list_tickets(conn, Some(project_id), &f.to_ticket_filter())?;
    let total = if f.is_empty() { page.total as usize } else { list_tickets(conn, Some(project_id), &TicketFilter { limit: Some(0), ..Default::default() })?.total as usize };
    let mut cols: HashMap<i64, (Option<i64>, Option<f64>)> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT v.ticket_id, f.key, v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
             WHERE t.project_id = ?1 AND ((f.key = 'priority' AND f.kind = 'dropdown') OR (f.key = 'story_points' AND f.kind = 'number')) AND v.num IS NOT NULL",
        )?;
        let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?)))?;
        for r in rows {
            let (id, key, num) = r?;
            let e = cols.entry(id).or_default();
            if key == "priority" {
                e.0 = Some(num as i64);
            } else {
                e.1 = Some(num);
            }
        }
    }
    let rows = page
        .items
        .into_iter()
        .map(|t| {
            let (prio, points) = cols.get(&t.id).copied().unwrap_or_default();
            Row { ticket: t, prio, points }
        })
        .collect();
    Ok((rows, total))
}

/// Suche für Auswahlfelder (Ticket-Verweis, Eltern-Ticket, Verknüpfungsziel): nach Schlüssel/Titel, höchstens `limit` Treffer,
/// zuletzt geänderte zuerst. `project` = `None` sucht in allen Projekten.
pub fn search_tickets(conn: &Connection, project: Option<i64>, text: &str, types: &[i64], exclude: &[i64], limit: usize) -> Vec<Ticket> {
    let f = TicketFilter { types: types.to_vec(), text: Some(text.trim().to_string()).filter(|t| !t.is_empty()), limit: Some(limit as i64 + exclude.len() as i64), order: TicketOrder::UpdatedDesc, ..Default::default() };
    list_tickets(conn, project, &f).map(|p| p.items.into_iter().filter(|t| !exclude.contains(&t.id)).take(limit).collect()).unwrap_or_default()
}

/// Bereits benutzte Labels des Projekts (für Vorschläge beim Eintippen).
pub fn label_suggestions(conn: &Connection, project_id: i64) -> Vec<String> {
    let q = "SELECT DISTINCT v.text FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
             WHERE f.kind = 'labels' AND t.project_id = ?1 AND v.text IS NOT NULL ORDER BY lower(v.text) LIMIT 300";
    let Ok(mut st) = conn.prepare(q) else { return Vec::new() };
    st.query_map([project_id], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Todo-Karten, die schon in Tickets umgewandelt wurden: Karte → Ticket.
pub fn conversions(conn: &Connection) -> HashMap<i64, i64> {
    let Ok(mut st) = conn.prepare("SELECT todo_id, ticket_id FROM todo_conversions") else { return HashMap::new() };
    st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

// ------------------------------------------------------------------ Verknüpfungen hinzufügen

/// Ein Angebot im Dialog „Verknüpfung hinzufügen“: Verknüpfungstyp mit Richtung.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkOffer {
    pub link_type_id: i64,
    /// Beschriftung („blockiert“, „ist blockiert durch“ …).
    pub label: String,
    /// Das aktuelle Ticket ist das Ziel (Gegenname gewählt).
    pub reverse: bool,
    pub kind: TargetKind,
}

/// Was sich von Hand anlegen lässt: Ticket-Verknüpfungen in beiden Richtungen, URLs und Branches.
/// Terminal-Verknüpfungen entstehen später aus einer Sitzung heraus und stehen hier nicht zur Wahl.
pub fn link_offers(types: &[LinkType], lang: Lang) -> Vec<LinkOffer> {
    let mut out = Vec::new();
    for t in types {
        let (name_out, name_in) = t.names(lang);
        match t.target_kind {
            TargetKind::Terminal => {}
            TargetKind::Ticket => {
                out.push(LinkOffer { link_type_id: t.id, label: name_out.clone(), reverse: false, kind: t.target_kind });
                if let Some(n) = name_in.filter(|n| *n != name_out && !t.symmetric) {
                    out.push(LinkOffer { link_type_id: t.id, label: n, reverse: true, kind: t.target_kind });
                }
            }
            TargetKind::Url | TargetKind::Branch => out.push(LinkOffer { link_type_id: t.id, label: name_out, reverse: false, kind: t.target_kind }),
        }
    }
    out
}

// ------------------------------------------------------------------ Detail

#[derive(Clone, Debug)]
pub struct ChildRow {
    pub ticket: Ticket,
    pub done: bool,
}

/// Alles, was die Ticket-Ansicht zum Zeichnen braucht.
pub struct Detail {
    pub ticket: Ticket,
    pub sum: Rc<ProjectSummary>,
    pub ty: TypeSummary,
    pub stored: BTreeMap<i64, Value>,
    pub parent: Option<Ticket>,
    pub children: Vec<ChildRow>,
    pub progress: Option<Progress>,
    pub links: Vec<LinkView>,
    pub comments: Vec<Comment>,
    /// Verlauf, neueste zuerst: (Zeitpunkt, Satz mit Quelle).
    pub history: Vec<(String, String)>,
    pub history_total: usize,
    pub time: Vec<TimeEntry>,
    pub time_total: Minutes,
    pub transitions: Vec<TransitionOption>,
    pub done: bool,
    /// Schnellaktionen des Typs als Knöpfe (wird mit dem Detail geladen, nicht je Bild).
    pub quick: Vec<QuickButton>,
}

impl Detail {
    /// Lädt das Ticket; `hint` ist die Zusammenfassung des gewählten Projekts (spart eine Abfrage).
    pub fn load(conn: &Connection, hint: Option<&Rc<ProjectSummary>>, id: i64, lang: Lang) -> TRes<Detail> {
        let ticket = get_ticket(conn, id)?;
        let sum = match hint.filter(|s| s.project.id == ticket.project_id) {
            Some(s) => s.clone(),
            None => Rc::new(project_summary(conn, ticket.project_id)?),
        };
        let ty = sum.type_by_id(ticket.type_id).cloned().ok_or_else(TicketError::not_found)?;
        let stored = load_values(conn, id)?;
        let parent = match ticket.parent_id {
            Some(p) => get_ticket(conn, p).ok(),
            None => None,
        };
        let mut children = Vec::new();
        for c in children_of(conn, id)? {
            let done = is_done(conn, &c).unwrap_or(false);
            children.push(ChildRow { ticket: c, done });
        }
        let progress = if children.is_empty() { None } else { Some(progress_of(conn, id, Basis::default())?) };
        let entries = history_of(conn, id)?;
        let history_total = entries.len();
        let history = entries.iter().rev().take(HISTORY_SHOW_MAX).map(|e| (e.at.replace('T', " ").chars().take(16).collect::<String>(), describe_with_source(conn, e, lang))).collect();
        let done = is_done(conn, &ticket).unwrap_or(false);
        let quick = buttons_for(conn, &sum, &ticket, lang);
        Ok(Detail {
            quick,
            links: links_of(conn, id, lang)?,
            comments: comments_of(conn, id)?,
            time: entries_of(conn, id)?,
            time_total: time_total(conn, id)?,
            transitions: available_transitions(conn, id)?,
            ticket,
            sum,
            ty,
            stored,
            parent,
            children,
            progress,
            history,
            history_total,
            done,
        })
    }

    /// Frische Eingabepuffer für alle Felder aus den gespeicherten Werten; vorhandene, noch nicht gespeicherte Eingaben bleiben.
    pub fn merge_drafts(&self, drafts: &mut HashMap<i64, Draft>) {
        let ids: Vec<i64> = self.ty.fields.iter().map(|f| f.id).collect();
        drafts.retain(|id, _| ids.contains(id));
        for f in &self.ty.fields {
            let keep = drafts.get(&f.id).is_some_and(|d| d.edited);
            if !keep {
                drafts.insert(f.id, Draft::from_value(f, self.stored.get(&f.id)));
            }
        }
    }
}

/// Anzeigetext des Fortschritts „3 von 5 (60 %)“; bei Punkten als Gewicht.
pub fn progress_text(p: &Progress, lang: Lang) -> String {
    let num = |x: f64| if x.fract() == 0.0 { format!("{}", x as i64) } else { format!("{x:.1}") };
    match lang {
        Lang::De => format!("{} von {} ({} %)", num(p.done), num(p.total), p.percent()),
        Lang::En => format!("{} of {} ({}%)", num(p.done), num(p.total), p.percent()),
    }
}

/// Ist das Feld eines, das die Liste als „Priorität“ nutzt?
pub fn is_priority(f: &crate::planning::fields::Field) -> bool {
    f.key.as_deref() == Some("priority") && f.kind == FieldKind::Dropdown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::history::Source;
    use crate::planning::link_types::list_link_types;
    use crate::planning::ticket_list::TicketFilter;
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::{create_ticket, set_flagged, NewTicket};
    use serde_json::json;

    #[test]
    fn rows_carry_priority_and_story_points() {
        let f = Fx::new();
        let sp = f.field(f.story, "story_points");
        let pr = f.field(f.story, "priority");
        let t = create_ticket(&f.c, &NewTicket { values: vec![(sp.id, json!(5))], ..NewTicket::new(f.project, f.story, "Mit Punkten") }, &Source::ui()).unwrap().0;
        f.mk(f.epic, "Epic");
        let (rows, total) = load_rows(&f.c, f.project, &Filters::default()).unwrap();
        assert_eq!((rows.len(), total), (2, 2));
        let r = rows.iter().find(|r| r.ticket.id == t.id).unwrap();
        assert_eq!(r.points, Some(5.0));
        let medium = pr.default_value.as_ref().and_then(|v| v.as_i64());
        assert_eq!(r.prio, medium, "Standard-Priorität „Medium“");
        let epic = rows.iter().find(|r| r.ticket.title == "Epic").unwrap();
        assert_eq!(epic.points, None);
    }

    #[test]
    fn filter_counts_hits_and_project_total() {
        let f = Fx::new();
        let a = f.mk(f.task, "Alpha Aufgabe");
        f.mk(f.task, "Beta Aufgabe");
        f.mk(f.epic, "Gamma");
        set_flagged(&f.c, a.id, true, &Source::ui()).unwrap();
        let by = |filters: Filters| load_rows(&f.c, f.project, &filters).unwrap();
        let (r, total) = by(Filters { text: "aufgabe".into(), ..Default::default() });
        assert_eq!((r.len(), total), (2, 3), "zwei Treffer von drei Tickets");
        assert_eq!(by(Filters { flagged: true, ..Default::default() }).0.len(), 1);
        assert_eq!(by(Filters { types: vec![f.epic], ..Default::default() }).0.len(), 1);
        assert_eq!(by(Filters { text: "PAN-3".into(), ..Default::default() }).0.len(), 1, "Suche nach dem Schlüssel");
        assert_eq!(by(Filters { text: "nichts davon".into(), ..Default::default() }).0.len(), 0);
    }

    #[test]
    fn search_respects_types_and_exclusions() {
        let f = Fx::new();
        let e1 = f.mk(f.epic, "Erstes Epic");
        f.mk(f.epic, "Zweites Epic");
        f.mk(f.task, "Epic-Aufgabe");
        let all = search_tickets(&f.c, Some(f.project), "epic", &[], &[], 10);
        assert_eq!(all.len(), 3);
        let only_epics = search_tickets(&f.c, Some(f.project), "epic", &[f.epic], &[], 10);
        assert_eq!(only_epics.len(), 2);
        let without = search_tickets(&f.c, Some(f.project), "epic", &[f.epic], &[e1.id], 10);
        assert_eq!(without.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Zweites Epic"]);
        assert_eq!(search_tickets(&f.c, Some(f.project), "epic", &[], &[], 1).len(), 1, "höchstens limit");
        assert_eq!(search_tickets(&f.c, None, "PAN-1", &[], &[], 5).len(), 1, "projektübergreifend nach Schlüssel");
        assert_eq!(search_tickets(&f.c, Some(f.project), "", &[], &[], 2).len(), 2, "leerer Text zeigt die jüngsten");
    }

    #[test]
    fn labels_are_suggested_from_existing_values() {
        let f = Fx::new();
        let l = f.add_field(f.task, "Tags", FieldKind::Labels, Default::default(), false);
        for (title, labels) in [("a", json!(["Backend", "UI"])), ("b", json!(["backend2"]))] {
            create_ticket(&f.c, &NewTicket { values: vec![(l.id, labels)], ..NewTicket::new(f.project, f.task, title) }, &Source::ui()).unwrap();
        }
        assert_eq!(label_suggestions(&f.c, f.project), vec!["Backend", "backend2", "UI"]);
        assert!(label_suggestions(&f.c, 9999).is_empty());
    }

    #[test]
    fn detail_collects_everything_for_the_page() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "Das Epic");
        let story = f.child(&epic, f.story, "Story");
        f.child(&story, f.task, "Aufgabe");
        let d = Detail::load(&f.c, None, story.id, Lang::En).unwrap();
        assert_eq!(d.parent.as_ref().map(|p| p.id), Some(epic.id));
        assert_eq!(d.children.len(), 1);
        assert_eq!(d.progress.map(|p| p.percent()), Some(0));
        assert!(!d.history.is_empty() && d.history[0].1.contains("User interface"), "{:?}", d.history);
        assert!(d.transitions.len() >= 1, "To Do darf weiterwechseln");
        assert!(!d.done);
        let mut drafts = HashMap::new();
        d.merge_drafts(&mut drafts);
        assert_eq!(drafts.len(), d.ty.fields.len());
        // bearbeitete Puffer bleiben beim erneuten Laden
        let fid = d.ty.fields[0].id;
        drafts.get_mut(&fid).unwrap().text = "ungespeichert".into();
        drafts.get_mut(&fid).unwrap().edited = true;
        d.merge_drafts(&mut drafts);
        assert_eq!(drafts[&fid].text, "ungespeichert");
        assert!(Detail::load(&f.c, None, 9999, Lang::En).is_err());
        let _ = TicketFilter::default();
    }

    #[test]
    fn link_offers_follow_the_link_types() {
        let f = Fx::new();
        let types = list_link_types(&f.c).unwrap();
        let offers = link_offers(&types, Lang::En);
        let labels: Vec<&str> = offers.iter().map(|o| o.label.as_str()).collect();
        assert!(labels.contains(&"blocks") && labels.contains(&"is blocked by"), "{labels:?}");
        assert!(labels.contains(&"relates to"), "{labels:?}");
        assert_eq!(labels.iter().filter(|l| **l == "relates to").count(), 1, "symmetrisch: nur eine Richtung");
        assert!(offers.iter().all(|o| o.kind != TargetKind::Terminal), "Terminal-Verknüpfungen nicht von Hand");
        let de = link_offers(&types, Lang::De);
        assert!(de.iter().any(|o| o.label == "blockiert") && de.iter().any(|o| o.reverse));
    }

    #[test]
    fn progress_reads_naturally() {
        let p = Progress { done: 3.0, total: 5.0, fraction: 0.6 };
        assert_eq!(progress_text(&p, Lang::En), "3 of 5 (60%)");
        assert_eq!(progress_text(&p, Lang::De), "3 von 5 (60 %)");
        let q = Progress { done: 1.5, total: 4.0, fraction: 0.375 };
        assert!(progress_text(&q, Lang::En).starts_with("1.5 of 4"));
    }
}
