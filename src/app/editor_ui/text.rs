//! Textfeld des Editors (#54): zeichnet nur die sichtbaren Zeilen (bleibt bei mehreren 100 MB flüssig), Cursor und
//! Auswahlen (mehrere), Zeilennummern, aktuelle Zeile, Minimap, weicher Umbruch; Maus (Klick, Ziehen, Doppel-/Dreifachklick),
//! Tastatur, Bildlauf, IME und Zwischenablage.
//!
//! Mehrere Cursor (#394, 3.3.0, alles mit eigenen Schaltern, Standard aus): Alt+Klick setzt einen weiteren Cursor bzw.
//! nimmt einen vorhandenen weg; die mittlere Maustaste zieht eine Spaltenauswahl über Bildschirmzeilen (Strg dazu =
//! hinzufügen), ein Klick setzt den Cursor (Strg dazu = einen weiteren). Höchstens [`crate::editor::MAX_CURSORS`].
//! Jede Änderung der Cursor/Auswahlen ohne Textänderung merkt sich die Ansicht für *Cursor/Auswahl zurücknehmen*.
//!
//! Im Syntax-Modus (#312) färbt es die sichtbaren Zeilen mit den Syntaxfarben des Dokuments (`crate::editor::syntax`,
//! Farben `code_*` des Themes); berechnet werden sie in einem Hilfsthread je Dokument, das Zeichnen wartet nie darauf.
//!
//! Positionen sind Zeichen-Indizes im Rope des Kerns. Spalten auf dem Bildschirm sind *visuelle* Spalten (Tabs bis zum
//! nächsten Tabstopp, breite Zeichen doppelt) im Raster der Monospace-Schrift. Sehr lange Zeilen (ab [`LONG_LINE`]
//! Zeichen) rechnen vereinfacht (ein Zeichen = eine Spalte), damit eine einzelne riesige Zeile nicht jedes Bild durchlaufen
//! werden muss.

use super::doc::{DocEntry, Op};
use crate::editor::syntax::{DocHighlight, Span, Tok};
use crate::editor::text as et;
use crate::editor::{Document, Selection, Selections};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Align2, Color32, Event, EventFilter, FontId, Id, Key, Modifiers, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// Ab dieser Länge (Zeichen) gilt eine Zeile als „sehr lang“ (vereinfachte Spaltenrechnung, kein Umbruch).
pub(super) const LONG_LINE: usize = 20_000;
const MINIMAP_W: f32 = 90.0;
const SCROLLBAR_W: f32 = 10.0;
const MINIMAP_ROW: f32 = 2.0;
/// Höchstzahl Spalten, die die Minimap je Zeile ansieht.
const MINIMAP_COLS: usize = 120;
/// Solange Syntaxfarben im Hilfsthread entstehen: so oft neu zeichnen, um sie abzuholen.
const SYNTAX_POLL: std::time::Duration = std::time::Duration::from_millis(16);

/// Farben der Syntax-Klassen aus dem Theme (Reihenfolge wie [`Tok::ALL`]).
pub(super) fn code_palette(u: &crate::theme::tokens::UiColors) -> [Color32; 17] {
    let mut p = [Color32::PLACEHOLDER; 17];
    for t in Tok::ALL {
        p[t.index()] = match t {
            Tok::Comment => u.code_comment,
            Tok::String => u.code_string,
            Tok::Number => u.code_number,
            Tok::Constant => u.code_constant,
            Tok::Keyword => u.code_keyword,
            Tok::Type => u.code_type,
            Tok::Function => u.code_function,
            Tok::Operator => u.code_operator,
            Tok::Tag => u.code_tag,
            Tok::Attribute => u.code_attribute,
            Tok::Preprocessor => u.code_preprocessor,
            Tok::Escape => u.code_escape,
            Tok::Heading => u.code_heading,
            Tok::Variable => u.code_variable,
            Tok::Invalid => u.code_invalid,
            Tok::Inserted => u.diff_add,
            Tok::Deleted => u.diff_remove,
        };
    }
    p
}

/// Ansicht eines Reiters: Cursor/Auswahlen, Bildlauf, Zustand der Maus.
#[derive(Clone, Debug, Default)]
pub(crate) struct View {
    /// Auswahlen (erst nach dem Laden des Dokuments gesetzt).
    pub sels: Option<Selections>,
    /// Gemerkte Lage aus `editor-state.json` (Zeile, Spalte) bis zum ersten Zeichnen.
    restore: Option<(usize, usize)>,
    /// Oberste sichtbare Zeile und Versatz (Pixel) innerhalb ihrer Zeilen auf dem Bildschirm.
    pub top_line: usize,
    pub top_off: f32,
    pub scroll_x: f32,
    restore_top: Option<f32>,
    drag: Option<DragMode>,
    last_press: Option<(f64, Pos2, u8)>,
    /// Nach Tastatur/Bearbeitung den Cursor in den sichtbaren Bereich holen.
    pub reveal: bool,
    /// Laufende IME-Eingabe (Zwischenstand).
    preedit: String,
    minimap_drag: Option<f32>,
    scroll_drag: Option<f32>,
    blink_epoch: f64,
    /// Waagerechte Bildlaufleiste (#389).
    pub(super) hscroll: super::hscroll::HScroll,
    /// Frühere Cursor/Auswahlen für *Cursor/Auswahl zurücknehmen* (#394, Strg+U), neueste zuletzt.
    sel_undo: Vec<Selections>,
}

/// So viele Stände merkt sich *Cursor/Auswahl zurücknehmen*.
const SEL_UNDO_MAX: usize = 200;
/// … und höchstens so viele Auswahlen in allen Ständen zusammen (viele Cursor: Speicher begrenzen).
const SEL_UNDO_TOTAL: usize = 200_000;

#[derive(Clone, Debug, PartialEq)]
enum DragMode {
    /// Zeichenweise ab `anchor`.
    Chars(usize),
    /// Wortweise ab dem Wort `start..end`.
    Words(usize, usize),
    /// Zeilenweise ab den Zeilen `start..end` (Positionen).
    Lines(usize, usize),
    /// Alt+Klick: weiterer Cursor an `pos`; Ziehen markiert ab dort (eine Auswahl, keine Spalte – #394).
    AddCursor(usize),
    /// Mittlere Maustaste (#394): Druckpunkt (Zeile, Bildschirmzeile darin, visuelle Spalte in ihr, Zeigerposition),
    /// mit Strg die Auswahlen davor (hinzufügen), gezogen? `before` = Stand beim Drücken (für Strg+U).
    Middle { line: usize, row: usize, v: usize, press: Pos2, base: Option<Selections>, moved: bool, before: Option<Selections> },
}

impl View {
    pub fn restored(line: usize, col: usize, top: f32) -> Self {
        View { restore: Some((line, col)), restore_top: Some(top), ..Default::default() }
    }

    /// Lage für `editor-state.json` ohne geladenes Dokument.
    pub fn restore_pos(&self) -> (usize, usize) {
        self.restore.unwrap_or((0, 0))
    }

    pub fn top_f32(&self) -> f32 {
        self.restore_top.unwrap_or(self.top_line as f32)
    }

    /// Cursor (Zeile, Spalte) zum Merken.
    pub fn cursor_linecol(&self, d: &DocEntry) -> (usize, usize) {
        match (&self.sels, d.doc.as_ref()) {
            (Some(s), Some(doc)) => {
                let lc = et::pos_to_linecol(doc.rope(), s.primary().head);
                (lc.line, lc.col)
            }
            _ => self.restore_pos(),
        }
    }

    /// Auswahlen mit dem Dokument abgleichen (Änderungen über andere Ansichten) bzw. aus der gemerkten Lage anlegen.
    pub fn sync(&mut self, doc: &Document) {
        match &mut self.sels {
            Some(s) => doc.sync(s),
            None => {
                let (line, col) = self.restore.take().unwrap_or((0, 0));
                let pos = et::linecol_to_pos(doc.rope(), et::LineCol::new(line, col));
                self.sels = Some(doc.new_selections(pos));
                let max = doc.len_lines().saturating_sub(1);
                self.top_line = self.restore_top.take().map(|t| t.max(0.0) as usize).unwrap_or(line.saturating_sub(5)).min(max);
                self.top_off = 0.0;
            }
        }
    }

    pub fn selection_count(&self) -> usize {
        self.sels.as_ref().map(|s| s.len()).unwrap_or(1)
    }

    /// Änderung der Cursor/Auswahlen (ohne Textänderung) merken: `before` war der Stand davor (#394).
    pub fn note_selection_change(&mut self, before: Selections) {
        if self.sels.as_ref().is_some_and(|s| s.as_slice() == before.as_slice()) {
            return;
        }
        if self.sel_undo.last().is_some_and(|l| l.as_slice() == before.as_slice()) {
            return;
        }
        // Grenze der Auswahlen insgesamt: ein Stand allein darüber wird nicht gemerkt, sonst fallen die ältesten weg.
        if before.len() > SEL_UNDO_TOTAL {
            return;
        }
        let mut total: usize = self.sel_undo.iter().map(Selections::len).sum();
        while !self.sel_undo.is_empty() && (self.sel_undo.len() >= SEL_UNDO_MAX || total + before.len() > SEL_UNDO_TOTAL) {
            total -= self.sel_undo.remove(0).len();
        }
        self.sel_undo.push(before);
    }

    #[cfg(test)]
    pub fn sel_undo_len(&self) -> usize {
        self.sel_undo.len()
    }

    #[cfg(test)]
    pub fn sel_undo_total(&self) -> usize {
        self.sel_undo.iter().map(Selections::len).sum()
    }

    /// *Cursor/Auswahl zurücknehmen* (#394): den vorigen Stand der Cursor/Auswahlen, der Text bleibt. Liefert, ob es
    /// einen gab.
    pub fn soft_undo(&mut self, doc: &Document) -> bool {
        while let Some(mut s) = self.sel_undo.pop() {
            doc.sync(&mut s);
            if self.sels.as_ref().is_some_and(|c| c.as_slice() == s.as_slice()) {
                continue;
            }
            self.sels = Some(s);
            return true;
        }
        false
    }

