//! Datenbankpflege der Wartungsseite: Integrität prüfen (SQLite `integrity_check`/`quick_check`, `foreign_key_check`, eigene
//! Prüfungen der Zähler), Reparatur nur eindeutiger Fälle (verwaiste Zeilen, falsche Zähler, Indizes) mit Vorschau, Verdichten
//! (VACUUM + ANALYZE + REINDEX), Statistik und die Rettung einer beschädigten Datenbank **in eine neue Datei** (Prinzip von
//! `.recover`: Zeile für Zeile lesen, was lesbar ist; die beschädigte Datei bleibt unberührt, ein Bericht nennt Gerettetes
//! und Verlorenes).
//!
//! Prüfen und Statistik lesen nur. Reparatur, Verdichten und Rettung schreiben (Rettung nur in die neue Datei); davor legt
//! die Oberfläche eine Sicherung „maintenance“ an.

use super::{Check, DbScope, Finding, Fix, MRes, MaintError, Severity, Target};
use crate::state::Lang;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Höchstzahl Meldungen von `integrity_check`.
const MAX_INTEGRITY_LINES: usize = 50;

// ------------------------------------------------------------------ Prüfen

/// Eine Fremdschlüssel-Beziehung: Kind-Tabelle, Spalte, Eltern-Tabelle, Eltern-Spalte.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FkRef {
    pub table: String,
    pub column: String,
    pub parent: String,
    pub parent_column: String,
}

/// Was die Reparatur mit verwaisten Zeilen einer Beziehung macht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanFix {
    /// Zeile löschen (sie ist ohne ihren Eltern-Eintrag bedeutungslos, z. B. ein Kommentar ohne Ticket).
    Delete,
    /// Verweis leeren (wie `ON DELETE SET NULL`, z. B. Eltern-Ticket).
    SetNull,
    /// Nicht eindeutig (z. B. Ticket ohne Typ): nur melden.
    Manual,
}

/// Wie eine verwaiste Zeile eindeutig repariert wird – nach der Fremdschlüssel-Regel der Spalte selbst.
pub fn orphan_fix(conn: &Connection, r: &FkRef) -> OrphanFix {
    // ON DELETE der Spalte ablesen (`PRAGMA foreign_key_list`): CASCADE → löschen, SET NULL → leeren, sonst manuell.
    let Ok(mut st) = conn.prepare(&format!("PRAGMA foreign_key_list({})", quote_ident(&r.table))) else { return OrphanFix::Manual };
    let rows = st.query_map([], |x| Ok((x.get::<_, String>(2)?, x.get::<_, String>(3)?, x.get::<_, String>(6)?)));
    if let Ok(rows) = rows {
        for (parent, from, on_delete) in rows.flatten() {
            if parent == r.parent && from == r.column {
                return match on_delete.to_ascii_uppercase().as_str() {
                    "CASCADE" => OrphanFix::Delete,
                    "SET NULL" => OrphanFix::SetNull,
                    _ => OrphanFix::Manual,
                };
            }
        }
    }
    OrphanFix::Manual
}

pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Verwaiste Zeilen je Beziehung (über `PRAGMA foreign_key_check`, auch für Tabellen ohne rowid).
pub fn orphans(conn: &Connection) -> rusqlite::Result<BTreeMap<FkRef, i64>> {
    let mut out: BTreeMap<FkRef, i64> = BTreeMap::new();
    let mut st = conn.prepare("PRAGMA foreign_key_check")?;
    let rows: Vec<(String, String, i64)> = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?.collect::<Result<_, _>>()?;
    // fkid → Spalte je Tabelle
    let mut cache: BTreeMap<String, Vec<(i64, String, String, String)>> = BTreeMap::new();
    for (table, parent, fkid) in rows {
        let list = match cache.get(&table) {
            Some(l) => l.clone(),
            None => {
                let mut st = conn.prepare(&format!("PRAGMA foreign_key_list({})", quote_ident(&table)))?;
                let l: Vec<(i64, String, String, String)> = st
                    .query_map([], |x| Ok((x.get::<_, i64>(0)?, x.get::<_, String>(2)?, x.get::<_, String>(3)?, x.get::<_, Option<String>>(4)?.unwrap_or_else(|| "id".into()))))?
                    .collect::<Result<_, _>>()?;
                cache.insert(table.clone(), l.clone());
                l
            }
        };
        if let Some((_, p, from, to)) = list.iter().find(|(id, p, _, _)| *id == fkid && *p == parent) {
            *out.entry(FkRef { table: table.clone(), column: from.clone(), parent: p.clone(), parent_column: to.clone() }).or_default() += 1;
        }
    }
    Ok(out)
}

