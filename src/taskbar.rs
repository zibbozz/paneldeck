//! Fortschritt im Taskleisten-Knopf (ITaskbarList3, #183): zeigt den Fortschritt, den Programme per OSC 9;4 melden.
//! Die COM-Schnittstelle wird von Hand angesprochen (windows-sys enthält keine COM-Schnittstellen); sie läuft im
//! UI-Thread (STA, von winit für Drag & Drop ohnehin initialisiert). Gesetzt wird bei einer Änderung; ein gesetzter
//! Fortschritt wird zusätzlich alle `REFRESH` erneuert (nach einem Neustart des Explorers oder bevor der Taskleisten-Knopf
//! existierte, wäre er sonst weg). Fehlschläge werden mit wachsendem Abstand wiederholt, das COM-Objekt dabei neu angelegt.
//! Anderswo ohne Wirkung.

use crate::vtnotify::Progress;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Ein gesetzter Fortschritt wird in diesem Abstand erneut gemeldet.
const REFRESH: Duration = Duration::from_secs(15);
/// Wartezeit nach einem Fehlschlag: beginnt bei `RETRY_MIN`, verdoppelt sich bis `RETRY_MAX`.
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// Stand eines Fensters.
#[derive(Debug, Clone)]
struct WinState {
    /// Zuletzt erfolgreich gemeldet (`None` = kein Fortschritt).
    applied: Option<Option<Progress>>,
    /// Nächster Versuch bzw. nächste Erneuerung.
    next: Instant,
    /// Aktuelle Wartezeit nach Fehlschlägen.
    backoff: Duration,
}

/// Zustand der Taskleisten-Anzeige je Fenster (HWND als `isize`).
pub struct Taskbar {
    wins: HashMap<isize, WinState>,
    #[cfg(windows)]
    com: imp::Com,
}

impl Default for Taskbar {
    fn default() -> Self {
        Taskbar {
            wins: HashMap::new(),
            #[cfg(windows)]
            com: imp::Com::default(),
        }
    }
}

/// Muss jetzt gemeldet werden? (Änderung sofort, außer während einer Wartezeit; sonst nur die fällige Erneuerung eines
/// gesetzten Fortschritts.)
fn due(st: &WinState, want: Option<Progress>, now: Instant) -> bool {
    match st.applied {
        Some(a) if a == want => want.is_some() && now >= st.next,
        Some(_) => now >= st.next || st.backoff == Duration::ZERO,
        None => now >= st.next,
    }
}

impl Taskbar {
    /// Fortschritt der Fenster setzen (`None` = keiner). Fenster, die nicht mehr genannt werden, werden vergessen.
    pub fn update(&mut self, wins: &[(isize, Option<Progress>)]) {
        let now = Instant::now();
        self.wins.retain(|h, _| wins.iter().any(|(w, _)| w == h));
        for &(hwnd, p) in wins {
            // Ein noch nie gesetztes Fenster ohne Fortschritt braucht keinen Aufruf.
            let st = self.wins.entry(hwnd).or_insert(WinState { applied: if p.is_none() { Some(None) } else { None }, next: now, backoff: Duration::ZERO });
            if !due(st, p, now) {
                continue;
            }
            let ok = {
                #[cfg(windows)]
                {
                    self.com.set(hwnd, p)
                }
                #[cfg(not(windows))]
                {
                    true
                }
            };
            let st = self.wins.get_mut(&hwnd).expect("eben eingefügt");
            if ok {
                *st = WinState { applied: Some(p), next: now + REFRESH, backoff: Duration::ZERO };
            } else {
                st.backoff = (st.backoff * 2).clamp(RETRY_MIN, RETRY_MAX);
                st.next = now + st.backoff;
                st.applied = None;
                #[cfg(windows)]
                self.com.reset();
            }
        }
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

    /// Das COM-Objekt (lazy angelegt; nach einem Fehlschlag verworfen und beim nächsten Versuch neu angelegt – die
    /// Wartezeit regelt `Taskbar`).
    #[derive(Default)]
    pub struct Com {
        ptr: Option<std::ptr::NonNull<c_void>>,
    }

    impl Com {
        fn get(&mut self) -> Option<*mut c_void> {
            if let Some(p) = self.ptr {
                return Some(p.as_ptr());
            }
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
            self.ptr = std::ptr::NonNull::new(obj);
            Some(obj)
        }

        pub fn set(&mut self, hwnd: isize, p: Option<Progress>) -> bool {
            let Some(obj) = self.get() else { return false };
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

    impl Com {
        /// Objekt verwerfen (z. B. nach einem Neustart des Explorers); das nächste `set` legt es neu an.
        pub fn reset(&mut self) {
            if let Some(p) = self.ptr.take() {
                // SAFETY: einmalige Freigabe des selbst angelegten Objekts.
                unsafe {
                    let vt = &**(p.as_ptr() as *mut *const Vtbl);
                    (vt.release)(p.as_ptr());
                }
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
    fn scheduling() {
        let t0 = Instant::now();
        let pg = Some(p(Normal, 10));
        let st = |applied, next, backoff| WinState { applied, next, backoff };
        // Gleich und noch nicht fällig: nichts; gesetzter Fortschritt wird nach REFRESH erneuert, „kein Fortschritt“ nie.
        assert!(!due(&st(Some(pg), t0 + REFRESH, Duration::ZERO), pg, t0));
        assert!(due(&st(Some(pg), t0, Duration::ZERO), pg, t0 + Duration::from_millis(1)));
        assert!(!due(&st(Some(None), t0, Duration::ZERO), None, t0 + REFRESH * 10));
        // Änderung: sofort, außer in einer Wartezeit nach einem Fehlschlag.
        assert!(due(&st(Some(None), t0 + REFRESH, Duration::ZERO), pg, t0));
        assert!(!due(&st(None, t0 + Duration::from_secs(4), Duration::from_secs(4)), pg, t0));
        assert!(due(&st(None, t0, Duration::from_secs(4)), pg, t0 + Duration::from_secs(4)));
    }

    #[test]
    fn windows_without_progress_need_no_call_and_are_forgotten() {
        let mut t = Taskbar::default();
        t.update(&[(1, None)]);
        assert_eq!(t.wins.get(&1).map(|s| s.applied), Some(Some(None)));
        t.update(&[(2, None)]);
        assert!(!t.wins.contains_key(&1));
    }
}
