//! Tests der Anzeige von Version, Meilenstein und Terminal in der Ticket-Liste und in den Verknüpfungen, und des Scrollens der
//! Liste zum geöffneten Ticket – ohne Fenster, ohne Maus und Tastatur des Nutzers (egui läuft mit künstlichen Eingaben, die Daten
//! liegen in einer Testdatenbank).

use super::ticket_state::ViewMode;
use super::ticket_view::Act;
use super::{Cx, PlanUi, Tab};
use crate::app::tasks_ui::Env;
use crate::app::terminal_links::{self, LiveSession};
use crate::db::Db;
use crate::planning::fields::{create_field, FieldKind, NewField};
use crate::planning::history::Source;
use crate::planning::link_types::{list_link_types, TargetKind};
use crate::planning::links::Target;
use crate::planning::projects::{create_project, update_project, NewProject, UpdateProject};
use crate::planning::sprints::releases::{create_milestone, create_version, NewRelease};
use crate::planning::tickets::{create_ticket, NewTicket};
use crate::planning::types::list_types;
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Pos2, RawInput, Rect, Vec2};
use serde_json::json;

struct H {
    ctx: egui::Context,
    db: Db,
    ui: PlanUi,
    lang: Lang,
    time: f64,
    project: i64,
}

impl H {
    fn new(lang: Lang) -> H {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("paneldeck-test-refui-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::open(&dir, &dir.join("backups")).unwrap();
        let mut np = NewProject::new("PAN", "Paneldeck", "#58A6E6");
        np.with_defaults = true;
        let project = create_project(&db.conn, &np).unwrap();
        update_project(&db.conn, project, &UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
        db.set_meta("expert_mode", "1");
        db.set_meta("expert_project", &project.to_string());
        let mut h = H { ctx: egui::Context::default(), db, ui: PlanUi::default(), lang, time: 0.0, project };
        h.frames(3);
        h
    }

    fn frame(&mut self) {
        self.time += 0.1;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0))), time: Some(self.time), events: Vec::<Event>::new(), ..Default::default() };
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
            self.frame();
        }
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut PlanUi, &Cx) -> R) -> R {
        let cx = Cx { conn: &self.db.conn, db: &self.db, lang: self.lang };
        f(&mut self.ui, &cx)
    }

    fn type_id(&self, key: &str) -> i64 {
        list_types(&self.db.conn, self.project).unwrap().into_iter().find(|t| t.key.as_deref() == Some(key)).unwrap().id
    }

    fn mk(&self, title: &str) -> i64 {
        create_ticket(&self.db.conn, &NewTicket::new(self.project, self.type_id("devtask"), title), &Source::ui()).unwrap().0.id
    }

    /// Position der Zeile in der Liste.
    fn index_of(&self, id: i64) -> usize {
        self.ui.tickets.rows.iter().position(|r| r.ticket.id == id).unwrap()
    }
}

fn both(f: impl Fn(&mut H)) {
    for lang in [Lang::En, Lang::De] {
        let mut h = H::new(lang);
        f(&mut h);
    }
}

#[test]
fn the_list_loads_and_draws_release_and_terminal_chips() {
    let _live = terminal_links::live_test_guard();
    both(|h| {
        terminal_links::set_live_for_test(vec![LiveSession { id: 7, name: "Build".into(), folder: String::new(), running: true }]);
        let task = h.type_id("devtask");
        let vf = create_field(&h.db.conn, &NewField::new(task, "Fix version", FieldKind::Version)).unwrap();
        let mf = create_field(&h.db.conn, &NewField::new(task, "Target milestone", FieldKind::Milestone)).unwrap();
        let tf = create_field(&h.db.conn, &NewField::new(task, "Session", FieldKind::Terminal)).unwrap();
        let v = create_version(&h.db.conn, &NewRelease::new(h.project, "1.0")).unwrap();
        let m = create_milestone(&h.db.conn, &NewRelease::new(h.project, "Beta")).unwrap();
        let with = create_ticket(&h.db.conn, &NewTicket { values: vec![(vf, json!(v)), (mf, json!(m)), (tf, json!("session:7"))], ..NewTicket::new(h.project, task, "Mit allem") }, &Source::ui()).unwrap().0.id;
        let ended = create_ticket(&h.db.conn, &NewTicket { values: vec![(tf, json!("session:9"))], ..NewTicket::new(h.project, task, "Sitzung weg") }, &Source::ui()).unwrap().0.id;
        let plain = h.mk("Ohne Werte");
        h.ui.dirty = true;
        h.ui.tickets.invalidate();
        h.frames(4);
        assert_eq!(h.ui.tab, Tab::Tickets);
        assert_eq!(h.ui.tickets.rows.len(), 3);
        let refs = &h.ui.tickets.row_refs;
        assert_eq!(refs.len(), 2, "nur Tickets mit Werten");
        assert_eq!(refs[&with].version.as_deref(), Some("1.0"));
        assert_eq!(refs[&with].milestone.as_deref(), Some("Beta"));
        assert_eq!(refs[&with].terminal.as_deref(), Some("session:7"));
        assert!(refs[&ended].version.is_none() && refs[&ended].terminal.is_some());
        assert!(!refs.contains_key(&plain));
        // die Chips: laufend normal, beendet gedämpft und mit Hinweis
        let c = super::ref_display::row_chips(&refs[&ended], h.lang);
        assert!(c.len() == 1 && c[0].muted, "{c:?}");
        let c = super::ref_display::row_chips(&refs[&with], h.lang);
        assert!(c.len() == 3 && c.iter().all(|x| !x.muted), "{c:?}");
        // umbenannte Version erscheint nach dem Neuladen der Liste
        crate::planning::sprints::releases::update_version(&h.db.conn, v, &crate::planning::sprints::releases::UpdateRelease { name: Some("2.0".into()), ..Default::default() }).unwrap();
        super::ref_inputs::clear_release_cache();
        h.ui.tickets.invalidate();
        h.frames(3);
        assert_eq!(h.ui.tickets.row_refs[&with].version.as_deref(), Some("2.0"));
        // Sitzung beendet: der Chip wird beim Zeichnen grau, ohne dass neu geladen wird
        terminal_links::set_live_for_test(Vec::new());
        h.frames(2);
        assert!(super::ref_display::row_chips(&h.ui.tickets.row_refs[&with], h.lang)[2].muted);
        // im Panel (schmalere Liste) zeichnet alles weiter
        h.ui.tickets.mode = ViewMode::Panel;
        h.ui.tickets.open_ticket(with, false);
        h.frames(3);
    });
}

