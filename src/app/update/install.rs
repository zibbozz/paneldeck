//! Installieren und Neustarten: Installationsart erkennen, Setup still ausführen bzw. `paneldeck.exe` tauschen (portabel),
//! danach Paneldeck mit denselben Instanz-Parametern neu starten.

use super::release::Flavor;
use std::path::{Path, PathBuf};

/// Umgebungsvariable für den Neustart: die neue Instanz wartet, bis der alte Prozess (diese PID) beendet ist, bevor sie die
/// Einzelinstanz-Prüfung macht (sonst würde sie sich an die noch laufende alte Instanz übergeben und beenden).
pub const WAIT_PID_ENV: &str = "PANELDECK_UPDATE_WAIT_PID";

/// Umgebungsvariable für den Neustart nach einem Update mit „Sitzungen merken“ aus (#157): `1` = die neue Instanz stellt die
/// Sitzungen **einmal** wieder her (ein Update soll laufende Arbeit nicht beenden). Gilt nur zusammen mit `WAIT_PID_ENV`
/// (also nur für einen echten Update-Neustart) und wird beim Start sofort entfernt – sie bleibt nie hängen: Weder Terminals
/// noch ein späterer Start erben sie, ein Absturz danach führt beim nächsten Start zum gewohnten frischen Start.
pub const KEEP_SESSIONS_ENV: &str = "PANELDECK_UPDATE_KEEP_SESSIONS";

