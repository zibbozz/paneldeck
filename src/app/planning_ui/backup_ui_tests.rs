//! Tests der Import-Läufe im Hintergrund (`backup_run.rs`, `backup_ed.rs`): Datei lesen, Fortschritt, Abbruch, Fehler und der Dialog
//! „Import läuft“ – ohne Fenster und ohne Maus/Tastatur des Nutzers (egui läuft mit künstlichen Eingaben, die Daten liegen in
//! Testdatenbanken als Dateien, also laufen die Läufe wirklich im Faden). Große Dateien (5000 Tickets) werden einmal erzeugt.

use super::backup_run::{load_csv_file, load_project_file, open_writer, start_csv_import, start_load_csv, start_load_project, start_project_import, LoadFail, Poll, Task};
use super::{Cx, PlanUi};
use crate::app::tasks_ui::Env;
use crate::db::Db;
use crate::planning::backup_doc::tickets_csv::{export_tickets_csv, import_tickets_csv_with, CsvOptions};
use crate::planning::backup_doc::{export_project, import_project_with, BackupError, Problem, ProjectDoc, Progress, RefKind, Where};
use crate::planning::history::Source;
use crate::planning::projects::{create_project, NewProject};
use crate::planning::tickets::{create_ticket, NewTicket};
use crate::planning::types::list_types;
use crate::state::Lang;
use eframe::egui::{self, Color32, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Tickets der großen Testdatei.
const N: usize = 5000;

fn temp_dir(tag: &str) -> PathBuf {
    static C: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("paneldeck-test-backuprun-{tag}-{}-{}", std::process::id(), C.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open_db(tag: &str) -> Db {
    let d = temp_dir(tag);
    crate::db::open(&d, &d.join("backups")).unwrap()
}

fn mk_project(db: &Db, key: &str) -> i64 {
    let mut np = NewProject::new(key, &format!("Project {key}"), "#336699");
    np.with_defaults = true;
    create_project(&db.conn, &np).unwrap()
}

fn seed(db: &Db, project: i64, n: usize, prefix: &str) {
    let ty = list_types(&db.conn, project).unwrap().into_iter().find(|t| !t.system).unwrap().id;
    db.conn.execute_batch("BEGIN").unwrap();
    for i in 0..n {
        create_ticket(&db.conn, &NewTicket::new(project, ty, &format!("{prefix} {i}")), &Source::ui()).unwrap();
    }
    db.conn.execute_batch("COMMIT").unwrap();
}

/// Anzahl der Zeilen je Tabelle (zum Vergleichen vorher/nachher).
fn fingerprint(c: &Connection) -> Vec<(String, i64)> {
    let names: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect();
    names.into_iter().map(|n| (n.clone(), c.query_row(&format!("SELECT COUNT(*) FROM \"{n}\""), [], |r| r.get::<_, i64>(0)).unwrap())).collect()
}

fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Die große Quelle: Projekt `SRC` mit `N` Tickets als Sicherung (Dokument und JSON-Datei) und als CSV (Text und Datei).
struct Fixtures {
    doc: Arc<ProjectDoc>,
    json: PathBuf,
    csv: PathBuf,
    csv_text: Arc<String>,
}

fn fixtures() -> &'static Fixtures {
    static F: OnceLock<Fixtures> = OnceLock::new();
    F.get_or_init(|| {
        let db = open_db("source");
        let p = mk_project(&db, "SRC");
        seed(&db, p, N, "Ticket");
        let doc = export_project(&db.conn, p).unwrap();
        assert_eq!(doc.tickets.len(), N);
        let dir = temp_dir("files");
        let json = dir.join("source.json");
        std::fs::write(&json, doc.to_json()).unwrap();
        let csv_text = export_tickets_csv(&db.conn, p, &CsvOptions::for_ui()).unwrap();
        let csv = dir.join("source.csv");
        std::fs::write(&csv, &csv_text).unwrap();
        Fixtures { doc: Arc::new(doc), json, csv, csv_text: Arc::new(csv_text) }
    })
}

/// Fragt den Lauf ab, bis er fertig ist (mit Zeitgrenze).
fn finish<T>(task: &mut Task<T>) -> T {
    let t = Instant::now();
    loop {
        match task.poll() {
            Poll::Done(v) => return v,
            Poll::Lost => panic!("der Faden ist verschwunden"),
            Poll::Pending => {}
        }
        assert!(t.elapsed() < Duration::from_secs(120), "Zeitüberschreitung");
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ------------------------------------------------------------------ Logik

#[test]
fn a_project_import_in_the_background_creates_a_new_project_and_counts_its_steps() {
    let f = fixtures();
    let db = open_db("target");
    mk_project(&db, "QQ");
    let before = count(&db.conn, "SELECT COUNT(*) FROM projects");
    let tickets_before = count(&db.conn, "SELECT COUNT(*) FROM tickets");
    let mut task = start_project_import(|| {}, &db, f.doc.clone(), None);
    let res = finish(&mut task).expect("Import gelungen");
    assert_eq!(res.key, "SRC");
    assert_eq!(res.report.tickets, N);
    assert_eq!(count(&db.conn, "SELECT COUNT(*) FROM projects"), before + 1, "genau ein neues Projekt");
    assert_eq!(count(&db.conn, &format!("SELECT COUNT(*) FROM tickets WHERE project_id = {}", res.project_id)), N as i64);
    assert_eq!(count(&db.conn, "SELECT COUNT(*) FROM tickets"), tickets_before + N as i64, "Bestehendes bleibt unverändert");
    let p = &task.progress;
    assert_eq!(p.total(), N * 7);
    assert_eq!(p.done(), N * 7, "jeder Schritt ist gezählt");
    assert_eq!(p.fraction(), Some(1.0));
    // mit anderem Kürzel: neu nummeriert
    let mut t2 = start_project_import(|| {}, &db, f.doc.clone(), Some("ZED".into()));
    let r2 = finish(&mut t2).unwrap();
    assert!(r2.renumbered && r2.key == "ZED");
    // dasselbe Kürzel noch einmal: Fehler, nichts verändert
    let fp = fingerprint(&db.conn);
    let mut t3 = start_project_import(|| {}, &db, f.doc.clone(), None);
    assert!(matches!(finish(&mut t3), Err(BackupError::KeyTaken(k)) if k == "SRC"));
    assert_eq!(fingerprint(&db.conn), fp);
}

#[test]
fn cancelling_a_running_project_import_rolls_everything_back() {
    let f = fixtures();
    let db = open_db("cancel");
    mk_project(&db, "QQ");
    let fp = fingerprint(&db.conn);
    let mut task = start_project_import(|| {}, &db, f.doc.clone(), None);
    // warten, bis der Import wirklich schreibt, dann abbrechen
    let t = Instant::now();
    while task.progress.done() == 0 {
        assert!(t.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(1));
    }
    task.progress.cancel();
    assert_eq!(finish(&mut task).unwrap_err(), BackupError::Cancelled);
    assert!(task.progress.done() < N * 7, "es wurde vorzeitig beendet");
    assert_eq!(fingerprint(&db.conn), fp, "nichts angelegt");
    // die Sperre ist frei: ein neuer Import klappt
    let mut again = start_project_import(|| {}, &db, f.doc.clone(), None);
    assert_eq!(finish(&mut again).unwrap().report.tickets, N);
}

#[test]
fn the_core_import_stops_between_tickets_and_the_savepoint_takes_everything_back() {
    let f = fixtures();
    let db = open_db("core");
    let q = mk_project(&db, "QQ");
    let fp = fingerprint(&db.conn);
    let w = open_writer(&db.path).unwrap();
    // Projektsicherung: in jedem der sieben Läufe abbrechen
    for at in [1, 10, N + 5, 3 * N + 7, 6 * N + 1] {
        let p = Progress::cancelling_after(at);
        assert_eq!(import_project_with(&w, &f.doc, None, &p).unwrap_err(), BackupError::Cancelled, "nach {at} Schritten");
        assert_eq!(fingerprint(&db.conn), fp, "nach {at} Schritten: nichts bleibt zurück");
    }
    // CSV: im Prüflauf, im Anlegen und im Nachbearbeiten
    for at in [1, 100, N + 50, 2 * N + 50] {
        let p = Progress::cancelling_after(at);
        let r = import_tickets_csv_with(&w, q, &f.csv_text, &Default::default(), &p);
        assert_eq!(r.unwrap_err(), BackupError::Cancelled, "CSV nach {at} Schritten");
        assert_eq!(fingerprint(&db.conn), fp, "CSV nach {at} Schritten: nichts bleibt zurück");
    }
    // ohne Abbruch klappt es auf derselben Verbindung
    let p = Progress::default();
    assert_eq!(import_tickets_csv_with(&w, q, &f.csv_text, &Default::default(), &p).unwrap().created, N);
    assert_eq!((p.total(), p.done()), (N * 3, N * 3));
    assert_eq!(count(&db.conn, &format!("SELECT COUNT(*) FROM tickets WHERE project_id = {q}")), N as i64);
}

#[test]
fn a_broken_document_fails_with_the_place_and_changes_nothing() {
    let f = fixtures();
    let db = open_db("broken");
    let q = mk_project(&db, "QQ");
    let fp = fingerprint(&db.conn);
    let mut doc = (*f.doc).clone();
    let bad = doc.tickets[N - 3].number;
    doc.tickets[N - 3].type_id = 99_999;
    let mut task = start_project_import(|| {}, &db, Arc::new(doc), None);
    match finish(&mut task) {
        Err(BackupError::At(Where::Ticket(n), Problem::UnknownRef(RefKind::Type, 99_999))) => assert_eq!(n, bad),
        other => panic!("{other:?}"),
    }
    assert_eq!(fingerprint(&db.conn), fp, "ein Fehler am Ende nimmt auch die ersten Tickets zurück");
    // eine CSV mit unbekanntem Typ in der letzten Zeile: keine Zeile wird angelegt
    let mut lines: Vec<String> = f.csv_text.lines().map(str::to_string).collect();
    let last = lines.len() - 1;
    let mut cells: Vec<&str> = lines[last].splitn(3, ',').collect();
    cells[1] = "Gibt es nicht";
    lines[last] = cells.join(",");
    let mut t2 = start_csv_import(|| {}, &db, q, Arc::new(lines.join("\n")));
    match finish(&mut t2) {
        Err(BackupError::CsvRows(rows)) => assert!(!rows.is_empty()),
        other => panic!("{other:?}"),
    }
    assert_eq!(fingerprint(&db.conn), fp);
}

#[test]
fn reading_a_file_needs_no_database_and_reports_what_is_wrong() {
    let f = fixtures();
    let l = load_project_file(&f.json).unwrap();
    assert_eq!(l.preview.tickets, N);
    assert_eq!(l.preview.key, "SRC");
    assert_eq!(l.doc.tickets.len(), N);
    let c = load_csv_file(&f.csv).unwrap();
    assert_eq!(c.rows, N);
    // Fehler
    let dir = temp_dir("bad");
    assert!(matches!(load_project_file(&dir.join("fehlt.json")), Err(LoadFail::Io(_))));
    let junk = dir.join("junk.json");
    std::fs::write(&junk, "kein json").unwrap();
    assert!(matches!(load_project_file(&junk), Err(LoadFail::Backup(BackupError::Json(_)))));
    std::fs::write(&junk, r#"{"format":"anders","version":1}"#).unwrap();
    assert!(matches!(load_project_file(&junk), Err(LoadFail::Backup(BackupError::Format))));
    let bin = dir.join("bin.csv");
    std::fs::write(&bin, [0xff_u8, 0xfe, 0xfd]).unwrap();
    assert!(matches!(load_csv_file(&bin), Err(LoadFail::Backup(BackupError::Csv(_)))));
    // im Hintergrund dasselbe, mit Wecker
    let woke = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let w2 = woke.clone();
    let mut t = start_load_project(move || w2.store(true, std::sync::atomic::Ordering::SeqCst), f.json.clone());
    assert_eq!(finish(&mut t).unwrap().preview.tickets, N);
    assert!(woke.load(std::sync::atomic::Ordering::SeqCst), "der Faden weckt die Oberfläche");
    let mut t = start_load_csv(|| {}, f.csv.clone());
    assert_eq!(finish(&mut t).unwrap().rows, N);
}

#[test]
fn a_task_without_a_thread_runs_inline_and_a_forgotten_task_is_harmless() {
    let mut t = Task::inline(|p| {
        p.set_total(2);
        let _ = p.step();
        7
    });
    assert_eq!(t.progress.done(), 1, "gleich gelaufen");
    assert_eq!(t.poll(), Poll::Done(7));
    assert_eq!(t.poll(), Poll::Lost, "das Ergebnis wird nur einmal abgeholt");
    // ein Lauf, dessen Ergebnis keiner mehr will (Dialog geschlossen): der Faden endet still
    let t = Task::spawn(
        || {},
        |_| {
            std::thread::sleep(Duration::from_millis(30));
            1
        },
    )
    .unwrap();
    drop(t);
    std::thread::sleep(Duration::from_millis(80));
}

// ------------------------------------------------------------------ Oberfläche

struct H {
    ctx: egui::Context,
    db: Db,
    ui: PlanUi,
    lang: Lang,
    time: f64,
    project: i64,
}

fn key_event(key: Key, pressed: bool) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
}

impl H {
    fn new(lang: Lang) -> H {
        let db = open_db("ui");
        let project = mk_project(&db, "QQ");
        db.set_meta("expert_mode", "1");
        db.set_meta("expert_project", &project.to_string());
        let mut h = H { ctx: egui::Context::default(), db, ui: PlanUi::default(), lang, time: 0.0, project };
        h.frames(3);
        h
    }

    fn frame_out(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 0.1;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))), time: Some(self.time), events, focused: true, ..Default::default() };
        let H { ctx, db, ui, lang, .. } = self;
        let env = Env { db: Some(db), db_error: None, lang: *lang, bg: Color32::from_rgb(24, 24, 28), undo_steps: 25 };
        ctx.run(input, |ctx| {
            ui.ensure_loaded(Some(db));
            ui.header(ctx, &env, db);
            if ui.expert {
                ui.page(ctx, &env, db);
            }
        })
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let _ = self.frame_out(Vec::new());
        }
    }

    /// Bilder laufen lassen, bis `done` wahr ist; liefert die Ausgaben aller Bilder.
    fn wait(&mut self, what: &str, done: impl Fn(&mut H) -> bool) -> Vec<egui::FullOutput> {
        let t = Instant::now();
        let mut outs = Vec::new();
        loop {
            outs.push(self.frame_out(Vec::new()));
            if done(self) {
                return outs;
            }
            assert!(t.elapsed() < Duration::from_secs(120), "Zeitüberschreitung: {what}");
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut PlanUi, &egui::Context, &Cx) -> R) -> R {
        let cx = Cx { conn: &self.db.conn, db: &self.db, lang: self.lang };
        f(&mut self.ui, &self.ctx, &cx)
    }

    fn busy(&self) -> bool {
        self.ui.templates.backup.job.is_some()
    }

    fn done_steps(&self) -> usize {
        self.ui.templates.backup.job.as_ref().map_or(0, |j| j.progress().done())
    }

    fn msg(&self) -> String {
        self.ui.msg.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    /// Wartet, bis der Lauf mindestens `n` Schritte getan hat.
    fn wait_steps(&self, n: usize) {
        let t = Instant::now();
        while self.done_steps() < n {
            assert!(t.elapsed() < Duration::from_secs(60), "der Lauf kommt nicht voran");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Datei lesen lassen und warten, bis der Vorschau-Dialog steht.
    fn load_project(&mut self, path: PathBuf) {
        self.with(|ui, ctx, _| ui.begin_load_project(ctx, path));
        self.wait("Datei gelesen", |h| !h.busy());
    }
}

fn both(f: impl Fn(&mut H)) {
    for lang in [Lang::En, Lang::De] {
        let mut h = H::new(lang);
        f(&mut h);
    }
}

fn texts_of(out: &egui::FullOutput) -> Vec<String> {
    fn walk(s: &egui::Shape, acc: &mut Vec<String>) {
        match s {
            egui::Shape::Text(t) => acc.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, acc)),
            _ => {}
        }
    }
    let mut acc = Vec::new();
    out.shapes.iter().for_each(|c| walk(&c.shape, &mut acc));
    acc
}

