//! Tests der Ticket-CSV: Format, Rundreise, Formel-Schutz, Fehler mit Zeile und Spalte, Atomarität.

use super::tickets_csv::*;
use super::*;
use crate::calc::csv::{read_csv, CsvTable, Delimiter};
use crate::planning::comments::add_comment;
use crate::planning::fields::{FieldKind, FieldOptions, OptionValue};
use crate::planning::history::{describe, history_of, Source, SourceKind};
use crate::planning::projects::{create_project, get_project, NewProject};
use crate::planning::template::{export_types, import_types};
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::{create_ticket, fixture::Fx, set_flagged, set_starred, ticket_by_key, update_ticket, NewTicket, TicketUpdate};
use crate::planning::transition::transition_ticket;
use crate::planning::types::list_types;
use crate::state::Lang;
use rusqlite::Connection;
use serde_json::json;

#[allow(dead_code)]
struct Setup {
    fx: Fx,
    epic: i64,
    story: i64,
    task1: i64,
    task2: i64,
    plain_type: i64,
}

fn setup() -> Setup {
    let fx = Fx::new();
    let c = &fx.c;
    let o = |names: &[&str]| FieldOptions { values: names.iter().map(|n| OptionValue::new(n)).collect(), ..Default::default() };
    let tags = fx.add_field(fx.task, "Tags", FieldKind::Labels, FieldOptions::default(), false);
    let areas = fx.add_field(fx.task, "Areas", FieldKind::Multi, o(&["Backend", "Front|end", "Docs"]), false);
    let reviewed = fx.add_field(fx.task, "Reviewed", FieldKind::Checkbox, FieldOptions::default(), false);
    let hours = fx.add_field(fx.task, "Hours", FieldKind::Number, FieldOptions { min: Some(-10.0), max: Some(100.0), decimals: Some(1), ..Default::default() }, false);
    let due = fx.add_field(fx.task, "Due", FieldKind::Date, FieldOptions::default(), false);
    let at = fx.add_field(fx.task, "Start at", FieldKind::Datetime, FieldOptions::default(), false);
    let note = fx.add_field(fx.task, "Note", FieldKind::Text, FieldOptions::default(), false);
    let related = fx.add_field(fx.task, "Related", FieldKind::Ticket, FieldOptions::default(), false);
    let plain_type = crate::planning::types::create_type(c, &crate::planning::types::NewType { project_id: fx.project, key: None, name: Some("Note".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
    let src = Source::ui();
    let epic = fx.mk(fx.epic, "Epic, one");
    let story = fx.child(&epic, fx.story, "Story \"two\"");
    update_ticket(c, story.id, &TicketUpdate { title: None, values: vec![(fx.field(fx.story, "epic").id, json!(epic.id))] }, &src).unwrap();
    let t1 = fx.child(&story, fx.task, "Task A");
    let t2 = fx.child(&story, fx.task, "Task B");
    update_ticket(
        c,
        t1.id,
        &TicketUpdate {
            title: None,
            values: vec![
                (tags.id, json!(["x|y", "z\\w"])),
                (areas.id, json!([2, 3])),
                (reviewed.id, json!(true)),
                (hours.id, json!(-2.5)),
                (due.id, json!("2026-03-01")),
                (at.id, json!("2026-03-01T09:30:00")),
                (note.id, json!("=SUM(A1)")),
                (related.id, json!(t2.id)),
                (fx.field(fx.task, "description").id, json!("# Titel\n\nZeile 2")),
            ],
        },
        &src,
    )
    .unwrap();
    transition_ticket(c, t1.id, fx.progress, &[], &src).unwrap();
    set_flagged(c, t1.id, true, &src).unwrap();
    set_starred(c, t2.id, true, &src).unwrap();
    add_comment(c, t1.id, "kommentar", &src).unwrap();
    create_ticket(c, &NewTicket::new(fx.project, plain_type, "Plain"), &src).unwrap();
    Setup { epic: epic.id, story: story.id, task1: t1.id, task2: t2.id, plain_type, fx }
}

fn opts() -> CsvOptions {
    CsvOptions::default()
}

fn exp(s: &Setup, o: &CsvOptions) -> String {
    export_tickets_csv(&s.fx.c, s.fx.project, o).unwrap()
}

fn table(text: &str) -> CsvTable {
    read_csv(text, Delimiter::Comma).unwrap()
}

fn table_any(text: &str) -> CsvTable {
    read_csv(text, crate::calc::csv::detect_delimiter(text)).unwrap()
}

fn idx(t: &CsvTable, name: &str) -> usize {
    t.column_index(name).unwrap_or_else(|| panic!("Spalte {name}: {:?}", t.headers))
}

/// Ein zweites Projekt mit derselben Konfiguration (über die Vorlage).
fn clone_project(s: &Setup, key: &str) -> i64 {
    let c = &s.fx.c;
    let b = create_project(c, &NewProject::new(key, "Copy", "#223344")).unwrap();
    import_types(c, b, &export_types(c, s.fx.project).unwrap()).unwrap();
    b
}

fn import_into(c: &Connection, p: i64, text: &str) -> Result<CsvImportReport, BackupError> {
    import_tickets_csv(c, p, text, &CsvImportOptions::default())
}

fn rows_of(e: BackupError) -> Vec<RowProblem> {
    match e {
        BackupError::CsvRows(r) => r,
        other => panic!("CsvRows erwartet, war {other:?}"),
    }
}

fn count_tickets(c: &Connection, p: i64) -> i64 {
    c.query_row("SELECT COUNT(*) FROM tickets WHERE project_id = ?1", [p], |r| r.get(0)).unwrap()
}

const HEAD: &str = "Key,Type,Title,Status,Parent,Flagged,Starred,Created,Updated";

// ------------------------------------------------------------------ Format

#[test]
fn header_has_fixed_columns_then_one_column_per_field_in_config_order() {
    let s = setup();
    let t = table(&exp(&s, &opts()));
    assert_eq!(&t.headers[..9], FIXED_COLUMNS.map(String::from));
    let rest: Vec<&str> = t.headers[9..].iter().map(String::as_str).collect();
    assert_eq!(rest[..4], ["Description", "Due date", "Labels", "Priority"], "eingebaute Felder zuerst");
    assert_eq!(rest.iter().filter(|h| **h == "Due date").count(), 1);
    assert!(rest.contains(&"Epic") && rest.contains(&"Tags") && rest.contains(&"Related") && rest.contains(&"Steps to reproduce"));
    assert_eq!(rest.iter().filter(|h| **h == "Description").count(), 1, "gleiches Feld in mehreren Typen = eine Spalte");
    assert_eq!(rest.iter().filter(|h| **h == "Priority").count(), 1);
    let tags = rest.iter().position(|h| *h == "Tags").unwrap();
    let hours = rest.iter().position(|h| *h == "Hours").unwrap();
    assert!(tags < hours, "Reihenfolge der Konfiguration");
}

#[test]
fn rows_hold_the_fixed_columns() {
    let s = setup();
    let t = table(&exp(&s, &opts()));
    assert_eq!(t.rows.len(), 5);
    let row = |title: &str| t.rows.iter().find(|r| r[2] == title).unwrap_or_else(|| panic!("{title}"));
    let a = row("Task A");
    assert_eq!(a[0], "PAN-3");
    assert_eq!(a[1], "Dev task");
    assert_eq!(a[3], "In Progress");
    assert_eq!(a[4], "PAN-2");
    assert_eq!((a[5].as_str(), a[6].as_str()), ("true", "false"));
    assert!(a[7].len() == 19 && a[7].contains('T') && a[8].len() == 19);
    let b = row("Task B");
    assert_eq!((b[3].as_str(), b[5].as_str(), b[6].as_str()), ("To Do", "false", "true"));
    let epic = row("Epic, one");
    assert_eq!(epic[4], "");
    let plain = row("Plain");
    assert_eq!((plain[1].as_str(), plain[3].as_str()), ("Note", ""), "Typ ohne Workflow: kein Status");
    assert_eq!(row("Story \"two\"")[2], "Story \"two\"");
}

#[test]
fn values_are_written_in_the_documented_formats() {
    let s = setup();
    let t = table(&exp(&s, &opts()));
    let a = t.rows.iter().find(|r| r[2] == "Task A").unwrap();
    let v = |h: &str| a[idx(&t, h)].clone();
    assert_eq!(v("Tags"), "x\\|y|z\\\\w", "| und \\ werden maskiert");
    assert_eq!(v("Areas"), "Front\\|end|Docs");
    assert_eq!(v("Reviewed"), "true");
    assert_eq!(v("Hours"), "-2.5");
    assert_eq!(v("Due"), "2026-03-01");
    assert_eq!(v("Start at"), "2026-03-01T09:30:00");
    assert_eq!(v("Note"), "=SUM(A1)", "ohne Schutz unverändert");
    assert_eq!(v("Related"), "PAN-4", "Ticket-Feld: Schlüssel des Ziels");
    assert_eq!(v("Description"), "# Titel\n\nZeile 2", "Rich-Text mit Zeilenumbrüchen");
    assert_eq!(v("Priority"), "Medium");
    let story = t.rows.iter().find(|r| r[2] == "Story \"two\"").unwrap();
    assert_eq!(story[idx(&t, "Epic")], "PAN-1");
    assert_eq!(story[idx(&t, "Tags")], "", "Felder, die der Typ nicht hat: leer");
}

#[test]
fn rows_follow_the_backlog_order() {
    let s = setup();
    let c = &s.fx.c;
    crate::planning::tickets::move_rank(c, s.task2, Some(s.epic)).unwrap();
    let t = table(&exp(&s, &opts()));
    assert_eq!(t.rows[0][2], "Task B", "Rang vor dem Epic");
}

#[test]
fn semicolon_and_bom_options_are_honoured_and_read_back() {
    let s = setup();
    let o = CsvOptions { delimiter: Delimiter::Semicolon, bom: true, excel_safe: false };
    let text = exp(&s, &o);
    assert!(text.starts_with('\u{feff}'));
    assert!(text.lines().next().unwrap().contains(";Type;"));
    let b = clone_project(&s, "BB");
    let rep = import_into(&s.fx.c, b, &text).unwrap();
    assert_eq!(rep.created, 5);
    let again = export_tickets_csv(&s.fx.c, b, &o).unwrap();
    assert_eq!(table_any(&again).rows.len(), 5);
}

#[test]
fn empty_project_exports_only_the_header_and_system_project_is_refused() {
    let c = crate::db::open_memory();
    let p = create_project(&c, &NewProject::new("EM", "Empty", "#010203")).unwrap();
    let text = export_tickets_csv(&c, p, &opts()).unwrap();
    let t = table(&text);
    assert_eq!(t.headers.len(), 9);
    assert!(t.rows.is_empty());
    assert_eq!(import_into(&c, p, &text).unwrap().created, 0);
    let todo = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
    assert_eq!(export_tickets_csv(&c, todo, &opts()).unwrap_err(), BackupError::Plan(crate::planning::PlanError::SystemProtected));
    assert_eq!(import_into(&c, todo, &text).unwrap_err(), BackupError::Plan(crate::planning::PlanError::SystemProtected));
}

#[test]
fn colliding_column_names_get_a_kind_suffix() {
    let fx = Fx::new();
    fx.add_field(fx.task, "Title", FieldKind::Text, FieldOptions::default(), false);
    fx.add_field(fx.task, "Parent", FieldKind::Ticket, FieldOptions::default(), false);
    fx.add_field(fx.task, "Size", FieldKind::Text, FieldOptions::default(), false);
    fx.add_field(fx.defect, "Size", FieldKind::Number, FieldOptions::default(), false);
    fx.add_field(fx.story, "size", FieldKind::Checkbox, FieldOptions::default(), false);
    let t = table(&export_tickets_csv(&fx.c, fx.project, &opts()).unwrap());
    let h: Vec<&str> = t.headers.iter().map(String::as_str).collect();
    assert!(h.contains(&"Title [text]") && h.contains(&"Parent [ticket]"));
    assert!(h.contains(&"size") && h.contains(&"Size [text]") && h.contains(&"Size [number]"), "{h:?}");
    let mut lower: Vec<String> = h.iter().map(|x| x.to_lowercase()).collect();
    lower.sort();
    lower.dedup();
    assert_eq!(lower.len(), h.len(), "alle Überschriften eindeutig");
    // die Datei lässt sich zurücklesen
    let p2 = create_project(&fx.c, &NewProject::new("BB", "B", "#223344")).unwrap();
    import_types(&fx.c, p2, &export_types(&fx.c, fx.project).unwrap()).unwrap();
    let text = export_tickets_csv(&fx.c, fx.project, &opts()).unwrap();
    assert_eq!(import_into(&fx.c, p2, &text).unwrap().created, 0);
}

// ------------------------------------------------------------------ Formel-Schutz

#[test]
fn excel_safe_prefixes_formula_cells_only_when_asked() {
    let s = setup();
    let plain = table(&exp(&s, &opts()));
    let safe = table(&exp(&s, &CsvOptions { excel_safe: true, ..opts() }));
    let a = |t: &CsvTable, h: &str| t.rows.iter().find(|r| r[2] == "Task A").unwrap()[idx(t, h)].clone();
    assert_eq!(a(&plain, "Note"), "=SUM(A1)");
    assert_eq!(a(&safe, "Note"), "'=SUM(A1)");
    assert_eq!(a(&safe, "Hours"), "'-2.5", "auch negative Zahlen beginnen mit -");
    assert_eq!(a(&safe, "Due"), "2026-03-01", "nichts anderes wird verändert");
    assert_eq!(a(&safe, "Tags"), a(&plain, "Tags"));
    assert!(safe.headers.iter().all(|h| !h.starts_with('\'')));
    // Zellen, die mit @ oder + beginnen
    let c = &s.fx.c;
    let note = crate::planning::fields::list_fields(c, s.fx.task).unwrap().into_iter().find(|f| f.name.as_deref() == Some("Note")).unwrap();
    update_ticket(c, s.task2, &TicketUpdate { title: Some("@evil".into()), values: vec![(note.id, json!("+1+1"))] }, &Source::ui()).unwrap();
    let safe = table(&exp(&s, &CsvOptions { excel_safe: true, ..opts() }));
    let b = safe.rows.iter().find(|r| r[2] == "'@evil").unwrap();
    assert_eq!(b[idx(&safe, "Note")], "'+1+1");
}

#[test]
fn excel_safe_export_imports_back_to_the_original_values() {
    let s = setup();
    let c = &s.fx.c;
    let note = crate::planning::fields::list_fields(c, s.fx.task).unwrap().into_iter().find(|f| f.name.as_deref() == Some("Note")).unwrap();
    update_ticket(c, s.task2, &TicketUpdate { title: Some("=1+1".into()), values: vec![(note.id, json!("'=schon mit Apostroph"))] }, &Source::ui()).unwrap();
    let text = exp(&s, &CsvOptions { excel_safe: true, ..opts() });
    let b = clone_project(&s, "BB");
    import_into(c, b, &text).unwrap();
    let tb = table(&export_tickets_csv(c, b, &opts()).unwrap());
    let row = |title: &str| tb.rows.iter().find(|r| r[2] == title).unwrap_or_else(|| panic!("{title}"));
    assert_eq!(row("Task A")[idx(&tb, "Note")], "=SUM(A1)");
    assert_eq!(row("Task A")[idx(&tb, "Hours")], "-2.5");
    assert_eq!(row("=1+1")[idx(&tb, "Note")], "'=schon mit Apostroph");
}

#[test]
fn unprotect_can_be_switched_off() {
    let s = setup();
    let b = clone_project(&s, "BB");
    // Ohne Entfernen bleibt das Apostroph in den Werten (und eine geschützte Zahl ist keine Zahl mehr)
    let text = exp(&s, &CsvOptions { excel_safe: true, ..opts() });
    let probs = rows_of(import_tickets_csv(&s.fx.c, b, &text, &CsvImportOptions { unprotect: false }).unwrap_err());
    assert!(probs.iter().any(|p| p.column == "Hours" && matches!(p.issue, RowIssue::CellInvalid(_))));
    let plain = exp(&s, &opts());
    assert!(import_tickets_csv(&s.fx.c, b, &plain, &CsvImportOptions { unprotect: false }).is_ok());
}

// ------------------------------------------------------------------ Rundreise

#[test]
fn csv_roundtrip_into_a_project_with_the_same_configuration() {
    let s = setup();
    let c = &s.fx.c;
    let before = exp(&s, &opts());
    let a = table(&before);
    let b = clone_project(&s, "BB");
    let rep = import_into(c, b, &before).unwrap();
    assert_eq!(rep.created, 5);
    assert_eq!(rep.keys.len(), 5);
    assert!(rep.keys.iter().all(|(old, new)| old.starts_with("PAN-") && new.starts_with("BB-")));
    let bt = table(&export_tickets_csv(c, b, &opts()).unwrap());
    assert_eq!(bt.headers, a.headers);
    assert_eq!(bt.rows.len(), a.rows.len());
    let key_title = |t: &CsvTable, k: &str| t.rows.iter().find(|r| r[0] == k).map(|r| r[2].clone()).unwrap_or_default();
    for (ra, rb) in a.rows.iter().zip(&bt.rows) {
        assert_eq!(ra[1..4], rb[1..4]);
        let refs = [idx(&a, "Related"), idx(&a, "Epic")];
        for i in 5..ra.len() {
            if !refs.contains(&i) {
                assert_eq!(ra[i], rb[i], "{} / {}", ra[2], a.headers[i]);
            }
        }
        assert_eq!(key_title(&a, &ra[4]), key_title(&bt, &rb[4]), "Eltern zeigen auf dasselbe Ticket");
        for i in refs {
            assert_eq!(key_title(&a, &ra[i]), key_title(&bt, &rb[i]), "{} von {}", a.headers[i], ra[2]);
        }
    }
    assert_eq!(exp(&s, &opts()), before, "die Quelle ist unverändert");
}

#[test]
fn import_creates_new_tickets_with_new_numbers_and_notes_the_old_key() {
    let s = setup();
    let c = &s.fx.c;
    let text = exp(&s, &opts());
    let before = count_tickets(c, s.fx.project);
    let rep = import_into(c, s.fx.project, &text).unwrap();
    assert_eq!(count_tickets(c, s.fx.project), before + 5);
    assert_eq!(rep.keys[0], ("PAN-1".to_string(), "PAN-6".to_string()), "Datei-Reihenfolge, neue Nummern");
    let new = ticket_by_key(c, "PAN-8").unwrap();
    let h = history_of(c, new.id).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].source.kind, SourceKind::Import);
    assert!(describe(c, &h[0], Lang::En).contains("key in the file: PAN-3"), "{}", describe(c, &h[0], Lang::En));
    assert!(describe(c, &h[0], Lang::De).contains("Schlüssel in der Datei: PAN-3"));
    // Vorhandene Tickets sind unverändert
    assert_eq!(ticket_by_key(c, "PAN-3").unwrap().title, "Task A");
    assert_eq!(table(&exp(&s, &opts())).rows[..5], table(&text).rows[..5]);
}

#[test]
fn parents_and_ticket_fields_resolve_inside_the_file_in_any_order() {
    let s = setup();
    let c = &s.fx.c;
    let mut t = table(&exp(&s, &opts()));
    t.rows.reverse();
    let reversed = crate::calc::csv::write_csv(&t, &Default::default()).unwrap();
    let b = clone_project(&s, "BB");
    let rep = import_into(c, b, &reversed).unwrap();
    assert_eq!(rep.created, 5);
    let get = |title: &str| -> (i64, Option<i64>) { c.query_row("SELECT id, parent_id FROM tickets WHERE project_id = ?1 AND title = ?2", rusqlite::params![b, title], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() };
    assert_eq!(get("Task A").1, Some(get("Story \"two\"").0));
    assert_eq!(get("Story \"two\"").1, Some(get("Epic, one").0));
    let ranks: Vec<String> = {
        let mut st = c.prepare("SELECT title FROM tickets WHERE project_id = ?1 ORDER BY rank").unwrap();
        st.query_map([b], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect()
    };
    assert_eq!(ranks, t.rows.iter().map(|r| r[2].clone()).collect::<Vec<_>>(), "Rang = Reihenfolge der Datei");
}

#[test]
fn flags_stars_and_timestamps_are_taken_from_the_file() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let text = format!("{HEAD}\n,Epic,E1,,,true,true,2020-01-02T03:04:05,2021-02-03T04:05:06\n,Epic,E2,,,,,,\n,Epic,E3,,,,,2022-05-05T05:05:05,\n");
    import_into(c, b, &text).unwrap();
    let row = |title: &str| -> (bool, bool, String, String) {
        c.query_row("SELECT flagged, starred, created_at, updated_at FROM tickets WHERE project_id = ?1 AND title = ?2", rusqlite::params![b, title], |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?))).unwrap()
    };
    assert_eq!(row("E1"), (true, true, "2020-01-02T03:04:05".into(), "2021-02-03T04:05:06".into()));
    let e2 = row("E2");
    assert!(!e2.0 && !e2.1 && e2.2 == e2.3 && e2.2.len() == 19, "ohne Angabe: jetzt");
    let e3 = row("E3");
    assert_eq!((e3.2.as_str(), e3.3.as_str()), ("2022-05-05T05:05:05", "2022-05-05T05:05:05"), "nur Created: Updated = Created");
    assert_eq!(get_project(c, b).unwrap().next_number, 4);
}

