//! Reiter „Backlog“ der Projektansicht (nur bei `use_sprints`): Sprint-Liste und Backlog mit Ziehen & Fallenlassen (auch mehrere
//! Tickets per Strg/Umschalt), Kontextmenü, dazu die Unteransichten „Aktiver Sprint“ und „Berichte“ (siehe `sprint_detail.rs`).
//!
//! Einhängepunkte:
//! * `PlanUi::backlog_tab` wird vom Reiter „Backlog“ aufgerufen (`mod.rs`, in der gemeinsamen Rollfläche);
//! * „Ticket öffnen“ (Doppelklick, Kontextmenü) ruft `PlanUi::open_ticket` (`ticket_nav.rs`); „Zurück“ führt wieder zum Backlog;
//! * `PlanUi::sprint_dialogs` (in `sprint_dlg.rs`) zeichnet die Dialoge der Sprints und der Releases.
//!
//! Die reine Logik steht in `sprint_logic.rs` und `sprint_chart.rs`, die Daten in `sprint_data.rs`; hier ist nur Zeichnen und
//! Ereignisse einsammeln.

use super::quick_menu::QuickTarget;
use super::sprint_data::{self as data, BacklogData, Row};
use super::sprint_detail::{DetailCache, VelCache};
use super::sprint_dlg::{CalDlg, CompleteDlg, DeleteAsk, SprintForm};
use super::sprint_logic::{self as logic, CalSettings, Section, SectionGeom, Selection};
use super::widgets;
use super::{Cx, PlanUi};
use crate::calc::progress::Basis;
use crate::planning::projects::Project;
use crate::planning::sprints::sprint::{active_sprint, Sprint, SprintState};
use crate::planning::sprints::SprintError;
use crate::planning::summary::ProjectSummary;
use crate::planning::tickets::POINTS_FIELD_KEY;
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, text::{LayoutJob, TextFormat, TextWrapping}, Align, Color32, FontId, Id, Key, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Höhe einer Ticket-Zeile.
const ROW_H: f32 = 30.0;
/// So viele Zeilen zeigt ein Bereich zunächst (dann „n weitere …“).
pub(super) const PAGE: usize = 40;
/// Abstand zum Rand, ab dem beim Ziehen gerollt wird, und Rollgeschwindigkeit je Bild.
const EDGE: f32 = 48.0;
const SCROLL_STEP: f32 = 18.0;
/// So oft werden die Daten neu geladen, falls sich anderswo (Agent, anderer Reiter) etwas ändert.
pub(super) const REFRESH: Duration = Duration::from_millis(2500);

/// Unteransichten des Reiters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sub {
    Backlog,
    Active,
    Reports,
}

/// Eine laufende Ziehaktion.
struct Drag {
    /// Die gezogenen Tickets in Anzeigereihenfolge.
    ids: Vec<i64>,
    /// Das angefasste Ticket (für die Anzeige am Mauszeiger).
    lead: i64,
}

pub(super) struct State {
    project: i64,
    pub(super) sub: Sub,
    pub(super) basis: Basis,
    /// Arbeitskalender des Projekts (Wochenenden, Feiertage), gespeichert in `meta`.
    pub(super) cal: CalSettings,
    pub(super) cal_rev: u32,
    pub(super) has_points: bool,
    pub(super) data: Option<BacklogData>,
    data_key: Option<(bool, Lang, u32)>,
    loaded_at: Instant,
    sum_seen: Option<Rc<ProjectSummary>>,
    /// Neu laden (nach eigener Änderung).
    pub(super) stale: bool,
    pub(super) show_done: bool,
    pub(super) selection: Selection,
    drag: Option<Drag>,
    /// Lage der Bereiche im letzten Bild (Bereiche, linke und rechte Kante) – für Tests und das Fallenlassen.
    pub(super) layout: Option<(Vec<SectionGeom>, (f32, f32))>,
    collapsed: HashSet<Section>,
    pages: HashMap<Section, usize>,
    pub(super) form: Option<SprintForm>,
    pub(super) del: Option<DeleteAsk>,
    pub(super) complete: Option<CompleteDlg>,
    pub(super) cal_dlg: Option<CalDlg>,
    // Unteransichten „Aktiver Sprint“ und „Berichte“
    pub(super) report_sprint: Option<i64>,
    pub(super) report_list: Vec<Sprint>,
    pub(super) vel_n: usize,
    pub(super) detail: Option<DetailCache>,
    pub(super) velocity: Option<VelCache>,
}

impl Default for State {
    fn default() -> Self {
        State::new(0, false, CalSettings::default())
    }
}

