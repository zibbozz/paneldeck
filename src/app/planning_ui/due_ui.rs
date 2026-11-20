//! Fälligkeit in der Oberfläche (Expertenmodus und einfache Todo-Liste): Markierung zeichnen (Rahmen, Symbol, Farbe), die
//! Plakette mit Tooltip („due in 3 days“), das Eingabefeld der Fälligkeit und der Verwalter der Schwellen (anlegen, ändern,
//! löschen, umsortieren), der in den Projekt-Einstellungen und in den Einstellungen der Todo-Liste gleich aussieht.
//!
//! Zeichnen: Der Rahmen ist sehr hell gefüllt (Farbe stark zu Weiß gemischt) und dunkler umrandet; das Symbol steht in der
//! gewählten Farbe, wird aber so weit abgedunkelt, dass es sich von der Füllung abhebt (gelb auf hellgelb). Ohne Rahmen wird das
//! Symbol so angepasst, dass es auf dem Hintergrund des Themes lesbar ist. Die reinen Rechnungen (Farben, Eckpunkte) sind
//! ohne Fenster testbar.

use super::widgets;
use crate::due::{self, DueMarker, DueState, DueThreshold, DurationUnit, MarkerFrame};
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2};
use eframe::egui::{self, Color32, Id, Painter, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, Vec2};

// ------------------------------------------------------------------ Farben und Formen (ohne Fenster testbar)

fn hex_rgb(hex: &str) -> [u8; 3] {
    let (r, g, b) = crate::tasks::rgb(hex);
    [r, g, b]
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

fn lum(c: [u8; 3]) -> f32 {
    let ch = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
}

/// Kontrastverhältnis nach WCAG (1 bis 21).
pub(crate) fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Mindestkontrast des Symbols zu seinem Untergrund.
pub(crate) const MIN_SYMBOL_CONTRAST: f32 = 3.0;

/// Farbe so weit zu `toward` (schwarz oder weiß) schieben, bis sie sich von `bg` abhebt.
fn readable_on(c: [u8; 3], bg: [u8; 3], toward: [u8; 3]) -> [u8; 3] {
    let mut out = c;
    let mut t = 0.0;
    while contrast(out, bg) < MIN_SYMBOL_CONTRAST && t < 1.0 {
        t += 0.05;
        out = mix(c, toward, t);
    }
    out
}

/// Die drei Farben einer Markierung.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MarkerColors {
    /// Füllung des Rahmens (sehr hell).
    pub fill: [u8; 3],
    /// Umriss des Rahmens (dunkler als die Farbe).
    pub stroke: [u8; 3],
    /// Symbol (die Farbe, nötigenfalls für den Kontrast angepasst).
    pub symbol: [u8; 3],
}

/// Farben einer Markierung für das Theme (`dark`) und den Hintergrund der Fläche, auf der sie steht.
pub(crate) fn marker_colors(hex: &str, frame: MarkerFrame, dark: bool, bg: [u8; 3]) -> MarkerColors {
    let c = hex_rgb(hex);
    let fill = mix(c, [255, 255, 255], 0.82);
    let stroke = mix(c, [0, 0, 0], 0.35);
    let symbol = if frame == MarkerFrame::None {
        readable_on(c, bg, if dark { [255, 255, 255] } else { [0, 0, 0] })
    } else {
        readable_on(c, fill, [0, 0, 0])
    };
    MarkerColors { fill, stroke, symbol }
}

/// Eckpunkte des Rahmens im Quadrat `rect` (Kreis: `None`, kein Rahmen: leer). Spitzen zeigen nach oben bzw. unten.
pub(crate) fn frame_points(frame: MarkerFrame, rect: Rect) -> Option<Vec<Pos2>> {
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0;
    let poly = |n: usize, start: f32, rr: f32| -> Vec<Pos2> { (0..n).map(|i| start + i as f32 * std::f32::consts::TAU / n as f32).map(|a| Pos2::new(c.x + rr * a.cos(), c.y + rr * a.sin())).collect() };
    let up = -std::f32::consts::FRAC_PI_2;
    match frame {
        MarkerFrame::None => Some(Vec::new()),
        MarkerFrame::Circle => None,
        MarkerFrame::Triangle => Some(vec![Pos2::new(c.x, rect.top()), Pos2::new(rect.right(), rect.bottom()), Pos2::new(rect.left(), rect.bottom())]),
        MarkerFrame::InvertedTriangle => Some(vec![Pos2::new(rect.left(), rect.top()), Pos2::new(rect.right(), rect.top()), Pos2::new(c.x, rect.bottom())]),
        MarkerFrame::Square => {
            let s = rect.shrink(r * 0.08);
            Some(vec![s.left_top(), s.right_top(), s.right_bottom(), s.left_bottom()])
        }
        MarkerFrame::Hexagon => Some(poly(6, up, r)),
        MarkerFrame::Octagon => Some(poly(8, up + std::f32::consts::PI / 8.0, r / (std::f32::consts::PI / 8.0).cos().max(0.01) * 0.94)),
    }
}