#[test]
fn german_names_and_both_line_endings_are_accepted() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let text = format!("{HEAD},Priority\r\n,Dev-Aufgabe,Eins,Erledigt,,,,,,Höchste\r\n,Dev task,\"Zwei\nmit Umbruch\",In progress,,,,,,Medium\n");
    let rep = import_into(c, b, &text).unwrap();
    assert_eq!(rep.created, 2);
    let t = table(&export_tickets_csv(c, b, &opts()).unwrap());
    let r1 = t.rows.iter().find(|r| r[2] == "Eins").unwrap();
    assert_eq!((r1[1].as_str(), r1[3].as_str(), r1[idx(&t, "Priority")].as_str()), ("Dev task", "Done", "Highest"));
    assert!(t.rows.iter().any(|r| r[2] == "Zwei mit Umbruch"), "Titel ohne Zeilenumbruch");
}

#[test]
fn empty_cells_mean_empty_but_absent_columns_use_the_default() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    import_into(c, b, &format!("{HEAD},Priority\n,Dev task,Leer,,,,,,,\n")).unwrap();
    import_into(c, b, &format!("{HEAD}\n,Dev task,Standard,,,,,,\n")).unwrap();
    let t = table(&export_tickets_csv(c, b, &opts()).unwrap());
    let p = idx(&t, "Priority");
    assert_eq!(t.rows.iter().find(|r| r[2] == "Leer").unwrap()[p], "");
    assert_eq!(t.rows.iter().find(|r| r[2] == "Standard").unwrap()[p], "Medium");
    assert_eq!(t.rows.iter().find(|r| r[2] == "Standard").unwrap()[3], "To Do", "Startstatus aus dem Workflow");
}

