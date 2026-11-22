//! Tests des Editor-Bereichs ohne Fenster (#54): Zeichnen ohne Panik (leer, mit Datei, Teilungen, Suche, Umbruch,
//! große Datei), Tastatur und Maus im Textfeld, Ablageziele gezogener Reiter, gemerkter Zustand.

use super::view::{self, Cmd};
use super::*;
use crate::layout::Dir;
use eframe::egui::{self, Event, Id, Key, Modifiers, Pos2, Rect, Vec2};

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

/// Wie [`frame`], aber mit Fenstergröße und Fokus des Fensters (minimiert meldet Windows 0×0 ohne Fokus).
fn frame_at(ctx: &egui::Context, ed: &mut EditorUi, events: Vec<Event>, t: &mut f64, size: Vec2, active: bool) -> Vec<Cmd> {
    *t += 0.05;
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(*t), events, focused: active, ..Default::default() };
    let mut cmds = Vec::new();
    let th = theme();
    let prefs = EditorPrefs::default();
    let _ = ctx.run(input, |ctx| view::draw(ctx, ed, &th, &prefs, crate::state::Lang::En, &mut cmds));
    cmds
}

const BIG: Vec2 = Vec2::new(1500.0, 960.0);

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
    // andere Größe als "one\ntwo\n": auf schnellen Rechnern kann die Änderungszeit gleich bleiben
    std::fs::write(&p, "changed on disk\n").unwrap();
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

