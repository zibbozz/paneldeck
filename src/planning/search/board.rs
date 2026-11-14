//! Boards: die Ansicht ([`board_view`]) und das Verschieben einer Karte ([`move_card`]).
//!
//! Die Ansicht folgt der [`BoardConfig`](crate::planning::views::boards::BoardConfig): je Spalte die Tickets, deren Status zur Spalte
//! gehört, gefiltert nach Typ-Filter, Quell-Filter (gespeicherte Abfrage; deren `ORDER BY` zählt nicht) und – sobald es Sprints gibt –
//! „nur aktiver Sprint“. Sortiert wird nach dem Rang (Backlog-Reihenfolge); je Spalte kommt nur eine Seite, dazu der Zähler.
//!
//! Beim Verschieben gilt der Workflow: nur erlaubte Übergänge (Matrix, Bedingungen); stehen in der Zielspalte mehrere Status, wird
//! der erste erlaubte genommen. Fehlen Pflichtfelder des Übergangs, kommt [`MoveError::NeedsFields`] mit der Liste (die Oberfläche
//! fragt die Werte ab und ruft [`move_card_with_values`] erneut auf).

use super::compile::{Cx, FROM};
use super::error::{SRes, SearchError};
use super::rows::{self, TicketRow};
use super::{count_compiled, paged, pending, prepare, search_schema, with_timeout, Ctx, Page, Scope};
use crate::planning::history::Source;
use crate::planning::ops::TicketOp;
use crate::planning::statuses::list_statuses;
use crate::planning::ticket_error::{FieldLabel, TicketError};
use crate::planning::tickets::{get_ticket, move_rank};
use crate::planning::transition::{plan_transition, transition_ticket};
use crate::planning::types::get_type;
use crate::planning::views::boards::{column_title, get_board};
use crate::planning::views::filters::get_filter;
use crate::state::Lang;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;

/// Wie viele Karten je Spalte (eine Seite) die Ansicht lädt.
pub const DEFAULT_COLUMN_LIMIT: i64 = 50;

/// Seitenbegrenzung je Spalte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardPaging {
    pub per_column: i64,
    /// Versatz je Spalte (Index = Spalte; fehlende Einträge = 0).
    pub offsets: Vec<i64>,
}

