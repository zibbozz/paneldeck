//! Inhalt eines Chat-Bereichs (#63): Nachrichtenliste, Werkzeugzeilen, Eingabe. Kennt kein Backend – Senden, Stopp,
//! Neu erzeugen und Bearbeiten meldet [`chat_body`] zurück; die App führt sie aus (`Action::Chat*`).

use crate::chat::session::UiCache;
use crate::chat::{ChatMsg, ChatState, Role, ToolStatus};
use crate::markdown::{self, RenderOpts};
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Color32, CornerRadius, Event, FontId, Id, Key, Margin, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Ui, UiBuilder, Vec2};

/// Was der Nutzer in diesem Bild ausgelöst hat.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ChatOut {
    pub send: bool,
    pub stop: bool,
    pub regenerate: bool,
    pub edit: bool,
    /// In den Bereich geklickt / Eingabe hat den Fokus (Bereich fokussieren).
    pub focus: bool,
}

const PAD: f32 = 10.0;
const BTN_W: f32 = 76.0;
const MAX_INPUT_ROWS: usize = 8;

/// Id des Eingabefelds eines Chats.
pub(super) fn input_id(session: u64) -> Id {
    Id::new(("chat_input", session))
}

/// Zeichnet den Chat in `body`. `grab_focus`: Eingabefeld fokussieren (neuer Chat, Sitzung aktiviert).
pub(super) fn chat_body(ui: &mut Ui, body: Rect, session: u64, chat: &mut ChatState, model: &str, lang: Lang, grab_focus: bool) -> ChatOut {
    let mut out = ChatOut::default();
    let row_h = ui.text_style_height(&TextStyle::Body);
    let lines = (chat.draft.split('\n').count()).clamp(1, MAX_INPUT_ROWS);
    let input_h = lines as f32 * row_h + 12.0;
    let edit_h = if chat.editing { 22.0 } else { 0.0 };
    let bottom_h = input_h + 2.0 * PAD + edit_h;
    let list_rect = Rect::from_min_max(body.min, Pos2::new(body.max.x, (body.max.y - bottom_h).max(body.min.y)));
    let bar_rect = Rect::from_min_max(Pos2::new(body.min.x, list_rect.max.y), body.max);

    // Trennlinie über der Eingabe
    ui.painter().hline(bar_rect.x_range(), bar_rect.min.y + 0.5, Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7)));

    let bg = ui.interact(list_rect, Id::new(("chat_bg", session)), Sense::click());
    if bg.clicked() {
        out.focus = true;
    }
    ui.scope_builder(UiBuilder::new().max_rect(list_rect.shrink2(Vec2::new(PAD, 0.0))), |ui| {
        messages(ui, session, chat, model, lang, &mut out);
    });
    ui.scope_builder(UiBuilder::new().max_rect(bar_rect.shrink(PAD)), |ui| {
        input_bar(ui, session, chat, lang, input_h, grab_focus, &mut out);
    });
    out
}

/// Geschätzte Höhe einer noch nie gezeichneten Nachricht.
const EST_ROW: f32 = 60.0;
/// So weit über/unter dem sichtbaren Bereich werden Nachrichten noch gezeichnet (weiches Scrollen).
const ROW_MARGIN: f32 = 300.0;

/// Was jede Zeile über den ganzen Verlauf wissen muss.
struct RowCx<'a> {
    session: u64,
    model: &'a str,
    lang: Lang,
    streaming: bool,
    has_error: bool,
    can_regen: bool,
    last: Option<usize>,
    last_user: Option<usize>,
    context_start: Option<usize>,
    /// Token-Verbrauch der letzten Antwort (Eingabe, Antwort).
    usage: Option<(Option<u32>, Option<u32>)>,
}