/// Mittelpunkt und Größe des Symbols im Rahmen (im Dreieck tiefer und kleiner, damit es hineinpasst).
pub(crate) fn symbol_place(frame: MarkerFrame, rect: Rect) -> (Pos2, f32) {
    let s = rect.width().min(rect.height());
    match frame {
        MarkerFrame::None => (rect.center(), s),
        MarkerFrame::Triangle => (Pos2::new(rect.center().x, rect.top() + s * 0.62), s * 0.5),
        MarkerFrame::InvertedTriangle => (Pos2::new(rect.center().x, rect.top() + s * 0.36), s * 0.5),
        _ => (rect.center(), s * 0.62),
    }
}

fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Hintergrundfarbe der Fläche als RGB.
fn bg_of(ui: &Ui) -> [u8; 3] {
    let c = ui.visuals().panel_fill;
    [c.r(), c.g(), c.b()]
}

/// Zeichnet eine Markierung in `rect` (quadratisch empfohlen).
pub(crate) fn paint_marker(p: &Painter, rect: Rect, m: &DueMarker, dark: bool, bg: [u8; 3]) {
    let col = marker_colors(&m.color, m.frame, dark, bg);
    let stroke = Stroke::new((rect.width() / 14.0).clamp(1.0, 2.0), c32(col.stroke));
    match frame_points(m.frame, rect) {
        None => {
            p.circle(rect.center(), rect.width().min(rect.height()) / 2.0 - stroke.width / 2.0, c32(col.fill), stroke);
        }
        Some(pts) if !pts.is_empty() => {
            p.add(egui::Shape::convex_polygon(pts, c32(col.fill), stroke));
        }
        Some(_) => {}
    }
    if let Some(sym) = &m.symbol {
        let (c, size) = symbol_place(m.frame, rect);
        widgets::type_icon(p, sym, c, size, c32(col.symbol));
    }
}

/// Text der Plakette: Tooltip mit Name der Markierung, falls vorhanden.
pub(crate) fn tooltip_text(st: &DueState, lang: Lang) -> String {
    let t = due::tooltip(st.minutes_left, lang);
    match st.threshold.as_ref().map(|t| t.marker.name.as_str()).filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} – {t}"),
        None => t,
    }
}

/// Plakette einer Fälligkeit (Markierung mit Tooltip). Nichts, wenn keine Markierung gilt.
pub(crate) fn badge(ui: &mut Ui, st: &DueState, size: f32, lang: Lang) -> Option<Response> {
    let m = st.marker()?;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_marker(ui.painter(), rect, m, ui.visuals().dark_mode, bg_of(ui));
    Some(resp.on_hover_text(tooltip_text(st, lang)))
}

/// Tönung einer Zeile/Karte (sehr dezent) als Farbe, oder `None`.
pub(crate) fn tint_color(st: Option<&DueState>, dark: bool) -> Option<Color32> {
    let hex = st?.tint()?;
    let [r, g, b] = hex_rgb(hex);
    Some(Color32::from_rgba_unmultiplied(r, g, b, if dark { 34 } else { 40 }))
}

/// Füllung mit der Tönung mischen (deckend, damit sich der Hintergrund nicht durchmischt).
pub(crate) fn tinted(fill: Color32, tint: Color32) -> Color32 {
    let a = tint.a() as f32 / 255.0;
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * a * 1.2).round().clamp(0.0, 255.0) as u8;
    let t = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 255);
    let t = [t.r(), t.g(), t.b()];
    Color32::from_rgb(m(fill.r(), t[0]), m(fill.g(), t[1]), m(fill.b(), t[2]))
}

/// Zeile „Fällig: 20.11.2026 ⚠“ (Datum gedämpft, rot wenn überfällig) mit Plakette und Tooltip.
pub(crate) fn due_line(ui: &mut Ui, due_text: &str, st: Option<&DueState>, lang: Lang) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if let Some(st) = st {
            badge(ui, st, 14.0, lang);
        }
        let overdue = st.is_some_and(|s| s.overdue());
        let color = if overdue { crate::style::ui().error } else { style::dim(ui) };
        let r = ui.label(RichText::new(due::display(due_text, lang)).small().color(color));
        if let Some(st) = st {
            r.on_hover_text(tooltip_text(st, lang));
        }
    });
}

// ------------------------------------------------------------------ Eingabe

