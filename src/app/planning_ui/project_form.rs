//! Assistent „Neues Projekt“, Projekt-Einstellungen und das Löschen eines Projekts (mit Sicherheitstimer).

use super::helpers::{next_palette_color, suggest_key};
use super::widgets::{self, hint};
use super::{Cx, PlanUi, Tab};
use crate::planning::common::{clean_name, normalize_color, normalize_key, validate_key};
use crate::planning::projects::{create_project, delete_impact, delete_project, update_project, NewProject, Project, TagColorMode, UpdateProject};
use crate::planning::PlanError;
use crate::state::Lang;
use crate::style::{S1, S2, S3};
use eframe::egui::{self, Key, RichText, Ui};
use std::time::Instant;

/// Wartezeit vor dem endgültigen Löschen eines Projekts.
const CONFIRM_SECS: f32 = 3.0;

/// Eingaben von Assistent und Einstellungen.
pub(super) struct Form {
    name: String,
    key: String,
    /// Der Schlüssel wurde von Hand geändert (folgt nicht mehr dem Namen).
    key_touched: bool,
    color: String,
    description: String,
    work_dir: String,
    sprints: bool,
    versions: bool,
    tag_mode: TagColorMode,
    /// Nur im Assistenten: Standard-Typen und -Workflows mit anlegen.
    defaults: bool,
    /// Schlüssel der anderen Projekte (Großbuchstaben).
    taken: Vec<String>,
    /// Ursprünglicher Schlüssel (nur Einstellungen), für den Hinweis beim Ändern.
    original_key: Option<String>,
    focus_name: bool,
    err: Option<String>,
    /// Nur Einstellungen: Wartung (#308) – Zielstatus „Won't Do“ und Schwellen, samt der Status zur Auswahl.
    maint: Option<(crate::planning::maintenance::Settings, Vec<(i64, String)>)>,
}

impl Form {
    pub fn new_project(projects: &[Project]) -> Form {
        let used: Vec<String> = projects.iter().filter(|p| !p.system).map(|p| p.color.clone()).collect();
        Form {
            name: String::new(),
            key: String::new(),
            key_touched: false,
            color: next_palette_color(&used).to_string(),
            description: String::new(),
            work_dir: String::new(),
            sprints: false,
            versions: false,
            tag_mode: TagColorMode::Default,
            defaults: true,
            taken: projects.iter().map(|p| p.key.to_ascii_uppercase()).collect(),
            original_key: None,
            focus_name: true,
            err: None,
            maint: None,
        }
    }

    pub fn from_project(p: &Project, projects: &[Project]) -> Form {
        Form {
            name: p.name.clone(),
            key: p.key.clone(),
            key_touched: true,
            color: p.color.clone(),
            description: p.description.clone(),
            work_dir: p.work_dir.clone(),
            sprints: p.use_sprints,
            versions: p.use_versions,
            tag_mode: p.tag_color_mode,
            defaults: false,
            taken: projects.iter().filter(|o| o.id != p.id).map(|o| o.key.to_ascii_uppercase()).collect(),
            original_key: Some(p.key.clone()),
            focus_name: false,
            err: None,
            maint: None,
        }
    }

    /// Fehler je Eingabefeld (live): Name, Schlüssel, Farbe.
    fn name_problem(&self) -> Option<PlanError> {
        clean_name(&self.name).err()
    }

    fn key_problem(&self) -> Option<PlanError> {
        let k = normalize_key(&self.key);
        if let Err(e) = validate_key(&k) {
            return Some(e);
        }
        self.taken.contains(&k).then_some(PlanError::KeyTaken)
    }

    fn color_problem(&self) -> Option<PlanError> {
        normalize_color(&self.color).err()
    }

    /// Die gewählte Farbe, wenn sie gültig ist (für die Live-Vorschau der Akzentfarbe).
    pub fn color_parsed(&self) -> Option<eframe::egui::Color32> {
        super::helpers::parse_hex(&self.color)
    }

    pub fn is_valid(&self) -> bool {
        self.name_problem().is_none() && self.key_problem().is_none() && self.color_problem().is_none()
    }
}

