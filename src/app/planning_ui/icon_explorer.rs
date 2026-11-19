//! Icon-Explorer (#190): eigenes Fenster mit den eingebauten Icons (Kategorie „Eingebaut“) und allen Lucide-Icons – Suche
//! nach Name und Stichwort, Kategorien, Raster mit Vorschau beim Darüberfahren, große Vorschau des gewählten Icons,
//! Auswählen/Abbrechen (Esc schließt). Geöffnet wird er aus jedem Icon-Picker über „Weitere Icons …“
//! ([`super::widgets::icon_picker`]); die eingebauten Icons stehen weiter direkt im Picker. Gespeichert wird ein Lucide-Icon
//! als `lucide:<name>` (`planning::icons`), ein eingebautes unter seinem Namen.
//!
//! Gezeichnet werden Lucide-Icons über die eingebettete Schrift (`planning::icons::lucide::font_ttf`, komprimiert in der
//! .exe, beim ersten Lucide-Icon entpackt und als eigene egui-Schriftfamilie [`FONT_FAMILY`] nachgeladen): [`paint_icon`]
//! zeichnet eingebaute Namen, `lucide:<name>` (auch alte Namen) und für Unbekanntes einen Platzhalter – in derselben Größe
//! und Farbe.

use super::widgets::{self, type_icon};
use crate::planning::icons::{self, lucide, lucide::LucideIcon, ICONS};
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Align2, Color32, FontFamily, FontId, Id, Key, Modifiers, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::sync::Arc;

/// Name der egui-Schriftfamilie mit der Lucide-Schrift (enthält nur diese eine Schrift).
pub const FONT_FAMILY: &str = "lucide";

/// Bindet die Lucide-Schrift als eigene Familie ein, falls sie schon entpackt ist (von `app::build_fonts`, damit ein neues
/// Setzen der Schriften sie behält). Sonst passiert nichts: beim ersten Lucide-Icon lädt [`paint_icon`] sie nach.
pub fn install_font(fonts: &mut egui::FontDefinitions) {
    if let Some(ttf) = lucide::font_if_unpacked() {
        fonts.font_data.insert(FONT_FAMILY.to_owned(), Arc::new(egui::FontData::from_static(ttf)));
        fonts.families.insert(family(), vec![FONT_FAMILY.to_owned()]);
    }
}

fn family() -> FontFamily {
    FontFamily::Name(FONT_FAMILY.into())
}

/// Was für einen gespeicherten Icon-Namen gezeichnet wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    /// Eingebautes, selbst gezeichnetes Icon (auch für unbekannte eingebaute Namen – die bekommen dort ein Quadrat).
    Builtin,
    /// Lucide-Icon mit seinem Zeichen in der Schrift (auch unter einem alten Namen).
    Lucide(char),
    /// `lucide:<name>`, das es in dieser Version nicht gibt: Platzhalter.
    Placeholder,
}

/// Ordnet einen gespeicherten Icon-Namen ein (ohne Blick auf die Schrift).
pub fn icon_kind(name: &str) -> IconKind {
    match icons::lucide_name(name) {
        None => IconKind::Builtin,
        Some(n) => lucide::resolve(n).map_or(IconKind::Placeholder, |i| IconKind::Lucide(i.ch)),
    }
}

/// Faktor Schriftgröße / Kantenlänge, damit Lucide-Icons so groß wirken wie die eingebauten (die nutzen etwa 14 von 16).
const EM_PER_SIZE: f32 = 1.05;

/// Ist die Lucide-Schrift in diesem Kontext geladen? Sonst wird sie (einmal, entpackt beim ersten Mal) nachgeladen; sie
/// gilt ab dem nächsten Bild – bis dahin zeichnet der Aufrufer einen Platzhalter.
fn ensure_font(ctx: &egui::Context) -> bool {
    if ctx.fonts(|f| f.definitions().families.contains_key(&family())) {
        return true;
    }
    let pass = ctx.cumulative_pass_nr();
    let key = Id::new("pd_lucide_font_requested");
    // Höchstens alle paar Bilder anfordern (ein Setzen der Schriften dazwischen hätte die Anforderung verworfen).
    let last: Option<u64> = ctx.data(|d| d.get_temp(key));
    if last.is_none_or(|l| pass >= l + 3) {
        let ttf = lucide::font_ttf();
        if !ttf.is_empty() {
            ctx.add_font(egui::epaint::text::FontInsert::new(
                FONT_FAMILY,
                egui::FontData::from_static(ttf),
                vec![egui::epaint::text::InsertFontFamily { family: family(), priority: egui::epaint::text::FontPriority::Highest }],
            ));
            ctx.data_mut(|d| d.insert_temp(key, pass));
            ctx.request_repaint();
        }
    }
    false
}

