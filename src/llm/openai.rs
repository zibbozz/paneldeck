//! OpenAI-kompatibel: `/v1/models`, `/v1/chat/completions` mit `stream: true` (SSE, `data:`-Zeilen, `[DONE]`).

use super::http::{self, MAX_BODY, Method};
use super::{Acc, ChatRequest, Endpoint, LlmError, ModelInfo, Role, StreamEvent, ToolCall, Transport, as_u32};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub fn list_models(t: &dyn Transport, ep: &Endpoint, cancel: &Arc<AtomicBool>) -> Result<Vec<ModelInfo>, LlmError> {
    let url = ep.api_url("/v1/models")?;
    let mut r = http::send_ok(t, &ep.request(Method::Get, &url, None, "application/json"), cancel)?;
    let bytes = http::read_all(r.as_mut(), MAX_BODY, cancel)?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| LlmError::BadResponse(format!("/v1/models: {e}")))?;
    let data = v.get("data").and_then(|d| d.as_array()).ok_or_else(|| LlmError::BadResponse("/v1/models: no \"data\"".into()))?;
    Ok(data.iter().filter_map(|m| m.get("id").and_then(|i| i.as_str())).map(|id| ModelInfo { name: id.to_string(), tools: None, context_length: None }).collect())
}

fn role(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// OpenAI erwartet die Argumente als JSON-Text.
fn args_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "{}".into(),
        v => v.to_string(),
    }
}

pub fn request_json(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let mut o = json!({ "role": role(m.role), "content": m.content });
            if !m.tool_calls.is_empty() {
                if m.content.is_empty() {
                    o["content"] = Value::Null;
                }
                o["tool_calls"] = m.tool_calls.iter().map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": args_string(&c.arguments) } })).collect();
            }
            if m.role == Role::Tool
                && let Some(id) = &m.tool_call_id
            {
                o["tool_call_id"] = json!(id);
            }
            o
        })
        .collect();
    let mut body = json!({ "model": req.model, "messages": messages, "stream": true, "stream_options": { "include_usage": true } });
    if !req.tools.is_empty() {
        body["tools"] = req.tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.parameters } })).collect();
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
}

/// Werkzeugaufrufe kommen in Stücken, je `index` zusammengesetzt; die Argumente sind JSON-Text in Teilen.
#[derive(Default)]
struct Partial {
    id: String,
    name: String,
    args: String,
}

#[derive(Default)]
pub struct Stream {
    calls: BTreeMap<u64, Partial>,
}

impl Stream {
    pub fn line(&mut self, l: &str, acc: &mut Acc, on: &mut dyn FnMut(StreamEvent)) {
        let l = l.trim_end();
        // Leerzeilen trennen Ereignisse, `:` = Kommentar (Keep-alive), andere Felder sind hier bedeutungslos.
        if l.is_empty() || l.starts_with(':') || l.starts_with("event:") || l.starts_with("id:") || l.starts_with("retry:") {
            return;
        }
        let Some(data) = l.strip_prefix("data:") else {
            // Manche Server antworten trotz `stream` mit einem einzelnen JSON-Objekt.
            return self.json(l, acc, on);
        };
        let data = data.trim();
        if data == "[DONE]" {
            acc.valid += 1;
            acc.finished = true;
            return;
        }
        self.json(data, acc, on)
    }

    fn json(&mut self, data: &str, acc: &mut Acc, on: &mut dyn FnMut(StreamEvent)) {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            acc.malformed += 1;
            return;
        };
        if let Some(e) = v.get("error") {
            let msg = e.get("message").and_then(|m| m.as_str()).or_else(|| e.as_str()).map(str::to_string).unwrap_or_else(|| e.to_string());
            acc.error = Some(LlmError::Server(msg));
            return;
        }
        let Some(choices) = v.get("choices").and_then(|c| c.as_array()) else {
            if v.get("usage").is_none() {
                acc.malformed += 1;
                return;
            }
            acc.valid += 1;
            self.usage(&v, acc);
            return;
        };
        acc.valid += 1;
        self.usage(&v, acc);
        let Some(c0) = choices.first() else { return };
        // `delta` beim Strom; `message` bei Servern, die doch am Stück antworten.
        let Some(d) = c0.get("delta").or_else(|| c0.get("message")) else { return };
        if let Some(t) = d.get("content").and_then(|c| c.as_str())
            && !t.is_empty()
        {
            on(StreamEvent::Delta(t.to_string()));
        }
        for (pos, tc) in d.get("tool_calls").and_then(|t| t.as_array()).into_iter().flatten().enumerate() {
            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(pos as u64);
            let p = self.calls.entry(idx).or_default();
            if let Some(id) = tc.get("id").and_then(|i| i.as_str())
                && !id.is_empty()
            {
                p.id = id.to_string();
            }
            if let Some(f) = tc.get("function") {
                if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                    p.name.push_str(n);
                }
                match f.get("arguments") {
                    Some(Value::String(s)) => p.args.push_str(s),
                    Some(Value::Null) | None => {}
                    Some(o) => p.args.push_str(&o.to_string()),
                }
            }
        }
    }

    fn usage(&self, v: &Value, acc: &mut Acc) {
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            acc.prompt_tokens = as_u32(u.get("prompt_tokens")).or(acc.prompt_tokens);
            acc.completion_tokens = as_u32(u.get("completion_tokens")).or(acc.completion_tokens);
        }
    }

    /// Setzt die gesammelten Aufrufe zusammen (Argumente erst jetzt als JSON gelesen).
    pub fn finish(&mut self, acc: &mut Acc) {
        for (idx, p) in std::mem::take(&mut self.calls) {
            if p.name.is_empty() {
                continue;
            }
            let raw = p.args.trim();
            let arguments = if raw.is_empty() { json!({}) } else { serde_json::from_str(raw).unwrap_or_else(|_| Value::String(p.args.clone())) };
            let id = if p.id.is_empty() { format!("call_{idx}") } else { p.id };
            acc.tool_calls.push(ToolCall { id, name: p.name, arguments });
        }
    }
}
