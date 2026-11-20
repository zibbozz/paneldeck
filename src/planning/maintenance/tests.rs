//! Tests der Wartung: je Prüfung und je Aktion am erzeugten Testprojekt mit bekannten Problemen (`fixture.rs`).

use super::actions::{apply, default_comment, preview, Action};
use super::fixture::{build, MaintFx};
use super::rules::{create_rule, list_rules, NewRule, RuleAction};
use super::*;
use crate::planning::history::history_of;
use crate::state::Lang;

fn report(m: &MaintFx) -> Report {
    run_report(&m.fx.c, m.fx.project, &RunOpts::full(m.now, Lang::En)).unwrap()
}

fn ids_of(r: &Report, c: Check) -> Vec<String> {
    let mut v: Vec<String> = r.findings.iter().filter(|f| f.check == c).map(|f| f.id.clone()).collect();
    v.sort();
    v
}

fn set_wont_do(m: &MaintFx) {
    let s = Settings { wont_do: Some(m.fx.done), ..Default::default() };
    save_settings(&m.fx.c, m.fx.project, &s).unwrap();
}

#[test]
fn ticket_checks_find_exactly_the_known_problems() {
    let m = build();
    let r = report(&m);
    assert_eq!(ids_of(&r, Check::StaleNew), vec!["stale_new:PAN-1"]);
    assert_eq!(ids_of(&r, Check::StaleInProgress), vec!["stale_in_progress:PAN-2"]);
    assert_eq!(ids_of(&r, Check::Overdue), vec!["overdue:PAN-4"]);
    assert_eq!(ids_of(&r, Check::PossibleDuplicate), vec!["possible_duplicate:PAN-5:PAN-6"]);
    assert_eq!(ids_of(&r, Check::ParentDoneChildrenOpen), vec!["parent_done_children_open:PAN-7"]);
    assert_eq!(ids_of(&r, Check::OrphanedSubtask), vec!["orphaned_subtask:PAN-9"]);
    assert_eq!(ids_of(&r, Check::BrokenReference), vec!["broken_reference:PAN-3"]);
    assert_eq!(ids_of(&r, Check::MissingRequired), vec!["missing_required:PAN-12"]);
    assert_eq!(ids_of(&r, Check::InvalidValue), vec!["invalid_value:PAN-13"]);
    let cyc = ids_of(&r, Check::LinkCycle);
    assert_eq!(cyc.len(), 1);
    assert!(cyc[0].ends_with(":PAN-10,PAN-11"), "{cyc:?}");
    assert!(!r.findings.iter().any(|f| f.ticket.as_ref().is_some_and(|t| t.key == "PAN-14")), "archivierte Tickets werden nicht geprüft");
    let dup = r.findings.iter().find(|f| f.check == Check::PossibleDuplicate).unwrap();
    assert_eq!(dup.other.as_ref().unwrap().key, "PAN-6");
    assert!(dup.detail["percent"].as_u64().unwrap() >= 80);
    assert_eq!(dup.severity, Severity::Hint);
    assert_eq!(r.findings.iter().find(|f| f.check == Check::Overdue).unwrap().severity, Severity::Warning);
    assert_eq!(r.findings.iter().find(|f| f.check == Check::OrphanedSubtask).unwrap().severity, Severity::Error);
}

#[test]
fn config_and_database_checks() {
    let m = build();
    let r = report(&m);
    let labels = r.findings.iter().find(|f| f.check == Check::LabelsSimilar).expect("Labels");
    assert_eq!(labels.target, Target::Labels { keep: "bug".into(), merge: vec!["Bug".into()] });
    assert!(r.findings.iter().any(|f| f.check == Check::TypeUnused), "Teilaufgabe hat keine Tickets");
    assert!(r.findings.iter().any(|f| f.check == Check::WorkKindUnused));
    assert!(ids_of(&r, Check::DbCounter).contains(&"db_counter:project:PAN".to_string()));
    assert!(ids_of(&r, Check::DbForeignKey).iter().any(|i| i.contains("comments.ticket_id")), "{:?}", ids_of(&r, Check::DbForeignKey));
    assert!(ids_of(&r, Check::DbIntegrity).is_empty());
    assert!(r.count(Category::Database) >= 2);
}

