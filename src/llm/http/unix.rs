//! LLM-Transport außerhalb von Windows (#149): ureq + rustls (`platform::tls`, Stammzertifikate des Systems).
//!
//! Gleiche Regeln wie WinHTTP (`llm::http`): Host-Regel vor JEDER Anfrage (`net::check_with`, Namen neu aufgelöst), es
//! wird nur mit den geprüften Adressen verbunden (eigener Resolver – eine andere Adresse kann ureq gar nicht erreichen),
//! `localhost` = erst 127.0.0.1, dann ::1 (#299); kein Proxy, keine Weiterleitungen, keine Cookies, kein gzip;
//! Zertifikate immer geprüft. Die Anfrage läuft in einem eigenen Thread, damit `cancel` sofort wirkt – auch während auf
//! das erste Token gewartet wird (der Thread endet dann mit der nächsten Antwort oder dem Zeitlimit).

use super::{HttpRequest, HttpResponse, Transport, CONNECT_TIMEOUT, SEND_TIMEOUT};
use crate::llm::error::{code, LlmError};
use crate::llm::net;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

/// Transport über ureq. `resolve` ist austauschbar (Tests); Standard = Systemauflösung.
pub struct Ureq {
    pub resolve: fn(&str, u16) -> std::io::Result<Vec<IpAddr>>,
}

impl Default for Ureq {
    fn default() -> Self {
        Self { resolve: net::system_resolve }
    }
}

enum Msg {
    Head(u16),
    Data(Vec<u8>),
    End,
    Fail(LlmError),
}

struct Resp {
    status: u16,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    pending: Vec<u8>,
    done: bool,
}

/// Wartet auf die nächste Nachricht und prüft dabei alle 50 ms `cancel`.
fn next(rx: &Receiver<Msg>, cancel: &AtomicBool) -> Result<Msg, LlmError> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(m) => return Ok(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(LlmError::Sys { code: code::CONNECTION_ERROR, call: "read" }),
        }
    }
}

