//! Inhalt eines Chat-Bereichs (#63): Nachrichtenliste, Werkzeugzeilen, Eingabe. Kennt kein Backend – Senden, Stopp,
//! Neu erzeugen und Bearbeiten meldet [`chat_body`] zurück; die App führt sie aus (`Action::Chat*`).

use crate::afford::Clickable;
use crate::chat::agent::files::Preview;
use crate::chat::session::{CallState, UiCache};
use crate::chat::store::{ChatTodo, TodoStatus};
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
    /// Jetzt kompaktieren (#359, Knopf in der Kopfzeile).
    pub compact: bool,
}

/// Höhe der Kopfzeile (Füllstand des Kontexts, dauerhafte Freigaben).
const HEAD_H: f32 = 24.0;
/// Breite des kleinen Füllstandsbalkens in der Kopfzeile.
const GAUGE_W: f32 = 48.0;
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
    // Kopfzeile: immer da (Füllstand des Kontexts, #339), links ggf. die To-dos des Modells (#338) und die dauerhaften
    // Freigaben. Die Zeile hat ihren eigenen Platz: so hoch wie ein Knopf (der To-do-Chip ist ein Menüknopf), der
    // Verlauf beginnt darunter und nichts aus der Kopfzeile ragt in ihn hinein (sonst lag der Chip beim Scrollen über
    // dem Text).
    let head_h = head_height(ui);
    let head_rect = Rect::from_min_max(body.min, Pos2::new(body.max.x, (body.min.y + head_h).min(body.max.y)));
    let list_rect = Rect::from_min_max(Pos2::new(body.min.x, head_rect.max.y), Pos2::new(body.max.x, (body.max.y - bottom_h).max(head_rect.max.y)));
    let bar_rect = Rect::from_min_max(Pos2::new(body.min.x, list_rect.max.y), body.max);
    ui.scope_builder(UiBuilder::new().max_rect(head_rect.shrink2(Vec2::new(PAD, 2.0))), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(head_rect));
        ui.horizontal_centered(|ui| {
            if !chat.file.todos.is_empty() {
                todos_menu(ui, session, &chat.file.todos, lang);
            }
            if !chat.file.always_allow.is_empty() {
                approvals_menu(ui, session, &chat.file.always_allow, lang, &mut out);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                context_gauge(ui, session, chat, lang);
                if !chat.messages().is_empty() {
                    compact_button(ui, chat, lang, &mut out);
                }
            });
        });
    });

    // Trennlinie über der Eingabe
    ui.painter().hline(bar_rect.x_range(), bar_rect.min.y + 0.5, Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7)));

    // Klick in den Verlauf fokussiert nur die Eingabe – kein Knopf, darum ohne Hand (#187).
    let bg = ui.interact(list_rect, Id::new(("chat_bg", session)), Sense::click()).no_pointer();
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

/// Höhe der Kopfzeile: mindestens [`HEAD_H`], sonst ein Knopf (Mindesthöhe der Bedienelemente) plus Rand.
fn head_height(ui: &Ui) -> f32 {
    HEAD_H.max(ui.spacing().interact_size.y + 6.0)
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
    /// Diff-Vorschau des wartenden Aufrufs (Kennung, Vorschau).
    preview: Option<(String, Preview)>,
    /// Text unter einer noch leeren Antwort („…“, „Denkt nach …“, „Warte auf das Modell … 12 s“).
    waiting: String,
    /// Kompaktierte Originale werden gezeigt (#359, „Original anzeigen“), Schalter dafür in den egui-Daten.
    show_original: bool,
    show_id: Id,
    /// Erste kompaktierte Nachricht (darüber steht aufgeklappt die Überschrift der Originale).
    first_compacted: Option<usize>,
}

/// Ab so vielen Sekunden ohne Daten zeigt eine leere Antwort, wie lange schon gewartet wird (#299).
const WAIT_SHOW_SECS: u64 = 3;
/// Ab so vielen Sekunden kommt der Hinweis dazu, dass der Server das Modell vermutlich noch lädt. Ollama zeigt ein
/// Modell erst nach dem Laden in `ollama ps`; das erste Laden kann deutlich länger dauern (#299: 18 s, dann
/// abgebrochen, weil nichts zu sehen war).
const WAIT_LOAD_HINT_SECS: u64 = 10;

/// Text unter einer noch leeren Antwort: „Denkt nach …“ bei Denk-Ausgabe, sonst ab [`WAIT_SHOW_SECS`] die Wartezeit
/// (das erste Token kann dauern, z. B. während Ollama das Modell lädt), ab [`WAIT_LOAD_HINT_SECS`] mit Hinweis aufs
/// Laden, davor „…“ (#299).
pub(super) fn waiting_text(thinking: bool, waited: Option<std::time::Duration>, lang: Lang) -> String {
    if thinking {
        return lang.t("Denkt nach …", "Thinking …").to_string();
    }
    match waited.map(|d| d.as_secs()) {
        Some(s) if s >= WAIT_LOAD_HINT_SECS => match lang {
            Lang::De => format!("Warte auf das Modell … {s} s – der Server lädt es vermutlich noch (das erste Laden kann länger dauern)"),
            Lang::En => format!("Waiting for the model … {s} s – the server is probably still loading it (the first load can take a while)"),
        },
        Some(s) if s >= WAIT_SHOW_SECS => match lang {
            Lang::De => format!("Warte auf das Modell … {s} s"),
            Lang::En => format!("Waiting for the model … {s} s"),
        },
        _ => "…".to_string(),
    }
}

/// Wartet ein Aufruf mit dieser Kennung auf Freigabe?
fn pending_has(calls: &[crate::chat::session::PendingCall], id: &str) -> bool {
    calls.iter().any(|p| p.call.id == id && p.state == CallState::Approval)
}