#[test]
fn opening_a_ticket_from_another_tab_scrolls_the_list_to_it_and_a_visible_row_stays_put() {
    both(|h| {
        let ids: Vec<i64> = (0..90).map(|i| h.mk(&format!("Ticket {i:02}"))).collect();
        h.ui.tickets.mode = ViewMode::Panel;
        h.ui.tickets.invalidate();
        h.frames(4);
        assert_eq!(h.ui.tickets.rows.len(), 90);
        assert_eq!(h.ui.tickets.list_offset, 0.0);
        // aus dem Board ein Ticket weit unten öffnen: die Liste rollt hin
        let far = ids[ids.len() / 2];
        h.ui.tab = Tab::Boards;
        h.frames(2);
        assert!(h.with(|ui, cx| ui.open_ticket(cx, far)));
        h.frames(5);
        assert_eq!(h.ui.tab, Tab::Tickets);
        assert_eq!(h.ui.tickets.open, Some(far));
        let top = h.index_of(far) as f32 * 30.0;
        let off = h.ui.tickets.list_offset;
        assert!(off > 0.0, "die Liste ist gerollt");
        assert!(top >= off && top + 30.0 <= off + 900.0, "die Zeile liegt im Ausschnitt: Zeile {top}, Position {off}");
        assert!(h.ui.tickets.scroll_to.is_none() && h.ui.tickets.scroll_offset.is_none(), "erledigt");
        // ein anderes Ticket, das ohnehin sichtbar ist: keine Bewegung
        let near = h.ui.tickets.rows[(off / 30.0) as usize + 2].ticket.id;
        h.ui.tab = Tab::Boards;
        h.with(|ui, cx| ui.open_ticket(cx, near));
        h.frames(4);
        assert_eq!(h.ui.tickets.list_offset, off, "sichtbare Zeile: die Position bleibt");
        // Auswahl in der Liste (wie ein Klick): scroll_to wird nicht gemerkt
        h.ui.tickets.open_ticket(near, false);
        h.ui.tickets.scroll_to = None;
        h.frames(3);
        assert_eq!(h.ui.tickets.list_offset, off);
        // ganz nach oben
        let first = h.ui.tickets.rows[0].ticket.id;
        h.ui.tab = Tab::Boards;
        h.with(|ui, cx| ui.open_ticket(cx, first));
        h.frames(5);
        assert_eq!(h.ui.tickets.list_offset, 0.0, "auch nach oben");
    });
}

