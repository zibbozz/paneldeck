//! Processes: our own command line, the process tree, other processes (alive? wait for exit), and how child processes
//! are started (hidden console window, raw arguments, elevated rights, the `claude` command).

use std::process::Command;

/// The raw command line of this process, unsplit (`cli::split_command_line` splits it, so that `"C:\"` and paths with
/// spaces after a drive root arrive correctly). `None` where the system only hands over split arguments.
pub fn raw_command_line() -> Option<String> {
    #[cfg(windows)]
    {
        // SAFETY: GetCommandLineW liefert einen nullterminierten Zeiger, der für die Prozesslaufzeit gültig ist.
        let p = unsafe { windows_sys::Win32::System::Environment::GetCommandLineW() };
        if p.is_null() {
            return None;
        }
        let mut n = 0usize;
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        Some(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) }))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Start the child without a console window of its own (Windows: `CREATE_NO_WINDOW`). No effect elsewhere.
pub fn hide_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000)
    }
    #[cfg(not(windows))]
    {
        cmd
    }
}

/// Append an argument verbatim, without the quoting of the standard library (Windows: `raw_arg`, e.g. NSIS `/D=` must
/// be the last argument and unquoted). Elsewhere arguments are passed separately anyway: a normal `arg`.
pub fn raw_arg(cmd: &mut Command, arg: String) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(arg)
    }
    #[cfg(not(windows))]
    {
        cmd.arg(arg)
    }
}

/// PID und alle Vorfahren (Eltern, Großeltern …) eines Prozesses, nächster zuerst. Dient dazu, eine MCP-Brücke
/// einer Hub-Sitzung zuzuordnen: Läuft ein Agent in einem Hub-Terminal, ist die Shell der Sitzung ein Vorfahre der Brücke.
/// Unter Windows über einen Prozess-Schnappschuss; ein wiederverwendeter Eltern-PID kann theoretisch täuschen
/// (deshalb höchstens 40 Stufen und Abbruch bei Wiederholung). Ohne Umsetzung nur die PID selbst.
#[cfg(windows)]
pub fn process_chain(pid: u32) -> Vec<u32> {
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut parent: HashMap<u32, u32> = HashMap::new();
    // SAFETY: `pe` ist ein lokal genullter PROCESSENTRY32W mit gesetztem `dwSize` und wird nur an die ToolHelp-Aufrufe
    // übergeben; das Snapshot-Handle wird auf jedem Pfad nach der Prüfung (`INVALID_HANDLE_VALUE`) geschlossen.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return vec![pid];
        }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                parent.insert(pe.th32ProcessID, pe.th32ParentProcessID);
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    let mut chain = vec![pid];
    let mut cur = pid;
    for _ in 0..40 {
        match parent.get(&cur).copied() {
            Some(p) if p != 0 && !chain.contains(&p) => {
                chain.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    chain
}

#[cfg(not(windows))]
pub fn process_chain(pid: u32) -> Vec<u32> {
    vec![pid]
}

/// Läuft der (fremde) Prozess `pid` noch? Im Zweifel (z. B. kein Zugriff) ja.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: OpenProcess nur mit Abfragerecht; das Handle wird geschlossen. GetLastError liest nur den Fehlerwert.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                // ERROR_INVALID_PARAMETER = keine solche PID (mehr)
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            !ok || code == STILL_ACTIVE
        }
    }
    #[cfg(not(windows))]
    {
        if std::path::Path::new("/proc/self").exists() {
            return std::path::Path::new(&format!("/proc/{pid}")).exists();
        }
        // Ohne /proc (macOS): Signal 0 prüft nur, ob es den Prozess gibt; kann `kill` nicht laufen, im Zweifel ja.
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map_or(true, |s| s.success())
    }
}

/// Warten, bis der Prozess `pid` beendet ist, höchstens `timeout_ms`. Ohne Umsetzung: sofort zurück.
pub fn wait_for_exit(pid: u32, timeout_ms: u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        // SAFETY: OpenProcess mit reinem Warte-Recht; das Handle wird geschlossen.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if !h.is_null() {
                WaitForSingleObject(h, timeout_ms);
                CloseHandle(h);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (pid, timeout_ms);
}

/// Start `program args` with elevated rights in a window of its own (Windows: PowerShell `Start-Process -Verb RunAs`,
/// UAC prompt). `Some(started)`, or `None` where this is not supported.
pub fn spawn_elevated(program: &str, args: &[String], cwd: &str) -> Option<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let q = |s: &str| s.replace('\'', "''");
        let mut script = format!("Start-Process -FilePath '{}' -Verb RunAs", q(program));
        if !args.is_empty() {
            script += &format!(" -ArgumentList '{}'", q(&args.join(" ")));
        }
        if std::path::Path::new(cwd).is_dir() {
            script += &format!(" -WorkingDirectory '{}'", q(cwd));
        }
        let ok = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(0x0800_0000)
            .spawn()
            .is_ok();
        Some(ok)
    }
    #[cfg(not(windows))]
    {
        let _ = (program, args, cwd);
        None
    }
}

/// `claude <args>` als Prozess vorbereiten. Unter Windows über `cmd.exe /d /v:off /c` (kein AutoRun, keine verzögerte
/// Erweiterung; `claude` kann eine .cmd sein) mit selbst quotierter Zeile (`launch::claude_cmd_line`); sonst direkt.
pub fn claude_command(args: &[String], cwd: &std::path::Path) -> Result<Command, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let line = crate::launch::claude_cmd_line(args)?;
        let mut c = std::process::Command::new("cmd.exe");
        c.raw_arg(line).current_dir(cwd).creation_flags(0x0800_0000);
        Ok(c)
    }
    #[cfg(not(windows))]
    {
        for a in args {
            crate::launch::check_arg(a)?;
        }
        let mut c = std::process::Command::new("claude");
        c.args(args).current_dir(cwd);
        Ok(c)
    }
}