fn messages(ui: &mut Ui, session: u64, chat: &mut ChatState, model: &str, lang: Lang, out: &mut ChatOut) {
    let streaming = chat.busy();
    let waiting = waiting_text(chat.thinking, chat.turn_started.filter(|_| chat.streaming()).map(|t| t.elapsed()), lang);
    let still_empty = chat.messages().last().is_some_and(|m| m.role == Role::Assistant && m.content.is_empty() && m.tool_calls.is_empty());
    if chat.streaming() && !chat.thinking && still_empty {
        // Wartezeit sekündlich nachführen (auch ohne neue Daten) – nur solange die Antwort noch leer ist.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    let has_error = chat.has_error();
    let can_regen = !streaming && crate::chat::logic::can_regenerate(chat.messages());
    let rev = chat.rev();
    let from = std::mem::replace(&mut chat.changed_from, usize::MAX);
    let pending = chat.calls.iter().map(|p| (p.call.id.clone(), p.state)).collect();
    // Während einer Zusammenfassung (#359) kein „Weiter“: die Runde geht erst danach weiter.
    let paused = (chat.paused && !chat.compact.running()).then(|| (chat.steps, chat.pause_note.clone(), chat.loop_stop));
    let level_question = chat.level_question.is_some();
    let target = chat.approval_target.clone();
    let preview = chat.approval_preview.clone().filter(|(id, _)| pending_has(&chat.calls, id));
    let compacting = chat.compact.elapsed();
    if compacting.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    let show_id = Id::new(("chat_show_original", session));
    let show_original = ui.data(|d| d.get_temp::<bool>(show_id)).unwrap_or(false);
    let ChatState { file, ui: cache, warning, follow, notice, usage, compact, .. } = chat;
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
        waiting,
        pending,
        target,
        preview,
        show_original,
        show_id,
        first_compacted: msgs.iter().position(|m| m.compacted),
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
            egui::Frame::new().fill(crate::style::ui().chat_approval_bg).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
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
            // Kompaktierte Originale (#359) nur aufgeklappt; sonst belegen sie keinen Platz.
            if m.compacted && !rc.show_original {
                continue;
            }
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
        if let Some((steps, note, loop_stop)) = paused {
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + y), Vec2::new(width, 1.0e6));
            let mut bar = ui.new_child(UiBuilder::new().id_salt(("chat_paused", session)).max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            paused_bar(&mut bar, steps, note.as_deref(), loop_stop, lang, out);
            y += bar.min_rect().height() + S1;
        }
        // Kompaktieren (#359): läuft gerade bzw. schlug fehl.
        if let Some(d) = compacting {
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + y), Vec2::new(width, 1.0e6));
            let mut bar = ui.new_child(UiBuilder::new().id_salt(("chat_compacting", session)).max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            let dim = style::dim(&bar);
            bar.label(RichText::new(compacting_text(d.as_secs(), lang)).color(dim));
            y += bar.min_rect().height() + S1;
        }
        if let Some(f) = compact.failure.clone() {
            let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y + y), Vec2::new(width, 1.0e6));
            let mut bar = ui.new_child(UiBuilder::new().id_salt(("chat_compact_failed", session)).max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            if compact_failed_bar(&mut bar, &f, lang) {
                compact.failure = None;
            }
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
    if rc.first_compacted == Some(i) {
        original_marker(ui, lang);
    }
    match m.role {
        Role::User => user_bubble(ui, m, Some(i) == rc.last_user && !rc.streaming, lang, out),
        Role::Assistant => {
            let is_last = Some(i) == rc.last;
            assistant(ui, i, m, msgs, cache, rc, out);
            if is_last && rc.streaming && m.content.is_empty() && m.tool_calls.is_empty() {
                ui.label(RichText::new(&rc.waiting).color(style::dim(ui)));
            }
            if let Some(n) = &m.note {
                ui.label(RichText::new(format!("⚠ {n}")).small().color(crate::style::ui().warning));
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
        Role::Tool => tool_line(ui, Id::new(("chat_tool_r", rc.session, i)), m.tool_name.as_deref().unwrap_or("?"), None, Some(m), None, (None, None), None, lang, out),
        Role::System if m.summary.is_some() => summary_card(ui, i, m, cache, rc),
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
    crate::style::ui().error
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

/// Über den aufgeklappten Originalen (#359): sie gehen nicht mehr an das Modell.
fn original_marker(ui: &mut Ui, lang: Lang) {
    ui.label(RichText::new(lang.t("Ursprünglicher Verlauf – kompaktiert, wird nicht mehr an das Modell gesendet", "Original history – compacted, no longer sent to the model")).small().strong().color(style::dim(ui)));
    ui.add_space(S1);
}

/// Text während des Kompaktierens.
pub(super) fn compacting_text(secs: u64, lang: Lang) -> String {
    match lang {
        Lang::De => format!("Kompaktiere den Chat – das Modell schreibt die Zusammenfassung … {secs} s"),
        Lang::En => format!("Compacting the chat – the model is writing the summary … {secs} s"),
    }
}

/// Zusammenfassung einer Kompaktierung (#359): Kopf mit Zahl der ersetzten Nachrichten und „Original anzeigen“, darunter
/// der Text, den das Modell künftig statt der Originale sieht. Ältere (selbst kompaktierte) Zusammenfassungen erscheinen
/// nur aufgeklappt und ohne Knopf.
fn summary_card(ui: &mut Ui, i: usize, m: &ChatMsg, cache: &mut UiCache, rc: &RowCx) {
    let lang = rc.lang;
    let n = m.summary.as_ref().map_or(0, |s| s.messages);
    let accent = crate::style::ui().accent;
    egui::Frame::new().fill(ui.visuals().faint_bg_color).stroke(Stroke::new(1.0_f32, accent.gamma_multiply(0.5))).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let head = match (m.compacted, lang) {
                (true, Lang::De) => format!("Frühere Zusammenfassung ({n} Nachrichten)"),
                (true, Lang::En) => format!("Earlier summary ({n} messages)"),
                (false, Lang::De) => format!("Kompaktiert – {n} ältere Nachrichten zusammengefasst"),
                (false, Lang::En) => format!("Compacted – {n} older messages summarised"),
            };
            ui.label(RichText::new(head).strong());
            if !m.compacted {
                let label = if rc.show_original { lang.t("Original ausblenden", "Hide original") } else { lang.t("Original anzeigen", "Show original") };
                let tip = lang.t(
                    "Die ursprünglichen Nachrichten bleiben im Chat gespeichert, gehen aber nicht mehr an das Modell.",
                    "The original messages stay saved in the chat but are no longer sent to the model.",
                );
                if ui.small_button(label).on_hover_text(tip).clicked() {
                    ui.data_mut(|d| d.insert_temp(rc.show_id, !rc.show_original));
                }
            }
        });
        let omitted = m.summary.as_ref().map_or(0, |s| s.omitted);
        if omitted > 0 {
            let text = match lang {
                Lang::De => format!("{omitted} der ältesten Nachrichten passten nicht mehr in die Anfrage und fehlen in der Zusammenfassung."),
                Lang::En => format!("{omitted} of the oldest messages no longer fitted into the request and are missing from the summary."),
            };
            ui.label(RichText::new(format!("⚠ {text}")).small().color(crate::style::ui().warning));
        }
        ui.label(RichText::new(lang.t("Das sieht das Modell statt der älteren Nachrichten:", "The model sees this instead of the older messages:")).small().color(style::dim(ui)));
        let doc = cache.md.entry(i).or_insert_with(|| markdown::parse_plain(&m.content));
        let opts = RenderOpts { soft_breaks_as_newlines: true, copy_button: Some(lang.t("Kopieren", "Copy")) };
        let o = markdown::render(ui, doc, &opts);
        if let Some(u) = o.clicked_url {
            ui.ctx().open_url(egui::OpenUrl::new_tab(u));
        }
    });
}

