//! Window chrome of the borderless main window. Windows: rounded corners and Snap Layouts (below). Other platforms:
//! `init`/`apply_corners`/`shutdown` do nothing, corners stay square, there is no system hover on the maximize button.
//!
//! Windows-Rahmen des rahmenlosen Hauptfensters (Windows 11):
//! - abgerundete Fensterecken über DWM,
//! - Snap-Layouts-Flyout am Maximieren-Knopf der eigenen Fensterleiste (Fensterprozedur meldet dort `HTMAXBUTTON`).
//!
//! Alles Übrige (Ziehen, Resize-Ränder, Doppelklick) bleibt bei egui; die Fensterleiste meldet nur das Rechteck des
//! Maximieren-Knopfes (`report_max_button`) und fragt den Hover-Zustand ab (`max_hover`), weil egui für diesen
//! Bereich keine Mausereignisse mehr bekommt (das System behandelt ihn als Nicht-Client-Bereich).
//!
//! Unterklassifizierung: bewusst `SetWindowLongPtrW(GWLP_WNDPROC)` statt `SetWindowSubclass` (comctl32). Die
//! comctl32-Funktion wird in der Version 5.82 nur über die Ordnungszahl exportiert; ohne Manifest für comctl32 v6
//! (die .exe hat keins) würde der Namensimport beim Start scheitern. `GWLP_WNDPROC` liegt in user32, ist überall
//! vorhanden, und außer winit hängt sich im Hauptfenster niemand in die Prozedur.

use eframe::egui;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};

/// Ecken-Radius des Fensters in Pixeln bei 100 % Skalierung (`DWMWCP_ROUND` von Windows 11).
const CORNER_PX: f32 = 8.0;

/// Rechteck des Maximieren-Knopfes in Client-Pixeln: `[links, oben, rechts, unten]`.
static BTN: [AtomicI32; 4] = [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)];
static BTN_VALID: AtomicBool = AtomicBool::new(false);
/// Zeiger steht (laut Nachrichten des Systems) auf dem Maximieren-Knopf.
static HOVER: AtomicBool = AtomicBool::new(false);
/// Linke Maustaste wurde auf dem Knopf gedrückt (Klick wird erst beim Loslassen ausgeführt).
#[cfg_attr(not(windows), allow(dead_code))]
static PRESSED: AtomicBool = AtomicBool::new(false);
/// Fensterecken sind tatsächlich rund (DWM hat das Attribut angenommen; unter Windows 10 nicht).
static ROUNDED: AtomicBool = AtomicBool::new(false);

// ------------------------------------------------------------------ reine Hilfsfunktionen

/// Rechteck in egui-Punkten → Client-Pixel `[links, oben, rechts, unten]`; `inset_top_right` (Punkte) lässt oben und rechts
/// den Resize-Rand des Fensters frei. Links/oben werden abgerundet, rechts/unten aufgerundet (Treffer im Zweifel großzügig).
pub fn rect_to_px(min: (f32, f32), max: (f32, f32), ppp: f32, inset_top_right: f32) -> [i32; 4] {
    let l = (min.0 * ppp).floor() as i32;
    let t = ((min.1 + inset_top_right) * ppp).ceil() as i32;
    let r = ((max.0 - inset_top_right) * ppp).floor() as i32;
    let b = (max.1 * ppp).ceil() as i32;
    [l, t, r, b]
}

/// Liegt der Client-Punkt im Rechteck (links/oben eingeschlossen, rechts/unten ausgeschlossen)?
#[cfg_attr(not(windows), allow(dead_code))]
pub fn contains(r: [i32; 4], x: i32, y: i32) -> bool {
    r[2] > r[0] && r[3] > r[1] && x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

/// Ecken-Radius in egui-Punkten: Windows skaliert ihn mit der Bildschirm-DPI (nicht mit dem Zoom der Oberfläche).
pub fn corner_radius_pts(zoom: f32, rounded: bool, maximized: bool) -> f32 {
    if !rounded || maximized {
        0.0
    } else {
        CORNER_PX / zoom.max(0.1)
    }
}

/// Aus `lParam` von `WM_NCHITTEST` die Bildschirmkoordinaten holen (vorzeichenbehaftete 16-Bit-Werte).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn point_from_lparam(lp: isize) -> (i32, i32) {
    ((lp & 0xffff) as u16 as i16 as i32, ((lp >> 16) & 0xffff) as u16 as i16 as i32)
}

// ------------------------------------------------------------------ Schnittstelle zur Fensterleiste

/// Rechteck des Maximieren-Knopfes (egui-Punkte) melden; jedes Frame aufrufen. Ohne aktives Subclass wirkungslos.
pub fn report_max_button(rect: egui::Rect, ppp: f32, inset_top_right: f32) {
    let px = rect_to_px((rect.min.x, rect.min.y), (rect.max.x, rect.max.y), ppp, inset_top_right);
    for (a, v) in BTN.iter().zip(px) {
        a.store(v, Relaxed);
    }
    BTN_VALID.store(true, Relaxed);
}

