//! Encryption of small secrets (API keys) for the current user. Windows: DPAPI (`CryptProtectData`, bound to the user
//! account), no description, no extra entropy, no dialog.
//!
//! Linux/macOS (#149/#150, `unix` below): the API key is encrypted with ChaCha20-Poly1305 (ring); only the encrypted
//! blob is stored (as before: Base64 in `settings.json`, never plain text). The 32-byte key comes from
//! * the system keyring (Linux: Secret Service – GNOME Keyring, KWallet, KeePassXC …; one item "Paneldeck – API key
//!   encryption", created on the first stored key), blob kind `K`, or
//! * if there is no keyring: a **master password** (PBKDF2-HMAC-SHA256, 600 000 rounds, random salt per blob), blob
//!   kind `P`. The password is asked in the window on the FIRST ACCESS to a key (never at start, #149 decision) and kept
//!   in memory only until Paneldeck quits (only the derived keys, not the password; [`password_wanted`], [`begin_unlock`]).
//!   PBKDF2 runs in a background thread. A keyring that is there but refuses reports `Keyring(..)` – the master
//!   password is only used when there is no keyring service at all.
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
    /// Linux: the keyring is being opened in the background (never in the UI thread) – try again in a moment.
    #[cfg_attr(windows, allow(dead_code))]
    Pending,
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
pub use unix::{begin_unlock, cancel_password, is_password_blob, password_state, password_wanted, protect, unprotect, PwState};

#[cfg(unix)]
mod unix {
    use super::Error;
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
    use std::num::NonZeroU32;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use zeroize::Zeroizing;

    /// Blob kinds (first byte; also the additional authenticated data).
    pub const KIND_KEYRING: u8 = b'K';
    pub const KIND_PASSWORD: u8 = b'P';
    const SALT_LEN: usize = 16;
    const ROUNDS: u32 = 600_000;

    type Key = Zeroizing<[u8; 32]>;

    /// Keys derived from the master password, one per salt (the password itself is not kept). New blobs use the
    /// first salt.
    static DERIVED: Mutex<Vec<([u8; SALT_LEN], Key)>> = Mutex::new(Vec::new());
    static WANTED: AtomicBool = AtomicBool::new(false);
    /// Key from the keyring, fetched once per run.
    static KEYRING_KEY: Mutex<Option<Key>> = Mutex::new(None);

