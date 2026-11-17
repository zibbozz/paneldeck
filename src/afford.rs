//! Affordanz (#187): Alles, was auf einen Klick reagiert, sieht klickbar aus und zeigt den Hand-Zeiger.
//!
//! - [`install`] hängt einen Nachlauf an das Ende jedes Durchgangs (in jedem Fenster): Liegt der Zeiger auf einem
//!   aktivierten Element, das nur Klicks annimmt (egui-Buttons, auswählbare Einträge, Kontrollkästchen, Auswahllisten,
//!   aufklappbare Überschriften, Menüeinträge, eigene `Sense::click()`-Flächen …), und hat in diesem Bild niemand einen
//!   anderen Zeiger gesetzt (Textfeld, markierbarer Text, Größe ändern, Ziehen), wird der Zeiger zur Hand. Neue
//!   Oberfläche bekommt das damit von selbst.
//! - Elemente, die auch Ziehen annehmen (Sitzungszeilen, Karten), und klickbarer markierbarer Text setzen den Zeiger
//!   selbst: [`Clickable::pointer`] bzw. [`Clickable::clickable`] (Zeiger + Tooltip).
//! - Große Klickflächen, die kein Knopf sind (Hintergrund eines Chats, Absätze mit Verweisen, Kopfzeile eines Bereichs,
//!   Fensterknöpfe), nehmen sich mit [`Clickable::no_pointer`] aus.
//! - Gemeinsamer Hover-Stil: [`hover_fill`]; Bausteine: [`choice`]/[`choice_value`] (Auswahl als echte Knöpfe),
//!   [`link_button`] (versteckte Aktion als kleiner Knopf), [`swatch`] (Farbfeld).

use crate::style;
use eframe::egui::{self, Color32, Context, CursorIcon, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, ViewportId, WidgetText};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

thread_local! {
    /// Ausgenommene Elemente je Fenster für den laufenden Durchgang (Nummer des Durchgangs, Ids). Nicht in
    /// `Memory::data`: weitere Fenster tauschen diese Daten (`multiwin`) und hätten sie am Ende des Durchgangs nicht mehr.
    static NO_POINTER: RefCell<HashMap<ViewportId, (u64, HashSet<Id>)>> = RefCell::new(HashMap::new());
}

/// Meldet den Nachlauf am Kontext an (einmal beim Start; gilt für alle Fenster).
pub fn install(ctx: &Context) {
    ctx.on_end_pass("paneldeck_afford", Arc::new(end_pass));
}

/// Nimmt nur Klicks an (kein Ziehen) und ist kein Hintergrund eines Fensters/Popups: wie ein Knopf.
fn button_like(w: &egui::WidgetRect) -> bool {
    w.enabled
        && w.sense.senses_click()
        && !w.sense.senses_drag()
        // Hintergrund nicht verschiebbarer Bereiche (Popups, Menüs, verankerte Fenster) und Titelleiste von `egui::Window`.
        && w.id != w.layer_id.id.with("move")
        && w.id != w.layer_id.id.with("__window_title_bar")
}

fn end_pass(ctx: &Context) {
    if ctx.output(|o| o.cursor_icon) != CursorIcon::Default {
        return; // Jemand hat schon entschieden (Textfeld, Ziehen, Größe ändern, Verweis im Terminal …).
    }
    let (vp, pass) = (ctx.viewport_id(), ctx.cumulative_pass_nr());
    let skip: HashSet<Id> = NO_POINTER.with(|m| m.borrow().get(&vp).filter(|(p, _)| *p == pass).map(|(_, s)| s.clone()).unwrap_or_default());
    let hand = ctx.viewport(|v| {
        v.interact_widgets.hovered.iter().any(|id| {
            !skip.contains(id) && v.this_pass.widgets.get(*id).or_else(|| v.prev_pass.widgets.get(*id)).is_some_and(button_like)
        })
    });
    if hand {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
}

/// Erweiterung für [`Response`]: Zeiger, Tooltip, Ausnahme.
pub trait Clickable: Sized {
    /// Hand-Zeiger beim Überfahren (nur aktiviert).
    fn pointer(self) -> Self;
    /// Hand-Zeiger und Tooltip – für alles, dessen Aktion man nicht sieht.
    fn clickable(self, tip: impl Into<WidgetText>) -> Self;
    /// Keine Hand für diese Fläche (Hintergründe, die nur fokussieren o. Ä.).
    fn no_pointer(self) -> Self;
}

impl Clickable for Response {
    fn pointer(self) -> Self {
        if self.enabled() && self.hovered() {
            self.ctx.set_cursor_icon(CursorIcon::PointingHand);
        }
        self
    }

    fn clickable(self, tip: impl Into<WidgetText>) -> Self {
        self.pointer().on_hover_text(tip)
    }

    fn no_pointer(self) -> Self {
        let (vp, pass) = (self.ctx.viewport_id(), self.ctx.cumulative_pass_nr());
        NO_POINTER.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.entry(vp).or_insert_with(|| (pass, HashSet::new()));
            if e.0 != pass {
                *e = (pass, HashSet::new());
            }
            e.1.insert(self.id);
        });
        self
    }
}

