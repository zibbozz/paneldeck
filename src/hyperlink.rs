//! Links aus OSC 8 (`ESC ] 8 ; params ; URI ST text ESC ] 8 ; ; ST`, #183): Regeln, was ein Klick tut.
//!
//! Entscheidung des Nutzers: `http`/`https` öffnen direkt (Strg+Klick wie die erkannten URLs), **jedes andere Schema
//! erst nach einer Rückfrage**, die das ganze Ziel zeigt. Nie still ausgeführt: `file:` und alles andere.
//! Zusätzlich gesperrt (auch nicht nach Rückfrage): überlange Ziele, Steuer-/Bidi-/unsichtbare Zeichen und Leerzeichen
//! im Ziel (auch nach dem Dekodieren; Täuschung in Tooltip und Rückfrage), Skript-Schemata und bekannte missbrauchte
//! Windows-Protokolle (auch die Office-Handler), `file:` auf alles außer lokalen Laufwerkspfaden (Netzwerkfreigaben
//! schicken schon beim Zugriff den NTLM-Hash) und alternative Datenströme. Ausführbare Dateien zeigt `links::open` nur im
//! Ordner. Ein http(s)-Link mit Anmeldedaten (`user@host`) oder nicht-ASCII-Rechnernamen (Homoglyphen) fragt nach.

/// Höchstlänge eines Ziels (Zeichen); längere Links sind nicht anklickbar.
pub const URI_MAX: usize = 2048;