/// Ein falscher Zähler.
#[derive(Debug, Clone, PartialEq)]
pub enum Counter {
    /// `projects.next_number` ist nicht größer als die höchste Ticketnummer.
    ProjectNumber { project_id: i64, key: String, current: i64, needed: i64 },
    /// `meta.history_batch` liegt unter der höchsten Aktions-Nummer im Verlauf.
    HistoryBatch { current: i64, needed: i64 },
    /// `sqlite_sequence` liegt unter der höchsten Kennung (AUTOINCREMENT würde Kennungen wiederverwenden).
    Sequence { table: String, current: i64, needed: i64 },
}

pub fn counters(conn: &Connection) -> rusqlite::Result<Vec<Counter>> {
    let mut out = Vec::new();
    let mut st = conn.prepare("SELECT p.id, p.key, p.next_number, (SELECT MAX(number) FROM tickets t WHERE t.project_id = p.id) FROM projects p")?;
    for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<i64>>(3)?)))? {
        let (project_id, key, current, max) = r?;
        if let Some(m) = max.filter(|m| current <= *m) {
            out.push(Counter::ProjectNumber { project_id, key, current, needed: m + 1 });
        }
    }
    let cur: i64 = conn.query_row("SELECT value FROM meta WHERE key = 'history_batch'", [], |r| r.get::<_, String>(0)).optional()?.and_then(|v| v.parse().ok()).unwrap_or(0);
    let max: i64 = conn.query_row("SELECT COALESCE(MAX(batch), 0) FROM history", [], |r| r.get(0))?;
    if max > cur {
        out.push(Counter::HistoryBatch { current: cur, needed: max });
    }
    let mut st = conn.prepare("SELECT name, seq FROM sqlite_sequence")?;
    let seqs: Vec<(String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (table, seq) in seqs {
        let exists: Option<String> = conn.query_row("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1", [&table], |r| r.get(0)).optional()?;
        if exists.is_none() {
            continue;
        }
        let max: Option<i64> = conn.query_row(&format!("SELECT MAX(rowid) FROM {}", quote_ident(&table)), [], |r| r.get(0))?;
        if let Some(m) = max.filter(|m| *m > seq) {
            out.push(Counter::Sequence { table, current: seq, needed: m });
        }
    }
    Ok(out)
}

impl Counter {
    pub fn text(&self, lang: Lang) -> String {
        match (self, lang) {
            (Counter::ProjectNumber { key, current, needed, .. }, Lang::De) => format!("Projekt {key}: nächste Ticketnummer {current}, nötig mindestens {needed} (sonst doppelte Nummern)."),
            (Counter::ProjectNumber { key, current, needed, .. }, Lang::En) => format!("Project {key}: next ticket number {current}, needs at least {needed} (otherwise numbers repeat)."),
            (Counter::HistoryBatch { current, needed }, Lang::De) => format!("Zähler der Verlaufsaktionen {current}, nötig mindestens {needed}."),
            (Counter::HistoryBatch { current, needed }, Lang::En) => format!("History action counter is {current}, needs at least {needed}."),
            (Counter::Sequence { table, current, needed }, Lang::De) => format!("Schlüsselfolge von {table} steht auf {current}, höchste Kennung ist {needed}."),
            (Counter::Sequence { table, current, needed }, Lang::En) => format!("Key sequence of {table} is {current}, the highest id is {needed}."),
        }
    }
}

/// Meldungen von `integrity_check` bzw. `quick_check` (leer = in Ordnung).
pub fn integrity(conn: &Connection, scope: DbScope) -> rusqlite::Result<Vec<String>> {
    let pragma = match scope {
        DbScope::Quick => "quick_check",
        DbScope::Full => "integrity_check",
    };
    let mut st = conn.prepare(&format!("PRAGMA {pragma}({MAX_INTEGRITY_LINES})"))?;
    let v: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(if v.len() == 1 && v[0] == "ok" { Vec::new() } else { v })
}

