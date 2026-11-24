//! Dateien eines Theme-Hintergrunds (#387): Ordner `themes/<datei>/` anlegen, kopieren (Duplizieren), löschen,
//! aufräumen und Export/Import als ZIP (`theme.json` + Bilder). Nur bloße, sichere Dateinamen; Symlinks werden nie
//! verfolgt; ZIP-Einträge mit Pfaden (Zip-Slip) werden abgelehnt. Kennt kein egui.

use super::background::{folder_of, real_dir, real_file, safe_file_name};
use super::file::{self, Msg};
use super::Theme;
use std::path::Path;

/// Größte ZIP-Datei beim Import.
pub const MAX_ZIP_BYTES: u64 = 160 * 1024 * 1024;
/// Höchstens so viel entpackt (Theme + zwei Bilder zu je 64 MB).
const MAX_UNPACKED: usize = (1 + 2 * 64) * 1024 * 1024;
/// Name des Themes im ZIP.
pub const ZIP_THEME: &str = "theme.json";

fn io_msg(e: impl std::fmt::Display) -> Msg {
    Msg::new(format!("Datei nicht lesbar: {e}"), format!("Cannot read the file: {e}"))
}

/// Ordner eines Themes anlegen (darf kein Symlink sein).
fn ensure_folder(source: &Path) -> Result<std::path::PathBuf, String> {
    let dir = folder_of(source).ok_or("no folder")?;
    if std::fs::symlink_metadata(&dir).is_ok() {
        if !real_dir(&dir) {
            return Err(format!("{}: {}", dir.display(), crate::state::t("kein echter Ordner", "not a real folder")));
        }
    } else {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

/// Bilder in den Ordner des Themes `source` schreiben (atomar; nur sichere Namen).
pub fn write_files(source: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let dir = ensure_folder(source)?;
    for (name, bytes) in files {
        if !safe_file_name(name) {
            return Err(format!("{}: {name}", crate::state::t("Ungültiger Dateiname", "Invalid file name")));
        }
        crate::state::write_atomic(&dir.join(name), bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Bilder, auf die das Theme nicht mehr verweist, löschen: nur Namen aus `old` (der zuvor gespeicherte Stand), die nicht
/// in `keep` stehen. Leerer Ordner wird entfernt.
pub fn remove_unused(source: &Path, old: &[String], keep: &[&str]) {
    let Some(dir) = folder_of(source).filter(|d| real_dir(d)) else { return };
    for n in old {
        if safe_file_name(n) && !keep.iter().any(|k| k == n) {
            let p = dir.join(n);
            if real_file(&p) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let _ = std::fs::remove_dir(&dir); // nur, wenn leer
}

/// Ordner eines Themes für eine Kopie (`to`) übernehmen: nur echte Dateien mit sicheren Namen, keine Unterordner.
pub fn copy_folder(from: &Path, to: &Path) -> Result<(), String> {
    let Some(src) = folder_of(from).filter(|d| real_dir(d)) else { return Ok(()) };
    let mut files = Vec::new();
    for e in std::fs::read_dir(&src).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let p = e.path();
        if safe_file_name(&name) && real_file(&p) {
            files.push((name, std::fs::read(&p).map_err(|e| e.to_string())?));
        }
    }
    write_files(to, &files)
}

/// Ordner eines gelöschten Themes entfernen: nur echte Dateien mit sicheren Namen, dann den (leeren) Ordner. Ein Symlink
/// oder ein Ordner mit anderem Inhalt bleibt stehen.
pub fn remove_folder(source: &Path) {
    let Some(dir) = folder_of(source).filter(|d| real_dir(d)) else { return };
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            if safe_file_name(&name) && real_file(&p) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let _ = std::fs::remove_dir(&dir);
}

/// Dateinamen im Ordner des Themes (für freie Namen neuer Bilder).
pub fn names_in_folder(source: Option<&Path>) -> Vec<String> {
    let Some(dir) = source.and_then(folder_of).filter(|d| real_dir(d)) else { return Vec::new() };
    std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default()
}

/// Vorhandene Bilder eines gespeicherten Themes (Name, Inhalt).
pub fn existing_files(t: &Theme) -> Vec<(String, Vec<u8>)> {
    let (Some(b), Some(src)) = (&t.background, &t.source) else { return Vec::new() };
    b.files().into_iter().filter_map(|n| super::background::resolve_file(src, n).and_then(|p| std::fs::read(p).ok()).map(|d| (n.to_string(), d))).collect()
}

/// ZIP mit `theme.json` und den Bildern des Themes schreiben.
pub fn export_zip(t: &Theme, out: &Path) -> Result<(), String> {
    let mut entries = vec![(ZIP_THEME.to_string(), file::to_json(t).into_bytes())];
    entries.extend(existing_files(t));
    crate::cleanup::zip::write(out, entries).map(|_| ())
}

/// Ein gelesenes Theme mit seinen Bildern (noch nicht gespeichert).
pub struct Imported {
    pub theme: Theme,
    pub files: Vec<(String, Vec<u8>)>,
}

/// Theme-Datei für den Import lesen: `.json` wie bisher, `.zip` mit `theme.json` und Bildern ([`import_zip`]).
pub fn read_import(path: &Path) -> Result<Imported, Msg> {
    let zip = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if !zip {
        let mut theme = file::read_file(path)?;
        theme.source = None;
        return Ok(Imported { theme, files: Vec::new() });
    }
    let meta = std::fs::metadata(path).map_err(io_msg)?;
    if meta.len() > MAX_ZIP_BYTES {
        return Err(Msg::new(format!("Die ZIP-Datei ist größer als {} MB", MAX_ZIP_BYTES >> 20), format!("The ZIP file is larger than {} MB", MAX_ZIP_BYTES >> 20)));
    }
    import_zip(&std::fs::read(path).map_err(io_msg)?)
}

/// Theme aus einem ZIP lesen. Erlaubt sind nur `theme.json` und Bilder mit bloßem, sicherem Namen, auf die das Theme
/// verweist – keine Ordner, keine Pfade (Zip-Slip), keine Symlinks, höchstens 3 Einträge; Größen begrenzt, jedes Bild
/// muss sich dekodieren lassen (es wird wie beim Wählen aufbereitet, höchstens 3840 px).
pub fn import_zip(bytes: &[u8]) -> Result<Imported, Msg> {
    let bad = |de: String, en: String| Msg::new(de, en);
    let entries = crate::app::update::zip::extract_tree(bytes, "", MAX_UNPACKED).map_err(|e| bad(format!("ZIP-Datei abgelehnt: {e}"), format!("ZIP file refused: {e}")))?;
    if entries.len() > 3 {
        return Err(bad("Zu viele Dateien im ZIP (erwartet theme.json und höchstens zwei Bilder)".into(), "Too many files in the ZIP (expected theme.json and at most two images)".into()));
    }
    let mut json = None;
    let mut imgs: Vec<(String, Vec<u8>)> = Vec::new();
    for e in entries {
        if e.dir || e.path.contains('/') {
            let p = file::short(&e.path);
            return Err(bad(format!("Unerwarteter Eintrag im ZIP: „{p}“"), format!("Unexpected entry in the ZIP: “{p}”")));
        }
        if e.path == ZIP_THEME {
            if e.data.len() as u64 > file::MAX_FILE_BYTES {
                return Err(Msg::new("Die Datei ist größer als 1 MB", "The file is larger than 1 MB"));
            }
            json = Some(e.data);
        } else if safe_file_name(&e.path) {
            imgs.push((e.path, e.data));
        } else {
            let p = file::short(&e.path);
            return Err(bad(format!("Unerwarteter Eintrag im ZIP: „{p}“"), format!("Unexpected entry in the ZIP: “{p}”")));
        }
    }
    let json = json.ok_or_else(|| Msg::new("theme.json fehlt im ZIP", "theme.json is missing in the ZIP"))?;
    let text = String::from_utf8(json).map_err(|_| Msg::new("Die Datei ist kein UTF-8-Text", "The file is not UTF-8 text"))?;
    let mut theme = file::parse_str(&text)?;
    let wanted: Vec<String> = theme.background.as_ref().map(|b| b.files().iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, data) in imgs {
        if !wanted.iter().any(|w| *w == name) {
            let p = file::short(&name);
            return Err(bad(format!("Das Theme verweist nicht auf „{p}“"), format!("The theme does not refer to “{p}”")));
        }
        let prep = super::bake::prepare(data).map_err(|e| bad(format!("„{}“: {e}", file::short(&name)), format!("“{}”: {e}", file::short(&name))))?;
        // Andere Endung nach dem Aufbereiten (z. B. WebP → JPEG): Name und Verweise anpassen.
        let same = std::path::Path::new(&name).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(prep.ext) || (prep.ext == "jpg" && e.eq_ignore_ascii_case("jpeg")));
        let new = if same {
            name.clone()
        } else {
            let taken: Vec<String> = files.iter().map(|f| f.0.clone()).chain(wanted.iter().cloned()).collect();
            super::bake::file_name_for(&name, prep.ext, &taken)
        };
        if new != name {
            if let Some(b) = theme.background.as_mut() {
                if b.image.as_deref() == Some(name.as_str()) {
                    b.image = Some(new.clone());
                }
                if let Some(p) = b.pattern.as_mut().filter(|p| p.image.as_deref() == Some(name.as_str())) {
                    p.image = Some(new.clone());
                }
            }
        }
        files.push((new, prep.bytes));
    }
    theme.source = None;
    Ok(Imported { theme, files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::background::{Background, Kind};

    fn png() -> Vec<u8> {
        crate::chat::images::encode_png(&image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])))).unwrap()
    }

    fn theme_with_image(name: &str) -> Theme {
        let mut t = crate::theme::all()[0].clone();
        t.builtin = false;
        t.name = "Bg".into();
        let mut b = Background::new(Kind::Image);
        b.image = Some(name.into());
        t.background = Some(b);
        t
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("paneldeck-test-bgfiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn export_and_import_round_trip() {
        let d = tmp("zip");
        let mut t = theme_with_image("a.png");
        t.source = Some(d.join("bg.json"));
        file::write(&d.join("bg.json"), &t).unwrap();
        write_files(&d.join("bg.json"), &[("a.png".into(), png())]).unwrap();
        let out = d.join("export.zip");
        export_zip(&t, &out).unwrap();
        let imp = read_import(&out).unwrap();
        assert_eq!(imp.theme.background, t.background);
        assert_eq!(imp.files.len(), 1);
        assert_eq!(imp.files[0].0, "a.png");
        assert!(imp.theme.source.is_none());
        // Ohne Bilder bleibt der Export eine JSON-Datei; JSON-Import wie bisher.
        file::write(&d.join("plain.json"), &crate::theme::all()[0]).unwrap();
        assert!(read_import(&d.join("plain.json")).unwrap().files.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn zip_import_refuses_paths_extra_files_and_broken_images() {
        use crate::app::update::zip::tests::make_zip;
        let t = theme_with_image("a.png");
        let json = file::to_json(&t);
        let img = png();
        // Zip-Slip und Ordner.
        for evil in ["../a.png", "..\\a.png", "sub/a.png", "C:a.png", "/abs.png"] {
            let z = make_zip(&[("theme.json", json.as_bytes(), false), (evil, &img, false)]);
            assert!(import_zip(&z).is_err(), "{evil}");
        }
        // Nicht verwiesene Datei, zu viele Dateien, fehlendes theme.json, kaputtes Bild.
        assert!(import_zip(&make_zip(&[("theme.json", json.as_bytes(), false), ("other.png", &img, false)])).is_err());
        assert!(import_zip(&make_zip(&[("theme.json", json.as_bytes(), false), ("a.png", &img, false), ("b.png", &img, false), ("c.png", &img, false)])).is_err());
        assert!(import_zip(&make_zip(&[("a.png", &img, false)])).is_err());
        assert!(import_zip(&make_zip(&[("theme.json", json.as_bytes(), false), ("a.png", b"nope", false)])).is_err());
        assert!(import_zip(b"not a zip").is_err());
        // Gut: Bild wird übernommen; ein BMP wird aufbereitet und der Verweis umbenannt.
        let ok = import_zip(&make_zip(&[("theme.json", json.as_bytes(), true), ("a.png", &img, true)])).unwrap();
        assert_eq!(ok.files[0].0, "a.png");
        let mut bmp = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]))).write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp).unwrap();
        let tb = theme_with_image("x.bmp");
        let jb = file::to_json(&tb);
        let ok = import_zip(&make_zip(&[("theme.json", jb.as_bytes(), false), ("x.bmp", &bmp, false)])).unwrap();
        assert_eq!(ok.files[0].0, "x.jpg");
        assert_eq!(ok.theme.background.unwrap().image.as_deref(), Some("x.jpg"));
    }

    #[test]
    fn folders_follow_duplicate_and_delete() {
        let d = tmp("folder");
        let a = d.join("a.json");
        write_files(&a, &[("one.png".into(), b"1".to_vec()), ("two.jpg".into(), b"2".to_vec())]).unwrap();
        std::fs::write(d.join("a").join("notes.txt"), b"keep").unwrap();
        copy_folder(&a, &d.join("b.json")).unwrap();
        assert!(d.join("b").join("one.png").is_file() && d.join("b").join("two.jpg").is_file());
        assert!(!d.join("b").join("notes.txt").exists(), "nur Bilder");
        remove_unused(&d.join("b.json"), &["one.png".into(), "two.jpg".into()], &["two.jpg"]);
        assert!(!d.join("b").join("one.png").exists() && d.join("b").join("two.jpg").exists());
        remove_folder(&a);
        assert!(!d.join("a").join("one.png").exists());
        assert!(d.join("a").join("notes.txt").exists(), "fremde Dateien bleiben");
        remove_folder(&d.join("b.json"));
        assert!(!d.join("b").exists(), "leer → entfernt");
        assert!(write_files(&a, &[("../x.png".into(), b"x".to_vec())]).is_err());
        let mut names = names_in_folder(Some(&a));
        names.sort();
        assert_eq!(names, ["notes.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
