//! Tests der Ticket-Oberfläche ohne Fenster: egui läuft mit künstlichen Eingaben (`Context::run`), die Daten liegen in einer
//! echten Testdatenbank. So lassen sich Zeichnen (kein Absturz bei Listen, Seiten, Panels und Dialogen in beiden Sprachen),
//! Tastenkürzel (Strg+N, Strg+Enter, Strg+Z/Y) und das Zusammenspiel mit der Datenschicht prüfen, ohne Maus und Tastatur
//! des Nutzers zu benutzen.

use super::ticket_state::{SortCol, ViewMode};
use super::ticket_view::Act;
use super::{Cx, PlanUi, Tab};
use crate::app::tasks_ui::Env;
use crate::db::Db;
use crate::planning::fields::{list_fields, FieldKind};
use crate::planning::history::Source;
use crate::planning::projects::{create_project, NewProject};
use crate::planning::ticket_list::{list_tickets, TicketFilter};
use crate::planning::tickets::{create_ticket, get_ticket, NewTicket};
use crate::planning::types::list_types;
use crate::planning::workflow::{list_transitions, set_transition, TransitionDef};
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
use serde_json::json;

struct H {
    ctx: egui::Context,
    db: Db,
    ui: PlanUi,
    lang: Lang,
    time: f64,
    project: i64,
}

fn ctrl(key: Key) -> Event {
    Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: Modifiers { ctrl: true, command: true, ..Default::default() } }
}

