//! Dialoge des Reiters „Dashboards“: neues Dashboard (Name, Bereich, Vorlage), Umbenennen, Löschen mit Bestätigung, Kachel
//! hinzufügen/einstellen (Art, Abfrage oder gespeicherter Filter, Spalten, Diagramm, Projekt, Platz und Größe) und Import mit Vorschau;
//! dazu der Export einer Datei. Die Prüfung der Einstellungen macht die Datenschicht (`views::dashboards`, zweisprachige Fehler).

use super::board_logic::free_name;
use super::dash_logic::{self as logic, KindSel, Template, TileDraft, COLUMNS, KIND_SELS};
use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::projects::{list_projects, project_by_key, Project};
use crate::planning::search::{prepare, Ctx, Scope};
use crate::planning::views::dashboards::{
    create_dashboard, delete_dashboard, export_dashboard, import_dashboard, update_dashboard, Dashboard, DashboardDoc, DashboardScope, ImportTarget, NewDashboard, UpdateDashboard, ChartKind, CountGroup, TimeRange, MAX_TILES,
};
use crate::planning::views::filters::{list_filters, list_filters_for_project, SavedFilter};
use crate::state::Lang;
use crate::style::{self, Icon, S1, S2, S3};
use eframe::egui::{self, RichText, Ui};

pub(super) enum Dlg {
    New(NewDlg),
    Rename(RenameDlg),
    Delete(DeleteDlg),
    Tile(Box<TileDlg>),
    Import(Box<ImportDlg>),
}

/// Dialog „Neues Dashboard“.
pub(super) struct NewDlg {
    name: String,
    /// Das Projekt, in dem der Reiter offen ist (`None` im System-Projekt: dort gibt es nur globale Dashboards).
    project: Option<i64>,
    global: bool,
    template: Template,
    /// Das Projekt nutzt Sprints (für die Übersichts-Vorlage).
    sprints: bool,
    err: Option<String>,
}

pub(super) struct RenameDlg {
    id: i64,
    name: String,
    err: Option<String>,
}

pub(super) struct DeleteDlg {
    id: i64,
    name: String,
}

/// Dialog „Kachel hinzufügen/einstellen“ (Arbeitskopie; gespeichert wird erst mit dem Knopf).
pub(super) struct TileDlg {
    /// `None` = neue Kachel.
    tile: Option<u32>,
    dash_id: i64,
    /// Projekt des Dashboards (`None` = globales Dashboard).
    dash_project: Option<i64>,
    draft: TileDraft,
    err: Option<String>,
    /// Gespeicherte Filter, die sich übernehmen lassen.
    filters: Vec<SavedFilter>,
    /// Wählbare Projekte (Kennung, Schlüssel, Name).
    projects: Vec<(i64, String, String)>,
    /// Zuletzt geprüfte Abfrage und ihr Fehler.
    check: (String, Option<String>),
}

pub(super) struct ImportDlg {
    file: String,
    doc: DashboardDoc,
    target: ImportTarget,
    /// Das Projekt dieses Reiters (Kennung, Schlüssel), falls es ein gewöhnliches ist.
    project: Option<(i64, String)>,
    err: Option<String>,
}

impl Dlg {
    pub(super) fn tile(cx: &Cx, d: &Dashboard, tile: Option<u32>, draft: TileDraft) -> Dlg {
        let filters = match d.project_id {
            Some(p) => list_filters_for_project(cx.conn, p),
            None => list_filters(cx.conn, None),
        }
        .unwrap_or_default();
        let projects = list_projects(cx.conn).unwrap_or_default().into_iter().filter(|p| !p.system).map(|p| (p.id, p.key, p.name)).collect();
        let mut dlg = TileDlg { tile, dash_id: d.id, dash_project: d.project_id, draft, err: None, filters, projects, check: (String::new(), None) };
        dlg.recheck(cx);
        Dlg::Tile(Box::new(dlg))
    }
}

/// Prüft eine Abfrage im Bereich des Dashboards; liefert die (zweisprachige) Fehlermeldung. Leer ist gültig (alle Tickets).
pub(super) fn check_jql(cx: &Cx, dash_project: Option<i64>, jql: &str) -> Option<String> {
    let scope = match dash_project {
        Some(p) => Scope::Project(p),
        None => Scope::All,
    };
    prepare(cx.conn, &Ctx::today_local(cx.lang), scope, jql).err().map(|e| e.message(cx.lang))
}

