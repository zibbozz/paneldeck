//! Eingebaute Felder jedes Tickets (jeder Typ außer dem System-Typ): **Beschreibung** (Rich-Text, optional), **Fälligkeit**
//! (Datum mit optionaler Uhrzeit, optional) und **Labels** (mehrere Texte, optional). Der Titel ist kein Feld, sondern eine
//! Spalte des Tickets.
//!
//! * Alle drei stehen als gewöhnliche Zeilen in `fields` (Schlüssel `description`, `due`, `labels`, `builtin = 1`). Damit gelten
//!   Werte, Verlauf, Rückgängig, Übergangsregeln, Quick-Actions, Boards, Suche, Sicherung und CSV unverändert.
//! * **Fest:** Ein Feld mit einem dieser Schlüssel und `builtin = 1` ist nicht löschbar, Name/Optionen/Pflicht/Standard sind nicht
//!   änderbar ([`super::fields::Field::is_fixed`]); sie stehen immer oben (Positionen -3, -2, -1).
//! * Umwandlungen bestehender Felder macht nur die jeweilige Migration (eingefrorenes SQL, [`MIGRATION`] = 8 für Beschreibung und
//!   Fälligkeit, [`MIGRATION_LABELS`] = 9 für Labels). Danach legt [`ensure`] nur noch fehlende Felder an (neue Typen, Importe,
//!   von außen eingefügte Typen) – billig geprüft und in einer Transaktion.
//!
//! Umwandlung beim Update (Migration 8 und 9, je Feld):
//! 1. Ein Feld mit dem Schlüssel und passendem Feldtyp **wird** das eingebaute Feld (Beschreibung: Text/Rich-Text → Rich-Text;
//!    Fälligkeit: Datum+Uhrzeit; Labels: Labels), nicht Pflicht und ohne Standardwert (eingebaute Felder haben keinen, er wäre
//!    danach weder änderbar noch Teil einer Sicherung); Werte, Verlauf und Verweise in Regeln bleiben, weil die Feld-Kennung
//!    gleich bleibt.
//! 2. Hat ein Typ kein solches Feld, wird das erste eigene Feld passender Art namens „Description“/„Beschreibung“ bzw. „Labels“
//!    zum eingebauten Feld.
//! 3. Ein Feld mit einem dieser Schlüssel und anderem Feldtyp verliert den Schlüssel und bleibt als eigenes Feld.
//! 4. Eigene Felder, deren Name danach mit dem eines eingebauten Feldes im selben Typ gleich wäre (z. B. „Due date“), heißen
//!    „… (custom)“ und bleiben eigene Felder.
//! 5. Projekte mit den mitgelieferten Typen (Schlüssel epic/userstory/devtask/subtask/defect) bekommen die Standard-Schwellen
//!    der Fälligkeits-Markierung, alle anderen keine. Todo-Karten bekommen die Spalte `due`.
//!
//! Text-Werte einer zu Rich-Text gewordenen Beschreibung werden nicht umgeschrieben: der Markdown-Leser kennt keine
//! Backslash-Maskierung, Sonderzeichen wie `*` erscheinen daher ggf. als Formatierung.

use rusqlite::Connection;

/// Schlüssel der eingebauten Beschreibung.
pub const DESCRIPTION_KEY: &str = "description";
/// Schlüssel der eingebauten Fälligkeit.
pub const DUE_KEY: &str = "due";
/// Schlüssel der eingebauten Labels.
pub const LABELS_KEY: &str = "labels";
/// Schlüssel der eingebauten Felder (in der Reihenfolge im Typ).
pub const FIXED_KEYS: [&str; 3] = [DESCRIPTION_KEY, DUE_KEY, LABELS_KEY];

/// Optionen der Fälligkeit (Datum+Uhrzeit, Uhrzeit optional).
pub const DUE_OPTIONS_JSON: &str = r#"{"time_optional":true}"#;

