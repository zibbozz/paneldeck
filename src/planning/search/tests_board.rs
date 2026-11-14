//! Prüfungen der Board-Ansicht ([`board_view`]) und von [`move_card`].

use super::tests::{ks, Sx, TODAY};
use super::*;
use crate::planning::fields::FieldKind;
use crate::planning::history::{history_of, HistoryField, Source, SourceKind};
use crate::planning::ops::TicketOp;
use crate::planning::tickets::{get_ticket, move_rank};
use crate::planning::types::{create_type, NewType};
use crate::planning::views::boards::{create_board, BoardColumn, BoardConfig, NewBoard};
use crate::planning::views::filters::{create_filter, NewFilter};
use crate::planning::workflow::{remove_transition, set_transition, Condition, TransitionDef};
use serde_json::json;

fn col(statuses: &[i64]) -> BoardColumn {
    BoardColumn { title: String::new(), status_ids: statuses.to_vec() }
}

fn board(s: &Sx, cfg: BoardConfig) -> i64 {
    create_board(&s.f.c, &NewBoard { project_id: s.f.project, name: "Board".into(), description: String::new(), config: cfg }).unwrap()
}

/// Board mit einer Spalte je Status: To Do, In Progress, Done.
fn three(s: &Sx) -> i64 {
    board(s, BoardConfig { columns: vec![col(&[s.f.todo]), col(&[s.f.progress]), col(&[s.f.done])], ..BoardConfig::default() })
}

fn column_keys(v: &BoardView, i: usize) -> Vec<String> {
    v.columns[i].tickets.iter().map(|t| t.key.clone()).collect()
}

#[test]
fn view_groups_tickets_by_column_in_rank_order() {
    let s = Sx::new();
    let b = three(&s);
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!(v.columns.len(), 3);
    assert_eq!(column_keys(&v, 0), ks(&[3, 5, 7, 10, 11]));
    assert_eq!(column_keys(&v, 1), ks(&[1, 4, 6, 8]));
    assert_eq!(column_keys(&v, 2), ks(&[2, 9]));
    assert_eq!(v.columns.iter().map(|c| c.total).collect::<Vec<_>>(), vec![5, 4, 2]);
    assert_eq!((v.board_id, v.project_id, v.name.as_str()), (b, s.f.project, "Board"));
}

#[test]
fn column_titles_default_to_the_first_status_name_in_the_language() {
    let s = Sx::new();
    let b = three(&s);
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!(v.columns.iter().map(|c| c.title.clone()).collect::<Vec<_>>(), vec!["To Do", "In Progress", "Done"]);
    let de = Ctx::new(TODAY, Lang::De);
    let v = board_view(&s.f.c, b, &de).unwrap();
    assert_eq!(v.columns.iter().map(|c| c.title.clone()).collect::<Vec<_>>(), vec!["Zu erledigen", "In Arbeit", "Erledigt"]);
}

#[test]
fn custom_column_title_wins() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![BoardColumn { title: "Backlog".into(), status_ids: vec![s.f.todo] }], ..BoardConfig::default() });
    assert_eq!(board_view(&s.f.c, b, &s.ctx).unwrap().columns[0].title, "Backlog");
}

#[test]
fn a_column_may_hold_several_statuses() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo, s.f.progress]), col(&[s.f.done])], ..BoardConfig::default() });
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!(column_keys(&v, 0), ks(&[1, 3, 4, 5, 6, 7, 8, 10, 11]));
    assert_eq!(v.columns[0].title, "To Do");
}

#[test]
fn statuses_without_a_column_are_not_shown() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.done])], ..BoardConfig::default() });
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!((v.columns.len(), column_keys(&v, 0)), (1, ks(&[2, 9])));
}

#[test]
fn a_board_without_columns_is_empty() {
    let s = Sx::new();
    let b = board(&s, BoardConfig::default());
    assert!(board_view(&s.f.c, b, &s.ctx).unwrap().columns.is_empty());
}

#[test]
fn type_filter_limits_the_cards() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo]), col(&[s.f.progress]), col(&[s.f.done])], type_filter: vec![s.f.story], ..BoardConfig::default() });
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!((column_keys(&v, 0), column_keys(&v, 1), column_keys(&v, 2)), (ks(&[3, 5]), ks(&[4]), vec![]));
    assert_eq!(v.columns[2].total, 0);
}

#[test]
fn type_filter_with_two_types() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.done])], type_filter: vec![s.f.epic, s.f.defect], ..BoardConfig::default() });
    assert_eq!(column_keys(&board_view(&s.f.c, b, &s.ctx).unwrap(), 0), ks(&[2, 9]));
}

