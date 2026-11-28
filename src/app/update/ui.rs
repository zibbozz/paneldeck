//! Oberfläche des Updates: Knopf in der Fensterleiste (links neben Minimieren), Update-Dialog, Neustart-Abfrage und der Abschnitt
//! „Updates“ unter *Einstellungen → Info → Updates*. Eingebunden mit je einer Zeile in `titlebar.rs`, `settings.rs` und `App::update`.

use super::release::{Flavor, NoInstall, Plan};
use super::version::Version;
use super::{install, Phase};
use crate::app::App;
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2};
use eframe::egui::{self, Color32, FontId, Id, Pos2, Rect, RichText, Sense, Vec2};
use std::time::Duration;

/// Release-Seite der Version im Standardbrowser öffnen. Die Adresse wird lokal aus der geprüften Versionsnummer gebaut (nichts
/// aus der API-Antwort) und per ShellExecuteW geöffnet (kein Umweg über `explorer`).
fn open_release_page(v: &Version) {
    let url = super::release::release_page(v);
    if !super::release::safe_release_page(&url) {
        return;
    }
    crate::platform::open::url(&url);
}

/// Esc schließt ein Update-Fenster nur, wenn es vorn liegt und weder ein Terminal noch ein Textfeld die Tastatur hat (sonst
/// gehört Esc dorthin, z. B. beim Tippen in ein Terminal, während der Dialog offen ist).
fn esc_for(ctx: &egui::Context, layer: Option<egui::LayerId>) -> bool {
    let Some(layer) = layer else { return false };
    // Nicht verschachteln: ctx.input() hält die Sperre des Kontexts.
    let front = ctx.top_layer_id() == Some(layer);
    let typing = ctx.wants_keyboard_input();
    front && !typing && ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

fn fmt_mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
}

