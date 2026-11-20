//! Encryption of small secrets (API keys) for the current user. Windows: DPAPI (`CryptProtectData`, bound to the user
//! account), no description, no extra entropy, no dialog.
//!
//! Linux/macOS (#149/#150, `unix` below): the API key is encrypted with ChaCha20-Poly1305 (ring); only the encrypted
//! blob is stored (as before: Base64 in `settings.json`, never plain text). The 32-byte key comes from
//! * the system keyring (Linux: Secret Service – GNOME Keyring, KWallet, KeePassXC …; one item "Paneldeck – API key
//!   encryption", created on the first stored key), blob kind `K`, or
//! * if there is no keyring: a **master password** (PBKDF2-HMAC-SHA256, 600 000 rounds, random salt per blob), blob
//!   kind `P`. The password is asked in the window on the FIRST ACCESS to a key (never at start, #149 decision) and kept
//!   in memory only until Paneldeck quits ([`password_wanted`], [`set_password`]).
//! macOS: only the master password for now (Keychain follows in #150).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Input longer than the system call accepts.
    #[cfg_attr(not(windows), allow(dead_code))]
    TooLarge,
    /// Error code of the system (e.g. a blob of another user or computer).
    #[cfg_attr(not(windows), allow(dead_code))]
    Sys(u32),
    /// No implementation on this platform.
    #[allow(dead_code)]
    Unsupported,
    /// Linux/macOS: no keyring, and the master password has not been entered yet (the window asks for it now).
    #[cfg_attr(windows, allow(dead_code))]
    NeedPassword,
    /// Linux/macOS: the blob does not open with the entered master password (or is damaged).
    #[cfg_attr(windows, allow(dead_code))]
    WrongPassword,
    /// Linux: the keyring refused (locked and the unlock prompt was dismissed, item missing …): text of the error.
    #[cfg_attr(windows, allow(dead_code))]
    Keyring(String),
}

#[cfg(windows)]
pub use imp::{protect, unprotect};

#[cfg(windows)]
mod imp {
    use super::Error;
    use windows_sys::Win32::Foundation::{GetLastError, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};

    fn run(input: &[u8], protect: bool) -> Result<Vec<u8>, Error> {
        let len = u32::try_from(input.len()).map_err(|_| Error::TooLarge)?;
        let inb = CRYPT_INTEGER_BLOB { cbData: len, pbData: input.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: Eingabe-Blob zeigt auf `input` (nur gelesen); keine Beschreibung, keine Zusatz-Entropie, kein Dialog.
        // Ausgabe wird von DPAPI mit LocalAlloc angelegt und unten mit LocalFree freigegeben.
        let ok = unsafe {
            if protect {
                CryptProtectData(&inb, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&inb, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
        };
        if ok == 0 {
            // SAFETY: liest nur den Fehlerwert des aktuellen Threads.
            return Err(Error::Sys(unsafe { GetLastError() }));
        }
        // SAFETY: `out` beschreibt `cbData` gültige Bytes, die DPAPI angelegt hat.
        let v = if out.pbData.is_null() { Vec::new() } else { unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec() };
        if !out.pbData.is_null() {
            if !protect {
                // Klartext nicht im freigegebenen Speicher liegen lassen.
                for i in 0..out.cbData as usize {
                    // SAFETY: innerhalb der `cbData` Bytes von DPAPI; volatile, damit es nicht wegoptimiert wird.
                    unsafe { std::ptr::write_volatile(out.pbData.add(i), 0) };
                }
            }
            // SAFETY: Speicher stammt von DPAPI (LocalAlloc) und wird genau einmal freigegeben.
            unsafe { LocalFree(out.pbData as HLOCAL) };
        }
        Ok(v)
    }

    pub fn protect(b: &[u8]) -> Result<Vec<u8>, Error> {
        run(b, true)
    }

    pub fn unprotect(b: &[u8]) -> Result<Vec<u8>, Error> {
        run(b, false)
    }
}

#[cfg(unix)]
pub use unix::{is_password_blob, password_opens, password_wanted, protect, set_password, unprotect};

#[cfg(unix)]
mod unix {
    use super::Error;
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
    use std::num::NonZeroU32;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// Blob kinds (first byte; also the additional authenticated data).
    pub const KIND_KEYRING: u8 = b'K';
    pub const KIND_PASSWORD: u8 = b'P';
    const SALT_LEN: usize = 16;
    const ROUNDS: u32 = 600_000;

    /// Master password (in memory only) and the key derived last (salt, key) – reused for new blobs of this run.
    struct Pw {
        password: String,
        derived: Vec<([u8; SALT_LEN], [u8; 32])>,
    }

    static PASSWORD: Mutex<Option<Pw>> = Mutex::new(None);
    static WANTED: AtomicBool = AtomicBool::new(false);
    /// Key from the keyring, fetched once per run.
    static KEYRING_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

    /// Does an access wait for the master password (the window shows the question)?
    pub fn password_wanted() -> bool {
        WANTED.load(Ordering::Relaxed)
    }

    /// Master password entered in the window (`None` = cancelled: the question closes, the next access asks again).
    pub fn set_password(pw: Option<String>) {
        WANTED.store(false, Ordering::Relaxed);
        if let Some(p) = pw {
            *PASSWORD.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pw { password: p, derived: Vec::new() });
        }
    }

    /// Is this blob protected by the master password (kind `P`)?
    pub fn is_password_blob(b: &[u8]) -> bool {
        b.first() == Some(&KIND_PASSWORD)
    }

    fn random<const N: usize>() -> Result<[u8; N], Error> {
        let mut b = [0u8; N];
        getrandom::fill(&mut b).map_err(|_| Error::Sys(0))?;
        Ok(b)
    }

    fn derive(password: &str, salt: &[u8; SALT_LEN]) -> [u8; 32] {
        let mut out = [0u8; 32];
        ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ROUNDS).expect("rounds"), salt, password.as_bytes(), &mut out);
        out
    }

