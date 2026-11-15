//! Installieren und Neustarten: Installationsart erkennen, Setup still ausführen bzw. `paneldeck.exe` tauschen (portabel),
//! danach Paneldeck mit denselben Instanz-Parametern neu starten.

use super::release::Flavor;
use std::path::{Path, PathBuf};

/// Umgebungsvariable für den Neustart: die neue Instanz wartet, bis der alte Prozess (diese PID) beendet ist, bevor sie die
/// Einzelinstanz-Prüfung macht (sonst würde sie sich an die noch laufende alte Instanz übergeben und beenden).
pub const WAIT_PID_ENV: &str = "PANELDECK_UPDATE_WAIT_PID";

/// Pfad ohne `\\?\`-Präfix, mit `\`, ohne abschließenden Trenner, klein geschrieben (Windows-Pfade sind nicht case-sensitiv).
pub fn norm_dir(p: &str) -> String {
    let s = p.trim().trim_matches('"').replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.trim_end_matches('\\').to_lowercase()
}

/// Installiert oder portabel? Installiert = der Uninstall-Eintrag des Installers (HKCU) zeigt auf genau den Ordner dieser .exe
/// und dort liegt `Uninstall.exe`. Alles andere (auch eine portable Kopie neben einer Installation) ist portabel.
pub fn flavor_for(exe: &Path, install_location: Option<&str>) -> Flavor {
    let Some(dir) = exe.parent() else { return Flavor::Portable };
    match install_location {
        Some(loc) if !loc.trim().is_empty() && norm_dir(loc) == norm_dir(&dir.to_string_lossy()) && dir.join("Uninstall.exe").is_file() => Flavor::Installed,
        _ => Flavor::Portable,
    }
}

/// Installationsart dieser Instanz (`None` außerhalb von Windows).
pub fn detect(exe: &Path) -> Option<Flavor> {
    #[cfg(windows)]
    {
        Some(flavor_for(exe, reg::install_location().as_deref()))
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        None
    }
}

#[cfg(windows)]
mod reg {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

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

/// Neue Programmdatei an den Platz der laufenden setzen: `<exe>.new` schreiben, die laufende in `<exe>.old` umbenennen (Windows
/// erlaubt das bei laufenden Programmen), die neue umbenennen. Schlägt ein Schritt fehl, wird zurückgerollt. `<exe>.old` räumt der
/// nächste Start auf (`main.rs`).
pub fn swap_exe(exe: &Path, new_bytes: &[u8]) -> Result<(), String> {
    let with = |ext: &str| {
        let mut s = exe.as_os_str().to_owned();
        s.push(ext);
        PathBuf::from(s)
    };
    let (new, old) = (with(".new"), with(".old"));
    let _ = std::fs::remove_file(&new);
    std::fs::write(&new, new_bytes).map_err(|e| format!("{}: {e}", new.display()))?;
    if old.exists() {
        if let Err(e) = std::fs::remove_file(&old) {
            let _ = std::fs::remove_file(&new);
            return Err(format!("{}: {e}", old.display()));
        }
    }
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("{}: {e}", exe.display()));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        // Zurückrollen: die alte Datei wieder an ihren Platz
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(format!("{}: {e}", exe.display()));
    }
    Ok(())
}