impl App {
    fn running_sessions(&self) -> usize {
        // Chats zählen nur, solange eine Antwort läuft.
        self.entries.values().filter(|e| e.running()).count()
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

    /// Zustand speichern (Sitzungen werden beim Start wiederhergestellt – mit „Sitzungen merken“ aus genau einmal, #157) und
    /// neu starten. Scheitert der Neustart, geht nichts verloren (es wurde nur normal gespeichert).
    fn update_restart(&mut self) {
        let lang = self.settings.language;
        let keep_once = self.save_for_update_restart();
        match install::spawn_restart(&self.exe, keep_once) {
            Ok(()) => {
                crate::platform::chrome::shutdown();
                self.tray = None; // Tray-Symbol entfernen (sonst bleibt bis zum Überfahren ein totes Symbol stehen)
                std::process::exit(0);
            }
            Err(e) => {
                // Mit „Sitzungen merken“ aus stand für den Neustart der volle Zustand auf der Platte; er läuft hier weiter wie
                // bisher, also wieder den zugeschnittenen schreiben – sonst sicherte ein späterer Absturz die Wegwerf-Sitzungen (#219).
                self.save_state();
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
        if self.updater.first_frame_done() {
            // Die neue Version läuft und zeichnet: die alte Programmdatei wird nicht mehr gebraucht.
            install::remove_old_exe(&self.exe);
        }
        if self.updater.poll(&mut self.settings) {
            self.update_restart();
        }
        if self.updater.auto_tick(&self.settings) {
            self.update_check_now(false, ctx);
        }
        if matches!(self.updater.phase, Phase::Downloading | Phase::Installing | Phase::Checking { .. }) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // Nie still neu starten (offene Eingaben gingen verloren): immer nachfragen, auch ohne laufende Terminals.
        if self.updater.confirm_restart && self.updater.phase == Phase::Ready {
            self.update_confirm_ui(ctx);
        }
        if self.updater.dialog {
            self.update_dialog_ui(ctx);
        }
    }

    /// Knopf in der Fensterleiste, links neben „Einstellungen“ (`right` = rechte Kante, #240). Nur sichtbar, wenn ein Update
    /// verfügbar ist oder gerade läuft (dann mit Fortschritt). Liefert die rechte Kante für den nächsten Knopf links davon.
    pub(crate) fn update_title_button(&mut self, ui: &egui::Ui, right: f32) -> f32 {
        if !self.updater.button_visible() {
            return right;
        }
        let lang = self.settings.language;
        let Some(version) = self.updater.offer.as_ref().map(|o| o.version.to_string()) else { return right };
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
        let r = Rect::from_min_size(Pos2::new(right - w, top), Vec2::new(w, h));
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
        r.left() - 4.0
    }

    fn update_dialog_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let Some(offer) = self.updater.offer.clone() else {
            // Kein Angebot (mehr), aber eine Meldung (z. B. Neustart fehlgeschlagen)
            if let Some(e) = self.updater.error.clone() {
                let mut open = true;
                let r = style::movable(ctx, Id::new("update_dialog"), egui::Window::new(lang.t("Update", "Update"))).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.colored_label(Color32::LIGHT_RED, e);
                });
                if !open || esc_for(ctx, r.map(|r| r.response.layer_id)) {
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
        let shown = style::movable(ctx, Id::new("update_dialog"), egui::Window::new(lang.t("Update verfügbar", "Update available")))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
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
                if ui.link(lang.t("Was ist neu? (Release-Seite auf GitHub)", "What's new? (release page on GitHub)")).clicked() {
                    open_release_page(&offer.version);
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
                            NoInstall::Unsupported if cfg!(target_os = "macos") => lang.t(
                                "Paneldeck.app liegt nicht in einem Ordner, in den du schreiben darfst (oder läuft direkt aus dem Disk-Image) – bitte die neue Version von der Release-Seite laden und nach „Programme“ ziehen.",
                                "Paneldeck.app is not in a folder you can write to (or runs straight from the disk image) – please download the new version from the release page and drag it to Applications.",
                            ),
                            NoInstall::Unsupported => lang.t(
                                "Automatische Updates gibt es nur unter Windows und für die Linux-AppImage – bitte über die Paketverwaltung bzw. von der Release-Seite aktualisieren.",
                                "Automatic updates are only available on Windows and for the Linux AppImage – please update through your package manager or from the release page.",
                            ),
                        }).color(dim));
                    }
                    Plan::Install { asset, .. } => {
                        ui.label(RichText::new(match flavor {
                            Some(Flavor::Installed) => lang.t(
                                "Installierte Version: Das Setup läuft still im selben Ordner, danach startet Paneldeck neu.",
                                "Installed copy: the setup runs silently in the same folder, then Paneldeck restarts.",
                            ),
                            Some(Flavor::MacApp) => lang.t(
                                "macOS: Paneldeck.app wird komplett ersetzt (die alte bleibt versteckt als .Paneldeck.app.old liegen, bis die neue Version gestartet ist), danach startet Paneldeck neu. Die SHA-256-Prüfung schützt vor Übertragungsfehlern, nicht vor einem manipulierten Release (die App ist nicht signiert).",
                                "macOS: Paneldeck.app is replaced as a whole (the old one stays hidden as .Paneldeck.app.old until the new version has started), then Paneldeck restarts. The SHA-256 check protects against transfer errors, not against a tampered release (the app is not signed).",
                            ),
                            Some(Flavor::AppImage) => lang.t(
                                "AppImage: Die Datei wird ersetzt (die alte bleibt als .old liegen, bis die neue Version gestartet ist), danach startet Paneldeck neu. Die SHA-256-Prüfung schützt vor Übertragungsfehlern, nicht vor einem manipulierten Release.",
                                "AppImage: the file is replaced (the old one stays as .old until the new version has started), then Paneldeck restarts. The SHA-256 check protects against transfer errors, not against a tampered release.",
                            ),
                            _ => lang.t(
                                "Portable Version: paneldeck.exe wird ersetzt (die alte bleibt als paneldeck.exe.old liegen, bis die neue Version gestartet ist), danach startet Paneldeck neu.",
                                "Portable copy: paneldeck.exe is replaced (the old one stays as paneldeck.exe.old until the new version has started), then Paneldeck restarts.",
                            ),
                        }).small().color(dim));
                        ui.label(RichText::new(format!("{} ({}) · {}", asset.name, fmt_mb(asset.size), lang.t("geprüft per SHA-256", "verified with SHA-256"))).small().color(dim));
                        ui.label(RichText::new(if self.settings.remember_sessions {
                            lang.t(
                                "Laufende Terminals werden beim Neustart beendet; die Sitzungen werden danach wiederhergestellt.",
                                "Running terminals end when Paneldeck restarts; the sessions are restored afterwards.",
                            )
                        } else {
                            lang.t(
                                "Laufende Terminals werden beim Neustart beendet; die Sitzungen werden danach einmal wiederhergestellt. „Sitzungen merken“ ist aus: Beim nächsten Beenden werden sie verworfen.",
                                "Running terminals end when Paneldeck restarts; the sessions are restored once afterwards. “Remember sessions” is off: they are discarded on the next exit.",
                            )
                        }).small().color(dim));
                    }
                }
                if phase == Phase::Downloading {
                    let (d, t) = self.updater.progress();
                    ui.add_space(S1);
                    let frac = if t > 0 { d as f32 / t as f32 } else { 0.0 };
                    let fill = crate::style::bar_fill(frac, ui.visuals().selection.bg_fill);
                    ui.add(egui::ProgressBar::new(frac).text(format!("{} / {}", fmt_mb(d), fmt_mb(t))).fill(fill));
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
                } else if let Some(i) = &self.updater.info {
                    ui.add_space(S1);
                    ui.label(RichText::new(i).color(dim));
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
            if phase != Phase::Ready {
                self.updater.start_download(lang, ctx);
            } else if self.running_sessions() == 0 {
                // Ausdrücklich „Neu starten und aktualisieren“ geklickt, kein Terminal läuft: das ist die Bestätigung.
                self.update_install_now(ctx);
            } else {
                self.updater.confirm_restart = true;
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
            // Während des Downloads: weiterladen, danach nur „Neu starten“ in der Fensterleiste.
            self.updater.later();
        }
        if !open || esc_for(ctx, shown.map(|r| r.response.layer_id)) {
            self.updater.dialog = false;
            self.updater.info = None;
        }
    }

