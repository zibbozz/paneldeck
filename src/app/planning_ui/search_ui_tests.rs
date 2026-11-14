//! Tests des Reiters „Suche“ und der Ticket-Liste mit Abfrage ohne Fenster: egui läuft mit künstlichen Eingaben, die Daten liegen in
//! einer echten Testdatenbank (Datei, also läuft die Abfrage im Hintergrund-Faden). Geprüft werden Abfrage → Ergebnis, Fehler,
//! Sortieren, Seiten, „alle Projekte“, Aktualisierung nach Änderungen, Schema (Sprints, Konflikte), die Ticket-Liste mit Abfrage und
//! beides in Deutsch und Englisch.

use super::search_logic::{Col, SortBy, PAGE_SIZE};
use super::search_run::SchemaCache;
use super::search_table::{draw_error, draw_table, TableCfg, TableOut};
use super::{Cx, PlanUi, Tab};
use crate::app::tasks_ui::Env;
use crate::db::Db;
use crate::planning::history::Source;
use crate::planning::projects::{create_project, NewProject};
use crate::planning::search::{Scope, SearchError, TicketRow};
use crate::planning::tickets::{create_ticket, set_starred, NewTicket};
use crate::planning::types::list_types;
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use std::time::{Duration, Instant};

struct H {
    ctx: egui::Context,
    db: Db,
    ui: PlanUi,
    lang: Lang,
    time: f64,
    project: i64,
    other: i64,
}

fn enter(pressed: bool) -> Event {
    Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
}

