//! Headless-egui-Tests des Kalenders (#164): Monat, Woche, Tag und Liste werden mit synthetischen Ereignissen gezeichnet
//! (Klicks, Doppelklicks, Tasten, Texteingabe) und das Ergebnis im Zustand bzw. in der Datenbank geprüft. Es wird kein
//! Fenster geöffnet und nichts an Maus oder Tastatur des Rechners angefasst. Feste Uhr (So 22.11.2026 10:00) und feste
//! Zeitzone (Europe/Berlin).

use super::dialog::{EventForm, RKind};
use super::{CalEnv, CalUi, Hit};
use crate::calendar::sources::Kind;
use crate::calendar::store::{self, EventInput, ViewKind};
use crate::calendar::zone::RuleZone;
use crate::calendar::{parse_when, Recurrence};
use crate::state::Lang;
use chrono::{NaiveDate, NaiveDateTime};
use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2};
use rusqlite::Connection;

fn dt(s: &str) -> NaiveDateTime {
    parse_when(s).unwrap().0
}
fn d(s: &str) -> NaiveDate {
    crate::calendar::parse_date(s).unwrap()
}

struct H {
    ctx: egui::Context,
    conn: Connection,
    cal: CalUi,
    now: NaiveDateTime,
    zone: RuleZone,
    lang: Lang,
    t: f64,
    size: Vec2,
}

impl H {
    fn new(lang: Lang) -> H {
        let mut h = H { ctx: egui::Context::default(), conn: crate::db::open_memory(), cal: CalUi::default(), now: dt("2026-11-22T10:00"), zone: RuleZone::berlin(), lang, t: 1.0, size: Vec2::new(1280.0, 900.0) };
        h.frames(2);
        h
    }

    fn event(&mut self, title: &str, start: &str, minutes: i64, cron: Option<&str>, all_day: bool) -> i64 {
        let mut i = EventInput::new(title, dt(start), minutes);
        i.all_day = all_day;
        i.recur = cron.map(|c| Recurrence { cron: c.into(), ..Default::default() });
        let id = store::create_event(&self.conn, i, None, "local").unwrap().id;
        self.cal.invalidate();
        id
    }

    /// Nach Änderungen an der Datenbank neu laden (wie nach MCP oder Dialog).
    fn reload(&mut self) {
        self.cal.invalidate();
        self.frames(1);
    }

    fn frame_with(&mut self, events: Vec<Event>, modifiers: Modifiers) {
        self.t += 0.05;
        let H { ctx, conn, cal, now, zone, lang, t, size } = self;
        let env = CalEnv { conn: Some(conn), lang: *lang, now: *now, zone };
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, *size)), time: Some(*t), events, modifiers, ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| cal.ui(ui, &env));
        });
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame_with(vec![], Modifiers::NONE);
        }
    }

    fn click(&mut self, pos: Pos2) {
        self.frame_with(vec![Event::PointerMoved(pos)], Modifiers::NONE);
        self.frame_with(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
        self.frame_with(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
        self.frames(1);
    }

    fn double_click(&mut self, pos: Pos2) {
        self.frame_with(vec![Event::PointerMoved(pos)], Modifiers::NONE);
        for _ in 0..2 {
            self.frame_with(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
            self.frame_with(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
        }
        self.frames(1);
    }

    fn key(&mut self, key: Key, m: Modifiers) {
        self.frame_with(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m }], m);
        self.frame_with(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m }], m);
        self.frames(1);
    }

    fn type_text(&mut self, s: &str) {
        self.frame_with(vec![Event::Text(s.into())], Modifiers::NONE);
        self.frames(1);
    }

    fn rect(&self, h: &Hit) -> Rect {
        self.cal.hits.iter().find(|(x, _)| x == h).map(|(_, r)| *r).unwrap_or_else(|| panic!("{h:?} nicht gezeichnet: {:?}", self.cal.hits.iter().map(|(h, _)| h).collect::<Vec<_>>()))
    }

    fn item_rects(&self, id: i64) -> Vec<Rect> {
        self.cal.hits.iter().filter(|(h, _)| matches!(h, Hit::Item(Kind::Event, i, _) if *i == id)).map(|(_, r)| *r).collect()
    }

    fn button(&mut self, name: &'static str) {
        let r = self.rect(&Hit::Button(name));
        self.click(r.center());
    }
}

