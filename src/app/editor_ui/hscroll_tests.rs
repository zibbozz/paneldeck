//! Tests der waagerechten Bildlaufleiste (#389) ohne Fenster: Ein-/Ausblenden mit Umbruch und Breite, Griff ziehen,
//! Klick auf die Bahn, rechte Grenze, letzte Zeile frei, Cursor folgt, Messung je Stand (große Dokumente im
//! Hintergrund), Tabs und breite Zeichen, alle Teilungen.

use super::doc::DocEntry;
use super::hscroll::{self, BAR_H, MARGIN_COLS};
use super::text::{self, LONG_LINE, TextCx, View};
use super::view;
use super::{EditorPrefs, EditorUi};
use crate::editor::Document;
use crate::editor::text as et;
use crate::layout::Dir;
use eframe::egui::{self, Event, FontId, Id, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};

const FONT: f32 = 14.0;

/// Ein Textfeld ohne den Rest des Editors.
struct H {
    ctx: egui::Context,
    t: f64,
    d: DocEntry,
    view: View,
    wrap: bool,
    rect: Rect,
    prefs: EditorPrefs,
    mods: Modifiers,
}

impl H {
    fn new(text: &str) -> H {
        let mut d = DocEntry::untitled(1, None);
        d.doc = Some(Document::from_text(text));
        let mut h = H { ctx: egui::Context::default(), t: 0.0, d, view: View::default(), wrap: false, rect: Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(800.0, 400.0)), prefs: EditorPrefs::default(), mods: Modifiers::NONE };
        h.frames(2);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.t += 0.05;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(4000.0, 1000.0))), time: Some(self.t), events, modifiers: self.mods, ..Default::default() };
        let th = crate::theme::all()[0].clone();
        let (d, view, prefs, wrap, rect) = (&mut self.d, &mut self.view, &self.prefs, self.wrap, self.rect);
        let _ = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                let cx = TextCx { id: Id::new("hs_text"), font: FontId::monospace(FONT), theme: &th, prefs, wrap, minimap: false, request_focus: true, find: None, lang: crate::state::Lang::En };
                text::show(ui, rect, d, view, &cx);
            });
        });
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn doc(&self) -> &Document {
        self.d.doc.as_ref().unwrap()
    }

    fn bar(&self) -> Option<Rect> {
        self.view.hscroll.last_bar
    }

    /// Zeichenbreite und Zeilenhöhe der Schrift.
    fn metrics(&self) -> (f32, f32) {
        let f = FontId::monospace(FONT);
        self.ctx.fonts_mut(|x| (x.glyph_width(&f, 'M').max(1.0), x.row_height(&f)))
    }

    /// Breite des Textfelds: Leiste reicht vom Rand der Zeilennummern (6 px vor dem Text) bis 4 px hinter den Text.
    fn text_w(&self) -> f32 {
        self.bar().expect("Leiste sichtbar").width() - 10.0
    }

    /// Erwarteter größter Bildlauf für `cols` Spalten.
    fn max_for(&self, cols: usize) -> f32 {
        let (cw, _) = self.metrics();
        (cols as f32 + MARGIN_COLS) * cw - self.text_w()
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.frames(1);
    }

    fn press(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    }

    fn release(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    }

    fn click(&mut self, p: Pos2) {
        self.press(p);
        self.release(p);
        self.frames(1);
    }

    fn wheel(&mut self, p: Pos2, delta: Vec2) {
        self.frame(vec![Event::PointerMoved(p), Event::MouseWheel { unit: MouseWheelUnit::Point, delta, modifiers: self.mods }]);
        // Weiches Rollen verteilt den Weg auf mehrere Bilder.
        self.frames(40);
    }

    /// Warten, bis die Messung im Hintergrund fertig ist.
    fn settle(&mut self) {
        let start = Instant::now();
        loop {
            self.frame(vec![]);
            let tab = self.doc().tab_width().max(1);
            if !self.view.hscroll.measuring() && self.view.hscroll.is_current(self.doc(), tab) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "Messung im Hintergrund wird fertig");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.frames(1);
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.75
}