fn messages(ui: &mut Ui, session: u64, chat: &mut ChatState, model: &str, lang: Lang, out: &mut ChatOut) {
    let streaming = chat.streaming();
    let has_error = chat.has_error();
    let can_regen = !streaming && crate::chat::logic::can_regenerate(chat.messages());
    let rev = chat.rev();
    let from = std::mem::replace(&mut chat.changed_from, usize::MAX);
    let ChatState { file, ui: cache, warning, follow, notice, usage, .. } = chat;
    let msgs = &file.messages;
    let n = msgs.len();
    // Zwischenspeicher nur für Geändertes erneuern (nicht jedes Bild den ganzen Verlauf).
    if from != usize::MAX {
        cache.md.retain(|k, _| *k < from);
        cache.heights.truncate(from.min(cache.heights.len()));
    }
    cache.heights.resize(n, 0.0);
    if cache.tool_rev != Some(rev) {
        cache.tool_rev = Some(rev);
        cache.tool_results.clear();
        cache.matched.clear();
        let calls: std::collections::HashSet<&str> = msgs.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.id.as_str())).collect();
        for (i, m) in msgs.iter().enumerate() {
            if let (Role::Tool, Some(tid)) = (m.role, m.tool_call_id.as_deref()) {
                if calls.contains(tid) {
                    cache.tool_results.insert(tid.to_string(), i);
                    cache.matched.insert(i);
                }
            }
        }
    }
    let rc = RowCx {
        session,
        model,
        lang,
        streaming,
        has_error,
        can_regen,
        last: n.checked_sub(1),
        last_user: crate::chat::logic::last_user(msgs),
        context_start: crate::chat::logic::clamp_context_start(file.context_start, n),
        usage: *usage,
    };
    // Am Ende kleben, solange der Nutzer nicht hochscrollt; nach dem Öffnen und nach dem Senden einmal ganz nach unten.
    let jump = std::mem::take(follow);
    let area = egui::ScrollArea::vertical().id_salt(("chat_scroll", session)).stick_to_bottom(true).auto_shrink([false, false]);
    area.show_viewport(ui, |ui, viewport| {
        let content_top = ui.max_rect().min.y;
        ui.add_space(S2);
        if let Some(n) = notice.as_deref() {
            egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(6)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new(n).small().color(style::dim(ui)));
            });
            ui.add_space(S1);
        }
        if let Some(w) = warning.clone() {
            egui::Frame::new().fill(Color32::from_rgb(0xe5, 0xc0, 0x7b).gamma_multiply(0.18)).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(w);
                    if ui.small_button(lang.t("Ausblenden", "Dismiss")).clicked() {
                        *warning = None;
                    }
                });
            });
            ui.add_space(S2);
        }
        if msgs.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(lang.t("Neuer Chat", "New chat")).heading());
                ui.add_space(S1);
                ui.label(RichText::new(format!("{}: {model}", lang.t("Modell", "Model"))).color(style::dim(ui)));
                ui.label(RichText::new(lang.t("Enter sendet, Umschalt+Enter macht eine neue Zeile.", "Enter sends, Shift+Enter adds a new line.")).color(style::dim(ui)));
            });
        }
        // Nur Zeilen im sichtbaren Bereich (plus Rand) zeichnen; die übrigen belegen ihre zuletzt gemessene Höhe.
        let origin = ui.cursor().min;
        let width = ui.available_width();
        let mut y = 0.0;
        let mut remeasured = false;
        for (i, m) in msgs.iter().enumerate() {
            let known = cache.heights[i];
            let h = if known > 0.0 { known } else { EST_ROW };
            let top = origin.y - content_top + y;
            if top + h < viewport.min.y - ROW_MARGIN || top > viewport.max.y + ROW_MARGIN {
                y += h;
                continue;
            }
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + y), Vec2::new(width, 1.0e6));
            let mut row_ui = ui.new_child(UiBuilder::new().id_salt(("chat_row", i)).max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            row(&mut row_ui, i, m, msgs, cache, &rc, out);
            let mh = row_ui.min_rect().height();
            if (mh - known).abs() > 0.5 {
                cache.heights[i] = mh;
                remeasured = true;
            }
            y += mh;
        }
        let total = Rect::from_min_size(origin, Vec2::new(width, y + S1));
        ui.allocate_rect(total, Sense::hover());
        if jump {
            ui.scroll_to_rect(Rect::from_min_size(Pos2::new(origin.x, total.max.y - 1.0), Vec2::new(1.0, 1.0)), Some(egui::Align::BOTTOM));
        }
        if remeasured {
            ui.ctx().request_repaint(); // Höhen haben sich geändert: Positionen im nächsten Bild stimmen
        }
    });
}