#[test]
fn month_view_shows_events_bars_and_overflow() {
    let mut h = H::new(Lang::En);
    let one = h.event("Dentist", "2026-11-18T14:00", 60, None, false);
    let trip = h.event("Trip", "2026-11-16", 4 * 1440, None, true);
    let daily = h.event("Standup", "2026-11-02T09:00", 15, Some("0 9 * * MON-FRI"), false);
    for i in 0..8 {
        h.event(&format!("Busy {i}"), &format!("2026-11-25T{:02}:00", 8 + i), 30, None, false);
    }
    h.frames(2);
    assert_eq!(h.cal.view, ViewKind::Month, "Monat beim ersten Öffnen");
    assert_eq!(h.cal.title(&CalEnv { conn: None, lang: Lang::En, now: h.now, zone: &h.zone }), "November 2026");
    assert_eq!(h.item_rects(one).len(), 1);
    // die Reise ist ein Balken über vier Tage
    let bar = h.item_rects(trip);
    assert_eq!(bar.len(), 1);
    let cell = h.rect(&Hit::Day(d("2026-11-16")));
    assert!(bar[0].width() > cell.width() * 3.5, "Balken über 4 Tage");
    // Serie: nur im sichtbaren Zeitraum (6 Wochen ab Mo 26.10.), werktags
    let n = h.item_rects(daily).len();
    assert!((20..=30).contains(&n), "{n} Standups im Raster");
    // zu viele an einem Tag: „+n more“ – Klick öffnet den Tag
    let more = h.rect(&Hit::More(d("2026-11-25")));
    h.click(more.center());
    assert_eq!((h.cal.view, h.cal.anchor), (ViewKind::Day, Some(d("2026-11-25"))));
    assert_eq!(store::load_options(&h.conn).view, ViewKind::Day, "Ansicht wird gemerkt");
}

#[test]
fn clicking_selects_and_shows_details_double_click_edits() {
    let mut h = H::new(Lang::De);
    let id = h.event("Zahnarzt", "2026-11-18T14:00", 60, None, false);
    h.frames(1);
    let r = h.item_rects(id)[0];
    h.click(r.center());
    assert_eq!(h.cal.selected, Some((Kind::Event, id, dt("2026-11-18T14:00"))));
    h.frames(1);
    let edit = h.rect(&Hit::Button("edit"));
    assert!(edit.left() > h.size.x - super::DETAIL_W - 10.0, "Details rechts");
    let r = h.item_rects(id)[0];
    h.double_click(r.center());
    let f = h.cal.form.as_ref().expect("Doppelklick öffnet den Dialog");
    assert_eq!((f.id, f.title.as_str(), f.time.as_str()), (Some(id), "Zahnarzt", "14:00"));
}

#[test]
fn navigation_and_views() {
    let mut h = H::new(Lang::En);
    let env_title = |h: &H| h.cal.title(&CalEnv { conn: None, lang: h.lang, now: h.now, zone: &h.zone });
    h.button("next");
    assert_eq!(env_title(&h), "December 2026");
    h.button("prev");
    h.button("prev");
    assert_eq!(env_title(&h), "October 2026");
    h.button("today");
    assert_eq!(env_title(&h), "November 2026");
    h.button("view_week");
    assert_eq!(h.cal.view, ViewKind::Week);
    assert_eq!(env_title(&h), "Week 47 · Nov 16 – Nov 22, 2026");
    h.button("next");
    assert!(env_title(&h).starts_with("Week 48"));
    h.button("view_day");
    assert_eq!(env_title(&h), "Sunday, November 29, 2026", "Tag = Anker der Woche (heute + 7)");
    h.button("view_list");
    assert_eq!(h.cal.view, ViewKind::List);
    // Deutsch
    h.lang = Lang::De;
    h.button("view_day");
    assert_eq!(env_title(&h), "Sonntag, 29. November 2026");
}