    /// State of the background check started by [`begin_unlock`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PwState {
        Idle,
        Working,
        /// The password does not open the stored keys.
        Wrong,
        /// Accepted: the keys are ready.
        Done,
    }

    static STATE: Mutex<PwState> = Mutex::new(PwState::Idle);
    /// Generation of the question: a check that ends after "cancel" (or after a newer check) is dropped.
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Does an access wait for the master password (the window shows the question)?
    pub fn password_wanted() -> bool {
        WANTED.load(Ordering::Relaxed)
    }

    /// The question was cancelled: it closes; the next access asks again.
    pub fn cancel_password() {
        GENERATION.fetch_add(1, Ordering::SeqCst);
        WANTED.store(false, Ordering::Relaxed);
        *lock(&STATE) = PwState::Idle;
    }

    /// Current state of the check; `Done`/`Wrong` are reported once (then `Idle`).
    pub fn password_state() -> PwState {
        let mut g = lock(&STATE);
        let s = *g;
        if matches!(s, PwState::Done | PwState::Wrong) {
            *g = PwState::Idle;
        }
        s
    }

    /// Check the entered password in a background thread (PBKDF2 takes a moment – never in the UI thread and never under
    /// a lock): derive the key for the salt of every stored blob (`blobs`, kind `P`), check that it opens the first one,
    /// then keep the keys. Without stored blobs a new salt is made (a new password).
    pub fn begin_unlock(password: Zeroizing<String>, blobs: Vec<Vec<u8>>) {
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&STATE) = PwState::Working;
        let current = move || GENERATION.load(Ordering::SeqCst) == generation;
        std::thread::spawn(move || {
            let mut salts: Vec<[u8; SALT_LEN]> = Vec::new();
            for b in blobs.iter().filter(|b| is_password_blob(b) && b.len() > 1 + SALT_LEN) {
                if let Ok(s) = <[u8; SALT_LEN]>::try_from(&b[1..1 + SALT_LEN]) {
                    if !salts.contains(&s) {
                        salts.push(s);
                    }
                }
            }
            if salts.is_empty() {
                match random::<SALT_LEN>() {
                    Ok(s) => salts.push(s),
                    Err(_) => {
                        *lock(&STATE) = PwState::Wrong;
                        return;
                    }
                }
            }
            let derived: Vec<([u8; SALT_LEN], Key)> = salts.into_iter().map(|s| (s, derive(&password, &s))).collect();
            if let Some(first) = blobs.iter().find(|b| is_password_blob(b) && b.len() > 1 + SALT_LEN) {
                let key = derived.iter().find(|(s, _)| *s == first[1..1 + SALT_LEN]).map(|(_, k)| k);
                if key.is_none_or(|k| open(KIND_PASSWORD, k, &first[1 + SALT_LEN..]).is_none()) {
                    if current() {
                        *lock(&STATE) = PwState::Wrong;
                    }
                    return;
                }
            }
            // cancelled (or superseded) meanwhile: keep nothing
            let mut d = lock(&DERIVED);
            if !current() {
                return;
            }
            *d = derived;
            drop(d);
            // (WANTED clears the window when it has read `Done`, see `cancel_password`.)
            *lock(&STATE) = PwState::Done;
        });
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

    fn derive(password: &str, salt: &[u8; SALT_LEN]) -> Key {
        let mut out = Zeroizing::new([0u8; 32]);
        ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ROUNDS).expect("rounds"), salt, password.as_bytes(), &mut *out);
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
        let mut buf = Zeroizing::new(data.to_vec());
        let plain = k.open_in_place(Nonce::try_assume_unique_for_key(nonce).ok()?, Aad::from([kind]), &mut buf).ok()?;
        Some(plain.to_vec())
    }

    /// Key for `salt` (or the first one for a new blob). `NeedPassword` (and the question opens) without one – it is
    /// never derived here (this runs in the UI thread).
    fn password_key(salt: Option<[u8; SALT_LEN]>) -> Result<([u8; SALT_LEN], Key), Error> {
        let g = lock(&DERIVED);
        let hit = match salt {
            Some(s) => g.iter().find(|(x, _)| *x == s),
            None => g.first(),
        };
        match hit {
            Some((s, k)) => Ok((*s, k.clone())),
            None => {
                WANTED.store(true, Ordering::Relaxed);
                Err(Error::NeedPassword)
            }
        }
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, Error> {
        match keyring::key(true) {
            Ok(k) => seal(KIND_KEYRING, &k, &[], plain),
            // only when there is no keyring service at all: master password
            Err(keyring::Fail::Unavailable) => {
                let (salt, key) = password_key(None)?;
                seal(KIND_PASSWORD, &key, &salt, plain)
            }
            // a keyring that refuses (prompt dismissed, D-Bus error): no silent switch to the password
            Err(keyring::Fail::Refused(e)) => Err(Error::Keyring(e)),
            Err(keyring::Fail::Pending) => Err(Error::Pending),
        }
    }

    pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>, Error> {
        match blob.first() {
            Some(&KIND_KEYRING) => {
                let key = keyring::key(false).map_err(|e| match e {
                    keyring::Fail::Unavailable => Error::Keyring(crate::state::t("Kein Schlüsselbund (Secret Service) erreichbar", "No keyring (Secret Service) reachable").into()),
                    keyring::Fail::Refused(e) => Error::Keyring(e),
                    keyring::Fail::Pending => Error::Pending,
                })?;
                open(KIND_KEYRING, &key, &blob[1..]).ok_or(Error::Keyring(crate::state::t("Der Schlüssel im Schlüsselbund passt nicht", "The key in the keyring does not match").into()))
            }
            Some(&KIND_PASSWORD) if blob.len() > 1 + SALT_LEN => {
                let salt: [u8; SALT_LEN] = blob[1..1 + SALT_LEN].try_into().map_err(|_| Error::TooLarge)?;
                let (_, key) = password_key(Some(salt))?;
                open(KIND_PASSWORD, &key, &blob[1 + SALT_LEN..]).ok_or(Error::WrongPassword)
            }
            _ => Err(Error::TooLarge),
        }
    }

    #[cfg(test)]
    pub fn test_reset() {
        lock(&DERIVED).clear();
        WANTED.store(false, Ordering::Relaxed);
        *lock(&STATE) = PwState::Idle;
        GENERATION.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn test_seal_password(password: &str, plain: &[u8]) -> Vec<u8> {
        let salt: [u8; SALT_LEN] = random().unwrap();
        seal(KIND_PASSWORD, &derive(password, &salt), &salt, plain).unwrap()
    }

    /// The system keyring (Linux: Secret Service over D-Bus). The D-Bus calls (and the keyring's own unlock dialog) run in
    /// a background thread, never in the UI thread: the first access starts it and reports `Pending`.
    mod keyring {
        use super::{lock, Key, KEYRING_KEY};

        pub enum Fail {
            /// No keyring service (no session bus, no Secret Service provider, macOS for now): the master password is used.
            Unavailable,
            /// The service is there but refused (locked and the prompt was dismissed, D-Bus error): text of the error.
            #[allow(dead_code)]
            Refused(String),
            /// Still being opened in the background.
            #[allow(dead_code)]
            Pending,
        }

        #[cfg(any(target_os = "macos", test))]
        pub fn key(_create: bool) -> Result<Key, Fail> {
            // Tests never touch the real keyring of the machine.
            let _ = &KEYRING_KEY;
            drop(lock(&KEYRING_KEY));
            Err(Fail::Unavailable)
        }

        #[cfg(all(not(target_os = "macos"), not(test)))]
        enum State {
            Idle,
            Fetching,
            Unavailable,
            Refused(String),
        }

        #[cfg(all(not(target_os = "macos"), not(test)))]
        static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State::Idle);

        /// The 32-byte key, if already fetched; otherwise the fetch starts in the background (`Pending`). A refusal is
        /// reported once, the next access tries again; "no service" stays for this run.
        #[cfg(all(not(target_os = "macos"), not(test)))]
        pub fn key(create: bool) -> Result<Key, Fail> {
            if let Some(k) = lock(&KEYRING_KEY).as_ref() {
                return Ok(k.clone());
            }
            let mut st = lock(&STATE);
            match std::mem::replace(&mut *st, State::Idle) {
                State::Unavailable => {
                    *st = State::Unavailable;
                    Err(Fail::Unavailable)
                }
                State::Refused(e) => Err(Fail::Refused(e)),
                State::Fetching => {
                    *st = State::Fetching;
                    Err(Fail::Pending)
                }
                State::Idle => {
                    *st = State::Fetching;
                    std::thread::spawn(move || {
                        let r = fetch(create);
                        let mut st = lock(&STATE);
                        *st = match r {
                            Ok(k) => {
                                *lock(&KEYRING_KEY) = Some(k);
                                State::Idle
                            }
                            Err(Fail::Unavailable) => State::Unavailable,
                            Err(Fail::Refused(e)) => State::Refused(e),
                            Err(Fail::Pending) => State::Idle,
                        };
                    });
                    Err(Fail::Pending)
                }
            }
        }

        /// Item "Paneldeck – API key encryption" (attributes `application=paneldeck`, `purpose=api-key-encryption`);
        /// `create`: make it if missing (only when storing a key). Unlocking may show the keyring's own dialog.
        #[cfg(all(not(target_os = "macos"), not(test)))]
        fn fetch(create: bool) -> Result<Key, Fail> {
            use secret_service::blocking::SecretService;
            use secret_service::{EncryptionType, Error as E};
            use std::collections::HashMap;
            let refused = |e: E| Fail::Refused(e.to_string());
            let ss = match SecretService::connect(EncryptionType::Dh) {
                Ok(s) => s,
                // no session bus or nobody provides org.freedesktop.secrets
                Err(E::Unavailable | E::Zbus(_) | E::ZbusFdo(_)) => return Err(Fail::Unavailable),
                Err(e) => return Err(refused(e)),
            };
            let attrs = HashMap::from([("application", "paneldeck"), ("purpose", "api-key-encryption")]);
            let found = ss.search_items(attrs.clone()).map_err(refused)?;
            let item = found.unlocked.into_iter().next().or_else(|| found.locked.into_iter().next());
            match item {
                Some(item) => {
                    item.ensure_unlocked().map_err(refused)?;
                    let v = zeroize::Zeroizing::new(item.get_secret().map_err(refused)?);
                    Ok(Key::new(<[u8; 32]>::try_from(v.as_slice()).map_err(|_| Fail::Refused("invalid key length".into()))?))
                }
                None if create => {
                    let mut k = Key::new([0u8; 32]);
                    getrandom::fill(&mut *k).map_err(|e| Fail::Refused(e.to_string()))?;
                    let coll = ss.get_default_collection().map_err(refused)?;
                    coll.ensure_unlocked().map_err(refused)?;
                    coll.create_item("Paneldeck – API key encryption", attrs, &*k, true, "application/octet-stream").map_err(refused)?;
                    Ok(k)
                }
                None => Err(Fail::Refused(crate::state::t("Schlüssel im Schlüsselbund fehlt", "Key missing in the keyring").into())),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn wait_state() -> PwState {
            for _ in 0..600 {
                let s = password_state();
                if s != PwState::Working && s != PwState::Idle {
                    return s;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("password check did not finish");
        }

        #[test]
        fn master_password_flow() {
            test_reset();
            // no keyring in tests → the first access asks for the password, never anything earlier
            assert!(!password_wanted());
            assert_eq!(protect(b"sk-1"), Err(Error::NeedPassword));
            assert!(password_wanted());
            // new password (no stored blobs): derived in the background
            begin_unlock(Zeroizing::new("correct horse".into()), Vec::new());
            assert_eq!(wait_state(), PwState::Done);
            cancel_password();
            assert!(!password_wanted());
            let blob = protect(b"sk-1").unwrap();
            assert!(is_password_blob(&blob));
            assert!(!blob.windows(4).any(|w| w == b"sk-1"), "never plain text");
            assert_ne!(blob, protect(b"sk-1").unwrap(), "fresh nonce per blob");
            assert_eq!(unprotect(&blob).unwrap(), b"sk-1");
            // after a restart: wrong password is refused, the right one opens every stored blob
            test_reset();
            assert_eq!(unprotect(&blob), Err(Error::NeedPassword));
            let other = test_seal_password("correct horse", b"sk-2");
            begin_unlock(Zeroizing::new("wrong".into()), vec![blob.clone(), other.clone()]);
            assert_eq!(wait_state(), PwState::Wrong);
            assert_eq!(unprotect(&blob), Err(Error::NeedPassword));
            begin_unlock(Zeroizing::new("correct horse".into()), vec![blob.clone(), other.clone()]);
            assert_eq!(wait_state(), PwState::Done);
            assert_eq!(unprotect(&blob).unwrap(), b"sk-1");
            assert_eq!(unprotect(&other).unwrap(), b"sk-2", "every salt was derived");
            // tampering is detected
            let mut bad = other.clone();
            let last = bad.len() - 1;
            bad[last] ^= 1;
            assert_eq!(unprotect(&bad), Err(Error::WrongPassword));
            // keyring blob without keyring
            assert!(matches!(unprotect(b"Kxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"), Err(Error::Keyring(_))));
            assert_eq!(unprotect(b"?"), Err(Error::TooLarge));
            // a check that ends after "cancel" keeps nothing
            test_reset();
            begin_unlock(Zeroizing::new("late".into()), Vec::new());
            cancel_password();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            assert_eq!(protect(b"x"), Err(Error::NeedPassword), "cancelled check must not set the keys");
            test_reset();
        }
    }
}
