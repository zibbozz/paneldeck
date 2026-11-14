//! Tickets als CSV: Export und Import **desselben** Formats (über `calc::csv`, UTF-8, Komma oder Semikolon).
//!
//! # Spalten
//!
//! Zuerst die festen Spalten `Key, Type, Title, Status, Parent, Flagged, Starred, Created, Updated`, danach je Feld des
//! Projekts eine Spalte in der Reihenfolge der Konfiguration (Typ für Typ). Felder mit gleicher englischer Bezeichnung und
//! gleichem Feldtyp in mehreren Typen teilen sich **eine** Spalte (z. B. „Priority“, „Description“); gleiche Bezeichnung mit
//! anderem Feldtyp ergibt `Name [Feldtyp]`, ein Name wie eine feste Spalte ebenso. Die Kopfzeile ist immer englisch.
//!
//! # Zellen
//!
//! * `Key` ist der Schlüssel (`PAN-12`), `Parent` der Schlüssel des Eltern-Tickets (in derselben Datei); `Flagged`/`Starred`
//!   sind `true`/`false`; `Created`/`Updated` ISO `JJJJ-MM-TTTHH:MM:SS`. Typ, Status und Werte von Dropdown/Mehrfachauswahl
//!   stehen mit ihrem Namen (Export englisch; der Import erkennt Deutsch und Englisch).
//! * Version- und Meilenstein-Felder stehen mit dem **Namen** der Version/des Meilensteins; der Import ordnet sie über den Namen
//!   (ohne Beachtung der Groß-/Kleinschreibung) den vorhandenen Einträgen des Zielprojekts zu – gibt es den Namen dort nicht,
//!   ist das ein Fehler mit Zeile und Spalte.
//! * Nutzt das Projekt Sprints (`use_sprints`), folgt als **zehnte, optionale Spalte `Sprint`** der Name des offenen (geplanten
//!   oder aktiven) Sprints des Tickets. Der Import legt das neue Ticket in den gleichnamigen Sprint des Zielprojekts (ohne
//!   Beachtung der Groß-/Kleinschreibung; unbekannter oder abgeschlossener Sprint = Fehler mit Zeile und Spalte). Die Spalte ist
//!   bei Projekten ohne Sprints nicht erlaubt; fehlt sie in der Datei, kommen die Tickets ins Backlog.
//! * Mehrfachwerte (Mehrfachauswahl, Labels) sind mit `|` verbunden; `|` und `\` im Wert werden als `\|` und `\\`
//!   geschrieben. Datum und Datum+Uhrzeit sind ISO, Checkboxen `true`/`false`, Zahlen mit Punkt, Ticket-Felder der Schlüssel
//!   des Ziels. Rich-Text bleibt unverändert (mit Zeilenumbrüchen).
//! * **Formel-Schutz** (`excel_safe`): Zellen, die mit `=`, `+`, `-`, `@`, Tabulator oder Wagenrücklauf beginnen (auch nach führenden `'`), bekommen beim
//!   Schreiben ein führendes `'`; der Import entfernt es wieder (`CsvImportOptions::unprotect`, Standard an). Wurde eine Datei
//!   *ohne* Schutz geschrieben und enthält Zellen, die mit `'` und dann `=`/`+`/`-`/`@` beginnen, muss der Import mit
//!   `unprotect: false` laufen (sonst fehlt ihnen das erste `'`).
//!
//! # Import
//!
//! Der Import legt **immer neue Tickets** an (nie vorhandene ändern). Schlüssel der Datei werden nicht übernommen: die Tickets
//! bekommen die nächsten Nummern des Projekts, der Schlüssel der Datei steht im Verlauf („Importiert (Schlüssel in der
//! Datei: PAN-12)“) und dient nur dazu, `Parent` und Ticket-Felder innerhalb der Datei aufzulösen. Die Kopfzeile wird streng
//! geprüft (feste Spalten genau am Anfang; jede weitere Spalte muss ein Feld dieses Projekts sein, keine doppelt; fehlende
//! Feld-Spalten sind erlaubt – dann gilt der Standardwert). Wird ein Feld gesetzt, das der Typ des Tickets nicht hat, ist das
//! ein Fehler. Leere Zellen in vorhandenen Feld-Spalten heißen „leer“ (kein Standardwert); Pflichtfelder prüft die Datenschicht
//! wie beim Anlegen in der Oberfläche. **Alles oder nichts:** gibt es Fehler (Zeile und Spalte, höchstens 50), wird nichts
//! angelegt. Die Reihenfolge der Datei wird zur Reihenfolge im Backlog.

