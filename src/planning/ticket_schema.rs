//! SQL der Migration 4: Tickets, Feldwerte, Verknüpfungen, Kommentare, Verlauf, Zeiteinträge und die Zuordnung von
//! umgewandelten Todo-Karten.
//!
//! Wertespeicherung (`ticket_values`): je Wert eine Zeile, Mehrfachwerte (Mehrfachauswahl, Labels) belegen mehrere Zeilen
//! (`pos`). Datum und Datum+Uhrzeit stehen als ISO-Text in `text`, Checkbox (0/1), Zahl und die Kennung eines Dropdown-/
//! Mehrfachauswahl-Werts in `num`, Verweise (Ticket, Version, Meilenstein: Kennung als Zahl-Text) und Terminal-Kennungen in
//! `text`.
//!
//! Hinweise zu den Indizes: `UNIQUE (project_id, number)` legt den Index auf `(project_id, number)` selbst an; für die
//! Suche nach der Kennung allein (Löschen, Integritätszähler) gibt es zusätzlich `tickets_status`.
//!
//! Nur anhängen: diese Migration nach dem Erscheinen nie ändern, spätere Änderungen kommen als eigene Migration.

pub const MIGRATION: &str = "
CREATE TABLE tickets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    type_id INTEGER NOT NULL REFERENCES ticket_types (id),
    title TEXT NOT NULL,
    status_id INTEGER REFERENCES statuses (id),
    parent_id INTEGER REFERENCES tickets (id) ON DELETE SET NULL,
    rank INTEGER NOT NULL DEFAULT 0,
    flagged INTEGER NOT NULL DEFAULT 0,
    starred INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (project_id, number)
);
CREATE INDEX tickets_project_status ON tickets (project_id, status_id);
CREATE INDEX tickets_status ON tickets (status_id);
CREATE INDEX tickets_type ON tickets (type_id);
CREATE INDEX tickets_parent ON tickets (parent_id);
CREATE INDEX tickets_rank ON tickets (project_id, rank);

CREATE TABLE ticket_values (
    ticket_id INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    field_id INTEGER NOT NULL REFERENCES fields (id) ON DELETE CASCADE,
    pos INTEGER NOT NULL DEFAULT 0,
    text TEXT,
    num REAL,
    PRIMARY KEY (ticket_id, field_id, pos)
) WITHOUT ROWID;
CREATE INDEX ticket_values_text ON ticket_values (field_id, text);
CREATE INDEX ticket_values_num ON ticket_values (field_id, num);

CREATE TABLE links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    link_type_id INTEGER NOT NULL REFERENCES link_types (id),
    from_ticket_id INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    to_kind TEXT NOT NULL CHECK (to_kind IN ('ticket', 'terminal', 'url', 'branch')),
    to_ticket_id INTEGER REFERENCES tickets (id) ON DELETE CASCADE,
    to_text TEXT,
    created_at TEXT NOT NULL,
    CHECK ((to_kind = 'ticket' AND to_ticket_id IS NOT NULL AND to_text IS NULL)
        OR (to_kind <> 'ticket' AND to_ticket_id IS NULL AND to_text IS NOT NULL))
);
CREATE INDEX links_from ON links (from_ticket_id);
CREATE INDEX links_to_ticket ON links (to_ticket_id) WHERE to_ticket_id IS NOT NULL;
CREATE INDEX links_type ON links (link_type_id);
CREATE INDEX links_text ON links (to_kind, to_text) WHERE to_text IS NOT NULL;
CREATE UNIQUE INDEX links_unique_ticket ON links (link_type_id, from_ticket_id, to_ticket_id) WHERE to_ticket_id IS NOT NULL;
CREATE UNIQUE INDEX links_unique_text ON links (link_type_id, from_ticket_id, to_kind, to_text) WHERE to_text IS NOT NULL;

CREATE TABLE comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX comments_ticket ON comments (ticket_id);

CREATE TABLE history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    at TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('ui', 'quick', 'agent', 'import')),
    source_detail TEXT,
    field TEXT NOT NULL,
    old TEXT,
    new TEXT,
    batch INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX history_ticket ON history (ticket_id, id);
CREATE INDEX history_source ON history (source, id);

CREATE TABLE time_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    start TEXT NOT NULL,
    minutes INTEGER NOT NULL CHECK (minutes > 0),
    work_kind_id INTEGER REFERENCES work_kinds (id) ON DELETE SET NULL,
    comment TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);
CREATE INDEX time_entries_ticket ON time_entries (ticket_id);
CREATE INDEX time_entries_start ON time_entries (start);
CREATE INDEX time_entries_work_kind ON time_entries (work_kind_id);

CREATE TABLE todo_conversions (
    todo_id INTEGER PRIMARY KEY REFERENCES todo_items (id) ON DELETE CASCADE,
    ticket_id INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE
);
CREATE INDEX todo_conversions_ticket ON todo_conversions (ticket_id);
";
