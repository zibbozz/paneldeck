//! Rückgängig/Wiederholen für Ticket-Änderungen: [`TicketOp`] beschreibt eine abgeschlossene Änderung und lässt sich mit
//! `undo::History` (Strg+Z / Strg+Y) zurücknehmen und wiederholen – im Stil von `tasks::TaskOp`.
//!
//! * Die Operationen halten Vorher-/Nachher-Zustände fest und schreiben beim Ausführen mit den „rohen“ Schreibfunktionen
//!   (ohne Regelprüfung: ein Statuswechsel, der einmal erlaubt war, lässt sich auch zurücknehmen, wenn die Matrix das
//!   Zurück nicht kennt). Jedes Rückgängig/Wiederholen schreibt den Verlauf des Tickets fort, damit er den Stand der Daten
//!   widerspiegelt.
//! * **Anlegen** ([`TicketOp::Created`]): Zurücknehmen entfernt das Ticket – aber nur, solange es noch keine weiteren Daten
//!   hat (Kommentare, Zeiteinträge, Verknüpfungen, Kinder, Verweise anderer Tickets); sonst `CannotUndoCreate`, und die
//!   History verwirft den Eintrag. Wiederholen legt es mit derselben Kennung und demselben Schlüssel wieder an.
//! * **Löschen** ist nicht rückgängig zu machen und hat keine Operation. Nach einem Löschen sollte die Oberfläche die
//!   History mit [`TicketOp::touches`] bereinigen (`History::retain`).
//! * **Sprint-Zuordnung** ([`TicketOp::Sprint`]): stellt die Zeilen von `sprint_tickets` samt Zeitstempeln wieder her (Begründung und
//!   Grenzen in `sprints::membership`); **Todo-Umwandlung** ([`TicketOp::TodoConversion`], hinter dem Anlegen in einer `Batch`):
//!   Zuordnung der Karte und Abhaken.
//! * Mehrere Änderungen einer Aktion (z. B. Statuswechsel samt Werten) bilden eine [`TicketOp::Batch`].

use super::comments::{self, Comment};
use super::history::{self, HistoryField, Source};
use super::links::{self, LinkRecord};
use super::sprints::membership::SprintChange;
use super::todo_bridge;
use super::ticket_error::{atomic, TRes, TicketError};
use super::tickets::{self, Ticket};
use super::time_entries::{self, TimeEntry, TimeState};
use super::values;
use crate::state;
use crate::undo::Op;
use rusqlite::{params, Connection};
use serde_json::Value;

/// Änderung eines Feldwerts (`None` = leer).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldChange {
    pub field_id: i64,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// Markierung („Flag“, Impediment).
    Flag,
    /// Favoriten-Stern.
    Star,
    /// Privat (#386): nie für Agenten sichtbar.
    Private,
}

/// Alles, was nötig ist, um ein Ticket mit derselben Kennung wieder anzulegen.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketSnapshot {
    pub ticket: Ticket,
    /// Gesetzte Feldwerte.
    pub values: Vec<(i64, Value)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TicketOp {
    /// Ticket ist (nicht) vorhanden: `present: true` legt es wieder an, `false` entfernt es (nur ohne weitere Daten).
    Created { snapshot: TicketSnapshot, present: bool, source: Source },
    Title { ticket: i64, before: String, after: String, source: Source },
    /// Feldwerte: `before` → `after` je Feld.
    Fields { ticket: i64, changes: Vec<FieldChange>, source: Source },
    Status { ticket: i64, before: Option<i64>, after: Option<i64>, source: Source },
    Parent { ticket: i64, before: Option<i64>, after: Option<i64>, source: Source },
    Mark { ticket: i64, kind: MarkKind, before: bool, after: bool, source: Source },
    /// Rang: (Ticket, vorher, nachher) für alle geänderten Tickets.
    Rank { changes: Vec<(i64, i64, i64)> },
    /// Verknüpfung ist (nicht) vorhanden.
    Link { link: LinkRecord, present: bool, source: Source },
    /// Kommentar ist (nicht) vorhanden.
    Comment { comment: Comment, present: bool, source: Source },
    CommentEdit { ticket: i64, comment: i64, before: String, after: String, source: Source },
    /// Zeiteintrag ist (nicht) vorhanden.
    Time { entry: TimeEntry, present: bool, source: Source },
    TimeEdit { ticket: i64, entry: i64, before: TimeState, after: TimeState, source: Source },
    /// Zugehörigkeit zu Sprints (Backlog/Sprint per Ziehen oder Menü): stellt die Zeilen von `sprint_tickets` samt Zeitstempeln her.
    Sprint(SprintChange),
    /// Umwandlung einer Todo-Karte: `present` = die Karte ist abgehakt und dem Ticket zugeordnet; rückwärts wird die Zuordnung
    /// gelöst und die Karte wieder geöffnet (wenn sie es vorher war: `was_done = false`). Steht in einer [`TicketOp::Batch`] hinter dem Anlegen.
    TodoConversion { todo: i64, ticket: i64, was_done: bool, present: bool },
    /// Mehrere Änderungen einer Aktion; rückwärts in umgekehrter Reihenfolge.
    Batch(Vec<TicketOp>),
}