use super::error::{BackupError, HeaderProblem, RowIssue, RowProblem, Warning, MAX_ROW_PROBLEMS};
use super::progress::Progress;
use super::valid_timestamp;
use crate::calc::csv::{detect_delimiter, read_csv, write_csv, CsvTable, Delimiter, WriteOptions};
use crate::planning::common::{self, labels, same_label};
use crate::planning::fields::{Field, FieldKind};
use crate::planning::history::Source;
use crate::planning::projects::editable;
use crate::planning::sprints::sprint::move_tickets_to_sprint;
use crate::planning::statuses::Status;
use crate::planning::summary::{project_summary, ProjectSummary};
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::{clean_title, create_ticket_noted, parse_key, raw_set_ranks, NewTicket};
use crate::planning::values::{decode, normalize_value, write_value, Row, RefCtx};
use crate::state::Lang;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Optionale zehnte Spalte bei Projekten mit Sprints.
pub const SPRINT_COLUMN: &str = "Sprint";

/// Die festen Spalten am Anfang jeder Datei.
pub const FIXED_COLUMNS: [&str; 9] = ["Key", "Type", "Title", "Status", "Parent", "Flagged", "Starred", "Created", "Updated"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvOptions {
    /// Zellen, die mit `=`, `+`, `-` oder `@` beginnen, bekommen ein führendes `'` (Schutz vor Formeln in Excel & Co.).
    pub excel_safe: bool,
    pub delimiter: Delimiter,
    /// UTF-8-BOM am Anfang (damit Excel UTF-8 erkennt).
    pub bom: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        CsvOptions { excel_safe: false, delimiter: Delimiter::Comma, bom: false }
    }
}

