//! Reiter „Releases“ der Projektansicht (nur bei `use_versions`): Versionen und Meilensteine mit Fälligkeit, Fortschritt
//! (To Do / In Arbeit / Erledigt), „als veröffentlicht/erreicht markieren“, Anlegen, Umbenennen, Sortieren und Löschen. Wird eine
//! Version oder ein Meilenstein noch von Tickets benutzt, fragt das Löschen nach Ersatz oder Verwerfen (`DeleteMode`).

use super::sprint_data::{self as data, bar_widths, progress_parts, ReleaseRow};
use super::sprint_logic as logic;
use super::sprint_view::REFRESH;
use super::widgets;
use super::{Cx, PlanUi};
use crate::calc::progress::Basis;
use crate::planning::integrity::DeleteMode;
use crate::planning::projects::Project;
use crate::planning::sprints::releases::{create_release, delete_release, release_usage, reorder_releases, set_release_done, update_release, NewRelease, ReleaseKind, UpdateRelease};
use crate::planning::sprints::{clock, MAX_DESCRIPTION_CHARS};
use crate::state::Lang;
use crate::style::{self, Icon, S1, S2};
use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Sense, Ui, Vec2};
use std::time::Instant;

/// Farbe „in Arbeit“ im Fortschrittsbalken.
const PROGRESS_BLUE: Color32 = Color32::from_rgb(0x58, 0xa6, 0xe6);

/// Dialog „Version/Meilenstein anlegen oder bearbeiten“.
pub(super) struct ReleaseForm {
    pub(super) id: Option<i64>,
    pub(super) kind: ReleaseKind,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) due: String,
    pub(super) err: Option<String>,
}

/// Dialog „Löschen“ (mit Ersatz, wenn der Eintrag benutzt wird).
pub(super) struct ReleaseDel {
    pub(super) id: i64,
    pub(super) kind: ReleaseKind,
    pub(super) name: String,
    pub(super) usage: i64,
    /// Verwendungen auf den Ersatz umhängen (sonst verwerfen).
    replace: bool,
    to: Option<i64>,
    cands: Vec<(i64, String)>,
    err: Option<String>,
}

pub(super) struct State {
    project: i64,
    pub(super) kind: ReleaseKind,
    pub(super) rows: Vec<ReleaseRow>,
    key: Option<(ReleaseKind, i64, Basis, Lang)>,
    loaded_at: Instant,
    stale: bool,
    counts: (usize, usize),
    pub(super) form: Option<ReleaseForm>,
    pub(super) del: Option<ReleaseDel>,
}

impl Default for State {
    fn default() -> Self {
        State { project: 0, kind: ReleaseKind::Version, rows: Vec::new(), key: None, loaded_at: Instant::now(), stale: true, counts: (0, 0), form: None, del: None }
    }
}

impl State {
    pub(super) fn invalidate(&mut self) {
        self.stale = true;
    }
}

pub(super) enum Act {
    New,
    Edit(i64),
    Delete(i64),
    Done(i64, bool),
    Move(usize, i32),
}

/// Beschriftung der Aktion „fertig markieren“.
pub(super) fn done_action_label(kind: ReleaseKind, done: bool, lang: Lang) -> &'static str {
    match (kind, done) {
        (ReleaseKind::Version, false) => lang.t("Als veröffentlicht markieren", "Mark as released"),
        (ReleaseKind::Version, true) => lang.t("Veröffentlichung zurücknehmen", "Undo release"),
        (ReleaseKind::Milestone, false) => lang.t("Als erreicht markieren", "Mark as reached"),
        (ReleaseKind::Milestone, true) => lang.t("Erreicht zurücknehmen", "Undo reached"),
    }
}

/// Text unter dem Fortschrittsbalken: „3 To Do · 2 In Arbeit · 5 Erledigt · 50 %“.
pub(super) fn progress_text(todo: u32, in_progress: u32, done: u32, percent: u32, lang: Lang) -> String {
    format!("{todo} {} · {in_progress} {} · {done} {} · {percent} %", lang.t("To Do", "To do"), lang.t("In Arbeit", "In progress"), lang.t("Erledigt", "Done"))
}