impl H {
    fn new(lang: Lang) -> H {
        // je Aufruf ein eigener Ordner: der Hintergrund-Leser des vorigen Laufs hält seine Datei vielleicht noch offen
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("paneldeck-test-searchui-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::open(&dir, &dir.join("backups")).unwrap();
        let mk = |key: &str, name: &str| {
            let mut np = NewProject::new(key, name, "#58A6E6");
            np.with_defaults = true;
            create_project(&db.conn, &np).unwrap()
        };
        let project = mk("QQ", "Query test");
        let other = mk("ZZ", "Other project");
        db.set_meta("expert_mode", "1");
        db.set_meta("expert_project", &project.to_string());
        let mut h = H { ctx: egui::Context::default(), db, ui: PlanUi::default(), lang, time: 0.0, project, other };
        h.frames(3);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.1;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))), time: Some(self.time), events, focused: true, ..Default::default() };
        let H { ctx, db, ui, lang, .. } = self;
        let env = Env { db: Some(db), db_error: None, lang: *lang, bg: Color32::from_rgb(24, 24, 28), undo_steps: 25 };
        let _ = ctx.run(input, |ctx| {
            ui.ensure_loaded(Some(db));
            ui.header(ctx, &env, db);
            if ui.expert {
                ui.page(ctx, &env, db);
            }
        });
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(Vec::new());
        }
    }

    /// Bilder laufen lassen, bis `done` wahr ist (der Hintergrund-Faden weckt die Oberfläche nur im echten Fenster).
    fn wait(&mut self, what: &str, done: impl Fn(&mut H) -> bool) {
        let t = Instant::now();
        loop {
            self.frame(Vec::new());
            if done(self) {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(15), "Zeitüberschreitung: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn type_id(&self, project: i64, key: &str) -> i64 {
        list_types(&self.db.conn, project).unwrap().into_iter().find(|t| t.key.as_deref() == Some(key)).unwrap().id
    }

    fn mk(&self, project: i64, key: &str, title: &str) -> i64 {
        create_ticket(&self.db.conn, &NewTicket::new(project, self.type_id(project, key), title), &Source::ui()).unwrap().0.id
    }

    /// Abfrage in die Leiste des Reiters „Suche“ tippen und mit Enter abschicken.
    fn submit_search(&mut self, q: &str) {
        self.ui.tab = Tab::Search;
        self.ui.search.bar.set_query(q);
        self.frames(2);
        self.frame(vec![enter(true)]);
        self.frame(vec![enter(false)]);
        let q = q.to_string();
        self.wait("Suche", move |h| h.ui.search.last.as_deref() == Some(q.trim()) && !h.ui.search.res.run.loading() && h.ui.search.res.run.shown.is_some());
    }

    fn titles(&self) -> Vec<String> {
        self.ui.search.res.run.rows().iter().map(|r| r.title.clone()).collect()
    }

    fn click(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    }
}

fn both(f: impl Fn(&mut H)) {
    for lang in [Lang::En, Lang::De] {
        let mut h = H::new(lang);
        f(&mut h);
    }
}

fn seed_basic(h: &H) {
    for t in ["Alpha epic", "Beta epic", "Gamma epic"] {
        h.mk(h.project, "epic", t);
    }
    for t in ["Delta task", "Epsilon task"] {
        h.mk(h.project, "devtask", t);
    }
    h.mk(h.other, "epic", "Zeta other epic");
}

// ------------------------------------------------------------------ Reiter „Suche“

#[test]
fn a_submitted_query_runs_and_fills_the_table_in_both_languages() {
    both(|h| {
        seed_basic(h);
        h.ui.invalidate();
        h.submit_search("type = Epic");
        assert_eq!(h.ui.search.last.as_deref(), Some("type = Epic"));
        assert_eq!(h.ui.search.res.run.total(), 3);
        let mut t = h.titles();
        t.sort();
        assert_eq!(t, vec!["Alpha epic", "Beta epic", "Gamma epic"], "{:?}: nur Epics des Projekts", h.lang);
        assert!(h.ui.search.res.run.error().is_none());
        // der Verlauf wurde gemerkt (aus der Leiste)
        assert!(h.db.meta("query_history").is_some_and(|x| x.contains("type = Epic")));
        // die Zeilen tragen alles, was die Tabelle zeigt
        let r = &h.ui.search.res.run.rows()[0];
        assert!(!r.key.is_empty() && !r.type_name.is_empty() && r.status_name.is_some());
        // ein Ticket ohne Treffer: leerer Zustand, kein Fehler
        h.submit_search("title ~ nirgendwo");
        assert_eq!(h.ui.search.res.run.total(), 0);
        assert!(h.ui.search.res.run.error().is_none());
        // eine leere Abfrage zeigt alle Tickets des Projekts
        h.submit_search("");
        assert_eq!(h.ui.search.res.run.total(), 5);
    });
}

#[test]
fn execution_errors_are_kept_with_their_positions_and_drawn() {
    both(|h| {
        seed_basic(h);
        // am Eingabefeld vorbei (z. B. aus der Hilfe oder einem alten Filter): ein Tippfehler in der Abfrage
        h.ui.tab = Tab::Search;
        h.ui.run_search("stauts = Done");
        h.wait("Fehler", |h| h.ui.search.res.run.shown.is_some() && !h.ui.search.res.run.loading());
        let e = h.ui.search.res.run.error().cloned().expect("Fehler der Ausführung");
        let problems = e.problems(h.lang);
        assert!(!problems.is_empty() && problems[0].start == 0, "{problems:?}");
        assert!(!e.message(h.lang).is_empty());
        assert_eq!(h.ui.search.res.run.rows().len(), 0);
        h.frames(2); // zeichnet den Fehler samt hervorgehobener Stelle
        // eine gute Abfrage danach räumt den Fehler weg
        h.ui.run_search("status = \"To Do\"");
        h.wait("danach", |h| h.ui.search.res.run.error().is_none() && h.ui.search.res.run.shown.is_some() && !h.ui.search.res.run.loading());
        assert_eq!(h.ui.search.res.run.total(), 5);
    });
}

#[test]
fn every_kind_of_error_draws_in_both_languages() {
    let ctx = egui::Context::default();
    let errors = [
        SearchError::Timeout,
        SearchError::NotAvailableYet(crate::planning::search::Feature::Sprints),
        SearchError::TooLong { len: 9000, max: 8000 },
        SearchError::TooDeep { max: 40 },
        SearchError::TooComplex { max: 0 },
        SearchError::NotFound,
        SearchError::Db("kaputt".into()),
        SearchError::Internal("x".into()),
    ];
    for lang in [Lang::En, Lang::De] {
        for e in &errors {
            assert!(e.problems(lang).is_empty());
            assert!(!e.message(lang).is_empty());
            let _ = ctx.run(RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| draw_error(ui, lang, e, "status = Done"));
            });
        }
    }
}

