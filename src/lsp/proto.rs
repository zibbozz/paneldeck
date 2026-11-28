//! Protokoll des Language Server Protocol (#367, 3.3.0): Rahmen (`Content-Length`), JSON-RPC-Nachrichten und die
//! wenigen LSP-Typen, die der Smart-Modus braucht (Initialisieren, Dokumente, Vervollständigen, Diagnosen) – schlank auf
//! `serde_json`, ohne eigene Crate. Positionen zählen wie in LSP 3.17 in UTF-16-Einheiten, sofern der Server nichts
//! anderes aushandelt (`positionEncoding`).

use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// Größte Nachricht, die gelesen wird (Grenze aus dem Issue, einstellbar über [`super::Limits`]).
pub const DEFAULT_MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// Fehler beim Lesen einer Nachricht.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Ende des Datenstroms (Server beendet).
    Eof,
    /// Kopf ohne gültige Länge oder Inhalt kein JSON.
    Malformed(String),
    /// Nachricht größer als erlaubt.
    TooLarge(usize),
    Io(String),
}

/// Eine Nachricht mit Rahmen schreiben.
pub fn write_message(w: &mut dyn Write, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Eine Nachricht lesen (Kopfzeilen bis zur Leerzeile, dann genau `Content-Length` Bytes).
pub fn read_message(r: &mut dyn BufRead, max: usize) -> Result<Value, ReadError> {
    let mut len: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = r.read_line(&mut line).map_err(|e| ReadError::Io(e.to_string()))?;
        if n == 0 {
            return Err(ReadError::Eof);
        }
        if line.len() > 4096 {
            return Err(ReadError::Malformed("header line too long".into()));
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(v.trim().parse().map_err(|_| ReadError::Malformed(format!("bad length {v:?}")))?);
        }
    }
    let len = len.unwrap_or(0);
    if len > max {
        return Err(ReadError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(|e| if e.kind() == std::io::ErrorKind::UnexpectedEof { ReadError::Eof } else { ReadError::Io(e.to_string()) })?;
    serde_json::from_slice(&buf).map_err(|e| ReadError::Malformed(e.to_string()))
}

/// Art einer eingehenden Nachricht.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// Antwort auf eine eigene Anfrage.
    Response { id: i64, result: Result<Value, (i64, String)> },
    /// Anfrage des Servers an den Client (`id` wie gesendet).
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
}

/// Eingehende Nachricht einordnen; `None` = unbrauchbar.
pub fn classify(v: Value) -> Option<Incoming> {
    let o = v.as_object()?;
    let method = o.get("method").and_then(Value::as_str).map(str::to_string);
    let params = o.get("params").cloned().unwrap_or(Value::Null);
    match (o.get("id"), method) {
        (Some(id), Some(method)) => Some(Incoming::Request { id: id.clone(), method, params }),
        (None, Some(method)) => Some(Incoming::Notification { method, params }),
        (Some(id), None) => {
            let id = id.as_i64()?;
            let result = match o.get("error") {
                Some(e) => Err((e.get("code").and_then(Value::as_i64).unwrap_or(0), e.get("message").and_then(Value::as_str).unwrap_or("").to_string())),
                None => Ok(o.get("result").cloned().unwrap_or(Value::Null)),
            };
            Some(Incoming::Response { id, result })
        }
        _ => None,
    }
}

pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// JSON-RPC: Methode unbekannt.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// LSP: Anfrage abgelehnt (`RequestFailed`).
pub const REQUEST_FAILED: i64 = -32803;

/// Anfragen des Servers, die Paneldeck in 3.3.0 immer ablehnt: sie würden Dateien ändern, Programme ausführen oder
/// Dateien/Links öffnen (Entscheidung zu #367).
pub const REFUSED_SERVER_REQUESTS: &[&str] = &["workspace/applyEdit", "window/showDocument", "workspace/executeCommand"];

/// Antwort auf eine Anfrage des Servers: harmlose bekommen eine leere Antwort, verweigerte und unbekannte einen Fehler.
pub fn answer_server_request(id: &Value, method: &str, params: &Value) -> Value {
    match method {
        "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" | "workspace/diagnostic/refresh" | "workspace/semanticTokens/refresh" | "workspace/inlayHint/refresh" | "workspace/codeLens/refresh" => response(id, Value::Null),
        // Auswahl-Frage des Servers: nie beantwortet (keine Aktion) – leere Antwort heißt „nichts gewählt“.
        "window/showMessageRequest" => response(id, Value::Null),
        // Einstellungen: für jeden erfragten Abschnitt `null` (Standards des Servers).
        "workspace/configuration" => {
            let n = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
            response(id, Value::Array(vec![Value::Null; n]))
        }
        "workspace/workspaceFolders" => response(id, Value::Null),
        m if REFUSED_SERVER_REQUESTS.contains(&m) => error_response(id, REQUEST_FAILED, "refused by Paneldeck"),
        _ => error_response(id, METHOD_NOT_FOUND, "not supported by Paneldeck"),
    }
}

// ------------------------------------------------------------------------------------------------ Typen

/// Position (Zeile, Zeichen in der ausgehandelten Kodierung; UTF-16 als Standard).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: u32,
    pub character: u32,
}

