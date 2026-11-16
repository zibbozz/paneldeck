//! Inhalt eines Chat-Bereichs (#63): Nachrichtenliste, Werkzeugzeilen, Eingabe. Kennt kein Backend – Senden, Stopp,
//! Neu erzeugen und Bearbeiten meldet [`chat_body`] zurück; die App führt sie aus (`Action::Chat*`).

use crate::chat::session::{CallState, UiCache};
use crate::chat::{AllowRule, ChatMsg, ChatState, Decision, Role, ToolStatus};
use crate::mcp::McpLevel;
use crate::markdown::{self, RenderOpts};
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Color32, CornerRadius, Event, FontId, Id, Key, Margin, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Ui, UiBuilder, Vec2};

/// Was der Nutzer in diesem Bild ausgelöst hat.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(super) struct ChatOut {
    pub send: bool,
    pub stop: bool,
    pub regenerate: bool,
    pub edit: bool,
    /// In den Bereich geklickt / Eingabe hat den Fokus (Bereich fokussieren).
    pub focus: bool,
    /// Entscheidung zum wartenden Werkzeugaufruf (Kennung des Aufrufs, Entscheidung).
    pub decide: Option<(String, Decision)>,
    /// „Weiter“ nach dem Anhalten (Höchstzahl der Werkzeugaufrufe, Kontext voll).
    pub cont: bool,
    /// Dauerhafte Freigabe zurücknehmen.
    pub revoke: Option<AllowRule>,
    /// Antwort auf die einmalige Frage nach der Stufe des Modells.
    pub level: Option<McpLevel>,
}

/// Höhe der Kopfzeile mit den dauerhaften Freigaben.
const HEAD_H: f32 = 24.0;
/// Sichtbare Zeilen der Parameter in der Freigabe-Zeile (mehr: scrollen, mit Hinweis „+N Zeichen“).
const APPROVAL_ROWS: usize = 12;

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
    let head_h = if chat.file.always_allow.is_empty() { 0.0 } else { HEAD_H };
    let head_rect = Rect::from_min_max(body.min, Pos2::new(body.max.x, body.min.y + head_h));
    let list_rect = Rect::from_min_max(Pos2::new(body.min.x, head_rect.max.y), Pos2::new(body.max.x, (body.max.y - bottom_h).max(head_rect.max.y)));
    let bar_rect = Rect::from_min_max(Pos2::new(body.min.x, list_rect.max.y), body.max);
    if head_h > 0.0 {
        ui.scope_builder(UiBuilder::new().max_rect(head_rect.shrink2(Vec2::new(PAD, 2.0))), |ui| {
            approvals_menu(ui, session, &chat.file.always_allow, lang, &mut out);
        });
    }

    // Trennlinie über der Eingabe
    ui.painter().hline(bar_rect.x_range(), bar_rect.min.y + 0.5, Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7)));

    let bg = ui.interact(list_rect, Id::new(("chat_bg", session)), Sense::click());
    if bg.clicked() {
        out.focus = true;
    }
    ui.scope_builder(UiBuilder::new().max_rect(list_rect.shrink2(Vec2::new(PAD, 0.0))), |ui| {
        messages(ui, session, chat, model, lang, &mut out);
    });
    // Nach einer Entscheidung zurück ins Eingabefeld: der Knopf an derselben Stelle gehört sonst gleich der nächsten
    // Frage, und Enter würde sie freigeben.
    if out.decide.is_some() || out.level.is_some() {
        chat.focus_input = true;
    }
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
    /// Offene Werkzeugaufrufe: Kennung → Zustand.
    pending: std::collections::HashMap<String, CallState>,
    /// Ziel des wartenden Aufrufs (`session 4 "build"`).
    target: Option<String>,
}

