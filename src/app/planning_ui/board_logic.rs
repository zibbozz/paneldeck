//! Reine Logik der Board-Ansicht (ohne egui): Spalten und Karten aus Konfiguration und Tickets zusammenstellen, Ziel-Status
//! beim Ziehen wählen, Einfüge-Position, Bearbeiten der Board-Konfiguration (Spalten, Status, Typen, Kartenfelder).
//! Die Oberfläche (`board_view.rs`, `board_cfg.rs`) ruft nur diese Funktionen auf; alles hier ist ohne Fenster testbar.
//!
//! Einhängepunkte für die noch fehlenden Teile:
//! * `// TODO search` – der Quellfilter (gespeicherte Abfrage) wird noch nicht ausgewertet ([`load_board`]),
//! * `// TODO sprints` – „nur aktiver Sprint“ wird noch nicht ausgewertet ([`load_board`]).

use super::values::show_value;
use crate::planning::fields::{Field, FieldKind};
use crate::planning::statuses::StatusCategory;
use crate::planning::summary::ProjectSummary;
use crate::planning::ticket_error::{TRes, TicketError};
use crate::planning::ticket_list::{list_tickets, TicketFilter, TicketOrder};
use crate::planning::tickets::{get_ticket, Ticket, POINTS_FIELD_KEY};
use crate::planning::transition::{plan_transition, TransitionOption};
use crate::planning::values::{decode, is_empty, Row};
use crate::planning::views::boards::{column_title, BoardColumn, BoardConfig};
use crate::state::Lang;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// So viele Karten zeigt eine Spalte auf einmal (danach „n weitere …“, je Klick eine Seite mehr).
pub const PAGE: usize = 25;

// ------------------------------------------------------------------ Schnellfilter

/// Die lokalen Schnellfilter oberhalb des Boards (die Abfragesprache kommt später).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quick {
    pub text: String,
    /// Nur diese Typen (leer = alle Typen des Boards).
    pub types: Vec<i64>,
    /// Nur Tickets mit Favoriten-Stern.
    pub starred: bool,
    /// Nur markierte Tickets (Flag).
    pub flagged: bool,
}

impl Quick {
    pub fn active(&self) -> bool {
        !self.text.trim().is_empty() || !self.types.is_empty() || self.starred || self.flagged
    }
}

/// Typen, die tatsächlich abgefragt werden: Typ-Filter des Boards und Typ-Chips zusammen. Leer = alle Typen;
/// `None` = die Schnittmenge ist leer (nichts passt).
pub fn effective_types(board_filter: &[i64], quick: &[i64]) -> Option<Vec<i64>> {
    match (board_filter.is_empty(), quick.is_empty()) {
        (true, true) => Some(Vec::new()),
        (true, false) => Some(quick.to_vec()),
        (false, true) => Some(board_filter.to_vec()),
        (false, false) => {
            let both: Vec<i64> = quick.iter().copied().filter(|t| board_filter.contains(t)).collect();
            if both.is_empty() { None } else { Some(both) }
        }
    }
}

// ------------------------------------------------------------------ Daten der Ansicht

/// Ein Kartenfeld, schon als Text aufbereitet (in der Sprache beim Laden).
#[derive(Debug, Clone, PartialEq)]
pub struct CardField {
    pub name: String,
    pub text: String,
    /// `#RRGGBB` (z. B. Farbe des Dropdown-Werts).
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub ticket: Ticket,
    pub type_name: String,
    pub type_color: String,
    pub type_icon: Option<String>,
    /// Status der Kategorie „Erledigt“ (die Karte wird gedämpft).
    pub done: bool,
    pub points: Option<f64>,
    pub fields: Vec<CardField>,
    /// Werte der Labels-Felder (als Chips).
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub title: String,
    pub status_ids: Vec<i64>,
    /// Farbe des ersten Status.
    pub color: String,
    pub cards: Vec<Card>,
    /// Summe der Story Points der Karten.
    pub points: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardData {
    pub columns: Vec<Column>,
    /// Es gibt in diesem Projekt ein Story-Points-Feld (sonst zeigt der Spaltenkopf keine Summe).
    pub has_points: bool,
    /// Status der Workflows, die in keiner Spalte stehen (erscheinen nicht auf dem Board).
    pub unassigned: Vec<i64>,
    pub total: usize,
}

impl BoardData {
    pub fn card(&self, ticket_id: i64) -> Option<&Card> {
        self.columns.iter().flat_map(|c| c.cards.iter()).find(|c| c.ticket.id == ticket_id)
    }
}

/// Alle Status, die in irgendeinem Workflow des Projekts vorkommen (in der Reihenfolge des Status-Pools).
pub fn workflow_status_ids(sum: &ProjectSummary) -> Vec<i64> {
    let used: HashSet<i64> = sum.types.iter().filter(|t| t.ticket_type.workflow).flat_map(|t| t.workflow_statuses.iter().copied()).collect();
    sum.statuses.iter().map(|s| s.id).filter(|id| used.contains(id)).collect()
}

/// Felder, die sich als Kartenfeld darstellen lassen (lange Texte und Verweise ohne Namen nicht).
pub fn renderable(kind: FieldKind) -> bool {
    !matches!(kind, FieldKind::Rich | FieldKind::Terminal | FieldKind::Version | FieldKind::Milestone)
}

fn field_rows(conn: &Connection, project_id: i64, field_ids: &[i64]) -> TRes<HashMap<(i64, i64), Vec<Row>>> {
    let mut out: HashMap<(i64, i64), Vec<Row>> = HashMap::new();
    if field_ids.is_empty() {
        return Ok(out);
    }
    let sql = format!(
        "SELECT v.ticket_id, v.field_id, v.text, v.num FROM ticket_values v JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND v.field_id IN ({}) ORDER BY v.ticket_id, v.field_id, v.pos",
        vec!["?"; field_ids.len()].join(",")
    );
    let mut args: Vec<i64> = vec![project_id];
    args.extend(field_ids.iter().copied());
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args.iter()), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<f64>>(3)?)))?;
    for row in rows {
        let (t, f, text, num) = row?;
        out.entry((t, f)).or_default().push((text, num));
    }
    Ok(out)
}

