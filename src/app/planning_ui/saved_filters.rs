//! Gespeicherte Filter in der Abfrage-Leiste: das Dropdown „Gespeicherte Filter“, der Dialog „Filter speichern“ und die
//! Verwaltung (übernehmen, umbenennen, Abfrage ersetzen, löschen mit Bestätigung). Projekt-Filter und globale Filter
//! stehen getrennt. Die Datenschicht ist `crate::planning::views::filters`; hier ist nur die Oberfläche.

use super::query_logic::one_line;
use super::widgets::{self, hint};
use crate::planning::views::filters::{NewFilter, SavedFilter, UpdateFilter, create_filter, delete_filter, list_filters, list_filters_for_project, update_filter};
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Color32, FontId, Id, Key, RichText, Ui, text::LayoutJob};
use rusqlite::Connection;

/// Was die Filter-Oberfläche der Leiste meldet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterEvent {
    /// Ein gespeicherter Filter wurde gewählt: seine Abfrage ins Feld übernehmen (und ausführen).
    Load(String),
}

/// Dialog „Filter speichern“.
struct SaveDlg {
    name: String,
    /// `true` = globaler Filter, `false` = nur dieses Projekt
    global: bool,
    jql: String,
    err: Option<String>,
    focus: bool,
}

/// Dialog „Gespeicherte Filter verwalten“.
#[derive(Default)]
struct ManageDlg {
    /// Umbenennen: (Filter, neuer Name)
    rename: Option<(i64, String)>,
    /// Löschen wartet auf Bestätigung
    delete: Option<i64>,
    err: Option<String>,
}

/// Zustand der Filter-Oberfläche einer Abfrage-Leiste.
#[derive(Default)]
pub struct FilterUi {
    cache: Option<Vec<SavedFilter>>,
    save: Option<SaveDlg>,
    manage: Option<ManageDlg>,
}

impl FilterUi {
    /// Die Liste beim nächsten Zeichnen neu aus der Datenbank lesen.
    pub fn invalidate(&mut self) {
        self.cache = None;
    }

    fn list(&mut self, conn: &Connection, project_id: Option<i64>) -> Vec<SavedFilter> {
        if self.cache.is_none() {
            let l = match project_id {
                Some(p) => list_filters_for_project(conn, p),
                None => list_filters(conn, None),
            };
            self.cache = Some(l.unwrap_or_default());
        }
        self.cache.clone().unwrap_or_default()
    }

    /// „Filter speichern“ öffnen (`jql` = die aktuelle Abfrage).
    pub fn open_save(&mut self, jql: &str, project_id: Option<i64>) {
        self.save = Some(SaveDlg { name: String::new(), global: project_id.is_none(), jql: jql.trim().to_string(), err: None, focus: true });
    }

    /// Das Dropdown „Gespeicherte Filter“. Gibt das Ereignis zurück, wenn ein Filter gewählt wurde.
    pub fn menu(&mut self, ui: &mut Ui, conn: &Connection, project_id: Option<i64>, lang: Lang) -> Option<FilterEvent> {
        let list = self.list(conn, project_id);
        let resp = ui.add(egui::Button::new(format!("{} ▾", lang.t("Gespeicherte Filter", "Saved filters")))).on_hover_text(lang.t(
            "Eine gespeicherte Abfrage übernehmen oder die gespeicherten Filter verwalten",
            "Load a saved query or manage the saved filters",
        ));
        let mut ev = None;
        let mut manage = false;
        egui::Popup::from_toggle_button_response(&resp).show(|ui| {
            ui.set_min_width(260.0);
            let mine: Vec<&SavedFilter> = list.iter().filter(|f| f.project_id.is_some()).collect();
            let global: Vec<&SavedFilter> = list.iter().filter(|f| f.project_id.is_none()).collect();
            if list.is_empty() {
                ui.label(RichText::new(lang.t("Noch keine gespeicherten Filter.", "No saved filters yet.")).color(style::dim(ui)));
            }
            let mut group = |ui: &mut Ui, title: &str, items: &[&SavedFilter]| {
                if items.is_empty() {
                    return;
                }
                ui.label(RichText::new(title).small().color(style::dim(ui)));
                for f in items {
                    if ui.add(egui::Button::new(entry_text(ui, f)).frame(false)).on_hover_text(&f.jql).clicked() {
                        ev = Some(FilterEvent::Load(f.jql.clone()));
                    }
                }
            };
            group(ui, lang.t("Dieses Projekt", "This project"), &mine);
            group(ui, lang.t("Alle Projekte", "All projects"), &global);
            ui.separator();
            if ui.add(egui::Button::new(format!("{} …", lang.t("Filter verwalten", "Manage filters"))).frame(false)).clicked() {
                manage = true;
            }
        });
        if manage {
            self.manage = Some(ManageDlg::default());
        }
        ev
    }

