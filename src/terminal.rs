//! Ein Terminal: PTY (ConPTY unter Windows) + VT-Emulation (alacritty_terminal).

use crate::shells::ShellSpec;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::color::{Colors, COUNT as COLOR_COUNT};
use alacritty_terminal::term::{test::TermSize, Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{Processor, Rgb};
use eframe::egui;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use crate::osc::{OscEvent, OscScanner, Query};
use crate::search::SearchState;
use crate::theme::Theme;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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

/// Höchstlänge (Zeichen) eines Titels, den ein Programm per OSC 0/2 setzt (#183).
pub const TITLE_MAX_CHARS: usize = 256;

/// Formatiert die Antwort auf eine Farbabfrage (von alacritty, enthält Präfix und Abschluss der Anfrage).
pub type ColorFmt = Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>;
/// Formatiert die Antwort auf `CSI 14 t` (von alacritty).
pub type SizeFmt = Arc<dyn Fn(WindowSize) -> String + Sync + Send + 'static>;

/// Antwort an das Programm. Alle Antworten (auch die der Emulation selbst, z. B. DA1 oder die Cursorposition) werden in
/// der Reihenfolge der Anfragen gesammelt und nach jedem verarbeiteten Ausgabe-Stück zusammen geschrieben
/// ([`Listener::take_reply_bytes`]): Farbabfragen brauchen die von Programmen gesetzten Farben (`Term::colors`), die
/// erst nach dem Verarbeiten ohne Sperrkonflikt lesbar sind, und Programme wie Neovim oder fish schicken DA1 als
/// „Wächter“ hinter OSC 11 – kommt dessen Antwort zuerst, gilt die Farbabfrage als nicht unterstützt.
#[derive(Clone)]
pub enum Reply {
    /// Fertiger Text (`Event::PtyWrite`).
    Text(String),
    /// OSC 4/10/11/12 `?` für den Farbindex (0–255 Palette, 256 Vordergrund, 257 Hintergrund, 258 Cursor).
    Color(usize, ColorFmt),
    /// `CSI 14 t`: Größe des Textbereichs in Pixeln.
    TextAreaPx(SizeFmt),
    /// Anfragen, die die Emulation nicht kennt (`CSI 16 t`, XTVERSION), erkannt in `osc::OscScanner`.
    Query(Query),
}

/// Was der PTY-Thread zum Beantworten braucht und der UI-Thread je Frame pflegt (`Session::set_view`):
/// Theme der Sitzung und Zellgröße in physischen Pixeln (0 = noch nie gezeichnet).
#[derive(Clone, PartialEq)]
pub struct ReplyView {
    pub theme: Theme,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Default for ReplyView {
    fn default() -> Self {
        ReplyView { theme: crate::theme::all().swap_remove(0), cell_w: 0, cell_h: 0 }
    }
}

/// Antworten in Bytes umsetzen (rein, testbar): Farben aus den von Programmen gesetzten Werten (`colors`), sonst aus
/// dem Theme; Größen aus `cols`×`rows` Zellen der Größe `view.cell_w`×`view.cell_h`.
pub fn render_replies(replies: Vec<Reply>, colors: &Colors, view: &ReplyView, cols: usize, rows: usize) -> Vec<u8> {
    let mut out = String::new();
    for r in replies {
        match r {
            Reply::Text(s) => out.push_str(&s),
            Reply::Color(i, f) => {
                let set = if i < COLOR_COUNT { colors[i] } else { None };
                let rgb = set.unwrap_or_else(|| {
                    let c = view.theme.index_color(i);
                    Rgb { r: c.r(), g: c.g(), b: c.b() }
                });
                out.push_str(&f(rgb));
            }
            Reply::TextAreaPx(f) => out.push_str(&f(WindowSize {
                num_lines: rows.min(u16::MAX as usize) as u16,
                num_cols: cols.min(u16::MAX as usize) as u16,
                cell_width: view.cell_w,
                cell_height: view.cell_h,
            })),
            Reply::Query(Query::CellSizePx) => out.push_str(&format!("\x1b[6;{};{}t", view.cell_h, view.cell_w)),
            Reply::Query(Query::XtVersion) => out.push_str(&format!("\x1bP>|Paneldeck {}\x1b\\", env!("CARGO_PKG_VERSION"))),
        }
    }
    out.into_bytes()
}

/// Titel eines Programms (OSC 0/2) bereinigen, bevor er in der Seitenleiste, im Fenstertitel oder über MCP erscheint:
/// ohne Steuerzeichen (C0, DEL, C1) und ohne Zeichen zur Schreibrichtung (Überschreiben/Isolieren U+202A–202E,
/// U+2066–2069 sowie die Marken U+200E/U+200F/U+061C), die die Anzeige umdrehen und einen anderen Titel vortäuschen
/// könnten; höchstens [`TITLE_MAX_CHARS`] Zeichen.
pub fn sanitize_title(t: &str) -> String {
    t.chars()
        .filter(|&c| !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '\u{061c}'))
        .take(TITLE_MAX_CHARS)
        .collect()
}

/// Eingefügten Text für das Programm aufbereiten: Zeilenumbrüche als `\r`, C1-Steuerzeichen (U+0080–U+009F) immer
/// entfernt (manche Programme deuten sie als Escape-Sequenzen). Mit Bracketed Paste zusätzlich alle anderen
/// Steuerzeichen (v. a. ESC: ein eingefügtes `ESC[201~` beendete sonst den Einfüge-Modus vorzeitig und der Rest liefe
/// als getippte Befehle) außer Tab und Zeilenumbruch, eingerahmt in `ESC[200~` … `ESC[201~`.
pub fn paste_payload(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    let c1 = |c: char| ('\u{80}'..='\u{9f}').contains(&c);
    if bracketed {
        let text: String = text.chars().filter(|&c| c >= ' ' && c != '\u{7f}' && !c1(c) || c == '\t' || c == '\r').collect();
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text.chars().filter(|&c| !c1(c)).collect()
    }
}

/// Zeilen eines eingefügten Textes, wenn er Zeilenumbrüche enthält (sonst `None`): Grundlage der Rückfrage beim
/// Einfügen mehrerer Zeilen ohne Bracketed Paste – jeder Umbruch kann dann einen Befehl ausführen.
pub fn paste_line_count(text: &str) -> Option<usize> {
    if !text.contains(['\r', '\n']) {
        return None;
    }
    Some(text.replace("\r\n", "\n").replace('\r', "\n").lines().count().max(1))
}

/// Fokusmeldung (DECSET 1004) beim Wechsel des Fokus: `CSI I` (bekommt Fokus) bzw. `CSI O` (verliert ihn), nur wenn
/// sich der Zustand ändert (`prev` = zuletzt gesehener, `None` = noch keiner) und das Programm sie angefordert hat.
pub fn focus_report(prev: Option<bool>, now: bool, mode: TermMode) -> Option<&'static [u8]> {
    if prev == Some(now) || !mode.contains(TermMode::FOCUS_IN_OUT) {
        return None;
    }
    Some(if now { b"\x1b[I" } else { b"\x1b[O" })
}

