//! SQL der Migration 3: Konfiguration der Projekte (Projekte, Status, Typen, Felder, Workflows, Verknüpfungstypen,
//! Arten der Arbeit) samt den fest eingebauten Daten (System-Projekt „Todolist“, System-Verknüpfungstypen).
//!
//! Gegenüber dem Entwurf kleine Ergänzungen:
//! * `link_types.target_kind` – Beziehungspartner des Typs (`ticket`, `terminal`, `url`, `branch`); der Typ `terminal` zeigt auf eine Sitzung.
//! * `CHECK (key IS NOT NULL OR name IS NOT NULL)` – ein Eintrag ohne Schlüssel und ohne Namen hätte keine Anzeige.
//! * Teil-Unique-Indizes auf `(Eltern, key)`, damit ein Standard-Schlüssel je Projekt/Typ nur einmal vorkommt.
//!
//! Nur anhängen: diese Migration nach dem Erscheinen nie ändern, spätere Änderungen kommen als eigene Migration.

pub const MIGRATION: &str = "
CREATE TABLE projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE CHECK (length(key) BETWEEN 2 AND 10),
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL,
    work_dir TEXT NOT NULL DEFAULT '',
    use_sprints INTEGER NOT NULL DEFAULT 0,
    use_versions INTEGER NOT NULL DEFAULT 0,
    tag_color_mode TEXT NOT NULL DEFAULT 'default' CHECK (tag_color_mode IN ('default', 'accent', 'random')),
    system INTEGER NOT NULL DEFAULT 0,
    next_number INTEGER NOT NULL DEFAULT 1,
    position INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE statuses (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    key TEXT,
    name TEXT,
    color TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('todo', 'progress', 'done')),
    position INTEGER NOT NULL DEFAULT 0,
    CHECK (key IS NOT NULL OR name IS NOT NULL)
);
CREATE INDEX statuses_project ON statuses (project_id, position);
CREATE UNIQUE INDEX statuses_key ON statuses (project_id, key) WHERE key IS NOT NULL;

CREATE TABLE ticket_types (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    key TEXT,
    name TEXT,
    icon TEXT,
    color TEXT NOT NULL,
    workflow INTEGER NOT NULL DEFAULT 1,
    system INTEGER NOT NULL DEFAULT 0,
    position INTEGER NOT NULL DEFAULT 0,
    CHECK (key IS NOT NULL OR name IS NOT NULL)
);
CREATE INDEX ticket_types_project ON ticket_types (project_id, position);
CREATE UNIQUE INDEX ticket_types_key ON ticket_types (project_id, key) WHERE key IS NOT NULL;

CREATE TABLE type_parents (
    type_id INTEGER NOT NULL REFERENCES ticket_types (id) ON DELETE CASCADE,
    parent_type_id INTEGER NOT NULL REFERENCES ticket_types (id) ON DELETE CASCADE,
    PRIMARY KEY (type_id, parent_type_id)
) WITHOUT ROWID;
CREATE INDEX type_parents_parent ON type_parents (parent_type_id);

CREATE TABLE type_statuses (
    type_id INTEGER NOT NULL REFERENCES ticket_types (id) ON DELETE CASCADE,
    status_id INTEGER NOT NULL REFERENCES statuses (id) ON DELETE CASCADE,
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (type_id, status_id)
) WITHOUT ROWID;
CREATE INDEX type_statuses_status ON type_statuses (status_id);

CREATE TABLE transitions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type_id INTEGER NOT NULL REFERENCES ticket_types (id) ON DELETE CASCADE,
    from_status_id INTEGER NOT NULL REFERENCES statuses (id) ON DELETE CASCADE,
    to_status_id INTEGER NOT NULL REFERENCES statuses (id) ON DELETE CASCADE,
    required_fields TEXT NOT NULL DEFAULT '[]',
    conditions TEXT NOT NULL DEFAULT '[]',
    set_values TEXT NOT NULL DEFAULT '[]',
    UNIQUE (type_id, from_status_id, to_status_id),
    CHECK (from_status_id <> to_status_id)
);
CREATE INDEX transitions_from_status ON transitions (from_status_id);
CREATE INDEX transitions_to_status ON transitions (to_status_id);

CREATE TABLE fields (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type_id INTEGER NOT NULL REFERENCES ticket_types (id) ON DELETE CASCADE,
    key TEXT,
    name TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('text', 'rich', 'checkbox', 'dropdown', 'number', 'ticket', 'terminal', 'version', 'milestone', 'date', 'datetime', 'multi', 'labels')),
    options TEXT NOT NULL DEFAULT '{}',
    required INTEGER NOT NULL DEFAULT 0,
    default_value TEXT,
    help TEXT NOT NULL DEFAULT '',
    section TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL DEFAULT 0,
    show_on_card INTEGER NOT NULL DEFAULT 0,
    builtin INTEGER NOT NULL DEFAULT 0,
    CHECK (key IS NOT NULL OR name IS NOT NULL)
);
CREATE INDEX fields_type ON fields (type_id, position);
CREATE UNIQUE INDEX fields_key ON fields (type_id, key) WHERE key IS NOT NULL;

CREATE TABLE link_types (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT UNIQUE,
    name_out TEXT,
    name_in TEXT,
    symmetric INTEGER NOT NULL DEFAULT 0,
    system INTEGER NOT NULL DEFAULT 0,
    target_kind TEXT NOT NULL DEFAULT 'ticket' CHECK (target_kind IN ('ticket', 'terminal', 'url', 'branch')),
    position INTEGER NOT NULL DEFAULT 0,
    CHECK (key IS NOT NULL OR name_out IS NOT NULL)
);

CREATE TABLE work_kinds (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    key TEXT,
    name TEXT,
    position INTEGER NOT NULL DEFAULT 0,
    CHECK (key IS NOT NULL OR name IS NOT NULL)
);
CREATE INDEX work_kinds_project ON work_kinds (project_id, position);
CREATE UNIQUE INDEX work_kinds_key ON work_kinds (project_id, key) WHERE key IS NOT NULL;

-- System-Projekt „Todolist“ (Schlüssel TODO) mit dem System-Typ „todolist“ (grau, ohne Workflow, ohne Felder).
INSERT INTO projects (key, name, description, color, work_dir, use_sprints, use_versions, tag_color_mode, system, next_number, position, created_at)
VALUES ('TODO', 'Todolist', '', '#97A3B0', '', 0, 0, 'default', 1, 1, 0, strftime('%Y-%m-%dT%H:%M:%S', 'now', 'localtime'));
INSERT INTO ticket_types (project_id, key, name, icon, color, workflow, system, position)
VALUES ((SELECT id FROM projects WHERE key = 'TODO'), 'todolist', NULL, NULL, '#97A3B0', 0, 1, 0);

-- Globale Verknüpfungstypen. Nur `terminal` ist System (nie löschbar); die drei anderen sind Standard und löschbar.
-- Namen kommen aus der Übersetzungstabelle (names.rs), solange name_out/name_in leer sind.
INSERT INTO link_types (key, name_out, name_in, symmetric, system, target_kind, position) VALUES
    ('blocks', NULL, NULL, 0, 0, 'ticket', 0),
    ('relates', NULL, NULL, 1, 0, 'ticket', 1),
    ('duplicates', NULL, NULL, 0, 0, 'ticket', 2),
    ('terminal', NULL, NULL, 0, 1, 'terminal', 3);
";