// ------------------------------------------------------------------ Kopfzeile

#[test]
fn foreign_headers_are_rejected() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let e = import_into(c, b, "Issue key,Summary,Status\nX-1,Hallo,Open\n").unwrap_err();
    assert_eq!(e, BackupError::CsvHeader(HeaderProblem::Fixed { column: 1, expected: "Key".into(), found: "Issue key".into() }));
    let e = import_into(c, b, "Key,Title,Type,Status,Parent,Flagged,Starred,Created,Updated\n").unwrap_err();
    assert!(matches!(e, BackupError::CsvHeader(HeaderProblem::Fixed { column: 2, .. })));
    let e = import_into(c, b, "Key,Type\n").unwrap_err();
    assert!(matches!(e, BackupError::CsvHeader(HeaderProblem::Fixed { column: 3, ref found, .. }) if found.is_empty()));
    let e = import_into(c, b, &format!("{HEAD},Sprint\n")).unwrap_err();
    assert_eq!(e, BackupError::CsvHeader(HeaderProblem::Unknown { column: 10, name: "Sprint".into() }));
    let e = import_into(c, b, &format!("{HEAD},Priority,Priority\n")).unwrap_err();
    assert_eq!(e, BackupError::CsvHeader(HeaderProblem::Duplicate { column: 11, name: "Priority".into() }));
    assert!(matches!(import_into(c, b, "key,Type,Title,Status,Parent,Flagged,Starred,Created,Updated\n").unwrap_err(), BackupError::CsvHeader(_)), "genau diese Schreibweise");
    let e = import_into(c, b, "a,b\n").unwrap_err();
    assert_ne!(e.message(Lang::De), e.message(Lang::En));
    assert_eq!(count_tickets(c, b), 0);
}