impl Pos {
    pub fn json(self) -> Value {
        json!({"line": self.line, "character": self.character})
    }

    pub fn parse(v: &Value) -> Option<Pos> {
        Some(Pos { line: v.get("line")?.as_u64()? as u32, character: v.get("character")?.as_u64()? as u32 })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn parse(v: &Value) -> Option<Span> {
        Some(Span { start: Pos::parse(v.get("start")?)?, end: Pos::parse(v.get("end")?)? })
    }

}

/// Kodierung der Spalten (`positionEncoding`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf16,
    Utf8,
    Utf32,
}

impl Encoding {
    pub fn from_id(s: &str) -> Encoding {
        match s {
            "utf-8" => Encoding::Utf8,
            "utf-32" => Encoding::Utf32,
            _ => Encoding::Utf16,
        }
    }

    /// Spalte (in dieser Kodierung) einer Zeichenspalte `col` in `line`.
    pub fn column_of(self, line: &str, col: usize) -> u32 {
        let it = line.chars().take(col);
        match self {
            Encoding::Utf16 => it.map(char::len_utf16).sum::<usize>() as u32,
            Encoding::Utf8 => it.map(char::len_utf8).sum::<usize>() as u32,
            Encoding::Utf32 => it.count() as u32,
        }
    }

    /// Zeichenspalte zu einer Spalte dieser Kodierung (mitten in einem Zeichen = dieses Zeichen; hinter dem Ende = Länge).
    pub fn char_of(self, line: &str, units: u32) -> usize {
        let mut u = 0u32;
        for (i, c) in line.chars().enumerate() {
            let w = match self {
                Encoding::Utf16 => c.len_utf16() as u32,
                Encoding::Utf8 => c.len_utf8() as u32,
                Encoding::Utf32 => 1,
            };
            if u + w > units {
                return i;
            }
            u += w;
        }
        line.chars().count()
    }
}

/// Schwere einer Diagnose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl Severity {
    fn from_num(n: i64) -> Severity {
        match n {
            2 => Severity::Warning,
            3 => Severity::Information,
            4 => Severity::Hint,
            _ => Severity::Error,
        }
    }
}

/// Eine Diagnose (Fehler, Warnung …).
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub range: Span,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
    /// Tag 1: unnötiger Code (gedimmt).
    pub unnecessary: bool,
    /// Tag 2: veraltet (durchgestrichen).
    pub deprecated: bool,
}

/// Höchstlänge einer Diagnose-Meldung, die gespeichert wird.
const MAX_DIAG_MESSAGE: usize = 4000;

