//! Paneldeck-Werkzeuge in Chats (#63): welche MCP-Werkzeuge ein Modell bekommt, Prüfung jedes Aufrufs zur
//! Ausführungszeit und die Ergebnis-Nachricht für das Modell. Kennt weder egui noch die App; ausgeführt wird im Hub
//! (`App::mcp_call_chat`) mit dem Chat als Aufrufer.
//!
//! Die Stufe des Modells (Einstellungen → Modelle) gilt unabhängig von der globalen MCP-Stufe für externe Agenten; die
//! Schutzregeln des Hubs (nie die eigene Sitzung, keine Administrator-Profile, nur eigene Kinder schließen …) gelten
//! unverändert, weil derselbe Ausführer läuft.

use super::store::{ToolCall, ToolStatus};
use crate::llm::config::LlmModel;
use crate::llm::ToolSpec;
use crate::mcp::{self, McpLevel};
use serde_json::{json, Value};

/// Größte Ergebnis-Nachricht an das Modell (Bytes); Größeres wird gekürzt und als gekürzt gekennzeichnet.
pub const MAX_RESULT_BYTES: usize = 16 * 1024;

/// Ergebnis des Ausführers (wie im Hub: Wert oder (Code, Meldung)).
pub type ToolResult = Result<Value, (String, String)>;

/// Was für die Werkzeuge eines Chats gilt (Momentaufnahme der Einstellungen, bei jedem Schritt neu).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolPolicy {
    /// Einstellungen des Modells (Stufe, gesperrte Werkzeuge, Nachfragen, Höchstzahl Aufrufe).
    pub cfg: LlmModel,
    /// Kann das Modell Werkzeuge aufrufen (eigene Angabe vor Erkanntem)? `Some(false)` = nur Chat.
    pub tools_supported: Option<bool>,
    /// Expertenmodus der Todo-Liste (bestimmt die Werkzeug-Gruppe).
    pub expert: bool,
}

impl ToolPolicy {
    pub fn new(cfg: LlmModel, tools_supported: Option<bool>, expert: bool) -> Self {
        ToolPolicy { cfg, tools_supported, expert }
    }

    /// Bekommt das Modell überhaupt Werkzeuge?
    pub fn enabled(&self) -> bool {
        self.tools_supported != Some(false) && self.cfg.level != McpLevel::Off
    }

    /// Stufe, für die der Systemprompt gerendert wird: ohne Werkzeuge „Aus“.
    pub fn prompt_level(&self) -> McpLevel {
        if self.enabled() { self.cfg.level } else { McpLevel::Off }
    }

    fn allowed(&self, def: &mcp::ToolDef) -> bool {
        def.level <= self.cfg.level && def.group.available(self.expert) && !self.cfg.disabled_tools.iter().any(|t| t == def.name)
    }

    /// Werkzeuge für die Anfrage: alle, die Stufe und Modus erlauben und die nicht einzeln gesperrt sind.
    pub fn specs(&self) -> Vec<ToolSpec> {
        if !self.enabled() {
            return Vec::new();
        }
        mcp::tools().into_iter().filter(|d| self.allowed(d)).map(|d| ToolSpec { name: d.name.to_string(), description: d.description.to_string(), parameters: d.schema }).collect()
    }

    /// Darf der Aufruf jetzt laufen? `Ok(schreibend)`; sonst die Ergebnis-Nachricht für das Modell (Status, Inhalt).
    /// Geprüft wird bei der Ausführung, nicht beim Senden: die Einstellungen können sich dazwischen ändern.
    pub fn check(&self, call: &ToolCall) -> Result<bool, (ToolStatus, String)> {
        let Some(def) = mcp::tools().into_iter().find(|d| d.name == call.name) else {
            return Err((ToolStatus::Error, error_json("unknown_tool", &format!("There is no tool named “{}”. Use only the tools you were given.", mcp::short(&call.name, 80)))));
        };
        if !self.enabled() || def.level > self.cfg.level || self.cfg.disabled_tools.iter().any(|t| t == def.name) {
            return Err((ToolStatus::Denied, error_json("forbidden", "This tool is not allowed for this model (Settings → Models).")));
        }
        if !def.group.available(self.expert) {
            let msg = if self.expert {
                "Expert mode is on: the to-do list is read-only; use the ticket tools."
            } else {
                "Expert mode is off: only the simple to-do list tools are available."
            };
            return Err((ToolStatus::Denied, error_json("wrong_mode", msg)));
        }
        match &call.arguments {
            Value::Object(_) | Value::Null => {}
            _ => return Err((ToolStatus::Error, error_json("bad_args", "arguments must be a JSON object"))),
        }
        Ok(def.level >= McpLevel::Control)
    }

    /// Muss der Nutzer diesen schreibenden Aufruf freigeben? (`always` = in diesem Chat immer erlaubte Werkzeuge.)
    pub fn needs_approval(&self, write: bool, name: &str, always: &[String]) -> bool {
        write && self.cfg.ask_write && !always.iter().any(|a| a == name)
    }
}

