//! Daten der Todo-Liste: Einträge („Karten“), Tags und deren Zuordnung in der SQLite-Datenbank. Reine Funktionen auf
//! `&Connection` ohne Oberfläche (die Anzeige steht in `app/tasks_ui.rs`).
//!
//! * Eine Karte hat optionalen Titel und optionalen Text – mindestens eins von beiden muss gefüllt sein.
//! * Die Nummer (`id`, Anzeige `TODO-<n>`) wird fortlaufend vergeben und nie wiederverwendet.
//! * Tags werden beim Zuordnen automatisch angelegt (Name ohne Beachtung der Groß-/Kleinschreibung eindeutig, angezeigt
//!   wird der zuerst angelegte Name) und bekommen reihum eine Farbe aus der geltenden Palette (`palette::Area::Todo`).
//! * Alle Änderungen außer dem Löschen liefern eine [`TaskOp`] zurück, die sich mit `undo::History` rückgängig machen
//!   und wiederholen lässt. Löschen bleibt endgültig.

use crate::state::{self, Lang};
use crate::undo::Op;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

/// Längste Tag-Bezeichnung in Zeichen.
pub const MAX_TAG_CHARS: usize = 32;

#[derive(Debug, PartialEq, Clone)]
pub enum TaskError {
    /// Weder Titel noch Text.
    Empty,
    /// Eintrag oder Tag existiert nicht (mehr).
    NotFound,
    /// Leerer Tag-Name.
    TagNameEmpty,
    /// Umbenennen scheitert, weil der Name schon vergeben ist (Id des anderen Tags) – Zusammenführen anbieten.
    TagExists(i64),
    /// Ein Tag lässt sich nicht wiederherstellen, weil sein Name inzwischen anders belegt ist.
    TagNameTaken,
    /// Fälligkeit nicht lesbar (Form `JJJJ-MM-TT` oder `JJJJ-MM-TTTHH:MM`).
    DueInvalid,
    /// Schwellen der Fälligkeits-Markierung ungültig.
    Thresholds(crate::due::ThresholdProblem),
    Db(String),
}

impl From<rusqlite::Error> for TaskError {
    fn from(e: rusqlite::Error) -> Self {
        TaskError::Db(e.to_string())
    }
}

impl TaskError {
    pub fn message(&self, lang: Lang) -> String {
        match self {
            TaskError::Empty => lang.t("Eine Karte braucht einen Titel oder einen Text.", "A card needs a title or some text.").to_string(),
            TaskError::NotFound => lang.t("Der Eintrag existiert nicht mehr.", "The item no longer exists.").to_string(),
            TaskError::TagNameEmpty => lang.t("Der Tag-Name darf nicht leer sein.", "The tag name must not be empty.").to_string(),
            TaskError::TagExists(_) => lang.t("Ein Tag mit diesem Namen gibt es schon.", "A tag with this name already exists.").to_string(),
            TaskError::TagNameTaken => lang.t("Der Tag-Name ist inzwischen anders vergeben.", "The tag name is now used by another tag.").to_string(),
            TaskError::DueInvalid => lang.t("Fälligkeit nicht lesbar – so geht es: 2026-11-20 oder 2026-11-20 14:30.", "Due date not readable – use 2026-11-20 or 2026-11-20 14:30.").to_string(),
            TaskError::Thresholds(p) => p.message(lang).to_string(),
            TaskError::Db(e) => format!("{}: {e}", lang.t("Datenbankfehler", "Database error")),
        }
    }
}

type Res<T> = Result<T, TaskError>;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub done: bool,
    pub position: i64,
    pub created_at: String,
    pub updated_at: String,
    pub done_at: Option<String>,
    /// Tag-Ids, aufsteigend.
    pub tags: Vec<i64>,
    /// Fälligkeit (`JJJJ-MM-TT` oder `JJJJ-MM-TTTHH:MM`, siehe `due.rs`), optional.
    pub due: Option<String>,
    /// Seitenleisten-Projekt der Karte (#386); `None` = Bereich „Allgemein“.
    pub project: Option<u64>,
    /// Privat (#386): nie für Agenten sichtbar (auch über einen privaten Tag, siehe [`is_private`]).
    pub private: bool,
    /// Herkunft (#386): Nutzer, Agent (mit Sitzungsname) oder Paneldeck – Agent/Paneldeck = Systemnotiz.
    pub origin: crate::planning::access::Origin,
    pub origin_detail: Option<String>,
}

/// Ist die Karte privat – selbst oder über einen ihrer Tags (#386)?
pub fn is_private(it: &Item, tags: &[Tag]) -> bool {
    it.private || it.tags.iter().any(|t| tags.iter().any(|x| x.id == *t && x.private))
}

/// Was beim Anlegen einer Karte zusätzlich gilt (#386).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemMeta {
    pub project: Option<u64>,
    pub private: bool,
    pub origin: crate::planning::access::Origin,
    pub origin_detail: Option<String>,
}

impl Item {
    /// Anzeigeschlüssel, z. B. `TODO-12`.
    pub fn key(&self) -> String {
        key(self.id)
    }

    fn state(&self) -> ItemState {
        ItemState { title: self.title.clone(), body: self.body.clone(), done: self.done, done_at: self.done_at.clone(), tags: self.tags.clone(), due: self.due.clone(), project: self.project, private: self.private }
    }
}

pub fn key(id: i64) -> String {
    format!("TODO-{id}")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    /// „#RRGGBB“
    pub color: String,
    /// Privat (#386): alle Karten mit diesem Tag sind privat.
    pub private: bool,
}

/// Veränderbarer Teil einer Karte (für Rückgängig/Wiederholen).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemState {
    pub title: String,
    pub body: String,
    pub done: bool,
    pub done_at: Option<String>,
    /// Tag-Ids, aufsteigend.
    pub tags: Vec<i64>,
    pub due: Option<String>,
    /// Projekt und Privat-Schalter (#386).
    pub project: Option<u64>,
    pub private: bool,
}

/// Welche Karten die Liste zeigt.
#[derive(Clone, Debug, Default)]
pub struct ItemFilter {
    /// Erledigte mit anzeigen.
    pub show_done: bool,
    /// Nur Karten, die ALLE diese Tags haben (leer = kein Tag-Filter).
    pub tags: Vec<i64>,
}

#[cfg(test)]
thread_local! {
    static FIXED_NOW: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Nur für Tests: legt „jetzt“ für diesen Thread fest (`None` = wieder die echte Uhr). Für das Erzeugen der Demo-Datenbank der Screenshots.
#[cfg(test)]
pub fn set_fixed_now(t: Option<&str>) {
    FIXED_NOW.with(|f| *f.borrow_mut() = t.map(String::from));
}

/// Lokale Zeit als ISO-Text wie `meta.created_at`.
pub fn now() -> String {
    #[cfg(test)]
    if let Some(t) = FIXED_NOW.with(|f| f.borrow().clone()) {
        return t;
    }
    chrono::Local::now().naive_local().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Farbe „#RRGGBB“ als RGB; ungültiger Text ergibt Grau.
pub fn rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return ((v >> 16) as u8, (v >> 8) as u8, v as u8);
        }
    }
    (0x97, 0xa3, 0xb0)
}

// ------------------------------------------------------------------ Eingaben bereinigen

/// Titel und Text bereinigen (Leerraum am Rand entfernen) und prüfen, dass mindestens eins gefüllt ist.
pub fn validate(title: &str, body: &str) -> Res<(String, String)> {
    let (t, b) = (title.trim().to_string(), body.trim().to_string());
    if t.is_empty() && b.is_empty() {
        return Err(TaskError::Empty);
    }
    Ok((t, b))
}

/// Tag-Name bereinigen: Leerraum zusammenfassen, auf [`MAX_TAG_CHARS`] kürzen.
pub fn clean_tag(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TAG_CHARS).collect::<String>().trim().to_string()
}

/// Vergleichsschlüssel eines Tag-Namens (ohne Groß-/Kleinschreibung).
pub fn tag_key(name: &str) -> String {
    clean_tag(name).to_lowercase()
}

