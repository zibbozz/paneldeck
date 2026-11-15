//! Tests der Schnellaktions-Knöpfe in der Ticket-Ansicht: egui läuft mit künstlichen Eingaben (`Context::run`), die Daten liegen
//! in einer echten Testdatenbank. Geprüft wird, dass die Leiste genau die Aktionen des Tickettyps zeigt (und sich nach
//! Änderungen aktualisiert), dass ein Klick dieselbe Logik wie das Schnellmenü ausführt (verschieben + Rückgängig, Pflichtfeld-
//! Formular, Titel-Eingabe, Ticket anlegen, Sitzungsanfrage) und dass ungültige Aktionen nicht ausführbar sind – ohne Maus und
//! Tastatur des Nutzers.

use super::quick_menu::Out;
use super::ticket_state::ViewMode;
use super::ticket_view::Act;
use super::{Cx, PlanUi};
use crate::app::tasks_ui::Env;
use crate::db::Db;
use crate::planning::fields::{create_field, FieldKind, NewField};
use crate::planning::history::Source;
use crate::planning::projects::{create_project, NewProject};
use crate::planning::quick::actions::{create_quick_action, delete_quick_action, reorder_quick_actions, update_quick_action, NewQuickAction, UpdateQuickAction};
use crate::planning::quick::params::{ActionParams, ChangeStatusParams, CreateTicketParams, DefaultValue, FieldRef, StartSessionParams};
use crate::planning::ticket_list::{list_tickets, TicketFilter};
use crate::planning::tickets::{create_ticket, get_ticket, NewTicket};
use crate::planning::types::list_types;
use crate::planning::workflow::{list_transitions, set_transition};
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Id, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

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
        let dir = std::env::temp_dir().join(format!("paneldeck-test-quickbtn-{}-{:?}", std::process::id(), std::thread::current().id()));
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

    fn type_id(&self, key: &str) -> i64 {
        list_types(&self.db.conn, self.project).unwrap().into_iter().find(|t| t.key.as_deref() == Some(key)).unwrap().id
    }

    fn status_id(&self, key: &str) -> i64 {
        self.ui.sum.as_ref().unwrap().statuses.iter().find(|s| s.key.as_deref() == Some(key)).unwrap().id
    }

    fn mk(&self, key: &str, title: &str) -> i64 {
        create_ticket(&self.db.conn, &NewTicket::new(self.project, self.type_id(key), title), &Source::ui()).unwrap().0.id
    }

    fn count(&self) -> usize {
        list_tickets(&self.db.conn, Some(self.project), &TicketFilter::default()).unwrap().total as usize
    }

    fn status_of(&self, ticket: i64) -> Option<i64> {
        get_ticket(&self.db.conn, ticket).unwrap().status_id
    }

    fn add(&self, type_key: &str, name: &str, p: ActionParams) -> i64 {
        create_quick_action(&self.db.conn, &NewQuickAction::new(self.type_id(type_key), name, p)).unwrap()
    }

    /// Wie nach einer Änderung im Editor: die Oberfläche lädt beim nächsten Bild neu.
    fn changed(&mut self) {
        self.ui.dirty = true;
        self.frames(3);
    }

    fn open(&mut self, id: i64) {
        self.ui.tickets.open_ticket(id, false);
        self.frames(3);
    }

    /// Beschriftungen der Knöpfe des geöffneten Tickets.
    fn labels(&self) -> Vec<String> {
        self.ui.tickets.detail.as_ref().unwrap().quick.iter().map(|b| b.label.clone()).collect()
    }

    /// Bildschirmrechteck des Knopfes (vom Zeichnen im letzten Bild gemerkt).
    fn rect_of(&self, action: i64) -> Option<Rect> {
        self.ctx.data(|d| d.get_temp::<Rect>(Id::new(("qbtn_rect", action))))
    }

    /// Linksklick auf den Knopf in drei Bildern (hover, drücken, loslassen) – künstliche Ereignisse, keine echte Eingabe.
    fn click_button(&mut self, action: i64) {
        let pos = self.rect_of(action).expect("Knopf wurde gezeichnet").center();
        let btn = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![btn(true)]);
        self.frame(vec![btn(false)]);
        self.frames(2);
    }

    fn esc(&mut self) {
        self.frame(vec![Event::Key { key: Key::Escape, physical_key: Some(Key::Escape), pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }
}

fn both(f: impl Fn(&mut H)) {
    for lang in [Lang::En, Lang::De] {
        let mut h = H::new(lang);
        f(&mut h);
    }
}

fn start_session() -> ActionParams {
    ActionParams::StartSession(StartSessionParams::default())
}

fn create_dev(h: &H, title: Option<&str>) -> ActionParams {
    let defaults = title.map(|t| vec![DefaultValue { field_id: FieldRef::Title, value: serde_json::json!(t) }]).unwrap_or_default();
    ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: h.type_id("devtask"), relation: None, defaults })
}

