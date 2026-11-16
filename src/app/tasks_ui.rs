//! Reiter „Todo-Liste“: einspaltige Liste von Karten mit Schnell-Eingabe, Tags, Filter, Drag & Drop, Rückgängig/Wiederholen.
//! Die Daten liegen in `crate::tasks` (SQLite), die Historie in `crate::undo`; hier steht nur die Oberfläche.

use super::*;
use crate::state::UNDO_STEP_CHOICES;
use crate::tasks::{self, Item, ItemFilter, Tag, TaskError, TaskOp};
use crate::undo::History;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, FontFamily, Galley, Layout, Painter, Response};
use std::sync::Arc;

/// Breite der Spalte (mehr wird nicht genutzt, die Liste bleibt zentriert).
const COLUMN_MAX: f32 = 720.0;
const CARD_PAD: f32 = 12.0;
const CHIP_H: f32 = 18.0;
/// Wartezeit der Sicherheitsabfrage vor dem endgültigen Löschen.
const CONFIRM_SECS: f32 = 3.0;
const DANGER: Color32 = Color32::from_rgb(0xc2, 0x41, 0x41);

fn quick_id() -> Id {
    Id::new("todo_quick_input")
}
fn title_id() -> Id {
    Id::new("todo_form_title")
}

/// Schreibfarbe eines Tags.
fn tag_color(t: &Tag) -> Color32 {
    let (r, g, b) = tasks::rgb(&t.color);
    Color32::from_rgb(r, g, b)
}

fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

// ------------------------------------------------------------------ Zustand

#[derive(Default, Clone)]
struct CardForm {
    title: String,
    body: String,
    /// Gewählte Tag-Namen (neue entstehen erst beim Bestätigen).
    tags: Vec<String>,
    tag_input: String,
    tag_sel: Option<usize>,
    /// Bereich der Vorschläge im letzten Bild (damit ein Klick darauf nicht am verlorenen Fokus scheitert).
    sugg_rect: Option<Rect>,
}

impl CardForm {
    /// Noch nicht bestätigten Text im Tag-Feld als Tag übernehmen.
    fn flush_tag(&mut self, all: &[Tag]) {
        let t = std::mem::take(&mut self.tag_input);
        add_tag_name(self, all, &t);
    }

    fn clear(&mut self) {
        *self = CardForm::default();
    }
}

/// Tag zum Formular hinzufügen (vorhandene Tags in ihrer Schreibweise, Doppelte nicht).
fn add_tag_name(form: &mut CardForm, all: &[Tag], name: &str) {
    let name = tasks::clean_tag(name);
    if name.is_empty() {
        return;
    }
    let k = tasks::tag_key(&name);
    let shown = all.iter().find(|t| tasks::tag_key(&t.name) == k).map(|t| t.name.clone()).unwrap_or(name);
    if !form.tags.iter().any(|t| tasks::tag_key(t) == k) {
        form.tags.push(shown);
    }
    form.tag_input.clear();
    form.tag_sel = None;
}

struct EditDialog {
    id: i64,
    key: String,
    form: CardForm,
    err: Option<TaskError>,
    focus_title: bool,
}

enum ConfirmKind {
    Item(i64),
    AllDone(usize),
    /// (Tag-Id, Name, Zahl der Karten)
    Tag(i64, String, usize),
}

struct Confirm {
    kind: ConfirmKind,
    since: Instant,
}

struct MergeAsk {
    from: i64,
    into: i64,
    from_name: String,
    into_name: String,
    cards: usize,
    /// Kam von einem Umbenennen (Name schon vergeben)?
    from_rename: bool,
}

#[derive(Default)]
struct TagMgr {
    /// Eingabepuffer je Tag (nur solange das Feld bearbeitet wird).
    names: HashMap<i64, String>,
    /// Tag, dessen Farbauswahl offen ist.
    picker: Option<i64>,
}

struct Drag {
    id: i64,
    /// Abstand des Zeigers zur linken oberen Ecke der Karte beim Greifen.
    grab: Vec2,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Quick,
    Title,
}

enum CardAct {
    Toggle(i64, bool),
    Delete(i64),
    Edit(i64),
    FilterTag(i64),
}

pub(super) struct TasksUi {
    loaded: bool,
    dirty: bool,
    items: Vec<Item>,
    tags: Vec<Tag>,
    tag_use: HashMap<i64, usize>,
    total: usize,
    done_total: usize,
    show_done: bool,
    filter: Vec<i64>,
    history: History<TaskOp>,
    quick: String,
    quick_focused: bool,
    form_open: bool,
    form: CardForm,
    focus: Option<Focus>,
    create_err: Option<TaskError>,
    scroll_top: bool,
    edit: Option<EditDialog>,
    confirm: Option<Confirm>,
    merge_ask: Option<MergeAsk>,
    tag_mgr: Option<TagMgr>,
    drag: Option<Drag>,
    /// Meldung für die Hinweisblase der Anwendung (z. B. Datenbankfehler).
    pub(super) notice: Option<String>,
}

impl Default for TasksUi {
    fn default() -> Self {
        Self {
            loaded: false,
            dirty: true,
            items: Vec::new(),
            tags: Vec::new(),
            tag_use: HashMap::new(),
            total: 0,
            done_total: 0,
            show_done: true,
            filter: Vec::new(),
            history: History::new(25),
            quick: String::new(),
            quick_focused: false,
            form_open: false,
            form: CardForm::default(),
            focus: None,
            create_err: None,
            scroll_top: false,
            edit: None,
            confirm: None,
            merge_ask: None,
            tag_mgr: None,
            drag: None,
            notice: None,
        }
    }
}

/// Was die Oberfläche von außen braucht.
pub(super) struct Env<'a> {
    pub db: Option<&'a crate::db::Db>,
    pub db_error: Option<&'a str>,
    pub lang: Lang,
    pub bg: Color32,
    pub undo_steps: usize,
}

struct Colors {
    fill: Color32,
    fill_hover: Color32,
    stroke: Color32,
    text: Color32,
    strong: Color32,
    dim: Color32,
}

impl Colors {
    fn of(ui: &Ui) -> Self {
        let v = ui.visuals();
        let fill = v.faint_bg_color;
        Colors {
            fill,
            fill_hover: style::mix(fill, v.strong_text_color(), 0.06),
            stroke: v.widgets.noninteractive.bg_stroke.color,
            text: v.text_color(),
            strong: v.strong_text_color(),
            dim: v.weak_text_color(),
        }
    }
}

// ------------------------------------------------------------------ Zeichnen: Kleinteile

/// Tag-Chip (Text, optional mit ×); Farbe `None` = neutral (neuer, noch nicht angelegter Tag).
fn chip(ui: &mut Ui, text: &str, color: Option<Color32>, removable: bool, active: bool) -> Response {
    let text_col = ui.visuals().text_color();
    let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.5), text_col);
    let w = g.size().x + 14.0 + if removable { 12.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, CHIP_H), Sense::CLICK);
    paint_chip(ui.painter(), rect, text, color, removable, active, resp.hovered(), ui.visuals().strong_text_color(), ui.visuals().weak_text_color(), 1.0);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

#[allow(clippy::too_many_arguments)]
fn paint_chip(p: &Painter, rect: Rect, text: &str, color: Option<Color32>, removable: bool, active: bool, hovered: bool, strong: Color32, dim: Color32, fade: f32) {
    let base = color.unwrap_or(dim);
    let fill = base.gamma_multiply((if hovered { 0.34 } else { 0.2 }) * fade);
    let stroke = if active { style::accent() } else { base.gamma_multiply(0.6 * fade) };
    p.rect(rect, 9.0, fill, Stroke::new(if active { 1.5_f32 } else { 1.0_f32 }, stroke), StrokeKind::Inside);
    let tc = style::mix(base, strong, 0.35).gamma_multiply(fade);
    let g = p.layout_no_wrap(text.to_string(), FontId::proportional(11.5), tc);
    let tx = rect.left() + 7.0;
    p.galley(Pos2::new(tx, rect.center().y - g.size().y / 2.0), g, tc);
    if removable {
        let c = Pos2::new(rect.right() - 11.0, rect.center().y);
        let s = Stroke::new(1.2_f32, tc);
        p.line_segment([c + Vec2::new(-2.5, -2.5), c + Vec2::new(2.5, 2.5)], s);
        p.line_segment([c + Vec2::new(-2.5, 2.5), c + Vec2::new(2.5, -2.5)], s);
    }
}