impl State {
    pub(super) fn new(project: i64, has_points: bool, cal: CalSettings) -> Self {
        State {
            project,
            sub: Sub::Backlog,
            basis: default_basis(has_points),
            cal,
            cal_rev: 0,
            has_points,
            data: None,
            data_key: None,
            loaded_at: Instant::now(),
            sum_seen: None,
            stale: true,
            show_done: false,
            selection: Selection::default(),
            drag: None,
            layout: None,
            collapsed: HashSet::new(),
            pages: HashMap::new(),
            form: None,
            del: None,
            complete: None,
            cal_dlg: None,
            report_sprint: None,
            report_list: Vec::new(),
            vel_n: 6,
            detail: None,
            velocity: None,
        }
    }

    /// Läuft gerade eine Ziehaktion?
    pub(super) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Daten der Unteransichten verwerfen (nach Änderungen).
    pub(super) fn invalidate(&mut self) {
        self.stale = true;
        self.detail = None;
        self.velocity = None;
    }
}

/// Standard-Zählweise: Punkte, wenn das Projekt Story Points kennt, sonst Anzahl.
pub(super) fn default_basis(has_points: bool) -> Basis {
    if has_points { Basis::PointsWithFallbackOne } else { Basis::TicketCount }
}

/// Kennt das Projekt ein Story-Points-Feld?
pub(super) fn project_has_points(sum: &ProjectSummary) -> bool {
    sum.types.iter().flat_map(|t| t.fields.iter()).any(|f| f.key.as_deref() == Some(POINTS_FIELD_KEY))
}

/// Was beim Zeichnen angestoßen wurde; wird nach dem Zeichnen ausgeführt.
pub(super) enum Act {
    Click { id: i64, ctrl: bool, shift: bool },
    Select(i64),
    Open(i64),
    /// Schnellmenü für diese Tickets öffnen (Kontextmenü, Taste „.“).
    Quick(Vec<i64>),
    DragStart(i64),
    MoveTo { ids: Vec<i64>, target: Section },
    Toggle(Section),
    More(Section),
    Edit(i64),
    Delete(i64),
    Start(i64),
    Complete(i64),
    ClearSelection,
    ShowDone(bool),
}

/// Was die Zeichenfunktionen der Bereiche brauchen.
struct DrawCx<'a> {
    lang: Lang,
    basis: Basis,
    /// Gezogene Tickets (leer ohne Ziehen).
    dragging: &'a [i64],
    selection: &'a Selection,
    /// Alle sichtbaren Tickets in Anzeigereihenfolge (für Mehrfachauswahl und Kontextmenü).
    order: &'a [i64],
    /// Offene Sprints als Ziele des Kontextmenüs.
    targets: &'a [(i64, String)],
}

impl PlanUi {
    /// Fehler der Sprint-Schicht unter den Reitern anzeigen.
    pub(super) fn sprint_fail(&mut self, lang: Lang, e: &SprintError) {
        self.msg = Some(super::Msg { text: e.message(lang), ok: false, at: Instant::now() });
    }

    /// Nach einer Änderung: Daten der Sprint-Ansichten neu laden und die Projektübersicht auffrischen.
    pub(super) fn sprint_changed(&mut self) {
        self.sprints.invalidate();
        self.releases.invalidate();
        self.dirty = true;
    }

    // ------------------------------------------------------------------ Reiter

    /// Der Reiter „Backlog“ (in der gemeinsamen Rollfläche der Projektansicht).
    pub(super) fn backlog_tab(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let Some(sum) = self.sum.clone() else { return };
        self.sprints_sync(cx, &sum, proj);
        if self.sprints.sub == Sub::Backlog {
            self.sprint_keys(ui.ctx(), cx);
        }
        self.sprints_toolbar(ui, cx, proj);
        ui.add_space(S2);
        match self.sprints.sub {
            Sub::Backlog => self.backlog_view(ui, cx, &sum),
            Sub::Active => self.active_view(ui, cx, &sum),
            Sub::Reports => self.reports_view(ui, cx, &sum),
        }
    }

    /// Zustand zum Projekt passend halten (Wechsel des Projekts, neue Projektübersicht).
    fn sprints_sync(&mut self, cx: &Cx, sum: &Rc<ProjectSummary>, proj: &Project) {
        let has_points = project_has_points(sum);
        if self.sprints.project != proj.id {
            self.sprints = State::new(proj.id, has_points, data::load_cal(cx.db, proj.id));
        }
        let st = &mut self.sprints;
        let changed = !st.sum_seen.as_ref().is_some_and(|s| Rc::ptr_eq(s, sum));
        if changed {
            st.sum_seen = Some(sum.clone());
            st.stale = true;
            st.detail = None;
            st.velocity = None;
            if st.has_points != has_points {
                st.has_points = has_points;
                st.basis = default_basis(has_points);
            }
        }
    }

