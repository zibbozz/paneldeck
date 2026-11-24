//! Reiter „Hintergrund“ des Theme-Editors (#387): Art (Bild, Verlauf, Farbe, Muster), Bild wählen, Skalierung, Anker,
//! Abstände, Deckkraft, Helligkeit, Unschärfe, Überlagerung, Verlauf mit ziehbaren Stopps, Muster, Bereiche und
//! „Hintergrund übernehmen von …“. Eine kleine Vorschau wird im Hintergrund-Faden berechnet; ihre Kennzahlen speisen die
//! Design-Regel „Hintergrund“. Gewählte Bilder liegen bis zum Speichern im Speicher und werden dann in `themes/<datei>/`
//! geschrieben. Untermodul von `theme_editor` (sieht dessen Felder).

use super::Editor;
use crate::state::Lang;
use crate::style;
use crate::theme::background::{self, Anchor, Area, Background, GradKind, Kind, Mode, Overlay, Pattern, PatternName, Stop};
use crate::theme::bake::{self, Slot, Src, Stats};
use crate::theme::tokens::Spec;
use crate::theme::Theme;
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use std::sync::{Arc, Mutex};

/// Größe der Vorschau (Pixel) und die Fensterbreite, die sie darstellt (Punkte; für Abstände und Muster).
const PREVIEW: (u32, u32) = (384, 216);
const PREVIEW_WINDOW_PT: f32 = 1280.0;
/// Auftraggeber der Vorschau im Hintergrund-Faden.
const PREVIEW_WHO: u64 = 0x5747_00ED;

/// Ein gewähltes, noch nicht gespeichertes Bild.
pub(super) struct Pending {
    pub(super) name: String,
    pub(super) bytes: Arc<Vec<u8>>,
    pub(super) src: Src,
}

/// Wohin ein gewähltes Bild gehört.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Image,
    Tile,
}

type ImportSlot = Arc<Mutex<Option<Result<(String, bake::Prepared), String>>>>;

/// Zustand des Reiters im Editor.
#[derive(Default)]
pub(super) struct BgState {
    pub(super) pending: Vec<Pending>,
    import: Option<(Target, ImportSlot)>,
    preview: Option<(u64, Slot)>,
    tex: Option<(u64, egui::TextureHandle)>,
    pub(super) stats: Option<Stats>,
    pub(super) error: Option<String>,
    sel_stop: usize,
    next_id: u64,
    spec_buf: std::collections::HashMap<&'static str, (String, String)>,
}

impl BgState {
    /// Dateien, die beim Speichern geschrieben werden: gewählte Bilder, auf die `t` verweist.
    pub(super) fn files_for(&self, t: &Theme) -> Vec<(String, Vec<u8>)> {
        let Some(b) = &t.background else { return Vec::new() };
        b.files().into_iter().filter_map(|n| self.pending.iter().find(|p| p.name == n).map(|p| (p.name.clone(), p.bytes.as_ref().clone()))).collect()
    }

    /// Quelle eines Bildnamens: gewählt (Speicher bzw. Datei eines anderen Themes) oder im Ordner des Themes.
    fn src_of(&self, t: &Theme, name: &str) -> Option<Src> {
        if let Some(p) = self.pending.iter().find(|p| p.name == name) {
            return Some(p.src.clone());
        }
        super::super::backdrop::file_src(t.source.as_deref(), name)
    }
}

