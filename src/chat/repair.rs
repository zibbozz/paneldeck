//! Robuste Werkzeugaufrufe in Chats (#340): Aufrufe, die ein Modell als Text schreibt statt über die Werkzeug-
//! Schnittstelle, erkennen ([`find_text_calls`]); Argumente gegen das Schema des Werkzeugs normalisieren, wo es eindeutig
//! ist ([`normalize`]); Argumente prüfen und dabei sagen, welches Feld falsch ist, was erwartet wird und wie ein
//! richtiger Aufruf aussieht ([`validate`]). Kennt weder egui noch die App.
//!
//! Erkannt werden die verbreiteten Textformate der Modellfamilien:
//! * „starke“ Marken, die nie in normalem Text stehen: `<tool_call>{…}</tool_call>` (Qwen, Hermes; auch die XML-Form
//!   `<function=name><parameter=k>v</parameter></function>` von Qwen3-Coder), `[TOOL_CALLS][{…}]` bzw.
//!   `[TOOL_CALLS]name[ARGS]{…}` (Mistral), `<|python_tag|>{…}` (Llama 3.1), `<function=name>{…}</function>` (Llama),
//!   `<|tool_call|>[…]` (Phi-4, Granite), `<｜tool▁call▁begin｜>…` (DeepSeek). Was nach so einer Marke steht, gilt als
//!   Aufruf – auch mit unbekanntem Werkzeug oder kaputtem JSON (dann bekommt das Modell einen Hinweis).
//! * „schwache“ Formen, die auch ein Beispiel im Text sein könnten: die ganze Antwort ist ein JSON-Objekt
//!   `{"name": …, "arguments"/"parameters": …}` (Llama), oder ein solches Objekt steht in einem Codeblock
//!   (```` ```json ````, ```` ```tool_code ```` …; Gemma, Phi, DeepSeek). Sie zählen nur, wenn das Werkzeug bekannt ist,
//!   das Argument-Feld vorhanden ist, nach dem letzten Block nichts mehr folgt und davor höchstens
//!   [`WEAK_PROSE_CHARS`] Zeichen Text stehen. Sonst bleibt die Antwort unverändert.
//!
//! Ein erkannter Aufruf läuft danach genau wie ein echter: dieselbe Prüfung der Stufe, Freigaben und Schutzregeln
//! (`ToolPolicy::check`, `needs_approval`, Umweg-Schutz) – es gibt keine Abkürzung.

use super::store::ToolCall;
use crate::mcp;
use serde_json::{json, Map, Value};

/// Höchstens so viel Text (Zeichen) vor einem Aufruf in einem Codeblock; mehr → der Block gilt als Beispiel im Text.
pub const WEAK_PROSE_CHARS: usize = 400;
/// So viel vom unlesbaren Text eines Aufrufs bleibt als Argument stehen (Zeichen).
const RAW_MAX: usize = 2000;
/// Höchstens so lang wird ein Wert in einer Korrektur-Notiz gezeigt (Zeichen).
const FIX_VALUE_CHARS: usize = 40;
/// Höchstens so lang wird die Feldliste in einer `bad_args`-Meldung (Zeichen).
const SIGNATURE_CHARS: usize = 700;

/// Ein im Antworttext erkannter Werkzeugaufruf. `name` ist leer, wenn keiner zu erkennen war; `arguments` ist ein
/// Objekt oder – bei unlesbarem JSON – der Text als `Value::String` (die Prüfung meldet das dem Modell).
#[derive(Debug, Clone, PartialEq)]
pub struct TextCall {
    pub name: String,
    pub arguments: Value,
}

/// Ergebnis von [`find_text_calls`]: die Aufrufe und der Antworttext ohne sie.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub calls: Vec<TextCall>,
    pub rest: String,
}

const DS_CALL: &str = "<｜tool▁call▁begin｜>";
const DS_SEP: &str = "<｜tool▁sep｜>";
const DS_END: &str = "<｜tool▁call▁end｜>";
/// Hüllen um DeepSeek-Aufrufe, die mit den Aufrufen aus dem Text fallen.
const DS_WRAP: &[&str] = &["<｜tool▁calls▁begin｜>", "<｜tool▁calls▁end｜>"];
const MARKERS: &[&str] = &["<tool_call>", "<|tool_call|>", "[TOOL_CALLS]", "<|python_tag|>", "<function=", DS_CALL];
/// Sprachangaben von Codeblöcken, in denen ein Aufruf stehen kann (klein geschrieben; leer = ohne Angabe).
const WEAK_INFO: &[&str] = &["", "json", "json5", "tool_code", "tool_call", "tool_calls", "tool", "function", "function_call"];

/// Schneller Vorab-Blick: Kann im Text überhaupt ein Aufruf stehen (eine starke Marke oder eine `{`)? Gewöhnliche
/// Antworten ohne JSON brauchen so keinen weiteren Schritt.
pub fn may_contain_call(content: &str) -> bool {
    content.contains('{') || MARKERS.iter().any(|m| content.contains(m))
}

/// Werkzeugaufrufe im Antworttext suchen (siehe Modulbeschreibung). `known` sagt, ob es ein Werkzeug dieses Namens gibt
/// (für die schwachen Formen). `None` = nichts gefunden, die Antwort bleibt unverändert.
pub fn find_text_calls(content: &str, known: &dyn Fn(&str) -> bool) -> Option<Found> {
    let (mut calls, spans) = strong(content);
    let found = if calls.is_empty() { weak(content, known)? } else {
        calls.truncate(crate::llm::MAX_TOOL_CALLS);
        Found { calls, rest: strip(content, &spans) }
    };
    Some(found)
}