/// Ergebnis von `wait_for_previous_instance`: diese Instanz ist ein Update-Neustart, der die Sitzungen einmal wiederherstellt.
static RESTORE_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Stellt dieser Start die Sitzungen einmal wieder her, obwohl „Sitzungen merken“ aus ist (Update-Neustart, #157)?
pub fn restore_once() -> bool {
    RESTORE_ONCE.load(std::sync::atomic::Ordering::SeqCst)
}

/// Gilt die Einmal-Wiederherstellung? Nur mit beiden Variablen (Update-Neustart) und genau dem Wert `1`.
fn keep_requested(wait_pid: Option<&std::ffi::OsStr>, keep: Option<&std::ffi::OsStr>) -> bool {
    wait_pid.is_some() && keep.and_then(|k| k.to_str()) == Some("1")
}

/// Pfad ohne `\\?\`-Präfix, mit `\`, ohne abschließenden Trenner, klein geschrieben (Windows-Pfade sind nicht case-sensitiv).
#[cfg_attr(target_os = "linux", allow(dead_code))] // Linux: nur die AppImage (`appimage_flavor`)
pub fn norm_dir(p: &str) -> String {
    let s = p.trim().trim_matches('"').replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.trim_end_matches('\\').to_lowercase()
}

/// Installiert oder portabel? Installiert = der Uninstall-Eintrag des Installers (HKCU) zeigt auf genau den Ordner dieser .exe
/// und dort liegt `Uninstall.exe`. Alles andere (auch eine portable Kopie neben einer Installation) ist portabel.
#[cfg_attr(target_os = "linux", allow(dead_code))] // Linux: nur die AppImage (`appimage_flavor`)
pub fn flavor_for(exe: &Path, install_location: Option<&str>) -> Flavor {
    let Some(dir) = exe.parent() else { return Flavor::Portable };
    match install_location {
        Some(loc) if !loc.trim().is_empty() && norm_dir(loc) == norm_dir(&dir.to_string_lossy()) && dir.join("Uninstall.exe").is_file() => Flavor::Installed,
        _ => Flavor::Portable,
    }
}

/// Installationsart dieser Instanz (`None`, wo es kein Selbst-Update gibt: `platform::SELF_UPDATE`, Windows; Linux nur als
/// AppImage, #149 – .deb/.rpm/Tarball melden eine neue Version nur).
pub fn detect(exe: &Path) -> Option<Flavor> {
    #[cfg(target_os = "linux")]
    {
        appimage_flavor(exe, std::env::var_os("APPIMAGE").as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::platform::SELF_UPDATE.then(|| flavor_for(exe, crate::platform::registry::install_location().as_deref()))
    }
}

/// Linux: läuft diese Instanz als AppImage (`$APPIMAGE` = genau diese Datei, beschreibbar)?
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn appimage_flavor(exe: &Path, appimage: Option<&std::ffi::OsStr>) -> Option<Flavor> {
    let a = Path::new(appimage?);
    (a == exe && exe.is_file()).then_some(Flavor::AppImage)
}

/// Ist das eine AppImage (ELF mit der Kennung `AI\x02` ab Byte 8)?
pub fn is_appimage(b: &[u8]) -> bool {
    b.len() > 4096 && b.starts_with(b"\x7fELF") && b.get(8..11) == Some(b"AI\x02")
}

/// Die neue AppImage an den Platz der laufenden setzen (die alte bleibt als `<datei>.old`, bis die neue Version gestartet
/// ist) und ausführbar machen. Die laufende Instanz läuft aus ihrem eingehängten Abbild weiter.
pub fn swap_appimage(file: &Path, new_bytes: &[u8]) -> Result<(), String> {
    swap_exe(file, new_bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}

/// `<p><ext>`
fn suffixed(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(ext);
    PathBuf::from(s)
}

/// `<p>.old` freimachen: löschen, und wenn das nicht geht (die alte Datei läuft noch, z. B. eine OpenConsole.exe einer
/// Sitzung), unter einem freien Namen `<p>.old.<n>` beiseitelegen. `remove_old_exe` räumt später alle `.old*` auf.
fn clear_old(p: &Path) -> Result<(), String> {
    let old = suffixed(p, ".old");
    if !old.exists() || std::fs::remove_file(&old).is_ok() {
        return Ok(());
    }
    for n in 1..1000 {
        let alt = suffixed(p, &format!(".old.{n}"));
        if !alt.exists() {
            return std::fs::rename(&old, &alt).map_err(|e| format!("{}: {e}", old.display()));
        }
    }
    Err(format!("{}: {}", old.display(), crate::state::t("kein freier Name", "no free name")))
}

/// Mehrere Dateien gemeinsam ersetzen (alles oder nichts): erst alle `<p>.new` schreiben, dann je Datei die bisherige in
/// `<p>.old` umbenennen (Windows erlaubt das bei laufenden Programmen und geladenen DLLs) und die neue an ihren Platz.
/// Schlägt ein Schritt fehl, werden alle schon getauschten Dateien zurückgerollt. Fehlt eine Datei noch (portable
/// Installation vor 2.3.0 ohne ConPTY), wird sie neu angelegt. `<p>.old` löscht erst die neue Version, nachdem sie ihren
/// ersten Frame gezeichnet hat (`remove_old_exe`).
pub fn swap_files(items: &[(PathBuf, Vec<u8>)]) -> Result<(), String> {
    let news: Vec<PathBuf> = items.iter().map(|(p, _)| suffixed(p, ".new")).collect();
    let drop_news = || {
        for n in &news {
            let _ = std::fs::remove_file(n);
        }
    };
    for ((_, bytes), new) in items.iter().zip(&news) {
        let _ = std::fs::remove_file(new);
        if let Err(e) = std::fs::write(new, bytes) {
            drop_news();
            return Err(format!("{}: {e}", new.display()));
        }
    }
    for (p, _) in items {
        if let Err(e) = clear_old(p) {
            drop_news();
            return Err(e);
        }
    }
    // (Pfad, hatte eine alte Fassung) der schon getauschten Dateien – für das Zurückrollen.
    let mut done: Vec<(&Path, bool)> = Vec::new();
    let rollback = |done: &[(&Path, bool)]| {
        for (p, had_old) in done.iter().rev() {
            let _ = std::fs::remove_file(p);
            if *had_old {
                let _ = std::fs::rename(suffixed(p, ".old"), p);
            }
        }
    };
    for ((p, _), new) in items.iter().zip(&news) {
        let had_old = p.exists();
        if had_old {
            if let Err(e) = std::fs::rename(p, suffixed(p, ".old")) {
                rollback(&done);
                drop_news();
                return Err(format!("{}: {e}", p.display()));
            }
        }
        if let Err(e) = std::fs::rename(new, p) {
            if had_old {
                let _ = std::fs::rename(suffixed(p, ".old"), p);
            }
            rollback(&done);
            drop_news();
            return Err(format!("{}: {e}", p.display()));
        }
        done.push((p.as_path(), had_old));
    }
    Ok(())
}

/// Neue Programmdatei an den Platz der laufenden setzen (`swap_files` mit einer Datei); die bisherige muss es geben.
pub fn swap_exe(exe: &Path, new_bytes: &[u8]) -> Result<(), String> {
    if !exe.is_file() {
        return Err(format!("{}: {}", exe.display(), crate::state::t("nicht gefunden", "not found")));
    }
    swap_files(&[(exe.to_path_buf(), new_bytes.to_vec())])
}

/// Neben `paneldeck.exe` mitgelieferte Programmdateien (ConPTY, #183): das portable Update ersetzt sie gemeinsam mit
/// (`swap_files`), alte Fassungen bleiben als `<name>.old` liegen, bis die neue Version gestartet ist.
pub const COMPANIONS: [&str; 2] = [crate::platform::conpty::DLL, crate::platform::conpty::HOST_EXE];

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
    crate::platform::fs::deny_write_share(&mut o);
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
    crate::platform::process::raw_arg(&mut cmd, d);
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
    // `<name>.old` und beiseitegelegte `<name>.old.<n>` der .exe und der mitgelieferten Dateien; was noch läuft (eine
    // OpenConsole.exe einer alten Sitzung), bleibt bis zum nächsten Start liegen.
    let Some(dir) = exe.parent() else { return };
    let exe_name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let names: Vec<String> = std::iter::once(exe_name).chain(COMPANIONS.iter().map(|s| s.to_string())).collect();
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for f in rd.flatten() {
        let file = f.file_name().to_string_lossy().into_owned();
        if names.iter().any(|n| is_old_of(&file, n)) {
            let _ = std::fs::remove_file(f.path());
        }
    }
}

/// `file` ist `<name>.old` oder `<name>.old.<zahl>` (Groß-/Kleinschreibung egal wie unter Windows).
fn is_old_of(file: &str, name: &str) -> bool {
    let (f, n) = (file.to_ascii_lowercase(), format!("{}.old", name.to_ascii_lowercase()));
    match f.strip_prefix(&n) {
        Some("") => true,
        Some(rest) => rest.strip_prefix('.').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// Startet `exe` neu mit denselben Instanz-Parametern (`--data-dir`/`--profile` aus `paths::instance_args`, als einzelne
/// Argumente). Nie `--open`, `--claude` oder `mcp`: der Neustart soll nur dieselbe Instanz öffnen. Die neue Instanz wartet
/// über `WAIT_PID_ENV`, bis dieser Prozess beendet ist. `keep_sessions`: die neue Instanz stellt die Sitzungen einmal wieder
/// her, obwohl „Sitzungen merken“ aus ist (`KEEP_SESSIONS_ENV`, #157).
pub fn spawn_restart(exe: &Path, keep_sessions: bool) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(restart_args(crate::paths::instance_args())).env(WAIT_PID_ENV, std::process::id().to_string());
    if keep_sessions {
        cmd.env(KEEP_SESSIONS_ENV, "1");
    } else {
        cmd.env_remove(KEEP_SESSIONS_ENV);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("{}: {e}", exe.display()))
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
/// die noch laufende alte und beendete sich. Merkt sich `KEEP_SESSIONS_ENV` ([`restore_once`]). Danach beide Variablen
/// entfernen (Terminals und spätere Neustarts sollen sie nicht erben).
pub fn wait_for_previous_instance() {
    let (wait, keep) = (std::env::var_os(WAIT_PID_ENV), std::env::var_os(KEEP_SESSIONS_ENV));
    RESTORE_ONCE.store(keep_requested(wait.as_deref(), keep.as_deref()), std::sync::atomic::Ordering::SeqCst);
    if keep.is_some() {
        // SAFETY: wird früh in `main` aufgerufen, bevor weitere Fäden laufen.
        unsafe { std::env::remove_var(KEEP_SESSIONS_ENV) };
    }
    let Some(v) = wait else { return };
    let pid = v.to_str().and_then(|s| s.parse::<u32>().ok()).filter(|p| *p != std::process::id());
    if let Some(pid) = pid {
        wait_pid(pid);
    }
    // SAFETY: wie oben.
    unsafe { std::env::remove_var(WAIT_PID_ENV) };
}

fn wait_pid(pid: u32) {
    crate::platform::process::wait_for_exit(pid, 30_000);
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
        // Mitgelieferte Dateien: `.old` weg, die aktuellen bleiben.
        for n in COMPANIONS {
            std::fs::write(d.join(n), b"MZ new").unwrap();
            std::fs::write(d.join(format!("{n}.old")), b"MZ old").unwrap();
        }
        remove_old_exe(&exe);
        for n in COMPANIONS {
            assert!(d.join(n).exists() && !d.join(format!("{n}.old")).exists(), "{n}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_files_creates_swaps_and_rolls_back() {
        let d = tmp("swapfiles");
        let (a, b) = (d.join("conpty.dll"), d.join("OpenConsole.exe"));
        // Noch nicht vorhanden (portable Installation vor 2.3.0): neu angelegt, kein .old.
        swap_files(&[(a.clone(), b"MZ a1".to_vec()), (b.clone(), b"MZ b1".to_vec())]).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"MZ a1");
        assert_eq!(std::fs::read(&b).unwrap(), b"MZ b1");
        assert!(!d.join("conpty.dll.old").exists() && !d.join("conpty.dll.new").exists());
        // Vorhanden: beide getauscht, alte als .old.
        swap_files(&[(a.clone(), b"MZ a2".to_vec()), (b.clone(), b"MZ b2".to_vec())]).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"MZ a2");
        assert_eq!(std::fs::read(d.join("OpenConsole.exe.old")).unwrap(), b"MZ b1");
        // Zweite Datei scheitert (offen ohne FILE_SHARE_DELETE: nicht umbenennbar): die erste wird zurückgerollt.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let _hold = std::fs::OpenOptions::new().read(true).share_mode(1 | 2).open(&b).unwrap();
            assert!(swap_files(&[(a.clone(), b"MZ a3".to_vec()), (b.clone(), b"MZ b3".to_vec())]).is_err());
            assert_eq!(std::fs::read(&a).unwrap(), b"MZ a2", "zurückgerollt");
            assert!(!d.join("conpty.dll.new").exists() && !d.join("OpenConsole.exe.new").exists());
        }
        assert_eq!(std::fs::read(&b).unwrap(), b"MZ b2");
        assert!(swap_files(&[(d.join("missing").join("conpty.dll"), b"x".to_vec())]).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Eine noch laufende alte Fassung (`.old` ist ein laufendes Programm: nicht löschbar, aber umbenennbar) wird als
    /// `.old.<n>` beiseitegelegt; `remove_old_exe` räumt alle `.old*` auf, sobald sie nicht mehr laufen.
    #[cfg(windows)]
    #[test]
    fn busy_old_is_moved_aside_and_cleaned_up() {
        let d = tmp("busyold");
        let f = d.join("OpenConsole.exe");
        std::fs::write(&f, b"MZ cur").unwrap();
        std::fs::write(d.join("OpenConsole.exe.old.1"), b"MZ older").unwrap();
        let old = d.join("OpenConsole.exe.old");
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        std::fs::copy(sys.join("System32").join("cmd.exe"), &old).unwrap();
        let mut running = std::process::Command::new(&old)
            .args(["/c", "ping", "-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::fs::remove_file(&old).is_err(), "laufendes Programm lässt sich nicht löschen");
        let res = swap_files(&[(f.clone(), b"MZ new".to_vec())]);
        let aside = d.join("OpenConsole.exe.old.2");
        let aside_ok = aside.is_file();
        let _ = running.kill();
        let _ = running.wait();
        res.unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"MZ new");
        assert_eq!(std::fs::read(d.join("OpenConsole.exe.old")).unwrap(), b"MZ cur");
        assert!(aside_ok, "unter freiem Namen beiseitegelegt (.old.1 war belegt)");
        // Aufräumen: alle .old* der .exe und der mitgelieferten Dateien, anderes bleibt.
        std::fs::write(d.join("OpenConsole.exe.older"), b"keep").unwrap();
        std::fs::write(d.join("paneldeck.exe"), b"MZ").unwrap();
        std::fs::write(d.join("paneldeck.exe.old.3"), b"MZ").unwrap();
        let t0 = std::time::Instant::now();
        loop {
            remove_old_exe(&d.join("paneldeck.exe"));
            if !aside.exists() || t0.elapsed() > std::time::Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let mut left: Vec<String> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["OpenConsole.exe", "OpenConsole.exe.older", "paneldeck.exe"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_names() {
        assert!(is_old_of("paneldeck.exe.old", "paneldeck.exe"));
        assert!(is_old_of("conpty.dll.old.12", "conpty.dll"));
        assert!(is_old_of("OPENCONSOLE.EXE.OLD.3", "OpenConsole.exe"));
        assert!(!is_old_of("conpty.dll.old.", "conpty.dll"));
        assert!(!is_old_of("conpty.dll.older", "conpty.dll"));
        assert!(!is_old_of("conpty.dll", "conpty.dll"));
        assert!(!is_old_of("x.conpty.dll.old", "conpty.dll"));
    }

    #[test]
    fn wait_env_is_consumed() {
        // Eigene PID: sofort zurück, Variable entfernt
        // SAFETY: Test setzt nur hier gelesene Variablen (einziger Test, der sie anfasst).
        unsafe { std::env::set_var(WAIT_PID_ENV, std::process::id().to_string()) };
        wait_for_previous_instance();
        assert!(std::env::var_os(WAIT_PID_ENV).is_none());
        assert!(!restore_once());
        wait_for_previous_instance(); // ohne Variable: nichts
        // Update-Neustart mit „Sitzungen merken“ aus: einmal wiederherstellen, beide Variablen sind danach weg (#157).
        unsafe {
            std::env::set_var(WAIT_PID_ENV, std::process::id().to_string());
            std::env::set_var(KEEP_SESSIONS_ENV, "1");
        }
        wait_for_previous_instance();
        assert!(restore_once());
        assert!(std::env::var_os(KEEP_SESSIONS_ENV).is_none() && std::env::var_os(WAIT_PID_ENV).is_none(), "nichts bleibt hängen");
        // Nur die Merk-Variable ohne Update-Neustart (z. B. von Hand gesetzt): gilt nicht, wird aber entfernt.
        unsafe { std::env::set_var(KEEP_SESSIONS_ENV, "1") };
        wait_for_previous_instance();
        assert!(!restore_once());
        assert!(std::env::var_os(KEEP_SESSIONS_ENV).is_none());
    }

    #[test]
    fn keep_sessions_needs_an_update_restart() {
        use std::ffi::OsStr;
        assert!(keep_requested(Some(OsStr::new("42")), Some(OsStr::new("1"))));
        assert!(!keep_requested(None, Some(OsStr::new("1"))), "ohne Update-Neustart nie");
        assert!(!keep_requested(Some(OsStr::new("42")), None));
        assert!(!keep_requested(Some(OsStr::new("42")), Some(OsStr::new("0"))));
        assert!(!keep_requested(Some(OsStr::new("42")), Some(OsStr::new("yes"))));
    }
}

/// Linux-AppImage (#149): Erkennung, Prüfung der Datei, Tausch mit Ausführungsrecht.
#[cfg(all(test, unix))]
mod appimage_tests {
    use super::*;

    #[test]
    fn appimage_detection_check_and_swap() {
        let d = std::env::temp_dir().join(format!("pd-appimage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join("Paneldeck-2.5.0-x86_64.AppImage");
        let mut old = b"\x7fELF\x02\x01\x01\x00AI\x02".to_vec();
        old.resize(8192, 0);
        std::fs::write(&file, &old).unwrap();
        assert_eq!(appimage_flavor(&file, Some(file.as_os_str())), Some(Flavor::AppImage));
        assert_eq!(appimage_flavor(&file, None), None, "no $APPIMAGE: a package, not an AppImage");
        assert_eq!(appimage_flavor(&file, Some(std::ffi::OsStr::new("/elsewhere.AppImage"))), None);
        assert!(is_appimage(&old));
        assert!(!is_appimage(b"\x7fELF\x02\x01\x01\x00\x00\x00\x00"), "plain ELF");
        assert!(!is_appimage(&[0u8; 8192]));
        let mut new = old.clone();
        new[100] = 7;
        swap_appimage(&file, &new).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), new);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(suffixed(&file, ".old")).unwrap(), old, "the old one stays until the new version runs");
        let _ = std::fs::remove_dir_all(&d);
    }
}
