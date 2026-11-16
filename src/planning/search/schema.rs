//! Aufbau des Abfrage-Schemas aus der Konfiguration: welche Felder gibt es im Bereich (Projekt oder alle), welchen Typ haben sie,
//! welche Werte nehmen Auswahlfelder an – und wie sind sie gespeichert (die „Bindung“ für die SQL-Übersetzung).
//!
//! # Regeln
//!
//! * **Felder zusammenlegen:** Die benutzerdefinierten Felder aller Tickettypen des Bereichs werden nach Anzeigename (ohne Groß-/
//!   Kleinschreibung) zusammengelegt, wenn sie dieselbe *Art* haben (Text und Rich-Text zählen als eine Art, sonst gilt der Feldtyp
//!   genau). So wird aus „Priority“ in fünf Typen ein Feld `priority`. Dropdown-/Mehrfachauswahl-Werte werden über ihre Namen
//!   (in beiden Sprachen) zusammengelegt.
//! * **Konflikt (gleicher Name, verschiedene Art):** Es gewinnt die Art, die in den meisten Feldern vorkommt; bei Gleichstand die,
//!   die zuerst vorkommt (Reihenfolge: Projekt, Typ, Feld). Die übrigen Felder sind in diesem Bereich nicht abfragbar und stehen in
//!   [`SearchSchema::conflicts`] (die Oberfläche kann darauf hinweisen). Ein Feld, das wie ein eingebautes Pseudo-Feld heißt
//!   (`type`, `status`, `title` …), steht ebenfalls in den Konflikten: das eingebaute gewinnt.
//! * **Eingebaute Pseudo-Felder:** `key`, `type`, `status`, `summary`/`title`, `parent`, `created`, `updated`, `flagged`, `starred`,
//!   `project`, `sprint` (nur wenn ein Projekt des Bereichs Sprints nutzt), `changedBy`, `terminal`.
//! * **Eingebaute Felder jedes Tickets:** `description` (dt. auch „Beschreibung“; eigene Text-Felder dieses Namens gehören dazu)
//!   und `due` (dt. auch „Fällig“). Die Fälligkeit vergleicht „nur Datum“ als Ende des Tages; ein Vergleich mit einem reinen
//!   Datum vergleicht nur den Tag (`due = 2026-11-20` trifft auch 14:30 an dem Tag). Eigene Datums-Felder namens „Due“/„Fällig“
//!   (aus älteren Versionen) gehören zur Abfrage `due` dazu; ein eigenes Feld „Due date“ bleibt eigenständig.
//! * **Namen:** Anzeigename in der Sprache der Oberfläche; ist sie Deutsch, gilt zusätzlich der englische Name (die Hilfe nennt die
//!   englischen). Auswahlwerte (Typ, Status, Dropdown, Projekt) gelten immer in beiden Sprachen.
//! * **Schlüsselwörter:** Prädikate und Sortier-Schlüsselwörter kommen aus `views::keywords` (kaputte werden ausgelassen).

use super::error::{SRes, SearchError};
use super::{pending, Scope};
use crate::planning::fields::{self, Field, FieldKind};
use crate::planning::names::display_name;
use crate::planning::views::keywords::{self, BuiltinId, Cond, Definition, KeywordKind};
use crate::query::{FieldDef, FieldType, Schema};
use crate::state::Lang;
use rusqlite::{params, Connection};
use std::collections::HashMap;

/// Quellen der Änderung, die `changedBy` kennt (wie in `history.source`).
pub const CHANGED_BY: [&str; 4] = ["ui", "agent", "quick", "import"];

/// Zusammenführungs-Art eines Feldes: gleicher Name + gleiche Art = ein Schema-Feld.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    Text,
    Number,
    Bool,
    Date,
    DateTime,
    /// Die eingebaute Fälligkeit: Datum oder Datum mit Uhrzeit (nur Datum = Ende des Tages).
    Due,
    Dropdown,
    Multi,
    Labels,
    Ticket,
    Terminal,
    Version,
    Milestone,
}

