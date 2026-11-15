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

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
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
pub mod winhttp {
    //! WinHTTP im asynchronen Modus (`WINHTTP_FLAG_ASYNC`): Jeder Aufruf kehrt sofort zurück, das Ergebnis kommt über den
    //! Rückruf in eine Warteschlange, auf die nur der Besitzer-Faden wartet. Dadurch schließt ausschließlich der Besitzer das
    //! Anfrage-Handle – auch beim Abbruch (er wartet mit kurzem Takt und prüft dabei `cancel`). Kein anderer Faden reicht je
    //! ein Handle an WinHTTP weiter, ein schon geschlossenes Handle wird nie benutzt.
    //!
    //! Puffer, die WinHTTP asynchron liest oder beschreibt (Rumpf, Kopfzeilen, Lesepuffer), gehören dem Kontext `Ctx` (im
    //! `Arc`). WinHTTP hält eine eigene Referenz darauf, die erst der Rückruf `HANDLE_CLOSING` freigibt – nach ihm kommen
    //! keine Rückrufe mehr. Der Besitzer wartet NICHT auf `HANDLE_CLOSING`: Er übergibt asynchronen Aufrufen nie eigenen
    //! (Stack-)Speicher, nur Speicher des Kontexts, und der lebt bis zum letzten Rückruf. Nach dem Schließen kann der Besitzer
    //! also sofort weitermachen.
    //!
    //! DNS-Namen: Die geprüfte IP wird mit `WINHTTP_OPTION_RESOLUTION_HOSTNAME` an die Anfrage gebunden – WinHTTP verbindet
    //! sich mit genau dieser IP, SNI, Zertifikatsprüfung und `Host:` bleiben beim Namen (http und https). Kennt Windows die
    //! Option nicht, verbindet http mit der IP und setzt `Host:` selbst; https verbindet dann über den Namen (Restrisiko siehe
    //! `WinHttp::send`).
    #[cfg(not(target_pointer_width = "64"))]
    compile_error!("llm::http::winhttp: the ConnInfo layout is only defined for 64-bit Windows");

    use super::super::error::{LlmError, classify};
    use super::super::net;
    use super::{HttpRequest, HttpResponse, Transport};
    use std::cell::UnsafeCell;
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::*;

    /// Transport über WinHTTP. `resolve` ist austauschbar (Tests); Standard = Systemauflösung.
    pub struct WinHttp {
        pub resolve: fn(&str, u16) -> std::io::Result<Vec<IpAddr>>,
        /// `false` = so tun, als kenne Windows `RESOLUTION_HOSTNAME` nicht (nur für Tests des Rückfalls).
        pub use_pin: bool,
    }