fn messages(ui: &mut Ui, session: u64, chat: &mut ChatState, model: &str, lang: Lang, out: &mut ChatOut) {
    let streaming = chat.busy();
    let has_error = chat.has_error();
    let can_regen = !streaming && crate::chat::logic::can_regenerate(chat.messages());
    let rev = chat.rev();
    let from = std::mem::replace(&mut chat.changed_from, usize::MAX);
    let pending = chat.calls.iter().map(|p| (p.call.id.clone(), p.state)).collect();
    let paused = chat.paused.then(|| (chat.steps, chat.pause_note.clone()));
    let level_question = chat.level_question.is_some();
    let target = chat.approval_target.clone();
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
        pending,
        target,
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
        // Angehalten an der Höchstzahl der Werkzeugaufrufe: Hinweis mit „Weiter“ unter der letzten Nachricht.
        if let Some((steps, note)) = paused {
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + y), Vec2::new(width, 1.0e6));
            let mut bar = ui.new_child(UiBuilder::new().id_salt(("chat_paused", session)).max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            paused_bar(&mut bar, steps, note.as_deref(), lang, out);
            y += bar.min_rect().height() + S1;
        }
        // Einmalige Frage nach der Stufe (die Nachricht wartet im Eingabefeld).
        if level_question {
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + y), Vec2::new(width, 1.0e6));
            let mut bar = ui.new_child(UiBuilder::new().id_salt(("chat_level", session)).max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            level_bar(&mut bar, lang, out);
            y += bar.min_rect().height() + S1;
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
            assistant(ui, i, m, msgs, cache, rc, out);
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
        Role::Tool => tool_line(ui, Id::new(("chat_tool_r", rc.session, i)), m.tool_name.as_deref().unwrap_or("?"), None, Some(m), None, None, lang, out),
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
fn assistant(ui: &mut Ui, i: usize, m: &ChatMsg, msgs: &[ChatMsg], cache: &mut UiCache, rc: &RowCx, out: &mut ChatOut) {
    let (session, lang) = (rc.session, rc.lang);
    ui.label(RichText::new(rc.model).small().color(style::dim(ui)));
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
        let state = if result.is_some() { None } else { rc.pending.get(&c.id).copied() };
        let target = if state == Some(CallState::Approval) { rc.target.as_deref() } else { None };
        tool_line(ui, Id::new(("chat_tool", session, i, k)), &c.name, Some((c.id.as_str(), &c.arguments)), result, state, target, lang, out);
    }
}

/// Zeichen und Farbe für den Stand eines Werkzeugaufrufs (`None` = läuft: Kreisel).
fn tool_mark(result: Option<&ChatMsg>, state: Option<CallState>, lang: Lang, dim: Color32) -> Option<(&'static str, Color32)> {
    let ok = Color32::from_rgb(0x4c, 0xc2, 0x6a);
    Some(match (result.map(|r| r.status), state) {
        (Some(Some(ToolStatus::Ok)) | Some(None), _) => ("✓", ok),
        (Some(Some(ToolStatus::Error)), _) => ("✗", error_col()),
        (Some(Some(ToolStatus::Denied)), _) => (lang.t("⊘ abgelehnt", "⊘ denied"), error_col()),
        (None, Some(CallState::Running)) => return None,
        (None, Some(CallState::Approval)) => (lang.t("wartet auf Freigabe", "needs approval"), Color32::from_rgb(0xe5, 0xc0, 0x7b)),
        (None, _) => ("…", dim),
    })
}

/// Eine eingeklappte Zeile je Werkzeugaufruf (`▸ name ✓`), aufgeklappt Parameter und Ergebnis (als JSON eingerückt,
/// gekürzt). Wartet der Aufruf auf Freigabe, stehen darunter Kurzfassung und die Knöpfe Erlauben / Immer / Ablehnen.
#[allow(clippy::too_many_arguments)]
fn tool_line(ui: &mut Ui, id: Id, name: &str, call: Option<(&str, &serde_json::Value)>, result: Option<&ChatMsg>, state: Option<CallState>, target: Option<&str>, lang: Lang, out: &mut ChatOut) {
    let args = call.map(|c| c.1);
    let mark = tool_mark(result, state, lang, style::dim(ui));
    let mut job = egui::text::LayoutJob::default();
    job.append(name, 0.0, egui::TextFormat { font_id: FontId::monospace(12.5), color: ui.visuals().text_color(), ..Default::default() });
    if let Some((mark, col)) = mark {
        job.append(mark, 6.0, egui::TextFormat { font_id: FontId::proportional(12.5), color: col, ..Default::default() });
    }
    ui.horizontal(|ui| {
        egui::CollapsingHeader::new(job).id_salt(id).default_open(false).show(ui, |ui| {
            if let Some(a) = args {
                ui.label(RichText::new(lang.t("Parameter", "Parameters")).small().color(style::dim(ui)));
                code(ui, &serde_json::to_string_pretty(a).unwrap_or_default());
            }
            ui.label(RichText::new(lang.t("Ergebnis", "Result")).small().color(style::dim(ui)));
            match result {
                Some(r) => code(ui, &pretty(&r.content)),
                None => {
                    ui.label(RichText::new(lang.t("(noch kein Ergebnis)", "(no result yet)")).small().color(style::dim(ui)));
                }
            }
        });
        if mark.is_none() {
            ui.add(egui::Spinner::new().size(12.0));
        }
    });
    if let (Some(CallState::Approval), Some((call_id, args))) = (state, call) {
        approval(ui, id, call_id, name, args, target, lang, out);
    }
}

