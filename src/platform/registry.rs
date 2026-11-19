//! Per-user settings store of the system, written without administrator rights: on Windows the registry under HKCU
//! (Unicode API). Used for autostart and the Explorer context menu (`crate::integration`) and for the install location
//! of the NSIS installer (`app::update::install`). Other platforms: nothing is stored, reads find nothing, writes report
//! "not supported" (autostart via XDG `.desktop`/LaunchAgent follows in #149/#150).

/// Ein Schreibschritt unter HKCU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegOp {
    /// Zeichenkette (`REG_SZ`) setzen; `name = None` ist der Standardwert. Der Schlüssel wird bei Bedarf angelegt.
    SetStr { key: String, name: Option<String>, value: String },
    /// Einen Wert löschen (fehlt er, ist das kein Fehler).
    DeleteValue { key: String, name: String },
    /// Einen Schlüssel samt Unterschlüsseln löschen (fehlt er, ist das kein Fehler).
    DeleteTree { key: String },
}

#[cfg(windows)]
pub use win::{apply, get_str, install_location, values};

#[cfg(windows)]
mod win {
    use super::RegOp;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE,
        REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    fn err(rc: u32) -> String {
        std::io::Error::from_raw_os_error(rc as i32).to_string()
    }

    /// Zeichenkette unter HKCU lesen (`value = None` = Standardwert); `REG_EXPAND_SZ` kommt erweitert zurück.
    /// `None`, wenn nicht vorhanden. Nur `RRF_RT_REG_SZ`: `RegGetValueW` erweitert `REG_EXPAND_SZ` selbst und meldet
    /// es dann als `REG_SZ`; `RRF_RT_REG_EXPAND_SZ` ohne `RRF_NOEXPAND` wäre `ERROR_INVALID_PARAMETER`.
    pub fn get_str(subkey: &str, value: Option<&str>) -> Option<String> {
        let sk = wide(subkey);
        let vn = value.map(wide);
        let vp = vn.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
        let flags = RRF_RT_REG_SZ;
        for _ in 0..3 {
            let mut size: u32 = 0;
            // SAFETY: gültige, nullterminierte UTF-16-Puffer; dieser Aufruf fragt nur die Größe ab.
            let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, sk.as_ptr(), vp, flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
            if rc != ERROR_SUCCESS {
                return None;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
            let mut cb = (buf.len() * 2) as u32;
            // SAFETY: `buf` ist `cb` Bytes groß.
            let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, sk.as_ptr(), vp, flags, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut cb) };
            if rc == ERROR_SUCCESS {
                return Some(from_wide(&buf));
            }
            // ERROR_MORE_DATA (Wert dazwischen gewachsen, Erweiterung länger): erneut versuchen
        }
        None
    }

    /// Alle Zeichenketten-Werte eines Schlüssels unter HKCU (Name, Daten; `REG_EXPAND_SZ` unerweitert).
    pub fn values(subkey: &str) -> Vec<(String, String)> {
        let sk = wide(subkey);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: gültiger Schlüsselname; `key` wird nur bei Erfolg benutzt und danach geschlossen.
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sk.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut name = vec![0u16; 16384];
        let mut data = vec![0u16; 32768];
        for i in 0..4096u32 {
            let mut name_len = name.len() as u32;
            let mut data_len = (data.len() * 2) as u32;
            let mut ty: u32 = 0;
            // SAFETY: die Puffergrößen stimmen mit `name_len` (Zeichen) und `data_len` (Bytes) überein.
            let rc = unsafe { RegEnumValueW(key, i, name.as_mut_ptr(), &mut name_len, std::ptr::null(), &mut ty, data.as_mut_ptr().cast(), &mut data_len) };
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc != ERROR_SUCCESS || (ty != REG_SZ && ty != REG_EXPAND_SZ) {
                continue;
            }
            let n = (data_len as usize / 2).min(data.len());
            out.push((String::from_utf16_lossy(&name[..name_len as usize]), from_wide(&data[..n])));
        }
        // SAFETY: `key` wurde oben erfolgreich geöffnet.
        unsafe { RegCloseKey(key) };
        out
    }

    fn set_str(subkey: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let sk = wide(subkey);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: gültiger Schlüsselname; `key` wird nur bei Erfolg benutzt und danach geschlossen.
        let rc = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, sk.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, std::ptr::null(), &mut key, std::ptr::null_mut()) };
        if rc != ERROR_SUCCESS {
            return Err(err(rc));
        }
        let vn = name.map(wide);
        let data = wide(value);
        // SAFETY: `data` ist nullterminiertes UTF-16; die Länge in Bytes schließt die Null ein (wie REG_SZ verlangt).
        let rc = unsafe { RegSetValueExW(key, vn.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        // SAFETY: `key` wurde oben erfolgreich geöffnet.
        unsafe { RegCloseKey(key) };
        if rc == ERROR_SUCCESS { Ok(()) } else { Err(err(rc)) }
    }

    fn ok_or_missing(rc: u32) -> Result<(), String> {
        if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(err(rc)) }
    }

    pub fn apply(ops: &[RegOp]) -> Result<(), String> {
        for op in ops {
            match op {
                RegOp::SetStr { key, name, value } => set_str(key, name.as_deref(), value)?,
                RegOp::DeleteValue { key, name } => {
                    let (k, n) = (wide(key), wide(name));
                    // SAFETY: gültige, nullterminierte UTF-16-Puffer.
                    ok_or_missing(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) })?
                }
                RegOp::DeleteTree { key } => {
                    let k = wide(key);
                    // SAFETY: gültiger Schlüsselname; mit Unterschlüssel-Name löscht RegDeleteTreeW auch den Schlüssel selbst.
                    ok_or_missing(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) })?
                }
            }
        }
        Ok(())
    }

    /// `InstallLocation` aus `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck` (vom NSIS-Installer).
    pub fn install_location() -> Option<String> {
        let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck".encode_utf16().chain([0]).collect();
        let val: Vec<u16> = "InstallLocation".encode_utf16().chain([0]).collect();
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: nullterminierte Namen, Puffer mit `len` Bytes; RRF_RT_REG_SZ liefert eine nullterminierte Zeichenkette.
        let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut len) };
        if rc != 0 {
            return None;
        }
        let n = (len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        Some(s.trim_end_matches('\0').to_string())
    }
}

