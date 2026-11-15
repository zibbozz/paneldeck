//! Adressen der LLM-Server: Zerlegen der URL und Host-Regel.
//!
//! Erlaubt sind nur dieser Rechner und private Netze: `localhost`, 127.0.0.0/8, ::1, 10/8, 172.16/12, 192.168/16,
//! 169.254/16, fc00::/7, fe80::/10. Andere Namen werden aufgelöst – ALLE Adressen müssen erlaubt sein (die Prüfung läuft vor
//! jeder Anfrage erneut, dazu prüft der Transport die tatsächlich verbundene Adresse). Mehrdeutige Zahlenformen wie
//! `127.1`, `0x7f.0.0.1`, `0177.0.0.1` oder `2130706433` werden abgelehnt statt geraten.

use super::error::{LlmError, UrlProblem};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    /// `localhost` (ohne Auflösung erlaubt).
    Localhost,
    Ip(IpAddr),
    /// Ein anderer DNS-Name (klein geschrieben); wird vor jeder Anfrage aufgelöst und geprüft.
    Name(String),
}

/// Zerlegte, geprüfte Adresse (Form, noch ohne Host-Regel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: Scheme,
    pub host: Host,
    pub port: u16,
    /// Pfad ohne abschließenden `/` (leer = Wurzel); nur unkritische ASCII-Zeichen.
    pub path: String,
}

impl Url {
    /// Host ohne Klammern (z. B. für die Namensauflösung).
    pub fn host_name(&self) -> String {
        match &self.host {
            Host::Localhost => "localhost".into(),
            Host::Ip(ip) => ip.to_string(),
            Host::Name(n) => n.clone(),
        }
    }

    /// Host für die Anzeige und in URLs (IPv6 in Klammern).
    pub fn host_display(&self) -> String {
        match &self.host {
            Host::Ip(IpAddr::V6(ip)) => format!("[{ip}]"),
            _ => self.host_name(),
        }
    }

    pub fn default_port(&self) -> u16 {
        match self.scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }

    /// Basis ohne Pfad, z. B. `http://localhost:11434` (Standard-Port weggelassen).
    pub fn origin(&self) -> String {
        let s = match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        if self.port == self.default_port() { format!("{s}://{}", self.host_display()) } else { format!("{s}://{}:{}", self.host_display(), self.port) }
    }

    /// Volle Adresse für `path` (beginnt mit `/`) unter dem Basis-Pfad.
    pub fn join(&self, path: &str) -> String {
        format!("{}{}{}", self.origin(), self.path, path)
    }
}

fn path_ok(p: &str) -> bool {
    p.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-._~%".contains(&b))
}

/// Zerlegt `http(s)://host[:port][/pfad]`. Kein Benutzer/Passwort, keine Query, kein Fragment.
pub fn parse(url: &str) -> Result<Url, LlmError> {
    let bad = |p| Err(LlmError::BadUrl(p));
    let url = url.trim();
    if url.is_empty() {
        return bad(UrlProblem::Empty);
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return bad(UrlProblem::Chars);
    }
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) if s.eq_ignore_ascii_case("http") => (Scheme::Http, r),
        Some((s, r)) if s.eq_ignore_ascii_case("https") => (Scheme::Https, r),
        _ => return bad(UrlProblem::Scheme),
    };
    let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let (auth, tail) = rest.split_at(end);
    if auth.contains('@') {
        return bad(UrlProblem::Userinfo);
    }
    if tail.contains(['?', '#', '\\']) || !tail.is_ascii() || !path_ok(tail) {
        return bad(UrlProblem::Path);
    }
    let (host_s, port_s) = if let Some(r) = auth.strip_prefix('[') {
        let Some((h, after)) = r.split_once(']') else { return bad(UrlProblem::Host) };
        let port = match after {
            "" => None,
            p => match p.strip_prefix(':') {
                Some(p) => Some(p),
                None => return bad(UrlProblem::Host),
            },
        };
        // Zonen-IDs (`%25eth0`) gibt es hier nicht.
        let Ok(ip) = h.parse::<Ipv6Addr>() else { return bad(UrlProblem::Host) };
        (HostTok::V6(ip), port)
    } else {
        match auth.rsplit_once(':') {
            Some((h, p)) => (HostTok::Text(h), Some(p)),
            None => (HostTok::Text(auth), None),
        }
    };
    let host = match host_s {
        HostTok::V6(ip) => Host::Ip(IpAddr::V6(ip)),
        HostTok::Text(h) => parse_host(h)?,
    };
    let mut u = Url { scheme, host, port: 0, path: tail.trim_end_matches('/').to_string() };
    u.port = match port_s {
        None => u.default_port(),
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => match p.parse::<u16>() {
            Ok(n) if n > 0 => n,
            _ => return bad(UrlProblem::Port),
        },
        Some(_) => return bad(UrlProblem::Port),
    };
    Ok(u)
}

