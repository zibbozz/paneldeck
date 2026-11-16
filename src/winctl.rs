//! Windows-Fensterfunktionen: Fenster im Tray verstecken (minimiert, ohne Taskleistenbutton).
//!
//! Das Fenster bleibt dabei "minimiert sichtbar", damit die Ereignisschleife (Hooks, Benachrichtigungen)
//! weiterläuft; nur der Taskleisten-/Alt+Tab-Eintrag verschwindet (WS_EX_TOOLWINDOW).

#[cfg(windows)]
pub fn hwnd_from(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn hwnd_from(_cc: &eframe::CreationContext<'_>) -> Option<isize> {
    None
}

/// `hidden = true`: minimieren und aus Taskleiste/Alt+Tab ausblenden; `false`: wiederherstellen.
#[cfg(windows)]
pub fn set_tray_hidden(hwnd: isize, hidden: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let h = hwnd as *mut core::ffi::c_void;
    // SAFETY: reine Win32-Aufrufe auf ein Fenster-Handle; ein ungültiges Handle lässt sie fehlschlagen,
    // es werden keine Zeiger auf Rust-Speicher übergeben.
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        ShowWindow(h, SW_HIDE);
        if hidden {
            SetWindowLongPtrW(h, GWL_EXSTYLE, (ex | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize));
            ShowWindow(h, SW_SHOWMINNOACTIVE);
        } else {
            SetWindowLongPtrW(h, GWL_EXSTYLE, (ex & !(WS_EX_TOOLWINDOW as isize)) | WS_EX_APPWINDOW as isize);
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
        }
    }
}

#[cfg(not(windows))]
pub fn set_tray_hidden(_hwnd: isize, _hidden: bool) {}

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

#[cfg(not(windows))]
pub fn tcp_peer_pid(_peer: std::net::SocketAddr, _local: std::net::SocketAddr) -> Option<u32> {
    None
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
}

/// Absender einer angenommenen Verbindung bestimmen – sofort auf dem Verbindungs-Thread, ohne Zeitfenster für
/// Täuschungen (#152): PID aus der TCP-Tabelle, Prozess öffnen (das Handle hält die PID fest), prüfen, dass er noch
/// läuft und die Zeile noch ihm gehört, dann seinen Baum mit Erzeugungszeiten ablaufen. `None` (= abweisen), wenn der
/// Absender schon beendet ist, nicht zu öffnen ist oder seine Zeile verschwunden ist.
#[cfg(windows)]
pub fn resolve_peer(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> Option<PeerChain> {
    use windows_sys::Win32::Foundation::CloseHandle;
    let pid = tcp_peer_pid(peer, local)?;
    let h = Proc::open(pid)?;
    if !h.alive() || tcp_peer_pid(peer, local) != Some(pid) {
        return None;
    }
    let born = h.created()?;
    let out = walk_tree(pid, born);
    // SAFETY: gültiges Handle aus `OpenProcess`, genau einmal geschlossen.
    unsafe { CloseHandle(h.0) };
    Some(out)
}

#[cfg(not(windows))]
pub fn resolve_peer(_peer: std::net::SocketAddr, _local: std::net::SocketAddr) -> Option<PeerChain> {
    Some(PeerChain::default())
}

/// Ein geöffneter Prozess (nur Abfragerechte). Schließen muss der Aufrufer (`CloseHandle`).
#[cfg(windows)]
struct Proc(windows_sys::Win32::Foundation::HANDLE);

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
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut parent: HashMap<u32, u32> = HashMap::new();
    // SAFETY: `pe` ist ein lokal genullter PROCESSENTRY32W mit gesetztem `dwSize` und wird nur an die ToolHelp-Aufrufe
    // übergeben; das Snapshot-Handle wird nach der Prüfung (`INVALID_HANDLE_VALUE`) geschlossen.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap != INVALID_HANDLE_VALUE {
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
    }
    let mut out = PeerChain { chain: vec![pid], orphan: false };
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
                    // SAFETY: gültiges Handle aus `OpenProcess`, genau einmal geschlossen.
                    unsafe { CloseHandle(h.0) };
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

/// Ein Abriss des Baums zählt als „verwaist“, wenn der letzte geprüfte Prozess nach dem Hub entstand. Ältere Abrisse
/// sind normal (z. B. der Explorer, dessen Elternprozess `userinit` längst beendet ist).
fn orphaned(last_born: u64, hub_born: u64) -> bool {
    last_born >= hub_born
}

/// Eigenes Fenster (dieses Prozesses) mit genau diesem Titel suchen – für weitere Fenster, deren Handle egui nicht herausgibt
/// (runde Ecken über DWM). Der Titel ist beim Anlegen eindeutig (`Paneldeck #<n>`).
#[cfg(windows)]
pub fn find_own_window(title: &str) -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` ist nullterminiert und lebt über den Aufruf; `pid` ist ein lokaler Ausgabewert.
    unsafe {
        let h = FindWindowW(std::ptr::null(), wide.as_ptr());
        if h.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        (pid == std::process::id()).then_some(h as isize)
    }
}

#[cfg(not(windows))]
pub fn find_own_window(_title: &str) -> Option<isize> {
    None
}

/// Liegt der Punkt (Bildschirmkoordinaten in physischen Pixeln) auf einem angeschlossenen Monitor? Für gespeicherte
/// Fensterlagen: nach einem Wechsel der Monitore soll kein Fenster unsichtbar außerhalb landen.
#[cfg(windows)]
pub fn point_on_screen(x: f32, y: f32) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONULL};
    // Etwas Abstand nach innen: die Titelleiste soll greifbar bleiben.
    let pt = POINT { x: x as i32 + 40, y: y as i32 + 10 };
    // SAFETY: reine Abfrage mit einem Wert-Argument.
    !unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONULL) }.is_null()
}

#[cfg(not(windows))]
pub fn point_on_screen(_x: f32, _y: f32) -> bool {
    true
}

/// Arbeitsfläche (ohne Taskleiste) des Monitors am Punkt oder des nächstgelegenen, in physischen Pixeln:
/// `[links, oben, rechts, unten]`.
#[cfg(windows)]
pub fn work_area(x: f32, y: f32) -> Option<[f32; 4]> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    // SAFETY: `mi` ist ein lokal genulltes MONITORINFO mit gesetztem `cbSize`; der Monitor-Griff stammt direkt aus
    // MonitorFromPoint (mit MONITOR_DEFAULTTONEAREST nie null) und wird nur gelesen.
    unsafe {
        let m = MonitorFromPoint(POINT { x: x as i32, y: y as i32 }, MONITOR_DEFAULTTONEAREST);
        if m.is_null() {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(m, &mut mi) == 0 {
            return None;
        }
        let r = mi.rcWork;
        Some([r.left as f32, r.top as f32, r.right as f32, r.bottom as f32])
    }
}

#[cfg(not(windows))]
pub fn work_area(_x: f32, _y: f32) -> Option<[f32; 4]> {
    None
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

    #[test]
    fn a_break_counts_as_orphan_only_after_the_hub_started() {
        assert!(orphaned(200, 100), "losgelöst nach dem Hub-Start");
        assert!(orphaned(100, 100));
        assert!(!orphaned(50, 100), "z. B. Explorer: Elternteil längst beendet, vor dem Hub gestartet");
    }
}
