//! Dialoge der Ticket-Ansicht: Pflichtfelder beim Statuswechsel, Löschen mit Sicherheitstimer, Klonen mit Vorschau,
//! Typ ändern / in ein anderes Projekt verschieben, Zeit erfassen, Todo in Ticket umwandeln, Bestätigungen.

use super::helpers::color_of;
use super::ref_inputs::{release_label, terminal_text};
use super::ticket_draft::{group_sections, Draft};
use super::ticket_inputs::{dot_item, field_block, InCx};
use super::ticket_state::countdown_secs;
use super::values::show_value;
use super::widgets;
use super::{Cx, PlanUi};
use crate::calc::duration::{format_duration, parse_duration, DEFAULT_WORK_HOURS_PER_DAY};
use crate::markdown::{rich_text_field, FieldMode, FieldOpts};
use crate::planning::clone_move::{change_type, change_type_preview, clone_ticket, ChangeTypePreview, CloneOptions, ClonePreviewRow};
use crate::planning::comments::delete_comment;
use crate::planning::fields::{list_fields, Field, FieldKind};
use crate::planning::history::Source;
use crate::planning::projects::list_projects;
use crate::planning::sprints::releases::ReleaseKind;
use crate::planning::ticket_error::{FieldLabel, TicketError};
use crate::planning::tickets::{delete_impact, delete_ticket, get_ticket, DeleteImpact};
use crate::planning::time_entries::{default_work_kind, delete_time_entry, edit_time_entry, get_time_entry, log_time, StartInput, TimeEdit};
use crate::planning::todo_bridge::convert_todo_with_values;
use crate::planning::transition::transition_ticket;
use crate::planning::types::list_types;
use crate::planning::work_kinds::list_work_kinds;
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Color32, Id, Key, Modifiers, RichText, Vec2};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

// ------------------------------------------------------------------ gemeinsame Hilfen

/// Lesbare Kurzfassung eines Werts (Vorschau beim Klonen und Typwechsel).
pub fn preview_value(conn: &rusqlite::Connection, f: &Field, v: &Value, lang: Lang) -> String {
    let s = match f.kind {
        FieldKind::Ticket => v.as_i64().and_then(|id| get_ticket(conn, id).ok()).map(|t| format!("{} {}", t.key(), t.title)).unwrap_or_else(|| "–".to_string()),
        FieldKind::Version => v.as_i64().map(|i| release_label(conn, ReleaseKind::Version, i)).unwrap_or_default(),
        FieldKind::Milestone => v.as_i64().map(|i| release_label(conn, ReleaseKind::Milestone, i)).unwrap_or_default(),
        FieldKind::Terminal => v.as_str().map(|s| terminal_text(s, lang)).unwrap_or_default(),
        FieldKind::Datetime => v.as_str().map(super::ticket_draft::datetime_text).unwrap_or_default(),
        _ => show_value(f.kind, &f.options, v, lang),
    };
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 90 {
        format!("{}…", one_line.chars().take(90).collect::<String>())
    } else {
        one_line
    }
}

/// Soll der Dialog schließen? Esc und der Schließen-Weg des Dialogs wirken immer; ein Klick auf den Hintergrund neben dem Dialog
/// nur, solange nichts eingegeben ist – ein Fehlklick würde sonst einen angefangenen Entwurf (Titel, Beschreibung, Kommentar) verwerfen.
pub(super) fn modal_closes<T>(ctx: &egui::Context, m: &egui::ModalResponse<T>, has_input: bool) -> bool {
    let backdrop = m.backdrop_response.clicked();
    let esc = ctx.input(|i| i.key_pressed(Key::Escape));
    m.should_close() && !(backdrop && !esc && has_input)
}

/// Strg+Enter als Bestätigung in Dialogen.
fn ctrl_enter(ctx: &egui::Context) -> bool {
    ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter))
}

fn icon_for_type(t: &crate::planning::types::TicketType) -> (Option<String>, Color32) {
    (t.icon.clone(), color_of(&t.color))
}

// ------------------------------------------------------------------ Statuswechsel mit Pflichtfeldern

pub(super) struct TransitionDlg {
    pub ticket: i64,
    pub key: String,
    pub to: i64,
    pub to_name: String,
    pub fields: Vec<Field>,
    pub drafts: HashMap<i64, Draft>,
    pub project: i64,
    pub err: Option<String>,
}

// ------------------------------------------------------------------ Löschen

pub(super) struct DeleteDlg {
    pub ticket: i64,
    pub key: String,
    pub title: String,
    pub impact: DeleteImpact,
    pub since: Instant,
}

// ------------------------------------------------------------------ Klonen

pub(super) struct CloneDlg {
    pub ticket: i64,
    pub key: String,
    pub title: String,
    pub rows: Vec<ClonePreviewRow>,
    pub children: usize,
    pub include_children: bool,
    pub suffix: String,
    pub err: Option<String>,
}

// ------------------------------------------------------------------ Typ ändern / verschieben

pub(super) struct ChangeDlg {
    pub ticket: i64,
    pub key: String,
    pub cur_project: i64,
    pub cur_type: i64,
    /// Projekte, in die sich das Ticket verschieben lässt (ohne das System-Projekt).
    pub projects: Vec<(i64, String)>,
    pub project: i64,
    pub types: Vec<crate::planning::types::TicketType>,
    pub to: i64,
    pub preview: Option<ChangeTypePreview>,
    pub preview_err: Option<String>,
    /// Felder des Zieltyps, deren Wert direkt gesetzt wird.
    pub set_on: HashSet<i64>,
    pub drafts: HashMap<i64, Draft>,
    pub err: Option<String>,
}

// ------------------------------------------------------------------ Zeit erfassen

pub(super) struct TimeDlg {
    pub ticket: i64,
    pub key: String,
    /// Zeiteintrag, der bearbeitet wird (`None` = neuer Eintrag).
    pub edit: Option<i64>,
    pub duration: String,
    pub start: String,
    pub kind: Option<i64>,
    pub kinds: Vec<(i64, String)>,
    pub comment: String,
    pub mode: FieldMode,
    pub err: Option<String>,
    pub focus: bool,
    /// Der Start, wie er beim Öffnen im Feld stand (nur Minuten): bleibt er unverändert, behält der Eintrag seinen genauen Start.
    pub start_orig: String,
}