fn galley(p: &Painter, text: &str, font: FontId, color: Color32, strike: bool, wrap: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple(text.to_string(), font, color, wrap);
    if strike {
        job.sections[0].format = TextFormat { strikethrough: Stroke::new(1.2_f32, color), ..job.sections[0].format.clone() };
    }
    p.layout_job(job)
}

struct ChipBox {
    tag: Tag,
    rect: Rect,
}

/// Berechnete Anordnung einer Karte; alle Positionen relativ zur linken oberen Ecke.
#[derive(Clone)]
struct CardLayout {
    size: Vec2,
    key: Arc<Galley>,
    key_pos: Vec2,
    title: Option<(Arc<Galley>, Vec2)>,
    body: Option<(Arc<Galley>, Vec2)>,
    sep_y: Option<f32>,
    chips: Arc<Vec<ChipBox>>,
    check: Rect,
    trash: Rect,
}

fn layout_card(p: &Painter, it: &Item, all: &[Tag], w: f32, col: &Colors) -> CardLayout {
    let fade = if it.done { 0.5 } else { 1.0 };
    let inner = w - 2.0 * CARD_PAD;
    let mut y = 8.0;
    // Kopfzeile: Schlüssel links, Mülltonne rechts
    let key = galley(p, &it.key(), FontId::proportional(11.5), col.dim.gamma_multiply(fade), it.done, inner);
    let key_pos = Vec2::new(CARD_PAD, y + (22.0 - key.size().y) / 2.0);
    let trash = Rect::from_min_size(Pos2::new(w - 8.0 - 22.0, y), Vec2::splat(22.0));
    y += 24.0;
    let mut title = None;
    if !it.title.is_empty() {
        let g = galley(p, &it.title, bold_font(15.0), col.strong.gamma_multiply(fade), it.done, inner);
        let h = g.size().y;
        title = Some((g, Vec2::new(CARD_PAD, y)));
        y += h;
    }
    let mut sep_y = None;
    if !it.title.is_empty() && !it.body.is_empty() {
        y += 7.0;
        sep_y = Some(y);
        y += 8.0;
    }
    let mut body = None;
    if !it.body.is_empty() {
        let g = galley(p, &it.body, FontId::proportional(13.5), col.text.gamma_multiply(fade), it.done, inner);
        let h = g.size().y;
        body = Some((g, Vec2::new(CARD_PAD, y)));
        y += h;
    }
    y += 8.0;
    // Fußzeile: Tags links (umbrechend), Checkbox rechts unten
    let mut chips = Vec::new();
    let mut tags: Vec<&Tag> = it.tags.iter().filter_map(|id| all.iter().find(|t| t.id == *id)).collect();
    tags.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let avail = inner - 30.0;
    let (mut x, mut row_y) = (0.0_f32, 0.0_f32);
    for t in tags {
        let g = p.layout_no_wrap(t.name.clone(), FontId::proportional(11.5), col.text);
        let cw = g.size().x + 14.0;
        if x + cw > avail && x > 0.0 {
            x = 0.0;
            row_y += CHIP_H + 4.0;
        }
        chips.push(ChipBox { tag: t.clone(), rect: Rect::from_min_size(Pos2::new(CARD_PAD + x, y + row_y), Vec2::new(cw, CHIP_H)) });
        x += cw + 5.0;
    }
    let chips_h = if chips.is_empty() { 0.0 } else { row_y + CHIP_H };
    let row_h = chips_h.max(20.0);
    let check = Rect::from_min_size(Pos2::new(w - CARD_PAD - 20.0, y + row_h - 20.0), Vec2::splat(20.0));
    y += row_h + 10.0;
    CardLayout { size: Vec2::new(w, y), key, key_pos, title, body, sep_y, chips: Arc::new(chips), check, trash }
}

#[derive(Default, Clone, Copy)]
struct Hover {
    card: bool,
    trash: bool,
    check: bool,
    chip: Option<usize>,
}

fn paint_card(p: &Painter, rect: Rect, lay: &CardLayout, it: &Item, col: &Colors, hv: Hover, ghost: bool) {
    let fade = if it.done { 0.5 } else { 1.0 };
    let o = rect.min.to_vec2();
    let fill = if hv.card || ghost { col.fill_hover } else { col.fill };
    let stroke = if ghost { style::accent() } else if hv.card { style::mix(col.stroke, col.text, 0.3) } else { col.stroke };
    p.rect(rect, 10.0, fill, Stroke::new(if ghost { 1.5_f32 } else { 1.0_f32 }, stroke), StrokeKind::Inside);
    p.galley(Pos2::ZERO + lay.key_pos + o, lay.key.clone(), col.dim);
    if let Some((g, pos)) = &lay.title {
        p.galley(Pos2::ZERO + *pos + o, g.clone(), col.strong);
    }
    if let Some(y) = lay.sep_y {
        p.line_segment([Pos2::new(rect.left() + CARD_PAD, rect.top() + y), Pos2::new(rect.right() - CARD_PAD, rect.top() + y)], Stroke::new(1.0_f32, col.stroke.gamma_multiply(0.8 * fade)));
    }
    if let Some((g, pos)) = &lay.body {
        p.galley(Pos2::ZERO + *pos + o, g.clone(), col.text);
    }
    for (i, c) in lay.chips.iter().enumerate() {
        paint_chip(p, c.rect.translate(o), &c.tag.name, Some(tag_color(&c.tag)), false, false, hv.chip == Some(i), col.strong, col.dim, fade);
    }
    // Mülltonne
    let tr = lay.trash.translate(o);
    if hv.trash {
        p.rect_filled(tr, 5.0, Color32::from_rgba_unmultiplied(DANGER.r(), DANGER.g(), DANGER.b(), 70));
    }
    style::paint_icon(p, SIcon::Trash, tr.center(), if hv.trash { DANGER } else { col.dim.gamma_multiply(if it.done { 0.8 } else { 1.0 }) });
    // Checkbox
    let cb = lay.check.translate(o);
    if it.done {
        p.rect_filled(cb, 5.0, style::accent().gamma_multiply(0.75));
        let c = cb.center();
        let s = Stroke::new(1.8_f32, Color32::WHITE);
        p.line_segment([c + Vec2::new(-4.5, 0.5), c + Vec2::new(-1.5, 3.5)], s);
        p.line_segment([c + Vec2::new(-1.5, 3.5), c + Vec2::new(4.5, -3.5)], s);
    } else {
        p.rect_stroke(cb, 5.0, Stroke::new(1.5_f32, if hv.check { style::accent() } else { col.dim }), StrokeKind::Inside);
        if hv.check {
            p.rect_filled(cb.shrink(1.0), 4.0, style::accent().gamma_multiply(0.15));
        }
    }
}

// ------------------------------------------------------------------ Formular (Anlegen und Bearbeiten)