/// Text (und Farbe) eines Kartenfelds; `None` = nichts anzuzeigen.
fn card_field_text(f: &Field, v: &Value, refs: &HashMap<i64, String>, lang: Lang) -> Option<(String, Option<String>)> {
    let (text, color) = match f.kind {
        FieldKind::Rich | FieldKind::Terminal | FieldKind::Version | FieldKind::Milestone | FieldKind::Labels => return None,
        FieldKind::Checkbox => {
            if v.as_bool() != Some(true) {
                return None;
            }
            (lang.t("Ja", "Yes").to_string(), None)
        }
        FieldKind::Ticket => (refs.get(&v.as_i64()?).cloned().unwrap_or_default(), None),
        FieldKind::Dropdown => {
            let id = v.as_i64()?;
            (show_value(f.kind, &f.options, v, lang), f.options.values.iter().find(|o| o.id == id).and_then(|o| o.color.clone()))
        }
        _ => (show_value(f.kind, &f.options, v, lang), None),
    };
    if text.trim().is_empty() { None } else { Some((text, color)) }
}

/// Stellt die Spalten mit ihren Karten zusammen: Tickets der Status der Spalten (und Typ-/Schnellfilter) in
/// Backlog-Reihenfolge, je Karte die Kartenfelder, Labels und Story Points.
pub fn load_board(conn: &Connection, sum: &ProjectSummary, cfg: &BoardConfig, q: &Quick, lang: Lang) -> TRes<BoardData> {
    let points_ids: Vec<i64> = sum.types.iter().flat_map(|t| t.fields.iter()).filter(|f| f.key.as_deref() == Some(POINTS_FIELD_KEY)).map(|f| f.id).collect();
    let has_points = !points_ids.is_empty();
    let mut columns: Vec<Column> = cfg
        .columns
        .iter()
        .map(|c| Column {
            title: column_title(c, &sum.statuses, lang),
            status_ids: c.status_ids.clone(),
            color: c.status_ids.first().and_then(|id| sum.status(*id)).map(|s| s.color.clone()).unwrap_or_else(|| "#888888".into()),
            cards: Vec::new(),
            points: 0.0,
        })
        .collect();
    let on_board: HashSet<i64> = cfg.status_ids().into_iter().collect();
    let unassigned: Vec<i64> = workflow_status_ids(sum).into_iter().filter(|s| !on_board.contains(s)).collect();
    let mut data = BoardData { columns: Vec::new(), has_points, unassigned, total: 0 };
    let types = effective_types(&cfg.type_filter, &q.types);
    let (Some(types), false) = (types, on_board.is_empty()) else {
        data.columns = columns;
        return Ok(data);
    };
    // TODO search: cfg.source_filter_id (gespeicherte Abfrage als Quelle) einschränken, sobald `planning::search` da ist.
    // TODO sprints: cfg.only_active_sprint – nur Tickets des aktiven Sprints, sobald es Sprints gibt.
    let filter = TicketFilter {
        types,
        statuses: cfg.status_ids(),
        starred: q.starred.then_some(true),
        flagged: q.flagged.then_some(true),
        text: Some(q.text.trim().to_string()).filter(|t| !t.is_empty()),
        order: TicketOrder::Rank,
        ..TicketFilter::default()
    };
    let page = list_tickets(conn, Some(sum.project.id), &filter)?;
    // Felder, deren Werte gebraucht werden
    let all_fields: HashMap<i64, &Field> = sum.types.iter().flat_map(|t| t.fields.iter()).map(|f| (f.id, f)).collect();
    let label_ids: Vec<i64> = all_fields.values().filter(|f| f.kind == FieldKind::Labels && (f.show_on_card || cfg.card_fields.contains(&f.id))).map(|f| f.id).collect();
    let mut wanted: Vec<i64> = points_ids.clone();
    wanted.extend(cfg.card_fields.iter().copied().filter(|id| all_fields.contains_key(id)));
    wanted.extend(label_ids.iter().copied());
    wanted.sort_unstable();
    wanted.dedup();
    let rows = field_rows(conn, sum.project.id, &wanted)?;
    let mut refs: HashMap<i64, String> = HashMap::new();
    for ((_, fid), r) in &rows {
        if all_fields.get(fid).is_some_and(|f| f.kind == FieldKind::Ticket) {
            if let Some(Value::Number(n)) = decode(FieldKind::Ticket, r) {
                if let Some(id) = n.as_i64() {
                    if let std::collections::hash_map::Entry::Vacant(e) = refs.entry(id) {
                        if let Ok(t) = get_ticket(conn, id) {
                            e.insert(t.key());
                        }
                    }
                }
            }
        }
    }
    for t in page.items {
        let Some(ci) = t.status_id.and_then(|s| cfg.column_of(s)) else { continue };
        let Some(ty) = sum.type_by_id(t.type_id) else { continue };
        let value_of = |fid: i64| -> Option<Value> {
            let f = all_fields.get(&fid)?;
            decode(f.kind, rows.get(&(t.id, fid))?)
        };
        let points = points_ids.iter().find_map(|fid| value_of(*fid)).and_then(|v| v.as_f64());
        let mut fields = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        for fid in cfg.card_fields.iter().chain(label_ids.iter()) {
            let Some(f) = all_fields.get(fid) else { continue };
            if f.type_id != t.type_id {
                continue;
            }
            let Some(v) = value_of(*fid) else { continue };
            if f.kind == FieldKind::Labels {
                for x in v.as_array().into_iter().flatten() {
                    if let Some(s) = x.as_str() {
                        if !labels.iter().any(|l| l == s) {
                            labels.push(s.to_string());
                        }
                    }
                }
            } else if let Some((text, color)) = card_field_text(f, &v, &refs, lang) {
                if !fields.iter().any(|c: &CardField| c.name == f.display(lang) && c.text == text) {
                    fields.push(CardField { name: f.display(lang), text, color });
                }
            }
        }
        let done = t.status_id.and_then(|s| sum.status(s)).is_some_and(|s| s.category == StatusCategory::Done);
        if let Some(p) = points {
            columns[ci].points += p;
        }
        columns[ci].cards.push(Card { type_name: ty.ticket_type.display(lang), type_color: ty.ticket_type.color.clone(), type_icon: ty.ticket_type.icon.clone(), done, points, fields, labels, ticket: t });
        data.total += 1;
    }
    data.columns = columns;
    Ok(data)
}

