//! Wartungsseite eines Experten-Projekts (#308): öffnet sich über den Knopf „Wartung“ der Übersicht als eigene Seite (wie ein
//! Ticket, mit *Zurück* / `Esc`). Links die Kategorien *Tickets* · *Konfiguration* · *Datenbank* · *Eigene Regeln* mit Anzahl,
//! dazu *Ignoriert*, *Protokoll* und *Prüfungen*; rechts die Befunde mit Schwere, Erklärung und Aktion, Mehrfachauswahl und
//! *Auswahl beheben …*.
//!
//! * Die volle Prüfung läuft beim Öffnen im Hintergrund (eigene, nur lesende Verbindung), die Übersicht zählt günstig vor.
//! * Aktionen laufen mit Vorschau ([`maint_dialogs`](super::maint_dialogs)) und in einem Hintergrund-Faden: Sicherung
//!   „maintenance“, Aktion, Protokoll. Solange hält die Wartung ihre Schreibsperre (MCP-Schreibwerkzeuge antworten `busy`).

use super::widgets::{self, hint};
use super::{Cx, PlanUi};
use crate::planning::maintenance::actions::Action;
use crate::planning::maintenance::dbcare::{self, RepairStep, RescueReport, Stats};
use crate::planning::maintenance::rules::Rule;
use crate::planning::maintenance::{self as maint, Category, Check, Finding, Fix, LogEntry, MRes, Report, RunOpts, Severity, Target};
use crate::planning::projects::Project;
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, Sense, Ui, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// So lange gilt die Zählung der Übersicht.
const COUNT_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Pane {
    Cat(Category),
    Ignored,
    Log,
    Checks,
}

/// Ergebnis eines Hintergrund-Auftrags.
pub(super) enum Done {
    Report(MRes<Report>),
    Action(MRes<maint::actions::Outcome>, String),
    Repair(MRes<i64>),
    /// Reparaturplan (Vorschau) – braucht ein volles `integrity_check`, läuft daher im Hintergrund.
    Plan(MRes<Vec<RepairStep>>),
    Compact(MRes<dbcare::CompactResult>),
    Rescue(MRes<RescueReport>),
    Stats(MRes<Stats>),
}

#[derive(Default)]
pub(super) struct State {
    pub open: bool,
    pub pane: Option<Pane>,
    pub project: i64,
    pub report: Option<Report>,
    pub selected: HashSet<String>,
    pub severity: Option<Severity>,
    /// Laufender Auftrag (Prüfung oder Aktion) und seine Beschriftung.
    pub job: Option<(Receiver<Done>, String, Instant)>,
    /// Wartet eine neue Prüfung, bis der Auftrag fertig ist?
    pub rerun: bool,
    pub msg: Option<(String, bool)>,
    /// Günstige Zählung je Projekt für den Knopf der Übersicht: (Probleme, Zeitpunkt).
    pub counts: HashMap<i64, (usize, Instant)>,
    pub stats: Option<(bool, Stats)>,
    pub whole_db: bool,
    pub repair_plan: Option<Vec<RepairStep>>,
    pub rescue: Option<RescueReport>,
    pub log: Option<Vec<LogEntry>>,
    pub rules: Option<Vec<Rule>>,
    pub ignored: Option<Vec<(String, String)>>,
    /// Dialoge (siehe `maint_dialogs`).
    pub dlg: super::maint_dialogs::Dialogs,
    /// Ordner der Sicherungen und der Datenordner (setzt die Anwendung).
    pub backup_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Wiederherstellung einer Sicherung/geretteten Datei anfordern (die Anwendung zeigt ihren Dialog).
    pub request_restore: Option<PathBuf>,
    /// Aus „Einstellungen → Daten“: Datenbankpflege öffnen (im nächsten Bild der Projektansicht).
    pub request_db: bool,
}

/// Farbe der Schwere.
pub(super) fn severity_color(s: Severity) -> Color32 {
    match s {
        Severity::Error => crate::style::ui().error,
        Severity::Warning => crate::style::ui().warning,
        Severity::Hint => crate::style::ui().info,
    }
}

/// Text des Knopfs der Übersicht: „Wartung (12)“ bzw. „Wartung“.
pub(super) fn button_text(count: Option<usize>, lang: Lang) -> String {
    match count.filter(|n| *n > 0) {
        Some(n) => format!("{} ({n})", lang.t("Wartung", "Maintenance")),
        None => lang.t("Wartung", "Maintenance").to_string(),
    }
}

/// Aktion zu einem Befund-Knopf.
pub(super) fn fix_label(f: Fix, lang: Lang) -> &'static str {
    match f {
        Fix::CloseWontDo => lang.t("Als Won't Do schließen …", "Close as Won't Do …"),
        Fix::Archive => lang.t("Archivieren …", "Archive …"),
        Fix::SetDue => lang.t("Fälligkeit setzen …", "Set due date …"),
        Fix::LinkDuplicate => lang.t("Vergleichen …", "Compare …"),
        Fix::Remove => lang.t("Entfernen …", "Remove …"),
        Fix::MergeLabels => lang.t("Zusammenführen …", "Merge …"),
        Fix::DetachParent => lang.t("Verweis lösen …", "Detach …"),
        Fix::ClearReference => lang.t("Aufräumen …", "Clean up …"),
        Fix::RuleAction => lang.t("Vorschlag ausführen …", "Apply suggestion …"),
        Fix::Repair => lang.t("Reparieren …", "Repair …"),
    }
}

