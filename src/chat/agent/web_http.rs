//! Transport der Web-Werkzeuge (#338): eine Anfrage ohne Weiterleitungen, Cookies und automatische Anmeldung, ohne
//! Proxy, verbunden nur mit vorher geprüften Adressen (`web::checked_addrs`); die tatsächliche Gegenstelle wird nach
//! dem Verbinden noch einmal geprüft. Windows: WinHTTP synchron (IP über `RESOLUTION_HOSTNAME` gebunden, Name bleibt
//! für `Host:`, SNI und Zertifikat); Linux/macOS: ureq + rustls mit eigenem Resolver.
//!
//! Bewusst getrennt vom LLM-Transport (`llm::http`), dessen Regel genau umgekehrt ist (nur lokale Server).

use super::web::{checked_addrs, public_ip, system_resolve, HostRule, WebReq, WebResp, WebTransport};
use std::net::IpAddr;
use std::time::Duration;

const CONNECT: Duration = Duration::from_secs(10);
const SEND: Duration = Duration::from_secs(15);
const RECEIVE: Duration = Duration::from_secs(20);

/// Der echte Transport.
pub struct System;

/// Kopfzeilen ohne Steuerzeichen (sonst ließen sich weitere einschleusen).
fn headers_ok(req: &WebReq<'_>) -> bool {
    req.headers.iter().all(|(k, v)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_graphic() && b != b':') && v.chars().all(|c| !c.is_control()))
}

