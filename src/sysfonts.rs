//! Installierte Schriften für das Terminal (#183): jede installierte Festbreitenschrift (z. B. Nerd Fonts) lässt sich
//! wählen, dazu ihre Schnitte Fett/Kursiv/Fett-kursiv für SGR 1/3.
//!
//! Unter Windows über GDI: `EnumFontFamiliesExW` liefert die Familien (nur TrueType/OpenType mit fester Breite, auch für
//! den Benutzer installierte Schriften ohne Pfad), `GetFontData` die Schriftdaten des gewählten Schnitts. Fehlt ein Schnitt,
//! ersetzt GDI ihn durch den normalen – das wird an den Daten erkannt (Gewicht/Kursiv-Bit), dann zeichnet das Terminal
//! Fett/Kursiv selbst nach (`termview`). Anderswo gibt es keine Liste und keine Daten.

use std::sync::atomic::{AtomicU8, Ordering};

/// Schnitte, die mit echter Schrift vorliegen (Bits: 1 = fett, 2 = kursiv, 4 = fett-kursiv), gesetzt von `app::build_fonts`.
static REAL_VARIANTS: AtomicU8 = AtomicU8::new(0);

/// Namen der Schriftfamilien für die Schnitte (immer angelegt; ohne echte Schrift = normale Terminalschrift).
pub const FAMILY_BOLD: &str = "term-bold";
pub const FAMILY_ITALIC: &str = "term-italic";
pub const FAMILY_BOLD_ITALIC: &str = "term-bold-italic";

pub fn set_real_variants(bold: bool, italic: bool, bold_italic: bool) {
    REAL_VARIANTS.store(bold as u8 | (italic as u8) << 1 | (bold_italic as u8) << 2, Ordering::Relaxed);
}

/// Wie eine Zelle mit Fett/Kursiv gezeichnet wird: Schriftfamilie (`None` = normale) und was nachgezeichnet werden muss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StylePlan {
    pub family: Option<&'static str>,
    pub fake_bold: bool,
    pub fake_italic: bool,
}

/// Plan für eine Zelle (`bold`/`italic` = SGR-Attribute), abhängig von den vorhandenen Schnitten.
pub fn plan(bold: bool, italic: bool) -> StylePlan {
    plan_with(REAL_VARIANTS.load(Ordering::Relaxed), bold, italic)
}

fn plan_with(real: u8, bold: bool, italic: bool) -> StylePlan {
    let (rb, ri, rbi) = (real & 1 != 0, real & 2 != 0, real & 4 != 0);
    let none = StylePlan { family: None, fake_bold: bold, fake_italic: italic };
    match (bold, italic) {
        (false, false) => StylePlan { family: None, fake_bold: false, fake_italic: false },
        (true, false) if rb => StylePlan { family: Some(FAMILY_BOLD), fake_bold: false, fake_italic: false },
        (false, true) if ri => StylePlan { family: Some(FAMILY_ITALIC), fake_bold: false, fake_italic: false },
        (true, true) if rbi => StylePlan { family: Some(FAMILY_BOLD_ITALIC), fake_bold: false, fake_italic: false },
        (true, true) if rb => StylePlan { family: Some(FAMILY_BOLD), fake_bold: false, fake_italic: true },
        (true, true) if ri => StylePlan { family: Some(FAMILY_ITALIC), fake_bold: true, fake_italic: false },
        _ => none,
    }
}

/// Schriftdaten eines Schnitts: Datei-Inhalt und Index in einer Sammlung (`.ttc`).
pub struct FaceData {
    pub bytes: Vec<u8>,
    pub index: u32,
}

/// Prüft geladene Schriftdaten: lesbar, und entspricht der Schnitt dem gewünschten (fett/kursiv)?
pub fn face_matches(data: &FaceData, bold: bool, italic: bool) -> bool {
    let Ok(face) = ttf_parser::Face::parse(&data.bytes, data.index) else { return false };
    let is_bold = face.is_bold() || face.weight().to_number() >= 600;
    let is_italic = face.is_italic() || face.is_oblique();
    is_bold == bold && is_italic == italic
}

/// Ist die Schrift (normaler Schnitt) eine Festbreitenschrift? (Prüfung für Daten, die nicht von der Liste kommen.)
pub fn is_monospace(data: &FaceData) -> bool {
    ttf_parser::Face::parse(&data.bytes, data.index).is_ok_and(|f| f.is_monospaced() || {
        // Manche Schriften setzen das Bit nicht: gleiche Breite von „i“ und „M“ genügt.
        let w = |c| f.glyph_index(c).and_then(|g| f.glyph_hor_advance(g));
        matches!((w('i'), w('M')), (Some(a), Some(b)) if a == b && a > 0)
    })
}

/// Installierte Festbreitenschriften (Familiennamen, sortiert, ohne Doppelte). Leer, wenn nicht ermittelbar.
pub fn monospace_families() -> Vec<String> {
    let mut v = imp::families();
    v.sort_by_key(|s| s.to_lowercase());
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}

/// Wie [`monospace_families`], einmal je Programmlauf ermittelt (für die Auswahl in den Einstellungen).
pub fn monospace_families_cached() -> &'static [String] {
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(monospace_families)
}

/// Schriftdaten des Schnitts `family` (fett/kursiv), wenn die Schrift installiert ist. Der Schnitt kann ein Ersatz sein
/// (siehe [`face_matches`]).
pub fn load(family: &str, bold: bool, italic: bool) -> Option<FaceData> {
    if family.is_empty() || family.chars().count() >= 32 || family.contains('\0') {
        return None;
    }
    imp::load(family, bold, italic)
}