/// Hinweis nach einem gescheiterten Kompaktieren (#359) mit „Ausblenden“. `true` = ausgeblendet.
fn compact_failed_bar(ui: &mut Ui, text: &str, lang: Lang) -> bool {
    let warn = crate::style::ui().warning;
    let mut dismiss = false;
    egui::Frame::new().fill(warn.gamma_multiply(0.14)).stroke(Stroke::new(1.0_f32, warn.gamma_multiply(0.6))).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("⚠ {text}"));
            dismiss = ui.small_button(lang.t("Ausblenden", "Dismiss")).clicked();
        });
    });
    dismiss
}

/// Kopfzeile: „Kompaktieren“ (#359) – aktiv, wenn es vor den letzten zwei Runden etwas zusammenzufassen gibt und keine
/// Runde läuft. Im Layout von rechts nach links aufrufen.
fn compact_button(ui: &mut Ui, chat: &ChatState, lang: Lang, out: &mut ChatOut) {
    let can = chat.can_compact();
    let tip = if chat.compact.running() {
        lang.t("Kompaktiert gerade …", "Compacting …")
    } else if chat.busy() || chat.editing {
        lang.t("Gerade nicht möglich – erst die laufende Antwort abwarten oder stoppen.", "Not possible right now – wait for the running answer or stop it first.")
    } else if can {
        lang.t(
            "Ältere Nachrichten durch eine Zusammenfassung des Modells ersetzen. Die letzten zwei Runden, offene To-dos und berührte Dateien bleiben wörtlich; die Originale bleiben im Chat. Auch mit /compact.",
            "Replace older messages with a summary written by the model. The last two rounds, open to-dos and touched files are kept as they are; the originals stay in the chat. Also with /compact.",
        )
    } else {
        lang.t("Noch nichts zu kompaktieren – die letzten zwei Runden bleiben immer wörtlich erhalten.", "Nothing to compact yet – the last two rounds are always kept as they are.")
    };
    let r = ui.add_enabled(can, egui::Button::new(RichText::new(lang.t("Kompaktieren", "Compact")).small())).on_hover_text(tip).on_disabled_hover_text(tip);
    if r.clicked() {
        out.compact = true;
    }
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
    ui.painter().rect_filled(bubble, 10.0, crate::style::ui().chat_user_bg);
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
        let preview = rc.preview.as_ref().filter(|(id, _)| state == Some(CallState::Approval) && *id == c.id).map(|(_, p)| p);
        let note = m.repairs.iter().find(|r| r.id == c.id).and_then(|r| repair_note(r, lang));
        tool_line(ui, Id::new(("chat_tool", session, i, k)), &c.name, Some((c.id.as_str(), &c.arguments)), result, state, (target, preview), note.as_deref(), lang, out);
    }
}

/// Dezenter Hinweis am Werkzeug-Block, wenn Paneldeck den Aufruf repariert hat (#340): „als Textaufruf erkannt“ und die
/// korrigierten Argumente (`lines: "3" → 3`). `None` ohne Reparatur.
pub(super) fn repair_note(r: &crate::chat::store::CallRepair, lang: Lang) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if r.text_call {
        parts.push(lang.t("als Textaufruf erkannt", "recognised as a text call").to_string());
    }
    parts.extend(r.fixes.iter().cloned());
    (!parts.is_empty()).then(|| format!("{} {}", lang.t("Von Paneldeck repariert:", "Repaired by Paneldeck:"), parts.join(" · ")))
}

/// Zeichen und Farbe für den Stand eines Werkzeugaufrufs (`None` = läuft: Kreisel).
fn tool_mark(result: Option<&ChatMsg>, state: Option<CallState>, lang: Lang, dim: Color32) -> Option<(&'static str, Color32)> {
    let ok = crate::style::ui().success;
    Some(match (result.map(|r| r.status), state) {
        (Some(Some(ToolStatus::Ok)) | Some(None), _) => ("✓", ok),
        (Some(Some(ToolStatus::Error)), _) => ("✗", error_col()),
        (Some(Some(ToolStatus::Denied)), _) => (lang.t("⊘ abgelehnt", "⊘ denied"), error_col()),
        (None, Some(CallState::Running)) => return None,
        (None, Some(CallState::Approval)) => (lang.t("wartet auf Freigabe", "needs approval"), crate::style::ui().warning),
        (None, _) => ("…", dim),
    })
}

/// Eine eingeklappte Zeile je Werkzeugaufruf (`▸ name ✓`), aufgeklappt Parameter und Ergebnis (als JSON eingerückt,
/// gekürzt). Wartet der Aufruf auf Freigabe, stehen darunter Kurzfassung und die Knöpfe Erlauben / Immer / Ablehnen.
/// `note`: dezenter Hinweis unter der Zeile, wenn Paneldeck den Aufruf repariert hat ([`repair_note`]).
/// `pending` = (Ziel, Diff-Vorschau) des wartenden Aufrufs.
#[allow(clippy::too_many_arguments)]
fn tool_line(ui: &mut Ui, id: Id, name: &str, call: Option<(&str, &serde_json::Value)>, result: Option<&ChatMsg>, state: Option<CallState>, pending: (Option<&str>, Option<&Preview>), note: Option<&str>, lang: Lang, out: &mut ChatOut) {
    let (target, preview) = pending;
    let args = call.map(|c| c.1);
    let mark = tool_mark(result, state, lang, style::dim(ui));
    let mut job = egui::text::LayoutJob::default();
    // Ein Aufruf ohne erkennbaren Namen (als Text geschrieben) heißt „?“.
    let name = if name.trim().is_empty() { "?" } else { name };
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
    if let Some(n) = note {
        ui.label(RichText::new(n).small().color(crate::style::ui().text_dim));
    }
    if let (Some(CallState::Approval), Some((call_id, args))) = (state, call) {
        approval(ui, id, call_id, name, args, target, preview, lang, out);
    }
}