impl WebTransport for System {
    fn send(&self, req: &WebReq<'_>) -> Result<WebResp, String> {
        if !headers_ok(req) {
            return Err("invalid header".into());
        }
        let addrs = checked_addrs(req.url, req.rule, &system_resolve)?;
        let mut last = String::from("no address");
        for ip in addrs.iter().take(4) {
            match imp::attempt(req, *ip) {
                Ok(r) => return Ok(r),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

fn user_agent() -> String {
    crate::llm::http::user_agent()
}

/// Darf die tatsächliche Gegenstelle bedient werden?
fn peer_ok(rule: HostRule, peer: Option<IpAddr>) -> Result<(), String> {
    match (rule, peer) {
        (HostRule::Any, _) => Ok(()),
        (HostRule::Public, Some(ip)) if public_ip(ip) => Ok(()),
        (HostRule::Public, other) => Err(format!("the connection went to a non-public address ({})", other.map_or("unknown".to_string(), |i| i.to_string()))),
    }
}

#[cfg(windows)]
mod imp {
    use super::super::web::{WebReq, WebResp};
    use super::{peer_ok, user_agent, CONNECT, RECEIVE, SEND};
    use std::ffi::c_void;
    use std::net::IpAddr;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::*;

    /// gzip | deflate (`WINHTTP_DECOMPRESSION_FLAG_ALL`).
    const DECOMPRESSION_ALL: u32 = 3;

    struct H(*mut c_void);
    impl Drop for H {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: eigenes, gültiges Handle; synchroner Modus, kein Rückruf.
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn fail(call: &str) -> String {
        // SAFETY: liest nur den Fehlerwert des Threads.
        let code = unsafe { GetLastError() };
        let what = match code {
            12002 => "timeout",
            12007 => "name not resolved",
            12029 => "cannot connect",
            12030 | 12031 => "connection closed",
            12175 | 12045 | 12037 | 12038 | 12044 => "certificate or TLS error",
            _ => "network error",
        };
        format!("{what} ({call}, {code})")
    }

    fn set_u32(h: *mut c_void, opt: u32, v: u32) -> bool {
        // SAFETY: `v` lebt bis zum Ende des Aufrufs; WinHTTP kopiert den Wert.
        unsafe { WinHttpSetOption(h, opt, &v as *const u32 as *const c_void, 4) != 0 }
    }

    fn query_string(h: *mut c_void, level: u32) -> Option<String> {
        let mut len: u32 = 0;
        // SAFETY: erste Abfrage nur für die Länge (Puffer null).
        unsafe { WinHttpQueryHeaders(h, level, std::ptr::null(), std::ptr::null_mut(), &mut len, std::ptr::null_mut()) };
        if len == 0 || len > 64 * 1024 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
        let mut blen = (buf.len() * 2) as u32;
        // SAFETY: Puffer mit der angegebenen Länge in Bytes.
        let ok = unsafe { WinHttpQueryHeaders(h, level, std::ptr::null(), buf.as_mut_ptr() as *mut c_void, &mut blen, std::ptr::null_mut()) };
        if ok == 0 {
            return None;
        }
        let n = (blen as usize / 2).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string())
    }

    pub fn attempt(req: &WebReq<'_>, ip: IpAddr) -> Result<WebResp, String> {
        let ms = |d: std::time::Duration| d.as_millis() as i32;
        let ua = wide(&user_agent());
        // SAFETY: nullterminierte Puffer; ohne Proxy (sonst ginge die Prüfung der Gegenstelle ins Leere), synchron.
        let s = H(unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), 0) });
        if s.0.is_null() {
            return Err(fail("WinHttpOpen"));
        }
        if !set_u32(s.0, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3) && !set_u32(s.0, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2) {
            return Err(fail("SECURE_PROTOCOLS"));
        }
        if !set_u32(s.0, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER) {
            return Err(fail("REDIRECT_POLICY"));
        }
        // SAFETY: gültiges Session-Handle.
        if unsafe { WinHttpSetTimeouts(s.0, ms(CONNECT), ms(CONNECT), ms(SEND), ms(RECEIVE)) } == 0 {
            return Err(fail("WinHttpSetTimeouts"));
        }
        let host = wide(&req.url.host);
        // SAFETY: gültiges Session-Handle, nullterminierter Name.
        let c = H(unsafe { WinHttpConnect(s.0, host.as_ptr(), req.url.port, 0) });
        if c.0.is_null() {
            return Err(fail("WinHttpConnect"));
        }
        let verb = wide(req.method);
        let target = wide(&req.url.target);
        let secure = if req.url.https { WINHTTP_FLAG_SECURE } else { 0 };
        // SAFETY: gültiges Verbindungs-Handle, nullterminierte Puffer (Ziel ist geprüftes, kodiertes ASCII).
        let h = H(unsafe { WinHttpOpenRequest(c.0, verb.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), secure | WINHTTP_FLAG_ESCAPE_DISABLE | WINHTTP_FLAG_REFRESH) });
        if h.0.is_null() {
            return Err(fail("WinHttpOpenRequest"));
        }
        if !set_u32(h.0, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION) || !set_u32(h.0, WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH) {
            return Err(fail("DISABLE_FEATURE"));
        }
        let _ = set_u32(h.0, WINHTTP_OPTION_DECOMPRESSION, DECOMPRESSION_ALL);
        // Mit genau dieser (geprüften) Adresse verbinden; ohne die Option (ältere Windows-Versionen) bleibt die Prüfung
        // der Gegenstelle nach dem Senden – und bei https das Zertifikat des Namens.
        if req.url.host.parse::<IpAddr>().is_err() {
            let w = wide(&ip.to_string());
            // SAFETY: Puffer mit Länge in Zeichen (ohne Null); WinHTTP kopiert ihn.
            unsafe { WinHttpSetOption(h.0, WINHTTP_OPTION_RESOLUTION_HOSTNAME, w.as_ptr() as *const c_void, (w.len() - 1) as u32) };
        }
        let mut headers = String::new();
        for (k, v) in &req.headers {
            headers.push_str(&format!("{k}: {v}\r\n"));
        }
        let wh = wide(&headers);
        let body = req.body.as_deref().unwrap_or(&[]);
        let blen = body.len() as u32;
        let bptr = if body.is_empty() { std::ptr::null() } else { body.as_ptr() as *const c_void };
        // SAFETY: synchron; Kopfzeilen und Rumpf leben bis zum Ende des Aufrufs.
        if unsafe { WinHttpSendRequest(h.0, if headers.is_empty() { std::ptr::null() } else { wh.as_ptr() }, if headers.is_empty() { 0 } else { u32::MAX }, bptr, blen, blen, 0) } == 0 {
            return Err(fail("WinHttpSendRequest"));
        }
        // SAFETY: gültiges Anfrage-Handle.
        if unsafe { WinHttpReceiveResponse(h.0, std::ptr::null_mut()) } == 0 {
            return Err(fail("WinHttpReceiveResponse"));
        }
        peer_ok(req.rule, crate::llm::http::winhttp::remote_ip(h.0))?;
        let mut status: u32 = 0;
        let mut len: u32 = 4;
        // SAFETY: u32-Puffer mit Länge 4.
        if unsafe { WinHttpQueryHeaders(h.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut status as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) } == 0 {
            return Err(fail("WinHttpQueryHeaders"));
        }
        let location = query_string(h.0, WINHTTP_QUERY_LOCATION);
        let content_type = query_string(h.0, WINHTTP_QUERY_CONTENT_TYPE).unwrap_or_default();
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut truncated = false;
        loop {
            let mut n: u32 = 0;
            // SAFETY: Puffer mit der angegebenen Länge; synchron.
            if unsafe { WinHttpReadData(h.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut n) } == 0 {
                return Err(fail("WinHttpReadData"));
            }
            if n == 0 {
                break;
            }
            let room = req.max - out.len();
            out.extend_from_slice(&buf[..(n as usize).min(room)]);
            if out.len() >= req.max {
                truncated = true;
                break;
            }
        }
        Ok(WebResp { status: status as u16, location, content_type, body: out, truncated })
    }
}

