//! Zugriff der Agenten, private Einträge, Herkunft und Papierkorb (#386, Migration 13).
//!
//! * Ticket-Projekte: `agent_level`/`agent_for_all` (bestehende: wie global + für alle, neue: gesperrt) und optional
//!   `sidebar_project` (verknüpftes Seitenleisten-Projekt, dann gilt dessen Zugriff); `origin` = von wem angelegt.
//! * Tickets: `private` (nie für Agenten), `origin`/`origin_detail` (Nutzer, Agent mit Sitzungs-/Modellname, Paneldeck),
//!   Papierkorb `trashed_at`/`trashed_by`/`trashed_was_archived`. Ein Ticket im Papierkorb ist zugleich archiviert (so fällt
//!   es überall heraus, wo archivierte Tickets fehlen); Wiederherstellen setzt die Archiv-Markierung zurück.
//! * Kommentare: `origin`/`origin_detail` und Papierkorb wie Tickets.
//! * Todo-Karten: `project` (Seitenleisten-Projekt, `NULL` = „Allgemein“), `private`, `origin`/`origin_detail`;
//!   Tags: `private` (alle Karten mit dem Tag sind privat).
//!
//! Nur anhängen: diese Migration nach dem Erscheinen nie ändern.

use super::history::{self, HistoryField, Source, SourceKind};
use super::ticket_error::{atomic, TRes, TicketError};
use crate::access::{AgentAccess, AreaLevel, TicketAccess};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

pub const MIGRATION: &str = "
ALTER TABLE projects ADD COLUMN agent_level TEXT NOT NULL DEFAULT 'global';
ALTER TABLE projects ADD COLUMN agent_for_all INTEGER NOT NULL DEFAULT 1;
ALTER TABLE projects ADD COLUMN sidebar_project INTEGER;
ALTER TABLE projects ADD COLUMN origin TEXT NOT NULL DEFAULT 'user';
ALTER TABLE projects ADD COLUMN origin_detail TEXT;
ALTER TABLE tickets ADD COLUMN private INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tickets ADD COLUMN origin TEXT NOT NULL DEFAULT 'user';
ALTER TABLE tickets ADD COLUMN origin_detail TEXT;
ALTER TABLE tickets ADD COLUMN trashed_at TEXT;
ALTER TABLE tickets ADD COLUMN trashed_by TEXT;
ALTER TABLE tickets ADD COLUMN trashed_was_archived INTEGER NOT NULL DEFAULT 0;
CREATE INDEX tickets_trashed ON tickets (trashed_at) WHERE trashed_at IS NOT NULL;
ALTER TABLE comments ADD COLUMN origin TEXT NOT NULL DEFAULT 'user';
ALTER TABLE comments ADD COLUMN origin_detail TEXT;
ALTER TABLE comments ADD COLUMN trashed_at TEXT;
ALTER TABLE comments ADD COLUMN trashed_by TEXT;
ALTER TABLE todo_items ADD COLUMN project INTEGER;
ALTER TABLE todo_items ADD COLUMN private INTEGER NOT NULL DEFAULT 0;
ALTER TABLE todo_items ADD COLUMN origin TEXT NOT NULL DEFAULT 'user';
ALTER TABLE todo_items ADD COLUMN origin_detail TEXT;
ALTER TABLE todo_tags ADD COLUMN private INTEGER NOT NULL DEFAULT 0;
";

/// Herkunft einer Karte, eines Tickets, Kommentars oder Ticket-Projekts. Agent und Paneldeck = Systemnotiz.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Origin {
    #[default]
    User,
    Agent,
    Paneldeck,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Agent => "agent",
            Origin::Paneldeck => "paneldeck",
        }
    }

    pub fn parse(s: &str) -> Origin {
        match s {
            "agent" => Origin::Agent,
            "paneldeck" => Origin::Paneldeck,
            _ => Origin::User,
        }
    }

    /// Aus der Quelle einer Änderung: ein Agent ist „Agent“, alles andere (Oberfläche, Schnellmenü, Import) der Nutzer.
    pub fn of(source: &Source) -> Origin {
        if source.kind == SourceKind::Agent { Origin::Agent } else { Origin::User }
    }

    /// Systemnotiz (Badge in den Listen)?
    pub fn is_system(self) -> bool {
        self != Origin::User
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Origin::User => lang.t("Nutzer", "User"),
            Origin::Agent => lang.t("Agent", "Agent"),
            Origin::Paneldeck => "Paneldeck",
        }
    }
}