    fn sprints_toolbar(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let mut sub = self.sprints.sub;
        let mut basis = self.sprints.basis;
        let (mut new_sprint, mut cal) = (false, false);
        let has_points = self.sprints.has_points;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (s, label) in [(Sub::Backlog, lang.t("Backlog", "Backlog")), (Sub::Active, lang.t("Aktiver Sprint", "Active sprint")), (Sub::Reports, lang.t("Berichte", "Reports"))] {
                if style::tab(ui, label, sub == s).clicked() {
                    sub = s;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::primary(ui, &format!("+ {}", lang.t("Neuer Sprint", "New sprint"))).clicked() {
                    new_sprint = true;
                }
                if ui.button(lang.t("Kalender …", "Calendar …")).on_hover_text(lang.t("Arbeitstage und Feiertage dieses Projekts (für Sprint-Dauer, Burndown und verbleibende Tage)", "Work days and holidays of this project (for sprint length, burndown and remaining days)")).clicked() {
                    cal = true;
                }
                if sub != Sub::Backlog {
                    // Zählweise: Reihenfolge von rechts nach links
                    if style::tab(ui, lang.t("Anzahl", "Count"), basis == Basis::TicketCount).on_hover_text(lang.t("Jedes Ticket zählt 1", "Every ticket counts 1")).clicked() {
                        basis = Basis::TicketCount;
                    }
                    let tip = lang.t("Story Points; Tickets ohne Punkte zählen 1", "Story points; tickets without points count 1");
                    if has_points && style::tab(ui, lang.t("Punkte", "Points"), basis != Basis::TicketCount).on_hover_text(tip).clicked() {
                        basis = Basis::PointsWithFallbackOne;
                    }
                }
            });
        });
        if sub != self.sprints.sub {
            self.sprints.sub = sub;
            self.msg = None;
            self.sprints.detail = None;
        }
        if basis != self.sprints.basis {
            self.sprints.basis = basis;
            self.sprints.detail = None;
            self.sprints.velocity = None;
            self.releases.invalidate();
        }
        if new_sprint {
            self.open_new_sprint(cx, proj);
        }
        if cal {
            self.open_calendar(lang, proj.id);
        }
    }

    // ------------------------------------------------------------------ Laden

    fn backlog_load(&mut self, cx: &Cx, sum: &ProjectSummary) {
        let st = &mut self.sprints;
        let key = (st.show_done, cx.lang, st.cal_rev);
        let fresh = !st.stale && st.data_key == Some(key) && st.loaded_at.elapsed() < REFRESH;
        if fresh || (st.drag.is_some() && st.data.is_some()) {
            return;
        }
        let r = data::load_backlog(cx.conn, sum, st.show_done, cx.lang);
        st.stale = false;
        st.data_key = Some(key);
        st.loaded_at = Instant::now();
        match r {
            Ok(d) => st.data = Some(d),
            Err(e) => {
                st.data = None;
                self.sprint_fail(cx.lang, &e);
            }
        }
    }

    // ------------------------------------------------------------------ Backlog

    fn backlog_view(&mut self, ui: &mut Ui, cx: &Cx, sum: &Rc<ProjectSummary>) {
        let lang = cx.lang;
        self.backlog_load(cx, sum);
        let Some(data) = self.sprints.data.take() else { return };
        let ctx = ui.ctx().clone();
        let view = ui.clip_rect();
        // Sichtbare Tickets in Anzeigereihenfolge und aktuelle Auswahl
        let order = {
            let st = &self.sprints;
            data.visible_ids(&|s| st.collapsed.contains(&s), &|s, n| (st.pages.get(&s).copied().unwrap_or(1) * PAGE).min(n))
        };
        self.sprints.selection.retain_visible(&order);
        let selection = self.sprints.selection.clone();
        let dragging: Vec<i64> = self.sprints.drag.as_ref().map(|d| d.ids.clone()).unwrap_or_default();
        let targets: Vec<(i64, String)> = data.sprints.iter().map(|s| (s.sprint.id, s.sprint.name.clone())).collect();
        let d = DrawCx { lang, basis: self.sprints.basis, dragging: &dragging, selection: &selection, order: &order, targets: &targets };
        let mut acts: Vec<Act> = Vec::new();
        let mut geoms: Vec<SectionGeom> = Vec::new();
        let mut section_x: Option<(f32, f32)> = None;
        // Auswahlzeile: immer vorhanden, damit sich das Bild beim Auswählen nicht verschiebt
        ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 26.0), Layout::left_to_right(Align::Center), |ui| {
            if selection.is_empty() {
                ui.label(RichText::new(lang.t("Strg/Umschalt + Klick wählt mehrere Tickets, Ziehen verschiebt alle.", "Ctrl/Shift + click selects several tickets; dragging moves them all.")).small().color(style::dim(ui)));
            } else {
                ui.label(RichText::new(match lang {
                    Lang::De => format!("{} ausgewählt", logic::tickets_text(selection.len(), lang)),
                    Lang::En => format!("{} selected", logic::tickets_text(selection.len(), lang)),
                }).small().color(style::accent()));
                if dragging.is_empty() && ui.add(egui::Button::new(RichText::new(lang.t("Auswahl aufheben", "Clear selection")).small())).clicked() {
                    acts.push(Act::ClearSelection);
                }
            }
        });
        ui.add_space(S1);
        if data.sprints.is_empty() {
            widgets::hint(ui, lang.t("Noch kein offener Sprint. Lege mit „Neuer Sprint“ einen an und ziehe Tickets aus dem Backlog hinein.", "No open sprint yet. Create one with “New sprint” and drag tickets from the backlog into it."));
            ui.add_space(S1);
        }
        for sb in &data.sprints {
            let sec = Section::Sprint(sb.sprint.id);
            let collapsed = self.sprints.collapsed.contains(&sec);
            let shown = (self.sprints.pages.get(&sec).copied().unwrap_or(1) * PAGE).min(sb.rows.len());
            let g = draw_sprint_section(ui, &d, sb, &self.sprints.cal, collapsed, shown, &mut acts);
            section_x = Some(g.1);
            geoms.push(g.0);
            ui.add_space(S2);
        }
        {
            let sec = Section::Backlog;
            let collapsed = self.sprints.collapsed.contains(&sec);
            let shown = (self.sprints.pages.get(&sec).copied().unwrap_or(1) * PAGE).min(data.backlog.len());
            let g = draw_backlog_section(ui, &d, &data, self.sprints.show_done, collapsed, shown, &mut acts);
            section_x = section_x.or(Some(g.1));
            geoms.push(g.0);
        }
        self.sprints.layout = section_x.map(|x| (geoms.clone(), x));
        // Ziehen: Einfügemarke, Anzeige am Mauszeiger, Esc, Loslassen, Rollen am Rand
        if let Some(drag) = self.sprints.drag.as_ref() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let ptr = ctx.input(|i| i.pointer.latest_pos());
            let mut drop = None;
            if let (Some(p), Some((x0, x1))) = (ptr, section_x) {
                if p.x >= x0 && p.x <= x1 {
                    drop = logic::locate_drop(&geoms, p.y);
                }
                if let Some(dr) = drop {
                    if let Some(g) = geoms.iter().find(|g| g.section == dr.section) {
                        let y = logic::insert_line_y(g, dr.idx);
                        ui.painter().line_segment([Pos2::new(x0 + 8.0, y), Pos2::new(x1 - 8.0, y)], Stroke::new(3.0_f32, style::accent()));
                        ui.painter().rect_stroke(Rect::from_min_max(Pos2::new(x0, g.top), Pos2::new(x1, g.bottom)), 8.0, Stroke::new(1.5_f32, style::accent().gamma_multiply(0.7)), StrokeKind::Inside);
                    }
                }
                // automatisches Rollen am Rand der Rollfläche
                if p.y < view.top() + EDGE {
                    ui.scroll_with_delta(Vec2::new(0.0, SCROLL_STEP));
                } else if p.y > view.bottom() - EDGE {
                    ui.scroll_with_delta(Vec2::new(0.0, -SCROLL_STEP));
                }
                // Anzeige am Mauszeiger
                if let Some(row) = data.row(drag.lead) {
                    let more = drag.ids.len().saturating_sub(1);
                    egui::Area::new(Id::new("sprint_ghost")).order(egui::Order::Tooltip).fixed_pos(p + Vec2::new(14.0, 8.0)).interactable(false).constrain(false).show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).stroke(Stroke::new(1.5_f32, style::accent())).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(row.ticket.key()).monospace().small());
                                ui.label(&row.ticket.title);
                                if more > 0 {
                                    ui.label(RichText::new(format!("+{more}")).strong().color(style::accent()));
                                }
                            });
                        });
                    });
                }
            }
            let esc = ctx.input(|i| i.key_pressed(Key::Escape));
            let down = ctx.input(|i| i.pointer.primary_down());
            if esc {
                self.sprints.drag = None;
            } else if !down {
                let drag = self.sprints.drag.take();
                if let (Some(drag), Some(dr)) = (drag, drop) {
                    let plan = logic::plan_drop(dr, &data.section_ids(dr.section), &drag.ids);
                    self.drop_tickets(cx, &data, plan);
                }
            }
        }
        // Ereignisse der Zeilen und Köpfe
        for a in acts {
            self.sprint_act(cx, &order, &data, a);
        }
        // Taste „.“: Schnellmenü für die ausgewählten Tickets (nicht beim Ziehen)
        if self.sprints.drag.is_none() && !self.sprints.selection.is_empty() {
            let targets: Vec<QuickTarget> = self.sprints.selection.in_order(&order).into_iter().map(|ticket_id| QuickTarget { ticket_id }).collect();
            self.quick_shortcut(&ctx, &targets);
        }
        if self.sprints.data.is_none() {
            self.sprints.data = Some(data);
        }
        if self.sprints.drag.is_some() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(REFRESH);
        }
    }

    /// Ergebnis eines Fallenlassens: Datenbank ändern, Meldung zeigen.
    fn drop_tickets(&mut self, cx: &Cx, data: &BacklogData, plan: logic::DropPlan) {
        let lang = cx.lang;
        match data::apply_drop(cx.conn, &plan) {
            Ok((moved, op)) => {
                // gemeinsame Rückgängig-Historie der Ticket-Oberfläche (Strg+Z / Strg+Y, siehe `undo_keys.rs`)
                self.tickets.commit(op);
                self.sprint_changed();
                if moved > 0 {
                    let to = match plan.target {
                        Section::Backlog => lang.t("Backlog", "backlog").to_string(),
                        Section::Sprint(id) => data.sprint(id).map(|s| s.sprint.name.clone()).unwrap_or_default(),
                    };
                    let text = match lang {
                        Lang::De => format!("{} nach {} verschoben.", logic::tickets_text(moved, lang), lang.q(&to)),
                        Lang::En => format!("Moved {} to {}.", logic::tickets_text(moved, lang), lang.q(&to)),
                    };
                    self.note(text);
                }
            }
            Err(e) => self.sprint_fail(lang, &e),
        }
    }

    pub(super) fn sprint_act(&mut self, cx: &Cx, order: &[i64], data: &BacklogData, a: Act) {
        let lang = cx.lang;
        match a {
            Act::Click { id, ctrl, shift } => self.sprints.selection.click(id, ctrl, shift, order),
            Act::Select(id) => {
                if !self.sprints.selection.contains(id) {
                    self.sprints.selection.click(id, false, false, order);
                }
            }
            Act::Open(id) => {
                self.open_ticket(cx, id);
            }
            Act::Quick(ids) => self.open_quick_menu(ids),
            Act::DragStart(id) => {
                let ids = self.sprints.selection.drag_set(id, order);
                if !self.sprints.selection.contains(id) {
                    self.sprints.selection.click(id, false, false, order);
                }
                self.sprints.drag = Some(Drag { ids, lead: id });
            }
            Act::MoveTo { ids, target } => match data::move_to(cx.conn, target, &ids) {
                Ok((_, op)) => {
                    self.tickets.commit(op);
                    self.sprint_changed();
                }
                Err(e) => self.sprint_fail(lang, &e),
            },
            Act::Toggle(sec) => {
                if !self.sprints.collapsed.remove(&sec) {
                    self.sprints.collapsed.insert(sec);
                }
            }
            Act::More(sec) => *self.sprints.pages.entry(sec).or_insert(1) += 1,
            Act::Edit(id) => {
                if let Some(sb) = data.sprint(id) {
                    self.open_edit_sprint(lang, &sb.sprint);
                }
            }
            Act::Delete(id) => {
                if let Some(sb) = data.sprint(id) {
                    self.sprints.del = Some(DeleteAsk { id, name: sb.sprint.name.clone(), tickets: sb.rows.len() });
                }
            }
            Act::Start(id) => self.start_sprint_now(cx, id),
            Act::Complete(id) => self.open_complete(cx, id),
            Act::ClearSelection => self.sprints.selection.clear(),
            Act::ShowDone(on) => {
                self.sprints.show_done = on;
                self.sprints.stale = true;
            }
        }
    }
}