/// Freigabe-Zeile eines schreibenden Aufrufs. Enter gilt nur, wenn ein Knopf hier den Fokus hat (Tab), nie aus der
/// Eingabe heraus – so gibt niemand versehentlich frei, während er tippt.
fn approval(ui: &mut Ui, id: Id, call_id: &str, name: &str, args: &serde_json::Value, target: Option<&str>, lang: Lang, out: &mut ChatOut) {
    let warn = Color32::from_rgb(0xe5, 0xc0, 0x7b);
    egui::Frame::new().fill(warn.gamma_multiply(0.14)).stroke(Stroke::new(1.0_f32, warn.gamma_multiply(0.6))).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(lang.t("Das Modell möchte ein Werkzeug mit Schreibzugriff nutzen:", "The model wants to use a tool with write access:")).small());
        let head = match target {
            Some(t) => format!("{name} → {t}"),
            None => name.to_string(),
        };
        ui.label(RichText::new(head).monospace().strong().size(12.5));
        // Alle Parameter vollständig (scrollbar), damit kein gefährliches Ende verborgen bleibt.
        let text = serde_json::to_string_pretty(args).unwrap_or_default();
        let font = FontId::monospace(12.0);
        let width = (ui.available_width() - 16.0).max(60.0);
        let galley = ui.painter().layout(text.clone(), font.clone(), ui.visuals().text_color(), width);
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let hidden: usize = galley.rows.iter().skip(APPROVAL_ROWS).map(|r| r.char_count_including_newline()).sum();
        egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt(id.with("approval_args")).max_height(row_h * APPROVAL_ROWS as f32 + 2.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).font(font)).wrap_mode(egui::TextWrapMode::Wrap));
            });
        });
        if hidden > 0 {
            let more = if lang == Lang::De { format!("+{hidden} Zeichen – nach unten scrollen, um alles zu sehen") } else { format!("+{hidden} characters – scroll down to see everything") };
            ui.label(RichText::new(more).small().color(warn));
        }
        ui.horizontal_wrapped(|ui| {
            let allow = ui.add(egui::Button::new(RichText::new(lang.t("Erlauben", "Allow")).strong()).fill(style::ACCENT.gamma_multiply(0.7)));
            // „Immer“ nur mit Ziel-Sitzung (Werkzeug + Sitzung); Tickets, Todos, create_session … fragen jedes Mal.
            let call = crate::chat::store::ToolCall { id: call_id.to_string(), name: name.to_string(), arguments: args.clone() };
            let always = AllowRule::eligible(&call).then(|| {
                let session = args.get("session").and_then(|s| s.as_u64()).unwrap_or(0);
                let scope = format!("{name} → {}", target.map(str::to_string).unwrap_or_else(|| format!("session {session}")));
                ui.button(lang.t("Immer für diesen Chat", "Always for this chat")).on_hover_text(if lang == Lang::De {
                    format!("Immer für {scope} (oben im Chat widerrufbar)")
                } else {
                    format!("Always for {scope} (can be revoked at the top of the chat)")
                })
            });
            let deny = ui.button(lang.t("Ablehnen", "Deny"));
            let d = if allow.clicked() {
                Some(Decision::Allow)
            } else if always.is_some_and(|a| a.clicked()) {
                Some(Decision::Always)
            } else if deny.clicked() {
                Some(Decision::Deny)
            } else {
                None
            };
            if let Some(d) = d {
                out.decide = Some((call_id.to_string(), d));
            }
        });
    });
}

