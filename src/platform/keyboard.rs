//! Keyboard state, independent of egui's events. Needed where egui-winit swallows or flattens a key press: Ctrl+V only
//! becomes `Event::Paste` when the clipboard holds text, egui does not say whether a digit came from the numeric keypad,
//! and the AltGr level of the keyboard layout (#249) is not part of egui's key events.

/// Keys the terminal asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Control,
    Alt,
    V,
    Insert,
}

/// Windows: ist die Taste (virtueller Tastencode) gerade gedrückt?
#[cfg(windows)]
fn vk_down(vk: u16) -> bool {
    // SAFETY: reine Abfrage des Tastenzustands, keine Zeiger.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) < 0 }
}

/// Is the key held down right now? Always `false` on platforms without an implementation.
#[cfg(windows)]
pub fn is_down(key: Key) -> bool {
    vk_down(match key {
        Key::Control => 0x11, // VK_CONTROL
        Key::Alt => 0x12,     // VK_MENU
        Key::V => b'V' as u16,
        Key::Insert => 0x2D, // VK_INSERT
    })
}

#[cfg(not(windows))]
pub fn is_down(_key: Key) -> bool {
    false
}

/// Taste des Ziffernblocks? egui meldet den Ort nicht (Ziffernblock-5 = `Key::Num5`); unter Windows zeigt der
/// Tastenzustand von VK_NUMPAD0…9 / VK_MULTIPLY / VK_ADD / VK_SUBTRACT / VK_DECIMAL / VK_DIVIDE, woher das Zeichen kam
/// (NumLock an – sonst wären es Pfeil- und Bildtasten). Always `false` on platforms without an implementation.
#[cfg(windows)]
pub fn numpad_down(c: char) -> bool {
    let vk = match c {
        '0'..='9' => 0x60 + (c as u16 - '0' as u16),
        '*' => 0x6A,
        '+' => 0x6B,
        '-' => 0x6D,
        '.' | ',' => 0x6E,
        '/' => 0x6F,
        _ => return false,
    };
    vk_down(vk)
}

#[cfg(not(windows))]
pub fn numpad_down(_c: char) -> bool {
    false
}

/// Zeichen der aktuellen Tastaturbelegung für die Taste mit Scancode `sc` bei gedrücktem Strg+Alt (= AltGr-Ebene) und
/// ggf. Umschalt (#249). `ToUnicodeEx` mit Flag 4 lässt den Zustand der Tottasten unberührt. Nur druckbare Zeichen;
/// eine Tottaste oder eine Belegung ohne AltGr-Ebene ergibt `None` (also auch auf Systemen ohne Umsetzung).
#[cfg(windows)]
pub fn altgr_char(sc: u16, shift: bool) -> Option<String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, GetKeyboardLayout, MapVirtualKeyExW, ToUnicodeEx, MAPVK_VSC_TO_VK_EX};
    // SAFETY: reine Abfragen; `state` und `buf` leben während der Aufrufe, die Längen stimmen.
    unsafe {
        let hkl = GetKeyboardLayout(0);
        let vk = MapVirtualKeyExW(sc as u32, MAPVK_VSC_TO_VK_EX, hkl);
        if vk == 0 {
            return None;
        }
        let mut state = [0u8; 256];
        state[0x11] = 0x80; // VK_CONTROL
        state[0x12] = 0x80; // VK_MENU
        if shift {
            state[0x10] = 0x80; // VK_SHIFT
        }
        state[0x14] = (GetKeyState(0x14) & 1) as u8; // VK_CAPITAL (eingerastet)
        let mut buf = [0u16; 8];
        let n = ToUnicodeEx(vk, sc as u32, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 4, hkl);
        if n <= 0 {
            return None;
        }
        let s = String::from_utf16(&buf[..n as usize]).ok()?;
        (!s.is_empty() && s.chars().all(|c| !c.is_control())).then_some(s)
    }
}

#[cfg(not(windows))]
pub fn altgr_char(_sc: u16, _shift: bool) -> Option<String> {
    None
}