fn long_doc(width: usize, lines: usize) -> String {
    let mut s = String::new();
    for i in 0..lines {
        if i == lines / 2 {
            s.push_str(&"x".repeat(width));
        } else {
            s.push_str(&format!("line {i}"));
        }
        s.push('\n');
    }
    s
}

#[test]
fn bar_appears_only_without_wrap_and_for_lines_wider_than_the_view() {
    let h = H::new("short\nlines\nonly\n");
    assert!(h.bar().is_none(), "kurze Zeilen: keine Leiste");
    let mut h = H::new(&long_doc(300, 20));
    let bar = h.bar().expect("lange Zeile: Leiste");
    assert!(near(bar.height(), BAR_H) && near(bar.bottom(), h.rect.bottom()), "unten im Textfeld: {bar:?}");
    assert!(bar.right() <= h.rect.right() - 10.0 + 0.5, "lässt die senkrechte Leiste frei: {bar:?}");
    // Umbruch an: keine Leiste, aus: wieder da
    h.wrap = true;
    h.frames(2);
    assert!(h.bar().is_none(), "Umbruch: keine Leiste");
    h.wrap = false;
    h.frames(2);
    assert!(h.bar().is_some());
    // Gerollt und dann so breit, dass alles passt: Leiste weg, Bildlauf zurück auf 0
    h.view.scroll_x = 500.0;
    h.frames(1);
    assert!(h.view.scroll_x > 0.0);
    h.rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(3800.0, 400.0));
    h.frames(2);
    assert!(h.bar().is_none(), "breit genug: keine Leiste");
    assert_eq!(h.view.scroll_x, 0.0, "nichts mehr zu rollen");
    // Zeile kürzen, bis sie passt: Leiste verschwindet mit dem neuen Stand
    h.rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 400.0));
    h.frames(2);
    assert!(h.bar().is_some());
    let doc = h.d.doc.as_mut().unwrap();
    let line = 10; // die lange Zeile von `long_doc(300, 20)`
    assert_eq!(doc.line_len(line), 300);
    let start = doc.line_to_char(line);
    let mut sels = doc.new_selections(start);
    sels.set_primary(crate::editor::Selection::new(start + 10, start + 300));
    assert!(super::doc::run(doc, &mut sels, super::doc::Op::Backspace(false), 4), "gekürzt");
    h.frames(2);
    assert!(h.bar().is_none(), "nach dem Kürzen passt alles: keine Leiste");
}

#[test]
fn dragging_the_thumb_and_clicking_the_track_scroll_and_stop_at_the_longest_line() {
    let mut h = H::new(&long_doc(400, 40));
    let bar = h.bar().expect("Leiste");
    let max = h.max_for(400);
    assert!(max > 100.0);
    let y = bar.center().y;
    // Klick rechts vom Griff: eine Seite weiter, noch einmal: zwei Seiten
    let page = h.text_w() * 0.9;
    h.click(Pos2::new(bar.right() - 4.0, y));
    assert!(near(h.view.scroll_x, page.min(max)), "eine Seite: {} statt {page}", h.view.scroll_x);
    h.click(Pos2::new(bar.right() - 4.0, y));
    assert!(near(h.view.scroll_x, (2.0 * page).min(max)), "zwei Seiten: {}", h.view.scroll_x);
    // Klick links vom Griff: eine Seite zurück
    h.click(Pos2::new(bar.left() + 3.0, y));
    assert!(near(h.view.scroll_x, page.min(max)), "zurück: {}", h.view.scroll_x);
    h.view.scroll_x = 0.0;
    h.frames(1);
    // Griff am linken Ende greifen und ganz nach rechts ziehen: rechtes Ende = längste Zeile + Rand
    let grab = Pos2::new(bar.left() + 8.0, y);
    h.press(grab);
    h.frame(vec![Event::PointerMoved(grab + Vec2::new(12.0, 0.0))]);
    h.frame(vec![Event::PointerMoved(Pos2::new(bar.right() + 300.0, y))]);
    h.release(Pos2::new(bar.right() + 300.0, y));
    h.frames(1);
    assert!(near(h.view.scroll_x, max), "ganz rechts: {} statt {max}", h.view.scroll_x);
    // Zurück nach links ziehen: 0
    let thumb_right = bar.right() - 6.0;
    h.press(Pos2::new(thumb_right, y));
    h.frame(vec![Event::PointerMoved(Pos2::new(thumb_right - 12.0, y))]);
    h.frame(vec![Event::PointerMoved(Pos2::new(bar.left() - 300.0, y))]);
    h.release(Pos2::new(bar.left() - 300.0, y));
    h.frames(1);
    assert_eq!(h.view.scroll_x, 0.0, "ganz links");
    // Ein Klick auf die Leiste setzt keinen Cursor
    let head = h.view.sels.as_ref().unwrap().primary().head;
    h.click(Pos2::new(bar.center().x, y));
    assert_eq!(h.view.sels.as_ref().unwrap().primary().head, head, "Leiste bewegt den Cursor nicht");
}

