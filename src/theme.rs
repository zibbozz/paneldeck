use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use eframe::egui::Color32;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod checks;
pub mod color;
pub mod file;
pub mod generate;
pub mod tokens;

pub use file::{load_dir, Issue};
pub use tokens::{Spec, UiColors};

#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub fg: Color32,
    pub bg: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Color32; 16],
    /// Gesetzte Oberflächenfarben (#309, Feld `ui` der Datei); fehlende werden abgeleitet ([`tokens::default_expr`]).
    pub ui: BTreeMap<String, Spec>,
    /// Eingebautes Theme (nur lesen; bearbeiten nur als Kopie).
    pub builtin: bool,
    /// Datei eines eigenen Themes.
    pub source: Option<PathBuf>,
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// Grobe Helligkeit (0–255) für „dunkel?“ ohne Angabe in der Datei.
pub(crate) fn luma(c: Color32) -> f32 {
    0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32
}

/// Farbe eines Namens (Oberfläche oder Terminal) auf einen festen Wert setzen. `false` bei unbekanntem Namen.
pub fn set_color(t: &mut Theme, name: &str, col: Color32) -> bool {
    let solid = || col.to_opaque();
    match name {
        "fg" => t.fg = solid(),
        "bg" => {
            let a = t.bg.a();
            let [r, g, b, _] = col.to_srgba_unmultiplied();
            t.bg = Color32::from_rgba_unmultiplied(r, g, b, a);
        }
        "cursor" => t.cursor = solid(),
        "selection" => t.selection = solid(),
        n if n.starts_with("ansi") => match n[4..].parse::<usize>() {
            Ok(i) if i < 16 => t.ansi[i] = solid(),
            _ => return false,
        },
        n if tokens::is_ui(n) => {
            t.ui.insert(n.to_string(), Spec::Hex(col));
        }
        _ => return false,
    }
    true
}

impl Theme {
    /// Alle Oberflächenfarben (gesetzt oder abgeleitet).
    pub fn ui_colors(&self) -> UiColors {
        tokens::ui_colors(self)
    }
}

pub fn all() -> Vec<Theme> {
    let mut v = builtin_list();
    for t in &mut v {
        t.builtin = true;
    }
    v
}