/// Vorschau des bearbeiteten Hintergrunds berechnen lassen und abholen (kein eigenes Neuzeichnen).
pub(super) fn tick_preview(ed: &mut Editor, ctx: &egui::Context) {
    let t = ed.theme.clone();
    let st = &mut ed.bg;
    if let Some((_, slot)) = &st.preview {
        if let Some(r) = bake::take(slot) {
            let (key, _) = st.preview.take().unwrap_or((0, Arc::new(Mutex::new(None))));
            if let Ok(b) = r {
                let img = egui::ColorImage::from_rgba_unmultiplied([b.img.width() as usize, b.img.height() as usize], b.img.as_raw());
                st.tex = Some((key, ctx.load_texture("pd-theme-bg-preview", img, egui::TextureOptions::LINEAR)));
                st.stats = Some(b.stats);
            }
        }
    }
    if t.background.is_none() {
        st.tex = None;
        st.stats = None;
        st.preview = None;
        return;
    }
    let spec = bake::spec_for(&t, PREVIEW.0, PREVIEW.1, PREVIEW.0 as f32 / PREVIEW_WINDOW_PT, &|n| st.src_of(&t, n));
    let Some(spec) = spec else { return };
    let key = spec.look_key();
    let shown = st.tex.as_ref().map(|x| x.0) == Some(key);
    let running = st.preview.as_ref().map(|x| x.0) == Some(key);
    if !shown && !running {
        st.preview = Some((key, bake::submit(PREVIEW_WHO, spec)));
    }
    // Gewähltes Bild fertig aufbereitet?
    if let Some((target, slot)) = &st.import {
        let done = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(r) = done {
            let target = *target;
            st.import = None;
            match r {
                Ok((orig, prep)) => {
                    let mut taken = crate::theme::bg_files::names_in_folder(t.source.as_deref());
                    taken.extend(st.pending.iter().map(|p| p.name.clone()));
                    let name = bake::file_name_for(&orig, prep.ext, &taken);
                    st.next_id += 1;
                    let id = 0x1000_0000 + st.next_id;
                    st.pending.push(Pending { name: name.clone(), bytes: Arc::new(prep.bytes), src: Src::Mem { id, img: prep.img } });
                    st.error = None;
                    let now = ctx.input(|i| i.time);
                    ed.edit(now, false, |t| {
                        let b = t.background.get_or_insert_with(|| Background::new(Kind::Image));
                        b.clear_invalid();
                        match target {
                            Target::Image => {
                                b.kind = Kind::Image;
                                b.image = Some(name);
                            }
                            Target::Tile => {
                                b.kind = Kind::Pattern;
                                b.pattern.get_or_insert_with(Pattern::default).image = Some(name);
                            }
                        }
                    });
                }
                Err(e) => st.error = Some(e),
            }
        }
    }
}

/// Bild wählen (Dialog) und im Hintergrund aufbereiten.
fn pick(st: &mut BgState, target: Target) {
    let Some(path) = crate::platform::notify::pick_background_image() else { return };
    let slot: ImportSlot = Arc::new(Mutex::new(None));
    let s2 = slot.clone();
    std::thread::spawn(move || {
        let orig = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let r = bake::prepare_file(&path).map(|p| (orig, p));
        *s2.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
    });
    st.import = Some((target, slot));
}

/// Hintergrund eines anderen Themes übernehmen (Bilder werden beim Speichern kopiert).
fn take_from(ed: &mut Editor, other: &Theme, now: f64) {
    let Some(b) = other.background.clone() else { return };
    let mut b = b;
    let mut taken = crate::theme::bg_files::names_in_folder(ed.theme.source.as_deref());
    taken.extend(ed.bg.pending.iter().map(|p| p.name.clone()));
    for name in b.files().into_iter().map(|s| s.to_string()).collect::<Vec<_>>() {
        let Some(path) = other.source.as_deref().and_then(|s| background::resolve_file(s, &name)) else { continue };
        let Ok(bytes) = crate::chat::images::read_limited(&path) else { continue };
        let Some(src) = Src::file(path.clone()) else { continue };
        let ext = std::path::Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
        let new = bake::file_name_for(&name, &ext, &taken);
        taken.push(new.clone());
        if b.image.as_deref() == Some(name.as_str()) {
            b.image = Some(new.clone());
        }
        if let Some(p) = b.pattern.as_mut().filter(|p| p.image.as_deref() == Some(name.as_str())) {
            p.image = Some(new.clone());
        }
        ed.bg.pending.push(Pending { name: new, bytes: Arc::new(bytes), src });
    }
    ed.edit(now, false, |t| t.background = Some(b));
}

