//! Bilder im Chat-Bereich (#357): Platzhalter `[Image#n]` hervorheben und anklickbar machen (Eingabefeld, eigene
//! Nachrichten, Werkzeug-Ergebnisse), das Popup mit dem Original (passt ins Hauptfenster, Esc schließt), die
//! Büroklammer und Strg+V. Was dabei in den Chat kommt, erledigt die App (`app/chat_images.rs`).

use crate::chat::images::{self, ImageEntry};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, text::CCursor, text::CCursorRange, Color32, CursorIcon, Event, FontId, Galley, Id, Key, Pos2, Rect, RichText, Sense, Ui, Vec2};
use std::path::Path;
use std::sync::Arc;

/// Was die Oberfläche an Bildern verlangt (die App führt es aus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImageReq {
    /// Strg+V mit einem Bild in der Zwischenablage (auch Win+Umschalt+S).
    Paste,
    /// Büroklammer: Bilddateien auswählen.
    Pick,
}

/// Breite der Büroklammer neben „Senden“.
pub(super) const CLIP_W: f32 = 30.0;

/// Farbe der Platzhalter (wie Verweise).
fn link_color(ui: &Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

/// Text mit hervorgehobenen Platzhaltern (unterstrichen, Verweisfarbe), umbrochen bei `wrap`.
pub(super) fn job(text: &str, font: FontId, color: Color32, link: Color32, wrap: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
    let tok = egui::TextFormat { font_id: font, color: link, underline: egui::Stroke::new(1.0_f32, link), ..Default::default() };
    let mut at = 0;
    for (s, e, _) in images::tokens(text) {
        if s > at {
            job.append(&text[at..s], 0.0, plain.clone());
        }
        job.append(&text[s..e], 0.0, tok.clone());
        at = e;
    }
    if at < text.len() || text.is_empty() {
        job.append(&text[at..], 0.0, plain);
    }
    job.wrap.max_width = wrap;
    job
}

/// Flächen der Platzhalter in einer Galerie an `origin` (ein Platzhalter über einen Umbruch: zwei Flächen).
fn token_rects(galley: &Galley, origin: Pos2, text: &str) -> Vec<(Rect, u32)> {
    let mut out = Vec::new();
    for (s, e, n) in images::char_tokens(text) {
        let a = galley.pos_from_cursor(CCursor::new(s));
        let b = galley.pos_from_cursor(CCursor::new(e));
        if (a.min.y - b.min.y).abs() < 1.0 {
            out.push((Rect::from_min_max(a.min, Pos2::new(b.max.x, a.max.y)).translate(origin.to_vec2()), n));
        } else {
            let r = galley.rect;
            out.push((Rect::from_min_max(a.min, Pos2::new(r.max.x, a.max.y)).translate(origin.to_vec2()), n));
            out.push((Rect::from_min_max(Pos2::new(r.min.x, b.min.y), b.max).translate(origin.to_vec2()), n));
        }
    }
    out
}

/// Platzhalter unter `pos` (Bildschirmkoordinaten) in der Galerie an `origin`.
pub(super) fn hit(galley: &Galley, origin: Pos2, text: &str, pos: Pos2) -> Option<u32> {
    token_rects(galley, origin, text).into_iter().find(|(r, _)| r.expand(1.0).contains(pos)).map(|(_, n)| n)
}

/// Zeiger über einem Platzhalter: Hand zeigen (und `true`).
pub(super) fn hover(ui: &Ui, galley: &Galley, origin: Pos2, text: &str) -> bool {
    let over = ui.input(|i| i.pointer.hover_pos()).and_then(|p| hit(galley, origin, text, p)).is_some();
    if over {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    over
}

/// Id des Popups eines Chats (gemerkt: Nummer des gezeigten Bildes).
fn popup_id(session: u64) -> Id {
    Id::new(("chat_img_popup", session))
}

fn texture_id(session: u64) -> Id {
    Id::new(("chat_img_view", session))
}

/// Popup mit Bild `n` öffnen.
pub(super) fn open(ctx: &egui::Context, session: u64, n: u32) {
    ctx.data_mut(|d| d.insert_temp(popup_id(session), n));
    ctx.request_repaint();
}

/// Nummer des Bildes im offenen Popup.
pub(super) fn open_image(ctx: &egui::Context, session: u64) -> Option<u32> {
    ctx.data(|d| d.get_temp::<u32>(popup_id(session)))
}

/// Popup schließen (auch die Textur fällt weg).
pub(super) fn close(ctx: &egui::Context, session: u64) {
    ctx.data_mut(|d| {
        d.remove::<u32>(popup_id(session));
        d.remove::<(u32, Result<egui::TextureHandle, String>)>(texture_id(session));
    });
}

/// Textur des Originals (einmal je geöffnetem Bild geladen).
fn texture(ctx: &egui::Context, session: u64, n: u32, entry: Option<&ImageEntry>, dir: Option<&Path>, lang: Lang) -> Result<egui::TextureHandle, String> {
    if let Some((k, t)) = ctx.data(|d| d.get_temp::<(u32, Result<egui::TextureHandle, String>)>(texture_id(session)))
        && k == n
    {
        return t;
    }
    let r = match (entry, dir) {
        (Some(e), Some(dir)) => images::load_view(dir, e).map(|img| {
            let size = [img.width() as usize, img.height() as usize];
            ctx.load_texture(format!("chat-image-{session}-{n}"), egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
        }),
        _ => Err(lang.t("Bild nicht gefunden", "Image not found").to_string()),
    };
    ctx.data_mut(|d| d.insert_temp(texture_id(session), (n, r.clone())));
    r
}

/// Größe, in der ein Bild `size` in `max` passt (nie größer als das Original).
pub(super) fn fit_size(size: Vec2, max: Vec2) -> Vec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return Vec2::ZERO;
    }
    let s = (max.x / size.x).min(max.y / size.y).min(1.0);
    size * s
}

/// Kurzbeschreibung eines Bildes (Platzhalter, Größe, Name).
pub(super) fn caption(n: u32, e: Option<&ImageEntry>) -> String {
    match e {
        Some(e) => {
            let mut s = format!("{} · {} × {}", images::token(n), e.width, e.height);
            if !e.name.is_empty() {
                s += &format!(" · {}", e.name);
            }
            s
        }
        None => images::token(n),
    }
}

/// Popup zeichnen, wenn eines offen ist: das Original, eingepasst ins Hauptfenster; Esc, Klick daneben oder
/// „Schließen“ schließt es.
pub(super) fn popup(ctx: &egui::Context, session: u64, entries: &[ImageEntry], dir: Option<&Path>, lang: Lang) {
    let Some(n) = open_image(ctx, session) else { return };
    let entry = entries.iter().find(|e| e.n == n);
    let tex = texture(ctx, session, n, entry, dir, lang);
    let screen = ctx.content_rect();
    let max = (screen.size() * 0.92 - Vec2::new(24.0, 64.0)).max(Vec2::splat(64.0));
    let r = egui::Modal::new(Id::new(("chat_img_modal", session))).show(ctx, |ui| {
        let mut closed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(caption(n, entry)).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                closed = ui.button(lang.t("Schließen", "Close")).on_hover_text(lang.t("Schließen (Esc)", "Close (Esc)")).clicked();
            });
        });
        ui.add_space(style::S1);
        match &tex {
            Ok(t) => {
                let size = fit_size(t.size_vec2(), max);
                ui.add(egui::Image::new((t.id(), size)));
            }
            Err(e) => {
                ui.label(RichText::new(e).color(crate::style::ui().error));
            }
        }
        closed
    });
    if r.inner || r.should_close() {
        close(ctx, session);
    }
}

