//! Anbindung des Reiters „Zeit“ an die Anwendung: Ansicht, Takt im Hauptdurchlauf (Wecker, Timer, Geburtstage, verpasste
//! Termine), Benachrichtigungen (`time::notify` → Klang, Toast, Blinken nach den Einstellungen), das Fenster „klingelt“,
//! die Rückfrage zur Zeiterfassung und die MCP-Werkzeuge.

use super::{Env, Out, Ring, RingKind, TimeUi};
use crate::app::App;
use crate::state::{self, Lang};
use crate::style;
use crate::time::engine::{self, Event};
use crate::time::notify::{self, Cmd, Kind, Note};
use crate::time::settings::TimeSettings;
use crate::time::sounds::SoundRef;
use crate::time::{agent, alarm, store};
use eframe::egui::{self, Align2, Id, RichText, Vec2};
use serde_json::Value;
use std::time::Duration;

/// Timer klingeln höchstens so lange (ms), danach verstummen sie (das Fenster bleibt).
const TIMER_RING_MS: i64 = 60_000;
/// Abstand der Abfrage der Kalender-Erinnerungen (ms).
const CAL_POLL_MS: i64 = 30_000;

/// Erinnerungen an Kalendertermine melden (`Kind::Reminder`); verpasste („Verpasst: …“) zusätzlich als Hinweis oben im
/// Reiter.
pub(super) fn calendar_reminders(t: &mut TimeUi, rs: &[crate::calendar::remind::Reminder], lang: Lang) {
    for r in rs {
        let text = r.text(lang);
        notify::fire(Kind::Reminder, lang.t("Termin", "Event"), &text, None);
        if r.missed {
            t.notes.push(text);
        }
    }
}

impl App {
    /// Reiter „Zeit“ zeichnen (jedes Fenster).
    pub(in crate::app) fn time_view(&mut self, ctx: &egui::Context) {
        let env = Env { conn: self.db.as_ref().map(|d| &d.conn), lang: self.settings.language, now_ms: crate::time::now_ms() };
        let mut s = self.settings.time.clone();
        self.time.show(ctx, &env, &mut s);
        self.settings.time = s;
        // Unterreiter „Kalender“: der Kalender (#164) mit eigener Ansicht und eigenem Detailbereich.
        if self.time.sub == super::Sub::Calendar {
            egui::CentralPanel::default().show(ctx, |ui| crate::app::calendar_ui::show(self, ui));
        }
        self.time_outs();
    }

    fn time_outs(&mut self) {
        for o in self.time.take_out() {
            match o {
                Out::Toast(t) => self.toast(t),
                Out::SettingsChanged => state::save_json(&self.dir.join("settings.json"), &self.settings),
                Out::Booked => {
                    self.plan.invalidate();
                    self.tasks.invalidate();
                }
            }
        }
    }

