//! Rahmen- und Blockzeichen (U+2500–U+259F) pixelgenau selbst zeichnen, damit Linien lückenlos
//! an die Nachbarzellen anschließen (Schriften füllen die Zelle selten exakt).

use eframe::egui::{Color32, Painter, Pos2, Rect, Vec2};

/// (oben, unten, links, rechts): 0 = keine, 1 = dünn, 2 = dick, 3 = doppelt
fn arms(c: char) -> Option<(u8, u8, u8, u8)> {
    Some(match c {
        '─' | '┄' | '┈' | '╌' => (0, 0, 1, 1),
        '━' | '┅' | '┉' | '╍' => (0, 0, 2, 2),
        '│' | '┆' | '┊' | '╎' => (1, 1, 0, 0),
        '┃' | '┇' | '┋' | '╏' => (2, 2, 0, 0),
        '┌' | '╭' => (0, 1, 0, 1),
        '┏' => (0, 2, 0, 2),
        '┐' | '╮' => (0, 1, 1, 0),
        '┓' => (0, 2, 2, 0),
        '└' | '╰' => (1, 0, 0, 1),
        '┗' => (2, 0, 0, 2),
        '┘' | '╯' => (1, 0, 1, 0),
        '┛' => (2, 0, 2, 0),
        '├' => (1, 1, 0, 1),
        '┣' => (2, 2, 0, 2),
        '┤' => (1, 1, 1, 0),
        '┫' => (2, 2, 2, 0),
        '┬' => (0, 1, 1, 1),
        '┳' => (0, 2, 2, 2),
        '┴' => (1, 0, 1, 1),
        '┻' => (2, 0, 2, 2),
        '┼' => (1, 1, 1, 1),
        '╋' => (2, 2, 2, 2),
        '═' => (0, 0, 3, 3),
        '║' => (3, 3, 0, 0),
        '╔' => (0, 3, 0, 3),
        '╗' => (0, 3, 3, 0),
        '╚' => (3, 0, 0, 3),
        '╝' => (3, 0, 3, 0),
        '╠' => (3, 3, 0, 3),
        '╣' => (3, 3, 3, 0),
        '╦' => (0, 3, 3, 3),
        '╩' => (3, 0, 3, 3),
        '╬' => (3, 3, 3, 3),
        _ => return None,
    })
}

/// Anteile der Zelle (x0, y0, x1, y1 in 0..1) und Deckkraft für Blockzeichen.
fn blocks(c: char) -> Option<(Vec<(f32, f32, f32, f32)>, f32)> {
    let u = c as u32;
    let f = |x0, y0, x1, y1| vec![(x0, y0, x1, y1)];
    Some(match u {
        0x2580 => (f(0.0, 0.0, 1.0, 0.5), 1.0),
        0x2581..=0x2587 => {
            let n = (u - 0x2580) as f32 / 8.0;
            (f(0.0, 1.0 - n, 1.0, 1.0), 1.0)
        }
        0x2588 => (f(0.0, 0.0, 1.0, 1.0), 1.0),
        0x2589..=0x258F => {
            let n = (0x2590 - u) as f32 / 8.0;
            (f(0.0, 0.0, n, 1.0), 1.0)
        }
        0x2590 => (f(0.5, 0.0, 1.0, 1.0), 1.0),
        0x2591 => (f(0.0, 0.0, 1.0, 1.0), 0.25),
        0x2592 => (f(0.0, 0.0, 1.0, 1.0), 0.5),
        0x2593 => (f(0.0, 0.0, 1.0, 1.0), 0.75),
        0x2594 => (f(0.0, 0.0, 1.0, 0.125), 1.0),
        0x2595 => (f(0.875, 0.0, 1.0, 1.0), 1.0),
        0x2596..=0x259F => {
            // Quadranten: Bits UL, UR, LL, LR
            let bits: u8 = match u {
                0x2596 => 0b0010,
                0x2597 => 0b0001,
                0x2598 => 0b1000,
                0x2599 => 0b1011,
                0x259A => 0b1001,
                0x259B => 0b1110,
                0x259C => 0b1101,
                0x259D => 0b0100,
                0x259E => 0b0110,
                _ => 0b0111,
            };
            let mut v = Vec::new();
            if bits & 0b1000 != 0 { v.push((0.0, 0.0, 0.5, 0.5)); }
            if bits & 0b0100 != 0 { v.push((0.5, 0.0, 1.0, 0.5)); }
            if bits & 0b0010 != 0 { v.push((0.0, 0.5, 0.5, 1.0)); }
            if bits & 0b0001 != 0 { v.push((0.5, 0.5, 1.0, 1.0)); }
            (v, 1.0)
        }
        _ => return None,
    })
}