impl TicketOp {
    /// Fasst Operationen einer Aktion zusammen: keine → `None`, eine → sie selbst, mehrere → [`TicketOp::Batch`].
    pub fn combine(mut ops: Vec<TicketOp>) -> Option<TicketOp> {
        match ops.len() {
            0 => None,
            1 => ops.pop(),
            _ => Some(TicketOp::Batch(ops)),
        }
    }

    /// Bezieht sich die Änderung auf eines dieser Tickets (zum Bereinigen der History nach dem Löschen)?
    pub fn touches(&self, ids: &[i64]) -> bool {
        match self {
            TicketOp::Created { snapshot, .. } => ids.contains(&snapshot.ticket.id),
            TicketOp::Title { ticket, .. }
            | TicketOp::Fields { ticket, .. }
            | TicketOp::Status { ticket, .. }
            | TicketOp::Parent { ticket, .. }
            | TicketOp::Mark { ticket, .. }
            | TicketOp::CommentEdit { ticket, .. }
            | TicketOp::TimeEdit { ticket, .. } => ids.contains(ticket),
            TicketOp::Rank { changes } => changes.iter().any(|(t, _, _)| ids.contains(t)),
            TicketOp::Link { link, .. } => ids.contains(&link.from_ticket_id) || matches!(link.target, links::Target::Ticket(t) if ids.contains(&t)),
            TicketOp::Comment { comment, .. } => ids.contains(&comment.ticket_id),
            TicketOp::Time { entry, .. } => ids.contains(&entry.ticket_id),
            TicketOp::Sprint(c) => c.touches(ids),
            TicketOp::TodoConversion { ticket, .. } => ids.contains(ticket),
            TicketOp::Batch(v) => v.iter().any(|o| o.touches(ids)),
        }
    }
}

fn insert_ticket(conn: &Connection, t: &Ticket) -> TRes<()> {
    conn.execute(
        "INSERT INTO tickets (id, project_id, number, type_id, title, status_id, parent_id, rank, flagged, starred, created_at, updated_at, private, origin, origin_detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        // Privat und Herkunft (#386) bleiben beim Wiederholen erhalten.
        params![t.id, t.project_id, t.number, t.type_id, t.title, t.status_id, t.parent_id, t.rank, t.flagged as i64, t.starred as i64, t.created_at, t.updated_at, t.private as i64, t.origin.as_str(), t.origin_detail],
    )?;
    Ok(())
}

/// Hat das Ticket Daten, die beim Entfernen verloren gingen?
fn has_further_data(conn: &Connection, id: i64) -> TRes<bool> {
    let n: i64 = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM comments WHERE ticket_id = ?1) + (SELECT COUNT(*) FROM time_entries WHERE ticket_id = ?1)
              + (SELECT COUNT(*) FROM links WHERE from_ticket_id = ?1 OR to_ticket_id = ?1) + (SELECT COUNT(*) FROM tickets WHERE parent_id = ?1)",
        [id],
        |r| r.get(0),
    )?;
    Ok(n > 0 || tickets::delete_impact(conn, id)?.referenced_by > 0)
}