/// Bilder eines Werkzeug-Ergebnisses als anklickbare Platzhalter („Bild: [Image#2]“).
pub(super) fn links(ui: &mut Ui, session: u64, nums: &[u32], entries: &[ImageEntry], lang: Lang) {
    if nums.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(lang.t("Bild:", "Image:")).small().color(style::dim(ui)));
        for n in nums {
            let e = entries.iter().find(|e| e.n == *n);
            let r = ui.add(egui::Label::new(RichText::new(images::token(*n)).small().color(link_color(ui)).underline()).sense(Sense::click()));
            let r = r.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(caption(*n, e));
            if r.clicked() {
                open(ui.ctx(), session, *n);
            }
        }
    });
}

/// Eigene Nachricht mit Platzhaltern: Galerie in Verweisfarbe (sonst wie bisher).
pub(super) fn bubble_galley(ui: &Ui, text: &str, color: Color32, wrap: f32) -> Arc<Galley> {
    if images::tokens(text).is_empty() {
        return ui.painter().layout(text.to_string(), FontId::proportional(14.0), color, wrap);
    }
    ui.fonts_mut(|f| f.layout_job(job(text, FontId::proportional(14.0), color, link_color(ui), wrap)))
}

/// Klick auf einen Platzhalter in einer eigenen Nachricht (Galerie an `origin`, Fläche `rect`): Popup öffnen.
pub(super) fn bubble_clicks(ui: &mut Ui, session: u64, id: Id, rect: Rect, galley: &Galley, origin: Pos2, text: &str) {
    if images::tokens(text).is_empty() {
        return;
    }
    let r = ui.interact(rect, id, Sense::click());
    hover(ui, galley, origin, text);
    if r.clicked()
        && let Some(n) = r.interact_pointer_pos().and_then(|p| hit(galley, origin, text, p))
    {
        open(ui.ctx(), session, n);
    }
}

