//! HTTP für das Update: nur GET über HTTPS zu GitHub, mit Zeitlimits, Größenlimits und eigener Prüfung jeder Weiterleitung.
//!
//! Die eigentliche Verbindung steckt hinter `Transport` (eine Anfrage, keine automatischen Weiterleitungen). Unter Windows ist das
//! WinHTTP (System-TLS, Proxy-Einstellungen des Systems, keine Cookies, keine automatische Anmeldung); die Tests nutzen eine
//! Attrappe, Unit-Tests gehen nie ins Internet. `fetch` folgt Weiterleitungen nur zu den Hosts aus `release::HOSTS`.

use super::release::allowed_url;
use std::sync::atomic::{AtomicBool, Ordering};

/// Antwort auf eine einzelne Anfrage (ohne Weiterleitungen zu folgen).
pub trait Response {
    fn status(&self) -> u16;
    fn location(&self) -> Option<String>;
    fn content_length(&self) -> Option<u64>;
    /// Liest den nächsten Block; `Ok(0)` = Ende.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String>;
}

pub trait Transport {
    /// Eine GET-Anfrage. `accept` landet im `Accept`-Kopf.
    fn get(&self, url: &str, accept: &str) -> Result<Box<dyn Response + '_>, String>;
}

pub const MAX_REDIRECTS: usize = 5;

#[derive(Debug, PartialEq)]
pub enum FetchError {
    /// Adresse oder Weiterleitungsziel nicht erlaubt.
    Blocked(String),
    Status(u16),
    TooLarge,
    Cancelled,
    Net(String),
}

impl FetchError {
    pub fn text(&self, lang: crate::state::Lang) -> String {
        match self {
            FetchError::Blocked(u) => format!("{} {u}", lang.t("Adresse nicht erlaubt:", "Address not allowed:")),
            FetchError::Status(404) => lang.t("nicht gefunden (HTTP 404)", "not found (HTTP 404)").into(),
            FetchError::Status(403) => lang.t("von GitHub abgelehnt (HTTP 403, evtl. zu viele Abfragen – später erneut versuchen)", "refused by GitHub (HTTP 403, possibly rate limited – try again later)").into(),
            FetchError::Status(s) => format!("HTTP {s}"),
            FetchError::TooLarge => lang.t("Antwort zu groß", "response too large").into(),
            FetchError::Cancelled => lang.t("abgebrochen", "cancelled").into(),
            FetchError::Net(e) => format!("{}: {e}", lang.t("Netzwerkfehler", "network error")),
        }
    }
}

/// Lädt `url` (Weiterleitungen nur zu erlaubten Hosts, höchstens `MAX_REDIRECTS`) und gibt jeden Datenblock an `sink`.
/// Bricht ab, sobald mehr als `max` Bytes kommen (auch wenn `Content-Length` lügt) oder `cancel` gesetzt wird.
/// `progress(geladen, gesamt)` wird nach jedem Block aufgerufen. Liefert die Anzahl der Bytes.
pub fn fetch(
    t: &dyn Transport,
    url: &str,
    accept: &str,
    max: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
    sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
) -> Result<u64, FetchError> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        if !allowed_url(&url) {
            return Err(FetchError::Blocked(url));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        let mut r = t.get(&url, accept).map_err(FetchError::Net)?;
        match r.status() {
            301 | 302 | 303 | 307 | 308 => {
                let loc = r.location().ok_or_else(|| FetchError::Net("redirect without location".into()))?;
                // Relative Weiterleitungen (selten bei GitHub) gibt es hier nicht: nur absolute, geprüfte HTTPS-Adressen.
                url = loc;
                continue;
            }
            200 => {}
            s => return Err(FetchError::Status(s)),
        }
        let total = r.content_length();
        if total.is_some_and(|n| n > max) {
            return Err(FetchError::TooLarge);
        }
        let mut done = 0u64;
        let mut buf = vec![0u8; 64 << 10];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(FetchError::Cancelled);
            }
            let n = r.read(&mut buf).map_err(FetchError::Net)?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > max {
                return Err(FetchError::TooLarge);
            }
            sink(&buf[..n]).map_err(FetchError::Net)?;
            progress(done, total);
        }
        if total.is_some_and(|n| n != done) {
            return Err(FetchError::Net(format!("incomplete ({done} of {} bytes)", total.unwrap_or(0))));
        }
        return Ok(done);
    }
    Err(FetchError::Net("too many redirects".into()))
}

/// Ganze Antwort in den Speicher (für kleine Dateien: JSON, Prüfsummen).
pub fn fetch_bytes(t: &dyn Transport, url: &str, accept: &str, max: u64, cancel: &AtomicBool) -> Result<Vec<u8>, FetchError> {
    let mut out = Vec::new();
    fetch(t, url, accept, max, cancel, &mut |_, _| {}, &mut |b| {
        out.extend_from_slice(b);
        Ok(())
    })?;
    Ok(out)
}

