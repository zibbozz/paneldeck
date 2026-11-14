//! Reiter „Vorlagen“: Typen, Felder, Status und Workflows eines Projekts als JSON-Datei exportieren und in ein Projekt
//! importieren. Der Import legt nur an, was fehlt, und meldet Gleichnamiges (Konflikte); Bestehendes bleibt unberührt.

use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::template::{export_types, import_types, ImportReport, TemplateDoc};
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, RichText, Ui};

#[derive(Default)]
pub(super) struct State {
    preview: Option<Preview>,
    report: Option<ImportReport>,
}

struct Preview {
    file: String,
    doc: TemplateDoc,
}

impl PlanUi {
    pub(super) fn templates_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        hint(ui, lang.t(
            "Vorlagen enthalten die Konfiguration, nicht die Tickets: Status, Typen, Felder, Workflows, Hierarchie und Arten der Arbeit.",
            "Templates hold the configuration, not the tickets: statuses, types, fields, workflows, hierarchy and kinds of work.",
        ));
        ui.add_space(S2);
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Exportieren", "Export")).strong());
            ui.label(RichText::new(lang.t("Schreibt die Konfiguration dieses Projekts in eine JSON-Datei, z. B. um sie in einem anderen Projekt zu verwenden.", "Writes this project's configuration to a JSON file, e.g. to reuse it in another project.")).small().color(style::dim(ui)));
            ui.add_space(S1);
            if ui.button(lang.t("Typen, Felder und Workflows exportieren …", "Export types, fields and workflows …")).clicked() {
                self.export_template(cx, sum.project.id, &sum.project.key);
            }
        });
        ui.add_space(S2);
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Importieren", "Import")).strong());
            ui.label(RichText::new(lang.t(
                "Liest eine Vorlagen-Datei in dieses Projekt. Es wird nur angelegt, was fehlt; gleichnamige Einträge bleiben unberührt und werden gemeldet.",
                "Reads a template file into this project. Only what is missing is created; entries with the same name stay untouched and are reported.",
            )).small().color(style::dim(ui)));
            ui.add_space(S1);
            if ui.button(lang.t("Vorlage importieren …", "Import template …")).clicked() {
                self.pick_template(cx);
            }
        });
    }

    fn export_template(&mut self, cx: &Cx, project_id: i64, key: &str) {
        let lang = cx.lang;
        let doc = match export_types(cx.conn, project_id) {
            Ok(d) => d,
            Err(e) => return self.fail(cx, &e),
        };
        let Some(path) = crate::notify::save_json(&format!("{}-template.json", key.to_lowercase())) else { return };
        match std::fs::write(&path, doc.to_json()) {
            Ok(()) => self.note(match lang {
                Lang::De => format!("Vorlage exportiert: {}", path.display()),
                Lang::En => format!("Template exported: {}", path.display()),
            }),
            Err(e) => self.msg = Some(super::Msg { text: format!("{}: {e}", lang.t("Die Datei konnte nicht geschrieben werden", "The file could not be written")), ok: false, at: std::time::Instant::now() }),
        }
    }

    fn pick_template(&mut self, cx: &Cx) {
        let lang = cx.lang;
        let Some(path) = crate::notify::open_json() else { return };
        let text = match std::fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).to_string(),
            Err(e) => {
                self.msg = Some(super::Msg { text: format!("{}: {e}", lang.t("Die Datei konnte nicht gelesen werden", "The file could not be read")), ok: false, at: std::time::Instant::now() });
                return;
            }
        };
        match TemplateDoc::from_json(&text) {
            Ok(doc) => {
                self.templates.preview = Some(Preview { file: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), doc });
                self.msg = None;
            }
            Err(e) => self.fail(cx, &e),
        }
    }

    pub(super) fn template_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        // Vorschau vor dem Import
        if let Some(p) = self.templates.preview.take() {
            let mut go = false;
            let mut close = false;
            let fields: usize = p.doc.types.iter().map(|t| t.fields.len()).sum();
            let m = egui::Modal::new(Self::modal_id("tpl_preview")).show(ctx, |ui| {
                ui.set_width(widgets::modal_width(ctx, 460.0));
                widgets::dialog_title(ui, lang.t("Vorlage importieren", "Import template"));
                ui.label(RichText::new(&p.file).monospace());
                ui.add_space(S2);
                let line = match lang {
                    Lang::De => format!("Enthält {} Status, {} Typ(en) mit {} Feld(ern) und {} Art(en) der Arbeit.", p.doc.statuses.len(), p.doc.types.len(), fields, p.doc.work_kinds.len()),
                    Lang::En => format!("Contains {} status(es), {} type(s) with {} field(s) and {} kind(s) of work.", p.doc.statuses.len(), p.doc.types.len(), fields, p.doc.work_kinds.len()),
                };
                ui.label(line);
                ui.add_space(S1);
                hint(ui, lang.t("Gleichnamige Einträge werden nicht verändert, sondern gemeldet.", "Entries with the same name are not changed but reported."));
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
                match import_types(cx.conn, self.selected, &p.doc) {
                    Ok(rep) => {
                        self.dirty = true;
                        self.templates.report = Some(rep);
                    }
                    Err(e) => self.fail(cx, &e),
                }
            } else if !close {
                self.templates.preview = Some(p);
            }
        }
        // Bericht nach dem Import
        if let Some(rep) = self.templates.report.take() {
            let mut close = false;
            let m = egui::Modal::new(Self::modal_id("tpl_report")).show(ctx, |ui| {
                ui.set_width(widgets::modal_width(ctx, 500.0));
                widgets::dialog_title(ui, lang.t("Import abgeschlossen", "Import finished"));
                let line = match lang {
                    Lang::De => format!("Angelegt: {} Status, {} Typ(en), {} Feld(er), {} Art(en) der Arbeit.", rep.created_statuses, rep.created_types, rep.created_fields, rep.created_work_kinds),
                    Lang::En => format!("Created: {} status(es), {} type(s), {} field(s), {} kind(s) of work.", rep.created_statuses, rep.created_types, rep.created_fields, rep.created_work_kinds),
                };
                ui.label(line);
                ui.add_space(S2);
                if rep.has_conflicts() {
                    ui.label(RichText::new(lang.t("Konflikte (nicht importiert):", "Conflicts (not imported):")).strong());
                    egui::ScrollArea::vertical().max_height(220.0).id_salt("tpl_conflicts").show(ui, |ui| {
                        for c in &rep.conflicts {
                            ui.label(format!("• {}", c.message(lang)));
                        }
                    });
                } else {
                    ui.label(RichText::new(lang.t("Keine Konflikte.", "No conflicts.")).color(style::dim(ui)));
                }
                ui.add_space(S3);
                if widgets::primary(ui, lang.t("Schließen", "Close")).clicked() {
                    close = true;
                }
            });
            if m.should_close() {
                close = true;
            }
            if !close {
                self.templates.report = Some(rep);
            }
        }
    }
}
