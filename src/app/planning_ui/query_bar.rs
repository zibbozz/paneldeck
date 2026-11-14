//! Die Abfrage-Leiste: ein wiederverwendbarer Baustein für die Abfragesprache (`crate::query`).
//!
//! * Einzeiliges, bei Bedarf wachsendes Textfeld (Umschalt+Enter = Zeilenumbruch, Enter = abschicken).
//! * **Autovervollständigung** unter dem Cursor (`query::suggest`): Felder, Operatoren, Werte, Schlüsselwörter, Funktionen.
//!   Pfeil hoch/runter wählt, Tab/Enter (nach Pfeil) bzw. Klick übernimmt, Esc schließt, Strg+Leertaste öffnet von Hand.
//! * **Live-Prüfung**: rote Wellenlinie an der Fehlerstelle, die Meldung (zweisprachig) unter dem Feld, bei „meinten Sie …“
//!   ein Knopf, der die Korrektur übernimmt.
//! * **Verlauf** der letzten Abfragen (Pfeil hoch/runter im leeren/unveränderten Feld, dazu ein Dropdown).
//! * Knöpfe „?“ (Hilfe), „Filter speichern“, Dropdown „Gespeicherte Filter“ und „Schlüsselwörter …“.
//!
//! Der Baustein **führt nichts aus**: eine fertige Abfrage kommt als [`QueryBarOutput::submitted`] zum Aufrufer, der sie
//! ausführt (Suche, Board-Quelle, Dashboard-Kachel …). Hilfeseite, Schlüsselwort-Editor und Ausführung öffnet ebenfalls der
//! Aufrufer (`help_clicked`, `keywords_clicked`). Nur die Dialoge für gespeicherte Filter gehören zur Leiste selbst.
//!
//! Aufruf (je Leiste ein [`QueryBarState`] mit eigenem Namen, das Schema baut der Aufrufer):
//! ```ignore
//! let out = query_bar(ui, &mut state, &QueryBarCtx::new(&schema, lang).with_filters(conn, Some(project_id), "PAN"));
//! if let Some(q) = out.submitted { /* ausführen */ }
//! ```

use super::query_logic::{Diag, Fix, History, Severity, apply_fix, apply_suggestion, byte_of, char_of, diagnose, is_submittable, one_line, wave_ranges};
use super::saved_filters::{FilterEvent, FilterUi};
use crate::query::{Schema, Suggestion, SuggestionKind, suggest};
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Color32, FontId, Id, Key, KeyboardShortcut, Modifiers, Pos2, Rect, Response, RichText, Sense, Stroke, TextStyle, Ui, Vec2, text::CCursor, text::CCursorRange};
use rusqlite::Connection;

/// So viele Vorschläge zeigt das Popup höchstens (der Rest ist über das Tippen erreichbar).
const MAX_SUGGESTIONS: usize = 80;
/// So viele Meldungen stehen unter dem Feld, der Rest wird gezählt.
const MAX_MESSAGES: usize = 3;
/// Rot der Fehler (wie `widgets::error`).
const RED: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x75);

/// Was die Leiste von außen braucht.
pub struct QueryBarCtx<'a> {
    /// Felder, Prädikate, Sortier-Schlüsselwörter und Funktionen für Vorschläge und Prüfung.
    pub schema: &'a Schema,
    pub lang: Lang,
    /// Datenbank für die gespeicherten Filter; ohne sie fehlen Dropdown und „Filter speichern“.
    pub conn: Option<&'a Connection>,
    /// Projekt der Leiste: seine Filter und die globalen (`None` = nur globale).
    pub project_id: Option<i64>,
    /// Kürzel des Projekts (für den Dialog „Filter speichern“).
    pub project_key: &'a str,
    /// Gibt es eine Hilfeseite? Dann zeigt die Leiste den Knopf „?“.
    pub show_help: bool,
    /// Gibt es einen Schlüsselwort-Editor? Dann zeigt die Leiste den Knopf „Schlüsselwörter …“.
    pub show_keywords: bool,
}

impl<'a> QueryBarCtx<'a> {
    pub fn new(schema: &'a Schema, lang: Lang) -> Self {
        Self { schema, lang, conn: None, project_id: None, project_key: "", show_help: true, show_keywords: true }
    }

    /// Mit gespeicherten Filtern (Datenbank, Projekt, Projektkürzel).
    pub fn with_filters(mut self, conn: &'a Connection, project_id: Option<i64>, project_key: &'a str) -> Self {
        self.conn = Some(conn);
        self.project_id = project_id;
        self.project_key = project_key;
        self
    }
}