/// Argumente für den Ausführer: `null` wird zu `{}`.
pub fn exec_args(call: &ToolCall) -> Value {
    match &call.arguments {
        Value::Null => json!({}),
        v => v.clone(),
    }
}

/// Fehler-Inhalt einer Werkzeug-Nachricht: `{"error": code, "message": …}` (englisch, für das Modell).
pub fn error_json(code: &str, message: &str) -> String {
    json!({ "error": code, "message": message }).to_string()
}

/// Inhalt für „vom Nutzer abgelehnt“.
pub fn denied() -> (ToolStatus, String) {
    (ToolStatus::Denied, error_json("denied", "The user denied this tool call. Do not retry it; ask the user what to do instead."))
}

/// Inhalt für „abgebrochen“ (Stopp, neue Nachricht).
pub fn cancelled() -> (ToolStatus, String) {
    (ToolStatus::Denied, error_json("cancelled", "The user stopped the answer before this tool call ran."))
}

/// Inhalt für „unterbrochen“ (Paneldeck endete während des Aufrufs).
pub fn interrupted() -> (ToolStatus, String) {
    (ToolStatus::Error, error_json("interrupted", "Paneldeck was closed before this tool call finished; it may or may not have run."))
}

/// Ergebnis des Ausführers → Status und Inhalt (JSON-Text, höchstens [`MAX_RESULT_BYTES`]).
pub fn result_message(tool: &str, r: ToolResult) -> (ToolStatus, String) {
    match r {
        Ok(mut v) => {
            // Bilder kann das Modell hier nicht sehen; Base64 würde nur den Kontext füllen.
            if tool == "screenshot" {
                if let Some(o) = v.as_object_mut() {
                    if o.remove("image_png_base64").is_some() {
                        o.insert("image".into(), json!("omitted – images are not passed to chat models"));
                    }
                }
            }
            (ToolStatus::Ok, cap(v.to_string(), MAX_RESULT_BYTES))
        }
        Err((code, msg)) => (ToolStatus::Error, cap(error_json(&code, &msg), MAX_RESULT_BYTES)),
    }
}

/// Zu langen JSON-Text in ein gültiges JSON-Objekt mit dem Anfang und einem Hinweis verpacken.
pub fn cap(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut cut = max.saturating_sub(300).min(text.len());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let total = text.len();
    let mut out = json!({ "truncated": true, "note": format!("Result truncated to the first {cut} of {total} bytes."), "partial": &text[..cut] }).to_string();
    // Maskierte Zeichen machen den Text länger: notfalls weiter kürzen.
    while out.len() > max && cut > 0 {
        cut = cut.saturating_sub((out.len() - max).max(64)).min(text.len());
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        out = json!({ "truncated": true, "note": format!("Result truncated to the first {cut} of {total} bytes."), "partial": &text[..cut] }).to_string();
    }
    out
}

