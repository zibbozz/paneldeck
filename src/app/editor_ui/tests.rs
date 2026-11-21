//! Tests des Editor-Bereichs ohne Fenster (#54): Zeichnen ohne Panik (leer, mit Datei, Teilungen, Suche, Umbruch,
//! große Datei), Tastatur und Maus im Textfeld, Ablageziele gezogener Reiter, gemerkter Zustand.

use super::view::{self, Cmd};
use super::*;
use crate::layout::Dir;
use eframe::egui::{self, Event, Key, Modifiers, Pos2, Rect, Vec2};

fn test_dir(name: &str) -> PathBuf {
    let base = std::env::var("PANELDECK_TEST_ROOT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!("pd-edui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn theme() -> crate::theme::Theme {
    crate::theme::all()[0].clone()
}

/// Ein Bild zeichnen; `events` gehen an dieses Bild. Liefert die gesammelten Befehle.
fn frame(ctx: &egui::Context, ed: &mut EditorUi, events: Vec<Event>, t: &mut f64) -> Vec<Cmd> {
    *t += 0.05;
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), time: Some(*t), events, ..Default::default() };
    let mut cmds = Vec::new();
    let th = theme();
    let prefs = EditorPrefs::default();
    let _ = ctx.run(input, |ctx| view::draw(ctx, ed, &th, &prefs, crate::state::Lang::En, &mut cmds));
    cmds
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn active_text(ed: &EditorUi) -> String {
    ed.docs[&ed.active_doc().unwrap()].text().unwrap()
}

#[test]
fn draws_empty_file_splits_find_and_wrap_without_panic() {
    let dir = test_dir("draw");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi { tree_open: true, minimap: true, ..Default::default() };
    frame(&ctx, &mut ed, vec![], &mut t);
    let long = "word ".repeat(400);
    let p = write(&dir, "a.txt", &format!("hello\n\tworld ä 中文\n{long}\nend"));
    ed.set_root(&dir);
    ed.open_path(&p, false);
    for _ in 0..3 {
        frame(&ctx, &mut ed, vec![], &mut t);
    }
    assert!(ed.docs.values().all(|d| d.is_loaded()), "sichtbare Datei wird gelesen");
    let g = ed.panes.focused;
    ed.panes.split(g, Dir::Horizontal).unwrap();
    let g2 = ed.panes.focused;
    ed.panes.split(g2, Dir::Vertical).unwrap();
    ed.find.open(true);
    ed.find.query = "o".into();
    ed.wrap = true;
    for _ in 0..3 {
        frame(&ctx, &mut ed, vec![], &mut t);
    }
    assert_eq!(ed.group_rects.len(), 3, "drei Teilungen gezeichnet");
    assert_eq!(ed.docs.len(), 1, "dieselbe Datei in allen Teilungen");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn typing_in_one_split_shows_in_the_other_with_own_cursor() {
    let dir = test_dir("sync");
    let p = write(&dir, "b.txt", "abc\ndef\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let a = ed.panes.focused;
    let b = ed.panes.split(a, Dir::Horizontal).unwrap();
    ed.want_focus = true;
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![Event::Text("X".into())], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    assert_eq!(active_text(&ed), "Xabc\ndef\n");
    // Die andere Ansicht hat ihren eigenen Cursor, der dem eingefügten Text folgt.
    let other = &ed.panes.groups[&a].tabs[0].view;
    let mine = &ed.panes.groups[&b].tabs[0].view;
    assert_eq!(mine.sels.as_ref().unwrap().primary().head, 1);
    assert!(other.sels.is_some());
    assert!(ed.docs.values().next().unwrap().dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn keyboard_moves_selects_and_undoes() {
    let dir = test_dir("keys");
    let p = write(&dir, "c.txt", "one two\nthree\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    let key = |k: Key, m: Modifiers| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
    // Ende der Zeile, Wort links markieren, ersetzen
    frame(&ctx, &mut ed, vec![key(Key::End, Modifiers::NONE), key(Key::ArrowLeft, Modifiers::CTRL | Modifiers::SHIFT)], &mut t);
    assert_eq!(ed.focused_view_and_doc().and_then(|(v, d)| v.selected_text(d)).as_deref(), Some("two"));
    frame(&ctx, &mut ed, vec![Event::Text("2".into())], &mut t);
    assert_eq!(active_text(&ed), "one 2\nthree\n");
    // Weitere Zeile darunter, Umschalt+Alt+Pfeil: zweiter Cursor
    frame(&ctx, &mut ed, vec![key(Key::Home, Modifiers::NONE), key(Key::ArrowDown, Modifiers::ALT | Modifiers::SHIFT)], &mut t);
    assert_eq!(ed.focused_view_and_doc().unwrap().0.selection_count(), 2);
    frame(&ctx, &mut ed, vec![Event::Text(">".into())], &mut t);
    assert_eq!(active_text(&ed), ">one 2\n>three\n");
    frame(&ctx, &mut ed, vec![key(Key::Z, Modifiers::CTRL)], &mut t);
    assert_eq!(active_text(&ed), "one 2\nthree\n");
    frame(&ctx, &mut ed, vec![key(Key::Z, Modifiers::CTRL)], &mut t);
    assert_eq!(active_text(&ed), "one two\nthree\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_large_generated_file_opens_and_draws_without_heavy_features() {
    let dir = test_dir("large");
    let p = dir.join("big.log");
    {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&p).unwrap());
        let line = "2026-11-21 12:00:00 INFO some log line with a little text in it 0123456789\n";
        let n = (21 * 1024 * 1024) / line.len() + 1;
        for _ in 0..n {
            f.write_all(line.as_bytes()).unwrap();
        }
    }
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi { minimap: true, wrap: true, ..Default::default() };
    ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let id = ed.active_doc().unwrap();
    let prefs = EditorPrefs::default();
    assert!(ed.docs[&id].is_loading(), "große Datei wird im Hintergrund gelesen");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while ed.docs[&id].is_loading() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(&ctx, &mut ed, vec![], &mut t);
    }
    assert!(ed.docs[&id].is_large(&prefs));
    // Ans Ende springen und zeichnen: muss schnell gehen (nur sichtbare Zeilen).
    let lines = ed.docs[&id].line_count();
    ed.goto_line(lines - 1, 0);
    let start = std::time::Instant::now();
    for _ in 0..5 {
        frame(&ctx, &mut ed, vec![], &mut t);
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "5 Bilder dauerten {:?}", start.elapsed());
    let v = &ed.panes.groups[&ed.panes.focused].tabs[0].view;
    assert!(v.top_line + 100 > lines, "unten angekommen: {} von {lines}", v.top_line);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_a_tab_targets_bars_and_split_edges() {
    let dir = test_dir("drop");
    let p1 = write(&dir, "1.txt", "1");
    let p2 = write(&dir, "2.txt", "2");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.open_path(&p1, false);
    ed.open_path(&p2, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let g = ed.panes.focused;
    let r = ed.group_rects[&g];
    let body_mid = Pos2::new(r.center().x, r.top() + view::BAR_H + (r.height() - view::BAR_H) / 2.0);
    match view::drop_target(&ed, Pos2::new(r.right() - 10.0, body_mid.y)) {
        Some((panes::Drop::Split { group, dir: Dir::Horizontal, before: false }, _)) => assert_eq!(group, g),
        other => panic!("rechter Rand: {other:?}"),
    }
    match view::drop_target(&ed, Pos2::new(body_mid.x, r.bottom() - 5.0)) {
        Some((panes::Drop::Split { dir: Dir::Vertical, before: false, .. }, _)) => {}
        other => panic!("unterer Rand: {other:?}"),
    }
    assert!(matches!(view::drop_target(&ed, body_mid), Some((panes::Drop::Bar { index: 2, .. }, _))));
    assert!(matches!(view::drop_target(&ed, Pos2::new(r.left() + 5.0, r.top() + 10.0)), Some((panes::Drop::Bar { index: 0, .. }, _))));
    // Ablegen rechts: neue Teilung mit dem Reiter
    ed.panes.move_tab(g, 1, panes::Drop::Split { group: g, dir: Dir::Horizontal, before: false });
    frame(&ctx, &mut ed, vec![], &mut t);
    assert_eq!(ed.group_rects.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn state_survives_save_and_restore() {
    let dir = test_dir("state");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let p = write(&dir, "s.txt", "l1\nl2\nl3\n");
    let mut ed = EditorUi::restore(&data);
    ed.set_root(&dir);
    ed.open_path(&p, false);
    let g = ed.panes.focused;
    ed.panes.split(g, Dir::Vertical).unwrap();
    ed.goto_line(2, 1);
    ed.wrap = true;
    let ctx = egui::Context::default();
    let mut t = 0.0;
    frame(&ctx, &mut ed, vec![], &mut t);
    ed.save(&data);
    let back = EditorUi::restore(&data);
    assert_eq!(back.root.as_deref(), Some(dir.as_path()));
    assert_eq!(back.panes.group_ids().len(), 2, "Teilung wiederhergestellt");
    assert_eq!(back.docs.len(), 1, "ein Dokument für beide Reiter");
    assert!(back.wrap);
    assert_eq!(back.recent_files.first().map(PathBuf::from).as_deref(), Some(p.as_path()));
    let tab = &back.panes.groups[&back.panes.focused].tabs[0];
    assert_eq!(tab.view.restore_pos(), (2, 1), "Cursor gemerkt");
    // Datei weg (ohne Ungespeichertes): Reiter fällt weg
    std::fs::remove_file(&p).unwrap();
    let gone = EditorUi::restore(&data);
    assert!(gone.docs.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rows_wrap_at_spaces_and_fall_back_to_hard_breaks() {
    let d = crate::editor::Document::from_text("aaaa bbbb cccc dddd\nxxxxxxxxxxxxxxxxxxxxxxxx\nshort");
    assert_eq!(super::text::rows_of(&d, 0, Some(10), 4), vec![0, 10], "nach dem Leerzeichen");
    assert_eq!(super::text::rows_of(&d, 1, Some(10), 4), vec![0, 10, 20], "hart umbrechen");
    assert_eq!(super::text::rows_of(&d, 2, Some(10), 4), vec![0]);
    assert_eq!(super::text::rows_of(&d, 0, None, 4), vec![0], "ohne Umbruch");
}

#[test]
fn key_mapping_leaves_ctrl_alt_arrows_to_the_area() {
    use super::text::{key_op, KeyAct, Move};
    assert_eq!(key_op(Key::ArrowLeft, Modifiers::CTRL | Modifiers::ALT, 10), None);
    assert_eq!(key_op(Key::ArrowLeft, Modifiers::SHIFT, 10), Some(KeyAct::Move(Move::Left, true)));
    assert_eq!(key_op(Key::PageDown, Modifiers::NONE, 30), Some(KeyAct::Move(Move::Down(30), false)));
    assert_eq!(key_op(Key::ArrowDown, Modifiers::ALT | Modifiers::SHIFT, 10), Some(KeyAct::AddCursor(true)));
    assert!(matches!(key_op(Key::Z, Modifiers::CTRL, 10), Some(KeyAct::Op(super::doc::Op::Undo))));
}

#[test]
fn find_and_replace_through_the_core() {
    use super::find::Query;
    let mut d = crate::editor::Document::from_text("Foo foo food\nfoo");
    let mut s = d.new_selections(0);
    let q = Query { text: "foo".into(), case: false, word: true, regex: false };
    assert_eq!(doc::count_matches(&d, &q, 100), 3, "ganzes Wort, ohne Groß/Klein");
    assert_eq!(doc::find_next(&d, &q, 1, true), Some(4..7));
    assert_eq!(doc::find_next(&d, &q, 4, false), Some(0..3));
    assert!(doc::check_query(&Query { text: "(".into(), regex: true, ..Default::default() }).is_some(), "ungültiger Ausdruck");
    let q2 = Query { text: r"f(o+)".into(), case: true, word: false, regex: true };
    assert_eq!(doc::replace_all(&mut d, &mut s, &q2, "F$1"), 3);
    assert_eq!(d.text(), "Foo Foo Food\nFoo");
    assert!(d.undo(&mut s));
    assert_eq!(d.text(), "Foo foo food\nfoo", "Alle ersetzen ist ein Schritt");
}

#[test]
fn deleted_and_changed_files_show_info_bars() {
    let dir = test_dir("disk");
    let p = write(&dir, "e.txt", "one\n");
    let mut ed = EditorUi::default();
    let id = ed.open_path(&p, false);
    let prefs = EditorPrefs::default();
    let lang = crate::state::Lang::En;
    ed.docs.get_mut(&id).unwrap().ensure_loaded(&prefs);
    // Ohne eigene Änderungen: leise neu laden
    std::fs::write(&p, "one\ntwo\n").unwrap();
    assert!(matches!(ed.docs.get_mut(&id).unwrap().disk_changed(&prefs, lang), doc::DiskReaction::Reloaded(_)));
    assert_eq!(ed.docs[&id].text().as_deref(), Some("one\ntwo\n"));
    // Mit eigenen Änderungen: Hinweisleiste
    ed.docs.get_mut(&id).unwrap().apply_agent_edit(vec![(0..0, "x".into())], lang).unwrap();
    std::fs::write(&p, "changed\n").unwrap();
    assert!(matches!(ed.docs.get_mut(&id).unwrap().disk_changed(&prefs, lang), doc::DiskReaction::Asked));
    assert!(ed.docs[&id].disk_conflict);
    assert!(!ed.docs[&id].disk_diff(lang).is_empty());
    ed.docs.get_mut(&id).unwrap().keep_mine();
    assert!(!ed.docs[&id].disk_conflict && ed.docs[&id].dirty());
    // Gelöscht
    std::fs::remove_file(&p).unwrap();
    ed.docs.get_mut(&id).unwrap().disk_changed(&prefs, lang);
    assert!(ed.docs[&id].disk_deleted);
    let info = ed.docs[&id].info(id, true, &prefs);
    assert!(info.disk_deleted && info.dirty && info.agent_edited);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hot_exit_restores_unsaved_buffers() {
    let dir = test_dir("hot");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let p = write(&dir, "h.txt", "disk\n");
    let mut ed = EditorUi::restore(&data);
    let id = ed.open_path(&p, false);
    let un = ed.new_untitled(Some("scratch text"));
    let prefs = EditorPrefs::default();
    for d in ed.docs.values_mut() {
        d.ensure_loaded(&prefs);
    }
    ed.docs.get_mut(&id).unwrap().apply_agent_edit(vec![(0..0, "edited ".into())], crate::state::Lang::En).unwrap();
    ed.save(&data);
    assert!(data.join(doc::HOT_DIR).is_dir());
    let mut back = EditorUi::restore(&data);
    assert_eq!(back.docs.len(), 2);
    for d in back.docs.values_mut() {
        d.ensure_loaded(&prefs);
    }
    let texts: Vec<String> = back.docs.values().map(|d| d.text().unwrap()).collect();
    assert!(texts.contains(&"edited disk\n".to_string()), "{texts:?}");
    assert!(texts.contains(&"scratch text".to_string()), "{texts:?}");
    assert!(back.docs.values().all(|d| d.dirty()));
    // Nach dem Speichern ist der Hot-Exit-Eintrag weg
    let fid = back.doc_for_path(&p).unwrap();
    back.docs.get_mut(&fid).unwrap().save(None, crate::state::Lang::En).unwrap();
    back.docs.get_mut(&fid).unwrap().drop_hot(&data);
    back.save(&data);
    let (_, entries) = doc::hot_entries(&data);
    assert_eq!(entries.len(), 1, "nur noch die unbenannte Datei");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "edited disk\n");
    let _ = un;
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_a_dirty_tab_asks_and_a_clean_one_closes() {
    let dir = test_dir("close");
    let p = write(&dir, "d.txt", "x");
    let mut ed = EditorUi::default();
    let id = ed.open_path(&p, false);
    let prefs = EditorPrefs::default();
    ed.docs.get_mut(&id).unwrap().ensure_loaded(&prefs);
    assert!(!ed.docs[&id].dirty());
    ed.docs.get_mut(&id).unwrap().apply_agent_edit(vec![(0..0, "y".into())], crate::state::Lang::En).unwrap();
    assert!(ed.docs[&id].dirty() && ed.docs[&id].agent_edited);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Aufrufe von Agenten (öffnen, neuer Reiter, zeigen) nehmen nie die Tastatur: Tippt der Nutzer gerade im Editor, landen
/// seine Tasten danach weder in der Datei des Agenten noch sonst in einem Dokument. Aktionen des Nutzers fokussieren
/// weiter.
#[test]
fn agent_calls_never_take_the_keyboard_focus() {
    let dir = test_dir("agentfocus");
    let a = write(&dir, "a.txt", "mine\n");
    let b = write(&dir, "b.txt", "agent\n");
    let c = write(&dir, "c.txt", "user\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    let ida = ed.open_path(&a, false);
    for _ in 0..2 {
        frame(&ctx, &mut ed, vec![], &mut t);
    }
    frame(&ctx, &mut ed, vec![Event::Text("1".into())], &mut t);
    assert_eq!(ed.docs[&ida].text().unwrap(), "1mine\n", "der Nutzer tippt in a.txt");
    // Agent öffnet b.txt: Reiter wird aktiv, die Tastatur bekommt er nicht.
    let m = ed.begin_agent();
    let idb = ed.open_path(&b, false);
    ed.end_agent(m);
    assert!(!ed.want_focus && ed.blur);
    for _ in 0..2 {
        frame(&ctx, &mut ed, vec![], &mut t);
    }
    frame(&ctx, &mut ed, vec![Event::Text("X".into())], &mut t);
    assert_eq!(ed.active_doc(), Some(idb));
    assert_eq!(ed.docs[&idb].text().unwrap(), "agent\n", "keine Taste in der Datei des Agenten");
    assert_eq!(ed.docs[&ida].text().unwrap(), "1mine\n");
    assert_eq!(ctx.memory(|m| m.focused()), None);
    // Neuer unbenannter Reiter und Reiter zeigen: ebenso ohne Tastatur.
    let m = ed.begin_agent();
    let idn = ed.new_untitled(Some("note"));
    ed.end_agent(m);
    frame(&ctx, &mut ed, vec![Event::Text("Y".into())], &mut t);
    assert_eq!(ed.docs[&idn].text().unwrap(), "note");
    // Eine Aktion des Nutzers (Datei öffnen) fokussiert wieder.
    let idc = ed.open_path(&c, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![Event::Text("Z".into())], &mut t);
    assert_eq!(ed.docs[&idc].text().unwrap(), "Zuser\n");
    // Ein Agent, der nichts am gezeigten Reiter ändert, lässt die Tastatur dem Nutzer.
    let m = ed.begin_agent();
    ed.open_path(&c, false);
    ed.end_agent(m);
    assert!(!ed.blur);
    frame(&ctx, &mut ed, vec![Event::Text("W".into())], &mut t);
    assert_eq!(ed.docs[&idc].text().unwrap(), "ZWuser\n");
    let _ = std::fs::remove_dir_all(&dir);
}
