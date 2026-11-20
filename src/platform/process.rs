//! Processes: the peer of a local TCP connection, the process tree, process names and windows, Job Objects of the
//! sessions (#168), and how child processes are started (hidden window, raw arguments, elevated rights, `claude`).
//!
//! Windows: TCP table, ToolHelp snapshot, Job Objects. Other platforms: stubs – no peer process, no process tree, no Job
//! Objects (see the blocker in the `platform` module docs); "is this PID alive" uses `kill(pid, 0)` on Unix. Real
//! implementations follow in #149/#150.

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

/// Can this platform start a program with elevated rights in a window of its own (`spawn_elevated`)? Windows only;
/// #149 decided that Linux/macOS get no admin/sudo shell.
pub const ELEVATED: bool = cfg!(windows);

/// Run a PowerShell script (`Start-Process … -Verb RunAs`, built by the caller) hidden, which raises the UAC prompt.
/// `false` if PowerShell could not be started or there is no implementation (`ELEVATED`).
pub fn spawn_elevated(script: &str) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script])
            .creation_flags(0x0800_0000)
            .spawn()
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = script;
        false
    }
}

/// `claude <args>` als Prozess vorbereiten. Unter Windows über `cmd.exe /d /v:off /c` (kein AutoRun, keine verzögerte
/// Erweiterung; `claude` kann eine .cmd sein) mit selbst quotierter Zeile (`launch::claude_cmd_line`); sonst direkt.
pub fn claude_command(args: &[String], cwd: &std::path::Path) -> Result<Command, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let line = crate::launch::claude_cmd_line(args)?;
        let mut c = Command::new("cmd.exe");
        c.raw_arg(line).current_dir(cwd).creation_flags(0x0800_0000);
        Ok(c)
    }
    #[cfg(not(windows))]
    {
        for a in args {
            crate::launch::check_arg(a)?;
        }
        let mut c = Command::new("claude");
        c.args(args).current_dir(cwd);
        Ok(c)
    }
}

/// Prozess am anderen Ende einer angenommenen IPv4-Verbindung (#152): `peer` = Adresse des Clients (`peer_addr`),
/// `local` = Adresse des Hubs (`local_addr`). Gesucht wird in der TCP-Tabelle des Systems (`GetExtendedTcpTable`,
/// `TCP_TABLE_OWNER_PID_CONNECTIONS`) die Zeile des Clients: lokale Adresse/Port = `peer`, entfernte Adresse/Port =
/// `local`. So muss der Hub der Prozess-ID, die ein Client selbst angibt, nicht glauben. `None`, wenn die Tabelle nicht
/// lesbar ist, keine Zeile passt oder der Besitzer PID 0 bzw. dieser Prozess ist (nur IPv4 – der Hub lauscht nur auf
/// 127.0.0.1).
///
/// Bewusst **kein** Filter auf den Zustand `ESTABLISHED`: Ein Client, der seine Anfrage geschickt und die Schreibseite
/// geschlossen hat (`shutdown(Write)`, so arbeiten Brücke und `--open`), steht zum Zeitpunkt der Abfrage in `FIN_WAIT_2`.
#[cfg(windows)]
pub fn tcp_peer_pid(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> Option<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_CONNECTIONS};
    let (std::net::SocketAddr::V4(peer), std::net::SocketAddr::V4(local)) = (peer, local) else { return None };
    const AF_INET: u32 = 2;
    const NO_ERROR: u32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    // Die Tabelle kann zwischen Größenabfrage und Abruf wachsen: ein paar Versuche mit Reserve.
    let mut size: u32 = 0;
    let mut buf: Vec<u32> = Vec::new();
    let mut ok = false;
    for _ in 0..4 {
        // SAFETY: `buf` ist (als u32-Vektor passend ausgerichtet) mindestens `size` Bytes groß bzw. beim ersten Aufruf leer
        // mit `size = 0` (dann schreibt die Funktion nur die nötige Größe); `size` ist ein lokaler Ein-/Ausgabewert.
        let r = unsafe { GetExtendedTcpTable(if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr().cast() }, &mut size, 0, AF_INET, TCP_TABLE_OWNER_PID_CONNECTIONS, 0) };
        if r == NO_ERROR && !buf.is_empty() {
            ok = true;
            break;
        }
        if r != ERROR_INSUFFICIENT_BUFFER && !(r == NO_ERROR && buf.is_empty()) {
            return None;
        }
        size = size.checked_add(u32::try_from(16 * row_size).ok()?)?;
        buf = vec![0u32; (size as usize).div_ceil(4)];
    }
    if !ok {
        return None;
    }
    let bytes = buf.len().checked_mul(4)?;
    let n = *buf.first()? as usize;
    if n.checked_mul(row_size)?.checked_add(4)? > bytes {
        return None;
    }
    // Adressen stehen in Netzwerk-Reihenfolge im Speicher (Bytes wie `octets()`), Ports in den unteren 16 Bit, ebenfalls
    // in Netzwerk-Reihenfolge.
    let addr = |ip: std::net::Ipv4Addr| u32::from_ne_bytes(ip.octets());
    let port = |p: u32| u16::from_be(p as u16);
    // SAFETY: Die Zeilen folgen dem Zähler (Offset 4, Ausrichtung 4) und liegen laut Prüfung oben ganz im Puffer.
    let rows: &[MIB_TCPROW_OWNER_PID] = unsafe { std::slice::from_raw_parts(buf.as_ptr().add(1).cast(), n) };
    let pid = rows
        .iter()
        .find(|r| r.dwLocalAddr == addr(*peer.ip()) && port(r.dwLocalPort) == peer.port() && r.dwRemoteAddr == addr(*local.ip()) && port(r.dwRemotePort) == local.port())
        .map(|r| r.dwOwningPid)?;
    // PID 0 = keine Zuordnung (System/Leerlauf). Der Hub selbst ist nie ein Client – außer in den Tests dieses Programms,
    // die den Hub im eigenen Prozess ansprechen.
    if pid == 0 || (pid == std::process::id() && !cfg!(test)) {
        return None;
    }
    Some(pid)
}

/// Linux/macOS: Besitzer des Client-Sockets (`/proc/net/tcp` bzw. libproc, `platform::unix_proc`, #149/#150).
#[cfg(unix)]
#[allow(dead_code)] // nur `resolve_peer` und die Tests rufen sie auf
pub fn tcp_peer_pid(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> Option<u32> {
    super::unix_proc::tcp_peer_pid(peer, local)
}

/// Prozessbaum des Absenders einer Verbindung, auf dem Verbindungs-Thread ermittelt (#152).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeerChain {
    /// Absender und seine geprüften Vorfahren, nächster zuerst (Erzeugungszeit jedes Elternteils ≤ der des Kindes).
    pub chain: Vec<u32>,
    /// Der Baum reißt bei einem Elternteil ab, das es nicht mehr gibt (oder dessen PID neu vergeben ist), und der letzte
    /// geprüfte Prozess ist nach dem Start des Hubs entstanden: ein losgelöster Prozess („verwaist“, z. B. per
    /// `Start-Process`/`cmd /c start` aus einer kurzlebigen Shell). Er gilt nicht als externer Client, sondern als
    /// „nicht zuzuordnen“ und darf nicht in mögliche Vorfahren schreiben.
    pub orphan: bool,
    /// Job Object der Sitzung, in dem der Absender läuft ([`SessionJob::key`], seit 2.3.0, #168). Zuverlässiger als
    /// `chain`: Die Mitgliedschaft übersteht losgelöste und verwaiste Prozesse und neu vergebene Eltern-PIDs. Ist sie
    /// bekannt, wird der Baum nicht mehr abgelaufen (`chain` = nur der Absender).
    pub job: Option<u64>,
}

