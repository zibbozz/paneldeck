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
    // Die Farben entstehen im Hilfsthread: zeichnen, bis sie da sind.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        frame(&ctx, &mut ed, vec![], &mut t);
        if ed.docs[&id].syntax.hl.as_ref().is_some_and(|h| h.spans(0).is_some()) || std::time::Instant::now() > until {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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
    // Ein Modus einer neueren Version bleibt beim Speichern erhalten.
    let mut s = persist::load(&data);
    s.docs[0].mode = Some("smart".into());
    persist::save(&data, &s);
    let again = EditorUi::restore(&data);
    assert_eq!(again.docs.values().next().unwrap().syntax.mode, None);
    again.save(&data);
    assert_eq!(persist::load(&data).docs[0].mode.as_deref(), Some("smart"));

    // Unbenannte Datei: ein später getippter Shebang führt zur Sprache.
    let mut ed2 = EditorUi::default();
    let u = ed2.new_untitled(Some("print(1)\n"));
    frame(&ctx, &mut ed2, vec![], &mut t);
    assert_eq!(ed2.docs.get_mut(&u).unwrap().language(&prefs), None);
    let d = ed2.docs.get_mut(&u).unwrap();
    let mut sels = d.doc.as_ref().unwrap().new_selections(0);
    d.doc.as_mut().unwrap().apply_edits(&mut sels, vec![crate::editor::TextEdit::insert(0, "#!/usr/bin/env python3\n")], crate::editor::EditOrigin::User).unwrap();
    assert_eq!(d.language(&prefs).as_deref(), Some("python"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------- Menüleiste und Formatieren (#381)

use crate::editor::format::{FormatError, Kind, Op as FmtOp, Options as FmtOptions, Outcome};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Ersatz-Formatierer: `Format`/`SortKeys` setzen nach jedem `,` einen Zeilenumbruch mit Einrückung, `Minify` entfernt
/// Leerraum, ein `!` ist ungültig (an seiner Stelle), `~` wartet, bis abgebrochen wird (Fortschritt 0,5).
fn fake_fmt() -> Arc<crate::editor_svc::format_job::Runner> {
    Arc::new(|_k: Kind, op: FmtOp, t: &str, o: FmtOptions, _f: bool, c: &AtomicBool, p: &dyn Fn(f32)| {
        if t.contains('~') {
            p(0.5);
            while !c.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            return Outcome::Cancelled;
        }
        if let Some(i) = t.find('!') {
            let before = &t[..i];
            return Outcome::Invalid(FormatError { line: before.matches('\n').count() + 1, column: before.rsplit('\n').next().unwrap_or("").chars().count() + 1, message: "unexpected '!'".into() });
        }
        let mut s = match op {
            FmtOp::Validate => return Outcome::Text(t.to_string()),
            FmtOp::Minify => t.split_whitespace().collect::<String>(),
            _ => t.trim().replace(",", ",\n  "),
        };
        if o.final_newline && !s.ends_with('\n') {
            s.push('\n');
        }
        Outcome::Text(s)
    })
}

/// Bilder zeichnen, bis kein Formatier-Lauf mehr läuft (höchstens 5 s).
fn settle(ctx: &egui::Context, ed: &mut EditorUi, t: &mut f64) {
    let t0 = std::time::Instant::now();
    while !ed.fmt.jobs.is_empty() && t0.elapsed().as_secs() < 5 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        frame(ctx, ed, vec![], t);
    }
    assert!(ed.fmt.jobs.is_empty(), "Lauf endet");
}

fn seen() -> Vec<(&'static str, Rect, bool)> {
    super::menu::SEEN.with(|s| s.borrow().clone())
}

fn clear_seen() {
    super::menu::SEEN.with(|s| s.borrow_mut().clear());
}

#[test]
fn menu_bar_opens_menus_greys_entries_and_folds_in_narrow_splits() {
    use super::menu::{menus, Entry};
    let dir = test_dir("menubar");
    let txt = write(&dir, "notes.txt", "hello\n");
    let json = write(&dir, "a.json", "[1,2]\n");
    let xml = write(&dir, "b.xml", "<a><b/></a>\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    let prefs = EditorPrefs::default();
    let en = crate::state::Lang::En;
    ed.open_path(&txt, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let g = ed.panes.focused;
    let (mr, narrow) = ed.menu_rects[&g];
    assert!(!narrow && (mr.height() - super::menu::MENU_H).abs() < 0.1, "Menüleiste über der Teilung");
    assert!(ed.bar_rects[&g].0.top() >= mr.bottom() - 0.5, "Reiterleiste darunter");
    // Modell: vier Menüs, Kürzel rechts, Ausgegrautes mit Grund
    let m = menus(&mut ed, g, &prefs, en);
    assert_eq!(m.iter().map(|m| m.title).collect::<Vec<_>>(), ["File", "Edit", "View", "Format"]);
    let fmt = &m[3];
    let f = fmt.item("format").unwrap();
    assert!(!f.enabled && f.tip.as_deref().is_some_and(|t| t.starts_with("Not JSON or XML")), "{f:?}");
    assert_eq!(f.shortcut, "Alt+Shift+F");
    assert!(!fmt.item("format_on_save").unwrap().enabled);
    let undo = m[1].item("undo").unwrap();
    assert!(!undo.enabled && undo.tip.as_deref() == Some("Nothing to undo"));
    assert!(m[0].item("save").unwrap().enabled && m[0].item("save").unwrap().shortcut == "Ctrl+S");
    assert!(!m[0].item("save_all").unwrap().enabled, "nichts Ungespeichertes");
    assert_eq!(m[2].item("split_right").unwrap().shortcut, "Ctrl+Shift+D");
    assert_eq!(m[2].item("wrap").unwrap().checked, Some(false));
    assert!(m[2].entries.iter().any(|e| matches!(e, Entry::Languages { .. })));
    let de = menus(&mut ed, g, &prefs, crate::state::Lang::De);
    assert_eq!(de.iter().map(|m| m.title).collect::<Vec<_>>(), ["Datei", "Bearbeiten", "Ansicht", "Format"]);
    assert_eq!(de[3].item("format").unwrap().shortcut, "Alt+Umschalt+F");
    // JSON: alles aktiv, Kürzel aus den Einstellungen; XML: Schlüssel sortieren ausgegraut
    ed.open_path(&json, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let m = menus(&mut ed, g, &prefs, en);
    for id in ["format", "minify", "validate", "sort_keys", "format_on_save"] {
        assert!(m[3].item(id).unwrap().enabled, "{id}");
    }
    assert_eq!(m[3].item("format").unwrap().label, "Format JSON");
    assert_eq!(m[3].item("format_on_save").unwrap().checked, Some(false));
    let mut p2 = prefs.clone();
    p2.keys.insert("ed_format".into(), "Ctrl+Shift+J".into());
    p2.format_on_save_json = true;
    let m2 = menus(&mut ed, g, &p2, en);
    assert_eq!(m2[3].item("format").unwrap().shortcut, "Ctrl+Shift+J");
    assert_eq!(m2[3].item("format_on_save").unwrap().cmds, vec![Cmd::SetFormatOnSave(Kind::Json, false)]);
    ed.open_path(&xml, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let m = menus(&mut ed, g, &prefs, en);
    let s = m[3].item("sort_keys").unwrap();
    assert!(!s.enabled && s.tip.as_deref() == Some("Only for JSON"));
    assert!(m[3].item("format").unwrap().enabled && m[3].item("format").unwrap().label == "Format XML");

    // Menü „Format“ per Klick öffnen und „Validate XML“ anklicken
    clear_seen();
    frame(&ctx, &mut ed, vec![], &mut t);
    let title = seen().into_iter().find(|(id, ..)| *id == "format").expect("Titel gezeichnet").1;
    frame(&ctx, &mut ed, click_at(title.center()), &mut t);
    clear_seen();
    frame(&ctx, &mut ed, vec![], &mut t);
    assert!(egui::Popup::is_any_open(&ctx), "Menü offen");
    let items = seen();
    let sort = items.iter().find(|(id, ..)| *id == "sort_keys").expect("Einträge gezeichnet");
    assert!(!sort.2, "ausgegraut");
    let val = items.iter().find(|(id, ..)| *id == "validate").unwrap().1;
    let cmds = frame(&ctx, &mut ed, click_at(val.center()), &mut t);
    assert!(cmds.contains(&Cmd::FocusGroup(g)) && cmds.contains(&Cmd::Format(g, FmtOp::Validate)), "{cmds:?}");
    // Ein ausgegrauter Eintrag tut nichts
    let title = seen().into_iter().find(|(id, ..)| *id == "format").map(|x| x.1).unwrap_or(title);
    frame(&ctx, &mut ed, click_at(title.center()), &mut t);
    clear_seen();
    frame(&ctx, &mut ed, vec![], &mut t);
    if let Some(sort) = seen().into_iter().find(|(id, ..)| *id == "sort_keys") {
        let cmds = frame(&ctx, &mut ed, click_at(sort.1.center()), &mut t);
        assert!(!cmds.iter().any(|c| matches!(c, Cmd::Format(..))), "{cmds:?}");
    }

    // Schmale Teilungen: ein einziges „Menu ▾“
    let g2 = ed.panes.split(g, Dir::Horizontal).unwrap();
    for _ in 0..2 {
        frame_at(&ctx, &mut ed, vec![], &mut t, Vec2::new(300.0, 600.0), true);
    }
    assert!(ed.menu_rects[&g].1 && ed.menu_rects[&g2].1, "zusammengeklappt");
    for _ in 0..2 {
        frame_at(&ctx, &mut ed, vec![], &mut t, BIG, true);
    }
    assert!(!ed.menu_rects[&g].1, "breit genug: vier Menüs");
    // Abschaltbar
    let mut off = prefs.clone();
    off.menu_bar = false;
    frame_prefs(&ctx, &mut ed, vec![], &mut t, &off);
    assert!(ed.menu_rects.is_empty(), "keine Menüleiste");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn formatting_runs_in_the_background_cancels_and_applies_one_undo_step() {
    let dir = test_dir("fmtjob");
    let p = write(&dir, "a.json", "[1,2,3]\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.fmt.runner = Some(fake_fmt());
    let id = ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    let g = ed.panes.focused;
    let en = crate::state::Lang::En;
    // Format: ein Rückgängig-Schritt, Cursor bleibt auf seiner Zeile
    ed.start_format(g, FmtOp::Format, false, || {}, en);
    assert!(ed.fmt.job_for(id).is_some());
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(active_text(&ed), "[1,\n  2,\n  3]\n");
    assert!(ed.hint.as_ref().is_some_and(|(h, _)| h.contains("JSON formatted")), "{:?}", ed.hint);
    ed.want_focus = true;
    frame(&ctx, &mut ed, vec![], &mut t);
    let undo = Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL };
    frame(&ctx, &mut ed, vec![undo.clone()], &mut t);
    assert_eq!(active_text(&ed), "[1,2,3]\n", "ein einziger Rückgängig-Schritt");
    // Läuft lange: Fortschrittszeile mit Abbrechen, nichts ändert sich
    {
        let d = ed.docs.get_mut(&id).unwrap();
        d.apply_agent_edit(vec![(0..0, "~".into())], en).unwrap();
    }
    ed.start_format(g, FmtOp::Format, false, || {}, en);
    let t0 = std::time::Instant::now();
    while ed.fmt.job_for(id).is_some_and(|j| j.job.progress() < 0.5) && t0.elapsed().as_secs() < 5 {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    frame(&ctx, &mut ed, vec![], &mut t);
    let body_top = ed.bar_rects[&g].0.bottom();
    // Hinweisleiste „vom Agenten geändert“ steht zuerst, dann die Fortschrittszeile.
    let line_top = body_top + 30.0;
    let gr = ed.group_rects[&g];
    let cancel = Pos2::new(gr.right() - 40.0, line_top + 15.0);
    let cmds = frame(&ctx, &mut ed, click_at(cancel), &mut t);
    assert!(cmds.contains(&Cmd::FormatCancel(id)), "Abbrechen in der Fortschrittszeile: {cmds:?}");
    ed.cancel_format(id);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(active_text(&ed), "~[1,2,3]\n", "abgebrochen: nichts geändert");
    assert!(ed.hint.as_ref().is_some_and(|(h, _)| h.contains("cancelled")));
    // Geändert während des Laufs: das Ergebnis wird nicht übernommen
    {
        let d = ed.docs.get_mut(&id).unwrap();
        d.apply_agent_edit(vec![(0..1, String::new())], en).unwrap();
    }
    ed.fmt.runner = Some(Arc::new(|_k: Kind, _o: FmtOp, t: &str, _p: FmtOptions, _f: bool, _c: &AtomicBool, _pr: &dyn Fn(f32)| {
        std::thread::sleep(std::time::Duration::from_millis(150));
        Outcome::Text(format!("{t}x"))
    }));
    ed.start_format(g, FmtOp::Format, false, || {}, en);
    {
        let d = ed.docs.get_mut(&id).unwrap();
        d.apply_agent_edit(vec![(0..0, " ".into())], en).unwrap();
    }
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(active_text(&ed), " [1,2,3]\n");
    assert!(matches!(ed.fmt.notes.get(&id), Some((super::format::Note::Info(s), _)) if s.contains("changed while formatting")), "{:?}", ed.fmt.notes);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_content_shows_line_and_column_with_go_to_and_selections_format_alone() {
    let dir = test_dir("fmterr");
    let p = write(&dir, "a.json", "{\n  \"a\": !\n}\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.fmt.runner = Some(fake_fmt());
    let en = crate::state::Lang::En;
    let id = ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    let g = ed.panes.focused;
    ed.start_format(g, FmtOp::Format, false, || {}, en);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(active_text(&ed), "{\n  \"a\": !\n}\n", "ungültig: nichts geändert");
    let Some((super::format::Note::Invalid(Kind::Json, e), _)) = ed.fmt.notes.get(&id).cloned() else { panic!("{:?}", ed.fmt.notes) };
    assert_eq!((e.line, e.column), (2, 8));
    assert!(super::format::invalid_text(Kind::Json, &e, en).starts_with("Invalid JSON at line 2, column 8"));
    // Die Leiste zeigt „Gehe zu“ links neben „Ausblenden“
    frame(&ctx, &mut ed, vec![], &mut t);
    let gr = ed.group_rects[&g];
    let top = ed.bar_rects[&g].0.bottom();
    let mut found = None;
    for dx in (40..200).step_by(8) {
        let cmds = frame(&ctx, &mut ed, click_at(Pos2::new(gr.right() - dx as f32, top + 15.0)), &mut t);
        if let Some(c) = cmds.iter().find(|c| matches!(c, Cmd::FormatGoto(..))) {
            found = Some(c.clone());
            break;
        }
    }
    assert_eq!(found, Some(Cmd::FormatGoto(id, 2, 8)));
    ed.format_goto(id, 2, 8);
    frame(&ctx, &mut ed, vec![], &mut t);
    let (v, d) = ed.focused_view_and_doc().unwrap();
    assert_eq!(v.cursor_linecol(d), (1, 7), "Cursor an der Fehlerstelle");
    // Prüfen: gültig
    let p2 = write(&dir, "b.json", "{\"k\": [1,2]}\n");
    let id2 = ed.open_path(&p2, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    ed.start_format(g, FmtOp::Validate, false, || {}, en);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(ed.fmt.notes.get(&id2).map(|n| n.0.clone()), Some(super::format::Note::Valid(Kind::Json)));
    // Eine Meldung veraltet mit der nächsten Änderung
    frame(&ctx, &mut ed, vec![Event::Text("x".into())], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    assert!(!ed.fmt.notes.contains_key(&id2));
    frame(&ctx, &mut ed, vec![undo_key()], &mut t);
    // Auswahl: nur sie wird formatiert
    {
        let text = active_text(&ed);
        let s = text.find('[').unwrap();
        let e = text.find(']').unwrap() + 1;
        let tab = ed.panes.groups.get_mut(&g).unwrap().active_tab_mut().unwrap();
        let doc = ed.docs[&id2].doc.as_ref().unwrap();
        tab.view.select_range(doc, s..e);
    }
    ed.start_format(g, FmtOp::Format, false, || {}, en);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(active_text(&ed), "{\"k\": [1,\n  2]}\n", "nur die Auswahl");
    let (v, d) = ed.focused_view_and_doc().unwrap();
    assert_eq!(v.selected_text(d).as_deref(), Some("[1,\n  2]"), "Auswahl umfasst den neuen Text");
    let _ = std::fs::remove_dir_all(&dir);
}

fn undo_key() -> Event {
    Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL }
}

#[test]
fn jsonc_minify_asks_first_and_format_on_save_never_loses_the_save() {
    let dir = test_dir("fmtsave");
    let p = write(&dir, "tsconfig.json", "{ \"a\": 1, \"b\": 2 }\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.fmt.runner = Some(fake_fmt());
    let en = crate::state::Lang::En;
    let id = ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    let g = ed.panes.focused;
    ed.start_format(g, FmtOp::Minify, false, || {}, en);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(ed.fmt.ask, vec![super::format::MinifyAsk { doc: id, group: g }], "JSONC: erst fragen");
    // Eine zweite Anfrage für dasselbe Dokument überschreibt nichts und kommt nicht doppelt.
    ed.start_format(g, FmtOp::Minify, false, || {}, en);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(ed.fmt.ask.len(), 1);
    assert_eq!(active_text(&ed), "{ \"a\": 1, \"b\": 2 }\n");
    frame(&ctx, &mut ed, vec![], &mut t);
    let esc = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
    let cmds = frame(&ctx, &mut ed, vec![esc], &mut t);
    assert!(cmds.contains(&Cmd::MinifyConfirm(false)), "Esc = Abbrechen: {cmds:?}");
    ed.fmt.ask.clear();
    ed.start_format(g, FmtOp::Minify, true, || {}, en);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(active_text(&ed), "{\"a\":1,\"b\":2}\n", "bestätigt");

    // Formatieren beim Speichern: aus → nichts; an → formatiert; ungültig → unverändert mit Hinweis; zu groß → Hinweis
    let mut prefs = EditorPrefs::default();
    assert_eq!(ed.format_before_save(id, None, &prefs, en), (None, false));
    assert_eq!(active_text(&ed), "{\"a\":1,\"b\":2}\n");
    prefs.format_on_save_json = true;
    assert_eq!(ed.format_before_save(id, None, &prefs, en), (None, true));
    assert_eq!(active_text(&ed), "{\"a\":1,\n  \"b\":2}\n");
    prefs.format_on_save_json = false;
    prefs.format_on_save_xml = true;
    let x = write(&dir, "c.xml", "<a>!</a>\n");
    let idx = ed.open_path(&x, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let h = ed.format_before_save(idx, None, &prefs, en).0.expect("Hinweis");
    assert!(h.starts_with("Saved unchanged: invalid XML at line 1, column 4"), "{h}");
    assert_eq!(active_text(&ed), "<a>!</a>\n");
    let big = write(&dir, "big.xml", &format!("<a>{}</a>\n", "x".repeat(crate::editor_svc::format_job::SAVE_MAX_BYTES + 10)));
    let idb = ed.open_path(&big, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let _ = ed.docs.get_mut(&idb).map(|d| d.ensure_loaded_now(&prefs));
    assert!(ed.format_before_save(idb, None, &prefs, en).0.is_some_and(|h| h.contains("over 1 MB")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review #381: Speichern unter richtet sich nach dem neuen Namen; scheitert das Speichern, wird das Formatieren
/// zurückgenommen; schreibgeschützte Dateien werden nicht formatiert; *Prüfen* prüft mit Auswahl die ganze Datei.
#[test]
fn format_on_save_follows_save_as_and_is_undone_when_the_save_fails() {
    let dir = test_dir("fmtsave2");
    let p = write(&dir, "draft.txt", "[1,2]\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.fmt.runner = Some(fake_fmt());
    let en = crate::state::Lang::En;
    let prefs = EditorPrefs { format_on_save_json: true, ..EditorPrefs::default() };
    let id = ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    // .txt: kein JSON (kein Raten am Inhalt) → unverändert gespeichert
    assert_eq!(ed.save_with_format(id, None, &prefs, en), Ok(None));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "[1,2]\n");
    // Speichern unter … .json: formatiert
    let j = dir.join("data.json");
    assert_eq!(ed.save_with_format(id, Some(j.clone()), &prefs, en), Ok(None));
    assert_eq!(std::fs::read_to_string(&j).unwrap(), "[1,\n  2]\n");
    // Speichern scheitert (Zeichen passt nicht in Windows-1252): das Formatieren wird zurückgenommen
    let k = write(&dir, "k.json", "[1,\"中\"]\n");
    let idk = ed.open_path(&k, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let _ = ed.docs.get_mut(&idk).unwrap().set_encoding("windows-1252", en);
    assert!(ed.save_with_format(idk, None, &prefs, en).is_err());
    assert_eq!(ed.docs[&idk].text().unwrap(), "[1,\"中\"]\n", "Dokument wie vorher");
    // Schreibgeschützt: nicht formatieren
    let r = write(&dir, "ro.json", "[3,4]\n");
    let orig = std::fs::metadata(&r).unwrap().permissions();
    let mut perm = orig.clone();
    perm.set_readonly(true);
    std::fs::set_permissions(&r, perm).unwrap();
    let idr = ed.open_path(&r, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    assert_eq!(ed.format_before_save(idr, None, &prefs, en), (None, false));
    assert_eq!(ed.docs[&idr].text().unwrap(), "[3,4]\n");
    let _ = std::fs::set_permissions(&r, orig);
    // Prüfen mit Auswahl: die ganze Datei (gültig), nicht nur die Auswahl
    let v = write(&dir, "v.json", "{\"a\": [1,2]}\n");
    let idv = ed.open_path(&v, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let g = ed.panes.focused;
    {
        let tab = ed.panes.groups.get_mut(&g).unwrap().active_tab_mut().unwrap();
        let doc = ed.docs[&idv].doc.as_ref().unwrap();
        tab.view.select_range(doc, 1..4);
    }
    ed.start_format(g, FmtOp::Validate, false, || {}, en);
    settle(&ctx, &mut ed, &mut t);
    assert_eq!(ed.fmt.notes.get(&idv).map(|n| n.0.clone()), Some(super::format::Note::Valid(Kind::Json)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ein Speicherweg (#381 + #383, `EditorUi::save_doc`): Formatieren beim Speichern und die Text-Aktionen der Regeln
/// sind **ein** Rückgängig-Schritt; *Formatieren* einer Regel läuft nach dem Formatieren beim Speichern nicht noch einmal;
/// ohne `format` (automatisch, Agent) kein Formatieren; scheitert das Schreiben, ist alles zurückgenommen (auch
/// Zeilenenden).
#[test]
fn save_doc_formats_and_runs_builtins_as_one_undo_step() {
    use crate::editor::LineEnding;
    use crate::save_actions::Builtin;
    let dir = test_dir("savedoc");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.fmt.runner = Some(fake_fmt());
    let en = crate::state::Lang::En;
    let prefs = EditorPrefs { format_on_save_json: true, ..EditorPrefs::default() };
    let p = write(&dir, "a.json", "[1,2]  ");
    let id = ed.open_path(&p, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    frame(&ctx, &mut ed, vec![], &mut t);
    let list = [Builtin::Format, Builtin::TrimWhitespace, Builtin::FinalNewline];
    let s = ed.save_doc(id, None, true, &list, &prefs, en, &[]).unwrap();
    // Der Ersatz-Formatierer schreibt „[1,\n  2]“; der echte (die Aktion *Formatieren*) hätte „[\n  1,\n  2\n]“ daraus gemacht.
    // Ohne Zeilenende in der Datei gilt das Standard-Zeilenende des Systems.
    assert_eq!(std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n"), "[1,\n  2]\n");
    assert_eq!(s.hint, None);
    assert_eq!(s.builtins, vec![(Builtin::Format, Ok(None)), (Builtin::TrimWhitespace, Ok(None)), (Builtin::FinalNewline, Ok(None))]);
    {
        let d = ed.docs.get_mut(&id).unwrap();
        let doc = d.doc.as_mut().unwrap();
        let mut sels = doc.new_selections(0);
        assert!(doc.undo(&mut sels), "ein Schritt");
        assert_eq!(doc.text(), "[1,2]  ", "Formatieren und Aktionen zusammen zurück");
        assert!(!doc.can_undo());
    }
    // Ohne Formatieren (automatisches Speichern, Agent): nur die Aktionen.
    let b = write(&dir, "b.json", "[5,6]  \n");
    let idb = ed.open_path(&b, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let s = ed.save_doc(idb, None, false, &[Builtin::TrimWhitespace], &prefs, en, &[]).unwrap();
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "[5,6]\n");
    assert_eq!(s.builtins, vec![(Builtin::TrimWhitespace, Ok(None))]);
    // Ungültig: Hinweis des Formatierens, die Aktion *Formatieren* der Regel bekommt den Fehler (ohne zweiten Lauf).
    let v = write(&dir, "v.json", "[1!]\n");
    let idv = ed.open_path(&v, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let s = ed.save_doc(idv, None, true, &[Builtin::Format], &prefs, en, &[]).unwrap();
    assert!(s.hint.as_deref().is_some_and(|h| h.starts_with("Saved unchanged: invalid JSON at line 1, column 3")), "{s:?}");
    assert!(matches!(&s.builtins[..], [(Builtin::Format, Err(e))] if e.starts_with("Invalid content, not formatted: 1:3")), "{s:?}");
    // Schreiben scheitert (Zeichen passt nicht in Windows-1252): Text und Zeilenenden wie vorher.
    let k = write(&dir, "k.json", "[1,\"中\"]  \n");
    let idk = ed.open_path(&k, false);
    frame(&ctx, &mut ed, vec![], &mut t);
    let _ = ed.docs.get_mut(&idk).unwrap().set_encoding("windows-1252", en);
    let before = ed.docs[&idk].doc.as_ref().unwrap().format_state();
    assert!(ed.save_doc(idk, None, true, &[Builtin::TrimWhitespace, Builtin::LineEndings(LineEnding::CrLf)], &prefs, en, &[]).is_err());
    let doc = ed.docs[&idk].doc.as_ref().unwrap();
    assert_eq!(doc.text(), "[1,\"中\"]  \n", "Dokument wie vorher");
    assert!(doc.format_state() == before, "Zeilenenden/Kodierung wie vorher");
    assert!(!doc.can_undo(), "kein übrig gebliebener Schritt");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agent_formatting_is_marked_undoable_and_never_takes_the_keyboard() {
    let dir = test_dir("fmtagent");
    let p = write(&dir, "a.json", "[1,2]\n");
    let q = write(&dir, "b.txt", "mine\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    ed.fmt.runner = Some(fake_fmt());
    let prefs = EditorPrefs::default();
    let ida = ed.open_path(&p, false);
    let idb = ed.open_path(&q, false);
    for _ in 0..2 {
        frame(&ctx, &mut ed, vec![], &mut t);
    }
    let focused_before = ctx.memory(|m| m.focused());
    let job = ed.start_agent_format(ida, FmtOp::Format, false, &prefs, || {}).unwrap();
    let done = job.wait();
    let crate::editor_svc::format_job::Done::Changed { edits, .. } = done else { panic!("{done:?}") };
    let m = ed.begin_agent();
    ed.apply_agent_format(ida, job.revision, edits.clone()).unwrap();
    ed.end_agent(m);
    assert_eq!(ed.docs[&ida].text().unwrap(), "[1,\n  2]\n");
    assert!(ed.docs[&ida].agent_edited && ed.docs[&ida].dirty());
    assert_eq!(ed.active_doc(), Some(idb), "der Reiter des Nutzers bleibt");
    frame(&ctx, &mut ed, vec![], &mut t);
    assert_eq!(ctx.memory(|m| m.focused()), focused_before);
    // Ein zweites Anwenden mit altem Stand: Konflikt, nichts geändert
    assert_eq!(ed.apply_agent_format(ida, job.revision, edits), Err(("conflict", "changed")));
    assert_eq!(ed.docs[&ida].text().unwrap(), "[1,\n  2]\n");
    let _ = std::fs::remove_dir_all(&dir);
}