/// Eine Nachricht samt Abstand darunter (und ggf. der Kürzungs-Markierung darüber).
fn row(ui: &mut Ui, i: usize, m: &ChatMsg, msgs: &[ChatMsg], cache: &mut UiCache, rc: &RowCx, out: &mut ChatOut) {
    let lang = rc.lang;
    if rc.context_start == Some(i) {
        context_marker(ui, lang);
    }
    match m.role {
        Role::User => user_bubble(ui, m, Some(i) == rc.last_user && !rc.streaming, lang, out),
        Role::Assistant => {
            let is_last = Some(i) == rc.last;
            assistant(ui, rc.session, i, m, msgs, cache, rc.model, lang);
            if is_last && rc.streaming && m.content.is_empty() && m.tool_calls.is_empty() {
                ui.label(RichText::new("…").color(style::dim(ui)));
            }
            if let Some(n) = &m.note {
                ui.label(RichText::new(format!("⚠ {n}")).small().color(Color32::from_rgb(0xe5, 0xc0, 0x7b)));
            }
            if let (true, Some(e)) = (is_last && rc.has_error, &m.error) {
                error_box(ui, e, lang, out);
            } else if let Some(e) = &m.error {
                ui.label(RichText::new(format!("{}: {e}", lang.t("Fehler", "Error"))).small().color(error_col()));
            } else if is_last && rc.can_regen {
                ui.horizontal(|ui| {
                    if ui.small_button("↻").on_hover_text(lang.t("Antwort neu erzeugen", "Regenerate answer")).clicked() {
                        out.regenerate = true;
                    }
                    if let Some(u) = usage_text(rc.usage, lang) {
                        ui.label(RichText::new(u).small().color(style::dim(ui)));
                    }
                });
            }
        }
        // Ergebnisse mit passendem Aufruf stehen in dessen Zeile.
        Role::Tool if cache.matched.contains(&i) => {}
        Role::Tool => tool_line(ui, Id::new(("chat_tool_r", rc.session, i)), m.tool_name.as_deref().unwrap_or("?"), None, Some(m), lang),
        Role::System => {
            egui::CollapsingHeader::new(RichText::new(lang.t("Systemprompt", "System prompt")).small().color(style::dim(ui))).id_salt(("chat_sys", rc.session, i)).show(ui, |ui| {
                ui.label(RichText::new(&m.content).small().monospace());
            });
        }
    }
    if !(m.role == Role::Tool && cache.matched.contains(&i)) {
        ui.add_space(S2 + 2.0);
    }
}

/// „Tokens: 120 Eingabe · 45 Antwort“ (nur, was der Server gemeldet hat).
fn usage_text(u: Option<(Option<u32>, Option<u32>)>, lang: Lang) -> Option<String> {
    let (p, c) = u?;
    let mut parts = Vec::new();
    if let Some(p) = p {
        parts.push(format!("{p} {}", lang.t("Eingabe", "in")));
    }
    if let Some(c) = c {
        parts.push(format!("{c} {}", lang.t("Antwort", "out")));
    }
    (!parts.is_empty()).then(|| format!("Tokens: {}", parts.join(" · ")))
}

fn error_col() -> Color32 {
    Color32::from_rgb(0xe0, 0x6c, 0x75)
}