/// Gemeinsamer Hover-Stil eigener Flächen: weich eingeblendete Hover-Farbe des Themas. Gibt den Hover-Anteil (0..1) zurück.
pub fn hover_fill(ui: &Ui, rect: Rect, resp: &Response, radius: f32) -> f32 {
    let h = style::anim(ui.ctx(), resp.id.with("afford_h"), resp.enabled() && resp.hovered(), style::D_FAST);
    if h > 0.0 {
        ui.painter().rect_filled(rect, radius, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(h));
    }
    h
}

/// Eine Wahl aus mehreren als echter Knopf: auch nicht gewählt mit Rahmen und Fläche (wie „Eintragen“/„Entfernen“),
/// gewählt in der Auswahlfarbe. Ersatz für `selectable_label` in Reihen nebeneinander (Stufen, Sprache, Modus).
/// `min_w`: Mindestbreite (0 = nach Text), damit untereinanderstehende Knöpfe gleich breit sind.
pub fn choice(ui: &mut Ui, selected: bool, text: impl Into<WidgetText>, min_w: f32) -> Response {
    ui.add(egui::Button::new(text).selected(selected).frame(true).frame_when_inactive(true).min_size(Vec2::new(min_w, 0.0)))
}

/// Wie `selectable_value`, aber mit [`choice`]: setzt `cur` bei Klick und meldet die Änderung.
pub fn choice_value<V: PartialEq>(ui: &mut Ui, cur: &mut V, value: V, text: impl Into<WidgetText>, min_w: f32) -> Response {
    let mut resp = choice(ui, *cur == value, text, min_w);
    if resp.clicked() && *cur != value {
        *cur = value;
        resp.mark_changed();
    }
    resp
}

/// Versteckte Aktion als kleiner Knopf (z. B. Kostenzeile in der Seitenleiste): gedämpfter Text mit feinem Rahmen und
/// Pfeil „›“, beim Überfahren Hover-Fläche, kräftiger Text, Hand und Tooltip.
pub fn link_button(ui: &mut Ui, text: &str, color: Color32, tip: &str) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(11.5), Color32::PLACEHOLDER);
    let size = Vec2::new(link_button_width(ui, text), 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let h = hover_fill(ui, rect, &resp, 5.0);
        let v = ui.visuals();
        let border = v.widgets.inactive.bg_stroke.color;
        ui.painter().rect_stroke(rect, 5.0, Stroke::new(1.0_f32, style::mix(border.gamma_multiply(0.6), v.widgets.hovered.bg_stroke.color, h)), StrokeKind::Inside);
        let tc = style::mix(color, v.strong_text_color(), 0.6 * h); // Farbe bleibt erkennbar (Status-Chip)
        ui.painter().galley(Pos2::new(rect.min.x + 8.0, rect.center().y - g.size().y / 2.0), g.clone(), tc);
        // Pfeil „›“: öffnet etwas
        let c = Pos2::new(rect.max.x - 9.0, rect.center().y);
        let st = Stroke::new(1.3_f32, tc);
        ui.painter().line_segment([c + Vec2::new(-1.5, -3.0), c + Vec2::new(1.5, 0.0)], st);
        ui.painter().line_segment([c + Vec2::new(1.5, 0.0), c + Vec2::new(-1.5, 3.0)], st);
    }
    resp.clickable(tip)
}

/// Breite, die [`link_button`] für `text` braucht (zum Kürzen, wenn der Platz nicht reicht).
pub fn link_button_width(ui: &Ui, text: &str) -> f32 {
    let g = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(11.5), Color32::PLACEHOLDER);
    8.0 + g.size().x + 6.0 + 5.0 + 7.0
}

