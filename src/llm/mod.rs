//! Anbindung lokaler LLM-Server (Ollama und OpenAI-kompatibel: LM Studio, llama.cpp-Server, vLLM).
//!
//! Kern ohne egui und ohne App: HTTP (`http`), Host-Regel (`net`), Adapter (`ollama`, `openai`), Datenstrom-Ereignisse,
//! DPAPI für API-Schlüssel (`secret`) und Token-Schätzung (`tokens`). Die Oberfläche (Chat-Sitzungen, Modelle in den
//! Einstellungen) baut darauf auf.

pub mod config;
pub mod error;
pub mod http;
pub mod net;
mod ollama;
mod openai;
pub mod secret;
pub mod tokens;

#[allow(unused_imports)]
pub use error::{LlmError, UrlProblem};
pub use http::{DEFAULT_RECEIVE_TIMEOUT, Transport, system_transport};
#[allow(unused_imports)]
pub use tokens::{estimate_tokens, trim_to_budget};

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// Kürzeste erlaubte Wartezeit auf Daten (auch wenn ein kleinerer Wert eingestellt ist).
pub const MIN_RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Höchstens so viele Werkzeugaufrufe je Antwort, so lange Argumente je Aufruf und so viel Text je Antwort (sonst `TooLarge`).
pub const MAX_TOOL_CALLS: usize = 128;
pub const MAX_TOOL_ARGS: usize = 1 << 20;
pub const MAX_CONTENT: usize = 16 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    #[default]
    Ollama,
    /// OpenAI-kompatibel (`/v1/models`, `/v1/chat/completions`).
    #[serde(rename = "openai")]
    OpenAi,
}

/// Ein Server: Typ, Basisadresse (z. B. `http://localhost:11434`, bei OpenAI auch mit `/v1`), optionaler Klartext-Schlüssel
/// (nur im Speicher; gespeichert wird er über `secret::protect`). `Debug` zeigt den Schlüssel nicht.
#[derive(Clone, PartialEq)]
pub struct Endpoint {
    pub kind: ServerKind,
    pub base_url: String,
    pub api_key: Option<String>,
    /// Wartezeit auf Daten (auch auf das erste Token); Standard 300 s.
    pub receive_timeout: Duration,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("receive_timeout", &self.receive_timeout)
            .finish()
    }
}

impl Endpoint {
    pub fn new(kind: ServerKind, base_url: &str) -> Self {
        Self { kind, base_url: base_url.trim().to_string(), api_key: None, receive_timeout: DEFAULT_RECEIVE_TIMEOUT }
    }

    /// Wartezeit auf Daten; mindestens `MIN_RECEIVE_TIMEOUT`.
    pub fn with_receive_timeout(mut self, d: Duration) -> Self {
        self.receive_timeout = d.max(MIN_RECEIVE_TIMEOUT);
        self
    }

    pub fn with_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_string());
        self
    }

    /// Volle Adresse eines API-Pfads (`/api/tags`, `/v1/models` …). Bei OpenAI darf die Basis schon auf `/v1` enden.
    pub fn api_url(&self, path: &str) -> Result<String, LlmError> {
        let mut u = net::parse(&self.base_url)?;
        if self.kind == ServerKind::OpenAi && u.path.ends_with("/v1") {
            u.path.truncate(u.path.len() - 3);
        }
        Ok(u.join(path))
    }

    pub(crate) fn request<'a>(&'a self, method: http::Method, url: &'a str, body: Option<&'a [u8]>, accept: &'a str) -> http::HttpRequest<'a> {
        http::HttpRequest { method, url, body, bearer: self.api_key.as_deref(), accept, receive_timeout: self.receive_timeout.max(MIN_RECEIVE_TIMEOUT) }
    }
}

/// Ein Modell des Servers. `tools`/`context_length` = `None`, wenn der Server es nicht verrät (OpenAI-kompatibel immer).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub tools: Option<bool>,
    pub context_length: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

/// Werkzeugaufruf des Modells. `arguments` ist ein JSON-Objekt; konnte der Text des Modells nicht als JSON gelesen werden,
/// steht er unverändert als `Value::String` darin (die Werkzeug-Schleife meldet das dann dem Modell zurück).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Nachricht im Verlauf (eigenes Format, unabhängig vom Server; die Adapter übersetzen).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Nur bei `Assistant`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Nur bei `Tool`: Bezug auf `ToolCall::id` (OpenAI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Nur bei `Tool`: Name des Werkzeugs (Ollama).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self { role, content: content.into(), ..Default::default() }
    }

    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Self { role: Role::Tool, content: content.into(), tool_call_id: Some(call.id.clone()), tool_name: Some(call.name.clone()), ..Default::default() }
    }
}

