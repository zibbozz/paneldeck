//! SQL der Migration 10 (Paneldeck 2.6.0, Issue #308): Wartungsseite der Experten-Projekte.
//!
//! * `tickets.archived` – archivierte Tickets sind aus Listen, Boards und Suche ausgeblendet (außer mit `archived = true`
//!   in der Abfrage bzw. `include_archived` bei MCP) und lassen sich jederzeit wiederherstellen.
//! * `maintenance_settings` – je Projekt: Zielstatus für „Als Won't Do schließen“, Schwellen (Tickets 90 Tage, Felder
//!   180 Tage, Duplikate 80 %), abgeschaltete Prüfungen (JSON-Liste der Prüf-Kennungen). Fehlt die Zeile, gelten die Vorgaben.
//! * `maintenance_ignored` – ignorierte Befunde (stabile Befund-Kennung je Projekt), wiederherstellbar.
//! * `maintenance_rules` – eigene Prüfregeln (gespeicherte Abfrage, Schwere, Aktionsvorschlag); aus, bis jemand eine anlegt.
//! * `maintenance_log` – Wartungsprotokoll (wann, was, wie viele, welche Sicherung). Wird nie automatisch gelöscht; beim
//!   Löschen eines Projekts bleibt der Eintrag mit dem Projektschlüssel stehen (`project_id` wird leer).
//!
//! Nur anhängen: diese Migration nach dem Erscheinen nie ändern, spätere Änderungen kommen als eigene Migration.

pub const MIGRATION: &str = "
ALTER TABLE tickets ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
CREATE INDEX tickets_archived ON tickets (project_id, archived);

CREATE TABLE maintenance_settings (
    project_id INTEGER PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
    wont_do_status_id INTEGER REFERENCES statuses (id) ON DELETE SET NULL,
    stale_days INTEGER NOT NULL DEFAULT 90 CHECK (stale_days BETWEEN 1 AND 3650),
    field_days INTEGER NOT NULL DEFAULT 180 CHECK (field_days BETWEEN 1 AND 3650),
    duplicate_percent INTEGER NOT NULL DEFAULT 80 CHECK (duplicate_percent BETWEEN 50 AND 100),
    disabled_checks TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE maintenance_ignored (
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    finding TEXT NOT NULL,
    ignored_at TEXT NOT NULL,
    PRIMARY KEY (project_id, finding)
) WITHOUT ROWID;

CREATE TABLE maintenance_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    jql TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'warning' CHECK (severity IN ('error', 'warning', 'hint')),
    action TEXT NOT NULL DEFAULT '{}',
    enabled INTEGER NOT NULL DEFAULT 1,
    position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX maintenance_rules_project ON maintenance_rules (project_id, position);

CREATE TABLE maintenance_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER REFERENCES projects (id) ON DELETE SET NULL,
    project_key TEXT NOT NULL DEFAULT '',
    at TEXT NOT NULL,
    action TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    backup TEXT,
    detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX maintenance_log_project ON maintenance_log (project_id, id);
";