/// Zusatz zur Herkunft aus der Quelle (Name der Agenten-Sitzung bzw. des Modells).
pub fn origin_detail(source: &Source) -> Option<String> {
    if source.kind == SourceKind::Agent { source.detail.clone() } else { None }
}

// ------------------------------------------------------------------ Ticket-Projekte

fn access_of(level: &str, for_all: i64) -> AgentAccess {
    AgentAccess { level: AreaLevel::parse(level), for_all: for_all != 0 }
}

/// Zugriff aller Ticket-Projekte (für `access::AccessMap`).
pub fn ticket_access(conn: &Connection) -> rusqlite::Result<HashMap<i64, TicketAccess>> {
    let mut st = conn.prepare("SELECT id, agent_level, agent_for_all, sidebar_project, system FROM projects")?;
    let rows = st.query_map([], |r| {
        let link: Option<i64> = r.get(3)?;
        Ok((r.get::<_, i64>(0)?, TicketAccess { own: access_of(&r.get::<_, String>(1)?, r.get(2)?), link: link.and_then(|l| u64::try_from(l).ok()), system: r.get::<_, i64>(4)? != 0 }))
    })?;
    rows.collect()
}

/// Zugriff und Verknüpfung eines Ticket-Projekts.
pub fn project_access(conn: &Connection, id: i64) -> TRes<(AgentAccess, Option<u64>)> {
    conn.query_row("SELECT agent_level, agent_for_all, sidebar_project FROM projects WHERE id = ?1", [id], |r| {
        let link: Option<i64> = r.get(2)?;
        Ok((access_of(&r.get::<_, String>(0)?, r.get(1)?), link.and_then(|l| u64::try_from(l).ok())))
    })
    .optional()?
    .ok_or_else(TicketError::not_found)
}

/// Zugriff und Verknüpfung setzen (nur der Nutzer über die Oberfläche).
pub fn set_project_access(conn: &Connection, id: i64, acc: AgentAccess, link: Option<u64>) -> TRes<()> {
    let n = conn.execute(
        "UPDATE projects SET agent_level = ?1, agent_for_all = ?2, sidebar_project = ?3 WHERE id = ?4",
        params![acc.level.as_str(), acc.for_all as i64, link.map(|l| l as i64), id],
    )?;
    if n == 0 {
        return Err(TicketError::not_found());
    }
    Ok(())
}

/// Ein Seitenleisten-Projekt wurde gelöscht und seine Sitzungen sind nach `to` gezogen: verknüpfte Ticket-Projekte und
/// Todo-Karten ziehen mit (es gilt sofort der Zugriff des Ziels).
pub fn sidebar_project_moved(conn: &Connection, from: u64, to: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE projects SET sidebar_project = ?2 WHERE sidebar_project = ?1", params![from as i64, to as i64])?;
    conn.execute("UPDATE todo_items SET project = ?2 WHERE project = ?1", params![from as i64, to as i64])?;
    Ok(())
}

/// Herkunft eines Ticket-Projekts (Badge „von einem Agenten angelegt“).
pub fn project_origin(conn: &Connection, id: i64) -> TRes<(Origin, Option<String>)> {
    conn.query_row("SELECT origin, origin_detail FROM projects WHERE id = ?1", [id], |r| Ok((Origin::parse(&r.get::<_, String>(0)?), r.get(1)?)))
        .optional()?
        .ok_or_else(TicketError::not_found)
}

pub(crate) fn set_project_origin(conn: &Connection, id: i64, origin: Origin, detail: Option<&str>) -> rusqlite::Result<()> {
    conn.execute("UPDATE projects SET origin = ?1, origin_detail = ?2 WHERE id = ?3", params![origin.as_str(), detail, id])?;
    Ok(())
}

/// Neues Ticket-Projekt: für Agenten gesperrt (Entscheidung „klein nach groß“).
pub(crate) fn lock_new_project(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    match conn.execute("UPDATE projects SET agent_level = 'off', agent_for_all = 0 WHERE id = ?1 AND system = 0", [id]) {
        // Ältere Schema-Stände (nur in den Migrations-Tests): die Spalten kommen erst mit Migration 13 – dort gilt dann ihr Standard.
        Err(e) if e.to_string().contains("no such column") => Ok(()),
        r => r.map(|_| ()),
    }
}