/// SQL der Migration 8: Spalten für Schwellen und Todo-Fälligkeit, eingebaute Beschreibung und Fälligkeit, Standard-Schwellen.
/// Eingefroren: nach dem Erscheinen nie ändern (spätere Änderungen kommen als eigene Migration).
pub const MIGRATION: &str = r##"
ALTER TABLE projects ADD COLUMN due_thresholds TEXT NOT NULL DEFAULT '[]';
ALTER TABLE todo_items ADD COLUMN due TEXT;
CREATE INDEX todo_items_due ON todo_items (due) WHERE due IS NOT NULL;
UPDATE projects SET due_thresholds = '[{"under_minutes":10080,"marker":{"frame":"triangle","symbol":"alert","color":"#D8B94A"},"tint":false},{"under_minutes":1440,"marker":{"frame":"triangle","symbol":"alert","color":"#E06C75"},"tint":false}]'
 WHERE system = 0 AND EXISTS (SELECT 1 FROM ticket_types t WHERE t.project_id = projects.id AND t.key IN ('epic', 'userstory', 'devtask', 'subtask', 'defect'));
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
UPDATE fields SET options = CASE WHEN kind = 'text' THEN '{"rows":8}' ELSE options END
 WHERE key = 'description' AND type_id IN (SELECT id FROM ticket_types WHERE system = 0);
UPDATE fields SET kind = 'rich', builtin = 1, required = 0, default_value = NULL, position = -3
 WHERE key = 'description' AND type_id IN (SELECT id FROM ticket_types WHERE system = 0);
UPDATE fields SET options = '{"time_optional":true}', builtin = 1, required = 0, default_value = NULL, position = -2
 WHERE key = 'due' AND type_id IN (SELECT id FROM ticket_types WHERE system = 0);
-- fehlende anlegen
INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'description', NULL, 'rich', '{"rows":8}', 0, NULL, '', '', -3, 0, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'description');
INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'due', NULL, 'datetime', '{"time_optional":true}', 0, NULL, '', '', -2, 0, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'due');
-- (4) eigene Felder, die jetzt wie ein eingebautes heißen, bekommen den Zusatz „(custom)“
UPDATE fields SET name = name || ' (custom)'
 WHERE key IS NULL AND lower(trim(name)) IN ('description', 'beschreibung')
   AND type_id IN (SELECT type_id FROM fields WHERE key = 'description' AND builtin = 1);
UPDATE fields SET name = name || ' (custom)'
 WHERE key IS NULL AND lower(trim(name)) IN ('due date', 'fällig am')
   AND type_id IN (SELECT type_id FROM fields WHERE key = 'due' AND builtin = 1);
"##;

/// SQL der Migration 9: eingebaute Labels (Issue #195). Eingefroren wie [`MIGRATION`].
pub const MIGRATION_LABELS: &str = r##"
-- (3) Schlüssel labels bei unpassendem Feldtyp: wird ein eigenes Feld
UPDATE fields SET key = NULL, name = COALESCE(name, 'Labels (custom)'), builtin = 0
 WHERE key = 'labels' AND kind <> 'labels';
-- (2) eigenes Labels-Feld namens „Labels“ in Typen ohne Labels-Feld mit Schlüssel (der Name bleibt als Anzeigename)
UPDATE fields SET key = 'labels', name = NULL
 WHERE id IN (
   SELECT MIN(f.id) FROM fields f JOIN ticket_types t ON t.id = f.type_id
    WHERE t.system = 0 AND f.key IS NULL AND f.kind = 'labels' AND lower(trim(f.name)) = 'labels'
      AND NOT EXISTS (SELECT 1 FROM fields g WHERE g.type_id = f.type_id AND g.key = 'labels')
    GROUP BY f.type_id);
-- (1) festschreiben (oben, nicht Pflicht; „auf Karte“ bleibt, wie es war)
UPDATE fields SET builtin = 1, required = 0, default_value = NULL, options = '{}', position = -1
 WHERE key = 'labels' AND type_id IN (SELECT id FROM ticket_types WHERE system = 0);
