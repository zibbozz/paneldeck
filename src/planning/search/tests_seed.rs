//! Prüfungen des Seedings und die Kreuzprobe: jedes Beispiel der Abfrage-Hilfe liefert auf den echten Datenbank-Daten genau
//! das Ergebnis der Referenz-Auswertung.

use super::*;
use crate::planning::fields::{list_fields, FieldKind};
use crate::planning::history::{history_of, SourceKind};
use crate::planning::links::linked_terminals;
use crate::planning::query_help::{shop, DemoField, DemoSpec, DemoStatus, DemoTicket, HelpDoc, REFERENCE_TODAY};
use crate::planning::statuses::{list_statuses, StatusCategory};
use crate::planning::tickets::get_ticket;
use crate::planning::types::list_types;

fn fresh() -> Connection {
    crate::db::open_memory()
}

fn seeded() -> (Connection, SeededDemo, DemoSpec) {
    let c = fresh();
    let spec = shop();
    let d = seed_demo(&c, &spec, REFERENCE_TODAY).unwrap();
    (c, d, spec)
}

fn ctx_for(d: &SeededDemo, lang: Lang) -> Ctx {
    Ctx { today: d.today.clone(), terminals: d.terminals.clone(), lang, timeout: std::time::Duration::from_secs(10) }
}

fn keys_of(c: &Connection, ctx: &Ctx, d: &SeededDemo, jql: &str) -> Result<Vec<String>, SearchError> {
    Ok(run_query(c, ctx, Scope::Project(d.project_id), jql, Page::ALL)?.rows.into_iter().map(|r| r.key).collect())
}

#[test]
fn seeds_the_shop_project() {
    let (c, d, spec) = seeded();
    assert_eq!(d.tickets.len(), spec.tickets.len());
    assert_eq!(d.tickets.len(), 22);
    for (i, (k, id)) in d.tickets.iter().enumerate() {
        assert_eq!(k, &format!("WEB-{}", i + 1));
        assert_eq!(get_ticket(&c, *id).unwrap().key(), *k);
    }
    assert_eq!(d.terminals, vec!["login-fix"]);
    assert_eq!(d.today, REFERENCE_TODAY);
    assert_eq!(d.ticket_id("web-3"), Some(d.tickets[2].1));
}

#[test]
fn seeds_sprints_versions_and_their_values() {
    use crate::planning::sprints::releases::list_versions;
    use crate::planning::sprints::sprint::{active_sprint, list_sprints, sprint_content, SprintState};
    let (c, d, _) = seeded();
    assert!(d.skipped.is_empty(), "{:?}", d.skipped);
    let sprints = list_sprints(&c, d.project_id).unwrap();
    assert_eq!(sprints.iter().map(|s| (s.name.as_str(), s.state)).collect::<Vec<_>>(), vec![("Sprint 14", SprintState::Completed), ("Sprint 15", SprintState::Active), ("Sprint 16", SprintState::Planned)]);
    assert_eq!(active_sprint(&c, d.project_id).unwrap().name, "Sprint 15");
    let in_sprint = |i: usize| sprint_content(&c, sprints[i].id).unwrap().items.iter().map(|x| x.ticket.key()).collect::<Vec<_>>();
    assert_eq!(in_sprint(0), vec!["WEB-6", "WEB-7", "WEB-18"]);
    assert_eq!(in_sprint(1), vec!["WEB-8", "WEB-9", "WEB-10", "WEB-11", "WEB-12", "WEB-14", "WEB-15", "WEB-19", "WEB-20"]);
    assert_eq!(in_sprint(2), vec!["WEB-13"]);
    let versions = list_versions(&c, d.project_id).unwrap();
    assert_eq!(versions.iter().map(|v| (v.name.as_str(), v.done)).collect::<Vec<_>>(), vec![("1.0.0", true), ("1.1.0", true), ("2.0.0", false), ("2.1.0", false)]);
}