impl PlanUi {
    /// Der Reiter „Releases“.
    pub(super) fn releases_tab(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        if self.releases.project != proj.id {
            self.releases = State { project: proj.id, ..State::default() };
        }
        self.releases_load(cx, proj.id);
        let mut kind = self.releases.kind;
        let mut acts: Vec<Act> = Vec::new();
        let (nv, nm) = self.releases.counts;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if style::pill(ui, lang.t("Versionen", "Versions"), Some(nv), style::accent(), kind == ReleaseKind::Version).clicked() {
                kind = ReleaseKind::Version;
            }
            if style::pill(ui, lang.t("Meilensteine", "Milestones"), Some(nm), style::accent(), kind == ReleaseKind::Milestone).clicked() {
                kind = ReleaseKind::Milestone;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = match kind {
                    ReleaseKind::Version => lang.t("Neue Version", "New version"),
                    ReleaseKind::Milestone => lang.t("Neuer Meilenstein", "New milestone"),
                };
                if widgets::primary(ui, &format!("+ {label}")).clicked() {
                    acts.push(Act::New);
                }
            });
        });
        ui.add_space(S2);
        let today = clock::today();
        if self.releases.rows.is_empty() {
            widgets::hint(ui, match kind {
                ReleaseKind::Version => lang.t("Noch keine Version. Versionen fassen Tickets zu einem Release zusammen (Feld „Version“ am Ticket).", "No version yet. Versions group tickets into a release (the “Version” field on a ticket)."),
                ReleaseKind::Milestone => lang.t("Noch kein Meilenstein. Meilensteine markieren Termine, auf die Tickets hinarbeiten (Feld „Meilenstein“ am Ticket).", "No milestone yet. Milestones mark dates that tickets work towards (the “Milestone” field on a ticket)."),
            });
        }
        let n = self.releases.rows.len();
        for (i, row) in self.releases.rows.iter().enumerate() {
            release_card(ui, lang, row, i, n, today, &mut acts);
            ui.add_space(S2);
        }
        if kind != self.releases.kind {
            self.releases.kind = kind;
            self.releases.stale = true;
        }
        for a in acts {
            self.release_act(cx, proj, a);
        }
        ui.ctx().request_repaint_after(REFRESH);
    }

    fn releases_load(&mut self, cx: &Cx, project: i64) {
        let basis = self.sprints.basis;
        let st = &mut self.releases;
        let key = (st.kind, project, basis, cx.lang);
        if !st.stale && st.key == Some(key) && st.loaded_at.elapsed() < REFRESH {
            return;
        }
        let r = data::load_releases(cx.conn, st.kind, project, basis);
        st.stale = false;
        st.key = Some(key);
        st.loaded_at = Instant::now();
        let other = if st.kind == ReleaseKind::Version { ReleaseKind::Milestone } else { ReleaseKind::Version };
        let count_of = |k: ReleaseKind| crate::planning::sprints::releases::list_releases(cx.conn, k, project).map(|v| v.len()).unwrap_or(0);
        let (c_this, c_other) = (count_of(st.kind), count_of(other));
        st.counts = if st.kind == ReleaseKind::Version { (c_this, c_other) } else { (c_other, c_this) };
        match r {
            Ok(rows) => st.rows = rows,
            Err(e) => {
                st.rows.clear();
                self.sprint_fail(cx.lang, &e);
            }
        }
    }

    pub(super) fn release_act(&mut self, cx: &Cx, proj: &Project, a: Act) {
        let lang = cx.lang;
        let kind = self.releases.kind;
        match a {
            Act::New => self.releases.form = Some(ReleaseForm { id: None, kind, name: String::new(), description: String::new(), due: String::new(), err: None }),
            Act::Edit(id) => {
                if let Some(r) = self.releases.rows.iter().find(|r| r.release.id == id) {
                    let rel = &r.release;
                    self.releases.form = Some(ReleaseForm { id: Some(id), kind, name: rel.name.clone(), description: rel.description.clone(), due: rel.due_date.map(|d| logic::fmt_date_ui(d, lang)).unwrap_or_default(), err: None });
                }
            }
            Act::Delete(id) => self.ask_delete_release(cx, id),
            Act::Done(id, done) => match set_release_done(cx.conn, kind, id, done) {
                Ok(()) => self.sprint_changed(),
                Err(e) => self.sprint_fail(lang, &e),
            },
            Act::Move(idx, dir) => {
                let ids: Vec<i64> = self.releases.rows.iter().map(|r| r.release.id).collect();
                if let Some(new) = logic::move_in_list(&ids, idx, dir) {
                    match reorder_releases(cx.conn, kind, proj.id, &new) {
                        Ok(()) => self.sprint_changed(),
                        Err(e) => self.sprint_fail(lang, &e),
                    }
                }
            }
        }
    }

    fn ask_delete_release(&mut self, cx: &Cx, id: i64) {
        let kind = self.releases.kind;
        let Some(row) = self.releases.rows.iter().find(|r| r.release.id == id) else { return };
        let usage = match release_usage(cx.conn, kind, id) {
            Ok(u) => u,
            Err(e) => {
                self.sprint_fail(cx.lang, &e);
                return;
            }
        };
        let cands: Vec<(i64, String)> = self.releases.rows.iter().filter(|r| r.release.id != id).map(|r| (r.release.id, r.release.name.clone())).collect();
        let to = cands.first().map(|c| c.0);
        self.releases.del = Some(ReleaseDel { id, kind, name: row.release.name.clone(), usage, replace: usage > 0 && to.is_some(), to, cands, err: None });
    }

    // ------------------------------------------------------------------ Dialoge

    pub(super) fn release_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        self.release_form_dialog(ctx, cx);
        self.release_delete_dialog(ctx, cx);
    }

    fn release_form_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut f) = self.releases.form.take() else { return };
        let lang = cx.lang;
        let (mut save, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("release_form")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 480.0));
            let title = match (f.id.is_some(), f.kind) {
                (false, ReleaseKind::Version) => lang.t("Neue Version", "New version"),
                (false, ReleaseKind::Milestone) => lang.t("Neuer Meilenstein", "New milestone"),
                (true, ReleaseKind::Version) => lang.t("Version bearbeiten", "Edit version"),
                (true, ReleaseKind::Milestone) => lang.t("Meilenstein bearbeiten", "Edit milestone"),
            };
            widgets::dialog_title(ui, title);
            ui.label(RichText::new(lang.t("Name", "Name")).strong());
            ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Fällig am (optional)", "Due date (optional)")).strong());
            ui.add(egui::TextEdit::singleline(&mut f.due).desired_width(140.0).margin(egui::Margin::symmetric(8, 5)));
            widgets::hint(ui, lang.t("TT.MM.JJJJ oder JJJJ-MM-TT; leer = ohne Fälligkeit.", "DD.MM.YYYY or YYYY-MM-DD; empty = no due date."));
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Beschreibung", "Description")).strong());
            ui.add(egui::TextEdit::multiline(&mut f.description).desired_rows(3).desired_width(f32::INFINITY).char_limit(MAX_DESCRIPTION_CHARS).margin(egui::Margin::symmetric(8, 5)));
            if let Some(e) = &f.err {
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
            match self.save_release_form(cx, &f) {
                Ok(()) => return,
                Err(e) => f.err = Some(e),
            }
        }
        if !close {
            self.releases.form = Some(f);
        }
    }

    pub(super) fn save_release_form(&mut self, cx: &Cx, f: &ReleaseForm) -> Result<(), String> {
        let lang = cx.lang;
        let due = logic::parse_opt_date(&f.due).map_err(|e| e.message(lang))?;
        let r = match f.id {
            None => create_release(cx.conn, f.kind, &NewRelease { project_id: self.selected, name: f.name.clone(), description: f.description.clone(), due_date: due }).map(|_| ()),
            Some(id) => update_release(cx.conn, f.kind, id, &UpdateRelease { name: Some(f.name.clone()), description: Some(f.description.clone()), due_date: Some(due) }),
        };
        match r {
            Ok(()) => {
                self.sprint_changed();
                self.note(lang.t("Gespeichert.", "Saved.").to_string());
                Ok(())
            }
            Err(e) => Err(e.message(lang)),
        }
    }

    fn release_delete_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut d) = self.releases.del.take() else { return };
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let noun = d.kind.label(lang);
        let m = egui::Modal::new(Self::modal_id("release_delete")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 500.0));
            let title = match lang {
                Lang::De => format!("{noun} {} löschen?", lang.q(&d.name)),
                Lang::En => format!("Delete {} {}?", noun.to_lowercase(), lang.q(&d.name)),
            };
            widgets::dialog_title(ui, &title);
            if d.usage == 0 {
                ui.label(lang.t("Kein Ticket verweist darauf. Das lässt sich nicht rückgängig machen.", "No ticket refers to it. This cannot be undone."));
            } else {
                ui.label(RichText::new(match lang {
                    Lang::De => format!("{} verweisen darauf.", logic::tickets_text(d.usage as usize, lang)),
                    Lang::En => format!("{} refer to it.", logic::tickets_text(d.usage as usize, lang)),
                }).strong());
                ui.add_space(S1);
                ui.label(lang.t("Ersatz wählen oder verwerfen:", "Choose a replacement or discard:"));
                ui.add_enabled_ui(!d.cands.is_empty(), |ui| {
                    widgets::radio(ui, &mut d.replace, true, lang.t("Auf einen Ersatz umhängen:", "Move to a replacement:"));
                });
                if d.cands.is_empty() {
                    d.replace = false;
                    widgets::hint(ui, lang.t("Es gibt keinen anderen Eintrag als Ersatz.", "There is no other entry to use as a replacement."));
                } else if d.replace {
                    ui.indent("release_del_to", |ui| {
                        let sel = d.cands.iter().find(|c| Some(c.0) == d.to).map(|c| c.1.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("release_del_pick").selected_text(sel).width(260.0).show_ui(ui, |ui| {
                            for (id, name) in &d.cands {
                                ui.selectable_value(&mut d.to, Some(*id), name);
                            }
                        });
                    });
                }
                widgets::radio(ui, &mut d.replace, false, lang.t("Verwerfen: das Feld der Tickets wird geleert.", "Discard: the ticket field is cleared."));
            }
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
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
            let mode = delete_mode(d.usage, d.replace, d.to);
            match delete_release(cx.conn, d.kind, d.id, mode) {
                Ok(()) => {
                    self.sprint_changed();
                    self.note(lang.t("Gelöscht.", "Deleted.").to_string());
                    return;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.releases.del = Some(d);
        }
    }
}