// ------------------------------------------------------------------ Bereiche zeichnen

fn section_frame(ui: &Ui) -> egui::Frame {
    let v = ui.visuals();
    egui::Frame::NONE.fill(v.faint_bg_color).stroke(Stroke::new(1.0_f32, v.widgets.noninteractive.bg_stroke.color)).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(8))
}

/// Kleines Dreieck „zugeklappt“ (nach rechts) oder „offen“ (nach unten).
pub(super) fn paint_chevron(p: &egui::Painter, c: Pos2, open: bool, col: Color32) {
    let pts = if open {
        vec![Pos2::new(c.x - 4.5, c.y - 2.5), Pos2::new(c.x + 4.5, c.y - 2.5), Pos2::new(c.x, c.y + 3.5)]
    } else {
        vec![Pos2::new(c.x - 2.5, c.y - 4.5), Pos2::new(c.x - 2.5, c.y + 4.5), Pos2::new(c.x + 3.5, c.y)]
    };
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
}

/// Dünner Fortschrittsbalken.
pub(super) fn progress_bar(ui: &mut Ui, w: f32, h: f32, frac: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter().rect_filled(rect, h / 2.0, ui.visuals().widgets.inactive.bg_fill.gamma_multiply(1.4));
    if frac > 0.0 {
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new((w * frac).max(h), h)), h / 2.0, color);
    }
}

