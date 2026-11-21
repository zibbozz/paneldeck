//! Suchen/Ersetzen-Leiste (#54: regulärer Ausdruck, Groß/Klein, ganzes Wort, einmal/alle ersetzen) und „Gehe zu Zeile“.
//! Gesucht wird mit dem Kern (`crate::editor`); hier liegen nur Eingaben, Zähler und Hervorhebung.

use super::doc::{self, DocEntry};
use super::text::View;
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Key, Margin, Rect, RichText, Ui, Vec2};
use std::ops::Range;

/// Suchanfrage (Text und Schalter).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub(crate) struct Query {
    pub text: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
}

/// Höchstzahl gezählter Treffer (darüber „10000+“).
pub(super) const COUNT_CAP: usize = 10_000;

#[derive(Default)]
pub(crate) struct FindBar {
    pub visible: bool,
    pub replace_mode: bool,
    pub query: String,
    pub replacement: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    focus_query: bool,
    opened_now: bool,
    /// Ungültiger regulärer Ausdruck.
    pub error: Option<String>,
    /// Zählung für (Dokument-Revision, Anfrage): Anzahl (gedeckelt) und Nummer des aktuellen Treffers.
    counted: Option<(u64, Query)>,
    count: usize,
    current: Option<Range<usize>>,
}

/// Was die Hervorhebung im Textfeld braucht.
pub(crate) struct Highlight {
    pub query: Query,
    pub current: Option<Range<usize>>,
}

impl Highlight {
    /// Treffer im Bereich `range` (Zeichen) – nur für den sichtbaren Ausschnitt, höchstens 2000.
    pub fn matches_in(&self, d: &crate::editor::Document, range: Range<usize>) -> Vec<Range<usize>> {
        doc::find_in_range(d, &self.query, range, 2000)
    }

    pub fn is_current(&self, r: &Range<usize>) -> bool {
        self.current.as_ref() == Some(r)
    }
}

impl FindBar {
    /// Leiste öffnen (`replace`: mit Ersetzen-Feld) und das Suchfeld fokussieren.
    pub fn open(&mut self, replace: bool) {
        if !self.visible {
            self.opened_now = true;
        }
        self.visible = true;
        self.replace_mode = replace || (self.replace_mode && self.visible && !self.opened_now);
        self.focus_query = true;
    }

    /// Gerade geöffnet (einmalig `true`): der Bereich übernimmt dann die Auswahl als Suchtext.
    pub fn just_opened(&mut self) -> bool {
        std::mem::take(&mut self.opened_now)
    }

    pub fn height(&self) -> f32 {
        if self.replace_mode { 64.0 } else { 34.0 }
    }

    pub fn query(&self) -> Query {
        Query { text: self.query.clone(), case: self.case, word: self.word, regex: self.regex }
    }

    /// Hervorhebung für das Textfeld (nur bei sichtbarer Leiste und gültiger Anfrage).
    pub fn highlight(&self) -> Option<Highlight> {
        (self.visible && !self.query.is_empty() && self.error.is_none()).then(|| Highlight { query: self.query(), current: self.current.clone() })
    }

    /// Treffer neu zählen, wenn sich Text oder Anfrage geändert haben.
    fn recount(&mut self, d: &crate::editor::Document) {
        let q = self.query();
        let key = (d.revision(), q.clone());
        if self.counted.as_ref() == Some(&key) {
            return;
        }
        self.error = doc::check_query(&q);
        self.count = if q.text.is_empty() || self.error.is_some() { 0 } else { doc::count_matches(d, &q, COUNT_CAP + 1) };
        self.counted = Some(key);
    }
}

