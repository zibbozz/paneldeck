//! Design-System: Farbflächen (aus dem Terminal-Theme abgeleitet), Abstände, Schriftgrößen,
//! selbst gezeichnete Icons und kleine Bausteine (Icon-Buttons, Pills).
//!
//! Grundsätze: Das Terminal dominiert, die Oberfläche drumherum bleibt ruhig und wird über
//! leichte Helligkeitsstufen statt über Rahmen gegliedert; eine Akzentfarbe; Abstände im 4-px-Raster;
//! wenige Schriftgrößen; ausreichender Kontrast (kein reines Schwarz/Weiß).

use crate::theme::Theme;
use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, FontFamily, FontId, Margin, Painter, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, TextStyle, Ui, Vec2,
};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// Motion-Tokens: Hover/Klick ~120 ms, Zustandswechsel ~200 ms, Layout ~280 ms; Kurve: ease-out.
pub const D_FAST: f32 = 0.12;
pub const D_BASE: f32 = 0.20;

static ANIMATIONS: AtomicBool = AtomicBool::new(true);

pub fn set_animations(on: bool) {
    ANIMATIONS.store(on, Ordering::Relaxed);
}

/// Weicher Übergang 0..1 hin zu `target` (ease-out). Bei abgeschalteten Animationen sofort.
pub fn anim(ctx: &egui::Context, id: egui::Id, target: bool, dur: f32) -> f32 {
    let d = if ANIMATIONS.load(Ordering::Relaxed) { dur } else { 0.0 };
    ctx.animate_bool_with_time_and_easing(id, target, d, egui::emath::easing::cubic_out)
}

/// Standard-Akzentfarbe (Terminal Hub). Im Todolist-Reiter ersetzt die Projektfarbe sie (siehe [`accent`]).
pub const ACCENT: Color32 = Color32::from_rgb(0xd9, 0x77, 0x57);

static ACCENT_RGB: AtomicU32 = AtomicU32::new(0x00d9_7757);

