//! Lokale SQLite-Datenbank im Datenordner (`paneldeck.db`): Verbindung, Schema-Versionen (Migrationen), Meta-Werte.
//!
//! Das Schema wird über `PRAGMA user_version` versioniert. Vor jeder Migration eines bestehenden Bestands wird automatisch
//! eine Sicherung angelegt (`backup::Kind::PreMigration`). Ein Bestand mit neuerem Schema als diese Version wird nicht angefasst.

use crate::backup;
use crate::state::Lang;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub const FILE: &str = "paneldeck.db";

/// Schema-Migrationen: Eintrag `i` hebt das Schema von Version `i` auf `i + 1`. Bestehende Einträge nie ändern, nur anhängen.
const MIGRATIONS: &[&str] = &[
    // 1: Meta-Tabelle (Schlüssel/Wert)
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;",
];

/// Aktuelle Schema-Version dieser Programmversion.
#[cfg(test)]
pub fn latest_version() -> u32 {
    MIGRATIONS.len() as u32
}

pub struct Db {
    pub conn: Connection,
    pub path: PathBuf,
}

#[derive(Debug)]
pub enum OpenError {
    /// Die Datenbank wurde von einer neueren Programmversion angelegt.
    Newer { found: u32, supported: u32 },
    /// Sicherung vor der Migration fehlgeschlagen (die Migration wurde nicht ausgeführt).
    BackupFailed(String),
    Other(String),
}

impl OpenError {
    pub fn message(&self, lang: Lang) -> String {
        match self {
            OpenError::Newer { found, supported } => format!(
                "{} {found}, {} {supported}. {}",
                lang.t("Die Datenbank hat die Schema-Version", "The database has schema version"),
                lang.t("dieses Paneldeck kennt nur bis", "but this Paneldeck only supports up to"),
                lang.t("Bitte Paneldeck aktualisieren; die Datenbank wird nicht verändert.", "Please update Paneldeck; the database is left untouched.")
            ),
            OpenError::BackupFailed(e) => format!(
                "{}: {e}",
                lang.t("Sicherung vor dem Datenbank-Update fehlgeschlagen, das Update wurde nicht ausgeführt", "Backup before the database update failed, the update was not performed")
            ),
            OpenError::Other(e) => format!("{}: {e}", lang.t("Datenbank konnte nicht geöffnet werden", "Could not open the database")),
        }
    }
}

fn other(e: rusqlite::Error) -> OpenError {
    OpenError::Other(e.to_string())
}

/// Öffnet (oder legt an) die Datenbank im Datenordner und führt ausstehende Migrationen aus.
pub fn open(data_dir: &Path, backup_dir: &Path) -> Result<Db, OpenError> {
    open_with(data_dir, backup_dir, MIGRATIONS)
}

fn open_with(data_dir: &Path, backup_dir: &Path, migrations: &[&str]) -> Result<Db, OpenError> {
    let path = data_dir.join(FILE);
    let existed = path.exists();
    let mut conn = Connection::open(&path).map_err(other)?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(other)?;
    // WAL: Leser (Sicherungen) blockieren den Schreiber nicht; liefert eine Zeile zurück, daher query statt execute.
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0)).map_err(other)?;
    conn.pragma_update(None, "foreign_keys", true).map_err(other)?;
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(other)?;
    let latest = migrations.len() as u32;
    if current > latest {
        return Err(OpenError::Newer { found: current, supported: latest });
    }
    if current < latest {
        if existed && current > 0 {
            backup::create(&path, backup_dir, backup::Kind::PreMigration { from: current, to: latest }).map_err(OpenError::BackupFailed)?;
        }
        for v in current..latest {
            let tx = conn.transaction().map_err(other)?;
            tx.execute_batch(migrations[v as usize]).map_err(other)?;
            tx.pragma_update(None, "user_version", v + 1).map_err(other)?;
            tx.commit().map_err(other)?;
        }
    }
    let db = Db { conn, path };
    if db.meta("created_at").is_none() {
        db.set_meta("created_at", &chrono::Local::now().naive_local().format("%Y-%m-%dT%H:%M:%S").to_string());
    }
    db.set_meta("app_version", env!("CARGO_PKG_VERSION"));
    Ok(db)
}

impl Db {
    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).ok()
    }

    pub fn set_meta(&self, key: &str, value: &str) {
        let _ = self.conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value]);
    }

    /// Zeitpunkt der Datenbank-Erstellung (Grundlage für die erste automatische Sicherung).
    pub fn created_at(&self) -> Option<chrono::NaiveDateTime> {
        chrono::NaiveDateTime::parse_from_str(&self.meta("created_at")?, "%Y-%m-%dT%H:%M:%S").ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("paneldeck-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn creates_schema_and_meta() {
        let d = tmp("db-new");
        let db = open(&d, &d.join("backups")).unwrap();
        let v: u32 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, latest_version());
        assert!(db.created_at().is_some());
        assert_eq!(db.meta("app_version").as_deref(), Some(env!("CARGO_PKG_VERSION")));
        drop(db);
        // erneutes Öffnen ändert nichts und legt keine Sicherung an
        let db = open(&d, &d.join("backups")).unwrap();
        assert!(backup::list(&d.join("backups")).is_empty());
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_newer_schema() {
        let d = tmp("db-newer");
        {
            let db = open(&d, &d.join("backups")).unwrap();
            db.conn.pragma_update(None, "user_version", latest_version() + 5).unwrap();
        }
        match open(&d, &d.join("backups")) {
            Err(OpenError::Newer { found, supported }) => {
                assert_eq!(found, latest_version() + 5);
                assert_eq!(supported, latest_version());
            }
            _ => panic!("neueres Schema muss abgelehnt werden"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn backs_up_before_migration_and_keeps_data() {
        let d = tmp("db-migrate");
        let bdir = d.join("backups");
        let v1: &[&str] = &["CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;"];
        let v2: &[&str] = &[v1[0], "CREATE TABLE extra (id INTEGER PRIMARY KEY, name TEXT);"];
        {
            let db = open_with(&d, &bdir, v1).unwrap();
            db.set_meta("marker", "vorher");
        }
        assert!(backup::list(&bdir).is_empty());
        let db = open_with(&d, &bdir, v2).unwrap();
        // Daten bleiben, neue Tabelle existiert, Version ist 2
        assert_eq!(db.meta("marker").as_deref(), Some("vorher"));
        db.conn.execute("INSERT INTO extra (name) VALUES ('x')", []).unwrap();
        let v: u32 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, 2);
        // genau eine Sicherung mit dem Stand vor der Migration
        let l = backup::list(&bdir);
        assert_eq!(l.len(), 1);
        assert!(matches!(l[0].kind, backup::Kind::PreMigration { from: 1, to: 2 }));
        let c = Connection::open(&l[0].path).unwrap();
        let m: String = c.query_row("SELECT value FROM meta WHERE key='marker'", [], |r| r.get(0)).unwrap();
        assert_eq!(m, "vorher");
        assert!(c.prepare("SELECT * FROM extra").is_err(), "die Sicherung enthält den Stand vor der Migration");
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}