impl HttpResponse for Resp {
    fn status(&self) -> u16 {
        self.status
    }
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LlmError> {
        while self.pending.is_empty() {
            if self.done {
                return Ok(0);
            }
            match next(&self.rx, &self.cancel)? {
                Msg::Data(d) => self.pending = d,
                Msg::End => self.done = true,
                Msg::Fail(e) => return Err(e),
                Msg::Head(_) => {}
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

/// Fehler von ureq in die Fehler dieses Moduls (gleiche Texte wie unter Windows, ohne WinHTTP-Codes im Wortlaut).
fn map_transport(t: &ureq::Transport) -> LlmError {
    use ureq::ErrorKind as K;
    if crate::platform::tls::is_cert_error(t) {
        return LlmError::Cert { code: code::INVALID_CA, flags: 0 };
    }
    let c = match t.kind() {
        K::Dns => code::NAME_NOT_RESOLVED,
        K::ConnectionFailed => code::CANNOT_CONNECT,
        K::Io if is_timeout(t) => code::TIMEOUT,
        K::Io => code::CONNECTION_ERROR,
        _ => code::SECURE_FAILURE,
    };
    LlmError::Sys { code: c, call: "ureq" }
}

fn is_timeout(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return matches!(io.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock);
        }
        cur = err.source();
    }
    false
}

fn map_io(e: &std::io::Error) -> LlmError {
    let c = if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) { code::TIMEOUT } else { code::CONNECTION_ERROR };
    LlmError::Sys { code: c, call: "read" }
}

impl Transport for Ureq {
    fn send(&self, req: &HttpRequest<'_>, cancel: &Arc<AtomicBool>) -> Result<Box<dyn HttpResponse>, LlmError> {
        // Host-Regel vor jeder Anfrage (Namen werden neu aufgelöst).
        let url = net::parse(req.url)?;
        let mut addrs = net::check_with(&url, &self.resolve)?;
        if url.host == net::Host::Localhost {
            addrs = vec![IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)];
        }
        if req.bearer.is_some_and(|k| !super::bearer_ok(k)) {
            return Err(LlmError::BadKey);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        let pinned: Vec<SocketAddr> = addrs.iter().map(|ip| SocketAddr::new(*ip, url.port)).collect();
        let mut b = ureq::AgentBuilder::new()
            .resolver(move |_: &str| Ok(pinned.clone()))
            .redirects(0)
            .try_proxy_from_env(false)
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_write(SEND_TIMEOUT)
            .timeout_read(req.receive_timeout)
            .user_agent(&super::user_agent());
        if url.scheme == net::Scheme::Https {
            b = b.tls_config(crate::platform::tls::client_config().map_err(|_| LlmError::Cert { code: code::INVALID_CA, flags: 0 })?);
        }
        let agent = b.build();
        let method = req.method.as_str();
        let target = req.url.to_string();
        let accept = req.accept.to_string();
        let bearer = req.bearer.map(|k| format!("Bearer {k}"));
        let body = req.body.map(<[u8]>::to_vec);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Msg>(16);
        std::thread::spawn(move || {
            let mut r = agent.request(method, &target).set("Accept", &accept);
            if let Some(a) = &bearer {
                r = r.set("Authorization", a);
            }
            let res = match &body {
                Some(b) => r.set("Content-Type", "application/json").send_bytes(b),
                None if method == "POST" => r.send_bytes(&[]),
                None => r.call(),
            };
            let resp = match res {
                Ok(r) | Err(ureq::Error::Status(_, r)) => r,
                Err(ureq::Error::Transport(t)) => {
                    let _ = tx.send(Msg::Fail(map_transport(&t)));
                    return;
                }
            };
            if tx.send(Msg::Head(resp.status())).is_err() {
                return;
            }
            let mut reader = resp.into_reader();
            let mut buf = vec![0u8; 16 << 10];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(Msg::End);
                        return;
                    }
                    Ok(n) => {
                        if tx.send(Msg::Data(buf[..n].to_vec())).is_err() {
                            return; // abgebrochen: Empfänger weg
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Fail(map_io(&e)));
                        return;
                    }
                }
            }
        });
        let status = match next(&rx, cancel)? {
            Msg::Head(s) => s,
            Msg::Fail(e) => return Err(e),
            _ => return Err(LlmError::Sys { code: code::CONNECTION_ERROR, call: "ureq" }),
        };
        Ok(Box::new(Resp { status, rx, cancel: cancel.clone(), pending: Vec::new(), done: false }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Method;
    use super::*;
    use std::io::Write;

    fn req(url: &str) -> HttpRequest<'_> {
        HttpRequest { method: Method::Get, url, body: None, bearer: None, accept: "application/json", receive_timeout: Duration::from_secs(5) }
    }

    /// Kleiner HTTP-Server auf 127.0.0.1, beantwortet eine Anfrage mit `reply`.
    fn serve(reply: &'static [u8]) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(reply);
            }
        });
        port
    }

    #[test]
    fn local_get_streams_and_keeps_status() {
        let port = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        let url = format!("http://127.0.0.1:{port}/api/tags");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut r = Ureq::default().send(&req(&url), &cancel).unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(super::super::read_all(&mut *r, 100, &cancel).unwrap(), b"hello");
    }

    #[test]
    fn redirects_are_not_followed_and_errors_keep_their_status() {
        let port = serve(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let url = format!("http://127.0.0.1:{port}/");
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(Ureq::default().send(&req(&url), &cancel).unwrap().status(), 302);
        let port = serve(b"HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnop");
        let url = format!("http://127.0.0.1:{port}/");
        match super::super::send_ok(&Ureq::default(), &req(&url), &cancel) {
            Err(LlmError::Status { code: 404, body }) => assert_eq!(body, "nop"),
            Err(e) => panic!("{e:?}"),
            Ok(_) => panic!("404 must be an error"),
        }
    }

    #[test]
    fn host_rule_and_cancel() {
        let cancel = Arc::new(AtomicBool::new(false));
        // public address: refused before any connection
        assert!(matches!(Ureq::default().send(&req("http://8.8.8.8/"), &cancel), Err(LlmError::Blocked { .. })));
        // a name that resolves to a public address is refused as well
        let t = Ureq { resolve: |_, _| Ok(vec!["1.1.1.1".parse().unwrap()]) };
        assert!(matches!(t.send(&req("http://example.test/"), &cancel), Err(LlmError::Blocked { .. })));
        // nothing listens: "cannot connect"
        assert!(matches!(Ureq::default().send(&req("http://127.0.0.1:1/"), &cancel), Err(LlmError::Sys { code: code::CANNOT_CONNECT, .. })));
        // a server that never answers: cancel ends the wait at once
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/", l.local_addr().unwrap().port());
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.store(true, Ordering::Relaxed);
        });
        let t0 = std::time::Instant::now();
        assert!(matches!(Ureq::default().send(&req(&url), &cancel), Err(LlmError::Cancelled)));
        assert!(t0.elapsed() < Duration::from_secs(3));
        drop(l);
    }
}
