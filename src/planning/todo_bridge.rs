//! Brücke zur einfachen Todo-Liste (`tasks.rs`): Die Karten erscheinen **nur lesbar** als Einträge des System-Projekts `TODO`
//! (Schlüssel `TODO-<n>`, Typ `todolist`, Tags als Labels, erledigt = Status-Kategorie „Done“) und lassen sich in echte Tickets
//! umwandeln. Die Umwandlung merkt sich die Zuordnung (`todo_conversions`) und hakt die Karte ab.
//!
//! Umwandlung ([`convert_todo_to_ticket`]):
//! * Titel = Titel der Karte, sonst die erste Textzeile (höchstens 255 Zeichen);
//! * der Text der Karte kommt in das Feld „Description“ (Schlüssel `description`, Rich-Text oder Text), falls der Typ eines hat;
//! * die Tags werden zu Werten des ersten Labels-Felds, falls der Typ eines hat – sonst verworfen und gemeldet;
//! * danach ist die Karte erledigt und als umgewandelt vermerkt; eine Karte lässt sich nur einmal umwandeln.

use super::fields::{list_fields, Field, FieldKind};
use super::history::Source;
use super::ops::TicketOp;
use super::statuses::StatusCategory;
use super::ticket_error::{atomic, TRes, TicketError};
use super::tickets::{create_ticket, delete_ticket, get_ticket, NewTicket, Ticket, MAX_TITLE_CHARS};
use super::values::{normalize_value, RefCtx};
use super::{common, projects};
use crate::tasks::{self, ItemFilter, TaskError, TaskOp};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Eine Todo-Karte als (nur lesbares) Ticket des System-Projekts.
#[derive(Debug, Clone, PartialEq)]
pub struct TodoTicket {
    pub todo_id: i64,
    /// `TODO-<n>`
    pub key: String,
    /// Das System-Projekt `TODO`.
    pub project_id: i64,
    /// Der System-Typ `todolist`.
    pub type_id: i64,
    /// Titel der Karte, sonst die erste Textzeile.
    pub title: String,
    pub body: String,
    /// Namen der Tags.
    pub labels: Vec<String>,
    /// „Done“, wenn abgehakt, sonst „To Do“.
    pub category: StatusCategory,
    pub done: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Das Ticket, in das die Karte umgewandelt wurde.
    pub converted_to: Option<i64>,
}

fn task_err(e: TaskError) -> TicketError {
    match e {
        TaskError::NotFound => TicketError::not_found(),
        other => TicketError::Db(other.message(crate::state::lang())),
    }
}

/// (Projekt, Typ) des System-Projekts „Todolist“.
pub fn todo_project(conn: &Connection) -> TRes<(i64, i64)> {
    let p = projects::project_by_key(conn, "TODO")?;
    let t: i64 = conn.query_row("SELECT id FROM ticket_types WHERE project_id = ?1 AND key = 'todolist'", [p.id], |r| r.get(0))?;
    Ok((p.id, t))
}

/// Titel einer Karte: ihr Titel, sonst die erste nicht leere Zeile des Texts.
pub fn display_title(item_title: &str, body: &str) -> String {
    let t = item_title.trim();
    let raw = if t.is_empty() { body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("") } else { t };
    raw.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TITLE_CHARS).collect()
}

fn conversions(conn: &Connection) -> TRes<HashMap<i64, i64>> {
    let mut st = conn.prepare("SELECT todo_id, ticket_id FROM todo_conversions")?;
    Ok(st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?)
}

fn build(item: tasks::Item, tags: &HashMap<i64, String>, conv: &HashMap<i64, i64>, ids: (i64, i64)) -> TodoTicket {
    TodoTicket {
        todo_id: item.id,
        key: item.key(),
        project_id: ids.0,
        type_id: ids.1,
        title: display_title(&item.title, &item.body),
        labels: item.tags.iter().filter_map(|t| tags.get(t).cloned()).collect(),
        category: if item.done { StatusCategory::Done } else { StatusCategory::Todo },
        done: item.done,
        converted_to: conv.get(&item.id).copied(),
        body: item.body,
        created_at: item.created_at,
        updated_at: item.updated_at,
    }
}

