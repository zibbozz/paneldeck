//! Hintergrund-Läufe für Projekt- und CSV-Import (Dialoge in `backup_ed.rs`): Datei lesen, parsen, prüfen und in die Datenbank
//! schreiben laufen in einem eigenen Faden, damit eine große Datei (bis 128 MB, ~0,2 ms je Ticket) die Oberfläche nicht anhält.
//!
//! * [`Task`] ist ein einzelner Lauf: Ergebnis per Kanal, [`Progress`] (Fortschritt und Abbruch) geteilt mit der Oberfläche. Der Faden
//!   weckt die Oberfläche nach dem Ergebnis (Muster `search_run`/`dash_data`). Ohne Datenbank-Datei (im Speicher, Tests) oder wenn
//!   der Faden nicht startet, läuft der Lauf gleich im Vordergrund.
//! * **Lesen, Parsen, Vorschau** ([`load_project_file`], [`load_csv_file`]) brauchen keine Datenbank.
//! * **Import** läuft auf einer **eigenen schreibenden Verbindung** ([`open_writer`]) in genau **einer Transaktion** (SAVEPOINT in
//!   `import_project_with` / `import_tickets_csv_with`): Fehler und Abbruch nehmen alles zurück, es entsteht nur ein **neues** Projekt
//!   bzw. entstehen nur neue Tickets, Bestehendes wird nie verändert. Die Datenbank läuft im WAL-Modus: Lesezugriffe der Oberfläche
//!   hängen während des Imports nicht (sie sehen den Stand davor); Schreibzugriffe der Oberfläche warten (Busy-Timeout) bis zum Ende.
//!   Die Oberfläche hält dazu den Dialog „Import läuft“ offen (modal), der Änderungen ohnehin verhindert.
//! * Was der Import in der Oberfläche auslöst (neu laden, Projekt wählen, Undo-Historie) geschieht erst in `backup_ed.rs`, **nachdem**
//!   das Ergebnis da ist, im Oberflächen-Faden. Der Import legt nur neue Zeilen an; die Undo-Historie der Tickets kennt sie nicht
//!   (wie bisher).

use crate::calc::csv::{detect_delimiter, read_csv, CsvError, CsvErrorKind};
use crate::db::Db;
use crate::planning::backup_doc::tickets_csv::{import_tickets_csv_with, CsvImportOptions, CsvImportReport};
use crate::planning::backup_doc::{import_project_with, read_limited, BackupError, ImportResult, Preview, ProjectDoc, Progress};
use rusqlite::Connection;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ Ein Lauf

/// Stand eines Laufs bei der Abfrage.
#[derive(Debug, PartialEq)]
pub enum Poll<T> {
    /// Läuft noch.
    Pending,
    /// Fertig.
    Done(T),
    /// Der Faden ist ohne Ergebnis verschwunden (abgebrochen oder abgestürzt).
    Lost,
}

enum Src<T> {
    Thread(Receiver<T>),
    Ready(Option<T>),
}

/// Ein Lauf im Hintergrund (oder, ohne Faden, schon fertig).
pub struct Task<T> {
    src: Src<T>,
    /// Fortschritt und Abbruch (mit dem Faden geteilt).
    pub progress: Arc<Progress>,
    started: Instant,
}

impl<T: Send + 'static> Task<T> {
    /// Startet `f` in einem Faden; `wake` weckt die Oberfläche, sobald das Ergebnis da ist. `None`, wenn der Faden nicht startet.
    pub fn spawn(wake: impl Fn() + Send + 'static, f: impl FnOnce(&Progress) -> T + Send + 'static) -> Option<Task<T>> {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel::<T>();
        let p = progress.clone();
        std::thread::Builder::new()
            .name("backup-import".into())
            .spawn(move || {
                let v = f(&p);
                // Hat die Oberfläche den Lauf inzwischen vergessen, ist niemand mehr da, der das Ergebnis will.
                if tx.send(v).is_ok() {
                    wake();
                }
            })
            .ok()?;
        Some(Task { src: Src::Thread(rx), progress, started: Instant::now() })
    }
}

impl<T> Task<T> {
    /// Läuft `f` gleich hier (im Vordergrund) und hält das Ergebnis bereit.
    pub fn inline(f: impl FnOnce(&Progress) -> T) -> Task<T> {
        let progress = Arc::new(Progress::default());
        let v = f(&progress);
        Task { src: Src::Ready(Some(v)), progress, started: Instant::now() }
    }

    /// Nachsehen, ob das Ergebnis da ist (nimmt es heraus).
    pub fn poll(&mut self) -> Poll<T> {
        match &mut self.src {
            Src::Ready(v) => v.take().map_or(Poll::Lost, Poll::Done),
            Src::Thread(rx) => match rx.try_recv() {
                Ok(v) => Poll::Done(v),
                Err(TryRecvError::Empty) => Poll::Pending,
                Err(TryRecvError::Disconnected) => Poll::Lost,
            },
        }
    }

    /// Seit wann der Lauf besteht.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

// ------------------------------------------------------------------ Lesen und Parsen

/// Warum eine Datei nicht geladen werden konnte.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadFail {
    /// Lesefehler des Dateisystems.
    Io(String),
    /// Zu groß, kein Paneldeck-Dokument, ungültiges JSON/CSV …
    Backup(BackupError),
}

