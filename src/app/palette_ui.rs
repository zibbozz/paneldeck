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

pub fn color32(hex: &str) -> Color32 {
    let [r, g, b] = palette::rgb(hex);
    Color32::from_rgb(r, g, b)
}

fn hex32(c: Color32) -> String {
    palette::hex_of([c.r(), c.g(), c.b()])
}

/// Farbfelder der geltenden Palette von `area` (höchstens `per_row` je Zeile, Tooltip mit Name und Wert) und darunter
/// „Eigene Farbe …“ (freie Farbe, die nicht in die Palette kommt). Liefert die gewählte Farbe (`#RRGGBB`).
pub fn pick(ui: &mut Ui, salt: impl Hash, area: Area, current: Option<&str>, look: Look, per_row: usize, lang: Lang) -> Option<String> {
    let salt = Id::new(salt);
    let colors = palette::colors(area);
    let mut out = None;
    if colors.is_empty() {
        ui.label(RichText::new(lang.t("Die Palette ist leer (Einstellungen → Farben).", "The palette is empty (Settings → Colors).")).small().color(style::dim(ui)));
    }
    let size = if look == Look::Square { 18.0 } else { 22.0 };
    for (row, chunk) in colors.chunks(per_row.max(1)).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(4.0);
            for (k, c) in chunk.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
                let resp = ui.interact(r, salt.with(("pal", row * per_row + k)), Sense::click());
                let col = color32(&c.hex);
                let sel = current.is_some_and(|h| h.eq_ignore_ascii_case(&c.hex));
                match look {
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
    if let Some(h) = custom(ui, salt.with("custom"), current, lang) {
        out = Some(h);
    }
    out
}

/// „Eigene Farbe …“: klappt einen Farbwähler mit Hex-Feld auf; „Übernehmen“ liefert die Farbe (sie kommt nicht in die
/// Palette).
pub fn custom(ui: &mut Ui, salt: Id, current: Option<&str>, lang: Lang) -> Option<String> {
    let open_id = salt.with("open");
    let col_id = salt.with("col");
    let buf_id = salt.with("buf");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    let mut out = None;
    if ui.selectable_label(open, lang.t("Eigene Farbe …", "Custom color …")).on_hover_text(lang.t("Freie Farbe wählen – sie wird nicht in die Palette aufgenommen", "Pick any color – it is not added to the palette")).clicked() {
        open = !open;
        ui.data_mut(|d| {
            d.insert_temp(open_id, open);
            d.remove_temp::<Color32>(col_id);
            d.remove_temp::<String>(buf_id);
        });
    }
    if !open {
        return None;
    }
    let mut c = ui.data(|d| d.get_temp::<Color32>(col_id)).unwrap_or_else(|| current.map(color32).unwrap_or(Color32::from_rgb(0x80, 0x80, 0x80)));
    let mut buf = ui.data(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| hex32(c));
    if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
        buf = hex32(c);
    }
    ui.horizontal(|ui| {
        let (sw, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        ui.painter().rect_filled(sw, 4.0, c);
        let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(80.0).char_limit(7).font(egui::TextStyle::Monospace));
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
        ui.data_mut(|d| {
            d.insert_temp(open_id, false);
            d.remove_temp::<Color32>(col_id);
            d.remove_temp::<String>(buf_id);
        });
    } else {
        ui.data_mut(|d| {
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
            "Änderungen gelten nur für künftige Auswahlen – vorhandene Sitzungen, Tags, Typen usw. behalten ihre Farbe.",
            "Changes only apply to future choices – existing sessions, tags, types etc. keep their color.",
        ))
        .small()
        .color(style::dim(ui)),
    );
    ui.add_space(S2);
    let defaults = which.defaults();
    list_editor(ui, salt.with(which_key(which)), p.get_mut(which), defaults, lang);
}

fn which_key(w: Which) -> u8 {
    match w {
        Which::Shared => 0,
        Which::Area(Area::Hub) => 1,
        Which::Area(Area::Todo) => 2,
        Which::Area(Area::Expert) => 3,
    }
}

/// Liste einer Palette bearbeiten.
fn list_editor(ui: &mut Ui, salt: Id, list: &mut Vec<PaletteColor>, defaults: &[&str], lang: Lang) {
    let drag_id = salt.with("drag");
    let msg_id = salt.with("msg");
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp::<usize>(drag_id)).filter(|i| *i < list.len());
    if list.is_empty() {
        ui.label(RichText::new(lang.t(
            "Die Palette ist leer. Automatisch vergebene Farben kommen dann aus den eingebauten Vorgaben; die Auswahl bietet nur „Eigene Farbe …“.",
            "The palette is empty. Automatically assigned colors then come from the built-in defaults; pickers only offer “Custom color …”.",
        )).color(style::dim(ui)));
    }
    let mut rects = Vec::with_capacity(list.len());
    let mut remove = None;
    for i in 0..list.len() {
        let row = ui.horizontal(|ui| {
            // Griff zum Ziehen
            let (hr, _) = ui.allocate_exact_size(Vec2::new(16.0, 26.0), Sense::hover());
            let hresp = ui.interact(hr, salt.with(("handle", i)), Sense::drag());
            let hc = if hresp.hovered() || dragging == Some(i) { ui.visuals().strong_text_color() } else { style::dim(ui) };
            for k in -1..=1 {
                let y = hr.center().y + k as f32 * 4.0;
                ui.painter().line_segment([egui::pos2(hr.left() + 3.0, y), egui::pos2(hr.right() - 3.0, y)], Stroke::new(1.5_f32, hc));
            }
            if hresp.drag_started() {
                dragging = Some(i);
            }
            hresp.on_hover_cursor(CursorIcon::Grab).on_hover_text(lang.t("Ziehen zum Sortieren", "Drag to reorder"));
            // Farbfeld: öffnet den Farbwähler
            let (sr, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            let sresp = ui.interact(sr, salt.with(("swatch", i)), Sense::click());
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
            let id = salt.with(("hex", i));
            let buf_id = salt.with(("hexbuf", i));
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
            ui.add(egui::TextEdit::singleline(&mut list[i].name).id(salt.with(("name", i))).hint_text(lang.t("Name (optional)", "Name (optional)")).desired_width(150.0).char_limit(palette::MAX_NAME_CHARS));
            if style::icon_btn_small(ui, SIcon::Trash, lang.t("Farbe aus der Palette löschen", "Delete color from the palette")).clicked() {
                remove = Some(i);
            }
            if let Some(t) = problem {
                ui.label(RichText::new(t).small().color(style::DANGER_SOFT));
            }
        });
        rects.push(row.response.rect);
    }
    // Ziehen: Einfügemarke zeigen, beim Loslassen verschieben.
    if let Some(from) = dragging {
        let pos = ui.ctx().pointer_interact_pos();
        let before = pos.map(|p| rects.iter().position(|r| p.y < r.center().y).unwrap_or(rects.len()));
        if let (Some(b), Some(last)) = (before, rects.last()) {
            let y = if b < rects.len() { rects[b].top() - 1.0 } else { last.bottom() + 1.0 };
            ui.painter().line_segment([egui::pos2(last.left(), y), egui::pos2(last.right(), y)], Stroke::new(2.0_f32, style::accent()));
            ui.painter().rect_filled(rects[from], 4.0, style::accent().gamma_multiply(0.12));
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            if let Some(b) = before {
                palette::move_before(list, from, b);
            }
            dragging = None;
        }
    }
    ui.data_mut(|d| match dragging {
        Some(i) => d.insert_temp(drag_id, i),
        None => {
            d.remove_temp::<usize>(drag_id);
        }
    });
    if let Some(i) = remove {
        list.remove(i);
    }
    ui.add_space(S2);
    ui.horizontal(|ui| {
        if ui.add_enabled(list.len() < palette::MAX_COLORS, egui::Button::new(format!("+ {}", lang.t("Farbe hinzufügen", "Add color")))).clicked() {
            list.push(PaletteColor::new(&palette::suggest_new(list, defaults)));
            ui.data_mut(|d| d.remove_temp::<String>(msg_id));
        }
        if ui
            .button(lang.t("Vorgaben wiederherstellen", "Restore defaults"))
            .on_hover_text(lang.t("Gelöschte Vorgabefarben wieder anhängen; deine Farben und Namen bleiben.", "Re-add deleted default colors at the end; your colors and names stay."))
            .clicked()
        {
            let n = palette::restore_defaults(list, defaults);
            let m = if n == 0 {
                lang.t("Alle Vorgaben sind schon da.", "All defaults are already there.").to_string()
            } else {
                format!("{} {n}", lang.t("Wiederhergestellt:", "Restored:"))
            };
            ui.data_mut(|d| d.insert_temp(msg_id, m));
        }
        if let Some(m) = ui.data(|d| d.get_temp::<String>(msg_id)) {
            ui.label(RichText::new(m).small().color(style::dim(ui)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};

    struct H {
        ctx: egui::Context,
        time: f64,
    }

    impl H {
        fn new() -> Self {
            Self { ctx: egui::Context::default(), time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>, mut f: impl FnMut(&mut Ui)) -> egui::FullOutput {
            self.time += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 1400.0))), time: Some(self.time), events, focused: true, ..Default::default() };
            self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| f(ui));
            })
        }

        fn click(&mut self, p: Pos2, mut f: impl FnMut(&mut Ui)) {
            self.frame(vec![Event::PointerMoved(p)], &mut f);
            self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], &mut f);
            self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], &mut f);
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

    /// Rechteck eines Elements der gemeinsamen Liste (`what` = "handle", "hex", "name", "swatch").
    fn row_rect(h: &H, what: &'static str, i: usize) -> Rect {
        h.ctx.read_response(Id::new("palette_settings").with(0u8).with((what, i))).unwrap_or_else(|| panic!("{what} {i}")).rect
    }

    fn handle(h: &H, i: usize) -> Pos2 {
        row_rect(h, "handle", i).center()
    }

    #[test]
    fn management_view_adds_restores_switches_and_reorders() {
        for lang in [Lang::En, Lang::De] {
            let mut p = Palettes::default();
            let mut h = H::new();
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            let (add, restore) = match lang {
                Lang::En => ("+ Add color", "Restore defaults"),
                Lang::De => ("+ Farbe hinzufügen", "Vorgaben wiederherstellen"),
            };
            assert!(has(&out, add) && has(&out, restore));
            assert!(has(&out, if lang == Lang::En { "Separate palette per area" } else { "Eigene Palette je Bereich" }));

            // Löschen: Papierkorb der ersten Zeile (rechts neben dem Namen)
            let first = p.shared[0].clone();
            let name = row_rect(&h, "name", 0);
            let trash = Pos2::new(name.right() + 8.0 + 11.0, name.center().y);
            h.click(trash, |ui| settings_view(ui, &mut p, lang));
            assert!(!p.shared.contains(&first), "gelöscht");
            assert_eq!(p.shared.len(), palette::SHARED_DEFAULTS.len() - 1);
            // Vorgaben wiederherstellen hängt sie wieder an
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            let pos = find(&out, restore);
            h.click(pos, |ui| settings_view(ui, &mut p, lang));
            assert_eq!(p.shared.last(), Some(&first));
            assert_eq!(p.shared.len(), palette::SHARED_DEFAULTS.len());
            // Hinzufügen: eine neue, noch nicht vorhandene Farbe
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            h.click(find(&out, add), |ui| settings_view(ui, &mut p, lang));
            assert_eq!(p.shared.len(), palette::SHARED_DEFAULTS.len() + 1);
            let new = p.shared.last().unwrap().hex.clone();
            assert_eq!(p.shared.iter().filter(|c| c.hex == new).count(), 1);

            // Ziehen: Zeile 0 hinter Zeile 2
            let a = p.shared[0].clone();
            let b = p.shared[1].clone();
            h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            let start = handle(&h, 0);
            let target = handle(&h, 2) + Vec2::new(0.0, 8.0);
            let steps = vec![
                vec![Event::PointerMoved(start)],
                vec![Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
                vec![Event::PointerMoved(start + Vec2::new(0.0, 6.0))],
                vec![Event::PointerMoved(start + Vec2::new(0.0, 20.0))],
                vec![Event::PointerMoved(target)],
                vec![Event::PointerButton { pos: target, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }],
            ];
            for ev in steps {
                h.frame(ev, |ui| settings_view(ui, &mut p, lang));
            }
            assert_eq!(p.shared[0], b, "die zweite rückt nach vorn");
            assert_eq!(p.shared[2], a, "die erste steht jetzt hinter der dritten");

            // Umschalten auf je Bereich: Reiter erscheinen, die gemeinsame bleibt
            let shared = p.shared.clone();
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            let label = find_rect(&out, if lang == Lang::En { "Separate palette per area" } else { "Eigene Palette je Bereich" });
            let toggle = Pos2::new(label.left() - 8.0 - 18.0, label.center().y);
            h.click(toggle, |ui| settings_view(ui, &mut p, lang));
            assert!(p.per_area);
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            let todo = find(&out, Area::Todo.label(lang));
            assert!(has(&out, "Terminal Hub") && has(&out, Area::Expert.label(lang)));
            h.click(todo, |ui| settings_view(ui, &mut p, lang));
            let out = h.frame(vec![], |ui| settings_view(ui, &mut p, lang));
            h.click(find(&out, add), |ui| settings_view(ui, &mut p, lang));
            assert_eq!(p.todo.len(), palette::TODO_DEFAULTS.len() + 1, "„Hinzufügen“ wirkt auf den gewählten Bereich");
            assert_eq!(p.shared, shared);
            // zurück: die Bereichspalette bleibt gespeichert
            h.click(toggle, |ui| settings_view(ui, &mut p, lang));
            assert!(!p.per_area);
            assert_eq!(p.todo.len(), palette::TODO_DEFAULTS.len() + 1);
        }
    }

    #[test]
    fn hex_field_edits_a_color_and_rejects_duplicates() {
        let mut p = Palettes::default();
        p.shared = vec![PaletteColor::new("#111111"), PaletteColor::new("#222222")];
        let mut h = H::new();
        h.frame(vec![], |ui| settings_view(ui, &mut p, Lang::En));
        let field = h.ctx.read_response(Id::new("palette_settings").with(0u8).with(("hex", 1usize))).unwrap().rect.center();
        h.click(field, |ui| settings_view(ui, &mut p, Lang::En));
        // alles markieren und ersetzen
        let sel = Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND };
        h.frame(vec![sel.clone()], |ui| settings_view(ui, &mut p, Lang::En));
        h.frame(vec![Event::Text("#111111".into())], |ui| settings_view(ui, &mut p, Lang::En));
        let out = h.frame(vec![], |ui| settings_view(ui, &mut p, Lang::En));
        assert_eq!(p.shared[1].hex, "#222222", "Doppelte werden nicht übernommen");
        assert!(has(&out, "already in the palette"));
        h.frame(vec![sel], |ui| settings_view(ui, &mut p, Lang::En));
        h.frame(vec![Event::Text("abcdef".into())], |ui| settings_view(ui, &mut p, Lang::En));
        assert_eq!(p.shared[1].hex, "#ABCDEF");
    }

    #[test]
    fn picker_offers_palette_with_tooltips_and_a_custom_color() {
        // ohne Veröffentlichung: gemeinsame Vorgaben
        let mut h = H::new();
        let mut got: Option<String> = None;
        let out = h.frame(vec![], |ui| {
            if let Some(c) = pick(ui, "t", Area::Hub, Some(palette::SHARED_DEFAULTS[1]), Look::Square, 10, Lang::De) {
                got = Some(c);
            }
        });
        assert!(has(&out, "Eigene Farbe …"));
        // erstes Farbfeld anklicken
        let sw = h.ctx.read_response(Id::new("t").with(("pal", 0usize))).unwrap().rect.center();
        h.click(sw, |ui| {
            if let Some(c) = pick(ui, "t", Area::Hub, None, Look::Square, 10, Lang::De) {
                got = Some(c);
            }
        });
        assert_eq!(got.as_deref(), Some(palette::SHARED_DEFAULTS[0]));
        // Tooltip zeigt den Wert
        h.frame(vec![Event::PointerMoved(sw)], |ui| {
            pick(ui, "t", Area::Hub, None, Look::Square, 10, Lang::En);
        });
        let mut tip = false;
        for _ in 0..15 {
            let out = h.frame(vec![], |ui| {
                pick(ui, "t", Area::Hub, None, Look::Square, 10, Lang::En);
            });
            tip |= has(&out, palette::SHARED_DEFAULTS[0]);
        }
        assert!(tip, "Tooltip mit dem Hex-Wert");

        // „Eigene Farbe …“ öffnen, Hex eintippen, übernehmen
        got = None;
        let run = |h: &mut H, ev: Vec<Event>, got: &mut Option<String>| {
            h.frame(ev, |ui| {
                if let Some(c) = pick(ui, "t", Area::Hub, None, Look::Square, 10, Lang::En) {
                    *got = Some(c);
                }
            })
        };
        let out = run(&mut h, vec![], &mut got);
        let c = find(&out, "Custom color …");
        for ev in [vec![Event::PointerMoved(c)], vec![Event::PointerButton { pos: c, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], vec![Event::PointerButton { pos: c, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]] {
            run(&mut h, ev, &mut got);
        }
        let out = run(&mut h, vec![], &mut got);
        let apply = find(&out, "Apply");
        // Hex-Feld links neben „Übernehmen“
        let field = apply - Vec2::new(70.0, 0.0);
        for ev in [vec![Event::PointerMoved(field)], vec![Event::PointerButton { pos: field, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], vec![Event::PointerButton { pos: field, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]] {
            run(&mut h, ev, &mut got);
        }
        run(&mut h, vec![Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }], &mut got);
        run(&mut h, vec![Event::Text("#2288FF".into())], &mut got);
        let out = run(&mut h, vec![], &mut got);
        let apply = find(&out, "Apply");
        for ev in [vec![Event::PointerMoved(apply)], vec![Event::PointerButton { pos: apply, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], vec![Event::PointerButton { pos: apply, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]] {
            run(&mut h, ev, &mut got);
        }
        assert_eq!(got.as_deref(), Some("#2288FF"));
        assert!(!palette::colors(Area::Hub).iter().any(|c| c.hex == "#2288FF"), "eigene Farbe kommt nicht in die Palette");
        let out = run(&mut h, vec![], &mut got);
        assert!(!has(&out, "Apply"), "nach dem Übernehmen klappt der Wähler zu");
    }
}
