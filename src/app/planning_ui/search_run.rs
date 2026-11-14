//! Ausführung der Abfragen für die Oberfläche: ein Hintergrund-Faden mit eigener, nur lesender Verbindung (Muster `dash_data::Worker`),
//! damit eine teure Abfrage die Oberfläche nie anhält. Ohne Datenbank-Datei (im Speicher, Tests) läuft die Abfrage gleich im Vordergrund.
//!
//! * [`QueryRun`] hält die **letzte Anforderung** und das **zuletzt gezeigte Ergebnis**. Veraltete Ergebnisse (eine neuere Anforderung
//!   wurde schon abgeschickt) werden verworfen; beim Nachladen bleiben die alten Zeilen stehen (kein Flackern).
//! * [`SchemaCache`] baut das Schema des Bereichs (`planning::search::search_schema`) nur bei Bedarf neu: anderer Bereich, andere
//!   Sprache oder als veraltet markiert (Daten oder Schlüsselwörter geändert).

use crate::db::Db;
use crate::planning::search::{run_query, search_schema, Conflict, Ctx, Page, Scope, SearchError, SearchPage, TicketRow};
use crate::query::Schema;
use crate::state::Lang;
use eframe::egui;
use rusqlite::Connection;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use super::search_logic::PAGE_SIZE;

/// Zeitlimit einer Abfrage der Oberfläche.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// Eine Abfrage: Bereich, Text (samt angehängter Sortierung), Sprache und Seite (0-basiert).
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub scope: Scope,
    pub jql: String,
    pub lang: Lang,
    pub page: i64,
}

/// Ein fertiges Ergebnis mit der Anforderung, zu der es gehört.
#[derive(Debug)]
pub struct Finished {
    pub req: Request,
    pub result: Result<SearchPage, SearchError>,
    pub took: Duration,
}

/// Führt die Anforderung auf der Verbindung aus (mit Zeitlimit).
pub fn execute(conn: &Connection, req: &Request) -> Finished {
    let t = Instant::now();
    let ctx = Ctx::today_local(req.lang).with_timeout(TIMEOUT);
    let result = run_query(conn, &ctx, req.scope, &req.jql, Page::new(PAGE_SIZE, req.page.max(0) * PAGE_SIZE));
    Finished { req: req.clone(), result, took: t.elapsed() }
}

// ------------------------------------------------------------------ Hintergrund-Faden

struct Job {
    id: u64,
    req: Request,
}

struct Done {
    id: u64,
    fin: Finished,
}

struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Done>,
}

fn open_reader(path: &std::path::Path) -> Result<Connection, String> {
    let c = Connection::open(path).map_err(|e| e.to_string())?;
    c.busy_timeout(Duration::from_secs(3)).map_err(|e| e.to_string())?;
    // Der Leser schreibt nie.
    c.pragma_update(None, "query_only", true).map_err(|e| e.to_string())?;
    Ok(c)
}

