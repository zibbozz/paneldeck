//! HTTP für das Update: nur GET über HTTPS zu GitHub, mit Zeitlimits, Größenlimits und eigener Prüfung jeder Weiterleitung.
//!
//! Die eigentliche Verbindung steckt hinter `Transport` (eine Anfrage, keine automatischen Weiterleitungen). Unter Windows ist das
//! WinHTTP (System-TLS, Proxy-Einstellungen des Systems, keine Cookies, keine automatische Anmeldung); die Tests nutzen eine
//! Attrappe, Unit-Tests gehen nie ins Internet. `fetch` folgt Weiterleitungen nur zu den Hosts aus `release::HOSTS`.

use super::release::allowed_url;
use crate::state::Lang;
use std::sync::atomic::{AtomicBool, Ordering};

/// Antwort auf eine einzelne Anfrage (ohne Weiterleitungen zu folgen).
pub trait Response {
    fn status(&self) -> u16;
    fn location(&self) -> Option<String>;
    fn content_length(&self) -> Option<u64>;
    /// Liest den nächsten Block; `Ok(0)` = Ende.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, FetchError>;
}

pub trait Transport {
    /// Eine GET-Anfrage. `accept` landet im `Accept`-Kopf.
    fn get(&self, url: &str, accept: &str) -> Result<Box<dyn Response + '_>, FetchError>;
}

pub const MAX_REDIRECTS: usize = 5;

