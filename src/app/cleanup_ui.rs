//! Fenster „Aufräumen …“ (#307, 2.6.0): *Einstellungen → Daten* bzw. Befehls-Palette. Einträge mit Häkchen, Größe und
//! kurzer Erklärung (im Hintergrund ermittelt, `crate::cleanup`), Summe unten, dann eine Rückfrage für den ganzen Lauf
//! (3-Sekunden-Knopf; bei Unwiderruflichem einmal `DELETE`/`LÖSCHEN` tippen) und am Ende ein Ergebnis je Eintrag mit
//! *Erneut versuchen* und ggf. *Jetzt neu starten*.
//!
//! Ablauf ohne Neustart: Was im Speicher steckt (Einstellungen, Prompt-Bibliothek, Vorlagen, Verlauf offener Terminals und
//! Chats) ändert die Oberfläche sofort; die Dateien löscht ein Hintergrund-Faden (`cleanup::execute`, Phase `Live`).
//! Mit Neustart (Datenbank, Sitzungen/Layout, alles zurücksetzen): Integration, andere Profile und die ZIP-Sicherung laufen
//! sofort, alles im Datenordner wird vorgemerkt (`cleanup::schedule`) und beim nächsten Start vor dem Laden ausgeführt;
//! dessen Ergebnis zeigt dieses Fenster dann nach dem Start. Fehlt die ZIP-Sicherung, wird nichts zurückgesetzt.
//!
//! Das Fenster erscheint nur im Hauptfenster (der Neustart beendet den Prozess ohne Speichern).

use super::planning_ui::{countdown_secs, danger_button};
use super::*;
use crate::cleanup::{self, Group, Item, ItemReport, Opts, Phase, Scan, Selection};
use std::sync::mpsc::Receiver;


/// Rückfrage vor dem Löschen.
struct Confirm {
    since: Instant,
    typed: String,
    /// Betroffene offene Sitzungen (Name).
    affected: Vec<String>,
    /// Davon mit laufendem Programm (Name, was läuft).
    running: Vec<(String, String)>,
}

/// Ergebnis des Hintergrund-Teils.
struct RunOut {
    reports: Vec<ItemReport>,
    /// Die ZIP-Sicherung fehlte: nichts zurücksetzen.
    abort: bool,
}

/// Neustart nach dem Hintergrund-Teil.
struct RestartPlan {
    pending: cleanup::Pending,
    ends_sessions: bool,
}

#[derive(Default)]
pub(super) struct CleanupUi {
    /// Hauptfenster offen (ohne: nur das Ergebnis nach einem Neustart).
    main: bool,
    scan: Option<Scan>,
    scan_rx: Option<Receiver<Scan>>,
    sel: Selection,
    keep: usize,
    all_backups: bool,
    age_on: bool,
    days: u32,
    backup_db: bool,
    zip_dir: String,
    confirm: Option<Confirm>,
    run_rx: Option<Receiver<RunOut>>,
    /// Teile, die die Oberfläche schon erledigt hat (kommen vor das Ergebnis des Fadens).
    ui_reports: Vec<ItemReport>,
    restart: Option<RestartPlan>,
    report: Option<Vec<ItemReport>>,
    /// Auswahl des letzten Laufs (für „Erneut versuchen“).
    last: Option<(Selection, Opts)>,
}

impl CleanupUi {
    fn new(zip_dir: String) -> Self {
        CleanupUi {
            main: true,
            sel: Selection { items: Item::ALL.into_iter().filter(|i| i.preselected()).collect(), profiles: Default::default() },
            keep: cleanup::DEFAULT_KEEP,
            days: cleanup::DEFAULT_DAYS,
            backup_db: true,
            zip_dir,
            ..Default::default()
        }
    }

    /// Ergebnis eines Laufs, der beim Start ausgeführt wurde.
    pub(super) fn with_report(r: Vec<ItemReport>) -> Self {
        CleanupUi { report: Some(r), keep: cleanup::DEFAULT_KEEP, days: cleanup::DEFAULT_DAYS, backup_db: true, ..Default::default() }
    }

    fn opts(&self) -> Opts {
        let reset = self.sel.items.contains(&Item::FactoryReset);
        Opts {
            keep_backups: if self.all_backups || reset { 0 } else { self.keep.max(1) },
            older_than_days: (self.age_on && !reset).then_some(self.days.max(1)),
            // Erzwungen, wenn die Datenbank selbst gelöscht wird.
            backup_db: self.backup_db || self.sel.items.contains(&Item::Database),
        }
    }

    fn busy(&self) -> bool {
        self.run_rx.is_some()
    }
}