    /// Text der Auswahlen (mehrere durch Zeilenumbruch getrennt); `None`, wenn nichts markiert ist.
    pub fn selected_text(&self, d: &DocEntry) -> Option<String> {
        let (s, doc) = (self.sels.as_ref()?, d.doc.as_ref()?);
        let parts: Vec<String> = s.iter().filter(|x| !x.is_empty()).map(|x| doc.slice_string(x.range())).collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    /// „Z. 3, Sp. 5“ (1-basiert; Spalte visuell); mit mehreren Cursorn „3 Cursor“ bzw. „3 Auswahlen“ (#394).
    pub fn position_text(&self, d: &DocEntry, lang: Lang) -> String {
        if let (Some(s), Some(_)) = (&self.sels, d.doc.as_ref())
            && s.is_multi()
        {
            let n = s.len();
            let any = s.iter().any(|x| !x.is_empty());
            return match (lang, any) {
                (Lang::De, false) => format!("{n} Cursor"),
                (Lang::De, true) => format!("{n} Auswahlen"),
                (_, false) => format!("{n} cursors"),
                (_, true) => format!("{n} selections"),
            };
        }
        let (line, col) = match (&self.sels, d.doc.as_ref()) {
            (Some(s), Some(doc)) => {
                let lc = et::pos_to_linecol(doc.rope(), s.primary().head);
                (lc.line, vcol(doc, lc.line, lc.col, doc.tab_width()))
            }
            _ => self.restore_pos(),
        };
        let sel: usize = match (&self.sels, d.doc.as_ref()) {
            (Some(s), Some(_)) => s.iter().map(|x| x.end() - x.start()).sum(),
            _ => 0,
        };
        let base = format!("{} {}, {} {}", lang.t("Z.", "Ln"), line + 1, lang.t("Sp.", "Col"), col + 1);
        if sel > 0 { format!("{base} ({sel} {})", lang.t("markiert", "selected")) } else { base }
    }

    /// Cursor auf Zeile/Spalte setzen (ab 0) und dorthin rollen.
    pub fn goto(&mut self, d: &DocEntry, line: usize, col: usize) {
        let Some(doc) = d.doc.as_ref() else {
            self.restore = Some((line, col));
            return;
        };
        let pos = et::linecol_to_pos(doc.rope(), et::LineCol::new(line, col));
        self.sels = Some(doc.new_selections(pos));
        self.reveal = true;
    }

    /// Auswahl auf `range` setzen (Suchen) und dorthin rollen.
    pub fn select_range(&mut self, doc: &Document, range: std::ops::Range<usize>) {
        let mut s = doc.new_selections(range.end);
        s.set_primary(Selection::new(range.start, range.end));
        self.sels = Some(s);
        self.reveal = true;
    }
}

/// Visuelle Spalte von `col` in `line` (sehr lange Zeilen: = `col`).
pub(super) fn vcol(doc: &Document, line: usize, col: usize, tab: usize) -> usize {
    if doc.line_len(line) >= LONG_LINE { col } else { et::visual_col(doc.rope(), line, col, tab) }
}

/// Zeichenspalte zu einer visuellen Spalte in `line` (sehr lange Zeilen: = `v`, begrenzt).
pub(super) fn col_at(doc: &Document, line: usize, v: usize, tab: usize) -> usize {
    let len = doc.line_len(line);
    if len >= LONG_LINE { v.min(len) } else { et::col_for_visual(doc.rope(), line, v, tab) }
}

/// Anfänge (Zeichenspalten) der Bildschirmzeilen einer Textzeile bei Umbruch nach `cols` Spalten: bevorzugt nach einem
/// Leerzeichen, sonst mitten im Wort. Ohne Umbruch (oder sehr lange Zeile): `[0]`.
pub(super) fn rows_of(doc: &Document, line: usize, cols: Option<usize>, tab: usize) -> Vec<usize> {
    let Some(cols) = cols.filter(|c| *c >= 8) else { return vec![0] };
    let len = doc.line_len(line);
    if len >= LONG_LINE || len <= cols / 2 {
        return vec![0];
    }
    let mut rows = vec![0];
    let mut v = 0usize; // visuelle Spalte ab Zeilenanfang
    let mut row_v = 0usize; // visuelle Spalte, an der die aktuelle Bildschirmzeile beginnt
    let mut last_space: Option<(usize, usize)> = None; // (Spalte nach dem Leerzeichen, visuelle Spalte dort)
    for (i, c) in et::line_slice(doc.rope(), line).chars().enumerate() {
        let w = et::char_width(c, v, tab);
        if v + w - row_v > cols && i > *rows.last().unwrap_or(&0) {
            match last_space.filter(|(col, _)| *col > *rows.last().unwrap_or(&0)) {
                Some((col, sv)) => {
                    rows.push(col);
                    row_v = sv;
                }
                None => {
                    rows.push(i);
                    row_v = v;
                }
            }
            last_space = None;
        }
        v += w;
        if c == ' ' || c == '\t' {
            last_space = Some((i + 1, v));
        }
    }
    rows
}

/// Gemeinsame Werte für das Zeichnen.
pub(super) struct TextCx<'a> {
    pub id: Id,
    pub font: FontId,
    pub theme: &'a crate::theme::Theme,
    pub prefs: &'a super::EditorPrefs,
    pub wrap: bool,
    pub minimap: bool,
    pub request_focus: bool,
    pub find: Option<&'a super::find::Highlight>,
    pub lang: Lang,
    /// Änderungsmarken am Rand (#385; `None` = aus).
    pub marks: Option<&'a crate::editor::linemarks::LineMarks>,
    /// Zeile, unter der das Fenster einer Marke offen ist (liefert seinen Anker in [`TextOut::popup_anchor`]).
    pub popup_line: Option<usize>,
    /// Diagnosen des Smart-Modus (#367): Wellenlinien und Meldung beim Überfahren.
    pub diags: Option<&'a [super::smart::Shown]>,
}

/// Ergebnis eines Bildes.
#[derive(Default)]
pub(super) struct TextOut {
    pub focused: bool,
    pub clicked: bool,
    /// Der Text wurde über diese Ansicht geändert.
    pub edited: bool,
    /// Klick auf eine Änderungsmarke in dieser Zeile (#385).
    pub strip_click: Option<usize>,
    /// Unterkante der Zeile [`TextCx::popup_line`] (links am Text) und Breite des Textfelds, wenn sie sichtbar ist.
    pub popup_anchor: Option<(Pos2, f32)>,
    /// Unterkante des Hauptcursors, wenn er sichtbar ist (Popup der Vervollständigung, #367).
    pub caret: Option<Pos2>,
    /// Hinweis für die Statuszeile (z. B. „Nur die ersten 10 000 Cursor.“, #394).
    pub hint: Option<String>,
}

/// Eine sichtbare Bildschirmzeile.
#[derive(Clone, Copy, Debug)]
struct Row {
    line: usize,
    /// Zeichenspalten `start..end` der Zeile.
    start: usize,
    end: usize,
    /// Visuelle Spalte am Anfang.
    vstart: usize,
    y: f32,
    first: bool,
}

/// Maße des Textfelds.
pub(super) struct Geo {
    text: Rect,
    gutter: Rect,
    scrollbar: Rect,
    row_h: f32,
    cw: f32,
    wrap_cols: Option<usize>,
    tab: usize,
}

impl Geo {
    fn x_of(&self, r: &Row, v: usize, scroll_x: f32) -> f32 {
        self.text.left() + (v as f32 - r.vstart as f32) * self.cw - if self.wrap_cols.is_some() { 0.0 } else { scroll_x }
    }
}

