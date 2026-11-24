//! Oberfläche „Zugriff für Agenten“ (#386): Tabelle in Einstellungen → Agenten (alle Projekte, „Allgemein“, Ticket-Projekte,
//! Schalter), Dialog „Zugriff für Agenten“ (Kontextmenü eines Projekts, Hinweis „Freigeben …“) und der Papierkorb.

use super::*;
use crate::access::{AgentAccess, AreaLevel};

/// Auswahl der Stufe eines Bereichs (Aufklappliste) und Schalter „Für alle“. Gibt `true` zurück, wenn sich etwas geändert hat.
pub(super) fn access_row(ui: &mut Ui, id: impl std::hash::Hash, acc: &mut AgentAccess, lang: Lang) -> bool {
    let before = *acc;
    egui::ComboBox::from_id_salt(("agent_access", id)).selected_text(acc.level.label(lang)).width(110.0).show_ui(ui, |ui| {
        for l in AreaLevel::ALL {
            ui.selectable_value(&mut acc.level, l, l.label(lang));
        }
    });
    style::toggle(ui, &mut acc.for_all).on_hover_text(lang.t(
        "Für alle: auch Agenten anderer Projekte und externe Agenten bekommen diese Stufe",
        "For all: agents of other projects and external agents get this level too",
    ));
    ui.label(RichText::new(lang.t("Für alle", "For all")).small());
    *acc != before
}

/// Hinweis unter der Tabelle: was die Stufen bedeuten.
fn rules_hint(ui: &mut Ui, lang: Lang) {
    ui.label(
        RichText::new(lang.t(
            "Ein Agent in einer Paneldeck-Sitzung arbeitet im Projekt dieser Sitzung; andere Projekte erreicht er nur mit „Für alle“, externe Agenten nur „Für alle“-Projekte. Die globale Stufe oben ist die Obergrenze („Wie global“ übernimmt sie). „Allgemein“ ist alles ohne Projekt (Karten ohne Projekt, Wecker, Termine ohne Projekt, Editor …). Neue Projekte beginnen gesperrt.",
            "An agent in a Paneldeck session works in that session's project; it reaches other projects only with “For all”, external agents only “For all” projects. The global level above is the upper bound (“As global” takes it over). “General” is everything without a project (cards without a project, alarms, events without a project, editor …). New projects start locked.",
        ))
        .small()
        .color(style::dim(ui)),
    );
}