/// Standardziel der ZIP-Sicherung: „Dokumente“ des Benutzers, sonst der Ordner über dem Datenordner.
fn default_zip_dir(data: &std::path::Path) -> String {
    let docs = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join("Documents")).filter(|p| p.is_dir());
    docs.or_else(|| data.parent().map(|p| p.to_path_buf())).map(|p| p.display().to_string()).unwrap_or_default()
}

fn amount_text(scan: Option<&Scan>, item: Item, opts: &Opts, lang: Lang) -> String {
    let Some(f) = scan.and_then(|s| s.found(item)) else { return "…".into() };
    let (b, n, e) = cleanup::amount(item, f, opts, std::time::SystemTime::now());
    cleanup::fmt_amount(b, n, e, lang)
}

impl App {
    /// Kontext für Scan und Löschen aus dem laufenden Hub.
    pub(super) fn cleanup_ctx(&self) -> cleanup::Ctx {
        cleanup::Ctx {
            data_dir: self.dir.clone(),
            backup_dir: self.backup_dir_path(),
            safety_dir: None,
            base_dir: crate::paths::get().base.clone(),
            open_ids: self.entries.keys().copied().collect(),
            exe: self.exe.clone(),
            temp_dir: std::env::temp_dir(),
            settings: Some(self.settings.clone()),
            system_integration: true,
            integ_override: None,
        }
    }

    /// Fenster öffnen (oder nach vorn holen) und Größen neu ermitteln.
    pub(super) fn open_cleanup(&mut self, ctx: &egui::Context) {
        let mut c = self.cleanup.take().filter(|c| c.main).unwrap_or_else(|| CleanupUi::new(default_zip_dir(&self.dir)));
        c.main = true;
        self.cleanup_rescan(&mut c, ctx);
        self.cleanup = Some(c);
    }

