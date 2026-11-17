//! Einfache (nicht-JQL) Abfrage der Tickets mit Filtern, Sortierung und Seiten – die Grundlage der Listenansicht, bis die
//! Abfragesprache angeschlossen ist.
//!
//! Der Text-Filter sucht ohne Beachtung der Groß-/Kleinschreibung (SQLite: nur ASCII) im Titel, im Schlüssel (`PAN-12`)
//! und in den Werten der Text-, Rich-Text- und Labels-Felder; mehrere Wörter müssen alle vorkommen.

use super::ticket_error::TRes;
use super::tickets::{from_row, Ticket, COLS, FROM};
use rusqlite::types::Value as Sql;
use rusqlite::{params_from_iter, Connection};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParentFilter {
    /// Egal.
    #[default]
    Any,
    /// Nur Tickets ohne Eltern-Ticket.
    Root,
    /// Nur Kinder dieses Tickets.
    Of(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TicketOrder {
    /// Backlog-Reihenfolge.
    #[default]
    Rank,
    NumberAsc,
    NumberDesc,
    CreatedDesc,
    UpdatedDesc,
    Title,
}

impl TicketOrder {
    fn sql(self) -> &'static str {
        match self {
            TicketOrder::Rank => "t.rank, t.id",
            TicketOrder::NumberAsc => "p.key, t.number",
            TicketOrder::NumberDesc => "p.key, t.number DESC",
            TicketOrder::CreatedDesc => "t.created_at DESC, t.id DESC",
            TicketOrder::UpdatedDesc => "t.updated_at DESC, t.id DESC",
            TicketOrder::Title => "t.title COLLATE NOCASE, t.id",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TicketFilter {
    /// Nur diese Typen (leer = alle).
    pub types: Vec<i64>,
    /// Nur diese Status (leer = alle).
    pub statuses: Vec<i64>,
    pub parent: ParentFilter,
    pub starred: Option<bool>,
    pub flagged: Option<bool>,
    pub text: Option<String>,
    /// Höchstens so viele Treffer (`None` = alle).
    pub limit: Option<i64>,
    pub offset: i64,
    pub order: TicketOrder,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TicketPage {
    pub items: Vec<Ticket>,
    /// Alle Treffer des Filters, unabhängig von `limit`/`offset`.
    pub total: i64,
}

fn like_pattern(word: &str) -> String {
    let esc: String = word.chars().flat_map(|c| if matches!(c, '%' | '_' | '\\') { vec!['\\', c] } else { vec![c] }).collect();
    format!("%{esc}%")
}

/// Tickets eines Projekts (`None` = aller Projekte) nach dem Filter.
pub fn list_tickets(conn: &Connection, project_id: Option<i64>, f: &TicketFilter) -> TRes<TicketPage> {
    let mut wh: Vec<String> = Vec::new();
    let mut args: Vec<Sql> = Vec::new();
    let mut push = |sql: String, v: Vec<Sql>, wh: &mut Vec<String>| {
        wh.push(sql);
        args.extend(v);
    };
    if let Some(p) = project_id {
        push("t.project_id = ?".into(), vec![Sql::Integer(p)], &mut wh);
    }
    if !f.types.is_empty() {
        push(format!("t.type_id IN ({})", vec!["?"; f.types.len()].join(",")), f.types.iter().map(|i| Sql::Integer(*i)).collect(), &mut wh);
    }
    if !f.statuses.is_empty() {
        push(format!("t.status_id IN ({})", vec!["?"; f.statuses.len()].join(",")), f.statuses.iter().map(|i| Sql::Integer(*i)).collect(), &mut wh);
    }
    match f.parent {
        ParentFilter::Any => {}
        ParentFilter::Root => push("t.parent_id IS NULL".into(), vec![], &mut wh),
        ParentFilter::Of(id) => push("t.parent_id = ?".into(), vec![Sql::Integer(id)], &mut wh),
    }
    if let Some(s) = f.starred {
        push("t.starred = ?".into(), vec![Sql::Integer(s as i64)], &mut wh);
    }
    if let Some(s) = f.flagged {
        push("t.flagged = ?".into(), vec![Sql::Integer(s as i64)], &mut wh);
    }
    if let Some(text) = &f.text {
        for word in text.split_whitespace() {
            let pat = like_pattern(word);
            push(
                "(t.title LIKE ? ESCAPE '\\' OR (p.key || '-' || t.number) LIKE ? ESCAPE '\\' OR EXISTS (SELECT 1 FROM ticket_values v WHERE v.ticket_id = t.id AND v.text LIKE ? ESCAPE '\\' AND v.field_id IN (SELECT id FROM fields WHERE kind IN ('text', 'rich', 'labels'))))".into(),
                vec![Sql::Text(pat.clone()), Sql::Text(pat.clone()), Sql::Text(pat)],
                &mut wh,
            );
        }
    }
    let where_sql = if wh.is_empty() { String::new() } else { format!("WHERE {}", wh.join(" AND ")) };
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) {FROM} {where_sql}"), params_from_iter(args.iter()), |r| r.get(0))?;
    let mut sql = format!("SELECT {COLS} {FROM} {where_sql} ORDER BY {}", f.order.sql());
    let mut all_args = args.clone();
    if let Some(l) = f.limit {
        sql.push_str(" LIMIT ? OFFSET ?");
        all_args.push(Sql::Integer(l.max(0)));
        all_args.push(Sql::Integer(f.offset.max(0)));
    } else if f.offset > 0 {
        sql.push_str(" LIMIT -1 OFFSET ?");
        all_args.push(Sql::Integer(f.offset));
    }
    let mut st = conn.prepare(&sql)?;
    let items = st.query_map(params_from_iter(all_args.iter()), from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(TicketPage { items, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::history::Source;
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::{set_field, set_flagged, set_starred};
    use crate::planning::transition::transition_ticket;
    use serde_json::json;

    fn ui() -> Source {
        Source::ui()
    }

    fn keys(p: &TicketPage) -> Vec<String> {
        p.items.iter().map(|t| t.key()).collect()
    }

    #[test]
    fn lists_in_rank_order_and_filters_by_type_and_status() {
        let f = Fx::new();
        f.mk(f.epic, "Epic");
        f.mk(f.story, "Story");
        let t = f.mk(f.task, "Task");
        transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
        let all = list_tickets(&f.c, Some(f.project), &TicketFilter::default()).unwrap();
        assert_eq!((keys(&all), all.total), (vec!["PAN-1".to_string(), "PAN-2".into(), "PAN-3".into()], 3));
        let by_type = list_tickets(&f.c, Some(f.project), &TicketFilter { types: vec![f.epic, f.task], ..Default::default() }).unwrap();
        assert_eq!(keys(&by_type), vec!["PAN-1", "PAN-3"]);
        let by_status = list_tickets(&f.c, Some(f.project), &TicketFilter { statuses: vec![f.progress], ..Default::default() }).unwrap();
        assert_eq!(keys(&by_status), vec!["PAN-3"]);
        let none = list_tickets(&f.c, Some(f.project), &TicketFilter { statuses: vec![f.done], ..Default::default() }).unwrap();
        assert!(none.items.is_empty() && none.total == 0);
    }

    #[test]
    fn filters_by_parent() {
        let f = Fx::new();
        let e = f.mk(f.epic, "E");
        let s1 = f.child(&e, f.story, "S1");
        let _s2 = f.child(&e, f.story, "S2");
        let _t = f.child(&s1, f.task, "T");
        let roots = list_tickets(&f.c, Some(f.project), &TicketFilter { parent: ParentFilter::Root, ..Default::default() }).unwrap();
        assert_eq!(keys(&roots), vec!["PAN-1"]);
        let kids = list_tickets(&f.c, Some(f.project), &TicketFilter { parent: ParentFilter::Of(e.id), ..Default::default() }).unwrap();
        assert_eq!(keys(&kids), vec!["PAN-2", "PAN-3"]);
        let none = list_tickets(&f.c, Some(f.project), &TicketFilter { parent: ParentFilter::Of(9999), ..Default::default() }).unwrap();
        assert_eq!(none.total, 0);
    }

    #[test]
    fn filters_by_flag_and_star() {
        let f = Fx::new();
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let _c = f.mk(f.task, "C");
        set_starred(&f.c, a.id, true, &ui()).unwrap();
        set_flagged(&f.c, b.id, true, &ui()).unwrap();
        set_starred(&f.c, b.id, true, &ui()).unwrap();
        let q = |s: Option<bool>, fl: Option<bool>| keys(&list_tickets(&f.c, Some(f.project), &TicketFilter { starred: s, flagged: fl, ..Default::default() }).unwrap());
        assert_eq!(q(Some(true), None), vec!["PAN-1", "PAN-2"]);
        assert_eq!(q(None, Some(true)), vec!["PAN-2"]);
        assert_eq!(q(Some(true), Some(false)), vec!["PAN-1"]);
        assert_eq!(q(Some(false), None), vec!["PAN-3"]);
        assert_eq!(q(None, None).len(), 3);
    }

    #[test]
    fn text_search_covers_title_key_and_text_values() {
        let f = Fx::new();
        let desc = f.field(f.story, "description");
        let a = f.mk(f.story, "Login überarbeiten");
        let b = f.mk(f.story, "Export");
        set_field(&f.c, b.id, desc.id, json!("Die Anmeldung nutzt 100% neue_Seiten"), &ui()).unwrap();
        let labels = f.field(f.story, "labels");
        set_field(&f.c, a.id, labels.id, json!(["frontend", "ui"]), &ui()).unwrap();
        let q = |t: &str| keys(&list_tickets(&f.c, None, &TicketFilter { text: Some(t.into()), ..Default::default() }).unwrap());
        assert_eq!(q("login"), vec!["PAN-1"], "Groß-/Kleinschreibung egal");
        assert_eq!(q("pan-2"), vec!["PAN-2"], "Schlüssel");
        assert_eq!(q("anmeldung"), vec!["PAN-2"], "Beschreibung");
        assert_eq!(q("frontend"), vec!["PAN-1"], "Label");
        assert_eq!(q("100%"), vec!["PAN-2"], "Prozentzeichen ist kein Platzhalter");
        assert_eq!(q("neue_s"), vec!["PAN-2"]);
        assert!(q("neue%s").is_empty() && q("a_b").is_empty());
        assert_eq!(q("login ui"), vec!["PAN-1"], "alle Wörter müssen passen");
        assert!(q("login export").is_empty());
        assert_eq!(q("  ").len(), 2, "leerer Suchtext = kein Filter");
        // Verweise (Ticket-Kennungen als Text) zählen nicht als Text
        let epic_field = f.field(f.story, "epic");
        let e = f.mk(f.epic, "E");
        set_field(&f.c, a.id, epic_field.id, json!(e.id), &ui()).unwrap();
        assert!(q("zzzz").is_empty());
    }

    #[test]
    fn paging_and_order() {
        let f = Fx::new();
        for i in 0..7 {
            f.mk(f.task, &format!("T{}", ["d", "a", "c", "b", "g", "e", "f"][i]));
        }
        let page = |limit, offset, order| list_tickets(&f.c, Some(f.project), &TicketFilter { limit, offset, order, ..Default::default() }).unwrap();
        let p = page(Some(3), 0, TicketOrder::Rank);
        assert_eq!((keys(&p), p.total), (vec!["PAN-1".to_string(), "PAN-2".into(), "PAN-3".into()], 7));
        let p = page(Some(3), 3, TicketOrder::Rank);
        assert_eq!(keys(&p), vec!["PAN-4", "PAN-5", "PAN-6"]);
        let p = page(Some(3), 6, TicketOrder::Rank);
        assert_eq!((keys(&p), p.total), (vec!["PAN-7".to_string()], 7));
        assert!(page(Some(3), 99, TicketOrder::Rank).items.is_empty());
        assert_eq!(page(None, 5, TicketOrder::Rank).items.len(), 2, "Versatz ohne Grenze");
        let titles: Vec<String> = page(None, 0, TicketOrder::Title).items.iter().map(|t| t.title.clone()).collect();
        assert_eq!(titles, vec!["Ta", "Tb", "Tc", "Td", "Te", "Tf", "Tg"]);
        assert_eq!(keys(&page(Some(2), 0, TicketOrder::NumberDesc)), vec!["PAN-7", "PAN-6"]);
        assert_eq!(keys(&page(Some(2), 0, TicketOrder::NumberAsc)), vec!["PAN-1", "PAN-2"]);
        assert_eq!(page(Some(2), 0, TicketOrder::CreatedDesc).items[0].number, 7);
        assert_eq!(page(Some(0), 0, TicketOrder::Rank).items.len(), 0);
    }

    #[test]
    fn project_scope_and_empty_database() {
        let f = Fx::new();
        assert_eq!(list_tickets(&f.c, None, &TicketFilter::default()).unwrap().total, 0);
        f.mk(f.task, "A");
        let other = f.other_project("OTH");
        let ot = crate::planning::types::list_types(&f.c, other).unwrap()[0].id;
        crate::planning::tickets::create_ticket(&f.c, &crate::planning::tickets::NewTicket::new(other, ot, "B"), &ui()).unwrap();
        assert_eq!(list_tickets(&f.c, None, &TicketFilter::default()).unwrap().total, 2);
        assert_eq!(list_tickets(&f.c, Some(other), &TicketFilter::default()).unwrap().total, 1);
        assert_eq!(list_tickets(&f.c, Some(9999), &TicketFilter::default()).unwrap().total, 0);
    }

    #[test]
    fn stays_fast_with_thousands_of_tickets() {
        let f = Fx::new();
        f.c.execute_batch("BEGIN").unwrap();
        for i in 0..3000 {
            crate::planning::tickets::create_ticket(&f.c, &crate::planning::tickets::NewTicket::new(f.project, f.task, &format!("Ticket {i}")), &ui()).unwrap();
        }
        f.c.execute_batch("COMMIT").unwrap();
        let start = std::time::Instant::now();
        let p = list_tickets(&f.c, Some(f.project), &TicketFilter { limit: Some(50), offset: 2900, statuses: vec![f.todo], ..Default::default() }).unwrap();
        assert_eq!((p.items.len(), p.total), (50, 3000));
        let q = list_tickets(&f.c, Some(f.project), &TicketFilter { text: Some("Ticket 2999".into()), ..Default::default() }).unwrap();
        assert_eq!(q.total, 2, "Titel „Ticket 2999“ und der Schlüssel PAN-2999 des Tickets davor");
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
}
