//! HTTP für LLM-Server: GET und POST (JSON), `http`/`https`, beliebiger Port, Antwort als Strom.
//!
//! Getrennt vom Update-HTTP (`app::update::http`), damit dessen Sicherheitseigenschaften unverändert bleiben. Gleiche Muster:
//! eigene Handles, TLS 1.2/1.3, keine Weiterleitungen, keine Cookies, keine automatische Anmeldung. Anders: nur Hosts nach
//! `net::check` (vor JEDER Anfrage, Namen werden neu aufgelöst), kein Proxy (`WINHTTP_ACCESS_TYPE_NO_PROXY`), nach dem
//! Verbinden wird die tatsächliche Gegenstelle geprüft, langes Empfangs-Zeitlimit (erstes Token kann auf der CPU Minuten
//! dauern), Abbruch über ein `AtomicBool` (auch während auf das erste Token gewartet wird; WinHTTP läuft asynchron,
//! nur der Besitzer schließt das Handle). Bei http + DNS-Name wird direkt mit der geprüften IP verbunden (`Host:`-Kopf). Zertifikate werden
//! immer geprüft; eine Sperrprüfung wird nicht erzwungen (interne Zertifizierungsstellen im LAN haben oft keine Sperrliste).

use super::error::LlmError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg_attr(not(windows), allow(dead_code))]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg_attr(not(windows), allow(dead_code))]
pub const SEND_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_RECEIVE_TIMEOUT: Duration = Duration::from_secs(300);
/// Höchstens so viel vom Fehlertext bei 4xx/5xx.
pub const MAX_ERROR_BODY: usize = 8 << 10;
/// Größte Zeile im Datenstrom (NDJSON/SSE).
pub const MAX_LINE: usize = 8 << 20;
/// Größter Datenstrom insgesamt.
pub const MAX_STREAM: usize = 128 << 20;
/// Größte Antwort, die ganz gelesen wird (Modell-Listen).
pub const MAX_BODY: usize = 32 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// Eine Anfrage. `url` ist die volle Adresse; der Transport prüft sie selbst (`net::parse` + `net::check`).
#[derive(Clone)]
pub struct HttpRequest<'a> {
    pub method: Method,
    pub url: &'a str,
    /// JSON-Rumpf (nur bei POST).
    pub body: Option<&'a [u8]>,
    /// Optionaler API-Schlüssel → `Authorization: Bearer …`.
    pub bearer: Option<&'a str>,
    pub accept: &'a str,
    pub receive_timeout: Duration,
}

/// Ohne den Schlüssel (nur `***`), damit er nie in einer Debug-Ausgabe oder einem Protokoll landet.
impl std::fmt::Debug for HttpRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("body_len", &self.body.map(<[u8]>::len))
            .field("bearer", &self.bearer.map(|_| "***"))
            .field("accept", &self.accept)
            .field("receive_timeout", &self.receive_timeout)
            .finish()
    }
}

pub trait HttpResponse {
    fn status(&self) -> u16;
    /// Nächster Block; `Ok(0)` = Ende.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LlmError>;
}

pub trait Transport: Send + Sync {
    /// Eine Anfrage ohne Weiterleitungen. Wird `cancel` gesetzt, endet sie (auch ein blockiertes Lesen) mit `Cancelled`.
    fn send(&self, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError>;
}

/// API-Schlüssel ohne Steuerzeichen (sonst ließen sich Kopfzeilen einschleusen).
pub fn bearer_ok(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| !c.is_control() && c != '\u{7f}') && k.is_ascii()
}

/// Sendet und prüft den Status: 2xx → Antwort; sonst `Status` mit höchstens `MAX_ERROR_BODY` Bytes des Fehlertexts.
/// Jeder Fehler nach gesetztem `cancel` gilt als `Cancelled` (z. B. Status 0 eines gerade geschlossenen Handles).
pub fn send_ok(t: &dyn Transport, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
    let r = send_ok_inner(t, req, cancel);
    if r.is_err() && cancel.load(Ordering::Relaxed) {
        return Err(LlmError::Cancelled);
    }
    r
}

