//! Unterreiter „Weltuhren“: Stadt/Zeitzone, Uhrzeit, Datum, Abstand zur lokalen Zeit; analog oder digital.

use super::{row, rows, Env, Sub, TimeUi};
use crate::state::Lang;
use crate::style;
use crate::time::settings::TimeSettings;
use crate::time::worldclock::{self, ClockView, WorldClock};
use crate::time::store;
use eframe::egui::{self, Color32, RichText, Sense, Stroke, Ui, Vec2};
use std::f32::consts::{FRAC_PI_2, TAU};

impl TimeUi {
    pub(super) fn clock_list(&mut self, ui: &mut Ui, env: &Env, settings: &TimeSettings) {
        let lang = env.lang;
        let idx: Vec<usize> = (0..self.data.clocks.len()).filter(|&i| self.matches(4, &format!("{} {}", self.data.clocks[i].label, self.data.clocks[i].zone))).collect();
        if self.list_head(ui, 4, lang.t("Neue Weltuhr", "New clock"), self.data.clocks.len(), lang)
            && let Some(conn) = env.conn {
                let mut c = WorldClock::new("", "UTC");
                if self.db_result(store::save_clock(conn, &mut c)).is_some() {
                    self.sel_clock = Some(c.id);
                    self.zone_query.clear();
                    self.data.clocks.push(c);
                }
            }
        // lokale Zeit als feste erste Zeile
        let local = chrono::Local::now().naive_local();
        ui.label(RichText::new(format!("{} {}  {}", lang.t("Hier:", "Here:"), local.format(if settings.clock_seconds { "%H:%M:%S" } else { "%H:%M" }), local.format("%Y-%m-%d"))).color(style::dim(ui)));
        if self.data.clocks.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new(lang.t("Noch keine Weltuhren.", "No world clocks yet.")).color(style::dim(ui)));
            return;
        }
        let secs = settings.clock_seconds;
        rows(ui, "time_clock_rows", idx.len(), |ui, i| {
            let c = &self.data.clocks[idx[i]];
            let id = c.id;
            let v = c.tz().and_then(|z| worldclock::view(z, env.now_ms, &chrono::Local));
            let (value, sub) = match &v {
                Some(v) => (v.local.format(if secs { "%H:%M:%S" } else { "%H:%M" }).to_string(), clock_sub(c, v, lang)),
                None => ("--:--".into(), lang.t("Unbekannte Zeitzone", "Unknown time zone").into()),
            };
            if row(ui, self.sel_clock == Some(id), &c.title(), &sub, &value, None, 0.0, |_| {}) {
                self.sel_clock = Some(id);
                self.zone_query.clear();
                self.confirm_delete = None;
            }
        });
    }

    pub(super) fn clock_detail(&mut self, ui: &mut Ui, env: &Env, settings: &TimeSettings) {
        let lang = env.lang;
        let Some(c) = self.sel_clock.and_then(|id| self.data.clock(id)).cloned() else {
            ui.label(RichText::new(lang.t("Wähle links eine Weltuhr.", "Select a clock on the left.")).color(style::dim(ui)));
            return;
        };
        let id = c.id;
        ui.heading(c.title());
        let v = c.tz().and_then(|z| worldclock::view(z, env.now_ms, &chrono::Local));
        if let Some(v) = &v {
            if c.analog {
                analog(ui, v, 150.0, settings.clock_seconds);
            } else {
                super::big_time(ui, &v.local.format(if settings.clock_seconds { "%H:%M:%S" } else { "%H:%M" }).to_string(), None);
            }
            ui.label(RichText::new(format!("{} · {} · {}", v.local.format("%A, %Y-%m-%d"), worldclock::fmt_utc(v.utc_offset_min), worldclock::fmt_offset(v.offset_to_local_min, lang))).color(style::dim(ui)));
        }
        ui.add_space(8.0);
        let mut e = c.clone();
        egui::Grid::new("time_clock_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(lang.t("Beschriftung", "Label"));
            ui.add(egui::TextEdit::singleline(&mut e.label).hint_text(worldclock::city_of(&e.zone)).char_limit(worldclock::MAX_LABEL_CHARS).desired_width(220.0));
            ui.end_row();
            ui.label(lang.t("Analog", "Analog"));
            style::toggle(ui, &mut e.analog);
            ui.end_row();
            ui.label(lang.t("Zeitzone", "Time zone"));
            ui.label(RichText::new(&e.zone).monospace());
            ui.end_row();
        });
        ui.add(egui::TextEdit::singleline(&mut self.zone_query).hint_text(lang.t("Zone suchen, z. B. Tokyo", "Search a zone, e.g. Tokyo")).desired_width(260.0));
        if !self.zone_query.trim().is_empty() {
            let hits = worldclock::search_zones(&self.zone_query, 50);
            egui::ScrollArea::vertical().id_salt("time_zone_hits").max_height(220.0).show(ui, |ui| {
                if hits.is_empty() {
                    ui.label(RichText::new(lang.t("Keine Zone gefunden.", "No zone found.")).color(style::dim(ui)));
                }
                for z in hits {
                    if ui.selectable_label(z.name() == e.zone, z.name()).clicked() {
                        e.zone = z.name().to_string();
                        self.zone_query.clear();
                    }
                }
            });
        }
        if e != c {
            e.label = worldclock::clean_label(&e.label);
            if let Some(conn) = env.conn
                && self.db_result(store::save_clock(conn, &mut e)).is_some()
                    && let Some(x) = self.data.clocks.iter_mut().find(|x| x.id == id) {
                        *x = e;
                    }
        }
        if self.delete_button(ui, Sub::Clocks, id, lang)
            && let Some(conn) = env.conn
                && self.db_result(store::delete(conn, store::Table::Clocks, id)).is_some() {
                    self.data.clocks.retain(|x| x.id != id);
                    self.sel_clock = None;
                }
    }
}

