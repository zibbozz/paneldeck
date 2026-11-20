//! Transport des Selbst-Updates außerhalb von Windows (#149): ureq + rustls (`platform::tls`, Stammzertifikate des
//! Systems), nur GET über HTTPS, keine Weiterleitungen (die prüft `fetch` selbst), keine Cookies, kein gzip, Proxy aus
//! der Umgebung (`HTTPS_PROXY`/`ALL_PROXY`, wie der System-Proxy bei WinHTTP). Regeln siehe `app::update::http`.

use super::{FetchError, Response, Transport};
use std::io::Read;
use std::time::Duration;

pub struct Rustls {
    agent: ureq::Agent,
}

impl Rustls {
    pub fn open() -> Result<Rustls, FetchError> {
        let tls = crate::platform::tls::client_config().map_err(FetchError::Net)?;
        let agent = ureq::AgentBuilder::new()
            .tls_config(tls)
            .https_only(true)
            .redirects(0)
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .timeout_write(Duration::from_secs(30))
            .user_agent(&super::user_agent())
            .try_proxy_from_env(true)
            .build();
        Ok(Rustls { agent })
    }
}

struct Resp {
    status: u16,
    location: Option<String>,
    len: Option<u64>,
    body: Box<dyn Read + Send + Sync + 'static>,
}

impl Response for Resp {
    fn status(&self) -> u16 {
        self.status
    }
    fn location(&self) -> Option<String> {
        self.location.clone()
    }
    fn content_length(&self) -> Option<u64> {
        self.len
    }
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, FetchError> {
        self.body.read(buf).map_err(|e| FetchError::Net(io_text(&e)))
    }
}

fn wrap(r: ureq::Response) -> Resp {
    Resp {
        status: r.status(),
        location: r.header("Location").map(str::to_string),
        len: r.header("Content-Length").and_then(|v| v.trim().parse().ok()),
        body: r.into_reader(),
    }
}

impl Transport for Rustls {
    fn get(&self, url: &str, accept: &str) -> Result<Box<dyn Response + '_>, FetchError> {
        if !url.starts_with("https://") {
            return Err(FetchError::Blocked(url.to_string()));
        }
        match self.agent.get(url).set("Accept", accept).call() {
            Ok(r) | Err(ureq::Error::Status(_, r)) => Ok(Box::new(wrap(r))),
            Err(ureq::Error::Transport(t)) => Err(FetchError::Net(transport_text(&t))),
        }
    }
}

/// Verständlicher Text (aktuelle Sprache) für einen Verbindungsfehler; das Technische folgt in Klammern.
fn transport_text(t: &ureq::Transport) -> String {
    use ureq::ErrorKind as K;
    let t_ = crate::state::t;
    let msg = if crate::platform::tls::is_cert_error(t) {
        t_(
            "Sichere Verbindung fehlgeschlagen (Zertifikat nicht vertrauenswürdig) – evtl. prüft ein Proxy HTTPS-Verbindungen; dessen Stammzertifikat gehört in den Zertifikatsspeicher des Systems.",
            "Secure connection failed (certificate not trusted) – a proxy may be intercepting HTTPS; its root certificate belongs in the system certificate store.",
        )
    } else {
        match t.kind() {
            K::Dns => t_(
                "Servername konnte nicht aufgelöst werden – keine Internetverbindung oder ein DNS-Problem.",
                "The server name could not be resolved – no internet connection or a DNS problem.",
            ),
            K::ConnectionFailed | K::ProxyConnect => t_(
                "Keine Verbindung zum Server – Internetverbindung, Proxy oder Firewall prüfen.",
                "Could not connect to the server – check the internet connection, proxy or firewall.",
            ),
            K::ProxyUnauthorized => t_("Der Proxy verlangt eine Anmeldung.", "The proxy requires authentication."),
            K::Io => t_("Die Verbindung wurde unterbrochen oder hat zu lange gedauert.", "The connection was interrupted or took too long."),
            _ => t_("Netzwerkfehler.", "Network error."),
        }
    };
    format!("{msg} ({t})")
}

fn io_text(e: &std::io::Error) -> String {
    let msg = if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
        crate::state::t("Zeitüberschreitung – der Server hat nicht rechtzeitig geantwortet.", "Timed out – the server did not answer in time.")
    } else {
        crate::state::t("Die Verbindung wurde unterbrochen.", "The connection was interrupted.")
    };
    format!("{msg} ({e})")
}
