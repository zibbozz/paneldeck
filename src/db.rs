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
    // 2: Todo-Liste (Einträge, Tags, Zuordnung). `id` wird nie wiederverwendet (AUTOINCREMENT) und ist die Nummer von „TODO-<id>“.
    // `name_key` ist der kleingeschriebene Tag-Name (Vergleich ohne Groß-/Kleinschreibung); `color` ein „#RRGGBB“-Text.
    "CREATE TABLE todo_items (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        title TEXT NOT NULL DEFAULT '',
        body TEXT NOT NULL DEFAULT '',
        done INTEGER NOT NULL DEFAULT 0,
        position INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        done_at TEXT
    );
    CREATE INDEX todo_items_position ON todo_items (position);
    CREATE TABLE todo_tags (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        name_key TEXT NOT NULL UNIQUE,
        color TEXT NOT NULL
    );
    CREATE TABLE todo_item_tags (
        item_id INTEGER NOT NULL REFERENCES todo_items (id) ON DELETE CASCADE,
        tag_id INTEGER NOT NULL REFERENCES todo_tags (id) ON DELETE CASCADE,
        PRIMARY KEY (item_id, tag_id)
    ) WITHOUT ROWID;
    CREATE INDEX todo_item_tags_tag ON todo_item_tags (tag_id);",
    // 3: Konfiguration der Projekte (Projekte, Status, Typen, Felder, Workflows, Verknüpfungstypen, Arten der Arbeit), SQL in `planning/schema.rs`
    crate::planning::schema::MIGRATION,
    // 4: Konfiguration der Ansichten (Boards, gespeicherte Filter, JQL-Schlüsselwörter, Dashboards), SQL in `planning/views/schema.rs`
    crate::planning::views::schema::SQL,
    // 5: Quick-Actions (Schnellaktionen) der Tickettypen, SQL in `planning/quick/schema.rs`
    crate::planning::quick::schema::MIGRATION,
    // 6: Tickets (Tickets, Feldwerte, Verknüpfungen, Kommentare, Verlauf, Zeiteinträge, Todo-Umwandlungen), SQL in `planning/ticket_schema.rs`
    crate::planning::ticket_schema::MIGRATION,
    // 7: Sprints, Zugehörigkeit der Tickets, Versionen und Meilensteine, SQL in `planning/sprints/schema.rs`
    crate::planning::sprints::schema::MIGRATION,
    // 8: eingebaute Felder Beschreibung/Fälligkeit, Fälligkeits-Schwellen je Projekt, Fälligkeit der Todo-Karten, SQL in `planning/builtin_fields.rs`
    crate::planning::builtin_fields::MIGRATION,
    // 9: eingebaute Labels an jedem Typ (Issue #195), SQL in `planning/builtin_fields.rs`
    crate::planning::builtin_fields::MIGRATION_LABELS,
    // 10: Wartung (Issue #308): Archiv-Markierung der Tickets, Wartungs-Einstellungen, ignorierte Befunde, eigene Regeln,
    // Wartungsprotokoll, SQL in `planning/maintenance/schema.rs`
    crate::planning::maintenance::schema::MIGRATION,
    // 11: Reiter „Zeit“ (Issue #164): Wecker, Timer und Timer-Vorlagen, Stoppuhren, Weltuhren, Geburtstage, SQL in `time/store.rs`
    crate::time::store::MIGRATION,
    // 12: Kalender (Issue #164): eigene Termine (`cal_events`) und Einstellungen (`cal_settings`), SQL in `calendar/store.rs`
    crate::calendar::store::MIGRATION,
    // 13: Zugriff der Agenten je Projekt, private Einträge, Herkunft, Papierkorb (Issue #386), SQL in `planning/access.rs`
    crate::planning::access::MIGRATION,
];

/// Schema-Version dieser Programmversion (auch für die Rettung einer beschädigten Datenbank in eine neue Datei).
pub fn schema_version() -> u32 {
    MIGRATIONS.len() as u32
}

/// Legt in einer leeren Verbindung das vollständige aktuelle Schema an (Rettung in eine neue Datei, `planning::maintenance`).
pub fn create_schema(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for m in MIGRATIONS {
        tx.execute_batch(m)?;
    }
    tx.pragma_update(None, "user_version", MIGRATIONS.len() as u32)?;
    tx.commit()
}

/// Aktuelle Schema-Version dieser Programmversion.
#[cfg(test)]
pub fn latest_version() -> u32 {
    MIGRATIONS.len() as u32
}