/// Eingabefeld der Fälligkeit (Datum, optional Uhrzeit). `buf` ist der Eingabetext; liefert `true` bei Änderung. Ein Fehler steht
/// rot daneben, solange der Text unlesbar ist.
pub(crate) fn due_input(ui: &mut Ui, id: Id, buf: &mut String, lang: Lang) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(buf).id(id).hint_text(lang.t("JJJJ-MM-TT [HH:MM]", "YYYY-MM-DD [HH:MM]")).desired_width(150.0).margin(egui::Margin::symmetric(8, 4)));
        changed |= r.changed();
        if !buf.trim().is_empty() && style::icon_btn_small(ui, SIcon::Close, lang.t("Fälligkeit entfernen", "Remove due date")).clicked() {
            buf.clear();
            changed = true;
        }
        if due::parse_input(buf).is_err() {
            ui.label(RichText::new(lang.t("z. B. 2026-11-20 oder 20.11.2026 14:30", "e.g. 2026-11-20 or 2026-11-20 14:30")).small().color(crate::style::ui().error));
        }
    });
    changed
}

// ------------------------------------------------------------------ Verwalter der Schwellen

/// Arbeitskopie des Verwalters (Projekt oder Todo-Liste).
#[derive(Default)]
pub(crate) struct Manager {
    pub loaded_for: Option<i64>,
    /// Arbeitskopie, so wie sie bearbeitet wird (Namen mit Leerzeichen beim Tippen, ungültige Zwischenstände).
    pub list: Vec<DueThreshold>,
    /// Zuletzt gespeicherte, bereinigte Liste: gespeichert wird nur, wenn sich die bereinigte Arbeitskopie davon unterscheidet.
    pub saved: Vec<DueThreshold>,
    pub err: Option<String>,
}

impl Manager {
    pub fn new(loaded_for: i64, list: Vec<DueThreshold>) -> Manager {
        Manager { loaded_for: Some(loaded_for), saved: list.clone(), list, err: None }
    }
}

/// Was der Verwalter in diesem Bild getan hat.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ManagerOut {
    None,
    /// Die Liste hat sich geändert und ist gültig: speichern.
    Save(Vec<DueThreshold>),
    /// „Standard wiederherstellen“.
    Defaults,
}

/// Symbol-Auswahl einer Markierung (keins oder ein Icon). Hinter dieser einen Funktion, damit später weitere Icon-Sätze
/// (z. B. `lucide:<name>`) dazukommen können; gespeichert wird nur der Name.
pub(crate) fn marker_symbol_picker(ui: &mut Ui, salt: impl std::hash::Hash, current: &mut Option<String>, color: Color32, lang: Lang) -> bool {
    widgets::icon_picker(ui, salt, current, color, lang)
}

/// Farbauswahl einer Markierung: Farbfeld, das die Palette öffnet (Expertenmodus bzw. mit `todo` die der Todo-Liste) samt
/// „Eigene Farbe …“.
pub(crate) fn marker_color_picker(ui: &mut Ui, salt: impl std::hash::Hash, hex: &mut String, todo: bool, lang: Lang) -> bool {
    use crate::app::palette_ui::{self, Look, PickOpts};
    use crate::palette::Area;
    let salt = Id::new(salt);
    let resp = widgets::swatch(ui, salt.with("sw"), super::helpers::color_of(hex), 16.0).on_hover_text(lang.t("Farbe ändern", "Change color"));
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let (area, opts) = if todo { (Area::Todo, PickOpts::TODO) } else { (Area::Expert, PickOpts { look: Look::Circle, per_row: 7, custom: true }) };
        if let Some(h) = palette_ui::pick(ui, salt.with("pick"), area, Some(hex.as_str()), opts, lang) {
            if !h.eq_ignore_ascii_case(hex) {
                *hex = h;
                changed = true;
            }
        }
    });
    changed
}

/// Anzeige einer Dauer als (Anzahl, Einheit): die gemerkte Einheit `remembered` (zuletzt gewählt, zur Dauer `minutes`), solange
/// sie noch zur Dauer gehört, sonst die größte glatte Einheit ([`due::split_duration`]).
pub(crate) fn duration_parts(minutes: i64, remembered: Option<(i64, DurationUnit)>) -> (i64, DurationUnit) {
    match remembered {
        Some((m, u)) if m == minutes && minutes > 0 && minutes % u.minutes() == 0 => (minutes / u.minutes(), u),
        _ => due::split_duration(minutes),
    }
}

/// Salt des Symbol-Pickers im Markierungs-Editor `id` (damit der Verwalter weiß, ob dessen Icon-Explorer offen ist).
fn marker_symbol_salt(id: Id) -> Id {
    id.with("sym")
}

/// Kleines Vorschaubild einer Markierung als Knopf (öffnet die Bearbeitung).
fn marker_preview(ui: &mut Ui, id: Id, m: &DueMarker, size: f32) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    if m.is_none() {
        ui.painter().circle_stroke(rect.center(), size * 0.3, Stroke::new(1.0_f32, style::dim(ui)));
    } else {
        paint_marker(ui.painter(), rect, m, ui.visuals().dark_mode, bg_of(ui));
    }
    resp
}