/// Was die Leiste in diesem Bild gemeldet hat.
#[derive(Default, Debug)]
pub struct QueryBarOutput {
    /// Eine gültige (oder leere) Abfrage wurde abgeschickt: Enter, „Suchen“ oder ein gewählter gespeicherter Filter.
    pub submitted: Option<String>,
    /// Der Knopf „?“ wurde geklickt (Aufrufer öffnet die Hilfeseite).
    pub help_clicked: bool,
    /// „Schlüsselwörter …“ wurde geklickt (Aufrufer öffnet den Editor).
    pub keywords_clicked: bool,
    /// Der Verlauf hat sich geändert (Aufrufer kann ihn speichern, `QueryBarState::history`).
    pub history_changed: bool,
    /// Der Text wurde in diesem Bild geändert.
    pub changed: bool,
    /// Die Abfrage hat keine Meldung (leer gilt als gültig).
    pub valid: bool,
}

/// Zustand einer Leiste (je Leiste einer, mit eigenem Namen).
pub struct QueryBarState {
    salt: &'static str,
    /// Der Abfragetext im Feld.
    pub text: String,
    pub history: History,
    pub filters: FilterUi,
    /// Popup mit Vorschlägen gewünscht (nach Tippen, Strg+Leertaste, Übernehmen)
    open: bool,
    sel: Option<usize>,
    sugg: Vec<Suggestion>,
    last_cursor: usize,
    popup_rect: Option<Rect>,
    focus_next: bool,
    cursor_next: Option<usize>,
    scroll_sel: bool,
}

impl QueryBarState {
    pub fn new(salt: &'static str) -> Self {
        Self {
            salt,
            text: String::new(),
            history: History::default(),
            filters: FilterUi::default(),
            open: false,
            sel: None,
            sugg: Vec::new(),
            last_cursor: 0,
            popup_rect: None,
            focus_next: false,
            cursor_next: None,
            scroll_sel: false,
        }
    }

    fn id(&self) -> Id {
        Id::new(("query_bar_edit", self.salt))
    }

    /// Abfrage von außen setzen (z. B. „In Suche übernehmen“): Text ersetzen, Cursor ans Ende, Feld fokussieren.
    pub fn set_query(&mut self, q: &str) {
        self.text = q.to_string();
        self.cursor_next = Some(self.text.len());
        self.last_cursor = self.text.len();
        self.focus_next = true;
        self.open = false;
        self.sel = None;
        self.history.reset_nav();
    }
}

/// Wurde in diesem Bild Umschalt+Enter gedrückt? (Das Textfeld macht daraus einen Zeilenumbruch.)
fn has_shift_enter(i: &egui::InputState) -> bool {
    i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if modifiers.shift))
}

/// Cursor des Textfelds setzen (Byte-Offset im Text).
fn set_cursor(ctx: &egui::Context, id: Id, text: &str, byte: usize) {
    let mut s = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(char_of(text, byte)))));
    s.store(ctx, id);
}

