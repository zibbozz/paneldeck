//! Eingebaute Felder jedes Tickets (jeder Typ außer dem System-Typ): **Beschreibung** (Rich-Text, optional) und **Fälligkeit**
//! (Datum mit optionaler Uhrzeit, optional). Der Titel ist kein Feld, sondern eine Spalte des Tickets.
//!
//! * Beide stehen als gewöhnliche Zeilen in `fields` (Schlüssel `description` bzw. `due`, `builtin = 1`). Damit gelten
//!   Werte, Verlauf, Rückgängig, Übergangsregeln, Quick-Actions, Boards, Suche, Sicherung und CSV unverändert.
//! * **Fest:** Ein Feld mit einem der beiden Schlüssel und `builtin = 1` ist nicht löschbar, Name/Optionen/Pflicht/Standard
//!   sind nicht änderbar ([`super::fields::Field::is_fixed`]); es steht immer oben (Positionen -2 und -1).
//! * [`ENSURE_SQL`] legt die Felder für jeden Typ an, dem sie fehlen, und wandelt bestehende um (idempotent). Die Migration 8
//!   führt sie einmal aus; danach rufen `create_type` und die Vorlagen-/Sicherungs-Importe [`ensure`] auf.
//!
//! Umwandlung beim Update (Migration 8):
//! 1. Ein Feld mit Schlüssel `description` und Feldtyp Text oder Rich-Text **wird** die eingebaute Beschreibung (Feldtyp
//!    Rich-Text, nicht Pflicht); Werte, Verlauf und Verweise in Regeln bleiben, weil die Feld-Kennung gleich bleibt.
//! 2. Hat ein Typ kein solches Feld, wird das erste eigene Text-/Rich-Text-Feld namens „Description“/„Beschreibung“ zur
//!    eingebauten Beschreibung.
//! 3. Ein Feld mit Schlüssel `description`/`due` anderen Feldtyps verliert den Schlüssel und bleibt als eigenes Feld.
//! 4. Ein eigenes Feld wie „Due date“ bleibt ein normales eigenes Feld (kein Zusammenlegen mit der Fälligkeit).
//! 5. Projekte mit den mitgelieferten Typen (Schlüssel epic/userstory/devtask/subtask/defect) bekommen die Standard-Schwellen
//!    der Fälligkeits-Markierung, alle anderen keine. Todo-Karten bekommen die Spalte `due`.

use rusqlite::Connection;

/// Schlüssel der eingebauten Beschreibung.
pub const DESCRIPTION_KEY: &str = "description";
/// Schlüssel der eingebauten Fälligkeit.
pub const DUE_KEY: &str = "due";
/// Schlüssel der eingebauten Felder.
pub const FIXED_KEYS: [&str; 2] = [DESCRIPTION_KEY, DUE_KEY];

/// Optionen der Fälligkeit (Datum+Uhrzeit, Uhrzeit optional).
pub const DUE_OPTIONS_JSON: &str = r#"{"time_optional":true}"#;

