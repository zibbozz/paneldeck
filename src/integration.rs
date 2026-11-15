//! Windows-Integration pro Benutzer (ohne Administratorrechte): Autostart und Explorer-Kontextmenü.
//! Gelesen und geschrieben wird über die Registry-API unter HKCU (Unicode-sicher); alles ist rückgängig machbar.
//!
//! Instanzen mit eigenem Datenordner/Profil (`--data-dir`, `--profile`) tragen ihre Argumente in die Befehle ein
//! (Aufbau und Namen: `crate::launch`). Autostart hat je Instanz einen eigenen Run-Wert; das Kontextmenü gibt es nur
//! einmal (feste Schlüssel), die Instanz, die es zuletzt schreibt, steht im Befehl.
//!
//! Was geschrieben wird, beschreibt ein reiner „Plan“ (`RegOp`-Liste, getestet); ausgeführt wird er von `win::apply`.

use std::path::Path;

use crate::launch;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const CONTEXT_PROBE: &str = r"Software\Classes\Directory\shell\Paneldeck\command";

/// Zustand einer Integration aus Sicht dieser Instanz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Nicht eingetragen.
    Off,
    /// Eingetragen, genau mit dem Befehl dieser Instanz.
    Mine,
    /// Eingetragen, aber mit einem anderen Befehl (andere .exe oder andere Instanz) – der gespeicherte Befehl.
    Other(String),
}

impl Status {
    pub fn is_mine(&self) -> bool {
        *self == Status::Mine
    }
    pub fn other(&self) -> Option<&str> {
        match self {
            Status::Other(c) => Some(c),
            _ => None,
        }
    }
}

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

fn status_of(stored: Option<&str>, want: Result<String, String>) -> Status {
    match (stored, want) {
        (None, _) => Status::Off,
        (Some(s), Ok(w)) if launch::same_command(s, &w) => Status::Mine,
        (Some(s), _) => Status::Other(s.to_string()),
    }
}

/// Status des Autostarts dieser Instanz aus dem gespeicherten Run-Wert (ihres eigenen Namens).
fn autostart_status_of(stored: Option<&str>, exe: &Path, args: &[String]) -> Status {
    status_of(stored, launch::run_command(exe, args))
}

/// Status des Kontextmenüs aus dem gespeicherten Befehl `Directory\shell\Paneldeck\command`.
fn context_status_of(stored: Option<&str>, exe: &Path, args: &[String]) -> Status {
    status_of(stored, launch::context_command(exe, args, false, "%1"))
}

/// Plan für den Autostart: nur der Run-Wert dieser Instanz.
fn autostart_plan(on: bool, exe: &Path, args: &[String]) -> Result<Vec<RegOp>, String> {
    let name = launch::run_value_name(&launch::Instance::from_args(args));
    Ok(vec![if on {
        RegOp::SetStr { key: RUN_KEY.into(), name: Some(name), value: launch::run_command(exe, args)? }
    } else {
        RegOp::DeleteValue { key: RUN_KEY.into(), name }
    }])
}

/// Plan für das Kontextmenü (Schlüssel, Beschriftung, Symbol, Befehl – wie bisher, der Befehl mit Instanz-Argumenten).
fn context_plan(on: bool, exe: &Path, args: &[String]) -> Result<Vec<RegOp>, String> {
    let open = crate::state::t("In Paneldeck öffnen", "Open in Paneldeck");
    let claude = crate::state::t("Mit Claude in Paneldeck starten", "Start with Claude in Paneldeck");
    let entries = [
        (r"Software\Classes\Directory\shell\Paneldeck", open, false, "%1"),
        (r"Software\Classes\Directory\Background\shell\Paneldeck", open, false, "%V"),
        (r"Software\Classes\Directory\shell\PaneldeckClaude", claude, true, "%1"),
        (r"Software\Classes\Directory\Background\shell\PaneldeckClaude", claude, true, "%V"),
    ];
    let mut ops = Vec::new();
    for (key, label, with_claude, ph) in entries {
        if on {
            let cmd = launch::context_command(exe, args, with_claude, ph)?;
            ops.push(RegOp::SetStr { key: key.into(), name: None, value: label.into() });
            ops.push(RegOp::SetStr { key: key.into(), name: Some("Icon".into()), value: format!("\"{}\"", exe.to_string_lossy()) });
            ops.push(RegOp::SetStr { key: format!("{key}\\command"), name: None, value: cmd });
        } else {
            ops.push(RegOp::DeleteTree { key: key.into() });
        }
    }
    Ok(ops)
}

/// Beim Deinstallieren: Run-Werte `Paneldeck (…)` (Instanzen mit Profil/Datenordner), deren Programm diese .exe ist.
/// Der Standardwert `Paneldeck` gehört dem Installer und wird dort entfernt.
fn instance_run_values_for(values: &[(String, String)], exe: &Path) -> Vec<String> {
    let exe = exe.to_string_lossy();
    values
        .iter()
        .filter(|(name, data)| launch::is_instance_run_value(name) && launch::same_path(launch::program_of(data), &exe))
        .map(|(name, _)| name.clone())
        .collect()
}

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
}

