//! Tests der zentralen Navigation „Ticket öffnen“ (`ticket_nav.rs`) ohne Fenster und ohne Maus/Tastatur des Nutzers: egui läuft mit
//! künstlichen Eingaben, die Daten liegen in einer Testdatenbank. Geprüft werden Öffnen aus Board/Backlog/Dashboard/Übersicht
//! (Projektwechsel, Seite und Panel), der Rückweg („Zurück“, Esc, Schließen), Verweisketten, Entwurf, Hilfe, Terminal-Hub-Anfrage,
//! Dashboard → Suche und die Taste „.“.

use super::ticket_state::ViewMode;
use super::ticket_view::Act;
use super::{Cx, PlanUi, Tab};
use crate::app::tasks_ui::Env;
use crate::db::Db;
use crate::planning::history::Source;
use crate::planning::projects::{create_project, NewProject};
use crate::planning::tickets::{create_ticket, NewTicket};
use crate::planning::types::list_types;
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

struct H {
    ctx: egui::Context,
    db: Db,
    ui: PlanUi,
    lang: Lang,
    time: f64,
    p1: i64,
    p2: i64,
}

impl H {
    fn new(lang: Lang) -> H {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("paneldeck-test-ticketnav-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::open(&dir, &dir.join("backups")).unwrap();
        let mut np = NewProject::new("PAN", "Paneldeck", "#58A6E6");
        np.with_defaults = true;
        let p1 = create_project(&db.conn, &np).unwrap();
        let mut np = NewProject::new("BBB", "Zweites", "#7FBF6A");
        np.with_defaults = true;
        let p2 = create_project(&db.conn, &np).unwrap();
        // Backlog-Reiter nur mit Sprints
        crate::planning::projects::update_project(&db.conn, p1, &crate::planning::projects::UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
        db.set_meta("expert_mode", "1");
        db.set_meta("expert_project", &p1.to_string());
        let mut h = H { ctx: egui::Context::default(), db, ui: PlanUi::default(), lang, time: 0.0, p1, p2 };
        h.frames(3);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.1;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0))), time: Some(self.time), events, ..Default::default() };
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

    fn key(&mut self, key: Key) {
        let ev = Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: Modifiers::NONE };
        self.frame(vec![ev]);
        self.frames(2);
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut PlanUi, &Cx) -> R) -> R {
        let cx = Cx { conn: &self.db.conn, db: &self.db, lang: self.lang };
        f(&mut self.ui, &cx)
    }

    fn mk(&self, project: i64, title: &str) -> i64 {
        let ty = list_types(&self.db.conn, project).unwrap().into_iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
        create_ticket(&self.db.conn, &NewTicket::new(project, ty, title), &Source::ui()).unwrap().0.id
    }

    /// Von `tab` aus das Ticket öffnen (wie ein Klick auf Karte, Zeile oder Kachel) und zwei Bilder zeichnen.
    fn open_from(&mut self, tab: Tab, id: i64) {
        self.ui.tab = tab;
        let ok = self.with(|ui, cx| ui.open_ticket(cx, id));
        assert!(ok);
        self.frames(3);
    }
}

fn both(f: impl Fn(&mut H)) {
    for lang in [Lang::En, Lang::De] {
        let mut h = H::new(lang);
        f(&mut h);
    }
}

#[test]
fn opening_from_board_backlog_dashboard_and_overview_shows_the_ticket_and_returns_to_the_origin() {
    both(|h| {
        let t = h.mk(h.p1, "Karte");
        for origin in [Tab::Boards, Tab::Backlog, Tab::Dashboards, Tab::Overview] {
            h.ui.tab = Tab::Tickets;
            h.ui.tickets.close();
            h.frames(2);
            h.open_from(origin, t);
            assert_eq!(h.ui.tab, Tab::Tickets, "{origin:?}");
            assert_eq!(h.ui.tickets.open, Some(t));
            assert_eq!(h.ui.tickets.return_tab, Some(origin));
            assert_eq!(h.ui.tickets.detail.as_ref().map(|d| d.ticket.id), Some(t), "Detailansicht geladen");
            assert_eq!(h.ui.back_target_label(h.lang).is_some(), true);
            // Esc: zurück zum Ursprung
            h.key(Key::Escape);
            assert_eq!(h.ui.tab, origin);
            assert!(h.ui.tickets.open.is_none());
            assert!(h.ui.tickets.return_tab.is_none());
        }
    });
}