/// Der Start, den der Dialog an die Datenschicht gibt. Leer = keine Angabe (neuer Eintrag: jetzt − Dauer; Bearbeiten: unverändert).
/// Beim Bearbeiten bleibt auch ein **unveränderter** Text ohne Wirkung: das Feld zeigt nur Minuten, ein Rückschreiben würde die
/// Sekunden eines Eintrags („jetzt − Dauer“) beim bloßen Ändern des Kommentars stillschweigend abschneiden.
pub(super) fn dialog_start(editing: bool, text: &str, original: &str) -> Result<Option<StartInput>, TicketError> {
    let t = text.trim();
    if t.is_empty() || (editing && t == original.trim()) {
        return Ok(None);
    }
    StartInput::parse(t).map(Some).ok_or(TicketError::StartInvalid)
}

// ------------------------------------------------------------------ Todo → Ticket

pub(super) struct ConvertDlg {
    pub todo: i64,
    pub todo_key: String,
    pub title: String,
    pub projects: Vec<(i64, String)>,
    pub project: i64,
    pub types: Vec<crate::planning::types::TicketType>,
    pub ty: i64,
    pub fields: Vec<Field>,
    pub drafts: HashMap<i64, Draft>,
    pub body: String,
    pub err: Option<String>,
}

// ------------------------------------------------------------------ Bestätigung

pub(super) enum ConfirmKind {
    Comment { ticket: i64, id: i64 },
    Time { ticket: i64, id: i64 },
}

pub(super) struct ConfirmDlg {
    pub kind: ConfirmKind,
}