#[cfg(windows)]
fn this_exe() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// Autostart dieser Instanz: aus, an (eigener Run-Wert mit genau ihrem Befehl) oder auf etwas anderes zeigend.
#[cfg(windows)]
pub fn autostart_status() -> Status {
    let args = launch::instance_args();
    let name = launch::run_value_name(&launch::Instance::from_args(&args));
    autostart_status_of(win::get_str(RUN_KEY, Some(&name)).as_deref(), &this_exe(), &args)
}

/// Autostart dieser Instanz ein-/ausschalten; berührt nur ihren eigenen Run-Wert.
#[cfg(windows)]
pub fn set_autostart(on: bool, exe: &Path) -> Result<(), String> {
    win::apply(&autostart_plan(on, exe, &launch::instance_args())?)
}

/// Kontextmenü: aus, an (zeigt auf diese Instanz) oder von einer anderen Instanz/.exe geschrieben.
#[cfg(windows)]
pub fn context_status() -> Status {
    context_status_of(win::get_str(CONTEXT_PROBE, None).as_deref(), &this_exe(), &launch::instance_args())
}

/// Kontextmenü ein-/ausschalten. Die Schlüssel sind für alle Instanzen dieselben; der Befehl trägt die Argumente
/// der Instanz, die zuletzt schreibt.
#[cfg(windows)]
pub fn set_context(on: bool, exe: &Path) -> Result<(), String> {
    win::apply(&context_plan(on, exe, &launch::instance_args())?)
}

/// Vom Deinstallationsprogramm (`--uninstall-hooks`): Autostart-Werte `Paneldeck (…)` entfernen, die auf diese .exe
/// zeigen. Best-Effort, still.
#[cfg(windows)]
pub fn remove_instance_autostarts() {
    for name in instance_run_values_for(&win::values(RUN_KEY), &this_exe()) {
        let _ = win::apply(&[RegOp::DeleteValue { key: RUN_KEY.into(), name }]);
    }
}