#[test]
fn messages_follow_the_language() {
    let m = build();
    let de = run_report(&m.fx.c, m.fx.project, &RunOpts::full(m.now, Lang::De)).unwrap();
    let f = de.findings.iter().find(|f| f.check == Check::StaleNew).unwrap();
    assert!(f.message.contains("Nie begonnen"), "{}", f.message);
    let en = report(&m);
    assert!(en.findings.iter().find(|f| f.check == Check::StaleNew).unwrap().message.contains("Never started"));
}

#[test]
fn thresholds_and_switches_per_project() {
    let m = build();
    let s = Settings { stale_days: 150, disabled: vec![Check::PossibleDuplicate], ..Default::default() };
    save_settings(&m.fx.c, m.fx.project, &s).unwrap();
    assert_eq!(load_settings(&m.fx.c, m.fx.project).unwrap(), s);
    let r = report(&m);
    assert_eq!(ids_of(&r, Check::StaleNew), vec!["stale_new:PAN-1"], "200 Tage ≥ 150");
    assert!(ids_of(&r, Check::StaleInProgress).is_empty(), "120 Tage < 150");
    assert!(ids_of(&r, Check::PossibleDuplicate).is_empty(), "abgeschaltet");
    // Grenzen werden eingehalten
    save_settings(&m.fx.c, m.fx.project, &Settings { stale_days: 0, duplicate_percent: 10, ..Default::default() }).unwrap();
    let s = load_settings(&m.fx.c, m.fx.project).unwrap();
    assert_eq!((s.stale_days, s.duplicate_percent), (1, 50));
}

#[test]
fn ignored_findings_are_remembered_and_restorable() {
    let m = build();
    ignore(&m.fx.c, m.fx.project, &["stale_new:PAN-1".to_string()]).unwrap();
    let r = report(&m);
    assert!(ids_of(&r, Check::StaleNew).is_empty());
    assert_eq!(r.ignored, 1);
    let all = run_report(&m.fx.c, m.fx.project, &RunOpts { include_ignored: true, ..RunOpts::full(m.now, Lang::En) }).unwrap();
    assert_eq!(ids_of(&all, Check::StaleNew), vec!["stale_new:PAN-1"]);
    unignore(&m.fx.c, m.fx.project, &["stale_new:PAN-1".to_string()]).unwrap();
    assert_eq!(ids_of(&report(&m), Check::StaleNew).len(), 1);
}

#[test]
fn system_project_has_no_maintenance() {
    let m = build();
    let todo: i64 = m.fx.c.query_row("SELECT id FROM projects WHERE key = 'TODO'", [], |r| r.get(0)).unwrap();
    assert_eq!(run_report(&m.fx.c, todo, &RunOpts::full(m.now, Lang::En)).unwrap_err(), MaintError::SystemProject);
}

#[test]
fn close_wont_do_needs_the_target_status_and_writes_comment_history_and_log() {
    let m = build();
    let a = Action::CloseWontDo { tickets: vec![m.id(1), m.id(2)], comment: default_comment(Lang::En, m.now.date(), "unchanged for 200 days") };
    assert!(matches!(preview(&m.fx.c, m.fx.project, &a, Lang::En), Err(MaintError::Refused { .. })), "ohne Zielstatus ist die Aktion aus");
    set_wont_do(&m);
    let p = preview(&m.fx.c, m.fx.project, &a, Lang::En).unwrap();
    assert_eq!(p.count, 2);
    assert!(p.headline.contains("2 ticket(s)"), "{}", p.headline);
    assert!(!p.confirm_slow);
    let out = apply(&m.fx.c, m.fx.project, &a, Lang::En, Some("maintenance-20261120-120000.db")).unwrap();
    assert_eq!(out.changed, 2);
    let t = crate::planning::tickets::get_ticket(&m.fx.c, m.id(1)).unwrap();
    assert_eq!(t.status_id, Some(m.fx.done));
    let c = crate::planning::comments::comments_of(&m.fx.c, m.id(1)).unwrap();
    assert_eq!(c.last().unwrap().body, "Closed by maintenance on 2026-11-20: unchanged for 200 days.");
    let h = history_of(&m.fx.c, m.id(1)).unwrap();
    assert!(h.iter().any(|e| e.source.is_maintenance() && e.source.label(Lang::En) == "Maintenance"));
    let log = read_log(&m.fx.c, m.fx.project).unwrap();
    assert_eq!((log[0].action.as_str(), log[0].count, log[0].backup.as_deref()), ("close_wont_do", 2, Some("maintenance-20261120-120000.db")));
    assert!(log[0].detail.contains("PAN-1"));
    assert!(log_csv(&log).starts_with("at,project,action,count,backup,detail\r\n"));
    // nie gelöscht
    let n: i64 = m.fx.c.query_row("SELECT COUNT(*) FROM tickets WHERE project_id = ?1", [m.fx.project], |r| r.get(0)).unwrap();
    assert_eq!(n, 14);
}