/// Farbwert (Hex oder Verweis) bearbeiten: Farbknopf + Textfeld (`#rrggbb[aa]`, `@name`, `@name/40`).
fn spec_edit(ui: &mut Ui, st: &mut BgState, key: &'static str, spec: &Spec, t: &Theme, lang: Lang) -> Option<Spec> {
    let mut out = None;
    ui.horizontal(|ui| {
        let mut c = background::spec_color(t, spec);
        if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend).changed() {
            out = Some(Spec::Hex(c));
        }
        let text = spec.to_text();
        let entry = st.spec_buf.entry(key).or_insert_with(|| (text.clone(), text.clone()));
        if entry.1 != text {
            *entry = (text.clone(), text.clone());
        }
        let r = ui.add(egui::TextEdit::singleline(&mut entry.0).desired_width(110.0).font(egui::TextStyle::Monospace)).on_hover_text(lang.t("#RRGGBB, #RRGGBBAA oder Verweis wie @accent/40", "#RRGGBB, #RRGGBBAA or a link like @accent/40"));
        if r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            match Spec::parse(&entry.0) {
                Ok(s) if s != *spec => out = Some(s),
                Ok(_) => {}
                Err(_) => entry.0 = text,
            }
        }
    });
    out
}

/// Kurzname einer Datei des Hintergrunds mit Zustand (fehlt / neu).
fn file_label(st: &BgState, t: &Theme, name: Option<&str>, lang: Lang) -> (String, bool) {
    match name {
        None => (lang.t("kein Bild gewählt", "no image chosen").to_string(), false),
        Some(n) if st.pending.iter().any(|p| p.name == n) => (format!("{n} ({})", lang.t("neu, wird beim Speichern kopiert", "new, copied when saving")), true),
        Some(n) if st.src_of(t, n).is_some() => (n.to_string(), true),
        Some(n) => (format!("{n} – {}", lang.t("fehlt, es gilt die Grundfarbe", "missing, the base colour applies")), false),
    }
}

/// Vorschau: Bild mit je einem Feld je Bereich (Fläche in der Deckkraft des Bereichs, Beispieltext).
fn preview(ui: &mut Ui, st: &BgState, t: &Theme, global: f32, lang: Lang) {
    let w = ui.available_width().min(340.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, w * 9.0 / 16.0), Sense::hover());
    let p = ui.painter_at(rect);
    match &st.tex {
        Some((_, tex)) => {
            p.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.rect_filled(rect, CornerRadius::same(4), t.bg.to_opaque());
            p.text(rect.center(), egui::Align2::CENTER_CENTER, lang.t("wird berechnet …", "rendering …"), egui::FontId::proportional(11.0), style::dim(ui));
        }
    }
    let Some(b) = &t.background else { return };
    let u = t.ui_colors();
    let cells = [(Area::Sidebar, u.sidebar_bg, u.text), (Area::Terminals, t.bg, t.fg), (Area::Editor, t.bg, t.fg), (Area::Todo, t.bg, u.text), (Area::Chat, t.bg, u.text)];
    let cw = (rect.width() - 12.0) / cells.len() as f32;
    for (i, (area, surf, text)) in cells.into_iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + 6.0 + i as f32 * cw, rect.top() + rect.height() * 0.35), Vec2::new(cw - 4.0, rect.height() * 0.5));
        let a = b.surface_alpha(area, global);
        let [sr, sg, sb, _] = surf.to_opaque().to_srgba_unmultiplied();
        p.rect_filled(r, CornerRadius::same(3), Color32::from_rgba_unmultiplied(sr, sg, sb, (a * 255.0).round() as u8));
        let label = match area {
            Area::Terminals => "$ ls",
            Area::Editor => "fn main()",
            Area::Todo => lang.t("Aufgabe", "Task"),
            Area::Chat => lang.t("Hallo", "Hello"),
            Area::Sidebar => lang.t("Sitzung", "Session"),
        };
        p.text(r.left_top() + Vec2::new(4.0, 4.0), egui::Align2::LEFT_TOP, label, egui::FontId::monospace(10.0), text);
    }
    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
}

