//! „Aus wenigen Farben erzeugen“ (#309): ein vollständiges Theme aus Hintergrund, Akzent und hell/dunkel. Terminal-
//! Palette mit festen Farbtönen und lesbarer Helligkeit; Status-, Diff- und Syntaxfarben verweisen auf die Palette,
//! damit spätere Feinarbeit an den ANSI-Farben durchschlägt.

use super::color::{contrast, from_hsv, lab, luminance, nudge_lightness, to_hsv, with_lightness};
use super::tokens::Spec;
use super::Theme;
use crate::style::mix;
use eframe::egui::Color32;
use std::collections::BTreeMap;

/// Eingaben des Generators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seed {
    pub bg: Color32,
    pub accent: Color32,
    pub dark: bool,
}

impl Seed {
    /// Ausgangswerte aus einem Theme (Hintergrund, Akzent, hell/dunkel).
    pub fn from_theme(t: &Theme) -> Seed {
        Seed { bg: t.bg.to_opaque(), accent: super::tokens::resolve(t, "accent").to_opaque(), dark: t.dark }
    }
}

/// Farbe mit Farbton `hue` (0–1) und Helligkeit `l` (CIELAB), mindestens `min`:1 gegen `bg`.
fn hue_color(hue: f32, sat: f32, l: f64, bg: Color32, min: f64) -> Color32 {
    let c = with_lightness(from_hsv(hue, sat, 0.9), l);
    if contrast(c, bg) >= min {
        c
    } else {
        nudge_lightness(c, |x| contrast(x, bg) >= min).unwrap_or(c)
    }
}

fn link(to: &str) -> Spec {
    Spec::Link { to: to.to_string(), alpha: None }
}

