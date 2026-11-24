//! Tests des Reiters „Zeit“ ohne Fenster (#164): jeder Unterreiter in beiden Sprachen, Anlegen per Klick, Takt mit
//! Klingeln/Schlummern/verpassten Weckern nach einem „Neustart“, Zeiterfassung an Tickets und 1000 Einträge je Art.

use super::app_glue::{book, handle_tick, reload};
use super::{Env, Out, RingKind, Sub, TimeUi};
use crate::db::Db;
use crate::state::Lang;
use crate::time::alarm::ALL_DAYS;
use crate::time::notify::{self, Cmd, Kind};
use crate::time::settings::TimeSettings;
use crate::time::{store, Alarm, Birthday, Stopwatch, Timer, WorldClock};
use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};

struct H {
    ctx: egui::Context,
    db: Db,
    ui: TimeUi,
    lang: Lang,
    time: f64,
    settings: TimeSettings,
    now_ms: i64,
    last: Option<egui::FullOutput>,
}

impl H {
    fn new(lang: Lang) -> H {
        let dir = std::env::temp_dir().join(format!("paneldeck-test-timeui-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::open(&dir, &dir.join("backups")).unwrap();
        H { ctx: egui::Context::default(), db, ui: TimeUi::default(), lang, time: 0.0, settings: TimeSettings::default(), now_ms: crate::time::now_ms(), last: None }
    }

    fn frame(&mut self, events: Vec<Event>) -> &egui::FullOutput {
        self.time += 0.1;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))), time: Some(self.time), events, ..Default::default() };
        let H { ctx, db, ui, lang, settings, now_ms, .. } = self;
        let env = Env { conn: Some(&db.conn), lang: *lang, now_ms: *now_ms };
        let out = ctx.run(input, |ctx| ui.show(ctx, &env, settings));
        self.last = Some(out);
        self.last.as_ref().unwrap()
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn texts(&self) -> Vec<String> {
        fn walk(s: &egui::Shape, acc: &mut Vec<(String, Rect)>) {
            match s {
                egui::Shape::Text(t) => acc.push((t.galley.text().to_string(), t.visual_bounding_rect())),
                egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, acc)),
                _ => {}
            }
        }
        let mut acc = Vec::new();
        if let Some(o) = &self.last {
            o.shapes.iter().for_each(|c| walk(&c.shape, &mut acc));
        }
        acc.into_iter().map(|t| t.0).collect()
    }

    fn text_pos(&self, text: &str) -> Option<Pos2> {
        fn walk(s: &egui::Shape, text: &str, acc: &mut Option<Pos2>) {
            match s {
                egui::Shape::Text(t) if acc.is_none() && t.galley.text() == text => *acc = Some(t.visual_bounding_rect().center()),
                egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, text, acc)),
                _ => {}
            }
        }
        let mut acc = None;
        self.last.as_ref()?.shapes.iter().for_each(|c| walk(&c.shape, text, &mut acc));
        acc
    }

    fn text_rect(&self, text: &str) -> Option<Rect> {
        fn walk(s: &egui::Shape, text: &str, acc: &mut Option<Rect>) {
            match s {
                egui::Shape::Text(t) if acc.is_none() && t.galley.text() == text => *acc = Some(t.visual_bounding_rect()),
                egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, text, acc)),
                _ => {}
            }
        }
        let mut acc = None;
        self.last.as_ref()?.shapes.iter().for_each(|c| walk(&c.shape, text, &mut acc));
        acc
    }

    fn has(&self, text: &str) -> bool {
        self.texts().iter().any(|t| t.contains(text))
    }

    fn click(&mut self, text: &str) {
        self.frames(2);
        let p = self.text_pos(text).unwrap_or_else(|| panic!("„{text}“ fehlt: {:?}", self.texts()));
        let btn = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![btn(true)]);
        self.frame(vec![btn(false)]);
        self.frames(1);
    }

    fn sub(&mut self, s: Sub) {
        self.ui.sub = s;
        self.frames(2);
    }

    fn conn(&self) -> &rusqlite::Connection {
        &self.db.conn
    }
}

fn both(f: impl Fn(&mut H)) {
    for lang in [Lang::En, Lang::De] {
        let mut h = H::new(lang);
        f(&mut h);
    }
}

