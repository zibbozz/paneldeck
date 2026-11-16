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
    chain_from(pid, parent_of)
}

/// Kette `pid`, Eltern, Großeltern … nach denselben Regeln wie unter Windows: höchstens 40 Stufen, Abbruch bei PID 0,
/// fehlendem Eltern-Eintrag oder Wiederholung.
#[cfg_attr(windows, allow(dead_code))]
fn chain_from(pid: u32, parent: impl Fn(u32) -> Option<u32>) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut cur = pid;
    for _ in 0..40 {
        match parent(cur) {
            Some(p) if p != 0 && !chain.contains(&p) => {
                chain.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    chain
}

/// Eltern-PID aus dem Inhalt von `/proc/<pid>/stat` (Feld 4). Der Programmname (Feld 2, in Klammern) darf Leerzeichen und
/// Klammern enthalten, deshalb wird ab der letzten `)` gezählt.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut f = rest.split_whitespace();
    f.next()?; // Feld 3: Zustand
    f.next()?.parse().ok()
}

/// Linux: Eltern-PID über `/proc`.
#[cfg(target_os = "linux")]
fn parent_of(pid: u32) -> Option<u32> {
    parse_stat_ppid(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// macOS: Eltern-PID über `proc_pidinfo(PROC_PIDTBSDINFO)`.
#[cfg(target_os = "macos")]
fn parent_of(pid: u32) -> Option<u32> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` ist ein lokal genullter proc_bsdinfo; Größe und Zeiger passen zusammen, der Aufruf schreibt höchstens `size` Bytes.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    (n == size).then_some(info.pbi_ppid)
}

/// Andere Systeme: keine Vorfahren bekannt (die Kette ist nur die PID selbst).
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn parent_of(_pid: u32) -> Option<u32> {
    None
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
    #[cfg(unix)]
    {
        // PID 0 wäre bei kill() die eigene Prozessgruppe, negative Werte ganze Gruppen: nie „lebt“.
        let Ok(p) = libc::pid_t::try_from(pid) else { return false };
        if p <= 0 {
            return false;
        }
        // SAFETY: Signal 0 wird nicht zugestellt; kill prüft nur, ob es den Prozess gibt und ob wir ihn erreichen dürften.
        if unsafe { libc::kill(p, 0) } == 0 {
            return true;
        }
        // EPERM: Prozess existiert, gehört aber jemand anderem.
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_parent_pid_is_field_four() {
        assert_eq!(parse_stat_ppid("1234 (bash) S 99 1234 1234 34816 0"), Some(99));
        // Programmname mit Leerzeichen und Klammern
        assert_eq!(parse_stat_ppid("77 (a) b (c)) R 5 77 77 0"), Some(5));
        assert_eq!(parse_stat_ppid("garbage"), None);
        assert_eq!(parse_stat_ppid("1 (x) S"), None);
    }

    #[test]
    fn chain_stops_at_zero_repeats_and_depth() {
        let map = |p: u32| -> Option<u32> { [(10, 5), (5, 1), (1, 0)].iter().find(|(c, _)| *c == p).map(|(_, q)| *q) };
        assert_eq!(chain_from(10, map), vec![10, 5, 1]);
        assert_eq!(chain_from(3, |p| Some(if p == 3 { 4 } else { 3 })), vec![3, 4], "Schleife");
        assert_eq!(chain_from(1, |p| Some(p + 1)).len(), 41, "höchstens 40 Stufen");
    }

    /// Echte Prozesse: ein Kind dieses Tests hat diesen Prozess und dessen Eltern als Vorfahren (Grundlage der Zuordnung
    /// einer MCP-Brücke zu ihrer Sitzung).
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn real_child_has_us_as_ancestor() {
        let me = std::process::id();
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/c", "ping", "-n", "10", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap()
        } else {
            std::process::Command::new("sleep").arg("10").spawn().unwrap()
        };
        let chain = process_chain(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(chain.first(), Some(&child.id()));
        assert_eq!(chain.get(1), Some(&me), "{chain:?}");
        #[cfg(unix)]
        assert_eq!(chain.get(2), Some(&std::os::unix::process::parent_id()), "{chain:?}");
    }

    #[test]
    fn alive_checks() {
        assert!(is_alive(std::process::id()));
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/c", "exit", "0"]).spawn().unwrap()
        } else {
            std::process::Command::new("true").spawn().unwrap()
        };
        let pid = child.id();
        child.wait().unwrap();
        // nach dem Einsammeln ist die PID frei (Wiederverwendung so schnell ist praktisch ausgeschlossen)
        assert!(!is_alive(pid));
        #[cfg(unix)]
        {
            assert!(!is_alive(0), "PID 0 nie");
            assert!(is_alive(1), "init/launchd gehört root: EPERM zählt als lebendig");
        }
    }
}
