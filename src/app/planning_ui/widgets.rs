//! Wiederverwendbare Bausteine der Projekt-Oberfläche: Farbwahl (Palette + Hex), Icon-Auswahl mit selbst gezeichneten
//! Typ-Icons, Textfelder mit „beim Verlassen übernehmen“, Schaltflächen, Karten, Dialog-Rahmen.

use super::helpers::color_of;
use crate::planning::common::normalize_color;
use crate::planning::icons::ICONS;
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Color32, Id, Key, Painter, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::hash::Hash;

pub const DANGER: Color32 = Color32::from_rgb(0xc2, 0x41, 0x41);

// ------------------------------------------------------------------ Schaltflächen und Rahmen

/// Hauptaktion (Akzentfarbe).
pub fn primary(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(text).strong()).fill(style::accent().gamma_multiply(0.5)).min_size(Vec2::new(96.0, 28.0)))
}

pub fn primary_enabled(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).strong()).fill(style::accent().gamma_multiply(0.5)).min_size(Vec2::new(96.0, 28.0)))
}

pub fn secondary(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(text).min_size(Vec2::new(90.0, 28.0)))
}

/// Zerstörende Aktion; `wait` = Sekunden bis zur Freigabe (0 = sofort), im Text als Countdown.
pub fn danger(ui: &mut Ui, text: &str, wait: u32) -> Response {
    let label = if wait > 0 { format!("{text} ({wait})") } else { text.to_string() };
    let b = egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(if wait > 0 { DANGER.gamma_multiply(0.4) } else { DANGER }).min_size(Vec2::new(110.0, 28.0));
    ui.add_enabled(wait == 0, b)
}

pub fn small_danger(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(text).small().color(Color32::from_rgb(0xe0, 0x6c, 0x75))))
}

/// Abgesetzte Karte für zusammengehörige Einstellungen.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let v = ui.visuals();
    egui::Frame::NONE
        .fill(v.faint_bg_color)
        .stroke(Stroke::new(1.0_f32, v.widgets.noninteractive.bg_stroke.color))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Kleine Zwischenüberschrift.
pub fn section(ui: &mut Ui, text: &str) {
    ui.add_space(S1);
    ui.label(RichText::new(text).strong());
}

/// Hinweistext in gedämpfter Farbe.
pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(style::dim(ui)));
}

pub fn error(ui: &mut Ui, text: &str) {
    ui.colored_label(Color32::from_rgb(0xe0, 0x6c, 0x75), text);
}

pub fn modal_width(ctx: &egui::Context, want: f32) -> f32 {
    want.min(ctx.content_rect().width() - 80.0).max(260.0)
}

/// Beschriftung links, Inhalt rechts in einer Rasterzeile (für Formulare).
pub fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.label(RichText::new(label).color(style::dim(ui)));
    let r = add(ui);
    ui.end_row();
    r
}

/// Eigene Checkbox mit gut sichtbarem Rahmen (die Standard-Checkbox ist auf dunklem Grund kaum zu sehen). `label` darf leer sein.
pub fn check(ui: &mut Ui, on: &mut bool, label: impl AsRef<str>) -> Response {
    let label = label.as_ref();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = (!label.is_empty()).then(|| ui.painter().layout_no_wrap(label.to_string(), font, ui.visuals().text_color()));
    let w = 18.0 + galley.as_ref().map(|g| g.size().x + 8.0).unwrap_or(0.0);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let b = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 9.0), Vec2::splat(18.0));
    paint_box(ui, b, *on, resp.hovered());
    if let Some(g) = galley {
        ui.painter().galley(Pos2::new(rect.left() + 26.0, rect.center().y - g.size().y / 2.0), g, ui.visuals().text_color());
    }
    resp
}

