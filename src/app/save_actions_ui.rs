//! Aktionen beim Speichern (#383) in der App: der eine Haken beim Speichern ([`App::editor_save_with`] – Text-Aktionen
//! der Regeln, auch *Formatieren* (#381), vor dem Schreiben, danach die übrigen Aktionen in die Warteschlange), die Laufzeit der Befehle (Pseudokonsole ohne
//! Fenster in einem eigenen Job Object wie die Prozessliste, wahlweise sichtbar als Terminal in der Seitenleiste),
//! Kopieren im Hilfsthread, Paneldeck-Aktionen, Verlauf (letzte 100 Läufe, nur im Speicher), Hinweise/Benachrichtigungen,
//! Rückfrage beim Beenden und die MCP-Auskünfte. Oberfläche (Einstellungen, Regel-Dialog, Ausgabe): `save_actions_dialog`.
//! Logik ohne egui: `crate::save_actions`.

use super::*;
use crate::procs;
use crate::save_actions::life::{CmdLife, Outcome};
use crate::save_actions::matching::{self, Plan, PlannedRule, Switches};
use crate::save_actions::queue::{Job, Queue};
use crate::save_actions::{self as sa, vars, ActionKind, Builtin, Config, Rule, SaveOrigin};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Receiver;

/// Kennung der Toast-Knöpfe *Ausgabe zeigen* (Bit 59, Rest = Lauf). Bit 63 = „Alle beenden“, Bits 60–62 = „Freigeben …“ (#386).
pub(super) const SA_TOKEN: u64 = 1 << 59;

/// Zustand eines Laufs im Verlauf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunStatus {
    Running,
    Ok,
    Failed,
    TimeLimit,
    Cancelled,
    /// Übersprungen (z. B. Sitzung nicht am Prompt, Rest der Regel nach einem Fehler).
    Skipped,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Ok => "ok",
            RunStatus::Failed => "failed",
            RunStatus::TimeLimit => "time_limit",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Skipped => "skipped",
        }
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            RunStatus::Running => lang.t("läuft", "running"),
            RunStatus::Ok => lang.t("erfolgreich", "succeeded"),
            RunStatus::Failed => lang.t("fehlgeschlagen", "failed"),
            RunStatus::TimeLimit => lang.t("Zeitlimit", "time limit"),
            RunStatus::Cancelled => lang.t("abgebrochen", "cancelled"),
            RunStatus::Skipped => lang.t("übersprungen", "skipped"),
        }
    }

    pub fn error(self) -> bool {
        matches!(self, RunStatus::Failed | RunStatus::TimeLimit)
    }
}

/// Ein Eintrag im Ausgabebereich (eine Aktion eines Laufs).
#[derive(Clone, Debug)]
pub(super) struct RunRecord {
    pub id: u64,
    /// Sekunden seit 1970.
    pub at: u64,
    pub file: PathBuf,
    pub rule: u64,
    pub rule_name: String,
    /// Art der Aktion (Anzeige).
    pub action: String,
    /// Befehlszeile für die Anzeige (geheime Werte als •••) bzw. Ziel.
    pub command: String,
    pub status: RunStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: u64,
    pub output: Vec<String>,
    /// Hinweis/Fehlermeldung.
    pub message: Option<String>,
    started: Option<Instant>,
}

/// Laufender Schritt einer Aktion.
enum Step {
    /// Befehl in einer Pseudokonsole ohne Fenster.
    /// `batch`: Batch-Datei des Laufs (cmd mit Laufzeit-Variablen), gelöscht, sobald der Schritt verworfen wird.
    Hidden { session: Session, life: CmdLife, flavor: procs::Flavor, record: u64, exit_seen: Option<Instant>, batch: Option<sa::batch::TempBatch> },
    /// Befehl in einem sichtbaren Terminal der Seitenleiste.
    Visible { entry: u64, life: CmdLife, flavor: procs::Flavor, record: u64, exit_seen: Option<Instant>, batch: Option<sa::batch::TempBatch> },
    /// Kopieren im Hilfsthread.
    Copy { rx: Receiver<Result<u64, String>>, record: u64 },
}

/// Ein laufender Lauf (eine Datei).
struct Active {
    job: Job,
    /// Regel und Aktion, die als Nächstes dran sind.
    rule_ix: usize,
    action_ix: usize,
    /// Die aktuelle Regel hatte einen Fehler (übrige Aktionen ohne „Bei Fehler weiter“ entfallen).
    rule_failed: bool,
    step: Option<Step>,
    /// Fehler in diesem Lauf.
    errors: usize,
    /// *Abbrechen* für den ganzen Lauf (und wann).
    cancelled: bool,
    cancel_at: Option<Instant>,
    /// Verlaufseinträge dieses Laufs und das schlechteste Ergebnis je Regel.
    records: Vec<u64>,
    results: HashMap<u64, RunStatus>,
}

/// Rang eines Ergebnisses (höher = schlechter) für das Gesamtergebnis einer Regel.
fn rank(s: RunStatus) -> u8 {
    match s {
        RunStatus::Running => 0,
        RunStatus::Ok => 1,
        RunStatus::Skipped => 2,
        RunStatus::Cancelled => 3,
        RunStatus::Failed | RunStatus::TimeLimit => 4,
    }
}

/// Was die Statusleiste des Editors zeigt (von der App jedes Bild gesetzt, `EditorUi::on_save`).
#[derive(Clone, Debug, Default)]
pub(crate) struct OnSaveView {
    /// Laufende und wartende Läufe.
    pub running: usize,
    /// Dateien (Pfad-Schlüssel), deren letzter Lauf einen Fehler hatte: roter Punkt im Reiter.
    pub failed: std::collections::HashSet<String>,
    /// Letztes Ergebnis je Datei (Pfad-Schlüssel → Fehler?).
    pub last: HashMap<String, bool>,
    pub paused: bool,
    /// Klick in der Statusleiste bzw. im Baum (von der App ausgewertet).
    pub clicked: Option<OnSaveClick>,
}

/// Klicks aus der Oberfläche des Editors.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OnSaveClick {
    /// Ausgabebereich öffnen.
    Output,
    /// Pause aufheben.
    Resume,
    /// Regeln für diese Datei zeigen (Test-Ansicht).
    RulesFor(PathBuf),
    /// Neue Regel für diese Datei bzw. diesen Ordner.
    NewRule(PathBuf, bool),
}

/// Zustand der Aktionen beim Speichern in der App.
#[derive(Default)]
pub(super) struct SaState {
    pub cfg: Config,
    meta: sa::FileMeta,
    load_notice: Option<String>,
    /// Bis zum Neustart pausiert.
    pub paused: bool,
    /// Das nächste Speichern läuft ohne Aktionen (*Ohne Aktionen speichern*).
    pub without_next: bool,
    queue: Queue,
    active: Vec<Active>,
    pub history: VecDeque<RunRecord>,
    next_record: u64,
    /// Letztes Ergebnis je Regel (Status, Zeit) – für die Liste und `list_save_actions`.
    pub last: HashMap<u64, (RunStatus, u64)>,
    /// Letztes Ergebnis je Datei (Pfad-Schlüssel → Fehler?).
    pub file_failed: HashMap<String, bool>,
    /// Bericht des letzten Speicherns (für `editor_save`).
    pub last_report: Option<Value>,
    logs: HashMap<u64, procs::LogFile>,
    /// Oberfläche (Dialoge, Ausgabe).
    pub ui: super::save_actions_dialog::SaUi,
    /// Rückfrage beim Beenden.
    pub quit: procs_ui::QuitState,
    /// Speichern im Hintergrund (WSL-Dateien, #392): was nach dem Schreiben noch kommt, je Dokument.
    pub pending_writes: HashMap<u64, AfterWrite>,
}

/// Der Teil von [`App::editor_save_with`] nach dem Schreiben (bei WSL-Dateien erst, wenn der Hilfsthread fertig ist).
pub(super) struct AfterWrite {
    plan: Plan,
    builtins: Vec<(Builtin, Result<Option<String>, String>)>,
    target: Option<PathBuf>,
    origin: SaveOrigin,
}

