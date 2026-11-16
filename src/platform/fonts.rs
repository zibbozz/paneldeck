//! System fonts that Paneldeck loads as fallbacks (symbols, CJK, Indic, maths) and as optional terminal fonts. The
//! callers ask by Windows file name (`segoeui.ttf`, `consola.ttf` …); other platforms have neither these names nor this
//! folder yet, so nothing is found there and egui's built-in fonts plus the embedded Cascadia Mono are used.

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