#[cfg(windows)]
mod imp {
    use super::FaceData;
    use windows_sys::Win32::Foundation::LPARAM;
    use windows_sys::Win32::Graphics::Gdi::*;

    const TTCF: u32 = 0x6663_7474; // 'ttcf'
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
                match read(TTCF) {
                    // Sammlung: ganze Datei, der gewählte Schnitt beginnt mit denselben Bytes wie seine Offset-Tabelle.
                    Some(all) => {
                        let own = read(0)?;
                        let index = super::collection_index(&all, &own)?;
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
mod imp {
    pub fn families() -> Vec<String> {
        Vec::new()
    }
    pub fn load(_family: &str, _bold: bool, _italic: bool) -> Option<super::FaceData> {
        None
    }
}

/// Index des Schnitts in einer Schriftsammlung (`all`), dessen Offset-Tabelle mit `own` beginnt (GDI liefert für einen
/// Schnitt einer Sammlung die Daten ab seiner Offset-Tabelle).
fn collection_index(all: &[u8], own: &[u8]) -> Option<u32> {
    let be32 = |b: &[u8], i: usize| b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    if all.get(0..4) != Some(b"ttcf") {
        return None;
    }
    let n = be32(all, 8)?.min(256);
    // Vergleich über den Kopf der Offset-Tabelle (12 Bytes + Tabelleneinträge, höchstens 256 Bytes).
    let cmp = own.len().min(256);
    if cmp < 12 {
        return None;
    }
    (0..n).find(|&i| {
        be32(all, 12 + 4 * i as usize).is_some_and(|off| all.get(off as usize..off as usize + cmp) == Some(&own[..cmp]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_plans() {
        let p = |f, b, i| StylePlan { family: f, fake_bold: b, fake_italic: i };
        // Keine echten Schnitte: alles nachzeichnen.
        assert_eq!(plan_with(0, false, false), p(None, false, false));
        assert_eq!(plan_with(0, true, false), p(None, true, false));
        assert_eq!(plan_with(0, true, true), p(None, true, true));
        // Alle vorhanden.
        assert_eq!(plan_with(7, true, false), p(Some(FAMILY_BOLD), false, false));
        assert_eq!(plan_with(7, false, true), p(Some(FAMILY_ITALIC), false, false));
        assert_eq!(plan_with(7, true, true), p(Some(FAMILY_BOLD_ITALIC), false, false));
        // Fett-kursiv fehlt: fetten Schnitt schräg stellen bzw. kursiven fett nachzeichnen.
        assert_eq!(plan_with(1, true, true), p(Some(FAMILY_BOLD), false, true));
        assert_eq!(plan_with(2, true, true), p(Some(FAMILY_ITALIC), true, false));
        assert_eq!(plan_with(1, false, true), p(None, false, true));
    }

    #[test]
    fn collection_index_matches_offset_table() {
        // Sammlung mit zwei Schnitten: Offset-Tabellen bei 20 und 40.
        let mut all = b"ttcf".to_vec();
        all.extend_from_slice(&[0, 1, 0, 0]); // Version
        all.extend_from_slice(&2u32.to_be_bytes());
        all.extend_from_slice(&20u32.to_be_bytes());
        all.extend_from_slice(&40u32.to_be_bytes());
        all.resize(20, 0);
        all.extend_from_slice(b"\x00\x01\x00\x00AAAAAAAAAAAAAAAA");
        all.resize(40, 0);
        all.extend_from_slice(b"\x00\x01\x00\x00BBBBBBBBBBBBBBBB");
        assert_eq!(collection_index(&all, b"\x00\x01\x00\x00BBBBBBBBBBBBBBBB"), Some(1));
        assert_eq!(collection_index(&all, b"\x00\x01\x00\x00AAAAAAAAAAAAAAAA"), Some(0));
        assert_eq!(collection_index(&all, b"\x00\x01\x00\x00CCCCCCCCCCCCCCCC"), None);
        assert_eq!(collection_index(b"OTTO", b"x"), None);
    }

    #[test]
    fn embedded_cascadia_is_monospace_and_regular() {
        let d = FaceData { bytes: include_bytes!("../assets/fonts/CascadiaMono-Regular.ttf").to_vec(), index: 0 };
        assert!(is_monospace(&d));
        assert!(face_matches(&d, false, false));
        assert!(!face_matches(&d, true, false) && !face_matches(&d, false, true));
        assert!(load("", false, false).is_none());
        assert!(load(&"x".repeat(40), false, false).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn system_fonts_can_be_listed_and_loaded() {
        // Consolas gehört zu Windows; die Liste enthält nur Festbreitenschriften.
        let fams = monospace_families();
        if fams.iter().any(|f| f == "Consolas") {
            let d = load("Consolas", false, false).expect("Consolas");
            assert!(face_matches(&d, false, false) && is_monospace(&d));
            assert!(load("Consolas", true, false).is_some_and(|d| face_matches(&d, true, false)));
        }
        assert!(!fams.iter().any(|f| f == "Arial" || f.starts_with('@')));
        // Unbekannte Namen ersetzt GDI durch eine andere Schrift – das wird erkannt.
        assert!(load("Keine Solche Schrift 123", false, false).is_none());
    }
}