/// Zeigt die Summe der Story Points ohne unnötige Nachkommastellen („13“, „2.5“).
pub fn points_text(x: f64) -> String {
    if x.fract() == 0.0 { format!("{}", x as i64) } else { format!("{x:.1}") }
}

// ------------------------------------------------------------------ Ziehen

/// Was beim Fallenlassen einer Karte in einer Spalte passiert.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Die Karte steht schon in dieser Spalte (nur umsortieren).
    Stay,
    /// Wechsel in diesen Status (erster erlaubter Status der Spalte); `needs` = Pflichtfelder fehlen noch (Dialog).
    Go { status_id: i64, needs: bool },
    /// Mindestens ein Status der Spalte wäre erreichbar, aber eine Bedingung sperrt den Wechsel gerade.
    Blocked,
    /// Der Workflow erlaubt den Wechsel in keinen Status der Spalte.
    NotAllowed,
}

impl Pick {
    pub fn allowed(&self) -> bool {
        matches!(self, Pick::Stay | Pick::Go { .. })
    }
}

/// Wählt den Ziel-Status für eine Spalte: bleibt die Karte in der Spalte → `Stay`; sonst der erste Status der Spalte (in
/// deren Reihenfolge), den die Übergänge erlauben und keine Bedingung sperrt.
pub fn pick_target(column_statuses: &[i64], current: Option<i64>, options: &[TransitionOption]) -> Pick {
    if current.is_some_and(|c| column_statuses.contains(&c)) {
        return Pick::Stay;
    }
    let mut blocked = false;
    for s in column_statuses {
        match options.iter().find(|o| o.status_id == *s) {
            Some(o) if !o.blocked => return Pick::Go { status_id: *s, needs: !o.needs_fields.is_empty() },
            Some(_) => blocked = true,
            None => {}
        }
    }
    if blocked { Pick::Blocked } else { Pick::NotAllowed }
}

fn reason_rank(e: &TicketError) -> u8 {
    match e {
        TicketError::ConditionsNotMet => 0,
        TicketError::TransitionNotAllowed => 1,
        TicketError::StatusNotInWorkflow => 2,
        _ => 3,
    }
}

/// Der aussagekräftigste Grund aus mehreren Fehlern (Bedingung vor Matrix vor Workflow-Zugehörigkeit).
pub fn best_reason(errors: Vec<TicketError>) -> Option<TicketError> {
    errors.into_iter().min_by_key(reason_rank)
}

/// Warum die Karte nicht in die Spalte darf (für den Hinweis nach dem Fallenlassen): Fehler der Datenschicht.
pub fn reason_for_column(conn: &Connection, ticket: &Ticket, column_statuses: &[i64]) -> Option<TicketError> {
    let errs: Vec<TicketError> = column_statuses
        .iter()
        .filter_map(|s| match plan_transition(conn, ticket, *s, &[]) {
            Err(TicketError::TransitionNeedsFields(_)) | Ok(_) => None,
            Err(e) => Some(e),
        })
        .collect();
    best_reason(errs)
}

/// Einfüge-Index in einer Spalte: Zahl der Karten, deren Mitte über dem Mauszeiger liegt.
pub fn insert_index(mids: &[f32], y: f32) -> usize {
    mids.iter().filter(|m| **m < y).count()
}

