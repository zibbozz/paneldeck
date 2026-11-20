//! System fonts: the system font folder (fallback fonts for symbols, CJK, Indic, maths, and the optional classic terminal
//! fonts) and the installed font families for the terminal font choice (#183, `crate::sysfonts`).
//!
//! Windows: `C:\Windows\Fonts` and GDI (`EnumFontFamiliesExW`, `GetFontData`). Other platforms have neither these file
//! names nor this folder: no system font is found there and egui's built-in fonts plus the embedded Cascadia Mono are
//! used. The installed families and their files come from fontconfig there (`fc-list`/`fc-match`, #149).
//! macOS (#150): fontconfig is not part of the system – the font folders are read directly ([`scan`]: only the table
//! directory and the `name`/`OS/2`/`post` tables of each file, with ttf-parser), and `system_font` looks the fallback
//! fonts up in `/System/Library/Fonts` (+ `Supplemental`) and `/Library/Fonts`.

use std::path::PathBuf;

/// Full path of a font file in the system font folder (Windows: `C:\Windows\Fonts\<file>`, macOS: the first of
/// `/System/Library/Fonts`, `…/Supplemental`, `/Library/Fonts` that has it); `None` where unknown.
pub fn system_font(file: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(PathBuf::from(format!(r"C:\Windows\Fonts\{file}")))
    }
    #[cfg(target_os = "macos")]
    {
        ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"].iter().map(|d| PathBuf::from(d).join(file)).find(|p| p.is_file())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = file;
        None
    }
}

/// Installed monospace font families (unsorted, may contain duplicates; `sysfonts::monospace_families` cleans up).
#[cfg(windows)]
pub use imp::{families, load};

#[cfg(windows)]
mod imp {
    use crate::sysfonts::FaceData;
    use windows_sys::Win32::Foundation::LPARAM;
    use windows_sys::Win32::Graphics::Gdi::*;

    const TTCF: u32 = 0x6663_7474; // 'ttcf'
    const OS2: u32 = 0x322f_534f; // 'OS/2'
    const GDI_ERROR: u32 = u32::MAX;

