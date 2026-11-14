//! Dialoge des Reiters „Boards“: Board anlegen/bearbeiten (Name, Beschreibung, Spalten mit mehreren Status, Typ-Filter, Quelle,
//! Sprint-Schalter, Kartenfelder), Löschen mit Bestätigung und der Dialog für fehlende Pflichtfelder beim Statuswechsel.
//! Die Prüfung der Konfiguration macht `views::boards::check_config` (zweisprachige Fehler); die Bearbeitungsschritte stehen in
//! `board_logic.rs`.

use super::board_logic as logic;
use super::helpers::color_of;
use super::values;
use super::widgets;
use super::{Cx, PlanUi};
use crate::planning::fields::{Field, FieldKind};
use crate::planning::history::Source;
use crate::planning::summary::ProjectSummary;
use crate::planning::ticket_error::{FieldLabel, TicketError};
use crate::planning::tickets::Ticket;
use crate::planning::transition::transition_ticket;
use crate::planning::views::boards::{check_config, column_title, create_board, delete_board, update_board, BoardConfig, NewBoard, UpdateBoard};
use crate::planning::views::filters::list_filters_for_project;
use crate::style::{self, Icon, S1, S2};
use eframe::egui::{self, Id, RichText, Ui};
use serde_json::Value;

/// Dialog „Board anlegen/bearbeiten“ (Arbeitskopie, gespeichert wird erst mit „Speichern“).
pub(super) struct CfgDlg {
    /// `None` = neues Board.
    id: Option<i64>,
    name: String,
    description: String,
    config: BoardConfig,
    err: Option<String>,
}

impl CfgDlg {
    pub(super) fn new(id: Option<i64>, name: String, description: String, config: BoardConfig) -> Self {
        CfgDlg { id, name, description, config, err: None }
    }
}

/// Bestätigung „Board löschen“.
pub(super) struct DeleteDlg {
    pub id: i64,
    pub name: String,
}

struct NeedField {
    label: FieldLabel,
    field: Option<Field>,
    value: Value,
}

/// Dialog „Pflichtfelder ausfüllen“: die Karte wurde auf einen Status gezogen, dessen Übergang Felder verlangt.
pub(super) struct NeedsDlg {
    ticket_id: i64,
    key: String,
    title: String,
    to_status: i64,
    to_name: String,
    /// Vor welches Ticket die Karte nach dem Wechsel gehört (Rang).
    before: Option<i64>,
    fields: Vec<NeedField>,
    err: Option<String>,
}

impl NeedsDlg {
    pub(super) fn new(sum: &ProjectSummary, ticket: &Ticket, to_status: i64, to_name: String, before: Option<i64>, missing: &[FieldLabel]) -> Self {
        let ty = sum.type_by_id(ticket.type_id);
        let fields = missing
            .iter()
            .map(|l| {
                let field = ty.and_then(|t| t.field(l.id)).cloned();
                let value = field.as_ref().map(|f| values::blank_full(f.kind, &f.options)).unwrap_or(Value::Null);
                NeedField { label: l.clone(), field, value }
            })
            .collect();
        NeedsDlg { ticket_id: ticket.id, key: ticket.key(), title: ticket.title.clone(), to_status, to_name, before, fields, err: None }
    }
}

/// Aktion aus dem Spalten-Editor (wird nach dem Zeichnen angewendet).
enum Act {
    MoveCol(usize, i32),
    DelCol(usize),
    Unassign(i64),
    Assign(usize, i64),
    MoveStatus(usize, i64, i32),
    AddCol(Option<i64>),
}

/// Einstellungs-Zeile: gedämpfte Beschriftung, darunter der Inhalt.
fn label(ui: &mut Ui, text: &str) {
    ui.add_space(S2);
    ui.label(RichText::new(text).strong());
}