    /// Die Dialoge zeichnen (nach der Leiste aufrufen). `current` ist die Abfrage im Feld, `can_replace`: sie ist gültig
    /// und nicht leer (dann darf eine gespeicherte Abfrage damit ersetzt werden).
    pub fn dialogs(&mut self, ctx: &egui::Context, conn: &Connection, project_id: Option<i64>, project_key: &str, lang: Lang, salt: Id, current: &str, can_replace: bool) -> Option<FilterEvent> {
        self.save_dialog(ctx, conn, project_id, project_key, lang, salt);
        self.manage_dialog(ctx, conn, project_id, lang, salt, current, can_replace)
    }

    fn save_dialog(&mut self, ctx: &egui::Context, conn: &Connection, project_id: Option<i64>, project_key: &str, lang: Lang, salt: Id) {
        let Some(mut d) = self.save.take() else { return };
        let mut save = false;
        let mut close = false;
        let m = crate::style::modal(salt.with("save_filter")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 440.0));
            widgets::dialog_title(ui, lang.t("Filter speichern", "Save filter"));
            hint(ui, lang.t("Die Abfrage:", "The query:"));
            query_box(ui, &d.jql);
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Name", "Name")).color(style::dim(ui)));
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)).char_limit(crate::planning::common::MAX_NAME_CHARS));
            if d.focus {
                r.request_focus();
                d.focus = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                save = true;
            }
            if project_id.is_some() {
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    widgets::radio(ui, &mut d.global, false, format!("{} ({project_key})", lang.t("Nur dieses Projekt", "This project only")));
                    widgets::radio(ui, &mut d.global, true, lang.t("Alle Projekte", "All projects"));
                });
            } else {
                d.global = true;
            }
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, !d.name.trim().is_empty(), lang.t("Speichern", "Save")).clicked() {
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
        if save && !d.name.trim().is_empty() {
            let scope = if d.global { None } else { project_id };
            match create_filter(conn, &NewFilter { project_id: scope, name: d.name.clone(), jql: d.jql.clone() }) {
                Ok(_) => {
                    self.invalidate();
                    return;
                }
                Err(e) => d.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.save = Some(d);
        }
    }

    fn manage_dialog(&mut self, ctx: &egui::Context, conn: &Connection, project_id: Option<i64>, lang: Lang, salt: Id, current: &str, can_replace: bool) -> Option<FilterEvent> {
        let mut d = self.manage.take()?;
        let list = self.list(conn, project_id);
        let mut close = false;
        let mut ev = None;
        let mut act: Option<Act> = None;
        let m = crate::style::modal(salt.with("manage_filters")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 640.0));
            widgets::dialog_title(ui, lang.t("Gespeicherte Filter", "Saved filters"));
            if list.is_empty() {
                ui.label(RichText::new(lang.t(
                    "Noch keine gespeicherten Filter. Schreibe eine Abfrage und wähle „Filter speichern“.",
                    "No saved filters yet. Write a query and choose “Save filter”.",
                )).color(style::dim(ui)));
            }
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 260.0).clamp(120.0, 420.0)).auto_shrink([false, true]).show(ui, |ui| {
                for f in &list {
                    widgets::card(ui, |ui| {
                        ui.horizontal(|ui| {
                            match &mut d.rename {
                                Some((id, name)) if *id == f.id => {
                                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(240.0).margin(egui::Margin::symmetric(8, 4)));
                                    r.request_focus();
                                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                                    if enter || ui.button(lang.t("OK", "OK")).clicked() {
                                        act = Some(Act::Rename(f.id, name.clone()));
                                    }
                                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape))) {
                                        act = Some(Act::CancelRename);
                                    }
                                }
                                _ => {
                                    ui.label(RichText::new(&f.name).strong());
                                    let (txt, tip) = if f.project_id.is_some() {
                                        (lang.t("Projekt", "Project"), lang.t("Gilt nur in diesem Projekt", "Applies to this project only"))
                                    } else {
                                        (lang.t("Global", "Global"), lang.t("Gilt in allen Projekten", "Applies to all projects"))
                                    };
                                    ui.label(RichText::new(txt).small().color(style::accent())).on_hover_text(tip);
                                }
                            }
                        });
                        query_box(ui, &f.jql);
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(lang.t("Übernehmen", "Use")).on_hover_text(lang.t("Abfrage ins Suchfeld übernehmen", "Put the query into the search field")).clicked() {
                                act = Some(Act::Use(f.jql.clone()));
                            }
                            if ui.button(lang.t("Umbenennen", "Rename")).clicked() {
                                act = Some(Act::StartRename(f.id, f.name.clone()));
                            }
                            if ui
                                .add_enabled(can_replace && current.trim() != f.jql, egui::Button::new(lang.t("Abfrage ersetzen", "Replace query")))
                                .on_hover_text(lang.t("Die Abfrage dieses Filters durch die aktuelle aus dem Suchfeld ersetzen", "Replace this filter's query with the one in the search field"))
                                .clicked()
                            {
                                act = Some(Act::Replace(f.id, current.trim().to_string()));
                            }
                            if widgets::small_danger(ui, lang.t("Löschen …", "Delete …")).clicked() {
                                act = Some(Act::AskDelete(f.id));
                            }
                        });
                    });
                    ui.add_space(S1);
                }
            });
            if let Some(e) = &d.err {
                ui.add_space(S1);
                widgets::error(ui, e);
            }
            ui.add_space(S3);
            if widgets::secondary(ui, lang.t("Schließen", "Close")).clicked() {
                close = true;
            }
        });
        if m.should_close() && d.delete.is_none() {
            close = true;
        }
        // Bestätigung vor dem Löschen
        if let Some(id) = d.delete {
            if let Some(f) = list.iter().find(|f| f.id == id) {
                let (mut yes, mut no) = (false, false);
                let m = crate::style::modal(salt.with("delete_filter")).show(ctx, |ui| {
                    ui.set_width(widgets::modal_width(ctx, 420.0));
                    widgets::dialog_title(ui, lang.t("Filter löschen?", "Delete filter?"));
                    ui.label(match lang {
                        Lang::De => format!("Der Filter {} wird endgültig gelöscht.", lang.q(&f.name)),
                        Lang::En => format!("The filter {} will be deleted for good.", lang.q(&f.name)),
                    });
                    ui.add_space(S1);
                    hint(ui, lang.t(
                        "Boards, die diesen Filter als Quelle nutzen, verlieren die Quelle. Gespeicherte Abfragen anderswo bleiben unberührt.",
                        "Boards that use this filter as their source lose that source. Queries saved elsewhere are not touched.",
                    ));
                    ui.add_space(S3);
                    ui.horizontal(|ui| {
                        if widgets::danger(ui, lang.t("Löschen", "Delete"), 0).clicked() {
                            yes = true;
                        }
                        if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                            no = true;
                        }
                    });
                });
                if m.should_close() {
                    no = true;
                }
                if yes {
                    match delete_filter(conn, id) {
                        Ok(()) => {
                            d.delete = None;
                            d.err = None;
                            self.invalidate();
                        }
                        Err(e) => {
                            d.delete = None;
                            d.err = Some(e.message(lang));
                        }
                    }
                } else if no {
                    d.delete = None;
                }
            } else {
                d.delete = None;
            }
        }
        match act {
            Some(Act::Use(jql)) => {
                ev = Some(FilterEvent::Load(jql));
                close = true;
            }
            Some(Act::StartRename(id, name)) => d.rename = Some((id, name)),
            Some(Act::CancelRename) => d.rename = None,
            Some(Act::Rename(id, name)) => match update_filter(conn, id, &UpdateFilter { name: Some(name), jql: None }) {
                Ok(()) => {
                    d.rename = None;
                    d.err = None;
                    self.invalidate();
                }
                Err(e) => d.err = Some(e.message(lang)),
            },
            Some(Act::Replace(id, jql)) => match update_filter(conn, id, &UpdateFilter { name: None, jql: Some(jql) }) {
                Ok(()) => {
                    d.err = None;
                    self.invalidate();
                }
                Err(e) => d.err = Some(e.message(lang)),
            },
            Some(Act::AskDelete(id)) => d.delete = Some(id),
            None => {}
        }
        if !close {
            self.manage = Some(d);
        }
        ev
    }
}

