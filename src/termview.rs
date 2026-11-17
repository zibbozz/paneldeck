//! Zeichnen eines Terminals und Verarbeitung von Tastatur/Maus.

use crate::terminal::Session;
use crate::vtkeys;
use crate::theme::Theme;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use eframe::egui::{
    self, Align2, Color32, Event, EventFilter, FontId, Id, Key, Modifiers, Pos2, Rect, Response,
    Sense, Stroke, StrokeKind, Ui, Vec2,
};

const PAD: f32 = 8.0;

/// Gleiche von Programmen gesetzte Farben? (`Colors` hat kein `PartialEq`.)
fn same_colors(a: &alacritty_terminal::term::color::Colors, b: &alacritty_terminal::term::color::Colors) -> bool {
    (0..alacritty_terminal::term::color::COUNT).all(|i| a[i] == b[i])
}

pub fn cell_size(ui: &Ui, font: &FontId) -> Vec2 {
    let g = ui.painter().layout_no_wrap("M".to_string(), font.clone(), Color32::WHITE);
    Vec2::new(g.size().x, g.size().y)
}

/// Zeichnet das Terminal in `rect` und verarbeitet Eingaben. Gibt die Interaktions-Antwort zurück.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    session: &mut Session,
    theme: &Theme,
    font: &FontId,
    cwd: &str,
    alt_wheel: crate::state::AltScreenWheel,
) -> Response {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.clicked() || response.drag_started() || response.secondary_clicked() {
        response.request_focus();
    }
    let focused = response.has_focus();
    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
            )
        });
    }

    let cs = cell_size(ui, font);
    let inner = rect.shrink(PAD);
    let cols = (inner.width() / cs.x).floor().max(2.0) as u16;
    let rows = (inner.height() / cs.y).floor().max(1.0) as u16;
    // Für Antworten an das Programm (Farbabfragen OSC 4/10/11/12, Pixelgrößen CSI 14t/16t).
    let ppp = ui.ctx().pixels_per_point();
    session.set_view(theme, (cs.x * ppp).round().clamp(0.0, u16::MAX as f32) as u16, (cs.y * ppp).round().clamp(0.0, u16::MAX as f32) as u16);
    session.resize(cols, rows);

    if focused {
        handle_keyboard(ui, session);
    }
    let (opened_link, osc8_hover) = handle_links(ui, &response, session, inner, cs, cwd);
    if !opened_link {
        handle_mouse(ui, &response, session, inner, cs, alt_wheel);
    }
    let cursor_on = cursor_blink_phase(ui, id, session, focused);
    paint(ui, rect, inner, session, theme, font, cs, focused, cursor_on, osc8_hover.as_ref());
    response
}

/// Einstellung „Blinkender Cursor“ (#183); die App schreibt sie jedes Bild (wie `mcp::LEVEL`).
pub static CURSOR_BLINK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// Halbe Blinkperiode (Sekunden), wie der Standard von Windows.
const BLINK_HALF: f64 = 0.53;
/// Nach so langer Zeit ohne Eingabe hört der Cursor auf zu blinken (spart Neuzeichnen).
const BLINK_IDLE: f64 = 30.0;

/// Ist der Cursor in dieser Blinkphase sichtbar? Blinkt nur, wenn das Programm es anfordert (DECSCUSR 1/3/5,
/// `CSI ? 12 h`), die Einstellung an ist und das Terminal im Fenster im Vordergrund den Fokus hat; jede Eingabe
/// beginnt die Phase neu (beim Tippen bleibt er sichtbar). Fordert das nächste Neuzeichnen zum Phasenwechsel an.
fn cursor_blink_phase(ui: &Ui, id: Id, session: &Session, focused: bool) -> bool {
    let blinking = session.term.lock().unwrap_or_else(|e| e.into_inner()).cursor_style().blinking;
    if !(focused && blinking && CURSOR_BLINK.load(std::sync::atomic::Ordering::Relaxed) && ui.input(|i| i.focused)) {
        return true;
    }
    let key = id.with("cursor-blink");
    let (now, typed) = ui.input(|i| (i.time, i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. } | Event::Text(_) | Event::Paste(_)))));
    let epoch = match ui.data(|d| d.get_temp::<f64>(key)) {
        Some(t) if !typed && t <= now => t,
        _ => now,
    };
    ui.data_mut(|d| d.insert_temp(key, epoch));
    let (on, next) = blink_state(now - epoch);
    if let Some(s) = next {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(s));
    }
    on
}

/// Blinkzustand nach `elapsed` Sekunden seit der letzten Eingabe: (sichtbar, Sekunden bis zum nächsten Wechsel).
fn blink_state(elapsed: f64) -> (bool, Option<f64>) {
    if elapsed >= BLINK_IDLE {
        return (true, None);
    }
    let phase = (elapsed / BLINK_HALF).floor() as i64;
    let next = (BLINK_HALF - elapsed % BLINK_HALF).min(BLINK_IDLE - elapsed).max(0.01);
    (phase % 2 == 0, Some(next))
}

/// Schriftfamilien der Schnitte fett/kursiv (einmal angelegt, statt je Zelle einen `Arc<str>`).
fn style_family(name: &'static str) -> egui::FontFamily {
    static FAMS: std::sync::OnceLock<[egui::FontFamily; 3]> = std::sync::OnceLock::new();
    let f = FAMS.get_or_init(|| {
        [crate::sysfonts::FAMILY_BOLD, crate::sysfonts::FAMILY_ITALIC, crate::sysfonts::FAMILY_BOLD_ITALIC].map(|n| egui::FontFamily::Name(n.into()))
    });
    match name {
        crate::sysfonts::FAMILY_BOLD => f[0].clone(),
        crate::sysfonts::FAMILY_ITALIC => f[1].clone(),
        _ => f[2].clone(),
    }
}

/// Neigung nachgezeichneter Kursivschrift (x-Versatz je Höheneinheit über der Grundlinie).
const SLANT: f32 = 0.2;

/// Text einer Zelle mit SGR-Fett/Kursiv zeichnen: echter Schnitt, wenn vorhanden, sonst nachgezeichnet (fett: zweimal
/// um ein Pixel versetzt; kursiv: die Glyphen-Dreiecke geschert).
#[allow(clippy::too_many_arguments)]
fn draw_cell_text(ui: &Ui, painter: &egui::Painter, cell: Rect, width: f32, wide: bool, s: String, font: &FontId, fg: Color32, bold: bool, italic: bool, ppp: f32) {
    let plan = crate::sysfonts::plan(bold, italic);
    let f = match plan.family {
        Some(n) => FontId::new(font.size, style_family(n)),
        None => font.clone(),
    };
    let g = painter.layout_no_wrap(s, f, fg);
    let x = if wide { cell.min.x + ((width - g.size().x) / 2.0).max(0.0) } else { cell.min.x };
    let snap = |v: f32| (v * ppp).round() / ppp;
    let pos = Pos2::new(snap(x), snap(cell.min.y));
    let dx = snap((font.size / 20.0).max(1.0 / ppp)).max(1.0 / ppp);
    if plan.fake_italic {
        let base = cell.max.y - cell.height() * 0.22;
        let size = ui.ctx().fonts(|fo| fo.font_image_size());
        let mesh = skewed_text(&g, pos, base, fg, size);
        if plan.fake_bold {
            let mut m2 = mesh.clone();
            m2.translate(Vec2::new(dx, 0.0));
            painter.add(egui::Shape::mesh(m2));
        }
        painter.add(egui::Shape::mesh(mesh));
    } else {
        if plan.fake_bold {
            painter.galley(pos + Vec2::new(dx, 0.0), g.clone(), fg);
        }
        painter.galley(pos, g, fg);
    }
}

/// Glyphen eines Galleys als geschertes Dreiecksnetz (nachgezeichnete Kursivschrift). `tex_size` = Größe des
/// Schrift-Atlas (die Netze im Galley tragen Texel-Koordinaten, die der Tessellator sonst selbst umrechnet).
fn skewed_text(g: &egui::Galley, pos: Pos2, base: f32, fg: Color32, tex_size: [usize; 2]) -> egui::Mesh {
    let norm = Vec2::new(1.0 / tex_size[0].max(1) as f32, 1.0 / tex_size[1].max(1) as f32);
    let mut mesh = egui::Mesh::default();
    for row in &g.rows {
        let m = &row.visuals.mesh;
        if m.is_empty() {
            continue;
        }
        mesh.texture_id = m.texture_id;
        let off = mesh.vertices.len() as u32;
        mesh.indices.extend(m.indices.iter().map(|i| i + off));
        mesh.vertices.extend(m.vertices.iter().map(|v| {
            let p = pos + row.pos.to_vec2() + v.pos.to_vec2();
            egui::epaint::Vertex { pos: Pos2::new(p.x + (base - p.y) * SLANT, p.y), uv: (v.uv.to_vec2() * norm).to_pos2(), color: fg }
        }));
    }
    mesh
}