    unsafe extern "system" fn enum_proc(lf: *const LOGFONTW, tm: *const TEXTMETRICW, font_type: u32, lp: LPARAM) -> i32 {
        // SAFETY: GDI übergibt gültige Zeiger für die Dauer des Rückrufs; `lp` ist unser `Vec<String>`.
        unsafe {
            let out = &mut *(lp as *mut Vec<String>);
            let lf = &*lf;
            let tm = &*tm;
            // TMPF_FIXED_PITCH gesetzt heißt (historisch verdreht) „variable Breite“.
            let fixed = tm.tmPitchAndFamily & TMPF_FIXED_PITCH as u8 == 0;
            if fixed && font_type & TRUETYPE_FONTTYPE != 0 {
                let len = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(lf.lfFaceName.len());
                let name = String::from_utf16_lossy(&lf.lfFaceName[..len]);
                if !name.is_empty() && !name.starts_with('@') && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        1
    }

    pub fn families() -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        // SAFETY: Speicher-DC nur für die Aufzählung, danach freigegeben; Rückruf schreibt nur in `out`.
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return out;
            }
            let mut lf: LOGFONTW = std::mem::zeroed();
            lf.lfCharSet = DEFAULT_CHARSET as u8;
            EnumFontFamiliesExW(dc, &lf, Some(enum_proc), &mut out as *mut Vec<String> as LPARAM, 0);
            DeleteDC(dc);
        }
        out
    }

    pub fn load(family: &str, bold: bool, italic: bool) -> Option<FaceData> {
        let wide: Vec<u16> = family.encode_utf16().collect();
        if wide.len() >= 32 {
            return None;
        }
        // SAFETY: Speicher-DC und Schrift werden hier angelegt und freigegeben; Puffergrößen kommen von GDI.
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let mut lf: LOGFONTW = std::mem::zeroed();
            lf.lfHeight = -32;
            lf.lfWeight = if bold { FW_BOLD as i32 } else { FW_NORMAL as i32 };
            lf.lfItalic = italic as u8;
            lf.lfCharSet = DEFAULT_CHARSET as u8;
            lf.lfOutPrecision = OUT_TT_ONLY_PRECIS as u8;
            lf.lfFaceName[..wide.len()].copy_from_slice(&wide);
            let font = CreateFontIndirectW(&lf);
            if font.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, font);
            let result = (|| {
                // GDI ersetzt unbekannte Namen stillschweigend durch eine andere Schrift: Namen prüfen.
                let mut face = [0u16; 64];
                let n = GetTextFaceW(dc, face.len() as i32, face.as_mut_ptr());
                if n <= 0 || !String::from_utf16_lossy(&face[..(n as usize).min(face.len())]).trim_end_matches('\0').eq_ignore_ascii_case(family) {
                    return None;
                }
                let read = |table: u32| -> Option<Vec<u8>> {
                    let size = GetFontData(dc, table, 0, std::ptr::null_mut(), 0);
                    if size == GDI_ERROR || size == 0 || size > 64 * 1024 * 1024 {
                        return None;
                    }
                    let mut buf = vec![0u8; size as usize];
                    let got = GetFontData(dc, table, 0, buf.as_mut_ptr().cast(), size);
                    (got == size).then_some(buf)
                };
                // Fett/kursiv gewünscht: erst die kleine OS/2-Tabelle prüfen – ersetzt GDI den Schnitt durch den normalen,
                // wird die (oft mehrere MB große) Datei gar nicht erst gelesen.
                if (bold || italic) && !read(OS2).is_some_and(|t| crate::sysfonts::os2_matches(&t, bold, italic)) {
                    return None;
                }
                match read(TTCF) {
                    // Sammlung: ganze Datei, der gewählte Schnitt beginnt mit denselben Bytes wie seine Offset-Tabelle
                    // (davon genügt der Kopf).
                    Some(all) => {
                        let mut own = vec![0u8; 256];
                        let got = GetFontData(dc, 0, 0, own.as_mut_ptr().cast(), own.len() as u32);
                        if got == GDI_ERROR || got < 12 {
                            return None;
                        }
                        own.truncate(got as usize);
                        let index = crate::sysfonts::collection_index(&all, &own)?;
                        Some(FaceData { bytes: all, index })
                    }
                    None => read(0).map(|bytes| FaceData { bytes, index: 0 }),
                }
            })();
            SelectObject(dc, old);
            DeleteObject(font);
            DeleteDC(dc);
            result
        }
    }
}

/// macOS (#150): font folders read directly (see [`scan`]).
#[cfg(target_os = "macos")]
pub fn families() -> Vec<String> {
    scan::families_in(scan::system_faces())
}

#[cfg(target_os = "macos")]
pub fn load(family: &str, bold: bool, italic: bool) -> Option<crate::sysfonts::FaceData> {
    scan::load_from(scan::system_faces(), family, bold, italic)
}