/// Felder Titel, Text und Tags. Gibt `true` zurück, wenn mit Strg+Enter bestätigt wurde.
fn card_form_ui(ui: &mut Ui, form: &mut CardForm, all: &[Tag], lang: Lang, salt: &str, focus_title: bool) -> bool {
    let dim = style::dim(ui);
    let tid = if salt == "new" { title_id() } else { Id::new((salt, "title")) };
    ui.label(RichText::new(lang.t("Titel (optional)", "Title (optional)")).small().color(dim));
    let r_title = ui.add(egui::TextEdit::singleline(&mut form.title).id(tid).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6)).hint_text(lang.t("Kurzer Titel …", "Short title …")));
    if focus_title {
        r_title.request_focus();
    }
    ui.add_space(S1);
    ui.label(RichText::new(lang.t("Text (optional)", "Text (optional)")).small().color(dim));
    let r_body = ui.add(egui::TextEdit::multiline(&mut form.body).id(Id::new((salt, "body"))).desired_width(f32::INFINITY).desired_rows(4).margin(Margin::symmetric(8, 6)).hint_text(lang.t("Beschreibung …", "Description …")));
    ui.add_space(S1);
    ui.label(RichText::new("Tags").small().color(dim));
    let tag_focus = tag_editor(ui, form, all, lang, Id::new((salt, "tag")));
    let any_focus = r_title.has_focus() || r_body.has_focus() || tag_focus;
    any_focus && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter))
}

/// Tag-Eingabe mit Chips der gewählten Tags und Autovervollständigung. Gibt zurück, ob das Eingabefeld den Fokus hat.
fn tag_editor(ui: &mut Ui, form: &mut CardForm, all: &[Tag], lang: Lang, id: Id) -> bool {
    let focused_before = ui.memory(|m| m.has_focus(id));
    let over_sugg = form.sugg_rect.is_some_and(|r| ui.rect_contains_pointer(r));
    let sugg: Vec<Tag> = tasks::suggest(all, &form.tag_input, &form.tags, 8).into_iter().cloned().collect();
    let (mut down, mut up, mut tab, mut enter, mut back) = (false, false, false, false, false);
    if focused_before {
        ui.input_mut(|i| {
            down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
            up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
            enter = i.consume_key(Modifiers::NONE, Key::Enter);
            back = i.key_pressed(Key::Backspace);
            if !form.tag_input.trim().is_empty() && !sugg.is_empty() {
                tab = i.consume_key(Modifiers::NONE, Key::Tab);
            }
        });
    }
    let n = sugg.len();
    if down && n > 0 {
        form.tag_sel = Some(form.tag_sel.map_or(0, |i| (i + 1).min(n - 1)));
    }
    if up {
        form.tag_sel = match form.tag_sel {
            Some(i) if i > 0 => Some(i - 1),
            _ => None,
        };
    }
    if tab {
        let pick = sugg.get(form.tag_sel.unwrap_or(0)).map(|t| t.name.clone());
        if let Some(name) = pick {
            add_tag_name(form, all, &name);
        }
    } else if enter {
        let pick = form.tag_sel.and_then(|i| sugg.get(i)).map(|t| t.name.clone());
        match pick {
            Some(name) => add_tag_name(form, all, &name),
            None => {
                let t = std::mem::take(&mut form.tag_input);
                add_tag_name(form, all, &t);
            }
        }
    } else if back && form.tag_input.is_empty() {
        form.tags.pop();
    }
    let mut remove = None;
    let mut has_focus = false;
    ui.horizontal_wrapped(|ui| {
        for (i, name) in form.tags.iter().enumerate() {
            let k = tasks::tag_key(name);
            let col = all.iter().find(|t| tasks::tag_key(&t.name) == k).map(tag_color);
            let tip = if col.is_some() { lang.t("Tag entfernen", "Remove tag") } else { lang.t("Neuer Tag – entfernen", "New tag – remove") };
            if chip(ui, name, col, true, false).on_hover_text(tip).clicked() {
                remove = Some(i);
            }
        }
        let r = ui.add(egui::TextEdit::singleline(&mut form.tag_input).id(id).desired_width(170.0).margin(Margin::symmetric(8, 4)).hint_text(lang.t("Tag hinzufügen (Enter) …", "Add a tag (Enter) …")));
        if r.changed() {
            form.tag_sel = None;
            // Komma schließt einen Tag ab
            if form.tag_input.contains(',') {
                let text = std::mem::take(&mut form.tag_input);
                let mut parts: Vec<&str> = text.split(',').collect();
                let rest = parts.pop().unwrap_or("").to_string();
                for p in parts {
                    add_tag_name(form, all, p);
                }
                form.tag_input = rest;
            }
        }
        has_focus = r.has_focus();
    });
    if let Some(i) = remove {
        form.tags.remove(i);
    }
    // Vorschläge
    form.sugg_rect = None;
    let typed = tasks::clean_tag(&form.tag_input);
    let is_new = !typed.is_empty() && !all.iter().any(|t| tasks::tag_key(&t.name) == tasks::tag_key(&typed));
    if (has_focus || focused_before || over_sugg) && (!sugg.is_empty() || is_new) {
        let mut pick = None;
        let r = ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(lang.t("Vorschläge:", "Suggestions:")).small().color(style::dim(ui)));
            for (i, t) in sugg.iter().enumerate() {
                if chip(ui, &t.name, Some(tag_color(t)), false, form.tag_sel == Some(i)).on_hover_text(lang.t("Tag übernehmen (Tab)", "Use this tag (Tab)")).clicked() {
                    pick = Some(t.name.clone());
                }
            }
            if is_new {
                ui.label(RichText::new(format!("{} {}", lang.t("Enter: neuen Tag anlegen:", "Enter: create new tag:"), lang.q(&typed))).small().color(style::dim(ui)));
            }
        });
        form.sugg_rect = Some(r.response.rect.expand(4.0));
        if let Some(name) = pick {
            add_tag_name(form, all, &name);
            ui.memory_mut(|m| m.request_focus(id));
        }
    }
    has_focus
}

// ------------------------------------------------------------------ Hauptansicht

impl TasksUi {
    fn filter(&self) -> ItemFilter {
        ItemFilter { show_done: self.show_done, tags: self.filter.clone() }
    }

    fn reload(&mut self, conn: &rusqlite::Connection) {
        self.dirty = false;
        let r = (|| -> Result<(), TaskError> {
            self.tags = tasks::list_tags(conn)?;
            self.tag_use = tasks::tag_counts(conn)?;
            let (t, d) = tasks::counts(conn)?;
            self.total = t;
            self.done_total = d;
            self.filter.retain(|f| self.tags.iter().any(|t| t.id == *f));
            self.items = tasks::list_items(conn, &self.filter())?;
            Ok(())
        })();
        if let Err(e) = r {
            self.notice = Some(e.message(state::lang()));
        }
    }

    /// Die Daten haben sich außerhalb der Oberfläche geändert (z. B. durch einen Agenten): beim nächsten Bild neu laden.
    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn fail(&mut self, lang: Lang, e: &TaskError) {
        self.notice = Some(e.message(lang));
        self.dirty = true;
    }

    fn commit(&mut self, op: Option<TaskOp>) {
        if let Some(op) = op {
            self.history.push(op);
        }
        self.dirty = true;
    }

    /// Ergebnis einer Änderung verarbeiten: Operation merken oder Fehler melden.
    fn finish(&mut self, lang: Lang, r: Result<Option<TaskOp>, TaskError>) -> bool {
        match r {
            Ok(op) => {
                self.commit(op);
                true
            }
            Err(e) => {
                self.fail(lang, &e);
                false
            }
        }
    }

    fn undo(&mut self, conn: &rusqlite::Connection, lang: Lang) {
        match self.history.undo(conn) {
            Ok(_) => {}
            Err(e) => self.notice = Some(format!("{}: {e}", lang.t("Rückgängig nicht möglich", "Cannot undo"))),
        }
        self.dirty = true;
    }

    fn redo(&mut self, conn: &rusqlite::Connection, lang: Lang) {
        match self.history.redo(conn) {
            Ok(_) => {}
            Err(e) => self.notice = Some(format!("{}: {e}", lang.t("Wiederholen nicht möglich", "Cannot redo"))),
        }
        self.dirty = true;
    }

    fn any_modal(&self) -> bool {
        self.edit.is_some() || self.confirm.is_some() || self.merge_ask.is_some() || self.tag_mgr.is_some()
    }