impl PlanUi {
    pub(super) fn ticket_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        self.create_dialog(ctx, cx);
        self.transition_dialog(ctx, cx);
        self.delete_ticket_dialog(ctx, cx);
        self.clone_dialog(ctx, cx);
        self.change_dialog(ctx, cx);
        self.time_dialog(ctx, cx);
        self.convert_dialog(ctx, cx);
        self.confirm_dialog(ctx, cx);
    }

    // ============================================================ Statuswechsel

    /// Statuswechsel mit Pflichtfeld-Dialog vorbereiten (`needs` = fehlende Felder laut Datenschicht).
    pub(super) fn open_transition(&mut self, cx: &Cx, ticket_id: i64, to: i64, needs: &[FieldLabel]) {
        let lang = cx.lang;
        let Some(d) = self.tickets.detail.as_ref().filter(|d| d.ticket.id == ticket_id) else { return };
        let fields: Vec<Field> = d.ty.fields.iter().filter(|f| needs.iter().any(|n| n.id == f.id)).cloned().collect();
        let drafts = fields.iter().map(|f| (f.id, Draft::from_value(f, d.stored.get(&f.id)))).collect();
        let to_name = d.sum.status(to).map(|s| s.display(lang)).unwrap_or_default();
        self.tickets.transition = Some(TransitionDlg { ticket: ticket_id, key: d.ticket.key(), to, to_name, fields, drafts, project: d.ticket.project_id, err: None });
    }

    fn transition_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut dlg) = self.tickets.transition.take() else { return };
        let lang = cx.lang;
        let prefixes: Vec<String> = self.projects.iter().map(|p| p.key.clone()).collect();
        let labels = std::mem::take(&mut self.tickets.labels);
        let (mut go, mut close) = (ctrl_enter(ctx), false);
        let m = crate::style::modal(Self::modal_id("ticket_transition")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 560.0));
            let title = match lang {
                Lang::De => format!("{} nach {} verschieben", dlg.key, lang.q(&dlg.to_name)),
                Lang::En => format!("Move {} to {}", dlg.key, lang.q(&dlg.to_name)),
            };
            widgets::dialog_title(ui, &title);
            ui.label(lang.t("Für diesen Statuswechsel müssen diese Felder ausgefüllt sein:", "This status change requires these fields to be filled in:"));
            ui.add_space(S2);
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.55).auto_shrink([false, true]).show(ui, |ui| {
                let ic = InCx { conn: cx.conn, lang, project_id: dlg.project, ticket_id: Some(dlg.ticket), prefixes: &prefixes, labels: &labels, id: Id::new(("transition", dlg.ticket)) };
                for f in &dlg.fields {
                    let Some(d) = dlg.drafts.get_mut(&f.id) else { continue };
                    let ev = field_block(ui, &ic, f, d, false, None);
                    go |= ev.enter;
                }
            });
            ui.add_space(S2);
            if let Some(e) = &dlg.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Verschieben", "Move")).on_hover_text("Ctrl+Enter").clicked() {
                    go = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        self.tickets.labels = labels;
        if m.should_close() {
            close = true;
        }
        if go && !close {
            let mut values: Vec<(i64, Value)> = Vec::new();
            let mut bad = false;
            for f in &dlg.fields {
                if let Some(d) = dlg.drafts.get_mut(&f.id) {
                    match d.to_value(f) {
                        Ok(v) if !crate::planning::values::is_empty(Some(&v)) => values.push((f.id, v)),
                        Ok(_) => {}
                        Err(e) => {
                            d.err = Some(e.message(lang));
                            bad = true;
                        }
                    }
                }
            }
            if !bad {
                match transition_ticket(cx.conn, dlg.ticket, dlg.to, &values, &Source::ui()) {
                    Ok(op) => {
                        self.tickets.commit(op);
                        self.dirty = true;
                        self.msg = None;
                        return;
                    }
                    Err(e) => dlg.err = Some(e.message(lang)),
                }
            }
        }
        if !close {
            self.tickets.transition = Some(dlg);
        }
    }

    // ============================================================ Löschen

    pub(super) fn ask_delete_ticket(&mut self, cx: &Cx, id: i64) {
        match (get_ticket(cx.conn, id), delete_impact(cx.conn, id)) {
            (Ok(t), Ok(impact)) => self.tickets.delete = Some(DeleteDlg { ticket: id, key: t.key(), title: t.title, impact, since: Instant::now() }),
            (Err(e), _) | (_, Err(e)) => self.t_fail(cx, &e),
        }
    }

    fn delete_ticket_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(dlg) = self.tickets.delete.take() else { return };
        let lang = cx.lang;
        let secs = countdown_secs(dlg.since, Instant::now());
        let (mut go, mut close) = (false, false);
        let m = crate::style::modal(Self::modal_id("ticket_delete")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 480.0));
            ui.horizontal(|ui| {
                ui.colored_label(widgets::DANGER(), RichText::new("!").heading().strong());
                ui.label(RichText::new(lang.t("Ticket endgültig löschen?", "Delete ticket permanently?")).heading());
            });
            ui.add_space(S2);
            ui.label(RichText::new(format!("{}  {}", dlg.key, dlg.title)).strong());
            ui.add_space(S1);
            let i = &dlg.impact;
            let lines: Vec<String> = [
                (i.children, "Kind-Ticket(s) verlieren ihr Eltern-Ticket (sie bleiben bestehen).", "child ticket(s) lose their parent (they stay)."),
                (i.links, "Verknüpfung(en) werden gelöscht.", "link(s) are deleted."),
                (i.comments, "Kommentar(e) werden gelöscht.", "comment(s) are deleted."),
                (i.time_entries, "Zeiteintrag/-einträge werden gelöscht.", "time entr(y/ies) are deleted."),
                (i.history, "Verlaufseintrag/-einträge gehen verloren.", "history entr(y/ies) are lost."),
                (i.referenced_by, "Ticket-Feld(er) anderer Tickets, die darauf verweisen, werden geleert.", "ticket field(s) of other tickets that refer to it are cleared."),
            ]
            .iter()
            .filter(|(n, _, _)| *n > 0)
            .map(|(n, de, en)| format!("{n} {}", lang.t(de, en)))
            .collect();
            if lines.is_empty() {
                widgets::hint(ui, lang.t("Es hängen keine weiteren Daten daran.", "No other data is attached to it."));
            } else {
                for l in lines {
                    ui.label(format!("• {l}"));
                }
            }
            ui.add_space(S2);
            ui.label(lang.t("Das lässt sich nicht rückgängig machen – auch nicht mit Strg+Z.", "This cannot be undone – not even with Ctrl+Z."));
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if widgets::danger(ui, lang.t("Löschen", "Delete"), secs).clicked() {
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
        if secs > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if go && secs == 0 {
            match delete_ticket(cx.conn, dlg.ticket, &Source::ui()) {
                Ok(_) => {
                    self.tickets.forget_tickets(&[dlg.ticket]);
                    self.tickets.back.retain(|b| *b != dlg.ticket);
                    if self.tickets.open == Some(dlg.ticket) {
                        self.ticket_back();
                    }
                    self.tickets.invalidate();
                    self.dirty = true;
                    self.msg = None;
                    self.tickets.toast(match lang {
                        Lang::De => format!("Ticket {} gelöscht.", dlg.key),
                        Lang::En => format!("Ticket {} deleted.", dlg.key),
                    });
                }
                Err(e) => self.t_fail(cx, &e),
            }
        } else if !close {
            self.tickets.delete = Some(dlg);
        }
    }

    // ============================================================ Klonen

    pub(super) fn ask_clone(&mut self, cx: &Cx, id: i64) {
        let lang = cx.lang;
        match (get_ticket(cx.conn, id), crate::planning::clone_move::clone_preview(cx.conn, id), crate::planning::tickets::children_of(cx.conn, id)) {
            (Ok(t), Ok(rows), Ok(ch)) => {
                self.tickets.clone = Some(CloneDlg { ticket: id, key: t.key(), title: t.title, rows, children: ch.len(), include_children: false, suffix: CloneOptions::new(lang).title_suffix, err: None });
            }
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => self.t_fail(cx, &e),
        }
    }

    fn clone_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut dlg) = self.tickets.clone.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (ctrl_enter(ctx), false);
        let m = crate::style::modal(Self::modal_id("ticket_clone")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 640.0));
            let title = match lang {
                Lang::De => format!("{} klonen", dlg.key),
                Lang::En => format!("Clone {}", dlg.key),
            };
            widgets::dialog_title(ui, &title);
            ui.horizontal(|ui| {
                ui.label(RichText::new(lang.t("Titelzusatz", "Title suffix")).color(style::dim(ui)));
                ui.add(egui::TextEdit::singleline(&mut dlg.suffix).desired_width(160.0).margin(egui::Margin::symmetric(8, 4)));
                ui.label(RichText::new(format!("→ {}{}", dlg.title, dlg.suffix)).small().color(style::dim(ui)));
            });
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Diese Felder übernehmen:", "Copy these fields:")).strong());
            ui.horizontal(|ui| {
                if ui.small_button(lang.t("Alle", "All")).clicked() {
                    dlg.rows.iter_mut().for_each(|r| r.included = true);
                }
                if ui.small_button(lang.t("Keine", "None")).clicked() {
                    dlg.rows.iter_mut().for_each(|r| r.included = false);
                }
            });
            ui.add_space(S1);
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.45).auto_shrink([false, true]).show(ui, |ui| {
                if dlg.rows.is_empty() {
                    widgets::hint(ui, lang.t("Der Typ hat keine Felder.", "The type has no fields."));
                }
                for r in dlg.rows.iter_mut() {
                    ui.horizontal(|ui| {
                        widgets::check(ui, &mut r.included, "");
                        ui.allocate_ui_with_layout(Vec2::new(150.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(r.field.display(lang)).strong()).truncate());
                        });
                        let (txt, dim) = match &r.value {
                            Some(v) => (preview_value(cx.conn, &r.field, v, lang), false),
                            None => (lang.t("(leer)", "(empty)").to_string(), true),
                        };
                        let mut t = RichText::new(txt);
                        if dim || !r.included {
                            t = t.color(style::dim(ui));
                        }
                        ui.add(egui::Label::new(t).truncate());
                    });
                }
            });
            ui.add_space(S2);
            if dlg.children > 0 {
                let label = match lang {
                    Lang::De => format!("Auch die {} Kind-Ticket(s) kopieren (mit allen Feldern)", dlg.children),
                    Lang::En => format!("Also copy the {} child ticket(s) (with all fields)", dlg.children),
                };
                widgets::check(ui, &mut dlg.include_children, label);
                ui.add_space(S1);
            }
            widgets::hint(ui, lang.t(
                "Der Klon bekommt einen neuen Schlüssel und den Startstatus. Kommentare, Verlauf, Zeiten, Verknüpfungen, Markierung und Stern werden nicht kopiert.",
                "The clone gets a new key and the starting status. Comments, history, time entries, links, flag and star are not copied.",
            ));
            ui.add_space(S2);
            if let Some(e) = &dlg.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Klonen", "Clone")).on_hover_text("Ctrl+Enter").clicked() {
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
        if go && !close {
            let opts = CloneOptions { fields: dlg.rows.iter().map(|r| (r.field.id, r.included)).collect(), include_children: dlg.include_children, title_suffix: dlg.suffix.clone() };
            match clone_ticket(cx.conn, dlg.ticket, &opts, &Source::ui()) {
                Ok((t, op)) => {
                    self.tickets.commit(Some(op));
                    self.dirty = true;
                    self.msg = None;
                    let key = t.key();
                    self.tickets.toast(match lang {
                        Lang::De => format!("{} als {key} geklont.", dlg.key),
                        Lang::En => format!("Cloned {} as {key}.", dlg.key),
                    });
                    self.tickets.open_ticket(t.id, true);
                    return;
                }
                Err(e) => dlg.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.tickets.clone = Some(dlg);
        }
    }

    // ============================================================ Typ ändern / verschieben

    pub(super) fn ask_change_type(&mut self, cx: &Cx, id: i64) {
        let t = match get_ticket(cx.conn, id) {
            Ok(t) => t,
            Err(e) => {
                self.t_fail(cx, &e);
                return;
            }
        };
        let projects: Vec<(i64, String)> = list_projects(cx.conn).unwrap_or_default().into_iter().filter(|p| !p.system).map(|p| (p.id, format!("{} · {}", p.key, p.name))).collect();
        let mut dlg = ChangeDlg {
            ticket: id,
            key: t.key(),
            cur_project: t.project_id,
            cur_type: t.type_id,
            projects,
            project: t.project_id,
            types: Vec::new(),
            to: t.type_id,
            preview: None,
            preview_err: None,
            set_on: HashSet::new(),
            drafts: HashMap::new(),
            err: None,
        };
        dlg.load_types(cx);
        dlg.refresh(cx);
        self.tickets.change = Some(dlg);
    }

    fn change_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut dlg) = self.tickets.change.take() else { return };
        let lang = cx.lang;
        let prefixes: Vec<String> = self.projects.iter().map(|p| p.key.clone()).collect();
        let labels = std::mem::take(&mut self.tickets.labels);
        let (mut go, mut close) = (false, false);
        let (mut new_project, mut new_type) = (None, None);
        let m = crate::style::modal(Self::modal_id("ticket_change")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 720.0));
            let title = match lang {
                Lang::De => format!("Typ von {} ändern oder verschieben", dlg.key),
                Lang::En => format!("Change type of {} or move it", dlg.key),
            };
            widgets::dialog_title(ui, &title);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(120.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Projekt", "Project")).color(style::dim(ui)));
                });
                let sel = dlg.projects.iter().find(|p| p.0 == dlg.project).map(|p| p.1.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("chg_project").selected_text(sel).width(300.0).show_ui(ui, |ui| {
                    for (id, name) in &dlg.projects {
                        if ui.selectable_label(*id == dlg.project, name).clicked() {
                            new_project = Some(*id);
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(120.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Neuer Typ", "New type")).color(style::dim(ui)));
                });
                let sel = dlg.types.iter().find(|t| t.id == dlg.to).map(|t| t.display(lang)).unwrap_or_default();
                egui::ComboBox::from_id_salt("chg_type").selected_text(sel).width(300.0).show_ui(ui, |ui| {
                    for t in &dlg.types {
                        let (_, col) = icon_for_type(t);
                        if dot_item(ui, col, &t.display(lang), t.id == dlg.to).clicked() {
                            new_type = Some(t.id);
                            ui.close();
                        }
                    }
                });
            });
            ui.add_space(S2);
            if let Some(e) = &dlg.preview_err {
                widgets::error(ui, e);
            }
            let same = dlg.to == dlg.cur_type;
            if same {
                widgets::hint(ui, lang.t("Das ist der aktuelle Typ – es ändert sich nichts.", "This is the current type – nothing changes."));
            }
            let mut set_toggle: Vec<(i64, bool)> = Vec::new();
            if let (Some(p), false) = (&dlg.preview, same) {
                egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.5).auto_shrink([false, true]).show(ui, |ui| {
                    if p.project_change {
                        let new_key = p.new_key.clone().unwrap_or_default();
                        ui.label(RichText::new(match lang {
                            Lang::De => format!("Neuer Schlüssel: {} → {new_key}", dlg.key),
                            Lang::En => format!("New key: {} → {new_key}", dlg.key),
                        }).strong());
                        widgets::hint(ui, lang.t("Verknüpfungen bleiben bestehen; Ticket-Felder, die auf andere Projekte verweisen, werden geleert.", "Links stay; ticket fields that refer across projects are cleared."));
                        ui.add_space(S1);
                    }
                    if let Some(s) = p.status {
                        let name = self.sum.as_ref().and_then(|x| x.status(s)).map(|s| s.display(lang));
                        if let Some(name) = name {
                            ui.label(format!("{}: {name}", lang.t("Status danach", "Status afterwards")));
                        }
                    }
                    if p.parent_removed {
                        ui.colored_label(Color32::from_rgb(0xe5, 0xa0, 0x4b), lang.t("Die Verbindung zum Eltern-Ticket wird gelöst.", "The link to the parent ticket is removed."));
                    }
                    if !p.detached_children.is_empty() {
                        ui.colored_label(Color32::from_rgb(0xe5, 0xa0, 0x4b), match lang {
                            Lang::De => format!("{} Kind-Ticket(s) verlieren ihr Eltern-Ticket.", p.detached_children.len()),
                            Lang::En => format!("{} child ticket(s) lose their parent.", p.detached_children.len()),
                        });
                    }
                    if !p.kept.is_empty() {
                        ui.add_space(S1);
                        ui.label(RichText::new(lang.t("Diese Werte wandern mit:", "These values move along:")).strong());
                        for k in &p.kept {
                            ui.label(format!("• {} → {}: {}", k.from.display(lang), k.to.display(lang), preview_value(cx.conn, &k.to, &k.value, lang)));
                        }
                    }
                    if !p.lost.is_empty() {
                        ui.add_space(S1);
                        ui.label(RichText::new(lang.t("Diese Werte fallen weg:", "These values are dropped:")).strong().color(crate::style::ui().error));
                        for (f, v) in &p.lost {
                            ui.label(format!("• {}: {}", f.display(lang), preview_value(cx.conn, f, v, lang)));
                        }
                    }
                    if !p.added.is_empty() {
                        ui.add_space(S1);
                        ui.label(RichText::new(lang.t("Neue Felder des Zieltyps:", "New fields of the target type:")).strong());
                        let ic = InCx { conn: cx.conn, lang, project_id: dlg.project, ticket_id: Some(dlg.ticket), prefixes: &prefixes, labels: &labels, id: Id::new(("change", dlg.ticket)) };
                        for f in &p.added {
                            let forced = f.required && f.default_value.is_none();
                            let mut on = dlg.set_on.contains(&f.id) || forced;
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(!forced, |ui| {
                                    if widgets::check(ui, &mut on, "").on_hover_text(lang.t("Wert direkt setzen", "Set a value right away")).changed() {
                                        set_toggle.push((f.id, on));
                                    }
                                });
                                ui.label(RichText::new(f.display(lang)).strong());
                                if f.required {
                                    ui.label(RichText::new("*").color(widgets::DANGER()).strong()).on_hover_text(lang.t("Pflichtfeld", "Required field"));
                                }
                                if !on {
                                    ui.label(RichText::new(lang.t("bleibt leer (bzw. Standardwert)", "stays empty (or its default)")).small().color(style::dim(ui)));
                                }
                            });
                            if on {
                                ui.indent(("chg_in", f.id), |ui| {
                                    if let Some(d) = dlg.drafts.get_mut(&f.id) {
                                        field_block(ui, &ic, f, d, false, None);
                                    }
                                });
                            }
                        }
                    }
                    ui.add_space(S1);
                    widgets::hint(ui, lang.t("Diese Aktion lässt sich nicht mit Strg+Z zurücknehmen.", "This action cannot be undone with Ctrl+Z."));
                });
            }
            for (id, on) in set_toggle {
                if on {
                    dlg.set_on.insert(id);
                } else {
                    dlg.set_on.remove(&id);
                }
            }
            ui.add_space(S2);
            if let Some(e) = &dlg.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                let label = if dlg.project != dlg.cur_project { lang.t("Verschieben", "Move") } else { lang.t("Typ ändern", "Change type") };
                if widgets::primary_enabled(ui, !same && dlg.preview.is_some(), label).clicked() {
                    go = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        self.tickets.labels = labels;
        if m.should_close() {
            close = true;
        }
        if let Some(p) = new_project {
            dlg.project = p;
            dlg.load_types(cx);
            dlg.refresh(cx);
        } else if let Some(t) = new_type {
            dlg.to = t;
            dlg.refresh(cx);
        }
        if go && !close {
            match dlg.execute(cx) {
                Ok(report) => {
                    // In ein anderes Projekt verschoben (#386): die Anwendung prüft, ob Agenten dadurch mehr Zugriff bekommen.
                    if let Some((old_prefix, _)) = report.old_key.rsplit_once('-') {
                        if let Ok(old) = crate::planning::projects::project_by_key(cx.conn, old_prefix) {
                            if old.id != report.ticket.project_id {
                                self.moved.push((old.id, report.ticket.project_id));
                            }
                        }
                    }
                    let mut ids = vec![dlg.ticket];
                    ids.extend(report.detached_children.iter().copied());
                    self.tickets.forget_tickets(&ids);
                    self.dirty = true;
                    self.msg = None;
                    let new_key = report.ticket.key();
                    self.tickets.toast(if report.old_key != new_key {
                        match lang {
                            Lang::De => format!("{} verschoben: jetzt {new_key}.", report.old_key),
                            Lang::En => format!("Moved {}: now {new_key}.", report.old_key),
                        }
                    } else {
                        match lang {
                            Lang::De => format!("Typ von {new_key} geändert."),
                            Lang::En => format!("Changed the type of {new_key}."),
                        }
                    });
                    if report.ticket.project_id != self.selected {
                        let pid = report.ticket.project_id;
                        self.select(cx, pid);
                    }
                    self.tickets.open_ticket(report.ticket.id, false);
                    self.tickets.invalidate();
                    return;
                }
                Err(e) => dlg.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.tickets.change = Some(dlg);
        }
    }

    // ============================================================ Zeit erfassen

    /// Dialog „Zeit erfassen“ (neuer Eintrag) bzw. „Eintrag bearbeiten“.
    pub(super) fn open_time(&mut self, cx: &Cx, ticket_id: i64, edit: Option<i64>) {
        let lang = cx.lang;
        let t = match get_ticket(cx.conn, ticket_id) {
            Ok(t) => t,
            Err(e) => {
                self.t_fail(cx, &e);
                return;
            }
        };
        let kinds: Vec<(i64, String)> = list_work_kinds(cx.conn, t.project_id).unwrap_or_default().into_iter().map(|k| (k.id, k.display(lang))).collect();
        let mut dlg = TimeDlg { ticket: ticket_id, key: t.key(), edit, duration: String::new(), start: String::new(), kind: default_work_kind(cx.conn, t.project_id).ok().flatten(), kinds, comment: String::new(), mode: FieldMode::Text, err: None, focus: true, start_orig: String::new() };
        if let Some(id) = edit {
            match get_time_entry(cx.conn, id) {
                Ok(e) => {
                    dlg.duration = format_duration(e.minutes, DEFAULT_WORK_HOURS_PER_DAY, lang);
                    dlg.start = e.start.replace('T', " ").chars().take(16).collect();
                    dlg.start_orig = dlg.start.clone();
                    dlg.kind = e.work_kind_id;
                    dlg.comment = e.comment.clone();
                    dlg.mode = if e.comment.trim().is_empty() { FieldMode::Text } else { FieldMode::Visual };
                }
                Err(e) => {
                    self.t_fail(cx, &e);
                    return;
                }
            }
        }
        self.tickets.time = Some(dlg);
    }

    fn time_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut dlg) = self.tickets.time.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (ctrl_enter(ctx), false);
        let m = crate::style::modal(Self::modal_id("ticket_time")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 560.0));
            let title = match (lang, dlg.edit.is_some()) {
                (Lang::De, false) => format!("Zeit erfassen – {}", dlg.key),
                (Lang::En, false) => format!("Log time – {}", dlg.key),
                (Lang::De, true) => format!("Zeiteintrag bearbeiten – {}", dlg.key),
                (Lang::En, true) => format!("Edit time entry – {}", dlg.key),
            };
            widgets::dialog_title(ui, &title);
            // Dauer
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(130.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Dauer", "Duration")).strong());
                    ui.label(RichText::new("*").color(widgets::DANGER()).strong());
                });
                let r = ui.add_sized([200.0, 28.0], egui::TextEdit::singleline(&mut dlg.duration).id(Id::new("time_duration")).hint_text("1h 30m").margin(egui::Margin::symmetric(8, 5)));
                if dlg.focus {
                    r.request_focus();
                    dlg.focus = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    go = true;
                }
                match parse_duration(&dlg.duration) {
                    Ok(mins) if !dlg.duration.trim().is_empty() => {
                        ui.label(RichText::new(format!("= {} ({} min)", format_duration(mins, DEFAULT_WORK_HOURS_PER_DAY, lang), mins)).color(style::dim(ui)));
                    }
                    Err(e) if !dlg.duration.trim().is_empty() => {
                        ui.label(RichText::new(e.message(lang)).small().color(crate::style::ui().error));
                    }
                    _ => {}
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(130.0 + 8.0);
                widgets::hint(ui, lang.t(
                    "m = Minuten, h = Stunden, d = Tage (1 d = 8 h), z. B. 30m, 1h 30m, 2d. Eine bloße Zahl zählt als Stunden.",
                    "m = minutes, h = hours, d = days (1 d = 8 h), e.g. 30m, 1h 30m, 2d. A plain number counts as hours.",
                ));
            });
            ui.add_space(S2);
            // Art der Arbeit
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(130.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Art der Arbeit", "Kind of work")).color(style::dim(ui)));
                });
                let sel = dlg.kind.and_then(|k| dlg.kinds.iter().find(|x| x.0 == k)).map(|x| x.1.clone()).unwrap_or_else(|| "–".to_string());
                egui::ComboBox::from_id_salt("time_kind").selected_text(sel).width(220.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut dlg.kind, None, "–");
                    for (id, name) in &dlg.kinds {
                        ui.selectable_value(&mut dlg.kind, Some(*id), name);
                    }
                });
            });
            ui.add_space(S2);
            // Start
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(130.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Start", "Start")).color(style::dim(ui)));
                });
                ui.add_sized([200.0, 28.0], egui::TextEdit::singleline(&mut dlg.start).hint_text(lang.t("JJJJ-MM-TT HH:MM", "YYYY-MM-DD HH:MM")).margin(egui::Margin::symmetric(8, 5)));
                if ui.small_button(lang.t("Jetzt", "Now")).clicked() {
                    dlg.start = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
                }
                if !dlg.start.trim().is_empty() && StartInput::parse(&dlg.start).is_none() {
                    ui.label(RichText::new(lang.t("nicht lesbar", "not readable")).small().color(crate::style::ui().error));
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(130.0 + 8.0);
                widgets::hint(ui, lang.t(
                    "Leer = jetzt minus Dauer (der Eintrag endet jetzt). Nur ein Datum = 08:00 Uhr. Das Ende ist Start + Dauer.",
                    "Empty = now minus the duration (the entry ends now). Date only = 08:00. The end is start + duration.",
                ));
            });
            ui.add_space(S2);
            // Kommentar
            ui.label(RichText::new(lang.t("Kommentar (optional)", "Comment (optional)")).color(style::dim(ui)));
            let opts = FieldOpts { rows: 4, max_height: Some(220.0), ..Default::default() };
            rich_text_field(ui, "time_comment", &mut dlg.comment, &mut dlg.mode, &opts);
            ui.add_space(S2);
            if let Some(e) = &dlg.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                let label = if dlg.edit.is_some() { lang.t("Speichern", "Save") } else { lang.t("Erfassen", "Log") };
                if widgets::primary(ui, label).on_hover_text("Ctrl+Enter").clicked() {
                    go = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if modal_closes(ctx, &m, !dlg.duration.trim().is_empty() || !dlg.comment.trim().is_empty() || dlg.start.trim() != dlg.start_orig.trim()) {
            close = true;
        }
        if go && !close {
            let start = dialog_start(dlg.edit.is_some(), &dlg.start, &dlg.start_orig);
            let res = start.and_then(|start| match dlg.edit {
                None => log_time(cx.conn, dlg.ticket, &dlg.duration, start, dlg.kind, &dlg.comment, chrono::Local::now().naive_local(), &Source::ui()).map(|(_, op)| Some(op)),
                Some(id) => edit_time_entry(cx.conn, id, &TimeEdit { duration: Some(dlg.duration.clone()), start, work_kind: Some(dlg.kind), comment: Some(dlg.comment.clone()) }, &Source::ui()),
            });
            match res {
                Ok(op) => {
                    self.tickets.commit(op);
                    self.dirty = true;
                    self.msg = None;
                    return;
                }
                Err(e) => dlg.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.tickets.time = Some(dlg);
        }
    }

    // ============================================================ Todo → Ticket

    pub(super) fn ask_convert(&mut self, cx: &Cx, todo_id: i64) {
        let item = match crate::tasks::get_item(cx.conn, todo_id) {
            Ok(i) => i,
            Err(_) => return,
        };
        let projects: Vec<(i64, String)> = list_projects(cx.conn).unwrap_or_default().into_iter().filter(|p| !p.system).map(|p| (p.id, format!("{} · {}", p.key, p.name))).collect();
        if projects.is_empty() {
            self.msg_error(cx, cx.lang.t("Es gibt noch kein Projekt, in das sich die Karte umwandeln ließe. Lege zuerst ein Projekt an.", "There is no project to convert the card into yet. Create a project first.").to_string());
            return;
        }
        let project = projects.iter().find(|p| p.0 == self.selected).or(projects.first()).map(|p| p.0).unwrap_or(0);
        let mut dlg = ConvertDlg {
            todo: todo_id,
            todo_key: item.key(),
            title: crate::planning::todo_bridge::display_title(&item.title, &item.body),
            projects,
            project,
            types: Vec::new(),
            ty: 0,
            fields: Vec::new(),
            drafts: HashMap::new(),
            body: item.body.clone(),
            err: None,
        };
        dlg.load(cx, None);
        self.tickets.convert = Some(dlg);
    }

    fn convert_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut dlg) = self.tickets.convert.take() else { return };
        let lang = cx.lang;
        let prefixes: Vec<String> = self.projects.iter().map(|p| p.key.clone()).collect();
        let labels = std::mem::take(&mut self.tickets.labels);
        let (mut go, mut close) = (ctrl_enter(ctx), false);
        let (mut new_project, mut new_type) = (None, None);
        let m = crate::style::modal(Self::modal_id("ticket_convert")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 600.0));
            let title = match lang {
                Lang::De => format!("{} in ein Ticket umwandeln", dlg.todo_key),
                Lang::En => format!("Convert {} into a ticket", dlg.todo_key),
            };
            widgets::dialog_title(ui, &title);
            ui.label(RichText::new(&dlg.title).strong());
            widgets::hint(ui, lang.t("Titel und Text der Karte werden übernommen, die Tags als Labels (falls der Typ ein Labels-Feld hat). Danach gilt die Karte als erledigt.", "The card's title and text are carried over, its tags become labels (if the type has a labels field). The card is then marked done."));
            ui.add_space(S2);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(120.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Projekt", "Project")).color(style::dim(ui)));
                });
                let sel = dlg.projects.iter().find(|p| p.0 == dlg.project).map(|p| p.1.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("cv_project").selected_text(sel).width(300.0).show_ui(ui, |ui| {
                    for (id, name) in &dlg.projects {
                        if ui.selectable_label(*id == dlg.project, name).clicked() {
                            new_project = Some(*id);
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(120.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(lang.t("Typ", "Type")).color(style::dim(ui)));
                });
                let sel = dlg.types.iter().find(|t| t.id == dlg.ty).map(|t| t.display(lang)).unwrap_or_default();
                egui::ComboBox::from_id_salt("cv_type").selected_text(sel).width(300.0).show_ui(ui, |ui| {
                    for t in &dlg.types {
                        if dot_item(ui, color_of(&t.color), &t.display(lang), t.id == dlg.ty).clicked() {
                            new_type = Some(t.id);
                            ui.close();
                        }
                    }
                });
            });
            ui.add_space(S2);
            if !dlg.fields.is_empty() {
                ui.label(RichText::new(lang.t("Pflichtfelder des Typs:", "Required fields of the type:")).strong());
                ui.add_space(S1);
                egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.4).auto_shrink([false, true]).show(ui, |ui| {
                    let ic = InCx { conn: cx.conn, lang, project_id: dlg.project, ticket_id: None, prefixes: &prefixes, labels: &labels, id: Id::new(("convert", dlg.todo)) };
                    for (_, idxs) in group_sections(&dlg.fields) {
                        for i in idxs {
                            let f = &dlg.fields[i];
                            if let Some(d) = dlg.drafts.get_mut(&f.id) {
                                let ev = field_block(ui, &ic, f, d, false, None);
                                go |= ev.enter;
                            }
                        }
                    }
                });
            }
            ui.add_space(S2);
            if let Some(e) = &dlg.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, dlg.ty != 0, lang.t("Umwandeln", "Convert")).on_hover_text("Ctrl+Enter").clicked() {
                    go = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        self.tickets.labels = labels;
        if m.should_close() {
            close = true;
        }
        if let Some(p) = new_project {
            dlg.project = p;
            dlg.load(cx, None);
        } else if let Some(t) = new_type {
            dlg.load(cx, Some(t));
        }
        if go && !close && dlg.ty != 0 {
            let mut extra: Vec<(i64, Value)> = Vec::new();
            let mut bad = false;
            for f in &dlg.fields {
                if let Some(d) = dlg.drafts.get_mut(&f.id) {
                    match d.to_value(f) {
                        Ok(v) if !crate::planning::values::is_empty(Some(&v)) => extra.push((f.id, v)),
                        Ok(_) => {}
                        Err(e) => {
                            d.err = Some(e.message(lang));
                            bad = true;
                        }
                    }
                }
            }
            if !bad {
                match convert_todo_with_values(cx.conn, dlg.todo, dlg.project, dlg.ty, &extra, &Source::ui()) {
                    Ok(conv) => {
                        self.tickets.commit(Some(conv.op));
                        self.todo.dirty = true;
                        self.dirty = true;
                        self.msg = None;
                        let key = conv.ticket.key();
                        let mut text = match lang {
                            Lang::De => format!("{} in {key} umgewandelt.", dlg.todo_key),
                            Lang::En => format!("Converted {} into {key}.", dlg.todo_key),
                        };
                        if !conv.dropped_tags.is_empty() {
                            text.push_str(&match lang {
                                Lang::De => format!(" Tags nicht übernommen (kein Labels-Feld): {}.", conv.dropped_tags.join(", ")),
                                Lang::En => format!(" Tags not carried over (no labels field): {}.", conv.dropped_tags.join(", ")),
                            });
                        }
                        if conv.body_dropped {
                            text.push_str(lang.t(" Der Text der Karte wurde nicht übernommen (kein Beschreibungsfeld).", " The card's text was not carried over (no description field)."));
                        }
                        self.tickets.toast(text);
                        return;
                    }
                    Err(TicketError::RequiredMissing(v)) => {
                        // die fehlenden Pflichtfelder markieren, falls sie im Dialog stehen
                        for l in &v {
                            if let Some(d) = dlg.drafts.get_mut(&l.id) {
                                d.edited = true;
                                d.err = Some(lang.t("Pflichtfeld – bitte ausfüllen.", "Required – please fill in.").to_string());
                            }
                        }
                        dlg.err = Some(TicketError::RequiredMissing(v).message(lang));
                    }
                    Err(e) => dlg.err = Some(e.message(lang)),
                }
            }
        }
        if !close {
            self.tickets.convert = Some(dlg);
        }
    }

    // ============================================================ Bestätigungen

    fn confirm_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(dlg) = self.tickets.confirm.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = crate::style::modal(Self::modal_id("ticket_confirm")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 420.0));
            let (title, text) = match dlg.kind {
                ConfirmKind::Comment { .. } => (lang.t("Kommentar löschen?", "Delete comment?"), lang.t("Der Kommentar wird endgültig gelöscht.", "The comment will be deleted permanently.")),
                ConfirmKind::Time { .. } => (lang.t("Zeiteintrag löschen?", "Delete time entry?"), lang.t("Der Zeiteintrag wird endgültig gelöscht.", "The time entry will be deleted permanently.")),
            };
            ui.horizontal(|ui| {
                ui.colored_label(widgets::DANGER(), RichText::new("!").heading().strong());
                ui.label(RichText::new(title).heading());
            });
            ui.add_space(S2);
            ui.label(text);
            ui.label(lang.t("Das lässt sich nicht rückgängig machen.", "This cannot be undone."));
            ui.add_space(S3);
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
            let (ticket, res) = match dlg.kind {
                ConfirmKind::Comment { ticket, id } => (ticket, delete_comment(cx.conn, id, &Source::ui())),
                ConfirmKind::Time { ticket, id } => (ticket, delete_time_entry(cx.conn, id, &Source::ui())),
            };
            match res {
                Ok(()) => {
                    // Frühere Änderungen an diesem Kommentar/Eintrag ließen sich nicht mehr zurücknehmen.
                    self.tickets.forget_tickets(&[ticket]);
                    self.tickets.invalidate();
                    self.dirty = true;
                    self.msg = None;
                }
                Err(e) => self.t_fail(cx, &e),
            }
        } else if !close {
            self.tickets.confirm = Some(dlg);
        }
    }
}