impl SaState {
    pub fn load(dir: &Path) -> SaState {
        let (cfg, meta, load_notice) = sa::load(dir);
        SaState { cfg, meta, load_notice, ..Default::default() }
    }

    /// Speichern der Regeln (atomar); Fehler als Text.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        sa::save(dir, &self.cfg, &self.meta).map_err(|e| e.to_string())
    }

    pub fn locked(&self) -> Option<&str> {
        self.meta.locked.as_deref()
    }

    /// Laufen oder warten Läufe (für die Rückfrage beim Beenden) – auch zwischen zwei Schritten?
    pub fn any_active(&self) -> bool {
        !self.active.is_empty() || self.queue.waiting() > 0
    }

    /// Laufende und wartende Läufe.
    pub fn busy(&self) -> usize {
        self.active.len() + self.queue.waiting()
    }

    fn record(&mut self, r: RunRecord) -> u64 {
        let id = r.id;
        self.history.push_front(r);
        // Die ältesten abgeschlossenen fallen weg – laufende nie (ihr Ergebnis kommt noch).
        while self.history.len() > sa::HISTORY_RUNS {
            match self.history.iter().rposition(|x| x.status != RunStatus::Running) {
                Some(i) => {
                    self.history.remove(i);
                }
                None => break,
            }
        }
        id
    }

    fn rec_mut(&mut self, id: u64) -> Option<&mut RunRecord> {
        self.history.iter_mut().find(|r| r.id == id)
    }

    fn new_record(&mut self, file: &Path, rule: &PlannedRule, action: String, command: String) -> u64 {
        self.next_record += 1;
        let r = RunRecord {
            id: self.next_record,
            at: sa::now_secs(),
            file: file.to_path_buf(),
            rule: rule.rule,
            rule_name: rule.name.clone(),
            action,
            command,
            status: RunStatus::Running,
            exit_code: None,
            duration_ms: 0,
            output: Vec::new(),
            message: None,
            started: Some(Instant::now()),
        };
        self.record(r)
    }

    /// Laufender Befehl zu einem Verlaufseintrag (für die Live-Ausgabe).
    pub fn live_lines(&self, record: u64) -> Option<Vec<String>> {
        for a in &self.active {
            if let Some(Step::Hidden { session, record: r, .. }) = &a.step
                && *r == record
            {
                return Some(session_lines(session));
            }
        }
        None
    }

    /// Ist dieser Verlaufseintrag ein laufender Befehl (kann abgebrochen werden)?
    pub fn cancellable(&self, record: u64) -> bool {
        self.active.iter().any(|a| match &a.step {
            Some(Step::Hidden { record: r, .. }) | Some(Step::Visible { record: r, .. }) => *r == record,
            _ => false,
        })
    }
}

/// Ganze Ausgabe einer Pseudokonsole (höchstens die letzten [`sa::OUTPUT_LINES`] Zeilen, ohne leere am Ende).
fn session_lines(s: &Session) -> Vec<String> {
    let base = s.hist_base();
    let cur = s.abs_cursor_line();
    let mut lines = s.lines_abs(base, cur);
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.len() > sa::OUTPUT_LINES {
        lines.drain(..lines.len() - sa::OUTPUT_LINES);
    }
    lines
}

/// Werte der eigenen Umgebungsvariablen einer Regel (entschlüsselt) und die Namen, die sich nicht entschlüsseln ließen.
fn rule_env(rule: &Rule) -> (Vec<(String, String)>, Vec<String>) {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for e in &rule.env {
        match e.value_protected.as_deref().map(crate::llm::secret::unprotect) {
            Some(Ok(v)) => ok.push((e.name.clone(), v)),
            _ => bad.push(e.name.clone()),
        }
    }
    (ok, bad)
}

/// Ein vorbereiteter Befehl.
pub(super) struct Prepared {
    pub shell: ShellSpec,
    pub flavor: procs::Flavor,
    pub spec: ShellSpec,
    pub env: Vec<(String, String)>,
    /// Startordner: Windows-Ordner, bei `linux_cd` der Linux-Pfad (nur zur Anzeige, er steht als `--cd` in `spec`).
    pub cwd: String,
    /// WSL startet über `wsl.exe --cd` im Linux-Ordner (#392).
    pub linux_cd: bool,
    /// Befehlszeile für die Anzeige (geheime Werte als •••).
    pub display: String,
    /// cmd mit `%PANELDECK_…%` oder geheimen `%NAME%`: Zeile für eine Batch-Datei (`sa::batch`), die cmd beim Ausführen
    /// aus der Umgebung ersetzt. Angelegt wird die Datei erst beim Start ([`App::sa_spawn`]).
    pub batch: Option<String>,
    /// Hinweise (nicht entschlüsselbare Variablen, Variablen in Anführungszeichen).
    pub notes: Vec<String>,
}