#[test]
fn sorting_by_a_column_sorts_all_hits_and_a_query_order_wins() {
    both(|h| {
        seed_basic(h);
        h.submit_search("");
        // Titel absteigend
        h.ui.search.res.sort = Some(SortBy { col: Col::Title, desc: true });
        h.wait("Titel absteigend", |h| h.titles().first().map(String::as_str) == Some("Gamma epic"));
        let t = h.titles();
        let mut sorted = t.clone();
        sorted.sort_by_key(|s| std::cmp::Reverse(s.to_lowercase()));
        assert_eq!(t, sorted, "{:?}", h.lang);
        let req = h.ui.search.res.run.requested().unwrap().clone();
        assert!(req.jql.ends_with("ORDER BY title DESC"), "{}", req.jql);
        // Schlüssel aufsteigend
        h.ui.search.res.sort = Some(SortBy { col: Col::Key, desc: false });
        h.wait("Schlüssel", |h| h.titles().first().map(String::as_str) == Some("Alpha epic"));
        let keys: Vec<String> = h.ui.search.res.run.rows().iter().map(|r| r.key.clone()).collect();
        assert_eq!(keys, vec!["QQ-1", "QQ-2", "QQ-3", "QQ-4", "QQ-5"]);
        // eine Abfrage mit eigenem ORDER BY behält ihre Reihenfolge, die Sortierung der Spalte wird nicht angehängt
        h.submit_search("ORDER BY title DESC");
        let req = h.ui.search.res.run.requested().unwrap().clone();
        assert_eq!(req.jql, "ORDER BY title DESC");
        assert_eq!(h.titles().first().map(String::as_str), Some("Gamma epic"));
        // Stern zuerst
        let first = h.ui.search.res.run.rows().iter().find(|r| r.title == "Delta task").unwrap().id;
        set_starred(&h.db.conn, first, true, &Source::ui()).unwrap();
        h.ui.search.last = Some(String::new());
        h.ui.search.res.sort = Some(SortBy { col: Col::Marks, desc: true });
        h.ui.search.res.restart();
        h.wait("Stern zuerst", |h| h.titles().first().map(String::as_str) == Some("Delta task"));
    });
}

#[test]
fn all_projects_changes_the_scope_the_schema_and_the_columns() {
    both(|h| {
        seed_basic(h);
        h.submit_search("type = Epic");
        assert_eq!(h.ui.search.res.run.total(), 3);
        assert_eq!(h.ui.search.schema.key(), Some((Scope::Project(h.project), h.lang)));
        let before = h.ui.search.schema.loaded().unwrap();
        assert!(before.schema.field("project").is_some());
        // Umschalten auf „alle Projekte“: das Schema wird neu gebaut und die Abfrage läuft im neuen Bereich
        h.ui.search.scope_all = true;
        h.wait("alle Projekte", |h| h.ui.search.res.run.total() == 4 && !h.ui.search.res.run.loading());
        assert_eq!(h.ui.search.schema.key(), Some((Scope::All, h.lang)));
        assert!(h.titles().iter().any(|t| t == "Zeta other epic"));
        assert!(h.ui.search.res.run.rows().iter().any(|r| r.project_key == "ZZ"));
        h.frames(2); // zeichnet die Spalte „Projekt“
        // wieder zurück
        h.ui.search.scope_all = false;
        h.wait("zurück", |h| h.ui.search.res.run.total() == 3 && !h.ui.search.res.run.loading());
        // eine Abfrage mit dem Projekt-Feld
        h.ui.search.scope_all = true;
        h.submit_search("project = ZZ");
        assert_eq!(h.ui.search.res.run.total(), 1);
    });
}

