use alacritty_terminal::vte::ansi::{Color, NamedColor};
use eframe::egui::Color32;
use serde::Deserialize;
use std::path::Path;

#[derive(Clone)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub fg: Color32,
    pub bg: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Color32; 16],
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn all() -> Vec<Theme> {
    vec![
        Theme {
            name: "One Dark".into(),
            dark: true,
            fg: c(0xab, 0xb2, 0xbf),
            bg: c(0x1e, 0x21, 0x27),
            cursor: c(0x52, 0x8b, 0xff),
            selection: c(0x3e, 0x44, 0x51),
            ansi: [
                c(0x1e, 0x21, 0x27), c(0xe0, 0x6c, 0x75), c(0x98, 0xc3, 0x79), c(0xe5, 0xc0, 0x7b),
                c(0x61, 0xaf, 0xef), c(0xc6, 0x78, 0xdd), c(0x56, 0xb6, 0xc2), c(0xab, 0xb2, 0xbf),
                c(0x5c, 0x63, 0x70), c(0xe0, 0x6c, 0x75), c(0x98, 0xc3, 0x79), c(0xe5, 0xc0, 0x7b),
                c(0x61, 0xaf, 0xef), c(0xc6, 0x78, 0xdd), c(0x56, 0xb6, 0xc2), c(0xff, 0xff, 0xff),
            ],
        },
        Theme {
            name: "Dracula".into(),
            dark: true,
            fg: c(0xf8, 0xf8, 0xf2),
            bg: c(0x28, 0x2a, 0x36),
            cursor: c(0xf8, 0xf8, 0xf2),
            selection: c(0x44, 0x47, 0x5a),
            ansi: [
                c(0x21, 0x22, 0x2c), c(0xff, 0x55, 0x55), c(0x50, 0xfa, 0x7b), c(0xf1, 0xfa, 0x8c),
                c(0xbd, 0x93, 0xf9), c(0xff, 0x79, 0xc6), c(0x8b, 0xe9, 0xfd), c(0xf8, 0xf8, 0xf2),
                c(0x62, 0x72, 0xa4), c(0xff, 0x6e, 0x6e), c(0x69, 0xff, 0x94), c(0xff, 0xff, 0xa5),
                c(0xd6, 0xac, 0xff), c(0xff, 0x92, 0xdf), c(0xa4, 0xff, 0xff), c(0xff, 0xff, 0xff),
            ],
        },
        Theme {
            name: "Solarized Dark".into(),
            dark: true,
            fg: c(0x83, 0x94, 0x96),
            bg: c(0x00, 0x2b, 0x36),
            cursor: c(0x93, 0xa1, 0xa1),
            selection: c(0x07, 0x36, 0x42),
            ansi: [
                c(0x07, 0x36, 0x42), c(0xdc, 0x32, 0x2f), c(0x85, 0x99, 0x00), c(0xb5, 0x89, 0x00),
                c(0x26, 0x8b, 0xd2), c(0xd3, 0x36, 0x82), c(0x2a, 0xa1, 0x98), c(0xee, 0xe8, 0xd5),
                c(0x00, 0x2b, 0x36), c(0xcb, 0x4b, 0x16), c(0x58, 0x6e, 0x75), c(0x65, 0x7b, 0x83),
                c(0x83, 0x94, 0x96), c(0x6c, 0x71, 0xc4), c(0x93, 0xa1, 0xa1), c(0xfd, 0xf6, 0xe3),
            ],
        },
        Theme {
            name: "Hell / Light".into(),
            dark: false,
            fg: c(0x38, 0x3a, 0x42),
            bg: c(0xfa, 0xfa, 0xfa),
            cursor: c(0x52, 0x6f, 0xff),
            selection: c(0xd0, 0xd8, 0xf0),
            ansi: [
                c(0x38, 0x3a, 0x42), c(0xca, 0x12, 0x43), c(0x50, 0xa1, 0x4f), c(0xc1, 0x84, 0x01),
                c(0x40, 0x78, 0xf2), c(0xa6, 0x26, 0xa4), c(0x01, 0x84, 0xbc), c(0xa0, 0xa1, 0xa7),
                c(0x4f, 0x52, 0x5e), c(0xe4, 0x56, 0x49), c(0x62, 0xb5, 0x50), c(0xd9, 0xa0, 0x1a),
                c(0x5c, 0x8f, 0xff), c(0xc0, 0x4a, 0xc0), c(0x20, 0x9f, 0xd0), c(0xff, 0xff, 0xff),
            ],
        },
    ]
}