impl CsvOptions {
    /// Voreinstellung für den Export aus der Oberfläche: mit Formel-Schutz und BOM.
    pub fn for_ui() -> Self {
        CsvOptions { excel_safe: true, delimiter: Delimiter::Comma, bom: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvImportOptions {
    /// Das führende `'` vor `=`, `+`, `-`, `@` entfernen (Gegenstück zu `CsvOptions::excel_safe`).
    pub unprotect: bool,
}

impl Default for CsvImportOptions {
    fn default() -> Self {
        CsvImportOptions { unprotect: true }
    }
}

// ------------------------------------------------------------------ Formel-Schutz und Listen

fn is_trigger(c: char) -> bool {
    // `=`, `+`, `-`, `@` sowie Tabulator und Wagenrücklauf (OWASP „CSV Injection“: Excel wertet auch diese als Beginn einer Formel)
    matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r')
}

/// Setzt ein `'` vor Zellen, die (nach beliebig vielen `'`) mit `=`, `+`, `-` oder `@` beginnen.
fn guard(s: &str) -> String {
    if s.trim_start_matches('\'').chars().next().is_some_and(is_trigger) { format!("'{s}") } else { s.to_string() }
}

/// Gegenstück zu [`guard`].
fn unguard(s: &str) -> &str {
    if s.starts_with('\'') && s.trim_start_matches('\'').chars().next().is_some_and(is_trigger) { &s[1..] } else { s }
}

fn join_list(items: &[String]) -> String {
    items.iter().map(|i| i.replace('\\', "\\\\").replace('|', "\\|")).collect::<Vec<_>>().join("|")
}

fn split_list(s: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if matches!(chars.peek(), Some('\\' | '|')) => out.last_mut().expect("nie leer").push(chars.next().expect("peek")),
            '|' => out.push(String::new()),
            c => out.last_mut().expect("nie leer").push(c),
        }
    }
    out
}

// ------------------------------------------------------------------ Spalten

/// Eine Feld-Spalte der Datei: alle Felder (je Typ eines) mit gleicher Bezeichnung und gleichem Feldtyp.
struct Col {
    header: String,
    kind: FieldKind,
    members: HashMap<i64, Field>,
}

fn columns(sum: &ProjectSummary) -> Vec<Col> {
    let mut cols: Vec<(String, Col)> = Vec::new();
    for t in sum.types.iter().filter(|t| !t.ticket_type.system) {
        for f in &t.fields {
            let base = f.display(Lang::En);
            let label = base.to_lowercase();
            match cols.iter_mut().find(|(l, c)| *l == label && c.kind == f.kind) {
                Some((_, c)) => {
                    c.members.insert(t.ticket_type.id, f.clone());
                }
                None => cols.push((label, Col { header: base, kind: f.kind, members: HashMap::from([(t.ticket_type.id, f.clone())]) })),
            }
        }
    }
    let mut used: HashSet<String> = FIXED_COLUMNS.iter().map(|c| c.to_lowercase()).collect();
    used.insert(SPRINT_COLUMN.to_lowercase());
    let mut out = Vec::new();
    for (_, mut c) in cols {
        let base = c.header.clone();
        let mut header = base.clone();
        if used.contains(&header.to_lowercase()) {
            header = format!("{base} [{}]", c.kind.as_str());
        }
        let mut n = 2;
        while used.contains(&header.to_lowercase()) {
            header = format!("{base} [{}] ({n})", c.kind.as_str());
            n += 1;
        }
        used.insert(header.to_lowercase());
        c.header = header;
        out.push(c);
    }
    out
}

// ------------------------------------------------------------------ Export

fn bool_text(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Namen der Versionen und Meilensteine des Projekts (nach Kennung).
#[derive(Default)]
struct Releases {
    versions: Vec<(i64, String)>,
    milestones: Vec<(i64, String)>,
}

impl Releases {
    fn load(conn: &Connection, project_id: i64) -> Result<Releases, BackupError> {
        let read = |table: &str| -> Result<Vec<(i64, String)>, BackupError> {
            let mut st = conn.prepare(&format!("SELECT id, name FROM {table} WHERE project_id = ?1 ORDER BY position, id"))?;
            let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        };
        Ok(Releases { versions: read("versions")?, milestones: read("milestones")? })
    }

    fn list(&self, kind: FieldKind) -> &[(i64, String)] {
        if kind == FieldKind::Version { &self.versions } else { &self.milestones }
    }

    fn name(&self, kind: FieldKind, id: i64) -> Option<&str> {
        self.list(kind).iter().find(|(i, _)| *i == id).map(|(_, n)| n.as_str())
    }

    fn id(&self, kind: FieldKind, name: &str) -> Option<i64> {
        let want = name.trim().to_lowercase();
        self.list(kind).iter().find(|(_, n)| n.to_lowercase() == want).map(|(i, _)| *i)
    }
}

fn render(field: &Field, v: &Value, key_of: &HashMap<i64, String>, releases: &Releases) -> String {
    let option = |id: i64| field.options.values.iter().find(|o| o.id == id).map(|o| o.display(Lang::En)).unwrap_or_else(|| format!("#{id}"));
    match (field.kind, v) {
        (FieldKind::Checkbox, v) => bool_text(v.as_bool().unwrap_or(false)).to_string(),
        (FieldKind::Dropdown, v) => option(v.as_i64().unwrap_or(0)),
        (FieldKind::Multi, Value::Array(a)) => join_list(&a.iter().map(|x| option(x.as_i64().unwrap_or(0))).collect::<Vec<_>>()),
        (FieldKind::Labels, Value::Array(a)) => join_list(&a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect::<Vec<_>>()),
        (FieldKind::Ticket, v) => v.as_i64().and_then(|id| key_of.get(&id).cloned()).unwrap_or_default(),
        (FieldKind::Version | FieldKind::Milestone, v) => v.as_i64().and_then(|id| releases.name(field.kind, id)).unwrap_or_default().to_string(),
        (_, Value::String(s)) => s.clone(),
        (_, other) => other.to_string(),
    }
}

/// Exportiert alle Tickets des Projekts (in der Reihenfolge des Backlogs) im oben beschriebenen Format.
pub fn export_tickets_csv(conn: &Connection, project_id: i64, opts: &CsvOptions) -> Result<String, BackupError> {
    let project = editable(conn, project_id)?;
    let sum = project_summary(conn, project_id)?;
    let cols = columns(&sum);
    let releases = Releases::load(conn, project_id)?;
    // Offener Sprint je Ticket (nur bei Projekten mit Sprints)
    let mut sprint_of: HashMap<i64, String> = HashMap::new();
    if project.use_sprints {
        let mut st = conn.prepare(
            "SELECT st.ticket_id, s.name FROM sprint_tickets st JOIN sprints s ON s.id = st.sprint_id WHERE s.project_id = ?1 AND st.removed_at IS NULL AND s.state IN ('planned', 'active')",
        )?;
        for row in st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (t, n) = row?;
            sprint_of.insert(t, n);
        }
    }
    let tickets = {
        let mut st = conn.prepare(&format!("SELECT {} {} WHERE t.project_id = ?1 ORDER BY t.rank, t.number", crate::planning::tickets::COLS, crate::planning::tickets::FROM))?;
        let rows = st.query_map([project_id], crate::planning::tickets::from_row)?.collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let key_of: HashMap<i64, String> = tickets.iter().map(|t| (t.id, t.key())).collect();
    let mut values: HashMap<i64, BTreeMap<i64, (FieldKind, Vec<Row>)>> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT v.ticket_id, v.field_id, f.kind, v.text, v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
             WHERE t.project_id = ?1 ORDER BY v.ticket_id, v.field_id, v.pos",
        )?;
        let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<f64>>(4)?)))?;
        for row in rows {
            let (tid, fid, kind, text, num) = row?;
            let kind = FieldKind::parse(&kind).unwrap_or(FieldKind::Text);
            values.entry(tid).or_default().entry(fid).or_insert_with(|| (kind, Vec::new())).1.push((text, num));
        }
    }
    let cell = |s: String| if opts.excel_safe { guard(&s) } else { s };
    let sprint_header: Option<&str> = project.use_sprints.then_some(SPRINT_COLUMN);
    let mut table = CsvTable::new(FIXED_COLUMNS.iter().copied().chain(sprint_header).map(|h| cell(h.to_string())).chain(cols.iter().map(|c| cell(c.header.clone()))));
    for t in &tickets {
        let ty = sum.type_by_id(t.type_id);
        let mut row: Vec<String> = vec![
            t.key(),
            ty.map(|x| x.ticket_type.display(Lang::En)).unwrap_or_default(),
            t.title.clone(),
            t.status_id.and_then(|s| sum.status(s)).map(|s| s.display(Lang::En)).unwrap_or_default(),
            t.parent_id.and_then(|p| key_of.get(&p).cloned()).unwrap_or_default(),
            bool_text(t.flagged).to_string(),
            bool_text(t.starred).to_string(),
            t.created_at.clone(),
            t.updated_at.clone(),
        ];
        if project.use_sprints {
            row.push(sprint_of.get(&t.id).cloned().unwrap_or_default());
        }
        let mine = values.remove(&t.id).unwrap_or_default();
        for c in &cols {
            let text = c
                .members
                .get(&t.type_id)
                .and_then(|f| mine.get(&f.id).and_then(|(kind, rows)| decode(*kind, rows)).map(|v| render(f, &v, &key_of, &releases)))
                .unwrap_or_default();
            row.push(text);
        }
        table.push_row(row.into_iter().map(&cell));
    }
    write_csv(&table, &WriteOptions { delimiter: opts.delimiter, bom: opts.bom, ..Default::default() }).map_err(BackupError::Csv)
}