fn paint(
    ui: &Ui,
    rect: Rect,
    inner: Rect,
    session: &Session,
    theme: &Theme,
    font: &FontId,
    cs: Vec2,
    focused: bool,
    cursor_on: bool,
    osc8_hover: Option<&alacritty_terminal::term::cell::Hyperlink>,
) {
    let ppp = ui.ctx().pixels_per_point();
    let painter = ui.painter_at(rect);

    let term = session.term.lock().unwrap_or_else(|e| e.into_inner());
    // Von Programmen gesetzte Farben (OSC 4/10/11/12; zurückgesetzt mit OSC 104/110–112) vor denen des Themes.
    let ov = *term.colors();
    let mut cache = session.theme_cache.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = cache.as_ref().is_some_and(|(base, colors, _)| base == theme && same_colors(colors, &ov));
    if !fresh {
        *cache = theme.with_overrides(&ov).map(|t| (theme.clone(), Box::new(ov), t));
    }
    let theme = cache.as_ref().map(|(_, _, t)| t).unwrap_or(theme);
    painter.rect_filled(rect, 0.0, theme.bg);
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let rows = session.rows as i32;
    let selection = content.selection;
    let cursor = content.cursor;

    // Suchtreffer
    if let Some(st) = &session.search {
        let hl = Color32::from_rgba_unmultiplied(0xe5, 0xc0, 0x7b, 90);
        let hl_cur = Color32::from_rgba_unmultiplied(0xd9, 0x77, 0x57, 170);
        for (i, m) in st.matches.iter().enumerate() {
            let row = m.line + offset;
            if row < 0 || row >= rows {
                continue;
            }
            let r = Rect::from_min_size(
                Pos2::new(inner.min.x + m.col as f32 * cs.x, inner.min.y + row as f32 * cs.y),
                Vec2::new(m.len as f32 * cs.x, cs.y),
            );
            painter.rect_filled(r, 2.0, if i == st.current { hl_cur } else { hl });
        }
    }

    let cells: Vec<_> = content.display_iter.collect();
    let mut skip_until = 0usize;
    // OSC-8-Unterstreichung je Zeile als zusammenhängender Lauf (gepunktet bzw. beim Strg-Hover durchgezogen), damit das
    // Punktmuster nicht an jeder Zelle neu ansetzt: (Rechteck, Farbe, hervorgehoben).
    let mut link_run: Option<(Rect, Color32, bool)> = None;
    let flush_link = |run: Option<(Rect, Color32, bool)>| {
        if let Some((r, col, hot)) = run {
            if hot {
                let yy = r.max.y - 1.5;
                painter.line_segment([Pos2::new(r.min.x, yy), Pos2::new(r.max.x, yy)], Stroke::new(1.5_f32, crate::style::ACCENT));
            } else {
                crate::glyphs::draw_underline(&painter, crate::glyphs::Underline::Dotted, r, col, ppp);
            }
        }
    };
    for (idx, ic) in cells.iter().enumerate() {
        if idx < skip_until {
            continue;
        }
        let row = ic.point.line.0 + offset;
        if row < 0 || row >= rows {
            continue;
        }
        let cell = ic.cell;
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let x = inner.min.x + ic.point.column.0 as f32 * cs.x;
        let y = inner.min.y + row as f32 * cs.y;
        let width = if flags.contains(Flags::WIDE_CHAR) { cs.x * 2.0 } else { cs.x };
        let bold = flags.contains(Flags::BOLD);
        let dim = flags.contains(Flags::DIM);

        let mut fg = theme.resolve_ov(cell.fg, bold, dim, &ov);
        let mut bg = theme.resolve_ov(cell.bg, false, false, &ov);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let selected = selection.is_some_and(|s| s.contains(Point::new(ic.point.line, ic.point.column)));
        if selected {
            bg = theme.selection;
        }
        fg = ensure_contrast(fg, bg);
        let cell_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, cs.y));
        if bg != theme.bg {
            painter.rect_filled(cell_rect, 0.0, bg);
        }
        let ch = cell.c;
        // Emoji-Sequenzen (ZWJ, Flaggen, Hautfarbe, Keycap) zu einem Bild zusammensetzen.
        if ch != ' ' && !flags.contains(Flags::HIDDEN) {
            if let Some((gid, used, span)) = emoji_cluster(&cells, idx) {
                let side = if span >= 2 { (cs.x * 2.0).min(cs.y) } else { cs.y * 0.85 };
                if let Some((tex, aspect)) = crate::emoji::texture_gid(ui.ctx(), gid, (side * ppp).round() as u32) {
                    let (w, h) = if aspect >= 1.0 { (side, side / aspect) } else { (side * aspect, side) };
                    let r = Rect::from_center_size(Pos2::new(cell_rect.min.x + span as f32 * cs.x / 2.0, cell_rect.center().y), Vec2::new(w, h));
                    painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    skip_until = idx + used;
                    continue;
                }
            }
        }
        if ch != ' ' && ch != '\0' && !flags.contains(Flags::HIDDEN) {
            let zw = cell.zerowidth();
            let vs16 = zw.is_some_and(|z| z.contains(&'\u{fe0f}'));
            let wide = flags.contains(Flags::WIDE_CHAR);
            let mut drawn = crate::boxdraw::draw(&painter, cell_rect, ch, fg, ppp) || crate::glyphs::draw_powerline(&painter, cell_rect, ch, fg, ppp);
            if !drawn && crate::emoji::wants_color(ch, wide, vs16) {
                let side = if wide { (cs.x * 2.0).min(cs.y) } else { cs.y * 0.85 };
                if let Some((tex, aspect)) = crate::emoji::texture(ui.ctx(), ch, (side * ppp).round() as u32) {
                    let (w, h) = if aspect >= 1.0 { (side, side / aspect) } else { (side * aspect, side) };
                    let r = Rect::from_center_size(Pos2::new(cell_rect.min.x + width / 2.0, cell_rect.center().y), Vec2::new(w, h));
                    painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    drawn = true;
                }
            }
            if !drawn {
                // Kombinationszeichen mitzeichnen; Variationsselektoren/ZWJ weglassen (Schriften haben dafür keine Glyphe).
                let mut s = String::with_capacity(8);
                s.push(ch);
                if let Some(z) = zw {
                    s.extend(z.iter().filter(|c| !matches!(**c, '\u{fe0e}' | '\u{fe0f}' | '\u{200d}' | '\u{200b}')));
                }
                // Breite Zeichen (CJK u. a.) werden in den zwei Zellen zentriert.
                draw_cell_text(ui, &painter, cell_rect, width, wide, s, font, fg, bold, flags.contains(Flags::ITALIC), ppp);
            }
        }
        // Unterstreichung (SGR 4:1–4:5, Farbe SGR 58); OSC-8-Links ohne eigene Unterstreichung dezent gepunktet.
        let link = cell.hyperlink();
        let link_hovered = osc8_hover.is_some_and(|h| link.as_ref() == Some(h));
        if let Some(style) = crate::glyphs::underline_of(flags) {
            let ucol = cell.underline_color().map(|c| theme.resolve(c, false, false)).unwrap_or(fg);
            crate::glyphs::draw_underline(&painter, style, cell_rect, ucol, ppp);
        }
        let seg = if link_hovered {
            Some((cell_rect, crate::style::ACCENT, true))
        } else if link.is_some() && crate::glyphs::underline_of(flags).is_none() {
            Some((cell_rect, fg.gamma_multiply(0.6), false))
        } else {
            None
        };
        if let Some((r, col, hot)) = seg {
            match &mut link_run {
                // Anschluss in derselben Zeile mit gleicher Art und Farbe: verlängern.
                Some((lr, lc, lh)) if (lr.max.x - r.min.x).abs() < 0.5 && lr.min.y == r.min.y && *lc == col && *lh == hot => lr.max.x = r.max.x,
                _ => flush_link(link_run.replace((r, col, hot))),
            }
        }
        if flags.contains(Flags::STRIKEOUT) {
            let yy = cell_rect.center().y;
            painter.line_segment(
                [Pos2::new(cell_rect.min.x, yy), Pos2::new(cell_rect.max.x, yy)],
                Stroke::new(1.0_f32, fg),
            );
        }
    }

    flush_link(link_run.take());

    // Cursor
    if cursor.shape != CursorShape::Hidden {
        let row = cursor.point.line.0 + offset;
        if row >= 0 && row < rows {
            let x = inner.min.x + cursor.point.column.0 as f32 * cs.x;
            let y = inner.min.y + row as f32 * cs.y;
            let cr = Rect::from_min_size(Pos2::new(x, y), cs);
            if !focused {
                painter.rect_stroke(cr, 0.0, Stroke::new(1.0_f32, theme.cursor), StrokeKind::Inside);
            } else if !cursor_on {
                // Blinkender Cursor in der dunklen Phase: nichts zeichnen (das Zeichen darunter steht schon da).
            } else {
                match cursor.shape {
                    CursorShape::Beam => {
                        painter.rect_filled(
                            Rect::from_min_size(cr.min, Vec2::new(2.0, cs.y)),
                            0.0,
                            theme.cursor,
                        );
                    }
                    CursorShape::Underline => {
                        painter.rect_filled(
                            Rect::from_min_size(Pos2::new(cr.min.x, cr.max.y - 2.0), Vec2::new(cs.x, 2.0)),
                            0.0,
                            theme.cursor,
                        );
                    }
                    CursorShape::HollowBlock => {
                        painter.rect_stroke(cr, 0.0, Stroke::new(1.0_f32, theme.cursor), StrokeKind::Inside);
                    }
                    _ => {
                        painter.rect_filled(cr, 0.0, theme.cursor);
                        let ch = term.grid()[cursor.point].c;
                        if ch != ' ' && ch != '\0' {
                            painter.text(cr.min, Align2::LEFT_TOP, ch, font.clone(), theme.bg);
                        }
                    }
                }
            }
        }
    }

    // Strg-Hover: Link unterstreichen
    if let Some((row, c0, c1, _)) = &session.hover_link {
        let y = inner.min.y + (*row as f32 + 1.0) * cs.y - 1.5;
        painter.line_segment(
            [Pos2::new(inner.min.x + *c0 as f32 * cs.x, y), Pos2::new(inner.min.x + *c1 as f32 * cs.x, y)],
            Stroke::new(1.5_f32, crate::style::ACCENT),
        );
    }

    // Scroll-Indikator
    let history = term.grid().total_lines().saturating_sub(term.grid().screen_lines());
    if history > 0 && offset > 0 {
        let frac = 1.0 - offset as f32 / history as f32;
        let h = 24.0;
        let y = rect.min.y + (rect.height() - h) * frac;
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(rect.max.x - 4.0, y), Vec2::new(3.0, h)),
            1.5,
            theme.fg.gamma_multiply(0.4),
        );
    }
}