#[cfg(not(windows))]
pub fn autostart_status() -> Status {
    Status::Off
}
#[cfg(not(windows))]
pub fn set_autostart(_on: bool, _exe: &Path) -> Result<(), String> {
    Err(crate::state::t("nur unter Windows", "Windows only").into())
}
#[cfg(not(windows))]
pub fn context_status() -> Status {
    Status::Off
}
#[cfg(not(windows))]
pub fn set_context(_on: bool, _exe: &Path) -> Result<(), String> {
    Err(crate::state::t("nur unter Windows", "Windows only").into())
}
#[cfg(not(windows))]
pub fn remove_instance_autostarts() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn set(key: &str, name: Option<&str>, value: &str) -> RegOp {
        RegOp::SetStr { key: key.into(), name: name.map(Into::into), value: value.into() }
    }

    #[test]
    fn autostart_plan_strings() {
        let exe = PathBuf::from(r"C:\Users\A B\AppData\Local\Programs\Paneldeck\paneldeck.exe");
        let e = r#""C:\Users\A B\AppData\Local\Programs\Paneldeck\paneldeck.exe""#;
        // Standardinstanz: exakt wie 2.0.0 (und wie der Installer)
        assert_eq!(autostart_plan(true, &exe, &[]).unwrap(), [set(RUN_KEY, Some("Paneldeck"), &format!("{e} --tray"))]);
        assert_eq!(autostart_plan(false, &exe, &[]).unwrap(), [RegOp::DeleteValue { key: RUN_KEY.into(), name: "Paneldeck".into() }]);
        let w = s(&["--profile", "Work"]);
        assert_eq!(autostart_plan(true, &exe, &w).unwrap(), [set(RUN_KEY, Some("Paneldeck (work)"), &format!("{e} --profile Work --tray"))]);
        assert_eq!(autostart_plan(false, &exe, &w).unwrap(), [RegOp::DeleteValue { key: RUN_KEY.into(), name: "Paneldeck (work)".into() }]);
        assert_eq!(RUN_KEY, r"Software\Microsoft\Windows\CurrentVersion\Run");
        assert!(autostart_plan(true, &exe, &s(&["--data-dir", r"D:\100%"])).is_err());
    }

    #[test]
    fn context_plan_strings() {
        let exe = PathBuf::from(r"C:\Program Files\Paneldeck\paneldeck.exe");
        let e = r#""C:\Program Files\Paneldeck\paneldeck.exe""#;
        let (open, claude) = (crate::state::t("In Paneldeck öffnen", "Open in Paneldeck"), crate::state::t("Mit Claude in Paneldeck starten", "Start with Claude in Paneldeck"));
        let d = context_plan(true, &exe, &[]).unwrap();
        let want = |args: &str| {
            let mut v = Vec::new();
            for (key, label, cl, ph) in [
                (r"Software\Classes\Directory\shell\Paneldeck", open, "", "%1"),
                (r"Software\Classes\Directory\Background\shell\Paneldeck", open, "", "%V"),
                (r"Software\Classes\Directory\shell\PaneldeckClaude", claude, " --claude", "%1"),
                (r"Software\Classes\Directory\Background\shell\PaneldeckClaude", claude, " --claude", "%V"),
            ] {
                v.push(set(key, None, label));
                v.push(set(key, Some("Icon"), e));
                v.push(set(&format!(r"{key}\command"), None, &format!(r#"{e}{args}{cl} --open "{ph}""#)));
            }
            v
        };
        assert_eq!(d, want(""));
        assert_eq!(context_plan(true, &exe, &s(&["--profile", "work"])).unwrap(), want(" --profile work"));
        let off = context_plan(false, &exe, &[]).unwrap();
        assert_eq!(off.len(), 4);
        assert!(off.iter().all(|o| matches!(o, RegOp::DeleteTree { key } if key.starts_with(r"Software\Classes\Directory\"))));
        assert_eq!(CONTEXT_PROBE,r"Software\Classes\Directory\shell\Paneldeck\command");
    }

    #[test]
    fn status_three_states() {
        let exe = PathBuf::from(r"C:\pd\paneldeck.exe");
        let work = s(&["--profile", "work"]);
        assert_eq!(autostart_status_of(Some(r#""C:\pd\paneldeck.exe" --tray"#), &exe, &[]), Status::Mine);
        assert_eq!(autostart_status_of(Some(r#""c:\PD\paneldeck.exe" --tray"#), &exe, &[]), Status::Mine);
        assert_eq!(autostart_status_of(None, &exe, &[]), Status::Off);
        let old = r#""D:\old\paneldeck.exe" --tray"#;
        assert_eq!(autostart_status_of(Some(old), &exe, &[]), Status::Other(old.into()));
        assert_eq!(autostart_status_of(Some(r#""C:\pd\paneldeck.exe" --profile work --tray"#), &exe, &work), Status::Mine);
        assert!(autostart_status_of(Some(r#""C:\pd\paneldeck.exe" --tray"#), &exe, &work).other().is_some());
        // Kontextmenü: von einer anderen Instanz geschrieben → „andere“, mit deren Befehl
        let mine = r#""C:\pd\paneldeck.exe" --profile work --open "%1""#;
        assert!(context_status_of(Some(mine), &exe, &work).is_mine());
        assert_eq!(context_status_of(Some(mine), &exe, &[]).other(), Some(mine));
        assert!(context_status_of(Some(r#""C:\pd\paneldeck.exe" --open "%1""#), &exe, &[]).is_mine());
        assert_eq!(context_status_of(None, &exe, &[]), Status::Off);
    }

    #[test]
    fn uninstall_picks_only_our_instance_values() {
        let exe = PathBuf::from(r"C:\Users\A\AppData\Local\Programs\Paneldeck\paneldeck.exe");
        let vals = vec![
            ("Paneldeck".to_string(), r#""C:\Users\A\AppData\Local\Programs\Paneldeck\paneldeck.exe" --tray"#.to_string()),
            ("Paneldeck (p1)".to_string(), r#""C:\Users\A\AppData\Local\Programs\Paneldeck\paneldeck.exe" --profile p1 --tray"#.to_string()),
            ("Paneldeck (abcd1234)".to_string(), r#""c:/users/a/appdata/local/programs/paneldeck/PANELDECK.exe" --data-dir "D:\x y" --tray"#.to_string()),
            ("Paneldeck (portable)".to_string(), r#""D:\Tools\paneldeck.exe" --profile portable --tray"#.to_string()),
            ("OneDrive".to_string(), r#""C:\Program Files\OneDrive\OneDrive.exe" /background"#.to_string()),
        ];
        assert_eq!(instance_run_values_for(&vals, &exe), s(&["Paneldeck (p1)", "Paneldeck (abcd1234)"]));
    }

    /// Nur lesend: `HKCU\Environment\TEMP` ist üblicherweise `REG_EXPAND_SZ` – `get_str` muss ihn (erweitert) liefern,
    /// wenn er existiert.
    #[cfg(windows)]
    #[test]
    fn reads_expand_sz() {
        let listed = win::values("Environment").into_iter().find(|(n, _)| n.eq_ignore_ascii_case("TEMP"));
        let got = win::get_str("Environment", Some("TEMP"));
        assert_eq!(listed.is_some(), got.is_some(), "{listed:?} / {got:?}");
        if let Some(g) = got {
            assert!(!g.contains("%USERPROFILE%"), "{g}");
        }
    }
}
