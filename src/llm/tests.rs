//! Tests der Adapter mit Attrappe (`http::fake`) und ein Ende-zu-Ende-Test gegen einen kleinen HTTP-Server auf 127.0.0.1.

use super::http::Method;
use super::http::fake::{Canned, Fake};
use super::*;
use serde_json::json;

const OL: &str = "http://localhost:11434";
const OA: &str = "http://127.0.0.1:1234/v1";

fn run(t: &Fake, ep: &Endpoint, req: &ChatRequest) -> Vec<StreamEvent> {
    let mut ev = Vec::new();
    chat(t, ep, req, &Arc::new(AtomicBool::new(false)), &mut |e| ev.push(e));
    ev
}

fn text(ev: &[StreamEvent]) -> String {
    ev.iter().filter_map(|e| if let StreamEvent::Delta(s) = e { Some(s.as_str()) } else { None }).collect()
}

fn req() -> ChatRequest {
    ChatRequest { model: "m1".into(), messages: vec![ChatMessage::new(Role::System, "sys"), ChatMessage::new(Role::User, "hi")], ..Default::default() }
}

fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

// ---------- Ollama ----------

#[test]
fn ollama_lists_models_with_details() {
    let t = Fake::default()
        .with(Method::Get, "http://localhost:11434/api/tags", Canned::ok(r#"{"models":[{"name":"qwen3:8b","size":1},{"name":"llama3:8b"},{"name":"broken"}]}"#))
        .with(Method::Post, "http://localhost:11434/api/show", Canned::ok(r#"{"capabilities":["completion","tools"],"model_info":{"general.architecture":"qwen3","qwen3.context_length":40960}}"#));
    let ep = Endpoint::new(ServerKind::Ollama, OL);
    let ms = list_models(&t, &ep, &no_cancel()).unwrap();
    assert_eq!(ms.len(), 3);
    assert_eq!(ms[0], ModelInfo { name: "qwen3:8b".into(), tools: Some(true), context_length: Some(40960) });
    let log = t.log();
    assert_eq!(log.len(), 4);
    assert_eq!(log[1].body, Some(json!({"model": "qwen3:8b"})));
    assert_eq!(log[2].body, Some(json!({"model": "llama3:8b"})));
    assert!(log.iter().all(|l| l.bearer.is_none()));
}

#[test]
fn ollama_show_details() {
    assert_eq!(ollama::show_info(&json!({"capabilities":["completion"]})), (Some(false), None));
    assert_eq!(ollama::show_info(&json!({})), (None, None));
    assert_eq!(ollama::show_info(&json!({"model_info":{"llama.context_length":8192}})), (None, Some(8192)));
}

#[test]
fn ollama_show_failure_keeps_model() {
    let t = Fake::default()
        .with(Method::Get, "http://localhost:11434/api/tags", Canned::ok(r#"{"models":[{"name":"a"}]}"#))
        .with(Method::Post, "http://localhost:11434/api/show", Canned::status(500, "{}"));
    let ms = list_models(&t, &Endpoint::new(ServerKind::Ollama, OL), &no_cancel()).unwrap();
    assert_eq!(ms, vec![ModelInfo { name: "a".into(), tools: None, context_length: None }]);
}

#[test]
fn ollama_list_errors() {
    let t = Fake::default().with(Method::Get, "http://localhost:11434/api/tags", Canned::ok("<html>"));
    assert!(matches!(list_models(&t, &Endpoint::new(ServerKind::Ollama, OL), &no_cancel()), Err(LlmError::BadResponse(_))));
    let t = Fake::default().with(Method::Get, "http://localhost:11434/api/tags", Canned::status(401, r#"{"error":"unauthorized"}"#));
    assert_eq!(list_models(&t, &Endpoint::new(ServerKind::Ollama, OL), &no_cancel()), Err(LlmError::Status { code: 401, body: r#"{"error":"unauthorized"}"#.into() }));
    let c = Arc::new(AtomicBool::new(true));
    assert_eq!(list_models(&t, &Endpoint::new(ServerKind::Ollama, OL), &c), Err(LlmError::Cancelled));
}

#[test]
fn ollama_request_json() {
    let call = ToolCall { id: "c1".into(), name: "list_sessions".into(), arguments: json!({"all": true}) };
    let mut r = req();
    r.messages.push(ChatMessage { role: Role::Assistant, tool_calls: vec![call.clone()], ..Default::default() });
    r.messages.push(ChatMessage::tool_result(&call, "[]"));
    r.tools = vec![ToolSpec { name: "list_sessions".into(), description: "List".into(), parameters: json!({"type":"object"}) }];
    r.temperature = Some(0.5);
    r.num_ctx = Some(8192);
    let v = ollama::request_json(&r);
    assert_eq!(
        v,
        json!({
            "model": "m1",
            "stream": true,
            "messages": [
                {"role":"system","content":"sys"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":"","tool_calls":[{"function":{"name":"list_sessions","arguments":{"all":true}}}]},
                {"role":"tool","content":"[]","tool_name":"list_sessions"}
            ],
            "tools": [{"type":"function","function":{"name":"list_sessions","description":"List","parameters":{"type":"object"}}}],
            "options": {"temperature": 0.5, "num_ctx": 8192}
        })
    );
    assert!(ollama::request_json(&req()).get("tools").is_none());
    assert!(ollama::request_json(&req()).get("options").is_none());
}

const OL_CHAT: &str = "http://localhost:11434/api/chat";

#[test]
fn ollama_streams_text() {
    let body = concat!(
        r#"{"model":"m1","message":{"role":"assistant","content":"Hel"},"done":false}"#,
        "\n",
        r#"{"model":"m1","message":{"role":"assistant","content":"lo ✓"},"done":false}"#,
        "\n",
        r#"{"model":"m1","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":12,"eval_count":3}"#,
        "\n"
    );
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok(body));
    let ep = Endpoint::new(ServerKind::Ollama, OL).with_key(Some("  secret ".into()));
    let ev = run(&t, &ep, &req());
    assert_eq!(text(&ev), "Hello ✓");
    assert_eq!(ev.last(), Some(&StreamEvent::Done { prompt_tokens: Some(12), completion_tokens: Some(3), finish_reason: Some("stop".into()) }));
    let log = t.log();
    assert_eq!(log[0].bearer.as_deref(), Some("secret"));
    assert_eq!(log[0].body.as_ref().unwrap()["stream"], json!(true));
}

/// Denkende Modelle (gemma4 über Ollama): lange nur `message.thinking`, dann Text (#299).
#[test]
fn ollama_thinking_is_reported_but_not_shown() {
    let body = concat!(
        r#"{"model":"gemma4:e2b","message":{"role":"assistant","content":"","thinking":"Let me"},"done":false}"#,
        "\n",
        r#"{"model":"gemma4:e2b","message":{"role":"assistant","content":"","thinking":" think."},"done":false}"#,
        "\n",
        r#"{"model":"gemma4:e2b","message":{"role":"assistant","content":"Hi"},"done":false}"#,
        "\n",
        r#"{"model":"gemma4:e2b","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#,
        "\n"
    );
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    assert_eq!(ev, vec![StreamEvent::Thinking, StreamEvent::Thinking, StreamEvent::Delta("Hi".into()), StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: Some("stop".into()) }]);
    // Nur Denken bis zur Token-Grenze: gültiger Strom (kein „unlesbar“), Ende mit `length`.
    let body = concat!(
        r#"{"model":"gemma4:e2b","message":{"role":"assistant","content":"","thinking":"hmm"},"done":false}"#,
        "\n",
        r#"{"model":"gemma4:e2b","message":{"role":"assistant","content":""},"done":true,"done_reason":"length","eval_count":4096}"#,
        "\n"
    );
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    assert_eq!(ev, vec![StreamEvent::Thinking, StreamEvent::Done { prompt_tokens: None, completion_tokens: Some(4096), finish_reason: Some("length".into()) }]);
}

#[test]
fn ollama_tool_calls() {
    let body = concat!(
        r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_terminal","arguments":{"session":3}}},{"id":"x9","function":{"name":"list_sessions","arguments":"{\"a\":1}"}}]},"done":false}"#,
        "\n",
        r#"{"message":{"role":"assistant","content":""},"done":true}"#
    );
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    assert_eq!(
        ev,
        vec![
            StreamEvent::ToolCalls(vec![
                ToolCall { id: "call_0".into(), name: "read_terminal".into(), arguments: json!({"session": 3}) },
                ToolCall { id: "x9".into(), name: "list_sessions".into(), arguments: json!({"a": 1}) },
            ]),
            StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: None },
        ]
    );
}

#[test]
fn ollama_malformed_lines_are_skipped() {
    let body = "garbage\n{\"message\":{\"content\":\"ok\"},\"done\":false}\n[1,2]\n{\"done\":true}\n";
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    assert_eq!(text(&ev), "ok");
    assert!(matches!(ev.last(), Some(StreamEvent::Done { .. })));
    // Nichts Gültiges → Fehler
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok("garbage\n<html>\n"));
    assert!(matches!(run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req()).last(), Some(StreamEvent::Error(LlmError::BadResponse(_)))));
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok(""));
    assert!(matches!(run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req()).as_slice(), [StreamEvent::Error(LlmError::BadResponse(_))]));
}

#[test]
fn ollama_errors() {
    // HTTP-Fehler mit Rumpf
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::status(404, r#"{"error":"model \"m1\" not found, try pulling it first"}"#));
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    let [StreamEvent::Error(e)] = ev.as_slice() else { panic!("{ev:?}") };
    assert!(e.text(crate::state::Lang::En).contains("not found, try pulling it first"));
    // Fehler mitten im Strom
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok("{\"message\":{\"content\":\"a\"}}\n{\"error\":\"out of memory\"}\n{\"message\":{\"content\":\"b\"}}\n"));
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    assert_eq!(ev, vec![StreamEvent::Delta("a".into()), StreamEvent::Error(LlmError::Server("out of memory".into()))]);
    // Abgerissene Verbindung
    let mut c = Canned::ok("{\"message\":{\"content\":\"a\"}}\n");
    c.fail_at_end = Some(LlmError::Sys { code: 12030, call: "WinHttpReadData" });
    let t = Fake::default().with(Method::Post, OL_CHAT, c);
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    assert_eq!(ev.last(), Some(&StreamEvent::Error(LlmError::Sys { code: 12030, call: "WinHttpReadData" })));
    // Adresse nicht erlaubt: keine Anfrage
    let t = Fake::default();
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, "http://user@localhost"), &req());
    assert_eq!(ev, vec![StreamEvent::Error(LlmError::BadUrl(UrlProblem::Userinfo))]);
    assert!(t.log().is_empty());
}