fn cps_of(cell: &alacritty_terminal::term::cell::Cell) -> Vec<char> {
    let mut v = vec![cell.c];
    if let Some(z) = cell.zerowidth() {
        v.extend_from_slice(z);
    }
    v
}

/// Erkennt ab Zelle `i` ein zusammengesetztes Emoji. Gibt (Glyph, belegte Zellen, Breite in Zellen) zurück.
fn emoji_cluster(
    cells: &[alacritty_terminal::grid::Indexed<&alacritty_terminal::term::cell::Cell>],
    i: usize,
) -> Option<(ttf_parser::GlyphId, usize, usize)> {
    let c0 = cells[i].cell;
    let ch = c0.c;
    let w0 = c0.flags.contains(Flags::WIDE_CHAR);
    let zw0 = c0.zerowidth().unwrap_or(&[]);
    let starts = crate::emoji::is_regional(ch) || zw0.contains(&'\u{20e3}') || crate::emoji::wants_color(ch, w0, zw0.contains(&'\u{fe0f}'));
    if !starts {
        return None;
    }
    let mut cps = cps_of(c0);
    let mut used = 1 + w0 as usize; // bei breiten Zeichen folgt eine Platzhalterzelle
    let mut span = if w0 { 2 } else { 1 };
    loop {
        let Some(next) = cells.get(i + used) else { break };
        if next.point.line != cells[i].point.line || next.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            break;
        }
        let (nc, nw) = (next.cell, next.cell.flags.contains(Flags::WIDE_CHAR));
        let nzw = nc.zerowidth().unwrap_or(&[]);
        let after_zwj = cps.last() == Some(&'\u{200d}');
        let joins = (after_zwj && crate::emoji::wants_color(nc.c, nw, nzw.contains(&'\u{fe0f}')))
            || crate::emoji::is_modifier(nc.c)
            || (crate::emoji::is_regional(ch) && crate::emoji::is_regional(nc.c) && cps.iter().filter(|c| crate::emoji::is_regional(**c)).count() == 1);
        if !joins {
            break;
        }
        if crate::emoji::is_regional(nc.c) {
            span = 2; // Flagge: zwei einspaltige Zellen
        }
        cps.extend(cps_of(nc));
        used += 1 + nw as usize;
    }
    if cps.len() == 1 {
        return None; // einzelnes Zeichen: normaler Weg
    }
    crate::emoji::shape(&cps).map(|g| (g, used, span))
}

fn point_at(session: &Session, inner: Rect, cs: Vec2, pos: Pos2) -> (Point, Side) {
    let rel = pos - inner.min;
    let col = (rel.x / cs.x).floor().clamp(0.0, session.cols as f32 - 1.0) as usize;
    let row = (rel.y / cs.y).floor().clamp(0.0, session.rows as f32 - 1.0) as i32;
    let side = if (rel.x / cs.x).fract() < 0.5 { Side::Left } else { Side::Right };
    let offset = session.term.lock().unwrap_or_else(|e| e.into_inner()).grid().display_offset() as i32;
    (Point::new(Line(row - offset), Column(col)), side)
}

/// Kodierung der Mausereignisse, wie sie das Programm angefordert hat: SGR (DECSET 1006), UTF-8 (1005) oder die
/// Standard-Kodierung (X10/„normal“, ohne eigene Anforderung). 1006 und 1005 schließen sich in alacritty_terminal aus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseEncoding {
    Normal,
    Utf8,
    Sgr,
}

impl MouseEncoding {
    pub fn of(mode: TermMode) -> Self {
        if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Normal
        }
    }
}

/// Größte Spalte/Zeile (0-basiert) der Standard-Kodierung. Ein Byte `32 + 1 + Position` reichte zwar bis 255, aber
/// ConPTY liest die Eingabe des Terminals als UTF-8: Einzelbytes 0x80–0xFF sind dort ungültig und kämen als U+FFFD
/// (falsche Position) beim Programm an. Deshalb nur 7-Bit-Werte (bis 127 = Position 94) senden, darüber nichts –
/// wie Windows Terminal. Programme, die breitere Terminals brauchen, fordern SGR (1006) an.
pub const NORMAL_MOUSE_MAX: usize = 0x7F - 33;
/// Größte Spalte/Zeile (0-basiert) der UTF-8-Kodierung: Zeichen bis U+07FF (zwei Bytes), wie in xterm.
pub const UTF8_MOUSE_MAX: usize = 0x7FF - 33;
/// Tastencodes des Mausrads (xterm): hoch/runter; Modifikatoren werden addiert (Alt +8, Strg +16).
pub const WHEEL_UP: u8 = 64;
pub const WHEEL_DOWN: u8 = 65;

/// Modifikator-Bits für Mausereignisse (xterm: Umschalt +4, Alt/Meta +8, Strg +16). Umschalt wird nie gemeldet:
/// Umschalt+Maus gehört Paneldeck (Auswahl, Verlauf scrollen).
pub fn mouse_mod_bits(m: Modifiers) -> u8 {
    (m.alt as u8) * 8 + (m.ctrl as u8) * 16
}

/// Maus-Ereignis für Programme mit Maus-Modus kodieren (`cb` = Tastencode inkl. Modifikatoren, `col`/`row` 0-basiert
/// im sichtbaren Bereich). `None`, wenn die Kodierung die Position nicht darstellen kann (Standard-Kodierung ab
/// Spalte/Zeile 95, 0-basiert, siehe `NORMAL_MOUSE_MAX`): dann wird nichts gesendet statt einer falschen Position.
pub fn mouse_seq(enc: MouseEncoding, cb: u8, col: usize, row: usize, release: bool) -> Option<Vec<u8>> {
    let max = match enc {
        MouseEncoding::Sgr => {
            return Some(format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, if release { 'm' } else { 'M' }).into_bytes());
        }
        MouseEncoding::Utf8 => UTF8_MOUSE_MAX,
        MouseEncoding::Normal => NORMAL_MOUSE_MAX,
    };
    if col > max || row > max {
        return None;
    }
    // Ohne SGR kennt das Loslassen keine Taste: Code 3, Modifikatoren bleiben erhalten (xterm).
    let b = if release { 3 | (cb & 0b1_1100) } else { cb };
    let mut out = vec![0x1b, b'[', b'M'];
    for v in [32 + b as u32, 33 + col as u32, 33 + row as u32] {
        if v < 0x80 || enc == MouseEncoding::Normal {
            out.push(v as u8);
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(char::from_u32(v)?.encode_utf8(&mut buf).as_bytes());
        }
    }
    Some(out)
}