#[test]
fn wheel_with_shift_and_over_the_bar_scroll_sideways_up_to_the_right_bound() {
    let mut h = H::new(&long_doc(500, 40));
    let max = h.max_for(500);
    let bar = h.bar().unwrap();
    // Rad über der Leiste rollt waagerecht
    h.wheel(bar.center(), Vec2::new(0.0, -120.0));
    assert!(h.view.scroll_x > 10.0, "Rad über der Leiste: {}", h.view.scroll_x);
    assert_eq!(h.view.top_line, 0, "nicht senkrecht");
    // Umschalt+Rad im Text, sehr weit: bleibt an der rechten Grenze
    h.mods = Modifiers::SHIFT;
    h.wheel(Pos2::new(300.0, 100.0), Vec2::new(0.0, -100_000.0));
    h.mods = Modifiers::NONE;
    h.frames(1);
    assert!(near(h.view.scroll_x, max), "rechte Grenze: {} statt {max}", h.view.scroll_x);
    // Waagerechtes Rollen (Touchpad) ebenso
    h.view.scroll_x = 0.0;
    h.wheel(Pos2::new(300.0, 100.0), Vec2::new(-100_000.0, 0.0));
    assert!(near(h.view.scroll_x, max), "Touchpad: {}", h.view.scroll_x);
    // Von außen gesetzt (z. B. alter Stand): begrenzt
    h.view.scroll_x = 1.0e7;
    h.frames(1);
    assert!(near(h.view.scroll_x, max));
}

#[test]
fn the_last_line_stays_above_the_bar_and_the_cursor_is_followed() {
    let mut text = String::new();
    for i in 0..199 {
        text.push_str(&format!("line {i}\n"));
    }
    text.push_str(&"y".repeat(600));
    let mut h = H::new(&text);
    let (cw, row_h) = h.metrics();
    let bar = h.bar().expect("Leiste");
    let text_top = h.rect.top() + 2.0;
    let text_left = bar.left() + 6.0;
    // Strg+Ende: Cursor am Ende der letzten (längsten) Zeile
    h.key(Key::End, Modifiers::CTRL);
    let last = h.doc().len_lines() - 1;
    let y = text_top - h.view.top_off + (last - h.view.top_line) as f32 * row_h;
    assert!(y + row_h <= bar.top() + 0.5, "letzte Zeile über der Leiste: {} > {}", y + row_h, bar.top());
    let x = text_left + 600.0 * cw - h.view.scroll_x;
    assert!(x > text_left && x < text_left + h.text_w(), "Cursor sichtbar: x {x}");
    assert!(h.view.scroll_x <= h.max_for(600) + 0.5, "innerhalb der Grenze");
    // Pos1: zurück an den Anfang
    h.key(Key::Home, Modifiers::NONE);
    h.key(Key::Home, Modifiers::NONE);
    assert_eq!(h.view.scroll_x, 0.0, "Zeilenanfang: ganz links");
    // Ende: wieder rechts, Tippen am Ende der längsten Zeile verlängert sie und der Cursor bleibt sichtbar
    h.key(Key::End, Modifiers::NONE);
    for _ in 0..30 {
        h.frame(vec![Event::Text("z".into())]);
    }
    h.frames(1);
    let x = text_left + 630.0 * cw - h.view.scroll_x;
    assert!(x > text_left && x < text_left + h.text_w(), "nach dem Tippen sichtbar: x {x}");
    assert!(near(h.max_for(630), hscroll::max_scroll(hscroll::content_width(630, cw), h.text_w())));
    // Viel auf einmal einfügen (ein Bild): der Cursor bleibt trotzdem sichtbar
    h.frame(vec![Event::Paste("p".repeat(200))]);
    let x = text_left + 830.0 * cw - h.view.scroll_x;
    assert!(x > text_left && x < text_left + h.text_w(), "nach dem Einfügen sichtbar: x {x}");
    // Cursor ganz unten in einer kurzen Datei: keine Zeile unter der Leiste
    let mut short = String::new();
    for _ in 0..((h.rect.height() / row_h) as usize - 1) {
        short.push_str("a\n");
    }
    short.push_str(&"b".repeat(400));
    let mut h = H::new(&short);
    h.key(Key::End, Modifiers::CTRL);
    let bar = h.bar().unwrap();
    let last = h.doc().len_lines() - 1;
    let y = text_top - h.view.top_off + (last - h.view.top_line) as f32 * row_h;
    assert!(y + row_h <= bar.top() + 0.5, "letzte Zeile frei: {} > {}", y + row_h, bar.top());
}

