//! Ein laufender Language Server (#367): Prozess (eigenes Job Object, ohne Fenster), Lese- und Schreibfaden, Anfragen
//! mit Zeitgrenze, Dokumente öffnen/ändern/schließen, Diagnosen und Vervollständigen. Kein egui: die Oberfläche fragt
//! [`Client::poll`] je Bild ab.
//!
//! Sicherheit (Entscheidung zu #367): Anfragen des Servers, die Dateien ändern, Programme ausführen oder etwas öffnen
//! (`workspace/applyEdit`, `window/showDocument`, `workspace/executeCommand`), lehnt der Lesefaden sofort ab
//! ([`proto::answer_server_request`]). Das Protokoll landet nur im Speicher (höchstens 1 MB je Server, Mitschnitt der
//! Nachrichten nur mit dem Schalter), nie auf der Platte und nie im MCP-Protokoll.

use super::proto::{self, Caps, Completion, Diagnostic, Incoming, Pos, ReadError};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Größe des Protokolls im Speicher je Server.
pub const LOG_BYTES: usize = 1024 * 1024;
/// Antwortet der Server so lange nicht auf `initialize`, gilt er als abgestürzt (zählt für die Absturz-Regel).
pub const INIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Längste Zeile der Fehlerausgabe, die gelesen wird (der Rest der Zeile wird verworfen).
const STDERR_LINE: usize = 8192;

/// Eine Zeile mit Obergrenze lesen: höchstens `max` Bytes landen in `out`, der Rest der Zeile wird überlesen. Liefert die
/// Zahl gelesener Bytes (0 = Ende).
pub(crate) fn read_capped_line(r: &mut dyn BufRead, max: usize, out: &mut Vec<u8>) -> std::io::Result<usize> {
    out.clear();
    let mut total = 0;
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(total);
        }
        let (take, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        let room = max.saturating_sub(out.len());
        out.extend_from_slice(&buf[..take.min(room)]);
        r.consume(take);
        total += take;
        if done {
            return Ok(total);
        }
    }
}

/// Protokoll eines Servers im Speicher (Ring, älteste Zeilen fallen weg).
#[derive(Default, Debug)]
pub struct Log {
    lines: VecDeque<String>,
    bytes: usize,
    /// Wächst mit jeder Zeile (die Anzeige merkt sich den zusammengesetzten Text je Stand).
    pub rev: u64,
}

impl Log {
    pub fn push(&mut self, line: impl Into<String>) {
        let mut l: String = line.into();
        if l.len() > 8192 {
            let mut cut = 8192;
            while !l.is_char_boundary(cut) {
                cut -= 1;
            }
            l.truncate(cut);
            l.push('…');
        }
        self.bytes += l.len();
        self.rev += 1;
        self.lines.push_back(l);
        while self.bytes > LOG_BYTES {
            match self.lines.pop_front() {
                Some(x) => self.bytes -= x.len(),
                None => break,
            }
        }
    }

    pub fn lines(&self) -> impl Iterator<Item = &String> {
        self.lines.iter()
    }

    pub fn text(&self) -> String {
        self.lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

/// Zustand eines Servers.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Gestartet, `initialize` läuft.
    Starting,
    Ready,
    /// Beendet (Grund für die Anzeige).
    Exited(String),
}

/// Was der Lesefaden meldet.
enum Event {
    Message(Incoming),
    /// Strom zu Ende oder kaputt (Grund).
    Closed(String),
    Stderr(String),
}

/// Ergebnis einer Anfrage, das die Oberfläche abholt.
#[derive(Debug, PartialEq)]
pub enum Reply {
    Completions { ticket: u64, items: Vec<Completion>, incomplete: bool },
    /// Zeitgrenze oder Fehler des Servers (`ticket` der Anfrage).
    Failed { ticket: u64, message: String },
}

enum Pending {
    Initialize,
    Shutdown,
    Completion { ticket: u64, max: usize },
}

/// Diagnosen einer Datei (zum Stand `version` des Dokuments, falls der Server sie nennt).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileDiagnostics {
    pub version: Option<i64>,
    pub items: Vec<Diagnostic>,
    /// Mehr als die Grenze je Datei: abgeschnitten.
    pub cut: bool,
}

/// Grenzen eines Servers.
#[derive(Clone, Copy, Debug)]
pub struct ClientLimits {
    pub max_message: usize,
    pub completion_timeout: Duration,
    pub max_diagnostics: usize,
    pub trace: bool,
    /// Zeitgrenze für die Antwort auf `initialize` (danach gilt der Server als abgestürzt).
    pub init_timeout: Duration,
}