/// Zeichnet ein Lucide-Zeichen mittig um `c` (nach der tatsächlichen Tinte zentriert); `false`, wenn die Schrift (noch) fehlt.
fn paint_glyph(p: &Painter, ch: char, c: Pos2, size: f32, col: Color32) -> bool {
    if !ensure_font(p.ctx()) {
        return false;
    }
    let galley = p.layout_no_wrap(ch.to_string(), FontId::new(size * EM_PER_SIZE, family()), col);
    let ink = galley.rows.first().map(|r| r.visuals.mesh_bounds.translate(r.pos.to_vec2())).filter(|r| r.is_positive()).unwrap_or(galley.rect);
    p.galley(c - ink.center().to_vec2(), galley, col);
    true
}

/// Platzhalter für ein unbekanntes Lucide-Icon: gestricheltes Quadrat mit Punkt.
fn paint_placeholder(p: &Painter, c: Pos2, size: f32, col: Color32) {
    let k = size / 16.0;
    let st = Stroke::new((1.2 * k).max(1.0), col.gamma_multiply(0.8));
    let r = Rect::from_center_size(c, Vec2::splat(12.0 * k));
    let dash = |a: Pos2, b: Pos2| {
        p.add(egui::Shape::dashed_line(&[a, b], st, 2.0 * k, 1.6 * k));
    };
    dash(r.left_top(), r.right_top());
    dash(r.right_top(), r.right_bottom());
    dash(r.right_bottom(), r.left_bottom());
    dash(r.left_bottom(), r.left_top());
    p.circle_filled(c, 1.3 * k, col);
}

/// Zeichnet ein Icon mittig um `center` mit der Kantenlänge `size` (Pixel) in `color`: eingebaute Namen
/// (`planning::icons::ICONS`), `lucide:<name>` über die Lucide-Schrift (auch alte Namen), unbekannte `lucide:`-Namen als
/// Platzhalter. Öffentliche Zeichen-Schnittstelle für alle Stellen mit Icons (Typen, Fälligkeits-Markierungen #188 …).
pub fn paint_icon(painter: &Painter, name: &str, center: Pos2, size: f32, color: Color32) {
    type_icon(painter, name, center, size, color);
}

/// Lucide-Teil von [`paint_icon`] (aufgerufen von `widgets::type_icon`): `true`, wenn `name` ein `lucide:`-Name war und
/// gezeichnet wurde (Icon oder Platzhalter); `false` für eingebaute Namen.
pub(super) fn paint_lucide(p: &Painter, name: &str, c: Pos2, size: f32, col: Color32) -> bool {
    match icon_kind(name) {
        IconKind::Builtin => false,
        IconKind::Lucide(ch) => {
            if !paint_glyph(p, ch, c, size, col) {
                paint_placeholder(p, c, size, col);
            }
            true
        }
        IconKind::Placeholder => {
            paint_placeholder(p, c, size, col);
            true
        }
    }
}

/// Text für den Hinweis beim Darüberfahren: eingebaute Namen wie gespeichert, Lucide als „name (Lucide)“ (aktueller Name).
pub fn icon_label(name: &str) -> String {
    match icons::lucide_name(name) {
        Some(n) => format!("{} (Lucide)", lucide::canonical(n).unwrap_or(n)),
        None => name.to_string(),
    }
}

// ------------------------------------------------------------------ Explorer

/// Kategorie-Schlüssel der eingebauten Icons im Explorer (kein Lucide-Schlüssel).
pub const BUILTIN_CATEGORY: &str = "paneldeck-builtin";