// ------------------------------------------------------------------ Tickets

/// Herkunft und Zusatz eines neuen Tickets aus der Quelle schreiben (in `create_ticket`).
pub(crate) fn note_ticket_origin(conn: &Connection, id: i64, source: &Source) -> rusqlite::Result<()> {
    let o = Origin::of(source);
    if o != Origin::User {
        conn.execute("UPDATE tickets SET origin = ?1, origin_detail = ?2 WHERE id = ?3", params![o.as_str(), origin_detail(source), id])?;
    }
    Ok(())
}

/// Privat-Schalter eines Tickets (nur der Nutzer). Gibt zurück, ob sich etwas geändert hat; Verlaufseintrag `private`.
pub fn set_ticket_private(conn: &Connection, id: i64, private: bool, source: &Source) -> TRes<bool> {
    atomic(conn, || {
        let cur: i64 = conn.query_row("SELECT private FROM tickets WHERE id = ?1", [id], |r| r.get(0)).optional()?.ok_or_else(TicketError::not_found)?;
        if (cur != 0) == private {
            return Ok(false);
        }
        conn.execute("UPDATE tickets SET private = ?1 WHERE id = ?2", params![private as i64, id])?;
        let batch = history::next_batch(conn)?;
        history::log(conn, id, source, batch, &HistoryField::Other("private".into()), Some(if cur != 0 { "1" } else { "0" }), Some(if private { "1" } else { "0" }))?;
        Ok(true)
    })
}