/// Zelle (Spalte, Zeile; 0-basiert im sichtbaren Bereich) unter `pos`, auf das Terminal begrenzt.
fn cell_at(s: &Session, inner: Rect, cs: Vec2, pos: Pos2) -> (usize, usize) {
    let col = ((pos.x - inner.min.x) / cs.x).floor().clamp(0.0, s.cols as f32 - 1.0) as usize;
    let row = ((pos.y - inner.min.y) / cs.y).floor().clamp(0.0, s.rows as f32 - 1.0) as usize;
    (col, row)
}

fn write_mouse(s: &Session, seq: Option<Vec<u8>>) {
    if let Some(seq) = seq {
        s.write(&seq);
    }
}

fn forward_mouse(ui: &Ui, resp: &Response, s: &mut Session, inner: Rect, cs: Vec2, mode: TermMode) {
    let mbits = mouse_mod_bits(ui.input(|i| i.modifiers));
    let enc = MouseEncoding::of(mode);
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()).or(resp.interact_pointer_pos()) else { return };
    if !resp.hovered() && s.mouse_down.is_none() {
        return;
    }
    let (col, row) = cell_at(s, inner, cs, pos);
    for (b, btn) in [(0u8, egui::PointerButton::Primary), (1, egui::PointerButton::Middle), (2, egui::PointerButton::Secondary)] {
        if resp.hovered() && ui.input(|i| i.pointer.button_pressed(btn)) {
            write_mouse(s, mouse_seq(enc, b | mbits, col, row, false));
            s.mouse_down = Some(b);
            s.mouse_last = Some((col, row));
        }
        if s.mouse_down == Some(b) && ui.input(|i| i.pointer.button_released(btn)) {
            write_mouse(s, mouse_seq(enc, b | mbits, col, row, true));
            s.mouse_down = None;
        }
    }
    if s.mouse_last != Some((col, row)) {
        if let Some(b) = s.mouse_down {
            if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                write_mouse(s, mouse_seq(enc, (b + 32) | mbits, col, row, false));
            }
        } else if mode.contains(TermMode::MOUSE_MOTION) && resp.hovered() {
            write_mouse(s, mouse_seq(enc, 35 | mbits, col, row, false));
        }
        s.mouse_last = Some((col, row));
    }
}

/// Was eine Drehung des Mausrads bewirkt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WheelAction {
    /// Als Mausereignis (Tasten 64/65) an das Programm melden.
    Report,
    /// Pfeiltasten hoch/runter senden (Alternate Scroll Mode).
    Arrows,
    /// Den eigenen Verlauf (Scrollback) von Paneldeck scrollen.
    Scrollback,
    /// Nichts tun.
    Nothing,
}

/// Entscheidung für das Mausrad (wie xterm/Windows Terminal):
/// - Umschalt+Rad scrollt immer den Verlauf; im alternativen Bildschirm gibt es keinen, dann passiert nichts.
/// - Hat das Programm Mausereignisse angefordert (1000/1002/1003, jede Kodierung), bekommt es das Rad gemeldet.
/// - Im alternativen Bildschirm ohne Maus-Modus: Pfeiltasten nur, wenn das Programm den Alternate Scroll Mode
///   (DECSET 1007, Standard an) nicht abgeschaltet hat und die Einstellung sie erlaubt.
/// - Sonst wird der Verlauf gescrollt.
///
/// `stale` (`Session::stale_modes`): Die Shell steht schon wieder am Prompt, obwohl ein Vollbildprogramm seine
/// Modi nicht zurückgesetzt hat. Dann bleiben die Modi unangetastet, das Rad verhält sich aber wie in einer normalen
/// Shell (keine Pfeiltasten in die Befehlszeile, keine Mausereignisse als Zeichensalat).
pub fn wheel_action(mode: TermMode, shift: bool, setting: crate::state::AltScreenWheel, stale: bool) -> WheelAction {
    let alt = mode.contains(TermMode::ALT_SCREEN);
    if shift || stale {
        return if alt { WheelAction::Nothing } else { WheelAction::Scrollback };
    }
    if mode.intersects(TermMode::MOUSE_MODE) {
        return WheelAction::Report;
    }
    if !alt {
        return WheelAction::Scrollback;
    }
    if mode.contains(TermMode::ALTERNATE_SCROLL) && setting == crate::state::AltScreenWheel::Arrows {
        WheelAction::Arrows
    } else {
        WheelAction::Nothing
    }
}

/// Pfeiltaste für eine Rad-Raste (`up` = vom Nutzer weg), je nach Cursor-Tasten-Modus (DECCKM).
pub fn wheel_arrow(up: bool, app_cursor: bool) -> &'static [u8] {
    match (up, app_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    }
}

/// Strg+Klick auf Links/Pfade und OSC-8-Links. Gibt zurück, ob geklickt wurde (dann keine weitere Mausbehandlung),
/// und den OSC-8-Link unter dem Mauszeiger bei gedrückter Strg-Taste (zum Unterstreichen).
fn handle_links(ui: &Ui, response: &Response, session: &mut Session, inner: Rect, cs: Vec2, cwd: &str) -> (bool, Option<alacritty_terminal::term::cell::Hyperlink>) {
    session.hover_link = None;
    if !response.hovered() {
        return (false, None);
    }
    let Some(pos) = response.hover_pos() else { return (false, None) };
    let col = ((pos.x - inner.min.x) / cs.x).floor();
    let row = ((pos.y - inner.min.y) / cs.y).floor();
    if col < 0.0 || row < 0.0 || row as u16 >= session.rows || col as u16 >= session.cols {
        return (false, None);
    }
    let ctrl = ui.input(|i| i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt);
    // OSC 8: das Programm hat Text und Ziel getrennt gesetzt – das Ziel zeigt der Tooltip.
    let osc8 = {
        let t = session.term.lock().unwrap_or_else(|e| e.into_inner());
        let off = t.grid().display_offset() as i32;
        t.grid()[Point::new(Line(row as i32 - off), Column(col as usize))].hyperlink()
    };
    if let Some(h) = osc8 {
        use crate::hyperlink::LinkKind;
        let kind = crate::hyperlink::classify(h.uri());
        let (resting, idle) = ui.input(|i| (i.pointer.time_since_last_movement(), !i.pointer.any_down()));
        if idle && (ctrl || resting >= TOOLTIP_DELAY) {
            link_tooltip(ui, response, h.uri(), &kind);
        } else if idle {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32(TOOLTIP_DELAY - resting + 0.02));
        }
        if !ctrl {
            return (false, None);
        }
        if !matches!(kind, LinkKind::Denied(_)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked_by(egui::PointerButton::Primary) {
            match kind {
                LinkKind::Web => crate::hyperlink::open(h.uri()),
                LinkKind::Ask { .. } => crate::hyperlink::request_confirm(ui.ctx(), h.uri()),
                LinkKind::Denied(_) => {}
            }
            return (true, Some(h));
        }
        return (false, Some(h));
    }
    if !ctrl {
        return (false, None);
    }
    let chars = session.row_chars(row as usize);
    let Some(link) = crate::links::find(&chars, col as usize, cwd, &session.shell_id) else { return (false, None) };
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    session.hover_link = Some((row as usize, link.start, link.end, link.target.clone()));
    if response.clicked_by(egui::PointerButton::Primary) {
        crate::links::open(&link.target);
        return (true, None);
    }
    (false, None)
}

/// Ruhezeit des Mauszeigers über einem OSC-8-Link, bis der Tooltip ohne Strg erscheint (Sekunden).
const TOOLTIP_DELAY: f32 = 0.6;

