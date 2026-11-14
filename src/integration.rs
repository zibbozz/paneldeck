//! Windows-Integration pro Benutzer (ohne Administratorrechte): Autostart und Explorer-Kontextmenü.
//! Umgesetzt mit `reg.exe` unter HKCU; alles ist rückgängig machbar.

use std::path::Path;

#[cfg(windows)]
fn reg(args: &[&str]) -> Result<String, String> {
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

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// (Registrierungsschlüssel, Beschriftung, Argumente nach dem Pfadplatzhalter)
#[cfg(windows)]
fn context_entries(exe: &Path) -> Vec<(String, &'static str, String)> {
    let e = exe.to_string_lossy();
    vec![
        (r"HKCU\Software\Classes\Directory\shell\Paneldeck".into(), crate::state::t("In Paneldeck öffnen", "Open in Paneldeck"), format!("\"{e}\" --open \"%1\"")),
        (r"HKCU\Software\Classes\Directory\Background\shell\Paneldeck".into(), crate::state::t("In Paneldeck öffnen", "Open in Paneldeck"), format!("\"{e}\" --open \"%V\"")),
        (r"HKCU\Software\Classes\Directory\shell\PaneldeckClaude".into(), crate::state::t("Mit Claude in Paneldeck starten", "Start with Claude in Paneldeck"), format!("\"{e}\" --claude --open \"%1\"")),
        (r"HKCU\Software\Classes\Directory\Background\shell\PaneldeckClaude".into(), crate::state::t("Mit Claude in Paneldeck starten", "Start with Claude in Paneldeck"), format!("\"{e}\" --claude --open \"%V\"")),
    ]
}

#[cfg(windows)]
pub fn autostart_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", "Paneldeck"]).is_ok()
}

#[cfg(windows)]
pub fn set_autostart(on: bool, exe: &Path) -> Result<(), String> {
    if on {
        let cmd = format!("\"{}\" --tray", exe.to_string_lossy());
        reg(&["add", RUN_KEY, "/v", "Paneldeck", "/t", "REG_SZ", "/d", &cmd, "/f"]).map(|_| ())
    } else {
        reg(&["delete", RUN_KEY, "/v", "Paneldeck", "/f"]).map(|_| ())
    }
}

#[cfg(windows)]
pub fn context_enabled() -> bool {
    reg(&["query", r"HKCU\Software\Classes\Directory\shell\Paneldeck\command"]).is_ok()
}

#[cfg(windows)]
pub fn set_context(on: bool, exe: &Path) -> Result<(), String> {
    for (key, label, cmd) in context_entries(exe) {
        if on {
            reg(&["add", &key, "/ve", "/d", label, "/f"])?;
            reg(&["add", &key, "/v", "Icon", "/t", "REG_SZ", "/d", &format!("\"{}\"", exe.to_string_lossy()), "/f"])?;
            reg(&["add", &format!("{key}\\command"), "/ve", "/d", &cmd, "/f"])?;
        } else {
            let _ = reg(&["delete", &key, "/f"]);
        }
    }
    Ok(())
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