fn run(conn: &Connection, op: &TicketOp) -> TRes<()> {
    atomic(conn, || {
        let batch = history::next_batch(conn)?;
        match op {
            TicketOp::Created { snapshot, present: true, source } => {
                insert_ticket(conn, &snapshot.ticket)?;
                for (fid, v) in &snapshot.values {
                    let f = super::fields::get_field(conn, *fid)?;
                    values::write_value(conn, snapshot.ticket.id, &f, Some(v))?;
                }
                history::log(conn, snapshot.ticket.id, source, batch, &HistoryField::Created, None, None)?;
            }
            TicketOp::Created { snapshot, present: false, source } => {
                tickets::get_ticket(conn, snapshot.ticket.id)?;
                if has_further_data(conn, snapshot.ticket.id)? {
                    return Err(TicketError::CannotUndoCreate);
                }
                tickets::delete_ticket(conn, snapshot.ticket.id, source)?;
            }
            TicketOp::Title { ticket, after, source, .. } => {
                tickets::raw_set_title(conn, *ticket, after, source, batch)?;
            }
            TicketOp::Fields { ticket, changes, source } => {
                let target: Vec<(i64, Option<Value>)> = changes.iter().map(|c| (c.field_id, c.after.clone())).collect();
                tickets::raw_apply_fields(conn, *ticket, &target, source, batch)?;
            }
            TicketOp::Status { ticket, after, source, .. } => {
                tickets::raw_set_status(conn, *ticket, *after, source, batch)?;
            }
            TicketOp::Parent { ticket, after, source, .. } => {
                tickets::raw_set_parent(conn, *ticket, *after, source, batch)?;
            }
            TicketOp::Mark { ticket, kind, after, source, .. } => {
                tickets::raw_set_mark(conn, *ticket, *kind, *after, source, batch)?;
            }
            TicketOp::Rank { changes } => {
                for (id, _, _) in changes {
                    tickets::get_ticket(conn, *id)?;
                }
                tickets::raw_set_ranks(conn, &changes.iter().map(|(i, _, a)| (*i, *a)).collect::<Vec<_>>())?;
            }
            TicketOp::Link { link, present: true, source } => {
                links::raw_insert(conn, link, source, batch)?;
            }
            TicketOp::Link { link, present: false, source } => {
                links::raw_delete(conn, link, source, batch)?;
            }
            TicketOp::Comment { comment, present: true, source } => {
                comments::raw_insert(conn, comment, source, batch)?;
            }
            TicketOp::Comment { comment, present: false, source } => {
                comments::raw_delete(conn, comment, source, batch)?;
            }
            TicketOp::CommentEdit { comment, after, source, .. } => {
                comments::raw_set_body(conn, *comment, after, source, batch)?;
            }
            TicketOp::Time { entry, present: true, source } => {
                time_entries::raw_insert(conn, entry, source, batch)?;
            }
            TicketOp::Time { entry, present: false, source } => {
                time_entries::raw_delete(conn, entry, source, batch)?;
            }
            TicketOp::TimeEdit { entry, after, source, .. } => {
                time_entries::raw_set_state(conn, *entry, after, source, batch)?;
            }
            TicketOp::Sprint(change) => {
                change.apply(conn)?;
            }
            TicketOp::TodoConversion { todo, ticket, was_done, present } => {
                todo_bridge::run_conversion(conn, *todo, *ticket, *was_done, *present)?;
            }
            TicketOp::Batch(ops) => {
                for o in ops {
                    run(conn, o)?;
                }
            }
        }
        Ok(())
    })
}

impl Op for TicketOp {
    type Ctx = Connection;

    fn apply(&self, conn: &Connection) -> Result<(), String> {
        run(conn, self).map_err(|e| e.message(state::lang()))
    }