/// Erklärt eine Meldung von `integrity_check` in einfachen Worten.
pub fn explain_integrity(line: &str, lang: Lang) -> String {
    let l = line.to_ascii_lowercase();
    let plain = if l.contains("index") {
        lang.t("Ein Suchindex passt nicht zu den Daten – lässt sich durch Neuaufbau (Reparatur) beheben.", "A search index does not match the data – rebuilding it (repair) fixes this.")
    } else if l.contains("freelist") || l.contains("never used") {
        lang.t("Freier Speicher ist falsch verbucht – Verdichten behebt das meist.", "Free space is accounted wrongly – compacting usually fixes this.")
    } else {
        lang.t("Die Datei ist beschädigt – letzte Sicherung wiederherstellen oder die Daten in eine neue Datei retten.", "The file is damaged – restore the last backup or rescue the data into a new file.")
    };
    format!("{plain} ({line})")
}

/// Datenbank-Befunde für den Bericht.
pub fn findings(conn: &Connection, scope: DbScope, lang: Lang) -> MRes<Vec<Finding>> {
    let mut out = Vec::new();
    let mk = |check: Check, id: String, message: String, fixes: Vec<Fix>, detail: serde_json::Value| Finding { id, check, severity: Severity::Error, ticket: None, other: None, target: Target::Database, message, fixes, detail };
    match integrity(conn, scope) {
        Ok(lines) => {
            let index_only = !lines.is_empty() && lines.iter().all(|l| l.to_ascii_lowercase().contains("index"));
            for (i, l) in lines.iter().enumerate() {
                out.push(mk(Check::DbIntegrity, format!("db_integrity:{i}"), explain_integrity(l, lang), if index_only { vec![Fix::Repair] } else { vec![] }, json!({"sqlite": l})));
            }
        }
        Err(e) => out.push(mk(Check::DbIntegrity, "db_integrity:unreadable".into(), explain_integrity(&e.to_string(), lang), vec![], json!({"sqlite": e.to_string()}))),
    }
    for (r, n) in orphans(conn)? {
        let fix = orphan_fix(conn, &r);
        let check = if r.table == "history" { Check::DbHistory } else { Check::DbForeignKey };
        let what = match (fix, lang) {
            (OrphanFix::Delete, Lang::De) => "Reparatur löscht sie",
            (OrphanFix::Delete, Lang::En) => "repair deletes them",
            (OrphanFix::SetNull, Lang::De) => "Reparatur leert den Verweis",
            (OrphanFix::SetNull, Lang::En) => "repair clears the reference",
            (OrphanFix::Manual, Lang::De) => "nicht eindeutig, bitte Sicherung wiederherstellen",
            (OrphanFix::Manual, Lang::En) => "not unambiguous, please restore a backup",
        };
        let message = match lang {
            Lang::De => format!("{n} Zeile(n) in {} verweisen über {} auf fehlende Einträge in {} ({what}).", r.table, r.column, r.parent),
            Lang::En => format!("{n} row(s) in {} refer via {} to missing entries in {} ({what}).", r.table, r.column, r.parent),
        };
        let fixes = if fix == OrphanFix::Manual { vec![] } else { vec![Fix::Repair] };
        out.push(mk(check, format!("{}:{}.{}", check.as_str(), r.table, r.column), message, fixes, json!({"table": r.table, "column": r.column, "parent": r.parent, "rows": n})));
    }
    for c in counters(conn)? {
        let id = match &c {
            Counter::ProjectNumber { key, .. } => format!("db_counter:project:{key}"),
            Counter::HistoryBatch { .. } => "db_counter:history_batch".into(),
            Counter::Sequence { table, .. } => format!("db_counter:seq:{table}"),
        };
        out.push(mk(Check::DbCounter, id, c.text(lang), vec![Fix::Repair], json!({})));
    }
    Ok(out)
}

// ------------------------------------------------------------------ Reparatur

/// Ein geplanter Reparaturschritt (Vorschau).
#[derive(Debug, Clone, PartialEq)]
pub enum RepairStep {
    Orphans { r: FkRef, rows: i64, fix: OrphanFix },
    Counter(Counter),
    Reindex,
}