#[test]
fn every_sub_tab_renders_in_both_languages_and_the_calendar_gets_the_main_area() {
    both(|h| {
        h.frames(2);
        for s in Sub::ALL {
            assert!(h.has(s.label(h.lang)), "Reiter {s:?} in {:?}", h.lang);
        }
        h.sub(Sub::Calendar);
        // Kalender: keine Liste des Zeit-Reiters (den Hauptbereich zeichnet `calendar_ui::show` über `App::time_view`)
        assert!(!h.has(&format!("+ {}", h.lang.t("Neuer Wecker", "New alarm"))) && h.has(Sub::Calendar.label(h.lang)));
        for (s, empty) in [
            (Sub::Alarms, h.lang.t("Noch keine Wecker", "No alarms yet")),
            (Sub::Timers, h.lang.t("Noch keine Timer", "No timers yet")),
            (Sub::Stopwatches, h.lang.t("Noch keine Stoppuhren", "No stopwatches yet")),
            (Sub::Clocks, h.lang.t("Noch keine Weltuhren", "No world clocks yet")),
            (Sub::Birthdays, h.lang.t("Noch keine Geburtstage", "No birthdays yet")),
        ] {
            h.sub(s);
            assert!(h.has(empty), "{s:?}: {:?}", h.texts());
        }
    });
}

#[test]
fn new_buttons_create_entries_and_show_their_details() {
    both(|h| {
        let lang = h.lang;
        h.sub(Sub::Alarms);
        h.click(&format!("+ {}", lang.t("Neuer Wecker", "New alarm")));
        assert_eq!(store::count(h.conn(), store::Table::Alarms).unwrap(), 1);
        assert!(h.ui.sel_alarm.is_some());
        h.frames(2);
        assert!(h.has("07:00") && h.has(lang.t("Schlummern", "Snooze")), "{:?}", h.texts());
        h.sub(Sub::Timers);
        h.click(&format!("+ {}", lang.t("Neuer Timer", "New timer")));
        assert_eq!(store::count(h.conn(), store::Table::Timers).unwrap(), 1);
        h.frames(2);
        h.click(lang.t("Starten", "Start"));
        assert!(h.ui.data.timers[0].is_running(), "Start in den Details");
        h.click(lang.t("Als Vorlage speichern", "Save as preset"));
        assert_eq!(h.ui.data.presets.len(), 1);
        assert!(h.ui.take_out().iter().any(|o| matches!(o, Out::Toast(t) if t.contains("5 min"))));
        h.sub(Sub::Stopwatches);
        h.click(&format!("+ {}", lang.t("Neue Stoppuhr", "New stopwatch")));
        h.frames(1);
        h.click(lang.t("Starten", "Start"));
        assert!(h.ui.data.stopwatches[0].running());
        h.sub(Sub::Clocks);
        h.click(&format!("+ {}", lang.t("Neue Weltuhr", "New clock")));
        assert_eq!(h.ui.data.clocks[0].zone, "UTC");
        h.ui.zone_query = "tokyo".into();
        h.frames(2);
        h.click("Asia/Tokyo");
        assert_eq!(h.ui.data.clocks[0].zone, "Asia/Tokyo");
        assert_eq!(store::load(h.conn()).unwrap().clocks[0].zone, "Asia/Tokyo", "gespeichert");
        h.sub(Sub::Birthdays);
        h.click(&format!("+ {}", lang.t("Neuer Geburtstag", "New birthday")));
        h.frames(2);
        assert!(h.has(lang.t("Heute! 🎂", "Today! 🎂")), "neuer Eintrag = heute: {:?}", h.texts());
    });
}

/// Geburtstags-Datum über den Puffer: ungültige Eingaben werden nicht angepasst und nicht gespeichert.
#[test]
fn birthday_date_is_saved_only_when_valid_and_never_clamped() {
    both(|h| {
        let lang = h.lang;
        let mut b = Birthday::new("Anna", 11, 20, Some(2000));
        store::save_birthday(h.conn(), &mut b).unwrap();
        h.ui.ensure_loaded(Some(&h.db.conn));
        h.ui.sel_birthday = Some(b.id);
        h.sub(Sub::Birthdays);
        // Tag 31 im November eingegeben (Monat folgt erst): bleibt 31, gespeichert bleibt der 20.11.
        h.ui.bday_edit.as_mut().unwrap().day = 31;
        h.frames(2);
        assert_eq!(h.ui.bday_edit.as_ref().unwrap().day, 31, "nicht angepasst");
        assert!(h.has(lang.t("Dieses Datum gibt es nicht", "This date does not exist")), "{:?}", h.texts());
        assert_eq!((store::load(h.conn()).unwrap().birthdays[0].day, store::load(h.conn()).unwrap().birthdays[0].month), (20, 11));
        // jetzt Januar: gültig, gespeichert
        h.ui.bday_edit.as_mut().unwrap().month = 1;
        h.frames(2);
        let s = store::load(h.conn()).unwrap().birthdays[0].clone();
        assert_eq!((s.day, s.month, s.year), (31, 1, Some(2000)));
        // 29.02. und das Jahr durch Nicht-Schaltjahre ziehen: der 29. bleibt, gespeichert wird nur in Schaltjahren
        {
            let e = h.ui.bday_edit.as_mut().unwrap();
            e.day = 29;
            e.month = 2;
            e.year = 2001;
        }
        h.frames(2);
        assert_eq!(store::load(h.conn()).unwrap().birthdays[0].day, 31, "2001 hat keinen 29.02.");
        h.ui.bday_edit.as_mut().unwrap().year = 2004;
        h.frames(2);
        let s = store::load(h.conn()).unwrap().birthdays[0].clone();
        assert_eq!((s.day, s.month, s.year), (29, 2, Some(2004)));
        assert_eq!(h.ui.bday_edit.as_ref().unwrap().day, 29);
    });
}