/// Eigener Radioknopf mit gut sichtbarem Rahmen; wählt `value`, wenn angeklickt.
pub fn radio<T: PartialEq>(ui: &mut Ui, cur: &mut T, value: T, label: impl AsRef<str>) -> Response {
    let label = label.as_ref();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, ui.visuals().text_color());
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(18.0 + galley.size().x + 8.0, 22.0), Sense::click());
    let selected = *cur == value;
    if resp.clicked() && !selected {
        *cur = value;
        resp.mark_changed();
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let c = Pos2::new(rect.left() + 9.0, rect.center().y);
    let a = style::accent();
    let ring = if selected || resp.hovered() { a } else { ui.visuals().text_color().gamma_multiply(0.45) };
    ui.painter().circle_stroke(c, 8.0, Stroke::new(1.3_f32, ring));
    if selected {
        ui.painter().circle_filled(c, 4.5, a);
    }
    ui.painter().galley(Pos2::new(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0), galley, ui.visuals().text_color());
    resp
}

/// Das Kästchen einer Checkbox (angehakt = Akzentfarbe mit Haken, sonst Rahmen).
pub fn paint_box(ui: &Ui, rect: Rect, on: bool, hovered: bool) {
    let a = style::accent();
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 5.0, a.gamma_multiply(0.8));
        let c = rect.center();
        let s = Stroke::new(1.8_f32, Color32::WHITE);
        p.line_segment([c + Vec2::new(-4.0, 0.5), c + Vec2::new(-1.2, 3.3)], s);
        p.line_segment([c + Vec2::new(-1.2, 3.3), c + Vec2::new(4.5, -3.5)], s);
    } else {
        p.rect_filled(rect, 5.0, if hovered { a.gamma_multiply(0.15) } else { Color32::TRANSPARENT });
        p.rect_stroke(rect, 5.0, Stroke::new(1.3_f32, if hovered { a } else { ui.visuals().text_color().gamma_multiply(0.45) }), StrokeKind::Inside);
    }
}

/// Ein/Aus-Zeile: Schalter + Text (+ gedämpfte Erklärung darunter).
pub fn toggle_row(ui: &mut Ui, on: &mut bool, text: &str, explain: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed = style::toggle(ui, on).changed();
        ui.vertical(|ui| {
            ui.label(text);
            if !explain.is_empty() {
                hint(ui, explain);
            }
        });
    });
    changed
}

// ------------------------------------------------------------------ Textfelder

/// Einzeiliges Feld, das erst beim Verlassen oder mit Enter einen neuen Wert meldet (Escape verwirft). `shown` ist der
/// gespeicherte Wert.
pub fn commit_text(ui: &mut Ui, id: Id, shown: &str, width: f32, hint_text: &str) -> Option<String> {
    let buf_id = id.with("buf");
    let focused = ui.memory(|m| m.has_focus(id));
    let mut buf: String = if focused { ui.data_mut(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| shown.to_string()) } else { shown.to_string() };
    let mut te = egui::TextEdit::singleline(&mut buf).id(id).desired_width(width).margin(egui::Margin::symmetric(8, 4));
    if !hint_text.is_empty() {
        te = te.hint_text(hint_text);
    }
    // feste Breite: in einem Raster würde das Feld sonst auf die Breite der Überschrift schrumpfen
    let r = ui.add_sized([width, 26.0], te);
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(buf_id, buf.clone()));
    }
    if r.lost_focus() {
        let esc = ui.input(|i| i.key_pressed(Key::Escape));
        let stored: Option<String> = ui.data_mut(|d| d.remove_temp(buf_id));
        if !esc {
            let v = stored.unwrap_or(buf);
            if v != shown {
                return Some(v);
            }
        }
    }
    None
}

// ------------------------------------------------------------------ Farben

/// „#RRGGBB“ aus einer Eingabe mit oder ohne `#` (Großbuchstaben); sonst `None`.
pub fn hex_input(s: &str) -> Option<String> {
    let t = s.trim();
    let full = if t.starts_with('#') { t.to_string() } else { format!("#{t}") };
    normalize_color(&full).ok()
}

