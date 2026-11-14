//! Zeichnen eines Terminals und Verarbeitung von Tastatur/Maus.

use crate::terminal::Session;
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
    session.resize(cols, rows);

    if focused {
        handle_keyboard(ui, session);
    }
    let opened_link = handle_links(ui, &response, session, inner, cs, cwd);
    if !opened_link {
        handle_mouse(ui, &response, session, inner, cs);
    }
    paint(ui, rect, inner, session, theme, font, cs, focused);
    response
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
) {
    let ppp = ui.ctx().pixels_per_point();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme.bg);

    let term = session.term.lock().unwrap_or_else(|e| e.into_inner());
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

        let mut fg = theme.resolve(cell.fg, bold, dim);
        let mut bg = theme.resolve(cell.bg, false, false);
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
            let mut drawn = crate::boxdraw::draw(&painter, cell_rect, ch, fg, ppp);
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
                if wide {
                    // Breite Zeichen (CJK u. a.) in den zwei Zellen zentrieren.
                    let g = painter.layout_no_wrap(s, font.clone(), fg);
                    let x = cell_rect.min.x + ((width - g.size().x) / 2.0).max(0.0);
                    painter.galley(Pos2::new(x, cell_rect.min.y), g, fg);
                } else {
                    painter.text(cell_rect.min, Align2::LEFT_TOP, s, font.clone(), fg);
                }
            }
        }
        if flags.intersects(Flags::ALL_UNDERLINES) {
            let yy = cell_rect.max.y - 1.5;
            painter.line_segment(
                [Pos2::new(cell_rect.min.x, yy), Pos2::new(cell_rect.max.x, yy)],
                Stroke::new(1.0_f32, fg),
            );
        }
        if flags.contains(Flags::STRIKEOUT) {
            let yy = cell_rect.center().y;
            painter.line_segment(
                [Pos2::new(cell_rect.min.x, yy), Pos2::new(cell_rect.max.x, yy)],
                Stroke::new(1.0_f32, fg),
            );
        }
    }

    // Cursor
    if cursor.shape != CursorShape::Hidden {
        let row = cursor.point.line.0 + offset;
        if row >= 0 && row < rows {
            let x = inner.min.x + cursor.point.column.0 as f32 * cs.x;
            let y = inner.min.y + row as f32 * cs.y;
            let cr = Rect::from_min_size(Pos2::new(x, y), cs);
            if !focused {
                painter.rect_stroke(cr, 0.0, Stroke::new(1.0_f32, theme.cursor), StrokeKind::Inside);
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

/// Maus-Ereignis für Programme mit Maus-Modus kodieren (SGR oder klassisches X10).
pub fn mouse_seq(sgr: bool, cb: u8, col: usize, row: usize, release: bool) -> Vec<u8> {
    if sgr {
        format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, if release { 'm' } else { 'M' }).into_bytes()
    } else {
        let b = if release { 3 } else { cb };
        let enc = |v: usize| (32 + v + 1).min(255) as u8;
        vec![0x1b, b'[', b'M', 32 + b, enc(col), enc(row)]
    }
}

fn forward_mouse(ui: &Ui, resp: &Response, s: &mut Session, inner: Rect, cs: Vec2, mode: TermMode) {
    let mods = ui.input(|i| i.modifiers);
    let mbits = (mods.alt as u8) * 8 + (mods.ctrl as u8) * 16;
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()).or(resp.interact_pointer_pos()) else { return };
    if !resp.hovered() && s.mouse_down.is_none() {
        return;
    }
    let col = ((pos.x - inner.min.x) / cs.x).floor().clamp(0.0, s.cols as f32 - 1.0) as usize;
    let row = ((pos.y - inner.min.y) / cs.y).floor().clamp(0.0, s.rows as f32 - 1.0) as usize;
    for (b, btn) in [(0u8, egui::PointerButton::Primary), (1, egui::PointerButton::Middle), (2, egui::PointerButton::Secondary)] {
        if resp.hovered() && ui.input(|i| i.pointer.button_pressed(btn)) {
            s.write(&mouse_seq(sgr, b | mbits, col, row, false));
            s.mouse_down = Some(b);
            s.mouse_last = Some((col, row));
        }
        if s.mouse_down == Some(b) && ui.input(|i| i.pointer.button_released(btn)) {
            s.write(&mouse_seq(sgr, b | mbits, col, row, true));
            s.mouse_down = None;
        }
    }
    if s.mouse_last != Some((col, row)) {
        if let Some(b) = s.mouse_down {
            if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                s.write(&mouse_seq(sgr, (b + 32) | mbits, col, row, false));
            }
        } else if mode.contains(TermMode::MOUSE_MOTION) && resp.hovered() {
            s.write(&mouse_seq(sgr, 35 | mbits, col, row, false));
        }
        s.mouse_last = Some((col, row));
    }
}

