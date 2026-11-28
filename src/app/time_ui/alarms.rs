//! Unterreiter „Wecker“: Liste (nach Uhrzeit) und Details (Uhrzeit, Wochentage oder einmalig, Name, Klang, Schlummern).

use super::{row, rows, sound_picker, Env, RingKind, Sub, TimeUi};
use crate::state::Lang;
use crate::style;
use crate::time::alarm::{self, Alarm, ALL_DAYS, WEEKEND, WORKDAYS};
use crate::time::store;
use eframe::egui::{self, RichText, Ui};

impl TimeUi {
    pub(super) fn alarm_list(&mut self, ui: &mut Ui, env: &Env) {
        let lang = env.lang;
        let idx: Vec<usize> = (0..self.data.alarms.len()).filter(|&i| self.matches(0, &self.data.alarms[i].name)).collect();
        if self.list_head(ui, 0, lang.t("Neuer Wecker", "New alarm"), self.data.alarms.len(), lang) {
            self.new_alarm(env);
        }
        if self.data.alarms.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new(lang.t("Noch keine Wecker. „+ Neuer Wecker“ legt einen an.", "No alarms yet. “+ New alarm” adds one.")).color(style::dim(ui)));
            return;
        }
        let now_s = env.now_ms.div_euclid(1000);
        let mut toggle: Option<(i64, bool)> = None;
        rows(ui, "time_alarm_rows", idx.len(), |ui, i| {
            let a = &self.data.alarms[idx[i]];
            let ringing = self.is_ringing(RingKind::Alarm(a.id));
            let sub = alarm_sub(a, now_s, lang, ringing);
            let mut on = a.enabled;
            let id = a.id;
            let dim_val = (!a.enabled).then(|| style::dim(ui));
            let clicked = row(ui, self.sel_alarm == Some(id), &a.display_name(lang), &sub, &a.time_text(), dim_val, 44.0, |ui| {
                if style::toggle(ui, &mut on).on_hover_text(lang.t("Ein/aus", "On/off")).changed() {
                    toggle = Some((id, on));
                }
            });
            if clicked {
                self.sel_alarm = Some(id);
                self.confirm_delete = None;
            }
        });
        if let Some((id, on)) = toggle {
            self.edit_alarm(env, id, |a| a.set_enabled(on, now_s));
            if !on {
                self.stop_ring(RingKind::Alarm(id));
            }
        }
    }

    fn new_alarm(&mut self, env: &Env) {
        let Some(conn) = env.conn else { return };
        let mut a = Alarm::new("", 7, 0, 0, env.now_ms.div_euclid(1000));
        if self.db_result(store::save_alarm(conn, &mut a)).is_some() {
            self.sel_alarm = Some(a.id);
            self.data.alarms.push(a);
            sort_alarms(&mut self.data.alarms);
        }
    }

    /// Ändert einen Wecker und speichert ihn.
    pub(super) fn edit_alarm(&mut self, env: &Env, id: i64, f: impl FnOnce(&mut Alarm)) {
        let Some(conn) = env.conn else { return };
        let Some(a) = self.data.alarm_mut(id) else { return };
        let before = a.clone();
        f(a);
        if *a == before {
            return;
        }
        let r = store::save_alarm(conn, a);
        self.db_result(r);
        sort_alarms(&mut self.data.alarms);
    }

    pub(super) fn alarm_detail(&mut self, ui: &mut Ui, env: &Env, settings: &crate::time::settings::TimeSettings) {
        let lang = env.lang;
        let Some(a) = self.sel_alarm.and_then(|id| self.data.alarm(id)).cloned() else {
            ui.label(RichText::new(lang.t("Wähle links einen Wecker.", "Select an alarm on the left.")).color(style::dim(ui)));
            return;
        };
        let now_s = env.now_ms.div_euclid(1000);
        let id = a.id;
        let mut e = a.clone();
        ui.heading(a.display_name(lang));
        ui.add_space(6.0);
        egui::Grid::new("time_alarm_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(lang.t("Name", "Name"));
            ui.add(egui::TextEdit::singleline(&mut e.name).hint_text(lang.t("Wecker", "Alarm")).char_limit(alarm::MAX_NAME_CHARS).desired_width(220.0));
            ui.end_row();
            ui.label(lang.t("Uhrzeit", "Time"));
            ui.horizontal(|ui| {
                let mut h = e.hour as u32;
                let mut m = e.minute as u32;
                ui.add(egui::DragValue::new(&mut h).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
                ui.label(":");
                ui.add(egui::DragValue::new(&mut m).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
                e.hour = h as u8;
                e.minute = m as u8;
            });
            ui.end_row();
            ui.label(lang.t("Wiederholen", "Repeat"));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    for d in 0..7u32 {
                        let on = e.days & (1 << d) != 0;
                        if ui.selectable_label(on, crate::time::weekday_short(d, lang)).clicked() {
                            e.days ^= 1 << d;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    for (bits, label) in [(0u8, lang.t("Einmalig", "Once")), (WORKDAYS, lang.t("Mo–Fr", "Mon–Fri")), (WEEKEND, lang.t("Sa, So", "Sat, Sun")), (ALL_DAYS, lang.t("Täglich", "Every day"))] {
                        if ui.small_button(label).clicked() {
                            e.days = bits;
                        }
                    }
                });
            });
            ui.end_row();
            ui.label(lang.t("Ein", "On"));
            let mut on = e.enabled;
            if style::toggle(ui, &mut on).changed() {
                e.set_enabled(on, now_s);
            }
            ui.end_row();
            ui.label(lang.t("Klang", "Sound"));
            let vol = settings.volume_f();
            if let Some(s) = sound_picker(ui, "time_alarm_sound", &e.sound, alarm::DEFAULT_SOUND, &mut self.previewing, vol, lang) {
                e.sound = s;
            }
            ui.end_row();
            ui.label(lang.t("Schlummern", "Snooze"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut e.snooze_minutes).range(alarm::SNOOZE_MINUTES_RANGE.0..=alarm::SNOOZE_MINUTES_RANGE.1).suffix(" min"));
                ui.label(lang.t("höchstens", "at most"));
                ui.add(egui::DragValue::new(&mut e.snooze_max).range(alarm::SNOOZE_MAX_RANGE.0..=alarm::SNOOZE_MAX_RANGE.1).suffix(lang.t("×", "×")));
            });
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.label(RichText::new(alarm_sub(&e, now_s, lang, self.is_ringing(RingKind::Alarm(id)))).color(style::dim(ui)));
        if e != a {
            let rearm = (e.hour, e.minute, e.days) != (a.hour, a.minute, a.days);
            self.edit_alarm(env, id, |x| {
                *x = e;
                if rearm {
                    x.rearm(now_s);
                }
            });
            if !self.data.alarm(id).is_some_and(|x| x.enabled) {
                self.stop_ring(RingKind::Alarm(id));
            }
        }
        if self.delete_button(ui, Sub::Alarms, id, lang)
            && let Some(conn) = env.conn
                && self.db_result(store::delete(conn, store::Table::Alarms, id)).is_some() {
                    self.stop_ring(RingKind::Alarm(id));
                    self.data.alarms.retain(|x| x.id != id);
                    self.sel_alarm = None;
                }
    }
}