-- fehlende anlegen (auf der Karte sichtbar)
INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'labels', NULL, 'labels', '{}', 0, NULL, '', '', -1, 1, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'labels');
-- (4) gleichnamige eigene Felder (anderer Art, oder ein zweites „Labels“) bekommen den Zusatz „(custom)“
UPDATE fields SET name = name || ' (custom)'
 WHERE key IS NULL AND lower(trim(name)) = 'labels'
   AND type_id IN (SELECT type_id FROM fields WHERE key = 'labels' AND builtin = 1);
"##;

/// Eingebaute Felder mit der Schema-Version, ab der es sie gibt, und dem SQL, das sie für jeden Typ ohne sie anlegt.
const INSERTS: [(&str, u32, &str); 3] = [
    (
        DESCRIPTION_KEY,
        8,
        r##"INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'description', NULL, 'rich', '{"rows":8}', 0, NULL, '', '', -3, 0, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'description')"##,
    ),
    (
        DUE_KEY,
        8,
        r##"INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'due', NULL, 'datetime', '{"time_optional":true}', 0, NULL, '', '', -2, 0, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'due')"##,
    ),
    (
        LABELS_KEY,
        9,
        r##"INSERT INTO fields (type_id, key, name, kind, options, required, default_value, help, section, position, show_on_card, builtin)
 SELECT t.id, 'labels', NULL, 'labels', '{}', 0, NULL, '', '', -1, 1, 1 FROM ticket_types t
  WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = 'labels')"##,
    ),
];

/// Fehlt einem Typ das eingebaute Feld `?1`? (billig, für den Aufruf beim Öffnen)
const MISSING_SQL: &str = "SELECT EXISTS (SELECT 1 FROM ticket_types t WHERE t.system = 0 AND NOT EXISTS (SELECT 1 FROM fields f WHERE f.type_id = t.id AND f.key = ?1))";

