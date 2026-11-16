//! Farbpaletten in der Oberfläche (seit 2.3.0, Issue #173): die Auswahl aus der Palette (mit Tooltip und
//! „Eigene Farbe …“) für alle Farbmenüs und die Verwaltung unter Einstellungen → Farben.
//!
//! Beides arbeitet ohne `App` (die Auswahl liest die veröffentlichte Palette `palette::colors`, die Verwaltung bekommt
//! `&mut Palettes`), damit es sich ohne Fenster testen lässt.

use crate::palette::{self, Area, PaletteColor, Palettes, Which};
use crate::state::Lang;
use crate::style::{self, Icon as SIcon, S1, S2};
use eframe::egui::{self, Color32, CursorIcon, Id, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::hash::Hash;

/// Index des Reiters „Farben“ in den Einstellungen (angehängt, damit die festen Indizes der übrigen gültig bleiben;
/// angezeigt wird er hinter „Darstellung“).
pub(super) const SETTINGS_TAB: usize = 11;

/// Form der Farbfelder.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Abgerundetes Quadrat (Terminal-Hub).
    Square,
    /// Kreis (Todo-Liste, Expertenmodus).
    Circle,
}

/// Darstellung einer Auswahl.
#[derive(Clone, Copy)]
pub struct PickOpts {
    pub look: Look,
    /// Farbfelder je Zeile.
    pub per_row: usize,
    /// „Eigene Farbe …“ anbieten (aus, wo daneben schon ein Hex-Feld steht).
    pub custom: bool,
}

impl PickOpts {
    /// Farbmenüs des Terminal-Hubs.
    pub const HUB: PickOpts = PickOpts { look: Look::Square, per_row: 10, custom: true };
    /// Tag-Farben der Todo-Liste.
    pub const TODO: PickOpts = PickOpts { look: Look::Circle, per_row: 12, custom: true };
    /// Expertenmodus: neben dem Hex-Feld (`widgets::palette_and_hex`), also ohne eigenen Farbwähler.
    pub const EXPERT_WITH_HEX: PickOpts = PickOpts { look: Look::Circle, per_row: 7, custom: false };
}

pub fn color32(hex: &str) -> Color32 {
    let [r, g, b] = palette::rgb(hex);
    Color32::from_rgb(r, g, b)
}

fn hex32(c: Color32) -> String {
    palette::hex_of([c.r(), c.g(), c.b()])
}

/// Untermenü „Farbe“ in einem Kontextmenü, das sich bei Klicks **in** ihm nicht schließt (sonst wäre „Eigene Farbe …“
/// nicht bedienbar). Wer gewählt hat, schließt das Menü selbst mit `ui.close()`.
pub fn color_menu<R>(ui: &mut Ui, text: &str, add: impl FnOnce(&mut Ui) -> R) {
    use egui::containers::menu::{is_in_menu, MenuButton, MenuConfig, SubMenuButton};
    let cfg = MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    if is_in_menu(ui) {
        SubMenuButton::new(text).config(cfg).ui(ui, add);
    } else {
        MenuButton::new(text).config(cfg).ui(ui, add);
    }
}

/// Farbfelder der geltenden Palette von `area` (Tooltip mit Name und Wert) und – wenn `opts.custom` – darunter
/// „Eigene Farbe …“ (freie Farbe, die nicht in die Palette kommt). Liefert die gewählte Farbe (`#RRGGBB`).
pub fn pick(ui: &mut Ui, salt: impl Hash, area: Area, current: Option<&str>, opts: PickOpts, lang: Lang) -> Option<String> {
    let salt = Id::new(salt);
    let colors = palette::colors(area);
    let mut out = None;
    if colors.is_empty() {
        ui.label(RichText::new(lang.t("Die Palette ist leer (Einstellungen → Farben).", "The palette is empty (Settings → Colors).")).small().color(style::dim(ui)));
    }
    let per_row = opts.per_row.max(1);
    let size = if opts.look == Look::Square { 18.0 } else { 22.0 };
    for (row, chunk) in colors.chunks(per_row).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(4.0);
            for (k, c) in chunk.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
                let resp = ui.interact(r, salt.with(("pal", row * per_row + k)), Sense::click());
                let col = color32(&c.hex);
                let sel = current.is_some_and(|h| h.eq_ignore_ascii_case(&c.hex));
                match opts.look {
                    Look::Square => {
                        ui.painter().rect_filled(r, 4.0, col);
                        if sel {
                            ui.painter().rect_stroke(r.expand(1.5), 5.0, Stroke::new(1.5_f32, ui.visuals().strong_text_color()), StrokeKind::Outside);
                        }
                    }
                    Look::Circle => {
                        ui.painter().circle_filled(r.center(), 8.5, col);
                        if sel {
                            ui.painter().circle_stroke(r.center(), 10.5, Stroke::new(1.8_f32, ui.visuals().strong_text_color()));
                        }
                    }
                }
                if resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(c.tooltip()).clicked() {
                    out = Some(c.hex.clone());
                }
            }
        });
    }
    if opts.custom {
        if let Some(h) = custom(ui, salt.with("custom"), current, lang) {
            out = Some(h);
        }
    }
    out
}

