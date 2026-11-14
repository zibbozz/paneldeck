//! Editor „Arten der Arbeit“: die Werteliste für die Zeiterfassung des Projekts (Standard: Entwicklung, Test, Planung, Schärfung).

use super::delete::Target;
use super::helpers::move_id;
use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::work_kinds::{create_work_kind, rename_work_kind, reorder_work_kinds};
use crate::style::{self, Icon as SIcon, S2};
use eframe::egui::{self, Id, Key, Ui, Vec2};

#[derive(Default)]
pub(super) struct State {
    new_name: String,
}

impl PlanUi {
    pub(super) fn kinds_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        hint(ui, lang.t(
            "Die Arten der Arbeit stehen später beim Erfassen von Zeit zur Auswahl. Die erste Art ist die Vorgabe.",
            "The kinds of work are offered later when logging time. The first kind is the default.",
        ));
        ui.add_space(S2);
        let mut add = false;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.kinds.new_name).desired_width(240.0).hint_text(lang.t("Neue Art der Arbeit …", "New kind of work …")).margin(egui::Margin::symmetric(8, 5)));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.kinds.new_name.trim().is_empty() {
                add = true;
            }
            if ui.add_enabled(!self.kinds.new_name.trim().is_empty(), egui::Button::new(format!("+ {}", lang.t("Anlegen", "Add")))).clicked() {
                add = true;
            }
        });
        if add && self.run(cx, create_work_kind(cx.conn, sum.project.id, None, Some(&self.kinds.new_name))).is_some() {
            self.kinds.new_name.clear();
        }
        ui.add_space(S2);
        if sum.work_kinds.is_empty() {
            widgets::card(ui, |ui| {
                ui.label(egui::RichText::new(lang.t("Noch keine Arten der Arbeit.", "No kinds of work yet.")).color(style::dim(ui)));
            });
            return;
        }
        let order: Vec<i64> = sum.work_kinds.iter().map(|w| w.id).collect();
        egui::Grid::new("kinds_grid").num_columns(3).spacing(Vec2::new(12.0, 8.0)).show(ui, |ui| {
            for w in &sum.work_kinds {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, w.id, -1) {
                            let r = reorder_work_kinds(cx.conn, sum.project.id, &o);
                            self.run(cx, r);
                        }
                    }
                    if style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, w.id, 1) {
                            let r = reorder_work_kinds(cx.conn, sum.project.id, &o);
                            self.run(cx, r);
                        }
                    }
                });
                if let Some(v) = widgets::commit_text(ui, Id::new(("kind_name", w.id)), &w.display(lang), 240.0, "") {
                    let r = rename_work_kind(cx.conn, w.id, &v);
                    self.run(cx, r);
                }
                if style::icon_btn_small(ui, SIcon::Trash, lang.t("Art der Arbeit löschen …", "Delete kind of work …")).clicked() {
                    self.ask_delete(cx, Target::WorkKind(w.id));
                }
                ui.end_row();
            }
        });
    }
}
