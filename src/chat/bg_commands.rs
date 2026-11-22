//! Befehle der Chats im Hintergrund (#355): `run_command` mit `background: true` gibt sofort eine Kennung (`bg1`, `bg2`
//! … je Chat) zurück; der Befehl läuft in einem eigenen, sichtbaren Terminal unter dem Chat (Kind in der Seitenleiste).
//! `command_status`, `command_output` (seitenweise oder das Ende) und `stop_command` arbeiten mit der Kennung. Endet ein
//! Befehl, kommt eine Nachricht in den Chat (`ChatState::push_event`), und das Modell macht weiter, wenn seine Runde
//! noch läuft. Höchstens [`AgentToolSettings::background_limit`](super::agent::AgentToolSettings) Befehle laufen je Chat
//! gleichzeitig.
//!
//! Dazu die Regeln für alle Befehle des Chats: Zeitlimit im Vordergrund ([`parse_run`]: Standard und Höchstwert aus den
//! Einstellungen) und die Wahl der Shell ([`resolve_shell`]: ein Profil oder eine erkannte Shell des Nutzers, nie
//! Administrator-Profile und nie Profile mit eigenem Programm, Argumenten oder Startbefehl).
//!
//! Kennt weder egui noch die App: Terminals, Eingaben und Nachrichten laufen über [`BgHost`] (umgesetzt von der App in
//! `app/mcp/bg.rs`, in Tests ein Ersatz-Hub).

use super::agent::{err, int_arg, str_arg, AgentToolSettings, ToolErr};
use super::store::ChatEvent;
use super::tools::ToolResult;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Grenzen der Einstellungen: gleichzeitige Hintergrund-Befehle je Chat.
pub const DEFAULT_LIMIT: u32 = 3;
pub const MIN_LIMIT: u32 = 1;
pub const MAX_LIMIT: u32 = 10;
/// Höchstes Zeitlimit eines Befehls im Vordergrund (Standard der Einstellung) und die Spanne der Einstellung (Sekunden).
pub const DEFAULT_TIMEOUT_MAX: u64 = 30 * 60;
pub const TIMEOUT_MAX_MIN: u64 = 10;
pub const TIMEOUT_MAX_LIMIT: u64 = 4 * 60 * 60;
/// Kleinstes Standard-Zeitlimit (Sekunden).
pub const TIMEOUT_MIN: u64 = 5;
/// So lange wartet ein neuer Hintergrund-Befehl auf die Shell-Integration seines Terminals.
pub const READY_WAIT: Duration = Duration::from_secs(20);
/// So lange nach Strg+C (`stop_command`) darf ein Befehl noch enden; danach wird sein Terminal geschlossen.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// Höchstens so viele Ausgabezeilen hält ein beendeter Befehl (das Ende).
pub const MAX_KEPT_LINES: usize = 5000;
/// `command_output`: Standard und Höchstzahl Zeilen je Aufruf.
pub const DEFAULT_PAGE: usize = 200;
pub const MAX_PAGE: usize = 2000;
/// Höchstens so viele beendete Befehle behält ein Chat (die ältesten fallen weg).
const KEEP_FINISHED: usize = 30;
/// Längster Befehl (Bytes), wie beim Paneldeck-Werkzeug `run_command`.
pub const MAX_COMMAND_BYTES: usize = 4096;

/// Zustand eines Hintergrund-Befehls (Feld `status` der Werkzeuge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BgState {
    /// Terminal startet, Befehl noch nicht eingegeben.
    Starting,
    Running,
    /// Beendet (Prompt kam zurück).
    Exited,
    /// Auf Wunsch angehalten (`stop_command`).
    Stopped,
    /// Ließ sich nicht ausführen oder das Terminal verschwand.
    Failed,
}

impl BgState {
    pub fn as_str(self) -> &'static str {
        match self {
            BgState::Starting => "starting",
            BgState::Running => "running",
            BgState::Exited => "exited",
            BgState::Stopped => "stopped",
            BgState::Failed => "failed",
        }
    }
    pub fn active(self) -> bool {
        matches!(self, BgState::Starting | BgState::Running)
    }
}

/// Ein Befehl des Chats, wie das Modell ihn verlangt hat (nach [`parse_run`]).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunRequest {
    pub command: String,
    pub background: bool,
    /// Zeitlimit im Vordergrund (Sekunden, schon begrenzt); im Hintergrund ohne Bedeutung.
    pub timeout_secs: u64,
    pub max_output_lines: usize,
    /// Gewünschte Shell bzw. Profil (Name, wie das Modell ihn schrieb); `None` = Profil des Projekts.
    pub shell: Option<String>,
}

/// Ist ein Schalter (`background`) gesetzt? `false`, `"false"`, `"no"`, `"0"`, `""`, `0` und `null` nicht; ein unklarer
/// Wert zählt als gesetzt (dann meldet [`parse_run`] `bad_args`).
pub fn flag_set(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "no" | "0" | ""),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(_) => true,
    }
}

/// Nennt der Aufruf eine Shell (`shell`, nicht leer)? Ein Wert, der kein Text ist, zählt als genannt (dann `bad_args`).
pub fn shell_given(args: &Value) -> bool {
    match args.get("shell") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

/// Schlüssel der verlangten Shell für Freigaben und Terminals: Name in Vergleichsform (`Git Bash` = `git-bash` =
/// `gitbash`); `None` = das Profil des Projekts.
pub fn shell_key(args: &Value) -> Option<String> {
    args.get("shell").and_then(Value::as_str).map(norm).filter(|s| !s.is_empty())
}

/// Höchste Nummer eines Hintergrund-Befehls (`bgN`) im Verlauf – aus den Nachrichten von Paneldeck und den Ergebnissen
/// von `run_command` –, damit die Kennungen nach einem Neustart weiterzählen (#355 Review).
pub fn max_id_in(msgs: &[super::store::ChatMsg]) -> u32 {
    let num = |id: &str| id.strip_prefix("bg").and_then(|n| n.parse::<u32>().ok());
    msgs.iter()
        .filter_map(|m| {
            if let Some(e) = &m.event {
                return num(&e.id);
            }
            if m.tool_name.as_deref() == Some("run_command") {
                let v: Value = serde_json::from_str(&m.content).ok()?;
                if v.get("background").and_then(Value::as_bool) == Some(true) {
                    return v.get("id").and_then(Value::as_str).and_then(num);
                }
            }
            None
        })
        .max()
        .unwrap_or(0)
}

/// Wahrheitswert auch aus Text.
fn bool_arg(args: &Value, key: &str) -> Result<bool, ToolErr> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1") => Ok(true),
        Some(Value::String(s)) if matches!(s.trim().to_lowercase().as_str(), "false" | "no" | "0" | "") => Ok(false),
        Some(_) => Err(err("bad_args", format!("{key} must be true or false. Example: {{\"command\": \"cargo build\", \"background\": true}}"))),
    }
}

