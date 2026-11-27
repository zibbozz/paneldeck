//! Projekte: anlegen (mit oder ohne Standard-Inhalte), ändern, Reihenfolge, löschen, nachschlagen.
//!
//! Das System-Projekt `TODO` („Todolist“) wird von der Migration angelegt und ist weder änderbar noch löschbar.
//! Der Ticket-Zähler `next_number` wird hier nur angelegt (1); vergeben wird er beim Anlegen der Tickets (`tickets.rs`).

use super::common::{self, atomic, bool_i, clean_name, normalize_color, normalize_key, validate_key};
use super::{defaults, integrity, PlanError, Res};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagColorMode {
    Default,
    Accent,
    Random,
}

impl TagColorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TagColorMode::Default => "default",
            TagColorMode::Accent => "accent",
            TagColorMode::Random => "random",
        }
    }

    pub fn parse(s: &str) -> TagColorMode {
        match s {
            "accent" => TagColorMode::Accent,
            "random" => TagColorMode::Random,
            _ => TagColorMode::Default,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub description: String,
    /// Projektfarbe „#RRGGBB“ (= Akzentfarbe im Projekt).
    pub color: String,
    pub work_dir: String,
    pub use_sprints: bool,
    pub use_versions: bool,
    pub tag_color_mode: TagColorMode,
    pub system: bool,
    /// Nummer des nächsten Tickets (`<KEY>-<n>`), wird nie zurückgesetzt.
    pub next_number: i64,
    pub position: i64,
    pub created_at: String,
}

/// Eingaben für ein neues Projekt. `with_defaults = false` ergibt ein komplett leeres Projekt (ohne Status, Typen, Felder).
#[derive(Debug, Clone)]
pub struct NewProject {
    pub key: String,
    pub name: String,
    pub color: String,
    pub description: String,
    pub work_dir: String,
    pub use_sprints: bool,
    pub use_versions: bool,
    pub tag_color_mode: TagColorMode,
    pub with_defaults: bool,
}

impl NewProject {
    /// Minimal: Schlüssel, Name, Farbe; alles andere aus, ohne Standard-Inhalte.
    pub fn new(key: &str, name: &str, color: &str) -> Self {
        NewProject {
            key: key.to_string(),
            name: name.to_string(),
            color: color.to_string(),
            description: String::new(),
            work_dir: String::new(),
            use_sprints: false,
            use_versions: false,
            tag_color_mode: TagColorMode::Default,
            with_defaults: false,
        }
    }
}

/// Änderungen an einem Projekt; `None` = unverändert.
#[derive(Debug, Clone, Default)]
pub struct UpdateProject {
    /// Neuer Schlüssel (die Anzeige `<KEY>-<n>` aller Tickets folgt; gespeicherte Abfragen mit dem alten Schlüssel greifen nicht mehr).
    pub key: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    pub work_dir: Option<String>,
    pub use_sprints: Option<bool>,
    pub use_versions: Option<bool>,
    pub tag_color_mode: Option<TagColorMode>,
}

const COLS: &str = "id, key, name, description, color, work_dir, use_sprints, use_versions, tag_color_mode, system, next_number, position, created_at";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        key: r.get(1)?,
        name: r.get(2)?,
        description: r.get(3)?,
        color: r.get(4)?,
        work_dir: r.get(5)?,
        use_sprints: r.get::<_, i64>(6)? != 0,
        use_versions: r.get::<_, i64>(7)? != 0,
        tag_color_mode: TagColorMode::parse(&r.get::<_, String>(8)?),
        system: r.get::<_, i64>(9)? != 0,
        next_number: r.get(10)?,
        position: r.get(11)?,
        created_at: r.get(12)?,
    })
}

pub fn get_project(conn: &Connection, id: i64) -> Res<Project> {
    conn.query_row(&format!("SELECT {COLS} FROM projects WHERE id = ?1"), [id], from_row).optional()?.ok_or(PlanError::NotFound)
}

/// Projekt über den Schlüssel (Groß-/Kleinschreibung egal).
pub fn project_by_key(conn: &Connection, key: &str) -> Res<Project> {
    conn.query_row(&format!("SELECT {COLS} FROM projects WHERE key = ?1"), [normalize_key(key)], from_row).optional()?.ok_or(PlanError::NotFound)
}