impl ChangeDlg {
    fn load_types(&mut self, cx: &Cx) {
        self.types = list_types(cx.conn, self.project).unwrap_or_default().into_iter().filter(|t| !t.system).collect();
        self.to = if self.project == self.cur_project { self.cur_type } else { self.types.first().map(|t| t.id).unwrap_or(0) };
    }

    pub(super) fn refresh(&mut self, cx: &Cx) {
        self.preview = None;
        self.preview_err = None;
        self.err = None;
        self.set_on.clear();
        self.drafts.clear();
        if self.to == 0 || self.to == self.cur_type {
            return;
        }
        match change_type_preview(cx.conn, self.ticket, self.to) {
            Ok(p) => {
                for f in &p.added {
                    self.drafts.insert(f.id, Draft::from_value(f, f.default_value.as_ref()));
                    if f.required && f.default_value.is_none() {
                        self.set_on.insert(f.id);
                    }
                }
                self.preview = Some(p);
            }
            Err(e) => self.preview_err = Some(e.message(cx.lang)),
        }
    }

    pub(super) fn execute(&mut self, cx: &Cx) -> Result<crate::planning::clone_move::ChangeTypeReport, TicketError> {
        let Some(p) = &self.preview else { return Err(TicketError::MappingInvalid) };
        let mut values: Vec<(i64, Value)> = Vec::new();
        for f in &p.added {
            if !self.set_on.contains(&f.id) {
                continue;
            }
            if let Some(d) = self.drafts.get_mut(&f.id) {
                match d.to_value(f) {
                    Ok(v) => values.push((f.id, v)),
                    Err(e) => {
                        let msg = e.message(cx.lang);
                        d.err = Some(msg.clone());
                        return Err(TicketError::Db(msg));
                    }
                }
            }
        }
        change_type(cx.conn, self.ticket, self.to, &p.mapping, &values, &Source::ui())
    }
}

