//! Tests des Icon-Explorers, des Pickers mit „Weitere Icons …“ und des Zeichnens von Lucide-Icons – ohne Fenster, ohne Maus
//! und Tastatur des Nutzers (egui läuft mit künstlichen Eingaben).

use super::icon_explorer::{self, icon_kind, ExplorerResult, IconExplorer, IconKind};
use super::widgets;
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, Vec2};

struct H {
    ctx: egui::Context,
    time: f64,
}

fn key_event(key: Key, pressed: bool) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
}

impl H {
    fn new(with_font: bool) -> H {
        let ctx = egui::Context::default();
        if with_font {
            let mut f = egui::FontDefinitions::default();
            icon_explorer::install_font(&mut f);
            ctx.set_fonts(f);
        }
        H { ctx, time: 0.0 }
    }

    fn frame(&mut self, events: Vec<Event>, mut add: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        self.time += 0.1;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))), time: Some(self.time), events, focused: true, ..Default::default() };
        self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| add(ui));
        })
    }

    /// Klick auf das Widget `id` (Lage nach jedem Bild neu gelesen, falls sich das Fenster noch einpendelt).
    fn click_id(&mut self, id: Id, mut add: impl FnMut(&mut egui::Ui)) {
        let mut at = cell_center(&self.ctx, id);
        for _ in 0..3 {
            let _ = self.frame(vec![Event::PointerMoved(at)], &mut add);
            let now = cell_center(&self.ctx, id);
            at = now;
        }
        self.click(at, add);
    }

    /// Klick an `pos`: Drücken in einem Bild, Loslassen im nächsten.
    fn click(&mut self, pos: Pos2, mut add: impl FnMut(&mut egui::Ui)) {
        let press = PointerButton::Primary;
        let _ = self.frame(vec![Event::PointerMoved(pos)], &mut add);
        let _ = self.frame(vec![Event::PointerButton { pos, button: press, pressed: true, modifiers: Modifiers::NONE }], &mut add);
        let _ = self.frame(vec![Event::PointerButton { pos, button: press, pressed: false, modifiers: Modifiers::NONE }], &mut add);
    }
}

fn each_shape<'a>(s: &'a Shape, f: &mut impl FnMut(&'a Shape)) {
    if let Shape::Vec(v) = s {
        for x in v {
            each_shape(x, f);
        }
    } else {
        f(s);
    }
}

/// Mitte eines gezeichneten Textes (z. B. einer Schaltfläche).
fn find_text(out: &egui::FullOutput, text: &str) -> Option<Pos2> {
    let mut found = None;
    for c in &out.shapes {
        each_shape(&c.shape, &mut |s| {
            if let Shape::Text(t) = s {
                if t.galley.text() == text {
                    found = Some(t.pos + t.galley.rect.center().to_vec2());
                }
            }
        });
    }
    found
}

fn cell_center(ctx: &egui::Context, id: Id) -> Pos2 {
    ctx.read_response(id).expect("Zelle nicht gezeichnet").rect.center()
}

// ------------------------------------------------------------------ Explorer

