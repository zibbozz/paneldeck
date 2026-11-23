//! Verlaufsdatei eines Chats: `chats/<id>.json` im Datenordner (öffentliches Datenformat ab 2.2.0).
//!
//! ```json
//! {
//!   "version": 1,
//!   "context_start": 2,
//!   "always_allow": [ { "tool": "send_text", "session": 4 }, { "tool": "run_command", "session": 7 } ],
//!   "messages": [
//!     { "role": "user", "content": "Which sessions are open?", "ts": 1763200000 },
//!     { "role": "assistant", "content": "", "ts": 1763200001,
//!       "tool_calls": [ { "id": "c1", "name": "list_sessions", "arguments": {} } ] },
//!     { "role": "tool", "content": "{\"sessions\":[]}", "ts": 1763200002,
//!       "tool_call_id": "c1", "tool_name": "list_sessions", "status": "ok" },
//!     { "role": "assistant", "content": "There are no sessions.", "ts": 1763200003 },
//!     { "role": "assistant", "content": "", "ts": 1763200004,
//!       "tool_calls": [ { "id": "c2", "name": "read_terminal", "arguments": { "session": 3, "lines": 50 } } ],
//!       "repairs": [ { "id": "c2", "text_call": true, "fixes": [ "lines: \"50\" → 50" ] } ] },
//!     { "role": "tool", "content": "{\"text\":\"…\"}", "ts": 1763200005,
//!       "tool_call_id": "c2", "tool_name": "read_terminal", "status": "ok" },
//!     { "role": "assistant", "content": "", "ts": 1763200010, "error": "connection refused" }
//!   ]
//! }
//! ```
//!
//! Lesen ist tolerant: BOM erlaubt, unbekannte Felder werden ignoriert, eine einzelne ungültige Nachricht wird
//! übersprungen; eine unlesbare oder zu große Datei ergibt einen leeren Chat mit Hinweis (die Datei wird vorher als
//! `<id>.defekt-<zeit>.json` gesichert). Geschrieben wird atomar (temporäre Datei, dann umbenennen).

use crate::state;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Aktuelle Version des Formats.
pub const VERSION: u32 = 1;
/// Größte Datei, die geladen wird; größere Verläufe kürzt das Speichern vorher (älteste Nachrichten zuerst).
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

/// Ergebnis eines Werkzeugaufrufs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Ok,
    Error,
    /// Vom Nutzer abgelehnt (oder durch die Einstellungen gesperrt).
    Denied,
}

/// Werkzeugaufruf, den das Modell in einer Antwort verlangt.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Parameter als JSON (meist ein Objekt).
    pub arguments: serde_json::Value,
}

/// Eine Nachricht im Verlauf.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatMsg {
    pub role: Role,
    pub content: String,
    /// Zeitpunkt (Sekunden seit 1970, UTC).
    pub ts: u64,
    /// Nur Antworten des Modells: verlangte Werkzeugaufrufe.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Nur Werkzeug-Nachrichten: zu welchem Aufruf das Ergebnis gehört.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolStatus>,
    /// Nur Antworten des Modells: Fehler, mit dem die Antwort abbrach (Anzeige mit „Wiederholen“).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Nur Antworten des Modells: Hinweis unter der Antwort, z. B. „an der Token-Grenze abgeschnitten“ (seit 2.2.0; wird
    /// nicht an das Modell gesendet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Nur Antworten des Modells: was Paneldeck an den Werkzeugaufrufen repariert hat (seit 2.6.1, #340; je Aufruf über
    /// `id`). Nur für die Anzeige, wird nicht an das Modell gesendet; Unlesbares ergibt eine leere Liste.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "lenient_repairs")]
    pub repairs: Vec<CallRepair>,
    /// Nur Nachrichten von Paneldeck (Rolle `system`, seit 3.1.0, #355): Ereignis, z. B. „Hintergrund-Befehl beendet“.
    /// `content` ist der Text für das Modell (geht als Nachricht des Nutzers mit); die Oberfläche zeigt eine eigene Zeile.
    /// Unlesbares ergibt `None` (ältere Versionen zeigen solche Nachrichten als Systemnachricht und senden sie nicht).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient_event")]
    pub event: Option<ChatEvent>,
}

/// Ereignis von Paneldeck im Verlauf (#355).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatEvent {
    /// Art: [`ChatEvent::BACKGROUND_COMMAND`].
    pub kind: String,
    /// Kennung des Befehls (`bg1`).
    pub id: String,
    pub command: String,
    /// `exited`, `stopped` oder `failed`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub duration_ms: u64,
    /// Fehlercode bei `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ChatEvent {
    pub const BACKGROUND_COMMAND: &'static str = "background_command";
}

fn lenient_event<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<ChatEvent>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<ChatEvent>(v).ok().filter(|e| !e.kind.is_empty()))
}

/// Reparatur an einem Werkzeugaufruf (#340): als Text geschriebener Aufruf erkannt und/oder Argumente normalisiert.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CallRepair {
    /// `ToolCall::id` des Aufrufs.
    pub id: String,
    /// Das Modell schrieb den Aufruf als Text (z. B. `<tool_call>{…}</tool_call>`); Paneldeck hat ihn erkannt.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub text_call: bool,
    /// Korrigierte Argumente in JSON-Schreibweise, z. B. `lines: "3" → 3` (sprachneutral).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<String>,
}

