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
                painter.rect_filled(Rect::from_center_size(icon_c, Vec2::splat(18.0)), 5.0, style::ACCENT);
                style::paint_icon(painter, SIcon::Terminal, icon_c, Color32::WHITE);
                // Reiter
                let tabs: [(Mode, SIcon, &str); 2] = [
                    (Mode::Terminals, SIcon::Terminal, lang.t("Terminal Hub", "Terminal Hub")),
                    (Mode::Tasks, SIcon::Checklist, lang.t("Todo-Liste", "Todo list")),
                ];
                let ctrl_hint = lang.t("Strg+Klick: in neuem Fenster öffnen", "Ctrl+click: open in a new window");
                let mut x = rect.left() + 42.0;
                for (mode, icon, tip) in tabs {
                    let r = Rect::from_min_size(Pos2::new(x, rect.top() + 4.0), Vec2::new(40.0, BAR_HEIGHT - 8.0));
                    let resp = ui.interact(r, Id::new(("titlebar_tab", mode as u8)), Sense::click()).on_hover_text(format!("{tip}\n{ctrl_hint}"));
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
                // Fenstertitel (mittig, ohne die Knöpfe zu überdecken)
                let max_chars = 70;
                let shown: String = if title.chars().count() > max_chars { title.chars().take(max_chars - 1).chain(std::iter::once('…')).collect() } else { title };
                painter.text(Pos2::new(rect.center().x, rect.center().y), Align2::CENTER_CENTER, shown, FontId::proportional(12.5), if focused { dim } else { dim.gamma_multiply(0.7) });
                // Fensterknöpfe rechts
                let bw = 46.0;
                let mut bx = rect.right() - bw * 3.0;
                self.update_title_button(ui, bx); // Update-Knopf links neben Minimieren (update/ui.rs)
                for (i, icon) in [SIcon::Minimize, if maximized { SIcon::Restore } else { SIcon::Maximize }, SIcon::Close].into_iter().enumerate() {
                    let r = Rect::from_min_size(Pos2::new(bx, rect.top()), Vec2::new(bw, BAR_HEIGHT));
                    let resp = ui.interact(r, Id::new(("titlebar_btn", i)), Sense::click());
                    let close = i == 2;
                    // Maximieren-Knopf: Rechteck ans System melden (Snap-Layouts); dort kommt der Hover nicht mehr von egui.
                    // Nur im Hauptfenster: weitere Fenster haben keine eigene Fensterprozedur (dort ohne Snap-Layouts).
                    let main = self.wins.cur == 0;
                    let snap_hover = main && i == 1 && chrome::max_hover();
                    if main && i == 1 {
                        chrome::report_max_button(r, ctx.pixels_per_point(), if maximized { 0.0 } else { EDGE });
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

    /// Resize-Ränder des rahmenlosen Fensters (Zeiger am Rand → Pfeil, Ziehen → Größe ändern) und eine feine Umrandung.
    pub(super) fn window_edges(&mut self, ctx: &egui::Context) {
        let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
        let screen = ctx.content_rect();
        if !maximized && !fullscreen {
            ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("window_border")))
                .rect_stroke(screen, chrome::corner_radius_pts(ctx.zoom_factor(), chrome::rounded(), false), Stroke::new(1.0_f32, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
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