/// Die Karten der Todo-Liste in Listenreihenfolge (oben zuerst) als lesbare Tickets des Projekts `TODO`.
pub fn list_todo_as_tickets(conn: &Connection, filter: &ItemFilter) -> TRes<Vec<TodoTicket>> {
    let ids = todo_project(conn)?;
    let tags: HashMap<i64, String> = tasks::list_tags(conn).map_err(task_err)?.into_iter().map(|t| (t.id, t.name)).collect();
    let conv = conversions(conn)?;
    Ok(tasks::list_items(conn, filter).map_err(task_err)?.into_iter().map(|i| build(i, &tags, &conv, ids)).collect())
}

/// Eine Karte als lesbares Ticket (`TODO-<n>`).
pub fn todo_ticket(conn: &Connection, todo_id: i64) -> TRes<TodoTicket> {
    let ids = todo_project(conn)?;
    let tags: HashMap<i64, String> = tasks::list_tags(conn).map_err(task_err)?.into_iter().map(|t| (t.id, t.name)).collect();
    let conv = conversions(conn)?;
    Ok(build(tasks::get_item(conn, todo_id).map_err(task_err)?, &tags, &conv, ids))
}

/// Das Ticket, in das die Karte umgewandelt wurde.
pub fn converted_ticket_of(conn: &Connection, todo_id: i64) -> TRes<Option<i64>> {
    Ok(conn.query_row("SELECT ticket_id FROM todo_conversions WHERE todo_id = ?1", [todo_id], |r| r.get(0)).optional()?)
}

/// Ergebnis einer Umwandlung.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversion {
    pub ticket: Ticket,
    /// Macht das Anlegen des Tickets rückgängig (und entfernt die Zuordnung).
    pub op: TicketOp,
    /// Das Abhaken der Karte (`None`, wenn sie schon erledigt war) für die History der Todo-Liste.
    pub todo_op: Option<TaskOp>,
    /// Tags, die nicht übernommen werden konnten (kein Labels-Feld am Typ).
    pub dropped_tags: Vec<String>,
    /// Der Text der Karte konnte nicht übernommen werden (kein Description-Feld oder Wert unzulässig).
    pub body_dropped: bool,
}

fn description_field(fields: &[Field]) -> Option<&Field> {
    let text_like = |f: &&Field| matches!(f.kind, FieldKind::Rich | FieldKind::Text);
    fields.iter().filter(text_like).find(|f| f.key.as_deref() == Some("description")).or_else(|| {
        fields.iter().filter(text_like).find(|f| common::same_label(Some("description"), None, f.key.as_deref(), f.name.as_deref()))
    })
}

/// Wandelt die Karte in ein Ticket des Projekts/Typs um (siehe Moduldoku).
pub fn convert_todo_to_ticket(conn: &Connection, todo_id: i64, project_id: i64, type_id: i64, source: &Source) -> TRes<Conversion> {
    convert_todo_with_values(conn, todo_id, project_id, type_id, &[], source)
}

