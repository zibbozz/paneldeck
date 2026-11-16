//! Ein Terminal: PTY (ConPTY unter Windows) + VT-Emulation (alacritty_terminal).

use crate::shells::ShellSpec;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{test::TermSize, Config, Osc52, Term};
use alacritty_terminal::vte::ansi::Processor;
use eframe::egui;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use crate::osc::{OscEvent, OscScanner};
use crate::search::SearchState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Höchstgröße eines Textes, den ein Programm per OSC 52 in die Zwischenablage legen darf (#181).
pub const OSC52_MAX_BYTES: usize = 1024 * 1024;

/// Kopierwunsch eines Programms per OSC 52, abgeholt vom UI-Thread (`Session::take_clipboard`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardRequest {
    /// Text für die Zwischenablage.
    Text(String),
    /// Verworfen, weil größer als [`OSC52_MAX_BYTES`] (Größe in Bytes).
    TooLarge(usize),
}

#[derive(Clone)]
pub struct Listener {
    title: Arc<Mutex<Option<String>>>,
    writer: SharedWriter,
    bell: Arc<AtomicBool>,
    /// Letzter Kopierwunsch (OSC 52); ein neuer ersetzt einen noch nicht abgeholten.
    clipboard: Arc<Mutex<Option<ClipboardRequest>>>,
}

