//! Links aus OSC 8 (`ESC ] 8 ; params ; URI ST text ESC ] 8 ; ; ST`, #183): Regeln, was ein Klick tut.
//!
//! Entscheidung des Nutzers: `http`/`https` öffnen direkt (Strg+Klick wie die erkannten URLs), **jedes andere Schema
//! erst nach einer Rückfrage**, die das ganze Ziel zeigt. Nie still ausgeführt: `file:` und alles andere.
//! Zusätzlich gesperrt (auch nicht nach Rückfrage): überlange Ziele, Steuer-/Bidi-/Leerzeichen im Ziel (Täuschung in
//! Tooltip und Rückfrage), Skript-Schemata und bekannte missbrauchte Windows-Protokolle, `file:` auf Netzwerkfreigaben
//! (schon der Zugriff schickt den NTLM-Hash). Ausführbare Dateien zeigt `links::open` nur im Ordner.

/// Höchstlänge eines Ziels (Zeichen); längere Links sind nicht anklickbar.
pub const URI_MAX: usize = 2048;

/// Schemata, die nie geöffnet werden (Skripte, Daten, bekannte Angriffswege über Windows-Protokoll-Handler).
const DENIED: &[&str] = &[
    "javascript", "vbscript", "data", "jar", "ms-msdt", "search-ms", "search", "ms-search", "ms-officecmd", "ms-appinstaller",
    "shell", "ms-cxh", "ms-cxh-full", "mk", "its", "ms-its",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// http/https: direkt öffnen.
    Web,
    /// Anderes Schema: erst nach Rückfrage. `file:` trägt den lokalen Pfad.
    Ask { file: Option<String> },
    /// Nicht anklickbar (Grund für den Tooltip).
    Denied(DenyReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    TooLong,
    BadChars,
    NoScheme,
    Scheme,
    NetworkPath,
}

/// Schema eines URI (klein geschrieben), wenn er mit einem gültigen `scheme:` beginnt (RFC 3986).
pub fn scheme(uri: &str) -> Option<String> {
    let (s, _) = uri.split_once(':')?;
    let mut ch = s.chars();
    let ok = ch.next().is_some_and(|c| c.is_ascii_alphabetic()) && ch.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    // Ein Laufwerksbuchstabe („C:\…“) ist kein Schema.
    (ok && s.len() > 1).then(|| s.to_ascii_lowercase())
}

fn bad_char(c: char) -> bool {
    c.is_control() || c.is_whitespace() || matches!(c, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// Was ein Klick auf diesen Link tun darf.
pub fn classify(uri: &str) -> LinkKind {
    if uri.chars().count() > URI_MAX {
        return LinkKind::Denied(DenyReason::TooLong);
    }
    if uri.chars().any(bad_char) {
        return LinkKind::Denied(DenyReason::BadChars);
    }
    let Some(sch) = scheme(uri) else { return LinkKind::Denied(DenyReason::NoScheme) };
    if DENIED.contains(&sch.as_str()) {
        return LinkKind::Denied(DenyReason::Scheme);
    }
    let rest = &uri[sch.len() + 1..];
    match sch.as_str() {
        "http" | "https" => {
            // Nur mit Rechnername („https://host…“); Seltsames wie „https:foo“ fragt nach.
            let host = rest.strip_prefix("//").map(|r| r.split(['/', '?', '#']).next().unwrap_or(""));
            if host.is_some_and(|h| !h.is_empty()) { LinkKind::Web } else { LinkKind::Ask { file: None } }
        }
        "file" => match file_path(rest) {
            Some(p) if crate::pathfmt::is_remote_unc(&p) => LinkKind::Denied(DenyReason::NetworkPath),
            Some(p) => LinkKind::Ask { file: Some(p) },
            None => LinkKind::Denied(DenyReason::NetworkPath),
        },
        _ => LinkKind::Ask { file: None },
    }
}

/// Lokaler Pfad aus dem Teil hinter `file:` (`///C:/x`, `//localhost/C:/x`, `/C:/x`, `///home/x`). Ein fremder Rechnername
/// ergibt einen UNC-Pfad (`\\host\…`), den `classify` sperrt.
fn file_path(rest: &str) -> Option<String> {
    let path = if let Some(r) = rest.strip_prefix("//") {
        let (host, p) = match r.find('/') {
            Some(i) => (&r[..i], &r[i..]),
            None => (r, ""),
        };
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            return Some(format!("\\\\{host}{}", percent_decode(p).replace('/', "\\")));
        }
        p.to_string()
    } else {
        rest.to_string()
    };
    let mut p = percent_decode(&path);
    let b = p.as_bytes();
    if b.len() > 2 && b[0] == b'/' && b[2] == b':' && b[1].is_ascii_alphabetic() {
        p.remove(0);
    }
    if p.is_empty() {
        return None;
    }
    // Windows-Schreibweise für Laufwerkspfade; nach dem Dekodieren nochmals auf UNC prüfen („%5C%5Cserver“).
    if p.as_bytes().get(1) == Some(&b':') {
        p = p.replace('/', "\\");
    }
    Some(p)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Öffnet einen Link nach der Rückfrage (oder direkt bei `Web`). Gesperrte Links tun nichts.
pub fn open(uri: &str) {
    match classify(uri) {
        LinkKind::Web => {
            crate::links::open(uri);
        }
        // Dateien wie bei erkannten Pfaden: Ausführbares nur im Ordner zeigen.
        LinkKind::Ask { file: Some(p) } => {
            crate::links::open(&p);
        }
        LinkKind::Ask { file: None } => crate::links::launch_uri(uri),
        LinkKind::Denied(_) => {}
    }
}

/// Übergabe der Rückfrage vom Terminal an die App (über die temporären egui-Daten): Fenster und Ziel.
fn request_id() -> eframe::egui::Id {
    eframe::egui::Id::new("paneldeck-osc8-link-ask")
}

pub fn request_confirm(ctx: &eframe::egui::Context, uri: &str) {
    let vp = ctx.viewport_id();
    ctx.data_mut(|d| d.insert_temp(request_id(), (vp, uri.to_string())));
}

pub fn take_request(ctx: &eframe::egui::Context) -> Option<(eframe::egui::ViewportId, String)> {
    ctx.data_mut(|d| d.remove_temp::<(eframe::egui::ViewportId, String)>(request_id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes() {
        assert_eq!(scheme("HTTPS://x").as_deref(), Some("https"));
        assert_eq!(scheme("vscode://file/x").as_deref(), Some("vscode"));
        assert_eq!(scheme("ms-settings:display").as_deref(), Some("ms-settings"));
        assert_eq!(scheme("C:\\x"), None);
        assert_eq!(scheme("1abc:x"), None);
        assert_eq!(scheme("no scheme"), None);
        assert_eq!(scheme(":x"), None);
    }

    #[test]
    fn web_links_open_directly() {
        assert_eq!(classify("https://example.com/a?b=1#c"), LinkKind::Web);
        assert_eq!(classify("HTTP://example.com"), LinkKind::Web);
        assert_eq!(classify("https:example.com"), LinkKind::Ask { file: None });
        assert_eq!(classify("https:///path"), LinkKind::Ask { file: None });
    }

    #[test]
    fn other_schemes_ask_first() {
        for u in ["mailto:a@b.c", "vscode://file/C:/x.rs:1", "ms-settings:display", "ssh://host", "steam://run/1"] {
            assert_eq!(classify(u), LinkKind::Ask { file: None }, "{u}");
        }
        assert_eq!(classify("file:///C:/Users/x/a%20b.txt"), LinkKind::Ask { file: Some("C:\\Users\\x\\a b.txt".into()) });
        assert_eq!(classify("file://localhost/C:/x"), LinkKind::Ask { file: Some("C:\\x".into()) });
        assert_eq!(classify("file:///home/max/a"), LinkKind::Ask { file: Some("/home/max/a".into()) });
    }

    #[test]
    fn dangerous_links_are_denied() {
        assert_eq!(classify("javascript:alert(1)"), LinkKind::Denied(DenyReason::Scheme));
        assert_eq!(classify("SEARCH-MS:query=x"), LinkKind::Denied(DenyReason::Scheme));
        assert_eq!(classify("ms-msdt:/id x"), LinkKind::Denied(DenyReason::BadChars));
        assert_eq!(classify("ms-msdt:/id"), LinkKind::Denied(DenyReason::Scheme));
        assert_eq!(classify("data:text/html,x"), LinkKind::Denied(DenyReason::Scheme));
        assert_eq!(classify("file://evil/share/x.txt"), LinkKind::Denied(DenyReason::NetworkPath));
        assert_eq!(classify("file:////evil/share"), LinkKind::Denied(DenyReason::NetworkPath));
        assert_eq!(classify("file:///%5C%5Cevil%5Cshare"), LinkKind::Denied(DenyReason::NetworkPath));
        assert_eq!(classify("https://example.com/\u{202e}gpj.exe"), LinkKind::Denied(DenyReason::BadChars));
        assert_eq!(classify("https://exa mple.com"), LinkKind::Denied(DenyReason::BadChars));
        assert_eq!(classify("https://x/\x1b[31m"), LinkKind::Denied(DenyReason::BadChars));
        assert_eq!(classify("relative/path"), LinkKind::Denied(DenyReason::NoScheme));
        assert_eq!(classify("C:\\Windows\\x.exe"), LinkKind::Denied(DenyReason::NoScheme));
        let long = format!("https://example.com/{}", "a".repeat(URI_MAX));
        assert_eq!(classify(&long), LinkKind::Denied(DenyReason::TooLong));
    }

    #[test]
    fn percent_decoding_is_safe() {
        assert_eq!(percent_decode("a%20b%"), "a b%");
        assert_eq!(percent_decode("%e9%"), "\u{fffd}%");
        assert_eq!(percent_decode("x%4"), "x%4");
        assert_eq!(percent_decode("%41\u{e9}"), "A\u{e9}");
    }
}