/// Kopfzeile eines Bereichs: Dreieck zum Auf-/Zuklappen und Titel; liefert die Antwort des Dreiecks.
fn collapse_toggle(ui: &mut Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    let col = if resp.hovered() { ui.visuals().strong_text_color() } else { style::dim(ui) };
    paint_chevron(ui.painter(), rect.center(), open, col);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Ein Sprint-Bereich. Liefert die Lage für das Fallenlassen und die waagerechte Ausdehnung.
fn draw_sprint_section(ui: &mut Ui, d: &DrawCx, sb: &data::SprintBox, cal: &CalSettings, collapsed: bool, shown: usize, acts: &mut Vec<Act>) -> (SectionGeom, (f32, f32)) {
    let lang = d.lang;
    let sec = Section::Sprint(sb.sprint.id);
    let sp = &sb.sprint;
    let mut body_top = 0.0;
    let mut rows: Vec<(f32, f32)> = Vec::new();
    let fr = section_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Zeile 1: Dreieck, Name, Zustand, Termine, Aktionen
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if collapse_toggle(ui, !collapsed).clicked() {
                acts.push(Act::Toggle(sec));
            }
            ui.label(RichText::new(&sp.name).strong().size(15.0));
            let (label, col) = match sp.state {
                SprintState::Active => (sp.state.label(lang), style::accent()),
                _ => (sp.state.label(lang), style::dim(ui)),
            };
            ui.label(RichText::new(label).small().color(col));
            if let (Some(s), Some(e)) = (sp.start_date, sp.end_date) {
                let days = cal.calendar().work_days_inclusive(s, e);
                let text = format!("{} – {}", logic::fmt_date_ui(s, lang), logic::fmt_date_ui(e, lang));
                ui.label(RichText::new(text).small().color(style::dim(ui))).on_hover_text(match lang {
                    Lang::De => format!("{days} Arbeitstage"),
                    Lang::En => format!("{days} work days"),
                });
            } else if let Some(s) = sp.start_date {
                ui.label(RichText::new(format!("{} –", logic::fmt_date_ui(s, lang))).small().color(style::dim(ui)));
            } else {
                ui.label(RichText::new(lang.t("ohne Termine", "no dates")).small().color(style::dim(ui)));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match sp.state {
                    SprintState::Planned => {
                        if ui.add(egui::Button::new(RichText::new(lang.t("Starten", "Start")).strong().small()).fill(style::accent().gamma_multiply(0.4))).on_hover_text(lang.t("Sprint starten (nur ein aktiver Sprint je Projekt)", "Start the sprint (only one active sprint per project)")).clicked() {
                            acts.push(Act::Start(sp.id));
                        }
                        if ui.add(egui::Button::new(RichText::new(lang.t("Bearbeiten", "Edit")).small())).clicked() {
                            acts.push(Act::Edit(sp.id));
                        }
                        if widgets::small_danger(ui, lang.t("Löschen", "Delete")).on_hover_text(lang.t("Nur geplante Sprints; die Tickets landen im Backlog", "Planned sprints only; the tickets go back to the backlog")).clicked() {
                            acts.push(Act::Delete(sp.id));
                        }
                    }
                    SprintState::Active => {
                        if ui.add(egui::Button::new(RichText::new(lang.t("Abschließen …", "Complete …")).strong().small()).fill(style::accent().gamma_multiply(0.4))).clicked() {
                            acts.push(Act::Complete(sp.id));
                        }
                        if ui.add(egui::Button::new(RichText::new(lang.t("Bearbeiten", "Edit")).small())).clicked() {
                            acts.push(Act::Edit(sp.id));
                        }
                    }
                    SprintState::Completed => {}
                }
            });
        });
        if !sp.goal.trim().is_empty() {
            ui.label(RichText::new(format!("{} {}", lang.t("Ziel:", "Goal:"), sp.goal.trim())).small().italics().color(style::dim(ui)));
        }
        // Zeile 2: Summen und Fortschritt
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.add_space(26.0);
            let t = &sb.totals;
            ui.label(RichText::new(format!("{} · {} {}", logic::figure_text(&t.total, lang), t.done.tickets, lang.t("erledigt", "done"))).small().color(style::dim(ui)));
            progress_bar(ui, 120.0, 6.0, logic::done_fraction(&t.done, &t.total, d.basis), super::sprint_chart::DONE_GREEN);
        });
        ui.add_space(S1);
        body_top = ui.cursor().top();
        if !collapsed {
            if sb.rows.is_empty() {
                drop_hint(ui, lang.t("Ziehe Tickets aus dem Backlog hierher.", "Drag tickets from the backlog here."));
            }
            draw_rows(ui, d, &sb.rows, shown, sec, &mut rows, acts);
        }
    });
    let r = fr.response.rect;
    (SectionGeom { section: sec, top: r.top(), bottom: r.bottom(), body_top, rows, collapsed }, (r.left(), r.right()))
}