/// Die gemeinsamen Eingabefelder. Gibt `true` zurück, wenn in einem Textfeld Enter gedrückt wurde.
fn form_ui(ui: &mut Ui, f: &mut Form, lang: Lang, new: bool) -> bool {
    let mut enter = false;
    let w = ui.available_width();
    // Name
    ui.label(RichText::new(lang.t("Name", "Name")).strong());
    let r = ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(w).hint_text(lang.t("z. B. Mein Projekt", "e.g. My project")).margin(egui::Margin::symmetric(8, 5)));
    if std::mem::take(&mut f.focus_name) {
        r.request_focus();
    }
    if r.changed() && !f.key_touched {
        f.key = suggest_key(&f.name, &f.taken);
    }
    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    match f.name_problem() {
        Some(PlanError::NameEmpty) => hint(ui, lang.t("Ein Name ist nötig.", "A name is required.")),
        Some(e) => widgets::error(ui, &e.message(lang)),
        None => {}
    }
    ui.add_space(S2);
    // Schlüssel und Farbe nebeneinander
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(lang.t("Schlüssel", "Key")).strong());
            let r = ui.add(egui::TextEdit::singleline(&mut f.key).desired_width(110.0).char_limit(10).margin(egui::Margin::symmetric(8, 5)));
            if r.changed() {
                f.key = f.key.to_ascii_uppercase();
                f.key_touched = true;
            }
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        });
        ui.add_space(S3);
        ui.vertical(|ui| {
            ui.label(RichText::new(lang.t("Farbe (Akzent im Projekt)", "Color (accent in the project)")).strong());
            widgets::palette_and_hex(ui, "plan_form_color", &mut f.color, lang);
        });
    });
    if let Some(e) = f.key_problem() {
        if !f.key.is_empty() {
            widgets::error(ui, &e.message(lang));
        } else {
            hint(ui, lang.t("Der Schlüssel (2–10 Zeichen) steht vor jeder Ticketnummer, z. B. PAN-12.", "The key (2–10 characters) prefixes every ticket number, e.g. PAN-12."));
        }
    } else if !f.key_touched && new {
        hint(ui, lang.t("Vorschlag aus dem Namen – du kannst ihn ändern.", "Suggested from the name – you can change it."));
    } else if let Some(orig) = &f.original_key {
        if normalize_key(&f.key) != *orig {
            ui.label(
                RichText::new(match lang {
                    Lang::De => format!("Achtung: Alle Ticketnummern ändern sich von {orig}-n auf {}-n; gespeicherte Abfragen mit dem alten Schlüssel greifen nicht mehr.", normalize_key(&f.key)),
                    Lang::En => format!("Careful: all ticket numbers change from {orig}-n to {}-n; saved queries that use the old key will no longer match.", normalize_key(&f.key)),
                })
                .small()
                .color(egui::Color32::from_rgb(0xe5, 0xa0, 0x4b)),
            );
        }
    }
    if let Some(e) = f.color_problem() {
        widgets::error(ui, &e.message(lang));
    }
    ui.add_space(S2);
    // Beschreibung
    ui.label(RichText::new(lang.t("Beschreibung", "Description")).strong());
    ui.add(egui::TextEdit::multiline(&mut f.description).desired_width(w).desired_rows(3).hint_text(lang.t("Wofür ist das Projekt? (optional)", "What is the project for? (optional)")).margin(egui::Margin::symmetric(8, 5)));
    ui.add_space(S2);
    // Arbeitsordner
    ui.label(RichText::new(lang.t("Arbeitsordner (Repo-Pfad)", "Working directory (repo path)")).strong());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut f.work_dir).desired_width((w - 120.0).max(120.0)).hint_text(lang.t("optional", "optional")).margin(egui::Margin::symmetric(8, 5)));
        if ui.button(lang.t("Wählen …", "Browse …")).clicked() {
            if let Some(d) = crate::platform::notify::pick_folder(&f.work_dir) {
                f.work_dir = d;
            }
        }
    });
    hint(ui, lang.t("Terminals zu Tickets dieses Projekts starten später hier.", "Terminals for tickets of this project will start here later."));
    ui.add_space(S2);
    // Schalter
    widgets::toggle_row(ui, &mut f.sprints, lang.t("Sprints verwenden", "Use sprints"), lang.t("Backlog und Sprint-Board – ohne Sprints arbeitet das Projekt im Kanban-Stil.", "Backlog and sprint board – without sprints the project works kanban-style."));
    ui.add_space(S1);
    widgets::toggle_row(ui, &mut f.versions, lang.t("Versionen/Meilensteine verwenden", "Use versions/milestones"), lang.t("Release-Versionen und Meilensteine mit Fälligkeit.", "Release versions and milestones with due dates."));
    ui.add_space(S2);
    ui.label(RichText::new(lang.t("Farbe neuer Tags", "Color of new tags")).strong());
    ui.horizontal_wrapped(|ui| {
        widgets::radio(ui, &mut f.tag_mode, TagColorMode::Default, lang.t("Standardfarbe", "Default color"));
        widgets::radio(ui, &mut f.tag_mode, TagColorMode::Accent, lang.t("Projektfarbe", "Project color"));
        widgets::radio(ui, &mut f.tag_mode, TagColorMode::Random, lang.t("Zufällig", "Random"));
    });
    if new {
        ui.add_space(S2);
        widgets::toggle_row(
            ui,
            &mut f.defaults,
            lang.t("Standard-Typen und -Workflows anlegen", "Create default types and workflows"),
            lang.t(
                "Epic, Userstory, Dev-Aufgabe, Unteraufgabe und Fehler mit Status, Feldern und Hierarchie. Aus: ein komplett leeres Projekt.",
                "Epic, user story, dev task, subtask and defect with statuses, fields and hierarchy. Off: a completely empty project.",
            ),
        );
    }
    enter
}

