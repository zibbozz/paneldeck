//! Anbindung lokaler LLM-Server (Ollama und OpenAI-kompatibel: LM Studio, llama.cpp-Server, vLLM).
//!
//! Kern ohne egui und ohne App: HTTP (`http`), Host-Regel (`net`), Adapter (`ollama`, `openai`), Datenstrom-Ereignisse,
//! DPAPI für API-Schlüssel (`secret`) und Token-Schätzung (`tokens`). Die Oberfläche (Chat-Sitzungen, Modelle in den
//! Einstellungen) baut darauf auf.
// Wird ab den Folge-Teilen (Chat-Sitzungen, Einstellungen) benutzt; bis dahin sind viele Einträge noch ungenutzt.
#![allow(dead_code)]

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
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

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
/// (nur im Speicher; gespeichert wird er über `secret::protect`).
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub kind: ServerKind,
    pub base_url: String,
    pub api_key: Option<String>,
    /// Wartezeit auf Daten (auch auf das erste Token); Standard 300 s.
    pub receive_timeout: Duration,
}

impl Endpoint {
    pub fn new(kind: ServerKind, base_url: &str) -> Self {
        Self { kind, base_url: base_url.trim().to_string(), api_key: None, receive_timeout: DEFAULT_RECEIVE_TIMEOUT }
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
        http::HttpRequest { method, url, body, bearer: self.api_key.as_deref(), accept, receive_timeout: self.receive_timeout }
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
}

/// Ereignisse eines Chat-Aufrufs. Am Ende kommt genau eines von `Done` oder `Error`; `ToolCalls` (falls vorhanden) direkt
/// vor `Done`.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Delta(String),
    ToolCalls(Vec<ToolCall>),
    Done { prompt_tokens: Option<u32>, completion_tokens: Option<u32> },
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
}

impl Acc {
    /// Schluss-Ereignisse nach dem Strom.
    fn finish(mut self, on: &mut dyn FnMut(StreamEvent)) {
        if let Some(e) = self.error.take() {
            return on(StreamEvent::Error(e));
        }
        if self.valid == 0 {
            let d = if self.malformed > 0 { format!("{} unreadable lines, no valid data", self.malformed) } else { "empty answer".into() };
            return on(StreamEvent::Error(LlmError::BadResponse(d)));
        }
        if !self.tool_calls.is_empty() {
            on(StreamEvent::ToolCalls(std::mem::take(&mut self.tool_calls)));
        }
        on(StreamEvent::Done { prompt_tokens: self.prompt_tokens, completion_tokens: self.completion_tokens });
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
        Err(e) => return on(StreamEvent::Error(e)),
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
        oa.finish(&mut acc);
    }
    if cancel.load(Ordering::Relaxed) {
        return on(StreamEvent::Error(LlmError::Cancelled));
    }
    if let Err(e) = res {
        // Abgerissene Verbindung nach gültigen Daten mit Ende-Kennung zählt nicht; sonst ist es ein Fehler.
        if !acc.finished || acc.error.is_some() {
            acc.error.get_or_insert(e);
        }
    }
    acc.finish(on);
}

/// Laufender Chat-Aufruf in einem eigenen Faden.
pub struct ChatHandle {
    pub rx: Receiver<StreamEvent>,
    pub cancel: Arc<AtomicBool>,
}

impl ChatHandle {
    /// Bricht ab (auch während auf das erste Token gewartet wird); danach kommt `Error(Cancelled)`.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Startet `chat` in einem Faden über die echte Verbindung; `repaint` wird nach jedem Ereignis aufgerufen.
pub fn spawn_chat(endpoint: Endpoint, request: ChatRequest, repaint: Box<dyn Fn() + Send>) -> ChatHandle {
    spawn_chat_with(system_transport(), endpoint, request, repaint)
}

/// Wie `spawn_chat`, mit eigenem Transport (Tests, Attrappen).
pub fn spawn_chat_with(t: Arc<dyn Transport>, endpoint: Endpoint, request: ChatRequest, repaint: Box<dyn Fn() + Send>) -> ChatHandle {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        chat(t.as_ref(), &endpoint, &request, &c, &mut |ev| {
            let _ = tx.send(ev);
            repaint();
        });
    });
    ChatHandle { rx, cancel }
}

#[cfg(test)]
mod tests;