impl RepairStep {
    pub fn text(&self, lang: Lang) -> String {
        match self {
            RepairStep::Orphans { r, rows, fix: OrphanFix::Delete } => match lang {
                Lang::De => format!("{rows} verwaiste Zeile(n) aus {} löschen (ohne Eintrag in {}).", r.table, r.parent),
                Lang::En => format!("Delete {rows} orphaned row(s) from {} (no entry in {}).", r.table, r.parent),
            },
            RepairStep::Orphans { r, rows, fix: OrphanFix::SetNull } => match lang {
                Lang::De => format!("In {rows} Zeile(n) von {} den Verweis {} leeren.", r.table, r.column),
                Lang::En => format!("Clear the reference {} in {rows} row(s) of {}.", r.column, r.table),
            },
            RepairStep::Orphans { r, rows, .. } => match lang {
                Lang::De => format!("{rows} Zeile(n) in {} (nicht eindeutig, wird nicht geändert).", r.table),
                Lang::En => format!("{rows} row(s) in {} (not unambiguous, left unchanged).", r.table),
            },
            RepairStep::Counter(c) => c.text(lang),
            RepairStep::Reindex => lang.t("Alle Suchindizes neu aufbauen (REINDEX).", "Rebuild all search indexes (REINDEX).").into(),
        }
    }

    pub fn applies(&self) -> bool {
        !matches!(self, RepairStep::Orphans { fix: OrphanFix::Manual, .. })
    }
}

/// Was die Reparatur tun würde (nur lesen).
pub fn plan_repair(conn: &Connection) -> MRes<Vec<RepairStep>> {
    let mut v = Vec::new();
    for (r, rows) in orphans(conn)? {
        let fix = orphan_fix(conn, &r);
        v.push(RepairStep::Orphans { r, rows, fix });
    }
    v.extend(counters(conn)?.into_iter().map(RepairStep::Counter));
    let lines = integrity(conn, DbScope::Full).unwrap_or_default();
    if lines.iter().any(|l| l.to_ascii_lowercase().contains("index")) {
        v.push(RepairStep::Reindex);
    }
    Ok(v)
}

/// Führt die eindeutigen Schritte aus (in einer Transaktion; REINDEX danach). Liefert die Zahl der geänderten Zeilen/Zähler.
pub fn apply_repair(conn: &Connection, steps: &[RepairStep]) -> MRes<i64> {
    let mut changed = 0i64;
    conn.execute_batch("SAVEPOINT maint_repair")?;
    let r = (|| -> MRes<()> {
        for s in steps {
            match s {
                RepairStep::Orphans { r, fix: OrphanFix::Delete, .. } => {
                    let sql = format!(
                        "DELETE FROM {t} WHERE {c} IS NOT NULL AND {c} NOT IN (SELECT {pc} FROM {p})",
                        t = quote_ident(&r.table),
                        c = quote_ident(&r.column),
                        p = quote_ident(&r.parent),
                        pc = quote_ident(&r.parent_column)
                    );
                    changed += conn.execute(&sql, [])? as i64;
                }
                RepairStep::Orphans { r, fix: OrphanFix::SetNull, .. } => {
                    let sql = format!(
                        "UPDATE {t} SET {c} = NULL WHERE {c} IS NOT NULL AND {c} NOT IN (SELECT {pc} FROM {p})",
                        t = quote_ident(&r.table),
                        c = quote_ident(&r.column),
                        p = quote_ident(&r.parent),
                        pc = quote_ident(&r.parent_column)
                    );
                    changed += conn.execute(&sql, [])? as i64;
                }
                RepairStep::Orphans { .. } => {}
                RepairStep::Counter(Counter::ProjectNumber { project_id, needed, .. }) => {
                    changed += conn.execute("UPDATE projects SET next_number = ?1 WHERE id = ?2 AND next_number < ?1", params![needed, project_id])? as i64;
                }
                RepairStep::Counter(Counter::HistoryBatch { needed, .. }) => {
                    changed += conn.execute("INSERT INTO meta (key, value) VALUES ('history_batch', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [needed.to_string()])? as i64;
                }
                RepairStep::Counter(Counter::Sequence { table, needed, .. }) => {
                    changed += conn.execute("UPDATE sqlite_sequence SET seq = ?1 WHERE name = ?2 AND seq < ?1", params![needed, table])? as i64;
                }
                RepairStep::Reindex => {}
            }
        }
        Ok(())
    })();
    match r {
        Ok(()) => conn.execute_batch("RELEASE maint_repair")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO maint_repair; RELEASE maint_repair");
            return Err(e);
        }
    }
    if steps.iter().any(|s| matches!(s, RepairStep::Reindex)) {
        conn.execute_batch("REINDEX")?;
        changed += 1;
    }
    Ok(changed)
}