/// Abschnitt „Wartung“ der Projekt-Einstellungen: Zielstatus „Won't Do“, Schwellen für liegengebliebene Tickets und Felder,
/// Ähnlichkeit für Duplikate.
fn maint_ui(ui: &mut Ui, m: &mut crate::planning::maintenance::Settings, statuses: &[(i64, String)], lang: Lang) {
    use crate::planning::maintenance::{DAYS_RANGE, PERCENT_RANGE};
    ui.add_space(S3);
    ui.label(RichText::new(lang.t("Wartung", "Maintenance")).strong());
    ui.horizontal(|ui| {
        ui.label(lang.t("Zielstatus „Als Won't Do schließen“:", "Target status of “Close as Won't Do”:"));
        let none = lang.t("– keiner (Aktion aus) –", "– none (action off) –");
        let sel = m.wont_do.and_then(|s| statuses.iter().find(|x| x.0 == s)).map(|x| x.1.clone()).unwrap_or_else(|| none.to_string());
        egui::ComboBox::from_id_salt("maint_wont_do").selected_text(sel).show_ui(ui, |ui| {
            ui.selectable_value(&mut m.wont_do, None, none);
            for (id, name) in statuses {
                ui.selectable_value(&mut m.wont_do, Some(*id), name);
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(lang.t("Tickets gelten als liegengeblieben nach", "Tickets count as stale after"));
        ui.add(egui::DragValue::new(&mut m.stale_days).range(DAYS_RANGE));
        ui.label(lang.t("Tagen, Felder nach", "days, fields after"));
        ui.add(egui::DragValue::new(&mut m.field_days).range(DAYS_RANGE));
        ui.label(lang.t("Tagen", "days"));
    });
    ui.horizontal(|ui| {
        ui.label(lang.t("Mögliche Duplikate ab Titel-Ähnlichkeit", "Possible duplicates from title similarity"));
        ui.add(egui::DragValue::new(&mut m.duplicate_percent).range(PERCENT_RANGE).suffix(" %"));
    });
    hint(ui, lang.t("Ohne Zielstatus ist „Als Won't Do schließen“ auf der Wartungsseite aus.", "Without a target status, “Close as Won't Do” is off on the maintenance page."));
}

impl PlanUi {
    // ------------------------------------------------------------------ Assistent „Neues Projekt“

    pub(super) fn wizard_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(mut f) = self.wizard.take() else { return };
        let lang = cx.lang;
        let mut create = false;
        let mut close = false;
        let m = crate::style::modal(Self::modal_id("wizard")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 540.0));
            widgets::dialog_title(ui, lang.t("Neues Projekt", "New project"));
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 220.0).max(240.0)).auto_shrink([true, true]).show(ui, |ui| {
                if form_ui(ui, &mut f, lang, true) && f.is_valid() {
                    create = true;
                }
            });
            ui.add_space(S2);
            if let Some(e) = &f.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, f.is_valid(), lang.t("Anlegen", "Create")).clicked() {
                    create = true;
                }
                if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if create && f.is_valid() {
            let np = NewProject {
                key: f.key.clone(),
                name: f.name.clone(),
                color: f.color.clone(),
                description: f.description.clone(),
                work_dir: f.work_dir.clone(),
                use_sprints: f.sprints,
                use_versions: f.versions,
                tag_color_mode: f.tag_mode,
                with_defaults: f.defaults,
            };
            match create_project(cx.conn, &np) {
                Ok(id) => {
                    self.selected = id;
                    cx.db.set_meta("expert_project", &id.to_string());
                    self.tab = Tab::Overview;
                    self.dirty = true;
                    self.note(match lang {
                        Lang::De => format!("Projekt {} angelegt.", lang.q(&np.name)),
                        Lang::En => format!("Project {} created.", lang.q(&np.name)),
                    });
                    close = true;
                }
                Err(e) => f.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.wizard = Some(f);
        }
    }

    // ------------------------------------------------------------------ Einstellungen

    pub(super) fn open_settings(&mut self, p: &Project) {
        self.settings = Some((p.id, Form::from_project(p, &self.projects)));
    }

    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some((id, mut f)) = self.settings.take() else { return };
        let lang = cx.lang;
        if f.maint.is_none() {
            let s = crate::planning::maintenance::load_settings(cx.conn, id).unwrap_or_default();
            let st = crate::planning::statuses::list_statuses(cx.conn, id).unwrap_or_default().iter().map(|s| (s.id, s.display(lang))).collect();
            f.maint = Some((s, st));
        }
        let mut save = false;
        let mut close = false;
        let m = crate::style::modal(Self::modal_id("settings")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 540.0));
            widgets::dialog_title(ui, lang.t("Projekt-Einstellungen", "Project settings"));
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 220.0).max(240.0)).auto_shrink([true, true]).show(ui, |ui| {
                if form_ui(ui, &mut f, lang, false) && f.is_valid() {
                    save = true;
                }
                if let Some((m, statuses)) = f.maint.as_mut() {
                    maint_ui(ui, m, statuses, lang);
                }
            });
            ui.add_space(S2);
            if let Some(e) = &f.err {
                widgets::error(ui, e);
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, f.is_valid(), lang.t("Speichern", "Save")).clicked() {
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
        if save && f.is_valid() {
            let u = UpdateProject {
                key: Some(f.key.clone()),
                name: Some(f.name.clone()),
                description: Some(f.description.clone()),
                color: Some(f.color.clone()),
                work_dir: Some(f.work_dir.clone()),
                use_sprints: Some(f.sprints),
                use_versions: Some(f.versions),
                tag_color_mode: Some(f.tag_mode),
            };
            let r = update_project(cx.conn, id, &u).and_then(|()| match &f.maint {
                Some((m, _)) => crate::planning::maintenance::save_settings(cx.conn, id, m).map_err(PlanError::from),
                None => Ok(()),
            });
            match r {
                Ok(()) => {
                    self.dirty = true;
                    self.maint.counts.remove(&id);
                    close = true;
                }
                Err(e) => f.err = Some(e.message(lang)),
            }
        }
        if !close {
            self.settings = Some((id, f));
        }
    }

    // ------------------------------------------------------------------ Löschen

    pub(super) fn ask_delete_project(&mut self, cx: &Cx, p: &Project) {
        let tickets = delete_impact(cx.conn, p.id).unwrap_or(0);
        let (types, statuses, fields) = match &self.sum {
            Some(s) if s.project.id == p.id => (s.types.len(), s.statuses.len(), s.types.iter().map(|t| t.fields.len()).sum()),
            _ => (0, 0, 0),
        };
        self.delete_project = Some(DeleteAsk { id: p.id, name: p.name.clone(), key: p.key.clone(), tickets, types, statuses, fields, since: Instant::now() });
    }

    pub(super) fn delete_project_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(ask) = self.delete_project.take() else { return };
        let lang = cx.lang;
        let left = CONFIRM_SECS - ask.since.elapsed().as_secs_f32();
        let secs = left.ceil().max(0.0) as u32;
        let mut go = false;
        let mut close = false;
        let m = crate::style::modal(Self::modal_id("delete_project")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 440.0));
            ui.horizontal(|ui| {
                ui.colored_label(widgets::DANGER(), RichText::new("!").heading().strong());
                ui.label(RichText::new(lang.t("Projekt endgültig löschen?", "Delete project permanently?")).heading());
            });
            ui.add_space(S2);
            ui.label(RichText::new(format!("{} ({})", ask.name, ask.key)).strong());
            ui.add_space(S1);
            let tickets = match lang {
                Lang::De => format!("{} Ticket(s) gehen verloren.", ask.tickets),
                Lang::En => format!("{} ticket(s) will be lost.", ask.tickets),
            };
            let config = match lang {
                Lang::De => format!("Gelöscht werden außerdem {} Typ(en), {} Feld(er) und {} Status samt Workflows und Arten der Arbeit.", ask.types, ask.fields, ask.statuses),
                Lang::En => format!("Also deleted: {} type(s), {} field(s) and {} status(es) with their workflows and kinds of work.", ask.types, ask.fields, ask.statuses),
            };
            ui.label(RichText::new(tickets).strong());
            ui.label(config);
            ui.add_space(S1);
            ui.label(lang.t("Das lässt sich nicht rückgängig machen.", "This cannot be undone."));
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
        if go {
            match delete_project(cx.conn, ask.id) {
                Ok(_) => {
                    // Kacheln anderer Dashboards, die das Projekt nannten, aufräumen
                    let _ = crate::planning::views::dashboards::prune_dashboard_refs(cx.conn);
                    self.dash_invalidate();
                    self.dirty = true;
                    self.selected = self.projects.iter().find(|p| p.system).map(|p| p.id).unwrap_or(0);
                    cx.db.set_meta("expert_project", &self.selected.to_string());
                    self.tab = Tab::Overview;
                    self.tickets.reset_history();
                    self.note(match lang {
                        Lang::De => format!("Projekt {} gelöscht.", lang.q(&ask.name)),
                        Lang::En => format!("Project {} deleted.", lang.q(&ask.name)),
                    });
                }
                Err(e) => self.fail(cx, &e),
            }
        } else if !close {
            self.delete_project = Some(ask);
        }
    }
}