/// Die Abfrage-Leiste zeichnen. Siehe Moduldoku.
pub fn query_bar(ui: &mut Ui, st: &mut QueryBarState, cx: &QueryBarCtx) -> QueryBarOutput {
    let lang = cx.lang;
    let ctx = ui.ctx().clone();
    let id = st.id();
    let mut out = QueryBarOutput::default();

    if st.focus_next {
        ctx.memory_mut(|m| m.request_focus(id));
        st.focus_next = false;
    }
    if let Some(b) = st.cursor_next.take() {
        set_cursor(&ctx, id, &st.text, b);
    }

    // ---- Tasten, bevor das Textfeld sie sieht
    let had_focus = ctx.memory(|m| m.has_focus(id));
    let mut accept: Option<usize> = None;
    let mut submit_key = false;
    let mut open_request = false;
    let mut history_text: Option<String> = None;
    let mut leave_focus = false;
    if had_focus {
        let n = st.sugg.len();
        let visible = st.open && n > 0;
        let single_line = !st.text.contains('\n');
        ctx.input_mut(|i| {
            let shift_enter = has_shift_enter(i);
            if visible {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    st.sel = Some(st.sel.map_or(0, |s| (s + 1) % n));
                    st.scroll_sel = true;
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    st.sel = Some(st.sel.map_or(n - 1, |s| if s == 0 { n - 1 } else { s - 1 }));
                    st.scroll_sel = true;
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    accept = Some(st.sel.unwrap_or(0));
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    st.open = false;
                    st.sel = None;
                }
                if st.sel.is_some() && !shift_enter && i.consume_key(Modifiers::NONE, Key::Enter) {
                    accept = st.sel;
                }
            } else {
                if single_line && i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    history_text = st.history.prev(&st.text);
                }
                if single_line && i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    history_text = st.history.next(&st.text).or(history_text.take());
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    if n > 0 { open_request = true } else { leave_focus = true }
                }
            }
            if i.consume_key(Modifiers::CTRL, Key::Space) {
                open_request = true;
            }
            if accept.is_none() && !shift_enter && i.consume_key(Modifiers::NONE, Key::Enter) {
                submit_key = true;
            }
        });
    }
    if leave_focus {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
    if let Some(q) = history_text {
        st.text = q;
        st.last_cursor = st.text.len();
        set_cursor(&ctx, id, &st.text, st.text.len());
        st.open = false;
        out.changed = true;
    }
    if let Some(i) = accept {
        if let Some(s) = st.sugg.get(i).cloned() {
            let (t, c) = apply_suggestion(&st.text, &s);
            st.text = t;
            st.last_cursor = c;
            set_cursor(&ctx, id, &st.text, c);
            st.open = true;
            st.sel = None;
            st.history.reset_nav();
            out.changed = true;
        }
    }

    // ---- Das Textfeld mit den Knöpfen „Suchen“ und „?“
    let mut search_clicked = false;
    let mut edit = None;
    ui.horizontal_top(|ui| {
        let buttons = 78.0 + if cx.show_help { 34.0 } else { 0.0 };
        let w = (ui.available_width() - buttons - S2 * 2.0).max(160.0);
        let o = egui::TextEdit::multiline(&mut st.text)
            .id(id)
            .font(TextStyle::Monospace)
            .desired_rows(1)
            .desired_width(w)
            .lock_focus(true)
            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
            .margin(egui::Margin::symmetric(8, 6))
            .hint_text(RichText::new(lang.t("Abfrage, z. B. type = Epic AND status != Done ORDER BY priority", "Query, e.g. type = Epic AND status != Done ORDER BY priority")).color(style::dim(ui)))
            .show(ui);
        edit = Some(o);
        if ui.add_sized([72.0, 30.0], egui::Button::new(RichText::new(lang.t("Suchen", "Search")).strong()).fill(style::accent().gamma_multiply(0.45))).on_hover_text(lang.t("Abfrage ausführen (Enter)", "Run the query (Enter)")).clicked() {
            search_clicked = true;
        }
        if cx.show_help && help_button(ui, lang.t("Hilfe zur Abfragesprache", "Help for the query language")).clicked() {
            out.help_clicked = true;
        }
    });
    let Some(o) = edit else { return out };
    let focused = o.response.has_focus();
    if o.response.changed() {
        st.open = true;
        st.sel = None;
        st.history.reset_nav();
        out.changed = true;
    }
    let cursor = o.cursor_range.map(|r| byte_of(&st.text, r.primary.index)).unwrap_or(st.last_cursor);
    if !o.response.changed() && cursor != st.last_cursor && st.cursor_next.is_none() && accept.is_none() {
        // Der Cursor ist ohne Tippen gewandert (Pfeile, Klick): Vorschläge nicht mehr aufdrängen.
        st.open = false;
    }
    st.last_cursor = cursor;
    if open_request {
        st.open = true;
    }

    // ---- Meldungen und Wellenlinie
    let diags = diagnose(&st.text, cx.schema, lang);
    out.valid = diags.is_empty();
    paint_waves(ui, &o, &st.text, &wave_ranges(&st.text, &diags));

    // ---- Vorschläge berechnen und zeichnen
    let over_popup = st.popup_rect.is_some_and(|r| ctx.input(|i| i.pointer.hover_pos().is_some_and(|p| r.contains(p))));
    if focused {
        let mut s = suggest(&st.text, cursor, cx.schema);
        s.retain(|x| !(!x.replace_range.is_empty() && st.text.get(x.replace_range.clone()) == Some(x.insert_text.as_str())));
        s.truncate(MAX_SUGGESTIONS);
        if st.sel.is_some_and(|i| i >= s.len()) {
            st.sel = None;
        }
        st.sugg = s;
    } else if !over_popup {
        st.open = false;
    }
    if ctx.input(|i| i.pointer.any_pressed()) {
        // Klick außerhalb von Feld und Popup schließt die Vorschläge.
        let pos = ctx.input(|i| i.pointer.interact_pos());
        if pos.is_some_and(|p| !o.response.rect.contains(p) && !st.popup_rect.is_some_and(|r| r.contains(p))) {
            st.open = false;
        }
    }
    let mut clicked_accept: Option<usize> = None;
    if focused && st.open && !st.sugg.is_empty() {
        // Solange das Popup offen ist, soll Esc nur das Popup schließen und dem Feld nicht den Fokus nehmen.
        ctx.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
    }
    if st.open && !st.sugg.is_empty() && (focused || over_popup) {
        let first = st.sugg[0].replace_range.start;
        let cur = o.galley.pos_from_cursor(CCursor::new(char_of(&st.text, first)));
        let pos = o.galley_pos + cur.left_bottom().to_vec2() + Vec2::new(0.0, 3.0);
        let r = suggestion_popup(&ctx, id, pos, st, lang, &mut clicked_accept);
        st.popup_rect = Some(r);
    } else {
        st.popup_rect = None;
    }
    if let Some(i) = clicked_accept {
        if let Some(s) = st.sugg.get(i).cloned() {
            let (t, c) = apply_suggestion(&st.text, &s);
            st.text = t;
            st.last_cursor = c;
            st.cursor_next = Some(c);
            st.focus_next = true;
            st.open = true;
            st.sel = None;
            st.history.reset_nav();
            out.changed = true;
            ctx.request_repaint();
        }
    }

    // ---- Meldungen unter dem Feld
    let mut apply: Option<Fix> = None;
    messages(ui, &diags, lang, &mut apply);
    if let Some(fix) = apply {
        let (t, c) = apply_fix(&st.text, &fix);
        st.text = t;
        st.last_cursor = c;
        st.cursor_next = Some(c);
        st.focus_next = true;
        st.open = false;
        st.history.reset_nav();
        out.changed = true;
        ctx.request_repaint();
    }

    // ---- Abschicken
    if (submit_key || search_clicked) && is_submittable(&diags_now(&st.text, cx)) {
        submit(st, &mut out);
    }

    // ---- Werkzeugzeile: Verlauf, gespeicherte Filter, Filter speichern, Schlüsselwörter
    let valid_now = !st.text.trim().is_empty() && is_submittable(&diags_now(&st.text, cx));
    ui.add_space(S1);
    let mut load: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        history_menu(ui, st, lang, &mut out);
        if let Some(conn) = cx.conn {
            if let Some(FilterEvent::Load(q)) = st.filters.menu(ui, conn, cx.project_id, lang) {
                load = Some(q);
            }
            let resp = ui
                .add_enabled(valid_now, egui::Button::new(lang.t("Filter speichern", "Save filter")))
                .on_hover_text(lang.t("Die Abfrage unter einem Namen speichern", "Save the query under a name"))
                .on_disabled_hover_text(lang.t("Zum Speichern muss eine gültige Abfrage im Feld stehen", "A valid query must be in the field to save it"));
            if resp.clicked() {
                st.filters.open_save(&st.text, cx.project_id);
            }
        }
        if cx.show_keywords && ui.add(egui::Button::new(format!("{} …", lang.t("Schlüsselwörter", "Keywords")))).on_hover_text(lang.t("Eigene Schlüsselwörter für die Abfragesprache verwalten (z. B. IS blocked)", "Manage your own keywords for the query language (e.g. IS blocked)")).clicked() {
            out.keywords_clicked = true;
        }
    });
    if let Some(conn) = cx.conn {
        if let Some(FilterEvent::Load(q)) = st.filters.dialogs(&ctx, conn, cx.project_id, cx.project_key, lang, id, &st.text, valid_now) {
            load = Some(q);
        }
    }
    if let Some(q) = load {
        st.set_query(&q);
        out.changed = true;
        if is_submittable(&diags_now(&st.text, cx)) {
            submit(st, &mut out);
        }
    }
    out
}