impl Theme {
    pub fn resolve(&self, color: Color, bold: bool, dim: bool) -> Color32 {
        let col = match color {
            Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
            Color::Indexed(i) => self.indexed(i, bold),
            Color::Named(n) => self.named(n, bold),
        };
        if dim {
            col.gamma_multiply(0.66)
        } else {
            col
        }
    }

    fn named(&self, n: NamedColor, bold: bool) -> Color32 {
        use NamedColor::*;
        match n {
            Foreground | BrightForeground => self.fg,
            DimForeground => self.fg.gamma_multiply(0.66),
            Background => self.bg,
            Cursor => self.cursor,
            DimBlack | DimRed | DimGreen | DimYellow | DimBlue | DimMagenta | DimCyan | DimWhite => {
                let idx = match n {
                    DimBlack => 0, DimRed => 1, DimGreen => 2, DimYellow => 3,
                    DimBlue => 4, DimMagenta => 5, DimCyan => 6, _ => 7,
                };
                self.ansi[idx].gamma_multiply(0.66)
            }
            other => {
                let idx = other as usize;
                if idx < 8 && bold {
                    self.ansi[idx + 8]
                } else {
                    self.ansi[idx.min(15)]
                }
            }
        }
    }

    fn indexed(&self, i: u8, bold: bool) -> Color32 {
        match i {
            0..=7 if bold => self.ansi[i as usize + 8],
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let lv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Color32::from_rgb(lv(i / 36), lv((i / 6) % 6), lv(i % 6))
            }
            232..=255 => {
                let v = 8 + (i - 232) * 10;
                Color32::from_rgb(v, v, v)
            }
        }
    }
}

#[derive(Deserialize)]
struct ThemeFile {
    name: String,
    dark: Option<bool>,
    fg: String,
    bg: String,
    cursor: Option<String>,
    selection: Option<String>,
    ansi: Vec<String>,
}

fn hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn from_file(f: ThemeFile) -> Option<Theme> {
    if f.ansi.len() < 16 {
        return None;
    }
    let mut ansi = [Color32::BLACK; 16];
    for (i, c) in f.ansi.iter().take(16).enumerate() {
        ansi[i] = hex(c)?;
    }
    let bg = hex(&f.bg)?;
    let fg = hex(&f.fg)?;
    let lum = |c: Color32| 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    Some(Theme {
        name: f.name,
        dark: f.dark.unwrap_or(lum(bg) < 128.0),
        fg,
        bg,
        cursor: f.cursor.as_deref().and_then(hex).unwrap_or(fg),
        selection: f.selection.as_deref().and_then(hex).unwrap_or_else(|| crate::style::mix(bg, fg, 0.2)),
        ansi,
    })
}

/// Eigene Themes aus `<dir>/*.json` laden (ungültige Dateien werden übersprungen).
pub fn load_dir(dir: &Path) -> Vec<Theme> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    for p in files {
        if let Some(t) = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<ThemeFile>(crate::state::strip_bom(&s)).ok()).and_then(from_file) {
            out.push(t);
        }
    }
    out
}

/// Beispiel-Theme anlegen, damit das Format sichtbar ist.
pub fn write_example(dir: &Path) {
    if dir.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let example = r##"{
  "name": "Nord (Beispiel)",
  "dark": true,
  "fg": "#d8dee9",
  "bg": "#2e3440",
  "cursor": "#88c0d0",
  "selection": "#434c5e",
  "ansi": [
    "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
    "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"
  ]
}
"##;
    let _ = std::fs::write(dir.join("nord-beispiel.json"), example);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_custom_theme() {
        let f: ThemeFile = serde_json::from_str(
            r##"{"name":"X","fg":"#ffffff","bg":"#000000","ansi":["#000000","#111111","#222222","#333333","#444444","#555555","#666666","#777777","#888888","#999999","#aaaaaa","#bbbbbb","#cccccc","#dddddd","#eeeeee","#ffffff"]}"##,
        )
        .unwrap();
        let t = from_file(f).unwrap();
        assert!(t.dark);
        assert_eq!(t.ansi[15], Color32::WHITE);
    }
}