#[test]
fn broken_csv_is_reported_with_line_and_column() {
    let s = setup();
    let b = clone_project(&s, "BB");
    let e = import_into(&s.fx.c, b, "").unwrap_err();
    assert!(matches!(e, BackupError::Csv(_)));
    let e = import_into(&s.fx.c, b, &format!("{HEAD}\n,Epic,\"offen\n")).unwrap_err();
    match &e {
        BackupError::Csv(x) => assert_eq!(x.line, 2),
        o => panic!("{o:?}"),
    }
    assert!(e.message(Lang::En).starts_with("Line 2, column"));
    assert!(e.message(Lang::De).starts_with("Zeile 2, Spalte"));
    let e = import_into(&s.fx.c, b, &format!("{HEAD}\n,Epic,zu wenig\n")).unwrap_err();
    assert!(matches!(e, BackupError::Csv(_)));
}

// ------------------------------------------------------------------ Zeilenfehler

#[test]
fn row_errors_name_row_and_column_and_nothing_is_created() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let cases: Vec<(&str, &str, RowIssue)> = vec![
        (",Zzz,T,,,,,,", "Type", RowIssue::TypeUnknown("Zzz".into())),
        (",,T,,,,,,", "Type", RowIssue::TypeMissing),
        (",Epic,T,Nope,,,,,", "Status", RowIssue::StatusUnknown("Nope".into())),
        (",Epic,T,,,maybe,,,", "Flagged", RowIssue::BoolInvalid("maybe".into())),
        (",Epic,T,,,,yes,,", "Starred", RowIssue::BoolInvalid("yes".into())),
        (",Epic,T,,,,,gestern,", "Created", RowIssue::TimestampInvalid("gestern".into())),
        (",Epic,T,,,,,,2026-01-01", "Updated", RowIssue::TimestampInvalid("2026-01-01".into())),
        (",Epic,,,,,,,", "Title", RowIssue::Ticket(TicketError::TitleEmpty)),
        ("PAN,Epic,T,,,,,,", "Key", RowIssue::KeyInvalid("PAN".into())),
        (",Epic,T,,PAN-99,,,,", "Parent", RowIssue::ParentUnknown("PAN-99".into())),
        (",Epic,T,,x,,,,", "Parent", RowIssue::KeyInvalid("x".into())),
        (",Note,T,Done,,,,,", "Status", RowIssue::StatusNotAllowed("Done".into())),
    ];
    for (line, col, issue) in cases {
        let probs = rows_of(import_into(c, b, &format!("{HEAD}\n{line}\n")).unwrap_err());
        assert_eq!(probs.len(), 1, "{line}: {probs:?}");
        let p = &probs[0];
        assert_eq!((p.row, p.column.as_str(), &p.issue), (2, col, &issue), "{line}");
        assert_ne!(p.message(Lang::De), p.message(Lang::En));
        assert_eq!(count_tickets(c, b), 0, "nichts angelegt: {line}");
    }
}