/// Font folders without fontconfig (macOS, #150; tested on every platform with the bundled fonts). Per file only the
/// table directory and the `name`, `OS/2` and `post` tables are read – the font data itself only for the chosen face.
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod scan {
    use crate::sysfonts::FaceData;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::{Path, PathBuf};

    /// One face of a font file.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Face {
        pub file: PathBuf,
        pub index: u32,
        pub family: String,
        pub mono: bool,
        pub bold: bool,
        pub italic: bool,
    }

    const MAX_FILE: u64 = 64 * 1024 * 1024;

    fn read_at(f: &mut std::fs::File, off: u64, len: usize) -> Option<Vec<u8>> {
        if len > 4 * 1024 * 1024 {
            return None;
        }
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut b = vec![0u8; len];
        f.read_exact(&mut b).ok()?;
        Some(b)
    }

    fn be32(b: &[u8], i: usize) -> Option<u32> {
        Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
    }

    fn be16(b: &[u8], i: usize) -> Option<u16> {
        Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?))
    }

    /// Faces of one font file (`.ttf`, `.otf`, `.ttc`); empty if it is none or unreadable.
    pub fn faces_of(path: &Path) -> Vec<Face> {
        let mut out = Vec::new();
        let Ok(mut f) = std::fs::File::open(path) else { return out };
        if f.metadata().map(|m| m.len() > MAX_FILE).unwrap_or(true) {
            return out;
        }
        let Some(head) = read_at(&mut f, 0, 12) else { return out };
        let offsets: Vec<u32> = if &head[..4] == b"ttcf" {
            let n = be32(&head, 8).unwrap_or(0).min(64) as usize;
            let Some(t) = read_at(&mut f, 12, n * 4) else { return out };
            (0..n).filter_map(|i| be32(&t, i * 4)).collect()
        } else {
            vec![0]
        };
        for (index, off) in offsets.into_iter().enumerate() {
            if let Some(face) = face_at(&mut f, off as u64, path, index as u32) {
                out.push(face);
            }
        }
        out
    }

    fn face_at(f: &mut std::fs::File, off: u64, path: &Path, index: u32) -> Option<Face> {
        let dir = read_at(f, off, 12)?;
        let n = be16(&dir, 4)? as usize;
        if n == 0 || n > 512 {
            return None;
        }
        let recs = read_at(f, off + 12, n * 16)?;
        let table = |f: &mut std::fs::File, tag: &[u8; 4]| -> Option<Vec<u8>> {
            let i = (0..n).find(|i| recs.get(i * 16..i * 16 + 4) == Some(&tag[..]))?;
            read_at(f, be32(&recs, i * 16 + 8)? as u64, be32(&recs, i * 16 + 12)? as usize)
        };
        let name = table(f, b"name")?;
        let names = ttf_parser::name::Table::parse(&name)?;
        let pick = |id: u16| names.names.into_iter().filter(|n| n.name_id == id).find_map(|n| n.to_string()).filter(|s| !s.trim().is_empty());
        let family = pick(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| pick(ttf_parser::name_id::FAMILY))?;
        let mono = table(f, b"post").and_then(|p| ttf_parser::post::Table::parse(&p).map(|t| t.is_monospaced)).unwrap_or(false);
        let (bold, italic) = table(f, b"OS/2")
            .and_then(|o| ttf_parser::os2::Table::parse(&o).map(|t| (t.weight().to_number() >= 600 || t.is_bold(), t.style() != ttf_parser::Style::Normal)))
            .unwrap_or((false, false));
        Some(Face { file: path.to_path_buf(), index, family: family.trim().to_string(), mono, bold, italic })
    }

    /// All faces in these folders (one level of subfolders).
    pub fn scan_dirs(dirs: &[PathBuf]) -> Vec<Face> {
        let mut out = Vec::new();
        let font = |p: &Path| p.extension().is_some_and(|e| ["ttf", "otf", "ttc"].iter().any(|x| e.eq_ignore_ascii_case(x)));
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if let Ok(sub) = std::fs::read_dir(&p) {
                        out.extend(sub.flatten().map(|x| x.path()).filter(|x| font(x)).flat_map(|x| faces_of(&x)));
                    }
                } else if font(&p) {
                    out.extend(faces_of(&p));
                }
            }
        }
        out
    }

    /// The font folders of macOS (system, local, user), read once per run.
    pub fn system_faces() -> &'static [Face] {
        static CACHE: std::sync::OnceLock<Vec<Face>> = std::sync::OnceLock::new();
        CACHE.get_or_init(|| {
            let mut dirs = vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")];
            if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
                dirs.push(PathBuf::from(h).join("Library").join("Fonts"));
            }
            scan_dirs(&dirs)
        })
    }

    /// Monospace families (unsorted). Emoji fonts set the fixed-pitch flag too, but are no terminal font.
    pub fn families_in(faces: &[Face]) -> Vec<String> {
        faces.iter().filter(|f| f.mono && !f.family.to_ascii_lowercase().contains("emoji")).map(|f| f.family.clone()).collect()
    }

    /// Data of the face of `family` (case-insensitive) that fits bold/italic best; the regular face as a substitute.
    pub fn load_from(faces: &[Face], family: &str, bold: bool, italic: bool) -> Option<FaceData> {
        let of: Vec<&Face> = faces.iter().filter(|f| f.family.eq_ignore_ascii_case(family)).collect();
        let face = of
            .iter()
            .find(|f| f.bold == bold && f.italic == italic)
            .or_else(|| of.iter().find(|f| !f.bold && !f.italic))
            .or_else(|| of.first())?;
        let meta = std::fs::metadata(&face.file).ok()?;
        if meta.len() > MAX_FILE {
            return None;
        }
        Some(FaceData { bytes: std::fs::read(&face.file).ok()?, index: face.index })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_names_and_flags_from_font_files() {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("fonts");
            let faces = scan_dirs(std::slice::from_ref(&dir));
            let cascadia = faces.iter().find(|f| f.family == "Cascadia Mono").expect("Cascadia Mono found");
            assert!(cascadia.mono && !cascadia.bold && !cascadia.italic && cascadia.index == 0);
            assert!(families_in(&faces).contains(&"Cascadia Mono".to_string()));
            assert!(!families_in(&faces).iter().any(|f| f.contains("Emoji")), "only monospace families");
            let d = load_from(&faces, "cascadia mono", true, false).expect("regular face as substitute");
            assert!(crate::sysfonts::is_monospace(&d));
            assert!(load_from(&faces, "No Such Font", false, false).is_none());
            assert!(faces_of(&dir.join("LICENSE-Cascadia.txt")).is_empty());
        }
    }
}