/// Starke Marken: alle Aufrufe und die Bereiche, die sie im Text belegen.
fn strong(content: &str) -> (Vec<TextCall>, Vec<(usize, usize)>) {
    let mut calls = Vec::new();
    let mut spans = Vec::new();
    let mut pos = 0;
    while pos < content.len() {
        let Some((at, m)) = MARKERS.iter().filter_map(|m| content[pos..].find(m).map(|i| (pos + i, *m))).min_by_key(|(i, _)| *i) else { break };
        let body = at + m.len();
        // In Backticks (`<tool_call>`) ist es Text über das Format, kein Aufruf.
        if content[..at].ends_with('`') {
            pos = body;
            continue;
        }
        let (inner_end, end) = match m {
            "<tool_call>" => close(content, body, &["</tool_call>"]),
            "<|tool_call|>" => close(content, body, &["<|/tool_call|>", "</tool_call>"]),
            "[TOOL_CALLS]" => {
                // Bis zum nächsten Aufruf (gehört nicht dazu) oder dem Ende-Zeichen `</s>` (gehört dazu).
                let next = content[body..].find("[TOOL_CALLS]").map(|i| (body + i, body + i));
                let eos = content[body..].find("</s>").map(|i| (body + i, body + i + 4));
                [next, eos].into_iter().flatten().min().unwrap_or((content.len(), content.len()))
            }
            "<|python_tag|>" => close(content, body, &["<|eom_id|>", "<|eot_id|>"]),
            "<function=" => close(content, body, &["</function>"]),
            _ => close(content, body, &[DS_END]),
        };
        let inner = &content[body..inner_end];
        let parsed = match m {
            "<tool_call>" | "<|tool_call|>" => json_calls(inner),
            "[TOOL_CALLS]" => mistral(inner),
            "<|python_tag|>" => python_tag(inner),
            "<function=" => function_tag(inner),
            _ => deepseek(inner),
        };
        if let Some(cs) = parsed {
            calls.extend(cs);
            spans.push((at, end));
        }
        pos = end.max(body);
    }
    (calls, spans)
}

/// Ende eines Bereichs ab `from`: (Ende des Inhalts, Ende samt Schluss-Marke); ohne Schluss-Marke das Textende
/// (abgeschnittene Antwort).
fn close(content: &str, from: usize, closers: &[&str]) -> (usize, usize) {
    closers.iter().filter_map(|c| content[from..].find(c).map(|i| (from + i, from + i + c.len()))).min().unwrap_or((content.len(), content.len()))
}

/// Text ohne die Bereiche der Aufrufe (und ohne DeepSeek-Hüllen), getrimmt.
fn strip(content: &str, spans: &[(usize, usize)]) -> String {
    let mut out = String::new();
    let mut last = 0;
    for &(a, b) in spans {
        if a >= last {
            out.push_str(&content[last..a]);
            last = b;
        }
    }
    out.push_str(&content[last.min(content.len())..]);
    for w in DS_WRAP {
        out = out.replace(w, "");
    }
    out.trim().to_string()
}

/// Erstes JSON am Anfang von `s` und wie viele Bytes es belegt.
fn json_prefix(s: &str) -> Option<(Value, usize)> {
    let mut it = serde_json::Deserializer::from_str(s).into_iter::<Value>();
    match it.next() {
        Some(Ok(v)) => Some((v, it.byte_offset())),
        _ => None,
    }
}

/// Aufeinanderfolgende JSON-Objekte/-Listen (getrennt durch Leerraum, `,` oder `;`); Rest danach wird übergangen.
fn json_values(t: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut s = t;
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == ';');
        if !(s.starts_with('{') || s.starts_with('[')) {
            break;
        }
        match json_prefix(s) {
            Some((v, n)) => {
                out.push(v);
                s = &s[n..];
            }
            None => break,
        }
    }
    out
}

/// Name und Argumente aus einem aufruf-förmigen Objekt: `name`/`tool_name`/`tool` und
/// `arguments`/`parameters`/`args`/`input`, auch verschachtelt als `{"function": {…}}` (OpenAI-Form).
fn call_parts(v: &Value) -> Option<(Option<String>, Option<Value>)> {
    let o = v.as_object()?;
    let f = o.get("function").and_then(Value::as_object);
    let name = f
        .and_then(|f| ["name", "tool_name"].iter().find_map(|k| f.get(*k).and_then(Value::as_str)))
        .or_else(|| ["name", "tool_name", "tool"].iter().find_map(|k| o.get(*k).and_then(Value::as_str)))
        .or_else(|| o.get("function").and_then(Value::as_str))
        .map(|n| n.trim().to_string());
    let args_of = |m: &Map<String, Value>| ["arguments", "parameters", "args", "input"].iter().find_map(|k| m.get(*k).cloned());
    let args = f.and_then(args_of).or_else(|| args_of(o));
    Some((name, args))
}

/// Argumente eines Textaufrufs: `null` → `{}`, sonst unverändert (Texte normalisiert [`normalize`] später).
fn args_value(a: Option<Value>) -> Value {
    match a {
        None | Some(Value::Null) => json!({}),
        Some(v) => v,
    }
}

/// Aufruf mit unlesbarem JSON: Name, soweit er zu finden ist, der Text als Argument.
fn broken(t: &str) -> TextCall {
    TextCall { name: scan_name(t).unwrap_or_default(), arguments: Value::String(t.trim().chars().take(RAW_MAX).collect()) }
}

