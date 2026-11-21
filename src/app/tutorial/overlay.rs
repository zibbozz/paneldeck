//! Zeichnen des Tutorials (#143): abgedunkeltes Fenster mit Aussparung um das Ziel und pulsierendem Rahmen, Sprechblase
//! mit „Schritt x/y“ und Zurück / Weiter / Überspringen / Beenden, die Karte „Kurze Tour?“ beim ersten Start und die
//! Rückfrage „Tutorial-Projekt behalten?“. Reine Funktionen auf dem egui-Kontext (ohne App), damit Tests sie ohne Fenster
//! zeichnen und anklicken können.
//!
//! Ebenen: Die Abdunklung liegt auf einer eigenen, nicht interaktiven Ebene der Ordnung `Middle` – über den Bereichen und
//! Fenstern (Einstellungen), unter Menüs, Popups und Modals; Klicks gehen durch sie hindurch. Die Blase ist eine `Area`
//! der Ordnung `Foreground`. Farben kommen aus dem Theme (`style::ui()`), das Pulsieren folgt der Einstellung
//! „Animationen“.

use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Align2, CornerRadius, Id, LayerId, Order, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2};

/// Ids der eigenen Ebenen (die Esc-Prüfung lässt sie aus).
pub(super) const BUBBLE_ID: &str = "tut_bubble";
pub(super) const CARD_ID: &str = "tut_card";
const DIM_ID: &str = "tut_dim";

/// Breite der Sprechblase.
pub(super) const BUBBLE_W: f32 = 360.0;
/// Abstand zwischen Ziel und Blase bzw. Rand.
const GAP: f32 = 14.0;
/// Aussparung um das Ziel.
const PAD: f32 = 6.0;

/// Was in der Blase steht.
pub(super) struct Bubble<'a> {
    pub lang: Lang,
    pub tour: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    /// Fehlende Voraussetzung: Erklärung und offizieller Link.
    pub note: Option<(&'a str, &'a str)>,
    /// Schritt (ab 1) und Zahl der Schritte.
    pub index: usize,
    pub total: usize,
    /// Der Schritt wartet auf eine Aktion (geht dann von selbst weiter).
    pub waits: bool,
    /// Aktion erkannt (kurz „Erledigt“, dann weiter).
    pub done: bool,
    /// Das Ziel ist gerade nicht zu sehen.
    pub missing: bool,
}

/// Klick in der Blase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Click {
    Back,
    Next,
    Skip,
    Quit,
}

/// Antwort auf die Karte „Kurze Tour?“.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CardClick {
    Start,
    Later,
    Never,
}

/// Antwort auf die Rückfrage am Ende.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndClick {
    Delete,
    Keep,
}

/// Fläche des Fensters (ohne Rand der Fensterleiste).
fn screen(ctx: &egui::Context) -> Rect {
    ctx.content_rect()
}

/// Deckkraft des Rahmens (0..1): pulsiert mit Animationen, sonst fest.
pub(super) fn pulse(time: f64, animate: bool) -> f32 {
    if animate { 0.55 + 0.45 * ((time * 3.2).sin() as f32 * 0.5 + 0.5) } else { 1.0 }
}

/// Fenster abdunkeln, `hole` (Ziel) bleibt frei und bekommt einen pulsierenden Rahmen in der Akzentfarbe.
pub(super) fn dim(ctx: &egui::Context, hole: Option<Rect>, animate: bool) {
    let scr = screen(ctx);
    let painter = ctx.layer_painter(LayerId::new(Order::Middle, Id::new(DIM_ID)));
    let u = style::ui();
    let shade = u.window_bg.linear_multiply(0.62);
    match hole.map(|h| h.expand(PAD).intersect(scr)).filter(|h| h.is_positive()) {
        None => {
            painter.rect_filled(scr, 0.0, shade);
        }
        Some(h) => {
            // Vier Streifen um die Aussparung (oben, unten, links, rechts).
            for r in [
                Rect::from_min_max(scr.min, Pos2::new(scr.max.x, h.min.y)),
                Rect::from_min_max(Pos2::new(scr.min.x, h.max.y), scr.max),
                Rect::from_min_max(Pos2::new(scr.min.x, h.min.y), Pos2::new(h.min.x, h.max.y)),
                Rect::from_min_max(Pos2::new(h.max.x, h.min.y), Pos2::new(scr.max.x, h.max.y)),
            ] {
                if r.is_positive() {
                    painter.rect_filled(r, 0.0, shade);
                }
            }
            let a = pulse(ctx.input(|i| i.time), animate);
            painter.rect_stroke(h, CornerRadius::same(6), Stroke::new(2.5_f32, u.accent.gamma_multiply(a)), StrokeKind::Outside);
            painter.rect_stroke(h.expand(3.0), CornerRadius::same(8), Stroke::new(4.0_f32, u.accent.gamma_multiply(0.25 * a)), StrokeKind::Outside);
        }
    }
    if animate {
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }
}