impl Diagnostic {
    pub fn parse(v: &Value) -> Option<Diagnostic> {
        let tags: Vec<i64> = v.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
        let mut message = v.get("message")?.as_str()?.to_string();
        if message.len() > MAX_DIAG_MESSAGE {
            let mut cut = MAX_DIAG_MESSAGE;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
            message.push('…');
        }
        Some(Diagnostic {
            range: Span::parse(v.get("range")?)?,
            severity: Severity::from_num(v.get("severity").and_then(Value::as_i64).unwrap_or(1)),
            message,
            source: v.get("source").and_then(Value::as_str).map(str::to_string),
            code: v.get("code").and_then(|c| c.as_str().map(str::to_string).or_else(|| c.as_i64().map(|n| n.to_string()))),
            unnecessary: tags.contains(&1),
            deprecated: tags.contains(&2),
        })
    }
}

/// `textDocument/publishDiagnostics`: Adresse, Version (falls genannt) und die ersten `max` Diagnosen (Fehler zuerst).
pub fn parse_publish(params: &Value, max: usize) -> Option<(String, Option<i64>, Vec<Diagnostic>, bool)> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let version = params.get("version").and_then(Value::as_i64);
    let mut all: Vec<Diagnostic> = params.get("diagnostics")?.as_array()?.iter().filter_map(Diagnostic::parse).collect();
    all.sort_by_key(|d| (d.severity, d.range.start));
    let cut = all.len() > max;
    all.truncate(max);
    Some((uri, version, all, cut))
}

/// Art eines Vorschlags (Symbol im Popup).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ItemKind {
    #[default]
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

impl ItemKind {
    fn from_num(n: i64) -> ItemKind {
        use ItemKind::*;
        const ALL: [ItemKind; 25] = [Text, Method, Function, Constructor, Field, Variable, Class, Interface, Module, Property, Unit, Value, Enum, Keyword, Snippet, Color, File, Reference, Folder, EnumMember, Constant, Struct, Event, Operator, TypeParameter];
        ALL.get((n - 1).max(0) as usize).copied().unwrap_or(Text)
    }

    /// Kurzes Zeichen für das Popup.
    pub fn glyph(self) -> &'static str {
        use ItemKind::*;
        match self {
            Method | Function | Constructor => "ƒ",
            Field | Property => "▪",
            Variable | Value => "𝑥",
            Class | Struct | Interface => "◆",
            Module | Folder | File => "▤",
            Enum | EnumMember => "≡",
            Keyword => "⌘",
            Snippet => "✂",
            Constant => "π",
            TypeParameter => "T",
            _ => "•",
        }
    }
}

/// Ein Vorschlag der Vervollständigung.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Completion {
    pub label: String,
    pub kind: ItemKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    /// Text zum Filtern/Sortieren.
    pub filter: String,
    pub sort: String,
    /// Einzufügender Text (ohne `text_edit`).
    pub insert: String,
    /// Ersetzung eines Bereichs (geht vor `insert`).
    pub text_edit: Option<(Span, String)>,
    /// Weitere Änderungen (z. B. `use`-Zeilen); werden mit eingefügt, sofern sie den Hauptbereich nicht berühren.
    pub additional: Vec<(Span, String)>,
    /// `insert`/`text_edit` ist ein Snippet (`$1`, `${2:x}`, `$0`).
    pub snippet: bool,
    pub preselect: bool,
}

fn doc_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn edit_of(v: &Value) -> Option<(Span, String)> {
    let text = v.get("newText")?.as_str()?.to_string();
    // InsertReplaceEdit: `insert`-Bereich nehmen (ersetzt nicht hinter dem Cursor).
    let range = v.get("range").or_else(|| v.get("insert"))?;
    Some((Span::parse(range)?, text))
}