#[test]
fn the_system_project_always_searches_all_projects() {
    let mut h = H::new(Lang::En);
    seed_basic(&h);
    let sys = crate::planning::projects::project_by_key(&h.db.conn, "TODO").unwrap().id;
    h.ui.select(&Cx { conn: &h.db.conn, db: &h.db, lang: h.lang }, sys);
    h.frames(3);
    h.ui.tab = Tab::Search;
    h.frames(3);
    assert!(h.ui.search.scope_all, "das System-Projekt hat keine Tickets");
    h.ui.run_search("type = Epic");
    h.wait("alle", |h| h.ui.search.res.run.shown.is_some() && !h.ui.search.res.run.loading());
    assert_eq!(h.ui.search.res.run.total(), 4);
}

#[test]
fn results_are_paged_and_a_page_past_the_end_falls_back() {
    both(|h| {
        for i in 0..(PAGE_SIZE + 37) {
            h.mk(h.project, "devtask", &format!("Bulk {i:03}"));
        }
        h.submit_search("");
        assert_eq!(h.ui.search.res.run.total(), PAGE_SIZE + 37);
        assert_eq!(h.ui.search.res.run.rows().len() as i64, PAGE_SIZE);
        h.ui.search.res.page = 1;
        h.wait("Seite 2", |h| h.ui.search.res.run.rows().len() == 37 && !h.ui.search.res.run.loading());
        assert_eq!(h.ui.search.res.run.rows()[0].title, "Bulk 100");
        h.frames(2);
        // zu weit geblättert (z. B. weil Tickets gelöscht wurden): die letzte Seite
        h.ui.search.res.page = 9;
        h.wait("Korrektur", |h| h.ui.search.res.page == 1 && !h.ui.search.res.run.loading() && h.ui.search.res.run.rows().len() == 37);
        // eine neue Abfrage beginnt wieder bei Seite 1
        h.submit_search("title ~ \"Bulk 00\"");
        assert_eq!(h.ui.search.res.page, 0);
        assert_eq!(h.ui.search.res.run.total(), 10);
    });
}

#[test]
fn data_changes_refresh_the_results() {
    both(|h| {
        seed_basic(h);
        h.submit_search("starred = true");
        assert_eq!(h.ui.search.res.run.total(), 0);
        let id = h.mk(h.project, "epic", "Neu und markiert");
        set_starred(&h.db.conn, id, true, &Source::ui()).unwrap();
        // die Oberfläche erfährt von der Änderung (wie nach einer Änderung durch einen Agenten)
        h.ui.invalidate();
        h.wait("aktualisiert", |h| h.ui.search.res.run.total() == 1 && !h.ui.search.res.run.loading());
        assert!(h.ui.search.res.run.rows()[0].starred);
        assert_eq!(h.ui.search.res.run.rows()[0].title, "Neu und markiert");
    });
}

#[test]
fn the_help_example_is_taken_over_and_run_at_once() {
    both(|h| {
        seed_basic(h);
        h.ui.help.open_at(None);
        h.frames(2);
        // dieselbe Übernahme wie beim Klick auf „In Suche übernehmen“
        h.ui.open_search_query("type = Epic", None);
        h.ui.help.open = false;
        assert_eq!(h.ui.tab, Tab::Search);
        assert_eq!(h.ui.search.last.as_deref(), Some("type = Epic"));
        h.wait("Hilfe-Beispiel", |h| h.ui.search.res.run.shown.is_some() && !h.ui.search.res.run.loading());
        assert_eq!(h.ui.search.bar.text, "type = Epic");
        assert_eq!(h.ui.search.res.run.total(), 3);
        // mit Bereich: von außen auf „alle Projekte“
        h.ui.open_search_query("type = Epic", Some(true));
        h.wait("alle", |h| h.ui.search.res.run.total() == 4 && !h.ui.search.res.run.loading());
        assert!(h.ui.search.scope_all);
    });
}