#[cfg(unix)]
mod imp {
    use super::super::web::{WebReq, WebResp};
    use super::{peer_ok, user_agent, CONNECT, RECEIVE, SEND};
    use std::io::Read;
    use std::net::{IpAddr, SocketAddr};

    pub fn attempt(req: &WebReq<'_>, ip: IpAddr) -> Result<WebResp, String> {
        // Nur mit der geprüften Adresse verbinden: eigener Resolver (eine andere Adresse kann ureq gar nicht erreichen).
        let pinned = vec![SocketAddr::new(ip, req.url.port)];
        let mut b = ureq::AgentBuilder::new()
            .resolver(move |_: &str| Ok(pinned.clone()))
            .redirects(0)
            .try_proxy_from_env(false)
            .timeout_connect(CONNECT)
            .timeout_write(SEND)
            .timeout_read(RECEIVE)
            .user_agent(&user_agent());
        if req.url.https {
            b = b.tls_config(crate::platform::tls::client_config().map_err(|_| "TLS is not available".to_string())?);
        }
        let agent = b.build();
        // Die Adresse ist geprüft und verbunden wird mit `ip`; die Gegenstelle ist damit die geprüfte.
        peer_ok(req.rule, Some(ip))?;
        let mut r = agent.request(req.method, &req.url.to_url());
        for (k, v) in &req.headers {
            r = r.set(k, v);
        }
        let res = match &req.body {
            Some(b) => r.send_bytes(b),
            None => r.call(),
        };
        let resp = match res {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => return Err(if crate::platform::tls::is_cert_error(&t) { "certificate error".into() } else { format!("network error ({})", t.kind()) }),
        };
        let status = resp.status();
        let location = resp.header("location").map(str::to_string);
        let content_type = resp.header("content-type").unwrap_or("").to_string();
        let mut out = Vec::new();
        resp.into_reader().take(req.max as u64 + 1).read_to_end(&mut out).map_err(|e| format!("reading failed ({e})"))?;
        let truncated = out.len() > req.max;
        out.truncate(req.max);
        Ok(WebResp { status, location, content_type, body: out, truncated })
    }
}

#[cfg(test)]
mod tests {
    use super::super::web::parse_url;
    use super::*;
    use std::io::{Read, Write};

    /// Kleiner HTTP-Server auf 127.0.0.1, beantwortet eine Anfrage mit `reply` und gibt die Anfrage zurück.
    fn serve(reply: &'static [u8]) -> (u16, std::sync::mpsc::Receiver<String>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                let _ = s.write_all(reply);
            }
        });
        (port, rx)
    }

    #[test]
    fn local_server_only_with_rule_any_and_no_redirects_followed() {
        let (port, rx) = serve(b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Type: text/html\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        let url = parse_url(&format!("http://127.0.0.1:{port}/search?q=a%20b"), false).unwrap();
        let req = WebReq { method: "GET", url: &url, headers: vec![("Accept".into(), "application/json".into())], body: None, rule: HostRule::Any, max: 3 };
        let r = System.send(&req).unwrap();
        assert_eq!((r.status, r.location.as_deref(), r.content_type.as_str()), (302, Some("/next"), "text/html"));
        assert_eq!((r.body.as_slice(), r.truncated), (&b"hel"[..], true));
        let got = rx.recv().unwrap();
        assert!(got.starts_with("GET /search?q=a%20b HTTP/1.1") && got.to_ascii_lowercase().contains("accept: application/json"), "{got}");
        assert!(!got.to_ascii_lowercase().contains("cookie"));
        // Öffentliche Regel: lokale Adresse wird vor jeder Verbindung abgelehnt
        let pubreq = WebReq { rule: HostRule::Public, ..req };
        assert!(System.send(&pubreq).unwrap_err().contains("non-public"));
        // Kopfzeilen mit Zeilenumbruch: abgelehnt
        let bad = WebReq { method: "GET", url: &url, headers: vec![("X".into(), "a\r\nInjected: 1".into())], body: None, rule: HostRule::Any, max: 10 };
        assert_eq!(System.send(&bad).unwrap_err(), "invalid header");
        assert!(peer_ok(HostRule::Public, None).is_err() && peer_ok(HostRule::Public, Some("8.8.8.8".parse().unwrap())).is_ok());
    }
}
