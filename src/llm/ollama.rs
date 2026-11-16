//! Ollama: `/api/tags`, `/api/show`, `/api/chat` (NDJSON-Strom).

use super::http::{self, MAX_BODY, Method};
use super::{Acc, ChatRequest, Endpoint, LlmError, ModelInfo, Role, StreamEvent, ToolCall, Transport, as_u32};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn get_json(t: &dyn Transport, ep: &Endpoint, m: Method, path: &str, body: Option<Value>, cancel: &Arc<AtomicBool>) -> Result<Value, LlmError> {
    let url = ep.api_url(path)?;
    let b = body.map(|b| b.to_string().into_bytes());
    let mut r = http::send_ok(t, &ep.request(m, &url, b.as_deref(), "application/json"), cancel)?;
    let bytes = http::read_all(r.as_mut(), MAX_BODY, cancel)?;
    serde_json::from_slice(&bytes).map_err(|e| LlmError::BadResponse(format!("{path}: {e}")))
}

pub fn list_models(t: &dyn Transport, ep: &Endpoint, cancel: &Arc<AtomicBool>) -> Result<Vec<ModelInfo>, LlmError> {
    let tags = get_json(t, ep, Method::Get, "/api/tags", None, cancel)?;
    let models = tags.get("models").and_then(|m| m.as_array()).ok_or_else(|| LlmError::BadResponse("/api/tags: no \"models\"".into()))?;
    let mut out = Vec::new();
    for m in models {
        let Some(name) = m.get("name").or_else(|| m.get("model")).and_then(|n| n.as_str()) else { continue };
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        // Einzelheiten sind ein Zusatz: scheitert /api/show für ein Modell, bleibt es mit „unbekannt“ in der Liste.
        let (tools, context_length) = match get_json(t, ep, Method::Post, "/api/show", Some(json!({ "model": name })), cancel) {
            Ok(s) => show_info(&s),
            Err(LlmError::Cancelled) => return Err(LlmError::Cancelled),
            Err(_) => (None, None),
        };
        out.push(ModelInfo { name: name.to_string(), tools, context_length });
    }
    Ok(out)
}

/// Aus `/api/show`: `capabilities` enthält „tools“; Kontextlänge aus `model_info.<arch>.context_length`.
pub(super) fn show_info(s: &Value) -> (Option<bool>, Option<u32>) {
    let tools = s.get("capabilities").and_then(|c| c.as_array()).map(|c| c.iter().any(|x| x.as_str() == Some("tools")));
    let ctx = s.get("model_info").and_then(|m| m.as_object()).and_then(|m| m.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, v)| as_u32(Some(v))));
    (tools, ctx)
}

fn role(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

pub fn request_json(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let mut o = json!({ "role": role(m.role), "content": m.content });
            if !m.tool_calls.is_empty() {
                o["tool_calls"] = m.tool_calls.iter().map(|c| json!({ "function": { "name": c.name, "arguments": args_object(&c.arguments) } })).collect();
            }
            if m.role == Role::Tool
                && let Some(n) = &m.tool_name
            {
                o["tool_name"] = json!(n);
            }
            o
        })
        .collect();
    let mut body = json!({ "model": req.model, "messages": messages, "stream": true });
    if !req.tools.is_empty() {
        body["tools"] = req.tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.parameters } })).collect();
    }
    let mut opts = serde_json::Map::new();
    if let Some(t) = req.temperature {
        opts.insert("temperature".into(), json!(t));
    }
    if let Some(n) = req.num_ctx {
        opts.insert("num_ctx".into(), json!(n));
    }
    if let Some(n) = req.num_predict {
        opts.insert("num_predict".into(), json!(n));
    }
    if !opts.is_empty() {
        body["options"] = Value::Object(opts);
    }
    body
}

/// Ollama erwartet die Argumente als Objekt.
fn args_object(v: &Value) -> Value {
    match v {
        Value::Object(_) => v.clone(),
        Value::String(s) => serde_json::from_str::<Value>(s).ok().filter(|v| v.is_object()).unwrap_or_else(|| json!({})),
        _ => json!({}),
    }
}

/// Eine NDJSON-Zeile von `/api/chat`.
pub fn line(l: &str, acc: &mut Acc, on: &mut dyn FnMut(StreamEvent)) {
    let l = l.trim();
    if l.is_empty() {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(l) else {
        acc.malformed += 1;
        return;
    };
    if let Some(e) = v.get("error") {
        acc.error = Some(LlmError::Server(e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string())));
        return;
    }
    if !v.is_object() || (v.get("message").is_none() && v.get("done").is_none()) {
        acc.malformed += 1;
        return;
    }
    acc.valid += 1;
    if let Some(m) = v.get("message") {
        if let Some(c) = m.get("content").and_then(|c| c.as_str()) {
            acc.delta(c, on);
        }
        for tc in m.get("tool_calls").and_then(|t| t.as_array()).into_iter().flatten() {
            let f = tc.get("function").unwrap_or(tc);
            let Some(name) = f.get("name").and_then(|n| n.as_str()) else { continue };
            let arguments = match f.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone())),
                Some(a) => a.clone(),
                None => json!({}),
            };
            let n = acc.tool_calls.len();
            let id = tc.get("id").and_then(|i| i.as_str()).map(str::to_string).unwrap_or_else(|| format!("call_{n}"));
            acc.push_call(ToolCall { id, name: name.to_string(), arguments });
            if acc.error.is_some() {
                return;
            }
        }
    }
    if v.get("done").and_then(|d| d.as_bool()) == Some(true) {
        acc.prompt_tokens = as_u32(v.get("prompt_eval_count"));
        acc.completion_tokens = as_u32(v.get("eval_count"));
        acc.finish_reason = v.get("done_reason").and_then(|r| r.as_str()).map(str::to_string);
        acc.finished = true;
    }
}
