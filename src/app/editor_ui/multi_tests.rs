//! Tests der mehreren Cursor ohne Fenster (#394): Schalter (alles standardmäßig aus), Alt+Klick setzt bzw. nimmt Cursor,
//! mittlere Maustaste (Spaltenauswahl über Bildschirmzeilen, mit Strg hinzufügen, Klick), Menü *Auswahl*,
//! *Cursor/Auswahl zurücknehmen*, Statusleiste, *Kommentar umschalten*.

use super::view::{self, Cmd};
use super::*;
use crate::state::Lang;
use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect, Vec2};

fn test_dir(name: &str) -> PathBuf {
    let base = std::env::var("PANELDECK_TEST_ROOT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!("pd-edmulti-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const SIZE: Vec2 = Vec2::new(1200.0, 800.0);

fn frame(ctx: &egui::Context, ed: &mut EditorUi, events: Vec<Event>, t: &mut f64, prefs: &EditorPrefs) -> Vec<Cmd> {
    // Die Modifikatoren der Klicks gelten für das ganze Bild (egui liest sie aus `RawInput::modifiers`).
    let modifiers = events.iter().find_map(|e| if let Event::PointerButton { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
    *t += 0.05;
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), time: Some(*t), events, modifiers, focused: true, ..Default::default() };
    let mut cmds = Vec::new();
    let th = crate::theme::all()[0].clone();
    let _ = ctx.run(input, |ctx| view::draw(ctx, ed, &th, prefs, Lang::En, &mut cmds));
    cmds
}

fn button(p: Pos2, b: PointerButton, pressed: bool, m: Modifiers) -> Event {
    Event::PointerButton { pos: p, button: b, pressed, modifiers: m }
}

fn click(p: Pos2, b: PointerButton, m: Modifiers) -> Vec<Event> {
    vec![Event::PointerMoved(p), button(p, b, true, m), button(p, b, false, m)]
}

/// Bildschirmpunkt vor Zeichenspalte `col` der Zeile `line` (ohne Umbruch, nur ASCII, ab dem Anfang der Datei).
fn at(ctx: &egui::Context, ed: &EditorUi, prefs: &EditorPrefs, line: usize, col: usize) -> Pos2 {
    let g = ed.panes.focused;
    let rect = ed.group_rects[&g];
    let font = egui::FontId::monospace(prefs.font_size);
    let (row_h, cw) = ctx.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
    let gutter_w = (3.0 + 1.5) * cw + 8.0;
    let menu_h = if prefs.menu_bar { super::menu::MENU_H } else { 0.0 };
    let top = rect.top() + menu_h + view::BAR_H + 2.0;
    Pos2::new(rect.left() + gutter_w + 6.0 + col as f32 * cw + 1.0, top + row_h * (line as f32 + 0.5))
}

fn sels(ed: &EditorUi) -> Vec<std::ops::Range<usize>> {
    ed.focused_view_and_doc().and_then(|(v, _)| v.sels.as_ref()).map(|s| s.iter().map(|x| x.range()).collect()).unwrap_or_default()
}

fn text(ed: &EditorUi) -> String {
    ed.focused_view_and_doc().and_then(|(_, d)| d.text()).unwrap_or_default()
}

fn open(name: &str, body: &str) -> (PathBuf, EditorUi, egui::Context, f64) {
    let dir = test_dir(name);
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    let mut ed = EditorUi::default();
    ed.wrap = false;
    ed.open_path(&p, false);
    ed.want_focus = true;
    (dir, ed, egui::Context::default(), 0.0)
}

#[test]
fn alt_click_is_off_by_default_adds_and_removes_cursors_when_on() {
    let (dir, mut ed, ctx, mut t) = open("alt.txt", "alpha\nbeta\ngamma\n");
    let off = EditorPrefs::default();
    assert_eq!(off.multi, MultiPrefs::default(), "alles aus");
    frame(&ctx, &mut ed, vec![], &mut t, &off);
    frame(&ctx, &mut ed, vec![], &mut t, &off);
    let p = at(&ctx, &ed, &off, 1, 2);
    frame(&ctx, &mut ed, click(p, PointerButton::Primary, Modifiers::ALT), &mut t, &off);
    assert_eq!(sels(&ed).len(), 1, "Alt+Klick ohne Schalter: nur ein Cursor");
    let on = EditorPrefs { multi: MultiPrefs { alt_click: true, ..Default::default() }, ..Default::default() };
    let p = at(&ctx, &ed, &on, 0, 0);
    frame(&ctx, &mut ed, click(p, PointerButton::Primary, Modifiers::NONE), &mut t, &on);
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    let p = at(&ctx, &ed, &on, 1, 2);
    frame(&ctx, &mut ed, click(p, PointerButton::Primary, Modifiers::ALT), &mut t, &on);
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    let p = at(&ctx, &ed, &on, 2, 3);
    frame(&ctx, &mut ed, click(p, PointerButton::Primary, Modifiers::ALT), &mut t, &on);
    assert_eq!(sels(&ed), vec![0..0, 8..8, 14..14]);
    assert_eq!(ed.focused_view_and_doc().map(|(v, d)| v.position_text(d, Lang::En)).as_deref(), Some("3 cursors"));
    assert_eq!(ed.focused_view_and_doc().map(|(v, d)| v.position_text(d, Lang::De)).as_deref(), Some("3 Cursor"));
    // Tippen geht an alle
    frame(&ctx, &mut ed, vec![Event::Text("|".into())], &mut t, &on);
    assert_eq!(text(&ed), "|alpha\nbe|ta\ngam|ma\n");
    // Alt+Klick auf einen Cursor nimmt ihn weg
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    let p = at(&ctx, &ed, &on, 1, 3);
    frame(&ctx, &mut ed, click(p, PointerButton::Primary, Modifiers::ALT), &mut t, &on);
    assert_eq!(sels(&ed).len(), 2, "{:?}", sels(&ed));
    // *Cursor/Auswahl zurücknehmen*: der Cursor ist wieder da, der Text bleibt
    let g = ed.panes.focused;
    ed.menu_edit_op(&ctx, g, super::doc::Op::SoftUndo);
    assert_eq!(sels(&ed).len(), 3);
    assert_eq!(text(&ed), "|alpha\nbe|ta\ngam|ma\n");
    // Der letzte Cursor bleibt
    ed.menu_edit_op(&ctx, g, super::doc::Op::SingleCursor);
    let only = sels(&ed);
    assert_eq!(only.len(), 1);
    let (line, col) = ed.focused_view_and_doc().map(|(v, d)| v.cursor_linecol(d)).unwrap();
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    let p = at(&ctx, &ed, &on, line, col);
    frame(&ctx, &mut ed, click(p, PointerButton::Primary, Modifiers::ALT), &mut t, &on);
    assert_eq!(sels(&ed), only, "der letzte Cursor bleibt");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn middle_button_selects_columns_adds_with_ctrl_and_places_cursors() {
    let (dir, mut ed, ctx, mut t) = open("mid.txt", "abcdef\nab\n\nabcdef\n");
    let on = EditorPrefs { multi: MultiPrefs { middle: true, ..Default::default() }, ..Default::default() };
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    // Ziehen von Zeile 1, Spalte 1 bis Zeile 4, Spalte 4: je Zeile eine Auswahl; kurze und leere Zeilen: Cursor am Ende
    let (a, b) = (at(&ctx, &ed, &on, 0, 1), at(&ctx, &ed, &on, 3, 4));
    frame(&ctx, &mut ed, vec![Event::PointerMoved(a), button(a, PointerButton::Middle, true, Modifiers::NONE)], &mut t, &on);
    frame(&ctx, &mut ed, vec![Event::PointerMoved(Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0))], &mut t, &on);
    frame(&ctx, &mut ed, vec![Event::PointerMoved(b)], &mut t, &on);
    frame(&ctx, &mut ed, vec![button(b, PointerButton::Middle, false, Modifiers::NONE)], &mut t, &on);
    assert_eq!(sels(&ed), vec![1..4, 8..9, 10..10, 12..15]);
    frame(&ctx, &mut ed, vec![Event::Text("#".into())], &mut t, &on);
    assert_eq!(text(&ed), "a#ef\na#\n#\na#ef\n");
    // Klick ohne Ziehen: ein Cursor statt aller (das Loslassen zählt im nächsten Bild)
    let p = at(&ctx, &ed, &on, 0, 0);
    frame(&ctx, &mut ed, click(p, PointerButton::Middle, Modifiers::NONE), &mut t, &on);
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    assert_eq!(sels(&ed), vec![0..0]);
    // Strg+Klick: einer dazu
    let p = at(&ctx, &ed, &on, 1, 1);
    frame(&ctx, &mut ed, click(p, PointerButton::Middle, Modifiers::CTRL), &mut t, &on);
    frame(&ctx, &mut ed, vec![], &mut t, &on);
    assert_eq!(sels(&ed), vec![0..0, 6..6]);
    // Strg+Ziehen: die Spalte kommt dazu
    let (a, b) = (at(&ctx, &ed, &on, 2, 0), at(&ctx, &ed, &on, 3, 0));
    frame(&ctx, &mut ed, vec![Event::PointerMoved(a), button(a, PointerButton::Middle, true, Modifiers::CTRL)], &mut t, &on);
    frame(&ctx, &mut ed, vec![Event::PointerMoved(b)], &mut t, &on);
    frame(&ctx, &mut ed, vec![button(b, PointerButton::Middle, false, Modifiers::CTRL)], &mut t, &on);
    assert_eq!(sels(&ed).len(), 4, "{:?}", sels(&ed));
    // Ohne Schalter tut die mittlere Taste nichts
    let off = EditorPrefs::default();
    let p = at(&ctx, &ed, &off, 0, 2);
    frame(&ctx, &mut ed, click(p, PointerButton::Middle, Modifiers::NONE), &mut t, &off);
    frame(&ctx, &mut ed, vec![], &mut t, &off);
    assert_eq!(sels(&ed).len(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn column_selection_counts_screen_rows_with_word_wrap() {
    let d = crate::editor::Document::from_text(&format!("{}\nshort\n", "word ".repeat(12)));
    let g = super::text::test_geo(Some(20), 4);
    let mut s = d.new_selections(0);
    // Zeile 1 hat bei 20 Spalten drei Bildschirmzeilen; von ihrer ersten bis zur Zeile „short“: vier Auswahlen.
    let rows = super::text::rows_of(&d, 0, Some(20), 4);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(!super::text::column_rows(&d, &mut s, (0, 0, 2), (1, 0, 4), &g, None));
    assert_eq!(s.len(), 4);
    // In jeder Bildschirmzeile ab ihrem Anfang gezählt
    let starts: Vec<usize> = s.iter().map(|x| x.start()).collect();
    assert_eq!(starts[1], rows[1] + 2);
    assert_eq!(starts[3], d.line_to_char(1) + 2);
}

#[test]
fn selection_menu_is_greyed_out_until_switched_on_and_toggle_comment_uses_the_language() {
    let (dir, mut ed, ctx, mut t) = open("lib.rs", "fn a() {}\nfn b() {}\n");
    let off = EditorPrefs::default();
    frame(&ctx, &mut ed, vec![], &mut t, &off);
    frame(&ctx, &mut ed, vec![], &mut t, &off);
    let g = ed.panes.focused;
    let menus = super::menu::menus(&mut ed, g, &off, Lang::En);
    let sel = menus.iter().find(|m| m.id == "selection").expect("Menü Auswahl");
    assert_eq!(menus.iter().map(|m| m.title).collect::<Vec<_>>(), vec!["File", "Edit", "Selection", "View", "Format"]);
    for id in ["next_occurrence", "all_occurrences", "soft_undo"] {
        let i = sel.item(id).unwrap();
        assert!(!i.enabled && i.tip.as_deref().is_some_and(|t| t.contains("Multiple cursors are off")), "{id}");
    }
    assert!(sel.item("single_cursor").unwrap().enabled);
    assert_eq!(sel.item("next_occurrence").unwrap().shortcut, "Ctrl+D");
    let on = EditorPrefs { multi: MultiPrefs { on: true, ..Default::default() }, ..Default::default() };
    let menus = super::menu::menus(&mut ed, g, &on, Lang::En);
    let sel = menus.iter().find(|m| m.id == "selection").unwrap();
    assert!(sel.item("all_occurrences").unwrap().enabled);
    assert_eq!(sel.item("all_occurrences").unwrap().shortcut, "Ctrl+Shift+D");
    // Alle Vorkommen von „fn“ (Cursor am Anfang)
    ed.menu_edit_op(&ctx, g, super::doc::Op::SelectAllOccurrences);
    assert_eq!(sels(&ed), vec![0..2, 10..12]);
    // Kommentar umschalten mit „//“ (Rust) an jedem Cursor, ein Rückgängig-Schritt
    let edit = menus.iter().find(|m| m.id == "edit").unwrap();
    let tc = edit.item("toggle_comment").unwrap();
    assert!(tc.enabled && tc.cmds.contains(&Cmd::EditOp(g, super::doc::Op::ToggleComment("//"))));
    ed.menu_edit_op(&ctx, g, super::doc::Op::ToggleComment("//"));
    assert_eq!(text(&ed), "// fn a() {}\n// fn b() {}\n");
    // Palette: die Befehle der Auswahl und der Kommentar
    let pal = super::menu::palette_entries(&ed, &on, Lang::En);
    assert!(pal.iter().any(|(l, _, _)| l == "Selection: select all occurrences"));
    assert!(super::menu::palette_entries(&ed, &off, Lang::En).iter().all(|(l, _, _)| !l.contains("occurrence")));
    // Ohne Zeilenkommentar (Markdown): ausgegraut
    let (dir2, mut ed2, ctx2, mut t2) = open("notes.md", "x\n");
    frame(&ctx2, &mut ed2, vec![], &mut t2, &off);
    let g2 = ed2.panes.focused;
    let m2 = super::menu::menus(&mut ed2, g2, &off, Lang::En);
    assert!(!m2.iter().find(|m| m.id == "edit").unwrap().item("toggle_comment").unwrap().enabled);
    let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&dir2));
}