#[test]
fn week_view_bars_on_top_blocks_side_by_side_and_double_click_creates() {
    let mut h = H::new(Lang::En);
    h.cal.view = ViewKind::Week;
    h.cal.anchor = Some(d("2026-11-23"));
    let a = h.event("A", "2026-11-24T09:00", 120, None, false);
    let b = h.event("B", "2026-11-24T10:00", 60, None, false);
    let trip = h.event("Conference", "2026-11-25", 3 * 1440, None, true);
    h.frames(3);
    let (ra, rb) = (h.item_rects(a)[0], h.item_rects(b)[0]);
    assert!(ra.right() <= rb.left() + 1.0, "überlappende Blöcke nebeneinander: {ra:?} {rb:?}");
    assert!((rb.top() - ra.top() - super::views::HOUR_H).abs() < 2.0, "B eine Stunde später");
    let bar = h.item_rects(trip)[0];
    let col = h.rect(&Hit::Day(d("2026-11-25")));
    assert!(bar.bottom() <= col.top() + 1.0 || bar.bottom() < ra.top(), "Balken oben über dem Raster");
    assert!(bar.width() > col.width() * 2.5, "über drei Tage");
    // Doppelklick in eine freie Stelle der Spalte Freitag: neuer Termin zu dieser Zeit
    let fri = h.rect(&Hit::Day(d("2026-11-27")));
    let visible_top = fri.top().max(bar.bottom() + 40.0);
    let y = visible_top + 30.0;
    h.double_click(Pos2::new(fri.center().x, y));
    let f = h.cal.form.as_ref().expect("Dialog offen");
    assert_eq!(f.date, "2026-11-27");
    let minutes = (((y - fri.top()) / super::views::HOUR_H * 60.0) as i64) / 30 * 30;
    assert_eq!(f.time, format!("{:02}:{:02}", minutes / 60, minutes % 60));
}

#[test]
fn new_event_dialog_with_builder_saves_a_recurring_event() {
    let mut h = H::new(Lang::En);
    h.button("new");
    assert!(h.cal.form.is_some());
    h.frames(1);
    h.type_text("Standup");
    assert_eq!(h.cal.form.as_ref().unwrap().title, "Standup", "Fokus im Titel");
    {
        let f = h.cal.form.as_mut().unwrap();
        f.date = "2026-11-23".into();
        f.time = "09:30".into();
        f.minutes = 15;
        f.rkind = RKind::Weekdays;
    }
    h.frames(2);
    assert_eq!(h.cal.form.as_ref().unwrap().cron, "30 9 * * MON-FRI", "Baukasten erzeugt den Ausdruck");
    let next = h.cal.form.as_ref().unwrap().preview(&CalEnv { conn: Some(&h.conn), lang: Lang::En, now: h.now, zone: &h.zone }).unwrap();
    assert_eq!(next.len(), 5);
    assert_eq!(next[4], dt("2026-11-27T09:30"));
    h.key(Key::Enter, Modifiers::CTRL);
    assert!(h.cal.form.is_none(), "gespeichert und geschlossen");
    let all = store::all_events(&h.conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].title.as_str(), all[0].recur.as_ref().unwrap().cron.as_str(), all[0].minutes), ("Standup", "30 9 * * MON-FRI", 15));
    assert_eq!(h.cal.selected.map(|s| s.1), Some(all[0].id), "neuer Termin ist ausgewählt");
}