#[cfg(not(windows))]
pub fn get_str(_subkey: &str, _value: Option<&str>) -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn values(_subkey: &str) -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn apply(_ops: &[RegOp]) -> Result<(), String> {
    Err(super::unsupported())
}

#[cfg(not(windows))]
pub fn install_location() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    /// Nur lesend: `HKCU\Environment\TEMP` ist üblicherweise `REG_EXPAND_SZ` – `get_str` muss ihn (erweitert) liefern,
    /// wenn er existiert.
    #[cfg(windows)]
    #[test]
    fn reads_expand_sz() {
        let listed = super::values("Environment").into_iter().find(|(n, _)| n.eq_ignore_ascii_case("TEMP"));
        let got = super::get_str("Environment", Some("TEMP"));
        assert_eq!(listed.is_some(), got.is_some(), "{listed:?} / {got:?}");
        if let Some(g) = got {
            assert!(!g.contains("%USERPROFILE%"), "{g}");
        }
    }

    #[test]
    fn nothing_is_read_without_a_store() {
        // Ein Schlüssel, den es nirgends gibt: keine Werte, keine Zeichenkette (auf jedem System).
        assert!(super::values(r"Software\Paneldeck-Test-Does-Not-Exist-7f3a").is_empty());
        assert_eq!(super::get_str(r"Software\Paneldeck-Test-Does-Not-Exist-7f3a", None), None);
    }
}