    /// Takt (nur Hauptdurchlauf): Laden, Prüfen, Benachrichtigen, Wecken für den nächsten Termin.
    pub(in crate::app) fn time_tick(&mut self, ctx: &egui::Context) {
        if !self.time.started && self.time.last_tick == 0 {
            let c = ctx.clone();
            notify::set_waker(Box::new(move || c.request_repaint()));
        }
        let conn = self.db.as_ref().map(|d| &d.conn);
        self.time.ensure_loaded(conn);
        let now = crate::time::now_ms();
        if self.time.loaded && !self.time.unavailable && (now - self.time.last_tick >= 250 || !self.time.started) {
            self.time.last_tick = now;
            let lang = self.settings.language;
            let settings = self.settings.time.clone();
            let conn = self.db.as_ref().map(|d| &d.conn);
            let first = !self.time.started;
            self.time.started = true;
            handle_tick(&mut self.time, conn, now, lang, &settings, first);
        }
        // Erinnerungen an Kalendertermine (#164; in den Kalender-Optionen standardmäßig aus): alle 30 s
        if self.time.loaded && now - self.time.last_cal_poll >= CAL_POLL_MS {
            self.time.last_cal_poll = now;
            if let Some(conn) = self.db.as_ref().map(|d| &d.conn) {
                let lang = self.settings.language;
                let rs = self.time.cal_poller.poll(conn, now.div_euclid(1000), &crate::calendar::zone::SystemZone);
                calendar_reminders(&mut self.time, &rs, lang);
            }
        }
        // Höchstdauer des Klingelns
        let alarm_limit = self.settings.time.ring_minutes as i64 * 60_000;
        for r in self.time.ringing.iter_mut() {
            let limit = match r.what {
                RingKind::Alarm(_) => alarm_limit,
                RingKind::Timer(_) => TIMER_RING_MS,
            };
            if r.sounding && now - r.since > limit {
                r.sounding = false;
                crate::platform::sound::stop(&r.what.key());
            }
        }
        self.time_notify_tick(ctx);
        let toasts: Vec<String> = self.time.out.iter().filter_map(|o| if let Out::Toast(t) = o { Some(t.clone()) } else { None }).collect();
        self.time.out.retain(|o| !matches!(o, Out::Toast(_)));
        for t in toasts {
            self.toast(t);
        }
        // nächster Termin: rechtzeitig wieder zeichnen (auch minimiert oder im Tray)
        let mut wait = Duration::from_secs(60);
        if let Some(next) = engine::next_wake(&self.time.data, now, &chrono::Local) {
            wait = wait.min(Duration::from_millis((next - now).clamp(20, 60_000) as u64));
        }
        if !self.time.ringing.is_empty() {
            wait = wait.min(Duration::from_secs(1));
        }
        ctx.request_repaint_after(wait);
    }

