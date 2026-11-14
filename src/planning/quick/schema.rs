//! SQL der Migration für die Quick-Actions (Schnellaktionen) eines Tickettyps.
//!
//! * `type_id` hängt die Aktion an den Typ, von dem aus sie im Ticket erscheint (Löschen des Typs löscht sie mit).
//! * `key`/`name` folgen dem Muster der übrigen Konfiguration: Standard-Schlüssel (übersetzbar) und/oder eigener Name
//!   (= Knopftext, gilt für beide Sprachen).
//! * `action` ist die Aktionsart, `params` ihre Parameter als JSON mit Format-Version (siehe `params.rs`).
//! * `invalid` markiert eine Aktion, deren Verweise nicht mehr stimmen (z. B. gelöschter Ziel-Typ); `prune_quick_action_refs`
//!   setzt und löscht die Markierung.
//!
//! Nur anhängen: diese Migration nach dem Erscheinen nie ändern, spätere Änderungen kommen als eigene Migration.

pub const MIGRATION: &str = "
CREATE TABLE quick_actions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type_id INTEGER NOT NULL REFERENCES ticket_types (id) ON DELETE CASCADE,
    key TEXT,
    name TEXT,
    action TEXT NOT NULL CHECK (action IN ('create_ticket', 'change_status', 'start_session')),
    params TEXT NOT NULL DEFAULT '{}',
    position INTEGER NOT NULL DEFAULT 0,
    invalid INTEGER NOT NULL DEFAULT 0,
    CHECK (key IS NOT NULL OR name IS NOT NULL)
);
CREATE INDEX quick_actions_type ON quick_actions (type_id, position);
CREATE UNIQUE INDEX quick_actions_key ON quick_actions (type_id, key) WHERE key IS NOT NULL;
";