fn diags_now(text: &str, cx: &QueryBarCtx) -> Vec<Diag> {
    diagnose(text, cx.schema, cx.lang)
}

/// Abfrage abschicken: in den Verlauf, an den Aufrufer, Vorschläge zu.
fn submit(st: &mut QueryBarState, out: &mut QueryBarOutput) {
    let q = st.text.trim().to_string();
    st.history.push(&q);
    st.open = false;
    st.sel = None;
    out.history_changed = true;
    out.submitted = Some(q);
}

/// Verlauf als Dropdown (Uhr-Symbol).
fn history_menu(ui: &mut Ui, st: &mut QueryBarState, lang: Lang, out: &mut QueryBarOutput) {
    let resp = ui.add(egui::Button::new(format!("{} ▾", lang.t("Verlauf", "History")))).on_hover_text(lang.t("Die zuletzt abgeschickten Abfragen (auch Pfeil hoch/runter im Feld)", "The most recently submitted queries (also arrow up/down in the field)"));
    let mut pick: Option<String> = None;
    let mut clear = false;
    egui::Popup::from_toggle_button_response(&resp).show(|ui| {
        ui.set_min_width(280.0);
        if st.history.is_empty() {
            ui.label(RichText::new(lang.t("Noch keine Abfragen.", "No queries yet.")).color(style::dim(ui)));
        }
        for q in st.history.newest_first() {
            if ui.add(egui::Button::new(RichText::new(one_line(q, 70)).monospace().size(12.0)).frame(false)).on_hover_text(q).clicked() {
                pick = Some(q.to_string());
            }
        }
        if !st.history.is_empty() {
            ui.separator();
            if ui.add(egui::Button::new(lang.t("Verlauf löschen", "Clear history")).frame(false)).clicked() {
                clear = true;
            }
        }
    });
    if let Some(q) = pick {
        st.set_query(&q);
        out.changed = true;
    }
    if clear {
        st.history = History::default();
        out.history_changed = true;
    }
}