/// Aktuelle Akzentfarbe: Standard ([`ACCENT`]) oder die Farbe des gewählten Projekts im Todolist-Reiter.
pub fn accent() -> Color32 {
    let v = ACCENT_RGB.load(Ordering::Relaxed);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Setzt die Akzentfarbe (jedes Bild von der Anwendung aufgerufen).
pub fn set_accent(c: Color32) {
    ACCENT_RGB.store(((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32, Ordering::Relaxed);
}

// Abstände (4-px-Raster)
pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
pub const RADIUS: u8 = 6;

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

fn opaque(c: Color32) -> Color32 {
    Color32::from_rgb(c.r(), c.g(), c.b())
}

/// Stil auf den egui-Kontext anwenden. `opacity` (0.3–1.0) wirkt auf Seitenleiste und Flächen.
pub fn apply(ctx: &egui::Context, theme: &Theme, opacity: f32) {
    let bg = opaque(theme.bg);
    let (white, black) = (Color32::WHITE, Color32::BLACK);
    let dark = theme.dark;
    // Flächenstufen: Seitenleiste etwas abgesetzt, Kopfzeilen/Eingabefelder/Hover in feinen Schritten.
    let side = if dark { mix(bg, black, 0.22) } else { mix(bg, black, 0.045) };
    let raised = if dark { mix(bg, white, 0.045) } else { mix(bg, white, 0.6) };
    let hover = if dark { mix(side, white, 0.07) } else { mix(side, black, 0.06) };
    let input = if dark { mix(side, black, 0.30) } else { mix(side, white, 0.7) };
    let border = if dark { mix(bg, white, 0.12) } else { mix(bg, black, 0.14) };
    let text = if dark { mix(theme.fg, white, 0.18) } else { mix(theme.fg, black, 0.15) };
    let dim = mix(text, side, 0.42);
    let a = opacity.clamp(0.3, 1.0);
    let fade = |c: Color32| c.gamma_multiply(a);

    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = fade(side);
    v.window_fill = if dark { mix(side, white, 0.05) } else { raised };
    v.window_stroke = Stroke::new(1.0_f32, border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    let shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 130 } else { 50 }) };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.faint_bg_color = fade(raised);
    v.extreme_bg_color = input;
    v.code_bg_color = input;
    v.override_text_color = None;
    v.hyperlink_color = accent();
    v.selection.bg_fill = accent().gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, accent());
    v.text_cursor.stroke = Stroke::new(1.5_f32, accent());

    let r = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = side;
    w.noninteractive.weak_bg_fill = side;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = if dark { mix(side, white, 0.10) } else { mix(side, black, 0.08) };
    w.inactive.weak_bg_fill = w.inactive.bg_fill;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, mix(side, text, 0.16));
    w.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = hover;
    w.hovered.weak_bg_fill = hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, mix(border, text, 0.25));
    w.hovered.fg_stroke = Stroke::new(1.0_f32, if dark { white } else { black });
    w.hovered.corner_radius = r;
    w.active.bg_fill = accent().gamma_multiply(0.45);
    w.active.weak_bg_fill = accent().gamma_multiply(0.45);
    w.active.bg_stroke = Stroke::new(1.0_f32, accent());
    w.active.fg_stroke = Stroke::new(1.0_f32, if dark { white } else { black });
    w.active.corner_radius = r;
    w.open = w.hovered.clone();

    let mut style = (*ctx.style()).clone();
    style.visuals = v;
    style.spacing.item_spacing = Vec2::new(S2, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size = Vec2::new(32.0, 26.0);
    style.spacing.window_margin = Margin::same(14);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.indent = 14.0;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.spacing.slider_width = 180.0;
    style.animation_time = if ANIMATIONS.load(Ordering::Relaxed) { 0.18 } else { 0.0 };
    style.text_styles.insert(TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Button, FontId::new(13.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
    ctx.set_style(style);
    let _ = (dim,);
}

/// Dezente Zweitfarbe für Hinweistexte.
pub fn dim(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Plus,
    Spark,
    SparkSmall,
    Search,
    Clock,
    Folder,
    Broadcast,
    Gear,
    Prompt,
    Close,
    SplitRight,
    SplitDown,
    Detach,
    Note,
    Doc,
    Diff,
    Up,
    Down,
    Queue,
    Tree,
    List,
    FolderPlus,
    Template,
    Terminal,
    Checklist,
    Trash,
    Minimize,
    Maximize,
    Restore,
}

fn arc(p: &Painter, c: Pos2, r: f32, a0: f32, a1: f32, st: Stroke) {
    let n = 10;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, st));
}