enum Act {
    Use(String),
    StartRename(i64, String),
    CancelRename,
    Rename(i64, String),
    Replace(i64, String),
    AskDelete(i64),
}

/// Name und gedämpfte Abfrage in einer Zeile (Dropdown-Eintrag).
fn entry_text(ui: &Ui, f: &SavedFilter) -> LayoutJob {
    let mut job = LayoutJob::default();
    let body = egui::TextStyle::Body.resolve(ui.style());
    job.append(&f.name, 0.0, egui::TextFormat { font_id: body, color: ui.visuals().text_color(), ..Default::default() });
    let preview = one_line(&f.jql, 40);
    if !preview.is_empty() {
        job.append(&format!("   {preview}"), 0.0, egui::TextFormat { font_id: FontId::monospace(11.0), color: style::dim(ui), ..Default::default() });
    }
    job
}

/// Abfrage als abgesetzter, umbrechender Code (Dialoge).
fn query_box(ui: &mut Ui, jql: &str) {
    egui::Frame::NONE.fill(Color32::from_black_alpha(60)).corner_radius(6.0).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let text = if jql.trim().is_empty() { "—".to_string() } else { jql.to_string() };
        ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).wrap());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::views::filters::get_filter;
    use crate::planning::views::testutil::{db, project};
    use eframe::egui::{Event, Modifiers, Pos2, RawInput, Rect, Vec2};

    struct Run {
        ctx: egui::Context,
        t: f64,
    }

    impl Run {
        fn new() -> Run {
            Run { ctx: egui::Context::default(), t: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>, add: impl FnOnce(&egui::Context)) {
            self.t += 0.05;
            let modifiers = events.iter().find_map(|e| if let Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
            let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))), events, focused: true, time: Some(self.t), modifiers, ..Default::default() };
            let mut add = Some(add);
            let _ = self.ctx.run(raw, |ctx| {
                if let Some(f) = add.take() {
                    f(ctx);
                }
            });
        }
    }

    fn key(k: Key) -> Vec<Event> {
        vec![Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]
    }

    #[test]
    fn the_save_dialog_stores_a_project_filter_with_enter() {
        let c = db();
        let p = project(&c, "AA", true);
        let mut ui = FilterUi::default();
        ui.open_save("  type = Epic ", Some(p));
        let mut r = Run::new();
        let draw = |r: &mut Run, ui: &mut FilterUi, ev: Vec<Event>| r.frame(ev, |ctx| {
            ui.dialogs(ctx, &c, Some(p), "AA", Lang::En, Id::new("t"), "type = Epic", true);
        });
        draw(&mut r, &mut ui, vec![]);
        draw(&mut r, &mut ui, vec![]);
        draw(&mut r, &mut ui, vec![Event::Text("Open epics".into())]);
        draw(&mut r, &mut ui, key(Key::Enter));
        draw(&mut r, &mut ui, vec![]);
        let l = list_filters(&c, Some(p)).unwrap();
        assert_eq!(l.len(), 1, "Enter im Namensfeld speichert");
        assert_eq!((l[0].name.as_str(), l[0].jql.as_str()), ("Open epics", "type = Epic"));
        assert!(ui.save.is_none(), "der Dialog schließt");
        assert!(list_filters(&c, None).unwrap().is_empty());
    }

    #[test]
    fn a_taken_name_is_reported_in_the_dialog() {
        let c = db();
        let p = project(&c, "AA", true);
        create_filter(&c, &NewFilter { project_id: Some(p), name: "Dup".into(), jql: "".into() }).unwrap();
        let mut ui = FilterUi::default();
        ui.open_save("type = Epic", Some(p));
        ui.save.as_mut().unwrap().name = "dup".into();
        let mut r = Run::new();
        let draw = |r: &mut Run, ui: &mut FilterUi, ev: Vec<Event>| r.frame(ev, |ctx| {
            ui.dialogs(ctx, &c, Some(p), "AA", Lang::De, Id::new("t"), "type = Epic", true);
        });
        draw(&mut r, &mut ui, vec![]);
        draw(&mut r, &mut ui, key(Key::Enter));
        draw(&mut r, &mut ui, vec![]);
        let d = ui.save.as_ref().expect("Dialog bleibt offen");
        assert!(d.err.as_deref().is_some_and(|e| !e.is_empty()), "Fehler wird gezeigt");
        assert_eq!(list_filters(&c, Some(p)).unwrap().len(), 1);
    }

    #[test]
    fn global_scope_without_a_project() {
        let c = db();
        let mut ui = FilterUi::default();
        ui.open_save("x = 1", None);
        assert!(ui.save.as_ref().unwrap().global);
        ui.save.as_mut().unwrap().name = "G".into();
        let mut r = Run::new();
        r.frame(vec![], |ctx| {
            ui.dialogs(ctx, &c, None, "", Lang::En, Id::new("t"), "x = 1", true);
        });
        r.frame(key(Key::Enter), |ctx| {
            ui.dialogs(ctx, &c, None, "", Lang::En, Id::new("t"), "x = 1", true);
        });
        assert_eq!(list_filters(&c, None).unwrap().len(), 1);
    }

    #[test]
    fn manage_dialog_draws_and_lists_project_and_global_filters() {
        let c = db();
        let p = project(&c, "AA", true);
        let a = create_filter(&c, &NewFilter { project_id: Some(p), name: "Mine".into(), jql: "type = Epic".into() }).unwrap();
        let g = create_filter(&c, &NewFilter { project_id: None, name: "Everywhere".into(), jql: "IS favorite".into() }).unwrap();
        let mut ui = FilterUi::default();
        ui.manage = Some(ManageDlg::default());
        let mut r = Run::new();
        for lang in [Lang::En, Lang::De] {
            r.frame(vec![], |ctx| {
                ui.dialogs(ctx, &c, Some(p), "AA", lang, Id::new("t"), "x = 1", true);
            });
        }
        assert_eq!(ui.cache.as_ref().unwrap().iter().map(|f| f.id).collect::<Vec<_>>(), vec![a, g], "erst Projekt, dann global");
        // Umbenennen-Zeile und Lösch-Bestätigung zeichnen
        ui.manage = Some(ManageDlg { rename: Some((a, "Mine".into())), delete: Some(g), err: Some("boom".into()) });
        r.frame(vec![], |ctx| {
            ui.dialogs(ctx, &c, Some(p), "AA", Lang::En, Id::new("t"), "x = 1", true);
        });
        assert!(ui.manage.is_some());
        // das Löschen selbst läuft über die Datenschicht und leert den Zwischenspeicher
        delete_filter(&c, g).unwrap();
        ui.invalidate();
        assert!(get_filter(&c, g).is_err());
        let mut ui2 = FilterUi::default();
        assert_eq!(ui2.list(&c, Some(p)).len(), 1);
    }

    #[test]
    fn the_dropdown_menu_draws() {
        let c = db();
        let p = project(&c, "AA", true);
        create_filter(&c, &NewFilter { project_id: Some(p), name: "Mine".into(), jql: "type = Epic".into() }).unwrap();
        let mut ui = FilterUi::default();
        let mut r = Run::new();
        r.frame(vec![], |ctx| {
            egui::CentralPanel::default().show(ctx, |u| {
                assert!(ui.menu(u, &c, Some(p), Lang::En).is_none());
            });
        });
        assert_eq!(ui.cache.as_ref().map(Vec::len), Some(1));
    }
}
