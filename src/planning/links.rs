//! Verknüpfungen von Tickets: zu anderen Tickets, Terminal-Sitzungen, URLs und Git-Branches. Der Verknüpfungstyp
//! (`link_types.rs`) legt fest, welche Art Ziel er erlaubt; Ticket-Verknüpfungen haben eine Rückrichtung („blockiert“ /
//! „ist blockiert durch“) und erscheinen am Ziel-Ticket mit dem Gegennamen.
//!
//! * Ablage: `links` mit `to_kind` und `to_ticket_id` (Ticket) bzw. `to_text` (Terminal-Kennung, URL, Branch als `Repo\nName`).
//! * Keine Verknüpfung mit sich selbst, keine Duplikate (bei symmetrischen Typen auch nicht in Gegenrichtung).
//! * Der Verlauf bekommt einen Eintrag am Quell-Ticket und – bei Ticket-Zielen – am Ziel-Ticket.

use super::history::{self, link_json, HistoryField, Source};
use super::link_types::{get_link_type, TargetKind};
use super::ops::TicketOp;
use super::ticket_error::{atomic, TRes, TicketError};
use super::tickets::{get_ticket, Ticket};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};

/// Das andere Ende einer Verknüpfung.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Ticket(i64),
    /// Kennung der Terminal-Sitzung.
    Terminal(String),
    Url(String),
    Branch { repo: String, name: String },
}

impl Target {
    pub fn kind(&self) -> TargetKind {
        match self {
            Target::Ticket(_) => TargetKind::Ticket,
            Target::Terminal(_) => TargetKind::Terminal,
            Target::Url(_) => TargetKind::Url,
            Target::Branch { .. } => TargetKind::Branch,
        }
    }

    /// (`to_ticket_id`, `to_text`) wie gespeichert.
    fn parts(&self) -> (Option<i64>, Option<String>) {
        match self {
            Target::Ticket(id) => (Some(*id), None),
            Target::Terminal(s) | Target::Url(s) => (None, Some(s.clone())),
            Target::Branch { repo, name } => (None, Some(format!("{repo}\n{name}"))),
        }
    }

    fn from_parts(kind: TargetKind, ticket: Option<i64>, text: Option<String>) -> Target {
        let text = text.unwrap_or_default();
        match kind {
            TargetKind::Ticket => Target::Ticket(ticket.unwrap_or(0)),
            TargetKind::Terminal => Target::Terminal(text),
            TargetKind::Url => Target::Url(text),
            TargetKind::Branch => {
                let (repo, name) = text.split_once('\n').unwrap_or(("", &text));
                Target::Branch { repo: repo.to_string(), name: name.to_string() }
            }
        }
    }
}

/// Eine gespeicherte Verknüpfung.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRecord {
    pub id: i64,
    pub link_type_id: i64,
    pub from_ticket_id: i64,
    pub target: Target,
    pub created_at: String,
}

const COLS: &str = "id, link_type_id, from_ticket_id, to_kind, to_ticket_id, to_text, created_at";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<LinkRecord> {
    let kind = TargetKind::parse(&r.get::<_, String>(3)?);
    Ok(LinkRecord { id: r.get(0)?, link_type_id: r.get(1)?, from_ticket_id: r.get(2)?, target: Target::from_parts(kind, r.get(4)?, r.get(5)?), created_at: r.get(6)? })
}