/// Der Verwalter: Liste der Schwellen mit Dauer, Markierung (Rahmen, Symbol, Farbe, Name) und „Zeile/Karte tönen“;
/// hinzufügen, löschen, nach oben/unten. Jede gültige Änderung wird sofort als [`ManagerOut::Save`] gemeldet.
pub(crate) fn manager_ui(ui: &mut Ui, salt: &str, m: &mut Manager, todo: bool, lang: Lang) -> ManagerOut {
    let mut out = ManagerOut::None;
    let mut changed = false;
    let dim = style::dim(ui);
    ui.label(RichText::new(lang.t(
        "Je Schwelle: Bleibt bis zur Fälligkeit weniger als die Dauer, zeigt die Karte die Markierung. Es gilt die kleinste passende Schwelle; überfällige Einträge liegen unter jeder Schwelle. Ein reines Datum zählt bis zum Ende des Tages.",
        "Per threshold: when less than the duration is left until the due date, the card shows the marker. The smallest matching threshold wins; overdue items are below every threshold. A date without time counts until the end of that day.",
    )).small().color(dim));
    ui.add_space(S1);
    if m.list.is_empty() {
        ui.label(RichText::new(lang.t("Keine Schwellen – Fälligkeiten werden nicht markiert.", "No thresholds – due dates are not marked.")).small().color(dim));
    }
    let mut edits: Vec<Edit> = Vec::new();
    let n = m.list.len();
    for i in 0..n {
        let id = Id::new((salt, "due_row", i));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(lang.t("unter", "under"));
            let t = &mut m.list[i];
            // Die zuletzt gewählte Einheit bleibt, solange sie zur Dauer passt – sonst springt sie beim Ziehen um (7 Tage → 1 Woche).
            let unit_mem = id.with("unit_mem");
            let remembered: Option<(i64, DurationUnit)> = ui.data(|d| d.get_temp(unit_mem));
            let (mut num, mut unit) = duration_parts(t.under_minutes, remembered);
            let r = ui.add(egui::DragValue::new(&mut num).range(1..=9999).speed(0.2));
            let mut unit_changed = false;
            egui::ComboBox::from_id_salt(id.with("unit")).selected_text(unit.label(num, lang)).width(80.0).show_ui(ui, |ui| {
                for u in DurationUnit::ALL {
                    unit_changed |= ui.selectable_value(&mut unit, u, u.label(num, lang)).changed();
                }
            });
            if r.changed() || unit_changed {
                t.under_minutes = num * unit.minutes();
                ui.data_mut(|d| d.insert_temp(unit_mem, (t.under_minutes, unit)));
                changed = true;
            }
            ui.label("→");
            let pv = marker_preview(ui, id.with("preview"), &t.marker, 20.0).on_hover_text(lang.t("Markierung bearbeiten", "Edit marker"));
            // Solange der Icon-Explorer des Symbol-Pickers offen ist, darf ein Klick in sein Fenster (für das Popup „außerhalb“)
            // die Bearbeitung nicht schließen – sonst verschwände mit ihr auch der Explorer (#215).
            let explorer_open = widgets::icon_picker_explorer_open(ui.ctx(), marker_symbol_salt(id.with("ed")));
            let close = if explorer_open { egui::PopupCloseBehavior::IgnoreClicks } else { egui::PopupCloseBehavior::CloseOnClickOutside };
            egui::Popup::from_toggle_button_response(&pv).close_behavior(close).show(|ui| {
                changed |= marker_editor(ui, id.with("ed"), &mut t.marker, todo, lang);
            });
            egui::ComboBox::from_id_salt(id.with("frame")).selected_text(t.marker.frame.label(lang)).width(140.0).show_ui(ui, |ui| {
                for f in MarkerFrame::ALL {
                    changed |= ui.selectable_value(&mut t.marker.frame, f, f.label(lang)).changed();
                }
            });
            changed |= marker_color_picker(ui, id.with("color"), &mut t.marker.color, todo, lang);
            let mut tint = t.tint;
            if ui.checkbox(&mut tint, lang.t("Zeile/Karte tönen", "Tint row/card")).changed() {
                edits.push(Edit::Tint(i, tint));
            }
            if !t.marker.name.is_empty() {
                ui.label(RichText::new(&t.marker.name).small().color(dim));
            }
            if i > 0 && style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                edits.push(Edit::Swap(i, i - 1));
            }
            if i + 1 < n && style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                edits.push(Edit::Swap(i, i + 1));
            }
            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Schwelle löschen", "Delete threshold")).clicked() {
                edits.push(Edit::Remove(i));
            }
        });
    }
    ui.add_space(S1);
    ui.horizontal(|ui| {
        if ui.add_enabled(m.list.len() < due::MAX_THRESHOLDS, egui::Button::new(lang.t("+ Schwelle", "+ Threshold"))).clicked() {
            edits.push(Edit::Add);
        }
        if ui.button(lang.t("Standard wiederherstellen", "Restore defaults")).on_hover_text(lang.t("Gelbes Warndreieck unter 1 Woche, rotes unter 1 Tag", "Yellow warning triangle under 1 week, red under 1 day")).clicked() {
            edits.push(Edit::Defaults);
        }
    });
    if changed {
        edits.push(Edit::Touched);
    }
    for e in edits {
        let o = apply(m, e, lang);
        if o != ManagerOut::None {
            out = o;
        }
    }
    if let Some(e) = &m.err {
        ui.add_space(S1);
        widgets::error(ui, e);
    }
    out
}