    /// Nachfrage vor dem Neustart, wenn Terminals laufen.
    fn update_confirm_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let n = self.running_sessions();
        let major = self.updater.offer.as_ref().is_some_and(|o| o.major);
        let (mut go, mut no) = (false, false);
        let title = if n == 0 { lang.t("Jetzt neu starten, um zu aktualisieren?", "Restart now to update?") } else { lang.t("Paneldeck neu starten?", "Restart Paneldeck?") };
        let shown = style::movable(ctx, Id::new("update_confirm"), egui::Window::new(title)).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.set_max_width(420.0);
            if n == 0 {
                ui.label(lang.t(
                    "Das Update ist geladen und geprüft. Für die Installation startet Paneldeck neu; nicht gespeicherte Eingaben in offenen Dialogen gehen dabei verloren.",
                    "The update is downloaded and verified. Paneldeck restarts to install it; unsaved input in open dialogs is lost.",
                ));
            } else {
                // Getrennt zählen: laufende Terminals und Chats, die noch arbeiten (Antwort, Werkzeug, Freigabe-Frage).
                let chats = self.entries.values().filter(|e| e.is_chat() && e.running()).count();
                let terms = n - chats;
                if terms == 1 {
                    ui.label(lang.t("1 Terminal läuft noch.", "1 terminal is still running."));
                } else if terms > 1 {
                    ui.label(format!("{terms} {}", lang.t("Terminals laufen noch.", "terminals are still running.")));
                }
                if chats == 1 {
                    ui.label(lang.t("1 Chat arbeitet noch (er wird angehalten).", "1 chat is still working (it will be stopped)."));
                } else if chats > 1 {
                    ui.label(format!("{chats} {}", lang.t("Chats arbeiten noch (sie werden angehalten).", "chats are still working (they will be stopped).")));
                }
                ui.label(if self.settings.remember_sessions {
                    lang.t(
                        "Für das Update startet Paneldeck neu: Laufende Prozesse (auch Claude-Sitzungen) werden beendet, die Sitzungen werden danach wiederhergestellt.",
                        "To finish the update Paneldeck restarts: running processes (including Claude sessions) end, the sessions are restored afterwards.",
                    )
                } else {
                    lang.t(
                        "Für das Update startet Paneldeck neu: Laufende Prozesse (auch Claude-Sitzungen) werden beendet, die Sitzungen werden danach einmal wiederhergestellt. „Sitzungen merken“ ist aus: Beim nächsten Beenden werden sie verworfen.",
                        "To finish the update Paneldeck restarts: running processes (including Claude sessions) end, the sessions are restored once afterwards. “Remember sessions” is off: they are discarded on the next exit.",
                    )
                });
            }
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
        if no || esc_for(ctx, shown.map(|r| r.response.layer_id)) {
            // Der Knopf „Neu starten“ in der Fensterleiste bleibt.
            self.updater.confirm_restart = false;
        }
    }

    /// Seite *Einstellungen → Info → Updates*.
    pub(in crate::app) fn update_about_ui(&mut self, ui: &mut egui::Ui, cx: &crate::app::settings_items::RowCx) {
        let lang = self.settings.language;
        let top = ui.cursor().min.y;
        crate::app::settings_items::srow(ui, cx, "update_auto_check", |ui| {
            style::toggle(ui, &mut self.settings.update_auto_check);
        });
        crate::app::settings_items::block(ui, cx, "updates", false, |ui| self.update_about_inner(ui, lang));
        crate::app::tutorial::spot_since(ui, crate::app::tutorial::Spot::AboutUpdates, top);
    }

    fn update_about_inner(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let ctx = ui.ctx().clone();
        let dim = style::dim(ui);
        let flavor = self.updater.flavor(&self.exe);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}:", lang.t("Version", "Version"))).color(dim));
            ui.label(RichText::new(Version::current().to_string()).small());
            ui.label(RichText::new(match flavor {
                Some(Flavor::Installed) => lang.t("(installiert)", "(installed)"),
                Some(Flavor::Portable) => lang.t("(portabel)", "(portable)"),
                Some(Flavor::AppImage) => "(AppImage)",
                Some(Flavor::MacApp) => "(Paneldeck.app)",
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
            "Die Prüfung fragt nur die öffentliche GitHub-API (api.github.com) nach dem neuesten Release, ein Download lädt zusätzlich von den Download-Servern von GitHub. GitHub sieht dabei deine IP-Adresse und die Paneldeck-Version (im User-Agent); weitere Daten werden nicht gesendet.",
            "The check only asks the public GitHub API (api.github.com) for the latest release; a download also uses GitHub's download servers. GitHub sees your IP address and the Paneldeck version (in the User-Agent); no other data is sent.",
        )).small().color(dim));
    }
}