/// User-Agent ohne persönliche Daten.
pub fn user_agent() -> String {
    format!("Paneldeck/{}", env!("CARGO_PKG_VERSION"))
}

/// Die Verbindung für den echten Betrieb.
pub fn system_transport() -> Result<Box<dyn Transport>, String> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("PANELDECK_UPDATE_TEST_FEED") {
        // Nur in Debug-Builds (`cargo build` ohne --release): lokales Schein-Release zum Testen, siehe `feed::FeedTransport`.
        return Ok(Box::new(feed::FeedTransport { dir: dir.into() }));
    }
    #[cfg(windows)]
    {
        Ok(Box::new(winhttp::WinHttp::open()?))
    }
    #[cfg(not(windows))]
    {
        Err(crate::state::t("nur unter Windows", "Windows only").into())
    }
}

#[cfg(windows)]
mod winhttp {
    use super::{Response, Transport};
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER};
    use windows_sys::Win32::Networking::WinHttp::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_err(what: &str) -> String {
        // SAFETY: GetLastError liest nur den Fehlerwert des aktuellen Threads.
        format!("{what} (WinHTTP {})", unsafe { GetLastError() })
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
        pub fn open() -> Result<Self, String> {
            let ua = wide(&super::user_agent());
            // SAFETY: Zeiger auf gültige, nullterminierte UTF-16-Puffer; Proxy-Einstellungen des Systems (Windows 8.1+).
            let mut s = unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0) };
            if s.is_null() {
                // Ältere Systeme kennen AUTOMATIC_PROXY nicht.
                // SAFETY: wie oben.
                s = unsafe { WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, std::ptr::null(), std::ptr::null(), 0) };
            }
            if s.is_null() {
                return Err(last_err("WinHttpOpen"));
            }
            let session = H(s);
            // Nur TLS 1.2/1.3 (1.3 kennt nicht jedes Windows 10 – dann nur 1.2).
            if !set_u32(s, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3) {
                set_u32(s, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2);
            }
            // Weiterleitungen prüft `fetch` selbst.
            set_u32(s, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER);
            // Zeitlimits: Namensauflösung, Verbindung, Senden, Empfangen (je Block).
            // SAFETY: gültiges Session-Handle.
            unsafe { WinHttpSetTimeouts(s, 15_000, 15_000, 30_000, 30_000) };
            Ok(Self { session })
        }
    }

    struct Resp {
        _conn: H,
        req: H,
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

        fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
            let mut n: u32 = 0;
            let want = buf.len().min(u32::MAX as usize) as u32;
            // SAFETY: `buf` hat mindestens `want` Bytes.
            if unsafe { WinHttpReadData(self.req.0, buf.as_mut_ptr() as *mut c_void, want, &mut n) } == 0 {
                return Err(last_err("WinHttpReadData"));
            }
            Ok(n as usize)
        }
    }

    impl Transport for WinHttp {
        fn get(&self, url: &str, accept: &str) -> Result<Box<dyn Response + '_>, String> {
            let (host, path) = super::super::release::split_https(url).ok_or("url")?;
            let whost = wide(&host);
            // SAFETY: gültiges Session-Handle und nullterminierter Hostname.
            let conn = H(unsafe { WinHttpConnect(self.session.0, whost.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0) });
            if conn.0.is_null() {
                return Err(last_err("WinHttpConnect"));
            }
            let verb = wide("GET");
            let wpath = wide(&path);
            // Adressen kommen fertig kodiert (GitHub-Weiterleitungen enthalten %-Sequenzen): nicht erneut kodieren.
            // SAFETY: gültiges Verbindungs-Handle, nullterminierte Puffer; keine Accept-Typen-Liste (Kopfzeile unten).
            let req = H(unsafe {
                WinHttpOpenRequest(conn.0, verb.as_ptr(), wpath.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), WINHTTP_FLAG_SECURE | WINHTTP_FLAG_ESCAPE_DISABLE | WINHTTP_FLAG_REFRESH)
            });
            if req.0.is_null() {
                return Err(last_err("WinHttpOpenRequest"));
            }
            // Keine Cookies, keine Weiterleitungen, keine automatische Anmeldung (weder NTLM noch Kerberos).
            set_u32(req.0, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION);
            set_u32(req.0, WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH);
            let headers = wide(&format!("Accept: {accept}\r\nX-GitHub-Api-Version: 2022-11-28\r\n"));
            // SAFETY: Kopfzeilen als nullterminierter Puffer (Länge u32::MAX = bis zur Null), kein Rumpf.
            if unsafe { WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, 0) } == 0 {
                return Err(last_err("WinHttpSendRequest"));
            }
            // SAFETY: gültiges Anfrage-Handle.
            if unsafe { WinHttpReceiveResponse(req.0, std::ptr::null_mut()) } == 0 {
                return Err(last_err("WinHttpReceiveResponse"));
            }
            Ok(Box::new(Resp { _conn: conn, req }))
        }
    }
}