/// Vor welches Ticket die Karte gehört (für `move_rank`): das Ticket an der Einfüge-Stelle der Spalte ohne die gezogene
/// Karte (`None` = ans Ende). `column_ids` enthält **alle** Tickets der Spalte in Reihenfolge – auch die noch nicht
/// angezeigten –, damit die Position auch bei „n weitere …“ stimmt.
pub fn rank_before(column_ids: &[i64], dragged: i64, idx: usize) -> Option<i64> {
    column_ids.iter().copied().filter(|i| *i != dragged).nth(idx)
}

/// Wie viele Karten wirklich angezeigt werden (Seitengröße, begrenzt auf die Zahl der Karten).
pub fn shown_count(pages: usize, total: usize) -> usize {
    (pages.max(1) * PAGE).min(total)
}

// ------------------------------------------------------------------ Konfiguration bearbeiten

/// Status in die Spalte aufnehmen (und aus jeder anderen Spalte entfernen). Leer gewordene Spalten bleiben stehen (die
/// Prüfung meldet sie), damit der Dialog nichts hinter dem Rücken des Nutzers löscht.
pub fn assign_status(cfg: &mut BoardConfig, col: usize, status: i64) {
    for c in &mut cfg.columns {
        c.status_ids.retain(|s| *s != status);
    }
    if let Some(c) = cfg.columns.get_mut(col) {
        c.status_ids.push(status);
    }
}

pub fn unassign_status(cfg: &mut BoardConfig, status: i64) {
    for c in &mut cfg.columns {
        c.status_ids.retain(|s| *s != status);
    }
}

/// Status innerhalb der Spalte verschieben (`dir` -1 = früher, 1 = später); `true`, wenn sich etwas geändert hat.
pub fn move_status(cfg: &mut BoardConfig, col: usize, status: i64, dir: i32) -> bool {
    let Some(c) = cfg.columns.get_mut(col) else { return false };
    let Some(i) = c.status_ids.iter().position(|s| *s == status) else { return false };
    let j = i as i64 + dir as i64;
    if j < 0 || j as usize >= c.status_ids.len() {
        return false;
    }
    c.status_ids.swap(i, j as usize);
    true
}

pub fn move_column(cfg: &mut BoardConfig, col: usize, dir: i32) -> bool {
    let j = col as i64 + dir as i64;
    if col >= cfg.columns.len() || j < 0 || j as usize >= cfg.columns.len() {
        return false;
    }
    cfg.columns.swap(col, j as usize);
    true
}

/// Neue Spalte am Ende; mit `status` wird er dorthin verschoben (leer = Spalte ohne Status).
pub fn add_column(cfg: &mut BoardConfig, status: Option<i64>) {
    if let Some(s) = status {
        unassign_status(cfg, s);
    }
    cfg.columns.push(BoardColumn { title: String::new(), status_ids: status.into_iter().collect() });
}

pub fn remove_column(cfg: &mut BoardConfig, col: usize) {
    if col < cfg.columns.len() {
        cfg.columns.remove(col);
    }
}

/// Status der Workflows, die in keiner Spalte stehen (Reihenfolge wie in `all`).
pub fn unassigned_statuses(cfg: &BoardConfig, all: &[i64]) -> Vec<i64> {
    let used = cfg.status_ids();
    all.iter().copied().filter(|s| !used.contains(s)).collect()
}

pub fn toggle_type(cfg: &mut BoardConfig, type_id: i64) {
    if let Some(i) = cfg.type_filter.iter().position(|t| *t == type_id) {
        cfg.type_filter.remove(i);
    } else {
        cfg.type_filter.push(type_id);
    }
}

/// Felder, die auf mehreren Typen gleich heißen (z. B. „Priorität“), erscheinen in der Auswahl als **ein** Kartenfeld.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldGroup {
    /// Gruppierungsschlüssel: Standard-Schlüssel des Felds oder kleingeschriebener Name, plus Feldtyp.
    pub key: String,
    pub name: String,
    pub kind: FieldKind,
    pub ids: Vec<i64>,
}

/// Auswählbare Kartenfelder des Projekts (Typen mit Workflow, darstellbare Feldtypen), nach Gruppen zusammengefasst, in der
/// Reihenfolge des ersten Auftretens.
pub fn field_groups(sum: &ProjectSummary, lang: Lang) -> Vec<FieldGroup> {
    let mut out: Vec<FieldGroup> = Vec::new();
    for t in sum.types.iter().filter(|t| t.ticket_type.workflow) {
        for f in t.fields.iter().filter(|f| renderable(f.kind)) {
            let name = f.display(lang);
            let key = format!("{}|{}", f.key.clone().unwrap_or_else(|| name.to_lowercase()), f.kind.as_str());
            match out.iter_mut().find(|g| g.key == key) {
                Some(g) => g.ids.push(f.id),
                None => out.push(FieldGroup { key, name, kind: f.kind, ids: vec![f.id] }),
            }
        }
    }
    out
}

pub fn group_selected(cfg: &BoardConfig, g: &FieldGroup) -> bool {
    g.ids.iter().any(|i| cfg.card_fields.contains(i))
}