/// Reparaturen tolerant lesen: Unlesbares fällt weg, ein falscher Typ ergibt eine leere Liste (die Nachricht bleibt).
fn lenient_repairs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<CallRepair>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value::<CallRepair>(x.clone()).ok()).filter(|r| !r.id.is_empty()).collect()).unwrap_or_default())
}

impl ChatMsg {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        ChatMsg { role, content: content.into(), ts: super::now_secs(), ..ChatMsg::default() }
    }
}

/// Inhalt von `chats/<id>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatFile {
    pub version: u32,
    /// Index der ersten Nachricht, die das Modell bei der letzten Anfrage noch gesehen hat (ältere wurden wegen der
    /// Kontextlänge weggelassen). Fehlt, solange nichts gekürzt wurde.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_start: Option<usize>,
    /// Dauerhafte Freigaben „Immer für diesen Chat“ (seit 2.2.0): je Werkzeug und Ziel-Sitzung keine Nachfrage mehr.
    /// Steht vor `messages`, weil `serialize_capped` die Nachrichten ans Ende setzt. Regeln ohne Sitzung (`null`) werden
    /// gelesen, gelten aber nie und fallen beim Speichern weg.
    #[serde(skip_serializing_if = "no_session_rules", serialize_with = "session_rules", deserialize_with = "lenient_rules")]
    pub always_allow: Vec<AllowRule>,
    /// To-do-Liste des Chats (`update_todos`, seit 2.7.0, #338); oben im Chat angezeigt. Unlesbares ergibt eine leere
    /// Liste.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "lenient_todos")]
    pub todos: Vec<ChatTodo>,
    pub messages: Vec<ChatMsg>,
}

impl Default for ChatFile {
    fn default() -> Self {
        ChatFile { version: VERSION, context_start: None, always_allow: Vec::new(), todos: Vec::new(), messages: Vec::new() }
    }
}

/// Stand eines To-dos des Chats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }
}

/// Ein To-do des Chats (`update_todos`).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatTodo {
    pub content: String,
    pub status: TodoStatus,
}

/// To-dos tolerant lesen: Unlesbares fällt weg, ein falscher Typ ergibt eine leere Liste.
fn lenient_todos<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ChatTodo>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value::<ChatTodo>(x.clone()).ok()).filter(|t| !t.content.trim().is_empty()).collect()).unwrap_or_default())
}

