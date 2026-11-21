//! Eigene Fensterleiste (ohne Windows-Titelleiste): Reiter für die Bereiche der Anwendung, Fenstertitel,
//! Minimieren/Maximieren/Schließen, Ziehen, Doppelklick = maximieren; dazu die Resize-Ränder des rahmenlosen Fensters.

use super::*;
use crate::style::Icon as SIcon;
use egui::{Align2, CursorIcon, Id, PointerButton, ResizeDirection, Sense, Stroke, ViewportCommand};

/// Hauptbereiche der Anwendung (Reiter in der Fensterleiste).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Mode {
    #[default]
    Terminals,
    Tasks,
    /// Texteditor (#54): eigener Bereich mit Ordnerbaum, Reitern und Teilungen, unabhängig von den Terminals.
    Editor,
}

pub(super) const BAR_HEIGHT: f32 = 34.0;

const EDGE: f32 = 6.0;

impl App {
    /// Fensterleiste zeichnen; setzt `self.mode` bei Klick auf einen Reiter.
    pub(super) fn titlebar(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let theme = self.theme();
        let (maximized, focused) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().focused.unwrap_or(true)));
        let c = theme.bg;
        let fill = Color32::from_rgba_unmultiplied((c.r() as f32 * 0.78) as u8, (c.g() as f32 * 0.78) as u8, (c.b() as f32 * 0.78) as u8, c.a());
        // Kein Einrücken beim Maximieren: winit begrenzt den Client-Bereich eines maximierten rahmenlosen Fensters schon selbst
        // auf den Arbeitsbereich des Monitors (WM_NCCALCSIZE → rcWork). Ein zusätzlicher Rand wäre durchsichtig (#134).
        let title = if self.last_title.is_empty() { "Paneldeck".to_string() } else { self.last_title.clone() };
        egui::TopBottomPanel::top("titlebar")
            .exact_height(BAR_HEIGHT)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(fill))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let dim = style::dim(ui);
                let fg = if focused { ui.visuals().text_color() } else { dim };
                // Ziehfläche zuerst (liegt unter den Knöpfen)
                let drag = ui.interact(rect, Id::new("titlebar_drag"), Sense::click_and_drag());
                if drag.drag_started_by(PointerButton::Primary) {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if drag.double_clicked_by(PointerButton::Primary) {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
                let painter = ui.painter();
                // Programmsymbol
                let icon_c = Pos2::new(rect.left() + 20.0, rect.center().y);
                painter.rect_filled(Rect::from_center_size(icon_c, Vec2::splat(18.0)), 5.0, style::brand());
                style::paint_icon(painter, SIcon::Terminal, icon_c, Color32::WHITE);
                // Reiter
                let tabs: [(Mode, SIcon, &str); 3] = [
                    (Mode::Terminals, SIcon::Terminal, lang.t("Terminal Hub", "Terminal Hub")),
                    (Mode::Tasks, SIcon::Checklist, lang.t("Todo-Liste", "Todo list")),
                    (Mode::Editor, SIcon::Doc, lang.t("Editor", "Editor")),
                ];
                let ctrl_hint = lang.t("Strg+Klick: in neuem Fenster öffnen", "Ctrl+click: open in a new window");
                let mut x = rect.left() + 42.0;
                for (mode, icon, tip) in tabs {
                    let r = Rect::from_min_size(Pos2::new(x, rect.top() + 4.0), Vec2::new(40.0, BAR_HEIGHT - 8.0));
                    let resp = ui.interact(r, Id::new(("titlebar_tab", mode as u8)), Sense::click()).on_hover_text(format!("{tip}\n{ctrl_hint}"));
                    if mode == Mode::Tasks {
                        super::tutorial::spot_rect(ui, Spot::TitlebarTodo, r);
                    }
                    let active = self.mode == mode;
                    if active {
                        painter.rect_filled(r, 6.0, style::accent().gamma_multiply(0.22));
                        painter.line_segment([Pos2::new(r.left() + 8.0, r.bottom() + 3.0), Pos2::new(r.right() - 8.0, r.bottom() + 3.0)], Stroke::new(2.0_f32, style::accent()));
                    } else if resp.hovered() {
                        painter.rect_filled(r, 6.0, Color32::from_white_alpha(14));
                    }
                    style::paint_icon(painter, icon, r.center(), if active { style::accent() } else { fg });
                    if resp.clicked() {
                        // Strg+Klick: Reiter in einem neuen Fenster öffnen (#110), sonst nur umschalten.
                        if super::multiwin::opens_window(ui.input(|i| i.modifiers)) {
                            self.request_window(ctx, mode);
                        } else {
                            self.set_mode(mode);
                        }
                    }
                    x += 44.0;
                }
                let tabs_end = x;
                super::tutorial::spot_rect(ui, Spot::TitlebarTabs, Rect::from_min_max(Pos2::new(rect.left() + 42.0, rect.top() + 4.0), Pos2::new(tabs_end - 4.0, rect.bottom() - 4.0)));
                // Fensterknöpfe rechts; links davon (von rechts nach links) Einstellungen, Update und MCP-Status (#240) –
                // in jedem Bereich und jedem Fenster. Der MCP-Status entfällt, wenn sonst kein Platz für die Reiter bliebe.
                let bw = 46.0;
                let mut bx = rect.right() - bw * 3.0;
                let mut right = bx - 4.0;
                right = self.settings_title_button(ui, right);
                right = self.update_title_button(ui, right); // update/ui.rs
                right = self.mcp_title_button(ui, right, tabs_end + 40.0); // mcp.rs
                // Fenstertitel: mittig, wenn er dort zwischen Reitern und Knöpfen Platz hat, sonst in der freien Mitte;
                // gekürzt auf den freien Platz (#259), nie über Reiter oder Knöpfe.
                let font = FontId::proportional(12.5);
                let col = if focused { dim } else { dim.gamma_multiply(0.7) };
                if let Some((text, cx)) = title_place(&title, tabs_end + 8.0, right - 8.0, rect.center().x, |s| painter.layout_no_wrap(s.to_string(), font.clone(), col).size().x) {
                    painter.text(Pos2::new(cx, rect.center().y), Align2::CENTER_CENTER, text, font, col);
                }
                for (i, icon) in [SIcon::Minimize, if maximized { SIcon::Restore } else { SIcon::Maximize }, SIcon::Close].into_iter().enumerate() {
                    let r = Rect::from_min_size(Pos2::new(bx, rect.top()), Vec2::new(bw, BAR_HEIGHT));
                    // Hand wie bei allen Knöpfen (#187); über dem Maximieren-Knopf entscheidet im Hauptfenster das System (Snap-Layouts).
                    let resp = ui.interact(r, Id::new(("titlebar_btn", i)), Sense::click());
                    let close = i == 2;
                    // Maximieren-Knopf: Rechteck ans System melden (Snap-Layouts); dort kommt der Hover nicht mehr von egui.
                    // Nur im Hauptfenster: weitere Fenster haben keine eigene Fensterprozedur (dort ohne Snap-Layouts).
                    let main = self.wins.cur == 0;
                    let snap_hover = main && i == 1 && crate::platform::chrome::max_hover();
                    if main && i == 1 {
                        crate::platform::chrome::report_max_button(r, ctx.pixels_per_point(), if maximized { 0.0 } else { EDGE });
                    }
                    if resp.hovered() || snap_hover {
                        painter.rect_filled(r, 0.0, if close { Color32::from_rgb(0xc4, 0x2b, 0x1c) } else { Color32::from_white_alpha(20) });
                    }
                    style::paint_icon(painter, icon, r.center(), if close && resp.hovered() { Color32::WHITE } else { fg });
                    if resp.clicked() {
                        ctx.send_viewport_cmd(match i {
                            0 => ViewportCommand::Minimized(true),
                            1 => ViewportCommand::Maximized(!maximized),
                            _ => ViewportCommand::Close,
                        });
                    }
                    bx += bw;
                }
                painter.line_segment([Pos2::new(rect.left(), rect.bottom() - 0.5), Pos2::new(rect.right(), rect.bottom() - 0.5)], Stroke::new(1.0_f32, Color32::from_white_alpha(18)));
            });
    }

    /// Knopf „Einstellungen“ in der Fensterleiste (#240), rechts vor den Fensterknöpfen; `right` = rechte Kante.
    /// Liefert die rechte Kante für den nächsten Knopf links davon.
    fn settings_title_button(&mut self, ui: &egui::Ui, right: f32) -> f32 {
        let lang = self.settings.language;
        let r = Rect::from_min_size(Pos2::new(right - 40.0, ui.max_rect().top() + 4.0), Vec2::new(40.0, BAR_HEIGHT - 8.0));
        let resp = title_button(ui, r, Id::new("titlebar_settings"), lang.t("Einstellungen", "Settings"), self.settings_open);
        let fg = if self.settings_open { style::accent() } else { ui.visuals().text_color() };
        style::paint_icon(ui.painter(), SIcon::Gear, r.center(), fg);
        if resp.clicked() {
            self.settings_open = !self.settings_open;
        }
        r.left() - 4.0
    }

    /// Resize-Ränder des rahmenlosen Fensters (Zeiger am Rand → Pfeil, Ziehen → Größe ändern) und eine feine Umrandung.
    pub(super) fn window_edges(&mut self, ctx: &egui::Context) {
        let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
        let screen = ctx.content_rect();
        if !maximized && !fullscreen {
            ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("window_border")))
                .rect_stroke(screen, crate::platform::chrome::corner_radius_pts(ctx.zoom_factor(), crate::platform::chrome::rounded(), false), Stroke::new(1.0_f32, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
            let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else { return };
            let left = p.x - screen.left() < EDGE;
            let right = screen.right() - p.x < EDGE;
            let top = p.y - screen.top() < EDGE;
            let bottom = screen.bottom() - p.y < EDGE;
            let dir = match (left, right, top, bottom) {
                (true, _, true, _) => Some(ResizeDirection::NorthWest),
                (_, true, true, _) => Some(ResizeDirection::NorthEast),
                (true, _, _, true) => Some(ResizeDirection::SouthWest),
                (_, true, _, true) => Some(ResizeDirection::SouthEast),
                (true, ..) => Some(ResizeDirection::West),
                (_, true, ..) => Some(ResizeDirection::East),
                (_, _, true, _) => Some(ResizeDirection::North),
                (.., true) => Some(ResizeDirection::South),
                _ => None,
            };
            if let Some(d) = dir {
                ctx.set_cursor_icon(match d {
                    ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
                    ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
                    ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
                    _ => CursorIcon::ResizeNeSw,
                });
                if ctx.input(|i| i.pointer.primary_pressed()) {
                    ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
                }
            }
        }
    }
}