#[test]
fn ollama_cancel_mid_stream() {
    let lines: Vec<Vec<u8>> = (0..10).map(|i| format!("{{\"message\":{{\"content\":\"{i}\"}}}}\n").into_bytes()).collect();
    let mut c = Canned::chunks(200, lines);
    c.cancel_after = Some(3);
    let t = Fake::default().with(Method::Post, OL_CHAT, c);
    let ev = run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req());
    assert_eq!(text(&ev), "012");
    assert_eq!(ev.last(), Some(&StreamEvent::Error(LlmError::Cancelled)));
    // Schon vorher abgebrochen: keine Anfrage
    let t = Fake::default();
    let mut ev = Vec::new();
    chat(&t, &Endpoint::new(ServerKind::Ollama, OL), &req(), &Arc::new(AtomicBool::new(true)), &mut |e| ev.push(e));
    assert_eq!(ev, vec![StreamEvent::Error(LlmError::Cancelled)]);
    assert!(t.log().is_empty());
}

// ---------- OpenAI ----------

const OA_CHAT: &str = "http://127.0.0.1:1234/v1/chat/completions";

#[test]
fn openai_lists_models() {
    let t = Fake::default().with(Method::Get, "http://127.0.0.1:1234/v1/models", Canned::ok(r#"{"object":"list","data":[{"id":"qwen2.5-7b","object":"model"},{"id":"phi-4"},{"x":1}]}"#));
    let ep = Endpoint::new(ServerKind::OpenAi, OA).with_key(Some("sk-1".into()));
    let ms = list_models(&t, &ep, &no_cancel()).unwrap();
    assert_eq!(ms.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["qwen2.5-7b", "phi-4"]);
    assert!(ms.iter().all(|m| m.tools.is_none() && m.context_length.is_none()));
    assert_eq!(t.log()[0].bearer.as_deref(), Some("sk-1"));
    // Basis ohne /v1 ergibt dieselbe Adresse
    assert_eq!(Endpoint::new(ServerKind::OpenAi, "http://127.0.0.1:1234").api_url("/v1/models").unwrap(), "http://127.0.0.1:1234/v1/models");
    assert_eq!(Endpoint::new(ServerKind::OpenAi, "http://127.0.0.1:1234/v1/").api_url("/v1/models").unwrap(), "http://127.0.0.1:1234/v1/models");
    assert_eq!(Endpoint::new(ServerKind::OpenAi, "http://h.lan/llm/v1").api_url("/v1/models").unwrap(), "http://h.lan/llm/v1/models");
    let t = Fake::default().with(Method::Get, "http://127.0.0.1:1234/v1/models", Canned::ok(r#"{"models":[]}"#));
    assert!(matches!(list_models(&t, &ep, &no_cancel()), Err(LlmError::BadResponse(_))));
}

#[test]
fn openai_request_json() {
    let call = ToolCall { id: "call_1".into(), name: "f".into(), arguments: json!({"x": 1}) };
    let mut r = req();
    r.messages.push(ChatMessage { role: Role::Assistant, tool_calls: vec![call.clone()], ..Default::default() });
    r.messages.push(ChatMessage::tool_result(&call, "ok"));
    r.tools = vec![ToolSpec { name: "f".into(), description: "d".into(), parameters: json!({"type":"object","properties":{}}) }];
    r.temperature = Some(0.25);
    r.num_ctx = Some(4096);
    let v = openai::request_json(&r);
    assert_eq!(
        v,
        json!({
            "model": "m1",
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": [
                {"role":"system","content":"sys"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{\"x\":1}"}}]},
                {"role":"tool","content":"ok","tool_call_id":"call_1"}
            ],
            "tools": [{"type":"function","function":{"name":"f","description":"d","parameters":{"type":"object","properties":{}}}}],
            "temperature": 0.25
        })
    );
}

#[test]
fn openai_streams_text_and_usage() {
    let body = concat!(
        ": keep-alive\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\r\n\r\n",
        "data:{\"choices\":[{\"index\":0,\"delta\":{\"content\":\" there\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":2,\"total_tokens\":22}}\n\n",
        "data: [DONE]\n\n"
    );
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req());
    assert_eq!(ev, vec![StreamEvent::Delta("Hi".into()), StreamEvent::Delta(" there".into()), StreamEvent::Done { prompt_tokens: Some(20), completion_tokens: Some(2), finish_reason: Some("stop".into()) }]);
}

