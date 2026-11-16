//! `Transport` über WinHTTP (Windows): System-TLS, Proxy-Einstellungen des Systems, keine Cookies, keine automatische
//! Anmeldung, keine automatischen Weiterleitungen (die prüft `fetch`).

use super::{FetchError, Response, Transport};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER};
use windows_sys::Win32::Networking::WinHttp::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn sys(call: &'static str, tls: u32) -> FetchError {
    // SAFETY: GetLastError liest nur den Fehlerwert des aktuellen Threads.
    FetchError::Sys { code: unsafe { GetLastError() }, tls, call }
}

/// Meldet WinHTTP einen TLS-Fehler, sammelt dieser Rückruf die Gründe (z. B. „Sperrprüfung nicht möglich“) in dem
/// `AtomicU32`, dessen Adresse als Kontext an `WinHttpSendRequest` ging.
unsafe extern "system" fn on_status(_h: *mut c_void, ctx: usize, status: u32, info: *mut c_void, len: u32) {
    if status == WINHTTP_CALLBACK_STATUS_SECURE_FAILURE && ctx != 0 && !info.is_null() && len >= 4 {
        // SAFETY: bei SECURE_FAILURE zeigt `info` auf ein DWORD mit Flags; `ctx` ist die Adresse des `AtomicU32` in `Resp`
        // bzw. in `get`, der länger lebt als das Anfrage-Handle.
        let (flags, cell) = unsafe { (*(info as *const u32), &*(ctx as *const AtomicU32)) };
        cell.fetch_or(flags, Ordering::Relaxed);
    }
}

/// Besitzt ein WinHTTP-Handle und schließt es beim Verlassen.
struct H(*mut c_void);
impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: das Handle stammt von WinHTTP und wird genau einmal geschlossen.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn set_u32(h: *mut c_void, opt: u32, v: u32) -> bool {
    // SAFETY: `v` lebt bis zum Ende des Aufrufs; WinHTTP kopiert den Wert.
    unsafe { WinHttpSetOption(h, opt, &v as *const u32 as *const c_void, 4) != 0 }
}

pub struct WinHttp {
    session: H,
}

// Das Handle wird nur im Faden benutzt, der es angelegt hat (die Prüfung läuft komplett in einem Hintergrundfaden).
impl WinHttp {
    pub fn open() -> Result<Self, FetchError> {
        let ua = wide(&super::user_agent());
        // SAFETY: Zeiger auf gültige, nullterminierte UTF-16-Puffer; Proxy-Einstellungen des Systems (Windows 8.1+).
        let mut s = unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0) };
        if s.is_null() {
            // Ältere Systeme kennen AUTOMATIC_PROXY nicht.
            // SAFETY: wie oben.
            s = unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, std::ptr::null(), std::ptr::null(), 0) };
        }
        if s.is_null() {
            return Err(sys("WinHttpOpen", 0));
        }
        let session = H(s);
        // Nur TLS 1.2/1.3 (1.3 kennt nicht jedes Windows 10 – dann nur 1.2).
        if !set_u32(s, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3)
            && !set_u32(s, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2)
        {
            return Err(sys("WinHttpSetOption(SECURE_PROTOCOLS)", 0));
        }
        // Weiterleitungen prüft `fetch` selbst – ohne diese Einstellung keine Anfrage.
        if !set_u32(s, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER) {
            return Err(sys("WinHttpSetOption(REDIRECT_POLICY)", 0));
        }
        // Zeitlimits: Namensauflösung, Verbindung, Senden, Empfangen (je Block).
        // SAFETY: gültiges Session-Handle.
        unsafe { WinHttpSetTimeouts(s, 15_000, 15_000, 30_000, 30_000) };
        Ok(Self { session })
    }
}

/// Felder in Schließ-Reihenfolge: erst die Anfrage, dann die Verbindung, zuletzt der Kontext des Rückrufs.
struct Resp {
    req: H,
    _conn: H,
    tls: Box<AtomicU32>,
}

impl Resp {
    fn query_u32(&self, level: u32) -> Option<u32> {
        let mut v: u32 = 0;
        let mut len: u32 = 4;
        // SAFETY: Puffer `v` mit Länge 4; WINHTTP_QUERY_FLAG_NUMBER liefert eine Zahl.
        let ok = unsafe { WinHttpQueryHeaders(self.req.0, level | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut v as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) };
        (ok != 0).then_some(v)
    }
}

impl Response for Resp {
    fn status(&self) -> u16 {
        self.query_u32(WINHTTP_QUERY_STATUS_CODE).unwrap_or(0) as u16
    }

    fn location(&self) -> Option<String> {
        let mut len: u32 = 0;
        // SAFETY: erster Aufruf nur zur Größenabfrage (Puffer null).
        unsafe { WinHttpQueryHeaders(self.req.0, WINHTTP_QUERY_LOCATION, std::ptr::null(), std::ptr::null_mut(), &mut len, std::ptr::null_mut()) };
        // SAFETY: liest nur den Fehlerwert.
        if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || len == 0 || len > 16 << 10 {
            return None;
        }
        let mut buf = vec![0u16; len as usize / 2 + 1];
        // SAFETY: Puffer mit `len` Bytes Platz.
        let ok = unsafe { WinHttpQueryHeaders(self.req.0, WINHTTP_QUERY_LOCATION, std::ptr::null(), buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut()) };
        if ok == 0 {
            return None;
        }
        buf.truncate(len as usize / 2);
        String::from_utf16(&buf).ok()
    }

