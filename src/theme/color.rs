//! Farbrechnung für den Theme-Editor (#309): Kontrast nach WCAG 2.x, CIELAB und ΔE (CIEDE2000), Simulation von
//! Farbsehschwächen (Machado 2009, volle Ausprägung) und kleinste Helligkeitsänderung, die eine Regel erfüllt.
//! Kennt kein egui außer `Color32` (vormultipliert gespeichert, deshalb vor jeder Rechnung [`over`]).

use eframe::egui::Color32;

/// Farbe in „#rrggbb“ bzw. „#rrggbbaa“ (nur mit Transparenz), Werte ohne Vormultiplikation.
pub fn to_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// „#RRGGBB“ oder „#RRGGBBAA“ (Groß-/Kleinschreibung egal, „#“ optional). Sonst `None`.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if !(s.len() == 6 || s.len() == 8) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    let a = if s.len() == 8 { v(6)? } else { 255 };
    Some(Color32::from_rgba_unmultiplied(v(0)?, v(2)?, v(4)?, a))
}

/// `c` deckend über `bg` (Alpha-Mischung); `bg` selbst wird als deckend angenommen.
pub fn over(c: Color32, bg: Color32) -> Color32 {
    let a = c.a() as f32 / 255.0;
    let m = |x: u8, y: u8| (x as f32 + y as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(c.r(), bg.r()), m(c.g(), bg.g()), m(c.b(), bg.b()))
}