/// Farbfläche (Kreis) mit Rahmen; klickbar.
pub fn swatch(ui: &mut Ui, id: Id, color: Color32, size: f32) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size + 4.0), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let c = rect.center();
    ui.painter().circle_filled(c, size / 2.0, color);
    ui.painter().circle_stroke(c, size / 2.0 + 0.5, Stroke::new(1.0_f32, Color32::from_white_alpha(if resp.hovered() { 140 } else { 60 })));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Palette und Hex-Feld untereinander (im Dialog eingebettet oder im Popup). Gibt `true` zurück, wenn sich die Farbe änderte.
pub fn palette_and_hex(ui: &mut Ui, salt: impl Hash, hex: &mut String, lang: Lang) -> bool {
    let salt = Id::new(salt);
    let mut changed = false;
    // Palette des Expertenmodus (Tooltip mit Name und Wert); freie Farben über das Hex-Feld darunter.
    if let Some(c) = crate::app::palette_ui::pick(ui, salt, crate::palette::Area::Expert, Some(hex), crate::app::palette_ui::PickOpts::EXPERT_WITH_HEX, lang) {
        *hex = c;
        ui.data_mut(|d| d.remove_temp::<String>(salt.with("hexbuf")));
        changed = true;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(lang.t("Hex:", "Hex:")).color(style::dim(ui)));
        let id = salt.with("hexfield");
        let buf_id = salt.with("hexbuf");
        let focused = ui.memory(|m| m.has_focus(id));
        let mut buf: String = if focused { ui.data_mut(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| hex.clone()) } else { hex.clone() };
        let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id).desired_width(80.0).char_limit(7).margin(egui::Margin::symmetric(8, 4)));
        if r.changed() {
            ui.data_mut(|d| d.insert_temp(buf_id, buf.clone()));
            if let Some(c) = hex_input(&buf) {
                if c != *hex {
                    *hex = c;
                    changed = true;
                }
            }
        }
        if hex_input(&buf).is_none() {
            ui.label(RichText::new(lang.t("Form #RRGGBB", "Use #RRGGBB")).small().color(Color32::from_rgb(0xe0, 0x6c, 0x75)));
        }
    });
    changed
}

/// Farbfeld, das beim Klick ein Popup mit Palette und Hex-Feld öffnet.
pub fn color_edit(ui: &mut Ui, salt: impl Hash, hex: &mut String, lang: Lang) -> bool {
    let salt = Id::new(salt);
    let resp = swatch(ui, salt.with("sw"), color_of(hex), 16.0).on_hover_text(lang.t("Farbe ändern", "Change color"));
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        changed = palette_and_hex(ui, salt.with("pop"), hex, lang);
    });
    changed
}

// ------------------------------------------------------------------ Typ-Icons

