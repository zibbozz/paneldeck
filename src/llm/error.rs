//! Fehler der LLM-Anbindung mit verständlichen Texten (Englisch/Deutsch); Technisches steht in Klammern dahinter.

use crate::state::Lang;
use std::net::IpAddr;

/// Was an einer Server-Adresse nicht stimmt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlProblem {
    Empty,
    /// Nur `http://` und `https://`.
    Scheme,
    /// `benutzer@host` ist nicht erlaubt.
    Userinfo,
    Host,
    /// Mehrdeutige Zahlenform wie `127.1` oder `0x7f.0.0.1`.
    NumericHost,
    Port,
    /// Query, Fragment oder unzulässige Zeichen im Pfad.
    Path,
    /// Leer- oder Steuerzeichen.
    Chars,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LlmError {
    BadUrl(UrlProblem),
    /// Host bzw. eine seiner Adressen liegt nicht in einem erlaubten Netz.
    Blocked { host: String, addr: Option<IpAddr> },
    /// Name konnte nicht aufgelöst werden.
    Resolve { host: String },
    /// API-Schlüssel mit Zeilenumbruch o. Ä. (würde Kopfzeilen einschleusen).
    BadKey,
    /// HTTP-Status ≥ 300 mit dem Anfang (höchstens 8 KB) des Fehlertexts.
    Status { code: u16, body: String },
    /// Zertifikat nicht vertrauenswürdig: WinHTTP-Fehlercode und Flags der TLS-Prüfung (`WINHTTP_CALLBACK_STATUS_FLAG_*`);
    /// unter Linux/macOS dieselben Codes (`llm/http/unix.rs`).
    Cert { code: u32, flags: u32 },
    Cancelled,
    /// Fehler eines WinHTTP-Aufrufs (Linux/macOS: dieselben Codes für die Fehler von ureq).
    Sys { code: u32, call: &'static str },
    /// Fehlermeldung des Servers im Datenstrom (`{"error": …}`).
    Server(String),
    /// Antwort nicht verständlich (falsches JSON, falscher Server-Typ …); Inhalt = technische Einzelheit.
    BadResponse(String),
    TooLarge,
    /// Auf diesem System nicht verfügbar (seit #149 ohne Erzeuger; bleibt für die Texte).
    #[allow(dead_code)]
    Unsupported,
}

/// WinHTTP-Fehlercodes und TLS-Flags (winhttp.h), als Zahlen, damit die Texte auch ohne Windows testbar sind.
pub(super) mod code {
    pub const TIMEOUT: u32 = 12002;
    pub const NAME_NOT_RESOLVED: u32 = 12007;
    pub const CANCELLED: u32 = 12017;
    pub const CANNOT_CONNECT: u32 = 12029;
    pub const CONNECTION_ERROR: u32 = 12030;
    pub const CERT_DATE_INVALID: u32 = 12037;
    pub const CERT_CN_INVALID: u32 = 12038;
    pub const INVALID_CA: u32 = 12045;
    pub const INVALID_CERT: u32 = 12169;
    pub const SECURE_FAILURE: u32 = 12175;
    pub const FLAG_INVALID_CERT: u32 = 0x2;
    pub const FLAG_INVALID_CA: u32 = 0x8;
    pub const FLAG_CN_INVALID: u32 = 0x10;
    pub const FLAG_DATE_INVALID: u32 = 0x20;
}

/// Ordnet einen WinHTTP-Fehler ein: Zertifikatsprobleme bekommen eine eigene Variante mit Hinweis.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn classify(c: u32, tls: u32, call: &'static str) -> LlmError {
    let cert_flags = code::FLAG_INVALID_CERT | code::FLAG_INVALID_CA | code::FLAG_CN_INVALID | code::FLAG_DATE_INVALID;
    if c == code::CANCELLED {
        LlmError::Cancelled
    } else if [code::CERT_DATE_INVALID, code::CERT_CN_INVALID, code::INVALID_CA, code::INVALID_CERT].contains(&c) || (c == code::SECURE_FAILURE && tls & cert_flags != 0) {
        LlmError::Cert { code: c, flags: tls }
    } else {
        LlmError::Sys { code: c, call }
    }
}

/// Lesbare Meldung aus einem Fehlertext (`{"error":"…"}` bei Ollama, `{"error":{"message":"…"}}` bei OpenAI), sonst der Text.
pub fn server_message(body: &str) -> String {
    let v: Option<serde_json::Value> = serde_json::from_str(body.trim()).ok();
    let msg = v.as_ref().and_then(|v| {
        let e = v.get("error")?;
        e.as_str().map(str::to_string).or_else(|| e.get("message").and_then(|m| m.as_str()).map(str::to_string))
    });
    let s = msg.unwrap_or_else(|| body.trim().to_string());
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if s.chars().count() > 300 { format!("{}…", s.chars().take(300).collect::<String>()) } else { s }
}

fn with_msg(head: String, body: &str) -> String {
    let m = server_message(body);
    if m.is_empty() { head } else { format!("{head}: {m}") }
}