/// Lokales Schein-Release für Ende-zu-Ende-Tests – **nur in Debug-Builds** (`cargo build` ohne `--release`) und nur über die
/// Umgebungsvariable `PANELDECK_UPDATE_TEST_FEED=<ordner>`. Der Ordner enthält `latest.json` (Antwort wie von der API) und die
/// Release-Dateien; Adressen der Form `…/releases/download/<tag>/<datei>` werden auf `<ordner>/<datei>` abgebildet. Alle Prüfungen
/// (Hosts, Größen, Prüfsummen) laufen wie im echten Betrieb. Ein Release-Build enthält diesen Code nicht.
#[cfg(any(debug_assertions, test))]
pub mod feed {
    use super::{Response, Transport};
    use std::io::Read;
    use std::path::PathBuf;

    pub struct FeedTransport {
        pub dir: PathBuf,
    }

    struct FileResp {
        status: u16,
        len: Option<u64>,
        file: Option<std::fs::File>,
    }

    impl Response for FileResp {
        fn status(&self) -> u16 {
            self.status
        }
        fn location(&self) -> Option<String> {
            None
        }
        fn content_length(&self) -> Option<u64> {
            self.len
        }
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
            match &mut self.file {
                Some(f) => f.read(buf).map_err(|e| e.to_string()),
                None => Ok(0),
            }
        }
    }

    impl Transport for FeedTransport {
        fn get(&self, url: &str, _accept: &str) -> Result<Box<dyn Response + '_>, String> {
            let name = if url == super::super::release::LATEST_URL {
                Some("latest.json".to_string())
            } else {
                url.strip_prefix(super::super::release::DOWNLOAD_PREFIX).and_then(|r| r.split_once('/')).map(|(_, f)| f.to_string())
            };
            let path = name.filter(|n| !n.contains(['/', '\\']) && !n.contains("..")).map(|n| self.dir.join(n));
            match path.and_then(|p| std::fs::File::open(p).ok()) {
                Some(f) => Ok(Box::new(FileResp { status: 200, len: f.metadata().ok().map(|m| m.len()), file: Some(f) })),
                None => Ok(Box::new(FileResp { status: 404, len: None, file: None })),
            }
        }
    }
}