impl H {
    fn new(lang: Lang) -> H {
        let dir = std::env::temp_dir().join(format!("paneldeck-test-ticketui-{}-{:?}", std::process::id(), std::thread::current().id()));
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

    fn with<R>(&mut self, f: impl FnOnce(&mut PlanUi, &Cx) -> R) -> R {
        let cx = Cx { conn: &self.db.conn, db: &self.db, lang: self.lang };
        f(&mut self.ui, &cx)
    }

    fn count(&self) -> usize {
        list_tickets(&self.db.conn, Some(self.project), &TicketFilter::default()).unwrap().total as usize
    }

    fn type_id(&self, key: &str) -> i64 {
        list_types(&self.db.conn, self.project).unwrap().into_iter().find(|t| t.key.as_deref() == Some(key)).unwrap().id
    }

    fn mk(&self, key: &str, title: &str) -> i64 {
        create_ticket(&self.db.conn, &NewTicket::new(self.project, self.type_id(key), title), &Source::ui()).unwrap().0.id
    }

    fn open(&mut self, id: i64) {
        self.ui.tickets.open_ticket(id, false);
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
fn list_renders_sorts_and_filters_in_both_languages() {
    both(|h| {
        let epic = h.type_id("epic");
        for i in 0..40 {
            h.mk(if i % 7 == 0 { "epic" } else { "devtask" }, &format!("Ticket Nummer {i}"));
        }
        h.ui.tickets.invalidate();
        h.frames(3);
        assert_eq!(h.ui.tab, Tab::Tickets);
        assert_eq!(h.ui.tickets.rows.len(), 40);
        for col in [SortCol::Title, SortCol::Status, SortCol::Priority, SortCol::Points, SortCol::Marks, SortCol::Type, SortCol::Key] {
            h.ui.tickets.sort = h.ui.tickets.sort.toggled(col);
            let sum = h.ui.sum.clone().unwrap();
            h.ui.resort(&sum);
            h.frames(2);
        }
        h.ui.tickets.filters.toggle_type(epic);
        h.ui.tickets.list_dirty = true;
        h.frames(3);
        assert_eq!(h.ui.tickets.rows.len(), 6);
        h.ui.tickets.filters.text = "Nummer 14".into();
        h.ui.tickets.list_dirty = true;
        h.frames(3);
        assert_eq!(h.ui.tickets.rows.len(), 1);
        h.ui.tickets.filters.text = "gibtsnicht".into();
        h.ui.tickets.list_dirty = true;
        h.frames(3);
        assert!(h.ui.tickets.rows.is_empty());
        h.frames(2);
    });
}

#[test]
fn page_and_panel_show_the_ticket_with_all_sections() {
    both(|h| {
        let epic = h.mk("epic", "Das Epic");
        let story = h.mk("userstory", "Eine Story");
        h.with(|ui, cx| {
            let r = crate::planning::tickets::set_parent(cx.conn, story, Some(epic), &Source::ui());
            ui.t_apply(cx, r);
        });
        let _ = h.mk("subtask", "Ein Subtask");
        h.open(story);
        assert!(h.ui.tickets.detail.is_some());
        h.ui.tickets.mode = ViewMode::Panel;
        h.frames(3);
        h.ui.tickets.mode = ViewMode::Page;
        h.frames(3);
        // Kommentar, Zeit, Verknüpfung, Kind über die Aufträge
        h.with(|ui, cx| {
            ui.ticket_act(cx, Act::AddComment("**Hallo** PAN-1".into()));
            ui.ticket_act(cx, Act::Star(true));
            ui.ticket_act(cx, Act::Flag(true));
        });
        h.frames(4);
        let d = h.ui.tickets.detail.as_ref().unwrap();
        assert_eq!(d.comments.len(), 1);
        assert!(d.ticket.starred && d.ticket.flagged);
        assert_eq!(d.parent.as_ref().map(|p| p.id), Some(epic));
        h.open(epic);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().children.len(), 1);
        assert!(h.ui.tickets.detail.as_ref().unwrap().progress.is_some());
    });
}

#[test]
fn ctrl_n_opens_the_dialog_and_ctrl_enter_creates_the_ticket() {
    both(|h| {
        assert_eq!(h.count(), 0);
        h.frame(vec![ctrl(Key::N)]);
        h.frames(2);
        assert!(h.ui.tickets.create.is_some(), "Strg+N öffnet den Anlege-Dialog");
        // ohne Titel: Fehler, nichts angelegt
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(1);
        assert_eq!(h.count(), 0);
        let dlg = h.ui.tickets.create.as_ref().unwrap();
        assert!(dlg.err.is_some() && dlg.tried, "Pflichtfeld-Fehler sichtbar");
        h.ui.tickets.create.as_mut().unwrap().title = "Mein erstes Ticket".into();
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(2);
        assert_eq!(h.count(), 1);
        assert!(h.ui.tickets.create.is_none(), "Dialog schließt");
        assert!(h.ui.tickets.open.is_some(), "das neue Ticket öffnet sich");
        assert_eq!(h.ui.tickets.toasts.len(), 1);
        // Strg+Z nimmt das Anlegen zurück
        h.ui.tickets.toasts.clear();
        h.frames(2);
        h.frame(vec![ctrl(Key::Z)]);
        h.frames(3);
        assert_eq!(h.count(), 0, "Strg+Z entfernt das Ticket wieder");
        assert!(h.ui.tickets.toasts.first().is_some_and(|t| t.contains(if h.lang == Lang::En { "Undone" } else { "Rückgängig" })), "{:?}", h.ui.tickets.toasts);
        h.frame(vec![ctrl(Key::Y)]);
        h.frames(3);
        assert_eq!(h.count(), 1, "Strg+Y legt es wieder an");
    });
}

#[test]
fn create_another_keeps_the_dialog_open_and_resets_the_fields() {
    both(|h| {
        h.with(|ui, cx| ui.open_create(cx, None));
        h.frames(2);
        h.ui.tickets.create_another = true;
        for i in 0..3 {
            h.ui.tickets.create.as_mut().unwrap().title = format!("Serie {i}");
            h.frame(vec![ctrl(Key::Enter)]);
            h.frames(1);
            assert!(h.ui.tickets.create.is_some(), "Dialog bleibt offen");
            assert!(h.ui.tickets.create.as_ref().unwrap().title.is_empty(), "Titel geleert");
        }
        assert_eq!(h.count(), 3);
    });
}

#[test]
fn required_fields_are_enforced_in_the_dialog() {
    let mut h = H::new(Lang::En);
    let task = h.type_id("devtask");
    let mut nf = crate::planning::fields::NewField::new(task, "Owner", FieldKind::Text);
    nf.required = true;
    let fid = crate::planning::fields::create_field(&h.db.conn, &nf).unwrap();
    h.ui.dirty = true;
    h.frames(2);
    h.with(|ui, cx| ui.open_create(cx, None));
    h.frames(2);
    {
        let dlg = h.ui.tickets.create.as_mut().unwrap();
        // der erste Typ ist „Epic“; auf Devtask umstellen
        let sum = crate::planning::summary::project_summary(&h.db.conn, h.project).unwrap();
        let ty = sum.type_by_id(task).unwrap().clone();
        dlg.type_id = ty.ticket_type.id;
        dlg.drafts = ty.fields.iter().map(|f| (f.id, super::ticket_draft::Draft::from_value(f, f.default_value.as_ref()))).collect();
        dlg.title = "Braucht Owner".into();
    }
    h.frame(vec![ctrl(Key::Enter)]);
    h.frames(2);
    assert_eq!(h.count(), 0, "ohne Pflichtfeld wird nichts angelegt");
    h.ui.tickets.create.as_mut().unwrap().drafts.get_mut(&fid).unwrap().text = "ich".into();
    h.ui.tickets.create.as_mut().unwrap().drafts.get_mut(&fid).unwrap().edited = true;
    h.frame(vec![ctrl(Key::Enter)]);
    h.frames(2);
    assert_eq!(h.count(), 1);
}

#[test]
fn status_change_with_required_field_asks_and_then_moves() {
    both(|h| {
        let task = h.type_id("devtask");
        let t = h.mk("devtask", "Bald fertig");
        let fields = list_fields(&h.db.conn, task).unwrap();
        let mut nf = crate::planning::fields::NewField::new(task, "Ergebnis", FieldKind::Text);
        nf.required = false;
        let fid = crate::planning::fields::create_field(&h.db.conn, &nf).unwrap();
        let _ = fields;
        let trs = list_transitions(&h.db.conn, task).unwrap();
        let tr = trs.iter().find(|t| t.def.to_status_id != t.def.from_status_id && h.ui.sum.as_ref().map(|s| s.status(t.def.to_status_id).is_some_and(|s| s.category == crate::planning::statuses::StatusCategory::Done)).unwrap_or(false)).unwrap();
        let mut def: TransitionDef = tr.def.clone();
        def.required_fields = vec![fid];
        set_transition(&h.db.conn, task, &def).unwrap();
        h.ui.dirty = true;
        h.open(t);
        let (to, from) = (def.to_status_id, def.from_status_id);
        assert_eq!(get_ticket(&h.db.conn, t).unwrap().status_id, Some(from));
        let opt = h.ui.tickets.detail.as_ref().unwrap().transitions.iter().find(|o| o.status_id == to).cloned().unwrap();
        assert_eq!(opt.needs_fields.len(), 1, "der Dialog wird gebraucht");
        h.with(|ui, cx| ui.ticket_act(cx, Act::Transition { to, needs: opt.needs_fields.clone() }));
        h.frames(2);
        assert!(h.ui.tickets.transition.is_some());
        // ohne Wert: bleibt offen, nichts passiert
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(1);
        assert_eq!(get_ticket(&h.db.conn, t).unwrap().status_id, Some(from));
        assert!(h.ui.tickets.transition.is_some());
        let d = h.ui.tickets.transition.as_mut().unwrap().drafts.get_mut(&fid).unwrap();
        d.text = "erledigt".into();
        d.edited = true;
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(2);
        assert!(h.ui.tickets.transition.is_none());
        assert_eq!(get_ticket(&h.db.conn, t).unwrap().status_id, Some(to));
        // Rückgängig: Status und Wert
        h.with(|ui, cx| ui.ticket_undo(cx, false));
        assert_eq!(get_ticket(&h.db.conn, t).unwrap().status_id, Some(from));
    });
}

#[test]
fn field_edits_are_saved_with_the_ui_source_and_can_be_undone() {
    both(|h| {
        let t = h.mk("userstory", "Story");
        h.open(t);
        let (sp, title) = {
            let d = h.ui.tickets.detail.as_ref().unwrap();
            (d.ty.field_by_key("story_points").unwrap().id, d.ticket.title.clone())
        };
        {
            let dr = h.ui.tickets.view.drafts.get_mut(&sp).unwrap();
            dr.text = "8".into();
            dr.edited = true;
        }
        h.with(|ui, cx| ui.ticket_act(cx, Act::CommitField(sp)));
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().stored.get(&sp), Some(&json!(8)));
        // ungültig: bleibt am Feld stehen, nichts gespeichert
        {
            let dr = h.ui.tickets.view.drafts.get_mut(&sp).unwrap();
            dr.text = "-5".into();
            dr.edited = true;
        }
        h.with(|ui, cx| ui.ticket_act(cx, Act::CommitField(sp)));
        h.frames(3);
        assert!(h.ui.tickets.view.drafts[&sp].err.is_some(), "Grenzen werden angezeigt");
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().stored.get(&sp), Some(&json!(8)));
        // Titel
        h.with(|ui, cx| ui.ticket_act(cx, Act::Title("Neuer Titel".into())));
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().ticket.title, "Neuer Titel");
        let hist = &h.ui.tickets.detail.as_ref().unwrap().history;
        assert!(hist.iter().all(|(_, text)| text.starts_with(if h.lang == Lang::En { "User interface" } else { "Oberfläche" })), "{hist:?}");
        h.with(|ui, cx| ui.ticket_undo(cx, false));
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().ticket.title, title);
        h.with(|ui, cx| ui.ticket_undo(cx, false));
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().stored.get(&sp), None);
        h.with(|ui, cx| ui.ticket_undo(cx, true));
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().stored.get(&sp), Some(&json!(8)), "Wiederholen");
    });
}

