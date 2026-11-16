//! Physical key state, independent of egui's events. Needed where egui-winit swallows a key press: Ctrl+V only becomes
//! `Event::Paste` when the clipboard holds text, so an image paste has to look at the keys themselves.

/// Keys the terminal asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Control,
    Alt,
    V,
    Insert,
}

/// Is the key held down right now? Always `false` on platforms without an implementation.
#[cfg(windows)]
pub fn is_down(key: Key) -> bool {
    let vk: u16 = match key {
        Key::Control => 0x11, // VK_CONTROL
        Key::Alt => 0x12,     // VK_MENU
        Key::V => b'V' as u16,
        Key::Insert => 0x2D, // VK_INSERT
    };
    // SAFETY: reine Abfrage des Tastenzustands, keine Zeiger.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) < 0 }
}

#[cfg(not(windows))]
pub fn is_down(_key: Key) -> bool {
    false
}