    impl Default for WinHttp {
        fn default() -> Self {
            Self { resolve: net::system_resolve, use_pin: true }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn ms(d: Duration) -> i32 {
        d.as_millis().clamp(1, i32::MAX as u128) as i32
    }

    fn last_error() -> u32 {
        // SAFETY: liest nur den Fehlerwert des aktuellen Threads.
        unsafe { GetLastError() }
    }

    /// Besitzt ein Session- oder Verbindungs-Handle (an diesen ist kein Rückruf registriert).
    struct H(*mut c_void);
    impl Drop for H {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: das Handle stammt von WinHTTP und wird genau einmal geschlossen.
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    // SAFETY: WinHTTP-Handles sind nicht an einen Faden gebunden; benutzt wird es nur vom Besitzer.
    unsafe impl Send for H {}
    unsafe impl Sync for H {}

    enum Ev {
        SendDone,
        Headers,
        Avail(usize),
        Read(usize),
        Error(u32),
        Closing,
    }

    #[derive(Default)]
    struct State {
        events: VecDeque<Ev>,
        tls: u32,
    }

    const BUF: usize = 64 << 10;

    struct Ctx {
        st: Mutex<State>,
        cv: Condvar,
        /// Lesepuffer für `WinHttpReadData` (WinHTTP schreibt asynchron hinein).
        buf: Box<[UnsafeCell<u8>]>,
        body: Vec<u8>,
        headers: Vec<u16>,
    }
    // SAFETY: `buf` beschreibt nur WinHTTP während eines laufenden Lesens; der Besitzer liest ihn erst nach `READ_COMPLETE`,
    // das über den Mutex übergeben wird (happens-before). Alles andere ist unveränderlich oder hinter dem Mutex.
    unsafe impl Sync for Ctx {}
    unsafe impl Send for Ctx {}

    impl Ctx {
        fn push(&self, ev: Option<Ev>, tls: u32) {
            let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
            g.tls |= tls;
            if let Some(ev) = ev {
                g.events.push_back(ev);
            }
            self.cv.notify_all();
        }
    }

    /// Rückruf von WinHTTP (beliebiger Faden). `ctx` ist der Zeiger aus `Arc::into_raw`, den WinHTTP hält.
    unsafe extern "system" fn on_status(_h: *mut c_void, ctx: usize, status: u32, info: *mut c_void, len: u32) {
        if ctx == 0 {
            return;
        }
        // SAFETY: Die Referenz von WinHTTP lebt bis zum Ende von HANDLE_CLOSING (unten freigegeben).
        let c = unsafe { &*(ctx as *const Ctx) };
        match status {
            WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => c.push(Some(Ev::SendDone), 0),
            WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => c.push(Some(Ev::Headers), 0),
            WINHTTP_CALLBACK_STATUS_READ_COMPLETE => c.push(Some(Ev::Read(len as usize)), 0),
            WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE => {
                // SAFETY: bei DATA_AVAILABLE zeigt `info` auf ein DWORD mit der Anzahl verfügbarer Bytes.
                let n = if info.is_null() || len < 4 { 0 } else { unsafe { *(info as *const u32) } };
                c.push(Some(Ev::Avail(n as usize)), 0)
            }
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
                // SAFETY: bei REQUEST_ERROR zeigt `info` auf ein WINHTTP_ASYNC_RESULT.
                let code = if info.is_null() { 0 } else { unsafe { (*(info as *const WINHTTP_ASYNC_RESULT)).dwError } };
                c.push(Some(Ev::Error(code)), 0)
            }
            WINHTTP_CALLBACK_STATUS_SECURE_FAILURE if !info.is_null() && len >= 4 => {
                // SAFETY: bei SECURE_FAILURE zeigt `info` auf ein DWORD mit Flags.
                c.push(None, unsafe { *(info as *const u32) })
            }
            WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => {
                c.push(Some(Ev::Closing), 0);
                // SAFETY: letzter Rückruf für dieses Handle; gibt die Referenz von WinHTTP frei (genau einmal).
                drop(unsafe { Arc::from_raw(ctx as *const Ctx) });
            }
            _ => {}
        }
    }

    /// Das Anfrage-Handle; nur der Besitzer benutzt und schließt es.
    struct Req {
        h: *mut c_void,
        ctx: Arc<Ctx>,
        cancel: Arc<AtomicBool>,
    }

    impl Req {
        fn close(&mut self) {
            if !self.h.is_null() {
                // SAFETY: eigenes Handle, genau einmal geschlossen; laufende asynchrone Vorgänge bricht WinHTTP ab.
                unsafe { WinHttpCloseHandle(self.h) };
                self.h = std::ptr::null_mut();
            }
        }

        fn handle(&mut self) -> Result<*mut c_void, LlmError> {
            if self.cancel.load(Ordering::Relaxed) {
                self.close();
            }
            if self.h.is_null() { Err(LlmError::Cancelled) } else { Ok(self.h) }
        }

        fn fail(&self, code: u32, call: &'static str) -> LlmError {
            if self.cancel.load(Ordering::Relaxed) {
                return LlmError::Cancelled;
            }
            let tls = self.ctx.st.lock().unwrap_or_else(|e| e.into_inner()).tls;
            classify(code, tls, call)
        }

        /// Wartet auf das Ergebnis des laufenden Vorgangs; prüft dabei alle 50 ms `cancel` (dann wird geschlossen).
        fn wait(&mut self, want: fn(&Ev) -> bool, call: &'static str) -> Result<Ev, LlmError> {
            let ctx = self.ctx.clone();
            let mut g = ctx.st.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                while let Some(ev) = g.events.pop_front() {
                    match ev {
                        Ev::Error(code) => {
                            drop(g);
                            return Err(self.fail(code, call));
                        }
                        Ev::Closing => return Err(LlmError::Cancelled),
                        e if want(&e) => return Ok(e),
                        _ => {}
                    }
                }
                if self.cancel.load(Ordering::Relaxed) {
                    drop(g);
                    self.close();
                    return Err(LlmError::Cancelled);
                }
                g = ctx.cv.wait_timeout(g, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
    }

    impl Drop for Req {
        fn drop(&mut self) {
            self.close();
        }
    }

    /// Felder in Schließ-Reihenfolge: Anfrage, Verbindung, Session.
    struct Resp {
        req: Req,
        _conn: H,
        _session: Arc<H>,
    }

    impl HttpResponse for Resp {
        fn status(&self) -> u16 {
            if self.req.h.is_null() {
                return 0;
            }
            let mut v: u32 = 0;
            let mut len: u32 = 4;
            // SAFETY: Puffer `v` mit Länge 4; Kopfzeilen liegen vor (HEADERS_AVAILABLE), die Abfrage ist synchron.
            let ok = unsafe { WinHttpQueryHeaders(self.req.h, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut v as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) };
            if ok != 0 { v as u16 } else { 0 }
        }

        fn read(&mut self, buf: &mut [u8]) -> Result<usize, LlmError> {
            let h = self.req.handle()?;
            // Erst fragen, wie viel schon da ist: WinHttpReadData wartet sonst, bis der ganze Puffer voll ist – beim
            // Datenstrom kämen Tokens dann nur in großen Blöcken an.
            // SAFETY: gültiges Handle; asynchron → Ergebnis über DATA_AVAILABLE, kein Zähler.
            if unsafe { WinHttpQueryDataAvailable(h, std::ptr::null_mut()) } == 0 {
                return Err(self.req.fail(last_error(), "WinHttpQueryDataAvailable"));
            }
            let avail = match self.req.wait(|e| matches!(e, Ev::Avail(_)), "WinHttpQueryDataAvailable")? {
                Ev::Avail(n) => n,
                _ => unreachable!(),
            };
            if avail == 0 {
                return Ok(0);
            }
            let h = self.req.handle()?;
            let want = buf.len().min(BUF).min(avail) as u32;
            let p = self.req.ctx.buf.as_ptr() as *mut u8;
            // SAFETY: Lesepuffer des Kontexts (BUF Bytes) lebt, bis WinHTTP seine Referenz freigibt; asynchron → kein Zähler.
            if unsafe { WinHttpReadData(h, p as *mut c_void, want, std::ptr::null_mut()) } == 0 {
                return Err(self.req.fail(last_error(), "WinHttpReadData"));
            }
            match self.req.wait(|e| matches!(e, Ev::Read(_)), "WinHttpReadData")? {
                Ev::Read(n) => {
                    let n = n.min(want as usize);
                    // SAFETY: WinHTTP hat `n` Bytes geschrieben und ist fertig (READ_COMPLETE über den Mutex empfangen).
                    buf[..n].copy_from_slice(unsafe { std::slice::from_raw_parts(p, n) });
                    Ok(n)
                }
                _ => unreachable!(),
            }
        }
    }

    /// `WINHTTP_CONNECTION_INFO` ohne das WinSock-Feature von windows-sys: `cbSize` + 2× `SOCKADDR_STORAGE` (128 Bytes,
    /// 8er-Ausrichtung) – so nur unter 64 Bit (siehe `compile_error!` oben).
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
        // SAFETY: Puffer der angegebenen Größe; synchrone Abfrage nach dem Verbindungsaufbau.
        let ok = unsafe { WinHttpQueryOption(h, WINHTTP_OPTION_CONNECTION_INFO, &mut ci as *mut ConnInfo as *mut c_void, &mut len) };
        if ok == 0 { None } else { sockaddr_ip(&ci.remote) }
    }

    fn set_u32(h: *mut c_void, opt: u32, v: u32) -> bool {
        // SAFETY: `v` lebt bis zum Ende des Aufrufs; WinHTTP kopiert den Wert.
        unsafe { WinHttpSetOption(h, opt, &v as *const u32 as *const c_void, 4) != 0 }
    }

    fn last(call: &'static str) -> LlmError {
        classify(last_error(), 0, call)
    }

    /// Name des Aufrufs, wenn WinHTTP `RESOLUTION_HOSTNAME` nicht kennt (löst den Rückfall aus).
    const PIN_CALL: &str = "WinHttpSetOption(RESOLUTION_HOSTNAME)";

    /// Ein Verbindungsziel.
    struct Target {
        connect: String,
        host_header: Option<String>,
        pin: Option<IpAddr>,
    }

    const CALLBACKS: u32 = WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE
        | WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE
        | WINHTTP_CALLBACK_FLAG_READ_COMPLETE
        | WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE
        | WINHTTP_CALLBACK_FLAG_REQUEST_ERROR
        | WINHTTP_CALLBACK_FLAG_SECURE_FAILURE
        | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;

    impl WinHttp {
        fn session(req: &HttpRequest<'_>) -> Result<Arc<H>, LlmError> {
            let ua = wide(&super::user_agent());
            // SAFETY: nullterminierter UTF-16-Puffer; ohne Proxy (Server liegen auf diesem Rechner bzw. im LAN), asynchron.
            let session = H(unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), WINHTTP_FLAG_ASYNC) });
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
            if unsafe { WinHttpSetTimeouts(s, ms(super::CONNECT_TIMEOUT), ms(super::CONNECT_TIMEOUT), ms(super::SEND_TIMEOUT), recv) } == 0 {
                return Err(last("WinHttpSetTimeouts"));
            }
            // Ollama schickt die Kopfzeilen erst mit dem ersten Token: auch darauf lange warten.
            if !set_u32(s, WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, recv as u32) {
                return Err(last("WinHttpSetOption(RECEIVE_RESPONSE_TIMEOUT)"));
            }
            Ok(Arc::new(session))
        }

