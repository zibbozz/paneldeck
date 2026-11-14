//! SQL der Migration 7: Sprints (samt Zugehörigkeit der Tickets), Versionen und Meilensteine je Projekt.
//!
//! Gegenüber dem Entwurf kleine Ergänzungen:
//! * `sprints.started_at` – Zeitpunkt des Starts (Zusage des Umfangs): Tickets, die davor hinzukamen, zählen zum Startumfang,
//!   die danach zum nachträglich hinzugekommenen Umfang.
//! * `sprint_tickets.id` und kein zusammengesetzter Schlüssel: ein Ticket kann denselben Sprint verlassen und später
//!   wieder betreten (mehrere Zeilen). Aktuell dabei ist, wer `removed_at IS NULL` hat.
//! * `versions.released_at` / `milestones.reached_at` – wann der Zustand gesetzt wurde.
//! * Ein Teil-Unique-Index lässt je Projekt höchstens einen aktiven Sprint zu.
//!
//! Dass ein Ticket höchstens in einem **offenen** (geplanten oder aktiven) Sprint steckt, ist über mehrere Tabellen verteilt und
//! wird deshalb im Code geprüft (`sprint.rs`). Zeilen abgeschlossener Sprints bleiben als Historie bestehen.
//!
//! Nur anhängen: diese Migration nach dem Erscheinen nie ändern, spätere Änderungen kommen als eigene Migration.

pub const MIGRATION: &str = "
CREATE TABLE sprints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    goal TEXT NOT NULL DEFAULT '',
    start_date TEXT,
    end_date TEXT,
    state TEXT NOT NULL DEFAULT 'planned' CHECK (state IN ('planned', 'active', 'completed')),
    default_days INTEGER NOT NULL DEFAULT 10 CHECK (default_days BETWEEN 1 AND 366),
    created_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    CHECK (state = 'planned' OR (start_date IS NOT NULL AND end_date IS NOT NULL)),
    CHECK (start_date IS NULL OR end_date IS NULL OR end_date >= start_date)
);
CREATE INDEX sprints_project ON sprints (project_id, state);
CREATE UNIQUE INDEX sprints_one_active ON sprints (project_id) WHERE state = 'active';

CREATE TABLE sprint_tickets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sprint_id INTEGER NOT NULL REFERENCES sprints (id) ON DELETE CASCADE,
    ticket_id INTEGER NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    added_at TEXT NOT NULL,
    removed_at TEXT,
    points_at_add REAL
);
CREATE UNIQUE INDEX sprint_tickets_open ON sprint_tickets (sprint_id, ticket_id) WHERE removed_at IS NULL;
CREATE INDEX sprint_tickets_ticket ON sprint_tickets (ticket_id);

CREATE TABLE versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    due_date TEXT,
    released INTEGER NOT NULL DEFAULT 0,
    released_at TEXT,
    position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX versions_project ON versions (project_id, position);

CREATE TABLE milestones (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    due_date TEXT,
    reached INTEGER NOT NULL DEFAULT 0,
    reached_at TEXT,
    position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX milestones_project ON milestones (project_id, position);
";