/// Zeilen der Diff-Vorschau, die ohne Scrollen sichtbar sind.
const DIFF_ROWS: usize = 16;

/// Diff-Vorschau von `edit_file`/`write_file` in der Freigabe-Zeile (#338): Datei, +/−, farbige Zeilen; scheitert der
/// Aufruf absehbar, steht stattdessen der Grund da.
fn diff_preview(ui: &mut Ui, id: Id, p: &Preview, lang: Lang) {
    use crate::chat::agent::diff::Line;
    let t = crate::style::ui();
    if let Some(e) = &p.error {
        ui.label(RichText::new(format!("{} {e}", lang.t("Dieser Aufruf wird so scheitern:", "This call will fail as it is:"))).small().color(t.warning));
        return;
    }
    let Some(d) = &p.diff else { return };
    let mut head = egui::text::LayoutJob::default();
    let small = FontId::proportional(12.0);
    head.append(&p.path, 0.0, egui::TextFormat { font_id: FontId::monospace(12.0), color: ui.visuals().text_color(), ..Default::default() });
    if p.new_file {
        head.append(lang.t("(neue Datei)", "(new file)"), 6.0, egui::TextFormat { font_id: small.clone(), color: style::dim(ui), ..Default::default() });
    }
    head.append(&format!("+{}", d.added), 8.0, egui::TextFormat { font_id: small.clone(), color: t.success, ..Default::default() });
    head.append(&format!("−{}", d.removed), 6.0, egui::TextFormat { font_id: small, color: error_col(), ..Default::default() });
    ui.label(head);
    if p.tolerant {
        ui.label(RichText::new(lang.t("Gefunden mit Toleranz für Leerzeichen am Zeilenende und Zeilenenden.", "Matched with tolerance for trailing spaces and line endings.")).small().color(style::dim(ui)));
    }
    let font = FontId::monospace(12.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(6)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt(id.with("approval_diff")).max_height(row_h * DIFF_ROWS as f32 + 2.0).auto_shrink([false, true]).show(ui, |ui| {
            let mut job = egui::text::LayoutJob::default();
            let fmt = |c: Color32| egui::TextFormat { font_id: font.clone(), color: c, ..Default::default() };
            let (add, del, dim) = (t.success, error_col(), style::dim(ui));
            for l in &d.lines {
                let (text, col) = match l {
                    Line::Same(n, s) => (format!("{n:>5}   {s}\n"), dim),
                    Line::Add(n, s) => (format!("{n:>5} + {s}\n"), add),
                    Line::Del(n, s) => (format!("{n:>5} − {s}\n"), del),
                    Line::Gap(n) => (if lang == Lang::De { format!("      ⋯ {n} unveränderte Zeilen\n") } else { format!("      ⋯ {n} unchanged lines\n") }, dim),
                };
                job.append(&text, 0.0, fmt(col));
            }
            if d.truncated {
                job.append(lang.t("      ⋯ (Vorschau gekürzt – die vollständigen Parameter stehen darunter)\n", "      ⋯ (preview cut – the full parameters are below)\n"), 0.0, fmt(t.warning));
            }
            if d.lines.is_empty() {
                job.append(lang.t("(keine Änderung am Inhalt)", "(no change to the content)"), 0.0, fmt(dim));
            }
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Wrap));
        });
    });
}