// ------------------------------------------------------------------ Verdichten

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactResult {
    pub before: u64,
    pub after: u64,
}

/// Größe der Datenbank samt WAL-Datei.
pub fn file_size(db_path: &Path) -> u64 {
    let wal = PathBuf::from(format!("{}-wal", db_path.display()));
    std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0) + std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0)
}

/// VACUUM + ANALYZE + REINDEX über die ganze Datei (eigene Verbindung; sperrt kurz alle Schreibzugriffe).
pub fn compact(db_path: &Path) -> MRes<CompactResult> {
    let before = file_size(db_path);
    let c = Connection::open(db_path)?;
    c.busy_timeout(std::time::Duration::from_secs(15))?;
    let _: Option<(i64, i64, i64)> = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    c.execute_batch("VACUUM; ANALYZE; REINDEX;")?;
    let _: Option<(i64, i64, i64)> = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    drop(c);
    Ok(CompactResult { before, after: file_size(db_path) })
}

// ------------------------------------------------------------------ Statistik

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stats {
    /// Bytes der Datei (Seiten × Seitengröße) und davon frei.
    pub file_bytes: i64,
    pub free_bytes: i64,
    pub tickets: i64,
    pub archived: i64,
    pub comments: i64,
    pub history: i64,
    pub time_entries: i64,
    pub links: i64,
    pub values: i64,
    /// Ungefähre Größe der Daten des Bereichs (Texte und Werte) in Bytes.
    pub data_bytes: i64,
    /// Die größten Tickets: (Schlüssel, Titel, Bytes).
    pub largest: Vec<(String, String, i64)>,
    /// Neue Tickets je Monat (`JJJJ-MM`, Anzahl), die letzten zwölf Monate mit Tickets.
    pub growth: Vec<(String, i64)>,
}

