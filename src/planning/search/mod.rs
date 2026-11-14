//! Ausführung der Abfragesprache auf der Datenbank: Schema-Aufbau, Übersetzung in SQL, Ausführen mit Seiten, Boards und das
//! Befüllen frischer Datenbanken mit Beispieldaten (für die Hilfe-Screenshots). Reine Logik auf `&Connection`, keine Oberfläche.
//!
//! * `schema` – [`schema_for`]/[`search_schema`]: Felder des Bereichs (Projekt oder alle) samt eingebauten Pseudo-Feldern und den
//!   JQL-Schlüsselwörtern; Regeln für gleichnamige Felder stehen in der Moduldoku dort.
//! * `compile` – [`compile`]: [`Query`](crate::query::Query) → parametrisiertes SQL. Die Semantik ist die der Referenz-Auswertung
//!   der Hilfe (`query_help::eval`): `!=`, `!~`, `NOT IN` treffen auch leere Felder, `=`, `~`, `<`, `>`, `IN` nie; leere Werte
//!   stehen beim Sortieren zuletzt; bei Gleichstand gilt die Reihenfolge der Schlüssel.
//! * [`run_query`], [`run_saved_filter`], [`count`] – parsen, prüfen, übersetzen und mit Zeitlimit ausführen.
//! * `board` – [`board_view`] und [`move_card`] (Karte in eine Spalte ziehen, mit Workflow-Regeln).
//! * `seed` – [`seed_demo`]: legt aus einer `query_help::DemoSpec` Projekt, Typen, Felder, Status und Tickets an.
//! * `pending` – Anschluss an `planning::sprints`: Sprints, Versionen und Meilensteine in der Abfrage (Entscheidungen in der Moduldoku dort).
//!
//! # Grenzen und Fehler
//!
//! Abfragetext höchstens [`MAX_QUERY_CHARS`] Zeichen; höchstens `compile::MAX_NODES` Bedingungen und `compile::MAX_DEPTH` Ebenen;
//! Zeitlimit [`Ctx::timeout`] (der Lauf wird über `sqlite3_interrupt` abgebrochen → `SearchError::Timeout`). Alle Fehler sind
//! [`SearchError`] (zweisprachig, Parse- und Prüffehler mit Positionen); die Schicht stürzt nie ab.

// Die Oberfläche (Suchleiste, Listen, Boards) schließt diese Schicht erst später an; bis dahin ist vieles „ungenutzt“.
#![allow(dead_code, unused_imports)]

mod board;
mod compile;
mod error;
mod pending;
mod rows;
mod schema;
mod seed;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_board;
#[cfg(test)]
mod tests_diff;
#[cfg(test)]
mod tests_seed;
#[cfg(test)]
mod tests_sprints;

pub use board::{board_source_ids, board_view, board_view_with, move_card, move_card_with_values, BoardColumnView, BoardPaging, BoardView, MoveError, MoveOutcome, DEFAULT_COLUMN_LIMIT};
pub use compile::{compile, Compiled, MAX_DEPTH, MAX_NODES, MAX_SQL_LEN};
pub use error::{Feature, Problem, SRes, SearchError};
pub use rows::{TicketRow, PRIORITY_FIELD_KEY};
pub use schema::{schema_for, search_schema, Conflict, SearchSchema, CHANGED_BY};
pub use seed::{seed_demo, SeedError, SeededDemo};

use crate::planning::views::filters::get_filter;
use crate::query::{parse, validate};
use crate::state::Lang;
use rusqlite::Connection;
use std::time::Duration;

/// Längster Abfragetext in Zeichen.
pub const MAX_QUERY_CHARS: usize = crate::planning::views::MAX_JQL_CHARS;
/// Standard-Zeitlimit einer Abfrage.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// Höchstzahl der Zeilen, wenn keine Seitengröße angegeben ist.
pub const MAX_ROWS: i64 = 100_000;

/// Wo gesucht wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Nur in diesem Projekt.
    Project(i64),
    /// In allen Projekten (ohne das System-Projekt der Todolist).
    All,
}

/// Umgebung einer Abfrage: das heutige Datum (`currentDate()`), die Terminals (`linkedTerminal()`), die Sprache und das Zeitlimit.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// `JJJJ-MM-TT`
    pub today: String,
    /// Kennungen der Terminals, die `linkedTerminal()` ergibt (üblich: das Terminal, aus dem die Suche gestartet wurde). Leer:
    /// `terminal = linkedTerminal()` trifft nichts.
    pub terminals: Vec<String>,
    pub lang: Lang,
    pub timeout: Duration,
}

impl Ctx {
    pub fn new(today: &str, lang: Lang) -> Ctx {
        Ctx { today: today.to_string(), terminals: Vec::new(), lang, timeout: DEFAULT_TIMEOUT }
    }

    /// Mit dem heutigen Datum der Systemuhr.
    pub fn today_local(lang: Lang) -> Ctx {
        Ctx::new(&chrono::Local::now().format("%Y-%m-%d").to_string(), lang)
    }

    pub fn with_terminals(mut self, terminals: &[&str]) -> Ctx {
        self.terminals = terminals.iter().map(|t| t.to_string()).collect();
        self
    }