/// Unterzeile eines Weckers: Tage · nächstes Klingeln bzw. aus / klingelt / schlummert.
pub(super) fn alarm_sub(a: &Alarm, now_s: i64, lang: Lang, ringing: bool) -> String {
    let days = a.days_text(lang);
    if ringing {
        return format!("{days} · {}", lang.t("klingelt", "ringing"));
    }
    if let Some(s) = a.snoozed_until {
        return format!("{days} · {} {}", lang.t("schlummert bis", "snoozed until"), super::local_hhmm(s * 1000));
    }
    if !a.enabled {
        return format!("{days} · {}", lang.t("aus", "off"));
    }
    match a.next_after(a.armed_from.max(now_s), &chrono::Local) {
        Some(n) => format!("{days} · {} {}", lang.t("in", "in"), until_text(n - now_s, lang)),
        None => days,
    }
}

/// „3 h 20 min“, „12 min“, „2 Tagen“ …
pub(super) fn until_text(secs: i64, lang: Lang) -> String {
    let m = (secs.max(0) + 59) / 60;
    if m < 60 {
        return format!("{m} min");
    }
    let h = m / 60;
    if h < 24 {
        return if m % 60 == 0 { format!("{h} h") } else { format!("{h} h {} min", m % 60) };
    }
    let d = h / 24;
    match (lang, d) {
        (Lang::De, 1) => format!("1 Tag {} h", h % 24),
        (Lang::De, _) => format!("{d} Tagen"),
        (Lang::En, 1) => format!("1 day {} h", h % 24),
        (Lang::En, _) => format!("{d} days"),
    }
}

pub(super) fn sort_alarms(v: &mut [Alarm]) {
    v.sort_by_key(|a| (a.hour, a.minute, a.id));
}
