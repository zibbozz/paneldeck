//! Oberfläche für Projekte und ihre Konfiguration (Expertenmodus des Reiters „Todo-Liste“): Projektauswahl, Übersicht,
//! Assistent „Neues Projekt“, Projekt-Einstellungen und die Editoren für Typen, Felder, Status, Workflows,
//! Verknüpfungstypen, Arten der Arbeit und Vorlagen. Die Daten liegen in `crate::planning` (Datenschicht ohne Oberfläche).
//!
//! * Der Expertenmodus ist ein Schalter im Kopf des Reiters (gespeichert in `meta.expert_mode`); aus = die einfache Liste.
//! * Die Farbe des gewählten Projekts ist im ganzen Reiter die Akzentfarbe ([`PlanUi::accent`], gesetzt von der Anwendung).
//! * Jeder sichtbare Text steht in beiden Sprachen (`lang.t(de, en)`), Fehler der Datenschicht über `PlanError::message`.

mod backup_ed;
mod board_cfg;
mod board_logic;
mod board_view;
mod delete;
mod fields_ed;
mod help_shots;
mod helpers;
mod keywords_ed;
mod kinds_ed;
mod links_ed;
mod overview;
mod project_form;
mod quick_ed;
mod quick_exec;
mod quick_logic;
mod quick_menu;
mod query_bar;
mod query_help_ui;
mod query_logic;
mod saved_filters;
mod search_tab;
mod status_ed;
mod template_ed;
mod ticket_create;
mod ticket_data;
mod ticket_dialogs;
mod ticket_draft;
mod ticket_inputs;
mod ticket_state;
mod ticket_tab;
#[cfg(test)]
mod ticket_ui_tests;
mod ticket_view;
mod types_ed;
mod values;
mod widgets;
mod workflow_ed;

pub(super) use helpers::color_of;
pub(super) use quick_exec::SessionRequest;
pub(super) use quick_menu::Out as QuickOut;
pub(super) use widgets::type_icon;

use super::tasks_ui::Env;
use crate::db::Db;
use crate::planning::link_types::{list_link_types, LinkType};
use crate::planning::projects::{list_projects, Project};
use crate::planning::summary::{project_summary, ProjectSummary};
use crate::planning::PlanError;
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Align, Color32, Id, Layout, Pos2, Rect, RichText, Ui, UiBuilder, Vec2};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Was die Editoren pro Bild brauchen.
pub(super) struct Cx<'a> {
    pub conn: &'a rusqlite::Connection,
    pub db: &'a Db,
    pub lang: Lang,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Tickets,
    Overview,
    Search,
    Boards,
    Types,
    Fields,
    Statuses,
    Workflows,
    Links,
    WorkKinds,
    Templates,
    Quick,
}

/// Rückmeldung unter den Reitern (Fehler rot, Erfolg gedämpft); verschwindet nach einigen Sekunden.
struct Msg {
    text: String,
    ok: bool,
    at: Instant,
}

pub(super) struct PlanUi {
    loaded: bool,
    /// Expertenmodus an (Projektansicht) oder aus (einfache Liste).
    pub(super) expert: bool,
    dirty: bool,
    projects: Vec<Project>,
    /// Gewähltes Projekt.
    selected: i64,
    sum: Option<Rc<ProjectSummary>>,
    links: Vec<LinkType>,
    tab: Tab,
    msg: Option<Msg>,
    wizard: Option<project_form::Form>,
    settings: Option<(i64, project_form::Form)>,
    delete_project: Option<project_form::DeleteAsk>,
    del: Option<delete::Dlg>,
    types: types_ed::State,
    fields: fields_ed::State,
    statuses: status_ed::State,
    workflow: workflow_ed::State,
    links_state: links_ed::State,
    kinds: kinds_ed::State,
    templates: template_ed::State,
    todo: overview::Todo,
    /// Reiter „Suche“ (Abfrage-Leiste, siehe `search_tab`)
    search: search_tab::State,
    /// Hilfeseite der Abfragesprache (Vollfläche über dem Reiterinhalt)
    help: query_help_ui::HelpState,
    /// Dialog „JQL-Schlüsselwörter“
    kw: keywords_ed::State,
    /// Ticket-Oberfläche (Liste, Ansicht, Dialoge, Rückgängig-Historie).
    tickets: ticket_state::TicketUi,
    boards: board_view::State,
    quick: quick_menu::State,
    quick_ed: quick_ed::State,
}