pub(super) struct DeleteAsk {
    id: i64,
    name: String,
    key: String,
    tickets: i64,
    types: usize,
    statuses: usize,
    fields: usize,
    since: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: i64, key: &str, system: bool) -> Project {
        Project {
            id,
            key: key.into(),
            name: key.into(),
            description: String::new(),
            color: "#112233".into(),
            work_dir: String::new(),
            use_sprints: false,
            use_versions: false,
            tag_color_mode: TagColorMode::Default,
            system,
            next_number: 1,
            position: 0,
            created_at: String::new(),
        }
    }

    #[test]
    fn form_validates_live() {
        let ps = vec![project(1, "TODO", true), project(2, "PAN", false)];
        let mut f = Form::new_project(&ps);
        assert!(!f.is_valid(), "ohne Namen nicht gültig");
        assert_eq!(f.name_problem(), Some(PlanError::NameEmpty));
        f.name = "Paneldeck".into();
        f.key = suggest_key(&f.name, &f.taken);
        assert_eq!(f.key, "PAN2", "PAN ist vergeben");
        assert!(f.is_valid());
        f.key = "pan".into();
        assert_eq!(f.key_problem(), Some(PlanError::KeyTaken), "Klein-/Großschreibung egal");
        f.key = "1AB".into();
        assert_eq!(f.key_problem(), Some(PlanError::KeyInvalid));
        f.key = "ABC".into();
        f.color = "red".into();
        assert_eq!(f.color_problem(), Some(PlanError::ColorInvalid));
        assert!(!f.is_valid());
    }

    #[test]
    fn settings_form_allows_own_key() {
        let ps = vec![project(1, "TODO", true), project(2, "PAN", false)];
        let f = Form::from_project(&ps[1], &ps);
        assert!(f.is_valid(), "der eigene Schlüssel zählt nicht als vergeben");
        assert!(f.taken.contains(&"TODO".to_string()) && !f.taken.contains(&"PAN".to_string()));
    }

    #[test]
    fn new_project_color_avoids_used_colors() {
        let mut ps = vec![project(1, "TODO", true)];
        let f = Form::new_project(&ps);
        let pal = crate::palette::auto(crate::palette::Area::Expert);
        assert_eq!(f.color, pal[0]);
        let mut p = project(2, "AB", false);
        p.color = pal[0].clone();
        ps.push(p);
        assert_eq!(Form::new_project(&ps).color, pal[1]);
    }
}