pub fn toggle_group(cfg: &mut BoardConfig, g: &FieldGroup, on: bool) {
    if on {
        for id in &g.ids {
            if !cfg.card_fields.contains(id) {
                cfg.card_fields.push(*id);
            }
        }
    } else {
        cfg.card_fields.retain(|i| !g.ids.contains(i));
    }
}

/// Ausgewählte Gruppen in der Reihenfolge, in der sie auf der Karte stehen (nach dem ersten ihrer Felder in `card_fields`).
pub fn selected_groups(cfg: &BoardConfig, groups: &[FieldGroup]) -> Vec<usize> {
    let mut v: Vec<(usize, usize)> = groups
        .iter()
        .enumerate()
        .filter_map(|(gi, g)| cfg.card_fields.iter().position(|i| g.ids.contains(i)).map(|p| (p, gi)))
        .collect();
    v.sort();
    v.into_iter().map(|(_, gi)| gi).collect()
}

/// Ausgewähltes Kartenfeld früher/später auf der Karte (`gi` = Index in `groups`).
pub fn move_group(cfg: &mut BoardConfig, groups: &[FieldGroup], gi: usize, dir: i32) -> bool {
    let mut order = selected_groups(cfg, groups);
    let Some(i) = order.iter().position(|g| *g == gi) else { return false };
    let j = i as i64 + dir as i64;
    if j < 0 || j as usize >= order.len() {
        return false;
    }
    order.swap(i, j as usize);
    let mut rebuilt: Vec<i64> = Vec::new();
    for g in order {
        for id in &groups[g].ids {
            if cfg.card_fields.contains(id) {
                rebuilt.push(*id);
            }
        }
    }
    // Felder, die zu keiner Gruppe gehören (Typ ohne Workflow o. Ä.), bleiben am Ende erhalten
    for id in &cfg.card_fields {
        if !rebuilt.contains(id) {
            rebuilt.push(*id);
        }
    }
    cfg.card_fields = rebuilt;
    true
}

/// Freier Name: `base`, sonst `base 2`, `base 3` … (Groß-/Kleinschreibung egal).
pub fn free_name(base: &str, taken: &[String]) -> String {
    let used = |n: &str| taken.iter().any(|t| t.trim().to_lowercase() == n.trim().to_lowercase());
    if !used(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !used(n)).unwrap_or_else(|| base.to_string())
}