/// Statistik für ein Projekt (`Some`) oder die ganze Datenbank (`None`).
pub fn stats(conn: &Connection, project: Option<i64>) -> MRes<Stats> {
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let w = if project.is_some() { "t.project_id = ?1" } else { "?1 IS NULL OR 1" };
    let n = |sql: &str| -> rusqlite::Result<i64> { conn.query_row(&sql.replace("{W}", w), params![project], |r| r.get(0)) };
    let mut s = Stats {
        file_bytes: page_size * pages,
        free_bytes: page_size * free,
        tickets: n("SELECT COUNT(*) FROM tickets t WHERE {W}")?,
        archived: n("SELECT COUNT(*) FROM tickets t WHERE ({W}) AND t.archived = 1")?,
        comments: n("SELECT COUNT(*) FROM comments c JOIN tickets t ON t.id = c.ticket_id WHERE {W}")?,
        history: n("SELECT COUNT(*) FROM history h JOIN tickets t ON t.id = h.ticket_id WHERE {W}")?,
        time_entries: n("SELECT COUNT(*) FROM time_entries e JOIN tickets t ON t.id = e.ticket_id WHERE {W}")?,
        links: n("SELECT COUNT(*) FROM links l JOIN tickets t ON t.id = l.from_ticket_id WHERE {W}")?,
        values: n("SELECT COUNT(*) FROM ticket_values v JOIN tickets t ON t.id = v.ticket_id WHERE {W}")?,
        ..Default::default()
    };
    // ungefähre Größe je Ticket: Titel, Werte, Kommentare, Verlauf
    let per_ticket = "SELECT t.id, p.key || '-' || t.number, t.title,
        length(t.title)
        + COALESCE((SELECT SUM(length(COALESCE(v.text, '')) + 8) FROM ticket_values v WHERE v.ticket_id = t.id), 0)
        + COALESCE((SELECT SUM(length(c.body)) FROM comments c WHERE c.ticket_id = t.id), 0)
        + COALESCE((SELECT SUM(length(COALESCE(h.old, '')) + length(COALESCE(h.new, '')) + 40) FROM history h WHERE h.ticket_id = t.id), 0) AS bytes
        FROM tickets t JOIN projects p ON p.id = t.project_id WHERE {W}";
    let mut st = conn.prepare(&format!("SELECT SUM(bytes) FROM ({})", per_ticket.replace("{W}", w)))?;
    s.data_bytes = st.query_row(params![project], |r| r.get::<_, Option<i64>>(0))?.unwrap_or(0);
    let mut st = conn.prepare(&format!("{} ORDER BY bytes DESC LIMIT 10", per_ticket.replace("{W}", w)))?;
    s.largest = st.query_map(params![project], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?.collect::<Result<_, _>>()?;
    let mut st = conn.prepare(&format!("SELECT substr(t.created_at, 1, 7) AS m, COUNT(*) FROM tickets t WHERE {w} GROUP BY m ORDER BY m DESC LIMIT 12"))?;
    let mut growth: Vec<(String, i64)> = st.query_map(params![project], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    growth.reverse();
    s.growth = growth;
    Ok(s)
}

/// Bytes lesbar (KB/MB).
pub fn human_bytes(b: i64) -> String {
    let b = b.max(0) as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{b:.0} B")
    }
}

// ------------------------------------------------------------------ Rettung in eine neue Datei

#[derive(Debug, Clone, PartialEq)]
pub struct TableRescue {
    pub table: String,
    pub rescued: i64,
    /// Zeilen, die die beschädigte Datei laut Zählung hatte (`None` = nicht zählbar).
    pub expected: Option<i64>,
    /// Übersprungene unlesbare Bereiche.
    pub skipped_ranges: usize,
    /// Erster Lesefehler.
    pub error: Option<String>,
}

impl TableRescue {
    /// Verlorene Zeilen, soweit bekannt.
    pub fn lost(&self) -> Option<i64> {
        self.expected.map(|e| (e - self.rescued).max(0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RescueReport {
    pub source: PathBuf,
    pub dest: PathBuf,
    pub tables: Vec<TableRescue>,
    /// Schema-Version der beschädigten Datei (falls lesbar).
    pub source_version: Option<u32>,
    /// `integrity_check` der neuen Datei (leer = in Ordnung).
    pub dest_problems: Vec<String>,
    /// Verwaiste Zeilen in der neuen Datei (Fremdschlüssel), z. B. Kommentare zu einem verlorenen Ticket.
    pub dest_orphans: i64,
}

impl RescueReport {
    pub fn rescued(&self) -> i64 {
        self.tables.iter().map(|t| t.rescued).sum()
    }

    pub fn complete(&self) -> bool {
        self.tables.iter().all(|t| t.error.is_none() && t.skipped_ranges == 0 && t.lost().unwrap_or(0) == 0)
    }

    /// Bericht als Text (wird neben die neue Datei gelegt).
    pub fn text(&self, lang: Lang) -> String {
        let mut s = String::new();
        s.push_str(&match lang {
            Lang::De => format!("Paneldeck – Rettung einer beschädigten Datenbank\r\nQuelle (unverändert): {}\r\nNeue Datei: {}\r\n\r\n", self.source.display(), self.dest.display()),
            Lang::En => format!("Paneldeck – rescue of a damaged database\r\nSource (unchanged): {}\r\nNew file: {}\r\n\r\n", self.source.display(), self.dest.display()),
        });
        for t in &self.tables {
            let lost = match t.lost() {
                Some(l) => l.to_string(),
                None => lang.t("unbekannt", "unknown").to_string(),
            };
            s.push_str(&match lang {
                Lang::De => format!("{}: {} gerettet, {} verloren", t.table, t.rescued, lost),
                Lang::En => format!("{}: {} rescued, {} lost", t.table, t.rescued, lost),
            });
            if t.skipped_ranges > 0 {
                s.push_str(&match lang {
                    Lang::De => format!(", {} unlesbare Bereiche übersprungen", t.skipped_ranges),
                    Lang::En => format!(", {} unreadable ranges skipped", t.skipped_ranges),
                });
            }
            if let Some(e) = &t.error {
                s.push_str(&format!(" ({e})"));
            }
            s.push_str("\r\n");
        }
        if self.dest_orphans > 0 {
            s.push_str(&match lang {
                Lang::De => format!("\r\n{} Zeile(n) der neuen Datei verweisen auf Verlorenes; die Reparatur der Wartungsseite räumt sie auf.\r\n", self.dest_orphans),
                Lang::En => format!("\r\n{} row(s) of the new file refer to lost data; the repair on the maintenance page cleans them up.\r\n", self.dest_orphans),
            });
        }
        s
    }
}

/// Spalten einer Tabelle (Namen in Reihenfolge).
fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare(&format!("PRAGMA table_info({})", quote_ident(table)))?;
    let v = st.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
    Ok(v)
}

fn has_rowid(conn: &Connection, table: &str) -> bool {
    conn.prepare(&format!("SELECT rowid FROM {} LIMIT 0", quote_ident(table))).is_ok()
}

/// Rettet alles Lesbare aus `source` in eine **neue** Datei in `dest_dir` (Name `paneldeck-rescued-<Zeit>.db`) mit dem aktuellen
/// Schema. Die Quelle wird nur lesend geöffnet und nie verändert; daneben entsteht ein Textbericht (`.txt`).
pub fn rescue(source: &Path, dest_dir: &Path, lang: Lang) -> MRes<RescueReport> {
    std::fs::create_dir_all(dest_dir).map_err(|e| MaintError::Db(e.to_string()))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut dest = dest_dir.join(format!("paneldeck-rescued-{stamp}.db"));
    let mut i = 2;
    while dest.exists() {
        dest = dest_dir.join(format!("paneldeck-rescued-{stamp}-{i}.db"));
        i += 1;
    }
    if dest == source {
        return Err(MaintError::refused("Ziel und Quelle sind gleich.", "Target and source are the same."));
    }
    let src = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let source_version: Option<u32> = src.pragma_query_value(None, "user_version", |r| r.get(0)).ok();
    let mut out = Connection::open(&dest)?;
    crate::db::create_schema(&mut out)?;
    out.pragma_update(None, "foreign_keys", false)?;
    let tables: Vec<String> = {
        let mut st = out.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    // Vorgaben des neuen Schemas (System-Projekt, Verknüpfungstypen …) weichen den geretteten Zeilen.
    for t in &tables {
        out.execute(&format!("DELETE FROM {}", quote_ident(t)), [])?;
    }
    out.execute("DELETE FROM sqlite_sequence", [])?;
    let mut report = Vec::new();
    for t in &tables {
        report.push(rescue_table(&src, &out, t));
    }
    // Schlüsselfolgen an die geretteten Kennungen anpassen
    for t in &tables {
        let max: Option<i64> = out.query_row(&format!("SELECT MAX(rowid) FROM {}", quote_ident(t)), [], |r| r.get(0)).unwrap_or(None);
        if let Some(m) = max {
            let _ = out.execute("INSERT OR REPLACE INTO sqlite_sequence (name, seq) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM sqlite_master WHERE name = ?1 AND sql LIKE '%AUTOINCREMENT%')", params![t, m]);
        }
    }
    out.pragma_update(None, "user_version", crate::db::schema_version())?;
    let dest_problems = integrity(&out, DbScope::Full).unwrap_or_else(|e| vec![e.to_string()]);
    let dest_orphans: i64 = orphans(&out).map(|m| m.values().sum()).unwrap_or(0);
    drop(out);
    let rep = RescueReport { source: source.to_path_buf(), dest: dest.clone(), tables: report, source_version, dest_problems, dest_orphans };
    let _ = std::fs::write(dest.with_extension("txt"), rep.text(lang));
    Ok(rep)
}

fn rescue_table(src: &Connection, out: &Connection, table: &str) -> TableRescue {
    let mut res = TableRescue { table: table.to_string(), rescued: 0, expected: None, skipped_ranges: 0, error: None };
    let dest_cols = match columns(out, table) {
        Ok(c) => c,
        Err(e) => {
            res.error = Some(e.to_string());
            return res;
        }
    };
    // gemeinsame Spalten (eine ältere Quelle kennt neuere Spalten nicht – die bekommen ihren Standardwert)
    let cols: Vec<String> = match columns(src, table) {
        Ok(sc) if !sc.is_empty() => dest_cols.iter().filter(|c| sc.contains(c)).cloned().collect(),
        Ok(_) => {
            res.expected = Some(0);
            return res; // Tabelle gibt es in der Quelle nicht
        }
        Err(_) => dest_cols.clone(),
    };
    if cols.is_empty() {
        return res;
    }
    res.expected = src.query_row(&format!("SELECT COUNT(*) FROM {}", quote_ident(table)), [], |r| r.get(0)).ok();
    let list = cols.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ");
    let marks = vec!["?"; cols.len()].join(", ");
    let insert = format!("INSERT OR IGNORE INTO {} ({list}) VALUES ({marks})", quote_ident(table));
    let Ok(mut ins) = out.prepare(&insert) else {
        res.error = Some("insert".into());
        return res;
    };
    let mut copy_row = |r: &rusqlite::Row, res: &mut TableRescue| {
        let vals: Vec<rusqlite::types::Value> = (0..cols.len()).map(|i| r.get::<_, rusqlite::types::Value>(i).unwrap_or(rusqlite::types::Value::Null)).collect();
        if ins.execute(rusqlite::params_from_iter(vals.iter())).map(|n| n > 0).unwrap_or(false) {
            res.rescued += 1;
        }
    };
    if !has_rowid(src, table) {
        // ohne rowid: lesen, bis ein Fehler kommt
        let sql = format!("SELECT {list} FROM {}", quote_ident(table));
        match src.prepare(&sql) {
            Ok(mut st) => match st.query([]) {
                Ok(mut rows) => loop {
                    match rows.next() {
                        Ok(Some(r)) => copy_row(r, &mut res),
                        Ok(None) => break,
                        Err(e) => {
                            res.error = Some(e.to_string());
                            break;
                        }
                    }
                },
                Err(e) => res.error = Some(e.to_string()),
            },
            Err(e) => res.error = Some(e.to_string()),
        }
        return res;
    }
    // mit rowid: aufsteigend lesen; ein unlesbarer Bereich wird übersprungen (Sprung mit wachsender Weite)
    let max_rowid: i64 = src.query_row(&format!("SELECT MAX(rowid) FROM {}", quote_ident(table)), [], |r| r.get::<_, Option<i64>>(0)).ok().flatten().unwrap_or(i64::MAX / 4);
    let sql = format!("SELECT {list}, rowid FROM {} WHERE rowid > ?1 ORDER BY rowid", quote_ident(table));
    let mut after: i64 = i64::MIN;
    let mut jumps = 0;
    loop {
        let mut last_ok = after;
        let mut failed = false;
        match src.prepare(&sql) {
            Ok(mut st) => match st.query([after]) {
                Ok(mut rows) => loop {
                    match rows.next() {
                        Ok(Some(r)) => {
                            copy_row(r, &mut res);
                            last_ok = r.get::<_, i64>(cols.len()).unwrap_or(last_ok);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            res.error.get_or_insert(e.to_string());
                            failed = true;
                            break;
                        }
                    }
                },
                Err(e) => {
                    res.error.get_or_insert(e.to_string());
                    failed = true;
                }
            },
            Err(e) => {
                res.error.get_or_insert(e.to_string());
                return res;
            }
        }
        if !failed {
            break;
        }
        // über den kaputten Bereich springen: erst kleine, dann größere Schritte, bis wieder etwas lesbar ist
        res.skipped_ranges += 1;
        jumps += 1;
        if jumps > 200 || last_ok >= max_rowid {
            break;
        }
        let base = if last_ok == i64::MIN { 0 } else { last_ok };
        let mut step: i64 = 1;
        let mut next = None;
        while base.saturating_add(step) <= max_rowid {
            let probe = base.saturating_add(step);
            let ok = src.query_row(&format!("SELECT rowid FROM {} WHERE rowid >= ?1 ORDER BY rowid LIMIT 1", quote_ident(table)), [probe], |r| r.get::<_, i64>(0));
            match ok {
                Ok(found) => {
                    next = Some(found - 1);
                    break;
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => break,
                Err(_) => step = step.saturating_mul(2),
            }
        }
        match next {
            Some(n) if n > after => after = n,
            _ => break,
        }
    }
    res
}