#[test]
fn german_default_comment() {
    assert_eq!(default_comment(Lang::De, super::fixture::now().date(), "seit 94 Tagen unverändert"), "Von der Wartung am 2026-11-20 geschlossen: seit 94 Tagen unverändert.");
}

#[test]
fn archive_hides_from_lists_and_search_and_restores() {
    let m = build();
    let c = &m.fx.c;
    let a = Action::Archive { tickets: vec![m.id(1)] };
    apply(c, m.fx.project, &a, Lang::En, None).unwrap();
    let list = crate::planning::ticket_list::list_tickets(c, Some(m.fx.project), &Default::default()).unwrap();
    assert!(!list.items.iter().any(|t| t.id == m.id(1)));
    let all = crate::planning::ticket_list::list_tickets(c, Some(m.fx.project), &crate::planning::ticket_list::TicketFilter { include_archived: true, ..Default::default() }).unwrap();
    assert!(all.items.iter().any(|t| t.id == m.id(1)));
    let ctx = crate::planning::search::Ctx::new("2026-11-20", Lang::En);
    let scope = crate::planning::search::Scope::Project(m.fx.project);
    let page = crate::planning::search::Page::first(100);
    let hits = |q: &str| crate::planning::search::run_query(c, &ctx, scope, q, page).unwrap().rows.iter().map(|r| r.key.clone()).collect::<Vec<_>>();
    assert!(!hits("").contains(&"PAN-1".to_string()));
    assert!(hits("type = Defect").contains(&"PAN-5".to_string()));
    assert!(!hits("title ~ onboarding").contains(&"PAN-1".to_string()));
    assert!(hits("archived = true").contains(&"PAN-1".to_string()) && hits("archived = true").contains(&"PAN-14".to_string()));
    let incl = crate::planning::search::run_query(c, &ctx.clone().with_archived(true), scope, "", page).unwrap();
    assert!(incl.rows.iter().any(|r| r.key == "PAN-1"));
    assert!(history_of(c, m.id(1)).unwrap().iter().any(|e| crate::planning::history::describe(c, e, Lang::En) == "Archived"));
    apply(c, m.fx.project, &Action::Unarchive { tickets: vec![m.id(1)] }, Lang::En, None).unwrap();
    assert!(!crate::planning::tickets::get_ticket(c, m.id(1)).unwrap().archived);
}

#[test]
fn action_is_rolled_back_when_the_log_cannot_be_written() {
    let m = build();
    let c = &m.fx.c;
    c.execute_batch("ALTER TABLE maintenance_log RENAME TO maintenance_log_away").unwrap();
    let r = apply(c, m.fx.project, &Action::Archive { tickets: vec![m.id(1)] }, Lang::En, None);
    assert!(r.is_err());
    assert!(c.is_autocommit(), "kein offener Savepoint mehr");
    assert!(!crate::planning::tickets::get_ticket(c, m.id(1)).unwrap().archived, "Aktion zurückgenommen");
    c.execute_batch("ALTER TABLE maintenance_log_away RENAME TO maintenance_log").unwrap();
    apply(c, m.fx.project, &Action::Archive { tickets: vec![m.id(1)] }, Lang::En, None).unwrap();
    assert!(crate::planning::tickets::get_ticket(c, m.id(1)).unwrap().archived);
}

#[test]
fn duplicate_search_stops_at_its_budget() {
    let m = build();
    let full = report(&m);
    assert!(!full.duplicates_truncated);
    assert!(full.findings.iter().any(|f| f.check == Check::PossibleDuplicate));
    let capped = RunOpts { duplicate_budget: Some(DupBudget { max_pairs: 0, max_time: std::time::Duration::from_secs(60) }), ..RunOpts::full(m.now, Lang::En) };
    let r = run_report(&m.fx.c, m.fx.project, &capped).unwrap();
    assert!(r.duplicates_truncated);
    assert!(!r.findings.iter().any(|f| f.check == Check::PossibleDuplicate));
    let roomy = RunOpts { duplicate_budget: Some(DupBudget::MCP), ..RunOpts::full(m.now, Lang::En) };
    let r = run_report(&m.fx.c, m.fx.project, &roomy).unwrap();
    assert!(!r.duplicates_truncated && r.findings.iter().any(|f| f.check == Check::PossibleDuplicate));
    // Ohne Integritätsprüfung (Structure) bleiben Fremdschlüssel- und Zähler-Befunde.
    let light = RunOpts { database: Some(DbScope::Structure), ..RunOpts::full(m.now, Lang::En) };
    let r = run_report(&m.fx.c, m.fx.project, &light).unwrap();
    assert!(r.findings.iter().any(|f| f.check == Check::DbForeignKey) && r.findings.iter().any(|f| f.check == Check::DbCounter));
}

