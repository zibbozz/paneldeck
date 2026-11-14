//! Dialoge der Sprint-Verwaltung: „Neuer Sprint“/„Sprint bearbeiten“ (Name, Dauer-Vorlage in Arbeitstagen, Start, Ende, Ziel),
//! Löschen (nur geplante, mit Bestätigung), Starten, Abschließen (Wohin mit den Unerledigten, danach die Zusammenfassung) und der
//! Arbeitskalender des Projekts. Die Eingabelogik steht in `sprint_logic.rs`.

use super::sprint_data as data;
use super::sprint_logic::{self as logic, CalSettings, LeftoverChoice, SprintFields};
use super::widgets;
use super::{Cx, PlanUi};
use crate::calc::calendar::WorkCalendar;
use crate::planning::projects::Project;
use crate::planning::sprints::sprint::{complete_sprint, create_sprint, delete_sprint, list_sprints, sprint_content, start_sprint, suggest_dates, suggest_name, update_sprint, NewSprint, Sprint, SprintState, UpdateSprint};
use crate::planning::sprints::{clock, DEFAULT_SPRINT_DAYS, MAX_GOAL_CHARS};
use crate::state::Lang;
use crate::style::{S1, S2};
use eframe::egui::{self, RichText};

/// Dialog „Sprint anlegen/bearbeiten“.
pub(super) struct SprintForm {
    /// `None` = neuer Sprint.
    pub(super) id: Option<i64>,
    pub(super) state: SprintState,
    pub(super) f: SprintFields,
    pub(super) err: Option<String>,
}

/// Bestätigung „Sprint löschen“.
pub(super) struct DeleteAsk {
    pub id: i64,
    pub name: String,
    pub tickets: usize,
}

/// Dialog „Sprint abschließen“.
pub(super) struct CompleteDlg {
    pub(super) sprint_id: i64,
    pub(super) name: String,
    pub(super) done: usize,
    /// Unerledigte Tickets (Schlüssel, Titel).
    pub(super) open: Vec<(String, String)>,
    /// Geplante Sprints des Projekts: Kennung, Name.
    pub(super) planned: Vec<(i64, String)>,
    pub(super) choice: LeftoverChoice,
    pub(super) target: Option<i64>,
    /// Nach dem Abschließen: die Zusammenfassung.
    pub(super) result: Option<String>,
    pub(super) err: Option<String>,
}

/// Dialog „Kalender“ (Arbeitstage und Feiertage des Projekts).
pub(super) struct CalDlg {
    project: i64,
    exclude_weekends: bool,
    holidays: String,
    err: Option<String>,
}

/// So viele unerledigte Tickets listet der Dialog höchstens auf.
const LIST_MAX: usize = 8;

impl PlanUi {
    // ------------------------------------------------------------------ Öffnen

    pub(super) fn open_new_sprint(&mut self, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let name = match suggest_name(cx.conn, proj.id) {
            Ok(n) => n,
            Err(e) => {
                self.sprint_fail(lang, &e);
                return;
            }
        };
        let mut f = SprintFields::new(&name);
        let cal = self.sprints.cal.calendar();
        if let Ok((s, e)) = suggest_dates(cx.conn, proj.id, DEFAULT_SPRINT_DAYS, &cal, clock::today()) {
            f.start = logic::fmt_date_ui(s, lang);
            f.end = logic::fmt_date_ui(e, lang);
        }
        self.sprints.form = Some(SprintForm { id: None, state: SprintState::Planned, f, err: None });
    }

    pub(super) fn open_edit_sprint(&mut self, lang: Lang, sp: &Sprint) {
        let cal = self.sprints.cal.calendar();
        let mut f = SprintFields::new(&sp.name);
        f.goal = sp.goal.clone();
        f.start = sp.start_date.map(|d| logic::fmt_date_ui(d, lang)).unwrap_or_default();
        f.end = sp.end_date.map(|d| logic::fmt_date_ui(d, lang)).unwrap_or_default();
        f.days = match (sp.start_date, sp.end_date) {
            (Some(s), Some(e)) => cal.work_days_inclusive(s, e).clamp(1, crate::planning::sprints::MAX_SPRINT_DAYS).to_string(),
            _ => sp.default_days.to_string(),
        };
        self.sprints.form = Some(SprintForm { id: Some(sp.id), state: sp.state, f, err: None });
    }