/// Stelle einer Migration in der Liste (für Migrationstests anderer Module, unabhängig von der Reihenfolge).
#[cfg(test)]
pub fn migration_index(sql: &str) -> usize {
    MIGRATIONS.iter().position(|m| *m == sql).expect("Migration nicht in der Liste")
}

/// Datenbank im Datenordner mit nur den ersten `n` Migrationen öffnen (Stand einer älteren Programmversion, für Tests).
#[cfg(test)]
pub fn open_prefix(data_dir: &Path, backup_dir: &Path, n: usize) -> Result<Db, OpenError> {
    open_with(data_dir, backup_dir, &MIGRATIONS[..n])
}

/// Datenbank im Speicher mit allen Migrationen (für Tests anderer Module).
#[cfg(test)]
pub fn open_memory() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    for m in MIGRATIONS {
        conn.execute_batch(m).unwrap();
    }
    conn.pragma_update(None, "user_version", MIGRATIONS.len() as u32).unwrap();
    conn
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
    // eingebaute Felder für Typen nachtragen, die von außen in die Datenbank kamen (idempotent; nur Felder, deren Migration schon
    // gelaufen ist – das prüft `ensure` an der Schema-Version)
    crate::planning::builtin_fields::ensure(&conn).map_err(other)?;
    // eigene Felder, die die Migration wegen Groß-/Kleinschreibung jenseits von ASCII übersah (#221) – nur eine Aufräumarbeit:
    // ein Fehler verhindert das Öffnen nicht (beim nächsten Start wird es erneut versucht)
    if let Err(e) = crate::planning::builtin_fields::rename_clashing_custom(&conn) {
        eprintln!("rename_clashing_custom: {e}");
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
    fn upgrades_a_configuration_only_database_and_keeps_its_data() {
        use crate::planning::projects::{create_project, get_project, NewProject};
        use crate::planning::tickets::{create_ticket, NewTicket};
        let d = tmp("db-v3");
        let bdir = d.join("backups");
        let pid = {
            let db = open_with(&d, &bdir, &MIGRATIONS[..3]).unwrap();
            let v: u32 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
            assert_eq!(v, 3);
            assert!(db.conn.prepare("SELECT * FROM tickets").is_err(), "Tickets gibt es erst ab Version 4");
            // die Standards von Hand (das Anlegen „mit Standards“ setzt die Schwellen-Spalte, die es erst ab Version 8 gibt)
            let pid = create_project(&db.conn, &NewProject::new("PAN", "Paneldeck", "#112233")).unwrap();
            crate::planning::defaults::apply(&db.conn, pid, false).unwrap();
            pid
        };
        let db = open(&d, &bdir).unwrap();
        let v: u32 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, latest_version());
        assert_eq!(get_project(&db.conn, pid).unwrap().key, "PAN", "Konfiguration bleibt");
        for t in ["tickets", "ticket_values", "links", "comments", "history", "time_entries", "todo_conversions"] {
            db.conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
        }
        let ty = crate::planning::types::list_types(&db.conn, pid).unwrap()[0].id;
        let (t, _) = create_ticket(&db.conn, &NewTicket::new(pid, ty, "Erstes"), &crate::planning::history::Source::ui()).unwrap();
        assert_eq!(t.key(), "PAN-1");
        let l = backup::list(&bdir);
        assert_eq!(l.len(), 1);
        assert!(matches!(l[0].kind, backup::Kind::PreMigration { from: 3, to } if to == latest_version()));
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

    fn user_version(c: &Connection) -> u32 {
        c.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap()
    }

    /// Fremdschlüssel und Datei in Ordnung?
    fn assert_healthy(c: &Connection) {
        let ic: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ic, "ok");
        let bad: i64 = c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
        assert_eq!(bad, 0, "Fremdschlüssel verletzt");
    }

    /// Ein Datenordner, wie ihn 1.0.x hinterlässt (es gibt dort noch keine Datenbank): Einstellungen, Zustand, Prompts, Themes, Agentenprotokoll.
    /// Das Öffnen der Datenbank legt sie neu an (ohne Sicherung, es gibt nichts zu sichern) und lässt alle alten Dateien unverändert.
    #[test]
    fn upgrade_from_a_1_0_data_folder_creates_the_database_and_leaves_the_old_files_alone() {
        let d = tmp("db-from-1-0");
        let files: [(&str, &str); 5] = [
            ("settings.json", "{\n  \"language\": \"De\",\n  \"ui_scale\": 1.25,\n  \"mcp_level\": 2,\n  \"scrollback_lines\": 20000\n}"),
            ("state.json", "{\"sessions\": [], \"focused\": 0, \"next_id\": 7, \"projects\": []}"),
            ("prompts.json", "[{\"name\": \"Größe ä\", \"text\": \"Hallo 😀\"}]"),
            ("mcp-log.jsonl", "{\"t\":\"2026-01-01T00:00:00\",\"tool\":\"list_sessions\"}\n"),
            ("themes/mein.json", "{\"name\": \"mein\"}"),
        ];
        std::fs::create_dir_all(d.join("themes")).unwrap();
        for (n, c) in files {
            std::fs::write(d.join(n), c).unwrap();
        }
        let bdir = d.join("backups");
        let db = open(&d, &bdir).unwrap();
        assert_eq!(user_version(&db.conn), latest_version());
        assert!(backup::list(&bdir).is_empty(), "eine neue Datenbank braucht keine Sicherung vor der Migration");
        // WAL, Fremdschlüssel an
        let mode: String = db.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = db.conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 1);
        assert_healthy(&db.conn);
        drop(db);
        for (n, c) in files {
            assert_eq!(std::fs::read_to_string(d.join(n)).unwrap(), c, "{n} darf nicht verändert werden");
        }
        // die alten Einstellungen/der alte Zustand sind weiter lesbar, neue Felder bekommen Standardwerte
        let mut s: crate::state::Settings = crate::state::load_json(&d.join("settings.json")).unwrap();
        s.sanitize();
        assert_eq!(s.undo_steps, 25);
        assert_eq!(s.backup_keep, 2);
        assert_eq!(s.backup_interval_hours, 24);
        assert!(s.backup_dir.is_empty());
        let (saved, note) = crate::state::load_saved(&d.join("state.json"));
        assert!(note.is_none());
        assert_eq!(saved.next_id, 7);
        // zweites Öffnen (Neustart): nichts ändert sich
        let db = open(&d, &bdir).unwrap();
        assert_eq!(user_version(&db.conn), latest_version());
        assert!(backup::list(&bdir).is_empty());
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Reiter „Zeit“ (#164): Eine Datenbank von 3.0/3.1 (Schema 10, mit Projekt und Ticket) bekommt die Tabellen der
    /// Migration 11, vorher eine Sicherung; die alten Daten bleiben, die neuen Tabellen sind leer und nutzbar. Eine
    /// Sicherung (`VACUUM INTO`) enthält danach auch die Einträge des Reiters, und die Wiederherstellung bringt sie zurück.
    #[test]
    fn a_3_0_database_gets_the_time_tables_and_backups_include_them() {
        use crate::planning::projects::{create_project, NewProject};
        use crate::planning::tickets::{create_ticket, NewTicket};
        use crate::time::{store, Alarm, Birthday};
        let d = tmp("db-v10-time");
        let bdir = d.join("backups");
        let ten = MIGRATIONS.iter().position(|m| *m == crate::time::store::MIGRATION).expect("Zeit-Migration vorhanden");
        let (pid, tid) = {
            let db = open_with(&d, &bdir, &MIGRATIONS[..ten]).unwrap();
            assert_eq!(user_version(&db.conn), ten as u32);
            assert!(!store::available(&db.conn), "vorher keine Zeit-Tabellen");
            let mut np = NewProject::new("PAN", "Paneldeck", "#112233");
            np.with_defaults = true;
            let pid = create_project(&db.conn, &np).unwrap();
            let ty = crate::planning::types::list_types(&db.conn, pid).unwrap()[0].id;
            // Direkt in SQL: die Ticket-Funktionen dieser Version lesen schon die Spalten späterer Migrationen (#386).
            db.conn.execute("INSERT INTO tickets (project_id, number, type_id, title, created_at, updated_at) VALUES (?1, 1, ?2, 'Alt', 'x', 'x')", rusqlite::params![pid, ty]).unwrap();
            (pid, db.conn.last_insert_rowid())
        };
        let db = open(&d, &bdir).unwrap();
        assert_eq!(user_version(&db.conn), latest_version());
        assert!(store::available(&db.conn));
        assert_healthy(&db.conn);
        assert_eq!(crate::planning::tickets::get_ticket(&db.conn, tid).unwrap().title, "Alt");
        assert_eq!(crate::planning::projects::get_project(&db.conn, pid).unwrap().key, "PAN");
        let l = backup::list(&bdir);
        assert!(l.iter().any(|b| matches!(b.kind, backup::Kind::PreMigration { from, .. } if from == ten as u32)), "Sicherung vor der Migration");
        let empty = store::load(&db.conn).unwrap();
        assert!(empty.alarms.is_empty() && empty.timers.is_empty() && empty.birthdays.is_empty());
        store::save_alarm(&db.conn, &mut Alarm::new("Aufstehen", 7, 0, 0x1f, 0)).unwrap();
        store::save_birthday(&db.conn, &mut Birthday::new("Anna", 2, 29, None)).unwrap();
        // Sicherung enthält die Einträge
        let b = backup::create(&db.path, &bdir, backup::Kind::Manual).unwrap();
        let copy = Connection::open(&b.path).unwrap();
        let data = store::load(&copy).unwrap();
        assert_eq!((data.alarms[0].name.as_str(), data.birthdays[0].name.as_str()), ("Aufstehen", "Anna"));
        drop(copy);
        // Wiederherstellen nach dem Löschen bringt den Wecker zurück
        store::delete(&db.conn, store::Table::Alarms, data.alarms[0].id).unwrap();
        drop(db);
        backup::schedule_restore(&d, &b.path).unwrap();
        assert!(backup::apply_pending_restore(&d, FILE).unwrap().is_ok());
        let db = open(&d, &bdir).unwrap();
        assert_eq!(store::load(&db.conn).unwrap().alarms.len(), 1, "wiederhergestellt");
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Jede Zwischenversion (auch von Entwicklungsständen) lässt sich auf den aktuellen Stand heben, die Daten bleiben erhalten.
    #[test]
    fn every_older_schema_version_upgrades_and_keeps_its_data() {
        for from in 2..latest_version() as usize {
            let d = tmp(&format!("db-from-v{from}"));
            let bdir = d.join("backups");
            {
                let db = open_with(&d, &bdir, &MIGRATIONS[..from]).unwrap();
                assert_eq!(user_version(&db.conn), from as u32);
                let c = &db.conn;
                c.execute_batch(
                    "INSERT INTO todo_tags (name, name_key, color) VALUES ('Zuhause', 'zuhause', '#336699');
                     INSERT INTO todo_items (title, body, done, position, created_at, updated_at) VALUES ('Karte ä😀', 'Text', 0, 1, '2026-01-01T00:00:00', '2026-01-01T00:00:00');
                     INSERT INTO todo_item_tags (item_id, tag_id) VALUES (1, 1);",
                )
                .unwrap();
                if from >= 3 {
                    let pid = crate::planning::projects::create_project(c, &crate::planning::projects::NewProject::new("PAN", "Paneldeck", "#112233")).unwrap();
                    crate::planning::defaults::apply(c, pid, false).unwrap();
                }
                db.set_meta("marker", "vorher");
            }
            let db = open(&d, &bdir).unwrap();
            assert_eq!(user_version(&db.conn), latest_version(), "von {from}");
            assert_healthy(&db.conn);
            assert_eq!(db.meta("marker").as_deref(), Some("vorher"));
            let items = crate::tasks::list_items(&db.conn, &crate::tasks::ItemFilter { show_done: true, tags: Vec::new() }).unwrap();
            assert_eq!(items.len(), 1, "von {from}: Todo-Karte bleibt");
            assert_eq!(items[0].title, "Karte ä😀");
            assert_eq!(items[0].tags.len(), 1, "von {from}: Tag-Zuordnung bleibt");
            let tags = crate::tasks::list_tags(&db.conn).unwrap();
            assert_eq!((tags.len(), tags[0].name.as_str(), tags[0].color.as_str()), (1, "Zuhause", "#336699"));
            if from >= 3 {
                assert_eq!(crate::planning::projects::project_by_key(&db.conn, "PAN").unwrap().name, "Paneldeck");
            }
            // neue Tabellen sind benutzbar
            for t in ["tickets", "sprints", "versions", "milestones", "sprint_tickets", "boards", "saved_filters", "quick_actions"] {
                db.conn.prepare(&format!("SELECT * FROM {t}")).unwrap_or_else(|e| panic!("{t}: {e}"));
            }
            let l = backup::list(&bdir);
            assert_eq!(l.len(), 1, "von {from}: genau eine Sicherung vor der Migration");
            assert!(matches!(l[0].kind, backup::Kind::PreMigration { from: f, to } if f == from as u32 && to == latest_version()));
            // die Sicherung ist der alte Stand
            let old = Connection::open(&l[0].path).unwrap();
            let v: u32 = old.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
            assert_eq!(v, from as u32);
            drop(old);
            drop(db);
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// Eine fehlschlagende Migration lässt nichts Halbes zurück: gleiche Version, keine neue Tabelle, Sicherung vorhanden,
    /// und ein späterer Lauf mit der korrigierten Migration funktioniert (abbrechbar und wiederholbar).
    #[test]
    fn a_failing_migration_is_rolled_back_and_can_be_retried() {
        let d = tmp("db-fail");
        let bdir = d.join("backups");
        let m1 = "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;";
        let m2 = "CREATE TABLE a (id INTEGER PRIMARY KEY);";
        let broken = "CREATE TABLE b (id INTEGER PRIMARY KEY); INSERT INTO a VALUES (1); THIS IS NOT SQL;";
        let fixed = "CREATE TABLE b (id INTEGER PRIMARY KEY); INSERT INTO a VALUES (1);";
        {
            let db = open_with(&d, &bdir, &[m1, m2]).unwrap();
            db.set_meta("marker", "vorher");
        }
        match open_with(&d, &bdir, &[m1, m2, broken]) {
            Err(OpenError::Other(_)) => {}
            Err(e) => panic!("falscher Fehler: {e:?}"),
            Ok(_) => panic!("die kaputte Migration muss scheitern"),
        }
        {
            let c = Connection::open(d.join(FILE)).unwrap();
            assert_eq!(user_version(&c), 2, "Version bleibt");
            assert!(c.prepare("SELECT * FROM b").is_err(), "Tabelle der kaputten Migration wurde zurückgenommen");
            let n: i64 = c.query_row("SELECT COUNT(*) FROM a", [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "Einfügen der kaputten Migration wurde zurückgenommen");
        }
        assert_eq!(backup::list(&bdir).len(), 1, "Sicherung vor dem Versuch");
        let db = open_with(&d, &bdir, &[m1, m2, fixed]).unwrap();
        assert_eq!(user_version(&db.conn), 3);
        assert_eq!(db.meta("marker").as_deref(), Some("vorher"));
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM a", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Eine Datenbank wie von 2.2.0 (Schema 7): ein Projekt mit den mitgelieferten Typen (Beschreibung als Feld mit Schlüssel
    /// `description`, umbenannt in einem Typ, Pflicht in einem anderen), ein Projekt ohne Vorlagen mit eigenem Text-Feld
    /// „Beschreibung“ und eigenem Feld „Due date“, Werte, Verlauf und eine Übergangsregel auf die Beschreibung, Todo-Karten.
    /// Nach dem Update sind die Beschreibungen eingebaut (gleiche Kennung: Werte, Verlauf, Regel bleiben), jede Typ hat eine
    /// Fälligkeit, „Due date“ bleibt eigenes Feld, nur das Projekt mit Vorlagen hat Schwellen – und ein zweiter Lauf ändert nichts.
    #[test]
    fn migration_8_turns_description_fields_into_built_ins_and_is_idempotent() {
        use crate::planning::fields::{list_fields, FieldKind};
        let d = tmp("db-v7-due");
        let bdir = d.join("backups");
        {
            let db = open_with(&d, &bdir, &MIGRATIONS[..7]).unwrap();
            db.conn
                .execute_batch(
                    "INSERT INTO projects (id, key, name, color, system, next_number, position, created_at) VALUES
                        (10, 'PAN', 'Paneldeck', '#112233', 0, 3, 1, '2026-01-01T00:00:00'),
                        (11, 'OWN', 'Eigen', '#445566', 0, 2, 2, '2026-01-01T00:00:00');
                     INSERT INTO statuses (id, project_id, key, name, color, category, position) VALUES
                        (20, 10, 'todo', NULL, '#97A3B0', 'todo', 0), (21, 10, 'done', NULL, '#7FBF6A', 'done', 1);
                     INSERT INTO ticket_types (id, project_id, key, name, icon, color, workflow, system, position) VALUES
                        (30, 10, 'epic', NULL, 'bolt', '#B77FE0', 1, 0, 0),
                        (31, 10, 'devtask', NULL, 'code', '#58A6E6', 1, 0, 1),
                        (32, 11, NULL, 'Aufgabe', NULL, '#445566', 0, 0, 0);
                     INSERT INTO type_statuses (type_id, status_id, position) VALUES (30, 20, 0), (30, 21, 1), (31, 20, 0), (31, 21, 1);
                     INSERT INTO fields (id, type_id, key, name, kind, options, required, position, builtin) VALUES
                        (40, 30, 'description', NULL, 'rich', '{\"rows\":8}', 0, 0, 1),
                        (41, 30, 'priority', NULL, 'dropdown', '{}', 0, 1, 1),
                        (42, 31, 'description', 'Details', 'rich', '{\"rows\":8}', 1, 0, 1),
                        (43, 32, NULL, 'Beschreibung', 'text', '{\"max_length\":500}', 0, 0, 0),
                        (44, 32, NULL, 'Due date', 'date', '{}', 0, 1, 0),
                        (45, 30, NULL, 'Labels', 'labels', '{}', 1, 2, 0),
                        (46, 31, NULL, 'Labels', 'text', '{}', 0, 1, 0);
                     INSERT INTO transitions (id, type_id, from_status_id, to_status_id, required_fields) VALUES (50, 31, 20, 21, '[42]');
                     INSERT INTO tickets (id, project_id, number, type_id, title, status_id, rank, created_at, updated_at) VALUES
                        (60, 10, 1, 30, 'Epic', 20, 0, '2026-01-01T00:00:00', '2026-01-01T00:00:00'),
                        (61, 10, 2, 31, 'Task', 20, 1, '2026-01-01T00:00:00', '2026-01-01T00:00:00'),
                        (62, 11, 1, 32, 'Eigen', NULL, 0, '2026-01-01T00:00:00', '2026-01-01T00:00:00');
                     INSERT INTO ticket_values (ticket_id, field_id, pos, text, num) VALUES
                        (60, 40, 0, 'Epic **Text**', NULL), (61, 42, 0, 'Details ä😀', NULL), (62, 43, 0, 'Eigener Text', NULL), (62, 44, 0, '2026-12-24', NULL),
                        (60, 45, 0, 'backend', NULL), (60, 45, 1, 'ui', NULL), (61, 46, 0, 'kein Label', NULL);
                     INSERT INTO history (ticket_id, at, source, field, old, new, batch) VALUES
                        (61, '2026-01-02T00:00:00', 'ui', 'field:42', NULL, '\"Details ä😀\"', 1),
                        (60, '2026-01-02T00:00:00', 'ui', 'field:45', NULL, '[\"backend\",\"ui\"]', 2);
                     INSERT INTO todo_items (title, body, done, position, created_at, updated_at) VALUES ('Karte', 'Text', 0, 1, '2026-01-01T00:00:00', '2026-01-01T00:00:00');",
                )
                .unwrap();
        }
        let check = |c: &Connection| {
            let fields = |t: i64| list_fields(c, t).unwrap();
            // Beschreibungen: gleiche Kennung, eingebaut, Rich-Text, nicht Pflicht, oben; Name „Details“ bleibt
            for (t, fid) in [(30, 40), (31, 42)] {
                let fs = fields(t);
                assert_eq!(fs[0].id, fid, "Beschreibung oben in Typ {t}");
                assert!(fs[0].is_description() && fs[0].kind == FieldKind::Rich && !fs[0].required);
                assert!(fs[1].is_due() && fs[1].kind == FieldKind::Datetime && fs[1].options.date_only_ok() && !fs[1].required);
            }
            assert_eq!(fields(31)[0].name.as_deref(), Some("Details"));
            // eigenes Text-Feld „Beschreibung“ wird die eingebaute Beschreibung (Wert bleibt), „Due date“ bleibt eigenes Feld
            // (umbenannt, damit es nicht wie die Fälligkeit heißt), dazu die eingebauten Labels
            let own = fields(32);
            assert_eq!(own.len(), 4);
            assert!(own[0].is_description() && own[1].is_due() && own[2].is_labels());
            let desc = own.iter().find(|f| f.id == 43).unwrap();
            assert!(desc.is_description() && desc.kind == FieldKind::Rich && desc.name.is_none());
            assert_eq!(desc.options.rows, Some(8));
            let custom_due = own.iter().find(|f| f.id == 44).unwrap();
            assert!(!custom_due.is_fixed() && custom_due.kind == FieldKind::Date && custom_due.name.as_deref() == Some("Due date (custom)"));
            // Labels: das eigene Labels-Feld des Epics wird das eingebaute (gleiche Kennung, Werte, Verlauf; nicht mehr Pflicht),
            // das Text-Feld „Labels“ der Dev-Aufgabe bleibt eigenes Feld mit Zusatz, die Dev-Aufgabe bekommt neue Labels
            let epic = fields(30);
            assert_eq!((epic[2].id, epic[2].is_labels(), epic[2].required, epic[2].default_value.clone()), (45, true, false, None));
            let lv: Vec<String> = { let mut st = c.prepare("SELECT text FROM ticket_values WHERE ticket_id = 60 AND field_id = 45 ORDER BY pos").unwrap(); st.query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect() };
            assert_eq!(lv, vec!["backend", "ui"]);
            let hl: i64 = c.query_row("SELECT COUNT(*) FROM history WHERE field = 'field:45'", [], |r| r.get(0)).unwrap();
            assert_eq!(hl, 1);
            let dev = fields(31);
            assert!(dev[2].is_labels() && dev[2].id != 46);
            let text_labels = dev.iter().find(|f| f.id == 46).unwrap();
            assert_eq!((text_labels.kind, text_labels.name.as_deref(), text_labels.is_fixed()), (FieldKind::Text, Some("Labels (custom)"), false));
            assert!(own.iter().any(|f| f.is_due()));
            // Werte, Verlauf, Regel bleiben
            let v = |t: i64, f: i64| -> String { c.query_row("SELECT text FROM ticket_values WHERE ticket_id = ?1 AND field_id = ?2", [t, f], |r| r.get(0)).unwrap() };
            assert_eq!((v(60, 40), v(61, 42), v(62, 43), v(62, 44)), ("Epic **Text**".into(), "Details ä😀".into(), "Eigener Text".into(), "2026-12-24".into()));
            let h: String = c.query_row("SELECT field FROM history WHERE ticket_id = 61", [], |r| r.get(0)).unwrap();
            assert_eq!(h, "field:42");
            let rf: String = c.query_row("SELECT required_fields FROM transitions WHERE id = 50", [], |r| r.get(0)).unwrap();
            assert_eq!(rf, "[42]");
            // Schwellen nur beim Projekt mit Vorlagen; Todo-Karten haben die Spalte `due`
            assert_eq!(crate::planning::projects::due_thresholds(c, 10).unwrap(), crate::due::default_thresholds());
            assert!(crate::planning::projects::due_thresholds(c, 11).unwrap().is_empty());
            let items = crate::tasks::list_items(c, &crate::tasks::ItemFilter { show_done: true, tags: Vec::new() }).unwrap();
            assert_eq!((items.len(), items[0].due.clone()), (1, None));
            // der System-Typ bleibt ohne Felder
            let n: i64 = c.query_row("SELECT COUNT(*) FROM fields f JOIN ticket_types t ON t.id = f.type_id WHERE t.system = 1", [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0);
        };
        let db = open(&d, &bdir).unwrap();
        assert_eq!(user_version(&db.conn), latest_version());
        assert_healthy(&db.conn);
        check(&db.conn);
        let snapshot: Vec<(i64, i64, Option<String>, String, String, i64, i64)> = {
            let mut st = db.conn.prepare("SELECT id, type_id, key, kind, options, position, builtin FROM fields ORDER BY id").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).unwrap().collect::<Result<_, _>>().unwrap()
        };
        // die Sicherung vor dem Update ist der Stand von 2.2.0
        let l = backup::list(&bdir);
        assert!(l.len() == 1 && matches!(l[0].kind, backup::Kind::PreMigration { from: 7, to } if to == latest_version()));
        // idempotent: die Umwandlung noch einmal (wie nach jedem neuen Typ) ändert nichts
        crate::planning::builtin_fields::ensure(&db.conn).unwrap();
        crate::planning::builtin_fields::ensure(&db.conn).unwrap();
        check(&db.conn);
        let again: Vec<(i64, i64, Option<String>, String, String, i64, i64)> = {
            let mut st = db.conn.prepare("SELECT id, type_id, key, kind, options, position, builtin FROM fields ORDER BY id").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).unwrap().collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(snapshot, again);
        drop(db);
        // erneutes Öffnen: keine weitere Migration, keine weitere Sicherung
        let db = open(&d, &bdir).unwrap();
        check(&db.conn);
        assert_eq!(backup::list(&bdir).len(), 1);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// #221: Schritt 4 der Migration verglich nur ASCII-Kleinschreibung; „FÄLLIG AM“ bekommt den Zusatz beim Öffnen, einmal.
    #[test]
    fn custom_fields_named_like_a_built_in_with_non_ascii_case_are_renamed_once() {
        use crate::planning::fields::list_fields;
        let d = tmp("db-v7-unicode-due");
        let bdir = d.join("backups");
        {
            let db = open_with(&d, &bdir, &MIGRATIONS[..7]).unwrap();
            db.conn
                .execute_batch(
                    "INSERT INTO projects (id, key, name, color, system, next_number, position, created_at) VALUES (10, 'UNI', 'U', '#112233', 0, 1, 1, '2026-01-01T00:00:00');
                     INSERT INTO ticket_types (id, project_id, key, name, icon, color, workflow, system, position) VALUES (30, 10, NULL, 'Story', NULL, '#445566', 0, 0, 0);
                     INSERT INTO fields (id, type_id, key, name, kind, options, required, position, builtin) VALUES
                        (40, 30, NULL, 'FÄLLIG AM', 'date', '{}', 0, 0, 0),
                        (41, 30, NULL, 'Fällig am', 'text', '{}', 0, 1, 0),
                        (42, 30, NULL, 'Fällig', 'text', '{}', 0, 2, 0),
                        (43, 30, NULL, 'LABELS', 'text', '{}', 0, 3, 0);",
                )
                .unwrap();
        }
        let names = |c: &Connection| -> Vec<(i64, Option<String>)> { list_fields(c, 30).unwrap().into_iter().filter(|f| !f.is_fixed()).map(|f| (f.id, f.name)).collect() };
        let want = vec![(40, Some("FÄLLIG AM (custom)".to_string())), (41, Some("Fällig am (custom)".to_string())), (42, Some("Fällig".to_string())), (43, Some("LABELS (custom)".to_string()))];
        let db = open(&d, &bdir).unwrap();
        assert_eq!(names(&db.conn), want);
        // idempotent: erneut ausführen und erneut öffnen ändert nichts
        assert_eq!(crate::planning::builtin_fields::rename_clashing_custom(&db.conn).unwrap(), 0);
        drop(db);
        let db = open(&d, &bdir).unwrap();
        assert_eq!(names(&db.conn), want);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Ein Stand mit Schema 8 (Beschreibung und Fälligkeit, noch keine eingebauten Labels) und einem eigenen Feld „Labels“ mit
    /// Standardwert: Öffnen mit Schema 8 legt die Labels noch nicht an (sonst könnte Migration 9 das eigene Feld nicht mehr
    /// umwandeln), das Update auf 9 wandelt es um (gleiche Kennung, Wert bleibt, Standardwert entfällt).
    #[test]
    fn ensure_waits_for_migration_9_before_adding_labels() {
        use crate::planning::fields::list_fields;
        let d = tmp("db-v8-labels");
        let bdir = d.join("backups");
        let (ty, fid) = {
            let db = open_with(&d, &bdir, &MIGRATIONS[..8]).unwrap();
            db.conn
                .execute_batch(
                    "INSERT INTO projects (id, key, name, color, system, next_number, position, created_at) VALUES (20, 'LAB', 'L', '#112233', 0, 2, 1, '2026-01-01T00:00:00');
                     INSERT INTO ticket_types (id, project_id, key, name, icon, color, workflow, system, position) VALUES (70, 20, NULL, 'Story', NULL, '#445566', 0, 0, 0);
                     INSERT INTO fields (id, type_id, key, name, kind, options, required, default_value, position, builtin) VALUES (80, 70, NULL, 'Labels', 'labels', '{}', 0, '[\"neu\"]', 0, 0);
                     INSERT INTO tickets (id, project_id, number, type_id, title, status_id, rank, created_at, updated_at) VALUES (90, 20, 1, 70, 'T', NULL, 0, '2026-01-01T00:00:00', '2026-01-01T00:00:00');
                     INSERT INTO ticket_values (ticket_id, field_id, pos, text, num) VALUES (90, 80, 0, 'alt', NULL);",
                )
                .unwrap();
            (70_i64, 80_i64)
        };
        // erneut mit Schema 8 öffnen: Beschreibung und Fälligkeit kommen dazu, Labels noch nicht
        {
            let db = open_with(&d, &bdir, &MIGRATIONS[..8]).unwrap();
            let keys: Vec<Option<String>> = list_fields(&db.conn, ty).unwrap().into_iter().map(|f| f.key).collect();
            assert_eq!(keys, vec![Some("description".into()), Some("due".into()), None]);
        }
        let db = open(&d, &bdir).unwrap();
        let fs = list_fields(&db.conn, ty).unwrap();
        assert_eq!(fs.len(), 3);
        let lab = fs.iter().find(|f| f.is_labels()).unwrap();
        assert_eq!((lab.id, lab.default_value.clone()), (fid, None), "das eigene Feld wurde umgewandelt, ohne Standardwert");
        let v: String = db.conn.query_row("SELECT text FROM ticket_values WHERE ticket_id = 90 AND field_id = 80", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "alt");
        drop(db);
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