fn send_ok_inner(t: &dyn Transport, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(LlmError::Cancelled);
    }
    if let Some(k) = req.bearer
        && !bearer_ok(k)
    {
        return Err(LlmError::BadKey);
    }
    let mut r = t.send(req, cancel)?;
    let code = r.status();
    if (200..300).contains(&code) {
        return Ok(r);
    }
    let mut body = Vec::new();
    let mut buf = [0u8; 2048];
    while body.len() < MAX_ERROR_BODY {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&buf[..n]),
        }
    }
    body.truncate(MAX_ERROR_BODY);
    Err(LlmError::Status { code, body: String::from_utf8_lossy(&body).into_owned() })
}

/// Liest die ganze Antwort (höchstens `max` Bytes).
pub fn read_all(r: &mut dyn HttpResponse, max: usize, cancel: &AtomicBool) -> Result<Vec<u8>, LlmError> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 16 << 10];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(out);
        }
        if out.len() + n > max {
            return Err(LlmError::TooLarge);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// Zerlegt den Strom in Zeilen (`\n`, ein `\r` davor wird entfernt) und gibt jede an `on_line`; `false` beendet das Lesen.
/// Eine letzte Zeile ohne Zeilenende zählt mit. Mehr als `MAX_LINE` je Zeile oder `MAX_STREAM` insgesamt → `TooLarge`.
pub fn read_lines(r: &mut dyn HttpResponse, cancel: &AtomicBool, on_line: &mut dyn FnMut(&str) -> bool) -> Result<(), LlmError> {
    read_lines_max(r, cancel, MAX_STREAM, on_line)
}

pub fn read_lines_max(r: &mut dyn HttpResponse, cancel: &AtomicBool, max_total: usize, on_line: &mut dyn FnMut(&str) -> bool) -> Result<(), LlmError> {
    let mut total = 0usize;
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 16 << 10];
    let emit = |bytes: &[u8], on_line: &mut dyn FnMut(&str) -> bool| {
        let b = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        on_line(&String::from_utf8_lossy(b))
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            if !pending.is_empty() {
                emit(&pending, on_line);
            }
            return Ok(());
        }
        total += n;
        if total > max_total {
            return Err(LlmError::TooLarge);
        }
        let mut chunk = &buf[..n];
        while let Some(i) = chunk.iter().position(|&b| b == b'\n') {
            pending.extend_from_slice(&chunk[..i]);
            chunk = &chunk[i + 1..];
            let line = std::mem::take(&mut pending);
            if !emit(&line, on_line) {
                return Ok(());
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(LlmError::Cancelled);
            }
        }
        pending.extend_from_slice(chunk);
        if pending.len() > MAX_LINE {
            return Err(LlmError::TooLarge);
        }
    }
}

/// User-Agent ohne persönliche Daten.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn user_agent() -> String {
    format!("Paneldeck/{}", env!("CARGO_PKG_VERSION"))
}

/// Die Verbindung für den echten Betrieb.
pub fn system_transport() -> Arc<dyn Transport> {
    #[cfg(windows)]
    {
        Arc::new(winhttp::WinHttp::default())
    }
    #[cfg(not(windows))]
    {
        Arc::new(Unsupported)
    }
}

#[cfg(not(windows))]
struct Unsupported;

#[cfg(not(windows))]
impl Transport for Unsupported {
    fn send(&self, _: &HttpRequest<'_>, _: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
        Err(LlmError::Unsupported)
    }
}

#[cfg(windows)]
pub mod winhttp;