// ------------------------------------------------------------------ Import

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CsvImportReport {
    /// Zahl der angelegten Tickets.
    pub created: usize,
    /// (Schlüssel in der Datei, neuer Schlüssel) in Dateireihenfolge; der alte ist leer, wenn die Zelle leer war.
    pub keys: Vec<(String, String)>,
    pub warnings: Vec<Warning>,
}

/// Ein Wert, wie er aus einer Zelle gelesen wurde.
enum Cell {
    Value(Value),
    /// Schlüssel eines Tickets in der Datei (wird erst nach dem Anlegen aufgelöst).
    TicketKey(String),
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn option_id(field: &Field, name: &str) -> Option<i64> {
    let want = name.trim().to_lowercase();
    field.options.values.iter().find(|o| labels(o.key.as_deref(), o.name.as_deref()).contains(&want)).map(|o| o.id)
}

fn parse_cell(field: &Field, cell: &str, releases: &Releases) -> Result<Cell, RowIssue> {
    let invalid = || RowIssue::CellInvalid(cell.to_string());
    Ok(match field.kind {
        FieldKind::Text | FieldKind::Rich | FieldKind::Terminal | FieldKind::Date | FieldKind::Datetime => Cell::Value(Value::String(cell.to_string())),
        FieldKind::Checkbox => Cell::Value(Value::Bool(parse_bool(cell).ok_or_else(|| RowIssue::BoolInvalid(cell.to_string()))?)),
        FieldKind::Number => {
            let x: f64 = cell.trim().parse().map_err(|_| invalid())?;
            if !x.is_finite() {
                return Err(invalid());
            }
            Cell::Value(crate::planning::values::canon_number(x))
        }
        FieldKind::Dropdown => Cell::Value(Value::from(option_id(field, cell).ok_or_else(|| RowIssue::OptionUnknown(cell.to_string()))?)),
        FieldKind::Multi => {
            let mut ids = Vec::new();
            for part in split_list(cell) {
                ids.push(Value::from(option_id(field, &part).ok_or_else(|| RowIssue::OptionUnknown(part.clone()))?));
            }
            Cell::Value(Value::Array(ids))
        }
        FieldKind::Labels => Cell::Value(Value::Array(split_list(cell).into_iter().map(Value::String).collect())),
        FieldKind::Ticket => {
            let (k, n) = parse_key(cell).ok_or_else(invalid)?;
            Cell::TicketKey(format!("{k}-{n}"))
        }
        FieldKind::Version | FieldKind::Milestone => Cell::Value(Value::from(releases.id(field.kind, cell).ok_or_else(|| RowIssue::ReleaseUnknown(cell.to_string()))?)),
    })
}

/// Eine geprüfte Zeile.
struct ParsedRow {
    /// Zeilennummer der Tabelle (Kopfzeile = 1).
    row: usize,
    old_key: Option<String>,
    type_id: i64,
    title: String,
    status: Option<i64>,
    parent_key: Option<String>,
    flagged: bool,
    starred: bool,
    created: Option<String>,
    updated: Option<String>,
    /// (Feld, Wert); `Null` = bewusst leer.
    values: Vec<(i64, Value)>,
    /// (Feld, Schlüssel des Ziel-Tickets, Spaltenüberschrift)
    ticket_values: Vec<(Field, String, String)>,
    /// Sprint (Kennung), in den das Ticket kommt.
    sprint: Option<i64>,
}

struct Problems(Vec<RowProblem>);

impl Problems {
    fn add(&mut self, row: usize, column: &str, issue: RowIssue) {
        if self.0.len() < MAX_ROW_PROBLEMS {
            self.0.push(RowProblem { row, column: column.to_string(), issue });
        }
    }

