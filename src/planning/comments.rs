//! Kommentare an Tickets (Markdown-Text). Anlegen und Bearbeiten lassen sich rückgängig machen, Löschen ist endgültig.
//! Der Verlauf des Tickets nennt jeden Kommentar-Vorgang mit einem Auszug des Texts.

use super::history::{self, HistoryField, Source};
use super::ops::TicketOp;
use super::ticket_error::{atomic, TRes, TicketError};
use super::tickets::{get_ticket, touch};
use super::access::Origin;
use rusqlite::{params, Connection, OptionalExtension};

/// Längster Kommentar in Zeichen.
pub const MAX_COMMENT_CHARS: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: i64,
    pub ticket_id: i64,
    /// Markdown.
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    /// Wer ihn geschrieben hat (#386): Agenten dürfen nur Kommentare mit Herkunft „Agent“ ändern oder löschen.
    pub origin: Origin,
    pub origin_detail: Option<String>,
}

const COLS: &str = "id, ticket_id, body, created_at, updated_at, origin, origin_detail";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Comment> {
    Ok(Comment { id: r.get(0)?, ticket_id: r.get(1)?, body: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)?, origin: Origin::parse(&r.get::<_, String>(5)?), origin_detail: r.get(6)? })
}

pub fn get_comment(conn: &Connection, id: i64) -> TRes<Comment> {
    conn.query_row(&format!("SELECT {COLS} FROM comments WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(TicketError::not_found)
}

/// Die Kommentare eines Tickets, älteste zuerst (ohne die im Papierkorb, #386).
pub fn comments_of(conn: &Connection, ticket_id: i64) -> TRes<Vec<Comment>> {
    get_ticket(conn, ticket_id)?;
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM comments WHERE ticket_id = ?1 AND trashed_at IS NULL ORDER BY id"))?;
    Ok(st.query_map([ticket_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

fn clean_body(body: &str) -> TRes<String> {
    let b = body.trim();
    if b.is_empty() {
        return Err(TicketError::CommentEmpty);
    }
    if b.chars().count() > MAX_COMMENT_CHARS {
        return Err(TicketError::CommentTooLong { max: MAX_COMMENT_CHARS });
    }
    Ok(b.to_string())
}

fn excerpt(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Schreibt den Kommentar (mit fester Kennung, wenn `c.id > 0`) samt Verlauf. Prüft nichts.
pub(crate) fn raw_insert(conn: &Connection, c: &Comment, source: &Source, batch: i64) -> TRes<i64> {
    if c.id > 0 {
        conn.execute(
            "INSERT INTO comments (id, ticket_id, body, created_at, updated_at, origin, origin_detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![c.id, c.ticket_id, c.body, c.created_at, c.updated_at, c.origin.as_str(), c.origin_detail],
        )?;
    } else {
        conn.execute(
            "INSERT INTO comments (ticket_id, body, created_at, updated_at, origin, origin_detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![c.ticket_id, c.body, c.created_at, c.updated_at, c.origin.as_str(), c.origin_detail],
        )?;
    }
    let id = if c.id > 0 { c.id } else { conn.last_insert_rowid() };
    history::log(conn, c.ticket_id, source, batch, &HistoryField::Comment, None, Some(&excerpt(&c.body)))?;
    touch(conn, c.ticket_id)?;
    Ok(id)
}

pub(crate) fn raw_delete(conn: &Connection, c: &Comment, source: &Source, batch: i64) -> TRes<()> {
    if conn.execute("DELETE FROM comments WHERE id = ?1", [c.id])? == 0 {
        return Err(TicketError::not_found());
    }
    history::log(conn, c.ticket_id, source, batch, &HistoryField::Comment, Some(&excerpt(&c.body)), None)?;
    touch(conn, c.ticket_id)
}

/// Text ersetzen (bereinigt). Liefert den alten Text, wenn er sich geändert hat.
pub(crate) fn raw_set_body(conn: &Connection, id: i64, body: &str, source: &Source, batch: i64) -> TRes<Option<String>> {
    let body = clean_body(body)?;
    let cur = get_comment(conn, id)?;
    if cur.body == body {
        return Ok(None);
    }
    conn.execute("UPDATE comments SET body = ?1, updated_at = ?2 WHERE id = ?3", params![body, super::common::now(), id])?;
    history::log(conn, cur.ticket_id, source, batch, &HistoryField::Comment, Some(&excerpt(&cur.body)), Some(&excerpt(&body)))?;
    touch(conn, cur.ticket_id)?;
    Ok(Some(cur.body))
}

/// Legt einen Kommentar an (Text nicht leer, höchstens [`MAX_COMMENT_CHARS`] Zeichen).
pub fn add_comment(conn: &Connection, ticket_id: i64, body: &str, source: &Source) -> TRes<(i64, TicketOp)> {
    atomic(conn, || {
        get_ticket(conn, ticket_id)?;
        let body = clean_body(body)?;
        let now = super::common::now();
        let batch = history::next_batch(conn)?;
        let c = Comment { id: 0, ticket_id, body, created_at: now.clone(), updated_at: now, origin: Origin::of(source), origin_detail: super::access::origin_detail(source) };
        let id = raw_insert(conn, &c, source, batch)?;
        Ok((id, TicketOp::Comment { comment: get_comment(conn, id)?, present: true, source: source.clone() }))
    })
}

/// Ändert den Text eines Kommentars. `None` = Text unverändert.
pub fn edit_comment(conn: &Connection, id: i64, body: &str, source: &Source) -> TRes<Option<TicketOp>> {
    atomic(conn, || {
        let ticket = get_comment(conn, id)?.ticket_id;
        let batch = history::next_batch(conn)?;
        let new = clean_body(body)?;
        Ok(raw_set_body(conn, id, &new, source, batch)?.map(|before| TicketOp::CommentEdit { ticket, comment: id, before, after: new, source: source.clone() }))
    })
}

/// Löscht einen Kommentar endgültig.
pub fn delete_comment(conn: &Connection, id: i64, source: &Source) -> TRes<()> {
    atomic(conn, || {
        let c = get_comment(conn, id)?;
        let batch = history::next_batch(conn)?;
        raw_delete(conn, &c, source, batch)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::history::{describe, history_of, SourceKind};
    use crate::planning::tickets::delete_ticket;
    use crate::planning::tickets::fixture::Fx;
    use crate::state::Lang;

    fn ui() -> Source {
        Source::ui()
    }

    #[test]
    fn adds_edits_and_deletes_comments() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let (a, _) = add_comment(&f.c, t.id, "  Erster **Kommentar**\n", &Source::agent("S1")).unwrap();
        let (b, _) = add_comment(&f.c, t.id, "Zweiter", &ui()).unwrap();
        let list = comments_of(&f.c, t.id).unwrap();
        assert_eq!(list.iter().map(|c| c.id).collect::<Vec<_>>(), vec![a, b], "älteste zuerst");
        assert_eq!(list[0].body, "Erster **Kommentar**");
        assert!(edit_comment(&f.c, a, "Erster, geändert", &ui()).unwrap().is_some());
        assert!(edit_comment(&f.c, a, " Erster, geändert ", &ui()).unwrap().is_none(), "unverändert");
        let c = get_comment(&f.c, a).unwrap();
        assert_eq!(c.body, "Erster, geändert");
        assert!(c.updated_at >= c.created_at);
        delete_comment(&f.c, b, &ui()).unwrap();
        assert_eq!(comments_of(&f.c, t.id).unwrap().len(), 1);
        assert!(delete_comment(&f.c, b, &ui()).unwrap_err().is_not_found());
        assert!(edit_comment(&f.c, b, "x", &ui()).unwrap_err().is_not_found());
        assert!(add_comment(&f.c, 9999, "x", &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn rejects_empty_and_huge_comments() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        for bad in ["", "  ", "\n\t\n"] {
            assert_eq!(add_comment(&f.c, t.id, bad, &ui()).unwrap_err(), TicketError::CommentEmpty);
        }
        assert_eq!(add_comment(&f.c, t.id, &"x".repeat(MAX_COMMENT_CHARS + 1), &ui()).unwrap_err(), TicketError::CommentTooLong { max: MAX_COMMENT_CHARS });
        assert!(add_comment(&f.c, t.id, &"x".repeat(MAX_COMMENT_CHARS), &ui()).is_ok());
        let (id, _) = add_comment(&f.c, t.id, "ok", &ui()).unwrap();
        assert_eq!(edit_comment(&f.c, id, "  ", &ui()).unwrap_err(), TicketError::CommentEmpty);
        assert_eq!(get_comment(&f.c, id).unwrap().body, "ok");
    }

    #[test]
    fn comments_appear_in_the_history_with_their_source() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let (id, _) = add_comment(&f.c, t.id, "Hallo Welt", &Source::agent("Agent-1")).unwrap();
        edit_comment(&f.c, id, "Hallo Mond", &ui()).unwrap();
        delete_comment(&f.c, id, &ui()).unwrap();
        let h: Vec<_> = history_of(&f.c, t.id).unwrap().into_iter().filter(|e| e.field == HistoryField::Comment).collect();
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].source.kind, SourceKind::Agent);
        assert_eq!(describe(&f.c, &h[0], Lang::En), "Comment added: Hallo Welt");
        assert_eq!(describe(&f.c, &h[1], Lang::De), "Kommentar geändert: Hallo Mond");
        assert_eq!(describe(&f.c, &h[2], Lang::De), "Kommentar gelöscht");
    }

    #[test]
    fn comments_go_away_with_their_ticket() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let (id, _) = add_comment(&f.c, t.id, "x", &ui()).unwrap();
        delete_ticket(&f.c, t.id, &ui()).unwrap();
        assert!(get_comment(&f.c, id).unwrap_err().is_not_found());
        assert!(comments_of(&f.c, t.id).unwrap_err().is_not_found());
    }
}