#[test]
fn link_duplicate_creates_the_link_type_when_needed() {
    let m = build();
    let c = &m.fx.c;
    c.execute("DELETE FROM link_types WHERE key = 'duplicates'", []).unwrap();
    let a = Action::LinkDuplicate { ticket: m.id(6), original: m.id(5), close: false, comment: String::new() };
    apply(c, m.fx.project, &a, Lang::En, None).unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM links l JOIN link_types lt ON lt.id = l.link_type_id WHERE lt.key = 'duplicates' AND l.from_ticket_id = ?1 AND l.to_ticket_id = ?2", [m.id(6), m.id(5)], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    assert!(ids_of(&report(&m), Check::PossibleDuplicate).is_empty(), "verknüpfte Paare werden nicht mehr vorgeschlagen");
}

#[test]
fn remove_only_unused_configuration() {
    let m = build();
    let c = &m.fx.c;
    let r = report(&m);
    let unused_type = r.findings.iter().find(|f| f.check == Check::TypeUnused).unwrap().target.clone();
    let used = Target::Type(m.fx.task);
    let a = Action::Remove { targets: vec![unused_type.clone(), used] };
    let p = preview(c, m.fx.project, &a, Lang::En).unwrap();
    assert!(p.confirm_slow, "Entfernen braucht immer den 3-Sekunden-Knopf");
    assert_eq!(p.count, 1);
    let out = apply(c, m.fx.project, &a, Lang::En, None).unwrap();
    assert_eq!(out.changed, 1);
    assert_eq!(out.skipped.len(), 1);
    assert!(crate::planning::types::get_type(c, m.fx.task).is_ok());
    if let Target::Type(id) = unused_type {
        assert!(crate::planning::types::get_type(c, id).is_err());
    }
}

#[test]
fn merge_labels_keeps_values() {
    let m = build();
    let a = Action::MergeLabels { keep: "bug".into(), merge: vec!["Bug".into()] };
    let p = preview(&m.fx.c, m.fx.project, &a, Lang::En).unwrap();
    assert_eq!(p.tickets.iter().map(|t| t.key.as_str()).collect::<Vec<_>>(), vec!["PAN-4"]);
    apply(&m.fx.c, m.fx.project, &a, Lang::En, None).unwrap();
    assert!(ids_of(&report(&m), Check::LabelsSimilar).is_empty());
}

#[test]
fn structural_repairs_on_tickets() {
    let m = build();
    let c = &m.fx.c;
    apply(c, m.fx.project, &Action::DetachParent { tickets: vec![m.id(9)] }, Lang::En, None).unwrap();
    apply(c, m.fx.project, &Action::ClearReference { tickets: vec![m.id(3), m.id(13)] }, Lang::En, None).unwrap();
    let r = report(&m);
    assert!(ids_of(&r, Check::OrphanedSubtask).is_empty());
    assert!(ids_of(&r, Check::BrokenReference).is_empty());
    assert!(ids_of(&r, Check::InvalidValue).is_empty());
}

#[test]
fn set_due_and_rules() {
    let m = build();
    let c = &m.fx.c;
    apply(c, m.fx.project, &Action::SetDue { tickets: vec![m.id(4)], due: "2026-12-31".into() }, Lang::En, None).unwrap();
    assert!(ids_of(&report(&m), Check::Overdue).is_empty());
    assert!(preview(c, m.fx.project, &Action::SetDue { tickets: vec![m.id(4)], due: "morgen".into() }, Lang::En).is_err());
    // eigene Regeln: aus, bis eine angelegt ist
    assert!(list_rules(c, m.fx.project).unwrap().is_empty());
    assert_eq!(report(&m).count(Category::Rules), 0);
    let bad = NewRule { name: "x".into(), jql: "nonsense ===".into(), severity: Severity::Warning, action: RuleAction::None, enabled: true };
    assert!(create_rule(c, m.fx.project, &bad, Lang::En).is_err());
    let rule = NewRule { name: "Bugs without priority".into(), jql: "type = Defect".into(), severity: Severity::Error, action: RuleAction::Archive, enabled: true };
    let id = create_rule(c, m.fx.project, &rule, Lang::En).unwrap();
    let r = report(&m);
    let hits: Vec<_> = r.findings.iter().filter(|f| f.check == Check::CustomRule).collect();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|f| f.severity == Severity::Error && f.fixes == vec![Fix::RuleAction]));
    apply(c, m.fx.project, &Action::RuleAction { rule: id, tickets: vec![m.id(5)] }, Lang::En, None).unwrap();
    assert!(crate::planning::tickets::get_ticket(c, m.id(5)).unwrap().archived);
    super::rules::set_enabled(c, id, false).unwrap();
    assert_eq!(report(&m).count(Category::Rules), 0);
}

