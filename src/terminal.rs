//! Ein Terminal: PTY (ConPTY unter Windows) + VT-Emulation (alacritty_terminal).

use crate::shells::ShellSpec;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{test::TermSize, Config, Osc52, Term};
use alacritty_terminal::vte::ansi::Processor;
use eframe::egui;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use crate::osc::{OscEvent, OscScanner};
use crate::search::SearchState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Höchstgröße eines Textes, den ein Programm per OSC 52 in die Zwischenablage legen darf (#181).
pub const OSC52_MAX_BYTES: usize = 1024 * 1024;

/// Kopierwünsche eines Programms per OSC 52 seit dem letzten Abholen durch den UI-Thread (`Session::take_clipboard`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardRequest {
    /// Neuester gültiger Text (nicht leer, höchstens [`OSC52_MAX_BYTES`]). Ein späterer zu großer Text verdrängt ihn nicht.
    pub text: Option<String>,
    /// Größe (Bytes) des zuletzt verworfenen, zu großen Textes – für eine Warnung.
    pub too_large: Option<usize>,
    /// Anzahl der nicht leeren Kopierwünsche (für die Warnung bei Dauerfeuer).
    pub count: u32,
}

impl ClipboardRequest {
    /// Einen Kopierwunsch aufnehmen. Leere Texte werden ignoriert (sie würden die Zwischenablage nur leeren).
    pub fn add(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.count = self.count.saturating_add(1);
        if text.len() > OSC52_MAX_BYTES {
            self.too_large = Some(text.len());
        } else {
            self.text = Some(text);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.too_large.is_none()
    }
}

#[derive(Clone)]
pub struct Listener {
    title: Arc<Mutex<Option<String>>>,
    writer: SharedWriter,
    bell: Arc<AtomicBool>,
    /// Kopierwünsche (OSC 52) seit dem letzten Abholen.
    clipboard: Arc<Mutex<ClipboardRequest>>,
}

impl Listener {
    /// Listener mit Standardwerten; `Session::spawn` übernimmt die geteilten Felder per `clone`.
    pub fn new(writer: SharedWriter) -> Self {
        Listener { title: Default::default(), writer, bell: Default::default(), clipboard: Default::default() }
    }
}

/// Emulator-Einstellungen eines Terminals. OSC 52 nur zum Kopieren: Programme dürfen die Zwischenablage
/// beschreiben, aber nie lesen (`ClipboardLoad` wird gar nicht erst ausgelöst).
pub fn term_config(scrollback_lines: usize) -> Config {
    Config { scrolling_history: scrollback_lines, osc52: Osc52::OnlyCopy, ..Config::default() }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(t) => *self.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(t),
            Event::ResetTitle => *self.title.lock().unwrap_or_else(|e| e.into_inner()) = None,
            // Antworten der Emulation (z.B. Cursorposition) zurück an die Shell.
            Event::PtyWrite(s) => {
                let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
                let _ = w.write_all(s.as_bytes());
                let _ = w.flush();
            }
            Event::Bell => self.bell.store(true, Ordering::Relaxed),
            // OSC 52 „Kopieren“ (Zwischenablage oder Auswahl – Windows kennt nur eine): an den UI-Thread übergeben,
            // der die Einstellung prüft und die Zwischenablage setzt. Die Verarbeitung läuft im PTY-Thread, der danach
            // ohnehin `request_repaint` aufruft.
            Event::ClipboardStore(_, text) => self.clipboard.lock().unwrap_or_else(|e| e.into_inner()).add(text),
            // Lesen der Zwischenablage durch Programme wird nie beantwortet (auch nicht für Agenten-Eingaben).
            Event::ClipboardLoad(..) => {}
            _ => {}
        }
    }
}

/// Von der Shell-Integration gemeldete Zustände.
#[derive(Default)]
pub struct ShellState {
    pub cwd: Option<String>,
    pub last_exit: Option<i32>,
    pub last_dur_ms: Option<u64>,
    /// Absolute Zeilen (ab Anfang des Verlaufs) der Prompt-Anfänge.
    pub marks: Vec<u64>,
    /// Beendete Befehle, die noch nicht ausgewertet wurden (Benachrichtigungen).
    pub done: Vec<CmdDone>,
    /// Monotone Zähler (nie zurückgesetzt, anders als `done`): Prompts, beendete Befehle, Ordnermeldungen.
    /// Die Agenten-Schnittstelle wartet damit auf „Prompt ist zurück“.
    pub prompt_seq: u64,
    pub done_seq: u64,
    pub cwd_seq: u64,
    /// Es kamen schon Meldungen der Shell-Integration (sonst gibt es keine Befehlsgrenzen).
    pub integration_seen: bool,
    pending_dur: Option<u64>,
    cmd_start: Option<std::time::Instant>,
}

#[derive(Clone, Debug)]
pub struct CmdDone {
    pub exit: Option<i32>,
    pub dur_ms: Option<u64>,
}

pub struct Session {
    pub shell_state: Arc<Mutex<ShellState>>,
    pub term: Arc<Mutex<Term<Listener>>>,
    /// Aus dem Verlauf gefallene Zeilen (#197), nur unter der Sperre von `term` geändert: absolute Zeile =
    /// `dropped` + Verlaufszeilen + Bildschirmzeile, damit bleiben Marken und Positionen bei vollem Verlauf gültig.
    dropped: Arc<std::sync::atomic::AtomicU64>,
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    title: Arc<Mutex<Option<String>>>,
    exited: Arc<AtomicBool>,
    #[allow(dead_code)] // wird gesetzt (Event::Bell); eine Anzeige dafür ist noch nicht eingebaut
    pub bell: Arc<AtomicBool>,
    clipboard: Arc<Mutex<ClipboardRequest>>,
    pub cols: u16,
    pub rows: u16,
    /// Rest-Scrollwert (Bruchteil einer Zeile) für sanftes Scrollen.
    pub scroll_acc: f32,
    pub search: Option<SearchState>,
    pub shell_id: String,
    pub mouse_down: Option<u8>,
    pub mouse_last: Option<(usize, usize)>,
    /// Link unter dem Mauszeiger bei gedrückter Strg-Taste: (Zeile, Start, Ende, Ziel)
    pub hover_link: Option<(usize, usize, usize, String)>,
    tee_on: AtomicBool,
    tee: Mutex<Vec<u8>>,
    /// Zeitpunkt der letzten Ausgabe des Programms (ms seit Programmstart) – für „Terminal ist ruhig“.
    last_activity: Arc<std::sync::atomic::AtomicU64>,
    /// Die Shell meldete ihren Prompt (OSC 133;A), während noch der alternative Bildschirm oder ein Maus-Modus
    /// aktiv war – ein Vollbildprogramm ist wohl ohne Aufräumen beendet worden (Absturz, getrennte SSH-Verbindung).
    /// Mausrad und Klicks behandeln die Sitzung dann wie eine normale Shell (keine Pfeiltasten, keine Mausereignisse);
    /// die Modi selbst bleiben unangetastet. Zurückgesetzt, sobald ein Befehl abgeschickt wird (`\r` in `write_raw`)
    /// oder ein Programm Vollbild/Maus-Modus erneut einschaltet (`next_stale_modes`).
    ///
    /// Heuristik mit bekannten Fehlgriffen: Ein Multiplexer (tmux, screen, zellij), der das OSC 133;A der inneren
    /// Shell durchreicht, während er selbst im alternativen Bildschirm mit Maus-Modus läuft, sieht wie ein hängen
    /// gebliebener Zustand aus – dann bekommt er bis zur nächsten Befehlszeile bzw. zum nächsten DECSET kein Mausrad
    /// und keine Klicks. Ebenso ein Vollbildprogramm, das selbst OSC 133;A ausgibt. Ohne Shell-Integration
    /// (cmd ohne Integration, entfernte Shells) greift die Erkennung nicht.
    pub stale_modes: Arc<AtomicBool>,
    /// Job Object der Shell und all ihrer Nachfahren (#168): ordnet MCP-Aufrufer auch losgelöst dieser Sitzung zu.
    /// Wird nach dem Beenden der Shell (`Drop`) freigegeben; die Prozesse darin laufen weiter.
    job: Option<crate::winctl::SessionJob>,
    /// PID der Shell (beim Start festgehalten; `pid()` kann am gesperrten Kind scheitern).
    shell_pid: Option<u32>,
    /// Konsolen-Host dieser Sitzung (#183): `"bundled"` (mitgelieferte ConPTY) oder `"system"`; `None` außerhalb von Windows.
    pub console_host: Option<&'static str>,
}

/// Millisekunden seit dem ersten Aufruf (monotone, billige Uhr für `last_activity`).
fn mono_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

pub struct SpawnOptions<'a> {
    pub shell: &'a ShellSpec,
    pub cwd: Option<&'a str>,
    /// Text, der vor dem Start der Shell in den Verlauf geschrieben wird.
    pub restore_text: Option<&'a str>,
    /// Wird direkt nach dem Start an die Shell gesendet (z.B. "claude\r").
    pub initial_input: Option<&'a str>,
    pub scrollback_lines: usize,
    /// Zusätzliche Variablen (Paneldecks eigene, Profil, Shell-Integration); gelten immer zuletzt.
    pub env: Vec<(String, String)>,
    /// Umgebung des Paneldeck-Prozesses unverändert erben statt sie mit der Registry aufzufrischen (`Settings::inherit_env`).
    pub inherit_env: bool,
}

/// Interne Variablen, die Paneldeck selbst setzt bzw. nur für einen Neustart nutzt: Ein Wert aus der Umgebung des
/// Paneldeck-Prozesses (z. B. Start aus einem Terminal eines anderen Hubs) darf nie an ein Terminal gehen – sonst fände
/// `paneldeck mcp` dort den falschen Hub. Die passenden Werte setzt `SpawnOptions::env` danach neu.
pub const INTERNAL_ENV: [&str; 11] = [
    crate::hooks::ENV_SESSION,
    crate::hooks::ENV_SESSION_KEY,
    crate::hooks::ENV_PORT,
    crate::hooks::ENV_TOKEN,
    crate::paths::ENV_HUB_DATA,
    crate::paths::ENV_HUB_BASE,
    "PANELDECK_UPDATE_WAIT_PID",
    "PANELDECK_UPDATE_KEEP_SESSIONS",
    // nur für Tests/Bildschirmfotos: nie an Terminals weitergeben
    "PANELDECK_UPDATE_TEST_FEED",
    "PANELDECK_SHOT",
    "PANELDECK_DEMO_",
];

/// Ist `name` eine interne Variable ([`INTERNAL_ENV`]; Einträge mit `_` am Ende sind Präfixe, z. B. `PANELDECK_DEMO_*`)?
/// Groß-/Kleinschreibung egal (Windows).
pub fn is_internal_env(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    INTERNAL_ENV.iter().any(|k| if k.ends_with('_') { n.starts_with(k) } else { n == *k })
}

