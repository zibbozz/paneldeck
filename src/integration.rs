//! Windows-Integration pro Benutzer (ohne Administratorrechte): Autostart und Explorer-Kontextmenü.
//! Gelesen und geschrieben wird über die Registry-API unter HKCU (Unicode-sicher); alles ist rückgängig machbar.
//!
//! Instanzen mit eigenem Datenordner/Profil (`--data-dir`, `--profile`) tragen ihre Argumente in die Befehle ein
//! (Aufbau und Namen: `crate::launch`). Autostart hat je Instanz einen eigenen Run-Wert; das Kontextmenü gibt es nur
//! einmal (feste Schlüssel), die Instanz, die es zuletzt schreibt, steht im Befehl.
//!
//! Was geschrieben wird, beschreibt ein reiner „Plan“ (`RegOp`-Liste, getestet); ausgeführt wird er von `platform::registry::apply`
//! (ohne Umsetzung auf dem System: Status „aus“, Schreiben meldet „nicht unterstützt“).

use std::path::Path;

use crate::launch;
use crate::platform::registry::{self, RegOp};

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

fn this_exe() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// Autostart dieser Instanz: aus, an (eigener Run-Wert mit genau ihrem Befehl) oder auf etwas anderes zeigend.
pub fn autostart_status() -> Status {
    let args = launch::instance_args();
    let name = launch::run_value_name(&launch::Instance::from_args(&args));
    autostart_status_of(registry::get_str(RUN_KEY, Some(&name)).as_deref(), &this_exe(), &args)
}

/// Autostart dieser Instanz ein-/ausschalten; berührt nur ihren eigenen Run-Wert.
pub fn set_autostart(on: bool, exe: &Path) -> Result<(), String> {
    registry::apply(&autostart_plan(on, exe, &launch::instance_args())?)
}

/// Kontextmenü: aus, an (zeigt auf diese Instanz) oder von einer anderen Instanz/.exe geschrieben.
pub fn context_status() -> Status {
    context_status_of(registry::get_str(CONTEXT_PROBE, None).as_deref(), &this_exe(), &launch::instance_args())
}

/// Kontextmenü ein-/ausschalten. Die Schlüssel sind für alle Instanzen dieselben; der Befehl trägt die Argumente
/// der Instanz, die zuletzt schreibt.
pub fn set_context(on: bool, exe: &Path) -> Result<(), String> {
    registry::apply(&context_plan(on, exe, &launch::instance_args())?)
}

/// Vom Deinstallationsprogramm (`--uninstall-hooks`): Autostart-Werte `Paneldeck (…)` entfernen, die auf diese .exe
/// zeigen. Best-Effort, still.
pub fn remove_instance_autostarts() {
    for name in instance_run_values_for(&registry::values(RUN_KEY), &this_exe()) {
        let _ = registry::apply(&[RegOp::DeleteValue { key: RUN_KEY.into(), name }]);
    }
}

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
        let listed = registry::values("Environment").into_iter().find(|(n, _)| n.eq_ignore_ascii_case("TEMP"));
        let got = registry::get_str("Environment", Some("TEMP"));
        assert_eq!(listed.is_some(), got.is_some(), "{listed:?} / {got:?}");
        if let Some(g) = got {
            assert!(!g.contains("%USERPROFILE%"), "{g}");
        }
    }
}