/// Fehlende eingebaute Felder für alle Typen anlegen (nach dem Anlegen eines Typs, nach Importen, beim Öffnen) – nur die, deren
/// Migration schon gelaufen ist (`PRAGMA user_version`; ein Feld vorher anzulegen würde die Umwandlung gleichnamiger eigener
/// Felder in der späteren Migration verhindern). Prüft zuerst billig, ob überhaupt etwas fehlt, und schreibt dann in einem
/// SAVEPOINT (auch innerhalb einer äußeren Transaktion).
pub fn ensure(conn: &Connection) -> rusqlite::Result<()> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let mut todo: Vec<&str> = Vec::new();
    for (key, since, sql) in INSERTS {
        if version >= since && conn.query_row(MISSING_SQL, [key], |r| r.get::<_, bool>(0))? {
            todo.push(sql);
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    conn.execute_batch("SAVEPOINT ensure_builtin_fields")?;
    for sql in todo {
        if let Err(e) = conn.execute_batch(sql) {
            let _ = conn.execute_batch("ROLLBACK TO ensure_builtin_fields; RELEASE ensure_builtin_fields");
            return Err(e);
        }
    }
    conn.execute_batch("RELEASE ensure_builtin_fields")
}

/// Nachtrag zu Schritt 4 der Migrationen 8 und 9 (#221): Deren SQL vergleicht mit `lower()`, das in SQLite nur ASCII kennt –
/// ein eigenes Feld „FÄLLIG AM“ behielt so seinen Namen und hieß danach wie die eingebaute Fälligkeit. Hier wird mit der
/// Groß-/Kleinschreibung von Unicode verglichen (wie beim Anlegen von Feldern und beim Import von Vorlagen) und wie in der
/// Migration „ (custom)“ angehängt – nur in Typen, die das eingebaute Feld schon haben, und nur für Felder, deren Migration
/// gelaufen ist. Idempotent (ein umbenanntes Feld passt nicht mehr); neue gleichnamige Felder lässt die Oberfläche ohnehin
/// nicht zu. Gibt die Zahl der umbenannten Felder zurück.
pub fn rename_clashing_custom(conn: &Connection) -> rusqlite::Result<usize> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let since = |key: &str| INSERTS.iter().find(|(k, _, _)| *k == key).map_or(u32::MAX, |(_, v, _)| *v);
    if INSERTS.iter().all(|(_, v, _)| version < *v) {
        return Ok(0); // vor Migration 8 (noch keine eingebauten Felder, ggf. noch keine Tabelle `fields`)
    }
    let mut todo: Vec<(i64, String)> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT f.id, f.name, g.key FROM fields f JOIN fields g ON g.type_id = f.type_id AND g.builtin = 1
              WHERE f.key IS NULL AND f.name IS NOT NULL AND g.key IN ('description', 'due', 'labels') ORDER BY f.id",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        for row in rows {
            let (id, name, key) = row?;
            if version < since(&key) || todo.iter().any(|(i, _)| *i == id) {
                continue;
            }
            let n = name.trim().to_lowercase();
            let clash = [crate::state::Lang::En, crate::state::Lang::De].iter().any(|l| super::names::display_name(Some(&key), None, *l).to_lowercase() == n);
            if clash {
                todo.push((id, format!("{name} (custom)")));
            }
        }
    }
    if todo.is_empty() {
        return Ok(0);
    }
    conn.execute_batch("SAVEPOINT rename_clashing_custom")?;
    for (id, name) in &todo {
        if let Err(e) = conn.execute("UPDATE fields SET name = ?1 WHERE id = ?2", rusqlite::params![name, id]) {
            let _ = conn.execute_batch("ROLLBACK TO rename_clashing_custom; RELEASE rename_clashing_custom");
            return Err(e);
        }
    }
    conn.execute_batch("RELEASE rename_clashing_custom")?;
    Ok(todo.len())
}

/// Passt ein Feldtyp zum eingebauten Feld?
fn kind_fits(key: &str, kind: super::fields::FieldKind) -> bool {
    use super::fields::FieldKind;
    match key {
        DESCRIPTION_KEY => matches!(kind, FieldKind::Rich | FieldKind::Text),
        DUE_KEY => kind == FieldKind::Datetime,
        LABELS_KEY => kind == FieldKind::Labels,
        _ => false,
    }
}

/// Namen eigener Felder, die beim Update (und beim Import älterer Vorlagen) zum eingebauten Feld werden.
fn legacy_names(key: &str) -> &'static [&'static str] {
    match key {
        DESCRIPTION_KEY => &["description", "beschreibung"],
        LABELS_KEY => &["labels"],
        _ => &[],
    }
}

/// Heißt ein eigenes Feld wie ein eingebautes (in einer der Sprachen)? Dann bekommt es beim Update/Import „(custom)“.
pub fn is_builtin_label(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    FIXED_KEYS.iter().any(|k| [crate::state::Lang::De, crate::state::Lang::En].iter().any(|l| super::names::display_name(Some(k), None, *l).to_lowercase() == n))
}