/// Leiste in `r` zeichnen und bedienen. Liefert, ob der Text geändert wurde (Ersetzen).
pub(super) fn show_bar(ui: &mut Ui, r: Rect, f: &mut FindBar, view: &mut View, d: &mut DocEntry, lang: Lang) -> bool {
    let u = style::ui();
    ui.painter().rect_filled(r, 0.0, u.surface);
    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], egui::Stroke::new(1.0_f32, u.border));
    let Some(docm) = d.doc.as_mut() else { return false };
    f.recount(docm);
    let mut edited = false;
    let mut go: Option<bool> = None; // Some(vorwärts)
    let mut replace_one = false;
    let mut replace_all = false;
    let mut close = false;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(Vec2::new(10.0, 4.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    egui::Frame::NONE.inner_margin(Margin::ZERO).show(ui, |ui| {
        ui.horizontal(|ui| {
            let resp = ui.add(egui::TextEdit::singleline(&mut f.query).hint_text(lang.t("Suchen", "Find")).desired_width(220.0));
            if std::mem::take(&mut f.focus_query) {
                resp.request_focus();
                if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(f.query.chars().count()))));
                    st.store(ui.ctx(), resp.id);
                }
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = Some(!ui.input(|i| i.modifiers.shift));
                resp.request_focus();
            }
            if resp.changed() {
                f.current = None;
            }
            for (on, label, tip) in [
                (&mut f.case, "Aa", lang.t("Groß-/Kleinschreibung beachten", "Match case")),
                (&mut f.word, "W", lang.t("Nur ganze Wörter", "Whole word")),
                (&mut f.regex, ".*", lang.t("Regulärer Ausdruck", "Regular expression")),
            ] {
                if ui.add(egui::Button::new(RichText::new(label).monospace()).selected(*on).min_size(Vec2::new(26.0, 20.0))).on_hover_text(tip).clicked() {
                    *on = !*on;
                    f.current = None;
                }
            }
            let count = if f.query.is_empty() {
                String::new()
            } else if let Some(e) = &f.error {
                e.clone()
            } else if f.count == 0 {
                lang.t("Keine Treffer", "No results").to_string()
            } else if f.count > COUNT_CAP {
                format!("{COUNT_CAP}+")
            } else {
                format!("{} {}", f.count, lang.t("Treffer", "results"))
            };
            ui.label(RichText::new(count).small().color(if f.error.is_some() { u.error } else { style::dim(ui) }));
            if ui.small_button("↑").on_hover_text(lang.t("Vorheriger Treffer (Umschalt+Enter)", "Previous match (Shift+Enter)")).clicked() {
                go = Some(false);
            }
            if ui.small_button("↓").on_hover_text(lang.t("Nächster Treffer (Enter)", "Next match (Enter)")).clicked() {
                go = Some(true);
            }
            if ui.small_button(if f.replace_mode { lang.t("Ersetzen ▴", "Replace ▴") } else { lang.t("Ersetzen ▾", "Replace ▾") }).clicked() {
                f.replace_mode = !f.replace_mode;
            }
            if style::icon_btn_small(ui, style::Icon::Close, lang.t("Schließen (Esc)", "Close (Esc)")).clicked() {
                close = true;
            }
        });
        if f.replace_mode {
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut f.replacement).hint_text(lang.t("Ersetzen durch", "Replace with")).desired_width(220.0));
                if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    replace_one = true;
                    resp.request_focus();
                }
                if ui.small_button(lang.t("Ersetzen", "Replace")).clicked() {
                    replace_one = true;
                }
                if ui.small_button(lang.t("Alle ersetzen", "Replace all")).clicked() {
                    replace_all = true;
                }
                if f.regex {
                    ui.label(RichText::new(lang.t("$1 … = Gruppen", "$1 … = groups")).small().color(style::dim(ui)));
                }
            });
        }
    });
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        close = true;
    }
    let q = f.query();
    if q.text.is_empty() || f.error.is_some() {
        go = None;
        replace_one = false;
        replace_all = false;
    }
    if let Some(sels) = view.sels.as_mut() {
        if replace_all {
            let n = doc::replace_all(docm, sels, &q, &f.replacement);
            edited |= n > 0;
            f.current = None;
        } else if replace_one {
            // Ist die Auswahl genau ein Treffer, ersetzen; danach (oder sonst) zum nächsten.
            let prim = sels.primary();
            if let Some(m) = doc::find_next(docm, &q, prim.start(), true).filter(|m| *m == prim.range()) {
                edited |= doc::replace_range(docm, sels, &q, m, &f.replacement);
            }
            go = Some(true);
        }
        if let Some(fwd) = go {
            let prim = sels.primary();
            let from = if fwd { prim.end() } else { prim.start() };
            if let Some(m) = doc::find_next(docm, &q, from, fwd) {
                view.select_range(docm, m.clone());
                f.current = Some(m);
            }
        }
    }
    if edited {
        d.on_user_edit();
    }
    if close {
        f.visible = false;
        f.current = None;
    }
    edited
}

/// Eingabe des Dialogs „Gehe zu Zeile“.
#[derive(Default, Clone, Debug)]
pub(crate) struct GotoLine {
    pub text: String,
    pub focused: bool,
}

/// „42“ oder „42:7“ → (Zeile, Spalte) ab 0, Zeile begrenzt auf `max` Zeilen. Ungültig → `None`.
pub(crate) fn parse_goto(s: &str, max: usize) -> Option<(usize, usize)> {
    let s = s.trim();
    let (l, c) = match s.split_once([':', ',']) {
        Some((l, c)) => (l.trim(), Some(c.trim())),
        None => (s, None),
    };
    let line: usize = l.parse().ok()?;
    let col: usize = match c {
        Some(c) if !c.is_empty() => c.parse().ok()?,
        _ => 1,
    };
    Some((line.clamp(1, max.max(1)) - 1, col.max(1) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goto_parses_line_and_column() {
        assert_eq!(parse_goto("42", 100), Some((41, 0)));
        assert_eq!(parse_goto(" 7:3 ", 100), Some((6, 2)));
        assert_eq!(parse_goto("7,3", 100), Some((6, 2)));
        assert_eq!(parse_goto("500", 100), Some((99, 0)), "begrenzt auf die letzte Zeile");
        assert_eq!(parse_goto("0", 100), Some((0, 0)));
        assert_eq!(parse_goto("abc", 100), None);
        assert_eq!(parse_goto("", 100), None);
        assert_eq!(parse_goto("3:x", 100), None);
    }

    #[test]
    fn opening_takes_focus_once() {
        let mut f = FindBar::default();
        f.open(false);
        assert!(f.visible && !f.replace_mode);
        assert!(f.just_opened());
        assert!(!f.just_opened(), "nur einmal");
        f.open(true);
        assert!(f.replace_mode);
        assert!(f.height() > 40.0);
        assert!(f.highlight().is_none(), "leere Anfrage: keine Hervorhebung");
        f.query = "x".into();
        assert!(f.highlight().is_some());
    }
}