    /// Tastenkürzel des Reiters: Strg+N (neue Karte), Strg+Z / Strg+Y / Strg+Umschalt+Z.
    fn global_keys(&mut self, ctx: &egui::Context, conn: &rusqlite::Connection, lang: Lang) {
        if self.any_modal() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::N)) {
            self.focus = Some(if self.form_open { Focus::Title } else { Focus::Quick });
        }
        // Solange ein Textfeld den Fokus hat, gehört Strg+Z dem Feld – außer das Schnellfeld ist leer.
        let text_owns = ctx.wants_keyboard_input() && !(self.quick_focused && self.quick.is_empty());
        if text_owns {
            return;
        }
        let (redo, undo) = ctx.input_mut(|i| {
            // Wiederholen zuerst: Strg+Umschalt+Z würde sonst als Strg+Z gelten (zusätzliches Umschalt wird ignoriert)
            let redo = i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::CTRL, Key::Y);
            let undo = !redo && i.consume_key(Modifiers::CTRL, Key::Z);
            (redo, undo)
        });
        if redo {
            self.redo(conn, lang);
        } else if undo {
            self.undo(conn, lang);
        }
    }

    pub(super) fn show(&mut self, ctx: &egui::Context, env: &Env) {
        let lang = env.lang;
        self.history.set_capacity(env.undo_steps.max(1));
        let Some(db) = env.db else {
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(env.bg)).show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    let msg = lang.t("Die Datenbank ist nicht verfügbar – die Todo-Liste kann nicht benutzt werden.", "The database is not available – the to-do list cannot be used.");
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(msg).color(style::dim(ui)));
                        if let Some(e) = env.db_error {
                            ui.colored_label(Color32::LIGHT_RED, e);
                        }
                    });
                });
            });
            return;
        };
        let conn = &db.conn;
        if !self.loaded {
            self.loaded = true;
            self.show_done = db.meta("todo_show_done").map(|v| v != "0").unwrap_or(true);
            self.dirty = true;
        }
        self.global_keys(ctx, conn, lang);
        if self.dirty {
            self.reload(conn);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(env.bg)).show(ctx, |ui| {
            self.body(ui, conn, db, lang);
        });
        self.dialogs(ctx, conn, lang);
        if self.dirty {
            self.reload(conn);
            ctx.request_repaint();
        }
    }

    fn body(&mut self, ui: &mut Ui, conn: &rusqlite::Connection, db: &crate::db::Db, lang: Lang) {
        let avail = ui.available_rect_before_wrap();
        let w = (avail.width() - 32.0).clamp(240.0, COLUMN_MAX);
        let x0 = avail.center().x - w / 2.0;
        ui.add_space(S3 + 2.0);
        let col_rect = Rect::from_min_size(Pos2::new(x0, ui.cursor().top()), Vec2::new(w, 400.0));
        ui.scope_builder(UiBuilder::new().max_rect(col_rect), |ui| {
            self.input_area(ui, conn, lang);
            ui.add_space(S2);
            self.filter_bar(ui, lang);
            self.toolbar(ui, conn, db, lang);
        });
        ui.add_space(S2);
        self.card_list(ui, conn, lang, w);
    }

    // ---- Eingabe

    fn input_area(&mut self, ui: &mut Ui, conn: &rusqlite::Connection, lang: Lang) {
        let focus = self.focus.take();
        if !self.form_open {
            ui.horizontal(|ui| {
                let btn_w = 92.0;
                let w = (ui.available_width() - btn_w - 90.0 - 2.0 * S2).max(80.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.quick)
                        .id(quick_id())
                        .desired_width(w)
                        .margin(Margin::symmetric(10, 7))
                        .hint_text(lang.t("Neues To-do – schreiben und Enter drücken …", "New to-do – type and press Enter …")),
                );
                if focus == Some(Focus::Quick) {
                    r.request_focus();
                }
                self.quick_focused = r.has_focus();
                if r.changed() {
                    self.create_err = None;
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let add = ui.add_sized([btn_w, 32.0], egui::Button::new(lang.t("Hinzufügen", "Add")).fill(style::accent().gamma_multiply(0.35))).clicked();
                if enter || add {
                    let text = std::mem::take(&mut self.quick);
                    match tasks::create_item(conn, "", &text, &[]) {
                        Ok((_, op)) => {
                            self.commit(Some(op));
                            self.create_err = None;
                            self.scroll_top = true;
                        }
                        Err(e) => {
                            self.quick = text;
                            self.create_err = Some(e);
                        }
                    }
                    ui.memory_mut(|m| m.request_focus(quick_id()));
                }
                if ui.add_sized([90.0, 32.0], egui::Button::new(lang.t("Mehr …", "More …"))).on_hover_text(lang.t("Titel, Text und Tags eingeben", "Enter title, text and tags")).clicked() {
                    self.form_open = true;
                    if self.form.body.is_empty() {
                        self.form.body = std::mem::take(&mut self.quick);
                    }
                    self.focus = Some(Focus::Title);
                    self.create_err = None;
                }
            });
        } else {
            let col = Colors::of(ui);
            if !self.form.title.trim().is_empty() || !self.form.body.trim().is_empty() {
                self.create_err = None;
            }
            egui::Frame::new().fill(col.fill).stroke(Stroke::new(1.0_f32, col.stroke)).corner_radius(10.0).inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let all = self.tags.clone();
                let submit = card_form_ui(ui, &mut self.form, &all, lang, "new", focus == Some(Focus::Title));
                ui.add_space(S2);
                let mut create = submit;
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(lang.t("Anlegen", "Create")).fill(style::accent().gamma_multiply(0.45)).min_size(Vec2::new(96.0, 28.0))).on_hover_text(lang.t("Strg+Enter", "Ctrl+Enter")).clicked() {
                        create = true;
                    }
                    if ui.add(egui::Button::new(lang.t("Weniger", "Less")).min_size(Vec2::new(80.0, 28.0))).on_hover_text(lang.t("Formular einklappen (Entwurf bleibt)", "Collapse the form (draft is kept)")).clicked() {
                        self.form_open = false;
                        self.focus = Some(Focus::Quick);
                    }
                    if let Some(e) = &self.create_err {
                        ui.colored_label(Color32::LIGHT_RED, e.message(lang));
                    }
                });
                if create {
                    self.form.flush_tag(&all);
                    let names = self.form.tags.clone();
                    match tasks::create_item(conn, &self.form.title, &self.form.body, &names) {
                        Ok((_, op)) => {
                            self.commit(Some(op));
                            self.form.clear();
                            self.create_err = None;
                            self.scroll_top = true;
                            self.focus = Some(Focus::Title);
                        }
                        Err(e) => self.create_err = Some(e),
                    }
                }
            });
            return;
        }
        if let Some(e) = &self.create_err {
            ui.add_space(2.0);
            ui.colored_label(Color32::LIGHT_RED, e.message(lang));
        }
    }

    fn filter_bar(&mut self, ui: &mut Ui, lang: Lang) {
        if self.filter.is_empty() {
            return;
        }
        let mut remove = None;
        let mut clear = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(lang.t("Filter:", "Filter:")).small().color(style::dim(ui)));
            for id in &self.filter {
                if let Some(t) = self.tags.iter().find(|t| t.id == *id) {
                    if chip(ui, &t.name, Some(tag_color(t)), true, true).on_hover_text(lang.t("Filter entfernen", "Remove filter")).clicked() {
                        remove = Some(*id);
                    }
                }
            }
            if self.filter.len() > 1 {
                ui.label(RichText::new(lang.t("(alle Tags müssen passen)", "(all tags must match)")).small().color(style::dim(ui)));
            }
            if ui.small_button(lang.t("Filter löschen", "Clear filters")).clicked() {
                clear = true;
            }
        });
        ui.add_space(S1);
        if clear {
            self.filter.clear();
            self.dirty = true;
        } else if let Some(id) = remove {
            self.filter.retain(|f| *f != id);
            self.dirty = true;
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, conn: &rusqlite::Connection, db: &crate::db::Db, lang: Lang) {
        let mut undo = false;
        let mut redo = false;
        let open = self.total - self.done_total;
        let count = if self.done_total > 0 {
            format!("{open} {} · {} {}", lang.t("offen", "open"), self.done_total, lang.t("erledigt", "done"))
        } else {
            format!("{open} {}", lang.t("offen", "open"))
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(count).small().color(style::dim(ui)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(lang.t("Tags …", "Tags …")).on_hover_text(lang.t("Tags verwalten", "Manage tags")).clicked() {
                    self.tag_mgr = Some(TagMgr::default());
                }
                if self.done_total > 0 {
                    if ui.small_button(format!("{} ({}) …", lang.t("Erledigte löschen", "Delete completed"), self.done_total)).clicked() {
                        self.confirm = Some(Confirm { kind: ConfirmKind::AllDone(self.done_total), since: Instant::now() });
                    }
                    let label = if self.show_done { lang.t("Erledigte ausblenden", "Hide completed") } else { lang.t("Erledigte anzeigen", "Show completed") };
                    if ui.small_button(label).clicked() {
                        self.show_done = !self.show_done;
                        db.set_meta("todo_show_done", if self.show_done { "1" } else { "0" });
                        self.dirty = true;
                    }
                }
                let _ = conn;
                let (cu, cr) = (self.history.can_undo(), self.history.can_redo());
                if ui.add_enabled(cr, egui::Button::new(RichText::new(lang.t("Wiederholen", "Redo")).small())).on_hover_text(format!("{} / {}", keys::pretty("Ctrl+Y", lang == Lang::De), keys::pretty("Ctrl+Shift+Z", lang == Lang::De))).clicked() {
                    redo = true;
                }
                if ui.add_enabled(cu, egui::Button::new(RichText::new(lang.t("Rückgängig", "Undo")).small())).on_hover_text(keys::pretty("Ctrl+Z", lang == Lang::De)).clicked() {
                    undo = true;
                }
            });
        });
        if undo {
            self.undo(conn, lang);
        }
        if redo {
            self.redo(conn, lang);
        }
    }

    // ---- Kartenliste

    fn card_list(&mut self, ui: &mut Ui, conn: &rusqlite::Connection, lang: Lang, w: f32) {
        let col = Colors::of(ui);
        let mut acts: Vec<CardAct> = Vec::new();
        let mut drop: Option<(i64, Option<i64>)> = None;
        let mut start_drag: Option<Drag> = None;
        let mut area = egui::ScrollArea::vertical().id_salt("todo_scroll").auto_shrink([false, false]);
        if std::mem::take(&mut self.scroll_top) {
            area = area.vertical_scroll_offset(0.0);
        }
        let ctx = ui.ctx().clone();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let modal = self.any_modal();
        area.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(2.0);
            if self.items.is_empty() {
                ui.add_space(40.0);
                let msg = if !self.filter.is_empty() {
                    lang.t("Keine Karte passt zu den Filtern.", "No cards match the filters.")
                } else if self.total > 0 && !self.show_done {
                    lang.t("Alle Karten sind erledigt und ausgeblendet.", "All cards are completed and hidden.")
                } else {
                    lang.t("Noch keine To-dos. Oben schreiben und Enter drücken.", "No to-dos yet. Type above and press Enter.")
                };
                ui.vertical_centered(|ui| ui.label(RichText::new(msg).color(col.dim)));
                return;
            }
            let cx = ui.available_rect_before_wrap().center().x;
            let viewport = ui.clip_rect();
            let mut rects: Vec<(i64, Rect)> = Vec::with_capacity(self.items.len());
            let mut ghost: Option<(CardLayout, Item)> = None;
            for it in &self.items {
                let lay = layout_card(ui.painter(), it, &self.tags, w, &col);
                let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), lay.size.y), Sense::hover());
                let rect = Rect::from_min_size(Pos2::new(cx - w / 2.0, row.top()), lay.size);
                rects.push((it.id, rect));
                let dragging_this = self.drag.as_ref().is_some_and(|d| d.id == it.id);
                if !ui.is_rect_visible(rect) && !dragging_this {
                    continue;
                }
                // Karte zuerst anmelden (liegt unten), danach die Bedienelemente (liegen oben und gewinnen)
                let base = Id::new(("todo_card", it.id));
                let card = ui.interact(rect, base, Sense::CLICK | Sense::DRAG);
                let o = rect.min.to_vec2();
                let r_trash = ui.interact(lay.trash.translate(o), base.with("trash"), Sense::CLICK);
                let r_check = ui.interact(lay.check.translate(o).expand(4.0), base.with("check"), Sense::CLICK);
                let mut hv = Hover { card: card.hovered() && !modal, trash: r_trash.hovered(), check: r_check.hovered(), chip: None };
                for (ci, c) in lay.chips.iter().enumerate() {
                    let r = ui.interact(c.rect.translate(o), base.with(("chip", ci)), Sense::CLICK).on_hover_text(format!("{} {}", lang.t("Nach diesem Tag filtern:", "Filter by this tag:"), c.tag.name));
                    if r.hovered() {
                        hv.chip = Some(ci);
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if r.clicked() {
                        acts.push(CardAct::FilterTag(c.tag.id));
                    }
                }
                r_trash.clone().on_hover_text(lang.t("Löschen …", "Delete …"));
                r_check.clone().on_hover_text(if it.done { lang.t("Wieder öffnen", "Reopen") } else { lang.t("Als erledigt markieren", "Mark as done") });
                if hv.trash || hv.check {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                } else if card.hovered() && hv.chip.is_none() && self.drag.is_none() && !modal {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if dragging_this {
                    // Platzhalter statt der Karte; die Karte selbst hängt am Zeiger
                    ui.painter().rect(rect, 10.0, col.fill.gamma_multiply(0.5), Stroke::new(1.0_f32, style::accent().gamma_multiply(0.5)), StrokeKind::Inside);
                    ghost = Some((lay.clone(), it.clone()));
                } else {
                    paint_card(ui.painter(), rect, &lay, it, &col, hv, false);
                }
                if r_trash.clicked() {
                    acts.push(CardAct::Delete(it.id));
                } else if r_check.clicked() {
                    acts.push(CardAct::Toggle(it.id, !it.done));
                } else if card.clicked() && hv.chip.is_none() {
                    acts.push(CardAct::Edit(it.id));
                }
                if card.drag_started() && !r_trash.dragged() && !r_check.dragged() {
                    let grab = pointer.map_or(Vec2::ZERO, |p| p - rect.min);
                    start_drag = Some(Drag { id: it.id, grab });
                }
            }
            ui.add_space(24.0);
            // Ziehen: Einfügemarke, Randscrollen, Loslassen
            let drag_id = start_drag.as_ref().map(|d| d.id).or(self.drag.as_ref().map(|d| d.id));
            if let (Some(id), Some(p)) = (drag_id, pointer) {
                let others: Vec<&(i64, Rect)> = rects.iter().filter(|(i, _)| *i != id).collect();
                let slot = others.iter().find(|(_, r)| p.y < r.center().y);
                let (before, line_y) = match slot {
                    Some((i, r)) => (Some(*i), r.top() - 4.0),
                    None => (None, others.last().map_or(viewport.top() + 4.0, |(_, r)| r.bottom() + 4.0)),
                };
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                ui.painter().line_segment([Pos2::new(cx - w / 2.0, line_y), Pos2::new(cx + w / 2.0, line_y)], Stroke::new(2.5_f32, style::accent()));
                if let Some((lay, it)) = &ghost {
                    let grab = self.drag.as_ref().map(|d| d.grab).or(start_drag.as_ref().map(|d| d.grab)).unwrap_or(Vec2::ZERO);
                    let rect = Rect::from_min_size(Pos2::new(cx - w / 2.0, p.y - grab.y), lay.size);
                    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("todo_ghost")));
                    painter.rect_filled(rect.translate(Vec2::new(0.0, 4.0)).expand(1.0), 10.0, Color32::from_black_alpha(60));
                    paint_card(&painter, rect, lay, it, &col, Hover::default(), true);
                }
                let edge = 36.0;
                if p.y < viewport.top() + edge {
                    ui.scroll_with_delta(Vec2::new(0.0, 12.0));
                    ctx.request_repaint();
                } else if p.y > viewport.bottom() - edge {
                    ui.scroll_with_delta(Vec2::new(0.0, -12.0));
                    ctx.request_repaint();
                }
                if self.drag.is_some() && !ctx.input(|i| i.pointer.any_down()) {
                    drop = Some((id, before));
                }
            }
        });
        if let Some(d) = start_drag {
            self.drag = Some(d);
        }
        if self.drag.is_some() {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.drag = None;
            }
            ctx.request_repaint();
        }
        if let Some((id, before)) = drop {
            self.drag = None;
            let r = tasks::move_before(conn, id, before);
            self.finish(lang, r);
        }
        for a in acts {
            match a {
                CardAct::Toggle(id, done) => {
                    let r = tasks::set_done(conn, id, done);
                    self.finish(lang, r);
                }
                CardAct::Delete(id) => self.confirm = Some(Confirm { kind: ConfirmKind::Item(id), since: Instant::now() }),
                CardAct::Edit(id) => {
                    if let Some(it) = self.items.iter().find(|i| i.id == id) {
                        let tags = it.tags.iter().filter_map(|t| self.tags.iter().find(|x| x.id == *t)).map(|t| t.name.clone()).collect::<Vec<_>>();
                        let mut tags = tags;
                        tags.sort_by_key(|a| a.to_lowercase());
                        self.edit = Some(EditDialog { id, key: it.key(), form: CardForm { title: it.title.clone(), body: it.body.clone(), tags, ..CardForm::default() }, err: None, focus_title: true });
                    }
                }
                CardAct::FilterTag(id) => {
                    if !self.filter.contains(&id) {
                        self.filter.push(id);
                        self.dirty = true;
                    }
                }
            }
        }
    }

    // ---- Dialoge

    fn dialogs(&mut self, ctx: &egui::Context, conn: &rusqlite::Connection, lang: Lang) {
        self.edit_dialog(ctx, conn, lang);
        self.tag_manager(ctx, conn, lang);
        self.merge_dialog(ctx, conn, lang);
        self.confirm_dialog(ctx, conn, lang);
    }

    fn modal_width(ctx: &egui::Context, want: f32) -> f32 {
        want.min(ctx.content_rect().width() - 80.0).max(220.0)
    }

    fn edit_dialog(&mut self, ctx: &egui::Context, conn: &rusqlite::Connection, lang: Lang) {
        let Some(mut ed) = self.edit.take() else { return };
        let all = self.tags.clone();
        let mut save = false;
        let mut close = false;
        let m = egui::Modal::new(Id::new("todo_edit_modal")).show(ctx, |ui| {
            ui.set_width(Self::modal_width(ctx, 480.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(lang.t("Karte bearbeiten", "Edit card")).heading());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(&ed.key).small().color(style::dim(ui)));
                });
            });
            ui.add_space(S2);
            let focus = std::mem::take(&mut ed.focus_title);
            if card_form_ui(ui, &mut ed.form, &all, lang, "edit", focus) {
                save = true;
            }
            ui.add_space(S2);
            if let Some(e) = &ed.err {
                ui.colored_label(Color32::LIGHT_RED, e.message(lang));
                ui.add_space(S1);
            }
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(lang.t("Speichern", "Save")).fill(style::accent().gamma_multiply(0.45)).min_size(Vec2::new(96.0, 28.0))).on_hover_text(lang.t("Strg+Enter", "Ctrl+Enter")).clicked() {
                    save = true;
                }
                if ui.add(egui::Button::new(lang.t("Abbrechen", "Cancel")).min_size(Vec2::new(90.0, 28.0))).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if save {
            ed.form.flush_tag(&all);
            match tasks::edit_item(conn, ed.id, &ed.form.title, &ed.form.body, &ed.form.tags) {
                Ok(op) => {
                    self.commit(op);
                    close = true;
                }
                Err(TaskError::Empty) => ed.err = Some(TaskError::Empty),
                Err(e) => {
                    self.fail(lang, &e);
                    close = true;
                }
            }
        }
        if !close {
            self.edit = Some(ed);
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context, conn: &rusqlite::Connection, lang: Lang) {
        let Some(cf) = self.confirm.take() else { return };
        let left = CONFIRM_SECS - cf.since.elapsed().as_secs_f32();
        let secs = left.ceil().max(0.0) as u32;
        let (title, text) = match &cf.kind {
            ConfirmKind::Item(id) => {
                let preview = self.items.iter().find(|i| i.id == *id).map(|i| if i.title.is_empty() { i.body.clone() } else { i.title.clone() }).unwrap_or_default();
                let preview: String = preview.lines().next().unwrap_or("").chars().take(80).collect();
                (
                    lang.t("Karte endgültig löschen?", "Delete card permanently?").to_string(),
                    format!(
                        "{} {}\n\n{}",
                        tasks::key(*id),
                        if preview.is_empty() { String::new() } else { lang.q(&preview) },
                        lang.t("Die Karte wird unwiderruflich gelöscht. Das lässt sich nicht rückgängig machen.", "The card will be deleted permanently. This cannot be undone.")
                    ),
                )
            }
            ConfirmKind::AllDone(n) => (
                lang.t("Alle erledigten Karten löschen?", "Delete all completed cards?").to_string(),
                format!(
                    "{n} {}\n\n{}",
                    if *n == 1 { lang.t("erledigte Karte", "completed card") } else { lang.t("erledigte Karten", "completed cards") },
                    lang.t("Sie werden unwiderruflich gelöscht. Das lässt sich nicht rückgängig machen.", "They will be deleted permanently. This cannot be undone.")
                ),
            ),
            ConfirmKind::Tag(_, name, n) => (
                lang.t("Tag endgültig löschen?", "Delete tag permanently?").to_string(),
                format!(
                    "{}\n\n{}",
                    lang.q(name),
                    if *n == 0 {
                        lang.t("Der Tag wird unwiderruflich gelöscht (er steht auf keiner Karte).", "The tag will be deleted permanently (it is not on any card).").to_string()
                    } else {
                        format!(
                            "{} {n} {}. {}",
                            lang.t("Der Tag wird unwiderruflich gelöscht und von", "The tag will be deleted permanently and removed from"),
                            if *n == 1 { lang.t("Karte entfernt", "card") } else { lang.t("Karten entfernt", "cards") },
                            lang.t("Das lässt sich nicht rückgängig machen.", "This cannot be undone.")
                        )
                    }
                ),
            ),
        };
        let mut go = false;
        let mut close = false;
        let m = egui::Modal::new(Id::new("todo_confirm_modal")).show(ctx, |ui| {
            ui.set_width(Self::modal_width(ctx, 400.0));
            ui.horizontal(|ui| {
                ui.colored_label(DANGER, RichText::new("!").heading().strong());
                ui.label(RichText::new(title).heading());
            });
            ui.add_space(S2);
            ui.label(text);
            ui.add_space(S3);
            ui.horizontal(|ui| {
                let label = if secs > 0 { format!("{} ({secs})", lang.t("Löschen", "Delete")) } else { lang.t("Löschen", "Delete").to_string() };
                let b = egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(if secs > 0 { DANGER.gamma_multiply(0.4) } else { DANGER }).min_size(Vec2::new(110.0, 28.0));
                if ui.add_enabled(secs == 0, b).clicked() {
                    go = true;
                }
                if ui.add(egui::Button::new(lang.t("Abbrechen", "Cancel")).min_size(Vec2::new(90.0, 28.0))).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if secs > 0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if go {
            match &cf.kind {
                ConfirmKind::Item(id) => match tasks::delete_item(conn, *id) {
                    Ok(()) => self.history.retain(|op| !op.touches_item(&[*id])),
                    Err(e) => self.fail(lang, &e),
                },
                ConfirmKind::AllDone(_) => match tasks::delete_done(conn) {
                    Ok(ids) => self.history.retain(|op| !op.touches_item(&ids)),
                    Err(e) => self.fail(lang, &e),
                },
                ConfirmKind::Tag(id, _, _) => match tasks::delete_tag(conn, *id) {
                    Ok(_) => {
                        self.history.retain(|op| !op.touches_tag(*id));
                        self.filter.retain(|f| f != id);
                    }
                    Err(e) => self.fail(lang, &e),
                },
            }
            self.dirty = true;
        } else if !close {
            self.confirm = Some(cf);
        }
    }

    fn merge_dialog(&mut self, ctx: &egui::Context, conn: &rusqlite::Connection, lang: Lang) {
        let Some(ma) = self.merge_ask.take() else { return };
        let mut go = false;
        let mut close = false;
        let m = egui::Modal::new(Id::new("todo_merge_modal")).show(ctx, |ui| {
            ui.set_width(Self::modal_width(ctx, 420.0));
            ui.label(RichText::new(lang.t("Tags zusammenführen", "Merge tags")).heading());
            ui.add_space(S2);
            if ma.from_rename {
                ui.label(format!("{} {}", lang.t("Einen Tag mit diesem Namen gibt es schon:", "A tag with this name already exists:"), lang.q(&ma.into_name)));
                ui.add_space(S1);
            }
            ui.label(format!(
                "{} {} {} {}.",
                lang.t("Tag", "Merge tag"),
                lang.q(&ma.from_name),
                lang.t("in", "into"),
                lang.q(&ma.into_name),
            ));
            ui.label(RichText::new(format!(
                "{} {} {}",
                ma.cards,
                if ma.cards == 1 { lang.t("Karte bekommt", "card gets") } else { lang.t("Karten bekommen", "cards get") },
                lang.q(&ma.into_name),
            )).small().color(style::dim(ui)));
            ui.label(RichText::new(lang.t(
                "Der Tag verschwindet danach. Mit Strg+Z lässt sich das zurücknehmen.",
                "The tag disappears afterwards. Ctrl+Z takes this back.",
            )).small().color(style::dim(ui)));
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(lang.t("Zusammenführen", "Merge")).fill(style::accent().gamma_multiply(0.45)).min_size(Vec2::new(120.0, 28.0))).clicked() {
                    go = true;
                }
                if ui.add(egui::Button::new(lang.t("Abbrechen", "Cancel")).min_size(Vec2::new(90.0, 28.0))).clicked() {
                    close = true;
                }
            });
        });
        if m.should_close() {
            close = true;
        }
        if go {
            match tasks::merge_tags(conn, ma.from, ma.into) {
                Ok(op) => {
                    self.commit(Some(op));
                    self.filter.retain(|f| *f != ma.from);
                }
                Err(e) => self.fail(lang, &e),
            }
        } else if !close {
            self.merge_ask = Some(ma);
        }
    }

    fn tag_manager(&mut self, ctx: &egui::Context, conn: &rusqlite::Connection, lang: Lang) {
        let Some(mut mgr) = self.tag_mgr.take() else { return };
        // Während eine Abfrage darüber offen ist, ist der Verwalter nicht bedienbar (die Abfrage liegt obenauf).
        let mut close = false;
        let tags = self.tags.clone();
        let tag_use = self.tag_use.clone();
        let mut renames: Vec<(i64, String)> = Vec::new();
        let mut recolor: Vec<(i64, &'static str)> = Vec::new();
        let mut merge_into: Vec<(i64, i64)> = Vec::new();
        let mut delete: Vec<i64> = Vec::new();
        let m = egui::Modal::new(Id::new("todo_tags_modal")).show(ctx, |ui| {
            ui.set_width(Self::modal_width(ctx, 540.0));
            ui.label(RichText::new(lang.t("Tags verwalten", "Manage tags")).heading());
            ui.add_space(S2);
            if tags.is_empty() {
                ui.label(RichText::new(lang.t("Noch keine Tags. Sie entstehen, sobald du einer Karte einen Tag gibst.", "No tags yet. They are created as soon as you give a card a tag.")).color(style::dim(ui)));
            }
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([true, true]).id_salt("todo_tags_scroll").show(ui, |ui| {
                for t in &tags {
                    ui.horizontal(|ui| {
                        // Farbfeld
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::CLICK);
                        ui.painter().circle_filled(r.center(), 8.0, tag_color(t));
                        ui.painter().circle_stroke(r.center(), 8.5, Stroke::new(1.0_f32, Color32::from_white_alpha(60)));
                        if resp.hovered() {
                            // Hover-Ring: das Farbfeld ist ein Knopf (öffnet die Farbauswahl, #187)
                            ui.painter().circle_stroke(r.center(), 10.5, Stroke::new(1.5_f32, ui.visuals().widgets.hovered.bg_stroke.color));
                        }
                        if resp.on_hover_text(lang.t("Farbe ändern", "Change color")).clicked() {
                            mgr.picker = if mgr.picker == Some(t.id) { None } else { Some(t.id) };
                        }
                        // Name (umbenennen mit Enter oder beim Verlassen des Felds)
                        let buf = mgr.names.entry(t.id).or_insert_with(|| t.name.clone());
                        let r = ui.add(egui::TextEdit::singleline(buf).desired_width(190.0).margin(Margin::symmetric(8, 4)));
                        if r.lost_focus() {
                            let new = buf.clone();
                            if new.trim() != t.name {
                                renames.push((t.id, new));
                            }
                            mgr.names.remove(&t.id);
                        }
                        let n = tag_use.get(&t.id).copied().unwrap_or(0);
                        ui.label(RichText::new(format!("{n} {}", if n == 1 { lang.t("Karte", "card") } else { lang.t("Karten", "cards") })).small().color(style::dim(ui)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Tag löschen …", "Delete tag …")).clicked() {
                                delete.push(t.id);
                            }
                            egui::ComboBox::from_id_salt(("todo_merge", t.id)).selected_text(lang.t("Zusammenführen …", "Merge into …")).width(140.0).show_ui(ui, |ui| {
                                for o in tags.iter().filter(|o| o.id != t.id) {
                                    if ui.selectable_label(false, &o.name).clicked() {
                                        merge_into.push((t.id, o.id));
                                    }
                                }
                            });
                        });
                    });
                    if mgr.picker == Some(t.id) {
                        ui.horizontal(|ui| {
                            ui.add_space(28.0);
                            for c in tasks::PALETTE {
                                let (r, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::CLICK);
                                let (cr, cg, cb) = tasks::rgb(c);
                                let col = Color32::from_rgb(cr, cg, cb);
                                ui.painter().circle_filled(r.center(), 8.0, col);
                                if t.color.eq_ignore_ascii_case(c) {
                                    ui.painter().circle_stroke(r.center(), 10.0, Stroke::new(1.5_f32, style::accent()));
                                } else if resp.hovered() {
                                    ui.painter().circle_stroke(r.center(), 10.0, Stroke::new(1.5_f32, ui.visuals().widgets.hovered.bg_stroke.color));
                                }
                                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    recolor.push((t.id, c));
                                    mgr.picker = None;
                                }
                            }
                        });
                    }
                }
            });
            ui.add_space(S3);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(lang.t("Schließen", "Close")).min_size(Vec2::new(90.0, 28.0))).clicked() {
                    close = true;
                }
                ui.label(RichText::new(lang.t("Umbenennen: Name ändern und Enter drücken.", "Rename: edit the name and press Enter.")).small().color(style::dim(ui)));
            });
        });
        let blocked = self.confirm.is_some() || self.merge_ask.is_some();
        if m.should_close() && !blocked {
            close = true;
        }
        for (id, name) in renames {
            match tasks::rename_tag(conn, id, &name) {
                Ok(op) => self.commit(op),
                Err(TaskError::TagExists(other)) => {
                    let from = tags.iter().find(|t| t.id == id);
                    let into = tags.iter().find(|t| t.id == other);
                    if let (Some(f), Some(i)) = (from, into) {
                        self.merge_ask = Some(MergeAsk { from: id, into: other, from_name: f.name.clone(), into_name: i.name.clone(), cards: tag_use.get(&id).copied().unwrap_or(0), from_rename: true });
                    }
                }
                Err(TaskError::TagNameEmpty) => {}
                Err(e) => self.fail(lang, &e),
            }
        }
        for (id, c) in recolor {
            let r = tasks::set_tag_color(conn, id, c);
            self.finish(lang, r);
        }
        for (from, into) in merge_into {
            let f = tags.iter().find(|t| t.id == from);
            let i = tags.iter().find(|t| t.id == into);
            if let (Some(f), Some(i)) = (f, i) {
                self.merge_ask = Some(MergeAsk { from, into, from_name: f.name.clone(), into_name: i.name.clone(), cards: tag_use.get(&from).copied().unwrap_or(0), from_rename: false });
            }
        }
        for id in delete {
            if let Some(t) = tags.iter().find(|t| t.id == id) {
                self.confirm = Some(Confirm { kind: ConfirmKind::Tag(id, t.name.clone(), tag_use.get(&id).copied().unwrap_or(0)), since: Instant::now() });
            }
        }
        if !close {
            self.tag_mgr = Some(mgr);
        }
    }
}

