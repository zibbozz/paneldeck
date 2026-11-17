//! Selbst gezeichnete Zellgrafik (#183): Powerline-Trenner (U+E0B0–U+E0BF, Nerd Fonts) und die Unterstreichungsarten
//! (SGR 4:1–4:5, 21; Farbe SGR 58/59). Wie bei `boxdraw` füllen Schriften die Zelle selten exakt – Trenner würden sonst
//! Lücken oder Stufen zur Nachbarzelle zeigen. Die Geometrie liegt in Einheitskoordinaten (0..1, y nach unten) und ist
//! ohne Fenster testbar; `draw_*` setzt sie in die Zelle um.
//!
//! Nicht selbst gezeichnet (kommen aus der Schrift, z. B. einer Nerd Font): die Symbole U+E0A0–U+E0A3 (Branch, Zeile,
//! Schloss) und die verzierten Trenner U+E0C0–U+E0D4 (Flammen, Pixel, Eis, Lego …).

use alacritty_terminal::term::cell::Flags;
use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke};

/// Teil eines Powerline-Zeichens in Einheitskoordinaten.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// Gefülltes konvexes Polygon.
    Fill(Vec<(f32, f32)>),
    /// Linienzug mit der Strichstärke dünner Rahmenlinien.
    Line(Vec<(f32, f32)>),
}

/// Punkte eines Halbkreises (Ellipse über die ganze Zellbreite) von oben nach unten; `right` = Bauch nach rechts.
fn half_circle(right: bool) -> Vec<(f32, f32)> {
    const N: usize = 24;
    (0..=N)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / N as f32;
            let x = a.cos().clamp(0.0, 1.0);
            let y = (0.5 + 0.5 * a.sin()).clamp(0.0, 1.0);
            (if right { x } else { 1.0 - x }, y)
        })
        .collect()
}

/// Geometrie eines Powerline-Trenners, `None` für andere Zeichen.
pub fn powerline(c: char) -> Option<Vec<Part>> {
    use Part::*;
    let v = |p: &[(f32, f32)]| p.to_vec();
    Some(match c as u32 {
        0xE0B0 => vec![Fill(v(&[(0.0, 0.0), (1.0, 0.5), (0.0, 1.0)]))],
        0xE0B1 => vec![Line(v(&[(0.0, 0.0), (1.0, 0.5), (0.0, 1.0)]))],
        0xE0B2 => vec![Fill(v(&[(1.0, 0.0), (1.0, 1.0), (0.0, 0.5)]))],
        0xE0B3 => vec![Line(v(&[(1.0, 0.0), (0.0, 0.5), (1.0, 1.0)]))],
        0xE0B4 => vec![Fill(half_circle(true))],
        0xE0B5 => vec![Line(half_circle(true))],
        0xE0B6 => vec![Fill(half_circle(false))],
        0xE0B7 => vec![Line(half_circle(false))],
        0xE0B8 => vec![Fill(v(&[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)]))],
        0xE0B9 | 0xE0BF => vec![Line(v(&[(0.0, 0.0), (1.0, 1.0)]))],
        0xE0BA => vec![Fill(v(&[(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]))],
        0xE0BB | 0xE0BD => vec![Line(v(&[(1.0, 0.0), (0.0, 1.0)]))],
        0xE0BC => vec![Fill(v(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]))],
        0xE0BE => vec![Fill(v(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]))],
        _ => return None,
    })
}

/// Strichstärke dünner Linien in einer Zelle (wie `boxdraw`), auf ganze Pixel gerundet.
pub fn line_width(cell_h: f32, ppp: f32) -> f32 {
    let w = (cell_h / 14.0).max(1.0);
    ((w * ppp).round() / ppp).max(1.0 / ppp)
}

