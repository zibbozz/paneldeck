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

/// PID und alle Vorfahren (Eltern, Großeltern …) eines Prozesses, nächster zuerst. Dient dazu, eine MCP-Brücke
/// einer Hub-Sitzung zuzuordnen: Läuft ein Agent in einem Hub-Terminal, ist die Shell der Sitzung ein Vorfahre der Brücke.
/// Unter Windows über einen Prozess-Schnappschuss; ein wiederverwendeter Eltern-PID kann theoretisch täuschen
/// (deshalb höchstens 40 Stufen und Abbruch bei Wiederholung).
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

/// Liegt der Punkt (Bildschirmkoordinaten, grob in Pixeln) auf einem angeschlossenen Monitor? Für gespeicherte Fensterlagen:
/// nach einem Wechsel der Monitore soll kein Fenster unsichtbar außerhalb landen.
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