/// Eine Änderung im Verwalter (aus den Bedienelementen; die Felder einer Zeile ändern die Liste direkt und melden `Touched`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Edit {
    Add,
    Remove(usize),
    Swap(usize, usize),
    Tint(usize, bool),
    Defaults,
    /// Die Liste wurde direkt geändert (Dauer, Rahmen, Symbol, Farbe, Name): nur prüfen und speichern.
    Touched,
}

/// Änderung anwenden und prüfen: gültig → [`ManagerOut::Save`], sonst steht der Fehler in `m.err` (gespeichert wird erst, wenn die
/// Liste wieder gültig ist).
pub(crate) fn apply(m: &mut Manager, e: Edit, lang: Lang) -> ManagerOut {
    match e {
        Edit::Add if m.list.len() < due::MAX_THRESHOLDS => m.list.push(new_threshold(&m.list)),
        Edit::Add => return ManagerOut::None,
        Edit::Remove(i) if i < m.list.len() => {
            m.list.remove(i);
        }
        Edit::Swap(a, b) if a < m.list.len() && b < m.list.len() => m.list.swap(a, b),
        Edit::Tint(i, on) if i < m.list.len() => m.list[i].tint = on,
        Edit::Defaults => {
            m.list = due::default_thresholds();
            m.saved = m.list.clone();
            m.err = None;
            return ManagerOut::Defaults;
        }
        Edit::Touched => {}
        _ => return ManagerOut::None,
    }
    match due::validate_thresholds(&m.list) {
        Ok(list) if list == m.saved => {
            m.err = None;
            ManagerOut::None
        }
        Ok(list) => {
            m.err = None;
            m.saved = list.clone();
            ManagerOut::Save(list)
        }
        Err(p) => {
            m.err = Some(p.message(lang).to_string());
            ManagerOut::None
        }
    }
}

/// Neue Schwelle: halbe Dauer der kleinsten vorhandenen (mindestens 1 Stunde), sonst 3 Tage; orangefarbenes Dreieck.
pub(crate) fn new_threshold(list: &[DueThreshold]) -> DueThreshold {
    let mut minutes = list.iter().map(|t| t.under_minutes).min().map(|m| (m / 2).max(due::MINUTES_PER_HOUR)).unwrap_or(3 * due::MINUTES_PER_DAY);
    while list.iter().any(|t| t.under_minutes == minutes) {
        minutes += due::MINUTES_PER_HOUR;
    }
    DueThreshold { under_minutes: minutes, marker: DueMarker::warning("#E5A04B"), tint: false }
}

/// Bearbeitung einer Markierung im Popup: Vorschau, Rahmen, Symbol, Farbe, Name.
fn marker_editor(ui: &mut Ui, id: Id, mk: &mut DueMarker, todo: bool, lang: Lang) -> bool {
    let mut changed = false;
    ui.set_max_width(400.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
        paint_marker(ui.painter(), rect, mk, ui.visuals().dark_mode, bg_of(ui));
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(lang.t("Name:", "Name:"));
                changed |= ui.add(egui::TextEdit::singleline(&mut mk.name).char_limit(due::MAX_NAME_CHARS).desired_width(180.0).hint_text(lang.t("optional", "optional"))).changed();
            });
            ui.horizontal(|ui| {
                ui.label(lang.t("Farbe:", "Color:"));
                changed |= marker_color_picker(ui, id.with("c"), &mut mk.color, todo, lang);
            });
        });
    });
    ui.add_space(S1);
    ui.label(RichText::new(lang.t("Rahmen", "Frame")).strong());
    ui.horizontal_wrapped(|ui| {
        for f in MarkerFrame::ALL {
            let sample = DueMarker { frame: f, symbol: None, color: mk.color.clone(), name: String::new() };
            let r = marker_preview(ui, id.with(("frame", f.as_str())), &sample, 24.0).on_hover_text(f.label(lang));
            if mk.frame == f {
                ui.painter().rect_stroke(r.rect.expand(2.0), 4.0, Stroke::new(1.5_f32, style::accent()), egui::StrokeKind::Outside);
            }
            if r.clicked() && mk.frame != f {
                mk.frame = f;
                changed = true;
            }
        }
    });
    ui.add_space(S1);
    ui.label(RichText::new(lang.t("Symbol", "Symbol")).strong());
    changed |= marker_symbol_picker(ui, marker_symbol_salt(id), &mut mk.symbol, super::helpers::color_of(&mk.color), lang);
    ui.add_space(S2);
    changed
}