macro_rules! ensure_sql {
    () => {
        "
-- (3) Schlüssel description/due bei unpassendem Feldtyp: wird ein eigenes Feld
UPDATE fields SET key = NULL, name = COALESCE(name, 'Description (custom)'), builtin = 0
 WHERE key = 'description' AND kind NOT IN ('rich', 'text');
UPDATE fields SET key = NULL, name = COALESCE(name, 'Due date (custom)'), builtin = 0
 WHERE key = 'due' AND kind <> 'datetime';
-- (2) eigenes Feld „Description“/„Beschreibung“ in Typen ohne Beschreibungsfeld
UPDATE fields SET key = 'description', name = NULL
 WHERE id IN (
   SELECT MIN(f.id) FROM fields f JOIN ticket_types t ON t.id = f.type_id
    WHERE t.system = 0 AND f.key IS NULL AND f.kind IN ('rich', 'text') AND lower(trim(f.name)) IN ('description', 'beschreibung')
      AND NOT EXISTS (SELECT 1 FROM fields g WHERE g.type_id = f.type_id AND g.key = 'description')
    GROUP BY f.type_id);
-- (1) vorhandene Beschreibungs-/Fälligkeitsfelder festschreiben
UPDATE fields SET options = CASE WHEN kind = 'text' THEN '{\"rows\":8}' ELSE options END
 WHERE key = 'description' AND type_id IN (SELECT id FROM ticket_types WHERE system = 0);
UPDATE fields SET kind = 'rich', builtin = 1, required = 0, position = -2
 WHERE key = 'description' AND type_id IN (SELECT id FROM ticket_types WHERE system = 0);
UPDATE fields SET options = '{\"time_optional\":true}', builtin = 1, required = 0, position = -1
 WHERE key = 'due' AND type_id IN (SELECT id FROM ticket_types WHERE system = 0);
-- fehlende anlegen
INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'description', NULL, 'rich', '{\"rows\":8}', 0, NULL, '', '', -2, 0, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'description');
INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'due', NULL, 'datetime', '{\"time_optional\":true}', 0, NULL, '', '', -1, 0, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'due');
"
    };
}

/// Legt die eingebauten Felder für alle Typen an bzw. wandelt sie um (idempotent).
pub const ENSURE_SQL: &str = ensure_sql!();

/// SQL der Migration 8: Spalten für Schwellen und Todo-Fälligkeit, eingebaute Felder, Standard-Schwellen.
/// Nur anhängen: diese Migration nach dem Erscheinen nie ändern.
pub const MIGRATION: &str = concat!(
    "
ALTER TABLE projects ADD COLUMN due_thresholds TEXT NOT NULL DEFAULT '[]';
ALTER TABLE todo_items ADD COLUMN due TEXT;
CREATE INDEX todo_items_due ON todo_items (due) WHERE due IS NOT NULL;
UPDATE projects SET due_thresholds = '",
    r##"[{"under_minutes":10080,"marker":{"frame":"triangle","symbol":"alert","color":"#D8B94A"},"tint":false},{"under_minutes":1440,"marker":{"frame":"triangle","symbol":"alert","color":"#E06C75"},"tint":false}]"##,
    "'
 WHERE system = 0 AND EXISTS (SELECT 1 FROM ticket_types t WHERE t.project_id = projects.id AND t.key IN ('epic', 'userstory', 'devtask', 'subtask', 'defect'));
",
    ensure_sql!()
);

/// Eingebaute Felder für alle Typen sicherstellen (nach dem Anlegen eines Typs und nach Importen).
pub fn ensure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(ENSURE_SQL)
}

/// Wird ein Feld einer Vorlage/Sicherung (Schlüssel, Feldtyp) zum eingebauten Feld? Liefert dessen Schlüssel.
pub fn template_fits(key: Option<&str>, kind: super::fields::FieldKind) -> Option<&'static str> {
    use super::fields::FieldKind;
    match key? {
        DESCRIPTION_KEY if matches!(kind, FieldKind::Rich | FieldKind::Text) => Some(DESCRIPTION_KEY),
        DUE_KEY if kind == FieldKind::Datetime => Some(DUE_KEY),
        _ => None,
    }
}

/// Hat das Projekt die mitgelieferten Typen (Schlüssel epic/userstory/devtask/subtask/defect)?
pub fn has_preset_types(conn: &Connection, project_id: i64) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM ticket_types WHERE project_id = ?1 AND key IN ('epic', 'userstory', 'devtask', 'subtask', 'defect'))",
        [project_id],
        |r| r.get(0),
    )
}