impl App {
    /// Teil von Einstellungen → Agenten: Schalter und Tabelle „Zugriff je Projekt“.
    pub(super) fn access_settings_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        let mut dirty_settings = false;
        ui.add_space(S1);
        ui.horizontal(|ui| {
            dirty_settings |= style::toggle(ui, &mut self.settings.agent_create_projects).changed();
            ui.label(RichText::new(lang.t("Agenten dürfen Projekte anlegen (nur Ticket-Projekte, verknüpft mit ihrem Projekt)", "Agents may create projects (ticket projects only, linked to their project)")).small());
        });
        ui.horizontal(|ui| {
            dirty_settings |= style::toggle(ui, &mut self.settings.agent_delete).changed();
            ui.label(RichText::new(lang.t("Agenten dürfen Tickets und eigene Kommentare löschen (in den Papierkorb)", "Agents may delete tickets and their own comments (to the trash)")).small());
        });
        ui.add_space(S2);
        let top = ui.cursor().min.y;
        ui.label(RichText::new(lang.t("Zugriff je Projekt", "Access per project")).strong());
        rules_hint(ui, lang);
        let mut dirty_state = false;
        egui::Grid::new("agent_access_grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            let mut projects = std::mem::take(&mut self.projects);
            for p in projects.iter_mut() {
                ui.label(&p.name);
                ui.horizontal(|ui| dirty_state |= access_row(ui, ("p", p.id), &mut p.agent_access, lang));
                ui.end_row();
            }
            self.projects = projects;
            ui.label(RichText::new(lang.t("Allgemein (ohne Projekt)", "General (no project)")).italics());
            ui.horizontal(|ui| dirty_settings |= access_row(ui, "general", &mut self.settings.agent_general, lang));
            ui.end_row();
        });
        super::tutorial::spot_since(ui, Spot::AgentAccess, top);
        // Ticket-Projekte (Datenbank): eigene Stufe oder verknüpft mit einem Seitenleisten-Projekt.
        if let Some(db) = self.db.as_ref() {
            let list = crate::planning::projects::list_projects(&db.conn).unwrap_or_default();
            if !list.is_empty() {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Ticket-Projekte", "Ticket projects")).strong());
                let names: Vec<(u64, String)> = self.projects.iter().map(|p| (p.id, p.name.clone())).collect();
                egui::Grid::new("agent_access_tickets").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    for p in list {
                        let Ok((mut acc, mut link)) = crate::planning::access::project_access(&db.conn, p.id) else { continue };
                        ui.label(if p.system { format!("{} ({})", p.key, lang.t("Todo-Liste", "to-do list")) } else { format!("{} – {}", p.key, p.display(lang)) });
                        let changed = ui.horizontal(|ui| ticket_access_row(ui, p.id, &mut acc, &mut link, &names, lang)).inner;
                        if changed {
                            let _ = crate::planning::access::set_project_access(&db.conn, p.id, acc, link);
                        }
                        ui.end_row();
                    }
                });
            }
        }
        if dirty_state {
            self.save_state();
        }
        if dirty_settings {
            // Sicherheitseinstellung: sofort sichern.
            state::save_json(&self.dir.join("settings.json"), &self.settings);
        }
        if dirty_state || dirty_settings {
            self.mcp_reach_dirty();
        }
    }

    /// Dialog „Zugriff für Agenten“ eines Bereichs (Kontextmenü eines Projekts, Hinweis „Freigeben …“).
    pub(super) fn access_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(area) = self.mcp.access_dlg else { return };
        let lang = self.settings.language;
        let mut open = true;
        let mut close = false;
        let title = format!("{} – {}", lang.t("Zugriff für Agenten", "Agent access"), self.area_name(area));
        egui::Window::new(title).id(egui::Id::new("agent_access_dlg")).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            rules_hint(ui, lang);
            ui.add_space(S1);
            match area {
                crate::access::Area::Project(pid) => {
                    if let Some(i) = self.projects.iter().position(|p| p.id == pid) {
                        let mut acc = self.projects[i].agent_access;
                        if ui.horizontal(|ui| access_row(ui, ("dlg", pid), &mut acc, lang)).inner {
                            self.projects[i].agent_access = acc;
                            self.save_state();
                            self.mcp_reach_dirty();
                        }
                    }
                }
                crate::access::Area::General => {
                    let mut acc = self.settings.agent_general;
                    if ui.horizontal(|ui| access_row(ui, "dlg_general", &mut acc, lang)).inner {
                        self.settings.agent_general = acc;
                        state::save_json(&self.dir.join("settings.json"), &self.settings);
                        self.mcp_reach_dirty();
                    }
                }
                crate::access::Area::Tickets(tid) => {
                    let names: Vec<(u64, String)> = self.projects.iter().map(|p| (p.id, p.name.clone())).collect();
                    if let Some(db) = self.db.as_ref() {
                        if let Ok((mut acc, mut link)) = crate::planning::access::project_access(&db.conn, tid) {
                            if ui.horizontal(|ui| ticket_access_row(ui, tid, &mut acc, &mut link, &names, lang)).inner {
                                let _ = crate::planning::access::set_project_access(&db.conn, tid, acc, link);
                                self.mcp_reach_dirty();
                            }
                        }
                    }
                }
            }
            ui.add_space(S1);
            if ui.button(lang.t("Schließen", "Close")).clicked() {
                close = true;
            }
        });
        if !open || close {
            self.mcp.access_dlg = None;
        }
    }

    /// Klick auf „Freigeben …“ eines Hinweises.
    pub(super) fn open_access_dialog(&mut self, area: crate::access::Area) {
        self.mcp.access_dlg = Some(area);
    }

    /// Höchste Stufe beim nächsten Bild neu veröffentlichen.
    pub(super) fn mcp_reach_dirty(&mut self) {
        self.mcp.reach_at = None;
    }

    /// Ein Ticket-Projekt im Papierkorb ansehen: Liste mit „Wiederherstellen“, „Endgültig löschen“, „Papierkorb leeren“.
    pub(super) fn trash_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        let Some(db) = self.db.as_ref() else { return };
        let list = crate::planning::access::trash(&db.conn).unwrap_or_default();
        ui.label(RichText::new(lang.t("Papierkorb", "Trash")).strong());
        ui.label(
            RichText::new(lang.t(
                "Hier landen Tickets und Kommentare, die Agenten gelöscht haben. Nur du kannst sie wiederherstellen oder endgültig löschen.",
                "Tickets and comments deleted by agents end up here. Only you can restore them or delete them permanently.",
            ))
            .small()
            .color(style::dim(ui)),
        );
        if list.is_empty() {
            ui.label(RichText::new(lang.t("Der Papierkorb ist leer.", "The trash is empty.")).small().italics());
            return;
        }
        let mut act: Option<(bool, crate::planning::access::TrashEntry)> = None;
        let mut empty = false;
        egui::Grid::new("trash_grid").num_columns(4).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
            for e in &list {
                let what = match e.comment {
                    Some(_) => format!("{} – {}", e.key, lang.t("Kommentar", "comment")),
                    None => e.key.clone(),
                };
                ui.label(RichText::new(what).monospace());
                ui.label(crate::mcp::short(&e.text, 60));
                ui.label(RichText::new(format!("{} · {}", crate::planning::history::Source::from_label_text(&e.by).label(lang), e.at)).small().color(style::dim(ui)));
                ui.horizontal(|ui| {
                    if ui.small_button(lang.t("Wiederherstellen", "Restore")).clicked() {
                        act = Some((true, e.clone()));
                    }
                    if ui.small_button(lang.t("Endgültig löschen", "Delete permanently")).clicked() {
                        act = Some((false, e.clone()));
                    }
                });
                ui.end_row();
            }
        });
        if ui.button(lang.t("Papierkorb leeren", "Empty trash")).clicked() {
            empty = true;
        }
        let src = crate::planning::history::Source::ui();
        let r = match (act, empty) {
            (Some((true, e)), _) => crate::planning::access::restore(&db.conn, &e, &src).map(|_| ()),
            (Some((false, e)), _) => crate::planning::access::purge(&db.conn, &e, &src),
            (None, true) => crate::planning::access::empty_trash(&db.conn, &src).map(|_| ()),
            _ => return,
        };
        match r {
            Ok(()) => {
                self.plan.invalidate();
                self.tasks.invalidate();
                self.wins.data_gen += 1;
            }
            Err(e) => self.push_toast(e.message(lang), super::toasts::ToastKind::Error, None),
        }
    }
}