/// Textfeld in `rect` zeichnen und Eingaben verarbeiten.
pub(super) fn show(ui: &mut Ui, rect: Rect, d: &mut DocEntry, view: &mut View, cx: &TextCx) -> TextOut {
    let mut out = TextOut::default();
    let resp = ui.interact(rect, cx.id, Sense::click_and_drag());
    if cx.request_focus {
        resp.request_focus();
    }
    if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
        resp.request_focus();
        out.clicked = true;
    }
    let focused = resp.has_focus();
    out.focused = focused;
    if focused {
        ui.memory_mut(|m| m.set_focus_lock_filter(cx.id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false }));
    }
    let large = d.is_large(cx.prefs);
    d.ensure_highlight(cx.prefs);
    let Some(doc) = d.doc.as_mut() else {
        if d.is_loading() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        let msg = d.load_error.clone().unwrap_or_else(|| cx.lang.t("wird geladen …", "loading …").to_string());
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), style::dim(ui));
        return out;
    };
    view.sync(doc);
    // Für *Cursor/Auswahl zurücknehmen* (#394): Stand vor diesem Bild; Ziehen zählt nur beim Drücken. Nur kopieren,
    // wenn eine Eingabe die Auswahl ändern kann (nicht in jedem Bild – bei vielen Cursorn teuer).
    let was_dragging = view.drag.is_some();
    let may_change = !was_dragging && ui.input(|i| i.events.iter().any(|e| !matches!(e, egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_) | egui::Event::PointerGone)));
    let sel_before = if may_change { view.sels.clone() } else { None };
    let tab = doc.tab_width().max(1);
    let (row_h, cw) = ui.fonts_mut(|f| (f.row_height(&cx.font), f.glyph_width(&cx.font, 'M')));
    let cw = cw.max(1.0);
    let n_lines = doc.len_lines().max(1);
    let digits = n_lines.to_string().len().max(3) as f32;
    let gutter_w = (digits + 1.5) * cw + 8.0;
    let show_minimap = cx.minimap && !large && rect.width() > 420.0;
    let scrollbar = Rect::from_min_max(Pos2::new(rect.right() - SCROLLBAR_W, rect.top()), rect.max);
    let minimap = show_minimap.then(|| Rect::from_min_max(Pos2::new(scrollbar.left() - MINIMAP_W, rect.top()), Pos2::new(scrollbar.left(), rect.bottom())));
    let text_right = minimap.map(|m| m.left()).unwrap_or(scrollbar.left()) - 4.0;
    let gutter = Rect::from_min_max(rect.min, Pos2::new(rect.left() + gutter_w, rect.bottom()));
    let mut text = Rect::from_min_max(Pos2::new(gutter.right() + 6.0, rect.top() + 2.0), Pos2::new(text_right.max(gutter.right() + 20.0), rect.bottom()));
    let wrap_cols = (cx.wrap && !large).then(|| ((text.width() / cw).floor() as usize).max(8));
    let mut hbar = hbar_prepare(ui, doc, &mut d.width, view, wrap_cols.is_none(), tab, cw, &mut text, gutter.right(), minimap.map(|m| m.left()).unwrap_or(scrollbar.left()), rect.bottom());
    let geo = Geo { text, gutter, scrollbar, row_h, cw, wrap_cols, tab };

    // ---- Eingaben ----
    let mut edited = false;
    if focused {
        edited |= keyboard(ui, doc, view, &geo, cx);
    }
    // Mausrad
    if resp.hovered() || ui.rect_contains_pointer(rect) {
        let (dy, dx, shift) = ui.input(|i| (i.smooth_scroll_delta.y, i.smooth_scroll_delta.x, i.modifiers.shift));
        // Umschalt+Rad und das Rad über der waagerechten Leiste rollen waagerecht.
        let over_hbar = hbar.as_ref().and_then(|h| h.area).is_some_and(|a| ui.rect_contains_pointer(a));
        let (dy, dx) = if (shift || over_hbar) && dx == 0.0 { (0.0, dy) } else { (dy, dx) };
        if dy != 0.0 {
            view.top_off -= dy;
            normalize_scroll(doc, view, &geo);
        }
        if dx != 0.0 && wrap_cols.is_none() {
            view.scroll_x = (view.scroll_x - dx).max(0.0);
        }
    }
    if view.reveal {
        view.reveal = false;
        reveal_cursor(doc, view, &geo);
    }
    normalize_scroll(doc, view, &geo);
    hbar_clamp(doc, view, tab, hbar.as_mut());

    // ---- sichtbare Zeilen ----
    let rows = visible_rows(doc, view, &geo);
    out.strip_click = strip_click(ui, &resp, &geo, &rows, cx);
    if out.strip_click.is_none() {
        let full;
        (edited, full) = {
            let (e, f) = mouse(ui, &resp, doc, view, &geo, &rows, cx);
            (edited | e, f)
        };
        if full {
            out.hint = Some(cx.lang.t("Nur die ersten 10 000 Cursor.", "Only the first 10 000 cursors.").to_string());
        }
    }
    let rows = if edited { visible_rows(doc, view, &geo) } else { rows };
    // Syntaxfarben der sichtbaren Zeilen (gemeinsam für alle Ansichten des Dokuments).
    let mut hl = d.syntax.hl.as_mut();
    if let (Some(h), Some(first), Some(last)) = (hl.as_deref_mut(), rows.first(), rows.last())
        && !h.prepare(doc, first.line..last.line + 1)
    {
        ui.ctx().request_repaint_after(SYNTAX_POLL);
    }
    hbar_clamp(doc, view, tab, hbar.as_mut()); // nach dem Rollen beim Ziehen
    paint(ui, rect, doc, view, &geo, &rows, cx, focused, hl.as_deref());
    // Smart-Modus (#367): Wellenlinien der Diagnosen, Meldung beim Überfahren.
    let scroll = if geo.wrap_cols.is_some() { 0.0 } else { view.scroll_x };
    if let Some(list) = cx.diags.filter(|l| !l.is_empty()) {
        let u = style::ui();
        let p = ui.painter_at(geo.text.expand2(Vec2::new(2.0, 0.0)));
        let mut hover: Option<&str> = None;
        let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| geo.text.contains(*p));
        let at = pointer.map(|pp| pos_at(doc, view, &geo, &rows, pp).0);
        for dgn in list {
            use crate::lsp::proto::Severity;
            let col = match dgn.severity {
                Severity::Error => u.diag_error,
                Severity::Warning => u.diag_warning,
                Severity::Information => u.diag_info,
                Severity::Hint => u.diag_hint,
            };
            let r = if dgn.range.is_empty() { dgn.range.start..(dgn.range.start + 1).min(doc.len_chars()) } else { dgn.range.clone() };
            paint_squiggle(&p, doc, &geo, &rows, r.clone(), scroll, col, dgn.severity == Severity::Hint || dgn.unnecessary, dgn.deprecated);
            if at.is_some_and(|a| a >= r.start && a <= r.end) && hover.is_none() {
                hover = Some(&dgn.message);
            }
        }
        if let Some(m) = hover {
            resp.clone().on_hover_text(m);
        }
    }
    if let Some(s) = view.sels.as_ref()
        && let Some(cp) = caret_pos(doc, &geo, &rows, s.primary().head, scroll)
    {
        out.caret = Some(cp + Vec2::new(0.0, geo.row_h));
    }
    if let Some(line) = cx.popup_line
        && let Some(r) = rows.iter().rfind(|r| r.line == line)
    {
        out.popup_anchor = Some((Pos2::new(geo.text.left(), r.y + geo.row_h + 2.0), geo.text.width()));
    }
    if let Some(m) = minimap {
        minimap_ui(ui, m, doc, view, &geo, cx);
        crate::app::tutorial::spot_rect(ui, crate::app::tutorial::Spot::EditorMinimap, m);
    }
    scrollbar_ui(ui, &geo, doc, view, &rows, cx);
    if let Some(h) = &hbar {
        hbar_ui(ui, h, &geo, view, cx, rect.bottom());
    }
    if focused {
        ime_output(ui, rect, doc, view, &geo, &rows, cx);
    }
    // Kontextmenü
    let mut menu_op = None;
    resp.context_menu(|ui| {
        let l = cx.lang;
        for (op, label) in [(Op::Cut, l.t("Ausschneiden", "Cut")), (Op::Copy, l.t("Kopieren", "Copy")), (Op::PasteFromClipboard, l.t("Einfügen", "Paste"))] {
            if ui.button(label).clicked() {
                menu_op = Some(op);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(l.t("Alles markieren", "Select all")).clicked() {
            menu_op = Some(Op::SelectAll);
            ui.close();
        }
        if ui.add_enabled(doc.can_undo(), egui::Button::new(l.t("Rückgängig", "Undo"))).clicked() {
            menu_op = Some(Op::Undo);
            ui.close();
        }
        if ui.add_enabled(doc.can_redo(), egui::Button::new(l.t("Wiederholen", "Redo"))).clicked() {
            menu_op = Some(Op::Redo);
            ui.close();
        }
    });
    if let Some(op) = menu_op {
        if let Some(sels) = view.sels.as_mut() {
            match op {
                Op::Copy => {
                    copy(ui, doc, sels, false);
                }
                Op::Cut => edited |= copy(ui, doc, sels, true),
                Op::PasteFromClipboard => {
                    // egui liefert den Inhalt der Zwischenablage nur als Ereignis: über die Plattform lesen.
                    if let Some(t) = crate::platform::clipboard::get_text() {
                        edited |= super::doc::run(doc, sels, Op::Paste(t), tab);
                    }
                }
                op => edited |= super::doc::run(doc, sels, op, tab),
            }
            view.reveal = true;
        }
        resp.request_focus();
    }
    if edited {
        d.on_user_edit();
        view.blink_epoch = ui.input(|i| i.time);
    } else if !was_dragging && let Some(b) = sel_before {
        view.note_selection_change(b);
    }
    out.edited = edited;
    if resp.hovered() && view.drag.is_none() && !ui.rect_contains_pointer(gutter) && !ui.rect_contains_pointer(scrollbar) && minimap.is_none_or(|m| !ui.rect_contains_pointer(m)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    out
}

/// Versatz in der obersten Zeile auf ihre Bildschirmzeilen begrenzen (über Zeilengrenzen weiterwandern).
fn normalize_scroll(doc: &Document, view: &mut View, g: &Geo) {
    let n = doc.len_lines().max(1);
    view.top_line = view.top_line.min(n - 1);
    let mut guard = 0;
    while view.top_off < 0.0 && view.top_line > 0 && guard < 10_000 {
        view.top_line -= 1;
        view.top_off += rows_of(doc, view.top_line, g.wrap_cols, g.tab).len() as f32 * g.row_h;
        guard += 1;
    }
    if view.top_off < 0.0 {
        view.top_off = 0.0;
    }
    loop {
        let h = rows_of(doc, view.top_line, g.wrap_cols, g.tab).len() as f32 * g.row_h;
        if view.top_off >= h && view.top_line + 1 < n && guard < 10_000 {
            view.top_off -= h;
            view.top_line += 1;
            guard += 1;
        } else {
            break;
        }
    }
    // Nicht weiter als bis zur letzten Zeile oben.
    if view.top_line + 1 >= n {
        let h = rows_of(doc, view.top_line, g.wrap_cols, g.tab).len() as f32 * g.row_h;
        view.top_off = view.top_off.min((h - g.row_h).max(0.0));
    }
}

/// Bildschirmzeilen, die in das Textfeld passen.
fn visible_rows(doc: &Document, view: &View, g: &Geo) -> Vec<Row> {
    let mut rows = Vec::new();
    let n = doc.len_lines();
    let mut y = g.text.top() - view.top_off;
    let mut line = view.top_line;
    while y < g.text.bottom() && line < n {
        let starts = rows_of(doc, line, g.wrap_cols, g.tab);
        let len = doc.line_len(line);
        for (i, s) in starts.iter().enumerate() {
            let end = starts.get(i + 1).copied().unwrap_or(len);
            if y + g.row_h > g.text.top() && y < g.text.bottom() {
                let vstart = if *s == 0 || g.wrap_cols.is_none() { 0 } else { vcol(doc, line, *s, g.tab) };
                rows.push(Row { line, start: *s, end, vstart, y, first: i == 0 });
            }
            y += g.row_h;
        }
        line += 1;
    }
    rows
}

/// Cursor (Kopf der Hauptauswahl) in den sichtbaren Bereich holen.
fn reveal_cursor(doc: &Document, view: &mut View, g: &Geo) {
    let Some(s) = &view.sels else { return };
    let head = s.primary().head;
    let lc = et::pos_to_linecol(doc.rope(), head);
    let starts = rows_of(doc, lc.line, g.wrap_cols, g.tab);
    let row = starts.iter().rposition(|s| *s <= lc.col).unwrap_or(0);
    let margin = 2.0 * g.row_h;
    let height = g.text.height();
    // Abstand von der obersten Zeile bis zur Cursorzeile (in Pixeln), begrenzt auf eine Bildschirmhöhe.
    if lc.line < view.top_line || (lc.line == view.top_line && (row as f32 * g.row_h) < view.top_off) {
        view.top_line = lc.line;
        view.top_off = (row as f32 * g.row_h - margin).max(0.0);
        if view.top_off == 0.0 && lc.line > 0 && row == 0 {
            view.top_off = -margin;
        }
    } else {
        let mut y = -view.top_off;
        let mut l = view.top_line;
        let mut found = None;
        while l <= lc.line && y < height * 2.0 {
            if l == lc.line {
                found = Some(y + row as f32 * g.row_h);
                break;
            }
            y += rows_of(doc, l, g.wrap_cols, g.tab).len() as f32 * g.row_h;
            l += 1;
        }
        match found {
            Some(cy) if cy + g.row_h + margin <= height => {}
            Some(cy) => view.top_off += cy + g.row_h + margin - height,
            None => {
                // Weit unten: von der Cursorzeile rückwärts eine Höhe auffüllen.
                let mut rest = height - margin - (row as f32 + 1.0) * g.row_h;
                let mut l = lc.line;
                while l > 0 && rest > 0.0 {
                    l -= 1;
                    rest -= rows_of(doc, l, g.wrap_cols, g.tab).len() as f32 * g.row_h;
                }
                view.top_line = l;
                view.top_off = (-rest).max(0.0);
            }
        }
    }
    normalize_scroll(doc, view, g);
    // Waagerecht (ohne Umbruch)
    if g.wrap_cols.is_none() {
        let x = vcol(doc, lc.line, lc.col, g.tab) as f32 * g.cw;
        let w = g.text.width();
        if x < view.scroll_x + g.cw * 2.0 {
            view.scroll_x = (x - w * 0.25).max(0.0);
        } else if x > view.scroll_x + w - g.cw * 3.0 {
            view.scroll_x = x - w * 0.75;
        }
    }
}

/// Position unter dem Zeiger (Zeichen-Index) und die visuelle Spalte dort.
fn pos_at(doc: &Document, view: &View, g: &Geo, rows: &[Row], p: Pos2) -> (usize, usize, usize) {
    let Some(first) = rows.first() else { return (0, 0, 0) };
    let r = if p.y < first.y {
        // Über dem Feld: Zeile davor
        let line = first.line.saturating_sub(if first.first && first.y >= g.text.top() - 0.5 { 1 } else { 0 });
        if line != first.line {
            let len = doc.line_len(line);
            let starts = rows_of(doc, line, g.wrap_cols, g.tab);
            let s = *starts.last().unwrap_or(&0);
            Row { line, start: s, end: len, vstart: if s == 0 { 0 } else { vcol(doc, line, s, g.tab) }, y: first.y - g.row_h, first: s == 0 }
        } else {
            *first
        }
    } else {
        rows.iter().rev().find(|r| p.y >= r.y).copied().unwrap_or(*first)
    };
    let r = if p.y > rows.last().map(|l| l.y + g.row_h).unwrap_or(0.0) && r.line + 1 < doc.len_lines() && rows.last().is_some_and(|l| l.line == r.line && l.end == doc.line_len(r.line)) {
        // Unter der letzten sichtbaren Zeile: nächste Zeile (Ziehen nach unten)
        let line = r.line + 1;
        let starts = rows_of(doc, line, g.wrap_cols, g.tab);
        Row { line, start: 0, end: starts.get(1).copied().unwrap_or(doc.line_len(line)), vstart: 0, y: r.y + g.row_h, first: true }
    } else {
        r
    };
    let scroll = if g.wrap_cols.is_some() { 0.0 } else { view.scroll_x };
    let v = (((p.x - g.text.left() + scroll) / g.cw).round().max(0.0) as usize) + r.vstart;
    let mut col = col_at(doc, r.line, v, g.tab).clamp(r.start, r.end);
    // Am Umbruch: das letzte Zeichen der Bildschirmzeile gehört schon zur nächsten.
    if g.wrap_cols.is_some() && col == r.end && r.end < doc.line_len(r.line) && col > r.start {
        col -= 1;
    }
    (doc.line_to_char(r.line) + col, r.line, v)
}

/// Mausbedienung: Klick setzt den Cursor, Ziehen markiert, Doppel-/Dreifachklick markiert Wort/Zeile, Umschalt+Klick
/// dehnt. Mehrere Cursor (#394, je mit eigenem Schalter): Alt+Klick fügt einen Cursor hinzu bzw. nimmt einen vorhandenen
/// weg; die mittlere Maustaste markiert eine Spalte bzw. setzt Cursor. Ziehen über den Rand rollt. Liefert (Text
/// geändert, Grenze der Cursor erreicht).
fn mouse(ui: &mut Ui, resp: &egui::Response, doc: &mut Document, view: &mut View, g: &Geo, rows: &[Row], cx: &TextCx) -> (bool, bool) {
    use egui::PointerButton::Middle;
    let (pressed, down, pos, time, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.time, i.modifiers));
    let (m_pressed, m_down) = ui.input(|i| (i.pointer.button_pressed(Middle), i.pointer.button_down(Middle)));
    let Some(p) = pos else { return (false, false) };
    let inside_text = Rect::from_min_max(Pos2::new(g.gutter.left(), g.text.top()), Pos2::new(g.text.right() + 4.0, g.text.bottom())).contains(p);
    if view.sels.is_none() {
        return (false, false);
    }
    let multi = cx.prefs.multi;
    // ---- Mittlere Maustaste (#394)
    if multi.middle && m_pressed && resp.contains_pointer() && inside_text && p.x >= g.text.left() - 2.0 {
        resp.request_focus();
        let (_, line, v) = pos_at(doc, view, g, rows, p);
        let r = rows.iter().rev().find(|r| p.y >= r.y && r.line == line).copied().or_else(|| rows.iter().find(|r| r.line == line).copied());
        let (row, rv) = r.map(|r| (rows_of(doc, r.line, g.wrap_cols, g.tab).iter().position(|s| *s == r.start).unwrap_or(0), r.vstart)).unwrap_or((0, 0));
        doc.break_undo_group();
        let before = view.sels.clone();
        view.drag = Some(DragMode::Middle { line, row, v: v.saturating_sub(rv), press: p, base: mods.ctrl.then(|| before.clone()).flatten(), moved: false, before });
        view.blink_epoch = time;
        return (false, false);
    }
    if let Some(DragMode::Middle { line, row, v, press, base, moved, before }) = view.drag.take() {
        if !m_down {
            let mut full = false;
            if !moved {
                // Klick ohne Ziehen: Cursor setzen (mit Strg einen dazu).
                let (pos, _, _) = pos_at(doc, view, g, rows, press);
                if let Some(sels) = view.sels.as_mut() {
                    match base {
                        Some(b) if b.len() >= crate::editor::MAX_CURSORS => full = true,
                        Some(mut b) => {
                            b.push(Selection::cursor(pos));
                            *sels = b;
                        }
                        None => sels.replace(vec![Selection::cursor(pos)], 0),
                    }
                }
            }
            // Strg+U nimmt Ziehen wie Klick zurück auf den Stand beim Drücken.
            if let Some(mut b) = before {
                doc.sync(&mut b);
                view.note_selection_change(b);
            }
            return (false, full);
        }
        let moved = moved || (p - press).length() > 4.0;
        let mut full = false;
        if moved {
            let (_, l1, v1) = pos_at(doc, view, g, rows, p);
            let r1 = rows.iter().rev().find(|r| p.y >= r.y && r.line == l1).copied().or_else(|| rows.iter().find(|r| r.line == l1).copied());
            let (row1, rv1) = r1.map(|r| (rows_of(doc, r.line, g.wrap_cols, g.tab).iter().position(|s| *s == r.start).unwrap_or(0), r.vstart)).unwrap_or((0, 0));
            if let Some(sels) = view.sels.as_mut() {
                full = column_rows(doc, sels, (line, row, v), (l1, row1, v1.saturating_sub(rv1)), g, base.as_ref());
            }
            // Ziehen über den Rand: rollen
            if p.y < g.text.top() {
                view.top_off -= ((g.text.top() - p.y) * 0.3).clamp(1.0, g.row_h * 3.0);
                ui.ctx().request_repaint();
            } else if p.y > g.text.bottom() {
                view.top_off += ((p.y - g.text.bottom()) * 0.3).clamp(1.0, g.row_h * 3.0);
                ui.ctx().request_repaint();
            }
        }
        view.drag = Some(DragMode::Middle { line, row, v, press, base, moved, before });
        return (false, full);
    }
    if pressed && resp.hovered() && inside_text {
        let (pos, line, v) = pos_at(doc, view, g, rows, p);
        let Some(sels) = view.sels.as_mut() else { return (false, false) };
        // Klickzähler (Doppel-/Dreifachklick)
        let count = match view.last_press {
            Some((t, lp, c)) if time - t < 0.45 && (lp - p).length() < 6.0 => (c % 3) + 1,
            _ => 1,
        };
        view.last_press = Some((time, p, count));
        doc.break_undo_group();
        let in_gutter = p.x < g.text.left() - 2.0;
        if in_gutter {
            // Klick auf die Zeilennummer: ganze Zeile
            let s = doc.line_to_char(line);
            let e = if line + 1 < doc.len_lines() { doc.line_to_char(line + 1) } else { doc.len_chars() };
            sels.replace(vec![Selection::new(s, e)], 0);
            view.drag = Some(DragMode::Lines(s, e));
        } else if mods.alt && !mods.shift && multi.alt_click {
            // Alt+Klick (#394): auf einem Cursor nimmt es ihn weg (der letzte bleibt), sonst ein weiterer.
            let hit = sels.iter().position(|s| s.is_empty() && s.head == pos);
            if let Some(i) = hit {
                if sels.len() > 1 {
                    let mut v: Vec<Selection> = sels.as_slice().to_vec();
                    v.remove(i);
                    let pi = sels.primary_index();
                    let primary = if pi == i { v.len() - 1 } else if pi > i { pi - 1 } else { pi };
                    sels.replace(v, primary);
                }
                view.drag = None;
            } else if sels.len() >= crate::editor::MAX_CURSORS {
                view.blink_epoch = time;
                return (false, true);
            } else {
                sels.push(Selection::cursor(pos));
                view.drag = Some(DragMode::AddCursor(pos));
            }
            let _ = (line, v);
        } else if mods.shift {
            let mut prim = sels.primary();
            prim.head = pos;
            prim.goal = None;
            sels.replace(vec![prim], 0);
            view.drag = Some(DragMode::Chars(prim.anchor));
        } else {
            match count {
                2 => {
                    let w = et::word_range_at(doc.rope(), pos);
                    sels.replace(vec![Selection::new(w.start, w.end)], 0);
                    view.drag = Some(DragMode::Words(w.start, w.end));
                }
                3 => {
                    let s = doc.line_to_char(line);
                    let e = if line + 1 < doc.len_lines() { doc.line_to_char(line + 1) } else { doc.len_chars() };
                    sels.replace(vec![Selection::new(s, e)], 0);
                    view.drag = Some(DragMode::Lines(s, e));
                }
                _ => {
                    sels.replace(vec![Selection::cursor(pos)], 0);
                    view.drag = Some(DragMode::Chars(pos));
                }
            }
        }
        view.blink_epoch = time;
        return (false, false);
    }
    if !down {
        view.drag = None;
        return (false, false);
    }
    let Some(mode) = view.drag.clone() else { return (false, false) };
    // Ziehen über den Rand: rollen
    if p.y < g.text.top() {
        view.top_off -= ((g.text.top() - p.y) * 0.3).clamp(1.0, g.row_h * 3.0);
        ui.ctx().request_repaint();
    } else if p.y > g.text.bottom() {
        view.top_off += ((p.y - g.text.bottom()) * 0.3).clamp(1.0, g.row_h * 3.0);
        ui.ctx().request_repaint();
    }
    if g.wrap_cols.is_none() {
        if p.x > g.text.right() {
            view.scroll_x += ((p.x - g.text.right()) * 0.3).clamp(1.0, g.cw * 4.0);
            ui.ctx().request_repaint();
        } else if p.x < g.text.left() && view.scroll_x > 0.0 && !matches!(mode, DragMode::Lines(..)) {
            view.scroll_x = (view.scroll_x - ((g.text.left() - p.x) * 0.3).clamp(1.0, g.cw * 4.0)).max(0.0);
            ui.ctx().request_repaint();
        }
    }
    let (pos, line, _v) = pos_at(doc, view, g, rows, p);
    let Some(sels) = view.sels.as_mut() else { return (false, false) };
    match mode {
        DragMode::Chars(a) => {
            let i = sels.primary_index();
            let mut v2: Vec<Selection> = sels.as_slice().to_vec();
            if let Some(s) = v2.get_mut(i) {
                *s = Selection::new(a, pos);
            }
            sels.replace(v2, i);
        }
        DragMode::AddCursor(a) => {
            let i = sels.primary_index();
            let mut v2: Vec<Selection> = sels.as_slice().to_vec();
            if let Some(s) = v2.get_mut(i) {
                *s = Selection::new(a, pos);
            }
            sels.replace(v2, i);
        }
        DragMode::Words(s0, e0) => {
            let w = et::word_range_at(doc.rope(), pos);
            let sel = if pos < s0 { Selection::new(e0, w.start.min(pos)) } else { Selection::new(s0, w.end.max(pos)) };
            sels.replace(vec![sel], 0);
        }
        DragMode::Lines(s0, e0) => {
            let s = doc.line_to_char(line);
            let e = if line + 1 < doc.len_lines() { doc.line_to_char(line + 1) } else { doc.len_chars() };
            let sel = if s < s0 { Selection::new(e0, s) } else { Selection::new(s0, e) };
            sels.replace(vec![sel], 0);
        }
        DragMode::Middle { .. } => {}
    }
    let _ = (cx, line);
    (false, false)
}

/// Spaltenauswahl über **Bildschirmzeilen** (#394; mit Umbruch zählen die Bildschirmzeilen) von (Zeile, Bildschirmzeile
/// darin, visuelle Spalte in ihr) `a` bis `b`: eine Auswahl je Bildschirmzeile, zu kurze und leere Zeilen bekommen einen
/// Cursor an ihrem Ende. `base` = Auswahlen davor (mit Strg: hinzufügen). Höchstens [`crate::editor::MAX_CURSORS`];
/// liefert, ob die Grenze erreicht wurde.
pub(super) fn column_rows(doc: &Document, sels: &mut Selections, a: (usize, usize, usize), b: (usize, usize, usize), g: &Geo, base: Option<&Selections>) -> bool {
    let last = doc.len_lines().saturating_sub(1);
    let (a, b) = ((a.0.min(last), a.1, a.2), (b.0.min(last), b.1, b.2));
    let forward = (b.0, b.1) >= (a.0, a.1);
    let (lo, hi) = if forward { (a, b) } else { (b, a) };
    let max = crate::editor::MAX_CURSORS.saturating_sub(base.map_or(0, |s| s.len()));
    let mut list: Vec<Selection> = Vec::new();
    let mut full = false;
    'outer: for line in lo.0..=hi.0 {
        let starts = rows_of(doc, line, g.wrap_cols, g.tab);
        let len = doc.line_len(line);
        let r0 = if line == lo.0 { lo.1.min(starts.len() - 1) } else { 0 };
        let r1 = if line == hi.0 { hi.1.min(starts.len() - 1) } else { starts.len() - 1 };
        for r in r0..=r1 {
            if list.len() >= max {
                full = true;
                break 'outer;
            }
            let rs = starts[r];
            let re = starts.get(r + 1).map(|n| n - 1).unwrap_or(len).max(rs);
            let rv = if rs == 0 { 0 } else { vcol(doc, line, rs, g.tab) };
            let base_pos = doc.line_to_char(line);
            let at = |v: usize| base_pos + col_at(doc, line, rv + v, g.tab).clamp(rs, re);
            list.push(Selection { anchor: at(a.2), head: at(b.2), goal: Some(b.2) });
        }
    }
    if list.is_empty() {
        return full;
    }
    let primary = if forward { list.len() - 1 } else { 0 };
    match base {
        Some(bs) => {
            let mut all: Vec<Selection> = bs.as_slice().to_vec();
            let p = all.len() + primary;
            all.extend(list);
            sels.replace(all, p);
        }
        None => sels.replace(list, primary),
    }
    full
}