/// Linie mit Hinweis: davor liegende Nachrichten sieht das Modell nicht mehr.
fn context_marker(ui: &mut Ui, lang: Lang) {
    let text = lang.t("Ältere Nachrichten sieht das Modell nicht mehr", "Older messages are no longer visible to the model");
    let col = style::dim(ui);
    let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.5), col);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 8.0), Sense::hover());
    let c = r.center();
    let half = g.size().x / 2.0 + 8.0;
    let st = Stroke::new(1.0_f32, col.gamma_multiply(0.6));
    ui.painter().hline(r.min.x..=(c.x - half).max(r.min.x), c.y, st);
    ui.painter().hline((c.x + half).min(r.max.x)..=r.max.x, c.y, st);
    ui.painter().galley(Pos2::new(c.x - g.size().x / 2.0, c.y - g.size().y / 2.0), g, col);
    ui.add_space(S2);
}

/// Eigene Nachricht: rechtsbündige Blase; an der letzten ein Knopf zum Bearbeiten.
fn user_bubble(ui: &mut Ui, m: &ChatMsg, editable: bool, lang: Lang, out: &mut ChatOut) {
    let full = ui.available_width();
    let maxw = (full * 0.78).max(120.0);
    let text_col = ui.visuals().strong_text_color();
    let galley = ui.painter().layout(m.content.clone(), FontId::proportional(14.0), text_col, maxw - 20.0);
    let size = galley.size() + Vec2::new(20.0, 14.0);
    let (row, _) = ui.allocate_exact_size(Vec2::new(full, size.y), Sense::hover());
    let bubble = Rect::from_min_size(Pos2::new(row.max.x - size.x, row.min.y), size);
    ui.painter().rect_filled(bubble, 10.0, style::ACCENT.gamma_multiply(0.28));
    ui.painter().galley(bubble.min + Vec2::new(10.0, 7.0), galley, text_col);
    if editable {
        let br = Rect::from_center_size(Pos2::new(bubble.min.x - 16.0, bubble.min.y + 12.0), Vec2::splat(22.0));
        let resp = ui.put(br, egui::Button::new("✎").small().frame(false)).on_hover_text(lang.t("Letzte Nachricht bearbeiten", "Edit last message"));
        if resp.clicked() {
            out.edit = true;
        }
    }
}

/// Antwort des Modells: Markdown (geparst nur, wenn sich die Nachricht geändert hat) und eine Zeile je Werkzeugaufruf.
#[allow(clippy::too_many_arguments)]
fn assistant(ui: &mut Ui, session: u64, i: usize, m: &ChatMsg, msgs: &[ChatMsg], cache: &mut UiCache, model: &str, lang: Lang) {
    ui.label(RichText::new(model).small().color(style::dim(ui)));
    if !m.content.is_empty() {
        let doc = cache.md.entry(i).or_insert_with(|| markdown::parse_plain(&m.content));
        let opts = RenderOpts { soft_breaks_as_newlines: true, copy_button: Some(lang.t("Kopieren", "Copy")) };
        let o = markdown::render(ui, doc, &opts);
        if let Some(u) = o.clicked_url {
            ui.ctx().open_url(egui::OpenUrl::new_tab(u));
        }
    }
    for (k, c) in m.tool_calls.iter().enumerate() {
        let result = cache.tool_results.get(&c.id).and_then(|j| msgs.get(*j));
        tool_line(ui, Id::new(("chat_tool", session, i, k)), &c.name, Some(&c.arguments), result, lang);
    }
}