#[test]
fn new_change_after_undo_drops_the_future() {
    let mut h = H::new(Lang::En);
    let t = h.mk("devtask", "T");
    h.open(t);
    h.with(|ui, cx| ui.ticket_act(cx, Act::Star(true)));
    h.with(|ui, cx| ui.ticket_act(cx, Act::Flag(true)));
    h.with(|ui, cx| ui.ticket_undo(cx, false));
    assert!(h.ui.tickets.history.can_redo());
    h.with(|ui, cx| ui.ticket_act(cx, Act::Title("Anders".into())));
    assert!(!h.ui.tickets.history.can_redo(), "Zukunft verworfen");
}

#[test]
fn links_children_and_parent_work_through_the_ui_actions() {
    both(|h| {
        let a = h.mk("devtask", "A");
        let b = h.mk("devtask", "B");
        h.open(a);
        let types = crate::planning::link_types::list_link_types(&h.db.conn).unwrap();
        let offers = super::ticket_data::link_offers(&types, h.lang);
        let blocks = offers.iter().find(|o| !o.reverse && o.kind == crate::planning::link_types::TargetKind::Ticket).unwrap().clone();
        h.with(|ui, cx| ui.ticket_act(cx, Act::AddLink { offer: blocks.clone(), target: crate::planning::links::Target::Ticket(b) }));
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().links.len(), 1);
        let lid = h.ui.tickets.detail.as_ref().unwrap().links[0].id;
        // gleiche Verknüpfung noch einmal: Fehlermeldung, keine zweite
        h.with(|ui, cx| ui.ticket_act(cx, Act::AddLink { offer: blocks.clone(), target: crate::planning::links::Target::Ticket(b) }));
        assert!(h.ui.msg.as_ref().is_some_and(|m| !m.ok));
        h.frames(2);
        // rückwärts: „B ist blockiert durch A“ entspricht A blockiert B – nochmals ein Duplikat
        h.with(|ui, cx| ui.ticket_act(cx, Act::RemoveLink(lid)));
        h.frames(3);
        assert!(h.ui.tickets.detail.as_ref().unwrap().links.is_empty());
        h.with(|ui, cx| ui.ticket_undo(cx, false));
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().links.len(), 1, "Entfernen lässt sich zurücknehmen");
        // Kind über den Dialog anlegen
        h.with(|ui, cx| ui.ticket_act(cx, Act::CreateChild));
        h.frames(2);
        assert!(h.ui.tickets.create.as_ref().is_some_and(|d| d.parent == Some(a)));
    });
}

