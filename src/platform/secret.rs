//! Encryption of small secrets (API keys) for the current user. Windows: DPAPI (`CryptProtectData`, bound to the user
//! account), no description, no extra entropy, no dialog. Other platforms: "not supported" (the caller shows a text).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Input longer than the system call accepts.
    #[cfg_attr(not(windows), allow(dead_code))]
    TooLarge,
    /// Error code of the system (e.g. a blob of another user or computer).
    #[cfg_attr(not(windows), allow(dead_code))]
    Sys(u32),
    /// No implementation on this platform.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
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

#[cfg(not(windows))]
pub fn protect(_: &[u8]) -> Result<Vec<u8>, Error> {
    Err(Error::Unsupported)
}

#[cfg(not(windows))]
pub fn unprotect(_: &[u8]) -> Result<Vec<u8>, Error> {
    Err(Error::Unsupported)
}