#[test]
fn a_saved_filter_query_runs_like_a_typed_one() {
    // Der Filter wird über die Leiste geladen und sofort abgeschickt (`FilterEvent::Load` → `submitted` → `run_search`); hier der
    // Weg ab `run_search` plus die Ausführung des gespeicherten Filters in der Suchschicht mit demselben Ergebnis.
    let mut h = H::new(Lang::En);
    seed_basic(&h);
    let f = crate::planning::views::filters::create_filter(
        &h.db.conn,
        &crate::planning::views::filters::NewFilter { name: "Epics".into(), jql: "type = Epic".into(), project_id: Some(h.project) },
    )
    .unwrap();
    let r = crate::planning::search::run_saved_filter(&h.db.conn, &crate::planning::search::Ctx::today_local(Lang::En), f, crate::planning::search::Page::first(10)).unwrap();
    assert_eq!(r.total, 3);
    h.ui.tab = Tab::Search;
    h.ui.run_search("type = Epic");
    h.wait("Filter", |h| h.ui.search.res.run.total() == 3 && !h.ui.search.res.run.loading());
}

#[test]
fn the_tab_help_keywords_and_history_work_end_to_end() {
    both(|h| {
        seed_basic(h);
        h.ui.tab = Tab::Search;
        h.frames(2);
        assert!(h.ui.search.schema.loaded().is_some(), "das Schema der Leiste wird beim ersten Zeichnen gebaut");
        h.submit_search("type = Epic");
        assert!(h.db.meta("query_history").is_some_and(|x| x.contains("type = Epic")), "Verlauf wird gespeichert");
        // Hilfe öffnen, Esc schließt sie
        h.ui.help.open_at(Some("filters"));
        h.frames(2);
        assert!(h.ui.help.open);
        h.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(!h.ui.help.open, "Esc schließt die Hilfe");
        // Schlüsselwort-Dialog
        h.ui.kw.open();
        h.frames(2);
        assert!(h.ui.kw.open);
        h.ui.kw.open = false;
        // Sprache gewechselt: das Schema wird neu gebaut
        let lang = if h.lang == Lang::En { Lang::De } else { Lang::En };
        h.lang = lang;
        h.frames(3);
        assert_eq!(h.ui.search.schema.key().map(|k| k.1), Some(lang));
    });
}

// ------------------------------------------------------------------ Schema

#[test]
fn the_schema_comes_from_the_search_layer_with_sprint_names_and_conflicts() {
    let h = H::new(Lang::En);
    crate::planning::projects::update_project(&h.db.conn, h.project, &crate::planning::projects::UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
    crate::planning::sprints::sprint::create_sprint(&h.db.conn, &crate::planning::sprints::sprint::NewSprint::new(h.project, "Sprint Alpha"), &crate::calc::calendar::WorkCalendar::default()).unwrap();
    let mut c = SchemaCache::default();
    c.ensure(&h.db.conn, Scope::Project(h.project), Lang::En);
    let l = c.loaded().unwrap();
    let f = l.schema.field("sprint").expect("Sprints stehen im Schema");
    assert_eq!(f.ty, crate::query::FieldType::Option(vec!["Sprint Alpha".into()]), "die Vorschläge zeigen die Sprint-Namen");
    // Vorschläge für den Wert kommen aus dem Schema
    let s = crate::query::suggest("sprint = ", 9, &l.schema);
    assert!(s.iter().any(|x| x.label.contains("Sprint Alpha")), "{:?}", s.iter().map(|x| &x.label).collect::<Vec<_>>());
    // ein Feld, das wie ein eingebautes heißt, steht in den Konflikten
    let ty = h.type_id(h.project, "epic");
    let made = crate::planning::fields::create_field(&h.db.conn, &crate::planning::fields::NewField::new(ty, "Status", crate::planning::fields::FieldKind::Text)).is_ok();
    c.invalidate();
    c.ensure(&h.db.conn, Scope::Project(h.project), Lang::En);
    let l = c.loaded().unwrap();
    if made {
        assert!(l.conflicts.iter().any(|x| x.name.eq_ignore_ascii_case("status")), "{:?}", l.conflicts);
    }
    for lang in [Lang::En, Lang::De] {
        let d = super::search_tab::conflict_details(lang, &l.conflicts);
        assert_eq!(d.lines().count(), l.conflicts.len());
    }
}

#[test]
fn conflict_notes_name_every_field_in_both_languages() {
    use crate::planning::fields::FieldKind;
    use crate::planning::search::Conflict;
    let cs = vec![
        Conflict { name: "Estimate".into(), kept: Some(FieldKind::Number), dropped: vec![FieldKind::Text] },
        Conflict { name: "status".into(), kept: None, dropped: vec![FieldKind::Dropdown] },
    ];
    for lang in [Lang::En, Lang::De] {
        let d = super::search_tab::conflict_details(lang, &cs);
        assert!(d.contains("Estimate") && d.contains("status"));
        assert_eq!(d.lines().count(), 2);
        let ctx = egui::Context::default();
        let _ = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| super::search_tab::conflict_note(ui, lang, &cs));
        });
    }
    assert_ne!(super::search_tab::conflict_details(Lang::De, &cs), super::search_tab::conflict_details(Lang::En, &cs));
}