impl PlanUi {
    /// Alle Dialoge des Reiters „Boards“ (aus `PlanUi::dialogs`).
    pub(super) fn board_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        self.board_cfg_dialog(ctx, cx);
        self.board_delete_dialog(ctx, cx);
        self.board_needs_dialog(ctx, cx);
    }

    fn board_cfg_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.boards.cfg.take() else { return };
        let Some(sum) = self.sum.clone() else { return };
        let lang = cx.lang;
        let pid = sum.project.id;
        let problem = check_config(cx.conn, pid, &d.config).err().map(|e| e.message(lang));
        let groups = logic::field_groups(&sum, lang);
        let status_ids = logic::workflow_status_ids(&sum);
        let filters = list_filters_for_project(cx.conn, pid).unwrap_or_default();
        let status_name = |id: i64| sum.status(id).map(|s| s.display(lang)).unwrap_or_else(|| format!("#{id}"));
        let status_color = |id: i64| sum.status(id).map(|s| color_of(&s.color)).unwrap_or(egui::Color32::GRAY);
        let (mut save, mut close) = (false, false);
        let mut acts: Vec<Act> = Vec::new();
        let m = egui::Modal::new(Self::modal_id("board_cfg")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 740.0));
            widgets::dialog_title(ui, if d.id.is_some() { lang.t("Board bearbeiten", "Edit board") } else { lang.t("Neues Board", "New board") });
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 250.0).max(220.0)).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(ui.available_width() - 8.0);
                // ---- Name und Beschreibung
                ui.label(RichText::new(lang.t("Name", "Name")).strong());
                ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(340.0).margin(egui::Margin::symmetric(8, 5)));
                label(ui, lang.t("Beschreibung", "Description"));
                ui.add(egui::TextEdit::multiline(&mut d.description).desired_rows(2).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
                // ---- Spalten
                label(ui, lang.t("Spalten", "Columns"));
                widgets::hint(
                    ui,
                    lang.t(
                        "Eine Karte steht in der Spalte ihres Status. Mehrere Status in einer Spalte sind möglich; beim Ziehen in die Spalte wird der erste erlaubte Status der Liste gesetzt. Ein leerer Titel zeigt den Namen des ersten Status.",
                        "A card sits in the column of its status. A column can hold several statuses; dragging a card into it sets the first allowed status of the list. An empty title shows the name of the first status.",
                    ),
                );
                ui.add_space(S1);
                let ncols = d.config.columns.len();
                for ci in 0..ncols {
                    widgets::card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{}", ci + 1)).color(style::dim(ui)));
                            let auto = column_title(&crate::planning::views::boards::BoardColumn { title: String::new(), status_ids: d.config.columns[ci].status_ids.clone() }, &sum.statuses, lang);
                            let hint = if auto.is_empty() { lang.t("Titel", "Title").to_string() } else { auto };
                            ui.add(egui::TextEdit::singleline(&mut d.config.columns[ci].title).id(Id::new(("board_col_title", ci))).desired_width(240.0).hint_text(hint).margin(egui::Margin::symmetric(8, 4)));
                            if style::icon_btn_small(ui, Icon::Up, lang.t("Spalte nach links", "Move column left")).clicked() {
                                acts.push(Act::MoveCol(ci, -1));
                            }
                            if style::icon_btn_small(ui, Icon::Down, lang.t("Spalte nach rechts", "Move column right")).clicked() {
                                acts.push(Act::MoveCol(ci, 1));
                            }
                            if style::icon_btn_small(ui, Icon::Trash, lang.t("Spalte löschen (die Status bleiben unten übrig)", "Delete column (its statuses move to “not on the board”)")).clicked() {
                                acts.push(Act::DelCol(ci));
                            }
                        });
                        ui.add_space(S1);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::Vec2::new(5.0, 4.0);
                            let ids = d.config.columns[ci].status_ids.clone();
                            for (k, sid) in ids.iter().enumerate() {
                                if k > 0 && ui.add(egui::Button::new(RichText::new("‹").small()).small()).on_hover_text(lang.t("Früher in der Reihenfolge", "Earlier in the order")).clicked() {
                                    acts.push(Act::MoveStatus(ci, *sid, -1));
                                }
                                let b = egui::Button::new(RichText::new(format!("{}  ×", status_name(*sid))).color(status_color(*sid))).small();
                                if ui.add(b).on_hover_text(lang.t("Aus dieser Spalte entfernen", "Remove from this column")).clicked() {
                                    acts.push(Act::Unassign(*sid));
                                }
                            }
                            ui.menu_button(format!("+ {}", lang.t("Status", "Status")), |ui| {
                                let mut any = false;
                                for sid in &status_ids {
                                    if ids.contains(sid) {
                                        continue;
                                    }
                                    any = true;
                                    let other = d.config.column_of(*sid).map(|c| format!("  ({} {})", lang.t("Spalte", "column"), c + 1)).unwrap_or_default();
                                    if ui.button(format!("{}{other}", status_name(*sid))).clicked() {
                                        acts.push(Act::Assign(ci, *sid));
                                        ui.close();
                                    }
                                }
                                if !any {
                                    ui.label(RichText::new(lang.t("Alle Status sind schon in dieser Spalte.", "All statuses are already in this column.")).small());
                                }
                            });
                        });
                    });
                    ui.add_space(S1);
                }
                let free = logic::unassigned_statuses(&d.config, &status_ids);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::new(6.0, 4.0);
                    if ui.button(format!("+ {}", lang.t("Spalte", "Column"))).on_hover_text(lang.t("Neue Spalte (mit dem ersten freien Status)", "New column (with the first free status)")).clicked() {
                        acts.push(Act::AddCol(free.first().copied()));
                    }
                    if free.is_empty() {
                        ui.label(RichText::new(lang.t("Jeder Status steht in einer Spalte.", "Every status is in a column.")).small().color(style::dim(ui)));
                    } else {
                        ui.label(RichText::new(lang.t("Nicht auf dem Board:", "Not on the board:")).small().color(style::dim(ui)));
                        for sid in &free {
                            let b = egui::Button::new(RichText::new(format!("+ {}", status_name(*sid))).small().color(status_color(*sid))).small();
                            if ui.add(b).on_hover_text(lang.t("Als neue Spalte hinzufügen", "Add as a new column")).clicked() {
                                acts.push(Act::AddCol(Some(*sid)));
                            }
                        }
                    }
                });
                // ---- Typ-Filter
                label(ui, lang.t("Typen", "Types"));
                widgets::hint(ui, lang.t("Nur diese Typen zeigen; keine Auswahl = alle Typen mit Workflow.", "Show only these types; no selection = all types with a workflow."));
                ui.add_space(S1);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::new(6.0, 4.0);
                    for t in sum.types.iter().filter(|t| t.ticket_type.workflow) {
                        let on = d.config.type_filter.contains(&t.ticket_type.id);
                        if style::pill(ui, &t.ticket_type.display(lang), None, color_of(&t.ticket_type.color), on).clicked() {
                            logic::toggle_type(&mut d.config, t.ticket_type.id);
                        }
                    }
                });
                // ---- Quelle und Sprint (noch nicht angeschlossen)
                label(ui, lang.t("Quelle", "Source"));
                ui.add_enabled_ui(false, |ui| {
                    let sel = d.config.source_filter_id.and_then(|id| filters.iter().find(|f| f.id == id)).map(|f| f.name.clone()).unwrap_or_else(|| lang.t("Alle Tickets", "All tickets").to_string());
                    egui::ComboBox::from_id_salt("board_source").selected_text(sel).width(260.0).show_ui(ui, |_| {});
                });
                widgets::hint(ui, lang.t("Ein gespeicherter Filter als Quelle braucht die Abfrage-Suche – noch nicht verfügbar.", "A saved filter as the source needs the query search — not available yet."));
                // TODO search: Auswahl gespeicherter Filter (`filters`) freischalten und in `load_board` auswerten.
                label(ui, lang.t("Umfang", "Scope"));
                ui.add_enabled_ui(false, |ui| {
                    let mut only = d.config.only_active_sprint;
                    ui.horizontal(|ui| {
                        widgets::radio(ui, &mut only, false, lang.t("Alle Tickets", "All tickets"));
                        widgets::radio(ui, &mut only, true, lang.t("Nur aktiver Sprint", "Only the active sprint"));
                    });
                });
                widgets::hint(ui, lang.t("„Nur aktiver Sprint“ braucht Sprints – noch nicht verfügbar.", "“Only the active sprint” needs sprints — not available yet."));
                // TODO sprints: Schalter freischalten und `cfg.only_active_sprint` in `load_board` auswerten.
                // ---- Karte
                label(ui, lang.t("Karte", "Card"));
                ui.horizontal(|ui| {
                    widgets::check(ui, &mut d.config.show_key, lang.t("Ticket-Schlüssel", "Ticket key"));
                    ui.add_space(S2);
                    widgets::check(ui, &mut d.config.show_type_icon, lang.t("Typ-Icon", "Type icon"));
                });
                ui.add_space(S1);
                widgets::hint(ui, lang.t("Weitere Felder auf der Karte (Reihenfolge wie in der Liste):", "More fields on the card (order as in the list):"));
                let selected = logic::selected_groups(&d.config, &groups);
                let mut order: Vec<usize> = selected.clone();
                order.extend((0..groups.len()).filter(|g| !selected.contains(g)));
                for gi in order {
                    let g = &groups[gi];
                    let mut on = logic::group_selected(&d.config, g);
                    ui.horizontal(|ui| {
                        if widgets::check(ui, &mut on, &g.name).changed() {
                            logic::toggle_group(&mut d.config, g, on);
                        }
                        if on {
                            if style::icon_btn_small(ui, Icon::Up, lang.t("Früher auf der Karte", "Earlier on the card")).clicked() {
                                logic::move_group(&mut d.config, &groups, gi, -1);
                            }
                            if style::icon_btn_small(ui, Icon::Down, lang.t("Später auf der Karte", "Later on the card")).clicked() {
                                logic::move_group(&mut d.config, &groups, gi, 1);
                            }
                        }
                    });
                }
            });
            ui.add_space(S2);
            let name_empty = d.name.trim().is_empty();
            if let Some(p) = &problem {
                widgets::error(ui, p);
            } else if name_empty {
                widgets::error(ui, lang.t("Der Name darf nicht leer sein.", "The name must not be empty."));
            }
            if let Some(e) = &d.err {
                widgets::error(ui, e);
            }
            ui.add_space(S1);
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, problem.is_none() && !name_empty, lang.t("Speichern", "Save")).clicked() {
                    save = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        for a in acts {
            match a {
                Act::MoveCol(c, dir) => {
                    logic::move_column(&mut d.config, c, dir);
                }
                Act::DelCol(c) => logic::remove_column(&mut d.config, c),
                Act::Unassign(s) => logic::unassign_status(&mut d.config, s),
                Act::Assign(c, s) => logic::assign_status(&mut d.config, c, s),
                Act::MoveStatus(c, s, dir) => {
                    logic::move_status(&mut d.config, c, s, dir);
                }
                Act::AddCol(s) => logic::add_column(&mut d.config, s),
            }
        }
        if save {
            let r = match d.id {
                Some(id) => update_board(cx.conn, id, &UpdateBoard { name: Some(d.name.clone()), description: Some(d.description.clone()), config: Some(d.config.clone()) }).map(|_| id),
                None => create_board(cx.conn, &NewBoard { project_id: pid, name: d.name.clone(), description: d.description.clone(), config: d.config.clone() }),
            };
            match r {
                Ok(id) => {
                    self.boards.stale = true;
                    self.boards.board_id = 0; // Liste neu lesen, dann `id` wählen
                    cx.db.set_meta(&format!("expert_board_{pid}"), &id.to_string());
                    self.dirty = true;
                    close = true;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.boards.cfg = Some(d);
        }
    }

    fn board_delete_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(d) = self.boards.del.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("board_delete")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 460.0));
            let title = match lang {
                crate::state::Lang::De => format!("Board {} löschen?", lang.q(&d.name)),
                crate::state::Lang::En => format!("Delete board {}?", lang.q(&d.name)),
            };
            widgets::dialog_title(ui, &title);
            ui.label(lang.t("Nur das Board wird gelöscht – die Tickets bleiben unverändert. Das lässt sich nicht rückgängig machen.", "Only the board is deleted — the tickets stay as they are. This cannot be undone."));
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if widgets::danger(ui, lang.t("Löschen", "Delete"), 0).clicked() {
                    go = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if go {
            match delete_board(cx.conn, d.id) {
                Ok(()) => {
                    self.boards.stale = true;
                    self.boards.board_id = 0;
                    self.dirty = true;
                    self.boards.notify(lang.t("Board gelöscht.", "Board deleted.").to_string(), false);
                }
                Err(e) => self.boards.notify(e.message(lang), true),
            }
        } else if !close {
            self.boards.del = Some(d);
        }
    }

    fn board_needs_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.boards.needs.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let can_submit = d.fields.iter().all(|f| f.field.as_ref().is_some_and(|f| !f.kind.is_reference()));
        let m = egui::Modal::new(Self::modal_id("board_needs")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 480.0));
            widgets::dialog_title(ui, lang.t("Pflichtfelder ausfüllen", "Fill in required fields"));
            let intro = match lang {
                crate::state::Lang::De => format!("{} „{}“ nach {}: Für diesen Statuswechsel müssen Felder ausgefüllt sein. Danach wird die Karte verschoben.", d.key, d.title, lang.q(&d.to_name)),
                crate::state::Lang::En => format!("{} “{}” to {}: this status change requires these fields. The card moves afterwards.", d.key, d.title, lang.q(&d.to_name)),
            };
            ui.label(intro);
            ui.add_space(S2);
            for (i, f) in d.fields.iter_mut().enumerate() {
                ui.label(RichText::new(f.label.text(lang)).strong());
                match &f.field {
                    Some(field) if !field.kind.is_reference() => {
                        let salt = Id::new(("board_need", i));
                        if field.kind == FieldKind::Rich {
                            let mut s = f.value.as_str().unwrap_or("").to_string();
                            if ui.add(egui::TextEdit::multiline(&mut s).id(salt).desired_rows(3).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5))).changed() {
                                f.value = Value::String(s);
                            }
                        } else {
                            values::full_editor(ui, salt, field.kind, &field.options, &mut f.value, lang);
                        }
                    }
                    _ => {
                        widgets::hint(ui, lang.t("Dieses Feld lässt sich hier nicht ausfüllen – öffne das Ticket und setze es dort.", "This field cannot be filled in here — open the ticket and set it there."));
                    }
                }
                ui.add_space(S1);
            }
            if let Some(e) = &d.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, can_submit, lang.t("Verschieben", "Move")).clicked() {
                    go = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if go {
            let entries: Vec<(i64, Value)> = d.fields.iter().map(|f| (f.label.id, f.value.clone())).collect();
            let vals = logic::collect_values(&entries);
            match transition_ticket(cx.conn, d.ticket_id, d.to_status, &vals, &Source::ui()) {
                Ok(_) => {
                    self.board_rank(cx, d.ticket_id, d.before);
                    return;
                }
                Err(TicketError::TransitionNeedsFields(missing)) => {
                    let names: Vec<&str> = missing.iter().map(|m| m.text(lang)).collect();
                    d.err = Some(match lang {
                        crate::state::Lang::De => format!("Noch nicht ausgefüllt: {}.", names.join(", ")),
                        crate::state::Lang::En => format!("Still missing: {}.", names.join(", ")),
                    });
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.boards.needs = Some(d);
        }
    }
}