/// Ein Eintrag im Raster: eingebautes oder Lucide-Icon.
#[derive(Debug, Clone, Copy)]
pub enum Item {
    Builtin(&'static str),
    Lucide(&'static LucideIcon),
}

impl Item {
    /// Gespeicherter Wert (`bug` bzw. `lucide:alarm-clock`).
    pub fn value(&self) -> String {
        match self {
            Item::Builtin(n) => (*n).to_string(),
            Item::Lucide(i) => icons::lucide_value(i.name),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Item::Builtin(n) => n,
            Item::Lucide(i) => i.name,
        }
    }

    fn is(&self, value: &str) -> bool {
        match self {
            Item::Builtin(n) => *n == value,
            Item::Lucide(i) => icons::lucide_name(value) == Some(i.name),
        }
    }

    fn paint(&self, p: &Painter, c: Pos2, size: f32, col: Color32) {
        match self {
            Item::Builtin(n) => type_icon(p, n, c, size, col),
            Item::Lucide(i) => {
                if !paint_glyph(p, i.ch, c, size, col) {
                    paint_placeholder(p, c, size, col);
                }
            }
        }
    }
}

/// Treffer der Explorer-Suche: eingebaute Icons (Kategorie [`BUILTIN_CATEGORY`] oder alle) und Lucide-Icons (eine
/// Lucide-Kategorie oder alle), nach Rang; bei gleichem Rang die eingebauten zuerst, dann nach Name.
pub fn explorer_search(query: &str, category: Option<&str>) -> Vec<Item> {
    let mut hits: Vec<(lucide::Rank, u8, Item)> = Vec::new();
    if category.is_none_or(|c| c == BUILTIN_CATEGORY) {
        hits.extend(ICONS.iter().filter_map(|n| lucide::rank(n, &[], query).map(|r| (r, 0, Item::Builtin(n)))));
    }
    if category != Some(BUILTIN_CATEGORY) {
        hits.extend(lucide::search_ranked(query, category).into_iter().map(|(r, i)| (r, 1, Item::Lucide(i))));
    }
    hits.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.name().cmp(b.2.name())));
    hits.into_iter().map(|(_, _, i)| i).collect()
}

/// Ergebnis eines Bildes des Explorers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerResult {
    /// Fenster bleibt offen.
    Open,
    /// „Auswählen“ (oder Doppelklick/Enter): der zu speichernde Wert, z. B. `bug` oder `lucide:alarm-clock`.
    Picked(String),
    /// „Abbrechen“, Esc oder Schließen-Kreuz.
    Cancelled,
}

/// Zustand des Explorer-Fensters (liegt bei Nutzung über den Picker in `egui::Memory::data`).
#[derive(Clone, Default)]
pub struct IconExplorer {
    /// Suchtext (Namen und Stichwörter, englisch).
    pub query: String,
    /// Kategorie-Schlüssel ([`BUILTIN_CATEGORY`] oder Lucide) oder `None` = alle.
    pub category: Option<String>,
    /// Gewähltes Icon als gespeicherter Wert (`bug`, `lucide:alarm-clock`).
    pub selected: Option<String>,
    /// Icon unter der Maus im letzten Bild (Wert); die Vorschau zeigt es.
    hovered: Option<String>,
    focus_search: bool,
    /// Bilder, in denen noch zum gewählten Icon gescrollt wird (das erste Bild eines Fensters misst nur aus).
    scroll_frames: u8,
    /// Letzter Durchlauf, in dem der Picker dieses Explorers gezeichnet wurde.
    seen_pass: u64,
    /// Fokus am Ende des letzten Bildes (ein Feld, das bei Enter den Fokus abgibt, ist im selben Bild schon fokuslos).
    focus_end: Option<Id>,
    cache: Option<(String, Option<String>, Arc<Vec<Item>>)>,
}

const CELL: f32 = 40.0;
const GAP: f32 = 4.0;

impl IconExplorer {
    /// Neuer Explorer; `current` ist der gespeicherte Wert des Pickers (ein bekanntes Icon wird vorgewählt, ein alter
    /// Lucide-Name unter dem neuen).
    pub fn new(current: Option<&str>) -> Self {
        let selected = current.and_then(|c| match icons::lucide_name(c) {
            Some(n) => lucide::canonical(n).map(icons::lucide_value),
            None => ICONS.contains(&c).then(|| c.to_string()),
        });
        IconExplorer { scroll_frames: if selected.is_some() { 2 } else { 0 }, selected, focus_search: true, ..Default::default() }
    }