/// Tooltip mit dem echten Ziel eines OSC-8-Links (bereinigt und gekürzt; die Rückfrage zeigt es ganz).
fn link_tooltip(ui: &Ui, response: &Response, uri: &str, kind: &crate::hyperlink::LinkKind) {
    use crate::hyperlink::LinkKind;
    use crate::state::t;
    // Bereinigt und in der Mitte gekürzt (Anfang und Dateiende bleiben sichtbar); der Rechnername eigens darunter.
    let target = crate::hyperlink::middle_truncate(&crate::vtnotify::sanitize(uri, crate::hyperlink::URI_MAX), 160);
    let host = crate::hyperlink::host(uri).filter(|h| !h.is_empty()).map(|h| crate::vtnotify::sanitize(h, 120));
    let hint = match kind {
        LinkKind::Web => t("Strg+Klick öffnet den Link", "Ctrl+click opens the link"),
        LinkKind::Ask { .. } => t("Strg+Klick: vor dem Öffnen wird nachgefragt", "Ctrl+click: asks before opening"),
        LinkKind::Denied(_) => t("Dieser Link wird nicht geöffnet (Schema oder Ziel nicht erlaubt)", "This link is not opened (scheme or target not allowed)"),
    };
    egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), response.id.with("osc8-tip"), egui::PopupAnchor::Pointer).gap(12.0).show(|ui| {
        ui.label(egui::RichText::new(target).monospace());
        if let Some(h) = host {
            ui.label(egui::RichText::new(format!("{} {h}", t("Rechner:", "Host:"))).monospace().strong());
        }
        ui.label(egui::RichText::new(hint).small());
    });
}

fn handle_mouse(ui: &Ui, response: &Response, session: &mut Session, inner: Rect, cs: Vec2, alt_wheel: crate::state::AltScreenWheel) {
    let mode = *session.term.lock().unwrap_or_else(|e| e.into_inner()).mode();
    let stale = session.stale_modes.load(std::sync::atomic::Ordering::Relaxed);
    let forwarding = mode.intersects(TermMode::MOUSE_MODE) && !ui.input(|i| i.modifiers.shift) && !stale;
    if forwarding {
        forward_mouse(ui, response, session, inner, cs, mode);
    } else if let Some(b) = session.mouse_down.take() {
        // Weitergeleiteter Klick/Zug endet hier (z. B. Umschalt während des Ziehens): dem Programm das Loslassen
        // melden, sonst glaubt es die Taste weiter gedrückt.
        if mode.intersects(TermMode::MOUSE_MODE) {
            if let Some((col, row)) = session.mouse_last {
                write_mouse(session, mouse_seq(MouseEncoding::of(mode), b, col, row, true));
            }
        }
    }
    if let Some(pos) = response.interact_pointer_pos().filter(|_| !forwarding) {
        if response.triple_clicked() {
            let (p, s) = point_at(session, inner, cs, pos);
            session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = Some(Selection::new(SelectionType::Lines, p, s));
        } else if response.double_clicked() {
            let (p, s) = point_at(session, inner, cs, pos);
            session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = Some(Selection::new(SelectionType::Semantic, p, s));
        } else if response.drag_started_by(egui::PointerButton::Primary) {
            let (p, s) = point_at(session, inner, cs, pos);
            session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = Some(Selection::new(SelectionType::Simple, p, s));
        } else if response.dragged_by(egui::PointerButton::Primary) {
            let (p, s) = point_at(session, inner, cs, pos);
            if let Some(sel) = session.term.lock().unwrap_or_else(|e| e.into_inner()).selection.as_mut() {
                sel.update(p, s);
            }
        } else if response.clicked_by(egui::PointerButton::Primary) {
            session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = None;
        }
    }

    // Rechtsklick: Auswahl kopieren, sonst einfügen (wie Windows Terminal).
    if response.secondary_clicked() && !forwarding {
        let text = session.term.lock().unwrap_or_else(|e| e.into_inner()).selection_to_string();
        match text {
            Some(t) if !t.is_empty() => {
                ui.ctx().copy_text(t);
                session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = None;
            }
            _ => ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste),
        }
    }

    // Mausrad
    if response.hovered() {
        // egui macht aus Umschalt+Rad waagerechtes Scrollen (Delta in x): für Umschalt+Rad beide Achsen nehmen.
        let dy = ui.input(|i| if i.modifiers.shift { i.raw_scroll_delta.x + i.raw_scroll_delta.y } else { i.raw_scroll_delta.y });
        if dy != 0.0 {
            session.scroll_acc += dy / cs.y;
            let lines = session.scroll_acc.trunc() as i32;
            session.scroll_acc -= lines as f32;
            if lines != 0 {
                let mode = *session.term.lock().unwrap_or_else(|e| e.into_inner()).mode();
                let mods = ui.input(|i| i.modifiers);
                let stale = session.stale_modes.load(std::sync::atomic::Ordering::Relaxed);
                match wheel_action(mode, mods.shift, alt_wheel, stale) {
                    WheelAction::Report => {
                        if let Some(pos) = response.hover_pos() {
                            let (col, row) = cell_at(session, inner, cs, pos);
                            let btn = if lines > 0 { WHEEL_UP } else { WHEEL_DOWN };
                            let seq = mouse_seq(MouseEncoding::of(mode), btn | mouse_mod_bits(mods), col, row, false);
                            if let Some(seq) = seq {
                                session.write(&seq.repeat(lines.unsigned_abs() as usize));
                            }
                        }
                    }
                    WheelAction::Arrows => {
                        let seq = wheel_arrow(lines > 0, mode.contains(TermMode::APP_CURSOR));
                        session.write(&seq.repeat(lines.unsigned_abs() as usize));
                    }
                    WheelAction::Scrollback => {
                        session.term.lock().unwrap_or_else(|e| e.into_inner()).scroll_display(Scroll::Delta(lines));
                    }
                    WheelAction::Nothing => {}
                }
            }
        }
    }
}

/// Windows: ist die Taste (virtueller Tastencode) gerade gedrückt?
#[cfg(windows)]
fn vk_down(vk: u16) -> bool {
    // SAFETY: reine Abfrage des Tastenzustands, keine Zeiger.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) < 0 }
}

/// Strg+V wurde gerade gedrückt (Flanke). egui-winit macht aus Strg+V nur dann ein `Event::Paste`, wenn die
/// Zwischenablage Text enthält, und verschluckt den Tastendruck sonst ganz – ein Bild käme also nie an.
#[cfg(windows)]
fn ctrl_v_pressed(ui: &Ui) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PREV: AtomicBool = AtomicBool::new(false);
    let down = ui.input(|i| i.focused) && vk_down(0x11 /* VK_CONTROL */) && vk_down(b'V' as u16) && !vk_down(0x12 /* VK_MENU */);
    // `swap` muss IMMER laufen: mit `down && !PREV.swap(..)` bliebe PREV nach dem ersten Druck dauerhaft `true`
    // (Kurzschluss), und nur das allererste Strg+V der Sitzung würde ein Bild einfügen.
    let prev = PREV.swap(down, Ordering::Relaxed);
    down && !prev
}

#[cfg(not(windows))]
fn ctrl_v_pressed(_ui: &Ui) -> bool {
    false
}