/// Fälligkeit bereinigen: leer → `None`, sonst Eingabe wie `2026-11-20`, `20.11.2026 14:30` in die gespeicherte Form bringen.
pub fn clean_due(due: Option<&str>) -> Res<Option<String>> {
    match due {
        None => Ok(None),
        Some(s) => crate::due::parse_input(s).map_err(|_| TaskError::DueInvalid),
    }
}

// ------------------------------------------------------------------ Fälligkeit: Schwellen, Filter, Sortierung

/// Meta-Schlüssel der Schwellen der Todo-Liste (JSON wie bei den Projekten).
const THRESHOLDS_META: &str = "todo_due_thresholds";

/// Schwellen der Fälligkeits-Markierung der Todo-Liste. Nie gesetzt = die Standard-Schwellen.
pub fn due_thresholds(conn: &Connection) -> Res<Vec<crate::due::DueThreshold>> {
    let json: Option<String> = conn.query_row("SELECT value FROM meta WHERE key = ?1", [THRESHOLDS_META], |r| r.get(0)).optional()?;
    Ok(match json {
        Some(j) => crate::due::parse_thresholds(&j),
        None => crate::due::default_thresholds(),
    })
}

/// Schwellen der Todo-Liste setzen (geprüft wie bei den Projekten; leer = keine Markierung).
pub fn set_due_thresholds(conn: &Connection, list: &[crate::due::DueThreshold]) -> Res<()> {
    let list = crate::due::validate_thresholds(list).map_err(TaskError::Thresholds)?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![THRESHOLDS_META, crate::due::thresholds_json(&list)],
    )?;
    Ok(())
}

/// Filter der Liste nach Fälligkeit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DueFilter {
    #[default]
    All,
    /// Nur Karten mit Fälligkeit.
    WithDue,
    /// Nur überfällige Karten.
    Overdue,
    /// Nur Karten mit Markierung oder Tönung (nach den Schwellen).
    Marked,
}

impl DueFilter {
    pub const ALL: [DueFilter; 4] = [DueFilter::All, DueFilter::WithDue, DueFilter::Overdue, DueFilter::Marked];

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            DueFilter::All => lang.t("Alle", "All"),
            DueFilter::WithDue => lang.t("Mit Fälligkeit", "With due date"),
            DueFilter::Overdue => lang.t("Überfällig", "Overdue"),
            DueFilter::Marked => lang.t("Markiert", "Marked"),
        }
    }

    /// Passt die Karte? Erledigte Karten sind nie überfällig oder markiert.
    pub fn matches(self, it: &Item, thresholds: &[crate::due::DueThreshold], now: chrono::NaiveDateTime) -> bool {
        let st = || if it.done { None } else { it.due.as_deref().and_then(|d| crate::due::evaluate(thresholds, d, now)) };
        match self {
            DueFilter::All => true,
            DueFilter::WithDue => it.due.is_some(),
            DueFilter::Overdue => st().is_some_and(|s| s.overdue()),
            DueFilter::Marked => st().is_some_and(|s| s.flagged()),
        }
    }
}

/// Sortierung der Liste.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemSort {
    /// Eigene Reihenfolge (Ziehen).
    #[default]
    Manual,
    /// Nach Fälligkeit, früheste zuerst; ohne Fälligkeit am Ende (in eigener Reihenfolge).
    Due,
}

/// Karten nach Fälligkeit sortieren (stabil; „nur Datum“ zählt als Ende des Tages).
pub fn sort_by_due(items: &mut [Item]) {
    items.sort_by_key(|i| (i.due.as_deref().and_then(crate::due::deadline).is_none(), i.due.as_deref().and_then(crate::due::deadline)));
}

/// Zählung für die Übersicht (Reiter-Kopf): (überfällig, markiert) der offenen Karten.
pub fn due_counts(conn: &Connection, now: chrono::NaiveDateTime) -> Res<(usize, usize)> {
    let th = due_thresholds(conn)?;
    let mut st = conn.prepare("SELECT due FROM todo_items WHERE done = 0 AND due IS NOT NULL")?;
    let dues = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(crate::due::count(&th, dues, now))
}

// ------------------------------------------------------------------ Lesen

const ITEM_COLS: &str = "id, title, body, done, position, created_at, updated_at, done_at, due, project, private, origin, origin_detail";

fn item_from_row(r: &rusqlite::Row) -> rusqlite::Result<Item> {
    Ok(Item {
        id: r.get(0)?,
        title: r.get(1)?,
        body: r.get(2)?,
        done: r.get::<_, i64>(3)? != 0,
        position: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        done_at: r.get(7)?,
        tags: Vec::new(),
        due: r.get(8)?,
        project: r.get::<_, Option<i64>>(9)?.and_then(|p| u64::try_from(p).ok()),
        private: r.get::<_, i64>(10)? != 0,
        origin: crate::planning::access::Origin::parse(&r.get::<_, String>(11)?),
        origin_detail: r.get(12)?,
    })
}

fn item_tag_ids(conn: &Connection, id: i64) -> Res<Vec<i64>> {
    let mut st = conn.prepare("SELECT tag_id FROM todo_item_tags WHERE item_id = ?1 ORDER BY tag_id")?;
    let v = st.query_map([id], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(v)
}

pub fn get_item(conn: &Connection, id: i64) -> Res<Item> {
    let mut it = conn
        .query_row(&format!("SELECT {ITEM_COLS} FROM todo_items WHERE id = ?1"), [id], item_from_row)
        .optional()?
        .ok_or(TaskError::NotFound)?;
    it.tags = item_tag_ids(conn, id)?;
    Ok(it)
}

/// Karten in Listenreihenfolge (oben zuerst), gefiltert nach `filter`.
pub fn list_items(conn: &Connection, filter: &ItemFilter) -> Res<Vec<Item>> {
    let sql = format!("SELECT {ITEM_COLS} FROM todo_items {} ORDER BY position, id", if filter.show_done { "" } else { "WHERE done = 0" });
    let mut st = conn.prepare(&sql)?;
    let mut items = st.query_map([], item_from_row)?.collect::<Result<Vec<Item>, _>>()?;
    let mut links: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut st = conn.prepare("SELECT item_id, tag_id FROM todo_item_tags ORDER BY tag_id")?;
    for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (i, t) = row?;
        links.entry(i).or_default().push(t);
    }
    for it in &mut items {
        it.tags = links.remove(&it.id).unwrap_or_default();
    }
    if !filter.tags.is_empty() {
        items.retain(|i| filter.tags.iter().all(|t| i.tags.contains(t)));
    }
    Ok(items)
}

/// (alle Karten, davon erledigt)
pub fn counts(conn: &Connection) -> Res<(usize, usize)> {
    let (n, d): (i64, Option<i64>) = conn.query_row("SELECT COUNT(*), SUM(done) FROM todo_items", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok((n as usize, d.unwrap_or(0) as usize))
}

/// Alle Tags, alphabetisch (ohne Beachtung der Groß-/Kleinschreibung).
pub fn list_tags(conn: &Connection) -> Res<Vec<Tag>> {
    let mut st = conn.prepare("SELECT id, name, color, private FROM todo_tags")?;
    let mut v = st.query_map([], |r| Ok(Tag { id: r.get(0)?, name: r.get(1)?, color: r.get(2)?, private: r.get::<_, i64>(3)? != 0 }))?.collect::<Result<Vec<Tag>, _>>()?;
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    Ok(v)
}

fn get_tag(conn: &Connection, id: i64) -> Res<Tag> {
    conn.query_row("SELECT id, name, color, private FROM todo_tags WHERE id = ?1", [id], |r| Ok(Tag { id: r.get(0)?, name: r.get(1)?, color: r.get(2)?, private: r.get::<_, i64>(3)? != 0 }))
        .optional()?
        .ok_or(TaskError::NotFound)
}

pub fn find_tag(conn: &Connection, name: &str) -> Res<Option<Tag>> {
    let k = tag_key(name);
    Ok(conn.query_row("SELECT id, name, color, private FROM todo_tags WHERE name_key = ?1", [k], |r| Ok(Tag { id: r.get(0)?, name: r.get(1)?, color: r.get(2)?, private: r.get::<_, i64>(3)? != 0 })).optional()?)
}

/// Auf wie vielen Karten ein Tag steht, je Tag-Id (Tags ohne Karte fehlen).
pub fn tag_counts(conn: &Connection) -> Res<HashMap<i64, usize>> {
    let mut st = conn.prepare("SELECT tag_id, COUNT(*) FROM todo_item_tags GROUP BY tag_id")?;
    let mut m = HashMap::new();
    for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (t, n) = row?;
        m.insert(t, n as usize);
    }
    Ok(m)
}

