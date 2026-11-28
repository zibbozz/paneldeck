//! Design-System: Farbflächen (aus dem Terminal-Theme abgeleitet), Abstände, Schriftgrößen,
//! selbst gezeichnete Icons und kleine Bausteine (Icon-Buttons, Pills).
//!
//! Grundsätze: Das Terminal dominiert, die Oberfläche drumherum bleibt ruhig und wird über
//! leichte Helligkeitsstufen statt über Rahmen gegliedert; eine Akzentfarbe; Abstände im 4-px-Raster;
//! wenige Schriftgrößen; ausreichender Kontrast (kein reines Schwarz/Weiß).

use crate::theme::{Theme, UiColors};
use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, FontFamily, FontId, Margin, Painter, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, TextStyle, Ui, Vec2,
};

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::RwLock;

// Motion-Tokens: Hover/Klick ~120 ms, Zustandswechsel ~200 ms, Layout ~280 ms; Kurve: ease-out.
pub const D_FAST: f32 = 0.12;
pub const D_BASE: f32 = 0.20;

static ANIMATIONS: AtomicBool = AtomicBool::new(true);

/// Zeigt das gerade gezeichnete Fenster den eigenen Hintergrund des Themes (#387)? Dann füllen Flächen nur einmal in der
/// Deckkraft ihres Bereichs (keine doppelten Schichten).
static BACKDROP: AtomicBool = AtomicBool::new(false);

pub fn set_backdrop(on: bool) {
    BACKDROP.store(on, Ordering::Relaxed);
}

pub fn backdrop_on() -> bool {
    BACKDROP.load(Ordering::Relaxed)
}

/// Zusatz für Animations-Ids: das gerade gezeichnete Paneldeck-Fenster (0 = Hauptfenster, #110). egui führt Animationen
/// für alle Fenster gemeinsam nach Id; ohne Zusatz kämpften gleiche Knöpfe zweier Fenster um denselben Wert.
static ID_SALT: AtomicU64 = AtomicU64::new(0);

pub fn set_id_salt(window: u64) {
    ID_SALT.store(window, Ordering::Relaxed);
}

/// Dialogfenster wie ein normales Fenster verschiebbar machen (#275, #288): an der Titelleiste ziehen, bleibt im
/// sichtbaren Bereich (`constrain`), öffnet mittig. Solange der Dialog offen ist (in jedem Durchlauf gezeichnet), behält
/// er die gezogene Position (feste `id`, unabhängig vom Titel); nach dem Schließen öffnet er beim nächsten Mal wieder
/// mittig. Drehpunkt ist die Mitte: wächst der Inhalt (z. B. „Weitere Optionen“), wächst das Fenster nach beiden Seiten.
/// Jedes Paneldeck-Fenster ist ein eigener egui-Viewport mit eigenem Speicher – Position und Zähler gelten je Fenster.
pub fn movable<'a>(ctx: &egui::Context, id: egui::Id, w: egui::Window<'a>) -> egui::Window<'a> {
    let center = ctx.content_rect().center();
    let reopened = just_opened(ctx, id.with("pd_movable_pass"));
    let w = w.id(id).pivot(egui::Align2::CENTER_CENTER).default_pos(center).constrain(true);
    if reopened { w.current_pos(center) } else { w }
}

/// Größe eines Fensters an das Hauptfenster binden (wie der Theme-Editor, 2.6.0): Anfangsgröße `default`, aber höchstens
/// so groß wie das Hauptfenster abzüglich eines Randes; die Höchstgröße folgt dem Hauptfenster in jedem Bild (egui merkt
/// sich die Größe und begrenzt nur den Inhalt, der Rahmen samt Titelleiste ragte sonst über den Rand); im Fenster gehalten.
pub fn fit_to_screen<'a>(ctx: &egui::Context, w: egui::Window<'a>, default: [f32; 2], min: [f32; 2]) -> egui::Window<'a> {
    let max = fit_max(ctx.content_rect().size(), min);
    w.default_size([default[0].min(max[0]), default[1].min(max[1])]).min_size(min).max_size(max).constrain(true)
}

/// Höchstgröße des Inhalts eines Fensters im Hauptfenster der Größe `screen` (Rand, Titelleiste und Rahmen abgezogen).
pub fn fit_max(screen: egui::Vec2, min: [f32; 2]) -> [f32; 2] {
    [(screen.x - 40.0).max(min[0]), (screen.y - 80.0).max(min[1])]
}

/// Im vorigen Durchlauf nicht gezeichnet = gerade geöffnet (merkt sich den aktuellen Durchlauf unter `key`).
fn just_opened(ctx: &egui::Context, key: egui::Id) -> bool {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let last = d.get_temp::<u64>(key);
        d.insert_temp(key, pass);
        last.is_none_or(|l| pass > l + 1)
    })
}

/// Höhe der Zieh-Leiste eines Modals unter dem oberen Rand des Rahmens (dazu kommt der Innenabstand des Rahmens).
pub const MODAL_STRIP_H: f32 = 8.0;