/// Löschmodus aus der Auswahl im Dialog: ohne Verwendungen keiner nötig, sonst Ersatz oder Verwerfen.
pub(super) fn delete_mode(usage: i64, replace: bool, to: Option<i64>) -> Option<DeleteMode> {
    if usage <= 0 {
        return None;
    }
    match (replace, to) {
        (true, Some(to)) => Some(DeleteMode::Migrate { to }),
        _ => Some(DeleteMode::Discard),
    }
}

/// Eine Karte für Version oder Meilenstein.
fn release_card(ui: &mut Ui, lang: Lang, row: &ReleaseRow, idx: usize, n: usize, today: chrono::NaiveDate, acts: &mut Vec<Act>) {
    let r = &row.release;
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new(&r.name).strong().size(15.0));
            let state_col = if r.done { super::sprint_chart::DONE_GREEN } else { style::dim(ui) };
            ui.label(RichText::new(r.kind.done_label(r.done, lang)).small().color(state_col));
            if let Some(d) = r.due_date {
                let over = r.is_overdue(today);
                let text = format!("{} {}", lang.t("fällig", "due"), logic::fmt_date_ui(d, lang));
                let col = if over { widgets::DANGER } else { style::dim(ui) };
                ui.label(RichText::new(text).small().color(col));
                if over {
                    ui.label(RichText::new(lang.t("überfällig", "overdue")).small().strong().color(widgets::DANGER));
                }
            }
            if r.done {
                if let Some(at) = &r.done_at {
                    ui.label(RichText::new(at.chars().take(10).collect::<String>()).small().color(style::dim(ui)));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::small_danger(ui, lang.t("Löschen", "Delete")).clicked() {
                    acts.push(Act::Delete(r.id));
                }
                if ui.add(egui::Button::new(RichText::new(lang.t("Bearbeiten", "Edit")).small())).clicked() {
                    acts.push(Act::Edit(r.id));
                }
                ui.add_enabled_ui(idx + 1 < n, |ui| {
                    if style::icon_btn_small(ui, Icon::Down, lang.t("Nach unten", "Move down")).clicked() {
                        acts.push(Act::Move(idx, 1));
                    }
                });
                ui.add_enabled_ui(idx > 0, |ui| {
                    if style::icon_btn_small(ui, Icon::Up, lang.t("Nach oben", "Move up")).clicked() {
                        acts.push(Act::Move(idx, -1));
                    }
                });
            });
        });
        if !r.description.trim().is_empty() {
            ui.label(RichText::new(r.description.trim()).small().color(style::dim(ui)));
        }
        ui.add_space(S1);
        // Fortschritt: drei Abschnitte
        let total_w = ui.available_width().min(560.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, 8.0), Sense::hover());
        ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.inactive.bg_fill.gamma_multiply(1.4));
        let widths = bar_widths(progress_parts(&row.progress), total_w);
        let colors = [style::dim(ui).gamma_multiply(0.7), PROGRESS_BLUE, super::sprint_chart::DONE_GREEN];
        let mut x = rect.left();
        for (w, c) in widths.iter().zip(colors) {
            if *w > 0.0 {
                ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new((x + w).min(rect.right()), rect.bottom())), 0.0, c);
                x += w;
            }
        }
        let p = &row.progress;
        ui.label(RichText::new(progress_text(p.todo, p.in_progress, p.done, p.progress.percent(), lang)).small().color(style::dim(ui)));
        ui.add_space(S1);
        if ui.button(done_action_label(r.kind, r.done, lang)).clicked() {
            acts.push(Act::Done(r.id, !r.done));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_mode_follows_the_dialog() {
        assert_eq!(delete_mode(0, true, Some(3)), None, "ohne Verwendungen kein Modus");
        assert_eq!(delete_mode(4, true, Some(3)), Some(DeleteMode::Migrate { to: 3 }));
        assert_eq!(delete_mode(4, false, Some(3)), Some(DeleteMode::Discard));
        assert_eq!(delete_mode(4, true, None), Some(DeleteMode::Discard), "ohne Ersatz bleibt nur Verwerfen");
    }

    #[test]
    fn labels_exist_in_both_languages() {
        for kind in [ReleaseKind::Version, ReleaseKind::Milestone] {
            for done in [false, true] {
                assert_ne!(done_action_label(kind, done, Lang::De), done_action_label(kind, done, Lang::En));
            }
        }
        assert_eq!(progress_text(3, 2, 5, 50, Lang::En), "3 To do · 2 In progress · 5 Done · 50 %");
        assert_eq!(progress_text(3, 2, 5, 50, Lang::De), "3 To Do · 2 In Arbeit · 5 Erledigt · 50 %");
    }
}
