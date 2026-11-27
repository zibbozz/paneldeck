//! Tests des Vergleichs-Reiters und der Änderungsmarken ohne Fenster (#385): Vergleich gegen die Platte und zwischen
//! zwei Dateien, Blockpfeile in beide Richtungen als ein Rückgängig-Schritt, Füllzeilen und automatisch untereinander in
//! schmalen Teilungen, Wiederherstellen nach dem Neustart, Marken am Rand mit Klick-Fenster und *Zurücksetzen*,
//! F7, Binär-/große Dateien, git (Index und HEAD) in einem Test-Repository.

use super::diff_tab::{Side, Target};
use super::view::{self, Cmd};
use super::*;
use crate::editor::compare::RowKind;
use crate::state::Lang;
use eframe::egui::{self, Event, Modifiers, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};

fn test_dir(name: &str) -> PathBuf {
    let base = std::env::var("PANELDECK_TEST_ROOT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!("pd-eddiff-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(dir: &Path, name: &str, text: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

const WIDE: Vec2 = Vec2::new(1500.0, 900.0);

fn frame(ctx: &egui::Context, ed: &mut EditorUi, events: Vec<Event>, t: &mut f64, size: Vec2, prefs: &EditorPrefs) -> Vec<Cmd> {
    *t += 0.05;
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(*t), events, focused: true, ..Default::default() };
    let mut cmds = Vec::new();
    let th = crate::theme::all()[0].clone();
    let _ = ctx.run(input, |ctx| view::draw(ctx, ed, &th, prefs, Lang::En, &mut cmds));
    cmds
}

/// Bilder zeichnen, bis `done` gilt (Hilfsthreads: höchstens 10 s).
fn settle(ctx: &egui::Context, ed: &mut EditorUi, t: &mut f64, size: Vec2, prefs: &EditorPrefs, done: impl Fn(&EditorUi) -> bool) {
    let t0 = Instant::now();
    loop {
        frame(ctx, ed, vec![], t, size, prefs);
        if done(ed) {
            // noch ein Bild, damit die Flächen (Pfeile, Falten) zum Ergebnis passen
            frame(ctx, ed, vec![], t, size, prefs);
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "Zeitüberschreitung");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn diff_settled(ed: &EditorUi) -> bool {
    !ed.diffs.is_empty() && ed.diffs.values().all(|d| d.settled(&ed.docs))
}

fn click_at(p: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(p),
        Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]
}

fn text_of(ed: &EditorUi, id: u64) -> String {
    ed.docs[&id].text().unwrap()
}

#[test]
fn saved_file_against_buffer_and_arrows_are_one_undo_step() {
    let dir = test_dir("disk");
    let p = write(&dir, "a.txt", b"one\ntwo\nthree\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let prefs = EditorPrefs::default();
    let mut ed = EditorUi::default();
    let id = ed.open_path(&p, false);
    ed.docs.get_mut(&id).unwrap().ensure_loaded_now(&prefs);
    assert!(ed.docs.get_mut(&id).unwrap().apply_user_edit(crate::editor::compare::minimal_edit("one\ntwo\nthree\n", "one\nTWO\nthree\nfour\n")));
    let diff = ed.compare(id, Target::Disk, &prefs, Lang::En).unwrap().expect("Vergleich");
    assert_eq!(ed.compare(id, Target::Disk, &prefs, Lang::En).unwrap(), Some(diff), "derselbe Vergleich: kein zweiter Reiter");
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
    let dt = &ed.diffs[&diff];
    assert_eq!(dt.title(&ed.docs, Lang::En), "a.txt (saved) ↔ a.txt");
    assert_eq!(dt.hunk_count(), 2);
    assert!(!dt.unified_shown, "breit: nebeneinander");
    assert!(dt.notes(&ed.docs, Lang::En).is_empty(), "gleiche Kodierung und Zeilenenden");
    // Links (gespeicherte Datei) ist schreibgeschützt: nur Pfeile nach rechts.
    assert!(!dt.arrows.is_empty() && dt.arrows.iter().all(|(_, _, to_left)| !to_left), "{:?}", dt.arrows);
    // Pfeil des ersten Blocks klicken: rechts wird wie links („two“).
    let r = dt.arrows.iter().find(|(_, k, _)| *k == 0).unwrap().0;
    frame(&ctx, &mut ed, click_at(r.center()), &mut t, WIDE, &prefs);
    assert_eq!(text_of(&ed, id), "one\ntwo\nthree\nfour\n");
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
    assert_eq!(ed.diffs[&diff].hunk_count(), 1, "live neu verglichen");
    // Ein Rückgängig-Schritt (über das Dokument, wie Strg+Z im Datei-Reiter)
    assert!(ed.docs.get_mut(&id).unwrap().undo_user());
    assert_eq!(text_of(&ed, id), "one\nTWO\nthree\nfour\n");
    // Nach links geht nicht (schreibgeschützt)
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
    let mut docs = std::mem::take(&mut ed.docs);
    assert!(ed.diffs.get_mut(&diff).unwrap().apply(0, true, &mut docs, Lang::En).is_err());
    ed.docs = docs;
    // Patch und Block
    let dt = ed.diffs.get_mut(&diff).unwrap();
    let patch = dt.patch(&ed.docs, true).unwrap();
    assert!(patch.starts_with("diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@"), "{patch}");
    dt.current = None;
    dt.step(true);
    assert_eq!(dt.current, Some(0));
    assert!(dt.block_text().unwrap().contains("-two\n+TWO\n"));
    dt.step(false);
    assert_eq!(dt.current, Some(1), "rückwärts ringsum");
    // Schließen: das Dokument hat noch seinen Datei-Reiter
    let (g, _) = ed.panes.diff_tab(diff).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    ed.close_diff(g, diff, &data);
    assert!(ed.diffs.is_empty() && ed.docs.contains_key(&id));
    assert_eq!(ed.panes.tabs_of(id).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_files_both_ways_fillers_unified_and_dirty_side_stays_open() {
    let dir = test_dir("two");
    let a = write(&dir, "a.json", b"{\n  \"name\": \"demo\",\n  \"color\": \"red\",\n  \"size\": 3\n}\n");
    let b = write(&dir, "b.json", b"{\n  \"name\": \"demo\",\n  \"colour\": \"red\",\n  \"size\": 3,\n  \"extra\": true\n}\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let prefs = EditorPrefs::default();
    let mut ed = EditorUi::default();
    let diff = ed.compare_paths(&a, &b, &prefs).unwrap();
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
    let dt = &ed.diffs[&diff];
    assert_eq!(dt.title(&ed.docs, Lang::En), "a.json ↔ b.json");
    let out = dt.out.clone().unwrap();
    let rows = crate::editor::compare::rows(&out.hunks, out.left.len(), out.right.len(), false);
    assert!(rows.iter().any(|r| r.kind == RowKind::Insert && r.left.is_none()), "Füllzeile links für die neue Zeile");
    // Beide Seiten sind Dateien: Pfeile in beide Richtungen
    assert!(dt.arrows.iter().any(|a| a.2) && dt.arrows.iter().any(|a| !a.2));
    let (la, lb) = (dt.left.doc().unwrap(), dt.right.doc().unwrap());
    // Rechts → links für alle Blöcke: links wird wie rechts
    let n = dt.hunk_count();
    let mut docs = std::mem::take(&mut ed.docs);
    for _ in 0..n {
        ed.diffs.get_mut(&diff).unwrap().apply(0, true, &mut docs, Lang::En).unwrap();
        ed.docs = std::mem::take(&mut docs);
        settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
        docs = std::mem::take(&mut ed.docs);
    }
    ed.docs = docs;
    assert_eq!(text_of(&ed, la), text_of(&ed, lb));
    assert!(ed.docs[&la].dirty() && !ed.docs[&lb].dirty());
    assert_eq!(ed.diffs[&diff].summary(Lang::En), "identical");
    // Schmal: automatisch untereinander
    settle(&ctx, &mut ed, &mut t, Vec2::new(620.0, 700.0), &prefs, |ed| ed.diffs.values().all(|d| d.unified_shown));
    // Tauschen
    ed.swap_diff(diff);
    assert_eq!(ed.diffs[&diff].left.doc(), Some(lb));
    // Schließen: die geänderte linke Datei bleibt als Datei-Reiter offen, die rechte wird frei
    let (g, _) = ed.panes.diff_tab(diff).unwrap();
    ed.close_diff(g, diff, &dir);
    assert!(ed.docs.contains_key(&la), "ungespeicherte Änderungen gehen nicht verloren");
    assert!(!ed.docs.contains_key(&lb));
    assert_eq!(ed.panes.file_tabs_of(la).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_tabs_are_restored_after_a_restart_and_split_gets_its_own_view() {
    let dir = test_dir("restore");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let p = write(&dir, "r.txt", b"x\ny\n");
    let q = write(&dir, "q.txt", b"x\nz\n");
    let prefs = EditorPrefs::default();
    let mut ed = EditorUi::restore(&data);
    let id = ed.open_path(&p, false);
    ed.docs.get_mut(&id).unwrap().ensure_loaded_now(&prefs);
    let d1 = ed.compare(id, Target::Disk, &prefs, Lang::En).unwrap().unwrap();
    let d2 = ed.compare_paths(&p, &q, &prefs).unwrap();
    ed.diffs.get_mut(&d2).unwrap().ignore_ws = true;
    // Zwischenablage wird nicht gemerkt
    let clip = ed.open_diff(super::diff_tab::DiffTab::new(Side::Text(Box::new(super::diff_tab::StaticSide::clipboard("clip"))), Side::Doc(id), &prefs)).unwrap();
    assert!(ed.diffs[&clip].to_saved(clip).is_none());
    // Teilen: eigener Vergleich mit eigener Kennung
    let g = ed.panes.focused;
    let ng = ed.panes.split(g, crate::layout::Dir::Horizontal).unwrap();
    ed.fix_split_diff(ng);
    let copy = ed.panes.groups[&ng].active_tab().unwrap().diff.unwrap();
    assert_ne!(copy, clip);
    ed.save(&data);
    let back = EditorUi::restore(&data);
    let titles: Vec<String> = back.diffs.values().map(|d| d.title(&back.docs, Lang::En)).collect();
    assert!(titles.contains(&"r.txt (saved) ↔ r.txt".to_string()), "{titles:?}");
    assert!(titles.contains(&"r.txt ↔ q.txt".to_string()), "{titles:?}");
    assert!(back.diffs.values().any(|d| d.ignore_ws));
    assert!(back.diffs.len() == 2, "ohne die Zwischenablage: {titles:?}");
    assert!(back.panes.diffs().len() == 2);
    let _ = (d1, std::fs::remove_dir_all(&dir));
}

#[test]
fn binary_and_large_files_show_sizes_and_checksums() {
    let dir = test_dir("big");
    let a = write(&dir, "a.bin", b"PK\x03\x04\0\0abc");
    let b = write(&dir, "b.bin", b"PK\x03\x04\0\0abd");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let prefs = EditorPrefs::default();
    let mut ed = EditorUi::default();
    let diff = ed.compare_paths(&a, &b, &prefs).unwrap();
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
    let big = ed.diffs[&diff].out.as_ref().unwrap().big.clone().expect("Binär");
    assert!(big.binary);
    assert_eq!(big.left.0, 9);
    assert_eq!(big.left.1, super::diff_tab::sha_hex(b"PK\x03\x04\0\0abc"));
    assert!(ed.diffs[&diff].patch(&ed.docs, true).is_none(), "kein Patch für Binärdateien");
    // Über der Grenze für große Dateien: nicht zeilenweise
    let c = write(&dir, "c.txt", "line\n".repeat(400).as_bytes());
    let d = write(&dir, "d.txt", "line\n".repeat(401).as_bytes());
    let small = EditorPrefs { large_file_bytes: 1000, ..Default::default() };
    let mut ed = EditorUi::default();
    let diff = ed.compare_paths(&c, &d, &small).unwrap();
    settle(&ctx, &mut ed, &mut t, WIDE, &small, diff_settled);
    let big = ed.diffs[&diff].out.as_ref().unwrap().big.clone().expect("zu groß");
    assert!(!big.binary && big.left.0 == 2000 && big.right.0 == 2005);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn markers_are_drawn_click_opens_the_popup_revert_and_f7() {
    let dir = test_dir("marks");
    let p = write(&dir, "m.txt", b"alpha\nbeta\ngamma\ndelta\n");
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let prefs = EditorPrefs { markers: true, marker_base: crate::editor_svc::settings::MarkerBase::Disk, ..Default::default() };
    let mut ed = EditorUi::default();
    let id = ed.open_path(&p, false);
    ed.docs.get_mut(&id).unwrap().ensure_loaded_now(&prefs);
    ed.docs.get_mut(&id).unwrap().apply_user_edit(crate::editor::compare::minimal_edit("alpha\nbeta\ngamma\ndelta\n", "alpha\nBETA\ngamma\ndelta\nnew\n"));
    let rev = ed.docs[&id].revision();
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, |ed| ed.docs[&id].marks.result.as_ref().is_some_and(|m| m.hunks.len() == 2) && ed.docs[&id].revision() == rev);
    // Klick auf den Streifen in Zeile 2 öffnet das Fenster
    let g = ed.panes.focused;
    let rect = ed.group_rects[&g];
    let font = egui::FontId::monospace(prefs.font_size);
    let (row_h, cw) = ctx.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
    let gutter_right = rect.left() + (3.0 + 1.5) * cw + 8.0;
    let y = rect.top() + view::BAR_H + 2.0 + row_h * 1.5;
    frame(&ctx, &mut ed, click_at(Pos2::new(gutter_right + 2.0, y)), &mut t, WIDE, &prefs);
    assert_eq!(ed.mark_popup.as_ref().map(|p| p.line), Some(1), "Fenster unter Zeile 2");
    frame(&ctx, &mut ed, vec![], &mut t, WIDE, &prefs);
    // F7: zur nächsten Änderung (vom Anfang: Zeile 2, dann Zeile 5)
    assert!(ed.step_change(true, &prefs, Lang::En).is_none());
    let (line, _) = ed.panes.groups[&g].tabs[0].view.cursor_linecol(&ed.docs[&id]);
    assert_eq!(line, 1);
    assert!(ed.step_change(true, &prefs, Lang::En).is_none());
    let (line, _) = ed.panes.groups[&g].tabs[0].view.cursor_linecol(&ed.docs[&id]);
    assert_eq!(line, 4);
    assert_eq!(ed.mark_popup.as_ref().map(|p| p.line), Some(4), "das Fenster wandert mit");
    // Veraltete Marken (Text inzwischen geändert): nichts zurücksetzen, Hinweis, neu berechnen
    let before = text_of(&ed, id);
    ed.docs.get_mut(&id).unwrap().apply_user_edit(crate::editor::compare::minimal_edit(&before, &format!("x{before}")));
    assert!(ed.revert_mark(id, 1, Lang::En).is_err());
    assert_eq!(text_of(&ed, id), format!("x{before}"), "nichts zurückgesetzt");
    assert!(ed.docs.get_mut(&id).unwrap().undo_user());
    let rev = ed.docs[&id].revision();
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, |ed| ed.docs[&id].marks.fresh_for(rev));
    // Zurücksetzen: ein Rückgängig-Schritt
    assert!(ed.revert_mark(id, 1, Lang::En).unwrap());
    assert_eq!(text_of(&ed, id), "alpha\nbeta\ngamma\ndelta\nnew\n");
    assert!(ed.mark_popup.is_none());
    assert!(ed.docs.get_mut(&id).unwrap().undo_user());
    assert_eq!(text_of(&ed, id), "alpha\nBETA\ngamma\ndelta\nnew\n");
    // Ohne Marken: Hinweis statt Sprung
    let off = EditorPrefs::default();
    assert!(ed.step_change(true, &off, Lang::En).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// git in einem Test-Repository (übersprungen ohne git): Marken gestagt/nicht gestagt und Vergleich mit HEAD.
#[test]
fn git_markers_and_head_comparison_in_a_temp_repo() {
    let ok = std::process::Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("git fehlt – übersprungen");
        return;
    }
    let dir = test_dir("git");
    let git = |args: &[&str]| {
        let mut c = std::process::Command::new("git");
        c.args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]).args(args).current_dir(&dir);
        crate::platform::process::hide_window(&mut c);
        assert!(c.output().unwrap().status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    let p = write(&dir, "f.txt", b"one\ntwo\nthree\nfour\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c1"]);
    // gestagt: two → TWO; danach ungestagt: four → FOUR
    std::fs::write(&p, "one\nTWO\nthree\nfour\n").unwrap();
    git(&["add", "-A"]);
    std::fs::write(&p, "one\nTWO\nthree\nFOUR\n").unwrap();
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let prefs = EditorPrefs { markers: true, ..Default::default() };
    let mut ed = EditorUi::default();
    let id = ed.open_path(&p, false);
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, |ed| ed.docs[&id].marks.result.as_ref().is_some_and(|m| m.hunks.len() == 2));
    let m = ed.docs[&id].marks.result.clone().unwrap();
    assert_eq!(m.hunks.iter().map(|h| (h.lines.start, h.staged)).collect::<Vec<_>>(), vec![(1, true), (3, false)]);
    assert!(ed.docs[&id].marks.base.as_ref().unwrap().is_git());
    assert_eq!(super::marks::diff_target(&m.hunks[0], ed.docs[&id].marks.base.as_deref()), Target::Head);
    assert_eq!(super::marks::diff_target(&m.hunks[1], ed.docs[&id].marks.base.as_deref()), Target::Index);
    // Vergleich mit HEAD: beide Änderungen
    let diff = ed.compare(id, Target::Head, &prefs, Lang::En).unwrap().unwrap();
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
    assert_eq!(ed.diffs[&diff].hunk_count(), 2);
    assert_eq!(ed.diffs[&diff].title(&ed.docs, Lang::En), "f.txt (HEAD) ↔ f.txt");
    let patch = ed.diffs[&diff].patch(&ed.docs, true).unwrap();
    assert!(patch.starts_with("diff --git a/f.txt b/f.txt\n"), "{patch}");
    // git apply nimmt den Patch (gegen HEAD) an
    let pf = dir.join("x.patch");
    std::fs::write(&pf, &patch).unwrap();
    git(&["stash", "-q"]);
    git(&["apply", "--check", "x.patch"]);
    // Unbekannte Revision: Fehler, kein Reiter
    assert!(ed.compare(id, Target::Rev("--all".into()), &prefs, Lang::En).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replacing_a_dirty_side_keeps_it_open_and_unreadable_sides_stop_working() {
    let dir = test_dir("side");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let a = write(&dir, "a.txt", b"one\n");
    let b = write(&dir, "b.txt", b"two\n");
    let prefs = EditorPrefs::default();
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut ed = EditorUi::default();
    let diff = ed.compare_paths(&a, &b, &prefs).unwrap();
    settle(&ctx, &mut ed, &mut t, WIDE, &prefs, diff_settled);
    let (la, lb) = (ed.diffs[&diff].left.doc().unwrap(), ed.diffs[&diff].right.doc().unwrap());
    // linke Seite ungespeichert ändern, dann durch einen festen Stand ersetzen
    let mut docs = std::mem::take(&mut ed.docs);
    ed.diffs.get_mut(&diff).unwrap().apply(0, true, &mut docs, Lang::En).unwrap();
    ed.docs = docs;
    assert!(ed.docs[&la].dirty());
    ed.set_diff_left(diff, Side::Text(Box::new(super::diff_tab::StaticSide::new(super::diff_tab::Source::Disk(b.clone())))), &data);
    assert!(ed.docs.contains_key(&la), "ungespeicherte Seite bleibt offen");
    assert_eq!(ed.panes.file_tabs_of(la).len(), 1, "als Datei-Reiter");
    let (g, i) = ed.panes.diff_tab(diff).unwrap();
    let tab = &ed.panes.groups[&g].tabs[i];
    assert_eq!((tab.doc, tab.extra), (lb, None), "Reiter kennt die neuen Seiten");
    assert_eq!(ed.panes.groups[&g].active, i, "der Vergleich bleibt aktiv");
    // Tauschen hält die Dokumente des Reiters aktuell
    ed.swap_diff(diff);
    let tab = &ed.panes.groups[&g].tabs[i];
    assert_eq!((tab.doc, tab.extra), (lb, None));
    assert!(ed.docs.contains_key(&lb));
    // Unlesbare Seite: Meldung, kein ewiges Neuzeichnen
    let mut ed2 = EditorUi::default();
    let gone = dir.join("missing-dir").join("x.txt");
    let id = ed2.alloc_doc_for_test(&gone);
    ed2.docs.get_mut(&id).unwrap().load_error = Some("not readable".into());
    let other = ed2.compare_paths(&a, &b, &prefs).unwrap();
    let dt = ed2.diffs.get_mut(&other).unwrap();
    dt.set_left(Side::Doc(id));
    let mut docs = std::mem::take(&mut ed2.docs);
    assert!(dt.side_error(&docs).is_some());
    for _ in 0..3 {
        assert!(!dt.tick(&mut docs, &prefs, 0, Instant::now()) || dt.side_error(&docs).is_none(), "nicht beschäftigt");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