#[test]
fn explorer_search_filters_select_returns_the_name_and_escape_closes() {
    let mut h = H::new(true);
    let id = Id::new("explorer_test");
    let mut ex = IconExplorer::new(None);
    let mut last = ExplorerResult::Open;
    let total = ex.results().len();
    {
        // Erstes Bild: egui misst das neue Fenster nur aus (unsichtbar).
        let _ = h.frame(vec![], |ui| last = ex.show(ui.ctx(), id, Lang::En));
        let out = h.frame(vec![], |ui| last = ex.show(ui.ctx(), id, Lang::En));
        assert_eq!(last, ExplorerResult::Open);
        assert!(find_text(&out, "Icon explorer").is_some(), "Fenster fehlt");
        assert!(find_text(&out, &format!("{total} icons")).is_some());
    }
    // Das Suchfeld hat den Fokus: getippter Text filtert.
    let _ = h.frame(vec![Event::Text("alarm clock".into())], |ui| last = ex.show(ui.ctx(), id, Lang::En));
    assert_eq!(ex.query, "alarm clock");
    let r = ex.results();
    assert_eq!(r[0].name, "alarm-clock");
    assert!(r.len() < total && r.iter().all(|i| i.name.contains("alarm") || i.tags.iter().any(|t| t.contains("alarm"))));
    // Kein Icon gewählt: „Auswählen“ ist gesperrt, Enter tut nichts.
    let _ = h.frame(vec![key_event(Key::Enter, true), key_event(Key::Enter, false)], |ui| last = ex.show(ui.ctx(), id, Lang::En));
    assert_eq!(last, ExplorerResult::Open);
    // Zelle anklicken wählt, „Select“ liefert den Namen.
    h.click_id(IconExplorer::cell_id(id, "alarm-clock"), |ui| last = ex.show(ui.ctx(), id, Lang::En));
    assert_eq!(ex.selected.as_deref(), Some("alarm-clock"));
    assert_eq!(last, ExplorerResult::Open);
    let out = h.frame(vec![], |ui| last = ex.show(ui.ctx(), id, Lang::En));
    let select = find_text(&out, "Select").expect("Schaltfläche Select");
    assert!(find_text(&out, "lucide:alarm-clock").is_some(), "Vorschau zeigt den gespeicherten Wert");
    let mut picked = None;
    h.click(select, |ui| {
        let r = ex.show(ui.ctx(), id, Lang::En);
        if r != ExplorerResult::Open {
            picked = Some(r);
        }
    });
    assert_eq!(picked, Some(ExplorerResult::Picked("alarm-clock".into())));

    // Esc schließt (Abbrechen), auch mit Fokus im Suchfeld.
    let mut ex = IconExplorer::new(Some("lucide:alarm-clock"));
    assert_eq!(ex.selected.as_deref(), Some("alarm-clock"), "aktuelles Icon ist vorgewählt");
    let _ = h.frame(vec![], |ui| last = ex.show(ui.ctx(), id, Lang::De));
    let _ = h.frame(vec![key_event(Key::Escape, true), key_event(Key::Escape, false)], |ui| last = ex.show(ui.ctx(), id, Lang::De));
    assert_eq!(last, ExplorerResult::Cancelled);

    // Kategorie filtert; Enter wählt das gewählte Icon.
    let mut ex = IconExplorer::new(Some("lucide:alarm-clock"));
    ex.category = Some("weather".into());
    assert!(ex.results().iter().all(|i| i.categories.contains(&"weather")));
    let _ = h.frame(vec![], |ui| last = ex.show(ui.ctx(), id, Lang::De));
    let out = h.frame(vec![], |ui| last = ex.show(ui.ctx(), id, Lang::De));
    for t in ["Icon-Explorer", "Alle", "Barrierefreiheit", "Auswählen", "Abbrechen"] {
        assert!(find_text(&out, t).is_some(), "{t}");
    }
    let _ = h.frame(vec![key_event(Key::Enter, true), key_event(Key::Enter, false)], |ui| last = ex.show(ui.ctx(), id, Lang::De));
    assert_eq!(last, ExplorerResult::Picked("alarm-clock".into()));
}

// ------------------------------------------------------------------ Picker

fn ui_fn<'a>(cur: &'a mut Option<String>, ch: &'a mut bool) -> impl FnMut(&mut egui::Ui) + 'a {
    move |ui: &mut egui::Ui| {
        *ch |= widgets::icon_picker(ui, "pick", cur, Color32::RED, Lang::En);
    }
}

#[test]
fn picker_button_opens_the_explorer_and_stores_a_lucide_value() {
    let mut h = H::new(true);
    let explorer = Id::new("pick").with("icon_explorer");
    let mut current = Some("bug".to_string());
    let mut changed = false;
    let out = h.frame(vec![], ui_fn(&mut current, &mut changed));
    // Eingebaute Icons stehen direkt im Picker, Lucide nur im Explorer.
    assert!(h.ctx.read_response(Id::new("pick").with("bug")).is_some());
    assert!(h.ctx.read_response(Id::new("pick").with("current_extra")).is_none());
    let more = find_text(&out, "More icons …").expect("Knopf „More icons …“");
    assert!(find_text(&out, "Icon explorer").is_none());
    h.click(more, ui_fn(&mut current, &mut changed));
    assert!(icon_explorer::explorer_open(&h.ctx, explorer));
    let _ = h.frame(vec![], ui_fn(&mut current, &mut changed));
    let _ = h.frame(vec![Event::Text("alarm-clock".into())], ui_fn(&mut current, &mut changed));
    let st: IconExplorer = h.ctx.data_mut(|d| d.get_temp(explorer)).unwrap();
    assert_eq!(st.query, "alarm-clock", "Suchfeld hat den Fokus");
    let _ = h.frame(vec![], ui_fn(&mut current, &mut changed));
    h.click_id(IconExplorer::cell_id(explorer, "alarm-clock"), ui_fn(&mut current, &mut changed));
    let out = h.frame(vec![], ui_fn(&mut current, &mut changed));
    assert!(!changed, "erst „Select“ übernimmt");
    let select = find_text(&out, "Select").unwrap();
    h.click(select, ui_fn(&mut current, &mut changed));
    assert!(changed);
    assert_eq!(current.as_deref(), Some("lucide:alarm-clock"));
    assert!(!icon_explorer::explorer_open(&h.ctx, explorer));
    // Das gewählte Lucide-Icon steht als markierte Zusatz-Zelle im Picker.
    let _ = h.frame(vec![], ui_fn(&mut current, &mut changed));
    assert!(h.ctx.read_response(Id::new("pick").with("current_extra")).is_some());

    // Wieder öffnen und mit Esc schließen: nichts ändert sich.
    changed = false;
    let out = h.frame(vec![], ui_fn(&mut current, &mut changed));
    h.click(find_text(&out, "More icons …").unwrap(), ui_fn(&mut current, &mut changed));
    assert!(icon_explorer::explorer_open(&h.ctx, explorer));
    let _ = h.frame(vec![key_event(Key::Escape, true), key_event(Key::Escape, false)], ui_fn(&mut current, &mut changed));
    assert!(!icon_explorer::explorer_open(&h.ctx, explorer));
    assert!(!changed && current.as_deref() == Some("lucide:alarm-clock"));

    // Ein eingebautes Icon wählen ersetzt das Lucide-Icon.
    let star = cell_center(&h.ctx, Id::new("pick").with("star"));
    h.click(star, ui_fn(&mut current, &mut changed));
    assert!(changed && current.as_deref() == Some("star"));
}