// ------------------------------------------------------------------ Zählung für die Übersicht

/// (überfällig, markiert) je Projekt (Kennung 0 = einfache Todo-Liste), höchstens einmal je halbe Minute neu gezählt.
#[derive(Clone, Default)]
pub(crate) struct Counts {
    map: std::collections::HashMap<i64, (usize, usize)>,
    at: Option<std::time::Instant>,
}

impl Counts {
    pub fn get(&self, project: i64) -> (usize, usize) {
        self.map.get(&project).copied().unwrap_or_default()
    }

    /// Zählung der einfachen Todo-Liste von außen setzen (sie lädt ihre Karten selbst).
    pub fn set(&mut self, project: i64, c: (usize, usize)) {
        self.map.insert(project, c);
    }

    /// Beim nächsten Bild neu zählen (nach Änderungen).
    pub fn invalidate(&mut self) {
        self.at = None;
    }

    pub fn refresh(&mut self, conn: &rusqlite::Connection, projects: &[crate::planning::projects::Project]) {
        if self.at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30)) {
            return;
        }
        self.at = Some(std::time::Instant::now());
        let now = chrono::Local::now().naive_local();
        self.map.clear();
        if let Ok(c) = crate::tasks::due_counts(conn, now) {
            self.map.insert(0, c);
        }
        for p in projects.iter().filter(|p| !p.system) {
            if let Ok(c) = crate::planning::builtin_fields::due_counts(conn, p.id, now) {
                self.map.insert(p.id, c);
            }
        }
    }
}

/// „3 überfällig“ (rot) bzw. „2 markiert“ (gedämpft), sonst nichts.
pub(crate) fn count_label(ui: &mut Ui, (overdue, marked): (usize, usize), lang: Lang) {
    let text = due::count_text(overdue, marked, lang);
    if text.is_empty() {
        return;
    }
    let color = if overdue > 0 { crate::style::ui().error } else { style::dim(ui) };
    let tip = format!("{overdue} {} · {marked} {}", lang.t("überfällig", "overdue"), lang.t("markiert (nach den Schwellen der Fälligkeit)", "marked (by the due-date thresholds)"));
    ui.label(RichText::new(text).small().color(color)).on_hover_text(tip);
}

// ------------------------------------------------------------------ Reiter „Fälligkeit“ des Projekts

impl super::PlanUi {
    /// Zählung der einfachen Todo-Liste für den Kopf (vom Reiter der Liste nach dem Laden).
    pub(crate) fn set_todo_due_counts(&mut self, c: (usize, usize)) {
        self.due_counts.set(0, c);
    }

