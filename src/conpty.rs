//! Konsolen-Host der Terminals (ConPTY) und sichere DLL-Suche (#183).
//!
//! - `harden_dll_search` steht ganz am Anfang von `main`: DLLs, die per Name geladen werden, kommen danach nur noch aus
//!   dem Programmordner und `System32` – nie aus dem Arbeitsordner (z. B. „In Paneldeck öffnen“ auf einem fremden Ordner).
//! - Neben `paneldeck.exe` liegt eine neuere ConPTY (`conpty.dll` + `OpenConsole.exe` aus dem NuGet-Paket
//!   `Microsoft.Windows.Console.ConPTY`, MIT; `tools/fetch-conpty.ps1`). Sie reicht Abfragen wie OSC 10/11 und die
//!   synchronisierte Ausgabe (Modus 2026) in der richtigen Reihenfolge durch. Die Einstellung `bundled_conpty` (Standard
//!   an) wählt sie für neue Sitzungen; fehlt eine der Dateien oder schlägt sie fehl, läuft die Sitzung auf der ConPTY von
//!   Windows (kernel32). Die Auswahl je Sitzung macht der kleine Patch in `vendor/portable-pty` möglich.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Einstellung „Mitgelieferten Konsolen-Host verwenden“ (`Settings::bundled_conpty`); vom UI-Thread beim Start und
/// jedes Frame gesetzt wie `mcp::LEVEL`, gelesen beim Start einer Sitzung.
pub static USE_BUNDLED: AtomicBool = AtomicBool::new(true);

/// Dateinamen der mitgelieferten ConPTY (neben `paneldeck.exe`).
pub const DLL: &str = "conpty.dll";
pub const HOST_EXE: &str = "OpenConsole.exe";

/// DLL-Suche des Prozesses einschränken: Programmordner, `System32` und mit `AddDllDirectory` ausdrücklich
/// hinzugefügte Ordner (`LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`) – nicht mehr der Arbeitsordner, `PATH` oder der
/// Windows-Ordner. Muss vor allem anderen laufen (danach geladene DLLs, auch verzögert geladene, folgen der Regel).
/// Gilt nur für diesen Prozess, nicht für Shells und Programme in den Terminals.
pub fn harden_dll_search() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, SetDefaultDllDirectories};
        // SAFETY: einfacher Win32-Aufruf ohne Zeiger; ein Fehler (sehr alte Systeme) lässt die Standardsuche bestehen.
        unsafe {
            SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        }
    }
}

/// `conpty.dll` in `dir`, wenn dort beide Dateien liegen (`conpty.dll` und `OpenConsole.exe`); sonst `None`.
pub fn bundled_dll_in(dir: &Path) -> Option<PathBuf> {
    let dll = dir.join(DLL);
    (dll.is_file() && dir.join(HOST_EXE).is_file()).then_some(dll)
}

/// Ordner der laufenden Programmdatei.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Mitgelieferte `conpty.dll` neben `paneldeck.exe` (absoluter Pfad), wenn vollständig vorhanden.
pub fn bundled_dll() -> Option<PathBuf> {
    exe_dir().and_then(|d| bundled_dll_in(&d))
}

/// `conpty.dll` für die nächste Sitzung: die mitgelieferte, wenn eingeschaltet und vorhanden; `None` = ConPTY von Windows.
pub fn selected_dll(use_bundled: bool, found: Option<PathBuf>) -> Option<PathBuf> {
    if use_bundled { found } else { None }
}