/// `"name": "…"` (auch mit einfachen Anführungszeichen) in kaputtem JSON finden.
fn scan_name(t: &str) -> Option<String> {
    for q in ['"', '\''] {
        let key = format!("{q}name{q}");
        let mut from = 0;
        while let Some(i) = t[from..].find(&key) {
            let rest = t[from + i + key.len()..].trim_start();
            if let Some(r) = rest.strip_prefix(':') {
                let r = r.trim_start();
                if let Some(r) = r.strip_prefix(q) {
                    if let Some(end) = r.find(q) {
                        let n = r[..end].trim();
                        if is_ident(n) {
                            return Some(n.to_string());
                        }
                    }
                }
            }
            from += i + key.len();
        }
    }
    None
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Führender Name (`[A-Za-z0-9_.-]+`) und der Rest.
fn lead_ident(s: &str) -> (&str, &str) {
    let n = s.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or(s.len());
    (&s[..n], &s[n..])
}

/// Inhalt von `<tool_call>…</tool_call>` / `<|tool_call|>…`: JSON (Objekt, Liste, mehrere) oder die XML-Form mit
/// `<function=…>`. `None`, wenn es gar nicht nach einem Aufruf aussieht (dann bleibt der Text stehen).
fn json_calls(inner: &str) -> Option<Vec<TextCall>> {
    let t = inner.trim();
    if let Some(f) = t.strip_prefix("<function=") {
        return function_tag(f);
    }
    if !(t.starts_with('{') || t.starts_with('[')) {
        return None;
    }
    let mut calls = Vec::new();
    for v in json_values(t) {
        let items = match v {
            Value::Array(a) => a,
            v => vec![v],
        };
        for it in items {
            match call_parts(&it) {
                Some((name, args)) => calls.push(TextCall { name: name.unwrap_or_default(), arguments: args_value(args) }),
                None => calls.push(TextCall { name: String::new(), arguments: Value::String(mcp::short(&it.to_string(), RAW_MAX)) }),
            }
        }
    }
    if calls.is_empty() {
        calls.push(broken(t));
    }
    Some(calls)
}

/// Nach `<function=`: `name>` und dann JSON oder `<parameter=k>v</parameter>`-Paare (Werte als Text; die Typen
/// korrigiert [`normalize`] nach dem Schema).
fn function_tag(s: &str) -> Option<Vec<TextCall>> {
    let gt = s.find('>')?;
    let name = s[..gt].trim().trim_matches(|c| c == '"' || c == '\'');
    if !is_ident(name) {
        return None;
    }
    let body = &s[gt + 1..];
    let body = &body[..body.find("</function>").unwrap_or(body.len())];
    let b = body.trim();
    let arguments = if b.is_empty() {
        json!({})
    } else if b.starts_with('{') {
        match json_prefix(b) {
            Some((v @ Value::Object(_), _)) => v,
            _ => Value::String(b.chars().take(RAW_MAX).collect()),
        }
    } else if b.contains("<parameter=") {
        let mut m = Map::new();
        let mut rest = b;
        while let Some(i) = rest.find("<parameter=") {
            let r = &rest[i + "<parameter=".len()..];
            let Some(gt) = r.find('>') else { break };
            let key = r[..gt].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            let after = &r[gt + 1..];
            let end = [after.find("</parameter>"), after.find("<parameter=")].into_iter().flatten().min().unwrap_or(after.len());
            if !key.is_empty() {
                m.insert(key, Value::String(after[..end].trim().to_string()));
            }
            rest = &after[end..];
        }
        Value::Object(m)
    } else {
        Value::String(b.chars().take(RAW_MAX).collect())
    };
    Some(vec![TextCall { name: name.to_string(), arguments }])
}

/// Nach `[TOOL_CALLS]`: JSON-Liste/-Objekt (ältere Vorlagen) oder `name[ARGS]{…}` bzw. `name{…}` (neuere).
fn mistral(inner: &str) -> Option<Vec<TextCall>> {
    let t = inner.trim();
    if t.starts_with('[') || t.starts_with('{') {
        return json_calls(t);
    }
    let (name, rest) = lead_ident(t);
    if name.is_empty() {
        return None;
    }
    let mut rest = rest.trim_start();
    if let Some(r) = rest.strip_prefix("[CALL_ID]") {
        rest = r.find("[ARGS]").map_or("", |i| &r[i..]);
    }
    let rest = rest.strip_prefix("[ARGS]").unwrap_or(rest).trim();
    let arguments = if rest.is_empty() {
        json!({})
    } else if rest.starts_with('{') {
        match json_prefix(rest) {
            Some((v @ Value::Object(_), _)) => v,
            _ => Value::String(rest.chars().take(RAW_MAX).collect()),
        }
    } else {
        // `[TOOL_CALLS] marks a call.` – Text über das Format, kein Aufruf.
        return None;
    };
    Some(vec![TextCall { name: name.to_string(), arguments }])
}

/// Nach `<|python_tag|>`: JSON (`{"name": …, "parameters": …}`, auch mehrere mit `;`) oder die eingebaute
/// Python-Form `werkzeug.call(…)` (unbekanntes Werkzeug → Hinweis).
fn python_tag(inner: &str) -> Option<Vec<TextCall>> {
    let t = inner.trim();
    if t.starts_with('{') || t.starts_with('[') {
        return json_calls(t);
    }
    let (name, _) = lead_ident(t);
    let name = name.split('.').next().unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    Some(vec![TextCall { name: name.to_string(), arguments: Value::String(t.chars().take(RAW_MAX).collect()) }])
}

/// DeepSeek: `function<｜tool▁sep｜>name\n```json\n{…}\n```` (V3) oder `name<｜tool▁sep｜>{…}` (V3.1).
fn deepseek(inner: &str) -> Option<Vec<TextCall>> {
    let Some((left, right)) = inner.split_once(DS_SEP) else { return Some(vec![broken(inner)]) };
    let (name, args) = if left.trim() == "function" {
        let r = right.trim_start();
        let (n, a) = r.split_once('\n').unwrap_or((r, ""));
        (n.trim(), a)
    } else {
        (left.trim(), right)
    };
    if !is_ident(name) {
        return Some(vec![broken(inner)]);
    }
    let a = unfence(args.trim());
    let arguments = if a.is_empty() {
        json!({})
    } else {
        match json_prefix(a) {
            Some((v @ Value::Object(_), _)) => v,
            _ => Value::String(a.chars().take(RAW_MAX).collect()),
        }
    };
    Some(vec![TextCall { name: name.to_string(), arguments }])
}

/// ```` ```json\n…\n``` ```` → der Inhalt (ohne Zaun unverändert).
fn unfence(s: &str) -> &str {
    let Some(r) = s.strip_prefix("```") else { return s };
    let (_, r) = lead_ident(r);
    let r = r.strip_suffix("```").unwrap_or(r);
    r.trim()
}

/// Codeblöcke: (Anfang, Ende, Sprachangabe, Inhalt). Nur geschlossene Blöcke.
fn fences(content: &str) -> Vec<(usize, usize, String, &str)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = content[pos..].find("```") {
        let start = pos + i;
        let after = &content[start + 3..];
        let (info, _) = lead_ident(after);
        let body_start = start + 3 + info.len();
        let Some(j) = content[body_start..].find("```") else { break };
        let body_end = body_start + j;
        out.push((start, body_end + 3, info.to_ascii_lowercase(), &content[body_start..body_end]));
        pos = body_end + 3;
    }
    out
}