/// „Eigene Farbe …“: klappt einen Farbwähler mit Hex-Feld auf; „Übernehmen“ liefert die Farbe (sie kommt nicht in die
/// Palette). War die Auswahl ein Bild lang nicht zu sehen (Menü zu), ist sie beim nächsten Öffnen wieder zugeklappt.
pub fn custom(ui: &mut Ui, salt: Id, current: Option<&str>, lang: Lang) -> Option<String> {
    let open_id = salt.with("open");
    let seen_id = salt.with("seen");
    let col_id = salt.with("col");
    let buf_id = salt.with("buf");
    let pass = ui.ctx().cumulative_pass_nr();
    let seen = ui.data(|d| d.get_temp::<u64>(seen_id));
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false) && seen.is_some_and(|s| s + 1 >= pass);
    ui.data_mut(|d| d.insert_temp(seen_id, pass));
    let reset = |ui: &mut Ui, open: bool| {
        ui.data_mut(|d| {
            d.insert_temp(open_id, open);
            d.remove_temp::<Color32>(col_id);
            d.remove_temp::<String>(buf_id);
        })
    };
    if ui.selectable_label(open, lang.t("Eigene Farbe …", "Custom color …")).on_hover_text(lang.t("Freie Farbe wählen – sie wird nicht in die Palette aufgenommen", "Pick any color – it is not added to the palette")).clicked() {
        open = !open;
        reset(ui, open);
    }
    if !open {
        ui.data_mut(|d| d.insert_temp(open_id, false));
        return None;
    }
    let mut out = None;
    let mut c = ui.data(|d| d.get_temp::<Color32>(col_id)).unwrap_or_else(|| current.map(color32).unwrap_or(Color32::from_rgb(0x80, 0x80, 0x80)));
    let mut buf = ui.data(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| hex32(c));
    if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
        buf = hex32(c);
    }
    ui.horizontal(|ui| {
        let (sw, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        ui.painter().rect_filled(sw, 4.0, c);
        let r = ui.add(egui::TextEdit::singleline(&mut buf).id(salt.with("hex")).desired_width(80.0).char_limit(7).font(egui::TextStyle::Monospace));
        if r.changed() {
            if let Some(h) = palette::normalize_hex(&buf) {
                c = color32(&h);
            }
        }
        if ui.button(lang.t("Übernehmen", "Apply")).clicked() {
            out = Some(hex32(c));
        }
    });
    if out.is_some() {
        reset(ui, false);
    } else {
        ui.data_mut(|d| {
            d.insert_temp(open_id, true);
            d.insert_temp(col_id, c);
            d.insert_temp(buf_id, buf);
        });
    }
    out
}

// ------------------------------------------------------------------ Verwaltung (Einstellungen → Farben)