pub(super) fn class_of(k: FieldKind) -> Class {
    match k {
        FieldKind::Text | FieldKind::Rich => Class::Text,
        FieldKind::Checkbox => Class::Bool,
        FieldKind::Dropdown => Class::Dropdown,
        FieldKind::Multi => Class::Multi,
        FieldKind::Number => Class::Number,
        FieldKind::Ticket => Class::Ticket,
        FieldKind::Terminal => Class::Terminal,
        FieldKind::Version => Class::Version,
        FieldKind::Milestone => Class::Milestone,
        FieldKind::Date => Class::Date,
        FieldKind::Datetime => Class::DateTime,
        FieldKind::Labels => Class::Labels,
    }
}

/// Ein zusammengelegter Auswahlwert: alle Namen (Anzeigename zuerst, beide Sprachen) und die Einträge, die dahinterstehen
/// (`owner`, `id`): bei Feld-Werten die Feld-Kennung und die Wert-Kennung, bei Typen/Status/Projekten `0` und die Kennung.
#[derive(Debug, Clone)]
pub(super) struct MOpt {
    pub names: Vec<String>,
    pub members: Vec<(i64, i64)>,
}

/// Geordnete Liste zusammengelegter Auswahlwerte (die Reihenfolge ist die Sortier-Reihenfolge).
#[derive(Debug, Clone, Default)]
pub(super) struct OptionSet {
    pub opts: Vec<MOpt>,
}

impl OptionSet {
    pub fn add(&mut self, owner: i64, id: i64, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let hit = self.opts.iter().position(|o| o.names.iter().any(|n| names.iter().any(|m| eq_ci(n, m))));
        match hit {
            Some(i) => {
                let o = &mut self.opts[i];
                for n in names {
                    if !o.names.iter().any(|x| eq_ci(x, n)) {
                        o.names.push(n.clone());
                    }
                }
                o.members.push((owner, id));
            }
            None => self.opts.push(MOpt { names: names.to_vec(), members: vec![(owner, id)] }),
        }
    }

    /// Die Werte, die zum Text passen (ohne Groß-/Kleinschreibung, in beiden Sprachen).
    pub fn matching(&self, text: &str) -> Vec<&MOpt> {
        self.opts.iter().filter(|o| o.names.iter().any(|n| eq_ci(n, text))).collect()
    }

    /// Alle Namen für den Validator.
    pub fn flat_names(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for o in &self.opts {
            for n in &o.names {
                if !v.iter().any(|x| eq_ci(x, n)) {
                    v.push(n.clone());
                }
            }
        }
        v
    }