/// Fälligkeiten aller Tickets eines Projekts (Ticket-Kennung → gespeicherter Text), für Listen und Boards in einer Abfrage.
pub fn due_map(conn: &Connection, project_id: i64) -> rusqlite::Result<std::collections::HashMap<i64, String>> {
    let mut st = conn.prepare(
        "SELECT v.ticket_id, v.text FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND f.key = 'due' AND f.builtin = 1 AND v.text IS NOT NULL",
    )?;
    let rows = st.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    rows.collect()
}

/// Zählung für die Übersicht: (überfällig, markiert) der offenen Tickets (Status nicht „Erledigt“) eines Projekts.
pub fn due_counts(conn: &Connection, project_id: i64, now: chrono::NaiveDateTime) -> super::Res<(usize, usize)> {
    let th = super::projects::due_thresholds(conn, project_id)?;
    let mut st = conn.prepare(
        "SELECT v.text FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         LEFT JOIN statuses s ON s.id = t.status_id
         WHERE t.project_id = ?1 AND f.key = 'due' AND f.builtin = 1 AND v.text IS NOT NULL AND COALESCE(s.category, '') <> 'done'",
    )?;
    let dues = st.query_map([project_id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(crate::due::count(&th, dues, now))
}

/// Kennung der eingebauten Beschreibung bzw. Fälligkeit eines Typs (`None` beim System-Typ).
pub fn field_id(conn: &Connection, type_id: i64, key: &str) -> rusqlite::Result<Option<i64>> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT id FROM fields WHERE type_id = ?1 AND key = ?2 AND builtin = 1", rusqlite::params![type_id, key], |r| r.get(0)).optional()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{list_fields, FieldKind};
    use crate::planning::projects::{create_project, due_thresholds, NewProject};
    use crate::planning::testutil::db;
    use crate::planning::types::{create_type, list_types, NewType};

    #[test]
    fn the_migration_sql_and_ensure_agree_and_are_idempotent() {
        assert!(MIGRATION.contains(ENSURE_SQL));
        assert!(MIGRATION.contains(crate::due::DEFAULT_THRESHOLDS_JSON));
        assert!(ENSURE_SQL.contains(DUE_OPTIONS_JSON));
        let c = db();
        let p = create_project(&c, &NewProject::new("AA", "P", "#112233")).unwrap();
        let t = create_type(&c, &NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let before = list_fields(&c, t).unwrap();
        assert_eq!(before.iter().map(|f| f.key.clone().unwrap()).collect::<Vec<_>>(), vec!["description", "due"]);
        assert!(before.iter().all(|f| f.is_fixed() && !f.required));
        assert_eq!(before[0].kind, FieldKind::Rich);
        assert_eq!((before[1].kind, before[1].options.time_optional), (FieldKind::Datetime, Some(true)));
        for _ in 0..3 {
            ensure(&c).unwrap();
        }
        assert_eq!(list_fields(&c, t).unwrap(), before, "mehrfaches Ausführen ändert nichts");
        // System-Typ (Todolist) bekommt keine Felder
        let sys: i64 = c.query_row("SELECT id FROM ticket_types WHERE system = 1", [], |r| r.get(0)).unwrap();
        assert!(list_fields(&c, sys).unwrap().is_empty());
        assert_eq!(field_id(&c, t, DUE_KEY).unwrap(), Some(before[1].id));
        assert_eq!(field_id(&c, sys, DUE_KEY).unwrap(), None);
        // ohne Standards keine Schwellen, mit Standards die Standard-Schwellen
        assert!(due_thresholds(&c, p).unwrap().is_empty());
        let mut np = NewProject::new("BB", "Q", "#112233");
        np.with_defaults = true;
        let q = create_project(&c, &np).unwrap();
        assert_eq!(due_thresholds(&c, q).unwrap(), crate::due::default_thresholds());
        for ty in list_types(&c, q).unwrap() {
            let keys: Vec<String> = list_fields(&c, ty.id).unwrap().iter().take(2).map(|f| f.key.clone().unwrap()).collect();
            assert_eq!(keys, vec!["description", "due"], "{:?}", ty.key);
        }
    }
}
