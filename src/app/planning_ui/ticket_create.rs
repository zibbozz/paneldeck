//! Anlege-Dialog für Tickets: Typauswahl, Titel und alle Felder des Typs in der konfigurierten Reihenfolge und in Abschnitten
//! (Pflichtfelder markiert, Standardwerte vorbelegt, Hilfetexte), Live-Prüfung über die Datenschicht, Haken „Weiteres anlegen“,
//! Enter im Titel bzw. Strg+Enter speichert.

use super::helpers::color_of;
use super::ticket_data::label_suggestions;
use super::ticket_draft::{group_sections, Draft};
use super::ticket_inputs::{field_block, ticket_search, InCx};
use super::widgets;
use super::{Cx, PlanUi};
use crate::planning::fields::Field;
use crate::planning::history::Source;
use crate::planning::summary::{ProjectSummary, TypeSummary};
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::{clean_title, create_ticket, get_ticket, NewTicket};
use crate::planning::values::{is_empty, normalize_value, RefCtx};
use crate::state::Lang;
use crate::style::{S1, S2, S3};
use eframe::egui::{self, Id, Key, Modifiers, RichText, Sense, Vec2};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;

pub(super) struct CreateDlg {
    pub project: i64,
    pub type_id: i64,
    pub title: String,
    /// Eltern-Ticket (bei „Kind anlegen“ fest).
    pub parent: Option<i64>,
    pub parent_fixed: bool,
    pub parent_text: String,
    pub drafts: HashMap<i64, Draft>,
    /// Nach dem ersten Versuch zu speichern zeigen leere Pflichtfelder ihre Meldung.
    pub tried: bool,
    pub err: Option<String>,
    pub focus_title: bool,
    pub salt: u32,
}

/// Ergebnis der Prüfung: Meldungen je Feld/Titel und die Werte, die gesendet werden.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Check {
    pub title: Option<String>,
    pub fields: HashMap<i64, String>,
    pub values: Vec<(i64, Value)>,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.title.is_none() && self.fields.is_empty()
    }
}

/// Prüft Titel und Eingaben gegen die Datenschicht. Leere Pflichtfelder melden sich nur mit `show_required`. Es werden nur
/// Felder gesendet, die der Nutzer verändert hat (alle anderen bekommen ihren Standardwert von der Datenschicht).
pub(super) fn check_create(conn: &Connection, project_id: i64, fields: &[Field], title: &str, drafts: &HashMap<i64, Draft>, show_required: bool, lang: Lang) -> Check {
    let mut c = Check::default();
    if !title.trim().is_empty() || show_required {
        if let Err(e) = clean_title(title) {
            c.title = Some(e.message(lang));
        }
    }
    let ctx = RefCtx { project_id, ticket_id: None };
    for f in fields {
        let Some(d) = drafts.get(&f.id) else { continue };
        let v = match d.to_value(f) {
            Ok(v) => v,
            Err(e) => {
                c.fields.insert(f.id, e.message(lang));
                continue;
            }
        };
        match normalize_value(conn, &ctx, f, &v) {
            Err(e) => {
                c.fields.insert(f.id, e.message(lang));
            }
            Ok(n) => {
                if is_empty(n.as_ref()) {
                    if f.required && (d.edited || f.default_value.is_none()) {
                        if show_required || d.edited {
                            c.fields.insert(f.id, lang.t("Pflichtfeld – bitte ausfüllen.", "Required – please fill in.").to_string());
                        }
                    } else if d.edited && f.default_value.is_some() {
                        // bewusst geleert: überstimmt den Standardwert
                        c.values.push((f.id, Value::Null));
                    }
                } else if d.edited {
                    if let Some(n) = n {
                        c.values.push((f.id, n));
                    }
                }
            }
        }
    }
    c
}

/// Felder des Typs, die der Anlege-Dialog zeigt (alle; Verweise auf Späteres bleiben sichtbar, aber ohne Eingabe).
fn drafts_for(fields: &[Field]) -> HashMap<i64, Draft> {
    fields.iter().map(|f| (f.id, Draft::from_value(f, f.default_value.as_ref()))).collect()
}

impl CreateDlg {
    pub fn new(sum: &ProjectSummary, type_id: Option<i64>, parent: Option<i64>) -> Option<CreateDlg> {
        let types: Vec<&TypeSummary> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
        let ty = type_id.and_then(|id| types.iter().find(|t| t.ticket_type.id == id)).or(types.first())?;
        Some(CreateDlg {
            project: sum.project.id,
            type_id: ty.ticket_type.id,
            title: String::new(),
            parent,
            parent_fixed: parent.is_some(),
            parent_text: String::new(),
            drafts: drafts_for(&ty.fields),
            tried: false,
            err: None,
            focus_title: true,
            salt: 0,
        })
    }