#[test]
fn widths_count_tabs_and_wide_characters_like_the_renderer() {
    let tab = 4;
    let lines = ["", "abc", "\tx", "a\tb\tc", "中文", "x中\ty", "😀a", "e\u{301}", "\u{1}\u{7f}z", "  \t  中", "ß\tä"];
    for l in lines {
        let doc = Document::from_text(l);
        let want = text::vcol(&doc, 0, doc.line_len(0), tab);
        assert_eq!(hscroll::line_cols(doc.line(0), tab), want, "{l:?}");
        assert_eq!(hscroll::longest_line_cols(doc.rope(), tab), want, "{l:?}");
        assert_eq!(want, et::visual_col(doc.rope(), 0, doc.line_len(0), tab));
    }
    let doc = Document::from_text(&lines.join("\n"));
    let want = (0..doc.len_lines()).map(|l| text::vcol(&doc, l, doc.line_len(l), tab)).max().unwrap();
    assert_eq!(hscroll::longest_line_cols(doc.rope(), tab), want);
    // Tabbreite zählt
    let doc = Document::from_text("\t\t\tx\nabcdefghij");
    assert_eq!(hscroll::longest_line_cols(doc.rope(), 4), 13);
    assert_eq!(hscroll::longest_line_cols(doc.rope(), 2), 10);
    assert_eq!(hscroll::longest_line_cols(doc.rope(), 8), 25);
    // Sehr lange Zeile: ein Zeichen = eine Spalte (wie gezeichnet)
    let doc = Document::from_text(&"中".repeat(LONG_LINE + 5));
    assert_eq!(hscroll::longest_line_cols(doc.rope(), tab), LONG_LINE + 5);
    // Breite Zeichen und Tabs im Textfeld: rechte Grenze danach
    let mut h = H::new(&format!("short\n{}\n", "\t中文".repeat(60)));
    let cols = et::visual_col(h.doc().rope(), 1, h.doc().line_len(1), h.doc().tab_width());
    assert_eq!(cols, 60 * (h.doc().tab_width() + 4));
    h.view.scroll_x = 1.0e7;
    h.frames(1);
    assert!(near(h.view.scroll_x, h.max_for(cols)), "Grenze mit Tabs und breiten Zeichen");
}

#[test]
fn large_documents_are_measured_once_per_revision_in_the_background() {
    // 100 000 Zeilen (über der Grenze für sofortiges Messen), die längste in der Mitte
    let text = long_doc(3000, 100_000);
    assert!(text.len() > hscroll::SYNC_BYTES);
    let start = Instant::now();
    let mut h = H::new(&text);
    assert!(start.elapsed() < Duration::from_secs(5), "erste Bilder warten nicht auf die Messung");
    h.settle();
    let max = h.max_for(3000);
    h.view.scroll_x = 1.0e7;
    h.frames(1);
    assert!(near(h.view.scroll_x, max), "Grenze nach der Messung: {} statt {max}", h.view.scroll_x);
    // Ohne Änderung wird nicht neu gemessen, egal wie viele Bilder
    let scans = h.view.hscroll.scans;
    let t = Instant::now();
    for _ in 0..30 {
        h.frame(vec![]);
    }
    assert_eq!(h.view.hscroll.scans, scans, "kein Durchlauf je Bild");
    assert!(t.elapsed() < Duration::from_secs(5), "Bilder bleiben schnell: {:?}", t.elapsed());
    assert!(!h.view.hscroll.measuring());
    // Änderung: ein neuer Durchlauf im Hintergrund, danach wieder Ruhe
    h.view.scroll_x = 0.0;
    let rev = h.doc().revision();
    h.frame(vec![Event::Text("q".into())]);
    assert!(h.doc().revision() > rev, "getippt");
    h.settle();
    assert!(h.view.hscroll.scans > scans, "neuer Stand wird gemessen");
    let scans = h.view.hscroll.scans;
    h.frames(10);
    assert_eq!(h.view.hscroll.scans, scans);
}

