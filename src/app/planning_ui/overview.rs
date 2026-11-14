//! Übersichtsseite eines Projekts (Name, Schlüssel, Farbe, Beschreibung, Arbeitsordner, Schalter, Zugang zu den Editoren)
//! und die schreibgeschützte Ansicht des System-Projekts „Todolist“.

use super::helpers::color_of;
use super::widgets::{self, hint};
use super::{Cx, PlanUi, Tab};
use crate::planning::projects::{update_project, Project, TagColorMode, UpdateProject};
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use crate::tasks::{self, Item, ItemFilter, Tag};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui, Vec2};

/// Die Einträge der einfachen Liste für die schreibgeschützte Ansicht.
#[derive(Default)]
pub(super) struct Todo {
    pub dirty: bool,
    items: Vec<Item>,
    tags: Vec<Tag>,
    total: usize,
    done: usize,
}

impl Todo {
    fn load(&mut self, conn: &rusqlite::Connection) {
        self.dirty = false;
        self.items = tasks::list_items(conn, &ItemFilter { show_done: true, tags: Vec::new() }).unwrap_or_default();
        self.tags = tasks::list_tags(conn).unwrap_or_default();
        let (total, done) = tasks::counts(conn).unwrap_or((0, 0));
        self.total = total;
        self.done = done;
    }
}

/// Zählzeile „n offen · m erledigt“.
pub(super) fn count_text(total: usize, done: usize, lang: Lang) -> String {
    let open = total.saturating_sub(done);
    format!("{open} {} · {done} {}", lang.t("offen", "open"), lang.t("erledigt", "done"))
}

impl PlanUi {
    pub(super) fn overview(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        if proj.system {
            self.todo_overview(ui, cx, proj);
        } else {
            self.project_overview(ui, cx, proj);
        }
    }

    // ------------------------------------------------------------------ gewöhnliches Projekt

    fn project_overview(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let col = color_of(&proj.color);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 8.0, col);
            ui.label(RichText::new(&proj.name).heading().strong());
            ui.label(RichText::new(&proj.key).monospace().color(col));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::small_danger(ui, lang.t("Projekt löschen …", "Delete project …")).clicked() {
                    self.ask_delete_project(cx, proj);
                }
                if ui.button(lang.t("Einstellungen …", "Settings …")).clicked() {
                    self.open_settings(proj);
                }
            });
        });
        ui.add_space(S2);
        widgets::card(ui, |ui| {
            egui::Grid::new("plan_overview_grid").num_columns(2).spacing(Vec2::new(18.0, 8.0)).show(ui, |ui| {
                widgets::row(ui, lang.t("Schlüssel", "Key"), |ui| {
                    ui.label(RichText::new(&proj.key).monospace());
                });
                widgets::row(ui, lang.t("Farbe", "Color"), |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 7.0, col);
                        ui.label(RichText::new(&proj.color).monospace().color(style::dim(ui)));
                    });
                });
                widgets::row(ui, lang.t("Beschreibung", "Description"), |ui| {
                    if proj.description.trim().is_empty() {
                        ui.label(RichText::new(lang.t("– keine –", "– none –")).color(style::dim(ui)));
                    } else {
                        ui.label(&proj.description);
                    }
                });
                widgets::row(ui, lang.t("Arbeitsordner", "Working directory"), |ui| {
                    if proj.work_dir.trim().is_empty() {
                        ui.label(RichText::new(lang.t("– nicht gesetzt –", "– not set –")).color(style::dim(ui)));
                    } else {
                        ui.label(RichText::new(&proj.work_dir).monospace());
                    }
                });
                widgets::row(ui, lang.t("Tag-Farben", "Tag colors"), |ui| {
                    ui.label(match proj.tag_color_mode {
                        TagColorMode::Default => lang.t("Standardfarbe", "Default color"),
                        TagColorMode::Accent => lang.t("Projektfarbe", "Project color"),
                        TagColorMode::Random => lang.t("Zufällig", "Random"),
                    });
                });
            });
            ui.add_space(S3);
            let mut sprints = proj.use_sprints;
            let mut versions = proj.use_versions;
            let a = widgets::toggle_row(ui, &mut sprints, lang.t("Sprints verwenden", "Use sprints"), lang.t("Backlog und Sprint-Board.", "Backlog and sprint board."));
            ui.add_space(S1);
            let b = widgets::toggle_row(ui, &mut versions, lang.t("Versionen/Meilensteine verwenden", "Use versions/milestones"), lang.t("Release-Versionen und Meilensteine mit Fälligkeit.", "Release versions and milestones with due dates."));
            if a || b {
                let u = UpdateProject { use_sprints: Some(sprints), use_versions: Some(versions), ..Default::default() };
                let r = update_project(cx.conn, proj.id, &u);
                self.run(cx, r);
            }
        });
        ui.add_space(S3);
        // Zugang zu den Editoren
        let Some(sum) = self.sum.clone() else { return };
        ui.label(RichText::new(lang.t("Konfiguration", "Configuration")).strong());
        ui.add_space(S1);
        let fields: usize = sum.types.iter().map(|t| t.fields.len()).sum();
        let entries: [(Tab, &str, String); 6] = [
            (Tab::Types, lang.t("Typen", "Types"), sum.types.len().to_string()),
            (Tab::Fields, lang.t("Felder", "Fields"), fields.to_string()),
            (Tab::Statuses, lang.t("Status", "Statuses"), sum.statuses.len().to_string()),
            (Tab::Workflows, lang.t("Workflows", "Workflows"), sum.types.iter().filter(|t| t.ticket_type.workflow).count().to_string()),
            (Tab::WorkKinds, lang.t("Arten der Arbeit", "Kinds of work"), sum.work_kinds.len().to_string()),
            (Tab::Templates, lang.t("Vorlagen", "Templates"), String::new()),
        ];
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            for (tab, label, count) in entries {
                let r = egui::Frame::NONE
                    .fill(ui.visuals().faint_bg_color)
                    .stroke(egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_min_width(130.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(label).strong());
                            if !count.is_empty() {
                                ui.label(RichText::new(count).color(style::dim(ui)));
                            } else {
                                ui.label(RichText::new(lang.t("Export und Import", "Export and import")).small().color(style::dim(ui)));
                            }
                        });
                    })
                    .response
                    .interact(Sense::click());
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    ui.painter().rect_stroke(r.rect, 8.0, egui::Stroke::new(1.0_f32, style::accent().gamma_multiply(0.7)), egui::StrokeKind::Outside);
                }
                if r.clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(S3);
        // Platzhalter, bis es die Tickets gibt
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Noch nicht verfügbar", "Not available yet")).strong());
            ui.add_space(S1);
            ui.label(RichText::new(lang.t("Tickets, Boards und Sprints kommen in den nächsten Schritten.", "Tickets, boards and sprints come in the next steps.")).color(style::dim(ui)));
        });
    }

    // ------------------------------------------------------------------ System-Projekt „Todolist“

    fn todo_overview(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        if self.todo.dirty {
            self.todo.load(cx.conn);
        }
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 8.0, color_of(&proj.color));
            ui.label(RichText::new(&proj.name).heading().strong());
            widgets::lock_icon(ui, lang.t("Schreibgeschützt im Expertenmodus", "Read-only in expert mode"));
            ui.label(RichText::new(lang.t("schreibgeschützt im Expertenmodus", "read-only in expert mode")).small().color(style::dim(ui)));
        });
        ui.add_space(S1);
        hint(ui, lang.t(
            "Die Einträge der einfachen Todo-Liste. Bearbeitet wird sie im normalen Modus (Expertenmodus aus); hier lassen sie sich später in Tickets umwandeln.",
            "The entries of the simple to-do list. You edit them in normal mode (expert mode off); here they can later be converted into tickets.",
        ));
        ui.add_space(S2);
        ui.label(RichText::new(count_text(self.todo.total, self.todo.done, lang)).small().color(style::dim(ui)));
        ui.add_space(S2);
        if self.todo.items.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(lang.t("Die Todo-Liste ist leer.", "The to-do list is empty.")).color(style::dim(ui))));
            return;
        }
        let items = self.todo.items.clone();
        for it in &items {
            todo_card(ui, it, &self.todo.tags, lang);
            ui.add_space(8.0);
        }
    }
}