    pub fn with_timeout(mut self, t: Duration) -> Ctx {
        self.timeout = t;
        self
    }
}

/// Seitenbegrenzung eines Ergebnisses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// `None` = alle (höchstens [`MAX_ROWS`]).
    pub limit: Option<i64>,
    pub offset: i64,
}

impl Page {
    pub const ALL: Page = Page { limit: None, offset: 0 };

    pub fn new(limit: i64, offset: i64) -> Page {
        Page { limit: Some(limit.max(0)), offset: offset.max(0) }
    }

    pub fn first(limit: i64) -> Page {
        Page::new(limit, 0)
    }
}

/// Eine Seite des Ergebnisses.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchPage {
    pub rows: Vec<TicketRow>,
    /// Alle Treffer der Abfrage, unabhängig von der Seite.
    pub total: i64,
}

/// Führt `f` aus und bricht die Datenbank nach `limit` ab (`SearchError::Timeout`). Der Wächter ist ein kurzlebiger Thread, der den
/// Abbruch über `sqlite3_interrupt` auslöst; ein Abbruch nach dem Ende von `f` bleibt wirkungslos.
pub(crate) fn with_timeout<T>(conn: &Connection, limit: Duration, f: impl FnOnce() -> SRes<T>) -> SRes<T> {
    let handle = conn.get_interrupt_handle();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let watcher = std::thread::Builder::new().name("search-timeout".into()).spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(limit) {
            handle.interrupt();
        }
    });
    let result = f();
    let _ = tx.send(());
    if let Ok(w) = watcher {
        let _ = w.join();
    }
    result
}

/// Prüft die Länge, parst, prüft gegen das Schema und übersetzt. Liefert Schema und SQL (z. B. für Anzeige und Tests).
pub fn prepare(conn: &Connection, ctx: &Ctx, scope: Scope, jql: &str) -> SRes<(SearchSchema, Compiled)> {
    let len = jql.chars().count();
    if len > MAX_QUERY_CHARS {
        return Err(SearchError::TooLong { len, max: MAX_QUERY_CHARS });
    }
    let schema = search_schema(conn, scope, ctx.lang)?;
    let query = parse(jql).map_err(|errors| SearchError::Parse { input: jql.to_string(), errors })?;
    let issues = validate(&query, &schema.schema);
    if !issues.is_empty() {
        return Err(SearchError::Invalid { input: jql.to_string(), issues });
    }
    let compiled = compile(&query, &schema, ctx)?;
    Ok((schema, compiled))
}

/// Seitenbegrenzung als SQL-Anhang und Parameter.
fn paged(base: &str, params: &[rusqlite::types::Value], page: Page) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value as Sql;
    let mut params = params.to_vec();
    let n = params.len();
    params.push(Sql::Integer(page.limit.unwrap_or(MAX_ROWS).clamp(0, MAX_ROWS)));
    params.push(Sql::Integer(page.offset.max(0)));
    (format!("{base} LIMIT ?{} OFFSET ?{}", n + 1, n + 2), params)
}

/// Zählt die Treffer einer fertig übersetzten Abfrage.
pub(super) fn count_compiled(conn: &Connection, c: &Compiled) -> SRes<i64> {
    let (sql, params) = c.count();
    let mut st = conn.prepare(&sql)?;
    let n = st.parameter_count().min(params.len());
    Ok(st.query_row(rusqlite::params_from_iter(params[..n].iter()), |r| r.get(0))?)
}

/// Eine Seite der fertig übersetzten Abfrage samt Gesamtzahl.
pub(super) fn fetch(conn: &Connection, ctx: &Ctx, c: &Compiled, page: Page) -> SRes<SearchPage> {
    with_timeout(conn, ctx.timeout, || {
        let total = count_compiled(conn, c)?;
        let (sql, params) = paged(&c.select(rows::CORE_COLS), &c.params, page);
        let rows = rows::load_rows(conn, ctx, &sql, &params)?;
        Ok(SearchPage { rows, total })
    })
}

/// Führt eine Abfrage im Bereich aus: parsen, prüfen, übersetzen, ausführen (mit Zeitlimit). Ein leerer Text trifft alle Tickets.
pub fn run_query(conn: &Connection, ctx: &Ctx, scope: Scope, jql: &str, page: Page) -> SRes<SearchPage> {
    let (_, compiled) = prepare(conn, ctx, scope, jql)?;
    fetch(conn, ctx, &compiled, page)
}

/// Zählt die Treffer einer Abfrage.
pub fn count(conn: &Connection, ctx: &Ctx, scope: Scope, jql: &str) -> SRes<i64> {
    let (_, compiled) = prepare(conn, ctx, scope, jql)?;
    with_timeout(conn, ctx.timeout, || count_compiled(conn, &compiled))
}

/// Führt einen gespeicherten Filter aus (Bereich: sein Projekt, bei einem globalen Filter alle Projekte).
pub fn run_saved_filter(conn: &Connection, ctx: &Ctx, filter_id: i64, page: Page) -> SRes<SearchPage> {
    let f = get_filter(conn, filter_id)?;
    let scope = match f.project_id {
        Some(p) => Scope::Project(p),
        None => Scope::All,
    };
    run_query(conn, ctx, scope, &f.jql, page)
}