impl Default for PlanUi {
    fn default() -> Self {
        PlanUi {
            loaded: false,
            expert: false,
            dirty: true,
            projects: Vec::new(),
            selected: 0,
            sum: None,
            links: Vec::new(),
            tab: Tab::Tickets,
            msg: None,
            wizard: None,
            settings: None,
            delete_project: None,
            del: None,
            types: Default::default(),
            fields: Default::default(),
            statuses: Default::default(),
            workflow: Default::default(),
            links_state: Default::default(),
            kinds: Default::default(),
            templates: Default::default(),
            todo: Default::default(),
            search: Default::default(),
            help: Default::default(),
            kw: Default::default(),
            tickets: Default::default(),
            boards: Default::default(),
            quick: Default::default(),
            quick_ed: Default::default(),
        }
    }
}

impl PlanUi {
    /// Gespeicherten Zustand laden (einmalig, sobald die Datenbank da ist).
    pub(super) fn ensure_loaded(&mut self, db: Option<&Db>) {
        let Some(db) = db else { return };
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.expert = db.meta("expert_mode").is_some_and(|v| v == "1");
        self.selected = db.meta("expert_project").and_then(|v| v.parse().ok()).unwrap_or(0);
        self.dirty = true;
    }

    /// Akzentfarbe des Reiters: im Expertenmodus die Farbe des gewählten Projekts, sonst `None` (Standardakzent).
    pub(super) fn accent(&self) -> Option<Color32> {
        if !self.expert {
            return None;
        }
        // Während der Assistent oder die Einstellungen offen sind, zeigt die Akzentfarbe live die gewählte Projektfarbe.
        if let Some(c) = self.wizard.as_ref().or(self.settings.as_ref().map(|(_, f)| f)).and_then(project_form::Form::color_parsed) {
            return Some(c);
        }
        self.projects.iter().find(|p| p.id == self.selected).map(|p| helpers::color_of(&p.color))
    }

    fn reload(&mut self, cx: &Cx) {
        self.dirty = false;
        self.projects = list_projects(cx.conn).unwrap_or_default();
        if !self.projects.iter().any(|p| p.id == self.selected) {
            self.selected = self.projects.first().map(|p| p.id).unwrap_or(0);
        }
        self.sum = project_summary(cx.conn, self.selected).ok().map(Rc::new);
        self.links = list_link_types(cx.conn).unwrap_or_default();
        self.todo.dirty = true;
        self.search.stale = true;
        self.tickets.invalidate();
        self.tickets.counts_dirty = true;
    }

