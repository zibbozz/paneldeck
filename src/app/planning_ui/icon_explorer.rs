//! Icon-Explorer (#190): eigenes Fenster mit allen Lucide-Icons – Suche nach Name und Stichwort, Kategorien, Raster mit
//! Vorschau beim Darüberfahren, große Vorschau des gewählten Icons, Auswählen/Abbrechen (Esc schließt). Geöffnet wird er aus
//! jedem Icon-Picker über „Weitere Icons …“ ([`super::widgets::icon_picker`]); die eingebauten Icons stehen weiter direkt im
//! Picker. Gespeichert wird ein Lucide-Icon als `lucide:<name>` (`planning::icons`).
//!
//! Gezeichnet werden Lucide-Icons über die eingebettete Schrift `assets/fonts/lucide.ttf` (eigene egui-Schriftfamilie
//! [`FONT_FAMILY`], nur für Icons): [`paint_icon`] zeichnet eingebaute Namen, `lucide:<name>` und für Unbekanntes einen
//! Platzhalter – in derselben Größe und Farbe.

use super::widgets::{self, type_icon};
use crate::planning::icons::{self, lucide, lucide::LucideIcon};
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Align2, Color32, FontFamily, FontId, Id, Key, Modifiers, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::sync::Arc;

/// Name der egui-Schriftfamilie mit der Lucide-Schrift (enthält nur diese eine Schrift).
pub const FONT_FAMILY: &str = "lucide";

/// Bindet die Lucide-Schrift als eigene Familie ein (von `app::build_fonts`); andere Familien bleiben unberührt.
pub fn install_font(fonts: &mut egui::FontDefinitions) {
    fonts.font_data.insert(FONT_FAMILY.to_owned(), Arc::new(egui::FontData::from_static(include_bytes!("../../../assets/fonts/lucide.ttf"))));
    fonts.families.insert(FontFamily::Name(FONT_FAMILY.into()), vec![FONT_FAMILY.to_owned()]);
}

fn family() -> FontFamily {
    FontFamily::Name(FONT_FAMILY.into())
}

/// Was für einen gespeicherten Icon-Namen gezeichnet wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    /// Eingebautes, selbst gezeichnetes Icon (auch für unbekannte eingebaute Namen – die bekommen dort ein Quadrat).
    Builtin,
    /// Lucide-Icon mit seinem Zeichen in der Schrift.
    Lucide(char),
    /// `lucide:<name>`, das es in dieser Version nicht gibt (oder ohne geladene Schrift): Platzhalter.
    Placeholder,
}

/// Ordnet einen gespeicherten Icon-Namen ein (ohne Blick auf die Schrift).
pub fn icon_kind(name: &str) -> IconKind {
    match icons::lucide_name(name) {
        None => IconKind::Builtin,
        Some(n) => lucide::get(n).map_or(IconKind::Placeholder, |i| IconKind::Lucide(i.ch)),
    }
}

/// Faktor Schriftgröße / Kantenlänge, damit Lucide-Icons so groß wirken wie die eingebauten (die nutzen etwa 14 von 16).
const EM_PER_SIZE: f32 = 1.05;

fn font_loaded(p: &Painter) -> bool {
    p.ctx().fonts(|f| f.definitions().families.contains_key(&family()))
}