/// Absender einer angenommenen Verbindung bestimmen – sofort auf dem Verbindungs-Thread, ohne Zeitfenster für
/// Täuschungen (#152): PID aus der TCP-Tabelle, Prozess öffnen (das Handle hält die PID fest), prüfen, dass er noch
/// läuft und die Zeile noch ihm gehört. Dann zuerst über das Job Object einer Sitzung zuordnen (`IsProcessInJob` auf
/// dieses Handle, #168); liegt er in keinem, seinen Baum mit Erzeugungszeiten ablaufen (Rückfallweg, z. B. wenn die
/// Zuweisung an den Job scheiterte). `None` (= abweisen), wenn der Absender schon beendet ist, nicht zu öffnen ist oder
/// seine Zeile verschwunden ist.
#[cfg(windows)]
pub fn resolve_peer(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> Option<PeerChain> {
    let pid = tcp_peer_pid(peer, local)?;
    // Das Handle schließt `Drop` von `Proc` – auch beim frühen Abbruch (2.3.0 ließ es dort offen, #221).
    let h = Proc::open(pid)?;
    if !h.alive() || tcp_peer_pid(peer, local) != Some(pid) {
        return None;
    }
    let job = job_containing(h.0);
    match (job, h.created()) {
        (Some(_), _) => Some(PeerChain { chain: vec![pid], orphan: false, job }),
        (None, Some(born)) => Some(walk_tree(pid, born)),
        (None, None) => None,
    }
}

/// Linux/macOS (#149/#150): Socket-Besitzer, Prozessbaum und Sitzungs-Markierung in der Umgebung
/// (`platform::unix_proc`). Nicht zu ermitteln → `None` (= abweisen), wie unter Windows.
#[cfg(unix)]
pub fn resolve_peer(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> Option<PeerChain> {
    super::unix_proc::resolve_peer(peer, local)
}

/// Ein geöffneter Prozess (nur Abfragerechte). `Drop` schließt das Handle.
#[cfg(windows)]
struct Proc(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for Proc {
    fn drop(&mut self) {
        // SAFETY: gültiges Handle aus `OpenProcess` (`Proc::open` liefert nie ein Nullhandle), genau einmal geschlossen.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(windows)]
impl Proc {
    fn open(pid: u32) -> Option<Proc> {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: reiner Aufruf; ein Nullhandle wird als Fehler behandelt.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        (!h.is_null()).then_some(Proc(h))
    }
    fn alive(&self) -> bool {
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;
        const STILL_ACTIVE: u32 = 259;
        let mut code = 0u32;
        // SAFETY: gültiges Handle, `code` ist ein lokaler Ausgabewert.
        unsafe { GetExitCodeProcess(self.0, &mut code) != 0 && code == STILL_ACTIVE }
    }
    /// Erzeugungszeit (FILETIME als 100-ns-Ticks).
    fn created(&self) -> Option<u64> {
        process_created(self.0)
    }
}

#[cfg(windows)]
fn process_created(h: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    // SAFETY: gültiges Handle; die vier FILETIME sind lokale Ausgabewerte.
    let ok = unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) } != 0;
    ok.then(|| ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
}

/// Erzeugungszeit eines laufenden Prozesses (FILETIME-Ticks), `None`, wenn er nicht zu öffnen ist.
#[cfg(windows)]
pub fn process_born(pid: u32) -> Option<u64> {
    Proc::open(pid)?.created()
}

#[cfg(unix)]
pub fn process_born(pid: u32) -> Option<u64> {
    super::unix_proc::process_born(pid)
}

/// Startzeit dieses Prozesses (des Hubs).
#[cfg(windows)]
fn hub_started() -> u64 {
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    static T: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    // SAFETY: Pseudo-Handle des eigenen Prozesses, muss nicht geschlossen werden.
    *T.get_or_init(|| process_created(unsafe { GetCurrentProcess() }).unwrap_or(0))
}

/// Vorfahren von `pid` (Erzeugungszeit `born`) ablaufen: jedes Elternteil muss noch leben und vor dem Kind entstanden
/// sein (sonst ist die Eltern-PID neu vergeben). Reißt der Baum bei einem nach dem Hub-Start entstandenen Prozess ab,
/// ist der Absender verwaist ([`PeerChain::orphan`]). Ein Elternteil, das sich nicht öffnen lässt (z. B. ein
/// geschützter Systemprozess), beendet den Lauf ohne Befund.
#[cfg(windows)]
fn walk_tree(pid: u32, born: u64) -> PeerChain {
    use std::collections::HashMap;
    let parent: HashMap<u32, u32> = process_parents().into_iter().collect();
    let mut out = PeerChain { chain: vec![pid], orphan: false, job: None };
    let (mut cur, mut cur_born) = (pid, born);
    for _ in 0..40 {
        let Some(&p) = parent.get(&cur) else { break };
        if p == 0 || p == 4 || out.chain.contains(&p) {
            break; // Wurzel (Leerlauf/System) oder Schleife
        }
        let broken = if !parent.contains_key(&p) {
            true // Elternteil beendet
        } else {
            match Proc::open(p) {
                None => break, // nicht zu prüfen (geschützt): Wurzel
                Some(h) => {
                    let t = h.created();
                    drop(h);
                    match t {
                        Some(t) if t <= cur_born => {
                            out.chain.push(p);
                            cur = p;
                            cur_born = t;
                            false
                        }
                        Some(_) => true, // nach dem Kind entstanden: PID neu vergeben
                        None => break,
                    }
                }
            }
        };
        if broken {
            out.orphan = orphaned(cur_born, hub_started());
            break;
        }
    }
    out
}

/// Alle laufenden Prozesse als (PID, Eltern-PID) – eine Momentaufnahme (ToolHelp). Leer, wenn sie nicht zu bekommen ist.
#[cfg(windows)]
fn process_parents() -> Vec<(u32, u32)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut out = Vec::new();
    // SAFETY: `pe` ist ein lokal genullter PROCESSENTRY32W mit gesetztem `dwSize` und wird nur an die ToolHelp-Aufrufe
    // übergeben; das Snapshot-Handle wird nach der Prüfung (`INVALID_HANDLE_VALUE`) geschlossen.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap != INVALID_HANDLE_VALUE {
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut pe) != 0 {
                loop {
                    out.push((pe.th32ProcessID, pe.th32ParentProcessID));
                    if Process32NextW(snap, &mut pe) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
    }
    out
}

/// Job Object einer Sitzung (#168): Der Hub steckt die Shell direkt nach dem Start hinein ([`SessionJob::adopt`]); jeder
/// Nachfahre erbt ihn – auch losgelöste (`Start-Process`, `cmd /c start`), verwaiste und solche, deren Eltern-PID neu
/// vergeben ist. So ordnet [`resolve_peer`] einen Absender seiner Sitzung zu, ohne den Prozessbaum zu brauchen.
///
/// * **Kein Ausbruch:** Der Job hat keine Grenzen, insbesondere weder `JOB_OBJECT_LIMIT_BREAKAWAY_OK` noch
///   `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK`: `CreateProcess` mit `CREATE_BREAKAWAY_FROM_JOB` schlägt für jeden
///   Nachfahren fehl (Zugriff verweigert). Das ist gewollt (Entscheidung des Nutzers zu #168): Ein Ausbruch würde die
///   Zuordnung umgehen; Programme, die ihn erzwingen, laufen in einer Paneldeck-Sitzung nicht.
/// * **Verschachtelt:** Legt ein Programm in der Sitzung eigene Jobs an (ab Windows 8 erlaubt), liegen sie unter diesem;
///   die Mitgliedschaft (`IsProcessInJob`) bleibt. Läuft der Hub selbst in einem Job, liegt dieser Job darunter.
/// * **Kein automatisches Beenden:** Ohne `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Laufen beim Schließen noch Prozesse
///   im Job, die das Schließen überleben ([`lingering`]), fragt der Hub: weiterlaufen lassen oder alle beenden
///   ([`SessionJob::terminate_all`]).
/// * Was außerhalb von Paneldeck startet (Aufgabenplanung, WMI, COM-Server, ein per ShellExecute schon laufender
///   Explorer), liegt in keinem Job; dafür bleibt der Prozessbaum als Rückfallweg. Ebenso WSL: Linux-Prozesse sind
///   keine Windows-Prozesse (dort gilt `PANELDECK_SESSION_KEY`).
///
/// Das Handle liegt in einer prozessweiten Liste (für den Verbindungs-Thread); `Drop` nimmt es heraus und schließt es.
pub struct SessionJob {
    key: u64,
}

/// Die Job-Handles aller Sitzungen: (Kennung, Handle). Unter dieser Sperre wird ein Handle benutzt und geschlossen.
#[cfg(windows)]
fn jobs() -> std::sync::MutexGuard<'static, Vec<(u64, isize)>> {
    static JOBS: std::sync::Mutex<Vec<(u64, isize)>> = std::sync::Mutex::new(Vec::new());
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

impl SessionJob {
    /// Neuen, anonymen Job anlegen. `None`, wenn das nicht geht (dann bleibt nur der Prozessbaum).
    #[cfg(windows)]
    pub fn new() -> Option<SessionJob> {
        use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        // SAFETY: ohne Sicherheitsattribute (Handle nicht vererbbar) und ohne Namen; ein Nullhandle ist ein Fehler.
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return None;
        }
        let key = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        jobs().push((key, h as isize));
        Some(SessionJob { key })
    }

    /// Linux/macOS: kein Job Object, sondern eine Markierung in der Umgebung der Shell ([`SessionJob::env_var`]) plus
    /// der Prozessbaum ab der Shell (`platform::unix_proc`).
    #[cfg(unix)]
    pub fn new() -> Option<SessionJob> {
        super::unix_proc::job_new().map(|key| SessionJob { key })
    }

    /// Variable, die in die Umgebung der Shell gehört (Linux/macOS: die Markierung der Sitzung); Windows: keine.
    pub fn env_var(&self) -> Option<(&'static str, String)> {
        #[cfg(windows)]
        {
            None
        }
        #[cfg(unix)]
        {
            super::unix_proc::job_env(self.key)
        }
    }

    /// Linux/macOS: die gestartete Shell (PID) merken – ihre Nachfahren gehören zur Sitzung.
    #[cfg(unix)]
    pub fn adopt_pid(&self, pid: u32) -> bool {
        super::unix_proc::job_adopt(self.key, pid)
    }

    /// Das Job-Handle (gültig, solange dieser `SessionJob` lebt – erst `Drop` schließt es), z. B. für den Start der
    /// Shell direkt im Job (`portable_pty::win::set_spawn_job`).
    #[cfg(windows)]
    pub fn raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        jobs().iter().find(|(k, _)| *k == self.key).map(|&(_, h)| h as std::os::windows::io::RawHandle)
    }

    /// Kennung für [`PeerChain::job`].
    pub fn key(&self) -> u64 {
        self.key
    }

    /// Den gerade gestarteten Prozess `process` (PID `pid`; Handle mit `PROCESS_SET_QUOTA | PROCESS_TERMINATE`, wie es
    /// `CreateProcess` liefert) in den Job stecken. portable-pty startet die Shell nicht angehalten; zwischen
    /// `CreateProcess` und dieser Zuweisung liegen Mikrosekunden, in denen eine Shell praktisch nie etwas startet. Falls
    /// doch, holt [`sweep`] solche Nachfahren nach. `false`, wenn die Zuweisung scheitert (z. B. verbietet ein Job, in
    /// dem der Hub selbst läuft, die Verschachtelung) – dann gilt der Prozessbaum.
    #[cfg(windows)]
    pub fn adopt(&self, process: std::os::windows::io::RawHandle, pid: u32) -> bool {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        let g = jobs();
        let Some(&(_, jh)) = g.iter().find(|(k, _)| *k == self.key) else { return false };
        let (jh, ph) = (jh as HANDLE, process as HANDLE);
        // SAFETY: gültiges Job-Handle (unter der Sperre, erst `Drop` schließt es) und das Prozess-Handle des Aufrufers.
        if unsafe { AssignProcessToJobObject(jh, ph) } == 0 {
            return false;
        }
        if let Some(born) = process_created(ph) {
            sweep(jh, pid, born);
        }
        true
    }

    /// Alle Prozesse des Jobs beenden („Alle beenden“ im Schließen-Dialog, #168). `true`, wenn der Aufruf gelang.
    #[cfg(windows)]
    pub fn terminate_all(&self) -> bool {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        jobs().iter().find(|(k, _)| *k == self.key).is_some_and(|&(_, jh)| {
            // SAFETY: gültiges Job-Handle unter der Sperre.
            unsafe { TerminateJobObject(jh as windows_sys::Win32::Foundation::HANDLE, 1) != 0 }
        })
    }

    #[cfg(unix)]
    pub fn terminate_all(&self) -> bool {
        super::unix_proc::job_terminate_all(self.key)
    }

    /// Alle Prozesse des Jobs enden, sobald sein Handle geschlossen wird (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) – auch
    /// wenn der Hub abstürzt. Für die Prozessliste (#243): ihre Programme enden mit Paneldeck. Sitzungs-Jobs bekommen
    /// das nie (dort entscheidet der Nutzer, siehe oben). `true`, wenn die Grenze gesetzt ist.
    #[cfg(windows)]
    pub fn kill_on_close(&self) -> bool {
        use windows_sys::Win32::System::JobObjects::{JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
        jobs().iter().find(|(k, _)| *k == self.key).is_some_and(|&(_, jh)| {
            // SAFETY: genullte Struktur mit nur dem einen Grenz-Flag; gültiges Job-Handle unter der Sperre.
            unsafe {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(jh as windows_sys::Win32::Foundation::HANDLE, JobObjectExtendedLimitInformation, (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) != 0
            }
        })
    }

    #[cfg(not(windows))]
    pub fn kill_on_close(&self) -> bool {
        false
    }

    /// Nur diese Prozesse beenden ((PID, Erzeugungszeit), z. B. die in einer Meldung genannten): jeden nur, wenn er
    /// noch in diesem Job läuft und dieselbe Erzeugungszeit hat (die PID also nicht neu vergeben ist). Anzahl beendeter.
    #[cfg(windows)]
    pub fn terminate_listed(&self, procs: &[(u32, u64)]) -> usize {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
        let g = jobs();
        let Some(&(_, jh)) = g.iter().find(|(k, _)| *k == self.key) else { return 0 };
        let mut n = 0;
        for &(pid, born) in procs {
            // SAFETY: reiner Aufruf; ein Nullhandle wird übersprungen, ein geöffnetes genau einmal geschlossen; das
            // Job-Handle ist unter der Sperre gültig, `r` ein lokaler Ausgabewert.
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE, 0, pid);
                if h.is_null() {
                    continue;
                }
                let mut r = 0;
                if IsProcessInJob(h, jh as windows_sys::Win32::Foundation::HANDLE, &mut r) != 0 && r != 0 && process_created(h) == Some(born) && TerminateProcess(h, 1) != 0 {
                    n += 1;
                }
                CloseHandle(h);
            }
        }
        n
    }

    #[cfg(unix)]
    pub fn terminate_listed(&self, procs: &[(u32, u64)]) -> usize {
        super::unix_proc::job_terminate_listed(self.key, procs)
    }

    /// PIDs aller laufenden Prozesse im Job (auch in darunter verschachtelten Jobs). `None`, wenn die Abfrage scheitert
    /// (dann ist nicht bekannt, ob der Job leer ist).
    #[cfg(windows)]
    pub fn pids(&self) -> Option<Vec<u32>> {
        use windows_sys::Win32::System::JobObjects::{JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST};
        let g = jobs();
        let Some(&(_, jh)) = g.iter().find(|(k, _)| *k == self.key) else { return None };
        let off = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);
        let mut cap = 64usize;
        for _ in 0..6 {
            // Kopf + `cap` Einträge (je ein usize); als usize-Vektor passend ausgerichtet.
            let words = off.div_ceil(std::mem::size_of::<usize>()) + cap;
            let mut buf = vec![0usize; words];
            let bytes = (words * std::mem::size_of::<usize>()) as u32;
            // SAFETY: `buf` ist `bytes` groß und für die Struktur ausgerichtet; das Job-Handle ist unter der Sperre gültig.
            let ok = unsafe { QueryInformationJobObject(jh as windows_sys::Win32::Foundation::HANDLE, JobObjectBasicProcessIdList, buf.as_mut_ptr().cast(), bytes, std::ptr::null_mut()) } != 0;
            // SAFETY: der Kopf liegt am Anfang des Puffers.
            let head = unsafe { &*(buf.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST) };
            let (assigned, listed) = (head.NumberOfAssignedProcesses as usize, (head.NumberOfProcessIdsInList as usize).min(cap));
            if ok && listed >= assigned.min(cap) && assigned <= cap {
                let ids = &buf[off / std::mem::size_of::<usize>()..][..listed];
                return Some(ids.iter().map(|p| *p as u32).filter(|p| *p != 0).collect());
            }
            cap = assigned.max(cap * 2) + 16;
        }
        None
    }

    #[cfg(unix)]
    pub fn pids(&self) -> Option<Vec<u32>> {
        super::unix_proc::job_pids(self.key)
    }
}

