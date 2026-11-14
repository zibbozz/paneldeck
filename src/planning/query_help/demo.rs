//! Beschreibung der Beispieldaten für die Screenshots der Abfrage-Hilfe.
//!
//! Ein [`DemoSpec`] ist ein kleines serde-Dokument (Projekt, Typen, Status, Prioritäten, Felder, Sprints, Versionen, Tickets samt
//! Feldwerten, Sternen und Markierungen). Aus ihm wird später – in einem eigenen Schritt – für jedes Beispiel eine frische Datenbank
//! befüllt, die Abfrage in der Suche ausgeführt und ein Screenshot gemacht. Die Seeding-Funktion ist NICHT Teil dieses Moduls; hier
//! liegen nur die reine Beschreibung, die Konsistenzprüfung ([`DemoSpec::check`]), das passende Abfrage-[`Schema`] ([`DemoSpec::schema`])
//! und die Referenz-Auswertung (`eval.rs`), die belegt, welche Tickets ein Beispiel trifft.
//!
//! Datumsangaben dürfen relativ zu „heute“ geschrieben werden (`@-3` = vor drei Tagen, `@0` = heute, `@+9` = in neun Tagen), damit
//! `currentDate()` und `activeSprint()` bei der Screenshot-Erzeugung an jedem Tag dasselbe Ergebnis liefern. [`DemoSpec::resolved`]
//! macht daraus absolute Daten (`JJJJ-MM-TT`); die Seeding-Funktion ruft es mit dem echten Datum auf.

use crate::planning::fields::FieldKind;
use crate::planning::statuses::StatusCategory;
use crate::query::{FieldType, Schema};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Zustand eines Sprints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SprintState {
    Planned,
    Active,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoStatus {
    pub name: String,
    pub category: StatusCategory,
}

/// Ein benutzerdefiniertes Feld (zusätzlich zu den festen Feldern Typ, Status, Priorität, Titel, Übergeordnet, Sprint, `changedBy`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoField {
    pub name: String,
    pub kind: FieldKind,
    /// Werte für Dropdown, Mehrfachauswahl und Meilenstein (Version nimmt die Werte aus [`DemoSpec::versions`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Für welche Tickettypen das Feld gilt (leer = alle).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoSprint {
    pub name: String,
    /// `JJJJ-MM-TT` oder relativ (`@-5`)
    pub start: String,
    pub end: String,
    pub state: SprintState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoVersion {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default)]
    pub released: bool,
}