/// Tickets, auf die eine Aktion für die Befunde wirkt.
pub(super) fn tickets_of(findings: &[&Finding]) -> Vec<i64> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for f in findings {
        let ids: Vec<i64> = match &f.target {
            Target::Tickets(ids) if f.check == Check::ParentDoneChildrenOpen => ids.clone(),
            _ => f.ticket.iter().map(|t| t.id).collect(),
        };
        for id in ids {
            if seen.insert(id) {
                v.push(id);
            }
        }
    }
    v
}

/// Gemeinsame Aktionen der gewählten Befunde (Schnittmenge, in der Reihenfolge des ersten).
pub(super) fn common_fixes(findings: &[&Finding]) -> Vec<Fix> {
    let Some(first) = findings.first() else { return Vec::new() };
    first.fixes.iter().copied().filter(|x| findings.iter().all(|f| f.fixes.contains(x))).filter(|x| *x != Fix::LinkDuplicate || findings.len() == 1).collect()
}

/// Baut die Aktion für die gewählten Befunde (ohne Eingaben wie Datum/Kommentar, die setzt der Vorschau-Dialog).
pub(super) fn build_action(fix: Fix, findings: &[&Finding], comment: String, due: String) -> Option<Action> {
    let tickets = tickets_of(findings);
    Some(match fix {
        Fix::CloseWontDo => Action::CloseWontDo { tickets, comment },
        Fix::Archive => Action::Archive { tickets },
        Fix::SetDue => Action::SetDue { tickets, due },
        Fix::Remove => Action::Remove { targets: findings.iter().map(|f| f.target.clone()).collect() },
        Fix::MergeLabels => match &findings.first()?.target {
            Target::Labels { keep, merge } => Action::MergeLabels { keep: keep.clone(), merge: merge.clone() },
            _ => return None,
        },
        Fix::DetachParent => Action::DetachParent { tickets },
        Fix::ClearReference => Action::ClearReference { tickets },
        Fix::RuleAction => {
            let rule = match &findings.first()?.target {
                Target::Rule(r) => *r,
                _ => return None,
            };
            if findings.iter().any(|f| f.target != Target::Rule(rule)) {
                return None;
            }
            Action::RuleAction { rule, tickets }
        }
        Fix::LinkDuplicate | Fix::Repair => return None,
    })
}

impl PlanUi {
    /// Aus „Einstellungen → Daten“: die Datenbankpflege der Wartungsseite anfordern (Expertenmodus wird eingeschaltet).
    pub(in crate::app) fn request_maintenance_db(&mut self) {
        self.maint.request_db = true;
    }

    /// Wiederherstellung, die die Wartungsseite angefordert hat (die Anwendung zeigt dafür ihren Dialog).
    pub(in crate::app) fn take_restore_request(&mut self) -> Option<PathBuf> {
        self.maint.request_restore.take()
    }

    /// Ordner für Sicherungen und Rettung (setzt die Anwendung jedes Bild).
    pub(in crate::app) fn set_maint_dirs(&mut self, backup_dir: PathBuf, data_dir: PathBuf) {
        self.maint.backup_dir = backup_dir;
        self.maint.data_dir = data_dir;
    }

    /// Angeforderte Datenbankpflege öffnen: erstes Experten-Projekt (oder das gewählte), Reiter Übersicht, Kategorie Datenbank.
    pub(super) fn run_maintenance_request(&mut self, cx: &Cx) {
        if !std::mem::take(&mut self.maint.request_db) {
            return;
        }
        if !self.expert {
            self.set_expert(cx.db, true);
        }
        let pick = self.project().filter(|p| !p.system).map(|p| p.id).or_else(|| self.projects.iter().find(|p| !p.system).map(|p| p.id));
        let Some(id) = pick else {
            self.note(cx.lang.t("Die Datenbankpflege gehört zu einem Experten-Projekt – lege zuerst eines an.", "Database care belongs to an expert project – create one first.").into());
            return;
        };
        self.select(cx, id);
        self.tab = super::Tab::Overview;
        self.open_maintenance(cx, id, Some(Pane::Cat(Category::Database)));
    }

    /// Wartungsseite für das Projekt öffnen (startet die volle Prüfung).
    pub(super) fn open_maintenance(&mut self, cx: &Cx, project: i64, pane: Option<Pane>) {
        let m = &mut self.maint;
        if m.project != project {
            m.report = None;
            m.selected.clear();
            m.stats = None;
            m.repair_plan = None;
            m.rescue = None;
        }
        m.open = true;
        m.project = project;
        if pane.is_some() {
            m.pane = pane;
        }
        m.log = None;
        m.rules = None;
        m.ignored = None;
        self.maint_run_check(cx);
    }