impl App {
    /// Je Bild im Hauptfenster: Hinweis beim Laden, wartende Läufe starten, laufende weiterführen.
    pub(super) fn poll_save_actions(&mut self, ctx: &egui::Context) {
        if let Some(n) = self.save_actions.load_notice.take() {
            self.push_toast(n, toasts::ToastKind::Warning, None);
        }
        let limit = self.save_actions.cfg.parallel();
        for job in self.save_actions.queue.take_ready(limit) {
            self.save_actions.active.push(Active { job, rule_ix: 0, action_ix: 0, rule_failed: false, step: None, errors: 0, cancelled: false, cancel_at: None, records: Vec::new(), results: HashMap::new() });
        }
        let mut i = 0;
        let mut waiting = false;
        while i < self.save_actions.active.len() {
            let done = self.sa_advance(ctx, i);
            if done {
                let a = self.save_actions.active.remove(i);
                self.sa_job_done(a);
            } else {
                waiting = true;
                i += 1;
            }
        }
        if waiting || self.save_actions.queue.waiting() > 0 {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.save_actions.quit == procs_ui::QuitState::Stopping && !self.save_actions.any_active() {
            self.save_actions.quit = procs_ui::QuitState::None;
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
    }

    /// Schritt eines Laufs weiterführen; `true` = der Lauf ist fertig.
    fn sa_advance(&mut self, ctx: &egui::Context, i: usize) -> bool {
        // Bis zu 16 sofort fertige Aktionen je Bild (Paneldeck-Aktionen), dann wieder im nächsten.
        for _ in 0..16 {
            if self.sa_poll_step(ctx, i) {
                return false; // läuft noch
            }
            let a = &mut self.save_actions.active[i];
            let Some(rule) = a.job.rules.get(a.rule_ix).cloned() else { return true };
            // Fehler der Text-Aktionen dieser Regel (vor dem Schreiben).
            if a.action_ix == 0 && a.job.failed_rules.iter().any(|(r, _)| *r == rule.rule) {
                a.rule_failed = true;
            }
            let Some(action) = rule.post.get(a.action_ix).cloned() else {
                a.rule_ix += 1;
                a.action_ix = 0;
                a.rule_failed = false;
                continue;
            };
            a.action_ix += 1;
            let file = a.job.file.clone();
            if a.cancelled {
                continue;
            }
            if a.rule_failed && !action.continue_on_error {
                let label = action.kind.label(self.settings.language);
                let id = self.sa_new_rec(i, &file, &rule, label, String::new());
                let msg = self.settings.language.t("Nach einem Fehler in dieser Regel übersprungen", "Skipped after an error in this rule").to_string();
                self.sa_finish_record(id, RunStatus::Skipped, None, Vec::new(), Some(msg));
                continue;
            }
            self.sa_start_action(ctx, i, &rule, &action, &file);
        }
        false
    }

    /// Laufenden Schritt prüfen; `true` = läuft noch.
    fn sa_poll_step(&mut self, ctx: &egui::Context, i: usize) -> bool {
        let now = Instant::now();
        let lang = self.settings.language;
        let Some(step) = self.save_actions.active[i].step.take() else { return false };
        match step {
            Step::Copy { rx, record } => match rx.try_recv() {
                Ok(Ok(bytes)) => {
                    let msg = format!("{bytes} {}", lang.t("Bytes kopiert", "bytes copied"));
                    self.sa_finish_record(record, RunStatus::Ok, None, Vec::new(), Some(msg));
                    false
                }
                Ok(Err(e)) => {
                    self.sa_action_failed(i);
                    self.sa_finish_record(record, RunStatus::Failed, None, Vec::new(), Some(e));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Abgebrochen: höchstens 5 s auf das Ende warten (atomar – es bleibt kein halbes Ziel), dann gilt es
                    // als abgebrochen.
                    if self.save_actions.active[i].cancel_at.is_some_and(|t| t.elapsed() >= procs::STOP_GRACE) {
                        self.sa_action_failed(i);
                        self.sa_finish_record(record, RunStatus::Cancelled, None, Vec::new(), Some(lang.t("Abgebrochen", "Cancelled").into()));
                        return false;
                    }
                    self.save_actions.active[i].step = Some(Step::Copy { rx, record });
                    true
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.sa_action_failed(i);
                    self.sa_finish_record(record, RunStatus::Failed, None, Vec::new(), Some("copy thread ended".into()));
                    false
                }
            },
            Step::Hidden { session, mut life, flavor, record, mut exit_seen, batch } => {
                if self.save_actions.active[i].cancelled {
                    let fx = life.cancel(now);
                    sa_effects(&session, fx);
                }
                let fx = life.tick(now);
                sa_effects(&session, fx);
                let (exited, code) = (session.has_exited(), session.exit_code());
                let finished = if exited {
                    let first = *exit_seen.get_or_insert(now);
                    code.is_some() || now.duration_since(first) >= Duration::from_secs(1)
                } else {
                    false
                };
                if finished || life.done() {
                    let code = code.map(procs::signed_code);
                    let outcome = if finished { life.exited(flavor, code) } else { life.outcome(flavor, None) };
                    if session.job_has_leftovers() {
                        session.kill_tree();
                    }
                    let lines = session_lines(&session);
                    self.sa_command_done(i, record, outcome, code, lines);
                    false
                } else {
                    self.save_actions.active[i].step = Some(Step::Hidden { session, life, flavor, record, exit_seen, batch });
                    let _ = ctx;
                    true
                }
            }
            Step::Visible { entry, mut life, flavor, record, mut exit_seen, batch } => {
                let Some(s) = self.entries.get(&entry).and_then(|e| e.live.as_ref()) else {
                    // Terminal geschlossen: gilt als abgebrochen.
                    let outcome = life.outcome(flavor, None);
                    let outcome = if outcome == Outcome::Ok || matches!(outcome, Outcome::Failed(_)) { Outcome::Cancelled } else { outcome };
                    self.sa_command_done(i, record, outcome, None, Vec::new());
                    return false;
                };
                if self.save_actions.active[i].cancelled {
                    let fx = life.cancel(now);
                    sa_effects(s, fx);
                }
                let fx = life.tick(now);
                sa_effects(s, fx);
                let (exited, code) = (s.has_exited(), s.exit_code());
                let finished = if exited {
                    let first = *exit_seen.get_or_insert(now);
                    code.is_some() || now.duration_since(first) >= Duration::from_secs(1)
                } else {
                    false
                };
                if finished || life.done() {
                    let code = code.map(procs::signed_code);
                    let outcome = if finished { life.exited(flavor, code) } else { life.outcome(flavor, None) };
                    let lines = session_lines(s);
                    self.sa_command_done(i, record, outcome, code, lines);
                    false
                } else {
                    self.save_actions.active[i].step = Some(Step::Visible { entry, life, flavor, record, exit_seen, batch });
                    true
                }
            }
        }
    }

    /// Verlaufseintrag für einen laufenden Lauf (Ergebnis zählt für seine Regel).
    fn sa_new_rec(&mut self, i: usize, file: &Path, rule: &PlannedRule, action: String, command: String) -> u64 {
        let id = self.save_actions.new_record(file, rule, action, command);
        self.save_actions.active[i].records.push(id);
        id
    }

    fn sa_action_failed(&mut self, i: usize) {
        let a = &mut self.save_actions.active[i];
        a.rule_failed = true;
        a.errors += 1;
    }

    /// Befehl beendet: Verlauf, Protokolldatei, bei Fehler Hinweis.
    fn sa_command_done(&mut self, i: usize, record: u64, outcome: Outcome, code: Option<i64>, lines: Vec<String>) {
        let lang = self.settings.language;
        let (status, msg) = match outcome {
            Outcome::Ok => (RunStatus::Ok, None),
            Outcome::Failed(c) => (RunStatus::Failed, Some(match c {
                Some(c) => format!("{} {c}", lang.t("Exit-Code", "exit code")),
                None => lang.t("Exit-Code unbekannt", "exit code unknown").to_string(),
            })),
            Outcome::NotFound => (RunStatus::Failed, Some(lang.t("Befehl nicht gefunden", "command not found").to_string())),
            Outcome::TimeLimit => (RunStatus::TimeLimit, Some(lang.t("Zeitlimit erreicht – Strg+C, danach der Prozessbaum beendet", "Time limit reached – Ctrl+C, then the process tree was ended").to_string())),
            Outcome::Cancelled => (RunStatus::Cancelled, Some(lang.t("Abgebrochen", "Cancelled").to_string())),
        };
        if status.error() || status == RunStatus::Cancelled {
            self.sa_action_failed(i);
        }
        // Protokolldatei der Regel.
        let rule = self.save_actions.history.iter().find(|r| r.id == record).map(|r| r.rule);
        if let Some(rule) = rule.and_then(|id| self.save_actions.cfg.rule(id).cloned())
            && rule.log_file
        {
            let dir = self.dir.join(sa::LOG_DIR);
            let log = self.save_actions.logs.entry(rule.id).or_insert_with(|| procs::LogFile::new(dir.join(procs::log_name(rule.id, &rule.display_name()))));
            let head = self.save_actions.history.iter().find(|r| r.id == record).map(|r| format!("== {} – {} → {}", r.file.display(), r.command, status.as_str())).unwrap_or_default();
            let mut all = vec![head];
            all.extend(lines.iter().cloned());
            log.write_lines(&all);
        }
        self.sa_finish_record(record, status, code, lines, msg);
    }

    /// Verlaufseintrag abschließen; Fehler → roter Hinweis mit *Ausgabe zeigen* (ohne Fokus auch von Windows).
    fn sa_finish_record(&mut self, record: u64, status: RunStatus, code: Option<i64>, lines: Vec<String>, msg: Option<String>) {
        let lang = self.settings.language;
        let Some(r) = self.save_actions.rec_mut(record) else { return };
        r.status = status;
        r.exit_code = code;
        r.duration_ms = r.started.take().map(|s| s.elapsed().as_millis() as u64).unwrap_or(0);
        r.output = lines;
        r.message = msg;
        let (rule, name, file, action) = (r.rule, r.rule_name.clone(), r.file.clone(), r.action.clone());
        let detail = r.message.clone().unwrap_or_default();
        if let Some(a) = self.save_actions.active.iter_mut().find(|a| a.records.contains(&record)) {
            let e = a.results.entry(rule).or_insert(RunStatus::Ok);
            if rank(status) > rank(*e) {
                *e = status;
            }
        }
        if status.error() {
            let fname = file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let text = format!("{} {fname} – {name}: {action} {} ({detail})", lang.t("Beim Speichern:", "On save:"), status.label(lang));
            if !self.seen_focus().0 && self.settings.notify_toast {
                notify::toast("Paneldeck", &text, self.settings.notify_sound);
            }
            let act = toasts::ToastAction { label: lang.t("Ausgabe zeigen", "Show output").to_string(), token: SA_TOKEN | record, window: self.wins.cur };
            self.push_toast_action(text, toasts::ToastKind::Error, None, Some(act));
        }
    }

    /// Lauf fertig: Ergebnis der Datei (roter Punkt), Erfolgsmeldung für Regeln mit „Auch bei Erfolg benachrichtigen“.
    fn sa_job_done(&mut self, a: Active) {
        let lang = self.settings.language;
        self.save_actions.queue.finished(&a.job.key);
        let failed = a.errors > 0 || !a.job.failed_rules.is_empty();
        self.save_actions.file_failed.insert(a.job.key.clone(), failed);
        // Ein Ergebnis je Regel des Laufs (vor dem Schreiben gescheiterte Text-Aktionen zählen mit).
        let now = sa::now_secs();
        for (rule, st) in rule_results(&a.job, &a.results, a.cancelled) {
            self.save_actions.last.insert(rule, (st, now));
        }
        if !failed && !a.cancelled {
            let fname = a.job.file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            for pr in &a.job.rules {
                if self.save_actions.cfg.rule(pr.rule).is_some_and(|r| r.notify_success) && !pr.post.is_empty() {
                    self.push_toast(format!("✓ {fname} – {}: {}", pr.name, lang.t("Aktionen beim Speichern erfolgreich", "actions on save succeeded")), toasts::ToastKind::Info, None);
                }
            }
        }
    }

    /// Eine Aktion starten (Befehl, Kopieren, Paneldeck-Aktion). Sofort fertige werden gleich abgeschlossen.
    fn sa_start_action(&mut self, ctx: &egui::Context, i: usize, rule: &PlannedRule, action: &sa::Action, file: &Path) {
        let lang = self.settings.language;
        let Some(def) = self.save_actions.cfg.rule(rule.rule).cloned() else {
            let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), String::new());
            self.sa_finish_record(id, RunStatus::Skipped, None, Vec::new(), Some(lang.t("Regel gibt es nicht mehr", "The rule no longer exists").into()));
            return;
        };
        let vctx = self.sa_vars_ctx(&def, file);
        match &action.kind {
            ActionKind::Command { command } => {
                let prepared = self.sa_prepare(&def, command, &vctx);
                match prepared {
                    Err(e) => {
                        let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), command.clone());
                        self.sa_action_failed(i);
                        self.sa_finish_record(id, RunStatus::Failed, None, Vec::new(), Some(e));
                    }
                    Ok(p) => {
                        let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), p.display.clone());
                        if !p.notes.is_empty() {
                            if let Some(r) = self.save_actions.rec_mut(id) {
                                r.message = Some(p.notes.join(" · "));
                            }
                        }
                        match self.sa_spawn(ctx, &def, &p) {
                            Ok(step) => {
                                let step = match step {
                                    Spawned::Hidden(session, batch) => Step::Hidden { session, life: CmdLife::started(Instant::now(), def.timeout()), flavor: p.flavor, record: id, exit_seen: None, batch },
                                    Spawned::Visible(entry, batch) => Step::Visible { entry, life: CmdLife::started(Instant::now(), def.timeout()), flavor: p.flavor, record: id, exit_seen: None, batch },
                                };
                                self.save_actions.active[i].step = Some(step);
                            }
                            Err(e) => {
                                self.sa_action_failed(i);
                                self.sa_finish_record(id, RunStatus::Failed, None, Vec::new(), Some(e));
                            }
                        }
                    }
                }
            }
            ActionKind::Copy { target, keep_structure } => {
                let folder = def.folder();
                match sa::copy::destination(file, &folder, target, *keep_structure) {
                    Err(e) => {
                        let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), target.clone());
                        self.sa_action_failed(i);
                        self.sa_finish_record(id, RunStatus::Failed, None, Vec::new(), Some(e.text(lang).into()));
                    }
                    Ok(dest) => {
                        let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), format!("→ {}", dest.display()));
                        let (tx, rx) = std::sync::mpsc::channel();
                        let (src, d) = (file.to_path_buf(), dest.clone());
                        let c = ctx.clone();
                        let spawned = std::thread::Builder::new().name("save-action-copy".into()).spawn(move || {
                            let _ = tx.send(sa::copy::copy_file(&src, &d));
                            c.request_repaint();
                        });
                        if spawned.is_err() {
                            self.sa_action_failed(i);
                            self.sa_finish_record(id, RunStatus::Failed, None, Vec::new(), Some("thread".into()));
                        } else {
                            self.save_actions.active[i].step = Some(Step::Copy { rx, record: id });
                        }
                    }
                }
            }
            ActionKind::SendToSession { session, name, text } => {
                let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), format!("→ {name}"));
                match self.sa_send_to_session(*session, text, &def, &vctx) {
                    Ok(()) => self.sa_finish_record(id, RunStatus::Ok, None, Vec::new(), None),
                    Err(hint) => self.sa_finish_record(id, RunStatus::Skipped, None, Vec::new(), Some(hint)),
                }
            }
            ActionKind::RestartProcess { process, group, name } => {
                let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), name.clone());
                let ids: Vec<u64> = if group.trim().is_empty() {
                    self.procs.defs.iter().filter(|d| d.id == *process).map(|d| d.id).collect()
                } else {
                    procs::group_members(&self.procs.defs, group)
                };
                if ids.is_empty() {
                    self.sa_action_failed(i);
                    self.sa_finish_record(id, RunStatus::Failed, None, Vec::new(), Some(lang.t("Prozess bzw. Gruppe gibt es nicht mehr", "The process or group no longer exists").into()));
                } else {
                    for p in ids {
                        self.procs.cmds.push(procs_ui::Cmd::Restart(p));
                    }
                    self.sa_finish_record(id, RunStatus::Ok, None, Vec::new(), None);
                }
            }
            ActionKind::Notify { text } => {
                let id = self.sa_new_rec(i, file, rule, action.kind.label(lang), String::new());
                let secret: Vec<String> = def.env_names();
                let lookup = crate::terminal::child_env_lookup(self.settings.inherit_env, &[]);
                let shown = vars::expand_text(text, &vctx, &lookup, &|n| secret.iter().any(|s| s.eq_ignore_ascii_case(n)));
                if !self.seen_focus().0 && self.settings.notify_toast {
                    notify::toast("Paneldeck", &shown, self.settings.notify_sound);
                }
                self.push_toast(shown, toasts::ToastKind::Info, None);
                self.sa_finish_record(id, RunStatus::Ok, None, Vec::new(), None);
            }
            ActionKind::Builtin(_) | ActionKind::Unknown(_) => {}
        }
    }

    /// Variablen für eine Datei und Regel.
    fn sa_vars_ctx(&self, rule: &Rule, file: &Path) -> vars::Ctx {
        vars::Ctx { file: file.to_path_buf(), rule_folder: rule.folder(), workspace: self.editor_root() }
    }

    /// Befehl vorbereiten: Shell, Umgebung, Arbeitsordner, Befehlszeile (auch für *Für Datei testen*).
    pub(super) fn sa_prepare(&self, rule: &Rule, template: &str, vctx: &vars::Ctx) -> Result<Prepared, String> {
        let lang = self.settings.language;
        if template.trim().is_empty() {
            return Err(lang.t("Kein Befehl angegeben", "No command given").into());
        }
        let shell = procs::resolve_shell(&rule.shell, &self.shells).ok_or_else(|| format!("{} {}", lang.t("Shell nicht gefunden:", "Shell not found:"), lang.q(&rule.shell)))?.clone();
        let target = vars::Target::of(&shell);
        let (vals, bad) = rule_env(rule);
        let mut env: Vec<(String, String)> = vctx.env();
        env.extend(vals);
        if target.flavor == procs::Flavor::Wsl {
            let existing = std::env::var("WSLENV").ok();
            env.push(("WSLENV".into(), vctx.wslenv(existing.as_deref(), &rule.env_names())));
        }
        let env_now = env.clone();
        let lookup = crate::terminal::child_env_lookup(self.settings.inherit_env, &env_now);
        let secret_names = rule.env_names();
        let secret = |n: &str| secret_names.iter().any(|s| s.eq_ignore_ascii_case(n));
        let cmd_lookup: &dyn Fn(&str) -> Option<String> = &lookup;
        let line = vars::expand_command(template, vctx, &target, Some(cmd_lookup), &lookup, &secret, false)?;
        let display = vars::expand_command(template, vctx, &target, None, &lookup, &secret, true)?;
        if line.contains('\n') || line.contains('\r') {
            return Err(lang.t("Der Befehl muss eine Zeile sein", "The command must be one line").into());
        }
        // cmd ersetzt `%PANELDECK_FILE%` und geheime `%NAME%` nur aus einer Batch-Datei zur Laufzeit (über die
        // Befehlszeile ersetzt es den übergebenen Text kein zweites Mal; geheime Werte gehören nie hinein).
        let defined = |n: &str| lookup(n).is_some();
        let batch = if target.flavor == procs::Flavor::Cmd && vars::cmd_needs_batch(template, &defined, &secret) {
            Some(vars::cmd_batch_line(template, vctx, &target, &defined, &lookup, &secret)?)
        } else {
            None
        };
        let (mut spec, extra) = procs::launch_spec(&shell, &line);
        if target.flavor == procs::Flavor::Cmd && spec.args.first().is_some_and(|a| a == "/d") {
            // `!` in Werten bleibt wörtlich (verzögerte Erweiterung aus).
            spec.args.insert(1, "/v:off".into());
        }
        env.extend(extra);
        let raw_cwd = if rule.cwd.trim().is_empty() { rule.folder().to_string_lossy().to_string() } else { rule.cwd.trim().to_string() };
        // Ordner nach der Pfadart der Shell (#392, wie die Prozessliste): WSL nimmt Linux-Pfade und
        // `\\wsl.localhost\<D>\…` derselben Distribution über `wsl.exe --cd`, Git Bash `/c/x` und `~` werden
        // Windows-Ordner; ein Ordner einer anderen Distribution oder ein ungültiger Pfad ist ein Fehler der Regel.
        let (cwd, linux_cd) = super::procs_ui::proc_cwd(&shell, &mut spec, &raw_cwd, lang)?;
        let cwd = if linux_cd { raw_cwd } else { cwd };
        let mut notes = Vec::new();
        if !bad.is_empty() {
            notes.push(format!("{} {}", lang.t("Nicht entschlüsselbar (Wert neu eingeben):", "Could not be decrypted (enter the value again):"), bad.join(", ")));
        }
        if target.flavor == procs::Flavor::Cmd {
            let bare = vars::cmd_unquoted_env(template);
            if !bare.is_empty() {
                notes.push(format!("{} {}", lang.t("cmd setzt diese Variablen ungequotet ein – als \"%NAME%\" schreiben:", "cmd inserts these variables unquoted – write them as \"%NAME%\":"), bare.join(", ")));
            }
        }
        Ok(Prepared { flavor: target.flavor, shell, spec, env, cwd, linux_cd, display, notes, batch })
    }

    /// Befehl starten (verborgen oder sichtbar).
    fn sa_spawn(&mut self, ctx: &egui::Context, rule: &Rule, p: &Prepared) -> Result<Spawned, String> {
        let lang = self.settings.language;
        let cwd = p.cwd.trim();
        // WSL über `--cd` (#392): kein Windows-Startordner, keine Prüfung im UI-Thread (das prüft WSL beim Start).
        if !p.linux_cd && !crate::wsl::is_wsl_unc(cwd) {
            match crate::pathfmt::dir_exists_quick(cwd, Duration::from_millis(400)) {
                Some(true) => {}
                Some(false) => return Err(format!("{} {}", lang.t("Ordner nicht gefunden:", "Folder not found:"), lang.q(cwd))),
                None => return Err(format!("{} {}", lang.t("Ordner antwortet nicht (Netzwerk?):", "Folder does not respond (network?):"), lang.q(cwd))),
            }
        }
        // cmd über eine Batch-Datei (`sa::batch`): Die Umgebungsvariable des Befehls nennt dann nur die Datei.
        let mut env = p.env.clone();
        let batch = match &p.batch {
            Some(line) => {
                let b = sa::batch::TempBatch::create(&sa::batch::content(line)).map_err(|e| format!("{} {e}", lang.t("Batch-Datei für cmd ließ sich nicht anlegen:", "Could not create the batch file for cmd:")))?;
                match env.iter_mut().find(|(k, _)| k == procs::ENV_COMMAND) {
                    Some(e) => e.1 = b.command(),
                    None => return Err(lang.t("Batch-Datei für cmd: unerwarteter Start der Shell", "Batch file for cmd: unexpected shell start").into()),
                }
                Some(b)
            }
            None => None,
        };
        let s = Session::spawn(
            ctx,
            SpawnOptions { shell: &p.spec, cwd: (!p.linux_cd).then_some(cwd), restore_text: None, initial_input: None, scrollback_lines: sa::OUTPUT_LINES + 200, env, user_env: Vec::new(), view: None, inherit_env: self.settings.inherit_env },
        )
        .map_err(|e| format!("{} {e}", lang.t("Start fehlgeschlagen:", "Failed to start:")))?;
        s.kill_on_close();
        if !rule.terminal {
            return Ok(Spawned::Hidden(s, batch));
        }
        // Sichtbar: eigene Sitzung in der Seitenleiste (Kind des aktiven Projekts), die genau diesen Befehl ausführt.
        let id = self.next_id;
        self.next_id += 1;
        let (window, project) = self.session_home(Some(0), None);
        let name: String = format!("{} {}", lang.t("Beim Speichern:", "On save:"), rule.display_name()).chars().take(80).collect();
        let meta = SessionMeta { id, name, custom_name: true, shell_id: p.shell.id.clone(), cwd: cwd.to_string(), project, color: hub_color(id), window, ..SessionMeta::default() };
        let mut e = Entry::new(meta);
        e.live = Some(s);
        self.entries.insert(id, e);
        Ok(Spawned::Visible(id, batch))
    }

    /// Text an eine Sitzung senden – nur, wenn sie an ihrem Prompt wartet (Shell-Integration), nie bei offener
    /// Freigabe-Frage, nie in Chats oder laufendes Claude. `Err` = Hinweis (übersprungen).
    fn sa_send_to_session(&mut self, session: u64, template: &str, rule: &Rule, vctx: &vars::Ctx) -> Result<(), String> {
        let lang = self.settings.language;
        let Some(e) = self.entries.get(&session) else { return Err(lang.t("Die Sitzung gibt es nicht mehr", "The session no longer exists").into()) };
        if e.is_chat() {
            return Err(lang.t("Chats bekommen keine Befehle", "Chats do not get commands").into());
        }
        if e.status == ClaudeStatus::Permission {
            return Err(lang.t("In der Sitzung wartet eine Freigabe-Frage – nichts gesendet", "An approval question is open in the session – nothing sent").into());
        }
        if e.is_claude() {
            return Err(lang.t("In der Sitzung läuft Claude – nichts gesendet", "Claude is running in the session – nothing sent").into());
        }
        let Some(s) = e.live.as_ref().filter(|s| !s.has_exited()) else { return Err(lang.t("Die Sitzung läuft nicht", "The session is not running").into()) };
        let idle = {
            let alt = s.alt_screen();
            let st = s.shell_state.lock().unwrap_or_else(|x| x.into_inner());
            st.integration_seen && !alt && st.enters_since_prompt == 0 && !st.nested(alt) && !st.mark_open
        };
        if !idle {
            return Err(lang.t("Die Sitzung wartet nicht an ihrem Prompt (oder ohne Shell-Integration) – nichts gesendet", "The session is not waiting at its prompt (or has no shell integration) – nothing sent").into());
        }
        let shell = self.shell_by_id(&e.eff_shell().to_string());
        let target = shell.as_ref().map(vars::Target::of).unwrap_or_else(vars::Target::pwsh);
        let (vals, _) = rule_env(rule);
        let lookup = crate::terminal::child_env_lookup(self.settings.inherit_env, &vals);
        let names = rule.env_names();
        let secret = |n: &str| names.iter().any(|s| s.eq_ignore_ascii_case(n));
        // Geheime Werte landeten sichtbar im Terminal und im Verlauf.
        if let Some(n) = vars::secret_used(template, &secret) {
            return Err(format!("{} {}", lang.t("Geheime Variablen gehen nicht an Sitzungen (sie stünden sichtbar im Terminal):", "Secret variables are not sent to sessions (they would be visible in the terminal):"), n));
        }
        let line = vars::expand_command(template, vctx, &target, None, &lookup, &secret, false)?;
        if line.trim().is_empty() || line.contains('\n') {
            return Err(lang.t("Der Text muss genau eine Zeile sein", "The text must be exactly one line").into());
        }
        s.send_text_agent(&line, true);
        Ok(())
    }

    // ---------------------------------------------------------------------------------------------------------
    // Der Haken beim Speichern

    /// Dokument speichern – **alle** Speicherwege des Editors laufen hier durch (Speichern, Speichern unter, Alle
    /// speichern, Rückfrage beim Schließen, automatisches Speichern, Agenten). Reihenfolge:
    /// 1. Plan der passenden Regeln (Herkunft, Pause, *Ohne Aktionen speichern*),
    /// 2. vor dem Schreiben, zusammen **ein** Rückgängig-Schritt ([`editor_ui::EditorUi::save_doc`]): die eingebauten
    ///    Aktionen der passenden Regeln (#383, auch *Formatieren*). Ohne passende Regel (oder mit *Ohne Aktionen
    ///    speichern*) wird genau der Text des Editors geschrieben – formatiert wird nie von allein (#381),
    /// 3. schreiben – scheitert das, wird Schritt 2 zurückgenommen und nichts weiter getan,
    /// 4. danach: Verlauf, Bericht für `editor_save` und die übrigen Aktionen in die Warteschlange.
    ///
    /// Die Datei wird immer gespeichert, auch wenn eine Aktion scheitert. Hot Exit speichert nie (geht hier nicht durch).
    pub(crate) fn editor_save_with(&mut self, id: u64, path: Option<PathBuf>, origin: SaveOrigin) -> Result<(), String> {
        let lang = self.settings.language;
        let prefs = self.editor_prefs();
        let without = std::mem::take(&mut self.save_actions.without_next);
        let target = path.clone().or_else(|| self.ed.docs.get(&id).and_then(|d| d.path.clone()));
        let plan = match &target {
            Some(t) => matching::plan(&self.save_actions.cfg, t, origin, Switches { paused: self.save_actions.paused, without_actions: without }),
            None => Plan::default(),
        };
        // Automatisches Speichern: Leerraum in Zeilen mit einem Cursor bleibt (man tippt dort gerade).
        let keep_lines: Vec<usize> = if origin == SaveOrigin::Autosave {
            match self.ed.docs.get(&id) {
                Some(d) => self.ed.panes.tabs_of(id).into_iter().filter_map(|(g, i)| self.ed.panes.groups.get(&g).and_then(|grp| grp.tabs.get(i)).map(|t| t.view.cursor_linecol(d).0)).collect(),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        if !self.ed.docs.contains_key(&id) {
            return Err(lang.t("Kein solches Dokument", "No such document").to_string());
        }
        // Agenten speichern nur lokale Windows-Dateien (#392; `editor_save` prüft das schon, hier die zweite Linie).
        if origin == SaveOrigin::Agent && target.as_deref().is_some_and(|p| crate::wsl::is_wsl_unc(&p.to_string_lossy())) {
            return Err(lang.t("Agenten speichern nur lokale Dateien, keine Dateien in WSL.", "Agents only save local files, not files in WSL.").to_string());
        }
        // Vor dem Schreiben: Text-Aktionen der Regeln (ein rückgängig machbarer Schritt), dann schreiben.
        let list: Vec<Builtin> = plan.builtins.iter().map(|(b, ..)| b.clone()).collect();
        let saved = self.ed.save_doc(id, path, &list, &prefs, lang, &keep_lines)?;
        let after = AfterWrite { plan, builtins: saved.builtins, target, origin };
        if saved.pending {
            // WSL (#392): Das Schreiben läuft im Hilfsthread. Verlauf, Bericht, Aktionen nach dem Schreiben und neue
            // Marken folgen erst mit dem Ergebnis (`sa_disk_job_done`); scheitert es, wird zurückgenommen und nichts
            // weiter getan – wie beim Speichern im UI-Thread.
            self.save_actions.pending_writes.insert(id, after);
            return Ok(());
        }
        self.sa_after_write(after);
        // Smart-Modus (#367): `didSave` (z. B. für die Prüfung beim Speichern von rust-analyzer).
        self.ed.smart_saved(id);
        Ok(())
    }

    /// Ergebnis eines Speicherns im Hintergrund (WSL, #392) für Dokument `id`: geschrieben → weiter wie nach dem
    /// Schreiben ([`App::sa_after_write`]); gescheitert → die Text-Aktionen der Regeln zurücknehmen (solange
    /// unverändert weitergetippt), keine Aktionen nach dem Schreiben. Liefert bei einem Fehler einen Zusatz für die
    /// Meldung.
    pub(super) fn sa_disk_job_done(&mut self, id: u64, saved: bool) -> Option<String> {
        let lang = self.settings.language;
        if saved {
            self.ed.pending_saves.remove(&id);
            match self.save_actions.pending_writes.remove(&id) {
                Some(w) => self.sa_after_write(w),
                // Ohne Regeln-Stand (z. B. alter Lauf): wenigstens Vergleiche und Marken neu lesen.
                None => self.ed.generation += 1,
            }
            return None;
        }
        let had = self.save_actions.pending_writes.remove(&id).is_some();
        let rolled = self.ed.rollback_pending_save(id);
        match (had, rolled) {
            (_, false) => Some(lang.t("Formatieren und Text-Aktionen bleiben im Dokument (es wurde weiter bearbeitet); Aktionen nach dem Schreiben liefen nicht.", "Formatting and text actions stay in the document (it was edited further); actions after writing did not run.").to_string()),
            (true, true) => Some(lang.t("Aktionen beim Speichern zurückgenommen bzw. nicht ausgeführt.", "Actions on save were undone or not run.").to_string()),
            (false, true) => None,
        }
    }

    /// Nach dem Schreiben (alle Speicherwege, auch das Ende eines Speicherns im Hintergrund): Vergleiche und Marken neu
    /// lesen, Verlauf, Bericht für `editor_save` und die übrigen Aktionen in die Warteschlange.
    fn sa_after_write(&mut self, w: AfterWrite) {
        let lang = self.settings.language;
        let AfterWrite { plan, builtins, target, origin } = w;
        // Vergleiche und Änderungsmarken lesen den gespeicherten Stand neu (#385).
        self.ed.generation += 1;
        let mut failed_rules: Vec<(u64, String)> = Vec::new();
        let mut pre_notes: Vec<(u64, Builtin, String, bool)> = Vec::new();
        for (b, res) in builtins {
            let Some((_, rule, _)) = plan.builtins.iter().find(|(x, ..)| *x == b) else { continue };
            match res {
                Ok(Some(note)) => pre_notes.push((*rule, b.clone(), note, false)),
                Ok(None) => {}
                Err(e) => {
                    // Gilt für jede Regel, die diese Aktion ohne „Bei Fehler weiter“ hat.
                    for pr in &plan.rules {
                        if pr.builtins.iter().any(|(x, cont)| *x == b && !cont) {
                            failed_rules.push((pr.rule, e.clone()));
                        }
                    }
                    pre_notes.push((*rule, b.clone(), e, true));
                }
            }
        }
        // Nach dem Schreiben.
        let file = target.unwrap_or_default();
        for (rule, b, note, err) in pre_notes {
            let pr = plan.rules.iter().find(|r| r.rule == rule).cloned().unwrap_or(PlannedRule { rule, name: String::new(), post: Vec::new(), builtins: Vec::new(), pos: 0 });
            let rid = self.save_actions.new_record(&file, &pr, b.label(lang), String::new());
            self.sa_finish_record(rid, if err { RunStatus::Failed } else { RunStatus::Skipped }, None, Vec::new(), Some(note));
        }
        // Bericht für `editor_save` nur, wenn es überhaupt Regeln gibt.
        self.save_actions.last_report = (!self.save_actions.cfg.rules.is_empty()).then(|| report_json(&plan, &failed_rules, &self.save_actions.cfg));
        let key = sa::path_key(&file);
        let now = sa::now_secs();
        // Regeln nur mit Text-Aktionen: Ergebnis jetzt; Regeln mit Aktionen danach: „läuft“ bis zum Ende des Laufs.
        for r in &plan.rules {
            let st = if !r.post.is_empty() {
                RunStatus::Running
            } else if failed_rules.iter().any(|(x, _)| *x == r.rule) {
                RunStatus::Failed
            } else {
                RunStatus::Ok
            };
            self.save_actions.last.insert(r.rule, (st, now));
        }
        if plan.rules.iter().any(|r| !r.post.is_empty()) {
            self.save_actions.queue.submit(Job { id: 0, file, key, rules: plan.rules.clone(), origin, failed_rules });
        } else if !plan.builtins.is_empty() {
            self.save_actions.file_failed.insert(key, !failed_rules.is_empty());
        }
    }

    /// *Jetzt ausführen* bzw. *Für Datei testen → Ausführen*: die Aktionen nach dem Schreiben der passenden Regeln (oder
    /// nur einer) für eine Datei, ohne zu speichern.
    pub(super) fn sa_run_now(&mut self, file: &Path, only: Option<u64>) -> usize {
        let mut plan = matching::plan(&self.save_actions.cfg, file, SaveOrigin::User, Switches::default());
        if let Some(r) = only {
            plan.rules.retain(|x| x.rule == r);
            if plan.rules.is_empty()
                && let Some(def) = self.save_actions.cfg.rule(r)
            {
                // Auch eine Regel, die gerade nicht passt (z. B. im Dialog noch nicht gespeichert): so, wie sie ist.
                let pos = self.save_actions.cfg.rules.iter().position(|x| x.id == r).unwrap_or(0);
                plan.rules.push(PlannedRule { rule: r, name: def.display_name(), post: def.actions.iter().filter(|a| !a.kind.modifies() && !matches!(a.kind, ActionKind::Unknown(_))).cloned().collect(), builtins: Vec::new(), pos });
            }
        }
        let n = plan.rules.iter().map(|r| r.post.len()).sum();
        if n > 0 {
            for r in plan.rules.iter().filter(|r| !r.post.is_empty()) {
                self.save_actions.last.insert(r.rule, (RunStatus::Running, sa::now_secs()));
            }
            self.save_actions.queue.submit(Job { id: 0, file: file.to_path_buf(), key: sa::path_key(file), rules: plan.rules, origin: SaveOrigin::User, failed_rules: Vec::new() });
        }
        n
    }

    /// Alles abbrechen (wartende verwerfen, laufende Befehle anhalten).
    pub(super) fn sa_cancel_all(&mut self) {
        self.save_actions.queue.clear_waiting();
        let now = Instant::now();
        for a in &mut self.save_actions.active {
            a.cancelled = true;
            a.cancel_at.get_or_insert(now);
        }
    }

    /// Den Lauf abbrechen, zu dem ein Verlaufseintrag gehört.
    pub(super) fn sa_cancel_record(&mut self, record: u64) {
        for a in &mut self.save_actions.active {
            let mine = match &a.step {
                Some(Step::Hidden { record: r, .. }) | Some(Step::Visible { record: r, .. }) | Some(Step::Copy { record: r, .. }) => *r == record,
                None => false,
            };
            if mine {
                a.cancelled = true;
                a.cancel_at.get_or_insert(Instant::now());
            }
        }
    }

    /// Stand für die Statusleiste und Reiter des Editors (jedes Bild vor dem Zeichnen).
    pub(super) fn sa_publish_view(&mut self) {
        let v = &mut self.ed.on_save;
        v.running = self.save_actions.busy();
        v.paused = self.save_actions.paused;
        v.failed = self.save_actions.file_failed.iter().filter(|(_, f)| **f).map(|(k, _)| k.clone()).collect();
        v.last = self.save_actions.file_failed.clone();
    }

    /// Klicks aus Statusleiste, Baum und Menüs des Editors.
    pub(super) fn sa_take_clicks(&mut self) {
        let Some(c) = self.ed.on_save.clicked.take() else { return };
        match c {
            OnSaveClick::Output => self.save_actions.ui.output_open = true,
            OnSaveClick::Resume => self.save_actions.paused = false,
            OnSaveClick::RulesFor(p) => self.sa_open_test(Some(p)),
            OnSaveClick::NewRule(p, dir) => self.sa_new_rule_for(&p, dir),
        }
    }

    /// Toast-Knopf *Ausgabe zeigen*.
    pub(super) fn sa_toast_clicked(&mut self, token: u64) {
        self.save_actions.ui.output_open = true;
        self.save_actions.ui.output_sel = Some(token & !SA_TOKEN);
    }

    // ---------------------------------------------------------------------------------------------------------
    // Beenden

    /// Beim Beenden: laufen Befehle, anhalten und fragen (*Alle stoppen* / *Abbrechen*). `true` = angehalten.
    pub(super) fn sa_quit_guard(&mut self, ctx: &egui::Context) -> bool {
        if !self.save_actions.any_active() {
            return false;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.in_tray {
            self.from_tray(ctx);
        }
        if self.save_actions.quit == procs_ui::QuitState::None {
            self.save_actions.quit = procs_ui::QuitState::Ask;
        }
        true
    }

    /// Namen der laufenden Befehle (Rückfrage beim Beenden).
    pub(super) fn sa_active_names(&self) -> Vec<String> {
        let lang = self.settings.language;
        let label = |j: &Job, ix: usize| {
            let f = j.file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let r = j.rules.get(ix).or_else(|| j.rules.last()).map(|r| r.name.clone()).unwrap_or_default();
            format!("{r} – {f}")
        };
        let mut out: Vec<String> = self.save_actions.active.iter().map(|a| label(&a.job, a.rule_ix)).collect();
        out.extend(self.save_actions.queue.waiting_jobs().map(|j| format!("{} ({})", label(j, 0), lang.t("wartet", "waiting"))));
        out
    }

    pub(super) fn sa_quit_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        match self.save_actions.quit {
            procs_ui::QuitState::None => {}
            procs_ui::QuitState::Ask => {
                let names = self.sa_active_names();
                if names.is_empty() {
                    self.save_actions.quit = procs_ui::QuitState::None;
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                    return;
                }
                let o = closing::confirm_window_keep(ctx, Id::new("sa_quit_ask"), lang.t("Paneldeck beenden?", "Quit Paneldeck?"), lang.t("Alle stoppen", "Stop all"), None, 0, lang, 440.0, |ui| {
                    ui.label(lang.t(
                        "Diese Aktionen beim Speichern laufen noch. Sie enden mit Paneldeck (Strg+C, nach 5 s der ganze Prozessbaum):",
                        "These actions on save are still running. They end with Paneldeck (Ctrl+C, after 5 s the whole process tree):",
                    ));
                    ui.add_space(S1);
                    for n in &names {
                        ui.label(format!("• {n}"));
                    }
                });
                match o {
                    closing::Outcome::Go | closing::Outcome::Keep => {
                        self.quitting = true;
                        self.sa_cancel_all();
                        self.save_actions.quit = procs_ui::QuitState::Stopping;
                    }
                    closing::Outcome::Cancel => {
                        self.save_actions.quit = procs_ui::QuitState::None;
                        self.quitting = false;
                    }
                    closing::Outcome::Open => {}
                }
            }
            procs_ui::QuitState::Stopping => {
                let n = self.sa_active_names().len();
                style::movable(ctx, Id::new("sa_quit_wait"), egui::Window::new(lang.t("Aktionen werden gestoppt …", "Stopping actions …"))).collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("{} {n}", lang.t("Noch laufend:", "Still running:")));
                    });
                });
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // MCP

    /// `list_save_actions` (Stufe Lesen): Name, Muster, Arten der Aktionen, an/aus, letztes Ergebnis – nie Befehle,
    /// Ziele, Texte oder Werte von Umgebungsvariablen.
    pub(super) fn mcp_save_actions(&self) -> Value {
        sa_list_json(&self.save_actions)
    }
}

