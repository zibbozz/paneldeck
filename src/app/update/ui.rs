//! Oberfläche des Updates: Knopf in der Fensterleiste (links neben Minimieren), Update-Dialog, Neustart-Abfrage und der Abschnitt
//! „Updates“ unter *Einstellungen → Info*. Eingebunden mit je einer Zeile in `titlebar.rs`, `settings.rs` und `App::update`.

use super::release::{Flavor, NoInstall, Plan};
use super::version::Version;
use super::{install, Phase};
use crate::app::App;
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2};
use eframe::egui::{self, Align2, Color32, FontId, Id, Pos2, Rect, RichText, Sense, Vec2};
use std::time::Duration;

/// Adresse im Browser öffnen – nur Release-Seiten dieses Repositorys.
fn open_release_page(url: &str) {
    if super::release::safe_release_page(url) {
        let _ = std::process::Command::new("explorer").arg(url).spawn();
    }
}

fn fmt_mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
}

impl App {
    fn running_sessions(&self) -> usize {
        self.entries.values().filter(|e| !e.exited()).count()
    }

    fn update_backup_target(&self) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let db = self.db.as_ref()?;
        Some((db.path.clone(), crate::backup::dir(&self.dir, &self.settings.backup_dir)))
    }

    fn update_check_now(&mut self, manual: bool, ctx: &egui::Context) {
        let skip = self.settings.update_skip_version.clone();
        self.updater.start_check(manual, self.exe.clone(), skip, self.settings.language, ctx);
    }

    fn update_install_now(&mut self, ctx: &egui::Context) {
        self.updater.confirm_restart = false;
        let backup = self.update_backup_target();
        self.updater.start_install(self.exe.clone(), backup, self.settings.language, ctx);
    }

    /// Zustand speichern (Sitzungen werden beim Start wiederhergestellt) und neu starten.
    fn update_restart(&mut self) {
        let lang = self.settings.language;
        self.save_all();
        match install::spawn_restart(&self.exe) {
            Ok(()) => {
                crate::app::winchrome::shutdown();
                std::process::exit(0);
            }
            Err(e) => {
                self.updater.phase = Phase::Idle;
                self.updater.offer = None;
                self.updater.error = Some(format!(
                    "{}: {e}",
                    lang.t("Das Update ist installiert, aber der Neustart schlug fehl – bitte Paneldeck selbst neu starten", "The update is installed, but restarting failed – please restart Paneldeck yourself")
                ));
                self.updater.dialog = true;
            }
        }
    }

    /// Pro Frame aus `App::update`: Ergebnisse abholen, automatische Prüfung, Dialoge.
    pub(crate) fn update_frame(&mut self, ctx: &egui::Context) {
        if self.updater.poll(&mut self.settings) {
            self.update_restart();
        }
        if self.updater.auto_tick(&self.settings) {
            self.update_check_now(false, ctx);
        }
        if matches!(self.updater.phase, Phase::Downloading | Phase::Installing | Phase::Checking { .. }) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.updater.confirm_restart && self.updater.phase == Phase::Ready {
            if self.running_sessions() == 0 {
                self.update_install_now(ctx);
            } else {
                self.update_confirm_ui(ctx);
            }
        }
        if self.updater.dialog {
            self.update_dialog_ui(ctx);
        }
    }

    /// Knopf in der Fensterleiste, rechtsbündig direkt links neben Minimieren (`right` = linke Kante des Minimieren-Knopfs).
    /// Nur sichtbar, wenn ein Update verfügbar ist oder gerade läuft (dann mit Fortschritt).
    pub(crate) fn update_title_button(&mut self, ui: &egui::Ui, right: f32) {
        if !self.updater.button_visible() {
            return;
        }
        let lang = self.settings.language;
        let Some(version) = self.updater.offer.as_ref().map(|o| o.version.to_string()) else { return };
        let (done, total) = self.updater.progress();
        let frac = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
        let (label, tip) = match self.updater.phase {
            Phase::Downloading => (format!("{:.0} %", frac * 100.0), format!("{} {version} …", lang.t("Update wird geladen:", "Downloading update"))),
            Phase::Ready => (lang.t("Neu starten", "Restart").to_string(), format!("{} {version} {}", lang.t("Update", "Update"), lang.t("ist bereit – klicken zum Neustarten", "is ready – click to restart"))),
            Phase::Installing => ("…".to_string(), lang.t("Update wird installiert …", "Installing update …").to_string()),
            _ => (version.clone(), format!("{} {version} {}", lang.t("Version", "Version"), lang.t("ist verfügbar", "is available"))),
        };
        let painter = ui.painter();
        let font = FontId::proportional(12.5);
        let fg = ui.visuals().text_color();
        let g = painter.layout_no_wrap(label, font, fg);
        let h = super::super::titlebar::BAR_HEIGHT - 8.0;
        let w = g.size().x + 36.0;
        let top = ui.max_rect().top() + 4.0;
        let r = Rect::from_min_size(Pos2::new(right - 6.0 - w, top), Vec2::new(w, h));
        let resp = ui.interact(r, Id::new("titlebar_update"), Sense::click()).on_hover_text(tip);
        // Wie ein aktiver Reiter (Akzentfläche), beim Überfahren etwas kräftiger
        painter.rect_filled(r, 6.0, style::accent().gamma_multiply(if resp.hovered() { 0.34 } else { 0.22 }));
        if self.updater.phase == Phase::Downloading {
            let pr = Rect::from_min_size(r.min, Vec2::new(r.width() * frac, r.height()));
            painter.rect_filled(pr, 6.0, style::accent().gamma_multiply(0.35));
        }
        style::paint_icon(painter, SIcon::Down, Pos2::new(r.left() + 14.0, r.center().y), style::accent());
        painter.galley(Pos2::new(r.left() + 26.0, r.center().y - g.size().y / 2.0), g, fg);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            if self.updater.phase == Phase::Ready {
                self.updater.confirm_restart = true;
            } else {
                self.updater.dialog = true;
            }
        }
    }

    fn update_dialog_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let Some(offer) = self.updater.offer.clone() else {
            // Kein Angebot (mehr), aber eine Meldung (z. B. Neustart fehlgeschlagen)
            if let Some(e) = self.updater.error.clone() {
                let mut open = true;
                egui::Window::new(lang.t("Update", "Update")).id(Id::new("update_dialog")).open(&mut open).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.colored_label(Color32::LIGHT_RED, e);
                });
                if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.updater.dialog = false;
                    self.updater.error = None;
                }
            } else {
                self.updater.dialog = false;
            }
            return;
        };
        let cur = Version::current();
        let flavor = self.updater.flavor(&self.exe);
        let phase = self.updater.phase;
        let mut open = true;
        let (mut go, mut later, mut skip, mut cancel) = (false, false, false, false);
        egui::Window::new(lang.t("Update verfügbar", "Update available"))
            .id(Id::new("update_dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                let dim = style::dim(ui);
                ui.label(RichText::new(format!("Paneldeck {}", offer.version)).heading());
                ui.label(RichText::new(format!("{} {cur}", lang.t("Installiert:", "Installed:"))).color(dim));
                ui.add_space(S2);
                if offer.major {
                    egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(0xe0, 0x6c, 0x00, 40)).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                        ui.label(RichText::new(format!("{} ({}.x → {})", lang.t("Neue Hauptversion", "New major version"), cur.major, offer.version)).strong().color(Color32::from_rgb(0xff, 0xa0, 0x40)));
                        ui.label(lang.t(
                            "Eine Hauptversion kann die Bedienung oder Datenformate grundlegend ändern. Vor dem Update wird automatisch eine Sicherung der Datenbank angelegt.",
                            "A major version can change how Paneldeck works or its data formats. A database backup is made automatically before updating.",
                        ));
                    });
                    ui.add_space(S2);
                }
                if super::release::safe_release_page(&offer.html_url) && ui.link(lang.t("Was ist neu? (Release-Seite auf GitHub)", "What's new? (release page on GitHub)")).clicked() {
                    open_release_page(&offer.html_url);
                }
                ui.add_space(S2);
                let installable = matches!(offer.plan, Plan::Install { .. });
                match &offer.plan {
                    Plan::AnnounceOnly(why) => {
                        ui.label(RichText::new(match why {
                            NoInstall::NoChecksums => lang.t(
                                "Diese Version lässt sich nicht automatisch installieren (das Release hat keine Prüfsummen-Datei). Bitte von der Release-Seite herunterladen.",
                                "This version can't be installed automatically (the release has no checksum file). Please download it from the release page.",
                            ),
                            NoInstall::NoAsset => lang.t(
                                "Das Release enthält keine passende Datei für diese Installation. Bitte von der Release-Seite herunterladen.",
                                "The release has no matching file for this installation. Please download it from the release page.",
                            ),
                            NoInstall::Unsupported => lang.t("Automatische Updates gibt es nur unter Windows.", "Automatic updates are only available on Windows."),
                        }).color(dim));
                    }
                    Plan::Install { asset, .. } => {
                        ui.label(RichText::new(match flavor {
                            Some(Flavor::Installed) => lang.t(
                                "Installierte Version: Das Setup läuft still im selben Ordner, danach startet Paneldeck neu.",
                                "Installed copy: the setup runs silently in the same folder, then Paneldeck restarts.",
                            ),
                            _ => lang.t(
                                "Portable Version: paneldeck.exe wird ersetzt (die alte bleibt bis zum nächsten Start als paneldeck.exe.old liegen), danach startet Paneldeck neu.",
                                "Portable copy: paneldeck.exe is replaced (the old one stays as paneldeck.exe.old until the next start), then Paneldeck restarts.",
                            ),
                        }).small().color(dim));
                        ui.label(RichText::new(format!("{} ({}) · {}", asset.name, fmt_mb(asset.size), lang.t("geprüft per SHA-256", "verified with SHA-256"))).small().color(dim));
                        ui.label(RichText::new(lang.t(
                            "Laufende Terminals werden beim Neustart beendet; die Sitzungen werden danach wiederhergestellt.",
                            "Running terminals end when Paneldeck restarts; the sessions are restored afterwards.",
                        )).small().color(dim));
                    }
                }
                if phase == Phase::Downloading {
                    let (d, t) = self.updater.progress();
                    ui.add_space(S1);
                    ui.add(egui::ProgressBar::new(if t > 0 { d as f32 / t as f32 } else { 0.0 }).text(format!("{} / {}", fmt_mb(d), fmt_mb(t))));
                }
                if phase == Phase::Installing {
                    ui.add_space(S1);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(lang.t("Wird installiert …", "Installing …"));
                    });
                }
                if let Some(e) = &self.updater.error {
                    ui.add_space(S1);
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    match phase {
                        Phase::Downloading => {
                            if ui.button(lang.t("Download abbrechen", "Cancel download")).clicked() {
                                cancel = true;
                            }
                        }
                        Phase::Ready => {
                            if ui.button(RichText::new(lang.t("Neu starten und aktualisieren", "Restart and update")).strong()).clicked() {
                                go = true;
                            }
                        }
                        Phase::Installing => {}
                        _ => {
                            if ui.add_enabled(installable && phase == Phase::Idle, egui::Button::new(RichText::new(lang.t("Jetzt aktualisieren", "Update now")).strong())).clicked() {
                                go = true;
                            }
                        }
                    }
                    if phase != Phase::Installing {
                        if ui.button(lang.t("Später", "Later")).clicked() {
                            later = true;
                        }
                        if !matches!(phase, Phase::Downloading | Phase::Ready) && ui.button(lang.t("Diese Version überspringen", "Skip this version")).clicked() {
                            skip = true;
                        }
                    }
                });
            });
        if go {
            if phase == Phase::Ready {
                self.updater.confirm_restart = true;
            } else {
                self.updater.start_download(lang, ctx);
            }
            self.updater.dialog = false;
        }
        if cancel {
            self.updater.cancel_download();
        }
        if skip {
            self.settings.update_skip_version = offer.version.to_string();
            self.updater.offer = None;
            self.updater.dialog = false;
        }
        if later {
            if phase == Phase::Downloading {
                // „Später“ während des Downloads: weiterladen, nur den Dialog schließen.
                self.updater.dialog = false;
            } else {
                self.updater.later();
            }
        }
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.updater.dialog = false;
        }
    }

    /// Nachfrage vor dem Neustart, wenn Terminals laufen.
    fn update_confirm_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let n = self.running_sessions();
        let major = self.updater.offer.as_ref().is_some_and(|o| o.major);
        let (mut go, mut no) = (false, false);
        egui::Window::new(lang.t("Paneldeck neu starten?", "Restart Paneldeck?")).id(Id::new("update_confirm")).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let running = if n == 1 {
                lang.t("1 Terminal läuft noch.", "1 terminal is still running.").to_string()
            } else {
                format!("{n} {}", lang.t("Terminals laufen noch.", "terminals are still running."))
            };
            ui.label(running);
            ui.label(lang.t(
                "Für das Update startet Paneldeck neu: Laufende Prozesse (auch Claude-Sitzungen) werden beendet, die Sitzungen werden danach wiederhergestellt.",
                "To finish the update Paneldeck restarts: running processes (including Claude sessions) end, the sessions are restored afterwards.",
            ));
            if major {
                ui.label(RichText::new(lang.t("Vorher wird die Datenbank gesichert.", "The database is backed up first.")).small().color(style::dim(ui)));
            }
            ui.add_space(S2);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(lang.t("Neu starten und aktualisieren", "Restart and update")).strong()).clicked() {
                    go = true;
                }
                if ui.button(lang.t("Nicht jetzt", "Not now")).clicked() {
                    no = true;
                }
            });
        });
        if go {
            self.update_install_now(ctx);
        }
        if no || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.updater.confirm_restart = false;
        }
    }

    /// Abschnitt „Updates“ unter *Einstellungen → Info*.
    pub(crate) fn update_about_ui(&mut self, ui: &mut egui::Ui) {
        let lang = self.settings.language;
        let ctx = ui.ctx().clone();
        let dim = style::dim(ui);
        ui.add_space(S2);
        ui.separator();
        ui.add_space(S1);
        ui.label(RichText::new(lang.t("Updates", "Updates")).strong());
        let flavor = self.updater.flavor(&self.exe);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}:", lang.t("Version", "Version"))).color(dim));
            ui.label(RichText::new(Version::current().to_string()).small());
            ui.label(RichText::new(match flavor {
                Some(Flavor::Installed) => lang.t("(installiert)", "(installed)"),
                Some(Flavor::Portable) => lang.t("(portabel)", "(portable)"),
                None => "",
            }).small().color(dim));
        });
        ui.horizontal_wrapped(|ui| {
            let checking = matches!(self.updater.phase, Phase::Checking { .. });
            if ui.add_enabled(!self.updater.busy(), egui::Button::new(lang.t("Nach Updates suchen", "Check for updates"))).clicked() {
                self.update_check_now(true, &ctx);
            }
            if checking {
                ui.spinner();
            }
            if let Some((text, ok)) = &self.updater.result {
                ui.label(RichText::new(text).small().color(if *ok { dim } else { Color32::LIGHT_RED }));
            }
            if self.updater.offer.is_some() && !self.updater.dialog && ui.small_button(lang.t("Details …", "Details …")).clicked() {
                self.updater.dialog = true;
            }
        });
        ui.horizontal(|ui| {
            style::toggle(ui, &mut self.settings.update_auto_check);
            ui.label(lang.t("Automatisch prüfen (beim Start, höchstens einmal am Tag)", "Check automatically (at start, at most once a day)"));
        });
        let last = if self.settings.update_last_check > 0 {
            chrono::DateTime::from_timestamp(self.settings.update_last_check, 0)
                .map(|t| t.with_timezone(&chrono::Local).format(if lang == Lang::De { "%d.%m.%Y %H:%M" } else { "%Y-%m-%d %H:%M" }).to_string())
                .unwrap_or_default()
        } else {
            lang.t("noch nie", "never").to_string()
        };
        ui.label(RichText::new(format!("{} {last}", lang.t("Letzte Prüfung:", "Last check:"))).small().color(dim));
        if !self.settings.update_skip_version.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} {}", lang.t("Übersprungene Version:", "Skipped version:"), self.settings.update_skip_version)).small().color(dim));
                if ui.small_button(lang.t("Zurücksetzen", "Reset")).clicked() {
                    self.settings.update_skip_version.clear();
                }
            });
        }
        ui.label(RichText::new(lang.t(
            "Die Prüfung fragt nur die öffentliche GitHub-API nach dem neuesten Release; es werden keine Daten über dich gesendet.",
            "The check only asks the public GitHub API for the latest release; no data about you is sent.",
        )).small().color(dim));
    }
}