#[test]
fn a_single_huge_line_scrolls_to_its_end_without_slow_frames() {
    let n = 2_000_000;
    let mut h = H::new(&"a".repeat(n));
    h.settle();
    let t = Instant::now();
    h.key(Key::End, Modifiers::NONE);
    h.frames(10);
    assert!(t.elapsed() < Duration::from_secs(5), "schnell: {:?}", t.elapsed());
    let (cw, _) = h.metrics();
    let bar = h.bar().expect("Leiste");
    let text_left = bar.left() + 6.0;
    let x = text_left + n as f32 * cw - h.view.scroll_x;
    assert!(x > text_left && x < text_left + h.text_w(), "Cursor am Ende sichtbar: x {x}");
    assert!(h.view.scroll_x <= h.max_for(n) + 1.0);
}

// ---- im ganzen Editor: jede Teilung ----

fn editor_frame(ctx: &egui::Context, ed: &mut EditorUi, t: &mut f64) {
    *t += 0.05;
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))), time: Some(*t), ..Default::default() };
    let mut cmds = Vec::new();
    let th = crate::theme::all()[0].clone();
    let prefs = EditorPrefs::default();
    let _ = ctx.run(input, |ctx| view::draw(ctx, ed, &th, &prefs, crate::state::Lang::En, &mut cmds));
}

#[test]
fn every_split_gets_its_own_bar_and_the_find_bar_and_minimap_stay_intact() {
    let base = std::env::var("PANELDECK_TEST_ROOT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join(format!("pd-edui-hscroll-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("wide.txt");
    std::fs::write(&p, long_doc(900, 30)).unwrap();
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi { minimap: true, ..Default::default() };
    ed.set_root(&dir);
    ed.open_path(&p, false);
    for _ in 0..3 {
        editor_frame(&ctx, &mut ed, &mut t);
    }
    let g = ed.panes.focused;
    ed.panes.split(g, Dir::Horizontal).unwrap();
    ed.find.open(true);
    ed.find.query = "line".into();
    for _ in 0..3 {
        editor_frame(&ctx, &mut ed, &mut t);
    }
    let bars: Vec<(u64, Option<Rect>)> = ed.panes.groups.iter().map(|(id, grp)| (*id, grp.tabs[grp.active].view.hscroll.last_bar)).collect();
    assert_eq!(bars.len(), 2);
    for (id, bar) in &bars {
        let bar = bar.unwrap_or_else(|| panic!("Teilung {id} hat eine Leiste"));
        let gr = ed.group_rects[id];
        assert!(gr.contains_rect(bar.shrink(0.5)), "Leiste in ihrer Teilung: {bar:?} in {gr:?}");
        assert!(bar.right() <= gr.right() - 100.0 + 0.5, "Minimap und senkrechte Leiste bleiben frei: {bar:?} in {gr:?}");
    }
    let (a, b) = (bars[0].1.unwrap(), bars[1].1.unwrap());
    assert!(!a.intersects(b), "jede Teilung ihre eigene");
    // Umbruch an: alle weg
    ed.wrap = true;
    for _ in 0..2 {
        editor_frame(&ctx, &mut ed, &mut t);
    }
    assert!(ed.panes.groups.values().all(|grp| grp.tabs[grp.active].view.hscroll.last_bar.is_none()), "Umbruch: keine Leiste");
    let _ = std::fs::remove_dir_all(&dir);
}