/// Was der Emulator zu einem OSC-52-Schreibwunsch meldet, in einen Kopierwunsch übersetzen (Größengrenze).
pub fn clipboard_request(text: String) -> ClipboardRequest {
    if text.len() > OSC52_MAX_BYTES {
        ClipboardRequest::TooLarge(text.len())
    } else {
        ClipboardRequest::Text(text)
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
            Event::ClipboardStore(_, text) => {
                *self.clipboard.lock().unwrap_or_else(|e| e.into_inner()) = Some(clipboard_request(text));
            }
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
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    title: Arc<Mutex<Option<String>>>,
    exited: Arc<AtomicBool>,
    #[allow(dead_code)] // wird gesetzt (Event::Bell); eine Anzeige dafür ist noch nicht eingebaut
    pub bell: Arc<AtomicBool>,
    clipboard: Arc<Mutex<Option<ClipboardRequest>>>,
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
    pub env: Vec<(String, String)>,
}

impl Session {
    pub fn spawn(ctx: &egui::Context, opts: SpawnOptions) -> Result<Session, String> {
        let (cols, rows) = (100u16, 30u16);
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;

        let mut cmd = CommandBuilder::new(&opts.shell.program);
        for a in &opts.shell.args {
            cmd.arg(a);
        }
        if let Some(cwd) = opts.cwd {
            if std::path::Path::new(cwd).is_dir() {
                cmd.cwd(cwd);
            }
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: SharedWriter = Arc::new(Mutex::new(
            pair.master.take_writer().map_err(|e| e.to_string())?,
        ));

        let title = Arc::new(Mutex::new(None));
        let bell = Arc::new(AtomicBool::new(false));
        let clipboard = Arc::new(Mutex::new(None));
        let listener = Listener { title: title.clone(), writer: writer.clone(), bell: bell.clone(), clipboard: clipboard.clone() };

        let config = term_config(opts.scrollback_lines);
        let mut term = Term::new(config, &TermSize::new(cols as usize, rows as usize), listener);
        let mut processor: Processor = Processor::new();

        if let Some(text) = opts.restore_text {
            let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
            processor.advance(&mut term, text.as_bytes());
            let sep = format!("\r\n\x1b[2m\u{2500}\u{2500} {} \u{2500}\u{2500}\x1b[0m\r\n", crate::state::t("wiederhergestellter Verlauf", "restored history"));
            processor.advance(&mut term, sep.as_bytes());
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
                                    processor.stop_sync(&mut *t);
                                }
                                if events.is_empty() {
                                    processor.advance(&mut *t, &bytes);
                                } else {
                                    // Den Chunk an den Ereignissen teilen: die Cursorzeile (z. B. Prompt-Anfang) wird dort ausgelesen,
                                    // wo die Sequenz steht – nicht erst am Chunk-Ende, wo ein mehrzeiliger Prompt schon fertig gedruckt ist.
                                    let mut done = 0usize;
                                    for (ev, end) in events.into_iter().zip(positions) {
                                        let end = end.clamp(done, bytes.len());
                                        processor.advance(&mut *t, &bytes[done..end]);
                                        done = end;
                                        let hist = t.grid().total_lines().saturating_sub(t.grid().screen_lines()) as i64;
                                        let line = t.grid().cursor.point.line.0 as i64;
                                        let was = stale.load(Ordering::Relaxed);
                                        stale.store(next_stale_modes(was, &ev, *t.mode()), Ordering::Relaxed);
                                        if ev == OscEvent::FullscreenModeSet {
                                            continue; // keine Shell-Integration
                                        }
                                        apply_events(&shell_state, vec![ev], (hist + line).max(0) as u64);
                                    }
                                    processor.advance(&mut *t, &bytes[done..]);
                                }
                            }
                            ctx.request_repaint();
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            processor.stop_sync(&mut *term.lock().unwrap_or_else(|e| e.into_inner()));
                            ctx.request_repaint();
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                // Restpuffer eines offenen Updates noch anzeigen.
                if processor.sync_timeout().sync_timeout().is_some() {
                    processor.stop_sync(&mut *term.lock().unwrap_or_else(|e| e.into_inner()));
                }
                exited.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }

        let session = Session {
            term,
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
        };
        if let Some(input) = opts.initial_input {
            session.write(input.as_bytes());
        }
        Ok(session)
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

    /// Wie `lines_abs`, aber umgebrochene Zeilen sind zu einer logischen Zeile zusammengefügt (für Ausgaben an Leser).
    pub fn text_abs(&self, from: u64, to: u64) -> Vec<String> {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let g = t.grid();
        let hist = g.total_lines().saturating_sub(g.screen_lines()) as i64;
        let top = -hist;
        let bottom = g.bottommost_line().0 as i64;
        let a = (from as i64 - hist).clamp(top, bottom);
        let b = (to as i64 - hist).clamp(top, bottom);
        if b < a {
            return vec![];
        }
        let text = t.bounds_to_string(Point::new(Line(a as i32), Column(0)), Point::new(Line(b as i32), Column(g.columns() - 1)));
        text.lines().map(|l| l.trim_end().to_string()).collect()
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
        let hist = t.grid().total_lines().saturating_sub(t.grid().screen_lines()) as i64;
        (hist + t.grid().cursor.point.line.0 as i64).max(0) as u64
    }

    /// Beginn der (ggf. umgebrochenen) Zeile, zu der `abs` gehört: läuft eine Zeile am Rand in die nächste über,
    /// gehört sie zur selben logischen Zeile (lange Prompts in schmalen Terminals).
    pub fn unwrap_start(&self, mut abs: u64) -> u64 {
        use alacritty_terminal::term::cell::Flags;
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let g = t.grid();
        let hist = g.total_lines().saturating_sub(g.screen_lines()) as i64;
        let top = -hist;
        let last_col = Column(g.columns() - 1);
        while abs > 0 {
            let prev = abs as i64 - 1 - hist;
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
    pub fn wrap_end(&self, mut abs: u64) -> u64 {
        use alacritty_terminal::term::cell::Flags;
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let g = t.grid();
        let hist = g.total_lines().saturating_sub(g.screen_lines()) as i64;
        let bottom = g.bottommost_line().0 as i64;
        let last_col = Column(g.columns() - 1);
        loop {
            let row = abs as i64 - hist;
            if row < -hist || row >= bottom || !g[Line(row as i32)][last_col].flags.contains(Flags::WRAPLINE) {
                return abs;
            }
            abs += 1;
        }
    }

    /// Terminal-Zeilen von `from` bis `to` (absolut, einschließlich), je Zeile ein Eintrag, ohne Leerraum am Zeilenende.
    pub fn lines_abs(&self, from: u64, to: u64) -> Vec<String> {
        let t = self.term.lock().unwrap_or_else(|e| e.into_inner());
        let g = t.grid();
        let hist = g.total_lines().saturating_sub(g.screen_lines()) as i64;
        let top = -(hist);
        let bottom = g.bottommost_line().0 as i64;
        let a = (from as i64 - hist).clamp(top, bottom);
        let b = (to as i64 - hist).clamp(top, bottom);
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
        self.clipboard.lock().unwrap_or_else(|e| e.into_inner()).take()
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
        let hist = t.grid().total_lines().saturating_sub(t.grid().screen_lines()) as i64;
        let top = hist - t.grid().display_offset() as i64;
        let target = if delta < 0 {
            marks.iter().rev().find(|m| (**m as i64) < top).copied()
        } else {
            marks.iter().find(|m| (**m as i64) > top).copied()
        };
        let offset = match target {
            Some(m) => (hist - m as i64).clamp(0, hist),
            None if delta > 0 => 0,
            None => hist,
        };
        let cur = t.grid().display_offset() as i64;
        t.scroll_display(alacritty_terminal::grid::Scroll::Delta((offset - cur) as i32));
    }
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
            SpawnOptions { shell: &shell, cwd: None, restore_text: None, initial_input: None, scrollback_lines: 100, env: vec![] },
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
    fn feed(bytes: &[u8]) -> (Option<ClipboardRequest>, Vec<u8>) {
        let out = Arc::new(Mutex::new(Vec::new()));
        let clipboard = Arc::new(Mutex::new(None));
        let listener = Listener {
            title: Arc::new(Mutex::new(None)),
            writer: Arc::new(Mutex::new(Box::new(Sink(out.clone())))),
            bell: Arc::new(AtomicBool::new(false)),
            clipboard: clipboard.clone(),
        };
        let mut term = Term::new(term_config(100), &TermSize::new(80, 24), listener);
        let mut p: Processor = Processor::new();
        p.advance(&mut term, bytes);
        let got = clipboard.lock().unwrap().take();
        let written = out.lock().unwrap().clone();
        (got, written)
    }

    fn b64(s: &[u8]) -> String {
        crate::mcp::base64(s)
    }

    #[test]
    fn osc52_copy_reaches_listener() {
        // BEL als Abschluss (Claude Code, tmux) und ST (ESC \).
        let (got, written) = feed(format!("\x1b]52;c;{}\x07", b64(b"hello")).as_bytes());
        assert_eq!(got, Some(ClipboardRequest::Text("hello".into())));
        assert!(written.is_empty());
        let (got, _) = feed(format!("vorher\x1b]52;c;{}\x1b\nachher", b64("Grüße ✓\nzweite Zeile".as_bytes())).as_bytes());
        assert_eq!(got, Some(ClipboardRequest::Text("Grüße ✓\nzweite Zeile".into())));
    }

    #[test]
    fn osc52_selection_targets_also_copy() {
        // „p“/„s“ (primäre Auswahl) – Windows hat nur eine Zwischenablage.
        for target in ["p", "s"] {
            let (got, _) = feed(format!("\x1b]52;{target};{}\x07", b64(b"sel")).as_bytes());
            assert_eq!(got, Some(ClipboardRequest::Text("sel".into())), "Ziel {target}");
        }
        // Der letzte von mehreren Wünschen gewinnt.
        let (got, _) = feed(format!("\x1b]52;c;{}\x07\x1b]52;c;{}\x07", b64(b"eins"), b64(b"zwei")).as_bytes());
        assert_eq!(got, Some(ClipboardRequest::Text("zwei".into())));
    }

    #[test]
    fn osc52_read_request_is_never_answered() {
        // Lesewunsch: keine Antwort an das Programm, kein Kopierwunsch.
        let (got, written) = feed(b"\x1b]52;c;?\x07");
        assert_eq!(got, None);
        assert!(written.is_empty(), "Antwort an das Programm: {:?}", String::from_utf8_lossy(&written));
        assert_eq!(term_config(10).osc52, Osc52::OnlyCopy);
    }

    #[test]
    fn osc52_size_limit() {
        assert_eq!(clipboard_request("x".repeat(OSC52_MAX_BYTES)), ClipboardRequest::Text("x".repeat(OSC52_MAX_BYTES)));
        assert_eq!(clipboard_request("x".repeat(OSC52_MAX_BYTES + 1)), ClipboardRequest::TooLarge(OSC52_MAX_BYTES + 1));
        let big = vec![b'a'; OSC52_MAX_BYTES + 3];
        let (got, _) = feed(format!("\x1b]52;c;{}\x07", b64(&big)).as_bytes());
        assert_eq!(got, Some(ClipboardRequest::TooLarge(OSC52_MAX_BYTES + 3)));
    }
}