impl ConvertDlg {
    /// Typen und Pflichtfelder des Projekts laden; `ty` = gewünschter Typ (sonst der erste).
    fn load(&mut self, cx: &Cx, ty: Option<i64>) {
        self.types = list_types(cx.conn, self.project).unwrap_or_default().into_iter().filter(|t| !t.system).collect();
        self.ty = ty.filter(|t| self.types.iter().any(|x| x.id == *t)).or_else(|| self.types.first().map(|t| t.id)).unwrap_or(0);
        self.err = None;
        self.drafts.clear();
        self.fields = if self.ty == 0 {
            Vec::new()
        } else {
            list_fields(cx.conn, self.ty).unwrap_or_default().into_iter().filter(|f| f.required && f.default_value.is_none()).collect()
        };
        for f in &self.fields {
            let mut d = Draft::from_value(f, None);
            // Pflicht-Beschreibung: mit dem Text der Karte vorbelegen
            if f.key.as_deref() == Some("description") && !self.body.trim().is_empty() {
                d.text = self.body.clone();
                d.mode = FieldMode::Visual;
                d.edited = true;
            }
            self.drafts.insert(f.id, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::FieldOptions;
    use crate::planning::tickets::fixture::Fx;
    use serde_json::json;

    #[test]
    fn editing_a_time_entry_keeps_its_exact_start_unless_the_field_changed() {
        use crate::planning::time_entries::{edit_time_entry, get_time_entry, log_time};
        let f = Fx::new();
        let t = f.mk(f.task, "T");
        // Start „jetzt − Dauer“ hat Sekunden: 12:00:41 − 1 h = 11:00:41 (das Feld zeigt nur 11:00)
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(12, 0, 41).unwrap();
        let (id, _) = log_time(&f.c, t.id, "1h", None, None, "alt", now, &Source::ui()).unwrap();
        let shown: String = get_time_entry(&f.c, id).unwrap().start.replace('T', " ").chars().take(16).collect();
        assert_eq!(shown, "2026-11-14 11:00");
        // nur den Kommentar ändern: der Start bleibt, wie er war
        let start = dialog_start(true, &shown, &shown).unwrap();
        assert_eq!(start, None);
        edit_time_entry(&f.c, id, &TimeEdit { comment: Some("neu".into()), start, ..Default::default() }, &Source::ui()).unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().start, "2026-11-14T11:00:41");
        // den Start tatsächlich ändern wirkt
        let start = dialog_start(true, "2026-11-14 09:00", &shown).unwrap();
        assert!(start.is_some());
        edit_time_entry(&f.c, id, &TimeEdit { start, ..Default::default() }, &Source::ui()).unwrap();
        assert_eq!(get_time_entry(&f.c, id).unwrap().start, "2026-11-14T09:00:00");
        // neuer Eintrag: gleicher Text wie „original“ zählt als Eingabe, leer als keine Angabe, Unsinn als Fehler
        assert!(dialog_start(false, "2026-11-14 09:00", "").unwrap().is_some());
        assert_eq!(dialog_start(false, "  ", "").unwrap(), None);
        assert_eq!(dialog_start(true, "gestern", &shown).unwrap_err(), TicketError::StartInvalid);
    }

    #[test]
    fn values_are_previewed_in_one_line() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "Mein Epic");
        let sp = f.field(f.story, "story_points");
        let pr = f.field(f.story, "priority");
        let ep = f.field(f.story, "epic");
        assert_eq!(preview_value(&f.c, &sp, &json!(5), Lang::En), "5");
        assert_eq!(preview_value(&f.c, &pr, &pr.default_value.clone().unwrap(), Lang::En), "Medium");
        assert_eq!(preview_value(&f.c, &pr, &pr.default_value.clone().unwrap(), Lang::De), "Normal");
        assert_eq!(preview_value(&f.c, &ep, &json!(epic.id), Lang::En), format!("{} Mein Epic", epic.key()));
        let long = f.add_field(f.story, "Lang", FieldKind::Text, FieldOptions::default(), false);
        let s = preview_value(&f.c, &long, &json!(format!("{}\n{}", "a".repeat(80), "b".repeat(80))), Lang::En);
        assert!(s.ends_with('…') && !s.contains('\n') && s.chars().count() <= 91, "{s}");
    }
}