fn to_status(h: &H, key: &str) -> ActionParams {
    ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: h.status_id(key), set_values: vec![] })
}

#[test]
fn the_bar_shows_exactly_the_actions_of_the_ticket_type_in_their_order() {
    both(|h| {
        let a = h.add("userstory", "Create Devtask", create_dev(h, Some("Do it")));
        let b = h.add("userstory", "Start", to_status(h, "progress"));
        let c = h.add("userstory", "Open terminal", start_session());
        let broken = h.add("userstory", "Broken", start_session());
        h.db.conn.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [broken]).unwrap();
        h.add("epic", "Epic only", start_session());
        let story = h.mk("userstory", "Eine Story");
        let epic = h.mk("epic", "Ein Epic");
        let task = h.mk("devtask", "Eine Aufgabe");
        h.changed();
        h.open(story);
        assert_eq!(h.labels(), ["Create Devtask", "Start", "Open terminal", "Broken"]);
        let q = &h.ui.tickets.detail.as_ref().unwrap().quick;
        assert!(q[..3].iter().all(|b| b.disabled.is_none()) && q[3].disabled.is_some(), "ungültig = ausgegraut mit Grund");
        assert!(q[3].tip.contains(':') && !q[0].tip.is_empty(), "Hinweistexte: {:?}", q.iter().map(|b| &b.tip).collect::<Vec<_>>());
        assert!(h.rect_of(a).is_some() && h.rect_of(b).is_some() && h.rect_of(c).is_some(), "alle Knöpfe gezeichnet");
        // andere Typen zeigen nur ihre eigenen Aktionen
        h.open(epic);
        assert_eq!(h.labels(), ["Epic only"]);
        h.open(task);
        assert!(h.labels().is_empty(), "Typ ohne Aktionen: keine Leiste");
        // Seitenpanel und Seite zeigen dieselbe Leiste
        h.open(story);
        h.ui.tickets.mode = ViewMode::Panel;
        h.frames(2);
        assert_eq!(h.labels().len(), 4);
        assert!(h.rect_of(b).is_some());
        // Statuswechsel in den aktuellen Status entfällt
        h.db.conn.execute("UPDATE tickets SET status_id = ?1 WHERE id = ?2", [h.status_id("progress"), story]).unwrap();
        h.changed();
        assert_eq!(h.labels(), ["Create Devtask", "Open terminal", "Broken"]);
    });
}

#[test]
fn the_bar_follows_changes_made_in_the_editor_or_by_an_agent() {
    both(|h| {
        let a = h.add("userstory", "One", start_session());
        let b = h.add("userstory", "Two", start_session());
        let story = h.mk("userstory", "S");
        h.changed();
        h.open(story);
        assert_eq!(h.labels(), ["One", "Two"]);
        // umbenennen
        update_quick_action(&h.db.conn, a, &UpdateQuickAction { name: Some("Uno".into()), params: None }).unwrap();
        h.changed();
        assert_eq!(h.labels(), ["Uno", "Two"]);
        // Reihenfolge
        reorder_quick_actions(&h.db.conn, h.type_id("userstory"), &[b, a]).unwrap();
        h.changed();
        assert_eq!(h.labels(), ["Two", "Uno"]);
        // löschen
        delete_quick_action(&h.db.conn, a).unwrap();
        h.changed();
        assert_eq!(h.labels(), ["Two"]);
        // viele Aktionen: die ersten stehen in der Leiste, der Rest im Überlauf-Menü; es wird in beiden Ansichten gezeichnet
        for i in 0..8 {
            h.add("userstory", &format!("Extra {i}"), start_session());
        }
        h.changed();
        assert_eq!(h.labels().len(), 9);
        h.ui.tickets.mode = ViewMode::Panel;
        h.frames(3);
        h.ui.tickets.mode = ViewMode::Page;
        h.frames(3);
        assert!(h.rect_of(b).is_some());
    });
}