// ------------------------------------------------------------------ Anbindung an die Anwendung

impl App {
    /// Reiter „Todo-Liste“ zeichnen: Kopf mit dem Schalter „Expertenmodus“, darunter die einfache Liste oder die Projektansicht.
    pub(super) fn tasks_view(&mut self, ctx: &egui::Context) {
        let env = Env { db: self.db.as_ref(), db_error: self.db_error.as_deref(), lang: self.settings.language, bg: self.theme().bg, undo_steps: self.settings.undo_steps as usize };
        if let Some(db) = env.db {
            self.plan.ensure_loaded(Some(db));
            self.plan.header(ctx, &env, db);
            #[cfg(feature = "readme-shots")]
            self.plan.dev_view(db, env.lang);
            if self.plan.expert {
                self.plan.page(ctx, &env, db);
                if self.plan.take_leave_to_terminals() {
                    // „Zurück“ aus einem Ticket, das vom Terminal Hub aus geöffnet wurde (Chip, Palette, Mini-Suche)
                    self.set_mode(titlebar::Mode::Terminals);
                    ctx.request_repaint();
                }
                let outs = self.plan.take_quick_out();
                self.handle_quick_out(ctx, outs);
                for t in self.plan.take_toasts() {
                    self.toast(t);
                }
                return;
            }
        }
        self.tasks.show(ctx, &env);
        if let Some(n) = self.tasks.notice.take() {
            self.notice = Some((n, Instant::now()));
        }
    }