#[test]
fn source_filter_restricts_the_cards() {
    let s = Sx::new();
    let f = create_filter(&s.f.c, &NewFilter { project_id: Some(s.f.project), name: "Highest".into(), jql: "priority = Highest".into() }).unwrap();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo]), col(&[s.f.progress]), col(&[s.f.done])], source_filter_id: Some(f), ..BoardConfig::default() });
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!((column_keys(&v, 0), column_keys(&v, 1), column_keys(&v, 2)), (vec![], ks(&[6, 8]), ks(&[2])));
}

#[test]
fn source_filter_sorting_is_ignored_and_a_global_filter_works() {
    let s = Sx::new();
    let f = create_filter(&s.f.c, &NewFilter { project_id: None, name: "Epics".into(), jql: "type = Epic ORDER BY priority DESC".into() }).unwrap();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.progress, s.f.done])], source_filter_id: Some(f), ..BoardConfig::default() });
    assert_eq!(column_keys(&board_view(&s.f.c, b, &s.ctx).unwrap(), 0), ks(&[1, 2]), "nach Rang, nicht nach der Sortierung des Filters");
}

#[test]
fn source_filter_and_type_filter_combine() {
    let s = Sx::new();
    let f = create_filter(&s.f.c, &NewFilter { project_id: Some(s.f.project), name: "Login".into(), jql: "summary ~ login".into() }).unwrap();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo, s.f.progress])], source_filter_id: Some(f), type_filter: vec![s.f.defect], ..BoardConfig::default() });
    assert_eq!(column_keys(&board_view(&s.f.c, b, &s.ctx).unwrap(), 0), ks(&[8]));
}

#[test]
fn an_invalid_source_filter_is_reported() {
    let s = Sx::new();
    let f = create_filter(&s.f.c, &NewFilter { project_id: Some(s.f.project), name: "Kaputt".into(), jql: "nonsense = 1".into() }).unwrap();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo])], source_filter_id: Some(f), ..BoardConfig::default() });
    assert!(matches!(board_view(&s.f.c, b, &s.ctx), Err(SearchError::Invalid { .. })));
}

#[test]
fn only_active_sprint_shows_the_cards_of_the_active_sprint() {
    use crate::calc::calendar::WorkCalendar;
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::sprint::{create_sprint, move_tickets_to_sprint, start_sprint, NewSprint};
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo, s.f.progress])], only_active_sprint: true, ..BoardConfig::default() });
    // Projekt ohne Sprints: der Schalter wirkt nicht
    assert_eq!(column_keys(&board_view(&s.f.c, b, &s.ctx).unwrap(), 0), ks(&[1, 3, 4, 5, 6, 7, 8, 10, 11]));
    update_project(&s.f.c, s.f.project, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
    // mit Sprints, aber ohne aktiven Sprint: keine Karten
    let cal = WorkCalendar::default();
    let sp = create_sprint(&s.f.c, &NewSprint::new(s.f.project, "Sprint 1"), &cal).unwrap();
    move_tickets_to_sprint(&s.f.c, sp, &[s.ids[2], s.ids[3], s.ids[1]]).unwrap();
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert!(column_keys(&v, 0).is_empty() && v.columns[0].total == 0);
    // aktiv: PAN-3 (To Do) und PAN-4 (In Progress); PAN-2 (Done) liegt nicht in einer Spalte des Boards
    start_sprint(&s.f.c, sp, &cal).unwrap();
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!(column_keys(&v, 0), ks(&[3, 4]));
    assert_eq!(v.columns[0].total, 2);
    // ohne den Schalter sind es wieder alle
    let cfg = BoardConfig { columns: vec![col(&[s.f.todo, s.f.progress])], only_active_sprint: false, ..BoardConfig::default() };
    let b2 = create_board(&s.f.c, &NewBoard { project_id: s.f.project, name: "Alle".into(), description: String::new(), config: cfg }).unwrap();
    assert_eq!(board_view(&s.f.c, b2, &s.ctx).unwrap().columns[0].total, 9);
}

#[test]
fn unknown_board_is_not_found() {
    let s = Sx::new();
    assert_eq!(board_view(&s.f.c, 9999, &s.ctx).unwrap_err(), SearchError::NotFound);
}

