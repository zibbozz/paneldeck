//! API-Schlüssel verschlüsselt ablegen: Windows-DPAPI (`CryptProtectData`, an den aktuellen Benutzer gebunden, über
//! `platform::secret`), gespeichert als Base64 des Blobs. Klartext landet nie in einer Datei.

use crate::state::Lang;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// Kein gültiges Base64.
    Encoding,
    /// Entschlüsselt, aber kein UTF-8.
    Utf8,
    /// DPAPI-Fehler (z. B. Blob eines anderen Benutzers/Rechners).
    Sys(u32),
    /// Nicht unter Windows (`platform::secret`).
    Unsupported,
}

impl SecretError {
    pub fn text(&self, lang: Lang) -> String {
        match self {
            SecretError::Encoding | SecretError::Utf8 => lang.t("Der gespeicherte API-Schlüssel ist beschädigt – bitte neu eingeben.", "The stored API key is damaged – please enter it again.").into(),
            SecretError::Sys(c) => format!(
                "{} (DPAPI {c})",
                lang.t(
                    "Der gespeicherte API-Schlüssel lässt sich nicht entschlüsseln (anderer Benutzer oder Rechner?) – bitte neu eingeben.",
                    "The stored API key cannot be decrypted (different user or computer?) – please enter it again.",
                )
            ),
            SecretError::Unsupported => lang.t("Nur unter Windows verfügbar.", "Only available on Windows.").into(),
        }
    }
}

/// Verschlüsselt `plain` für den aktuellen Benutzer; Ergebnis = Base64.
pub fn protect(plain: &str) -> Result<String, SecretError> {
    crate::platform::secret::protect(plain.as_bytes()).map_err(from_platform).map(|b| crate::mcp::base64(&b))
}

/// Entschlüsselt einen Wert von `protect`.
pub fn unprotect(b64: &str) -> Result<String, SecretError> {
    let blob = unbase64(b64.trim()).ok_or(SecretError::Encoding)?;
    String::from_utf8(crate::platform::secret::unprotect(&blob).map_err(from_platform)?).map_err(|_| SecretError::Utf8)
}

fn from_platform(e: crate::platform::secret::Error) -> SecretError {
    use crate::platform::secret::Error;
    match e {
        Error::TooLarge => SecretError::Encoding,
        Error::Sys(c) => SecretError::Sys(c),
        Error::Unsupported => SecretError::Unsupported,
    }
}

/// Base64 (Standard-Alphabet, Auffüllung optional) → Bytes; `None` bei ungültigen Zeichen.
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let s = s.trim_end_matches('=').as_bytes();
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for c in s.chunks(4) {
        let mut n = 0u32;
        for (i, &b) in c.iter().enumerate() {
            n |= val(b)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if c.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            assert_eq!(unbase64(&crate::mcp::base64(s.as_bytes())).unwrap(), s.as_bytes(), "{s}");
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(unbase64(&crate::mcp::base64(&all)).unwrap(), all);
        assert_eq!(unbase64("Zm9v YmFy"), None);
        assert_eq!(unbase64("Z"), None);
        assert_eq!(unbase64("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip() {
        let key = "sk-test-äöü-1234567890";
        let enc = protect(key).unwrap();
        assert!(!enc.contains("sk-test"));
        assert_ne!(enc, protect(key).unwrap(), "DPAPI salts every blob");
        assert_eq!(unprotect(&enc).unwrap(), key);
        assert_eq!(unprotect(&protect("").unwrap()).unwrap(), "");
        assert_eq!(unprotect("!!!"), Err(SecretError::Encoding));
        assert!(matches!(unprotect("AAAA"), Err(SecretError::Sys(_))));
        assert!(SecretError::Sys(13).text(Lang::De).contains("entschlüsseln"));
    }
}