/// Zeichnet `c`, falls es ein Powerline-Trenner ist. Gibt `true` zurück, wenn gezeichnet wurde.
pub fn draw_powerline(p: &Painter, cell: Rect, c: char, fg: Color32, ppp: f32) -> bool {
    let Some(parts) = powerline(c) else { return false };
    // Kanten auf dem Zellrand ein halbes Pixel nach außen: die Kantenglättung läge sonst als heller Saum auf der Grenze.
    let e = 0.5 / ppp;
    let map = |(u, v): (f32, f32), grow: bool| {
        let gx = if !grow { 0.0 } else if u <= 0.0 { -e } else if u >= 1.0 { e } else { 0.0 };
        let gy = if !grow { 0.0 } else if v <= 0.0 { -e } else if v >= 1.0 { e } else { 0.0 };
        Pos2::new(cell.min.x + u * cell.width() + gx, cell.min.y + v * cell.height() + gy)
    };
    let lw = line_width(cell.height(), ppp);
    for part in parts {
        match part {
            Part::Fill(pts) => {
                p.add(Shape::convex_polygon(pts.into_iter().map(|q| map(q, true)).collect(), fg, Stroke::NONE));
            }
            Part::Line(pts) => {
                p.add(Shape::line(pts.into_iter().map(|q| map(q, false)).collect(), Stroke::new(lw, fg)));
            }
        }
    }
    true
}

/// Art der Unterstreichung (SGR 4, 4:1–4:5, 21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Underline {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

pub fn underline_of(flags: Flags) -> Option<Underline> {
    Some(if flags.contains(Flags::UNDERCURL) {
        Underline::Curly
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Underline::Double
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Underline::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Underline::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        Underline::Single
    } else {
        return None;
    })
}

/// Linienzüge einer Unterstreichung in der Zelle (absolute Koordinaten), jeweils mit Strichstärke `t` zu zeichnen.
/// Punkte und Striche richten sich an einem festen Raster der x-Achse aus, die Welle hat je Zelle genau eine Periode –
/// so laufen sie über Zellgrenzen ohne Sprung weiter.
pub fn underline_paths(style: Underline, cell: Rect, t: f32) -> Vec<Vec<Pos2>> {
    let base = cell.max.y - t * 1.5;
    let hline = |y: f32| vec![Pos2::new(cell.min.x, y), Pos2::new(cell.max.x, y)];
    let pattern = |on: f32, period: f32| -> Vec<Vec<Pos2>> {
        let mut out = Vec::new();
        let mut k = (cell.min.x / period).floor();
        loop {
            let x0 = (k * period).max(cell.min.x);
            let x1 = (k * period + on).min(cell.max.x);
            if k * period >= cell.max.x {
                break;
            }
            if x1 > x0 {
                out.push(vec![Pos2::new(x0, base), Pos2::new(x1, base)]);
            }
            k += 1.0;
        }
        out
    };
    match style {
        Underline::Single => vec![hline(base)],
        Underline::Double => vec![hline(base), hline(base - t * 2.0)],
        Underline::Dotted => pattern(t, t * 2.0),
        Underline::Dashed => pattern(t * 3.0, t * 5.0),
        Underline::Curly => {
            let amp = t.max(1.0) * 1.25;
            let n = ((cell.width() / 1.5).ceil() as usize).clamp(6, 64);
            vec![(0..=n)
                .map(|i| {
                    let f = i as f32 / n as f32;
                    Pos2::new(cell.min.x + f * cell.width(), base - amp * (f * std::f32::consts::TAU).cos())
                })
                .collect()]
        }
    }
}