enum P {
    /// Linie
    L(f32, f32, f32, f32),
    /// Linienzug (offen oder durch Wiederholen des ersten Punkts geschlossen)
    Line(&'static [(f32, f32)]),
    /// Rechteck (x, y, Breite, Höhe)
    R(f32, f32, f32, f32),
    /// Kreis
    C(f32, f32, f32),
    /// Gefüllter Punkt
    D(f32, f32, f32),
    /// Ellipse (rx, ry)
    E(f32, f32),
    /// Fünfzackiger Stern
    Star,
}

fn prims(name: &str) -> &'static [P] {
    use P::*;
    match name {
        "alert" => &[L(0.0, -6.0, 0.0, 2.0), D(0.0, 5.5, 1.3)],
        "bolt" => &[Line(&[(2.0, -7.0), (-3.0, 0.5), (1.0, 0.5), (-2.0, 7.0)])],
        "bookmark" => &[Line(&[(-3.5, -6.5), (3.5, -6.5), (3.5, 6.5), (0.0, 3.5), (-3.5, 6.5), (-3.5, -6.5)])],
        "bug" => &[C(0.0, 1.5, 4.0), L(0.0, -2.5, 0.0, 5.5), L(-4.0, 0.0, -6.5, -1.5), L(4.0, 0.0, 6.5, -1.5), L(-4.0, 3.0, -6.5, 4.5), L(4.0, 3.0, 6.5, 4.5), L(-1.5, -3.0, -3.0, -6.0), L(1.5, -3.0, 3.0, -6.0)],
        "check" => &[Line(&[(-5.5, 0.5), (-2.0, 4.0), (5.5, -4.5)])],
        "checkbox" => &[R(-5.5, -5.5, 11.0, 11.0), Line(&[(-3.0, 0.0), (-1.0, 2.5), (3.0, -2.5)])],
        "clipboard" => &[R(-5.0, -5.0, 10.0, 12.0), R(-2.5, -7.0, 5.0, 3.0), L(-2.5, 0.0, 2.5, 0.0), L(-2.5, 3.0, 1.0, 3.0)],
        "clock" => &[C(0.0, 0.0, 6.0), Line(&[(0.0, -3.5), (0.0, 0.0), (3.0, 1.5)])],
        "code" => &[Line(&[(-2.5, -4.5), (-6.0, 0.0), (-2.5, 4.5)]), Line(&[(2.5, -4.5), (6.0, 0.0), (2.5, 4.5)]), L(1.0, -5.0, -1.0, 5.0)],
        "cog" => &[C(0.0, 0.0, 2.5), C(0.0, 0.0, 5.0), L(5.0, 0.0, 7.0, 0.0), L(-5.0, 0.0, -7.0, 0.0), L(0.0, 5.0, 0.0, 7.0), L(0.0, -5.0, 0.0, -7.0), L(3.5, 3.5, 5.0, 5.0), L(-3.5, -3.5, -5.0, -5.0), L(3.5, -3.5, 5.0, -5.0), L(-3.5, 3.5, -5.0, 5.0)],
        "comment" => &[Line(&[(-6.0, -5.0), (6.0, -5.0), (6.0, 3.0), (-1.0, 3.0), (-4.0, 6.5), (-4.0, 3.0), (-6.0, 3.0), (-6.0, -5.0)])],
        "compass" => &[C(0.0, 0.0, 6.5), Line(&[(-3.0, 3.0), (-1.0, -1.0), (3.0, -3.0), (1.0, 1.0), (-3.0, 3.0)])],
        "document" => &[Line(&[(-4.5, -7.0), (2.0, -7.0), (5.0, -4.0), (5.0, 7.0), (-4.5, 7.0), (-4.5, -7.0)]), L(-2.0, -1.0, 2.5, -1.0), L(-2.0, 2.0, 2.5, 2.0), L(-2.0, 5.0, 0.5, 5.0)],
        "fire" => &[Line(&[(0.0, -7.0), (4.0, -1.0), (4.0, 3.0), (2.0, 6.0), (-2.0, 6.0), (-4.0, 3.0), (-4.0, -1.0), (-1.5, 1.5), (0.0, -7.0)])],
        "flag" => &[L(-4.0, -7.0, -4.0, 7.0), Line(&[(-4.0, -6.5), (5.0, -6.5), (2.0, -3.0), (5.0, 0.5), (-4.0, 0.5)])],
        "flask" => &[Line(&[(-2.0, -6.5), (-2.0, -1.0), (-6.0, 6.0), (6.0, 6.0), (2.0, -1.0), (2.0, -6.5)]), L(-3.0, -6.5, 3.0, -6.5)],
        "folder" => &[Line(&[(-6.5, -4.5), (-2.5, -4.5), (-1.0, -2.5), (6.5, -2.5), (6.5, 5.5), (-6.5, 5.5), (-6.5, -4.5)])],
        "globe" => &[C(0.0, 0.0, 6.5), L(-6.5, 0.0, 6.5, 0.0), E(3.0, 6.5)],
        "heart" => &[Line(&[(0.0, 6.0), (-6.0, -0.5), (-6.0, -3.5), (-3.5, -5.5), (-1.0, -4.5), (0.0, -3.0), (1.0, -4.5), (3.5, -5.5), (6.0, -3.5), (6.0, -0.5), (0.0, 6.0)])],
        "key" => &[C(-3.0, -3.0, 3.0), L(-1.0, -1.0, 6.0, 6.0), L(3.5, 3.5, 5.5, 1.5), L(5.0, 5.0, 6.5, 3.5)],
        "layers" => &[Line(&[(0.0, -6.0), (6.5, -2.5), (0.0, 1.0), (-6.5, -2.5), (0.0, -6.0)]), Line(&[(-6.5, 1.0), (0.0, 4.5), (6.5, 1.0)]), Line(&[(-6.5, 4.0), (0.0, 7.0), (6.5, 4.0)])],
        "lightbulb" => &[C(0.0, -2.0, 4.5), L(-2.0, 3.5, 2.0, 3.5), L(-1.5, 5.5, 1.5, 5.5), L(-2.0, 2.5, -2.0, 3.5), L(2.0, 2.5, 2.0, 3.5)],
        "link" => &[C(-2.5, 0.0, 3.8), C(2.5, 0.0, 3.8)],
        "list" => &[L(-1.0, -4.5, 6.0, -4.5), L(-1.0, 0.0, 6.0, 0.0), L(-1.0, 4.5, 6.0, 4.5), D(-5.0, -4.5, 1.1), D(-5.0, 0.0, 1.1), D(-5.0, 4.5, 1.1)],
        "lock" => &[R(-4.5, -1.0, 9.0, 7.0), Line(&[(-2.5, -1.0), (-2.5, -4.0), (-1.0, -5.5), (1.0, -5.5), (2.5, -4.0), (2.5, -1.0)]), D(0.0, 2.5, 1.0)],
        "mail" => &[R(-6.5, -4.5, 13.0, 9.0), Line(&[(-6.5, -4.5), (0.0, 1.0), (6.5, -4.5)])],
        "package" => &[Line(&[(0.0, -7.0), (6.0, -3.5), (6.0, 3.5), (0.0, 7.0), (-6.0, 3.5), (-6.0, -3.5), (0.0, -7.0)]), Line(&[(-6.0, -3.5), (0.0, 0.0), (6.0, -3.5)]), L(0.0, 0.0, 0.0, 7.0)],
        "puzzle" => &[Line(&[(-5.0, -5.0), (-1.0, -5.0), (-1.0, -6.5), (1.0, -6.5), (1.0, -5.0), (5.0, -5.0), (5.0, -1.0), (6.5, -1.0), (6.5, 1.0), (5.0, 1.0), (5.0, 5.0), (-5.0, 5.0), (-5.0, -5.0)])],
        "rocket" => &[Line(&[(0.0, -7.0), (3.0, -2.0), (3.0, 3.0), (-3.0, 3.0), (-3.0, -2.0), (0.0, -7.0)]), Line(&[(-3.0, 0.0), (-6.0, 4.0), (-3.0, 3.0)]), Line(&[(3.0, 0.0), (6.0, 4.0), (3.0, 3.0)]), L(0.0, 3.0, 0.0, 6.5), C(0.0, -1.5, 1.3)],
        "shield" => &[Line(&[(0.0, -7.0), (6.0, -4.5), (5.5, 2.0), (0.0, 7.0), (-5.5, 2.0), (-6.0, -4.5), (0.0, -7.0)])],
        "star" => &[Star],
        "subtask" => &[L(-5.0, -6.0, -5.0, 3.0), L(-5.0, 3.0, 0.0, 3.0), R(0.5, 0.5, 6.0, 5.0), L(-5.0, -2.0, -2.0, -2.0)],
        "tag" => &[Line(&[(-6.5, -6.0), (0.0, -6.0), (6.5, 0.5), (0.5, 6.5), (-6.5, 0.0), (-6.5, -6.0)]), D(-3.5, -3.0, 1.0)],
        "target" => &[C(0.0, 0.0, 6.5), C(0.0, 0.0, 3.5), D(0.0, 0.0, 1.0)],
        "terminal" => &[R(-6.5, -5.0, 13.0, 10.0), Line(&[(-4.0, -2.0), (-1.5, 0.0), (-4.0, 2.0)]), L(0.5, 2.5, 4.0, 2.5)],
        "user" => &[C(0.0, -3.0, 3.0), Line(&[(-6.0, 6.5), (-5.0, 2.5), (-2.0, 1.0), (2.0, 1.0), (5.0, 2.5), (6.0, 6.5)])],
        "wrench" => &[L(-6.0, 6.0, 0.0, 0.0), C(2.5, -2.5, 3.5)],
        _ => &[R(-5.0, -5.0, 10.0, 10.0)],
    }
}