impl LlmError {
    pub fn text(&self, lang: Lang) -> String {
        match self {
            LlmError::BadUrl(p) => {
                let why = match p {
                    UrlProblem::Empty => lang.t("keine Adresse angegeben", "no address given"),
                    UrlProblem::Scheme => lang.t("sie muss mit http:// oder https:// beginnen", "it must start with http:// or https://"),
                    UrlProblem::Userinfo => lang.t("Benutzername/Passwort in der Adresse sind nicht erlaubt (API-Schlüssel separat eintragen)", "a user name/password in the address is not allowed (enter an API key separately)"),
                    UrlProblem::Host => lang.t("der Servername ist ungültig", "the server name is invalid"),
                    UrlProblem::NumericHost => lang.t("mehrdeutige Zahlenadresse – bitte die Form 192.168.1.10 verwenden", "ambiguous numeric address – please use the form 192.168.1.10"),
                    UrlProblem::Port => lang.t("der Port ist ungültig (1–65535)", "the port is invalid (1–65535)"),
                    UrlProblem::Path => lang.t("der Pfad enthält unzulässige Zeichen, eine Abfrage (?) oder ein Fragment (#)", "the path contains invalid characters, a query (?) or a fragment (#)"),
                    UrlProblem::Chars => lang.t("sie enthält Leer- oder Steuerzeichen", "it contains spaces or control characters"),
                };
                format!("{}: {why}.", lang.t("Ungültige Serveradresse", "Invalid server address"))
            }
            LlmError::Blocked { host, addr } => {
                let a = match addr {
                    Some(a) if a.to_string() != host.trim_matches(['[', ']']) => format!(" ({a})"),
                    _ => String::new(),
                };
                format!(
                    "{} {host}{a} – {}",
                    lang.t("Adresse nicht erlaubt:", "Address not allowed:"),
                    lang.t(
                        "erlaubt sind nur dieser Rechner (localhost) und private Netze (10.x, 172.16–31.x, 192.168.x, 169.254.x, fc00::/7, fe80::/10).",
                        "only this computer (localhost) and private networks (10.x, 172.16–31.x, 192.168.x, 169.254.x, fc00::/7, fe80::/10) are allowed.",
                    )
                )
            }
            LlmError::Resolve { host } => format!("{} {}", lang.t("Der Servername konnte nicht aufgelöst werden:", "The server name could not be resolved:"), host),
            LlmError::BadKey => lang.t("Der API-Schlüssel enthält unzulässige Zeichen (z. B. Zeilenumbrüche).", "The API key contains invalid characters (e.g. line breaks).").into(),
            LlmError::Status { code: c @ (401 | 403), body } => {
                with_msg(format!("{} (HTTP {c})", lang.t("Zugriff verweigert – API-Schlüssel prüfen", "Access denied – check the API key")), body)
            }
            LlmError::Status { code: 404, body } => with_msg(
                format!("{} (HTTP 404)", lang.t("Nicht gefunden – Adresse, Server-Typ und Modellname prüfen", "Not found – check the address, the server type and the model name")),
                body,
            ),
            LlmError::Status { code: c @ 300..=399, .. } => {
                format!("{} (HTTP {c})", lang.t("Der Server leitet weiter – Weiterleitungen werden nicht verfolgt, bitte die Zieladresse eintragen", "The server redirects – redirects are not followed, please enter the target address"))
            }
            LlmError::Status { code, body } => with_msg(format!("{} (HTTP {code})", lang.t("Der Server meldet einen Fehler", "The server reported an error")), body),
            LlmError::Cert { code: c, flags } => {
                let why = if *c == code::CERT_CN_INVALID || flags & code::FLAG_CN_INVALID != 0 {
                    lang.t("Das Zertifikat des Servers passt nicht zum Servernamen.", "The server's certificate does not match the server name.")
                } else if *c == code::CERT_DATE_INVALID || flags & code::FLAG_DATE_INVALID != 0 {
                    lang.t("Das Zertifikat des Servers ist abgelaufen oder noch nicht gültig.", "The server's certificate has expired or is not yet valid.")
                } else {
                    lang.t(
                        "Das Zertifikat des Servers ist nicht vertrauenswürdig (selbst signiert oder unbekannte Zertifizierungsstelle).",
                        "The server's certificate is not trusted (self-signed or unknown certificate authority).",
                    )
                };
                let hint = if cfg!(windows) {
                    lang.t(
                        "Zertifikat bzw. Stammzertifikat in Windows unter „Vertrauenswürdige Stammzertifizierungsstellen“ (aktueller Benutzer) importieren oder im lokalen Netz http:// verwenden.",
                        "Import the certificate or its root into Windows under “Trusted Root Certification Authorities” (current user), or use http:// in the local network.",
                    )
                } else {
                    lang.t(
                        "Stammzertifikat in den Zertifikatsspeicher des Systems aufnehmen (z. B. `update-ca-certificates`, `trust anchor`) oder im lokalen Netz http:// verwenden.",
                        "Add the root certificate to the system certificate store (e.g. `update-ca-certificates`, `trust anchor`), or use http:// in the local network.",
                    )
                };
                let f = if *flags != 0 { format!(", TLS 0x{flags:x}") } else { String::new() };
                if cfg!(windows) { format!("{why} {hint} (WinHTTP {c}{f})") } else { format!("{why} {hint}") }
            }
            LlmError::Cancelled => lang.t("Abgebrochen.", "Cancelled.").into(),
            LlmError::Sys { code: c, call } => {
                let msg = match *c {
                    code::TIMEOUT => lang.t("Zeitüberschreitung – der Server hat nicht rechtzeitig geantwortet.", "Timed out – the server did not answer in time."),
                    code::NAME_NOT_RESOLVED => lang.t("Der Servername konnte nicht aufgelöst werden.", "The server name could not be resolved."),
                    code::CANNOT_CONNECT => lang.t(
                        "Keine Verbindung zum Server – läuft er, und stimmen Adresse und Port? (Ollama: Port 11434)",
                        "Could not connect to the server – is it running, and are the address and port right? (Ollama: port 11434)",
                    ),
                    code::CONNECTION_ERROR => lang.t("Die Verbindung wurde unterbrochen.", "The connection was interrupted."),
                    code::SECURE_FAILURE => lang.t("Sichere Verbindung fehlgeschlagen (TLS).", "Secure connection failed (TLS)."),
                    _ => lang.t("Netzwerkfehler.", "Network error."),
                };
                if cfg!(windows) { format!("{msg} (WinHTTP {c}, {call})") } else { msg.to_string() }
            }
            LlmError::Server(m) => format!("{}: {}", lang.t("Der Server meldet einen Fehler", "The server reported an error"), server_message(m)),
            LlmError::BadResponse(d) => format!("{} ({d})", lang.t("Unerwartete Antwort des Servers – stimmt der Server-Typ?", "Unexpected answer from the server – is the server type right?")),
            LlmError::TooLarge => lang.t("Die Antwort des Servers ist größer als erlaubt.", "The server's answer is larger than allowed.").into(),
            LlmError::Unsupported => lang.t("Nur unter Windows verfügbar.", "Only available on Windows.").into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_in_both_languages() {
        let all = [
            LlmError::BadUrl(UrlProblem::NumericHost),
            LlmError::Blocked { host: "8.8.8.8".into(), addr: Some("8.8.8.8".parse().unwrap()) },
            LlmError::Resolve { host: "x.lan".into() },
            LlmError::BadKey,
            LlmError::Status { code: 500, body: "boom".into() },
            LlmError::Cert { code: 12175, flags: 8 },
            LlmError::Cancelled,
            LlmError::Sys { code: 12029, call: "WinHttpSendRequest" },
            LlmError::Server("model not found".into()),
            LlmError::BadResponse("no valid data".into()),
            LlmError::TooLarge,
            LlmError::Unsupported,
        ];
        for e in all {
            let (en, de) = (e.text(Lang::En), e.text(Lang::De));
            assert!(!en.is_empty() && !de.is_empty() && en != de, "{e:?}");
        }
    }

    #[test]
    fn status_texts_show_server_message() {
        let e = LlmError::Status { code: 404, body: r#"{"error":"model \"llama9\" not found, try pulling it first"}"#.into() };
        assert_eq!(e.text(Lang::En), "Not found – check the address, the server type and the model name (HTTP 404): model \"llama9\" not found, try pulling it first");
        let e = LlmError::Status { code: 401, body: r#"{"error":{"message":"Invalid API key","type":"auth"}}"#.into() };
        assert!(e.text(Lang::De).starts_with("Zugriff verweigert"));
        assert!(e.text(Lang::En).ends_with(": Invalid API key"));
        let e = LlmError::Status { code: 502, body: "".into() };
        assert_eq!(e.text(Lang::En), "The server reported an error (HTTP 502)");
        let long = "x".repeat(1000);
        assert!(LlmError::Status { code: 500, body: long }.text(Lang::En).chars().count() < 400);
        assert_eq!(server_message("line1\nline2"), "line1 line2");
    }

    #[test]
    fn certificate_errors_get_a_hint() {
        assert_eq!(classify(12175, 8, "x"), LlmError::Cert { code: 12175, flags: 8 });
        assert_eq!(classify(12045, 0, "x"), LlmError::Cert { code: 12045, flags: 0 });
        assert_eq!(classify(12175, 0x80000000, "x"), LlmError::Sys { code: 12175, call: "x" });
        assert_eq!(classify(12017, 0, "x"), LlmError::Cancelled);
        let t = classify(12175, 8, "x").text(Lang::En);
        assert!(t.contains("self-signed") && (t.contains("Trusted Root") || !cfg!(windows)), "{t}");
        assert!(classify(12038, 0, "x").text(Lang::De).contains("passt nicht zum Servernamen"));
        assert!(classify(12175, 0x20, "x").text(Lang::En).contains("expired"));
        assert!(LlmError::Sys { code: 12029, call: "WinHttpSendRequest" }.text(Lang::En).contains("11434"));
    }
}