#[derive(Debug, PartialEq)]
pub enum FetchError {
    /// Adresse oder Weiterleitungsziel nicht erlaubt.
    Blocked(String),
    Status(u16),
    TooLarge,
    Cancelled,
    /// Fehler mit fertigem (zweisprachigem) Text.
    Net(String),
    /// Fehler eines WinHTTP-Aufrufs: Fehlercode, Flags der TLS-Prüfung (`WINHTTP_CALLBACK_STATUS_FLAG_*`, 0 = keine), Aufruf.
    #[cfg_attr(not(windows), allow(dead_code))]
    Sys { code: u32, tls: u32, call: &'static str },
}

/// Fehlercodes und Flags von WinHTTP (winhttp.h) als Zahlen, damit die Texte auch ohne Windows testbar sind.
mod code {
    pub const TIMEOUT: u32 = 12002;
    pub const NAME_NOT_RESOLVED: u32 = 12007;
    pub const CANNOT_CONNECT: u32 = 12029;
    pub const CONNECTION_ERROR: u32 = 12030;
    pub const CERT_REV_FAILED: u32 = 12057;
    pub const CERT_REVOKED: u32 = 12170;
    pub const SECURE_FAILURE: u32 = 12175;
    /// Weitere TLS-/Zertifikatsfehler (Datum, Name, Aussteller, Kanal, Verwendung, Proxy).
    pub const TLS: [u32; 7] = [12037, 12038, 12045, 12157, 12169, 12179, 12188];
    pub const FLAG_REV_FAILED: u32 = 1;
    pub const FLAG_REVOKED: u32 = 4;
}

/// Verständlicher Text für einen WinHTTP-Fehler; das Technische folgt in Klammern.
fn sys_text(c: u32, tls: u32, call: &str, lang: Lang) -> String {
    let secure = c == code::SECURE_FAILURE;
    let msg = if c == code::CERT_REVOKED || (secure && tls & code::FLAG_REVOKED != 0) {
        lang.t(
            "Das Zertifikat des Servers wurde widerrufen – die Verbindung ist nicht vertrauenswürdig, es wird nichts geladen.",
            "The server's certificate has been revoked – the connection is not trustworthy, nothing is downloaded.",
        )
    } else if c == code::CERT_REV_FAILED || (secure && tls & code::FLAG_REV_FAILED != 0) {
        lang.t(
            "Die Sperrprüfung des Zertifikats war nicht möglich (Server für Sperrlisten nicht erreichbar). Aus Sicherheitsgründen wird nichts geladen – später erneut versuchen oder Firewall/Proxy prüfen.",
            "The certificate revocation check could not be done (revocation server unreachable). For safety nothing is downloaded – try again later or check the firewall/proxy.",
        )
    } else if secure || code::TLS.contains(&c) {
        lang.t(
            "Sichere Verbindung fehlgeschlagen (TLS/Zertifikat) – evtl. prüft ein Proxy oder Virenscanner HTTPS-Verbindungen.",
            "Secure connection failed (TLS/certificate) – a proxy or antivirus program may be intercepting HTTPS.",
        )
    } else {
        match c {
            code::TIMEOUT => lang.t("Zeitüberschreitung – der Server hat nicht rechtzeitig geantwortet.", "Timed out – the server did not answer in time."),
            code::NAME_NOT_RESOLVED => lang.t(
                "Servername konnte nicht aufgelöst werden – keine Internetverbindung oder ein DNS-Problem.",
                "The server name could not be resolved – no internet connection or a DNS problem.",
            ),
            code::CANNOT_CONNECT => lang.t(
                "Keine Verbindung zum Server – Internetverbindung, Proxy oder Firewall prüfen.",
                "Could not connect to the server – check the internet connection, proxy or firewall.",
            ),
            code::CONNECTION_ERROR => lang.t("Die Verbindung wurde unterbrochen.", "The connection was interrupted."),
            _ => lang.t("Netzwerkfehler.", "Network error."),
        }
    };
    let flags = if tls != 0 { format!(", TLS 0x{tls:x}") } else { String::new() };
    format!("{msg} (WinHTTP {c}{flags}, {call})")
}

impl FetchError {
    pub fn text(&self, lang: Lang) -> String {
        match self {
            FetchError::Blocked(u) => format!("{} ({u})", lang.t("Adresse nicht erlaubt – nur GitHub über HTTPS", "Address not allowed – only GitHub over HTTPS")),
            FetchError::Status(404) => lang.t("Nicht gefunden (HTTP 404).", "Not found (HTTP 404).").into(),
            FetchError::Status(403) => lang.t("Von GitHub abgelehnt, evtl. zu viele Abfragen – später erneut versuchen (HTTP 403).", "Refused by GitHub, possibly rate limited – try again later (HTTP 403).").into(),
            FetchError::Status(407) => lang
                .t(
                    "Der Proxy verlangt eine Anmeldung – das unterstützt Paneldeck nicht. Bitte das Update von der Release-Seite laden (HTTP 407).",
                    "The proxy requires a login – Paneldeck does not support that. Please download the update from the release page (HTTP 407).",
                )
                .into(),
            FetchError::Status(s) => format!("{} (HTTP {s})", lang.t("Unerwartete Antwort des Servers", "Unexpected answer from the server")),
            FetchError::TooLarge => lang.t("Die Antwort ist größer als erlaubt.", "The response is larger than allowed.").into(),
            FetchError::Cancelled => lang.t("Abgebrochen.", "Cancelled.").into(),
            FetchError::Net(e) => format!("{} ({e})", lang.t("Netzwerkfehler", "Network error")),
            FetchError::Sys { code, tls, call } => sys_text(*code, *tls, call, lang),
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
        let mut r = t.get(&url, accept)?;
        match r.status() {
            301 | 302 | 303 | 307 | 308 => {
                let loc = r.location().ok_or_else(|| FetchError::Net(crate::state::t("Weiterleitung ohne Ziel", "redirect without location").into()))?;
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
            let n = r.read(&mut buf)?;
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
            return Err(FetchError::Net(format!("{} {done} / {} bytes", crate::state::t("unvollständig:", "incomplete:"), total.unwrap_or(0))));
        }
        return Ok(done);
    }
    Err(FetchError::Net(crate::state::t("zu viele Weiterleitungen", "too many redirects").into()))
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
#[cfg_attr(not(windows), allow(dead_code))]
pub fn user_agent() -> String {
    format!("Paneldeck/{}", env!("CARGO_PKG_VERSION"))
}

/// Die Verbindung für den echten Betrieb.
pub fn system_transport() -> Result<Box<dyn Transport>, FetchError> {
    #[cfg(feature = "update-test-feed")]
    if let Some(dir) = std::env::var_os("PANELDECK_UPDATE_TEST_FEED") {
        // Nur mit dem Cargo-Feature `update-test-feed` (Standard: aus, nie im Release): lokales Schein-Release, siehe `feed`.
        return Ok(Box::new(feed::FeedTransport { dir: dir.into() }));
    }
    #[cfg(windows)]
    {
        Ok(Box::new(winhttp::WinHttp::open()?))
    }
    #[cfg(not(windows))]
    {
        Err(FetchError::Net(crate::platform::unsupported()))
    }
}

#[cfg(windows)]
mod winhttp;

/// Lokales Schein-Release für Ende-zu-Ende-Tests – **nur mit dem Cargo-Feature `update-test-feed`** (Standard: aus; Release-Builds
/// über `tools/release.sh` haben es nie, das Skript prüft das) und nur über die
/// Umgebungsvariable `PANELDECK_UPDATE_TEST_FEED=<ordner>`. Der Ordner enthält `latest.json` (Antwort wie von der API) und die
/// Release-Dateien; Adressen der Form `…/releases/download/<tag>/<datei>` werden auf `<ordner>/<datei>` abgebildet. Alle Prüfungen
/// (Hosts, Größen, Prüfsummen) laufen wie im echten Betrieb. Ohne das Feature enthält die .exe diesen Code nicht.
#[cfg(any(feature = "update-test-feed", test))]
pub mod feed {
    use super::{FetchError, Response, Transport};
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
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, FetchError> {
            match &mut self.file {
                Some(f) => f.read(buf).map_err(|e| FetchError::Net(e.to_string())),
                None => Ok(0),
            }
        }
    }

    impl Transport for FeedTransport {
        fn get(&self, url: &str, _accept: &str) -> Result<Box<dyn Response + '_>, FetchError> {
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
    use super::{FetchError, Response, Transport};
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
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, FetchError> {
            // In kleinen Stücken liefern (prüft das Zusammensetzen)
            let n = (self.c.body.len() - self.pos).min(buf.len()).min(7);
            buf[..n].copy_from_slice(&self.c.body[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    impl Transport for Fake {
        fn get(&self, url: &str, _accept: &str) -> Result<Box<dyn Response + '_>, FetchError> {
            self.log.borrow_mut().push(url.to_string());
            let c = self.map.get(url).cloned().ok_or_else(|| FetchError::Net(format!("unexpected request {url}")))?;
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
    fn error_texts_are_explained() {
        let t = |e: FetchError, l| e.text(l);
        let sys = |code, tls| FetchError::Sys { code, tls, call: "WinHttpSendRequest" };
        assert!(t(FetchError::Status(407), Lang::En).contains("proxy requires a login"));
        assert!(t(FetchError::Status(407), Lang::De).contains("Proxy verlangt eine Anmeldung"));
        assert!(t(sys(12002, 0), Lang::En).starts_with("Timed out"));
        assert!(t(sys(12007, 0), Lang::De).contains("nicht aufgelöst"));
        assert!(t(sys(12029, 0), Lang::En).contains("Could not connect"));
        assert!(t(sys(12175, 0), Lang::En).contains("Secure connection failed"));
        // Sperrprüfung nicht möglich: eigene Meldung (über den Fehlercode oder die Flags des Rückrufs)
        assert!(t(sys(12175, 1), Lang::En).contains("revocation check could not be done"), "{}", t(sys(12175, 1), Lang::En));
        assert!(t(sys(12057, 0), Lang::De).contains("Sperrprüfung"));
        assert!(t(sys(12175, 4), Lang::En).contains("revoked"));
        // Technisches nur in Klammern dahinter
        let s = t(sys(12029, 0), Lang::En);
        assert!(s.ends_with("(WinHTTP 12029, WinHttpSendRequest)"), "{s}");
        assert!(t(sys(12345, 0), Lang::En).starts_with("Network error."));
        assert!(t(FetchError::Blocked("https://evil.example/".into()), Lang::En).starts_with("Address not allowed"));
    }

    #[test]
    fn user_agent_is_neutral() {
        assert_eq!(user_agent(), format!("Paneldeck/{}", env!("CARGO_PKG_VERSION")));
    }
}