/// Umgebung jeder Shell: Terminaltyp, Farbtiefe und Name/Version des Terminals (`TERM_PROGRAM`, wie Windows Terminal,
/// iTerm2 oder WezTerm; von Programmen wie Claude Code ausgewertet).
pub fn term_env() -> Vec<(&'static str, String)> {
    vec![
        ("TERM", "xterm-256color".to_string()),
        ("COLORTERM", "truecolor".to_string()),
        ("TERM_PROGRAM", "Paneldeck".to_string()),
        ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION").to_string()),
    ]
}

#[derive(Clone)]
pub struct Listener {
    title: Arc<Mutex<Option<String>>>,
    writer: SharedWriter,
    bell: Arc<AtomicBool>,
    /// Kopierwünsche (OSC 52) seit dem letzten Abholen.
    clipboard: Arc<Mutex<ClipboardRequest>>,
    /// Noch nicht geschriebene Antworten an das Programm, in Reihenfolge (siehe [`Reply`]).
    replies: Arc<Mutex<Vec<Reply>>>,
}

impl Listener {
    /// Listener mit Standardwerten; `Session::spawn` übernimmt die geteilten Felder per `clone`.
    pub fn new(writer: SharedWriter) -> Self {
        Listener { title: Default::default(), writer, bell: Default::default(), clipboard: Default::default(), replies: Default::default() }
    }

