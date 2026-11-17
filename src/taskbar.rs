//! Fortschritt im Taskleisten-Knopf (ITaskbarList3, #183): zeigt den Fortschritt, den Programme per OSC 9;4 melden.
//! Die COM-Schnittstelle wird von Hand angesprochen (windows-sys enthält keine COM-Schnittstellen); sie läuft im
//! UI-Thread (STA, von winit für Drag & Drop ohnehin initialisiert). Gesetzt wird nur bei einer Änderung.
//! Anderswo ohne Wirkung.

use crate::vtnotify::Progress;
use std::collections::HashMap;

/// Zustand der Taskleisten-Anzeige je Fenster (HWND als `isize`).
pub struct Taskbar {
    applied: HashMap<isize, Option<Progress>>,
    #[cfg(windows)]
    com: imp::Com,
}

impl Default for Taskbar {
    fn default() -> Self {
        Taskbar {
            applied: HashMap::new(),
            #[cfg(windows)]
            com: imp::Com::default(),
        }
    }
}

impl Taskbar {
    /// Fortschritt der Fenster setzen (`None` = keiner). Fenster, die nicht mehr genannt werden, werden vergessen.
    pub fn update(&mut self, wins: &[(isize, Option<Progress>)]) {
        self.applied.retain(|h, _| wins.iter().any(|(w, _)| w == h));
        for &(hwnd, p) in wins {
            if self.applied.get(&hwnd) == Some(&p) {
                continue;
            }
            // Ein noch nie gesetztes Fenster ohne Fortschritt braucht keinen Aufruf.
            if p.is_none() && !self.applied.contains_key(&hwnd) {
                self.applied.insert(hwnd, None);
                continue;
            }
            if self.apply(hwnd, p) {
                self.applied.insert(hwnd, p);
            }
        }
    }

    #[cfg(windows)]
    fn apply(&mut self, hwnd: isize, p: Option<Progress>) -> bool {
        self.com.set(hwnd, p)
    }

    #[cfg(not(windows))]
    fn apply(&mut self, _hwnd: isize, _p: Option<Progress>) -> bool {
        true
    }
}

/// Zusammenfassung mehrerer Sitzungen eines Fensters für den einen Taskleisten-Knopf: der „schwerste“ Zustand gewinnt
/// (Fehler > angehalten > normal > unbestimmt), bei gleichem Zustand der kleinste Prozentwert (was am längsten dauert).
pub fn aggregate(items: impl IntoIterator<Item = Progress>) -> Option<Progress> {
    use crate::vtnotify::ProgressState::*;
    let rank = |p: &Progress| match p.state {
        Error => 3,
        Paused => 2,
        Normal => 1,
        Indeterminate => 0,
    };
    items.into_iter().fold(None, |best: Option<Progress>, p| match best {
        None => Some(p),
        Some(b) if rank(&p) > rank(&b) || (rank(&p) == rank(&b) && p.percent < b.percent) => Some(p),
        b => b,
    })
}