fn drop_hint(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
    ui.painter().rect_stroke(rect.shrink(1.0), 6.0, Stroke::new(1.0_f32, style::dim(ui).gamma_multiply(0.6)), StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, FontId::proportional(12.5), style::dim(ui));
}

fn draw_backlog_section(ui: &mut Ui, d: &DrawCx, data: &BacklogData, show_done: bool, collapsed: bool, shown: usize, acts: &mut Vec<Act>) -> (SectionGeom, (f32, f32)) {
    let lang = d.lang;
    let sec = Section::Backlog;
    let mut body_top = 0.0;
    let mut rows: Vec<(f32, f32)> = Vec::new();
    let fr = section_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if collapse_toggle(ui, !collapsed).clicked() {
                acts.push(Act::Toggle(sec));
            }
            ui.label(RichText::new(lang.t("Backlog", "Backlog")).strong().size(15.0));
            let mut points = 0.0;
            let mut with = 0;
            for r in &data.backlog {
                if let Some(p) = r.points {
                    points += p;
                    with += 1;
                }
            }
            let mut text = logic::tickets_text(data.backlog.len(), lang);
            if with > 0 {
                text.push_str(&format!(" · {} P", logic::points_text(points)));
            }
            ui.label(RichText::new(text).small().color(style::dim(ui)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut on = show_done;
                let label = if data.hidden_done > 0 && !show_done { format!("{} ({})", lang.t("Erledigte zeigen", "Show done"), data.hidden_done) } else { lang.t("Erledigte zeigen", "Show done").to_string() };
                if widgets::check(ui, &mut on, label).changed() {
                    acts.push(Act::ShowDone(on));
                }
            });
        });
        ui.add_space(S1);
        body_top = ui.cursor().top();
        if !collapsed {
            if data.backlog.is_empty() {
                drop_hint(ui, lang.t("Das Backlog ist leer. Ziehe Tickets aus einem Sprint hierher, um sie herauszunehmen.", "The backlog is empty. Drag tickets here to take them out of a sprint."));
            }
            draw_rows(ui, d, &data.backlog, shown, sec, &mut rows, acts);
        }
    });
    let r = fr.response.rect;
    (SectionGeom { section: sec, top: r.top(), bottom: r.bottom(), body_top, rows, collapsed }, (r.left(), r.right()))
}