#[test]
fn required_picker_has_no_none_cell() {
    let mut h = H::new(true);
    let mut cur = "bug".to_string();
    let mut changed = false;
    let _ = h.frame(vec![], |ui| changed |= widgets::icon_picker_required(ui, "req", &mut cur, Color32::RED, Lang::De));
    assert!(h.ctx.read_response(Id::new("req").with("none")).is_none());
    let flag = cell_center(&h.ctx, Id::new("req").with("flag"));
    h.click(flag, |ui| changed |= widgets::icon_picker_required(ui, "req", &mut cur, Color32::RED, Lang::De));
    assert!(changed && cur == "flag");
}

// ------------------------------------------------------------------ Zeichnen

fn shape_count(out: &egui::FullOutput) -> usize {
    let mut n = 0;
    for cs in &out.shapes {
        each_shape(&cs.shape, &mut |_| n += 1);
    }
    n
}

/// Zeichnet ein Icon und liefert (Tinte des Lucide-Zeichens, Zahl der übrigen Formen des Icons).
fn paint(h: &mut H, name: &str, c: Pos2, size: f32) -> (Option<Rect>, usize) {
    let base = shape_count(&h.frame(vec![], |_| {}));
    let out = h.frame(vec![], |ui| icon_explorer::paint_icon(ui.painter(), name, c, size, Color32::WHITE));
    let mut ink = None;
    for cs in &out.shapes {
        each_shape(&cs.shape, &mut |s| {
            if let Shape::Text(t) = s {
                if t.galley.text().chars().all(|ch| ('\u{E000}'..='\u{F8FF}').contains(&ch)) {
                    let r = &t.galley.rows[0];
                    ink = Some(r.visuals.mesh_bounds.translate(t.pos.to_vec2() + r.pos.to_vec2()));
                }
            }
        });
    }
    (ink, shape_count(&out) - base - usize::from(ink.is_some()))
}

#[test]
fn lucide_icons_are_painted_centred_in_the_size_of_the_built_in_ones() {
    assert_eq!(icon_kind("bug"), IconKind::Builtin);
    assert_eq!(icon_kind("unknown-builtin"), IconKind::Builtin);
    assert!(matches!(icon_kind("lucide:alarm-clock"), IconKind::Lucide(_)));
    assert_eq!(icon_kind("lucide:not-an-icon"), IconKind::Placeholder);
    assert_eq!(icon_explorer::icon_label("lucide:alarm-clock"), "alarm-clock (Lucide)");
    assert_eq!(icon_explorer::icon_label("bug"), "bug");

    let mut h = H::new(true);
    let c = Pos2::new(300.0, 200.0);
    for (name, size) in [("lucide:circle", 32.0_f32), ("lucide:alarm-clock", 32.0), ("lucide:circle", 15.0)] {
        let (ink, _) = paint(&mut h, name, c, size);
        let ink = ink.unwrap_or_else(|| panic!("{name}: kein Zeichen gezeichnet"));
        assert!((ink.center() - c).length() <= 1.0, "{name} {size}: Mitte {:?} statt {c:?}", ink.center());
        // Die Tinte enthält je Seite bis zu einem Pixel Kantenglättung.
        let ratio = (ink.height().max(ink.width()) - 2.0) / size;
        assert!((0.75..=1.0).contains(&ratio), "{name} {size}: Größe {ratio}");
    }
    // Unbekanntes Lucide-Icon: Platzhalter statt Zeichen, kein Absturz.
    let (ink, other) = paint(&mut h, "lucide:not-an-icon", c, 32.0);
    assert!(ink.is_none() && other > 0);
    // Eingebaute Icons bleiben selbst gezeichnet.
    let (ink, other) = paint(&mut h, "bug", c, 32.0);
    assert!(ink.is_none() && other > 0);
    // Ohne geladene Lucide-Schrift (z. B. andere Kontexte): Platzhalter statt Panik.
    let mut bare = H::new(false);
    let (ink, other) = paint(&mut bare, "lucide:alarm-clock", c, 32.0);
    assert!(ink.is_none() && other > 0);
}

#[test]
fn every_lucide_icon_has_a_glyph_in_the_font() {
    let mut h = H::new(true);
    let _ = h.frame(vec![], |_| {});
    let fid = egui::FontId::new(16.0, egui::FontFamily::Name(icon_explorer::FONT_FAMILY.into()));
    let missing: Vec<&str> = h.ctx.fonts_mut(|f| crate::planning::icons::lucide::table().icons.iter().filter(|i| !f.has_glyph(&fid, i.ch)).map(|i| i.name).collect());
    assert!(missing.is_empty(), "ohne Zeichen: {missing:?}");
}