#[test]
fn columns_are_paged_with_a_counter() {
    let s = Sx::new();
    let b = three(&s);
    let v = board_view_with(&s.f.c, b, &s.ctx, &BoardPaging { per_column: 2, offsets: vec![] }).unwrap();
    assert_eq!(column_keys(&v, 0), ks(&[3, 5]));
    assert_eq!(v.columns[0].total, 5);
    assert!(v.columns[0].has_more() && !v.columns[2].has_more());
    let v = board_view_with(&s.f.c, b, &s.ctx, &BoardPaging { per_column: 2, offsets: vec![2, 0, 0] }).unwrap();
    assert_eq!(column_keys(&v, 0), ks(&[7, 10]));
    assert_eq!(v.columns[0].offset, 2);
    let v = board_view_with(&s.f.c, b, &s.ctx, &BoardPaging { per_column: 2, offsets: vec![4] }).unwrap();
    assert_eq!(column_keys(&v, 0), ks(&[11]));
    assert!(!v.columns[0].has_more());
}

#[test]
fn default_page_size_is_fifty() {
    assert_eq!(DEFAULT_COLUMN_LIMIT, 50);
    assert_eq!(BoardPaging::default().per_column, 50);
}

#[test]
fn cards_follow_the_backlog_rank() {
    let s = Sx::new();
    let b = three(&s);
    move_rank(&s.f.c, s.ids[10], Some(s.ids[2])).unwrap().unwrap();
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!(column_keys(&v, 0), ks(&[11, 3, 5, 7, 10]));
}

#[test]
fn cards_carry_the_row_data() {
    let s = Sx::new();
    let b = three(&s);
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    let card = v.columns[1].tickets.iter().find(|t| t.key == "PAN-6").unwrap();
    assert_eq!((card.priority.as_deref(), card.story_points, card.parent_key.as_deref(), card.flagged), (Some("Highest"), Some(2.0), Some("PAN-3"), true));
}

// ---------------------------------------------------------------------------------------------
// move_card
// ---------------------------------------------------------------------------------------------

fn agent() -> Source {
    Source::agent("test")
}

#[test]
fn moving_to_another_column_changes_the_status_and_goes_to_the_end() {
    let s = Sx::new();
    let b = three(&s);
    let r = move_card(&s.f.c, s.ids[2], b, 1, None, &agent()).unwrap();
    assert_eq!((r.status_id, r.status_changed, r.reordered), (Some(s.f.progress), true, true));
    assert!(matches!(r.op, Some(TicketOp::Batch(_))));
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!(column_keys(&v, 1), ks(&[1, 4, 6, 8, 3]));
    assert_eq!(column_keys(&v, 0), ks(&[5, 7, 10, 11]));
    assert_eq!(s.q("changedBy = agent"), ks(&[3, 6]), "die Quelle steht im Verlauf");
}

#[test]
fn moving_before_another_card_orders_it_there() {
    let s = Sx::new();
    let b = three(&s);
    move_card(&s.f.c, s.ids[2], b, 1, Some(s.ids[3]), &Source::ui()).unwrap();
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert_eq!(column_keys(&v, 1), ks(&[1, 3, 4, 6, 8]));
}

#[test]
fn reordering_inside_a_column_keeps_the_status() {
    let s = Sx::new();
    let b = three(&s);
    let r = move_card(&s.f.c, s.ids[10], b, 0, Some(s.ids[2]), &Source::ui()).unwrap();
    assert_eq!((r.status_changed, r.reordered, r.status_id), (false, true, Some(s.f.todo)));
    assert_eq!(column_keys(&board_view(&s.f.c, b, &s.ctx).unwrap(), 0), ks(&[11, 3, 5, 7, 10]));
}

#[test]
fn dropping_a_card_where_it_is_changes_nothing() {
    let s = Sx::new();
    let b = three(&s);
    // PAN-11 ist die letzte Karte der Spalte „To Do“ und hat den höchsten Rang
    let r = move_card(&s.f.c, s.ids[10], b, 0, None, &Source::ui()).unwrap();
    assert_eq!((r.status_changed, r.reordered), (false, false));
    assert!(r.op.is_none());
}

#[test]
fn dropping_on_itself_changes_nothing() {
    let s = Sx::new();
    let b = three(&s);
    let r = move_card(&s.f.c, s.ids[2], b, 0, Some(s.ids[2]), &Source::ui()).unwrap();
    assert!(r.op.is_none());
}