/// Strg+Einfg löst ebenfalls `Event::Copy` aus – dann ohne Auswahl kein ^C senden.
fn insert_held() -> bool {
    #[cfg(windows)]
    {
        vk_down(0x2D /* VK_INSERT */)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Taste des Ziffernblocks? egui meldet den Ort nicht (Ziffernblock-5 = `Key::Num5`); unter Windows zeigt der
/// Tastenzustand von VK_NUMPAD0…9 / VK_MULTIPLY / VK_ADD / VK_SUBTRACT / VK_DECIMAL / VK_DIVIDE, woher das Zeichen kam
/// (NumLock an – sonst wären es Pfeil- und Bildtasten).
#[cfg(windows)]
fn numpad_down(c: char) -> bool {
    let vk = match c {
        '0'..='9' => 0x60 + (c as u16 - '0' as u16),
        '*' => 0x6A,
        '+' => 0x6B,
        '-' => 0x6D,
        '.' | ',' => 0x6E,
        '/' => 0x6F,
        _ => return false,
    };
    vk_down(vk)
}

#[cfg(not(windows))]
fn numpad_down(_c: char) -> bool {
    false
}

/// Tasten, deren Drücken gesendet wurde (mit Ziffernblock-Merkmal), und zu welcher Sitzung: nur deren Loslassen geht
/// ans Programm (kitty-Flag 2). Sonst käme das Loslassen von Tastenkürzeln des Hubs an, deren Drücken egui schon
/// verbraucht hat. Geleert bei Fokusverlust und Sitzungswechsel.
static KEYS_DOWN: std::sync::Mutex<(usize, Vec<(Key, bool)>)> = std::sync::Mutex::new((0, Vec::new()));

fn handle_keyboard(ui: &Ui, session: &mut Session) {
    let events = ui.input(|i| i.events.clone());
    let pasted_text = events.iter().any(|e| matches!(e, Event::Paste(_)));
    if ctrl_v_pressed(ui) && !pasted_text {
        if let Some(p) = crate::clip::image_to_file() {
            let t = crate::pathfmt::for_shell(&session.shell_id, &p.to_string_lossy());
            paste(session, &format!("{t} "));
        }
    }
    let mode = session.key_mode();
    {
        let me = session as *const Session as usize;
        let mut down = KEYS_DOWN.lock().unwrap_or_else(|e| e.into_inner());
        if down.0 != me {
            *down = (me, Vec::new());
        }
    }
    // Der Broadcast bekommt jede Taste und jeden Text, auch wenn hier nichts gesendet wird: Ziele mit anderem Modus
    // kodieren selbst (z. B. Text, den kitty-Flag 8 hier mit der Taste schon gesendet hat).
    let key_input = |session: &mut Session, k: vtkeys::KeyPress| -> vtkeys::Encoded {
        let out = vtkeys::encode_key(&k, mode);
        if out.bytes.is_empty() {
            session.write_input(&[], vtkeys::TeeInput::key(&k));
        } else {
            send_input(session, &out.bytes, vtkeys::TeeInput::key(&k));
        }
        out
    };
    // Index des `Event::Text`, das zur gerade kodierten Taste gehört und mit ihr schon gesendet ist.
    let mut skip: Option<usize> = None;
    // Das vorige Ereignis war ein nicht verbrauchter Tastendruck. Text mit reinem Alt gilt nur dann: Kürzel des Hubs
    // (Alt+1…9, Alt+Umschalt+1/2, eigene Alt-Belegungen) verbraucht egui nur als Taste, ihr Text bliebe übrig.
    let mut after_key = false;
    for (i, ev) in events.iter().enumerate() {
        let was_after_key = std::mem::take(&mut after_key);
        match ev {
            Event::WindowFocused(false) => KEYS_DOWN.lock().unwrap_or_else(|e| e.into_inner()).1.clear(),
            // egui-winit wandelt Strg+C / Strg+X (und Strg+Einfg / Umschalt+Entf) in Copy/Cut um und
            // liefert dafür KEIN Key-Ereignis – ohne diese Zweige käme ^C nie im Terminal an.
            Event::Copy => {
                let sel = session.term.lock().unwrap_or_else(|e| e.into_inner()).selection_to_string();
                if let Some(t) = sel.filter(|t| !t.is_empty()) {
                    ui.ctx().copy_text(t);
                    session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = None;
                } else if !ui.input(|i| i.modifiers.shift) && !insert_held() {
                    key_input(session, vtkeys::KeyPress::new(Key::C, Modifiers::CTRL));
                }
            }
            Event::Cut => {
                let m = ui.input(|i| i.modifiers);
                if m.ctrl {
                    key_input(session, vtkeys::KeyPress::new(Key::X, Modifiers::CTRL));
                } else {
                    // Umschalt+Entf
                    key_input(session, vtkeys::KeyPress::new(Key::Delete, m));
                }
            }
            Event::Text(t) => {
                let m = ui.input(|i| i.modifiers);
                if m.alt && !m.ctrl && !was_after_key {
                    continue; // Rest eines vom Hub verbrauchten Alt-Kürzels
                }
                let numpad = t.chars().count() == 1 && t.chars().next().is_some_and(numpad_down);
                let tee = vtkeys::TeeInput::Text { text: t.clone(), mods: m, numpad };
                if skip == Some(i) {
                    session.write_input(&[], tee); // hier schon mit der Taste gesendet
                    continue;
                }
                let bytes = vtkeys::encode_text(t, m, numpad, mode);
                if bytes.is_empty() {
                    session.write_input(&[], tee);
                } else {
                    send_input(session, &bytes, tee);
                }
            }
            Event::Paste(t) => paste(session, t),
            Event::Key { key, physical_key, pressed, repeat, modifiers } => {
                let (key, modifiers) = (*key, *modifiers);
                if *pressed {
                    // Einfügen: Text kommt über Event::Paste, Bilder über ctrl_v_pressed(); das Steuerzeichen ^V
                    // selbst wird nicht gesendet (sonst fügt z. B. PSReadLine doppelt ein).
                    if key == Key::V && modifiers.ctrl && !modifiers.alt {
                        continue;
                    }
                    // Kopieren
                    if key == Key::C && modifiers.ctrl && !modifiers.alt {
                        let sel = session.term.lock().unwrap_or_else(|e| e.into_inner()).selection_to_string();
                        if let Some(t) = sel.filter(|t| !t.is_empty()) {
                            ui.ctx().copy_text(t);
                            session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = None;
                            continue;
                        } else if modifiers.shift {
                            continue;
                        }
                    }
                    after_key = true;
                }
                let text = match events.get(i + 1) {
                    Some(Event::Text(t)) => Some(t.as_str()),
                    _ => None,
                };
                let phase = match (*pressed, *repeat) {
                    (false, _) => vtkeys::Phase::Release,
                    (true, true) => vtkeys::Phase::Repeat,
                    (true, false) => vtkeys::Phase::Press,
                };
                let numpad = if phase == vtkeys::Phase::Release {
                    // Beim Loslassen ist die Taste schon oben: Merkmal vom Drücken nehmen.
                    let mut down = KEYS_DOWN.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(pos) = down.1.iter().position(|(k, _)| *k == key) else { continue };
                    down.1.remove(pos).1
                } else {
                    key_digit(key).is_some_and(numpad_down)
                };
                let kp = vtkeys::KeyPress { key, physical: *physical_key, mods: modifiers, phase, numpad, text };
                let out = key_input(session, kp);
                if out.eat_text && text.is_some() {
                    skip = Some(i + 1);
                }
                if !out.bytes.is_empty() && phase != vtkeys::Phase::Release {
                    let mut down = KEYS_DOWN.lock().unwrap_or_else(|e| e.into_inner());
                    if !down.1.iter().any(|(k, _)| *k == key) {
                        down.1.push((key, numpad));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Ziffer bzw. Rechenzeichen einer Taste, die es auch auf dem Ziffernblock gibt.
fn key_digit(key: Key) -> Option<char> {
    let n = key.name();
    match key {
        Key::Plus => Some('+'),
        Key::Minus => Some('-'),
        Key::Slash => Some('/'),
        Key::Period => Some('.'),
        Key::Comma => Some(','),
        _ if n.len() == 1 && n.as_bytes()[0].is_ascii_digit() => n.chars().next(),
        _ => None,
    }
}

/// Tastatureingabe senden; der Broadcast bekommt die Eingabe selbst und kodiert sie je Ziel (`TeeInput`).
fn send_input(session: &mut Session, bytes: &[u8], tee: vtkeys::TeeInput) {
    {
        let mut t = session.term.lock().unwrap_or_else(|e| e.into_inner());
        if t.grid().display_offset() > 0 {
            t.scroll_display(Scroll::Bottom);
        }
        t.selection = None;
    }
    session.write_input(bytes, tee);
}

/// Einfügen durch den Nutzer; mehrere Zeilen ohne Bracketed Paste ggf. erst nach Rückfrage (`Session::paste_user`).
fn paste(session: &mut Session, text: &str) {
    session.paste_user(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::AltScreenWheel;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{test::TermSize, Config, Term};
    use alacritty_terminal::vte::ansi::Processor;
    use MouseEncoding::{Normal, Sgr, Utf8};

    fn some(v: &[u8]) -> Option<Vec<u8>> {
        Some(v.to_vec())
    }

    #[test]
    fn cursor_blink_phases() {
        assert!(blink_state(0.0).0);
        assert!(!blink_state(BLINK_HALF + 0.01).0);
        assert!(blink_state(2.0 * BLINK_HALF + 0.01).0);
        let (_, next) = blink_state(0.1);
        assert!((next.unwrap() - (BLINK_HALF - 0.1)).abs() < 1e-9);
        // Nach langer Ruhe: sichtbar und kein weiteres Neuzeichnen.
        assert_eq!(blink_state(BLINK_IDLE), (true, None));
        assert_eq!(blink_state(BLINK_IDLE + 100.0), (true, None));
        assert!(blink_state(BLINK_IDLE - 0.001).1.unwrap() <= 0.01 + 1e-9);
    }

    #[test]
    fn mouse_encoding() {
        assert_eq!(mouse_seq(Sgr, 0, 4, 9, false), some(b"\x1b[<0;5;10M"));
        assert_eq!(mouse_seq(Sgr, 0, 4, 9, true), some(b"\x1b[<0;5;10m"));
        assert_eq!(mouse_seq(Normal, 0, 0, 0, false), some(&[0x1b, b'[', b'M', 32, 33, 33]));
        assert_eq!(mouse_seq(Normal, 2, 0, 0, true), some(&[0x1b, b'[', b'M', 35, 33, 33]));
        // Loslassen ohne SGR: Code 3, Modifikatoren bleiben.
        assert_eq!(mouse_seq(Normal, 16, 1, 2, true), some(&[0x1b, b'[', b'M', 32 + 19, 34, 35]));
        assert_eq!(mouse_seq(Utf8, 8 | 2, 0, 0, true), some(&[0x1b, b'[', b'M', 32 + 11, 33, 33]));
    }

    #[test]
    fn wheel_encoding_all_encodings() {
        // Rad hoch/runter = 64/65, Spalte 5 / Zeile 10 (1-basiert).
        assert_eq!(mouse_seq(Sgr, WHEEL_UP, 4, 9, false), some(b"\x1b[<64;5;10M"));
        assert_eq!(mouse_seq(Sgr, WHEEL_DOWN, 4, 9, false), some(b"\x1b[<65;5;10M"));
        assert_eq!(mouse_seq(Normal, WHEEL_UP, 4, 9, false), some(&[0x1b, b'[', b'M', 96, 37, 42]));
        assert_eq!(mouse_seq(Normal, WHEEL_DOWN, 4, 9, false), some(&[0x1b, b'[', b'M', 97, 37, 42]));
        assert_eq!(mouse_seq(Utf8, WHEEL_UP, 4, 9, false), some(&[0x1b, b'[', b'M', 96, 37, 42]));
        assert_eq!(mouse_seq(Utf8, WHEEL_DOWN, 4, 9, false), some(&[0x1b, b'[', b'M', 97, 37, 42]));
        // Modifikatoren: Alt +8, Strg +16; Umschalt wird nie gemeldet.
        let m = |alt, ctrl, shift| Modifiers { alt, ctrl, shift, ..Default::default() };
        assert_eq!(mouse_mod_bits(m(true, false, false)), 8);
        assert_eq!(mouse_mod_bits(m(false, true, false)), 16);
        assert_eq!(mouse_mod_bits(m(true, true, true)), 24);
        assert_eq!(mouse_seq(Sgr, WHEEL_UP | 16, 0, 0, false), some(b"\x1b[<80;1;1M"));
        assert_eq!(mouse_seq(Sgr, WHEEL_DOWN | 24, 0, 0, false), some(b"\x1b[<89;1;1M"));
        assert_eq!(mouse_seq(Normal, WHEEL_DOWN | 8, 0, 0, false), some(&[0x1b, b'[', b'M', 32 + 73, 33, 33]));
    }

    #[test]
    fn normal_encoding_limits() {
        // Spalte/Zeile 94 (0-basiert) ist die letzte darstellbare: 32 + 1 + 94 = 127 (7 Bit, übersteht ConPTY).
        assert_eq!(NORMAL_MOUSE_MAX, 94);
        assert_eq!(mouse_seq(Normal, WHEEL_UP, 94, 94, false), some(&[0x1b, b'[', b'M', 96, 127, 127]));
        // Ab 95 nichts senden (Bytes ab 0x80 würden in ConPTY zu U+FFFD), auch nicht an der alten 8-Bit-Grenze 223.
        assert_eq!(mouse_seq(Normal, WHEEL_UP, 95, 0, false), None);
        assert_eq!(mouse_seq(Normal, 0, 0, 95, false), None);
        assert_eq!(mouse_seq(Normal, WHEEL_DOWN, 222, 0, false), None);
        assert_eq!(mouse_seq(Normal, 0, 223, 223, false), None);
        assert_eq!(mouse_seq(Normal, 0, 500, 0, true), None);
        // Kein gesendetes Byte der Standard-Kodierung hat das achte Bit.
        for col in 0..=NORMAL_MOUSE_MAX {
            assert!(mouse_seq(Normal, WHEEL_DOWN | 24, col, col, false).unwrap().iter().all(|b| *b < 0x80));
        }
        // UTF-8: ab Wert 128 zwei Bytes (U+0080 = C2 80), bis U+07FF.
        assert_eq!(mouse_seq(Utf8, WHEEL_UP, 94, 0, false), some(&[0x1b, b'[', b'M', 96, 127, 33]));
        assert_eq!(mouse_seq(Utf8, WHEEL_UP, 95, 0, false), some(&[0x1b, b'[', b'M', 96, 0xC2, 0x80, 33]));
        assert_eq!(mouse_seq(Utf8, WHEEL_DOWN, 300, 223, false), {
            let mut v = vec![0x1b, b'[', b'M', 97];
            v.extend("\u{14D}\u{100}".as_bytes()); // 333 = U+014D, 256 = U+0100
            Some(v)
        });
        assert!(mouse_seq(Utf8, 0, UTF8_MOUSE_MAX, 0, false).is_some_and(|s| s.ends_with(&[0xDF, 0xBF, 33])));
        assert_eq!(mouse_seq(Utf8, 0, UTF8_MOUSE_MAX + 1, 0, false), None);
        // SGR hat keine Grenze.
        assert_eq!(mouse_seq(Sgr, WHEEL_UP, 999, 2999, false), some(b"\x1b[<64;1000;3000M"));
    }

    /// Modus nach den Bytes, die ein Programm schreibt, aus der echten Emulation.
    fn mode_after(bytes: &str) -> TermMode {
        let mut term = Term::new(Config::default(), &TermSize::new(80, 24), VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut term, bytes.as_bytes());
        *term.mode()
    }

    #[test]
    fn encoding_follows_program_request() {
        assert_eq!(MouseEncoding::of(mode_after("\x1b[?1000h")), Normal);
        assert_eq!(MouseEncoding::of(mode_after("\x1b[?1002h\x1b[?1006h")), Sgr);
        assert_eq!(MouseEncoding::of(mode_after("\x1b[?1003h\x1b[?1005h")), Utf8);
        // Die letzte Anforderung gewinnt (1005 und 1006 schließen sich aus).
        assert_eq!(MouseEncoding::of(mode_after("\x1b[?1006h\x1b[?1005h")), Utf8);
        assert_eq!(MouseEncoding::of(mode_after("\x1b[?1005h\x1b[?1006h")), Sgr);
        assert_eq!(MouseEncoding::of(mode_after("\x1b[?1006h\x1b[?1006l")), Normal);
    }

    #[test]
    fn wheel_decision() {
        use WheelAction::*;
        let arrows = AltScreenWheel::Arrows;
        let off = AltScreenWheel::Off;
        let main = mode_after("");
        let alt = mode_after("\x1b[?1049h");
        let alt_no_1007 = mode_after("\x1b[?1049h\x1b[?1007l");
        assert!(alt.contains(TermMode::ALTERNATE_SCROLL), "1007 ist in alacritty_terminal standardmäßig an");

        // Normale Shell: Verlauf scrollen, Umschalt auch.
        assert_eq!(wheel_action(main, false, arrows, false), Scrollback);
        assert_eq!(wheel_action(main, true, arrows, false), Scrollback);
        // Vollbildprogramm ohne Maus (less, man): Pfeiltasten nach Einstellung und 1007.
        assert_eq!(wheel_action(alt, false, arrows, false), Arrows);
        assert_eq!(wheel_action(alt, false, off, false), Nothing);
        assert_eq!(wheel_action(alt_no_1007, false, arrows, false), Nothing);
        // Im alternativen Bildschirm gibt es keinen Verlauf: Umschalt+Rad tut nichts.
        assert_eq!(wheel_action(alt, true, arrows, false), Nothing);

        // Maus-Modus in jeder Variante und Kodierung: Rad wird gemeldet – auch im Hauptbildschirm.
        for req in ["\x1b[?1000h", "\x1b[?1002h", "\x1b[?1003h", "\x1b[?1000h\x1b[?1006h", "\x1b[?1002h\x1b[?1005h"] {
            let m = mode_after(req);
            assert_eq!(wheel_action(m, false, arrows, false), Report, "{req:?}");
            assert_eq!(wheel_action(m, false, off, false), Report, "{req:?}");
            let a = mode_after(&format!("\x1b[?1049h{req}"));
            assert_eq!(wheel_action(a, false, arrows, false), Report, "alt {req:?}");
            assert_eq!(wheel_action(a, false, off, false), Report, "alt {req:?}");
            // Umschalt+Rad: Hauptbildschirm = Verlauf, alternativer Bildschirm = nichts.
            assert_eq!(wheel_action(m, true, arrows, false), Scrollback, "{req:?}");
            assert_eq!(wheel_action(a, true, arrows, false), Nothing, "alt {req:?}");
        }
        // Programm hat Maus-Modus wieder ausgeschaltet.
        assert_eq!(wheel_action(mode_after("\x1b[?1000h\x1b[?1000l"), false, arrows, false), Scrollback);

        // Hängengebliebene Modi (Prompt kam trotz Vollbild/Maus-Modus): wie eine normale Shell.
        assert_eq!(wheel_action(alt, false, arrows, true), Nothing);
        assert_eq!(wheel_action(mode_after("\x1b[?1049h\x1b[?1000h"), false, arrows, true), Nothing);
        assert_eq!(wheel_action(mode_after("\x1b[?1000h\x1b[?1006h"), false, arrows, true), Scrollback);
        // Nach dem ordentlichen Verlassen (1049l) ist alles wieder normal.
        assert_eq!(wheel_action(mode_after("\x1b[?1049h\x1b[?1049l"), false, arrows, false), Scrollback);
    }

    #[test]
    fn stale_mode_detection() {
        use crate::terminal::prompt_with_stale_modes;
        assert!(!prompt_with_stale_modes(mode_after("")));
        assert!(!prompt_with_stale_modes(mode_after("\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?1006l\x1b[?1000l\x1b[?1049l")));
        // Abgestürztes Vollbildprogramm: alternativer Bildschirm oder Maus-Modus blieb an.
        assert!(prompt_with_stale_modes(mode_after("\x1b[?1049h")));
        assert!(prompt_with_stale_modes(mode_after("\x1b[?1002h\x1b[?1006h")));
        // `reset` (RIS) räumt beides auf.
        assert!(!prompt_with_stale_modes(mode_after("\x1b[?1049h\x1b[?1003h\x1bc")));
    }

    /// kitty-Tastaturflags, die ein abgestürztes Programm stehen ließ, setzt der Lese-Thread am Prompt zurück (#183).
    #[test]
    fn kitty_flags_reset_at_prompt() {
        use crate::osc::OscScanner;
        use crate::terminal::{kitty_left_at_prompt, term_config};
        let run = |chunks: &[&str]| -> TermMode {
            let mut term = Term::new(term_config(100), &TermSize::new(80, 24), VoidListener);
            let mut p: Processor = Processor::new();
            let mut scanner = OscScanner::default();
            for chunk in chunks {
                let bytes = chunk.as_bytes();
                let (events, positions) = scanner.feed_at(bytes);
                let mut done = 0;
                for (ev, end) in events.into_iter().zip(positions) {
                    p.advance(&mut term, &bytes[done..end]);
                    done = end;
                    if kitty_left_at_prompt(&ev, *term.mode()) {
                        p.advance(&mut term, crate::terminal::KITTY_RESET);
                    }
                }
                p.advance(&mut term, &bytes[done..]);
            }
            *term.mode()
        };
        const PROMPT: &str = "\x1b]133;A\x07PS> \x1b]133;B\x07";
        // Programm schaltet kitty ein (wie Claude Code: Flags 5) – solange es läuft, bleiben sie.
        let m = run(&[PROMPT, "\x1b[>5u"]);
        assert_eq!(crate::vtkeys::KeyMode::new(m, true).kitty, 5);
        // ordentlich beendet (Pop) oder abgestürzt: am nächsten Prompt aus
        assert!(!run(&[PROMPT, "\x1b[>5u", "\x1b[<u", PROMPT]).intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
        assert!(!run(&[PROMPT, "\x1b[>5u", PROMPT]).intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
        assert!(!run(&[PROMPT, &format!("\x1b[>1u\x1b[>5uboom{PROMPT}")]).intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
        // Text nach dem Prompt im selben Chunk, der kitty wieder einschaltet (Shell mit kitty-Unterstützung): bleibt.
        assert!(run(&[&format!("{PROMPT}\x1b[>1u")]).contains(TermMode::DISAMBIGUATE_ESC_CODES));
    }

    /// Ausgabe-Chunks wie im Lese-Thread (`Session::new`) verarbeiten: an den Sequenzen teilen, nach jeder
    /// `next_stale_modes` mit dem Modus an dieser Stelle; `enter` vor einem Chunk = abgeschickte Befehlszeile.
    fn stale_after(chunks: &[(bool, &str)]) -> bool {
        use crate::osc::OscScanner;
        use crate::terminal::next_stale_modes;
        let mut term = Term::new(Config::default(), &TermSize::new(80, 24), VoidListener);
        let mut p: Processor = Processor::new();
        let mut scanner = OscScanner::default();
        let mut stale = false;
        for (enter, chunk) in chunks {
            if *enter {
                stale = false; // wie `Session::write_raw` mit `\r`
            }
            let bytes = chunk.as_bytes();
            let (events, positions) = scanner.feed_at(bytes);
            let mut done = 0;
            for (ev, end) in events.into_iter().zip(positions) {
                p.advance(&mut term, &bytes[done..end]);
                done = end;
                stale = next_stale_modes(stale, &ev, *term.mode());
            }
            p.advance(&mut term, &bytes[done..]);
        }
        stale
    }

    #[test]
    fn stale_flag_lifecycle() {
        const PROMPT: &str = "\x1b]133;A\x07$ \x1b]133;B\x07";
        // Ordentlich beendetes Vollbildprogramm: nie gesetzt.
        assert!(!stale_after(&[(false, PROMPT), (true, "\x1b[?1049h\x1b[?1000hvim"), (false, "\x1b[?1000l\x1b[?1049l"), (false, PROMPT)]));
        // Absturz im alternativen Bildschirm, danach Prompt: gesetzt.
        assert!(stale_after(&[(false, PROMPT), (true, "\x1b[?1049h\x1b[?1006h\x1b[?1002h"), (false, PROMPT)]));
        // ... in einem Chunk mit dem Prompt.
        assert!(stale_after(&[(true, &format!("\x1b[?1049hboom{PROMPT}"))]));
        // Enter löscht, der nächste Prompt im hängenden Bildschirm setzt wieder.
        assert!(!stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (true, "dir\r\n")]));
        assert!(stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (true, "dir\r\n"), (false, PROMPT)]));
        // Vollbildprogramm startet ohne Enter (Tastenkürzel, mc nach Strg+O): 1049h im schon aktiven alternativen
        // Bildschirm ändert keine Modus-Bits, löscht aber den Zustand.
        assert!(!stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (false, "\x1b[?1049h\x1b[?1000h")]));
        assert!(!stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (false, "\x1b[?47h")]));
        assert!(!stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (false, "\x1b[?1003h")]));
        // Wettlauf: der Prompt kommt erst nach dem Enter, das nächste Programm schaltet aber Vollbild ein.
        assert!(!stale_after(&[(false, "\x1b[?1049h"), (true, PROMPT), (false, "\x1b[?1049h")]));
        // Sequenz über eine Chunk-Grenze.
        assert!(!stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (false, "\x1b[?10"), (false, "49h")]));
        // Andere Sequenzen (Farben, Cursor, Abschalten) lassen den Zustand stehen.
        assert!(stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (false, "\x1b[31mx\x1b[?25h\x1b[?1000l\x1b[?2004h")]));
        // Prompt im Hauptbildschirm ohne Maus-Modus: gelöscht.
        assert!(!stale_after(&[(false, "\x1b[?1049h"), (false, PROMPT), (false, "\x1b[?1049l"), (false, PROMPT)]));
    }

    #[test]
    fn wheel_arrows_follow_cursor_mode() {
        assert_eq!(wheel_arrow(true, false), b"\x1b[A");
        assert_eq!(wheel_arrow(false, false), b"\x1b[B");
        assert_eq!(wheel_arrow(true, true), b"\x1bOA");
        assert_eq!(wheel_arrow(false, true), b"\x1bOB");
        assert!(mode_after("\x1b[?1h").contains(TermMode::APP_CURSOR));
    }
}

fn rel_lum(c: Color32) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (rel_lum(a), rel_lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Mindestkontrast für Terminaltext (z. B. dunkelblau auf schwarz): Vordergrund wird bei Bedarf aufgehellt/abgedunkelt.
fn ensure_contrast(fg: Color32, bg: Color32) -> Color32 {
    const MIN: f32 = 2.6;
    if bg.a() == 0 || contrast(fg, bg) >= MIN {
        return fg;
    }
    let target = if rel_lum(bg) < 0.4 { Color32::WHITE } else { Color32::BLACK };
    let mut out = fg;
    for step in 1..=10 {
        let t = step as f32 / 10.0;
        out = Color32::from_rgb(
            (fg.r() as f32 + (target.r() as f32 - fg.r() as f32) * t) as u8,
            (fg.g() as f32 + (target.g() as f32 - fg.g() as f32) * t) as u8,
            (fg.b() as f32 + (target.b() as f32 - fg.b() as f32) * t) as u8,
        );
        if contrast(out, bg) >= MIN {
            break;
        }
    }
    out
}