/// Ein Prozess im Job einer Sitzung, der das Schließen der Sitzung überleben würde (#168): Name der Programmdatei und –
/// falls er ein sichtbares Fenster hat – dessen Titel.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobProc {
    pub pid: u32,
    pub name: String,
    pub title: String,
    /// Hat ein sichtbares Hauptfenster (ein Programm, das man vor „Alle beenden“ besser selbst speichert und schließt).
    pub window: bool,
}

/// Welche Prozesse eines Jobs überleben das Schließen der Sitzung? Alle außer
/// * der Shell und den Prozessen, die an ihrer Konsole hängen (die beendet ConPTY beim Schließen ohnehin, z. B. ein
///   laufendes `claude` oder `vim`),
/// * Konsolen-Hosts (`conhost.exe`, `OpenConsole.exe`),
/// * Hilfsprozessen eines Vordergrundprogramms (#213): ohne sichtbares Fenster, gestartet von einem Prozess an der
///   Konsole der Shell (nicht von der Shell selbst) oder von einem solchen Hilfsprozess – z. B. die stdio-MCP-Server und
///   Werkzeugaufrufe von Claude Code. Sie laufen mit `CREATE_NO_WINDOW` in einer eigenen, unsichtbaren Konsole und
///   enden meist mit ihrem Elternprozess (Pipes) – aber nicht immer (Hintergrund-Shells, Entwicklungsserver, Dienste).
///   Deshalb ist das nur die Auswahl für die Rückfrage: Nach dem Schließen/Neu laden prüft der Hub den alten Job nach
///   kurzer Zeit erneut und meldet, was dann noch läuft (`App::poll_retired`). Weiter gezählt werden Programme mit
///   Fenster, alles, was die Shell selbst startet (`Start-Process`, `start`), und Prozesse, deren Elternprozess nicht
///   mehr im Job läuft.
///
/// `console = None`: Konsole nicht zu ermitteln (Shell beendet) – dann zählt alles außer Shell und Konsolen-Hosts.
/// `parent_of`: Elternprozess (nur einer im Job, der vor dem Kind entstand, [`job_parents`]); `has_window`: hat ein
/// sichtbares Fenster.
pub fn survivors(job: &[u32], shell: Option<u32>, console: Option<&[u32]>, parent_of: impl Fn(u32) -> Option<u32>, has_window: impl Fn(u32) -> bool, name_of: impl Fn(u32) -> String) -> Vec<u32> {
    // Hilfsprozess eines Vordergrundprogramms: über Prozesse ohne Fenster im Job bis zu einem an der Konsole (≠ Shell).
    let helper = |p: u32| {
        let Some(c) = console else { return false };
        let mut cur = p;
        for _ in 0..32 {
            if has_window(cur) {
                return false;
            }
            let Some(q) = parent_of(cur) else { return false };
            if q == cur || !job.contains(&q) || Some(q) == shell {
                return false;
            }
            if c.contains(&q) {
                return true;
            }
            cur = q;
        }
        false
    };
    job.iter()
        .copied()
        .filter(|p| Some(*p) != shell && !console.is_some_and(|c| c.contains(p)))
        .filter(|p| {
            let n = name_of(*p).to_ascii_lowercase();
            n != "conhost.exe" && n != "openconsole.exe"
        })
        .filter(|p| !helper(*p))
        .collect()
}