#[test]
fn creates_types_statuses_and_fields_like_the_defaults() {
    let (c, d, _) = seeded();
    let st = list_statuses(&c, d.project_id).unwrap();
    assert_eq!(st.len(), 4);
    assert_eq!(st[0].key.as_deref(), Some("todo"));
    assert!(st[0].name.is_none(), "Standard-Status übersetzen sich");
    assert_eq!((st[2].key.clone(), st[2].name.clone(), st[2].category), (None, Some("Development finished".into()), StatusCategory::Progress));
    let ty = list_types(&c, d.project_id).unwrap();
    assert_eq!(ty.iter().map(|t| t.key.clone().unwrap_or_default()).collect::<Vec<_>>(), vec!["epic", "userstory", "devtask", "subtask", "defect"]);
    assert!(ty.iter().all(|t| t.workflow));
    let f = list_fields(&c, ty[1].id).unwrap();
    let prio = f.iter().find(|x| x.key.as_deref() == Some("priority")).unwrap();
    assert_eq!(prio.options.values.len(), 5);
    assert_eq!(prio.options.values[0].key.as_deref(), Some("priority.highest"));
    assert!(f.iter().any(|x| x.key.as_deref() == Some("story_points")) && f.iter().any(|x| x.name.as_deref() == Some("Labels")));
}

#[test]
fn sets_parents_flags_stars_and_terminal_links() {
    let (c, d, _) = seeded();
    let id = |k: &str| d.ticket_id(k).unwrap();
    assert_eq!(get_ticket(&c, id("WEB-6")).unwrap().parent_id, Some(id("WEB-2")));
    assert_eq!(get_ticket(&c, id("WEB-19")).unwrap().parent_id, Some(id("WEB-12")));
    assert_eq!(get_ticket(&c, id("WEB-1")).unwrap().parent_id, None);
    let starred: Vec<String> = d.tickets.iter().filter(|(_, i)| get_ticket(&c, *i).unwrap().starred).map(|(k, _)| k.clone()).collect();
    assert_eq!(starred, vec!["WEB-1", "WEB-8", "WEB-11", "WEB-14"]);
    let flagged: Vec<String> = d.tickets.iter().filter(|(_, i)| get_ticket(&c, *i).unwrap().flagged).map(|(k, _)| k.clone()).collect();
    assert_eq!(flagged, vec!["WEB-10", "WEB-15"]);
    assert_eq!(linked_terminals(&c, id("WEB-12")).unwrap(), vec!["login-fix"]);
    assert_eq!(linked_terminals(&c, id("WEB-11")).unwrap(), vec!["api-work"]);
    assert!(linked_terminals(&c, id("WEB-1")).unwrap().is_empty());
}

#[test]
fn the_last_change_of_every_ticket_has_the_requested_source() {
    let (c, d, spec) = seeded();
    for (t, (k, id)) in spec.tickets.iter().zip(&d.tickets) {
        let h = history_of(&c, *id).unwrap();
        let last = h.last().unwrap_or_else(|| panic!("{k} hat keinen Verlauf"));
        let want = match t.changed_by.as_deref() {
            Some("agent") => SourceKind::Agent,
            Some("import") => SourceKind::Import,
            _ => SourceKind::Ui,
        };
        assert_eq!(last.source.kind, want, "{k}");
    }
}

#[test]
fn tickets_without_changed_by_have_no_history() {
    let mut spec = DemoSpec {
        project_key: "TST".into(),
        project_name: "Test".into(),
        types: vec!["Task".into()],
        statuses: vec![DemoStatus { name: "To Do".into(), category: StatusCategory::Todo }],
        ..Default::default()
    };
    spec.add(DemoTicket::new("Task", "A").status("To Do"));
    spec.add(DemoTicket::new("Task", "B").status("To Do").changed_by("agent"));
    let c = fresh();
    let d = seed_demo(&c, &spec, REFERENCE_TODAY).unwrap();
    assert!(history_of(&c, d.tickets[0].1).unwrap().is_empty());
    assert!(!history_of(&c, d.tickets[1].1).unwrap().is_empty());
    let ctx = ctx_for(&d, Lang::En);
    assert_eq!(keys_of(&c, &ctx, &d, "changedBy IS EMPTY").unwrap(), vec!["TST-1"]);
    assert_eq!(keys_of(&c, &ctx, &d, "changedBy = agent").unwrap(), vec!["TST-2"]);
}

#[test]
fn relative_dates_are_resolved_against_today() {
    let c = fresh();
    let d = seed_demo(&c, &shop(), "2027-03-10").unwrap();
    let ctx = Ctx { today: "2027-03-10".into(), ..ctx_for(&d, Lang::En) };
    // WEB-11 hat „Due = @-2“
    assert_eq!(keys_of(&c, &ctx, &d, "due = 2027-03-08 AND type = \"Dev task\"").unwrap(), vec!["WEB-11"]);
    assert!(keys_of(&c, &ctx, &d, "due < currentDate() AND status != Done").unwrap().contains(&"WEB-11".to_string()));
}