/// Klicktest 3.0.0: Nach Minimieren und Wiederherstellen bzw. einem Wechsel zu einem anderen Programm tippt der Nutzer
/// weiter in das Textfeld, das vorher die Tastatur hatte – aber nur dann, nie nach einem Reiterwechsel durch einen Agenten
/// und nicht, wenn inzwischen ein anderes Feld die Tastatur hat.
#[test]
fn keyboard_comes_back_after_minimize_and_window_switch() {
    let dir = test_dir("refocus");
    let a = write(&dir, "a.txt", "text\n");
    let b = write(&dir, "b.txt", "agent\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    let ida = ed.open_path(&a, false);
    let type_ = |ctx: &egui::Context, ed: &mut EditorUi, t: &mut f64, s: &str| {
        frame_at(ctx, ed, vec![Event::Text(s.into())], t, BIG, true);
    };
    for _ in 0..2 {
        frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    }
    type_(&ctx, &mut ed, &mut t, "1");
    assert_eq!(ed.docs[&ida].text().unwrap(), "1text\n");
    // Minimiert: 0×0 und ohne Fokus, mehrere Bilder lang (egui nahm dem nicht gezeichneten Feld die Tastatur).
    for _ in 0..3 {
        frame_at(&ctx, &mut ed, vec![], &mut t, Vec2::ZERO, false);
    }
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    type_(&ctx, &mut ed, &mut t, "2");
    assert_eq!(ed.docs[&ida].text().unwrap(), "12text\n", "nach dem Wiederherstellen");
    // Ein anderes Programm im Vordergrund, das Fenster bleibt sichtbar; die Tastatur geht dabei verloren.
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, false);
    let g = ed.panes.focused;
    ctx.memory_mut(|m| m.surrender_focus(view::text_id(g)));
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, false);
    assert_eq!(ctx.memory(|m| m.focused()), None);
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    type_(&ctx, &mut ed, &mut t, "3");
    assert_eq!(ed.docs[&ida].text().unwrap(), "123text\n", "nach dem Fensterwechsel");
    // Ein Agent zeigt in der Zeit eine andere Datei: die Tastatur kommt nicht zurück.
    frame_at(&ctx, &mut ed, vec![], &mut t, Vec2::ZERO, false);
    let m = ed.begin_agent();
    let idb = ed.open_path(&b, false);
    ed.end_agent(m);
    frame_at(&ctx, &mut ed, vec![], &mut t, Vec2::ZERO, false);
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    type_(&ctx, &mut ed, &mut t, "X");
    assert_eq!(ed.docs[&idb].text().unwrap(), "agent\n", "keine Taste in der Datei des Agenten");
    assert_eq!(ed.docs[&ida].text().unwrap(), "123text\n");
    assert_eq!(ctx.memory(|m| m.focused()), None);
    // Ohne Tastatur vor dem Wechsel auch danach keine.
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, false);
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    type_(&ctx, &mut ed, &mut t, "Y");
    assert_eq!(ed.docs[&idb].text().unwrap(), "agent\n");
    // Der Nutzer fokussiert wieder; während das Fenster weg ist, nimmt ein anderes Feld die Tastatur: sie bleibt dort.
    ed.want_focus = true;
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    type_(&ctx, &mut ed, &mut t, "4");
    assert_eq!(ed.docs[&idb].text().unwrap(), "4agent\n");
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, false);
    ctx.memory_mut(|m| m.request_focus(Id::new("some_dialog_field")));
    frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    assert_ne!(ctx.memory(|m| m.focused()), Some(view::text_id(ed.panes.focused)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Klicktest 3.0.0: Der Ordnerbaum behält seine Breite über Minimieren (0×0) und ein sehr schmales Fenster hinweg; die
/// an den fehlenden Platz angepasste Breite wird weder übernommen noch gemerkt.
#[test]
fn tree_width_survives_minimize_and_a_tiny_window() {
    use egui::containers::panel::PanelState;
    assert_eq!(view::tree_width_seen(240.0, 0.0, 0.0), 240.0, "minimiert");
    assert_eq!(view::tree_width_seen(240.0, 200.0, 200.0), 240.0, "an zu wenig Platz angepasst");
    assert_eq!(view::tree_width_seen(240.0, 300.0, 1200.0), 300.0, "breiter gezogen");
    assert_eq!(view::tree_width_seen(240.0, 160.0, 1200.0), 160.0, "schmaler gezogen");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi { tree_open: true, tree_width: 240.0, ..Default::default() };
    let panel = |ctx: &egui::Context| PanelState::load(ctx, Id::new("ed_tree")).map(|s| s.rect.width());
    for _ in 0..3 {
        frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    }
    assert!((ed.tree_width - 240.0).abs() < 0.5 && panel(&ctx).is_some_and(|w| (w - 240.0).abs() < 0.5), "{} {:?}", ed.tree_width, panel(&ctx));
    for (size, active) in [(Vec2::ZERO, false), (Vec2::new(200.0, 400.0), true)] {
        for _ in 0..3 {
            frame_at(&ctx, &mut ed, vec![], &mut t, size, active);
        }
        assert_eq!(ed.tree_width, 240.0, "{size:?}: gemerkte Breite");
        for _ in 0..2 {
            frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
        }
        let w = panel(&ctx).unwrap();
        assert!((w - 240.0).abs() < 0.5, "{size:?}: danach wieder 240 px breit, nicht {w}");
        assert_eq!(ed.tree_width, 240.0);
    }
}

/// Klicktest 3.0.0 (Blocker): Das Fenster „Unterschied anzeigen“ bleibt mit einem langen Unterschied im Hauptfenster –
/// Titelleiste (Schließen) und Knöpfe erreichbar, auch wenn das Hauptfenster kleiner wird – und wächst nicht.
#[test]
fn diff_window_stays_inside_the_main_window() {
    let lines: Vec<(char, String)> = (0..3000).map(|i| (if i % 2 == 0 { '+' } else { '-' }, format!("line {i} {}", "x".repeat(300)))).collect();
    let mono = egui::FontId::monospace(13.0);
    let ctx = egui::Context::default();
    let mut n = 0;
    let mut run = |w: f32, h: f32| {
        let mut seen = None;
        let mut rects = Vec::new();
        for _ in 0..8 {
            n += 1;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))), time: Some(n as f64 * 0.05), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                let mut open = true;
                let mut cmds = Vec::new();
                view::diff_window(ctx, "Difference – a.txt".into()).open(&mut open).show(ctx, |ui| {
                    view::diff_body(ui, 1, &lines, &mono, crate::state::Lang::En, &mut cmds);
                    seen = Some((ui.min_rect(), ui.max_rect()));
                });
            });
            rects.push(ctx.memory(|m| m.area_rect(Id::new("ed_diff_win"))).expect("Fenster"));
        }
        let win = *rects.last().unwrap();
        assert!(win.min.x >= -0.5 && win.min.y >= -0.5 && win.max.x <= w + 0.5 && win.max.y <= h + 0.5, "{w}×{h}: Fenster {win:?} ragt hinaus");
        assert!((rects[4].height() - win.height()).abs() < 0.5, "{w}×{h}: Fenster wächst: {rects:?}");
        let (used, avail) = seen.expect("gezeichnet");
        assert!(used.max.y <= avail.max.y + 0.5, "{w}×{h}: Knöpfe {used:?} unter dem Fensterrand {avail:?}");
    };
    run(1500.0, 960.0);
    run(1100.0, 700.0);
    run(1500.0, 960.0);
}

/// Schmale Teilung: Die Knöpfe der Hinweisleiste bekommen Kurzformen, statt abgeschnitten zu werden.
#[test]
fn info_bar_buttons_shorten_in_narrow_splits() {
    assert!(!view::info_narrow(900.0, 380.0));
    assert!(view::info_narrow(500.0, 380.0));
}

// ------------------------------------------------------------------------------------------- Syntax-Modus (#312)