#[cfg(windows)]
mod imp {
    use crate::vtnotify::{Progress, ProgressState};
    use std::ffi::c_void;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};

    const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
    const IID_ITASKBAR_LIST3: GUID = GUID::from_u128(0xea1afb91_9e28_4b86_90e9_9e9f8a5eefaf);
    const TBPF_NOPROGRESS: i32 = 0;
    const TBPF_INDETERMINATE: i32 = 1;
    const TBPF_NORMAL: i32 = 2;
    const TBPF_ERROR: i32 = 4;
    const TBPF_PAUSED: i32 = 8;

    type Hwnd = *mut c_void;

    /// Vtable von ITaskbarList3 bis `SetProgressState` (IUnknown, ITaskbarList, ITaskbarList2, ITaskbarList3).
    #[repr(C)]
    struct Vtbl {
        _query_interface: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        hr_init: unsafe extern "system" fn(*mut c_void) -> i32,
        _add_tab: usize,
        _delete_tab: usize,
        _activate_tab: usize,
        _set_active_alt: usize,
        _mark_fullscreen_window: usize,
        set_progress_value: unsafe extern "system" fn(*mut c_void, Hwnd, u64, u64) -> i32,
        set_progress_state: unsafe extern "system" fn(*mut c_void, Hwnd, i32) -> i32,
    }

    #[derive(Default)]
    pub struct Com {
        ptr: Option<std::ptr::NonNull<c_void>>,
        /// Anlegen schlug fehl: nicht jedes Bild neu versuchen.
        failed: bool,
    }

    impl Com {
        fn get(&mut self) -> Option<*mut c_void> {
            if let Some(p) = self.ptr {
                return Some(p.as_ptr());
            }
            if self.failed {
                return None;
            }
            self.failed = true;
            let mut obj: *mut c_void = std::ptr::null_mut();
            // SAFETY: Standard-COM-Aufrufe mit gültigen Zeigern; CoInitializeEx ist bei schon initialisiertem Thread harmlos
            // (S_FALSE bzw. RPC_E_CHANGED_MODE), das Objekt wird nur in diesem Thread benutzt.
            unsafe {
                let _ = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
                let hr = CoCreateInstance(&CLSID_TASKBAR_LIST, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ITASKBAR_LIST3, &mut obj);
                if hr < 0 || obj.is_null() {
                    return None;
                }
                let vt = &**(obj as *mut *const Vtbl);
                if (vt.hr_init)(obj) < 0 {
                    (vt.release)(obj);
                    return None;
                }
            }
            self.failed = false;
            self.ptr = std::ptr::NonNull::new(obj);
            Some(obj)
        }

        pub fn set(&mut self, hwnd: isize, p: Option<Progress>) -> bool {
            let Some(obj) = self.get() else { return true };
            let (flag, pct) = match p {
                None => (TBPF_NOPROGRESS, None),
                Some(p) => match p.state {
                    ProgressState::Normal => (TBPF_NORMAL, Some(p.percent)),
                    ProgressState::Error => (TBPF_ERROR, Some(p.percent)),
                    ProgressState::Paused => (TBPF_PAUSED, Some(p.percent)),
                    ProgressState::Indeterminate => (TBPF_INDETERMINATE, None),
                },
            };
            // SAFETY: `obj` ist ein gültiges ITaskbarList3 (siehe `get`); ein ungültiges Fenster liefert nur einen Fehlercode.
            unsafe {
                let vt = &**(obj as *mut *const Vtbl);
                let h = hwnd as Hwnd;
                let mut ok = (vt.set_progress_state)(obj, h, flag) >= 0;
                if let Some(v) = pct {
                    ok &= (vt.set_progress_value)(obj, h, v as u64, 100) >= 0;
                }
                ok
            }
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if let Some(p) = self.ptr.take() {
                // SAFETY: einmalige Freigabe des selbst angelegten Objekts.
                unsafe {
                    let vt = &**(p.as_ptr() as *mut *const Vtbl);
                    (vt.release)(p.as_ptr());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vtnotify::ProgressState::*;

    fn p(state: crate::vtnotify::ProgressState, percent: u8) -> Progress {
        Progress { state, percent }
    }

    #[test]
    fn aggregation_prefers_the_most_severe_state() {
        assert_eq!(aggregate([]), None);
        assert_eq!(aggregate([p(Normal, 40), p(Normal, 20), p(Indeterminate, 0)]), Some(p(Normal, 20)));
        assert_eq!(aggregate([p(Normal, 40), p(Error, 90), p(Paused, 10)]), Some(p(Error, 90)));
        assert_eq!(aggregate([p(Indeterminate, 0), p(Paused, 50)]), Some(p(Paused, 50)));
        assert_eq!(aggregate([p(Indeterminate, 0)]), Some(p(Indeterminate, 0)));
    }

    #[test]
    fn only_changes_are_applied() {
        let mut t = Taskbar { applied: HashMap::new(), #[cfg(windows)] com: imp::Com::default() };
        // Ohne Fortschritt nichts setzen (auch keine COM-Anlage), aber merken.
        t.update(&[(1, None)]);
        assert_eq!(t.applied.get(&1), Some(&None));
        // Verschwundene Fenster werden vergessen.
        t.update(&[(2, None)]);
        assert!(!t.applied.contains_key(&1));
    }
}