/// Der Knopf „?“: Kreis mit Fragezeichen.
fn help_button(ui: &mut Ui, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    let a = style::accent();
    let h = style::anim(ui.ctx(), resp.id.with("h"), resp.hovered(), style::D_FAST);
    let c = rect.center();
    ui.painter().circle_filled(c, 11.5, a.gamma_multiply(0.12 + 0.25 * h));
    ui.painter().circle_stroke(c, 11.5, Stroke::new(1.4_f32, a.gamma_multiply(0.7 + 0.3 * h)));
    ui.painter().text(c, egui::Align2::CENTER_CENTER, "?", FontId::proportional(15.0), style::mix(ui.visuals().text_color(), ui.visuals().strong_text_color(), h));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

// ------------------------------------------------------------------ Meldungen

fn messages(ui: &mut Ui, diags: &[Diag], lang: Lang, apply: &mut Option<Fix>) {
    if diags.is_empty() {
        return;
    }
    ui.add_space(S1);
    for d in diags.iter().take(MAX_MESSAGES) {
        let col = match d.severity {
            Severity::Error => RED,
            Severity::Incomplete => style::dim(ui),
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(if d.severity == Severity::Error { "▲" } else { "…" }).small().color(col));
            ui.label(RichText::new(&d.message).color(col));
            if let Some(fix) = &d.fix {
                let label = format!("{} {}", lang.t("Übernehmen:", "Apply:"), lang.q(&fix.shown));
                if ui.add(egui::Button::new(RichText::new(label).small())).on_hover_text(lang.t("Die Korrektur in die Abfrage einsetzen", "Put the correction into the query")).clicked() {
                    *apply = Some(fix.clone());
                }
            }
        });
    }
    if diags.len() > MAX_MESSAGES {
        let n = diags.len() - MAX_MESSAGES;
        ui.label(RichText::new(match lang {
            Lang::De => format!("… und {n} weitere"),
            Lang::En => format!("… and {n} more"),
        }).small().color(style::dim(ui)));
    }
}

/// Wellenlinie unter den Bereichen (Byte-Bereiche im Text) in Rot.
fn paint_waves(ui: &Ui, o: &egui::text_edit::TextEditOutput, text: &str, ranges: &[std::ops::Range<usize>]) {
    let painter = ui.painter();
    for r in ranges {
        let (cs, ce) = (char_of(text, r.start), char_of(text, r.end));
        // Zusammenhängende Strecken je Zeile: (x0, x1, y)
        let mut segs: Vec<(f32, f32, f32)> = Vec::new();
        for i in cs..ce {
            let a = o.galley.pos_from_cursor(CCursor::new(i));
            let b = o.galley.pos_from_cursor(CCursor::new(i + 1));
            let (x0, x1) = if (a.min.y - b.min.y).abs() < 1.0 { (a.min.x, b.min.x) } else { (a.min.x, a.min.x + 7.0) };
            let y = a.max.y - 1.0;
            match segs.last_mut() {
                Some(l) if (l.2 - y).abs() < 0.5 && (l.1 - x0).abs() < 0.5 => l.1 = x1,
                _ => segs.push((x0, x1, y)),
            }
        }
        for (x0, x1, y) in segs {
            let (x0, x1) = (x0.min(x1), x0.max(x1).max(x0 + 4.0));
            let mut pts = Vec::new();
            let mut x = x0;
            let mut up = false;
            while x < x1 {
                pts.push(o.galley_pos + Vec2::new(x, y + if up { -1.6 } else { 0.4 }));
                x += 2.5;
                up = !up;
            }
            pts.push(o.galley_pos + Vec2::new(x1, y + if up { -1.6 } else { 0.4 }));
            painter.add(egui::Shape::line(pts, Stroke::new(1.3_f32, RED)));
        }
    }
}

