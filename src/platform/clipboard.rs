//! System clipboard where egui is not enough: reading an image (Ctrl+V of a screenshot into a terminal, see
//! `crate::clip`) and writing text from OSC 52 (#181). Text pastes go through egui.

/// The image in the clipboard, if it holds one and no text (Windows; Linux/macOS through arboard – X11, on Wayland via
/// XWayland).
pub fn image() -> Option<image::RgbaImage> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if cb.get_text().map(|t| !t.is_empty()).unwrap_or(false) {
        return None;
    }
    let img = cb.get_image().ok()?;
    image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
}

/// Put text into the system clipboard. `false` if that failed (e.g. locked by another program) or there is no
/// implementation – the caller then goes through egui (Linux/macOS: always egui; on X11 a clipboard owned by a
/// short-lived arboard instance could vanish with it).
#[cfg(windows)]
pub fn set_text(text: &str) -> bool {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).is_ok()
}

#[cfg(not(windows))]
pub fn set_text(_text: &str) -> bool {
    false
}