fn any_text(outs: &[egui::FullOutput], want: &str) -> bool {
    outs.iter().any(|o| texts_of(o).iter().any(|t| t == want))
}

#[test]
fn a_large_backup_is_read_and_imported_in_the_background_while_the_ui_keeps_running() {
    let f = fixtures();
    both(|h| {
        let lang = h.lang;
        let before = count(&h.db.conn, "SELECT COUNT(*) FROM projects");
        // ---- Lesen: kehrt sofort zurück, der Dialog „Datei wird gelesen“ steht, die Bilder laufen weiter
        let json = f.json.clone();
        h.with(|ui, ctx, _| ui.begin_load_project(ctx, json));
        assert!(h.busy() && h.ui.templates.backup.project.is_none(), "{lang:?}: das Lesen läuft im Hintergrund");
        h.wait("Datei gelesen", |h| !h.busy());
        let p = h.ui.templates.backup.project.as_ref().expect("Vorschau-Dialog");
        assert_eq!((p.preview.tickets, p.preview.key.as_str()), (N, "SRC"));
        assert!(texts_of(&h.frame_out(Vec::new())).iter().any(|t| t == "source.json"), "{lang:?}: der Dateiname steht im Dialog");
        // ---- Import: im Hintergrund auf eigener Verbindung
        let p = h.ui.templates.backup.project.take().unwrap();
        h.with(|ui, ctx, cx| ui.begin_project_import(ctx, cx, p, None));
        assert!(h.busy());
        h.wait_steps(1);
        // Der Import schreibt: die Oberfläche liest weiter (WAL), sieht den Stand davor und hängt nicht.
        let total = h.ui.templates.backup.job.as_ref().unwrap().progress().total();
        let t = Instant::now();
        let seen = count(&h.db.conn, "SELECT COUNT(*) FROM projects");
        let took = t.elapsed();
        if h.done_steps() < total {
            assert_eq!(seen, before, "{lang:?}: mitten im Import ist noch nichts sichtbar");
            assert!(took < Duration::from_secs(2), "{lang:?}: ein Lesezugriff der Oberfläche hängt nicht ({took:?})");
        }
        h.wait("Import fertig", |h| !h.busy());
        assert_eq!(count(&h.db.conn, "SELECT COUNT(*) FROM projects"), before + 1);
        let rep = h.ui.templates.backup.project_report.as_ref().expect("Bericht");
        assert_eq!(rep.report.tickets, N);
        assert_eq!(h.ui.selected, rep.project_id, "das neue Projekt ist gewählt");
        assert_eq!(h.msg(), lang.t("Projekt SRC importiert (5000 Tickets)", "Project SRC imported (5000 tickets)"));
        let out = h.frame_out(Vec::new());
        let done = lang.t("Import abgeschlossen", "Import finished");
        assert!(texts_of(&out).iter().any(|t| t == done), "{lang:?}: Bericht {done:?}");
    });
}

