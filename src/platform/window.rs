//! Native windows: the window handle, hiding to the tray, finding own windows, monitor work areas.
//!
//! Windows: Fenster im Tray verstecken (minimiert, ohne Taskleistenbutton). Das Fenster bleibt dabei "minimiert
//! sichtbar", damit die Ereignisschleife (Hooks, Benachrichtigungen) weiterläuft; nur der Taskleisten-/Alt+Tab-Eintrag
//! verschwindet (WS_EX_TOOLWINDOW). Other platforms: no handle (`None`), so the callers fall back to egui viewport
//! commands; every point counts as on screen.

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