// ------------------------------------------------------------------ Tabelle

fn row(i: i64) -> TicketRow {
    TicketRow {
        id: 100 + i,
        project_id: 1,
        project_key: "QQ".into(),
        number: i,
        key: format!("QQ-{i}"),
        title: format!("Row {i}"),
        type_id: 1,
        type_name: "Epic".into(),
        type_color: "#58A6E6".into(),
        type_icon: None,
        status_id: Some(1),
        status_name: Some("To Do".into()),
        status_color: Some("#888888".into()),
        status_category: None,
        priority: Some("High".into()),
        priority_color: Some("#E06C75".into()),
        story_points: Some(3.0),
        parent_id: None,
        parent_key: None,
        flagged: i % 2 == 0,
        starred: i % 3 == 0,
        rank: i,
        updated_at: "2026-01-01 00:00:00".into(),
    }
}

fn table_frame(ctx: &egui::Context, events: Vec<Event>, width: f32, rows: &[TicketRow], locked: bool, project: bool, scroll: Option<f32>) -> TableOut {
    let mut out = TableOut::default();
    let sortable = [Col::Type, Col::Key, Col::Title, Col::Status, Col::Priority, Col::Points, Col::Project, Col::Marks];
    let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width + 40.0, 700.0))), events, focused: true, ..Default::default() };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let cfg = TableCfg { lang: Lang::En, show_project: project, sort: Some(SortBy { col: Col::Title, desc: false }), sort_locked: locked, sortable: &sortable, selected: Some(101), scroll_height: scroll, salt: "t", };
            draw_table(ui, &cfg, rows, &mut out);
        });
    });
    out
}

#[test]
fn the_table_draws_at_every_width_with_and_without_scroll_and_project() {
    let rows: Vec<TicketRow> = (1..=40).map(row).collect();
    for width in [280.0, 480.0, 700.0, 1000.0, 1500.0] {
        for project in [false, true] {
            for scroll in [None, Some(300.0)] {
                let ctx = egui::Context::default();
                for _ in 0..2 {
                    let out = table_frame(&ctx, vec![], width, &rows, false, project, scroll);
                    assert!(out.open.is_none() && out.mark.is_none() && out.sort_click.is_none());
                }
            }
        }
    }
    // leere Liste und eine Zeile ohne Status/Priorität/Punkte
    let ctx = egui::Context::default();
    table_frame(&ctx, vec![], 800.0, &[], false, true, None);
    let mut bare = row(1);
    bare.status_name = None;
    bare.priority = None;
    bare.story_points = None;
    table_frame(&ctx, vec![], 800.0, &[bare], true, true, None);
}