#[test]
fn the_progress_dialog_names_the_work_in_both_languages() {
    let f = fixtures();
    both(|h| {
        let lang = h.lang;
        let json = f.json.clone();
        h.with(|ui, ctx, _| ui.begin_load_project(ctx, json));
        let reading = lang.t("Datei wird gelesen …", "Reading the file …");
        let outs = h.wait("Datei gelesen", |h| !h.busy());
        // die Bilder, in denen noch gelesen wurde, zeigen den Dialog (das letzte Bild hat ihn schon verloren). Das erste Bild
        // zählt nicht: egui zeichnet ein neues Fenster (Modal/Area) dort unsichtbar, um seine Größe zu messen. Liest ein
        // schneller Rechner die Datei in zwei Bildern, gibt es also kein sichtbares Bild mit dem Dialog.
        assert!(outs.len() < 3 || any_text(&outs[1..outs.len() - 1], reading), "{lang:?}: {reading:?}");
        let p = h.ui.templates.backup.project.take().unwrap();
        h.with(|ui, ctx, cx| ui.begin_project_import(ctx, cx, p, None));
        let importing = lang.t("Projekt wird importiert …", "Importing project …");
        let cancel = lang.t("Abbrechen", "Cancel");
        let mut seen = (false, false, false);
        while h.busy() {
            let out = h.frame_out(Vec::new());
            let texts = texts_of(&out);
            seen.0 |= texts.iter().any(|t| t == importing);
            seen.1 |= texts.iter().any(|t| t == cancel);
            seen.2 |= texts.iter().any(|t| t.ends_with('%'));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(seen.0 && seen.1, "{lang:?}: Titel {importing:?} und Knopf {cancel:?} im Dialog (gesehen: {seen:?})");
        h.frames(2);
    });
}

#[test]
fn escape_cancels_a_running_import_and_nothing_is_created() {
    let f = fixtures();
    both(|h| {
        let lang = h.lang;
        let fp = fingerprint(&h.db.conn);
        h.load_project(f.json.clone());
        let p = h.ui.templates.backup.project.take().unwrap();
        // Die Oberfläche hält die Schreibsperre: der Import wartet auf seinen ersten Schreibzugriff, bis Esc gedrückt ist
        // (deterministisch, ohne auf das Tempo des Rechners zu wetten). Die Oberfläche selbst bleibt dabei bedienbar.
        h.db.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        h.with(|ui, ctx, cx| ui.begin_project_import(ctx, cx, p, None));
        let out = h.frame_out(vec![key_event(Key::Escape, true)]);
        let _ = h.frame_out(vec![key_event(Key::Escape, false)]);
        assert!(texts_of(&out).iter().any(|t| t == lang.t("Projekt wird importiert …", "Importing project …")), "{lang:?}: Dialog gezeigt");
        // Ob der Lauf hier noch „busy“ ist, hängt vom Tempo ab (Esc kann ihn abbrechen, bevor er die Sperre erreicht) – nicht prüfen.
        h.db.conn.execute_batch("ROLLBACK").unwrap();
        h.wait("abgebrochen", |h| !h.busy());
        assert_eq!(fingerprint(&h.db.conn), fp, "{lang:?}: nichts angelegt");
        assert!(h.ui.templates.backup.project.is_none() && h.ui.templates.backup.project_report.is_none(), "alles geschlossen");
        assert_eq!(h.msg(), lang.t("Der Import wurde abgebrochen – es wurde nichts angelegt.", "The import was cancelled – nothing was created."));
    });
}

#[test]
fn a_failed_import_reopens_the_dialog_with_the_error() {
    let f = fixtures();
    both(|h| {
        let lang = h.lang;
        let fp = fingerprint(&h.db.conn);
        h.load_project(f.json.clone());
        // das Dokument im Dialog verderben (das letzte Ticket verweist auf einen Typ, den es nicht gibt)
        let mut p = h.ui.templates.backup.project.take().unwrap();
        let mut doc = (*p.doc).clone();
        doc.tickets[N - 1].type_id = 77_777;
        p.doc = Arc::new(doc);
        h.with(|ui, ctx, cx| ui.begin_project_import(ctx, cx, p, None));
        h.wait("Fehler", |h| !h.busy());
        assert_eq!(fingerprint(&h.db.conn), fp, "{lang:?}: ein Fehler nimmt alles zurück");
        let p = h.ui.templates.backup.project.as_ref().expect("Dialog wieder da");
        let e = p.error.clone().expect("Fehler im Dialog");
        assert!(matches!(e, BackupError::At(Where::Ticket(_), Problem::UnknownRef(RefKind::Type, 77_777))), "{e:?}");
        let msg = e.message(lang);
        assert!(msg.contains(&N.to_string()), "mit der Ticketnummer: {msg}");
        let out = h.frame_out(Vec::new());
        assert!(texts_of(&out).iter().any(|t| t.contains(&msg)), "{lang:?}: Meldung {msg:?} im Dialog");
        assert!(h.ui.templates.backup.project_report.is_none());
    });
}

#[test]
fn files_that_cannot_be_read_end_with_a_message_in_both_languages() {
    both(|h| {
        let lang = h.lang;
        let dir = temp_dir("ui-bad");
        // fehlt
        let missing = dir.join("fehlt.json");
        h.load_project(missing);
        assert!(h.msg().starts_with(lang.t("Die Datei konnte nicht gelesen werden", "The file could not be read")), "{}", h.msg());
        assert!(!h.ui.msg.as_ref().unwrap().ok);
        // kein Paneldeck-Dokument
        let junk = dir.join("x.json");
        std::fs::write(&junk, r#"{"format":"x"}"#).unwrap();
        h.load_project(junk);
        assert_eq!(h.msg(), BackupError::Format.message(lang));
        assert!(h.ui.templates.backup.project.is_none());
        // CSV mit ungültigem UTF-8
        let bin = dir.join("x.csv");
        std::fs::write(&bin, [0xff_u8, 0xfe]).unwrap();
        let q = h.project;
        h.with(|ui, ctx, _| ui.begin_load_csv(ctx, bin, q, "QQ"));
        h.wait("CSV kaputt", |h| !h.busy());
        assert!(h.msg().contains("UTF-8"), "{}", h.msg());
        assert!(h.ui.templates.backup.csv.is_none());
    });
}

#[test]
fn a_large_csv_is_read_and_imported_in_the_background_and_cancelling_leaves_no_tickets() {
    let f = fixtures();
    both(|h| {
        let lang = h.lang;
        let q = h.project;
        let tickets = |h: &H| count(&h.db.conn, &format!("SELECT COUNT(*) FROM tickets WHERE project_id = {q}"));
        // Lesen
        let csv = f.csv.clone();
        h.with(|ui, ctx, _| ui.begin_load_csv(ctx, csv, q, "QQ"));
        assert!(h.busy() && h.ui.templates.backup.csv.is_none());
        h.wait("CSV gelesen", |h| !h.busy());
        assert_eq!(h.ui.templates.backup.csv.as_ref().map(|c| c.rows), Some(N));
        // Import abbrechen (Esc), sobald er im Anlege-Lauf ist
        let c = h.ui.templates.backup.csv.take().unwrap();
        h.db.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        h.with(|ui, ctx, cx| ui.begin_csv_import(ctx, cx, c));
        let out = h.frame_out(vec![key_event(Key::Escape, true)]);
        let _ = h.frame_out(vec![key_event(Key::Escape, false)]);
        assert!(texts_of(&out).iter().any(|t| t == lang.t("Tickets werden importiert …", "Importing tickets …")), "{lang:?}: Dialog gezeigt");
        h.db.conn.execute_batch("ROLLBACK").unwrap();
        h.wait("CSV abgebrochen", |h| !h.busy());
        assert_eq!(tickets(h), 0, "{lang:?}: nach dem Abbruch gibt es keine Tickets");
        assert!(h.ui.templates.backup.csv_report.is_none());
        // noch einmal, diesmal bis zum Ende
        let csv = f.csv.clone();
        h.with(|ui, ctx, _| ui.begin_load_csv(ctx, csv, q, "QQ"));
        h.wait("CSV erneut gelesen", |h| !h.busy());
        let c = h.ui.templates.backup.csv.take().unwrap();
        h.with(|ui, ctx, cx| ui.begin_csv_import(ctx, cx, c));
        h.wait("CSV importiert", |h| !h.busy());
        assert_eq!(tickets(h), N as i64);
        let (key, rep) = h.ui.templates.backup.csv_report.as_ref().expect("Bericht");
        assert_eq!((key.as_str(), rep.created), ("QQ", N));
        assert_eq!(h.msg(), lang.t("5000 Ticket(s) importiert", "5000 ticket(s) imported"));
        // die Oberfläche hat neu geladen: die neuen Tickets zählen
        h.frames(4);
        assert_eq!(h.ui.tickets.project_total, N);
    });
}