/// Hängt an der Konsole einer Shell nur noch die Shell selbst – läuft also kein Programm in ihrem Vordergrund?
///
/// `console` = Prozesse an der Konsole ([`console_pids`]), `shell` = PID der Shell. Zur Shell zählt auch eine Kette von
/// Prozessen mit demselben Programmnamen, je als einziges solches Kind des vorigen: Git Bash startet als `bin\bash.exe`
/// die eigentliche `usr\bin\bash.exe`. Jeder andere Prozess an der Konsole (ein laufendes `claude`, `node`, `vim`, ein
/// Hilfsprozess ohne lebenden Elternprozess) heißt: nicht frei.
///
/// Damit bestätigt der Hub, dass der Prompt der Shell (OSC 133;A) echt ist: Die Sequenz kann jedes Programm ausgeben,
/// aber solange es läuft, hängt es (oder das Programm, das es gestartet hat) an der Konsole.
// Außerhalb von Windows ruft nur der Test sie auf (`shell_is_idle` meldet dort immer „nicht zu ermitteln“).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shell_idle(console: &[u32], shell: u32, parent_of: impl Fn(u32) -> Option<u32>, name_of: impl Fn(u32) -> String) -> bool {
    if !console.contains(&shell) {
        return false;
    }
    let shell_name = name_of(shell).to_ascii_lowercase();
    let mut rest: Vec<u32> = console.iter().copied().filter(|p| *p != shell).collect();
    let mut cur = shell;
    while !rest.is_empty() {
        let kids: Vec<u32> = rest.iter().copied().filter(|p| parent_of(*p) == Some(cur)).collect();
        match kids.as_slice() {
            [k] if !shell_name.is_empty() && name_of(*k).to_ascii_lowercase() == shell_name => {
                cur = *k;
                rest.retain(|p| p != k);
            }
            _ => return false,
        }
    }
    true
}

/// Läuft im Vordergrund der Shell `shell` (PID) gerade nichts ([`shell_idle`])? `None`, wenn es nicht zu ermitteln ist.
/// Nur aus der UI-Schleife aufrufen (siehe [`console_pids`]).
#[cfg(windows)]
pub fn shell_is_idle(shell: u32) -> Option<bool> {
    // Für die Abfrage hängt sich der Hub selbst kurz an die Konsole: er steht mit in der Liste.
    let me = std::process::id();
    let console: Vec<u32> = console_pids(shell)?.into_iter().filter(|p| *p != me).collect();
    if console == [shell] {
        return Some(true);
    }
    let parents = job_parents(&console);
    let names = process_names();
    Some(shell_idle(&console, shell, |p| parents.get(&p).copied(), |p| names.get(&p).cloned().unwrap_or_default()))
}

/// Linux/macOS: Die Vordergrund-Prozessgruppe des Terminals ist die Gruppe der Shell (`platform::unix_proc`).
#[cfg(unix)]
pub fn shell_is_idle(shell: u32) -> Option<bool> {
    super::unix_proc::shell_is_idle(shell)
}

/// Elternprozesse der Prozesse `pids` (eines Jobs), soweit der Elternprozess selbst in `pids` liegt und vor dem Kind
/// entstand (sonst gehört die Eltern-PID einem früheren Prozess) – für [`survivors`].
#[cfg(windows)]
pub fn job_parents(pids: &[u32]) -> std::collections::HashMap<u32, u32> {
    let born: std::collections::HashMap<u32, u64> = pids.iter().filter_map(|p| Proc::open(*p).and_then(|h| h.created()).map(|t| (*p, t))).collect();
    process_parents()
        .into_iter()
        .filter(|(c, p)| c != p && pids.contains(c) && pids.contains(p))
        .filter(|(c, p)| matches!((born.get(p), born.get(c)), (Some(tp), Some(tc)) if tp <= tc))
        .collect()
}

#[cfg(unix)]
pub fn job_parents(pids: &[u32]) -> std::collections::HashMap<u32, u32> {
    super::unix_proc::job_parents(pids)
}

/// Prozesse, die an derselben Konsole hängen wie `pid` (die Shell einer Sitzung). Der Hub hängt sich dafür kurz an diese
/// Konsole (`AttachConsole` → `GetConsoleProcessList` → `FreeConsole`; er selbst hat keine). Strg+C-/Schließen-Ereignisse
/// in diesem Moment ignoriert ein eigener Handler; die Standard-Handles werden danach wiederhergestellt. Nur aus der
/// UI-Schleife aufrufen (eine Konsole je Prozess; eine Sperre schützt trotzdem). `None`, wenn das nicht geht.
#[cfg(windows)]
pub fn console_pids(pid: u32) -> Option<Vec<u32>> {
    use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole, GetConsoleProcessList, GetStdHandle, SetConsoleCtrlHandler, SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    unsafe extern "system" fn ignore(_ctrl: u32) -> windows_sys::Win32::Foundation::BOOL {
        1
    }
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static HANDLER: std::sync::Once = std::sync::Once::new();
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: reine Win32-Aufrufe; `ignore` ist eine `extern "system"`-Funktion mit passender Signatur, `buf` ist ein
    // lokaler Puffer mit der übergebenen Länge; die gesicherten Standard-Handles werden unverändert zurückgesetzt.
    unsafe {
        HANDLER.call_once(|| {
            SetConsoleCtrlHandler(Some(ignore), 1);
        });
        let std_ids = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
        let saved: Vec<_> = std_ids.iter().map(|i| GetStdHandle(*i)).collect();
        if AttachConsole(pid) == 0 {
            return None;
        }
        let mut buf = vec![0u32; 64];
        let mut n = GetConsoleProcessList(buf.as_mut_ptr(), buf.len() as u32) as usize;
        if n > buf.len() {
            buf = vec![0u32; n + 16];
            n = GetConsoleProcessList(buf.as_mut_ptr(), buf.len() as u32) as usize;
        }
        FreeConsole();
        for (i, h) in std_ids.iter().zip(saved) {
            SetStdHandle(*i, h);
        }
        (n > 0 && n <= buf.len()).then(|| buf[..n].to_vec())
    }
}

/// Linux/macOS: Prozesse mit demselben steuernden Terminal (bekommen beim Schließen der Sitzung SIGHUP).
#[cfg(unix)]
pub fn console_pids(pid: u32) -> Option<Vec<u32>> {
    super::unix_proc::console_pids(pid)
}

/// Namen aller laufenden Prozesse (PID → Programmdatei, z. B. `notepad.exe`).
#[cfg(windows)]
pub fn process_names() -> std::collections::HashMap<u32, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut out = std::collections::HashMap::new();
    // SAFETY: wie `process_parents`.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap != INVALID_HANDLE_VALUE {
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut pe) != 0 {
                loop {
                    let len = pe.szExeFile.iter().position(|c| *c == 0).unwrap_or(pe.szExeFile.len());
                    out.insert(pe.th32ProcessID, String::from_utf16_lossy(&pe.szExeFile[..len]));
                    if Process32NextW(snap, &mut pe) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
    }
    out
}