/// Zeile eines Ticket-Projekts: verknüpftes Seitenleisten-Projekt (dann gilt dessen Zugriff) oder eigene Stufe.
pub(super) fn ticket_access_row(ui: &mut Ui, id: i64, acc: &mut AgentAccess, link: &mut Option<u64>, sidebar: &[(u64, String)], lang: Lang) -> bool {
    let before = (*acc, *link);
    let shown = link.and_then(|l| sidebar.iter().find(|(i, _)| *i == l)).map(|(_, n)| n.clone()).unwrap_or_else(|| lang.t("Keine Verknüpfung", "No link").to_string());
    egui::ComboBox::from_id_salt(("ticket_link", id)).selected_text(shown).width(140.0).show_ui(ui, |ui| {
        ui.selectable_value(link, None, lang.t("Keine Verknüpfung", "No link"));
        for (pid, name) in sidebar {
            ui.selectable_value(link, Some(*pid), name);
        }
    })
    .response
    .on_hover_text(lang.t("Verknüpft: folgt dem Zugriff dieses Projekts der Seitenleiste", "Linked: follows the access of this sidebar project"));
    if link.is_none() {
        access_row(ui, ("t", id), acc, lang);
    } else {
        ui.label(RichText::new(lang.t("folgt dem Projekt", "follows the project")).small().color(style::dim(ui)));
    }
    (*acc, *link) != before
}