/// Knopf im Stil der Reiter der Fensterleiste (#240): Fläche beim Überfahren, aktiv in der Akzentfarbe, Hand, Tooltip.
pub(super) fn title_button(ui: &egui::Ui, r: Rect, id: Id, tip: &str, active: bool) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click()).on_hover_text(tip);
    let painter = ui.painter();
    if active {
        painter.rect_filled(r, 6.0, style::accent().gamma_multiply(if resp.hovered() { 0.34 } else { 0.22 }));
    } else if resp.hovered() {
        painter.rect_filled(r, 6.0, Color32::from_white_alpha(14));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// Fenstertitel in den freien Platz zwischen `left` und `right` legen: möglichst mittig bei `center`, sonst mittig im
/// freien Platz; zu lang → mit „…“ gekürzt. `None`, wenn kein sinnvoller Platz bleibt. `width` misst einen Text.
/// Liefert (Text, x-Mitte).
fn title_place(title: &str, left: f32, right: f32, center: f32, width: impl Fn(&str) -> f32) -> Option<(String, f32)> {
    let room = right - left;
    if room < 24.0 || title.is_empty() {
        return None;
    }
    let text = if width(title) <= room {
        title.to_string()
    } else {
        // Längsten Anfang suchen, der mit „…“ passt (binäre Suche über die Zeichenzahl).
        let chars: Vec<char> = title.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let cand: String = chars[..mid].iter().collect::<String>() + "…";
            if width(&cand) <= room { lo = mid } else { hi = mid - 1 }
        }
        if lo == 0 {
            return None;
        }
        chars[..lo].iter().collect::<String>().trim_end().to_string() + "…"
    };
    let w = width(&text);
    let cx = if center - w / 2.0 >= left && center + w / 2.0 <= right { center } else { (left + right) / 2.0 };
    Some((text, cx))
}