/// Zeigt der Zeiger laut System auf den Maximieren-Knopf? (egui sieht dort keine Mausereignisse.)
pub fn max_hover() -> bool {
    HOVER.load(Relaxed)
}

/// Sind die Fensterecken rund?
pub fn rounded() -> bool {
    ROUNDED.load(Relaxed)
}

// ------------------------------------------------------------------ Windows

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::atomic::AtomicIsize;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    /// Ursprüngliche Fensterprozedur (von winit); 0 = nicht installiert.
    static OLD_PROC: AtomicIsize = AtomicIsize::new(0);
    static HWND_MAIN: AtomicIsize = AtomicIsize::new(0);
    static CTX: OnceLock<egui::Context> = OnceLock::new();

    fn repaint() {
        if let Some(c) = CTX.get() {
            c.request_repaint();
        }
    }

    /// Fensterprozedur des Hauptfensters. Darf nie paniken (die App läuft mit `panic = "abort"`): nur Atomics, keine
    /// `unwrap`/Indexzugriffe. Alles, was nicht den Maximieren-Knopf betrifft, geht unverändert an winit weiter.
    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let old = OLD_PROC.load(Relaxed);
        // SAFETY: `old` ist der von `SetWindowLongPtrW` gelieferte vorherige Prozedur-Zeiger (oder 0 = None).
        let old_fn: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(old) };
        // SAFETY (alle Aufrufe unten): `hwnd` ist das Fenster, dem diese Prozedur gehört; Zeiger zeigen auf lokale Werte.
        unsafe {
            match msg {
                WM_NCHITTEST => {
                    let r = CallWindowProcW(old_fn, hwnd, msg, wp, lp);
                    // Nur dort eingreifen, wo winit „Client“ meldet (Ränder/Resize bleiben unberührt).
                    if r == HTCLIENT as LRESULT && BTN_VALID.load(Relaxed) {
                        let (x, y) = point_from_lparam(lp);
                        let mut pt = POINT { x, y };
                        if ScreenToClient(hwnd, &mut pt) != 0 && !is_iconic(hwnd) {
                            let rect = [BTN[0].load(Relaxed), BTN[1].load(Relaxed), BTN[2].load(Relaxed), BTN[3].load(Relaxed)];
                            if contains(rect, pt.x, pt.y) {
                                return HTMAXBUTTON as LRESULT;
                            }
                        }
                    }
                    return r;
                }
                WM_NCMOUSEMOVE => {
                    if wp == HTMAXBUTTON as WPARAM {
                        if !HOVER.swap(true, Relaxed) {
                            repaint();
                        }
                        // Ohne eigene Verfolgung käme kein WM_NCMOUSELEAVE.
                        let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                        TrackMouseEvent(&mut t);
                    } else if HOVER.swap(false, Relaxed) {
                        repaint();
                    }
                }
                WM_NCMOUSELEAVE => {
                    PRESSED.store(false, Relaxed);
                    if HOVER.swap(false, Relaxed) {
                        repaint();
                    }
                }
                WM_MOUSEMOVE => {
                    // Zeiger ist in den Client-Bereich zurückgekehrt.
                    if HOVER.swap(false, Relaxed) {
                        repaint();
                    }
                }
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wp == HTMAXBUTTON as WPARAM => {
                    // Standardverhalten (Tracking-Schleife) vermeiden; geklickt wird beim Loslassen.
                    PRESSED.store(true, Relaxed);
                    return 0;
                }
                WM_NCLBUTTONUP if wp == HTMAXBUTTON as WPARAM => {
                    if PRESSED.swap(false, Relaxed) {
                        let cmd = if IsZoomed(hwnd) != 0 { SC_RESTORE } else { SC_MAXIMIZE };
                        // Post statt Send: die Größenänderung läuft nach dem Verlassen dieser Prozedur.
                        PostMessageW(hwnd, WM_SYSCOMMAND, cmd as WPARAM, 0);
                    }
                    return 0;
                }
                WM_NCDESTROY => {
                    reset_state();
                    if old != 0 {
                        SetWindowLongPtrW(hwnd, GWLP_WNDPROC, old);
                    }
                }
                _ => {}
            }
            CallWindowProcW(old_fn, hwnd, msg, wp, lp)
        }
    }

    fn reset_state() {
        BTN_VALID.store(false, Relaxed);
        HOVER.store(false, Relaxed);
        PRESSED.store(false, Relaxed);
    }

    /// Minimiert (z. B. im Tray versteckt) gibt es keinen Knopf.
    unsafe fn is_iconic(hwnd: HWND) -> bool {
        // SAFETY: reine Abfrage auf das eigene Fenster.
        unsafe { IsIconic(hwnd) != 0 }
    }

    fn proc_addr() -> isize {
        proc as *const () as usize as isize
    }

    pub fn init(hwnd: isize, ctx: &egui::Context) {
        let _ = CTX.set(ctx.clone());
        apply_corners(hwnd);
        if OLD_PROC.load(Relaxed) != 0 {
            return;
        }
        let h = hwnd as HWND;
        HWND_MAIN.store(hwnd, Relaxed);
        // SAFETY: `h` ist das Hauptfenster (Aufruf im UI-Thread, der es erzeugt hat); `proc` hat die passende Signatur.
        let old = unsafe { SetWindowLongPtrW(h, GWLP_WNDPROC, proc_addr()) };
        OLD_PROC.store(old, Relaxed);
    }

    /// Abgerundete Ecken verlangen; unter Windows 10 lehnt DWM das Attribut ab (dann bleibt es still bei eckig).
    pub fn apply_corners(hwnd: isize) {
        const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
        const DWMWCP_ROUND: i32 = 2;
        let pref: i32 = DWMWCP_ROUND;
        // SAFETY: `pref` lebt über den Aufruf; die Größe stimmt mit dem Attributtyp (DWORD) überein.
        let hr = unsafe { DwmSetWindowAttribute(hwnd as HWND, DWMWA_WINDOW_CORNER_PREFERENCE, (&pref as *const i32).cast(), 4) };
        ROUNDED.store(hr >= 0, Relaxed);
    }

    pub fn shutdown() {
        let hwnd = HWND_MAIN.load(Relaxed);
        let old = OLD_PROC.load(Relaxed);
        if hwnd == 0 || old == 0 {
            return;
        }
        let h = hwnd as HWND;
        // SAFETY: nur zurückschreiben, wenn unsere Prozedur noch ganz oben hängt; sonst bliebe ein fremdes Subclass
        // hinter uns verwaist. Dann bleibt `proc` installiert und reicht alles unverändert durch (OLD_PROC bleibt gesetzt).
        unsafe {
            if GetWindowLongPtrW(h, GWLP_WNDPROC) == proc_addr() {
                SetWindowLongPtrW(h, GWLP_WNDPROC, old);
                OLD_PROC.store(0, Relaxed);
            }
        }
        reset_state();
    }
}

