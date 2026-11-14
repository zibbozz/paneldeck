//! Bilder aus der Zwischenablage als PNG-Datei ablegen (damit Claude sie per Pfad lesen kann).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn init(data_dir: &Path) {
    let d = data_dir.join("clipboard");
    let _ = std::fs::create_dir_all(&d);
    // Alte Dateien (> 14 Tage) entfernen.
    if let Ok(rd) = std::fs::read_dir(&d) {
        for f in rd.flatten() {
            let old = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|e| e.as_secs() > 14 * 86400);
            if old {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
    let _ = DIR.set(d);
}

/// Liefert den Pfad einer neuen PNG-Datei, wenn die Zwischenablage ein Bild (und keinen Text) enthält.
#[cfg(windows)]
pub fn image_to_file() -> Option<PathBuf> {
    let dir = DIR.get()?;
    let mut cb = arboard::Clipboard::new().ok()?;
    if cb.get_text().map(|t| !t.is_empty()).unwrap_or(false) {
        return None;
    }
    let img = cb.get_image().ok()?;
    let rgba = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())?;
    // Millisekunden + laufende Nummer: zwei Bilder in derselben Sekunde überschreiben sich nicht.
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("clip-{ms}-{n}.png"));
    rgba.save(&path).ok()?;
    Some(path)
}

#[cfg(not(windows))]
pub fn image_to_file() -> Option<PathBuf> {
    None
}