/// Verschiebbares Modal (#288): wie [`egui::Modal`] (Hintergrund sperrt den Rest, Esc/Klick daneben über
/// `should_close`, gleicher Rahmen), aber mit einer schmalen Zieh-Leiste oben, an der man es verschieben kann. Öffnet bei
/// jedem Öffnen an seinem Anker (Standard: Mitte), behält die gezogene Position, solange es offen ist, und bleibt immer im
/// Fenster – auch wenn das Fenster kleiner wird. Ersetzt `egui::Modal::new(id).show(ctx, …)` eins zu eins.
pub struct Modal {
    id: egui::Id,
    align: egui::Align2,
    offset: Vec2,
}

/// Verschiebbares Modal mit Anker Mitte, siehe [`Modal`].
pub fn modal(id: egui::Id) -> Modal {
    Modal { id, align: egui::Align2::CENTER_CENTER, offset: Vec2::ZERO }
}

impl Modal {
    /// Anderer Anker als die Mitte (z. B. oben mittig mit Abstand), Ausgangslage beim Öffnen.
    pub fn anchor(mut self, align: egui::Align2, offset: Vec2) -> Self {
        self.align = align;
        self.offset = offset;
        self
    }

    pub fn show<T>(self, ctx: &egui::Context, content: impl FnOnce(&mut Ui) -> T) -> egui::ModalResponse<T> {
        let Modal { id, align, offset } = self;
        let key = id.with("pd_modal_drag");
        let reopened = just_opened(ctx, id.with("pd_modal_pass"));
        let screen = ctx.content_rect();
        let mut drag: Vec2 = if reopened { Vec2::ZERO } else { ctx.data(|d| d.get_temp(key)).unwrap_or_default() };
        // Mit der Größe des letzten Bildes im Fenster halten (auch nach dem Verkleinern des Fensters).
        if let Some(size) = egui::AreaState::load(ctx, id).and_then(|s| s.size) {
            drag = clamp_modal_drag(screen, size, align, offset, drag);
        }
        let frame = egui::Frame::popup(&ctx.style());
        let area = egui::Modal::default_area(id).anchor(align, offset + drag);
        let mut delta = Vec2::ZERO;
        let r = egui::Modal::new(id).area(area).frame(frame).show(ctx, |ui| {
            let top = ui.cursor().top();
            ui.add_space(MODAL_STRIP_H);
            let inner = content(ui);
            let m = frame.inner_margin;
            let c = ui.min_rect();
            let rect = Rect::from_min_max(
                Pos2::new(c.left() - m.left as f32, top - m.top as f32),
                Pos2::new(c.right() + m.right as f32, top + MODAL_STRIP_H),
            );
            let resp = ui.interact(rect, id.with("pd_modal_strip"), Sense::drag());
            paint_modal_strip(ui, rect, &resp, frame.corner_radius);
            if resp.dragged() {
                delta = resp.drag_delta();
            }
            inner
        });
        if delta != Vec2::ZERO {
            drag = clamp_modal_drag(screen, r.response.rect.size(), align, offset, drag + delta);
            ctx.request_repaint();
        }
        ctx.data_mut(|d| d.insert_temp(key, drag));
        r
    }
}

/// Verschiebung so begrenzen, dass ein Modal der Größe `size` ganz im Fenster `screen` liegt (ist es größer als das
/// Fenster, bleibt die linke bzw. obere Kante sichtbar).
fn clamp_modal_drag(screen: Rect, size: Vec2, align: egui::Align2, offset: Vec2, drag: Vec2) -> Vec2 {
    let base = align.align_size_within_rect(size, screen).left_top() + offset;
    let lo = screen.min - base;
    let hi = screen.max - size - base;
    Vec2::new(drag.x.min(hi.x).max(lo.x), drag.y.min(hi.y).max(lo.y))
}

/// Zieh-Leiste: in Ruhe nur ein dezenter Griff, beim Überfahren leicht hinterlegt, Mauszeiger „Greifen“.
fn paint_modal_strip(ui: &Ui, rect: Rect, resp: &Response, radius: CornerRadius) {
    let hot = resp.hovered() || resp.dragged();
    let v = ui.visuals();
    if hot {
        ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        let r = CornerRadius { nw: radius.nw, ne: radius.ne, sw: 0, se: 0 };
        ui.painter().rect_filled(rect.shrink(1.0), r, v.widgets.hovered.weak_bg_fill.gamma_multiply(0.6));
    }
    let grip = Rect::from_center_size(Pos2::new(rect.center().x, rect.center().y + 1.0), Vec2::new(32.0, 4.0));
    let c = v.weak_text_color().gamma_multiply(if hot { 0.9 } else { 0.45 });
    ui.painter().rect_filled(grip, CornerRadius::same(2), c);
}

/// Id je Fenster unterscheiden (im Hauptfenster unverändert).
pub fn salted(id: egui::Id) -> egui::Id {
    match ID_SALT.load(Ordering::Relaxed) {
        0 => id,
        w => id.with(("pd_window", w)),
    }
}

pub fn set_animations(on: bool) {
    ANIMATIONS.store(on, Ordering::Relaxed);
}