/// Wie [`convert_todo_to_ticket`], mit zusätzlichen Feldwerten (z. B. für Pflichtfelder des Typs); sie haben Vorrang vor
/// den aus der Karte übernommenen.
pub fn convert_todo_with_values(conn: &Connection, todo_id: i64, project_id: i64, type_id: i64, extra: &[(i64, Value)], source: &Source) -> TRes<Conversion> {
    let mut conv = atomic(conn, || {
            let item = tasks::get_item(conn, todo_id).map_err(task_err)?;
            if let Some(tid) = converted_ticket_of(conn, todo_id)? {
                return Err(TicketError::TodoAlreadyConverted { ticket_id: tid });
            }
            projects::get_project(conn, project_id)?;
            let title = display_title(&item.title, &item.body);
            if title.is_empty() {
                return Err(TicketError::TitleEmpty);
            }
            let fields = match super::types::get_type(conn, type_id) {
                Ok(t) if t.project_id == project_id => list_fields(conn, type_id)?,
                Ok(_) => return Err(TicketError::TypeNotInProject),
                Err(e) => return Err(e.into()),
            };
            let ctx = RefCtx { project_id, ticket_id: None };
            let mut values: Vec<(i64, Value)> = Vec::new();
            let mut body_dropped = false;
            let body = item.body.trim();
            if !body.is_empty() && body != title {
                match description_field(&fields).and_then(|f| normalize_value(conn, &ctx, f, &json!(body)).ok().flatten().map(|v| (f.id, v))) {
                    Some(v) => values.push(v),
                    None => body_dropped = true,
                }
            }
            let mut dropped_tags: Vec<String> = Vec::new();
            if !item.tags.is_empty() {
                let all: HashMap<i64, String> = tasks::list_tags(conn).map_err(task_err)?.into_iter().map(|t| (t.id, t.name)).collect();
                let names: Vec<String> = item.tags.iter().filter_map(|t| all.get(t).cloned()).collect();
                let target = fields.iter().find(|f| f.kind == FieldKind::Labels);
                match target.and_then(|f| normalize_value(conn, &ctx, f, &json!(names)).ok().flatten().map(|v| (f.id, v))) {
                    Some(v) => values.push(v),
                    None => dropped_tags = names,
                }
            }
            for (fid, v) in extra {
                values.retain(|(i, _)| i != fid);
                values.push((*fid, v.clone()));
            }
            let (ticket, op) = create_ticket(conn, &NewTicket { project_id, type_id, title, parent_id: None, values }, source)?;
            conn.execute("INSERT INTO todo_conversions (todo_id, ticket_id) VALUES (?1, ?2)", params![todo_id, ticket.id])?;
            Ok(Conversion { ticket, op, todo_op: None, dropped_tags, body_dropped })
    })?;
    // Das Abhaken läuft in einer eigenen Transaktion der Todo-Liste; scheitert es, wird das Ticket wieder entfernt.
    match tasks::set_done(conn, todo_id, true) {
        Ok(op) => conv.todo_op = op,
        Err(e) => {
            let _ = delete_ticket(conn, conv.ticket.id, source);
            return Err(task_err(e));
        }
    }
    conv.ticket = get_ticket(conn, conv.ticket.id)?;
    Ok(conv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{create_field, NewField};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::values::get_value;
    use crate::state::Lang;

    fn ui() -> Source {
        Source::ui()
    }

    fn todo(c: &Connection, title: &str, body: &str, tags: &[&str]) -> i64 {
        tasks::create_item(c, title, body, &tags.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap().0.id
    }

    #[test]
    fn todo_items_appear_as_read_only_tickets_of_the_system_project() {
        let f = Fx::new();
        let a = todo(&f.c, "Einkaufen", "Milch\nBrot", &["Haushalt", "dringend"]);
        let b = todo(&f.c, "", "  \nNur Text\nzweite Zeile", &[]);
        let c = todo(&f.c, "Erledigt", "", &[]);
        tasks::set_done(&f.c, c, true).unwrap();
        let (pid, tid) = todo_project(&f.c).unwrap();
        let all = list_todo_as_tickets(&f.c, &ItemFilter { show_done: true, tags: vec![] }).unwrap();
        assert_eq!(all.len(), 3);
        // oben zuerst (neueste Karte)
        assert_eq!(all.iter().map(|t| t.key.clone()).collect::<Vec<_>>(), vec![format!("TODO-{c}"), format!("TODO-{b}"), format!("TODO-{a}")]);
        let ta = all.iter().find(|t| t.todo_id == a).unwrap();
        assert_eq!((ta.project_id, ta.type_id), (pid, tid));
        assert_eq!(ta.title, "Einkaufen");
        assert_eq!(ta.labels, vec!["Haushalt", "dringend"]);
        assert_eq!((ta.category, ta.done), (StatusCategory::Todo, false));
        let tb = all.iter().find(|t| t.todo_id == b).unwrap();
        assert_eq!(tb.title, "Nur Text", "erste Textzeile");
        let tc = all.iter().find(|t| t.todo_id == c).unwrap();
        assert_eq!((tc.category, tc.done), (StatusCategory::Done, true));
        // Filter wie in der Liste: Erledigte ausblenden
        let open = list_todo_as_tickets(&f.c, &ItemFilter::default()).unwrap();
        assert_eq!(open.len(), 2);
        assert_eq!(todo_ticket(&f.c, a).unwrap().key, format!("TODO-{a}"));
        assert!(todo_ticket(&f.c, 9999).unwrap_err().is_not_found());
        // Der System-Typ heißt in beiden Sprachen Todolist; es gibt keine echten Tickets im System-Projekt
        assert_eq!(crate::planning::types::get_type(&f.c, tid).unwrap().display(Lang::De), "Todolist");
        assert_eq!(crate::planning::ticket_list::list_tickets(&f.c, Some(pid), &Default::default()).unwrap().total, 0);
    }

    #[test]
    fn converts_title_text_and_tags() {
        let f = Fx::new();
        let labels = f.add_field(f.task, "Labels", FieldKind::Labels, Default::default(), false);
        let a = todo(&f.c, "Einkaufen", "Milch\nBrot", &["Haushalt", "dringend"]);
        let conv = convert_todo_to_ticket(&f.c, a, f.project, f.task, &Source::agent("S")).unwrap();
        assert_eq!(conv.ticket.title, "Einkaufen");
        assert_eq!(conv.ticket.key(), "PAN-1");
        let desc = f.field(f.task, "description");
        assert_eq!(get_value(&f.c, conv.ticket.id, &desc).unwrap(), Some(json!("Milch\nBrot")));
        assert_eq!(get_value(&f.c, conv.ticket.id, &labels).unwrap(), Some(json!(["Haushalt", "dringend"])));
        assert!(conv.dropped_tags.is_empty() && !conv.body_dropped);
        // Karte erledigt und als umgewandelt vermerkt
        let t = todo_ticket(&f.c, a).unwrap();
        assert!(t.done);
        assert_eq!(t.converted_to, Some(conv.ticket.id));
        assert_eq!(converted_ticket_of(&f.c, a).unwrap(), Some(conv.ticket.id));
        assert!(conv.todo_op.is_some(), "für die History der Todo-Liste");
        assert!(matches!(conv.op, TicketOp::Created { .. }));
        // nur einmal
        assert_eq!(convert_todo_to_ticket(&f.c, a, f.project, f.task, &ui()).unwrap_err(), TicketError::TodoAlreadyConverted { ticket_id: conv.ticket.id });
        assert_eq!(crate::planning::ticket_list::list_tickets(&f.c, Some(f.project), &Default::default()).unwrap().total, 1);
    }

    #[test]
    fn tags_are_dropped_and_reported_without_a_labels_field() {
        let f = Fx::new();
        let a = todo(&f.c, "T", "", &["x", "y"]);
        let conv = convert_todo_to_ticket(&f.c, a, f.project, f.task, &ui()).unwrap();
        assert_eq!(conv.dropped_tags, vec!["x", "y"]);
        // Typ ohne Description-Feld: Text wird gemeldet
        let plain = crate::planning::types::create_type(&f.c, &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Notiz".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let b = todo(&f.c, "B", "Inhalt", &[]);
        let conv = convert_todo_to_ticket(&f.c, b, f.project, plain, &ui()).unwrap();
        assert!(conv.body_dropped);
        assert!(conv.dropped_tags.is_empty());
        // Description als einfaches Text-Feld mit zu kleiner Grenze: auch gemeldet
        let small = crate::planning::types::create_type(&f.c, &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Klein".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let mut nf = NewField::new(small, "Beschreibung", FieldKind::Text);
        nf.options.max_length = Some(3);
        create_field(&f.c, &nf).unwrap();
        let c = todo(&f.c, "C", "viel zu lang", &[]);
        assert!(convert_todo_to_ticket(&f.c, c, f.project, small, &ui()).unwrap().body_dropped);
    }

    #[test]
    fn title_comes_from_the_first_line_and_body_equal_to_title_is_not_repeated() {
        let f = Fx::new();
        let a = todo(&f.c, "", "\n  Erste Zeile  \nZweite", &[]);
        let conv = convert_todo_to_ticket(&f.c, a, f.project, f.task, &ui()).unwrap();
        assert_eq!(conv.ticket.title, "Erste Zeile");
        let desc = f.field(f.task, "description");
        assert_eq!(get_value(&f.c, conv.ticket.id, &desc).unwrap(), Some(json!("Erste Zeile  
Zweite")));
        // Text = Titel (einzeilige Karte): keine doppelte Beschreibung
        let b = todo(&f.c, "", "Nur eine Zeile", &[]);
        let conv = convert_todo_to_ticket(&f.c, b, f.project, f.task, &ui()).unwrap();
        assert_eq!(get_value(&f.c, conv.ticket.id, &desc).unwrap(), None);
        assert!(!conv.body_dropped);
        // sehr lange erste Zeile wird gekürzt
        let c = todo(&f.c, "", &"x".repeat(400), &[]);
        let conv = convert_todo_to_ticket(&f.c, c, f.project, f.task, &ui()).unwrap();
        assert_eq!(conv.ticket.title.chars().count(), 255);
    }

    #[test]
    fn validates_project_type_and_required_fields_and_leaves_the_card_open_on_errors() {
        let f = Fx::new();
        let a = todo(&f.c, "A", "", &[]);
        let other = f.other_project("OTH");
        let other_type = crate::planning::types::list_types(&f.c, other).unwrap()[0].id;
        assert_eq!(convert_todo_to_ticket(&f.c, a, f.project, other_type, &ui()).unwrap_err(), TicketError::TypeNotInProject);
        let (tp, tt) = todo_project(&f.c).unwrap();
        assert_eq!(convert_todo_to_ticket(&f.c, a, tp, tt, &ui()).unwrap_err(), TicketError::Plan(crate::planning::PlanError::SystemProtected));
        assert!(convert_todo_to_ticket(&f.c, 9999, f.project, f.task, &ui()).unwrap_err().is_not_found());
        assert!(convert_todo_to_ticket(&f.c, a, 9999, f.task, &ui()).unwrap_err().is_not_found());
        let must = f.add_field(f.task, "Ziel", FieldKind::Text, Default::default(), true);
        assert!(matches!(convert_todo_to_ticket(&f.c, a, f.project, f.task, &ui()), Err(TicketError::RequiredMissing(_))));
        assert!(!todo_ticket(&f.c, a).unwrap().done, "Karte bleibt offen");
        assert_eq!(converted_ticket_of(&f.c, a).unwrap(), None);
        let conv = convert_todo_with_values(&f.c, a, f.project, f.task, &[(must.id, json!("x"))], &ui()).unwrap();
        assert_eq!(get_value(&f.c, conv.ticket.id, &must).unwrap(), Some(json!("x")));
        assert!(todo_ticket(&f.c, a).unwrap().done);
    }

    #[test]
    fn an_already_done_card_gets_no_second_check_and_deleting_the_ticket_allows_converting_again() {
        let f = Fx::new();
        let a = todo(&f.c, "A", "", &[]);
        tasks::set_done(&f.c, a, true).unwrap();
        let conv = convert_todo_to_ticket(&f.c, a, f.project, f.task, &ui()).unwrap();
        assert!(conv.todo_op.is_none());
        delete_ticket(&f.c, conv.ticket.id, &ui()).unwrap();
        assert_eq!(converted_ticket_of(&f.c, a).unwrap(), None, "Zuordnung kaskadiert");
        let again = convert_todo_to_ticket(&f.c, a, f.project, f.task, &ui()).unwrap();
        assert_eq!(again.ticket.key(), "PAN-2", "Nummer 1 wird nicht wiederverwendet");
        // die Karte zu löschen entfernt auch die Zuordnung
        tasks::delete_item(&f.c, a).unwrap();
        assert_eq!(converted_ticket_of(&f.c, a).unwrap(), None);
        assert!(get_ticket(&f.c, again.ticket.id).is_ok(), "das Ticket bleibt");
    }

    #[test]
    fn the_todo_check_can_be_undone_with_the_task_history() {
        let f = Fx::new();
        let a = todo(&f.c, "A", "", &[]);
        let conv = convert_todo_to_ticket(&f.c, a, f.project, f.task, &ui()).unwrap();
        let mut h: crate::undo::History<TaskOp> = crate::undo::History::new(5);
        h.push(conv.todo_op.unwrap());
        h.undo(&f.c).unwrap();
        assert!(!todo_ticket(&f.c, a).unwrap().done);
        assert_eq!(todo_ticket(&f.c, a).unwrap().converted_to, Some(conv.ticket.id), "die Zuordnung bleibt");
    }
}