#[test]
fn clicking_a_status_action_moves_the_ticket_and_the_toast_undo_restores_it() {
    both(|h| {
        let start = h.add("userstory", "Start", to_status(h, "progress"));
        let story = h.mk("userstory", "Login");
        h.changed();
        h.open(story);
        let todo = h.status_id("todo");
        assert_eq!(h.status_of(story), Some(todo));
        h.click_button(start);
        assert_eq!(h.status_of(story), Some(h.status_id("progress")));
        assert!(!h.ui.quick.is_open(), "nichts einzugeben: kein Menü");
        let outs = h.ui.take_quick_out();
        let Some(Out::Toast { undo: Some(token), error: false, .. }) = outs.into_iter().next() else { panic!("Toast mit Rückgängig erwartet") };
        // die Ansicht lädt neu: der Knopf verschwindet (schon im Status)
        h.frames(3);
        assert!(h.labels().is_empty());
        let lang = h.lang;
        assert!(h.ui.quick_undo(&h.db.conn, token, lang).is_ok());
        assert_eq!(h.status_of(story), Some(todo));
        h.frames(3);
        assert_eq!(h.labels(), ["Start"], "nach dem Rückgängig ist der Knopf wieder da");
    });
}

#[test]
fn a_required_field_opens_the_form_instead_of_moving_and_esc_closes_it() {
    both(|h| {
        let start = h.add("userstory", "Start", to_status(h, "progress"));
        let story = h.mk("userstory", "Login");
        let ty = h.type_id("userstory");
        let fid = create_field(&h.db.conn, &NewField::new(ty, "Result", FieldKind::Text)).unwrap();
        let (todo, progress) = (h.status_id("todo"), h.status_id("progress"));
        let mut def = list_transitions(&h.db.conn, ty).unwrap().into_iter().find(|t| t.def.from_status_id == todo && t.def.to_status_id == progress).unwrap().def;
        def.required_fields = vec![fid];
        set_transition(&h.db.conn, ty, &def).unwrap();
        h.changed();
        h.open(story);
        h.click_button(start);
        assert!(h.ui.quick.is_open(), "das Pflichtfeld-Formular ist offen");
        assert_eq!(h.status_of(story), Some(todo), "noch nichts verschoben");
        h.esc();
        assert!(!h.ui.quick.is_open(), "Esc schließt das Menü");
        assert_eq!(h.status_of(story), Some(todo));
    });
}

#[test]
fn create_actions_create_a_ticket_or_ask_for_the_title_first() {
    both(|h| {
        let fixed = h.add("userstory", "Create Devtask", create_dev(h, Some("Fixed title")));
        let ask = h.add("userstory", "Create Devtask (ask)", create_dev(h, None));
        let story = h.mk("userstory", "Login");
        h.changed();
        h.open(story);
        let before = h.count();
        h.click_button(fixed);
        assert_eq!(h.count(), before + 1);
        let Some(Out::Toast { undo: Some(_), error: false, text }) = h.ui.take_quick_out().into_iter().next() else { panic!("Toast erwartet") };
        assert!(text.contains("PAN-"), "{text}");
        // ohne Titelvorgabe: erst die Eingabe, nichts wird angelegt, Esc bricht ab
        h.click_button(ask);
        assert!(h.ui.quick.is_open());
        assert_eq!(h.count(), before + 1);
        h.esc();
        assert!(!h.ui.quick.is_open());
        assert_eq!(h.count(), before + 1);
    });
}

#[test]
fn a_session_action_without_a_session_environment_behaves_like_in_the_quick_menu() {
    both(|h| {
        let term = h.add("userstory", "Open terminal", start_session());
        let story = h.mk("userstory", "Login");
        h.changed();
        h.open(story);
        let before = h.count();
        h.click_button(term);
        assert!(!h.ui.quick.is_open());
        let outs = h.ui.take_quick_out();
        assert!(outs.iter().any(|o| matches!(o, Out::Session(s) if s.ticket_id == story)), "die Anwendung bekommt die Sitzungsanfrage");
        assert_eq!(h.count(), before, "in der Datenbank ändert sich nichts");
    });
}

#[test]
fn an_invalid_action_cannot_be_run() {
    both(|h| {
        let broken = h.add("userstory", "Broken", start_session());
        h.db.conn.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [broken]).unwrap();
        let story = h.mk("userstory", "Login");
        h.changed();
        h.open(story);
        assert!(h.ui.tickets.detail.as_ref().unwrap().quick[0].disabled.is_some());
        h.click_button(broken);
        assert!(!h.ui.quick.is_open());
        assert!(h.ui.take_quick_out().is_empty(), "der ausgegraute Knopf löst nichts aus");
        // auch über den Auftrag (z. B. ein veralteter Knopf) bleibt es bei einer Fehlermeldung
        let cx = Cx { conn: &h.db.conn, db: &h.db, lang: h.lang };
        h.ui.ticket_act(&cx, Act::QuickAction(broken));
        h.frames(2);
        assert!(!h.ui.quick.is_open());
        assert!(matches!(h.ui.take_quick_out().as_slice(), [Out::Toast { error: true, undo: None, .. }]));
    });
}