#[test]
fn delete_asks_first_and_then_removes_the_entry() {
    both(|h| {
        let lang = h.lang;
        let mut c = WorldClock::new("Büro", "Europe/Berlin");
        store::save_clock(h.conn(), &mut c).unwrap();
        h.ui.ensure_loaded(Some(&h.db.conn));
        h.ui.sel_clock = Some(c.id);
        h.sub(Sub::Clocks);
        h.click(lang.t("Löschen …", "Delete …"));
        assert!(h.has(lang.t("Wirklich löschen?", "Really delete?")));
        assert_eq!(store::count(h.conn(), store::Table::Clocks).unwrap(), 1);
        h.click(lang.t("Löschen", "Delete"));
        assert_eq!(store::count(h.conn(), store::Table::Clocks).unwrap(), 0);
        assert!(h.ui.data.clocks.is_empty() && h.ui.sel_clock.is_none());
    });
}

#[test]
fn options_window_changes_settings_and_reports_it() {
    both(|h| {
        let lang = h.lang;
        h.frames(1);
        h.click(lang.t("Optionen …", "Options …"));
        assert!(h.ui.options_open);
        assert!(h.has(lang.t("Termin-Erinnerungen", "Appointment reminders")));
        let _ = h.ui.take_out();
        // Schalter links neben der Beschriftung „Taskleistenknopf blinken lassen“
        let label = lang.t("Taskleistenknopf blinken lassen", "Flash the taskbar button");
        h.frames(1);
        let p = h.text_pos(label).unwrap();
        let r = h.text_rect(label).unwrap();
        let at = Pos2::new(r.left() - 26.0, p.y);
        let btn = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        h.frame(vec![Event::PointerMoved(at)]);
        h.frame(vec![btn(true)]);
        h.frame(vec![btn(false)]);
        assert!(h.settings.flash, "Blinken eingeschaltet");
        assert!(h.ui.take_out().contains(&Out::SettingsChanged));
    });
}

#[test]
fn rows_show_live_values_for_each_type() {
    both(|h| {
        let lang = h.lang;
        let now = h.now_ms;
        let mut a = Alarm::new("Aufstehen", 6, 45, ALL_DAYS, now / 1000);
        store::save_alarm(h.conn(), &mut a).unwrap();
        let mut t = Timer::new("Tee", 240_000);
        t.start(now - 60_000);
        store::save_timer(h.conn(), &mut t).unwrap();
        let mut s = Stopwatch::new("Lauf");
        s.start(now - 61_000);
        store::save_stopwatch(h.conn(), &mut s).unwrap();
        let mut b = Birthday::new("Anna", 2, 29, Some(2000));
        store::save_birthday(h.conn(), &mut b).unwrap();
        h.frames(1);
        h.sub(Sub::Alarms);
        assert!(h.has("Aufstehen") && h.has("06:45") && h.has(lang.t("Täglich", "Every day")));
        h.sub(Sub::Timers);
        assert!(h.has("Tee") && h.has("3:00"), "{:?}", h.texts());
        h.sub(Sub::Stopwatches);
        assert!(h.has("Lauf") && h.has("1:01."), "{:?}", h.texts());
        h.sub(Sub::Birthdays);
        assert!(h.has("Anna") && h.has(&b.date_text(lang)));
    });
}

