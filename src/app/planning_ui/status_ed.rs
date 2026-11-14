//! Editor „Status“: der Status-Pool des Projekts mit Farbe, Kategorie (To Do / In Arbeit / Erledigt) und Reihenfolge.

use super::delete::Target;
use super::helpers::{category_label, move_id, next_palette_color};
use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::statuses::{create_status, reorder_statuses, update_status, NewStatus, StatusCategory, UpdateStatus};
use crate::style::{self, Icon as SIcon, S2};
use eframe::egui::{self, Id, Key, Ui, Vec2};

pub(super) struct State {
    new_name: String,
    new_cat: StatusCategory,
}

impl Default for State {
    fn default() -> Self {
        State { new_name: String::new(), new_cat: StatusCategory::Todo }
    }
}

const CATEGORIES: [StatusCategory; 3] = [StatusCategory::Todo, StatusCategory::Progress, StatusCategory::Done];

impl PlanUi {
    pub(super) fn statuses_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        hint(ui, lang.t(
            "Der Status-Pool des Projekts. Jeder Typ wählt im Reiter „Workflows“ daraus seine Status. Die Kategorie „Erledigt“ steuert Burndown und „erledigt“.",
            "The project's status pool. Each type picks its statuses from it in the “Workflows” tab. The “Done” category drives burndown and “completed”.",
        ));
        ui.add_space(S2);
        let mut add = false;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.statuses.new_name).desired_width(220.0).hint_text(lang.t("Name des neuen Status …", "Name of the new status …")).margin(egui::Margin::symmetric(8, 5)));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.statuses.new_name.trim().is_empty() {
                add = true;
            }
            egui::ComboBox::from_id_salt("new_status_cat").selected_text(category_label(self.statuses.new_cat, lang)).width(120.0).show_ui(ui, |ui| {
                for c in CATEGORIES {
                    ui.selectable_value(&mut self.statuses.new_cat, c, category_label(c, lang));
                }
            });
            if ui.add_enabled(!self.statuses.new_name.trim().is_empty(), egui::Button::new(format!("+ {}", lang.t("Anlegen", "Add")))).clicked() {
                add = true;
            }
        });
        if add {
            let used: Vec<String> = sum.statuses.iter().map(|s| s.color.clone()).collect();
            let n = NewStatus { project_id: sum.project.id, key: None, name: Some(self.statuses.new_name.clone()), color: next_palette_color(&used).to_string(), category: self.statuses.new_cat };
            if self.run(cx, create_status(cx.conn, &n)).is_some() {
                self.statuses.new_name.clear();
            }
        }
        ui.add_space(S2);
        if sum.statuses.is_empty() {
            widgets::card(ui, |ui| {
                ui.label(egui::RichText::new(lang.t("Noch keine Status. Ohne Status arbeiten Typen ohne Workflow.", "No statuses yet. Without statuses, types work without a workflow.")).color(style::dim(ui)));
            });
            return;
        }
        let order: Vec<i64> = sum.statuses.iter().map(|s| s.id).collect();
        egui::Grid::new("status_grid").num_columns(5).spacing(Vec2::new(12.0, 8.0)).show(ui, |ui| {
            for s in &sum.statuses {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, s.id, -1) {
                            let r = reorder_statuses(cx.conn, sum.project.id, &o);
                            self.run(cx, r);
                        }
                    }
                    if style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, s.id, 1) {
                            let r = reorder_statuses(cx.conn, sum.project.id, &o);
                            self.run(cx, r);
                        }
                    }
                });
                let mut hex = s.color.clone();
                if widgets::color_edit(ui, ("status_color", s.id), &mut hex, lang) {
                    let r = update_status(cx.conn, s.id, &UpdateStatus { color: Some(hex), ..Default::default() });
                    self.run(cx, r);
                }
                if let Some(v) = widgets::commit_text(ui, Id::new(("status_name", s.id)), &s.display(lang), 220.0, "") {
                    let r = update_status(cx.conn, s.id, &UpdateStatus { name: Some(v), ..Default::default() });
                    self.run(cx, r);
                }
                let mut cat = s.category;
                egui::ComboBox::from_id_salt(("status_cat", s.id)).selected_text(category_label(cat, lang)).width(120.0).show_ui(ui, |ui| {
                    for c in CATEGORIES {
                        ui.selectable_value(&mut cat, c, category_label(c, lang));
                    }
                });
                if cat != s.category {
                    let r = update_status(cx.conn, s.id, &UpdateStatus { category: Some(cat), ..Default::default() });
                    self.run(cx, r);
                }
                if style::icon_btn_small(ui, SIcon::Trash, lang.t("Status löschen …", "Delete status …")).clicked() {
                    self.ask_delete(cx, Target::Status(s.id));
                }
                ui.end_row();
            }
        });
        if sum.statuses.iter().any(|s| s.key.is_some() && s.name.is_none()) {
            ui.add_space(S2);
            hint(ui, lang.t("Standardnamen werden automatisch übersetzt, bis du sie umbenennst (dann gilt dein Text in beiden Sprachen).", "Default names are translated automatically until you rename them (then your text applies to both languages)."));
        }
    }
}