/// Denk-Ausgabe OpenAI-kompatibler Server (`reasoning_content` bei llama.cpp/vLLM, `reasoning` bei LM Studio/Ollama).
#[test]
fn openai_reasoning_is_reported_but_not_shown() {
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"plan\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\" more\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req());
    assert_eq!(ev, vec![StreamEvent::Thinking, StreamEvent::Thinking, StreamEvent::Delta("ok".into()), StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: Some("stop".into()) }]);
}

#[test]
fn openai_fragmented_tool_calls() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"read_terminal\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"sess\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"function\":{\"name\":\"list_sessions\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ion\\\": 3}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":2,\"function\":{\"name\":\"bad\",\"arguments\":\"{oops\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req());
    assert_eq!(
        ev,
        vec![
            StreamEvent::ToolCalls(vec![
                ToolCall { id: "call_a".into(), name: "read_terminal".into(), arguments: json!({"session": 3}) },
                ToolCall { id: "call_b".into(), name: "list_sessions".into(), arguments: json!({}) },
                ToolCall { id: "call_2".into(), name: "bad".into(), arguments: json!("{oops") },
            ]),
            StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: Some("tool_calls".into()) },
        ]
    );
}

#[test]
fn openai_malformed_and_errors() {
    let body = "data: {broken\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\nnonsense\n\ndata: [DONE]\n\n";
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req());
    assert_eq!(ev, vec![StreamEvent::Delta("x".into()), StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: None }]);
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok("data: {broken\n\nhello\n"));
    assert!(matches!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()).as_slice(), [StreamEvent::Error(LlmError::BadResponse(_))]));
    // Fehler-Objekt im Strom
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok("data: {\"error\":{\"message\":\"context length exceeded\",\"code\":400}}\n\n"));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Error(LlmError::Server("context length exceeded".into()))]);
    // HTTP-Fehler mit Rumpf
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::status(400, r#"{"error":{"message":"tools not supported","type":"invalid_request_error"}}"#));
    let ev = run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req());
    let [StreamEvent::Error(e @ LlmError::Status { code: 400, .. })] = ev.as_slice() else { panic!("{ev:?}") };
    assert!(e.text(crate::state::Lang::En).ends_with("(HTTP 400): tools not supported"));
    // Server antwortet am Stück (ohne SSE)
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(r#"{"choices":[{"message":{"role":"assistant","content":"whole"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Delta("whole".into()), StreamEvent::Done { prompt_tokens: Some(1), completion_tokens: Some(1), finish_reason: None }]);
}

