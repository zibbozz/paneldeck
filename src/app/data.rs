//! Datenbank-Sicherung in der Oberfläche: Zeitplan (Nachholen beim Start), Einstellungs-Tab „Daten & Sicherung“, Wiederherstellen.

use super::*;
use crate::backup::{self, Kind};

impl App {
    pub(super) fn backup_dir_path(&self) -> PathBuf {
        // `--backup-dir` gilt nur für diesen Start und landet nie in den Einstellungen.
        if let Some(b) = &crate::cli::get().backup_dir {
            return b.clone();
        }
        backup::dir(&self.dir, &self.settings.backup_dir)
    }

    /// Sicherung im Hintergrund starten (nur eine gleichzeitig).
    fn start_backup(&mut self, kind: Kind) {
        if self.backup_rx.is_some() {
            return;
        }
        let Some(db) = &self.db else { return };
        self.backup_rx = Some(backup::spawn(db.path.clone(), self.backup_dir_path(), kind, self.settings.backup_keep as usize));
    }

    /// Pro Frame: Ergebnis einer laufenden Sicherung abholen; einmal pro Minute prüfen, ob eine automatische fällig ist
    /// (beim Start sofort – eine verpasste Sicherung wird dann nachgeholt, mehrere verpasste ergeben nur eine).
    pub(super) fn backup_tick(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        if let Some(rx) = &self.backup_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.backup_rx = None;
                    self.backups_dirty = true;
                    self.backup_msg = Some(match r {
                        Ok(b) => format!("{} {}", lang.t("Sicherung angelegt:", "Backup created:"), b.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                        Err(e) => format!("{}: {e}", lang.t("Sicherung fehlgeschlagen", "Backup failed")),
                    });
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(200));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.backup_rx = None,
            }
        }
        if self.backup_checked.elapsed() < Duration::from_secs(60) {
            return;
        }
        self.backup_checked = Instant::now();
        let Some(db) = &self.db else { return };
        let reference = backup::last_auto(&self.backup_dir_path()).or_else(|| db.created_at());
        if backup::due(reference, self.settings.backup_interval_hours, chrono::Local::now().naive_local()) {
            self.start_backup(Kind::Auto);
        }
    }

    fn fmt_time(&self, t: chrono::NaiveDateTime) -> String {
        t.format(if self.settings.language == Lang::De { "%d.%m.%Y %H:%M" } else { "%Y-%m-%d %H:%M" }).to_string()
    }

    fn fmt_size(bytes: u64) -> String {
        if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64) } else { format!("{} KB", (bytes + 1023) / 1024) }
    }

    /// *Einstellungen → Daten & Sicherung → Sicherung*.
    pub(super) fn data_backup_page(&mut self, ui: &mut Ui, cx: &super::settings_items::RowCx) {
        use super::settings_items::{note, srow};
        let lang = cx.lang;
        if let Some(e) = &self.db_error {
            ui.colored_label(Color32::LIGHT_RED, e);
            ui.add_space(S2);
        }
        let top = ui.cursor().min.y;
        note(ui, cx, lang.t(
            "Paneldeck legt automatisch Kopien der Datenbank an. War der Rechner zur fälligen Zeit aus, wird die Sicherung beim nächsten Start nachgeholt (mehrere verpasste ergeben nur eine).",
            "Paneldeck makes copies of the database automatically. If the computer was off when one was due, it is made at the next start (several missed ones result in just one).",
        ));
        srow(ui, cx, "backup_interval", |ui| {
            ui.add(egui::DragValue::new(&mut self.settings.backup_interval_hours).range(0..=720).suffix(" h"));
        });
        srow(ui, cx, "backup_keep", |ui| {
            ui.add(egui::DragValue::new(&mut self.settings.backup_keep).range(1..=50));
        });
        let dir = self.backup_dir_path();
        let fixed = crate::cli::get().backup_dir.is_some();
        let hint = if fixed { format!("{}  {}", dir.display(), lang.t("(festgelegt mit --backup-dir)", "(set by --backup-dir)")) } else { dir.display().to_string() };
        super::settings_items::srow_hint(ui, cx, "backup_dir", Some(&hint), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!fixed, egui::Button::new(lang.t("Ordner wählen …", "Choose folder …"))).clicked() {
                    if let Some(p) = notify::pick_folder(&dir.display().to_string()) {
                        self.settings.backup_dir = p;
                        self.backups_dirty = true;
                    }
                }
                if ui.add_enabled(!fixed && !self.settings.backup_dir.is_empty(), egui::Button::new(lang.t("Standardordner", "Default folder"))).clicked() {
                    self.settings.backup_dir.clear();
                    self.backups_dirty = true;
                }
                if ui.button(lang.t("Ordner öffnen", "Open folder")).clicked() {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                }
                let busy = self.backup_rx.is_some();
                if ui.add_enabled(self.db.is_some() && !busy, egui::Button::new(lang.t("Jetzt sichern", "Back up now"))).clicked() {
                    self.start_backup(Kind::Manual);
                }
            });
        });
        if let Some(m) = &self.backup_msg {
            note(ui, cx, m);
        }
        tutorial::spot_since(ui, Spot::DataBackups, top);
        super::settings_items::link(ui, cx, lang.t("Vorhandene Sicherungen wiederherstellen:", "Restore existing backups:"), super::settings_nav::Page::DataRestore);
    }

    /// *Einstellungen → Daten & Sicherung → Wiederherstellen*: vorhandene Sicherungen.
    pub(super) fn data_restore_page(&mut self, ui: &mut Ui, cx: &super::settings_items::RowCx) {
        let lang = cx.lang;
        let dim = style::dim(ui);
        let dir = self.backup_dir_path();
        if self.backups_dirty || self.backups_cache.is_none() {
            self.backups_cache = Some(backup::list(&dir));
            self.backups_dirty = false;
        }
        let list = self.backups_cache.clone().unwrap_or_default();
        super::settings_items::block(ui, cx, "backups_list", true, |ui| {
            if list.is_empty() {
                ui.label(RichText::new(lang.t("Noch keine Sicherungen.", "No backups yet.")).small().color(dim));
            }
            egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() - 220.0).max(160.0)).id_salt("backups").show(ui, |ui| {
                for b in &list {
                    ui.horizontal(|ui| {
                        ui.label(self.fmt_time(b.time));
                        ui.label(RichText::new(b.kind.label(lang)).small().color(dim));
                        ui.label(RichText::new(Self::fmt_size(b.size)).small().color(dim));
                        if ui.small_button(lang.t("Wiederherstellen …", "Restore …")).clicked() {
                            self.confirm_restore = Some(b.path.clone());
                        }
                    });
                }
            });
        });
    }

    /// *Einstellungen → Daten & Sicherung → Pflege & Aufräumen*: Weg zur Datenbankpflege, Aufräumen, Rückgängig-Schritte.
    pub(super) fn data_maintenance_page(&mut self, ui: &mut Ui, cx: &super::settings_items::RowCx) {
        use super::settings_items::{block, srow};
        let lang = cx.lang;
        // Datenbankpflege (#308) gibt es nur auf der Wartungsseite; hier nur der Weg dorthin.
        block(ui, cx, "db_care", true, |ui| {
            if ui.add_enabled(self.db.is_some(), egui::Button::new(lang.t("Wartungsseite öffnen", "Open the maintenance page"))).clicked() {
                self.plan.request_maintenance_db();
                self.set_mode(titlebar::Mode::Tasks);
                self.settings_open = false;
            }
        });
        block(ui, cx, "cleanup", true, |ui| {
            if ui.button(lang.t("Aufräumen …", "Clean up …")).spot(Spot::DataCleanup).clicked() {
                let ctx = ui.ctx().clone();
                self.open_cleanup(&ctx);
            }
        });
        srow(ui, cx, "undo_steps", |ui| {
            egui::ComboBox::from_id_salt("undo_steps").selected_text(self.settings.undo_steps.to_string()).width(80.0).show_ui(ui, |ui| {
                for n in crate::state::UNDO_STEP_CHOICES {
                    ui.selectable_value(&mut self.settings.undo_steps, n, n.to_string());
                }
            });
        });
    }

    /// Sicherheitsabfrage für das Wiederherstellen.
    pub(super) fn restore_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self.confirm_restore.clone() else { return };
        let lang = self.settings.language;
        let mut close = false;
        let mut go = false;
        style::movable(ctx, Id::new("restore_dialog"), egui::Window::new(lang.t("Sicherung wiederherstellen", "Restore backup")))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                ui.add_space(S2);
                ui.label(lang.t(
                    "Der aktuelle Stand wird zuerst als Sicherung abgelegt. Danach beendet sich Paneldeck; die Wiederherstellung wird beim nächsten Start eingespielt.",
                    "The current data is saved as a backup first. Paneldeck then quits; the restore is applied at the next start.",
                ));
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    if ui.button(lang.t("Wiederherstellen und beenden", "Restore and quit")).clicked() {
                        go = true;
                    }
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        close = true;
                    }
                });
                if let Some(e) = &self.restore_error {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
            });
        if go {
            match backup::schedule_restore(&self.dir, &path) {
                // Speichern geht nur mit dem Hauptfenster in den Feldern: aus einem weiteren Fenster erst nach dessen Bild (#110).
                Ok(()) if self.wins.cur != 0 => self.wins.exit_after_save = true,
                Ok(()) => {
                    self.save_for_exit();
                    std::process::exit(0);
                }
                Err(e) => self.restore_error = Some(format!("{}: {e}", lang.t("Sicherung ist nicht lesbar", "Backup is not readable"))),
            }
        }
        if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.confirm_restore = None;
            self.restore_error = None;
        }
    }
}
