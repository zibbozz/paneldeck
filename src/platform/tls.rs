//! TLS outside Windows (#149): rustls with the `ring` provider, TLS 1.2/1.3, root certificates from the system trust
//! store (`rustls-native-certs`: the distribution's CA bundle, `SSL_CERT_FILE`/`SSL_CERT_DIR` if set), so company
//! CAs and inspecting proxies work like with WinHTTP on Windows (#149 decision). Certificates are always checked.

use std::sync::{Arc, OnceLock};

/// Client configuration (built once; reading the trust store takes a moment). Error text in the current language.
pub fn client_config() -> Result<Arc<rustls::ClientConfig>, String> {
    static CFG: OnceLock<Result<Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CFG.get_or_init(build).clone()
}

fn build() -> Result<Arc<rustls::ClientConfig>, String> {
    let found = rustls_native_certs::load_native_certs();
    let mut roots = rustls::RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(found.certs);
    if added == 0 {
        return Err(no_roots_text());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

fn no_roots_text() -> String {
    crate::state::t(
        "Keine Stammzertifikate des Systems gefunden (z. B. Paket „ca-certificates“ fehlt) – HTTPS ist nicht möglich.",
        "No system root certificates found (e.g. the “ca-certificates” package is missing) – HTTPS is not possible.",
    )
    .to_string()
}

/// Is this I/O error (from ureq/rustls) a certificate problem? rustls wraps its errors in `io::Error`.
pub fn is_cert_error(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(r) = err.downcast_ref::<rustls::Error>() {
            return matches!(r, rustls::Error::InvalidCertificate(_));
        }
        cur = err.source();
    }
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_roots_are_found_on_ci() {
        // The CI runners and every desktop distribution ship a CA bundle.
        assert!(super::client_config().is_ok(), "{:?}", super::client_config().err());
    }
}
