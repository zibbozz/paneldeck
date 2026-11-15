//! Kleines, wiederverwendbares Toast-System (Issue #50): kurze Meldungen unten rechts, bis zu
//! fünf übereinander, jede 10 Sekunden sichtbar.
//!
//! Aufbau: `ToastQueue` ist reine Logik (Stapel, Ablauf, Verdrängung, Pausieren) mit injizierter
//! Zeit in Sekunden und kennt kein egui – dadurch unit-testbar. `App::toasts_ui` zeichnet.
//! Benutzung aus der App: `self.toast("Text")` oder `self.push_toast(text, ToastKind::Warning, Some(ziel))`.
//! Die bestehende Einzelmeldung `notify_text`/`notice` (unten mittig) bleibt unberührt.

// Die Schnittstelle wird von den Auslösern (Agenten-Änderungen, #50) erst nach und nach genutzt.
#![allow(dead_code)]

use super::App;
use crate::style;
use eframe::egui::{self, Align2, Color32, CornerRadius, Id, Margin, RichText, Sense, Stroke, Vec2};
use std::time::Duration;

/// Höchstzahl gleichzeitig sichtbarer Toasts; ein weiterer verdrängt den ältesten.
pub const MAX_TOASTS: usize = 5;
/// Sichtbarkeitsdauer in Sekunden (bei Mauszeiger darüber angehalten).
pub const LIFETIME: f64 = 10.0;
/// Dauer des Ausblendens in Sekunden (bei abgeschalteten Animationen 0).
pub const FADE_OUT: f64 = 0.4;
/// Dauer des Einblendens in Sekunden.
pub const FADE_IN: f64 = 0.18;

/// Art der Meldung; bestimmt nur die Farbe des Akzentstreifens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Info,
    Warning,
    Error,
}

impl ToastKind {
    fn color(self) -> Color32 {
        match self {
            ToastKind::Info => style::ACCENT,
            ToastKind::Warning => Color32::from_rgb(0xe5, 0xc0, 0x7b),
            ToastKind::Error => Color32::from_rgb(0xe0, 0x4f, 0x5f),
        }
    }
}

/// Ein Knopf im Toast; `token` ordnet den Klick dem Auslöser zu.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastAction {
    pub label: String,
    pub token: u64,
    /// Paneldeck-Fenster, dessen Ansicht den Knopf ausgelöst hat (0 = Hauptfenster, #110): der Klick wird dorthin gereicht.
    pub window: u64,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub kind: ToastKind,
    /// Zielhinweis (z. B. Sitzungs- oder Ticket-Kennung) als Text. Hook für später:
    /// Klick darauf soll einmal dorthin springen; heute wird er nur angezeigt.
    pub target: Option<String>,
    /// Knopf unter dem Text (z. B. „Rückgängig“); Klick meldet `token` an die Anwendung.
    pub action: Option<ToastAction>,
    /// Zeitpunkt des Erscheinens (Sekunden der injizierten Uhr).
    born: f64,
    /// Bisher angezeigte Zeit ohne Pausen.
    age: f64,
    /// Beginn des Ausblendens; danach wird der Toast entfernt.
    closing: Option<f64>,
}

impl Toast {
    pub fn is_closing(&self) -> bool {
        self.closing.is_some()
    }
}

/// Stapel der Toasts; älteste zuerst. Zeit wird immer von außen übergeben.
pub struct ToastQueue {
    items: Vec<Toast>,
    next_id: u64,
    last_tick: f64,
    fade_out: f64,
}

impl Default for ToastQueue {
    fn default() -> Self {
        Self { items: Vec::new(), next_id: 1, last_tick: 0.0, fade_out: FADE_OUT }
    }
}

impl ToastQueue {
    /// Ausblenddauer setzen (0 = sofort entfernen, z. B. ohne Animationen).
    pub fn set_fade_out(&mut self, secs: f64) {
        self.fade_out = secs.max(0.0);
    }

    /// Neuen Toast ganz unten einreihen. Sind schon `MAX_TOASTS` aktiv, wird der älteste verdrängt.
    pub fn push(&mut self, now: f64, text: String, kind: ToastKind, target: Option<String>) -> u64 {
        self.push_with_action(now, text, kind, target, None)
    }

