//! Prüfungen der Abfrage-Ausführung: Operatoren je Feldtyp, NULL-Semantik, Mehrfachwerte, Verweise, Freitext, Sortierung,
//! Funktionen, Prädikate, SQL-Injektion, Fehler, Seiten und Leistung.

use super::*;
use crate::planning::fields::{FieldKind, FieldOptions};
use crate::planning::history::Source;
use crate::planning::link_types::link_type_by_key;
use crate::planning::links::{add_link, Target};
use crate::planning::projects::{update_project, UpdateProject};
use crate::planning::tickets::fixture::Fx;
use crate::planning::tickets::{create_ticket, set_field, set_flagged, set_starred, NewTicket};
use crate::planning::transition::transition_ticket;
use crate::planning::views::keywords::{create_keyword, Cond, Definition, NewKeyword, SortDir as KwDir};
use crate::planning::workflow::ConditionOp;
use crate::query::{Expr, Name, Query};
use serde_json::{json, Value};

pub(super) const TODAY: &str = "2026-11-16";

/// Testdaten: Projekt PAN mit den Standard-Typen, zusätzlichen Feldern und 11 Tickets (siehe `new`).
///
/// | Nr | Typ | Titel | Status | Priorität | Eltern | SP | Due | Labels | sonst |
/// |----|-----|-------|--------|-----------|--------|----|-----|--------|-------|
/// | 1 | Epic | Checkout redesign | In Progress | High | | | | | Stern (UI) |
/// | 2 | Epic | Payments | Done | Highest | | | | | |
/// | 3 | Story | Login with e-mail | To Do | High | 1 | 5 | 2026-11-10 | auth, backend | Feld Epic = PAN-1 |
/// | 4 | Story | Reset password | In Progress | Medium | 1 | 3 | 2026-11-20 | auth | Stern (Quick) |
/// | 5 | Story | Wishlist | To Do | Lowest | | | | | |
/// | 6 | Task | Rate limiting 100% safe | In Progress | Highest | 3 | 2 | 2026-11-16 | backend | Terminal t-login, Reviewed ja, Components frontend, Start 2026-11-15T09:30, Markierung (Agent) |
/// | 7 | Task | Add_index on table | To Do | Medium | | 8 | 2026-11-30 | db | Terminal t-docs, Reviewed nein, Components backend+db |
/// | 8 | Defect | Login fails with Ümlaut | In Progress | Highest | 2 | | 2026-11-12 | auth | Steps „Press ENTER“, Stern (UI), Markierung (Import) |
/// | 9 | Defect | Typo | Done | Low | | | | | |
/// | 10 | Subtask | Write tests | To Do | Medium | 6 | | | | |
/// | 11 | Task | No priority | To Do | (leer) | | | | | |
pub(super) struct Sx {
    pub f: Fx,
    /// Kennungen von PAN-1 … PAN-11 (Index 0 = PAN-1)
    pub ids: Vec<i64>,
    pub ctx: Ctx,
}

pub(super) fn ks(n: &[u32]) -> Vec<String> {
    n.iter().map(|i| format!("PAN-{i}")).collect()
}

fn ui() -> Source {
    Source::ui()
}

impl Sx {
    pub fn new() -> Sx {
        let f = Fx::new();
        let date = |t: i64| f.add_field(t, "Due", FieldKind::Date, FieldOptions::default(), false);
        let labels = |t: i64| f.add_field(t, "Labels", FieldKind::Labels, FieldOptions::default(), false);
        let (due_story, due_task, due_defect) = (date(f.story), date(f.task), date(f.defect));
        let (lab_story, lab_task, lab_defect) = (labels(f.story), labels(f.task), labels(f.defect));
        let terminal = f.add_field(f.task, "Terminal", FieldKind::Terminal, FieldOptions::default(), false);
        let reviewed = f.add_field(f.task, "Reviewed", FieldKind::Checkbox, FieldOptions::default(), false);
        let comps = f.add_field(f.task, "Components", FieldKind::Multi, Fx::dropdown(&["frontend", "backend", "db"]), false);
        let start = f.add_field(f.task, "Start", FieldKind::Datetime, FieldOptions::default(), false);
        let (sp_story, sp_task) = (f.field(f.story, "story_points"), f.field(f.task, "story_points"));
        let (prio_epic, prio_story, prio_task, prio_defect, prio_sub) =
            (f.field(f.epic, "priority"), f.field(f.story, "priority"), f.field(f.task, "priority"), f.field(f.defect, "priority"), f.field(f.subtask, "priority"));
        let steps = f.field(f.defect, "steps");
        let epic_field = f.field(f.story, "epic");
        let mut ids: Vec<i64> = Vec::new();
        let mut mk = |ty: i64, title: &str, parent: Option<usize>, values: Vec<(i64, Value)>, status: Option<i64>| {
            let n = NewTicket { project_id: f.project, type_id: ty, title: title.into(), parent_id: parent.map(|p| ids[p - 1]), values };
            let id = create_ticket(&f.c, &n, &ui()).unwrap().0.id;
            if let Some(s) = status {
                transition_ticket(&f.c, id, s, &[], &ui()).unwrap();
            }
            ids.push(id);
        };
        mk(f.epic, "Checkout redesign", None, vec![(prio_epic.id, json!(2))], Some(f.progress));
        mk(f.epic, "Payments", None, vec![(prio_epic.id, json!(1))], Some(f.done));
        mk(f.story, "Login with e-mail", Some(1), vec![(prio_story.id, json!(2)), (sp_story.id, json!(5)), (due_story.id, json!("2026-11-10")), (lab_story.id, json!(["auth", "backend"]))], None);
        mk(f.story, "Reset password", Some(1), vec![(prio_story.id, json!(3)), (sp_story.id, json!(3)), (due_story.id, json!("2026-11-20")), (lab_story.id, json!(["auth"]))], Some(f.progress));
        mk(f.story, "Wishlist", None, vec![(prio_story.id, json!(5))], None);
        mk(
            f.task,
            "Rate limiting 100% safe",
            Some(3),
            vec![
                (prio_task.id, json!(1)),
                (sp_task.id, json!(2)),
                (due_task.id, json!("2026-11-16")),
                (lab_task.id, json!(["backend"])),
                (terminal.id, json!("t-login")),
                (reviewed.id, json!(true)),
                (comps.id, json!([1])),
                (start.id, json!("2026-11-15T09:30")),
            ],
            Some(f.progress),
        );
        mk(
            f.task,
            "Add_index on table",
            None,
            vec![(sp_task.id, json!(8)), (due_task.id, json!("2026-11-30")), (lab_task.id, json!(["db"])), (terminal.id, json!("t-docs")), (reviewed.id, json!(false)), (comps.id, json!([2, 3]))],
            None,
        );
        mk(
            f.defect,
            "Login fails with Ümlaut",
            Some(2),
            vec![(prio_defect.id, json!(1)), (due_defect.id, json!("2026-11-12")), (lab_defect.id, json!(["auth"])), (steps.id, json!("Press ENTER"))],
            Some(f.progress),
        );
        mk(f.defect, "Typo", None, vec![(prio_defect.id, json!(4))], Some(f.done));
        mk(f.subtask, "Write tests", Some(6), vec![], None);
        mk(f.task, "No priority", None, vec![(prio_task.id, json!(null))], None);
        let _ = prio_sub;
        set_field(&f.c, ids[2], epic_field.id, json!(ids[0]), &ui()).unwrap();
        // zuletzt geänderte Quellen
        set_starred(&f.c, ids[0], true, &ui()).unwrap();
        set_starred(&f.c, ids[3], true, &Source::quick()).unwrap();
        set_flagged(&f.c, ids[5], true, &Source::agent("s")).unwrap();
        set_starred(&f.c, ids[7], true, &ui()).unwrap();
        set_flagged(&f.c, ids[7], true, &Source::import()).unwrap();
        Sx { f, ids, ctx: Ctx::new(TODAY, Lang::En).with_terminals(&["t-login"]) }
    }

