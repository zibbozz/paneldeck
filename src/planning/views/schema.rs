//! SQL der Migration für die Konfiguration der Ansichten: Boards, gespeicherte Filter, JQL-Schlüsselwörter, Dashboards.
//!
//! * `boards.config` und `dashboards.layout` sind JSON mit Format-Version (Struktur in `boards.rs` bzw. `dashboards.rs`).
//! * Namen sind je Bereich (Projekt bzw. global) eindeutig, ohne Groß-/Kleinschreibung (Unique-Indizes über `COALESCE`,
//!   weil `NULL` in einem Unique-Index sonst nie kollidiert). Die Unicode-genaue Prüfung macht der Code; der Index ist das Netz.
//! * Höchstens ein Standard-Dashboard je Bereich garantiert der Teil-Unique-Index `dashboards_default`.
//! * Die vier eingebauten JQL-Schlüsselwörter werden hier angelegt (`builtin = 1`).
//!
//! Nur anhängen: diese Migration nach dem Erscheinen nie ändern, spätere Änderungen kommen als eigene Migration.

pub const SQL: &str = r#"
CREATE TABLE saved_filters (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    jql TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX saved_filters_project ON saved_filters (project_id, position);
CREATE UNIQUE INDEX saved_filters_name ON saved_filters (COALESCE(project_id, 0), name COLLATE NOCASE);

CREATE TABLE boards (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL DEFAULT 0,
    config TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX boards_project ON boards (project_id, position);
CREATE UNIQUE INDEX boards_name ON boards (project_id, name COLLATE NOCASE);

CREATE TABLE jql_keywords (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind TEXT NOT NULL CHECK (kind IN ('predicate', 'sort')),
    builtin INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    definition TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE dashboards (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('project', 'global')),
    project_id INTEGER REFERENCES projects (id) ON DELETE CASCADE,
    position INTEGER NOT NULL DEFAULT 0,
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    layout TEXT NOT NULL DEFAULT '{}',
    CHECK ((scope = 'global' AND project_id IS NULL) OR (scope = 'project' AND project_id IS NOT NULL))
);
CREATE INDEX dashboards_project ON dashboards (project_id, position);
CREATE UNIQUE INDEX dashboards_name ON dashboards (COALESCE(project_id, 0), name COLLATE NOCASE);
CREATE UNIQUE INDEX dashboards_default ON dashboards (COALESCE(project_id, 0)) WHERE is_default = 1;

-- Eingebaute JQL-Schlüsselwörter: Prädikate `blocked` (markiert) und `favorite` (Stern), Sortierschlüssel `favorites` und `flagged`.
-- Beschreibung NULL = mitgelieferter, übersetzter Text.
INSERT INTO jql_keywords (name, kind, builtin, description, definition, position) VALUES
    ('blocked', 'predicate', 1, NULL, '{"type":"builtin","id":"blocked"}', 0),
    ('favorite', 'predicate', 1, NULL, '{"type":"builtin","id":"favorite"}', 1),
    ('favorites', 'sort', 1, NULL, '{"type":"builtin","id":"favorites"}', 2),
    ('flagged', 'sort', 1, NULL, '{"type":"builtin","id":"flagged"}', 3);
"#;