#[cfg(unix)]
pub fn process_names() -> std::collections::HashMap<u32, String> {
    super::unix_proc::process_names()
}

/// Prozesse mit sichtbarem Hauptfenster und dessen Titel (das erste Fenster mit Titel; leer, wenn keins einen hat).
#[cfg(windows)]
pub fn window_titles() -> std::collections::HashMap<u32, String> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible};
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        // SAFETY: `l` ist der Zeiger auf die HashMap aus `window_titles`, die während `EnumWindows` lebt; `h` kommt von
        // `EnumWindows`; `pid` und `buf` sind lokale Ausgabewerte mit passender Länge.
        unsafe {
            let map = &mut *(l as *mut std::collections::HashMap<u32, String>);
            if IsWindowVisible(h) == 0 {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            if pid == 0 || map.get(&pid).is_some_and(|t| !t.is_empty()) {
                return 1;
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
            map.insert(pid, String::from_utf16_lossy(&buf[..n.max(0) as usize]));
            1
        }
    }
    let mut map = std::collections::HashMap::new();
    // SAFETY: Rückruf mit passender Signatur; der Zeiger auf `map` gilt für die Dauer des Aufrufs.
    unsafe { EnumWindows(Some(each), &mut map as *mut _ as LPARAM) };
    map
}

#[cfg(not(windows))]
pub fn window_titles() -> std::collections::HashMap<u32, String> {
    std::collections::HashMap::new()
}

/// Prozessnamen und Fenster aller Prozesse, je Aufruf höchstens einmal und erst bei Bedarf ermittelt (für mehrere
/// Sitzungen in einer Rückfrage, #168).
#[derive(Default)]
pub struct ProcInfo {
    names: std::cell::OnceCell<std::collections::HashMap<u32, String>>,
    windows: std::cell::OnceCell<std::collections::HashMap<u32, String>>,
}

impl ProcInfo {
    fn names(&self) -> &std::collections::HashMap<u32, String> {
        self.names.get_or_init(process_names)
    }
    fn windows(&self) -> &std::collections::HashMap<u32, String> {
        self.windows.get_or_init(window_titles)
    }
}

/// Die überlebenden Prozesse eines Sitzungs-Jobs mit Namen, Fenster und Fenstertitel (#168). `shell` = PID der Shell.
pub fn lingering(job: &SessionJob, shell: Option<u32>, info: &ProcInfo) -> Vec<JobProc> {
    let pids = job.pids().unwrap_or_default();
    if pids.iter().all(|p| Some(*p) == shell) {
        return Vec::new();
    }
    let console = shell.and_then(console_pids);
    let names = info.names();
    let parents = job_parents(&pids);
    let keep = survivors(&pids, shell, console.as_deref(), |p| parents.get(&p).copied(), |p| info.windows().contains_key(&p), |p| names.get(&p).cloned().unwrap_or_default());
    if keep.is_empty() {
        return Vec::new();
    }
    let windows = info.windows();
    keep.into_iter()
        .map(|pid| JobProc {
            pid,
            name: names.get(&pid).cloned().unwrap_or_else(|| "?".into()),
            title: windows.get(&pid).cloned().unwrap_or_default(),
            window: windows.contains_key(&pid),
        })
        .collect()
}

/// Startet `spawn` (z. B. `spawn_command` einer Pseudokonsole) so, dass der neue Prozess schon im Job `job` beginnt
/// (vendored portable-pty, `PROC_THREAD_ATTRIBUTE_JOB_LIST`, #168). Liefert das Ergebnis und ob der Start im Job gelang
/// (sonst muss der Aufrufer den Prozess nachträglich zuweisen, [`SessionJob::adopt`]).
#[cfg(windows)]
pub fn spawn_in_job<T>(job: Option<&SessionJob>, spawn: impl FnOnce() -> T) -> (T, bool) {
    portable_pty::win::set_spawn_job(job.and_then(|j| j.raw_handle()));
    let out = spawn();
    // Erst lesen, dann zurücksetzen: `set_spawn_job` setzt auch die Meldung zurück.
    let in_job = job.is_some() && portable_pty::win::spawned_in_job();
    portable_pty::win::set_spawn_job(None);
    (out, in_job)
}

#[cfg(not(windows))]
pub fn spawn_in_job<T>(_job: Option<&SessionJob>, spawn: impl FnOnce() -> T) -> (T, bool) {
    (spawn(), false)
}

/// Ganze Prozessgruppe von `pid` beenden (Unix, #243): portable-pty startet das Kind mit `setsid`, es führt also eine
/// eigene Gruppe an; `SIGKILL` an `-pid` trifft alle ihre Prozesse. Unter Windows nichts (dort beendet der Job den Baum).
#[cfg(unix)]
pub fn kill_group(pid: u32) -> bool {
    let Ok(p) = i32::try_from(pid) else { return false };
    // SAFETY: reiner Systemaufruf; eine negative PID adressiert die Gruppe, p > 1 (PID 0/1 nie).
    p > 1 && unsafe { libc::kill(-p, libc::SIGKILL) } == 0
}

#[cfg(not(unix))]
pub fn kill_group(_pid: u32) -> bool {
    false
}

impl Drop for SessionJob {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let mut g = jobs();
            if let Some(i) = g.iter().position(|(k, _)| *k == self.key) {
                let (_, h) = g.remove(i);
                // SAFETY: Handle aus `CreateJobObjectW`, aus der Liste genommen und genau einmal geschlossen.
                unsafe { windows_sys::Win32::Foundation::CloseHandle(h as windows_sys::Win32::Foundation::HANDLE) };
            }
        }
        #[cfg(unix)]
        super::unix_proc::job_drop(self.key);
    }
}

/// Kennung des Sitzungs-Jobs, in dem der Prozess `process` (Handle mit Abfragerecht) läuft (#168).
#[cfg(windows)]
fn job_containing(process: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    jobs()
        .iter()
        .find(|(_, jh)| {
            let mut r = 0;
            // SAFETY: gültige Handles (das Job-Handle unter der Sperre), `r` ist ein lokaler Ausgabewert.
            unsafe { IsProcessInJob(process, *jh as windows_sys::Win32::Foundation::HANDLE, &mut r) != 0 && r != 0 }
        })
        .map(|(k, _)| *k)
}

/// Nachfahren von `root` (Erzeugungszeit `root_born`), die vor seiner Zuweisung an `job` entstanden sind, nachträglich
/// hineinstecken (#168). Jedes Kind muss nach seinem Elternteil entstanden sein (sonst gehört die Eltern-PID einem
/// früheren Prozess). Wiederholt, bis ein Durchgang nichts Neues findet: Kinder, die ein nachträglich zugewiesener
/// Prozess vor seiner Zuweisung gestartet hat, stehen erst in der nächsten Momentaufnahme.
#[cfg(windows)]
fn sweep(job: windows_sys::Win32::Foundation::HANDLE, root: u32, root_born: u64) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, IsProcessInJob};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
    for _ in 0..5 {
        let procs = process_parents();
        let mut tree: Vec<(u32, u64)> = vec![(root, root_born)];
        let mut added = false;
        let mut i = 0;
        while i < tree.len() {
            let (p, p_born) = tree[i];
            i += 1;
            for &(c, pp) in &procs {
                if pp != p || c == p || c == 0 || tree.iter().any(|(t, _)| *t == c) {
                    continue;
                }
                // SAFETY: reiner Aufruf; ein Nullhandle wird übersprungen.
                let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, c) };
                if h.is_null() {
                    continue;
                }
                if let Some(t) = process_created(h).filter(|t| *t >= p_born) {
                    let mut r = 0;
                    // SAFETY: gültige Handles, `r` ist ein lokaler Ausgabewert.
                    if unsafe { IsProcessInJob(h, job, &mut r) } != 0 && r == 0 && unsafe { AssignProcessToJobObject(job, h) } != 0 {
                        added = true;
                    }
                    tree.push((c, t));
                }
                // SAFETY: gültiges Handle aus `OpenProcess`, genau einmal geschlossen.
                unsafe { CloseHandle(h) };
            }
        }
        if !added {
            break;
        }
    }
}

/// Ein Abriss des Baums zählt als „verwaist“, wenn der letzte geprüfte Prozess nach dem Hub entstand. Ältere Abrisse
/// sind normal (z. B. der Explorer, dessen Elternprozess `userinit` längst beendet ist).
#[cfg_attr(not(windows), allow(dead_code))]
fn orphaned(last_born: u64, hub_born: u64) -> bool {
    last_born >= hub_born
}

#[cfg(all(test, windows))]
mod peer_tests {
    use super::*;