/// Zeichnet ein Lucide-Zeichen mittig um `c` (nach der tatsächlichen Tinte zentriert); `false`, wenn die Schrift fehlt.
fn paint_glyph(p: &Painter, ch: char, c: Pos2, size: f32, col: Color32) -> bool {
    if !font_loaded(p) {
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
/// (`planning::icons::ICONS`), `lucide:<name>` über die Lucide-Schrift, unbekannte `lucide:`-Namen als Platzhalter.
/// Öffentliche Zeichen-Schnittstelle für alle Stellen mit Icons (Typen, Fälligkeits-Markierungen #188 …).
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

/// Text für den Hinweis beim Darüberfahren: eingebaute Namen wie gespeichert, Lucide als „name (Lucide)“.
pub fn icon_label(name: &str) -> String {
    match icons::lucide_name(name) {
        Some(n) => format!("{n} (Lucide)"),
        None => name.to_string(),
    }
}

// ------------------------------------------------------------------ Explorer

/// Ergebnis eines Bildes des Explorers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerResult {
    /// Fenster bleibt offen.
    Open,
    /// „Auswählen“ (oder Doppelklick/Enter): kanonischer Lucide-Name ohne Vorsatz, z. B. `alarm-clock`.
    Picked(String),
    /// „Abbrechen“, Esc oder Schließen-Kreuz.
    Cancelled,
}

/// Zustand des Explorer-Fensters (liegt bei Nutzung über den Picker in `egui::Memory::data`).
#[derive(Clone, Default)]
pub struct IconExplorer {
    /// Suchtext (Namen und Stichwörter, englisch).
    pub query: String,
    /// Kategorie-Schlüssel oder `None` = alle.
    pub category: Option<String>,
    /// Gewähltes Icon (kanonischer Name).
    pub selected: Option<String>,
    hovered: Option<&'static str>,
    focus_search: bool,
    scroll_to_selected: bool,
    cache: Option<(String, Option<String>, Arc<Vec<&'static LucideIcon>>)>,
}

const CELL: f32 = 40.0;
const GAP: f32 = 4.0;

impl IconExplorer {
    /// Neuer Explorer; `current` ist der gespeicherte Wert des Pickers (ein bekanntes `lucide:`-Icon wird vorgewählt).
    pub fn new(current: Option<&str>) -> Self {
        let selected = current.and_then(icons::lucide_name).filter(|n| lucide::get(n).is_some()).map(str::to_string);
        IconExplorer { scroll_to_selected: selected.is_some(), selected, focus_search: true, ..Default::default() }
    }

    /// Treffer für Suche und Kategorie (zwischengespeichert, solange sich beides nicht ändert).
    pub fn results(&mut self) -> Arc<Vec<&'static LucideIcon>> {
        if let Some((q, c, r)) = &self.cache {
            if *q == self.query && *c == self.category {
                return r.clone();
            }
        }
        let r = Arc::new(lucide::search(&self.query, self.category.as_deref()));
        self.cache = Some((self.query.clone(), self.category.clone(), r.clone()));
        r
    }

    /// Id einer Rasterzelle (für Tests und Bildschirmleser).
    pub fn cell_id(win: Id, name: &str) -> Id {
        win.with(("cell", name))
    }

    /// Zeigt das Fenster für ein Bild. `id` muss je Picker eindeutig sein.
    pub fn show(&mut self, ctx: &egui::Context, id: Id, lang: Lang) -> ExplorerResult {
        let mut result = ExplorerResult::Open;
        // Esc schließt immer (auch wenn das Suchfeld den Fokus hat); Enter wählt, wenn etwas gewählt ist.
        let (esc, enter) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Escape), i.key_pressed(Key::Enter)));
        if esc {
            return ExplorerResult::Cancelled;
        }
        let mut open = true;
        let screen = ctx.content_rect();
        let size = Vec2::new(widgets::modal_width(ctx, 820.0), (screen.height() - 80.0).clamp(320.0, 560.0));
        egui::Window::new(lang.t("Icon-Explorer", "Icon explorer"))
            .id(id)
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .default_size(size)
            .min_size(Vec2::new(480.0, 320.0))
            .pivot(Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .show(ctx, |ui| {
                result = self.body(ui, id, lang, enter);
            });
        if !open {
            return ExplorerResult::Cancelled;
        }
        result
    }

    fn body(&mut self, ui: &mut Ui, id: Id, lang: Lang, enter: bool) -> ExplorerResult {
        let mut result = ExplorerResult::Open;
        self.hovered = None;
        egui::TopBottomPanel::top(id.with("top")).show_separator_line(false).show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut self.query)
                    .id(id.with("search"))
                    .hint_text(lang.t("Namen und Stichwörter suchen (englisch) …", "Search names and tags …"))
                    .desired_width(ui.available_width());
                let r = ui.add(edit);
                if self.focus_search {
                    r.request_focus();
                    self.focus_search = false;
                }
            });
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
        egui::SidePanel::right(id.with("preview")).resizable(false).exact_width(210.0).show_inside(ui, |ui| self.preview(ui, lang));
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(name) = self.grid(ui, id, &results, lang) {
                result = ExplorerResult::Picked(name);
            }
        });
        // Vorschau folgt der Maus erst im nächsten Bild (Raster wird nach der Vorschau gezeichnet).
        if self.hovered.is_some() {
            ui.ctx().request_repaint();
        }
        result
    }

    fn categories(&mut self, ui: &mut Ui, id: Id, lang: Lang) {
        let mut cats: Vec<(&'static str, String)> = lucide::table().categories.iter().map(|c| (c.key, lucide::category_title(c.key, lang).to_string())).collect();
        cats.sort_by_key(|c| c.1.to_lowercase());
        egui::ScrollArea::vertical().id_salt(id.with("cat_scroll")).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let all = lang.t("Alle", "All");
            if ui.selectable_label(self.category.is_none(), RichText::new(all).strong()).clicked() {
                self.category = None;
            }
            for (key, title) in cats {
                let on = self.category.as_deref() == Some(key);
                if ui.selectable_label(on, title).clicked() {
                    self.category = if on { None } else { Some(key.to_string()) };
                }
            }
        });
    }

    fn preview(&mut self, ui: &mut Ui, lang: Lang) {
        let shown = self.hovered.or(self.selected.as_deref().and_then(|n| lucide::get(n).map(|i| i.name)));
        ui.add_space(S2);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 120.0), Sense::hover());
        ui.painter().rect_filled(rect, 8.0, ui.visuals().faint_bg_color);
        let Some(icon) = shown.and_then(lucide::get) else {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, lang.t("Kein Icon gewählt", "No icon selected"), FontId::proportional(13.0), style::dim(ui));
            return;
        };
        paint_glyph(ui.painter(), icon.ch, rect.center(), 80.0, ui.visuals().strong_text_color());
        ui.add_space(S2);
        ui.label(RichText::new(icon.name).strong().monospace());
        ui.label(RichText::new(icons::lucide_value(icon.name)).small().monospace().color(style::dim(ui)));
        if self.hovered.is_some() && self.selected.as_deref() != Some(icon.name) {
            widgets::hint(ui, lang.t("Vorschau – klicken zum Wählen", "Preview – click to select"));
        }
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

    /// Raster der Treffer (nur sichtbare Zeilen werden gebaut); liefert einen Namen bei Doppelklick.
    fn grid(&mut self, ui: &mut Ui, id: Id, results: &[&'static LucideIcon], lang: Lang) -> Option<String> {
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
        if self.scroll_to_selected {
            self.scroll_to_selected = false;
            if let Some(pos) = self.selected.as_deref().and_then(|s| results.iter().position(|i| i.name == s)) {
                area = area.vertical_scroll_offset(((pos / cols) as f32 * (CELL + GAP) - CELL).max(0.0));
            }
        }
        let text = ui.visuals().text_color();
        let accent = style::accent();
        area.show_rows(ui, CELL, rows, |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::splat(GAP);
            for row in range {
                ui.horizontal(|ui| {
                    for icon in results.iter().skip(row * cols).take(cols) {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::hover());
                        let resp = ui.interact(rect, Self::cell_id(id, icon.name), Sense::click());
                        let sel = self.selected.as_deref() == Some(icon.name);
                        if sel {
                            ui.painter().rect_filled(rect, 6.0, accent.gamma_multiply(0.25));
                            ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.3_f32, accent), StrokeKind::Inside);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        paint_glyph(ui.painter(), icon.ch, rect.center(), 22.0, if sel { accent } else { text });
                        if resp.hovered() {
                            self.hovered = Some(icon.name);
                        }
                        let resp = resp.on_hover_text(icon.name);
                        if resp.clicked() {
                            self.selected = Some(icon.name.to_string());
                        }
                        if resp.double_clicked() {
                            picked = Some(icon.name.to_string());
                        }
                    }
                });
            }
        });
        picked
    }
}

/// Öffnet/zeigt den Explorer eines Pickers (Zustand in `egui::Memory::data` unter `id`). `Some(name)` = gewählt.
pub(super) fn picker_explorer(ctx: &egui::Context, id: Id, lang: Lang) -> Option<String> {
    let mut st: IconExplorer = ctx.data_mut(|d| d.get_temp(id))?;
    match st.show(ctx, id, lang) {
        ExplorerResult::Open => {
            ctx.data_mut(|d| d.insert_temp(id, st));
            None
        }
        ExplorerResult::Picked(name) => {
            ctx.data_mut(|d| d.remove::<IconExplorer>(id));
            Some(name)
        }
        ExplorerResult::Cancelled => {
            ctx.data_mut(|d| d.remove::<IconExplorer>(id));
            None
        }
    }
}

/// Öffnet den Explorer eines Pickers (vorgewählt: `current`, falls ein bekanntes Lucide-Icon).
pub(super) fn open_picker_explorer(ctx: &egui::Context, id: Id, current: Option<&str>) {
    ctx.data_mut(|d| d.insert_temp(id, IconExplorer::new(current)));
}

/// Ist der Explorer dieses Pickers offen?
pub fn explorer_open(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<IconExplorer>(id).is_some())
}