/// Layouter für das Eingabefeld: Platzhalter hervorgehoben (nur nötig, wenn der Entwurf welche hat).
pub(super) fn input_layouter(ui: &Ui) -> impl FnMut(&Ui, &dyn egui::TextBuffer, f32) -> Arc<Galley> + use<> {
    let font = egui::FontSelection::default().resolve(ui.style());
    let color = ui.visuals().override_text_color.unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
    let link = link_color(ui);
    move |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| ui.fonts_mut(|f| f.layout_job(job(buf.as_str(), font.clone(), color, link, wrap)))
}

/// Strg+V im Eingabefeld, ohne dass Text eingefügt wird: egui-winit macht aus Strg+V nur bei Text in der
/// Zwischenablage ein `Event::Paste` und verschluckt den Tastendruck sonst (siehe `termview::ctrl_v_pressed`), darum
/// zählt die Flanke der echten Tasten. Ein `Key::V` mit Strg in den Ereignissen zählt ebenso (andere Systeme, Tests).
pub(super) fn paste_requested(ui: &Ui) -> bool {
    use crate::platform::keyboard::{is_down, Key as PKey};
    use std::sync::atomic::{AtomicBool, Ordering};
    static PREV: AtomicBool = AtomicBool::new(false);
    let down = ui.input(|i| i.focused) && is_down(PKey::Control) && is_down(PKey::V) && !is_down(PKey::Alt);
    let prev = PREV.swap(down, Ordering::Relaxed);
    let (text, key) = ui.input(|i| {
        (
            i.events.iter().any(|e| matches!(e, Event::Paste(_))),
            i.events.iter().any(|e| matches!(e, Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt)),
        )
    });
    ((down && !prev) || key) && !text
}

/// Platzhalter `n` an der Cursor-Position des Eingabefelds `id` einfügen (ohne Zustand: am Ende) und den Cursor
/// dahinter setzen.
pub(super) fn insert_token(ctx: &egui::Context, id: Id, draft: &mut String, n: u32) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let at = state.cursor.char_range().map_or_else(|| draft.chars().count(), |r| r.primary.index.max(r.secondary.index));
    let after = images::insert_token(draft, at, n);
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(after))));
    state.store(ctx, id);
}