/// Freigabe-Zeile eines schreibenden Aufrufs. Enter gilt nur, wenn ein Knopf hier den Fokus hat (Tab), nie aus der
/// Eingabe heraus – so gibt niemand versehentlich frei, während er tippt.
#[allow(clippy::too_many_arguments)]
fn approval(ui: &mut Ui, id: Id, call_id: &str, name: &str, args: &serde_json::Value, target: Option<&str>, preview: Option<&Preview>, lang: Lang, out: &mut ChatOut) {
    let warn = crate::style::ui().warning;
    let call = crate::chat::store::ToolCall { id: call_id.to_string(), name: name.to_string(), arguments: args.clone() };
    let local = crate::chat::agent::is_local(&call);
    egui::Frame::new().fill(warn.gamma_multiply(0.14)).stroke(Stroke::new(1.0_f32, warn.gamma_multiply(0.6))).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let sensitive = preview.is_some_and(|p| p.sensitive);
        let what = match name {
            _ if !local => lang.t("Das Modell möchte ein Werkzeug mit Schreibzugriff nutzen:", "The model wants to use a tool with write access:"),
            "move_file" if sensitive => lang.t(
                "Das Modell möchte eine sensible Datei verschieben (z. B. mit Passwörtern oder Schlüsseln) – unter einem anderen Namen wäre sie danach ohne Nachfrage lesbar:",
                "The model wants to move a sensitive file (e.g. with passwords or keys) – under another name it could then be read without asking:",
            ),
            "edit_file" | "write_file" if sensitive => lang.t("Das Modell möchte eine sensible Datei ändern (z. B. mit Passwörtern oder Schlüsseln):", "The model wants to change a sensitive file (e.g. with passwords or keys):"),
            "read_file" => lang.t("Das Modell möchte eine sensible Datei lesen (z. B. mit Passwörtern oder Schlüsseln):", "The model wants to read a sensitive file (e.g. with passwords or keys):"),
            "web_fetch" | "web_search" => lang.t("Das Modell möchte auf das Internet zugreifen:", "The model wants to access the internet:"),
            "run_command" => lang.t("Das Modell möchte einen Befehl im Terminal des Chats ausführen:", "The model wants to run a command in the chat's terminal:"),
            "delete_file" => lang.t("Das Modell möchte etwas in den Papierkorb legen:", "The model wants to move something to the recycle bin:"),
            _ => lang.t("Das Modell möchte Dateien im Projektordner ändern:", "The model wants to change files in the project folder:"),
        };
        ui.label(RichText::new(what).small());
        let head = match target {
            Some(t) => format!("{name} → {t}"),
            None => name.to_string(),
        };
        ui.label(RichText::new(head).monospace().strong().size(12.5));
        if let Some(p) = preview {
            diff_preview(ui, id, p, lang);
            ui.label(RichText::new(lang.t("Parameter", "Parameters")).small().color(style::dim(ui)));
        }
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
            let allow = ui.add(egui::Button::new(RichText::new(lang.t("Erlauben", "Allow")).strong()).fill(crate::style::ui().chat_send));
            // „Immer“ nur mit Ziel-Sitzung (Werkzeug + Sitzung); Tickets, Todos, create_session … fragen jedes Mal.
            // Agenten-Werkzeuge (#338): je Werkzeug für den Chat, Befehle nur für denselben Anfang (`cargo test`).
            // Sensible Dateien (#338): jedes Mal fragen.
            let always = (AllowRule::eligible(&call) && !sensitive).then(|| {
                let scope = match AllowRule::for_call(&call) {
                    AllowRule { prefix: Some(p), .. } => {
                        if lang == Lang::De {
                            format!("Befehle, die mit „{p}“ beginnen")
                        } else {
                            format!("commands starting with “{p}”")
                        }
                    }
                    _ if local => {
                        let web = name.starts_with("web_");
                        match (lang, web) {
                            (Lang::De, true) => format!("{name} in diesem Chat (alle Adressen bzw. Suchen)"),
                            (Lang::De, false) => format!("{name} in diesem Chat (alle Pfade im Projektordner)"),
                            (_, true) => format!("{name} in this chat (all addresses or searches)"),
                            (_, false) => format!("{name} in this chat (all paths in the project folder)"),
                        }
                    }
                    _ => {
                        let session = args.get("session").and_then(|s| s.as_u64()).unwrap_or(0);
                        format!("{name} → {}", target.map(str::to_string).unwrap_or_else(|| format!("session {session}")))
                    }
                };
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
/// `loop_stop`: der Schleifen-Schutz hielt an (derselbe fehlerhafte Aufruf mehrmals in Folge, #340) – deutlich in der
/// Warnfarbe statt dezent.
fn paused_bar(ui: &mut Ui, steps: u32, note: Option<&str>, loop_stop: bool, lang: Lang, out: &mut ChatOut) {
    let warn = crate::style::ui().warning;
    let frame = if loop_stop {
        egui::Frame::new().fill(warn.gamma_multiply(0.14)).stroke(Stroke::new(1.0_f32, warn.gamma_multiply(0.6)))
    } else {
        egui::Frame::new().fill(ui.visuals().faint_bg_color)
    };
    frame.corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let text = match note {
                Some(n) => n.to_string(),
                None if lang == Lang::De => format!("Nach {steps} Werkzeugaufrufen angehalten."),
                None => format!("Stopped after {steps} tool calls."),
            };
            let text = if loop_stop { RichText::new(format!("⚠ {text}")).strong() } else { RichText::new(text).color(style::dim(ui)) };
            ui.label(text);
            if ui.button(lang.t("Weiter", "Continue")).on_hover_text(lang.t("Noch einmal so viele Werkzeugaufrufe erlauben", "Allow the same number of tool calls again")).clicked() {
                out.cont = true;
            }
        });
    });
}