#[test]
fn openai_cancel() {
    let lines: Vec<Vec<u8>> = (0..10).map(|i| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{i}\"}}}}]}}\n\n").into_bytes()).collect();
    let mut c = Canned::chunks(200, lines);
    c.cancel_after = Some(2);
    let t = Fake::default().with(Method::Post, OA_CHAT, c);
    let ev = run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req());
    assert_eq!(text(&ev), "01");
    assert_eq!(ev.last(), Some(&StreamEvent::Error(LlmError::Cancelled)));
}

#[test]
fn truncated_streams_are_errors() {
    // Ollama ohne `done`
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok("{\"message\":{\"content\":\"a\"}}\n"));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req()), vec![StreamEvent::Delta("a".into()), StreamEvent::Error(LlmError::BadResponse("stream ended early".into()))]);
    // OpenAI ohne `[DONE]` und ohne `finish_reason`
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok("data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n"));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()).last(), Some(&StreamEvent::Error(LlmError::BadResponse("stream ended early".into()))));
    // `finish_reason` genügt als Ende (manche Server lassen `[DONE]` weg); „length“ = an der Token-Grenze abgeschnitten
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok("data: {\"choices\":[{\"delta\":{\"content\":\"a\"},\"finish_reason\":\"length\"}]}\n\n"));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()).last(), Some(&StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: Some("length".into()) }));
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok("{\"message\":{\"content\":\"a\"},\"done\":true,\"done_reason\":\"length\"}"));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req()).last(), Some(&StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: Some("length".into()) }));
}