    fn content_length(&self) -> Option<u64> {
        self.query_u32(WINHTTP_QUERY_CONTENT_LENGTH).map(u64::from)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, FetchError> {
        let mut n: u32 = 0;
        let want = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `buf` hat mindestens `want` Bytes.
        if unsafe { WinHttpReadData(self.req.0, buf.as_mut_ptr() as *mut c_void, want, &mut n) } == 0 {
            return Err(sys("WinHttpReadData", self.tls.load(Ordering::Relaxed)));
        }
        Ok(n as usize)
    }
}

/// Adresse, die WinHTTP tatsächlich abgerufen hat (`WINHTTP_OPTION_URL`).
fn final_url(req: *mut c_void) -> Option<String> {
    let mut len: u32 = 0;
    // SAFETY: erster Aufruf nur zur Größenabfrage (Puffer null).
    unsafe { WinHttpQueryOption(req, WINHTTP_OPTION_URL, std::ptr::null_mut(), &mut len) };
    // SAFETY: liest nur den Fehlerwert.
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || len == 0 || len > 64 << 10 {
        return None;
    }
    let mut buf = vec![0u16; len as usize / 2 + 1];
    // SAFETY: Puffer mit mindestens `len` Bytes Platz.
    if unsafe { WinHttpQueryOption(req, WINHTTP_OPTION_URL, buf.as_mut_ptr() as *mut c_void, &mut len) } == 0 {
        return None;
    }
    buf.truncate(len as usize / 2);
    String::from_utf16(&buf).ok().map(|s| s.trim_end_matches('\0').to_string())
}

impl Transport for WinHttp {
    fn get(&self, url: &str, accept: &str) -> Result<Box<dyn Response + '_>, FetchError> {
        let (host, path) = super::super::release::split_https(url).ok_or_else(|| FetchError::Blocked(url.to_string()))?;
        let whost = wide(&host);
        // Kontext des Rückrufs (Adresse bleibt fest, weil im Box); vor den Handles angelegt, damit er sie überlebt.
        let tls = Box::new(AtomicU32::new(0));
        // SAFETY: gültiges Session-Handle und nullterminierter Hostname.
        let conn = H(unsafe { WinHttpConnect(self.session.0, whost.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0) });
        if conn.0.is_null() {
            return Err(sys("WinHttpConnect", 0));
        }
        let verb = wide("GET");
        let wpath = wide(&path);
        // Adressen kommen fertig kodiert (GitHub-Weiterleitungen enthalten %-Sequenzen): nicht erneut kodieren.
        // SAFETY: gültiges Verbindungs-Handle, nullterminierte Puffer; keine Accept-Typen-Liste (Kopfzeile unten).
        let req = H(unsafe {
            WinHttpOpenRequest(conn.0, verb.as_ptr(), wpath.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), WINHTTP_FLAG_SECURE | WINHTTP_FLAG_ESCAPE_DISABLE | WINHTTP_FLAG_REFRESH)
        });
        if req.0.is_null() {
            return Err(sys("WinHttpOpenRequest", 0));
        }
        // Pflicht (sonst keine Anfrage): keine Cookies, keine Weiterleitungen, keine automatische Anmeldung (weder NTLM noch
        // Kerberos), Sperrprüfung der Zertifikate. Ist ein Sperrlisten-Server nicht erreichbar, scheitert die Anfrage mit
        // einer eigenen Meldung (`sys_text`) – die Prüfung wird nie still abgeschaltet.
        for (opt, v, call) in [
            (WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION, "WinHttpSetOption(DISABLE_FEATURE)"),
            (WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH, "WinHttpSetOption(AUTOLOGON_POLICY)"),
            (WINHTTP_OPTION_ENABLE_FEATURE, WINHTTP_ENABLE_SSL_REVOCATION, "WinHttpSetOption(ENABLE_SSL_REVOCATION)"),
        ] {
            if !set_u32(req.0, opt, v) {
                return Err(sys(call, 0));
            }
        }
        // SAFETY: gültiges Anfrage-Handle; der Rückruf liest nur `info` und den Kontext (siehe `on_status`). Scheitert das
        // Setzen, fehlen nur die Gründe in der Fehlermeldung.
        unsafe { WinHttpSetStatusCallback(req.0, Some(on_status), WINHTTP_CALLBACK_FLAG_SECURE_FAILURE, 0) };
        let ctx = &*tls as *const AtomicU32 as usize;
        let headers = wide(&format!("Accept: {accept}\r\nX-GitHub-Api-Version: 2022-11-28\r\n"));
        // SAFETY: Kopfzeilen als nullterminierter Puffer (Länge u32::MAX = bis zur Null), kein Rumpf, Kontext s. o.
        if unsafe { WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, ctx) } == 0 {
            return Err(sys("WinHttpSendRequest", tls.load(Ordering::Relaxed)));
        }
        // SAFETY: gültiges Anfrage-Handle.
        if unsafe { WinHttpReceiveResponse(req.0, std::ptr::null_mut()) } == 0 {
            return Err(sys("WinHttpReceiveResponse", tls.load(Ordering::Relaxed)));
        }
        // Zusätzlich: die tatsächlich abgerufene Adresse muss erlaubt sein und denselben Host haben (WinHTTP darf hier
        // nirgendwohin weitergeleitet haben).
        let got = final_url(req.0).unwrap_or_default();
        let same_host = super::super::release::split_https(&got).is_some_and(|(h, _)| h == host);
        if !super::allowed_url(&got) || !same_host {
            return Err(FetchError::Blocked(if got.is_empty() { url.to_string() } else { got }));
        }
        Ok(Box::new(Resp { req, _conn: conn, tls }))
    }
}
