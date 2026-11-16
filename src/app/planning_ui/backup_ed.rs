//! Projektsicherung (JSON) und Tickets als CSV im Reiter „Vorlagen & Sicherung“: exportieren, importieren, Vorschau und
//! Bericht. Ein Projekt-Import legt immer ein **neues** Projekt an (bei vergebenem Kürzel mit Eingabe eines neuen Kürzels);
//! der CSV-Import legt neue Tickets im gewählten Projekt an. Die Logik steckt in `crate::planning::backup_doc`.
//!
//! **Große Dateien:** Lesen, Parsen, Prüfen und Schreiben laufen in einem Hintergrund-Faden (`backup_run.rs`), die Oberfläche zeigt
//! währenddessen den Dialog „Import läuft“ mit Fortschritt und „Abbrechen“. Der Import ist eine einzige Transaktion: Fehler und
//! Abbruch nehmen alles zurück. Ergebnis und Fehler erscheinen danach wie bisher in den Dialogen.

use super::backup_run::{self as run, LoadFail, LoadedCsv, LoadedProject, Poll, Task};
use super::helpers::suggest_key;
use super::widgets::{self, hint};
use super::{Cx, Msg, PlanUi};
use crate::planning::backup_doc::tickets_csv::{export_tickets_csv, CsvImportReport, CsvOptions};
use crate::planning::backup_doc::{check_new_key, export_project, BackupError, ImportResult, Preview, ProjectDoc, Progress};
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Key, RichText, Ui};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct State {
    pub(super) project: Option<ProjectImport>,
    pub(super) project_report: Option<ImportResult>,
    /// Formel-Schutz beim CSV-Export (Standard an).
    csv_safe: bool,
    pub(super) csv: Option<CsvImport>,
    pub(super) csv_report: Option<(String, CsvImportReport)>,
    /// Ein Lauf im Hintergrund (Datei lesen oder importieren); solange er besteht, zeigt der Dialog „Import läuft“ den Fortschritt.
    pub(super) job: Option<Job>,
}

impl Default for State {
    fn default() -> Self {
        State { project: None, project_report: None, csv_safe: true, csv: None, csv_report: None, job: None }
    }
}

pub(super) struct ProjectImport {
    pub(super) file: String,
    /// Mit dem Import-Lauf geteilt (nicht kopiert: die Datei kann 128 MB groß sein).
    pub(super) doc: Arc<ProjectDoc>,
    pub(super) preview: Preview,
    /// Eingabe des neuen Kürzels (nur bei Kollision).
    pub(super) new_key: String,
    pub(super) error: Option<BackupError>,
}

pub(super) struct CsvImport {
    pub(super) file: String,
    pub(super) text: Arc<String>,
    pub(super) rows: usize,
    pub(super) project_id: i64,
    pub(super) project_key: String,
    pub(super) error: Option<BackupError>,
}

/// Ein Lauf im Hintergrund. Beim Import bleibt der Dialogzustand (`p`) hier, damit ein Fehler den Dialog wieder öffnen kann.
pub(super) enum Job {
    LoadProject { file: String, task: Task<Result<LoadedProject, LoadFail>> },
    LoadCsv { file: String, project_id: i64, project_key: String, task: Task<Result<LoadedCsv, LoadFail>> },
    ImportProject { p: ProjectImport, task: Task<Result<ImportResult, BackupError>>, cancelling: bool },
    ImportCsv { p: CsvImport, task: Task<Result<CsvImportReport, BackupError>>, cancelling: bool },
}

impl Job {
    fn file(&self) -> &str {
        match self {
            Job::LoadProject { file, .. } | Job::LoadCsv { file, .. } => file,
            Job::ImportProject { p, .. } => &p.file,
            Job::ImportCsv { p, .. } => &p.file,
        }
    }