    fn project(&self) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == self.selected)
    }

    fn select(&mut self, cx: &Cx, id: i64) {
        if id != self.selected {
            self.selected = id;
            cx.db.set_meta("expert_project", &id.to_string());
            self.tickets.switch_project(id);
            self.types = Default::default();
            self.fields = Default::default();
            self.workflow = Default::default();
            self.dirty = true;
            self.msg = None;
            if self.projects.iter().find(|p| p.id == id).is_some_and(|p| p.system) && !matches!(self.tab, Tab::Overview | Tab::Links) {
                self.tab = Tab::Overview;
            }
        }
    }

    /// Hinweise der Ticket-Oberfläche für den Toast-Stapel der Anwendung abholen.
    pub(super) fn take_toasts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.tickets.toasts)
    }

    /// Die Daten haben sich außerhalb der Oberfläche geändert (z. B. durch einen Agenten): beim nächsten Bild neu laden.
    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub(super) fn set_expert(&mut self, db: &Db, on: bool) {
        self.expert = on;
        db.set_meta("expert_mode", if on { "1" } else { "0" });
        self.dirty = true;
    }

    /// Ergebnis einer Änderung auswerten: Fehler als Meldung, Erfolg → Daten neu laden.
    fn run<T>(&mut self, cx: &Cx, r: Result<T, PlanError>) -> Option<T> {
        match r {
            Ok(v) => {
                self.dirty = true;
                self.msg = None;
                Some(v)
            }
            Err(e) => {
                self.fail(cx, &e);
                None
            }
        }
    }

    fn fail(&mut self, cx: &Cx, e: &PlanError) {
        self.msg = Some(Msg { text: e.message(cx.lang), ok: false, at: Instant::now() });
    }

    fn note(&mut self, text: String) {
        self.msg = Some(Msg { text, ok: true, at: Instant::now() });
    }

    // ------------------------------------------------------------------ Kopf des Reiters

    /// Kopf des Reiters „Todo-Liste“: im Expertenmodus Projektauswahl, immer der Schalter „Expertenmodus“.
    pub(super) fn header(&mut self, ctx: &egui::Context, env: &Env, db: &Db) {
        let lang = env.lang;
        let cx = Cx { conn: &db.conn, db, lang };
        if self.dirty {
            self.reload(&cx);
        }
        let fill = env.bg;
        let mut new_project = false;
        let mut pick: Option<i64> = None;
        let mut expert = self.expert;
        egui::TopBottomPanel::top("tasks_head").exact_height(44.0).show_separator_line(false).frame(egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(14, 0))).show(ctx, |ui| {
            let rect = ui.max_rect();
            let toggle_w = 210.0;
            let right = Rect::from_min_max(Pos2::new(rect.right() - toggle_w, rect.top()), rect.max);
            let left = Rect::from_min_max(rect.min, Pos2::new(rect.right() - toggle_w - S2, rect.bottom()));
            ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
                let tip = lang.t(
                    "Expertenmodus: Projekte mit eigenen Typen, Feldern, Status und Workflows. Aus: die einfache Todo-Liste.",
                    "Expert mode: projects with their own types, fields, statuses and workflows. Off: the simple to-do list.",
                );
                let r = style::toggle(ui, &mut expert).on_hover_text(tip);
                let _ = r;
                ui.label(RichText::new(lang.t("Expertenmodus", "Expert mode")).color(if expert { ui.visuals().text_color() } else { style::dim(ui) }));
            });
            if self.expert {
                ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
                    egui::ScrollArea::horizontal().id_salt("plan_projects").auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            for p in &self.projects {
                                let col = helpers::color_of(&p.color);
                                let r = style::pill(ui, &p.name, None, col, p.id == self.selected);
                                let r = if p.system { r.on_hover_text(lang.t("Schreibgeschützt im Expertenmodus", "Read-only in expert mode")) } else { r.on_hover_text(format!("{} · {}", p.key, p.name)) };
                                if p.system {
                                    let c = Pos2::new(r.rect.right() + 8.0, r.rect.center().y);
                                    widgets::paint_lock(ui.painter(), c, style::dim(ui));
                                    ui.add_space(14.0);
                                }
                                if r.clicked() {
                                    pick = Some(p.id);
                                }
                            }
                            if ui.add(egui::Button::new(format!("+ {}", lang.t("Neues Projekt", "New project"))).small()).on_hover_text(lang.t("Neues Projekt anlegen …", "Create a new project …")).clicked() {
                                new_project = true;
                            }
                        });
                    });
                });
            }
            ui.painter().line_segment([Pos2::new(rect.left() - 14.0, rect.bottom() - 0.5), Pos2::new(rect.right() + 14.0, rect.bottom() - 0.5)], egui::Stroke::new(1.0_f32, Color32::from_white_alpha(16)));
        });
        if expert != self.expert {
            self.set_expert(db, expert);
        }
        if let Some(id) = pick {
            self.select(&cx, id);
        }
        if new_project {
            self.wizard = Some(project_form::Form::new_project(&self.projects));
        }
    }

    // ------------------------------------------------------------------ Seite

    /// Die Projektansicht (Expertenmodus): Reiter, Rückmeldung und der Inhalt des gewählten Reiters, dazu die Dialoge.
    pub(super) fn page(&mut self, ctx: &egui::Context, env: &Env, db: &Db) {
        let cx = Cx { conn: &db.conn, db, lang: env.lang };
        if self.dirty {
            self.reload(&cx);
        }
        self.ticket_refresh(&cx, env.undo_steps);
        self.ticket_keys(ctx, &cx);
        let side = self.tab == Tab::Tickets && self.tickets.open.is_some() && self.tickets.mode == ticket_state::ViewMode::Panel && !self.project().is_some_and(|p| p.system);
        if side {
            self.ticket_panel(ctx, &cx, env.bg);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(env.bg)).show(ctx, |ui| {
            if self.help.open {
                // Die Hilfe liegt wie ein Ticket über dem Reiterinhalt (Vollfläche); „X“ oder Esc schließt sie.
                let out = query_help_ui::help_page(ui, &mut self.help, cx.lang, true);
                if let Some(q) = out.use_query {
                    self.search.bar.set_query(&q);
                    self.show_search_tab();
                    self.help.open = false;
                } else if out.close {
                    self.help.open = false;
                }
            } else {
                self.body(ui, &cx);
            }
        });
        self.dialogs(ctx, &cx);
        self.toast(ctx);
        if self.dirty {
            self.reload(&cx);
            ctx.request_repaint();
        }
    }

    /// Kurzer Hinweis unten in der Mitte nach einer gelungenen Aktion (verschiebt keinen Inhalt).
    fn toast(&mut self, ctx: &egui::Context) {
        let Some(m) = self.msg.as_ref().filter(|m| m.ok) else { return };
        if m.at.elapsed() > Duration::from_secs(5) {
            self.msg = None;
            return;
        }
        let text = m.text.clone();
        egui::Area::new(Id::new("plan_toast")).anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -18.0)).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(egui::CornerRadius::same(10)).show(ui, |ui| {
                ui.label(text);
            });
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn tabs(&self, lang: Lang) -> Vec<(Tab, &'static str)> {
        let system = self.project().is_some_and(|p| p.system);
        let mut v = Vec::new();
        if !system {
            v.push((Tab::Tickets, lang.t("Tickets", "Tickets")));
        }
        v.push((Tab::Overview, lang.t("Übersicht", "Overview")));
        v.push((Tab::Search, lang.t("Suche", "Search")));
        if !system {
            v.push((Tab::Boards, lang.t("Boards", "Boards")));
            v.extend([
                (Tab::Types, lang.t("Typen", "Types")),
                (Tab::Fields, lang.t("Felder", "Fields")),
                (Tab::Statuses, lang.t("Status", "Statuses")),
                (Tab::Workflows, lang.t("Workflows", "Workflows")),
                (Tab::Quick, lang.t("Schnellaktionen", "Quick actions")),
            ]);
        }
        v.push((Tab::Links, lang.t("Verknüpfungstypen", "Link types")));
        if !system {
            v.extend([(Tab::WorkKinds, lang.t("Arten der Arbeit", "Kinds of work")), (Tab::Templates, lang.t("Vorlagen & Sicherung", "Templates & backup"))]);
        }
        v
    }

    fn body(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(proj) = self.project().cloned() else {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.label(RichText::new(lang.t("Kein Projekt vorhanden.", "No project available.")).color(style::dim(ui)));
            });
            return;
        };
        let avail = ui.available_rect_before_wrap();
        let wide = self.tab == Tab::Tickets;
        let w = (avail.width() - 40.0).clamp(300.0, if wide { 1600.0 } else { 1040.0 });
        let x0 = avail.center().x - w / 2.0;
        let col = Rect::from_min_size(Pos2::new(x0, avail.top() + S3), Vec2::new(w, avail.height() - S3));
        let mut below = avail.top() + S3;
        ui.scope_builder(UiBuilder::new().max_rect(col), |ui| {
            // Ticket als eigene Seite: ohne Reiter, mit Zurück-Pfeil
            if self.tab == Tab::Tickets && self.tickets.open.is_some() && self.tickets.mode == ticket_state::ViewMode::Page && !proj.system {
                self.ticket_page(ui, cx);
                return;
            }
            // Reiter innerhalb des Projekts
            let tabs = self.tabs(lang);
            if !tabs.iter().any(|(t, _)| *t == self.tab) {
                self.tab = Tab::Overview;
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                for (t, label) in &tabs {
                    if style::tab(ui, label, self.tab == *t).clicked() {
                        self.tab = *t;
                        self.msg = None;
                    }
                }
            });
            ui.add_space(S2);
            if self.tab == Tab::Tickets {
                self.tickets_tab(ui, cx, &proj);
                return;
            }
            // Fehler stehen unter den Reitern (bis zur nächsten gelungenen Änderung), Erfolgsmeldungen als kurzer Hinweis unten.
            if let Some(m) = self.msg.as_ref().filter(|m| !m.ok) {
                let text = m.text.clone();
                ui.horizontal_wrapped(|ui| widgets::error(ui, &text));
                ui.add_space(S1);
            }
            // Die Boards brauchen die ganze Breite und Höhe (eigene Rollflächen) – sie werden unterhalb gezeichnet.
            if self.tab == Tab::Boards {
                below = ui.cursor().top();
                return;
            }
            egui::ScrollArea::vertical().id_salt(("plan_page", self.tab as u8)).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(w - 8.0);
                match self.tab {
                    Tab::Tickets => {}
                    Tab::Overview => self.overview(ui, cx, &proj),
                    Tab::Search => self.search_tab(ui, cx),
                    Tab::Boards => {}
                    Tab::Types => self.types_tab(ui, cx),
                    Tab::Fields => self.fields_tab(ui, cx),
                    Tab::Statuses => self.statuses_tab(ui, cx),
                    Tab::Workflows => self.workflow_tab(ui, cx),
                    Tab::Links => self.links_tab(ui, cx),
                    Tab::WorkKinds => self.kinds_tab(ui, cx),
                    Tab::Templates => self.templates_tab(ui, cx),
                    Tab::Quick => self.quick_tab(ui, cx),
                }
                ui.add_space(40.0);
            });
        });
        if self.tab == Tab::Boards {
            let full = Rect::from_min_max(Pos2::new(avail.left() + S3, below), Pos2::new(avail.right() - S3, avail.bottom()));
            ui.scope_builder(UiBuilder::new().max_rect(full), |ui| self.boards_tab(ui, cx, &proj));
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        self.wizard_dialog(ctx, cx);
        self.board_dialogs(ctx, cx);
        self.settings_dialog(ctx, cx);
        self.fields_dialogs(ctx, cx);
        self.workflow_dialogs(ctx, cx);
        self.template_dialogs(ctx, cx);
        self.delete_dialog(ctx, cx);
        self.delete_project_dialog(ctx, cx);
        self.quick_ed_dialogs(ctx, cx);
        self.quick_dialogs(ctx, cx);
        self.keywords_dialog_ui(ctx, cx);
        self.ticket_dialogs(ctx, cx);
    }

    /// Schnellzugriff für Dialoge: Id eines Modals dieses Bereichs.
    fn modal_id(name: &str) -> Id {
        Id::new(("plan_modal", name))
    }
}
