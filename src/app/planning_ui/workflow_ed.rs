//! Editor „Workflows“: welche Status ein Typ nutzt, die Übergangsmatrix (Von × Nach) und je Übergang ein Dialog für
//! Pflichtfelder, Bedingungen und Setze-Werte. Ohne definierte Übergänge gilt freie Bewegung zwischen allen Status.

use super::helpers::{color_of, has_transition, kind_label, linear_transitions, op_label, op_needs_value, ops_for, problem_text};
use super::values;
use super::widgets::{self, hint};
use super::{Cx, PlanUi, Tab};
use crate::planning::fields::{Field, FieldKind};
use crate::planning::summary::{ProjectSummary, TypeSummary};
use crate::planning::workflow::{allow_all_transitions, remove_transition, replace_transitions, set_transition, set_workflow, set_workflow_statuses, workflow_problems, Condition, ConditionOp, SetValue, TransitionDef};
use crate::planning::PlanError;
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2, S3};
use eframe::egui::{self, Id, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use serde_json::{json, Value};

#[derive(Default)]
pub(super) struct State {
    pub ty: Option<i64>,
    rules: Option<RulesDlg>,
    /// Workflow ausschalten, obwohl Tickets einen Status haben: (Typ, Zahl) → Rückfrage.
    off: Option<(i64, i64)>,
}

struct RulesDlg {
    type_id: i64,
    def: TransitionDef,
    exists: bool,
    fields: Vec<Field>,
    title: String,
    err: Option<String>,
}

impl PlanUi {
    /// Workflow eines Typs ein- oder ausschalten (beim Einschalten werden alle Status des Projekts gewählt).
    pub(super) fn toggle_workflow(&mut self, cx: &Cx, type_id: i64, on: bool) {
        match set_workflow(cx.conn, type_id, on, false) {
            Ok(()) => {
                if on {
                    if let Some(sum) = &self.sum {
                        let ids: Vec<i64> = sum.statuses.iter().map(|s| s.id).collect();
                        let _ = set_workflow_statuses(cx.conn, type_id, &ids);
                    }
                }
                self.dirty = true;
                self.msg = None;
            }
            Err(PlanError::InUse { count }) => self.workflow.off = Some((type_id, count)),
            Err(e) => self.fail(cx, &e),
        }
    }

    pub(super) fn workflow_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        let list: Vec<&TypeSummary> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
        if !self.workflow.ty.is_some_and(|s| list.iter().any(|t| t.ticket_type.id == s)) {
            self.workflow.ty = list.iter().find(|t| t.ticket_type.workflow).or(list.first()).map(|t| t.ticket_type.id);
        }
        hint(ui, lang.t(
            "Ein Workflow legt fest, welche Status ein Typ nutzt und von wo nach wo ein Ticket wandern darf.",
            "A workflow decides which statuses a type uses and where a ticket may move from and to.",
        ));
        ui.add_space(S2);
        if list.is_empty() {
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Es gibt noch keinen Typ. Lege zuerst im Reiter „Typen“ einen an.", "There is no type yet. Create one in the “Types” tab first.")).color(style::dim(ui)));
            });
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(lang.t("Typ:", "Type:")).color(style::dim(ui)));
            for t in &list {
                let tt = &t.ticket_type;
                if style::pill(ui, &tt.display(lang), None, color_of(&tt.color), self.workflow.ty == Some(tt.id)).clicked() {
                    self.workflow.ty = Some(tt.id);
                }
            }
        });
        ui.add_space(S2);
        let Some(t) = self.workflow.ty.and_then(|s| list.iter().find(|t| t.ticket_type.id == s)).copied() else { return };
        let tt = &t.ticket_type;
        if !tt.workflow {
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Dieser Typ hat keinen Workflow.", "This type has no workflow.")).strong());
                ui.label(RichText::new(lang.t("Ohne Workflow hat ein Ticket dieses Typs kein Statusfeld und erscheint nur in Liste und Suche, nicht im Board.", "Without a workflow a ticket of this type has no status field and only appears in lists and search, not on the board.")).small().color(style::dim(ui)));
                ui.add_space(S1);
                if widgets::primary(ui, lang.t("Workflow einschalten", "Enable workflow")).clicked() {
                    self.toggle_workflow(cx, tt.id, true);
                }
            });
            return;
        }
        // Status des Typs
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(lang.t("Status dieses Typs", "Statuses of this type")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::small_danger(ui, lang.t("Workflow ausschalten", "Turn workflow off")).clicked() {
                        self.toggle_workflow(cx, tt.id, false);
                    }
                });
            });
            if sum.statuses.is_empty() {
                ui.label(RichText::new(lang.t("Das Projekt hat noch keine Status.", "The project has no statuses yet.")).color(style::dim(ui)));
                if ui.button(lang.t("Zu den Status …", "Go to statuses …")).clicked() {
                    self.tab = Tab::Statuses;
                }
            }
            ui.horizontal_wrapped(|ui| {
                for s in &sum.statuses {
                    let mut on = t.workflow_statuses.contains(&s.id);
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, color_of(&s.color));
                    if widgets::check(ui, &mut on, s.display(lang)).changed() {
                        let new: Vec<i64> = sum.statuses.iter().filter(|x| if x.id == s.id { on } else { t.workflow_statuses.contains(&x.id) }).map(|x| x.id).collect();
                        let r = set_workflow_statuses(cx.conn, tt.id, &new);
                        self.run(cx, r);
                    }
                    ui.add_space(S2);
                }
            });
        });
        ui.add_space(S2);
        // Hinweise zu einem unvollständigen Workflow
        if let Ok(problems) = workflow_problems(cx.conn, tt.id) {
            let name = |id: i64| sum.status(id).map(|s| s.display(lang)).unwrap_or_default();
            for p in &problems {
                ui.label(RichText::new(format!("• {}", problem_text(p, &name, lang))).small().color(egui::Color32::from_rgb(0xe5, 0xa0, 0x4b)));
            }
            if !problems.is_empty() {
                ui.add_space(S1);
            }
        }
        if t.workflow_statuses.is_empty() {
            return;
        }
        // Übergangsmatrix
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Übergänge (Von → Nach)", "Transitions (from → to)")).strong());
            hint(ui, if t.transitions.is_empty() {
                lang.t("Kein Übergang definiert = freie Bewegung zwischen allen Status.", "No transition defined = free movement between all statuses.")
            } else {
                lang.t("Nur die angehakten Übergänge sind erlaubt. Zahnrad: Regeln je Übergang (optional).", "Only the ticked transitions are allowed. Gear: rules per transition (optional).")
            });
            ui.add_space(S1);
            self.matrix(ui, cx, &sum, t);
            ui.add_space(S2);
            ui.horizontal_wrapped(|ui| {
                if ui.button(lang.t("Alle erlauben", "Allow all")).on_hover_text(lang.t("Jeder Status zu jedem anderen (bestehende Regeln bleiben)", "Every status to every other (existing rules stay)")).clicked() {
                    let r = allow_all_transitions(cx.conn, tt.id);
                    self.run(cx, r);
                }
                if ui.button(lang.t("Linearer Ablauf", "Linear flow")).on_hover_text(lang.t("Nur zum nächsten und zum vorigen Status", "Only to the next and the previous status")).clicked() {
                    let mut defs = linear_transitions(&t.workflow_statuses);
                    for d in &mut defs {
                        if let Some(old) = t.transitions.iter().find(|x| x.def.from_status_id == d.from_status_id && x.def.to_status_id == d.to_status_id) {
                            *d = old.def.clone();
                        }
                    }
                    let r = replace_transitions(cx.conn, tt.id, &defs);
                    self.run(cx, r);
                }
                if ui.add_enabled(!t.transitions.is_empty(), egui::Button::new(lang.t("Alle entfernen (frei)", "Clear all (free)"))).on_hover_text(lang.t("Entfernt alle Übergänge samt Regeln – danach ist die Bewegung frei", "Removes all transitions and their rules – movement is free afterwards")).clicked() {
                    let r = replace_transitions(cx.conn, tt.id, &[]);
                    self.run(cx, r);
                }
            });
        });
    }

    /// Das Raster Von × Nach (von Hand gezeichnet, damit die Spalten exakt unter ihren Überschriften liegen).
    fn matrix(&mut self, ui: &mut Ui, cx: &Cx, sum: &ProjectSummary, t: &TypeSummary) {
        const LABEL_W: f32 = 150.0;
        const CELL_W: f32 = 84.0;
        const ROW_H: f32 = 34.0;
        const HEAD_H: f32 = 30.0;
        let lang = cx.lang;
        let ids = &t.workflow_statuses;
        let defs: Vec<TransitionDef> = t.transitions.iter().map(|x| x.def.clone()).collect();
        let short = |s: &str, n: usize| -> String { if s.chars().count() > n { s.chars().take(n - 1).chain(std::iter::once('…')).collect() } else { s.to_string() } };
        let name_of = |id: i64| sum.status(id).map(|s| s.display(lang)).unwrap_or_default();
        let color_of_status = |id: i64| sum.status(id).map(|s| color_of(&s.color)).unwrap_or(egui::Color32::GRAY);
        let size = Vec2::new(LABEL_W + CELL_W * ids.len() as f32, HEAD_H + ROW_H * ids.len() as f32);
        egui::ScrollArea::horizontal().id_salt("matrix_scroll").auto_shrink([false, true]).show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let p = ui.painter().clone();
            let dim = style::dim(ui);
            let strong = ui.visuals().strong_text_color();
            p.text(Pos2::new(rect.left() + 8.0, rect.top() + HEAD_H / 2.0), egui::Align2::LEFT_CENTER, lang.t("Von ↓   Nach →", "From ↓   To →"), egui::FontId::proportional(11.5), dim);
            for (j, to) in ids.iter().enumerate() {
                let c = Pos2::new(rect.left() + LABEL_W + CELL_W * j as f32 + CELL_W / 2.0, rect.top() + HEAD_H / 2.0);
                p.text(c, egui::Align2::CENTER_CENTER, short(&name_of(*to), 11), egui::FontId::proportional(12.5), color_of_status(*to));
                let hr = Rect::from_center_size(c, Vec2::new(CELL_W - 6.0, HEAD_H));
                ui.interact(hr, Id::new(("mx_head", t.ticket_type.id, *to)), Sense::hover()).on_hover_text(name_of(*to));
            }
            for (i, from) in ids.iter().enumerate() {
                let y = rect.top() + HEAD_H + ROW_H * i as f32;
                let row = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(size.x, ROW_H));
                if i % 2 == 0 {
                    p.rect_filled(row, 6.0, ui.visuals().widgets.noninteractive.bg_fill.gamma_multiply(0.5));
                }
                p.circle_filled(Pos2::new(rect.left() + 14.0, row.center().y), 4.5, color_of_status(*from));
                p.text(Pos2::new(rect.left() + 26.0, row.center().y), egui::Align2::LEFT_CENTER, short(&name_of(*from), 16), egui::FontId::proportional(13.5), strong);
                for (j, to) in ids.iter().enumerate() {
                    let cell = Rect::from_min_size(Pos2::new(rect.left() + LABEL_W + CELL_W * j as f32, y), Vec2::new(CELL_W, ROW_H));
                    if from == to {
                        p.line_segment([cell.center() - Vec2::new(5.0, 0.0), cell.center() + Vec2::new(5.0, 0.0)], Stroke::new(1.0_f32, dim.gamma_multiply(0.5)));
                        continue;
                    }
                    let has = has_transition(&defs, *from, *to);
                    let boxr = Rect::from_center_size(cell.center() - Vec2::new(if has { 14.0 } else { 0.0 }, 0.0), Vec2::splat(22.0));
                    let resp = ui.interact(boxr.expand(3.0), Id::new(("mx_cell", t.ticket_type.id, *from, *to)), Sense::click());
                    widgets::paint_box(ui, boxr, has, resp.hovered());
                    let tip = format!("{} → {}", name_of(*from), name_of(*to));
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip).clicked() {
                        let r = if has {
                            remove_transition(cx.conn, t.ticket_type.id, *from, *to)
                        } else {
                            set_transition(cx.conn, t.ticket_type.id, &TransitionDef { from_status_id: *from, to_status_id: *to, ..Default::default() }).map(|_| ())
                        };
                        self.run(cx, r);
                    }
                    if has {
                        let def = defs.iter().find(|d| d.from_status_id == *from && d.to_status_id == *to).cloned().unwrap_or_default();
                        let has_rules = !def.required_fields.is_empty() || !def.conditions.is_empty() || !def.set_values.is_empty();
                        let gr = Rect::from_center_size(cell.center() + Vec2::new(16.0, 0.0), Vec2::splat(24.0));
                        let gresp = ui.interact(gr, Id::new(("mx_gear", t.ticket_type.id, *from, *to)), Sense::click());
                        if gresp.hovered() {
                            p.rect_filled(gr, 6.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        style::paint_icon(&p, SIcon::Gear, gr.center(), if has_rules { style::accent() } else { dim });
                        if has_rules {
                            p.circle_filled(gr.right_top() + Vec2::new(-3.0, 4.0), 3.0, style::accent());
                        }
                        if gresp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(lang.t("Regeln für diesen Übergang …", "Rules for this transition …")).clicked() {
                            self.open_rules(sum, t, def, lang);
                        }
                    }
                }
            }
        });
    }

    fn open_rules(&mut self, sum: &ProjectSummary, t: &TypeSummary, def: TransitionDef, lang: Lang) {
        let name = |id: i64| sum.status(id).map(|s| s.display(lang)).unwrap_or_default();
        let title = format!("{}: {} → {}", t.ticket_type.display(lang), name(def.from_status_id), name(def.to_status_id));
        let exists = t.transitions.iter().any(|x| x.def.from_status_id == def.from_status_id && x.def.to_status_id == def.to_status_id);
        self.workflow.rules = Some(RulesDlg { type_id: t.ticket_type.id, def, exists, fields: t.fields.clone(), title, err: None });
    }

    // ------------------------------------------------------------------ Dialoge

    pub(super) fn workflow_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        // Workflow ausschalten, obwohl Tickets einen Status haben
        if let Some((tid, count)) = self.workflow.off {
            let mut go = false;
            let mut close = false;
            let m = egui::Modal::new(Self::modal_id("wf_off")).show(ctx, |ui| {
                ui.set_width(widgets::modal_width(ctx, 440.0));
                widgets::dialog_title(ui, lang.t("Workflow ausschalten?", "Turn the workflow off?"));
                ui.label(match lang {
                    Lang::De => format!("{count} Ticket(s) dieses Typs haben einen Status. Beim Ausschalten wird dieser Status verworfen."),
                    Lang::En => format!("{count} ticket(s) of this type have a status. Turning the workflow off discards that status."),
                });
                ui.add_space(S3);
                ui.horizontal(|ui| {
                    if widgets::danger(ui, lang.t("Ausschalten", "Turn off"), 0).clicked() {
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
                let r = set_workflow(cx.conn, tid, false, true);
                self.run(cx, r);
                self.workflow.off = None;
            } else if close {
                self.workflow.off = None;
            }
        }
        // Regeln eines Übergangs
        let Some(mut d) = self.workflow.rules.take() else { return };
        let mut save = false;
        let mut remove = false;
        let mut close = false;
        let m = egui::Modal::new(Self::modal_id("rules")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 640.0));
            widgets::dialog_title(ui, lang.t("Regeln für den Übergang", "Transition rules"));
            ui.label(RichText::new(&d.title).strong());
            hint(ui, lang.t("Alles hier ist optional und unabhängig voneinander nutzbar. Die Regeln gelten für Oberfläche, Quick-Menü und Agenten gleichermaßen.", "Everything here is optional and independent. The rules apply to the UI, the quick menu and agents alike."));
            ui.add_space(S2);
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 300.0).max(220.0)).auto_shrink([true, true]).show(ui, |ui| {
                rules_form(ui, &mut d, lang);
            });
            ui.add_space(S2);
            if let Some(e) = &d.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Speichern", "Save")).clicked() {
                    save = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
                if d.exists && widgets::small_danger(ui, lang.t("Übergang entfernen", "Remove transition")).clicked() {
                    remove = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if save {
            match set_transition(cx.conn, d.type_id, &d.def) {
                Ok(_) => {
                    self.dirty = true;
                    close = true;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        if remove {
            match remove_transition(cx.conn, d.type_id, d.def.from_status_id, d.def.to_status_id) {
                Ok(()) => {
                    self.dirty = true;
                    close = true;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.workflow.rules = Some(d);
        }
    }
}

fn field_label(f: &Field, lang: Lang) -> String {
    format!("{} ({})", f.display(lang), kind_label(f.kind, lang))
}

fn rules_form(ui: &mut Ui, d: &mut RulesDlg, lang: Lang) {
    if d.fields.is_empty() {
        hint(ui, lang.t("Dieser Typ hat keine Felder – Regeln brauchen Felder.", "This type has no fields – rules need fields."));
        return;
    }
    // Pflichtfelder
    widgets::section(ui, lang.t("Pflichtfelder beim Übergang", "Required fields on transition"));
    hint(ui, lang.t("Ein Dialog fragt diese Felder ab, bevor das Ticket wechselt.", "A dialog asks for these fields before the ticket moves."));
    for f in &d.fields {
        let mut on = d.def.required_fields.contains(&f.id);
        if widgets::check(ui, &mut on, field_label(f, lang)).changed() {
            if on {
                d.def.required_fields.push(f.id);
            } else {
                d.def.required_fields.retain(|x| *x != f.id);
            }
        }
    }
    ui.add_space(S2);
    // Bedingungen
    widgets::section(ui, lang.t("Bedingungen", "Conditions"));
    hint(ui, lang.t(
        "Wird der Reihe nach geprüft: Die erste zutreffende entscheidet. Gibt es eine „erlauben“-Bedingung und keine trifft zu, ist der Übergang gesperrt.",
        "Checked in order: the first one that matches decides. If an “allow” condition exists and none matches, the transition is blocked.",
    ));
    let mut remove: Option<usize> = None;
    for i in 0..d.def.conditions.len() {
        ui.horizontal_wrapped(|ui| {
            let c = &mut d.def.conditions[i];
            let kind = d.fields.iter().find(|f| f.id == c.field_id).map(|f| f.kind).unwrap_or(FieldKind::Text);
            let sel = d.fields.iter().find(|f| f.id == c.field_id).map(|f| f.display(lang)).unwrap_or_default();
            let before = c.field_id;
            egui::ComboBox::from_id_salt(("cond_field", i)).selected_text(sel).width(150.0).show_ui(ui, |ui| {
                for f in &d.fields {
                    ui.selectable_value(&mut c.field_id, f.id, f.display(lang));
                }
            });
            let fld = d.fields.iter().find(|f| f.id == c.field_id).cloned();
            let kind = if c.field_id != before { fld.as_ref().map(|f| f.kind).unwrap_or(kind) } else { kind };
            let opts = fld.as_ref().map(|f| f.options.clone()).unwrap_or_default();
            if c.field_id != before {
                c.op = ops_for(kind)[0];
                c.value = values::blank(kind, &opts);
            }
            let ops = ops_for(kind);
            if !ops.contains(&c.op) {
                c.op = ops[0];
            }
            let before_op = c.op;
            egui::ComboBox::from_id_salt(("cond_op", i)).selected_text(op_label(c.op, lang)).width(130.0).show_ui(ui, |ui| {
                for o in &ops {
                    ui.selectable_value(&mut c.op, *o, op_label(*o, lang));
                }
            });
            let list_op = matches!(c.op, ConditionOp::In | ConditionOp::NotIn);
            if c.op != before_op {
                let was_list = matches!(before_op, ConditionOp::In | ConditionOp::NotIn);
                if list_op && !was_list {
                    c.value = json!([]);
                } else if !list_op && was_list {
                    c.value = values::blank(kind, &opts);
                }
            }
            if op_needs_value(c.op) {
                if list_op {
                    if !c.value.is_array() {
                        c.value = json!([]);
                    }
                    values::list_editor(ui, Id::new(("cond_val", i)), kind, &opts, &mut c.value, lang);
                } else {
                    if c.value.is_null() || c.value.is_array() {
                        c.value = values::blank(kind, &opts);
                    }
                    values::element_editor(ui, Id::new(("cond_val", i)), kind, &opts, &mut c.value, lang);
                }
            }
            egui::ComboBox::from_id_salt(("cond_allow", i)).selected_text(if c.allow { lang.t("→ erlauben", "→ allow") } else { lang.t("→ sperren", "→ block") }).width(110.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.allow, true, lang.t("→ erlauben", "→ allow"));
                ui.selectable_value(&mut c.allow, false, lang.t("→ sperren", "→ block"));
            });
            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Bedingung entfernen", "Remove condition")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        d.def.conditions.remove(i);
    }
    if ui.button(format!("+ {}", lang.t("Bedingung", "Add condition"))).clicked() {
        let f = &d.fields[0];
        let ops = ops_for(f.kind);
        d.def.conditions.push(Condition { field_id: f.id, op: ops[0], value: if op_needs_value(ops[0]) { values::blank(f.kind, &f.options) } else { Value::Null }, allow: true });
    }
    ui.add_space(S2);
    // Setze-Werte
    widgets::section(ui, lang.t("Werte beim Übergang setzen", "Set values on transition"));
    hint(ui, lang.t("Diese Felder bekommen beim Übergang automatisch den Wert (z. B. Fehler → Erledigt: Lösung = Behoben).", "These fields get the value automatically on transition (e.g. defect → done: resolution = fixed)."));
    let settable: Vec<Field> = d.fields.iter().filter(|f| !f.kind.is_reference()).cloned().collect();
    let mut remove: Option<usize> = None;
    for i in 0..d.def.set_values.len() {
        ui.horizontal_wrapped(|ui| {
            let s = &mut d.def.set_values[i];
            let before = s.field_id;
            let sel = settable.iter().find(|f| f.id == s.field_id).map(|f| f.display(lang)).unwrap_or_default();
            egui::ComboBox::from_id_salt(("set_field", i)).selected_text(sel).width(170.0).show_ui(ui, |ui| {
                for f in &settable {
                    ui.selectable_value(&mut s.field_id, f.id, f.display(lang));
                }
            });
            let fld = settable.iter().find(|f| f.id == s.field_id).cloned();
            if let Some(f) = &fld {
                if s.field_id != before {
                    s.value = values::blank_full(f.kind, &f.options);
                }
                ui.label("=");
                values::full_editor(ui, Id::new(("set_val", i)), f.kind, &f.options, &mut s.value, lang);
            }
            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Eintrag entfernen", "Remove entry")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        d.def.set_values.remove(i);
    }
    if ui.add_enabled(!settable.is_empty(), egui::Button::new(format!("+ {}", lang.t("Wert setzen", "Add value to set")))).clicked() {
        let used: Vec<i64> = d.def.set_values.iter().map(|s| s.field_id).collect();
        if let Some(f) = settable.iter().find(|f| !used.contains(&f.id)).or(settable.first()) {
            d.def.set_values.push(SetValue { field_id: f.id, value: values::blank_full(f.kind, &f.options) });
        }
    }
}