/// Eine dauerhafte Freigabe: dieses Werkzeug für diese Ziel-Sitzung (`session` aus den Argumenten des Aufrufs). Nur
/// wenn beides passt, läuft ein Aufruf ohne Nachfrage. Regeln ohne Sitzung (`null`, aus Vorabständen) gelten nie –
/// außer für die Agenten-Werkzeuge des Chats (seit 2.7.0, #338): dort gilt eine Regel je Werkzeug für den ganzen
/// Chat, bei `run_command` im Terminal des Chats nur mit `prefix` (Befehle, die wortweise so beginnen).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AllowRule {
    pub tool: String,
    pub session: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Nur `run_command` mit `prefix` (seit 3.1.0, #355): die Regel gilt nur für diese Shell (Vergleichsform des Namens,
    /// `bg_commands::shell_key`; fehlt = das Profil des Projekts) …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// … und nur im Vordergrund bzw. nur im Hintergrund. Ältere Regeln (ohne beides) gelten nur im Vordergrund mit dem
    /// Profil des Projekts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

/// Werkzeuge, für die es nie „Immer für diesen Chat“ gibt (auch mit `session`-Argument nicht).
pub const NEVER_ALWAYS: &[&str] = &["create_session"];

impl AllowRule {
    /// Regel für einen Aufruf: Werkzeug + Ziel-Sitzung aus `arguments.session`; Agenten-Werkzeuge ohne Sitzung,
    /// `run_command` im Terminal des Chats mit dem Präfix des Befehls.
    pub fn for_call(call: &ToolCall) -> Self {
        if super::agent::is_local(call) {
            let rc = call.name == "run_command";
            let prefix = rc.then(|| call.arguments.get("command").and_then(|c| c.as_str()).and_then(super::agent::command_prefix)).flatten();
            let (shell, background) = if rc { (super::bg_commands::shell_key(&call.arguments), super::bg_commands::flag_set(call.arguments.get("background"))) } else { (None, false) };
            return AllowRule { tool: call.name.clone(), session: None, prefix, shell, background };
        }
        AllowRule { tool: call.name.clone(), session: call.arguments.get("session").and_then(|s| s.as_u64()), ..AllowRule::default() }
    }

    /// Darf es für diesen Aufruf „Immer für diesen Chat“ geben? Paneldeck-Werkzeuge nur mit einer Ziel-Sitzung (Zahl
    /// im Argument `session`) und nie für [`NEVER_ALWAYS`] – Werkzeuge ohne Ziel (Tickets, Todos, `create_session` …)
    /// fragen immer. Agenten-Werkzeuge: siehe `agent::always_eligible` (nie für sensible Dateien).
    pub fn eligible(call: &ToolCall) -> bool {
        if super::agent::is_local(call) {
            return super::agent::always_eligible(call);
        }
        call.arguments.get("session").is_some_and(|s| s.as_u64().is_some()) && !NEVER_ALWAYS.contains(&call.name.as_str())
    }

    pub fn matches(&self, call: &ToolCall) -> bool {
        if self.session.is_none() {
            // Regel eines Agenten-Werkzeugs (nur gültige; alte Regeln ohne Sitzung gelten weiter nie).
            if !self.valid() || self.tool != call.name || !super::agent::is_local(call) || !AllowRule::eligible(call) {
                return false;
            }
            return match &self.prefix {
                // Befehle: gleicher Anfang, dieselbe Shell und derselbe Modus (Vorder-/Hintergrund, #355 Review).
                Some(p) => {
                    let a = &call.arguments;
                    a.get("command").and_then(|c| c.as_str()).is_some_and(|c| super::agent::prefix_matches(p, c))
                        && self.shell == super::bg_commands::shell_key(a)
                        && self.background == super::bg_commands::flag_set(a.get("background"))
                }
                None => true,
            };
        }
        AllowRule::eligible(call) && *self == AllowRule::for_call(call)
    }

    /// Gilt die Regel überhaupt? Mit Sitzung immer; ohne nur für Agenten-Werkzeuge (außer `read_file`), bei
    /// `run_command` nur mit einem nicht leeren Präfix.
    pub fn valid(&self) -> bool {
        if self.session.is_some() {
            return self.prefix.is_none();
        }
        match self.tool.as_str() {
            "run_command" => self.prefix.as_deref().is_some_and(|p| !p.trim().is_empty()),
            "read_file" => false,
            t => super::agent::is_agent_tool(t) && self.prefix.is_none() && self.shell.is_none() && !self.background,
        }
    }
}

fn no_session_rules(v: &[AllowRule]) -> bool {
    !v.iter().any(AllowRule::valid)
}

/// Nur gültige Regeln schreiben (mit Sitzung oder gültige Regeln der Agenten-Werkzeuge).
fn session_rules<S: serde::Serializer>(v: &[AllowRule], s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let keep: Vec<&AllowRule> = v.iter().filter(|r| r.valid()).collect();
    let mut seq = s.serialize_seq(Some(keep.len()))?;
    for r in keep {
        seq.serialize_element(r)?;
    }
    seq.end()
}

/// Höchste Sitzungs-Id, auf die eine gespeicherte Regel zeigt (der Hub vergibt danach keine kleinere Id neu).
pub fn max_rule_session(file: &ChatFile) -> u64 {
    file.always_allow.iter().filter_map(|r| r.session).max().unwrap_or(0)
}

/// Freigaben tolerant lesen: `{"tool": …, "session": …}`; ein reiner Werkzeugname (Vorabstand) gilt als Regel ohne
/// Sitzung; Unlesbares fällt weg, ein falscher Typ ergibt eine leere Liste.
fn lenient_rules<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<AllowRule>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let one = |x: &serde_json::Value| -> Option<AllowRule> {
        if let Some(s) = x.as_str() {
            return Some(AllowRule { tool: s.to_string(), ..AllowRule::default() });
        }
        let tool = x.get("tool")?.as_str()?.to_string();
        let session = match x.get("session") {
            None | Some(serde_json::Value::Null) => None,
            Some(s) => Some(s.as_u64()?),
        };
        let prefix = match x.get("prefix") {
            None | Some(serde_json::Value::Null) => None,
            Some(p) => Some(p.as_str()?.to_string()),
        };
        let shell = match x.get("shell") {
            None | Some(serde_json::Value::Null) => None,
            Some(p) => Some(p.as_str()?.to_string()),
        };
        let background = match x.get("background") {
            None | Some(serde_json::Value::Null) => false,
            Some(b) => b.as_bool()?,
        };
        Some(AllowRule { tool, session, prefix, shell, background })
    };
    let mut out: Vec<AllowRule> = Vec::new();
    for r in v.as_array().map(|a| a.iter().filter_map(one).collect::<Vec<_>>()).unwrap_or_default() {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    Ok(out)
}

/// Ordner der Verlaufsdateien im Datenordner.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("chats")
}

/// Pfad der Verlaufsdatei eines Chats.
pub fn path(data_dir: &Path, id: u64) -> PathBuf {
    dir(data_dir).join(format!("{id}.json"))
}