/// Schwache Formen: Aufrufe nur mit bekanntem Werkzeug und Argument-Feld; sonst `None`.
fn weak_calls(v: &Value, known: &dyn Fn(&str) -> bool) -> Option<Vec<TextCall>> {
    let items: Vec<&Value> = match v {
        Value::Array(a) if !a.is_empty() => a.iter().collect(),
        Value::Object(_) => vec![v],
        _ => return None,
    };
    let mut calls = Vec::new();
    for it in items {
        let (Some(name), Some(args)) = call_parts(it)? else { return None };
        if !known(&name) || !(args.is_object() || args.is_null() || args.as_str().is_some_and(|s| s.trim_start().starts_with('{'))) {
            return None;
        }
        calls.push(TextCall { name, arguments: args_value(Some(args)) });
    }
    Some(calls)
}

fn weak(content: &str, known: &dyn Fn(&str) -> bool) -> Option<Found> {
    let t = content.trim();
    // Die ganze Antwort ist der Aufruf (Llama: `{"name": …, "parameters": …}`).
    if t.starts_with('{') || t.starts_with('[') {
        let v: Value = serde_json::from_str(t).ok()?;
        return weak_calls(&v, known).map(|calls| Found { calls, rest: String::new() });
    }
    let mut calls = Vec::new();
    let mut spans = Vec::new();
    for (a, b, info, body) in fences(content) {
        if !WEAK_INFO.contains(&info.as_str()) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(body.trim()) else { continue };
        match weak_calls(&v, known) {
            Some(cs) => {
                calls.extend(cs);
                spans.push((a, b));
            }
            // Ein aufruf-förmiges Objekt mit unbekanntem Werkzeug: eher ein Beispiel – dann gar nichts erkennen.
            None if call_parts(&v).is_some_and(|(n, a)| n.is_some() && a.is_some()) => return None,
            None => {}
        }
    }
    let (first, last) = (spans.first()?.0, spans.last()?.1);
    if !content[last..].trim().is_empty() || content[..first].trim().chars().count() > WEAK_PROSE_CHARS {
        return None;
    }
    calls.truncate(crate::llm::MAX_TOOL_CALLS);
    Some(Found { calls, rest: strip(content, &spans) })
}

// ---- Argumente normalisieren und prüfen

/// Der eine Typ eines Schema-Feldes (`"integer"`, oder `["array", "null"]` → `"array"`); mehrdeutig → `None`.
fn single_type(ps: &Value) -> Option<&str> {
    match ps.get("type")? {
        Value::String(s) => Some(s),
        Value::Array(a) => {
            let t: Vec<&str> = a.iter().filter_map(Value::as_str).filter(|t| *t != "null").collect();
            (t.len() == 1).then(|| t[0])
        }
        _ => None,
    }
}

/// Argumente des Aufrufs gegen das Schema seines Werkzeugs normalisieren (nur Eindeutiges, siehe
/// [`normalize_with`]). Gibt die Korrekturen zurück; ein unbekanntes Werkzeug bleibt unverändert.
pub fn normalize(call: &mut ToolCall) -> Vec<String> {
    // Agenten-Werkzeuge des Chats (#338) haben ein eigenes Schema.
    if let Some(schema) = super::agent::schema_for(call) {
        return normalize_with(&schema, &mut call.arguments);
    }
    // `more_tools` (#393) gibt es nur in Chats.
    if call.name == super::more_tools::NAME {
        return normalize_with(&super::more_tools::schema(), &mut call.arguments);
    }
    match mcp::tools().into_iter().find(|d| d.name == call.name) {
        Some(def) => normalize_with(&def.schema, &mut call.arguments),
        None => Vec::new(),
    }
}

/// Eindeutige Korrekturen: ganze Argumente als JSON-Text → Objekt (auch doppelt verpackt); je Feld nach seinem Typ
/// `"3"` → `3`, `3.0` → `3` (integer), `"2.5"` → `2.5` (number), `"true"`/`"false"` → Wahrheitswert, Zahl/Wahrheitswert →
/// Text (string), ein einzelner Wert → `[Wert]` und eine Liste als JSON-Text → Liste (array), ein Objekt als JSON-Text →
/// Objekt (object); Listeneinträge und Unterobjekte ebenso. Alles andere (z. B. `"3.5"` für eine Ganzzahl, `"ja"`) bleibt
/// stehen und wird von [`validate`] gemeldet. Korrekturen als `feld: alt → neu` (JSON, gekürzt).
pub fn normalize_with(schema: &Value, args: &mut Value) -> Vec<String> {
    let mut fixes = Vec::new();
    if let Value::String(s) = &*args {
        if let Some(o) = object_text(s) {
            fixes.push(fix("arguments", args, &o));
            *args = o;
        }
    }
    if let Value::Object(m) = args {
        norm_props(schema, m, "", &mut fixes);
    }
    fixes
}

/// JSON-Text eines Objekts (auch ein JSON-Text, der selbst ein JSON-Text ist) → das Objekt.
fn object_text(s: &str) -> Option<Value> {
    let mut v: Value = serde_json::from_str(s.trim()).ok()?;
    if let Value::String(inner) = &v {
        v = serde_json::from_str(inner.trim()).ok()?;
    }
    v.is_object().then_some(v)
}

fn norm_props(schema: &Value, m: &mut Map<String, Value>, prefix: &str, fixes: &mut Vec<String>) {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return };
    for (k, v) in m.iter_mut() {
        if let Some(ps) = props.get(k) {
            norm_value(ps, v, &format!("{prefix}{k}"), fixes);
        }
    }
}