#[test]
fn field_cell_errors() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let h = format!("{HEAD},Hours,Reviewed,Areas,Related,Priority,Epic");
    let cases: Vec<(&str, &str, RowIssue)> = vec![
        (",Dev task,T,,,,,,,abc,,,,,", "Hours", RowIssue::CellInvalid("abc".into())),
        (",Dev task,T,,,,,,,,maybe,,,,", "Reviewed", RowIssue::BoolInvalid("maybe".into())),
        (",Dev task,T,,,,,,,,,Nowhere,,,", "Areas", RowIssue::OptionUnknown("Nowhere".into())),
        (",Dev task,T,,,,,,,,,Backend|Nowhere,,,", "Areas", RowIssue::OptionUnknown("Nowhere".into())),
        (",Dev task,T,,,,,,,,,,PAN-77,,", "Related", RowIssue::TicketRefUnknown("PAN-77".into())),
        (",Dev task,T,,,,,,,,,,,Urgent,", "Priority", RowIssue::OptionUnknown("Urgent".into())),
        (",Dev task,T,,,,,,,,,,,,PAN-1", "Epic", RowIssue::FieldNotInType("Dev task".into())),
        (",Dev task,T,,,,,,,,,,xyz,,", "Related", RowIssue::CellInvalid("xyz".into())),
    ];
    for (line, col, issue) in cases {
        let probs = rows_of(import_into(c, b, &format!("{h}\n{line}\n")).unwrap_err());
        assert_eq!(probs.len(), 1, "{line}: {probs:?}");
        assert_eq!((probs[0].column.as_str(), &probs[0].issue), (col, &issue), "{line}");
    }
    // Werte außerhalb der Grenzen weist die Datenschicht ab (Zeile als Ganzes)
    let probs = rows_of(import_into(c, b, &format!("{h}\n,Dev task,T,,,,,,,500,,,,,\n")).unwrap_err());
    assert!(matches!(probs[0].issue, RowIssue::Ticket(TicketError::ValueInvalid(_))), "{probs:?}");
    assert_eq!(count_tickets(c, b), 0);
}