/// Tolerant laden. Fehlt die Datei, ist der Chat einfach leer (ohne Hinweis); sonst gibt es bei Problemen einen
/// Hinweistext für die Oberfläche.
pub fn load(path: &Path) -> (ChatFile, Option<String>) {
    let Ok(meta) = std::fs::metadata(path) else { return (ChatFile::default(), None) };
    if meta.len() > MAX_BYTES {
        let bk = backup_note(path);
        return (ChatFile::default(), Some(format!("{} ({bk})", state::t("Chatverlauf ist zu groß und wurde nicht geladen", "Chat history is too large and was not loaded"))));
    }
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => return (ChatFile::default(), Some(format!("{}: {e}", state::t("Chatverlauf nicht lesbar", "Chat history could not be read")))),
    };
    parse(state::strip_bom(&text)).map_or_else(
        || {
            let bk = backup_note(path);
            (ChatFile::default(), Some(format!("{} ({bk})", state::t("Chatverlauf war beschädigt – leerer Chat", "Chat history was corrupt – empty chat"))))
        },
        |(file, skipped)| {
            let note = (skipped > 0).then(|| {
                let bk = backup_note(path);
                if state::lang() == state::Lang::De {
                    format!("{skipped} ungültige Nachricht(en) im Chatverlauf übersprungen ({bk})")
                } else {
                    format!("{skipped} invalid message(s) in the chat history skipped ({bk})")
                }
            });
            (file, note)
        },
    )
}

/// Text parsen: `None`, wenn es kein JSON-Objekt ist; sonst die Datei und die Zahl übersprungener Nachrichten.
pub fn parse(text: &str) -> Option<(ChatFile, usize)> {
    let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object_mut()?;
    let msgs = obj.remove("messages");
    let mut file = serde_json::from_value::<ChatFile>(serde_json::Value::Object(std::mem::take(obj))).unwrap_or_default();
    let mut skipped = 0;
    match msgs {
        Some(serde_json::Value::Array(items)) => {
            for it in items {
                match serde_json::from_value::<ChatMsg>(it) {
                    Ok(m) => file.messages.push(m),
                    Err(_) => skipped += 1,
                }
            }
        }
        None | Some(serde_json::Value::Null) => {}
        Some(_) => skipped += 1,
    }
    if file.context_start.is_some_and(|c| c > file.messages.len()) {
        file.context_start = None;
    }
    Some((file, skipped))
}

fn backup_note(path: &Path) -> String {
    match state::backup_corrupt(path) {
        Some(b) => format!("{} {}", state::t("Sicherung:", "backup:"), b.display()),
        None => state::t("keine Sicherung möglich", "no backup possible").to_string(),
    }
}

/// Atomar speichern (Ordner wird angelegt). Ein Verlauf über [`MAX_BYTES`] verliert vorher seine ältesten Nachrichten
/// (Systemnachrichten am Anfang bleiben), damit er beim nächsten Start wieder geladen wird.
pub fn save(path: &Path, file: &ChatFile) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let data = serialize_capped(file, MAX_BYTES as usize);
    state::write_atomic(path, &data)
}

/// JSON der Datei (kompakt), höchstens `cap` Bytes groß: jede Nachricht wird genau einmal serialisiert; reicht der Platz
/// nicht, fallen die ältesten Nachrichten nach den Systemnachrichten am Anfang weg (anhand der gemessenen Größen).
pub fn serialize_capped(file: &ChatFile, cap: usize) -> Vec<u8> {
    let parts: Vec<Vec<u8>> = file.messages.iter().map(|m| serde_json::to_vec(m).unwrap_or_else(|_| b"{}".to_vec())).collect();
    // Ereignisse (#355) sind keine Systemnachrichten am Anfang, die immer bleiben.
    let keep_from = file.messages.iter().take_while(|m| m.role == Role::System && m.event.is_none()).count();
    let size = |from: usize| -> usize { parts[..keep_from].iter().chain(&parts[from..]).map(|p| p.len() + 1).sum() };
    // Kopf ohne Nachrichten (+ Reserve für context_start): `{"version":1,"context_start":N,"always_allow":[…],"messages":[]}`
    let head_len = 64 + serde_json::to_vec(&file.always_allow).map(|v| v.len()).unwrap_or(0) + serde_json::to_vec(&file.todos).map(|v| v.len() + 10).unwrap_or(0) + 16;
    let mut drop_to = keep_from;
    if head_len + size(keep_from) > cap {
        let mut total = size(keep_from);
        while drop_to < parts.len() && head_len + total > cap {
            total -= parts[drop_to].len() + 1;
            drop_to += 1;
        }
    }
    let dropped = drop_to - keep_from;
    // Gekürzt: der Index rückt nach; lag er im weggefallenen Teil, sieht das Modell ohnehin alles Übrige.
    let context_start = if dropped == 0 { file.context_start } else { file.context_start.and_then(|c| (c >= drop_to).then(|| c - dropped)).filter(|c| *c > keep_from) };
    let head = ChatFile { version: file.version, context_start, always_allow: file.always_allow.clone(), todos: file.todos.clone(), messages: Vec::new() };
    let mut data = serde_json::to_vec(&head).unwrap_or_default();
    // `messages` ist das letzte Feld: `…"messages":[]}` → Nachrichten zwischen die Klammern setzen.
    data.truncate(data.len().saturating_sub(2));
    let mut first = true;
    for p in parts[..keep_from].iter().chain(&parts[drop_to..]) {
        if !first {
            data.push(b',');
        }
        first = false;
        data.extend_from_slice(p);
    }
    data.extend_from_slice(b"]}");
    data
}