enum HostTok<'a> {
    V6(Ipv6Addr),
    Text(&'a str),
}

/// Wie im WHATWG-URL-Standard: endet ein Host auf ein Zahlen-Label, ist er als IPv4 gemeint – dann nur die eindeutige
/// Form `a.b.c.d` (dezimal, ohne führende Nullen), alles andere wird abgelehnt.
fn numeric_label(l: &str) -> bool {
    let hex = l.strip_prefix("0x").or_else(|| l.strip_prefix("0X"));
    match hex {
        Some(h) => h.bytes().all(|b| b.is_ascii_hexdigit()),
        None => !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()),
    }
}

fn parse_host(h: &str) -> Result<Host, LlmError> {
    let h = h.to_ascii_lowercase();
    if h.is_empty() || !h.is_ascii() {
        return Err(LlmError::BadUrl(UrlProblem::Host));
    }
    if h == "localhost" {
        return Ok(Host::Localhost);
    }
    let last = h.rsplit('.').next().unwrap_or("");
    if numeric_label(last) {
        return match h.parse::<Ipv4Addr>() {
            Ok(ip) => Ok(Host::Ip(IpAddr::V4(ip))),
            Err(_) => Err(LlmError::BadUrl(UrlProblem::NumericHost)),
        };
    }
    let label_ok = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if h.len() > 253 || !h.split('.').all(label_ok) {
        return Err(LlmError::BadUrl(UrlProblem::Host));
    }
    Ok(Host::Name(h))
}

/// Darf eine Verbindung zu `ip` aufgebaut werden? Nur dieser Rechner und private bzw. Link-Local-Netze.
pub fn allowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => allowed_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return allowed_v4(v4);
            }
            let s = v6.segments();
            // fe80::/10 ist erlaubt, aber ohne Zonen-ID (die in URLs hier nicht zugelassen ist) praktisch nicht erreichbar:
            // Windows weiß dann nicht, über welche Schnittstelle. Für LAN-Server IPv4 oder ULA (fc00::/7) verwenden.
            v6 == Ipv6Addr::LOCALHOST || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

fn allowed_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 127 || o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168) || (o[0] == 169 && o[1] == 254)
}

/// Namensauflösung (austauschbar für Tests).
pub type Resolver<'a> = &'a dyn Fn(&str, u16) -> std::io::Result<Vec<IpAddr>>;

pub fn system_resolve(host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
    Ok((host, port).to_socket_addrs()?.map(|a| a.ip()).collect())
}

/// Wendet die Host-Regel an (Namen werden aufgelöst). Liefert die geprüften Adressen (leer bei `localhost`).
pub fn check_with(u: &Url, resolve: Resolver) -> Result<Vec<IpAddr>, LlmError> {
    match &u.host {
        Host::Localhost => Ok(Vec::new()),
        Host::Ip(ip) => {
            if allowed_ip(*ip) {
                Ok(vec![*ip])
            } else {
                Err(LlmError::Blocked { host: u.host_display(), addr: Some(*ip) })
            }
        }
        Host::Name(n) => {
            let addrs = resolve(n, u.port).map_err(|_| LlmError::Resolve { host: n.clone() })?;
            if addrs.is_empty() {
                return Err(LlmError::Resolve { host: n.clone() });
            }
            if let Some(bad) = addrs.iter().find(|a| !allowed_ip(**a)) {
                return Err(LlmError::Blocked { host: n.clone(), addr: Some(*bad) });
            }
            Ok(addrs)
        }
    }
}

pub fn check(u: &Url) -> Result<Vec<IpAddr>, LlmError> {
    check_with(u, &system_resolve)
}