impl TileDlg {
    /// Prüft die Abfrage neu, wenn sie sich geändert hat.
    fn recheck(&mut self, cx: &Cx) {
        if self.draft.has_query() && (self.check.0 != self.draft.jql || (self.check.0.is_empty() && self.check.1.is_none() && self.draft.jql.is_empty())) {
            self.check = (self.draft.jql.clone(), check_jql(cx, self.dash_project, &self.draft.jql));
        }
    }

    /// Kann gespeichert werden? Eine Abfrage muss gültig sein, Sprint/Burndown im globalen Dashboard brauchen ein Projekt.
    fn problem(&self, lang: Lang) -> Option<String> {
        if self.draft.has_query() {
            if let Some(m) = &self.check.1 {
                return Some(m.clone());
            }
        }
        if self.draft.needs_project() && self.dash_project.is_none() && self.draft.project.is_none() {
            return Some(lang.t("Wähle ein Projekt: Ein globales Dashboard hat keins.", "Choose a project: a global dashboard has none of its own.").to_string());
        }
        None
    }
}

/// Gültigkeit einer Tile-Eingabe, ohne Oberfläche (für Tests).
#[cfg(test)]
fn tile_problem(d: &TileDlg, lang: Lang) -> Option<String> {
    d.problem(lang)
}

impl PlanUi {
    pub(super) fn open_new_dashboard(&mut self, proj: &Project, lang: Lang) {
        let taken: Vec<String> = self.dash.list.iter().map(|d| d.name.clone()).collect();
        let project = (!proj.system).then_some(proj.id);
        self.dash.dlg = Some(Dlg::New(NewDlg { name: free_name(lang.t("Dashboard", "Dashboard"), &taken), project, global: project.is_none(), template: Template::Search, sprints: proj.use_sprints, err: None }));
    }

    pub(super) fn open_rename_dashboard(&mut self, id: i64, name: String) {
        self.dash.dlg = Some(Dlg::Rename(RenameDlg { id, name, err: None }));
    }

    pub(super) fn open_delete_dashboard(&mut self, id: i64, name: String) {
        self.dash.dlg = Some(Dlg::Delete(DeleteDlg { id, name }));
    }

    // ------------------------------------------------------------------ Export / Import

    /// Schreibt das Dashboard als JSON-Datei (Speichern-Dialog).
    pub(super) fn dash_export(&mut self, cx: &Cx, id: i64) {
        let lang = cx.lang;
        let doc = match export_dashboard(cx.conn, id) {
            Ok(d) => d,
            Err(e) => return self.dash.notify(e.message(lang), true),
        };
        let Some(path) = crate::notify::save_json(&format!("{}.json", logic::file_slug(&doc.name))) else { return };
        match std::fs::write(&path, doc.to_json()) {
            Ok(()) => self.dash.notify(
                match lang {
                    Lang::De => format!("Dashboard exportiert: {}", path.display()),
                    Lang::En => format!("Dashboard exported: {}", path.display()),
                },
                false,
            ),
            Err(e) => self.dash.notify(format!("{}: {e}", lang.t("Die Datei konnte nicht geschrieben werden", "The file could not be written")), true),
        }
    }