pub fn get_link(conn: &Connection, id: i64) -> TRes<LinkRecord> {
    conn.query_row(&format!("SELECT {COLS} FROM links WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(TicketError::not_found)
}

/// Eine Terminal-Kennung, URL oder einen Branch bereinigen und prüfen.
pub(crate) fn clean_target(t: &Target) -> TRes<Target> {
    let bad = || TicketError::LinkTargetInvalid;
    Ok(match t {
        Target::Ticket(id) => Target::Ticket(*id),
        Target::Terminal(s) => {
            let s = s.trim();
            if s.is_empty() || s.chars().count() > 200 || s.contains('\n') {
                return Err(bad());
            }
            Target::Terminal(s.to_string())
        }
        Target::Url(s) => {
            let s = s.trim();
            if s.is_empty() || s.chars().count() > 2000 || s.chars().any(|c| c.is_whitespace() || c.is_control()) || !s.contains("://") || s.starts_with("://") || s.ends_with("://") {
                return Err(bad());
            }
            Target::Url(s.to_string())
        }
        Target::Branch { repo, name } => {
            let (repo, name) = (repo.trim(), name.trim());
            if name.is_empty() || name.chars().count() > 255 || name.chars().any(|c| c.is_whitespace() || c.is_control()) || repo.chars().count() > 500 || repo.chars().any(|c| c.is_control()) {
                return Err(bad());
            }
            Target::Branch { repo: repo.to_string(), name: name.to_string() }
        }
    })
}

/// Text des Ziels für den Verlauf (Ticket: Schlüssel).
fn target_text(conn: &Connection, t: &Target) -> String {
    match t {
        Target::Ticket(id) => get_ticket(conn, *id).map(|t| t.key()).unwrap_or_else(|_| id.to_string()),
        Target::Terminal(s) | Target::Url(s) => s.clone(),
        Target::Branch { repo, name } => format!("{repo}\n{name}"),
    }
}

/// Schreibt die Verknüpfung (mit fester Kennung, wenn `rec.id > 0`) samt Verlauf an beiden Tickets. Prüft nichts.
pub(crate) fn raw_insert(conn: &Connection, rec: &LinkRecord, source: &Source, batch: i64) -> TRes<i64> {
    let (to_ticket, to_text) = rec.target.parts();
    if rec.id > 0 {
        conn.execute(
            "INSERT INTO links (id, link_type_id, from_ticket_id, to_kind, to_ticket_id, to_text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![rec.id, rec.link_type_id, rec.from_ticket_id, rec.target.kind().as_str(), to_ticket, to_text, rec.created_at],
        )?;
    } else {
        conn.execute(
            "INSERT INTO links (link_type_id, from_ticket_id, to_kind, to_ticket_id, to_text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rec.link_type_id, rec.from_ticket_id, rec.target.kind().as_str(), to_ticket, to_text, rec.created_at],
        )?;
    }
    let id = if rec.id > 0 { rec.id } else { conn.last_insert_rowid() };
    log_link(conn, rec, source, batch, true)?;
    Ok(id)
}

/// Löscht die Verknüpfung samt Verlauf an beiden Tickets.
pub(crate) fn raw_delete(conn: &Connection, rec: &LinkRecord, source: &Source, batch: i64) -> TRes<()> {
    if conn.execute("DELETE FROM links WHERE id = ?1", [rec.id])? == 0 {
        return Err(TicketError::not_found());
    }
    log_link(conn, rec, source, batch, false)
}

fn log_link(conn: &Connection, rec: &LinkRecord, source: &Source, batch: i64, added: bool) -> TRes<()> {
    let kind = rec.target.kind().as_str();
    let out = link_json(rec.link_type_id, "out", kind, &target_text(conn, &rec.target));
    let (old, new) = if added { (None, Some(out)) } else { (Some(out), None) };
    history::log(conn, rec.from_ticket_id, source, batch, &HistoryField::Link, old.as_deref(), new.as_deref())?;
    super::tickets::touch(conn, rec.from_ticket_id)?;
    if let Target::Ticket(other) = &rec.target {
        if get_ticket(conn, *other).is_ok() {
            let from_key = get_ticket(conn, rec.from_ticket_id).map(|t| t.key()).unwrap_or_default();
            let inc = link_json(rec.link_type_id, "in", "ticket", &from_key);
            let (old, new) = if added { (None, Some(inc)) } else { (Some(inc), None) };
            history::log(conn, *other, source, batch, &HistoryField::Link, old.as_deref(), new.as_deref())?;
            super::tickets::touch(conn, *other)?;
        }
    }
    Ok(())
}

/// Legt eine Verknüpfung an. Der Verknüpfungstyp muss das Ziel erlauben (`LinkTargetMismatch`), ein Ticket nicht mit sich
/// selbst (`LinkSelf`), das Ziel-Ticket muss existieren (`LinkTargetMissing`), Duplikate sind verboten (`LinkDuplicate`,
/// bei symmetrischen Typen auch in Gegenrichtung).
pub fn add_link(conn: &Connection, from: i64, link_type_id: i64, target: Target, source: &Source) -> TRes<(i64, TicketOp)> {
    atomic(conn, || {
        get_ticket(conn, from)?;
        let lt = get_link_type(conn, link_type_id)?;
        if target.kind() != lt.target_kind {
            return Err(TicketError::LinkTargetMismatch);
        }
        let target = clean_target(&target)?;
        let (to_ticket, to_text) = target.parts();
        if let Some(to) = to_ticket {
            if to == from {
                return Err(TicketError::LinkSelf);
            }
            if get_ticket(conn, to).is_err() {
                return Err(TicketError::LinkTargetMissing);
            }
        }
        let exists = |a: i64, b: Option<i64>, text: &Option<String>| -> TRes<bool> {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM links WHERE link_type_id = ?1 AND from_ticket_id = ?2 AND to_kind = ?3 AND to_ticket_id IS ?4 AND to_text IS ?5",
                params![link_type_id, a, target.kind().as_str(), b, text],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        };
        if exists(from, to_ticket, &to_text)? || (lt.symmetric && to_ticket.is_some_and(|to| exists(to, Some(from), &None).unwrap_or(false))) {
            return Err(TicketError::LinkDuplicate);
        }
        let batch = history::next_batch(conn)?;
        let rec = LinkRecord { id: 0, link_type_id, from_ticket_id: from, target, created_at: super::common::now() };
        let id = raw_insert(conn, &rec, source, batch)?;
        let rec = LinkRecord { id, ..rec };
        Ok((id, TicketOp::Link { link: rec, present: true, source: source.clone() }))
    })
}

/// Entfernt eine Verknüpfung (an beiden Enden). Die Operation macht es rückgängig.
pub fn remove_link(conn: &Connection, link_id: i64, source: &Source) -> TRes<TicketOp> {
    atomic(conn, || {
        let rec = get_link(conn, link_id)?;
        let batch = history::next_batch(conn)?;
        raw_delete(conn, &rec, source, batch)?;
        Ok(TicketOp::Link { link: rec, present: false, source: source.clone() })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    /// Das Ticket ist die Quelle („blockiert“).
    Out,
    /// Das Ticket ist das Ziel („ist blockiert durch“).
    In,
}

/// Kurzverweis auf ein Ticket.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketRef {
    pub id: i64,
    pub key: String,
    pub title: String,
}

impl TicketRef {
    fn of(t: &Ticket) -> Self {
        TicketRef { id: t.id, key: t.key(), title: t.title.clone() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    Ticket(TicketRef),
    Terminal(String),
    Url(String),
    Branch { repo: String, name: String },
}

/// Eine Verknüpfung aus Sicht eines Tickets.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkView {
    pub id: i64,
    pub link_type_id: i64,
    /// Bezeichnung in der Sprache, schon aus Sicht des Tickets (Hin- oder Gegenname).
    pub name: String,
    pub direction: Direction,
    pub target: LinkTarget,
    pub created_at: String,
}

/// Alle Verknüpfungen des Tickets – ausgehende mit dem Namen der Hinrichtung, eingehende mit dem Gegennamen –, sortiert nach
/// Verknüpfungstyp, dann Richtung, dann Anlegen.
pub fn links_of(conn: &Connection, ticket_id: i64, lang: Lang) -> TRes<Vec<LinkView>> {
    get_ticket(conn, ticket_id)?;
    let mut rows: Vec<(i64, Direction, LinkRecord)> = Vec::new();
    {
        let mut st = conn.prepare(&format!("SELECT {COLS} FROM links WHERE from_ticket_id = ?1 ORDER BY id"))?;
        for r in st.query_map([ticket_id], from_row)? {
            let r = r?;
            rows.push((r.id, Direction::Out, r));
        }
        let mut st = conn.prepare(&format!("SELECT {COLS} FROM links WHERE to_ticket_id = ?1 ORDER BY id"))?;
        for r in st.query_map([ticket_id], from_row)? {
            let r = r?;
            rows.push((r.id, Direction::In, r));
        }
    }
    let mut out: Vec<(i64, LinkView)> = Vec::new();
    for (_, dir, rec) in rows {
        let lt = get_link_type(conn, rec.link_type_id)?;
        let (name_out, name_in) = lt.names(lang);
        let (name, target) = match dir {
            Direction::Out => (
                name_out,
                match &rec.target {
                    Target::Ticket(id) => LinkTarget::Ticket(TicketRef::of(&get_ticket(conn, *id)?)),
                    Target::Terminal(s) => LinkTarget::Terminal(s.clone()),
                    Target::Url(s) => LinkTarget::Url(s.clone()),
                    Target::Branch { repo, name } => LinkTarget::Branch { repo: repo.clone(), name: name.clone() },
                },
            ),
            Direction::In => (name_in.unwrap_or(name_out), LinkTarget::Ticket(TicketRef::of(&get_ticket(conn, rec.from_ticket_id)?))),
        };
        out.push((lt.position, LinkView { id: rec.id, link_type_id: rec.link_type_id, name, direction: dir, target, created_at: rec.created_at }));
    }
    out.sort_by_key(|(pos, v)| (*pos, v.direction, v.id));
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

/// Kennungen der Terminal-Sitzungen, die mit dem Ticket verknüpft sind (in der Reihenfolge des Verknüpfens, ohne Doppelte).
pub fn linked_terminals(conn: &Connection, ticket_id: i64) -> TRes<Vec<String>> {
    let mut st = conn.prepare("SELECT to_text FROM links WHERE from_ticket_id = ?1 AND to_kind = 'terminal' ORDER BY id")?;
    let all: Vec<String> = st.query_map([ticket_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut out: Vec<String> = Vec::new();
    for t in all {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

/// Tickets, die mit dieser Terminal-Sitzung verknüpft sind (nach Ticket-Kennung, ohne Doppelte).
pub fn tickets_linked_to_terminal(conn: &Connection, terminal_id: &str) -> TRes<Vec<TicketRef>> {
    let ids: Vec<i64> = {
        let mut st = conn.prepare("SELECT DISTINCT from_ticket_id FROM links WHERE to_kind = 'terminal' AND to_text = ?1 ORDER BY from_ticket_id")?;
        st.query_map([terminal_id], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    ids.into_iter().map(|id| Ok(TicketRef::of(&get_ticket(conn, id)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::history::{history_of, SourceKind};
    use crate::planning::link_types::{create_link_type, link_type_by_key, NewLinkType};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::delete_ticket;

    fn ui() -> Source {
        Source::ui()
    }

    fn lt(f: &Fx, key: &str) -> i64 {
        link_type_by_key(&f.c, key).unwrap().id
    }

    #[test]
    fn links_two_tickets_with_names_from_both_sides() {
        let f = Fx::new();
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let (id, _) = add_link(&f.c, a.id, lt(&f, "blocks"), Target::Ticket(b.id), &ui()).unwrap();
        let va = links_of(&f.c, a.id, Lang::En).unwrap();
        assert_eq!(va.len(), 1);
        assert_eq!((va[0].name.as_str(), va[0].direction), ("blocks", Direction::Out));
        assert!(matches!(&va[0].target, LinkTarget::Ticket(t) if t.key == "PAN-2"));
        let vb = links_of(&f.c, b.id, Lang::En).unwrap();
        assert_eq!((vb[0].name.as_str(), vb[0].direction), ("is blocked by", Direction::In));
        assert!(matches!(&vb[0].target, LinkTarget::Ticket(t) if t.id == a.id));
        assert_eq!(links_of(&f.c, b.id, Lang::De).unwrap()[0].name, "ist blockiert durch");
        assert_eq!(links_of(&f.c, a.id, Lang::De).unwrap()[0].name, "blockiert");
        assert_eq!(vb[0].id, id);
        // symmetrisch: beide Seiten gleicher Name
        add_link(&f.c, a.id, lt(&f, "relates"), Target::Ticket(b.id), &ui()).unwrap();
        let vb = links_of(&f.c, b.id, Lang::En).unwrap();
        assert!(vb.iter().any(|v| v.name == "relates to" && v.direction == Direction::In));
        // Duplikat-Namen: „duplicates“
        add_link(&f.c, b.id, lt(&f, "duplicates"), Target::Ticket(a.id), &ui()).unwrap();
        let va = links_of(&f.c, a.id, Lang::De).unwrap();
        assert!(va.iter().any(|v| v.name == "hat Duplikat" && v.direction == Direction::In));
        let vb = links_of(&f.c, b.id, Lang::De).unwrap();
        assert!(vb.iter().any(|v| v.name == "ist Duplikat von" && v.direction == Direction::Out));
    }

    #[test]
    fn validates_target_kind_self_missing_and_duplicates() {
        let f = Fx::new();
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let (blocks, relates, term) = (lt(&f, "blocks"), lt(&f, "relates"), lt(&f, "terminal"));
        assert_eq!(add_link(&f.c, a.id, blocks, Target::Url("https://x.y".into()), &ui()).unwrap_err(), TicketError::LinkTargetMismatch);
        assert_eq!(add_link(&f.c, a.id, term, Target::Ticket(b.id), &ui()).unwrap_err(), TicketError::LinkTargetMismatch, "terminal nur für Terminals");
        assert_eq!(add_link(&f.c, a.id, blocks, Target::Ticket(a.id), &ui()).unwrap_err(), TicketError::LinkSelf);
        assert_eq!(add_link(&f.c, a.id, blocks, Target::Ticket(9999), &ui()).unwrap_err(), TicketError::LinkTargetMissing);
        assert!(add_link(&f.c, 9999, blocks, Target::Ticket(b.id), &ui()).unwrap_err().is_not_found());
        assert!(add_link(&f.c, a.id, 9999, Target::Ticket(b.id), &ui()).unwrap_err().is_not_found());
        add_link(&f.c, a.id, blocks, Target::Ticket(b.id), &ui()).unwrap();
        assert_eq!(add_link(&f.c, a.id, blocks, Target::Ticket(b.id), &ui()).unwrap_err(), TicketError::LinkDuplicate);
        // gerichtet: Gegenrichtung ist ein anderer Eintrag und erlaubt
        assert!(add_link(&f.c, b.id, blocks, Target::Ticket(a.id), &ui()).is_ok());
        // symmetrisch: Gegenrichtung ist ein Duplikat
        add_link(&f.c, a.id, relates, Target::Ticket(b.id), &ui()).unwrap();
        assert_eq!(add_link(&f.c, b.id, relates, Target::Ticket(a.id), &ui()).unwrap_err(), TicketError::LinkDuplicate);
        // anderer Typ, gleiches Paar ist kein Duplikat
        assert!(add_link(&f.c, a.id, lt(&f, "duplicates"), Target::Ticket(b.id), &ui()).is_ok());
    }

    #[test]
    fn terminal_url_and_branch_targets() {
        let f = Fx::new();
        let a = f.mk(f.task, "A");
        let term = lt(&f, "terminal");
        let url = create_link_type(&f.c, &NewLinkType { name_out: "Doku".into(), name_in: None, symmetric: false, target_kind: TargetKind::Url }).unwrap();
        let br = create_link_type(&f.c, &NewLinkType { name_out: "Branch".into(), name_in: None, symmetric: false, target_kind: TargetKind::Branch }).unwrap();
        add_link(&f.c, a.id, term, Target::Terminal(" sess-1 ".into()), &ui()).unwrap();
        add_link(&f.c, a.id, url, Target::Url("https://example.com/a".into()), &ui()).unwrap();
        add_link(&f.c, a.id, br, Target::Branch { repo: "E:\\repo".into(), name: "feature/x".into() }, &ui()).unwrap();
        let v = links_of(&f.c, a.id, Lang::En).unwrap();
        assert_eq!(v.len(), 3);
        assert!(v.iter().any(|x| x.target == LinkTarget::Terminal("sess-1".into()) && x.name == "Terminal"));
        assert!(v.iter().any(|x| x.target == LinkTarget::Url("https://example.com/a".into())));
        assert!(v.iter().any(|x| x.target == LinkTarget::Branch { repo: "E:\\repo".into(), name: "feature/x".into() }));
        // Duplikate auch bei Text-Zielen
        assert_eq!(add_link(&f.c, a.id, term, Target::Terminal("sess-1".into()), &ui()).unwrap_err(), TicketError::LinkDuplicate);
        assert_eq!(add_link(&f.c, a.id, br, Target::Branch { repo: "E:\\repo".into(), name: "feature/x".into() }, &ui()).unwrap_err(), TicketError::LinkDuplicate);
        assert!(add_link(&f.c, a.id, br, Target::Branch { repo: "E:\\other".into(), name: "feature/x".into() }, &ui()).is_ok());
        // ungültige Ziele
        for bad in [Target::Terminal("  ".into()), Target::Terminal("a\nb".into())] {
            assert_eq!(add_link(&f.c, a.id, term, bad, &ui()).unwrap_err(), TicketError::LinkTargetInvalid);
        }
        for bad in ["", "example.com", "http://a b", "://x", "https://"] {
            assert_eq!(add_link(&f.c, a.id, url, Target::Url(bad.into()), &ui()).unwrap_err(), TicketError::LinkTargetInvalid, "{bad}");
        }
        for bad in [("r", ""), ("r", "a b"), ("r", "a\nb")] {
            assert_eq!(add_link(&f.c, a.id, br, Target::Branch { repo: bad.0.into(), name: bad.1.into() }, &ui()).unwrap_err(), TicketError::LinkTargetInvalid, "{bad:?}");
        }
        // Branch ohne Repo ist erlaubt
        assert!(add_link(&f.c, a.id, br, Target::Branch { repo: "".into(), name: "main".into() }, &ui()).is_ok());
    }

    #[test]
    fn terminal_lookups_work_in_both_directions() {
        let f = Fx::new();
        let (a, b) = (f.mk(f.task, "A"), f.mk(f.task, "B"));
        let term = lt(&f, "terminal");
        add_link(&f.c, a.id, term, Target::Terminal("s1".into()), &ui()).unwrap();
        add_link(&f.c, a.id, term, Target::Terminal("s2".into()), &ui()).unwrap();
        add_link(&f.c, b.id, term, Target::Terminal("s1".into()), &ui()).unwrap();
        assert_eq!(linked_terminals(&f.c, a.id).unwrap(), vec!["s1", "s2"]);
        assert_eq!(linked_terminals(&f.c, b.id).unwrap(), vec!["s1"]);
        let r = tickets_linked_to_terminal(&f.c, "s1").unwrap();
        assert_eq!(r.iter().map(|t| t.key.as_str()).collect::<Vec<_>>(), vec!["PAN-1", "PAN-2"]);
        assert_eq!(r[0].title, "A");
        assert!(tickets_linked_to_terminal(&f.c, "s9").unwrap().is_empty());
        assert!(linked_terminals(&f.c, 9999).unwrap().is_empty());
        // Terminal-Links zählen nicht als Ticket-Links des Gegenübers
        assert!(links_of(&f.c, b.id, Lang::En).unwrap().iter().all(|v| v.direction == Direction::Out));
    }

    #[test]
    fn removing_a_link_removes_both_ends_and_logs() {
        let f = Fx::new();
        let (a, b) = (f.mk(f.task, "A"), f.mk(f.task, "B"));
        let (id, _) = add_link(&f.c, a.id, lt(&f, "blocks"), Target::Ticket(b.id), &Source::agent("S")).unwrap();
        remove_link(&f.c, id, &ui()).unwrap();
        assert!(links_of(&f.c, a.id, Lang::En).unwrap().is_empty());
        assert!(links_of(&f.c, b.id, Lang::En).unwrap().is_empty());
        assert!(remove_link(&f.c, id, &ui()).unwrap_err().is_not_found());
        for t in [a.id, b.id] {
            let h: Vec<_> = history_of(&f.c, t).unwrap().into_iter().filter(|e| e.field == HistoryField::Link).collect();
            assert_eq!(h.len(), 2, "hinzugefügt und entfernt, an beiden Tickets");
            assert!(h[0].old.is_none() && h[0].new.is_some());
            assert!(h[1].old.is_some() && h[1].new.is_none());
            assert_eq!(h[0].source.kind, SourceKind::Agent);
        }
        // Gegenrichtung steht mit „in“ im Verlauf von B
        let hb = history_of(&f.c, b.id).unwrap();
        let e = hb.iter().find(|e| e.field == HistoryField::Link).unwrap();
        assert!(e.new.as_ref().unwrap().contains("\"dir\":\"in\""));
        let text = crate::planning::history::describe(&f.c, e, Lang::De);
        assert!(text.contains("ist blockiert durch") && text.contains("PAN-1"), "{text}");
    }

    #[test]
    fn deleting_a_ticket_removes_links_on_both_sides() {
        let f = Fx::new();
        let (a, b, c) = (f.mk(f.task, "A"), f.mk(f.task, "B"), f.mk(f.task, "C"));
        add_link(&f.c, a.id, lt(&f, "blocks"), Target::Ticket(b.id), &ui()).unwrap();
        add_link(&f.c, c.id, lt(&f, "relates"), Target::Ticket(b.id), &ui()).unwrap();
        delete_ticket(&f.c, b.id, &ui()).unwrap();
        assert!(links_of(&f.c, a.id, Lang::En).unwrap().is_empty());
        assert!(links_of(&f.c, c.id, Lang::En).unwrap().is_empty());
    }

    #[test]
    fn links_are_sorted_by_type_then_direction() {
        let f = Fx::new();
        let (a, b, c) = (f.mk(f.task, "A"), f.mk(f.task, "B"), f.mk(f.task, "C"));
        add_link(&f.c, a.id, lt(&f, "relates"), Target::Ticket(b.id), &ui()).unwrap();
        add_link(&f.c, c.id, lt(&f, "blocks"), Target::Ticket(a.id), &ui()).unwrap();
        add_link(&f.c, a.id, lt(&f, "blocks"), Target::Ticket(b.id), &ui()).unwrap();
        let v: Vec<_> = links_of(&f.c, a.id, Lang::En).unwrap().into_iter().map(|v| (v.name, v.direction)).collect();
        assert_eq!(v, vec![("blocks".to_string(), Direction::Out), ("is blocked by".to_string(), Direction::In), ("relates to".to_string(), Direction::Out)]);
    }
}
