//! Die Screenshots der Abfrage-Hilfe (ein Bild je Beispiel, nur Englisch), in die `.exe` eingebettet.
//!
//! **Neuen Screenshot ergänzen:** Datei `assets/query-help/query-help-<id>.png` ablegen (`<id>` = `Example::id`, der Name ist
//! `Example::screenshot_file()`) und unten in [`SHOTS`] eine Zeile `("<id>", include_bytes!("../../../assets/query-help/query-help-<id>.png"))`
//! eintragen. Beispiele ohne Eintrag zeigen den Platzhalter „Screenshot folgt“. Der Test `table_matches_the_directory` stellt sicher,
//! dass Ordner und Tabelle übereinstimmen und jedes Bild lesbar ist.

use eframe::egui::ColorImage;

/// Beispiel-ID → PNG-Daten. Noch gibt es keine Screenshots; die Bilder entstehen später aus der gebauten Software.
pub const SHOTS: &[(&str, &[u8])] = &[
    // ("epics-release", include_bytes!("../../../assets/query-help/query-help-epics-release.png")),
];

/// Die eingebetteten PNG-Daten des Beispiels, falls es einen Screenshot gibt.
pub fn shot_bytes(example_id: &str) -> Option<&'static [u8]> {
    SHOTS.iter().find(|(id, _)| *id == example_id).map(|(_, b)| *b)
}

/// PNG → Bild für die Textur (`None`, wenn es nicht lesbar ist).
pub fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    if size[0] == 0 || size[1] == 0 {
        return None;
    }
    Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::query_help::HelpDoc;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_a_png_and_rejects_garbage() {
        let img = decode(&png(3, 2)).expect("PNG");
        assert_eq!(img.size, [3, 2]);
        assert!(decode(b"not a png").is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn table_matches_the_directory() {
        let doc = HelpDoc::builtin();
        // jeder Eintrag gehört zu einem Beispiel, ist nur einmal da und lesbar
        for (i, (id, bytes)) in SHOTS.iter().enumerate() {
            let ex = doc.example(id).unwrap_or_else(|| panic!("kein Beispiel {id}"));
            assert!(SHOTS[i + 1..].iter().all(|(o, _)| o != id), "doppelt: {id}");
            assert!(decode(bytes).is_some(), "{} ist kein lesbares PNG", ex.screenshot_file());
        }
        // jede Datei im Ordner hat einen Eintrag (der Ordner darf noch fehlen)
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("query-help");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".png") {
                continue;
            }
            let ex = doc.examples().into_iter().find(|x| x.screenshot_file() == name);
            let ex = ex.unwrap_or_else(|| panic!("{name} gehört zu keinem Beispiel"));
            assert!(shot_bytes(ex.id).is_some(), "{name} liegt im Ordner, steht aber nicht in SHOTS");
        }
    }

    #[test]
    fn lookup_works() {
        assert!(shot_bytes("does-not-exist").is_none());
    }
}
