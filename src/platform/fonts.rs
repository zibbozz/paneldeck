//! System fonts: the system font folder (fallback fonts for symbols, CJK, Indic, maths, and the optional classic terminal
//! fonts) and the installed font families for the terminal font choice (#183, `crate::sysfonts`).
//!
//! Windows: `C:\Windows\Fonts` and GDI (`EnumFontFamiliesExW`, `GetFontData`). Other platforms have neither these file
//! names nor this folder: no system font is found there and egui's built-in fonts plus the embedded Cascadia Mono are
//! used; the list of families is empty (fontconfig/CoreText follow in #149/#150).

use std::path::PathBuf;

/// Full path of a font file in the system font folder (Windows: `C:\Windows\Fonts\<file>`); `None` where unknown.
pub fn system_font(file: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(PathBuf::from(format!(r"C:\Windows\Fonts\{file}")))
    }
    #[cfg(not(windows))]
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

#[cfg(not(windows))]
pub fn families() -> Vec<String> {
    Vec::new()
}

/// Font data of the face `family` (bold/italic) as GDI hands it out; `None` where there is no implementation.
#[cfg(not(windows))]
pub fn load(_family: &str, _bold: bool, _italic: bool) -> Option<crate::sysfonts::FaceData> {
    None
}