/// Lage der Blase der Größe `size`: neben dem Ziel (rechts, unten, oben, links – was zuerst ganz passt), sonst unten
/// mittig; immer ganz im Fenster `scr`, nie über dem Ziel, wenn es irgendwo passt.
pub(super) fn place(scr: Rect, target: Option<Rect>, size: Vec2) -> Pos2 {
    let inner = scr.shrink(GAP);
    let clamp = |p: Pos2| Pos2::new(p.x.clamp(inner.min.x, (inner.max.x - size.x).max(inner.min.x)), p.y.clamp(inner.min.y, (inner.max.y - size.y).max(inner.min.y)));
    let Some(t) = target.map(|t| t.expand(PAD)) else {
        return clamp(Pos2::new(scr.center().x - size.x / 2.0, scr.center().y - size.y / 2.0));
    };
    let candidates = [
        Pos2::new(t.max.x + GAP, t.center().y - size.y / 2.0),
        Pos2::new(t.center().x - size.x / 2.0, t.max.y + GAP),
        Pos2::new(t.center().x - size.x / 2.0, t.min.y - GAP - size.y),
        Pos2::new(t.min.x - GAP - size.x, t.center().y - size.y / 2.0),
    ];
    for c in candidates {
        let p = clamp(c);
        if !Rect::from_min_size(p, size).intersects(t) {
            return p;
        }
    }
    clamp(Pos2::new(scr.center().x - size.x / 2.0, scr.max.y - size.y - GAP))
}

/// Rahmen der Blase und der Karte (Theme-Farben).
fn frame(ctx: &egui::Context) -> egui::Frame {
    let u = style::ui();
    egui::Frame::new()
        .fill(u.window_bg)
        .stroke(Stroke::new(1.5_f32, u.accent))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
        .shadow(ctx.style().visuals.popup_shadow)
}

/// Sprechblase zeichnen; liefert den Klick und das Rechteck der Blase.
pub(super) fn bubble(ctx: &egui::Context, target: Option<Rect>, b: &Bubble) -> (Option<Click>, Rect) {
    let lang = b.lang;
    let id = Id::new(BUBBLE_ID);
    let size = egui::AreaState::load(ctx, id).and_then(|s| s.size).unwrap_or(Vec2::new(BUBBLE_W + 28.0, 200.0));
    let pos = place(screen(ctx), target, size);
    let mut click = None;
    let r = egui::Area::new(id).order(Order::Foreground).fixed_pos(pos).constrain(true).show(ctx, |ui| {
        frame(ui.ctx()).show(ui, |ui| {
            ui.set_width(BUBBLE_W);
            let dim = style::dim(ui);
            ui.horizontal(|ui| {
                ui.label(RichText::new(b.tour).small().color(dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if style::icon_btn_small(ui, style::Icon::Close, lang.t("Tour beenden (Esc)", "End the tour (Esc)")).clicked() {
                        click = Some(Click::Quit);
                    }
                    ui.label(RichText::new(step_label(lang, b.index, b.total)).small().color(dim));
                });
            });
            ui.add_space(S1);
            ui.label(RichText::new(b.title).strong().size(15.0));
            ui.add_space(S1);
            ui.label(b.body);
            if let Some((note, link)) = b.note {
                ui.add_space(S2);
                ui.label(RichText::new(note).small().color(style::ui().warning));
                ui.hyperlink_to(RichText::new(link).small(), link);
            }
            if b.missing {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Das gezeigte Element ist gerade nicht zu sehen – „Weiter“ geht trotzdem.", "The highlighted element is not visible right now – “Next” still works.")).small().color(dim));
            }
            if b.done {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("✓ Erledigt", "✓ Done")).color(style::ui().success));
            } else if b.waits {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Geht von selbst weiter, sobald du es gemacht hast.", "Continues by itself once you have done it.")).small().color(dim));
            }
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if b.index > 1 && ui.button(lang.t("Zurück", "Back")).clicked() {
                    click = Some(Click::Back);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let last = b.index >= b.total;
                    let next = egui::Button::new(RichText::new(if last { lang.t("Fertig", "Finish") } else { lang.t("Weiter", "Next") }).strong()).fill(style::ui().accent.gamma_multiply(0.45));
                    if ui.add(next).clicked() {
                        click = Some(Click::Next);
                    }
                    if !last && ui.button(lang.t("Überspringen", "Skip")).clicked() {
                        click = Some(Click::Skip);
                    }
                });
            });
        });
    });
    (click, r.response.rect)
}

