//! EINHÄNGEPUNKTE für Sprints, Versionen und Meilensteine.
//!
//! Die Tabellen dazu gibt es in diesem Zweig noch nicht (ein anderer Teil baut sie). Bis dahin liefert die Abfrage dafür
//! `SearchError::NotAvailableYet`. Wer die Tabellen anschließt, ersetzt NUR die Funktionen in dieser Datei:
//!
//! * [`sprint_names`] / [`version_names`] – Namen für das Schema (Validator, Autovervollständigung). `None` = noch keine Tabelle:
//!   das Feld steht dann als Text im Schema (der Validator lässt jeden Wert zu), die Auswertung meldet `NotAvailableYet`.
//! * [`sprint_condition`] – SQL für `sprint <op> werte` und `sprint IS [NOT] EMPTY` (über die Sprint-Zuordnung der Tickets).
//! * [`sprint_order`] – SQL-Ausdruck für `ORDER BY sprint` (Reihenfolge der Sprints).
//! * [`active_sprint_name`] – Auflösung von `activeSprint()` (Name des aktiven Sprints des Projekts).
//! * [`active_sprint_condition`] – `WHERE`-Teil für „nur aktiver Sprint“ am Board.
//! * [`version_condition`] – SQL für Felder der Art Version/Meilenstein (`ticket_values.text` enthält die Kennung der Version).
//!
//! Die Werte stehen für Versionen schon in `ticket_values` (Kennung als Text, siehe `ticket_schema.rs`); es fehlen nur die Namen.

use super::compile::Cx;
use super::error::{Feature, SRes, SearchError};
use super::Scope;
use crate::query::CmpOp;
use rusqlite::Connection;

/// Namen der Sprints im Bereich (für das Schema). EINHÄNGEPUNKT: Sprint-Tabelle.
pub(super) fn sprint_names(_conn: &Connection, _scope: Scope) -> Option<Vec<String>> {
    None
}

/// Namen der Versionen bzw. Meilensteine im Bereich (Kennung, Name). EINHÄNGEPUNKT: Versions-Tabelle.
pub(super) fn version_names(_conn: &Connection, _scope: Scope, _milestones: bool) -> Option<Vec<(i64, String)>> {
    None
}

/// SQL (immer 0/1, nie NULL) für `sprint <op> werte`, mit `Eq` als „eines der Werte“. `None` als Operator = `IS EMPTY` (der Aufrufer
/// verneint für `IS NOT EMPTY`). EINHÄNGEPUNKT: Sprint-Zuordnung.
pub(super) fn sprint_condition(_cx: &mut Cx<'_>, _op: Option<CmpOp>, _values: &[String]) -> SRes<String> {
    Err(SearchError::NotAvailableYet(Feature::Sprints))
}

/// Sortier-Ausdruck für `ORDER BY sprint` (kleiner = früher). EINHÄNGEPUNKT: Sprint-Tabelle.
pub(super) fn sprint_order(_cx: &mut Cx<'_>) -> SRes<String> {
    Err(SearchError::NotAvailableYet(Feature::Sprints))
}

/// Name des aktiven Sprints (für `activeSprint()`). EINHÄNGEPUNKT: Sprint-Tabelle.
pub(super) fn active_sprint_name(_cx: &mut Cx<'_>) -> SRes<Vec<String>> {
    Err(SearchError::NotAvailableYet(Feature::Sprints))
}

/// Bedingung (SQL, 0/1) „Ticket gehört zum aktiven Sprint des Projekts“ (Board-Schalter). EINHÄNGEPUNKT: Sprint-Zuordnung.
pub(crate) fn active_sprint_condition(_cx: &mut Cx<'_>, _project_id: i64) -> SRes<String> {
    Err(SearchError::NotAvailableYet(Feature::Sprints))
}

/// SQL (0/1) für ein Feld der Art Version/Meilenstein (`None` als Operator = `IS EMPTY`). EINHÄNGEPUNKT: Versions-Tabelle.
pub(super) fn version_condition(_cx: &mut Cx<'_>, _field_ids: &[i64], _op: Option<CmpOp>, _values: &[String]) -> SRes<String> {
    Err(SearchError::NotAvailableYet(Feature::Versions))
}