/// Tastatur: Bewegen, Markieren, Bearbeiten, Zwischenablage, IME. Liefert, ob der Text geändert wurde.
fn keyboard(ui: &mut Ui, doc: &mut Document, view: &mut View, g: &Geo, cx: &TextCx) -> bool {
    let events = ui.input(|i| i.events.clone());
    let mut edited = false;
    let tab = g.tab;
    for e in events {
        let Some(sels) = view.sels.as_mut() else { break };
        let mut moved = true;
        match e {
            Event::Text(t) => {
                if !view.preedit.is_empty() {
                    continue;
                }
                edited |= super::doc::run(doc, sels, Op::Type(t), tab);
            }
            Event::Paste(t) => edited |= super::doc::run(doc, sels, Op::Paste(t), tab),
            Event::Copy => {
                copy(ui, doc, sels, false);
                moved = false;
            }
            Event::Cut => edited |= copy(ui, doc, sels, true),
            Event::Ime(egui::ImeEvent::Preedit(t)) => {
                view.preedit = t;
            }
            Event::Ime(egui::ImeEvent::Commit(t)) => {
                view.preedit.clear();
                if !t.is_empty() {
                    edited |= super::doc::run(doc, sels, Op::Type(t), tab);
                }
            }
            Event::Ime(egui::ImeEvent::Disabled) => view.preedit.clear(),
            Event::Key { key, pressed: true, modifiers: m, .. } => {
                let page = ((g.text.height() / g.row_h).floor() as usize).saturating_sub(1).max(1);
                match key_op(key, m, page) {
                    Some(KeyAct::Op(op)) => edited |= super::doc::run(doc, sels, op, tab),
                    Some(KeyAct::Move(mv, extend)) => move_cursors(doc, sels, mv, extend, g),
                    Some(KeyAct::ScrollLines(n)) => {
                        view.top_off += n as f32 * g.row_h;
                        moved = false;
                    }
                    Some(KeyAct::Escape) => {
                        if sels.is_multi() {
                            sels.collapse_to_primary();
                        } else {
                            let mut p = sels.primary();
                            p.anchor = p.head;
                            sels.set_primary(p);
                        }
                    }
                    None => moved = false,
                }
            }
            _ => moved = false,
        }
        if moved {
            view.reveal = true;
            view.blink_epoch = ui.input(|i| i.time);
        }
    }
    let _ = cx;
    edited
}