/// Wie [`frame`], mit eigenen Einstellungen.
fn frame_prefs(ctx: &egui::Context, ed: &mut EditorUi, events: Vec<Event>, t: &mut f64, prefs: &EditorPrefs) -> Vec<Cmd> {
    *t += 0.05;
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), time: Some(*t), events, ..Default::default() };
    let mut cmds = Vec::new();
    let th = theme();
    let _ = ctx.run(input, |ctx| view::draw(ctx, ed, &th, prefs, crate::state::Lang::En, &mut cmds));
    cmds
}

fn click_at(p: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(p),
        Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]
}

#[test]
fn status_bar_picks_the_language_and_the_mode_is_text_by_default() {
    use crate::editor::syntax::Mode;
    let dir = test_dir("syntax");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let p = write(&dir, "main.rs", "fn main() {\n    let s = \"hi\"; // x\n}\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let prefs = EditorPrefs::default();
    let mut ed = EditorUi::restore(&data);
    ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    let id = ed.active_doc().unwrap();
    let st = ed.docs.get_mut(&id).unwrap().syntax_status(&prefs, crate::state::Lang::En);
    assert_eq!(st.lang.as_deref(), Some("rust"), "aus der Endung erkannt");
    assert_eq!((st.mode, st.mode_chosen, st.active), (Mode::Text, false, false), "neue Dateien: Text");
    assert!(ed.docs[&id].syntax.hl.is_none(), "im Text-Modus keine Syntaxfarben");

    // Sprachknopf der Statusleiste anklicken, suchen, Enter.
    let r = ed.status_rects.0.expect("Sprachknopf gezeichnet");
    frame(&ctx, &mut ed, click_at(r.center()), &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    assert!(ctx.memory(|m| m.focused()) == Some(Id::new("ed_lang_search")), "Suchfeld hat die Tastatur");
    frame(&ctx, &mut ed, vec![Event::Text("pytho".into())], &mut t);
    assert_eq!(ed.lang_search, "pytho");
    let key = Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
    let cmds = frame(&ctx, &mut ed, vec![key], &mut t);
    assert!(cmds.contains(&Cmd::SetLanguage(id, Some("python".into()))), "{cmds:?}");
    ed.set_doc_language(id, Some("python".into()));
    let st = ed.docs.get_mut(&id).unwrap().syntax_status(&prefs, crate::state::Lang::En);
    assert_eq!((st.lang.as_deref(), st.lang_chosen, st.lang_name.as_str()), (Some("python"), true, "Python"));
    assert!(view::filter_languages(".au3").iter().any(|l| l.id == "autoit"), "Suche nach Endung");

    // Moduswahl: Syntax für diese Datei → Farben, auch in einer zweiten Teilung (ein Zustand je Dokument).
    let m = ed.status_rects.1.expect("Modusknopf gezeichnet");
    frame(&ctx, &mut ed, click_at(m.center()), &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    ed.set_doc_language(id, None);
    ed.set_doc_mode(id, Some(Mode::Syntax));
    let g = ed.panes.focused;
    ed.panes.split(g, Dir::Horizontal).unwrap();
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    let d = &ed.docs[&id];
    let hl = d.syntax.hl.as_ref().expect("Syntaxfarben an");
    assert_eq!(hl.lang, "rust");
    assert!(hl.spans(0).is_some_and(|s| !s.is_empty()), "sichtbare Zeilen eingefärbt");
    // Einstellung je Sprache: Syntax für Rust, ohne eigene Wahl im Dokument.
    ed.set_doc_mode(id, None);
    let mut p2 = EditorPrefs::default();
    p2.syntax.language_modes.insert("rust".into(), Mode::Syntax);
    frame_prefs(&ctx, &mut ed, vec![], &mut t, &p2);
    assert!(ed.docs[&id].syntax.hl.is_some(), "Einstellung der Sprache gilt");
    frame(&ctx, &mut ed, vec![], &mut t);
    assert!(ed.docs[&id].syntax.hl.is_none(), "Standard Text");
    // Große Datei: auch im Syntax-Modus keine Farben.
    let mut p3 = p2.clone();
    p3.large_file_bytes = 10;
    frame_prefs(&ctx, &mut ed, vec![], &mut t, &p3);
    assert!(ed.docs[&id].syntax.hl.is_none(), "große Datei: Syntaxfarben aus");

    // Gewählte Sprache und Modus werden mit dem Dokument gemerkt.
    ed.set_doc_language(id, Some("source.haskell".into()));
    ed.set_doc_mode(id, Some(Mode::Syntax));
    ed.save(&data);
    let mut back = EditorUi::restore(&data);
    let bid = *back.docs.keys().next().unwrap();
    assert_eq!(back.docs[&bid].syntax.lang.as_deref(), Some("source.haskell"));
    assert_eq!(back.docs[&bid].syntax.mode, Some(Mode::Syntax));
    frame(&ctx, &mut back, vec![], &mut t);
    let _ = std::fs::remove_dir_all(&dir);
}