/// Ergebnis von [`App::sa_spawn`].
enum Spawned {
    Hidden(Session, Option<sa::batch::TempBatch>),
    Visible(u64, Option<sa::batch::TempBatch>),
}

/// Ein Ergebnis je Regel eines fertigen Laufs (nur Regeln mit Aktionen nach dem Schreiben): das schlechteste ihrer
/// Aktionen; vor dem Schreiben gescheiterte Text-Aktionen zählen als Fehler; ohne Aktion im Lauf (abgebrochen) =
/// abgebrochen.
fn rule_results(job: &Job, results: &HashMap<u64, RunStatus>, cancelled: bool) -> Vec<(u64, RunStatus)> {
    job.rules
        .iter()
        .filter(|r| !r.post.is_empty())
        .map(|pr| {
            let mut st = results.get(&pr.rule).copied().unwrap_or(if cancelled { RunStatus::Cancelled } else { RunStatus::Ok });
            if job.failed_rules.iter().any(|(x, _)| *x == pr.rule) {
                st = RunStatus::Failed;
            }
            (pr.rule, st)
        })
        .collect()
}

/// Strg+C bzw. Baum beenden.
fn sa_effects(s: &Session, fx: Vec<procs::Effect>) {
    for e in fx {
        match e {
            procs::Effect::CtrlC => {
                let k = crate::vtkeys::KeyPress::new(Key::C, Modifiers::CTRL);
                let out = crate::vtkeys::encode_key(&k, s.key_mode());
                s.write_raw(if out.bytes.is_empty() { b"\x03" } else { &out.bytes });
            }
            procs::Effect::KillTree => {
                s.kill_tree();
            }
            _ => {}
        }
    }
}