#[cfg(windows)]
pub use imp::{apply_corners, init, shutdown};

#[cfg(not(windows))]
pub fn init(_hwnd: isize, _ctx: &egui::Context) {}
#[cfg(not(windows))]
pub fn apply_corners(_hwnd: isize) {}
#[cfg(not(windows))]
pub fn shutdown() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_conversion_scales_and_insets() {
        // 100 %: Punkte = Pixel
        assert_eq!(rect_to_px((1000.0, 0.0), (1046.0, 34.0), 1.0, 0.0), [1000, 0, 1046, 34]);
        // Resize-Rand 6 Punkte oben und rechts frei lassen
        assert_eq!(rect_to_px((1000.0, 0.0), (1046.0, 34.0), 1.0, 6.0), [1000, 6, 1040, 34]);
        // 150 %: Pixel = Punkte * 1,5 (links abrunden, rechts abrunden nach dem Inset, unten aufrunden)
        assert_eq!(rect_to_px((100.5, 0.0), (146.5, 34.0), 1.5, 0.0), [150, 0, 219, 51]);
    }

    #[test]
    fn hit_test_is_half_open_and_ignores_empty_rects() {
        let r = [100, 0, 146, 34];
        assert!(contains(r, 100, 0));
        assert!(contains(r, 145, 33));
        assert!(!contains(r, 146, 10));
        assert!(!contains(r, 120, 34));
        assert!(!contains(r, 99, 10));
        assert!(!contains([10, 10, 10, 20], 10, 15), "leeres Rechteck trifft nie");
        assert!(!contains([0, 0, 0, 0], 0, 0));
    }

    #[test]
    fn lparam_screen_coordinates_are_signed() {
        assert_eq!(point_from_lparam((20 << 16) | 10), (10, 20));
        // negative Koordinaten (Monitor links/oberhalb des Hauptmonitors)
        assert_eq!(point_from_lparam(((-5i16 as u16 as isize) << 16) | (-300i16 as u16 as isize)), (-300, -5));
    }

    #[test]
    fn corner_radius_follows_dpi_not_zoom_and_vanishes_when_maximized() {
        assert_eq!(corner_radius_pts(1.0, true, false), 8.0);
        assert_eq!(corner_radius_pts(2.0, true, false), 4.0);
        assert_eq!(corner_radius_pts(1.0, true, true), 0.0);
        assert_eq!(corner_radius_pts(1.0, false, false), 0.0);
    }
}