/// Argumente eines `run_command` im Chat prüfen: eine Zeile, Zeitlimit (Standard und Höchstwert aus `s`; ein zu großer
/// Wert wird auf den Höchstwert gesetzt, mit Hinweis), Hintergrund, Shell. `Ok((Anfrage, Korrekturen))`.
pub fn parse_run(args: &Value, s: &AgentToolSettings) -> Result<(RunRequest, Vec<String>), ToolErr> {
    let mut fixes = Vec::new();
    let command = str_arg(args, "command").unwrap_or("").trim().to_string();
    if command.is_empty() || command.contains('\n') || command.contains('\r') || command.len() > MAX_COMMAND_BYTES {
        return Err(err("bad_args", "command must be exactly one line (at most 4096 bytes). Example: {\"command\":\"cargo test\"}"));
    }
    let background = bool_arg(args, "background")?;
    let max = s.command_timeout_max_secs;
    let given = match args.get("timeout_secs") {
        None | Some(Value::Null) => None,
        Some(_) => Some(int_arg(args, "timeout_secs").ok_or_else(|| err("bad_args", "timeout_secs must be a whole number of seconds. Example: {\"command\": \"cargo test\", \"timeout_secs\": 600}"))?),
    };
    let timeout_secs = match given {
        _ if background => {
            if given.is_some() {
                fixes.push("timeout_secs is ignored for background commands – they run until they end or you stop them".to_string());
            }
            0
        }
        None => s.command_timeout_secs.min(max),
        Some(t) if t < 1 => {
            fixes.push(format!("timeout_secs: {t} → 1"));
            1
        }
        Some(t) if t as u64 > max => {
            fixes.push(format!("timeout_secs: {t} → {max} (the maximum set by the user); use background: true for longer commands"));
            max
        }
        Some(t) => t as u64,
    };
    let max_output_lines = match int_arg(args, "max_output_lines") {
        Some(n) if n >= 1 => (n as usize).min(MAX_KEPT_LINES),
        _ => 200,
    };
    let shell = match args.get("shell") {
        None | Some(Value::Null) => None,
        Some(Value::String(v)) => Some(v.trim().to_string()).filter(|v| !v.is_empty()),
        Some(_) => return Err(err("bad_args", "shell must be the name of one of the user's terminal profiles or shells, e.g. \"Git Bash\". Example: {\"command\": \"ls\", \"shell\": \"Git Bash\"}")),
    };
    Ok((RunRequest { command, background, timeout_secs, max_output_lines, shell }, fixes))
}

// ---------------------------------------------------------------------------------------------------------------
// Shell wählen

/// Ein Profil oder eine erkannte Shell, die ein Befehl des Chats nutzen könnte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCandidate {
    /// Kennung für die App: Shell-Kennung (`pwsh`, `gitbash`, `wsl:Ubuntu`) bzw. `profile:<id>`.
    pub key: String,
    /// Anzeigename (Name des Profils bzw. der Shell).
    pub label: String,
    /// Weitere Namen, unter denen sie gefunden wird (z. B. die Kennung der Shell).
    pub aliases: Vec<String>,
    pub profile: bool,
    /// Warum sie nie gilt: `Some(("admin_profile" | "forbidden", Grund))`.
    pub blocked: Option<(&'static str, &'static str)>,
}

impl ShellCandidate {
    /// Profil des Nutzers: gesperrt, wenn es als Administrator startet oder ein eigenes Programm, eigene Argumente oder
    /// einen Startbefehl hat.
    pub fn profile(id: u64, name: &str, admin: bool, custom: bool) -> Self {
        let blocked = if admin {
            Some(("admin_profile", "it starts as administrator, which agents may never use"))
        } else if custom {
            Some(("forbidden", "it has its own program, arguments or startup command, which agents may never start"))
        } else {
            None
        };
        ShellCandidate { key: format!("profile:{id}"), label: name.trim().to_string(), aliases: Vec::new(), profile: true, blocked }
    }
    /// Erkannte Shell (Kennung und Anzeigename).
    pub fn shell(id: &str, label: &str) -> Self {
        ShellCandidate { key: id.to_string(), label: label.trim().to_string(), aliases: vec![id.to_string()], profile: false, blocked: None }
    }
    fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.label.as_str()).chain(self.aliases.iter().map(String::as_str))
    }
}

/// Vergleichsform eines Namens: klein, ohne Leerzeichen, Binde- und Unterstriche.
fn norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect()
}

/// Namen, die das Modell nehmen kann (für Fehlermeldungen; ohne gesperrte).
pub fn available_names(cands: &[ShellCandidate]) -> String {
    let names: Vec<String> = cands.iter().filter(|c| c.blocked.is_none()).map(|c| format!("“{}”", c.label)).collect();
    if names.is_empty() { "(none)".into() } else { names.join(", ") }
}