#[test]
fn dialogs_for_clone_change_time_delete_render_and_work() {
    both(|h| {
        let story = h.mk("userstory", "Story");
        let epic = h.mk("epic", "Epic");
        let sp = list_fields(&h.db.conn, h.type_id("userstory")).unwrap().into_iter().find(|f| f.key.as_deref() == Some("story_points")).unwrap();
        h.with(|_, cx| crate::planning::tickets::set_field(cx.conn, story, sp.id, json!(3), &Source::ui()).unwrap());
        h.open(story);
        // Klonen
        h.with(|ui, cx| ui.ask_clone(cx, story));
        h.frames(2);
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(3);
        assert_eq!(h.count(), 3, "Klon angelegt");
        assert!(h.ui.tickets.clone.is_none());
        // Typ ändern (Story → Devtask)
        let story = h.ui.tickets.open.unwrap_or(story);
        h.with(|ui, cx| ui.ask_change_type(cx, story));
        h.frames(2);
        let task = h.type_id("devtask");
        let report = h.with(|ui, cx| {
            let dlg = ui.tickets.change.as_mut().unwrap();
            dlg.to = task;
            dlg.refresh(cx);
            assert!(dlg.preview.is_some());
            dlg.execute(cx).unwrap()
        });
        assert_eq!(report.ticket.type_id, task);
        h.ui.tickets.change = None;
        h.frames(2);
        // Zeit erfassen
        h.with(|ui, cx| ui.open_time(cx, story, None));
        h.frames(2);
        h.ui.tickets.time.as_mut().unwrap().duration = "1h 30m".into();
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().time_total, 90);
        // Eintrag bearbeiten
        let eid = h.ui.tickets.detail.as_ref().unwrap().time[0].id;
        h.with(|ui, cx| ui.open_time(cx, story, Some(eid)));
        h.frames(2);
        h.ui.tickets.time.as_mut().unwrap().duration = "2h".into();
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(3);
        assert_eq!(h.ui.tickets.detail.as_ref().unwrap().time_total, 120);
        // ungültige Dauer bleibt im Dialog
        h.with(|ui, cx| ui.open_time(cx, story, None));
        h.ui.tickets.time.as_mut().unwrap().duration = "abc".into();
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(2);
        assert!(h.ui.tickets.time.as_ref().is_some_and(|t| t.err.is_some()));
        h.ui.tickets.time = None;
        // Löschen: Dialog zeigt den Timer; ohne Klick bleibt alles
        h.with(|ui, cx| ui.ask_delete_ticket(cx, epic));
        h.frames(3);
        assert!(h.ui.tickets.delete.is_some());
        h.ui.tickets.delete = None;
        // Bestätigungen
        h.ui.tickets.confirm = Some(super::ticket_dialogs::ConfirmDlg { kind: super::ticket_dialogs::ConfirmKind::Time { ticket: story, id: eid } });
        h.frames(2);
        h.ui.tickets.confirm = None;
        h.frames(2);
    });
}