    /// Treffer für Suche und Kategorie (zwischengespeichert, solange sich beides nicht ändert).
    pub fn results(&mut self) -> Arc<Vec<Item>> {
        if let Some((q, c, r)) = &self.cache {
            if *q == self.query && *c == self.category {
                return r.clone();
            }
        }
        let r = Arc::new(explorer_search(&self.query, self.category.as_deref()));
        self.cache = Some((self.query.clone(), self.category.clone(), r.clone()));
        r
    }

    /// Id einer Rasterzelle (`value` = gespeicherter Wert; für Tests und Bildschirmleser).
    pub fn cell_id(win: Id, value: &str) -> Id {
        win.with(("cell", value))
    }

    /// Id des Suchfelds.
    pub fn search_id(win: Id) -> Id {
        win.with("search")
    }

    /// Zeigt das Fenster für ein Bild. `id` muss je Picker eindeutig sein.
    pub fn show(&mut self, ctx: &egui::Context, id: Id, lang: Lang) -> ExplorerResult {
        let mut result = ExplorerResult::Open;
        // Esc/Enter nur, wenn der Explorer gemeint ist: Fokus im Suchfeld oder nirgends (nicht z. B. im Namensfeld des Typs).
        let search = Self::search_id(id);
        let focus = ctx.memory(|m| m.focused());
        let mine = |f: Option<Id>| f.is_none() || f == Some(search);
        let ours = mine(focus) && mine(self.focus_end);
        let (esc, enter) = if ours { ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Escape), i.key_pressed(Key::Enter))) } else { (false, false) };
        if esc {
            return ExplorerResult::Cancelled;
        }
        let mut open = true;
        let screen = ctx.content_rect();
        let size = Vec2::new(widgets::modal_width(ctx, 820.0), (screen.height() - 80.0).clamp(320.0, 560.0));
        style::movable(ctx, style::salted(id), egui::Window::new(lang.t("Icon-Explorer", "Icon explorer")))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .default_size(size)
            .min_size(Vec2::new(480.0, 320.0))
            .show(ctx, |ui| {
                result = self.body(ui, id, lang, enter);
            });
        self.focus_end = ctx.memory(|m| m.focused());
        if !open {
            return ExplorerResult::Cancelled;
        }
        result
    }

    fn body(&mut self, ui: &mut Ui, id: Id, lang: Lang, enter: bool) -> ExplorerResult {
        let mut result = ExplorerResult::Open;
        // Die Vorschau zeigt, was im letzten Bild unter der Maus lag (das Raster wird erst danach gezeichnet).
        let prev_hover = self.hovered.take();
        egui::TopBottomPanel::top(id.with("top")).show_separator_line(false).show_inside(ui, |ui| {
            let edit = egui::TextEdit::singleline(&mut self.query)
                .id(Self::search_id(id))
                .hint_text(lang.t("Namen und Stichwörter suchen (englisch) …", "Search names and tags …"))
                .desired_width(ui.available_width());
            let r = ui.add(edit);
            if self.focus_search {
                r.request_focus();
                self.focus_search = false;
            }
            ui.add_space(S1);
        });
        // Erst nach dem Suchfeld: so zeigt das Raster schon im selben Bild die Treffer zur neuen Eingabe.
        let results = self.results();
        egui::TopBottomPanel::bottom(id.with("bottom")).show_separator_line(false).show_inside(ui, |ui| {
            ui.add_space(S1);
            ui.horizontal(|ui| {
                let n = results.len();
                ui.label(RichText::new(match lang {
                    Lang::De => format!("{n} Icons"),
                    Lang::En => format!("{n} icons"),
                }).color(style::dim(ui)));
                widgets::hint(ui, &format!("Lucide {} · ISC", lucide::VERSION));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::secondary(ui, lang.t("Abbrechen", "Cancel")).clicked() {
                        result = ExplorerResult::Cancelled;
                    }
                    let can = self.selected.is_some();
                    if widgets::primary_enabled(ui, can, lang.t("Auswählen", "Select")).clicked() || (can && enter && result == ExplorerResult::Open) {
                        if let Some(s) = &self.selected {
                            result = ExplorerResult::Picked(s.clone());
                        }
                    }
                });
            });
        });
        egui::SidePanel::left(id.with("cats")).resizable(false).exact_width(190.0).show_inside(ui, |ui| self.categories(ui, id, lang));
        egui::SidePanel::right(id.with("preview")).resizable(false).exact_width(210.0).show_inside(ui, |ui| self.preview(ui, prev_hover.as_deref(), lang));
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(v) = self.grid(ui, id, &results, lang) {
                result = ExplorerResult::Picked(v);
            }
        });
        // Neues Bild nur, wenn sich das Icon unter der Maus geändert hat (sonst stimmt die Vorschau schon).
        if self.hovered != prev_hover {
            ui.ctx().request_repaint();
        }
        result
    }

    fn categories(&mut self, ui: &mut Ui, id: Id, lang: Lang) {
        let mut cats: Vec<(&'static str, String)> = lucide::table().categories.iter().map(|c| (c.key, lucide::category_title(c.key, lang).to_string())).collect();
        cats.sort_by_key(|c| c.1.to_lowercase());
        egui::ScrollArea::vertical().id_salt(id.with("cat_scroll")).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if ui.selectable_label(self.category.is_none(), RichText::new(lang.t("Alle", "All")).strong()).clicked() {
                self.category = None;
            }
            let on = self.category.as_deref() == Some(BUILTIN_CATEGORY);
            if ui.selectable_label(on, RichText::new(lang.t("Eingebaut", "Built-in")).strong()).clicked() {
                self.category = if on { None } else { Some(BUILTIN_CATEGORY.to_string()) };
            }
            ui.separator();
            for (key, title) in cats {
                let on = self.category.as_deref() == Some(key);
                if ui.selectable_label(on, title).clicked() {
                    self.category = if on { None } else { Some(key.to_string()) };
                }
            }
        });
    }

    fn item_of(value: &str) -> Option<Item> {
        match icons::lucide_name(value) {
            Some(n) => lucide::resolve(n).map(Item::Lucide),
            None => ICONS.iter().find(|i| **i == value).map(|n| Item::Builtin(n)),
        }
    }

    fn preview(&mut self, ui: &mut Ui, hovered: Option<&str>, lang: Lang) {
        let shown = hovered.or(self.selected.as_deref()).and_then(Self::item_of);
        ui.add_space(S2);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 120.0), Sense::hover());
        ui.painter().rect_filled(rect, 8.0, ui.visuals().faint_bg_color);
        let Some(item) = shown else {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, lang.t("Kein Icon gewählt", "No icon selected"), FontId::proportional(13.0), style::dim(ui));
            return;
        };
        item.paint(ui.painter(), rect.center(), 80.0, ui.visuals().strong_text_color());
        ui.add_space(S2);
        ui.label(RichText::new(item.name()).strong().monospace());
        ui.label(RichText::new(item.value()).small().monospace().color(style::dim(ui)));
        if hovered.is_some() && self.selected.as_deref().is_none_or(|s| !item.is(s)) {
            widgets::hint(ui, lang.t("Vorschau – klicken zum Wählen", "Preview – click to select"));
        }
        match item {
            Item::Builtin(_) => {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Eingebautes Icon", "Built-in icon")).small().color(style::dim(ui)));
            }
            Item::Lucide(icon) => {
                if !icon.categories.is_empty() {
                    ui.add_space(S1);
                    ui.label(RichText::new(lang.t("Kategorien", "Categories")).small().strong().color(style::dim(ui)));
                    let cats: Vec<&str> = icon.categories.iter().map(|c| lucide::category_title(c, lang)).collect();
                    ui.label(RichText::new(cats.join(", ")).small());
                }
                if !icon.tags.is_empty() {
                    ui.add_space(S1);
                    ui.label(RichText::new(lang.t("Stichwörter", "Tags")).small().strong().color(style::dim(ui)));
                    egui::ScrollArea::vertical().id_salt("tags").max_height(ui.available_height().max(40.0)).show(ui, |ui| {
                        ui.label(RichText::new(icon.tags.join(", ")).small().color(style::dim(ui)));
                    });
                }
            }
        }
    }

    /// Raster der Treffer (nur sichtbare Zeilen werden gebaut); liefert einen Wert bei Doppelklick.
    fn grid(&mut self, ui: &mut Ui, id: Id, results: &[Item], lang: Lang) -> Option<String> {
        if results.is_empty() {
            ui.add_space(S2 * 2.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(lang.t("Keine Treffer.", "No matches.")).color(style::dim(ui))));
            return None;
        }
        let width = ui.available_width();
        let cols = (((width + GAP) / (CELL + GAP)).floor() as usize).max(1);
        let rows = results.len().div_ceil(cols);
        let mut picked = None;
        let mut area = egui::ScrollArea::vertical().id_salt(id.with("grid")).auto_shrink([false, false]);
        if self.scroll_frames > 0 {
            self.scroll_frames -= 1;
            if let Some(pos) = self.selected.as_deref().and_then(|s| results.iter().position(|i| i.is(s))) {
                area = area.vertical_scroll_offset(((pos / cols) as f32 * (CELL + GAP) - CELL).max(0.0));
            }
        }
        let text = ui.visuals().text_color();
        let accent = style::accent();
        area.show_rows(ui, CELL, rows, |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::splat(GAP);
            for row in range {
                ui.horizontal(|ui| {
                    for item in results.iter().skip(row * cols).take(cols) {
                        let value = item.value();
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::hover());
                        let resp = ui.interact(rect, Self::cell_id(id, &value), Sense::click());
                        let sel = self.selected.as_deref() == Some(value.as_str());
                        if sel {
                            ui.painter().rect_filled(rect, 6.0, accent.gamma_multiply(0.25));
                            ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.3_f32, accent), StrokeKind::Inside);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        item.paint(ui.painter(), rect.center(), 22.0, if sel { accent } else { text });
                        if resp.hovered() {
                            self.hovered = Some(value.clone());
                        }
                        let resp = resp.on_hover_text(item.name());
                        if resp.clicked() {
                            self.selected = Some(value.clone());
                        }
                        if resp.double_clicked() {
                            picked = Some(value);
                        }
                    }
                });
            }
        });
        picked
    }
}