    fn title(&self, lang: Lang) -> &'static str {
        match self {
            Job::LoadProject { .. } | Job::LoadCsv { .. } => lang.t("Datei wird gelesen …", "Reading the file …"),
            Job::ImportProject { .. } => lang.t("Projekt wird importiert …", "Importing project …"),
            Job::ImportCsv { .. } => lang.t("Tickets werden importiert …", "Importing tickets …"),
        }
    }

    /// Schreibt der Lauf in die Datenbank (dann lässt er sich nur mit Rücknahme abbrechen)?
    pub(super) fn importing(&self) -> bool {
        matches!(self, Job::ImportProject { .. } | Job::ImportCsv { .. })
    }

    fn cancelling(&self) -> bool {
        matches!(self, Job::ImportProject { cancelling: true, .. } | Job::ImportCsv { cancelling: true, .. })
    }

    pub(super) fn progress(&self) -> &Arc<Progress> {
        match self {
            Job::LoadProject { task, .. } => &task.progress,
            Job::LoadCsv { task, .. } => &task.progress,
            Job::ImportProject { task, .. } => &task.progress,
            Job::ImportCsv { task, .. } => &task.progress,
        }
    }

    fn elapsed(&self) -> Duration {
        match self {
            Job::LoadProject { task, .. } => task.elapsed(),
            Job::LoadCsv { task, .. } => task.elapsed(),
            Job::ImportProject { task, .. } => task.elapsed(),
            Job::ImportCsv { task, .. } => task.elapsed(),
        }
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Weckt die Oberfläche aus dem Hintergrund-Faden.
fn waker(ctx: &egui::Context) -> impl Fn() + Send + 'static {
    let ctx = ctx.clone();
    move || ctx.request_repaint()
}

fn counts_line(lang: Lang, p: &Preview) -> String {
    match lang {
        Lang::De => format!(
            "{} Ticket(s), {} Kommentar(e), {} Verknüpfung(en), {} Zeiteintrag/-einträge, {} Verlaufseinträge · {} Sprint(s), {} Version(en), {} Meilenstein(e) · {} Typ(en) mit {} Feld(ern), {} Status, {} Board(s), {} Filter",
            p.tickets, p.comments, p.links, p.time_entries, p.history, p.sprints, p.versions, p.milestones, p.types, p.fields, p.statuses, p.boards, p.filters
        ),
        Lang::En => format!(
            "{} ticket(s), {} comment(s), {} link(s), {} time entr(ies), {} history entries · {} sprint(s), {} version(s), {} milestone(s) · {} type(s) with {} field(s), {} status(es), {} board(s), {} filter(s)",
            p.tickets, p.comments, p.links, p.time_entries, p.history, p.sprints, p.versions, p.milestones, p.types, p.fields, p.statuses, p.boards, p.filters
        ),
    }
}

impl PlanUi {
    /// Läuft gerade ein Lese- oder Import-Lauf (Dialog „Import läuft“)? Dann gehören Esc und Strg+Z dem Dialog, nicht der Ticket-Liste.
    pub(super) fn backup_running(&self) -> bool {
        self.templates.backup.job.is_some()
    }

    fn backup_error(&mut self, cx: &Cx, e: &BackupError) {
        self.msg = Some(Msg { text: e.message(cx.lang), ok: false, at: Instant::now() });
    }

    fn backup_io_error(&mut self, cx: &Cx, what: (&'static str, &'static str), e: &dyn std::fmt::Display) {
        self.msg = Some(Msg { text: format!("{}: {e}", cx.lang.t(what.0, what.1)), ok: false, at: Instant::now() });
    }