    /// Liest eine JSON-Datei (Öffnen-Dialog) und zeigt die Vorschau des Imports.
    pub(super) fn dash_start_import(&mut self, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let Some(path) = crate::notify::open_json() else { return };
        let text = match std::fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).to_string(),
            Err(e) => return self.dash.notify(format!("{}: {e}", lang.t("Die Datei konnte nicht gelesen werden", "The file could not be read")), true),
        };
        match DashboardDoc::from_json(&text) {
            Ok(doc) => {
                let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                self.dash.dlg = Some(Dlg::Import(Box::new(new_import(cx, proj, file, doc))));
            }
            Err(e) => self.dash.notify(e.message(lang), true),
        }
    }

    // ------------------------------------------------------------------ Dialoge

    /// Alle Dialoge des Reiters „Dashboards“ (aus `PlanUi::dialogs`).
    pub(super) fn dash_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(dlg) = self.dash.dlg.take() else { return };
        let keep = match dlg {
            Dlg::New(d) => self.new_dialog(ctx, cx, d).map(Dlg::New),
            Dlg::Rename(d) => self.rename_dialog(ctx, cx, d).map(Dlg::Rename),
            Dlg::Delete(d) => self.delete_dashboard_dialog(ctx, cx, d).map(Dlg::Delete),
            Dlg::Tile(d) => self.tile_dialog(ctx, cx, d).map(Dlg::Tile),
            Dlg::Import(d) => self.import_dialog(ctx, cx, d).map(Dlg::Import),
        };
        if self.dash.dlg.is_none() {
            self.dash.dlg = keep;
        }
    }

    fn new_dialog(&mut self, ctx: &egui::Context, cx: &Cx, mut d: NewDlg) -> Option<NewDlg> {
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("dash_new")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 480.0));
            widgets::dialog_title(ui, lang.t("Neues Dashboard", "New dashboard"));
            ui.label(RichText::new(lang.t("Name", "Name")).strong());
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(340.0).margin(egui::Margin::symmetric(8, 5)));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            if d.project.is_some() {
                widgets::section(ui, lang.t("Gilt für", "Applies to"));
                widgets::radio(ui, &mut d.global, false, lang.t("Dieses Projekt", "This project"));
                widgets::radio(ui, &mut d.global, true, lang.t("Alle Projekte (global)", "All projects (global)"));
            } else {
                hint(ui, lang.t("Im System-Projekt gibt es nur globale Dashboards (über alle Projekte).", "The system project only has global dashboards (across all projects)."));
            }
            widgets::section(ui, lang.t("Inhalt", "Content"));
            widgets::radio(ui, &mut d.template, Template::Search, lang.t("Eine große Such-Kachel", "One big search tile"));
            widgets::radio(ui, &mut d.template, Template::Overview, lang.t("Übersicht mit mehreren Kacheln", "Overview with several tiles"));
            hint(
                ui,
                lang.t(
                    "Die Übersicht zeigt Tickets nach Status und Typ, Markierte, zuletzt Geändertes, Favoriten und die Zeiterfassung – im Projekt mit Sprints auch Sprint und Burndown. Alles lässt sich danach ändern.",
                    "The overview shows tickets by status and type, flagged, recently changed, favorites and time tracking – in a project with sprints also sprint and burndown. Everything can be changed afterwards.",
                ),
            );
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, !d.name.trim().is_empty(), lang.t("Anlegen", "Create")).clicked() {
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
        if go && !d.name.trim().is_empty() {
            let project_id = if d.global { None } else { d.project };
            let layout = match d.template {
                Template::Search => None,
                Template::Overview => Some(logic::overview_layout(lang, d.sprints && project_id.is_some(), project_id.is_none())),
            };
            match create_dashboard(cx.conn, &NewDashboard { name: d.name.clone(), project_id, layout }) {
                Ok(id) => {
                    self.dash_select(id);
                    self.dash.notify(lang.t("Dashboard angelegt.", "Dashboard created.").to_string(), false);
                    return None;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        (!close).then_some(d)
    }

    fn rename_dialog(&mut self, ctx: &egui::Context, cx: &Cx, mut d: RenameDlg) -> Option<RenameDlg> {
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("dash_rename")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 420.0));
            widgets::dialog_title(ui, lang.t("Dashboard umbenennen", "Rename dashboard"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(340.0).margin(egui::Margin::symmetric(8, 5)));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, !d.name.trim().is_empty(), lang.t("Speichern", "Save")).clicked() {
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
        if go && !d.name.trim().is_empty() {
            match update_dashboard(cx.conn, d.id, &UpdateDashboard { name: Some(d.name.clone()), layout: None }) {
                Ok(()) => {
                    self.dash.stale = true;
                    return None;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        (!close).then_some(d)
    }

    fn delete_dashboard_dialog(&mut self, ctx: &egui::Context, cx: &Cx, d: DeleteDlg) -> Option<DeleteDlg> {
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("dash_delete")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 460.0));
            widgets::dialog_title(
                ui,
                &match lang {
                    Lang::De => format!("Dashboard {} löschen?", lang.q(&d.name)),
                    Lang::En => format!("Delete dashboard {}?", lang.q(&d.name)),
                },
            );
            ui.label(lang.t("Nur das Dashboard mit seinen Kacheln wird gelöscht – die Tickets bleiben unverändert. Das lässt sich nicht rückgängig machen.", "Only the dashboard with its tiles is deleted — the tickets stay as they are. This cannot be undone."));
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
            match delete_dashboard(cx.conn, d.id) {
                Ok(()) => {
                    self.dash.cur = 0;
                    self.dash.chosen.retain(|_, v| *v != d.id);
                    self.dash.stale = true;
                    self.dash.notify(lang.t("Dashboard gelöscht.", "Dashboard deleted.").to_string(), false);
                }
                Err(e) => self.dash.notify(e.message(lang), true),
            }
            return None;
        }
        (!close).then_some(d)
    }

    fn tile_dialog(&mut self, ctx: &egui::Context, cx: &Cx, mut d: Box<TileDlg>) -> Option<Box<TileDlg>> {
        let lang = cx.lang;
        d.recheck(cx);
        let problem = d.problem(lang);
        let (mut go, mut close) = (false, false);
        let editing = d.tile.is_some();
        let dash_project = d.dash_project;
        let m = egui::Modal::new(Self::modal_id("dash_tile")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 660.0));
            widgets::dialog_title(ui, if editing { lang.t("Kachel einstellen", "Tile settings") } else { lang.t("Kachel hinzufügen", "Add a tile") });
            let TileDlg { draft, filters, projects, check, err, .. } = &mut *d;
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 250.0).max(220.0)).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(ui.available_width() - 8.0);
                ui.label(RichText::new(lang.t("Art", "Kind")).strong());
                egui::ComboBox::from_id_salt("dash_kind").selected_text(draft.sel.label(lang)).width(300.0).show_ui(ui, |ui| {
                    for s in KIND_SELS {
                        if ui.selectable_label(draft.sel == s, s.label(lang)).clicked() && draft.sel != s {
                            draft.sel = s;
                            if !editing {
                                draft.set_default_size();
                            }
                        }
                    }
                });
                hint(ui, draft.sel.explain(lang));
                match draft.sel {
                    KindSel::Search | KindSel::Counts => {
                        if draft.sel == KindSel::Search {
                            widgets::section(ui, lang.t("Titel (optional)", "Title (optional)"));
                            ui.add(egui::TextEdit::singleline(&mut draft.title).desired_width(340.0).margin(egui::Margin::symmetric(8, 5)).hint_text(lang.t("Tickets", "Tickets")));
                        }
                        widgets::section(ui, lang.t("Abfrage (JQL)", "Query (JQL)"));
                        ui.add(
                            egui::TextEdit::multiline(&mut draft.jql)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .margin(egui::Margin::symmetric(8, 5))
                                .hint_text(lang.t("z. B. type = Defect ORDER BY updated DESC – leer = alle Tickets", "e.g. type = Defect ORDER BY updated DESC – empty = all tickets")),
                        );
                        if let Some(msg) = &check.1 {
                            ui.add(egui::Label::new(RichText::new(msg).color(widgets::DANGER)).wrap());
                        }
                        if !filters.is_empty() {
                            ui.add_space(S1);
                            egui::ComboBox::from_id_salt("dash_filter").selected_text(lang.t("Gespeicherten Filter übernehmen …", "Use a saved filter …")).width(300.0).show_ui(ui, |ui| {
                                for f in filters.iter() {
                                    if ui.selectable_label(false, &f.name).on_hover_text(&f.jql).clicked() {
                                        draft.jql = f.jql.clone();
                                        if draft.sel == KindSel::Search && draft.title.trim().is_empty() {
                                            draft.title = f.name.clone();
                                        }
                                    }
                                }
                            });
                            hint(ui, lang.t("Die Abfrage des Filters wird in die Kachel kopiert (spätere Änderungen am Filter wirken nicht).", "The filter's query is copied into the tile (later changes to the filter do not apply)."));
                        }
                        if draft.sel == KindSel::Search {
                            widgets::section(ui, lang.t("Spalten", "Columns"));
                            for c in COLUMNS.iter() {
                                ui.horizontal(|ui| {
                                    let mut on = draft.columns.iter().any(|x| x == c.id);
                                    if widgets::check(ui, &mut on, c.label(lang)).changed() {
                                        draft.toggle_column(c.id);
                                    }
                                    if on {
                                        if style::icon_btn_small(ui, Icon::Up, lang.t("Nach vorn", "Move earlier")).clicked() {
                                            draft.move_column(c.id, -1);
                                        }
                                        if style::icon_btn_small(ui, Icon::Down, lang.t("Nach hinten", "Move later")).clicked() {
                                            draft.move_column(c.id, 1);
                                        }
                                    }
                                });
                            }
                        } else {
                            widgets::section(ui, lang.t("Gruppieren nach", "Group by"));
                            ui.horizontal(|ui| {
                                widgets::radio(ui, &mut draft.group_by, CountGroup::Status, lang.t("Status", "Status"));
                                widgets::radio(ui, &mut draft.group_by, CountGroup::Type, lang.t("Typ", "Type"));
                                widgets::radio(ui, &mut draft.group_by, CountGroup::Priority, lang.t("Priorität", "Priority"));
                            });
                            widgets::section(ui, lang.t("Darstellung", "Chart"));
                            ui.horizontal(|ui| {
                                widgets::radio(ui, &mut draft.chart, ChartKind::Pie, lang.t("Torte", "Pie"));
                                widgets::radio(ui, &mut draft.chart, ChartKind::Bar, lang.t("Balken", "Bars"));
                            });
                        }
                    }
                    KindSel::Sprint | KindSel::Burndown => {
                        widgets::section(ui, lang.t("Projekt", "Project"));
                        let none = if dash_project.is_some() { lang.t("Das Projekt des Dashboards", "The dashboard's project") } else { lang.t("– bitte wählen –", "– please choose –") };
                        project_combo(ui, "dash_proj", &mut draft.project, projects, none);
                        hint(ui, lang.t("Gezeigt wird der aktive Sprint des Projekts (das Projekt muss Sprints nutzen).", "Shows the active sprint of the project (the project must use sprints)."));
                    }
                    KindSel::TimeTracking => {
                        widgets::section(ui, lang.t("Zeitraum", "Range"));
                        ui.horizontal(|ui| {
                            widgets::radio(ui, &mut draft.range, TimeRange::Day, lang.t("Je Tag (14 Tage)", "Per day (14 days)"));
                            widgets::radio(ui, &mut draft.range, TimeRange::Week, lang.t("Je Woche (8 Wochen)", "Per week (8 weeks)"));
                        });
                        widgets::section(ui, lang.t("Projekt", "Project"));
                        let none = if dash_project.is_some() { lang.t("Das Projekt des Dashboards", "The dashboard's project") } else { lang.t("Alle Projekte", "All projects") };
                        project_combo(ui, "dash_proj", &mut draft.project, projects, none);
                    }
                    KindSel::Favorites | KindSel::Flagged | KindSel::Projects => {}
                }
                if editing {
                    widgets::section(ui, lang.t("Platz und Größe (Rasterzellen)", "Position and size (grid cells)"));
                    egui::Grid::new("dash_geom").num_columns(4).spacing(egui::Vec2::new(14.0, 6.0)).show(ui, |ui| {
                        ui.label(RichText::new(lang.t("Spalte", "Column")).color(style::dim(ui)));
                        ui.add(egui::DragValue::new(&mut draft.x).range(0..=11));
                        ui.label(RichText::new(lang.t("Zeile", "Row")).color(style::dim(ui)));
                        ui.add(egui::DragValue::new(&mut draft.y).range(0..=998));
                        ui.end_row();
                        ui.label(RichText::new(lang.t("Breite", "Width")).color(style::dim(ui)));
                        ui.add(egui::DragValue::new(&mut draft.w).range(1..=12));
                        ui.label(RichText::new(lang.t("Höhe", "Height")).color(style::dim(ui)));
                        ui.add(egui::DragValue::new(&mut draft.h).range(1..=24));
                        ui.end_row();
                    });
                }
            });
            ui.add_space(S2);
            if let Some(p) = &problem {
                widgets::error(ui, p);
            } else if let Some(e) = err.as_ref() {
                widgets::error(ui, e);
            }
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, problem.is_none(), if editing { lang.t("Übernehmen", "Apply") } else { lang.t("Hinzufügen", "Add") }).clicked() {
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
        if go && problem.is_none() {
            let Some(dash) = self.dash.list.iter().find(|x| x.id == d.dash_id).cloned() else { return None };
            let mut layout = dash.layout.clone();
            match logic::apply_draft(&mut layout, d.tile, &d.draft) {
                Some(_) => match self.dash_try_save(cx, d.dash_id, layout) {
                    Ok(()) => return None,
                    Err(m) => d.err = Some(m),
                },
                None => {
                    d.err = Some(match lang {
                        Lang::De => format!("Ein Dashboard hat höchstens {MAX_TILES} Kacheln."),
                        Lang::En => format!("A dashboard has at most {MAX_TILES} tiles."),
                    })
                }
            }
        }
        (!close).then_some(d)
    }

    fn import_dialog(&mut self, ctx: &egui::Context, cx: &Cx, mut d: Box<ImportDlg>) -> Option<Box<ImportDlg>> {
        let lang = cx.lang;
        let (mut go, mut close) = (false, false);
        let m = egui::Modal::new(Self::modal_id("dash_import")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 520.0));
            widgets::dialog_title(ui, lang.t("Dashboard importieren", "Import dashboard"));
            ui.label(RichText::new(&d.file).monospace());
            ui.add_space(S2);
            let scope = match (&d.doc.scope, &d.doc.project) {
                (DashboardScope::Project, Some(k)) => format!("{} {k}", lang.t("Projekt", "project")),
                _ => lang.t("global (alle Projekte)", "global (all projects)").to_string(),
            };
            ui.label(match lang {
                Lang::De => format!("{} – {scope}, {} Kachel(n).", lang.q(&d.doc.name), d.doc.layout.tiles.len()),
                Lang::En => format!("{} – {scope}, {} tile(s).", lang.q(&d.doc.name), d.doc.layout.tiles.len()),
            });
            widgets::section(ui, lang.t("Anlegen als", "Create as"));
            widgets::radio(ui, &mut d.target, ImportTarget::Document, lang.t("Wie in der Datei", "As in the file"));
            widgets::radio(ui, &mut d.target, ImportTarget::Global, lang.t("Globales Dashboard (alle Projekte)", "Global dashboard (all projects)"));
            if let Some((id, key)) = d.project.clone() {
                widgets::radio(ui, &mut d.target, ImportTarget::Project(id), format!("{} {key}", lang.t("Dashboard des Projekts", "Dashboard of project")));
            }
            hint(ui, lang.t("Der Import legt immer ein neues Dashboard an (nie Standard) und ändert nichts Bestehendes. Kacheln für unbekannte Projekte werden gemeldet.", "The import always creates a new dashboard (never the default) and changes nothing existing. Tiles for unknown projects are reported."));
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if widgets::primary(ui, lang.t("Importieren", "Import")).clicked() {
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
            match import_dashboard(cx.conn, &d.doc, d.target) {
                Ok(done) => {
                    self.dash_select(done.id);
                    let mut text = lang.t("Dashboard importiert.", "Dashboard imported.").to_string();
                    if done.report.renamed {
                        text.push(' ');
                        text.push_str(lang.t("Der Name war vergeben und wurde ergänzt.", "The name was taken and has been extended."));
                    }
                    if let Some(m) = done.report.message(lang) {
                        text.push(' ');
                        text.push_str(&m);
                    }
                    self.dash.notify(text, done.report.message(lang).is_some());
                    return None;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        (!close).then_some(d)
    }
}

/// Vorschau eines Imports: Standardziel ist „wie in der Datei“, wenn das geht (global oder bekanntes Projekt), sonst dieses Projekt,
/// sonst global.
fn new_import(cx: &Cx, proj: &Project, file: String, doc: DashboardDoc) -> ImportDlg {
    let project = (!proj.system).then(|| (proj.id, proj.key.clone()));
    let known = match (&doc.scope, &doc.project) {
        (DashboardScope::Global, _) => true,
        (DashboardScope::Project, Some(k)) => project_by_key(cx.conn, k).map(|p| !p.system).unwrap_or(false),
        (DashboardScope::Project, None) => false,
    };
    let target = if known {
        ImportTarget::Document
    } else if let Some((id, _)) = &project {
        ImportTarget::Project(*id)
    } else {
        ImportTarget::Global
    };
    ImportDlg { file, doc, target, project, err: None }
}

/// Auswahl eines Projekts (oder „keins“ mit eigenem Text).
fn project_combo(ui: &mut Ui, salt: &str, cur: &mut Option<i64>, projects: &[(i64, String, String)], none_label: &str) {
    let text = cur.and_then(|id| projects.iter().find(|p| p.0 == id)).map(|p| format!("{} · {}", p.1, p.2)).unwrap_or_else(|| none_label.to_string());
    egui::ComboBox::from_id_salt(salt).selected_text(text).width(300.0).show_ui(ui, |ui| {
        if ui.selectable_label(cur.is_none(), none_label).clicked() {
            *cur = None;
        }
        for (id, key, name) in projects {
            if ui.selectable_label(*cur == Some(*id), format!("{key} · {name}")).clicked() {
                *cur = Some(*id);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::planning::projects::{create_project, get_project, NewProject};
    use crate::planning::views::dashboards::{create_dashboard, DashboardDoc};
    use crate::planning::views::testutil::{db, project};

    fn cx_for<'a>(db: &'a Db, lang: Lang) -> Cx<'a> {
        Cx { conn: &db.conn, db, lang }
    }

    fn mem() -> Db {
        Db { conn: db(), path: std::path::PathBuf::new() }
    }

    fn dlg(d: &Db, project_id: Option<i64>, draft: TileDraft) -> TileDlg {
        let dash = Dashboard { id: 1, name: "D".into(), scope: if project_id.is_some() { DashboardScope::Project } else { DashboardScope::Global }, project_id, position: 0, is_default: false, layout: Default::default() };
        let Dlg::Tile(mut t) = Dlg::tile(&cx_for(d, Lang::En), &dash, None, draft) else { panic!() };
        t.recheck(&cx_for(d, Lang::En));
        *t
    }

    #[test]
    fn query_check_accepts_valid_and_reports_invalid_in_both_languages() {
        let d = mem();
        let p = project(&d.conn, "AA", true);
        for jql in ["", "ORDER BY updated DESC", r#"type = "Dev task" AND status = Done"#] {
            assert_eq!(check_jql(&cx_for(&d, Lang::En), Some(p), jql), None, "{jql}");
        }
        let en = check_jql(&cx_for(&d, Lang::En), Some(p), "type = ").unwrap();
        let de = check_jql(&cx_for(&d, Lang::De), Some(p), "type = ").unwrap();
        assert!(!en.is_empty() && !de.is_empty() && en != de);
        assert!(check_jql(&cx_for(&d, Lang::En), None, "bogus = 1").is_some());
        assert!(check_jql(&cx_for(&d, Lang::En), Some(p), &"x".repeat(5000)).is_some());
    }

    #[test]
    fn tile_dialog_blocks_invalid_queries_and_projectless_sprint_tiles() {
        let d = mem();
        let p = project(&d.conn, "AA", true);
        let mut draft = TileDraft::new(KindSel::Search);
        draft.jql = "type = ".into();
        let mut t = dlg(&d, Some(p), draft);
        assert!(tile_problem(&t, Lang::En).is_some());
        t.draft.jql = "type = Defect".into();
        t.recheck(&cx_for(&d, Lang::En));
        assert_eq!(tile_problem(&t, Lang::En), None, "nach der Korrektur gültig");
        // Sprint-Kachel: im Projekt-Dashboard reicht „das Projekt des Dashboards“, im globalen muss eins gewählt sein
        let t = dlg(&d, Some(p), TileDraft::new(KindSel::Sprint));
        assert_eq!(tile_problem(&t, Lang::En), None);
        let mut t = dlg(&d, None, TileDraft::new(KindSel::Sprint));
        let msg = tile_problem(&t, Lang::En).unwrap();
        assert_ne!(msg, tile_problem(&t, Lang::De).unwrap(), "zweisprachig");
        t.draft.project = Some(p);
        assert_eq!(tile_problem(&t, Lang::En), None);
        // eine kaputte Abfrage einer Art ohne Abfrage stört nicht
        let mut t = dlg(&d, None, TileDraft::new(KindSel::Favorites));
        t.draft.jql = "((".into();
        t.recheck(&cx_for(&d, Lang::En));
        assert_eq!(tile_problem(&t, Lang::En), None);
    }

    #[test]
    fn tile_dialog_offers_saved_filters_of_the_scope() {
        let d = mem();
        let p = project(&d.conn, "AA", true);
        use crate::planning::views::filters::{create_filter, NewFilter};
        create_filter(&d.conn, &NewFilter { project_id: Some(p), name: "Mine".into(), jql: "type = Defect".into() }).unwrap();
        create_filter(&d.conn, &NewFilter { project_id: None, name: "Everywhere".into(), jql: "flagged = true".into() }).unwrap();
        let t = dlg(&d, Some(p), TileDraft::new(KindSel::Search));
        assert_eq!(t.filters.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["Mine", "Everywhere"]);
        let t = dlg(&d, None, TileDraft::new(KindSel::Search));
        assert_eq!(t.filters.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["Everywhere"], "ein globales Dashboard bietet nur globale Filter an");
        assert_eq!(t.projects.len(), 1, "ohne das System-Projekt");
    }

    #[test]
    fn import_target_defaults_follow_the_document() {
        let d = mem();
        let a = project(&d.conn, "AA", false);
        let cx = cx_for(&d, Lang::En);
        let proj_a = get_project(&d.conn, a).unwrap();
        let mk = |scope: DashboardScope, project: Option<&str>| DashboardDoc {
            format: crate::planning::views::dashboards::FORMAT.into(),
            version: 1,
            name: "X".into(),
            scope,
            project: project.map(String::from),
            layout: Default::default(),
        };
        assert_eq!(new_import(&cx, &proj_a, "f".into(), mk(DashboardScope::Global, None)).target, ImportTarget::Document);
        assert_eq!(new_import(&cx, &proj_a, "f".into(), mk(DashboardScope::Project, Some("AA"))).target, ImportTarget::Document);
        assert_eq!(new_import(&cx, &proj_a, "f".into(), mk(DashboardScope::Project, Some("ZZ"))).target, ImportTarget::Project(a), "unbekanntes Projekt: in dieses Projekt");
        let system = list_projects(&d.conn).unwrap().into_iter().find(|p| p.system).unwrap();
        let i = new_import(&cx, &system, "f".into(), mk(DashboardScope::Project, Some("ZZ")));
        assert_eq!((i.target, i.project), (ImportTarget::Global, None), "im System-Projekt bleibt nur global");
    }

    #[test]
    fn export_import_round_trip_through_the_data_layer() {
        let d = mem();
        let a = project(&d.conn, "AA", false);
        let b = create_project(&d.conn, &NewProject::new("BB", "B", "#112233")).unwrap();
        let id = create_dashboard(&d.conn, &NewDashboard { name: "Ü".into(), project_id: Some(a), layout: Some(logic::overview_layout(Lang::En, true, false)) }).unwrap();
        let doc = export_dashboard(&d.conn, id).unwrap();
        let text = doc.to_json();
        let back = DashboardDoc::from_json(&text).unwrap();
        let cx = cx_for(&d, Lang::En);
        let proj_b = get_project(&d.conn, b).unwrap();
        let dlg = new_import(&cx, &proj_b, "ue.json".into(), back);
        let done = import_dashboard(&d.conn, &dlg.doc, dlg.target).unwrap();
        assert_ne!(done.id, id);
        let imported = crate::planning::views::dashboards::get_dashboard(&d.conn, done.id).unwrap();
        assert_eq!(imported.project_id, Some(a), "wie in der Datei");
        assert_eq!(imported.name, "Ü (2)", "Name war vergeben");
        assert!(done.report.renamed);
        assert!(!imported.is_default);
        assert_eq!(imported.layout.tiles.len(), crate::planning::views::dashboards::get_dashboard(&d.conn, id).unwrap().layout.tiles.len());
    }
}