/// Unterstreichung zeichnen.
pub fn draw_underline(p: &Painter, style: Underline, cell: Rect, col: Color32, ppp: f32) {
    let t = line_width(cell.height(), ppp);
    for path in underline_paths(style, cell, t) {
        p.add(Shape::line(path, Stroke::new(t, col)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    fn all_points(parts: &[Part]) -> Vec<(f32, f32)> {
        parts.iter().flat_map(|p| match p { Part::Fill(v) | Part::Line(v) => v.clone() }).collect()
    }

    #[test]
    fn powerline_geometry_fills_the_cell_edges() {
        for c in 0xE0B0u32..=0xE0BF {
            let parts = powerline(char::from_u32(c).unwrap()).unwrap_or_else(|| panic!("{c:X}"));
            for (x, y) in all_points(&parts) {
                assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y), "{c:X}: {x},{y}");
            }
        }
        // Volle Dreiecke berühren beide Ecken der flachen Seite und die Spitze in der Mitte der anderen.
        let right = all_points(&powerline('\u{e0b0}').unwrap());
        assert!(right.contains(&(0.0, 0.0)) && right.contains(&(0.0, 1.0)) && right.contains(&(1.0, 0.5)));
        let left = all_points(&powerline('\u{e0b2}').unwrap());
        assert!(left.contains(&(1.0, 0.0)) && left.contains(&(1.0, 1.0)) && left.contains(&(0.0, 0.5)));
        // Halbkreise: von der oberen zur unteren Ecke der flachen Seite, Bauch reicht bis zur anderen Seite.
        let hc = half_circle(true);
        assert_eq!(hc.first().map(|p| (p.0.abs() < 1e-5, p.1)), Some((true, 0.0)));
        assert_eq!(hc.last().map(|p| (p.0.abs() < 1e-5, (p.1 - 1.0).abs() < 1e-5)), Some((true, true)));
        assert!(hc.iter().any(|p| (p.0 - 1.0).abs() < 1e-5 && (p.1 - 0.5).abs() < 1e-5));
        assert!(half_circle(false).iter().all(|p| p.0 >= -1e-5 && p.0 <= 1.0 + 1e-5));
        // Andere Zeichen (auch die Symbole davor und die verzierten danach) bleiben der Schrift.
        for c in ['\u{e0a0}', '\u{e0a3}', '\u{e0c0}', '\u{e0d4}', 'A', '─'] {
            assert!(powerline(c).is_none(), "{c:?}");
        }
    }

    #[test]
    fn line_width_snaps_to_pixels() {
        assert_eq!(line_width(17.0, 1.0), 1.0);
        assert_eq!(line_width(28.0, 1.0), 2.0);
        assert_eq!(line_width(17.0, 1.5), 2.0 / 1.5);
        assert!(line_width(10.0, 2.0) >= 0.5);
    }

    #[test]
    fn underline_kinds() {
        assert_eq!(underline_of(Flags::UNDERLINE), Some(Underline::Single));
        assert_eq!(underline_of(Flags::DOUBLE_UNDERLINE), Some(Underline::Double));
        assert_eq!(underline_of(Flags::UNDERCURL), Some(Underline::Curly));
        assert_eq!(underline_of(Flags::DOTTED_UNDERLINE), Some(Underline::Dotted));
        assert_eq!(underline_of(Flags::DASHED_UNDERLINE), Some(Underline::Dashed));
        assert_eq!(underline_of(Flags::BOLD | Flags::ITALIC), None);
    }

    #[test]
    fn underline_geometry_stays_in_the_cell_and_continues() {
        let t = 1.0;
        for x in [0.0f32, 8.0, 13.0, 104.5] {
            let cell = Rect::from_min_size(Pos2::new(x, 20.0), Vec2::new(8.0, 17.0));
            for style in [Underline::Single, Underline::Double, Underline::Curly, Underline::Dotted, Underline::Dashed] {
                let paths = underline_paths(style, cell, t);
                assert!(!paths.is_empty(), "{style:?} @ {x}");
                for p in paths.iter().flatten() {
                    assert!(p.x >= cell.min.x - 1e-4 && p.x <= cell.max.x + 1e-4, "{style:?}: {p:?}");
                    assert!(p.y >= cell.min.y && p.y <= cell.max.y, "{style:?}: {p:?}");
                }
            }
            // Doppelt: zwei waagerechte Linien mit Abstand.
            let d = underline_paths(Underline::Double, cell, t);
            assert_eq!(d.len(), 2);
            assert!((d[0][0].y - d[1][0].y - 2.0 * t).abs() < 1e-4);
            // Welle: beginnt und endet auf derselben Höhe (Anschluss an die Nachbarzelle).
            let w = &underline_paths(Underline::Curly, cell, t)[0];
            assert!((w[0].y - w[w.len() - 1].y).abs() < 1e-3);
            assert!((w[0].x - cell.min.x).abs() < 1e-4 && (w[w.len() - 1].x - cell.max.x).abs() < 1e-4);
        }
        // Punkte liegen auf einem festen Raster: Zelle bei x=8 setzt das Muster der Zelle bei x=0 fort.
        let a = underline_paths(Underline::Dotted, Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(8.0, 17.0)), t);
        let b = underline_paths(Underline::Dotted, Rect::from_min_size(Pos2::new(8.0, 0.0), Vec2::new(8.0, 17.0)), t);
        assert_eq!(a.len(), 4);
        assert_eq!(b[0][0].x, 8.0);
        assert!(a.iter().chain(&b).all(|s| (s[0].x / 2.0).fract() == 0.0));
    }
}