/// Vollständiges Theme (Name `name`, nicht eingebaut, ohne Datei).
pub fn generate(name: &str, seed: Seed) -> Theme {
    let bg = seed.bg.to_opaque();
    let dark = seed.dark;
    let (white, black) = (Color32::WHITE, Color32::BLACK);
    // Schrift leicht in Richtung Hintergrund getönt, mindestens 7:1.
    let fg0 = if dark { mix(bg, white, 0.84) } else { mix(bg, black, 0.80) };
    let fg = if contrast(fg0, bg) >= 7.0 { fg0 } else { nudge_lightness(fg0, |x| contrast(x, bg) >= 7.0).unwrap_or(if dark { white } else { black }) };
    let accent = seed.accent.to_opaque();
    let sel0 = mix(bg, accent, if dark { 0.30 } else { 0.22 });
    let selection = if contrast(fg, sel0) >= 4.5 { sel0 } else { nudge_lightness(sel0, |x| contrast(fg, x) >= 4.5).unwrap_or(sel0) };
    let cursor = if contrast(accent, bg) >= 3.0 { accent } else { nudge_lightness(accent, |x| contrast(x, bg) >= 3.0).unwrap_or(fg) };
    // Farbtöne Rot, Grün, Gelb, Blau, Magenta, Cyan; Sättigung etwas nach der des Akzents.
    let sat = (to_hsv(accent)[1] * 0.6 + 0.25).clamp(0.35, 0.7);
    let hues = [0.99_f32, 0.36, 0.12, 0.60, 0.83, 0.51];
    let (ln, lb, min_n, min_b): (f64, f64, f64, f64) = if dark { (68.0, 80.0, 4.5, 4.5) } else { (42.0, 52.0, 4.5, 3.0) };
    let mut ansi = [Color32::BLACK; 16];
    for (i, h) in hues.iter().enumerate() {
        let n = hue_color(*h, sat, ln, bg, min_n);
        let mut b = hue_color(*h, sat * 0.9, lb.max(lab(n)[0] + 6.0), bg, min_b);
        if luminance(b) <= luminance(n) {
            b = nudge_lightness(b, |x| luminance(x) > luminance(n) * 1.15 + 0.01).unwrap_or(b);
        }
        ansi[i + 1] = n;
        ansi[i + 9] = b;
    }
    if dark {
        ansi[0] = mix(bg, white, 0.10);
        ansi[8] = mix(bg, fg, 0.50);
        ansi[7] = mix(fg, bg, 0.12);
        ansi[15] = mix(fg, white, 0.6);
    } else {
        ansi[0] = mix(fg, black, 0.25);
        ansi[8] = mix(bg, fg, 0.62);
        ansi[7] = mix(bg, black, 0.22);
        ansi[15] = mix(bg, black, 0.08);
    }
    for i in [8usize] {
        if contrast(ansi[i], bg) < 3.0 {
            ansi[i] = nudge_lightness(ansi[i], |x| contrast(x, bg) >= 3.0).unwrap_or(ansi[i]);
        }
    }
    if luminance(ansi[8]) <= luminance(ansi[0]) {
        ansi[8] = nudge_lightness(ansi[8], |x| luminance(x) > luminance(ansi[0]) * 1.15 + 0.01).unwrap_or(ansi[8]);
    }
    if luminance(ansi[15]) <= luminance(ansi[7]) {
        ansi[15] = nudge_lightness(ansi[15], |x| luminance(x) > luminance(ansi[7]) * 1.15 + 0.01).unwrap_or(ansi[15]);
    }
    let mut ui = BTreeMap::new();
    ui.insert("accent".to_string(), Spec::Hex(accent));
    for (k, to) in [
        ("success", "ansi2"),
        ("warning", "ansi3"),
        ("error", "ansi1"),
        ("danger", "ansi9"),
        ("info", "ansi4"),
        ("diff_add", "ansi2"),
        ("diff_remove", "ansi1"),
        ("diff_change", "ansi3"),
        ("diff_rename", "ansi4"),
        ("syntax_field", "ansi4"),
        ("syntax_operator", "ansi3"),
        ("syntax_value", "ansi2"),
        ("syntax_keyword", "ansi5"),
        ("syntax_function", "ansi13"),
        ("syntax_predicate", "ansi6"),
        ("syntax_sort", "ansi11"),
    ] {
        ui.insert(k.to_string(), link(to));
    }
    Theme { name: name.to_string(), dark, fg, bg, cursor, selection, ansi, ui, builtin: false, source: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::checks::{run, Rule};

    #[test]
    fn generated_themes_are_readable() {
        let seeds = [
            Seed { bg: Color32::from_rgb(0x1b, 0x1d, 0x2b), accent: Color32::from_rgb(0x7a, 0xa2, 0xf7), dark: true },
            Seed { bg: Color32::from_rgb(0x10, 0x10, 0x10), accent: Color32::from_rgb(0xff, 0x8c, 0x00), dark: true },
            Seed { bg: Color32::from_rgb(0xfa, 0xf8, 0xf5), accent: Color32::from_rgb(0x00, 0x7a, 0x5a), dark: false },
            Seed { bg: Color32::WHITE, accent: Color32::from_rgb(0xe0, 0x10, 0x60), dark: false },
        ];
        for s in seeds {
            let t = generate("Gen", s);
            assert!(contrast(t.fg, t.bg) >= 7.0, "{s:?}");
            assert!(contrast(t.fg, t.selection) >= 4.5, "{s:?}");
            let term = run(&t, |r| r == Rule::Terminal);
            assert!(term.is_empty(), "{s:?}: {:?}", term.iter().map(|h| &h.msg.en).collect::<Vec<_>>());
            for h in run(&t, |r| r == Rule::Contrast) {
                assert!(!h.tokens.contains(&"text"), "{s:?}: {}", h.msg.en);
            }
            // Status verweist auf die Palette.
            let u = crate::theme::tokens::ui_colors(&t);
            assert_eq!(u.error, t.ansi[1]);
            assert_eq!(u.accent, s.accent);
            assert!(crate::theme::tokens::find_cycle(&t).is_none());
            // Speichern und wieder lesen.
            let back = crate::theme::file::parse_str(&crate::theme::file::to_json(&t)).unwrap();
            assert_eq!(back, t);
        }
    }
}