/// Takt wie in der Anwendung: Klingeln mit Benachrichtigung und Schleifen-Schlüssel, Schlummern, Ausschalten; nach einem
/// „Neustart“ die verpassten Termine als Hinweis.
#[test]
fn tick_rings_snoozes_and_reports_missed_alarms_after_restart() {
    let _g = notify::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = H::new(Lang::En);
    let conn = &h.db.conn;
    let mut ui = TimeUi::default();
    let now = crate::time::now_ms();
    let s = TimeSettings::default();
    // ein Wecker, der in einer Sekunde klingelt
    let soon = chrono::Local::now() + chrono::Duration::seconds(61);
    use chrono::Timelike;
    let mut a = Alarm::new("Test", soon.hour() as u8, soon.minute() as u8, 0, now / 1000);
    store::save_alarm(conn, &mut a).unwrap();
    ui.ensure_loaded(Some(conn));
    let _ = notify::take();
    let fire_at = a.next_after(now / 1000, &chrono::Local).unwrap() * 1000;
    handle_tick(&mut ui, Some(conn), fire_at - 1000, Lang::En, &s, true);
    assert!(ui.ringing.is_empty());
    handle_tick(&mut ui, Some(conn), fire_at + 500, Lang::En, &s, false);
    assert!(ui.is_ringing(RingKind::Alarm(a.id)));
    let cmds = notify::take();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Fire(n) if n.kind == Kind::Alarm && n.ring.as_deref() == Some(&*format!("alarm:{}", a.id)))), "{cmds:?}");
    assert!(!store::load(conn).unwrap().alarms[0].enabled, "einmalig: danach aus (gespeichert)");
    // Schlummern über die Oberfläche
    let env = Env { conn: Some(conn), lang: Lang::En, now_ms: fire_at + 1000 };
    ui.edit_alarm(&env, a.id, |x| {
        x.snooze((fire_at + 1000) / 1000);
    });
    ui.stop_ring(RingKind::Alarm(a.id));
    assert!(notify::take().contains(&Cmd::Stop(format!("alarm:{}", a.id))));
    handle_tick(&mut ui, Some(conn), fire_at + 1000 + 300_500, Lang::En, &s, false);
    assert!(ui.is_ringing(RingKind::Alarm(a.id)), "Schlummern vorbei: klingelt wieder");
    // täglicher Wecker, Programm „beendet“, drei Tage später neu gestartet
    let mut d = Alarm::new("Daily", 7, 0, ALL_DAYS, now / 1000);
    store::save_alarm(conn, &mut d).unwrap();
    reload(&mut ui, conn);
    let later = now + 3 * 86_400_000;
    handle_tick(&mut ui, Some(conn), later, Lang::En, &s, true);
    let missed: Vec<&String> = ui.notes.iter().filter(|n| n.starts_with("Missed: alarm “Daily” 07:00 (")).collect();
    assert!(missed.len() >= 2, "{:?}", ui.notes);
    assert!(ui.take_out().iter().any(|o| matches!(o, Out::Toast(t) if t.starts_with("Missed:"))));
    // ein Timer, der ablief, während Paneldeck zu war
    let mut t = Timer::new("Tee", 240_000);
    t.start(now);
    store::save_timer(conn, &mut t).unwrap();
    reload(&mut ui, conn);
    handle_tick(&mut ui, Some(conn), now + 3_600_000, Lang::En, &s, true);
    assert!(ui.notes.iter().any(|n| n.starts_with("Timer “Tee” expired at ")), "{:?}", ui.notes);
    assert!(!ui.is_ringing(RingKind::Timer(t.id)));
    let _ = notify::take();
}

/// Erinnerungen an Kalendertermine (#164): über den Benachrichtiger mit `Kind::Reminder`, verpasste auch als Hinweis.
#[test]
fn calendar_reminders_go_through_the_notifier() {
    use crate::calendar::remind::Reminder;
    let _g = notify::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = notify::take();
    let mut ui = TimeUi::default();
    let start = chrono::NaiveDate::from_ymd_opt(2026, 11, 23).unwrap().and_hms_opt(9, 30, 0).unwrap();
    let rs = vec![
        Reminder { event_id: 1, title: "Standup".into(), start, fire_at: 0, minutes_before: 0, missed: false },
        Reminder { event_id: 2, title: "Review".into(), start, fire_at: 0, minutes_before: 15, missed: true },
    ];
    super::app_glue::calendar_reminders(&mut ui, &rs, Lang::En);
    let cmds = notify::take();
    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Cmd::Fire(n) if n.kind == Kind::Reminder && n.title == "Event" && n.body == "Standup – now"));
    assert!(matches!(&cmds[1], Cmd::Fire(n) if n.kind == Kind::Reminder && n.body.starts_with("Missed: Review")));
    assert_eq!(ui.notes, vec![rs[1].text(Lang::En)]);
}