#[cfg(test)]
pub mod fake {
    //! Attrappe für die Tests: feste Antworten je Adresse, protokolliert alle Anfragen.
    use super::{Response, Transport};
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Clone)]
    pub struct Canned {
        pub status: u16,
        pub location: Option<String>,
        pub body: Vec<u8>,
        /// Abweichende Content-Length (z. B. gelogen); `None` = echte Länge.
        pub claimed_len: Option<Option<u64>>,
    }

    impl Canned {
        pub fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self { status: 200, location: None, body: body.into(), claimed_len: None }
        }
        pub fn redirect(to: &str) -> Self {
            Self { status: 302, location: Some(to.into()), body: Vec::new(), claimed_len: None }
        }
        pub fn status(s: u16) -> Self {
            Self { status: s, location: None, body: Vec::new(), claimed_len: None }
        }
    }

    #[derive(Default)]
    pub struct Fake {
        pub map: HashMap<String, Canned>,
        pub log: RefCell<Vec<String>>,
    }

    impl Fake {
        pub fn with(mut self, url: &str, c: Canned) -> Self {
            self.map.insert(url.into(), c);
            self
        }
    }

    struct R {
        c: Canned,
        pos: usize,
    }

    impl Response for R {
        fn status(&self) -> u16 {
            self.c.status
        }
        fn location(&self) -> Option<String> {
            self.c.location.clone()
        }
        fn content_length(&self) -> Option<u64> {
            self.c.claimed_len.unwrap_or(Some(self.c.body.len() as u64))
        }
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
            // In kleinen Stücken liefern (prüft das Zusammensetzen)
            let n = (self.c.body.len() - self.pos).min(buf.len()).min(7);
            buf[..n].copy_from_slice(&self.c.body[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    impl Transport for Fake {
        fn get(&self, url: &str, _accept: &str) -> Result<Box<dyn Response + '_>, String> {
            self.log.borrow_mut().push(url.to_string());
            let c = self.map.get(url).cloned().ok_or_else(|| format!("unexpected request {url}"))?;
            Ok(Box::new(R { c, pos: 0 }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Canned, Fake};
    use super::*;

    const GH: &str = "https://github.com/zibbozz/paneldeck/releases/download/v9.0.0/a.zip";
    const CDN: &str = "https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sig=x%2Fy";

    fn get(t: &Fake, url: &str, max: u64) -> Result<Vec<u8>, FetchError> {
        fetch_bytes(t, url, "*/*", max, &AtomicBool::new(false))
    }

    #[test]
    fn follows_allowed_redirects() {
        let t = Fake::default().with(GH, Canned::redirect(CDN)).with(CDN, Canned::ok(b"hello world, this is the asset".to_vec()));
        assert_eq!(get(&t, GH, 1000).unwrap(), b"hello world, this is the asset");
        assert_eq!(*t.log.borrow(), vec![GH.to_string(), CDN.to_string()]);
    }

    #[test]
    fn blocks_foreign_redirects() {
        for bad in ["http://release-assets.githubusercontent.com/x", "https://evil.example/x", "https://github.com.evil.example/x", "https://user@github.com/x"] {
            let t = Fake::default().with(GH, Canned::redirect(bad));
            assert_eq!(get(&t, GH, 1000), Err(FetchError::Blocked(bad.into())), "{bad}");
            assert_eq!(t.log.borrow().len(), 1, "keine Anfrage an {bad}");
        }
        // Startadresse selbst wird geprüft, bevor eine Anfrage rausgeht
        let t = Fake::default();
        assert!(matches!(get(&t, "https://example.com/", 10), Err(FetchError::Blocked(_))));
        assert!(t.log.borrow().is_empty());
    }

    #[test]
    fn redirect_limit() {
        let a = "https://github.com/a";
        let b = "https://github.com/b";
        let t = Fake::default().with(a, Canned::redirect(b)).with(b, Canned::redirect(a));
        assert!(matches!(get(&t, a, 10), Err(FetchError::Net(_))));
        assert_eq!(t.log.borrow().len(), MAX_REDIRECTS + 1);
    }

    #[test]
    fn size_limits_and_status() {
        let t = Fake::default().with(GH, Canned::ok(vec![1u8; 100]));
        assert_eq!(get(&t, GH, 99), Err(FetchError::TooLarge));
        assert_eq!(get(&t, GH, 100).unwrap().len(), 100);
        // Gelogene/fehlende Content-Length: das Limit greift trotzdem beim Lesen
        let mut c = Canned::ok(vec![1u8; 100]);
        c.claimed_len = Some(None);
        let t = Fake::default().with(GH, c);
        assert_eq!(get(&t, GH, 50), Err(FetchError::TooLarge));
        let mut c = Canned::ok(vec![1u8; 100]);
        c.claimed_len = Some(Some(10));
        let t = Fake::default().with(GH, c);
        assert_eq!(get(&t, GH, 50), Err(FetchError::TooLarge));
        // Kürzer als angekündigt = unvollständig
        let mut c = Canned::ok(vec![1u8; 10]);
        c.claimed_len = Some(Some(20));
        let t = Fake::default().with(GH, c);
        assert!(matches!(get(&t, GH, 50), Err(FetchError::Net(_))));
        let t = Fake::default().with(GH, Canned::status(404));
        assert_eq!(get(&t, GH, 50), Err(FetchError::Status(404)));
        let t = Fake::default().with(GH, Canned::status(302));
        assert!(matches!(get(&t, GH, 50), Err(FetchError::Net(_))), "Weiterleitung ohne Ziel");
    }

    #[test]
    fn cancel_and_progress() {
        let t = Fake::default().with(GH, Canned::ok(vec![7u8; 50]));
        let cancel = AtomicBool::new(true);
        assert_eq!(fetch_bytes(&t, GH, "*/*", 100, &cancel), Err(FetchError::Cancelled));
        let mut seen = Vec::new();
        let n = fetch(&t, GH, "*/*", 100, &AtomicBool::new(false), &mut |d, tot| seen.push((d, tot)), &mut |_| Ok(())).unwrap();
        assert_eq!(n, 50);
        assert_eq!(seen.last(), Some(&(50, Some(50))));
        assert!(seen.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn feed_transport_maps_files() {
        let d = std::env::temp_dir().join(format!("paneldeck-test-feed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("latest.json"), b"{}").unwrap();
        std::fs::write(d.join("a.zip"), b"zip").unwrap();
        let t = feed::FeedTransport { dir: d.clone() };
        let cancel = AtomicBool::new(false);
        assert_eq!(fetch_bytes(&t, super::super::release::LATEST_URL, "*/*", 10, &cancel).unwrap(), b"{}");
        assert_eq!(fetch_bytes(&t, GH, "*/*", 10, &cancel).unwrap(), b"zip");
        assert_eq!(fetch_bytes(&t, "https://github.com/zibbozz/paneldeck/releases/download/v9.0.0/..%2Fx", "*/*", 10, &cancel), Err(FetchError::Status(404)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn user_agent_is_neutral() {
        assert_eq!(user_agent(), format!("Paneldeck/{}", env!("CARGO_PKG_VERSION")));
    }
}