fn norm_value(ps: &Value, v: &mut Value, path: &str, fixes: &mut Vec<String>) {
    if v.is_null() {
        return;
    }
    let Some(ty) = single_type(ps) else { return };
    let new = match (ty, &*v) {
        ("integer", Value::String(s)) => int_text(s),
        ("integer", Value::Number(n)) if !n.is_i64() && !n.is_u64() => n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| Value::from(f as i64)),
        ("number", Value::String(s)) => int_text(s).or_else(|| num_text(s)),
        ("boolean", Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        ("string", Value::Number(n)) => Some(Value::String(n.to_string())),
        ("string", Value::Bool(b)) => Some(Value::String(b.to_string())),
        ("array", Value::Array(_)) => None,
        ("array", Value::String(s)) => Some(serde_json::from_str::<Value>(s.trim()).ok().filter(Value::is_array).unwrap_or_else(|| Value::Array(vec![v.clone()]))),
        ("array", _) => Some(Value::Array(vec![v.clone()])),
        ("object", Value::String(s)) => object_text(s),
        _ => None,
    };
    if let Some(n) = new {
        fixes.push(fix(path, v, &n));
        *v = n;
    }
    match v {
        Value::Array(items) => {
            if let Some(is) = ps.get("items") {
                for (i, it) in items.iter_mut().enumerate() {
                    norm_value(is, it, &format!("{path}[{i}]"), fixes);
                }
            }
        }
        Value::Object(m) if ty == "object" => norm_props(ps, m, &format!("{path}."), fixes),
        _ => {}
    }
}

/// `" 42 "` → 42, `"-3"` → -3 (nur Ziffern, höchstens 18).
fn int_text(s: &str) -> Option<Value> {
    let t = s.trim();
    let d = t.strip_prefix(['+', '-']).unwrap_or(t);
    if d.is_empty() || d.len() > 18 || !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.trim_start_matches('+').parse::<i64>().ok().map(Value::from)
}

/// `"2.5"` → 2.5, `"1e3"` → 1000.0 (nur Zahlzeichen, endlich).
fn num_text(s: &str) -> Option<Value> {
    let t = s.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) || !t.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<f64>().ok().filter(|f| f.is_finite()).and_then(serde_json::Number::from_f64).map(Value::Number)
}

fn fix(path: &str, old: &Value, new: &Value) -> String {
    format!("{path}: {} → {}", mcp::short(&old.to_string(), FIX_VALUE_CHARS), mcp::short(&new.to_string(), FIX_VALUE_CHARS))
}

/// Prüft die (schon normalisierten) Argumente gegen das Schema: ein Objekt (`null` = keine Argumente), alle Pflichtfelder
/// vorhanden, die Typen stimmen. Unbekannte Felder allein sind kein Fehler (der Ausführer übergeht sie), werden aber in
/// einer Fehlermeldung genannt. `Err(Meldung)` für `bad_args` (englisch, für das Modell): was falsch ist, die Felder
/// des Werkzeugs und ein richtiges Beispiel.
pub fn validate(tool: &str, schema: &Value, args: &Value) -> Result<(), String> {
    let empty = Map::new();
    let m = match args {
        Value::Null => &empty,
        Value::Object(m) => m,
        Value::String(s) => {
            let why = match serde_json::from_str::<Value>(s) {
                Err(e) => format!("the text is not valid JSON ({e})"),
                Ok(_) => "the text is JSON, but not an object".to_string(),
            };
            return Err(format!("arguments must be a JSON object; {why}. Received: {}. {}", mcp::short(s, 200), usage(tool, schema, &[])));
        }
        other => return Err(format!("arguments must be a JSON object, not {}. {}", mcp::short(&other.to_string(), 80), usage(tool, schema, &[]))),
    };
    let props = schema.get("properties").and_then(Value::as_object);
    let prop = |k: &str| props.and_then(|p| p.get(k));
    let mut problems = Vec::new();
    let mut fields: Vec<&str> = Vec::new();
    for r in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if m.get(r).is_none_or(Value::is_null) {
            problems.push(format!("missing required field \"{r}\" ({})", prop(r).map(type_desc).unwrap_or_else(|| "a value".into())));
        }
    }
    let mut unknown = Vec::new();
    for (k, v) in m {
        match prop(k) {
            None => unknown.push(format!("\"{k}\"")),
            Some(ps) => {
                if let Some(p) = type_problem(k, ps, v) {
                    problems.push(p);
                    fields.push(k);
                }
            }
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    if !unknown.is_empty() {
        problems.push(format!("unknown field(s) {} (use only the field names below)", unknown.join(", ")));
    }
    Err(format!("Invalid arguments for {tool}: {}. {}", problems.join("; "), usage(tool, schema, &fields)))
}

/// Falscher Typ eines Feldes (auch eines Listeneintrags) als Satz; `None`, wenn er passt oder mehrdeutig ist.
fn type_problem(path: &str, ps: &Value, v: &Value) -> Option<String> {
    if v.is_null() {
        return None;
    }
    let ok = match single_type(ps)? {
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "string" => v.is_string(),
        "object" => v.is_object(),
        "array" => {
            let items = ps.get("items");
            return match (v.as_array(), items) {
                (None, _) => Some(format!("field \"{path}\" must be {} (got {})", type_desc(ps), mcp::short(&v.to_string(), FIX_VALUE_CHARS))),
                (Some(a), Some(is)) => a.iter().enumerate().find_map(|(i, it)| type_problem(&format!("{path}[{i}]"), is, it)),
                _ => None,
            };
        }
        _ => true,
    };
    (!ok).then(|| format!("field \"{path}\" must be {} (got {})", type_desc(ps), mcp::short(&v.to_string(), FIX_VALUE_CHARS)))
}

/// Erwarteter Typ in Worten: „an integer“, „one of "a", "b"“, „a list of strings“ …
fn type_desc(ps: &Value) -> String {
    if let Some(e) = ps.get("enum").and_then(Value::as_array) {
        return format!("one of {}", e.iter().map(Value::to_string).collect::<Vec<_>>().join(", "));
    }
    match single_type(ps) {
        Some("integer") => "an integer (a JSON number like 3, not \"3\")".into(),
        Some("number") => "a number".into(),
        Some("boolean") => "true or false (not \"true\")".into(),
        Some("string") => "a string".into(),
        Some("object") => "a JSON object".into(),
        Some("array") => match ps.get("items") {
            Some(is) => format!("a JSON list whose entries are {}", type_desc(is)),
            None => "a JSON list".into(),
        },
        _ => "a value".into(),
    }
}

/// Kurzer Typname für die Feldliste.
fn type_name(ps: &Value) -> String {
    if let Some(e) = ps.get("enum").and_then(Value::as_array) {
        return e.iter().map(Value::to_string).collect::<Vec<_>>().join("|");
    }
    match ps.get("type") {
        Some(Value::String(s)) if s == "array" => format!("list of {}", ps.get("items").map(type_name).unwrap_or_else(|| "values".into())),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" or "),
        _ => "value".into(),
    }
}