/// Linux (#149): fontconfig (`fc-list`, `fc-match`; on every Linux desktop).
#[cfg(all(unix, not(target_os = "macos")))]
pub use fc::{families, load};

#[cfg(all(unix, not(target_os = "macos")))]
mod fc {
    use crate::sysfonts::FaceData;
    use std::process::Command;

    fn run(args: &[&str]) -> Option<String> {
        let out = Command::new("fc-list").args(args).output().ok().filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Family names from `fc-list` output (one face per line, alternative names separated by commas).
    pub fn parse_families(out: &str) -> Vec<String> {
        out.lines()
            .filter_map(|l| l.split(',').next())
            .map(|f| f.replace("\\-", "-").trim().to_string())
            .filter(|f| !f.is_empty())
            .collect()
    }

    /// Monospace (spacing 100) and dual-width (90, e.g. some Nerd Fonts) families.
    pub fn families() -> Vec<String> {
        let mut v = Vec::new();
        for spacing in [":spacing=100", ":spacing=90"] {
            if let Some(out) = run(&[spacing, "family"]) {
                v.extend(parse_families(&out));
            }
        }
        v
    }

    /// Escape a family name for a fontconfig pattern (`-`, `:`, `,` and `\` are special).
    pub fn escape(family: &str) -> String {
        family.chars().flat_map(|c| if matches!(c, '-' | ':' | ',' | '\\') { vec!['\\', c] } else { vec![c] }).collect()
    }

    /// `fc-match` output `file|index|family` → (file, index) if the matched family is the wanted one (fontconfig always
    /// returns *some* font).
    pub fn parse_match(out: &str, family: &str) -> Option<(String, u32)> {
        let mut p = out.splitn(3, '|');
        let (file, index, fams) = (p.next()?, p.next()?, p.next()?);
        if file.is_empty() || !fams.split(',').any(|f| f.trim().eq_ignore_ascii_case(family)) {
            return None;
        }
        Some((file.to_string(), index.trim().parse().ok()?))
    }

    pub fn load(family: &str, bold: bool, italic: bool) -> Option<FaceData> {
        let pattern = format!("{}:weight={}:slant={}", escape(family), if bold { 200 } else { 80 }, if italic { 100 } else { 0 });
        let out = Command::new("fc-match").args(["--format=%{file}|%{index}|%{family}", &pattern]).output().ok().filter(|o| o.status.success())?;
        let (file, index) = parse_match(&String::from_utf8_lossy(&out.stdout), family)?;
        let meta = std::fs::metadata(&file).ok()?;
        if meta.len() > 64 * 1024 * 1024 {
            return None;
        }
        Some(FaceData { bytes: std::fs::read(&file).ok()?, index })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fontconfig_output() {
            assert_eq!(parse_families("DejaVu Sans Mono,DejaVu Sans Mono Book\nHack\n\n"), ["DejaVu Sans Mono", "Hack"]);
            assert_eq!(escape("Fira-Code:x"), "Fira\\-Code\\:x");
            assert_eq!(parse_match("/usr/share/fonts/h.ttf|0|Hack", "hack"), Some(("/usr/share/fonts/h.ttf".into(), 0)));
            assert_eq!(parse_match("/usr/share/fonts/d.ttf|0|DejaVu Sans", "Hack"), None, "substitute is no match");
            assert_eq!(parse_match("/x.ttc|2|Noto Sans Mono CJK JP,Noto Sans Mono CJK", "Noto Sans Mono CJK"), Some(("/x.ttc".into(), 2)));
        }
    }
}