    pub(super) fn open_complete(&mut self, cx: &Cx, id: i64) {
        let lang = cx.lang;
        let c = match sprint_content(cx.conn, id) {
            Ok(c) => c,
            Err(e) => {
                self.sprint_fail(lang, &e);
                return;
            }
        };
        let open: Vec<(String, String)> = c.items.iter().filter(|i| !i.done).map(|i| (i.ticket.key(), i.ticket.title.clone())).collect();
        let done = c.items.iter().filter(|i| i.done).count();
        let planned_all: Vec<_> = list_sprints(cx.conn, c.sprint.project_id).unwrap_or_default().into_iter().filter(|s| s.state == SprintState::Planned).collect();
        let target = logic::default_target(&planned_all.iter().map(|s| (s.id, s.start_date)).collect::<Vec<_>>());
        let mut planned: Vec<(i64, String)> = planned_all.iter().map(|s| (s.id, s.name.clone())).collect();
        planned.sort_by_key(|(sid, _)| (Some(*sid) != target, *sid));
        self.sprints.complete = Some(CompleteDlg { sprint_id: id, name: c.sprint.name, done, open, planned, choice: LeftoverChoice::Backlog, target, result: None, err: None });
    }

    pub(super) fn open_calendar(&mut self, lang: Lang, project: i64) {
        let c = self.sprints.cal.clone();
        self.sprints.cal_dlg = Some(CalDlg { project, exclude_weekends: c.exclude_weekends, holidays: logic::holidays_edit_text(&c.holidays, lang), err: None });
    }

    /// Sprint starten (höchstens einer je Projekt; sonst eine Fehlermeldung).
    pub(super) fn start_sprint_now(&mut self, cx: &Cx, id: i64) {
        let cal = self.sprints.cal.calendar();
        match start_sprint(cx.conn, id, &cal) {
            Ok(sp) => {
                self.sprint_changed();
                let lang = cx.lang;
                self.note(match lang {
                    Lang::De => format!("Sprint {} gestartet.", lang.q(&sp.name)),
                    Lang::En => format!("Started sprint {}.", lang.q(&sp.name)),
                });
            }
            Err(e) => self.sprint_fail(cx.lang, &e),
        }
    }

    // ------------------------------------------------------------------ Zeichnen