#[test]
fn the_back_arrow_returns_to_the_origin_and_close_works_in_panel_mode() {
    both(|h| {
        let t = h.mk(h.p1, "A");
        h.open_from(Tab::Boards, t);
        h.with(|ui, cx| ui.ticket_act(cx, Act::Back));
        h.frames(2);
        assert_eq!((h.ui.tab, h.ui.tickets.open), (Tab::Boards, None));
        // Panel: Kreuz schließt und führt zurück
        h.ui.tickets.mode = ViewMode::Panel;
        h.open_from(Tab::Dashboards, t);
        h.frames(2);
        assert_eq!(h.ui.tab, Tab::Tickets);
        h.with(|ui, cx| ui.ticket_act(cx, Act::Close));
        h.frames(2);
        assert_eq!((h.ui.tab, h.ui.tickets.open), (Tab::Dashboards, None));
    });
}

#[test]
fn another_project_is_selected_and_the_way_back_stays_in_that_project() {
    both(|h| {
        let t1 = h.mk(h.p1, "Eins");
        let t2 = h.mk(h.p2, "Zwei");
        assert_eq!(h.ui.selected, h.p1);
        h.open_from(Tab::Dashboards, t2);
        assert_eq!(h.ui.selected, h.p2, "Projektwechsel auf das des Tickets");
        assert_eq!(h.ui.tickets.open, Some(t2));
        assert_eq!(h.ui.tickets.detail.as_ref().map(|d| (d.ticket.id, d.ticket.project_id)), Some((t2, h.p2)));
        assert_eq!(h.db.meta("expert_project"), Some(h.p2.to_string()));
        h.key(Key::Escape);
        assert_eq!((h.ui.tab, h.ui.selected), (Tab::Dashboards, h.p2));
        // und wieder zurück ins erste Projekt (aus dem Backlog; auch wenn der Reiter dort fehlt, bleibt die Ansicht gültig)
        h.open_from(Tab::Boards, t1);
        assert_eq!((h.ui.selected, h.ui.tickets.open), (h.p1, Some(t1)));
        assert_eq!(h.ui.tickets.return_tab, Some(Tab::Boards));
    });
}

#[test]
fn links_keep_the_origin_and_back_walks_the_chain_first() {
    both(|h| {
        let (a, b) = (h.mk(h.p1, "A"), h.mk(h.p1, "B"));
        h.open_from(Tab::Boards, a);
        // Verweis in der Ansicht: B öffnen
        h.with(|ui, cx| ui.ticket_act(cx, Act::OpenId(b)));
        h.frames(3);
        assert_eq!(h.ui.tickets.open, Some(b));
        assert_eq!(h.ui.tickets.back, vec![a]);
        assert_eq!(h.ui.tickets.return_tab, Some(Tab::Boards), "der Ursprung bleibt erhalten");
        assert!(h.ui.back_target_label(h.lang).is_none(), "mit Verweiskette führt der Pfeil zum vorherigen Ticket");
        h.key(Key::Escape);
        assert_eq!((h.ui.tab, h.ui.tickets.open), (Tab::Tickets, Some(a)));
        h.key(Key::Escape);
        assert_eq!((h.ui.tab, h.ui.tickets.open), (Tab::Boards, None));
    });
}

#[test]
fn opening_from_the_list_returns_to_the_list_and_a_manual_tab_change_forgets_the_way() {
    both(|h| {
        let t = h.mk(h.p1, "L");
        // aus der Liste (Reiter „Tickets“, nichts offen)
        h.open_from(Tab::Tickets, t);
        assert_eq!(h.ui.tickets.return_tab, None);
        h.key(Key::Escape);
        assert_eq!((h.ui.tab, h.ui.tickets.open), (Tab::Tickets, None));
        // Panel: vom Board geöffnet, dann von Hand ein Reiter gewählt → Rückweg vergessen
        h.ui.tickets.mode = ViewMode::Panel;
        h.open_from(Tab::Boards, t);
        assert_eq!(h.ui.tickets.return_tab, Some(Tab::Boards));
        h.ui.tickets.return_tab = None; // so setzt es der Klick auf einen Reiter in `body`
        h.key(Key::Escape);
        assert_eq!((h.ui.tab, h.ui.tickets.open), (Tab::Tickets, None), "ohne Rückweg zur Liste");
    });
}