#[test]
fn quick_count_leaves_out_duplicates_and_database() {
    let m = build();
    let full = report(&m).findings.len();
    let quick = quick_count(&m.fx.c, m.fx.project, m.now).unwrap();
    assert!(quick < full && quick > 0);
}

#[test]
fn every_check_has_a_unique_id_and_both_labels() {
    let mut seen = std::collections::HashSet::new();
    for c in Check::ALL {
        assert!(seen.insert(c.as_str()));
        assert_eq!(Check::parse(c.as_str()), Some(c));
        assert!(!c.label(Lang::De).is_empty() && !c.label(Lang::En).is_empty());
    }
    for c in Category::ALL {
        assert_eq!(Category::parse(c.as_str()), Some(c));
    }
}

#[test]
fn busy_lock_is_released() {
    if std::env::var("PANELDECK_TEST_MAINT_BUSY").is_ok() {
        return;
    }
    {
        let _l = WriteLock::take();
        assert_eq!(busy(), Some(RETRY_AFTER_MS));
    }
    // andere Tests können parallel eine Sperre halten; nach dem Ende dieses Blocks ist unsere jedenfalls frei
    let _ = busy();
}

#[test]
fn strongly_connected_components() {
    let mut g = std::collections::HashMap::new();
    g.insert(1, vec![2]);
    g.insert(2, vec![3]);
    g.insert(3, vec![1]);
    g.insert(4, vec![5]);
    assert_eq!(super::checks_tickets::strongly_connected(&g), vec![vec![1, 2, 3]]);
}

#[test]
fn project_backup_keeps_archive_settings_and_rules() {
    let m = build();
    let c = &m.fx.c;
    set_wont_do(&m);
    // erst die absichtlich kaputten Verweise reparieren (die Sicherung prüft Verweise streng)
    let plan = super::dbcare::plan_repair(c).unwrap();
    super::dbcare::apply_repair(c, &plan).unwrap();
    apply(c, m.fx.project, &Action::ClearReference { tickets: vec![m.id(3), m.id(13)] }, Lang::En, None).unwrap();
    let rule = NewRule { name: "Close old".into(), jql: "type = Defect".into(), severity: Severity::Hint, action: RuleAction::Status { status: m.fx.done }, enabled: false };
    create_rule(c, m.fx.project, &rule, Lang::En).unwrap();
    let doc = crate::planning::backup_doc::export_project(c, m.fx.project).unwrap();
    assert_eq!(doc.version, 4);
    assert!(doc.tickets.iter().any(|t| t.archived));
    let json = serde_json::to_string(&doc).unwrap();
    let back: crate::planning::backup_doc::ProjectDoc = serde_json::from_str(&json).unwrap();
    let res = crate::planning::backup_doc::import_project(c, &back, Some("COPY".into())).unwrap();
    let s = load_settings(c, res.project_id).unwrap();
    let st = crate::planning::statuses::get_status(c, s.wont_do.unwrap()).unwrap();
    assert_eq!((st.project_id, st.key.as_deref()), (res.project_id, Some("done")));
    let rules = list_rules(c, res.project_id).unwrap();
    assert_eq!(rules.len(), 1);
    assert!(!rules[0].enabled);
    match rules[0].action {
        RuleAction::Status { status } => assert_eq!(crate::planning::statuses::get_status(c, status).unwrap().project_id, res.project_id),
        _ => panic!("Aktion"),
    }
    let archived: i64 = c.query_row("SELECT COUNT(*) FROM tickets WHERE project_id = ?1 AND archived = 1", [res.project_id], |r| r.get(0)).unwrap();
    assert_eq!(archived, 1);
}