/// Bewegung des Cursors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up(usize),
    Down(usize),
    Home,
    End,
    DocStart,
    DocEnd,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum KeyAct {
    Op(Op),
    Move(Move, bool),
    ScrollLines(i32),
    Escape,
}

/// Taste → Aktion (ohne Kürzel, die der Bereich schon verbraucht hat). `page` = Zeilen je Bildschirmseite.
pub(super) fn key_op(key: Key, m: Modifiers, page: usize) -> Option<KeyAct> {
    let (ctrl, shift, alt) = (m.ctrl || m.command, m.shift, m.alt);
    if ctrl && alt {
        return None; // Strg+Alt+Pfeil: Fokus zwischen Teilungen; AltGr liefert Text
    }
    Some(match key {
        Key::ArrowLeft => KeyAct::Move(if ctrl { Move::WordLeft } else { Move::Left }, shift),
        Key::ArrowRight => KeyAct::Move(if ctrl { Move::WordRight } else { Move::Right }, shift),
        // Alt+Umschalt+↑/↓ hat seit #394 keine Belegung mehr (kein Cursor darüber/darunter).
        Key::ArrowUp | Key::ArrowDown if alt && shift => return None,
        Key::ArrowUp if alt => KeyAct::Op(Op::MoveLines(false)),
        Key::ArrowDown if alt => KeyAct::Op(Op::MoveLines(true)),
        Key::ArrowUp if ctrl => KeyAct::ScrollLines(-1),
        Key::ArrowDown if ctrl => KeyAct::ScrollLines(1),
        Key::ArrowUp => KeyAct::Move(Move::Up(1), shift),
        Key::ArrowDown => KeyAct::Move(Move::Down(1), shift),
        Key::PageUp => KeyAct::Move(Move::Up(page), shift),
        Key::PageDown => KeyAct::Move(Move::Down(page), shift),
        Key::Home if ctrl => KeyAct::Move(Move::DocStart, shift),
        Key::End if ctrl => KeyAct::Move(Move::DocEnd, shift),
        Key::Home => KeyAct::Move(Move::Home, shift),
        Key::End => KeyAct::Move(Move::End, shift),
        Key::Backspace => KeyAct::Op(Op::Backspace(ctrl)),
        Key::Delete if shift && !ctrl => KeyAct::Op(Op::Cut),
        Key::Delete => KeyAct::Op(Op::Delete(ctrl)),
        Key::Enter if !ctrl => KeyAct::Op(Op::Newline),
        Key::Tab if shift => KeyAct::Op(Op::Outdent),
        Key::Tab if !ctrl => KeyAct::Op(Op::Indent),
        Key::Escape => KeyAct::Escape,
        Key::A if ctrl => KeyAct::Op(Op::SelectAll),
        Key::Z if ctrl && shift => KeyAct::Op(Op::Redo),
        Key::Z if ctrl => KeyAct::Op(Op::Undo),
        Key::Y if ctrl => KeyAct::Op(Op::Redo),
        // Strg+D / Strg+Umschalt+D (mehrere Cursor) sind belegbare Kürzel des Editors (#394, `editor_keys`); Strg+Umschalt+L
        // hat keine Bedeutung mehr.
        Key::L if ctrl && !shift => KeyAct::Op(Op::SelectLine),
        Key::K if ctrl && shift => KeyAct::Op(Op::DeleteLine),
        _ => return None,
    })
}