// ------------------------------------------------------------------ Zeilen

/// Zeichnet die ersten `shown` Zeilen eines Bereichs und sammelt Lage (ohne gezogene Zeilen) und Ereignisse.
fn draw_rows(ui: &mut Ui, d: &DrawCx, rows: &[Row], shown: usize, sec: Section, rects: &mut Vec<(f32, f32)>, acts: &mut Vec<Act>) {
    let lang = d.lang;
    ui.spacing_mut().item_spacing.y = 2.0;
    for row in rows.iter().take(shown) {
        let id = row.ticket.id;
        let is_dragged = d.dragging.contains(&id);
        let selected = d.selection.contains(id);
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::hover());
        let resp = ui.interact(rect, Id::new(("sprint_row", id)), Sense::click_and_drag());
        if ui.is_rect_visible(rect) {
            paint_row(ui, rect, row, selected, resp.hovered() && d.dragging.is_empty(), is_dragged);
        }
        if resp.hovered() && d.dragging.is_empty() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if !is_dragged {
            rects.push((rect.top(), rect.bottom()));
        }
        if d.dragging.is_empty() {
            let m = ui.input(|i| i.modifiers);
            let plain = !(m.command || m.ctrl || m.shift);
            if resp.drag_started() {
                acts.push(Act::DragStart(id));
            } else if resp.double_clicked() && plain && d.selection.len() == 1 && selected {
                // Doppelklick öffnet nur das Ticket, dessen erster Klick es gerade ausgewählt hat (zwei schnelle Klicks auf verschiedene
                // Zeilen zählen für egui auch als Doppelklick)
                acts.push(Act::Open(id));
            } else if resp.clicked() {
                acts.push(Act::Click { id, ctrl: m.command || m.ctrl, shift: m.shift });
            }
            if resp.secondary_clicked() {
                acts.push(Act::Select(id));
            }
            resp.context_menu(|ui| row_menu(ui, d, id, sec, acts));
        }
    }
    if rows.len() > shown {
        let rest = rows.len() - shown;
        if ui.add_sized([ui.available_width(), 24.0], egui::Button::new(RichText::new(format!("{rest} {} …", lang.t("weitere", "more"))).small())).clicked() {
            acts.push(Act::More(sec));
        }
    }
}