    /// Volle Prüfung im Hintergrund (eigene, nur lesende Verbindung).
    pub(super) fn maint_run_check(&mut self, cx: &Cx) {
        if self.maint.job.is_some() {
            self.maint.rerun = true;
            return;
        }
        let path = cx.db.path.clone();
        let (project, lang) = (self.maint.project, cx.lang);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = (|| -> MRes<Report> {
                let c = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
                c.busy_timeout(Duration::from_secs(10))?;
                maint::run_report(&c, project, &RunOpts::full(chrono::Local::now().naive_local(), lang))
            })();
            let _ = tx.send(Done::Report(r));
        });
        self.maint.job = Some((rx, cx.lang.t("Prüfe …", "Checking …").to_string(), Instant::now()));
    }

    /// Ergebnis eines Hintergrund-Auftrags abholen.
    pub(super) fn maint_poll(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some((rx, _, _)) = &self.maint.job else { return };
        let got = match rx.try_recv() {
            Ok(d) => d,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(150));
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.maint.job = None;
                return;
            }
        };
        self.maint.job = None;
        let lang = cx.lang;
        let mut recheck = std::mem::take(&mut self.maint.rerun);
        match got {
            Done::Report(Ok(r)) => {
                let problems = r.findings.iter().filter(|f| f.severity != Severity::Hint && f.category() != Category::Database).count();
                self.maint.counts.insert(r.project_id, (problems, Instant::now()));
                let ids: HashSet<String> = r.findings.iter().map(|f| f.id.clone()).collect();
                self.maint.selected.retain(|s| ids.contains(s));
                self.maint.report = Some(r);
            }
            Done::Report(Err(e)) => self.maint.msg = Some((e.message(lang), false)),
            Done::Action(r, label) => {
                match r {
                    Ok(o) => {
                        let mut t = match lang {
                            Lang::De => format!("{label}: {} geändert.", o.changed),
                            Lang::En => format!("{label}: {} changed.", o.changed),
                        };
                        if !o.skipped.is_empty() {
                            t.push_str(&match lang {
                                Lang::De => format!(" {} übersprungen.", o.skipped.len()),
                                Lang::En => format!(" {} skipped.", o.skipped.len()),
                            });
                        }
                        self.maint.msg = Some((t, true));
                        self.maint.selected.clear();
                    }
                    Err(e) => self.maint.msg = Some((e.message(lang), false)),
                }
                self.after_maint_write();
                recheck = true;
            }
            Done::Repair(r) => {
                self.maint.msg = Some(match r {
                    Ok(n) => (match lang {
                        Lang::De => format!("Reparatur fertig: {n} Änderung(en)."),
                        Lang::En => format!("Repair done: {n} change(s)."),
                    }, true),
                    Err(e) => (e.message(lang), false),
                });
                self.maint.repair_plan = None;
                self.after_maint_write();
                recheck = true;
            }
            Done::Plan(r) => match r {
                Ok(p) => {
                    self.maint.repair_plan = Some(p);
                    self.maint.dlg.repair = true;
                    self.maint.dlg.since = Some(Instant::now());
                }
                Err(e) => self.maint.msg = Some((e.message(lang), false)),
            },
            Done::Compact(r) => {
                self.maint.msg = Some(match r {
                    Ok(c) => (format!("{}: {} → {}", lang.t("Verdichtet", "Compacted"), dbcare::human_bytes(c.before as i64), dbcare::human_bytes(c.after as i64)), true),
                    Err(e) => (e.message(lang), false),
                });
                self.maint.stats = None;
                self.after_maint_write();
            }
            Done::Rescue(r) => match r {
                Ok(rep) => {
                    self.maint.msg = Some((format!("{} {}", lang.t("Gerettet in", "Rescued into"), rep.dest.display()), true));
                    // kein Protokolleintrag in der (beschädigten) Quelle: `rescue` schreibt ihn in die neue Datei
                    self.maint.rescue = Some(rep);
                }
                Err(e) => self.maint.msg = Some((e.message(lang), false)),
            },
            Done::Stats(r) => match r {
                Ok(s) => self.maint.stats = Some((self.maint.whole_db, s)),
                Err(e) => self.maint.msg = Some((e.message(lang), false)),
            },
        }
        if recheck && self.maint.open {
            self.maint_run_check(cx);
        }
        ctx.request_repaint();
    }

    /// Nach einer schreibenden Wartungsaktion: alle Ansichten neu laden.
    fn after_maint_write(&mut self) {
        self.dirty = true;
        self.tickets.invalidate();
        self.tickets.counts_dirty = true;
        self.tickets.reset_history();
        self.maint.log = None;
        self.maint.rules = None;
        self.maint.counts.clear();
        self.dash_invalidate();
    }

    /// Schreibender Auftrag im Hintergrund: Sperre, Sicherung „maintenance“ (höchstens drei), Aktion, Protokoll.
    pub(super) fn maint_spawn_action(&mut self, cx: &Cx, a: Action, label: String) {
        if self.maint.job.is_some() {
            return;
        }
        let (path, dir, project, lang) = (cx.db.path.clone(), self.maint.backup_dir.clone(), self.maint.project, cx.lang);
        let (tx, rx) = mpsc::channel();
        let l2 = label.clone();
        std::thread::spawn(move || {
            let _lock = maint::WriteLock::take();
            let r = (|| {
                let name = backup_first(&path, &dir)?;
                let c = open_rw(&path)?;
                maint::actions::apply(&c, project, &a, lang, Some(&name))
            })();
            let _ = tx.send(Done::Action(r, l2));
        });
        self.maint.job = Some((rx, label, Instant::now()));
    }

    pub(super) fn maint_spawn_repair(&mut self, cx: &Cx, steps: Vec<RepairStep>) {
        if self.maint.job.is_some() {
            return;
        }
        let (path, dir, project) = (cx.db.path.clone(), self.maint.backup_dir.clone(), self.maint.project);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _lock = maint::WriteLock::take();
            let r = (|| {
                let name = backup_first(&path, &dir)?;
                let c = open_rw(&path)?;
                let n = dbcare::apply_repair(&c, &steps)?;
                maint::write_log(&c, Some(project), "repair", n, Some(&name), &steps.iter().filter(|s| s.applies()).map(|s| s.text(Lang::En)).collect::<Vec<_>>().join("; "))?;
                Ok(n)
            })();
            let _ = tx.send(Done::Repair(r));
        });
        self.maint.job = Some((rx, cx.lang.t("Repariere …", "Repairing …").to_string(), Instant::now()));
    }

    /// Reparaturplan im Hintergrund erstellen (eigene, nur lesende Verbindung); das Ergebnis öffnet den Reparatur-Dialog.
    pub(super) fn maint_spawn_plan(&mut self, cx: &Cx) {
        if self.maint.job.is_some() {
            self.maint.msg = Some((cx.lang.t("Ein Wartungsauftrag läuft noch – bitte gleich noch einmal.", "A maintenance job is still running – please try again shortly.").into(), false));
            return;
        }
        let path = cx.db.path.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = (|| -> MRes<Vec<RepairStep>> {
                let c = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
                c.busy_timeout(Duration::from_secs(10))?;
                dbcare::plan_repair(&c)
            })();
            let _ = tx.send(Done::Plan(r));
        });
        self.maint.job = Some((rx, cx.lang.t("Prüfe die Datenbank …", "Checking the database …").to_string(), Instant::now()));
    }

    pub(super) fn maint_spawn_compact(&mut self, cx: &Cx) {
        if self.maint.job.is_some() {
            return;
        }
        let (path, dir, project) = (cx.db.path.clone(), self.maint.backup_dir.clone(), self.maint.project);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _lock = maint::WriteLock::take();
            let r = (|| {
                let name = backup_first(&path, &dir)?;
                let res = dbcare::compact(&path)?;
                let c = open_rw(&path)?;
                maint::write_log(&c, Some(project), "compact", 1, Some(&name), &format!("{} -> {}", dbcare::human_bytes(res.before as i64), dbcare::human_bytes(res.after as i64)))?;
                Ok(res)
            })();
            let _ = tx.send(Done::Compact(r));
        });
        self.maint.job = Some((rx, cx.lang.t("Verdichte …", "Compacting …").to_string(), Instant::now()));
    }

    pub(super) fn maint_spawn_rescue(&mut self, cx: &Cx) {
        if self.maint.job.is_some() {
            return;
        }
        let (path, dir, lang) = (cx.db.path.clone(), self.maint.data_dir.clone(), cx.lang);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Done::Rescue(dbcare::rescue(&path, &dir, lang)));
        });
        self.maint.job = Some((rx, cx.lang.t("Rette Daten …", "Rescuing data …").to_string(), Instant::now()));
    }

    pub(super) fn maint_spawn_stats(&mut self, cx: &Cx) {
        if self.maint.job.is_some() {
            return;
        }
        let (path, project) = (cx.db.path.clone(), (!self.maint.whole_db).then_some(self.maint.project));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = (|| -> MRes<Stats> {
                let c = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                dbcare::stats(&c, project)
            })();
            let _ = tx.send(Done::Stats(r));
        });
        self.maint.job = Some((rx, cx.lang.t("Zähle …", "Counting …").to_string(), Instant::now()));
    }

    /// Zahl der Probleme für den Knopf der Übersicht (günstig, mit Zwischenspeicher).
    pub(super) fn maint_count(&mut self, cx: &Cx, project: i64) -> Option<usize> {
        if let Some((n, at)) = self.maint.counts.get(&project) {
            if at.elapsed() < COUNT_TTL {
                return Some(*n);
            }
        }
        let n = maint::quick_count(cx.conn, project, chrono::Local::now().naive_local()).ok()?;
        self.maint.counts.insert(project, (n, Instant::now()));
        Some(n)
    }

    // ------------------------------------------------------------------ Seite

    pub(super) fn maint_page(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        self.maint_poll(ui.ctx(), cx);
        if proj.id != self.maint.project {
            self.open_maintenance(cx, proj.id, None);
        }
        let dialogs_open = self.maint.dlg.any_open();
        if !dialogs_open && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.maint.open = false;
            return;
        }
        // Kopf: Zurück, Titel, erneut prüfen
        ui.horizontal(|ui| {
            if ui.button(format!("← {}", lang.t("Zurück", "Back"))).on_hover_text(lang.t("Zurück zur Übersicht (Esc)", "Back to the overview (Esc)")).clicked() {
                self.maint.open = false;
            }
            ui.label(RichText::new(format!("{} · {}", lang.t("Wartung", "Maintenance"), proj.key)).heading().strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let running = self.maint.job.is_some();
                if ui.add_enabled(!running, egui::Button::new(lang.t("Erneut prüfen", "Check again"))).clicked() {
                    self.maint_run_check(cx);
                }
                if let Some((_, label, _)) = &self.maint.job {
                    ui.label(RichText::new(label.as_str()).color(style::dim(ui)));
                    ui.add(egui::Spinner::new());
                }
            });
        });
        if self.maint.job.is_some() && crate::mcp::LEVEL.load(std::sync::atomic::Ordering::Relaxed) > 0 && crate::planning::maintenance::busy().is_some() {
            hint(ui, lang.t(
                "Agenten sind verbunden: Ihre schreibenden Aufrufe warten, bis die Wartungsaktion fertig ist (Fehler „busy“).",
                "Agents are connected: their write calls wait until the maintenance action has finished (error “busy”).",
            ));
        }
        if let Some((text, ok)) = self.maint.msg.clone() {
            ui.horizontal_wrapped(|ui| {
                if ok {
                    ui.label(RichText::new(text).color(style::dim(ui)));
                } else {
                    widgets::error(ui, &text);
                }
            });
        }
        ui.add_space(S2);
        let pane = self.maint.pane.unwrap_or(Pane::Cat(Category::Tickets));
        let avail = ui.available_size();
        ui.horizontal_top(|ui| {
            // linke Spalte: Kategorien
            ui.allocate_ui_with_layout(Vec2::new(210.0, avail.y), Layout::top_down(Align::Min), |ui| {
                ui.set_width(210.0);
                for c in Category::ALL {
                    let n = self.maint.report.as_ref().map(|r| r.count(c));
                    let text = match n {
                        Some(n) => format!("{}  ({n})", c.label(lang)),
                        None => c.label(lang).to_string(),
                    };
                    if style::tab(ui, &text, pane == Pane::Cat(c)).clicked() {
                        self.maint.pane = Some(Pane::Cat(c));
                        self.maint.selected.clear();
                    }
                    ui.add_space(4.0);
                }
                ui.add_space(S2);
                let ign = self.maint.report.as_ref().map(|r| r.ignored).unwrap_or(0);
                for (p, label) in [
                    (Pane::Ignored, format!("{} ({ign})", lang.t("Ignoriert", "Ignored"))),
                    (Pane::Log, lang.t("Protokoll", "Log").to_string()),
                    (Pane::Checks, lang.t("Prüfungen", "Checks").to_string()),
                ] {
                    if style::tab(ui, &label, pane == p).clicked() {
                        self.maint.pane = Some(p);
                    }
                    ui.add_space(4.0);
                }
            });
            ui.add_space(S3);
            ui.vertical(|ui| {
                ui.set_width((avail.x - 210.0 - S3 - 8.0).max(200.0));
                egui::ScrollArea::vertical().id_salt(("maint_right", pane_salt(pane))).auto_shrink([false, false]).show(ui, |ui| match pane {
                    Pane::Cat(Category::Database) => self.maint_db_pane(ui, cx),
                    Pane::Cat(Category::Rules) => {
                        self.maint_rules_list(ui, cx, proj);
                        ui.add_space(S2);
                        self.maint_findings(ui, cx, Category::Rules);
                    }
                    Pane::Cat(c) => self.maint_findings(ui, cx, c),
                    Pane::Ignored => self.maint_ignored(ui, cx),
                    Pane::Log => self.maint_log(ui, cx),
                    Pane::Checks => self.maint_checks(ui, cx, proj),
                });
            });
        });
    }

    /// Befundliste einer Kategorie mit Mehrfachauswahl.
    fn maint_findings(&mut self, ui: &mut Ui, cx: &Cx, cat: Category) {
        let lang = cx.lang;
        let Some(report) = self.maint.report.clone() else {
            if self.maint.job.is_none() {
                hint(ui, lang.t("Noch kein Ergebnis.", "No result yet."));
            }
            return;
        };
        let list: Vec<&Finding> = report.findings.iter().filter(|f| f.category() == cat && self.maint.severity.is_none_or(|s| f.severity == s)).collect();
        // Filter und Auswahl
        ui.horizontal_wrapped(|ui| {
            let mut sev = self.maint.severity;
            widgets::radio(ui, &mut sev, None, lang.t("Alle", "All"));
            for s in Severity::ALL {
                let n = report.findings.iter().filter(|f| f.category() == cat && f.severity == s).count();
                widgets::radio(ui, &mut sev, Some(s), format!("{} ({n})", s.label(lang)));
            }
            self.maint.severity = sev;
        });
        ui.add_space(S1);
        if list.is_empty() {
            ui.add_space(S3);
            ui.label(RichText::new(lang.t("Keine Befunde – alles in Ordnung.", "No findings – all good.")).color(style::dim(ui)));
            return;
        }
        let chosen: Vec<&Finding> = list.iter().copied().filter(|f| self.maint.selected.contains(&f.id)).collect();
        ui.horizontal_wrapped(|ui| {
            let all = !list.is_empty() && list.iter().all(|f| self.maint.selected.contains(&f.id));
            let mut a = all;
            if widgets::check(ui, &mut a, lang.t("Alle wählen", "Select all")).clicked() {
                if all {
                    for f in &list {
                        self.maint.selected.remove(&f.id);
                    }
                } else {
                    for f in &list {
                        self.maint.selected.insert(f.id.clone());
                    }
                }
            }
            ui.label(RichText::new(format!("{} {}", chosen.len(), lang.t("gewählt", "selected"))).small().color(style::dim(ui)));
            let fixes = common_fixes(&chosen);
            let busy = self.maint.job.is_some();
            ui.add_enabled_ui(!chosen.is_empty() && !busy, |ui| {
                ui.menu_button(lang.t("Auswahl beheben …", "Fix selection …"), |ui| {
                    if fixes.is_empty() {
                        ui.label(RichText::new(lang.t("Keine gemeinsame Aktion für diese Auswahl.", "No common action for this selection.")).small());
                    }
                    for fx in &fixes {
                        if ui.button(fix_label(*fx, lang)).clicked() {
                            self.maint_open_fix(cx, *fx, &chosen);
                            ui.close();
                        }
                    }
                });
                if ui.button(lang.t("Auswahl ignorieren", "Ignore selection")).clicked() {
                    let ids: Vec<String> = chosen.iter().map(|f| f.id.clone()).collect();
                    let _ = maint::ignore(cx.conn, self.maint.project, &ids);
                    self.maint.selected.clear();
                    self.maint.ignored = None;
                    self.maint_run_check(cx);
                }
            });
        });
        ui.add_space(S1);
        let mut open_ticket: Option<i64> = None;
        let mut fix_one: Option<(Fix, Finding)> = None;
        let mut ignore_one: Option<String> = None;
        for f in &list {
            let id = f.id.clone();
            egui::Frame::NONE.fill(ui.visuals().faint_bg_color).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut on = self.maint.selected.contains(&id);
                    if widgets::check(ui, &mut on, "").changed() {
                        if on {
                            self.maint.selected.insert(id.clone());
                        } else {
                            self.maint.selected.remove(&id);
                        }
                    }
                    let col = severity_color(f.severity);
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.5, col);
                    ui.label(RichText::new(f.severity.label(lang)).small().color(col));
                    ui.label(RichText::new(f.check.label(lang)).small().strong());
                    if let Some(t) = &f.ticket {
                        if widgets::key_link(ui, &t.key).on_hover_text(&t.title).clicked() {
                            open_ticket = Some(t.id);
                        }
                        ui.label(RichText::new(clip(&t.title, 60)).small().color(style::dim(ui)));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(lang.t("Ignorieren", "Ignore")).clicked() {
                            ignore_one = Some(id.clone());
                        }
                        for fx in f.fixes.iter().rev() {
                            if ui.add_enabled(self.maint.job.is_none(), egui::Button::new(RichText::new(fix_label(*fx, lang)).small())).clicked() {
                                fix_one = Some((*fx, (*f).clone()));
                            }
                        }
                    });
                });
                ui.label(RichText::new(&f.message).small());
            });
            ui.add_space(4.0);
        }
        if let Some(id) = ignore_one {
            let _ = maint::ignore(cx.conn, self.maint.project, &[id]);
            self.maint.ignored = None;
            self.maint_run_check(cx);
        }
        if let Some((fx, f)) = fix_one {
            self.maint_open_fix(cx, fx, &[&f]);
        }
        if let Some(t) = open_ticket {
            self.open_ticket_anywhere(cx, t, self.maint.project);
        }
    }

    /// Ignorierte Befunde mit „Wiederherstellen“.
    fn maint_ignored(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        if self.maint.ignored.is_none() {
            self.maint.ignored = maint::ignored(cx.conn, self.maint.project).ok();
        }
        let list = self.maint.ignored.clone().unwrap_or_default();
        widgets::section(ui, lang.t("Ignorierte Befunde", "Ignored findings"));
        hint(ui, lang.t("Ignorierte Befunde erscheinen nicht mehr im Bericht (auch nicht für Agenten). Wiederherstellen holt sie zurück.", "Ignored findings no longer appear in the report (also not for agents). Restore brings them back."));
        ui.add_space(S1);
        if list.is_empty() {
            ui.label(RichText::new(lang.t("Nichts ignoriert.", "Nothing ignored.")).color(style::dim(ui)));
            return;
        }
        let mut restore: Vec<String> = Vec::new();
        if ui.button(lang.t("Alle wiederherstellen", "Restore all")).clicked() {
            restore = list.iter().map(|(id, _)| id.clone()).collect();
        }
        for (id, at) in &list {
            ui.horizontal(|ui| {
                ui.label(RichText::new(id).monospace().small());
                ui.label(RichText::new(at).small().color(style::dim(ui)));
                if ui.small_button(lang.t("Wiederherstellen", "Restore")).clicked() {
                    restore.push(id.clone());
                }
            });
        }
        if !restore.is_empty() {
            let _ = maint::unignore(cx.conn, self.maint.project, &restore);
            self.maint.ignored = None;
            self.maint_run_check(cx);
        }
    }

    /// Wartungsprotokoll mit Export.
    fn maint_log(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        if self.maint.log.is_none() {
            self.maint.log = maint::read_log(cx.conn, self.maint.project).ok();
        }
        let log = self.maint.log.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            widgets::section(ui, lang.t("Wartungsprotokoll", "Maintenance log"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!log.is_empty(), egui::Button::new(lang.t("Als CSV exportieren …", "Export as CSV …"))).clicked() {
                    if let Some(p) = crate::platform::notify::save_csv("paneldeck-maintenance-log.csv") {
                        self.maint.msg = Some(match std::fs::write(&p, maint::log_csv(&log)) {
                            Ok(()) => (format!("{} {}", lang.t("Gespeichert:", "Saved:"), p.display()), true),
                            Err(e) => (e.to_string(), false),
                        });
                    }
                }
            });
        });
        hint(ui, lang.t("Wann, was, wie viele und welche Sicherung. Das Protokoll wird nie automatisch gelöscht.", "When, what, how many and which backup. The log is never deleted automatically."));
        ui.add_space(S1);
        if log.is_empty() {
            ui.label(RichText::new(lang.t("Noch keine Wartungsaktionen.", "No maintenance actions yet.")).color(style::dim(ui)));
            return;
        }
        egui::Grid::new("maint_log").striped(true).num_columns(4).spacing(Vec2::new(14.0, 6.0)).show(ui, |ui| {
            for h in [lang.t("Zeit", "Time"), lang.t("Aktion", "Action"), lang.t("Anzahl", "Count"), lang.t("Sicherung", "Backup")] {
                ui.label(RichText::new(h).small().strong());
            }
            ui.end_row();
            for e in &log {
                ui.label(RichText::new(e.at.replace('T', " ")).small());
                ui.label(RichText::new(maint::action_label(&e.action, lang)).small()).on_hover_text(&e.detail);
                ui.label(RichText::new(e.count.to_string()).small());
                ui.label(RichText::new(e.backup.clone().unwrap_or_else(|| "–".into())).small().monospace());
                ui.end_row();
            }
        });
    }

    /// Prüfungen ein-/ausschalten (je Projekt) und Verweis auf die Schwellen in den Projekt-Einstellungen.
    fn maint_checks(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let mut s = maint::load_settings(cx.conn, proj.id).unwrap_or_default();
        widgets::section(ui, lang.t("Prüfungen dieses Projekts", "Checks of this project"));
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(match lang {
                Lang::De => format!("Schwellen: Tickets {} Tage, Felder {} Tage, Duplikate {} %.", s.stale_days, s.field_days, s.duplicate_percent),
                Lang::En => format!("Thresholds: tickets {} days, fields {} days, duplicates {}%.", s.stale_days, s.field_days, s.duplicate_percent),
            }).small());
            if ui.link(lang.t("Projekt-Einstellungen …", "Project settings …")).clicked() {
                self.open_settings(proj);
            }
        });
        ui.add_space(S1);
        let mut changed = false;
        for cat in [Category::Tickets, Category::Configuration] {
            ui.label(RichText::new(cat.label(lang)).strong());
            for c in Check::ALL.into_iter().filter(|c| c.category() == cat && c.switchable()) {
                let mut on = s.enabled(c);
                if widgets::check(ui, &mut on, c.label(lang)).changed() {
                    if on {
                        s.disabled.retain(|x| *x != c);
                    } else {
                        s.disabled.push(c);
                    }
                    changed = true;
                }
            }
            ui.add_space(S1);
        }
        hint(ui, lang.t("Datenbank-Prüfungen laufen immer; eigene Regeln haben je Regel einen Schalter.", "Database checks always run; custom rules have a switch per rule."));
        if changed {
            let _ = maint::save_settings(cx.conn, proj.id, &s);
            self.maint.counts.remove(&proj.id);
            self.maint_run_check(cx);
        }
    }

    /// Eigene Regeln: Liste mit Schalter, Bearbeiten, Löschen, Neu.
    fn maint_rules_list(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        if self.maint.rules.is_none() {
            self.maint.rules = maint::rules::list_rules(cx.conn, proj.id).ok();
        }
        let rules = self.maint.rules.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            widgets::section(ui, lang.t("Eigene Prüfregeln", "Custom check rules"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("+ {}", lang.t("Neue Regel", "New rule"))).clicked() {
                    self.maint.dlg.rule = Some(super::maint_dialogs::RuleForm::new(None));
                }
            });
        });
        if rules.is_empty() {
            hint(ui, lang.t(
                "Noch keine Regeln – sie sind aus, bis du eine anlegst. Eine Regel ist eine gespeicherte Abfrage (z. B. „type = Bug AND priority IS EMPTY“) mit Schwere und Vorschlag.",
                "No rules yet – they are off until you create one. A rule is a saved query (e.g. “type = Bug AND priority IS EMPTY”) with a severity and a suggestion.",
            ));
            return;
        }
        let mut toggle: Option<(i64, bool)> = None;
        for r in &rules {
            ui.horizontal(|ui| {
                let mut on = r.enabled;
                if style::toggle(ui, &mut on).changed() {
                    toggle = Some((r.id, on));
                }
                let col = severity_color(r.severity);
                ui.label(RichText::new(r.severity.label(lang)).small().color(col));
                ui.label(RichText::new(&r.name).strong());
                ui.label(RichText::new(clip(&r.jql, 60)).monospace().small().color(style::dim(ui)));
                ui.label(RichText::new(maint::rules::action_text(cx.conn, &r.action, lang)).small().color(style::dim(ui)));
                if ui.small_button(lang.t("Bearbeiten …", "Edit …")).clicked() {
                    self.maint.dlg.rule = Some(super::maint_dialogs::RuleForm::new(Some(r)));
                }
            });
        }
        if let Some((id, on)) = toggle {
            let _ = maint::rules::set_enabled(cx.conn, id, on);
            self.maint.rules = None;
            self.maint_run_check(cx);
        }
    }

    /// Datenbankpflege: Prüfen, Statistik, Verdichten, Reparieren, Retten, Sicherung wiederherstellen.
    fn maint_db_pane(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let busy = self.maint.job.is_some();
        widgets::section(ui, lang.t("Datenbankpflege", "Database care"));
        hint(ui, lang.t(
            "Prüfen und Statistik lesen nur. Vor Reparatur und Verdichten legt Paneldeck eine Sicherung „maintenance“ an (höchstens drei). Die Rettung schreibt immer in eine neue Datei.",
            "Checking and statistics only read. Before repairing and compacting Paneldeck makes a “maintenance” backup (at most three). The rescue always writes a new file.",
        ));
        ui.add_space(S1);
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if ui.button(lang.t("Datenbank prüfen", "Check database")).clicked() {
                    self.maint_run_check(cx);
                }
                if ui.button(lang.t("Statistik", "Statistics")).clicked() {
                    self.maint_spawn_stats(cx);
                }
                if ui.button(lang.t("Reparieren …", "Repair …")).clicked() {
                    self.maint_spawn_plan(cx);
                }
                if ui.button(lang.t("Verdichten …", "Compact …")).clicked() {
                    self.maint.dlg.compact = true;
                }
                if ui.button(lang.t("In neue Datei retten …", "Rescue into a new file …")).clicked() {
                    self.maint.dlg.rescue = true;
                }
                if ui.button(lang.t("Letzte Sicherung wiederherstellen …", "Restore the last backup …")).clicked() {
                    match crate::backup::list(&self.maint.backup_dir).into_iter().next() {
                        Some(b) => self.maint.request_restore = Some(b.path),
                        None => self.maint.msg = Some((lang.t("Es gibt noch keine Sicherung.", "There is no backup yet.").into(), false)),
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            let mut w = self.maint.whole_db;
            if widgets::check(ui, &mut w, lang.t("Statistik für die ganze Datenbank", "Statistics for the whole database")).changed() {
                self.maint.whole_db = w;
                self.maint.stats = None;
            }
        });
        if let Some((whole, s)) = self.maint.stats.clone() {
            ui.add_space(S1);
            widgets::card(ui, |ui| stats_ui(ui, &s, whole, lang));
        }
        if let Some(rep) = self.maint.rescue.clone() {
            ui.add_space(S1);
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Rettung", "Rescue")).strong());
                ui.label(RichText::new(rep.text(lang)).small().monospace());
                if ui.button(lang.t("Gerettete Datei verwenden …", "Use the rescued file …")).on_hover_text(lang.t("Wie eine Sicherung wiederherstellen: der aktuelle Stand wird vorher gesichert, Paneldeck startet danach neu.", "Restored like a backup: the current data is backed up first, then Paneldeck restarts.")).clicked() {
                    self.maint.request_restore = Some(rep.dest.clone());
                }
            });
        }
        ui.add_space(S2);
        self.maint_findings(ui, cx, Category::Database);
    }
}