#[test]
fn several_problems_are_collected_up_to_a_limit() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let text = format!("{HEAD}\n,Epic,ok,,,,,,\n,Zzz,a,,,,,,\n,Epic,b,Nope,,,,,\n,Epic,c,,,maybe,,,\n");
    let probs = rows_of(import_into(c, b, &text).unwrap_err());
    assert_eq!(probs.iter().map(|p| (p.row, p.column.as_str())).collect::<Vec<_>>(), vec![(3, "Type"), (4, "Status"), (5, "Flagged")]);
    let mut many = format!("{HEAD}\n");
    for _ in 0..200 {
        many += ",Zzz,x,,,,,,\n";
    }
    let probs = rows_of(import_into(c, b, &many).unwrap_err());
    assert_eq!(probs.len(), MAX_ROW_PROBLEMS_FOR_TEST);
    let msg = BackupError::CsvRows(probs).message(Lang::En);
    assert!(msg.contains("further problems"));
    assert_eq!(count_tickets(c, b), 0);
}

const MAX_ROW_PROBLEMS_FOR_TEST: usize = super::error::MAX_ROW_PROBLEMS;

#[test]
fn duplicate_keys_and_parent_cycles_are_errors() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let dup = format!("{HEAD}\nA-1,Epic,x,,,,,,\nA-1,Epic,y,,,,,,\n");
    let probs = rows_of(import_into(c, b, &dup).unwrap_err());
    assert_eq!(probs[0].issue, RowIssue::KeyDuplicate("A-1".into()));
    assert_eq!((probs[0].row, probs[0].column.as_str()), (3, "Key"));
    let cyc = format!("{HEAD}\nA-1,Dev task,x,,A-2,,,,\nA-2,Dev task,y,,A-1,,,,\n");
    let probs = rows_of(import_into(c, b, &cyc).unwrap_err());
    assert_eq!(probs.len(), 2);
    assert!(probs.iter().all(|p| p.issue == RowIssue::ParentCycle));
    let me = format!("{HEAD}\nA-1,Dev task,x,,A-1,,,,\n");
    assert_eq!(rows_of(import_into(c, b, &me).unwrap_err())[0].issue, RowIssue::ParentCycle);
    assert_eq!(count_tickets(c, b), 0);
}

#[test]
fn hierarchy_rules_of_the_data_layer_apply() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let text = format!("{HEAD}\nA-1,Dev task,x,,,,,,\nA-2,Epic,y,,A-1,,,,\n");
    let probs = rows_of(import_into(c, b, &text).unwrap_err());
    assert_eq!((probs[0].row, probs[0].issue.clone()), (3, RowIssue::Ticket(TicketError::ParentNotAllowed)));
    assert_eq!(count_tickets(c, b), 0, "auch das schon angelegte A-1 bleibt nicht zurück");
    assert_eq!(get_project(c, b).unwrap().next_number, 1);
}

#[test]
fn required_fields_are_enforced_like_in_the_ui() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let dev = list_types(c, b).unwrap().into_iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
    let note = crate::planning::fields::list_fields(c, dev).unwrap().into_iter().find(|f| f.name.as_deref() == Some("Note")).unwrap();
    crate::planning::fields::update_field(c, note.id, &crate::planning::fields::UpdateField { required: Some(true), ..Default::default() }).unwrap();
    let probs = rows_of(import_into(c, b, &format!("{HEAD},Note\n,Dev task,x,,,,,,,\n")).unwrap_err());
    assert!(matches!(&probs[0].issue, RowIssue::Ticket(TicketError::RequiredMissing(v)) if v.len() == 1));
    assert_eq!(probs[0].row, 2);
    assert!(probs[0].message(Lang::De).contains("Pflichtfelder"));
    assert_eq!(import_into(c, b, &format!("{HEAD},Note\n,Dev task,x,,,,,,,ok\n")).unwrap().created, 1);
}

/// Projekt mit Sprints, Versionen und Meilensteinen und Feldern darauf.
struct Planned {
    fx: Fx,
    t1: i64,
    t2: i64,
    s_plan: i64,
    s_active: i64,
}

