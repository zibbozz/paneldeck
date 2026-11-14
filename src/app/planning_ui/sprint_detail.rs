//! Unteransichten „Aktiver Sprint“ und „Berichte“ des Backlog-Reiters: Zusammenfassung (Umfang, erledigt, offen, verbleibende Tage),
//! Burndown-Diagramm, Sprint-Bericht (geplant/erledigt/hinzugefügt/entfernt/offen mit Ticketlisten) und das Velocity-Diagramm der
//! letzten abgeschlossenen Sprints. Die Berechnung kommt aus `planning::sprints::analysis` und `calc::burndown`, das Zeichnen
//! der Diagramme aus `sprint_chart.rs`.

use super::sprint_chart as chart;
use super::sprint_data::{self as data, Detail, Group};
use super::sprint_logic as logic;
use super::sprint_view::{active_of, progress_bar, REFRESH};
use super::widgets;
use super::{Cx, PlanUi};
use crate::calc::burndown::VelocityPoint;
use crate::calc::progress::Basis;
use crate::planning::sprints::sprint::SprintState;
use crate::planning::sprints::{clock, Figure, SprintReport};
use crate::planning::summary::ProjectSummary;
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use std::rc::Rc;
use std::time::Instant;

/// Zwischenspeicher der Sprint-Ansicht (wird alle paar Sekunden und bei Änderungen neu geladen).
pub(super) struct DetailCache {
    key: (i64, Basis, u32, Lang),
    at: Instant,
    pub detail: Detail,
}

/// Zwischenspeicher der Velocity.
pub(super) struct VelCache {
    key: (i64, usize, Basis),
    at: Instant,
    pub points: Vec<VelocityPoint>,
}

/// Wahlmöglichkeiten für die Zahl der Sprints im Velocity-Diagramm.
const VEL_CHOICES: [usize; 4] = [3, 6, 10, 15];

/// Was in der Ansicht angeklickt wurde.
enum Act {
    Open(i64),
    Complete(i64),
    Edit(i64),
}

impl PlanUi {
    // ------------------------------------------------------------------ Aktiver Sprint

    pub(super) fn active_view(&mut self, ui: &mut Ui, cx: &Cx, sum: &Rc<ProjectSummary>) {
        let lang = cx.lang;
        match active_of(cx, sum.project.id) {
            Some(s) => self.sprint_detail(ui, cx, s.id),
            None => {
                widgets::card(ui, |ui| {
                    ui.label(RichText::new(lang.t("Kein aktiver Sprint.", "No active sprint.")).strong());
                    ui.add_space(S1);
                    widgets::hint(ui, lang.t("Starte einen geplanten Sprint im Reiter „Backlog“. Abgeschlossene Sprints findest du unter „Berichte“.", "Start a planned sprint in the “Backlog” tab. Completed sprints are under “Reports”."));
                    ui.add_space(S2);
                    if ui.button(lang.t("Zum Backlog", "Go to backlog")).clicked() {
                        self.sprints.sub = super::sprint_view::Sub::Backlog;
                    }
                });
            }
        }
    }

    // ------------------------------------------------------------------ Berichte

    pub(super) fn reports_view(&mut self, ui: &mut Ui, cx: &Cx, sum: &Rc<ProjectSummary>) {
        let lang = cx.lang;
        let pid = sum.project.id;
        if self.sprints.detail.is_none() || self.sprints.report_list.is_empty() {
            self.sprints.report_list = data::report_sprints(cx.conn, pid).unwrap_or_default();
        }
        let list = self.sprints.report_list.clone();
        if !list.iter().any(|s| Some(s.id) == self.sprints.report_sprint) {
            self.sprints.report_sprint = list.first().map(|s| s.id);
        }
        if let Some(cur) = self.sprints.report_sprint {
            let mut pick = cur;
            ui.horizontal(|ui| {
                ui.label(RichText::new(lang.t("Sprint:", "Sprint:")).color(style::dim(ui)));
                let name = list.iter().find(|s| s.id == cur).map(|s| format!("{} ({})", s.name, s.state.label(lang))).unwrap_or_default();
                egui::ComboBox::from_id_salt("report_sprint").selected_text(name).width(280.0).show_ui(ui, |ui| {
                    for s in &list {
                        ui.selectable_value(&mut pick, s.id, format!("{} ({})", s.name, s.state.label(lang)));
                    }
                });
            });
            if pick != cur {
                self.sprints.report_sprint = Some(pick);
                self.sprints.detail = None;
            }
            ui.add_space(S2);
            self.sprint_detail(ui, cx, pick);
        } else {
            widgets::hint(ui, lang.t("Noch kein gestarteter Sprint – ohne ihn gibt es keinen Bericht.", "No sprint has been started yet, so there is no report."));
        }
        ui.add_space(S3);
        self.velocity_card(ui, cx, pid);
    }