#[test]
fn dialog_validation_keeps_it_open_and_esc_cancels() {
    let mut h = H::new(Lang::De);
    h.cal.form = Some(EventForm::new_at(dt("2026-11-23T09:00"), false));
    h.frames(1);
    h.type_text("Kaputt");
    {
        let f = h.cal.form.as_mut().unwrap();
        f.rkind = RKind::Custom;
        f.cron = "61 * * * *".into();
    }
    h.frames(1);
    h.key(Key::Enter, Modifiers::CTRL);
    let f = h.cal.form.as_ref().expect("bleibt offen");
    assert!(f.error.as_deref().unwrap_or("").contains("Minute"), "{:?}", f.error);
    assert!(store::all_events(&h.conn).unwrap().is_empty());
    // ODER-Fall speichert (mit Warnung im Dialog)
    h.cal.form.as_mut().unwrap().cron = "0 0 13 * FRI".into();
    h.frames(1);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.cal.form.is_none(), "Esc bricht ab");
    assert!(store::all_events(&h.conn).unwrap().is_empty());
}

#[test]
fn editing_recognises_builder_patterns_and_exceptions() {
    let mut e = crate::calendar::Event {
        id: 7,
        uid: "x".into(),
        title: "Board".into(),
        note: String::new(),
        color: String::new(),
        all_day: false,
        start: dt("2026-11-02T18:00"),
        minutes: 60,
        recur: Some(Recurrence { cron: "0 18 * * MON#1".into(), until: Some(d("2027-06-30")), count: None, except: vec![d("2027-01-04")] }),
        reminder: Some(30),
        project_id: None,
        origin: "local".into(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    let f = EventForm::from_event(&e);
    assert_eq!((f.rkind, f.nth, f.nth_wd, f.reminder), (RKind::MonthlyNth, 1, 1, Some(30)));
    assert_eq!((f.end, f.until.as_str(), f.except.len()), (super::dialog::EndKind::Until, "2027-06-30", 1));
    let i = f.input(Lang::En).unwrap();
    assert_eq!(i.recur.unwrap().cron, "0 18 * * MON#1");
    // fremder Ausdruck bleibt „eigener Ausdruck“
    e.recur.as_mut().unwrap().cron = "0 9-17 * * *".into();
    assert_eq!(EventForm::from_event(&e).rkind, RKind::Custom);
}

#[test]
fn list_view_detail_skip_and_delete() {
    let mut h = H::new(Lang::En);
    let id = h.event("Gym", "2026-11-23T07:00", 60, Some("0 7 * * MON,WED"), false);
    h.cal.view = ViewKind::List;
    h.frames(2);
    let rows = h.item_rects(id);
    assert!(rows.len() >= 10, "60 Tage, zweimal die Woche: {}", rows.len());
    h.click(rows[0].center());
    assert_eq!(h.cal.selected, Some((Kind::Event, id, dt("2026-11-23T07:00"))));
    h.frames(1);
    h.button("skip");
    let e = store::get_event(&h.conn, id).unwrap();
    assert_eq!(e.recur.unwrap().except, [d("2026-11-23")]);
    h.frames(2);
    let rows = h.item_rects(id);
    h.click(rows[0].center());
    h.frames(1);
    h.button("delete");
    assert_eq!(h.cal.confirm_delete, Some(id));
    h.frames(1);
    h.button("delete_confirm");
    assert!(store::get_event(&h.conn, id).is_err(), "gelöscht");
    assert!(h.cal.confirm_delete.is_none() && h.cal.selected.is_none());
}

#[test]
fn options_window_opens_and_sources_show_up() {
    let mut h = H::new(Lang::En);
    h.conn.execute("INSERT INTO todo_items (title, body, done, position, created_at, updated_at, due) VALUES ('Pay rent', '', 0, 1, 'x', 'x', '2026-11-30')", []).unwrap();
    h.reload();
    h.frames(1);
    assert!(h.cal.hits.iter().any(|(x, _)| matches!(x, Hit::Item(Kind::Todo, _, _))), "Fälligkeiten sind beim ersten Öffnen an");
    h.button("options");
    assert!(h.cal.options_open);
    h.frames(2);
    // abschalten wie im Fenster (Speichern + neu laden)
    let mut o = store::load_options(&h.conn);
    o.sources.due = false;
    store::save_options(&h.conn, &o).unwrap();
    h.cal = CalUi::default();
    h.frames(2);
    assert!(!h.cal.hits.iter().any(|(x, _)| matches!(x, Hit::Item(Kind::Todo, _, _))));
    h.key(Key::Escape, Modifiers::NONE);
}

#[test]
fn import_and_export_from_the_ui_report_back() {
    let mut h = H::new(Lang::En);
    let dir = std::env::temp_dir().join(format!("paneldeck-test-cal-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("in.ics");
    std::fs::write(&p, "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a@x\r\nDTSTART:20261124T100000\r\nSUMMARY:Imported\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:b@x\r\nSUMMARY:Broken\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").unwrap();
    let env = CalEnv { conn: Some(&h.conn), lang: Lang::En, now: h.now, zone: &h.zone };
    h.cal.import_path(&env, &p);
    let rep = h.cal.report.clone().expect("Bericht");
    assert_eq!((rep.imported, rep.skipped.len()), (1, 1));
    let out = dir.join("out.ics");
    let env = CalEnv { conn: Some(&h.conn), lang: Lang::En, now: h.now, zone: &h.zone };
    h.cal.export_path(&env, &out);
    assert_eq!(h.cal.msg.as_ref().map(|m| (m.0.as_str(), m.1)), Some(("1 events exported.", false)));
    assert!(std::fs::read_to_string(&out).unwrap().contains("SUMMARY:Imported"));
    h.frames(2);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.cal.report.is_none(), "Bericht mit Esc zu");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_thousand_events_render_smoothly() {
    let mut h = H::new(Lang::En);
    h.conn.execute_batch("BEGIN").unwrap();
    for i in 0..1000i64 {
        let start = dt("2026-10-01T08:00") + chrono::Duration::hours(i * 3);
        let cron = if i % 5 == 0 { Some("0 9 * * MON-FRI") } else { None };
        let mut inp = EventInput::new(&format!("E{i}"), start, 45);
        inp.recur = cron.map(|c| Recurrence { cron: c.into(), ..Default::default() });
        store::create_event(&h.conn, inp, None, "local").unwrap();
    }
    h.conn.execute_batch("COMMIT").unwrap();
    h.reload();
    for v in [ViewKind::Month, ViewKind::Week, ViewKind::Day, ViewKind::List] {
        h.cal.view = v;
        h.frames(1); // lädt
        let t = std::time::Instant::now();
        h.frames(5);
        let per = t.elapsed().as_millis() / 5;
        assert!(per < 250, "{v:?}: {per} ms je Bild");
    }
}

#[test]
fn no_database_shows_a_hint() {
    let ctx = egui::Context::default();
    let zone = RuleZone::utc();
    let mut cal = CalUi::default();
    let env = CalEnv { conn: None, lang: Lang::De, now: dt("2026-11-22T10:00"), zone: &zone };
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| cal.ui(ui, &env));
    });
    assert!(cal.hits.is_empty());
}

#[test]
fn demo_data_for_the_docs_pictures() {
    let mut h = H::new(Lang::En);
    super::demo_seed(&h.conn, h.now.date(), Lang::En);
    super::demo_seed(&h.conn, h.now.date(), Lang::En);
    assert_eq!(store::all_events(&h.conn).unwrap().len(), 6, "nur einmal angelegt");
    let env = CalEnv { conn: Some(&h.conn), lang: Lang::En, now: h.now, zone: &h.zone };
    h.cal.demo_prepare(&env, "dialog");
    assert!(h.cal.form.as_ref().is_some_and(|f| f.rkind == RKind::Weekdays));
    assert!(h.cal.selected.is_some());
    h.frames(3);
    assert!(h.cal.hits.iter().any(|(x, _)| matches!(x, Hit::Item(Kind::Todo, _, _))));
}