/// Weicher Übergang 0..1 hin zu `target` (ease-out). Bei abgeschalteten Animationen sofort.
pub fn anim(ctx: &egui::Context, id: egui::Id, target: bool, dur: f32) -> f32 {
    let d = if ANIMATIONS.load(Ordering::Relaxed) { dur } else { 0.0 };
    ctx.animate_bool_with_time_and_easing(salted(id), target, d, egui::emath::easing::cubic_out)
}

static ACCENT_RGB: AtomicU32 = AtomicU32::new(0x00d9_7757);

/// Oberflächenfarben des aktiven Themes (#309), gesetzt von [`apply`] bzw. [`set_theme_ui`].
static THEME_UI: RwLock<Option<UiColors>> = RwLock::new(None);

thread_local! {
    /// Vorschau des Theme-Editors: solange gesetzt, liefern [`ui`] und [`accent`] die Farben des bearbeiteten Themes.
    static PREVIEW: Cell<Option<UiColors>> = const { Cell::new(None) };
}

/// Oberflächenfarben des aktiven Themes (benannte Farben, siehe `theme::tokens`). Ersetzt feste Farbwerte in der
/// Oberfläche (Status, Diff, Syntax …), damit eigene Themes sie ändern können.
pub fn ui() -> UiColors {
    if let Some(u) = PREVIEW.with(|p| p.get()) {
        return u;
    }
    if let Some(u) = *THEME_UI.read().unwrap_or_else(|e| e.into_inner()) {
        return u;
    }
    let u = crate::theme::all()[0].ui_colors();
    *THEME_UI.write().unwrap_or_else(|e| e.into_inner()) = Some(u);
    u
}

/// Oberflächenfarben des aktiven Themes setzen (ohne den egui-Stil neu zu bauen).
pub fn set_theme_ui(u: UiColors) {
    *THEME_UI.write().unwrap_or_else(|e| e.into_inner()) = Some(u);
}

/// `f` mit den Farben `u` als aktive Oberflächenfarben ausführen (Vorschau im Theme-Editor).
pub fn with_preview<R>(u: UiColors, f: impl FnOnce() -> R) -> R {
    let prev = PREVIEW.with(|p| p.replace(Some(u)));
    let r = f();
    PREVIEW.with(|p| p.set(prev));
    r
}

/// Akzentfarbe des Themes (ohne Projektfarbe) – früher die feste Konstante `ACCENT`.
pub fn brand() -> Color32 {
    ui().accent
}