    fn set_type(&mut self, ty: &TypeSummary) {
        self.type_id = ty.ticket_type.id;
        self.drafts = drafts_for(&ty.fields);
        self.err = None;
    }

    /// Hat der Nutzer schon etwas eingegeben (Titel oder ein verändertes Feld)?
    pub fn has_draft(&self) -> bool {
        !self.title.trim().is_empty() || self.drafts.values().any(|d| d.edited)
    }

    /// Titel und Felder leeren (für „Weiteres anlegen“); Typ und Eltern bleiben.
    fn reset(&mut self, ty: &TypeSummary) {
        self.title.clear();
        self.drafts = drafts_for(&ty.fields);
        self.tried = false;
        self.err = None;
        self.focus_title = true;
        self.salt = self.salt.wrapping_add(1);
    }
}

/// Typen, die unter dem Eltern-Ticket stehen dürfen (ohne Eltern: alle).
pub(super) fn allowed_types<'a>(sum: &'a ProjectSummary, parent_type: Option<i64>) -> Vec<&'a TypeSummary> {
    sum.types.iter().filter(|t| !t.ticket_type.system).filter(|t| parent_type.is_none_or(|p| t.parents.contains(&p))).collect()
}

impl PlanUi {
    /// Dialog „Neues Ticket“ öffnen (`parent` = Kind unter diesem Ticket anlegen).
    pub(super) fn open_create(&mut self, cx: &Cx, parent: Option<i64>) {
        let Some(sum) = self.sum.clone() else { return };
        if sum.project.system {
            return;
        }
        let mut sum_use = sum.clone();
        let mut parent_type = None;
        if let Some(p) = parent {
            match get_ticket(cx.conn, p) {
                Ok(t) => {
                    parent_type = Some(t.type_id);
                    if t.project_id != sum.project.id {
                        match crate::planning::summary::project_summary(cx.conn, t.project_id) {
                            Ok(s) => sum_use = std::rc::Rc::new(s),
                            Err(_) => return,
                        }
                    }
                }
                Err(e) => {
                    self.t_fail(cx, &e);
                    return;
                }
            }
        }
        let allowed = allowed_types(&sum_use, parent_type);
        let first = allowed.first().map(|t| t.ticket_type.id);
        if first.is_none() {
            self.msg_error(cx, lang_no_types(cx.lang));
            return;
        }
        // Der zuletzt gewählte Typ bleibt Standard, solange er erlaubt ist.
        let pick = self.tickets.create_last_type.filter(|id| allowed.iter().any(|t| t.ticket_type.id == *id)).or(first);
        self.tickets.labels = label_suggestions(cx.conn, sum_use.project.id);
        self.tickets.create = CreateDlg::new(&sum_use, pick, parent);
    }