/// Kontextmenü einer Zeile; wirkt auf die ganze Auswahl, wenn die Zeile dazugehört.
fn row_menu(ui: &mut Ui, d: &DrawCx, id: i64, sec: Section, acts: &mut Vec<Act>) {
    let lang = d.lang;
    let ids = d.selection.targets(id, d.order);
    if ui.button(lang.t("Öffnen", "Open")).clicked() {
        acts.push(Act::Open(id));
        ui.close();
    }
    if ui.button(format!("{} (.)", lang.t("Schnellmenü …", "Quick menu …"))).clicked() {
        acts.push(Act::Quick(ids.clone()));
        ui.close();
    }
    ui.separator();
    let others: Vec<&(i64, String)> = d.targets.iter().filter(|(sid, _)| Section::Sprint(*sid) != sec).collect();
    ui.add_enabled_ui(!others.is_empty(), |ui| {
        ui.menu_button(lang.t("In Sprint …", "To sprint …"), |ui| {
            for (sid, name) in &others {
                if ui.button(name).clicked() {
                    acts.push(Act::MoveTo { ids: ids.clone(), target: Section::Sprint(*sid) });
                    ui.close();
                }
            }
        });
    });
    if sec != Section::Backlog && ui.button(lang.t("Ins Backlog", "To backlog")).clicked() {
        acts.push(Act::MoveTo { ids: ids.clone(), target: Section::Backlog });
        ui.close();
    }
    if ids.len() > 1 {
        ui.separator();
        ui.label(RichText::new(logic::tickets_text(ids.len(), lang)).small().color(style::dim(ui)));
    }
}

/// Ein einzeiliger Text mit „…“, wenn er nicht passt.
fn one_line(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32, strike: bool) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.wrap = TextWrapping { max_width: max_w.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let mut fmt = TextFormat { font_id: font, color, ..Default::default() };
    if strike {
        fmt.strikethrough = Stroke::new(1.0_f32, color);
    }
    job.append(text, 0.0, fmt);
    ui.painter().layout_job(job)
}

fn paint_row(ui: &Ui, rect: Rect, row: &Row, selected: bool, hovered: bool, dragged: bool) {
    let v = ui.visuals();
    let fade = if dragged { 0.35 } else if row.done { 0.62 } else { 1.0 };
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 5.0, style::accent().gamma_multiply(0.22));
        p.rect_stroke(rect, 5.0, Stroke::new(1.0_f32, style::accent().gamma_multiply(0.6)), StrokeKind::Inside);
    } else if hovered {
        p.rect_filled(rect, 5.0, v.widgets.hovered.bg_fill.gamma_multiply(0.7));
    }
    let text = v.text_color().gamma_multiply(fade);
    let dim = style::dim(ui).gamma_multiply(fade);
    let tcol = super::helpers::color_of(&row.type_color).gamma_multiply(fade);
    let badge = Rect::from_center_size(Pos2::new(rect.left() + 16.0, rect.center().y), Vec2::splat(18.0));
    widgets::paint_type_badge(p, badge, row.type_icon.as_deref(), tcol);
    // rechts: Status, Punkte
    let mut right = rect.right() - 8.0;
    if !row.status_name.is_empty() {
        let w = style::status_pill(p, Pos2::new(right, rect.center().y), &row.status_name, super::helpers::color_of(&row.status_color).gamma_multiply(fade));
        right -= w + 8.0;
    }
    if let Some(pt) = row.points {
        let g = p.layout_no_wrap(logic::points_text(pt), FontId::proportional(11.5), text);
        let w = (g.size().x + 12.0).max(22.0);
        let r = Rect::from_center_size(Pos2::new(right - w / 2.0, rect.center().y), Vec2::new(w, 18.0));
        p.rect_filled(r, 9.0, v.widgets.inactive.bg_fill.gamma_multiply(1.6 * fade));
        p.galley(Pos2::new(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0), g, text);
        right -= w + 8.0;
    }
    // Schlüssel und Titel
    let key = p.layout_no_wrap(row.ticket.key(), FontId::monospace(11.5), dim);
    let kx = rect.left() + 32.0;
    let key_w = key.size().x;
    p.galley(Pos2::new(kx, rect.center().y - key.size().y / 2.0), key, dim);
    let tx = kx + key_w + 10.0;
    let title = one_line(ui, &row.ticket.title, FontId::proportional(13.5), text, right - tx, row.done);
    p.galley(Pos2::new(tx, rect.center().y - title.size().y / 2.0), title, text);
    if row.ticket.flagged {
        widgets::type_icon(p, "flag", Pos2::new(rect.left() + 3.0, rect.center().y), 9.0, Color32::from_rgb(0xe0, 0x6c, 0x5b).gamma_multiply(fade));
    }
}

/// Aktiver Sprint des Projekts (für die Unteransicht).
pub(super) fn active_of(cx: &Cx, project: i64) -> Option<Sprint> {
    active_sprint(cx.conn, project)
}
