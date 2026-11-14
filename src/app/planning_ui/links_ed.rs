//! Editor „Verknüpfungstypen“ (gelten für alle Projekte): Richtungsnamen, symmetrisch, Zielart. Der System-Typ `terminal`
//! ist mit einem Schloss markiert und nicht löschbar.

use super::delete::Target;
use super::helpers::{move_id, target_label};
use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::link_types::{create_link_type, reorder_link_types, update_link_type, NewLinkType, TargetKind, UpdateLinkType};
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2};
use eframe::egui::{self, Id, Key, RichText, Ui, Vec2};

pub(super) struct State {
    new_out: String,
    new_in: String,
    new_sym: bool,
    new_kind: TargetKind,
}

impl Default for State {
    fn default() -> Self {
        State { new_out: String::new(), new_in: String::new(), new_sym: false, new_kind: TargetKind::Ticket }
    }
}

const KINDS: [TargetKind; 4] = [TargetKind::Ticket, TargetKind::Terminal, TargetKind::Url, TargetKind::Branch];

impl PlanUi {
    pub(super) fn links_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        hint(ui, lang.t(
            "Verknüpfungstypen gelten für alle Projekte. Gerichtete Typen haben einen Namen je Richtung („blockiert“ / „ist blockiert durch“), symmetrische einen gemeinsamen.",
            "Link types apply to all projects. Directed types have one name per direction (“blocks” / “is blocked by”), symmetric ones share a name.",
        ));
        ui.add_space(S2);
        let links = self.links.clone();
        let order: Vec<i64> = links.iter().map(|l| l.id).collect();
        egui::Grid::new("links_grid").num_columns(6).spacing(Vec2::new(12.0, 8.0)).show(ui, |ui| {
            for h in [lang.t("Reihenfolge", "Order"), lang.t("Hin-Richtung", "Outgoing name"), lang.t("Rück-Richtung", "Incoming name"), lang.t("Art", "Kind"), lang.t("Ziel", "Target"), ""] {
                ui.label(RichText::new(h).small().color(style::dim(ui)));
            }
            ui.end_row();
            for l in &links {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, l.id, -1) {
                            let r = reorder_link_types(cx.conn, &o);
                            self.run(cx, r);
                        }
                    }
                    if style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                        let mut o = order.clone();
                        if move_id(&mut o, l.id, 1) {
                            let r = reorder_link_types(cx.conn, &o);
                            self.run(cx, r);
                        }
                    }
                });
                let (out, inn) = l.names(lang);
                if l.system {
                    ui.horizontal(|ui| {
                        widgets::lock_icon(ui, lang.t("System-Typ: nicht änderbar und nicht löschbar", "System type: cannot be changed or deleted"));
                        ui.label(RichText::new(&out).strong());
                    });
                    ui.label(RichText::new("–").color(style::dim(ui)));
                    ui.label(RichText::new(lang.t("System", "System")).small().color(style::dim(ui)));
                } else {
                    if let Some(v) = widgets::commit_text(ui, Id::new(("link_out", l.id)), &out, 190.0, "") {
                        let r = update_link_type(cx.conn, l.id, &UpdateLinkType { name_out: Some(v), ..Default::default() });
                        self.run(cx, r);
                    }
                    if !l.symmetric && l.target_kind == TargetKind::Ticket {
                        if let Some(v) = widgets::commit_text(ui, Id::new(("link_in", l.id)), &inn.clone().unwrap_or_default(), 190.0, "") {
                            let r = update_link_type(cx.conn, l.id, &UpdateLinkType { name_in: Some(v), ..Default::default() });
                            self.run(cx, r);
                        }
                    } else {
                        ui.label(RichText::new("–").color(style::dim(ui)));
                    }
                    if l.target_kind == TargetKind::Ticket {
                        let mut sym = l.symmetric;
                        if widgets::check(ui, &mut sym, lang.t("symmetrisch", "symmetric")).changed() {
                            let name_in = if !sym && inn.is_none() { Some(match lang {
                                Lang::De => format!("{out} (Rückrichtung)"),
                                Lang::En => format!("{out} (inverse)"),
                            }) } else { None };
                            let r = update_link_type(cx.conn, l.id, &UpdateLinkType { symmetric: Some(sym), name_in, ..Default::default() });
                            self.run(cx, r);
                        }
                    } else {
                        ui.label(RichText::new(lang.t("gerichtet", "directed")).small().color(style::dim(ui)));
                    }
                }
                ui.label(target_label(l.target_kind, lang));
                if l.system {
                    ui.label("");
                } else if style::icon_btn_small(ui, SIcon::Trash, lang.t("Verknüpfungstyp löschen …", "Delete link type …")).clicked() {
                    self.ask_delete(cx, Target::LinkType(l.id));
                }
                ui.end_row();
            }
        });
        ui.add_space(S2);
        // neuer Typ
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Neuer Verknüpfungstyp", "New link type")).strong());
            ui.add_space(S1);
            let ticket = self.links_state.new_kind == TargetKind::Ticket;
            let directed = ticket && !self.links_state.new_sym;
            let mut add = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(lang.t("Ziel:", "Target:"));
                egui::ComboBox::from_id_salt("new_link_kind").selected_text(target_label(self.links_state.new_kind, lang)).width(110.0).show_ui(ui, |ui| {
                    for k in KINDS {
                        ui.selectable_value(&mut self.links_state.new_kind, k, target_label(k, lang));
                    }
                });
                if self.links_state.new_kind != TargetKind::Ticket {
                    self.links_state.new_sym = false;
                } else {
                    widgets::check(ui, &mut self.links_state.new_sym, lang.t("symmetrisch", "symmetric"));
                }
            });
            ui.horizontal_wrapped(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.links_state.new_out).desired_width(200.0).hint_text(if directed { lang.t("Hin-Richtung, z. B. „löst aus“", "Outgoing, e.g. “causes”") } else { lang.t("Name", "Name") }).margin(egui::Margin::symmetric(8, 5)));
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    add = true;
                }
                if directed {
                    ui.add(egui::TextEdit::singleline(&mut self.links_state.new_in).desired_width(200.0).hint_text(lang.t("Rück-Richtung, z. B. „wird ausgelöst von“", "Incoming, e.g. “is caused by”")).margin(egui::Margin::symmetric(8, 5)));
                }
                let ready = !self.links_state.new_out.trim().is_empty() && (!directed || !self.links_state.new_in.trim().is_empty());
                if ui.add_enabled(ready, egui::Button::new(format!("+ {}", lang.t("Anlegen", "Add")))).clicked() {
                    add = true;
                }
            });
            if add {
                let n = NewLinkType {
                    name_out: self.links_state.new_out.clone(),
                    name_in: if directed { Some(self.links_state.new_in.clone()) } else { None },
                    symmetric: ticket && self.links_state.new_sym,
                    target_kind: self.links_state.new_kind,
                };
                if self.run(cx, create_link_type(cx.conn, &n)).is_some() {
                    self.links_state.new_out.clear();
                    self.links_state.new_in.clear();
                }
            }
        });
    }
}