    pub(super) fn create_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut dlg) = self.tickets.create.take() else { return };
        let lang = cx.lang;
        let sum = match self.sum.clone().filter(|s| s.project.id == dlg.project) {
            Some(s) => s,
            None => match crate::planning::summary::project_summary(cx.conn, dlg.project) {
                Ok(s) => std::rc::Rc::new(s),
                Err(_) => return,
            },
        };
        let parent_ticket = dlg.parent.and_then(|p| get_ticket(cx.conn, p).ok());
        let allowed: Vec<TypeSummary> = allowed_types(&sum, parent_ticket.as_ref().map(|t| t.type_id)).into_iter().cloned().collect();
        let Some(ty) = allowed.iter().find(|t| t.ticket_type.id == dlg.type_id).or(allowed.first()).cloned() else { return };
        if ty.ticket_type.id != dlg.type_id {
            dlg.set_type(&ty);
        }
        let prefixes: Vec<String> = self.projects.iter().map(|p| p.key.clone()).collect();
        let labels = std::mem::take(&mut self.tickets.labels);
        let mut another = self.tickets.create_another;
        let mut submit = false;
        let mut close = false;
        let mut new_type: Option<TypeSummary> = None;
        let check = check_create(cx.conn, dlg.project, &ty.fields, &dlg.title, &dlg.drafts, dlg.tried, lang);
        let m = egui::Modal::new(Self::modal_id("ticket_create")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 660.0));
            let title = match &parent_ticket {
                Some(p) => match lang {
                    Lang::De => format!("Kind-Ticket zu {} anlegen", p.key()),
                    Lang::En => format!("New child of {}", p.key()),
                },
                None => lang.t("Neues Ticket", "New ticket").to_string(),
            };
            widgets::dialog_title(ui, &title);
            // Typ
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(160.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Typ", "Type")).color(ui.visuals().weak_text_color()));
                });
                let cur = ty.ticket_type.display(lang);
                let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                widgets::paint_type_badge(ui.painter(), r, ty.ticket_type.icon.as_deref(), color_of(&ty.ticket_type.color));
                egui::ComboBox::from_id_salt("create_type").selected_text(cur).width(220.0).show_ui(ui, |ui| {
                    for t in &allowed {
                        let col = color_of(&t.ticket_type.color);
                        if super::ticket_inputs::dot_item(ui, col, &t.ticket_type.display(lang), t.ticket_type.id == dlg.type_id).clicked() {
                            new_type = Some(t.clone());
                            ui.close();
                        }
                    }
                });
            });
            ui.add_space(S2);
            // Titel
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(160.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Titel", "Title")).strong());
                    ui.label(RichText::new("*").color(widgets::DANGER).strong());
                });
                let id = Id::new(("create_title", dlg.salt));
                let r = ui.add_sized([ui.available_width().min(460.0), 28.0], egui::TextEdit::singleline(&mut dlg.title).id(id).hint_text(lang.t("Kurzer, aussagekräftiger Titel", "A short, meaningful title")).margin(egui::Margin::symmetric(8, 5)));
                if dlg.focus_title {
                    r.request_focus();
                    dlg.focus_title = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            });
            if let Some(e) = &check.title {
                ui.horizontal(|ui| {
                    ui.add_space(160.0);
                    widgets::error(ui, e);
                });
            }
            ui.add_space(S2);
            // Eltern-Ticket (nur wenn der Typ Eltern haben darf und nicht fest vorgegeben)
            if let Some(p) = &parent_ticket {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(Vec2::new(160.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(RichText::new(lang.t("Eltern-Ticket", "Parent")).color(ui.visuals().weak_text_color()));
                    });
                    ui.label(RichText::new(p.key()).monospace());
                    ui.label(&p.title);
                });
                ui.add_space(S2);
            } else if !ty.parents.is_empty() {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(Vec2::new(160.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(RichText::new(lang.t("Eltern-Ticket", "Parent")).color(ui.visuals().weak_text_color()));
                    });
                    ui.vertical(|ui| match dlg.parent.and_then(|p| get_ticket(cx.conn, p).ok()) {
                        Some(p) => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(p.key()).monospace());
                                ui.label(&p.title);
                                if ui.small_button("✕").on_hover_text(lang.t("Eltern-Ticket entfernen", "Remove the parent")).clicked() {
                                    dlg.parent = None;
                                }
                            });
                        }
                        None => {
                            let ic = InCx { conn: cx.conn, lang, project_id: dlg.project, ticket_id: None, prefixes: &prefixes, labels: &labels, id: Id::new("create_parent") };
                            let hint = lang.t("Optional: Eltern-Ticket suchen …", "Optional: search a parent ticket …");
                            if let Some(t) = ticket_search(ui, &ic, Id::new("create_parent_search"), &mut dlg.parent_text, Some(dlg.project), &ty.parents, &[], hint) {
                                dlg.parent = Some(t.id);
                            }
                        }
                    });
                });
                ui.add_space(S2);
            }
            ui.separator();
            ui.add_space(S2);
            // Felder
            let max_h = (ctx.content_rect().height() * 0.55).clamp(200.0, 560.0);
            egui::ScrollArea::vertical().id_salt("create_scroll").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                let ic = InCx { conn: cx.conn, lang, project_id: dlg.project, ticket_id: None, prefixes: &prefixes, labels: &labels, id: Id::new(("create", dlg.salt)) };
                if ty.fields.is_empty() {
                    widgets::hint(ui, lang.t("Dieser Typ hat keine weiteren Felder.", "This type has no further fields."));
                }
                for (name, idxs) in group_sections(&ty.fields) {
                    if !name.is_empty() {
                        ui.add_space(S1);
                        ui.label(RichText::new(&name).strong());
                        ui.separator();
                    }
                    for i in idxs {
                        let f = &ty.fields[i];
                        let Some(d) = dlg.drafts.get_mut(&f.id) else { continue };
                        let ev = field_block(ui, &ic, f, d, false, check.fields.get(&f.id).map(String::as_str));
                        if ev.enter {
                            submit = true;
                        }
                    }
                }
            });
            ui.add_space(S2);
            if let Some(e) = &dlg.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.separator();
            ui.add_space(S1);
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Anlegen", "Create")).on_hover_text(lang.t("Enter im Titel oder Strg+Enter", "Enter in the title or Ctrl+Enter")).clicked() {
                    submit = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
                ui.add_space(S3);
                widgets::check(ui, &mut another, lang.t("Weiteres anlegen", "Create another")).on_hover_text(lang.t("Nach dem Anlegen bleibt der Dialog offen für das nächste Ticket.", "Keeps the dialog open for the next ticket after creating."));
            });
        });
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            submit = true;
        }
        // Ein Klick neben den Dialog verwirft keinen angefangenen Entwurf (ein Fehlklick würde Titel und Beschreibung kosten).
        if super::ticket_dialogs::modal_closes(ctx, &m, dlg.has_draft()) {
            close = true;
        }
        self.tickets.create_another = another;
        self.tickets.labels = labels;
        if let Some(t) = new_type {
            dlg.set_type(&t);
            dlg.parent = dlg.parent.filter(|_| dlg.parent_fixed);
        }
        if close {
            return;
        }
        if submit {
            dlg.tried = true;
            let check = check_create(cx.conn, dlg.project, &ty.fields, &dlg.title, &dlg.drafts, true, lang);
            if !check.ok() {
                dlg.err = Some(lang.t("Bitte die markierten Angaben korrigieren.", "Please correct the marked entries.").to_string());
            } else {
                let n = NewTicket { project_id: dlg.project, type_id: dlg.type_id, title: dlg.title.clone(), parent_id: dlg.parent, values: check.values };
                match create_ticket(cx.conn, &n, &Source::ui()) {
                    Ok((t, op)) => {
                        self.tickets.commit(Some(op));
                        self.dirty = true;
                        self.msg = None;
                        self.tickets.create_last_type = Some(dlg.type_id);
                        let key = t.key();
                        self.tickets.toast(match lang {
                            Lang::De => format!("Ticket {key} angelegt."),
                            Lang::En => format!("Ticket {key} created."),
                        });
                        if another {
                            dlg.reset(&ty);
                        } else {
                            if dlg.parent_fixed {
                                // Der Anlege-Dialog kam aus dem Eltern-Ticket: dort bleiben, die Kinder-Liste zeigt das neue.
                            } else {
                                self.tickets.open_ticket(t.id, false);
                            }
                            return;
                        }
                    }
                    Err(e) => {
                        dlg.err = Some(e.message(lang));
                        mark_error(&mut dlg, &e);
                    }
                }
            }
        }
        self.tickets.create = Some(dlg);
    }
}