/// Hinweiszeile unter dem Eingabefeld: Bild wird vorbereitet, Fehler beim Einfügen, Modell ohne Bildverständnis.
pub(super) fn input_note(draft: &str, busy: bool, error: Option<&str>, vision: Option<bool>, lang: Lang) -> Option<(String, bool)> {
    if let Some(e) = error {
        return Some((format!("{} {e}", lang.t("Bild nicht eingefügt:", "Image not added:")), true));
    }
    let has = !images::tokens(draft).is_empty();
    if busy {
        return Some((lang.t("Bild wird vorbereitet …", "Preparing image …").to_string(), false));
    }
    (has && vision == Some(false)).then(|| (lang.t("Dieses Modell kann keine Bilder lesen – Bilder werden nicht gesendet.", "This model cannot read images – images are not sent.").to_string(), true))
}

/// Höhe der Hinweiszeile.
pub(super) const NOTE_H: f32 = 18.0;

/// Büroklammer um `c` zeichnen (16 px, schräg wie üblich).
fn paint_clip(p: &egui::Painter, c: Pos2, col: Color32) {
    use std::f32::consts::PI;
    let mut pts: Vec<Vec2> = Vec::new();
    let arc = |pts: &mut Vec<Vec2>, cx: f32, cy: f32, r: f32, a0: f32, a1: f32| {
        for i in 0..=8 {
            let a = a0 + (a1 - a0) * i as f32 / 8.0;
            pts.push(Vec2::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    };
    pts.push(Vec2::new(0.0, 3.0));
    arc(&mut pts, 1.25, -3.0, 1.25, PI, 2.0 * PI);
    arc(&mut pts, 0.0, 4.0, 2.5, 0.0, PI);
    arc(&mut pts, 0.75, -4.5, 3.25, PI, 2.0 * PI);
    pts.push(Vec2::new(4.0, 2.0));
    let (s, co) = (-PI / 4.0).sin_cos();
    let line: Vec<Pos2> = pts.into_iter().map(|v| c + Vec2::new(v.x * co - v.y * s, v.x * s + v.y * co) - Vec2::new(0.5, 0.0)).collect();
    p.add(egui::Shape::line(line, egui::Stroke::new(1.4_f32, col)));
}

/// Büroklammer (Bilder anhängen).
pub(super) fn attach_button(ui: &mut Ui, rect: Rect, lang: Lang) -> bool {
    let r = ui
        .put(rect, egui::Button::new(""))
        .on_hover_text(lang.t("Bild anhängen … (oder Strg+V, oder aus dem Explorer hierher ziehen)", "Attach image … (or Ctrl+V, or drag it here from Explorer)"));
    let col = if r.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    paint_clip(ui.painter(), rect.center(), col);
    r.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_never_enlarges_and_keeps_the_ratio() {
        assert_eq!(fit_size(Vec2::new(4000.0, 2000.0), Vec2::new(1000.0, 1000.0)), Vec2::new(1000.0, 500.0));
        assert_eq!(fit_size(Vec2::new(100.0, 50.0), Vec2::new(1000.0, 1000.0)), Vec2::new(100.0, 50.0));
        assert_eq!(fit_size(Vec2::ZERO, Vec2::splat(10.0)), Vec2::ZERO);
    }

    #[test]
    fn notes_under_the_input() {
        assert_eq!(input_note("x", false, None, Some(false), Lang::En), None, "ohne Bild kein Hinweis");
        assert_eq!(input_note("[Image#1]", false, None, Some(true), Lang::En), None);
        assert!(input_note("[Image#1]", false, None, Some(false), Lang::En).unwrap().0.starts_with("This model cannot read images"));
        assert!(input_note("[Image#1]", false, None, Some(false), Lang::De).unwrap().0.starts_with("Dieses Modell kann keine Bilder lesen"));
        assert_eq!(input_note("[Image#1]", true, None, None, Lang::En).unwrap().0, "Preparing image …");
        assert!(input_note("", false, Some("boom"), None, Lang::En).unwrap().0.ends_with("boom"));
        assert_eq!(caption(3, None), "[Image#3]");
        let e = ImageEntry { n: 3, width: 40, height: 30, name: "a.png".into(), ..Default::default() };
        assert_eq!(caption(3, Some(&e)), "[Image#3] · 40 × 30 · a.png");
    }
}
