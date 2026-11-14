//! Kanban-Boards je Projekt: Name, Beschreibung, Reihenfolge und eine JSON-Konfiguration ([`BoardConfig`]).
//!
//! * **Spalten**: frei angeordnet, jede mit Titel und **mehreren Status** (Reihenfolge der Status in der Spalte ist frei).
//!   Ein leerer Titel heißt „Anzeigename des ersten Status“ ([`column_title`]) – so übersetzen sich Standard-Spalten live.
//!   Kein Status darf in zwei Spalten stehen; Status, die in keiner Spalte stehen, erscheinen nicht auf dem Board.
//! * **Typ-Filter** (leer = alle Typen), **Quelle** (optional ein gespeicherter Filter, global oder aus dem Projekt),
//!   **nur aktiver Sprint** und die **Kartenfelder** (Schlüssel und Typ-Icon sind eigene Schalter, dazu Feld-Kennungen).
//! * Prüfung beim Anlegen/Ändern ([`check_config`]): Status/Typen/Felder gehören zum Projekt, der Filter ist verfügbar,
//!   nichts doppelt. Doppelte Typen/Felder werden stillschweigend zusammengefasst.
//! * **Löschen in der Konfiguration**: Status/Typen/Felder/Filter können verschwinden, ohne dass ein Board kaputt geht – die
//!   Löschfunktionen rufen [`prune_board_refs`] auf (entfernt verwaiste Verweise; Spalten, die dadurch leer werden, fallen weg;
//!   ein leer gewordener Typ-Filter heißt dann wieder „alle Typen“). Das Löschen eines gespeicherten Filters tut das selbst.
//! * [`default_board_for`] baut das Standard-Board aus den Status, die in einem Workflow vorkommen.
//!
//! Boards eines System-Projekts (Todolist) gibt es nicht.

use super::{atomic, clean_name, clean_text, copy_name, ensure_name_free, names_in, Res, ViewError, MAX_DESCRIPTION_CHARS, MAX_NAME_CHARS};
use crate::planning::common;
use crate::planning::projects::editable;
use crate::planning::statuses::Status;
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Aktuelle Format-Version der Board-Konfiguration.
pub const CONFIG_VERSION: u32 = 1;
/// Höchste Spaltenzahl.
pub const MAX_COLUMNS: usize = 30;

fn yes() -> bool {
    true
}

fn config_version() -> u32 {
    CONFIG_VERSION
}

/// Eine Spalte: Titel (leer = Name des ersten Status) und die zugeordneten Status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BoardColumn {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardConfig {
    #[serde(default = "config_version")]
    pub version: u32,
    #[serde(default)]
    pub columns: Vec<BoardColumn>,
    /// Nur diese Typen zeigen; leer = alle.
    #[serde(default)]
    pub type_filter: Vec<i64>,
    /// Gespeicherter Filter als Quelle der Tickets (global oder aus dem Projekt).
    #[serde(default)]
    pub source_filter_id: Option<i64>,
    #[serde(default)]
    pub only_active_sprint: bool,
    /// Ticket-Schlüssel (z. B. PAN-12) auf der Karte.
    #[serde(default = "yes")]
    pub show_key: bool,
    /// Typ-Icon auf der Karte.
    #[serde(default = "yes")]
    pub show_type_icon: bool,
    /// Weitere Felder auf der Karte (Feld-Kennungen, Reihenfolge = Anzeige-Reihenfolge).
    #[serde(default)]
    pub card_fields: Vec<i64>,
}

impl Default for BoardConfig {
    fn default() -> Self {
        BoardConfig { version: CONFIG_VERSION, columns: Vec::new(), type_filter: Vec::new(), source_filter_id: None, only_active_sprint: false, show_key: true, show_type_icon: true, card_fields: Vec::new() }
    }
}

impl BoardConfig {
    /// Alle Status, die auf dem Board in einer Spalte stehen.
    pub fn status_ids(&self) -> Vec<i64> {
        self.columns.iter().flat_map(|c| c.status_ids.iter().copied()).collect()
    }

    /// Index der Spalte, in der der Status steht.
    pub fn column_of(&self, status_id: i64) -> Option<usize> {
        self.columns.iter().position(|c| c.status_ids.contains(&status_id))
    }

