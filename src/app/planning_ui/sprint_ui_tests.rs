//! Headless-egui-Tests der Backlog-/Sprint-/Release-Oberfläche: Die Seite wird mit echten (synthetischen) egui-Ereignissen
//! gezeichnet – Klicks mit Strg, Ziehen und Fallenlassen, Esc – und das Ergebnis in der Datenbank geprüft. Es wird kein Fenster
//! geöffnet und nichts an Maus oder Tastatur des Rechners angefasst.

use super::release_view::Act as RAct;
use super::sprint_logic::{LeftoverChoice, Section, SectionGeom};
use super::sprint_view::Sub;
use super::{Cx, PlanUi, Tab};
use crate::app::tasks_ui::Env;
use crate::calc::calendar::WorkCalendar;
use crate::db::Db;
use crate::planning::history::Source;
use crate::planning::projects::{update_project, UpdateProject};
use crate::planning::sprints::releases::{create_version, NewRelease, ReleaseKind};
use crate::planning::sprints::sprint::{complete_sprint, create_sprint, move_tickets_to_sprint, sprint_content, start_sprint, NewSprint};
use crate::planning::sprints::{clock, Leftover};
use crate::planning::tickets::fixture::Fx;
use crate::planning::transition::transition_ticket;
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Id, Modifiers, PointerButton, Pos2, Rect, Vec2};

/// Kontext der Oberfläche auf die Datenbank der Testumgebung (leiht nur `db`, nicht die ganze Umgebung).
macro_rules! cx {
    ($h:expr) => {
        Cx { conn: &$h.db.conn, db: &$h.db, lang: $h.lang }
    };
}

/// Testumgebung: Datenbank mit Projekt „PAN“ (Sprints und Versionen an) und eine Projektansicht.
struct H {
    ctx: egui::Context,
    db: Db,
    ui: PlanUi,
    lang: Lang,
    project: i64,
    t: f64,
}

impl H {
    fn new(lang: Lang, setup: impl FnOnce(&Fx) -> Vec<i64>) -> (H, Vec<i64>) {
        clock::set("2026-11-02T09:00:00");
        let f = Fx::new();
        update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
        let ids = setup(&f);
        let project = f.project;
        let db = Db { conn: f.c, path: std::path::PathBuf::new() };
        let mut ui = PlanUi::default();
        ui.loaded = true;
        ui.expert = true;
        ui.selected = project;
        ui.tab = Tab::Backlog;
        let mut h = H { ctx: egui::Context::default(), db, ui, lang, project, t: 1.0 };
        h.frames(3);
        (h, ids)
    }

    fn frame_with(&mut self, events: Vec<Event>, modifiers: Modifiers) {
        self.t += 0.05;
        let env = Env { db: Some(&self.db), db_error: None, lang: self.lang, bg: Color32::BLACK, undo_steps: 0 };
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 900.0))), time: Some(self.t), events, modifiers, ..Default::default() };
        let (ui, db) = (&mut self.ui, &self.db);
        let _ = self.ctx.run(input, |ctx| {
            ui.header(ctx, &env, db);
            ui.page(ctx, &env, db);
        });
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame_with(vec![], Modifiers::NONE);
        }
    }

    fn button(&mut self, pos: Pos2, pressed: bool, mods: Modifiers) {
        self.frame_with(vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: mods }], mods);
    }

    fn moved(&mut self, pos: Pos2) {
        self.frame_with(vec![Event::PointerMoved(pos)], Modifiers::NONE);
    }

    /// Rechteck der Zeile eines Tickets (aus dem letzten Bild).
    fn row(&self, ticket: i64) -> Rect {
        self.ctx.read_response(Id::new(("sprint_row", ticket))).unwrap_or_else(|| panic!("Zeile {ticket} nicht gezeichnet")).rect
    }

    fn click(&mut self, ticket: i64, mods: Modifiers) {
        let p = self.row(ticket).center();
        self.button(p, true, mods);
        self.button(p, false, mods);
    }

    /// Zieht ein Ticket an `to` und lässt es dort fallen.
    fn drag(&mut self, ticket: i64, to: Pos2) {
        let from = self.row(ticket).center();
        self.button(from, true, Modifiers::NONE);
        self.moved(from + Vec2::new(30.0, 12.0));
        self.moved(to);
        self.moved(to);
        self.button(to, false, Modifiers::NONE);
        self.frames(3);
    }

    fn geom(&self, s: Section) -> SectionGeom {
        self.ui.sprints.layout.as_ref().unwrap().0.iter().find(|g| g.section == s).cloned().unwrap_or_else(|| panic!("Bereich {s:?} fehlt"))
    }

    fn x_mid(&self) -> f32 {
        let (_, (a, b)) = self.ui.sprints.layout.as_ref().unwrap();
        (a + b) / 2.0
    }

    fn sprint_ids(&self, sprint: i64) -> Vec<i64> {
        sprint_content(&self.db.conn, sprint).unwrap().items.iter().map(|i| i.ticket.id).collect()
    }

    fn backlog_ids(&self) -> Vec<i64> {
        crate::planning::sprints::sprint::backlog(&self.db.conn, self.project, true).unwrap().iter().map(|t| t.id).collect()
    }
}