/// Stopp-Leiste des Verlaufs: zeigt den Verlauf, Stopps ziehen, anklicken wählt.
fn stop_bar(ui: &mut Ui, g: &mut background::Gradient, t: &Theme, sel: &mut usize) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(300.0), 34.0), Sense::hover());
    let bar = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 18.0));
    let p = ui.painter();
    let base = t.bg.to_opaque();
    let n = 48;
    for i in 0..n {
        let x0 = bar.left() + bar.width() * i as f32 / n as f32;
        let x1 = bar.left() + bar.width() * (i + 1) as f32 / n as f32;
        let tt = (i as f32 + 0.5) / n as f32 * 100.0;
        let c = sample(g, t, tt);
        p.rect_filled(Rect::from_min_max(Pos2::new(x0, bar.top()), Pos2::new(x1, bar.bottom())), 0.0, crate::theme::color::over(c, base));
    }
    let mut changed = false;
    for i in 0..g.stops.len() {
        let x = bar.left() + bar.width() * g.stops[i].pos as f32 / 100.0;
        let h = Rect::from_center_size(Pos2::new(x, bar.bottom() + 8.0), Vec2::new(12.0, 14.0));
        let resp = ui.interact(h, ui.id().with(("pd_bg_stop", i)), Sense::click_and_drag());
        if resp.clicked() || resp.drag_started() {
            *sel = i;
        }
        if resp.dragged() {
            let pos = ((resp.interact_pointer_pos().map(|q| q.x).unwrap_or(x) - bar.left()) / bar.width() * 100.0).round().clamp(0.0, 100.0) as u8;
            if pos != g.stops[i].pos {
                g.stops[i].pos = pos;
                changed = true;
            }
        }
        let col = if *sel == i { style::accent() } else { ui.visuals().text_color() };
        let tri = vec![Pos2::new(x, bar.bottom()), Pos2::new(x - 6.0, bar.bottom() + 12.0), Pos2::new(x + 6.0, bar.bottom() + 12.0)];
        ui.painter().add(egui::Shape::convex_polygon(tri, crate::theme::color::over(background::spec_color(t, &g.stops[i].color), base), Stroke::new(1.5_f32, col)));
    }
    changed
}

fn sample(g: &background::Gradient, t: &Theme, pos: f32) -> Color32 {
    let mut stops: Vec<(f32, Color32)> = g.stops.iter().map(|s| (s.pos as f32, background::spec_color(t, &s.color))).collect();
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(first) = stops.first().copied() else { return Color32::TRANSPARENT };
    if pos <= first.0 {
        return first.1;
    }
    for w in stops.windows(2) {
        if pos <= w[1].0 {
            let k = if w[1].0 > w[0].0 { (pos - w[0].0) / (w[1].0 - w[0].0) } else { 1.0 };
            return style::mix(w[0].1, w[1].1, k);
        }
    }
    stops.last().map(|s| s.1).unwrap_or(first.1)
}

/// Der Reiter „Hintergrund“ (linke Spalte des Editors). `others`: Themes mit Hintergrund zum Übernehmen.
pub(super) fn panel(ed: &mut Editor, ui: &mut Ui, lang: Lang, now: f64) {
    let out = egui::ScrollArea::vertical().id_salt("pd_theme_bg").auto_shrink([false, false]).show(ui, |ui| body(ed, ui, lang, now));
    let _ = out;
}