/// Farbfeld (18 px) zum Anklicken: beim Überfahren ein Ring und die Hand, gewählt mit kräftigem Rahmen.
pub fn swatch(ui: &mut Ui, color: Color32, selected: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    let h = style::anim(ui.ctx(), resp.id.with("afford_h"), resp.hovered(), style::D_FAST);
    ui.painter().rect_filled(r, 4.0, color);
    if selected {
        ui.painter().rect_stroke(r.expand(2.0), 5.0, Stroke::new(1.5_f32, ui.visuals().strong_text_color()), StrokeKind::Outside);
    } else if h > 0.0 {
        ui.painter().rect_stroke(r.expand(2.0), 5.0, Stroke::new(1.5_f32, ui.visuals().widgets.hovered.bg_stroke.color.gamma_multiply(h)), StrokeKind::Outside);
    }
    resp.pointer()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    /// Ein Bild mit Zeiger an `p`; liefert den Zeiger am Ende des Bilds.
    pub fn frame_at(ctx: &Context, t: &mut f64, p: Pos2, ui_fn: &mut dyn FnMut(&mut Ui)) -> egui::FullOutput {
        frame_with(ctx, t, vec![Event::PointerMoved(p)], ui_fn)
    }

    /// Ein Bild mit beliebigen Ereignissen.
    pub fn frame_with(ctx: &Context, t: &mut f64, events: Vec<Event>, ui_fn: &mut dyn FnMut(&mut Ui)) -> egui::FullOutput {
        *t += 0.05;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(*t), events, ..Default::default() };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| ui_fn(ui));
        })
    }

    /// Zeiger, wenn die Maus (über mehrere Bilder) an `p` steht.
    pub fn cursor_at(p: Pos2, ui_fn: &mut dyn FnMut(&mut Ui)) -> CursorIcon {
        let ctx = Context::default();
        install(&ctx);
        let mut t = 0.0;
        let mut out = frame_at(&ctx, &mut t, p, ui_fn);
        for _ in 0..3 {
            out = frame_at(&ctx, &mut t, p, ui_fn);
        }
        out.platform_output.cursor_icon
    }

    /// Rechteck eines Elements (erstes Bild ohne Zeiger).
    pub fn rect_of(ui_fn: &mut dyn FnMut(&mut Ui) -> Rect) -> Rect {
        let ctx = Context::default();
        let mut r = Rect::NOTHING;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(0.0), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| r = ui_fn(ui));
        });
        r
    }

    /// Hat das Element an `p` nach dem Überfahren einen Tooltip? (Tooltips erscheinen erst nach kurzer Ruhe.)
    pub fn tooltip_at(p: Pos2, ui_fn: &mut dyn FnMut(&mut Ui)) -> bool {
        let ctx = Context::default();
        install(&ctx);
        let mut t = 0.0;
        frame_at(&ctx, &mut t, p, ui_fn);
        for _ in 0..30 {
            frame_with(&ctx, &mut t, vec![], ui_fn); // Zeiger ruht
        }
        // Ein Tooltip ist ein eigener Bereich (Ebene „Tooltip“) über dem Inhalt.
        ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.order == egui::Order::Tooltip))
    }

    #[test]
    fn plain_buttons_selectable_labels_and_click_areas_get_the_hand() {
        let center = |f: &mut dyn FnMut(&mut Ui) -> Rect| rect_of(f).center();
        // egui-Button
        let p = center(&mut |ui| ui.button("Register").rect);
        assert_eq!(cursor_at(p, &mut |ui| { let _ = ui.button("Register"); }), CursorIcon::PointingHand);
        // auswählbarer Eintrag (nicht gewählt: ohne Rahmen)
        let p = center(&mut |ui| ui.selectable_label(false, "Off").rect);
        assert_eq!(cursor_at(p, &mut |ui| { let _ = ui.selectable_label(false, "Off"); }), CursorIcon::PointingHand);
        // Kontrollkästchen
        let p = center(&mut |ui| ui.checkbox(&mut false, "x").rect);
        assert_eq!(cursor_at(p, &mut |ui| { ui.checkbox(&mut false, "x"); }), CursorIcon::PointingHand);
        // eigene Klickfläche
        let p = center(&mut |ui| ui.allocate_exact_size(Vec2::splat(30.0), Sense::click()).0);
        assert_eq!(cursor_at(p, &mut |ui| { ui.allocate_exact_size(Vec2::splat(30.0), Sense::click()); }), CursorIcon::PointingHand);
    }

    #[test]
    fn text_fields_drag_areas_disabled_and_opted_out_areas_keep_their_cursor() {
        let p = rect_of(&mut |ui| ui.text_edit_singleline(&mut String::from("abc")).rect).center();
        assert_eq!(cursor_at(p, &mut |ui| { ui.text_edit_singleline(&mut String::from("abc")); }), CursorIcon::Text, "Textfeld bleibt I-Balken");
        let p = rect_of(&mut |ui| ui.allocate_exact_size(Vec2::splat(30.0), Sense::click_and_drag()).0).center();
        assert_eq!(cursor_at(p, &mut |ui| { ui.allocate_exact_size(Vec2::splat(30.0), Sense::click_and_drag()); }), CursorIcon::Default, "Zieh-Flächen entscheiden selbst");
        let p = rect_of(&mut |ui| ui.add_enabled(false, egui::Button::new("x")).rect).center();
        assert_eq!(cursor_at(p, &mut |ui| { ui.add_enabled(false, egui::Button::new("x")); }), CursorIcon::Default, "deaktiviert: keine Hand");
        let p = rect_of(&mut |ui| ui.allocate_exact_size(Vec2::splat(30.0), Sense::click()).0).center();
        let c = cursor_at(p, &mut |ui| {
            let (_, r) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
            r.no_pointer();
        });
        assert_eq!(c, CursorIcon::Default, "ausgenommene Fläche");
        // Kein Element unter dem Zeiger
        assert_eq!(cursor_at(Pos2::new(700.0, 500.0), &mut |ui| { let _ = ui.button("x"); }), CursorIcon::Default);
    }

    #[test]
    fn explicit_pointer_wins_over_selectable_text_and_drag_areas() {
        // Klickbarer, markierbarer Text: egui setzt den I-Balken, `clickable` macht daraus die Hand.
        let p = rect_of(&mut |ui| ui.add(egui::Label::new("costs").sense(Sense::click())).rect).center();
        let plain = cursor_at(p, &mut |ui| { ui.add(egui::Label::new("costs").sense(Sense::click())); });
        assert_eq!(plain, CursorIcon::Text, "Vorbedingung: markierbarer Text zeigt den I-Balken");
        let c = cursor_at(p, &mut |ui| { ui.add(egui::Label::new("costs").sense(Sense::click())).clickable("Open"); });
        assert_eq!(c, CursorIcon::PointingHand);
        let p = rect_of(&mut |ui| ui.allocate_exact_size(Vec2::splat(30.0), Sense::click_and_drag()).0).center();
        let c = cursor_at(p, &mut |ui| { ui.allocate_exact_size(Vec2::splat(30.0), Sense::click_and_drag()).1.pointer(); });
        assert_eq!(c, CursorIcon::PointingHand);
    }

    #[test]
    fn window_backgrounds_and_title_bars_do_not_get_the_hand() {
        let draw = |ctx: &Context| {
            egui::Window::new("Dialog").fixed_pos(Pos2::new(100.0, 100.0)).fixed_size(Vec2::new(200.0, 120.0)).collapsible(false).show(ctx, |ui| {
                ui.label("text");
            });
        };
        for p in [Pos2::new(200.0, 112.0), Pos2::new(250.0, 200.0)] {
            let ctx = Context::default();
            install(&ctx);
            let mut out = None;
            for i in 0..4 {
                let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(i as f64 * 0.05), events: vec![Event::PointerMoved(p)], ..Default::default() };
                out = Some(ctx.run(input, |ctx| draw(ctx)));
            }
            assert_ne!(out.unwrap().platform_output.cursor_icon, CursorIcon::PointingHand, "Fenster an {p:?}");
        }
    }

    #[test]
    fn choice_buttons_look_like_buttons_and_switch_the_value() {
        let ctx = Context::default();
        install(&ctx);
        let mut cur = 0u8;
        let mut rects = [Rect::NOTHING; 3];
        let draw = |ctx: &Context, cur: &mut u8, rects: &mut [Rect; 3], events: Vec<Event>, t: f64| {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(t), events, ..Default::default() };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        for i in 0..3u8 {
                            rects[i as usize] = choice_value(ui, cur, i, ["Off", "Read only", "Control"][i as usize], 0.0).rect;
                        }
                    });
                });
            })
        };
        draw(&ctx, &mut cur, &mut rects, vec![], 0.0);
        let p = rects[2].center();
        draw(&ctx, &mut cur, &mut rects, vec![Event::PointerMoved(p)], 0.1);
        let out = draw(&ctx, &mut cur, &mut rects, vec![Event::PointerMoved(p)], 0.2);
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::PointingHand);
        let ev = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        draw(&ctx, &mut cur, &mut rects, vec![ev(true)], 0.3);
        draw(&ctx, &mut cur, &mut rects, vec![ev(false)], 0.35);
        assert_eq!(cur, 2);
    }

    #[test]
    fn link_button_shows_hand_and_tooltip() {
        let p = rect_of(&mut |ui| link_button(ui, "Σ 12k tokens · ≈ $0.40", Color32::GRAY, "Open cost overview").rect).center();
        let mut f = |ui: &mut Ui| {
            link_button(ui, "Σ 12k tokens · ≈ $0.40", Color32::GRAY, "Open cost overview");
        };
        assert_eq!(cursor_at(p, &mut f), CursorIcon::PointingHand);
        assert!(tooltip_at(p, &mut f), "Tooltip erscheint");
        assert!(!tooltip_at(Pos2::new(700.0, 500.0), &mut f), "ohne Überfahren kein Tooltip");
    }

    #[test]
    fn swatch_shows_hand() {
        let p = rect_of(&mut |ui| swatch(ui, Color32::RED, false).rect).center();
        assert_eq!(cursor_at(p, &mut |ui| { swatch(ui, Color32::RED, false); }), CursorIcon::PointingHand);
    }
}