impl Worker {
    /// Startet den Faden; `wake` weckt die Oberfläche nach jedem Ergebnis. Der Faden endet, wenn der `Worker` verworfen wird.
    fn spawn(path: std::path::PathBuf, wake: impl Fn() + Send + 'static) -> Option<Worker> {
        let (tx, jobs) = channel::<Job>();
        let (done_tx, rx) = channel::<Done>();
        std::thread::Builder::new()
            .name("search-loader".into())
            .spawn(move || {
                let mut conn: Option<Result<Connection, String>> = None;
                while let Ok(mut job) = jobs.recv() {
                    // Wartet schon eine neuere Anforderung, ist die ältere überholt: überspringen.
                    while let Ok(newer) = jobs.try_recv() {
                        job = newer;
                    }
                    let c = conn.get_or_insert_with(|| open_reader(&path));
                    let fin = match c {
                        Ok(c) => execute(c, &job.req),
                        Err(e) => Finished { req: job.req.clone(), result: Err(SearchError::Db(e.clone())), took: Duration::ZERO },
                    };
                    if done_tx.send(Done { id: job.id, fin }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .ok()?;
        Some(Worker { tx, rx })
    }
}

// ------------------------------------------------------------------ Zustand einer Ergebnisliste

/// Zustand einer Ergebnisliste: die letzte Anforderung, ob etwas läuft, und das zuletzt gezeigte Ergebnis.
#[derive(Default)]
pub struct QueryRun {
    worker: Option<Worker>,
    worker_failed: bool,
    next_id: u64,
    /// Kennung des Auftrags, auf dessen Ergebnis gewartet wird (andere Ergebnisse sind veraltet).
    inflight: Option<u64>,
    /// Die zuletzt abgeschickte Anforderung.
    last: Option<Request>,
    /// Neu ausführen, auch wenn die Anforderung dieselbe ist (Daten geändert, „Suchen“ erneut gedrückt).
    stale: bool,
    /// Das zuletzt eingetroffene Ergebnis (bleibt beim Nachladen stehen).
    pub shown: Option<Finished>,
}

impl QueryRun {
    /// Als veraltet markieren: beim nächsten [`QueryRun::drive`] läuft dieselbe Anforderung noch einmal.
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Läuft gerade eine Abfrage im Hintergrund?
    pub fn loading(&self) -> bool {
        self.inflight.is_some()
    }

    /// Die zuletzt abgeschickte Anforderung (für Tests).
    #[cfg(test)]
    pub fn requested(&self) -> Option<&Request> {
        self.last.as_ref()
    }

    pub fn rows(&self) -> &[TicketRow] {
        match self.shown.as_ref().map(|f| &f.result) {
            Some(Ok(p)) => &p.rows,
            _ => &[],
        }
    }

    pub fn total(&self) -> i64 {
        match self.shown.as_ref().map(|f| &f.result) {
            Some(Ok(p)) => p.total,
            _ => 0,
        }
    }

    pub fn error(&self) -> Option<&SearchError> {
        self.shown.as_ref().and_then(|f| f.result.as_ref().err())
    }

    /// Alles vergessen (auch Laufendes: sein Ergebnis wird verworfen).
    pub fn clear(&mut self) {
        self.inflight = None;
        self.last = None;
        self.stale = false;
        self.shown = None;
    }

    fn ensure_worker(&mut self, ctx: &egui::Context, db: &Db) {
        if self.worker.is_some() || self.worker_failed || db.path.as_os_str().is_empty() {
            return;
        }
        let wake = ctx.clone();
        match Worker::spawn(db.path.clone(), move || wake.request_repaint()) {
            Some(w) => self.worker = Some(w),
            None => self.worker_failed = true,
        }
    }

    /// Schickt die Anforderung ab: an den Hintergrund-Faden, sonst gleich im Vordergrund.
    pub fn submit(&mut self, ctx: &egui::Context, db: &Db, req: Request) {
        self.last = Some(req.clone());
        self.stale = false;
        self.ensure_worker(ctx, db);
        if let Some(w) = &self.worker {
            self.next_id += 1;
            let id = self.next_id;
            if w.tx.send(Job { id, req: req.clone() }).is_ok() {
                self.inflight = Some(id);
                return;
            }
            // der Faden ist weg: ab jetzt im Vordergrund
            self.worker = None;
            self.worker_failed = true;
        }
        self.inflight = None;
        let fin = execute(&db.conn, &req);
        self.accept(ctx, db, fin);
    }

    /// Nimmt ein Ergebnis an. Liegt die Seite hinter dem Ende (die Treffer sind weniger geworden), wird die letzte Seite geladen;
    /// gibt dann deren Nummer zurück.
    fn accept(&mut self, ctx: &egui::Context, db: &Db, fin: Finished) -> Option<i64> {
        if let Ok(p) = &fin.result {
            if p.rows.is_empty() && p.total > 0 && fin.req.page > 0 {
                let last = (p.total - 1) / PAGE_SIZE;
                if last < fin.req.page {
                    let mut r = fin.req.clone();
                    r.page = last;
                    self.submit(ctx, db, r);
                    return Some(last);
                }
            }
        }
        self.shown = Some(fin);
        None
    }

    /// Sammelt fertige Ergebnisse ein; veraltete werden verworfen. Gibt eine korrigierte Seite zurück (siehe `accept`).
    pub fn poll(&mut self, ctx: &egui::Context, db: &Db) -> Option<i64> {
        let mut done: Vec<Done> = Vec::new();
        let mut lost = false;
        if let Some(w) = &self.worker {
            loop {
                match w.rx.try_recv() {
                    Ok(d) => done.push(d),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        lost = true;
                        break;
                    }
                }
            }
        }
        let mut fix = None;
        for d in done {
            if self.inflight == Some(d.id) {
                self.inflight = None;
                if let Some(p) = self.accept(ctx, db, d.fin) {
                    fix = Some(p);
                }
            }
        }
        if lost {
            // Der Faden ist gestorben: im Vordergrund nachholen, was noch ausstand.
            self.worker = None;
            self.worker_failed = true;
            if self.inflight.take().is_some() {
                if let Some(req) = self.last.clone() {
                    self.submit(ctx, db, req);
                }
            }
        }
        fix
    }

    /// Pro Bild aufrufen: Ergebnisse einsammeln und, wenn die gewünschte Anforderung von der letzten abweicht (oder veraltet ist),
    /// eine neue abschicken. `None` = nichts gewünscht (alles vergessen). Gibt eine korrigierte Seitennummer zurück, die der
    /// Aufrufer übernehmen muss.
    pub fn drive(&mut self, ctx: &egui::Context, db: &Db, desired: Option<Request>) -> Option<i64> {
        let fix = self.poll(ctx, db);
        if fix.is_some() {
            ctx.request_repaint();
            return fix;
        }
        match desired {
            None => {
                if self.last.is_some() || self.shown.is_some() {
                    self.clear();
                }
            }
            Some(r) => {
                if self.last.as_ref() != Some(&r) || self.stale {
                    self.submit(ctx, db, r);
                }
            }
        }
        None
    }
}

// ------------------------------------------------------------------ Schema

/// Das Schema eines Bereichs samt der Namen, die dort nicht abfragbar sind.
pub struct Loaded {
    pub schema: Schema,
    pub conflicts: Vec<Conflict>,
}

/// Zwischenspeicher für das Schema der Abfrage-Leiste.
#[derive(Default)]
pub struct SchemaCache {
    key: Option<(Scope, Lang)>,
    value: Option<Result<Rc<Loaded>, SearchError>>,
    stale: bool,
}

impl SchemaCache {
    /// Beim nächsten [`SchemaCache::ensure`] neu bauen (Felder, Typen, Status oder Schlüsselwörter haben sich geändert).
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Baut das Schema, wenn Bereich oder Sprache anders sind oder es veraltet ist. `true`, wenn neu gebaut wurde.
    pub fn ensure(&mut self, conn: &Connection, scope: Scope, lang: Lang) -> bool {
        if !self.stale && self.key == Some((scope, lang)) && self.value.is_some() {
            return false;
        }
        self.stale = false;
        self.key = Some((scope, lang));
        self.value = Some(search_schema(conn, scope, lang).map(|s| Rc::new(Loaded { schema: s.schema, conflicts: s.conflicts })));
        true
    }

    pub fn loaded(&self) -> Option<Rc<Loaded>> {
        self.value.as_ref().and_then(|v| v.as_ref().ok()).cloned()
    }

    pub fn error(&self) -> Option<&SearchError> {
        self.value.as_ref().and_then(|v| v.as_ref().err())
    }

    #[cfg(test)]
    pub fn key(&self) -> Option<(Scope, Lang)> {
        self.key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{create_project, NewProject};
    use crate::planning::history::Source;
    use crate::planning::tickets::{create_ticket, NewTicket};
    use crate::planning::types::list_types;

    fn open_db(tag: &str) -> Db {
        let d = std::env::temp_dir().join(format!("paneldeck-test-searchrun-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        crate::db::open(&d, &d.join("backups")).unwrap()
    }

    fn seed(db: &Db, n: usize) -> i64 {
        let mut np = NewProject::new("RR", "Run test", "#336699");
        np.with_defaults = true;
        let p = create_project(&db.conn, &np).unwrap();
        let ty = list_types(&db.conn, p).unwrap().into_iter().find(|t| !t.system).unwrap().id;
        for i in 0..n {
            create_ticket(&db.conn, &NewTicket::new(p, ty, &format!("Ticket {i}")), &Source::ui()).unwrap();
        }
        p
    }

    fn req(p: i64, jql: &str, page: i64) -> Request {
        Request { scope: Scope::Project(p), jql: jql.into(), lang: Lang::En, page }
    }

    /// Frames laufen lassen, bis nichts mehr lädt (mit Zeitgrenze).
    fn settle(run: &mut QueryRun, ctx: &egui::Context, db: &Db, desired: &Request) {
        let t = Instant::now();
        loop {
            let _ = run.drive(ctx, db, Some(desired.clone()));
            if !run.loading() || t.elapsed() > Duration::from_secs(10) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!run.loading(), "die Abfrage kommt zurück");
    }

    #[test]
    fn runs_synchronously_without_a_database_file() {
        let db = open_db("sync");
        let p = seed(&db, 3);
        // dieselbe Datenbank, aber ohne Pfad (wie im Speicher): kein Hintergrund-Faden, die Abfrage läuft sofort
        let nodb = Db { conn: Connection::open(&db.path).unwrap(), path: std::path::PathBuf::new() };
        let ctx = egui::Context::default();
        let mut run = QueryRun::default();
        run.submit(&ctx, &nodb, req(p, "", 0));
        assert!(!run.loading(), "im Vordergrund ist das Ergebnis sofort da");
        assert_eq!(run.total(), 3);
        assert_eq!(run.rows().len(), 3);
    }

    #[test]
    fn background_run_delivers_rows_and_total() {
        let db = open_db("bg");
        let p = seed(&db, 5);
        let ctx = egui::Context::default();
        let mut run = QueryRun::default();
        let r = req(p, "", 0);
        settle(&mut run, &ctx, &db, &r);
        assert_eq!(run.total(), 5);
        assert_eq!(run.rows().len(), 5);
        assert!(run.error().is_none());
        assert_eq!(run.requested(), Some(&r));
        // dieselbe Anforderung läuft nicht noch einmal ...
        let _ = run.drive(&ctx, &db, Some(r.clone()));
        assert!(!run.loading());
        // ... außer sie ist als veraltet markiert
        run.mark_stale();
        let _ = run.drive(&ctx, &db, Some(r.clone()));
        assert!(run.loading() || run.shown.is_some());
        settle(&mut run, &ctx, &db, &r);
    }

    #[test]
    fn stale_results_are_discarded_and_old_rows_stay_while_loading() {
        let db = open_db("stale");
        let p = seed(&db, 4);
        let ctx = egui::Context::default();
        let mut run = QueryRun::default();
        let all = req(p, "", 0);
        settle(&mut run, &ctx, &db, &all);
        assert_eq!(run.rows().len(), 4);
        // zwei Anforderungen hintereinander: nur die zweite zählt
        let first = req(p, "title ~ \"Ticket 0\"", 0);
        let second = req(p, "title ~ \"Ticket 3\"", 0);
        run.submit(&ctx, &db, first);
        assert!(run.loading());
        assert_eq!(run.rows().len(), 4, "die alten Zeilen bleiben beim Nachladen stehen");
        run.submit(&ctx, &db, second.clone());
        settle(&mut run, &ctx, &db, &second);
        assert_eq!(run.rows().len(), 1);
        assert_eq!(run.rows()[0].title, "Ticket 3");
        assert_eq!(run.shown.as_ref().map(|f| f.req.clone()), Some(second));
    }

    #[test]
    fn clear_discards_a_running_query() {
        let db = open_db("clear");
        let p = seed(&db, 2);
        let ctx = egui::Context::default();
        let mut run = QueryRun::default();
        run.submit(&ctx, &db, req(p, "", 0));
        run.clear();
        assert!(!run.loading());
        std::thread::sleep(Duration::from_millis(200));
        let _ = run.poll(&ctx, &db);
        assert!(run.shown.is_none(), "ein Ergebnis nach clear() wird verworfen");
        assert!(run.requested().is_none());
        // drive ohne Wunsch bleibt leer
        assert!(run.drive(&ctx, &db, None).is_none());
        assert!(run.shown.is_none());
    }

    #[test]
    fn errors_are_kept_and_cleared_by_the_next_good_result() {
        let db = open_db("err");
        let p = seed(&db, 2);
        let ctx = egui::Context::default();
        let mut run = QueryRun::default();
        let bad = req(p, "stauts = Done", 0);
        settle(&mut run, &ctx, &db, &bad);
        let e = run.error().expect("Fehler");
        assert!(!e.problems(Lang::En).is_empty(), "Prüffehler tragen Positionen: {e:?}");
        assert_eq!(run.rows().len(), 0);
        let good = req(p, "", 0);
        settle(&mut run, &ctx, &db, &good);
        assert!(run.error().is_none());
        assert_eq!(run.rows().len(), 2);
    }

    #[test]
    fn a_page_beyond_the_end_falls_back_to_the_last_page() {
        let db = open_db("page");
        let p = seed(&db, 130);
        let ctx = egui::Context::default();
        let mut run = QueryRun::default();
        let far = req(p, "", 5);
        // die Seite 5 gibt es nicht: der Lauf lädt die letzte (Seite 1 mit 30 Zeilen) und meldet die Korrektur
        let t = Instant::now();
        let mut fix = None;
        loop {
            // der Aufrufer übernimmt die korrigierte Seite (wie `Results::drive`)
            let wanted = req(p, "", fix.unwrap_or(far.page));
            if let Some(f) = run.drive(&ctx, &db, Some(wanted)) {
                fix = Some(f);
            }
            if fix.is_some() && !run.loading() {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "Zeitüberschreitung");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fix, Some(1));
        let fixed = req(p, "", 1);
        settle(&mut run, &ctx, &db, &fixed);
        assert_eq!(run.total(), 130);
        assert_eq!(run.rows().len(), 30);
    }

    #[test]
    fn schema_cache_rebuilds_for_scope_language_and_invalidation() {
        let db = open_db("schema");
        let p = seed(&db, 0);
        let mut c = SchemaCache::default();
        assert!(c.loaded().is_none());
        assert!(c.ensure(&db.conn, Scope::Project(p), Lang::En));
        assert!(!c.ensure(&db.conn, Scope::Project(p), Lang::En), "unverändert: nicht neu bauen");
        assert!(c.loaded().unwrap().schema.field("status").is_some());
        assert!(c.ensure(&db.conn, Scope::All, Lang::En), "anderer Bereich");
        assert!(c.ensure(&db.conn, Scope::All, Lang::De), "andere Sprache");
        assert!(!c.ensure(&db.conn, Scope::All, Lang::De));
        c.invalidate();
        assert!(c.ensure(&db.conn, Scope::All, Lang::De), "als veraltet markiert");
        assert_eq!(c.key(), Some((Scope::All, Lang::De)));
        // ein unbekanntes Projekt ist ein Fehler, kein Absturz
        assert!(c.ensure(&db.conn, Scope::Project(99_999), Lang::En));
        assert!(c.loaded().is_none() && c.error().is_some());
    }
}