    /// Ergebnisse des Schnellmenüs: Toasts (mit „Rückgängig“) zeigen und gewünschte Sitzungen starten.
    fn handle_quick_out(&mut self, ctx: &egui::Context, outs: Vec<super::planning_ui::QuickOut>) {
        use super::planning_ui::QuickOut;
        use super::toasts::{ToastAction, ToastKind};
        let lang = self.settings.language;
        for o in outs {
            match o {
                QuickOut::Toast { text, error, undo } => {
                    let action = undo.map(|token| ToastAction { label: lang.t("Rückgängig", "Undo").to_string(), token, window: self.wins.cur });
                    self.push_toast_action(text, if error { ToastKind::Error } else { ToastKind::Info }, None, action);
                }
                QuickOut::Session(req) => self.quick_start_session(ctx, req),
            }
        }
    }

    /// „Sitzung starten“ aus dem Schnellmenü: neue Sitzung (Shell nur, wenn sie bekannt ist), mit dem Ticket verknüpft.
    fn quick_start_session(&mut self, ctx: &egui::Context, req: super::planning_ui::SessionRequest) {
        use super::toasts::ToastKind;
        let lang = self.settings.language;
        let shell_id = req.shell.clone().filter(|s| self.shell_by_id(s).is_some());
        let unknown_shell = req.shell.is_some() && shell_id.is_none();
        let id = self.new_session(ctx, super::NewOpts { shell_id, cwd: req.cwd.clone(), name: Some(req.ticket_key.clone()), claude: req.claude, prompt: req.prompt.clone(), ..Default::default() });
        let linked = match self.db.as_ref() {
            Some(db) => super::terminal_links::link_ticket_to(&db.conn, req.ticket_id, id),
            None => Ok(()),
        };
        self.tlinks_invalidate();
        match linked {
            Ok(()) if unknown_shell => self.push_toast(lang.t("Die Shell der Aktion ist unbekannt – es wurde die Standard-Shell benutzt.", "The action's shell is unknown – the default shell was used.").to_string(), ToastKind::Warning, Some(req.ticket_key)),
            Ok(()) => self.push_toast(format!("{} {}", req.ticket_key, lang.t("mit neuer Sitzung verknüpft.", "linked to a new session.")), ToastKind::Info, Some(req.ticket_key.clone())),
            Err(e) => self.push_toast(e.message(lang), ToastKind::Error, None),
        }
    }