    fn to_json(&self) -> String {
        let mut c = self.clone();
        c.version = CONFIG_VERSION;
        serde_json::to_string(&c).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Liest die Konfiguration; eine unlesbare Zeile ergibt die leere Standard-Konfiguration (das Board bleibt benutzbar).
pub fn parse_config(text: &str) -> BoardConfig {
    serde_json::from_str(text).unwrap_or_default()
}

/// Anzeigetitel der Spalte: der eigene Titel, sonst der Name des ersten Status (aus `statuses`), sonst leer.
pub fn column_title(col: &BoardColumn, statuses: &[Status], lang: Lang) -> String {
    if !col.title.trim().is_empty() {
        return col.title.clone();
    }
    col.status_ids.first().and_then(|id| statuses.iter().find(|s| s.id == *id)).map(|s| s.display(lang)).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub position: i64,
    pub config: BoardConfig,
}

#[derive(Debug, Clone)]
pub struct NewBoard {
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub config: BoardConfig,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateBoard {
    pub name: Option<String>,
    pub description: Option<String>,
    pub config: Option<BoardConfig>,
}

const COLS: &str = "id, project_id, name, description, position, config";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Board> {
    Ok(Board { id: r.get(0)?, project_id: r.get(1)?, name: r.get(2)?, description: r.get(3)?, position: r.get(4)?, config: parse_config(&r.get::<_, String>(5)?) })
}

pub fn get_board(conn: &Connection, id: i64) -> Res<Board> {
    conn.query_row(&format!("SELECT {COLS} FROM boards WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(ViewError::not_found)
}

pub fn list_boards(conn: &Connection, project_id: i64) -> Res<Vec<Board>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM boards WHERE project_id = ?1 ORDER BY position, id"))?;
    Ok(st.query_map([project_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

// ------------------------------------------------------------------ Prüfen

fn id_set(conn: &Connection, sql: &str, project_id: i64) -> Res<HashSet<i64>> {
    let mut st = conn.prepare(sql)?;
    Ok(st.query_map([project_id], |r| r.get::<_, i64>(0))?.collect::<Result<HashSet<_>, _>>()?)
}

fn project_statuses(conn: &Connection, project_id: i64) -> Res<HashSet<i64>> {
    id_set(conn, "SELECT id FROM statuses WHERE project_id = ?1", project_id)
}

fn project_types(conn: &Connection, project_id: i64) -> Res<HashSet<i64>> {
    id_set(conn, "SELECT id FROM ticket_types WHERE project_id = ?1", project_id)
}

fn project_fields(conn: &Connection, project_id: i64) -> Res<HashSet<i64>> {
    id_set(conn, "SELECT f.id FROM fields f JOIN ticket_types t ON t.id = f.type_id WHERE t.project_id = ?1", project_id)
}

/// Ist der Filter für das Projekt verfügbar (global oder aus dem Projekt)?
fn filter_available(conn: &Connection, filter_id: i64, project_id: i64) -> Res<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM saved_filters WHERE id = ?1 AND (project_id IS NULL OR project_id = ?2)", [filter_id, project_id], |r| r.get(0))?;
    Ok(n > 0)
}

fn dedup(v: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    for x in v {
        if !out.contains(x) {
            out.push(*x);
        }
    }
    out
}

/// Prüft die Konfiguration gegen das Projekt und liefert die bereinigte Fassung (Titel bereinigt, Typen/Felder ohne Doppelte,
/// Version gesetzt). Fehler: zu viele Spalten, Spalte ohne Status, fremder oder doppelter Status, fremder Typ, fremdes Feld,
/// nicht verfügbarer Filter.
pub fn check_config(conn: &Connection, project_id: i64, cfg: &BoardConfig) -> Res<BoardConfig> {
    if cfg.columns.len() > MAX_COLUMNS {
        return Err(ViewError::TooManyColumns { max: MAX_COLUMNS });
    }
    let statuses = project_statuses(conn, project_id)?;
    let mut seen = HashSet::new();
    let mut columns = Vec::new();
    for c in &cfg.columns {
        if c.status_ids.is_empty() {
            return Err(ViewError::ColumnEmpty);
        }
        let title = clean_text(&c.title, MAX_NAME_CHARS)?;
        for s in &c.status_ids {
            if !statuses.contains(s) {
                return Err(ViewError::StatusNotInProject);
            }
            if !seen.insert(*s) {
                return Err(ViewError::StatusTwice);
            }
        }
        columns.push(BoardColumn { title, status_ids: c.status_ids.clone() });
    }
    let types = project_types(conn, project_id)?;
    let type_filter = dedup(&cfg.type_filter);
    if type_filter.iter().any(|t| !types.contains(t)) {
        return Err(ViewError::TypeNotInProject);
    }
    let fields = project_fields(conn, project_id)?;
    let card_fields = dedup(&cfg.card_fields);
    if card_fields.iter().any(|f| !fields.contains(f)) {
        return Err(ViewError::FieldNotInProject);
    }
    if let Some(f) = cfg.source_filter_id {
        if !filter_available(conn, f, project_id)? {
            return Err(ViewError::FilterNotAvailable);
        }
    }
    Ok(BoardConfig { version: CONFIG_VERSION, columns, type_filter, source_filter_id: cfg.source_filter_id, only_active_sprint: cfg.only_active_sprint, show_key: cfg.show_key, show_type_icon: cfg.show_type_icon, card_fields })
}

// ------------------------------------------------------------------ Anlegen, ändern, löschen

pub fn create_board(conn: &Connection, n: &NewBoard) -> Res<i64> {
    editable(conn, n.project_id)?;
    let name = clean_name(&n.name)?;
    let description = clean_text(&n.description, MAX_DESCRIPTION_CHARS)?;
    atomic(conn, || {
        let cfg = check_config(conn, n.project_id, &n.config)?;
        ensure_name_free(conn, "boards", "project_id = ?1", &[n.project_id], &name, None)?;
        let pos = common::next_position(conn, "boards", "project_id = ?1", &[n.project_id])?;
        conn.execute("INSERT INTO boards (project_id, name, description, position, config) VALUES (?1, ?2, ?3, ?4, ?5)", params![n.project_id, name, description, pos, cfg.to_json()])?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_board(conn: &Connection, id: i64, u: &UpdateBoard) -> Res<()> {
    let cur = get_board(conn, id)?;
    let name = match &u.name {
        Some(s) => clean_name(s)?,
        None => cur.name.clone(),
    };
    let description = match &u.description {
        Some(s) => clean_text(s, MAX_DESCRIPTION_CHARS)?,
        None => cur.description.clone(),
    };
    atomic(conn, || {
        let cfg = match &u.config {
            Some(c) => check_config(conn, cur.project_id, c)?,
            None => cur.config.clone(),
        };
        ensure_name_free(conn, "boards", "project_id = ?1", &[cur.project_id], &name, Some(id))?;
        conn.execute("UPDATE boards SET name = ?1, description = ?2, config = ?3 WHERE id = ?4", params![name, description, cfg.to_json(), id])?;
        Ok(())
    })
}

/// Neue Reihenfolge der Boards des Projekts; `ids` muss genau die vorhandenen Boards enthalten.
pub fn reorder_boards(conn: &Connection, project_id: i64, ids: &[i64]) -> Res<()> {
    Ok(common::reorder(conn, "boards", "project_id = ?1", &[project_id], ids)?)
}

pub fn delete_board(conn: &Connection, id: i64) -> Res<()> {
    get_board(conn, id)?;
    conn.execute("DELETE FROM boards WHERE id = ?1", [id])?;
    Ok(())
}

/// Dupliziert ein Board (Name „Copy of X“ / „Kopie von X“, ans Ende der Reihenfolge, gleiche Konfiguration).
pub fn duplicate_board(conn: &Connection, id: i64, lang: Lang) -> Res<i64> {
    let b = get_board(conn, id)?;
    atomic(conn, || {
        let taken = names_in(conn, "boards", "project_id = ?1", &[b.project_id])?;
        let name = copy_name(lang, &b.name, &taken);
        let pos = common::next_position(conn, "boards", "project_id = ?1", &[b.project_id])?;
        conn.execute("INSERT INTO boards (project_id, name, description, position, config) VALUES (?1, ?2, ?3, ?4, ?5)", params![b.project_id, name, b.description, pos, b.config.to_json()])?;
        Ok(conn.last_insert_rowid())
    })
}

// ------------------------------------------------------------------ Standard-Board

/// Schlüssel der Felder, die das Standard-Board auf der Karte zeigt (in dieser Reihenfolge): Priorität, Story Points, Epic.
pub const DEFAULT_CARD_FIELD_KEYS: [&str; 3] = ["priority", "story_points", "epic"];

/// Das Standard-Board des Projekts (nicht gespeichert): eine Spalte je Status, der im Workflow eines Typs mit Workflow
/// vorkommt (in der Reihenfolge des Status-Pools, Titel leer = Name des Status), alle Typen, alle Tickets, Schlüssel und
/// Typ-Icon sowie – soweit vorhanden – Priorität, Story Points und Epic auf der Karte.
pub fn default_board_for(conn: &Connection, project_id: i64, lang: Lang) -> Res<NewBoard> {
    editable(conn, project_id)?;
    let status_ids: Vec<i64> = {
        let mut st = conn.prepare(
            "SELECT DISTINCT s.id FROM statuses s
             JOIN type_statuses ts ON ts.status_id = s.id
             JOIN ticket_types t ON t.id = ts.type_id
             WHERE s.project_id = ?1 AND t.workflow = 1
             ORDER BY s.position, s.id",
        )?;
        st.query_map([project_id], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let mut fields: Vec<(usize, i64)> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT f.id, f.key FROM fields f JOIN ticket_types t ON t.id = f.type_id
             WHERE t.project_id = ?1 AND t.workflow = 1 AND f.key IS NOT NULL
             ORDER BY t.position, t.id, f.position, f.id",
        )?;
        let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, key) = row?;
            if let Some(rank) = DEFAULT_CARD_FIELD_KEYS.iter().position(|k| *k == key) {
                fields.push((rank, id));
            }
        }
    }
    fields.sort_by_key(|(rank, _)| *rank); // stabil: je Rang bleibt die Reihenfolge der Typen
    let config = BoardConfig {
        columns: status_ids.into_iter().map(|id| BoardColumn { title: String::new(), status_ids: vec![id] }).collect(),
        card_fields: fields.into_iter().map(|(_, id)| id).collect(),
        ..BoardConfig::default()
    };
    Ok(NewBoard {
        project_id,
        name: lang.t("Board", "Board").to_string(),
        description: lang.t("Alle Status der Workflows des Projekts.", "All statuses of the project's workflows.").to_string(),
        config,
    })
}

/// Legt das Standard-Board an, wenn das Projekt noch kein Board hat; sonst das erste Board. Liefert die Board-Kennung.
pub fn ensure_default_board(conn: &Connection, project_id: i64, lang: Lang) -> Res<i64> {
    atomic(conn, || {
        if let Some(first) = list_boards(conn, project_id)?.first() {
            return Ok(first.id);
        }
        create_board(conn, &default_board_for(conn, project_id, lang)?)
    })
}

// ------------------------------------------------------------------ Verwaiste Verweise entfernen

fn pruned(conn: &Connection, project_id: i64, cfg: &BoardConfig) -> Res<BoardConfig> {
    let statuses = project_statuses(conn, project_id)?;
    let types = project_types(conn, project_id)?;
    let fields = project_fields(conn, project_id)?;
    let mut seen = HashSet::new();
    let columns = cfg
        .columns
        .iter()
        .filter_map(|c| {
            let ids: Vec<i64> = c.status_ids.iter().copied().filter(|s| statuses.contains(s) && seen.insert(*s)).collect();
            if ids.is_empty() { None } else { Some(BoardColumn { title: c.title.clone(), status_ids: ids }) }
        })
        .collect();
    let source = match cfg.source_filter_id {
        Some(f) if filter_available(conn, f, project_id)? => Some(f),
        _ => None,
    };
    Ok(BoardConfig {
        version: CONFIG_VERSION,
        columns,
        type_filter: dedup(&cfg.type_filter).into_iter().filter(|t| types.contains(t)).collect(),
        source_filter_id: source,
        only_active_sprint: cfg.only_active_sprint,
        show_key: cfg.show_key,
        show_type_icon: cfg.show_type_icon,
        card_fields: dedup(&cfg.card_fields).into_iter().filter(|f| fields.contains(f)).collect(),
    })
}

/// Entfernt aus den Boards des Projekts alles, was es nicht mehr gibt: gelöschte Status, Typen, Felder und Filter (auch
/// Filter eines anderen Projekts). Spalten ohne Status fallen weg. Liefert die Zahl der geänderten Boards.
/// Wird von den Löschfunktionen für Status, Typen, Felder und Filter (und beim Zusammenführen) aufgerufen.
pub fn prune_board_refs(conn: &Connection, project_id: i64) -> Res<usize> {
    atomic(conn, || {
        let mut changed = 0;
        for b in list_boards(conn, project_id)? {
            let p = pruned(conn, project_id, &b.config)?;
            if p != (BoardConfig { version: CONFIG_VERSION, ..b.config.clone() }) {
                conn.execute("UPDATE boards SET config = ?1 WHERE id = ?2", params![p.to_json(), b.id])?;
                changed += 1;
            }
        }
        Ok(changed)
    })
}

/// Aufräumen für die Löschfunktionen von Status, Typ und Feld (`PlanError` statt `ViewError`): Ohne das behielten die Boards die
/// Kennungen des Gelöschten (leere Spalte, Typ-Filter ohne Treffer, Board lässt sich nicht mehr speichern).
pub(crate) fn prune_after_delete(conn: &Connection, project_id: i64) -> Result<(), crate::planning::PlanError> {
    match prune_board_refs(conn, project_id) {
        Ok(_) => Ok(()),
        Err(ViewError::Plan(e)) => Err(e),
        Err(other) => Err(crate::planning::PlanError::Db(other.message(crate::state::lang()))),
    }
}

/// Wie [`prune_board_refs`] für alle Projekte (z. B. nach dem Löschen eines globalen Filters).
pub fn prune_all_board_refs(conn: &Connection) -> Res<usize> {
    atomic(conn, || {
        let ids: Vec<i64> = {
            let mut st = conn.prepare("SELECT DISTINCT project_id FROM boards")?;
            st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        let mut n = 0;
        for p in ids {
            n += prune_board_refs(conn, p)?;
        }
        Ok(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{create_field, delete_field, list_fields, FieldKind, NewField};
    use crate::planning::projects::{delete_project, get_project};
    use crate::planning::statuses::{create_status, list_statuses, NewStatus, StatusCategory};
    use crate::planning::types::{list_types, TicketType};
    use crate::planning::views::filters::{create_filter, delete_filter, NewFilter};
    use crate::planning::views::testutil::{db, project};
    use crate::planning::PlanError;

    fn new_board(project_id: i64, name: &str, config: BoardConfig) -> NewBoard {
        NewBoard { project_id, name: name.into(), description: String::new(), config }
    }

    fn statuses(c: &Connection, p: i64) -> Vec<i64> {
        list_statuses(c, p).unwrap().iter().map(|s| s.id).collect()
    }

    fn type_by_key(c: &Connection, p: i64, key: &str) -> TicketType {
        list_types(c, p).unwrap().into_iter().find(|t| t.key.as_deref() == Some(key)).unwrap()
    }

    fn cfg_cols(cols: &[&[i64]]) -> BoardConfig {
        BoardConfig { columns: cols.iter().map(|ids| BoardColumn { title: String::new(), status_ids: ids.to_vec() }).collect(), ..BoardConfig::default() }
    }

    #[test]
    fn migration_creates_the_tables() {
        let c = db();
        for t in ["boards", "saved_filters", "jql_keywords", "dashboards"] {
            let n: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [t], |r| r.get(0)).unwrap();
            assert_eq!(n, 1, "{t}");
        }
        assert!(crate::db::latest_version() >= 4);
    }

    #[test]
    fn config_serializes_with_defaults() {
        let d = BoardConfig::default();
        assert_eq!(d.version, CONFIG_VERSION);
        assert!(d.show_key && d.show_type_icon && !d.only_active_sprint && d.columns.is_empty());
        let parsed = parse_config("{}");
        assert_eq!(parsed, d);
        // unlesbar -> Standard
        assert_eq!(parse_config("kaputt"), d);
        // unbekannte Felder (neuere Version) stören nicht
        let p = parse_config(r#"{"version":9,"columns":[{"title":"A","status_ids":[1,2]}],"zukunft":true}"#);
        assert_eq!(p.columns[0].status_ids, vec![1, 2]);
        assert!(p.show_key);
    }

    #[test]
    fn creates_reads_and_lists_boards() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = statuses(&c, p);
        let cfg = BoardConfig { columns: vec![BoardColumn { title: " Start ".into(), status_ids: vec![s[0], s[1]] }, BoardColumn { title: String::new(), status_ids: vec![s[2]] }], ..BoardConfig::default() };
        let id = create_board(&c, &NewBoard { project_id: p, name: "  Mein   Board ".into(), description: " Text ".into(), config: cfg }).unwrap();
        let b = get_board(&c, id).unwrap();
        assert_eq!((b.name.as_str(), b.description.as_str(), b.position), ("Mein Board", "Text", 0));
        assert_eq!(b.config.columns[0].title, "Start");
        assert_eq!(b.config.columns[0].status_ids, vec![s[0], s[1]]);
        let id2 = create_board(&c, &new_board(p, "Zweites", BoardConfig::default())).unwrap();
        let all = list_boards(&c, p).unwrap();
        assert_eq!(all.iter().map(|b| b.id).collect::<Vec<_>>(), vec![id, id2]);
        assert_eq!(all[1].position, 1);
        assert_eq!(get_board(&c, 9999), Err(ViewError::not_found()));
    }

    #[test]
    fn several_statuses_per_column_in_free_order() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = statuses(&c, p);
        let cfg = cfg_cols(&[&[s[2], s[0]], &[s[1]]]);
        let id = create_board(&c, &new_board(p, "B", cfg)).unwrap();
        let b = get_board(&c, id).unwrap();
        assert_eq!(b.config.columns[0].status_ids, vec![s[2], s[0]]);
        assert_eq!(b.config.column_of(s[0]), Some(0));
        assert_eq!(b.config.column_of(s[1]), Some(1));
        assert_eq!(b.config.status_ids(), vec![s[2], s[0], s[1]]);
        // freie Spaltenreihenfolge: Spalte 1 nach vorn
        let mut cfg2 = b.config.clone();
        cfg2.columns.reverse();
        update_board(&c, id, &UpdateBoard { config: Some(cfg2), ..Default::default() }).unwrap();
        assert_eq!(get_board(&c, id).unwrap().config.columns[0].status_ids, vec![s[1]]);
    }

    #[test]
    fn validates_status_rules() {
        let c = db();
        let (p, q) = (project(&c, "AA", true), project(&c, "BB", true));
        let (s, sq) = (statuses(&c, p), statuses(&c, q));
        let make = |cfg| create_board(&c, &new_board(p, "X", cfg));
        assert_eq!(make(cfg_cols(&[&[s[0]], &[s[0]]])), Err(ViewError::StatusTwice));
        assert_eq!(make(cfg_cols(&[&[s[0], s[0]]])), Err(ViewError::StatusTwice));
        assert_eq!(make(cfg_cols(&[&[sq[0]]])), Err(ViewError::StatusNotInProject));
        assert_eq!(make(cfg_cols(&[&[9999]])), Err(ViewError::StatusNotInProject));
        assert_eq!(make(cfg_cols(&[&[]])), Err(ViewError::ColumnEmpty));
        let too_many = BoardConfig { columns: (0..=MAX_COLUMNS).map(|_| BoardColumn { title: String::new(), status_ids: vec![s[0]] }).collect(), ..Default::default() };
        assert_eq!(make(too_many), Err(ViewError::TooManyColumns { max: MAX_COLUMNS }));
        let long = BoardConfig { columns: vec![BoardColumn { title: "x".repeat(81), status_ids: vec![s[0]] }], ..Default::default() };
        assert_eq!(make(long), Err(ViewError::TextTooLong { max: 80 }));
        // nichts angelegt
        assert!(list_boards(&c, p).unwrap().is_empty());
    }

    #[test]
    fn validates_type_filter_fields_and_source() {
        let c = db();
        let (p, q) = (project(&c, "AA", true), project(&c, "BB", true));
        let us = type_by_key(&c, p, "userstory").id;
        let us_q = type_by_key(&c, q, "userstory").id;
        let prio = list_fields(&c, us).unwrap().into_iter().find(|f| f.key.as_deref() == Some("priority")).unwrap().id;
        let prio_q = list_fields(&c, us_q).unwrap().into_iter().find(|f| f.key.as_deref() == Some("priority")).unwrap().id;
        let mk = |cfg: BoardConfig| create_board(&c, &new_board(p, "X", cfg));
        assert_eq!(mk(BoardConfig { type_filter: vec![us_q], ..Default::default() }), Err(ViewError::TypeNotInProject));
        assert_eq!(mk(BoardConfig { card_fields: vec![prio_q], ..Default::default() }), Err(ViewError::FieldNotInProject));
        assert_eq!(mk(BoardConfig { source_filter_id: Some(1), ..Default::default() }), Err(ViewError::FilterNotAvailable));
        // fremder Projekt-Filter ist nicht verfügbar, globaler und eigener schon
        let foreign = create_filter(&c, &NewFilter { project_id: Some(q), name: "f".into(), jql: String::new() }).unwrap();
        let own = create_filter(&c, &NewFilter { project_id: Some(p), name: "own".into(), jql: String::new() }).unwrap();
        let global = create_filter(&c, &NewFilter { project_id: None, name: "g".into(), jql: String::new() }).unwrap();
        assert_eq!(mk(BoardConfig { source_filter_id: Some(foreign), ..Default::default() }), Err(ViewError::FilterNotAvailable));
        let ok = mk(BoardConfig { type_filter: vec![us, us], card_fields: vec![prio, prio], source_filter_id: Some(own), ..Default::default() }).unwrap();
        let b = get_board(&c, ok).unwrap();
        assert_eq!(b.config.type_filter, vec![us], "Doppelte werden zusammengefasst");
        assert_eq!(b.config.card_fields, vec![prio]);
        assert_eq!(b.config.source_filter_id, Some(own));
        let ok2 = create_board(&c, &new_board(p, "Y", BoardConfig { source_filter_id: Some(global), ..Default::default() })).unwrap();
        assert_eq!(get_board(&c, ok2).unwrap().config.source_filter_id, Some(global));
    }

    #[test]
    fn names_are_unique_per_project_and_validated() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        create_board(&c, &new_board(p, "Sprint", BoardConfig::default())).unwrap();
        assert_eq!(create_board(&c, &new_board(p, " sprint ", BoardConfig::default())), Err(PlanError::NameTaken.into()));
        assert_eq!(create_board(&c, &new_board(p, "ÄÖ", BoardConfig::default())).map(|_| ()), Ok(()));
        assert_eq!(create_board(&c, &new_board(p, "äö", BoardConfig::default())), Err(PlanError::NameTaken.into()));
        assert!(create_board(&c, &new_board(q, "Sprint", BoardConfig::default())).is_ok(), "anderes Projekt");
        assert_eq!(create_board(&c, &new_board(p, "  ", BoardConfig::default())), Err(PlanError::NameEmpty.into()));
        assert_eq!(create_board(&c, &new_board(p, &"x".repeat(81), BoardConfig::default())), Err(PlanError::NameTooLong { max: 80 }.into()));
        let mut n = new_board(p, "Z", BoardConfig::default());
        n.description = "d".repeat(501);
        assert_eq!(create_board(&c, &n), Err(ViewError::TextTooLong { max: 500 }));
    }

    #[test]
    fn system_and_missing_projects_are_rejected() {
        let c = db();
        let sys = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(create_board(&c, &new_board(sys, "X", BoardConfig::default())), Err(PlanError::SystemProtected.into()));
        assert_eq!(create_board(&c, &new_board(9999, "X", BoardConfig::default())), Err(ViewError::not_found()));
    }

    #[test]
    fn updates_partially_and_checks_name_conflicts() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = statuses(&c, p);
        let a = create_board(&c, &new_board(p, "A", cfg_cols(&[&[s[0]]]))).unwrap();
        let b = create_board(&c, &new_board(p, "B", BoardConfig::default())).unwrap();
        update_board(&c, a, &UpdateBoard { description: Some("neu".into()), ..Default::default() }).unwrap();
        let got = get_board(&c, a).unwrap();
        assert_eq!((got.name.as_str(), got.description.as_str()), ("A", "neu"));
        assert_eq!(got.config.columns.len(), 1, "Konfiguration blieb");
        assert_eq!(update_board(&c, a, &UpdateBoard { name: Some("b".into()), ..Default::default() }), Err(PlanError::NameTaken.into()));
        // eigenen Namen mit anderer Schreibweise setzen ist erlaubt
        update_board(&c, a, &UpdateBoard { name: Some("a".into()), ..Default::default() }).unwrap();
        assert_eq!(get_board(&c, a).unwrap().name, "a");
        // ungültige Konfiguration ändert nichts
        let bad = cfg_cols(&[&[9999]]);
        assert!(update_board(&c, b, &UpdateBoard { name: Some("B2".into()), config: Some(bad), ..Default::default() }).is_err());
        assert_eq!(get_board(&c, b).unwrap().name, "B");
    }

    #[test]
    fn reorders_and_deletes() {
        let c = db();
        let p = project(&c, "AA", false);
        let ids: Vec<i64> = ["A", "B", "C"].iter().map(|n| create_board(&c, &new_board(p, n, BoardConfig::default())).unwrap()).collect();
        reorder_boards(&c, p, &[ids[2], ids[0], ids[1]]).unwrap();
        assert_eq!(list_boards(&c, p).unwrap().iter().map(|b| b.name.clone()).collect::<Vec<_>>(), vec!["C", "A", "B"]);
        assert_eq!(reorder_boards(&c, p, &[ids[0]]), Err(PlanError::ReorderMismatch.into()));
        assert_eq!(reorder_boards(&c, p, &[ids[0], ids[0], ids[1]]), Err(PlanError::ReorderMismatch.into()));
        delete_board(&c, ids[0]).unwrap();
        assert_eq!(list_boards(&c, p).unwrap().len(), 2);
        assert_eq!(delete_board(&c, ids[0]), Err(ViewError::not_found()));
        // neue Boards kommen ans Ende (Position wird nie doppelt)
        let n = create_board(&c, &new_board(p, "N", BoardConfig::default())).unwrap();
        assert_eq!(list_boards(&c, p).unwrap().last().unwrap().id, n);
    }

    #[test]
    fn duplicates_with_unique_names_in_both_languages() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = statuses(&c, p);
        let a = create_board(&c, &NewBoard { project_id: p, name: "Team".into(), description: "d".into(), config: cfg_cols(&[&[s[0]], &[s[1]]]) }).unwrap();
        let d1 = duplicate_board(&c, a, Lang::En).unwrap();
        let d2 = duplicate_board(&c, a, Lang::En).unwrap();
        let d3 = duplicate_board(&c, a, Lang::De).unwrap();
        let get = |id| get_board(&c, id).unwrap();
        assert_eq!(get(d1).name, "Copy of Team");
        assert_eq!(get(d2).name, "Copy 2 of Team");
        assert_eq!(get(d3).name, "Kopie von Team");
        assert_eq!(get(d1).config, get(a).config);
        assert_eq!(get(d1).description, "d");
        assert!(get(d3).position > get(d1).position);
        assert_eq!(duplicate_board(&c, 9999, Lang::En), Err(ViewError::not_found()));
    }

    #[test]
    fn default_board_follows_workflow_statuses() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = statuses(&c, p);
        let nb = default_board_for(&c, p, Lang::En).unwrap();
        assert_eq!(nb.name, "Board");
        assert_eq!(nb.config.columns.len(), 3);
        assert_eq!(nb.config.columns.iter().map(|c| c.status_ids.clone()).collect::<Vec<_>>(), vec![vec![s[0]], vec![s[1]], vec![s[2]]]);
        assert!(nb.config.columns.iter().all(|c| c.title.is_empty()));
        assert!(nb.config.type_filter.is_empty() && nb.config.source_filter_id.is_none() && !nb.config.only_active_sprint);
        assert!(nb.config.show_key && nb.config.show_type_icon);
        // Priorität (alle 5 Typen), Story Points (Userstory, Devtask), Epic (Userstory)
        let keys: Vec<String> = nb.config.card_fields.iter().map(|id| crate::planning::fields::get_field(&c, *id).unwrap().key.unwrap()).collect();
        assert_eq!(keys.iter().filter(|k| *k == "priority").count(), 5);
        assert_eq!(keys.iter().filter(|k| *k == "story_points").count(), 2);
        assert_eq!(keys.iter().filter(|k| *k == "epic").count(), 1);
        let first_sp = keys.iter().position(|k| k == "story_points").unwrap();
        assert!(keys[..first_sp].iter().all(|k| k == "priority"), "Priorität zuerst, dann Story Points, dann Epic");
        assert_eq!(keys.last().unwrap(), "epic");
        // gespeicherbar und gültig
        let id = create_board(&c, &nb).unwrap();
        assert_eq!(get_board(&c, id).unwrap().config, nb.config);
        assert_eq!(default_board_for(&c, p, Lang::De).unwrap().name, "Board");
        assert_ne!(default_board_for(&c, p, Lang::De).unwrap().description, nb.description);
    }

    #[test]
    fn default_board_skips_unused_statuses_and_missing_fields() {
        let c = db();
        let p = project(&c, "AA", false);
        // ohne Status und Typen: leeres Board
        let nb = default_board_for(&c, p, Lang::En).unwrap();
        assert!(nb.config.columns.is_empty() && nb.config.card_fields.is_empty());
        // ein Status ohne Workflow-Zuordnung erscheint nicht
        let lonely = create_status(&c, &NewStatus { project_id: p, key: None, name: Some("Einsam".into()), color: "#112233".into(), category: StatusCategory::Todo }).unwrap();
        assert!(default_board_for(&c, p, Lang::En).unwrap().config.columns.is_empty());
        // Typ mit Workflow nutzt ihn
        let t = crate::planning::types::create_type(&c, &crate::planning::types::NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#445566".into(), workflow: true }).unwrap();
        crate::planning::workflow::set_workflow_statuses(&c, t, &[lonely]).unwrap();
        let nb = default_board_for(&c, p, Lang::En).unwrap();
        assert_eq!(nb.config.columns.len(), 1);
        assert_eq!(nb.config.columns[0].status_ids, vec![lonely]);
        // Typ ohne Workflow: seine Felder zählen nicht
        let t2 = crate::planning::types::create_type(&c, &crate::planning::types::NewType { project_id: p, key: None, name: Some("U".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        create_field(&c, &NewField::new(t2, "x", FieldKind::Number)).unwrap();
        assert!(default_board_for(&c, p, Lang::En).unwrap().config.card_fields.is_empty());
    }

    #[test]
    fn ensure_default_board_creates_only_once() {
        let c = db();
        let p = project(&c, "AA", true);
        let id = ensure_default_board(&c, p, Lang::De).unwrap();
        assert_eq!(ensure_default_board(&c, p, Lang::En).unwrap(), id);
        assert_eq!(list_boards(&c, p).unwrap().len(), 1);
    }

    #[test]
    fn column_titles_follow_the_status_name_and_language() {
        let c = db();
        let p = project(&c, "AA", true);
        let sts = list_statuses(&c, p).unwrap();
        let col = BoardColumn { title: String::new(), status_ids: vec![sts[0].id, sts[1].id] };
        assert_eq!(column_title(&col, &sts, Lang::En), "To Do");
        assert_eq!(column_title(&col, &sts, Lang::De), "Zu erledigen");
        let named = BoardColumn { title: "Backlog".into(), status_ids: vec![sts[0].id] };
        assert_eq!(column_title(&named, &sts, Lang::De), "Backlog");
        assert_eq!(column_title(&BoardColumn::default(), &sts, Lang::En), "");
    }

    #[test]
    fn prune_removes_deleted_statuses_and_empty_columns() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = statuses(&c, p);
        let id = create_board(&c, &new_board(p, "B", cfg_cols(&[&[s[0], s[1]], &[s[2]]]))).unwrap();
        // ohne Änderung: nichts zu tun
        assert_eq!(prune_board_refs(&c, p).unwrap(), 0);
        // Status direkt aus der Tabelle entfernen (wie es die Löschfunktion nach der Migration tut)
        c.execute("DELETE FROM statuses WHERE id = ?1", [s[1]]).unwrap();
        assert_eq!(prune_board_refs(&c, p).unwrap(), 1);
        assert_eq!(get_board(&c, id).unwrap().config.columns, vec![BoardColumn { title: String::new(), status_ids: vec![s[0]] }, BoardColumn { title: String::new(), status_ids: vec![s[2]] }]);
        c.execute("DELETE FROM statuses WHERE id = ?1", [s[2]]).unwrap();
        assert_eq!(prune_board_refs(&c, p).unwrap(), 1);
        assert_eq!(get_board(&c, id).unwrap().config.columns.len(), 1, "die leer gewordene Spalte fällt weg");
        // jetzt ist alles gültig
        assert_eq!(prune_board_refs(&c, p).unwrap(), 0);
        assert!(update_board(&c, id, &UpdateBoard { config: Some(get_board(&c, id).unwrap().config), ..Default::default() }).is_ok());
    }

    #[test]
    fn prune_removes_types_fields_and_filters() {
        let c = db();
        let p = project(&c, "AA", true);
        let us = type_by_key(&c, p, "userstory").id;
        let dt = type_by_key(&c, p, "devtask").id;
        let fields = list_fields(&c, us).unwrap();
        let (f1, f2) = (fields[0].id, fields[1].id);
        let flt = create_filter(&c, &NewFilter { project_id: Some(p), name: "F".into(), jql: "type = Epic".into() }).unwrap();
        let id = create_board(&c, &new_board(p, "B", BoardConfig { type_filter: vec![us, dt], card_fields: vec![f1, f2], source_filter_id: Some(flt), ..Default::default() })).unwrap();
        c.execute("DELETE FROM fields WHERE id = ?1", [f1]).unwrap();
        c.execute("DELETE FROM ticket_types WHERE id = ?1", [dt]).unwrap();
        c.execute("DELETE FROM saved_filters WHERE id = ?1", [flt]).unwrap();
        assert_eq!(prune_board_refs(&c, p).unwrap(), 1);
        let cfg = get_board(&c, id).unwrap().config;
        assert_eq!(cfg.type_filter, vec![us]);
        assert_eq!(cfg.card_fields, vec![f2]);
        assert_eq!(cfg.source_filter_id, None);
        // Typ-Filter, dessen Typen alle weg sind: wieder „alle“
        c.execute("DELETE FROM ticket_types WHERE id = ?1", [us]).unwrap();
        prune_board_refs(&c, p).unwrap();
        assert!(get_board(&c, id).unwrap().config.type_filter.is_empty());
    }

    #[test]
    fn prune_keeps_other_settings_and_other_projects() {
        let c = db();
        let (p, q) = (project(&c, "AA", true), project(&c, "BB", true));
        let (sp, sq) = (statuses(&c, p), statuses(&c, q));
        let cfg = BoardConfig { columns: vec![BoardColumn { title: "T".into(), status_ids: vec![sp[0]] }], only_active_sprint: true, show_key: false, show_type_icon: false, ..Default::default() };
        let a = create_board(&c, &new_board(p, "A", cfg.clone())).unwrap();
        let b = create_board(&c, &new_board(q, "B", cfg_cols(&[&[sq[0]]]))).unwrap();
        c.execute("DELETE FROM statuses WHERE id = ?1", [sq[0]]).unwrap();
        assert_eq!(prune_board_refs(&c, p).unwrap(), 0, "Projekt AA hat nichts verloren");
        assert_eq!(get_board(&c, a).unwrap().config, cfg);
        assert_eq!(prune_all_board_refs(&c).unwrap(), 1);
        assert!(get_board(&c, b).unwrap().config.columns.is_empty());
        assert_eq!(get_board(&c, a).unwrap().config, cfg);
    }

    #[test]
    fn prune_works_with_the_real_delete_functions() {
        let c = db();
        let p = project(&c, "AA", true);
        let us = type_by_key(&c, p, "userstory").id;
        let sp = list_fields(&c, us).unwrap().into_iter().find(|f| f.key.as_deref() == Some("story_points")).unwrap().id;
        let id = create_board(&c, &new_board(p, "B", BoardConfig { card_fields: vec![sp], ..Default::default() })).unwrap();
        // die Löschfunktion räumt die Boards selbst auf; ein weiteres Aufräumen ändert nichts mehr
        delete_field(&c, sp, None).unwrap();
        assert!(get_board(&c, id).unwrap().config.card_fields.is_empty());
        assert_eq!(prune_board_refs(&c, p).unwrap(), 0);
    }

    #[test]
    fn deleting_a_filter_clears_board_sources() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        let own = create_filter(&c, &NewFilter { project_id: Some(p), name: "own".into(), jql: String::new() }).unwrap();
        let global = create_filter(&c, &NewFilter { project_id: None, name: "g".into(), jql: String::new() }).unwrap();
        let a = create_board(&c, &new_board(p, "A", BoardConfig { source_filter_id: Some(own), ..Default::default() })).unwrap();
        let b = create_board(&c, &new_board(p, "B", BoardConfig { source_filter_id: Some(global), ..Default::default() })).unwrap();
        let d = create_board(&c, &new_board(q, "D", BoardConfig { source_filter_id: Some(global), ..Default::default() })).unwrap();
        delete_filter(&c, own).unwrap();
        assert_eq!(get_board(&c, a).unwrap().config.source_filter_id, None);
        assert_eq!(get_board(&c, b).unwrap().config.source_filter_id, Some(global));
        delete_filter(&c, global).unwrap();
        assert_eq!(get_board(&c, b).unwrap().config.source_filter_id, None);
        assert_eq!(get_board(&c, d).unwrap().config.source_filter_id, None, "auch in anderen Projekten");
    }

    #[test]
    fn boards_cascade_with_the_project() {
        let c = db();
        let p = project(&c, "AA", true);
        create_board(&c, &new_board(p, "B", BoardConfig::default())).unwrap();
        delete_project(&c, p).unwrap();
        assert!(get_project(&c, p).is_err());
        let n: i64 = c.query_row("SELECT COUNT(*) FROM boards", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn unreadable_config_rows_do_not_break_listing() {
        let c = db();
        let p = project(&c, "AA", false);
        let id = create_board(&c, &new_board(p, "B", BoardConfig::default())).unwrap();
        c.execute("UPDATE boards SET config = 'kaputt' WHERE id = ?1", [id]).unwrap();
        let b = get_board(&c, id).unwrap();
        assert_eq!(b.config, BoardConfig::default());
        assert_eq!(list_boards(&c, p).unwrap().len(), 1);
    }
}