impl Completion {
    pub fn parse(v: &Value) -> Option<Completion> {
        let label = v.get("label")?.as_str()?.to_string();
        let insert = v.get("insertText").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| label.clone());
        Some(Completion {
            kind: ItemKind::from_num(v.get("kind").and_then(Value::as_i64).unwrap_or(1)),
            detail: v.get("detail").and_then(Value::as_str).map(str::to_string),
            documentation: v.get("documentation").and_then(doc_text),
            filter: v.get("filterText").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| label.clone()),
            sort: v.get("sortText").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| label.clone()),
            insert,
            text_edit: v.get("textEdit").and_then(edit_of),
            additional: v.get("additionalTextEdits").and_then(Value::as_array).map(|a| a.iter().filter_map(edit_of).collect()).unwrap_or_default(),
            snippet: v.get("insertTextFormat").and_then(Value::as_i64) == Some(2),
            preselect: v.get("preselect").and_then(Value::as_bool).unwrap_or(false),
            label,
        })
    }
}

/// Antwort auf `textDocument/completion`: Liste oder `CompletionList`; höchstens `max` Einträge.
pub fn parse_completions(v: &Value, max: usize) -> (Vec<Completion>, bool) {
    let (items, incomplete) = match v {
        Value::Array(a) => (a.as_slice(), false),
        Value::Object(o) => (o.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]), o.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false)),
        _ => (&[][..], false),
    };
    (items.iter().take(max).filter_map(Completion::parse).collect(), incomplete)
}

/// Was der Server kann (aus der Antwort auf `initialize`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Caps {
    /// 1 = ganzer Text je Änderung, 2 = Teiländerungen.
    pub sync_kind: i64,
    pub completion: bool,
    pub triggers: Vec<String>,
    pub encoding: Encoding,
    /// Der Server will `didSave` mit Text.
    pub save_text: bool,
}

pub fn parse_caps(init_result: &Value) -> Caps {
    let c = init_result.get("capabilities").cloned().unwrap_or(Value::Null);
    let sync = c.get("textDocumentSync");
    let sync_kind = match sync {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(1),
        Some(o @ Value::Object(_)) => o.get("change").and_then(Value::as_i64).unwrap_or(1),
        _ => 1,
    };
    let save_text = sync.and_then(|s| s.get("save")).and_then(|s| s.get("includeText")).and_then(Value::as_bool).unwrap_or(false);
    let comp = c.get("completionProvider");
    Caps {
        sync_kind,
        completion: comp.is_some_and(|v| !v.is_null()),
        triggers: comp.and_then(|v| v.get("triggerCharacters")).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter(|s| s.chars().count() == 1).map(str::to_string).take(32).collect()).unwrap_or_default(),
        encoding: c.get("positionEncoding").and_then(Value::as_str).map(Encoding::from_id).unwrap_or_default(),
        save_text,
    }
}