impl Default for ClientLimits {
    fn default() -> Self {
        ClientLimits { max_message: proto::DEFAULT_MAX_MESSAGE, completion_timeout: Duration::from_secs(5), max_diagnostics: 1000, trace: false, init_timeout: INIT_TIMEOUT }
    }
}

/// Ein Language Server.
pub struct Client {
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Event>,
    next_id: i64,
    pending: HashMap<i64, (Pending, Instant)>,
    pub caps: Caps,
    pub state: State,
    pub log: Log,
    limits: ClientLimits,
    /// Offene Dokumente: Adresse → Version.
    docs: HashMap<String, i64>,
    /// Diagnosen je Adresse.
    pub diagnostics: HashMap<String, FileDiagnostics>,
    /// Zähler: wächst bei jeder neuen Diagnose-Meldung (die Oberfläche merkt so Änderungen).
    pub diag_gen: u64,
    replies: Vec<Reply>,
    /// Nach `initialize` zu sendende Nachrichten (Dokumente, die vorher geöffnet wurden).
    queued: Vec<Value>,
    pub pid: Option<u32>,
    process: Option<std::process::Child>,
    /// Fehlerausgabe des Prozesses (zeilenweise).
    stderr: Option<mpsc::Receiver<Event>>,
    #[cfg_attr(not(any(windows, unix)), allow(dead_code))]
    job: Option<crate::platform::process::SessionJob>,
    pub last_used: Instant,
    /// Start (Zeitgrenze für `initialize`).
    started: Instant,
    /// Stand des Fortschritts (`$/progress`): Titel/Meldung, solange etwas läuft.
    pub progress: Option<String>,
}