#[test]
fn deleting_forgets_the_undo_entries_of_that_ticket() {
    let mut h = H::new(Lang::En);
    let a = h.mk("devtask", "A");
    let b = h.mk("devtask", "B");
    h.open(a);
    h.with(|ui, cx| ui.ticket_act(cx, Act::Star(true)));
    h.open(b);
    h.with(|ui, cx| ui.ticket_act(cx, Act::Star(true)));
    h.with(|ui, cx| ui.ask_delete_ticket(cx, a));
    // Sicherheitstimer überspringen und löschen: so, als wäre „Löschen“ nach 3 s geklickt worden
    {
        let d = h.ui.tickets.delete.as_mut().unwrap();
        d.since -= std::time::Duration::from_secs(5);
    }
    // Der Klick selbst ist nicht simulierbar; die Folgen der Löschung prüft die Datenschicht direkt:
    h.with(|ui, cx| {
        crate::planning::tickets::delete_ticket(cx.conn, a, &Source::ui()).unwrap();
        ui.tickets.forget_tickets(&[a]);
    });
    h.ui.tickets.delete = None;
    // nur noch der Stern von B ist übrig
    h.with(|ui, cx| ui.ticket_undo(cx, false));
    assert!(!get_ticket(&h.db.conn, b).unwrap().starred);
    h.with(|ui, cx| ui.ticket_undo(cx, false));
    assert!(!h.ui.tickets.history.can_undo(), "A taucht nicht mehr auf");
}