    fn full(&self) -> bool {
        self.0.len() >= MAX_ROW_PROBLEMS
    }
}

fn status_of<'a>(statuses: &'a [Status], name: &str) -> Option<&'a Status> {
    statuses.iter().find(|s| same_label(None, Some(name), s.key.as_deref(), s.name.as_deref()))
}

/// Liest Tickets im Format von [`export_tickets_csv`] und legt sie im Projekt **neu** an (siehe Moduldoku).
pub fn import_tickets_csv(conn: &Connection, project_id: i64, text: &str, opts: &CsvImportOptions) -> Result<CsvImportReport, BackupError> {
    import_tickets_csv_with(conn, project_id, text, opts, &Progress::default())
}

/// Wie [`import_tickets_csv`], mit Fortschritt und Abbruch: `progress` zählt die Schritte (drei Läufe über die Zeilen: prüfen, anlegen,
/// nachbearbeiten) und bricht mit [`BackupError::Cancelled`] ab, wenn es verlangt wird – nach dem Anlegen wird dann alles zurückgenommen.
pub fn import_tickets_csv_with(conn: &Connection, project_id: i64, text: &str, opts: &CsvImportOptions, progress: &Progress) -> Result<CsvImportReport, BackupError> {
    editable(conn, project_id)?;
    let delimiter = detect_delimiter(text);
    let mut table = read_csv(text, delimiter)?;
    progress.set_total(table.rows.len() * CSV_PASSES);
    progress.check()?;
    if opts.unprotect {
        table.headers.iter_mut().for_each(|h| *h = unguard(h).to_string());
        table.rows.iter_mut().flatten().for_each(|c| *c = unguard(c).to_string());
    }
    // Kopfzeile
    for (i, fixed) in FIXED_COLUMNS.iter().enumerate() {
        if table.headers.get(i).map(String::as_str) != Some(*fixed) {
            return Err(BackupError::CsvHeader(HeaderProblem::Fixed { column: i + 1, expected: fixed.to_string(), found: table.headers.get(i).cloned().unwrap_or_default() }));
        }
    }
    let project = editable(conn, project_id)?;
    let sum = project_summary(conn, project_id)?;
    let cols = columns(&sum);
    // Die optionale Spalte „Sprint“ steht an zehnter Stelle und nur bei Projekten mit Sprints.
    let has_sprint_col = table.headers.get(FIXED_COLUMNS.len()).map(String::as_str) == Some(SPRINT_COLUMN);
    if has_sprint_col && !project.use_sprints {
        return Err(BackupError::CsvHeader(HeaderProblem::Unknown { column: FIXED_COLUMNS.len() + 1, name: SPRINT_COLUMN.to_string() }));
    }
    let first_field = FIXED_COLUMNS.len() + has_sprint_col as usize;
    let sprints: Vec<(i64, String)> = {
        let mut st = conn.prepare("SELECT id, name FROM sprints WHERE project_id = ?1 ORDER BY id")?;
        let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let releases = Releases::load(conn, project_id)?;
    let mut col_of: Vec<usize> = Vec::new();
    for (i, h) in table.headers.iter().enumerate().skip(first_field) {
        let Some(ci) = cols.iter().position(|c| c.header == *h) else {
            return Err(BackupError::CsvHeader(HeaderProblem::Unknown { column: i + 1, name: h.clone() }));
        };
        if col_of.contains(&ci) {
            return Err(BackupError::CsvHeader(HeaderProblem::Duplicate { column: i + 1, name: h.clone() }));
        }
        col_of.push(ci);
    }

    // Zeilen prüfen
    let types: Vec<_> = sum.types.iter().filter(|t| !t.ticket_type.system).collect();
    let mut problems = Problems(Vec::new());
    let mut rows: Vec<ParsedRow> = Vec::new();
    let mut key_row: HashMap<String, usize> = HashMap::new();
    for (ri, r) in table.rows.iter().enumerate() {
        if problems.full() {
            break;
        }
        progress.step()?;
        let row = ri + 2;
        let before = problems.0.len();
        let get = |i: usize| r[i].as_str();
        let old_key = match get(0).trim() {
            "" => None,
            k => match parse_key(k) {
                Some((p, n)) => {
                    let key = format!("{p}-{n}");
                    if key_row.insert(key.clone(), rows.len()).is_some() {
                        problems.add(row, FIXED_COLUMNS[0], RowIssue::KeyDuplicate(key.clone()));
                    }
                    Some(key)
                }
                None => {
                    problems.add(row, FIXED_COLUMNS[0], RowIssue::KeyInvalid(k.to_string()));
                    None
                }
            },
        };
        let ty = match get(1).trim() {
            "" => {
                problems.add(row, FIXED_COLUMNS[1], RowIssue::TypeMissing);
                None
            }
            n => match types.iter().find(|t| same_label(None, Some(n), t.ticket_type.key.as_deref(), t.ticket_type.name.as_deref())) {
                Some(t) => Some(*t),
                None => {
                    problems.add(row, FIXED_COLUMNS[1], RowIssue::TypeUnknown(n.to_string()));
                    None
                }
            },
        };
        let title = match clean_title(get(2)) {
            Ok(t) => t,
            Err(e) => {
                problems.add(row, FIXED_COLUMNS[2], RowIssue::Ticket(e));
                String::new()
            }
        };
        let status = match get(3).trim() {
            "" => None,
            n => match status_of(&sum.statuses, n) {
                None => {
                    problems.add(row, FIXED_COLUMNS[3], RowIssue::StatusUnknown(n.to_string()));
                    None
                }
                Some(s) => match ty {
                    Some(t) if t.workflow_statuses.contains(&s.id) => Some(s.id),
                    Some(_) => {
                        problems.add(row, FIXED_COLUMNS[3], RowIssue::StatusNotAllowed(n.to_string()));
                        None
                    }
                    None => None,
                },
            },
        };
        let parent_key = match get(4).trim() {
            "" => None,
            k => match parse_key(k) {
                Some((p, n)) => Some(format!("{p}-{n}")),
                None => {
                    problems.add(row, FIXED_COLUMNS[4], RowIssue::KeyInvalid(k.to_string()));
                    None
                }
            },
        };
        let flag = |i: usize, problems: &mut Problems| match get(i).trim() {
            "" => false,
            v => parse_bool(v).unwrap_or_else(|| {
                problems.add(row, FIXED_COLUMNS[i], RowIssue::BoolInvalid(v.to_string()));
                false
            }),
        };
        let flagged = flag(5, &mut problems);
        let starred = flag(6, &mut problems);
        let ts = |i: usize, problems: &mut Problems| match get(i).trim() {
            "" => None,
            v if valid_timestamp(v) => Some(v.to_string()),
            v => {
                problems.add(row, FIXED_COLUMNS[i], RowIssue::TimestampInvalid(v.to_string()));
                None
            }
        };
        let (created, updated) = (ts(7, &mut problems), ts(8, &mut problems));
        let sprint = if has_sprint_col {
            match get(FIXED_COLUMNS.len()).trim() {
                "" => None,
                n => match sprints.iter().find(|(_, s)| s.to_lowercase() == n.to_lowercase()) {
                    Some((id, _)) => Some(*id),
                    None => {
                        problems.add(row, SPRINT_COLUMN, RowIssue::SprintUnknown(n.to_string()));
                        None
                    }
                },
            }
        } else {
            None
        };
        // Feldwerte
        let mut values = Vec::new();
        let mut ticket_values = Vec::new();
        if let Some(t) = ty {
            for (k, ci) in col_of.iter().enumerate() {
                let header = &table.headers[first_field + k];
                let cell = get(first_field + k);
                let Some(field) = cols[*ci].members.get(&t.ticket_type.id) else {
                    if !cell.is_empty() {
                        problems.add(row, header, RowIssue::FieldNotInType(t.ticket_type.display(Lang::En)));
                    }
                    continue;
                };
                if cell.is_empty() {
                    values.push((field.id, Value::Null));
                    continue;
                }
                match parse_cell(field, cell, &releases) {
                    Ok(Cell::Value(v)) => values.push((field.id, v)),
                    Ok(Cell::TicketKey(k)) => {
                        values.push((field.id, Value::Null));
                        ticket_values.push((field.clone(), k, header.clone()));
                    }
                    Err(issue) => problems.add(row, header, issue),
                }
            }
        }
        if problems.0.len() == before {
            if let Some(t) = ty {
                rows.push(ParsedRow { row, old_key, type_id: t.ticket_type.id, title, status, parent_key, flagged, starred, created, updated, values, ticket_values, sprint });
            }
        } else if let Some(k) = &old_key {
            // Ein fehlerhaftes Ticket soll Verweise darauf nicht zusätzlich als „fehlt“ melden.
            key_row.entry(k.clone()).or_insert(usize::MAX);
        }
    }
    // Verweise innerhalb der Datei
    for r in &rows {
        if let Some(p) = &r.parent_key {
            if !key_row.contains_key(p) {
                problems.add(r.row, FIXED_COLUMNS[4], RowIssue::ParentUnknown(p.clone()));
            }
        }
        for (_, k, header) in &r.ticket_values {
            if !key_row.contains_key(k) {
                problems.add(r.row, header, RowIssue::TicketRefUnknown(k.clone()));
            }
        }
    }
    if !problems.0.is_empty() {
        return Err(BackupError::CsvRows(problems.0));
    }

    // Anlegen (alles oder nichts)
    conn.execute_batch("SAVEPOINT csv_import")?;
    match create_all(conn, project_id, &rows, &key_row, progress) {
        Ok(report) => {
            conn.execute_batch("RELEASE csv_import")?;
            Ok(report)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO csv_import; RELEASE csv_import");
            Err(e)
        }
    }
}

/// So viele Läufe über die Zeilen macht der CSV-Import (prüfen, anlegen, nachbearbeiten); jeder zählt einen Schritt je Zeile.
const CSV_PASSES: usize = 3;

fn create_all(conn: &Connection, project_id: i64, rows: &[ParsedRow], key_row: &HashMap<String, usize>, progress: &Progress) -> Result<CsvImportReport, BackupError> {
    let source = Source::import();
    let mut ids: Vec<Option<i64>> = vec![None; rows.len()];
    let mut problems = Problems(Vec::new());
    // Eltern zuerst; sonst in Dateireihenfolge
    loop {
        let mut progress_made = false;
        for (i, r) in rows.iter().enumerate() {
            if ids[i].is_some() {
                continue;
            }
            let parent = match &r.parent_key {
                None => None,
                Some(k) => match key_row.get(k).and_then(|j| ids.get(*j).copied().flatten()) {
                    Some(id) => Some(id),
                    None => continue,
                },
            };
            progress_made = true;
            progress.step()?;
            let new = NewTicket { project_id, type_id: r.type_id, title: r.title.clone(), parent_id: parent, values: r.values.clone() };
            let note = Some(match &r.old_key {
                Some(k) => format!("import:{k}"),
                None => "import".to_string(),
            });
            match create_ticket_noted(conn, &new, &source, note) {
                Ok((t, _)) => ids[i] = Some(t.id),
                Err(e) => {
                    problems.add(r.row, "", RowIssue::Ticket(e));
                    // Als „angelegt“ markieren, damit Kinder nicht zusätzlich scheitern.
                    ids[i] = Some(0);
                }
            }
        }
        if !progress_made {
            break;
        }
    }
    for (i, r) in rows.iter().enumerate() {
        if ids[i].is_none() {
            problems.add(r.row, FIXED_COLUMNS[4], RowIssue::ParentCycle);
        }
    }
    if !problems.0.is_empty() {
        return Err(BackupError::CsvRows(problems.0));
    }
    // Status, Markierungen, Verweise auf andere Tickets, Zeitstempel und Reihenfolge
    let now = common::now();
    let first_rank: i64 = ids.iter().flatten().filter_map(|id| conn.query_row("SELECT rank FROM tickets WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).ok()).min().unwrap_or(0);
    let mut ranks = Vec::new();
    let mut report = CsvImportReport::default();
    for (i, r) in rows.iter().enumerate() {
        progress.step()?;
        let id = ids[i].expect("angelegt");
        if let Some(s) = r.status {
            conn.execute("UPDATE tickets SET status_id = ?1 WHERE id = ?2", [s, id])?;
        }
        for (field, key, header) in &r.ticket_values {
            let target = key_row.get(key).and_then(|j| ids[*j]).expect("geprüft");
            let ctx = RefCtx { project_id, ticket_id: Some(id) };
            match normalize_value(conn, &ctx, field, &Value::from(target)) {
                Ok(Some(v)) => write_value(conn, id, field, Some(&v)).map_err(|e| BackupError::CsvRows(vec![RowProblem { row: r.row, column: header.clone(), issue: RowIssue::Ticket(e) }]))?,
                Ok(None) => {}
                Err(e) => return Err(BackupError::CsvRows(vec![RowProblem { row: r.row, column: header.clone(), issue: RowIssue::Ticket(e) }])),
            }
        }
        let created = r.created.clone().unwrap_or_else(|| now.clone());
        let updated = r.updated.clone().or_else(|| r.created.clone()).unwrap_or_else(|| now.clone());
        conn.execute("UPDATE tickets SET flagged = ?1, starred = ?2, created_at = ?3, updated_at = ?4 WHERE id = ?5", params![r.flagged as i64, r.starred as i64, created, updated, id])?;
        ranks.push((id, first_rank + i as i64));
        let new_key: String = conn.query_row("SELECT p.key || '-' || t.number FROM tickets t JOIN projects p ON p.id = t.project_id WHERE t.id = ?1", [id], |r| r.get(0))?;
        report.keys.push((r.old_key.clone().unwrap_or_default(), new_key));
    }
    raw_set_ranks(conn, &ranks).map_err(|e: TicketError| BackupError::Ticket(e))?;
    // Sprints zuletzt: Punkte und Werte der Tickets stehen dann fest (`points_at_add`)
    for (i, r) in rows.iter().enumerate() {
        if let Some(sprint) = r.sprint {
            if let Err(e) = move_tickets_to_sprint(conn, sprint, &[ids[i].expect("angelegt")]) {
                problems.add(r.row, SPRINT_COLUMN, RowIssue::Sprint(e));
            }
        }
    }
    if !problems.0.is_empty() {
        return Err(BackupError::CsvRows(problems.0));
    }
    report.created = rows.len();
    Ok(report)
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn guard_and_unguard_are_inverse() {
        for s in ["=1+1", "+x", "-5", "@a", "'=x", "''-1", "plain", "'plain", "", "'", "a=b", "-", "\t=cmd", "\r=cmd", "'\tx", "x\t=1"] {
            assert_eq!(unguard(&guard(s)), s, "{s:?}");
        }
        // OWASP: auch Zellen, die mit Tabulator oder Wagenrücklauf beginnen, können in Excel eine Formel einleiten
        assert_eq!(guard("\t=1+1"), "'\t=1+1");
        assert_eq!(guard("\r=1+1"), "'\r=1+1");
        assert_eq!(guard("a\t=1"), "a\t=1", "nur der Anfang zählt");
        assert_eq!(guard("=A1"), "'=A1");
        assert_eq!(guard("'=A1"), "''=A1");
        assert_eq!(guard("x=1"), "x=1");
        assert_eq!(guard("'x"), "'x");
        assert_eq!(unguard("'=A1"), "=A1");
        assert_eq!(unguard("'abc"), "'abc", "ein Apostroph ohne Formelzeichen bleibt");
    }

    #[test]
    fn lists_roundtrip_with_escapes() {
        for items in [vec!["a", "b"], vec!["a|b", "c"], vec!["x\\y", "|", "\\|"], vec!["ein"], vec!["", "a"]] {
            let items: Vec<String> = items.into_iter().map(String::from).collect();
            assert_eq!(split_list(&join_list(&items)), items);
        }
        assert_eq!(join_list(&["a|b".to_string(), "c\\".to_string()]), "a\\|b|c\\\\");
        assert_eq!(split_list("a\\xb"), vec!["a\\xb".to_string()], "lose Rückstriche bleiben");
    }
}