    /// Wie [`ToastQueue::push`], mit einem Knopf (z. B. „Rückgängig“).
    pub fn push_with_action(&mut self, now: f64, text: String, kind: ToastKind, target: Option<String>, action: Option<ToastAction>) -> u64 {
        // Zeit nachziehen, damit die Pause-Rechnung nicht rückwirkend auf den neuen Toast fällt.
        self.last_tick = self.last_tick.max(now);
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Toast { id, text, kind, target, action, born: now, age: 0.0, closing: None });
        while self.active_len() > MAX_TOASTS {
            if let Some(old) = self.items.iter_mut().find(|t| t.closing.is_none()) {
                old.closing = Some(now);
            }
        }
        self.purge(now);
        id
    }

    /// Zeit fortschreiben: Alter aller nicht pausierten Toasts erhöhen, Abgelaufene ausblenden,
    /// Ausgeblendete entfernen. `hovered` = Toast unter dem Mauszeiger (dessen Ablauf ruht).
    pub fn tick(&mut self, now: f64, hovered: Option<u64>) {
        let dt = (now - self.last_tick).max(0.0);
        self.last_tick = self.last_tick.max(now);
        for t in self.items.iter_mut() {
            if t.closing.is_some() {
                continue;
            }
            if hovered != Some(t.id) {
                t.age += dt;
            }
            if t.age >= LIFETIME {
                t.closing = Some(now);
            }
        }
        self.purge(now);
    }

    /// Einen Toast sofort (mit kurzem Ausblenden) schließen, z. B. per Klick.
    pub fn dismiss(&mut self, now: f64, id: u64) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id && t.closing.is_none()) {
            t.closing = Some(now);
        }
        self.purge(now);
    }

    fn purge(&mut self, now: f64) {
        let fade = self.fade_out;
        self.items.retain(|t| match t.closing {
            Some(c) => now - c < fade,
            None => true,
        });
    }

    /// Alle sichtbaren Toasts (ältester zuerst), auch solche, die gerade ausblenden.
    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// Zahl der Toasts, die nicht ausblenden.
    pub fn active_len(&self) -> usize {
        self.items.iter().filter(|t| t.closing.is_none()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Deckkraft 0..1: Einblenden nach dem Erscheinen, Ausblenden nach dem Schließen.
    pub fn alpha(&self, t: &Toast, now: f64) -> f32 {
        let fin = ((now - t.born) / FADE_IN).clamp(0.0, 1.0);
        let fout = match t.closing {
            Some(c) if self.fade_out > 0.0 => 1.0 - ((now - c) / self.fade_out).clamp(0.0, 1.0),
            Some(_) => 0.0,
            None => 1.0,
        };
        (fin.min(fout)) as f32
    }
}

const WIDTH_MAX: f32 = 340.0;
const WIDTH_MIN: f32 = 200.0;
const EDGE: f32 = 16.0;
const GAP: f32 = 8.0;
const STRIP: f32 = 4.0;
const RADIUS: u8 = 8;

impl App {
    /// Kurzen Info-Toast anzeigen (unten rechts, 10 s, Klick schließt).
    pub(super) fn toast(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), ToastKind::Info, None);
    }

    /// Toast mit Art und optionalem Zielhinweis (z. B. Sitzungs-/Ticket-Kennung) anzeigen.
    pub(super) fn push_toast(&mut self, text: String, kind: ToastKind, target: Option<String>) {
        self.push_toast_action(text, kind, target, None);
    }

    /// Toast mit Knopf (z. B. „Rückgängig“); ein Klick darauf landet in `toast_clicked` (siehe `toasts_ui`).
    pub(super) fn push_toast_action(&mut self, text: String, kind: ToastKind, target: Option<String>, action: Option<ToastAction>) {
        let now = self.toasts_epoch.elapsed().as_secs_f64();
        self.toasts.set_fade_out(if self.settings.animations { FADE_OUT } else { 0.0 });
        self.toasts.push_with_action(now, text, kind, target, action);
    }

    /// Toasts zeichnen (am Ende von `update`): rechts unten, neueste ganz unten, ältere darüber.
    pub(super) fn toasts_ui(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let now = self.toasts_epoch.elapsed().as_secs_f64();
        let animate = self.settings.animations;
        self.toasts.set_fade_out(if animate { FADE_OUT } else { 0.0 });
        let hovered = self.toasts_hover;
        self.toasts.tick(now, hovered);
        if self.toasts.is_empty() {
            self.toasts_hover = None;
            return;
        }
        let lang = self.settings.language;
        let tip = lang.t("Zum Schließen klicken", "Click to dismiss");
        let pointer = ctx.input(|i| i.pointer.hover_pos());

        // Momentaufnahme (neueste zuerst = unterste Position; die Höhe wird je Toast beim Zeichnen gemessen), damit beim Zeichnen nichts geliehen bleibt.
        let snap: Vec<(u64, String, ToastKind, Option<String>, Option<ToastAction>, f32)> = self
            .toasts
            .items()
            .iter()
            .rev()
            .map(|t| (t.id, t.text.clone(), t.kind, t.target.clone(), t.action.clone(), self.toasts.alpha(t, now)))
            .collect();

        let mut clicked: Option<u64> = None;
        let mut action_token: Option<(u64, u64)> = None;
        let mut new_hover: Option<u64> = None;
        let mut offset = EDGE;
        for (id, text, kind, target, action, alpha) in snap {
            // Stapelposition weich nachführen (rückt auf, wenn darunter einer verschwindet).
            let y = if animate { ctx.animate_value_with_time(Id::new(("toast-y", id)), offset, style::D_BASE) } else { offset };
            let slide = if animate { (1.0 - alpha) * 14.0 } else { 0.0 };
            let area = egui::Area::new(Id::new(("toast", id)))
                .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-EDGE + slide, -y))
                .order(egui::Order::Tooltip)
                .interactable(true);
            let out = area.show(ctx, |ui| {
                ui.set_opacity(alpha);
                let vis = ui.visuals().clone();
                let fill = vis.window_fill.gamma_multiply(0.94);
                let frame = egui::Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0_f32, vis.window_stroke.color))
                    .corner_radius(CornerRadius::same(RADIUS))
                    .shadow(vis.popup_shadow)
                    .inner_margin(Margin { left: 16, right: 12, top: 8, bottom: 8 });
                let mut action_hit = false;
                let mut button_top: Option<f32> = None;
                let r = frame.show(ui, |ui| {
                    ui.set_min_width(WIDTH_MIN);
                    ui.set_max_width(WIDTH_MAX);
                    ui.add(egui::Label::new(RichText::new(&text).color(vis.text_color())).wrap());
                    if let Some(tg) = &target {
                        ui.add(egui::Label::new(RichText::new(tg).small().color(vis.weak_text_color())).wrap());
                    }
                    if let Some(a) = &action {
                        ui.add_space(4.0);
                        let b = ui.add(egui::Button::new(RichText::new(&a.label).small()).small());
                        button_top = Some(b.rect.top());
                        action_hit = b.clicked();
                    }
                });
                let rect = r.response.rect;
                if action_hit {
                    action_token = action.as_ref().map(|a| (a.token, a.window));
                }
                // Akzentstreifen links (folgt der Rundung der Ecken).
                let strip = egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + STRIP, rect.max.y));
                ui.painter().rect_filled(
                    strip,
                    CornerRadius { nw: RADIUS, sw: RADIUS, ne: 0, se: 0 },
                    kind.color().gamma_multiply(alpha),
                );
                // Der Knopf bleibt frei: nur der Bereich darüber schließt den Toast per Klick.
                let hit = match button_top {
                    Some(top) => egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, top - 2.0)),
                    None => rect,
                };
                let resp = ui.interact(hit, Id::new(("toast-hit", id)), Sense::click());
                (rect, resp.on_hover_text(tip), action_hit)
            });
            let (rect, resp, hit_action) = out.inner;
            if resp.clicked() || hit_action {
                // Hook: Klick auf einen Toast mit Zielhinweis soll später dorthin springen (#50).
                clicked = Some(id);
            }
            if alpha > 0.0 && pointer.is_some_and(|p| rect.contains(p)) {
                new_hover = Some(id);
            }
            offset += rect.height() + GAP;
        }
        self.toasts_hover = new_hover;
        if let Some(id) = clicked {
            self.toasts.dismiss(now, id);
        }
        if let Some((t, window)) = action_token {
            self.toast_action_clicked(ctx, t, window);
        }
        // Für Ein-/Ausblenden und den Ablauf alle ~33 ms neu zeichnen, solange Toasts stehen.
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> ToastQueue {
        ToastQueue::default()
    }

    fn texts(q: &ToastQueue) -> Vec<String> {
        q.items().iter().filter(|t| !t.is_closing()).map(|t| t.text.clone()).collect()
    }

    #[test]
    fn push_keeps_order_oldest_first() {
        let mut q = q();
        q.push(0.0, "a".into(), ToastKind::Info, None);
        q.push(0.1, "b".into(), ToastKind::Warning, Some("S1".into()));
        assert_eq!(texts(&q), ["a", "b"]);
        assert_eq!(q.items()[1].kind, ToastKind::Warning);
        assert_eq!(q.items()[1].target.as_deref(), Some("S1"));
    }

    #[test]
    fn toast_can_carry_an_action_button() {
        let mut q = q();
        q.push(0.0, "plain".into(), ToastKind::Info, None);
        q.push_with_action(0.1, "undoable".into(), ToastKind::Info, None, Some(ToastAction { label: "Undo".into(), token: 7, window: 0 }));
        assert_eq!(q.items()[0].action, None);
        assert_eq!(q.items()[1].action, Some(ToastAction { label: "Undo".into(), token: 7, window: 0 }));
    }

    #[test]
    fn at_most_five_active_oldest_pushed_out() {
        let mut q = q();
        for i in 0..7 {
            q.push(i as f64 * 0.01, format!("t{i}"), ToastKind::Info, None);
        }
        assert_eq!(q.active_len(), MAX_TOASTS);
        assert_eq!(texts(&q), ["t2", "t3", "t4", "t5", "t6"]);
        // Die verdrängten blenden nur noch aus und sind nach dem Ausblenden weg.
        q.tick(1.0, None);
        assert_eq!(q.items().len(), MAX_TOASTS);
    }

    #[test]
    fn sixth_pushes_out_exactly_the_oldest() {
        let mut q = q();
        for i in 0..5 {
            q.push(0.0, format!("t{i}"), ToastKind::Info, None);
        }
        assert_eq!(q.active_len(), 5);
        q.push(0.0, "t5".into(), ToastKind::Info, None);
        assert_eq!(texts(&q), ["t1", "t2", "t3", "t4", "t5"]);
    }

    #[test]
    fn expires_after_ten_seconds_then_fades() {
        let mut q = q();
        q.push(0.0, "a".into(), ToastKind::Info, None);
        q.tick(9.9, None);
        assert_eq!(q.active_len(), 1);
        q.tick(10.0, None);
        assert_eq!(q.active_len(), 0);
        assert_eq!(q.items().len(), 1, "blendet noch aus");
        q.tick(10.0 + FADE_OUT + 0.01, None);
        assert!(q.is_empty());
    }

    #[test]
    fn hover_pauses_only_that_toast() {
        let mut q = q();
        let a = q.push(0.0, "a".into(), ToastKind::Info, None);
        q.push(0.0, "b".into(), ToastKind::Info, None);
        // 8 s Mauszeiger auf a, dann weg: a hat nur 2 s Lebenszeit verbraucht.
        q.tick(2.0, None);
        q.tick(10.0, Some(a));
        assert_eq!(texts(&q), ["a"], "b ist abgelaufen, a pausiert");
        q.tick(17.9, None);
        assert_eq!(texts(&q), ["a"]);
        q.tick(18.1, None);
        assert!(texts(&q).is_empty());
    }

    #[test]
    fn dismiss_closes_immediately_and_removes_after_fade() {
        let mut q = q();
        let a = q.push(0.0, "a".into(), ToastKind::Info, None);
        q.push(0.0, "b".into(), ToastKind::Info, None);
        q.dismiss(1.0, a);
        assert_eq!(texts(&q), ["b"]);
        q.tick(1.0 + FADE_OUT + 0.01, None);
        assert_eq!(q.items().len(), 1);
        // Unbekannte oder schon schließende Kennung ist harmlos.
        q.dismiss(2.0, a);
        q.dismiss(2.0, 999);
        assert_eq!(texts(&q), ["b"]);
    }

    #[test]
    fn no_fade_removes_immediately() {
        let mut q = q();
        q.set_fade_out(0.0);
        let a = q.push(0.0, "a".into(), ToastKind::Info, None);
        q.dismiss(1.0, a);
        assert!(q.is_empty());
        for i in 0..6 {
            q.push(2.0, format!("t{i}"), ToastKind::Info, None);
        }
        assert_eq!(q.items().len(), MAX_TOASTS);
        assert_eq!(q.items()[0].text, "t1");
    }

    #[test]
    fn alpha_fades_in_and_out() {
        let mut q = q();
        let a = q.push(5.0, "a".into(), ToastKind::Info, None);
        let t = q.items()[0].clone();
        assert_eq!(q.alpha(&t, 5.0), 0.0);
        assert!((q.alpha(&t, 5.0 + FADE_IN / 2.0) - 0.5).abs() < 1e-4);
        assert_eq!(q.alpha(&t, 6.0), 1.0);
        q.dismiss(6.0, a);
        let t = q.items()[0].clone();
        assert!((q.alpha(&t, 6.0 + FADE_OUT / 2.0) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn ids_are_unique_and_tick_is_monotonic() {
        let mut q = q();
        let a = q.push(0.0, "a".into(), ToastKind::Info, None);
        let b = q.push(0.0, "b".into(), ToastKind::Info, None);
        assert_ne!(a, b);
        // Rückwärts springende Uhr darf das Alter nicht verringern oder abstürzen.
        q.tick(5.0, None);
        q.tick(3.0, None);
        q.tick(9.0, None);
        assert_eq!(q.active_len(), 2);
        q.tick(10.1, None);
        assert_eq!(q.active_len(), 0);
    }
}