/// Höchste Sitzungs-Id, zu der es im Datenordner noch Dateien gibt (`chats/<id>…`, `scrollback/<id>…`). Damit vergibt
/// der Hub nach einem Absturz oder einer zurückgesetzten `state.json` keine Id, deren alter Verlauf noch daliegt.
pub fn max_used_id(data_dir: &Path) -> u64 {
    let mut max = 0;
    for sub in ["chats", "scrollback"] {
        let Ok(rd) = std::fs::read_dir(data_dir.join(sub)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() < name.len() && name[digits.len()..].starts_with('.') {
                if let Ok(n) = digits.parse::<u64>() {
                    max = max.max(n);
                }
            }
        }
    }
    max
}

/// Fremden Verlauf unter dieser Id beiseitelegen (`<id>.orphan-<zeit>.json`), statt ihn zu laden oder zu überschreiben.
pub fn set_aside(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let secs = super::now_secs();
    let mut target = path.with_file_name(format!("{stem}.orphan-{secs}.json"));
    let mut n = 2;
    while target.exists() {
        target = path.with_file_name(format!("{stem}.orphan-{secs}-{n}.json"));
        n += 1;
    }
    std::fs::rename(path, &target).ok()?;
    Some(target)
}

/// Verlauf eines geschlossenen Chats löschen, samt seiner Sicherungen beschädigter Dateien (`<id>.defekt-*.json`).
pub fn remove(data_dir: &Path, id: u64) {
    let _ = std::fs::remove_file(path(data_dir, id));
    let prefix = format!("{id}.defekt-");
    if let Ok(rd) = std::fs::read_dir(dir(data_dir)) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pd-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn sample() -> ChatFile {
        let mut a = ChatMsg::new(Role::Assistant, "");
        a.tool_calls.push(ToolCall { id: "c1".into(), name: "list_sessions".into(), arguments: serde_json::json!({}) });
        let mut t = ChatMsg::new(Role::Tool, "{\"sessions\":[]}");
        t.tool_call_id = Some("c1".into());
        t.tool_name = Some("list_sessions".into());
        t.status = Some(ToolStatus::Ok);
        let mut e = ChatMsg::new(Role::Assistant, "");
        e.error = Some("connection refused".into());
        ChatFile { version: VERSION, context_start: Some(1), always_allow: vec![rule("send_text", Some(4))], todos: Vec::new(), messages: vec![ChatMsg::new(Role::User, "Hi ä 😀"), a, t, ChatMsg::new(Role::Assistant, "**done**"), e] }
    }

    #[test]
    fn always_allow_is_optional_lenient_and_survives_the_size_cap() {
        // Datei von 2.2.0-Vorabständen ohne das Feld
        let (f, skipped) = parse(r#"{"version":1,"messages":[{"role":"user","content":"x"}]}"#).unwrap();
        assert_eq!((f.always_allow.len(), skipped, f.messages.len()), (0, 0, 1));
        assert!(!String::from_utf8(serialize_capped(&f, 1000)).unwrap().contains("always_allow"), "leer: nicht geschrieben");
        // Falscher Typ / falsche Einträge: kein Verlust des Verlaufs
        let (f, _) = parse(r#"{"always_allow":"send_text","messages":[{"role":"user","content":"x"}]}"#).unwrap();
        assert!(f.always_allow.is_empty() && f.messages.len() == 1);
        // Altes Format (reine Namen) → Regeln ohne Sitzung; ungültige Einträge fallen weg, Doppelte auch
        let (f, _) = parse(r#"{"always_allow":["send_text",5,"run_command",{"tool":"send_keys","session":4},{"tool":"x","session":"4"},{"session":1},{"tool":"add_todo","session":null},{"tool":"send_keys","session":4}],"messages":[]}"#).unwrap();
        assert_eq!(f.always_allow, [rule("send_text", None), rule("run_command", None), rule("send_keys", Some(4)), rule("add_todo", None)]);
        // Geschrieben werden nur Regeln mit Sitzung (die ohne gelten nie)
        let text = String::from_utf8(serialize_capped(&f, 10_000)).unwrap();
        assert!(text.contains(r#""always_allow":[{"tool":"send_keys","session":4}]"#), "{text}");
        let only_null = ChatFile { always_allow: vec![rule("add_todo", None)], ..ChatFile::default() };
        assert!(!String::from_utf8(serialize_capped(&only_null, 1000)).unwrap().contains("always_allow"), "nur ungültige Regeln: Feld fehlt");
        assert_eq!(max_rule_session(&f), 4);
        assert_eq!(max_rule_session(&only_null), 0);
        // Gekürzt gespeichert: das Feld bleibt
        let mut big = sample();
        for i in 0..100 {
            big.messages.push(ChatMsg::new(Role::User, format!("{i}{}", "z".repeat(200))));
        }
        let data = serialize_capped(&big, 4_000);
        assert!(data.len() <= 4_000);
        let (back, _) = parse(std::str::from_utf8(&data).unwrap()).unwrap();
        assert_eq!(back.always_allow, [rule("send_text", Some(4))]);
    }

    #[test]
    fn repairs_are_optional_lenient_and_round_trip() {
        // Datei älterer Versionen ohne das Feld: gelesen, und ohne Reparaturen wird es nicht geschrieben
        let (f, skipped) = parse(r#"{"messages":[{"role":"assistant","content":"x","tool_calls":[{"id":"a","name":"whoami","arguments":{}}]}]}"#).unwrap();
        assert_eq!((skipped, f.messages[0].repairs.len()), (0, 0));
        assert!(!String::from_utf8(serialize_capped(&f, 10_000)).unwrap().contains("repairs"));
        // Unlesbares fällt weg, die Nachricht bleibt
        let (f, skipped) = parse(r#"{"messages":[{"role":"assistant","content":"x","repairs":"oops"},{"role":"assistant","content":"y","repairs":[5,{"id":"a","text_call":true},{"text_call":true},{"id":"b","fixes":["n: \"3\" → 3"],"future":1}]}]}"#).unwrap();
        assert_eq!((skipped, f.messages.len()), (0, 2));
        assert!(f.messages[0].repairs.is_empty());
        assert_eq!(f.messages[1].repairs, [CallRepair { id: "a".into(), text_call: true, fixes: vec![] }, CallRepair { id: "b".into(), text_call: false, fixes: vec!["n: \"3\" → 3".into()] }]);
        // Hin und zurück; leere Teile werden nicht geschrieben
        let text = String::from_utf8(serialize_capped(&f, 10_000)).unwrap();
        assert!(text.contains(r#""repairs":[{"id":"a","text_call":true},{"id":"b","fixes":["n: \"3\" → 3"]}]"#), "{text}");
        assert_eq!(parse(&text).unwrap().0, f);
    }

    fn rule(tool: &str, session: Option<u64>) -> AllowRule {
        AllowRule { tool: tool.into(), session, ..AllowRule::default() }
    }

    #[test]
    fn agent_tool_rules_are_per_tool_and_commands_per_prefix() {
        let call = |name: &str, args: serde_json::Value| ToolCall { id: "c".into(), name: name.into(), arguments: args };
        // Agenten-Werkzeug: eine Regel für den ganzen Chat
        let edit = call("edit_file", serde_json::json!({"path": "a.rs", "old_string": "x", "new_string": "y"}));
        assert!(AllowRule::eligible(&edit));
        let r = AllowRule::for_call(&edit);
        assert_eq!(r, rule("edit_file", None));
        assert!(r.valid() && r.matches(&call("edit_file", serde_json::json!({"path": "b.rs"}))));
        assert!(!r.matches(&call("write_file", serde_json::json!({"path": "b.rs"}))));
        // Sensible Dateien: nie „Immer“
        assert!(!AllowRule::eligible(&call("read_file", serde_json::json!({"path": ".env"}))));
        assert!(!rule("read_file", None).valid());
        // Befehle im Terminal des Chats: nur mit Präfix, wortweise, ohne Verkettung
        let cmd = |c: &str| call("run_command", serde_json::json!({"command": c}));
        let r = AllowRule::for_call(&cmd("cargo test --release"));
        assert_eq!(r, AllowRule { tool: "run_command".into(), session: None, prefix: Some("cargo test".into()), ..AllowRule::default() });
        assert!(r.matches(&cmd("cargo test")) && r.matches(&cmd("cargo test -p x")));
        assert!(!r.matches(&cmd("cargo build")) && !r.matches(&cmd("cargo test; del x")));
        assert!(!r.matches(&call("run_command", serde_json::json!({"command": "cargo test", "session": 4}))), "nicht für andere Sitzungen");
        // Alte Regel „run_command“ ohne Sitzung und Präfix gilt nie
        assert!(!rule("run_command", None).matches(&cmd("cargo test")));
        assert!(!rule("run_command", None).valid());
        // Speichern: gültige Regeln der Agenten-Werkzeuge bleiben, ungültige fallen weg
        let f = ChatFile { always_allow: vec![rule("edit_file", None), r.clone(), rule("send_text", None), rule("read_file", None)], ..ChatFile::default() };
        let text = String::from_utf8(serialize_capped(&f, 10_000)).unwrap();
        assert!(text.contains(r#""always_allow":[{"tool":"edit_file","session":null},{"tool":"run_command","session":null,"prefix":"cargo test"}]"#), "{text}");
        let (back, _) = parse(&text).unwrap();
        assert_eq!(back.always_allow, [rule("edit_file", None), r.clone()]);
        // #355 Review: Befehlsregeln gelten nur für dieselbe Shell und denselben Modus; Ersetzungen fragen immer.
        let with = |c: &str, extra: serde_json::Value| {
            let mut a = serde_json::json!({"command": c});
            for (k, v) in extra.as_object().unwrap() {
                a[k] = v.clone();
            }
            call("run_command", a)
        };
        assert!(!r.matches(&with("cargo test", serde_json::json!({"shell": "Git Bash"}))), "andere Shell");
        assert!(!r.matches(&with("cargo test", serde_json::json!({"background": true}))), "Hintergrund");
        assert!(r.matches(&with("cargo test", serde_json::json!({"background": "false", "shell": ""}))), "Standard");
        assert!(!r.matches(&cmd("cargo test (Remove-Item -Recurse -Force src)")), "PowerShell-Ausdruck");
        let gb = AllowRule::for_call(&with("cargo test", serde_json::json!({"shell": "Git Bash", "background": true})));
        assert_eq!((gb.shell.as_deref(), gb.background), (Some("gitbash"), true));
        assert!(gb.matches(&with("cargo test -q", serde_json::json!({"shell": "git-bash", "background": "yes"}))));
        assert!(!gb.matches(&with("cargo test", serde_json::json!({"shell": "Git Bash"}))), "Vordergrund");
        assert!(!gb.matches(&cmd("cargo test")));
        let f = ChatFile { always_allow: vec![gb.clone()], ..ChatFile::default() };
        let text = String::from_utf8(serialize_capped(&f, 10_000)).unwrap();
        assert!(text.contains(r#"{"tool":"run_command","session":null,"prefix":"cargo test","shell":"gitbash","background":true}"#), "{text}");
        assert_eq!(parse(&text).unwrap().0.always_allow, [gb]);
        // Regeln anderer Werkzeuge mit Shell-Angabe sind ungültig.
        assert!(!AllowRule { tool: "edit_file".into(), shell: Some("x".into()), ..AllowRule::default() }.valid());
    }

    #[test]
    fn todos_are_optional_lenient_and_round_trip() {
        let (f, _) = parse(r#"{"messages":[]}"#).unwrap();
        assert!(f.todos.is_empty());
        assert!(!String::from_utf8(serialize_capped(&f, 1000)).unwrap().contains("todos"));
        let (f, skipped) = parse(r#"{"todos":[{"content":"a","status":"in_progress"},5,{"content":"","status":"pending"},{"content":"b","status":"weird"},{"content":"c"}],"messages":[{"role":"user","content":"x"}]}"#).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(f.todos, [ChatTodo { content: "a".into(), status: TodoStatus::InProgress }, ChatTodo { content: "c".into(), status: TodoStatus::Pending }]);
        let text = String::from_utf8(serialize_capped(&f, 10_000)).unwrap();
        assert!(text.contains(r#""todos":[{"content":"a","status":"in_progress"},{"content":"c","status":"pending"}]"#), "{text}");
        assert_eq!(parse(&text).unwrap().0, f);
        let (f, _) = parse(r#"{"todos":"x","messages":[]}"#).unwrap();
        assert!(f.todos.is_empty());
    }

    #[test]
    fn allow_rules_match_tool_and_target() {
        let call = |name: &str, args: serde_json::Value| ToolCall { id: "c".into(), name: name.into(), arguments: args };
        let r = AllowRule::for_call(&call("send_text", serde_json::json!({"session": 4, "text": "x"})));
        assert_eq!(r, rule("send_text", Some(4)));
        assert!(r.matches(&call("send_text", serde_json::json!({"session": 4, "text": "other"}))));
        assert!(!r.matches(&call("send_text", serde_json::json!({"session": 5}))), "andere Sitzung");
        assert!(!r.matches(&call("send_keys", serde_json::json!({"session": 4}))), "anderes Werkzeug");
        // Ohne Ziel-Sitzung nie „Immer“: Regel ohne Sitzung passt nicht, create_session nie
        assert!(!rule("add_todo", None).matches(&call("add_todo", serde_json::json!({"title": "t"}))));
        assert!(!AllowRule::eligible(&call("add_todo", serde_json::json!({"title": "t"}))));
        assert!(!AllowRule::eligible(&call("send_text", serde_json::json!({"session": "4"}))), "keine Zahl");
        assert!(!AllowRule::eligible(&call("create_session", serde_json::json!({"session": 4}))));
        assert!(!rule("create_session", Some(4)).matches(&call("create_session", serde_json::json!({"session": 4}))));
        assert!(AllowRule::eligible(&call("send_keys", serde_json::json!({"session": 4}))));
    }

    #[test]
    fn roundtrip_atomic_without_temp_file() {
        let d = tmp("rt");
        let p = path(&d, 5);
        let f = sample();
        save(&p, &f).unwrap();
        assert!(!dir(&d).join("5.json.tmp").exists());
        let (back, note) = load(&p);
        assert!(note.is_none(), "{note:?}");
        assert_eq!(back, f);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("\"version\":1"));
        assert!(text.contains("\"status\":\"ok\""));
        // Leere optionale Felder werden nicht geschrieben
        assert!(!text.contains("\"tool_calls\":[]"));
        // Ohne context_start und ohne Nachrichten bleibt es gültiges JSON
        let (empty, _) = parse(std::str::from_utf8(&serialize_capped(&ChatFile::default(), 1000)).unwrap()).unwrap();
        assert_eq!(empty, ChatFile::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_is_an_empty_chat_without_warning() {
        let (f, note) = load(&tmp("none").join("1.json"));
        assert!(f.messages.is_empty() && note.is_none());
        assert_eq!(f.version, VERSION);
    }

    #[test]
    fn bom_and_unknown_fields_are_fine() {
        let d = tmp("bom");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("2.json");
        std::fs::write(&p, "\u{feff}{\"version\":1,\"future\":true,\"messages\":[{\"role\":\"user\",\"content\":\"x\",\"ts\":1,\"extra\":[1]}]}").unwrap();
        let (f, note) = load(&p);
        assert!(note.is_none());
        assert_eq!(f.messages.len(), 1);
        assert_eq!(f.messages[0].content, "x");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn broken_file_gives_empty_chat_warning_and_backup() {
        let d = tmp("broken");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("3.json");
        std::fs::write(&p, "{ not json").unwrap();
        let (f, note) = load(&p);
        assert!(f.messages.is_empty());
        assert!(note.is_some());
        let backups = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("3.defekt-")).count();
        assert_eq!(backups, 1, "beschädigte Datei wird gesichert");
        // Einzelne ungültige Nachricht: der Rest bleibt
        std::fs::write(&p, r#"{"messages":[{"role":"user","content":"a"},{"role":"wizard","content":"b"},7,{"role":"assistant","content":"c"}],"context_start":99}"#).unwrap();
        let (f, note) = load(&p);
        assert_eq!(f.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(note.unwrap().contains('2'));
        assert_eq!(f.context_start, None, "Index hinter dem Ende wird verworfen");
        // Kein Objekt
        assert!(parse("[1,2]").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn size_cap_on_load_and_save() {
        let d = tmp("cap");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("4.json");
        // Zu groß zum Laden
        let big = format!("{{\"messages\":[{{\"role\":\"user\",\"content\":\"{}\"}}]}}", "x".repeat(MAX_BYTES as usize + 10));
        std::fs::write(&p, big).unwrap();
        let (f, note) = load(&p);
        assert!(f.messages.is_empty() && note.is_some());
        // Speichern kürzt auf die Grenze, Systemnachricht bleibt
        let mut file = ChatFile::default();
        file.messages.push(ChatMsg::new(Role::System, "sys"));
        for i in 0..200 {
            file.messages.push(ChatMsg::new(Role::User, format!("{i}-{}", "y".repeat(100))));
        }
        file.context_start = Some(150);
        let data = serialize_capped(&file, 8_000);
        assert!(data.len() <= 8_000);
        let (back, _) = parse(std::str::from_utf8(&data).unwrap()).unwrap();
        assert_eq!(back.messages[0].role, Role::System);
        assert!(back.messages.len() < 201);
        assert!(back.messages.last().unwrap().content.starts_with("199-"), "die neuesten bleiben");
        assert!(data.len() > 6_000, "nicht mehr als nötig gekürzt");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn used_ids_orphans_and_removal() {
        let d = tmp("ids");
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::create_dir_all(d.join("scrollback")).unwrap();
        assert_eq!(max_used_id(&tmp("ids-none")), 0);
        std::fs::write(path(&d, 12), "{}").unwrap();
        std::fs::write(dir(&d).join("40.defekt-1.json"), "x").unwrap();
        std::fs::write(d.join("scrollback").join("77.txt"), "x").unwrap();
        std::fs::write(d.join("scrollback").join("notes.txt"), "x").unwrap();
        std::fs::write(d.join("scrollback").join("999"), "x").unwrap();
        assert_eq!(max_used_id(&d), 77);
        // Beiseitelegen: Datei weg, Inhalt in der Sicherung
        let b = set_aside(&path(&d, 12)).unwrap();
        assert!(!path(&d, 12).exists());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "{}");
        assert!(set_aside(&path(&d, 12)).is_none());
        // Löschen: Verlauf und Sicherungen beschädigter Dateien, nicht beiseitegelegte fremde Verläufe
        std::fs::write(path(&d, 40), "{}").unwrap();
        remove(&d, 40);
        assert!(!path(&d, 40).exists() && !dir(&d).join("40.defekt-1.json").exists());
        assert!(b.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Nachrichten von Paneldeck (#355): `event` kommt mit, Unlesbares fällt weg, ohne `event` wird nichts geschrieben;
    /// eine Ereignis-Nachricht am Anfang zählt beim Kürzen nicht als Systemnachricht.
    #[test]
    fn events_round_trip_and_are_read_leniently() {
        let mut m = ChatMsg::new(Role::System, "[Paneldeck] Background command bg1 finished");
        m.event = Some(ChatEvent { kind: ChatEvent::BACKGROUND_COMMAND.into(), id: "bg1".into(), command: "cargo build".into(), state: "exited".into(), exit_code: Some(0), duration_ms: 1500, error: None });
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["event"]["kind"], "background_command");
        assert!(j["event"].get("error").is_none());
        let back: ChatMsg = serde_json::from_value(j).unwrap();
        assert_eq!(back, m);
        let bad: ChatMsg = serde_json::from_value(serde_json::json!({"role": "system", "content": "x", "event": 5})).unwrap();
        assert_eq!(bad.event, None);
        let plain = serde_json::to_value(ChatMsg::new(Role::User, "hi")).unwrap();
        assert!(plain.get("event").is_none());
        let mut f = ChatFile::default();
        f.messages.push(m);
        for i in 0..50 {
            f.messages.push(ChatMsg::new(Role::User, format!("message {i} {}", "x".repeat(200))));
        }
        let data = serialize_capped(&f, 4000);
        let back: ChatFile = serde_json::from_slice(&data).unwrap();
        assert!(back.messages.first().is_some_and(|m| m.event.is_none()), "das alte Ereignis fällt beim Kürzen weg");
    }
}