#[cfg(test)]
pub mod fake {
    //! Attrappe: feste Antworten je (Methode, Adresse), in kleinen Stücken geliefert; protokolliert Anfragen samt Rumpf.
    use super::{HttpRequest, HttpResponse, Method, Transport};
    use crate::llm::error::LlmError;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub struct Canned {
        pub status: u16,
        pub chunks: Vec<Vec<u8>>,
        /// Nach so vielen Stücken wird `cancel` gesetzt (simuliert „Stopp“ während des Streams).
        pub cancel_after: Option<usize>,
        /// Fehler nach allen Stücken (z. B. abgerissene Verbindung).
        pub fail_at_end: Option<LlmError>,
    }

    impl Canned {
        pub fn ok(body: &str) -> Self {
            Self::chunks(200, body.as_bytes().chunks(5).map(|c| c.to_vec()).collect())
        }
        pub fn chunks(status: u16, chunks: Vec<Vec<u8>>) -> Self {
            Self { status, chunks, cancel_after: None, fail_at_end: None }
        }
        pub fn status(status: u16, body: &str) -> Self {
            Self::chunks(status, vec![body.as_bytes().to_vec()])
        }
    }

    #[derive(Clone, Debug)]
    pub struct Logged {
        // Nur in Debug-Ausgaben fehlgeschlagener Tests gelesen.
        #[allow(dead_code)]
        pub method: Method,
        #[allow(dead_code)]
        pub url: String,
        pub body: Option<serde_json::Value>,
        pub bearer: Option<String>,
    }

    #[derive(Default)]
    pub struct Fake {
        pub map: HashMap<(Method, String), Canned>,
        pub log: Mutex<Vec<Logged>>,
    }

    impl Fake {
        pub fn with(mut self, m: Method, url: &str, c: Canned) -> Self {
            self.map.insert((m, url.into()), c);
            self
        }
        pub fn log(&self) -> Vec<Logged> {
            self.log.lock().unwrap().clone()
        }
    }

    struct R {
        c: Canned,
        i: usize,
        cancel: Arc<AtomicBool>,
    }

    impl HttpResponse for R {
        fn status(&self) -> u16 {
            self.c.status
        }
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, LlmError> {
            if self.c.cancel_after == Some(self.i) {
                // Wie WinHTTP beim Abbruch: das laufende Lesen endet mit einem Fehler.
                self.cancel.store(true, Ordering::Relaxed);
                return Err(LlmError::Sys { code: 12017, call: "WinHttpReadData" });
            }
            let Some(ch) = self.c.chunks.get(self.i) else {
                return match self.c.fail_at_end.clone() {
                    Some(e) => Err(e),
                    None => Ok(0),
                };
            };
            assert!(ch.len() <= buf.len());
            buf[..ch.len()].copy_from_slice(ch);
            self.i += 1;
            Ok(ch.len())
        }
    }