/// Zeiterfassung: Stoppuhr mit Ticket → beim Stoppen die Rückfrage → Buchen legt einen Zeiteintrag an.
#[test]
fn stopwatch_with_ticket_asks_and_books_time() {
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::tickets::{create_ticket, NewTicket};
    let h = H::new(Lang::En);
    let conn = &h.db.conn;
    let mut np = NewProject::new("PAN", "Paneldeck", "#58A6E6");
    np.with_defaults = true;
    let pid = create_project(conn, &np).unwrap();
    let ty = crate::planning::types::list_types(conn, pid).unwrap()[0].id;
    let (tk, _) = create_ticket(conn, &NewTicket::new(pid, ty, "Arbeit"), &crate::planning::history::Source::ui()).unwrap();
    let mut ui = TimeUi::default();
    let now = crate::time::now_ms();
    let mut sw = Stopwatch::new("Projekt");
    sw.ticket = tk.key();
    sw.start(now - 47 * 60_000);
    store::save_stopwatch(conn, &mut sw).unwrap();
    ui.ensure_loaded(Some(conn));
    let env = Env { conn: Some(conn), lang: Lang::En, now_ms: now };
    let off = TimeSettings::default();
    ui.stopwatch_action(&env, sw.id, "stop", Some(&off));
    assert!(ui.book.is_empty(), "Zeiterfassung ist standardmäßig aus");
    let on = TimeSettings { ticket_booking: true, ..Default::default() };
    ui.stopwatch_action(&env, sw.id, "start", Some(&on));
    ui.stopwatch_action(&env, sw.id, "stop", Some(&on));
    assert_eq!(ui.book.len(), 1);
    let ask = ui.book[0].clone();
    assert_eq!((ask.ticket.as_str(), ask.minutes), ("PAN-1", 47));
    assert_eq!(book(Some(conn), &ask, Lang::En).unwrap(), "Booked 47 min on PAN-1");
    let entries = crate::planning::time_entries::entries_of(conn, tk.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].minutes, 47);
    assert!(entries[0].comment.contains("Projekt"));
    // unbekanntes Ticket: Fehlermeldung statt Buchung
    let mut bad = ask.clone();
    bad.ticket = "PAN-99".into();
    assert!(book(Some(conn), &bad, Lang::En).is_err());
}

/// 1000 Einträge je Art: jeder Unterreiter zeichnet ein Bild deutlich unter 100 ms (nur die sichtbaren Zeilen).
#[test]
fn a_thousand_entries_per_type_stay_smooth() {
    let mut h = H::new(Lang::En);
    let now = h.now_ms;
    store::in_tx(&h.db.conn, || {
        for i in 0..1000i64 {
            store::save_alarm(&h.db.conn, &mut Alarm::new(&format!("Alarm {i}"), (i % 24) as u8, (i % 60) as u8, ALL_DAYS, now / 1000))?;
            let mut t = Timer::new(&format!("Timer {i}"), 60_000 + i * 1000);
            if i % 2 == 0 {
                t.start(now);
            }
            store::save_timer(&h.db.conn, &mut t)?;
            let mut s = Stopwatch::new(&format!("Stopwatch {i}"));
            s.start(now);
            store::save_stopwatch(&h.db.conn, &mut s)?;
            store::save_clock(&h.db.conn, &mut WorldClock::new("", crate::time::worldclock::all_zones()[i as usize % 400].name()))?;
            store::save_birthday(&h.db.conn, &mut Birthday::new(&format!("Person {i}"), 1 + (i % 12) as u32, 1 + (i % 28) as u32, Some(1950 + (i % 70) as i32)))?;
        }
        Ok(())
    })
    .unwrap();
    h.frames(2);
    for s in [Sub::Alarms, Sub::Timers, Sub::Stopwatches, Sub::Clocks, Sub::Birthdays] {
        h.sub(s);
        let t0 = std::time::Instant::now();
        h.frames(5);
        let per = t0.elapsed() / 5;
        assert!(per < std::time::Duration::from_millis(100), "{s:?}: {per:?} je Bild");
    }
    // auch der Takt mit 1000 Einträgen je Art (Sperre vor dem Messen holen – das Warten auf andere Tests zählt nicht)
    let _g = notify::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t0 = std::time::Instant::now();
    handle_tick(&mut h.ui, Some(&h.db.conn), now + 1000, Lang::En, &TimeSettings::default(), true);
    let _ = notify::take();
    assert!(t0.elapsed() < std::time::Duration::from_millis(500), "{:?}", t0.elapsed());
}