impl Client {
    /// Mit fertigen Strömen (Tests, Prozess): startet Lese- und Schreibfaden und sendet `initialize`.
    pub fn with_streams(read: Box<dyn Read + Send>, write: Box<dyn Write + Send>, root: &Path, limits: ClientLimits, init: Option<Value>) -> Client {
        let (tx, wrx) = mpsc::channel::<Value>();
        let (etx, rx) = mpsc::channel::<Event>();
        // Schreibfaden
        std::thread::spawn(move || {
            let mut w = write;
            while let Ok(m) = wrx.recv() {
                if proto::write_message(&mut w, &m).is_err() {
                    break;
                }
            }
        });
        // Lesefaden: beantwortet Anfragen des Servers selbst (verweigert, was etwas ändern würde)
        let answer = tx.clone();
        let max = limits.max_message;
        std::thread::spawn(move || {
            let mut r = BufReader::new(read);
            loop {
                match proto::read_message(&mut r, max) {
                    Ok(v) => match proto::classify(v) {
                        Some(Incoming::Request { id, method, params }) => {
                            let _ = answer.send(proto::answer_server_request(&id, &method, &params));
                            if etx.send(Event::Message(Incoming::Request { id, method, params: Value::Null })).is_err() {
                                break;
                            }
                        }
                        Some(m) => {
                            if etx.send(Event::Message(m)).is_err() {
                                break;
                            }
                        }
                        None => {}
                    },
                    Err(e) => {
                        let why = match e {
                            ReadError::Eof => "eof".to_string(),
                            ReadError::TooLarge(n) => format!("message too large ({n} bytes)"),
                            ReadError::Malformed(m) => format!("malformed message: {m}"),
                            ReadError::Io(m) => m,
                        };
                        let _ = etx.send(Event::Closed(why));
                        break;
                    }
                }
            }
        });
        let mut c = Client {
            tx,
            rx,
            next_id: 1,
            pending: HashMap::new(),
            caps: Caps::default(),
            state: State::Starting,
            log: Log::default(),
            limits,
            docs: HashMap::new(),
            diagnostics: HashMap::new(),
            diag_gen: 0,
            replies: Vec::new(),
            queued: Vec::new(),
            pid: None,
            process: None,
            stderr: None,
            job: None,
            last_used: Instant::now(),
            started: Instant::now(),
            progress: None,
        };
        let root_s = root.to_string_lossy().to_string();
        let uri = proto::path_to_uri(&root_s);
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| root_s.clone());
        let mut params = proto::initialize_params(std::process::id(), &uri, &name);
        if let Some(o) = init {
            params["initializationOptions"] = o;
        }
        c.request(Pending::Initialize, "initialize", params);
        c
    }

    /// Server starten: `program` mit `args` im Ordner `cwd`, ohne Fenster, in einem eigenen Job Object (endet mit
    /// Paneldeck), Fehlerausgabe ins Protokoll.
    pub fn spawn(program: &Path, args: &[String], raw_tail: Option<&str>, cwd: &Path, root: &Path, limits: ClientLimits, init: Option<Value>) -> Result<Client, String> {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args).current_dir(cwd).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        // Befehlszeile einer `.cmd`-Hülle unverändert (sonst maskiert Rust die Anführungszeichen für cmd unlesbar).
        if let Some(t) = raw_tail {
            crate::platform::process::raw_arg(&mut cmd, t.to_string());
        }
        // Programme ohne Pfad (z. B. `node` in einer npm-Hülle) nie im Arbeitsordner (= Projekt) suchen.
        cmd.env("NoDefaultCurrentDirectoryInExePath", "1");
        crate::platform::process::hide_window(&mut cmd);
        let job = crate::platform::process::SessionJob::new();
        if let Some(j) = &job {
            j.kill_on_close();
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let pid = child.id();
        #[cfg(windows)]
        if let Some(j) = &job {
            use std::os::windows::io::AsRawHandle;
            j.adopt(child.as_raw_handle(), pid);
        }
        #[cfg(unix)]
        if let Some(j) = &job {
            j.adopt_pid(pid);
        }
        let out = child.stdout.take().ok_or("no stdout")?;
        let inp = child.stdin.take().ok_or("no stdin")?;
        let err = child.stderr.take();
        let mut c = Client::with_streams(Box::new(out), Box::new(inp), root, limits, init);
        c.pid = Some(pid);
        c.process = Some(child);
        c.job = job;
        if let Some(e) = err {
            // Fehlerausgabe des Servers zeilenweise ins Protokoll (über den Ereignis-Kanal).
            let (etx, erx) = mpsc::channel::<Event>();
            std::thread::spawn(move || {
                let mut r = BufReader::new(e);
                let mut buf = Vec::new();
                while let Ok(n) = read_capped_line(&mut r, STDERR_LINE, &mut buf) {
                    if n == 0 {
                        break;
                    }
                    let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                    if etx.send(Event::Stderr(line)).is_err() {
                        break;
                    }
                }
            });
            c.stderr = Some(erx);
        }
        c.log.push(format!("started {} (pid {pid})", program.display()));
        Ok(c)
    }

    fn send(&mut self, msg: Value) {
        if self.limits.trace {
            self.log.push(format!("→ {msg}"));
        }
        let _ = self.tx.send(msg);
    }

    fn request(&mut self, kind: Pending, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, (kind, Instant::now()));
        self.send(proto::request(id, method, params));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = proto::notification(method, params);
        if self.state == State::Starting {
            self.queued.push(msg);
        } else {
            self.send(msg);
        }
    }

    /// Vor der Antwort auf `initialize`: für dieses Dokument nur den neuesten Text vormerken (nie eine wachsende
    /// Schlange): das vorgemerkte `didOpen` bekommt den neuen Text, `didChange` entfällt.
    fn queue_latest(&mut self, uri: &str, version: i64, text: &str) -> bool {
        for q in self.queued.iter_mut() {
            if q["method"] == "textDocument/didOpen" && q["params"]["textDocument"]["uri"] == uri {
                q["params"]["textDocument"]["text"] = Value::String(text.to_string());
                q["params"]["textDocument"]["version"] = json!(version);
                return true;
            }
        }
        false
    }

    /// Dokument öffnen (`didOpen`); schon offen = nichts.
    pub fn open(&mut self, uri: &str, language: &str, text: &str) {
        if self.docs.contains_key(uri) || matches!(self.state, State::Exited(_)) {
            return;
        }
        self.docs.insert(uri.to_string(), 1);
        self.notify("textDocument/didOpen", json!({"textDocument": {"uri": uri, "languageId": language_id(language), "version": 1, "text": text}}));
        self.last_used = Instant::now();
    }

    /// Ganzen Text senden (`didChange` sofort, Entscheidung zu #367: nur das Anzeigen der Diagnosen wartet).
    pub fn change(&mut self, uri: &str, text: &str) -> Option<i64> {
        let v = self.docs.get_mut(uri)?;
        *v += 1;
        let version = *v;
        if self.state == State::Starting && self.queue_latest(uri, version, text) {
            self.last_used = Instant::now();
            return Some(version);
        }
        self.notify("textDocument/didChange", json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}));
        // Diagnosen für einen älteren Stand sind ab jetzt veraltet (die Oberfläche zeigt sie nicht mehr an).
        self.last_used = Instant::now();
        Some(version)
    }

    /// Mitschnitt der Nachrichten an?
    pub fn traced(&self) -> bool {
        self.limits.trace
    }

    pub fn open_docs(&self) -> usize {
        self.docs.len()
    }

    /// Arbeitsspeicher des Prozesses (Bytes; unter WSL nur der von `wsl.exe`, darum dort `None`).
    pub fn memory(&self) -> Option<u64> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            self.process.as_ref().and_then(|p| crate::platform::process::working_set(p.as_raw_handle()))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn version(&self, uri: &str) -> Option<i64> {
        self.docs.get(uri).copied()
    }

    pub fn saved(&mut self, uri: &str, text: &str) {
        if !self.docs.contains_key(uri) {
            return;
        }
        let params = if self.caps.save_text { json!({"textDocument": {"uri": uri}, "text": text}) } else { json!({"textDocument": {"uri": uri}}) };
        self.notify("textDocument/didSave", params);
    }

    pub fn close(&mut self, uri: &str) {
        if self.docs.remove(uri).is_some() {
            self.diagnostics.remove(uri);
            self.diag_gen += 1;
            self.notify("textDocument/didClose", json!({"textDocument": {"uri": uri}}));
        }
    }

    /// Vervollständigen an `pos` anfragen; die Antwort kommt als [`Reply::Completions`] mit diesem `ticket`.
    pub fn complete(&mut self, uri: &str, pos: Pos, trigger: Option<char>, ticket: u64, max: usize) -> bool {
        if self.state != State::Ready || !self.caps.completion || !self.docs.contains_key(uri) {
            return false;
        }
        let context = match trigger {
            Some(c) => json!({"triggerKind": 2, "triggerCharacter": c.to_string()}),
            None => json!({"triggerKind": 1}),
        };
        self.request(Pending::Completion { ticket, max }, "textDocument/completion", json!({"textDocument": {"uri": uri}, "position": pos.json(), "context": context}));
        self.last_used = Instant::now();
        true
    }

    /// Anfragen abbrechen, deren Ergebnis keiner mehr will (`$/cancelRequest`).
    pub fn cancel_completions(&mut self) {
        let ids: Vec<i64> = self.pending.iter().filter(|(_, (p, _))| matches!(p, Pending::Completion { .. })).map(|(id, _)| *id).collect();
        for id in ids {
            self.pending.remove(&id);
            self.send(proto::notification("$/cancelRequest", json!({"id": id})));
        }
    }

    /// Ergebnisse abholen.
    pub fn take_replies(&mut self) -> Vec<Reply> {
        std::mem::take(&mut self.replies)
    }

    /// Je Bild: Nachrichten verarbeiten, Zeitgrenzen prüfen. Liefert, ob sich etwas getan hat.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        if let Some(erx) = &self.stderr {
            let lines: Vec<String> = erx.try_iter().filter_map(|e| if let Event::Stderr(l) = e { Some(l) } else { None }).collect();
            for l in lines {
                self.log.push(format!("stderr: {l}"));
                any = true;
            }
        }
        while let Ok(ev) = self.rx.try_recv() {
            any = true;
            match ev {
                Event::Message(m) => self.handle(m),
                Event::Closed(why) => {
                    self.log.push(format!("closed: {why}"));
                    self.exited(why);
                }
                Event::Stderr(l) => self.log.push(format!("stderr: {l}")),
            }
        }
        let now = Instant::now();
        let timeout = self.limits.completion_timeout;
        let late: Vec<i64> = self.pending.iter().filter(|(_, (p, at))| matches!(p, Pending::Completion { .. }) && now.duration_since(*at) > timeout).map(|(id, _)| *id).collect();
        for id in late {
            if let Some((Pending::Completion { ticket, .. }, _)) = self.pending.remove(&id) {
                self.send(proto::notification("$/cancelRequest", json!({"id": id})));
                self.replies.push(Reply::Failed { ticket, message: "timeout".into() });
                any = true;
            }
        }
        if self.state == State::Starting && self.started.elapsed() > self.limits.init_timeout {
            self.log.push("initialize timeout");
            self.kill();
            self.exited("initialize timeout".into());
            any = true;
        }
        if let Some(p) = self.process.as_mut()
            && let Ok(Some(status)) = p.try_wait()
            && !matches!(self.state, State::Exited(_))
        {
            self.exited(format!("exit {}", status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())));
            any = true;
        }
        any
    }

    fn exited(&mut self, why: String) {
        if !matches!(self.state, State::Exited(_)) {
            self.state = State::Exited(why);
            for (_, (p, _)) in self.pending.drain() {
                if let Pending::Completion { ticket, .. } = p {
                    self.replies.push(Reply::Failed { ticket, message: "server ended".into() });
                }
            }
            self.diagnostics.clear();
            self.diag_gen += 1;
        }
    }

    fn handle(&mut self, m: Incoming) {
        match m {
            Incoming::Response { id, result } => {
                let Some((kind, _)) = self.pending.remove(&id) else { return };
                match (kind, result) {
                    (Pending::Initialize, Ok(v)) => {
                        self.caps = proto::parse_caps(&v);
                        self.state = State::Ready;
                        self.send(proto::notification("initialized", json!({})));
                        for q in std::mem::take(&mut self.queued) {
                            self.send(q);
                        }
                        self.log.push("initialized");
                    }
                    (Pending::Initialize, Err((c, msg))) => {
                        self.log.push(format!("initialize failed ({c}): {msg}"));
                        self.exited(format!("initialize failed: {msg}"));
                    }
                    (Pending::Completion { ticket, max }, Ok(v)) => {
                        let (items, incomplete) = proto::parse_completions(&v, max);
                        self.replies.push(Reply::Completions { ticket, items, incomplete });
                    }
                    (Pending::Completion { ticket, .. }, Err((_, msg))) => self.replies.push(Reply::Failed { ticket, message: msg }),
                    (Pending::Shutdown, _) => {
                        self.send(proto::notification("exit", Value::Null));
                    }
                }
            }
            Incoming::Notification { method, params } => {
                if self.limits.trace {
                    self.log.push(format!("← {method} {params}"));
                }
                match method.as_str() {
                    "textDocument/publishDiagnostics" => {
                        if let Some((uri, version, items, cut)) = proto::parse_publish(&params, self.limits.max_diagnostics) {
                            // Nur für offene Dokumente; eine Meldung für einen älteren Stand fällt weg (sie passte nicht
                            // mehr zum Text).
                            // Adressen vergleichen wie Pfade: `file:///c%3A/x` (vscode-uri) = `file:///c:/x`.
                            let k = proto::uri_key(&uri);
                            let Some(ours) = self.docs.keys().find(|d| proto::uri_key(d) == k).cloned() else { return };
                            let cur = self.docs.get(&ours).copied();
                            if cur.is_some() && !version.is_some_and(|v| cur.is_some_and(|c| v < c)) {
                                self.diagnostics.insert(ours, FileDiagnostics { version, items, cut });
                                self.diag_gen += 1;
                            }
                        }
                    }
                    "window/logMessage" | "window/showMessage" => {
                        let msg = params.get("message").and_then(Value::as_str).unwrap_or("");
                        self.log.push(format!("{method}: {msg}"));
                    }
                    "$/progress" => {
                        let v = params.get("value");
                        match v.and_then(|v| v.get("kind")).and_then(Value::as_str) {
                            Some("begin") | Some("report") => {
                                let t = v.and_then(|v| v.get("title").or_else(|| v.get("message"))).and_then(Value::as_str).unwrap_or("…");
                                self.progress = Some(t.chars().take(80).collect());
                            }
                            Some("end") => self.progress = None,
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Incoming::Request { method, .. } => {
                // Schon im Lesefaden beantwortet; hier nur fürs Protokoll.
                if proto::REFUSED_SERVER_REQUESTS.contains(&method.as_str()) {
                    self.log.push(format!("refused server request {method}"));
                } else if self.limits.trace {
                    self.log.push(format!("← request {method}"));
                }
            }
        }
    }

    /// Server ordentlich beenden (`shutdown`, `exit`), ohne den UI-Thread warten zu lassen: ein Hilfsfaden gibt ihm
    /// kurz Zeit und beendet ihn dann hart über das Job Object.
    pub fn stop(&mut self) {
        if matches!(self.state, State::Exited(_)) {
            return;
        }
        self.request(Pending::Shutdown, "shutdown", Value::Null);
        self.send(proto::notification("exit", Value::Null));
        let (mut process, job) = (self.process.take(), self.job.take());
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_millis(800);
            while Instant::now() < until {
                if process.as_mut().is_none_or(|p| p.try_wait().ok().flatten().is_some()) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Some(j) = &job {
                j.terminate_all();
            }
            if let Some(p) = process.as_mut() {
                let _ = p.kill();
                let _ = p.wait();
            }
        });
        self.exited("stopped".into());
    }

    fn kill(&mut self) {
        if let Some(j) = &self.job {
            j.terminate_all();
        }
        if let Some(p) = self.process.as_mut() {
            let _ = p.kill();
            let _ = p.wait();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}

/// `languageId` für LSP aus der Sprach-Id des Syntax-Modus.
pub fn language_id(lang: &str) -> &str {
    match lang {
        "shell" => "shellscript",
        "tsx" => "typescriptreact",
        "csharp" => "csharp",
        "latex" => "latex",
        "makefile" => "makefile",
        other => other,
    }
}

/// Wurzel des Projekts: der nächste Ordner über `file` mit einer Markierungsdatei der Sprache (`Cargo.toml`,
/// `package.json` …), sonst mit `.git`, sonst der Ordner der Datei. `stop` (z. B. das Benutzerverzeichnis) wird nie
/// überschritten.
pub fn project_root(file: &Path, lang: &str, exists: &dyn Fn(&Path) -> bool, stop: Option<&Path>) -> PathBuf {
    let markers: &[&str] = match lang {
        "rust" => &["Cargo.toml"],
        "python" => &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt"],
        "typescript" | "javascript" | "tsx" | "json" => &["package.json", "tsconfig.json", "jsconfig.json"],
        "go" => &["go.mod"],
        "c" | "cpp" => &["compile_commands.json", "CMakeLists.txt", ".clangd"],
        "csharp" => &["*.sln", "*.csproj"],
        "java" | "kotlin" => &["pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle"],
        "php" => &["composer.json"],
        "ruby" => &["Gemfile"],
        "lua" => &[".luarc.json"],
        _ => &[],
    };
    let dir = file.parent().unwrap_or(file).to_path_buf();
    let mut best_git: Option<PathBuf> = None;
    let mut cur = Some(dir.as_path());
    // Rust-Arbeitsbereiche: der oberste Ordner mit Cargo.toml (Workspace) gewinnt.
    let mut found: Option<PathBuf> = None;
    while let Some(d) = cur {
        if markers.iter().any(|m| !m.starts_with('*') && exists(&d.join(m))) {
            found = Some(d.to_path_buf());
            if lang != "rust" {
                break;
            }
        }
        if best_git.is_none() && exists(&d.join(".git")) {
            best_git = Some(d.to_path_buf());
        }
        if stop.is_some_and(|s| s == d) {
            break;
        }
        cur = d.parent();
    }
    found.or(best_git).unwrap_or(dir)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Ersatz-Server im selben Prozess: liest, was der Client schreibt, und antwortet nach `script`.
    struct Fake {
        to_client: Option<std::io::PipeWriter>,
    }

    fn pair() -> (std::io::PipeReader, std::io::PipeWriter) {
        std::io::pipe().unwrap()
    }

    /// Startet den Ersatz-Server; `seen` sammelt die Methoden, die er bekam.
    fn fake(seen: Arc<Mutex<Vec<Value>>>, diag_version: i64) -> (Client, Fake) {
        let (c_read, s_write) = pair();
        let (s_read, c_write) = pair();
        let mut s_out = s_write.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut r = BufReader::new(s_read);
            while let Ok(m) = proto::read_message(&mut r, proto::DEFAULT_MAX_MESSAGE) {
                seen.lock().unwrap().push(m.clone());
                let method = m.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                let id = m.get("id").cloned();
                match method.as_str() {
                    "initialize" => {
                        let _ = proto::write_message(&mut s_out, &proto::response(&id.unwrap(), json!({"capabilities": {"textDocumentSync": 1, "completionProvider": {"triggerCharacters": ["."]}}})));
                        // Der Server versucht, eine Datei zu ändern: muss abgelehnt werden.
                        let _ = proto::write_message(&mut s_out, &json!({"jsonrpc": "2.0", "id": "s1", "method": "workspace/applyEdit", "params": {"edit": {}}}));
                    }
                    "textDocument/didOpen" => {
                        let uri = m["params"]["textDocument"]["uri"].clone();
                        let d = json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "severity": 1, "message": "bad"});
                        let _ = proto::write_message(&mut s_out, &proto::notification("textDocument/publishDiagnostics", json!({"uri": uri, "version": diag_version, "diagnostics": [d]})));
                    }
                    "textDocument/completion" => {
                        let _ = proto::write_message(&mut s_out, &proto::response(&id.unwrap(), json!([{"label": "len", "kind": 2}, {"label": "last", "kind": 3}])));
                    }
                    "shutdown" => {
                        let _ = proto::write_message(&mut s_out, &proto::response(&id.unwrap(), Value::Null));
                    }
                    "exit" => break,
                    _ => {}
                }
            }
        });
        let c = Client::with_streams(Box::new(c_read), Box::new(c_write), Path::new(r"C:\proj"), ClientLimits::default(), Some(json!({"x": 1})));
        (c, Fake { to_client: Some(s_write) })
    }

    fn wait(c: &mut Client, done: impl Fn(&Client) -> bool) {
        let t0 = Instant::now();
        while !done(c) {
            c.poll();
            assert!(t0.elapsed() < Duration::from_secs(5), "Zeitüberschreitung");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn initialize_open_diagnostics_completion_and_refused_edits() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut c, _f) = fake(seen.clone(), 1);
        // Vor `initialize` geöffnet: wartet und geht danach raus
        c.open("file:///c:/proj/a.rs", "rust", "fn main() {}");
        wait(&mut c, |c| c.state == State::Ready);
        assert!(c.caps.completion && c.caps.triggers == vec![".".to_string()]);
        wait(&mut c, |c| c.diagnostics.contains_key("file:///c:/proj/a.rs"));
        assert_eq!(c.diagnostics["file:///c:/proj/a.rs"].items[0].message, "bad");
        assert!(c.complete("file:///c:/proj/a.rs", Pos { line: 0, character: 3 }, Some('.'), 7, 100));
        wait(&mut c, |c| !c.replies.is_empty());
        match &c.take_replies()[0] {
            Reply::Completions { ticket, items, .. } => assert_eq!((*ticket, items.len()), (7, 2)),
            r => panic!("{r:?}"),
        }
        // Die Antwort auf applyEdit war ein Fehler
        let t0 = Instant::now();
        loop {
            let s = seen.lock().unwrap();
            if let Some(a) = s.iter().find(|m| m.get("id") == Some(&json!("s1"))) {
                assert_eq!(a["error"]["code"], proto::REQUEST_FAILED);
                break;
            }
            drop(s);
            assert!(t0.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let methods: Vec<String> = seen.lock().unwrap().iter().filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string)).collect();
        assert_eq!(&methods[..3], &["initialize".to_string(), "initialized".to_string(), "textDocument/didOpen".to_string()]);
        assert_eq!(seen.lock().unwrap()[0]["params"]["initializationOptions"], json!({"x": 1}));
        // Ändern: neue Version; Schließen nimmt die Diagnosen weg
        assert_eq!(c.change("file:///c:/proj/a.rs", "fn main() { }"), Some(2));
        c.close("file:///c:/proj/a.rs");
        assert!(c.diagnostics.is_empty());
        c.stop();
        assert!(matches!(c.state, State::Exited(_)));
    }

    #[test]
    fn stale_diagnostics_are_dropped_and_the_end_of_the_stream_ends_the_server() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut c, mut f) = fake(seen, 1);
        wait(&mut c, |c| c.state == State::Ready);
        // Schon vor der Meldung geändert: Version 2 > 1 der Meldung → verworfen
        c.open("file:///c:/proj/b.rs", "rust", "x");
        c.change("file:///c:/proj/b.rs", "xy");
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(300) {
            c.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!c.diagnostics.contains_key("file:///c:/proj/b.rs"), "alte Diagnose fällt weg");
        // Server weg: Zustand beendet
        f.to_client.take();
        drop(f);
        c.tx.send(proto::notification("exit", Value::Null)).ok();
        wait(&mut c, |c| matches!(c.state, State::Exited(_)));
    }

    /// Ersatz-Server als eigener Prozess (`examples/fake_lsp.rs`, von `cargo test` gebaut); fehlt er, wird übersprungen.
    pub(crate) fn fake_server_exe() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.parent()?.join("examples");
        let p = dir.join(if cfg!(windows) { "fake_lsp.exe" } else { "fake_lsp" });
        p.is_file().then_some(p)
    }

    #[test]
    fn a_real_server_process_starts_reports_and_stops() {
        let Some(exe) = fake_server_exe() else {
            eprintln!("fake_lsp fehlt – übersprungen");
            return;
        };
        let dir = std::env::temp_dir();
        let mut c = Client::spawn(&exe, &[], None, &dir, &dir, ClientLimits::default(), None).expect("startet");
        assert!(c.pid.is_some());
        #[cfg(windows)]
        assert!(c.memory().is_some_and(|m| m > 0), "Arbeitsspeicher");
        c.open("file:///c:/t/a.rs", "rust", "abc");
        wait(&mut c, |c| c.diagnostics.contains_key("file:///c:/t/a.rs"));
        assert_eq!(c.diagnostics["file:///c:/t/a.rs"].items[0].message, "fake error");
        assert!(c.complete("file:///c:/t/a.rs", Pos { line: 0, character: 1 }, None, 3, 50));
        wait(&mut c, |c| !c.replies.is_empty());
        assert!(matches!(&c.take_replies()[0], Reply::Completions { items, .. } if items[0].snippet));
        wait(&mut c, |c| c.log.lines().any(|l| l.contains("refused server request workspace/applyEdit")));
        c.stop();
        assert!(matches!(c.state, State::Exited(_)));
    }

    /// Server hinter einer `.cmd`-Hülle in einem Pfad mit Leerzeichen startet wirklich (Review: cmd-Zeile über
    /// `raw_arg`, nicht über die maskierende Übergabe).
    #[cfg(windows)]
    #[test]
    fn a_cmd_wrapper_in_a_path_with_spaces_starts() {
        let Some(exe) = fake_server_exe() else {
            eprintln!("fake_lsp fehlt – übersprungen");
            return;
        };
        let dir = std::env::temp_dir().join(format!("pd lsp cmd {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shim = dir.join("fake lsp.cmd");
        std::fs::write(&shim, format!("@\"{}\" %*\r\n", exe.display())).unwrap();
        let f = |p: &Path| std::fs::metadata(p).ok().map(|m| m.len());
        let sys32 = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32")).unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"));
        let env = super::super::servers::Env { path: vec![], exts: vec![], project: None, system32: sys32, rustup_size: None, is_file: &f };
        let l = super::super::servers::launch_for(&shim, ".cmd", vec!["--stdio".into()], &env, 10);
        assert!(l.invalid.is_none());
        let mut c = Client::spawn(&l.program, &l.args, l.raw_tail.as_deref(), &dir, &dir, ClientLimits::default(), None).expect("startet");
        c.open("file:///c:/t/b.rs", "rust", "x");
        wait(&mut c, |c| c.diagnostics.contains_key("file:///c:/t/b.rs"));
        c.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Vor `initialize`: nur der neueste Text je Dokument wartet; antwortet der Server nicht, endet er nach der Frist.
    #[test]
    fn the_queue_before_initialize_keeps_only_the_latest_text_and_initialize_times_out() {
        let (c_read, s_write) = pair();
        let (_s_read, c_write) = pair();
        let limits = ClientLimits { init_timeout: Duration::from_millis(100), ..Default::default() };
        let mut c = Client::with_streams(Box::new(c_read), Box::new(c_write), Path::new(r"C:\p"), limits, None);
        c.open("file:///c:/p/a.rs", "rust", "v1");
        for i in 0..500 {
            c.change("file:///c:/p/a.rs", &format!("v{i}"));
        }
        assert_eq!(c.queued.len(), 1, "nie eine wachsende Schlange");
        assert_eq!(c.queued[0]["params"]["textDocument"]["text"], "v499");
        wait(&mut c, |c| matches!(c.state, State::Exited(_)));
        assert!(matches!(&c.state, State::Exited(w) if w.contains("initialize")));
        drop(s_write);
    }

    #[test]
    fn long_stderr_lines_are_capped() {
        let data = format!("{}\nshort\n", "x".repeat(100_000));
        let mut r = BufReader::new(data.as_bytes());
        let mut buf = Vec::new();
        assert_eq!(read_capped_line(&mut r, 10, &mut buf).unwrap(), 100_001);
        assert_eq!(buf.len(), 10);
        read_capped_line(&mut r, 10, &mut buf).unwrap();
        assert_eq!(buf, b"short\n");
        assert_eq!(read_capped_line(&mut r, 10, &mut buf).unwrap(), 0);
    }

    #[test]
    fn project_roots_use_markers_then_git_and_never_pass_the_stop_folder() {
        let files = ["C:/w/ws/Cargo.toml", "C:/w/ws/crates/a/Cargo.toml", "C:/w/.git", "C:/w/js/package.json"];
        let exists = |p: &Path| files.contains(&p.to_string_lossy().replace('\\', "/").as_str());
        assert_eq!(project_root(Path::new("C:/w/ws/crates/a/src/lib.rs"), "rust", &exists, None), PathBuf::from("C:/w/ws"), "Workspace");
        assert_eq!(project_root(Path::new("C:/w/js/src/x.ts"), "typescript", &exists, None), PathBuf::from("C:/w/js"));
        assert_eq!(project_root(Path::new("C:/w/docs/a.md"), "markdown", &exists, None), PathBuf::from("C:/w"), ".git");
        assert_eq!(project_root(Path::new("C:/w/docs/a.md"), "markdown", &exists, Some(Path::new("C:/w/docs"))), PathBuf::from("C:/w/docs"));
        let mut log = Log::default();
        for i in 0..3000 {
            log.push(format!("{i} {}", "x".repeat(500)));
        }
        assert!(log.bytes <= LOG_BYTES && log.lines().next().is_some_and(|l| !l.starts_with("0 ")));
        assert_eq!(language_id("shell"), "shellscript");
    }
}