fn lin(v: u8) -> f64 {
    let v = v as f64 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn enc(v: f64) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Relative Leuchtdichte nach WCAG 2.x (0–1) der deckenden Farbe.
pub fn luminance(c: Color32) -> f64 {
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// Kontrastverhältnis nach WCAG (1–21).
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

const WHITE: [f64; 3] = [0.950_47, 1.0, 1.088_83];

/// CIELAB (D65) einer deckenden Farbe: `[L, a, b]`.
pub fn lab(c: Color32) -> [f64; 3] {
    let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b;
    let f = |t: f64| {
        let d: f64 = 6.0 / 29.0;
        if t > d * d * d {
            t.cbrt()
        } else {
            t / (3.0 * d * d) + 4.0 / 29.0
        }
    };
    let (fx, fy, fz) = (f(x / WHITE[0]), f(y / WHITE[1]), f(z / WHITE[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Zurück aus CIELAB (außerhalb von sRGB wird beschnitten).
pub fn from_lab(l: [f64; 3]) -> Color32 {
    let fy = (l[0] + 16.0) / 116.0;
    let fx = fy + l[1] / 500.0;
    let fz = fy - l[2] / 200.0;
    let finv = |t: f64| {
        let d: f64 = 6.0 / 29.0;
        if t > d {
            t * t * t
        } else {
            3.0 * d * d * (t - 4.0 / 29.0)
        }
    };
    let (x, y, z) = (finv(fx) * WHITE[0], finv(fy) * WHITE[1], finv(fz) * WHITE[2]);
    let r = 3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z;
    let g = -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z;
    let b = 0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z;
    Color32::from_rgb(enc(r), enc(g), enc(b))
}

/// Buntheit (CIELAB-Chroma) der deckenden Farbe.
pub fn chroma(c: Color32) -> f64 {
    let [_, a, b] = lab(c);
    a.hypot(b)
}

/// Farbabstand ΔE nach CIEDE2000 zwischen zwei deckenden Farben (≈1: gerade sichtbar, ≥10: klar verschieden).
pub fn delta_e(a: Color32, b: Color32) -> f64 {
    ciede2000(lab(a), lab(b))
}

fn ciede2000(p: [f64; 3], q: [f64; 3]) -> f64 {
    let (l1, a1, b1) = (p[0], p[1], p[2]);
    let (l2, a2, b2) = (q[0], q[1], q[2]);
    let deg = |r: f64| r.to_degrees();
    let rad = |d: f64| d.to_radians();
    let p7 = |x: f64| x.powi(7);
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cbar = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (p7(cbar) / (p7(cbar) + p7(25.0))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);
    let hue = |b: f64, a: f64| if b == 0.0 && a == 0.0 { 0.0 } else { deg(b.atan2(a)).rem_euclid(360.0) };
    let h1p = hue(b1, a1p);
    let h2p = hue(b2, a2p);
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d.abs() <= 180.0 {
            d
        } else if d > 180.0 {
            d - 360.0
        } else {
            d + 360.0
        }
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * rad(dh / 2.0).sin();
    let lbar = (l1 + l2) / 2.0;
    let cbarp = (c1p + c2p) / 2.0;
    let hbar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * rad(hbar - 30.0).cos() + 0.24 * rad(2.0 * hbar).cos() + 0.32 * rad(3.0 * hbar + 6.0).cos() - 0.20 * rad(4.0 * hbar - 63.0).cos();
    let dtheta = 30.0 * (-((hbar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (p7(cbarp) / (p7(cbarp) + p7(25.0))).sqrt();
    let sl = 1.0 + 0.015 * (lbar - 50.0).powi(2) / (20.0 + (lbar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cbarp;
    let sh = 1.0 + 0.015 * cbarp * t;
    let rt = -rad(2.0 * dtheta).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh)).sqrt()
}

/// Farbsehschwäche für die Simulation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cvd {
    /// Rot-Schwäche (Protanopie).
    Protan,
    /// Grün-Schwäche (Deuteranopie).
    Deutan,
    /// Blau-Gelb-Schwäche (Tritanopie).
    Tritan,
}

impl Cvd {
    pub const ALL: [Cvd; 3] = [Cvd::Protan, Cvd::Deutan, Cvd::Tritan];

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Cvd::Protan => lang.t("Rot-Schwäche", "red deficiency"),
            Cvd::Deutan => lang.t("Grün-Schwäche", "green deficiency"),
            Cvd::Tritan => lang.t("Blau-Gelb-Schwäche", "blue-yellow deficiency"),
        }
    }
}

/// Wie eine deckende Farbe bei der Farbsehschwäche erscheint (Machado, Oliveira, Fernandes 2009, Schwere 1.0, im
/// linearen RGB).
pub fn simulate(c: Color32, k: Cvd) -> Color32 {
    let m: [[f64; 3]; 3] = match k {
        Cvd::Protan => [[0.152_286, 1.052_583, -0.204_868], [0.114_503, 0.786_281, 0.099_216], [-0.003_882, -0.048_116, 1.051_998]],
        Cvd::Deutan => [[0.367_322, 0.860_646, -0.227_968], [0.280_085, 0.672_501, 0.047_413], [-0.011_820, 0.042_940, 0.968_881]],
        Cvd::Tritan => [[1.255_528, -0.076_749, -0.178_779], [-0.078_411, 0.930_809, 0.147_602], [0.004_733, 0.691_367, 0.303_900]],
    };
    let v = [lin(c.r()), lin(c.g()), lin(c.b())];
    let row = |r: [f64; 3]| r[0] * v[0] + r[1] * v[1] + r[2] * v[2];
    Color32::from_rgb(enc(row(m[0])), enc(row(m[1])), enc(row(m[2])))
}

/// Gleiche Farbe (Farbton und Buntheit im CIELAB) mit der Helligkeit `l` (0–100).
pub fn with_lightness(c: Color32, l: f64) -> Color32 {
    let [_, a, b] = lab(c);
    let out = from_lab([l.clamp(0.0, 100.0), a, b]);
    Color32::from_rgba_unmultiplied(out.r(), out.g(), out.b(), c.to_srgba_unmultiplied()[3])
}

/// Kleinste Helligkeitsänderung (CIELAB-L in Schritten von 0,5, Farbton bleibt), nach der `ok` gilt. `None`, wenn es
/// weder heller noch dunkler gelingt. Die Farbe selbst zählt nicht (der Aufrufer prüft sie vorher).
pub fn nudge_lightness(c: Color32, ok: impl Fn(Color32) -> bool) -> Option<Color32> {
    let l0 = lab(c)[0];
    let mut step = 0.5;
    while step <= 100.0 {
        for l in [l0 + step, l0 - step] {
            if (0.0..=100.0).contains(&l) {
                let cand = with_lightness(c, l);
                if cand != c && ok(cand) {
                    return Some(cand);
                }
            }
        }
        step += 0.5;
    }
    None
}

/// HSV (0–1) einer Farbe – für den Generator.
pub fn to_hsv(c: Color32) -> [f32; 3] {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, if max == 0.0 { 0.0 } else { d / max }, max]
}

/// Farbe aus HSV (0–1).
pub fn from_hsv(h: f32, s: f32, v: f32) -> Color32 {
    let h = h.rem_euclid(1.0) * 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let q = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(q(r), q(g), q(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_rejects() {
        for s in ["#1e2127", "#000000", "#ffffffff"] {
            let c = parse_hex(s).unwrap();
            let want = if s.len() == 9 && s.ends_with("ff") { &s[..7] } else { s };
            assert_eq!(to_hex(c), want);
        }
        // Mit Transparenz speichert Color32 vormultipliert: der Text kann um 1 abweichen, bleibt danach aber stabil.
        let c = parse_hex("#d9775780").unwrap();
        assert_eq!(parse_hex(&to_hex(c)), Some(c));
        assert!(to_hex(c).ends_with("80"));
        assert_eq!(parse_hex("ABCDEF"), Some(Color32::from_rgb(0xab, 0xcd, 0xef)), "ohne # und groß");
        for bad in ["", "#12345", "#1234567", "#gggggg", "#ff00ff0", "red", "#ff00ff 1"] {
            assert!(parse_hex(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn wcag_contrast_reference_values() {
        assert!((contrast(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 1e-9);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-9);
        // #767676 auf Weiß ist der bekannte Grenzfall 4,54:1.
        let k = contrast(Color32::from_gray(0x76), Color32::WHITE);
        assert!((k - 4.54).abs() < 0.01, "{k}");
    }

    #[test]
    fn ciede2000_matches_published_pairs() {
        // Sharma, Wu, Dalal (2005), Tabelle 1, Paare 1, 7 und 17.
        let cases = [([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425), ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669), ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492)];
        for (a, b, want) in cases {
            let got = ciede2000(a, b);
            assert!((got - want).abs() < 1e-3, "{a:?} {b:?}: {got}");
        }
        assert_eq!(delta_e(Color32::RED, Color32::RED), 0.0);
    }

    #[test]
    fn lab_round_trip_keeps_colours() {
        for c in [Color32::from_rgb(0x1e, 0x21, 0x27), Color32::from_rgb(0xd9, 0x77, 0x57), Color32::WHITE, Color32::BLACK, Color32::from_rgb(0x12, 0xf0, 0x80)] {
            let back = from_lab(lab(c));
            for (x, y) in [(c.r(), back.r()), (c.g(), back.g()), (c.b(), back.b())] {
                assert!((x as i32 - y as i32).abs() <= 1, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn colour_blindness_merges_red_and_green() {
        let (red, green) = (Color32::from_rgb(0xcc, 0x33, 0x33), Color32::from_rgb(0x33, 0x99, 0x33));
        let normal = delta_e(red, green);
        let deutan = delta_e(simulate(red, Cvd::Deutan), simulate(green, Cvd::Deutan));
        assert!(deutan < normal / 2.0, "Rot/Grün rücken bei Grün-Schwäche zusammen: {normal:.1} -> {deutan:.1}");
        // Grau bleibt grau.
        let g = simulate(Color32::from_gray(128), Cvd::Protan);
        assert!((g.r() as i32 - g.b() as i32).abs() <= 2, "{g:?}");
    }

    #[test]
    fn nudging_finds_the_smallest_lightness_change() {
        let bg = Color32::from_rgb(0x1e, 0x21, 0x27);
        let dim = Color32::from_rgb(0x50, 0x55, 0x60);
        assert!(contrast(dim, bg) < 4.5);
        let fixed = nudge_lightness(dim, |c| contrast(c, bg) >= 4.5).unwrap();
        let k = contrast(fixed, bg);
        assert!((4.5..5.0).contains(&k), "knapp über der Grenze: {k}");
        assert!(lab(fixed)[0] > lab(dim)[0], "heller auf dunklem Grund");
        assert!(nudge_lightness(dim, |_| false).is_none());
    }

    #[test]
    fn over_blends_premultiplied_colours() {
        let half = Color32::WHITE.gamma_multiply(0.5);
        let c = over(half, Color32::BLACK);
        assert!((c.r() as i32 - 128).abs() <= 1, "{c:?}");
        assert_eq!(over(Color32::RED, Color32::BLUE), Color32::RED);
        let hsv = to_hsv(Color32::from_rgb(0xd9, 0x77, 0x57));
        let back = from_hsv(hsv[0], hsv[1], hsv[2]);
        assert!((back.r() as i32 - 0xd9).abs() <= 1 && (back.g() as i32 - 0x77).abs() <= 1, "{back:?}");
    }
}