    fn inverse(&self) -> Self {
        match self {
            TicketOp::Created { snapshot, present, source } => TicketOp::Created { snapshot: snapshot.clone(), present: !present, source: source.clone() },
            TicketOp::Title { ticket, before, after, source } => TicketOp::Title { ticket: *ticket, before: after.clone(), after: before.clone(), source: source.clone() },
            TicketOp::Fields { ticket, changes, source } => TicketOp::Fields {
                ticket: *ticket,
                changes: changes.iter().map(|c| FieldChange { field_id: c.field_id, before: c.after.clone(), after: c.before.clone() }).collect(),
                source: source.clone(),
            },
            TicketOp::Status { ticket, before, after, source } => TicketOp::Status { ticket: *ticket, before: *after, after: *before, source: source.clone() },
            TicketOp::Parent { ticket, before, after, source } => TicketOp::Parent { ticket: *ticket, before: *after, after: *before, source: source.clone() },
            TicketOp::Mark { ticket, kind, before, after, source } => TicketOp::Mark { ticket: *ticket, kind: *kind, before: *after, after: *before, source: source.clone() },
            TicketOp::Rank { changes } => TicketOp::Rank { changes: changes.iter().map(|(t, b, a)| (*t, *a, *b)).collect() },
            TicketOp::Link { link, present, source } => TicketOp::Link { link: link.clone(), present: !present, source: source.clone() },
            TicketOp::Comment { comment, present, source } => TicketOp::Comment { comment: comment.clone(), present: !present, source: source.clone() },
            TicketOp::CommentEdit { ticket, comment, before, after, source } => TicketOp::CommentEdit { ticket: *ticket, comment: *comment, before: after.clone(), after: before.clone(), source: source.clone() },
            TicketOp::Time { entry, present, source } => TicketOp::Time { entry: entry.clone(), present: !present, source: source.clone() },
            TicketOp::TimeEdit { ticket, entry, before, after, source } => TicketOp::TimeEdit { ticket: *ticket, entry: *entry, before: after.clone(), after: before.clone(), source: source.clone() },
            TicketOp::Sprint(c) => TicketOp::Sprint(c.inverse()),
            TicketOp::TodoConversion { todo, ticket, was_done, present } => TicketOp::TodoConversion { todo: *todo, ticket: *ticket, was_done: *was_done, present: !present },
            TicketOp::Batch(ops) => TicketOp::Batch(ops.iter().rev().map(|o| o.inverse()).collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::comments::{add_comment, comments_of, edit_comment, get_comment};
    use crate::planning::history::{history_of, SourceKind};
    use crate::planning::link_types::link_type_by_key;
    use crate::planning::links::{add_link, links_of, remove_link, Target};
    use crate::planning::ticket_list::{list_tickets, TicketFilter};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::*;
    use crate::planning::time_entries::{edit_time_entry, entries_of, log_time, tests_now, TimeEdit};
    use crate::planning::transition::transition_ticket;
    use crate::state::Lang;
    use crate::undo::History;
    use serde_json::json;

    fn ui() -> Source {
        Source::ui()
    }

    /// Operation ausführen lassen, rückgängig machen, wiederholen: `check` prüft jeweils den Zustand.
    fn roundtrip(f: &Fx, op: &TicketOp, after: &dyn Fn(&Fx), before: &dyn Fn(&Fx)) {
        let mut h: History<TicketOp> = History::new(10);
        after(f);
        h.push(op.clone());
        assert!(h.undo(&f.c).unwrap());
        before(f);
        assert!(h.redo(&f.c).unwrap());
        after(f);
        assert!(h.undo(&f.c).unwrap());
        before(f);
    }

    #[test]
    fn title_and_fields() {
        let f = Fx::new();
        let t = f.mk(f.story, "Alt");
        let sp = f.field(f.story, "story_points");
        let op = update_ticket(&f.c, t.id, &TicketUpdate { title: Some("Neu".into()), values: vec![(sp.id, json!(5))] }, &ui()).unwrap().unwrap();
        roundtrip(
            &f,
            &op,
            &|f| {
                assert_eq!(get_ticket(&f.c, t.id).unwrap().title, "Neu");
                assert_eq!(values::get_value(&f.c, t.id, &sp).unwrap(), Some(json!(5)));
            },
            &|f| {
                assert_eq!(get_ticket(&f.c, t.id).unwrap().title, "Alt");
                assert_eq!(values::get_value(&f.c, t.id, &sp).unwrap(), None);
            },
        );
        // der Verlauf schreibt jeden Schritt mit (nach 2 Rückgängig + 1 Wiederholen: Anlegen + 2 + 2 + 2 + 2 Einträge)
        let n = history_of(&f.c, t.id).unwrap().len();
        assert_eq!(n, 1 + 2 + 2 + 2 + 2);
    }

    #[test]
    fn status_transition_including_set_values_can_be_undone_even_against_the_matrix() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let note = f.add_field(f.task, "Notiz", crate::planning::fields::FieldKind::Text, Default::default(), false);
        let op = transition_ticket(&f.c, t.id, f.done, &[(note.id, json!("fertig"))], &ui()).unwrap().unwrap();
        // „done -> todo“ ist laut Matrix nicht erlaubt, Rückgängig muss trotzdem gehen
        assert_eq!(transition_ticket(&f.c, t.id, f.todo, &[], &ui()).unwrap_err(), TicketError::TransitionNotAllowed);
        roundtrip(
            &f,
            &op,
            &|f| {
                assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.done));
                assert_eq!(values::get_value(&f.c, t.id, &note).unwrap(), Some(json!("fertig")));
            },
            &|f| {
                assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.todo));
                assert_eq!(values::get_value(&f.c, t.id, &note).unwrap(), None);
            },
        );
    }

    #[test]
    fn flag_star_and_parent() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "E");
        let t = f.mk(f.story, "S");
        let op = set_flagged(&f.c, t.id, true, &ui()).unwrap().unwrap();
        roundtrip(&f, &op, &|f| assert!(get_ticket(&f.c, t.id).unwrap().flagged), &|f| assert!(!get_ticket(&f.c, t.id).unwrap().flagged));
        let op = set_starred(&f.c, t.id, true, &ui()).unwrap().unwrap();
        roundtrip(&f, &op, &|f| assert!(get_ticket(&f.c, t.id).unwrap().starred), &|f| assert!(!get_ticket(&f.c, t.id).unwrap().starred));
        let op = set_parent(&f.c, t.id, Some(epic.id), &ui()).unwrap().unwrap();
        roundtrip(&f, &op, &|f| assert_eq!(get_ticket(&f.c, t.id).unwrap().parent_id, Some(epic.id)), &|f| assert_eq!(get_ticket(&f.c, t.id).unwrap().parent_id, None));
    }

    #[test]
    fn rank_moves() {
        let f = Fx::new();
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let c = f.mk(f.task, "C");
        let order = |f: &Fx| -> Vec<i64> { list_tickets(&f.c, Some(f.project), &TicketFilter::default()).unwrap().items.iter().map(|t| t.id).collect() };
        let op = move_rank(&f.c, c.id, Some(a.id)).unwrap().unwrap();
        roundtrip(&f, &op, &|f| assert_eq!(order(f), vec![c.id, a.id, b.id]), &|f| assert_eq!(order(f), vec![a.id, b.id, c.id]));
        assert_eq!(get_ticket(&f.c, a.id).unwrap().rank, 0);
    }

    #[test]
    fn links_add_and_remove() {
        let f = Fx::new();
        let (a, b) = (f.mk(f.task, "A"), f.mk(f.task, "B"));
        let blocks = link_type_by_key(&f.c, "blocks").unwrap().id;
        let (id, op) = add_link(&f.c, a.id, blocks, Target::Ticket(b.id), &ui()).unwrap();
        roundtrip(&f, &op, &|f| assert_eq!(links_of(&f.c, a.id, Lang::En).unwrap().len(), 1), &|f| assert!(links_of(&f.c, a.id, Lang::En).unwrap().is_empty()));
        // die Kennung bleibt beim Wiederholen gleich
        op.apply(&f.c).unwrap();
        assert_eq!(links_of(&f.c, b.id, Lang::En).unwrap()[0].id, id);
        // Entfernen und zurück
        let rm = remove_link(&f.c, id, &ui()).unwrap();
        roundtrip(&f, &rm, &|f| assert!(links_of(&f.c, a.id, Lang::En).unwrap().is_empty()), &|f| assert_eq!(links_of(&f.c, a.id, Lang::En).unwrap().len(), 1));
    }

    #[test]
    fn comments_and_time_entries() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let (cid, op) = add_comment(&f.c, t.id, "Hallo", &ui()).unwrap();
        roundtrip(&f, &op, &|f| assert_eq!(comments_of(&f.c, t.id).unwrap().len(), 1), &|f| assert!(comments_of(&f.c, t.id).unwrap().is_empty()));
        op.apply(&f.c).unwrap();
        assert_eq!(comments_of(&f.c, t.id).unwrap()[0].id, cid);
        let edit = edit_comment(&f.c, cid, "Geändert", &ui()).unwrap().unwrap();
        roundtrip(&f, &edit, &|f| assert_eq!(get_comment(&f.c, cid).unwrap().body, "Geändert"), &|f| assert_eq!(get_comment(&f.c, cid).unwrap().body, "Hallo"));
        let (eid, op) = log_time(&f.c, t.id, "1h", None, None, "x", tests_now(), &ui()).unwrap();
        roundtrip(&f, &op, &|f| assert_eq!(entries_of(&f.c, t.id).unwrap().len(), 1), &|f| assert!(entries_of(&f.c, t.id).unwrap().is_empty()));
        op.apply(&f.c).unwrap();
        assert_eq!(entries_of(&f.c, t.id).unwrap()[0].id, eid);
        let edit = edit_time_entry(&f.c, eid, &TimeEdit { duration: Some("2h".into()), comment: Some("y".into()), ..Default::default() }, &ui()).unwrap().unwrap();
        roundtrip(
            &f,
            &edit,
            &|f| assert_eq!(entries_of(&f.c, t.id).unwrap()[0].minutes, 120),
            &|f| {
                let e = &entries_of(&f.c, t.id).unwrap()[0];
                assert_eq!((e.minutes, e.comment.as_str()), (60, "x"));
            },
        );
    }

    #[test]
    fn creating_can_be_undone_while_the_ticket_is_empty() {
        let f = Fx::new();
        let sp = f.field(f.story, "story_points");
        let (t, op) = create_ticket(&f.c, &NewTicket { values: vec![(sp.id, json!(3))], ..NewTicket::new(f.project, f.story, "Neu") }, &Source::agent("A")).unwrap();
        let mut h: History<TicketOp> = History::new(5);
        h.push(op);
        assert!(h.undo(&f.c).unwrap());
        assert!(get_ticket(&f.c, t.id).unwrap_err().is_not_found());
        assert_eq!(list_tickets(&f.c, None, &TicketFilter::default()).unwrap().total, 0);
        // Wiederholen: gleiche Kennung, gleicher Schlüssel, gleiche Werte
        assert!(h.redo(&f.c).unwrap());
        let back = get_ticket(&f.c, t.id).unwrap();
        assert_eq!((back.id, back.key(), back.title.as_str(), back.status_id), (t.id, "PAN-1".to_string(), "Neu", Some(f.todo)));
        assert_eq!(values::get_value(&f.c, t.id, &sp).unwrap(), Some(json!(3)));
        // der Schlüssel wird auch nach dem Rückgängig nicht neu vergeben
        assert!(h.undo(&f.c).unwrap());
        assert_eq!(f.mk(f.task, "Anderes").key(), "PAN-2");
    }

    #[test]
    fn creating_cannot_be_undone_once_the_ticket_has_data() {
        let f = Fx::new();
        for case in 0..5 {
            let (t, op) = create_ticket(&f.c, &NewTicket::new(f.project, f.epic, "E"), &ui()).unwrap();
            match case {
                0 => {
                    add_comment(&f.c, t.id, "x", &ui()).unwrap();
                }
                1 => {
                    log_time(&f.c, t.id, "1h", None, None, "", tests_now(), &ui()).unwrap();
                }
                2 => {
                    let other = f.mk(f.epic, "O");
                    add_link(&f.c, other.id, link_type_by_key(&f.c, "relates").unwrap().id, Target::Ticket(t.id), &ui()).unwrap();
                }
                3 => {
                    create_child(&f.c, t.id, f.story, "K", vec![], &ui()).unwrap();
                }
                _ => {
                    let story = f.mk(f.story, "S");
                    let ef = f.field(f.story, "epic");
                    set_field(&f.c, story.id, ef.id, json!(t.id), &ui()).unwrap();
                }
            }
            let mut h: History<TicketOp> = History::new(5);
            h.push(op);
            let err = h.undo(&f.c).unwrap_err();
            assert_eq!(err, TicketError::CannotUndoCreate.message(crate::state::lang()), "Fall {case}");
            assert!(get_ticket(&f.c, t.id).is_ok(), "Ticket bleibt (Fall {case})");
            assert!(!h.can_undo(), "der Eintrag wurde verworfen");
        }
        // Änderungen an Feldern allein verhindern das Zurücknehmen nicht, wenn sie vorher zurückgenommen wurden
        let (t, create_op) = create_ticket(&f.c, &NewTicket::new(f.project, f.story, "S"), &ui()).unwrap();
        let sp = f.field(f.story, "story_points");
        let field_op = set_field(&f.c, t.id, sp.id, json!(1), &ui()).unwrap().unwrap();
        let mut h: History<TicketOp> = History::new(5);
        h.push(create_op);
        h.push(field_op);
        assert!(h.undo(&f.c).unwrap());
        assert!(h.undo(&f.c).unwrap());
        assert!(get_ticket(&f.c, t.id).unwrap_err().is_not_found());
    }

    #[test]
    fn undo_errors_when_the_target_is_gone() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let op = set_flagged(&f.c, t.id, true, &ui()).unwrap().unwrap();
        delete_ticket(&f.c, t.id, &ui()).unwrap();
        let mut h: History<TicketOp> = History::new(5);
        h.push(op.clone());
        assert!(h.undo(&f.c).is_err());
        assert!(op.touches(&[t.id]) && !op.touches(&[t.id + 1]));
        // bereinigen
        let mut h: History<TicketOp> = History::new(5);
        h.push(op);
        h.retain(|o| !o.touches(&[t.id]));
        assert!(!h.can_undo());
    }

    #[test]
    fn batches_run_backwards_and_inverse_of_inverse_is_the_original() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let sp = f.field(f.task, "story_points");
        let op = update_ticket(&f.c, t.id, &TicketUpdate { title: Some("Neu".into()), values: vec![(sp.id, json!(2))] }, &ui()).unwrap().unwrap();
        assert_eq!(op.inverse().inverse(), op);
        for o in [
            set_flagged(&f.c, t.id, true, &ui()).unwrap().unwrap(),
            move_rank(&f.c, t.id, None).unwrap_or(None).unwrap_or(TicketOp::Rank { changes: vec![(t.id, 0, 1)] }),
            add_comment(&f.c, t.id, "x", &ui()).unwrap().1,
        ] {
            assert_eq!(o.inverse().inverse(), o);
        }
        assert_eq!(TicketOp::combine(vec![]), None);
        let one = TicketOp::Rank { changes: vec![(1, 0, 1)] };
        assert_eq!(TicketOp::combine(vec![one.clone()]), Some(one.clone()));
        assert!(matches!(TicketOp::combine(vec![one.clone(), one]), Some(TicketOp::Batch(_))));
    }

    #[test]
    fn undo_history_logs_with_the_original_source() {
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        let op = set_flagged(&f.c, t.id, true, &Source::agent("S")).unwrap().unwrap();
        let mut h: History<TicketOp> = History::new(5);
        h.push(op);
        h.undo(&f.c).unwrap();
        let flags: Vec<_> = history_of(&f.c, t.id).unwrap().into_iter().filter(|e| e.field == HistoryField::Flagged).collect();
        assert_eq!(flags.len(), 2);
        assert!(flags.iter().all(|e| e.source.kind == SourceKind::Agent));
        assert_eq!(flags[1].new.as_deref(), Some("0"));
    }
}