    pub(super) fn due_tab(&mut self, ui: &mut Ui, cx: &super::Cx, proj: &crate::planning::projects::Project) {
        let lang = cx.lang;
        if self.due_mgr.loaded_for != Some(proj.id) {
            self.due_mgr = Manager::new(proj.id, crate::planning::projects::due_thresholds(cx.conn, proj.id).unwrap_or_default());
        }
        widgets::hint(ui, lang.t(
            "Jedes Ticket hat eine optionale Fälligkeit (Datum, optional mit Uhrzeit). Hier legst du fest, ab wann Karten, Listen und Boards eine Markierung zeigen.",
            "Every ticket has an optional due date (a date, optionally with a time). Here you decide from when cards, lists and boards show a marker.",
        ));
        ui.add_space(S2);
        let save = match manager_ui(ui, &format!("proj{}", proj.id), &mut self.due_mgr, false, lang) {
            ManagerOut::None => None,
            ManagerOut::Save(list) => Some(list),
            ManagerOut::Defaults => Some(due::default_thresholds()),
        };
        if let Some(list) = save {
            // die Arbeitskopie bleibt, wie sie getippt wurde; nur die bereinigte Liste geht in die Datenbank
            if self.run(cx, crate::planning::projects::set_due_thresholds(cx.conn, proj.id, &list)).is_some() {
                self.due_counts.invalidate();
            } else {
                self.due_mgr.saved.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::due::{DueMarker, MarkerFrame};

    const LIGHT_BG: [u8; 3] = [248, 248, 248];
    const DARK_BG: [u8; 3] = [30, 30, 30];

    #[test]
    fn marker_colors_are_readable_in_both_themes() {
        for hex in ["#D8B94A", "#E06C75", "#58A6E6", "#FFFF00", "#FFFFFF", "#000000", "#7FBF6A"] {
            let c = hex_rgb(hex);
            for frame in MarkerFrame::ALL {
                for (dark, bg) in [(false, LIGHT_BG), (true, DARK_BG)] {
                    let mc = marker_colors(hex, frame, dark, bg);
                    let under = if frame == MarkerFrame::None { bg } else { mc.fill };
                    assert!(contrast(mc.symbol, under) >= MIN_SYMBOL_CONTRAST - 0.01, "{hex} {frame:?} dark={dark}: Symbol hebt sich nicht ab");
                    // Füllung sehr hell, Umriss dunkler als die Farbe
                    assert!(lum(mc.fill) > 0.6, "{hex}: Füllung zu dunkel");
                    assert!(lum(mc.stroke) <= lum(c) + 1e-6, "{hex}: Umriss nicht dunkler");
                }
            }
        }
        // eine kräftige Farbe bleibt als Symbol unverändert, gelb auf hellgelb wird abgedunkelt
        assert_eq!(marker_colors("#2050C0", MarkerFrame::Square, false, LIGHT_BG).symbol, hex_rgb("#2050C0"));
        assert_ne!(marker_colors("#D8B94A", MarkerFrame::Triangle, false, LIGHT_BG).symbol, hex_rgb("#D8B94A"));
    }

    #[test]
    fn the_chosen_duration_unit_stays_while_dragging() {
        let d = DurationUnit::Days;
        // ohne gemerkte Einheit: größte glatte Einheit
        assert_eq!(duration_parts(7 * 1440, None), (1, DurationUnit::Weeks));
        // gezogen von 6 auf 7 Tage: bleibt „7 Tage“ statt „1 Woche“ (sonst springt der Wert beim Weiterziehen auf Wochen)
        assert_eq!(duration_parts(7 * 1440, Some((7 * 1440, d))), (7, d));
        assert_eq!(duration_parts(14 * 1440, Some((14 * 1440, d))), (14, d));
        // gemerkt für eine andere Dauer (Zeile gelöscht/verschoben, Standard wiederhergestellt): wie ohne
        assert_eq!(duration_parts(10080, Some((60, DurationUnit::Minutes))), (1, DurationUnit::Weeks));
        // gemerkte Einheit passt nicht mehr glatt: größte glatte Einheit
        assert_eq!(duration_parts(90, Some((90, DurationUnit::Hours))), (90, DurationUnit::Minutes));
    }

    #[test]
    fn every_frame_has_a_shape_inside_its_square() {
        let r = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::splat(16.0));
        assert_eq!(frame_points(MarkerFrame::None, r), Some(Vec::new()));
        assert_eq!(frame_points(MarkerFrame::Circle, r), None);
        for (f, n) in [(MarkerFrame::Triangle, 3), (MarkerFrame::InvertedTriangle, 3), (MarkerFrame::Square, 4), (MarkerFrame::Hexagon, 6), (MarkerFrame::Octagon, 8)] {
            let pts = frame_points(f, r).unwrap();
            assert_eq!(pts.len(), n, "{f:?}");
            assert!(pts.iter().all(|p| r.expand(0.01).contains(*p)), "{f:?} ragt heraus: {pts:?}");
        }
        // Dreieck: Spitze oben, umgedreht: Spitze unten; das Symbol sitzt im breiten Teil
        let t = frame_points(MarkerFrame::Triangle, r).unwrap();
        assert_eq!(t[0].y, r.top());
        let it = frame_points(MarkerFrame::InvertedTriangle, r).unwrap();
        assert_eq!(it[2].y, r.bottom());
        assert!(symbol_place(MarkerFrame::Triangle, r).0.y > r.center().y);
        assert!(symbol_place(MarkerFrame::InvertedTriangle, r).0.y < r.center().y);
    }

    #[test]
    fn paints_every_marker_combination_without_a_window() {
        let ctx = egui::Context::default();
        let mut shapes = 0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for f in MarkerFrame::ALL {
                    for sym in [None, Some("alert".to_string()), Some("bug".to_string())] {
                        let m = DueMarker { frame: f, symbol: sym.clone(), color: "#D8B94A".into(), name: String::new() };
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_marker(ui.painter(), rect, &m, ui.visuals().dark_mode, bg_of(ui));
                        shapes += usize::from(!m.is_none());
                    }
                }
            });
        });
        assert_eq!(shapes, 6 * 3 + 2);
    }

    #[test]
    fn badge_shows_only_with_a_marker_and_has_the_tooltip_text() {
        let now = chrono::NaiveDateTime::parse_from_str("2026-11-17T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap();
        let yellow = due::evaluate(&due::default_thresholds(), "2026-11-20", now).unwrap();
        let far = due::evaluate(&due::default_thresholds(), "2027-11-20", now).unwrap();
        assert_eq!(tooltip_text(&yellow, Lang::En), "due in 4 days");
        let mut named = due::default_thresholds();
        named[0].marker.name = "Bald".into();
        named[0].tint = true;
        let st = due::evaluate(&named, "2026-11-20", now).unwrap();
        assert_eq!(tooltip_text(&st, Lang::De), "Bald – fällig in 4 Tagen");
        assert!(tint_color(Some(&st), true).is_some() && tint_color(Some(&yellow), true).is_none() && tint_color(None, false).is_none());
        let ctx = egui::Context::default();
        let (mut a, mut b) = (false, true);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                a = badge(ui, &yellow, 14.0, Lang::En).is_some();
                b = badge(ui, &far, 14.0, Lang::En).is_some();
            });
        });
        assert!(a && !b, "Plakette nur mit Markierung");
    }

    #[test]
    fn the_manager_adds_reorders_validates_and_saves() {
        let ctx = egui::Context::default();
        let mut m = Manager::new(1, due::default_thresholds());
        // ohne Eingabe: nichts zu speichern
        let mut out = ManagerOut::None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| out = manager_ui(ui, "t", &mut m, false, Lang::En));
        });
        assert_eq!(out, ManagerOut::None);
        // neue Schwelle: kleiner als die kleinste, gültig
        let n = new_threshold(&m.list);
        assert_eq!(n.under_minutes, 12 * 60);
        assert!(!n.tint, "Tönung ist anfangs aus");
        m.list.push(n);
        assert!(due::validate_thresholds(&m.list).is_ok());
        assert_eq!(new_threshold(&[]).under_minutes, 3 * due::MINUTES_PER_DAY);
        let clash = vec![DueThreshold { under_minutes: 120, marker: DueMarker::default(), tint: false }, DueThreshold { under_minutes: 60, marker: DueMarker::default(), tint: false }];
        assert!(!clash.iter().any(|t| t.under_minutes == new_threshold(&clash).under_minutes));
        // Änderungen: hinzufügen, Tönung an, umsortieren, löschen, Standard
        let mut m = Manager::new(1, due::default_thresholds());
        // unverändert: nichts speichern; Leerzeichen am Namensende (beim Tippen) bleiben in der Arbeitskopie und speichern nicht
        assert_eq!(apply(&mut m, Edit::Touched, Lang::En), ManagerOut::None);
        m.list[0].marker.name = "Bald ".into();
        assert!(matches!(apply(&mut m, Edit::Touched, Lang::En), ManagerOut::Save(l) if l[0].marker.name == "Bald"));
        assert_eq!(m.list[0].marker.name, "Bald ", "die Arbeitskopie wird nicht bereinigt");
        m.list[0].marker.name = "Bald  ".into();
        assert_eq!(apply(&mut m, Edit::Touched, Lang::En), ManagerOut::None, "bereinigt gleich: keine weitere Speicherung");
        m.list[0].marker.name = "Bald b".into();
        assert!(matches!(apply(&mut m, Edit::Touched, Lang::En), ManagerOut::Save(l) if l[0].marker.name == "Bald b"));
        m.list[0].marker.name.clear();
        let _ = apply(&mut m, Edit::Touched, Lang::En);
        assert!(matches!(apply(&mut m, Edit::Add, Lang::En), ManagerOut::Save(l) if l.len() == 3));
        match apply(&mut m, Edit::Tint(2, true), Lang::En) {
            ManagerOut::Save(l) => assert!(l[2].tint && !l[0].tint && !l[1].tint, "nur die gewählte Schwelle tönt"),
            o => panic!("{o:?}"),
        }
        assert!(matches!(apply(&mut m, Edit::Swap(0, 2), Lang::En), ManagerOut::Save(l) if l[0].tint && l[2].under_minutes == due::MINUTES_PER_WEEK));
        assert!(matches!(apply(&mut m, Edit::Remove(0), Lang::En), ManagerOut::Save(l) if l.len() == 2));
        // ungültig (gleiche Dauer): nicht speichern, Fehler zeigen; wieder gültig: speichern
        m.list[1].under_minutes = m.list[0].under_minutes;
        assert_eq!(apply(&mut m, Edit::Touched, Lang::De), ManagerOut::None);
        assert!(m.err.as_deref().is_some_and(|e| e.contains("dieselbe Dauer")));
        m.list[1].under_minutes = 5;
        assert!(matches!(apply(&mut m, Edit::Touched, Lang::De), ManagerOut::Save(_)) && m.err.is_none());
        assert_eq!(apply(&mut m, Edit::Remove(9), Lang::En), ManagerOut::None, "falscher Index ändert nichts");
        assert_eq!(apply(&mut m, Edit::Defaults, Lang::En), ManagerOut::Defaults);
        assert_eq!(m.list, due::default_thresholds());
        assert!(m.list.iter().all(|t| !t.tint), "Standard-Schwellen tönen nicht");
        // Verwalter zeichnet in beiden Sprachen, auch mit Fehler
        m.err = Some("x".into());
        for lang in [Lang::En, Lang::De] {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = manager_ui(ui, "t2", &mut m, true, lang);
                });
            });
        }
    }
}