/// Wert eines benutzerdefinierten Feldes. Datumsfelder stehen als Text (`2026-11-20` oder relativ `@+4`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DemoValue {
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoTicket {
    /// `WEB-12`; leer = beim Hinzufügen mit [`DemoSpec::add`] fortlaufend vergeben
    pub key: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprint: Option<String>,
    /// Schlüssel des übergeordneten Tickets
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Quelle der letzten Änderung (`ui`, `agent`, `import`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<String>,
    /// Stern (`IS favorite`)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub favorite: bool,
    /// Markierung „blockiert“ (`IS blocked`)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub blocked: bool,
    /// Benutzerdefinierte Felder: Feldname → Wert
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, DemoValue>,
}

impl DemoTicket {
    pub fn new(ty: &str, summary: &str) -> Self {
        Self { ty: ty.into(), summary: summary.into(), ..Self::default() }
    }

    pub fn status(mut self, s: &str) -> Self {
        self.status = Some(s.into());
        self
    }

    pub fn priority(mut self, p: &str) -> Self {
        self.priority = Some(p.into());
        self
    }

    pub fn sprint(mut self, s: &str) -> Self {
        self.sprint = Some(s.into());
        self
    }

    pub fn parent(mut self, key: &str) -> Self {
        self.parent = Some(key.into());
        self
    }

    pub fn changed_by(mut self, who: &str) -> Self {
        self.changed_by = Some(who.into());
        self
    }

    pub fn favorite(mut self) -> Self {
        self.favorite = true;
        self
    }

    pub fn blocked(mut self) -> Self {
        self.blocked = true;
        self
    }

    pub fn text(mut self, field: &str, v: &str) -> Self {
        self.values.insert(field.into(), DemoValue::Text(v.into()));
        self
    }

    pub fn num(mut self, field: &str, v: f64) -> Self {
        self.values.insert(field.into(), DemoValue::Number(v));
        self
    }

    pub fn list(mut self, field: &str, v: &[&str]) -> Self {
        self.values.insert(field.into(), DemoValue::List(v.iter().map(|s| s.to_string()).collect()));
        self
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoSpec {
    pub project_key: String,
    pub project_name: String,
    /// Tickettypen (Anzeigenamen, englisch wie in den Screenshots)
    pub types: Vec<String>,
    /// Status in Pool-Reihenfolge
    pub statuses: Vec<DemoStatus>,
    /// Prioritäten, wichtigste zuerst – so sortiert auch `ORDER BY priority`
    pub priorities: Vec<String>,
    pub fields: Vec<DemoField>,
    /// Leer = das Projekt arbeitet ohne Sprints (das Feld `sprint` entfällt)
    pub sprints: Vec<DemoSprint>,
    pub versions: Vec<DemoVersion>,
    /// Namen der laufenden Terminals (Auswahl des Feldtyps „Terminal“)
    pub terminals: Vec<String>,
    /// Das Terminal, aus dem die Suche gestartet wird (`linkedTerminal()`)
    pub current_terminal: Option<String>,
    pub tickets: Vec<DemoTicket>,
}

/// Die Quellen, die `changedBy` kennt.
pub const CHANGED_BY_VALUES: [&str; 3] = ["ui", "agent", "import"];
/// Prädikate (`IS favorite`, `IS blocked`) und Sortier-Schlüsselwörter (`SORT BY favorites`) der Beispiele.
pub const PREDICATES: [&str; 2] = ["favorite", "blocked"];
pub const SORT_KEYWORDS: [&str; 1] = ["favorites"];

impl DemoSpec {
    /// Hängt ein Ticket an; ein leerer Schlüssel wird fortlaufend vergeben. Gibt den Schlüssel zurück.
    pub fn add(&mut self, mut t: DemoTicket) -> String {
        if t.key.is_empty() {
            t.key = format!("{}-{}", self.project_key, self.tickets.len() + 1);
        }
        let key = t.key.clone();
        self.tickets.push(t);
        key
    }

    pub fn ticket(&self, key: &str) -> Option<&DemoTicket> {
        self.tickets.iter().find(|t| t.key.eq_ignore_ascii_case(key))
    }

    pub fn active_sprint(&self) -> Option<&DemoSprint> {
        self.sprints.iter().find(|s| s.state == SprintState::Active)
    }

    /// Das zur Beschreibung passende Abfrage-Schema (Felder, Prädikate, Sortier-Schlüsselwörter, eingebaute Funktionen).
    pub fn schema(&self) -> Schema {
        let names = |v: &[String]| v.to_vec();
        let mut s = Schema::new()
            .with_field("type", FieldType::Option(names(&self.types)))
            .with_field("status", FieldType::Option(self.statuses.iter().map(|x| x.name.clone()).collect()))
            .with_field("priority", FieldType::Option(names(&self.priorities)))
            .with_field("summary", FieldType::Text)
            .with_field("parent", FieldType::TicketRef)
            .with_field("changedBy", FieldType::Option(CHANGED_BY_VALUES.iter().map(|x| x.to_string()).collect()));
        if !self.sprints.is_empty() {
            s = s.with_field("sprint", FieldType::Option(self.sprints.iter().map(|x| x.name.clone()).collect()));
        }
        for f in &self.fields {
            s = match f.kind {
                FieldKind::Text | FieldKind::Rich => s.with_field(&f.name, FieldType::Text),
                FieldKind::Checkbox => s.with_field(&f.name, FieldType::Bool),
                FieldKind::Number => s.with_field(&f.name, FieldType::Number),
                FieldKind::Ticket => s.with_field(&f.name, FieldType::TicketRef),
                FieldKind::Terminal => s.with_field(&f.name, FieldType::Terminal),
                FieldKind::Date => s.with_field(&f.name, FieldType::Date),
                FieldKind::Datetime => s.with_field(&f.name, FieldType::DateTime),
                FieldKind::Labels => s.with_field(&f.name, FieldType::Labels),
                FieldKind::Dropdown | FieldKind::Milestone => s.with_field(&f.name, FieldType::Option(names(&f.options))),
                FieldKind::Multi => s.with_multi_field(&f.name, FieldType::Option(names(&f.options))),
                FieldKind::Version => s.with_field(&f.name, FieldType::Option(self.versions.iter().map(|v| v.name.clone()).collect())),
            };
        }
        for p in PREDICATES {
            s = s.with_predicate(p);
        }
        for k in SORT_KEYWORDS {
            s = s.with_sort_keyword(k);
        }
        s
    }

    /// Kopie mit absoluten Daten: `@±N`-Angaben (auch mit angehängter Uhrzeit `@+2T14:30`) werden zu `JJJJ-MM-TT`.
    /// `today` ist `JJJJ-MM-TT`; ungültige Angaben bleiben unverändert (und fallen in [`DemoSpec::check`] auf).
    pub fn resolved(&self, today: &str) -> DemoSpec {
        let Ok(today) = NaiveDate::parse_from_str(today, "%Y-%m-%d") else { return self.clone() };
        let mut s = self.clone();
        for sp in &mut s.sprints {
            sp.start = resolve_day(&sp.start, today);
            sp.end = resolve_day(&sp.end, today);
        }
        for v in &mut s.versions {
            v.due = v.due.as_deref().map(|d| resolve_day(d, today));
        }
        let date_fields: Vec<String> =
            self.fields.iter().filter(|f| matches!(f.kind, FieldKind::Date | FieldKind::Datetime)).map(|f| f.name.to_lowercase()).collect();
        for t in &mut s.tickets {
            for (name, v) in t.values.iter_mut() {
                if date_fields.contains(&name.to_lowercase()) {
                    if let DemoValue::Text(d) = v {
                        *d = resolve_day(d, today);
                    }
                }
            }
        }
        s
    }

    /// Prüft die Beschreibung auf Widersprüche (unbekannte Typen/Status/…, doppelte Schlüssel, Werte für Felder, die der Typ nicht hat).
    /// Leere Liste = in Ordnung. Das braucht die Seeding-Funktion später als Voraussetzung.
    pub fn check(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut keys: Vec<&str> = Vec::new();
        let has = |list: &[String], v: &str| list.iter().any(|x| x.eq_ignore_ascii_case(v));
        if self.project_key.len() < 2 || self.project_key.len() > 10 {
            problems.push(format!("project key {:?} must have 2-10 characters", self.project_key));
        }
        if self.sprints.iter().filter(|s| s.state == SprintState::Active).count() > 1 {
            problems.push("more than one active sprint".to_string());
        }
        if let Some(c) = &self.current_terminal
            && !has(&self.terminals, c)
        {
            problems.push(format!("current terminal {c:?} is not in the terminal list"));
        }
        for sp in &self.sprints {
            for d in [&sp.start, &sp.end] {
                if !is_absolute_or_relative_day(d) {
                    problems.push(format!("sprint {:?}: bad date {d:?}", sp.name));
                }
            }
        }
        for v in &self.versions {
            if let Some(d) = &v.due
                && !is_absolute_or_relative_day(d)
            {
                problems.push(format!("version {:?}: bad date {d:?}", v.name));
            }
        }
        for t in &self.tickets {
            let at = |m: String| format!("{}: {m}", t.key);
            if t.key.is_empty() || keys.iter().any(|k| k.eq_ignore_ascii_case(&t.key)) {
                problems.push(at("empty or duplicate key".into()));
            }
            keys.push(&t.key);
            if !has(&self.types, &t.ty) {
                problems.push(at(format!("unknown type {:?}", t.ty)));
            }
            if let Some(s) = &t.status
                && !self.statuses.iter().any(|x| x.name.eq_ignore_ascii_case(s))
            {
                problems.push(at(format!("unknown status {s:?}")));
            }
            if let Some(p) = &t.priority
                && !has(&self.priorities, p)
            {
                problems.push(at(format!("unknown priority {p:?}")));
            }
            if let Some(s) = &t.sprint
                && !self.sprints.iter().any(|x| x.name.eq_ignore_ascii_case(s))
            {
                problems.push(at(format!("unknown sprint {s:?}")));
            }
            if let Some(p) = &t.parent
                && !self.tickets.iter().any(|x| x.key.eq_ignore_ascii_case(p))
            {
                problems.push(at(format!("unknown parent {p:?}")));
            }
            if let Some(c) = &t.changed_by
                && !CHANGED_BY_VALUES.contains(&c.as_str())
            {
                problems.push(at(format!("unknown changedBy {c:?}")));
            }
            for (name, v) in &t.values {
                let Some(def) = self.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name)) else {
                    problems.push(at(format!("unknown field {name:?}")));
                    continue;
                };
                if !def.types.is_empty() && !has(&def.types, &t.ty) {
                    problems.push(at(format!("field {name:?} does not exist on type {:?}", t.ty)));
                }
                match (def.kind, v) {
                    (FieldKind::Date | FieldKind::Datetime, DemoValue::Text(d)) if !is_absolute_or_relative_day(d) => {
                        problems.push(at(format!("field {name:?}: bad date {d:?}")));
                    }
                    (FieldKind::Date | FieldKind::Datetime, DemoValue::Text(_)) => {}
                    (FieldKind::Date | FieldKind::Datetime, _) => problems.push(at(format!("field {name:?} needs a date text"))),
                    (FieldKind::Number, DemoValue::Number(_)) => {}
                    (FieldKind::Number, _) => problems.push(at(format!("field {name:?} needs a number"))),
                    (FieldKind::Checkbox, DemoValue::Bool(_)) => {}
                    (FieldKind::Checkbox, _) => problems.push(at(format!("field {name:?} needs a bool"))),
                    (FieldKind::Labels | FieldKind::Multi, DemoValue::List(_)) => {}
                    (FieldKind::Labels | FieldKind::Multi, _) => problems.push(at(format!("field {name:?} needs a list"))),
                    (FieldKind::Dropdown | FieldKind::Milestone, DemoValue::Text(x)) if !has(&def.options, x) => {
                        problems.push(at(format!("field {name:?}: {x:?} is not one of its options")));
                    }
                    (FieldKind::Version, DemoValue::Text(x)) if !self.versions.iter().any(|y| y.name.eq_ignore_ascii_case(x)) => {
                        problems.push(at(format!("field {name:?}: unknown version {x:?}")));
                    }
                    (FieldKind::Terminal, DemoValue::Text(x)) if !has(&self.terminals, x) => {
                        problems.push(at(format!("field {name:?}: unknown terminal {x:?}")));
                    }
                    (FieldKind::Ticket, DemoValue::Text(x)) if !self.tickets.iter().any(|y| y.key.eq_ignore_ascii_case(x)) => {
                        problems.push(at(format!("field {name:?}: unknown ticket {x:?}")));
                    }
                    (FieldKind::Dropdown | FieldKind::Milestone | FieldKind::Version | FieldKind::Terminal | FieldKind::Ticket | FieldKind::Text | FieldKind::Rich, DemoValue::Text(_)) => {}
                    (k, _) => problems.push(at(format!("field {name:?} ({k:?}) has a value of the wrong shape"))),
                }
            }
        }
        problems
    }
}

/// `@-3`, `@0`, `@+9` (optional mit `T14:30`) → absolutes Datum; alles andere bleibt unverändert.
pub fn resolve_day(s: &str, today: NaiveDate) -> String {
    let Some(rest) = s.strip_prefix('@') else { return s.to_string() };
    let (days, time) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    let Ok(n) = days.trim_start_matches('+').parse::<i64>() else { return s.to_string() };
    let day = today + Duration::days(n);
    match time {
        Some(t) => format!("{}T{t}", day.format("%Y-%m-%d")),
        None => day.format("%Y-%m-%d").to_string(),
    }
}

fn is_absolute_or_relative_day(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix('@') {
        return rest.split_once('T').map_or(rest, |(d, _)| d).trim_start_matches('+').parse::<i64>().is_ok();
    }
    s.get(..10).is_some_and(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()) && (s.len() == 10 || s.get(10..11) == Some("T"))
}
