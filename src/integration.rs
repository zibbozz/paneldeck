//! Windows-Integration pro Benutzer (ohne Administratorrechte): Autostart und Explorer-Kontextmenü.
//! Geschrieben wird mit `reg.exe` unter HKCU, gelesen über die Registry-API (Unicode-sicher); alles ist rückgängig machbar.
//!
//! Instanzen mit eigenem Datenordner/Profil (`--data-dir`, `--profile`) tragen ihre Argumente in die Befehle ein
//! (Aufbau und Namen: `crate::launch`). Autostart hat je Instanz einen eigenen Run-Wert; das Kontextmenü gibt es nur
//! einmal (feste Schlüssel), die Instanz, die es zuletzt schreibt, steht im Befehl.

use std::path::Path;

use crate::launch;

/// Startargumente dieser Instanz (`--data-dir …`, `--profile …`); leer für die Standardinstanz.
// TODO(2.1 wiring): use paths::instance_args()
fn instance_args() -> Vec<String> {
    Vec::new()
}

/// Erwartete Kontextmenü-Einträge: (Registrierungsschlüssel unter HKCU, Beschriftung, Befehl).
fn context_entries(exe: &Path, args: &[String]) -> Result<Vec<(&'static str, &'static str, String)>, String> {
    let open = crate::state::t("In Paneldeck öffnen", "Open in Paneldeck");
    let claude = crate::state::t("Mit Claude in Paneldeck starten", "Start with Claude in Paneldeck");
    Ok(vec![
        (r"Software\Classes\Directory\shell\Paneldeck", open, launch::context_command(exe, args, false, "%1")?),
        (r"Software\Classes\Directory\Background\shell\Paneldeck", open, launch::context_command(exe, args, false, "%V")?),
        (r"Software\Classes\Directory\shell\PaneldeckClaude", claude, launch::context_command(exe, args, true, "%1")?),
        (r"Software\Classes\Directory\Background\shell\PaneldeckClaude", claude, launch::context_command(exe, args, true, "%V")?),
    ])
}

/// Ob ein gespeicherter Run-Wert genau der Autostart-Befehl dieser Instanz ist.
fn autostart_matches(stored: Option<&str>, exe: &Path, args: &[String]) -> bool {
    match (stored, launch::run_command(exe, args)) {
        (Some(s), Ok(want)) => launch::same_command(s, &want),
        _ => false,
    }
}