    /// Wartende Benachrichtigungen umsetzen (auch die anderer Teile, z. B. des Kalenders).
    fn time_notify_tick(&mut self, ctx: &egui::Context) {
        let cmds = notify::take();
        if cmds.is_empty() {
            return;
        }
        let s = self.settings.time.clone();
        let lang = self.settings.language;
        let (window_focused, _) = self.seen_focus();
        for c in cmds {
            match c {
                Cmd::Fire(n) => {
                    if s.sound(n.kind) {
                        play_note(&n, &s, lang);
                    }
                    if s.toast(n.kind) {
                        if window_focused {
                            let text = if n.body.is_empty() { n.title.clone() } else { format!("{}: {}", n.title, n.body) };
                            self.toast(text);
                        } else {
                            crate::platform::notify::toast(&n.title, &n.body, false);
                        }
                    }
                    if s.flash && !window_focused {
                        let urgency = if n.kind == Kind::Alarm { egui::UserAttentionType::Critical } else { egui::UserAttentionType::Informational };
                        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestUserAttention(urgency));
                    }
                }
                Cmd::Stop(key) => crate::platform::sound::stop(&key),
            }
        }
    }

    /// Fenster „klingelt“ und Rückfrage zur Zeiterfassung (im Fenster, das Hinweise zeigt).
    pub(in crate::app) fn time_overlay(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        if !self.time.ringing.is_empty() {
            self.time_ring_window(ctx, lang);
        }
        if !self.time.book.is_empty() {
            self.time_book_window(ctx, lang);
        }
    }

    fn time_ring_window(&mut self, ctx: &egui::Context, lang: Lang) {
        let now = crate::time::now_ms();
        let mut act: Option<(RingKind, &'static str)> = None;
        egui::Window::new("time_ring")
            .id(style::salted(Id::new("time_ring")))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 60.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_width(300.0);
                for r in &self.time.ringing {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&r.title).size(18.0).strong());
                    });
                    if !r.body.is_empty() {
                        ui.label(&r.body);
                    }
                    ui.horizontal(|ui| match r.what {
                        RingKind::Alarm(id) => {
                            let a = self.time.data.alarm(id);
                            let can = a.is_some_and(|a| a.can_snooze());
                            let mins = a.map(|a| a.snooze_minutes).unwrap_or(5);
                            let left = a.map(|a| a.snooze_max.saturating_sub(a.snooze_count)).unwrap_or(0);
                            let label = format!("{} {mins} min", lang.t("Schlummern", "Snooze"));
                            if ui.add_enabled(can, egui::Button::new(label)).on_hover_text(format!("{} {left}", lang.t("noch möglich:", "left:"))).clicked() {
                                act = Some((r.what, "snooze"));
                            }
                            if ui.button(lang.t("Aus", "Dismiss")).clicked() {
                                act = Some((r.what, "dismiss"));
                            }
                        }
                        RingKind::Timer(_) => {
                            if ui.button(lang.t("Aus", "Dismiss")).clicked() {
                                act = Some((r.what, "dismiss"));
                            }
                            if ui.button(lang.t("Neu starten", "Restart")).clicked() {
                                act = Some((r.what, "restart"));
                            }
                        }
                    });
                    ui.separator();
                }
                if ui.small_button(lang.t("Zum Reiter „Zeit“", "Open the Time tab")).clicked() {
                    act = act.or(Some((RingKind::Alarm(-1), "open")));
                }
            });
        let Some((k, a)) = act else { return };
        if a == "open" {
            self.set_mode(crate::app::titlebar::Mode::Time);
            return;
        }
        let conn = self.db.as_ref().map(|d| &d.conn);
        let env = Env { conn, lang, now_ms: now };
        match (k, a) {
            (RingKind::Alarm(id), "snooze") => {
                self.time.edit_alarm(&env, id, |x| {
                    x.snooze(now.div_euclid(1000));
                });
                self.time.stop_ring(k);
            }
            (RingKind::Alarm(id), _) => {
                self.time.edit_alarm(&env, id, |x| x.dismiss());
                self.time.stop_ring(k);
            }
            (RingKind::Timer(id), "restart") => {
                let s = self.settings.time.clone();
                self.time.timer_action(&env, id, "restart", Some(&s));
            }
            (RingKind::Timer(_), _) => self.time.stop_ring(k),
        }
    }

    fn time_book_window(&mut self, ctx: &egui::Context, lang: Lang) {
        let Some(ask) = self.time.book.first().cloned() else { return };
        let mut choice: Option<bool> = None;
        egui::Window::new(lang.t("Zeit buchen", "Book time"))
            .id(style::salted(Id::new("time_book")))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let q = if lang == Lang::De { format!("{} min auf {} buchen?", ask.minutes, ask.ticket) } else { format!("Book {} min on {}?", ask.minutes, ask.ticket) };
                ui.label(RichText::new(q).size(15.0));
                ui.label(RichText::new(&ask.comment).small().color(style::dim(ui)));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(lang.t("Buchen", "Book")).clicked() {
                        choice = Some(true);
                    }
                    if ui.button(lang.t("Nicht buchen", "Not now")).clicked() {
                        choice = Some(false);
                    }
                });
            });
        let Some(yes) = choice else { return };
        self.time.book.remove(0);
        if !yes {
            return;
        }
        let conn = self.db.as_ref().map(|d| &d.conn);
        match book(conn, &ask, lang) {
            Ok(text) => {
                if let Some((id, upto)) = ask.stopwatch {
                    let now = crate::time::now_ms();
                    let env = Env { conn, lang, now_ms: now };
                    self.time.edit_stopwatch(&env, id, |s| s.mark_booked(upto, now));
                }
                self.time.out.push(Out::Booked);
                self.toast(text);
            }
            Err(e) => self.toast(e),
        }
        self.time_outs();
    }

    /// MCP-Werkzeuge des Reiters (`time::agent`).
    pub(in crate::app) fn mcp_time(&mut self, tool: &str, args: &Value, caller: Option<u64>) -> Result<Value, (String, String)> {
        let conn = self.db.as_ref().map(|d| &d.conn);
        self.time.ensure_loaded(conn);
        if conn.is_none() || self.time.unavailable {
            return Err(("db_unavailable".into(), state::lang().t("Die Datenbank ist nicht verfügbar.", "The database is not available.").into()));
        }
        let lang = state::lang();
        let ctx = agent::Ctx {
            conn,
            now_ms: crate::time::now_ms(),
            tz: chrono::Local,
            lang,
            ringing_alarms: self.time.ringing.iter().filter_map(|r| if let RingKind::Alarm(id) = r.what { Some(id) } else { None }).collect(),
            ringing_timers: self.time.ringing.iter().filter_map(|r| if let RingKind::Timer(id) = r.what { Some(id) } else { None }).collect(),
        };
        let done = agent::call(&ctx, &mut self.time.data, tool, args)?;
        for id in done.silence_alarms {
            self.time.stop_ring(RingKind::Alarm(id));
        }
        for id in done.silence_timers {
            self.time.stop_ring(RingKind::Timer(id));
        }
        if let Some(c) = done.change {
            let who = caller.and_then(|id| self.entries.get(&id).map(|e| e.display_name().to_string())).filter(|n| !n.trim().is_empty());
            let label = match who {
                Some(n) => format!("{} ({n})", lang.t("Agent", "Agent")),
                None => lang.t("Agent", "Agent").to_string(),
            };
            self.toast(crate::mcp::short(&format!("{label}: {c}"), 140));
        }
        Ok(done.value)
    }
}