/// Aktuelle Akzentfarbe: die des Themes ([`brand`]) oder die Farbe des gewählten Projekts im Todolist-Reiter.
pub fn accent() -> Color32 {
    if let Some(u) = PREVIEW.with(|p| p.get()) {
        return u.accent;
    }
    let v = ACCENT_RGB.load(Ordering::Relaxed);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Setzt die Akzentfarbe (jedes Bild von der Anwendung aufgerufen).
pub fn set_accent(c: Color32) {
    ACCENT_RGB.store(((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32, Ordering::Relaxed);
}

// Abstände (4-px-Raster)
pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
pub const RADIUS: u8 = 6;

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Stil auf den egui-Kontext anwenden. `opacity` (0.3–1.0) wirkt auf Seitenleiste und Flächen. Setzt auch die
/// Oberflächenfarben für [`ui`].
pub fn apply(ctx: &egui::Context, theme: &Theme, opacity: f32) {
    set_theme_ui(theme.ui_colors());
    let mut style = (*ctx.style()).clone();
    build_style(&mut style, theme, opacity, accent());
    ctx.set_style(style);
}

/// Stil aus einem Theme bauen (auch für die Vorschau des Theme-Editors). `accent`: aktuelle Akzentfarbe (Projektfarbe
/// im Todo-Reiter); weicht sie vom Akzent des Themes ab, folgen ihr alle Farben, die auf `@accent` verweisen.
pub fn build_style(style: &mut egui::Style, theme: &Theme, opacity: f32, accent: Color32) {
    let base = theme.ui_colors();
    let u = if accent.to_opaque() != base.accent.to_opaque() {
        let mut t = theme.clone();
        t.ui.insert("accent".into(), crate::theme::Spec::Hex(accent.to_opaque()));
        t.ui_colors()
    } else {
        base
    };
    let dark = theme.dark;
    // Mit eigenem Hintergrund des Themes (#387) darf die Seitenleiste durchscheinender sein als der Regler (30 %); Titelleiste
    // und Design-Prüfung nutzen denselben Wert.
    let a = opacity.clamp(0.0, 1.0);
    let fade = |c: Color32| c.gamma_multiply(a);

    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = fade(u.sidebar_bg);
    v.window_fill = u.window_bg;
    v.window_stroke = Stroke::new(1.0_f32, u.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    let shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 130 } else { 50 }) };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.faint_bg_color = fade(u.surface);
    v.extreme_bg_color = u.input_bg;
    v.code_bg_color = u.code_bg;
    v.override_text_color = None;
    v.weak_text_color = Some(u.text_dim);
    v.hyperlink_color = u.link;
    v.selection.bg_fill = u.selection_bg;
    v.selection.stroke = Stroke::new(1.0_f32, u.focus);
    v.text_cursor.stroke = Stroke::new(1.5_f32, u.focus);

    let r = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = u.sidebar_bg;
    w.noninteractive.weak_bg_fill = u.sidebar_bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, u.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, u.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = u.widget_bg;
    w.inactive.weak_bg_fill = u.widget_bg;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, u.widget_border);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, u.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = u.hover_bg;
    w.hovered.weak_bg_fill = u.hover_bg;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, u.hover_border);
    w.hovered.fg_stroke = Stroke::new(1.0_f32, u.text_strong);
    w.hovered.corner_radius = r;
    w.active.bg_fill = u.active_bg;
    w.active.weak_bg_fill = u.active_bg;
    w.active.bg_stroke = Stroke::new(1.0_f32, u.accent);
    w.active.fg_stroke = Stroke::new(1.0_f32, u.text_strong);
    w.active.corner_radius = r;
    w.open = w.hovered.clone();

    style.visuals = v;
    style.spacing.item_spacing = Vec2::new(S2, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size = Vec2::new(32.0, 26.0);
    style.spacing.window_margin = Margin::same(14);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.indent = 14.0;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.spacing.slider_width = 180.0;
    style.animation_time = if ANIMATIONS.load(Ordering::Relaxed) { 0.18 } else { 0.0 };
    style.text_styles.insert(TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Button, FontId::new(13.0, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
    style.text_styles.insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
}

/// Dezente Zweitfarbe für Hinweistexte.
pub fn dim(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

/// Füllfarbe eines Fortschrittsbalkens: Bei 0 % zeichnet egui den gefüllten Teil als Kreis von Balkenhöhe (über dem
/// Anfang der Beschriftung) – dann keine Füllung.
pub fn bar_fill(frac: f32, color: Color32) -> Color32 {
    if frac > 0.0 { color } else { Color32::TRANSPARENT }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Plus,
    Spark,
    SparkSmall,
    Search,
    Clock,
    Folder,
    Broadcast,
    Gear,
    Prompt,
    Close,
    SplitRight,
    SplitDown,
    Detach,
    Note,
    Doc,
    Diff,
    Up,
    Down,
    Queue,
    Tree,
    List,
    FolderPlus,
    Template,
    Terminal,
    Checklist,
    Trash,
    Minimize,
    Maximize,
    Restore,
    /// Kettenglieder (Ticket verknüpfen).
    Link,
    /// Sprechblase (Chat mit einem lokalen Modell).
    Chat,
    /// Pfeil nach unten auf eine Linie: Bereich minimieren (aus dem Layout nehmen, Sitzung läuft weiter).
    Collapse,
    /// Kreispfeil: Sitzung neu laden.
    Reload,
    /// Dreieck: Prozess starten (#243).
    Play,
    /// Quadrat: Prozess stoppen (#243).
    Stop,
    /// Schloss: für Agenten gesperrt (#386).
    Lock,
}

fn arc(p: &Painter, c: Pos2, r: f32, a0: f32, a1: f32, st: Stroke) {
    let n = 10;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, st));
}

/// Zeichnet ein 16-px-Icon mittig um `c`.
pub fn paint_icon(p: &Painter, icon: Icon, c: Pos2, col: Color32) {
    let st = Stroke::new(1.4_f32, col);
    let l = |a: Vec2, b: Vec2| p.line_segment([c + a, c + b], st);
    let v = Vec2::new;
    match icon {
        Icon::Plus => {
            l(v(-5.0, 0.0), v(5.0, 0.0));
            l(v(0.0, -5.0), v(0.0, 5.0));
        }
        Icon::Spark => {
            let st2 = Stroke::new(1.6_f32, col);
            for (a, b) in [(v(0.0, -7.0), v(0.0, 7.0)), (v(-7.0, 0.0), v(7.0, 0.0)), (v(-4.5, -4.5), v(4.5, 4.5)), (v(-4.5, 4.5), v(4.5, -4.5))] {
                p.line_segment([c + a, c + b], st2);
            }
        }
        Icon::SparkSmall => {
            let st2 = Stroke::new(1.3_f32, col);
            for (a, b) in [(v(0.0, -4.0), v(0.0, 4.0)), (v(-4.0, 0.0), v(4.0, 0.0)), (v(-2.8, -2.8), v(2.8, 2.8)), (v(-2.8, 2.8), v(2.8, -2.8))] {
                p.line_segment([c + a, c + b], st2);
            }
        }
        Icon::Chat => {
            // Abgerundete Blase mit Spitze links unten und drei Punkten
            let r = Rect::from_center_size(c + v(0.0, -1.0), v(13.0, 9.5));
            p.rect_stroke(r, 3.0, st, StrokeKind::Inside);
            l(v(-3.5, 3.6), v(-5.0, 6.5));
            l(v(-5.0, 6.5), v(-0.5, 3.6));
            for x in [-3.0, 0.0, 3.0] {
                p.circle_filled(c + v(x, -1.0), 0.9, col);
            }
        }
        Icon::Link => {
            p.rect_stroke(Rect::from_center_size(c + v(-3.0, 0.0), v(9.5, 6.0)), 3.0, st, StrokeKind::Inside);
            p.rect_stroke(Rect::from_center_size(c + v(3.0, 0.0), v(9.5, 6.0)), 3.0, st, StrokeKind::Inside);
        }
        Icon::Search => {
            p.circle_stroke(c + v(-1.5, -1.5), 4.6, st);
            l(v(1.8, 1.8), v(6.0, 6.0));
        }
        Icon::Tree => {
            l(v(-5.0, -5.0), v(-5.0, 5.0));
            l(v(-5.0, -3.0), v(5.0, -3.0));
            l(v(-5.0, 1.0), v(2.0, 1.0));
            l(v(-5.0, 5.0), v(5.0, 5.0));
        }
        Icon::List => {
            for y in [-4.5, 0.0, 4.5] {
                l(v(-5.5, y), v(5.5, y));
            }
        }
        Icon::FolderPlus => {
            p.rect_stroke(Rect::from_center_size(c + v(-1.0, 0.5), v(11.0, 8.0)), 1.5, st, StrokeKind::Inside);
            l(v(4.0, -6.0), v(4.0, -1.0));
            l(v(1.5, -3.5), v(6.5, -3.5));
        }
        Icon::Template => {
            p.rect_stroke(Rect::from_center_size(c + v(-2.0, -2.0), v(9.0, 9.0)), 1.5, st, StrokeKind::Inside);
            p.rect_stroke(Rect::from_center_size(c + v(2.0, 2.0), v(9.0, 9.0)), 1.5, st, StrokeKind::Inside);
        }
        Icon::Clock => {
            p.circle_stroke(c, 6.2, st);
            l(v(0.0, -3.5), v(0.0, 0.0));
            l(v(0.0, 0.0), v(3.0, 1.8));
        }
        Icon::Folder => {
            let r = Rect::from_center_size(c + v(0.0, 1.0), v(13.0, 9.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-6.5, -3.6), v(-1.5, -3.6));
        }
        Icon::Broadcast => {
            p.circle_filled(c, 1.8, col);
            for dir in [0.0_f32, std::f32::consts::PI] {
                arc(p, c, 4.5, dir - 0.7, dir + 0.7, st);
                arc(p, c, 7.5, dir - 0.7, dir + 0.7, st);
            }
        }
        Icon::Gear => {
            p.circle_stroke(c, 3.0, st);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let (s, co) = a.sin_cos();
                p.line_segment([c + v(co * 5.0, s * 5.0), c + v(co * 7.0, s * 7.0)], Stroke::new(2.0_f32, col));
            }
            p.circle_stroke(c, 5.2, st);
        }
        Icon::Prompt => {
            let r = Rect::from_center_size(c + v(0.0, -1.0), v(13.0, 9.5));
            p.rect_stroke(r, 2.0, st, StrokeKind::Inside);
            l(v(-3.0, 3.8), v(-3.0, 6.5));
            l(v(-3.0, 6.5), v(0.0, 3.8));
        }
        Icon::Terminal => {
            let r = Rect::from_center_size(c, v(15.0, 11.0));
            p.rect_stroke(r, 2.5, st, StrokeKind::Inside);
            l(v(-4.0, -2.0), v(-1.5, 0.0));
            l(v(-1.5, 0.0), v(-4.0, 2.0));
            l(v(0.5, 2.5), v(4.0, 2.5));
        }
        Icon::Checklist => {
            for (y, check) in [(-5.0_f32, true), (0.0, true), (5.0, false)] {
                if check {
                    l(v(-6.5, y), v(-5.0, y + 1.5));
                    l(v(-5.0, y + 1.5), v(-2.5, y - 1.5));
                } else {
                    p.rect_stroke(Rect::from_center_size(c + v(-4.5, y), v(4.0, 4.0)), 0.5, st, StrokeKind::Inside);
                }
                l(v(0.0, y), v(6.5, y));
            }
        }
        Icon::Trash => {
            // Deckel mit Griff, Eimer mit zwei Rillen
            l(v(-6.0, -4.0), v(6.0, -4.0));
            l(v(-2.0, -4.0), v(-2.0, -6.0));
            l(v(-2.0, -6.0), v(2.0, -6.0));
            l(v(2.0, -6.0), v(2.0, -4.0));
            l(v(-4.5, -4.0), v(-3.7, 6.0));
            l(v(-3.7, 6.0), v(3.7, 6.0));
            l(v(3.7, 6.0), v(4.5, -4.0));
            l(v(-1.5, -1.5), v(-1.5, 3.5));
            l(v(1.5, -1.5), v(1.5, 3.5));
        }
        Icon::Minimize => {
            l(v(-5.0, 0.5), v(5.0, 0.5));
        }
        Icon::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, v(9.0, 9.0)), 0.0, st, StrokeKind::Inside);
        }
        Icon::Restore => {
            p.rect_stroke(Rect::from_center_size(c + v(-1.0, 1.0), v(7.5, 7.5)), 0.0, st, StrokeKind::Inside);
            l(v(0.0, -4.5), v(4.5, -4.5));
            l(v(4.5, -4.5), v(4.5, 0.0));
            l(v(0.0, -4.5), v(0.0, -2.8));
        }
        Icon::Close => {
            l(v(-4.0, -4.0), v(4.0, 4.0));
            l(v(-4.0, 4.0), v(4.0, -4.0));
        }
        Icon::Reload => {
            // Kreis mit Lücke oben, Pfeilspitze am Ende des Bogens (im Uhrzeigersinn).
            let (r, a0, a1) = (5.0_f32, -1.25_f32, 4.0_f32);
            arc(p, c, r, a0, a1, st);
            let tip = v(r * a1.cos(), r * a1.sin());
            let back = v(a1.sin(), -a1.cos()); // gegen die Laufrichtung am Bogenende
            let side = v(-back.y, back.x);
            l(tip, tip + back * 3.2 + side * 2.4);
            l(tip, tip + back * 3.2 - side * 2.4);
        }
        Icon::Collapse => {
            l(v(0.0, -5.0), v(0.0, 1.5));
            l(v(-3.0, -1.5), v(0.0, 1.5));
            l(v(3.0, -1.5), v(0.0, 1.5));
            l(v(-5.0, 4.5), v(5.0, 4.5));
        }
        Icon::SplitRight => {
            let r = Rect::from_center_size(c, v(12.0, 10.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(0.0, -5.0), v(0.0, 5.0));
        }
        Icon::SplitDown => {
            let r = Rect::from_center_size(c, v(12.0, 10.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-6.0, 0.0), v(6.0, 0.0));
        }
        Icon::Detach => {
            let r = Rect::from_center_size(c + v(-1.5, 1.5), v(9.0, 8.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(5.5, -5.5), v(1.5, -5.5));
            l(v(5.5, -5.5), v(5.5, -1.5));
            l(v(5.5, -5.5), v(-0.5, 0.5));
        }
        Icon::Diff => {
            let r = Rect::from_center_size(c, v(12.0, 14.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-3.0, -3.0), v(3.0, -3.0));
            l(v(0.0, -6.0), v(0.0, 0.0));
            l(v(-3.0, 3.5), v(3.0, 3.5));
        }
        Icon::Up => {
            l(v(-4.5, 2.0), v(0.0, -3.0));
            l(v(0.0, -3.0), v(4.5, 2.0));
        }
        Icon::Down => {
            l(v(-4.5, -2.0), v(0.0, 3.0));
            l(v(0.0, 3.0), v(4.5, -2.0));
        }
        Icon::Queue => {
            for y in [-4.5, 0.0, 4.5] {
                l(v(-2.0, y), v(6.0, y));
                p.circle_filled(c + v(-5.5, y), 1.2, col);
            }
        }
        Icon::Doc => {
            let r = Rect::from_center_size(c, v(11.0, 14.0));
            p.rect_stroke(r, 1.5, st, StrokeKind::Inside);
            l(v(-3.0, -2.0), v(3.0, -2.0));
            l(v(-3.0, 1.0), v(3.0, 1.0));
            l(v(-3.0, 4.0), v(1.0, 4.0));
        }
        Icon::Note => {
            for y in [-3.5, 0.0, 3.5] {
                l(v(-5.0, y), v(5.0, y));
            }
        }
        Icon::Play => {
            let pts = vec![c + v(-3.5, -5.0), c + v(-3.5, 5.0), c + v(5.0, 0.0)];
            p.add(egui::Shape::convex_polygon(pts, Color32::TRANSPARENT, st));
        }
        Icon::Stop => {
            p.rect_stroke(Rect::from_center_size(c, v(9.0, 9.0)), 1.5, st, StrokeKind::Inside);
        }
        Icon::Lock => {
            // Schloss: Körper und Bügel (Projekt für Agenten gesperrt, #386)
            p.rect_stroke(Rect::from_center_size(c + v(0.0, 2.0), v(10.0, 7.0)), 1.5, st, StrokeKind::Inside);
            arc(p, c + v(0.0, -2.0), 3.0, std::f32::consts::PI, 2.0 * std::f32::consts::PI, st);
            l(v(-3.0, -2.0), v(-3.0, -1.5));
            l(v(3.0, -2.0), v(3.0, -1.5));
        }
    }
}