/// „Fields of x: a (integer, required), b (boolean). Correct example: {…}“ – `extra` = Felder, die zusätzlich ins
/// Beispiel kommen (die falschen).
fn usage(tool: &str, schema: &Value, extra: &[&str]) -> String {
    let required: Vec<&str> = schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let props = schema.get("properties").and_then(Value::as_object);
    let sig: Vec<String> = props.into_iter().flatten().map(|(k, ps)| if required.contains(&k.as_str()) { format!("{k} ({}, required)", type_name(ps)) } else { format!("{k} ({})", type_name(ps)) }).collect();
    let sig = if sig.is_empty() { "none".to_string() } else { mcp::short(&sig.join(", "), SIGNATURE_CHARS) };
    format!("Fields of {tool}: {sig}. Correct example: {}", example(schema, extra))
}

/// Kleinstes gültiges Beispiel aus dem Schema: alle Pflichtfelder (und `extra`) mit Platzhaltern ihres Typs.
pub fn example(schema: &Value, extra: &[&str]) -> Value {
    let mut m = Map::new();
    let props = schema.get("properties").and_then(Value::as_object);
    let required = schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
    for k in required.chain(extra.iter().copied()) {
        if let Some(ps) = props.and_then(|p| p.get(k)) {
            m.insert(k.to_string(), placeholder(ps));
        }
    }
    Value::Object(m)
}