/// Ein Takt samt Umsetzung der Ereignisse (ohne App, damit testbar). `first` = erster Takt nach dem Start (Verpasstes
/// gesammelt melden).
pub(super) fn handle_tick(t: &mut TimeUi, conn: Option<&rusqlite::Connection>, now: i64, lang: Lang, settings: &TimeSettings, first: bool) {
    let (events, changed) = engine::tick(&mut t.data, now, &chrono::Local);
    if let Some(conn) = conn
        && !changed.is_empty() {
            let data = &mut t.data;
            let r = store::in_tx(conn, || {
                for id in &changed.alarms {
                    if let Some(a) = data.alarm_mut(*id) {
                        store::save_alarm(conn, a)?;
                    }
                }
                for id in &changed.timers {
                    if let Some(x) = data.timer_mut(*id) {
                        store::save_timer(conn, x)?;
                    }
                }
                for id in &changed.birthdays {
                    if let Some(b) = data.birthdays.iter_mut().find(|b| b.id == *id) {
                        store::save_birthday(conn, b)?;
                    }
                }
                Ok(())
            });
            if let Err(e) = r {
                t.error = Some(e.to_string());
            }
        }
    let mut missed: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            Event::AlarmRing { id, at, snoozed } => {
                let Some(a) = t.data.alarm(id).cloned() else { continue };
                let title = format!("⏰ {}", a.display_name(lang));
                let mut body = super::local_hhmm(at * 1000);
                if snoozed {
                    body = format!("{body} · {}", lang.t("Schlummern vorbei", "snooze over"));
                }
                start_ring(t, RingKind::Alarm(id), now, title, body, Kind::Alarm, &a.sound);
            }
            Event::AlarmMissed { id, at, snoozed } => {
                if let Some(a) = t.data.alarm(id) {
                    let mut text = alarm::missed_text(a, at, &chrono::Local, lang);
                    if snoozed {
                        text = format!("{text} – {}", lang.t("Schlummern vorbei", "snooze over"));
                    }
                    missed.push(text);
                }
            }
            Event::TimerDone { id, at } => {
                let Some(x) = t.data.timer(id).cloned() else { continue };
                let title = format!("⏱ {}", x.display_name(lang));
                let body = format!("{} ({})", lang.t("Zeit ist um", "Time is up"), crate::time::fmt_duration_short(x.duration_ms));
                let _ = at;
                start_ring(t, RingKind::Timer(id), now, title, body, Kind::Timer, &x.sound);
                if settings.ticket_booking {
                    t.ask_book_timer(&x, now, lang);
                }
            }
            Event::TimerExpiredAway { id, at } => {
                let Some(x) = t.data.timer(id).cloned() else { continue };
                let text = if lang == Lang::De {
                    format!("Timer {} abgelaufen um {}", lang.q(&x.display_name(lang)), super::local_hhmm(at))
                } else {
                    format!("Timer {} expired at {}", lang.q(&x.display_name(lang)), super::local_hhmm(at))
                };
                t.notes.push(text.clone());
                t.out.push(Out::Toast(text));
                if settings.ticket_booking {
                    t.ask_book_timer(&x, now, lang);
                }
            }
            Event::Birthday { id } => {
                let Some(b) = t.data.birthday(id) else { continue };
                let today = super::birthdays::today(now);
                let body = match b.age_on(today) {
                    Some(age) => format!("{} · {age}", b.describe(today, lang)),
                    None => b.describe(today, lang),
                };
                notify::fire(Kind::Birthday, &format!("🎂 {}", b.name), &body, None);
            }
        }
    }
    if !missed.is_empty() {
        let n = missed.len();
        let toast = if n == 1 { missed[0].clone() } else { format!("{} (+{})", missed[n - 1], n - 1) };
        t.notes.extend(missed);
        if t.notes.len() > 100 {
            let cut = t.notes.len() - 100;
            t.notes.drain(..cut);
        }
        t.out.push(Out::Toast(toast));
    }
    let _ = first;
}