fn planned() -> Planned {
    use crate::calc::calendar::WorkCalendar;
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::releases::{create_milestone, create_version, NewRelease};
    use crate::planning::sprints::sprint::{create_sprint, move_tickets_to_sprint, start_sprint, NewSprint};
    let fx = Fx::new();
    let c = &fx.c;
    update_project(c, fx.project, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
    let rel = fx.add_field(fx.task, "Release", FieldKind::Version, FieldOptions::default(), false);
    let goal = fx.add_field(fx.task, "Goal", FieldKind::Milestone, FieldOptions::default(), false);
    let v1 = create_version(c, &NewRelease::new(fx.project, "1.0")).unwrap();
    create_version(c, &NewRelease::new(fx.project, "2.0")).unwrap();
    let m1 = create_milestone(c, &NewRelease::new(fx.project, "Alpha")).unwrap();
    let t1 = fx.mk(fx.task, "A");
    let t2 = fx.mk(fx.task, "B");
    update_ticket(c, t1.id, &TicketUpdate { title: None, values: vec![(rel.id, json!(v1)), (goal.id, json!(m1))] }, &Source::ui()).unwrap();
    let cal = WorkCalendar::default();
    let d = |m: u32, day: u32| chrono::NaiveDate::from_ymd_opt(2026, m, day).unwrap();
    let s_active = create_sprint(c, &NewSprint { project_id: fx.project, name: "Sprint One".into(), goal: String::new(), start: Some(d(11, 2)), end: Some(d(11, 13)), default_days: None }, &cal).unwrap();
    start_sprint(c, s_active, &cal).unwrap();
    let s_plan = create_sprint(c, &NewSprint::new(fx.project, "Next"), &cal).unwrap();
    move_tickets_to_sprint(c, s_active, &[t1.id]).unwrap();
    move_tickets_to_sprint(c, s_plan, &[t2.id]).unwrap();
    Planned { t1: t1.id, t2: t2.id, s_plan, s_active, fx }
}

/// Ein zweites Projekt mit derselben Struktur, denselben Versionen, Meilensteinen und Sprints.
fn clone_planned(s: &Planned, key: &str) -> i64 {
    use crate::calc::calendar::WorkCalendar;
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::releases::{create_milestone, create_version, NewRelease};
    use crate::planning::sprints::sprint::{create_sprint, NewSprint};
    let c = &s.fx.c;
    let b = create_project(c, &NewProject::new(key, "Copy", "#223344")).unwrap();
    import_types(c, b, &export_types(c, s.fx.project).unwrap()).unwrap();
    update_project(c, b, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
    create_version(c, &NewRelease::new(b, "1.0")).unwrap();
    create_version(c, &NewRelease::new(b, "2.0")).unwrap();
    create_milestone(c, &NewRelease::new(b, "Alpha")).unwrap();
    create_sprint(c, &NewSprint::new(b, "Sprint One"), &WorkCalendar::default()).unwrap();
    create_sprint(c, &NewSprint::new(b, "Next"), &WorkCalendar::default()).unwrap();
    b
}

#[test]
fn version_milestone_and_sprint_columns_are_exported_as_names() {
    let s = planned();
    let t = table(&export_tickets_csv(&s.fx.c, s.fx.project, &opts()).unwrap());
    assert_eq!(t.headers[9], "Sprint", "zehnte, optionale Spalte");
    assert!(t.headers.contains(&"Release".to_string()) && t.headers.contains(&"Goal".to_string()));
    let a = t.rows.iter().find(|r| r[2] == "A").unwrap();
    assert_eq!((a[9].as_str(), a[idx(&t, "Release")].as_str(), a[idx(&t, "Goal")].as_str()), ("Sprint One", "1.0", "Alpha"));
    let b = t.rows.iter().find(|r| r[2] == "B").unwrap();
    assert_eq!((b[9].as_str(), b[idx(&t, "Release")].as_str()), ("Next", ""));
    let _ = (s.t1, s.t2);
}

#[test]
fn sprint_column_exists_only_for_projects_with_sprints() {
    let s = setup();
    let t = table(&exp(&s, &opts()));
    assert!(!t.headers.contains(&"Sprint".to_string()));
    assert_eq!(t.headers[9], "Description");
    // ein Feld namens „Sprint“ bekommt einen Zusatz und kollidiert nie mit der Spalte
    let fx = Fx::new();
    fx.add_field(fx.task, "Sprint", FieldKind::Text, FieldOptions::default(), false);
    let t = table(&export_tickets_csv(&fx.c, fx.project, &opts()).unwrap());
    assert!(t.headers.contains(&"Sprint [text]".to_string()) && !t.headers.contains(&"Sprint".to_string()));
}

#[test]
fn versions_milestones_and_sprints_roundtrip_by_name() {
    let s = planned();
    let c = &s.fx.c;
    let text = export_tickets_csv(c, s.fx.project, &opts()).unwrap();
    let b = clone_planned(&s, "BB");
    let rep = import_into(c, b, &text).unwrap();
    assert_eq!(rep.created, 2);
    assert!(rep.warnings.is_empty());
    let again = table(&export_tickets_csv(c, b, &opts()).unwrap());
    let a = table(&text);
    assert_eq!(again.headers, a.headers);
    for (x, y) in a.rows.iter().zip(&again.rows) {
        assert_eq!(x[1..].to_vec(), y[1..].to_vec(), "{}", x[2]);
    }
    // Die Zugehörigkeit steckt wirklich im Sprint des Zielprojekts, mit den Punkten zum Zeitpunkt des Hinzufügens
    let sprint_of = |title: &str| -> String {
        c.query_row(
            "SELECT s.name FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id JOIN tickets t ON t.id = st.ticket_id WHERE t.project_id = ?1 AND t.title = ?2 AND st.removed_at IS NULL",
            rusqlite::params![b, title],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!((sprint_of("A"), sprint_of("B")), ("Sprint One".to_string(), "Next".to_string()));
    // Die Version steht in der Zielprojekt-Version (andere Kennung als im Quellprojekt)
    let ver: String = c.query_row("SELECT v.name FROM ticket_values tv JOIN versions v ON v.id = CAST(tv.text AS INTEGER) JOIN tickets t ON t.id = tv.ticket_id WHERE t.project_id = ?1 AND t.title = 'A'", [b], |r| r.get(0)).unwrap();
    assert_eq!(ver, "1.0");
}

#[test]
fn unknown_releases_and_sprints_are_row_errors() {
    let s = planned();
    let c = &s.fx.c;
    let b = clone_planned(&s, "BB");
    let h = format!("{HEAD},Sprint,Release,Goal");
    let cases: Vec<(&str, &str, RowIssue)> = vec![
        (",Dev task,T,,,,,,,Nowhere,,", "Sprint", RowIssue::SprintUnknown("Nowhere".into())),
        (",Dev task,T,,,,,,,,9.9,", "Release", RowIssue::ReleaseUnknown("9.9".into())),
        (",Dev task,T,,,,,,,,,Omega", "Goal", RowIssue::ReleaseUnknown("Omega".into())),
    ];
    for (line, col, issue) in cases {
        let probs = rows_of(import_into(c, b, &format!("{h}\n{line}\n")).unwrap_err());
        assert_eq!(probs.len(), 1, "{line}");
        assert_eq!((probs[0].row, probs[0].column.as_str(), &probs[0].issue), (2, col, &issue));
        assert_ne!(probs[0].message(Lang::De), probs[0].message(Lang::En));
    }
    // Namen werden ohne Beachtung der Groß-/Kleinschreibung gefunden
    assert_eq!(import_into(c, b, &format!("{h}\n,Dev task,ok,,,,,,,sprint one,1.0,alpha\n")).unwrap().created, 1);
    // Die Spalten dürfen auch fehlen: das Ticket kommt ins Backlog
    assert_eq!(import_into(c, b, &format!("{HEAD}\n,Dev task,backlog,,,,,,\n")).unwrap().created, 1);
    let in_sprint: i64 = c.query_row("SELECT COUNT(*) FROM sprint_tickets st JOIN tickets t ON t.id = st.ticket_id WHERE t.project_id = ?1 AND t.title = 'backlog'", [b], |r| r.get(0)).unwrap();
    assert_eq!(in_sprint, 0);
}

#[test]
fn a_completed_sprint_and_a_project_without_sprints_are_refused() {
    use crate::planning::sprints::sprint::{complete_sprint, Leftover};
    let s = planned();
    let c = &s.fx.c;
    let b = clone_planned(&s, "BB");
    // Abgeschlossener Sprint im Zielprojekt (aktivieren und abschließen)
    let cal = crate::calc::calendar::WorkCalendar::default();
    let sprint_one: i64 = c.query_row("SELECT id FROM sprints WHERE project_id = ?1 AND name = 'Sprint One'", [b], |r| r.get(0)).unwrap();
    crate::planning::sprints::sprint::update_sprint(
        c,
        sprint_one,
        &crate::planning::sprints::sprint::UpdateSprint { start: Some(Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap())), end: Some(Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 13).unwrap())), ..Default::default() },
    )
    .unwrap();
    crate::planning::sprints::sprint::start_sprint(c, sprint_one, &cal).unwrap();
    complete_sprint(c, sprint_one, Leftover::ToBacklog).unwrap();
    let h = format!("{HEAD},Sprint");
    let probs = rows_of(import_into(c, b, &format!("{h}\n,Dev task,T,,,,,,,Sprint One\n")).unwrap_err());
    assert_eq!((probs[0].row, probs[0].column.as_str()), (2, "Sprint"));
    assert!(matches!(probs[0].issue, RowIssue::Sprint(crate::planning::sprints::SprintError::SprintCompleted)));
    assert_eq!(count_tickets(c, b), 0, "alles oder nichts");
    // Projekt ohne Sprints: die Spalte „Sprint“ ist nicht erlaubt
    let fx = Fx::new();
    let e = import_into(&fx.c, fx.project, &format!("{h}\n,Dev task,T,,,,,,,X\n")).unwrap_err();
    assert_eq!(e, BackupError::CsvHeader(HeaderProblem::Unknown { column: 10, name: "Sprint".into() }));
    assert_eq!(count_tickets(&fx.c, fx.project), 0);
    let _ = (s.s_plan, s.s_active, s.t1, s.t2);
}

#[test]
fn multi_line_cells_with_quotes_and_delimiters_survive() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let desc = "Zeile 1\r\n\"zitiert\", mit Komma; und Semikolon\n\nLetzte";
    let mut row = vec![String::new(); 10];
    row[1] = "Dev task".into();
    row[2] = "T \"q\", x".into();
    row[9] = desc.into();
    let t = CsvTable { headers: [HEAD.split(',').map(String::from).collect::<Vec<_>>(), vec!["Description".into()]].concat(), rows: vec![row] };
    let text = crate::calc::csv::write_csv(&t, &Default::default()).unwrap();
    import_into(c, b, &text).unwrap();
    let out = table(&export_tickets_csv(c, b, &opts()).unwrap());
    let r = &out.rows[0];
    assert_eq!(r[2], "T \"q\", x");
    assert_eq!(r[idx(&out, "Description")], desc);
}