/// Zeichnet ein 16-px-Icon mittig um `c`.
pub fn paint_icon(p: &Painter, icon: Icon, c: Pos2, col: Color32) {
    let st = Stroke::new(1.4_f32, col);
    let l = |a: Vec2, b: Vec2| p.line_segment([c + a, c + b], st);
    let v = Vec2::new;
    match icon {
        Icon::Plus => {
            l(v(-5.0, 0.0), v(5.0, 0.0));
            l(v(0.0, -5.0), v(0.0, 5.0));
        }
        Icon::Spark => {
            let st2 = Stroke::new(1.6_f32, col);
            for (a, b) in [(v(0.0, -7.0), v(0.0, 7.0)), (v(-7.0, 0.0), v(7.0, 0.0)), (v(-4.5, -4.5), v(4.5, 4.5)), (v(-4.5, 4.5), v(4.5, -4.5))] {
                p.line_segment([c + a, c + b], st2);
            }
        }
        Icon::SparkSmall => {
            let st2 = Stroke::new(1.3_f32, col);
            for (a, b) in [(v(0.0, -4.0), v(0.0, 4.0)), (v(-4.0, 0.0), v(4.0, 0.0)), (v(-2.8, -2.8), v(2.8, 2.8)), (v(-2.8, 2.8), v(2.8, -2.8))] {
                p.line_segment([c + a, c + b], st2);
            }
        }
        Icon::Search => {
            p.circle_stroke(c + v(-1.5, -1.5), 4.6, st);
            l(v(1.8, 1.8), v(6.0, 6.0));
        }
        Icon::Tree => {
            l(v(-5.0, -5.0), v(-5.0, 5.0));
            l(v(-5.0, -3.0), v(5.0, -3.0));
            l(v(-5.0, 1.0), v(2.0, 1.0));
            l(v(-5.0, 5.0), v(5.0, 5.0));
        }
        Icon::List => {
            for y in [-4.5, 0.0, 4.5] {
                l(v(-5.5, y), v(5.5, y));
            }
        }
        Icon::FolderPlus => {
            p.rect_stroke(Rect::from_center_size(c + v(-1.0, 0.5), v(11.0, 8.0)), 1.5, st, StrokeKind::Inside);
            l(v(4.0, -6.0), v(4.0, -1.0));
            l(v(1.5, -3.5), v(6.5, -3.5));
        }
        Icon::Template => {
            p.rect_stroke(Rect::from_center_size(c + v(-2.0, -2.0), v(9.0, 9.0)), 1.5, st, StrokeKind::Inside);
            p.rect_stroke(Rect::from_center_size(c + v(2.0, 2.0), v(9.0, 9.0)), 1.5, st, StrokeKind::Inside);
        }
        Icon::Clock => {
            p.circle_stroke(c, 6.2, st);
            l(v(0.0, -3.5), v(0.0, 0.0));
            l(v(0.0, 0.0), v(3.0, 1.8));
        }
        Icon::Folder => {
            let r = Rect::from_center_size(c + v(0.0, 1.0), v(13.0, 9.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-6.5, -3.6), v(-1.5, -3.6));
        }
        Icon::Broadcast => {
            p.circle_filled(c, 1.8, col);
            for dir in [0.0_f32, std::f32::consts::PI] {
                arc(p, c, 4.5, dir - 0.7, dir + 0.7, st);
                arc(p, c, 7.5, dir - 0.7, dir + 0.7, st);
            }
        }
        Icon::Gear => {
            p.circle_stroke(c, 3.0, st);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (s, co) = a.sin_cos();
                p.line_segment([c + v(co * 5.0, s * 5.0), c + v(co * 7.0, s * 7.0)], Stroke::new(2.0_f32, col));
            }
            p.circle_stroke(c, 5.2, st);
        }
        Icon::Prompt => {
            let r = Rect::from_center_size(c + v(0.0, -1.0), v(13.0, 9.5));
            p.rect_stroke(r, 2.0, st, StrokeKind::Inside);
            l(v(-3.0, 3.8), v(-3.0, 6.5));
            l(v(-3.0, 6.5), v(0.0, 3.8));
        }
        Icon::Terminal => {
            let r = Rect::from_center_size(c, v(15.0, 11.0));
            p.rect_stroke(r, 2.5, st, StrokeKind::Inside);
            l(v(-4.0, -2.0), v(-1.5, 0.0));
            l(v(-1.5, 0.0), v(-4.0, 2.0));
            l(v(0.5, 2.5), v(4.0, 2.5));
        }
        Icon::Checklist => {
            for (y, check) in [(-5.0_f32, true), (0.0, true), (5.0, false)] {
                if check {
                    l(v(-6.5, y), v(-5.0, y + 1.5));
                    l(v(-5.0, y + 1.5), v(-2.5, y - 1.5));
                } else {
                    p.rect_stroke(Rect::from_center_size(c + v(-4.5, y), v(4.0, 4.0)), 0.5, st, StrokeKind::Inside);
                }
                l(v(0.0, y), v(6.5, y));
            }
        }
        Icon::Trash => {
            // Deckel mit Griff, Eimer mit zwei Rillen
            l(v(-6.0, -4.0), v(6.0, -4.0));
            l(v(-2.0, -4.0), v(-2.0, -6.0));
            l(v(-2.0, -6.0), v(2.0, -6.0));
            l(v(2.0, -6.0), v(2.0, -4.0));
            l(v(-4.5, -4.0), v(-3.7, 6.0));
            l(v(-3.7, 6.0), v(3.7, 6.0));
            l(v(3.7, 6.0), v(4.5, -4.0));
            l(v(-1.5, -1.5), v(-1.5, 3.5));
            l(v(1.5, -1.5), v(1.5, 3.5));
        }
        Icon::Minimize => {
            l(v(-5.0, 0.5), v(5.0, 0.5));
        }
        Icon::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, v(9.0, 9.0)), 0.0, st, StrokeKind::Inside);
        }
        Icon::Restore => {
            p.rect_stroke(Rect::from_center_size(c + v(-1.0, 1.0), v(7.5, 7.5)), 0.0, st, StrokeKind::Inside);
            l(v(0.0, -4.5), v(4.5, -4.5));
            l(v(4.5, -4.5), v(4.5, 0.0));
            l(v(0.0, -4.5), v(0.0, -2.8));
        }
        Icon::Close => {
            l(v(-4.0, -4.0), v(4.0, 4.0));
            l(v(-4.0, 4.0), v(4.0, -4.0));
        }
        Icon::SplitRight => {
            let r = Rect::from_center_size(c, v(12.0, 10.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(0.0, -5.0), v(0.0, 5.0));
        }
        Icon::SplitDown => {
            let r = Rect::from_center_size(c, v(12.0, 10.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-6.0, 0.0), v(6.0, 0.0));
        }
        Icon::Detach => {
            let r = Rect::from_center_size(c + v(-1.5, 1.5), v(9.0, 8.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(5.5, -5.5), v(1.5, -5.5));
            l(v(5.5, -5.5), v(5.5, -1.5));
            l(v(5.5, -5.5), v(-0.5, 0.5));
        }
        Icon::Diff => {
            let r = Rect::from_center_size(c, v(12.0, 14.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-3.0, -3.0), v(3.0, -3.0));
            l(v(0.0, -6.0), v(0.0, 0.0));
            l(v(-3.0, 3.5), v(3.0, 3.5));
        }
        Icon::Up => {
            l(v(-4.5, 2.0), v(0.0, -3.0));
            l(v(0.0, -3.0), v(4.5, 2.0));
        }
        Icon::Down => {
            l(v(-4.5, -2.0), v(0.0, 3.0));
            l(v(0.0, 3.0), v(4.5, -2.0));
        }
        Icon::Queue => {
            for y in [-4.5, 0.0, 4.5] {
                l(v(-2.0, y), v(6.0, y));
                p.circle_filled(c + v(-5.5, y), 1.2, col);
            }
        }
        Icon::Doc => {
            let r = Rect::from_center_size(c, v(11.0, 14.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-3.0, -2.0), v(3.0, -2.0));
            l(v(-3.0, 1.0), v(3.0, 1.0));
            l(v(-3.0, 4.0), v(1.0, 4.0));
        }
        Icon::Note => {
            for y in [-3.5, 0.0, 3.5] {
                l(v(-5.0, y), v(5.0, y));
            }
        }
    }
}