    /// Stelle des Werts `(owner, id)` in der Reihenfolge.
    pub fn index_of(&self, owner: i64, id: i64) -> Option<usize> {
        self.opts.iter().position(|o| o.members.contains(&(owner, id)))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ValField {
    pub id: i64,
    pub kind: FieldKind,
}

/// Feld, dessen Werte in `ticket_values` stehen.
#[derive(Debug, Clone)]
pub(super) struct Vals {
    pub class: Class,
    pub fields: Vec<ValField>,
    pub opts: OptionSet,
    /// Auch die Terminal-Verknüpfungen (`links`) zählen als Wert (nur das Feld `terminal`).
    pub links: bool,
}

#[derive(Debug, Clone)]
pub(super) enum Store {
    Key,
    Title,
    Parent,
    Created,
    Updated,
    Flagged,
    Starred,
    ChangedBy,
    Sprint,
    Type(OptionSet),
    Status(OptionSet),
    Project(OptionSet),
    Values(Vals),
}

#[derive(Debug, Clone)]
pub(super) struct Binding {
    /// Anzeigename (zuerst registrierter Name).
    pub name: String,
    pub store: Store,
}

#[derive(Debug, Clone)]
pub(super) enum Pred {
    Blocked,
    Favorite,
    Custom(Cond),
}

#[derive(Debug, Clone)]
pub(super) enum SortKw {
    Favorites,
    Flagged,
    Custom { field_id: i64, dir: keywords::SortDir },
}

/// Ein Feldname, der in diesem Bereich nicht abfragbar ist (siehe Moduldoku).
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub name: String,
    /// Die Art, die gewonnen hat (`None` bei Überschattung durch ein eingebautes Feld).
    pub kept: Option<FieldKind>,
    /// Die Arten der Felder, die wegfallen.
    pub dropped: Vec<FieldKind>,
}

/// Schema für Validator und SQL-Übersetzung: das reine [`Schema`] plus die Speicher-Bindung jedes Feldes.
#[derive(Debug, Clone)]
pub struct SearchSchema {
    pub schema: Schema,
    pub scope: Scope,
    pub conflicts: Vec<Conflict>,
    pub(super) bindings: Vec<Binding>,
    by_name: HashMap<String, usize>,
    preds: Vec<(String, Pred)>,
    sorts: Vec<(String, SortKw)>,
    pub(super) kw_fields: HashMap<i64, Field>,
    /// Die aktiven Sprints des Bereichs (Kennung, Name) für `activeSprint()` (Stand beim Aufbau des Schemas).
    pub(super) active_sprints: Vec<(i64, String)>,
}

impl SearchSchema {
    pub(super) fn binding(&self, name: &str) -> Option<&Binding> {
        self.by_name.get(&name.to_lowercase()).map(|i| &self.bindings[*i])
    }

    pub(super) fn pred(&self, name: &str) -> Option<&Pred> {
        let n = name.to_lowercase();
        self.preds.iter().find(|(k, _)| *k == n).map(|(_, p)| p)
    }

    pub(super) fn sort_kw(&self, name: &str) -> Option<&SortKw> {
        let n = name.to_lowercase();
        self.sorts.iter().find(|(k, _)| *k == n).map(|(_, p)| p)
    }
}

/// Groß-/Kleinschreibung ignorierender Vergleich (Unicode).
pub(super) fn eq_ci(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

fn other(l: Lang) -> Lang {
    match l {
        Lang::De => Lang::En,
        Lang::En => Lang::De,
    }
}

/// Alle Namen eines Auswahlwerts: ein eigener Name gilt in beiden Sprachen, sonst beide Übersetzungen des Schlüssels.
fn value_names(key: Option<&str>, name: Option<&str>, lang: Lang) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        return vec![n.to_string()];
    }
    if let Some(k) = key {
        for l in [lang, other(lang)] {
            let d = display_name(Some(k), None, l);
            if !d.is_empty() && !v.iter().any(|x| eq_ci(x, &d)) {
                v.push(d);
            }
        }
    }
    v
}

/// Namen eines Feldes: Anzeigename in der Sprache, bei Deutsch zusätzlich der englische.
fn field_names(f: &Field, lang: Lang) -> Vec<String> {
    let mut v = vec![f.display(lang)];
    if lang == Lang::De {
        let en = f.display(Lang::En);
        if !en.is_empty() && !eq_ci(&en, &v[0]) {
            v.push(en);
        }
    }
    v.retain(|n| !n.trim().is_empty());
    v
}

struct Reg {
    bindings: Vec<Binding>,
    by_name: HashMap<String, usize>,
    defs: Vec<FieldDef>,
}

impl Reg {
    /// Registriert das Feld unter allen noch freien Namen. Liefert den Index (oder `None`, wenn kein Name frei war).
    fn add(&mut self, names: &[String], ty: FieldType, multi: bool, store: Store) -> Option<usize> {
        let free: Vec<&String> = names.iter().filter(|n| !self.by_name.contains_key(&n.to_lowercase())).collect();
        if free.is_empty() {
            return None;
        }
        let idx = self.bindings.len();
        self.bindings.push(Binding { name: free[0].clone(), store });
        for n in free {
            self.by_name.insert(n.to_lowercase(), idx);
            self.defs.push(FieldDef { name: n.clone(), ty: ty.clone(), multi });
        }
        Some(idx)
    }
}