/// Parameter für `initialize`: was Paneldeck kann (nur Vervollständigen, Diagnosen, Fortschritt; keine Bearbeitungen
/// durch den Server, keine Befehle).
pub fn initialize_params(pid: u32, root_uri: &str, root_name: &str) -> Value {
    json!({
        "processId": pid,
        "clientInfo": {"name": "Paneldeck", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri,
        "workspaceFolders": [{"uri": root_uri, "name": root_name}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "workspace": {"applyEdit": false, "workspaceFolders": false, "configuration": true, "didChangeConfiguration": {"dynamicRegistration": false}},
            "window": {"workDoneProgress": true, "showDocument": {"support": false}},
            "textDocument": {
                "synchronization": {"didSave": true, "willSave": false, "willSaveWaitUntil": false, "dynamicRegistration": false},
                "completion": {
                    "dynamicRegistration": false,
                    "contextSupport": true,
                    "completionItem": {"snippetSupport": true, "documentationFormat": ["plaintext", "markdown"], "deprecatedSupport": true, "preselectSupport": true, "insertReplaceSupport": true, "labelDetailsSupport": false},
                    "completionItemKind": {"valueSet": (1..=25).collect::<Vec<i32>>()}
                },
                "publishDiagnostics": {"relatedInformation": false, "versionSupport": true, "tagSupport": {"valueSet": [1, 2]}, "codeDescriptionSupport": false, "dataSupport": false}
            }
        },
        "trace": "off"
    })
}

// ------------------------------------------------------------------------------------------------ Adressen

/// `file:///C:/a%20b/x.rs` aus einem Windows- bzw. Unix-Pfad (Laufwerksbuchstabe klein wie VS Code, Rest kodiert).
pub fn path_to_uri(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = if p.starts_with("//") {
        // UNC (\\server\share): file://server/share
        p.trim_start_matches('/').to_string()
    } else if p.len() >= 2 && p.as_bytes()[1] == b':' {
        format!("/{}{}", p[..1].to_ascii_lowercase(), &p[1..])
    } else {
        p
    };
    let mut out = String::from("file://");
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            b':' if out.len() <= "file:///c".len() => out.push(':'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Pfad aus einer `file:`-Adresse (`None` für andere Schemata). Windows: `C:\…` bzw. `\\server\share\…`.
pub fn uri_to_path(uri: &str, windows: bool) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let (host, path) = match rest.find('/') {
        Some(0) => ("", rest),
        Some(i) => (&rest[..i], &rest[i..]),
        None => return None,
    };
    let mut bytes = Vec::with_capacity(path.len());
    let b = path.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|s| u8::from_str_radix(s, 16).ok())?;
            bytes.push(h);
            i += 3;
        } else {
            bytes.push(b[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    if !windows {
        return Some(if host.is_empty() { path } else { format!("//{host}{path}") });
    }
    if !host.is_empty() {
        return Some(format!("\\\\{host}{}", path.replace('/', "\\")));
    }
    let p = path.strip_prefix('/').unwrap_or(&path);
    let p = if p.len() >= 2 && p.as_bytes()[1] == b':' { format!("{}{}", p[..1].to_ascii_uppercase(), &p[1..]) } else { format!("/{p}") };
    Some(p.replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_limits_hold() {
        let mut buf = Vec::new();
        write_message(&mut buf, &request(1, "initialize", json!({"a": "ä"}))).unwrap();
        write_message(&mut buf, &notification("initialized", json!({}))).unwrap();
        let mut r = std::io::BufReader::new(&buf[..]);
        let a = read_message(&mut r, DEFAULT_MAX_MESSAGE).unwrap();
        assert_eq!(a["method"], "initialize");
        assert_eq!(a["params"]["a"], "ä");
        assert_eq!(classify(read_message(&mut r, DEFAULT_MAX_MESSAGE).unwrap()), Some(Incoming::Notification { method: "initialized".into(), params: json!({}) }));
        assert_eq!(read_message(&mut r, DEFAULT_MAX_MESSAGE), Err(ReadError::Eof));
        let big = b"Content-Length: 999999\r\n\r\n{}".to_vec();
        assert_eq!(read_message(&mut std::io::BufReader::new(&big[..]), 1000), Err(ReadError::TooLarge(999999)));
        let bad = b"Content-Length: x\r\n\r\n".to_vec();
        assert!(matches!(read_message(&mut std::io::BufReader::new(&bad[..]), 1000), Err(ReadError::Malformed(_))));
        // Weitere Kopfzeilen (Content-Type) werden überlesen.
        let ct = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: 2\r\n\r\n{}".to_vec();
        assert_eq!(read_message(&mut std::io::BufReader::new(&ct[..]), 1000), Ok(json!({})));
    }

    #[test]
    fn server_requests_that_change_things_are_refused() {
        for m in REFUSED_SERVER_REQUESTS {
            let a = answer_server_request(&json!(7), m, &json!({}));
            assert_eq!(a["error"]["code"], REQUEST_FAILED, "{m}");
        }
        assert_eq!(answer_server_request(&json!(1), "workspace/configuration", &json!({"items": [{}, {}]}))["result"], json!([null, null]));
        assert_eq!(answer_server_request(&json!(1), "window/workDoneProgress/create", &json!({}))["result"], Value::Null);
        assert_eq!(answer_server_request(&json!("x"), "custom/thing", &json!({}))["error"]["code"], METHOD_NOT_FOUND);
        assert!(matches!(classify(json!({"id": 3, "result": [1]})), Some(Incoming::Response { id: 3, result: Ok(_) })));
        assert!(matches!(classify(json!({"id": 3, "error": {"code": -1, "message": "m"}})), Some(Incoming::Response { result: Err((-1, _)), .. })));
        assert!(matches!(classify(json!({"id": "s1", "method": "workspace/applyEdit"})), Some(Incoming::Request { .. })));
    }

    #[test]
    fn utf16_columns_and_types() {
        let l = "a😀b";
        assert_eq!(Encoding::Utf16.column_of(l, 2), 3);
        assert_eq!(Encoding::Utf16.char_of(l, 3), 2);
        assert_eq!(Encoding::Utf16.char_of(l, 2), 1, "mitten im Ersatzpaar");
        assert_eq!(Encoding::Utf8.column_of(l, 2), 5);
        assert_eq!(Encoding::Utf32.char_of(l, 9), 3);
        let p = json!({"uri": "file:///c%3A/x.rs", "version": 4, "diagnostics": [
            {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}, "severity": 2, "message": "w"},
            {"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 1}}, "message": "e", "tags": [2], "code": 42},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "severity": 4, "message": "h", "tags": [1]}
        ]});
        let (uri, v, d, cut) = parse_publish(&p, 2).unwrap();
        assert_eq!((uri.as_str(), v, cut), ("file:///c%3A/x.rs", Some(4), true));
        assert_eq!(d[0].severity, Severity::Error, "Fehler zuerst");
        assert!(d[0].deprecated && d[0].code.as_deref() == Some("42"));
        assert_eq!(d.len(), 2);
        let (items, inc) = parse_completions(&json!({"isIncomplete": true, "items": [{"label": "len", "kind": 2, "detail": "fn()", "insertTextFormat": 2, "insertText": "len()$0"}, {"label": "x", "textEdit": {"insert": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 2}}, "replace": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 5}}, "newText": "xy"}}]}), 100);
        assert!(inc);
        assert_eq!((items[0].kind, items[0].snippet, items[0].insert.as_str()), (ItemKind::Method, true, "len()$0"));
        assert_eq!(items[1].text_edit.as_ref().map(|(s, t)| (s.end.character, t.as_str())), Some((2, "xy")), "Einfügen-Bereich");
        let caps = parse_caps(&json!({"capabilities": {"textDocumentSync": {"change": 2, "save": {"includeText": true}}, "completionProvider": {"triggerCharacters": [".", "::", ":"]}}}));
        assert_eq!((caps.sync_kind, caps.completion, caps.triggers.clone(), caps.save_text), (2, true, vec![".".to_string(), ":".to_string()], true));
        assert_eq!(parse_caps(&json!({"capabilities": {"textDocumentSync": 1}})).completion, false);
    }

    #[test]
    fn uris_round_trip_for_windows_unc_and_unix_paths() {
        let u = path_to_uri(r"C:\work\a b\x#.rs");
        assert_eq!(u, "file:///c:/work/a%20b/x%23.rs");
        assert_eq!(uri_to_path(&u, true).as_deref(), Some(r"C:\work\a b\x#.rs"));
        assert_eq!(uri_to_path("file:///c%3A/x/%C3%A4.rs", true).as_deref(), Some(r"C:\x\ä.rs"));
        let unc = path_to_uri(r"\\wsl.localhost\Ubuntu\home\u\a.py");
        assert_eq!(unc, "file://wsl.localhost/Ubuntu/home/u/a.py");
        assert_eq!(uri_to_path(&unc, true).as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\u\a.py"));
        assert_eq!(path_to_uri("/home/u/a.py"), "file:///home/u/a.py");
        assert_eq!(uri_to_path("file:///home/u/a%20b.py", false).as_deref(), Some("/home/u/a b.py"));
        assert_eq!(uri_to_path("https://x/y", true), None);
    }
}