#[test]
fn todo_cards_convert_into_tickets() {
    both(|h| {
        let (item, _) = crate::tasks::create_item(&h.db.conn, "Wichtige Idee", "Mehr Text dazu", &["Alpha".to_string()]).unwrap();
        // das System-Projekt „Todolist“ anzeigen
        let todo = crate::planning::projects::list_projects(&h.db.conn).unwrap().into_iter().find(|p| p.system).unwrap().id;
        h.with(|ui, cx| ui.select(cx, todo));
        h.frames(3);
        h.with(|ui, cx| ui.ask_convert(cx, item.id));
        h.frames(2);
        assert!(h.ui.tickets.convert.is_some());
        {
            let dlg = h.ui.tickets.convert.as_mut().unwrap();
            assert_eq!(dlg.project, h.project, "das einzige normale Projekt");
        }
        h.frame(vec![ctrl(Key::Enter)]);
        h.frames(3);
        assert!(h.ui.tickets.convert.is_none(), "{:?}", h.ui.tickets.convert.as_ref().map(|c| c.err.clone()));
        assert_eq!(h.count(), 1);
        assert!(crate::planning::todo_bridge::converted_ticket_of(&h.db.conn, item.id).unwrap().is_some());
        // das Zurücknehmen entfernt das Ticket wieder
        h.with(|ui, cx| ui.ticket_undo(cx, false));
        assert_eq!(h.count(), 0);
    });
}

#[test]
fn switching_projects_resets_the_ticket_view() {
    let mut h = H::new(Lang::En);
    let t = h.mk("devtask", "T");
    h.open(t);
    let p2 = create_project(&h.db.conn, &NewProject::new("ZWEI", "Zweites", "#7FBF6A")).unwrap();
    h.with(|ui, cx| ui.select(cx, p2));
    h.frames(3);
    assert!(h.ui.tickets.open.is_none());
    assert!(h.ui.tickets.rows.is_empty());
    // ein Ticket aus einem anderen Projekt öffnen (Verweis) wechselt das Projekt
    let pid = h.project;
    h.with(|ui, cx| ui.open_ticket_anywhere(cx, t, pid));
    h.frames(3);
    assert_eq!(h.ui.selected, h.project);
    assert_eq!(h.ui.tickets.open, Some(t));
}