/// Zeichnet das benannte Typ-Icon (Name aus `planning::icons::ICONS` oder `lucide:<name>`) mittig um `c`; `size` ist die
/// Kantenlänge in Pixeln. Öffentlicher Name dafür: [`super::icon_explorer::paint_icon`].
pub fn type_icon(p: &Painter, name: &str, c: Pos2, size: f32, col: Color32) {
    if super::icon_explorer::paint_lucide(p, name, c, size, col) {
        return;
    }
    let k = size / 16.0;
    let st = Stroke::new((1.4 * k).max(1.0), col);
    let pt = |x: f32, y: f32| Pos2::new(c.x + x * k, c.y + y * k);
    for prim in prims(name) {
        match prim {
            P::L(a, b, x, y) => {
                p.line_segment([pt(*a, *b), pt(*x, *y)], st);
            }
            P::Line(pts) => {
                p.add(egui::Shape::line(pts.iter().map(|(x, y)| pt(*x, *y)).collect(), st));
            }
            P::R(x, y, w, h) => {
                p.rect_stroke(Rect::from_min_size(pt(*x, *y), Vec2::new(*w * k, *h * k)), 1.0, st, StrokeKind::Inside);
            }
            P::C(x, y, r) => {
                p.circle_stroke(pt(*x, *y), *r * k, st);
            }
            P::D(x, y, r) => {
                p.circle_filled(pt(*x, *y), *r * k, col);
            }
            P::E(rx, ry) => {
                let pts: Vec<Pos2> = (0..=24).map(|i| i as f32 / 24.0 * std::f32::consts::TAU).map(|a| pt(rx * a.cos(), ry * a.sin())).collect();
                p.add(egui::Shape::line(pts, st));
            }
            P::Star => {
                let pts: Vec<Pos2> = (0..=10)
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                        let r = if i % 2 == 0 { 7.0 } else { 3.0 };
                        pt(r * a.cos(), r * a.sin())
                    })
                    .collect();
                p.add(egui::Shape::line(pts, st));
            }
        }
    }
}