/// Cursor bewegen (alle Auswahlen). Senkrechte Bewegung folgt den Bildschirmzeilen (Umbruch) und hält die Zielspalte.
fn move_cursors(doc: &Document, sels: &mut Selections, mv: Move, extend: bool, g: &Geo) {
    let rope = doc.rope();
    let len = doc.len_chars();
    sels.transform(|s| {
        // Ohne Umschalt fällt eine Auswahl bei Links/Rechts auf ihre Seite zusammen.
        if !extend && !s.is_empty() {
            match mv {
                Move::Left => return Selection::cursor(s.start()),
                Move::Right => return Selection::cursor(s.end()),
                _ => {}
            }
        }
        let mut goal = None;
        let head = match mv {
            Move::Left => et::prev_grapheme(rope, s.head),
            Move::Right => et::next_grapheme(rope, s.head),
            Move::WordLeft => word_left(doc, s.head),
            Move::WordRight => word_right(doc, s.head),
            Move::Home => {
                let lc = et::pos_to_linecol(rope, s.head);
                let starts = rows_of(doc, lc.line, g.wrap_cols, g.tab);
                let row = starts.iter().rposition(|x| *x <= lc.col).unwrap_or(0);
                let base = doc.line_to_char(lc.line);
                if row > 0 {
                    base + starts[row]
                } else {
                    // Smart Home: erst zum ersten Nicht-Leerzeichen, dann zum Zeilenanfang.
                    let ind = et::indent_len(rope, lc.line);
                    if lc.col == ind { base } else { base + ind }
                }
            }
            Move::End => {
                let lc = et::pos_to_linecol(rope, s.head);
                let starts = rows_of(doc, lc.line, g.wrap_cols, g.tab);
                let row = starts.iter().rposition(|x| *x <= lc.col).unwrap_or(0);
                let base = doc.line_to_char(lc.line);
                match starts.get(row + 1) {
                    Some(next) if base + next - 1 != s.head => base + next - 1,
                    _ => et::line_end(rope, lc.line),
                }
            }
            Move::DocStart => 0,
            Move::DocEnd => len,
            Move::Up(n) | Move::Down(n) => {
                let down = matches!(mv, Move::Down(_));
                let (pos, gv) = vertical(doc, s.head, s.goal, n, down, g);
                goal = Some(gv);
                pos
            }
        };
        if extend {
            Selection { anchor: s.anchor, head, goal }
        } else {
            Selection { anchor: head, head, goal }
        }
    });
}

/// Maße ohne Textfeld (Tests der Spaltenauswahl, #394).
#[cfg(test)]
pub(super) fn test_geo(wrap_cols: Option<usize>, tab: usize) -> Geo {
    Geo { text: Rect::NOTHING, gutter: Rect::NOTHING, scrollbar: Rect::NOTHING, row_h: 1.0, cw: 1.0, wrap_cols, tab }
}

/// Cursor ohne Textfeld bewegen (Vergleichs-Reiter, #391): senkrecht in Textzeilen (ohne Umbruch).
pub(super) fn move_in_doc(doc: &Document, sels: &mut Selections, mv: Move, extend: bool, tab: usize) {
    let g = Geo { text: Rect::NOTHING, gutter: Rect::NOTHING, scrollbar: Rect::NOTHING, row_h: 1.0, cw: 1.0, wrap_cols: None, tab: tab.max(1) };
    move_cursors(doc, sels, mv, extend, &g);
}

/// `n` Bildschirmzeilen hoch/runter ab `pos`, Zielspalte `goal` (visuell innerhalb der Bildschirmzeile).
fn vertical(doc: &Document, pos: usize, goal: Option<usize>, n: usize, down: bool, g: &Geo) -> (usize, usize) {
    let rope = doc.rope();
    let lc = et::pos_to_linecol(rope, pos);
    let mut line = lc.line;
    let mut starts = rows_of(doc, line, g.wrap_cols, g.tab);
    let mut row = starts.iter().rposition(|x| *x <= lc.col).unwrap_or(0);
    let row_v = if starts[row] == 0 { 0 } else { vcol(doc, line, starts[row], g.tab) };
    let gv = goal.unwrap_or_else(|| vcol(doc, line, lc.col, g.tab) - row_v);
    let last = doc.len_lines().saturating_sub(1);
    for _ in 0..n {
        if down {
            if row + 1 < starts.len() {
                row += 1;
            } else if line < last {
                line += 1;
                starts = rows_of(doc, line, g.wrap_cols, g.tab);
                row = 0;
            } else {
                return (doc.len_chars(), gv);
            }
        } else if row > 0 {
            row -= 1;
        } else if line > 0 {
            line -= 1;
            starts = rows_of(doc, line, g.wrap_cols, g.tab);
            row = starts.len() - 1;
        } else {
            return (0, gv);
        }
    }
    let rs = starts[row];
    let re = starts.get(row + 1).map(|x| x - 1).unwrap_or(doc.line_len(line));
    let rv = if rs == 0 { 0 } else { vcol(doc, line, rs, g.tab) };
    let col = col_at(doc, line, rv + gv, g.tab).clamp(rs, re.max(rs));
    (doc.line_to_char(line) + col, gv)
}

fn word_left(doc: &Document, pos: usize) -> usize {
    let rope = doc.rope();
    let mut p = pos;
    // Leerraum überspringen (nicht über einen Zeilenumbruch hinaus, außer direkt am Zeilenanfang)
    while p > 0 && matches!(rope.char(p - 1), ' ' | '\t') {
        p -= 1;
    }
    if p > 0 && rope.char(p - 1) == '\n' {
        return if p == pos { p - 1 } else { p };
    }
    if p == 0 {
        return 0;
    }
    let class = et::char_class(rope.char(p - 1));
    while p > 0 && et::char_class(rope.char(p - 1)) == class && rope.char(p - 1) != '\n' {
        p -= 1;
    }
    p
}

fn word_right(doc: &Document, pos: usize) -> usize {
    let rope = doc.rope();
    let len = rope.len_chars();
    let mut p = pos;
    if p < len && rope.char(p) == '\n' {
        return p + 1;
    }
    if p < len && !matches!(rope.char(p), ' ' | '\t') {
        let class = et::char_class(rope.char(p));
        while p < len && et::char_class(rope.char(p)) == class && rope.char(p) != '\n' {
            p += 1;
        }
    }
    while p < len && matches!(rope.char(p), ' ' | '\t') {
        p += 1;
    }
    p
}

/// Auswahl kopieren bzw. ausschneiden (ohne Auswahl: die ganzen Zeilen, wie in vielen Editoren). Liefert, ob der Text
/// geändert wurde.
fn copy(ui: &Ui, doc: &mut Document, sels: &mut Selections, cut: bool) -> bool {
    let (text, edited) = super::doc::clip(doc, sels, cut);
    if !text.is_empty() {
        // Unter Windows mit CRLF, damit andere Programme die Zeilen erkennen.
        ui.ctx().copy_text(if cfg!(windows) { text.replace('\n', "\r\n") } else { text });
    }
    edited
}

/// Streifen der Änderungsmarken (#385) zwischen Zeilennummer und Text: Balken für hinzugefügte/geänderte Zeilen,
/// Dreieck an der Oberkante für entfernte Zeilen.
fn paint_marks(p: &egui::Painter, g: &Geo, rows: &[Row], m: &crate::editor::linemarks::LineMarks, u: &crate::theme::tokens::UiColors, n_lines: usize) {
    let x = g.gutter.right() + 1.0;
    for r in rows {
        let Some(k) = m.at(r.line) else { continue };
        let hk = &m.hunks[k];
        let col = super::marks::color(hk, u);
        if hk.lines.is_empty() {
            if r.first {
                let y = r.y;
                p.add(egui::Shape::convex_polygon(vec![Pos2::new(x - 1.0, y - 4.0), Pos2::new(x + 4.0, y), Pos2::new(x - 1.0, y + 4.0)], col, Stroke::NONE));
            }
        } else {
            p.rect_filled(Rect::from_min_max(Pos2::new(x, r.y), Pos2::new(x + 3.0, r.y + g.row_h)), 0.0, col);
        }
    }
    // Entfernt hinter der letzten Zeile: Dreieck an deren Unterkante.
    if let (Some(last), Some(hk)) = (rows.last(), m.hunks.last())
        && hk.lines.is_empty()
        && hk.lines.start >= n_lines.saturating_sub(1)
        && last.line + 1 >= n_lines
        && m.at(last.line).is_none()
    {
        let y = last.y + g.row_h;
        p.add(egui::Shape::convex_polygon(vec![Pos2::new(x - 1.0, y - 4.0), Pos2::new(x + 4.0, y), Pos2::new(x - 1.0, y + 4.0)], super::marks::color(hk, u), Stroke::NONE));
    }
}

/// Klick auf den Streifen der Änderungsmarken: Zeile der Marke (das Fenster öffnet der Aufrufer).
fn strip_click(ui: &Ui, resp: &egui::Response, g: &Geo, rows: &[Row], cx: &TextCx) -> Option<usize> {
    let m = cx.marks?;
    let (pressed, pos) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.interact_pos()));
    let p = pos?;
    if !pressed || !resp.hovered() || p.x < g.gutter.right() - 3.0 || p.x > g.text.left() - 1.0 {
        return None;
    }
    let r = rows.iter().rev().find(|r| p.y >= r.y && p.y < r.y + g.row_h)?;
    // Löschmarke an der Oberkante gehört zur Zeile darunter, auch wenn knapp darüber geklickt wird.
    m.at(r.line).map(|_| r.line).or_else(|| (p.y - r.y > g.row_h * 0.7).then(|| r.line + 1).filter(|l| m.at(*l).is_some_and(|k| m.hunks[k].lines.is_empty())))
}