/// Quadratischer Icon-Button (flach, Hover-Fläche, Tooltip, optional "aktiv").
pub fn icon_btn(ui: &mut Ui, icon: Icon, tip: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let v = ui.visuals();
    let (text, strong, hover_bg) = (v.text_color(), v.strong_text_color(), v.widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    let on = anim(ui.ctx(), resp.id.with("on"), active, D_BASE);
    ui.painter().rect_filled(rect, RADIUS as f32, hover_bg.gamma_multiply(h));
    ui.painter().rect_filled(rect, RADIUS as f32, accent().gamma_multiply(0.28 * on));
    let base = mix(text.gamma_multiply(0.78), strong, h);
    let col = mix(base, accent(), on);
    paint_icon(ui.painter(), icon, rect.center(), if icon == Icon::Spark && !active { accent() } else { col });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

/// Kleinerer Icon-Button für Kopfzeilen (22 px).
pub fn icon_btn_small(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let v = ui.visuals();
    let (text, strong, hover_bg) = (v.weak_text_color(), v.strong_text_color(), v.widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(rect, 5.0, hover_bg.gamma_multiply(h));
    paint_icon(ui.painter(), icon, rect.center(), mix(text, strong, h));
    resp.on_hover_text(tip)
}

/// Pill (Segment) – z. B. für Projekte. Gibt die Antwort zurück.
pub fn pill(ui: &mut Ui, text: &str, count: Option<usize>, color: Color32, active: bool) -> Response {
    let font = FontId::proportional(12.5);
    let label = ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::PLACEHOLDER);
    let cnt = count.map(|n| ui.painter().layout_no_wrap(n.to_string(), FontId::proportional(11.0), Color32::PLACEHOLDER));
    let w = 12.0 + 8.0 + label.size().x + cnt.as_ref().map(|c| c.size().x + 6.0).unwrap_or(0.0) + 8.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    let v = ui.visuals();
    let (text_c, hover_bg) = (v.text_color(), v.widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    let on = anim(ui.ctx(), resp.id.with("on"), active, D_BASE);
    ui.painter().rect_filled(rect, 12.0, hover_bg.gamma_multiply(h * (1.0 - on)));
    ui.painter().rect_filled(rect, 12.0, color.gamma_multiply(0.28 * on));
    ui.painter().rect_stroke(rect, 12.0, Stroke::new(1.0_f32, color.gamma_multiply(0.7 * on)), StrokeKind::Inside);
    ui.painter().circle_filled(Pos2::new(rect.min.x + 12.0, rect.center().y), 3.5, color);
    let tc = mix(text_c.gamma_multiply(0.75), text_c, on.max(h * 0.6));
    let mut x = rect.min.x + 12.0 + 8.0;
    ui.painter().galley(Pos2::new(x, rect.center().y - label.size().y / 2.0), label.clone(), tc);
    x += label.size().x + 6.0;
    if let Some(c) = cnt {
        ui.painter().galley(Pos2::new(x, rect.center().y - c.size().y / 2.0), c, tc.gamma_multiply(0.6));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Statuspille mit Punkt und Text (rechts in Kopfzeilen).
pub fn status_pill(p: &Painter, right_center: Pos2, label: &str, color: Color32) -> f32 {
    let g = p.layout_no_wrap(label.to_string(), FontId::proportional(11.5), color);
    let w = g.size().x + 8.0 + 14.0 + 4.0;
    let rect = Rect::from_min_size(Pos2::new(right_center.x - w, right_center.y - 10.0), Vec2::new(w, 20.0));
    p.rect_filled(rect, 10.0, color.gamma_multiply(0.18));
    p.circle_filled(Pos2::new(rect.min.x + 10.0, rect.center().y), 3.2, color);
    p.galley(Pos2::new(rect.min.x + 18.0, rect.center().y - g.size().y / 2.0), g, color);
    w
}

/// Animierter Ein/Aus-Schalter.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let t = anim(ui.ctx(), resp.id.with("t"), *on, D_BASE);
    let off_col = ui.visuals().widgets.inactive.bg_fill.gamma_multiply(1.6);
    let track = mix(off_col, accent(), t);
    ui.painter().rect_filled(rect, 10.0, track);
    let x = rect.min.x + 10.0 + (rect.width() - 20.0) * t;
    ui.painter().circle_filled(Pos2::new(x, rect.center().y), 7.0, Color32::from_rgb(0xf4, 0xf4, 0xf6));
    resp
}

/// Tab-Pille ohne Farbpunkt (z. B. Einstellungen).
pub fn tab(ui: &mut Ui, text: &str, active: bool) -> Response {
    let font = FontId::proportional(13.0);
    let g = ui.painter().layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 24.0, 26.0), Sense::click());
    let (text_c, hover_bg) = (ui.visuals().text_color(), ui.visuals().widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    let on = anim(ui.ctx(), resp.id.with("on"), active, D_BASE);
    ui.painter().rect_filled(rect, 13.0, hover_bg.gamma_multiply(h * (1.0 - on)));
    ui.painter().rect_filled(rect, 13.0, accent().gamma_multiply(0.25 * on));
    let tc = mix(text_c.gamma_multiply(0.72), text_c, on.max(h * 0.6));
    ui.painter().galley(Pos2::new(rect.min.x + 12.0, rect.center().y - g.size().y / 2.0), g, tc);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}