/// Eine eingeklappte Zeile je Werkzeugaufruf (`▸ name ✓`), aufgeklappt Parameter und Ergebnis.
fn tool_line(ui: &mut Ui, id: Id, name: &str, args: Option<&serde_json::Value>, result: Option<&ChatMsg>, lang: Lang) {
    let (mark, col) = match result.and_then(|r| r.status) {
        Some(ToolStatus::Ok) => ("✓", Color32::from_rgb(0x4c, 0xc2, 0x6a)),
        Some(ToolStatus::Error) => ("✗", error_col()),
        Some(ToolStatus::Denied) => (lang.t("abgelehnt", "denied"), error_col()),
        None if result.is_some() => ("✓", Color32::from_rgb(0x4c, 0xc2, 0x6a)),
        None => ("…", style::dim(ui)),
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(name, 0.0, egui::TextFormat { font_id: FontId::monospace(12.5), color: ui.visuals().text_color(), ..Default::default() });
    job.append(mark, 6.0, egui::TextFormat { font_id: FontId::proportional(12.5), color: col, ..Default::default() });
    egui::CollapsingHeader::new(job).id_salt(id).default_open(false).show(ui, |ui| {
        if let Some(a) = args {
            ui.label(RichText::new(lang.t("Parameter", "Parameters")).small().color(style::dim(ui)));
            code(ui, &serde_json::to_string_pretty(a).unwrap_or_default());
        }
        ui.label(RichText::new(lang.t("Ergebnis", "Result")).small().color(style::dim(ui)));
        match result {
            Some(r) => code(ui, &r.content),
            None => {
                ui.label(RichText::new(lang.t("(noch kein Ergebnis)", "(no result yet)")).small().color(style::dim(ui)));
            }
        }
    });
}

fn code(ui: &mut Ui, text: &str) {
    let shown: String = text.chars().take(20_000).collect();
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(6)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(shown).monospace().size(12.0)).wrap_mode(egui::TextWrapMode::Wrap));
    });
}

fn error_box(ui: &mut Ui, e: &str, lang: Lang, out: &mut ChatOut) {
    egui::Frame::new().fill(error_col().gamma_multiply(0.15)).stroke(Stroke::new(1.0_f32, error_col().gamma_multiply(0.6))).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(e).color(error_col()));
            if ui.button(lang.t("Wiederholen", "Retry")).clicked() {
                out.regenerate = true;
            }
        });
    });
}