/// Angehalten: „Nach N Werkzeugaufrufen angehalten“ (oder der Grund, z. B. Kontext voll) mit dem Knopf „Weiter“.
fn paused_bar(ui: &mut Ui, steps: u32, note: Option<&str>, lang: Lang, out: &mut ChatOut) {
    egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let text = match note {
                Some(n) => n.to_string(),
                None if lang == Lang::De => format!("Nach {steps} Werkzeugaufrufen angehalten."),
                None => format!("Stopped after {steps} tool calls."),
            };
            ui.label(RichText::new(text).color(style::dim(ui)));
            if ui.button(lang.t("Weiter", "Continue")).on_hover_text(lang.t("Noch einmal so viele Werkzeugaufrufe erlauben", "Allow the same number of tool calls again")).clicked() {
                out.cont = true;
            }
        });
    });
}

/// Einmalige Frage vor dem ersten Senden: Darf dieses Modell Paneldeck nutzen? (Aus / Lesen / Steuern.)
fn level_bar(ui: &mut Ui, lang: Lang, out: &mut ChatOut) {
    let warn = Color32::from_rgb(0xe5, 0xc0, 0x7b);
    egui::Frame::new().fill(warn.gamma_multiply(0.14)).stroke(Stroke::new(1.0_f32, warn.gamma_multiply(0.6))).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(lang.t("Darf dieses Modell Paneldeck nutzen?", "May this model use Paneldeck?")).strong());
        ui.label(
            RichText::new(lang.t(
                "Aus = nur Chat · Lesen = Terminals, Sitzungen, Todos lesen · Steuern = zusätzlich in Terminals tippen, Sitzungen anlegen … (Schreibaktionen fragen vorher)",
                "Off = chat only · Read = read terminals, sessions, to-dos · Control = also type into terminals, create sessions … (write actions ask first)",
            ))
            .small()
            .color(style::dim(ui)),
        );
        ui.horizontal_wrapped(|ui| {
            for (l, de, en) in [(McpLevel::Off, "Aus", "Off"), (McpLevel::Read, "Lesen", "Read"), (McpLevel::Control, "Steuern", "Control")] {
                if ui.button(lang.t(de, en)).clicked() {
                    out.level = Some(l);
                }
            }
            ui.label(RichText::new(lang.t("(später änderbar in Einstellungen → Modelle)", "(can be changed later in Settings → Models)")).small().color(style::dim(ui)));
        });
    });
}

/// Kopfzeile: „Dauerhafte Freigaben (N)“ mit „Widerrufen“ je Eintrag.
fn approvals_menu(ui: &mut Ui, session: u64, rules: &[AllowRule], lang: Lang, out: &mut ChatOut) {
    ui.horizontal(|ui| {
        let title = if lang == Lang::De { format!("Dauerhafte Freigaben ({})", rules.len()) } else { format!("Permanent approvals ({})", rules.len()) };
        ui.push_id(("chat_approvals", session), |ui| {
            ui.menu_button(RichText::new(title).small(), |ui| {
                for r in rules {
                    ui.horizontal(|ui| {
                        let what = match r.session {
                            Some(s) => format!("{} → session {s}", r.tool),
                            None => r.tool.clone(),
                        };
                        ui.label(RichText::new(what).monospace().size(12.0));
                        if ui.small_button(lang.t("Widerrufen", "Revoke")).clicked() {
                            out.revoke = Some(r.clone());
                        }
                    });
                }
            });
        });
    });
}