/// Zeichnet `c`, falls es ein Rahmen-/Blockzeichen ist. Gibt `true` zurück, wenn gezeichnet wurde.
pub fn draw(p: &Painter, cell: Rect, c: char, fg: Color32, ppp: f32) -> bool {
    let u = c as u32;
    if !(0x2500..=0x259F).contains(&u) {
        return false;
    }
    let snap = |v: f32| (v * ppp).round() / ppp;
    if let Some((rects, alpha)) = blocks(c) {
        let col = if alpha < 1.0 { fg.gamma_multiply(alpha) } else { fg };
        for (x0, y0, x1, y1) in rects {
            let r = Rect::from_min_max(
                Pos2::new(snap(cell.min.x + cell.width() * x0), snap(cell.min.y + cell.height() * y0)),
                Pos2::new(snap(cell.min.x + cell.width() * x1), snap(cell.min.y + cell.height() * y1)),
            );
            p.rect_filled(r, 0.0, col);
        }
        return true;
    }
    let Some((up, down, left, right)) = arms(c) else { return false };
    let light = (cell.height() / 14.0).max(1.0 / ppp).max(1.0);
    let light = snap(light).max(1.0 / ppp);
    let thick = |w: u8| match w {
        2 => light * 2.0,
        _ => light,
    };
    let (cx, cy) = (snap(cell.center().x), snap(cell.center().y));
    let seg = |x0: f32, y0: f32, x1: f32, y1: f32| {
        p.rect_filled(Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)), 0.0, fg);
    };
    let gap = light * 1.5; // Abstand der Doppellinien von der Mitte
    // Jede Linie reicht bis zur Mitte (plus halbe Dicke), damit Ecken sauber schließen.
    let (hl, hr) = (left.max(0), right.max(0));
    let h_half = |w: u8| if w == 3 { gap + light / 2.0 } else { thick(w) / 2.0 };
    let v_half = |w: u8| if w == 3 { gap + light / 2.0 } else { thick(w) / 2.0 };
    let extend_v = v_half(up).max(v_half(down));
    let extend_h = h_half(hl).max(h_half(hr));
    // senkrechte Arme
    for (w, is_up) in [(up, true), (down, false)] {
        if w == 0 {
            continue;
        }
        let (y0, y1) = if is_up { (cell.min.y, cy + extend_h) } else { (cy - extend_h, cell.max.y) };
        if w == 3 {
            for off in [-gap, gap] {
                seg(cx + off - light / 2.0, y0, cx + off + light / 2.0, y1);
            }
        } else {
            let t = thick(w);
            seg(cx - t / 2.0, y0, cx + t / 2.0, y1);
        }
    }
    // waagerechte Arme
    for (w, is_left) in [(left, true), (right, false)] {
        if w == 0 {
            continue;
        }
        let (x0, x1) = if is_left { (cell.min.x, cx + extend_v) } else { (cx - extend_v, cell.max.x) };
        if w == 3 {
            for off in [-gap, gap] {
                seg(x0, cy + off - light / 2.0, x1, cy + off + light / 2.0);
            }
        } else {
            let t = thick(w);
            seg(x0, cy - t / 2.0, x1, cy + t / 2.0);
        }
    }
    let _ = Vec2::ZERO;
    true
}
