//! Unterreiter „Geburtstage“: Liste (nächster Geburtstag zuerst) mit Alter und Countdown, Details mit Name, Datum (Jahr
//! optional) und Erinnerung (Standard aus).

use super::{row, rows, BdayEdit, Env, Sub, TimeUi};
use crate::style;
use crate::time::birthday::{self, Birthday};
use crate::time::store;
use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, RichText, Ui};

impl TimeUi {
    pub(super) fn birthday_list(&mut self, ui: &mut Ui, env: &Env) {
        let lang = env.lang;
        let today = today(env.now_ms);
        let mut idx: Vec<(i64, usize)> = (0..self.data.birthdays.len())
            .filter(|&i| self.matches(5, &self.data.birthdays[i].name))
            .map(|i| (self.data.birthdays[i].days_until(today).unwrap_or(i64::MAX), i))
            .collect();
        idx.sort_by_key(|(d, i)| (*d, self.data.birthdays[*i].name.to_lowercase()));
        if self.list_head(ui, 5, lang.t("Neuer Geburtstag", "New birthday"), self.data.birthdays.len(), lang)
            && let Some(conn) = env.conn {
                let mut b = Birthday::new("", today.month(), today.day(), None);
                if self.db_result(store::save_birthday(conn, &mut b)).is_some() {
                    self.sel_birthday = Some(b.id);
                    self.data.birthdays.push(b);
                }
            }
        if self.data.birthdays.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new(lang.t("Noch keine Geburtstage.", "No birthdays yet.")).color(style::dim(ui)));
            return;
        }
        rows(ui, "time_bday_rows", idx.len(), |ui, i| {
            let b = &self.data.birthdays[idx[i].1];
            let id = b.id;
            let age = b.age_on(today).map(|a| a.to_string()).unwrap_or_default();
            let col = b.is_today(today).then(style::accent);
            let name = if b.name.trim().is_empty() { lang.t("(ohne Namen)", "(no name)").to_string() } else { b.name.clone() };
            if row(ui, self.sel_birthday == Some(id), &name, &format!("{} · {}", b.date_text(lang), b.describe(today, lang)), &age, col, 0.0, |_| {}) {
                self.sel_birthday = Some(id);
                self.confirm_delete = None;
            }
        });
    }

    pub(super) fn birthday_detail(&mut self, ui: &mut Ui, env: &Env) {
        let lang = env.lang;
        let today = today(env.now_ms);
        let Some(b) = self.sel_birthday.and_then(|id| self.data.birthday(id)).cloned() else {
            ui.label(RichText::new(lang.t("Wähle links einen Geburtstag.", "Select a birthday on the left.")).color(style::dim(ui)));
            return;
        };
        let id = b.id;
        ui.heading(if b.name.trim().is_empty() { lang.t("(ohne Namen)", "(no name)").to_string() } else { b.name.clone() });
        ui.label(RichText::new(b.describe(today, lang)).size(18.0).color(if b.is_today(today) { style::accent() } else { ui.visuals().text_color() }));
        if let Some(age) = b.age_on(today) {
            ui.label(RichText::new(format!("{} {age}", lang.t("Alter:", "Age:"))).color(style::dim(ui)));
        }
        ui.add_space(8.0);
        let mut e = b.clone();
        // Datum über einen Bearbeitungspuffer: Eingaben werden nie angepasst; gespeichert wird nur ein gültiges Datum.
        if self.bday_edit.as_ref().is_none_or(|x| x.id != id) {
            self.bday_edit = Some(BdayEdit { id, day: b.day, month: b.month, has_year: b.year.is_some(), year: b.year.unwrap_or(today.year() - 30) });
        }
        let mut buf = self.bday_edit.clone().expect("gesetzt");
        egui::Grid::new("time_bday_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(lang.t("Name", "Name"));
            ui.add(egui::TextEdit::singleline(&mut e.name).char_limit(birthday::MAX_NAME_CHARS).desired_width(220.0));
            ui.end_row();
            ui.label(lang.t("Datum", "Date"));
            ui.horizontal(|ui| {
                if lang == crate::state::Lang::De {
                    ui.add(egui::DragValue::new(&mut buf.day).range(1..=31).suffix("."));
                    ui.add(egui::DragValue::new(&mut buf.month).range(1..=12).suffix("."));
                } else {
                    ui.add(egui::DragValue::new(&mut buf.month).range(1..=12).prefix(lang.t("Monat ", "Month ")));
                    ui.add(egui::DragValue::new(&mut buf.day).range(1..=31).prefix(lang.t("Tag ", "Day ")));
                }
            });
            ui.end_row();
            ui.label(lang.t("Jahr", "Year"));
            ui.horizontal(|ui| {
                ui.checkbox(&mut buf.has_year, "");
                ui.add_enabled(buf.has_year, egui::DragValue::new(&mut buf.year).range(1000..=today.year()));
                if !buf.has_year {
                    ui.label(RichText::new(lang.t("unbekannt (nur Countdown)", "unknown (countdown only)")).color(style::dim(ui)));
                }
            });
            ui.end_row();
            ui.label(lang.t("Erinnerung", "Reminder"));
            ui.horizontal(|ui| {
                style::toggle(ui, &mut e.remind);
                ui.label(RichText::new(lang.t("am Tag selbst ab 8 Uhr", "on the day from 8:00")).color(style::dim(ui)));
            });
            ui.end_row();
        });
        let year = buf.has_year.then_some(buf.year);
        let valid = birthday::valid(buf.month, buf.day, year);
        if valid {
            (e.day, e.month, e.year) = (buf.day, buf.month, year);
        } else {
            ui.colored_label(style::danger_soft(), lang.t("Dieses Datum gibt es nicht – gespeichert bleibt das vorige.", "This date does not exist – the previous one stays saved."));
        }
        if buf.month == 2 && buf.day == 29 {
            ui.label(RichText::new(lang.t("In Nicht-Schaltjahren am 28. Februar.", "On 28 February in non-leap years.")).small().color(style::dim(ui)));
        }
        self.bday_edit = Some(buf);
        if e != b {
            e.name = crate::time::alarm::clean_name(&e.name);
            if e.remind != b.remind || (e.month, e.day) != (b.month, b.day) {
                e.reminded_year = None;
            }
            if let Some(conn) = env.conn
                && self.db_result(store::save_birthday(conn, &mut e)).is_some()
                && let Some(x) = self.data.birthdays.iter_mut().find(|x| x.id == id)
            {
                *x = e;
            }
        }
        if self.delete_button(ui, Sub::Birthdays, id, lang)
            && let Some(conn) = env.conn
                && self.db_result(store::delete(conn, store::Table::Birthdays, id)).is_some() {
                    self.data.birthdays.retain(|x| x.id != id);
                    self.sel_birthday = None;
                }
    }
}

/// Heutiges lokales Datum zum Zeitpunkt `now_ms`.
pub(super) fn today(now_ms: i64) -> NaiveDate {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(now_ms).single().map(|d| d.date_naive()).unwrap_or_else(|| chrono::Local::now().date_naive())
}