/// Welche Felder eines Typs aus einer Vorlage/Sicherung (Schlüssel, Name, Feldtyp) zu welchem eingebauten Feld gehören – nach
/// denselben Regeln wie beim Update: zuerst ein Feld mit dem Schlüssel und passendem Feldtyp, sonst das erste eigene Feld
/// passender Art mit einem der alten Namen („Description“/„Beschreibung“, „Labels“).
pub fn builtin_targets(fields: &[(Option<&str>, Option<&str>, super::fields::FieldKind)]) -> Vec<Option<&'static str>> {
    let mut out: Vec<Option<&'static str>> = vec![None; fields.len()];
    for k in FIXED_KEYS {
        let keyed = fields.iter().position(|(key, _, kind)| *key == Some(k) && kind_fits(k, *kind));
        let by_name = || fields.iter().enumerate().position(|(i, (key, name, kind))| out[i].is_none() && key.is_none() && kind_fits(k, *kind) && name.is_some_and(|n| legacy_names(k).contains(&n.trim().to_lowercase().as_str())));
        if let Some(i) = keyed.or_else(by_name) {
            out[i] = Some(k);
        }
    }
    out
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
    fn new_types_get_all_built_in_fields_and_ensure_is_idempotent() {
        assert!(MIGRATION.contains(crate::due::DEFAULT_THRESHOLDS_JSON));
        assert!(MIGRATION.contains(DUE_OPTIONS_JSON) && INSERTS[1].2.contains(DUE_OPTIONS_JSON));
        let c = db();
        let p = create_project(&c, &NewProject::new("AA", "P", "#112233")).unwrap();
        let t = create_type(&c, &NewType { project_id: p, key: None, name: Some("T".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let before = list_fields(&c, t).unwrap();
        assert_eq!(before.iter().map(|f| f.key.clone().unwrap()).collect::<Vec<_>>(), vec!["description", "due", "labels"]);
        assert_eq!((before[2].kind, before[2].show_on_card), (FieldKind::Labels, true));
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
            let keys: Vec<String> = list_fields(&c, ty.id).unwrap().iter().take(3).map(|f| f.key.clone().unwrap()).collect();
            assert_eq!(keys, vec!["description", "due", "labels"], "{:?}", ty.key);
        }
        // ein von außen eingefügter Typ (ohne Felder) bekommt sie beim nächsten `ensure`, in einer Transaktion des Aufrufers
        c.execute("INSERT INTO ticket_types (project_id, key, name, color, workflow, system, position) VALUES (?1, NULL, 'Roh', '#112233', 0, 0, 9)", [p]).unwrap();
        let raw = c.last_insert_rowid();
        assert!(list_fields(&c, raw).unwrap().is_empty());
        c.execute_batch("BEGIN").unwrap();
        ensure(&c).unwrap();
        c.execute_batch("COMMIT").unwrap();
        assert_eq!(list_fields(&c, raw).unwrap().len(), 3);
        for k in FIXED_KEYS {
            let missing: bool = c.query_row(MISSING_SQL, [k], |r| r.get(0)).unwrap();
            assert!(!missing, "danach fehlt {k} nicht mehr");
        }
    }

    #[test]
    fn builtin_targets_follow_the_update_rules() {
        use FieldKind::*;
        let t = |v: &[(Option<&str>, Option<&str>, FieldKind)]| builtin_targets(v);
        // mit Schlüssel und passender Art
        assert_eq!(t(&[(Some("description"), None, Text), (Some("due"), None, Datetime), (Some("labels"), None, Labels)]), vec![Some(DESCRIPTION_KEY), Some(DUE_KEY), Some(LABELS_KEY)]);
        // ohne Schlüssel über den alten Namen (erstes passendes), andere Art nicht
        assert_eq!(t(&[(None, Some("Beschreibung"), Rich), (None, Some("Labels"), Labels), (None, Some("labels"), Labels)]), vec![Some(DESCRIPTION_KEY), Some(LABELS_KEY), None]);
        assert_eq!(t(&[(None, Some("Labels"), Text), (None, Some("Due date"), Date), (Some("due"), None, Date)]), vec![None, None, None]);
        // Schlüssel geht vor Namen
        assert_eq!(t(&[(None, Some("Description"), Text), (Some("description"), Some("Details"), Rich)]), vec![None, Some(DESCRIPTION_KEY)]);
        assert!(is_builtin_label("Due date") && is_builtin_label(" fällig am ") && is_builtin_label("LABELS") && is_builtin_label("Beschreibung"));
        assert!(!is_builtin_label("Due") && !is_builtin_label("Notes"));
    }
}