#[test]
fn openai_repeated_tool_name_is_not_doubled() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c\",\"function\":{\"name\":\"read_terminal\",\"arguments\":\"{\\\"a\\\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"read_terminal\",\"arguments\":\": 1}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"name\":\"list_\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"name\":\"sessions\"}}]}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(body));
    let ev = run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req());
    let StreamEvent::ToolCalls(calls) = &ev[0] else { panic!("{ev:?}") };
    assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["read_terminal", "list_sessions"]);
    assert_eq!(calls[0].arguments, json!({"a": 1}));
}

#[test]
fn openai_multiline_sse_data() {
    let body = "data: {\"choices\":[{\"delta\":\ndata: {\"content\":\"x\"}}]}\n\ndata: [DONE]";
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(body));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Delta("x".into()), StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: None }]);
}

#[test]
fn tool_call_limits() {
    let calls: Vec<String> = (0..=MAX_TOOL_CALLS).map(|i| format!("{{\"function\":{{\"name\":\"f{i}\",\"arguments\":{{}}}}}}")).collect();
    let body = format!("{{\"message\":{{\"tool_calls\":[{}]}}}}\n{{\"done\":true}}\n", calls.join(","));
    let t = Fake::default().with(Method::Post, OL_CHAT, Canned::ok(&body));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::Ollama, OL), &req()), vec![StreamEvent::Error(LlmError::TooLarge)]);
    // Argumente über 1 MiB in Stücken
    let piece = format!("data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"name\":\"f\",\"arguments\":\"{}\"}}}}]}}}}]}}\n\n", "a".repeat(300_000));
    let body = format!("{}data: [DONE]\n\n", piece.repeat(4));
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(&body));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Error(LlmError::TooLarge)]);
}

#[test]
fn openai_name_id_and_event_limits() {
    let long = "n".repeat(300);
    let body = format!("data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"name\":\"{long}\"}}}}]}}}}]}}\n\ndata: [DONE]\n\n");
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(&body));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Error(LlmError::TooLarge)]);
    let body = format!("data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"{long}\",\"function\":{{\"name\":\"f\"}}}}]}}}}]}}\n\ndata: [DONE]\n\n");
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok(&body));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Error(LlmError::TooLarge)]);
    // Viele `data:`-Zeilen ohne Leerzeile dazwischen
    let line = format!("data: {}\n", "x".repeat(1 << 20));
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::chunks(200, line.repeat(9).into_bytes().chunks(16 << 10).map(|c| c.to_vec()).collect()));
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Error(LlmError::TooLarge)]);
}

#[test]
fn openai_read_error_is_not_hidden_by_leftover_done() {
    let mut c = Canned::ok("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\ndata: [DONE]\n");
    c.fail_at_end = Some(LlmError::Sys { code: 12030, call: "WinHttpReadData" });
    let t = Fake::default().with(Method::Post, OA_CHAT, c);
    assert_eq!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()), vec![StreamEvent::Delta("x".into()), StreamEvent::Error(LlmError::Sys { code: 12030, call: "WinHttpReadData" })]);
    // Ohne Lesefehler zählt das letzte Ereignis ohne Leerzeile.
    let t = Fake::default().with(Method::Post, OA_CHAT, Canned::ok("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\ndata: [DONE]\n"));
    assert!(matches!(run(&t, &Endpoint::new(ServerKind::OpenAi, OA), &req()).last(), Some(StreamEvent::Done { .. })));
}

#[test]
fn endpoint_receive_timeout_is_clamped() {
    let ep = Endpoint::new(ServerKind::Ollama, OL).with_receive_timeout(std::time::Duration::from_millis(1));
    assert_eq!(ep.receive_timeout, MIN_RECEIVE_TIMEOUT);
    let mut ep = Endpoint::new(ServerKind::Ollama, OL);
    ep.receive_timeout = std::time::Duration::ZERO;
    assert_eq!(ep.request(Method::Get, "x", None, "a").receive_timeout, MIN_RECEIVE_TIMEOUT);
    assert_eq!(Endpoint::new(ServerKind::Ollama, OL).receive_timeout, DEFAULT_RECEIVE_TIMEOUT);
}

#[test]
fn dropping_the_handle_cancels() {
    let lines: Vec<Vec<u8>> = (0..1000).map(|i| format!("{{\"message\":{{\"content\":\"{i}\"}}}}\n").into_bytes()).collect();
    let t: Arc<dyn Transport> = Arc::new(Fake::default().with(Method::Post, OL_CHAT, Canned::chunks(200, lines)));
    let h = spawn_chat_with(t, Endpoint::new(ServerKind::Ollama, OL), req(), Box::new(|| {}));
    let c = h.cancel.clone();
    drop(h);
    assert!(c.load(Ordering::Relaxed));
}

// ---------- Typen und Faden ----------