fn setup_three(f: &Fx) -> Vec<i64> {
    let a = f.mk(f.task, "Alpha");
    let b = f.mk(f.task, "Beta");
    let c = f.mk(f.task, "Gamma");
    let s = create_sprint(&f.c, &NewSprint::new(f.project, "Sprint 1"), &WorkCalendar::default()).unwrap();
    vec![a.id, b.id, c.id, s]
}

fn d(y: i32, m: u32, day: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

#[test]
fn the_backlog_tab_shows_sprints_and_backlog() {
    let (h, ids) = H::new(Lang::En, setup_three);
    let data = h.ui.sprints.data.as_ref().expect("Daten geladen");
    assert_eq!(data.sprints.len(), 1);
    assert_eq!(data.backlog.iter().map(|r| r.ticket.id).collect::<Vec<_>>(), vec![ids[0], ids[1], ids[2]]);
    let tabs: Vec<Tab> = h.ui.tabs(Lang::En).into_iter().map(|(t, _)| t).collect();
    assert!(tabs.contains(&Tab::Backlog) && tabs.contains(&Tab::Releases));
    // beide Bereiche haben eine Lage im Bild, der Sprint liegt über dem Backlog
    let (sp, bl) = (h.geom(Section::Sprint(ids[3])), h.geom(Section::Backlog));
    assert!(sp.bottom < bl.top);
    assert_eq!(bl.rows.len(), 3);
}

#[test]
fn the_tabs_follow_the_project_switches() {
    let (mut h, _) = H::new(Lang::De, |_| vec![]);
    update_project(&h.db.conn, h.project, &UpdateProject { use_sprints: Some(false), use_versions: Some(false), ..Default::default() }).unwrap();
    h.ui.invalidate();
    h.frames(3);
    let tabs: Vec<Tab> = h.ui.tabs(Lang::De).into_iter().map(|(t, _)| t).collect();
    assert!(!tabs.contains(&Tab::Backlog) && !tabs.contains(&Tab::Releases));
    assert_eq!(h.ui.tab, Tab::Overview, "der Reiter fällt auf die Übersicht zurück");
}

#[test]
fn dragging_a_ticket_into_a_sprint_moves_it() {
    let (mut h, ids) = H::new(Lang::En, setup_three);
    let (a, s) = (ids[0], ids[3]);
    let g = h.geom(Section::Sprint(s));
    h.drag(a, Pos2::new(h.x_mid(), g.body_top + 12.0));
    assert_eq!(h.sprint_ids(s), vec![a]);
    assert_eq!(h.backlog_ids(), vec![ids[1], ids[2]]);
    let dd = h.ui.sprints.data.as_ref().unwrap();
    assert_eq!(dd.sprints[0].rows.len(), 1, "die Ansicht hat neu geladen");
    assert!(h.ui.msg.as_ref().is_none_or(|m| m.ok), "kein Fehler: {:?}", h.ui.msg.as_ref().map(|m| &m.text));
}

#[test]
fn dragging_inside_the_backlog_reorders_by_rank() {
    let (mut h, ids) = H::new(Lang::En, setup_three);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let top_of_a = h.row(a).top() + 2.0;
    let x = h.x_mid();
    h.drag(c, Pos2::new(x, top_of_a));
    assert_eq!(h.backlog_ids(), vec![c, a, b]);
    // und ans Ende (unter die letzte Zeile)
    let end = h.geom(Section::Backlog).bottom - 2.0;
    h.drag(c, Pos2::new(x, end));
    assert_eq!(h.backlog_ids(), vec![a, b, c]);
}

#[test]
fn ctrl_click_selects_several_and_dragging_one_moves_all() {
    let (mut h, ids) = H::new(Lang::En, setup_three);
    let (a, b, c, s) = (ids[0], ids[1], ids[2], ids[3]);
    h.click(a, Modifiers::NONE);
    h.click(c, Modifiers::COMMAND);
    assert!(h.ui.sprints.selection.contains(a) && h.ui.sprints.selection.contains(c) && !h.ui.sprints.selection.contains(b));
    h.frames(2);
    let g = h.geom(Section::Sprint(s));
    h.drag(c, Pos2::new(h.x_mid(), g.body_top + 12.0));
    assert_eq!(h.sprint_ids(s), vec![a, c], "beide, in Reihenfolge des Rangs");
    assert_eq!(h.backlog_ids(), vec![b]);
}

#[test]
fn shift_click_selects_a_range() {
    let (mut h, ids) = H::new(Lang::En, setup_three);
    h.click(ids[0], Modifiers::NONE);
    h.click(ids[2], Modifiers::SHIFT);
    for i in &ids[..3] {
        assert!(h.ui.sprints.selection.contains(*i));
    }
}

#[test]
fn escape_cancels_a_drag() {
    let (mut h, ids) = H::new(Lang::En, setup_three);
    let (a, s) = (ids[0], ids[3]);
    let g = h.geom(Section::Sprint(s));
    let to = Pos2::new(h.x_mid(), g.body_top + 12.0);
    let from = h.row(a).center();
    h.button(from, true, Modifiers::NONE);
    h.moved(from + Vec2::new(30.0, 12.0));
    h.moved(to);
    h.frame_with(vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
    h.button(to, false, Modifiers::NONE);
    h.frames(2);
    assert!(h.sprint_ids(s).is_empty());
    assert_eq!(h.backlog_ids().len(), 3);
}

#[test]
fn a_second_start_shows_a_bilingual_error() {
    for (lang, needle) in [(Lang::En, "already running"), (Lang::De, "läuft schon")] {
        let (mut h, ids) = H::new(lang, |f| {
            let s1 = create_sprint(&f.c, &NewSprint::new(f.project, "S1"), &WorkCalendar::default()).unwrap();
            let s2 = create_sprint(&f.c, &NewSprint::new(f.project, "S2"), &WorkCalendar::default()).unwrap();
            vec![s1, s2]
        });
            h.ui.start_sprint_now(&cx!(h), ids[0]);
        assert!(h.ui.msg.as_ref().is_none_or(|m| m.ok), "der erste Start gelingt");
        h.ui.start_sprint_now(&cx!(h), ids[1]);
        let m = h.ui.msg.as_ref().expect("Fehlermeldung");
        assert!(!m.ok && m.text.contains(needle), "{}", m.text);
        h.frames(2); // im Bild unter den Reitern, ohne Absturz
    }
}

#[test]
fn new_sprint_form_suggests_name_and_dates_and_saves() {
    let (mut h, _) = H::new(Lang::En, |_| vec![]);
    let proj = h.ui.project().cloned().unwrap();
    h.ui.open_new_sprint(&cx!(h), &proj);
    {
        let f = &h.ui.sprints.form.as_ref().expect("Formular").f;
        assert_eq!(f.name, "Sprint 1");
        assert!(!f.start.is_empty() && !f.end.is_empty(), "Termine vorgeschlagen");
        assert_eq!(f.days, "10");
    }
    h.frames(2); // Dialog zeichnen
    let mut form = h.ui.sprints.form.take().unwrap();
    h.ui.save_sprint_form(&cx!(h), &form, &WorkCalendar::default()).unwrap();
    let list = crate::planning::sprints::sprint::list_sprints(&h.db.conn, h.project).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Sprint 1");
    assert!(list[0].start_date.is_some() && list[0].end_date.is_some());
    // schlechte Eingaben liefern zweisprachige Fehler und legen nichts an
    form.f.start = "kein Datum".into();
    let e = h.ui.save_sprint_form(&cx!(h), &form, &WorkCalendar::default()).unwrap_err();
    assert!(e.contains("YYYY-MM-DD"), "{e}");
    form.f.start.clear();
    form.f.end.clear();
    form.f.name = "sprint 1".into();
    let e = h.ui.save_sprint_form(&cx!(h), &form, &WorkCalendar::default()).unwrap_err();
    assert!(!e.is_empty(), "der Name ist schon vergeben");
    assert_eq!(crate::planning::sprints::sprint::list_sprints(&h.db.conn, h.project).unwrap().len(), 1);
}

#[test]
fn completing_carries_the_unfinished_tickets_to_the_next_sprint() {
    let (mut h, ids) = H::new(Lang::En, |f| {
        let a = f.mk(f.task, "Fertig");
        let b = f.mk(f.task, "Offen");
        let cal = WorkCalendar::default();
        let mut n = NewSprint::new(f.project, "S1");
        n.start = Some(d(2026, 11, 2));
        let s1 = create_sprint(&f.c, &n, &cal).unwrap();
        let s2 = create_sprint(&f.c, &NewSprint::new(f.project, "S2"), &cal).unwrap();
        move_tickets_to_sprint(&f.c, s1, &[a.id, b.id]).unwrap();
        start_sprint(&f.c, s1, &cal).unwrap();
        transition_ticket(&f.c, a.id, f.done, &[], &Source::ui()).unwrap();
        vec![a.id, b.id, s1, s2]
    });
    let (b, s1, s2) = (ids[1], ids[2], ids[3]);
    h.ui.open_complete(&cx!(h), s1);
    h.frames(2);
    let mut dlg = h.ui.sprints.complete.take().expect("Dialog");
    assert_eq!((dlg.done, dlg.open.len()), (1, 1));
    assert_eq!(dlg.planned.iter().map(|p| p.0).collect::<Vec<_>>(), vec![s2]);
    dlg.choice = LeftoverChoice::NextSprint;
    h.ui.finish_sprint(&cx!(h), &mut dlg);
    assert_eq!(dlg.result.as_deref().unwrap(), "1 ticket done; 1 ticket open – moved to the sprint “S2”.");
    assert_eq!(h.sprint_ids(s2), vec![b]);
    h.ui.sprints.complete = Some(dlg);
    h.frames(2); // Ergebnisansicht zeichnen
    h.ui.sprints.sub = Sub::Reports;
    h.frames(3);
    assert!(h.ui.sprints.detail.is_some(), "Bericht geladen");
    assert_eq!(h.ui.sprints.velocity.as_ref().map(|v| v.points.len()), Some(1));
}

#[test]
fn active_sprint_and_report_views_render() {
    let (mut h, ids) = H::new(Lang::De, |f| {
        let a = f.mk(f.task, "Eins");
        let cal = WorkCalendar::default();
        let mut n = NewSprint::new(f.project, "S1");
        n.start = Some(d(2026, 11, 2));
        let s1 = create_sprint(&f.c, &n, &cal).unwrap();
        move_tickets_to_sprint(&f.c, s1, &[a.id]).unwrap();
        start_sprint(&f.c, s1, &cal).unwrap();
        vec![a.id, s1]
    });
    h.ui.sprints.sub = Sub::Active;
    h.frames(3);
    let det = &h.ui.sprints.detail.as_ref().expect("Detail").detail;
    assert_eq!(det.sprint.id, ids[1]);
    assert!(det.series.is_some());
    // andere Zählweise und leeres Velocity-Diagramm
    h.ui.sprints.basis = crate::calc::progress::Basis::TicketCount;
    h.ui.sprints.sub = Sub::Reports;
    h.frames(3);
    assert_eq!(h.ui.sprints.velocity.as_ref().map(|v| v.points.len()), Some(0));
    // ohne aktiven Sprint: Hinweis statt Absturz
    complete_sprint(&h.db.conn, ids[1], Leftover::ToBacklog).unwrap();
    h.ui.sprints.sub = Sub::Active;
    h.ui.sprint_changed();
    h.frames(3);
}

#[test]
fn releases_tab_lists_versions_with_progress_and_asks_before_deleting() {
    let (mut h, ids) = H::new(Lang::En, |f| {
        let t = f.mk(f.task, "Mit Version");
        let v = create_version(&f.c, &NewRelease::new(f.project, "1.0")).unwrap();
        create_version(&f.c, &NewRelease::new(f.project, "1.1")).unwrap();
        let field = f.add_field(f.task, "Fix in", crate::planning::fields::FieldKind::Version, Default::default(), false);
        crate::planning::tickets::set_field(&f.c, t.id, field.id, serde_json::json!(v), &Source::ui()).unwrap();
        vec![t.id, v]
    });
    h.ui.tab = Tab::Releases;
    h.frames(3);
    assert_eq!(h.ui.releases.rows.len(), 2);
    assert_eq!(h.ui.releases.rows[0].progress.total(), 1);
    let proj = h.ui.project().cloned().unwrap();
    h.ui.release_act(&cx!(h), &proj, RAct::Delete(ids[1]));
    h.frames(2);
    let del = h.ui.releases.del.as_ref().expect("Löschdialog mit Rückfrage");
    assert_eq!((del.id, del.usage), (ids[1], 1));
    // markieren, sortieren
    h.ui.release_act(&cx!(h), &proj, RAct::Done(ids[1], true));
    assert!(h.ui.msg.is_none());
    h.frames(2);
    assert!(h.ui.releases.rows.iter().find(|r| r.release.id == ids[1]).unwrap().release.done);
    h.ui.release_act(&cx!(h), &proj, RAct::Move(0, 1));
    h.frames(2);
    assert_eq!(h.ui.releases.rows[1].release.id, ids[1]);
    // Meilensteine
    h.ui.releases.kind = ReleaseKind::Milestone;
    h.ui.release_act(&cx!(h), &proj, RAct::New);
    h.frames(3);
    assert!(h.ui.releases.form.is_some());
    assert!(h.ui.releases.rows.is_empty());
}

#[test]
fn many_tickets_are_paged_in_both_languages() {
    for lang in [Lang::De, Lang::En] {
        let (h, _) = H::new(lang, |f| {
            for i in 0..90 {
                f.mk(f.task, &format!("Ticket mit einem ziemlich langen Titel, der sicher nicht mehr in eine Zeile passt – Nummer {i}"));
            }
            vec![]
        });
        assert_eq!(h.ui.sprints.data.as_ref().unwrap().backlog.len(), 90);
        // Seitenlimit: nicht mehr als PAGE Zeilen gezeichnet
        assert_eq!(h.geom(Section::Backlog).rows.len(), super::sprint_view::PAGE);
    }
}