/// Eine gelesene und geprüfte Projektsicherung samt Vorschau (das Dokument wird mit dem Import-Lauf geteilt, nicht kopiert).
pub struct LoadedProject {
    pub doc: Arc<ProjectDoc>,
    pub preview: Preview,
}

/// Eine gelesene Ticket-CSV: der Text und die Anzahl der Zeilen.
pub struct LoadedCsv {
    pub text: Arc<String>,
    pub rows: usize,
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, LoadFail> {
    match read_limited(path) {
        Ok(Ok(b)) => Ok(b),
        Ok(Err(e)) => Err(LoadFail::Backup(e)),
        Err(e) => Err(LoadFail::Io(e.to_string())),
    }
}

/// Liest eine Projektsicherung (Datei lesen, JSON parsen, Format und Version prüfen) und macht die Vorschau.
pub fn load_project_file(path: &Path) -> Result<LoadedProject, LoadFail> {
    let bytes = read_bytes(path)?;
    let text = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    let doc = ProjectDoc::from_json(&text).map_err(LoadFail::Backup)?;
    drop(text);
    let preview = doc.preview();
    Ok(LoadedProject { doc: Arc::new(doc), preview })
}

/// Liest eine Ticket-CSV (Datei lesen, UTF-8 und CSV-Form prüfen) und zählt die Zeilen.
pub fn load_csv_file(path: &Path) -> Result<LoadedCsv, LoadFail> {
    let bytes = read_bytes(path)?;
    let text = String::from_utf8(bytes).map_err(|_| LoadFail::Backup(BackupError::Csv(CsvError { kind: CsvErrorKind::InvalidUtf8, line: 1, column: 1 })))?;
    let table = read_csv(&text, detect_delimiter(&text)).map_err(|e| LoadFail::Backup(BackupError::Csv(e)))?;
    Ok(LoadedCsv { rows: table.rows.len(), text: Arc::new(text) })
}

/// Startet das Lesen einer Projektsicherung im Hintergrund (ohne Faden: gleich).
pub fn start_load_project(wake: impl Fn() + Send + 'static, path: std::path::PathBuf) -> Task<Result<LoadedProject, LoadFail>> {
    let p2 = path.clone();
    Task::spawn(wake, move |_| load_project_file(&p2)).unwrap_or_else(|| Task::inline(|_| load_project_file(&path)))
}

/// Startet das Lesen einer Ticket-CSV im Hintergrund (ohne Faden: gleich).
pub fn start_load_csv(wake: impl Fn() + Send + 'static, path: std::path::PathBuf) -> Task<Result<LoadedCsv, LoadFail>> {
    let p2 = path.clone();
    Task::spawn(wake, move |_| load_csv_file(&p2)).unwrap_or_else(|| Task::inline(|_| load_csv_file(&path)))
}

// ------------------------------------------------------------------ Import

/// Öffnet eine eigene schreibende Verbindung zur Datenbank (Busy-Timeout und Fremdschlüssel wie die Hauptverbindung; der WAL-Modus
/// gehört zur Datei).
pub fn open_writer(path: &Path) -> Result<Connection, String> {
    let c = Connection::open(path).map_err(|e| e.to_string())?;
    c.busy_timeout(Duration::from_secs(10)).map_err(|e| e.to_string())?;
    c.pragma_update(None, "foreign_keys", true).map_err(|e| e.to_string())?;
    Ok(c)
}

/// Startet den Import einer Projektsicherung. Läuft auf einer eigenen Verbindung im Hintergrund; ohne Datenbank-Datei (oder ohne
/// Faden) gleich auf der Verbindung der Oberfläche.
pub fn start_project_import(wake: impl Fn() + Send + 'static, db: &Db, doc: Arc<ProjectDoc>, key: Option<String>) -> Task<Result<ImportResult, BackupError>> {
    if !db.path.as_os_str().is_empty() {
        let (path, d, k) = (db.path.clone(), doc.clone(), key.clone());
        let t = Task::spawn(wake, move |p| match open_writer(&path) {
            Ok(c) => import_project_with(&c, &d, k, p),
            Err(e) => Err(BackupError::Db(e)),
        });
        if let Some(t) = t {
            return t;
        }
    }
    Task::inline(|p| import_project_with(&db.conn, &doc, key, p))
}

/// Startet den Import einer Ticket-CSV in das Projekt (wie [`start_project_import`]).
pub fn start_csv_import(wake: impl Fn() + Send + 'static, db: &Db, project_id: i64, text: Arc<String>) -> Task<Result<CsvImportReport, BackupError>> {
    if !db.path.as_os_str().is_empty() {
        let (path, t2) = (db.path.clone(), text.clone());
        let t = Task::spawn(wake, move |p| match open_writer(&path) {
            Ok(c) => import_tickets_csv_with(&c, project_id, &t2, &CsvImportOptions::default(), p),
            Err(e) => Err(BackupError::Db(e)),
        });
        if let Some(t) = t {
            return t;
        }
    }
    Task::inline(|p| import_tickets_csv_with(&db.conn, project_id, &text, &CsvImportOptions::default(), p))
}