#[test]
fn the_new_ticket_draft_the_help_and_the_selection_survive_navigation() {
    both(|h| {
        let (a, b) = (h.mk(h.p1, "A"), h.mk(h.p1, "B"));
        h.open_from(Tab::Tickets, a);
        h.with(|ui, cx| ui.open_create(cx, None));
        h.frames(2);
        assert!(h.ui.tickets.create.is_some());
        h.ui.help.open = true;
        h.open_from(Tab::Boards, b);
        assert!(!h.ui.help.open, "die Hilfe schließt");
        assert!(h.ui.tickets.create.is_some(), "der Entwurf „Neues Ticket“ bleibt");
        assert_eq!(h.ui.tickets.open, Some(b));
        // die Liste hebt das geöffnete Ticket hervor (Zustand `open`), und nach dem Schließen ist sie wieder da
        h.ui.tickets.create = None;
        h.key(Key::Escape);
        h.ui.tab = Tab::Tickets;
        h.frames(3);
        assert_eq!(h.ui.tickets.rows.len(), 2);
    });
}

#[test]
fn an_unknown_ticket_reports_an_error_and_changes_nothing() {
    both(|h| {
        h.ui.tab = Tab::Boards;
        let ok = h.with(|ui, cx| ui.open_ticket(cx, 99_999));
        assert!(!ok);
        assert_eq!(h.ui.tab, Tab::Boards);
        assert!(h.ui.tickets.open.is_none());
        assert!(h.ui.msg.as_ref().is_some_and(|m| !m.ok));
    });
}

#[test]
fn the_terminal_hub_request_switches_expert_mode_on_and_opens_the_ticket() {
    both(|h| {
        let t = h.mk(h.p2, "Aus dem Terminal");
        h.with(|ui, cx| ui.set_expert(cx.db, false));
        h.frames(2);
        assert!(!h.ui.expert);
        h.ui.request_open_ticket(t);
        h.frames(4);
        assert!(h.ui.expert, "Expertenmodus eingeschaltet");
        assert_eq!((h.ui.selected, h.ui.tab, h.ui.tickets.open), (h.p2, Tab::Tickets, Some(t)));
        assert!(h.ui.tickets.requested_open.is_none());
        assert_eq!(h.ui.tickets.detail.as_ref().map(|d| d.ticket.id), Some(t));
        // Panel-Modus: gleiches Ergebnis
        h.ui.tickets.close();
        h.ui.tickets.mode = ViewMode::Panel;
        h.ui.request_open_ticket(t);
        h.frames(3);
        assert_eq!(h.ui.tickets.open, Some(t));
    });
}

#[test]
fn a_dashboard_filter_opens_the_search_tab_with_the_query_in_the_field() {
    both(|h| {
        h.ui.tab = Tab::Dashboards;
        let p2 = h.p2;
        h.with(|ui, cx| ui.open_search_in_project(cx, "status = Done", Some(p2)));
        h.frames(2);
        assert_eq!((h.ui.tab, h.ui.selected), (Tab::Search, h.p2));
        assert_eq!(h.ui.search.bar.text, "status = Done");
        // ohne Projekt (alle Projekte): im aktuellen Projekt bleiben
        h.ui.tab = Tab::Dashboards;
        h.with(|ui, cx| ui.open_search_in_project(cx, "type = Bug", None));
        h.frames(2);
        assert_eq!((h.ui.tab, h.ui.selected), (Tab::Search, h.p2));
        assert_eq!(h.ui.search.bar.text, "type = Bug");
    });
}

#[test]
fn the_period_key_opens_the_quick_menu_for_the_open_ticket_only_without_text_focus() {
    both(|h| {
        let t = h.mk(h.p1, "Q");
        h.open_from(Tab::Tickets, t);
        assert!(!h.ui.quick.is_open());
        h.frame(vec![Event::Key { key: Key::Period, physical_key: Some(Key::Period), pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(h.ui.quick.is_open(), "Taste „.“ im Ticket öffnet das Schnellmenü");
        h.frames(3);
        assert!(h.ui.quick.is_open(), "das Menü bleibt offen");
        // solange das Menü offen ist, beendet Esc nicht die Ticket-Ansicht
        assert_eq!(h.ui.tickets.open, Some(t));
    });
}

#[test]
fn deleting_the_open_ticket_returns_to_the_origin() {
    both(|h| {
        let t = h.mk(h.p1, "Weg");
        h.open_from(Tab::Boards, t);
        h.with(|ui, cx| ui.ticket_act(cx, Act::Delete));
        h.frames(2);
        // Datenschicht-seitiges Löschen (wie nach der Bestätigung): das Ticket gibt es nicht mehr → Ansicht verlässt sich selbst
        crate::planning::tickets::delete_ticket(&h.db.conn, t, &Source::ui()).unwrap();
        h.ui.tickets.confirm = None;
        h.ui.tickets.delete = None;
        h.ui.tickets.invalidate();
        h.frames(4);
        assert_eq!((h.ui.tab, h.ui.tickets.open), (Tab::Boards, None));
    });
}