#[test]
fn messages_serde_roundtrip() {
    let call = ToolCall { id: "1".into(), name: "n".into(), arguments: json!({"a": [1]}) };
    let msgs = vec![
        ChatMessage::new(Role::System, "s"),
        ChatMessage { role: Role::Assistant, content: "".into(), tool_calls: vec![call.clone()], ..Default::default() },
        ChatMessage::tool_result(&call, "r"),
    ];
    let s = serde_json::to_string(&msgs).unwrap();
    assert!(s.contains(r#""role":"system""#) && !s.contains("tool_call_id\":null"));
    assert_eq!(serde_json::from_str::<Vec<ChatMessage>>(&s).unwrap(), msgs);
    // Minimale Form lädt (fehlende Felder → Standard)
    assert_eq!(serde_json::from_str::<ChatMessage>(r#"{"role":"user"}"#).unwrap(), ChatMessage::new(Role::User, ""));
    assert_eq!(serde_json::to_string(&ServerKind::OpenAi).unwrap(), "\"openai\"");
    assert_eq!(serde_json::from_str::<ServerKind>("\"ollama\"").unwrap(), ServerKind::Ollama);
}

#[test]
fn spawn_chat_delivers_events() {
    let t: Arc<dyn Transport> = Arc::new(Fake::default().with(Method::Post, OL_CHAT, Canned::ok("{\"message\":{\"content\":\"yo\"},\"done\":true}\n")));
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n2 = n.clone();
    let h = spawn_chat_with(t, Endpoint::new(ServerKind::Ollama, OL), req(), Box::new(move || {
        n2.fetch_add(1, Ordering::Relaxed);
    }));
    let ev: Vec<StreamEvent> = h.rx.iter().collect();
    assert_eq!(ev, vec![StreamEvent::Delta("yo".into()), StreamEvent::Done { prompt_tokens: None, completion_tokens: None, finish_reason: None }]);
    assert_eq!(n.load(Ordering::Relaxed), 2);
}

// ---------- Ende-zu-Ende gegen echtes WinHTTP ----------

#[cfg(windows)]
mod e2e {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    /// Mitgeschnittene Anfrage: Kopf (Kleinbuchstaben-Schlüssel) und Rumpf.
    struct Seen {
        head: String,
        body: String,
    }

    /// Ein Server für genau eine Verbindung; `respond` schreibt die Antwort.
    fn serve(addr: &str, respond: impl FnOnce(&mut std::net::TcpStream) + Send + 'static) -> (String, std::thread::JoinHandle<Seen>) {
        let l = TcpListener::bind(addr).unwrap();
        let a = l.local_addr().unwrap();
        let base = if a.is_ipv6() { format!("http://[{}]:{}", a.ip(), a.port()) } else { format!("http://{a}") };
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let head_end = loop {
                let n = s.read(&mut tmp).unwrap();
                assert!(n > 0, "client closed early");
                buf.extend_from_slice(&tmp[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
            while buf.len() < head_end + len {
                let n = s.read(&mut tmp).unwrap();
                buf.extend_from_slice(&tmp[..n]);
            }
            let body = String::from_utf8_lossy(&buf[head_end..head_end + len]).to_string();
            respond(&mut s);
            Seen { head, body }
        });
        (base, h)
    }

    #[test]
    fn real_winhttp_streams_ndjson() {
        let (base, h) = serve("127.0.0.1:0", |s| {
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n").unwrap();
            for part in ["{\"message\":{\"content\":\"Hal\"}}\n", "{\"message\":{\"content\":\"lo\"}}\n", "{\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n"] {
                s.write_all(part.as_bytes()).unwrap();
                s.flush().unwrap();
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let ep = Endpoint::new(ServerKind::Ollama, &base).with_key(Some("k-123".into()));
        let mut ev = Vec::new();
        chat(system_transport().as_ref(), &ep, &req(), &no_cancel(), &mut |e| ev.push(e));
        assert_eq!(ev, vec![StreamEvent::Delta("Hal".into()), StreamEvent::Delta("lo".into()), StreamEvent::Done { prompt_tokens: Some(5), completion_tokens: Some(2), finish_reason: None }]);
        let seen = h.join().unwrap();
        assert!(seen.head.starts_with("POST /api/chat HTTP/1.1\r\n"), "{}", seen.head);
        let lower = seen.head.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer k-123\r\n"));
        assert!(lower.contains("content-type: application/json\r\n"));
        assert!(!lower.contains("cookie"));
        let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
        assert_eq!(body["model"], "m1");
    }

    #[test]
    fn real_winhttp_error_body_and_ipv6() {
        // IPv6-Loopback ist nicht überall eingerichtet; dann reicht der IPv4-Teil.
        let addr = if TcpListener::bind("[::1]:0").is_ok() { "[::1]:0" } else { "127.0.0.1:0" };
        let (base, h) = serve(addr, |s| {
            let body = r#"{"error":"model 'm1' not found"}"#;
            s.write_all(format!("HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
        });
        let r = list_models(system_transport().as_ref(), &Endpoint::new(ServerKind::Ollama, &base), &no_cancel());
        assert_eq!(r, Err(LlmError::Status { code: 404, body: r#"{"error":"model 'm1' not found"}"#.into() }));
        assert!(h.join().unwrap().head.starts_with("GET /api/tags HTTP/1.1\r\n"));
    }

    #[test]
    fn real_winhttp_localhost_reaches_ipv4_server() {
        // Wie Ollama: lauscht nur auf 127.0.0.1, eingetragen ist `localhost`.
        let (base, h) = serve("127.0.0.1:0", |s| {
            let body = r#"{"object":"list","data":[{"id":"phi-4"}]}"#;
            s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
        });
        let base = base.replace("127.0.0.1", "localhost");
        let r = list_models(system_transport().as_ref(), &Endpoint::new(ServerKind::OpenAi, &format!("{base}/v1")), &no_cancel()).unwrap();
        assert_eq!(r, vec![ModelInfo { name: "phi-4".into(), tools: None, context_length: None }]);
        assert!(h.join().unwrap().head.starts_with("GET /v1/models HTTP/1.1\r\n"));
    }

    #[test]
    fn real_winhttp_cancel_while_waiting_for_first_token() {
        let (base, h) = serve("127.0.0.1:0", |s| {
            // Antwortet nie; wartet, bis der Client die Verbindung schließt.
            let mut b = [0u8; 16];
            let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
            let _ = s.read(&mut b);
        });
        let ep = Endpoint::new(ServerKind::Ollama, &base);
        let handle = spawn_chat(ep, req(), Box::new(|| {}));
        std::thread::sleep(Duration::from_millis(400));
        let t0 = Instant::now();
        handle.stop();
        let ev = handle.rx.recv_timeout(Duration::from_secs(5)).expect("cancel must end the call quickly");
        assert_eq!(ev, StreamEvent::Error(LlmError::Cancelled));
        assert!(t0.elapsed() < Duration::from_secs(3));
        let _ = h.join();
    }

    #[test]
    fn real_winhttp_does_not_follow_redirects() {
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        other.set_nonblocking(true).unwrap();
        let target = format!("http://{}/api/tags", other.local_addr().unwrap());
        let (base, h) = serve("127.0.0.1:0", move |s| {
            s.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
        });
        let r = list_models(system_transport().as_ref(), &Endpoint::new(ServerKind::Ollama, &base), &no_cancel());
        assert!(matches!(r, Err(LlmError::Status { code: 302, .. })), "{r:?}");
        h.join().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(other.accept().is_err(), "redirect target must not be contacted");
    }

    #[test]
    fn real_winhttp_cancel_during_read() {
        let (base, h) = serve("127.0.0.1:0", |s| {
            s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"message\":{\"content\":\"first\"}}\n").unwrap();
            s.flush().unwrap();
            // Danach nichts mehr, bis der Client schließt.
            let mut b = [0u8; 16];
            let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
            let _ = s.read(&mut b);
        });
        let handle = spawn_chat(Endpoint::new(ServerKind::Ollama, &base), req(), Box::new(|| {}));
        assert_eq!(handle.rx.recv_timeout(Duration::from_secs(5)).unwrap(), StreamEvent::Delta("first".into()));
        std::thread::sleep(Duration::from_millis(200));
        let t0 = Instant::now();
        handle.stop();
        assert_eq!(handle.rx.recv_timeout(Duration::from_secs(5)).unwrap(), StreamEvent::Error(LlmError::Cancelled));
        assert!(t0.elapsed() < Duration::from_secs(3));
        let _ = h.join();
    }

    fn resolve_to_loopback(host: &str, _: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        assert_eq!(host, "model-box.lan");
        Ok(vec!["127.0.0.1".parse().unwrap()])
    }

    fn resolve_mixed(_: &str, _: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        Ok(vec!["127.0.0.1".parse().unwrap(), "93.184.216.34".parse().unwrap()])
    }

    #[test]
    fn real_winhttp_pins_http_names_to_the_checked_ip() {
        let (base, h) = serve("127.0.0.1:0", |s| {
            let body = r#"{"data":[{"id":"m"}]}"#;
            s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
        });
        let port = base.rsplit(':').next().unwrap().to_string();
        let t = super::super::http::winhttp::WinHttp { resolve: resolve_to_loopback, use_pin: true };
        let ep = Endpoint::new(ServerKind::OpenAi, &format!("http://model-box.lan:{port}")).with_key(Some("k".into()));
        assert_eq!(list_models(&t, &ep, &no_cancel()).unwrap().len(), 1);
        let seen = h.join().unwrap();
        let lower = seen.head.to_ascii_lowercase();
        assert!(lower.contains(&format!("\r\nhost: model-box.lan:{port}\r\n")), "{}", seen.head);
        assert_eq!(lower.matches("\r\nhost:").count(), 1, "exactly one Host header: {}", seen.head);
    }

    #[test]
    fn real_winhttp_fallback_without_resolution_hostname() {
        // Rückfall für Windows ohne RESOLUTION_HOSTNAME: http verbindet mit der geprüften IP und setzt `Host:` selbst.
        let (base, h) = serve("127.0.0.1:0", |s| {
            let body = r#"{"data":[{"id":"m"}]}"#;
            s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
        });
        let port = base.rsplit(':').next().unwrap().to_string();
        let t = super::super::http::winhttp::WinHttp { resolve: resolve_to_loopback, use_pin: false };
        let ep = Endpoint::new(ServerKind::OpenAi, &format!("http://model-box.lan:{port}"));
        assert_eq!(list_models(&t, &ep, &no_cancel()).unwrap().len(), 1);
        let lower = h.join().unwrap().head.to_ascii_lowercase();
        assert!(lower.contains(&format!("\r\nhost: model-box.lan:{port}\r\n")), "{lower}");
        assert_eq!(lower.matches("\r\nhost:").count(), 1, "{lower}");
    }

    #[test]
    fn real_winhttp_pins_https_names_with_sni_on_the_name() {
        // `model-box.lan` gibt es nicht im DNS: kommt die Verbindung auf 127.0.0.1 an, hat WinHTTP die gebundene IP benutzt.
        // Im TLS-ClientHello muss der Name als SNI stehen (Zertifikatsprüfung auf den Namen).
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        l.set_nonblocking(true).unwrap();
        let h = std::thread::spawn(move || {
            let t0 = Instant::now();
            let mut s = loop {
                match l.accept() {
                    Ok((s, _)) => break s,
                    Err(_) if t0.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => return Vec::new(),
                }
            };
            s.set_nonblocking(false).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut hello = vec![0u8; 4096];
            let n = s.read(&mut hello).unwrap_or(0);
            hello.truncate(n);
            hello
        });
        let t = super::super::http::winhttp::WinHttp { resolve: resolve_to_loopback, use_pin: true };
        let ep = Endpoint::new(ServerKind::Ollama, &format!("https://model-box.lan:{port}")).with_key(Some("k".into()));
        assert!(list_models(&t, &ep, &no_cancel()).is_err());
        let hello = h.join().unwrap();
        assert!(hello.first() == Some(&0x16), "TLS handshake expected");
        assert!(hello.windows(13).any(|w| w == b"model-box.lan"), "SNI must be the name");
    }

    #[test]
    fn real_winhttp_disallowed_resolution_sends_nothing() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let t = super::super::http::winhttp::WinHttp { resolve: resolve_mixed, use_pin: true };
        let ep = Endpoint::new(ServerKind::Ollama, &format!("http://model-box.lan:{port}")).with_key(Some("secret".into()));
        let r = list_models(&t, &ep, &no_cancel());
        assert!(matches!(r, Err(LlmError::Blocked { .. })), "{r:?}");
        std::thread::sleep(Duration::from_millis(200));
        assert!(l.accept().is_err(), "no connection may be made");
    }

    #[test]
    fn real_winhttp_refuses_blocked_and_closed() {
        let r = list_models(system_transport().as_ref(), &Endpoint::new(ServerKind::Ollama, "http://8.8.8.8"), &no_cancel());
        assert!(matches!(r, Err(LlmError::Blocked { .. })));
        // Freier Port ohne Server → verständlicher Verbindungsfehler
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let r = list_models(system_transport().as_ref(), &Endpoint::new(ServerKind::Ollama, &format!("http://127.0.0.1:{port}")), &no_cancel());
        assert!(matches!(r, Err(LlmError::Sys { code: 12029, .. })), "{r:?}");
    }

    #[test]
    fn real_winhttp_untrusted_certificate() {
        // Ein „HTTPS-Server“, der kein TLS spricht, ergibt einen TLS-Fehler (kein Zertifikat zum Prüfen). Ein echter
        // selbst signierter Server lässt sich ohne TLS-Bibliothek nicht nachbauen; die Zuordnung prüft `error::tests`.
        let (base, h) = serve_raw();
        let url = base.replace("http://", "https://");
        let r = list_models(system_transport().as_ref(), &Endpoint::new(ServerKind::Ollama, &url), &no_cancel());
        assert!(matches!(r, Err(LlmError::Sys { .. } | LlmError::Cert { .. })), "{r:?}");
        let _ = h.join();
    }

    fn serve_raw() -> (String, std::thread::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut b = [0u8; 1024];
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
            }
        });
        (base, h)
    }
}