#[test]
fn a_disallowed_transition_is_refused_and_leaves_everything_unchanged() {
    let s = Sx::new();
    let b = three(&s);
    remove_transition(&s.f.c, s.f.story, s.f.todo, s.f.done).unwrap();
    let before = get_ticket(&s.f.c, s.ids[2]).unwrap();
    let e = move_card(&s.f.c, s.ids[2], b, 2, None, &Source::ui()).unwrap_err();
    assert_eq!(e, MoveError::Ticket(crate::planning::ticket_error::TicketError::TransitionNotAllowed));
    assert_eq!(get_ticket(&s.f.c, s.ids[2]).unwrap(), before);
    assert!(!e.message(Lang::De).is_empty() && e.message(Lang::De) != e.message(Lang::En));
}

#[test]
fn the_first_allowed_status_of_a_multi_status_column_is_taken() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo]), col(&[s.f.done, s.f.progress])], ..BoardConfig::default() });
    // erlaubt: To Do → Done (Standard-Matrix) → „Done“ steht zuerst
    let r = move_card(&s.f.c, s.ids[2], b, 1, None, &Source::ui()).unwrap();
    assert_eq!(r.status_id, Some(s.f.done));
    // ohne To Do → Done wird der nächste erlaubte genommen
    remove_transition(&s.f.c, s.f.story, s.f.todo, s.f.done).unwrap();
    let r = move_card(&s.f.c, s.ids[4], b, 1, None, &Source::ui()).unwrap();
    assert_eq!(r.status_id, Some(s.f.progress));
}

#[test]
fn a_card_that_already_has_a_status_of_the_column_keeps_it() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.progress, s.f.done])], ..BoardConfig::default() });
    let r = move_card(&s.f.c, s.ids[1], b, 0, None, &Source::ui()).unwrap();
    assert_eq!((r.status_id, r.status_changed), (Some(s.f.done), false));
}