        /// Ein Versuch gegen `t.connect`. `t.host_header` = eigener `Host:`-Kopf, `t.pin` = IP für die Namensauflösung.
        fn attempt(session: &Arc<H>, url: &net::Url, t: &Target, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Resp, LlmError> {
            let (connect_host, host_header) = (t.connect.as_str(), t.host_header.as_deref());
            let whost = wide(connect_host);
            // SAFETY: gültiges Session-Handle und nullterminierter Hostname.
            let conn = H(unsafe { WinHttpConnect(session.0, whost.as_ptr(), url.port, 0) });
            if conn.0.is_null() {
                return Err(last("WinHttpConnect"));
            }
            let path = if url.path.is_empty() { "/".to_string() } else { url.path.clone() };
            let verb = wide(req.method.as_str());
            let wpath = wide(&path);
            let secure = if url.scheme == net::Scheme::Https { WINHTTP_FLAG_SECURE } else { 0 };
            // SAFETY: gültiges Verbindungs-Handle, nullterminierte Puffer. Der Pfad ist geprüft (nur ASCII ohne Sonderzeichen).
            let h = unsafe { WinHttpOpenRequest(conn.0, verb.as_ptr(), wpath.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), secure | WINHTTP_FLAG_ESCAPE_DISABLE | WINHTTP_FLAG_REFRESH) };
            if h.is_null() {
                return Err(last("WinHttpOpenRequest"));
            }
            let mut headers = format!("Accept: {}\r\n", req.accept);
            if req.body.is_some() {
                headers.push_str("Content-Type: application/json\r\n");
            }
            if let Some(k) = req.bearer {
                headers.push_str(&format!("Authorization: Bearer {k}\r\n"));
            }
            let ctx = Arc::new(Ctx {
                st: Mutex::new(State::default()),
                cv: Condvar::new(),
                buf: (0..BUF).map(|_| UnsafeCell::new(0u8)).collect(),
                body: req.body.unwrap_or(&[]).to_vec(),
                headers: wide(&headers),
            });
            let raw = Arc::into_raw(ctx.clone()) as usize;
            let raw_ok = {
                let v = raw;
                // SAFETY: pointergroßer Wert, WinHTTP kopiert ihn.
                unsafe { WinHttpSetOption(h, WINHTTP_OPTION_CONTEXT_VALUE, &v as *const usize as *const c_void, std::mem::size_of::<usize>() as u32) != 0 }
            };
            // SAFETY: gültiges Anfrage-Handle; Rückruf siehe `on_status`.
            let prev = if raw_ok { unsafe { WinHttpSetStatusCallback(h, Some(on_status), CALLBACKS, 0) } } else { None };
            if !raw_ok || prev.map(|f| f as usize) == Some(usize::MAX) {
                let e = last("WinHttpSetStatusCallback");
                // Ohne Rückruf kommt kein HANDLE_CLOSING: Referenz selbst zurücknehmen.
                // SAFETY: Handle noch nicht weitergegeben; `raw` stammt von `Arc::into_raw` oben und wurde nie freigegeben.
                unsafe {
                    WinHttpSetStatusCallback(h, None, 0, 0);
                    WinHttpCloseHandle(h);
                    drop(Arc::from_raw(raw as *const Ctx));
                }
                return Err(e);
            }
            let mut r = Req { h, ctx, cancel: cancel.clone() };
            // Pflicht: keine Cookies, keine Weiterleitungen, keine automatische Anmeldung.
            for (opt, v, call) in [
                (WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION, "WinHttpSetOption(DISABLE_FEATURE)"),
                (WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH, "WinHttpSetOption(AUTOLOGON_POLICY)"),
            ] {
                if !set_u32(h, opt, v) {
                    return Err(last(call));
                }
            }
            if let Some(ip) = t.pin {
                let w = wide(&ip.to_string());
                // SAFETY: Puffer mit Länge in Zeichen (ohne Null); WinHTTP kopiert ihn.
                if unsafe { WinHttpSetOption(h, WINHTTP_OPTION_RESOLUTION_HOSTNAME, w.as_ptr() as *const c_void, (w.len() - 1) as u32) } == 0 {
                    return Err(last(PIN_CALL));
                }
            }
            if let Some(hh) = host_header {
                let w = wide(&format!("Host: {hh}\r\n"));
                // SAFETY: nullterminierter Puffer (Länge u32::MAX = bis zur Null), synchron.
                if unsafe { WinHttpAddRequestHeaders(h, w.as_ptr(), u32::MAX, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE) } == 0 {
                    return Err(last("WinHttpAddRequestHeaders(Host)"));
                }
            }
            let c = r.ctx.clone();
            let blen = u32::try_from(c.body.len()).map_err(|_| LlmError::TooLarge)?;
            let bptr = if c.body.is_empty() { std::ptr::null() } else { c.body.as_ptr() as *const c_void };
            let h = r.handle()?;
            // SAFETY: Kopfzeilen und Rumpf gehören dem Kontext und leben, bis WinHTTP seine Referenz freigibt.
            if unsafe { WinHttpSendRequest(h, c.headers.as_ptr(), u32::MAX, bptr, blen, blen, raw) } == 0 {
                return Err(r.fail(last_error(), "WinHttpSendRequest"));
            }
            r.wait(|e| matches!(e, Ev::SendDone), "WinHttpSendRequest")?;
            // Zusätzliche Prüfung der tatsächlichen Gegenstelle (die Anfrage ist zu diesem Zeitpunkt schon gesendet). Bei
            // gebundener IP ist das nur eine zweite Absicherung; im Rückfall „https + Name ohne Bindung“ entdeckt sie eine
            // Umleitung erst nachträglich – die Antwort wird dann nicht gelesen.
            let h = r.handle()?;
            match remote_ip(h) {
                Some(ip) if net::allowed_ip(ip) => {}
                other => return Err(LlmError::Blocked { host: url.host_display(), addr: other }),
            }
            // SAFETY: gültiges Anfrage-Handle; Ergebnis kommt über den Rückruf.
            if unsafe { WinHttpReceiveResponse(h, std::ptr::null_mut()) } == 0 {
                return Err(r.fail(last_error(), "WinHttpReceiveResponse"));
            }
            r.wait(|e| matches!(e, Ev::Headers), "WinHttpReceiveResponse")?;
            Ok(Resp { req: r, _conn: conn, _session: session.clone() })
        }
    }