/// Bericht für `editor_save`: welche Regeln liefen bzw. übersprungen wurden (ohne Befehle).
pub(super) fn report_json(plan: &Plan, failed: &[(u64, String)], cfg: &Config) -> Value {
    let name = |id: u64| cfg.rule(id).map(|r| r.display_name()).unwrap_or_default();
    json!({
        "triggered": plan.rules.iter().map(|r| json!({"id": r.rule, "name": r.name, "actions": r.builtins.iter().map(|(b, _)| b.kind().to_string()).chain(r.post.iter().map(|a| a.kind.kind())).collect::<Vec<_>>(), "failed_before_write": failed.iter().any(|(x, _)| *x == r.rule)})).collect::<Vec<_>>(),
        "skipped": plan.skipped.iter().map(|(id, n, why)| json!({"id": id, "name": n, "reason": why.code(), "detail": why.text(Lang::En, name)})).collect::<Vec<_>>(),
        "builtins": plan.builtins.iter().map(|(b, ..)| b.kind()).collect::<Vec<_>>(),
        "note": "Commands run in the background after saving; their output is untrusted data and is not returned here.",
    })
}

/// `list_save_actions`.
pub(super) fn sa_list_json(st: &SaState) -> Value {
    let rules: Vec<Value> = st
        .cfg
        .rules
        .iter()
        .map(|r| {
            let last = st.last.get(&r.id).map(|(s, at)| json!({"status": s.as_str(), "at": at}));
            let missing = !r.path.trim().is_empty() && !crate::pathfmt::is_remote(r.path.trim()) && !Path::new(r.path.trim()).exists();
            json!({
                "id": r.id,
                "name": r.display_name(),
                "enabled": r.enabled,
                "scope": r.scope.as_str(),
                "path": r.path,
                "patterns": r.patterns,
                "actions": r.actions.iter().map(|a| a.kind.kind()).collect::<Vec<_>>(),
                "on_autosave": r.on_autosave,
                "on_agent_save": r.on_agent_save,
                "stop_after": r.stop_after,
                "no_parent_rules": r.no_parent_rules,
                "terminal": r.terminal,
                "timeout_secs": r.timeout_secs,
                "folder_missing": missing,
                "last_result": last,
            })
        })
        .collect();
    json!({
        "enabled": st.cfg.enabled,
        "paused": st.paused,
        "parallel": st.cfg.parallel(),
        "running": st.busy(),
        "rules": rules,
        "note": "Rules are created only by the user (Settings -> Editor -> Actions on save); agents cannot create or change them. Command lines, targets, texts and environment values are never returned.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::EnvVar;
    use crate::save_actions::{Action, Scope};

    #[test]
    fn mcp_list_never_contains_commands_targets_or_env_values() {
        let mut st = SaState::default();
        let mut r = Rule::new(1, Scope::Pattern, if cfg!(windows) { r"C:\w" } else { "/w" });
        r.name = "Upload".into();
        r.patterns = vec!["*.php".into()];
        r.env = vec![EnvVar { name: "DEPLOY_TOKEN".into(), value_protected: Some("BLOBVALUE".into()) }];
        r.actions = vec![
            Action::new(ActionKind::Command { command: "scp ${file} secret-host:/var/www".into() }),
            Action::new(ActionKind::Copy { target: "D:\\mirror-target".into(), keep_structure: true }),
            Action::new(ActionKind::SendToSession { session: 3, name: "Build".into(), text: "make deploy-now".into() }),
            Action::new(ActionKind::Notify { text: "hello-notify".into() }),
        ];
        st.cfg.rules.push(r);
        st.last.insert(1, (RunStatus::Failed, 5));
        let v = sa_list_json(&st);
        let text = v.to_string();
        for bad in ["scp", "secret-host", "mirror-target", "deploy-now", "hello-notify", "BLOBVALUE", "DEPLOY_TOKEN"] {
            assert!(!text.contains(bad), "{bad} darf nicht in list_save_actions stehen");
        }
        assert_eq!(v["rules"][0]["actions"], json!(["command", "copy", "send_to_session", "notify"]));
        assert_eq!(v["rules"][0]["last_result"]["status"], "failed");
        assert_eq!(v["rules"][0]["patterns"], json!(["*.php"]));
    }

    #[test]
    fn one_result_per_rule_and_running_records_are_never_evicted() {
        let pr = |rule: u64, post: bool| PlannedRule { rule, name: String::new(), post: if post { vec![sa::Action::new(ActionKind::Notify { text: "x".into() })] } else { vec![] }, builtins: vec![], pos: 0 };
        let job = Job { id: 1, file: PathBuf::from("f"), key: "f".into(), rules: vec![pr(1, true), pr(2, true), pr(3, false), pr(4, true)], origin: SaveOrigin::User, failed_rules: vec![(4, "trim".into())] };
        let results: HashMap<u64, RunStatus> = [(1, RunStatus::Failed), (2, RunStatus::Ok)].into_iter().collect();
        assert_eq!(rule_results(&job, &results, false), vec![(1, RunStatus::Failed), (2, RunStatus::Ok), (4, RunStatus::Failed)]);
        assert_eq!(rule_results(&job, &HashMap::new(), true)[0], (1, RunStatus::Cancelled));
        // Verlauf: der laufende Eintrag bleibt, auch wenn mehr als 100 dazukommen.
        let mut st = SaState::default();
        let p = pr(1, true);
        let first = st.new_record(Path::new("f"), &p, "a".into(), String::new());
        for _ in 0..(sa::HISTORY_RUNS + 20) {
            let id = st.new_record(Path::new("f"), &p, "a".into(), String::new());
            st.rec_mut(id).unwrap().status = RunStatus::Ok;
        }
        assert_eq!(st.history.len(), sa::HISTORY_RUNS);
        assert!(st.history.iter().any(|r| r.id == first && r.status == RunStatus::Running));
    }

    #[test]
    fn editor_save_report_lists_triggered_and_skipped() {
        let base = if cfg!(windows) { r"C:\w" } else { "/w" };
        let mut a = Rule::new(1, Scope::Folder, base);
        a.actions = vec![Action::new(ActionKind::Command { command: "secret-cmd".into() })];
        let mut b = Rule::new(2, Scope::Folder, base);
        b.actions = vec![Action::new(ActionKind::Builtin(Builtin::TrimWhitespace))];
        b.on_agent_save = true;
        let cfg = Config { rules: vec![a, b], ..Default::default() };
        let plan = matching::plan(&cfg, &Path::new(base).join("x.txt"), SaveOrigin::Agent, Switches::default());
        let v = report_json(&plan, &[], &cfg);
        assert_eq!(v["triggered"][0]["id"], 2);
        assert_eq!(v["skipped"][0]["id"], 1);
        assert_eq!(v["skipped"][0]["reason"], "agent_not_allowed");
        assert_eq!(v["builtins"], json!(["trim_whitespace"]));
        assert!(!v.to_string().contains("secret-cmd"));
    }
}