/// Schreibgeschützte Karte: Schlüssel, Titel, Text, Tags; ohne Bearbeiten, Abhaken und Löschen.
fn todo_card(ui: &mut Ui, it: &Item, tags: &[Tag], lang: Lang) {
    let v = ui.visuals();
    let dim = v.weak_text_color();
    let fade = if it.done { 0.6 } else { 1.0 };
    egui::Frame::NONE
        .fill(v.faint_bg_color)
        .stroke(egui::Stroke::new(1.0_f32, v.widgets.noninteractive.bg_stroke.color))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(it.key()).small().color(dim));
                if it.done {
                    ui.label(RichText::new(lang.t("erledigt", "done")).small().color(dim));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled(false, egui::Button::new(RichText::new(lang.t("In Ticket umwandeln", "Convert to ticket")).small())).on_disabled_hover_text(lang.t("Noch nicht verfügbar", "Not available yet"));
                });
            });
            if !it.title.is_empty() {
                let mut t = RichText::new(&it.title).strong().size(15.0);
                if it.done {
                    t = t.strikethrough().color(dim);
                }
                ui.label(t);
            }
            if !it.body.is_empty() {
                let body: String = it.body.lines().take(4).collect::<Vec<_>>().join("\n");
                let more = it.body.lines().count() > 4;
                let mut t = RichText::new(if more { format!("{body} …") } else { body });
                if it.done {
                    t = t.strikethrough().color(dim);
                }
                ui.label(t);
            }
            let mut mine: Vec<&Tag> = it.tags.iter().filter_map(|id| tags.iter().find(|t| t.id == *id)).collect();
            mine.sort_by_key(|t| t.name.to_lowercase());
            if !mine.is_empty() {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for t in mine {
                        let (r, g, b) = tasks::rgb(&t.color);
                        let c = Color32::from_rgb(r, g, b);
                        egui::Frame::NONE.fill(c.gamma_multiply(0.22 * fade)).corner_radius(9.0).inner_margin(egui::Margin::symmetric(8, 1)).show(ui, |ui| {
                            ui.label(RichText::new(&t.name).small().color(c.gamma_multiply(fade)));
                        });
                    }
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_line() {
        assert_eq!(count_text(5, 2, Lang::En), "3 open · 2 done");
        assert_eq!(count_text(5, 2, Lang::De), "3 offen · 2 erledigt");
        assert_eq!(count_text(0, 0, Lang::En), "0 open · 0 done");
        assert_eq!(count_text(1, 3, Lang::En), "0 open · 3 done", "nie negativ");
    }
}