/// Die eingegebenen Werte des Pflichtfeld-Dialogs für `transition_ticket`: leere Eingaben bleiben weg.
pub fn collect_values(entries: &[(i64, Value)]) -> Vec<(i64, Value)> {
    entries.iter().filter(|(_, v)| !is_empty(Some(v))).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::history::Source;
    use crate::planning::statuses::list_statuses;
    use crate::planning::summary::project_summary;
    use crate::planning::ticket_error::FieldLabel;
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::{set_field, set_flagged, set_starred};
    use crate::planning::transition::{available_transitions, transition_ticket};
    use crate::planning::views::boards::{check_config, create_board, default_board_for, NewBoard};
    use serde_json::json;

    fn opt(status_id: i64, blocked: bool, needs: usize) -> TransitionOption {
        TransitionOption { status_id, blocked, needs_fields: (0..needs).map(|i| FieldLabel { id: i as i64, en: "F".into(), de: "F".into() }).collect() }
    }

    fn default_cfg(f: &Fx) -> (ProjectSummary, BoardConfig) {
        let sum = project_summary(&f.c, f.project).unwrap();
        let b = default_board_for(&f.c, f.project, Lang::En).unwrap();
        (sum, b.config)
    }

    #[test]
    fn effective_types_combine_board_and_chips() {
        assert_eq!(effective_types(&[], &[]), Some(vec![]));
        assert_eq!(effective_types(&[], &[3]), Some(vec![3]));
        assert_eq!(effective_types(&[1, 2], &[]), Some(vec![1, 2]));
        assert_eq!(effective_types(&[1, 2], &[2, 9]), Some(vec![2]));
        assert_eq!(effective_types(&[1, 2], &[9]), None, "Schnittmenge leer");
    }

    #[test]
    fn pick_prefers_the_first_allowed_status_of_the_column() {
        // Ticket in 1; Spalte hat 5, 6, 7
        let opts = vec![opt(6, false, 0), opt(7, false, 0)];
        assert_eq!(pick_target(&[5, 6, 7], Some(1), &opts), Pick::Go { status_id: 6, needs: false });
        // Status in der Spalte → nur umsortieren
        assert_eq!(pick_target(&[1, 2], Some(1), &opts), Pick::Stay);
        // erster erlaubte ist gesperrt → der nächste
        let opts = vec![opt(5, true, 0), opt(6, false, 2)];
        assert_eq!(pick_target(&[5, 6], Some(1), &opts), Pick::Go { status_id: 6, needs: true });
        // nur gesperrte
        assert_eq!(pick_target(&[5], Some(1), &[opt(5, true, 0)]), Pick::Blocked);
        // nichts erlaubt
        assert_eq!(pick_target(&[8, 9], Some(1), &opts), Pick::NotAllowed);
        assert_eq!(pick_target(&[8], None, &[]), Pick::NotAllowed);
        assert!(Pick::Stay.allowed() && !Pick::Blocked.allowed() && !Pick::NotAllowed.allowed());
    }

    #[test]
    fn best_reason_prefers_conditions_then_matrix() {
        let r = best_reason(vec![TicketError::StatusNotInWorkflow, TicketError::TransitionNotAllowed, TicketError::ConditionsNotMet]);
        assert_eq!(r, Some(TicketError::ConditionsNotMet));
        assert_eq!(best_reason(vec![TicketError::StatusNotInWorkflow, TicketError::TransitionNotAllowed]), Some(TicketError::TransitionNotAllowed));
        assert_eq!(best_reason(vec![]), None);
    }

    #[test]
    fn insert_position_and_rank_target() {
        let mids = [10.0, 40.0, 70.0];
        assert_eq!(insert_index(&mids, 5.0), 0);
        assert_eq!(insert_index(&mids, 41.0), 2);
        assert_eq!(insert_index(&mids, 200.0), 3);
        assert_eq!(insert_index(&[], 50.0), 0);
        // Spalte hat A=1, B=2, C=3; B wird gezogen
        assert_eq!(rank_before(&[1, 2, 3], 2, 0), Some(1), "vor A");
        assert_eq!(rank_before(&[1, 2, 3], 2, 1), Some(3), "zwischen A und C → vor C");
        assert_eq!(rank_before(&[1, 2, 3], 2, 2), None, "ans Ende");
        // neue Karte von außen: alle drei bleiben, Index 3 = Ende
        assert_eq!(rank_before(&[1, 2, 3], 99, 3), None);
        assert_eq!(rank_before(&[1, 2, 3], 99, 1), Some(2));
    }

    #[test]
    fn paging_shows_page_sizes() {
        assert_eq!(shown_count(1, 10), 10);
        assert_eq!(shown_count(1, 100), PAGE);
        assert_eq!(shown_count(2, 100), 2 * PAGE);
        assert_eq!(shown_count(0, 100), PAGE, "mindestens eine Seite");
        assert_eq!(shown_count(5, 30), 30);
    }

    #[test]
    fn column_editing_keeps_each_status_in_one_column() {
        let mut cfg = BoardConfig::default();
        add_column(&mut cfg, Some(1));
        add_column(&mut cfg, Some(2));
        add_column(&mut cfg, None);
        assign_status(&mut cfg, 2, 3);
        assign_status(&mut cfg, 2, 1); // wandert von Spalte 0 nach 2
        assert_eq!(cfg.columns[0].status_ids, Vec::<i64>::new());
        assert_eq!(cfg.columns[2].status_ids, vec![3, 1]);
        assert!(move_status(&mut cfg, 2, 1, -1));
        assert_eq!(cfg.columns[2].status_ids, vec![1, 3]);
        assert!(!move_status(&mut cfg, 2, 1, -1), "schon vorn");
        unassign_status(&mut cfg, 3);
        assert_eq!(cfg.columns[2].status_ids, vec![1]);
        assert!(move_column(&mut cfg, 2, -1));
        assert_eq!(cfg.columns[1].status_ids, vec![1]);
        assert!(!move_column(&mut cfg, 0, -1));
        assert!(!move_column(&mut cfg, 2, 1));
        remove_column(&mut cfg, 0);
        assert_eq!(cfg.columns.len(), 2);
        remove_column(&mut cfg, 9);
        assert_eq!(cfg.columns.len(), 2);
        // add_column mit Status nimmt ihn aus der alten Spalte
        add_column(&mut cfg, Some(1));
        assert_eq!(cfg.columns.iter().flat_map(|c| c.status_ids.iter()).filter(|s| **s == 1).count(), 1);
    }

    #[test]
    fn type_toggle_and_free_names() {
        let mut cfg = BoardConfig::default();
        toggle_type(&mut cfg, 4);
        toggle_type(&mut cfg, 5);
        toggle_type(&mut cfg, 4);
        assert_eq!(cfg.type_filter, vec![5]);
        let taken = vec!["Board".to_string(), "board 2".to_string()];
        assert_eq!(free_name("Board", &[]), "Board");
        assert_eq!(free_name("Board", &taken), "Board 3");
        assert_eq!(free_name("Sprint", &taken), "Sprint");
    }

    #[test]
    fn collected_values_skip_empty_inputs() {
        let v = collect_values(&[(1, json!("")), (2, json!("x")), (3, json!([])), (4, json!(false)), (5, json!(0)), (6, Value::Null)]);
        assert_eq!(v.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![2, 4, 5]);
    }

    #[test]
    fn points_text_trims_zeros() {
        assert_eq!(points_text(13.0), "13");
        assert_eq!(points_text(2.5), "2.5");
        assert_eq!(points_text(0.0), "0");
    }

    #[test]
    fn default_board_loads_columns_cards_and_points() {
        let f = Fx::new();
        let (sum, cfg) = default_cfg(&f);
        let a = f.mk(f.story, "Alpha");
        let b = f.mk(f.story, "Beta");
        let c = f.mk(f.task, "Gamma");
        let d = f.mk(f.story, "Delta");
        let sp = f.field(f.story, "story_points");
        set_field(&f.c, a.id, sp.id, json!(5), &Source::ui()).unwrap();
        set_field(&f.c, b.id, sp.id, json!(3), &Source::ui()).unwrap();
        transition_ticket(&f.c, c.id, f.progress, &[], &Source::ui()).unwrap();
        transition_ticket(&f.c, d.id, f.done, &[], &Source::ui()).unwrap();
        let data = load_board(&f.c, &sum, &cfg, &Quick::default(), Lang::En).unwrap();
        assert!(data.has_points);
        assert_eq!(data.total, 4);
        let col = |sid: i64| data.columns.iter().find(|c| c.status_ids == vec![sid]).unwrap();
        let todo = col(f.todo);
        assert_eq!(todo.title, "To Do");
        assert_eq!(todo.cards.iter().map(|c| c.ticket.title.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Beta"], "Rang-Reihenfolge");
        assert_eq!(todo.points, 8.0);
        assert_eq!(col(f.progress).cards.len(), 1);
        let done = col(f.done);
        assert!(done.cards[0].done && !todo.cards[0].done);
        // Priorität steht (Standardwert Normal) als Kartenfeld, Story Points als Zahl
        let alpha = todo.cards.iter().find(|c| c.ticket.id == a.id).unwrap();
        assert_eq!(alpha.points, Some(5.0));
        assert!(alpha.fields.iter().any(|x| x.name == "Priority" && x.color.is_some()), "{:?}", alpha.fields);
        assert!(alpha.fields.iter().any(|x| x.name == "Story points" && x.text == "5"), "{:?}", alpha.fields);
        assert!(data.unassigned.is_empty());
        assert_eq!(data.card(c.id).unwrap().ticket.title, "Gamma");
        assert_eq!(data.columns.iter().position(|k| k.cards.iter().any(|x| x.ticket.id == c.id)), cfg.column_of(f.progress));
    }

    #[test]
    fn quick_filters_narrow_the_cards() {
        let f = Fx::new();
        let (sum, cfg) = default_cfg(&f);
        let a = f.mk(f.story, "Login page");
        let b = f.mk(f.task, "Logout button");
        let _c = f.mk(f.defect, "Crash");
        set_starred(&f.c, a.id, true, &Source::ui()).unwrap();
        set_flagged(&f.c, b.id, true, &Source::ui()).unwrap();
        let titles = |q: Quick| {
            let d = load_board(&f.c, &sum, &cfg, &q, Lang::En).unwrap();
            let mut t: Vec<String> = d.columns.iter().flat_map(|c| c.cards.iter().map(|k| k.ticket.title.clone())).collect();
            t.sort();
            t
        };
        assert_eq!(titles(Quick::default()).len(), 3);
        assert_eq!(titles(Quick { text: "log".into(), ..Quick::default() }), vec!["Login page", "Logout button"]);
        assert_eq!(titles(Quick { starred: true, ..Quick::default() }), vec!["Login page"]);
        assert_eq!(titles(Quick { flagged: true, ..Quick::default() }), vec!["Logout button"]);
        assert_eq!(titles(Quick { types: vec![f.defect], ..Quick::default() }), vec!["Crash"]);
        assert_eq!(titles(Quick { types: vec![f.defect], starred: true, ..Quick::default() }), Vec::<String>::new());
    }

    #[test]
    fn board_type_filter_and_unassigned_statuses() {
        let f = Fx::new();
        let (sum, mut cfg) = default_cfg(&f);
        f.mk(f.story, "S");
        f.mk(f.defect, "D");
        cfg.type_filter = vec![f.defect];
        // „Done“ nicht auf dem Board
        unassign_status(&mut cfg, f.done);
        cfg.columns.retain(|c| !c.status_ids.is_empty());
        let d = load_board(&f.c, &sum, &cfg, &Quick::default(), Lang::En).unwrap();
        assert_eq!(d.total, 1);
        assert_eq!(d.unassigned, vec![f.done]);
        // Chip eines Typs außerhalb des Board-Filters: nichts passt
        let d = load_board(&f.c, &sum, &cfg, &Quick { types: vec![f.story], ..Quick::default() }, Lang::En).unwrap();
        assert_eq!(d.total, 0);
        // ein Board ohne Spalten fragt nichts ab (nicht „alle Status“)
        let d = load_board(&f.c, &sum, &BoardConfig::default(), &Quick::default(), Lang::En).unwrap();
        assert_eq!((d.total, d.columns.len()), (0, 0));
    }

    #[test]
    fn several_statuses_in_one_column_merge_the_cards() {
        let f = Fx::new();
        let sum = project_summary(&f.c, f.project).unwrap();
        let t1 = f.mk(f.task, "One");
        let t2 = f.mk(f.task, "Two");
        transition_ticket(&f.c, t2.id, f.progress, &[], &Source::ui()).unwrap();
        let cfg = BoardConfig { columns: vec![BoardColumn { title: "Open".into(), status_ids: vec![f.todo, f.progress] }, BoardColumn { title: String::new(), status_ids: vec![f.done] }], ..BoardConfig::default() };
        let d = load_board(&f.c, &sum, &cfg, &Quick::default(), Lang::De).unwrap();
        assert_eq!(d.columns[0].title, "Open");
        assert_eq!(d.columns[1].title, "Erledigt", "leerer Titel = Name des ersten Status, live übersetzt");
        assert_eq!(d.columns[0].cards.iter().map(|c| c.ticket.id).collect::<Vec<_>>(), vec![t1.id, t2.id]);
    }

    #[test]
    fn drag_targets_follow_the_workflow() {
        let f = Fx::new();
        let (_, cfg) = default_cfg(&f);
        let t = f.mk(f.task, "T");
        let opts = available_transitions(&f.c, t.id).unwrap();
        let ticket = crate::planning::tickets::get_ticket(&f.c, t.id).unwrap();
        // alle Spalten erreichbar (Matrix leer = frei), die eigene bleibt
        for (i, c) in cfg.columns.iter().enumerate() {
            let p = pick_target(&c.status_ids, ticket.status_id, &opts);
            if c.status_ids.contains(&f.todo) {
                assert_eq!(p, Pick::Stay, "{i}");
            } else {
                assert!(matches!(p, Pick::Go { needs: false, .. }), "{i}: {p:?}");
            }
        }
        // Übergang todo → done verbieten: die Spalte ist nicht erlaubt, Grund = Matrix
        use crate::planning::workflow::{allow_all_transitions, remove_transition};
        allow_all_transitions(&f.c, f.task).unwrap();
        remove_transition(&f.c, f.task, f.todo, f.done).unwrap();
        let opts = available_transitions(&f.c, t.id).unwrap();
        let done_col = cfg.columns.iter().find(|c| c.status_ids.contains(&f.done)).unwrap();
        assert_eq!(pick_target(&done_col.status_ids, ticket.status_id, &opts), Pick::NotAllowed);
        assert_eq!(reason_for_column(&f.c, &ticket, &done_col.status_ids), Some(TicketError::TransitionNotAllowed));
        let prog_col = cfg.columns.iter().find(|c| c.status_ids.contains(&f.progress)).unwrap();
        assert!(pick_target(&prog_col.status_ids, ticket.status_id, &opts).allowed());
        assert_eq!(reason_for_column(&f.c, &ticket, &prog_col.status_ids), None);
    }

    #[test]
    fn field_groups_merge_equal_fields_across_types() {
        let f = Fx::new();
        let sum = project_summary(&f.c, f.project).unwrap();
        let groups = field_groups(&sum, Lang::En);
        let prio = groups.iter().find(|g| g.name == "Priority").expect("Priorität");
        assert!(prio.ids.len() >= 4, "mehrere Typen haben Priorität: {:?}", prio.ids);
        assert!(groups.iter().all(|g| renderable(g.kind)));
        assert!(!groups.iter().any(|g| g.name == "Description"), "Rich-Text nicht auf der Karte");
        let mut cfg = BoardConfig::default();
        let sp = groups.iter().position(|g| g.name == "Story points").unwrap();
        let pi = groups.iter().position(|g| g.name == "Priority").unwrap();
        toggle_group(&mut cfg, &groups[pi], true);
        toggle_group(&mut cfg, &groups[sp], true);
        toggle_group(&mut cfg, &groups[pi], true); // doppelt ändert nichts
        assert_eq!(selected_groups(&cfg, &groups), vec![pi, sp]);
        assert!(group_selected(&cfg, &groups[sp]));
        assert!(move_group(&mut cfg, &groups, sp, -1));
        assert_eq!(selected_groups(&cfg, &groups), vec![sp, pi]);
        assert!(!move_group(&mut cfg, &groups, sp, -1), "schon vorn");
        assert!(!move_group(&mut cfg, &groups, 0, 1) || group_selected(&cfg, &groups[0]));
        toggle_group(&mut cfg, &groups[pi], false);
        assert!(!group_selected(&cfg, &groups[pi]));
        assert!(group_selected(&cfg, &groups[sp]));
    }

    #[test]
    fn edited_configs_pass_the_data_layer_check() {
        let f = Fx::new();
        let (sum, mut cfg) = default_cfg(&f);
        assert!(check_config(&f.c, f.project, &cfg).is_ok());
        // zwei Status in einer Spalte
        let mut merged = cfg.clone();
        let (a, b) = (merged.columns[0].status_ids[0], merged.columns[1].status_ids[0]);
        assign_status(&mut merged, 0, b);
        merged.columns.retain(|c| !c.status_ids.is_empty());
        assert_eq!(merged.columns[0].status_ids, vec![a, b]);
        assert!(check_config(&f.c, f.project, &merged).is_ok());
        // leere Spalte → Fehler (die Oberfläche zeigt ihn zweisprachig)
        add_column(&mut cfg, None);
        let e = check_config(&f.c, f.project, &cfg).unwrap_err();
        assert!(e.message(Lang::De).contains("mindestens einen Status") && e.message(Lang::En).contains("at least one status"));
        // Status doppelt
        let mut twice = merged.clone();
        twice.columns.push(BoardColumn { title: String::new(), status_ids: vec![a] });
        assert!(check_config(&f.c, f.project, &twice).is_err());
        // Typ-Filter mit fremdem Typ
        let mut bad = merged.clone();
        bad.type_filter = vec![987654];
        assert!(check_config(&f.c, f.project, &bad).is_err());
        // Anlegen mit freiem Namen
        let nb = NewBoard { project_id: f.project, name: free_name("Board", &["Board".to_string()]), description: String::new(), config: merged };
        create_board(&f.c, &nb).unwrap();
        let _ = (sum, list_statuses(&f.c, f.project));
    }
}