#[test]
fn clicking_a_row_opens_it_and_clicking_a_header_asks_for_a_sort() {
    let rows: Vec<TicketRow> = (1..=5).map(row).collect();
    let ctx = egui::Context::default();
    // erster Lauf: Lage bestimmen
    let mut first = None;
    let mut out = TableOut::default();
    let sortable = [Col::Title, Col::Key];
    let run = |events: Vec<Event>, out: &mut TableOut, first: &mut Option<Rect>, locked: bool| {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 700.0))), events, focused: true, ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let cfg = TableCfg { lang: Lang::En, show_project: false, sort: None, sort_locked: locked, sortable: &sortable, selected: None, scroll_height: None, salt: "c" };
                draw_table(ui, &cfg, &rows, out);
            });
        });
        if out.first_row.is_some() {
            *first = out.first_row;
        }
    };
    run(vec![], &mut out, &mut first, false);
    run(vec![], &mut out, &mut first, false);
    let r = first.expect("Zeile gezeichnet");
    // Klick in den Titel der dritten Zeile
    let p = Pos2::new(r.left() + 300.0, r.center().y + 2.0 * r.height());
    let mut opened = None;
    for ev in [vec![Event::PointerMoved(p)], vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]] {
        run(ev, &mut out, &mut first, false);
        if out.open.is_some() {
            opened = out.open;
        }
    }
    assert_eq!(opened, Some(103), "die dritte Zeile hat die Kennung 103");
    // Klick auf die Überschrift „Titel“ (über der ersten Zeile): Sortierwunsch
    let head = Pos2::new(r.left() + 200.0, r.top() - 14.0);
    let mut asked = None;
    for ev in [vec![Event::PointerMoved(head)], vec![Event::PointerButton { pos: head, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], vec![Event::PointerButton { pos: head, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]] {
        run(ev, &mut out, &mut first, false);
        if out.sort_click.is_some() {
            asked = out.sort_click;
        }
    }
    assert_eq!(asked, Some(Col::Title));
    // mit eigenem ORDER BY: kein Sortierwunsch
    let mut out2 = TableOut::default();
    let mut asked2 = None;
    for ev in [vec![Event::PointerMoved(head)], vec![Event::PointerButton { pos: head, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], vec![Event::PointerButton { pos: head, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]] {
        run(ev, &mut out2, &mut first, true);
        if out2.sort_click.is_some() {
            asked2 = out2.sort_click;
        }
    }
    assert!(asked2.is_none());
}

// ------------------------------------------------------------------ Ticket-Liste mit Abfrage

impl H {
    fn submit_tickets(&mut self, q: &str) {
        self.ui.tab = Tab::Tickets;
        self.ui.tq.bar.set_query(q);
        self.frames(2);
        self.frame(vec![enter(true)]);
        self.frame(vec![enter(false)]);
    }
}

#[test]
fn the_ticket_list_switches_to_the_query_and_back() {
    both(|h| {
        seed_basic(h);
        h.ui.invalidate();
        h.frames(3);
        assert_eq!(h.ui.tab, Tab::Tickets);
        assert!(!h.ui.tq_active());
        assert_eq!(h.ui.tickets.rows.len(), 5, "ohne Abfrage: lokale Liste");
        // lokale Filter wirken wie bisher
        h.submit_tickets("type = Epic");
        assert_eq!(h.ui.tq.active.as_deref(), Some("type = Epic"));
        h.wait("Liste mit Abfrage", |h| h.ui.tq.res.run.shown.is_some() && !h.ui.tq.res.run.loading());
        assert_eq!(h.ui.tq.res.run.total(), 3);
        // Standard: neueste zuerst
        let keys: Vec<String> = h.ui.tq.res.run.rows().iter().map(|r| r.key.clone()).collect();
        assert_eq!(keys, vec!["QQ-3", "QQ-2", "QQ-1"], "{:?}", h.lang);
        assert!(h.ui.tq.res.run.requested().unwrap().jql.ends_with("ORDER BY key DESC"));
        // Die Zeile lässt sich öffnen (Klick auf die erste Zeile)
        h.frames(2);
        let r = h.ui.tq.res.first_row.expect("Tabelle gezeichnet");
        let before = h.ui.tickets.open;
        h.click(Pos2::new(r.left() + 300.0, r.center().y));
        assert_ne!(h.ui.tickets.open, before, "ein Klick auf die Zeile öffnet das Ticket");
        assert!(h.ui.tickets.open.is_some());
        h.ui.tickets.go_back();
        h.frames(2);
        // Sortierung wie die lokale Liste: Titel aufsteigend
        h.ui.tq.res.sort = Some(SortBy { col: Col::Title, desc: false });
        h.wait("sortiert", |h| h.ui.tq.res.run.rows().first().map(|r| r.title.as_str()) == Some("Alpha epic") && !h.ui.tq.res.run.loading());
        // eine andere Abfrage ersetzt die erste
        h.submit_tickets("type = Defect OR type = \"Dev task\" OR title ~ task");
        h.wait("zweite Abfrage", |h| h.ui.tq.res.run.total() == 2 && !h.ui.tq.res.run.loading());
        // „Abfrage löschen“: zurück zur lokalen Liste
        h.ui.tq.active = None;
        h.ui.tq.res.run.clear();
        h.ui.tickets.invalidate();
        h.frames(3);
        assert!(!h.ui.tq_active());
        assert_eq!(h.ui.tickets.rows.len(), 5);
    });
}

#[test]
fn an_empty_submit_in_query_mode_clears_the_query() {
    let mut h = H::new(Lang::En);
    seed_basic(&h);
    h.submit_tickets("type = Epic");
    assert!(h.ui.tq_active());
    h.wait("läuft", |h| h.ui.tq.res.run.shown.is_some() && !h.ui.tq.res.run.loading());
    h.ui.tq.bar.set_query("");
    h.frames(2);
    h.frame(vec![enter(true)]);
    h.frame(vec![enter(false)]);
    h.frames(3);
    assert!(!h.ui.tq_active(), "eine leere Abfrage schaltet zur lokalen Liste zurück");
    assert_eq!(h.ui.tickets.rows.len(), 5);
}

#[test]
fn a_new_ticket_appears_in_the_query_list_and_the_project_switch_clears_it() {
    let mut h = H::new(Lang::En);
    seed_basic(&h);
    h.submit_tickets("type = Epic");
    h.wait("läuft", |h| h.ui.tq.res.run.total() == 3 && !h.ui.tq.res.run.loading());
    h.mk(h.project, "epic", "Delta epic");
    h.ui.invalidate();
    h.wait("neu geladen", |h| h.ui.tq.res.run.total() == 4 && !h.ui.tq.res.run.loading());
    // anderes Projekt: die Abfrage gilt nicht mehr
    let other = h.other;
    {
        let cx = Cx { conn: &h.db.conn, db: &h.db, lang: h.lang };
        h.ui.select(&cx, other);
    }
    h.frames(4);
    assert!(!h.ui.tq_active(), "ein Projektwechsel löscht die Abfrage");
    assert!(h.ui.tq.bar.text.is_empty());
}

#[test]
fn query_mode_keeps_local_filters_out_of_the_way_and_tickets_open_in_panel_mode() {
    let mut h = H::new(Lang::De);
    seed_basic(&h);
    h.ui.tickets.filters.text = "Alpha".into();
    h.ui.tickets.list_dirty = true;
    h.frames(3);
    assert_eq!(h.ui.tickets.rows.len(), 1);
    h.submit_tickets("type = Epic");
    h.wait("Abfrage", |h| h.ui.tq.res.run.total() == 3 && !h.ui.tq.res.run.loading());
    assert_eq!(h.ui.tq.res.run.rows().len(), 3, "der lokale Textfilter gilt nicht mehr");
    h.ui.tickets.mode = super::ticket_state::ViewMode::Panel;
    let id = h.ui.tq.res.run.rows()[0].id;
    h.ui.tickets.open_ticket(id, false);
    h.frames(4);
    assert_eq!(h.ui.tickets.open, Some(id));
}