    // ------------------------------------------------------------------ Sprint-Ansicht

    fn detail_load(&mut self, cx: &Cx, id: i64) {
        let st = &mut self.sprints;
        let key = (id, st.basis, st.cal_rev, cx.lang);
        if st.detail.as_ref().is_some_and(|c| c.key == key && c.at.elapsed() < REFRESH) {
            return;
        }
        let cal = st.cal.calendar();
        match data::load_detail(cx.conn, id, st.basis, &cal, clock::today()) {
            Ok(detail) => st.detail = Some(DetailCache { key, at: Instant::now(), detail }),
            Err(e) => {
                st.detail = None;
                self.sprint_fail(cx.lang, &e);
            }
        }
    }

    /// Zusammenfassung, Burndown und Bericht eines Sprints.
    fn sprint_detail(&mut self, ui: &mut Ui, cx: &Cx, id: i64) {
        let lang = cx.lang;
        self.detail_load(cx, id);
        let Some(cache) = self.sprints.detail.take() else { return };
        let d = &cache.detail;
        let basis = self.sprints.basis;
        let cal = self.sprints.cal.calendar();
        let today = clock::today();
        let sp = &d.sprint;
        let mut act: Option<Act> = None;
        // Kopf
        ui.horizontal(|ui| {
            ui.label(RichText::new(&sp.name).heading().strong());
            ui.label(RichText::new(sp.state.label(lang)).color(if sp.state == SprintState::Active { style::accent() } else { style::dim(ui) }));
            if let (Some(s), Some(e)) = (sp.start_date, sp.end_date) {
                ui.label(RichText::new(format!("{} – {}", logic::fmt_date_ui(s, lang), logic::fmt_date_ui(e, lang))).color(style::dim(ui)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if sp.state == SprintState::Active {
                    if widgets::primary(ui, lang.t("Abschließen …", "Complete …")).clicked() {
                        act = Some(Act::Complete(sp.id));
                    }
                }
                if sp.state != SprintState::Completed && ui.button(lang.t("Bearbeiten", "Edit")).clicked() {
                    act = Some(Act::Edit(sp.id));
                }
            });
        });
        if !sp.goal.trim().is_empty() {
            ui.label(RichText::new(format!("{} {}", lang.t("Ziel:", "Goal:"), sp.goal.trim())).italics().color(style::dim(ui)));
        }
        ui.add_space(S2);
        // Zusammenfassung
        let total = &d.totals.total;
        let done = &d.totals.done;
        let open_w = (total.weight(basis) - done.weight(basis)).max(0.0);
        let unit = logic::unit_label(basis, lang);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            stat_card(ui, lang.t("Umfang", "Scope"), &logic::weight_text(total, basis, lang), &logic::figure_text(total, lang));
            stat_card(ui, lang.t("Erledigt", "Done"), &logic::weight_text(done, basis, lang), &format!("{} %", (logic::done_fraction(done, total, basis) * 100.0).round() as i64));
            stat_card(ui, lang.t("Offen", "Open"), &format!("{} {unit}", logic::points_text(open_w)), &logic::tickets_text(total.tickets.saturating_sub(done.tickets) as usize, lang));
            if let (Some(s), Some(e)) = (sp.start_date, sp.end_date) {
                match sp.state {
                    SprintState::Active => {
                        let left = data::remaining_work_days(s, e, &cal, today);
                        let over = today > e;
                        let sub = if over { lang.t("Das Ende ist überschritten", "Past the end date").to_string() } else { format!("{} {}", lang.t("bis", "until"), logic::fmt_date_ui(e, lang)) };
                        stat_card(ui, lang.t("Verbleibende Arbeitstage", "Work days left"), &left.to_string(), &sub);
                    }
                    _ => {
                        let n = cal.work_days_inclusive(s, e);
                        stat_card(ui, lang.t("Dauer", "Length"), &format!("{n} {}", lang.t("Arbeitstage", "work days")), "");
                    }
                }
            }
        });
        ui.add_space(S1);
        progress_bar(ui, ui.available_width().min(520.0), 8.0, logic::done_fraction(done, total, basis), chart::DONE_GREEN);
        ui.add_space(S3);
        // Burndown
        ui.label(RichText::new(lang.t("Burndown", "Burndown")).strong().size(15.0));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            chart::legend_item(ui, style::dim(ui), lang.t("Ideal", "Ideal"), true);
            chart::legend_item(ui, style::accent(), lang.t("Ist (Rest am Tagesende)", "Actual (remaining at end of day)"), true);
            chart::legend_item(ui, chart::SCOPE_UP, lang.t("Umfang gewachsen", "Scope grew"), true);
            ui.label(RichText::new(format!("{} {unit}", lang.t("Einheit:", "Unit:"))).small().color(style::dim(ui)));
        });
        match &d.series {
            Some(series) => {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 270.0), Sense::hover());
                let day = clock::today();
                let drawn_day = data::burn_day(sp, day);
                let bp = chart::paint_burndown(ui, rect, series, &cal, drawn_day, lang);
                if let Some(pos) = resp.hover_pos() {
                    if let Some(i) = chart::day_at(bp.plot, bp.days, pos.x) {
                        let x0 = bp.plot.left() + bp.day_w * i as f32;
                        ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x0, bp.plot.top()), Pos2::new(x0 + bp.day_w, bp.plot.bottom())), 0.0, Color32::from_white_alpha(10));
                        if let Some(t) = chart::burn_tooltip(series, i, unit, lang) {
                            resp.on_hover_text_at_pointer(t);
                        }
                    }
                }
                ui.label(RichText::new(match lang {
                    Lang::De => format!("Startumfang {} · hinzugekommen {} · entfernt {} · erledigt {} ({unit})", logic::points_text(series.scope_total_start), logic::points_text(series.scope_added), logic::points_text(series.scope_removed), logic::points_text(series.completed)),
                    Lang::En => format!("Start scope {} · added {} · removed {} · completed {} ({unit})", logic::points_text(series.scope_total_start), logic::points_text(series.scope_added), logic::points_text(series.scope_removed), logic::points_text(series.completed)),
                }).small().color(style::dim(ui)));
            }
            None => widgets::hint(ui, lang.t("Ohne Start- und Enddatum gibt es kein Burndown. Setze die Termine mit „Bearbeiten“.", "Without start and end dates there is no burndown. Set them with “Edit”.")),
        }
        ui.add_space(S3);
        // Bericht
        ui.label(RichText::new(lang.t("Sprint-Bericht", "Sprint report")).strong().size(15.0));
        ui.add_space(S1);
        for (g, items) in &d.groups {
            let fig = group_figure(&d.report, *g);
            let title = format!("{} · {}", g.label(lang), logic::figure_text(fig, lang));
            egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt(("sprint_group", sp.id, *g as u8)).default_open(matches!(g, Group::Completed | Group::Open)).show(ui, |ui| {
                widgets::hint(ui, g.hint(lang));
                if items.is_empty() {
                    ui.label(RichText::new(lang.t("– keine –", "– none –")).color(style::dim(ui)));
                }
                for it in items {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&it.key).monospace().small().color(style::dim(ui)));
                        if ui.add(egui::Label::new(&it.title).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            act = Some(Act::Open(it.id));
                        }
                        if let Some(p) = it.points {
                            ui.label(RichText::new(format!("{} P", logic::points_text(p))).small().color(style::dim(ui)));
                        }
                    });
                }
            });
        }
        self.sprints.detail = Some(cache);
        match act {
            Some(Act::Open(t)) => self.request_open_sprint_ticket(t),
            Some(Act::Complete(s)) => self.open_complete(cx, s),
            Some(Act::Edit(s)) => {
                if let Ok(sp) = crate::planning::sprints::sprint::get_sprint(cx.conn, s) {
                    self.open_edit_sprint(lang, &sp);
                }
            }
            None => {}
        }
        ui.ctx().request_repaint_after(REFRESH);
    }

    // ------------------------------------------------------------------ Velocity

    fn velocity_card(&mut self, ui: &mut Ui, cx: &Cx, pid: i64) {
        let lang = cx.lang;
        let mut n = self.sprints.vel_n;
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang.t("Velocity", "Velocity")).strong().size(15.0));
            ui.label(RichText::new(lang.t("letzte", "last")).color(style::dim(ui)));
            for c in VEL_CHOICES {
                if style::tab(ui, &c.to_string(), n == c).clicked() {
                    n = c;
                }
            }
            ui.label(RichText::new(lang.t("abgeschlossene Sprints", "completed sprints")).color(style::dim(ui)));
        });
        if n != self.sprints.vel_n {
            self.sprints.vel_n = n;
            self.sprints.velocity = None;
        }
        let basis = self.sprints.basis;
        let key = (pid, self.sprints.vel_n, basis);
        if !self.sprints.velocity.as_ref().is_some_and(|c| c.key == key && c.at.elapsed() < REFRESH) {
            match data::load_velocity(cx.conn, pid, self.sprints.vel_n, basis) {
                Ok(points) => self.sprints.velocity = Some(VelCache { key, at: Instant::now(), points }),
                Err(e) => {
                    self.sprints.velocity = None;
                    self.sprint_fail(lang, &e);
                }
            }
        }
        let Some(cache) = self.sprints.velocity.as_ref() else { return };
        let points = &cache.points;
        let unit = logic::unit_label(basis, lang);
        if points.is_empty() {
            widgets::hint(ui, lang.t("Noch kein abgeschlossener Sprint.", "No completed sprint yet."));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            chart::legend_item(ui, style::dim(ui).gamma_multiply(0.55), lang.t("Geplant", "Planned"), false);
            chart::legend_item(ui, chart::DONE_GREEN, lang.t("Erledigt", "Completed"), false);
            chart::legend_item(ui, style::accent(), lang.t("Durchschnitt (erledigt)", "Average (completed)"), true);
            if let Some(a) = chart::velocity_average(points) {
                ui.label(RichText::new(match lang {
                    Lang::De => format!("Ø {} {unit} je Sprint", logic::points_text((a * 10.0).round() / 10.0)),
                    Lang::En => format!("avg {} {unit} per sprint", logic::points_text((a * 10.0).round() / 10.0)),
                }).small().strong());
            }
        });
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 240.0), Sense::hover());
        let vp = chart::paint_velocity(ui, rect, points, lang);
        if let Some(pos) = resp.hover_pos() {
            if let Some(i) = chart::bar_at(vp.plot, points.len(), pos.x) {
                resp.on_hover_text_at_pointer(chart::velocity_tooltip(&points[i], unit, lang));
            }
        }
    }
}

/// Summe der Gruppe im Bericht.
fn group_figure(r: &SprintReport, g: Group) -> &Figure {
    match g {
        Group::Planned => &r.planned,
        Group::Completed => &r.completed,
        Group::Added => &r.added,
        Group::Removed => &r.removed,
        Group::Open => &r.open,
    }
}

/// Kleine Kennzahl-Karte.
fn stat_card(ui: &mut Ui, title: &str, value: &str, sub: &str) {
    let v = ui.visuals();
    egui::Frame::NONE
        .fill(v.faint_bg_color)
        .stroke(Stroke::new(1.0_f32, v.widgets.noninteractive.bg_stroke.color))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width(110.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).small().color(style::dim(ui)));
                ui.label(RichText::new(value).strong().size(17.0));
                if !sub.is_empty() {
                    ui.label(RichText::new(sub).small().color(style::dim(ui)));
                }
            });
        });
}