// ------------------------------------------------------------------ Popup

fn kind_color(k: SuggestionKind) -> Color32 {
    match k {
        SuggestionKind::Field => Color32::from_rgb(0x58, 0xA6, 0xE6),
        SuggestionKind::Operator => Color32::from_rgb(0xE5, 0xA0, 0x4B),
        SuggestionKind::Value => Color32::from_rgb(0x7F, 0xBF, 0x6A),
        SuggestionKind::Keyword => Color32::from_rgb(0xB7, 0x7F, 0xE0),
        SuggestionKind::Function => Color32::from_rgb(0xE0, 0x7F, 0xB0),
        SuggestionKind::Predicate => Color32::from_rgb(0x4F, 0xB8, 0xA8),
        SuggestionKind::SortKey => Color32::from_rgb(0xD8, 0xB9, 0x4A),
    }
}

/// Kurzzeichen im Symbol der Vorschlagsart.
pub fn kind_symbol(k: SuggestionKind) -> &'static str {
    match k {
        SuggestionKind::Field => "F",
        SuggestionKind::Operator => "=",
        SuggestionKind::Value => "V",
        SuggestionKind::Keyword => "K",
        SuggestionKind::Function => "fx",
        SuggestionKind::Predicate => "IS",
        SuggestionKind::SortKey => "↕",
    }
}

/// Name der Vorschlagsart (zweisprachig).
pub fn kind_name(k: SuggestionKind, lang: Lang) -> &'static str {
    match k {
        SuggestionKind::Field => lang.t("Feld", "Field"),
        SuggestionKind::Operator => lang.t("Operator", "Operator"),
        SuggestionKind::Value => lang.t("Wert", "Value"),
        SuggestionKind::Keyword => lang.t("Schlüsselwort", "Keyword"),
        SuggestionKind::Function => lang.t("Funktion", "Function"),
        SuggestionKind::Predicate => lang.t("Prädikat", "Predicate"),
        SuggestionKind::SortKey => lang.t("Sortierung", "Sort key"),
    }
}