    fn seal(kind: u8, key: &[u8; 32], prefix: &[u8], plain: &[u8]) -> Result<Vec<u8>, Error> {
        let k = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| Error::Sys(0))?);
        let nonce: [u8; NONCE_LEN] = random()?;
        let mut data = plain.to_vec();
        k.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from([kind]), &mut data).map_err(|_| Error::Sys(0))?;
        let mut out = vec![kind];
        out.extend_from_slice(prefix);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&data);
        Ok(out)
    }

    fn open(kind: u8, key: &[u8; 32], nonce_and_data: &[u8]) -> Option<Vec<u8>> {
        if nonce_and_data.len() < NONCE_LEN + CHACHA20_POLY1305.tag_len() {
            return None;
        }
        let (nonce, data) = nonce_and_data.split_at(NONCE_LEN);
        let k = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).ok()?);
        let mut buf = data.to_vec();
        let plain = k.open_in_place(Nonce::try_assume_unique_for_key(nonce).ok()?, Aad::from([kind]), &mut buf).ok()?;
        Some(plain.to_vec())
    }

    /// Key derived from the master password for `salt` (cached). `NeedPassword` (and the question opens) without one.
    fn password_key(salt: Option<[u8; SALT_LEN]>) -> Result<([u8; SALT_LEN], [u8; 32]), Error> {
        let mut g = PASSWORD.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pw) = g.as_mut() else {
            WANTED.store(true, Ordering::Relaxed);
            return Err(Error::NeedPassword);
        };
        let salt = match salt {
            Some(s) => s,
            None => match pw.derived.first() {
                Some((s, _)) => *s,
                None => random()?,
            },
        };
        if let Some((_, k)) = pw.derived.iter().find(|(s, _)| *s == salt) {
            return Ok((salt, *k));
        }
        let k = derive(&pw.password, &salt);
        pw.derived.push((salt, k));
        Ok((salt, k))
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, Error> {
        match keyring::key(true) {
            Ok(k) => seal(KIND_KEYRING, &k, &[], plain),
            Err(keyring::Unavailable) => {
                let (salt, key) = password_key(None)?;
                seal(KIND_PASSWORD, &key, &salt, plain)
            }
        }
    }

    pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>, Error> {
        match blob.first() {
            Some(&KIND_KEYRING) => {
                let key = keyring::key(false).map_err(|_| Error::Keyring(crate::state::t("Kein Schlüsselbund (Secret Service) erreichbar", "No keyring (Secret Service) reachable").into()))?;
                open(KIND_KEYRING, &key, &blob[1..]).ok_or(Error::Keyring(crate::state::t("Der Schlüssel im Schlüsselbund passt nicht", "The key in the keyring does not match").into()))
            }
            Some(&KIND_PASSWORD) if blob.len() > 1 + SALT_LEN => {
                let salt: [u8; SALT_LEN] = blob[1..1 + SALT_LEN].try_into().map_err(|_| Error::TooLarge)?;
                let (_, key) = password_key(Some(salt))?;
                match open(KIND_PASSWORD, &key, &blob[1 + SALT_LEN..]) {
                    Some(p) => Ok(p),
                    None => {
                        // wrong password: forget it, the next access asks again
                        *PASSWORD.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        Err(Error::WrongPassword)
                    }
                }
            }
            _ => Err(Error::TooLarge),
        }
    }

    /// Does `password` open `blob` (a `P` blob)? For the question in the window: check before accepting.
    pub fn password_opens(password: &str, blob: &[u8]) -> bool {
        if !is_password_blob(blob) || blob.len() <= 1 + SALT_LEN {
            return false;
        }
        let Ok(salt) = <[u8; SALT_LEN]>::try_from(&blob[1..1 + SALT_LEN]) else { return false };
        open(KIND_PASSWORD, &derive(password, &salt), &blob[1 + SALT_LEN..]).is_some()
    }

    #[cfg(test)]
    pub fn test_reset() {
        *PASSWORD.lock().unwrap_or_else(|e| e.into_inner()) = None;
        WANTED.store(false, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn test_seal_password(password: &str, plain: &[u8]) -> Vec<u8> {
        let salt: [u8; SALT_LEN] = random().unwrap();
        seal(KIND_PASSWORD, &derive(password, &salt), &salt, plain).unwrap()
    }

    /// The system keyring (Linux: Secret Service over D-Bus).
    mod keyring {
        use super::KEYRING_KEY;

        /// No keyring service (no session bus, no Secret Service provider, macOS for now).
        pub struct Unavailable;

        #[cfg(any(target_os = "macos", test))]
        pub fn key(_create: bool) -> Result<[u8; 32], Unavailable> {
            // Tests never touch the real keyring of the machine.
            let _ = &KEYRING_KEY;
            Err(Unavailable)
        }

        /// The 32-byte key of the item "Paneldeck – API key encryption" (attributes `application=paneldeck`,
        /// `purpose=api-key-encryption`). `create`: make it if missing (only when storing a key). Unlocking the
        /// collection may show the keyring's own unlock dialog.
        #[cfg(all(not(target_os = "macos"), not(test)))]
        pub fn key(create: bool) -> Result<[u8; 32], Unavailable> {
            use secret_service::blocking::SecretService;
            use secret_service::EncryptionType;
            use std::collections::HashMap;
            if let Some(k) = *KEYRING_KEY.lock().unwrap_or_else(|e| e.into_inner()) {
                return Ok(k);
            }
            let ss = SecretService::connect(EncryptionType::Dh).map_err(|_| Unavailable)?;
            let attrs = HashMap::from([("application", "paneldeck"), ("purpose", "api-key-encryption")]);
            let found = ss.search_items(attrs.clone()).map_err(|_| Unavailable)?;
            let item = found.unlocked.into_iter().next().or_else(|| found.locked.into_iter().next());
            let key = match item {
                Some(item) => {
                    item.ensure_unlocked().map_err(|_| Unavailable)?;
                    let v = item.get_secret().map_err(|_| Unavailable)?;
                    <[u8; 32]>::try_from(v.as_slice()).map_err(|_| Unavailable)?
                }
                None if create => {
                    let mut k = [0u8; 32];
                    getrandom::fill(&mut k).map_err(|_| Unavailable)?;
                    let coll = ss.get_default_collection().map_err(|_| Unavailable)?;
                    coll.ensure_unlocked().map_err(|_| Unavailable)?;
                    coll.create_item("Paneldeck – API key encryption", attrs, &k, true, "application/octet-stream").map_err(|_| Unavailable)?;
                    k
                }
                None => return Err(Unavailable),
            };
            *KEYRING_KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(key);
            Ok(key)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn master_password_flow() {
            test_reset();
            // no keyring in tests → the first access asks for the password, never anything earlier
            assert!(!password_wanted());
            assert_eq!(protect(b"sk-1"), Err(Error::NeedPassword));
            assert!(password_wanted());
            set_password(Some("correct horse".into()));
            assert!(!password_wanted());
            let blob = protect(b"sk-1").unwrap();
            assert!(is_password_blob(&blob));
            assert!(!blob.windows(4).any(|w| w == b"sk-1"), "never plain text");
            assert_ne!(blob, protect(b"sk-1").unwrap(), "fresh nonce per blob");
            assert_eq!(unprotect(&blob).unwrap(), b"sk-1");
            assert!(password_opens("correct horse", &blob));
            assert!(!password_opens("wrong", &blob));
            // a blob of another password: wrong password, the cached one is dropped and asked again next time
            let other = test_seal_password("other", b"sk-2");
            assert_eq!(unprotect(&other), Err(Error::WrongPassword));
            assert_eq!(unprotect(&blob), Err(Error::NeedPassword));
            set_password(Some("other".into()));
            assert_eq!(unprotect(&other).unwrap(), b"sk-2");
            // tampering is detected
            let mut bad = other.clone();
            let last = bad.len() - 1;
            bad[last] ^= 1;
            assert_eq!(unprotect(&bad), Err(Error::WrongPassword));
            // keyring blob without keyring
            assert!(matches!(unprotect(b"Kxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"), Err(Error::Keyring(_))));
            assert_eq!(unprotect(b"?"), Err(Error::TooLarge));
            test_reset();
        }
    }
}