/// Läuft gerade eine IME-Eingabe (Vorschautext)? Dann gehört Enter der IME (Übernehmen), nicht dem Senden.
fn ime_composing(ui: &Ui, tid: Id) -> bool {
    let key = tid.with("ime");
    let mut composing = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    let mut preedit_now = false;
    ui.input(|i| {
        for e in &i.events {
            match e {
                Event::Ime(egui::ImeEvent::Preedit(s)) => {
                    composing = !s.is_empty();
                    preedit_now |= !s.is_empty();
                }
                Event::Ime(egui::ImeEvent::Commit(_)) | Event::Ime(egui::ImeEvent::Disabled) => composing = false,
                _ => {}
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(key, composing));
    composing || preedit_now
}

/// Enter (ohne Umschalt/Strg/Alt) aus den Eingaben nehmen; Umschalt+Enter bleibt für die neue Zeile im Feld.
fn take_plain_enter(ui: &Ui) -> bool {
    ui.input_mut(|i| {
        let pos = i.events.iter().position(|e| matches!(e, Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command));
        match pos {
            Some(p) => {
                i.events.remove(p);
                true
            }
            None => false,
        }
    })
}

fn input_bar(ui: &mut Ui, session: u64, chat: &mut ChatState, lang: Lang, input_h: f32, grab_focus: bool, out: &mut ChatOut) {
    let streaming = chat.streaming();
    let tid = input_id(session);
    if chat.editing {
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang.t("Letzte Nachricht wird bearbeitet – Senden ersetzt sie.", "Editing the last message – sending replaces it.")).small().color(style::dim(ui)));
            if ui.small_button(lang.t("Abbrechen", "Cancel")).clicked() {
                chat.cancel_edit();
            }
        });
    }
    let focused = ui.memory(|m| m.has_focus(tid));
    // Enter sendet; während einer Antwort tut es nichts (auch keinen Zeilenumbruch). Nicht während einer IME-Eingabe.
    if focused && !ime_composing(ui, tid) && take_plain_enter(ui) && !streaming {
        out.send = true;
    }
    if focused && chat.editing && ui.input(|i| i.key_pressed(Key::Escape)) {
        chat.cancel_edit();
    }
    let avail = ui.available_rect_before_wrap();
    let field = Rect::from_min_max(avail.min, Pos2::new(avail.max.x - BTN_W - S2, avail.min.y + input_h));
    let btn = Rect::from_min_max(Pos2::new(avail.max.x - BTN_W, field.max.y - 30.0), Pos2::new(avail.max.x, field.max.y));
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(field), |ui| {
            egui::ScrollArea::vertical().id_salt(("chat_input_scroll", session)).max_height(input_h).auto_shrink([false, true]).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut chat.draft)
                        .id(tid)
                        .desired_rows(1)
                        .desired_width(f32::INFINITY)
                        .hint_text(lang.t("Nachricht … (Enter sendet, Umschalt+Enter: neue Zeile)", "Message … (Enter sends, Shift+Enter: new line)")),
                )
            })
            .inner
        })
        .inner;
    if grab_focus || chat.focus_input {
        resp.request_focus();
        chat.focus_input = false;
    }
    if resp.has_focus() || resp.clicked() {
        out.focus = true;
    }
    let (label, enabled) = if streaming { (lang.t("Stopp", "Stop"), true) } else { (lang.t("Senden", "Send"), !chat.draft.trim().is_empty()) };
    let b = ui.put(btn, egui::Button::new(RichText::new(label).strong()).fill(if streaming { error_col().gamma_multiply(0.6) } else { style::ACCENT.gamma_multiply(0.7) }));
    if enabled && b.clicked() {
        if streaming {
            out.stop = true;
        } else {
            out.send = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{store::ToolCall, ChatFile, EchoBackend};
    use eframe::egui::{Modifiers, PointerButton, RawInput};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct H {
        ctx: egui::Context,
        t: f64,
        texts: Vec<String>,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl H {
        fn new() -> H {
            H { ctx: egui::Context::default(), t: 0.0, texts: Vec::new(), shapes: Vec::new() }
        }
        fn frame(&mut self, chat: &mut ChatState, lang: Lang, focus: bool, events: Vec<Event>) -> ChatOut {
            self.t += 0.05;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))), time: Some(self.t), events, ..Default::default() };
            let mut res = ChatOut::default();
            let full = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = ui.available_rect_before_wrap();
                    res = chat_body(ui, r, 1, chat, "echo-model", lang, focus);
                });
            });
            self.texts = full.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect();
            self.shapes = full.shapes;
            res
        }
        fn pos_of(&self, text: &str) -> Option<Pos2> {
            self.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect().center()),
                _ => None,
            })
        }
        fn click(&mut self, chat: &mut ChatState, lang: Lang, pos: Pos2) -> ChatOut {
            let btn = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            let mut o = self.frame(chat, lang, false, vec![Event::PointerMoved(pos)]);
            for e in [btn(true), btn(false)] {
                let r = self.frame(chat, lang, false, vec![e]);
                o.send |= r.send;
                o.stop |= r.stop;
                o.regenerate |= r.regenerate;
                o.edit |= r.edit;
            }
            o
        }
    }

    fn key(k: Key, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }

    fn pump(h: &mut H, c: &mut ChatState) {
        let t = Instant::now();
        while c.streaming() && t.elapsed() < Duration::from_secs(5) {
            c.poll();
            h.frame(c, Lang::En, false, vec![]);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!c.streaming());
    }

    #[test]
    fn typing_enter_sends_shift_enter_adds_a_line_and_the_echo_answer_arrives() {
        let backend = EchoBackend { delay: Duration::ZERO };
        let meta = crate::chat::default_chat_meta();
        let mut c = ChatState::new(ChatFile::default(), None);
        let mut h = H::new();
        h.frame(&mut c, Lang::En, true, vec![]);
        assert!(h.texts.iter().any(|t| t == "New chat"), "{:?}", h.texts);
        // Tippen geht ins Feld (fokussiert über grab_focus)
        h.frame(&mut c, Lang::En, false, vec![Event::Text("hello".into())]);
        assert_eq!(c.draft, "hello");
        // Umschalt+Enter: neue Zeile, nicht senden
        let o = h.frame(&mut c, Lang::En, false, vec![key(Key::Enter, Modifiers::SHIFT)]);
        assert!(!o.send);
        assert_eq!(c.draft, "hello\n");
        h.frame(&mut c, Lang::En, false, vec![Event::Text("world".into())]);
        // Enter: senden, kein Zeilenumbruch im Feld
        let o = h.frame(&mut c, Lang::En, false, vec![key(Key::Enter, Modifiers::NONE)]);
        assert!(o.send, "Enter sendet");
        assert_eq!(c.draft, "hello\nworld", "Enter fügt keinen Zeilenumbruch ein");
        assert!(c.send(&backend, &meta, &crate::chat::TurnContext::default(), Arc::new(|| {})));
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "Stop"), "Während der Antwort heißt der Knopf Stopp: {:?}", h.texts);
        pump(&mut h, &mut c);
        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[1].content, EchoBackend::reply(&meta, "hello\nworld"));
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "Send"));
        assert!(h.texts.iter().any(|t| t == "Copy"), "Codeblock der Antwort hat einen Kopierknopf: {:?}", h.texts);
        // ↻ und ✎ melden Neu erzeugen / Bearbeiten
        let p = h.pos_of("↻").expect("Neu-erzeugen-Knopf");
        assert!(h.click(&mut c, Lang::En, p).regenerate);
        let p = h.pos_of("✎").expect("Bearbeiten-Knopf");
        assert!(h.click(&mut c, Lang::En, p).edit);
    }

    #[test]
    fn error_retry_tool_lines_marker_and_german_texts() {
        let mut call = ChatMsg::new(Role::Assistant, "Let me look.");
        call.tool_calls.push(ToolCall { id: "c1".into(), name: "list_sessions".into(), arguments: serde_json::json!({"x": 1}) });
        let mut res = ChatMsg::new(Role::Tool, "{\"sessions\":[]}");
        res.tool_call_id = Some("c1".into());
        res.tool_name = Some("list_sessions".into());
        res.status = Some(ToolStatus::Ok);
        let mut err = ChatMsg::new(Role::Assistant, "");
        err.error = Some("connection refused".into());
        let file = ChatFile { context_start: Some(1), messages: vec![ChatMsg::new(Role::User, "old"), ChatMsg::new(Role::User, "q"), call, res, err], ..ChatFile::default() };
        for (lang, retry, marker) in [(Lang::En, "Retry", "Older messages are no longer visible to the model"), (Lang::De, "Wiederholen", "Ältere Nachrichten sieht das Modell nicht mehr")] {
            let mut c = ChatState::new(file.clone(), Some("warn".into()));
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![]);
            assert!(h.texts.iter().any(|t| t == "connection refused"), "{:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == marker), "{:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == "list_sessions✓" || t.contains("list_sessions")), "Werkzeugzeile: {:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == "warn"));
            assert!(!h.texts.iter().any(|t| t == "{\"sessions\":[]}"), "Ergebnis ist eingeklappt");
            let p = h.pos_of(retry).expect("Wiederholen-Knopf");
            assert!(h.click(&mut c, lang, p).regenerate);
        }
    }

    #[test]
    fn enter_while_streaming_neither_sends_nor_adds_a_line_and_ime_keeps_enter() {
        let backend = EchoBackend { delay: Duration::from_millis(50) };
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "a b c d e f g h".into();
        c.send(&backend, &crate::chat::default_chat_meta(), &crate::chat::TurnContext::default(), Arc::new(|| {}));
        let mut h = H::new();
        h.frame(&mut c, Lang::En, true, vec![]);
        h.frame(&mut c, Lang::En, false, vec![Event::Text("x".into())]);
        let o = h.frame(&mut c, Lang::En, false, vec![key(Key::Enter, Modifiers::NONE)]);
        assert!(!o.send);
        assert_eq!(c.draft, "x", "kein Zeilenumbruch während der Antwort");
        c.stop();
        // IME: Enter während des Vorschautexts gehört der IME
        let o = h.frame(&mut c, Lang::En, false, vec![Event::Ime(egui::ImeEvent::Preedit("ka".into()))]);
        assert!(!o.send);
        let o = h.frame(&mut c, Lang::En, false, vec![key(Key::Enter, Modifiers::NONE)]);
        assert!(!o.send, "Enter übernimmt den IME-Text, sendet nicht");
        let o = h.frame(&mut c, Lang::En, false, vec![Event::Ime(egui::ImeEvent::Commit("か".into()))]);
        assert!(!o.send);
        let o = h.frame(&mut c, Lang::En, false, vec![key(Key::Enter, Modifiers::NONE)]);
        assert!(o.send, "nach dem Übernehmen sendet Enter wieder");
    }

    #[test]
    fn a_frame_with_5000_messages_only_lays_out_the_visible_ones() {
        let mut file = ChatFile::default();
        for i in 0..5000 {
            file.messages.push(ChatMsg::new(if i % 2 == 0 { Role::User } else { Role::Assistant }, format!("m{i}")));
        }
        let mut c = ChatState::new(file, None);
        let mut h = H::new();
        let t = Instant::now();
        for _ in 0..4 {
            h.frame(&mut c, Lang::En, false, vec![]);
        }
        let drawn = h.texts.iter().filter(|t| t.starts_with('m') && t[1..].chars().all(|c| c.is_ascii_digit()) && t.len() > 1).count();
        assert!(drawn > 0 && drawn < 200, "nur sichtbare Nachrichten werden gezeichnet: {drawn}");
        assert!(h.texts.iter().any(|t| t == "m4999"), "nach dem Öffnen steht das Ende im Bild");
        assert!(t.elapsed() < Duration::from_secs(5), "vier Bilder dauerten {:?}", t.elapsed());
        // Zwischenspeicher: nur gezeichnete Antworten sind geparst
        assert!(c.ui.md.len() < 200, "{}", c.ui.md.len());
    }

    #[test]
    fn stop_button_reports_stop_while_streaming() {
        let backend = EchoBackend { delay: Duration::from_millis(50) };
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "a b c d e f g h".into();
        c.send(&backend, &crate::chat::default_chat_meta(), &crate::chat::TurnContext::default(), Arc::new(|| {}));
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        let p = h.pos_of("Stop").expect("Stopp-Knopf");
        assert!(h.click(&mut c, Lang::En, p).stop);
    }

    #[test]
    fn notice_cut_off_note_and_token_usage_are_shown() {
        let mut a = ChatMsg::new(Role::Assistant, "partial answer");
        a.note = Some(crate::chat::llm_backend::cut_off_note(Lang::En));
        let file = ChatFile { messages: vec![ChatMsg::new(Role::User, "q"), a], ..ChatFile::default() };
        for (lang, notice, tokens) in [(Lang::En, "This model cannot use tools – chat only.", "Tokens: 120 in · 45 out"), (Lang::De, "Dieses Modell kann keine Werkzeuge nutzen – nur Chat.", "Tokens: 120 Eingabe · 45 Antwort")] {
            let mut c = ChatState::new(file.clone(), None);
            c.notice = crate::chat::llm_backend::chat_notice(
                &[crate::llm::config::LlmServer { id: "s".into(), last_models: vec![crate::llm::ModelInfo { name: "m".into(), tools: Some(false), context_length: None }], ..Default::default() }],
                &crate::state::ChatMeta { server: "s".into(), model: "m".into() },
                lang,
            );
            c.usage = Some((Some(120), Some(45)));
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![]);
            assert!(h.texts.iter().any(|t| t == notice), "{:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == tokens), "{:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == "⚠ Answer cut off at the model's token limit."), "{:?}", h.texts);
        }
        assert_eq!(usage_text(Some((None, Some(3))), Lang::En).as_deref(), Some("Tokens: 3 out"));
        assert_eq!(usage_text(Some((None, None)), Lang::En), None);
    }
}