/// Einmalige Frage vor dem ersten Senden: Darf dieses Modell Paneldeck nutzen? (Aus / Lesen / Steuern.)
fn level_bar(ui: &mut Ui, lang: Lang, out: &mut ChatOut) {
    let warn = crate::style::ui().warning;
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

/// Zeichen und Farbe für den Stand eines To-dos.
fn todo_mark(s: TodoStatus, dim: Color32) -> (&'static str, Color32) {
    match s {
        TodoStatus::Completed => ("✓", crate::style::ui().success),
        TodoStatus::InProgress => ("▸", crate::style::ui().warning),
        TodoStatus::Pending => ("·", dim),
    }
}

/// Kopfzeile: To-do-Liste des Modells (`update_todos`, #338) – „To-dos 2/5 · ▸ aktueller Punkt“, aufgeklappt die
/// ganze Liste.
fn todos_menu(ui: &mut Ui, session: u64, todos: &[ChatTodo], lang: Lang) {
    let done = todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
    let title = match todos.iter().find(|t| t.status == TodoStatus::InProgress) {
        Some(t) => format!("{} {done}/{} · ▸ {}", lang.t("To-dos", "To-dos"), todos.len(), crate::mcp::short(&t.content, 60)),
        None => format!("{} {done}/{}", lang.t("To-dos", "To-dos"), todos.len()),
    };
    let dim = style::dim(ui);
    ui.push_id(("chat_todos", session), |ui| {
        ui.menu_button(RichText::new(title).small(), |ui| {
            ui.label(RichText::new(lang.t("Plan des Modells (update_todos)", "The model's plan (update_todos)")).small().color(dim));
            for t in todos {
                ui.horizontal(|ui| {
                    let (mark, col) = todo_mark(t.status, dim);
                    ui.label(RichText::new(mark).color(col));
                    let text = RichText::new(&t.content).size(12.5);
                    ui.label(if t.status == TodoStatus::Completed { text.color(dim).strikethrough() } else { text });
                });
            }
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
                        let what = match (r.session, &r.prefix) {
                            (Some(s), _) => format!("{} → {} {s}", r.tool, lang.t("Sitzung", "session")),
                            (None, Some(p)) => format!("{}: {p} …", r.tool),
                            (None, None) => r.tool.clone(),
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

/// Füllstand des Kontexts rechts in der Kopfzeile (#339, #341): `4342/8192 (53%)` (geschätzt mit `~`), ein kleiner
/// Balken, nahe der Grenze in Warn- bzw. Fehlerfarbe mit Hinweis; Tooltip mit Aufteilung und Herkunft der Zahl.
/// Im Layout von rechts nach links aufrufen.
fn context_gauge(ui: &mut Ui, session: u64, chat: &ChatState, lang: Lang) {
    use crate::chat::context::{self, FillLevel};
    let Some(fill) = context::fill(chat.context.as_ref(), chat.usage, chat.file.context_start.is_some()) else {
        ui.label(RichText::new("–").small().color(style::dim(ui))).on_hover_text(lang.t(
            "Füllstand des Kontexts unbekannt (Server oder Modell des Chats fehlt).",
            "Context fill level unknown (the chat's server or model is missing).",
        ));
        return;
    };
    let col = match fill.level() {
        FillLevel::Normal => style::dim(ui),
        FillLevel::Near => crate::style::ui().warning,
        FillLevel::Full => error_col(),
    };
    let bar_col = if fill.level() == FillLevel::Normal { crate::style::ui().accent } else { col };
    let tip = fill.tooltip(lang);
    ui.push_id(("chat_ctx", session), |ui| {
        // Von rechts nach links: Balken, Zahl, Hinweis.
        let (rect, mut r) = ui.allocate_exact_size(Vec2::new(GAUGE_W, 6.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(3), ui.visuals().widgets.noninteractive.bg_stroke.color);
        let share = (fill.used as f32 / fill.parts.max.max(1) as f32).clamp(0.0, 1.0);
        if share > 0.0 {
            p.rect_filled(Rect::from_min_size(rect.min, Vec2::new((GAUGE_W * share).max(2.0), rect.height())), CornerRadius::same(3), bar_col);
        }
        r |= ui.add(egui::Label::new(RichText::new(fill.text()).small().monospace().color(col)).selectable(false));
        if let Some(h) = fill.hint(lang) {
            r |= ui.add(egui::Label::new(RichText::new(h).small().color(col)).selectable(false));
        }
        r.on_hover_text(tip);
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
    let b = ui.put(btn, egui::Button::new(RichText::new(label).strong()).fill(if streaming { error_col().gamma_multiply(0.6) } else { crate::style::ui().chat_send }));
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
                o.compact |= r.compact;
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

    /// Eine noch leere Antwort zeigt, was los ist (#299): „…“, nach ein paar Sekunden die Wartezeit, bei Denk-Ausgabe
    /// „Denkt nach …“ – statt endlos nur „…“.
    #[test]
    fn empty_answer_shows_waiting_time_and_thinking() {
        /// Hält die Antwort offen (Sender bleibt hier) und schickt vorab die gegebenen Ereignisse.
        struct Open(std::sync::Mutex<Vec<std::sync::mpsc::Sender<crate::chat::backend::TurnEvent>>>, Vec<crate::chat::backend::TurnEvent>);
        impl crate::chat::ChatBackend for Open {
            fn start(&self, _: &crate::state::ChatMeta, _: &[ChatMsg], _: &crate::chat::TurnContext, _: crate::chat::Repaint) -> crate::chat::backend::TurnHandle {
                let (h, tx, _) = crate::chat::backend::TurnHandle::pair();
                for ev in &self.1 {
                    tx.send(ev.clone()).unwrap();
                }
                self.0.lock().unwrap().push(tx);
                h
            }
        }
        for lang in [Lang::En, Lang::De] {
            let silent = Open(Default::default(), Vec::new());
            let mut c = ChatState::new(ChatFile::default(), None);
            c.draft = "hi".into();
            assert!(c.send(&silent, &crate::state::ChatMeta::default(), &crate::chat::TurnContext::default(), Arc::new(|| {})));
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            assert!(h.texts.iter().any(|t| t == "…"), "{:?}", h.texts);
            c.turn_started = Some(Instant::now() - Duration::from_secs(12));
            h.frame(&mut c, lang, false, vec![]);
            let want = waiting_text(false, Some(Duration::from_secs(12)), lang);
            assert!(h.texts.iter().any(|t| *t == want), "{want} fehlt: {:?}", h.texts);
            let thinking = Open(Default::default(), vec![crate::chat::backend::TurnEvent::Thinking]);
            let mut c = ChatState::new(ChatFile::default(), None);
            c.draft = "hi".into();
            assert!(c.send(&thinking, &crate::state::ChatMeta::default(), &crate::chat::TurnContext::default(), Arc::new(|| {})));
            c.poll();
            h.frame(&mut c, lang, false, vec![]);
            let want = lang.t("Denkt nach …", "Thinking …");
            assert!(h.texts.iter().any(|t| t == want), "{want} fehlt: {:?}", h.texts);
        }
        assert_eq!(waiting_text(false, Some(Duration::from_secs(2)), Lang::En), "…");
        assert_eq!(waiting_text(false, None, Lang::En), "…");
        assert_eq!(waiting_text(false, Some(Duration::from_secs(5)), Lang::De), "Warte auf das Modell … 5 s");
        assert!(waiting_text(false, Some(Duration::from_secs(18)), Lang::En).starts_with("Waiting for the model … 18 s – the server is probably still loading it"));
        assert!(waiting_text(false, Some(Duration::from_secs(65)), Lang::De).contains("lädt es vermutlich noch"));
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
    fn agent_approval_shows_the_diff_preview_and_the_todo_header() {
        struct One(ToolCall);
        impl crate::chat::ChatBackend for One {
            fn start(&self, _: &crate::state::ChatMeta, _: &[ChatMsg], _: &crate::chat::TurnContext, _: crate::chat::Repaint) -> crate::chat::backend::TurnHandle {
                let (h, tx, _) = crate::chat::backend::TurnHandle::pair();
                tx.send(crate::chat::backend::TurnEvent::ToolCall(self.0.clone())).unwrap();
                tx.send(crate::chat::backend::TurnEvent::Done).unwrap();
                h
            }
        }
        let (d, env) = crate::chat::agent::paths::tests::tmp_root("chatui");
        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        let call = ToolCall { id: "e".into(), name: "edit_file".into(), arguments: serde_json::json!({"path": "a.txt", "old_string": "two", "new_string": "2"}) };
        let mut c = ChatState::new(ChatFile::default(), None);
        c.draft = "edit".into();
        assert!(c.send(&One(call.clone()), &crate::state::ChatMeta::default(), &crate::chat::TurnContext::default(), Arc::new(|| {})));
        c.poll();
        assert_eq!(c.tool_step(&control().with_agent(env.clone())), crate::chat::ToolStep::Idle);
        assert!(c.awaiting_approval());
        c.approval_target = Some("a.txt".into());
        c.approval_preview = crate::chat::agent::files::preview("edit_file", &call.arguments, &env).map(|p| ("e".to_string(), p));
        c.set_todos(vec![ChatTodo { content: "Fix it".into(), status: TodoStatus::InProgress }, ChatTodo { content: "Test".into(), status: TodoStatus::Completed }]);
        for (lang, what) in [(Lang::En, "The model wants to change files in the project folder:"), (Lang::De, "Das Modell möchte Dateien im Projektordner ändern:")] {
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![]);
            assert!(h.texts.iter().any(|t| t == what), "{:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == "edit_file → a.txt"), "{:?}", h.texts);
            assert!(h.texts.iter().any(|t| t.starts_with("a.txt") && t.contains("+1") && t.contains("−1")), "Kopf der Vorschau: {:?}", h.texts);
            assert!(h.texts.iter().any(|t| t.contains("− two") && t.contains("+ 2")), "Diff-Zeilen: {:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == "To-dos 1/2 · ▸ Fix it"), "To-do-Kopfzeile: {:?}", h.texts);
            let always = if lang == Lang::De { "Immer für diesen Chat" } else { "Always for this chat" };
            assert!(h.texts.iter().any(|t| t == always), "Immer je Werkzeug: {:?}", h.texts);
        }
        // Scheitert der Aufruf absehbar, steht der Grund statt des Diffs da
        c.approval_preview = crate::chat::agent::files::preview("edit_file", &serde_json::json!({"path": "a.txt", "old_string": "zwei", "new_string": "2"}), &env).map(|p| ("e".to_string(), p));
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t.starts_with("This call will fail as it is:") && t.contains("not found")), "{:?}", h.texts);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Der To-do-Chip der Kopfzeile liegt nie über dem Verlauf: Auch mit den Maßen von Paneldeck (Knöpfe 26 px hoch)
    /// beginnt der sichtbare Bereich des Verlaufs erst unter dem Knopf, und nichts der Kopfzeile wird darunter gezeichnet.
    #[test]
    fn the_todo_chip_never_overlaps_the_history() {
        let mut file = ChatFile::default();
        for i in 0..200 {
            file.messages.push(ChatMsg::new(if i % 2 == 0 { Role::User } else { Role::Assistant }, format!("m{i}")));
        }
        let mut c = ChatState::new(file, None);
        c.set_todos(vec![ChatTodo { content: "Fix it".into(), status: TodoStatus::InProgress }, ChatTodo { content: "Test".into(), status: TodoStatus::Pending }]);
        let mut h = H::new();
        h.ctx.style_mut(|s| {
            s.spacing.interact_size = Vec2::new(32.0, 26.0);
            s.spacing.button_padding = Vec2::new(10.0, 5.0);
        });
        for _ in 0..3 {
            h.frame(&mut c, Lang::En, false, vec![]);
        }
        let text_shape = |pred: &dyn Fn(&str) -> bool| {
            h.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if pred(t.galley.text()) => Some((t.visual_bounding_rect(), s.clip_rect)),
                _ => None,
            })
        };
        let (chip, chip_clip) = text_shape(&|t| t == "To-dos 0/2 · ▸ Fix it").unwrap_or_else(|| panic!("{:?}", h.texts));
        let (_, list_clip) = text_shape(&|t| t.starts_with('m') && t.len() > 1 && t[1..].chars().all(|c| c.is_ascii_digit())).expect("Nachricht");
        // Knopf um den Text: mindestens 26 px hoch, mittig zum Text.
        let button_bottom = chip.center().y + 13.0;
        assert!(button_bottom <= list_clip.min.y + 0.5, "Chip unten {button_bottom} > Verlauf oben {}", list_clip.min.y);
        // Die Kopfzeile zeichnet nur in ihrem Bereich, der Verlauf beginnt darunter (sein Clip ragt um den Rand
        // `clip_rect_margin` von egui darüber hinaus).
        let margin = h.ctx.style().visuals.clip_rect_margin;
        assert!(chip_clip.max.y <= list_clip.min.y + margin + 0.5, "Kopfzeile zeichnet nur in ihrem Bereich: {chip_clip:?} / {list_clip:?}");
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
            let rules = vec![AllowRule { tool: "send_text".into(), session: Some(4), prefix: None }, AllowRule { tool: "add_todo".into(), session: None, prefix: None }];
            let mut c = ChatState::new(ChatFile { always_allow: rules.clone(), ..ChatFile::default() }, None);
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            let p = h.pos_of(title).unwrap_or_else(|| panic!("{title}: {:?}", h.texts));
            let mv = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            for evs in [vec![Event::PointerMoved(p)], vec![mv(true)], vec![mv(false)]] {
                h.frame(&mut c, lang, false, evs);
            }
            h.frame(&mut c, lang, false, vec![]);
            let target = if lang == Lang::De { "send_text → Sitzung 4" } else { "send_text → session 4" };
            assert!(h.texts.iter().any(|t| t == target), "{:?}", h.texts);
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

    /// #340: Reparaturen stehen dezent am Werkzeug-Block (beide Sprachen); der Schleifen-Schutz hält deutlich sichtbar an.
    #[test]
    fn repaired_calls_show_a_note_and_a_stopped_loop_a_warning() {
        use crate::chat::store::CallRepair;
        let mut a = ChatMsg::new(Role::Assistant, "");
        a.tool_calls.push(ToolCall { id: "c1".into(), name: "read_terminal".into(), arguments: serde_json::json!({"session": 3}) });
        a.tool_calls.push(ToolCall { id: "c2".into(), name: "whoami".into(), arguments: serde_json::json!({}) });
        a.repairs.push(CallRepair { id: "c1".into(), text_call: true, fixes: vec![r#"session: "3" → 3"#.into()] });
        let mut r = ChatMsg::new(Role::Tool, "{}");
        r.tool_call_id = Some("c1".into());
        r.status = Some(ToolStatus::Ok);
        for (lang, note) in [
            (Lang::En, r#"Repaired by Paneldeck: recognised as a text call · session: "3" → 3"#),
            (Lang::De, r#"Von Paneldeck repariert: als Textaufruf erkannt · session: "3" → 3"#),
        ] {
            let mut c = ChatState::new(ChatFile { messages: vec![ChatMsg::new(Role::User, "q"), a.clone(), r.clone()], ..ChatFile::default() }, None);
            c.paused = true;
            c.loop_stop = true;
            c.pause_note = Some("LOOP-NOTE".into());
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![]);
            assert_eq!(h.texts.iter().filter(|t| t.starts_with(lang.t("Von Paneldeck", "Repaired by"))).count(), 1, "nur am reparierten Aufruf: {:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == note), "{:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == "⚠ LOOP-NOTE"), "{:?}", h.texts);
            assert!(h.pos_of(lang.t("Weiter", "Continue")).is_some());
        }
        assert_eq!(repair_note(&CallRepair { id: "x".into(), text_call: false, fixes: vec![] }, Lang::En), None);
        assert_eq!(repair_note(&CallRepair { id: "x".into(), text_call: false, fixes: vec!["a: 1 → \"1\"".into()] }, Lang::En).as_deref(), Some("Repaired by Paneldeck: a: 1 → \"1\""));
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

    /// Kompaktieren (#359) in der Oberfläche: Knopf in der Kopfzeile (nur mit genug Runden), Hinweis während der
    /// Zusammenfassung, Karte mit „Original anzeigen“ – die Originale erscheinen erst aufgeklappt –, Fehlermeldung mit
    /// „Ausblenden“; Texte in beiden Sprachen.
    #[test]
    fn compact_button_summary_card_show_original_and_failure_bar() {
        let rounds = |n: usize| (0..n).flat_map(|i| [ChatMsg::new(Role::User, format!("question {i}")), ChatMsg::new(Role::Assistant, format!("answer {i}"))]).collect::<Vec<_>>();
        // Zwei Runden: der Knopf ist da, aber ohne Wirkung.
        let mut c = ChatState::new(ChatFile { messages: rounds(2), ..ChatFile::default() }, None);
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        let p = h.pos_of("Compact").expect("Knopf in der Kopfzeile");
        assert!(!h.click(&mut c, Lang::En, p).compact, "nichts zu kompaktieren: Knopf gesperrt");
        // Drei Runden: Klick meldet „kompaktieren“.
        let mut c = ChatState::new(ChatFile { messages: rounds(3), ..ChatFile::default() }, None);
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        let p = h.pos_of("Compact").unwrap();
        assert!(h.click(&mut c, Lang::En, p).compact);
        // Die App kompaktiert (hier mit dem Echo-Backend); währenddessen ein Hinweis und Stopp statt Senden.
        let b = EchoBackend { delay: Duration::from_millis(30) };
        c.compact_now(&b, &crate::chat::default_chat_meta(), &crate::chat::TurnContext::default(), Arc::new(|| {})).unwrap();
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t.starts_with("Compacting the chat – the model is writing the summary …")), "{:?}", h.texts);
        assert!(h.pos_of("Stop").is_some());
        let t = Instant::now();
        while c.compact.running() && t.elapsed() < Duration::from_secs(5) {
            c.poll();
            h.frame(&mut c, Lang::En, false, vec![]);
            std::thread::sleep(Duration::from_millis(2));
        }
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "Compacted – 2 older messages summarised"), "{:?}", h.texts);
        assert!(h.texts.iter().any(|t| t == "The model sees this instead of the older messages:"));
        assert!(h.texts.iter().any(|t| t.contains("Summary of 2 messages.")), "{:?}", h.texts);
        assert!(!h.texts.iter().any(|t| t == "question 0" || t == "answer 0"), "Originale zugeklappt: {:?}", h.texts);
        assert!(h.texts.iter().any(|t| t == "question 1") && h.texts.iter().any(|t| t == "answer 2"), "behaltene Runden sichtbar");
        // Original anzeigen / ausblenden.
        let p = h.pos_of("Show original").unwrap();
        h.click(&mut c, Lang::En, p);
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "question 0"), "{:?}", h.texts);
        assert!(h.texts.iter().any(|t| t == "Original history – compacted, no longer sent to the model"));
        let p = h.pos_of("Hide original").unwrap();
        h.click(&mut c, Lang::En, p);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(!h.texts.iter().any(|t| t == "question 0"), "{:?}", h.texts);
        // Deutsch.
        h.frame(&mut c, Lang::De, false, vec![]);
        h.frame(&mut c, Lang::De, false, vec![]);
        for want in ["Kompaktiert – 2 ältere Nachrichten zusammengefasst", "Original anzeigen", "Kompaktieren", "Das sieht das Modell statt der älteren Nachrichten:"] {
            assert!(h.texts.iter().any(|t| t == want), "{want}: {:?}", h.texts);
        }
        // Fehlermeldung mit „Ausblenden“.
        c.compact.failure = Some("Compaction failed: boom The chat is unchanged.".into());
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "⚠ Compaction failed: boom The chat is unchanged."), "{:?}", h.texts);
        let p = h.pos_of("Dismiss").unwrap();
        h.click(&mut c, Lang::En, p);
        assert_eq!(c.compact.failure, None);
        assert_eq!(compacting_text(12, Lang::De), "Kompaktiere den Chat – das Modell schreibt die Zusammenfassung … 12 s");
    }

    /// Füllstand des Kontexts (#339) in der Kopfzeile: immer da, geschätzt mit `~`, vom Server ohne, nahe der Grenze mit
    /// Hinweis, Tooltip mit Aufteilung.
    #[test]
    fn context_fill_level_is_always_shown_in_the_header() {
        use crate::chat::context::ContextParts;
        let parts = |history: usize| ContextParts { system: 3000, tools: 1000, history, max: 8192, reserve: 2048, dropped: false, scale: 1.0 };
        // Ohne Aufteilung (Server fehlt): Platzhalter.
        let mut c = ChatState::new(ChatFile::default(), None);
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "–"), "{:?}", h.texts);
        // Vor der ersten Antwort: Schätzung.
        c.context = Some(parts(342));
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "~4342/8192 (53%)"), "{:?}", h.texts);
        assert!(!h.texts.iter().any(|t| t.contains("drop out")), "{:?}", h.texts);
        // Zahl des Servers.
        c.usage = Some((Some(4000), Some(342)));
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "4342/8192 (53%)"), "{:?}", h.texts);
        // Tooltip beim Überfahren.
        let p = h.pos_of("4342/8192 (53%)").unwrap();
        h.frame(&mut c, Lang::En, false, vec![Event::PointerMoved(p)]);
        for _ in 0..30 {
            h.frame(&mut c, Lang::En, false, vec![]);
        }
        assert!(h.texts.iter().any(|t| t.contains("System prompt + tools: ~4000") && t.contains("Source: the server's count")), "{:?}", h.texts);
        // Nahe der Grenze (Platz vor der Kürzung 6144): Hinweis, deutsch.
        let mut c = ChatState::new(ChatFile::default(), None);
        c.context = Some(parts(1000));
        let mut h = H::new();
        h.frame(&mut c, Lang::De, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "~5000/8192 (61%)") && h.texts.iter().any(|t| t == "bald fallen alte Nachrichten heraus"), "{:?}", h.texts);
        // Ältere Nachrichten fallen schon weg.
        c.file.context_start = Some(0);
        h.frame(&mut c, Lang::De, false, vec![]);
        assert!(h.texts.iter().any(|t| t == "alte Nachrichten fallen heraus"), "{:?}", h.texts);
    }
}