/// Zerlegen und prüfen in einem Schritt.
pub fn parse_checked(url: &str) -> Result<Url, LlmError> {
    let u = parse(url)?;
    check(&u)?;
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Url, LlmError> {
        parse(s)
    }
    fn prob(s: &str) -> UrlProblem {
        match parse(s) {
            Err(LlmError::BadUrl(p)) => p,
            other => panic!("{s}: {other:?}"),
        }
    }
    fn none(_: &str, _: u16) -> std::io::Result<Vec<IpAddr>> {
        Err(std::io::Error::other("no dns in tests"))
    }
    fn chk(s: &str) -> Result<Vec<IpAddr>, LlmError> {
        check_with(&parse(s).unwrap(), &none)
    }

    #[test]
    fn parses_basic_forms() {
        let u = p("http://localhost:11434").unwrap();
        assert_eq!((u.scheme, u.host.clone(), u.port, u.path.as_str()), (Scheme::Http, Host::Localhost, 11434, ""));
        assert_eq!(u.join("/api/tags"), "http://localhost:11434/api/tags");
        let u = p("HTTPS://LocalHost/").unwrap();
        assert_eq!((u.scheme, u.port), (Scheme::Https, 443));
        assert_eq!(u.origin(), "https://localhost");
        let u = p("http://192.168.1.5:1234/v1/").unwrap();
        assert_eq!(u.host, Host::Ip("192.168.1.5".parse().unwrap()));
        assert_eq!(u.path, "/v1");
        assert_eq!(u.join("/models"), "http://192.168.1.5:1234/v1/models");
        let u = p("http://[::1]:8080").unwrap();
        assert_eq!(u.host, Host::Ip("::1".parse().unwrap()));
        assert_eq!((u.port, u.host_name(), u.origin()), (8080, "::1".into(), "http://[::1]:8080".into()));
        let u = p("http://[fe80::1]").unwrap();
        assert_eq!(u.port, 80);
        let u = p("http://gpu-box.lan:8000").unwrap();
        assert_eq!(u.host, Host::Name("gpu-box.lan".into()));
        assert_eq!(p("http://127.0.0.1:65535").unwrap().port, 65535);
    }

    #[test]
    fn rejects_bad_forms() {
        assert_eq!(prob(""), UrlProblem::Empty);
        assert_eq!(prob("localhost:11434"), UrlProblem::Scheme);
        assert_eq!(prob("ftp://localhost"), UrlProblem::Scheme);
        assert_eq!(prob("file:///c:/x"), UrlProblem::Scheme);
        assert_eq!(prob("http://user:pw@localhost"), UrlProblem::Userinfo);
        assert_eq!(prob("http://user@127.0.0.1:1/"), UrlProblem::Userinfo);
        assert_eq!(prob("http://evil.example@localhost"), UrlProblem::Userinfo);
        assert_eq!(prob("http://localhost:0"), UrlProblem::Port);
        assert_eq!(prob("http://localhost:65536"), UrlProblem::Port);
        assert_eq!(prob("http://localhost:"), UrlProblem::Port);
        assert_eq!(prob("http://localhost:+80"), UrlProblem::Port);
        assert_eq!(prob("http://localhost:80a"), UrlProblem::Port);
        assert_eq!(prob("http://localhost:000080"), UrlProblem::Port);
        assert_eq!(prob("http://localhost/x?y=1"), UrlProblem::Path);
        assert_eq!(prob("http://localhost/#x"), UrlProblem::Path);
        assert_eq!(prob("http://localhost/ä"), UrlProblem::Path);
        assert_eq!(prob("http://localhost\\x"), UrlProblem::Path);
        assert_eq!(prob("http://local host"), UrlProblem::Chars);
        assert_eq!(prob("http://localhost\r\nX: y"), UrlProblem::Chars);
        assert_eq!(prob("http://"), UrlProblem::Host);
        assert_eq!(prob("http://:80"), UrlProblem::Host);
        assert_eq!(prob("http://[::1"), UrlProblem::Host);
        assert_eq!(prob("http://[::1]x"), UrlProblem::Host);
        assert_eq!(prob("http://[fe80::1%25eth0]"), UrlProblem::Host);
        assert_eq!(prob("http://[127.0.0.1]"), UrlProblem::Host);
        assert_eq!(prob("http://::1/"), UrlProblem::Host);
        assert_eq!(prob("http://ex_ample.lan"), UrlProblem::Host);
        assert_eq!(prob("http://-x.lan"), UrlProblem::Host);
        assert_eq!(prob("http://a..b"), UrlProblem::Host);
        assert_eq!(prob("http://localhost./"), UrlProblem::Host);
        assert_eq!(prob("http://bücher.lan"), UrlProblem::Host);
    }

    #[test]
    fn ambiguous_numeric_hosts_are_rejected() {
        for h in ["127.1", "0x7f.1", "0x7f.0.0.1", "0177.0.0.1", "2130706433", "0x7f000001", "127.0.0.01", "127.0.1", "1.2.3.4.5", "127.0.0.256", "x.0x1", "a.1"] {
            assert_eq!(prob(&format!("http://{h}/")), UrlProblem::NumericHost, "{h}");
        }
        // Nur ein Zahlen-Label am Ende macht den Host zur Zahl; Ziffern im Namen sind normal.
        assert_eq!(p("http://1.lan").unwrap().host, Host::Name("1.lan".into()));
        assert_eq!(p("http://gpu1").unwrap().host, Host::Name("gpu1".into()));
        assert_eq!(p("http://0xfoo").unwrap().host, Host::Name("0xfoo".into()));
    }

    #[test]
    fn host_policy_ipv4() {
        for ok in ["127.0.0.1", "127.255.255.254", "10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.255", "192.168.0.1", "169.254.1.1"] {
            assert!(chk(&format!("http://{ok}:1")).is_ok(), "{ok}");
        }
        for bad in ["8.8.8.8", "1.1.1.1", "0.0.0.0", "172.15.255.255", "172.32.0.1", "192.169.0.1", "100.64.0.1", "169.255.0.1", "11.0.0.1", "255.255.255.255", "224.0.0.1", "192.0.2.1"] {
            assert!(matches!(chk(&format!("http://{bad}:1")), Err(LlmError::Blocked { addr: Some(_), .. })), "{bad}");
        }
    }

    #[test]
    fn host_policy_ipv6() {
        for ok in ["::1", "fc00::1", "fd12:3456::1", "fe80::1", "febf::1", "::ffff:127.0.0.1", "::ffff:192.168.1.1"] {
            assert!(chk(&format!("https://[{ok}]")).is_ok(), "{ok}");
        }
        for bad in ["::", "2001:4860:4860::8888", "fec0::1", "fe00::1", "ff02::1", "::ffff:8.8.8.8", "64:ff9b::808:808", "2002:7f00:1::1"] {
            assert!(matches!(chk(&format!("https://[{bad}]")), Err(LlmError::Blocked { .. })), "{bad}");
        }
    }

    #[test]
    fn names_are_resolved_and_all_addresses_checked() {
        let u = parse("http://box.lan:8000").unwrap();
        let lan = |h: &str, port: u16| {
            assert_eq!((h, port), ("box.lan", 8000));
            Ok(vec!["192.168.1.2".parse().unwrap(), "fd00::2".parse().unwrap()])
        };
        assert_eq!(check_with(&u, &lan).unwrap().len(), 2);
        // Eine öffentliche Adresse unter vielen genügt zum Ablehnen (DNS-Rebinding).
        let mixed = |_: &str, _: u16| Ok(vec!["192.168.1.2".parse().unwrap(), "93.184.216.34".parse().unwrap()]);
        assert_eq!(check_with(&u, &mixed), Err(LlmError::Blocked { host: "box.lan".into(), addr: Some("93.184.216.34".parse().unwrap()) }));
        let empty = |_: &str, _: u16| Ok(vec![]);
        assert_eq!(check_with(&u, &empty), Err(LlmError::Resolve { host: "box.lan".into() }));
        assert_eq!(check_with(&u, &none), Err(LlmError::Resolve { host: "box.lan".into() }));
        // localhost braucht keine Auflösung.
        assert_eq!(check_with(&parse("http://localhost:1").unwrap(), &none), Ok(vec![]));
    }

    #[test]
    fn system_resolver_checks_real_names() {
        // Ohne Netz: `localhost` gilt direkt, eine IP-Literal-Auflösung geht ohne DNS.
        assert!(parse_checked("http://localhost:11434").is_ok());
        assert!(parse_checked("http://127.0.0.1:11434").is_ok());
        assert!(matches!(parse_checked("http://8.8.8.8"), Err(LlmError::Blocked { .. })));
    }
}