/// Die Verwaltung: Schalter „Eigene Palette je Bereich“, ggf. Bereichswahl, Liste der Farben (ziehen zum Sortieren,
/// Farbe/Hex/Name ändern, löschen), „Farbe hinzufügen“ und „Vorgaben wiederherstellen“.
pub fn settings_view(ui: &mut Ui, p: &mut Palettes, lang: Lang) {
    let salt = Id::new("palette_settings");
    ui.horizontal(|ui| {
        style::toggle(ui, &mut p.per_area);
        ui.label(lang.t("Eigene Palette je Bereich", "Separate palette per area"));
    });
    ui.label(
        RichText::new(lang.t(
            "Aus: eine gemeinsame Palette für Terminal Hub, Todo-Liste und Expertenmodus (auch Diagramme). An: jeder Bereich hat seine eigene. Beim Umschalten geht nichts verloren.",
            "Off: one shared palette for Terminal Hub, Todo list and Expert mode (including charts). On: each area has its own. Switching loses nothing.",
        ))
        .small()
        .color(style::dim(ui)),
    );
    ui.add_space(S2);
    let which = if p.per_area {
        let sel_id = salt.with("area");
        let mut sel = ui.data(|d| d.get_temp::<usize>(sel_id)).unwrap_or(0).min(2);
        ui.horizontal(|ui| {
            for (i, a) in Area::ALL.iter().enumerate() {
                if style::tab(ui, a.label(lang), sel == i).clicked() {
                    sel = i;
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(sel_id, sel));
        ui.add_space(S1);
        Which::Area(Area::ALL[sel])
    } else {
        Which::Shared
    };
    ui.label(
        RichText::new(lang.t(
            "Änderungen gelten nur für künftige Auswahlen – vorhandene Sitzungen, Tags, Typen usw. behalten ihre Farbe. Solange eine Palette unverändert ist, vergibt Paneldeck Farben automatisch wie bisher.",
            "Changes only apply to future choices – existing sessions, tags, types etc. keep their color. As long as a palette is unchanged, automatic colors are assigned as before.",
        ))
        .small()
        .color(style::dim(ui)),
    );
    ui.add_space(S2);
    // Auf einer Kopie arbeiten: Ansehen allein speichert nichts, eine Liste gleich den Vorgaben bleibt ungespeichert.
    let mut list = p.list(which);
    let before = list.clone();
    list_editor(ui, salt.with(which_key(which)), &mut list, which.defaults(), lang);
    if list != before {
        p.set(which, list);
    }
}

fn which_key(w: Which) -> u8 {
    match w {
        Which::Shared => 0,
        Which::Area(Area::Hub) => 1,
        Which::Area(Area::Todo) => 2,
        Which::Area(Area::Expert) => 3,
    }
}

/// Stabile Kennungen der Zeilen (damit Eingabefelder, Farbwähler und Ziehen nach Löschen/Sortieren an der richtigen
/// Farbe hängen). Liegen parallel zur Liste im Speicher von egui; passt die Länge nicht (Liste von außen geändert),
/// werden sie neu vergeben.
struct RowIds {
    id: Id,
    ids: Vec<u64>,
    next: u64,
}

impl RowIds {
    fn load(ui: &Ui, salt: Id, len: usize) -> Self {
        let id = salt.with("rowids");
        let (ids, next) = ui.data(|d| d.get_temp::<(Vec<u64>, u64)>(id)).unwrap_or_default();
        let mut r = RowIds { id, ids, next };
        if r.ids.len() != len {
            r.ids = (0..len).map(|_| r.fresh()).collect();
        }
        r
    }

    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn store(self, ui: &Ui) {
        ui.data_mut(|d| d.insert_temp(self.id, (self.ids, self.next)));
    }
}

/// Liste einer Palette bearbeiten.
fn list_editor(ui: &mut Ui, salt: Id, list: &mut Vec<PaletteColor>, defaults: &[&str], lang: Lang) {
    let drag_id = salt.with("drag");
    let msg_id = salt.with("msg");
    let mut rows = RowIds::load(ui, salt, list.len());
    // gezogen wird die Zeile mit dieser Kennung
    let mut dragging: Option<u64> = ui.data(|d| d.get_temp::<u64>(drag_id)).filter(|k| rows.ids.contains(k));
    if list.is_empty() {
        ui.label(RichText::new(lang.t(
            "Die Palette ist leer. Automatisch vergebene Farben kommen dann aus den eingebauten Vorgaben; die Auswahl bietet nur „Eigene Farbe …“.",
            "The palette is empty. Automatically assigned colors then come from the built-in defaults; pickers only offer “Custom color …”.",
        )).color(style::dim(ui)));
    }
    let mut rects = Vec::with_capacity(list.len());
    let mut remove = None;
    for i in 0..list.len() {
        let key = rows.ids[i];
        let row = ui.horizontal(|ui| {
            // Griff zum Ziehen
            let (hr, _) = ui.allocate_exact_size(Vec2::new(16.0, 26.0), Sense::hover());
            let hresp = ui.interact(hr, salt.with(("handle", key)), Sense::drag());
            let hc = if hresp.hovered() || dragging == Some(key) { ui.visuals().strong_text_color() } else { style::dim(ui) };
            for k in -1..=1 {
                let y = hr.center().y + k as f32 * 4.0;
                ui.painter().line_segment([egui::pos2(hr.left() + 3.0, y), egui::pos2(hr.right() - 3.0, y)], Stroke::new(1.5_f32, hc));
            }
            if hresp.drag_started() {
                dragging = Some(key);
            }
            hresp.on_hover_cursor(CursorIcon::Grab).on_hover_text(lang.t("Ziehen zum Sortieren", "Drag to reorder"));
            // Farbfeld: öffnet den Farbwähler
            let (sr, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            let sresp = ui.interact(sr, salt.with(("swatch", key)), Sense::click());
            ui.painter().rect_filled(sr, 5.0, color32(&list[i].hex));
            ui.painter().rect_stroke(sr, 5.0, Stroke::new(1.0_f32, Color32::from_white_alpha(60)), StrokeKind::Inside);
            let sresp = sresp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(list[i].tooltip());
            let others: Vec<String> = list.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, c)| c.hex.clone()).collect();
            egui::Popup::from_toggle_button_response(&sresp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                let mut c = color32(&list[i].hex);
                if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
                    let h = hex32(c);
                    if !others.contains(&h) {
                        list[i].hex = h;
                    }
                }
            });
            // Hex
            let id = salt.with(("hex", key));
            let buf_id = salt.with(("hexbuf", key));
            let focused = ui.memory(|m| m.has_focus(id));
            let mut buf: String = if focused { ui.data(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| list[i].hex.clone()) } else { list[i].hex.clone() };
            let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id).desired_width(76.0).char_limit(7).font(egui::TextStyle::Monospace));
            let mut problem = None;
            if r.has_focus() {
                match palette::normalize_hex(&buf) {
                    Some(h) if others.contains(&h) => problem = Some(lang.t("schon in der Palette", "already in the palette")),
                    Some(h) => list[i].hex = h,
                    None => problem = Some(lang.t("Form #RRGGBB", "Use #RRGGBB")),
                }
                ui.data_mut(|d| d.insert_temp(buf_id, buf));
            } else {
                ui.data_mut(|d| d.remove_temp::<String>(buf_id));
            }
            // Name
            ui.add(egui::TextEdit::singleline(&mut list[i].name).id(salt.with(("name", key))).hint_text(lang.t("Name (optional)", "Name (optional)")).desired_width(150.0).char_limit(palette::MAX_NAME_CHARS));
            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Farbe aus der Palette löschen", "Delete color from the palette")).clicked() {
                remove = Some(i);
            }
            if let Some(t) = problem {
                ui.label(RichText::new(t).small().color(style::DANGER_SOFT));
            }
        });
        rects.push(row.response.rect);
    }
    // Ziehen: Einfügemarke zeigen, nahe am Rand des sichtbaren Bereichs mitscrollen, beim Loslassen verschieben –
    // außerhalb der Liste losgelassen: nichts tun.
    if let Some(from) = dragging.and_then(|k| rows.ids.iter().position(|x| *x == k)) {
        let pos = ui.ctx().pointer_interact_pos();
        let area = rects.iter().fold(egui::Rect::NOTHING, |a, r| a.union(*r)).expand2(Vec2::new(40.0, 20.0));
        let inside = pos.is_some_and(|p| area.contains(p));
        let before = pos.filter(|_| inside).map(|p| rects.iter().position(|r| p.y < r.center().y).unwrap_or(rects.len()));
        if let (Some(b), Some(last)) = (before, rects.last()) {
            let y = if b < rects.len() { rects[b].top() - 1.0 } else { last.bottom() + 1.0 };
            ui.painter().line_segment([egui::pos2(last.left(), y), egui::pos2(last.right(), y)], Stroke::new(2.0_f32, style::accent()));
        }
        ui.painter().rect_filled(rects[from], 4.0, style::accent().gamma_multiply(0.12));
        if let Some(p) = pos {
            let clip = ui.clip_rect();
            let edge = 30.0;
            let dy = if p.y < clip.top() + edge { (clip.top() + edge - p.y).min(edge) } else if p.y > clip.bottom() - edge { -(p.y - (clip.bottom() - edge)).min(edge) } else { 0.0 };
            if dy != 0.0 {
                ui.scroll_with_delta(Vec2::new(0.0, dy * 0.5));
                ui.ctx().request_repaint();
            }
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            if let Some(b) = before {
                palette::move_before(list, from, b);
                palette::move_before(&mut rows.ids, from, b);
            }
            dragging = None;
        }
    } else {
        dragging = None;
    }
    ui.data_mut(|d| match dragging {
        Some(k) => d.insert_temp(drag_id, k),
        None => {
            d.remove_temp::<u64>(drag_id);
        }
    });
    if let Some(i) = remove {
        list.remove(i);
        rows.ids.remove(i);
    }
    ui.add_space(S2);
    ui.horizontal(|ui| {
        if ui.add_enabled(list.len() < palette::MAX_COLORS, egui::Button::new(format!("+ {}", lang.t("Farbe hinzufügen", "Add color")))).clicked() {
            list.push(PaletteColor::new(&palette::suggest_new(list, defaults)));
            let k = rows.fresh();
            rows.ids.push(k);
            ui.data_mut(|d| d.remove_temp::<String>(msg_id));
        }
        if ui
            .button(lang.t("Vorgaben wiederherstellen", "Restore defaults"))
            .on_hover_text(lang.t("Gelöschte Vorgabefarben wieder anhängen; deine Farben und Namen bleiben.", "Re-add deleted default colors at the end; your colors and names stay."))
            .clicked()
        {
            let n = palette::restore_defaults(list, defaults);
            for _ in 0..n {
                let k = rows.fresh();
                rows.ids.push(k);
            }
            let left = palette::missing_defaults(list, defaults);
            let m = if n == 0 && left == 0 {
                lang.t("Alle Vorgaben sind schon da.", "All defaults are already there.").to_string()
            } else if left > 0 {
                format!("{} {n} · {} {left} ({} {})", lang.t("Wiederhergestellt:", "Restored:"), lang.t("Palette voll, es fehlen", "palette full, missing"), lang.t("höchstens", "at most"), palette::MAX_COLORS)
            } else {
                format!("{} {n}", lang.t("Wiederhergestellt:", "Restored:"))
            };
            ui.data_mut(|d| d.insert_temp(msg_id, m));
        }
        if let Some(m) = ui.data(|d| d.get_temp::<String>(msg_id)) {
            ui.label(RichText::new(m).small().color(style::dim(ui)));
        }
    });
    rows.store(ui);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};

    struct H {
        ctx: egui::Context,
        time: f64,
        height: f32,
    }

    fn press(p: Pos2, pressed: bool, button: PointerButton) -> Event {
        Event::PointerButton { pos: p, button, pressed, modifiers: Modifiers::NONE }
    }

    impl H {
        fn new() -> Self {
            Self { ctx: egui::Context::default(), time: 0.0, height: 1400.0 }
        }

        fn frame(&mut self, events: Vec<Event>, mut f: impl FnMut(&mut Ui)) -> egui::FullOutput {
            self.time += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, self.height))), time: Some(self.time), events, focused: true, ..Default::default() };
            self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| f(ui));
            })
        }

        fn click_with(&mut self, p: Pos2, button: PointerButton, mut f: impl FnMut(&mut Ui)) -> egui::FullOutput {
            self.frame(vec![Event::PointerMoved(p)], &mut f);
            self.frame(vec![press(p, true, button)], &mut f);
            self.frame(vec![press(p, false, button)], &mut f);
            self.frame(vec![], &mut f)
        }

        fn click(&mut self, p: Pos2, f: impl FnMut(&mut Ui)) -> egui::FullOutput {
            self.click_with(p, PointerButton::Primary, f)
        }

        /// Kennung der Zeile `i` der gemeinsamen Liste.
        fn row_key(&self, i: usize) -> u64 {
            self.ctx.data(|d| d.get_temp::<(Vec<u64>, u64)>(Id::new("palette_settings").with(0u8).with("rowids"))).unwrap().0[i]
        }

        /// Rechteck eines Elements der Zeile `i` der gemeinsamen Liste (`what` = "handle", "hex", "name", "swatch").
        fn row_rect(&self, what: &'static str, i: usize) -> Rect {
            let k = self.row_key(i);
            self.ctx.read_response(Id::new("palette_settings").with(0u8).with((what, k))).unwrap_or_else(|| panic!("{what} {i}")).rect
        }
    }

    /// Alle gezeichneten Texte mit Position.
    fn texts(out: &egui::FullOutput) -> Vec<(String, Rect)> {
        fn walk(s: &egui::Shape, acc: &mut Vec<(String, Rect)>) {
            match s {
                egui::Shape::Text(t) => acc.push((t.galley.text().to_string(), t.visual_bounding_rect())),
                egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, acc)),
                _ => {}
            }
        }
        let mut acc = Vec::new();
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut acc));
        acc
    }

    fn find_rect(out: &egui::FullOutput, text: &str) -> Rect {
        texts(out).into_iter().find(|(t, _)| t == text).unwrap_or_else(|| panic!("{text} fehlt: {:?}", texts(out).iter().map(|t| &t.0).collect::<Vec<_>>())).1
    }

    fn find(out: &egui::FullOutput, text: &str) -> Pos2 {
        find_rect(out, text).center()
    }

    fn has(out: &egui::FullOutput, text: &str) -> bool {
        texts(out).iter().any(|(t, _)| t == text)
    }

    fn drag(h: &mut H, start: Pos2, target: Pos2, mut f: impl FnMut(&mut Ui)) {
        let steps = vec![
            vec![Event::PointerMoved(start)],
            vec![press(start, true, PointerButton::Primary)],
            vec![Event::PointerMoved(start + Vec2::new(0.0, 6.0))],
            vec![Event::PointerMoved(start + Vec2::new(0.0, 20.0))],
            vec![Event::PointerMoved(target)],
            vec![press(target, false, PointerButton::Primary)],
        ];
        for ev in steps {
            h.frame(ev, &mut f);
        }
        h.frame(vec![], &mut f);
    }

    #[test]
    fn management_view_adds_restores_switches_and_reorders() {
        for lang in [Lang::En, Lang::De] {
            let mut p = Palettes::default();
            let mut h = H::new();
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            assert!(p.is_default(), "Ansehen speichert nichts");
            let (add, restore) = match lang {
                Lang::En => ("+ Add color", "Restore defaults"),
                Lang::De => ("+ Farbe hinzufügen", "Vorgaben wiederherstellen"),
            };
            assert!(has(&out, add) && has(&out, restore));

            // Name der zweiten Zeile, dann die erste löschen: der Name bleibt an seiner Farbe (stabile Kennungen)
            let second = p.colors(Area::Hub)[1].hex.clone();
            let name = h.row_rect("name", 1).center();
            h.click(name, |ui| settings_view(ui, &mut p, lang));
            h.frame(vec![Event::Text("Grün".into())], |ui| settings_view(ui, &mut p, lang));
            assert_eq!(p.shared.as_ref().unwrap()[1].name, "Grün");
            let first = p.list(Which::Shared)[0].clone();
            let name0 = h.row_rect("name", 0);
            let trash = Pos2::new(name0.right() + 8.0 + 11.0, name0.center().y);
            h.click(trash, |ui| settings_view(ui, &mut p, lang));
            let l = p.list(Which::Shared);
            assert!(!l.contains(&first), "gelöscht");
            assert_eq!((l[0].hex.as_str(), l[0].name.as_str()), (second.as_str(), "Grün"));
            // Vorgaben wiederherstellen hängt sie wieder an
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            h.click(find(&out, restore), |ui| settings_view(ui, &mut p, lang));
            let l = p.list(Which::Shared);
            assert_eq!(l.last(), Some(&first));
            assert_eq!(l.len(), palette::SHARED_DEFAULTS.len());
            // Hinzufügen: eine neue, noch nicht vorhandene Farbe
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            h.click(find(&out, add), |ui| settings_view(ui, &mut p, lang));
            let l = p.list(Which::Shared);
            assert_eq!(l.len(), palette::SHARED_DEFAULTS.len() + 1);
            assert_eq!(l.iter().filter(|c| c.hex == l.last().unwrap().hex).count(), 1);

            // Ziehen: Zeile 0 hinter Zeile 2
            let (a, b) = (l[0].clone(), l[1].clone());
            let start = h.row_rect("handle", 0).center();
            let target = h.row_rect("handle", 2).center() + Vec2::new(0.0, 8.0);
            drag(&mut h, start, target, |ui| settings_view(ui, &mut p, lang));
            let l = p.list(Which::Shared);
            assert_eq!(l[0], b, "die zweite rückt nach vorn");
            assert_eq!(l[2], a, "die erste steht jetzt hinter der dritten");
            // außerhalb der Liste losgelassen: nichts passiert
            let start = h.row_rect("handle", 0).center();
            drag(&mut h, start, Pos2::new(start.x, 1350.0), |ui| settings_view(ui, &mut p, lang));
            assert_eq!(p.list(Which::Shared), l, "kein Sprung ans Ende");

            // Umschalten auf je Bereich: Reiter erscheinen, die gemeinsame bleibt
            let shared = p.shared.clone();
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            let label = find_rect(&out, if lang == Lang::En { "Separate palette per area" } else { "Eigene Palette je Bereich" });
            let toggle = Pos2::new(label.left() - 8.0 - 18.0, label.center().y);
            h.click(toggle, |ui| settings_view(ui, &mut p, lang));
            assert!(p.per_area);
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            assert!(has(&out, "Terminal Hub") && has(&out, Area::Expert.label(lang)));
            h.click(find(&out, Area::Todo.label(lang)), |ui| settings_view(ui, &mut p, lang));
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            h.click(find(&out, add), |ui| settings_view(ui, &mut p, lang));
            assert_eq!(p.todo.as_ref().map(|t| t.len()), Some(palette::TODO_DEFAULTS.len() + 1), "„Hinzufügen“ wirkt auf den gewählten Bereich");
            assert_eq!(p.hub, None, "andere Bereiche bleiben bei den Vorgaben");
            assert_eq!(p.shared, shared);
            h.click(toggle, |ui| settings_view(ui, &mut p, lang));
            assert!(!p.per_area);
            assert_eq!(p.todo.as_ref().map(|t| t.len()), Some(palette::TODO_DEFAULTS.len() + 1));
        }
    }

    #[test]
    fn hex_field_edits_a_color_and_rejects_duplicates() {
        let mut p = Palettes::default();
        p.shared = Some(vec![PaletteColor::new("#111111"), PaletteColor::new("#222222")]);
        let mut h = H::new();
        h.frame(vec![], |ui| settings_view(ui, &mut p, Lang::En));
        let field = h.row_rect("hex", 1).center();
        h.click(field, |ui| settings_view(ui, &mut p, Lang::En));
        let sel = Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND };
        h.frame(vec![sel.clone()], |ui| settings_view(ui, &mut p, Lang::En));
        h.frame(vec![Event::Text("#111111".into())], |ui| settings_view(ui, &mut p, Lang::En));
        let out = h.frame(vec![], |ui| settings_view(ui, &mut p, Lang::En));
        assert_eq!(p.shared.as_ref().unwrap()[1].hex, "#222222", "Doppelte werden nicht übernommen");
        assert!(has(&out, "already in the palette"));
        h.frame(vec![sel], |ui| settings_view(ui, &mut p, Lang::En));
        h.frame(vec![Event::Text("abcdef".into())], |ui| settings_view(ui, &mut p, Lang::En));
        assert_eq!(p.shared.as_ref().unwrap()[1].hex, "#ABCDEF");
    }

    #[test]
    fn restore_reports_a_full_palette() {
        let mut p = Palettes::default();
        p.shared = Some((0..palette::MAX_COLORS - 2).map(|i| PaletteColor::new(&format!("#0000{i:02X}"))).collect());
        let mut h = H::new();
        h.height = 3000.0;
        let out = h.frame(vec![], |ui| settings_view(ui, &mut p, Lang::En));
        let out2 = h.click(find(&out, "Restore defaults"), |ui| settings_view(ui, &mut p, Lang::En));
        assert_eq!(p.shared.as_ref().unwrap().len(), palette::MAX_COLORS);
        let left = palette::SHARED_DEFAULTS.len() - 2;
        assert!(has(&out2, &format!("Restored: 2 · palette full, missing {left} (at most {})", palette::MAX_COLORS)));
    }

    /// Ein Kontextmenü wie in der Seitenleiste: Rechtsklick auf „Ziel“, darin das Untermenü „Color“ mit der Auswahl.
    fn menu_ui(ui: &mut Ui, got: &mut Option<String>) {
        let r = ui.button("Ziel");
        r.context_menu(|ui| {
            let _ = ui.button("Rename");
            color_menu(ui, "Color", |ui| {
                if let Some(c) = pick(ui, "menu_pick", Area::Hub, None, PickOpts::HUB, Lang::En) {
                    *got = Some(c);
                    ui.close();
                }
            });
        });
    }

    fn open_color_menu(h: &mut H, got: &mut Option<String>) -> egui::FullOutput {
        let out = h.frame(vec![], |ui| menu_ui(ui, got));
        h.click_with(find(&out, "Ziel"), PointerButton::Secondary, |ui| menu_ui(ui, got));
        let out = h.frame(vec![], |ui| menu_ui(ui, got));
        let color = find(&out, "Color");
        h.frame(vec![Event::PointerMoved(color)], |ui| menu_ui(ui, got));
        h.frame(vec![], |ui| menu_ui(ui, got))
    }

    #[test]
    fn custom_color_works_inside_a_context_menu_and_swatches_close_it() {
        let mut got = None;
        let mut h = H::new();
        let out = open_color_menu(&mut h, &mut got);
        assert!(has(&out, "Custom color …"), "Untermenü offen");
        // „Eigene Farbe …“ aufklappen: das Menü bleibt offen
        let out = h.click(find(&out, "Custom color …"), |ui| menu_ui(ui, &mut got));
        assert!(has(&out, "Apply"), "Farbwähler aufgeklappt, Menü offen");
        // Hex eintippen (Feld links neben „Apply“), übernehmen
        let field = h.ctx.read_response(Id::new("menu_pick").with("custom").with("hex")).unwrap().rect.center();
        h.click(field, |ui| menu_ui(ui, &mut got));
        h.frame(vec![Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }], |ui| menu_ui(ui, &mut got));
        let out = h.frame(vec![Event::Text("#2288FF".into())], |ui| menu_ui(ui, &mut got));
        assert!(has(&out, "Apply"), "Tippen schließt das Menü nicht");
        let out = h.click(find(&out, "Apply"), |ui| menu_ui(ui, &mut got));
        assert_eq!(got.as_deref(), Some("#2288FF"));
        assert!(!palette::colors(Area::Hub).iter().any(|c| c.hex == "#2288FF"), "eigene Farbe kommt nicht in die Palette");
        assert!(!has(&out, "Color"), "nach dem Übernehmen ist das Menü zu");

        // Wieder öffnen: zugeklappt; ein Farbfeld wählt und schließt das Menü
        got = None;
        let out = open_color_menu(&mut h, &mut got);
        assert!(has(&out, "Custom color …") && !has(&out, "Apply"), "zugeklappt beim nächsten Öffnen");
        let sw = h.ctx.read_response(Id::new("menu_pick").with(("pal", 0usize))).unwrap().rect.center();
        let out = h.click(sw, |ui| menu_ui(ui, &mut got));
        assert_eq!(got.as_deref(), Some(palette::SHARED_DEFAULTS[0]));
        assert!(!has(&out, "Custom color …"), "Menü zu");
        // aufklappen, Menü per Klick daneben schließen, wieder öffnen: zugeklappt
        let out = open_color_menu(&mut h, &mut got);
        h.click(find(&out, "Custom color …"), |ui| menu_ui(ui, &mut got));
        let out = h.click(Pos2::new(800.0, 1300.0), |ui| menu_ui(ui, &mut got));
        assert!(!has(&out, "Custom color …"), "Klick daneben schließt");
        let out = open_color_menu(&mut h, &mut got);
        assert!(!has(&out, "Apply"));
    }

    #[test]
    fn picker_shows_tooltips_and_hides_custom_where_a_hex_field_exists() {
        let mut h = H::new();
        let out = h.frame(vec![], |ui| {
            pick(ui, "t", Area::Expert, None, PickOpts::EXPERT_WITH_HEX, Lang::En);
        });
        assert!(!has(&out, "Custom color …"));
        let out = h.frame(vec![], |ui| {
            pick(ui, "t", Area::Todo, None, PickOpts::TODO, Lang::De);
        });
        assert!(has(&out, "Eigene Farbe …"));
        let sw = h.ctx.read_response(Id::new("t").with(("pal", 0usize))).unwrap().rect.center();
        h.frame(vec![Event::PointerMoved(sw)], |ui| {
            pick(ui, "t", Area::Todo, None, PickOpts::TODO, Lang::En);
        });
        let mut tip = false;
        for _ in 0..15 {
            let out = h.frame(vec![], |ui| {
                pick(ui, "t", Area::Todo, None, PickOpts::TODO, Lang::En);
            });
            tip |= has(&out, palette::SHARED_DEFAULTS[0]);
        }
        assert!(tip, "Tooltip mit dem Hex-Wert");
    }
}
