//! Clipboard images. Text goes through egui; only images need the system clipboard directly (Ctrl+V of a screenshot
//! into a terminal, see `crate::clip`).

/// The image in the clipboard, if it holds one and no text. `None` on other platforms.
#[cfg(windows)]
pub fn image() -> Option<image::RgbaImage> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if cb.get_text().map(|t| !t.is_empty()).unwrap_or(false) {
        return None;
    }
    let img = cb.get_image().ok()?;
    image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
}

#[cfg(not(windows))]
pub fn image() -> Option<image::RgbaImage> {
    None
}