// ------------------------------------------------------------------ Tags schreiben

/// Palettenfarbe, die bisher am seltensten vergeben ist (bei Gleichstand die erste) – so rotiert die Vergabe. Palette:
/// die geltende der Todo-Liste (`palette::auto`, bei leerer Palette die eingebauten Vorgaben).
pub fn next_color(conn: &Connection) -> Res<String> {
    next_color_from(conn, &crate::palette::auto(crate::palette::Area::Todo))
}

/// Wie [`next_color`] mit vorgegebener Palette (`pool` darf nicht leer sein).
pub fn next_color_from(conn: &Connection, pool: &[String]) -> Res<String> {
    let mut st = conn.prepare("SELECT color FROM todo_tags")?;
    let used: Vec<String> = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?;
    Ok(crate::palette::least_used(pool, used.iter().map(|s| s.as_str())).unwrap_or_else(|| crate::palette::TODO_DEFAULTS[0].to_string()))
}

/// Tag zum Namen finden oder (mit der nächsten Palettenfarbe) anlegen.
fn find_or_create_tag(conn: &Connection, name: &str) -> Res<Tag> {
    let name = clean_tag(name);
    if name.is_empty() {
        return Err(TaskError::TagNameEmpty);
    }
    if let Some(t) = find_tag(conn, &name)? {
        return Ok(t);
    }
    let color = next_color(conn)?;
    conn.execute("INSERT INTO todo_tags (name, name_key, color) VALUES (?1, ?2, ?3)", params![name, tag_key(&name), color])?;
    Ok(Tag { id: conn.last_insert_rowid(), name, color, private: false })
}

