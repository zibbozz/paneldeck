//! Tests der Fälligkeit in der Oberfläche ohne Fenster: Feldliste mit festen eingebauten Zeilen, Reiter „Fälligkeit“ (Verwalter
//! der Schwellen je Projekt), Liste und Board mit Markierung und Tönung, Zählung im Kopf – in beiden Sprachen.

use super::board_logic::{load_board, Quick};
use super::fields_ed::row_controls;
use super::{PlanUi, Tab};
use crate::app::tasks_ui::Env;
use crate::db::Db;
use crate::due::{self, DueMarker, DueThreshold, MarkerFrame};
use crate::planning::builtin_fields::{field_id, DUE_KEY};
use crate::planning::fields::{delete_field, get_field, list_fields, update_field, UpdateField};
use crate::planning::history::Source;
use crate::planning::projects::{create_project, due_thresholds, set_due_thresholds, NewProject};
use crate::planning::summary::project_summary;
use crate::planning::tickets::{create_ticket, NewTicket};
use crate::planning::types::list_types;
use crate::planning::views::boards::{ensure_default_board, list_boards};
use crate::planning::PlanError;
use crate::state::Lang;
use eframe::egui::{self, Color32, Pos2, RawInput, Rect, Vec2};
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
        let dir = std::env::temp_dir().join(format!("paneldeck-test-dueui-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::open(&dir, &dir.join("backups")).unwrap();
        let mut np = NewProject::new("PAN", "Paneldeck", "#58A6E6");
        np.with_defaults = true;
        let project = create_project(&db.conn, &np).unwrap();
        db.set_meta("expert_mode", "1");
        db.set_meta("expert_project", &project.to_string());
        let mut h = H { ctx: egui::Context::default(), db, ui: PlanUi::default(), lang, time: 0.0, project };
        h.frames(3);
        h
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0))), time: Some(self.time), ..Default::default() };
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
    }

    fn type_id(&self, key: &str) -> i64 {
        list_types(&self.db.conn, self.project).unwrap().into_iter().find(|t| t.key.as_deref() == Some(key)).unwrap().id
    }

    /// Ticket mit Fälligkeit `due` (relativ: Tage ab heute) anlegen.
    fn mk_due(&self, title: &str, days: i64) -> i64 {
        let ty = self.type_id("devtask");
        let due = (chrono::Local::now().date_naive() + chrono::Duration::days(days)).format("%Y-%m-%d").to_string();
        let mut n = NewTicket::new(self.project, ty, title);
        n.values.push((field_id(&self.db.conn, ty, DUE_KEY).unwrap().unwrap(), json!(due)));
        create_ticket(&self.db.conn, &n, &Source::ui()).unwrap().0.id
    }
}

fn both(f: impl Fn(&mut H)) {
    for lang in [Lang::En, Lang::De] {
        let mut h = H::new(lang);
        f(&mut h);
    }
}

#[test]
fn the_field_editor_shows_built_ins_as_fixed_rows_that_cannot_be_deleted() {
    both(|h| {
        h.ui.tab = Tab::Fields;
        h.frames(3);
        for ty in list_types(&h.db.conn, h.project).unwrap() {
            let fields = list_fields(&h.db.conn, ty.id).unwrap();
            assert!(fields[0].is_description() && fields[1].is_due(), "{:?}: eingebaute Felder oben", ty.key);
            for f in &fields {
                let rc = row_controls(f);
                assert_eq!((rc.movable, rc.editable, rc.deletable), (!f.is_fixed(), !f.is_fixed(), !f.is_fixed()), "{:?}", f.key);
            }
            // auch die Datenschicht schützt sie
            assert_eq!(delete_field(&h.db.conn, fields[0].id, None), Err(PlanError::SystemProtected));
            assert_eq!(delete_field(&h.db.conn, fields[1].id, Some(crate::planning::integrity::DeleteMode::Discard)), Err(PlanError::SystemProtected));
            assert_eq!(update_field(&h.db.conn, fields[1].id, &UpdateField { required: Some(true), ..Default::default() }), Err(PlanError::SystemProtected));
            update_field(&h.db.conn, fields[1].id, &UpdateField { show_on_card: Some(true), section: Some("Plan".into()), ..Default::default() }).unwrap();
            let f = get_field(&h.db.conn, fields[1].id).unwrap();
            assert!(f.show_on_card && f.section == "Plan");
        }
    });
}