#[cfg(test)]
mod tests {
    use super::{title_button, title_place};
    use eframe::egui::{self, Id, Pos2, Rect, Vec2};

    /// Knöpfe der Fensterleiste (Einstellungen, MCP-Status, #240): Hand und Tooltip wie bei allen Knöpfen, daneben nicht.
    #[test]
    fn title_buttons_show_the_hand_and_a_tooltip() {
        use crate::afford::tests::{cursor_at, tooltip_at};
        let r = Rect::from_min_size(Pos2::new(300.0, 4.0), Vec2::new(40.0, 26.0));
        for active in [false, true] {
            let mut f = |ui: &mut egui::Ui| {
                title_button(ui, r, Id::new("t"), "Settings", active);
            };
            assert_eq!(cursor_at(r.center(), &mut f), egui::CursorIcon::PointingHand, "Hand (aktiv: {active})");
            assert!(tooltip_at(r.center(), &mut f), "Tooltip (aktiv: {active})");
            assert_ne!(cursor_at(r.center() + Vec2::new(80.0, 0.0), &mut f), egui::CursorIcon::PointingHand, "daneben keine Hand");
        }
    }

    fn w(s: &str) -> f32 {
        s.chars().count() as f32 * 7.0
    }

    #[test]
    fn title_stays_centered_when_it_fits() {
        let (t, cx) = title_place("api-server – Paneldeck", 130.0, 1000.0, 600.0, w).unwrap();
        assert_eq!(t, "api-server – Paneldeck");
        assert_eq!(cx, 600.0);
    }

    #[test]
    fn title_moves_into_the_free_space_and_is_truncated_to_it() {
        // Mitte wäre von den Knöpfen rechts überdeckt: in die freie Mitte rücken
        let (t, cx) = title_place("abcdefghij", 130.0, 300.0, 320.0, w).unwrap();
        assert_eq!((t.as_str(), cx), ("abcdefghij", 215.0));
        // zu lang für den freien Platz (640-pt-Fenster mit Update- und MCP-Knopf): gekürzt, passt hinein
        let long = "(3) a very long session name that never fits – Paneldeck";
        let (t, cx) = title_place(long, 130.0, 300.0, 320.0, w).unwrap();
        assert!(t.ends_with('…') && w(&t) <= 170.0, "{t}");
        assert!(cx - w(&t) / 2.0 >= 130.0 && cx + w(&t) / 2.0 <= 300.0, "{t} bei {cx}");
        // kein Platz: kein Titel
        assert!(title_place(long, 130.0, 140.0, 320.0, w).is_none());
    }
}