/// Kleines Typ-Symbol in Farbe: Icon im Quadrat oder, ohne Icon, ein gefüllter Punkt.
pub fn type_badge(ui: &mut Ui, icon: Option<&str>, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_type_badge(ui.painter(), rect, icon, color);
}

pub fn paint_type_badge(p: &Painter, rect: Rect, icon: Option<&str>, color: Color32) {
    p.rect_filled(rect, 5.0, color.gamma_multiply(0.22));
    match icon {
        Some(i) => type_icon(p, i, rect.center(), rect.width() * 0.72, color),
        None => {
            p.circle_filled(rect.center(), rect.width() * 0.2, color);
        }
    }
}

/// Icon-Picker: Raster aller eingebauten Icons (und „Kein Icon“) plus „Weitere Icons …“, das den Icon-Explorer mit den
/// Lucide-Icons öffnet ([`super::icon_explorer`]). Ein gewähltes Lucide-Icon (`lucide:<name>`) steht als zusätzliche, markierte
/// Zelle im Raster. `salt` muss je Picker eindeutig sein; `true` bei Änderung (auch wenn im Explorer gewählt wurde).
pub fn icon_picker(ui: &mut Ui, salt: impl Hash, current: &mut Option<String>, color: Color32, lang: Lang) -> bool {
    icon_picker_impl(ui, Id::new(salt), current, color, lang, true)
}