/// „Schritt x/y“.
pub(super) fn step_label(lang: Lang, index: usize, total: usize) -> String {
    format!("{} {index}/{total}", lang.t("Schritt", "Step"))
}

/// Karte „Kurze Tour (3 Min.)?“ beim ersten Start, oben mittig.
pub(super) fn card(ctx: &egui::Context, lang: Lang) -> Option<CardClick> {
    let mut out = None;
    egui::Area::new(Id::new(CARD_ID)).order(Order::Foreground).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 70.0)).show(ctx, |ui| {
        frame(ui.ctx()).show(ui, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(lang.t("Kurze Tour (3 Min.)?", "Short tour (3 min)?")).strong().size(16.0));
            ui.add_space(S1);
            ui.label(lang.t(
                "Paneldeck zeigt dir die wichtigsten Teile direkt im Fenster. Die Tour nutzt ein eigenes Projekt; deine Sitzungen bleiben unberührt. Später findest du sie in der Befehls-Palette und unter Einstellungen → Info.",
                "Paneldeck shows you the main parts right in the window. The tour uses its own project; your sessions stay untouched. Later you find it in the command palette and under Settings → About.",
            ));
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(lang.t("Starten", "Start")).strong()).fill(style::ui().accent.gamma_multiply(0.45))).clicked() {
                    out = Some(CardClick::Start);
                }
                if ui.button(lang.t("Später", "Later")).clicked() {
                    out = Some(CardClick::Later);
                }
                if ui.button(lang.t("Nie", "Never")).on_hover_text(lang.t("Nicht mehr fragen – die Tour bleibt in der Hilfe", "Don't ask again – the tour stays in the help")).clicked() {
                    out = Some(CardClick::Never);
                }
            });
        });
    });
    out
}

/// Rückfrage am Ende (`finished`) bzw. nach dem Beenden einer Tour: Tutorial-Projekt löschen (Standard) oder behalten.
/// `what` beschreibt, was dazugehört (Sitzungen, To-dos, Tickets).
pub(super) fn end_ask(ctx: &egui::Context, lang: Lang, finished: bool, what: &str) -> Option<EndClick> {
    let mut out = None;
    let r = style::modal(Id::new("tut_end")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(RichText::new(if finished { lang.t("Geschafft!", "Done!") } else { lang.t("Tour beendet", "Tour ended") }).strong().size(16.0));
        ui.add_space(S1);
        if !finished {
            ui.label(RichText::new(lang.t(
                "Dein Fortschritt bleibt erhalten: „Fortsetzen“ in der Befehls-Palette („Tutorial“) oder unter Einstellungen → Info macht dort weiter.",
                "Your progress is kept: “Continue” in the command palette (“Tutorial”) or under Settings → About picks up where you left off.",
            )).color(style::dim(ui)));
            ui.add_space(S1);
        }
        ui.label(format!("{} {what}", lang.t("Das Tutorial-Projekt löschen? Dazu gehören:", "Delete the tutorial project? It includes:")));
        ui.label(RichText::new(lang.t("Deine eigenen Sitzungen und Daten bleiben unberührt.", "Your own sessions and data stay untouched.")).small().color(style::dim(ui)));
        ui.add_space(S3);
        ui.horizontal(|ui| {
            let del = ui.add(egui::Button::new(RichText::new(lang.t("Löschen", "Delete")).strong()).fill(style::ui().accent.gamma_multiply(0.45)));
            if del.clicked() {
                out = Some(EndClick::Delete);
            }
            if ui.button(lang.t("Behalten", "Keep")).clicked() {
                out = Some(EndClick::Keep);
            }
            // Standard ist Löschen: Enter bestätigt es.
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                out = out.or(Some(EndClick::Delete));
            }
        });
    });
    // Esc oder ein Klick daneben behält (nichts wird ungefragt gelöscht).
    if r.should_close() && out.is_none() {
        out = Some(EndClick::Keep);
    }
    out
}