    /// Alle Dialoge der Sprints und der Releases (aus `PlanUi::dialogs`).
    pub(super) fn sprint_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        self.sprint_form_dialog(ctx, cx);
        self.sprint_delete_dialog(ctx, cx);
        self.sprint_complete_dialog(ctx, cx);
        self.calendar_dialog(ctx, cx);
        self.release_dialogs(ctx, cx);
    }

    fn sprint_form_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut form) = self.sprints.form.take() else { return };
        let lang = cx.lang;
        let cal = self.sprints.cal.calendar();
        let (mut save, mut close) = (false, false);
        let active = form.state == SprintState::Active;
        let m = egui::Modal::new(Self::modal_id("sprint_form")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 520.0));
            widgets::dialog_title(ui, if form.id.is_some() { lang.t("Sprint bearbeiten", "Edit sprint") } else { lang.t("Neuer Sprint", "New sprint") });
            ui.label(RichText::new(lang.t("Name", "Name")).strong());
            ui.add(egui::TextEdit::singleline(&mut form.f.name).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
            ui.add_space(S2);
            egui::Grid::new("sprint_form_grid").num_columns(2).spacing(egui::Vec2::new(14.0, 8.0)).show(ui, |ui| {
                widgets::row(ui, lang.t("Dauer (Arbeitstage)", "Length (work days)"), |ui| {
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut form.f.days).desired_width(60.0).margin(egui::Margin::symmetric(8, 4)));
                        if r.changed() {
                            form.f.recompute_end(&cal, lang);
                        }
                        for n in [5u32, 10, 15] {
                            if ui.small_button(n.to_string()).on_hover_text(lang.t("Vorlage: Arbeitstage", "Preset: work days")).clicked() {
                                form.f.days = n.to_string();
                                form.f.end_manual = false;
                                form.f.recompute_end(&cal, lang);
                            }
                        }
                    });
                });
                widgets::row(ui, lang.t("Start", "Start"), |ui| {
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut form.f.start).desired_width(120.0).hint_text(logic::fmt_date_ui(chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap_or_default(), lang)).margin(egui::Margin::symmetric(8, 4)));
                        if r.changed() {
                            form.f.recompute_end(&cal, lang);
                        }
                        if ui.small_button(lang.t("Heute", "Today")).clicked() {
                            form.f.start = logic::fmt_date_ui(clock::today(), lang);
                            form.f.recompute_end(&cal, lang);
                        }
                    });
                });
                widgets::row(ui, lang.t("Ende", "End"), |ui| {
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut form.f.end).desired_width(120.0).margin(egui::Margin::symmetric(8, 4)));
                        if r.changed() {
                            if form.f.end.trim().is_empty() {
                                form.f.end_manual = false;
                            } else {
                                form.f.after_end_edit(&cal, lang);
                            }
                        }
                        if form.f.end_manual && ui.small_button(lang.t("aus Dauer", "from length")).on_hover_text(lang.t("Ende wieder aus Start und Dauer berechnen", "Calculate the end from start and length again")).clicked() {
                            form.f.end_manual = false;
                            form.f.recompute_end(&cal, lang);
                        }
                        if let Some(n) = form.f.work_days(&cal) {
                            ui.label(RichText::new(match lang {
                                Lang::De => format!("{n} Arbeitstage"),
                                Lang::En => format!("{n} work days"),
                            }).small().color(crate::style::dim(ui)));
                        }
                    });
                });
            });
            widgets::hint(ui, lang.t("Datum als TT.MM.JJJJ oder JJJJ-MM-TT. Ohne Termine legt „Starten“ den Start auf heute und das Ende aus der Dauer fest.", "Dates as DD.MM.YYYY or YYYY-MM-DD. Without dates, “Start” uses today and works out the end from the length."));
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Ziel", "Goal")).strong());
            ui.add(egui::TextEdit::multiline(&mut form.f.goal).desired_rows(3).desired_width(f32::INFINITY).char_limit(MAX_GOAL_CHARS).margin(egui::Margin::symmetric(8, 5)));
            if active {
                widgets::hint(ui, lang.t("Der Sprint läuft: Start und Ende müssen gesetzt bleiben.", "The sprint is running: start and end must stay set."));
            }
            if let Some(e) = &form.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Speichern", "Save")).clicked() {
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
        if save {
            match self.save_sprint_form(cx, &form, &cal) {
                Ok(()) => return,
                Err(e) => form.err = Some(e),
            }
        }
        if !close {
            self.sprints.form = Some(form);
        }
    }

    pub(super) fn save_sprint_form(&mut self, cx: &Cx, form: &SprintForm, cal: &WorkCalendar) -> Result<(), String> {
        let lang = cx.lang;
        let p = form.f.parse().map_err(|e| e.message(lang))?;
        let pid = self.selected;
        let r = match form.id {
            None => create_sprint(cx.conn, &NewSprint { project_id: pid, name: form.f.name.clone(), goal: form.f.goal.clone(), start: p.start, end: p.end, default_days: Some(p.days) }, cal).map(|_| ()),
            Some(id) => update_sprint(cx.conn, id, &UpdateSprint { name: Some(form.f.name.clone()), goal: Some(form.f.goal.clone()), start: Some(p.start), end: Some(p.end), default_days: Some(p.days) }),
        };
        match r {
            Ok(()) => {
                self.sprint_changed();
                self.note(if form.id.is_some() { lang.t("Sprint gespeichert.", "Sprint saved.").to_string() } else { lang.t("Sprint angelegt.", "Sprint created.").to_string() });
                Ok(())
            }
            Err(e) => Err(e.message(lang)),
        }
    }

    fn sprint_delete_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(d) = self.sprints.del.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("sprint_delete")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 460.0));
            let title = match lang {
                Lang::De => format!("Sprint {} löschen?", lang.q(&d.name)),
                Lang::En => format!("Delete sprint {}?", lang.q(&d.name)),
            };
            widgets::dialog_title(ui, &title);
            let body = match (lang, d.tickets) {
                (Lang::De, 0) => "Der Sprint ist leer. Das lässt sich nicht rückgängig machen.".to_string(),
                (Lang::En, 0) => "The sprint is empty. This cannot be undone.".to_string(),
                (Lang::De, n) => format!("Die {} des Sprints landen wieder im Backlog. Das lässt sich nicht rückgängig machen.", logic::tickets_text(n, lang)),
                (Lang::En, n) => format!("The sprint's {} go back to the backlog. This cannot be undone.", logic::tickets_text(n, lang)),
            };
            ui.label(body);
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
            match delete_sprint(cx.conn, d.id) {
                Ok(()) => {
                    self.sprint_changed();
                    self.note(lang.t("Sprint gelöscht.", "Sprint deleted.").to_string());
                }
                Err(e) => self.sprint_fail(lang, &e),
            }
        } else if !close {
            self.sprints.del = Some(d);
        }
    }

    fn sprint_complete_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.sprints.complete.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("sprint_complete")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 520.0));
            let title = match lang {
                Lang::De => format!("Sprint {} abschließen", lang.q(&d.name)),
                Lang::En => format!("Complete sprint {}", lang.q(&d.name)),
            };
            widgets::dialog_title(ui, &title);
            if let Some(res) = &d.result {
                ui.label(res);
                ui.add_space(S2);
                if widgets::primary(ui, lang.t("Schließen", "Close")).clicked() {
                    close = true;
                }
                return;
            }
            ui.label(match lang {
                Lang::De => format!("{} erledigt, {} unerledigt.", logic::tickets_text(d.done, lang), logic::tickets_text(d.open.len(), lang)),
                Lang::En => format!("{} done, {} unfinished.", logic::tickets_text(d.done, lang), logic::tickets_text(d.open.len(), lang)),
            });
            if !d.open.is_empty() {
                ui.add_space(S1);
                for (k, t) in d.open.iter().take(LIST_MAX) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(k).monospace().small().color(crate::style::dim(ui)));
                        ui.add(egui::Label::new(t).truncate());
                    });
                }
                if d.open.len() > LIST_MAX {
                    ui.label(RichText::new(format!("+ {} {}", d.open.len() - LIST_MAX, lang.t("weitere", "more"))).small().color(crate::style::dim(ui)));
                }
                ui.add_space(S2);
                ui.label(RichText::new(lang.t("Wohin mit den unerledigten Tickets?", "Where should the unfinished tickets go?")).strong());
                widgets::radio(ui, &mut d.choice, LeftoverChoice::Backlog, lang.t("Ins Backlog", "To the backlog"));
                ui.add_enabled_ui(!d.planned.is_empty(), |ui| {
                    widgets::radio(ui, &mut d.choice, LeftoverChoice::NextSprint, lang.t("In einen geplanten Sprint:", "To a planned sprint:"));
                });
                if d.planned.is_empty() {
                    widgets::hint(ui, lang.t("Es gibt keinen geplanten Sprint. Lege vorher einen an, um Tickets dorthin mitzunehmen.", "There is no planned sprint. Create one first to carry the tickets over."));
                } else if d.choice == LeftoverChoice::NextSprint {
                    ui.indent("complete_target", |ui| {
                        let sel = d.planned.iter().find(|(id, _)| Some(*id) == d.target).map(|(_, n)| n.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("complete_target_pick").selected_text(sel).width(260.0).show_ui(ui, |ui| {
                            for (id, name) in &d.planned {
                                ui.selectable_value(&mut d.target, Some(*id), name);
                            }
                        });
                    });
                }
            }
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Abschließen", "Complete")).clicked() {
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
            self.finish_sprint(cx, &mut d);
        }
        if !close {
            self.sprints.complete = Some(d);
        }
    }

    /// Schließt den Sprint ab und merkt sich die Zusammenfassung für den Dialog.
    pub(super) fn finish_sprint(&mut self, cx: &Cx, d: &mut CompleteDlg) {
        let lang = cx.lang;
        let Some(leftover) = logic::leftover_from(d.choice, d.target) else {
            d.err = Some(crate::planning::sprints::SprintError::LeftoverInvalid.message(lang));
            return;
        };
        match complete_sprint(cx.conn, d.sprint_id, leftover) {
            Ok(r) => {
                let target_name = match leftover {
                    crate::planning::sprints::Leftover::ToSprint(id) => d.planned.iter().find(|(i, _)| *i == id).map(|(_, n)| n.as_str()),
                    _ => None,
                };
                d.result = Some(logic::complete_summary(r.done.len(), r.leftover.len(), leftover, target_name, lang));
                d.err = None;
                self.sprint_changed();
            }
            Err(e) => d.err = Some(e.message(lang)),
        }
    }

    fn calendar_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.sprints.cal_dlg.take() else { return };
        let lang = cx.lang;
        let (mut save, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("sprint_cal")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 460.0));
            widgets::dialog_title(ui, lang.t("Kalender des Projekts", "Project calendar"));
            widgets::hint(ui, lang.t("Bestimmt die Arbeitstage für die Dauer eines Sprints, die Ideal-Linie im Burndown und die verbleibenden Tage.", "Defines the work days used for the length of a sprint, the ideal line of the burndown and the remaining days."));
            ui.add_space(S2);
            widgets::check(ui, &mut d.exclude_weekends, lang.t("Wochenenden ausschließen (Montag bis Freitag)", "Exclude weekends (Monday to Friday)"));
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Feiertage und freie Tage", "Holidays and days off")).strong());
            widgets::hint(ui, lang.t("Je Zeile ein Datum (TT.MM.JJJJ oder JJJJ-MM-TT).", "One date per line (DD.MM.YYYY or YYYY-MM-DD)."));
            ui.add(egui::TextEdit::multiline(&mut d.holidays).desired_rows(5).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Speichern", "Save")).clicked() {
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
        if save {
            match logic::parse_holidays(&d.holidays) {
                Ok(h) => {
                    let c = CalSettings { exclude_weekends: d.exclude_weekends, holidays: h };
                    data::save_cal(cx.db, d.project, &c);
                    self.sprints.cal = c;
                    self.sprints.cal_rev += 1;
                    self.sprints.invalidate();
                    return;
                }
                Err(bad) => {
                    d.err = Some(match lang {
                        Lang::De => format!("„{bad}“ ist kein Datum."),
                        Lang::En => format!("“{bad}” is not a date."),
                    });
                }
            }
        }
        if !close {
            self.sprints.cal_dlg = Some(d);
        }
    }
}