    /// Klick auf den Knopf eines Toasts (heute nur „Rückgängig“ des Schnellmenüs).
    /// `window`: Fenster, dessen Todo-Ansicht die Änderung gemacht hat; ihr Rückgängig-Stapel liegt dort.
    pub(super) fn toast_action_clicked(&mut self, ctx: &egui::Context, token: u64, window: u64) {
        if window != self.wins.cur {
            self.wins_undo_later(ctx, window, token);
            return;
        }
        self.undo_here(ctx, token);
    }

    /// „Rückgängig“ mit dem Stapel der Todo-Ansicht des gerade gezeichneten Fensters.
    pub(super) fn undo_here(&mut self, _ctx: &egui::Context, token: u64) {
        use super::toasts::ToastKind;
        let lang = self.settings.language;
        let Some(db) = self.db.as_ref() else { return };
        match self.plan.quick_undo(&db.conn, token, lang) {
            Ok(text) => self.push_toast(text, ToastKind::Info, None),
            Err(e) if e.is_empty() => {}
            Err(e) => self.push_toast(e, ToastKind::Error, None),
        }
    }

    /// Akzentfarbe setzen: im Reiter „Todo-Liste“ im Expertenmodus die Farbe des gewählten Projekts, sonst der Standard.
    pub(super) fn sync_accent(&mut self) {
        if self.mode == titlebar::Mode::Tasks {
            self.plan.ensure_loaded(self.db.as_ref());
        }
        let c = if self.mode == titlebar::Mode::Tasks { self.plan.accent() } else { None };
        style::set_accent(c.unwrap_or(style::ACCENT));
    }

    /// Auswahl „Änderungshistorie“ im Tab „Daten & Sicherung“.
    pub(super) fn undo_steps_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        let dim = style::dim(ui);
        ui.label(RichText::new(lang.t("Änderungshistorie (Rückgängig)", "Undo history")).strong());
        ui.label(RichText::new(lang.t(
            "Wie viele Änderungen der Todo-Liste sich mit Strg+Z zurücknehmen lassen. Die Historie gilt für die laufende Sitzung; Löschen bleibt endgültig.",
            "How many changes to the to-do list Ctrl+Z can take back. The history lasts for the running session; deleting is final.",
        )).small().color(dim));
        ui.add_space(S1);
        ui.horizontal(|ui| {
            ui.label(lang.t("Schritte:", "Steps:"));
            egui::ComboBox::from_id_salt("undo_steps").selected_text(self.settings.undo_steps.to_string()).width(80.0).show_ui(ui, |ui| {
                for n in UNDO_STEP_CHOICES {
                    ui.selectable_value(&mut self.settings.undo_steps, n, n.to_string());
                }
            });
        });
    }
}