fn lang_no_types(lang: Lang) -> String {
    lang.t("Das Projekt hat noch keinen Ticket-Typ, der hier passt. Lege zuerst Typen an.", "The project has no ticket type that fits here yet. Create types first.").to_string()
}

/// Fehler der Datenschicht dem betroffenen Feld zuordnen.
fn mark_error(dlg: &mut CreateDlg, e: &TicketError) {
    let ids: Vec<i64> = match e {
        TicketError::RequiredMissing(v) => v.iter().map(|l| l.id).collect(),
        TicketError::ValueInvalid(l) | TicketError::RefInvalid(l) => vec![l.id],
        _ => Vec::new(),
    };
    for id in ids {
        if let Some(d) = dlg.drafts.get_mut(&id) {
            d.edited = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{FieldKind, FieldOptions};
    use crate::planning::tickets::fixture::Fx;
    use serde_json::json;

    fn fields_of(f: &Fx, ty: i64) -> Vec<Field> {
        crate::planning::fields::list_fields(&f.c, ty).unwrap()
    }

    fn defaults(fields: &[Field]) -> HashMap<i64, Draft> {
        drafts_for(fields)
    }

    #[test]
    fn untouched_fields_use_the_defaults_and_send_nothing() {
        let f = Fx::new();
        let fields = fields_of(&f, f.story);
        let d = defaults(&fields);
        let c = check_create(&f.c, f.project, &fields, "Titel", &d, true, Lang::En);
        assert!(c.ok(), "{c:?}");
        assert!(c.values.is_empty(), "nichts verändert → die Datenschicht setzt die Standardwerte: {:?}", c.values);
        // und die Datenschicht legt damit an
        let (t, _) = create_ticket(&f.c, &NewTicket { values: c.values, ..NewTicket::new(f.project, f.story, "Titel") }, &Source::ui()).unwrap();
        let prio = f.field(f.story, "priority");
        assert_eq!(crate::planning::values::get_value(&f.c, t.id, &prio).unwrap(), prio.default_value);
    }

    #[test]
    fn empty_title_and_required_fields_are_reported_after_the_first_try() {
        let f = Fx::new();
        let req = f.add_field(f.task, "Zuständig", FieldKind::Text, FieldOptions::default(), true);
        let fields = fields_of(&f, f.task);
        let d = defaults(&fields);
        let live = check_create(&f.c, f.project, &fields, "", &d, false, Lang::En);
        assert!(live.ok(), "beim Öffnen keine Fehlermeldungen: {live:?}");
        let tried = check_create(&f.c, f.project, &fields, "", &d, true, Lang::En);
        assert!(tried.title.is_some());
        assert!(tried.fields.contains_key(&req.id), "{tried:?}");
        assert!(tried.fields[&req.id].contains("Required"));
        let de = check_create(&f.c, f.project, &fields, "", &d, true, Lang::De);
        assert!(de.fields[&req.id].contains("Pflichtfeld"));
    }

    #[test]
    fn edited_values_are_validated_and_sent() {
        let f = Fx::new();
        let mut opts = FieldOptions::default();
        opts.min = Some(1.0);
        opts.max = Some(10.0);
        let n = f.add_field(f.task, "Aufwand", FieldKind::Number, opts, false);
        let fields = fields_of(&f, f.task);
        let mut d = defaults(&fields);
        {
            let dr = d.get_mut(&n.id).unwrap();
            dr.text = "5".into();
            dr.edited = true;
        }
        let c = check_create(&f.c, f.project, &fields, "T", &d, false, Lang::En);
        assert!(c.ok());
        assert_eq!(c.values, vec![(n.id, json!(5))]);
        d.get_mut(&n.id).unwrap().text = "50".into();
        let c = check_create(&f.c, f.project, &fields, "T", &d, false, Lang::En);
        assert!(c.fields.contains_key(&n.id), "außerhalb der Grenzen: {c:?}");
        d.get_mut(&n.id).unwrap().text = "viel".into();
        let c = check_create(&f.c, f.project, &fields, "T", &d, false, Lang::En);
        assert!(c.fields[&n.id].contains("not a number"), "{c:?}");
    }

    #[test]
    fn a_cleared_default_is_sent_as_empty_on_purpose() {
        let f = Fx::new();
        let fields = fields_of(&f, f.task);
        let prio = f.field(f.task, "priority");
        let mut d = defaults(&fields);
        let dr = d.get_mut(&prio.id).unwrap();
        dr.value = Value::Null;
        dr.edited = true;
        let c = check_create(&f.c, f.project, &fields, "T", &d, true, Lang::En);
        assert_eq!(c.values, vec![(prio.id, Value::Null)], "bewusst leer überstimmt den Standardwert");
        let (t, _) = create_ticket(&f.c, &NewTicket { values: c.values, ..NewTicket::new(f.project, f.task, "T") }, &Source::ui()).unwrap();
        assert_eq!(crate::planning::values::get_value(&f.c, t.id, &prio).unwrap(), None);
    }

    #[test]
    fn child_dialogs_only_offer_allowed_types() {
        let f = Fx::new();
        let sum = crate::planning::summary::project_summary(&f.c, f.project).unwrap();
        let all = allowed_types(&sum, None);
        assert_eq!(all.len(), 5);
        let under_epic = allowed_types(&sum, Some(f.epic));
        let ids: Vec<i64> = under_epic.iter().map(|t| t.ticket_type.id).collect();
        assert!(ids.contains(&f.story) && ids.contains(&f.task) && ids.contains(&f.defect));
        assert!(!ids.contains(&f.epic) && !ids.contains(&f.subtask));
        let dlg = CreateDlg::new(&sum, None, None).unwrap();
        assert!(dlg.drafts.len() >= 2);
        let dlg = CreateDlg::new(&sum, Some(f.story), Some(1)).unwrap();
        assert_eq!((dlg.type_id, dlg.parent, dlg.parent_fixed), (f.story, Some(1), true));
    }
}