/// Alle Projekte: das System-Projekt zuerst, dann in der eingestellten Reihenfolge.
pub fn list_projects(conn: &Connection) -> Res<Vec<Project>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM projects ORDER BY system DESC, position, id"))?;
    Ok(st.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Das Projekt, wenn es existiert und änderbar ist (kein System-Projekt) – Grundlage aller Änderungen an Status/Typen/Feldern.
pub fn editable(conn: &Connection, id: i64) -> Res<Project> {
    let p = get_project(conn, id)?;
    if p.system { Err(PlanError::SystemProtected) } else { Ok(p) }
}

fn key_taken(conn: &Connection, key: &str, except: Option<i64>) -> Res<bool> {
    let id: Option<i64> = conn.query_row("SELECT id FROM projects WHERE key = ?1", [key], |r| r.get(0)).optional()?;
    Ok(matches!(id, Some(i) if Some(i) != except))
}

/// Legt ein Projekt an (Schlüssel wird in Großbuchstaben gebracht und geprüft). Mit `with_defaults` entstehen Status, die
/// fünf Standard-Typen samt Workflows, Hierarchie, Feldern und der Liste der Arten der Arbeit.
pub fn create_project(conn: &Connection, p: &NewProject) -> Res<i64> {
    let key = normalize_key(&p.key);
    validate_key(&key)?;
    let name = clean_name(&p.name)?;
    let color = normalize_color(&p.color)?;
    atomic(conn, || {
        if key_taken(conn, &key, None)? {
            return Err(PlanError::KeyTaken);
        }
        let pos = common::next_position(conn, "projects", "system = 0", &[])?;
        conn.execute(
            "INSERT INTO projects (key, name, description, color, work_dir, use_sprints, use_versions, tag_color_mode, system, next_number, position, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 1, ?9, ?10)",
            params![key, name, p.description.trim(), color, p.work_dir.trim(), bool_i(p.use_sprints), bool_i(p.use_versions), p.tag_color_mode.as_str(), pos, common::now()],
        )?;
        let id = conn.last_insert_rowid();
        // Neue Ticket-Projekte beginnen für Agenten gesperrt (#386).
        super::access::lock_new_project(conn, id)?;
        if p.with_defaults {
            defaults::apply(conn, id, p.use_versions)?;
            // Projekte mit den mitgelieferten Typen bekommen die Standard-Schwellen der Fälligkeit
            conn.execute("UPDATE projects SET due_thresholds = ?1 WHERE id = ?2", params![crate::due::DEFAULT_THRESHOLDS_JSON, id])?;
        }
        Ok(id)
    })
}

/// Schwellen der Fälligkeits-Markierung eines Projekts (lange zuerst; leer = keine Markierung).
pub fn due_thresholds(conn: &Connection, id: i64) -> Res<Vec<crate::due::DueThreshold>> {
    let json: String = conn.query_row("SELECT due_thresholds FROM projects WHERE id = ?1", [id], |r| r.get(0)).optional()?.ok_or(PlanError::NotFound)?;
    Ok(crate::due::parse_thresholds(&json))
}

/// Schwellen setzen (geprüft: höchstens [`crate::due::MAX_THRESHOLDS`] = 20, Dauer > 0, keine doppelte Dauer). Nicht beim System-Projekt.
pub fn set_due_thresholds(conn: &Connection, id: i64, list: &[crate::due::DueThreshold]) -> Res<()> {
    editable(conn, id)?;
    let list = crate::due::validate_thresholds(list).map_err(PlanError::Thresholds)?;
    conn.execute("UPDATE projects SET due_thresholds = ?1 WHERE id = ?2", params![crate::due::thresholds_json(&list), id])?;
    Ok(())
}

pub fn update_project(conn: &Connection, id: i64, u: &UpdateProject) -> Res<()> {
    let cur = editable(conn, id)?;
    let key = match &u.key {
        Some(k) => {
            let k = normalize_key(k);
            validate_key(&k)?;
            k
        }
        None => cur.key.clone(),
    };
    let name = match &u.name {
        Some(n) => clean_name(n)?,
        None => cur.name.clone(),
    };
    let color = match &u.color {
        Some(c) => normalize_color(c)?,
        None => cur.color.clone(),
    };
    atomic(conn, || {
        if key != cur.key && key_taken(conn, &key, Some(id))? {
            return Err(PlanError::KeyTaken);
        }
        conn.execute(
            "UPDATE projects SET key = ?1, name = ?2, description = ?3, color = ?4, work_dir = ?5, use_sprints = ?6, use_versions = ?7, tag_color_mode = ?8 WHERE id = ?9",
            params![
                key,
                name,
                u.description.as_deref().map(str::trim).unwrap_or(&cur.description),
                color,
                u.work_dir.as_deref().map(str::trim).unwrap_or(&cur.work_dir),
                bool_i(u.use_sprints.unwrap_or(cur.use_sprints)),
                bool_i(u.use_versions.unwrap_or(cur.use_versions)),
                u.tag_color_mode.unwrap_or(cur.tag_color_mode).as_str(),
                id
            ],
        )?;
        Ok(())
    })
}

/// Neue Reihenfolge der änderbaren Projekte (das System-Projekt steht immer vorn und gehört nicht in die Liste).
pub fn reorder_projects(conn: &Connection, ids: &[i64]) -> Res<()> {
    common::reorder(conn, "projects", "system = 0", &[], ids)
}

/// Wie viele Tickets beim Löschen verloren gehen (für die Warnung).
pub fn delete_impact(conn: &Connection, id: i64) -> Res<i64> {
    editable(conn, id)?;
    integrity::usage_count_project(conn, id)
}

/// Löscht das Projekt samt allen Status, Typen, Feldern, Workflows und Arten der Arbeit (Kaskade; die Tickets des Projekts
/// hängen mit `ON DELETE CASCADE` ebenfalls daran). Liefert die Zahl der mitgelöschten Tickets – die
/// Oberfläche warnt vorher mit [`delete_impact`]. System-Projekte lassen sich nicht löschen.
pub fn delete_project(conn: &Connection, id: i64) -> Res<i64> {
    editable(conn, id)?;
    atomic(conn, || {
        let tickets = integrity::usage_count_project(conn, id)?;
        // Ticket-Felder anderer Projekte, die auf Tickets dieses Projekts verweisen, werden geleert.
        super::tickets::before_project_delete(conn, id).map_err(|e| match e {
            super::ticket_error::TicketError::Plan(p) => p,
            other => PlanError::Db(other.message(crate::state::lang())),
        })?;
        conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(tickets)
    })
}

impl Project {
    /// Anzeigename des Projekts (immer der eingegebene Name; das System-Projekt heißt in beiden Sprachen „Todolist“).
    pub fn display(&self, _lang: Lang) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::testutil::db;

    fn count(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn migration_creates_system_project_and_link_types() {
        // über den echten Öffnungsweg (Datei, Versionszähler), nicht nur über die Migrationsliste
        let d = std::env::temp_dir().join(format!("paneldeck-test-plan-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let opened = crate::db::open(&d, &d.join("backups")).unwrap();
        let v: u32 = opened.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, crate::db::latest_version());
        assert!(v >= 3);
        let c = &opened.conn;
        let ps = list_projects(&c).unwrap();
        assert_eq!(ps.len(), 1, "genau ein Projekt");
        let t = &ps[0];
        assert_eq!((t.key.as_str(), t.name.as_str(), t.color.as_str()), ("TODO", "Todolist", "#97A3B0"));
        assert!(t.system && !t.use_sprints && !t.use_versions);
        assert_eq!(t.next_number, 1);
        // genau ein System-Typ „todolist“: grau, ohne Workflow, ohne Felder
        let (key, color, wf, sys, name, icon): (String, String, i64, i64, Option<String>, Option<String>) = c
            .query_row("SELECT key, color, workflow, system, name, icon FROM ticket_types WHERE project_id = ?1", [t.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            .unwrap();
        assert_eq!((key.as_str(), color.as_str(), wf, sys), ("todolist", "#97A3B0", 0, 1));
        assert!(name.is_none() && icon.is_none());
        assert_eq!(count(&c, "ticket_types"), 1);
        assert_eq!(count(&c, "fields"), 0);
        assert_eq!(count(&c, "statuses"), 0);
        // vier Verknüpfungstypen; nur terminal ist System
        let rows: Vec<(String, i64, i64, String)> = {
            let mut st = c.prepare("SELECT key, symmetric, system, target_kind FROM link_types ORDER BY position").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(|x| x.unwrap()).collect()
        };
        assert_eq!(
            rows,
            vec![
                ("blocks".to_string(), 0, 0, "ticket".to_string()),
                ("relates".to_string(), 1, 0, "ticket".to_string()),
                ("duplicates".to_string(), 0, 0, "ticket".to_string()),
                ("terminal".to_string(), 0, 1, "terminal".to_string()),
            ]
        );
        drop(opened);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn migration_applies_on_top_of_version_2() {
        // Eine bestehende Datenbank der Version 2 (mit Todo-Daten) wird sauber auf 3 gehoben und behält ihre Daten.
        let d = std::env::temp_dir().join(format!("paneldeck-test-plan-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        {
            let c = Connection::open(d.join(crate::db::FILE)).unwrap();
            c.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID; PRAGMA user_version = 2;").unwrap();
            c.execute_batch("CREATE TABLE todo_items (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL DEFAULT '', body TEXT NOT NULL DEFAULT '', done INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, done_at TEXT); INSERT INTO todo_items (title, position, created_at, updated_at) VALUES ('alt', 0, 'x', 'x');").unwrap();
            c.execute_batch("CREATE TABLE todo_tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, name_key TEXT NOT NULL UNIQUE, color TEXT NOT NULL);").unwrap();
        }
        let db = crate::db::open(&d, &d.join("backups")).unwrap();
        let v: u32 = db.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, crate::db::latest_version());
        assert_eq!(count(&db.conn, "todo_items"), 1);
        assert_eq!(list_projects(&db.conn).unwrap().len(), 1);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn creates_an_empty_project() {
        let c = db();
        let id = create_project(&c, &NewProject::new("pan", "Paneldeck", "#58a6e6")).unwrap();
        let p = get_project(&c, id).unwrap();
        assert_eq!((p.key.as_str(), p.name.as_str(), p.color.as_str()), ("PAN", "Paneldeck", "#58A6E6"), "Schlüssel und Farbe normalisiert");
        assert!(!p.system && p.next_number == 1 && !p.use_sprints && !p.use_versions);
        assert_eq!(p.tag_color_mode, TagColorMode::Default);
        // komplett leer: keine Status/Typen/Felder/Arten der Arbeit (nur der System-Typ „todolist“ des anderen Projekts)
        for t in ["statuses", "type_parents", "type_statuses", "transitions", "fields", "work_kinds"] {
            assert_eq!(count(&c, t), 0, "{t}");
        }
        assert_eq!(count(&c, "ticket_types"), 1);
        assert_eq!(project_by_key(&c, "Pan").unwrap().id, id);
    }

    #[test]
    fn validates_key_name_color_and_uniqueness() {
        let c = db();
        let mk = |k: &str, n: &str, col: &str| create_project(&c, &NewProject::new(k, n, col));
        assert_eq!(mk("A", "x", "#112233"), Err(PlanError::KeyInvalid));
        assert_eq!(mk("1AB", "x", "#112233"), Err(PlanError::KeyInvalid));
        assert_eq!(mk("AB-C", "x", "#112233"), Err(PlanError::KeyInvalid));
        assert_eq!(mk("ABCDEFGHIJK", "x", "#112233"), Err(PlanError::KeyInvalid));
        assert_eq!(mk("ÄÖ", "x", "#112233"), Err(PlanError::KeyInvalid));
        assert_eq!(mk("AB", "  ", "#112233"), Err(PlanError::NameEmpty));
        assert_eq!(mk("AB", "x", "blau"), Err(PlanError::ColorInvalid));
        assert_eq!(mk("TODO", "x", "#112233"), Err(PlanError::KeyTaken), "auch der Schlüssel des System-Projekts ist vergeben");
        mk("AB", "x", "#112233").unwrap();
        assert_eq!(mk("ab", "y", "#112233"), Err(PlanError::KeyTaken), "ohne Beachtung der Groß-/Kleinschreibung");
        assert_eq!(list_projects(&c).unwrap().len(), 2, "fehlgeschlagene Anlage hinterlässt nichts");
        assert_eq!(project_by_key(&c, "zz"), Err(PlanError::NotFound));
    }

    #[test]
    fn updates_projects_and_protects_the_system_project() {
        let c = db();
        let a = create_project(&c, &NewProject::new("AA", "A", "#112233")).unwrap();
        let b = create_project(&c, &NewProject::new("BB", "B", "#112233")).unwrap();
        update_project(
            &c,
            a,
            &UpdateProject {
                name: Some(" Neu ".into()),
                color: Some("#abcdef".into()),
                description: Some(" Text ".into()),
                work_dir: Some("C:\\repo".into()),
                use_sprints: Some(true),
                use_versions: Some(true),
                tag_color_mode: Some(TagColorMode::Accent),
                key: Some("aax".into()),
            },
        )
        .unwrap();
        let p = get_project(&c, a).unwrap();
        assert_eq!((p.key.as_str(), p.name.as_str(), p.color.as_str(), p.description.as_str(), p.work_dir.as_str()), ("AAX", "Neu", "#ABCDEF", "Text", "C:\\repo"));
        assert!(p.use_sprints && p.use_versions);
        assert_eq!(p.tag_color_mode, TagColorMode::Accent);
        // unveränderte Felder bleiben
        update_project(&c, a, &UpdateProject { name: Some("N2".into()), ..Default::default() }).unwrap();
        let p = get_project(&c, a).unwrap();
        assert!(p.use_sprints && p.color == "#ABCDEF" && p.name == "N2");
        // Fehler
        assert_eq!(update_project(&c, a, &UpdateProject { key: Some("BB".into()), ..Default::default() }), Err(PlanError::KeyTaken));
        assert_eq!(update_project(&c, a, &UpdateProject { key: Some("x".into()), ..Default::default() }), Err(PlanError::KeyInvalid));
        assert_eq!(update_project(&c, a, &UpdateProject { name: Some("".into()), ..Default::default() }), Err(PlanError::NameEmpty));
        assert_eq!(update_project(&c, a, &UpdateProject { color: Some("rot".into()), ..Default::default() }), Err(PlanError::ColorInvalid));
        // der eigene Schlüssel ist kein Konflikt
        update_project(&c, b, &UpdateProject { key: Some("BB".into()), ..Default::default() }).unwrap();
        assert_eq!(update_project(&c, 999, &UpdateProject::default()), Err(PlanError::NotFound));
        // System-Projekt
        let todo = project_by_key(&c, "TODO").unwrap();
        assert_eq!(update_project(&c, todo.id, &UpdateProject { name: Some("x".into()), ..Default::default() }), Err(PlanError::SystemProtected));
        assert_eq!(delete_project(&c, todo.id), Err(PlanError::SystemProtected));
        assert_eq!(get_project(&c, todo.id).unwrap().name, "Todolist");
    }

    #[test]
    fn orders_projects_with_the_system_project_first() {
        let c = db();
        let a = create_project(&c, &NewProject::new("AA", "A", "#112233")).unwrap();
        let b = create_project(&c, &NewProject::new("BB", "B", "#112233")).unwrap();
        let d = create_project(&c, &NewProject::new("CC", "C", "#112233")).unwrap();
        let keys = |c: &Connection| list_projects(c).unwrap().into_iter().map(|p| p.key).collect::<Vec<_>>();
        assert_eq!(keys(&c), vec!["TODO", "AA", "BB", "CC"]);
        reorder_projects(&c, &[d, a, b]).unwrap();
        assert_eq!(keys(&c), vec!["TODO", "CC", "AA", "BB"]);
        assert_eq!(reorder_projects(&c, &[d, a]), Err(PlanError::ReorderMismatch));
        assert_eq!(reorder_projects(&c, &[d, a, b, b]), Err(PlanError::ReorderMismatch));
        let todo = project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(reorder_projects(&c, &[todo, d, a, b]), Err(PlanError::ReorderMismatch), "das System-Projekt gehört nicht in die Liste");
    }

    #[test]
    fn deletes_projects_with_cascade() {
        let c = db();
        let mut np = NewProject::new("AA", "A", "#112233");
        np.with_defaults = true;
        let a = create_project(&c, &np).unwrap();
        let keep = create_project(&c, &NewProject::new("BB", "B", "#112233")).unwrap();
        assert!(count(&c, "statuses") > 0 && count(&c, "fields") > 0 && count(&c, "work_kinds") > 0 && count(&c, "transitions") > 0 && count(&c, "type_parents") > 0);
        assert_eq!(delete_impact(&c, a).unwrap(), 0);
        assert_eq!(delete_project(&c, a).unwrap(), 0);
        for t in ["statuses", "type_parents", "type_statuses", "transitions", "fields", "work_kinds"] {
            assert_eq!(count(&c, t), 0, "{t} kaskadiert");
        }
        assert_eq!(count(&c, "ticket_types"), 1, "nur der System-Typ bleibt");
        assert_eq!(get_project(&c, a), Err(PlanError::NotFound));
        assert!(get_project(&c, keep).is_ok());
        assert_eq!(delete_project(&c, a), Err(PlanError::NotFound));
    }
}
