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
/// erlaubt das bei laufenden Programmen), die neue umbenennen. Schlägt ein Schritt fehl, wird zurückgerollt. `<exe>.old` löscht erst die neue
/// Version, nachdem sie ihren ersten Frame gezeichnet hat (`remove_old_exe`).
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
        return Err(format!("{d}: {}", crate::state::t("Ordnername wird nicht unterstützt", "unsupported folder name")));
    }
    Ok(format!("/D={}", d.trim_end_matches('\\')))
}

/// Geprüfte Datei lesen und gesperrt halten: andere dürfen sie nur lesen (nicht schreiben, umbenennen oder löschen), bis das
/// zurückgegebene Handle geschlossen wird. So kann zwischen der Prüfung und dem Start des Setups niemand die Datei austauschen.
pub fn read_locked(p: &Path) -> std::io::Result<(Vec<u8>, std::fs::File)> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        o.share_mode(FILE_SHARE_READ);
    }
    let mut f = o.open(p)?;
    let mut v = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut v)?;
    Ok((v, f))
}

#[derive(Debug, PartialEq)]
pub enum SetupError {
    /// Setup ließ sich nicht starten.
    Start(String),
    /// Setup endete mit einem Fehlercode.
    Exit(i32),
    /// Setup läuft nach der Wartezeit noch (wird nicht abgebrochen).
    StillRunning,
    Wait(String),
}

/// Setup still (`/S`) in den Ordner `dir` ausführen und auf das Ende warten. Der Installer kommt mit einer laufenden Instanz
/// zurecht (benennt die gesperrte .exe um) und behält die bisherige Auswahl (Desktop-Verknüpfung, Kontextmenü) bei. Der Aufrufer
/// hält die Datei dabei mit `read_locked` gesperrt.
pub fn run_setup(setup: &Path, dir: &Path, timeout: std::time::Duration) -> Result<(), SetupError> {
    let mut cmd = std::process::Command::new(setup);
    cmd.arg("/S");
    let d = nsis_dir_arg(dir).map_err(SetupError::Start)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(d);
    }
    #[cfg(not(windows))]
    cmd.arg(d);
    let mut child = cmd.spawn().map_err(|e| SetupError::Start(format!("{}: {e}", setup.display())))?;
    // Auch nach der Wartezeit nicht beenden: `child` wird ohne kill() freigegeben, das Setup läuft weiter.
    wait_setup(&mut child, timeout)
}

/// Auf das Setup warten. Nach `timeout` wird es nicht abgebrochen (ein halb fertiges Setup wäre schlimmer als ein langsames),
/// nur nicht länger gewartet.
pub fn wait_setup(child: &mut std::process::Child, timeout: std::time::Duration) -> Result<(), SetupError> {
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => return Err(SetupError::Exit(st.code().unwrap_or(-1))),
            Ok(None) if t0.elapsed() > timeout => return Err(SetupError::StillRunning),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => return Err(SetupError::Wait(e.to_string())),
        }
    }
}

/// `paneldeck.exe.old` (alte Programmdatei nach einem Update – vom portablen Tausch oder vom Installer über eine laufende
/// Instanz) löschen. Aufgerufen erst, wenn die neue Version ihren ersten Frame gezeichnet hat, also nachweislich startet.
///
/// Wiederherstellung von Hand, falls eine neue Version gar nicht startet: Paneldeck beenden, `paneldeck.exe` löschen (oder
/// umbenennen) und `paneldeck.exe.old` wieder in `paneldeck.exe` umbenennen.
pub fn remove_old_exe(exe: &Path) {
    let mut old = exe.as_os_str().to_owned();
    old.push(".old");
    let _ = std::fs::remove_file(PathBuf::from(old));
}

/// Startet `exe` neu mit denselben Instanz-Parametern (`--data-dir`/`--profile` aus `paths::instance_args`, als einzelne
/// Argumente). Nie `--open`, `--claude` oder `mcp`: der Neustart soll nur dieselbe Instanz öffnen. Die neue Instanz wartet
/// über `WAIT_PID_ENV`, bis dieser Prozess beendet ist.
pub fn spawn_restart(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(restart_args(crate::paths::instance_args()))
        .env(WAIT_PID_ENV, std::process::id().to_string())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", exe.display()))
}