    pub fn scope(&self) -> Scope {
        Scope::Project(self.f.project)
    }

    pub fn page(&self, jql: &str) -> SearchPage {
        run_query(&self.f.c, &self.ctx, self.scope(), jql, Page::ALL).unwrap_or_else(|e| panic!("{jql}: {}", e.message(Lang::En)))
    }

    pub fn q(&self, jql: &str) -> Vec<String> {
        self.page(jql).rows.iter().map(|r| r.key.clone()).collect()
    }

    pub fn err(&self, jql: &str) -> SearchError {
        run_query(&self.f.c, &self.ctx, self.scope(), jql, Page::ALL).expect_err(jql)
    }

    pub fn ticket_count(&self) -> i64 {
        self.f.c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap()
    }
}

/// Tabellen-Test: Abfrage → erwartete Tickets in Ergebnis-Reihenfolge.
macro_rules! case {
    ($name:ident, $jql:expr, [$($n:expr),*]) => {
        #[test]
        fn $name() {
            let s = Sx::new();
            let want: Vec<u32> = vec![$($n),*];
            assert_eq!(s.q($jql), ks(&want), "{}", $jql);
        }
    };
}

// ---------------------------------------------------------------------------------------------
// Typ, Status, Auswahlfelder
// ---------------------------------------------------------------------------------------------