/// Beschreibung eines Werkzeugs für das Modell (`parameters` = JSON-Schema).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    /// Kontextlänge (nur Ollama: `options.num_ctx`; OpenAI-kompatible Server legen sie beim Start fest).
    pub num_ctx: Option<u32>,
    /// Höchstzahl Tokens der Antwort (nur Ollama: `options.num_predict`), damit die Antwort die Anfrage nicht aus dem
    /// Kontext drängt.
    pub num_predict: Option<u32>,
}

/// Ereignisse eines Chat-Aufrufs. Am Ende kommt genau eines von `Done` oder `Error`; `ToolCalls` (falls vorhanden) direkt
/// vor `Done`. `finish_reason` = Grund des Servers (OpenAI `finish_reason`, Ollama `done_reason`), z. B. `"stop"`, `"length"`
/// (an der Token-Grenze abgeschnitten) oder `"tool_calls"`; `None`, wenn der Server keinen nennt.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Delta(String),
    ToolCalls(Vec<ToolCall>),
    Done { prompt_tokens: Option<u32>, completion_tokens: Option<u32>, finish_reason: Option<String> },
    Error(LlmError),
}

/// Liest die Modelle des Servers (Ollama: mit Werkzeug-Fähigkeit und Kontextlänge aus `/api/show`).
pub fn list_models(t: &dyn Transport, ep: &Endpoint, cancel: &Arc<AtomicBool>) -> Result<Vec<ModelInfo>, LlmError> {
    match ep.kind {
        ServerKind::Ollama => ollama::list_models(t, ep, cancel),
        ServerKind::OpenAi => openai::list_models(t, ep, cancel),
    }
}

/// Zustand beim Lesen des Datenstroms, gemeinsam für beide Adapter.
#[derive(Default)]
pub(crate) struct Acc {
    pub valid: usize,
    pub malformed: usize,
    pub finished: bool,
    pub error: Option<LlmError>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub content_bytes: usize,
}

impl Acc {
    /// Text weitergeben (mit Obergrenze für die ganze Antwort).
    pub fn delta(&mut self, s: &str, on: &mut dyn FnMut(StreamEvent)) {
        if s.is_empty() {
            return;
        }
        self.content_bytes += s.len();
        if self.content_bytes > MAX_CONTENT {
            self.error = Some(LlmError::TooLarge);
            return;
        }
        on(StreamEvent::Delta(s.to_string()));
    }

    /// Werkzeugaufruf aufnehmen (mit Obergrenzen für Anzahl und Länge der Argumente).
    pub fn push_call(&mut self, c: ToolCall) {
        let len = match &c.arguments {
            serde_json::Value::String(s) => s.len(),
            v => v.to_string().len(),
        };
        if self.tool_calls.len() >= MAX_TOOL_CALLS || len > MAX_TOOL_ARGS {
            self.error = Some(LlmError::TooLarge);
            return;
        }
        self.tool_calls.push(c);
    }

    /// Schluss-Ereignisse nach dem Strom.
    fn finish(mut self, on: &mut dyn FnMut(StreamEvent)) {
        if let Some(e) = self.error.take() {
            return on(StreamEvent::Error(e));
        }
        if self.valid == 0 {
            let d = if self.malformed > 0 { format!("{} unreadable lines, no valid data", self.malformed) } else { "empty answer".into() };
            return on(StreamEvent::Error(LlmError::BadResponse(d)));
        }
        // Ohne Ende-Kennung (`done`, `[DONE]`, `finish_reason`) ist die Antwort unvollständig.
        if !self.finished && self.finish_reason.is_none() {
            return on(StreamEvent::Error(LlmError::BadResponse("stream ended early".into())));
        }
        if !self.tool_calls.is_empty() {
            on(StreamEvent::ToolCalls(std::mem::take(&mut self.tool_calls)));
        }
        on(StreamEvent::Done { prompt_tokens: self.prompt_tokens, completion_tokens: self.completion_tokens, finish_reason: self.finish_reason });
    }
}

pub(crate) fn as_u32(v: Option<&serde_json::Value>) -> Option<u32> {
    v.and_then(|v| v.as_u64()).map(|n| n.min(u32::MAX as u64) as u32)
}