/// Strg+Klick auf Links/Pfade. Gibt `true` zurück, wenn geöffnet wurde (dann keine weitere Mausbehandlung).
fn handle_links(ui: &Ui, response: &Response, session: &mut Session, inner: Rect, cs: Vec2, cwd: &str) -> bool {
    session.hover_link = None;
    let ctrl = ui.input(|i| i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt);
    if !ctrl || !response.hovered() {
        return false;
    }
    let Some(pos) = response.hover_pos() else { return false };
    let col = ((pos.x - inner.min.x) / cs.x).floor();
    let row = ((pos.y - inner.min.y) / cs.y).floor();
    if col < 0.0 || row < 0.0 || row as u16 >= session.rows {
        return false;
    }
    let chars = session.row_chars(row as usize);
    let Some(link) = crate::links::find(&chars, col as usize, cwd, &session.shell_id) else { return false };
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    session.hover_link = Some((row as usize, link.start, link.end, link.target.clone()));
    if response.clicked_by(egui::PointerButton::Primary) {
        crate::links::open(&link.target);
        return true;
    }
    false
}

fn handle_mouse(ui: &Ui, response: &Response, session: &mut Session, inner: Rect, cs: Vec2) {
    let mode = *session.term.lock().unwrap_or_else(|e| e.into_inner()).mode();
    let forwarding = mode.intersects(TermMode::MOUSE_MODE) && !ui.input(|i| i.modifiers.shift);
    if forwarding {
        forward_mouse(ui, response, session, inner, cs, mode);
    } else {
        session.mouse_down = None;
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
        let dy = ui.input(|i| i.raw_scroll_delta.y);
        if dy != 0.0 {
            session.scroll_acc += dy / cs.y;
            let lines = session.scroll_acc.trunc() as i32;
            session.scroll_acc -= lines as f32;
            if lines != 0 {
                let mode = *session.term.lock().unwrap_or_else(|e| e.into_inner()).mode();
                if mode.contains(TermMode::MOUSE_MODE) && mode.contains(TermMode::SGR_MOUSE) {
                    if let Some(pos) = response.hover_pos() {
                        let (p, _) = point_at(session, inner, cs, pos);
                        let btn = if lines > 0 { 64 } else { 65 };
                        for _ in 0..lines.abs() {
                            let seq = format!("\x1b[<{};{};{}M", btn, p.column.0 + 1, p.line.0.max(0) + 1);
                            session.write(seq.as_bytes());
                        }
                    }
                } else if mode.contains(TermMode::ALT_SCREEN) {
                    let app = mode.contains(TermMode::APP_CURSOR);
                    let seq = match (lines > 0, app) {
                        (true, true) => "\x1bOA",
                        (true, false) => "\x1b[A",
                        (false, true) => "\x1bOB",
                        (false, false) => "\x1b[B",
                    };
                    for _ in 0..lines.abs() {
                        session.write(seq.as_bytes());
                    }
                } else {
                    session.term.lock().unwrap_or_else(|e| e.into_inner()).scroll_display(Scroll::Delta(lines));
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

fn handle_keyboard(ui: &Ui, session: &mut Session) {
    let events = ui.input(|i| i.events.clone());
    let pasted_text = events.iter().any(|e| matches!(e, Event::Paste(_)));
    if ctrl_v_pressed(ui) && !pasted_text {
        if let Some(p) = crate::clip::image_to_file() {
            let t = crate::pathfmt::for_shell(&session.shell_id, &p.to_string_lossy());
            paste(session, &format!("{t} "));
        }
    }
    for ev in events {
        match ev {
            // egui-winit wandelt Strg+C / Strg+X (und Strg+Einfg / Umschalt+Entf) in Copy/Cut um und
            // liefert dafür KEIN Key-Ereignis – ohne diese Zweige käme ^C nie im Terminal an.
            Event::Copy => {
                let sel = session.term.lock().unwrap_or_else(|e| e.into_inner()).selection_to_string();
                if let Some(t) = sel.filter(|t| !t.is_empty()) {
                    ui.ctx().copy_text(t);
                    session.term.lock().unwrap_or_else(|e| e.into_inner()).selection = None;
                } else if !ui.input(|i| i.modifiers.shift) && !insert_held() {
                    send(session, b"\x03");
                }
            }
            Event::Cut => {
                let m = ui.input(|i| i.modifiers);
                if m.ctrl {
                    send(session, b"\x18");
                } else {
                    // Umschalt+Entf
                    let mode = *session.term.lock().unwrap_or_else(|e| e.into_inner()).mode();
                    if let Some(bytes) = encode_key(Key::Delete, m, mode.contains(TermMode::APP_CURSOR)) {
                        send(session, &bytes);
                    }
                }
            }
            Event::Text(t) => {
                let m = ui.input(|i| i.modifiers);
                let altgr = m.ctrl && m.alt;
                let pure_ctrl = m.ctrl && !m.alt;
                let pure_alt = m.alt && !m.ctrl;
                if !(pure_ctrl || pure_alt) || altgr {
                    if !t.chars().all(|c| c.is_control()) {
                        send(session, t.as_bytes());
                    }
                }
            }
            Event::Paste(t) => paste(session, &t),
            Event::Key { key, pressed: true, modifiers, .. } => {
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
                let mode = *session.term.lock().unwrap_or_else(|e| e.into_inner()).mode();
                if let Some(bytes) = encode_key(key, modifiers, mode.contains(TermMode::APP_CURSOR)) {
                    send(session, &bytes);
                }
            }
            _ => {}
        }
    }
}

fn send(session: &mut Session, bytes: &[u8]) {
    {
        let mut t = session.term.lock().unwrap_or_else(|e| e.into_inner());
        if t.grid().display_offset() > 0 {
            t.scroll_display(Scroll::Bottom);
        }
        t.selection = None;
    }
    session.write(bytes);
}

fn paste(session: &mut Session, text: &str) {
    {
        let mut t = session.term.lock().unwrap_or_else(|e| e.into_inner());
        if t.grid().display_offset() > 0 {
            t.scroll_display(Scroll::Bottom);
        }
        t.selection = None;
    }
    session.send_text(text, false);
}

fn mod_param(m: Modifiers) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn encode_key(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let altgr = m.ctrl && m.alt;
    let any_mod = m.shift || m.alt || m.ctrl;
    let csi = |final_byte: char| -> Vec<u8> {
        if any_mod {
            format!("\x1b[1;{}{}", mod_param(m), final_byte).into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if any_mod {
            format!("\x1b[{};{}~", n, mod_param(m)).into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let b = |s: &str| Some(s.as_bytes().to_vec());
    match key {
        Key::Enter => {
            if m.shift || m.alt || m.ctrl { b("\x1b\r") } else { b("\r") }
        }
        Key::Backspace => {
            if m.ctrl { b("\x17") } else if m.alt { b("\x1b\x7f") } else { b("\x7f") }
        }
        Key::Tab => if m.shift { b("\x1b[Z") } else { b("\t") },
        Key::Escape => b("\x1b"),
        Key::ArrowUp => Some(csi('A')),
        Key::ArrowDown => Some(csi('B')),
        Key::ArrowRight => Some(csi('C')),
        Key::ArrowLeft => Some(csi('D')),
        Key::Home => Some(csi('H')),
        Key::End => Some(csi('F')),
        Key::Insert => Some(tilde(2)),
        Key::Delete => Some(tilde(3)),
        Key::PageUp => Some(tilde(5)),
        Key::PageDown => Some(tilde(6)),
        Key::F1 => Some(if any_mod { format!("\x1b[1;{}P", mod_param(m)).into_bytes() } else { b"\x1bOP".to_vec() }),
        Key::F2 => Some(if any_mod { format!("\x1b[1;{}Q", mod_param(m)).into_bytes() } else { b"\x1bOQ".to_vec() }),
        Key::F3 => Some(if any_mod { format!("\x1b[1;{}R", mod_param(m)).into_bytes() } else { b"\x1bOR".to_vec() }),
        Key::F4 => Some(if any_mod { format!("\x1b[1;{}S", mod_param(m)).into_bytes() } else { b"\x1bOS".to_vec() }),
        Key::F5 => Some(tilde(15)),
        Key::F6 => Some(tilde(17)),
        Key::F7 => Some(tilde(18)),
        Key::F8 => Some(tilde(19)),
        Key::F9 => Some(tilde(20)),
        Key::F10 => Some(tilde(21)),
        Key::F11 => Some(tilde(23)),
        Key::F12 => Some(tilde(24)),
        Key::Space if m.ctrl && !m.alt => Some(vec![0]),
        Key::OpenBracket if m.ctrl && !m.alt => Some(vec![0x1b]),
        Key::Backslash if m.ctrl && !m.alt => Some(vec![0x1c]),
        Key::CloseBracket if m.ctrl && !m.alt => Some(vec![0x1d]),
        Key::Slash if m.ctrl && !m.alt => Some(vec![0x1f]),
        _ => {
            // Buchstaben: Ctrl+X / Alt+X
            let name = key.name();
            if name.len() == 1 && name.as_bytes()[0].is_ascii_uppercase() && !altgr {
                let ch = name.as_bytes()[0];
                if m.ctrl {
                    return Some(vec![ch - b'A' + 1]);
                }
                if m.alt {
                    let c = if m.shift { ch } else { ch.to_ascii_lowercase() };
                    return Some(vec![0x1b, c]);
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_encoding() {
        assert_eq!(mouse_seq(true, 0, 4, 9, false), b"\x1b[<0;5;10M".to_vec());
        assert_eq!(mouse_seq(true, 0, 4, 9, true), b"\x1b[<0;5;10m".to_vec());
        assert_eq!(mouse_seq(false, 0, 0, 0, false), vec![0x1b, b'[', b'M', 32, 33, 33]);
        assert_eq!(mouse_seq(false, 2, 0, 0, true), vec![0x1b, b'[', b'M', 35, 33, 33]);
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