struct ProjectRow {
    id: i64,
    key: String,
    name: String,
    use_sprints: bool,
}

struct TypeRow {
    id: i64,
    project_id: i64,
    key: Option<String>,
    name: Option<String>,
}

struct StatusRow {
    id: i64,
    project_id: i64,
    key: Option<String>,
    name: Option<String>,
}

fn scope_id(scope: Scope) -> Option<i64> {
    match scope {
        Scope::Project(p) => Some(p),
        Scope::All => None,
    }
}

/// Das reine Schema für den Validator (Felder, Prädikate, Sortier-Schlüsselwörter, Funktionen).
pub fn schema_for(conn: &Connection, scope: Scope, lang: Lang) -> SRes<Schema> {
    Ok(search_schema(conn, scope, lang)?.schema)
}

/// Das vollständige Schema samt Bindungen (für `compile`).
pub fn search_schema(conn: &Connection, scope: Scope, lang: Lang) -> SRes<SearchSchema> {
    let sid = scope_id(scope);
    // ---- Projekte
    let projects: Vec<ProjectRow> = {
        let mut st = conn.prepare("SELECT id, key, name, use_sprints FROM projects WHERE system = 0 AND (?1 IS NULL OR id = ?1) ORDER BY position, id")?;
        let rows = st.query_map(params![sid], |r| Ok(ProjectRow { id: r.get(0)?, key: r.get(1)?, name: r.get(2)?, use_sprints: r.get::<_, i64>(3)? != 0 }))?;
        rows.collect::<Result<_, _>>()?
    };
    if let Scope::Project(p) = scope {
        if projects.is_empty() {
            // ein System-Projekt oder ein unbekanntes: nur dann „nicht gefunden“, wenn es das Projekt gar nicht gibt
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", [p], |r| r.get(0))?;
            if n == 0 {
                return Err(SearchError::NotFound);
            }
        }
    }
    let order_of = |pid: i64, projects: &[ProjectRow]| projects.iter().position(|p| p.id == pid).unwrap_or(usize::MAX);
    // ---- Typen, Status, Felder
    let mut types: Vec<TypeRow> = {
        let mut st = conn.prepare("SELECT id, project_id, key, name FROM ticket_types WHERE system = 0 AND (?1 IS NULL OR project_id = ?1) ORDER BY position, id")?;
        let rows = st.query_map(params![sid], |r| Ok(TypeRow { id: r.get(0)?, project_id: r.get(1)?, key: r.get(2)?, name: r.get(3)? }))?;
        rows.collect::<Result<_, _>>()?
    };
    types.sort_by_key(|t| order_of(t.project_id, &projects)); // stabil: je Projekt bleibt die Reihenfolge der Typen
    let mut statuses: Vec<StatusRow> = {
        let mut st = conn.prepare("SELECT id, project_id, key, name FROM statuses WHERE project_id IN (SELECT id FROM projects WHERE system = 0 AND (?1 IS NULL OR id = ?1)) ORDER BY position, id")?;
        let rows = st.query_map(params![sid], |r| Ok(StatusRow { id: r.get(0)?, project_id: r.get(1)?, key: r.get(2)?, name: r.get(3)? }))?;
        rows.collect::<Result<_, _>>()?
    };
    statuses.sort_by_key(|s| order_of(s.project_id, &projects));
    let all_fields: Vec<Field> = {
        let mut st = conn.prepare(&format!(
            "SELECT {} FROM fields WHERE type_id IN (SELECT id FROM ticket_types WHERE system = 0 AND (?1 IS NULL OR project_id = ?1)) ORDER BY position, id",
            fields::COLS
        ))?;
        let rows = st.query_map(params![sid], fields::from_row)?;
        rows.collect::<Result<_, _>>()?
    };
    // Felder in der Reihenfolge Projekt, Typ, Feld
    let mut fields_in_order: Vec<&Field> = Vec::new();
    for t in &types {
        fields_in_order.extend(all_fields.iter().filter(|f| f.type_id == t.id));
    }

    // ---- Auswahllisten der eingebauten Felder
    let mut type_set = OptionSet::default();
    for t in &types {
        type_set.add(0, t.id, &value_names(t.key.as_deref(), t.name.as_deref(), lang));
    }
    let mut status_set = OptionSet::default();
    for s in &statuses {
        status_set.add(0, s.id, &value_names(s.key.as_deref(), s.name.as_deref(), lang));
    }
    let mut project_set = OptionSet::default();
    for p in &projects {
        project_set.add(0, p.id, &[p.key.clone(), p.name.clone()]);
    }

    let mut reg = Reg { bindings: Vec::new(), by_name: HashMap::new(), defs: Vec::new() };
    let de = lang == Lang::De;
    let nm = |en: &str, de_name: &str| -> Vec<String> {
        let mut v = vec![en.to_string()];
        if de && !eq_ci(en, de_name) {
            v.push(de_name.to_string());
        }
        v
    };
    reg.add(&nm("key", "Schlüssel"), FieldType::Text, false, Store::Key);
    reg.add(&nm("type", "Typ"), FieldType::Option(type_set.flat_names()), false, Store::Type(type_set));
    reg.add(&nm("status", "Status"), FieldType::Option(status_set.flat_names()), false, Store::Status(status_set));
    let mut title_names = vec!["summary".to_string(), "title".to_string()];
    if de {
        title_names.push("Titel".into());
        title_names.push("Zusammenfassung".into());
    }
    reg.add(&title_names, FieldType::Text, false, Store::Title);
    reg.add(&nm("parent", "Übergeordnet"), FieldType::TicketRef, false, Store::Parent);
    reg.add(&nm("created", "Erstellt"), FieldType::DateTime, false, Store::Created);
    reg.add(&nm("updated", "Geändert"), FieldType::DateTime, false, Store::Updated);
    reg.add(&nm("flagged", "Markiert"), FieldType::Bool, false, Store::Flagged);
    reg.add(&nm("starred", "Stern"), FieldType::Bool, false, Store::Starred);
    reg.add(&nm("project", "Projekt"), FieldType::Option(project_set.flat_names()), false, Store::Project(project_set));
    if projects.iter().any(|p| p.use_sprints) {
        // Auswahlwerte: die Namen der Sprints (siehe `pending.rs`)
        reg.add(&["sprint".to_string()], FieldType::Option(pending::sprint_names(conn, scope)?), false, Store::Sprint);
    }
    reg.add(&nm("changedBy", "GeändertVon"), FieldType::Option(CHANGED_BY.iter().map(|s| s.to_string()).collect()), false, Store::ChangedBy);
    let terminal_idx = reg
        .add(
            &["terminal".to_string()],
            FieldType::Terminal,
            false,
            Store::Values(Vals { class: Class::Terminal, fields: Vec::new(), opts: OptionSet::default(), links: true }),
        )
        .ok_or_else(|| SearchError::Internal("Feld `terminal` beim Aufbau schon belegt".to_string()))?;

    // ---- eingebaute Felder jedes Tickets: Beschreibung und Fälligkeit (über ihren Schlüssel, nicht den Anzeigenamen)
    let desc_fields: Vec<ValField> = fields_in_order.iter().filter(|f| f.is_description()).map(|f| ValField { id: f.id, kind: f.kind }).collect();
    let due_fields: Vec<ValField> = fields_in_order.iter().filter(|f| f.is_due()).map(|f| ValField { id: f.id, kind: f.kind }).collect();
    let desc_idx = reg.add(&nm("description", "Beschreibung"), FieldType::Text, false, Store::Values(Vals { class: Class::Text, fields: desc_fields, opts: OptionSet::default(), links: false }));
    let due_idx = reg.add(&nm("due", "Fällig"), FieldType::DateTime, false, Store::Values(Vals { class: Class::Due, fields: due_fields, opts: OptionSet::default(), links: false }));
    fields_in_order.retain(|f| !f.is_fixed());

    // ---- benutzerdefinierte Felder: nach Name und Art gruppieren
    struct Group {
        name_lc: String,
        names: Vec<String>,
        class: Class,
        fields: Vec<(ValField, Vec<(i64, Vec<String>)>)>, // Feld und seine Auswahlwerte (Wert-Kennung, Namen)
    }
    let mut groups: Vec<Group> = Vec::new();
    for f in &fields_in_order {
        let names = field_names(f, lang);
        let Some(first) = names.first() else { continue };
        let name_lc = first.to_lowercase();
        let class = class_of(f.kind);
        let opts: Vec<(i64, Vec<String>)> = f.options.values.iter().map(|o| (o.id, value_names(o.key.as_deref(), o.name.as_deref(), lang))).collect();
        let entry = (ValField { id: f.id, kind: f.kind }, opts);
        match groups.iter_mut().find(|g| g.name_lc == name_lc && g.class == class) {
            Some(g) => g.fields.push(entry),
            None => groups.push(Group { name_lc, names, class, fields: vec![entry] }),
        }
    }
    // pro Name die gewinnende Art bestimmen
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut seen_names: Vec<String> = Vec::new();
    let mut winners: Vec<usize> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        if seen_names.contains(&g.name_lc) {
            continue;
        }
        seen_names.push(g.name_lc.clone());
        let same: Vec<usize> = (0..groups.len()).filter(|j| groups[*j].name_lc == g.name_lc).collect();
        // meiste Felder gewinnen, bei Gleichstand die zuerst vorkommende
        let win = same.iter().copied().fold(i, |best, j| if groups[j].fields.len() > groups[best].fields.len() { j } else { best });
        winners.push(win);
        let dropped: Vec<FieldKind> = same.iter().filter(|j| **j != win).flat_map(|j| groups[*j].fields.iter().map(|f| f.0.kind)).collect();
        if !dropped.is_empty() {
            conflicts.push(Conflict { name: groups[win].names[0].clone(), kept: groups[win].fields.first().map(|f| f.0.kind), dropped });
        }
    }
    let versions_on = pending::versions_enabled(conn, scope)?;
    for wi in winners {
        let g = &groups[wi];
        // ohne Versionen im Bereich (Schalter `use_versions` überall aus) gibt es diese Felder in der Abfrage nicht
        if matches!(g.class, Class::Version | Class::Milestone) && !versions_on {
            continue;
        }
        // eigene Text-Felder namens „Description“/„Beschreibung“ gehören zur eingebauten Beschreibung
        if g.class == Class::Text && (g.name_lc == "description" || g.name_lc == "beschreibung") {
            if let Some(Store::Values(v)) = desc_idx.map(|i| &mut reg.bindings[i].store) {
                v.fields.extend(g.fields.iter().map(|f| f.0));
                continue;
            }
        }
        // eigene Datums-Felder namens „Due“/„Fällig“ (vor 2.3 angelegt) gehören zur eingebauten Fälligkeit (gleiche Speicherform)
        if matches!(g.class, Class::Date | Class::DateTime) && (g.name_lc == "due" || g.name_lc == "fällig") {
            if let Some(Store::Values(v)) = due_idx.map(|i| &mut reg.bindings[i].store) {
                v.fields.extend(g.fields.iter().map(|f| f.0));
                continue;
            }
        }
        // Terminal-Felder namens „terminal“ gehören zum eingebauten Feld
        if g.class == Class::Terminal && g.name_lc == "terminal" {
            if let Store::Values(v) = &mut reg.bindings[terminal_idx].store {
                v.fields.extend(g.fields.iter().map(|f| f.0));
            }
            continue;
        }
        let mut opts = OptionSet::default();
        if matches!(g.class, Class::Dropdown | Class::Multi) {
            for (vf, o) in &g.fields {
                for (oid, names) in o {
                    opts.add(vf.id, *oid, names);
                }
            }
        }
        let (ty, multi) = match g.class {
            Class::Text => (FieldType::Text, false),
            Class::Number => (FieldType::Number, false),
            Class::Bool => (FieldType::Bool, false),
            Class::Date => (FieldType::Date, false),
            Class::DateTime | Class::Due => (FieldType::DateTime, false),
            Class::Dropdown => (FieldType::Option(opts.flat_names()), false),
            Class::Multi => (FieldType::Option(opts.flat_names()), true),
            Class::Labels => (FieldType::Labels, true),
            Class::Ticket => (FieldType::TicketRef, false),
            Class::Terminal => (FieldType::Terminal, false),
            Class::Version | Class::Milestone => (FieldType::Option(pending::version_names(conn, scope, g.class == Class::Milestone)?), false),
        };
        let store = Store::Values(Vals { class: g.class, fields: g.fields.iter().map(|f| f.0).collect(), opts, links: false });
        if reg.add(&g.names, ty, multi, store).is_none() {
            // alle Namen sind von eingebauten Feldern belegt
            conflicts.push(Conflict { name: g.names[0].clone(), kept: None, dropped: g.fields.iter().map(|f| f.0.kind).collect() });
        }
    }

    // ---- Schlüsselwörter
    let mut preds: Vec<(String, Pred)> = Vec::new();
    let mut sorts: Vec<(String, SortKw)> = Vec::new();
    let mut kw_fields: HashMap<i64, Field> = HashMap::new();
    let broken = keywords::broken_keywords(conn)?;
    for k in keywords::list_keywords(conn)?.into_iter().filter(|k| !broken.contains(&k.id)) {
        let name = k.name.to_lowercase();
        match (&k.definition, k.kind) {
            (Definition::Builtin { id: BuiltinId::Blocked }, _) => preds.push((name, Pred::Blocked)),
            (Definition::Builtin { id: BuiltinId::Favorite }, _) => preds.push((name, Pred::Favorite)),
            (Definition::Builtin { id: BuiltinId::Favorites }, _) => sorts.push((name, SortKw::Favorites)),
            (Definition::Builtin { id: BuiltinId::Flagged }, _) => sorts.push((name, SortKw::Flagged)),
            (Definition::Predicate { condition }, KeywordKind::Predicate) => {
                for id in condition.field_ids() {
                    if let std::collections::hash_map::Entry::Vacant(e) = kw_fields.entry(id) {
                        e.insert(fields::get_field(conn, id)?);
                    }
                }
                preds.push((name, Pred::Custom(condition.clone())));
            }
            (Definition::Sort { field_id, dir }, KeywordKind::Sort) => {
                if let std::collections::hash_map::Entry::Vacant(e) = kw_fields.entry(*field_id) {
                    e.insert(fields::get_field(conn, *field_id)?);
                }
                sorts.push((name, SortKw::Custom { field_id: *field_id, dir: *dir }));
            }
            _ => {}
        }
    }
    let mut schema = Schema::new();
    schema.fields = reg.defs;
    schema.predicates = keyword_names(conn, true, &broken)?;
    schema.sort_keywords = keyword_names(conn, false, &broken)?;
    let active_sprints = if projects.iter().any(|p| p.use_sprints) { pending::active_sprints(conn, scope)? } else { Vec::new() };
    Ok(SearchSchema { schema, scope, conflicts, bindings: reg.bindings, by_name: reg.by_name, preds, sorts, kw_fields, active_sprints })
}

fn keyword_names(conn: &Connection, predicates: bool, broken: &[i64]) -> SRes<Vec<String>> {
    Ok(keywords::list_keywords(conn)?
        .into_iter()
        .filter(|k| !broken.contains(&k.id) && (k.kind == KeywordKind::Predicate) == predicates)
        .map(|k| k.name)
        .collect())
}