/// Welche Shell meint `name`? Zuerst genaue Namen von Profilen, dann von Shells (Groß-/Kleinschreibung, Leerzeichen und
/// Bindestriche egal), dann ein eindeutiger Teil eines Namens. Heißt ein gesperrtes Profil wie eine erlaubte Shell, gilt
/// die Shell. Gesperrte Profile ergeben ihren Fehler, Unbekanntes `bad_args` mit den verfügbaren Namen.
/// `Ok(Index in cands)`.
pub fn resolve_shell(name: &str, cands: &[ShellCandidate]) -> Result<usize, ToolErr> {
    let want = norm(name);
    let pick = |i: usize| -> Result<usize, ToolErr> {
        match cands[i].blocked {
            Some((code, why)) => Err(err(code, format!("The profile “{}” cannot be used: {why}. Available: {}.", cands[i].label, available_names(cands)))),
            None => Ok(i),
        }
    };
    if want.is_empty() {
        return Err(err("bad_args", format!("shell is empty. Available: {}.", available_names(cands))));
    }
    // Genaue Treffer: ein erlaubtes Profil, sonst eine (erlaubte) Shell, erst dann der Fehler eines gesperrten Profils.
    let exact: Vec<usize> = (0..cands.len()).filter(|i| cands[*i].names().any(|n| norm(n) == want)).collect();
    let rules: [fn(&ShellCandidate) -> bool; 3] = [|c| c.profile && c.blocked.is_none(), |c| !c.profile && c.blocked.is_none(), |_| true];
    for ok in rules {
        if let Some(i) = exact.iter().copied().find(|i| ok(&cands[*i])) {
            return pick(i);
        }
    }
    let hits: Vec<usize> = (0..cands.len()).filter(|i| cands[*i].names().any(|n| norm(n).contains(&want))).collect();
    match hits.as_slice() {
        [i] => pick(*i),
        [] => Err(err("bad_args", format!("There is no profile or shell named “{}”. Available: {}.", crate::mcp::short(name, 60), available_names(cands)))),
        many => {
            let names: Vec<String> = many.iter().map(|i| format!("“{}”", cands[*i].label)).collect();
            Err(err("bad_args", format!("“{}” matches several profiles or shells: {}. Use the exact name.", crate::mcp::short(name, 60), names.join(", "))))
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Hintergrund-Befehle

/// Was die Hintergrund-Befehle vom Hub brauchen.
pub trait BgHost {
    /// Terminal für einen Hintergrund-Befehl des Chats `chat` öffnen (sichtbar, Kind des Chats, im Projektordner, mit der
    /// Shell aus `req.shell`) – oder eines aus `reuse` wiederverwenden (Terminals beendeter Befehle mit derselben Shell,
    /// wenn sie noch leben und an ihrem Prompt ruhen). `title` = Name für die Seitenleiste.
    /// `Ok((Sitzung, Shell zur Anzeige, Hinweis für das Modell))` – der Hinweis z. B., wenn statt des Projektprofils die
    /// Standard-Shell gilt.
    fn open_terminal(&mut self, chat: u64, req: &RunRequest, title: &str, reuse: &[u64]) -> Result<(u64, String, Option<String>), ToolErr>;
    /// Ist die Shell der Sitzung bereit (Shell-Integration gemeldet)? `None` = die Sitzung gibt es nicht mehr.
    fn terminal_ready(&self, session: u64) -> Option<bool>;
    /// Befehl eingeben (wie `run_command`, ohne Zeitlimit); das Ergebnis kommt über den Kanal, wenn er endet.
    fn run(&mut self, id: &str, chat: u64, session: u64, command: &str) -> Result<Receiver<ToolResult>, ToolErr>;
    /// Bisherige Ausgabe eines laufenden Befehls (Zeilen seit der Eingabe). `None` = nicht lesbar.
    fn live_output(&self, id: &str, session: u64, command: &str) -> Option<Vec<String>>;
    /// Strg+C an das Terminal.
    fn interrupt(&mut self, session: u64);
    /// Alle Prozesse des Terminals beenden (Job der Sitzung, wie „Alle beenden“) und es schließen – wenn Strg+C nicht
    /// reichte. Danach läuft nichts davon weiter.
    fn close_terminal(&mut self, session: u64);
    /// Ein Befehl ist zu Ende: Nachricht `text` (für das Modell) mit `event` (für die Anzeige) in den Chat.
    fn finished(&mut self, chat: u64, event: ChatEvent, text: String);
}

/// Ein Hintergrund-Befehl.
pub struct BgCommand {
    pub id: String,
    pub chat: u64,
    pub session: u64,
    pub command: String,
    /// Gewünschte Shell (wie verlangt, für die Wiederverwendung) und angezeigte Shell.
    pub shell_req: Option<String>,
    pub shell: String,
    pub state: BgState,
    pub exit_code: Option<i64>,
    /// Fehler (Code, Meldung) bei `Failed`.
    pub error: Option<(String, String)>,
    pub started: Instant,
    pub ended: Option<Instant>,
    /// Ausgabe nach dem Ende (höchstens [`MAX_KEPT_LINES`], das Ende) und ob vorne gekürzt wurde.
    pub output: Vec<String>,
    pub output_cut: bool,
    /// `quality` der Ausgabe (`exact`/`approx`/`heuristic`, wie `run_command`).
    pub quality: Option<String>,
    /// `stop_command` wurde aufgerufen (keine Nachricht beim Ende).
    pub stop_requested: bool,
    /// Terminal darf für den nächsten Befehl wiederverwendet werden (Prompt kam zurück).
    reusable: bool,
    ready_deadline: Instant,
    stop_deadline: Option<Instant>,
    rx: Option<Receiver<ToolResult>>,
}

impl BgCommand {
    fn duration(&self, now: Instant) -> Duration {
        self.ended.unwrap_or(now).saturating_duration_since(self.started)
    }

    /// Eintrag für `command_status`.
    pub fn status_json(&self, now: Instant) -> Value {
        let mut v = json!({
            "id": self.id,
            "command": self.command,
            "status": self.state.as_str(),
            "exit_code": self.exit_code,
            "duration_ms": self.duration(now).as_millis() as u64,
            "session": self.session,
            "shell": self.shell,
        });
        if !self.state.active() {
            v["output_lines"] = json!(self.output.len());
        }
        if self.stop_requested && self.state.active() {
            v["stopping"] = json!(true);
        }
        if let Some((code, msg)) = &self.error {
            v["error"] = json!({"code": code, "message": msg});
        }
        v
    }

    /// Ereignis für die Nachricht im Chat.
    pub fn event(&self, now: Instant) -> ChatEvent {
        ChatEvent {
            kind: ChatEvent::BACKGROUND_COMMAND.into(),
            id: self.id.clone(),
            command: self.command.clone(),
            state: self.state.as_str().into(),
            exit_code: self.exit_code,
            duration_ms: self.duration(now).as_millis() as u64,
            error: self.error.as_ref().map(|(c, _)| c.clone()),
        }
    }
}

/// Dauer für Menschen und Modell: `850 ms`, `12 s`, `3 min 5 s`, `1 h 2 min`.
pub fn human_duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0 => format!("{ms} ms"),
        1..=59 => format!("{s} s"),
        60..=3599 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// Text der Nachricht für das Modell, wenn ein Hintergrund-Befehl endet (Englisch; ohne Ausgabe – die ist nicht
/// vertrauenswürdig und gehört nicht in eine Nachricht mit der Rolle des Nutzers, das Modell liest sie mit
/// `command_output`).
pub fn event_text(e: &ChatEvent, lines: usize, error_msg: Option<&str>) -> String {
    let d = human_duration(e.duration_ms);
    let cmd = crate::mcp::short(&e.command, 200);
    match e.state.as_str() {
        "failed" => format!(
            "[Paneldeck] Background command {} (`{cmd}`) failed after {d}: {}{}. This message comes from Paneldeck, not from the user.",
            e.id,
            e.error.as_deref().unwrap_or("error"),
            error_msg.map(|m| format!(" – {}", crate::mcp::short(m, 200))).unwrap_or_default()
        ),
        _ => {
            let code = match e.exit_code {
                Some(c) => format!("exit code {c}"),
                None => "exit code unknown".into(),
            };
            format!(
                "[Paneldeck] Background command {} (`{cmd}`) finished after {d} with {code}; {lines} output lines. Read the output with command_output {{\"id\": \"{}\"}}. This message comes from Paneldeck, not from the user.",
                e.id, e.id
            )
        }
    }
}

/// Alle Hintergrund-Befehle des Hubs (je Chat eigene Kennungen).
#[derive(Default)]
pub struct BgCommands {
    items: Vec<BgCommand>,
    counters: HashMap<u64, u32>,
}

/// `id` aus den Argumenten: `"bg3"`, `"3"` oder `3`.
fn id_arg(args: &Value) -> Option<String> {
    match args.get("id")? {
        Value::Number(n) => n.as_u64().map(|n| format!("bg{n}")),
        Value::String(s) => {
            let t = s.trim().to_lowercase();
            let t = t.trim_start_matches('#');
            if t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty() { Some(format!("bg{t}")) } else { Some(t.to_string()) }
        }
        _ => None,
    }
}

impl BgCommands {
    /// Befehle des Chats (älteste zuerst).
    pub fn of_chat(&self, chat: u64) -> impl Iterator<Item = &BgCommand> {
        self.items.iter().filter(move |c| c.chat == chat)
    }

    /// Wie viele laufen im Chat gerade (auch startende)?
    pub fn active(&self, chat: u64) -> usize {
        self.of_chat(chat).filter(|c| c.state.active()).count()
    }

    /// Hat der Chat überhaupt Hintergrund-Befehle (dann bekommt das Modell die Werkzeuge dafür)?
    pub fn any(&self, chat: u64) -> bool {
        self.of_chat(chat).next().is_some()
    }

    /// Läuft (irgendwo) noch etwas? (Dann jedes Bild nachsehen.)
    pub fn busy(&self) -> bool {
        self.items.iter().any(|c| c.state.active())
    }

    /// (Sitzung, Kennung) aller laufenden Befehle.
    pub fn running_keys(&self) -> impl Iterator<Item = (u64, String)> + '_ {
        self.items.iter().filter(|c| c.state.active()).map(|c| (c.session, c.id.clone()))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self, chat: u64, id: &str) -> Option<&BgCommand> {
        self.items.iter().find(|c| c.chat == chat && c.id == id)
    }

    fn find(&self, chat: u64, args: &Value) -> Result<usize, ToolErr> {
        let known = || {
            let ids: Vec<String> = self.of_chat(chat).map(|c| c.id.clone()).collect();
            if ids.is_empty() { "This chat has no background commands; start one with run_command and background: true.".to_string() } else { format!("Known ids: {}.", ids.join(", ")) }
        };
        let Some(id) = id_arg(args) else { return Err(err("bad_args", format!("id is missing – the id that run_command returned, e.g. {{\"id\": \"bg1\"}}. {}", known()))) };
        self.items.iter().position(|c| c.chat == chat && c.id == id).ok_or_else(|| err("not_found", format!("There is no background command “{}” in this chat. {}", crate::mcp::short(&id, 40), known())))
    }

    /// Hintergrund-Befehl starten (nach Prüfung und Freigabe). Prüft die Grenze `limit`, öffnet das Terminal und
    /// antwortet sofort mit der Kennung; eingegeben wird der Befehl in [`BgCommands::poll`], sobald die Shell bereit ist.
    pub fn start(&mut self, host: &mut dyn BgHost, chat: u64, req: &RunRequest, limit: u32, fixes: &[String], now: Instant) -> Result<Value, ToolErr> {
        let limit = limit.clamp(MIN_LIMIT, MAX_LIMIT) as usize;
        if self.active(chat) >= limit {
            let running: Vec<String> = self.of_chat(chat).filter(|c| c.state.active()).map(|c| format!("{} (`{}`)", c.id, crate::mcp::short(&c.command, 60))).collect();
            return Err(err(
                "limit_reached",
                format!(
                    "At most {limit} background commands may run at the same time in this chat (set by the user in Settings → Models). Running: {}. Wait until one ends (a message comes) or stop one with stop_command.",
                    running.join(", ")
                ),
            ));
        }
        let n = self.counters.get(&chat).copied().unwrap_or(0) + 1;
        let id = format!("bg{n}");
        let key = |s: &Option<String>| s.as_deref().map(norm).filter(|k| !k.is_empty());
        let reuse: Vec<u64> = self.of_chat(chat).filter(|c| c.reusable && key(&c.shell_req) == key(&req.shell) && !c.state.active()).map(|c| c.session).collect();
        // Ein Terminal, das gerade ein anderer laufender Befehl nutzt, kommt nie in Frage.
        let busy: Vec<u64> = self.items.iter().filter(|c| c.state.active()).map(|c| c.session).collect();
        let reuse: Vec<u64> = reuse.into_iter().filter(|s| !busy.contains(s)).collect();
        let title = format!("{id}: {}", crate::mcp::short(&req.command, 40));
        let (session, shell, note) = host.open_terminal(chat, req, &title, &reuse)?;
        let fixes: Vec<String> = fixes.iter().cloned().chain(note).collect();
        self.counters.insert(chat, n);
        // Das Terminal gehört jetzt diesem Befehl (ein früherer Befehl darin ist beendet).
        for c in self.items.iter_mut().filter(|c| c.session == session) {
            c.reusable = false;
        }
        self.items.push(BgCommand {
            id: id.clone(),
            chat,
            session,
            command: req.command.clone(),
            shell_req: req.shell.clone(),
            shell: shell.clone(),
            state: BgState::Starting,
            exit_code: None,
            error: None,
            started: now,
            ended: None,
            output: Vec::new(),
            output_cut: false,
            quality: None,
            stop_requested: false,
            reusable: false,
            ready_deadline: now + READY_WAIT,
            stop_deadline: None,
            rx: None,
        });
        self.prune(chat);
        let mut v = json!({
            "id": id,
            "status": "running",
            "background": true,
            "session": session,
            "shell": shell,
            "note": "The command runs in the background in its own terminal below this chat. When it ends, a message from Paneldeck comes into this chat. Meanwhile use command_status, command_output and stop_command with this id – or go on with other work.",
        });
        if !fixes.is_empty() {
            v["corrected"] = json!(fixes);
        }
        Ok(v)
    }

    /// Beendete Befehle über [`KEEP_FINISHED`] hinaus vergessen (die ältesten).
    fn prune(&mut self, chat: u64) {
        let finished: Vec<usize> = self.items.iter().enumerate().filter(|(_, c)| c.chat == chat && !c.state.active()).map(|(i, _)| i).collect();
        if finished.len() > KEEP_FINISHED {
            let drop: Vec<usize> = finished[..finished.len() - KEEP_FINISHED].to_vec();
            let mut i = 0;
            self.items.retain(|_| {
                let keep = !drop.contains(&i);
                i += 1;
                keep
            });
        }
    }

    fn end(c: &mut BgCommand, state: BgState, now: Instant) {
        c.state = state;
        c.ended = Some(now);
        c.rx = None;
        c.stop_deadline = None;
    }

    /// Fertigen Befehl melden (außer nach `stop_command`).
    fn report(host: &mut dyn BgHost, c: &BgCommand, now: Instant) {
        if c.stop_requested {
            return;
        }
        let e = c.event(now);
        let text = event_text(&e, c.output.len(), c.error.as_ref().map(|(_, m)| m.as_str()));
        host.finished(c.chat, e, text);
    }

    /// Jedes Bild: startende Befehle eingeben, Ergebnisse abholen, Stopps nach der Frist durchsetzen. `true`, wenn sich
    /// etwas geändert hat.
    pub fn poll(&mut self, host: &mut dyn BgHost, now: Instant) -> bool {
        let mut changed = false;
        for c in self.items.iter_mut() {
            match c.state {
                BgState::Starting => match host.terminal_ready(c.session) {
                    None => {
                        c.error = Some(("session_closed".into(), "The command's terminal was closed before the command ran.".into()));
                        Self::end(c, BgState::Failed, now);
                        Self::report(host, c, now);
                        changed = true;
                    }
                    Some(ready) if ready || now >= c.ready_deadline => {
                        match host.run(&c.id, c.chat, c.session, &c.command) {
                            Ok(rx) => {
                                c.rx = Some(rx);
                                c.state = BgState::Running;
                            }
                            Err(e) => {
                                c.error = Some(e);
                                Self::end(c, BgState::Failed, now);
                                Self::report(host, c, now);
                            }
                        }
                        changed = true;
                    }
                    Some(_) => {}
                },
                BgState::Running => {
                    let got = match c.rx.as_ref().map(|rx| rx.try_recv()) {
                        Some(Ok(r)) => Some(r),
                        Some(Err(TryRecvError::Empty)) => None,
                        Some(Err(TryRecvError::Disconnected)) | None => Some(Err(("internal".to_string(), "The command ended without a result.".to_string()))),
                    };
                    match got {
                        Some(Ok(v)) => {
                            let text = v.get("output").and_then(Value::as_str).unwrap_or("");
                            let mut lines: Vec<String> = if text.is_empty() { Vec::new() } else { text.split('\n').map(str::to_string).collect() };
                            if lines.len() > MAX_KEPT_LINES {
                                lines = lines.split_off(lines.len() - MAX_KEPT_LINES);
                                c.output_cut = true;
                            }
                            c.output_cut |= v.get("truncated").and_then(Value::as_bool).unwrap_or(false);
                            c.output = lines;
                            c.exit_code = v.get("exit_code").and_then(Value::as_i64);
                            c.quality = v.get("quality").and_then(Value::as_str).map(str::to_string);
                            c.reusable = !v.get("session_exited").and_then(Value::as_bool).unwrap_or(false);
                            let st = if c.stop_requested { BgState::Stopped } else { BgState::Exited };
                            Self::end(c, st, now);
                            Self::report(host, c, now);
                            changed = true;
                        }
                        Some(Err(e)) => {
                            let st = if c.stop_requested { BgState::Stopped } else { BgState::Failed };
                            if st == BgState::Failed {
                                c.error = Some(e);
                            }
                            Self::end(c, st, now);
                            Self::report(host, c, now);
                            changed = true;
                        }
                        None => {
                            if c.stop_deadline.is_some_and(|d| now >= d) {
                                // Strg+C reichte nicht: Terminal schließen (beendet die Shell samt Befehl).
                                c.output = host.live_output(&c.id, c.session, &c.command).unwrap_or_default();
                                host.close_terminal(c.session);
                                Self::end(c, BgState::Stopped, now);
                                changed = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        changed
    }

    /// `command_status`: ein Befehl (`id`) oder alle des Chats.
    pub fn status(&self, chat: u64, args: &Value, now: Instant) -> Result<Value, ToolErr> {
        if args.get("id").is_none_or(Value::is_null) {
            let list: Vec<Value> = self.of_chat(chat).map(|c| c.status_json(now)).collect();
            return Ok(json!({"commands": list, "running": self.active(chat)}));
        }
        let i = self.find(chat, args)?;
        Ok(self.items[i].status_json(now))
    }

    /// `command_output`: Ausgabe seitenweise (`offset` ab 1, `limit`) oder das Ende (`tail`, Standard: die letzten
    /// [`DEFAULT_PAGE`] Zeilen); laufende Befehle lesen das Terminal. `max_bytes` = Platz für den Text.
    pub fn output(&self, host: &dyn BgHost, chat: u64, args: &Value, max_bytes: usize) -> Result<Value, ToolErr> {
        let i = self.find(chat, args)?;
        let c = &self.items[i];
        let live;
        let lines: &[String] = if c.state.active() {
            live = host.live_output(&c.id, c.session, &c.command).unwrap_or_default();
            &live
        } else {
            &c.output
        };
        let mut v = page(lines, args, max_bytes)?;
        v["id"] = json!(c.id);
        v["status"] = json!(c.state.as_str());
        v["finished"] = json!(!c.state.active());
        if c.output_cut && !c.state.active() {
            v["older_lines_dropped"] = json!(true);
        }
        if let Some(q) = &c.quality {
            v["quality"] = json!(q);
        }
        v["untrusted_note"] = json!(super::tools::UNTRUSTED_NOTE);
        Ok(v)
    }

    /// `stop_command`: Strg+C an das Terminal; endet der Befehl nicht binnen [`STOP_GRACE`], wird das Terminal
    /// geschlossen. Ein noch nicht eingegebener Befehl entfällt sofort.
    pub fn stop(&mut self, host: &mut dyn BgHost, chat: u64, args: &Value, now: Instant) -> Result<Value, ToolErr> {
        let i = self.find(chat, args)?;
        let c = &mut self.items[i];
        match c.state {
            BgState::Starting => {
                c.stop_requested = true;
                c.reusable = true;
                Self::end(c, BgState::Stopped, now);
                Ok(json!({"id": c.id, "status": "stopped", "note": "The command had not started yet; nothing ran."}))
            }
            BgState::Running => {
                if !c.stop_requested {
                    c.stop_requested = true;
                    host.interrupt(c.session);
                    c.stop_deadline = Some(now + STOP_GRACE);
                }
                Ok(json!({"id": c.id, "status": "stopping", "note": format!("Ctrl+C was sent. If the command does not end within {} s, its terminal is closed. No message follows; check command_status if needed.", STOP_GRACE.as_secs())}))
            }
            st => Ok(json!({"id": c.id, "status": st.as_str(), "note": "The command had already ended."})),
        }
    }

    /// Kennungen nach einem Neustart weiterzählen: der Chat hat schon Befehle bis `bg<n>` (aus dem Verlauf).
    pub fn seed(&mut self, chat: u64, n: u32) {
        let c = self.counters.entry(chat).or_insert(0);
        *c = (*c).max(n);
    }

    /// (Sitzung, Kennung) aller Befehle des Chats.
    pub fn keys_of(&self, chat: u64) -> Vec<(u64, String)> {
        self.of_chat(chat).map(|c| (c.session, c.id.clone())).collect()
    }

    /// Chat geschlossen: seine Befehle vergessen (die Terminals bleiben als normale Sitzungen).
    pub fn chat_closed(&mut self, chat: u64) {
        self.items.retain(|c| c.chat != chat);
        self.counters.remove(&chat);
    }
}

/// Zeilen für `command_output`: `tail` = die letzten N, sonst ab `offset` (1-basiert) `limit` Zeilen; ohne beides das
/// Ende ([`DEFAULT_PAGE`] Zeilen). Der Text hat höchstens `max_bytes` (0 = 12 KB); was nicht passt, steht in
/// `more_before`/`more_after` bzw. `next_offset`.
pub fn page(lines: &[String], args: &Value, max_bytes: usize) -> Result<Value, ToolErr> {
    let budget = if max_bytes == 0 { 12 * 1024 } else { max_bytes.saturating_sub(600).max(512) };
    let ex = "Example: {\"id\": \"bg1\", \"tail\": 50} or {\"id\": \"bg1\", \"offset\": 1, \"limit\": 200}";
    let num = |k: &str| -> Result<Option<i64>, ToolErr> {
        match args.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(_) => int_arg(args, k).map(Some).ok_or_else(|| err("bad_args", format!("{k} must be a whole number. {ex}"))),
        }
    };
    let (tail, offset, limit) = (num("tail")?, num("offset")?, num("limit")?);
    if tail.is_some() && offset.is_some() {
        return Err(err("bad_args", format!("Use either tail or offset, not both. {ex}")));
    }
    let total = lines.len();
    let cost = |l: &String| l.len() + 1;
    let (from, to) = match (tail, offset) {
        (_, Some(o)) => {
            let start = (o.max(1) as usize - 1).min(total);
            let n = limit.map_or(DEFAULT_PAGE, |l| (l.max(1) as usize).min(MAX_PAGE));
            let mut end = start;
            let mut used = 0;
            while end < total && end - start < n && (end == start || used + cost(&lines[end]) <= budget) {
                used += cost(&lines[end]);
                end += 1;
            }
            (start, end)
        }
        (t, None) => {
            let n = t.or(limit).map_or(DEFAULT_PAGE, |l| (l.max(1) as usize).min(MAX_PAGE));
            let mut start = total;
            let mut used = 0;
            while start > 0 && total - start < n && (start == total || used + cost(&lines[start - 1]) <= budget) {
                used += cost(&lines[start - 1]);
                start -= 1;
            }
            (start, total)
        }
    };
    let mut text = lines[from..to].join("\n");
    if text.len() > budget {
        // Eine einzelne Riesenzeile: kürzen (an einer Zeichengrenze).
        let mut cut = budget;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    let mut v = json!({
        "total_lines": total,
        "from": if to > from { from + 1 } else { from },
        "to": to,
        "text": text,
        "more_before": from > 0,
        "more_after": to < total,
    });
    if to < total {
        v["next_offset"] = json!(to + 1);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{sync_channel, SyncSender};

    fn settings() -> AgentToolSettings {
        AgentToolSettings::default()
    }

    #[test]
    fn foreground_timeouts_use_default_and_maximum() {
        let s = settings();
        let (r, f) = parse_run(&json!({"command": "cargo test"}), &s).unwrap();
        assert_eq!((r.timeout_secs, r.background, r.max_output_lines, r.shell.clone()), (120, false, 200, None));
        assert!(f.is_empty());
        let (r, f) = parse_run(&json!({"command": "x", "timeout_secs": 600}), &s).unwrap();
        assert_eq!(r.timeout_secs, 600);
        assert!(f.is_empty());
        let (r, f) = parse_run(&json!({"command": "x", "timeout_secs": "5000"}), &s).unwrap();
        assert_eq!(r.timeout_secs, DEFAULT_TIMEOUT_MAX, "höchstens 30 min");
        assert!(f[0].contains("5000 → 1800") && f[0].contains("background"), "{f:?}");
        let (r, _) = parse_run(&json!({"command": "x", "timeout_secs": 0}), &s).unwrap();
        assert_eq!(r.timeout_secs, 1);
        // Eigener Standard und Höchstwert
        let own = AgentToolSettings { command_timeout_secs: 300, command_timeout_max_secs: 240, ..settings() };
        assert_eq!(parse_run(&json!({"command": "x"}), &own).unwrap().0.timeout_secs, 240, "Standard nie über dem Höchstwert");
        assert_eq!(parse_run(&json!({"command": "x", "timeout_secs": 1000}), &own).unwrap().0.timeout_secs, 240);
        // Hintergrund: kein Zeitlimit
        let (r, f) = parse_run(&json!({"command": "npm run dev", "background": "true", "timeout_secs": 10, "shell": " Git Bash "}), &s).unwrap();
        assert!(r.background);
        assert_eq!((r.timeout_secs, r.shell.as_deref()), (0, Some("Git Bash")));
        assert!(f[0].contains("ignored"), "{f:?}");
        for bad in [json!({"command": "a\nb"}), json!({"command": ""}), json!({"command": "x", "background": 3}), json!({"command": "x", "timeout_secs": "soon"}), json!({"command": "x", "shell": 4})] {
            assert_eq!(parse_run(&bad, &s).unwrap_err().0, "bad_args", "{bad}");
        }
        assert!(parse_run(&json!({"command": "x".repeat(4097)}), &s).is_err());
    }

    #[test]
    fn shells_are_found_by_profile_or_shell_name_and_unsafe_profiles_never() {
        let c = vec![
            ShellCandidate::profile(1, "Dev", false, false),
            ShellCandidate::profile(2, "Admin PS", true, false),
            ShellCandidate::profile(3, "Node REPL", false, true),
            ShellCandidate::shell("pwsh", "PowerShell 7"),
            ShellCandidate::shell("powershell", "Windows PowerShell"),
            ShellCandidate::shell("gitbash", "Git Bash"),
            ShellCandidate::shell("wsl:Ubuntu", "WSL: Ubuntu"),
        ];
        assert_eq!(resolve_shell("dev", &c), Ok(0));
        assert_eq!(resolve_shell("git-bash", &c), Ok(5));
        assert_eq!(resolve_shell("GITBASH", &c), Ok(5));
        assert_eq!(resolve_shell("pwsh", &c), Ok(3));
        assert_eq!(resolve_shell("ubuntu", &c), Ok(6), "eindeutiger Teil");
        assert_eq!(resolve_shell("wsl", &c), Ok(6));
        // „powershell“ ist genau die Kennung von Windows PowerShell (kein Teil-Treffer bei PowerShell 7).
        assert_eq!(resolve_shell("powershell", &c), Ok(4));
        let (code, msg) = resolve_shell("Admin PS", &c).unwrap_err();
        assert_eq!(code, "admin_profile");
        assert!(msg.contains("administrator") && msg.contains("“Git Bash”") && !msg.contains("“Admin PS”,"), "{msg}");
        let (code, msg) = resolve_shell("node repl", &c).unwrap_err();
        assert_eq!(code, "forbidden");
        assert!(msg.contains("own program"), "{msg}");
        let (code, msg) = resolve_shell("fish", &c).unwrap_err();
        assert_eq!(code, "bad_args");
        assert!(msg.contains("no profile or shell named “fish”") && msg.contains("“Dev”") && msg.contains("“WSL: Ubuntu”"), "{msg}");
        let (code, msg) = resolve_shell("power", &c).unwrap_err();
        assert_eq!(code, "bad_args");
        assert!(msg.contains("several"), "{msg}");
        assert_eq!(resolve_shell(" ", &c).unwrap_err().0, "bad_args");
        // Ein Profil gewinnt vor einer gleichnamigen Shell – ein gesperrtes nicht (dann gilt die Shell).
        let c2 = vec![ShellCandidate::shell("gitbash", "Git Bash"), ShellCandidate::profile(9, "Git Bash", false, false)];
        assert_eq!(resolve_shell("git bash", &c2), Ok(1));
        let c3 = vec![ShellCandidate::profile(9, "PowerShell 7", true, false), ShellCandidate::shell("pwsh", "PowerShell 7")];
        assert_eq!(resolve_shell("PowerShell 7", &c3), Ok(1), "Administrator-Profil mit dem Namen einer Shell");
        let c4 = vec![ShellCandidate::profile(9, "Admin", true, false), ShellCandidate::shell("pwsh", "PowerShell 7")];
        assert_eq!(resolve_shell("admin", &c4).unwrap_err().0, "admin_profile", "ohne gleichnamige Shell: klarer Fehler");
    }

    #[test]
    fn output_pages_and_tails_within_the_budget() {
        let lines: Vec<String> = (1..=1000).map(|i| format!("line {i}")).collect();
        let v = page(&lines, &json!({}), 0).unwrap();
        assert_eq!((v["from"].as_u64(), v["to"].as_u64(), v["more_before"].as_bool(), v["more_after"].as_bool()), (Some(801), Some(1000), Some(true), Some(false)));
        assert!(v["text"].as_str().unwrap().ends_with("line 1000"));
        let v = page(&lines, &json!({"tail": 3}), 0).unwrap();
        assert_eq!(v["text"], "line 998\nline 999\nline 1000");
        let v = page(&lines, &json!({"offset": 10, "limit": 2}), 0).unwrap();
        assert_eq!(v["text"], "line 10\nline 11");
        assert_eq!(v["next_offset"], 12);
        let v = page(&lines, &json!({"offset": 2000}), 0).unwrap();
        assert_eq!((v["text"].as_str(), v["more_after"].as_bool()), (Some(""), Some(false)));
        // Knapper Platz: weniger Zeilen, dafür next_offset
        let v = page(&lines, &json!({"offset": 1, "limit": 2000}), 1200).unwrap();
        let to = v["to"].as_u64().unwrap();
        assert!(to > 10 && to < 200, "{to}");
        assert_eq!(v["next_offset"], to + 1);
        assert!(v["text"].as_str().unwrap().len() <= 1200);
        // Eine Riesenzeile wird gekürzt
        let huge = vec!["x".repeat(100_000)];
        let v = page(&huge, &json!({}), 4000).unwrap();
        assert!(v["text"].as_str().unwrap().len() < 4000);
        assert_eq!(page(&lines, &json!({"tail": 1, "offset": 1}), 0).unwrap_err().0, "bad_args");
        assert_eq!(page(&lines, &json!({"tail": "x"}), 0).unwrap_err().0, "bad_args");
        let v = page(&[], &json!({}), 0).unwrap();
        assert_eq!((v["total_lines"].as_u64(), v["from"].as_u64(), v["to"].as_u64()), (Some(0), Some(0), Some(0)));
    }

    /// Ersatz-Hub: Terminals sind Zahlen, ein „Befehl“ läuft, bis der Test sein Ergebnis über den Kanal schickt.
    #[derive(Default)]
    struct Fake {
        next: u64,
        ready: HashMap<u64, bool>,
        typed: Vec<(String, u64, String)>,
        senders: HashMap<String, SyncSender<ToolResult>>,
        live: HashMap<String, Vec<String>>,
        interrupted: Vec<u64>,
        closed: Vec<u64>,
        events: Vec<(u64, ChatEvent, String)>,
        reused: Vec<u64>,
        fail_open: bool,
    }

    impl BgHost for Fake {
        fn open_terminal(&mut self, _chat: u64, req: &RunRequest, _title: &str, reuse: &[u64]) -> Result<(u64, String, Option<String>), ToolErr> {
            if self.fail_open {
                return Err(err("terminal_failed", "no terminal"));
            }
            let shell = req.shell.clone().unwrap_or_else(|| "PowerShell".into());
            let note = (req.command == "noted").then(|| "default shell used".to_string());
            if let Some(s) = reuse.iter().copied().find(|s| self.ready.contains_key(s) && !self.closed.contains(s)) {
                self.reused.push(s);
                return Ok((s, shell, note));
            }
            self.next += 1;
            self.ready.insert(self.next, false);
            Ok((self.next, shell, note))
        }
        fn terminal_ready(&self, session: u64) -> Option<bool> {
            if self.closed.contains(&session) { None } else { self.ready.get(&session).copied() }
        }
        fn run(&mut self, id: &str, chat: u64, session: u64, command: &str) -> Result<Receiver<ToolResult>, ToolErr> {
            if command == "broken" {
                return Err(err("no_shell_integration", "no integration"));
            }
            self.typed.push((id.into(), session, command.into()));
            let (tx, rx) = sync_channel(1);
            self.senders.insert(format!("{chat}/{id}"), tx);
            Ok(rx)
        }
        fn live_output(&self, id: &str, _session: u64, _command: &str) -> Option<Vec<String>> {
            self.live.get(id).cloned()
        }
        fn interrupt(&mut self, session: u64) {
            self.interrupted.push(session);
        }
        fn close_terminal(&mut self, session: u64) {
            self.closed.push(session);
        }
        fn finished(&mut self, chat: u64, event: ChatEvent, text: String) {
            self.events.push((chat, event, text));
        }
    }

    fn req(cmd: &str) -> RunRequest {
        parse_run(&json!({"command": cmd, "background": true}), &settings()).unwrap().0
    }

    #[test]
    fn background_command_lifecycle_start_run_finish_message() {
        let mut h = Fake::default();
        let mut bg = BgCommands::default();
        let t0 = Instant::now();
        let v = bg.start(&mut h, 7, &req("cargo build"), 3, &[], t0).unwrap();
        assert_eq!((v["id"].as_str(), v["status"].as_str(), v["session"].as_u64()), (Some("bg1"), Some("running"), Some(1)));
        assert!(bg.any(7) && !bg.any(8));
        assert_eq!(bg.get(7, "bg1").unwrap().state, BgState::Starting);
        // Shell noch nicht bereit: nichts eingegeben
        assert!(!bg.poll(&mut h, t0));
        assert!(h.typed.is_empty());
        h.ready.insert(1, true);
        assert!(bg.poll(&mut h, t0 + Duration::from_millis(100)));
        assert_eq!(h.typed, [("bg1".to_string(), 1, "cargo build".to_string())]);
        assert_eq!(bg.get(7, "bg1").unwrap().state, BgState::Running);
        // Laufend: Status und Ausgabe aus dem Terminal
        h.live.insert("bg1".into(), vec!["Compiling a".into(), "Compiling b".into()]);
        let s = bg.status(7, &json!({"id": "bg1"}), t0 + Duration::from_secs(3)).unwrap();
        assert_eq!((s["status"].as_str(), s["exit_code"].clone(), s["duration_ms"].as_u64()), (Some("running"), Value::Null, Some(3000)));
        let o = bg.output(&h, 7, &json!({"id": 1}), 0).unwrap();
        assert_eq!((o["text"].as_str(), o["finished"].as_bool()), (Some("Compiling a\nCompiling b"), Some(false)));
        // Ende: Ergebnis kommt, Nachricht in den Chat
        h.senders["7/bg1"].send(Ok(json!({"output": "Compiling a\nCompiling b\nFinished", "exit_code": 0, "quality": "exact"}))).unwrap();
        assert!(bg.poll(&mut h, t0 + Duration::from_secs(65)));
        let c = bg.get(7, "bg1").unwrap();
        assert_eq!((c.state, c.exit_code, c.output.len()), (BgState::Exited, Some(0), 3));
        assert_eq!(h.events.len(), 1);
        let (chat, e, text) = &h.events[0];
        assert_eq!((*chat, e.id.as_str(), e.state.as_str(), e.exit_code, e.duration_ms), (7, "bg1", "exited", Some(0), 65_000));
        assert!(text.contains("bg1") && text.contains("`cargo build`") && text.contains("exit code 0") && text.contains("1 min 5 s") && text.contains("3 output lines") && text.contains("command_output"), "{text}");
        assert!(!text.contains("Finished"), "keine (nicht vertrauenswürdige) Ausgabe in der Nachricht");
        let o = bg.output(&h, 7, &json!({"id": "bg1", "tail": 1}), 0).unwrap();
        assert_eq!((o["text"].as_str(), o["finished"].as_bool(), o["quality"].as_str()), (Some("Finished"), Some(true), Some("exact")));
        let all = bg.status(7, &json!({}), t0).unwrap();
        assert_eq!((all["commands"].as_array().unwrap().len(), all["running"].as_u64()), (1, Some(0)));
        // Nächster Befehl mit derselben Shell nimmt das freie Terminal wieder
        let v = bg.start(&mut h, 7, &req("cargo test"), 3, &[], t0).unwrap();
        assert_eq!((v["id"].as_str(), v["session"].as_u64()), (Some("bg2"), Some(1)));
        assert_eq!(h.reused, [1]);
        // Chat zu: vergessen
        bg.chat_closed(7);
        assert!(!bg.any(7));
    }

    #[test]
    fn parallel_limit_per_chat() {
        let mut h = Fake::default();
        let mut bg = BgCommands::default();
        let t0 = Instant::now();
        for i in 0..2 {
            bg.start(&mut h, 1, &req(&format!("job {i}")), 2, &[], t0).unwrap();
        }
        let (code, msg) = bg.start(&mut h, 1, &req("third"), 2, &[], t0).unwrap_err();
        assert_eq!(code, "limit_reached");
        assert!(msg.contains("At most 2") && msg.contains("bg1 (`job 0`)") && msg.contains("bg2 (`job 1`)") && msg.contains("stop_command"), "{msg}");
        // Ein anderer Chat hat seine eigene Grenze und eigene Kennungen
        assert_eq!(bg.start(&mut h, 2, &req("other"), 2, &[], t0).unwrap()["id"], "bg1");
        // Grenze wird auf 1–10 begrenzt
        assert!(bg.start(&mut h, 3, &req("a"), 0, &[], t0).is_ok());
        assert_eq!(bg.start(&mut h, 3, &req("b"), 0, &[], t0).unwrap_err().0, "limit_reached");
        // Ist einer fertig, geht der nächste
        for s in h.ready.values_mut() {
            *s = true;
        }
        bg.poll(&mut h, t0);
        h.senders["1/bg1"].send(Ok(json!({"output": "", "exit_code": 1}))).unwrap();
        bg.poll(&mut h, t0);
        assert_eq!(bg.active(1), 1);
        assert_eq!(bg.start(&mut h, 1, &req("third"), 2, &[], t0).unwrap()["id"], "bg3");
        // Kein Terminal: Fehler, keine Kennung verbraucht
        h.fail_open = true;
        assert_eq!(bg.start(&mut h, 4, &req("x"), 3, &[], t0).unwrap_err().0, "terminal_failed");
        h.fail_open = false;
        assert_eq!(bg.start(&mut h, 4, &req("x"), 3, &[], t0).unwrap()["id"], "bg1");
    }

    #[test]
    fn stop_interrupts_then_closes_and_failures_are_reported() {
        let mut h = Fake::default();
        let mut bg = BgCommands::default();
        let t0 = Instant::now();
        bg.start(&mut h, 1, &req("npm run dev"), 3, &[], t0).unwrap();
        bg.start(&mut h, 1, &req("slow"), 3, &[], t0).unwrap();
        h.ready.insert(1, true);
        bg.poll(&mut h, t0);
        // bg2 wartet noch auf seine Shell: Stopp lässt ihn sofort entfallen
        let v = bg.stop(&mut h, 1, &json!({"id": "bg2"}), t0).unwrap();
        assert_eq!(v["status"], "stopped");
        // bg1 läuft: Strg+C, nach der Frist Terminal zu
        let v = bg.stop(&mut h, 1, &json!({"id": "bg1"}), t0).unwrap();
        assert_eq!(v["status"], "stopping");
        assert_eq!(h.interrupted, [1]);
        assert_eq!(bg.status(1, &json!({"id": "bg1"}), t0).unwrap()["stopping"], true);
        assert!(!bg.poll(&mut h, t0 + Duration::from_secs(1)));
        assert!(bg.poll(&mut h, t0 + STOP_GRACE + Duration::from_millis(1)));
        assert_eq!(h.closed, [1]);
        assert_eq!(bg.get(1, "bg1").unwrap().state, BgState::Stopped);
        assert!(h.events.is_empty(), "nach stop_command keine Nachricht");
        assert_eq!(bg.stop(&mut h, 1, &json!({"id": "bg1"}), t0).unwrap()["status"], "stopped");
        // Strg+C reicht: Ergebnis kommt, Stopped mit Exit-Code
        bg.start(&mut h, 1, &req("watch"), 3, &[], t0).unwrap();
        h.ready.insert(2, true);
        bg.poll(&mut h, t0);
        bg.stop(&mut h, 1, &json!({"id": "bg3"}), t0).unwrap();
        h.senders["1/bg3"].send(Ok(json!({"output": "^C", "exit_code": 130}))).unwrap();
        bg.poll(&mut h, t0);
        let c = bg.get(1, "bg3").unwrap();
        assert_eq!((c.state, c.exit_code), (BgState::Stopped, Some(130)));
        // Eingabe scheitert → failed mit Nachricht
        bg.start(&mut h, 1, &req("broken"), 3, &[], t0).unwrap();
        bg.poll(&mut h, t0);
        let c = bg.get(1, "bg4").unwrap();
        assert_eq!((c.state, c.error.as_ref().map(|e| e.0.as_str())), (BgState::Failed, Some("no_shell_integration")));
        let (_, e, text) = h.events.last().unwrap();
        assert_eq!(e.state, "failed");
        assert!(text.contains("failed") && text.contains("no_shell_integration"), "{text}");
        // Terminal verschwindet, bevor der Befehl lief
        bg.start(&mut h, 1, &req("gone"), 3, &[], t0).unwrap();
        let s = bg.get(1, "bg5").unwrap().session;
        h.closed.push(s);
        bg.poll(&mut h, t0);
        assert_eq!(bg.get(1, "bg5").unwrap().error.as_ref().unwrap().0, "session_closed");
        // Shell meldet sich nie: nach READY_WAIT wird trotzdem eingegeben (dann meldet run_command den Grund)
        bg.start(&mut h, 1, &req("late"), 3, &[], t0).unwrap();
        let before = h.typed.len();
        bg.poll(&mut h, t0 + Duration::from_secs(1));
        assert_eq!(h.typed.len(), before);
        bg.poll(&mut h, t0 + READY_WAIT);
        assert_eq!(h.typed.len(), before + 1);
        // Ergebnis-Fehler beim Laufen (Terminal geschlossen) → failed
        h.senders["1/bg6"].send(Err(("session_closed".into(), "The session was closed.".into()))).unwrap();
        bg.poll(&mut h, t0);
        assert_eq!(bg.get(1, "bg6").unwrap().state, BgState::Failed);
        // Unbekannte und fehlende Kennungen
        let (code, msg) = bg.status(1, &json!({"id": "bg99"}), t0).unwrap_err();
        assert_eq!(code, "not_found");
        assert!(msg.contains("bg1, bg2"), "{msg}");
        assert_eq!(bg.output(&h, 1, &json!({}), 0).unwrap_err().0, "bad_args");
        assert!(bg.stop(&mut h, 2, &json!({"id": "bg1"}), t0).unwrap_err().1.contains("no background commands"));
    }

    #[test]
    fn flags_shell_keys_and_ids_from_the_history() {
        for (v, want) in [(json!(true), true), (json!("yes"), true), (json!(1), true), (json!(false), false), (json!("false"), false), (json!(0), false), (json!(""), false), (Value::Null, false), (json!([1]), true)] {
            assert_eq!(flag_set(Some(&v)), want, "{v}");
        }
        assert!(!flag_set(None));
        assert_eq!(shell_key(&json!({"shell": " Git-Bash "})), Some("gitbash".into()));
        assert_eq!(shell_key(&json!({"shell": "  "})), None);
        assert!(shell_given(&json!({"shell": 3})) && !shell_given(&json!({"shell": ""})));
        use super::super::store::{ChatMsg, Role};
        let mut ev = ChatMsg::new(Role::System, "x");
        ev.event = Some(ChatEvent { kind: ChatEvent::BACKGROUND_COMMAND.into(), id: "bg4".into(), ..ChatEvent::default() });
        let mut tr = ChatMsg::new(Role::Tool, r#"{"id":"bg7","status":"running","background":true}"#);
        tr.tool_name = Some("run_command".into());
        let mut fg = ChatMsg::new(Role::Tool, r#"{"id":"bg99","output":"x"}"#);
        fg.tool_name = Some("run_command".into());
        assert_eq!(max_id_in(&[ev, tr, fg]), 7, "Vordergrund-Ergebnisse zählen nicht");
        assert_eq!(max_id_in(&[]), 0);
        // weiterzählen
        let mut h = Fake::default();
        let mut bg = BgCommands::default();
        bg.seed(5, 7);
        assert_eq!(bg.start(&mut h, 5, &req("x"), 3, &[], Instant::now()).unwrap()["id"], "bg8");
        // Hinweis des Hubs (z. B. Standard-Shell statt Projektprofil) landet in `corrected`
        let v = bg.start(&mut h, 5, &req("noted"), 3, &["a".into()], Instant::now()).unwrap();
        assert_eq!(v["corrected"], json!(["a", "default shell used"]));
        assert_eq!(bg.keys_of(5).len(), 2);
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(human_duration(850), "850 ms");
        assert_eq!(human_duration(12_400), "12 s");
        assert_eq!(human_duration(185_000), "3 min 5 s");
        assert_eq!(human_duration(3_720_000), "1 h 2 min");
    }
}