fn placeholder(ps: &Value) -> Value {
    if let Some(first) = ps.get("enum").and_then(Value::as_array).and_then(|e| e.first()) {
        return first.clone();
    }
    match single_type(ps) {
        Some("integer") | Some("number") => json!(1),
        Some("boolean") => json!(true),
        Some("array") => Value::Array(ps.get("items").map(placeholder).into_iter().collect()),
        Some("object") => json!({}),
        _ => json!("text"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(n: &str) -> bool {
        mcp::tool_level(n).is_some()
    }

    fn find(s: &str) -> Option<Found> {
        find_text_calls(s, &known)
    }

    fn one(s: &str) -> (TextCall, String) {
        let f = find(s).unwrap_or_else(|| panic!("nicht erkannt: {s}"));
        assert_eq!(f.calls.len(), 1, "{s}: {:?}", f.calls);
        (f.calls[0].clone(), f.rest)
    }

    fn tc(name: &str, args: Value) -> TextCall {
        TextCall { name: name.into(), arguments: args }
    }

    fn schema_of(name: &str) -> Value {
        mcp::tools().into_iter().find(|d| d.name == name).unwrap().schema
    }

    // ---- Aufgezeichnete typische Antworten je Modellfamilie

    #[test]
    fn qwen_and_hermes_tool_call_tags() {
        // qwen2.5/qwen3 (Hermes-Vorlage), wenn Ollama den Aufruf nicht selbst erkennt
        let (c, rest) = one("<tool_call>\n{\"name\": \"list_sessions\", \"arguments\": {}}\n</tool_call>");
        assert_eq!((c, rest.as_str()), (tc("list_sessions", json!({})), ""));
        // mit Text davor; zwei Aufrufe
        let f = find("Let me look.\n<tool_call>\n{\"name\": \"read_terminal\", \"arguments\": {\"session\": 3, \"lines\": 40}}\n</tool_call>\n<tool_call>\n{\"name\": \"whoami\", \"arguments\": {}}\n</tool_call>").unwrap();
        assert_eq!(f.calls, [tc("read_terminal", json!({"session": 3, "lines": 40})), tc("whoami", json!({}))]);
        assert_eq!(f.rest, "Let me look.");
        // abgeschnitten (kein Schluss-Tag)
        assert_eq!(one("<tool_call>{\"name\": \"list_sessions\", \"arguments\": {}}").0, tc("list_sessions", json!({})));
        // Qwen3-Coder: XML-Form, Werte als Text
        let (c, _) = one("<tool_call>\n<function=read_terminal>\n<parameter=session>\n3\n</parameter>\n<parameter=screen>\ntrue\n</parameter>\n</function>\n</tool_call>");
        assert_eq!(c, tc("read_terminal", json!({"session": "3", "screen": "true"})));
        // Kaputtes JSON (Komma am Ende): erkannt, Name gefunden, Text als Argument (→ bad_args mit Hinweis)
        let (c, _) = one("<tool_call>{\"name\": \"read_terminal\", \"arguments\": {\"session\": 3,}}</tool_call>");
        assert_eq!(c.name, "read_terminal");
        assert!(c.arguments.as_str().is_some_and(|s| s.contains("\"session\": 3,")), "{c:?}");
        // Python-Wörterbuch statt JSON
        let (c, _) = one("<tool_call>\n{'name': 'list_sessions', 'arguments': {}}\n</tool_call>");
        assert_eq!((c.name.as_str(), c.arguments.is_string()), ("list_sessions", true));
        // Ohne Namen
        let (c, _) = one("<tool_call>{\"arguments\": {\"session\": 1}}</tool_call>");
        assert_eq!(c, tc("", json!({"session": 1})));
    }

    #[test]
    fn mistral_tool_calls_marker() {
        let (c, rest) = one("[TOOL_CALLS][{\"name\": \"list_sessions\", \"arguments\": {}}]</s>");
        assert_eq!((c, rest.as_str()), (tc("list_sessions", json!({})), ""));
        // neuere Vorlage (v11): name[ARGS]{…}; auch mehrere
        let f = find("[TOOL_CALLS]read_terminal[ARGS]{\"session\": 2, \"lines\": \"20\"}[TOOL_CALLS]whoami[ARGS]{}").unwrap();
        assert_eq!(f.calls, [tc("read_terminal", json!({"session": 2, "lines": "20"})), tc("whoami", json!({}))]);
        // ohne [ARGS], mit [CALL_ID]
        assert_eq!(one("[TOOL_CALLS]read_terminal{\"session\": 2}").0, tc("read_terminal", json!({"session": 2})));
        assert_eq!(one("[TOOL_CALLS]read_terminal[CALL_ID]abc123[ARGS]{\"session\": 2}").0, tc("read_terminal", json!({"session": 2})));
        // ministral: Liste mit zwei Aufrufen
        let f = find("[TOOL_CALLS] [{\"name\": \"whoami\", \"arguments\": {}}, {\"name\": \"list_todos\", \"arguments\": {}}]").unwrap();
        assert_eq!(f.calls.len(), 2);
    }

    #[test]
    fn llama_python_tag_function_tag_and_bare_json() {
        let (c, rest) = one("<|python_tag|>{\"name\": \"list_sessions\", \"parameters\": {}}<|eom_id|>");
        assert_eq!((c, rest.as_str()), (tc("list_sessions", json!({})), ""));
        // mehrere mit Semikolon
        assert_eq!(find("<|python_tag|>{\"name\": \"whoami\", \"parameters\": {}}; {\"name\": \"list_sessions\", \"parameters\": {}}").unwrap().calls.len(), 2);
        // eingebautes Python-Werkzeug: unbekannt → Aufruf mit Text (Hinweis)
        let (c, _) = one("<|python_tag|>brave_search.call(query=\"weather\")");
        assert_eq!(c.name, "brave_search");
        assert!(c.arguments.is_string());
        // <function=…>
        assert_eq!(one("<function=read_terminal>{\"session\": 4}</function>").0, tc("read_terminal", json!({"session": 4})));
        // Llama 3.x JSON-Aufruf als ganze Antwort (ohne Marke)
        let (c, rest) = one("{\"name\": \"read_terminal\", \"parameters\": {\"session\": \"4\", \"lines\": 30}}");
        assert_eq!((c, rest.as_str()), (tc("read_terminal", json!({"session": "4", "lines": 30})), ""));
        // OpenAI-Form, Argumente als JSON-Text
        assert_eq!(one("{\"type\": \"function\", \"function\": {\"name\": \"whoami\", \"arguments\": \"{}\"}}").0, tc("whoami", json!("{}")));
    }

    #[test]
    fn gemma_phi_granite_and_deepseek_formats() {
        // gemma3: Codeblock am Ende
        let (c, rest) = one("I'll list the sessions.\n```json\n{\"name\": \"list_sessions\", \"arguments\": {}}\n```");
        assert_eq!((c, rest.as_str()), (tc("list_sessions", json!({})), "I'll list the sessions."));
        let (c, _) = one("```tool_code\n{\"name\": \"read_terminal\", \"parameters\": {\"session\": 2}}\n```\n");
        assert_eq!(c, tc("read_terminal", json!({"session": 2})));
        // Block ohne Sprachangabe, Liste
        assert_eq!(find("```\n[{\"name\": \"whoami\", \"arguments\": {}}]\n```").unwrap().calls, [tc("whoami", json!({}))]);
        // phi-4-mini / granite
        assert_eq!(one("<|tool_call|>[{\"name\": \"list_sessions\", \"arguments\": {}}]<|/tool_call|>").0, tc("list_sessions", json!({})));
        assert_eq!(one("<|tool_call|>[{\"name\": \"whoami\", \"arguments\": {}}]").0, tc("whoami", json!({})));
        // deepseek-v3: Marken mit Codeblock; deepseek-r1-distill: Codeblock nach kurzem Text
        let ds = "<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>read_terminal\n```json\n{\"session\": 5}\n```<｜tool▁call▁end｜><｜tool▁calls▁end｜>";
        let (c, rest) = one(ds);
        assert_eq!((c, rest.as_str()), (tc("read_terminal", json!({"session": 5})), ""));
        assert_eq!(one("<｜tool▁call▁begin｜>whoami<｜tool▁sep｜>{}<｜tool▁call▁end｜>").0, tc("whoami", json!({})));
        assert_eq!(one("Sure.\n\n```json\n{\"name\": \"whoami\", \"arguments\": {}}\n```\n").0, tc("whoami", json!({})));
    }

    #[test]
    fn prose_that_only_mentions_json_or_tools_triggers_nothing() {
        for s in [
            "You can call list_sessions to see your sessions.",
            "Here is the JSON:\n```json\n{\"name\": \"Alice\", \"age\": 3}\n```",
            "Die Menge {a, b} ist leer.",
            "{\"name\": \"Alice\", \"age\": 3}",
            "Qwen writes tool calls in `<tool_call>` tags, Mistral uses `[TOOL_CALLS]`.",
            // Beispiel mitten im Text (danach geht der Text weiter)
            "To list sessions an agent sends:\n```json\n{\"name\": \"list_sessions\", \"arguments\": {}}\n```\nThe answer contains the ids of all sessions.",
            // unbekanntes Werkzeug in einem Block
            "```json\n{\"name\": \"get_weather\", \"arguments\": {\"city\": \"Berlin\"}}\n```",
            // ohne Argument-Feld
            "```json\n{\"name\": \"list_sessions\"}\n```",
            // ganze Antwort JSON mit unbekanntem Werkzeug
            "{\"name\": \"get_weather\", \"parameters\": {}}",
            "<tool_call> is the tag qwen uses.",
            "[TOOL_CALLS] marks a call.",
            "",
        ] {
            assert_eq!(find(s), None, "{s}");
        }
        // Zu viel Text vor dem Block
        let long = format!("{}\n```json\n{{\"name\": \"list_sessions\", \"arguments\": {{}}}}\n```", "word ".repeat(100));
        assert_eq!(find(&long), None);
        // Ein Block mit unbekanntem Werkzeug neben einem bekannten: lieber gar nichts
        assert_eq!(find("```json\n{\"name\": \"get_weather\", \"arguments\": {}}\n```\n```json\n{\"name\": \"whoami\", \"arguments\": {}}\n```"), None);
    }

    // ---- Normalisieren

    fn norm(tool: &str, args: Value) -> (Value, Vec<String>) {
        let mut c = ToolCall { id: "c".into(), name: tool.into(), arguments: args };
        let fixes = normalize(&mut c);
        (c.arguments, fixes)
    }

    #[test]
    fn unambiguous_types_are_normalised_against_the_schema() {
        let (a, f) = norm("read_terminal", json!({"session": "3", "lines": " 50 ", "screen": "TRUE"}));
        assert_eq!(a, json!({"session": 3, "lines": 50, "screen": true}));
        let mut f = f;
        f.sort();
        assert_eq!(f, [r#"lines: " 50 " → 50"#, r#"screen: "TRUE" → true"#, r#"session: "3" → 3"#]);
        // 3.0 → 3
        assert_eq!(norm("read_terminal", json!({"session": 3.0})).0, json!({"session": 3}));
        // einzelner Wert → Liste, Liste als Text → Liste
        let (a, f) = norm("wait_for_status", json!({"session": 1, "status": "ready"}));
        assert_eq!(a, json!({"session": 1, "status": ["ready"]}));
        assert_eq!(f, [r#"status: "ready" → ["ready"]"#]);
        assert_eq!(norm("send_keys", json!({"session": 1, "keys": "[\"ctrl+c\", \"enter\"]"})).0, json!({"session": 1, "keys": ["ctrl+c", "enter"]}));
        // Listeneinträge werden mitnormalisiert
        assert_eq!(norm("send_keys", json!({"session": 1, "keys": [5]})).0, json!({"session": 1, "keys": ["5"]}));
        // Zahl → Text
        assert_eq!(norm("rename_session", json!({"session": 1, "name": 42})).0, json!({"session": 1, "name": "42"}));
        // ganze Argumente als JSON-Text, auch doppelt verpackt
        let (a, f) = norm("read_terminal", json!("{\"session\": \"7\"}"));
        assert_eq!(a, json!({"session": 7}));
        assert!(f[0].starts_with("arguments: ") && f[1] == r#"session: "7" → 7"#, "{f:?}");
        assert_eq!(norm("read_terminal", json!("\"{\\\"session\\\": 7}\"")).0, json!({"session": 7}));
        // Typ mit null: ["array", "null"] gilt als array
        let s = json!({"type": "object", "properties": {"labels": {"type": ["array", "null"], "items": {"type": "string"}}, "x": {"type": ["integer", "string"]}}});
        let mut a = json!({"labels": "bug", "x": "3"});
        assert_eq!(normalize_with(&s, &mut a), [r#"labels: "bug" → ["bug"]"#]);
        assert_eq!(a, json!({"labels": ["bug"], "x": "3"}), "mehrdeutiger Typ bleibt");
        // Unveränderliches: null, schon Richtiges, unbekanntes Werkzeug
        assert_eq!(norm("read_terminal", json!({"session": 3, "lines": null})), (json!({"session": 3, "lines": null}), vec![]));
        assert_eq!(norm("no_such_tool", json!({"a": "1"})), (json!({"a": "1"}), vec![]));
    }

    #[test]
    fn ambiguous_values_are_left_for_validation() {
        for (args, field) in [(json!({"session": "3.5"}), "session"), (json!({"session": "three"}), "session"), (json!({"session": 1, "screen": "ja"}), "screen"), (json!({"session": 1, "lines": 2.5}), "lines")] {
            let (a, f) = norm("read_terminal", args.clone());
            assert_eq!((a.clone(), f.len()), (args, 0));
            let e = validate("read_terminal", &schema_of("read_terminal"), &a).unwrap_err();
            assert!(e.contains(&format!("field \"{field}\" must be")) && e.contains("Correct example: {"), "{e}");
        }
    }

    // ---- Prüfen

    #[test]
    fn bad_args_name_the_field_the_type_and_an_example() {
        let s = schema_of("read_terminal");
        assert_eq!(validate("read_terminal", &s, &json!({"session": 3})), Ok(()));
        assert_eq!(validate("read_terminal", &s, &json!({"session": 3, "reason": "x"})), Ok(()), "unbekanntes Feld allein ist kein Fehler");
        let e = validate("read_terminal", &s, &json!({"session": "x"})).unwrap_err();
        assert!(e.contains("field \"session\" must be an integer") && e.contains(r#"Correct example: {"session":1}"#) && e.contains("session (integer, required)"), "{e}");
        // fehlendes Pflichtfeld, falsch geschriebenes Feld genannt
        let e = validate("read_terminal", &s, &json!({"session_id": 3})).unwrap_err();
        assert!(e.contains("missing required field \"session\"") && e.contains("unknown field(s) \"session_id\""), "{e}");
        let e = validate("read_terminal", &s, &Value::Null).unwrap_err();
        assert!(e.contains("missing required field \"session\""), "{e}");
        // kaputtes JSON: nie still {}, sondern Fehler mit dem empfangenen Text und Beispiel
        let e = validate("read_terminal", &s, &json!("{\"session\": 3,}")).unwrap_err();
        assert!(e.starts_with("arguments must be a JSON object; the text is not valid JSON") && e.contains("Received: {\"session\": 3,}") && e.contains("Correct example"), "{e}");
        assert!(validate("read_terminal", &s, &json!([1])).unwrap_err().starts_with("arguments must be a JSON object, not [1]"));
        // Listeneinträge, Aufzählungen im Beispiel
        let w = schema_of("wait_for_status");
        let e = validate("wait_for_status", &w, &json!({"session": 1, "status": [3]})).unwrap_err();
        assert!(e.contains("field \"status[0]\" must be one of \"ready\"") && e.contains(r#""status":["ready"]"#), "{e}");
        // Werkzeug ohne Felder
        assert_eq!(validate("whoami", &schema_of("whoami"), &json!({})), Ok(()));
        assert_eq!(example(&schema_of("whoami"), &[]), json!({}));
    }

    #[test]
    fn every_tool_example_is_valid_and_its_usage_stays_short() {
        for d in mcp::tools() {
            let ex = example(&d.schema, &[]);
            assert_eq!(validate(d.name, &d.schema, &ex), Ok(()), "{}: {ex}", d.name);
            assert!(usage(d.name, &d.schema, &[]).chars().count() < SIGNATURE_CHARS + 300, "{}", d.name);
        }
    }
}