/// Das Popup mit den Vorschlägen; gibt sein Rechteck zurück. Ein Klick landet in `clicked`.
fn suggestion_popup(ctx: &egui::Context, id: Id, pos: Pos2, st: &mut QueryBarState, lang: Lang, clicked: &mut Option<usize>) -> Rect {
    let scroll_sel = std::mem::take(&mut st.scroll_sel);
    let area = egui::Area::new(id.with("suggest_popup")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
            // Breite nach dem längsten Vorschlag (Zeichen in Festbreite + Symbol + Art rechts)
            let longest = st.sugg.iter().map(|s| s.label.chars().count()).max().unwrap_or(0) as f32;
            ui.set_width((longest * 8.0 + 24.0 + 12.0 + 110.0).clamp(300.0, 560.0));
            egui::ScrollArea::vertical().max_height(250.0).auto_shrink([true, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (i, s) in st.sugg.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                    let selected = st.sel == Some(i);
                    if selected {
                        ui.painter().rect_filled(rect, 4.0, style::accent().gamma_multiply(0.35));
                        if scroll_sel {
                            ui.scroll_to_rect(rect, None);
                        }
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let col = kind_color(s.kind);
                    let badge = Rect::from_min_size(Pos2::new(rect.min.x + 3.0, rect.center().y - 8.0), Vec2::new(24.0, 16.0));
                    ui.painter().rect_filled(badge, 4.0, col.gamma_multiply(0.25));
                    ui.painter().text(badge.center(), egui::Align2::CENTER_CENTER, kind_symbol(s.kind), FontId::monospace(10.0), col);
                    ui.painter().text(Pos2::new(badge.max.x + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, &s.label, FontId::monospace(13.0), ui.visuals().text_color());
                    ui.painter().text(Pos2::new(rect.max.x - 6.0, rect.center().y), egui::Align2::RIGHT_CENTER, kind_name(s.kind, lang), FontId::proportional(11.0), style::dim(ui));
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        *clicked = Some(i);
                    }
                }
            });
            ui.add_space(2.0);
            ui.label(RichText::new(lang.t("↑↓ wählen · Tab/Enter übernehmen · Esc schließen", "↑↓ choose · Tab/Enter accept · Esc close")).size(10.5).color(style::dim(ui)));
        });
    });
    area.response.rect
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [SuggestionKind; 7] = [
        SuggestionKind::Field,
        SuggestionKind::Operator,
        SuggestionKind::Value,
        SuggestionKind::Keyword,
        SuggestionKind::Function,
        SuggestionKind::Predicate,
        SuggestionKind::SortKey,
    ];

    #[test]
    fn every_suggestion_kind_has_a_symbol_and_both_names() {
        for k in KINDS {
            assert!(!kind_symbol(k).is_empty());
            let (de, en) = (kind_name(k, Lang::De), kind_name(k, Lang::En));
            assert!(!de.is_empty() && !en.is_empty());
        }
        let symbols: Vec<&str> = KINDS.iter().map(|k| kind_symbol(*k)).collect();
        for (i, s) in symbols.iter().enumerate() {
            assert!(!symbols[i + 1..].contains(s), "Symbol doppelt: {s}");
        }
        assert_ne!(kind_name(SuggestionKind::Field, Lang::De), kind_name(SuggestionKind::Field, Lang::En));
    }

    #[test]
    fn set_query_puts_the_cursor_at_the_end_and_asks_for_focus() {
        let mut st = QueryBarState::new("t");
        st.history.push("old");
        st.set_query("status = Größe");
        assert_eq!(st.text, "status = Größe");
        assert_eq!(st.cursor_next, Some(st.text.len()));
        assert!(st.focus_next);
        assert!(!st.open);
    }

    #[test]
    fn bars_with_different_names_have_different_field_ids() {
        assert_ne!(QueryBarState::new("a").id(), QueryBarState::new("b").id());
        assert_eq!(QueryBarState::new("a").id(), QueryBarState::new("a").id());
    }

    // ---- Headless-Läufe der Leiste (egui ohne Fenster: Ereignisse werden von Hand eingespeist)

    use crate::query::FieldType;
    use eframe::egui::{Event, Key, Modifiers, RawInput};

    struct Harness {
        ctx: egui::Context,
        st: QueryBarState,
        schema: Schema,
        t: f64,
    }

    impl Harness {
        fn new() -> Harness {
            let schema = Schema::new()
                .with_field("status", FieldType::Option(vec!["To Do".into(), "Done".into(), "In Progress".into()]))
                .with_field("type", FieldType::Option(vec!["Epic".into(), "Defect".into()]))
                .with_field("summary", FieldType::Text)
                .with_sort_keyword("favorites");
            let mut h = Harness { ctx: egui::Context::default(), st: QueryBarState::new("test"), schema, t: 0.0 };
            h.st.set_query("");
            h.frame(vec![]);
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<Event>) -> QueryBarOutput {
            self.t += 0.05;
            let modifiers = events.iter().find_map(|e| if let Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
            let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))), events, focused: true, time: Some(self.t), modifiers, ..Default::default() };
            let mut out = None;
            let (st, schema) = (&mut self.st, &self.schema);
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    out = Some(query_bar(ui, st, &QueryBarCtx::new(schema, Lang::En)));
                });
            });
            out.unwrap()
        }

        fn typed(&mut self, s: &str) -> QueryBarOutput {
            self.frame(vec![Event::Text(s.to_string())])
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) -> QueryBarOutput {
            let o = self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
            self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
            o
        }

        fn labels(&self) -> Vec<String> {
            self.st.sugg.iter().map(|s| s.label.clone()).collect()
        }
    }

    #[test]
    fn typing_opens_suggestions_and_tab_accepts() {
        let mut h = Harness::new();
        assert!(h.ctx.memory(|m| m.has_focus(h.st.id())), "das Feld hat den Fokus");
        h.typed("sta");
        assert_eq!(h.st.text, "sta");
        assert!(h.st.open);
        assert_eq!(h.labels(), vec!["status"]);
        h.key(Key::Tab, Modifiers::NONE);
        assert_eq!(h.st.text, "status ", "Tab übernimmt den ersten Vorschlag samt Leerzeichen");
        assert!(h.labels().contains(&"=".to_string()), "gleich folgen die Operatoren: {:?}", h.labels());
    }

    #[test]
    fn arrows_choose_and_enter_accepts_only_after_a_choice() {
        let mut h = Harness::new();
        h.typed("status = ");
        assert!(h.labels().contains(&"Done".to_string()));
        // Enter ohne Auswahl schickt ab (hier: unfertig, also nicht abgeschickt) und übernimmt nichts
        let o = h.key(Key::Enter, Modifiers::NONE);
        assert!(o.submitted.is_none());
        assert_eq!(h.st.text, "status = ");
        // Pfeil runter wählt den ersten Wert, Enter übernimmt ihn
        h.key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(h.st.sel, Some(0));
        let first = h.labels()[0].clone();
        let o = h.key(Key::Enter, Modifiers::NONE);
        assert!(o.submitted.is_none(), "Enter nach der Auswahl übernimmt nur");
        assert!(h.st.text.starts_with("status = "), "{}", h.st.text);
        assert!(h.st.text.contains(first.trim_matches('"')), "{} in {}", first, h.st.text);
    }

    #[test]
    fn enter_submits_a_valid_query_and_fills_the_history() {
        let mut h = Harness::new();
        h.typed("status = Done");
        let o = h.key(Key::Enter, Modifiers::NONE);
        assert_eq!(o.submitted.as_deref(), Some("status = Done"));
        assert!(o.history_changed);
        assert_eq!(h.st.history.newest_first().collect::<Vec<_>>(), vec!["status = Done"]);
        assert!(!h.st.open);
    }

    #[test]
    fn invalid_queries_are_not_submitted_and_show_messages() {
        let mut h = Harness::new();
        let o = h.typed("stauts = Done");
        assert!(!o.valid);
        let o = h.key(Key::Enter, Modifiers::NONE);
        assert!(o.submitted.is_none());
        assert!(h.st.history.is_empty());
        // die Korrektur wird angeboten und stellt die Abfrage her
        let d = diagnose(&h.st.text, &h.schema, Lang::En);
        assert_eq!(d[0].fix.as_ref().map(|f| f.shown.as_str()), Some("status"));
        h.st.text = apply_fix(&h.st.text, d[0].fix.as_ref().unwrap()).0;
        let o = h.frame(vec![]);
        assert!(o.valid);
        // leer ist gültig und wird als leere Abfrage abgeschickt
        h.st.text.clear();
        let o = h.key(Key::Enter, Modifiers::NONE);
        assert_eq!(o.submitted.as_deref(), Some(""));
    }

    #[test]
    fn shift_enter_makes_a_new_line_and_enter_still_submits() {
        let mut h = Harness::new();
        h.typed("status = Done");
        h.key(Key::Enter, Modifiers::SHIFT);
        assert_eq!(h.st.text, "status = Done\n", "Umschalt+Enter fügt einen Zeilenumbruch ein");
        h.typed("AND type = Epic");
        let o = h.key(Key::Enter, Modifiers::NONE);
        assert_eq!(o.submitted.as_deref(), Some("status = Done\nAND type = Epic"));
    }

    #[test]
    fn arrow_up_recalls_the_history_and_down_restores_it() {
        let mut h = Harness::new();
        for q in ["type = Epic", "status = Done"] {
            h.st.text = q.to_string();
            h.key(Key::Enter, Modifiers::NONE);
        }
        h.st.text.clear();
        h.st.open = false;
        h.frame(vec![]);
        h.key(Key::ArrowUp, Modifiers::NONE);
        assert_eq!(h.st.text, "status = Done");
        h.key(Key::ArrowUp, Modifiers::NONE);
        assert_eq!(h.st.text, "type = Epic");
        h.st.open = false;
        h.key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(h.st.text, "status = Done");
    }

    #[test]
    fn escape_closes_the_popup_and_ctrl_space_reopens_it() {
        let mut h = Harness::new();
        h.typed("ty");
        assert!(h.st.open && !h.labels().is_empty());
        h.key(Key::Escape, Modifiers::NONE);
        assert!(!h.st.open);
        assert!(h.ctx.memory(|m| m.has_focus(h.st.id())), "Esc schließt nur das Popup, das Feld behält den Fokus");
        h.key(Key::Space, Modifiers::CTRL);
        assert!(h.st.open);
        assert_eq!(h.st.text, "ty", "Strg+Leertaste fügt kein Leerzeichen ein");
    }

    #[test]
    fn umlauts_survive_typing_and_accepting() {
        let mut h = Harness::new();
        h.schema = h.schema.clone().with_field("Priorität", FieldType::Option(vec!["Hoch".into()]));
        h.typed("Prio");
        h.key(Key::Tab, Modifiers::NONE);
        assert_eq!(h.st.text, "Priorität ");
        h.typed("= ");
        h.typed("Ho");
        h.key(Key::Tab, Modifiers::NONE);
        assert_eq!(h.st.text, "Priorität = Hoch");
        let o = h.key(Key::Enter, Modifiers::NONE);
        assert_eq!(o.submitted.as_deref(), Some("Priorität = Hoch"));
    }

    #[test]
    fn logic_helpers_are_reexported_consistently() {
        // die Leiste rechnet Cursor-Positionen über dieselben Helfer wie die Tests der Logik
        let t = "Größe";
        assert_eq!(super::super::query_logic::byte_of(t, 3), 4);
        assert_eq!(char_of(t, 4), 3);
    }
}
