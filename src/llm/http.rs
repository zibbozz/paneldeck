//! HTTP für LLM-Server: GET und POST (JSON), `http`/`https`, beliebiger Port, Antwort als Strom.
//!
//! Getrennt vom Update-HTTP (`app::update::http`), damit dessen Sicherheitseigenschaften unverändert bleiben. Gleiche Muster:
//! eigene Handles, TLS 1.2/1.3, keine Weiterleitungen, keine Cookies, keine automatische Anmeldung. Anders: nur Hosts nach
//! `net::check` (vor JEDER Anfrage, Namen werden neu aufgelöst), kein Proxy (`WINHTTP_ACCESS_TYPE_NO_PROXY`), nach dem
//! Verbinden wird die tatsächliche Gegenstelle geprüft, langes Empfangs-Zeitlimit (erstes Token kann auf der CPU Minuten
//! dauern), Abbruch über ein `AtomicBool` (schließt das Anfrage-Handle, auch während ein Lesen blockiert). Zertifikate werden
//! immer geprüft; eine Sperrprüfung wird nicht erzwungen (interne Zertifizierungsstellen im LAN haben oft keine Sperrliste).

use super::error::LlmError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const SEND_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_RECEIVE_TIMEOUT: Duration = Duration::from_secs(300);
/// Höchstens so viel vom Fehlertext bei 4xx/5xx.
pub const MAX_ERROR_BODY: usize = 8 << 10;
/// Größte Zeile im Datenstrom (NDJSON/SSE).
pub const MAX_LINE: usize = 8 << 20;
/// Größte Antwort, die ganz gelesen wird (Modell-Listen).
pub const MAX_BODY: usize = 32 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// Eine Anfrage. `url` ist die volle Adresse; der Transport prüft sie selbst (`net::parse` + `net::check`).
#[derive(Clone, Debug)]
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
pub fn send_ok(t: &dyn Transport, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
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
/// Eine letzte Zeile ohne Zeilenende zählt mit.
pub fn read_lines(r: &mut dyn HttpResponse, cancel: &AtomicBool, on_line: &mut dyn FnMut(&str) -> bool) -> Result<(), LlmError> {
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
pub fn user_agent() -> String {
    format!("Paneldeck/{}", env!("CARGO_PKG_VERSION"))
}

/// Die Verbindung für den echten Betrieb.
pub fn system_transport() -> Arc<dyn Transport> {
    #[cfg(windows)]
    {
        Arc::new(winhttp::WinHttp)
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
pub mod winhttp {
    use super::super::error::{LlmError, classify};
    use super::super::net;
    use super::{HttpRequest, HttpResponse, Transport};
    use std::ffi::c_void;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::*;

    pub struct WinHttp;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn ms(d: Duration) -> i32 {
        d.as_millis().clamp(1, i32::MAX as u128) as i32
    }

    /// Meldet WinHTTP einen TLS-Fehler, landen die Gründe (Flags) im `AtomicU32` hinter dem Kontext.
    unsafe extern "system" fn on_status(_h: *mut c_void, ctx: usize, status: u32, info: *mut c_void, len: u32) {
        if status == WINHTTP_CALLBACK_STATUS_SECURE_FAILURE && ctx != 0 && !info.is_null() && len >= 4 {
            // SAFETY: bei SECURE_FAILURE zeigt `info` auf ein DWORD; `ctx` ist die Adresse von `Shared::tls`, das (im Arc)
            // länger lebt als das Anfrage-Handle.
            let (flags, cell) = unsafe { (*(info as *const u32), &*(ctx as *const AtomicU32)) };
            cell.fetch_or(flags, Ordering::Relaxed);
        }
    }

    /// Besitzt ein Session- oder Verbindungs-Handle.
    struct H(*mut c_void);
    impl Drop for H {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: das Handle stammt von WinHTTP und wird genau einmal geschlossen.
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    // SAFETY: WinHTTP-Handles sind nicht an einen Faden gebunden.
    unsafe impl Send for H {}

    /// Anfrage-Handle, das auch der Abbruch-Wächter schließen kann (wer es per `swap` holt, schließt es genau einmal).
    struct Shared {
        req: AtomicPtr<c_void>,
        tls: AtomicU32,
        done: AtomicBool,
    }

    impl Shared {
        fn req(&self) -> Result<*mut c_void, LlmError> {
            let h = self.req.load(Ordering::Acquire);
            if h.is_null() { Err(LlmError::Cancelled) } else { Ok(h) }
        }
        fn close(&self) {
            let h = self.req.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !h.is_null() {
                // SAFETY: nur der Aufrufer, der das Handle per `swap` bekommen hat, schließt es. Ein blockierter
                // WinHttpReadData/ReceiveResponse in einem anderen Faden kehrt dann mit einem Fehler zurück.
                unsafe { WinHttpCloseHandle(h) };
            }
        }
    }

    /// Felder in Schließ-Reihenfolge: Anfrage (in `Drop`), Verbindung, Session.
    struct Resp {
        shared: Arc<Shared>,
        _conn: H,
        _session: H,
        cancel: Arc<AtomicBool>,
    }

    impl Drop for Resp {
        fn drop(&mut self) {
            self.shared.done.store(true, Ordering::Release);
            self.shared.close();
        }
    }

    impl Resp {
        fn err(&self, call: &'static str) -> LlmError {
            // SAFETY: liest nur den Fehlerwert des aktuellen Threads.
            let code = unsafe { GetLastError() };
            if self.cancel.load(Ordering::Relaxed) {
                return LlmError::Cancelled;
            }
            classify(code, self.shared.tls.load(Ordering::Relaxed), call)
        }
    }

    impl HttpResponse for Resp {
        fn status(&self) -> u16 {
            let Ok(h) = self.shared.req() else { return 0 };
            let mut v: u32 = 0;
            let mut len: u32 = 4;
            // SAFETY: Puffer `v` mit Länge 4; WINHTTP_QUERY_FLAG_NUMBER liefert eine Zahl.
            let ok = unsafe { WinHttpQueryHeaders(h, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut v as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) };
            if ok != 0 { v as u16 } else { 0 }
        }

        fn read(&mut self, buf: &mut [u8]) -> Result<usize, LlmError> {
            let h = self.shared.req()?;
            let mut n: u32 = 0;
            let want = buf.len().min(u32::MAX as usize) as u32;
            // SAFETY: `buf` hat mindestens `want` Bytes.
            if unsafe { WinHttpReadData(h, buf.as_mut_ptr() as *mut c_void, want, &mut n) } == 0 {
                return Err(self.err("WinHttpReadData"));
            }
            Ok(n as usize)
        }
    }

    /// `WINHTTP_CONNECTION_INFO` ohne das WinSock-Feature von windows-sys: `cbSize` + 2× `SOCKADDR_STORAGE` (128 Bytes, 8er-Ausrichtung).
    #[repr(C)]
    struct ConnInfo {
        cb: u32,
        local: [u64; 16],
        remote: [u64; 16],
    }

    /// Gegenstelle aus einem `SOCKADDR_STORAGE` (AF_INET = 2, AF_INET6 = 23).
    fn sockaddr_ip(s: &[u64; 16]) -> Option<IpAddr> {
        // SAFETY: 128 Bytes lesen, die zu `s` gehören.
        let b: &[u8; 128] = unsafe { &*(s as *const [u64; 16] as *const [u8; 128]) };
        match u16::from_le_bytes([b[0], b[1]]) {
            2 => Some(IpAddr::V4(Ipv4Addr::new(b[4], b[5], b[6], b[7]))),
            23 => {
                let mut a = [0u8; 16];
                a.copy_from_slice(&b[8..24]);
                Some(IpAddr::V6(Ipv6Addr::from(a)))
            }
            _ => None,
        }
    }

    fn remote_ip(h: *mut c_void) -> Option<IpAddr> {
        let mut ci = ConnInfo { cb: std::mem::size_of::<ConnInfo>() as u32, local: [0; 16], remote: [0; 16] };
        let mut len = std::mem::size_of::<ConnInfo>() as u32;
        // SAFETY: Puffer der angegebenen Größe; WinHTTP füllt ihn nach dem Verbindungsaufbau.
        let ok = unsafe { WinHttpQueryOption(h, WINHTTP_OPTION_CONNECTION_INFO, &mut ci as *mut ConnInfo as *mut c_void, &mut len) };
        if ok == 0 { None } else { sockaddr_ip(&ci.remote) }
    }

    fn set_u32(h: *mut c_void, opt: u32, v: u32) -> bool {
        // SAFETY: `v` lebt bis zum Ende des Aufrufs; WinHTTP kopiert den Wert.
        unsafe { WinHttpSetOption(h, opt, &v as *const u32 as *const c_void, 4) != 0 }
    }

    fn last(call: &'static str) -> LlmError {
        // SAFETY: liest nur den Fehlerwert des aktuellen Threads.
        classify(unsafe { GetLastError() }, 0, call)
    }

    impl Transport for WinHttp {
        fn send(&self, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
            // Host-Regel vor jeder Anfrage (Namen werden neu aufgelöst).
            let url = net::parse(req.url)?;
            net::check(&url)?;
            if req.bearer.is_some_and(|k| !super::bearer_ok(k)) {
                return Err(LlmError::BadKey);
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(LlmError::Cancelled);
            }
            let ua = wide(&super::user_agent());
            // SAFETY: nullterminierter UTF-16-Puffer; ausdrücklich ohne Proxy (Server liegen auf diesem Rechner bzw. im LAN).
            let session = H(unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), 0) });
            if session.0.is_null() {
                return Err(last("WinHttpOpen"));
            }
            let s = session.0;
            if !set_u32(s, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3)
                && !set_u32(s, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2)
            {
                return Err(last("WinHttpSetOption(SECURE_PROTOCOLS)"));
            }
            if !set_u32(s, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER) {
                return Err(last("WinHttpSetOption(REDIRECT_POLICY)"));
            }
            let recv = ms(req.receive_timeout);
            // SAFETY: gültiges Session-Handle. Namensauflösung, Verbindung, Senden, Empfangen (je Block).
            unsafe { WinHttpSetTimeouts(s, ms(super::CONNECT_TIMEOUT), ms(super::CONNECT_TIMEOUT), ms(super::SEND_TIMEOUT), recv) };
            // Ollama schickt die Kopfzeilen erst mit dem ersten Token: auch darauf lange warten.
            set_u32(s, WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, recv as u32);

            // IPv6 in Klammern (ohne: ERROR_WINHTTP_INVALID_URL).
            let whost = wide(&url.host_display());
            // SAFETY: gültiges Session-Handle und nullterminierter Hostname.
            let conn = H(unsafe { WinHttpConnect(s, whost.as_ptr(), url.port, 0) });
            if conn.0.is_null() {
                return Err(last("WinHttpConnect"));
            }
            let path = match url.path.as_str() {
                "" => "/".to_string(),
                p => p.to_string(),
            };
            let verb = wide(req.method.as_str());
            let wpath = wide(&path);
            let secure = if url.scheme == net::Scheme::Https { WINHTTP_FLAG_SECURE } else { 0 };
            // SAFETY: gültiges Verbindungs-Handle, nullterminierte Puffer. Der Pfad ist geprüft (nur ASCII ohne Sonderzeichen).
            let h = unsafe { WinHttpOpenRequest(conn.0, verb.as_ptr(), wpath.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), secure | WINHTTP_FLAG_ESCAPE_DISABLE | WINHTTP_FLAG_REFRESH) };
            if h.is_null() {
                return Err(last("WinHttpOpenRequest"));
            }
            let shared = Arc::new(Shared { req: AtomicPtr::new(h), tls: AtomicU32::new(0), done: AtomicBool::new(false) });
            let resp = Resp { shared: shared.clone(), _conn: conn, _session: session, cancel: cancel.clone() };
            // Pflicht: keine Cookies, keine Weiterleitungen, keine automatische Anmeldung.
            for (opt, v, call) in [
                (WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION, "WinHttpSetOption(DISABLE_FEATURE)"),
                (WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH, "WinHttpSetOption(AUTOLOGON_POLICY)"),
            ] {
                if !set_u32(h, opt, v) {
                    return Err(last(call));
                }
            }
            // SAFETY: gültiges Anfrage-Handle; der Rückruf liest nur `info` und den Kontext (siehe `on_status`).
            unsafe { WinHttpSetStatusCallback(h, Some(on_status), WINHTTP_CALLBACK_FLAG_SECURE_FAILURE, 0) };
            let ctx = &shared.tls as *const AtomicU32 as usize;

            // Abbruch-Wächter: schließt das Anfrage-Handle, sobald `cancel` gesetzt wird; endet mit der Antwort.
            {
                let (sh, c) = (shared.clone(), cancel.clone());
                std::thread::spawn(move || {
                    while !sh.done.load(Ordering::Acquire) {
                        if c.load(Ordering::Relaxed) {
                            sh.close();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                });
            }

            let mut headers = format!("Accept: {}\r\n", req.accept);
            if req.body.is_some() {
                headers.push_str("Content-Type: application/json\r\n");
            }
            if let Some(k) = req.bearer {
                headers.push_str(&format!("Authorization: Bearer {k}\r\n"));
            }
            let wh = wide(&headers);
            let body = req.body.unwrap_or(&[]);
            let blen = u32::try_from(body.len()).map_err(|_| LlmError::TooLarge)?;
            let h = shared.req()?;
            // SAFETY: Kopfzeilen nullterminiert (Länge u32::MAX = bis zur Null); Rumpf lebt bis zum Ende des Aufrufs (synchron).
            if unsafe { WinHttpSendRequest(h, wh.as_ptr(), u32::MAX, if body.is_empty() { std::ptr::null() } else { body.as_ptr() as *const c_void }, blen, blen, ctx) } == 0 {
                return Err(resp.err("WinHttpSendRequest"));
            }
            // Die tatsächliche Gegenstelle muss erlaubt sein (Schutz gegen DNS-Rebinding zwischen Prüfung und Verbindung).
            let h = shared.req()?;
            match remote_ip(h) {
                Some(ip) if net::allowed_ip(ip) => {}
                other => return Err(LlmError::Blocked { host: url.host_display(), addr: other }),
            }
            // SAFETY: gültiges Anfrage-Handle.
            if unsafe { WinHttpReceiveResponse(shared.req()?, std::ptr::null_mut()) } == 0 {
                return Err(resp.err("WinHttpReceiveResponse"));
            }
            Ok(Box::new(resp))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn conn_info_layout_matches_winhttp() {
            // x64: cbSize (4) + 4 Füllbytes + 2 × 128.
            assert_eq!(std::mem::size_of::<ConnInfo>(), 264);
            let mut s = [0u64; 16];
            let b: &mut [u8; 128] = unsafe { &mut *(&mut s as *mut [u64; 16] as *mut [u8; 128]) };
            b[0] = 2;
            b[4..8].copy_from_slice(&[192, 168, 1, 9]);
            assert_eq!(sockaddr_ip(&s), Some("192.168.1.9".parse().unwrap()));
            let b: &mut [u8; 128] = unsafe { &mut *(&mut s as *mut [u64; 16] as *mut [u8; 128]) };
            b[0] = 23;
            b[8..24].copy_from_slice(&Ipv6Addr::LOCALHOST.octets());
            assert_eq!(sockaddr_ip(&s), Some("::1".parse().unwrap()));
        }
    }
}

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
        pub method: Method,
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
                // Wie WinHTTP: der Wächter schließt das Handle, das blockierte Lesen endet mit einem Fehler.
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
    fn read_all_limit() {
        let t = Fake::default().with(Method::Get, "http://localhost/x", Canned::ok(&"x".repeat(100)));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut r = send_ok(&t, &req("http://localhost/x"), &cancel).unwrap();
        assert_eq!(read_all(r.as_mut(), 99, &cancel), Err(LlmError::TooLarge));
    }
}