/// Id-Schlüssel des offenen Explorers im Kontext (höchstens einer je Fenster).
fn active_key() -> Id {
    Id::new("pd_icon_explorer_active")
}

/// Zeigt den Explorer eines Pickers, falls offen (Zustand in `egui::Memory::data` unter `id`); vom Picker jedes Bild
/// aufgerufen. `Some(wert)` = gewählt. Ein Explorer, dessen Picker im letzten Durchlauf nicht gezeichnet wurde (anderer Typ,
/// anderer Reiter), wird verworfen statt später wieder aufzutauchen.
pub(super) fn picker_explorer(ctx: &egui::Context, id: Id, lang: Lang) -> Option<String> {
    let mut st: IconExplorer = ctx.data_mut(|d| d.get_temp(id))?;
    let pass = ctx.cumulative_pass_nr();
    let active: Option<Id> = ctx.data(|d| d.get_temp(active_key()));
    if st.seen_pass + 1 < pass || active != Some(id) {
        close_explorer(ctx, id);
        return None;
    }
    st.seen_pass = pass;
    match st.show(ctx, id, lang) {
        ExplorerResult::Open => {
            ctx.data_mut(|d| d.insert_temp(id, st));
            None
        }
        ExplorerResult::Picked(v) => {
            close_explorer(ctx, id);
            Some(v)
        }
        ExplorerResult::Cancelled => {
            close_explorer(ctx, id);
            None
        }
    }
}

fn close_explorer(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| {
        d.remove::<IconExplorer>(id);
        if d.get_temp::<Id>(active_key()) == Some(id) {
            d.remove::<Id>(active_key());
        }
    });
}

/// Öffnet den Explorer eines Pickers (vorgewählt: `current`); ein anderer offener Explorer wird dabei geschlossen.
pub(super) fn open_picker_explorer(ctx: &egui::Context, id: Id, current: Option<&str>) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        if let Some(old) = d.get_temp::<Id>(active_key()).filter(|o| *o != id) {
            d.remove::<IconExplorer>(old);
        }
        d.insert_temp(active_key(), id);
        d.insert_temp(id, IconExplorer { seen_pass: pass, ..IconExplorer::new(current) });
    });
}

/// Ist der Explorer dieses Pickers offen?
pub fn explorer_open(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<IconExplorer>(id).is_some())
}