/// Namen zu Tag-Ids auflösen und fehlende Tags anlegen; leere Namen werden übersprungen, Doppelte (auch in anderer
/// Schreibweise) zusammengefasst. Ergebnis aufsteigend sortiert.
pub fn resolve_tags(conn: &Connection, names: &[String]) -> Res<Vec<i64>> {
    let mut ids = Vec::new();
    for n in names {
        if clean_tag(n).is_empty() {
            continue;
        }
        let t = find_or_create_tag(conn, n)?;
        if !ids.contains(&t.id) {
            ids.push(t.id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

fn write_tag(conn: &Connection, id: i64, name: &str, color: &str) -> Res<()> {
    let n = conn
        .execute("UPDATE todo_tags SET name = ?2, name_key = ?3, color = ?4 WHERE id = ?1", params![id, name, tag_key(name), color])
        .map_err(|e| match &e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => TaskError::TagNameTaken,
            _ => TaskError::from(e),
        })?;
    if n == 0 { Err(TaskError::NotFound) } else { Ok(()) }
}

/// Tag umbenennen (wirkt überall). Ist der Name bei einem anderen Tag schon vergeben, kommt `TagExists(andere Id)` –
/// dann kann die Oberfläche das Zusammenführen anbieten. Nur die Schreibweise ändern (anderer Buchstabe groß) ist erlaubt.
pub fn rename_tag(conn: &Connection, id: i64, new_name: &str) -> Res<Option<TaskOp>> {
    let new_name = clean_tag(new_name);
    if new_name.is_empty() {
        return Err(TaskError::TagNameEmpty);
    }
    let tx = conn.unchecked_transaction()?;
    let before = get_tag(&tx, id)?;
    if let Some(other) = find_tag(&tx, &new_name)? {
        if other.id != id {
            return Err(TaskError::TagExists(other.id));
        }
    }
    if before.name == new_name {
        return Ok(None);
    }
    write_tag(&tx, id, &new_name, &before.color)?;
    tx.commit()?;
    Ok(Some(TaskOp::TagEdit { id, before: (before.name, before.color.clone()), after: (new_name, before.color) }))
}

pub fn set_tag_color(conn: &Connection, id: i64, color: &str) -> Res<Option<TaskOp>> {
    let tx = conn.unchecked_transaction()?;
    let before = get_tag(&tx, id)?;
    if before.color.eq_ignore_ascii_case(color) {
        return Ok(None);
    }
    write_tag(&tx, id, &before.name, color)?;
    tx.commit()?;
    Ok(Some(TaskOp::TagEdit { id, before: (before.name.clone(), before.color), after: (before.name, color.to_string()) }))
}

/// Tag `from` in `into` aufgehen lassen: alle Karten mit `from` bekommen `into`, `from` verschwindet.
pub fn merge_tags(conn: &Connection, from: i64, into: i64) -> Res<TaskOp> {
    if from == into {
        return Err(TaskError::NotFound);
    }
    let tx = conn.unchecked_transaction()?;
    let from_tag = get_tag(&tx, from)?;
    get_tag(&tx, into)?;
    let mut moved = Vec::new();
    {
        let mut st = tx.prepare(
            "SELECT item_id, EXISTS (SELECT 1 FROM todo_item_tags x WHERE x.item_id = t.item_id AND x.tag_id = ?2) FROM todo_item_tags t WHERE t.tag_id = ?1 ORDER BY item_id",
        )?;
        for row in st.query_map(params![from, into], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0)))? {
            moved.push(row?);
        }
    }
    do_merge(&tx, from, into, &moved)?;
    tx.commit()?;
    Ok(TaskOp::Merge { from: from_tag, into, moved, merged: true })
}

fn do_merge(conn: &Connection, from: i64, into: i64, moved: &[(i64, bool)]) -> Res<()> {
    get_tag(conn, from)?;
    get_tag(conn, into)?;
    for (item, _) in moved {
        conn.execute("INSERT OR IGNORE INTO todo_item_tags (item_id, tag_id) SELECT id, ?2 FROM todo_items WHERE id = ?1", params![item, into])?;
    }
    conn.execute("DELETE FROM todo_tags WHERE id = ?1", [from])?;
    Ok(())
}

fn undo_merge(conn: &Connection, from: &Tag, into: i64, moved: &[(i64, bool)]) -> Res<()> {
    get_tag(conn, into)?;
    // Hat jemand den Namen inzwischen neu angelegt: nur ein unbenutzter Tag darf dafür weichen.
    if let Some(other) = find_tag(conn, &from.name)? {
        let used: i64 = conn.query_row("SELECT COUNT(*) FROM todo_item_tags WHERE tag_id = ?1", [other.id], |r| r.get(0))?;
        if used > 0 {
            return Err(TaskError::TagNameTaken);
        }
        conn.execute("DELETE FROM todo_tags WHERE id = ?1", [other.id])?;
    }
    conn.execute("INSERT INTO todo_tags (id, name, name_key, color, private) VALUES (?1, ?2, ?3, ?4, ?5)", params![from.id, from.name, tag_key(&from.name), from.color, from.private as i64])?;
    for (item, had_into) in moved {
        conn.execute("INSERT OR IGNORE INTO todo_item_tags (item_id, tag_id) SELECT id, ?2 FROM todo_items WHERE id = ?1", params![item, from.id])?;
        if !had_into {
            conn.execute("DELETE FROM todo_item_tags WHERE item_id = ?1 AND tag_id = ?2", params![item, into])?;
        }
    }
    Ok(())
}

/// Tag endgültig löschen (verschwindet von allen Karten). Gibt zurück, von wie vielen Karten er entfernt wurde.
pub fn delete_tag(conn: &Connection, id: i64) -> Res<usize> {
    let tx = conn.unchecked_transaction()?;
    get_tag(&tx, id)?;
    let n: i64 = tx.query_row("SELECT COUNT(*) FROM todo_item_tags WHERE tag_id = ?1", [id], |r| r.get(0))?;
    tx.execute("DELETE FROM todo_tags WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(n as usize)
}

// ------------------------------------------------------------------ Karten schreiben

fn set_item_tags(conn: &Connection, id: i64, tags: &[i64]) -> Res<()> {
    conn.execute("DELETE FROM todo_item_tags WHERE item_id = ?1", [id])?;
    for t in tags {
        // fehlende Tags (inzwischen gelöscht) werden still übergangen
        conn.execute("INSERT OR IGNORE INTO todo_item_tags (item_id, tag_id) SELECT ?1, id FROM todo_tags WHERE id = ?2", params![id, t])?;
    }
    Ok(())
}

fn apply_state(conn: &Connection, id: i64, st: &ItemState) -> Res<()> {
    let n = conn.execute(
        "UPDATE todo_items SET title = ?2, body = ?3, done = ?4, done_at = ?5, updated_at = ?6, due = ?7, project = ?8, private = ?9 WHERE id = ?1",
        params![id, st.title, st.body, st.done as i64, st.done_at, now(), st.due, st.project.map(|p| p as i64), st.private as i64],
    )?;
    if n == 0 {
        return Err(TaskError::NotFound);
    }
    set_item_tags(conn, id, &st.tags)
}

fn insert_item(conn: &Connection, it: &Item) -> Res<()> {
    conn.execute(
        "INSERT INTO todo_items (id, title, body, done, position, created_at, updated_at, done_at, due, project, private, origin, origin_detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![it.id, it.title, it.body, it.done as i64, it.position, it.created_at, it.updated_at, it.done_at, it.due, it.project.map(|p| p as i64), it.private as i64, it.origin.as_str(), it.origin_detail],
    )?;
    set_item_tags(conn, it.id, &it.tags)
}

/// Neue Karte oben in der Liste anlegen. Titel/Text: mindestens eins gefüllt, sonst `Empty` (dann entsteht auch kein Tag).
/// Seit #386 nur noch in Tests; das Programm nimmt [`create_item_ext`].
#[cfg(test)]
pub fn create_item(conn: &Connection, title: &str, body: &str, due: Option<&str>, tag_names: &[String]) -> Res<(Item, TaskOp)> {
    create_item_ext(conn, title, body, due, tag_names, &ItemMeta::default())
}

/// [`create_item`] mit Projekt, Privat-Schalter und Herkunft (#386).
pub fn create_item_ext(conn: &Connection, title: &str, body: &str, due: Option<&str>, tag_names: &[String], meta: &ItemMeta) -> Res<(Item, TaskOp)> {
    let (title, body) = validate(title, body)?;
    let due = clean_due(due)?;
    let tx = conn.unchecked_transaction()?;
    let tags = resolve_tags(&tx, tag_names)?;
    let position: i64 = tx.query_row("SELECT COALESCE(MIN(position), 0) - 1 FROM todo_items", [], |r| r.get(0))?;
    let t = now();
    tx.execute(
        "INSERT INTO todo_items (title, body, done, position, created_at, updated_at, due, project, private, origin, origin_detail) VALUES (?1, ?2, 0, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![title, body, position, t, due, meta.project.map(|p| p as i64), meta.private as i64, meta.origin.as_str(), meta.origin_detail],
    )?;
    let id = tx.last_insert_rowid();
    set_item_tags(&tx, id, &tags)?;
    let item = get_item(&tx, id)?;
    tx.commit()?;
    Ok((item.clone(), TaskOp::Presence { item, present: true }))
}

/// Titel, Text, Fälligkeit und Tags einer Karte ändern (gleiche Regel wie beim Anlegen). `None` = nichts geändert.
pub fn edit_item(conn: &Connection, id: i64, title: &str, body: &str, due: Option<&str>, tag_names: &[String]) -> Res<Option<TaskOp>> {
    let (title, body) = validate(title, body)?;
    let due = clean_due(due)?;
    let tx = conn.unchecked_transaction()?;
    let before = get_item(&tx, id)?.state();
    let tags = resolve_tags(&tx, tag_names)?;
    let after = ItemState { title, body, done: before.done, done_at: before.done_at.clone(), tags, due, project: before.project, private: before.private };
    if before == after {
        return Ok(None);
    }
    apply_state(&tx, id, &after)?;
    tx.commit()?;
    Ok(Some(TaskOp::Edit { id, before, after }))
}

/// Projekt und Privat-Schalter einer Karte (nur der Nutzer, #386). `None` = nichts geändert.
pub fn set_item_meta(conn: &Connection, id: i64, project: Option<u64>, private: bool) -> Res<Option<TaskOp>> {
    let tx = conn.unchecked_transaction()?;
    let before = get_item(&tx, id)?.state();
    let after = ItemState { project, private, ..before.clone() };
    if before == after {
        return Ok(None);
    }
    apply_state(&tx, id, &after)?;
    tx.commit()?;
    Ok(Some(TaskOp::Edit { id, before, after }))
}

/// Privat-Schalter eines Tags (nur der Nutzer, #386).
pub fn set_tag_private(conn: &Connection, id: i64, private: bool) -> Res<()> {
    if conn.execute("UPDATE todo_tags SET private = ?1 WHERE id = ?2", params![private as i64, id])? == 0 {
        return Err(TaskError::NotFound);
    }
    Ok(())
}

/// Karte abhaken oder wieder öffnen. `None` = war schon so.
pub fn set_done(conn: &Connection, id: i64, done: bool) -> Res<Option<TaskOp>> {
    let tx = conn.unchecked_transaction()?;
    let before = get_item(&tx, id)?.state();
    if before.done == done {
        return Ok(None);
    }
    let after = ItemState { done, done_at: if done { Some(now()) } else { None }, ..before.clone() };
    apply_state(&tx, id, &after)?;
    tx.commit()?;
    Ok(Some(TaskOp::Edit { id, before, after }))
}

fn order(conn: &Connection) -> Res<Vec<i64>> {
    let mut st = conn.prepare("SELECT id FROM todo_items ORDER BY position, id")?;
    let v = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(v)
}

/// Karte vor die Karte `before` setzen (`None` = ans Ende) und die Positionen neu durchnummerieren. Gibt
/// `(Nachbar vorher, Nachbar nachher)` zurück (jeweils die Karte direkt darunter), `None` wenn sich nichts ändert.
fn apply_move(conn: &Connection, id: i64, before: Option<i64>) -> Res<Option<(Option<i64>, Option<i64>)>> {
    let old = order(conn)?;
    let from_idx = old.iter().position(|x| *x == id).ok_or(TaskError::NotFound)?;
    if before == Some(id) {
        return Ok(None);
    }
    let mut ord = old.clone();
    ord.remove(from_idx);
    let at = before.and_then(|b| ord.iter().position(|x| *x == b)).unwrap_or(ord.len());
    ord.insert(at, id);
    if ord == old {
        return Ok(None);
    }
    for (i, x) in ord.iter().enumerate() {
        conn.execute("UPDATE todo_items SET position = ?1 WHERE id = ?2", params![i as i64, x])?;
    }
    Ok(Some((old.get(from_idx + 1).copied(), ord.get(at + 1).copied())))
}

/// Karte per Drag & Drop vor eine andere setzen (`before` = die Karte, vor der sie landen soll; `None` = ans Ende).
pub fn move_before(conn: &Connection, id: i64, before: Option<i64>) -> Res<Option<TaskOp>> {
    let tx = conn.unchecked_transaction()?;
    let r = apply_move(&tx, id, before)?;
    tx.commit()?;
    Ok(r.map(|(from, to)| TaskOp::Move { id, from, to }))
}

/// Eine Karte endgültig löschen.
pub fn delete_item(conn: &Connection, id: i64) -> Res<()> {
    if conn.execute("DELETE FROM todo_items WHERE id = ?1", [id])? == 0 {
        return Err(TaskError::NotFound);
    }
    Ok(())
}

/// Alle erledigten Karten endgültig löschen; gibt deren Ids zurück.
pub fn delete_done(conn: &Connection) -> Res<Vec<i64>> {
    let tx = conn.unchecked_transaction()?;
    let ids = {
        let mut st = tx.prepare("SELECT id FROM todo_items WHERE done = 1 ORDER BY id")?;
        st.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?
    };
    tx.execute("DELETE FROM todo_items WHERE done = 1", [])?;
    tx.commit()?;
    Ok(ids)
}

// ------------------------------------------------------------------ Rückgängig/Wiederholen

/// Umkehrbare Änderung an der Todo-Liste (für `undo::History`). Löschen von Karten und Tags gehört nicht dazu.
#[derive(Clone, Debug, PartialEq)]
pub enum TaskOp {
    /// Titel/Text/Erledigt/Tags einer Karte: `before` → `after`.
    Edit { id: i64, before: ItemState, after: ItemState },
    /// Karte ist (nicht) vorhanden: `present: true` legt sie mit derselben Nummer wieder an, `false` entfernt sie.
    Presence { item: Item, present: bool },
    /// Karte stand vor `from` und steht jetzt vor `to` (jeweils die Karte darunter, `None` = ganz unten).
    Move { id: i64, from: Option<i64>, to: Option<i64> },
    /// Name und Farbe eines Tags: `before` → `after`.
    TagEdit { id: i64, before: (String, String), after: (String, String) },
    /// Tag `from` ist in `into` aufgegangen (`merged: true`) bzw. wiederhergestellt. `moved`: (Karte, hatte `into` schon).
    Merge { from: Tag, into: i64, moved: Vec<(i64, bool)>, merged: bool },
}

impl TaskOp {
    /// Bezieht sich die Änderung auf diese Karte (Historie nach dem Löschen bereinigen)?
    pub fn touches_item(&self, ids: &[i64]) -> bool {
        match self {
            TaskOp::Edit { id, .. } | TaskOp::Move { id, .. } => ids.contains(id),
            TaskOp::Presence { item, .. } => ids.contains(&item.id),
            TaskOp::TagEdit { .. } | TaskOp::Merge { .. } => false,
        }
    }

    /// Bezieht sich die Änderung auf diesen Tag (Historie nach dem Löschen des Tags bereinigen)?
    pub fn touches_tag(&self, tag: i64) -> bool {
        match self {
            TaskOp::TagEdit { id, .. } => *id == tag,
            TaskOp::Merge { from, into, .. } => from.id == tag || *into == tag,
            _ => false,
        }
    }
}

impl Op for TaskOp {
    type Ctx = Connection;

    fn apply(&self, conn: &Connection) -> Result<(), String> {
        let run = || -> Res<()> {
            let tx = conn.unchecked_transaction()?;
            match self {
                TaskOp::Edit { id, after, .. } => apply_state(&tx, *id, after)?,
                TaskOp::Presence { item, present: true } => insert_item(&tx, item)?,
                TaskOp::Presence { item, present: false } => delete_item(&tx, item.id)?,
                TaskOp::Move { id, to, .. } => {
                    apply_move(&tx, *id, *to)?;
                }
                TaskOp::TagEdit { id, after, .. } => write_tag(&tx, *id, &after.0, &after.1)?,
                TaskOp::Merge { from, into, moved, merged: true } => do_merge(&tx, from.id, *into, moved)?,
                TaskOp::Merge { from, into, moved, merged: false } => undo_merge(&tx, from, *into, moved)?,
            }
            tx.commit()?;
            Ok(())
        };
        run().map_err(|e| e.message(state::lang()))
    }

    fn inverse(&self) -> Self {
        match self {
            TaskOp::Edit { id, before, after } => TaskOp::Edit { id: *id, before: after.clone(), after: before.clone() },
            TaskOp::Presence { item, present } => TaskOp::Presence { item: item.clone(), present: !present },
            TaskOp::Move { id, from, to } => TaskOp::Move { id: *id, from: *to, to: *from },
            TaskOp::TagEdit { id, before, after } => TaskOp::TagEdit { id: *id, before: after.clone(), after: before.clone() },
            TaskOp::Merge { from, into, moved, merged } => TaskOp::Merge { from: from.clone(), into: *into, moved: moved.clone(), merged: !merged },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::undo::History;

    fn db() -> Connection {
        crate::db::open_memory()
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn all() -> ItemFilter {
        ItemFilter { show_done: true, tags: vec![] }
    }

    fn ids(conn: &Connection, f: &ItemFilter) -> Vec<i64> {
        list_items(conn, f).unwrap().iter().map(|i| i.id).collect()
    }

    fn mk(conn: &Connection, text: &str) -> i64 {
        create_item(conn, "", text, None, &[]).unwrap().0.id
    }

    fn tag_names(conn: &Connection, item: i64) -> Vec<String> {
        let tags = list_tags(conn).unwrap();
        get_item(conn, item).unwrap().tags.iter().map(|t| tags.iter().find(|x| x.id == *t).unwrap().name.clone()).collect()
    }

    #[test]
    fn schema_has_foreign_keys_on() {
        let c = db();
        let on: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(on, 1);
        for t in ["todo_items", "todo_tags", "todo_item_tags"] {
            c.prepare(&format!("SELECT * FROM {t}")).unwrap();
        }
    }

    #[test]
    fn card_needs_title_or_text() {
        let c = db();
        for (t, b) in [("", ""), ("   ", "\n\t "), ("", "  ")] {
            assert_eq!(create_item(&c, t, b, None, &names(&["x"])).unwrap_err(), TaskError::Empty);
        }
        assert!(list_items(&c, &all()).unwrap().is_empty());
        assert!(list_tags(&c).unwrap().is_empty(), "bei einem Fehler entsteht kein Tag");
        // nur Titel, nur Text, beides – jeweils ohne Leerraum am Rand
        let a = create_item(&c, "  Titel  ", "", None, &[]).unwrap().0;
        assert_eq!((a.title.as_str(), a.body.as_str()), ("Titel", ""));
        let b = create_item(&c, "", " nur Text\n", None, &[]).unwrap().0;
        assert_eq!((b.title.as_str(), b.body.as_str()), ("", "nur Text"));
        let d = create_item(&c, "T", "B\nzweite Zeile", None, &[]).unwrap().0;
        assert_eq!((d.title.as_str(), d.body.as_str()), ("T", "B\nzweite Zeile"));
        // gleiche Regel beim Bearbeiten
        assert_eq!(edit_item(&c, a.id, " ", "", None, &[]).unwrap_err(), TaskError::Empty);
        assert_eq!(get_item(&c, a.id).unwrap().title, "Titel");
        assert!(edit_item(&c, a.id, "", "jetzt Text", None, &[]).unwrap().is_some());
        assert_eq!(get_item(&c, a.id).unwrap().title, "");
        // Fehlertexte in beiden Sprachen
        assert_ne!(TaskError::Empty.message(Lang::De), TaskError::Empty.message(Lang::En));
    }

    #[test]
    fn keys_are_sequential_and_never_reused() {
        let c = db();
        let ids_: Vec<i64> = (0..3).map(|i| mk(&c, &format!("k{i}"))).collect();
        assert_eq!(ids_, vec![1, 2, 3]);
        assert_eq!(get_item(&c, 3).unwrap().key(), "TODO-3");
        delete_item(&c, 3).unwrap();
        assert_eq!(mk(&c, "neu"), 4, "die Nummer 3 wird nie wiederverwendet");
        // auch nicht, wenn alle weg sind
        for i in [1, 2, 4] {
            delete_item(&c, i).unwrap();
        }
        assert_eq!(mk(&c, "wieder neu"), 5);
        // und nicht nach „Erledigte löschen“
        set_done(&c, 5, true).unwrap();
        assert_eq!(delete_done(&c).unwrap(), vec![5]);
        assert_eq!(mk(&c, "x"), 6);
        assert_eq!(delete_item(&c, 99).unwrap_err(), TaskError::NotFound);
    }

    #[test]
    fn new_cards_go_on_top_and_timestamps_are_iso() {
        let c = db();
        let (a, b, d) = (mk(&c, "a"), mk(&c, "b"), mk(&c, "c"));
        assert_eq!(ids(&c, &all()), vec![d, b, a]);
        let it = get_item(&c, a).unwrap();
        assert_eq!(it.created_at.len(), 19);
        assert_eq!(it.created_at.as_bytes()[10], b'T');
        assert_eq!(it.created_at, it.updated_at);
        assert!(it.done_at.is_none());
    }

    #[test]
    fn done_toggle_sets_timestamps() {
        let c = db();
        let a = mk(&c, "a");
        assert!(set_done(&c, a, false).unwrap().is_none(), "war schon offen");
        assert!(set_done(&c, a, true).unwrap().is_some());
        let it = get_item(&c, a).unwrap();
        assert!(it.done && it.done_at.is_some());
        assert!(set_done(&c, a, true).unwrap().is_none());
        set_done(&c, a, false).unwrap();
        let it = get_item(&c, a).unwrap();
        assert!(!it.done && it.done_at.is_none());
        assert_eq!(set_done(&c, 42, true).unwrap_err(), TaskError::NotFound);
    }

    #[test]
    fn moving_reorders_and_persists() {
        let c = db();
        let (a, b, d, e) = (mk(&c, "a"), mk(&c, "b"), mk(&c, "c"), mk(&c, "d")); // Liste: e d b a
        assert_eq!(ids(&c, &all()), vec![e, d, b, a]);
        move_before(&c, a, Some(e)).unwrap().unwrap(); // a ganz nach oben
        assert_eq!(ids(&c, &all()), vec![a, e, d, b]);
        move_before(&c, a, None).unwrap().unwrap(); // a ans Ende
        assert_eq!(ids(&c, &all()), vec![e, d, b, a]);
        move_before(&c, e, Some(b)).unwrap().unwrap(); // mitten hinein
        assert_eq!(ids(&c, &all()), vec![d, e, b, a]);
        // keine Änderung → keine Operation
        assert!(move_before(&c, d, Some(e)).unwrap().is_none());
        assert!(move_before(&c, a, None).unwrap().is_none());
        assert!(move_before(&c, d, Some(d)).unwrap().is_none());
        assert_eq!(move_before(&c, 77, None).unwrap_err(), TaskError::NotFound);
        // unbekannter Nachbar = ans Ende
        move_before(&c, d, Some(1234)).unwrap().unwrap();
        assert_eq!(ids(&c, &all()), vec![e, b, a, d]);
        // neue Karten landen weiter oben, die Reihenfolge der übrigen bleibt
        let n = mk(&c, "neu");
        assert_eq!(ids(&c, &all()), vec![n, e, b, a, d]);
    }

    #[test]
    fn tags_are_created_case_insensitively() {
        let c = db();
        let (it, _) = create_item(&c, "t", "", None, &names(&["Bug", "bug", " BUG ", "Feature", "", "  "])).unwrap();
        assert_eq!(it.tags.len(), 2, "Doppelte (auch anders geschrieben) zählen einmal");
        assert_eq!(tag_names(&c, it.id), vec!["Bug", "Feature"], "Anzeige wie zuerst angelegt");
        let (it2, _) = create_item(&c, "u", "", None, &names(&["BUG", "  neu   mit   Leerraum "])).unwrap();
        let tags = list_tags(&c).unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tag_names(&c, it2.id), vec!["Bug", "neu mit Leerraum"]);
        // Umlaute werden ebenfalls ohne Beachtung der Schreibweise verglichen
        let (it3, _) = create_item(&c, "v", "", None, &names(&["Ärger", "ärger"])).unwrap();
        assert_eq!(it3.tags.len(), 1);
        assert_eq!(find_tag(&c, "ÄRGER").unwrap().unwrap().name, "Ärger");
        // lange Namen werden gekürzt
        let long = "x".repeat(100);
        let (it4, _) = create_item(&c, "w", "", None, &[long]).unwrap();
        assert_eq!(list_tags(&c).unwrap().iter().find(|t| t.id == it4.tags[0]).unwrap().name.chars().count(), MAX_TAG_CHARS);
    }

    #[test]
    fn tag_colors_rotate_through_the_palette() {
        let c = db();
        let mut seen = Vec::new();
        let pal = crate::palette::auto(crate::palette::Area::Todo);
        for i in 0..pal.len() {
            create_item(&c, "t", "", None, &[format!("tag{i}")]).unwrap();
        }
        for t in list_tags(&c).unwrap() {
            assert!(pal.contains(&t.color));
            seen.push(t.color);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), pal.len(), "die ersten Tags haben lauter verschiedene Farben");
        // der nächste beginnt wieder von vorn
        create_item(&c, "t", "", None, &names(&["elf"])).unwrap();
        assert_eq!(find_tag(&c, "elf").unwrap().unwrap().color, pal[0]);
        // eigene Palette: Vergabe daraus, Kleinbuchstaben in der Datenbank zählen mit
        let pool = vec!["#010101".to_string(), "#020202".to_string()];
        assert_eq!(next_color_from(&c, &pool).unwrap(), "#010101");
        create_item(&c, "t", "", None, &names(&["zwölf"])).unwrap();
        set_tag_color(&c, find_tag(&c, "zwölf").unwrap().unwrap().id, "#010101").unwrap();
        assert_eq!(next_color_from(&c, &pool).unwrap(), "#020202");
        assert_eq!(rgb("#E06C75"), (0xe0, 0x6c, 0x75));
        assert_eq!(rgb("kaputt"), (0x97, 0xa3, 0xb0));
    }

    #[test]
    fn rename_and_color() {
        let c = db();
        let (a, _) = create_item(&c, "a", "", None, &names(&["alt"])).unwrap();
        let (b, _) = create_item(&c, "b", "", None, &names(&["alt", "andere"])).unwrap();
        let alt = find_tag(&c, "alt").unwrap().unwrap().id;
        let andere = find_tag(&c, "andere").unwrap().unwrap().id;
        // Umbenennen wirkt auf allen Karten
        rename_tag(&c, alt, "  Neu  ").unwrap().unwrap();
        assert_eq!(tag_names(&c, a.id), vec!["Neu"]);
        assert_eq!(tag_names(&c, b.id), vec!["Neu", "andere"]);
        assert!(find_tag(&c, "alt").unwrap().is_none());
        // nur die Schreibweise ändern geht, derselbe Name ist keine Änderung
        rename_tag(&c, alt, "NEU").unwrap().unwrap();
        assert!(rename_tag(&c, alt, "NEU").unwrap().is_none());
        // Konflikt mit einem anderen Tag
        assert_eq!(rename_tag(&c, alt, "Andere").unwrap_err(), TaskError::TagExists(andere));
        assert_eq!(find_tag(&c, "neu").unwrap().unwrap().name, "NEU", "bei einem Konflikt bleibt alles beim Alten");
        assert_eq!(rename_tag(&c, alt, "  ").unwrap_err(), TaskError::TagNameEmpty);
        assert_eq!(rename_tag(&c, 999, "x").unwrap_err(), TaskError::NotFound);
        // Farbe
        let p3 = crate::palette::TODO_DEFAULTS[3];
        assert!(set_tag_color(&c, alt, p3).unwrap().is_some());
        assert_eq!(find_tag(&c, "neu").unwrap().unwrap().color, p3);
        assert!(set_tag_color(&c, alt, &p3.to_lowercase()).unwrap().is_none());
    }

    #[test]
    fn merge_and_delete_tags() {
        let c = db();
        let (a, _) = create_item(&c, "a", "", None, &names(&["x"])).unwrap();
        let (b, _) = create_item(&c, "b", "", None, &names(&["x", "y"])).unwrap();
        let (d, _) = create_item(&c, "d", "", None, &names(&["y"])).unwrap();
        let (e, _) = create_item(&c, "e", "", None, &[]).unwrap();
        let x = find_tag(&c, "x").unwrap().unwrap().id;
        let y = find_tag(&c, "y").unwrap().unwrap().id;
        assert_eq!(merge_tags(&c, x, x).unwrap_err(), TaskError::NotFound);
        merge_tags(&c, x, y).unwrap();
        assert!(find_tag(&c, "x").unwrap().is_none());
        assert_eq!(tag_names(&c, a.id), vec!["y"]);
        assert_eq!(tag_names(&c, b.id), vec!["y"], "kein doppelter Tag bei Karten, die beide hatten");
        assert_eq!(tag_names(&c, d.id), vec!["y"]);
        assert!(tag_names(&c, e.id).is_empty());
        assert_eq!(tag_counts(&c).unwrap()[&y], 3);
        // Löschen entfernt den Tag von allen Karten und meldet die Zahl
        assert_eq!(delete_tag(&c, y).unwrap(), 3);
        assert!(list_tags(&c).unwrap().is_empty());
        assert!(tag_names(&c, a.id).is_empty());
        assert_eq!(delete_tag(&c, y).unwrap_err(), TaskError::NotFound);
        // die Karten selbst bleiben
        assert_eq!(counts(&c).unwrap(), (4, 0));
    }

    #[test]
    fn filters_by_tags_with_and_and_hides_done() {
        let c = db();
        let (a, _) = create_item(&c, "a", "", None, &names(&["rot"])).unwrap();
        let (b, _) = create_item(&c, "b", "", None, &names(&["rot", "blau"])).unwrap();
        let (d, _) = create_item(&c, "d", "", None, &names(&["blau"])).unwrap();
        let e = mk(&c, "e");
        let rot = find_tag(&c, "rot").unwrap().unwrap().id;
        let blau = find_tag(&c, "blau").unwrap().unwrap().id;
        let f = |tags: Vec<i64>, show_done| ItemFilter { show_done, tags };
        assert_eq!(ids(&c, &f(vec![], true)), vec![e, d.id, b.id, a.id]);
        assert_eq!(ids(&c, &f(vec![rot], true)), vec![b.id, a.id]);
        assert_eq!(ids(&c, &f(vec![blau], true)), vec![d.id, b.id]);
        assert_eq!(ids(&c, &f(vec![rot, blau], true)), vec![b.id], "mehrere Tags = UND");
        // Erledigte ausblenden
        set_done(&c, b.id, true).unwrap();
        assert_eq!(ids(&c, &f(vec![], false)), vec![e, d.id, a.id]);
        assert_eq!(ids(&c, &f(vec![rot, blau], false)), Vec::<i64>::new());
        assert_eq!(ids(&c, &f(vec![rot, blau], true)), vec![b.id]);
        assert_eq!(counts(&c).unwrap(), (4, 1));
    }

    #[test]
    fn delete_done_removes_only_done_and_links() {
        let c = db();
        let (a, _) = create_item(&c, "a", "", None, &names(&["t"])).unwrap();
        let (b, _) = create_item(&c, "b", "", None, &names(&["t"])).unwrap();
        let d = mk(&c, "d");
        set_done(&c, a.id, true).unwrap();
        set_done(&c, d, true).unwrap();
        assert_eq!(delete_done(&c).unwrap(), vec![a.id, d]);
        assert_eq!(ids(&c, &all()), vec![b.id]);
        assert!(delete_done(&c).unwrap().is_empty());
        // Fremdschlüssel: die Zuordnungen der gelöschten Karten sind weg, der Tag bleibt
        let links: i64 = c.query_row("SELECT COUNT(*) FROM todo_item_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(links, 1);
        assert_eq!(list_tags(&c).unwrap().len(), 1);
        delete_item(&c, b.id).unwrap();
        let links: i64 = c.query_row("SELECT COUNT(*) FROM todo_item_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(links, 0);
    }

    // ---- Rückgängig/Wiederholen

    /// Zustand der ganzen Liste als Vergleichswert.
    fn snapshot(c: &Connection) -> (Vec<Item>, Vec<Tag>) {
        let mut items = list_items(c, &all()).unwrap();
        for i in &mut items {
            i.updated_at.clear(); // die Aktualisierungszeit ändert sich bei jeder Operation
            i.position = 0; // nach einem Verschieben wird neu durchnummeriert; entscheidend ist die Reihenfolge
        }
        (items, list_tags(c).unwrap())
    }

    /// Liste wie `target`? Tags, die nur zwischendurch automatisch angelegt wurden, dürfen als unbenutzte Tags übrig sein.
    fn same_state(c: &Connection, target: &(Vec<Item>, Vec<Tag>)) -> bool {
        let (items, tags) = snapshot(c);
        let counts = tag_counts(c).unwrap();
        items == target.0
            && target.1.iter().all(|t| tags.contains(t))
            && tags.iter().filter(|t| !target.1.contains(t)).all(|t| counts.get(&t.id).copied().unwrap_or(0) == 0)
    }

    fn push(h: &mut History<TaskOp>, op: Option<TaskOp>) {
        h.push(op.expect("Änderung erwartet"));
    }

    #[test]
    fn undo_redo_roundtrip_of_every_change_kind() {
        let c = db();
        let mut h: History<TaskOp> = History::new(25);
        let mut states = vec![snapshot(&c)];

        // 1 anlegen (mit neuem Tag)
        let (a, op) = create_item(&c, "Titel", "Text", None, &names(&["Alpha"])).unwrap();
        h.push(op);
        states.push(snapshot(&c));
        // 2 anlegen
        let (b, op) = create_item(&c, "", "zweite", None, &names(&["Beta"])).unwrap();
        h.push(op);
        states.push(snapshot(&c));
        // 3 bearbeiten (Tag Alpha raus, Beta rein, Text geändert)
        push(&mut h, edit_item(&c, a.id, "Titel neu", "", None, &names(&["beta", "Gamma"])).unwrap());
        states.push(snapshot(&c));
        // 4 abhaken
        push(&mut h, set_done(&c, b.id, true).unwrap());
        states.push(snapshot(&c));
        // 5 verschieben
        push(&mut h, move_before(&c, b.id, None).unwrap());
        states.push(snapshot(&c));
        // 6 Tag umbenennen, 7 Farbe, 8 zusammenführen
        let alpha = find_tag(&c, "alpha").unwrap().unwrap().id;
        push(&mut h, rename_tag(&c, alpha, "Alpha 2").unwrap());
        states.push(snapshot(&c));
        push(&mut h, set_tag_color(&c, alpha, crate::palette::TODO_DEFAULTS[9]).unwrap());
        states.push(snapshot(&c));
        let gamma = find_tag(&c, "gamma").unwrap().unwrap().id;
        let beta = find_tag(&c, "beta").unwrap().unwrap().id;
        h.push(merge_tags(&c, gamma, beta).unwrap());
        states.push(snapshot(&c));
        assert_eq!(h.len(), 8);

        // komplett zurück …
        for i in (0..8).rev() {
            assert_eq!(h.undo(&c), Ok(true));
            assert!(same_state(&c, &states[i]), "Zustand nach dem {}. Zurücknehmen", 8 - i);
        }
        assert_eq!(h.undo(&c), Ok(false));
        // … und wieder vor
        for (i, st) in states.iter().enumerate().skip(1) {
            assert_eq!(h.redo(&c), Ok(true));
            assert!(same_state(&c, st), "Zustand nach dem {i}. Wiederholen");
        }
        assert_eq!(h.redo(&c), Ok(false));
    }

    #[test]
    fn undone_creation_keeps_its_number_on_redo() {
        let c = db();
        let mut h: History<TaskOp> = History::new(25);
        let a = mk(&c, "a");
        let (b, op) = create_item(&c, "b", "text", None, &names(&["T"])).unwrap();
        h.push(op);
        assert_eq!(h.undo(&c), Ok(true));
        assert!(get_item(&c, b.id).is_err());
        assert_eq!(h.redo(&c), Ok(true));
        let again = get_item(&c, b.id).unwrap();
        assert_eq!((again.id, again.position, again.created_at.clone()), (b.id, b.position, b.created_at.clone()));
        assert_eq!(tag_names(&c, b.id), vec!["T"]);
        assert_eq!(ids(&c, &all()), vec![b.id, a]);
        // nach Zurücknehmen + neuer Änderung wird die Nummer nicht vergeben, weil sie nie frei war
        h.undo(&c).unwrap();
        let (n, op) = create_item(&c, "n", "", None, &[]).unwrap();
        h.push(op);
        assert!(n.id > b.id, "die Nummer der zurückgenommenen Karte wird nicht neu vergeben");
        assert!(!h.can_redo());
    }

    #[test]
    fn new_change_after_undo_discards_future() {
        let c = db();
        let mut h: History<TaskOp> = History::new(25);
        let (a, op) = create_item(&c, "a", "", None, &[]).unwrap();
        h.push(op);
        push(&mut h, edit_item(&c, a.id, "b", "", None, &[]).unwrap());
        h.undo(&c).unwrap();
        assert!(h.can_redo());
        push(&mut h, set_done(&c, a.id, true).unwrap());
        assert!(!h.can_redo(), "die Zukunfts-Änderung (Bearbeiten) ist verworfen");
        assert_eq!(h.undo(&c), Ok(true));
        assert_eq!(get_item(&c, a.id).unwrap().title, "a");
        assert!(!get_item(&c, a.id).unwrap().done);
    }

    #[test]
    fn merge_undo_restores_tag_and_links_exactly() {
        let c = db();
        let (a, _) = create_item(&c, "a", "", None, &names(&["x"])).unwrap();
        let (b, _) = create_item(&c, "b", "", None, &names(&["x", "y"])).unwrap();
        let (d, _) = create_item(&c, "d", "", None, &names(&["y"])).unwrap();
        let x = find_tag(&c, "x").unwrap().unwrap();
        let y = find_tag(&c, "y").unwrap().unwrap();
        let before = snapshot(&c);
        let op = merge_tags(&c, x.id, y.id).unwrap();
        let after = snapshot(&c);
        assert_ne!(before, after);
        op.inverse().apply(&c).unwrap();
        assert_eq!(snapshot(&c), before);
        assert_eq!(tag_names(&c, a.id), vec!["x"]);
        assert_eq!(tag_names(&c, b.id), vec!["x", "y"], "b hatte y schon vorher");
        assert_eq!(tag_names(&c, d.id), vec!["y"]);
        op.apply(&c).unwrap();
        assert_eq!(snapshot(&c), after);
        // Der Name ist inzwischen von einem benutzten Tag belegt: Wiederherstellen schlägt sauber fehl.
        let (_, _) = create_item(&c, "n", "", None, &names(&["X"])).unwrap();
        assert!(op.inverse().apply(&c).is_err());
        // Ein unbenutzter Tag mit dem Namen weicht dagegen.
        let cc = db();
        let (_, _) = create_item(&cc, "a", "", None, &names(&["x", "y"])).unwrap();
        let x = find_tag(&cc, "x").unwrap().unwrap();
        let y = find_tag(&cc, "y").unwrap().unwrap();
        let op = merge_tags(&cc, x.id, y.id).unwrap();
        create_item(&cc, "n", "", None, &names(&["X"])).unwrap();
        let junk = find_tag(&cc, "x").unwrap().unwrap().id;
        delete_item(&cc, 2).unwrap(); // der Tag „X“ bleibt unbenutzt zurück
        assert!(list_tags(&cc).unwrap().iter().any(|t| t.id == junk));
        op.inverse().apply(&cc).unwrap();
        assert_eq!(find_tag(&cc, "x").unwrap().unwrap().id, x.id);
    }

    #[test]
    fn moves_undo_to_the_original_place() {
        let c = db();
        let mut h: History<TaskOp> = History::new(25);
        let (a, b, d, e) = (mk(&c, "a"), mk(&c, "b"), mk(&c, "c"), mk(&c, "d"));
        let start = ids(&c, &all());
        assert_eq!(start, vec![e, d, b, a]);
        push(&mut h, move_before(&c, a, Some(e)).unwrap());
        push(&mut h, move_before(&c, d, None).unwrap());
        push(&mut h, move_before(&c, b, Some(a)).unwrap());
        let end = ids(&c, &all());
        for _ in 0..3 {
            h.undo(&c).unwrap();
        }
        assert_eq!(ids(&c, &all()), start);
        for _ in 0..3 {
            h.redo(&c).unwrap();
        }
        assert_eq!(ids(&c, &all()), end);
    }

    #[test]
    fn history_survives_deletion_of_referenced_items() {
        let c = db();
        let mut h: History<TaskOp> = History::new(25);
        let (a, op) = create_item(&c, "a", "", None, &[]).unwrap();
        h.push(op);
        let (b, op) = create_item(&c, "b", "", None, &[]).unwrap();
        h.push(op);
        push(&mut h, edit_item(&c, a.id, "a2", "", None, &[]).unwrap());
        push(&mut h, set_done(&c, b.id, true).unwrap());
        // Löschen ist endgültig: a verschwindet samt seiner Einträge in der Historie
        delete_item(&c, a.id).unwrap();
        h.retain(|op| !op.touches_item(&[a.id]));
        assert_eq!(h.len(), 2);
        assert_eq!(h.undo(&c), Ok(true)); // b wieder offen
        assert!(!get_item(&c, b.id).unwrap().done);
        assert_eq!(h.undo(&c), Ok(true)); // b-Anlegen zurück
        assert!(get_item(&c, b.id).is_err());
        assert_eq!(h.undo(&c), Ok(false));
        // Ohne Bereinigung scheitert das Zurücknehmen sauber statt etwas Falsches zu tun
        let mut h2: History<TaskOp> = History::new(25);
        let (x, op) = create_item(&c, "x", "", None, &[]).unwrap();
        h2.push(op);
        push(&mut h2, edit_item(&c, x.id, "x2", "", None, &[]).unwrap());
        delete_item(&c, x.id).unwrap();
        assert!(h2.undo(&c).is_err());
        assert!(h2.undo(&c).is_err(), "auch das Rückgängigmachen des Anlegens: nichts mehr da");
        assert!(get_item(&c, x.id).is_err());
        // Tag gelöscht: Operationen auf ihn fallen weg, Karten-Änderungen laufen ohne den Tag weiter
        let mut h3: History<TaskOp> = History::new(25);
        let (y, op) = create_item(&c, "y", "", None, &names(&["T"])).unwrap();
        h3.push(op);
        let t = find_tag(&c, "T").unwrap().unwrap().id;
        h3.push(rename_tag(&c, t, "U").unwrap().unwrap());
        assert_eq!(delete_tag(&c, t).unwrap(), 1);
        assert!(h3.len() == 2);
        h3.retain(|op| !op.touches_tag(t));
        assert_eq!(h3.len(), 1);
        assert_eq!(h3.undo(&c), Ok(true));
        assert!(get_item(&c, y.id).is_err());
        assert_eq!(h3.redo(&c), Ok(true));
        assert!(get_item(&c, y.id).unwrap().tags.is_empty(), "der gelöschte Tag kommt nicht zurück");
    }

    #[test]
    fn edit_without_change_is_no_op() {
        let c = db();
        let (a, _) = create_item(&c, "a", "b", None, &names(&["t"])).unwrap();
        assert!(edit_item(&c, a.id, "a", "b", None, &names(&["T"])).unwrap().is_none());
        assert!(edit_item(&c, a.id, " a ", "b", None, &names(&["t"])).unwrap().is_none());
        assert!(edit_item(&c, a.id, "a", "b", None, &names(&["t", "u"])).unwrap().is_some());
        assert_eq!(edit_item(&c, 77, "a", "", None, &[]).unwrap_err(), TaskError::NotFound);
    }

    #[test]
    fn edit_keeps_done_state_and_updates_tags() {
        let c = db();
        let (a, _) = create_item(&c, "a", "", None, &names(&["x"])).unwrap();
        set_done(&c, a.id, true).unwrap();
        edit_item(&c, a.id, "a", "neuer Text", None, &names(&["y"])).unwrap().unwrap();
        let it = get_item(&c, a.id).unwrap();
        assert!(it.done && it.done_at.is_some());
        assert_eq!(it.body, "neuer Text");
        assert_eq!(tag_names(&c, a.id), vec!["y"]);
        // der entfernte Tag bleibt als unbenutzter Tag bestehen
        assert!(find_tag(&c, "x").unwrap().is_some());
        assert_eq!(*tag_counts(&c).unwrap().get(&find_tag(&c, "x").unwrap().unwrap().id).unwrap_or(&0), 0);
    }
}
