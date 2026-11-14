//! Editor „Typen“: Liste, Name, Farbe, Icon, Workflow ein/aus, erlaubte Eltern-Typen, anlegen/umbenennen/löschen.

use super::delete::Target;
use super::helpers::{color_of, move_id, next_palette_color};
use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::summary::TypeSummary;
use crate::planning::types::{create_type, reorder_types, set_parents, update_type, NewType, UpdateType};
use crate::planning::workflow::set_workflow_statuses;
use crate::style::{self, Icon as SIcon, S1, S2, S3};
use eframe::egui::{self, Id, Key, RichText, Sense, Ui, Vec2};

#[derive(Default)]
pub(super) struct State {
    pub sel: Option<i64>,
    new_name: String,
}

const LIST_W: f32 = 250.0;

impl PlanUi {
    pub(super) fn types_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        let list: Vec<&TypeSummary> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
        if !self.types.sel.is_some_and(|s| list.iter().any(|t| t.ticket_type.id == s)) {
            self.types.sel = list.first().map(|t| t.ticket_type.id);
        }
        hint(ui, lang.t(
            "Typen beschreiben, was ein Ticket ist (z. B. Epic, Userstory, Fehler). Jeder Typ hat eigene Felder und optional einen Workflow.",
            "Types describe what a ticket is (e.g. epic, user story, defect). Each type has its own fields and optionally a workflow.",
        ));
        ui.add_space(S2);
        // neuer Typ
        let mut add = false;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.types.new_name).desired_width(LIST_W - 70.0).hint_text(lang.t("Name des neuen Typs …", "Name of the new type …")).margin(egui::Margin::symmetric(8, 5)));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.types.new_name.trim().is_empty() {
                add = true;
            }
            if ui.add_enabled(!self.types.new_name.trim().is_empty(), egui::Button::new(format!("+ {}", lang.t("Anlegen", "Add")))).clicked() {
                add = true;
            }
        });
        if add {
            let used: Vec<String> = sum.types.iter().map(|t| t.ticket_type.color.clone()).collect();
            let wf = !sum.statuses.is_empty();
            let n = NewType { project_id: sum.project.id, key: None, name: Some(self.types.new_name.clone()), icon: None, color: next_palette_color(&used).to_string(), workflow: wf };
            if let Some(id) = self.run(cx, create_type(cx.conn, &n)) {
                if wf {
                    let ids: Vec<i64> = sum.statuses.iter().map(|s| s.id).collect();
                    let _ = set_workflow_statuses(cx.conn, id, &ids);
                }
                self.types.sel = Some(id);
                self.types.new_name.clear();
            }
        }
        ui.add_space(S2);
        if list.is_empty() {
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Dieses Projekt hat noch keine Typen.", "This project has no types yet.")).color(style::dim(ui)));
                ui.label(RichText::new(lang.t("Lege oben einen an – oder importiere eine Vorlage im Reiter „Vorlagen“.", "Add one above – or import a template in the “Templates” tab.")).small().color(style::dim(ui)));
            });
            return;
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(LIST_W);
                for t in &list {
                    let tt = &t.ticket_type;
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(LIST_W, 34.0), Sense::hover());
                    let resp = ui.interact(rect, Id::new(("type_row", tt.id)), Sense::click());
                    let sel = self.types.sel == Some(tt.id);
                    let fill = if sel { style::accent().gamma_multiply(0.2) } else if resp.hovered() { ui.visuals().widgets.hovered.bg_fill } else { egui::Color32::TRANSPARENT };
                    ui.painter().rect_filled(rect, 7.0, fill);
                    let badge = egui::Rect::from_min_size(rect.min + Vec2::new(6.0, 5.0), Vec2::splat(24.0));
                    widgets::paint_type_badge(ui.painter(), badge, tt.icon.as_deref(), color_of(&tt.color));
                    ui.painter().text(rect.min + Vec2::new(38.0, 17.0), egui::Align2::LEFT_CENTER, tt.display(lang), egui::FontId::proportional(13.5), ui.visuals().text_color());
                    if !tt.workflow {
                        ui.painter().text(rect.right_center() - Vec2::new(8.0, 0.0), egui::Align2::RIGHT_CENTER, lang.t("ohne Workflow", "no workflow"), egui::FontId::proportional(10.5), style::dim(ui));
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.types.sel = Some(tt.id);
                    }
                }
            });
            ui.add_space(S3);
            ui.separator();
            ui.add_space(S3);
            ui.vertical(|ui| {
                if let Some(t) = self.types.sel.and_then(|s| list.iter().find(|t| t.ticket_type.id == s)) {
                    self.type_detail(ui, cx, &sum.project.id, t, &list, &sum.types.iter().map(|t| t.ticket_type.id).collect::<Vec<_>>());
                }
            });
        });
    }

    fn type_detail(&mut self, ui: &mut Ui, cx: &Cx, project_id: &i64, t: &TypeSummary, all: &[&TypeSummary], order: &[i64]) {
        let lang = cx.lang;
        let tt = &t.ticket_type;
        ui.horizontal(|ui| {
            widgets::type_badge(ui, tt.icon.as_deref(), color_of(&tt.color), 26.0);
            ui.label(RichText::new(tt.display(lang)).heading());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::small_danger(ui, lang.t("Typ löschen …", "Delete type …")).clicked() {
                    self.ask_delete(cx, Target::Type(tt.id));
                }
                if style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                    let mut o = order.to_vec();
                    if move_id(&mut o, tt.id, 1) {
                        let r = reorder_types(cx.conn, *project_id, &o);
                        self.run(cx, r);
                    }
                }
                if style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                    let mut o = order.to_vec();
                    if move_id(&mut o, tt.id, -1) {
                        let r = reorder_types(cx.conn, *project_id, &o);
                        self.run(cx, r);
                    }
                }
            });
        });
        ui.add_space(S2);
        let caption = |ui: &mut Ui, text: &str| {
            ui.add_space(S2);
            ui.label(RichText::new(text).small().strong().color(style::dim(ui)));
            ui.add_space(2.0);
        };
        caption(ui, lang.t("Name", "Name"));
        if let Some(v) = widgets::commit_text(ui, Id::new(("type_name", tt.id)), &tt.display(lang), 300.0, "") {
            let r = update_type(cx.conn, tt.id, &UpdateType { name: Some(v), ..Default::default() });
            self.run(cx, r);
        }
        caption(ui, lang.t("Farbe", "Color"));
        ui.horizontal(|ui| {
            let mut hex = tt.color.clone();
            if widgets::color_edit(ui, ("type_color", tt.id), &mut hex, lang) {
                let r = update_type(cx.conn, tt.id, &UpdateType { color: Some(hex), ..Default::default() });
                self.run(cx, r);
            }
            ui.label(RichText::new(&tt.color).monospace().color(style::dim(ui)));
        });
        caption(ui, lang.t("Icon (optional)", "Icon (optional)"));
        let mut icon = tt.icon.clone();
        if widgets::icon_picker(ui, ("type_icon", tt.id), &mut icon, color_of(&tt.color), lang) {
            let r = update_type(cx.conn, tt.id, &UpdateType { icon: Some(icon), ..Default::default() });
            self.run(cx, r);
        }
        caption(ui, lang.t("Workflow", "Workflow"));
        let mut on = tt.workflow;
        if widgets::toggle_row(ui, &mut on, lang.t("Mit Status und Workflow", "With status and workflow"), lang.t("Aus: der Typ hat kein Statusfeld und erscheint nicht im Board.", "Off: the type has no status field and does not appear on the board.")) {
            self.toggle_workflow(cx, tt.id, on);
        }
        caption(ui, lang.t("Darf Kind sein von", "May be a child of"));
        let others: Vec<&&TypeSummary> = all.iter().filter(|o| o.ticket_type.id != tt.id).collect();
        if others.is_empty() {
            hint(ui, lang.t("Es gibt keinen anderen Typ.", "There is no other type."));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for o in others {
                let oid = o.ticket_type.id;
                let mut on = t.parents.contains(&oid);
                if widgets::check(ui, &mut on, o.ticket_type.display(lang)).changed() {
                    let mut parents = t.parents.clone();
                    if on {
                        parents.push(oid);
                    } else {
                        parents.retain(|p| *p != oid);
                    }
                    let r = set_parents(cx.conn, tt.id, &parents);
                    self.run(cx, r);
                }
            }
        });
        hint(ui, lang.t("Beispiel: Eine Unteraufgabe darf unter einer Userstory hängen.", "Example: a subtask may sit below a user story."));
        let children: Vec<String> = all.iter().filter(|o| o.parents.contains(&tt.id)).map(|o| o.ticket_type.display(lang)).collect();
        caption(ui, lang.t("Darf enthalten", "May contain"));
        if children.is_empty() {
            ui.label(RichText::new(lang.t("– nichts –", "– nothing –")).color(style::dim(ui)));
        } else {
            ui.label(children.join(", "));
        }
        if tt.key.is_some() && tt.name.is_none() {
            ui.add_space(S1);
            hint(ui, lang.t("Standardname – er wird automatisch übersetzt, bis du ihn umbenennst (dann gilt dein Text in beiden Sprachen).", "Default name – it is translated automatically until you rename it (then your text applies to both languages)."));
        }
    }
}