/// Kommandozeilen-Teil `/D=<ordner>` für NSIS: muss der letzte Parameter sein und darf nicht in Anführungszeichen stehen
/// (auch nicht bei Leerzeichen). Zeichen, die NSIS bzw. die Kommandozeile verwirren, werden abgelehnt.
pub fn nsis_dir_arg(dir: &Path) -> Result<String, String> {
    let d = dir.to_string_lossy();
    let d = d.strip_prefix(r"\\?\").unwrap_or(&d);
    if d.is_empty() || d.contains(['"', '\n', '\r']) {
        return Err(format!("{d}: unsupported folder name"));
    }
    Ok(format!("/D={}", d.trim_end_matches('\\')))
}

/// Setup still (`/S`) in den Ordner `dir` ausführen und auf das Ende warten. Der Installer kommt mit einer laufenden Instanz
/// zurecht (benennt die gesperrte .exe um) und behält die bisherige Auswahl (Desktop-Verknüpfung, Kontextmenü) bei.
pub fn run_setup(setup: &Path, dir: &Path, timeout: std::time::Duration) -> Result<(), String> {
    let mut cmd = std::process::Command::new(setup);
    cmd.arg("/S");
    let d = nsis_dir_arg(dir)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(d);
    }
    #[cfg(not(windows))]
    cmd.arg(d);
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", setup.display()))?;
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => return Err(format!("setup exit code {}", st.code().unwrap_or(-1))),
            Ok(None) if t0.elapsed() > timeout => {
                let _ = child.kill();
                return Err("setup timed out".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Parameter der laufenden Instanz (Datenordner/Profil), damit der Neustart dieselben Daten öffnet.
fn instance_args() -> Vec<String> {
    // TODO(2.1 wiring): paths::instance_args()
    Vec::new()
}

/// Startet `exe` neu (gleiche Instanz-Parameter). Die neue Instanz wartet über `WAIT_PID_ENV`, bis dieser Prozess beendet ist.
pub fn spawn_restart(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(instance_args())
        .env(WAIT_PID_ENV, std::process::id().to_string())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", exe.display()))
}

/// Für `main.rs`, ganz am Anfang (vor der Einzelinstanz-Weiterleitung): wurde diese Instanz von einem Update neu gestartet,
/// warten, bis die alte beendet ist (höchstens 30 s), und die Variable entfernen (Terminals sollen sie nicht erben).
///
/// TODO(2.1 wiring): in `main()` vor `cli::init()`/`forward_to_running` aufrufen:
/// `app::update_wait_for_previous_instance();`
#[allow(dead_code)]
pub fn wait_for_previous_instance() {
    let Some(v) = std::env::var_os(WAIT_PID_ENV) else { return };
    // SAFETY: wird ganz am Anfang von `main` aufgerufen, bevor weitere Fäden laufen.
    unsafe { std::env::remove_var(WAIT_PID_ENV) };
    let Some(pid) = v.to_str().and_then(|s| s.parse::<u32>().ok()) else { return };
    if pid == std::process::id() {
        return;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        // SAFETY: OpenProcess mit reinem Warte-Recht; das Handle wird geschlossen.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if !h.is_null() {
                WaitForSingleObject(h, 30_000);
                CloseHandle(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        static C: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("paneldeck-test-upd-{tag}-{}-{}", std::process::id(), C.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn flavor_detection() {
        let d = tmp("flavor");
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let loc = d.to_string_lossy().to_string();
        // Ohne Uninstall.exe: portabel, auch wenn der Registry-Eintrag passt (veralteter Eintrag)
        assert_eq!(flavor_for(&exe, Some(&loc)), Flavor::Portable);
        std::fs::write(d.join("Uninstall.exe"), b"MZ").unwrap();
        assert_eq!(flavor_for(&exe, Some(&loc)), Flavor::Installed);
        assert_eq!(flavor_for(&exe, Some(&format!("{}\\", loc.to_uppercase()))), Flavor::Installed, "Groß/klein und Trenner egal");
        assert_eq!(flavor_for(&exe, Some(&format!("\"{loc}\""))), Flavor::Installed);
        assert_eq!(flavor_for(&exe, Some(r"C:\Users\x\AppData\Local\Programs\Paneldeck")), Flavor::Portable, "anderer Ordner");
        assert_eq!(flavor_for(&exe, Some("")), Flavor::Portable);
        assert_eq!(flavor_for(&exe, None), Flavor::Portable);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn norm() {
        assert_eq!(norm_dir(r"\\?\C:\Foo\Bar\"), r"c:\foo\bar");
        assert_eq!(norm_dir("C:/Foo/Bar"), r"c:\foo\bar");
    }

    #[test]
    fn swap_portable() {
        let d = tmp("swap");
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(d.join("paneldeck.exe.old"), b"older").unwrap();
        swap_exe(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(d.join("paneldeck.exe.old")).unwrap(), b"old");
        assert!(!d.join("paneldeck.exe.new").exists());
        // Läuft die alte Datei noch (gesperrt), klappt das Umbenennen trotzdem – unter Windows hier mit offenem Handle geprüft
        #[cfg(windows)]
        {
            let f = std::fs::File::open(&exe).unwrap();
            swap_exe(&exe, b"newer").unwrap();
            drop(f);
            assert_eq!(std::fs::read(&exe).unwrap(), b"newer");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_rolls_back() {
        let d = tmp("rollback");
        // Programmdatei fehlt: Umbenennen scheitert, keine Reste
        let exe = d.join("paneldeck.exe");
        assert!(swap_exe(&exe, b"new").is_err());
        assert!(!d.join("paneldeck.exe.new").exists());
        assert!(!exe.exists());
        // Ordner nicht beschreibbar (existiert nicht)
        assert!(swap_exe(&d.join("missing").join("paneldeck.exe"), b"x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nsis_arg() {
        assert_eq!(nsis_dir_arg(Path::new(r"C:\Users\A B\AppData\Local\Programs\Paneldeck")).unwrap(), r"/D=C:\Users\A B\AppData\Local\Programs\Paneldeck");
        assert_eq!(nsis_dir_arg(Path::new(r"\\?\C:\X\")).unwrap(), r"/D=C:\X");
        assert!(nsis_dir_arg(Path::new("C:\\a\"b")).is_err());
        assert!(nsis_dir_arg(Path::new("")).is_err());
    }

    #[test]
    fn wait_env_is_consumed() {
        // Eigene PID: sofort zurück, Variable entfernt
        // SAFETY: Test setzt eine nur hier gelesene Variable.
        unsafe { std::env::set_var(WAIT_PID_ENV, std::process::id().to_string()) };
        wait_for_previous_instance();
        assert!(std::env::var_os(WAIT_PID_ENV).is_none());
        wait_for_previous_instance(); // ohne Variable: nichts
    }
}