/// Umgebung des Terminals setzen (#175). `cmd` bringt von portable-pty schon die Standard-Umgebung mit (Prozessumgebung,
/// darüber System- und Benutzervariablen frisch aus der Registry). `inherit`: stattdessen genau die Prozessumgebung
/// `process`. In beiden Fällen fallen die internen Variablen ([`INTERNAL_ENV`]) weg, dann gelten `TERM`/`COLORTERM` und
/// `extra` (Paneldecks eigene Variablen, Profil, Shell-Integration) – sie gewinnen immer.
fn apply_env(cmd: &mut CommandBuilder, inherit: bool, process: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>, extra: &[(String, String)]) {
    if inherit {
        cmd.env_clear();
        for (k, v) in process {
            cmd.env(k, v);
        }
    }
    let internal: Vec<String> = cmd.iter_full_env_as_str().map(|(k, _)| k.to_string()).filter(|k| is_internal_env(k)).collect();
    for k in internal {
        cmd.env_remove(k);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    for (k, v) in extra {
        cmd.env(k, v);
    }
}

impl Session {
    pub fn spawn(ctx: &egui::Context, opts: SpawnOptions) -> Result<Session, String> {
        let (cols, rows) = (100u16, 30u16);
        // Mitgelieferte ConPTY (Einstellung, Dateien neben paneldeck.exe) oder die von Windows (#183).
        let (pty, want_bundled) = crate::conpty::pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let console_host = crate::conpty::host_of(&*pair.master);
        crate::conpty::note_spawn(want_bundled, console_host);

        let mut cmd = CommandBuilder::new(&opts.shell.program);
        for a in &opts.shell.args {
            cmd.arg(a);
        }
        if let Some(cwd) = opts.cwd {
            // `\\wsl$`-Pfade ungeprüft (das Prüfen im UI-Thread könnte die WSL-VM starten und blockieren).
            if crate::wsl::is_wsl_unc(cwd) || std::path::Path::new(cwd).is_dir() {
                cmd.cwd(cwd);
            }
        }
        apply_env(&mut cmd, opts.inherit_env, std::env::vars_os(), &opts.env);

        // Job Object der Sitzung (#168): Die Shell startet schon darin (vendored portable-pty,
        // `PROC_THREAD_ATTRIBUTE_JOB_LIST`) – kein Zeitfenster zwischen Start und Zuweisung. Ihre Nachfahren gehören
        // dann sicher zu ihr, auch losgelöst. Lehnt Windows den Job beim Start ab, wird die Shell direkt danach
        // zugewiesen (mit Nachholen schon gestarteter Kinder); scheitert auch das, bleibt der Prozessbaum.
        let job = crate::winctl::SessionJob::new();
        let (spawned, in_job) = crate::winctl::spawn_in_job(job.as_ref(), || pair.slave.spawn_command(cmd));
        let child = spawned.map_err(|e| e.to_string())?;
        let job = Self::adopt_into_job(job, in_job, child.as_ref());
        let shell_pid = child.process_id();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: SharedWriter = Arc::new(Mutex::new(
            pair.master.take_writer().map_err(|e| e.to_string())?,
        ));

        let listener = Listener::new(writer.clone());
        let (title, bell, clipboard) = (listener.title.clone(), listener.bell.clone(), listener.clipboard.clone());

        // Etwas mehr Verlauf als eingestellt: `HistoryTrack` kürzt selbst auf das Limit und zählt dabei mit (#197).
        let config = term_config(opts.scrollback_lines + HISTORY_SLACK);
        let mut term = Term::new(config, &TermSize::new(cols as usize, rows as usize), listener);
        let mut processor: Processor = Processor::new();
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let hist_track = HistoryTrack::new(opts.scrollback_lines, dropped.clone());

        if let Some(text) = opts.restore_text {
            let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
            hist_track.advance(&mut processor, &mut term, text.as_bytes());
            let sep = format!("\r\n\x1b[2m\u{2500}\u{2500} {} \u{2500}\u{2500}\x1b[0m\r\n", crate::state::t("wiederhergestellter Verlauf", "restored history"));
            hist_track.advance(&mut processor, &mut term, sep.as_bytes());
        }

        let term = Arc::new(Mutex::new(term));
        let exited = Arc::new(AtomicBool::new(false));
        let shell_state: Arc<Mutex<ShellState>> = Arc::new(Mutex::new(ShellState::default()));
        let last_activity = Arc::new(std::sync::atomic::AtomicU64::new(mono_ms()));
        let stale_modes = Arc::new(AtomicBool::new(false));

        {
            let term = term.clone();
            let exited = exited.clone();
            let ctx = ctx.clone();
            let shell_state = shell_state.clone();
            let activity = last_activity.clone();
            let stale = stale_modes.clone();
            // Lesen und Verarbeiten getrennt: Der Verarbeitungs-Thread muss die Frist eines „Synchronized Update“
            // (CSI ?2026h) selbst durchsetzen. vte puffert sonst nach einem abgebrochenen Update (Programm stirbt
            // mitten im Frame, `cat` einer Datei mit ESC[?2026h) bis zu 2 MiB und das Terminal zeigt nichts mehr an.
            let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 65536];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            std::thread::spawn(move || {
                use std::sync::mpsc::RecvTimeoutError;
                let mut scanner = OscScanner::default();
                // Prompt-Anfang bei „Umbruch ausstehend“, dessen Zeile erst das nächste Zeichen festlegt (`advance_tracked`).
                let mut pending_mark: Option<u64> = None;
                let hist_track = hist_track;
                loop {
                    let msg = match processor.sync_timeout().sync_timeout() {
                        Some(deadline) => rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match msg {
                        Ok(bytes) => {
                            activity.store(mono_ms(), Ordering::Relaxed);
                            let (events, positions) = scanner.feed_at(&bytes);
                            {
                                let mut t = term.lock().unwrap_or_else(|e| e.into_inner());
                                if processor.sync_timeout().sync_timeout().is_some_and(|d| d <= std::time::Instant::now()) {
                                    let before = primary_history(&t);
                                    processor.stop_sync(&mut *t);
                                    hist_track.settle(&mut t, before);
                                }
                                advance_tracked(&mut processor, &mut t, &hist_track, &bytes, events, positions, &mut pending_mark, &shell_state, |ev, t| {
                                    let was = stale.load(Ordering::Relaxed);
                                    stale.store(next_stale_modes(was, ev, *t.mode()), Ordering::Relaxed);
                                });
                            }
                            ctx.request_repaint();
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            let mut t = term.lock().unwrap_or_else(|e| e.into_inner());
                            let before = primary_history(&t);
                            processor.stop_sync(&mut *t);
                            hist_track.settle(&mut t, before);
                            drop(t);
                            ctx.request_repaint();
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                // Restpuffer eines offenen Updates noch anzeigen.
                if processor.sync_timeout().sync_timeout().is_some() {
                    let mut t = term.lock().unwrap_or_else(|e| e.into_inner());
                    let before = primary_history(&t);
                    processor.stop_sync(&mut *t);
                    hist_track.settle(&mut t, before);
                }
                exited.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }

        let session = Session {
            term,
            dropped,
            shell_state,
            writer,
            master: pair.master,
            child: Mutex::new(child),
            title,
            exited,
            bell,
            clipboard,
            cols,
            rows,
            scroll_acc: 0.0,
            search: None,
            shell_id: opts.shell.id.clone(),
            mouse_down: None,
            mouse_last: None,
            hover_link: None,
            tee_on: AtomicBool::new(false),
            tee: Mutex::new(Vec::new()),
            last_activity,
            stale_modes,
            job,
            shell_pid,
            console_host,
        };
        if let Some(input) = opts.initial_input {
            session.write(input.as_bytes());
        }
        Ok(session)
    }

    /// Job Object der gerade gestarteten Shell (#168): `in_job` = schon im Job gestartet (`winctl::spawn_in_job`), sonst
    /// jetzt zuweisen (mit Nachholen schon gestarteter Kinder). `None` ohne Windows oder wenn das scheitert.
    #[cfg(windows)]
    fn adopt_into_job(job: Option<crate::winctl::SessionJob>, in_job: bool, child: &(dyn Child + Send + Sync)) -> Option<crate::winctl::SessionJob> {
        let job = job?;
        if in_job {
            return Some(job);
        }
        let (h, pid) = (child.as_raw_handle()?, child.process_id()?);
        job.adopt(h, pid).then_some(job)
    }

    #[cfg(not(windows))]
    fn adopt_into_job(_job: Option<crate::winctl::SessionJob>, _in_job: bool, _child: &(dyn Child + Send + Sync)) -> Option<crate::winctl::SessionJob> {
        None
    }

    /// Kennung des Job Objects dieser Sitzung (#168), `None` ohne Job (dann zählt nur der Prozessbaum).
    pub fn job_key(&self) -> Option<u64> {
        self.job.as_ref().map(|j| j.key())
    }

    /// Prozesse der Sitzung, die ihr Schließen überleben würden (#168): im Job, aber weder die Shell noch an ihrer
    /// Konsole (z. B. per `Start-Process` gestartet, ein GUI-Programm). Leer ohne Job.
    /// `info`: Prozessnamen/Fenster, für mehrere Sitzungen einmal ermittelt (`winctl::ProcInfo::default()`).
    pub fn lingering(&self, info: &crate::winctl::ProcInfo) -> Vec<crate::winctl::JobProc> {
        self.job.as_ref().map(|j| crate::winctl::lingering(j, self.shell_pid, info)).unwrap_or_default()
    }

    /// Alle Prozesse im Job der Sitzung beenden („Alle beenden“, #168). `false` ohne Job.
    pub fn end_job(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.terminate_all())
    }

    pub fn write(&self, bytes: &[u8]) {
        if self.tee_on.load(Ordering::Relaxed) {
            self.tee.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(bytes);
        }
        self.write_raw(bytes);
    }

    /// Wie `write`, aber ohne Broadcast-Mitschnitt: Eingaben eines Agenten dürfen nicht in fremden Terminals landen.
    pub fn write_raw(&self, bytes: &[u8]) {
        // Ein Befehl wird abgeschickt: die Shell verlässt ihren Prompt (siehe `stale_modes`).
        if bytes.contains(&b'\r') {
            self.stale_modes.store(false, Ordering::Relaxed);
        }
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Eingabe eines Agenten: Text wie eingefügt (Bracketed Paste, falls angefordert), optional mit Enter, in EINEM
    /// Schreibvorgang, ohne Broadcast; scrollt vorher ans Ende.
    pub fn send_text_agent(&self, text: &str, submit: bool) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bracketed = {
            let mut t = self.term.lock().unwrap_or_else(|e| e.into_inner());
            t.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
            t.mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
        };
        let mut out: Vec<u8> = Vec::new();
        if bracketed {
            let text: String = text.chars().filter(|&c| c >= ' ' && c != '\u{7f}' || c == '\t' || c == '\r').collect();
            out.extend_from_slice(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            out.extend_from_slice(text.as_bytes());
        }
        if submit {
            out.push(b'\r');
        }
        self.write_raw(&out);
    }

    /// Wie lange das Programm schon nichts mehr ausgegeben hat.
    pub fn quiet_for(&self) -> std::time::Duration {
        std::time::Duration::from_millis(mono_ms().saturating_sub(self.last_activity.load(Ordering::Relaxed)))
    }

    /// Prozess-ID der Shell dieser Sitzung (Kind des Pseudo-Terminals).
    pub fn pid(&self) -> Option<u32> {
        self.child.try_lock().ok().and_then(|c| c.process_id())
    }

    /// Anwendung hat den Cursor-Tasten-Modus (Pfeile als `ESC O A`) gesetzt.
    pub fn app_cursor(&self) -> bool {
        self.term.lock().unwrap_or_else(|e| e.into_inner()).mode().contains(alacritty_terminal::term::TermMode::APP_CURSOR)
    }

    pub fn alt_screen(&self) -> bool {
        self.term.lock().unwrap_or_else(|e| e.into_inner()).mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
    }

    /// Absolute Zeilennummer (ab Anfang des Verlaufs) der Cursorzeile.
    pub fn abs_cursor_line(&self) -> u64 {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        cursor_abs_in(&t, self.base())
    }

    /// Aus dem Verlauf gefallene Zeilen (Anfang der absoluten Zählung); nur unter der Sperre von `term` lesen.
    fn base(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Terminal-Zeilen von `from` bis `to` (absolut, einschließlich), je Zeile ein Eintrag, ohne Leerraum am Zeilenende.
    pub fn lines_abs(&self, from: u64, to: u64) -> Vec<String> {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        lines_abs_in(&t, self.base(), from, to)
    }

    /// Ausgabe eines `run_command` (MCP) aus dem Terminalinhalt, siehe [`run_output`].
    pub fn run_output(&self, cmd: &str, start_abs: u64, mark0: Option<u64>, mark: Option<u64>, advanced: bool) -> RunOutput {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        run_output(&t, self.base(), cmd, start_abs, mark0, mark, advanced)
    }

    /// Letzte `n` Zeilen des Verlaufs (ohne leere Zeilen am Ende) und ob es davor noch mehr gab.
    pub fn tail_text(&self, n: usize) -> (String, bool) {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let g = t.grid();
        let top = g.topmost_line();
        let bottom = g.bottommost_line();
        // Etwas mehr als n Zeilen holen (leere Zeilen unter dem Cursor werden danach abgeschnitten).
        let want = n as i32 + g.screen_lines() as i32;
        let start = Line((bottom.0 - want).max(top.0));
        let text = t.bounds_to_string(Point::new(start, Column(0)), Point::new(bottom, Column(g.columns() - 1)));
        let mut lines: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let more = start.0 > top.0 || lines.len() > n;
        let from = lines.len().saturating_sub(n);
        (lines[from..].join("\n"), more)
    }

    /// Nur der sichtbare Bildschirm (unabhängig davon, wie weit der Nutzer gescrollt hat).
    pub fn screen_text(&self) -> String {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let g = t.grid();
        let text = t.bounds_to_string(Point::new(Line(0), Column(0)), Point::new(Line(g.screen_lines() as i32 - 1), Column(g.columns() - 1)));
        let mut lines: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Eingaben mitschneiden (für Broadcast an andere Terminals).
    pub fn set_tee(&self, on: bool) {
        self.tee_on.store(on, Ordering::Relaxed);
        if !on {
            self.tee.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    pub fn take_tee(&self) -> Vec<u8> {
        std::mem::take(&mut *self.tee.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Text wie eingefügt senden (Bracketed Paste, falls die Anwendung es anfordert).
    pub fn send_text(&self, text: &str, submit: bool) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bracketed = self.term.lock().unwrap_or_else(|e| e.into_inner()).mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        if bracketed {
            // Steuerzeichen (v. a. ESC) entfernen: ein eingefügtes „\e[201~“ beendete sonst den Paste-Modus
            // vorzeitig und der Rest liefe als getippte Befehle.
            let text: String = text.chars().filter(|&c| c >= ' ' && c != '\u{7f}' || c == '\t' || c == '\r').collect();
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.write(text.as_bytes());
        }
        if submit {
            self.write(b"\r");
        }
    }

    /// Zeichen einer sichtbaren Zeile (nach Spalten, Platzhalterzellen als Leerzeichen).
    pub fn row_chars(&self, viewport_row: usize) -> Vec<char> {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let off = t.grid().display_offset() as i32;
        let line = alacritty_terminal::index::Line(viewport_row as i32 - off);
        if viewport_row >= self.rows as usize {
            return vec![];
        }
        (0..t.grid().columns())
            .map(|c| {
                let cell = &t.grid()[line][alacritty_terminal::index::Column(c)];
                if cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER) || cell.c == '\0' { ' ' } else { cell.c }
            })
            .collect()
    }

    /// Kopierwunsch eines Programms (OSC 52) abholen, falls einer wartet.
    pub fn take_clipboard(&self) -> Option<ClipboardRequest> {
        let req = std::mem::take(&mut *self.clipboard.lock().unwrap_or_else(|e| e.into_inner()));
        (!req.is_empty()).then_some(req)
    }

    pub fn title(&self) -> Option<String> {
        self.title.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|t| !t.trim().is_empty())
    }

    pub fn has_exited(&self) -> bool {
        if self.exited.load(Ordering::Relaxed) {
            return true;
        }
        // Unter ConPTY kommt das EOF am Ausgabekanal erst mit ClosePseudoConsole – also den Prozess selbst fragen.
        let gone = self.child.try_lock().is_ok_and(|mut c| matches!(c.try_wait(), Ok(Some(_))));
        if gone {
            self.exited.store(true, Ordering::Relaxed);
        }
        gone
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.lock().unwrap_or_else(|e| e.into_inner()).resize(TermSize::new(cols as usize, rows as usize));
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }

    /// Gesamten Verlauf (Scrollback + sichtbarer Bereich) als Text, auf `max_lines` begrenzt.
    pub fn dump_text(&self, max_lines: usize) -> String {
        let term = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let grid = term.grid();
        let top = grid.topmost_line();
        let bottom = grid.bottommost_line();
        let text = term.bounds_to_string(
            Point::new(top, Column(0)),
            Point::new(bottom, Column(grid.columns() - 1)),
        );
        let mut lines: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let start = lines.len().saturating_sub(max_lines);
        let _ = Line(0);
        lines[start..].join("\n")
    }
}

/// Meldet die Shell ihren Prompt, obwohl noch der alternative Bildschirm oder ein Maus-Modus aktiv ist?
/// Dann ist das Vollbildprogramm ohne Aufräumen beendet worden (`Session::stale_modes`).
pub fn prompt_with_stale_modes(mode: alacritty_terminal::term::TermMode) -> bool {
    use alacritty_terminal::term::TermMode;
    mode.intersects(TermMode::ALT_SCREEN | TermMode::MOUSE_MODE)
}

/// Neuer Wert von `Session::stale_modes` nach einem Ereignis im Ausgabestrom (`mode` = Modus der Emulation, nachdem
/// alles bis einschließlich der Sequenz verarbeitet ist):
/// - Prompt-Anfang (OSC 133;A): gesetzt, wenn dabei noch Vollbild/Maus-Modus aktiv ist, sonst gelöscht.
/// - Ein Programm schaltet Vollbild oder Maus-Modus ein (DECSET 47/1047/1049/1000/1002/1003): gelöscht – auch wenn
///   sich kein Modus-Bit ändert (1049h im schon aktiven alternativen Bildschirm). So hängt der Zustand nicht, wenn ein
///   Vollbildprogramm ohne abgeschickte Befehlszeile startet (Tastenkürzel der Shell, mc nach Strg+O) oder der Prompt
///   erst nach dem Enter eintraf.
/// - Sonst unverändert.
pub fn next_stale_modes(prev: bool, ev: &OscEvent, mode: alacritty_terminal::term::TermMode) -> bool {
    match ev {
        OscEvent::PromptStart => prompt_with_stale_modes(mode),
        OscEvent::FullscreenModeSet => false,
        _ => prev,
    }
}

fn apply_events(state: &Arc<Mutex<ShellState>>, events: Vec<OscEvent>, abs_line: u64) {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    for ev in events {
        st.integration_seen |= ev != OscEvent::FullscreenModeSet;
        match ev {
            OscEvent::PromptStart => {
                st.prompt_seq += 1;
                if st.marks.last() != Some(&abs_line) {
                    st.marks.push(abs_line);
                    if st.marks.len() > 2000 {
                        st.marks.remove(0);
                    }
                }
            }
            OscEvent::CommandStart => st.cmd_start = Some(std::time::Instant::now()),
            OscEvent::CommandDone { exit } => {
                // Dauer exakt (PowerShell: Historie) oder ab Befehlsstart (bash: PS0); ohne Quelle keine Dauer (kein Fehlalarm).
                let dur = st.pending_dur.take().or_else(|| st.cmd_start.map(|t| t.elapsed().as_millis() as u64));
                st.cmd_start = None;
                st.done_seq += 1;
                st.last_exit = exit;
                st.last_dur_ms = dur;
                st.done.push(CmdDone { exit, dur_ms: dur });
                if st.done.len() > 50 {
                    st.done.remove(0);
                }
            }
            OscEvent::Cwd(p) => {
                st.cwd_seq += 1;
                st.cwd = Some(p);
            }
            OscEvent::DurationMs(ms) => st.pending_dur = Some(ms),
            // Kommt nicht von der Shell-Integration; der Lese-Thread wertet es selbst aus (`next_stale_modes`).
            OscEvent::FullscreenModeSet => {}
        }
    }
}

impl Session {
    /// Springt zum vorherigen (`delta < 0`) bzw. nächsten (`delta > 0`) Prompt.
    pub fn jump_mark(&self, delta: i32) {
        let marks = self.shell_state.lock().unwrap_or_else(|e| e.into_inner()).marks.clone();
        let mut t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        jump_mark_in(&mut t, self.base(), &marks, delta);
    }
}

/// Zum vorherigen (`delta < 0`) bzw. nächsten (`delta > 0`) Prompt rollen (`marks` absolut, `base` = `Session::dropped`).
fn jump_mark_in<T: EventListener>(t: &mut Term<T>, base: u64, marks: &[u64], delta: i32) {
    {
        let hist = hist_of(t);
        // Bildschirmzeile 0 als absolute Zeile (Marken sind absolut, siehe `Session::dropped`).
        let screen0 = hist + base as i64;
        let top = screen0 - t.grid().display_offset() as i64;
        let target = if delta < 0 {
            marks.iter().rev().find(|m| (**m as i64) < top).copied()
        } else {
            marks.iter().find(|m| (**m as i64) > top).copied()
        };
        let offset = match target {
            Some(m) => (screen0 - m as i64).clamp(0, hist),
            None if delta > 0 => 0,
            None => hist,
        };
        let cur = t.grid().display_offset() as i64;
        t.scroll_display(alacritty_terminal::grid::Scroll::Delta((offset - cur) as i32));
    }
}

/// Anzahl der Zeilen im Verlauf über dem sichtbaren Bereich (absolute Zeile = `hist` + Bildschirmzeile).
fn hist_of<T>(t: &Term<T>) -> i64 {
    t.grid().total_lines().saturating_sub(t.grid().screen_lines()) as i64
}

/// Absolute Zeile der Cursorzeile (`base` = aus dem Verlauf gefallene Zeilen, `Session::dropped`).
fn cursor_abs_in<T>(t: &Term<T>, base: u64) -> u64 {
    (hist_of(t) + base as i64 + t.grid().cursor.point.line.0 as i64).max(0) as u64
}

/// Spielraum über dem eingestellten Verlauf (#197): alacritty verwirft bei vollem Verlauf die ältesten Zeilen, ohne das
/// zu melden. Die Emulation bekommt deshalb `HISTORY_SLACK` Zeilen mehr; `HistoryTrack` kürzt nach jedem Stück der
/// Ausgabe (höchstens `HISTORY_SLACK` Bytes, je Byte meist höchstens eine neue Zeile) selbst auf das Limit und zählt die
/// verworfenen Zeilen. Nur ein einzelnes Stück, das mehr Zeilen erzeugt (sehr großes „Synchronized Update“, CSI n S),
/// kann am Zähler vorbei Zeilen verlieren.
pub const HISTORY_SLACK: usize = 1000;

/// Verlaufslänge des Hauptbildschirms; `None` im alternativen Bildschirm (der hat keinen Verlauf).
fn primary_history<T>(t: &Term<T>) -> Option<usize> {
    (!t.mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN)).then(|| t.grid().history_size())
}

/// Zählt aus dem Verlauf gefallene Zeilen (#197), damit absolute Zeilen (Prompt-Marken, `run_command`) stabil bleiben.
pub struct HistoryTrack {
    limit: usize,
    dropped: Arc<std::sync::atomic::AtomicU64>,
}

impl HistoryTrack {
    pub fn new(limit: usize, dropped: Arc<std::sync::atomic::AtomicU64>) -> Self {
        HistoryTrack { limit, dropped }
    }

    fn base(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wie `Processor::advance`, in Stücken mit Kürzen und Zählen danach.
    fn advance<T: EventListener>(&self, p: &mut Processor, t: &mut Term<T>, bytes: &[u8]) {
        for piece in bytes.chunks(HISTORY_SLACK) {
            let before = primary_history(t);
            p.advance(t, piece);
            self.settle(t, before);
        }
    }

    /// Nach Ausgabe: Verlauf über dem Limit selbst kürzen und mitzählen; ein geschrumpfter Verlauf (CSI 3J, RIS) zählt
    /// ebenfalls als verworfen. `before` = `primary_history` vor der Ausgabe.
    fn settle<T>(&self, t: &mut Term<T>, before: Option<usize>) {
        let Some(h) = primary_history(t) else { return };
        let mut gone = before.map_or(0, |b| b.saturating_sub(h));
        if h > self.limit {
            let g = t.grid_mut();
            g.update_history(self.limit);
            g.update_history(self.limit + HISTORY_SLACK);
            gone += h - self.limit;
        }
        if gone > 0 {
            self.dropped.fetch_add(gone as u64, Ordering::Relaxed);
        }
    }
}

/// Einen Chunk der Programmausgabe in die Emulation geben und die Ereignisse der Shell-Integration (`events` mit ihren
/// `positions` aus `OscScanner::feed_at`) an der Cursorzeile auswerten, an der die Sequenz steht – nicht erst am
/// Chunk-Ende, wo ein mehrzeiliger Prompt schon fertig gedruckt ist. `on_event` sieht jedes Ereignis (auch
/// `FullscreenModeSet`) mit dem Stand der Emulation an dieser Stelle.
///
/// Prompt-Anfang bei „Umbruch ausstehend“ (#193): Füllt die letzte Ausgabezeile die Breite genau und lässt ConPTY den
/// Zeilenvorschub weg, steht der Cursor bei OSC 133;A noch am Rand der Ausgabezeile. Wo der Prompt beginnt, entscheidet
/// erst das nächste Zeichen: ein druckbares Zeichen bricht in die nächste Zeile um (bei vollem Verlauf rollt dabei nur
/// der Inhalt, die absolute Zeile bleibt gleich), CR oder eine Cursorbewegung (oh-my-posh, starship) heben den Umbruch
/// auf. Die Marke wird deshalb vorläufig gesetzt (`pending`, auch über Chunk-Grenzen) und danach Byte für Byte bis zu
/// dieser Entscheidung auf die Cursorzeile korrigiert.
fn advance_tracked<T: EventListener>(
    p: &mut Processor,
    t: &mut Term<T>,
    hs: &HistoryTrack,
    bytes: &[u8],
    events: Vec<OscEvent>,
    positions: Vec<usize>,
    pending: &mut Option<u64>,
    state: &Arc<Mutex<ShellState>>,
    mut on_event: impl FnMut(&OscEvent, &Term<T>),
) {
    let mut done = 0usize;
    for (ev, end) in events.into_iter().zip(positions) {
        let end = end.clamp(done, bytes.len());
        advance_resolving(p, t, hs, &bytes[done..end], pending, state);
        done = end;
        on_event(&ev, t);
        if ev == OscEvent::FullscreenModeSet {
            continue; // keine Shell-Integration
        }
        let prompt = ev == OscEvent::PromptStart;
        let abs = cursor_abs_in(t, hs.base());
        apply_events(state, vec![ev], abs);
        if prompt {
            *pending = t.grid().cursor.input_needs_wrap.then_some(abs);
        }
    }
    advance_resolving(p, t, hs, &bytes[done..], pending, state);
}

/// Wie `Processor::advance`; solange eine Marke offen ist (`advance_tracked`), Byte für Byte mit Prüfung danach.
fn advance_resolving<T: EventListener>(p: &mut Processor, t: &mut Term<T>, hs: &HistoryTrack, bytes: &[u8], pending: &mut Option<u64>, state: &Arc<Mutex<ShellState>>) {
    let mut i = 0;
    while pending.is_some() && i < bytes.len() {
        hs.advance(p, t, &bytes[i..i + 1]);
        i += 1;
        resolve_mark(t, hs.base(), pending, state);
    }
    hs.advance(p, t, &bytes[i..]);
}

/// Offene Marke festlegen, sobald der ausstehende Umbruch erledigt ist: auf die Zeile, in der der Cursor dann steht.
fn resolve_mark<T>(t: &Term<T>, base: u64, pending: &mut Option<u64>, state: &Arc<Mutex<ShellState>>) {
    let Some(old) = *pending else { return };
    let now = cursor_abs_in(t, base);
    if t.grid().cursor.input_needs_wrap && now == old {
        return; // noch offen (z. B. Farbsequenz vor dem Prompt-Text)
    }
    *pending = None;
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    if now != old && st.marks.last() == Some(&old) {
        st.marks.pop();
        if st.marks.last() != Some(&now) {
            st.marks.push(now);
        }
    }
}

/// Wie `lines_abs_in`, aber umgebrochene Zeilen sind zu einer logischen Zeile zusammengefügt (für Ausgaben an Leser).
fn text_abs_in<T: EventListener>(t: &Term<T>, base: u64, from: u64, to: u64) -> Vec<String> {
    let g = t.grid();
    let hist = hist_of(t);
    let off = hist + base as i64;
    let top = -hist;
    let bottom = g.bottommost_line().0 as i64;
    let a = (from as i64 - off).clamp(top, bottom);
    let b = (to as i64 - off).clamp(top, bottom);
    if b < a {
        return vec![];
    }
    let text = t.bounds_to_string(Point::new(Line(a as i32), Column(0)), Point::new(Line(b as i32), Column(g.columns() - 1)));
    text.lines().map(|l| l.trim_end().to_string()).collect()
}

/// Beginn der (ggf. umgebrochenen) Zeile, zu der `abs` gehört: läuft eine Zeile am Rand in die nächste über,
/// gehört sie zur selben logischen Zeile (lange Prompts in schmalen Terminals).
fn unwrap_start_in<T>(t: &Term<T>, base: u64, mut abs: u64) -> u64 {
    use alacritty_terminal::term::cell::Flags;
    let g = t.grid();
    let hist = hist_of(t);
    let off = hist + base as i64;
    let top = -hist;
    let last_col = Column(g.columns() - 1);
    while abs > 0 {
        let prev = abs as i64 - 1 - off;
        if prev < top || prev > g.bottommost_line().0 as i64 {
            break;
        }
        if g[Line(prev as i32)][last_col].flags.contains(Flags::WRAPLINE) {
            abs -= 1;
        } else {
            break;
        }
    }
    abs
}

/// Letzte Zeile der (ggf. umgebrochenen) logischen Zeile, die bei `abs` beginnt: läuft eine Zeile am Rand in die
/// nächste über, gehört diese noch dazu (lange eingegebene Befehle in schmalen Terminals).
fn wrap_end_in<T>(t: &Term<T>, base: u64, mut abs: u64) -> u64 {
    use alacritty_terminal::term::cell::Flags;
    let g = t.grid();
    let hist = hist_of(t);
    let bottom = g.bottommost_line().0 as i64;
    let last_col = Column(g.columns() - 1);
    loop {
        let row = abs as i64 - hist - base as i64;
        if row < -hist || row >= bottom || !g[Line(row as i32)][last_col].flags.contains(Flags::WRAPLINE) {
            return abs;
        }
        abs += 1;
    }
}

/// Terminal-Zeilen von `from` bis `to` (absolut, einschließlich), je Zeile ein Eintrag, ohne Leerraum am Zeilenende.
fn lines_abs_in<T: EventListener>(t: &Term<T>, base: u64, from: u64, to: u64) -> Vec<String> {
    let g = t.grid();
    let hist = hist_of(t);
    let off = hist + base as i64;
    let top = -(hist);
    let bottom = g.bottommost_line().0 as i64;
    let a = (from as i64 - off).clamp(top, bottom);
    let b = (to as i64 - off).clamp(top, bottom);
    if b < a {
        return vec![];
    }
    // Zeile für Zeile (Index = Zeilennummer − from): `bounds_to_string` über mehrere Zeilen fügt umgebrochene Zeilen
    // zu einer zusammen, dann stimmten die Indizes für Aufrufer nicht mehr.
    let last_col = Column(g.columns() - 1);
    (a..=b)
        .map(|r| t.bounds_to_string(Point::new(Line(r as i32), Column(0)), Point::new(Line(r as i32), last_col)).trim_end().to_string())
        .collect()
}

/// Ergebnis von [`run_output`].
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutput {
    /// Das Echo des Befehls wurde gefunden (sonst ist der Anfang der Ausgabe geraten).
    pub first_ok: bool,
    /// Ausgabezeilen (umgebrochene Zeilen zusammengefügt), ohne leere Zeilen am Ende.
    pub lines: Vec<String>,
}

/// Ausgabe eines per `run_command` eingegebenen Befehls: Zeilen nach dem Echo des Befehls bis vor den neuen Prompt.
/// `start_abs` = Cursorzeile beim Senden, `mark0` = Prompt-Anfang davor, `mark` = Anfang des neuen Prompts (OSC 133;A,
/// falls die Shell ihn meldete), `advanced` = der Befehl ist fertig (neuer Prompt da); alle absolut (`base` =
/// `Session::dropped`).
pub fn run_output<T: EventListener>(t: &Term<T>, base: u64, cmd: &str, start_abs: u64, mark0: Option<u64>, mark: Option<u64>, advanced: bool) -> RunOutput {
    let cursor = cursor_abs_in(t, base);
    // Ende: vor dem neuen Prompt. Die Marke ist sein genauer Anfang (`advance_tracked`); von dort nicht über umgebrochene
    // Zeilen zurücklaufen – eine Ausgabezeile, die die Breite genau füllt, trägt oft WRAPLINE (ConPTY lässt den
    // Zeilenvorschub weg) und fiele sonst samt Ausgabe dem Prompt zu (#193). Ohne Marke (cmd) bleibt nur die Cursorzeile;
    // ein umbrochener Prompt beginnt dann an der ersten Zeile davon.
    let end_incl = if advanced {
        match mark {
            Some(m) => m.saturating_sub(1),
            None => unwrap_start_in(t, base, cursor).saturating_sub(1),
        }
    } else {
        cursor
    };
    // Anfang der Ausgabe: die Zeile mit dem eingegebenen Befehl. Gesucht ab dem vorherigen Prompt-Anfang – die Cursorzeile
    // beim Senden ist unsicher, weil langsame Prompts (z. B. Git Bash mit Git-Status) zeilenweise mit Pausen erscheinen.
    let scan_from = mark0.map_or(start_abs, |m| m.min(start_abs));
    let all = lines_abs_in(t, base, scan_from, end_incl.max(scan_from));
    // Ein langer Befehl kann umbrochen sein: den ganzen Eingabeblock überspringen, nicht nur die erste Zeile. Das Echo
    // wird am Befehlstext selbst verfolgt (WRAPLINE fehlt, wenn ConPTY/PSReadLine die Eingabe neu zeichnen).
    let (first_ok, skip) = match find_echo(&all, cmd) {
        Some((_, last)) => (true, last + 1),
        None => {
            // Rückfall (Echo unvollständig, z. B. aus dem Verlauf gefallen): erste Zeile mit dem Befehlsanfang.
            let key: String = cmd.chars().take(12).collect();
            match all.iter().position(|l| l.contains(key.as_str())) {
                Some(i) => (true, (wrap_end_in(t, base, scan_from + i as u64) - scan_from) as usize + 1),
                None => (false, (start_abs - scan_from) as usize + 1),
            }
        }
    };
    // Ausgabe mit zusammengefügten Zeilenumbrüchen des Terminals (eine 300 Zeichen lange Zeile bleibt eine Zeile).
    let first_out = scan_from + skip as u64;
    let mut lines: Vec<String> = if first_out <= end_incl { text_abs_in(t, base, first_out, end_incl) } else { Vec::new() };
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    RunOutput { first_ok, lines }
}

/// Sucht in Terminal-Zeilen (eine Bildschirmzeile je Eintrag, wie `Session::lines_abs`) das Echo eines eingegebenen
/// Befehls und liefert die erste und letzte Zeile davon (Indizes in `rows`).
///
/// Ein langer Befehl läuft über mehrere Zeilen. Auf das WRAPLINE-Flag ist dabei kein Verlass: ConPTY und PSReadLine
/// zeichnen die Eingabezeile oft mit eigenen Zeilenvorschüben bzw. Cursorsprüngen neu, dann fehlt das Flag, und
/// Leerzeichen am Zeilenrand gehen durch das Abschneiden am Zeilenende verloren. Deshalb wird der Befehlstext selbst
/// verfolgt (ohne Leerraum verglichen): Er beginnt irgendwo in einer Zeile (nach dem Prompt) und setzt sich in den
/// folgenden Zeilen fort, bis er vollständig ist. `None`, wenn kein vollständiges Echo gefunden wird.
pub fn find_echo(rows: &[String], cmd: &str) -> Option<(usize, usize)> {
    let strip = |s: &str| -> Vec<char> { s.chars().filter(|c| !c.is_whitespace()).collect() };
    let want = strip(cmd);
    if want.is_empty() {
        return None;
    }
    let rows: Vec<Vec<char>> = rows.iter().map(|r| strip(r)).collect();
    for (i, r) in rows.iter().enumerate() {
        // Ganzer Befehl in einer Zeile (kurze Befehle, wie bisher).
        if r.windows(want.len()).any(|w| w == want.as_slice()) {
            return Some((i, i));
        }
        // Sonst: ein Zeilenende, das ein Anfang des Befehls ist (längster Kandidat zuerst), und die Fortsetzung danach.
        'cand: for p in 0..r.len() {
            let head = &r[p..];
            if !want.starts_with(head) {
                continue;
            }
            let mut done = head.len();
            for (j, next) in rows.iter().enumerate().skip(i + 1) {
                let rest = &want[done..];
                if next.is_empty() {
                    continue 'cand;
                }
                if next.starts_with(rest) {
                    return Some((i, j));
                }
                if !rest.starts_with(next) {
                    continue 'cand;
                }
                done += next.len();
            }
        }
    }
    None
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.get_mut().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn spawn_echo_and_read_grid() {
        let ctx = egui::Context::default();
        let shell = ShellSpec {
            id: "sh".into(),
            label: "sh".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hallo-paneldeck; sleep 0.3".into()],
        };
        let s = Session::spawn(
            &ctx,
            SpawnOptions {
                shell: &shell,
                cwd: None,
                restore_text: Some("alter verlauf"),
                initial_input: None,
                scrollback_lines: 1000,
                env: vec![],
                inherit_env: false,
            },
        )
        .unwrap();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if s.dump_text(100).contains("hallo-paneldeck") {
                break;
            }
        }
        let dump = s.dump_text(100);
        assert!(dump.contains("hallo-paneldeck"), "dump: {dump}");
        assert!(dump.contains("alter verlauf"), "dump: {dump}");
    }

    #[test]
    fn unterminated_sync_update_still_shows_output() {
        // Programm beginnt ein Synchronized Update und beendet es nie, läuft aber weiter.
        let ctx = egui::Context::default();
        let shell = ShellSpec {
            id: "sh".into(),
            label: "sh".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf '\\033[?2026hsichtbar-nach-frist'; sleep 2".into()],
        };
        let s = Session::spawn(
            &ctx,
            SpawnOptions { shell: &shell, cwd: None, restore_text: None, initial_input: None, scrollback_lines: 100, env: vec![], inherit_env: false },
        )
        .unwrap();
        let mut ok = false;
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if s.dump_text(10).contains("sichtbar-nach-frist") {
                ok = true;
                break;
            }
        }
        assert!(ok, "Ausgabe blieb im Sync-Puffer hängen: {}", s.dump_text(10));
    }
}

#[cfg(test)]
mod echo_tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;

    /// Bytes durch die echte Emulation schicken und die Bildschirmzeilen wie `Session::lines_abs` lesen.
    fn rows(cols: usize, bytes: &str) -> Vec<String> {
        let mut term = Term::new(Config::default(), &TermSize::new(cols, 40), VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut term, bytes.as_bytes());
        let g = term.grid();
        let last = term.grid().cursor.point.line.0;
        (0..=last)
            .map(|r| term.bounds_to_string(Point::new(Line(r), Column(0)), Point::new(Line(r), Column(g.columns() - 1))).trim_end().to_string())
            .collect()
    }

    /// Ausgabe nach dem Echo bis vor den nächsten Prompt (Zeilen, die mit `next_prompt` beginnen).
    fn output(rows: &[String], cmd: &str, next_prompt: &str) -> String {
        let (_, last) = find_echo(rows, cmd).unwrap_or_else(|| panic!("kein Echo in {rows:?}"));
        let end = rows.iter().rposition(|l| l.starts_with(next_prompt)).unwrap();
        // Leere Zeilen vor dem Prompt fallen weg (wie im run_command-Ausführer).
        rows[last + 1..end].join("\n").trim_end_matches('\n').to_string()
    }

    /// Text auf Zeilen der Breite `cols` mit festen Zeilenvorschüben verteilen (so zeichnet ConPTY/PSReadLine oft neu:
    /// ohne WRAPLINE). `start` = Spalte, an der der Text beginnt.
    fn hard_wrapped(text: &str, start: usize, cols: usize) -> String {
        let mut out = String::new();
        let mut col = start;
        for c in text.chars() {
            if col == cols {
                out.push_str("\r\n");
                col = 0;
            }
            out.push(c);
            col += 1;
        }
        out
    }

    const PS_CMD: &str = "$env:PANELDECK_SESSION=$null; '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"whoami\",\"arguments\":{}}}' | & 'E:\\paneldeck-test\\tmp-wrap-long-path-test\\smoke\\paneldeck.exe' mcp | Out-String";

    #[test]
    fn powershell_long_command_soft_wrapped() {
        let prompt = "PS E:\\paneldeck-test\\tmp-wrap-long-path-test\\smoke\\proj> ";
        let r = rows(80, &format!("{prompt}{PS_CMD}\r\n{{\"result\":{{}}}}\r\n\r\n{prompt}"));
        assert!(r.len() >= 5, "soll über mehrere Zeilen laufen: {r:?}");
        assert_eq!(output(&r, PS_CMD, "PS "), "{\"result\":{}}");
    }

    #[test]
    fn powershell_long_command_hard_wrapped_without_wrapline() {
        // Wie in der Fehlermeldung: der Umbruch fällt genau vor „ mcp“ – mit festem Zeilenvorschub statt WRAPLINE.
        let at = PS_CMD.find(" mcp |").unwrap();
        let base = "PS E:\\paneldeck-test\\tmp-wrap-long-path-test\\proj";
        // Ordnername so auffüllen, dass „ mcp“ (bzw. das Leerzeichen davor am Zeilenende) genau am Rand einer 100er-Zeile liegt.
        for (cols, shift) in [(100, 0), (100, 1), (97, 0), (64, 0), (120, 0)] {
            let pad = if cols == 100 { (200 - (base.len() + 2 + at + shift) % 100) % 100 } else { 0 };
            let prompt = format!("{base}{}> ", "x".repeat(pad));
            if cols == 100 {
                assert_eq!((prompt.len() + at + shift) % 100, 0);
            }
            let echo = hard_wrapped(&format!("{prompt}{PS_CMD}"), 0, cols);
            let r = rows(cols, &format!("{echo}\r\n{{\"result\":{{}}}}\r\n\r\n{prompt}"));
            let out = output(&r, PS_CMD, "PS ");
            assert_eq!(out, "{\"result\":{}}", "Breite {cols}: {r:?}");
            assert!(!out.contains("Out-String"));
        }
    }

    #[test]
    fn command_start_split_across_rows() {
        // Der Prompt füllt die Zeile fast: schon der Befehlsanfang (die ersten 12 Zeichen) wird geteilt.
        let prompt = "PS E:\\".to_string() + &"x".repeat(32) + "> ";
        let cmd = "Get-ChildItem -Path . -Recurse -Filter *.rs | Measure-Object";
        let echo = hard_wrapped(&format!("{prompt}{cmd}"), 0, 45);
        let r = rows(45, &format!("{echo}\r\nCount : 3\r\n\r\n{prompt}"));
        assert_eq!(output(&r, cmd, "PS "), "Count : 3");
    }

    #[test]
    fn cmd_long_command_three_rows() {
        let prompt = "C:\\Users\\nutzer\\Projekte\\ein-recht-langer-ordnername>";
        let cmd = "echo eins zwei drei vier fuenf sechs sieben acht neun zehn elf zwoelf dreizehn vierzehn";
        let r = rows(60, &format!("{prompt}{cmd}\r\neins zwei drei vier fuenf sechs sieben acht neun zehn elf zwoelf dreizehn vierzehn\r\n\r\n{prompt}"));
        let (first, last) = find_echo(&r, cmd).unwrap();
        assert_eq!((first, last), (0, 2), "{r:?}");
        let out = output(&r, cmd, "C:\\");
        assert!(out.starts_with("eins zwei") && !out.contains(prompt), "{out}");
    }

    #[test]
    fn bash_two_line_prompt_long_command() {
        let p1 = "nutzer@rechner MINGW64 /e/paneldeck-test/tmp-wrap-long-path-test (main)";
        let cmd = "printf '%s\\n' alpha beta gamma delta epsilon zeta eta theta iota kappa lambda my && echo fertig";
        // Readline erzwingt den Umbruch am Rand mit Leerzeichen + CR (das Leerzeichen wird danach überschrieben).
        let echo = hard_wrapped(&format!("$ {cmd}"), 0, 50).replace("\r\n", " \r");
        let r = rows(50, &format!("{p1}\r\n{echo}\r\nalpha\r\nfertig\r\n{p1}\r\n$ "));
        let (first, last) = find_echo(&r, cmd).unwrap();
        // Die erste Prompt-Zeile ist selbst umbrochen; das Echo beginnt in der Zeile mit „$ “ und läuft über zwei Zeilen.
        assert_eq!(first, r.iter().position(|l| l.starts_with("$ printf")).unwrap(), "{r:?}");
        assert_eq!(last, first + 1, "{r:?}");
        let end = r.iter().rposition(|l| l.starts_with("nutzer@")).unwrap();
        assert_eq!(r[last + 1..end].join("\n"), "alpha\nfertig");
    }

    #[test]
    fn short_command_unchanged() {
        let r = rows(80, "PS C:\\> echo kind-ok\r\nkind-ok\r\nPS C:\\> ");
        assert_eq!(find_echo(&r, "echo kind-ok"), Some((0, 0)));
        assert_eq!(output(&r, "echo kind-ok", "PS "), "kind-ok");
        // Erste Zeile mit dem Echo gewinnt, auch wenn die Ausgabe den Befehl wiederholt.
        let r = rows(80, "PS C:\\> echo x\r\necho x\r\nPS C:\\> ");
        assert_eq!(find_echo(&r, "echo x"), Some((0, 0)));
    }

    #[test]
    fn no_or_incomplete_echo() {
        let r = rows(80, "PS C:\\> \r\nirgendwas\r\nPS C:\\> ");
        assert_eq!(find_echo(&r, "Get-Date"), None);
        // Nur der Anfang ist da (Rest fehlt): kein vollständiges Echo.
        let r = rows(20, "PS C:\\> Get-ChildIt\r\nanderes\r\n");
        assert_eq!(find_echo(&r, "Get-ChildItem -Force"), None);
        assert_eq!(find_echo(&r, "   "), None);
    }
}

/// Umgebung der Terminals (#175): plattformunabhängig, ohne Prozess zu starten.
#[cfg(test)]
mod env_tests {
    use super::*;

    fn envs(cmd: &CommandBuilder) -> std::collections::BTreeMap<String, String> {
        cmd.iter_full_env_as_str().map(|(k, v)| (k.to_uppercase(), v.to_string())).collect()
    }

    #[test]
    fn inherit_env_uses_exactly_the_process_environment_plus_our_variables() {
        use std::ffi::OsString;
        let process = || {
            vec![
                (OsString::from("PATH"), OsString::from(r"C:\custom\bin")),
                (OsString::from("MY_VAR"), OsString::from("from-shell")),
                (OsString::from("PANELDECK_SESSION"), OsString::from("99")),
                (OsString::from("PANELDECK_HUB_DATA"), OsString::from(r"C:\other-hub")),
                (OsString::from("PANELDECK_UPDATE_KEEP_SESSIONS"), OsString::from("1")),
                (OsString::from("PANELDECK_UPDATE_TEST_FEED"), OsString::from(r"C:\feed")),
                (OsString::from("paneldeck_shot"), OsString::from("1")),
                (OsString::from("PANELDECK_DEMO_DIR"), OsString::from(r"C:\demo")),
                (OsString::from("PANELDECK_DATA"), OsString::from(r"C:\data")),
            ]
        };
        let ours = vec![("PANELDECK_SESSION".to_string(), "7".to_string()), ("PATH".to_string(), r"C:\profile\bin".to_string())];
        // An: genau die Prozessumgebung (nichts aus der Registry), interne Variablen weg, unsere gewinnen.
        let mut cmd = CommandBuilder::new("cmd.exe");
        apply_env(&mut cmd, true, process(), &ours);
        let e = envs(&cmd);
        assert_eq!(e.get("MY_VAR").map(String::as_str), Some("from-shell"));
        assert_eq!(e.get("PATH").map(String::as_str), Some(r"C:\profile\bin"), "Profilvariable gewinnt");
        assert_eq!(e.get("PANELDECK_SESSION").map(String::as_str), Some("7"), "eigene Sitzung, nicht die geerbte");
        assert!(!e.contains_key("PANELDECK_HUB_DATA") && !e.contains_key("PANELDECK_UPDATE_KEEP_SESSIONS"), "interne Variablen fremder Hubs fallen weg");
        assert!(!e.contains_key("PANELDECK_UPDATE_TEST_FEED") && !e.contains_key("PANELDECK_SHOT") && !e.contains_key("PANELDECK_DEMO_DIR"), "Test-Variablen fallen weg");
        assert!(is_internal_env("paneldeck_demo_lang") && is_internal_env("PANELDECK_TOKEN") && !is_internal_env("PANELDECK_DATA") && !is_internal_env("PANELDECK_DEMO"));
        assert_eq!(e.get("PANELDECK_DATA").map(String::as_str), Some(r"C:\data"), "öffentliche Variable bleibt");
        assert_eq!(e.get("TERM").map(String::as_str), Some("xterm-256color"));
        let mut want: Vec<&str> = vec!["COLORTERM", "MY_VAR", "PANELDECK_DATA", "PANELDECK_SESSION", "PATH", "TERM"];
        want.sort_unstable();
        assert_eq!(e.keys().map(String::as_str).collect::<Vec<_>>(), want, "nichts sonst (keine Registry)");
        // Ohne Profil-PATH bleibt der geerbte PATH.
        let mut cmd = CommandBuilder::new("cmd.exe");
        apply_env(&mut cmd, true, process(), &[]);
        assert_eq!(envs(&cmd).get("PATH").map(String::as_str), Some(r"C:\custom\bin"));
    }

    #[test]
    fn default_env_keeps_portable_pty_base_but_drops_internal_variables() {
        // Aus (Standard): die Basis von portable-pty bleibt (Prozess + Registry), `process` wird nicht benutzt.
        let mut base = CommandBuilder::new("cmd.exe");
        base.env("PANELDECK_PORT", "1234"); // wie geerbt aus dem Terminal eines anderen Hubs
        let before = envs(&base);
        apply_env(&mut base, false, vec![(std::ffi::OsString::from("ONLY_IN_ARG"), std::ffi::OsString::from("x"))], &[("PANELDECK_SESSION".into(), "3".into())]);
        let e = envs(&base);
        assert!(!e.contains_key("ONLY_IN_ARG"));
        assert!(!e.contains_key("PANELDECK_PORT"), "interne Variable entfernt");
        assert_eq!(e.get("PANELDECK_SESSION").map(String::as_str), Some("3"));
        for (k, v) in before.iter().filter(|(k, _)| !k.starts_with("PANELDECK_") && *k != "TERM" && *k != "COLORTERM") {
            assert_eq!(e.get(k), Some(v), "{k} aus der Basis bleibt");
        }
    }
}

#[cfg(test)]
mod run_output_tests {
    //! `run_command`-Ausgabe mit fester Breite durch die echte Emulation und die Shell-Integration (#193).
    use super::*;
    use alacritty_terminal::event::VoidListener;

    const COLS: usize = 40;
    /// PowerShell-Prompt wie `PS_INIT`: Befehlsende, Ordner, Prompt-Anfang, Prompt-Text, Eingabe-Anfang.
    const PROMPT: &str = "\x1b]133;D;0\x07\x1b]9;9;\"C:\\\"\x07\x1b]133;A\x07PS C:\\> \x1b]133;B\x07";

    /// Emulation plus Shell-Zustand, gefüttert wie im Lese-Thread von `Session::spawn` (Chunk an den Ereignissen geteilt).
    struct Fake {
        term: Term<VoidListener>,
        p: Processor,
        sc: OscScanner,
        pending: Option<u64>,
        hs: HistoryTrack,
        st: Arc<Mutex<ShellState>>,
    }

    impl Fake {
        fn new(cols: usize, lines: usize) -> Self {
            Self::with_history(cols, lines, Config::default().scrolling_history)
        }

        /// Wie `Session::spawn`: Emulation mit `HISTORY_SLACK` Zeilen mehr, `HistoryTrack` kürzt auf `scrolling_history`.
        fn with_history(cols: usize, lines: usize, scrolling_history: usize) -> Self {
            let config = Config { scrolling_history: scrolling_history + HISTORY_SLACK, ..Config::default() };
            Fake {
                term: Term::new(config, &TermSize::new(cols, lines), VoidListener),
                p: Processor::new(),
                sc: OscScanner::default(),
                pending: None,
                hs: HistoryTrack::new(scrolling_history, Arc::new(std::sync::atomic::AtomicU64::new(0))),
                st: Arc::new(Mutex::new(ShellState::default())),
            }
        }

        fn base(&self) -> u64 {
            self.hs.base()
        }

        fn cursor(&self) -> u64 {
            cursor_abs_in(&self.term, self.base())
        }

        /// Wie der Lese-Thread von `Session::spawn` (`advance_tracked`).
        fn feed(&mut self, bytes: &str) {
            let bytes = bytes.as_bytes();
            let (events, positions) = self.sc.feed_at(bytes);
            advance_tracked(&mut self.p, &mut self.term, &self.hs, bytes, events, positions, &mut self.pending, &self.st, |_, _| {});
        }

        fn last_mark(&self) -> Option<u64> {
            self.st.lock().unwrap().marks.last().copied()
        }
    }

    /// Ein `run_command`: Prompt steht, `cmd` wird eingegeben (Echo + Enter), dann kommt `out` (Ausgabe samt neuem Prompt).
    /// `history` füllt vorher den Bildschirm (Zeilen laufen in den Verlauf).
    fn run(cols: usize, history: usize, cmd: &str, out: &str) -> RunOutput {
        run_in(Fake::new(cols, 8), history, cmd, out)
    }

    fn run_in(mut f: Fake, history: usize, cmd: &str, out: &str) -> RunOutput {
        for i in 0..history {
            f.feed(&format!("alt {i}\r\n"));
        }
        f.feed(PROMPT);
        let start_abs = f.cursor();
        let mark0 = f.last_mark();
        let seq0 = f.st.lock().unwrap().prompt_seq;
        f.feed(&format!("{cmd}\r\n"));
        f.feed(out);
        assert!(f.st.lock().unwrap().prompt_seq > seq0, "kein neuer Prompt");
        run_output(&f.term, f.base(), cmd, start_abs, mark0, f.last_mark(), true)
    }

    fn text(r: &RunOutput) -> String {
        assert!(r.first_ok, "Echo nicht gefunden: {r:?}");
        r.lines.join("\n")
    }

    #[test]
    fn exact_width_line_soft_wrapped_by_conpty() {
        // ConPTY lässt nach einer Zeile, die die Breite genau füllt, den Zeilenvorschub weg (die Zeile gilt als umbrochen):
        // OSC 133;A kommt bei „Umbruch ausstehend“, der Prompt-Text landet per Autowrap in der nächsten Zeile (WRAPLINE).
        let line = "x".repeat(COLS);
        for history in [0, 20] {
            let r = run(COLS, history, "'x'*40", &format!("{line}{PROMPT}"));
            assert_eq!(text(&r), line, "Verlauf {history}");
        }
    }

    #[test]
    fn exact_width_line_with_newline() {
        let line = "y".repeat(COLS);
        let r = run(COLS, 0, "'y'*40", &format!("{line}\r\n{PROMPT}"));
        assert_eq!(text(&r), line);
        // Mit Leerzeile vor dem Prompt (PowerShell nach formatierter Ausgabe).
        let r = run(COLS, 20, "'y'*40", &format!("{line}\r\n\r\n{PROMPT}"));
        assert_eq!(text(&r), line);
    }

    #[test]
    fn width_plus_one() {
        let line = "z".repeat(COLS + 1);
        let r = run(COLS, 0, "'z'*41", &format!("{line}\r\n{PROMPT}"));
        assert_eq!(text(&r), line, "eine umbrochene Zeile bleibt eine Zeile");
        // Eine Zeichen weniger als die Breite: kein Umbruch, kein ausstehender Umbruch.
        let short = "z".repeat(COLS - 1);
        let r = run(COLS, 20, "'z'*39", &format!("{short}\r\n{PROMPT}"));
        assert_eq!(text(&r), short);
        // Zwei volle Zeilen plus eins, im Verlauf.
        let long = "z".repeat(2 * COLS + 1);
        let r = run(COLS, 20, "'z'*81", &format!("{long}\r\n{PROMPT}"));
        assert_eq!(text(&r), long);
    }

    #[test]
    fn multiple_wraps() {
        // Drei volle Zeilen am Stück (genau 3 × Breite), ohne Zeilenvorschub vor dem Prompt.
        let line = "m".repeat(3 * COLS);
        let r = run(COLS, 20, "'m'*120", &format!("{line}{PROMPT}"));
        assert_eq!(text(&r), line);
        // Zweieinhalb Zeilen, dann zwei Zeilen genau in Breite (je mit und ohne Zeilenvorschub).
        let a = "a".repeat(2 * COLS + COLS / 2);
        let b = "b".repeat(COLS);
        let c = "c".repeat(COLS);
        let r = run(COLS, 20, "dreierlei", &format!("{a}\r\n{b}\r\n{c}{PROMPT}"));
        assert_eq!(text(&r), format!("{a}\n{b}\n{c}"));
    }

    #[test]
    fn exact_width_followed_by_wrapped_prompt() {
        // Der neue Prompt ist selbst länger als die Zeile (langer Ordner) und folgt direkt auf eine volle Ausgabezeile.
        let long = format!("\x1b]133;D;0\x07\x1b]133;A\x07PS C:\\{}> \x1b]133;B\x07", "o".repeat(COLS + 10));
        let line = "w".repeat(COLS);
        let r = run(COLS, 20, "'w'*40", &format!("{line}{long}"));
        assert_eq!(text(&r), line);
        let r = run(COLS, 20, "'w'*40", &format!("kurz\r\n{line}\r\n{long}"));
        assert_eq!(text(&r), format!("kurz\n{line}"));
    }

    #[test]
    fn no_output_stays_empty() {
        let r = run(COLS, 20, "$null", PROMPT);
        assert_eq!(text(&r), "");
    }

    #[test]
    fn mark_after_pending_wrap_is_next_row() {
        let mut f = Fake::new(COLS, 8);
        f.feed(&"q".repeat(COLS));
        assert!(f.term.grid().cursor.input_needs_wrap);
        f.feed("\x1b]133;A\x07$ ");
        assert_eq!(f.last_mark(), Some(1));
        // Ohne ausstehenden Umbruch: die Cursorzeile selbst.
        f.feed("\r\n\x1b]133;A\x07$ ");
        assert_eq!(f.last_mark(), Some(2));
        // Marke am Chunk-Ende, Prompt-Text (nach einer Farbsequenz) erst im nächsten Chunk.
        f.feed(&format!("\r\n{}\x1b]133;A\x07", "q".repeat(COLS)));
        assert_eq!(f.last_mark(), Some(3), "vorläufig: die Ausgabezeile");
        f.feed("\x1b[32m$ ");
        assert_eq!(f.last_mark(), Some(4));
    }

    #[test]
    fn mark_when_cr_or_cursor_move_cancels_the_wrap() {
        // oh-my-posh/starship: CR bzw. Cursorbewegung vor dem Prompt-Text heben den ausstehenden Umbruch auf; der Prompt
        // überschreibt dann die Ausgabezeile (bzw. steht, wo der Cursor hinspringt) – die Marke folgt dem.
        let mut f = Fake::new(COLS, 8);
        f.feed(&format!("{}\x1b]133;A\x07\r$ ", "q".repeat(COLS)));
        assert_eq!(f.last_mark(), Some(0));
        f.feed(&format!("\r\n{}\x1b]133;A\x07\x1b[5;1H$ ", "q".repeat(COLS)));
        assert_eq!(f.last_mark(), Some(4));
        // Wie im Lauf: Ausgabe mit CR vor dem Prompt endet mit dem überschriebenen Rest nicht in der Ausgabe.
        let line = "r".repeat(COLS);
        let prompt_cr = "\x1b]133;D;0\x07\x1b]133;A\x07\rPS C:\\> \x1b]133;B\x07";
        let r = run(COLS, 20, "'r'*40", &format!("kurz\r\n{line}{prompt_cr}"));
        assert_eq!(text(&r), "kurz");
    }

    #[test]
    fn exact_width_with_full_scrollback() {
        // Verlauf voll (kleines Limit): der Umbruch in der untersten Zeile rollt nur, die absolute Zeile wächst nicht –
        // die Marke darf dann nicht eine Zeile tiefer liegen (sonst endete die Ausgabe mit dem Prompt).
        let line = "f".repeat(COLS);
        for nl in ["", "\r\n"] {
            let mut f = Fake::with_history(COLS, 8, 10);
            for i in 0..40 {
                f.feed(&format!("alt {i}\r\n"));
            }
            assert_eq!(f.term.grid().history_size(), 10, "Verlauf voll");
            f.feed(&format!("{line}{nl}{PROMPT}"));
            let m = f.last_mark().unwrap();
            let rows = lines_abs_in(&f.term, f.base(), m - 1, m);
            assert_eq!(rows, vec![line.clone(), "PS C:\\>".to_string()], "Zeilenvorschub {nl:?}");
        }
    }

    #[test]
    fn full_history_keeps_absolute_lines_stable() {
        // #197: bei vollem Verlauf wächst die absolute Zählung weiter (verworfene Zeilen werden mitgezählt).
        let mut f = Fake::with_history(COLS, 8, 10);
        f.feed("\x1b]133;A\x07$ ");
        let m0 = f.last_mark().unwrap();
        for i in 0..100 {
            f.feed(&format!("zeile {i}\r\n"));
        }
        assert_eq!(f.term.grid().history_size(), 10, "Verlauf auf das Limit gekürzt");
        assert_eq!(f.base(), 100 + 1 - 8 - 10, "verworfene Zeilen gezählt");
        assert_eq!(f.cursor(), 100, "Cursor in absoluter Zeile 100");
        assert_eq!(lines_abs_in(&f.term, f.base(), 99, 99), vec!["zeile 99".to_string()]);
        // Die erste Marke ist aus dem Verlauf gefallen: Lesen davon liefert die älteste noch vorhandene Zeile.
        assert_eq!(m0, 0);
        assert_eq!(lines_abs_in(&f.term, f.base(), 0, 0), vec!["zeile 83".to_string()]);
        // Verlauf löschen (CSI 3J) zählt ebenfalls als verworfen: die Bildschirmzeilen behalten ihre Nummern.
        f.feed("\x1b[3J");
        assert_eq!(f.term.grid().history_size(), 0);
        assert_eq!(f.cursor(), 100);
        assert_eq!(lines_abs_in(&f.term, f.base(), 99, 99), vec!["zeile 99".to_string()]);
    }

    #[test]
    fn run_command_with_full_history() {
        // #197: vorher lieferte schon `echo hi` bei vollem Verlauf nichts (Echo nicht gefunden).
        for history in [40, 200] {
            let r = run_in(Fake::with_history(COLS, 8, 10), history, "echo hi", &format!("hi\r\n{PROMPT}"));
            assert_eq!(text(&r), "hi", "Verlauf {history}");
            // Ausgabe länger als Bildschirm und Verlauf zusammen: der Anfang ist weg, der Rest stimmt.
            let many: Vec<String> = (0..30).map(|i| format!("z{i}")).collect();
            let r = run_in(Fake::with_history(COLS, 8, 10), history, "1..30", &format!("{}\r\n{PROMPT}", many.join("\r\n")));
            assert!(!r.first_ok, "Echo ist aus dem Verlauf gefallen");
            let line = "f".repeat(COLS);
            let r = run_in(Fake::with_history(COLS, 8, 10), history, "'f'*40", &format!("{line}{PROMPT}"));
            assert_eq!(text(&r), line, "genaue Breite, Verlauf {history}");
            let r = run_in(Fake::with_history(COLS, 8, 10), history, "'f'*40", &format!("{line}\r\n{PROMPT}"));
            assert_eq!(text(&r), line, "genaue Breite mit Zeilenvorschub, Verlauf {history}");
        }
        // Mehrere Befehle hintereinander in derselben Sitzung (mark0 = Marke des vorherigen Prompts).
        let mut f = Fake::with_history(COLS, 8, 10);
        for i in 0..50 {
            f.feed(&format!("alt {i}\r\n"));
        }
        f.feed(PROMPT);
        for i in 0..20 {
            let start_abs = f.cursor();
            let mark0 = f.last_mark();
            let cmd = format!("echo n{i}");
            f.feed(&format!("{cmd}\r\nn{i}\r\n{PROMPT}"));
            let r = run_output(&f.term, f.base(), &cmd, start_abs, mark0, f.last_mark(), true);
            assert_eq!(text(&r), format!("n{i}"), "Befehl {i}");
        }
    }

    #[test]
    fn prompt_jumps_with_full_history() {
        // Prompt-Navigation: Marken sind absolut; bei vollem Verlauf landen die Sprünge trotzdem auf dem Prompt.
        let mut f = Fake::with_history(COLS, 8, 30);
        for i in 0..60 {
            f.feed(&format!("{PROMPT}cmd{i}\r\nout{i}\r\nout{i}b\r\n"));
        }
        f.feed(PROMPT);
        assert_eq!(f.term.grid().history_size(), 30);
        let marks = f.st.lock().unwrap().marks.clone();
        let top_row = |f: &Fake| {
            let off = f.term.grid().display_offset() as i32;
            f.term.bounds_to_string(Point::new(Line(-off), Column(0)), Point::new(Line(-off), Column(COLS - 1))).trim_end().to_string()
        };
        jump_mark_in(&mut f.term, f.hs.base(), &marks, -1);
        assert_eq!(top_row(&f), "PS C:\\> cmd57");
        jump_mark_in(&mut f.term, f.hs.base(), &marks, -1);
        assert_eq!(top_row(&f), "PS C:\\> cmd56");
        jump_mark_in(&mut f.term, f.hs.base(), &marks, 1);
        assert_eq!(top_row(&f), "PS C:\\> cmd57");
        // Ganz nach oben: ältester noch vorhandener Prompt, dann der Anfang des Verlaufs.
        for _ in 0..20 {
            jump_mark_in(&mut f.term, f.hs.base(), &marks, -1);
        }
        assert_eq!(f.term.grid().display_offset(), 30);
    }

    #[test]
    fn without_mark_wrapped_prompt_still_skipped() {
        // cmd meldet keinen Prompt-Anfang: Ende über die Cursorzeile; ein umbrochener langer Prompt fällt ganz weg.
        let mut f = Fake::new(COLS, 8);
        let prompt = format!("C:\\{}>", "k".repeat(COLS + 5));
        f.feed(&prompt);
        let start_abs = f.cursor();
        f.feed("echo hallo\r\nhallo\r\n\r\n");
        f.feed(&prompt);
        let r = run_output(&f.term, f.base(), "echo hallo", start_abs, None, None, true);
        assert_eq!(text(&r), "hallo");
    }
}

#[cfg(test)]
mod osc52_tests {
    use super::*;

    /// Schreibt in einen geteilten Puffer: so lässt sich prüfen, dass nichts an das Programm zurückgeht.
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Bytes durch die echte Emulation (mit der Konfiguration der Sitzungen) schicken.
    fn feed(bytes: &[u8]) -> (ClipboardRequest, Vec<u8>) {
        let out = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener::new(Arc::new(Mutex::new(Box::new(Sink(out.clone())))));
        let clipboard = listener.clipboard.clone();
        let mut term = Term::new(term_config(100), &TermSize::new(80, 24), listener);
        let mut p: Processor = Processor::new();
        p.advance(&mut term, bytes);
        let got = std::mem::take(&mut *clipboard.lock().unwrap());
        let written = out.lock().unwrap().clone();
        (got, written)
    }

    fn text(t: &str, count: u32) -> ClipboardRequest {
        ClipboardRequest { text: Some(t.into()), too_large: None, count }
    }

    fn b64(s: &[u8]) -> String {
        crate::mcp::base64(s)
    }

    #[test]
    fn osc52_copy_reaches_listener() {
        // BEL als Abschluss (Claude Code, tmux) und ST (ESC \).
        let (got, written) = feed(format!("\x1b]52;c;{}\x07", b64(b"hello")).as_bytes());
        assert_eq!(got, text("hello", 1));
        assert!(written.is_empty());
        let (got, _) = feed(format!("vorher\x1b]52;c;{}\x1b\\nachher", b64("Grüße ✓\nzweite Zeile".as_bytes())).as_bytes());
        assert_eq!(got, text("Grüße ✓\nzweite Zeile", 1));
    }

    #[test]
    fn osc52_selection_targets_also_copy() {
        // „p“/„s“ (primäre Auswahl) – Windows hat nur eine Zwischenablage.
        for target in ["p", "s"] {
            let (got, _) = feed(format!("\x1b]52;{target};{}\x07", b64(b"sel")).as_bytes());
            assert_eq!(got, text("sel", 1), "Ziel {target}");
        }
        // Der letzte von mehreren Wünschen gewinnt, gezählt werden alle.
        let (got, _) = feed(format!("\x1b]52;c;{}\x07\x1b]52;c;{}\x07", b64(b"eins"), b64(b"zwei")).as_bytes());
        assert_eq!(got, text("zwei", 2));
    }

    #[test]
    fn osc52_read_request_is_never_answered() {
        // Lesewunsch: keine Antwort an das Programm, kein Kopierwunsch.
        let (got, written) = feed(b"\x1b]52;c;?\x07");
        assert!(got.is_empty());
        assert!(written.is_empty(), "Antwort an das Programm: {:?}", String::from_utf8_lossy(&written));
        assert_eq!(term_config(10).osc52, Osc52::OnlyCopy);
    }

    #[test]
    fn osc52_empty_payload_is_ignored() {
        // Leerer Inhalt würde die Zwischenablage leeren: kein Wunsch, kein Zähler.
        let (got, _) = feed(b"\x1b]52;c;\x07");
        assert_eq!(got, ClipboardRequest::default());
        let (got, _) = feed(format!("\x1b]52;c;{}\x07\x1b]52;c;\x07", b64(b"bleibt")).as_bytes());
        assert_eq!(got, text("bleibt", 1));
    }

    #[test]
    fn osc52_size_limit_keeps_last_valid_text() {
        let mut r = ClipboardRequest::default();
        r.add("x".repeat(OSC52_MAX_BYTES));
        assert_eq!(r.text.as_deref().map(str::len), Some(OSC52_MAX_BYTES));
        assert_eq!(r.too_large, None);
        // Gültig, dann zu groß im selben Durchgang: der gültige Text bleibt, dazu die Warnung.
        let big = vec![b'a'; OSC52_MAX_BYTES + 3];
        let (got, _) = feed(format!("\x1b]52;c;{}\x07\x1b]52;c;{}\x07", b64(b"klein"), b64(&big)).as_bytes());
        assert_eq!(got, ClipboardRequest { text: Some("klein".into()), too_large: Some(OSC52_MAX_BYTES + 3), count: 2 });
        assert!(!got.is_empty());
    }
}

#[cfg(all(test, windows))]
mod win_env_tests {
    use portable_pty::CommandBuilder;

    /// Die Standard-Umgebung eines Terminals (portable-pty frischt sie aus der Registry auf) muss den angemeldeten
    /// Benutzer behalten: HKLM\...\Session Manager\Environment enthält `USERNAME=SYSTEM`, das wie bei
    /// CreateEnvironmentBlock nicht übernommen werden darf. Auch die Benutzer-Variablen der Anmeldung bleiben.
    #[test]
    fn registry_refresh_keeps_the_current_user() {
        let cmd = CommandBuilder::new("cmd.exe");
        for k in ["USERNAME", "USERDOMAIN", "USERPROFILE"] {
            if let Some(want) = std::env::var_os(k) {
                assert_eq!(cmd.get_env(k), Some(want.as_os_str()), "{k} aus der Prozessumgebung, nicht aus HKLM");
            }
        }
        assert_ne!(cmd.get_env("USERNAME").map(|v| v.to_ascii_uppercase()), Some("SYSTEM".into()));
    }
}

#[cfg(all(test, windows))]
mod win_job_tests {
    use super::*;
    use crate::shells::ShellSpec;

    #[test]
    fn a_windows_session_starts_inside_its_job_and_reports_background_processes() {
        let ctx = egui::Context::default();
        let shell = ShellSpec { id: "cmd".into(), label: "cmd".into(), program: "cmd.exe".into(), args: vec!["/q".into()] };
        let s = Session::spawn(&ctx, SpawnOptions { shell: &shell, cwd: None, restore_text: None, initial_input: None, scrollback_lines: 100, env: Vec::new(), inherit_env: false }).expect("Sitzung startet");
        assert!(s.job_key().is_some(), "Sitzung hat einen Job");
        assert!(s.lingering(&crate::winctl::ProcInfo::default()).is_empty(), "nur die Shell: nichts");
        let pid = s.shell_pid.expect("Shell-PID");
        assert!(s.job.as_ref().unwrap().pids().contains(&pid), "Shell im Job");
        assert!(s.end_job());
    }
}