#[test]
fn the_due_tab_manages_the_project_thresholds() {
    both(|h| {
        h.ui.tab = Tab::DueMarkers;
        h.frames(3);
        assert_eq!(h.ui.due_mgr.loaded_for, Some(h.project));
        assert_eq!(h.ui.due_mgr.list, due::default_thresholds(), "Projekt mit Vorlagen: Standard-Schwellen");
        // eine dritte Schwelle mit eigener Markierung und Tönung, gespeichert über die Datenschicht wie im Reiter
        let mut list = h.ui.due_mgr.list.clone();
        list.push(DueThreshold { under_minutes: 60, marker: DueMarker { frame: MarkerFrame::Octagon, symbol: Some("clock".into()), color: "#58A6E6".into(), name: "Gleich".into() }, tint: true });
        set_due_thresholds(&h.db.conn, h.project, &list).unwrap();
        h.ui.due_mgr.loaded_for = None;
        h.frames(2);
        assert_eq!(h.ui.due_mgr.list.len(), 3);
        assert!(h.ui.due_mgr.list[2].tint && h.ui.due_mgr.list[2].marker.frame == MarkerFrame::Octagon);
        // Projekt ohne Vorlagen: keine Schwellen
        let other = create_project(&h.db.conn, &NewProject::new("OWN", "Eigen", "#445566")).unwrap();
        assert!(due_thresholds(&h.db.conn, other).unwrap().is_empty());
    });
}

#[test]
fn list_board_and_header_show_markers_tint_and_counts() {
    both(|h| {
        let overdue = h.mk_due("Überfällig", -2);
        let soon = h.mk_due("Bald", 3);
        let later = h.mk_due("Später", 30);
        // Tönung für „bald“ einschalten
        let mut th = due::default_thresholds();
        th[0].tint = true;
        set_due_thresholds(&h.db.conn, h.project, &th).unwrap();
        h.ui.invalidate();
        // Liste
        h.ui.tab = Tab::Tickets;
        h.ui.tickets.invalidate();
        h.frames(3);
        assert!(h.ui.tickets.rows.iter().any(|r| r.ticket.id == overdue && r.due.is_some()));
        // Board: Karten tragen Fälligkeit und Auswertung
        let sum = project_summary(&h.db.conn, h.project).unwrap();
        let bid = ensure_default_board(&h.db.conn, h.project, h.lang).unwrap();
        let board = list_boards(&h.db.conn, h.project).unwrap().into_iter().find(|b| b.id == bid).unwrap();
        let data = load_board(&h.db.conn, &sum, &board.config, &Quick::default(), h.lang).unwrap();
        let card = |id: i64| data.card(id).unwrap();
        let red = card(overdue).due_state.clone().unwrap();
        assert!(red.overdue() && red.marker().unwrap().color == due::RED && red.tint().is_none());
        let yellow = card(soon).due_state.clone().unwrap();
        assert_eq!((yellow.marker().unwrap().color.as_str(), yellow.tint()), (due::YELLOW, Some(due::YELLOW)));
        assert!(card(later).due_state.as_ref().unwrap().marker().is_none());
        h.ui.tab = Tab::Boards;
        h.frames(3);
        // Kopf: 1 überfällig, 2 markiert
        h.ui.due_counts.invalidate();
        h.frames(1);
        assert_eq!(h.ui.due_counts.get(h.project), (1, 2));
        assert_eq!(due::count_text(1, 2, h.lang), if h.lang == Lang::De { "1 überfällig" } else { "1 overdue" });
    });
}