/// Alles zeichnen: Zeilennummern, aktuelle Zeile, Auswahlen, Treffer, Agenten-Markierungen, Text, Cursor.
#[allow(clippy::too_many_arguments)]
fn paint(ui: &Ui, rect: Rect, doc: &Document, view: &View, g: &Geo, rows: &[Row], cx: &TextCx, focused: bool, hl: Option<&DocHighlight>) {
    let u = style::ui();
    // Nicht unter die waagerechte Leiste (#389) zeichnen: unten endet alles mit dem Textfeld.
    let painter = ui.painter_at(Rect::from_min_max(rect.min, Pos2::new(rect.right(), g.text.bottom())));
    let fg = cx.theme.fg;
    let dim = style::dim(ui);
    let text_p = painter.with_clip_rect(g.text.expand2(Vec2::new(2.0, 0.0)).intersect(rect));
    let sels = view.sels.as_ref();
    let primary_line = sels.map(|s| doc.char_to_line(s.primary().head));
    let sel_lines: Vec<usize> = sels.map(|s| s.iter().map(|x| doc.char_to_line(x.head)).collect()).unwrap_or_default();
    let scroll = if g.wrap_cols.is_some() { 0.0 } else { view.scroll_x };
    // Gutter (mit eigenem Hintergrund des Themes deckt die Mitte schon in der Deckkraft des Editors, #387)
    if !style::backdrop_on() {
        painter.rect_filled(g.gutter, 0.0, cx.theme.bg);
    }
    if let Some(m) = cx.marks {
        paint_marks(&painter, g, rows, m, &u, doc.len_lines());
    }
    // Aktuelle Zeile(n)
    let only_cursors = sels.is_some_and(|s| s.iter().all(|x| x.is_empty()));
    for r in rows {
        if only_cursors && sel_lines.contains(&r.line) {
            let band = Rect::from_min_max(Pos2::new(g.gutter.left(), r.y), Pos2::new(g.text.right(), r.y + g.row_h));
            painter.rect_filled(band, 0.0, u.hover_bg.gamma_multiply(0.55));
        }
        if r.first {
            let cur = primary_line == Some(r.line);
            let num = (r.line + 1).to_string();
            painter.text(Pos2::new(g.gutter.right() - 6.0, r.y), Align2::RIGHT_TOP, num, cx.font.clone(), if cur { fg } else { dim.gamma_multiply(0.8) });
        }
    }
    // Treffer der Suche und Agenten-Markierungen im sichtbaren Bereich
    if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
        let vis = doc.line_to_char(first.line) + first.start..doc.line_to_char(last.line) + last.end + 1;
        if let Some(h) = cx.find {
            for m in h.matches_in(doc, vis.clone()) {
                paint_range(&text_p, doc, g, rows, m.clone(), scroll, u.warning.gamma_multiply(if h.is_current(&m) { 0.55 } else { 0.25 }));
            }
        }
        for m in doc.agent_marks() {
            if m.end < vis.start || m.start > vis.end {
                continue;
            }
            paint_range(&text_p, doc, g, rows, m.clone(), scroll, u.info.gamma_multiply(0.16));
        }
    }
    // Auswahlen
    if let Some(s) = sels {
        for sel in s.iter().filter(|x| !x.is_empty()) {
            paint_range(&text_p, doc, g, rows, sel.range(), scroll, u.selection_bg);
        }
    }
    // Text (im Syntax-Modus mit den Farben des Themes)
    let pal = hl.map(|_| code_palette(&u));
    for r in rows {
        let spans = hl.and_then(|h| h.spans(r.line));
        paint_row(&text_p, doc, g, r, scroll, fg, dim, &cx.font, spans.zip(pal.as_ref()));
    }
    // Cursor
    let blink_on = !focused || {
        let t = ui.input(|i| i.time) - view.blink_epoch;
        let on = t > 30.0 || ((t / 0.53) as u64).is_multiple_of(2);
        if t <= 30.0 {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(530));
        }
        on
    };
    if let Some(s) = sels {
        let col = if focused { u.focus } else { dim.gamma_multiply(0.6) };
        if blink_on {
            for sel in s.iter() {
                if let Some(p) = caret_pos(doc, g, rows, sel.head, scroll) {
                    text_p.rect_filled(Rect::from_min_size(p, Vec2::new(2.0, g.row_h)), 0.0, col);
                }
            }
        }
    }
    // IME-Zwischenstand am Hauptcursor
    if !view.preedit.is_empty()
        && let Some(p) = sels.and_then(|s| caret_pos(doc, g, rows, s.primary().head, scroll))
    {
        let gal = painter.layout_no_wrap(view.preedit.clone(), cx.font.clone(), fg);
        let r = Rect::from_min_size(p, Vec2::new(gal.size().x.max(g.cw), g.row_h));
        painter.rect_filled(r, 0.0, cx.theme.bg.to_opaque());
        painter.galley(p, gal, fg);
        painter.line_segment([Pos2::new(r.left(), r.bottom() - 1.0), Pos2::new(r.right(), r.bottom() - 1.0)], Stroke::new(1.0_f32, fg));
    }
}

/// Bildschirmposition (oben links) des Cursors vor dem Zeichen `pos`, wenn er sichtbar ist.
fn caret_pos(doc: &Document, g: &Geo, rows: &[Row], pos: usize, scroll: f32) -> Option<Pos2> {
    let lc = et::pos_to_linecol(doc.rope(), pos);
    let r = rows.iter().rfind(|r| r.line == lc.line && r.start <= lc.col)?;
    if lc.col > r.end {
        return None;
    }
    let v = vcol(doc, r.line, lc.col, g.tab);
    Some(Pos2::new(g.x_of(r, v, scroll) - 1.0, r.y))
}

/// Wellenlinie (Diagnose, #367) unter `range`; `dotted` = Hinweis bzw. unnötiger Code (gepunktet), `strike` = veraltet
/// (zusätzlich durchgestrichen).
#[allow(clippy::too_many_arguments)]
fn paint_squiggle(p: &egui::Painter, doc: &Document, g: &Geo, rows: &[Row], range: std::ops::Range<usize>, scroll: f32, col: Color32, dotted: bool, strike: bool) {
    for r in rows {
        let base = doc.line_to_char(r.line);
        let (rs, re) = (base + r.start, base + r.end);
        if range.end <= rs || range.start > re {
            continue;
        }
        let a = range.start.max(rs) - base;
        let b = range.end.min(re).max(range.start.max(rs)) - base;
        let x0 = g.x_of(r, vcol(doc, r.line, a, g.tab), scroll);
        let x1 = g.x_of(r, vcol(doc, r.line, b, g.tab), scroll).max(x0 + g.cw * 0.6);
        let y = r.y + g.row_h - 2.0;
        if dotted {
            let mut x = x0;
            while x < x1 {
                p.circle_filled(Pos2::new(x + 0.75, y), 0.9, col);
                x += 3.0;
            }
        } else {
            let mut pts = Vec::new();
            let mut x = x0;
            let mut up = false;
            while x <= x1 {
                pts.push(Pos2::new(x, if up { y - 1.5 } else { y + 0.5 }));
                x += 2.0;
                up = !up;
            }
            if pts.len() >= 2 {
                p.add(egui::Shape::line(pts, Stroke::new(1.0_f32, col)));
            }
        }
        if strike {
            let ym = r.y + g.row_h / 2.0;
            p.line_segment([Pos2::new(x0, ym), Pos2::new(x1, ym)], Stroke::new(1.0_f32, col.gamma_multiply(0.8)));
        }
    }
}

/// Fläche hinter dem Bereich `range` (Auswahl, Treffer, Markierung) in den sichtbaren Zeilen.
fn paint_range(p: &egui::Painter, doc: &Document, g: &Geo, rows: &[Row], range: std::ops::Range<usize>, scroll: f32, col: Color32) {
    for r in rows {
        let base = doc.line_to_char(r.line);
        let (rs, re) = (base + r.start, base + r.end);
        let last_row = r.end == doc.line_len(r.line);
        // Ende der Zeile: den Umbruch als halbe Zelle zeigen, wenn die Auswahl darüber hinausgeht.
        if range.end < rs || range.start > re || (range.start == re && !last_row) {
            continue;
        }
        let a = range.start.max(rs) - base;
        let b = range.end.min(re) - base;
        let x0 = g.x_of(r, vcol(doc, r.line, a, g.tab), scroll);
        let mut x1 = g.x_of(r, vcol(doc, r.line, b, g.tab), scroll);
        if range.end > re && last_row {
            x1 += g.cw * 0.6;
        }
        if x1 > x0 {
            p.rect_filled(Rect::from_min_max(Pos2::new(x0, r.y), Pos2::new(x1, r.y + g.row_h)), 0.0, col);
        }
    }
}

/// Eine Bildschirmzeile zeichnen: Läufe einfacher Zeichen gleicher Farbe am Stück, Tabs als Lücke, breite/fremde
/// Zeichen einzeln im Raster, Steuerzeichen als gedämpfter Punkt. Nur der sichtbare Teil (waagerechter Bildlauf).
/// `syntax`: Farbbereiche der Zeile (Zeichenspalten) und die Farben der Klassen.
#[allow(clippy::too_many_arguments)]
fn paint_row(p: &egui::Painter, doc: &Document, g: &Geo, r: &Row, scroll: f32, fg: Color32, dim: Color32, font: &FontId, syntax: Option<(&[Span], &[Color32; 17])>) {
    let line = doc.line(r.line);
    let long = doc.line_len(r.line) >= LONG_LINE;
    let first_v = if g.wrap_cols.is_some() { r.vstart } else { (scroll / g.cw).floor() as usize };
    let max_v = first_v + (g.text.width() / g.cw).ceil() as usize + 2;
    let mut v = r.vstart;
    let mut run = String::new();
    let mut run_v = 0usize;
    let mut run_col = fg;
    let flush = |run: &mut String, run_v: usize, col: Color32| {
        if !run.is_empty() {
            p.text(Pos2::new(g.x_of(r, run_v, scroll), r.y), Align2::LEFT_TOP, std::mem::take(run), font.clone(), col);
        }
    };
    // Farbe der Zeichenspalte `ci` (die Bereiche sind sortiert; `si` wandert mit).
    let mut si = 0usize;
    let mut color_at = |ci: usize| -> Color32 {
        let Some((spans, pal)) = syntax else { return fg };
        while si < spans.len() && (spans[si].end as usize) <= ci {
            si += 1;
        }
        match spans.get(si) {
            Some(s) if (s.start as usize) <= ci => pal[s.tok.index()],
            _ => fg,
        }
    };
    // Sehr lange Zeile: direkt an die sichtbare Spalte springen.
    let (skip, start_v) = if long && first_v > r.start { (first_v, first_v) } else { (r.start, v) };
    v = v.max(start_v);
    let end = r.end.min(line.len_chars());
    if skip >= end {
        return;
    }
    for (k, c) in line.slice(skip..end).chars().enumerate() {
        if v > max_v {
            break;
        }
        let w = if long { 1 } else { et::char_width(c, v, g.tab) };
        if v + w < first_v {
            v += w;
            continue;
        }
        match c {
            '\t' => {
                flush(&mut run, run_v, run_col);
            }
            '\n' | '\r' => {}
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                flush(&mut run, run_v, run_col);
                p.text(Pos2::new(g.x_of(r, v, scroll) + g.cw * 0.5, r.y), Align2::CENTER_TOP, "·", font.clone(), dim);
            }
            c if c.is_ascii() => {
                let col = color_at(skip + k);
                if col != run_col {
                    flush(&mut run, run_v, run_col);
                    run_col = col;
                }
                if run.is_empty() {
                    run_v = v;
                }
                run.push(c);
            }
            c => {
                flush(&mut run, run_v, run_col);
                let col = color_at(skip + k);
                let mut b = [0u8; 4];
                p.text(Pos2::new(g.x_of(r, v, scroll), r.y), Align2::LEFT_TOP, c.encode_utf8(&mut b).to_string(), font.clone(), col);
            }
        }
        v += w;
    }
    flush(&mut run, run_v, run_col);
}