#[test]
fn a_second_seeding_with_the_same_key_is_refused_and_changes_nothing() {
    let (c, d, spec) = seeded();
    assert_eq!(seed_demo(&c, &spec, REFERENCE_TODAY).unwrap_err(), SeedError::NotFresh);
    let n: i64 = c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
    assert_eq!(n as usize, d.tickets.len());
}

#[test]
fn a_contradictory_spec_is_refused() {
    let c = fresh();
    let mut spec = shop();
    spec.tickets[0].ty = "Nope".into();
    match seed_demo(&c, &spec, REFERENCE_TODAY).unwrap_err() {
        SeedError::Spec(p) => assert!(p.iter().any(|m| m.contains("unknown type"))),
        e => panic!("{e:?}"),
    }
}

#[test]
fn a_failed_seeding_leaves_no_project_behind() {
    let c = fresh();
    let mut spec = shop();
    spec.project_key = "XYZ".into(); // die Ticket-Schlüssel (WEB-n) passen nicht mehr
    let e = seed_demo(&c, &spec, REFERENCE_TODAY).unwrap_err();
    assert!(matches!(e, SeedError::BadKey(_)), "{e:?}");
    let n: i64 = c.query_row("SELECT COUNT(*) FROM projects WHERE key = 'XYZ'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    assert!(!e.message(Lang::De).is_empty() && e.message(Lang::De) != e.message(Lang::En));
}

#[test]
fn seeds_a_minimal_custom_spec_with_every_field_kind_it_can() {
    let mut spec = DemoSpec {
        project_key: "MIN".into(),
        project_name: "Minimal".into(),
        types: vec!["Task".into(), "Bug".into()],
        statuses: vec![DemoStatus { name: "Open".into(), category: StatusCategory::Todo }, DemoStatus { name: "Closed".into(), category: StatusCategory::Done }],
        priorities: vec!["P1".into(), "P2".into()],
        fields: vec![
            DemoField { name: "Size".into(), kind: FieldKind::Number, options: vec![], types: vec![] },
            DemoField { name: "Flavor".into(), kind: FieldKind::Dropdown, options: vec!["Sweet".into(), "Sour".into()], types: vec!["Task".into()] },
            DemoField { name: "Parts".into(), kind: FieldKind::Multi, options: vec!["a".into(), "b".into()], types: vec![] },
            DemoField { name: "Done".into(), kind: FieldKind::Checkbox, options: vec![], types: vec![] },
            DemoField { name: "Related".into(), kind: FieldKind::Ticket, options: vec![], types: vec![] },
        ],
        ..Default::default()
    };
    spec.add(DemoTicket::new("Task", "One").status("Open").priority("P2").num("Size", 3.0).changed_by("ui"));
    let mut t = DemoTicket::new("Bug", "Two").status("Closed").priority("P1").changed_by("ui");
    t.values.insert("Done".into(), crate::planning::query_help::DemoValue::Bool(true));
    t.values.insert("Parts".into(), crate::planning::query_help::DemoValue::List(vec!["b".into()]));
    t.values.insert("Related".into(), crate::planning::query_help::DemoValue::Text("MIN-1".into()));
    spec.add(t);
    assert!(spec.check().is_empty(), "{:?}", spec.check());
    let c = fresh();
    let d = seed_demo(&c, &spec, REFERENCE_TODAY).unwrap();
    let ctx = ctx_for(&d, Lang::En);
    assert_eq!(keys_of(&c, &ctx, &d, "size > 2").unwrap(), vec!["MIN-1"]);
    assert_eq!(keys_of(&c, &ctx, &d, "done = true").unwrap(), vec!["MIN-2"]);
    assert_eq!(keys_of(&c, &ctx, &d, "parts = b").unwrap(), vec!["MIN-2"]);
    assert_eq!(keys_of(&c, &ctx, &d, "related = MIN-1").unwrap(), vec!["MIN-2"]);
    assert_eq!(keys_of(&c, &ctx, &d, "priority = P1").unwrap(), vec!["MIN-2"]);
    assert_eq!(keys_of(&c, &ctx, &d, "type = Bug AND status = Closed").unwrap(), vec!["MIN-2"]);
}

#[test]
fn seeded_cards_show_the_priority_and_points() {
    let (c, d, _) = seeded();
    let ctx = ctx_for(&d, Lang::En);
    let r = run_query(&c, &ctx, Scope::Project(d.project_id), "key = WEB-8", Page::ALL).unwrap().rows.remove(0);
    assert_eq!((r.priority.as_deref(), r.story_points, r.status_name.as_deref(), r.type_name.as_str()), (Some("Highest"), Some(8.0), Some("In Progress"), "User story"));
}

/// Die Kreuzprobe: jedes Beispiel der Hilfe, auf frisch gesäten Daten, liefert genau `expected_keys()` (auch die Reihenfolge).
fn cross_check(lang: Lang) -> usize {
    let mut checked = 0;
    for e in HelpDoc::builtin().examples() {
        let c = fresh();
        let d = seed_demo(&c, &e.demo, REFERENCE_TODAY).unwrap();
        let ctx = ctx_for(&d, lang);
        match keys_of(&c, &ctx, &d, &e.jql) {
            Ok(keys) => {
                assert_eq!(keys, e.expected_keys(), "{} ({:?}): {}", e.id, lang, e.jql);
                checked += 1;
            }
            Err(err) => panic!("{}: {}", e.id, err.message(Lang::En)),
        }
    }
    checked
}

#[test]
fn cross_check_with_the_help_examples_in_english() {
    let checked = cross_check(Lang::En);
    assert_eq!(checked, HelpDoc::builtin().examples().len(), "jedes Beispiel der Hilfe stimmt mit der Referenz überein");
    assert_eq!(checked, 29);
}

#[test]
fn cross_check_with_the_help_examples_in_german() {
    assert_eq!(cross_check(Lang::De), HelpDoc::builtin().examples().len());
}

#[test]
fn every_example_count_matches_the_expected_total() {
    for e in HelpDoc::builtin().examples() {
        let c = fresh();
        let d = seed_demo(&c, &e.demo, REFERENCE_TODAY).unwrap();
        let n = count(&c, &ctx_for(&d, Lang::En), Scope::Project(d.project_id), &e.jql).unwrap();
        assert_eq!(n as usize, e.expected_keys().len(), "{}", e.id);
    }
}

/// Erzeugt für die Screenshots der Abfrage-Hilfe (`tools/query-help-shots.ps1`) je Beispiel einen frischen Datenordner
/// `PANELDECK_QHELP_DIR/<id>/paneldeck-data` (Datenbank aus `Example::demo` mit dem heutigen Datum, Expertenmodus, englische
/// Oberfläche, MCP-Stufe „Steuern“ für das Werkzeug `screenshot`). Vorher wird geprüft, dass die echte Suche auf diesen Daten genau
/// `Example::expected_keys` liefert. Optional `PANELDECK_QHELP_ONLY` = kommagetrennte Beispiel-IDs.
#[test]
#[ignore = "erzeugt Dateien für die Screenshots; mit --ignored und PANELDECK_QHELP_DIR starten"]
fn query_help_shot_data() {
    let root = std::env::var("PANELDECK_QHELP_DIR").expect("PANELDECK_QHELP_DIR fehlt");
    let only: Vec<String> = std::env::var("PANELDECK_QHELP_ONLY").unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let today = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
    let mut made = 0;
    for e in HelpDoc::builtin().examples() {
        if !only.is_empty() && !only.iter().any(|o| o == e.id) {
            continue;
        }
        let dir = std::path::Path::new(&root).join(e.id).join("paneldeck-data");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!dir.join(crate::db::FILE).exists(), "{}: Datenordner ist nicht leer", dir.display());
        let db = crate::db::open(&dir, &dir.join("backups")).unwrap_or_else(|e| panic!("Datenbank: {e:?}"));
        let d = seed_demo(&db.conn, &e.demo, &today).unwrap_or_else(|err| panic!("{}: {}", e.id, err.message(Lang::En)));
        let keys = keys_of(&db.conn, &ctx_for(&d, Lang::En), &d, &e.jql).unwrap_or_else(|err| panic!("{}: {}", e.id, err.message(Lang::En)));
        assert_eq!(keys, e.expected_keys(), "{}: {}", e.id, e.jql);
        db.set_meta("expert_mode", "1");
        db.set_meta("expert_project", &d.project_id.to_string());
        drop(db);
        let mut settings = crate::state::Settings::default();
        settings.language = Lang::En;
        settings.backup_interval_hours = 0;
        settings.mcp_level = crate::mcp::McpLevel::Control;
        crate::state::save_json(&dir.join("settings.json"), &settings);
        println!("Beispiel {}: {} Treffer {}", e.id, keys.len(), keys.join(","));
        made += 1;
    }
    assert!(made > 0, "kein Beispiel erzeugt");
}
