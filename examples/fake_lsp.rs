//! Ersatz-Language-Server für die Tests des Smart-Modus (#367): spricht LSP über stdin/stdout, meldet eine Diagnose je
//! geöffnetem bzw. geändertem Dokument (an Zeile 1), vervollständigt mit zwei festen Vorschlägen und versucht einmal,
//! eine Datei zu ändern (`workspace/applyEdit` – Paneldeck muss das ablehnen). Kein Teil der veröffentlichten Programme,
//! keine Kommandozeilen-Option von Paneldeck; `cargo test` baut ihn als Beispiel.

use std::io::{BufRead, BufReader, Write};

fn read(r: &mut impl BufRead) -> Option<String> {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let l = line.trim();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            len = v.trim().parse().ok()?;
        }
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

fn write(w: &mut impl Write, body: &str) {
    let _ = write!(w, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = w.flush();
}

/// Wert eines einfachen Feldes aus dem JSON-Text (genügt für die festen Nachrichten der Tests).
fn field<'a>(msg: &'a str, key: &str) -> Option<&'a str> {
    let i = msg.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = msg[i..].trim_start();
    if let Some(r) = rest.strip_prefix('"') {
        r.split('"').next()
    } else {
        rest.split([',', '}']).next().map(str::trim)
    }
}

fn main() {
    let stdin = std::io::stdin();
    let mut r = BufReader::new(stdin.lock());
    let mut out = std::io::stdout();
    while let Some(msg) = read(&mut r) {
        let method = field(&msg, "method").unwrap_or("");
        let id = field(&msg, "id").unwrap_or("0").to_string();
        match method {
            "initialize" => {
                write(&mut out, &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"capabilities":{{"textDocumentSync":1,"completionProvider":{{"triggerCharacters":["."]}}}}}}}}"#));
                write(&mut out, r#"{"jsonrpc":"2.0","id":"x1","method":"workspace/applyEdit","params":{"edit":{}}}"#);
            }
            "textDocument/didOpen" | "textDocument/didChange" => {
                let uri = field(&msg, "uri").unwrap_or("");
                let version = field(&msg, "version").unwrap_or("1");
                write(&mut out, &format!(r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{uri}","version":{version},"diagnostics":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":3}}}},"severity":1,"message":"fake error","source":"fake"}}]}}}}"#));
            }
            "textDocument/completion" => {
                write(&mut out, &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":[{{"label":"println","kind":3,"insertTextFormat":2,"insertText":"println!(\"$1\")$0"}},{{"label":"print","kind":3}}]}}"#));
            }
            "shutdown" => write(&mut out, &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#)),
            "exit" => return,
            _ => {}
        }
    }
}