/// Wie [`icon_picker`], aber ein Icon ist Pflicht (ohne „Kein Icon“); `current` ist der gespeicherte Name.
#[allow(dead_code)] // für Picker mit Pflicht-Icon (Fälligkeits-Markierungen, #188)
pub fn icon_picker_required(ui: &mut Ui, salt: impl Hash, current: &mut String, color: Color32, lang: Lang) -> bool {
    let mut cur = Some(std::mem::take(current));
    let changed = icon_picker_impl(ui, Id::new(salt), &mut cur, color, lang, false);
    *current = cur.unwrap_or_default();
    changed
}

fn icon_picker_impl(ui: &mut Ui, salt: Id, current: &mut Option<String>, color: Color32, lang: Lang, allow_none: bool) -> bool {
    let mut changed = false;
    let explorer = salt.with("icon_explorer");
    let dim = ui.visuals().text_color().gamma_multiply(0.85);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(3.0);
        ui.set_max_width(12.0 * 31.0);
        if allow_none {
            // „Kein Icon“
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
            let resp = ui.interact(rect, salt.with("none"), Sense::click());
            let sel = current.is_none();
            paint_pick_cell(ui, rect, sel, resp.hovered());
            let c = ui.visuals().weak_text_color();
            ui.painter().line_segment([rect.center() + Vec2::new(-5.0, 5.0), rect.center() + Vec2::new(5.0, -5.0)], Stroke::new(1.3_f32, c));
            ui.painter().circle_stroke(rect.center(), 6.0, Stroke::new(1.3_f32, c));
            if resp.on_hover_text(lang.t("Kein Icon", "No icon")).clicked() && !sel {
                *current = None;
                changed = true;
            }
        }
        for name in ICONS {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
            let resp = ui.interact(rect, salt.with(*name), Sense::click());
            let sel = current.as_deref() == Some(*name);
            paint_pick_cell(ui, rect, sel, resp.hovered());
            type_icon(ui.painter(), name, rect.center(), 18.0, if sel { color } else { dim });
            if resp.on_hover_text(*name).clicked() && !sel {
                *current = Some((*name).to_string());
                changed = true;
            }
        }
        // Gewähltes Icon, das nicht zum eingebauten Satz gehört (Lucide oder unbekannt): als markierte Zelle dazu.
        if let Some(cur) = current.as_deref().filter(|c| !ICONS.contains(c)) {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
            let resp = ui.interact(rect, salt.with("current_extra"), Sense::click());
            paint_pick_cell(ui, rect, true, resp.hovered());
            super::icon_explorer::paint_icon(ui.painter(), cur, rect.center(), 18.0, color);
            if resp.on_hover_text(super::icon_explorer::icon_label(cur)).clicked() {
                super::icon_explorer::open_picker_explorer(ui.ctx(), explorer, Some(cur));
            }
        }
    });
    ui.add_space(3.0);
    let open = super::icon_explorer::explorer_open(ui.ctx(), explorer);
    let more = ui.add_enabled(!open, egui::Button::new(lang.t("Weitere Icons …", "More icons …")).min_size(Vec2::new(0.0, 24.0)));
    let more = more
        .on_hover_text(lang.t("Icon-Explorer mit über 2.000 Lucide-Icons öffnen", "Open the icon explorer with more than 2,000 Lucide icons"))
        .on_disabled_hover_text(lang.t("Der Icon-Explorer ist schon offen.", "The icon explorer is already open."));
    if more.clicked() {
        super::icon_explorer::open_picker_explorer(ui.ctx(), explorer, current.as_deref());
    }
    if let Some(v) = super::icon_explorer::picker_explorer(ui.ctx(), explorer, lang) {
        if current.as_deref() != Some(v.as_str()) {
            *current = Some(v);
            changed = true;
        }
    }
    changed
}