case!(type_equals, "type = Epic", [1, 2]);
case!(type_by_display_name_with_space, r#"type = "User story""#, [3, 4, 5]);
case!(type_accepts_the_german_name_too, "type = Userstory", [3, 4, 5]);
case!(type_ignores_case, "TYPE = defect", [8, 9]);
case!(type_not_equals, "type != Epic", [3, 4, 5, 6, 7, 8, 9, 10, 11]);
case!(type_in, "type IN (Epic, Defect)", [1, 2, 8, 9]);
case!(type_not_in, "type NOT IN (Epic, Subtask)", [3, 4, 5, 6, 7, 8, 9, 11]);
case!(status_equals_english, r#"status = "In Progress""#, [1, 4, 6, 8]);
case!(status_equals_german, r#"status = "In Arbeit""#, [1, 4, 6, 8]);
case!(status_not_equals_includes_all_others, "status != Done", [1, 3, 4, 5, 6, 7, 8, 10, 11]);
case!(priority_equals, "priority = High", [1, 3]);
case!(priority_german_value, "priority = Hoch", [1, 3]);
case!(priority_in, "priority IN (High, Highest)", [1, 2, 3, 6, 8]);
case!(priority_not_equals_matches_empty_field, "priority != Highest", [1, 3, 4, 5, 7, 9, 10, 11]);
case!(priority_not_in_matches_empty_field, "priority NOT IN (Highest, High)", [4, 5, 7, 9, 10, 11]);
case!(priority_is_empty, "priority IS EMPTY", [11]);
case!(priority_is_not_empty, "priority IS NOT EMPTY", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
case!(priority_is_null_synonym, "priority IS NULL", [11]);
case!(priority_equals_null, "priority = NULL", [11]);
case!(priority_not_equals_null, "priority != EMPTY", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
case!(in_list_with_null_includes_empty, "priority IN (Low, NULL)", [9, 11]);
case!(status_is_never_empty_for_workflow_types, "status IS EMPTY", []);
case!(project_equals_key, "project = PAN", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);

#[test]
fn project_values_are_limited_to_the_scope() {
    let s = Sx::new();
    s.f.other_project("OTH");
    assert!(matches!(s.err("project = OTH"), SearchError::Invalid { .. }));
    assert_eq!(run_query(&s.f.c, &s.ctx, Scope::All, "project = OTH", Page::ALL).unwrap().total, 0);
    assert_eq!(run_query(&s.f.c, &s.ctx, Scope::All, r#"project = "Paneldeck""#, Page::ALL).unwrap().total, 11, "auch der Projektname gilt");
}

// ---------------------------------------------------------------------------------------------
// Zahlen
// ---------------------------------------------------------------------------------------------

case!(number_greater, r#""Story points" > 3"#, [3, 7]);
case!(number_greater_equal, r#""Story points" >= 3"#, [3, 4, 7]);
case!(number_less, r#""Story points" < 3"#, [6]);
case!(number_less_equal, r#""Story points" <= 3"#, [4, 6]);
case!(number_equals, r#""Story points" = 5"#, [3]);
case!(number_not_equals_matches_empty, r#""Story points" != 5"#, [1, 2, 4, 5, 6, 7, 8, 9, 10, 11]);
case!(number_is_empty_also_for_types_without_the_field, r#""Story points" IS EMPTY"#, [1, 2, 5, 8, 9, 10, 11]);
case!(number_is_not_empty, r#""Story points" IS NOT EMPTY"#, [3, 4, 6, 7]);
case!(number_in, r#""Story points" IN (2, 8)"#, [6, 7]);
case!(number_not_in, r#""Story points" NOT IN (2, 8)"#, [1, 2, 3, 4, 5, 8, 9, 10, 11]);
case!(number_range, r#""Story points" > 2 AND "Story points" < 8"#, [3, 4]);
case!(number_comparison_never_matches_empty, r#""Story points" < 100"#, [3, 4, 6, 7]);
case!(field_name_matches_case_insensitively, r#""STORY POINTS" = 5"#, [3]);

// ---------------------------------------------------------------------------------------------
// Datum und Datum mit Uhrzeit
// ---------------------------------------------------------------------------------------------

case!(date_less, "due < 2026-11-16", [3, 8]);
case!(date_less_equal, "due <= 2026-11-16", [3, 6, 8]);
case!(date_equals, "due = 2026-11-16", [6]);
case!(date_greater, "due > 2026-11-16", [4, 7]);
case!(date_not_equals_matches_empty, "due != 2026-11-16", [1, 2, 3, 4, 5, 7, 8, 9, 10, 11]);
case!(date_is_empty, "due IS EMPTY", [1, 2, 5, 9, 10, 11]);
case!(date_range, "due >= 2026-11-17 AND due <= 2026-11-30", [4, 7]);
case!(date_with_current_date, "due < currentDate()", [3, 8]);
case!(date_overdue_and_open, "due < currentDate() AND status != Done", [3, 8]);
case!(datetime_greater_than_midnight, "start > 2026-11-15", [6]);
case!(datetime_less_than_next_day, "start < 2026-11-16", [6]);
case!(datetime_equals_exact_time, "start = 2026-11-15T09:30", [6]);
case!(datetime_date_only_means_midnight, "start = 2026-11-15", []);
case!(datetime_with_current_date, "start < currentDate()", [6]);
case!(created_is_set_for_all, "created >= 2000-01-01", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
case!(created_in_the_past_matches_nothing, "created < 2000-01-01", []);
case!(updated_is_never_empty, "updated IS EMPTY", []);

// ---------------------------------------------------------------------------------------------
// Labels, Mehrfachauswahl, Checkbox
// ---------------------------------------------------------------------------------------------

case!(labels_equals_means_contains, "labels = auth", [3, 4, 8]);
case!(labels_not_equals_means_does_not_contain, "labels != auth", [1, 2, 5, 6, 7, 9, 10, 11]);
case!(labels_in, "labels IN (db, backend)", [3, 6, 7]);
case!(labels_not_in, "labels NOT IN (db)", [1, 2, 3, 4, 5, 6, 8, 9, 10, 11]);
case!(labels_is_empty, "labels IS EMPTY", [1, 2, 5, 9, 10, 11]);
case!(labels_contains_text, r#"labels ~ "ack""#, [3, 6]);
case!(labels_not_contains_text, r#"labels !~ "ack""#, [1, 2, 4, 5, 7, 8, 9, 10, 11]);
case!(labels_with_status, "labels = auth AND status != Done", [3, 4, 8]);
case!(multi_equals_means_contains, "components = backend", [7]);
case!(multi_in, "components IN (frontend, db)", [6, 7]);
case!(multi_not_equals, "components != db", [1, 2, 3, 4, 5, 6, 8, 9, 10, 11]);
case!(multi_is_empty, "components IS EMPTY", [1, 2, 3, 4, 5, 8, 9, 10, 11]);
case!(checkbox_true, "reviewed = true", [6]);
case!(checkbox_false, "reviewed = false", [7]);
case!(checkbox_not_true_matches_empty, "reviewed != true", [1, 2, 3, 4, 5, 7, 8, 9, 10, 11]);
case!(checkbox_is_empty, "reviewed IS EMPTY", [1, 2, 3, 4, 5, 8, 9, 10, 11]);
case!(checkbox_accepts_yes_no, "reviewed = no", [7]);

// ---------------------------------------------------------------------------------------------
// Verweise, Terminals
// ---------------------------------------------------------------------------------------------

case!(parent_equals, "parent = PAN-1", [3, 4]);
case!(parent_key_ignores_case, "parent = pan-1", [3, 4]);
case!(parent_not_equals_matches_empty, "parent != PAN-1", [1, 2, 5, 6, 7, 8, 9, 10, 11]);
case!(parent_is_empty, "parent IS EMPTY", [1, 2, 5, 7, 9, 11]);
case!(parent_in, "parent IN (PAN-1, PAN-6)", [3, 4, 10]);
case!(parent_unknown_ticket_matches_nothing, "parent = PAN-999", []);
case!(parent_unknown_ticket_not_equals_matches_all, "parent != PAN-999", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
case!(ticket_field_equals, "epic = PAN-1", [3]);
case!(ticket_field_is_empty, "epic IS EMPTY", [1, 2, 4, 5, 6, 7, 8, 9, 10, 11]);
case!(terminal_equals, "terminal = t-login", [6]);
case!(terminal_equals_ignores_case, "terminal = T-LOGIN", [6]);
case!(terminal_is_not_empty, "terminal IS NOT EMPTY", [6, 7]);
case!(terminal_not_equals_matches_empty, "terminal != t-login", [1, 2, 3, 4, 5, 7, 8, 9, 10, 11]);
case!(terminal_linked_terminal_function, "terminal = linkedTerminal()", [6]);

// ---------------------------------------------------------------------------------------------
// Text und Freitext
// ---------------------------------------------------------------------------------------------

case!(key_equals, "key = PAN-6", [6]);
case!(key_contains, r#"key ~ "PAN-1""#, [1, 10, 11]);
case!(summary_contains, "summary ~ login", [3, 8]);
case!(summary_contains_ignores_case, "summary ~ LOGIN", [3, 8]);
case!(title_is_an_alias_of_summary, "title ~ login", [3, 8]);
case!(summary_equals_whole_text, r#"summary = "typo""#, [9]);
case!(summary_equals_does_not_match_a_part, r#"summary = "ty""#, []);
case!(summary_not_contains_matches_everything_else, "summary !~ login", [1, 2, 4, 5, 6, 7, 9, 10, 11]);
case!(summary_contains_non_ascii_ignores_case, r#"summary ~ "ümlaut""#, [8]);
case!(summary_contains_non_ascii_upper_needle, r#"summary ~ "ÜMLAUT""#, [8]);
case!(summary_percent_is_not_a_wildcard, r#"summary ~ "100%""#, [6]);
case!(summary_percent_wildcard_does_not_match, r#"summary ~ "add%index""#, []);
case!(summary_underscore_is_literal, r#"summary ~ "add_index""#, [7]);
case!(summary_underscore_is_not_a_wildcard, r#"summary ~ "a_d""#, []);
case!(summary_with_glob_characters_is_literal, r#"summary ~ "ü*""#, []);
case!(free_text_in_title, r#""login""#, [3, 8]);
case!(free_text_in_rich_text_value, r#""enter""#, [8]);
case!(free_text_in_labels, r#""backend""#, [3, 6]);
case!(free_text_in_key, r#""pan-7""#, [7]);
case!(free_text_phrase_must_match_as_a_whole, r#""login fails""#, [8]);
case!(free_text_finds_nothing, r#""nothing at all""#, []);
case!(free_text_combines_with_filters, r#""login" AND status = Done"#, []);
case!(free_text_ignores_terminal_names, r#""t-login""#, []);

// ---------------------------------------------------------------------------------------------
// Prädikate, Markierungen, Quelle
// ---------------------------------------------------------------------------------------------

case!(is_blocked, "IS blocked", [6, 8]);
case!(is_not_blocked, "IS NOT blocked", [1, 2, 3, 4, 5, 7, 9, 10, 11]);
case!(is_favorite, "IS favorite", [1, 4, 8]);
case!(is_not_favorite, "IS NOT favorite", [2, 3, 5, 6, 7, 9, 10, 11]);
case!(flagged_field, "flagged = true", [6, 8]);
case!(starred_field_not_equals, "starred != true", [2, 3, 5, 6, 7, 9, 10, 11]);
case!(flagged_is_never_empty, "flagged IS EMPTY", []);
case!(changed_by_agent, "changedBy = agent", [6]);
case!(changed_by_import, "changedBy = import", [8]);
case!(changed_by_quick, "changedBy = quick", [4]);
case!(changed_by_ui, "changedBy = ui", [1, 2, 3, 5, 7, 9, 10, 11]);
case!(changed_by_not_ui, "changedBy != ui", [4, 6, 8]);
case!(changed_by_is_never_empty_with_history, "changedBy IS EMPTY", []);

// ---------------------------------------------------------------------------------------------
// Logik
// ---------------------------------------------------------------------------------------------

case!(and_binds_stronger_than_or, "type = Defect AND priority = Highest OR priority = High", [1, 3, 8]);
case!(parentheses_group, "type = Defect AND (priority = Highest OR priority = Low)", [8, 9]);
case!(not_negates_a_condition, "NOT type = Epic", [3, 4, 5, 6, 7, 8, 9, 10, 11]);
case!(not_of_group, r#"NOT (status = Done OR status = "To Do")"#, [1, 4, 6, 8]);
case!(double_not, "NOT NOT type = Epic", [1, 2]);
case!(not_in_does_not_equal_not_of_in_for_empty_fields, "NOT priority IN (Highest)", [1, 3, 4, 5, 7, 9, 10, 11]);
case!(empty_query_matches_everything, "", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
case!(blank_query_matches_everything, "   ", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);

// ---------------------------------------------------------------------------------------------
// Sortierung (Ergebnis-Reihenfolge ist Teil der Erwartung)
// ---------------------------------------------------------------------------------------------

case!(order_by_priority_uses_value_order_and_puts_empty_last, "ORDER BY priority", [2, 6, 8, 1, 3, 4, 7, 10, 9, 5, 11]);
case!(order_by_priority_desc_keeps_empty_last, "ORDER BY priority DESC", [5, 9, 4, 7, 10, 1, 3, 2, 6, 8, 11]);
case!(order_by_number, r#"ORDER BY "Story points""#, [6, 4, 3, 7, 1, 2, 5, 8, 9, 10, 11]);
case!(order_by_number_desc, r#"ORDER BY "Story points" DESC"#, [7, 3, 4, 6, 1, 2, 5, 8, 9, 10, 11]);
case!(order_by_date, "ORDER BY due", [3, 8, 6, 4, 7, 1, 2, 5, 9, 10, 11]);
case!(order_by_date_desc, "ORDER BY due DESC", [7, 4, 6, 8, 3, 1, 2, 5, 9, 10, 11]);
case!(order_by_text_ignores_case, "ORDER BY summary", [7, 1, 8, 3, 11, 2, 6, 4, 9, 5, 10]);
case!(order_by_type_follows_type_order, "ORDER BY type", [1, 2, 3, 4, 5, 6, 7, 11, 10, 8, 9]);
case!(order_by_status_desc, "ORDER BY status DESC", [2, 9, 1, 4, 6, 8, 3, 5, 7, 10, 11]);
case!(order_by_two_fields, "ORDER BY status, priority", [3, 7, 10, 5, 11, 6, 8, 1, 4, 2, 9]);
case!(order_by_key_desc_is_numeric, "ORDER BY key DESC", [11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
case!(order_by_parent, "ORDER BY parent", [3, 4, 8, 6, 10, 1, 2, 5, 7, 9, 11]);
case!(order_by_terminal, "ORDER BY terminal", [7, 6, 1, 2, 3, 4, 5, 8, 9, 10, 11]);
case!(order_by_checkbox, "ORDER BY reviewed", [7, 6, 1, 2, 3, 4, 5, 8, 9, 10, 11]);
case!(order_by_changed_by, "ORDER BY changedBy", [1, 2, 3, 5, 7, 9, 10, 11, 6, 4, 8]);
case!(sort_by_is_a_synonym, "SORT BY priority", [2, 6, 8, 1, 3, 4, 7, 10, 9, 5, 11]);
case!(sort_by_favorites_puts_starred_first, "SORT BY favorites", [1, 4, 8, 2, 3, 5, 6, 7, 9, 10, 11]);
case!(sort_by_favorites_desc_puts_starred_last, "SORT BY favorites DESC", [2, 3, 5, 6, 7, 9, 10, 11, 1, 4, 8]);
case!(sort_by_flagged_keyword_puts_flagged_first, "ORDER BY flagged", [6, 8, 1, 2, 3, 4, 5, 7, 9, 10, 11]);
case!(sort_with_filter, "type = Defect ORDER BY priority DESC", [9, 8]);
case!(sort_favorites_then_due, "status != Done SORT BY favorites, due", [8, 4, 1, 3, 6, 7, 5, 10, 11]);
case!(order_without_filter_keeps_all, "ORDER BY key", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);

// ---------------------------------------------------------------------------------------------
// Rest: Schema, Konflikte, Prädikate, Fehler, Seiten, Injektion, Board-Hilfen, Leistung
// ---------------------------------------------------------------------------------------------

#[test]
fn schema_contains_builtin_fields_and_custom_fields_once() {
    let s = Sx::new();
    let schema = schema_for(&s.f.c, s.scope(), Lang::En).unwrap();
    for name in ["key", "type", "status", "summary", "title", "parent", "created", "updated", "flagged", "starred", "project", "changedBy", "terminal", "priority", "Story points", "Due", "Labels", "Components", "Reviewed", "Start", "Epic"] {
        assert!(schema.field(name).is_some(), "{name} fehlt");
    }
    assert_eq!(schema.fields.iter().filter(|f| f.name.eq_ignore_ascii_case("labels")).count(), 1, "gleichnamige Felder werden zusammengelegt");
    assert_eq!(schema.fields.iter().filter(|f| f.name.eq_ignore_ascii_case("priority")).count(), 1);
    assert!(schema.field("sprint").is_none(), "ohne Sprints im Projekt kein Feld sprint");
    assert!(schema.predicate("blocked").is_some() && schema.predicate("favorite").is_some());
    assert!(schema.sort_keyword("favorites").is_some() && schema.sort_keyword("flagged").is_some());
    assert!(schema.function("currentDate").is_some() && schema.function("linkedTerminal").is_some() && schema.function("activeSprint").is_some());
}

#[test]
fn schema_types_and_values() {
    let s = Sx::new();
    let schema = schema_for(&s.f.c, s.scope(), Lang::En).unwrap();
    use crate::query::FieldType;
    assert!(matches!(&schema.field("labels").unwrap().ty, FieldType::Labels) && schema.field("labels").unwrap().multi);
    assert!(matches!(&schema.field("due").unwrap().ty, FieldType::Date));
    assert!(matches!(&schema.field("start").unwrap().ty, FieldType::DateTime));
    assert!(matches!(&schema.field("reviewed").unwrap().ty, FieldType::Bool));
    assert!(matches!(&schema.field("epic").unwrap().ty, FieldType::TicketRef));
    assert!(matches!(&schema.field("terminal").unwrap().ty, FieldType::Terminal));
    let comps = schema.field("components").unwrap();
    assert!(comps.multi && matches!(&comps.ty, FieldType::Option(v) if v == &["frontend", "backend", "db"]));
    if let FieldType::Option(v) = &schema.field("priority").unwrap().ty {
        for want in ["Highest", "Höchste", "High", "Hoch", "Medium", "Normal", "Low", "Niedrig", "Lowest", "Niedrigste"] {
            assert!(v.iter().any(|x| x == want), "{want} fehlt in {v:?}");
        }
    } else {
        panic!("priority ist ein Auswahlfeld");
    }
    if let FieldType::Option(v) = &schema.field("type").unwrap().ty {
        assert!(v.contains(&"User story".to_string()) && v.contains(&"Userstory".to_string()) && v.contains(&"Defect".to_string()) && v.contains(&"Fehler".to_string()));
    } else {
        panic!("type ist ein Auswahlfeld");
    }
    if let FieldType::Option(v) = &schema.field("changedBy").unwrap().ty {
        assert_eq!(v, &["ui", "agent", "quick", "import"]);
    } else {
        panic!("changedBy");
    }
}

#[test]
fn german_schema_adds_english_aliases() {
    let s = Sx::new();
    let de = schema_for(&s.f.c, s.scope(), Lang::De).unwrap();
    for name in ["Priorität", "priority", "Story Points", "Typ", "Titel", "Projekt", "Übergeordnet", "parent"] {
        assert!(de.field(name).is_some(), "{name}");
    }
    let en = schema_for(&s.f.c, s.scope(), Lang::En).unwrap();
    assert!(en.field("Priorität").is_none(), "englisch: keine deutschen Namen");
}

#[test]
fn german_query_by_german_field_name() {
    let s = Sx::new();
    let ctx = Ctx::new(TODAY, Lang::De);
    let r = run_query(&s.f.c, &ctx, s.scope(), "Priorität = Hoch", Page::ALL).unwrap();
    assert_eq!(r.rows.iter().map(|x| x.key.clone()).collect::<Vec<_>>(), ks(&[1, 3]));
}

#[test]
fn conflicting_field_kinds_keep_the_most_common_and_report_the_rest() {
    let s = Sx::new();
    let other = s.f.other_project("OTH");
    let epic = crate::planning::types::list_types(&s.f.c, other).unwrap()[0].id;
    // „Due“ als Text in einem Typ, drei Datumsfelder im Projekt PAN
    crate::planning::fields::create_field(&s.f.c, &crate::planning::fields::NewField::new(epic, "Due", FieldKind::Text)).unwrap();
    let ss = search_schema(&s.f.c, Scope::All, Lang::En).unwrap();
    assert!(matches!(ss.schema.field("due").unwrap().ty, crate::query::FieldType::Date));
    assert_eq!(ss.conflicts.len(), 1);
    assert_eq!(ss.conflicts[0].kept, Some(FieldKind::Date));
    assert_eq!(ss.conflicts[0].dropped, vec![FieldKind::Text]);
}

#[test]
fn a_custom_field_named_like_a_builtin_is_reported_not_used() {
    let s = Sx::new();
    crate::planning::fields::create_field(&s.f.c, &crate::planning::fields::NewField::new(s.f.story, "Status", FieldKind::Text)).unwrap();
    let ss = search_schema(&s.f.c, s.scope(), Lang::En).unwrap();
    assert!(ss.conflicts.iter().any(|c| c.name == "Status" && c.kept.is_none()));
    assert_eq!(s.q("status = Done"), ks(&[2, 9]), "das eingebaute Feld gewinnt");
}

#[test]
fn sprint_field_exists_with_sprints_but_is_not_available_yet() {
    let s = Sx::new();
    update_project(&s.f.c, s.f.project, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
    let schema = schema_for(&s.f.c, s.scope(), Lang::En).unwrap();
    assert!(schema.field("sprint").is_some());
    for jql in ["sprint = activeSprint()", "sprint = \"Sprint 1\"", "sprint IS EMPTY", "sprint IS NOT EMPTY", "ORDER BY sprint"] {
        let e = s.err(jql);
        assert_eq!(e, SearchError::NotAvailableYet(Feature::Sprints), "{jql}");
        assert!(!e.message(Lang::De).is_empty() && e.message(Lang::De) != e.message(Lang::En));
    }
}

#[test]
fn active_sprint_function_is_not_available_yet_anywhere() {
    let s = Sx::new();
    let e = s.err("summary = activeSprint()");
    assert_eq!(e, SearchError::NotAvailableYet(Feature::Sprints));
}

#[test]
fn unknown_field_without_sprints_is_a_validation_error() {
    let s = Sx::new();
    match s.err("sprint = activeSprint()") {
        SearchError::Invalid { issues, .. } => assert!(issues[0].message(Lang::En).contains("sprint")),
        e => panic!("{e:?}"),
    }
}

#[test]
fn version_fields_are_not_available_yet() {
    let s = Sx::new();
    s.f.add_field(s.f.epic, "Release version", FieldKind::Version, FieldOptions::default(), false);
    for jql in [r#""Release version" = 2.0.0"#, r#""Release version" IS EMPTY"#, r#""Release version" IN (1, 2)"#, r#"ORDER BY "Release version""#] {
        assert_eq!(s.err(jql), SearchError::NotAvailableYet(Feature::Versions), "{jql}");
    }
}

#[test]
fn terminal_links_count_as_terminal_values() {
    let s = Sx::new();
    let lt = link_type_by_key(&s.f.c, "terminal").unwrap();
    add_link(&s.f.c, s.ids[8], lt.id, Target::Terminal("t-login".into()), &ui()).unwrap();
    assert_eq!(s.q("terminal = t-login"), ks(&[6, 9]));
    assert_eq!(s.q("terminal IS NOT EMPTY"), ks(&[6, 7, 9]));
    assert_eq!(s.q("terminal = linkedTerminal()"), ks(&[6, 9]));
}

#[test]
fn linked_terminal_without_terminals_matches_nothing() {
    let mut s = Sx::new();
    s.ctx.terminals.clear();
    assert!(s.q("terminal = linkedTerminal()").is_empty());
    assert_eq!(s.q("terminal != linkedTerminal()").len(), 11, "verneint trifft alle");
}

#[test]
fn linked_terminal_with_several_terminals() {
    let mut s = Sx::new();
    s.ctx.terminals = vec!["t-login".into(), "t-docs".into()];
    assert_eq!(s.q("terminal = linkedTerminal()"), ks(&[6, 7]));
}

#[test]
fn current_date_follows_the_context() {
    let mut s = Sx::new();
    s.ctx.today = "2026-11-25".into();
    assert_eq!(s.q("due < currentDate()"), ks(&[3, 4, 6, 8]));
    s.ctx.today = "2026-11-01".into();
    assert!(s.q("due < currentDate()").is_empty());
}

#[test]
fn custom_predicate_from_a_condition() {
    let s = Sx::new();
    let prio = s.f.field(s.f.story, "priority").id;
    create_keyword(&s.f.c, &NewKeyword { name: "highstory".into(), description: String::new(), definition: Definition::Predicate { condition: Cond::Compare { field_id: prio, op: ConditionOp::Eq, value: json!(2) } } }).unwrap();
    assert_eq!(s.q("IS highstory"), ks(&[3]));
    assert_eq!(s.q("IS NOT highstory"), ks(&[1, 2, 4, 5, 6, 7, 8, 9, 10, 11]));
    assert_eq!(s.q("IS HIGHSTORY AND status = \"To Do\""), ks(&[3]), "Name ohne Beachtung der Schreibweise");
}

#[test]
fn custom_predicate_with_and_or_conditions() {
    let s = Sx::new();
    let sp = s.f.field(s.f.story, "story_points").id;
    let prio = s.f.field(s.f.story, "priority").id;
    let cond = Cond::Or {
        items: vec![
            Cond::And { items: vec![Cond::Compare { field_id: sp, op: ConditionOp::Ge, value: json!(5) }, Cond::Compare { field_id: prio, op: ConditionOp::In, value: json!([1, 2]) }] },
            Cond::Compare { field_id: prio, op: ConditionOp::Eq, value: json!(5) },
        ],
    };
    create_keyword(&s.f.c, &NewKeyword { name: "bigorlow".into(), description: String::new(), definition: Definition::Predicate { condition: cond } }).unwrap();
    assert_eq!(s.q("IS bigorlow"), ks(&[3, 5]));
}

#[test]
fn custom_predicate_on_empty_and_not_equal() {
    let s = Sx::new();
    let sp = s.f.field(s.f.story, "story_points").id;
    create_keyword(&s.f.c, &NewKeyword { name: "nopoints".into(), description: String::new(), definition: Definition::Predicate { condition: Cond::Compare { field_id: sp, op: ConditionOp::IsEmpty, value: Value::Null } } }).unwrap();
    // gilt auch für Tickets ohne dieses Feld (leer)
    assert_eq!(s.q("IS nopoints"), ks(&[1, 2, 5, 6, 7, 8, 9, 10, 11]));
    create_keyword(&s.f.c, &NewKeyword { name: "notfive".into(), description: String::new(), definition: Definition::Predicate { condition: Cond::Compare { field_id: sp, op: ConditionOp::Ne, value: json!(5) } } }).unwrap();
    assert_eq!(s.q("IS notfive").len(), 10);
}

#[test]
fn custom_sort_keyword() {
    let s = Sx::new();
    let sp = s.f.field(s.f.story, "story_points").id;
    create_keyword(&s.f.c, &NewKeyword { name: "bysp".into(), description: String::new(), definition: Definition::Sort { field_id: sp, dir: KwDir::Desc } }).unwrap();
    assert_eq!(s.q("ORDER BY bysp"), ks(&[3, 4, 1, 2, 5, 6, 7, 8, 9, 10, 11]));
    assert_eq!(s.q("ORDER BY bysp ASC"), ks(&[4, 3, 1, 2, 5, 6, 7, 8, 9, 10, 11]), "eine angegebene Richtung gilt");
}

#[test]
fn broken_keyword_is_not_offered() {
    let s = Sx::new();
    let sp = s.f.field(s.f.task, "story_points").id;
    create_keyword(&s.f.c, &NewKeyword { name: "gone".into(), description: String::new(), definition: Definition::Predicate { condition: Cond::Compare { field_id: sp, op: ConditionOp::IsEmpty, value: Value::Null } } }).unwrap();
    assert_eq!(s.q("IS gone").len(), 9, "vorhanden: alle ohne Wert in diesem Feld");
    crate::planning::fields::delete_field(&s.f.c, sp, Some(crate::planning::integrity::DeleteMode::Discard)).unwrap();
    assert!(matches!(s.err("IS gone"), SearchError::Invalid { .. }));
}

#[test]
fn rows_carry_everything_lists_and_cards_need() {
    let s = Sx::new();
    let r = s.page("key = PAN-6").rows.remove(0);
    assert_eq!((r.key.as_str(), r.project_key.as_str(), r.number), ("PAN-6", "PAN", 6));
    assert_eq!(r.title, "Rate limiting 100% safe");
    assert_eq!((r.type_name.as_str(), r.status_name.as_deref(), r.priority.as_deref()), ("Dev task", Some("In Progress"), Some("Highest")));
    assert_eq!(r.story_points, Some(2.0));
    assert_eq!(r.parent_key.as_deref(), Some("PAN-3"));
    assert!(r.flagged && !r.starred);
    assert_eq!(r.status_category, Some(crate::planning::statuses::StatusCategory::Progress));
    assert!(r.type_color.starts_with('#') && r.type_icon.is_some() && r.priority_color.is_some());
    assert!(!r.updated_at.is_empty());
}

#[test]
fn rows_follow_the_language() {
    let s = Sx::new();
    let ctx = Ctx::new(TODAY, Lang::De);
    let r = run_query(&s.f.c, &ctx, s.scope(), "key = PAN-6", Page::ALL).unwrap().rows.remove(0);
    assert_eq!((r.type_name.as_str(), r.status_name.as_deref(), r.priority.as_deref()), ("Dev-Aufgabe", Some("In Arbeit"), Some("Höchste")));
}

#[test]
fn rows_without_priority_points_or_parent() {
    let s = Sx::new();
    let r = s.page("key = PAN-11").rows.remove(0);
    assert_eq!((r.priority, r.story_points, r.parent_id, r.parent_key), (None, None, None, None));
}

#[test]
fn paging_returns_a_page_and_the_total() {
    let s = Sx::new();
    let p = run_query(&s.f.c, &s.ctx, s.scope(), "ORDER BY key", Page::new(4, 0)).unwrap();
    assert_eq!((p.rows.len(), p.total), (4, 11));
    let p = run_query(&s.f.c, &s.ctx, s.scope(), "ORDER BY key", Page::new(4, 8)).unwrap();
    assert_eq!(p.rows.iter().map(|r| r.key.clone()).collect::<Vec<_>>(), ks(&[9, 10, 11]));
    assert_eq!(p.total, 11);
    let p = run_query(&s.f.c, &s.ctx, s.scope(), "ORDER BY key", Page::new(4, 99)).unwrap();
    assert!(p.rows.is_empty());
    assert_eq!(p.total, 11);
    let p = run_query(&s.f.c, &s.ctx, s.scope(), "ORDER BY key", Page::new(0, 0)).unwrap();
    assert!(p.rows.is_empty() && p.total == 11);
}

#[test]
fn paging_keeps_the_filter_total() {
    let s = Sx::new();
    let p = run_query(&s.f.c, &s.ctx, s.scope(), "type = Epic OR type = Defect", Page::new(1, 1)).unwrap();
    assert_eq!((p.rows.len(), p.total), (1, 4));
    assert_eq!(p.rows[0].key, "PAN-2");
}

#[test]
fn count_matches_total() {
    let s = Sx::new();
    assert_eq!(count(&s.f.c, &s.ctx, s.scope(), "status = Done").unwrap(), 2);
    assert_eq!(count(&s.f.c, &s.ctx, s.scope(), "").unwrap(), 11);
    assert_eq!(count(&s.f.c, &s.ctx, s.scope(), "type = Nothing").unwrap_err().problems(Lang::En).len(), 1);
}

#[test]
fn saved_filters_run_in_their_scope() {
    use crate::planning::views::filters::{create_filter, NewFilter};
    let s = Sx::new();
    let a = create_filter(&s.f.c, &NewFilter { project_id: Some(s.f.project), name: "Defects".into(), jql: "type = Defect ORDER BY key DESC".into() }).unwrap();
    let r = run_saved_filter(&s.f.c, &s.ctx, a, Page::ALL).unwrap();
    assert_eq!(r.rows.iter().map(|x| x.key.clone()).collect::<Vec<_>>(), ks(&[9, 8]));
    let other = s.f.other_project("OTH");
    let ot = crate::planning::types::list_types(&s.f.c, other).unwrap()[0].id;
    create_ticket(&s.f.c, &NewTicket::new(other, ot, "Other epic"), &ui()).unwrap();
    let g = create_filter(&s.f.c, &NewFilter { project_id: None, name: "Epics everywhere".into(), jql: "type = Epic".into() }).unwrap();
    let r = run_saved_filter(&s.f.c, &s.ctx, g, Page::ALL).unwrap();
    assert_eq!(r.rows.iter().map(|x| x.key.clone()).collect::<Vec<_>>(), vec!["PAN-1", "PAN-2", "OTH-1"]);
    assert_eq!(run_saved_filter(&s.f.c, &s.ctx, 9999, Page::ALL).unwrap_err(), SearchError::NotFound);
}

#[test]
fn all_scope_spans_projects_and_merges_statuses() {
    let s = Sx::new();
    let other = s.f.other_project("OTH");
    let ot = crate::planning::types::list_types(&s.f.c, other).unwrap()[0].id;
    let (t, _) = create_ticket(&s.f.c, &NewTicket::new(other, ot, "Other epic"), &ui()).unwrap();
    let r = run_query(&s.f.c, &s.ctx, Scope::All, "type = Epic AND status = \"To Do\"", Page::ALL).unwrap();
    assert_eq!(r.rows.iter().map(|x| x.key.clone()).collect::<Vec<_>>(), vec![t.key()]);
    let r = run_query(&s.f.c, &s.ctx, Scope::All, "project = OTH", Page::ALL).unwrap();
    assert_eq!(r.total, 1);
    let schema = schema_for(&s.f.c, Scope::All, Lang::En).unwrap();
    if let crate::query::FieldType::Option(v) = &schema.field("status").unwrap().ty {
        assert_eq!(v.iter().filter(|x| x.as_str() == "Done").count(), 1, "gleiche Status-Namen werden zusammengelegt");
    }
}

#[test]
fn project_scope_excludes_other_projects() {
    let s = Sx::new();
    let other = s.f.other_project("OTH");
    let ot = crate::planning::types::list_types(&s.f.c, other).unwrap()[0].id;
    create_ticket(&s.f.c, &NewTicket::new(other, ot, "Other epic"), &ui()).unwrap();
    assert_eq!(s.q("type = Epic"), ks(&[1, 2]));
    assert_eq!(s.q("").len(), 11);
}

#[test]
fn unknown_project_scope_is_not_found() {
    let s = Sx::new();
    let e = run_query(&s.f.c, &s.ctx, Scope::Project(9999), "", Page::ALL).unwrap_err();
    assert_eq!(e, SearchError::NotFound);
}

#[test]
fn todolist_project_scope_is_empty() {
    let s = Sx::new();
    let todo: i64 = s.f.c.query_row("SELECT id FROM projects WHERE system = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(run_query(&s.f.c, &s.ctx, Scope::Project(todo), "", Page::ALL).unwrap().total, 0);
}

// ---------------------------------------------------------------------------------------------
// Fehler und Grenzen
// ---------------------------------------------------------------------------------------------

#[test]
fn parse_errors_carry_positions_and_both_languages() {
    let s = Sx::new();
    let e = s.err("type = ");
    let ps = e.problems(Lang::En);
    assert_eq!(ps.len(), 1);
    assert_eq!(ps[0].column, 8, "Ende der Eingabe");
    assert!(e.message(Lang::En).contains("Position 8") && e.message(Lang::De).contains("Position 8"));
    assert_ne!(e.message(Lang::En), e.message(Lang::De));
}

#[test]
fn several_parse_errors_are_all_reported() {
    let s = Sx::new();
    let e = s.err("type = AND status IN ()");
    assert!(e.problems(Lang::En).len() >= 2);
}

#[test]
fn validation_problems_have_positions_and_suggestions() {
    let s = Sx::new();
    match s.err("typ = Epic AND status = Dnoe") {
        SearchError::Invalid { issues, .. } => {
            assert_eq!(issues.len(), 2);
            assert_eq!(issues[0].suggestion(), Some("type"));
            assert_eq!(issues[1].suggestion(), Some("Done"));
        }
        e => panic!("{e:?}"),
    }
    let e = s.err("typ = Epic");
    assert_eq!(e.problems(Lang::De)[0].column, 1);
}

#[test]
fn operator_and_value_mismatch_are_validation_errors() {
    let s = Sx::new();
    for jql in [r#"priority ~ "x""#, r#""Story points" = viel"#, "due = 2026-13-45", "parent = nonsense", "labels > a", "reviewed = maybe"] {
        assert!(matches!(s.err(jql), SearchError::Invalid { .. }), "{jql}");
    }
}

#[test]
fn ordering_by_a_multi_value_field_is_rejected() {
    let s = Sx::new();
    assert!(matches!(s.err("ORDER BY labels"), SearchError::Invalid { .. }));
    assert!(matches!(s.err("ORDER BY components"), SearchError::Invalid { .. }));
}

#[test]
fn too_long_query_is_rejected() {
    let s = Sx::new();
    let jql = format!("summary ~ \"{}\"", "x".repeat(4001));
    match s.err(&jql) {
        SearchError::TooLong { len, max } => assert!(len > max && max == MAX_QUERY_CHARS),
        e => panic!("{e:?}"),
    }
    let ok = format!("summary ~ \"{}\"", "x".repeat(3900));
    assert!(s.q(&ok).is_empty());
}

#[test]
fn deeply_nested_parentheses_do_not_crash() {
    let s = Sx::new();
    let jql = format!("{}type = Epic{}", "(".repeat(300), ")".repeat(300));
    match s.err(&jql) {
        SearchError::Parse { .. } => {}
        e => panic!("{e:?}"),
    }
    let ok = format!("{}type = Epic{}", "(".repeat(60), ")".repeat(60));
    assert_eq!(s.q(&ok), ks(&[1, 2]));
}

fn nested_not(depth: usize) -> Query {
    let leaf = Expr::Compare { field: Name::new("type"), op: crate::query::CmpOp::Eq, op_loc: Default::default(), value: word("Epic") };
    let mut e = leaf;
    for _ in 0..depth {
        e = Expr::Not(Box::new(e));
    }
    Query { filter: Some(e), order: vec![] }
}

fn word(w: &str) -> crate::query::Value {
    crate::query::Value::new(crate::query::ValueKind::Word(w.into()))
}

#[test]
fn compile_rejects_a_hand_built_tree_that_is_too_deep() {
    let s = Sx::new();
    let ss = search_schema(&s.f.c, s.scope(), Lang::En).unwrap();
    assert!(matches!(compile(&nested_not(MAX_DEPTH + 20), &ss, &s.ctx), Err(SearchError::TooDeep { .. })));
    assert!(compile(&nested_not(20), &ss, &s.ctx).is_ok());
}

#[test]
fn compile_rejects_too_many_conditions() {
    let s = Sx::new();
    let ss = search_schema(&s.f.c, s.scope(), Lang::En).unwrap();
    let jql = vec!["type = Epic"; MAX_NODES + 50].join(" AND ");
    let q = crate::query::parse(&jql).unwrap();
    assert!(matches!(compile(&q, &ss, &s.ctx), Err(SearchError::TooComplex { .. })));
    let jql = vec!["type = Epic"; 50].join(" OR ");
    assert!(compile(&crate::query::parse(&jql).unwrap(), &ss, &s.ctx).is_ok());
}

#[test]
fn many_conditions_run_with_sqlite_limits() {
    let s = Sx::new();
    let jql = vec!["type = Epic"; 150].join(" OR ");
    assert_eq!(s.q(&jql), ks(&[1, 2]));
    let jql = vec!["NOT type = Epic"; 100].join(" AND ");
    assert_eq!(s.q(&jql).len(), 9);
}

#[test]
fn long_in_lists_work() {
    let s = Sx::new();
    let names: Vec<String> = (0..300).map(|i| format!("Nope{i}")).collect();
    // unbekannte Werte meldet der Validator
    assert!(matches!(s.err(&format!("type IN ({})", names.join(", "))), SearchError::Invalid { .. }));
    let nums: Vec<String> = (0..500).map(|i| i.to_string()).collect();
    assert_eq!(s.q(&format!(r#""Story points" IN ({})"#, nums.join(", "))), ks(&[3, 4, 6, 7]));
}

#[test]
fn timeout_interrupts_a_runaway_statement_and_the_connection_stays_usable() {
    let s = Sx::new();
    let started = std::time::Instant::now();
    let r: SRes<i64> = with_timeout(&s.f.c, std::time::Duration::from_millis(80), || {
        Ok(s.f.c.query_row("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT COUNT(*) FROM c", [], |r| r.get(0))?)
    });
    assert_eq!(r, Err(SearchError::Timeout));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(s.q("type = Epic"), ks(&[1, 2]), "die Verbindung funktioniert danach weiter");
    assert!(!SearchError::Timeout.message(Lang::De).is_empty());
}

#[test]
fn a_fast_query_is_not_affected_by_the_watchdog() {
    let mut s = Sx::new();
    s.ctx.timeout = std::time::Duration::from_millis(5000);
    for _ in 0..20 {
        assert_eq!(s.q("type = Epic"), ks(&[1, 2]));
    }
}

#[test]
fn every_error_has_both_languages() {
    let p = crate::query::parse("type = ").unwrap_err();
    let errors = vec![
        SearchError::Parse { input: "type = ".into(), errors: p },
        SearchError::NotAvailableYet(Feature::Sprints),
        SearchError::NotAvailableYet(Feature::Versions),
        SearchError::TooLong { len: 5000, max: 4000 },
        SearchError::TooDeep { max: 100 },
        SearchError::TooComplex { max: 300 },
        SearchError::TooComplex { max: 0 },
        SearchError::Timeout,
        SearchError::NotFound,
        SearchError::Db("x".into()),
        SearchError::Internal("x".into()),
    ];
    for e in errors {
        let (de, en) = (e.message(Lang::De), e.message(Lang::En));
        assert!(!de.is_empty() && !en.is_empty(), "{e:?}");
        assert_ne!(de, en, "{e:?}");
    }
    assert!(SearchError::TooLong { len: 5000, max: 4000 }.message(Lang::En).contains("5000"));
}

#[test]
fn rusqlite_interrupt_maps_to_timeout() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    let h = c.get_interrupt_handle();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        h.interrupt();
    });
    let e = c.query_row("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT COUNT(*) FROM c", [], |r| r.get::<_, i64>(0)).unwrap_err();
    t.join().unwrap();
    assert_eq!(SearchError::from(e), SearchError::Timeout);
}

// ---------------------------------------------------------------------------------------------
// SQL-Injektion: Werte und Namen werden nie in das SQL geklebt
// ---------------------------------------------------------------------------------------------

#[test]
fn injection_in_text_values_is_just_text() {
    let s = Sx::new();
    for jql in [
        r#"summary = "x' OR 1=1 --""#,
        r#"summary ~ "'); DROP TABLE tickets; --""#,
        r#"summary ~ "\" OR \"1\"=\"1""#,
        r#"summary !~ "' OR ''='""#,
        r#""x'); DELETE FROM tickets; --""#,
        r#"labels = "a' OR '1'='1""#,
        r#"terminal = "' OR 1=1 --""#,
    ] {
        let before = s.ticket_count();
        let hits = s.q(jql);
        assert_eq!(s.ticket_count(), before, "{jql}");
        if !jql.contains("!~") {
            assert!(hits.is_empty(), "{jql}: {hits:?}");
        }
    }
    assert_eq!(s.ticket_count(), 11);
}

#[test]
fn injection_text_never_appears_in_the_sql() {
    let s = Sx::new();
    let evil = "zzq'; DROP TABLE tickets; --";
    let jql = format!(r#"summary ~ "{evil}" AND labels = "{evil}" AND "{evil}" ORDER BY priority"#);
    let (_, c) = prepare(&s.f.c, &s.ctx, s.scope(), &jql).unwrap();
    assert!(!c.sql.contains("zzq") && !c.sql.contains("DROP"), "{}", c.sql);
    assert!(c.params.iter().any(|p| matches!(p, rusqlite::types::Value::Text(t) if t.contains("zzq"))));
}

#[test]
fn injection_in_context_terminals_is_a_parameter() {
    let mut s = Sx::new();
    s.ctx.terminals = vec!["x' OR 1=1 --".into()];
    let (_, c) = prepare(&s.f.c, &s.ctx, s.scope(), "terminal = linkedTerminal()").unwrap();
    assert!(!c.sql.contains("1=1"));
    assert!(s.q("terminal = linkedTerminal()").is_empty());
}

#[test]
fn injection_in_field_names_is_a_validation_error() {
    let s = Sx::new();
    for jql in [
        r#""summary) OR 1=1 --" = x"#,
        r#""x; DROP TABLE tickets" IS EMPTY"#,
        r#"IS "blocked OR 1=1""#,
        r#"ORDER BY "id; DROP TABLE tickets""#,
        r#"ORDER BY "t.id) --""#,
        "parent = \"PAN-1' OR '1'='1\"",
        "status = \"Done' OR 1=1\"",
        "due = \"2026-01-01' OR 1=1\"",
    ] {
        assert!(matches!(s.err(jql), SearchError::Invalid { .. } | SearchError::Parse { .. }), "{jql}");
    }
    assert_eq!(s.ticket_count(), 11);
}

#[test]
fn injection_in_order_by_does_not_change_the_statement() {
    let s = Sx::new();
    let (_, c) = prepare(&s.f.c, &s.ctx, s.scope(), "ORDER BY priority DESC, key").unwrap();
    assert!(c.sql.matches("ORDER BY").count() == 1);
    assert!(!c.sql.contains(';'));
}

#[test]
fn like_escape_survives_backslashes() {
    let s = Sx::new();
    assert!(s.q(r#"summary ~ "\\""#).is_empty());
    assert!(s.q(r#"summary ~ "a\\%""#).is_empty());
}

#[test]
fn sql_has_only_numbered_placeholders_and_no_quotes_from_values() {
    let s = Sx::new();
    let (_, c) = prepare(&s.f.c, &s.ctx, s.scope(), r#"type = Epic AND priority IN (High, Highest) AND "Story points" > 3 AND due < 2026-11-16 AND summary ~ "login""#).unwrap();
    assert!(c.sql.contains("?1"));
    // einzige Anführungszeichen im SQL sind die festen Bausteine (ESCAPE '\\', Tabellenarten)
    let quoted: Vec<&str> = c.sql.split('\'').skip(1).step_by(2).collect();
    assert!(quoted.iter().all(|q| ["\\", "-", "text", "rich", "labels", "terminal"].contains(q)), "{quoted:?}");
}

// ---------------------------------------------------------------------------------------------
// Leistung: 10.000 Tickets
// ---------------------------------------------------------------------------------------------

#[test]
fn ten_thousand_tickets_answer_typical_queries_quickly() {
    let f = Fx::new();
    let c = &f.c;
    let prio = f.field(f.task, "priority").id;
    let sp = f.field(f.task, "story_points").id;
    let desc = f.field(f.task, "description").id;
    let labels = f.add_field(f.task, "Labels", FieldKind::Labels, FieldOptions::default(), false).id;
    let types = [f.epic, f.story, f.task, f.subtask, f.defect];
    let statuses = [f.todo, f.progress, f.done];
    c.execute_batch("BEGIN").unwrap();
    {
        let mut ins = c.prepare("INSERT INTO tickets (project_id, number, type_id, title, status_id, rank, flagged, starred, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?2, ?6, ?7, '2026-01-01T00:00:00', '2026-01-02T00:00:00')").unwrap();
        let mut val = c.prepare("INSERT INTO ticket_values (ticket_id, field_id, pos, text, num) VALUES (?1, ?2, ?3, ?4, ?5)").unwrap();
        let mut hist = c.prepare("INSERT INTO history (ticket_id, at, source, field, batch) VALUES (?1, '2026-01-01T00:00:00', ?2, 'created', 1)").unwrap();
        for i in 1..=10_000i64 {
            let ty = types[(i % 5) as usize];
            ins.execute(rusqlite::params![f.project, i, ty, format!("Ticket {i} about checkout and login"), statuses[(i % 3) as usize], (i % 17 == 0) as i64, (i % 11 == 0) as i64]).unwrap();
            let id = c.last_insert_rowid();
            if ty == f.task {
                val.execute(rusqlite::params![id, prio, 0, None::<String>, 1 + ((i / 5) % 5)]).unwrap();
                val.execute(rusqlite::params![id, sp, 0, None::<String>, (i % 13) as f64]).unwrap();
                val.execute(rusqlite::params![id, desc, 0, format!("Description of {i} with some words"), None::<f64>]).unwrap();
                val.execute(rusqlite::params![id, labels, 0, ["frontend", "backend", "db"][(i % 3) as usize], None::<f64>]).unwrap();
            }
            hist.execute(rusqlite::params![id, ["ui", "agent", "import"][(i % 3) as usize]]).unwrap();
        }
    }
    c.execute_batch("COMMIT").unwrap();
    let limit = if cfg!(debug_assertions) { std::time::Duration::from_secs(4) } else { std::time::Duration::from_millis(150) };
    let ctx = Ctx::new(TODAY, Lang::En);
    let scope = Scope::Project(f.project);
    for jql in [
        r#"type = "Dev task" AND status = "In Progress""#,
        r#""checkout""#,
        r#"summary ~ "ticket 77""#,
        r#"type = "Dev task" AND priority IN (High, Highest) ORDER BY priority"#,
        r#"type = "Dev task" ORDER BY "Story points" DESC"#,
        "labels = backend AND status != Done",
        "IS blocked OR IS favorite ORDER BY key DESC",
        "changedBy = agent AND type = Defect",
        "status != Done ORDER BY summary",
        r#""description of 99""#,
        "ORDER BY updated DESC",
    ] {
        let started = std::time::Instant::now();
        let page = run_query(c, &ctx, scope, jql, Page::new(50, 0)).unwrap();
        let took = started.elapsed();
        assert!(page.total > 0, "{jql}");
        assert!(took < limit, "{jql}: {took:?} (Grenze {limit:?})");
    }
}

#[test]
fn sql_plan_uses_the_project_number_index_for_the_default_order() {
    let s = Sx::new();
    let (_, c) = prepare(&s.f.c, &s.ctx, s.scope(), "type = Epic").unwrap();
    let plan: Vec<String> = {
        let mut st = s.f.c.prepare(&format!("EXPLAIN QUERY PLAN {}", c.sql)).unwrap();
        let n = st.parameter_count();
        st.query_map(rusqlite::params_from_iter(c.params[..n].iter()), |r| r.get::<_, String>(3)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert!(!plan.iter().any(|p| p.contains("USE TEMP B-TREE FOR ORDER BY")), "{plan:?}");
}