/// JSON-Text eingerückt (anderes unverändert).
fn pretty(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or_else(|| text.to_string())
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
    // Auch während Werkzeuge laufen oder auf Freigabe warten: Stopp statt Senden.
    let streaming = chat.busy();
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

    /// Chat, dessen Antwort `send_text` verlangt und der nun auf Freigabe wartet.
    fn awaiting() -> ChatState {
        awaiting_with(vec![ToolCall { id: "w".into(), name: "send_text".into(), arguments: serde_json::json!({"session": 4, "text": "dir"}) }])
    }

    fn control() -> crate::chat::tools::ToolPolicy {
        crate::chat::tools::ToolPolicy::new(crate::llm::config::LlmModel { level: crate::mcp::McpLevel::Control, level_chosen: true, ..crate::llm::config::LlmModel::named("m") }, None, false)
    }

    /// Chat, dessen Antwort diese Aufrufe verlangt; der erste wartet auf Freigabe.
    fn awaiting_with(calls: Vec<ToolCall>) -> ChatState {
        struct Calls(Vec<ToolCall>);
        impl crate::chat::ChatBackend for Calls {
            fn start(&self, _: &crate::state::ChatMeta, _: &[ChatMsg], _: &crate::chat::TurnContext, _: crate::chat::Repaint) -> crate::chat::backend::TurnHandle {
                let (h, tx, _) = crate::chat::backend::TurnHandle::pair();
                for c in &self.0 {
                    tx.send(crate::chat::backend::TurnEvent::ToolCall(c.clone())).unwrap();
                }
                tx.send(crate::chat::backend::TurnEvent::Done).unwrap();
                h
            }
        }
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "type dir in 4".into();
        assert!(c.send(&Calls(calls), &crate::state::ChatMeta::default(), &crate::chat::TurnContext::default(), Arc::new(|| {})));
        c.poll();
        assert_eq!(c.tool_step(&control()), crate::chat::ToolStep::Idle);
        assert!(c.awaiting_approval());
        c
    }

    fn click_decide(h: &mut H, c: &mut ChatState, lang: Lang, label: &str) -> Option<(String, Decision)> {
        let p = h.pos_of(label).unwrap_or_else(|| panic!("{label} fehlt: {:?}", h.texts));
        let mv = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let mut got = None;
        for evs in [vec![Event::PointerMoved(p)], vec![mv(true)], vec![mv(false)]] {
            got = got.or(h.frame(c, lang, false, evs).decide);
        }
        got
    }

    #[test]
    fn approval_shows_full_arguments_target_and_hidden_length() {
        // Lang genug für mehr als zwölf umbrochene Zeilen im Freigabe-Feld
        let long: String = (0..400).map(|i| format!("echo line {i}\n")).collect();
        let mut c = awaiting_with(vec![ToolCall { id: "w".into(), name: "send_text".into(), arguments: serde_json::json!({"session": 4, "text": format!("{long}rm -rf C:\\important")}) }]);
        c.approval_target = Some("session 4 \"build\"".into());
        for lang in [Lang::En, Lang::De] {
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![]);
            assert!(h.texts.iter().any(|t| t == "send_text → session 4 \"build\""), "Ziel mit Namen: {:?}", h.texts);
            // Volle Parameter (auch das gefährliche Ende), ungekürzt im scrollbaren Bereich
            assert!(h.texts.iter().any(|t| t.contains("rm -rf C:\\\\important")), "{:?}", h.texts.iter().map(|t| t.chars().take(40).collect::<String>()).collect::<Vec<_>>());
            let marker = if lang == Lang::De { "Zeichen – nach unten scrollen" } else { "characters – scroll down" };
            assert!(h.texts.iter().any(|t| t.starts_with('+') && t.contains(marker)), "Hinweis auf verborgene Länge");
        }
        // Kurze Parameter: kein Hinweis
        let mut c = awaiting();
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(!h.texts.iter().any(|t| t.contains("characters – scroll down")));
        assert!(h.texts.iter().any(|t| t.contains("\"text\": \"dir\"")), "{:?}", h.texts);
    }

    #[test]
    fn always_is_offered_only_for_calls_with_a_target_session() {
        let cases = [
            (ToolCall { id: "a".into(), name: "create_session".into(), arguments: serde_json::json!({"name": "x"}) }, false),
            (ToolCall { id: "b".into(), name: "add_todo".into(), arguments: serde_json::json!({"title": "t"}) }, false),
            (ToolCall { id: "c".into(), name: "send_keys".into(), arguments: serde_json::json!({"session": 4, "keys": ["enter"]}) }, true),
        ];
        for (call, offered) in cases {
            let mut c = awaiting_with(vec![call.clone()]);
            let mut h = H::new();
            h.frame(&mut c, Lang::En, false, vec![]);
            h.frame(&mut c, Lang::En, false, vec![]);
            assert!(h.texts.iter().any(|t| t == "Allow") && h.texts.iter().any(|t| t == "Deny"), "{}", call.name);
            assert_eq!(h.texts.iter().any(|t| t == "Always for this chat"), offered, "{}", call.name);
        }
        // Ein „Immer“ ohne Ziel (z. B. veralteter Klick) wirkt wie „Erlauben“ und speichert nichts
        let mut c = awaiting_with(vec![ToolCall { id: "t".into(), name: "add_todo".into(), arguments: serde_json::json!({"title": "t"}) }]);
        assert!(c.decide("t", Decision::Always));
        assert!(c.file.always_allow.is_empty());
    }

    #[test]
    fn enter_right_after_allow_does_not_approve_the_next_call() {
        let st = |id: &str| ToolCall { id: id.into(), name: "send_text".into(), arguments: serde_json::json!({"session": 4, "text": id}) };
        let mut c = awaiting_with(vec![st("one"), st("two")]);
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        let got = click_decide(&mut h, &mut c, Lang::En, "Allow");
        assert_eq!(got, Some(("one".to_string(), Decision::Allow)));
        // Hub: Entscheidung anwenden, ersten ausführen, der zweite wartet nun an derselben Stelle
        assert!(c.decide("one", Decision::Allow));
        let crate::chat::ToolStep::Execute(_) = c.tool_step(&control()) else { panic!() };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(Ok(serde_json::json!({"ok": true}))).unwrap();
        c.tool_started(rx);
        c.poll_tools();
        assert_eq!(c.tool_step(&control()), crate::chat::ToolStep::Idle);
        assert_eq!(c.pending_approval().map(|p| p.id.as_str()), Some("two"));
        h.frame(&mut c, Lang::En, false, vec![]);
        // Enter sofort danach: gibt den zweiten nicht frei (Fokus ist zurück im Eingabefeld)
        let o = h.frame(&mut c, Lang::En, false, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(o.decide, None);
        let o = h.frame(&mut c, Lang::En, false, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(o.decide, None);
        assert!(h.ctx.memory(|m| m.has_focus(input_id(1))), "Eingabefeld hat den Fokus");
    }

    #[test]
    fn level_question_and_permanent_approvals_menu() {
        for (lang, q, off, read, ctrl) in [(Lang::En, "May this model use Paneldeck?", "Off", "Read", "Control"), (Lang::De, "Darf dieses Modell Paneldeck nutzen?", "Aus", "Lesen", "Steuern")] {
            for (label, want) in [(off, McpLevel::Off), (read, McpLevel::Read), (ctrl, McpLevel::Control)] {
                let mut c = ChatState::new(ChatFile::default(), None);
                c.draft = "hello".into();
                c.level_question = Some(crate::chat::LevelAsker::User);
                let mut h = H::new();
                h.frame(&mut c, lang, false, vec![]);
                h.frame(&mut c, lang, false, vec![]);
                assert!(h.texts.iter().any(|t| t == q), "{:?}", h.texts);
                let p = h.pos_of(label).unwrap();
                let mv = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
                let mut got = None;
                for evs in [vec![Event::PointerMoved(p)], vec![mv(true)], vec![mv(false)]] {
                    got = got.or(h.frame(&mut c, lang, false, evs).level);
                }
                assert_eq!(got, Some(want), "{label}");
                assert_eq!(c.draft, "hello", "die Nachricht wartet");
            }
        }
        // Dauerhafte Freigaben: Menü in der Kopfzeile, „Widerrufen“ je Eintrag
        for (lang, title, revoke) in [(Lang::En, "Permanent approvals (2)", "Revoke"), (Lang::De, "Dauerhafte Freigaben (2)", "Widerrufen")] {
            let rules = vec![AllowRule { tool: "send_text".into(), session: Some(4) }, AllowRule { tool: "add_todo".into(), session: None }];
            let mut c = ChatState::new(ChatFile { always_allow: rules.clone(), ..ChatFile::default() }, None);
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            let p = h.pos_of(title).unwrap_or_else(|| panic!("{title}: {:?}", h.texts));
            let mv = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            for evs in [vec![Event::PointerMoved(p)], vec![mv(true)], vec![mv(false)]] {
                h.frame(&mut c, lang, false, evs);
            }
            h.frame(&mut c, lang, false, vec![]);
            assert!(h.texts.iter().any(|t| t == "send_text → session 4"), "{:?}", h.texts);
            let p = h.pos_of(revoke).expect("Widerrufen-Knopf");
            let mv = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            let mut got = None;
            for evs in [vec![Event::PointerMoved(p)], vec![mv(true)], vec![mv(false)]] {
                got = got.or(h.frame(&mut c, lang, false, evs).revoke);
            }
            assert_eq!(got, Some(rules[0].clone()));
        }
    }

    #[test]
    fn approval_buttons_in_english_and_german_and_enter_in_the_input_does_not_approve() {
        for (lang, allow, always, deny, hint) in [
            (Lang::En, "Allow", "Always for this chat", "Deny", "The model wants to use a tool with write access:"),
            (Lang::De, "Erlauben", "Immer für diesen Chat", "Ablehnen", "Das Modell möchte ein Werkzeug mit Schreibzugriff nutzen:"),
        ] {
            for (label, want) in [(allow, Decision::Allow), (always, Decision::Always), (deny, Decision::Deny)] {
                let mut c = awaiting();
                let mut h = H::new();
                h.frame(&mut c, lang, true, vec![]);
                h.frame(&mut c, lang, false, vec![]);
                assert!(h.texts.iter().any(|t| t == hint), "{:?}", h.texts);
                assert!(h.texts.iter().any(|t| t == "send_text"), "Werkzeug: {:?}", h.texts);
                assert!(h.texts.iter().any(|t| t == if lang == Lang::De { "Stopp" } else { "Stop" }), "während der Frage: Stopp");
                // Enter im (fokussierten) Eingabefeld gibt nicht frei
                h.frame(&mut c, lang, false, vec![Event::Text("x".into())]);
                let o = h.frame(&mut c, lang, false, vec![key(Key::Enter, Modifiers::NONE)]);
                assert_eq!((o.decide, o.send), (None, false));
                let got = click_decide(&mut h, &mut c, lang, label);
                assert_eq!(got, Some(("w".to_string(), want)), "{label}");
            }
        }
    }

    #[test]
    fn paused_bar_offers_continue_and_tool_marks() {
        for (lang, text, btn) in [(Lang::En, "Stopped after 10 tool calls.", "Continue"), (Lang::De, "Nach 10 Werkzeugaufrufen angehalten.", "Weiter")] {
            let mut c = ChatState::new(ChatFile { messages: vec![ChatMsg::new(Role::User, "q"), ChatMsg::new(Role::Assistant, "a")], ..ChatFile::default() }, None);
            c.paused = true;
            c.steps = 10;
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![]);
            assert!(h.texts.iter().any(|t| t == text), "{:?}", h.texts);
            let p = h.pos_of(btn).expect("Weiter-Knopf");
            let mv = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            let mut cont = false;
            for evs in [vec![Event::PointerMoved(p)], vec![mv(true)], vec![mv(false)]] {
                cont |= h.frame(&mut c, lang, false, evs).cont;
            }
            assert!(cont);
        }
        let dim = Color32::GRAY;
        let mut r = ChatMsg::new(Role::Tool, "{}");
        r.status = Some(ToolStatus::Denied);
        assert_eq!(tool_mark(Some(&r), None, Lang::En, dim).unwrap().0, "⊘ denied");
        assert!(tool_mark(None, Some(CallState::Running), Lang::En, dim).is_none(), "läuft: Kreisel");
        assert_eq!(tool_mark(None, Some(CallState::Approval), Lang::De, dim).unwrap().0, "wartet auf Freigabe");
        assert_eq!(pretty("{\"a\":1}"), "{\n  \"a\": 1\n}");
        assert_eq!(pretty("not json"), "not json");
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