/// Argumente für den Neustart: nur Instanz-Parameter, Start-Aktionen (`--open`, `--claude`, `mcp`) fallen sicherheitshalber weg.
fn restart_args(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--open" => {
                it.next();
            }
            "--claude" | "mcp" => {}
            _ => out.push(a),
        }
    }
    out
}

/// Für `main.rs`, nach den `--hook`/`--uninstall-hooks`/`--help`/`mcp`-Zweigen und vor der Einzelinstanz-Weiterleitung: wurde
/// diese Instanz von einem Update neu gestartet, warten, bis die alte beendet ist (höchstens 30 s) – sonst übergäbe sie sich an
/// die noch laufende alte und beendete sich. Danach die Variable entfernen (Terminals sollen sie nicht erben).
pub fn wait_for_previous_instance() {
    let Some(v) = std::env::var_os(WAIT_PID_ENV) else { return };
    let pid = v.to_str().and_then(|s| s.parse::<u32>().ok()).filter(|p| *p != std::process::id());
    if let Some(pid) = pid {
        wait_pid(pid);
    }
    // SAFETY: wird früh in `main` aufgerufen, bevor weitere Fäden laufen.
    unsafe { std::env::remove_var(WAIT_PID_ENV) };
}

fn wait_pid(pid: u32) {
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
    }    #[cfg(not(windows))]
    let _ = pid;
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

    #[cfg(windows)]
    #[test]
    fn locked_setup_can_run_but_not_change() {
        let d = tmp("lock");
        let exe = d.join("Paneldeck-Setup-9.9.9.exe");
        std::fs::copy(r"C:\Windows\System32\whoami.exe", &exe).unwrap();
        let (bytes, lock) = read_locked(&exe).unwrap();
        assert_eq!(&bytes[..2], b"MZ");
        // Gesperrt: kein Schreiben, Umbenennen oder Löschen
        assert!(std::fs::OpenOptions::new().write(true).open(&exe).is_err());
        assert!(std::fs::rename(&exe, d.join("x.exe")).is_err());
        assert!(std::fs::remove_file(&exe).is_err());
        // … aber starten lässt sie sich (whoami lehnt /S ab: Fehlercode statt Startfehler)
        let r = run_setup(&exe, &d, std::time::Duration::from_secs(20));
        assert!(matches!(r, Err(SetupError::Exit(_))), "{r:?}");
        drop(lock);
        std::fs::remove_file(&exe).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn slow_setup_is_not_killed() {
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/c", "ping", "-n", "30", "127.0.0.1"]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg("30");
            c
        };
        let mut child = cmd.stdout(std::process::Stdio::null()).spawn().unwrap();
        assert_eq!(wait_setup(&mut child, std::time::Duration::from_millis(300)), Err(SetupError::StillRunning));
        assert!(child.try_wait().unwrap().is_none(), "läuft weiter");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn restart_never_carries_start_actions() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(restart_args(v(&["--data-dir", r"D:\my data", "--profile", "work"])), v(&["--data-dir", r"D:\my data", "--profile", "work"]));
        assert_eq!(restart_args(v(&["--open", r"C:\x", "--claude", "mcp", "--profile", "a"])), v(&["--profile", "a"]));
        assert!(restart_args(Vec::new()).is_empty());
    }

    #[test]
    fn old_exe_is_removed() {
        let d = tmp("old");
        let exe = d.join("paneldeck.exe");
        std::fs::write(&exe, b"MZ new").unwrap();
        std::fs::write(d.join("paneldeck.exe.old"), b"MZ old").unwrap();
        remove_old_exe(&exe);
        assert!(!d.join("paneldeck.exe.old").exists() && exe.exists());
        remove_old_exe(&exe); // ohne .old: nichts
        let _ = std::fs::remove_dir_all(&d);
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