fn body(ed: &mut Editor, ui: &mut Ui, lang: Lang, now: f64) {
    let mut on = ed.theme.background.is_some();
    if ui.checkbox(&mut on, RichText::new(lang.t("Eigener Hintergrund", "Own background")).strong()).on_hover_text(lang.t(
        "Bild, Verlauf, Farbe oder Muster hinter dem ganzen Fenster. Die Flächen darüber lassen ihn je Bereich durchscheinen.",
        "Image, gradient, colour or pattern behind the whole window. The surfaces above let it show through per area.",
    )).changed() {
        ed.edit(now, false, |t| t.background = if on { Some(Background::new(Kind::Gradient)) } else { None });
    }
    // Übernehmen von einem anderen Theme.
    let others: Vec<Theme> = ed.others.iter().filter(|o| o.name != ed.theme.name).cloned().collect();
    if !others.is_empty() {
        let mut pick_other = None;
        egui::ComboBox::from_id_salt("pd_bg_take").selected_text(lang.t("Hintergrund übernehmen von …", "Take background from …")).show_ui(ui, |ui| {
            for o in &others {
                if ui.selectable_label(false, o.name.as_str()).clicked() {
                    pick_other = Some(o.clone());
                }
            }
        });
        if let Some(o) = pick_other {
            take_from(ed, &o, now);
        }
    }
    if let Some(e) = &ed.bg.error {
        ui.label(RichText::new(e).small().color(style::ui().error));
    }
    if ed.bg.import.is_some() {
        ui.label(RichText::new(lang.t("Bild wird aufbereitet …", "Preparing the image …")).small().color(style::dim(ui)));
    }
    let Some(mut b) = ed.theme.background.clone() else { return };
    let t = ed.theme.clone();
    ui.add_space(4.0);
    preview(ui, &ed.bg, &t, ed.global, lang);
    ui.add_space(6.0);
    let before = b.clone();
    let mut merge = false;
    // Art.
    ui.horizontal_wrapped(|ui| {
        for k in Kind::ALL {
            ui.radio_value(&mut b.kind, k, k.label(lang));
        }
    });
    if b.kind != before.kind {
        match b.kind {
            Kind::Gradient if b.gradient.is_none() => b.gradient = Some(Default::default()),
            Kind::Pattern if b.pattern.is_none() => b.pattern = Some(Pattern::default()),
            Kind::Color if b.color.is_none() => b.color = Some(Spec::Link { to: "accent".into(), alpha: Some(25) }),
            _ => {}
        }
    }
    ui.separator();
    match b.kind {
        Kind::Image => {
            let (label, ok) = file_label(&ed.bg, &t, b.image.as_deref(), lang);
            ui.label(RichText::new(label).small().color(if ok { style::dim(ui) } else { style::ui().warning }));
            ui.horizontal(|ui| {
                if ui.add_enabled(ed.bg.import.is_none(), egui::Button::new(lang.t("Bild wählen …", "Choose image …"))).on_hover_text(lang.t(
                    "PNG, JPEG, WebP oder BMP; animiert = erstes Bild. Eine Kopie (höchstens 3840 px) kommt in den Ordner des Themes, das Original bleibt unverändert.",
                    "PNG, JPEG, WebP or BMP; animated = first frame. A copy (at most 3840 px) goes into the theme's folder, the original stays unchanged.",
                )).clicked() {
                    pick(&mut ed.bg, Target::Image);
                }
                if b.image.is_some() && ui.button(lang.t("Entfernen", "Remove")).clicked() {
                    b.image = None;
                }
            });
            egui::Grid::new("pd_bg_img").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(lang.t("Skalierung", "Scaling"));
                egui::ComboBox::from_id_salt("pd_bg_mode").selected_text(b.mode.label(lang)).show_ui(ui, |ui| {
                    for m in Mode::ALL {
                        ui.selectable_value(&mut b.mode, m, m.label(lang));
                    }
                });
                ui.end_row();
                ui.label(lang.t("Anker", "Anchor"));
                egui::Grid::new("pd_bg_anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                    for (i, a) in Anchor::ALL.into_iter().enumerate() {
                        let sel = b.anchor == a;
                        if ui.add(egui::Button::new(if sel { "●" } else { "○" }).min_size(Vec2::splat(20.0)).selected(sel)).on_hover_text(a.key()).clicked() {
                            b.anchor = a;
                        }
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
                ui.end_row();
            });
        }
        Kind::Color => {
            let cur = b.color.clone().unwrap_or(Spec::Link { to: "bg".into(), alpha: None });
            if let Some(s) = spec_edit(ui, &mut ed.bg, "color", &cur, &t, lang) {
                b.color = Some(s);
            }
        }
        Kind::Gradient => {
            let g = b.gradient.get_or_insert_with(Default::default);
            ui.horizontal(|ui| {
                ui.radio_value(&mut g.kind, GradKind::Linear, lang.t("Linear", "Linear"));
                ui.radio_value(&mut g.kind, GradKind::Radial, lang.t("Radial", "Radial"));
            });
            if g.kind == GradKind::Linear {
                merge |= ui.add(egui::Slider::new(&mut g.angle, 0..=359).suffix("°").text(lang.t("Winkel", "Angle"))).dragged();
            } else {
                merge |= ui.add(egui::Slider::new(&mut g.center[0], 0..=100).suffix(" %").text(lang.t("Mitte x", "Centre x"))).dragged();
                merge |= ui.add(egui::Slider::new(&mut g.center[1], 0..=100).suffix(" %").text(lang.t("Mitte y", "Centre y"))).dragged();
                merge |= ui.add(egui::Slider::new(&mut g.radius, 1..=200).suffix(" %").text(lang.t("Radius", "Radius"))).dragged();
            }
            let sel = &mut ed.bg.sel_stop;
            *sel = (*sel).min(g.stops.len().saturating_sub(1));
            if stop_bar(ui, g, &t, sel) {
                merge = true;
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(g.stops.len() < 4, egui::Button::new("+")).on_hover_text(lang.t("Stopp hinzufügen (höchstens 4)", "Add a stop (at most 4)")).clicked() {
                    let pos = 50;
                    let color = Spec::Hex(sample(g, &t, pos as f32));
                    g.stops.push(Stop { pos, color });
                    *sel = g.stops.len() - 1;
                }
                if ui.add_enabled(g.stops.len() > 2, egui::Button::new("−")).on_hover_text(lang.t("Gewählten Stopp entfernen (mindestens 2)", "Remove the chosen stop (at least 2)")).clicked() {
                    g.stops.remove(*sel);
                    *sel = 0;
                }
                ui.label(RichText::new(format!("{} {} · {} %", lang.t("Stopp", "Stop"), *sel + 1, g.stops.get(*sel).map(|s| s.pos).unwrap_or(0))).small().color(style::dim(ui)));
            });
            let idx = *sel;
            if let Some(cur) = g.stops.get(idx).map(|s| s.color.clone()) {
                if let Some(s) = spec_edit(ui, &mut ed.bg, "stop", &cur, &t, lang) {
                    g.stops[idx].color = s;
                }
            }
        }
        Kind::Pattern => {
            let p = b.pattern.get_or_insert_with(Pattern::default);
            ui.horizontal_wrapped(|ui| {
                for n in PatternName::ALL {
                    if ui.selectable_label(p.image.is_none() && p.name == n, n.label(lang)).clicked() {
                        p.name = n;
                        p.image = None;
                    }
                }
            });
            let (label, ok) = file_label(&ed.bg, &t, p.image.as_deref(), lang);
            if p.image.is_some() {
                ui.label(RichText::new(label).small().color(if ok { style::dim(ui) } else { style::ui().warning }));
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(ed.bg.import.is_none(), egui::Button::new(lang.t("Eigene Kachel …", "Own tile …"))).clicked() {
                    pick(&mut ed.bg, Target::Tile);
                }
            });
            if p.image.is_none() {
                ui.label(lang.t("Musterfarbe", "Pattern colour"));
                let cur = p.color.clone();
                if let Some(s) = spec_edit(ui, &mut ed.bg, "pattern", &cur, &t, lang) {
                    p.color = s;
                }
            }
            merge |= ui.add(egui::Slider::new(&mut p.scale, 25..=400).suffix(" %").text(lang.t("Größe", "Size"))).dragged();
            ui.horizontal(|ui| {
                ui.label(lang.t("Versatz", "Offset"));
                ui.add(egui::DragValue::new(&mut p.offset[0]).range(-2000..=2000).prefix("x "));
                ui.add(egui::DragValue::new(&mut p.offset[1]).range(-2000..=2000).prefix("y "));
            });
        }
    }
    ui.separator();
    ui.label(RichText::new(lang.t("Abstand (oben, rechts, unten, links)", "Margin (top, right, bottom, left)")).small().color(style::dim(ui)));
    ui.horizontal(|ui| {
        for m in b.margin.iter_mut() {
            ui.add(egui::DragValue::new(m).range(0..=2000).suffix(" pt"));
        }
    });
    merge |= ui.add(egui::Slider::new(&mut b.opacity, 0..=100).suffix(" %").text(lang.t("Deckkraft", "Opacity"))).on_hover_text(lang.t("Unter 100 % scheint der Desktop durch", "Below 100 % the desktop shows through")).dragged();
    merge |= ui.add(egui::Slider::new(&mut b.brightness, -100..=100).suffix(" %").text(lang.t("Helligkeit", "Brightness"))).dragged();
    merge |= ui.add(egui::Slider::new(&mut b.blur, 0..=40).suffix(" pt").text(lang.t("Unschärfe", "Blur"))).dragged();
    let mut ov = b.overlay.is_some();
    if ui.checkbox(&mut ov, lang.t("Überlagerung", "Overlay")).on_hover_text(lang.t("Farbe über dem Bild, unter allen Flächen", "Colour above the image, below all surfaces")).changed() {
        b.overlay = ov.then(|| Overlay { color: Spec::Link { to: "bg".into(), alpha: None }, opacity: 40 });
    }
    if let Some(o) = b.overlay.as_mut() {
        let cur = o.color.clone();
        if let Some(s) = spec_edit(ui, &mut ed.bg, "overlay", &cur, &t, lang) {
            o.color = s;
        }
        merge |= ui.add(egui::Slider::new(&mut o.opacity, 0..=100).suffix(" %").text(lang.t("Deckkraft der Überlagerung", "Overlay opacity"))).dragged();
    }
    ui.separator();
    ui.label(RichText::new(lang.t("Bereiche (Deckkraft der Fläche über dem Hintergrund; aus = deckend)", "Areas (opacity of the surface above the background; off = opaque)")).small().color(style::dim(ui)));
    egui::Grid::new("pd_bg_areas").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for a in Area::ALL {
            let c = b.area_mut(a);
            ui.checkbox(&mut c.on, a.label(lang));
            merge |= ui.add_enabled(c.on, egui::Slider::new(&mut c.opacity, 0..=100).suffix(" %")).dragged();
            ui.end_row();
        }
    });
    ui.label(RichText::new(lang.t("Dialoge, Menüs, Tooltips und Eingabefelder bleiben immer deckend.", "Dialogs, menus, tooltips and input fields always stay opaque.")).small().color(style::dim(ui)));
    if b != before {
        let mut b = b;
        b.clear_invalid();
        ed.edit(now, merge, |t| t.background = Some(b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_draws_every_kind_without_panic() {
        let ctx = egui::Context::default();
        for k in Kind::ALL {
            let mut t = crate::theme::all()[0].clone();
            t.builtin = false;
            t.background = Some(Background::new(k));
            let mut e = Editor::new(t.clone(), t);
            e.others = vec![e.theme.clone()];
            for lang in [Lang::En, Lang::De] {
                let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 900.0))), ..Default::default() }, |ctx| {
                    tick_preview(&mut e, ctx);
                    egui::CentralPanel::default().show(ctx, |ui| panel(&mut e, ui, lang, 0.0));
                });
            }
            assert!(e.bg.preview.is_some() || e.bg.tex.is_some(), "{k:?}: Vorschau angefordert");
        }
    }

    #[test]
    fn pending_images_are_written_only_when_referenced() {
        let mut st = BgState::default();
        st.pending.push(Pending { name: "a.png".into(), bytes: Arc::new(vec![1]), src: Src::Mem { id: 1, img: Arc::new(image::RgbaImage::new(1, 1)) } });
        st.pending.push(Pending { name: "old.png".into(), bytes: Arc::new(vec![2]), src: Src::Mem { id: 2, img: Arc::new(image::RgbaImage::new(1, 1)) } });
        let mut t = crate::theme::all()[0].clone();
        let mut b = Background::new(Kind::Image);
        b.image = Some("a.png".into());
        t.background = Some(b);
        assert_eq!(st.files_for(&t), vec![("a.png".to_string(), vec![1u8])]);
        t.background = None;
        assert!(st.files_for(&t).is_empty());
        let g = background::Gradient::default();
        let mid = sample(&g, &t, 50.0);
        assert_ne!(mid, sample(&g, &t, 0.0));
    }
}