/// Minimap: jede Zeile als dünne Striche der Nicht-Leerzeichen, der sichtbare Ausschnitt hinterlegt; Klicken/Ziehen rollt.
fn minimap_ui(ui: &mut Ui, m: Rect, doc: &Document, view: &mut View, g: &Geo, cx: &TextCx) {
    let u = style::ui();
    let painter = ui.painter_at(m);
    if !style::backdrop_on() {
        painter.rect_filled(m, 0.0, cx.theme.bg);
    }
    painter.line_segment([m.left_top(), m.left_bottom()], Stroke::new(1.0_f32, u.border.gamma_multiply(0.5)));
    let n = doc.len_lines();
    let fit = (m.height() / MINIMAP_ROW).floor() as usize;
    let vis_lines = ((g.text.height() / g.row_h).ceil() as usize).max(1);
    // Erste Zeile der Minimap: so, dass der sichtbare Ausschnitt mitwandert.
    let first = if n <= fit { 0 } else { ((view.top_line as f64 / (n - vis_lines.min(n)).max(1) as f64) * (n - fit) as f64) as usize };
    let first = first.min(n.saturating_sub(fit));
    let scale = (m.width() - 8.0) / MINIMAP_COLS as f32;
    let col = cx.theme.fg.gamma_multiply(0.45);
    for (i, line) in (first..n.min(first + fit)).enumerate() {
        let y = m.top() + i as f32 * MINIMAP_ROW;
        let mut v = 0usize;
        let mut run_start: Option<usize> = None;
        for c in et::line_slice(doc.rope(), line).chars().take(MINIMAP_COLS) {
            let w = et::char_width(c, v, g.tab);
            let blank = c == ' ' || c == '\t';
            match (blank, run_start) {
                (false, None) => run_start = Some(v),
                (true, Some(s)) => {
                    painter.rect_filled(Rect::from_min_max(Pos2::new(m.left() + 4.0 + s as f32 * scale, y), Pos2::new(m.left() + 4.0 + v as f32 * scale, y + MINIMAP_ROW - 0.6)), 0.0, col);
                    run_start = None;
                }
                _ => {}
            }
            v += w;
            if v >= MINIMAP_COLS {
                break;
            }
        }
        if let Some(s) = run_start {
            painter.rect_filled(Rect::from_min_max(Pos2::new(m.left() + 4.0 + s as f32 * scale, y), Pos2::new(m.left() + 4.0 + v.min(MINIMAP_COLS) as f32 * scale, y + MINIMAP_ROW - 0.6)), 0.0, col);
        }
    }
    // Änderungsmarken (#385) am linken Rand der Minimap
    if let Some(mk) = cx.marks {
        for hk in mk.hunks.iter().filter(|h| h.lines.end.max(h.lines.start + 1) > first && h.lines.start < first + fit) {
            let y0 = m.top() + hk.lines.start.saturating_sub(first) as f32 * MINIMAP_ROW;
            let hh = (hk.lines.len().max(1) as f32 * MINIMAP_ROW).max(2.0);
            painter.rect_filled(Rect::from_min_size(Pos2::new(m.left() + 1.0, y0), Vec2::new(2.5, hh)), 0.0, super::marks::color(hk, &u));
        }
    }
    // Sichtbarer Ausschnitt
    let vy = m.top() + (view.top_line.saturating_sub(first)) as f32 * MINIMAP_ROW;
    let vr = Rect::from_min_size(Pos2::new(m.left() + 1.0, vy), Vec2::new(m.width() - 1.0, vis_lines as f32 * MINIMAP_ROW));
    painter.rect_filled(vr, 0.0, u.hover_bg.gamma_multiply(0.6));
    // Bedienung
    let resp = ui.interact(m, cx.id.with("minimap"), Sense::click_and_drag());
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() || resp.clicked() {
            view.minimap_drag = Some(p.y - vy);
        }
        if resp.dragged() || resp.clicked() {
            let grab = view.minimap_drag.unwrap_or(vis_lines as f32 * MINIMAP_ROW / 2.0).clamp(0.0, vis_lines as f32 * MINIMAP_ROW);
            let line = first as f32 + ((p.y - grab - m.top()) / MINIMAP_ROW);
            view.top_line = (line.max(0.0) as usize).min(n.saturating_sub(1));
            view.top_off = 0.0;
        }
    }
    if !resp.dragged() {
        view.minimap_drag = None;
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
}

/// Senkrechte Bildlaufleiste (nach Zeilen); Ziehen des Griffs oder Klick daneben.
fn scrollbar_ui(ui: &mut Ui, g: &Geo, doc: &Document, view: &mut View, rows: &[Row], cx: &TextCx) {
    let u = style::ui();
    let n = doc.len_lines().max(1) as f32;
    let vis = rows.iter().map(|r| r.line).collect::<std::collections::BTreeSet<_>>().len().max(1) as f32;
    if vis >= n && view.top_line == 0 && cx.marks.is_none_or(|m| m.hunks.is_empty()) {
        return;
    }
    let track = g.scrollbar.shrink2(Vec2::new(2.0, 2.0));
    if let Some(m) = cx.marks {
        for hk in &m.hunks {
            let y = track.top() + hk.lines.start as f32 / n * track.height();
            let hh = (hk.lines.len() as f32 / n * track.height()).max(2.0);
            ui.painter().rect_filled(Rect::from_min_size(Pos2::new(g.scrollbar.left() + 1.0, y), Vec2::new(3.0, hh)), 0.0, super::marks::color(hk, &u));
        }
    }
    let h = (track.height() * (vis / n)).clamp(24.0, track.height());
    let frac = view.top_line as f32 / (n - 1.0).max(1.0);
    let y = track.top() + (track.height() - h) * frac.clamp(0.0, 1.0);
    let thumb = Rect::from_min_size(Pos2::new(track.left(), y), Vec2::new(track.width(), h));
    let resp = ui.interact(g.scrollbar, cx.id.with("vscroll"), Sense::click_and_drag());
    let hot = resp.hovered() || resp.dragged();
    ui.painter().rect_filled(thumb, 4.0, if hot { u.text_dim.gamma_multiply(0.7) } else { u.text_dim.gamma_multiply(0.35) });
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() {
            view.scroll_drag = Some(if thumb.contains(p) { p.y - thumb.top() } else { h / 2.0 });
        }
        if resp.dragged() || resp.clicked() {
            let grab = view.scroll_drag.unwrap_or(h / 2.0);
            let f = ((p.y - grab - track.top()) / (track.height() - h).max(1.0)).clamp(0.0, 1.0);
            view.top_line = (f * (n - 1.0)).round() as usize;
            view.top_off = 0.0;
        }
    }
    if !resp.dragged() {
        view.scroll_drag = None;
    }
}

/// IME (wie im Terminal): Eingabemethode erlauben und ihr Fenster an den Cursor setzen.
fn ime_output(ui: &Ui, rect: Rect, doc: &Document, view: &View, g: &Geo, rows: &[Row], _cx: &TextCx) {
    let scroll = if g.wrap_cols.is_some() { 0.0 } else { view.scroll_x };
    let p = view.sels.as_ref().and_then(|s| caret_pos(doc, g, rows, s.primary().head, scroll)).unwrap_or(g.text.min);
    let cursor_rect = Rect::from_min_size(p, Vec2::new(1.0, g.row_h));
    let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
    ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { rect: to_global * rect, cursor_rect: to_global * cursor_rect }));
}

// ---- waagerechte Bildlaufleiste (#389, Zeichnen und Messen in `hscroll`) ----

/// Ohne Umbruch: längste Zeile messen und, wenn sie nicht passt, die Leiste unter den Text legen (`text` wird kürzer).
#[allow(clippy::too_many_arguments)]
fn hbar_prepare(ui: &Ui, doc: &Document, width: &mut super::hscroll::DocWidth, view: &mut View, no_wrap: bool, tab: usize, cw: f32, text: &mut Rect, left: f32, right: f32, bottom: f32) -> Option<super::hscroll::Layout> {
    if !no_wrap {
        view.hscroll.last_bar = None;
        return None;
    }
    // Die Zeilen der Cursor zählen sofort (Tippen am Ende der längsten Zeile, während im Hintergrund gemessen wird).
    let cur = match &view.sels {
        Some(s) => view.hscroll.head_cols(doc, s, tab),
        None => 0,
    };
    Some(super::hscroll::prepare(width, &mut view.hscroll, ui.ctx(), doc, tab, cw, cur, text, left, right, bottom))
}

/// Waagerechten Bildlauf auf die längste Zeile begrenzen (ohne Umbruch). Die Zeilen der Cursor zählen nach Eingaben
/// in diesem Bild schon mit (Einfügen am Ende der längsten Zeile, auch mit mehreren Cursorn: sie bleiben sichtbar).
fn hbar_clamp(doc: &Document, view: &mut View, tab: usize, h: Option<&mut super::hscroll::Layout>) {
    let Some(h) = h else { return };
    if let Some(s) = &view.sels {
        h.include(view.hscroll.head_cols(doc, s, tab));
    }
    view.scroll_x = view.scroll_x.clamp(0.0, h.max);
}

/// Leiste zeichnen und bedienen; der Rand der Zeilennummern reicht weiter bis unten.
fn hbar_ui(ui: &mut Ui, h: &super::hscroll::Layout, g: &Geo, view: &mut View, cx: &TextCx, bottom: f32) {
    let Some(area) = h.area else { return };
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(g.gutter.left(), area.top()), Pos2::new(g.gutter.right(), bottom)), 0.0, cx.theme.bg);
    let View { scroll_x, hscroll, .. } = view;
    super::hscroll::bar(ui, area, cx.id.with("hscroll"), scroll_x, &mut hscroll.drag, h.content, g.text.width());
}