/// Einzeilige Kurzfassung der Parameter für die Freigabe-Zeile (`text="dir", submit=true`), höchstens `max` Zeichen.
pub fn summary(args: &Value, max: usize) -> String {
    let s = match args {
        Value::Object(o) => o
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::String(s) => format!("{s:?}"),
                    v => v.to_string(),
                };
                format!("{k}={v}")
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Null => String::new(),
        v => v.to_string(),
    };
    mcp::short(&s, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(level: McpLevel, disabled: &[&str], expert: bool) -> ToolPolicy {
        let cfg = LlmModel { level, disabled_tools: disabled.iter().map(|s| s.to_string()).collect(), ..LlmModel::named("m") };
        ToolPolicy::new(cfg, None, expert)
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall { id: "c".into(), name: name.into(), arguments: args }
    }

    fn names(p: &ToolPolicy) -> Vec<String> {
        p.specs().into_iter().map(|s| s.name).collect()
    }

    #[test]
    fn specs_follow_level_mode_and_disabled_tools() {
        assert!(names(&policy(McpLevel::Off, &[], false)).is_empty(), "Aus: keine Werkzeuge");
        let read = names(&policy(McpLevel::Read, &[], false));
        assert!(read.contains(&"list_sessions".to_string()) && read.contains(&"read_terminal".to_string()));
        assert!(!read.contains(&"send_text".to_string()), "Lesen: keine schreibenden Werkzeuge");
        assert!(read.contains(&"list_todos".to_string()) && !read.contains(&"add_todo".to_string()));
        let control = names(&policy(McpLevel::Control, &[], false));
        assert!(control.contains(&"send_text".to_string()) && control.contains(&"add_todo".to_string()));
        assert!(!control.contains(&"create_ticket".to_string()), "einfacher Modus: keine Ticket-Werkzeuge");
        let expert = names(&policy(McpLevel::Control, &[], true));
        assert!(expert.contains(&"create_ticket".to_string()) && !expert.contains(&"add_todo".to_string()));
        let dis = names(&policy(McpLevel::Control, &["send_text", "read_terminal"], false));
        assert!(!dis.contains(&"send_text".to_string()) && !dis.contains(&"read_terminal".to_string()) && dis.contains(&"send_keys".to_string()));
        // Ohne Werkzeug-Fähigkeit: nichts, und der Prompt gilt für „Aus“.
        let mut p = policy(McpLevel::Control, &[], false);
        p.tools_supported = Some(false);
        assert!(p.specs().is_empty());
        assert_eq!(p.prompt_level(), McpLevel::Off);
        p.tools_supported = None;
        assert_eq!(p.prompt_level(), McpLevel::Control);
        // Spezifikation mit Schema
        let s = policy(McpLevel::Read, &[], false).specs().into_iter().find(|s| s.name == "read_terminal").unwrap();
        assert_eq!(s.parameters["type"], "object");
        assert!(!s.description.is_empty());
        // Größe der Beschreibungen (geht vom Kontext ab): Stufe Lesen passt in den Standardkontext.
        let size = |p: &ToolPolicy| crate::llm::tokens::estimate_tokens(&serde_json::to_string(&p.specs()).unwrap());
        let read = size(&policy(McpLevel::Read, &[], false));
        let all = size(&policy(McpLevel::Control, &[], true));
        eprintln!("tool spec tokens: read {read}, control+expert {all}");
        assert!(read < crate::llm::config::DEFAULT_CONTEXT as usize / 2, "{read}");
    }

    #[test]
    fn check_validates_name_level_mode_and_arguments() {
        let p = policy(McpLevel::Read, &["get_usage"], false);
        assert_eq!(p.check(&call("list_sessions", json!({}))), Ok(false));
        assert_eq!(p.check(&call("list_sessions", Value::Null)), Ok(false), "null = keine Argumente");
        let (st, body) = p.check(&call("rm_rf", json!({}))).unwrap_err();
        assert_eq!(st, ToolStatus::Error);
        assert!(body.contains("\"unknown_tool\""), "{body}");
        let (st, body) = p.check(&call("send_text", json!({}))).unwrap_err();
        assert_eq!(st, ToolStatus::Denied);
        assert!(body.contains("\"forbidden\""));
        assert!(p.check(&call("get_usage", json!({}))).unwrap_err().1.contains("forbidden"), "einzeln gesperrt");
        let (_, body) = p.check(&call("list_sessions", Value::String("{oops".into()))).unwrap_err();
        assert!(body.contains("arguments must be a JSON object"), "{body}");
        assert!(p.check(&call("list_sessions", json!([1]))).is_err());
        let c = policy(McpLevel::Control, &[], false);
        assert_eq!(c.check(&call("send_text", json!({"session": 1, "text": "x"}))), Ok(true), "schreibend");
        assert!(c.check(&call("create_ticket", json!({}))).unwrap_err().1.contains("wrong_mode"));
        assert!(policy(McpLevel::Off, &[], false).check(&call("list_sessions", json!({}))).unwrap_err().1.contains("forbidden"));
        // Freigabe
        assert!(c.needs_approval(true, "send_text", &[]));
        assert!(!c.needs_approval(true, "send_text", &["send_text".into()]));
        assert!(!c.needs_approval(false, "list_sessions", &[]));
        let mut quiet = c.clone();
        quiet.cfg.ask_write = false;
        assert!(!quiet.needs_approval(true, "send_text", &[]));
    }

    #[test]
    fn results_are_json_capped_and_screenshots_drop_the_image() {
        let (st, body) = result_message("list_sessions", Ok(json!({"sessions": []})));
        assert_eq!((st, body.as_str()), (ToolStatus::Ok, r#"{"sessions":[]}"#));
        let (st, body) = result_message("send_text", Err(("forbidden".into(), "nope".into())));
        assert_eq!(st, ToolStatus::Error);
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({"error": "forbidden", "message": "nope"}));
        let (_, body) = result_message("screenshot", Ok(json!({"image_png_base64": "A".repeat(100_000), "width": 3})));
        assert!(!body.contains("AAAA") && body.contains("omitted"), "{body}");
        let big = json!({"text": "ä\"x".repeat(20_000)}).to_string();
        let capped = cap(big.clone(), MAX_RESULT_BYTES);
        assert!(capped.len() <= MAX_RESULT_BYTES, "{}", capped.len());
        let v: Value = serde_json::from_str(&capped).unwrap();
        assert_eq!(v["truncated"], true);
        assert!(big.starts_with(v["partial"].as_str().unwrap()));
        assert_eq!(cap("short".into(), 10), "short");
    }

    #[test]
    fn summary_is_one_short_line() {
        assert_eq!(summary(&json!({"session": 3, "text": "dir\nls"}), 80), r#"session=3, text="dir\nls""#);
        assert_eq!(summary(&Value::Null, 80), "");
        assert!(summary(&json!({"t": "x".repeat(500)}), 40).chars().count() <= 41);
        assert_eq!(exec_args(&call("x", Value::Null)), json!({}));
    }
}