fn builtin_list() -> Vec<Theme> {
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
            ui: BTreeMap::new(),
            builtin: true,
            source: None,
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
            ui: BTreeMap::new(),
            builtin: true,
            source: None,
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
            ui: BTreeMap::new(),
            builtin: true,
            source: None,
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
            ui: BTreeMap::new(),
            builtin: true,
            source: None,
        },
        // Rosé hell: Puderrosa, Pflaume als Schrift, Beerenrosa als Cursor. „Weiß“ ist ein Malvengrau,
        // damit auch weißer Text auf dem hellen Grund lesbar bleibt.
        Theme {
            name: "Mashkull".into(),
            dark: false,
            fg: c(0x4a, 0x19, 0x42),
            bg: c(0xfb, 0xee, 0xf3),
            cursor: c(0xc2, 0x18, 0x5b),
            selection: c(0xf3, 0xc6, 0xd8),
            ansi: [
                c(0x3d, 0x1f, 0x36), c(0xb0, 0x10, 0x3f), c(0x2c, 0x73, 0x49), c(0x85, 0x54, 0x0a),
                c(0x3a, 0x56, 0xb0), c(0xa0, 0x1f, 0x7c), c(0x17, 0x70, 0x7c), c(0x6c, 0x52, 0x66),
                c(0x86, 0x66, 0x7c), c(0xd2, 0x2e, 0x6a), c(0x3b, 0x8f, 0x5d), c(0xa4, 0x6b, 0x12),
                c(0x58, 0x74, 0xd0), c(0xc2, 0x3c, 0x9c), c(0x23, 0x90, 0x9c), c(0x96, 0x7a, 0x8d),
            ],
            ui: BTreeMap::new(),
            builtin: true,
            source: None,
        },
        // Rosé dunkel: Aubergine als Grund, zartes Rosa als Schrift, Rosa als Cursor. „Schwarz“ ist ein
        // gedämpftes Pflaumengrau, damit es auf dem dunklen Grund sichtbar bleibt.
        Theme {
            name: "Mashkull Night".into(),
            dark: true,
            fg: c(0xf7, 0xd9, 0xe6),
            bg: c(0x2b, 0x1a, 0x2c),
            cursor: c(0xff, 0x8f, 0xbd),
            selection: c(0x5c, 0x36, 0x58),
            ansi: [
                c(0x8a, 0x69, 0x82), c(0xff, 0x7c, 0x98), c(0x8e, 0xd5, 0xa5), c(0xf0, 0xcc, 0x8c),
                c(0x9d, 0xb5, 0xf2), c(0xe7, 0x9c, 0xd9), c(0x88, 0xd2, 0xd6), c(0xe6, 0xcb, 0xd9),
                c(0xa9, 0x86, 0x9f), c(0xff, 0x9d, 0xb2), c(0xa9, 0xf0, 0xbf), c(0xff, 0xe1, 0xa6),
                c(0xbc, 0xcd, 0xff), c(0xf8, 0xb8, 0xec), c(0xa6, 0xee, 0xf0), c(0xff, 0xf1, 0xf7),
            ],
            ui: BTreeMap::new(),
            builtin: true,
            source: None,
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

    /// Farbe zu einem Index der Emulation (`Term::colors`, OSC 4/10/11/12): 0–255 Palette, 256 Vordergrund,
    /// 257 Hintergrund, 258 Cursor; alles andere wie der Vordergrund. Ohne Transparenz.
    pub fn index_color(&self, index: usize) -> Color32 {
        let c = match index {
            0..=255 => self.indexed(index as u8, false),
            256 => self.fg,
            257 => self.bg,
            258 => self.cursor,
            _ => self.fg,
        };
        // Color32 speichert vormultipliert: ein durchsichtiger Hintergrund (Fenster-Transparenz) hätte sonst dunklere Werte.
        let [r, g, b, _] = c.to_srgba_unmultiplied();
        Color32::from_rgb(r, g, b)
    }

    /// Theme mit den Farben, die ein Programm per OSC 4/10/11/12 gesetzt hat (Palette 0–15, Vorder-/Hintergrund,
    /// Cursor). `None`, wenn keine davon überschrieben ist. Der Hintergrund behält die Deckkraft des Themes
    /// (durchsichtiges Fenster). Überschriebene Palettenfarben ab 16 löst [`Theme::resolve_ov`] auf.
    pub fn with_overrides(&self, ov: &Colors) -> Option<Theme> {
        let any = (0..16).chain(256..=258).any(|i| ov[i].is_some());
        if !any {
            return None;
        }
        let rgb = |c: Rgb| Color32::from_rgb(c.r, c.g, c.b);
        let mut t = self.clone();
        for (i, a) in t.ansi.iter_mut().enumerate() {
            if let Some(c) = ov[i] {
                *a = rgb(c);
            }
        }
        if let Some(c) = ov[NamedColor::Foreground] {
            t.fg = rgb(c);
        }
        if let Some(c) = ov[NamedColor::Background] {
            t.bg = Color32::from_rgba_unmultiplied(c.r, c.g, c.b, self.bg.a());
        }
        if let Some(c) = ov[NamedColor::Cursor] {
            t.cursor = rgb(c);
        }
        Some(t)
    }

    /// Wie [`Theme::resolve`], aber mit den von Programmen gesetzten Palettenfarben 16–255 (`self` ist schon das
    /// Theme aus [`Theme::with_overrides`]).
    pub fn resolve_ov(&self, color: Color, bold: bool, dim: bool, ov: &Colors) -> Color32 {
        if let Color::Indexed(i @ 16..=255) = color {
            if let Some(c) = ov[i as usize] {
                let col = Color32::from_rgb(c.r, c.g, c.b);
                return if dim { col.gamma_multiply(0.66) } else { col };
            }
        }
        self.resolve(color, bold, dim)
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
        let t = file::parse_str(
            r##"{"name":"X","fg":"#ffffff","bg":"#000000","ansi":["#000000","#111111","#222222","#333333","#444444","#555555","#666666","#777777","#888888","#999999","#aaaaaa","#bbbbbb","#cccccc","#dddddd","#eeeeee","#ffffff"]}"##,
        )
        .unwrap();
        assert!(t.dark && !t.builtin);
        assert!(all().iter().all(|t| t.builtin && t.source.is_none() && t.ui.is_empty()));
        // Setzen per Name: Terminal und Oberfläche; Hintergrund behält seine Deckkraft.
        let mut x = t.clone();
        x.bg = Color32::from_rgba_unmultiplied(0, 0, 0, 200);
        assert!(set_color(&mut x, "ansi3", Color32::RED) && x.ansi[3] == Color32::RED);
        assert!(set_color(&mut x, "bg", Color32::from_rgb(10, 20, 30)) && x.bg.a() == 200);
        assert!(set_color(&mut x, "border", Color32::BLUE) && x.ui["border"] == Spec::Hex(Color32::BLUE));
        assert!(!set_color(&mut x, "ansi16", Color32::RED) && !set_color(&mut x, "nope", Color32::RED));
        assert_eq!(t.ansi[15], Color32::WHITE);
    }

    #[test]
    fn program_colour_overrides() {
        let mut base = all()[0].clone();
        base.bg = Color32::from_rgba_unmultiplied(0x1e, 0x21, 0x27, 200);
        let mut ov = Colors::default();
        assert!(base.with_overrides(&ov).is_none());
        // Indizes der Emulation: Palette, Vorder-/Hintergrund, Cursor (ohne Transparenz).
        assert_eq!(base.index_color(1), base.ansi[1]);
        assert_eq!(base.index_color(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(base.index_color(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(base.index_color(256), base.fg);
        // Durchsichtiger Hintergrund: Farbe ohne Deckkraft (vormultipliert gespeichert, Rundung höchstens 1).
        let bg = base.index_color(257);
        assert_eq!(bg.a(), 255);
        for (got, want) in [(bg.r(), 0x1e), (bg.g(), 0x21), (bg.b(), 0x27)] {
            assert!((got as i32 - want).abs() <= 1, "{bg:?}");
        }
        assert_eq!(all()[0].index_color(257), all()[0].bg);
        assert_eq!(base.index_color(258), base.cursor);
        ov[1] = Some(Rgb { r: 1, g: 2, b: 3 });
        ov[200] = Some(Rgb { r: 4, g: 5, b: 6 });
        ov[NamedColor::Background] = Some(Rgb { r: 7, g: 8, b: 9 });
        ov[NamedColor::Foreground] = Some(Rgb { r: 10, g: 11, b: 12 });
        let t = base.with_overrides(&ov).unwrap();
        assert_eq!(t.ansi[1], Color32::from_rgb(1, 2, 3));
        assert_eq!(t.ansi[2], base.ansi[2]);
        assert_eq!(t.bg, Color32::from_rgba_unmultiplied(7, 8, 9, 200), "Deckkraft bleibt");
        assert_eq!(t.fg, Color32::from_rgb(10, 11, 12));
        assert_eq!(t.cursor, base.cursor);
        assert_eq!(t.resolve_ov(Color::Indexed(200), false, false, &ov), Color32::from_rgb(4, 5, 6));
        assert_eq!(t.resolve_ov(Color::Indexed(201), false, false, &ov), base.index_color(201));
        assert_eq!(t.resolve_ov(Color::Named(NamedColor::Red), false, false, &ov), Color32::from_rgb(1, 2, 3));
        assert_eq!(t.resolve_ov(Color::Named(NamedColor::Foreground), false, false, &ov), Color32::from_rgb(10, 11, 12));
        // Nur Palettenfarben ab 16 überschrieben: kein eigenes Theme nötig.
        let mut ov2 = Colors::default();
        ov2[100] = Some(Rgb { r: 0, g: 0, b: 0 });
        assert!(base.with_overrides(&ov2).is_none());
    }

    /// Relative Leuchtdichte nach WCAG 2.x.
    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    /// Kontrastverhältnis nach WCAG (1–21).
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Farbton in Grad (0–360).
    fn hue(c: Color32) -> f64 {
        let (r, g, b) = (c.r() as f64, c.g() as f64, c.b() as f64);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let d = max - min;
        if d == 0.0 {
            return 0.0;
        }
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        h * 60.0
    }

    #[test]
    fn rose_themes_are_readable() {
        let themes = all();
        for name in ["Mashkull", "Mashkull Night"] {
            let t = themes.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{name} fehlt"));
            assert_eq!(t.dark, name.ends_with("Night"), "{name}: hell/dunkel");
            assert!(luminance(t.bg) > 0.5 || t.dark, "{name}: heller Grund");
            assert!(contrast(t.fg, t.bg) >= 7.0, "{name}: Schrift {:.2}", contrast(t.fg, t.bg));
            assert!(contrast(t.fg, t.selection) >= 4.5, "{name}: Schrift auf Auswahl {:.2}", contrast(t.fg, t.selection));
            assert!(contrast(t.selection, t.bg) >= 1.25, "{name}: Auswahl sichtbar {:.2}", contrast(t.selection, t.bg));
            // Block-Cursor: Fläche in Cursorfarbe, Zeichen in Hintergrundfarbe.
            assert!(contrast(t.cursor, t.bg) >= 4.5, "{name}: Cursor {:.2}", contrast(t.cursor, t.bg));
            for (i, &a) in t.ansi.iter().enumerate() {
                let k = contrast(a, t.bg);
                // Normale Farben (außer Schwarz im dunklen Thema) mindestens 4.5:1, alle mindestens 3:1.
                let min = if i < 8 && !(t.dark && i == 0) { 4.5 } else { 3.0 };
                assert!(k >= min, "{name}: ANSI {i} nur {k:.2}:1");
                if i >= 8 {
                    assert!(luminance(a) > luminance(t.ansi[i - 8]), "{name}: ANSI {i} nicht heller als {}", i - 8);
                }
            }
            // Rot, Grün, Gelb, Blau bleiben unterscheidbar (Farbton mindestens 45° auseinander).
            for base in [1, 9] {
                for x in 0..4 {
                    for y in x + 1..4 {
                        let d = (hue(t.ansi[base + x]) - hue(t.ansi[base + y])).abs();
                        let d = d.min(360.0 - d);
                        assert!(d >= 45.0, "{name}: ANSI {} und {} zu ähnlich ({d:.0}°)", base + x, base + y);
                    }
                }
            }
        }
    }
}