    #[test]
    fn tcp_peer_pid_finds_the_client_and_nothing_for_unknown_ports() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let c = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (s, _) = l.accept().unwrap();
        let (peer, local) = (s.peer_addr().unwrap(), s.local_addr().unwrap());
        assert_eq!(tcp_peer_pid(peer, local), Some(std::process::id()));
        // vertauscht (das wäre die Zeile des Hubs) oder unbekannte Ports: keine Zuordnung
        let unknown = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        assert_eq!(tcp_peer_pid(unknown, local), None);
        assert_eq!(tcp_peer_pid(peer, unknown), None);
        // Absender lebt: Baum beginnt bei ihm, nicht verwaist (der Testprozess ist vor seinem Start-Zeitpunkt nicht jünger)
        let pc = resolve_peer(peer, local).expect("lebender Absender");
        assert_eq!(pc.chain.first(), Some(&std::process::id()));
        assert!(!pc.orphan);
        drop(c);
    }

    /// Frist für das Starten von Hilfsprozessen (cmd, PowerShell, ping). Großzügig, weil ein ausgelasteter
    /// CI-Runner (Virenscanner, kalter .NET-Start) PowerShell erst nach vielen Sekunden startet (#290); die Wartenden
    /// fragen alle 50 ms ab und sind sonst sofort fertig.
    const SPAWN_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

    /// Kinder von `pid` (laut Momentaufnahme), wartet bis zu [`SPAWN_WAIT`] auf mindestens eines. Nur Prozesse, die nach
    /// `pid` entstanden sind: Die Eltern-PID in der Momentaufnahme kann von einem früheren, längst beendeten Prozess mit
    /// derselben PID stammen (unter Last vergibt Windows PIDs schnell neu, z. B. hinterlassen andere Tests losgelöste
    /// PowerShell-Prozesse ohne Elternteil). Solche fremden „Kinder“ steckt [`sweep`] zu Recht nicht in den Job.
    fn children_of(pid: u32) -> Vec<u32> {
        let born = process_born(pid).expect("Elternprozess läuft");
        let t0 = std::time::Instant::now();
        loop {
            let c: Vec<u32> = process_parents()
                .into_iter()
                .filter(|&(c, p)| p == pid && process_born(c).is_some_and(|t| t >= born))
                .map(|(c, _)| c)
                .collect();
            if !c.is_empty() || t0.elapsed() > SPAWN_WAIT {
                return c;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn job_of(pid: u32) -> Option<u64> {
        let h = Proc::open(pid)?;
        job_containing(h.0)
    }

    #[test]
    fn a_session_job_takes_the_shell_and_children_it_started_before_the_assignment() {
        use std::os::windows::io::AsRawHandle;
        // Die „Shell“ startet ihr Kind, bevor sie in den Job kommt (der Wettlauf, den `sweep` abdeckt).
        let mut shell = std::process::Command::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
        let kids = children_of(shell.id());
        assert!(!kids.is_empty(), "ping gestartet");
        let job = SessionJob::new().expect("Job");
        assert!(job.adopt(shell.as_raw_handle(), shell.id()));
        assert_eq!(job_of(shell.id()), Some(job.key()));
        for k in &kids {
            assert_eq!(job_of(*k), Some(job.key()), "vor der Zuweisung gestartetes Kind {k} nachgeholt");
        }
        assert_eq!(job_of(std::process::id()), None, "der Hub selbst liegt in keinem Sitzungs-Job");
        let other = SessionJob::new().expect("zweiter Job");
        assert_ne!(other.key(), job.key());
        job.terminate_all();
        let _ = shell.wait();
        let key = job.key();
        drop(job);
        assert!(jobs().iter().all(|(k, _)| *k != key), "Drop nimmt den Job aus der Liste");
        assert!(jobs().iter().any(|(k, _)| *k == other.key()));
    }

    #[test]
    fn a_detached_process_of_a_session_is_attributed_by_its_job() {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        // Kurzlebige „Shell“ im Job startet per `start` einen losgelösten PowerShell-Prozess und endet sofort; der
        // verbindet sich danach – sein Elternteil gibt es dann nicht mehr, der Job bleibt.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        // Lebt nach dem Verbinden noch lange genug für `resolve_peer`; `terminate_all` beendet ihn am Ende.
        let ps = format!("$c = New-Object Net.Sockets.TcpClient('127.0.0.1', {port}); Start-Sleep 120");
        // `raw_arg`: cmd versteht die Anführungszeichen-Maskierung von `args` (\") nicht.
        let mut shell = std::process::Command::new("cmd")
            .raw_arg(format!("/c start \"\" /b powershell -NoProfile -WindowStyle Hidden -Command \"{ps}\""))
            .spawn()
            .unwrap();
        let job = SessionJob::new().expect("Job");
        assert!(job.adopt(shell.as_raw_handle(), shell.id()));
        let _ = shell.wait();
        l.set_nonblocking(true).unwrap();
        let t0 = std::time::Instant::now();
        // Nicht blockierend: WouldBlock (10035) heißt nur „noch keine Verbindung“ – weiter warten, bis die Frist abläuft.
        let s = loop {
            match l.accept() {
                Ok((s, _)) => break s,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) && t0.elapsed() < SPAWN_WAIT => {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                Err(e) => panic!("keine Verbindung des losgelösten Prozesses nach {:?}: {e}", t0.elapsed()),
            }
        };
        let pc = resolve_peer(s.peer_addr().unwrap(), s.local_addr().unwrap()).expect("lebender Absender");
        assert_eq!(pc.job, Some(job.key()), "{pc:?}");
        assert!(!pc.orphan);
        job.terminate_all();
    }

    /// Hilfsprozess für [`no_breakaway_and_nested_jobs_keep_the_membership`] (läuft nur mit `PANELDECK_JOB_HELPER`):
    /// wartet auf „go“ (dann ist er im Job), versucht auszubrechen, steckt ein Kind in einen eigenen Job und meldet
    /// `RESULT breakaway=<ok|Fehlercode> nested=<0|1> pid=<Kind>`; beendet sich nach der nächsten Zeile.
    #[test]
    fn job_helper_process() {
        use std::io::BufRead;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        use windows_sys::Win32::System::Threading::{CreateProcessW, CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW};
        if std::env::var_os("PANELDECK_JOB_HELPER").is_none() {
            return;
        }
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        let mut cmdline: Vec<u16> = "cmd.exe /c exit 0".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: genullte Strukturen mit gesetztem `cb`; `cmdline` ist veränderbar und nullterminiert; die Handles aus
        // `pi` werden bei Erfolg geschlossen.
        let breakaway = unsafe {
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            if CreateProcessW(std::ptr::null(), cmdline.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW, std::ptr::null(), std::ptr::null(), &si, &mut pi) != 0 {
                CloseHandle(pi.hThread);
                CloseHandle(pi.hProcess);
                "ok".to_string()
            } else {
                GetLastError().to_string()
            }
        };
        let mut kid = std::process::Command::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
        // SAFETY: anonymer Job; das Prozess-Handle gehört `kid` und lebt über den Aufruf.
        let nested = unsafe {
            let j = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            !j.is_null() && AssignProcessToJobObject(j, kid.as_raw_handle() as _) != 0
        };
        println!("RESULT breakaway={breakaway} nested={} pid={}", nested as u8, kid.id());
        let _ = std::io::stdin().lock().read_line(&mut line);
        let _ = kid.kill();
        std::process::exit(0);
    }

    #[test]
    fn no_breakaway_and_nested_jobs_keep_the_membership() {
        use std::io::{BufRead, Write};
        use std::os::windows::io::AsRawHandle;
        let exe = std::env::current_exe().unwrap();
        let mut helper = std::process::Command::new(exe)
            .args(["--exact", "platform::process::peer_tests::job_helper_process", "--nocapture", "--test-threads=1"])
            .env("PANELDECK_JOB_HELPER", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let job = SessionJob::new().expect("Job");
        assert!(job.adopt(helper.as_raw_handle(), helper.id()));
        let mut stdin = helper.stdin.take().unwrap();
        writeln!(stdin, "go").unwrap();
        let out = std::io::BufReader::new(helper.stdout.take().unwrap());
        // libtest schreibt „test … ... “ ohne Zeilenende davor: die Meldung steht mitten in der Zeile.
        let res = out.lines().map_while(Result::ok).find_map(|l| l.find("RESULT ").map(|i| l[i..].to_string())).expect("Ergebnis des Hilfsprozesses");
        let field = |k: &str| res.split(' ').find_map(|f| f.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
        assert_ne!(field("breakaway"), "ok", "CREATE_BREAKAWAY_FROM_JOB muss scheitern: {res}");
        assert_eq!(field("breakaway"), "5", "Zugriff verweigert: {res}");
        assert_eq!(field("nested"), "1", "eigener Job eines Programms (verschachtelt): {res}");
        let kid: u32 = field("pid").parse().unwrap();
        assert_eq!(job_of(kid), Some(job.key()), "Kind im verschachtelten Job bleibt im Sitzungs-Job");
        let _ = writeln!(stdin, "done");
        let _ = helper.wait();
        job.terminate_all();
    }

    #[test]
    fn the_shell_starts_inside_its_job_without_a_race() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        // Wie `terminal::Session::spawn`: Job vor dem Start setzen, die Shell ist ab ihrer ersten Anweisung Mitglied.
        let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        // Ausgabe abholen (sonst blockiert ConPTY seine Programme beim Schreiben)
        let mut reader = pair.master.try_clone_reader().unwrap();
        std::thread::spawn(move || std::io::copy(&mut reader, &mut std::io::sink()));
        let job = SessionJob::new().expect("Job");
        // Genau der Weg von `terminal::Session::spawn`
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.args(["/c", "ping -n 30 127.0.0.1 >nul"]);
        let (child, in_job) = spawn_in_job(Some(&job), || pair.slave.spawn_command(cmd));
        let mut child = child.unwrap();
        assert!(in_job, "im Job gestartet (PROC_THREAD_ATTRIBUTE_JOB_LIST)");
        let pid = child.process_id().unwrap();
        assert_eq!(job_of(pid), Some(job.key()));
        // Der Job gilt nur für einen Start: der nächste auf diesem Thread läuft ohne
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.args(["/c", "exit 0"]);
        let mut other = pair.slave.spawn_command(cmd).unwrap();
        assert!(!portable_pty::win::spawned_in_job());
        let (_, in_job) = spawn_in_job(None, || ());
        assert!(!in_job, "ohne Job: nie im Job");
        assert_ne!(other.process_id().and_then(job_of), Some(job.key()));
        let _ = other.kill();
        assert!(job.terminate_all());
        let t0 = std::time::Instant::now();
        while matches!(child.try_wait(), Ok(None)) && t0.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(matches!(child.try_wait(), Ok(Some(_))), "„Alle beenden“ beendet die Shell");
    }

    #[test]
    fn survivors_are_job_processes_off_the_shell_console() {
        let names = |p: u32| match p {
            7 => "conhost.exe".to_string(),
            8 => "OpenConsole.exe".to_string(),
            _ => "x.exe".to_string(),
        };
        let none = |_: u32| None;
        let no_win = |_: u32| false;
        // Shell 1, an ihrer Konsole 1 und 2 (z. B. claude im Vordergrund); 3 losgelöst; 7/8 Konsolen-Hosts
        assert_eq!(survivors(&[1, 2, 3, 7, 8], Some(1), Some(&[1, 2]), none, no_win, names), [3]);
        assert_eq!(survivors(&[1, 2], Some(1), Some(&[1, 2]), none, no_win, names), Vec::<u32>::new(), "nur Vordergrund: nichts");
        // Konsole unbekannt (Shell beendet): alles außer Shell und Hosts
        assert_eq!(survivors(&[1, 2, 3, 7], Some(1), None, none, no_win, names), [2, 3]);
        assert_eq!(survivors(&[2, 3], None, None, none, no_win, names), [2, 3]);
    }

    #[test]
    fn helpers_of_a_foreground_program_do_not_count_as_survivors() {
        let names = |_: u32| "x.exe".to_string();
        // Shell 1, claude 2 an der Konsole; 3 = MCP-Server von claude (eigene unsichtbare Konsole), 4 = dessen Kind,
        // 5 = von der Shell gestartet (Start-Process), 6 = Fenster-Programm von claude, 9 = Kind des Fensterprogramms,
        // 10 = Elternprozess nicht im Job (verwaist).
        let parent = |p: u32| match p {
            2 | 5 => Some(1),
            3 | 6 => Some(2),
            4 => Some(3),
            9 => Some(6),
            10 => Some(99),
            _ => None,
        };
        let window = |p: u32| p == 6;
        let job = [1, 2, 3, 4, 5, 6, 9, 10];
        assert_eq!(survivors(&job, Some(1), Some(&[1, 2]), parent, window, names), [5, 6, 9, 10]);
        // Konsole unbekannt: keine Ausnahme für Hilfsprozesse (das Vordergrundprogramm ist nicht zu bestimmen)
        assert_eq!(survivors(&job, Some(1), None, parent, window, names), [2, 3, 4, 5, 6, 9, 10]);
        // Schleife in den Eltern-PIDs bricht ab
        let looped = |p: u32| match p {
            3 => Some(4),
            4 => Some(3),
            _ => None,
        };
        assert_eq!(survivors(&[1, 3, 4], Some(1), Some(&[1]), looped, |_| false, names), [3, 4]);
    }

    /// Hilfsprozess ohne Konsole (`DETACHED_PROCESS`) für [`lingering_processes_are_listed_and_end_all_ends_them`]: meldet
    /// `CONSOLE <pid>,<pid>…` der Konsole des Prozesses aus `PANELDECK_CONSOLE_OF` (wie der Hub, der keine Konsole hat).
    #[test]
    fn job_console_helper() {
        let Some(pid) = std::env::var("PANELDECK_CONSOLE_OF").ok().and_then(|p| p.parse::<u32>().ok()) else { return };
        let list = console_pids(pid).unwrap_or_default();
        println!("CONSOLE {}", list.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(","));
        std::process::exit(0);
    }

    #[test]
    fn lingering_processes_are_listed_and_end_all_ends_them() {
        use std::io::BufRead;
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        // „Shell“ mit eigener (unsichtbarer) Konsole und einem Vordergrund-Kind daran; dazu ein losgelöster Prozess ohne
        // Konsole im selben Job (wie `Start-Process` aus der Sitzung).
        let mut shell = std::process::Command::new("cmd").raw_arg("/c ping -n 30 127.0.0.1 >nul").creation_flags(CREATE_NO_WINDOW).spawn().unwrap();
        // Vordergrund-Kind = ping.exe (nicht der Konsolen-Host conhost.exe, der als erstes Kind der Shell startet)
        let t0 = std::time::Instant::now();
        let fg: Vec<u32> = loop {
            let names = process_names();
            let pings: Vec<u32> = process_parents().into_iter().filter(|(c, p)| *p == shell.id() && names.get(c).is_some_and(|n| n.eq_ignore_ascii_case("ping.exe"))).map(|(c, _)| c).collect();
            if !pings.is_empty() || t0.elapsed() > SPAWN_WAIT {
                break pings;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert_eq!(fg.len(), 1, "ping gestartet");
        let mut bg = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).creation_flags(DETACHED_PROCESS).spawn().unwrap();
        let job = SessionJob::new().expect("Job");
        assert!(job.adopt(shell.as_raw_handle(), shell.id()));
        assert!(job.adopt(bg.as_raw_handle(), bg.id()));
        let mut pids = job.pids().unwrap();
        pids.sort_unstable();
        let mut want = vec![shell.id(), bg.id()];
        want.extend(&fg);
        want.sort_unstable();
        // Dazu der Konsolen-Host der neuen Konsole der Shell (conhost.exe als ihr Kind, ebenfalls im Job).
        let names = process_names();
        let extra: Vec<u32> = pids.iter().copied().filter(|p| !want.contains(p)).collect();
        assert!(want.iter().all(|p| pids.contains(p)), "alle Prozesse des Jobs: {pids:?} ⊇ {want:?}");
        assert!(extra.iter().all(|p| names.get(p).is_some_and(|n| n.eq_ignore_ascii_case("conhost.exe"))), "sonst nur Konsolen-Hosts: {extra:?}");
        // Konsole der Shell aus einem Prozess ohne eigene Konsole bestimmen (der Testprozess hat eine)
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args(["--exact", "platform::process::peer_tests::job_console_helper", "--nocapture", "--test-threads=1"])
            .env("PANELDECK_CONSOLE_OF", shell.id().to_string())
            .creation_flags(DETACHED_PROCESS)
            .stderr(std::process::Stdio::null())
            .output()
            .unwrap();
        let line = out.stdout.lines().map_while(Result::ok).find_map(|l| l.find("CONSOLE ").map(|i| l[i + 8..].to_string())).expect("Ausgabe des Hilfsprozesses");
        let console: Vec<u32> = line.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        assert!(console.contains(&shell.id()) && fg.iter().all(|p| console.contains(p)), "Shell und Vordergrund an der Konsole: {console:?}");
        assert!(!console.contains(&bg.id()));
        let names = process_names();
        let parents = job_parents(&pids);
        let keep = survivors(&pids, Some(shell.id()), Some(&console), |p| parents.get(&p).copied(), |_| false, |p| names.get(&p).cloned().unwrap_or_default());
        assert_eq!(keep, [bg.id()], "nur der losgelöste Prozess überlebt das Schließen");
        assert_eq!(names.get(&bg.id()).map(|n| n.to_ascii_lowercase()), Some("ping.exe".into()));
        // „Alle beenden“
        assert!(job.terminate_all());
        let _ = shell.wait();
        let _ = bg.wait();
        let t0 = std::time::Instant::now();
        while job.pids() != Some(Vec::new()) && t0.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(job.pids(), Some(Vec::new()), "alle beendet");
    }

    #[test]
    fn a_hidden_console_helper_of_a_foreground_program_is_not_lingering() {
        use std::io::BufRead;
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let exe = std::env::current_exe().unwrap();
        // „Shell“ (cmd, eigene unsichtbare Konsole) mit einem Vordergrundprogramm daran (PowerShell; der Testprozess
        // selbst ist im Release-Build ein GUI-Programm ohne Konsole). Es startet ein Kind mit CREATE_NO_WINDOW
        // (`CreateNoWindow`, wie Claude Code seine stdio-MCP-Server) in einer eigenen Konsole und wartet darauf.
        // ping lebt lange genug für die Prüfungen auch auf einem langsamen Runner; `terminate_all` beendet ihn am Ende.
        let ps = "$i=New-Object Diagnostics.ProcessStartInfo 'ping','-n 300 127.0.0.1'; $i.UseShellExecute=$false; $i.CreateNoWindow=$true; [Diagnostics.Process]::Start($i).WaitForExit()";
        let mut shell = std::process::Command::new("cmd").raw_arg(format!("/c powershell -NoProfile -NonInteractive -Command \"{ps}\"")).creation_flags(CREATE_NO_WINDOW).spawn().unwrap();
        let job = SessionJob::new().expect("Job");
        assert!(job.adopt(shell.as_raw_handle(), shell.id()));
        let t0 = std::time::Instant::now();
        let (fg, mcp) = loop {
            let names = process_names();
            let procs = process_parents();
            let kid = |parent: u32, name: &str| procs.iter().find(|(c, p)| *p == parent && names.get(c).is_some_and(|n| n.eq_ignore_ascii_case(name))).map(|(c, _)| *c);
            let fg = kid(shell.id(), "powershell.exe");
            let mcp = fg.and_then(|f| kid(f, "ping.exe"));
            if let (Some(f), Some(m)) = (fg, mcp) {
                break (f, m);
            }
            assert!(t0.elapsed() < SPAWN_WAIT, "Vordergrundprogramm und Kind gestartet (nach {:?})", t0.elapsed());
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let pids = job.pids().unwrap();
        assert!([shell.id(), fg, mcp].iter().all(|p| pids.contains(p)), "alle im Job: {pids:?}");
        // Konsole der Shell aus einem Prozess ohne eigene Konsole bestimmen
        let out = std::process::Command::new(&exe)
            .args(["--exact", "platform::process::peer_tests::job_console_helper", "--nocapture", "--test-threads=1"])
            .env("PANELDECK_CONSOLE_OF", shell.id().to_string())
            .creation_flags(DETACHED_PROCESS)
            .stderr(std::process::Stdio::null())
            .output()
            .unwrap();
        let line = out.stdout.lines().map_while(Result::ok).find_map(|l| l.find("CONSOLE ").map(|i| l[i + 8..].to_string())).expect("Ausgabe des Hilfsprozesses");
        let console: Vec<u32> = line.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        assert!(console.contains(&shell.id()) && console.contains(&fg), "Shell und Vordergrund an der Konsole: {console:?}");
        assert!(!console.contains(&mcp), "CREATE_NO_WINDOW: eigene, unsichtbare Konsole: {console:?}");
        let names = process_names();
        let name = |p: u32| names.get(&p).cloned().unwrap_or_default();
        // 2.3.0: das Kind zählte als Hintergrundprozess (Fehlalarm, #213)
        let old = survivors(&pids, Some(shell.id()), Some(&console), |_| None, |_| false, name);
        assert!(old.contains(&mcp), "ohne Elternprüfung: Fehlalarm {old:?}");
        let parents = job_parents(&pids);
        assert_eq!(parents.get(&mcp), Some(&fg), "Elternprozess im Job: {parents:?}");
        let keep = survivors(&pids, Some(shell.id()), Some(&console), |p| parents.get(&p).copied(), |_| false, name);
        assert!(keep.is_empty(), "Hilfsprozess des Vordergrundprogramms überlebt nicht: {keep:?}");
        assert!(job.terminate_all());
        let _ = shell.wait();
    }

    #[test]
    fn terminate_listed_ends_only_the_listed_processes() {
        use std::os::windows::io::AsRawHandle;
        let mut a = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let mut b = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let job = SessionJob::new().expect("Job");
        assert!(job.adopt(a.as_raw_handle(), a.id()) && job.adopt(b.as_raw_handle(), b.id()));
        let born_a = process_born(a.id()).expect("Erzeugungszeit");
        let born_b = process_born(b.id()).expect("Erzeugungszeit");
        // Falsche Erzeugungszeit (PID neu vergeben) und Prozess außerhalb des Jobs: nichts
        assert_eq!(job.terminate_listed(&[(a.id(), born_a + 1), (std::process::id(), process_born(std::process::id()).unwrap())]), 0);
        assert_eq!(job.terminate_listed(&[(a.id(), born_a)]), 1);
        let _ = a.wait();
        assert!(matches!(b.try_wait(), Ok(None)), "nicht genannter Prozess läuft weiter");
        assert_eq!(job.terminate_listed(&[(b.id(), born_b)]), 1);
        let _ = b.wait();
    }

    #[test]
    fn dropping_a_process_handle_closes_it() {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
        let count = || {
            let mut n = 0u32;
            // SAFETY: Pseudo-Handle des eigenen Prozesses; `n` ist ein lokaler Ausgabewert.
            unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut n) };
            n
        };
        let before = count();
        for _ in 0..500 {
            let h = Proc::open(std::process::id()).expect("eigener Prozess");
            assert!(h.alive());
        }
        // Andere Tests laufen parallel und öffnen eigene Handles; 500 vergessene wären deutlich zu sehen.
        assert!(count() < before + 250, "Handles geschlossen: {before} → {}", count());
    }

    #[test]
    fn a_break_counts_as_orphan_only_after_the_hub_started() {
        assert!(orphaned(200, 100), "losgelöst nach dem Hub-Start");
        assert!(orphaned(100, 100));
        assert!(!orphaned(50, 100), "z. B. Explorer: Elternteil längst beendet, vor dem Hub gestartet");
    }
}

/// Reine Logik, auf allen Plattformen getestet.
#[cfg(test)]
mod idle_tests {
    use super::*;

    /// Review (#282): Der Prompt der Shell (OSC 133;A) lässt sich von jedem Programm ausgeben – echt ist er nur, wenn an
    /// der Konsole nichts mehr außer der Shell hängt.
    #[test]
    fn a_shell_is_idle_only_when_nothing_else_hangs_on_its_console() {
        let name = |p: u32| match p {
            1 | 2 | 6 => "bash.exe".to_string(),
            3 => "claude.exe".to_string(),
            4 => "node.exe".to_string(),
            _ => "powershell.exe".to_string(),
        };
        let parents = |pairs: &'static [(u32, u32)]| move |c: u32| pairs.iter().find(|(k, _)| *k == c).map(|(_, p)| *p);
        let none = |_: u32| None;
        // PowerShell (10) allein: frei. Mit einem laufenden Programm (3) nicht – auch wenn es den Prompt vortäuscht.
        assert!(shell_idle(&[10], 10, none, name));
        assert!(!shell_idle(&[10, 3], 10, parents(&[(3, 10)]), name));
        assert!(!shell_idle(&[10, 3, 4], 10, parents(&[(3, 10), (4, 3)]), name), "Claude samt Hilfsprozess");
        // Ein Prozess ohne lebenden Elternprozess an der Konsole (Rest eines abgeschossenen Programms): nicht frei
        assert!(!shell_idle(&[10, 4], 10, none, name));
        // Git Bash: der Starter (1) und die eigentliche bash (2) sind die Shell
        assert!(shell_idle(&[1, 2], 1, parents(&[(2, 1)]), name));
        assert!(!shell_idle(&[1, 2, 3], 1, parents(&[(2, 1), (3, 2)]), name), "claude unter der bash");
        // Ein Skript in bash, das node startet: die Kette gleichnamiger Prozesse zählt zur Shell, node nicht
        assert!(!shell_idle(&[1, 2, 6, 4], 1, parents(&[(2, 1), (6, 2), (4, 6)]), name));
        // Zwei gleichnamige Kinder (eine Pipeline, ein Hintergrundprozess): nicht eindeutig, also nicht frei
        assert!(!shell_idle(&[1, 2, 6], 1, parents(&[(2, 1), (6, 1)]), name));
        // Ein gleichnamiger Prozess, der nicht von der Shell stammt, oder die Shell gar nicht an der Konsole: nicht frei
        assert!(!shell_idle(&[10, 11], 10, parents(&[(11, 3)]), name));
        assert!(!shell_idle(&[3], 10, none, name));
        assert!(!shell_idle(&[], 10, none, name));
    }
}