#[test]
fn leaving_the_page_view_shows_the_list_at_the_ticket_just_closed() {
    both(|h| {
        let ids: Vec<i64> = (0..90).map(|i| h.mk(&format!("Ticket {i:02}"))).collect();
        h.ui.tickets.invalidate();
        h.frames(4);
        let far = h.ui.tickets.rows[h.ui.tickets.rows.len() - 5].ticket.id;
        // Seitenansicht (Standard): die Liste ist verdeckt; zurück zur Liste rollt zum verlassenen Ticket
        h.ui.tickets.mode = ViewMode::Page;
        assert!(h.with(|ui, cx| ui.open_ticket(cx, far)));
        h.frames(4);
        assert_eq!(h.ui.tickets.open, Some(far));
        h.ui.ticket_back();
        h.frames(5);
        assert!(h.ui.tickets.open.is_none());
        let top = h.index_of(far) as f32 * 30.0;
        let off = h.ui.tickets.list_offset;
        assert!(off > 0.0 && top >= off && top + 30.0 <= off + 900.0, "Zeile {top}, Position {off}");
        // ein Ticket, das nicht (mehr) in der Liste steht, hinterlässt nichts
        h.ui.tickets.filters.text = "Ticket 01".into();
        h.ui.tickets.list_dirty = true;
        h.frames(3);
        h.ui.tickets.scroll_to = Some(ids[80]);
        assert!(h.ui.tickets.rows.iter().all(|r| r.ticket.id != ids[80]), "gefiltert");
        h.frames(3);
        assert!(h.ui.tickets.scroll_to.is_none());
    });
}

#[test]
fn terminal_links_in_the_ticket_view_jump_to_running_sessions_and_show_ended_ones_greyed() {
    let _live = terminal_links::live_test_guard();
    both(|h| {
        terminal_links::set_live_for_test(vec![
            LiveSession { id: 7, name: "Build".into(), folder: String::new(), running: true },
            LiveSession { id: 8, name: "Old".into(), folder: String::new(), running: false },
        ]);
        let t = h.mk("Mit Terminal-Verknüpfung");
        let lt = list_link_types(&h.db.conn).unwrap().into_iter().find(|l| l.target_kind == TargetKind::Terminal).unwrap().id;
        for target in ["session:7", "session:8", "session:99", "frei"] {
            crate::planning::links::add_link(&h.db.conn, t, lt, Target::Terminal(target.into()), &Source::ui()).unwrap();
        }
        h.with(|ui, cx| ui.open_ticket(cx, t));
        h.frames(4);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().links.len(), 4);
        let (txt, run, tip) = super::ref_display::terminal_link_view("session:7", h.lang);
        assert_eq!((txt.as_str(), run), ("Build", Some(7)));
        assert_eq!(tip, h.lang.t("Zur Sitzung springen", "Jump to the session"));
        let ended = h.lang.t("beendet", "ended");
        let (txt, run, tip) = super::ref_display::terminal_link_view("session:8", h.lang);
        assert_eq!((txt, run), (format!("Old ({ended})"), None));
        assert_eq!(tip, h.lang.t("Die Sitzung ist beendet.", "The session has ended."));
        assert_eq!(super::ref_display::terminal_link_view("session:99", h.lang).0, format!("Terminal 99 ({ended})"));
        let (txt, run, tip) = super::ref_display::terminal_link_view("frei", h.lang);
        assert_eq!((txt.as_str(), run), ("frei", None));
        assert_eq!(tip, h.lang.t("Kein Verweis auf eine Sitzung dieser Anwendung.", "Not a reference to a session of this application."));
        // der Klick auf den laufenden Eintrag merkt den Sprung (einmalig)
        assert_eq!(terminal_links::take_jump(), None);
        h.with(|ui, cx| ui.ticket_act(cx, Act::OpenSession(7)));
        assert_eq!(terminal_links::take_jump(), Some(7));
        assert_eq!(terminal_links::take_jump(), None);
        terminal_links::set_live_for_test(Vec::new());
        h.frames(2);
    });
}

#[test]
fn ticket_links_open_the_other_ticket_through_the_central_navigation_and_back_returns() {
    both(|h| {
        let a = h.mk("A");
        let b = h.mk("B");
        let types = list_link_types(&h.db.conn).unwrap();
        let rel = types.iter().find(|l| l.target_kind == TargetKind::Ticket).unwrap().id;
        crate::planning::links::add_link(&h.db.conn, a, rel, Target::Ticket(b), &Source::ui()).unwrap();
        h.ui.tab = Tab::Boards;
        h.with(|ui, cx| ui.open_ticket(cx, a));
        h.frames(3);
        let target = match &h.ui.tickets.detail.as_ref().unwrap().links[0].target {
            crate::planning::links::LinkTarget::Ticket(r) => r.id,
            _ => 0,
        };
        assert_eq!(target, b);
        h.with(|ui, cx| ui.ticket_act(cx, Act::OpenId(target)));
        h.frames(3);
        assert_eq!((h.ui.tickets.open, h.ui.tickets.back.clone()), (Some(b), vec![a]), "Verweiskette");
        assert_eq!(h.ui.tickets.return_tab, Some(Tab::Boards), "der Ursprung bleibt");
        h.ui.ticket_back();
        h.frames(2);
        assert_eq!(h.ui.tickets.open, Some(a));
        h.ui.ticket_back();
        h.frames(2);
        assert_eq!(h.ui.tab, Tab::Boards);
    });
}