/// Quadratischer Icon-Button (flach, Hover-Fläche, Tooltip, optional "aktiv").
pub fn icon_btn(ui: &mut Ui, icon: Icon, tip: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let v = ui.visuals();
    let (text, strong, hover_bg) = (v.text_color(), v.strong_text_color(), v.widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    let on = anim(ui.ctx(), resp.id.with("on"), active, D_BASE);
    ui.painter().rect_filled(rect, RADIUS as f32, hover_bg.gamma_multiply(h));
    ui.painter().rect_filled(rect, RADIUS as f32, accent().gamma_multiply(0.28 * on));
    let base = mix(text.gamma_multiply(0.78), strong, h);
    let col = mix(base, accent(), on);
    paint_icon(ui.painter(), icon, rect.center(), if icon == Icon::Spark && !active { accent() } else { col });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

/// Kleinerer Icon-Button für Kopfzeilen (22 px).
pub fn icon_btn_small(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let v = ui.visuals();
    let (text, strong, hover_bg) = (v.weak_text_color(), v.strong_text_color(), v.widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(rect, 5.0, hover_bg.gamma_multiply(h));
    paint_icon(ui.painter(), icon, rect.center(), mix(text, strong, h));
    resp.on_hover_text(tip)
}

/// Rot für Fehler und zerstörende Aktionen in der Oberfläche (Farbe `error` des Themes; Standard: das Rot von One Dark).
pub fn danger_soft() -> Color32 {
    ui().error
}

/// Farbe des Symbols von [`danger_icon_at`] bei Überfahren-Anteil `hover` (0..1, animiert): in Ruhe unauffällig in der
/// gedämpften Textfarbe (kein Rot), beim Überfahren das übliche Warnrot (Entscheidung des Nutzers zu #156, wie der
/// Papierkorb der Todo-Karten). `enabled = false`: blass.
pub fn danger_icon_color(text: Color32, enabled: bool, hover: f32) -> Color32 {
    if enabled { mix(text.gamma_multiply(0.7), danger_soft(), hover) } else { text.gamma_multiply(0.3) }
}

/// Zerstörender Icon-Knopf (Papierkorb, „alle schließen“) an einer festen Stelle: in Ruhe unauffällig, beim Überfahren
/// Warnrot mit rötlicher Fläche ([`danger_icon_color`]). `enabled = false` zeichnet ihn blass und reagiert nicht.
pub fn danger_icon_at(ui: &mut Ui, rect: Rect, id: egui::Id, tip: &str, enabled: bool) -> Response {
    // Auch Ziehen annehmen: sonst begänne ein Druck auf den Knopf mit Bewegung ein Ziehen der Zeile darunter (Ordner).
    let resp = ui.interact(rect, id, if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let text = ui.visuals().text_color();
    let h = anim(ui.ctx(), id.with("h"), enabled && resp.hovered(), D_FAST);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(rect, RADIUS as f32, danger_soft().gamma_multiply(0.18 * h));
    paint_icon(ui.painter(), Icon::Trash, rect.center(), danger_icon_color(text, enabled, h));
    resp.on_hover_text(tip)
}

/// Pill (Segment) – z. B. für Projekte. Gibt die Antwort zurück.
pub fn pill(ui: &mut Ui, text: &str, count: Option<usize>, color: Color32, active: bool) -> Response {
    pill_ex(ui, text, count, color, active, false)
}

/// [`pill`] mit Schloss am Ende (`locked`: Projekt für Agenten gesperrt, #386).
pub fn pill_ex(ui: &mut Ui, text: &str, count: Option<usize>, color: Color32, active: bool, locked: bool) -> Response {
    let font = FontId::proportional(12.5);
    let label = ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::PLACEHOLDER);
    let cnt = count.map(|n| ui.painter().layout_no_wrap(n.to_string(), FontId::proportional(11.0), Color32::PLACEHOLDER));
    let w = 12.0 + 8.0 + label.size().x + cnt.as_ref().map(|c| c.size().x + 6.0).unwrap_or(0.0) + 8.0 + if locked { 14.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    let v = ui.visuals();
    let (text_c, hover_bg) = (v.text_color(), v.widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    let on = anim(ui.ctx(), resp.id.with("on"), active, D_BASE);
    ui.painter().rect_filled(rect, 12.0, hover_bg.gamma_multiply(h * (1.0 - on)));
    ui.painter().rect_filled(rect, 12.0, chip_fill(color).gamma_multiply(on));
    ui.painter().rect_stroke(rect, 12.0, Stroke::new(1.0_f32, color.gamma_multiply(0.7 * on)), StrokeKind::Inside);
    ui.painter().circle_filled(Pos2::new(rect.min.x + 12.0, rect.center().y), 3.5, color);
    let tc = mix(text_c.gamma_multiply(0.75), text_c, on.max(h * 0.6));
    let mut x = rect.min.x + 12.0 + 8.0;
    ui.painter().galley(Pos2::new(x, rect.center().y - label.size().y / 2.0), label.clone(), tc);
    x += label.size().x + 6.0;
    if let Some(c) = cnt {
        let cw = c.size().x;
        ui.painter().galley(Pos2::new(x, rect.center().y - c.size().y / 2.0), c, tc.gamma_multiply(0.6));
        x += cw + 6.0;
    }
    if locked {
        paint_icon(ui.painter(), Icon::Lock, Pos2::new(x + 4.0, rect.center().y), tc.gamma_multiply(0.8));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Statuspille mit Punkt und Text (rechts in Kopfzeilen).
pub fn status_pill(p: &Painter, right_center: Pos2, label: &str, color: Color32) -> f32 {
    let g = p.layout_no_wrap(label.to_string(), FontId::proportional(11.5), color);
    let w = g.size().x + 8.0 + 14.0 + 4.0;
    let rect = Rect::from_min_size(Pos2::new(right_center.x - w, right_center.y - 10.0), Vec2::new(w, 20.0));
    p.rect_filled(rect, 10.0, color.gamma_multiply(0.18));
    p.circle_filled(Pos2::new(rect.min.x + 10.0, rect.center().y), 3.2, color);
    p.galley(Pos2::new(rect.min.x + 18.0, rect.center().y - g.size().y / 2.0), g, color);
    w
}

/// Animierter Ein/Aus-Schalter.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let t = anim(ui.ctx(), resp.id.with("t"), *on, D_BASE);
    let off_col = ui.visuals().widgets.inactive.bg_fill.gamma_multiply(1.6);
    let track = mix(off_col, accent(), t);
    ui.painter().rect_filled(rect, 10.0, track);
    let x = rect.min.x + 10.0 + (rect.width() - 20.0) * t;
    ui.painter().circle_filled(Pos2::new(x, rect.center().y), 7.0, Color32::from_rgb(0xf4, 0xf4, 0xf6));
    resp
}

fn ui_tab_active() -> Color32 {
    ui().tab_active
}

/// Fläche, die dem Akzent folgt: Hat das Theme sie nicht geändert (Standard `@accent/<pct>`), die aktuelle
/// Akzentfarbe (Projektfarbe im Todo-Reiter) mit dieser Deckkraft, sonst die Farbe des Themes.
fn follow_accent(themed: Color32, pct: f32) -> Color32 {
    if themed == brand().gamma_multiply(pct) { accent().gamma_multiply(pct) } else { themed }
}

/// Fläche eines aktiven Chips der Farbe `color`: Standard `color` zu 28 %; setzt das Theme `chip_active`, dessen Farbe.
fn chip_fill(color: Color32) -> Color32 {
    let themed = ui().chip_active;
    if themed == brand().gamma_multiply(0.28) { color.gamma_multiply(0.28) } else { themed }
}

/// Tab-Pille ohne Farbpunkt (z. B. Einstellungen).
pub fn tab(ui: &mut Ui, text: &str, active: bool) -> Response {
    let font = FontId::proportional(13.0);
    let g = ui.painter().layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 24.0, 26.0), Sense::click());
    let (text_c, hover_bg) = (ui.visuals().text_color(), ui.visuals().widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    let on = anim(ui.ctx(), resp.id.with("on"), active, D_BASE);
    ui.painter().rect_filled(rect, 13.0, hover_bg.gamma_multiply(h * (1.0 - on)));
    ui.painter().rect_filled(rect, 13.0, follow_accent(ui_tab_active(), 0.25).gamma_multiply(on));
    let tc = mix(text_c.gamma_multiply(0.72), text_c, on.max(h * 0.6));
    ui.painter().galley(Pos2::new(rect.min.x + 12.0, rect.center().y - g.size().y / 2.0), g, tc);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danger_icon_is_subtle_until_hovered() {
        let text = Color32::from_gray(200);
        let rest = danger_icon_color(text, true, 0.0);
        assert!(rest.r() == rest.g() && rest.g() == rest.b(), "in Ruhe kein Rot: {rest:?}");
        assert_eq!(danger_icon_color(text, true, 1.0), danger_soft(), "überfahren: Warnrot");
        let half = danger_icon_color(text, true, 0.5);
        assert!(half.r() > half.g(), "Übergang wird rötlich");
        let off = danger_icon_color(text, false, 1.0);
        assert!(off.r() == off.g() && off.a() < rest.a(), "deaktiviert: blass, nie rot");
    }

    #[test]
    fn modal_drag_keeps_the_dialog_inside_the_window() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));
        let size = Vec2::new(400.0, 300.0);
        let c = egui::Align2::CENTER_CENTER;
        assert_eq!(clamp_modal_drag(screen, size, c, Vec2::ZERO, Vec2::new(100.0, -50.0)), Vec2::new(100.0, -50.0), "innen: unverändert");
        assert_eq!(clamp_modal_drag(screen, size, c, Vec2::ZERO, Vec2::new(-900.0, 900.0)), Vec2::new(-300.0, 250.0), "an den Rand");
        // Anker oben mittig mit Abstand (Schnellmenü): nach oben nur bis zum Rand.
        let top = egui::Align2::CENTER_TOP;
        assert_eq!(clamp_modal_drag(screen, size, top, Vec2::new(0.0, 90.0), Vec2::new(0.0, -500.0)), Vec2::new(0.0, -90.0));
        // Größer als das Fenster: linke/obere Kante bleibt sichtbar.
        let huge = Vec2::new(1200.0, 900.0);
        assert_eq!(clamp_modal_drag(screen, huge, c, Vec2::ZERO, Vec2::new(300.0, 300.0)), Vec2::new(100.0, 50.0));
    }

    #[test]
    fn ids_are_salted_per_window_only_outside_the_main_window() {
        let id = egui::Id::new("tab");
        assert_eq!(salted(id), id, "Hauptfenster: Ids wie bisher");
        set_id_salt(3);
        let three = salted(id);
        set_id_salt(4);
        let four = salted(id);
        set_id_salt(0);
        assert!(three != id && four != id && three != four, "jedes weitere Fenster hat eigene Animations-Ids");
        assert_eq!(salted(id), id);
    }
}
