//! Unterreiter „Timer“: mehrere benannte Countdowns, Vorlagen („Tee 4 min“) als Knöpfe über der Liste, Details mit
//! Dauer, Start/Pause/Weiter/Neustart/Zurücksetzen, Klang und – mit eingeschalteter Zeiterfassung – Ticket.

use super::{big_time, local_hhmm, row, rows, sound_picker, ticket_field, BookAsk, Env, RingKind, Sub, TimeUi};
use crate::state::Lang;
use crate::style;
use crate::time::settings::TimeSettings;
use crate::time::{fmt_duration_short, fmt_hms, store, Preset, Timer, TimerState};
use eframe::egui::{self, RichText, Ui};

impl TimeUi {
    pub(super) fn timer_list(&mut self, ui: &mut Ui, env: &Env) {
        let lang = env.lang;
        let now = env.now_ms;
        // Vorlagen
        if !self.data.presets.is_empty() {
            ui.add_space(6.0);
            let mut start: Option<Preset> = None;
            let mut del: Option<i64> = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(lang.t("Vorlagen:", "Presets:")).color(style::dim(ui)));
                for p in &self.data.presets {
                    let r = ui.button(p.label()).on_hover_text(lang.t("Timer anlegen und starten (Rechtsklick: Vorlage löschen)", "Create and start a timer (right-click: delete preset)"));
                    if r.clicked() {
                        start = Some(p.clone());
                    }
                    r.context_menu(|ui| {
                        if ui.button(lang.t("Vorlage löschen", "Delete preset")).clicked() {
                            del = Some(p.id);
                            ui.close();
                        }
                    });
                }
            });
            if let Some(p) = start {
                self.new_timer(env, &p.name, p.duration_ms, true);
            }
            if let (Some(id), Some(conn)) = (del, env.conn)
                && self.db_result(store::delete(conn, store::Table::Presets, id)).is_some() {
                    self.data.presets.retain(|p| p.id != id);
                }
        }
        let idx: Vec<usize> = (0..self.data.timers.len()).filter(|&i| self.matches(1, &self.data.timers[i].name)).collect();
        if self.list_head(ui, 1, lang.t("Neuer Timer", "New timer"), self.data.timers.len(), lang) {
            self.new_timer(env, "", 5 * 60_000, false);
        }
        if self.data.timers.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new(lang.t("Noch keine Timer.", "No timers yet.")).color(style::dim(ui)));
            return;
        }
        let mut act: Option<(i64, &'static str)> = None;
        rows(ui, "time_timer_rows", idx.len(), |ui, i| {
            let t = &self.data.timers[idx[i]];
            let id = t.id;
            let ringing = self.is_ringing(RingKind::Timer(id));
            let sub = timer_sub(t, now, lang, ringing);
            let col = match t.state {
                TimerState::Running { .. } => Some(style::accent()),
                TimerState::Expired { .. } => Some(style::danger_soft()),
                _ => None,
            };
            let running = t.is_running();
            let clicked = row(ui, self.sel_timer == Some(id), &t.display_name(lang), &sub, &fmt_hms(t.remaining(now) + 999, false), col, 34.0, |ui| {
                let (icon, tip, a) = if running { (style::Icon::Stop, lang.t("Anhalten", "Pause"), "pause") } else { (style::Icon::Play, lang.t("Starten", "Start"), "start") };
                if style::icon_btn_small(ui, icon, tip).clicked() {
                    act = Some((id, a));
                }
            });
            if clicked {
                self.sel_timer = Some(id);
                self.confirm_delete = None;
            }
        });
        if let Some((id, a)) = act {
            self.timer_action(env, id, a, None);
        }
    }

    fn new_timer(&mut self, env: &Env, name: &str, ms: i64, start: bool) {
        let Some(conn) = env.conn else { return };
        let mut t = Timer::new(name, ms);
        if start {
            t.start(env.now_ms);
        }
        if self.db_result(store::save_timer(conn, &mut t)).is_some() {
            self.sel_timer = Some(t.id);
            self.data.timers.push(t);
        }
    }

    pub(super) fn edit_timer(&mut self, env: &Env, id: i64, f: impl FnOnce(&mut Timer)) {
        let Some(conn) = env.conn else { return };
        let Some(t) = self.data.timer_mut(id) else { return };
        let before = t.clone();
        f(t);
        if *t != before {
            let r = store::save_timer(conn, t);
            self.db_result(r);
        }
    }

    /// Start/Pause/Weiter/Neustart/Zurücksetzen aus der Oberfläche. Beim Zurücksetzen eines Timers mit Ticket fragt die
    /// Zeiterfassung nach (wenn eingeschaltet und mindestens eine Minute gelaufen).
    pub(super) fn timer_action(&mut self, env: &Env, id: i64, action: &str, settings: Option<&TimeSettings>) {
        let now = env.now_ms;
        let Some(t) = self.data.timer(id).cloned() else { return };
        if matches!(action, "reset" | "restart") && settings.is_some_and(|s| s.ticket_booking) && !t.ticket.is_empty() && !matches!(t.state, TimerState::Expired { .. } | TimerState::Idle) {
            self.ask_book_timer(&t, now, env.lang);
        }
        self.edit_timer(env, id, |t| match action {
            "start" => t.start(now),
            "pause" => t.pause(now),
            "restart" => t.restart(now),
            "reset" => t.reset(),
            _ => {}
        });
        if matches!(action, "reset" | "restart" | "start") {
            self.stop_ring(RingKind::Timer(id));
        }
    }

    /// Rückfrage zur Buchung der gelaufenen Zeit eines Timers (ab einer Minute).
    pub(super) fn ask_book_timer(&mut self, t: &Timer, now: i64, lang: Lang) {
        let worked = t.worked(now);
        if worked < 60_000 || t.ticket.is_empty() {
            return;
        }
        let end = match t.state {
            TimerState::Expired { at } => at,
            _ => now,
        };
        self.book.push(BookAsk { ticket: t.ticket.clone(), minutes: minutes_of(worked), start_ms: end - worked, comment: format!("{}: {}", lang.t("Timer", "Timer"), t.display_name(lang)), stopwatch: None });
    }

    pub(super) fn timer_detail(&mut self, ui: &mut Ui, env: &Env, settings: &TimeSettings) {
        let lang = env.lang;
        let now = env.now_ms;
        let Some(t) = self.sel_timer.and_then(|id| self.data.timer(id)).cloned() else {
            ui.label(RichText::new(lang.t("Wähle links einen Timer.", "Select a timer on the left.")).color(style::dim(ui)));
            return;
        };
        let id = t.id;
        ui.heading(t.display_name(lang));
        let col = match t.state {
            TimerState::Running { .. } => Some(style::accent()),
            TimerState::Expired { .. } => Some(style::danger_soft()),
            _ => None,
        };
        big_time(ui, &fmt_hms(t.remaining(now) + 999, false), col);
        ui.label(RichText::new(timer_sub(&t, now, lang, self.is_ringing(RingKind::Timer(id)))).color(style::dim(ui)));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mut act = None;
            match t.state {
                TimerState::Running { .. } => {
                    if ui.button(lang.t("Anhalten", "Pause")).clicked() {
                        act = Some("pause");
                    }
                }
                TimerState::Paused { .. } => {
                    if ui.button(lang.t("Weiter", "Resume")).clicked() {
                        act = Some("start");
                    }
                }
                _ => {
                    if ui.button(lang.t("Starten", "Start")).clicked() {
                        act = Some("start");
                    }
                }
            }
            if ui.button(lang.t("Neu starten", "Restart")).clicked() {
                act = Some("restart");
            }
            if ui.add_enabled(t.state != TimerState::Idle, egui::Button::new(lang.t("Zurücksetzen", "Reset"))).clicked() {
                act = Some("reset");
            }
            if let Some(a) = act {
                self.timer_action(env, id, a, Some(settings));
            }
        });
        ui.add_space(10.0);
        let mut e = t.clone();
        egui::Grid::new("time_timer_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(lang.t("Name", "Name"));
            ui.add(egui::TextEdit::singleline(&mut e.name).hint_text(lang.t("Timer", "Timer")).char_limit(crate::time::timer::MAX_NAME_CHARS).desired_width(220.0));
            ui.end_row();
            ui.label(lang.t("Dauer", "Duration"));
            ui.horizontal(|ui| {
                let total = e.duration_ms / 1000;
                let (mut h, mut m, mut s) = (total / 3600, (total / 60) % 60, total % 60);
                ui.add(egui::DragValue::new(&mut h).range(0..=99).suffix(" h"));
                ui.add(egui::DragValue::new(&mut m).range(0..=59).suffix(" min"));
                ui.add(egui::DragValue::new(&mut s).range(0..=59).suffix(" s"));
                let ms = (h * 3600 + m * 60 + s) * 1000;
                if ms != e.duration_ms && ms > 0 {
                    e.set_duration(ms);
                }
            });
            ui.end_row();
            ui.label(lang.t("Klang", "Sound"));
            if let Some(s) = sound_picker(ui, "time_timer_sound", &e.sound, crate::time::timer::DEFAULT_SOUND, &mut self.previewing, settings.volume_f(), lang) {
                e.sound = s;
            }
            ui.end_row();
        });
        if settings.ticket_booking {
            ui.add_space(6.0);
            if let Some(k) = ticket_field(ui, "time_timer_ticket", &e.ticket, env.conn, lang) {
                e.ticket = k;
            }
        }
        if e != t {
            self.edit_timer(env, id, |x| *x = e.clone());
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text(t.display_name(lang)).desired_width(150.0));
            if ui.button(lang.t("Als Vorlage speichern", "Save as preset")).clicked() {
                let name = if self.preset_name.trim().is_empty() { t.display_name(lang) } else { self.preset_name.trim().to_string() };
                let mut p = Preset { id: 0, name: crate::time::timer::clean_name(&name), duration_ms: e.duration_ms };
                if let Some(conn) = env.conn
                    && self.db_result(store::save_preset(conn, &mut p)).is_some() {
                        self.out.push(super::Out::Toast(format!("{} {}", lang.t("Vorlage gespeichert:", "Preset saved:"), p.label())));
                        self.data.presets.push(p);
                        self.preset_name.clear();
                    }
            }
        });
        if self.delete_button(ui, Sub::Timers, id, lang)
            && let Some(conn) = env.conn
                && self.db_result(store::delete(conn, store::Table::Timers, id)).is_some() {
                    self.stop_ring(RingKind::Timer(id));
                    self.data.timers.retain(|x| x.id != id);
                    self.sel_timer = None;
                }
    }
}

/// Unterzeile eines Timers: Zustand, Endzeit bzw. „abgelaufen um 14:32“.
pub(super) fn timer_sub(t: &Timer, now: i64, lang: Lang, ringing: bool) -> String {
    let dur = fmt_duration_short(t.duration_ms);
    let _ = now;
    match t.state {
        _ if ringing => format!("{dur} · {}", lang.t("klingelt", "ringing")),
        TimerState::Idle => format!("{dur} · {}", lang.t("bereit", "ready")),
        TimerState::Running { ends_at } => format!("{dur} · {} {}", lang.t("endet um", "ends at"), local_hhmm(ends_at)),
        TimerState::Paused { .. } => format!("{dur} · {}", lang.t("angehalten", "paused")),
        TimerState::Expired { at } => format!("{dur} · {} {}", lang.t("abgelaufen um", "expired at"), local_hhmm(at)),
    }
}

/// Minuten für die Buchung: gerundet, mindestens 1.
pub(super) fn minutes_of(ms: i64) -> u32 {
    (((ms + 30_000) / 60_000).max(1)).min(u32::MAX as i64) as u32
}