#[test]
fn two_thousand_rows_import_quickly() {
    let s = setup();
    let c = &s.fx.c;
    let b = clone_project(&s, "BB");
    let mut text = format!("{HEAD},Priority,Description\n");
    for i in 0..2000 {
        text += &format!("OLD-{},Dev task,Ticket {i},,,,,,,High,\"Beschreibung {i}\nzweite Zeile\"\n", i + 1);
    }
    let t0 = std::time::Instant::now();
    let rep = import_into(c, b, &text).unwrap();
    let dt = t0.elapsed();
    assert_eq!(rep.created, 2000);
    assert_eq!(count_tickets(c, b), 2000);
    let t1 = std::time::Instant::now();
    let out = export_tickets_csv(c, b, &CsvOptions::for_ui()).unwrap();
    eprintln!("2000 Zeilen: Import {dt:?}, Export {:?}", t1.elapsed());
    assert_eq!(table_any(&out).rows.len(), 2000);
    // Allein dauert es unter einer Sekunde; die Grenze ist großzügig, weil die Tests parallel laufen.
    assert!(dt.as_secs() < 90, "{dt:?}");
}

#[test]
fn ui_defaults_are_safe() {
    let o = CsvOptions::for_ui();
    assert!(o.excel_safe && o.bom);
    assert!(!CsvOptions::default().excel_safe);
    assert!(CsvImportOptions::default().unprotect);
}

#[test]
fn numbers_roundtrip_exactly() {
    let s = setup();
    let c = &s.fx.c;
    let hours = crate::planning::fields::list_fields(c, s.fx.task).unwrap().into_iter().find(|f| f.name.as_deref() == Some("Hours")).unwrap();
    for (v, text) in [(json!(3), "3"), (json!(0.5), "0.5"), (json!(-1), "-1"), (json!(12.3), "12.3")] {
        update_ticket(c, s.task2, &TicketUpdate { title: None, values: vec![(hours.id, v)] }, &Source::ui()).unwrap();
        let t = table(&exp(&s, &opts()));
        assert_eq!(t.rows.iter().find(|r| r[2] == "Task B").unwrap()[idx(&t, "Hours")], text);
    }
    let _ = (s.story, s.task1, s.plain_type);
}