#[test]
fn required_fields_are_reported_and_can_be_given_in_the_same_operation() {
    let s = Sx::new();
    let b = three(&s);
    let desc = s.f.field(s.f.task, "description");
    set_transition(&s.f.c, s.f.task, &TransitionDef { from_status_id: s.f.progress, to_status_id: s.f.done, required_fields: vec![desc.id], ..Default::default() }).unwrap();
    match move_card(&s.f.c, s.ids[5], b, 2, None, &Source::ui()).unwrap_err() {
        MoveError::NeedsFields { status_id, fields } => {
            assert_eq!(status_id, s.f.done);
            assert_eq!(fields.iter().map(|f| f.id).collect::<Vec<_>>(), vec![desc.id]);
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(get_ticket(&s.f.c, s.ids[5]).unwrap().status_id, Some(s.f.progress));
    let r = move_card_with_values(&s.f.c, s.ids[5], b, 2, None, &[(desc.id, json!("erledigt"))], &Source::ui()).unwrap();
    assert_eq!(r.status_id, Some(s.f.done));
    let v = crate::planning::values::get_value(&s.f.c, s.ids[5], &desc).unwrap();
    assert_eq!(v, Some(json!("erledigt")));
}

#[test]
fn a_needs_fields_message_names_the_fields() {
    let s = Sx::new();
    let b = three(&s);
    let desc = s.f.field(s.f.task, "description");
    set_transition(&s.f.c, s.f.task, &TransitionDef { from_status_id: s.f.progress, to_status_id: s.f.done, required_fields: vec![desc.id], ..Default::default() }).unwrap();
    let e = move_card(&s.f.c, s.ids[5], b, 2, None, &Source::ui()).unwrap_err();
    assert!(e.message(Lang::En).contains("Description") && e.message(Lang::De).contains("Beschreibung"));
}

#[test]
fn a_blocking_condition_refuses_the_move() {
    let s = Sx::new();
    let b = three(&s);
    let sp = s.f.field(s.f.task, "story_points");
    let cond = Condition { field_id: sp.id, op: crate::planning::workflow::ConditionOp::Gt, value: json!(100), allow: true };
    set_transition(&s.f.c, s.f.task, &TransitionDef { from_status_id: s.f.progress, to_status_id: s.f.done, conditions: vec![cond], ..Default::default() }).unwrap();
    let e = move_card(&s.f.c, s.ids[5], b, 2, None, &Source::ui()).unwrap_err();
    assert_eq!(e, MoveError::Ticket(crate::planning::ticket_error::TicketError::ConditionsNotMet));
}

#[test]
fn an_invalid_column_is_refused() {
    let s = Sx::new();
    let b = three(&s);
    assert_eq!(move_card(&s.f.c, s.ids[2], b, 9, None, &Source::ui()).unwrap_err(), MoveError::ColumnInvalid);
}

#[test]
fn unknown_board_or_ticket_is_not_found() {
    let s = Sx::new();
    let b = three(&s);
    assert_eq!(move_card(&s.f.c, 99999, b, 0, None, &Source::ui()).unwrap_err(), MoveError::NotFound);
    assert_eq!(move_card(&s.f.c, s.ids[0], 99999, 0, None, &Source::ui()).unwrap_err(), MoveError::NotFound);
}

#[test]
fn a_ticket_of_another_project_is_not_on_the_board() {
    let s = Sx::new();
    let b = three(&s);
    let other = s.f.other_project("OTH");
    let ot = crate::planning::types::list_types(&s.f.c, other).unwrap()[0].id;
    let (t, _) = crate::planning::tickets::create_ticket(&s.f.c, &crate::planning::tickets::NewTicket::new(other, ot, "Fremd"), &Source::ui()).unwrap();
    assert_eq!(move_card(&s.f.c, t.id, b, 1, None, &Source::ui()).unwrap_err(), MoveError::NotOnBoard);
}

#[test]
fn a_ticket_excluded_by_the_type_filter_is_not_on_the_board() {
    let s = Sx::new();
    let b = board(&s, BoardConfig { columns: vec![col(&[s.f.todo]), col(&[s.f.progress])], type_filter: vec![s.f.story], ..BoardConfig::default() });
    assert_eq!(move_card(&s.f.c, s.ids[5], b, 1, None, &Source::ui()).unwrap_err(), MoveError::NotOnBoard);
    assert!(move_card(&s.f.c, s.ids[2], b, 1, None, &Source::ui()).is_ok());
}

#[test]
fn a_type_without_workflow_cannot_be_moved() {
    let s = Sx::new();
    let b = three(&s);
    let note = create_type(&s.f.c, &NewType { project_id: s.f.project, key: None, name: Some("Notiz".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
    let (t, _) = crate::planning::tickets::create_ticket(&s.f.c, &crate::planning::tickets::NewTicket::new(s.f.project, note, "N"), &Source::ui()).unwrap();
    assert_eq!(move_card(&s.f.c, t.id, b, 1, None, &Source::ui()).unwrap_err(), MoveError::NotOnBoard);
    // und sie erscheint auch nicht auf dem Board
    let v = board_view(&s.f.c, b, &s.ctx).unwrap();
    assert!(v.columns.iter().all(|c| c.tickets.iter().all(|x| x.id != t.id)));
}

#[test]
fn a_failed_reorder_rolls_the_status_change_back() {
    let s = Sx::new();
    let b = three(&s);
    let other = s.f.other_project("OTH");
    let ot = crate::planning::types::list_types(&s.f.c, other).unwrap()[0].id;
    let (foreign, _) = crate::planning::tickets::create_ticket(&s.f.c, &crate::planning::tickets::NewTicket::new(other, ot, "Fremd"), &Source::ui()).unwrap();
    let before = get_ticket(&s.f.c, s.ids[2]).unwrap();
    let hist = history_of(&s.f.c, s.ids[2]).unwrap().len();
    // `before` liegt in einem anderen Projekt → die Reihenfolge scheitert nach dem Statuswechsel
    let e = move_card(&s.f.c, s.ids[2], b, 1, Some(foreign.id), &Source::ui()).unwrap_err();
    assert_eq!(e, MoveError::NotFound);
    assert_eq!(get_ticket(&s.f.c, s.ids[2]).unwrap(), before, "der Statuswechsel wurde zurückgenommen");
    assert_eq!(history_of(&s.f.c, s.ids[2]).unwrap().len(), hist);
}

#[test]
fn moving_writes_the_status_into_the_history_with_the_source() {
    let s = Sx::new();
    let b = three(&s);
    move_card(&s.f.c, s.ids[2], b, 1, None, &Source::agent("Sitzung")).unwrap();
    let h = history_of(&s.f.c, s.ids[2]).unwrap();
    let e = h.iter().rev().find(|e| e.field == HistoryField::Status).unwrap();
    assert_eq!((e.source.kind, e.source.detail.as_deref()), (SourceKind::Agent, Some("Sitzung")));
    assert_eq!(e.new.as_deref(), Some(s.f.progress.to_string().as_str()));
}

#[test]
fn field_kind_import_is_used() {
    // sichert, dass die Feldarten des Tests importiert bleiben (Tests der Board-Karte mit Zahlenfeld)
    assert_eq!(FieldKind::Number.as_str(), "number");
}