    /// Die beiden Karten des Reiters: Projektsicherung und Ticket-CSV.
    pub(super) fn backup_cards(&mut self, ui: &mut Ui, cx: &Cx, project_id: i64, key: &str) {
        let lang = cx.lang;
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Projektsicherung (JSON)", "Project backup (JSON)")).strong());
            ui.label(
                RichText::new(lang.t(
                    "Sichert das ganze Projekt: Einstellungen, Typen, Felder, Workflows, Boards, Filter, Sprints, Versionen, Meilensteine, Tickets mit Werten, Kommentaren, Verlauf, Verknüpfungen und Zeiteinträgen. Der Import legt immer ein neues Projekt an und ändert nie Bestehendes.",
                    "Backs up the whole project: settings, types, fields, workflows, boards, filters, sprints, versions, milestones, tickets with values, comments, history, links and time entries. Importing always creates a new project and never changes existing data.",
                ))
                .small()
                .color(style::dim(ui)),
            );
            ui.add_space(S1);
            ui.horizontal(|ui| {
                if ui.button(lang.t("Projekt exportieren (JSON) …", "Export project (JSON) …")).clicked() {
                    self.export_backup(cx, project_id, key);
                }
                if ui.button(lang.t("Projekt importieren …", "Import project …")).clicked() {
                    self.pick_backup(ui.ctx());
                }
            });
        });
        ui.add_space(S2);
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Tickets (CSV)", "Tickets (CSV)")).strong());
            ui.label(
                RichText::new(lang.t(
                    "Eine Zeile je Ticket: Schlüssel, Typ, Titel, Status, Eltern-Schlüssel, Markierungen, Zeitstempel, bei Projekten mit Sprints der Sprint und je Feld eine Spalte (Versionen und Meilensteine mit ihrem Namen). Der Import nimmt genau dieses Format (kein fremdes) und legt immer neue Tickets in diesem Projekt an; Schlüssel der Datei werden nur zum Auflösen von Eltern und Ticket-Feldern benutzt.",
                    "One row per ticket: key, type, title, status, parent key, flags, timestamps, the sprint (for projects with sprints) and one column per field (versions and milestones by name). The import accepts exactly this format (no foreign formats) and always creates new tickets in this project; keys in the file are only used to resolve parents and ticket fields.",
                ))
                .small()
                .color(style::dim(ui)),
            );
            ui.add_space(S1);
            widgets::check(ui, &mut self.templates.backup.csv_safe, lang.t("Excel-sicher (Formeln mit ' schützen)", "Excel-safe (protect formulas with ')"));
            hint(
                ui,
                lang.t(
                    "Zellen, die mit = + - oder @ beginnen, bekommen beim Export ein führendes ' (kein Formel-Angriff beim Öffnen in Excel); der Import entfernt es wieder.",
                    "Cells starting with = + - or @ get a leading ' on export (no formula injection when opened in Excel); the import removes it again.",
                ),
            );
            ui.add_space(S1);
            ui.horizontal(|ui| {
                if ui.button(lang.t("Tickets exportieren (CSV) …", "Export tickets (CSV) …")).clicked() {
                    self.export_csv(cx, project_id, key);
                }
                if ui.button(lang.t("Tickets importieren (CSV) …", "Import tickets (CSV) …")).clicked() {
                    self.pick_csv(ui.ctx(), project_id, key);
                }
            });
        });
    }

    // ------------------------------------------------------------------ Projektsicherung

    fn export_backup(&mut self, cx: &Cx, project_id: i64, key: &str) {
        let lang = cx.lang;
        let doc = match export_project(cx.conn, project_id) {
            Ok(d) => d,
            Err(e) => return self.backup_error(cx, &e),
        };
        let Some(path) = crate::platform::notify::save_json(&format!("{}-backup.json", key.to_lowercase())) else { return };
        match std::fs::write(&path, doc.to_json()) {
            Ok(()) => self.note(match lang {
                Lang::De => format!("Projekt exportiert: {}", path.display()),
                Lang::En => format!("Project exported: {}", path.display()),
            }),
            Err(e) => self.backup_io_error(cx, ("Die Datei konnte nicht geschrieben werden", "The file could not be written"), &e),
        }
    }

    fn pick_backup(&mut self, ctx: &egui::Context) {
        if self.templates.backup.job.is_some() {
            return;
        }
        let Some(path) = crate::platform::notify::open_json() else { return };
        self.begin_load_project(ctx, path);
    }

    /// Eine Projektsicherung im Hintergrund lesen; danach öffnet sich der Vorschau-Dialog (siehe [`PlanUi::backup_poll`]).
    pub(super) fn begin_load_project(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        let file = file_name(&path);
        self.templates.backup.job = Some(Job::LoadProject { file, task: run::start_load_project(waker(ctx), path) });
        self.msg = None;
        ctx.request_repaint();
    }

    // ------------------------------------------------------------------ CSV

    fn export_csv(&mut self, cx: &Cx, project_id: i64, key: &str) {
        let lang = cx.lang;
        let opts = CsvOptions { excel_safe: self.templates.backup.csv_safe, ..CsvOptions::for_ui() };
        let text = match export_tickets_csv(cx.conn, project_id, &opts) {
            Ok(t) => t,
            Err(e) => return self.backup_error(cx, &e),
        };
        let Some(path) = crate::platform::notify::save_csv(&format!("{}-tickets.csv", key.to_lowercase())) else { return };
        match std::fs::write(&path, text) {
            Ok(()) => self.note(match lang {
                Lang::De => format!("Tickets exportiert: {}", path.display()),
                Lang::En => format!("Tickets exported: {}", path.display()),
            }),
            Err(e) => self.backup_io_error(cx, ("Die Datei konnte nicht geschrieben werden", "The file could not be written"), &e),
        }
    }

    fn pick_csv(&mut self, ctx: &egui::Context, project_id: i64, key: &str) {
        if self.templates.backup.job.is_some() {
            return;
        }
        let Some(path) = crate::platform::notify::open_csv() else { return };
        self.begin_load_csv(ctx, path, project_id, key);
    }

    /// Eine Ticket-CSV im Hintergrund lesen; danach öffnet sich der Dialog (siehe [`PlanUi::backup_poll`]).
    pub(super) fn begin_load_csv(&mut self, ctx: &egui::Context, path: std::path::PathBuf, project_id: i64, key: &str) {
        let file = file_name(&path);
        self.templates.backup.job = Some(Job::LoadCsv { file, project_id, project_key: key.to_string(), task: run::start_load_csv(waker(ctx), path) });
        self.msg = None;
        ctx.request_repaint();
    }

    /// Den Import der Projektsicherung im Hintergrund starten (Transaktion auf eigener Verbindung, siehe `backup_run`).
    pub(super) fn begin_project_import(&mut self, ctx: &egui::Context, cx: &Cx, p: ProjectImport, key: Option<String>) {
        let task = run::start_project_import(waker(ctx), cx.db, p.doc.clone(), key);
        self.templates.backup.job = Some(Job::ImportProject { p, task, cancelling: false });
        ctx.request_repaint();
    }

    /// Den Import der Ticket-CSV im Hintergrund starten.
    pub(super) fn begin_csv_import(&mut self, ctx: &egui::Context, cx: &Cx, p: CsvImport) {
        let task = run::start_csv_import(waker(ctx), cx.db, p.project_id, p.text.clone());
        self.templates.backup.job = Some(Job::ImportCsv { p, task, cancelling: false });
        ctx.request_repaint();
    }

    /// Meldung zu einer Datei, die nicht gelesen werden konnte.
    fn load_fail(&mut self, cx: &Cx, f: LoadFail) {
        match f {
            LoadFail::Backup(e) => self.backup_error(cx, &e),
            LoadFail::Io(e) => self.backup_io_error(cx, ("Die Datei konnte nicht gelesen werden", "The file could not be read"), &e),
        }
    }

    /// Pro Bild: ein fertiger Lauf wird ausgewertet. Gelesene Dateien öffnen ihren Dialog, ein gelungener Import lädt neu und zeigt den
    /// Bericht, ein misslungener öffnet den Dialog mit dem Fehler wieder, ein abgebrochener hinterlässt nichts.
    pub(super) fn backup_poll(&mut self, cx: &Cx) {
        let lang = cx.lang;
        let Some(job) = self.templates.backup.job.take() else { return };
        let lost = || BackupError::Db(lang.t("Der Import-Faden wurde unerwartet beendet", "The import thread ended unexpectedly").to_string());
        match job {
            Job::LoadProject { file, mut task } => match task.poll() {
                Poll::Pending => self.templates.backup.job = Some(Job::LoadProject { file, task }),
                Poll::Done(Ok(l)) => {
                    let taken: Vec<String> = self.projects.iter().map(|p| p.key.clone()).collect();
                    let new_key = suggest_key(&l.doc.project.key.to_lowercase(), &taken);
                    self.templates.backup.project = Some(ProjectImport { file, doc: l.doc, preview: l.preview, new_key, error: None });
                    self.msg = None;
                }
                Poll::Done(Err(f)) => self.load_fail(cx, f),
                Poll::Lost => self.backup_error(cx, &lost()),
            },
            Job::LoadCsv { file, project_id, project_key, mut task } => match task.poll() {
                Poll::Pending => self.templates.backup.job = Some(Job::LoadCsv { file, project_id, project_key, task }),
                Poll::Done(Ok(l)) => {
                    self.templates.backup.csv = Some(CsvImport { file, text: l.text, rows: l.rows, project_id, project_key, error: None });
                    self.msg = None;
                }
                Poll::Done(Err(f)) => self.load_fail(cx, f),
                Poll::Lost => self.backup_error(cx, &lost()),
            },
            Job::ImportProject { mut p, mut task, cancelling } => match task.poll() {
                Poll::Pending => self.templates.backup.job = Some(Job::ImportProject { p, task, cancelling }),
                Poll::Done(Ok(res)) => {
                    self.invalidate();
                    self.select(cx, res.project_id);
                    self.note(match lang {
                        Lang::De => format!("Projekt {} importiert ({} Tickets)", res.key, res.report.tickets),
                        Lang::En => format!("Project {} imported ({} tickets)", res.key, res.report.tickets),
                    });
                    self.templates.backup.project_report = Some(res);
                }
                Poll::Done(Err(BackupError::Cancelled)) => self.note(BackupError::Cancelled.message(lang)),
                Poll::Done(Err(e)) => {
                    p.error = Some(e);
                    self.templates.backup.project = Some(p);
                }
                Poll::Lost => {
                    p.error = Some(lost());
                    self.templates.backup.project = Some(p);
                }
            },
            Job::ImportCsv { mut p, mut task, cancelling } => match task.poll() {
                Poll::Pending => self.templates.backup.job = Some(Job::ImportCsv { p, task, cancelling }),
                Poll::Done(Ok(rep)) => {
                    self.invalidate();
                    self.note(match lang {
                        Lang::De => format!("{} Ticket(s) importiert", rep.created),
                        Lang::En => format!("{} ticket(s) imported", rep.created),
                    });
                    self.templates.backup.csv_report = Some((p.project_key.clone(), rep));
                }
                Poll::Done(Err(BackupError::Cancelled)) => self.note(BackupError::Cancelled.message(lang)),
                Poll::Done(Err(e)) => {
                    p.error = Some(e);
                    self.templates.backup.csv = Some(p);
                }
                Poll::Lost => {
                    p.error = Some(lost());
                    self.templates.backup.csv = Some(p);
                }
            },
        }
    }

    /// Dialog „Import läuft“: Fortschritt und Abbrechen. Das Lesen einer Datei wird mit „Abbrechen“ vergessen (das Ergebnis wird
    /// verworfen); ein laufender Import bekommt den Abbruch gemeldet und nimmt dann alles zurück.
    fn backup_busy_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        let Some(job) = &self.templates.backup.job else { return };
        let (title, file, importing, cancelling) = (job.title(lang), job.file().to_string(), job.importing(), job.cancelling());
        let progress = job.progress().clone();
        let secs = job.elapsed().as_secs();
        // Esc bricht ab (vor dem Dialog gelesen: er verbraucht die Taste selbst); ein Klick daneben nicht, damit ein langer Import
        // nicht aus Versehen endet.
        let mut cancel = ctx.input(|i| i.key_pressed(Key::Escape));
        egui::Modal::new(Self::modal_id("backup_busy")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 460.0));
            widgets::dialog_title(ui, title);
            ui.label(RichText::new(&file).monospace());
            ui.add_space(S2);
            match progress.fraction() {
                Some(f) if importing => {
                    ui.add(egui::ProgressBar::new(f).show_percentage().desired_width(ui.available_width()));
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(RichText::new(if importing { lang.t("Läuft …", "Running …") } else { lang.t("Große Dateien brauchen einen Moment.", "Large files take a moment.") }).color(style::dim(ui)));
                    });
                }
            }
            ui.add_space(S1);
            ui.label(RichText::new(format!("{secs} s")).small().color(style::dim(ui)));
            if importing {
                hint(
                    ui,
                    lang.t(
                        "Alles oder nichts: Bei „Abbrechen“ oder einem Fehler wird nichts angelegt.",
                        "All or nothing: if you cancel or an error occurs, nothing is created.",
                    ),
                );
            }
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if cancelling {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(lang.t("Wird abgebrochen …", "Cancelling …")).color(style::dim(ui)));
                } else if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            match &mut self.templates.backup.job {
                Some(Job::ImportProject { task, cancelling, .. }) => {
                    task.progress.cancel();
                    *cancelling = true;
                }
                Some(Job::ImportCsv { task, cancelling, .. }) => {
                    task.progress.cancel();
                    *cancelling = true;
                }
                // Lesen: vergessen (der Faden läuft zu Ende, sein Ergebnis wird nicht mehr abgeholt)
                other => *other = None,
            }
        }
        ctx.request_repaint_after(Duration::from_millis(120));
    }

    // ------------------------------------------------------------------ Dialoge

    pub(super) fn backup_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        self.backup_poll(cx);
        self.backup_busy_dialog(ctx, cx);
        self.project_import_dialog(ctx, cx);
        self.project_report_dialog(ctx, cx);
        self.csv_import_dialog(ctx, cx);
        self.csv_report_dialog(ctx, cx);
    }

    fn project_import_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        let Some(mut p) = self.templates.backup.project.take() else { return };
        // Kollision des Kürzels des Dokuments? (live: das Projekt kann inzwischen angelegt oder gelöscht sein)
        let doc_key = check_new_key(cx.conn, &p.doc.project.key);
        let need_new = doc_key.is_err();
        let live = if need_new { check_new_key(cx.conn, &p.new_key) } else { Ok(String::new()) };
        let mut go = false;
        let mut close = false;
        let m = egui::Modal::new(Self::modal_id("backup_import")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 520.0));
            widgets::dialog_title(ui, lang.t("Projekt importieren", "Import project"));
            ui.label(RichText::new(&p.file).monospace());
            ui.add_space(S2);
            ui.label(RichText::new(format!("{} ({})", p.preview.name, p.preview.key)).strong());
            if !p.preview.exported_at.is_empty() {
                let when = p.preview.exported_at.replace('T', " ");
                hint(
                    ui,
                    &match lang {
                        Lang::De => format!("Exportiert am {when} mit Paneldeck {}", p.preview.app_version),
                        Lang::En => format!("Exported on {when} with Paneldeck {}", p.preview.app_version),
                    },
                );
            }
            ui.add_space(S1);
            ui.label(counts_line(lang, &p.preview));
            ui.add_space(S1);
            hint(ui, lang.t("Der Import legt ein neues Projekt an. Bestehende Projekte und Tickets werden nicht verändert.", "The import creates a new project. Existing projects and tickets are not changed."));
            if need_new {
                ui.add_space(S2);
                let used = match &doc_key {
                    Err(BackupError::KeyTaken(k)) => k.clone(),
                    _ => p.preview.key.clone(),
                };
                ui.label(RichText::new(match lang {
                    Lang::De => format!("Das Projektkürzel {used} ist nicht verfügbar. Neues Kürzel:"),
                    Lang::En => format!("The project key {used} is not available. New key:"),
                }).strong());
                let r = ui.add(egui::TextEdit::singleline(&mut p.new_key).desired_width(120.0).char_limit(10).margin(egui::Margin::symmetric(8, 5)));
                if r.changed() {
                    p.new_key = p.new_key.to_ascii_uppercase();
                }
                match &live {
                    Ok(_) => hint(
                        ui,
                        lang.t(
                            "Frei. Die Tickets werden von 1 an neu nummeriert; Ticketschlüssel in Texten (z. B. in Kommentaren) bleiben unverändert.",
                            "Free. The tickets are renumbered from 1; ticket keys inside texts (e.g. in comments) stay unchanged.",
                        ),
                    ),
                    Err(e) if p.new_key.is_empty() => {
                        let _ = e;
                        hint(ui, lang.t("Das Kürzel (2–10 Zeichen) steht vor jeder Ticketnummer, z. B. PAN-12.", "The key (2–10 characters) prefixes every ticket number, e.g. PAN-12."));
                    }
                    Err(e) => widgets::error(ui, &e.message(lang)),
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && live.is_ok() {
                    go = true;
                }
            }
            if let Some(e) = &p.error {
                ui.add_space(S2);
                egui::ScrollArea::vertical().max_height(120.0).id_salt("backup_import_err").show(ui, |ui| widgets::error(ui, &e.message(lang)));
            }
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if widgets::primary_enabled(ui, live.is_ok(), lang.t("Importieren", "Import")).clicked() {
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
        if go && live.is_ok() && !close {
            // der Import läuft im Hintergrund; das Ergebnis (oder der Fehler) kommt über `backup_poll` zurück in den Dialog
            p.error = None;
            let key = if need_new { Some(p.new_key.clone()) } else { None };
            self.begin_project_import(ctx, cx, p, key);
            return;
        }
        if !close {
            self.templates.backup.project = Some(p);
        }
    }

    fn project_report_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        let Some(res) = self.templates.backup.project_report.take() else { return };
        let mut close = false;
        let r = &res.report;
        let m = egui::Modal::new(Self::modal_id("backup_report")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 520.0));
            widgets::dialog_title(ui, lang.t("Import abgeschlossen", "Import finished"));
            ui.label(match lang {
                Lang::De => format!("Neues Projekt {} angelegt.", res.key),
                Lang::En => format!("New project {} created.", res.key),
            });
            ui.add_space(S1);
            ui.label(match lang {
                Lang::De => format!(
                    "{} Ticket(s), {} Kommentar(e), {} Verknüpfung(en), {} Zeiteintrag/-einträge, {} Verlaufseinträge · {} Sprint(s), {} Version(en), {} Meilenstein(e) · {} Typ(en), {} Feld(er), {} Status, {} Board(s), {} Filter, {} Quick-Action(s)",
                    r.tickets, r.comments, r.links, r.time_entries, r.history, r.sprints, r.versions, r.milestones, r.types, r.fields, r.statuses, r.boards, r.filters, r.quick_actions
                ),
                Lang::En => format!(
                    "{} ticket(s), {} comment(s), {} link(s), {} time entr(ies), {} history entries · {} sprint(s), {} version(s), {} milestone(s) · {} type(s), {} field(s), {} status(es), {} board(s), {} filter(s), {} quick action(s)",
                    r.tickets, r.comments, r.links, r.time_entries, r.history, r.sprints, r.versions, r.milestones, r.types, r.fields, r.statuses, r.boards, r.filters, r.quick_actions
                ),
            });
            ui.add_space(S2);
            if r.warnings.is_empty() {
                ui.label(RichText::new(lang.t("Keine Hinweise.", "No notes.")).color(style::dim(ui)));
            } else {
                ui.label(RichText::new(lang.t("Hinweise:", "Notes:")).strong());
                egui::ScrollArea::vertical().max_height(220.0).id_salt("backup_warnings").show(ui, |ui| {
                    for w in &r.warnings {
                        ui.label(format!("• {}", w.message(lang)));
                    }
                });
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
            self.templates.backup.project_report = Some(res);
        }
    }

    fn csv_import_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        let Some(mut p) = self.templates.backup.csv.take() else { return };
        let mut go = false;
        let mut close = false;
        let m = egui::Modal::new(Self::modal_id("csv_import")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 560.0));
            widgets::dialog_title(ui, lang.t("Tickets importieren (CSV)", "Import tickets (CSV)"));
            ui.label(RichText::new(&p.file).monospace());
            ui.add_space(S2);
            ui.label(match lang {
                Lang::De => format!("Die Datei enthält {} Zeile(n). Sie werden als neue Tickets im Projekt {} angelegt.", p.rows, p.project_key),
                Lang::En => format!("The file contains {} row(s). They are created as new tickets in the project {}.", p.rows, p.project_key),
            });
            hint(
                ui,
                lang.t(
                    "Alles oder nichts: Gibt es Fehler, wird nichts angelegt. Bestehende Tickets werden nie verändert.",
                    "All or nothing: if there are errors, nothing is created. Existing tickets are never changed.",
                ),
            );
            if let Some(e) = &p.error {
                ui.add_space(S2);
                ui.label(RichText::new(lang.t("Der Import ist nicht möglich:", "The import is not possible:")).strong());
                egui::ScrollArea::vertical().max_height(200.0).id_salt("csv_import_err").show(ui, |ui| match e {
                    BackupError::CsvRows(rows) => {
                        for r in rows {
                            widgets::error(ui, &r.message(lang));
                        }
                        if rows.len() >= crate::planning::backup_doc::error::MAX_ROW_PROBLEMS {
                            hint(ui, lang.t("… weitere Probleme werden nicht angezeigt.", "… further problems are not shown."));
                        }
                    }
                    other => widgets::error(ui, &other.message(lang)),
                });
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
        if go && !close {
            p.error = None;
            self.begin_csv_import(ctx, cx, p);
            return;
        }
        if !close {
            self.templates.backup.csv = Some(p);
        }
    }

    fn csv_report_dialog(&mut self, ctx: &egui::Context, cx: &Cx) {
        let lang = cx.lang;
        let Some((key, rep)) = self.templates.backup.csv_report.take() else { return };
        let mut close = false;
        let m = egui::Modal::new(Self::modal_id("csv_report")).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 520.0));
            widgets::dialog_title(ui, lang.t("Import abgeschlossen", "Import finished"));
            ui.label(match lang {
                Lang::De => format!("{} Ticket(s) im Projekt {key} angelegt.", rep.created),
                Lang::En => format!("{} ticket(s) created in project {key}.", rep.created),
            });
            ui.add_space(S2);
            if !rep.keys.is_empty() {
                ui.label(RichText::new(lang.t("Neue Schlüssel (Schlüssel in der Datei → neuer Schlüssel):", "New keys (key in the file → new key):")).strong());
                egui::ScrollArea::vertical().max_height(160.0).id_salt("csv_keys").show(ui, |ui| {
                    for (old, new) in rep.keys.iter().take(200) {
                        let old = if old.is_empty() { lang.t("(ohne)", "(none)") } else { old.as_str() };
                        ui.label(RichText::new(format!("{old} → {new}")).monospace());
                    }
                    if rep.keys.len() > 200 {
                        hint(ui, &match lang {
                            Lang::De => format!("… und {} weitere", rep.keys.len() - 200),
                            Lang::En => format!("… and {} more", rep.keys.len() - 200),
                        });
                    }
                });
            }
            for w in &rep.warnings {
                ui.add_space(S1);
                ui.label(format!("• {}", w.message(lang)));
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
            self.templates.backup.csv_report = Some((key, rep));
        }
    }
}