/// Ticket in den Papierkorb (wiederherstellbar). `source` steht im Verlauf und als `trashed_by`.
pub fn trash_ticket(conn: &Connection, id: i64, source: &Source) -> TRes<()> {
    atomic(conn, || {
        let (archived, trashed): (i64, Option<String>) = conn.query_row("SELECT archived, trashed_at FROM tickets WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?.ok_or_else(TicketError::not_found)?;
        if trashed.is_some() {
            return Err(TicketError::not_found());
        }
        conn.execute(
            "UPDATE tickets SET trashed_at = ?1, trashed_by = ?2, trashed_was_archived = ?3, archived = 1 WHERE id = ?4",
            params![super::common::now(), source.as_label_text(), archived, id],
        )?;
        let batch = history::next_batch(conn)?;
        history::log(conn, id, source, batch, &HistoryField::Other("trashed".into()), None, Some("1"))
    })
}

/// Ticket aus dem Papierkorb zurückholen (nur der Nutzer).
pub fn restore_ticket(conn: &Connection, id: i64, source: &Source) -> TRes<()> {
    atomic(conn, || {
        let n = conn.execute("UPDATE tickets SET archived = trashed_was_archived, trashed_at = NULL, trashed_by = NULL, trashed_was_archived = 0 WHERE id = ?1 AND trashed_at IS NOT NULL", [id])?;
        if n == 0 {
            return Err(TicketError::not_found());
        }
        let batch = history::next_batch(conn)?;
        history::log(conn, id, source, batch, &HistoryField::Other("trashed".into()), Some("1"), None)
    })
}

pub fn is_trashed(conn: &Connection, id: i64) -> bool {
    conn.query_row("SELECT trashed_at IS NOT NULL FROM tickets WHERE id = ?1", [id], |r| r.get::<_, bool>(0)).optional().ok().flatten().unwrap_or(false)
}

/// Ein Eintrag im Papierkorb.
#[derive(Debug, Clone, PartialEq)]
pub struct TrashEntry {
    /// Ticket-Kennung (bei Kommentaren das Ticket des Kommentars).
    pub ticket: i64,
    /// Kommentar-Kennung (`None` = das Ticket selbst).
    pub comment: Option<i64>,
    pub key: String,
    /// Titel des Tickets bzw. Auszug des Kommentars.
    pub text: String,
    pub at: String,
    pub by: String,
}

/// Alles im Papierkorb, jüngstes zuerst.
pub fn trash(conn: &Connection) -> TRes<Vec<TrashEntry>> {
    let mut out = Vec::new();
    let mut st = conn.prepare("SELECT t.id, p.key || '-' || t.number, t.title, t.trashed_at, COALESCE(t.trashed_by, '') FROM tickets t JOIN projects p ON p.id = t.project_id WHERE t.trashed_at IS NOT NULL")?;
    for r in st.query_map([], |r| Ok(TrashEntry { ticket: r.get(0)?, comment: None, key: r.get(1)?, text: r.get(2)?, at: r.get(3)?, by: r.get(4)? }))? {
        out.push(r?);
    }
    let mut st = conn.prepare(
        "SELECT c.ticket_id, c.id, p.key || '-' || t.number, substr(c.body, 1, 120), c.trashed_at, COALESCE(c.trashed_by, '') FROM comments c JOIN tickets t ON t.id = c.ticket_id JOIN projects p ON p.id = t.project_id WHERE c.trashed_at IS NOT NULL",
    )?;
    for r in st.query_map([], |r| Ok(TrashEntry { ticket: r.get(0)?, comment: Some(r.get(1)?), key: r.get(2)?, text: r.get(3)?, at: r.get(4)?, by: r.get(5)? }))? {
        out.push(r?);
    }
    out.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(out)
}

/// Endgültig löschen (nur der Nutzer): ein Eintrag des Papierkorbs.
pub fn purge(conn: &Connection, e: &TrashEntry, source: &Source) -> TRes<()> {
    match e.comment {
        Some(c) => super::comments::delete_comment(conn, c, source),
        None => super::tickets::delete_ticket(conn, e.ticket, source).map(|_| ()),
    }
}

/// Papierkorb leeren (nur der Nutzer). Gibt die Zahl der gelöschten Einträge zurück.
pub fn empty_trash(conn: &Connection, source: &Source) -> TRes<usize> {
    atomic(conn, || {
        let all = trash(conn)?;
        let mut n = 0;
        // Kommentare zuerst (das Ticket eines Kommentars kann ebenfalls im Papierkorb liegen).
        for e in all.iter().filter(|e| e.comment.is_some()) {
            purge(conn, e, source)?;
            n += 1;
        }
        for e in all.iter().filter(|e| e.comment.is_none()) {
            purge(conn, e, source)?;
            n += 1;
        }
        Ok(n)
    })
}

/// Zurückholen (Ticket oder Kommentar).
pub fn restore(conn: &Connection, e: &TrashEntry, source: &Source) -> TRes<()> {
    match e.comment {
        Some(c) => restore_comment(conn, c, source),
        None => restore_ticket(conn, e.ticket, source),
    }
}

// ------------------------------------------------------------------ Kommentare

/// Kommentar in den Papierkorb (Verlaufseintrag wie beim Löschen, mit Zusatz).
pub fn trash_comment(conn: &Connection, id: i64, source: &Source) -> TRes<()> {
    atomic(conn, || {
        let c = super::comments::get_comment(conn, id)?;
        let n = conn.execute("UPDATE comments SET trashed_at = ?1, trashed_by = ?2 WHERE id = ?3 AND trashed_at IS NULL", params![super::common::now(), source.as_label_text(), id])?;
        if n == 0 {
            return Err(TicketError::not_found());
        }
        let batch = history::next_batch(conn)?;
        history::log(conn, c.ticket_id, source, batch, &HistoryField::Other("comment_trashed".into()), Some(&c.body.chars().take(200).collect::<String>()), None)
    })
}

pub fn restore_comment(conn: &Connection, id: i64, source: &Source) -> TRes<()> {
    atomic(conn, || {
        let ticket: i64 = conn.query_row("SELECT ticket_id FROM comments WHERE id = ?1 AND trashed_at IS NOT NULL", [id], |r| r.get(0)).optional()?.ok_or_else(TicketError::not_found)?;
        conn.execute("UPDATE comments SET trashed_at = NULL, trashed_by = NULL WHERE id = ?1", [id])?;
        let batch = history::next_batch(conn)?;
        history::log(conn, ticket, source, batch, &HistoryField::Other("comment_restored".into()), None, None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::comments::{add_comment, comments_of};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::get_ticket;

    #[test]
    fn existing_projects_keep_their_access_and_new_ones_start_locked() {
        let f = Fx::new();
        let m = ticket_access(&f.c).unwrap();
        // Fixture-Projekt (wie ein bestehendes) und System-Projekt TODO: wie global, für alle
        let todo = m.values().find(|t| t.system).expect("TODO");
        assert_eq!(todo.own, AgentAccess::AS_BEFORE);
        let id = crate::planning::projects::create_project(&f.c, &crate::planning::projects::NewProject::new("NEW", "Neu", "#336699")).unwrap();
        assert_eq!(project_access(&f.c, id).unwrap(), (AgentAccess::LOCKED, None));
        set_project_access(&f.c, id, AgentAccess { level: AreaLevel::Read, for_all: false }, Some(4)).unwrap();
        assert_eq!(ticket_access(&f.c).unwrap()[&id], TicketAccess { link: Some(4), own: AgentAccess { level: AreaLevel::Read, for_all: false }, system: false });
        sidebar_project_moved(&f.c, 4, 9).unwrap();
        assert_eq!(project_access(&f.c, id).unwrap().1, Some(9));
        assert_eq!(set_project_access(&f.c, 9999, AgentAccess::LOCKED, None).unwrap_err().is_not_found(), true);
    }

    #[test]
    fn tickets_and_comments_carry_their_origin() {
        let f = Fx::new();
        let t = f.mk(f.task, "User");
        assert_eq!((get_ticket(&f.c, t.id).unwrap().origin.as_str(), get_ticket(&f.c, t.id).unwrap().origin_detail), ("user", None));
        let (a, _) = crate::planning::tickets::create_ticket(&f.c, &crate::planning::tickets::NewTicket::new(f.project, f.task, "Agent"), &Source::agent("Builder")).unwrap();
        let a = get_ticket(&f.c, a.id).unwrap();
        assert_eq!((a.origin, a.origin_detail.as_deref()), (Origin::Agent, Some("Builder")));
        add_comment(&f.c, a.id, "von mir", &Source::ui()).unwrap();
        add_comment(&f.c, a.id, "vom Agenten", &Source::agent("Builder")).unwrap();
        let c = comments_of(&f.c, a.id).unwrap();
        assert_eq!(c.iter().map(|c| c.origin).collect::<Vec<_>>(), vec![Origin::User, Origin::Agent]);
    }

    #[test]
    fn trash_hides_restores_and_purges() {
        let f = Fx::new();
        let t = f.mk(f.task, "Weg damit");
        let (c, _) = add_comment(&f.c, t.id, "Notiz", &Source::agent("A")).unwrap();
        trash_comment(&f.c, c, &Source::agent("A")).unwrap();
        assert!(comments_of(&f.c, t.id).unwrap().is_empty(), "Kommentar im Papierkorb fehlt");
        trash_ticket(&f.c, t.id, &Source::agent("A")).unwrap();
        assert!(is_trashed(&f.c, t.id) && get_ticket(&f.c, t.id).unwrap().archived);
        assert_eq!(trash_ticket(&f.c, t.id, &Source::agent("A")).unwrap_err().is_not_found(), true, "schon im Papierkorb");
        let list = trash(&f.c).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|e| e.by.contains('A')), "{list:?}");
        let page = crate::planning::ticket_list::list_tickets(&f.c, None, &crate::planning::ticket_list::TicketFilter { include_archived: true, ..Default::default() }).unwrap();
        assert!(page.items.iter().all(|x| x.id != t.id), "auch mit archivierten nie im Papierkorb");
        restore(&f.c, list.iter().find(|e| e.comment.is_none()).unwrap(), &Source::ui()).unwrap();
        assert!(!is_trashed(&f.c, t.id) && !get_ticket(&f.c, t.id).unwrap().archived, "Archiv-Markierung zurück");
        restore_comment(&f.c, c, &Source::ui()).unwrap();
        assert_eq!(comments_of(&f.c, t.id).unwrap().len(), 1);
        // endgültig löschen
        trash_ticket(&f.c, t.id, &Source::agent("A")).unwrap();
        assert_eq!(empty_trash(&f.c, &Source::ui()).unwrap(), 1);
        assert!(get_ticket(&f.c, t.id).is_err());
        assert!(trash(&f.c).unwrap().is_empty());
    }

    #[test]
    fn private_switch_is_logged() {
        let f = Fx::new();
        let t = f.mk(f.task, "Geheim");
        assert!(set_ticket_private(&f.c, t.id, true, &Source::ui()).unwrap());
        assert!(!set_ticket_private(&f.c, t.id, true, &Source::ui()).unwrap());
        assert!(get_ticket(&f.c, t.id).unwrap().private);
        let h = crate::planning::history::history_of(&f.c, t.id).unwrap();
        assert!(h.iter().any(|e| e.field == HistoryField::Other("private".into())));
    }
}