/// Pty-System für eine neue Sitzung (Windows: mitgelieferte oder System-ConPTY nach `USE_BUNDLED`).
pub fn pty_system() -> Box<dyn portable_pty::PtySystem + Send> {
    #[cfg(windows)]
    {
        let dll = selected_dll(USE_BUNDLED.load(std::sync::atomic::Ordering::Relaxed), bundled_dll());
        Box::new(portable_pty::win::conpty::ConPtySystem::with_sideloaded_conpty(dll))
    }
    #[cfg(not(windows))]
    {
        portable_pty::native_pty_system()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("paneldeck-conpty-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn bundled_needs_both_files() {
        let d = tmp("both");
        assert_eq!(bundled_dll_in(&d), None);
        std::fs::write(d.join(DLL), b"x").unwrap();
        assert_eq!(bundled_dll_in(&d), None, "OpenConsole.exe fehlt");
        std::fs::write(d.join(HOST_EXE), b"x").unwrap();
        assert_eq!(bundled_dll_in(&d), Some(d.join(DLL)));
        std::fs::remove_file(d.join(DLL)).unwrap();
        assert_eq!(bundled_dll_in(&d), None, "conpty.dll fehlt");
        // Ein Ordner mit dem Namen zählt nicht.
        std::fs::create_dir(d.join(DLL)).unwrap();
        assert_eq!(bundled_dll_in(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn selection_follows_setting() {
        let p = PathBuf::from(r"C:\x\conpty.dll");
        assert_eq!(selected_dll(true, Some(p.clone())), Some(p.clone()));
        assert_eq!(selected_dll(false, Some(p)), None);
        assert_eq!(selected_dll(true, None), None);
    }

    /// Ohne mitgelieferte Dateien neben der Test-.exe läuft eine Sitzung auf der ConPTY von Windows; mit
    /// `PANELDECK_TEST_CONPTY_DIR` (Ordner mit conpty.dll + OpenConsole.exe) zusätzlich auf der mitgelieferten.
    #[cfg(windows)]
    #[test]
    fn pty_runs_on_selected_host() {
        use portable_pty::win::ConPtyHost;
        use portable_pty::win::conpty::{ConPtyMasterPty, ConPtySystem};
        use portable_pty::{CommandBuilder, PtySize, PtySystem};
        let run = |sys: ConPtySystem| -> ConPtyHost {
            let pair = sys.openpty(PtySize { rows: 10, cols: 40, pixel_width: 0, pixel_height: 0 }).unwrap();
            let mut cmd = CommandBuilder::new("cmd.exe");
            cmd.args(["/c", "exit", "3"]);
            let mut child = pair.slave.spawn_command(cmd).unwrap();
            drop(pair.slave);
            let mut reader = pair.master.try_clone_reader().unwrap();
            let mut writer = pair.master.take_writer().unwrap();
            // Mit PSEUDOCONSOLE_INHERIT_CURSOR wartet der Konsolen-Host auf die Antwort zur Cursorposition (ESC[6n).
            std::thread::spawn(move || {
                let mut b = [0u8; 4096];
                while let Ok(n) = std::io::Read::read(&mut reader, &mut b) {
                    if n == 0 {
                        break;
                    }
                    if b[..n].windows(4).any(|w| w == b"\x1b[6n") {
                        let _ = std::io::Write::write_all(&mut writer, b"\x1b[1;1R");
                    }
                }
            });
            let status = child.wait().unwrap();
            assert_eq!(status.exit_code(), 3);
            let master: &dyn portable_pty::MasterPty = &*pair.master;
            master.downcast_ref::<ConPtyMasterPty>().unwrap().host()
        };
        assert_eq!(run(ConPtySystem::default()), ConPtyHost::System);
        // Nicht ladbare „conpty.dll“: Rückfall auf die ConPTY von Windows statt eines Fehlers.
        let d = tmp("bad");
        std::fs::write(d.join(DLL), b"not a dll").unwrap();
        std::fs::write(d.join(HOST_EXE), b"not an exe").unwrap();
        assert_eq!(run(ConPtySystem::with_sideloaded_conpty(Some(d.join(DLL)))), ConPtyHost::System);
        let _ = std::fs::remove_dir_all(&d);
        if let Some(dir) = std::env::var_os("PANELDECK_TEST_CONPTY_DIR") {
            let dll = bundled_dll_in(Path::new(&dir)).expect("PANELDECK_TEST_CONPTY_DIR ohne conpty.dll/OpenConsole.exe");
            assert_eq!(run(ConPtySystem::with_sideloaded_conpty(Some(dll))), ConPtyHost::Sideloaded);
        }
    }
}