    impl Transport for Fake {
        fn send(&self, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
            self.log.lock().unwrap().push(Logged {
                method: req.method,
                url: req.url.into(),
                body: req.body.map(|b| serde_json::from_slice(b).expect("request body is JSON")),
                bearer: req.bearer.map(str::to_string),
            });
            let c = self.map.get(&(req.method, req.url.to_string())).cloned().ok_or_else(|| LlmError::BadResponse(format!("unexpected request {}", req.url)))?;
            Ok(Box::new(R { c, i: 0, cancel: cancel.clone() }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Canned, Fake};
    use super::*;

    fn req(url: &str) -> HttpRequest<'_> {
        HttpRequest { method: Method::Get, url, body: None, bearer: None, accept: "application/json", receive_timeout: DEFAULT_RECEIVE_TIMEOUT }
    }

    #[test]
    fn error_body_is_limited() {
        let big = "e".repeat(20_000);
        let t = Fake::default().with(Method::Get, "http://localhost/x", Canned::chunks(500, big.as_bytes().chunks(1000).map(|c| c.to_vec()).collect()));
        match send_ok(&t, &req("http://localhost/x"), &Arc::new(AtomicBool::new(false))) {
            Err(LlmError::Status { code: 500, body }) => assert_eq!(body.len(), MAX_ERROR_BODY),
            _ => panic!(),
        }
    }

    #[test]
    fn bearer_must_be_clean() {
        let t = Fake::default();
        let mut r = req("http://localhost/x");
        r.bearer = Some("abc\r\nX-Evil: 1");
        assert!(matches!(send_ok(&t, &r, &Arc::new(AtomicBool::new(false))), Err(LlmError::BadKey)));
        assert!(t.log().is_empty());
        assert!(bearer_ok("sk-abc_123.XYZ"));
        assert!(!bearer_ok(""));
    }

    #[test]
    fn lines_are_split_across_chunks() {
        let body = "a\r\nbb\n\nccc";
        let t = Fake::default().with(Method::Get, "http://localhost/x", Canned::chunks(200, body.as_bytes().chunks(1).map(|c| c.to_vec()).collect()));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut r = send_ok(&t, &req("http://localhost/x"), &cancel).unwrap();
        let mut lines = Vec::new();
        read_lines(r.as_mut(), &cancel, &mut |l| {
            lines.push(l.to_string());
            true
        })
        .unwrap();
        assert_eq!(lines, ["a", "bb", "", "ccc"]);
        // UTF-8 über Stückgrenzen hinweg
        let t = Fake::default().with(Method::Get, "http://localhost/u", Canned::chunks(200, "äö€\n".as_bytes().chunks(1).map(|c| c.to_vec()).collect()));
        let mut r = send_ok(&t, &req("http://localhost/u"), &cancel).unwrap();
        let mut got = String::new();
        read_lines(r.as_mut(), &cancel, &mut |l| {
            got.push_str(l);
            true
        })
        .unwrap();
        assert_eq!(got, "äö€");
    }

    #[test]
    fn line_and_stream_limits() {
        let cancel = Arc::new(AtomicBool::new(false));
        let chunk = vec![b'x'; 16 << 10];
        let n = MAX_LINE / chunk.len() + 2;
        let t = Fake::default().with(Method::Get, "http://localhost/x", Canned::chunks(200, vec![chunk.clone(); n]));
        let mut r = send_ok(&t, &req("http://localhost/x"), &cancel).unwrap();
        let mut lines = 0;
        assert_eq!(read_lines(r.as_mut(), &cancel, &mut |_| { lines += 1; true }), Err(LlmError::TooLarge));
        assert_eq!(lines, 0);
        let mut line = b"y".repeat(99);
        line.push(b'\n');
        let t = Fake::default().with(Method::Get, "http://localhost/y", Canned::chunks(200, vec![line; 20]));
        let mut r = send_ok(&t, &req("http://localhost/y"), &cancel).unwrap();
        let mut lines = 0;
        assert_eq!(read_lines_max(r.as_mut(), &cancel, 1000, &mut |_| { lines += 1; true }), Err(LlmError::TooLarge));
        assert_eq!(lines, 10);
    }

    #[test]
    fn errors_after_cancel_are_cancelled() {
        let cancel = Arc::new(AtomicBool::new(false));
        struct T;
        impl Transport for T {
            fn send(&self, _: &HttpRequest<'_>, c: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
                c.store(true, Ordering::Relaxed);
                Err(LlmError::Status { code: 0, body: String::new() })
            }
        }
        assert_eq!(send_ok(&T, &req("http://localhost/x"), &cancel).err(), Some(LlmError::Cancelled));
    }

    #[test]
    fn read_all_limit() {
        let t = Fake::default().with(Method::Get, "http://localhost/x", Canned::ok(&"x".repeat(100)));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut r = send_ok(&t, &req("http://localhost/x"), &cancel).unwrap();
        assert_eq!(read_all(r.as_mut(), 99, &cancel), Err(LlmError::TooLarge));
    }
}
