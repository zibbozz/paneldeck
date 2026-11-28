//! Unterreiter „Stoppuhren“: mehrere Stoppuhren mit Runden (Start/Stopp/Runde/Zurücksetzen, Rundenliste) und – mit
//! eingeschalteter Zeiterfassung – Ticket (beim Stoppen die Frage „47 min auf PAN-12 buchen?“).

use super::timers::minutes_of;
use super::{big_time, row, rows, ticket_field, BookAsk, Env, Sub, TimeUi};
use crate::style;
use crate::time::settings::TimeSettings;
use crate::time::{fmt_hms, store, Stopwatch};
use eframe::egui::{self, RichText, Ui};

impl TimeUi {
    pub(super) fn stopwatch_list(&mut self, ui: &mut Ui, env: &Env) {
        let lang = env.lang;
        let now = env.now_ms;
        let idx: Vec<usize> = (0..self.data.stopwatches.len()).filter(|&i| self.matches(3, &self.data.stopwatches[i].name)).collect();
        if self.list_head(ui, 3, lang.t("Neue Stoppuhr", "New stopwatch"), self.data.stopwatches.len(), lang)
            && let Some(conn) = env.conn {
                let mut s = Stopwatch::new("");
                if self.db_result(store::save_stopwatch(conn, &mut s)).is_some() {
                    self.sel_stopwatch = Some(s.id);
                    self.data.stopwatches.push(s);
                }
            }
        if self.data.stopwatches.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new(lang.t("Noch keine Stoppuhren.", "No stopwatches yet.")).color(style::dim(ui)));
            return;
        }
        let mut act: Option<(i64, &'static str)> = None;
        rows(ui, "time_sw_rows", idx.len(), |ui, i| {
            let s = &self.data.stopwatches[idx[i]];
            let id = s.id;
            let running = s.running();
            let sub = format!(
                "{}{}",
                if running { lang.t("läuft", "running") } else { lang.t("gestoppt", "stopped") },
                if s.laps.is_empty() { String::new() } else { format!(" · {} {}", s.laps.len(), lang.t("Runden", "laps")) }
            );
            let col = running.then(style::accent);
            let clicked = row(ui, self.sel_stopwatch == Some(id), &s.display_name(lang), &sub, &fmt_hms(s.elapsed(now), true), col, 34.0, |ui| {
                let (icon, tip, a) = if running { (style::Icon::Stop, lang.t("Stoppen", "Stop"), "stop") } else { (style::Icon::Play, lang.t("Starten", "Start"), "start") };
                if style::icon_btn_small(ui, icon, tip).clicked() {
                    act = Some((id, a));
                }
            });
            if clicked {
                self.sel_stopwatch = Some(id);
                self.confirm_delete = None;
            }
        });
        if let Some((id, a)) = act {
            self.stopwatch_action(env, id, a, None);
        }
    }

    pub(super) fn edit_stopwatch(&mut self, env: &Env, id: i64, f: impl FnOnce(&mut Stopwatch)) {
        let Some(conn) = env.conn else { return };
        let Some(s) = self.data.stopwatch_mut(id) else { return };
        let before = s.clone();
        f(s);
        if *s != before {
            let r = store::save_stopwatch(conn, s);
            self.db_result(r);
        }
    }

    /// Start/Stopp/Runde/Zurücksetzen. Beim Stoppen mit Ticket (und eingeschalteter Zeiterfassung) die Rückfrage.
    pub(super) fn stopwatch_action(&mut self, env: &Env, id: i64, action: &str, settings: Option<&TimeSettings>) {
        let now = env.now_ms;
        let lang = env.lang;
        self.edit_stopwatch(env, id, |s| match action {
            "start" => s.start(now),
            "stop" => s.stop(now),
            "lap" => {
                s.lap(now);
            }
            "reset" => s.reset(),
            _ => {}
        });
        if action == "stop" && settings.is_some_and(|s| s.ticket_booking)
            && let Some(s) = self.data.stopwatch(id) {
                let rest = s.unbooked(now);
                if !s.ticket.is_empty() && rest >= 60_000 {
                    self.book.push(BookAsk {
                        ticket: s.ticket.clone(),
                        minutes: minutes_of(rest),
                        start_ms: now - rest,
                        comment: format!("{}: {}", lang.t("Stoppuhr", "Stopwatch"), s.display_name(lang)),
                        stopwatch: Some((id, s.elapsed(now))),
                    });
                }
            }
    }

    pub(super) fn stopwatch_detail(&mut self, ui: &mut Ui, env: &Env, settings: &TimeSettings) {
        let lang = env.lang;
        let now = env.now_ms;
        let Some(s) = self.sel_stopwatch.and_then(|id| self.data.stopwatch(id)).cloned() else {
            ui.label(RichText::new(lang.t("Wähle links eine Stoppuhr.", "Select a stopwatch on the left.")).color(style::dim(ui)));
            return;
        };
        let id = s.id;
        ui.heading(s.display_name(lang));
        big_time(ui, &fmt_hms(s.elapsed(now), true), s.running().then(style::accent));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mut act = None;
            if s.running() {
                if ui.button(lang.t("Stoppen", "Stop")).clicked() {
                    act = Some("stop");
                }
                if ui.button(lang.t("Runde", "Lap")).clicked() {
                    act = Some("lap");
                }
            } else if ui.button(lang.t("Starten", "Start")).clicked() {
                act = Some("start");
            }
            if ui.add_enabled(!s.running() && (s.elapsed(now) > 0 || !s.laps.is_empty()), egui::Button::new(lang.t("Zurücksetzen", "Reset"))).clicked() {
                act = Some("reset");
            }
            if let Some(a) = act {
                self.stopwatch_action(env, id, a, Some(settings));
            }
        });
        ui.add_space(8.0);
        let mut e = s.clone();
        ui.horizontal(|ui| {
            ui.label(lang.t("Name", "Name"));
            ui.add(egui::TextEdit::singleline(&mut e.name).hint_text(lang.t("Stoppuhr", "Stopwatch")).char_limit(crate::time::stopwatch::MAX_NAME_CHARS).desired_width(220.0));
        });
        if settings.ticket_booking {
            if let Some(k) = ticket_field(ui, "time_sw_ticket", &e.ticket, env.conn, lang) {
                e.ticket = k;
            }
            if e.booked_ms > 0 {
                ui.label(RichText::new(format!("{} {}", lang.t("Schon gebucht:", "Already booked:"), crate::time::fmt_duration_short(e.booked_ms))).small().color(style::dim(ui)));
            }
        }
        if e != s {
            self.edit_stopwatch(env, id, |x| *x = e.clone());
        }
        if !s.laps.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(lang.t("Runden", "Laps")).strong());
            let laps = s.lap_rows();
            egui::ScrollArea::vertical().id_salt("time_laps").max_height(260.0).show_rows(ui, 18.0, laps.len(), |ui, range| {
                egui::Grid::new("time_lap_grid").num_columns(3).spacing([20.0, 2.0]).show(ui, |ui| {
                    for (n, lap, total) in &laps[range] {
                        ui.label(format!("#{n}"));
                        ui.monospace(fmt_hms(*lap, true));
                        ui.monospace(RichText::new(fmt_hms(*total, true)).color(style::dim(ui)));
                        ui.end_row();
                    }
                });
            });
        }
        if self.delete_button(ui, Sub::Stopwatches, id, lang)
            && let Some(conn) = env.conn
                && self.db_result(store::delete(conn, store::Table::Stopwatches, id)).is_some() {
                    self.data.stopwatches.retain(|x| x.id != id);
                    self.sel_stopwatch = None;
                }
    }
}