fn start_ring(t: &mut TimeUi, what: RingKind, now: i64, title: String, body: String, kind: Kind, sound: &str) {
    t.ringing.retain(|r| r.what != what);
    t.ringing.push(Ring { what, since: now, title: title.clone(), body: body.clone(), sounding: true });
    notify::fire_note(Note::new(kind, &title, &body).sound(Some(sound)).ring(&what.key()));
}

/// Standardklang einer Art.
pub(super) fn default_sound(k: Kind) -> &'static str {
    match k {
        Kind::Alarm => alarm::DEFAULT_SOUND,
        Kind::Timer => crate::time::timer::DEFAULT_SOUND,
        Kind::Reminder => "builtin:soft",
        Kind::Birthday => "builtin:chime",
    }
}

fn play_note(n: &Note, s: &TimeSettings, lang: Lang) {
    let fb = default_sound(n.kind);
    let r = SoundRef::parse(n.sound.as_deref().unwrap_or(fb), fb);
    // eigene Datei fehlt oder ist ungeeignet: Standardklang
    let clip = r.clip(lang).or_else(|_| SoundRef::parse(fb, fb).clip(lang));
    if let Ok(c) = clip {
        let key = n.ring.clone().unwrap_or_else(|| format!("note:{:?}", n.kind));
        crate::platform::sound::play(&key, c, n.ring.is_some(), s.volume_f());
    }
}

/// Bucht die Zeit einer Rückfrage über die Zeiterfassung der Tickets.
pub(super) fn book(conn: Option<&rusqlite::Connection>, ask: &super::BookAsk, lang: Lang) -> Result<String, String> {
    use crate::planning::time_entries::{log_time, StartInput};
    use chrono::TimeZone;
    let conn = conn.ok_or_else(|| lang.t("Die Datenbank ist nicht verfügbar.", "The database is not available.").to_string())?;
    let t = crate::planning::tickets::ticket_by_key(conn, &ask.ticket).map_err(|e| e.message(lang))?;
    let start = chrono::Local.timestamp_millis_opt(ask.start_ms).single().map(|d| d.naive_local()).ok_or_else(|| "start".to_string())?;
    let now = chrono::Local::now().naive_local();
    log_time(conn, t.id, &format!("{}m", ask.minutes), Some(StartInput::DateTime(start)), None, &ask.comment, now, &crate::planning::history::Source::ui()).map_err(|e| e.message(lang))?;
    Ok(if lang == Lang::De { format!("{} min auf {} gebucht", ask.minutes, t.key()) } else { format!("Booked {} min on {}", ask.minutes, t.key()) })
}

/// Für Tests: alle Daten neu laden (wie ein Neustart).
#[cfg(test)]
pub(super) fn reload(t: &mut TimeUi, conn: &rusqlite::Connection) {
    t.data = store::load(conn).unwrap_or_default();
    t.ringing.clear();
    t.started = false;
}