    impl Transport for WinHttp {
        fn send(&self, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
            // Host-Regel vor jeder Anfrage (Namen werden neu aufgelöst).
            let url = net::parse(req.url)?;
            let addrs = net::check_with(&url, &self.resolve)?;
            if req.bearer.is_some_and(|k| !super::bearer_ok(k)) {
                return Err(LlmError::BadKey);
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(LlmError::Cancelled);
            }
            // Ziele (IPv6 in Klammern, ohne: ERROR_WINHTTP_INVALID_URL):
            // - DNS-Name: je geprüfte Adresse ein Versuch mit dem Namen, die IP per RESOLUTION_HOSTNAME gebunden.
            // - IP-Literal oder `localhost` (löst Windows selbst auf, nie über DNS): direkt.
            let targets: Vec<Target> = match &url.host {
                net::Host::Name(_) => addrs.iter().map(|ip| Target { connect: url.host_display(), host_header: None, pin: Some(*ip) }).collect(),
                _ => vec![Target { connect: url.host_display(), host_header: None, pin: None }],
            };
            let session = Self::session(req)?;
            let first = if self.use_pin { Self::try_all(&session, &url, &targets, req, cancel) } else { Err(LlmError::Sys { code: 0, call: PIN_CALL }) };
            match first {
                // Windows kennt RESOLUTION_HOSTNAME nicht: Rückfall.
                Err(LlmError::Sys { call: PIN_CALL, .. }) => {
                    let host_header = if url.port == url.default_port() { url.host_display() } else { format!("{}:{}", url.host_display(), url.port) };
                    let fallback: Vec<Target> = match url.scheme {
                        // http: mit der geprüften IP verbinden, `Host:` selbst setzen – es geht nichts an eine ungeprüfte Adresse.
                        net::Scheme::Http => addrs
                            .iter()
                            .map(|ip| Target { connect: if ip.is_ipv6() { format!("[{ip}]") } else { ip.to_string() }, host_header: Some(host_header.clone()), pin: None })
                            .collect(),
                        // https: über den Namen (SNI/Zertifikat). Restrisiko: WinHTTP löst selbst neu auf; liefert DNS jetzt eine
                        // andere Adresse, geht die Anfrage (per TLS, nur bei gültigem Zertifikat für den Namen) dorthin, bevor die
                        // Prüfung der Gegenstelle anschlägt.
                        net::Scheme::Https => vec![Target { connect: url.host_display(), host_header: None, pin: None }],
                    };
                    Self::try_all(&session, &url, &fallback, req, cancel).map(|r| Box::new(r) as Box<dyn HttpResponse>)
                }
                r => r.map(|r| Box::new(r) as Box<dyn HttpResponse>),
            }
        }
    }

    impl WinHttp {
        /// Versuche der Reihe nach; bei „keine Verbindung“ (12029) oder Zeitüberschreitung beim Senden (12002) die nächste Adresse.
        fn try_all(session: &Arc<H>, url: &net::Url, targets: &[Target], req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Resp, LlmError> {
            let mut last_err = LlmError::Resolve { host: url.host_display() };
            for (i, t) in targets.iter().enumerate() {
                match Self::attempt(session, url, t, req, cancel) {
                    Ok(r) => return Ok(r),
                    Err(LlmError::Sys { code, call }) if i + 1 < targets.len() && (code == 12029 || (code == 12002 && call == "WinHttpSendRequest")) => {
                        last_err = LlmError::Sys { code, call }
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(last_err)
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