fn pane_salt(p: Pane) -> u8 {
    match p {
        Pane::Cat(c) => c as u8,
        Pane::Ignored => 10,
        Pane::Log => 11,
        Pane::Checks => 12,
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

fn stats_ui(ui: &mut Ui, s: &Stats, whole: bool, lang: Lang) {
    ui.label(RichText::new(if whole { lang.t("Ganze Datenbank", "Whole database") } else { lang.t("Dieses Projekt", "This project") }).strong());
    egui::Grid::new("maint_stats").num_columns(2).spacing(Vec2::new(18.0, 4.0)).show(ui, |ui| {
        let row = |ui: &mut Ui, k: &str, v: String| {
            ui.label(RichText::new(k).small());
            ui.label(RichText::new(v).small().monospace());
            ui.end_row();
        };
        row(ui, lang.t("Dateigröße (ganze Datei)", "File size (whole file)"), format!("{} ({} {})", dbcare::human_bytes(s.file_bytes), dbcare::human_bytes(s.free_bytes), lang.t("frei", "free")));
        row(ui, lang.t("Daten des Bereichs (ungefähr)", "Data of the scope (approx.)"), dbcare::human_bytes(s.data_bytes));
        row(ui, lang.t("Tickets (davon archiviert)", "Tickets (archived)"), format!("{} ({})", s.tickets, s.archived));
        row(ui, lang.t("Kommentare", "Comments"), s.comments.to_string());
        row(ui, lang.t("Verlaufseinträge", "History entries"), s.history.to_string());
        row(ui, lang.t("Zeiteinträge", "Time entries"), s.time_entries.to_string());
        row(ui, lang.t("Verknüpfungen", "Links"), s.links.to_string());
        row(ui, lang.t("Feldwerte", "Field values"), s.values.to_string());
    });
    if !s.largest.is_empty() {
        ui.add_space(S1);
        ui.label(RichText::new(lang.t("Größte Tickets", "Largest tickets")).small().strong());
        for (k, t, b) in &s.largest {
            ui.label(RichText::new(format!("{k}  {}  {}", dbcare::human_bytes(*b), clip(t, 50))).small());
        }
    }
    if !s.growth.is_empty() {
        ui.add_space(S1);
        ui.label(RichText::new(lang.t("Wachstum (neue Tickets je Monat)", "Growth (new tickets per month)")).small().strong());
        ui.label(RichText::new(s.growth.iter().map(|(m, n)| format!("{m}: {n}")).collect::<Vec<_>>().join(" · ")).small());
    }
}

/// Sicherung „maintenance“ vor einer Aktion (höchstens drei bleiben). Liefert den Dateinamen.
fn backup_first(path: &std::path::Path, dir: &std::path::Path) -> MRes<String> {
    let b = crate::backup::create(path, dir, crate::backup::Kind::PreMaintenance).map_err(|e| maint::MaintError::refused(format!("Sicherung fehlgeschlagen, nichts geändert: {e}"), format!("Backup failed, nothing changed: {e}")))?;
    crate::backup::rotate_maintenance(dir);
    Ok(b.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
}

fn open_rw(path: &std::path::Path) -> MRes<rusqlite::Connection> {
    let c = rusqlite::Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(10))?;
    c.pragma_update(None, "foreign_keys", true)?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_text_counts_only_problems() {
        assert_eq!(button_text(Some(12), Lang::En), "Maintenance (12)");
        assert_eq!(button_text(Some(0), Lang::De), "Wartung");
        assert_eq!(button_text(None, Lang::En), "Maintenance");
    }

    #[test]
    fn actions_for_a_selection() {
        let m = crate::planning::maintenance::fixture::build();
        let r = maint::run_report(&m.fx.c, m.fx.project, &RunOpts::full(m.now, Lang::En)).unwrap();
        let stale: Vec<&Finding> = r.findings.iter().filter(|f| matches!(f.check, Check::StaleNew | Check::StaleInProgress)).collect();
        assert_eq!(common_fixes(&stale), vec![Fix::CloseWontDo, Fix::Archive, Fix::SetDue]);
        let Some(Action::Archive { mut tickets }) = build_action(Fix::Archive, &stale, String::new(), String::new()) else { panic!("Aktion") };
        tickets.sort_unstable();
        assert_eq!(tickets, vec![m.id(1), m.id(2)]);
        let parent: Vec<&Finding> = r.findings.iter().filter(|f| f.check == Check::ParentDoneChildrenOpen).collect();
        assert_eq!(tickets_of(&parent), vec![m.id(8)], "die offenen Kinder, nicht das erledigte Eltern-Ticket");
        let mixed: Vec<&Finding> = r.findings.iter().filter(|f| matches!(f.check, Check::StaleNew | Check::OrphanedSubtask)).collect();
        assert!(common_fixes(&mixed).is_empty());
        for f in Check::ALL.iter().flat_map(|_| [Fix::CloseWontDo, Fix::Archive, Fix::SetDue, Fix::LinkDuplicate, Fix::Remove, Fix::MergeLabels, Fix::DetachParent, Fix::ClearReference, Fix::RuleAction, Fix::Repair]) {
            assert!(!fix_label(f, Lang::De).is_empty() && !fix_label(f, Lang::En).is_empty());
        }
    }
}