    /// Ereignis der Emulation verarbeiten (Inhalt von `send_event`). Läuft im PTY-Thread, während er die Emulation
    /// gesperrt hält – deshalb wird hier nichts geschrieben, nur gesammelt.
    fn on_event(&self, event: Event) {
        match event {
            Event::Title(t) => *self.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(sanitize_title(&t)),
            Event::ResetTitle => *self.title.lock().unwrap_or_else(|e| e.into_inner()) = None,
            // Antworten der Emulation (z.B. Cursorposition) zurück an die Shell – in Reihenfolge gesammelt.
            Event::PtyWrite(s) => self.push_reply(Reply::Text(s)),
            Event::ColorRequest(i, f) => self.push_reply(Reply::Color(i, f)),
            Event::TextAreaSizeRequest(f) => self.push_reply(Reply::TextAreaPx(f)),
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

    pub fn push_reply(&self, r: Reply) {
        let mut q = self.replies.lock().unwrap_or_else(|e| e.into_inner());
        // Ein Programm, das in einer Schleife fragt, soll den Speicher nicht füllen (nach jedem Stück wird geleert).
        if q.len() < 4096 {
            q.push(r);
        }
    }

    /// Gesammelte Antworten abholen und in Bytes umsetzen (`term` = die gesperrte Emulation dieses Listeners).
    pub fn take_reply_bytes(&self, term: &Term<Listener>, view: &Mutex<ReplyView>) -> Vec<u8> {
        let replies = std::mem::take(&mut *self.replies.lock().unwrap_or_else(|e| e.into_inner()));
        if replies.is_empty() {
            return Vec::new();
        }
        let view = view.lock().unwrap_or_else(|e| e.into_inner()).clone();
        render_replies(replies, term.colors(), &view, term.columns(), term.screen_lines())
    }

    /// Antworten an das Programm schreiben (ohne die Emulation gesperrt zu halten).
    pub fn write_out(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Gesammelte Antworten verwerfen (z. B. nach dem Wiederherstellen des alten Verlaufs: dessen Anfragen dürfen
    /// nicht an die neue Shell gehen).
    pub fn drop_replies(&self) {
        self.replies.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Emulator-Einstellungen eines Terminals. OSC 52 nur zum Kopieren: Programme dürfen die Zwischenablage
/// beschreiben, aber nie lesen (`ClipboardLoad` wird gar nicht erst ausgelöst).
pub fn term_config(scrollback_lines: usize) -> Config {
    Config { scrolling_history: scrollback_lines, osc52: Osc52::OnlyCopy, ..Config::default() }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.on_event(event);
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
    /// Theme und Zellgröße für Antworten an das Programm (Farb- und Größenabfragen), vom UI-Thread gepflegt.
    view: Arc<Mutex<ReplyView>>,
    /// Zuletzt gemeldeter Fokus (DECSET 1004): 0 = noch keiner, 1 = ohne Fokus, 2 = mit Fokus.
    focus_seen: AtomicU8,
    /// Vor dem Einfügen mehrerer Zeilen ohne Bracketed Paste nachfragen (Einstellung, vom UI-Thread gesetzt).
    paste_warn: AtomicBool,
    /// Eingefügter Text, der auf die Rückfrage wartet (`take_paste_ask`).
    paste_ask: Mutex<Option<String>>,
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
    pub env: Vec<(String, String)>,
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
        for (k, v) in term_env() {
            cmd.env(k, v);
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: SharedWriter = Arc::new(Mutex::new(
            pair.master.take_writer().map_err(|e| e.to_string())?,
        ));

        let listener = Listener::new(writer.clone());
        let (title, bell, clipboard) = (listener.title.clone(), listener.bell.clone(), listener.clipboard.clone());
        let lst = listener.clone();
        let view: Arc<Mutex<ReplyView>> = Default::default();

        let config = term_config(opts.scrollback_lines);
        let mut term = Term::new(config, &TermSize::new(cols as usize, rows as usize), listener);
        let mut processor: Processor = Processor::new();

        if let Some(text) = opts.restore_text {
            let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
            processor.advance(&mut term, text.as_bytes());
            let sep = format!("\r\n\x1b[2m\u{2500}\u{2500} {} \u{2500}\u{2500}\x1b[0m\r\n", crate::state::t("wiederhergestellter Verlauf", "restored history"));
            processor.advance(&mut term, sep.as_bytes());
            // Anfragen im alten Verlauf beantwortet niemand: die neue Shell hat sie nicht gestellt.
            lst.drop_replies();
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
            let view = view.clone();
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
                            let replies = {
                                let mut t = term.lock().unwrap_or_else(|e| e.into_inner());
                                if processor.sync_timeout().sync_timeout().is_some_and(|d| d <= std::time::Instant::now()) {
                                    processor.stop_sync(&mut *t);
                                }
                                process_chunk(&mut processor, &mut t, &bytes, (events, positions), &lst, &shell_state, &stale);
                                lst.take_reply_bytes(&t, &view)
                            };
                            lst.write_out(&replies);
                            ctx.request_repaint();
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            let replies = {
                                let mut t = term.lock().unwrap_or_else(|e| e.into_inner());
                                processor.stop_sync(&mut *t);
                                lst.take_reply_bytes(&t, &view)
                            };
                            lst.write_out(&replies);
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
            view,
            focus_seen: AtomicU8::new(0),
            paste_warn: AtomicBool::new(true),
            paste_ask: Mutex::new(None),
            console_host,
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
        let bracketed = {
            let mut t = self.term.lock().unwrap_or_else(|e| e.into_inner());
            t.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
            t.mode().contains(TermMode::BRACKETED_PASTE)
        };
        let mut out: Vec<u8> = paste_payload(text, bracketed).into_bytes();
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
        let bracketed = self.bracketed_paste();
        // Steuerzeichen filtern (siehe `paste_payload`).
        self.write(paste_payload(text, bracketed).as_bytes());
        if submit {
            self.write(b"\r");
        }
    }

    /// Das Programm hat Bracketed Paste (DECSET 2004) eingeschaltet.
    pub fn bracketed_paste(&self) -> bool {
        self.term.lock().unwrap_or_else(|e| e.into_inner()).mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Einfügen durch den Nutzer (Strg+V, Rechtsklick): mehrere Zeilen ohne Bracketed Paste warten auf die Rückfrage
    /// (Einstellung „Vor dem Einfügen mehrerer Zeilen nachfragen“, `take_paste_ask`), alles andere geht sofort.
    pub fn paste_user(&self, text: &str) {
        if self.paste_warn.load(Ordering::Relaxed) && paste_line_count(text).is_some() && !self.bracketed_paste() {
            *self.paste_ask.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.to_string());
            return;
        }
        self.paste_now(text);
    }

    /// Text einfügen: ans Ende scrollen, Auswahl aufheben, senden.
    pub fn paste_now(&self, text: &str) {
        {
            let mut t = self.term.lock().unwrap_or_else(|e| e.into_inner());
            if t.grid().display_offset() > 0 {
                t.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
            }
            t.selection = None;
        }
        self.send_text(text, false);
    }

    /// Text, der auf die Rückfrage vor dem Einfügen wartet (einmal abholen).
    pub fn take_paste_ask(&self) -> Option<String> {
        self.paste_ask.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Einstellung „Vor dem Einfügen mehrerer Zeilen nachfragen“ übernehmen (UI-Thread, jedes Frame).
    pub fn set_paste_warn(&self, on: bool) {
        self.paste_warn.store(on, Ordering::Relaxed);
    }

    /// Fokus der Sitzung (Bereich fokussiert und Fenster im Vordergrund) melden, jedes Frame: Beim Wechsel bekommt
    /// ein Programm, das Fokusmeldungen angefordert hat (DECSET 1004), `CSI I` bzw. `CSI O`.
    pub fn report_focus(&self, focused: bool) {
        let now = if focused { 2 } else { 1 };
        let prev = match self.focus_seen.swap(now, Ordering::Relaxed) {
            0 => None,
            p => Some(p == 2),
        };
        if prev == Some(focused) {
            return;
        }
        let mode = *self.term.lock().unwrap_or_else(|e| e.into_inner()).mode();
        if let Some(seq) = focus_report(prev, focused, mode) {
            // Ohne Broadcast-Mitschnitt und ohne `stale_modes` anzufassen.
            let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            let _ = w.write_all(seq);
            let _ = w.flush();
        }
    }

    /// Theme und Zellgröße (physische Pixel) für Antworten an das Programm übernehmen (UI-Thread, beim Zeichnen).
    pub fn set_view(&self, theme: &Theme, cell_w: u16, cell_h: u16) {
        let mut v = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if v.theme != *theme {
            v.theme = theme.clone();
        }
        if (v.cell_w, v.cell_h) != (cell_w, cell_h) {
            v.cell_w = cell_w;
            v.cell_h = cell_h;
            drop(v);
            // Unter Windows wertet ConPTY die Pixelgröße nicht aus; ein Größenwechsel ohne neue Zeilen/Spalten löste
            // dort nur ein Neuzeichnen aus. Sonst (Unix) bekommt das Programm die neue Pixelgröße (TIOCSWINSZ).
            #[cfg(not(windows))]
            {
                let _ = self.master.resize(self.pty_size());
            }
        }
    }

    /// Größe für das Pseudo-Terminal: Zeilen/Spalten und die Pixelgröße des Textbereichs (0, solange unbekannt).
    fn pty_size(&self) -> PtySize {
        let v = self.view.lock().unwrap_or_else(|e| e.into_inner());
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.cols.saturating_mul(v.cell_w),
            pixel_height: self.rows.saturating_mul(v.cell_h),
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
        let _ = self.master.resize(self.pty_size());
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

/// Ein Stück Programmausgabe durch die Emulation schicken (PTY-Thread, Emulation gesperrt). `scanned` = Ereignisse und
/// ihre Positionen aus `OscScanner::feed_at` für genau diese Bytes. Antworten landen in der Reihe des Listeners
/// (`lst`), Meldungen der Shell-Integration in `shell_state`.
fn process_chunk(
    processor: &mut Processor,
    t: &mut Term<Listener>,
    bytes: &[u8],
    scanned: (Vec<OscEvent>, Vec<usize>),
    lst: &Listener,
    shell_state: &Arc<Mutex<ShellState>>,
    stale: &AtomicBool,
) {
    let (events, positions) = scanned;
    if events.is_empty() {
        processor.advance(t, bytes);
        return;
    }
    // Den Chunk an den Ereignissen teilen: die Cursorzeile (z. B. Prompt-Anfang) wird dort ausgelesen,
    // wo die Sequenz steht – nicht erst am Chunk-Ende, wo ein mehrzeiliger Prompt schon fertig gedruckt ist.
    let mut done = 0usize;
    for (ev, end) in events.into_iter().zip(positions) {
        let end = end.clamp(done, bytes.len());
        processor.advance(t, &bytes[done..end]);
        done = end;
        if let OscEvent::Query(q) = ev {
            // In die Reihe der Antworten: nach allem, was davor im Strom stand.
            lst.push_reply(Reply::Query(q));
            continue;
        }
        let hist = t.grid().total_lines().saturating_sub(t.grid().screen_lines()) as i64;
        let line = t.grid().cursor.point.line.0 as i64;
        let was = stale.load(Ordering::Relaxed);
        stale.store(next_stale_modes(was, &ev, *t.mode()), Ordering::Relaxed);
        if !ev.is_integration() {
            continue; // keine Shell-Integration
        }
        apply_events(shell_state, vec![ev], (hist + line).max(0) as u64);
    }
    processor.advance(t, &bytes[done..]);
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
        st.integration_seen |= ev.is_integration();
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
            // Kommen nicht von der Shell-Integration; der Lese-Thread wertet sie selbst aus (`next_stale_modes`, Antworten).
            OscEvent::FullscreenModeSet | OscEvent::Query(_) => {}
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

#[cfg(test)]
mod vt_reply_tests {
    use super::*;
    use crate::theme::Theme;
    use eframe::egui::Color32;

    /// Schreibt in einen geteilten Puffer (wie das Pseudo-Terminal).
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

    /// Eine Sitzung ohne Prozess: Emulation, Listener und Scanner wie im PTY-Thread.
    struct Rig {
        term: Term<Listener>,
        lst: Listener,
        p: Processor,
        scanner: OscScanner,
        view: Mutex<ReplyView>,
        out: Arc<Mutex<Vec<u8>>>,
        shell: Arc<Mutex<ShellState>>,
        stale: AtomicBool,
    }

    fn theme() -> Theme {
        let mut t = crate::theme::all().swap_remove(0);
        t.fg = Color32::from_rgb(0xab, 0xb2, 0xbf);
        t.bg = Color32::from_rgba_unmultiplied(0x1e, 0x21, 0x27, 230);
        t.cursor = Color32::from_rgb(0x52, 0x8b, 0xff);
        t.ansi[1] = Color32::from_rgb(0xe0, 0x6c, 0x75);
        t
    }

    impl Rig {
        fn new() -> Rig {
            let out = Arc::new(Mutex::new(Vec::new()));
            let lst = Listener::new(Arc::new(Mutex::new(Box::new(Sink(out.clone())))));
            let term = Term::new(term_config(100), &TermSize::new(100, 30), lst.clone());
            Rig {
                term,
                lst,
                p: Processor::new(),
                scanner: OscScanner::default(),
                view: Mutex::new(ReplyView { theme: theme(), cell_w: 9, cell_h: 18 }),
                out,
                shell: Default::default(),
                stale: AtomicBool::new(false),
            }
        }

        /// Ausgabe des Programms verarbeiten (wie der PTY-Thread) und die Antworten an das Programm liefern.
        fn feed(&mut self, s: &str) -> String {
            let bytes = s.as_bytes();
            let scanned = self.scanner.feed_at(bytes);
            process_chunk(&mut self.p, &mut self.term, bytes, scanned, &self.lst, &self.shell, &self.stale);
            let r = self.lst.take_reply_bytes(&self.term, &self.view);
            self.lst.write_out(&r);
            String::from_utf8(std::mem::take(&mut *self.out.lock().unwrap())).unwrap()
        }
    }

    #[test]
    fn colour_queries_answer_with_theme() {
        let mut r = Rig::new();
        assert_eq!(r.feed("\x1b]10;?\x1b\\"), "\x1b]10;rgb:abab/b2b2/bfbf\x1b\\");
        // Hintergrund ohne Transparenz; BEL-Abschluss bleibt BEL.
        assert_eq!(r.feed("\x1b]11;?\x07"), "\x1b]11;rgb:1e1e/2121/2727\x07");
        assert_eq!(r.feed("\x1b]12;?\x1b\\"), "\x1b]12;rgb:5252/8b8b/ffff\x1b\\");
        assert_eq!(r.feed("\x1b]4;1;?\x1b\\"), "\x1b]4;1;rgb:e0e0/6c6c/7575\x1b\\");
        // Würfel- und Graufarben aus der festen Palette.
        assert_eq!(r.feed("\x1b]4;196;?\x07"), "\x1b]4;196;rgb:ffff/0000/0000\x07");
        assert_eq!(r.feed("\x1b]4;232;?\x07"), "\x1b]4;232;rgb:0808/0808/0808\x07");
    }

    #[test]
    fn colour_queries_follow_overrides_and_resets() {
        let mut r = Rig::new();
        // Setzen: Abfrage liefert den gesetzten Wert, nichts wird geschrieben.
        assert_eq!(r.feed("\x1b]11;#102030\x1b\\\x1b]10;rgb:aa/bb/cc\x1b\\\x1b]12;#010203\x07\x1b]4;1;#445566\x07\x1b]4;100;#778899\x07"), "");
        assert_eq!(r.feed("\x1b]11;?\x1b\\"), "\x1b]11;rgb:1010/2020/3030\x1b\\");
        assert_eq!(r.feed("\x1b]10;?\x1b\\"), "\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\");
        assert_eq!(r.feed("\x1b]12;?\x1b\\"), "\x1b]12;rgb:0101/0202/0303\x1b\\");
        assert_eq!(r.feed("\x1b]4;1;?\x1b\\"), "\x1b]4;1;rgb:4444/5555/6666\x1b\\");
        assert_eq!(r.feed("\x1b]4;100;?\x1b\\"), "\x1b]4;100;rgb:7777/8888/9999\x1b\\");
        // Die Darstellung nutzt die gesetzten Farben vor dem Theme.
        let eff = theme().with_overrides(r.term.colors()).unwrap();
        assert_eq!(eff.bg, Color32::from_rgba_unmultiplied(0x10, 0x20, 0x30, 230));
        assert_eq!(eff.ansi[1], Color32::from_rgb(0x44, 0x55, 0x66));
        // Zurücksetzen: OSC 110/111/112 einzeln, OSC 104 mit Index bzw. ohne (alle).
        r.feed("\x1b]111\x1b\\\x1b]110\x07\x1b]112\x07");
        assert_eq!(r.feed("\x1b]11;?\x1b\\"), "\x1b]11;rgb:1e1e/2121/2727\x1b\\");
        assert_eq!(r.feed("\x1b]10;?\x1b\\"), "\x1b]10;rgb:abab/b2b2/bfbf\x1b\\");
        assert_eq!(r.feed("\x1b]12;?\x1b\\"), "\x1b]12;rgb:5252/8b8b/ffff\x1b\\");
        r.feed("\x1b]104;1\x07");
        assert_eq!(r.feed("\x1b]4;1;?\x07"), "\x1b]4;1;rgb:e0e0/6c6c/7575\x07");
        assert_eq!(r.feed("\x1b]4;100;?\x07"), "\x1b]4;100;rgb:7777/8888/9999\x07");
        r.feed("\x1b]104\x07");
        assert_eq!(r.feed("\x1b]4;100;?\x07"), "\x1b]4;100;rgb:8787/8787/0000\x07");
        assert!(theme().with_overrides(r.term.colors()).is_none());
    }

    #[test]
    fn theme_change_reaches_replies() {
        let mut r = Rig::new();
        let mut t = theme();
        t.bg = Color32::from_rgb(0xfa, 0xfa, 0xfa);
        r.view.lock().unwrap().theme = t;
        assert_eq!(r.feed("\x1b]11;?\x07"), "\x1b]11;rgb:fafa/fafa/fafa\x07");
    }

    #[test]
    fn replies_keep_request_order() {
        // fish/Neovim: OSC 11 mit DA1 als Wächter – die Farbantwort muss vor der DA1-Antwort ankommen.
        let mut r = Rig::new();
        let got = r.feed("\x1b]11;?\x1b\\\x1b[c");
        assert_eq!(got, "\x1b]11;rgb:1e1e/2121/2727\x1b\\\x1b[?6c");
        // Ebenso XTVERSION (vom Scanner erkannt) zwischen Antworten der Emulation.
        let got = r.feed("\x1b[5n\x1b[>q\x1b[c");
        assert_eq!(got, format!("\x1b[0n\x1bP>|Paneldeck {}\x1b\\\x1b[?6c", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn size_and_identity_replies() {
        let mut r = Rig::new();
        // 14t: Textbereich in Pixeln (Zeilen×Zellhöhe, Spalten×Zellbreite), 16t: Zelle, 18t: Zeichen.
        assert_eq!(r.feed("\x1b[14t"), "\x1b[4;540;900t");
        assert_eq!(r.feed("\x1b[16t"), "\x1b[6;18;9t");
        assert_eq!(r.feed("\x1b[18t"), "\x1b[8;30;100t");
        let ver = format!("\x1bP>|Paneldeck {}\x1b\\", env!("CARGO_PKG_VERSION"));
        assert_eq!(r.feed("\x1b[>q"), ver);
        assert_eq!(r.feed("\x1b[>0q"), ver);
        // Neue Zellgröße (andere Schrift/Skalierung) gilt sofort.
        *r.view.lock().unwrap() = ReplyView { theme: theme(), cell_w: 10, cell_h: 21 };
        assert_eq!(r.feed("\x1b[16t\x1b[14t"), "\x1b[6;21;10t\x1b[4;630;1000t");
        // Nicht verwechseln: DECSCUSR, DA2.
        assert_eq!(r.feed("\x1b[2 q"), "");
        assert_eq!(r.feed("\x1b[>c"), "\x1b[>0;2600;1c");
    }

    #[test]
    fn restored_history_queries_are_not_answered() {
        let r = Rig::new();
        let mut term = Term::new(term_config(100), &TermSize::new(80, 24), r.lst.clone());
        let mut p: Processor = Processor::new();
        p.advance(&mut term, b"\x1b[c\x1b]11;?\x07");
        r.lst.drop_replies();
        assert!(r.lst.take_reply_bytes(&term, &r.view).is_empty());
    }

    #[test]
    fn focus_reports_only_with_mode_and_on_change() {
        let mut r = Rig::new();
        let off = *r.term.mode();
        assert_eq!(focus_report(None, true, off), None);
        assert_eq!(focus_report(Some(false), true, off), None);
        r.feed("\x1b[?1004h");
        let on = *r.term.mode();
        assert!(on.contains(TermMode::FOCUS_IN_OUT));
        assert_eq!(focus_report(Some(false), true, on), Some(&b"\x1b[I"[..]));
        assert_eq!(focus_report(Some(true), false, on), Some(&b"\x1b[O"[..]));
        assert_eq!(focus_report(None, false, on), Some(&b"\x1b[O"[..]));
        // Kein Wechsel: nichts (kein Senden in jedem Frame).
        assert_eq!(focus_report(Some(true), true, on), None);
        assert_eq!(focus_report(Some(false), false, on), None);
        r.feed("\x1b[?1004l");
        assert_eq!(focus_report(Some(false), true, *r.term.mode()), None);
    }

    #[test]
    fn paste_filters_esc_and_c1() {
        // Bracketed: ESC, andere C0, DEL und C1 raus; Tab und Zeilenumbruch bleiben.
        let evil = "a\x1b[201~rm -rf /\r\nb\tc\u{9b}201~d\u{85}e\u{7f}\u{7}";
        assert_eq!(paste_payload(evil, true), "\x1b[200~a[201~rm -rf /\rb\tc201~de\x1b[201~");
        // Ohne Bracketed Paste: nur C1 raus, Zeilenumbrüche als CR.
        assert_eq!(paste_payload("x\u{9b}31m\u{90}y\nz", false), "x31my\rz");
        assert_eq!(paste_payload("Grüße ✓ – ok", false), "Grüße ✓ – ok");
        assert_eq!(paste_payload("Grüße ✓", true), "\x1b[200~Grüße ✓\x1b[201~");
    }

    #[test]
    fn paste_line_counts() {
        assert_eq!(paste_line_count("eine Zeile"), None);
        assert_eq!(paste_line_count("cmd\n"), Some(1));
        assert_eq!(paste_line_count("a\r\nb\r\nc"), Some(3));
        assert_eq!(paste_line_count("a\rb"), Some(2));
        assert_eq!(paste_line_count("\n"), Some(1));
    }

    #[test]
    fn titles_are_sanitised() {
        assert_eq!(sanitize_title("vim – datei.rs"), "vim – datei.rs");
        // Steuerzeichen (C0, DEL, C1) und Schreibrichtungs-Zeichen fallen weg.
        assert_eq!(sanitize_title("a\x07b\x1b[31mc\u{7f}d\u{9b}e"), "ab[31mcde");
        assert_eq!(sanitize_title("evil\u{202e}txt.exe\u{202c}\u{2066}x\u{2069}\u{200f}"), "eviltxt.exex");
        assert_eq!(sanitize_title(&"x".repeat(1000)).chars().count(), TITLE_MAX_CHARS);
        assert_eq!(sanitize_title(&"ä".repeat(300)), "ä".repeat(TITLE_MAX_CHARS));
        // Über die Emulation: der Listener speichert nur den bereinigten Titel.
        let mut r = Rig::new();
        r.feed(&format!("\x1b]0;ok\u{202e}gpj.exe{}\x07", "y".repeat(400)));
        let t = r.lst.title.lock().unwrap().clone().unwrap();
        assert!(t.starts_with("okgpj.exe") && t.chars().count() == TITLE_MAX_CHARS, "{t}");
    }

    #[test]
    fn term_program_env() {
        let env = term_env();
        let get = |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(get("TERM_PROGRAM").as_deref(), Some("Paneldeck"));
        assert_eq!(get("TERM_PROGRAM_VERSION").as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert_eq!(get("TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(get("COLORTERM").as_deref(), Some("truecolor"));
    }
}

/// Ende-zu-Ende-Probe der Antworten durch ein echtes Pseudo-Terminal (#183), von Hand gestartet:
/// `PANELDECK_VTPROBE=<vtprobe.exe> cargo test --release conpty_probe -- --ignored --nocapture`.
/// `vtprobe app q <Sequenz>` (Wegwerf-Werkzeug aus der Analyse zu #183) schreibt die Anfrage, liest die Antwort als
/// Konsoleneingabe und legt sie in `PROBE_OUT` ab. Mit der neueren ConPTY (conpty.dll + OpenConsole.exe neben der
/// Test-.exe) kommen alle Anfragen an; mit der des Systems fehlen u. a. OSC 10/11.
#[cfg(all(test, windows))]
mod conpty_probe {
    use super::*;
    use crate::shells::ShellSpec;

    fn probe(seq: &str, focus: bool) -> String {
        let exe = std::env::var("PANELDECK_VTPROBE").expect("PANELDECK_VTPROBE");
        let out = std::env::temp_dir().join(format!("pd-vtprobe-{}-{}.txt", std::process::id(), seq.len()));
        let _ = std::fs::remove_file(&out);
        let shell = ShellSpec { id: "probe".into(), label: "probe".into(), program: exe, args: vec!["app".into(), "q".into(), seq.into()] };
        let ctx = egui::Context::default();
        let s = Session::spawn(
            &ctx,
            SpawnOptions { shell: &shell, cwd: None, restore_text: None, initial_input: None, scrollback_lines: 100, env: vec![("PROBE_OUT".into(), out.display().to_string())] },
        )
        .unwrap();
        s.set_view(&crate::theme::all()[0], 9, 18);
        let t0 = std::time::Instant::now();
        let mut sent = false;
        while t0.elapsed() < std::time::Duration::from_secs(8) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if focus && !sent && s.term.lock().unwrap().mode().contains(TermMode::FOCUS_IN_OUT) && s.dump_text(5).contains("<<S>>") {
                sent = true;
                s.report_focus(false);
                std::thread::sleep(std::time::Duration::from_millis(100));
                s.report_focus(true);
                s.report_focus(true); // kein zweites CSI I
            }
            if let Ok(r) = std::fs::read_to_string(&out).map(|r| r.trim().to_string()) {
                if !r.is_empty() {
                    return if focus { format!("{r} (Fokus gesendet: {sent})") } else { r };
                }
            }
        }
        "<keine Antwort>".into()
    }

    #[test]
    #[ignore]
    fn conpty_probe() {
        for seq in ["\\e]10;?\\e\\", "\\e]11;?\\e\\", "\\e]12;?\\a", "\\e]4;1;?\\e\\", "\\e]11;#102030\\e\\\\e]11;?\\e\\", "\\e[14t", "\\e[16t", "\\e[18t", "\\e[>q", "\\e[>0q", "\\e]11;?\\e\\\\e[c"] {
            println!("{seq:<32} {}", probe(seq, false));
        }
        // Das Programm fordert Fokusmeldungen an (DECSET 1004); OpenConsole reicht CSI O/I dann als Eingabe durch.
        println!("{:<32} {}", "focus out/in (?1004h)", probe("\\e[?1004h", true));
    }
}