fn paint_pick_cell(ui: &Ui, rect: Rect, selected: bool, hovered: bool) {
    let a = style::accent();
    if selected {
        ui.painter().rect_filled(rect, 6.0, a.gamma_multiply(0.25));
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.3_f32, a), StrokeKind::Inside);
    } else if hovered {
        ui.painter().rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
    }
}

// ------------------------------------------------------------------ Schloss

/// Kleines Schloss (schreibgeschützt, System-Eintrag).
pub fn paint_lock(p: &Painter, c: Pos2, col: Color32) {
    type_icon(p, "lock", c, 14.0, col);
}

/// Schloss mit Hinweistext bei Mausberührung.
pub fn lock_icon(ui: &mut Ui, tip: &str) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(16.0, 18.0), Sense::hover());
    paint_lock(ui.painter(), rect.center(), style::dim(ui));
    resp.on_hover_text(tip);
}

/// Kopfzeile eines Dialogs mit Titel; schmaler Abstand darunter.
pub fn dialog_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).heading());
    ui.add_space(S2);
}

/// Breite der Füllung eines Balkens: bei 0 % gar keine (kein Knopf, der den Anfang der Beschriftung überdeckt), sonst anteilig.
pub fn bar_fill_width(total: f32, fraction: f32) -> f32 {
    if !fraction.is_finite() || fraction <= 0.0 || total <= 0.0 {
        return 0.0;
    }
    total * fraction.min(1.0)
}

/// Fortschrittsbalken mit Beschriftung: die Füllung (Akzentfarbe, abgeschwächt) liegt unter dem Text, der Text steht immer
/// ungekürzt und gut lesbar links darüber – auch bei 0 %.
pub fn labeled_bar(ui: &mut Ui, fraction: f32, text: &str, max_width: f32) -> Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, ui.visuals().text_color());
    let width = max_width.min(ui.available_width()).max(galley.size().x + 16.0).min(ui.available_width().max(1.0));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, (galley.size().y + 6.0).max(18.0)), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, ui.visuals().widgets.inactive.bg_fill);
    let w = bar_fill_width(rect.width(), fraction);
    if w > 0.0 {
        p.rect_filled(Rect::from_min_size(rect.min, Vec2::new(w, rect.height())), 4.0, style::accent().gamma_multiply(0.55));
    }
    p.with_clip_rect(rect).galley(Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, ui.visuals().text_color());
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_fill_has_no_width_at_zero_percent() {
        assert_eq!(bar_fill_width(200.0, 0.0), 0.0);
        assert_eq!(bar_fill_width(200.0, f32::NAN), 0.0);
        assert_eq!(bar_fill_width(200.0, 0.5), 100.0);
        assert_eq!(bar_fill_width(200.0, 3.0), 200.0);
    }

    #[test]
    fn every_icon_name_has_a_drawing() {
        // Der Rückfall (Quadrat) darf nur für unbekannte Namen gelten: jede benannte Zeichnung hat mindestens ein Teil.
        for n in ICONS {
            assert!(!prims(n).is_empty(), "{n}");
            let fallback = matches!(prims(n), [P::R(a, b, c, d)] if (*a, *b, *c, *d) == (-5.0, -5.0, 10.0, 10.0));
            assert!(!fallback, "{n} hat keine eigene Zeichnung");
        }
    }

    #[test]
    fn hex_input_accepts_with_and_without_hash() {
        assert_eq!(hex_input("d97757").as_deref(), Some("#D97757"));
        assert_eq!(hex_input(" #d97757 ").as_deref(), Some("#D97757"));
        assert_eq!(hex_input("#d9775"), None);
        assert_eq!(hex_input(""), None);
        assert_eq!(hex_input("zzzzzz"), None);
    }
}