/// Ein Chat-Aufruf mit Datenstrom. Meldet Text, Werkzeugaufrufe und das Ende über `on` (blockiert bis zum Ende).
/// Unlesbare Zeilen werden übersprungen; kommt gar nichts Gültiges, endet es mit `Error(BadResponse)`.
pub fn chat(t: &dyn Transport, ep: &Endpoint, req: &ChatRequest, cancel: &Arc<AtomicBool>, on: &mut dyn FnMut(StreamEvent)) {
    let (path, body, accept) = match ep.kind {
        ServerKind::Ollama => ("/api/chat", ollama::request_json(req), "application/x-ndjson, application/json"),
        ServerKind::OpenAi => ("/v1/chat/completions", openai::request_json(req), "text/event-stream, application/json"),
    };
    let url = match ep.api_url(path) {
        Ok(u) => u,
        Err(e) => return on(StreamEvent::Error(e)),
    };
    let body = body.to_string().into_bytes();
    let mut r = match http::send_ok(t, &ep.request(http::Method::Post, &url, Some(&body), accept), cancel) {
        Ok(r) => r,
        Err(e) => return on(StreamEvent::Error(if cancel.load(Ordering::Relaxed) { LlmError::Cancelled } else { e })),
    };
    let mut acc = Acc::default();
    let mut oa = openai::Stream::default();
    let res = http::read_lines(r.as_mut(), cancel, &mut |line| {
        match ep.kind {
            ServerKind::Ollama => ollama::line(line, &mut acc, on),
            ServerKind::OpenAi => oa.line(line, &mut acc, on),
        }
        !acc.finished && acc.error.is_none()
    });
    if ep.kind == ServerKind::OpenAi {
        oa.finish(&mut acc, res.is_ok() && !cancel.load(Ordering::Relaxed), on);
    }
    if cancel.load(Ordering::Relaxed) {
        return on(StreamEvent::Error(LlmError::Cancelled));
    }
    if let Err(e) = res {
        // Nach der Ende-Kennung wird nicht mehr gelesen; ein Lesefehler davor ist ein Fehler.
        if !acc.finished {
            acc.error.get_or_insert(e);
        }
    }
    acc.finish(on);
}

/// Laufender Chat-Aufruf in einem eigenen Faden.
#[cfg(test)] // Faden-Hülle nur für die Tests; die Chats nutzen `chat::llm_backend`
pub struct ChatHandle {
    pub rx: Receiver<StreamEvent>,
    pub cancel: Arc<AtomicBool>,
}

#[cfg(test)] // Faden-Hülle nur für die Tests; die Chats nutzen `chat::llm_backend`
impl ChatHandle {
    /// Bricht ab (auch während auf das erste Token gewartet wird); danach kommt `Error(Cancelled)`.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
impl Drop for ChatHandle {
    /// Wer den Aufruf nicht mehr hält, braucht ihn nicht mehr: Verbindung beenden.
    fn drop(&mut self) {
        self.stop();
    }
}

/// Startet `chat` in einem Faden über die echte Verbindung; `repaint` wird nach jedem Ereignis aufgerufen.
#[cfg(test)] // Faden-Hülle nur für die Tests; die Chats nutzen `chat::llm_backend`
#[cfg_attr(not(windows), allow(dead_code))]
pub fn spawn_chat(endpoint: Endpoint, request: ChatRequest, repaint: Box<dyn Fn() + Send>) -> ChatHandle {
    spawn_chat_with(system_transport(), endpoint, request, repaint)
}

/// Wie `spawn_chat`, mit eigenem Transport (Tests, Attrappen).
#[cfg(test)] // Faden-Hülle nur für die Tests; die Chats nutzen `chat::llm_backend`
pub fn spawn_chat_with(t: Arc<dyn Transport>, endpoint: Endpoint, request: ChatRequest, repaint: Box<dyn Fn() + Send>) -> ChatHandle {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        let mut ended = false;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            chat(t.as_ref(), &endpoint, &request, &c, &mut |ev| {
                ended |= matches!(ev, StreamEvent::Done { .. } | StreamEvent::Error(_));
                if tx.send(ev).is_err() {
                    // Empfänger weg: abbrechen.
                    c.store(true, Ordering::Relaxed);
                }
                repaint();
            })
        }));
        // Panik im Faden (nur ohne `panic = "abort"` abfangbar, also in Tests/Debug): trotzdem ein Ende melden.
        if r.is_err() && !ended {
            let _ = tx.send(StreamEvent::Error(LlmError::BadResponse("internal error".into())));
            repaint();
        }
    });
    ChatHandle { rx, cancel }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod debug_tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_key() {
        let ep = Endpoint::new(ServerKind::Ollama, "http://localhost:11434").with_key(Some("sk-secret-1".into()));
        let s = format!("{ep:?}");
        assert!(!s.contains("sk-secret-1") && s.contains("***"), "{s}");
        let r = ep.request(http::Method::Post, "http://localhost:11434/api/chat", Some(b"{}"), "application/json");
        let s = format!("{r:?}");
        assert!(!s.contains("sk-secret-1") && s.contains("***"), "{s}");
    }
}