    fn cleanup_rescan(&self, c: &mut CleanupUi, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cx = self.cleanup_ctx();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(cleanup::scan(&cx));
            repaint.request_repaint();
        });
        c.scan_rx = Some(rx);
    }

    /// MCP `cleanup_sizes` (nur lesend): Größen und Anzahlen je Bereich, nie Pfade oder Namen.
    pub(super) fn cleanup_sizes_json(&self) -> serde_json::Value {
        let s = cleanup::scan(&self.cleanup_ctx());
        let now = std::time::SystemTime::now();
        let all = Opts { keep_backups: 0, older_than_days: None, backup_db: false };
        let items: Vec<serde_json::Value> = Item::ALL
            .into_iter()
            .filter(|i| s.visible(*i))
            .filter_map(|i| {
                let f = s.found(i)?;
                let (bytes, files, entries) = cleanup::amount(i, f, &all, now);
                Some(serde_json::json!({
                    "id": i.id(), "group": i.group().id(), "bytes": bytes, "files": files, "entries": entries,
                    "preselected": i.preselected(), "needs_typing": i.needs_typing(false), "needs_restart": i.needs_restart(),
                }))
            })
            .collect();
        let data = s.found(Item::FactoryReset).map(|f| (f.files.iter().map(|e| e.bytes).sum::<u64>(), f.files.len())).unwrap_or_default();
        serde_json::json!({
            "data_folder": {"bytes": data.0, "files": data.1},
            "items": items,
            "profiles": {
                "count": s.profiles.len(),
                "running": s.profiles.iter().filter(|p| p.running).count(),
                "bytes": s.profiles.iter().map(|p| p.bytes).sum::<u64>(),
            },
            "note": state::t(
                "Nur lesend. Aufräumen geht nur im Paneldeck-Fenster (Einstellungen → Daten → Aufräumen …); Agenten können nichts löschen.",
                "Read-only. Cleaning up is only possible in the Paneldeck window (Settings → Data → Clean up …); agents cannot delete anything.",
            ),
        })
    }

    /// Pro Bild (nur im Hauptfenster): Fenster, Rückfrage, Ergebnis.
    pub(super) fn cleanup_ui(&mut self, ctx: &egui::Context) {
        let Some(mut c) = self.cleanup.take() else { return };
        if let Some(rx) = &c.scan_rx {
            match rx.try_recv() {
                Ok(s) => {
                    // Profile, die es nicht mehr gibt, fallen aus der Auswahl.
                    c.sel.profiles.retain(|p| s.profiles.iter().any(|x| &x.name == p && !x.running));
                    c.scan = Some(s);
                    c.scan_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => c.scan_rx = None,
                Err(_) => ctx.request_repaint_after(Duration::from_millis(200)),
            }
        }
        if let Some(rx) = &c.run_rx {
            match rx.try_recv() {
                Ok(out) => {
                    c.run_rx = None;
                    self.cleanup_finish(&mut c, out, ctx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => c.run_rx = None,
                Err(_) => ctx.request_repaint_after(Duration::from_millis(200)),
            }
        }
        let mut keep = true;
        if c.report.is_some() {
            keep = self.cleanup_report_ui(ctx, &mut c);
        } else if c.main {
            keep = self.cleanup_main_ui(ctx, &mut c);
        }
        if c.confirm.is_some() {
            self.cleanup_confirm_ui(ctx, &mut c);
        }
        if keep || c.busy() {
            self.cleanup = Some(c);
        }
    }

    fn cleanup_main_ui(&mut self, ctx: &egui::Context, c: &mut CleanupUi) -> bool {
        let lang = self.settings.language;
        let mut open = true;
        let (mut go, mut close, mut refresh) = (false, false, false);
        let opts = c.opts();
        let reset = c.sel.items.contains(&Item::FactoryReset);
        let busy = c.busy();
        style::movable(ctx, Id::new("cleanup_window"), egui::Window::new(lang.t("Aufräumen", "Clean up")))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 620.0])
            .show(ctx, |ui| {
                let dim = style::dim(ui);
                ui.label(RichText::new(lang.t(
                    "Häkchen setzen, was gelöscht werden soll. Gelöscht wird endgültig (kein Papierkorb); geänderte Einstellungs-, JSON- und Registry-Einträge werden vorher gesichert.",
                    "Tick what to delete. Deleting is permanent (no recycle bin); changed settings, JSON and registry entries are backed up first.",
                )).small().color(dim));
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(lang.t("Datenordner:", "Data folder:")).small().color(dim));
                    ui.label(RichText::new(self.dir.display().to_string()).small().monospace());
                });
                if c.scan_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(lang.t("Größen werden ermittelt …", "Calculating sizes …")).small().color(dim));
                    });
                }
                ui.add_space(S1);
                egui::ScrollArea::vertical().id_salt("cleanup_list").max_height(ui.available_height() - 110.0).auto_shrink([false, true]).show(ui, |ui| {
                    for g in Group::ALL {
                        let items: Vec<Item> = Item::ALL.into_iter().filter(|i| i.group() == g && c.scan.as_ref().is_none_or(|s| s.visible(*i))).collect();
                        if items.is_empty() {
                            continue;
                        }
                        ui.add_space(S2);
                        let title = if g == Group::Integration && !cfg!(windows) { lang.t("System-Integration", "System integration") } else { g.label(lang) };
                        ui.label(RichText::new(title).strong());
                        if g == Group::History {
                            ui.horizontal(|ui| {
                                ui.add_enabled(!reset, egui::Checkbox::new(&mut c.age_on, lang.t("Nur Verlauf älter als", "Only history older than")));
                                ui.add_enabled(!reset && c.age_on, egui::DragValue::new(&mut c.days).range(1..=3650));
                                ui.label(lang.t("Tage", "days"));
                            });
                        }
                        for item in items {
                            Self::cleanup_row(ui, c, item, &opts, lang);
                        }
                        if g == Group::Everything {
                            if reset {
                                ui.horizontal_wrapped(|ui| {
                                    ui.add_space(24.0);
                                    ui.label(RichText::new(lang.t("ZIP-Sicherung nach:", "ZIP backup to:")).small());
                                    ui.label(RichText::new(&c.zip_dir).small().monospace());
                                    if ui.small_button(lang.t("Ordner wählen …", "Choose folder …")).clicked() {
                                        if let Some(p) = notify::pick_folder(&c.zip_dir) {
                                            c.zip_dir = p;
                                        }
                                    }
                                });
                                ui.horizontal_wrapped(|ui| {
                                    ui.add_space(24.0);
                                    ui.label(RichText::new(lang.t(
                                        "Die ZIP-Datei enthält keine Zugangsdaten: gespeicherte API-Schlüssel bleiben draußen (sie liegen ohnehin nur verschlüsselt für diesen Windows-Benutzer vor).",
                                        "The ZIP file contains no credentials: stored API keys are left out (they are only stored encrypted for this Windows user anyway).",
                                    )).small().color(dim));
                                });
                            }
                            Self::cleanup_profiles_ui(ui, c, lang);
                        }
                    }
                    ui.add_space(S2);
                    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(".claude").display().to_string()).unwrap_or_else(|| "~/.claude".into());
                    ui.label(RichText::new(format!(
                        "{} {home}",
                        lang.t("Nie berührt: die Daten von Claude Code selbst (Konversationen, Transkripte, projects). Sie liegen unter", "Never touched: Claude Code's own data (conversations, transcripts, projects). It lives in")
                    )).small().color(dim));
                });
                ui.separator();
                let (bytes, files) = c.scan.as_ref().map(|s| cleanup::total(s, &c.sel, &opts, std::time::SystemTime::now())).unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} {}", lang.t("Wird gelöscht:", "Will be deleted:"), cleanup::fmt_bytes(bytes))).strong());
                    ui.label(RichText::new(format!("({files} {})", if files == 1 { lang.t("Datei", "file") } else { lang.t("Dateien", "files") })).small().color(style::dim(ui)));
                });
                let forced = c.sel.items.contains(&Item::Database) || reset;
                let mut b = c.backup_db || c.sel.items.contains(&Item::Database);
                if ui
                    .add_enabled(!forced, egui::Checkbox::new(&mut b, lang.t("Datenbank vorher sichern", "Back up the database first")))
                    .on_disabled_hover_text(lang.t("Immer, wenn die Datenbank selbst gelöscht wird (beim Zurücksetzen ist die ZIP-Datei die Sicherung).", "Always when the database itself is deleted (when resetting, the ZIP file is the backup)."))
                    .changed()
                {
                    c.backup_db = b;
                }
                ui.horizontal(|ui| {
                    let can = !busy && !c.sel.is_empty() && c.scan.is_some();
                    let label = if reset { lang.t("Zurücksetzen …", "Reset …") } else { lang.t("Löschen …", "Delete …") };
                    let btn = egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(if can { style::ui().danger_button } else { style::ui().danger_button.gamma_multiply(0.4) });
                    if ui.add_enabled(can, btn).clicked() {
                        go = true;
                    }
                    if ui.button(lang.t("Schließen", "Close")).clicked() {
                        close = true;
                    }
                    if ui.add_enabled(!busy, egui::Button::new(lang.t("Neu ermitteln", "Refresh"))).clicked() {
                        refresh = true;
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            });
        if close {
            open = false;
        }
        if refresh {
            c.scan = None;
            self.cleanup_rescan(c, ctx);
        }
        if go && c.confirm.is_none() {
            c.confirm = Some(self.cleanup_confirm_for(c));
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) && c.confirm.is_none() && !busy {
            open = false;
        }
        open
    }

    fn cleanup_row(ui: &mut Ui, c: &mut CleanupUi, item: Item, opts: &Opts, lang: Lang) {
        let reset = c.sel.items.contains(&Item::FactoryReset);
        let forced = reset && item != Item::FactoryReset;
        let mut on = c.sel.items.contains(&item) || forced;
        ui.horizontal(|ui| {
            let mut text = RichText::new(item.label(lang));
            if item == Item::FactoryReset {
                text = text.color(style::ui().error).strong();
            }
            if ui.add_enabled(!forced && !c.busy(), egui::Checkbox::new(&mut on, text)).changed() {
                if on {
                    c.sel.items.insert(item);
                } else {
                    c.sel.items.remove(&item);
                }
            }
            if item.needs_typing(c.all_backups) {
                ui.label(RichText::new("⚠").small().color(style::ui().error)).on_hover_text(lang.t("Unwiderruflich – verlangt das Eintippen der Bestätigung.", "Irreversible – requires typing the confirmation."));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Platz für die (schwebende) Bildlaufleiste lassen, sonst liegt sie über dem
                // Ende der Größenangabe und schneidet die letzten Zeichen ab.
                let pad = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
                ui.add_space(pad);
                ui.label(RichText::new(amount_text(c.scan.as_ref(), item, opts, lang)).small().monospace());
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.add_space(24.0);
            let col = if item == Item::McpLog { style::ui().warning } else { style::dim(ui) };
            ui.label(RichText::new(item.hint(lang)).small().color(col));
        });
        if item == Item::DbBackups && (on || forced) {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.add_enabled_ui(!c.all_backups && !reset, |ui| {
                    ui.label(RichText::new(lang.t("Neueste behalten:", "Keep newest:")).small());
                    ui.add(egui::DragValue::new(&mut c.keep).range(1..=100));
                });
                ui.add_enabled(!reset, egui::Checkbox::new(&mut c.all_backups, RichText::new(lang.t("alle löschen", "delete all")).small()));
            });
        }
    }

    fn cleanup_profiles_ui(ui: &mut Ui, c: &mut CleanupUi, lang: Lang) {
        let Some(s) = &c.scan else { return };
        if s.profiles.is_empty() {
            return;
        }
        ui.add_space(S2);
        ui.label(RichText::new(lang.t("Andere Profile", "Other profiles")).strong());
        ui.label(RichText::new(lang.t("Ganz löschen geht nur, solange das Profil nicht läuft.", "Deleting completely is only possible while the profile is not running.")).small().color(style::dim(ui)));
        let profiles = s.profiles.clone();
        for p in profiles {
            let mut on = c.sel.profiles.contains(&p.name);
            ui.horizontal(|ui| {
                let text = format!("{} {} – {}", lang.q(&p.name), lang.t("komplett löschen", "delete completely"), cleanup::fmt_amount(p.bytes, p.files, 0, lang));
                let r = ui.add_enabled(!p.running && !c.busy(), egui::Checkbox::new(&mut on, text));
                if p.running {
                    r.on_disabled_hover_text(lang.t("Dieses Profil läuft gerade.", "This profile is running."));
                    ui.label(RichText::new(lang.t("(läuft)", "(running)")).small().color(style::dim(ui)));
                } else if r.changed() {
                    if on {
                        c.sel.profiles.insert(p.name.clone());
                    } else {
                        c.sel.profiles.remove(&p.name);
                    }
                }
            });
        }
    }

    /// Rückfrage zusammenstellen: betroffene Sitzungen und laufende Programme (über die Kindprozesse der Shell).
    fn cleanup_confirm_for(&self, c: &CleanupUi) -> Confirm {
        let items = c.sel.expanded();
        let mut affected = Vec::new();
        let mut running = Vec::new();
        // Mit Neustart enden alle laufenden Programme (auch in Sitzungen, die danach wiederhergestellt werden).
        let restart = c.sel.needs_restart();
        let sessions = items.iter().any(|i| i.affects_sessions());
        if sessions || restart {
            let lang = self.settings.language;
            let ends = items.iter().any(|i| i.ends_sessions());
            for e in self.entries.values() {
                let touched = sessions && (ends || (e.is_chat() && (items.contains(&Item::ChatsAll))) || (!e.is_chat() && items.contains(&Item::ScrollbackAll)));
                if !touched && !restart {
                    continue;
                }
                let name = e.display_name();
                if touched {
                    affected.push(name.clone());
                }
                let what = if let Some(chat) = &e.chat {
                    chat.busy().then(|| lang.t("Chat arbeitet", "chat is working").to_string())
                } else if e.exited() {
                    None
                } else if e.launched_claude {
                    Some("Claude Code".to_string())
                } else {
                    let pid = e.live.as_ref().and_then(|s| s.pid());
                    match pid.and_then(crate::platform::process::shell_is_idle) {
                        Some(false) => Some(lang.t("Programm läuft", "program running").to_string()),
                        _ => None,
                    }
                };
                if let Some(w) = what {
                    running.push((name, w));
                }
            }
        }
        Confirm { since: Instant::now(), typed: String::new(), affected, running }
    }

    fn cleanup_confirm_ui(&mut self, ctx: &egui::Context, c: &mut CleanupUi) {
        let lang = self.settings.language;
        let opts = c.opts();
        let Some(mut cf) = c.confirm.take() else { return };
        let items = c.sel.expanded();
        let reset = items.contains(&Item::FactoryReset);
        let restart = c.sel.needs_restart();
        let typing = c.sel.needs_typing(&opts);
        let ends = items.iter().any(|i| i.ends_sessions());
        let secs = countdown_secs(cf.since, Instant::now());
        if secs > 0 {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let (mut go, mut cancel) = (false, ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)));
        let now = std::time::SystemTime::now();
        style::movable(ctx, Id::new("cleanup_confirm"), egui::Window::new(lang.t("Endgültig löschen?", "Delete permanently?")))
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(480.0);
                let dim = style::dim(ui);
                ui.label(lang.t("Folgendes wird endgültig gelöscht (kein Papierkorb):", "The following is deleted permanently (no recycle bin):"));
                egui::ScrollArea::vertical().id_salt("cleanup_confirm_list").max_height(220.0).show(ui, |ui| {
                    for item in Item::ALL.into_iter().filter(|i| items.contains(i)) {
                        let Some(f) = c.scan.as_ref().and_then(|s| s.found(item)) else { continue };
                        if item != Item::FactoryReset && f.is_empty() && reset {
                            continue;
                        }
                        let (b, n, e) = cleanup::amount(item, f, &opts, now);
                        ui.label(format!("• {} – {}", item.label(lang), cleanup::fmt_amount(b, n, e, lang)));
                    }
                    for p in c.scan.iter().flat_map(|s| s.profiles.iter()).filter(|p| c.sel.profiles.contains(&p.name)) {
                        ui.label(format!("• {} {} – {}", lang.t("Profil", "Profile"), lang.q(&p.name), cleanup::fmt_amount(p.bytes, p.files, 0, lang)));
                    }
                });
                if opts.backup_db && !reset {
                    ui.label(RichText::new(lang.t("Die Datenbank wird vorher gesichert.", "The database is backed up first.")).small().color(dim));
                }
                if reset {
                    ui.label(RichText::new(format!("{} {}", lang.t("Vorher entsteht eine ZIP-Sicherung (ohne Zugangsdaten) in", "A ZIP backup (without credentials) is saved first to"), c.zip_dir)).small());
                }
                if restart {
                    ui.label(RichText::new(lang.t("Danach startet Paneldeck neu.", "Paneldeck then restarts.")).small().color(dim));
                    ui.label(
                        RichText::new(lang.t(
                            "Beim Neustart enden alle laufenden Programme in Terminals sofort (ohne Strg+C) – vorher speichern und beenden.",
                            "On the restart every program running in a terminal ends at once (without Ctrl+C) – save and quit them first.",
                        ))
                        .small()
                        .color(if cf.running.is_empty() { dim } else { style::ui().error }),
                    );
                    if cf.affected.is_empty() && !cf.running.is_empty() {
                        ui.label(RichText::new(lang.t("Darin laufen noch Programme:", "Programs are still running in:")).color(style::ui().error));
                        for (n, w) in &cf.running {
                            ui.label(RichText::new(format!("• {n} – {w}")).small());
                        }
                    }
                }
                if !cf.affected.is_empty() {
                    ui.add_space(S1);
                    ui.label(RichText::new(if ends { lang.t("Diese Sitzungen werden beendet:", "These sessions are ended:") } else { lang.t("Betroffene Sitzungen:", "Affected sessions:") }).strong());
                    ui.label(RichText::new(cf.affected.join(", ")).small());
                    if !cf.running.is_empty() {
                        ui.label(RichText::new(lang.t("Darin laufen noch Programme:", "Programs are still running in:")).color(style::ui().error));
                        for (n, w) in &cf.running {
                            ui.label(RichText::new(format!("• {n} – {w}")).small());
                        }
                    }
                }
                let mut ok = true;
                if typing {
                    ui.add_space(S1);
                    let word = cleanup::confirm_word(lang);
                    ui.label(format!("{} {}", lang.t("Zum Bestätigen eintippen:", "Type to confirm:"), word));
                    ui.add(egui::TextEdit::singleline(&mut cf.typed).hint_text(word).desired_width(180.0));
                    ok = cleanup::confirm_ok(&cf.typed);
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    let label = if ends && !cf.running.is_empty() { lang.t("Schließen und löschen", "Close them and delete") } else if reset { lang.t("Zurücksetzen", "Reset") } else { lang.t("Löschen", "Delete") };
                    ui.add_enabled_ui(ok, |ui| {
                        if danger_button(ui, label, secs).clicked() {
                            go = true;
                        }
                    });
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if go {
            let (sel, opts) = (c.sel.clone(), c.opts());
            self.cleanup_start(ctx, c, sel, opts);
        } else if !cancel {
            c.confirm = Some(cf);
        }
    }

    /// Lauf starten: Speicher-Teile sofort, Dateien im Hintergrund (siehe Moduldoku).
    fn cleanup_start(&mut self, ctx: &egui::Context, c: &mut CleanupUi, sel: Selection, opts: Opts) {
        let lang = self.settings.language;
        let items = sel.expanded();
        let restart = sel.needs_restart();
        let reset = items.contains(&Item::FactoryReset);
        c.last = Some((sel.clone(), opts.clone()));
        c.ui_reports.clear();
        if !restart {
            self.cleanup_memory(c, &items, &opts, lang);
        }
        let mut cx = self.cleanup_ctx();
        let (live_sel, live_opts) = if restart {
            (Selection { items: items.iter().copied().filter(|i| i.outside_data()).collect(), profiles: sel.profiles.clone() }, Opts { backup_db: false, ..opts.clone() })
        } else {
            (sel.clone(), opts.clone())
        };
        let zip_to = reset.then(|| PathBuf::from(&c.zip_dir));
        // Beim Zurücksetzen kommen die Sicherheitskopien der Integration (.reg, claude.json-*.bak) neben die ZIP-Datei:
        // der Sicherungsordner im Datenordner wird beim Neustart mit gelöscht.
        cx.safety_dir = zip_to.clone();
        if zip_to.is_some() {
            // Der aktuelle Stand gehört in die Sicherung.
            self.save_all();
        }
        if restart {
            let data_items: Vec<String> = items.iter().filter(|i| !i.outside_data()).map(|i| i.id().to_string()).collect();
            c.restart = Some(RestartPlan {
                pending: cleanup::Pending { version: 1, items: data_items, opts: opts.clone(), backup_dir: Some(self.backup_dir_path()), done: Vec::new() },
                ends_sessions: items.iter().any(|i| i.ends_sessions()),
            });
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let mut reports = Vec::new();
            if let Some(dir) = zip_to {
                let r = cleanup::zip_backup(&cx.data_dir, &dir);
                let failed = !r.ok();
                reports.push(r);
                if failed {
                    let _ = tx.send(RunOut { reports, abort: true });
                    repaint.request_repaint();
                    return;
                }
            }
            let scan = cleanup::scan(&cx);
            reports.extend(cleanup::execute(&cx, &scan, &live_sel, &live_opts, Phase::Live));
            let _ = tx.send(RunOut { reports, abort: false });
            repaint.request_repaint();
        });
        c.run_rx = Some(rx);
    }

    /// Was im Speicher steckt, sofort ändern (nur ohne Neustart).
    fn cleanup_memory(&mut self, c: &mut CleanupUi, items: &std::collections::BTreeSet<Item>, opts: &Opts, lang: Lang) {
        let mut settings_changed = false;
        for item in Item::ALL.into_iter().filter(|i| i.is_setting() && items.contains(i)) {
            let mut r = ItemReport::new(item.id());
            if item == Item::SettingsAll {
                match cleanup::backup_settings(&self.dir) {
                    Ok(Some(b)) => {
                        let p = b.display().to_string();
                        r.notes.push((format!("Sicherung: {p}"), format!("Backup: {p}")));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        r.errors.push(e);
                        c.ui_reports.push(r);
                        continue;
                    }
                }
                r.notes.push(("Die Sprache bleibt. Manches wirkt erst nach einem Neustart.".into(), "The language stays. Some changes apply only after a restart.".into()));
                r.restart = true;
            }
            cleanup::reset_setting(item, &mut self.settings, true);
            settings_changed = true;
            r.entries = 1;
            c.ui_reports.push(r);
        }
        if settings_changed {
            self.settings.sanitize();
            crate::platform::conpty::USE_BUNDLED.store(self.settings.bundled_conpty, std::sync::atomic::Ordering::Relaxed);
            state::save_json(&self.dir.join("settings.json"), &self.settings);
        }
        if items.contains(&Item::Prompts) {
            self.prompts = prompts::defaults(state::persisted_lang(self.settings.language));
        }
        if items.contains(&Item::Templates) {
            self.templates.clear();
        }
        // Verlauf offener Sitzungen: Terminals behalten nur den Bildschirm, Chats werden leer – bei „älter als X Tage“ nur
        // die, deren Datei so alt ist.
        let now = std::time::SystemTime::now();
        for (item, chat) in [(Item::ScrollbackAll, false), (Item::ChatsAll, true)] {
            if !items.contains(&item) {
                continue;
            }
            let ids: Vec<u64> = match (opts.older_than_days, c.scan.as_ref().and_then(|s| s.found(item))) {
                (Some(_), Some(f)) => cleanup::selected_files(item, f, opts, now)
                    .iter()
                    .filter_map(|e| e.path.file_name().and_then(|n| cleanup::session_id_of(&n.to_string_lossy())))
                    .collect(),
                _ => self.entries.keys().copied().collect(),
            };
            let mut r = ItemReport::new(item.id());
            for id in ids {
                let Some(e) = self.entries.get_mut(&id) else { continue };
                if chat != e.is_chat() {
                    continue;
                }
                let name = e.display_name();
                if let Some(ch) = &mut e.chat {
                    if !ch.clear_history() {
                        r.errors.push(format!("{}: {}", name, lang.t("Chat arbeitet gerade – Verlauf nicht geleert.", "chat is working – history not cleared.")));
                    }
                } else if let Some(s) = &e.live {
                    if !s.clear_history() {
                        r.errors.push(format!("{}: {}", name, lang.t("Vollbildprogramm läuft – Verlauf nicht geleert.", "a full-screen program runs – history not cleared.")));
                    }
                }
            }
            if !r.errors.is_empty() {
                c.ui_reports.push(r);
            }
        }
    }

    fn cleanup_finish(&mut self, c: &mut CleanupUi, out: RunOut, ctx: &egui::Context) {
        let lang = self.settings.language;
        let mut reports = std::mem::take(&mut c.ui_reports);
        reports.extend(out.reports);
        let reports = cleanup::merge_reports(reports);
        if let Some(plan) = c.restart.take() {
            if out.abort {
                c.report = Some(reports);
                return;
            }
            let mut pending = plan.pending;
            pending.done = reports.clone();
            if let Err(e) = cleanup::schedule(&self.dir, &pending) {
                let mut r = ItemReport::new("pending");
                r.errors.push(format!("{}: {e}", lang.t("Vormerken für den Neustart fehlgeschlagen", "Scheduling for the restart failed")));
                let mut all = reports;
                all.push(r);
                c.report = Some(all);
                return;
            }
            let keep_once = if plan.ends_sessions {
                let ids: Vec<u64> = self.entries.keys().copied().collect();
                self.close_many(&ids);
                false
            } else {
                self.save_for_update_restart()
            };
            match update::install::spawn_restart(&self.exe, keep_once) {
                Ok(()) => {
                    crate::platform::chrome::shutdown();
                    self.tray = None;
                    std::process::exit(0);
                }
                Err(e) => {
                    let mut r = ItemReport::new("pending");
                    r.errors.push(format!(
                        "{}: {e}",
                        lang.t("Neustart fehlgeschlagen – bitte Paneldeck selbst neu starten, das Aufräumen läuft dann beim Start", "Restart failed – please restart Paneldeck yourself, the clean-up then runs at start")
                    ));
                    let mut all = reports;
                    all.push(r);
                    c.report = Some(all);
                    return;
                }
            }
        }
        let items = c.last.as_ref().map(|(s, _)| s.expanded()).unwrap_or_default();
        if items.contains(&Item::Themes) {
            self.reload_themes();
            if !self.themes.iter().any(|t| t.name == self.settings.theme) {
                self.settings.theme = Settings::default().theme;
            }
        }
        c.report = Some(reports);
        self.cleanup_rescan(c, ctx);
    }

    fn cleanup_report_ui(&mut self, ctx: &egui::Context, c: &mut CleanupUi) -> bool {
        let lang = self.settings.language;
        let Some(list) = c.report.clone() else { return false };
        let (mut close, mut retry, mut restart) = (false, false, false);
        let (bytes, files) = cleanup::report_total(&list);
        let failed: Vec<&ItemReport> = list.iter().filter(|r| !r.ok()).collect();
        let can_retry = c.last.is_some() && failed.iter().any(|r| Item::from_id(&r.id).is_some() || r.id.starts_with("profile:"));
        let wants_restart = list.iter().any(|r| r.restart);
        style::movable(ctx, Id::new("cleanup_report"), egui::Window::new(lang.t("Aufräumen – Ergebnis", "Clean up – result")))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
                let dim = style::dim(ui);
                ui.label(RichText::new(format!(
                    "{} {} {} {files} {}",
                    lang.t("Gelöscht:", "Deleted:"),
                    cleanup::fmt_bytes(bytes),
                    lang.t("in", "in"),
                    if files == 1 { lang.t("Datei", "file") } else { lang.t("Dateien", "files") }
                )).strong());
                ui.add_space(S1);
                egui::ScrollArea::vertical().id_salt("cleanup_report_list").max_height(360.0).show(ui, |ui| {
                    for r in &list {
                        ui.horizontal(|ui| {
                            let (mark, col) = if r.ok() { ("✓", style::ui().success) } else { ("✗", style::ui().error) };
                            ui.label(RichText::new(mark).color(col).strong());
                            ui.label(r.label(lang));
                            if r.bytes > 0 || r.files > 0 || r.entries > 0 {
                                ui.label(RichText::new(cleanup::fmt_amount(r.bytes, r.files, r.entries, lang)).small().color(dim));
                            }
                        });
                        for e in &r.errors {
                            ui.label(RichText::new(format!("    {e}")).small().color(style::ui().error));
                        }
                        for (de, en) in &r.notes {
                            ui.label(RichText::new(format!("    {}", if lang == Lang::De { de } else { en })).small().color(dim));
                        }
                    }
                });
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    if can_retry && ui.add_enabled(!c.busy(), egui::Button::new(lang.t("Erneut versuchen", "Retry"))).clicked() {
                        retry = true;
                    }
                    if wants_restart
                        && ui
                            .button(lang.t("Jetzt neu starten", "Restart now"))
                            .on_hover_text(lang.t("Laufende Programme in Terminals enden dabei sofort (ohne Strg+C).", "Programs running in terminals end at once (without Ctrl+C)."))
                            .clicked()
                    {
                        restart = true;
                    }
                    if ui.button(lang.t("Schließen", "Close")).clicked() {
                        close = true;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        if retry {
            if let Some((_, opts)) = c.last.clone() {
                let mut sel = Selection::default();
                for r in &failed {
                    if let Some(i) = Item::from_id(&r.id) {
                        sel.items.insert(i);
                    } else if let Some(p) = r.id.strip_prefix("profile:") {
                        sel.profiles.insert(p.to_string());
                    }
                }
                c.report = None;
                self.cleanup_start(ctx, c, sel, Opts { backup_db: false, ..opts });
            }
            return true;
        }
        if restart {
            let keep_once = self.save_for_update_restart();
            if update::install::spawn_restart(&self.exe, keep_once).is_ok() {
                crate::platform::chrome::shutdown();
                self.tray = None;
                std::process::exit(0);
            }
        }
        if close {
            c.report = None;
            return c.main;
        }
        true
    }
}