/// Ob der gespeicherte Kontextmenü-Befehl (`Directory\shell\Paneldeck\command`) zu dieser Instanz gehört.
fn context_matches(stored: Option<&str>, exe: &Path, args: &[String]) -> bool {
    match (stored, launch::context_command(exe, args, false, "%1")) {
        (Some(s), Ok(want)) => launch::same_command(s, &want),
        _ => false,
    }
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
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegEnumValueW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_EXPAND_SZ, REG_SZ, RRF_RT_REG_SZ};

    pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// Zeichenkette unter HKCU lesen (`value = None` = Standardwert). `None`, wenn nicht vorhanden.
    pub fn get_str(subkey: &str, value: Option<&str>) -> Option<String> {
        let sk = wide(subkey);
        let vn = value.map(wide);
        let vp = vn.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
        let mut size: u32 = 0;
        // SAFETY: gültige, nullterminierte UTF-16-Puffer; der erste Aufruf fragt nur die Größe ab.
        let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, sk.as_ptr(), vp, RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut cb = (buf.len() * 2) as u32;
        // SAFETY: `buf` ist `cb` Bytes groß.
        let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, sk.as_ptr(), vp, RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut cb) };
        (rc == ERROR_SUCCESS).then(|| from_wide(&buf))
    }

    /// Alle Zeichenketten-Werte eines Schlüssels unter HKCU (Name, Daten).
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

    pub fn reg(args: &[&str]) -> Result<String, String> {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("reg.exe")
            .args(args)
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

#[cfg(windows)]
fn this_exe() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// Ob der Autostart dieser Instanz eingetragen ist (eigener Run-Wert mit genau ihrem Befehl).
#[cfg(windows)]
pub fn autostart_enabled() -> bool {
    let args = instance_args();
    let name = launch::run_value_name(&launch::Instance::from_args(&args));
    autostart_matches(win::get_str(win::RUN_KEY, Some(&name)).as_deref(), &this_exe(), &args)
}

/// Autostart dieser Instanz ein-/ausschalten; berührt nur ihren eigenen Run-Wert.
#[cfg(windows)]
pub fn set_autostart(on: bool, exe: &Path) -> Result<(), String> {
    let args = instance_args();
    let name = launch::run_value_name(&launch::Instance::from_args(&args));
    let key = format!(r"HKCU\{}", win::RUN_KEY);
    if on {
        let cmd = launch::run_command(exe, &args)?;
        win::reg(&["add", &key, "/v", &name, "/t", "REG_SZ", "/d", &cmd, "/f"]).map(|_| ())
    } else {
        win::reg(&["delete", &key, "/v", &name, "/f"]).map(|_| ())
    }
}

/// Ob das Kontextmenü eingetragen ist und auf diese Instanz zeigt (eine andere Instanz kann es überschrieben haben).
#[cfg(windows)]
pub fn context_enabled() -> bool {
    let stored = win::get_str(r"Software\Classes\Directory\shell\Paneldeck\command", None);
    context_matches(stored.as_deref(), &this_exe(), &instance_args())
}

/// Kontextmenü ein-/ausschalten. Die Schlüssel sind für alle Instanzen dieselben; der Befehl trägt die Argumente
/// der Instanz, die zuletzt schreibt.
#[cfg(windows)]
pub fn set_context(on: bool, exe: &Path) -> Result<(), String> {
    let args = instance_args();
    for (key, label, cmd) in context_entries(exe, &args)? {
        let key = format!(r"HKCU\{key}");
        if on {
            win::reg(&["add", &key, "/ve", "/d", label, "/f"])?;
            win::reg(&["add", &key, "/v", "Icon", "/t", "REG_SZ", "/d", &format!("\"{}\"", exe.to_string_lossy()), "/f"])?;
            win::reg(&["add", &format!("{key}\\command"), "/ve", "/d", &cmd, "/f"])?;
        } else {
            let _ = win::reg(&["delete", &key, "/f"]);
        }
    }
    Ok(())
}

/// Vom Deinstallationsprogramm (`--uninstall-hooks`): Autostart-Werte `Paneldeck (…)` entfernen, die auf diese .exe
/// zeigen. Best-Effort, still.
#[cfg(windows)]
pub fn remove_instance_autostarts() {
    let key = format!(r"HKCU\{}", win::RUN_KEY);
    for name in instance_run_values_for(&win::values(win::RUN_KEY), &this_exe()) {
        let _ = win::reg(&["delete", &key, "/v", &name, "/f"]);
    }
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    false
}
#[cfg(not(windows))]
pub fn set_autostart(_on: bool, _exe: &Path) -> Result<(), String> {
    Err(crate::state::t("nur unter Windows", "Windows only").into())
}
#[cfg(not(windows))]
pub fn context_enabled() -> bool {
    false
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

    #[test]
    fn context_entries_default_and_profile() {
        let exe = PathBuf::from(r"C:\Program Files\Paneldeck\paneldeck.exe");
        let d = context_entries(&exe, &[]).unwrap();
        let keys: Vec<&str> = d.iter().map(|e| e.0).collect();
        assert_eq!(keys, [r"Software\Classes\Directory\shell\Paneldeck", r"Software\Classes\Directory\Background\shell\Paneldeck", r"Software\Classes\Directory\shell\PaneldeckClaude", r"Software\Classes\Directory\Background\shell\PaneldeckClaude"]);
        let cmds: Vec<&str> = d.iter().map(|e| e.2.as_str()).collect();
        let e = r#""C:\Program Files\Paneldeck\paneldeck.exe""#;
        assert_eq!(cmds, [format!(r#"{e} --open "%1""#), format!(r#"{e} --open "%V""#), format!(r#"{e} --claude --open "%1""#), format!(r#"{e} --claude --open "%V""#)]);
        let p = context_entries(&exe, &s(&["--profile", "work"])).unwrap();
        assert_eq!(p[0].0, d[0].0, "keys stay the same for every instance");
        assert_eq!(p[0].2, format!(r#"{e} --profile work --open "%1""#));
        assert_eq!(p[3].2, format!(r#"{e} --profile work --claude --open "%V""#));
    }

    #[test]
    fn status_compares_commands() {
        let exe = PathBuf::from(r"C:\pd\paneldeck.exe");
        let work = s(&["--profile", "work"]);
        assert!(autostart_matches(Some(r#""C:\pd\paneldeck.exe" --tray"#), &exe, &[]));
        assert!(autostart_matches(Some(r#""c:\PD\paneldeck.exe" --tray"#), &exe, &[]));
        assert!(!autostart_matches(None, &exe, &[]));
        assert!(!autostart_matches(Some(r#""D:\old\paneldeck.exe" --tray"#), &exe, &[]));
        assert!(autostart_matches(Some(r#""C:\pd\paneldeck.exe" --profile work --tray"#), &exe, &work));
        assert!(!autostart_matches(Some(r#""C:\pd\paneldeck.exe" --tray"#), &exe, &work));
        // Kontextmenü: von einer anderen Instanz geschrieben → für mich aus
        let mine = r#""C:\pd\paneldeck.exe" --profile work --open "%1""#;
        assert!(context_matches(Some(mine), &exe, &work));
        assert!(!context_matches(Some(mine), &exe, &[]));
        assert!(context_matches(Some(r#""C:\pd\paneldeck.exe" --open "%1""#), &exe, &[]));
        assert!(!context_matches(None, &exe, &[]));
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
}