/// Schemata, die nie geöffnet werden (Skripte, Daten, bekannte Angriffswege über Windows-Protokoll-Handler).
const DENIED: &[&str] = &[
    "javascript", "vbscript", "data", "jar", "ms-msdt", "search-ms", "search", "ms-search", "ms-officecmd", "ms-appinstaller",
    "shell", "ms-cxh", "ms-cxh-full", "mk", "its", "ms-its", "ms-word", "ms-excel", "ms-powerpoint", "ms-visio", "ms-access",
    "ms-project", "ms-publisher", "ms-spd", "ms-infopath", "hcp", "mhtml", "res", "ms-help",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// http/https mit unauffälligem Rechnernamen: direkt öffnen.
    Web,
    /// Anderes Schema (oder auffälliger http-Link): erst nach Rückfrage. `file:` trägt den lokalen Pfad.
    Ask { file: Option<String> },
    /// Nicht anklickbar.
    Denied(DenyReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    TooLong,
    BadChars,
    NoScheme,
    Scheme,
    /// `file:` auf etwas anderes als einen lokalen Laufwerkspfad (Netzwerk, Gerät, alternativer Datenstrom).
    NotLocal,
}

/// Schema eines URI (klein geschrieben), wenn er mit einem gültigen `scheme:` beginnt (RFC 3986).
pub fn scheme(uri: &str) -> Option<String> {
    let (s, _) = uri.split_once(':')?;
    let mut ch = s.chars();
    let ok = ch.next().is_some_and(|c| c.is_ascii_alphabetic()) && ch.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    // Ein Laufwerksbuchstabe („C:\…“) ist kein Schema.
    (ok && s.len() > 1).then(|| s.to_ascii_lowercase())
}

/// Zeichen, die in einem Ziel nie vorkommen dürfen (auch nicht prozent-kodiert).
fn bad_char(c: char) -> bool {
    c.is_control() || c.is_whitespace() || hidden(c)
}

/// Nach dem Dekodieren verboten (ein kodiertes Leerzeichen in einem Dateinamen ist dagegen in Ordnung).
fn hidden(c: char) -> bool {
    c.is_control() || crate::vtnotify::is_hidden_format(c)
}

/// Rechnername (Authority ohne Port) eines http(s)-Links, für Tooltip und Prüfung.
pub fn host(uri: &str) -> Option<&str> {
    let sch = scheme(uri)?;
    if sch != "http" && sch != "https" {
        return None;
    }
    let rest = uri[sch.len() + 1..].strip_prefix("//")?;
    let auth = rest.split(['/', '?', '#', '\\']).next().unwrap_or("");
    Some(auth)
}

/// Was ein Klick auf diesen Link tun darf.
pub fn classify(uri: &str) -> LinkKind {
    if uri.chars().count() > URI_MAX {
        return LinkKind::Denied(DenyReason::TooLong);
    }
    if uri.chars().any(bad_char) || percent_decode(uri).chars().any(hidden) {
        return LinkKind::Denied(DenyReason::BadChars);
    }
    let Some(sch) = scheme(uri) else { return LinkKind::Denied(DenyReason::NoScheme) };
    if DENIED.contains(&sch.as_str()) {
        return LinkKind::Denied(DenyReason::Scheme);
    }
    let rest = &uri[sch.len() + 1..];
    match sch.as_str() {
        "http" | "https" => {
            // Direkt nur mit schlichtem Rechnernamen: kein „user@“ (https://bank.de@evil.com), nur ASCII (keine
            // Homoglyphen), kein Backslash. Sonst wie andere Schemata nachfragen.
            let auth = host(uri).unwrap_or("");
            let plain = !auth.is_empty() && !auth.contains('@') && auth.is_ascii() && !rest.contains('\\');
            if plain { LinkKind::Web } else { ask_other(uri) }
        }
        "file" => match file_path(rest) {
            Some(p) => LinkKind::Ask { file: Some(p) },
            None => LinkKind::Denied(DenyReason::NotLocal),
        },
        _ => ask_other(uri),
    }
}

/// Rückfrage für ein Nicht-Datei-Ziel; Zeichen, die Kommandozeilen der Protokoll-Handler aufbrechen, sperren.
fn ask_other(uri: &str) -> LinkKind {
    if uri.contains(['"', '<', '>', '^', '`']) || percent_decode(uri).contains(['"', '<', '>', '^', '`']) {
        LinkKind::Denied(DenyReason::BadChars)
    } else {
        LinkKind::Ask { file: None }
    }
}

/// Lokaler Laufwerkspfad aus dem Teil hinter `file:` (`///C:/x`, `//localhost/C:/x`, `/C:/x`). Alles andere (fremder
/// Rechner, `/home/…`, Gerätepfade, alternative Datenströme) ergibt `None`.
fn file_path(rest: &str) -> Option<String> {
    let path = match rest.strip_prefix("//") {
        Some(r) => {
            let (host, p) = match r.find('/') {
                Some(i) => (&r[..i], &r[i..]),
                None => (r, ""),
            };
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return None;
            }
            p
        }
        None => rest,
    };
    let mut p = percent_decode(path);
    if p.chars().any(hidden) {
        return None;
    }
    if let Some(s) = p.strip_prefix('/') {
        p = s.to_string();
    }
    let p = p.replace('/', "\\");
    // Windows ignoriert Punkte/Leerzeichen am Ende; ohne sie prüfen und öffnen.
    let p = p.trim_end_matches(['.', ' ']).to_string();
    let b = p.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    // Doppelpunkt hinter dem Laufwerk = alternativer Datenstrom; `\\` mitten im Pfad könnte wieder ein UNC-Pfad werden.
    if !drive || p[2..].contains(':') || p[2..].contains("\\\\") || crate::pathfmt::is_remote_unc(&p) {
        return None;
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

/// Text auf `max` Zeichen kürzen, dabei Anfang und Ende behalten („https://host/…/datei“).
pub fn middle_truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max < 3 {
        return s.to_string();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let mut out: String = s.chars().take(head).collect();
    out.push('…');
    out.extend(s.chars().skip(n - tail));
    out
}

/// Öffnet einen Link nach der Rückfrage (oder direkt bei `Web`). Gesperrte Links tun nichts.
pub fn open(uri: &str) {
    match classify(uri) {
        // Webseiten direkt an den Browser (nie die Prüfung auf Dateiendungen: „http://x/a.exe“ ist eine Seite).
        LinkKind::Web | LinkKind::Ask { file: None } => crate::links::launch_uri(uri),
        // Dateien wie bei erkannten Pfaden: Ausführbares nur im Ordner zeigen.
        LinkKind::Ask { file: Some(p) } => {
            crate::links::open(&p);
        }
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
        assert_eq!(classify("HTTP://example.com/a.exe"), LinkKind::Web);
        assert_eq!(classify("https://example.com:8443/x"), LinkKind::Web);
        assert_eq!(host("https://example.com:8443/x?y"), Some("example.com:8443"));
        assert_eq!(classify("https:example.com"), LinkKind::Ask { file: None });
        assert_eq!(classify("https:///path"), LinkKind::Ask { file: None });
    }

    #[test]
    fn spoofed_web_links_ask() {
        // Anmeldedaten vor dem Rechnernamen, Homoglyphen, Backslash: nachfragen statt direkt öffnen.
        assert_eq!(classify("https://bank.de@evil.com/login"), LinkKind::Ask { file: None });
        assert_eq!(classify("https://exаmple.com/"), LinkKind::Ask { file: None }); // kyrillisches „а“
        assert_eq!(classify("https://example.com\\@evil.com"), LinkKind::Ask { file: None });
        assert_eq!(host("https://bank.de@evil.com/x"), Some("bank.de@evil.com"));
    }

    #[test]
    fn other_schemes_ask_first() {
        for u in ["mailto:a@b.c", "vscode://file/C:/x.rs:1", "ms-settings:display", "ssh://host", "steam://run/1"] {
            assert_eq!(classify(u), LinkKind::Ask { file: None }, "{u}");
        }
        assert_eq!(classify("file:///C:/Users/x/a%20b.txt"), LinkKind::Ask { file: Some(r"C:\Users\x\a b.txt".into()) });
        assert_eq!(classify("file:///C:/Users/x/a_b.txt"), LinkKind::Ask { file: Some("C:\\Users\\x\\a_b.txt".into()) });
        assert_eq!(classify("file://localhost/C:/x"), LinkKind::Ask { file: Some("C:\\x".into()) });
        assert_eq!(classify("file:///C:/x/setup.exe.."), LinkKind::Ask { file: Some("C:\\x\\setup.exe".into()) });
    }

    #[test]
    fn dangerous_links_are_denied() {
        let d = |r| LinkKind::Denied(r);
        assert_eq!(classify("javascript:alert(1)"), d(DenyReason::Scheme));
        assert_eq!(classify("SEARCH-MS:query=x"), d(DenyReason::Scheme));
        assert_eq!(classify("ms-msdt:/id"), d(DenyReason::Scheme));
        assert_eq!(classify("ms-word:ofe|u|https://evil/x.docx"), d(DenyReason::Scheme));
        assert_eq!(classify("hcp://x"), d(DenyReason::Scheme));
        assert_eq!(classify("data:text/html,x"), d(DenyReason::Scheme));
        assert_eq!(classify("ms-msdt:/id x"), d(DenyReason::BadChars));
        // file: nur lokale Laufwerkspfade.
        for u in ["file://evil/share/x.txt", "file:////evil/share", "file:///%5C%5Cevil%5Cshare", "file:///home/max/a",
                  "file:///C:/x/a.txt:evil.exe", "file:///%5C%5C.%5CGLOBALROOT%5CDevice%5CMup%5Cevil", "file:///C:/x//evil/s",
                  "file:///%3F%3F/UNC/evil/s"] {
            assert_eq!(classify(u), d(DenyReason::NotLocal), "{u}");
        }
        // Versteckte Zeichen, auch prozent-kodiert.
        assert_eq!(classify("https://example.com/\u{202e}gpj.exe"), d(DenyReason::BadChars));
        assert_eq!(classify("https://example.com/%E2%80%AEgpj.exe"), d(DenyReason::BadChars));
        assert_eq!(classify("https://exa\u{00ad}mple.com"), d(DenyReason::BadChars));
        assert_eq!(classify("https://exa mple.com"), d(DenyReason::BadChars));
        assert_eq!(classify("https://x/\x1b[31m"), d(DenyReason::BadChars));
        assert_eq!(classify("https://x/%0aSet-Cookie"), d(DenyReason::BadChars));
        // Zeichen, die Handler-Kommandozeilen aufbrechen.
        assert_eq!(classify("vscode://x\"--evil"), d(DenyReason::BadChars));
        assert_eq!(classify("foo:a%22b"), d(DenyReason::BadChars));
        assert_eq!(classify("foo:a^b"), d(DenyReason::BadChars));
        assert_eq!(classify("relative/path"), d(DenyReason::NoScheme));
        assert_eq!(classify("C:\\Windows\\x.exe"), d(DenyReason::NoScheme));
        let long = format!("https://example.com/{}", "a".repeat(URI_MAX));
        assert_eq!(classify(&long), d(DenyReason::TooLong));
    }

    #[test]
    fn percent_decoding_is_safe() {
        assert_eq!(percent_decode("a%20b%"), "a b%");
        assert_eq!(percent_decode("%e9%"), "\u{fffd}%");
        assert_eq!(percent_decode("x%4"), "x%4");
        assert_eq!(percent_decode("%41\u{e9}"), "A\u{e9}");
    }

    #[test]
    fn truncation_keeps_both_ends() {
        assert_eq!(middle_truncate("abcdef", 10), "abcdef");
        let t = middle_truncate("https://example.com/aaaaaaaaaaaaaaaaaaaa/file.exe", 21);
        assert_eq!(t.chars().count(), 21);
        assert!(t.starts_with("https://ex") && t.ends_with("file.exe") && t.contains('…'), "{t}");
    }
}