fn clock_sub(c: &WorldClock, v: &ClockView, lang: Lang) -> String {
    let zone = if c.label.trim().is_empty() { c.zone.clone() } else { format!("{} · {}", worldclock::city_of(&c.zone), c.zone) };
    format!("{zone} · {} · {}", v.local.format("%Y-%m-%d"), worldclock::fmt_offset(v.offset_to_local_min, lang))
}

/// Analoges Zifferblatt.
pub(super) fn analog(ui: &mut Ui, v: &ClockView, size: f32, seconds: bool) {
    use chrono::Timelike;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let r = size / 2.0 - 4.0;
    let fg = ui.visuals().text_color();
    p.circle_stroke(c, r, Stroke::new(2.0_f32, fg.gamma_multiply(0.7)));
    for i in 0..12 {
        let a = i as f32 / 12.0 * TAU - FRAC_PI_2;
        let (inner, w) = if i % 3 == 0 { (r * 0.82, 2.5_f32) } else { (r * 0.9, 1.5_f32) };
        p.line_segment([c + Vec2::angled(a) * inner, c + Vec2::angled(a) * (r - 2.0)], Stroke::new(w, fg.gamma_multiply(0.8)));
    }
    let t = v.local.time();
    let (h, m, s) = (t.hour() as f32, t.minute() as f32, t.second() as f32);
    let hand = |frac: f32, len: f32, w: f32, col: Color32| {
        let a = frac * TAU - FRAC_PI_2;
        p.line_segment([c, c + Vec2::angled(a) * len], Stroke::new(w, col));
    };
    hand(((h % 12.0) + m / 60.0) / 12.0, r * 0.5, 4.0, fg);
    hand((m + s / 60.0) / 60.0, r * 0.75, 2.5, fg);
    if seconds {
        hand(s / 60.0, r * 0.85, 1.2, style::accent());
    }
    p.circle_filled(c, 3.5, style::accent());
}