impl Default for BoardPaging {
    fn default() -> Self {
        BoardPaging { per_column: DEFAULT_COLUMN_LIMIT, offsets: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoardColumnView {
    pub index: usize,
    /// Anzeigetitel (der eigene, sonst der Name des ersten Status).
    pub title: String,
    pub status_ids: Vec<i64>,
    /// Die geladene Seite, nach Rang sortiert.
    pub tickets: Vec<TicketRow>,
    /// Alle Tickets der Spalte (mit allen Filtern), unabhängig von der Seite.
    pub total: i64,
    pub offset: i64,
}

impl BoardColumnView {
    /// Gibt es noch Karten hinter der geladenen Seite?
    pub fn has_more(&self) -> bool {
        self.offset + (self.tickets.len() as i64) < self.total
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoardView {
    pub board_id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub columns: Vec<BoardColumnView>,
}

/// Die Ansicht eines Boards mit der Standard-Seitengröße je Spalte.
pub fn board_view(conn: &Connection, board_id: i64, ctx: &Ctx) -> SRes<BoardView> {
    board_view_with(conn, board_id, ctx, &BoardPaging::default())
}

/// Die Ansicht eines Boards mit eigener Seitenbegrenzung je Spalte.
pub fn board_view_with(conn: &Connection, board_id: i64, ctx: &Ctx, paging: &BoardPaging) -> SRes<BoardView> {
    let board = get_board(conn, board_id)?;
    let pid = board.project_id;
    let cfg = &board.config;
    // Quelle: der gespeicherte Filter (seine Sortierung bleibt unbeachtet) oder alle Tickets des Projekts
    let jql = match cfg.source_filter_id {
        Some(f) => get_filter(conn, f)?.jql,
        None => String::new(),
    };
    let (schema, base) = prepare(conn, ctx, Scope::Project(pid), &jql)?;
    let statuses = list_statuses(conn, pid)?;
    with_timeout(conn, ctx.timeout, || {
        let mut columns = Vec::new();
        for (i, col) in cfg.columns.iter().enumerate() {
            let mut cx = Cx::continuing(&schema, ctx, base.params[..base.where_params.min(base.params.len())].to_vec());
            let mut where_sql = format!("{} AND {}", base.where_sql, cx.ids_clause("t.status_id", &col.status_ids));
            if !cfg.type_filter.is_empty() {
                where_sql.push_str(&format!(" AND {}", cx.ids_clause("t.type_id", &cfg.type_filter)));
            }
            if cfg.only_active_sprint {
                // EINHÄNGEPUNKT Sprints (siehe `pending.rs`)
                where_sql.push_str(&format!(" AND {}", pending::active_sprint_condition(&mut cx, pid)?));
            }
            let count_sql = format!("SELECT COUNT(*) {FROM} WHERE {where_sql}");
            let mut st = conn.prepare(&count_sql)?;
            let n = st.parameter_count().min(cx.params.len());
            let total: i64 = st.query_row(params_from_iter(cx.params[..n].iter()), |r| r.get(0))?;
            drop(st);
            let offset = paging.offsets.get(i).copied().unwrap_or(0).max(0);
            let select = format!("SELECT {} {FROM} WHERE {where_sql} ORDER BY t.rank, t.id", rows::CORE_COLS);
            let (sql, params) = paged(&select, &cx.params, Page::new(paging.per_column.max(1), offset));
            let tickets = rows::load_rows(conn, ctx, &sql, &params)?;
            columns.push(BoardColumnView { index: i, title: column_title(col, &statuses, ctx.lang), status_ids: col.status_ids.clone(), tickets, total, offset });
        }
        Ok(BoardView { board_id, project_id: pid, name: board.name.clone(), description: board.description.clone(), columns })
    })
}

// ------------------------------------------------------------------ Karte verschieben

#[derive(Debug, Clone, PartialEq)]
pub enum MoveError {
    /// Ein Fehler der Ticket-Schicht (Übergang nicht erlaubt, Bedingung sperrt …).
    Ticket(TicketError),
    /// Der Übergang braucht noch Pflichtfelder (für den Status `status_id`): Werte abfragen und mit
    /// [`move_card_with_values`] erneut aufrufen.
    NeedsFields { status_id: i64, fields: Vec<FieldLabel> },
    /// Board oder Ticket gibt es nicht.
    NotFound,
    /// Die Spalte gibt es nicht.
    ColumnInvalid,
    /// Das Ticket gehört nicht auf dieses Board (anderes Projekt, vom Typ-Filter ausgeschlossen, Typ ohne Workflow).
    NotOnBoard,
    Db(String),
}

impl From<TicketError> for MoveError {
    fn from(e: TicketError) -> Self {
        if e.is_not_found() { MoveError::NotFound } else { MoveError::Ticket(e) }
    }
}

impl From<rusqlite::Error> for MoveError {
    fn from(e: rusqlite::Error) -> Self {
        MoveError::Db(e.to_string())
    }
}

impl From<crate::planning::views::ViewError> for MoveError {
    fn from(e: crate::planning::views::ViewError) -> Self {
        match e {
            crate::planning::views::ViewError::Plan(crate::planning::PlanError::NotFound) => MoveError::NotFound,
            other => MoveError::Db(other.message(Lang::En)),
        }
    }
}

impl MoveError {
    pub fn message(&self, lang: Lang) -> String {
        match self {
            MoveError::Ticket(e) => e.message(lang),
            MoveError::NeedsFields { fields, .. } => {
                TicketError::TransitionNeedsFields(fields.clone()).message(lang)
            }
            MoveError::NotFound => lang.t("Das Board oder das Ticket existiert nicht mehr.", "The board or the ticket no longer exists.").to_string(),
            MoveError::ColumnInvalid => lang.t("Diese Spalte gibt es nicht.", "This column does not exist.").to_string(),
            MoveError::NotOnBoard => lang.t("Dieses Ticket gehört nicht auf dieses Board.", "This ticket does not belong on this board.").to_string(),
            MoveError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
        }
    }
}

/// Was beim Verschieben geschehen ist.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveOutcome {
    /// Der Status des Tickets nach dem Verschieben.
    pub status_id: Option<i64>,
    pub status_changed: bool,
    /// Die Reihenfolge hat sich geändert.
    pub reordered: bool,
    /// Die Änderung für Rückgängig/Wiederholen (`None` = es hat sich nichts geändert).
    pub op: Option<TicketOp>,
}

/// Verschiebt die Karte in die Spalte `to_column` des Boards und setzt sie vor `before` (`None` = ans Ende der Spalte).
/// Siehe Moduldoku für die Regeln.
pub fn move_card(conn: &Connection, ticket: i64, board_id: i64, to_column: usize, before: Option<i64>, source: &Source) -> Result<MoveOutcome, MoveError> {
    move_card_with_values(conn, ticket, board_id, to_column, before, &[], source)
}

/// Wie [`move_card`], mit Feldwerten, die in derselben Operation gesetzt werden (Pflichtfelder des Übergangs).
pub fn move_card_with_values(conn: &Connection, ticket: i64, board_id: i64, to_column: usize, before: Option<i64>, values: &[(i64, Value)], source: &Source) -> Result<MoveOutcome, MoveError> {
    conn.execute_batch("SAVEPOINT move_card")?;
    let r = move_inner(conn, ticket, board_id, to_column, before, values, source);
    match &r {
        Ok(_) => conn.execute_batch("RELEASE move_card")?,
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK TO move_card; RELEASE move_card");
        }
    }
    r
}

fn move_inner(conn: &Connection, ticket: i64, board_id: i64, to_column: usize, before: Option<i64>, values: &[(i64, Value)], source: &Source) -> Result<MoveOutcome, MoveError> {
    let board = get_board(conn, board_id)?;
    let t = get_ticket(conn, ticket)?;
    if t.project_id != board.project_id {
        return Err(MoveError::NotOnBoard);
    }
    let cfg = &board.config;
    if !cfg.type_filter.is_empty() && !cfg.type_filter.contains(&t.type_id) {
        return Err(MoveError::NotOnBoard);
    }
    let col = cfg.columns.get(to_column).ok_or(MoveError::ColumnInvalid)?;
    if !get_type(conn, t.type_id).map_err(|e| TicketError::Plan(e))?.workflow {
        return Err(MoveError::NotOnBoard);
    }
    let mut ops: Vec<TicketOp> = Vec::new();
    let mut status_changed = false;
    // Status: bleibt, wenn er schon zur Spalte gehört; sonst der erste erlaubte der Spalte
    if !t.status_id.is_some_and(|s| col.status_ids.contains(&s)) {
        let mut needs: Option<(i64, Vec<FieldLabel>)> = None;
        let mut first_err: Option<TicketError> = None;
        let mut chosen: Option<i64> = None;
        for &to in &col.status_ids {
            match plan_transition(conn, &t, to, values) {
                Ok(_) => {
                    chosen = Some(to);
                    break;
                }
                Err(TicketError::TransitionNeedsFields(l)) => {
                    needs.get_or_insert((to, l));
                }
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        let to = match (chosen, needs, first_err) {
            (Some(to), _, _) => to,
            (None, Some((status_id, fields)), _) => return Err(MoveError::NeedsFields { status_id, fields }),
            (None, None, Some(e)) => return Err(e.into()),
            (None, None, None) => return Err(MoveError::ColumnInvalid),
        };
        if let Some(op) = transition_ticket(conn, ticket, to, values, source)? {
            ops.push(op);
            status_changed = true;
        }
    }
    let reordered = match move_rank(conn, ticket, before)? {
        Some(op) => {
            ops.push(op);
            true
        }
        None => false,
    };
    let status_id = get_ticket(conn, ticket)?.status_id;
    Ok(MoveOutcome { status_id, status_changed, reordered, op: TicketOp::combine(ops) })
}
