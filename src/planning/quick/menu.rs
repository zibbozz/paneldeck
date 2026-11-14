//! Quick-Menü (Taste `.` im Ticket): Einträge zusammenstellen und filtern. Reine Logik; die Oberfläche zeichnet das Menü.
//!
//! * [`menu_entries`] liefert für ein Ticket die Einträge in dieser Reihenfolge: der **Titel** des Tickets, die **Felder** des
//!   Tickettyps (mit Feldtyp und aktuellem Wert), die **erlaubten Statusübergänge** (Name des Zielstatus) und die gültigen
//!   **Quick-Actions** des Typs. Hat ein Feld eine Eingabe nach Enter, steht deren Form in [`MenuEntry::input`].
//! * [`filter_entries`] sortiert nach der tolerant gematchten Eingabe (siehe `matcher.rs`), bei gleicher Punktzahl bleibt die
//!   ursprüngliche Reihenfolge.
//! * Erlaubt ist ein Übergang laut Übergangsmatrix und, wenn der Übergang Bedingungen hat, laut Feldwerten des Tickets
//!   (`workflow::evaluate_conditions`). Pflichtfelder und zu setzende Werte des Übergangs behandelt die Ticket-Schicht.
//!   Dasselbe gilt für „Status ändern“-Aktionen: ist der Übergang gesperrt, fehlt der Eintrag.

use super::actions::{list_project_quick_actions, QuickAction};
use super::input::InputKind;
use super::matcher;
use super::params::ActionParams;
use super::QRes;
use crate::planning::fields::{Field, FieldKind};
use crate::planning::names;
use crate::planning::statuses::{list_statuses, Status};
use crate::planning::summary::{project_summary, ProjectSummary};
use crate::planning::types::{get_type, TicketType};
use crate::planning::workflow::{evaluate_conditions, Transition};
use crate::planning::PlanError;
use crate::state::Lang;
use serde_json::Value;
use std::collections::HashMap;

/// Worauf ein Eintrag zielt.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    /// Der Titel des Tickets (Texteingabe).
    Title,
    Field { field_id: i64, kind: FieldKind },
    Transition { to_status_id: i64 },
    Action { action_id: i64 },
}

impl EntryKind {
    /// Überschrift der Gruppe in der Menüliste.
    pub fn group_label(&self, lang: Lang) -> &'static str {
        match self {
            EntryKind::Title | EntryKind::Field { .. } => lang.t("Felder", "Fields"),
            EntryKind::Transition { .. } => lang.t("Status", "Status"),
            EntryKind::Action { .. } => lang.t("Aktionen", "Actions"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    pub kind: EntryKind,
    /// Hauptzeile: Name des Feldes, des Zielstatus oder der Aktion.
    pub label: String,
    /// Weitere Namen, die ebenfalls treffen (Name in der anderen Sprache).
    pub aliases: Vec<String>,
    /// Zweitzeile: bei Feldern „Feldtyp · aktueller Wert“, bei Übergängen „Von → Nach“, bei Aktionen eine Beschreibung.
    pub detail: String,
    /// Eingabeform nach Enter (nur Titel und Felder).
    pub input: Option<InputKind>,
    /// Aktueller Wert des Feldes (roh, wie gespeichert).
    pub current: Option<Value>,
    /// Farbe des Zielstatus (`#RRGGBB`), sonst `None`.
    pub color: Option<String>,
}

impl MenuEntry {
    /// Alle Bezeichnungen, gegen die die Eingabe geprüft wird.
    pub fn search_terms(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.label.as_str()).chain(self.aliases.iter().map(String::as_str))
    }
}

/// Was die Menü-Logik vom Ticket braucht.
#[derive(Debug, Clone, Default)]
pub struct TicketContext {
    pub title: String,
    /// Aktueller Status; `None` bei Typen ohne Workflow.
    pub status_id: Option<i64>,
    /// Aktuelle Feldwerte (Feld-Id → Wert).
    pub values: HashMap<i64, Value>,
    /// Anzeigetexte für Verweis-Felder (Ticket, Terminal, Version, Meilenstein), z. B. „PAN-12 Login“; die Menü-Logik kann sie nicht auflösen.
    pub labels: HashMap<i64, String>,
}

/// Die Konfiguration des Tickettyps für das Menü.
#[derive(Debug, Clone)]
pub struct MenuConfig {
    pub ticket_type: TicketType,
    pub fields: Vec<Field>,
    /// Alle Status des Projekts.
    pub statuses: Vec<Status>,
    pub workflow_statuses: Vec<i64>,
    pub transitions: Vec<Transition>,
    /// Alle Typen des Projekts (für die Beschreibung von „Ticket anlegen“).
    pub types: Vec<TicketType>,
    /// Aktionen des Typs in Anzeigereihenfolge.
    pub actions: Vec<QuickAction>,
}

impl MenuConfig {
    /// Aus der gebündelten Projektübersicht und den Aktionen des Typs.
    pub fn from_summary(s: &ProjectSummary, type_id: i64, actions: Vec<QuickAction>) -> Option<MenuConfig> {
        let t = s.type_by_id(type_id)?;
        Some(MenuConfig {
            ticket_type: t.ticket_type.clone(),
            fields: t.fields.clone(),
            statuses: s.statuses.clone(),
            workflow_statuses: t.workflow_statuses.clone(),
            transitions: t.transitions.clone(),
            types: s.types.iter().map(|x| x.ticket_type.clone()).collect(),
            actions: actions.into_iter().filter(|a| a.type_id == type_id).collect(),
        })
    }

    /// Lädt alles aus der Datenbank.
    pub fn load(conn: &rusqlite::Connection, type_id: i64) -> QRes<MenuConfig> {
        let t = get_type(conn, type_id)?;
        let s = project_summary(conn, t.project_id)?;
        let actions = list_project_quick_actions(conn, t.project_id)?;
        MenuConfig::from_summary(&s, type_id, actions).ok_or(PlanError::NotFound.into())
    }

    fn status_name(&self, id: i64, lang: Lang) -> String {
        self.statuses.iter().find(|s| s.id == id).map(|s| s.display(lang)).unwrap_or_default()
    }

    fn type_name(&self, id: i64, lang: Lang) -> String {
        self.types.iter().find(|t| t.id == id).map(|t| t.display(lang)).unwrap_or_default()
    }
}

/// Status, in die das Ticket jetzt wechseln darf (in Workflow-Reihenfolge). Ohne Workflow leer; ohne aktuellen Status
/// (noch keiner gesetzt) alle Status des Workflows.
pub fn allowed_status_targets(cfg: &MenuConfig, ctx: &TicketContext) -> Vec<i64> {
    if !cfg.ticket_type.workflow {
        return Vec::new();
    }
    let wf = &cfg.workflow_statuses;
    let Some(from) = ctx.status_id else { return wf.clone() };
    if !wf.contains(&from) {
        return Vec::new();
    }
    let value_of = |f: i64| ctx.values.get(&f).cloned();
    wf.iter()
        .copied()
        .filter(|to| *to != from)
        .filter(|to| {
            if cfg.transitions.is_empty() {
                return true;
            }
            cfg.transitions.iter().find(|t| t.def.from_status_id == from && t.def.to_status_id == *to).is_some_and(|t| evaluate_conditions(&t.def.conditions, &value_of))
        })
        .collect()
}

/// Bezeichnung des Feldtyps.
pub fn kind_label(kind: FieldKind, lang: Lang) -> &'static str {
    match kind {
        FieldKind::Text => lang.t("Text", "Text"),
        FieldKind::Rich => lang.t("Formatierter Text", "Rich text"),
        FieldKind::Checkbox => lang.t("Kontrollkästchen", "Checkbox"),
        FieldKind::Dropdown => lang.t("Auswahlliste", "Dropdown"),
        FieldKind::Number => lang.t("Zahl", "Number"),
        FieldKind::Ticket => lang.t("Ticket-Verweis", "Ticket link"),
        FieldKind::Terminal => lang.t("Terminal", "Terminal"),
        FieldKind::Version => lang.t("Version", "Version"),
        FieldKind::Milestone => lang.t("Meilenstein", "Milestone"),
        FieldKind::Date => lang.t("Datum", "Date"),
        FieldKind::Datetime => lang.t("Datum und Uhrzeit", "Date and time"),
        FieldKind::Multi => lang.t("Mehrfachauswahl", "Multi-select"),
        FieldKind::Labels => lang.t("Labels", "Labels"),
    }
}

fn shorten(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > max { format!("{}…", line.chars().take(max).collect::<String>().trim_end()) } else { line.to_string() }
}

fn number_text(x: f64, decimals: Option<u8>) -> String {
    match decimals {
        Some(d) => format!("{x:.*}", d as usize),
        None if x.fract() == 0.0 && x.abs() < 1e15 => format!("{}", x as i64),
        None => format!("{x}"),
    }
}

fn date_text(s: &str, lang: Lang) -> String {
    if lang == Lang::En {
        return s.replace('T', " ");
    }
    let (d, t) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let parts: Vec<&str> = d.split('-').collect();
    let de = if parts.len() == 3 { format!("{}.{}.{}", parts[2], parts[1], parts[0]) } else { d.to_string() };
    match t {
        Some(t) => format!("{de} {t}"),
        None => de,
    }
}

/// Der Wert eines Feldes als kurzer Anzeigetext (leer, wenn nichts gesetzt ist).
pub fn format_value(f: &Field, v: Option<&Value>, label: Option<&str>, lang: Lang) -> String {
    let Some(v) = v.filter(|v| !v.is_null()) else { return String::new() };
    let opt_name = |id: &Value| f.options.values.iter().find(|o| Some(o.id) == id.as_i64()).map(|o| o.display(lang));
    match f.kind {
        FieldKind::Text | FieldKind::Rich => shorten(v.as_str().unwrap_or_default(), 40),
        FieldKind::Checkbox => match v.as_bool() {
            Some(true) => lang.t("Ja", "Yes").to_string(),
            Some(false) => lang.t("Nein", "No").to_string(),
            None => String::new(),
        },
        FieldKind::Dropdown => opt_name(v).unwrap_or_default(),
        FieldKind::Multi => v.as_array().map(|a| a.iter().filter_map(opt_name).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        FieldKind::Labels => v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        FieldKind::Number => {
            let n = v.as_f64().map(|x| number_text(x, f.options.decimals)).unwrap_or_default();
            match &f.options.unit {
                Some(u) if !n.is_empty() => format!("{n} {u}"),
                _ => n,
            }
        }
        FieldKind::Date | FieldKind::Datetime => date_text(v.as_str().unwrap_or_default(), lang),
        FieldKind::Ticket | FieldKind::Terminal | FieldKind::Version | FieldKind::Milestone => label.map(str::to_string).unwrap_or_default(),
    }
}

/// Zusätzliche Namen aus der anderen Sprache (nur bei übersetzbaren Standardnamen).
fn aliases(key: Option<&str>, name: Option<&str>, lang: Lang, label: &str) -> Vec<String> {
    if name.is_some_and(|n| !n.trim().is_empty()) {
        return Vec::new();
    }
    let other = match lang {
        Lang::De => Lang::En,
        Lang::En => Lang::De,
    };
    let alt = names::display_name(key, name, other);
    if alt.is_empty() || alt == label { Vec::new() } else { vec![alt] }
}

fn action_detail(cfg: &MenuConfig, a: &QuickAction, lang: Lang) -> String {
    match &a.params {
        ActionParams::CreateTicket(c) => format!("{}: {}", lang.t("Ticket anlegen", "Create ticket"), cfg.type_name(c.ticket_type_id, lang)),
        ActionParams::ChangeStatus(c) => format!("{}: {}", lang.t("Status setzen", "Set status"), cfg.status_name(c.to_status_id, lang)),
        ActionParams::StartSession(c) => {
            if c.claude { lang.t("Claude-Sitzung starten", "Start Claude session").to_string() } else { lang.t("Sitzung starten", "Start session").to_string() }
        }
    }
}

/// Alle Einträge des Quick-Menüs für ein Ticket (siehe Moduldoku).
pub fn menu_entries(cfg: &MenuConfig, ctx: &TicketContext, lang: Lang) -> Vec<MenuEntry> {
    let mut out = Vec::new();
    // Titel
    out.push(MenuEntry {
        kind: EntryKind::Title,
        label: lang.t("Titel", "Title").to_string(),
        aliases: vec![lang.t("Title", "Titel").to_string()],
        detail: [lang.t("Text", "Text").to_string(), shorten(&ctx.title, 40)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "),
        input: Some(InputKind::Text { max_length: Some(super::resolve::MAX_TITLE_CHARS as u32) }),
        current: Some(Value::String(ctx.title.clone())),
        color: None,
    });
    // Felder
    for f in &cfg.fields {
        let label = f.display(lang);
        let value = ctx.values.get(&f.id);
        let shown = format_value(f, value, ctx.labels.get(&f.id).map(String::as_str), lang);
        let kind = kind_label(f.kind, lang);
        out.push(MenuEntry {
            kind: EntryKind::Field { field_id: f.id, kind: f.kind },
            aliases: aliases(f.key.as_deref(), f.name.as_deref(), lang, &label),
            label,
            detail: if shown.is_empty() { kind.to_string() } else { format!("{kind} · {shown}") },
            input: Some(InputKind::for_field(f, lang)),
            current: value.cloned().filter(|v| !v.is_null()),
            color: None,
        });
    }
    // Statusübergänge
    let targets = allowed_status_targets(cfg, ctx);
    let from_name = ctx.status_id.map(|s| cfg.status_name(s, lang));
    for to in &targets {
        let Some(st) = cfg.statuses.iter().find(|s| s.id == *to) else { continue };
        let label = st.display(lang);
        let detail = match &from_name {
            Some(f) => format!("{}: {f} → {label}", lang.t("Status", "Status")),
            None => format!("{}: {label}", lang.t("Status setzen", "Set status")),
        };
        out.push(MenuEntry {
            kind: EntryKind::Transition { to_status_id: *to },
            aliases: aliases(st.key.as_deref(), st.name.as_deref(), lang, &label),
            label,
            detail,
            input: None,
            current: None,
            color: Some(st.color.clone()),
        });
    }
    // Quick-Actions (ungültige fehlen; „Status ändern“ nur, wenn der Übergang jetzt erlaubt ist)
    for a in cfg.actions.iter().filter(|a| !a.invalid) {
        if let ActionParams::ChangeStatus(c) = &a.params {
            if !targets.contains(&c.to_status_id) {
                continue;
            }
        }
        let label = a.display(lang);
        out.push(MenuEntry {
            kind: EntryKind::Action { action_id: a.id },
            aliases: aliases(a.key.as_deref(), a.name.as_deref(), lang, &label),
            label,
            detail: action_detail(cfg, a, lang),
            input: None,
            current: None,
            color: None,
        });
    }
    out
}

/// Filtert und sortiert die Einträge nach der Eingabe: beste Punktzahl zuerst, bei Gleichstand die ursprüngliche Reihenfolge.
/// Eine leere Eingabe liefert alle Einträge mit Punktzahl 0 in ursprünglicher Reihenfolge.
pub fn filter_entries(entries: &[MenuEntry], query: &str) -> Vec<(MenuEntry, u32)> {
    let mut hits: Vec<(usize, u32)> = entries.iter().enumerate().filter_map(|(i, e)| matcher::best_score(query, e.search_terms()).map(|s| (i, s))).collect();
    // `sort_by` ist stabil: gleiche Punktzahl behält die Reihenfolge.
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    hits.into_iter().map(|(i, s)| (entries[i].clone(), s)).collect()
}

/// Statusnamen für die Anzeige „Status“ ohne MenuConfig (Hilfe für die Oberfläche).
pub fn load_statuses(conn: &rusqlite::Connection, project_id: i64) -> QRes<Vec<Status>> {
    Ok(list_statuses(conn, project_id)?)
}

#[cfg(test)]
mod tests {
    use super::super::actions::fixture::*;
    use super::super::actions::{create_quick_action, NewQuickAction};
    use super::super::params::{ChangeStatusParams, CreateTicketParams, DefaultValue, FieldRef, StartSessionParams};
    use super::*;
    use crate::planning::fields::{create_field, FieldOptions, NewField, OptionValue};
    use crate::planning::workflow::{replace_transitions, Condition, ConditionOp, SetValue, TransitionDef};
    use serde_json::json;

    fn cfg(f: &Fx, ty: i64) -> MenuConfig {
        MenuConfig::load(&f.c, ty).unwrap()
    }

    fn labels(es: &[MenuEntry]) -> Vec<String> {
        es.iter().map(|e| e.label.clone()).collect()
    }

    fn ctx(status: Option<i64>) -> TicketContext {
        TicketContext { title: "Login page".into(), status_id: status, ..Default::default() }
    }

    fn add(f: &Fx, ty: i64, name: &str, p: ActionParams) -> i64 {
        create_quick_action(&f.c, &NewQuickAction::new(ty, name, p)).unwrap()
    }

    #[test]
    fn entries_of_a_userstory_in_order() {
        let f = fx();
        add(&f, f.story, "Create Dev task", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: None, defaults: vec![] }));
        add(&f, f.story, "Work in Claude", ActionParams::StartSession(StartSessionParams { claude: true, ..Default::default() }));
        let es = menu_entries(&cfg(&f, f.story), &ctx(Some(f.todo)), Lang::En);
        assert_eq!(
            labels(&es),
            vec!["Title", "Description", "Priority", "Epic", "Release version", "Story points", "In Progress", "Done", "Create Dev task", "Work in Claude"]
        );
        assert!(matches!(es[0].kind, EntryKind::Title));
        assert!(matches!(es[1].kind, EntryKind::Field { kind: FieldKind::Rich, .. }));
        assert!(matches!(es[6].kind, EntryKind::Transition { to_status_id } if to_status_id == f.progress));
        assert!(matches!(es[8].kind, EntryKind::Action { .. }));
    }

    #[test]
    fn entries_are_translated_and_carry_the_other_language_as_alias() {
        let f = fx();
        let c = cfg(&f, f.story);
        let de = menu_entries(&c, &ctx(Some(f.todo)), Lang::De);
        assert_eq!(labels(&de)[..5], ["Titel", "Beschreibung", "Priorität", "Epic", "Release-Version"].map(String::from));
        let prio = &de[2];
        assert_eq!(prio.aliases, vec!["Priority".to_string()]);
        // englisches Wort findet auch im deutschen Menü
        let hits = filter_entries(&de, "priority");
        assert_eq!(hits[0].0.label, "Priorität");
        let statuses: Vec<&str> = de.iter().filter(|e| matches!(e.kind, EntryKind::Transition { .. })).map(|e| e.label.as_str()).collect();
        assert_eq!(statuses, vec!["In Arbeit", "Erledigt"]);
    }

    #[test]
    fn field_entries_show_kind_current_value_and_input_form() {
        let f = fx();
        let prio = f.field(f.story, "priority");
        let sp = f.field(f.story, "story_points");
        let desc = f.field(f.story, "description");
        let mut cx = ctx(Some(f.todo));
        cx.values.insert(prio.id, json!(2));
        cx.values.insert(sp.id, json!(5));
        cx.values.insert(desc.id, json!("First line\nSecond line"));
        let es = menu_entries(&cfg(&f, f.story), &cx, Lang::En);
        let by = |l: &str| es.iter().find(|e| e.label == l).unwrap();
        assert_eq!(by("Priority").detail, "Dropdown · High");
        assert_eq!(by("Priority").current, Some(json!(2)));
        assert!(matches!(by("Priority").input, Some(InputKind::Dropdown { ref values }) if values.len() == 5));
        assert_eq!(by("Story points").detail, "Number · 5");
        assert!(matches!(by("Story points").input, Some(InputKind::Number { min: Some(m), decimals: Some(0), .. }) if m == 0.0));
        assert_eq!(by("Description").detail, "Rich text · First line");
        assert!(matches!(by("Description").input, Some(InputKind::RichTextEditor { rows: Some(8) })));
        // leer: nur der Feldtyp
        assert_eq!(by("Release version").detail, "Version");
        assert_eq!(by("Release version").current, None);
        assert_eq!(by("Release version").input, Some(InputKind::VersionPick));
        assert_eq!(by("Epic").input, Some(InputKind::TicketSearch { target_types: vec![f.epic] }));
        assert_eq!(by("Title").detail, "Text · Login page");
        // deutsch
        let de = menu_entries(&cfg(&f, f.story), &cx, Lang::De);
        assert_eq!(de.iter().find(|e| e.label == "Priorität").unwrap().detail, "Auswahlliste · Hoch");
        assert_eq!(de.iter().find(|e| e.label == "Beschreibung").unwrap().detail, "Formatierter Text · First line");
    }

    #[test]
    fn reference_fields_use_the_given_display_labels() {
        let f = fx();
        let epic = f.field(f.story, "epic");
        let mut cx = ctx(Some(f.todo));
        cx.values.insert(epic.id, json!(7));
        let without = menu_entries(&cfg(&f, f.story), &cx, Lang::En);
        assert_eq!(without.iter().find(|e| e.label == "Epic").unwrap().detail, "Ticket link");
        assert_eq!(without.iter().find(|e| e.label == "Epic").unwrap().current, Some(json!(7)));
        cx.labels.insert(epic.id, "PAN-7 Big plan".into());
        let with = menu_entries(&cfg(&f, f.story), &cx, Lang::En);
        assert_eq!(with.iter().find(|e| e.label == "Epic").unwrap().detail, "Ticket link · PAN-7 Big plan");
    }

    #[test]
    fn value_formatting_per_kind() {
        let mk = |kind: FieldKind, o: FieldOptions| Field { id: 1, type_id: 1, key: None, name: Some("F".into()), kind, options: o, required: false, default_value: None, help: String::new(), section: String::new(), position: 0, show_on_card: false, builtin: false };
        let o = FieldOptions { values: vec![OptionValue { id: 1, key: None, name: Some("A".into()), color: None }, OptionValue { id: 2, key: None, name: Some("B".into()), color: None }], ..Default::default() };
        let fv = |f: &Field, v: Value, l: Lang| format_value(f, Some(&v), None, l);
        assert_eq!(fv(&mk(FieldKind::Checkbox, o.clone()), json!(true), Lang::De), "Ja");
        assert_eq!(fv(&mk(FieldKind::Checkbox, o.clone()), json!(false), Lang::En), "No");
        assert_eq!(fv(&mk(FieldKind::Multi, o.clone()), json!([2, 1, 9]), Lang::En), "B, A");
        assert_eq!(fv(&mk(FieldKind::Labels, o.clone()), json!(["x", "y"]), Lang::En), "x, y");
        assert_eq!(fv(&mk(FieldKind::Date, o.clone()), json!("2026-11-14"), Lang::De), "14.11.2026");
        assert_eq!(fv(&mk(FieldKind::Date, o.clone()), json!("2026-11-14"), Lang::En), "2026-11-14");
        assert_eq!(fv(&mk(FieldKind::Datetime, o.clone()), json!("2026-11-14T09:30"), Lang::De), "14.11.2026 09:30");
        assert_eq!(fv(&mk(FieldKind::Datetime, o.clone()), json!("2026-11-14T09:30"), Lang::En), "2026-11-14 09:30");
        let n = FieldOptions { decimals: Some(1), unit: Some("h".into()), ..Default::default() };
        assert_eq!(fv(&mk(FieldKind::Number, n), json!(2.5), Lang::En), "2.5 h");
        assert_eq!(fv(&mk(FieldKind::Number, FieldOptions::default()), json!(3), Lang::En), "3");
        assert_eq!(fv(&mk(FieldKind::Text, o.clone()), json!("x".repeat(60)), Lang::En).chars().count(), 41);
        assert_eq!(format_value(&mk(FieldKind::Text, o.clone()), None, None, Lang::En), "");
        assert_eq!(format_value(&mk(FieldKind::Text, o), Some(&Value::Null), None, Lang::En), "");
    }

    #[test]
    fn only_allowed_transitions_are_offered() {
        let f = fx();
        // Standard-Workflow: To Do → In Progress/Done; In Progress → To Do/Done; Done → In Progress
        let c = cfg(&f, f.story);
        let names = |st: i64| {
            menu_entries(&c, &ctx(Some(st)), Lang::En).into_iter().filter(|e| matches!(e.kind, EntryKind::Transition { .. })).map(|e| e.label).collect::<Vec<_>>()
        };
        assert_eq!(names(f.todo), vec!["In Progress", "Done"]);
        assert_eq!(names(f.progress), vec!["To Do", "Done"]);
        assert_eq!(names(f.done), vec!["In Progress"]);
        // eigener Übergangsplan: nur To Do → Done
        replace_transitions(&f.c, f.story, &[TransitionDef { from_status_id: f.todo, to_status_id: f.done, ..Default::default() }]).unwrap();
        let c = cfg(&f, f.story);
        assert_eq!(allowed_status_targets(&c, &ctx(Some(f.todo))), vec![f.done]);
        assert!(allowed_status_targets(&c, &ctx(Some(f.done))).is_empty());
    }

    #[test]
    fn without_transitions_everything_else_is_allowed_and_without_status_everything() {
        let f = fx();
        replace_transitions(&f.c, f.story, &[]).unwrap();
        let c = cfg(&f, f.story);
        assert_eq!(allowed_status_targets(&c, &ctx(Some(f.progress))), vec![f.todo, f.done]);
        assert_eq!(allowed_status_targets(&c, &ctx(None)), vec![f.todo, f.progress, f.done]);
        let es = menu_entries(&c, &ctx(None), Lang::En);
        let e = es.iter().find(|e| e.label == "To Do").unwrap();
        assert_eq!(e.detail, "Set status: To Do");
        assert_eq!(e.color.as_deref(), Some("#97A3B0"));
    }

    #[test]
    fn transition_conditions_hide_blocked_transitions() {
        let f = fx();
        let sp = f.field(f.story, "story_points");
        // In Progress → Done nur, wenn Story points gesetzt sind (Bedingung: IsNotEmpty erlaubt)
        let defs = vec![
            TransitionDef { from_status_id: f.todo, to_status_id: f.progress, ..Default::default() },
            TransitionDef { from_status_id: f.progress, to_status_id: f.done, conditions: vec![Condition { field_id: sp.id, op: ConditionOp::IsNotEmpty, value: Value::Null, allow: true }], ..Default::default() },
        ];
        replace_transitions(&f.c, f.story, &defs).unwrap();
        let c = cfg(&f, f.story);
        let mut cx = ctx(Some(f.progress));
        assert!(allowed_status_targets(&c, &cx).is_empty(), "Story points fehlen");
        cx.values.insert(sp.id, json!(3));
        assert_eq!(allowed_status_targets(&c, &cx), vec![f.done]);
        // Sperr-Bedingung
        let defs = vec![TransitionDef { from_status_id: f.todo, to_status_id: f.done, conditions: vec![Condition { field_id: sp.id, op: ConditionOp::Eq, value: json!(13), allow: false }], ..Default::default() }];
        replace_transitions(&f.c, f.story, &defs).unwrap();
        let c = cfg(&f, f.story);
        let mut cx = ctx(Some(f.todo));
        cx.values.insert(sp.id, json!(13));
        assert!(allowed_status_targets(&c, &cx).is_empty());
        cx.values.insert(sp.id, json!(8));
        assert_eq!(allowed_status_targets(&c, &cx), vec![f.done]);
    }

    #[test]
    fn types_without_workflow_have_no_transitions() {
        let f = fx();
        let t = crate::planning::types::create_type(&f.c, &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Note".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let mut nf = NewField::new(t, "Remark", FieldKind::Text);
        nf.options.max_length = Some(10);
        create_field(&f.c, &nf).unwrap();
        let es = menu_entries(&cfg(&f, t), &ctx(None), Lang::En);
        assert_eq!(labels(&es), vec!["Title", "Remark"]);
    }

    #[test]
    fn change_status_actions_follow_the_allowed_transitions_and_invalid_ones_are_hidden() {
        let f = fx();
        let sp = f.field(f.story, "story_points");
        let start = add(&f, f.story, "Start work", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.progress, set_values: vec![] }));
        add(&f, f.story, "Finish", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.done, set_values: vec![SetValue { field_id: sp.id, value: json!(1) }] }));
        let sess = add(&f, f.story, "Session", ActionParams::StartSession(StartSessionParams::default()));
        let c = cfg(&f, f.story);
        let act = |st: i64| menu_entries(&c, &ctx(Some(st)), Lang::En).into_iter().filter(|e| matches!(e.kind, EntryKind::Action { .. })).map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(act(f.todo), vec!["Start work", "Finish", "Session"]);
        assert_eq!(act(f.progress), vec!["Finish", "Session"], "„Start work“ zielt auf den aktuellen Status");
        assert_eq!(act(f.done), vec!["Start work", "Session"]);
        f.c.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [start]).unwrap();
        f.c.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [sess]).unwrap();
        let c = cfg(&f, f.story);
        let act2 = menu_entries(&c, &ctx(Some(f.todo)), Lang::En).into_iter().filter(|e| matches!(e.kind, EntryKind::Action { .. })).map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(act2, vec!["Finish"]);
    }

    #[test]
    fn action_details_describe_what_they_do() {
        let f = fx();
        add(&f, f.story, "Create Dev task", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Title, value: json!("x") }] }));
        add(&f, f.story, "Done", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.done, set_values: vec![] }));
        add(&f, f.story, "Shell", ActionParams::StartSession(StartSessionParams::default()));
        add(&f, f.story, "Claude", ActionParams::StartSession(StartSessionParams { claude: true, ..Default::default() }));
        let c = cfg(&f, f.story);
        let en: Vec<String> = menu_entries(&c, &ctx(Some(f.todo)), Lang::En).into_iter().filter(|e| matches!(e.kind, EntryKind::Action { .. })).map(|e| e.detail).collect();
        assert_eq!(en, vec!["Create ticket: Dev task", "Set status: Done", "Start session", "Start Claude session"]);
        let de: Vec<String> = menu_entries(&c, &ctx(Some(f.todo)), Lang::De).into_iter().filter(|e| matches!(e.kind, EntryKind::Action { .. })).map(|e| e.detail).collect();
        assert_eq!(de, vec!["Ticket anlegen: Dev-Aufgabe", "Status setzen: Erledigt", "Sitzung starten", "Claude-Sitzung starten"]);
    }

    #[test]
    fn actions_of_other_types_are_not_in_the_menu() {
        let f = fx();
        add(&f, f.epic, "Only epics", ActionParams::StartSession(StartSessionParams::default()));
        let es = menu_entries(&cfg(&f, f.story), &ctx(Some(f.todo)), Lang::En);
        assert!(!labels(&es).contains(&"Only epics".to_string()));
        let es = menu_entries(&cfg(&f, f.epic), &ctx(Some(f.todo)), Lang::En);
        assert!(labels(&es).contains(&"Only epics".to_string()));
    }

    #[test]
    fn entries_for_every_default_type() {
        let f = fx();
        for (ty, want) in [
            (f.epic, vec!["Title", "Description", "Priority"]),
            (f.dev, vec!["Title", "Description", "Priority", "Story points"]),
            (f.sub, vec!["Title", "Description", "Priority"]),
            (f.defect, vec!["Title", "Description", "Priority", "Steps to reproduce"]),
        ] {
            let es = menu_entries(&cfg(&f, ty), &ctx(Some(f.done)), Lang::En);
            let fields: Vec<String> = es.iter().filter(|e| matches!(e.kind, EntryKind::Title | EntryKind::Field { .. })).map(|e| e.label.clone()).collect();
            assert_eq!(fields, want);
            assert_eq!(es.iter().filter(|e| matches!(e.kind, EntryKind::Transition { .. })).count(), 1, "aus Done nur nach In Progress");
        }
    }

    #[test]
    fn group_labels_are_bilingual() {
        for k in [EntryKind::Title, EntryKind::Transition { to_status_id: 1 }, EntryKind::Action { action_id: 1 }, EntryKind::Field { field_id: 1, kind: FieldKind::Text }] {
            assert!(!k.group_label(Lang::De).is_empty() && !k.group_label(Lang::En).is_empty());
        }
        assert_ne!(EntryKind::Title.group_label(Lang::De), EntryKind::Title.group_label(Lang::En));
        assert_eq!(EntryKind::Transition { to_status_id: 1 }.group_label(Lang::De), "Status");
        for k in FieldKind::ALL {
            assert!(!kind_label(k, Lang::De).is_empty() && !kind_label(k, Lang::En).is_empty());
        }
        let labels_en: std::collections::HashSet<_> = FieldKind::ALL.iter().map(|k| kind_label(*k, Lang::En)).collect();
        assert_eq!(labels_en.len(), 13, "jeder Feldtyp hat eine eigene Bezeichnung");
    }

    fn entry(label: &str, aliases: &[&str]) -> MenuEntry {
        MenuEntry { kind: EntryKind::Title, label: label.into(), aliases: aliases.iter().map(|s| s.to_string()).collect(), detail: String::new(), input: None, current: None, color: None }
    }

    fn sample() -> Vec<MenuEntry> {
        ["Description", "Priority", "Release version", "Story points", "Steps to reproduce", "Status", "Done", "Über uns"].iter().map(|l| entry(l, &[])).collect()
    }

    fn order(es: &[(MenuEntry, u32)]) -> Vec<&str> {
        es.iter().map(|(e, _)| e.label.as_str()).collect()
    }

    #[test]
    fn empty_query_keeps_everything_in_order() {
        let r = filter_entries(&sample(), "");
        assert_eq!(r.len(), 8);
        assert!(r.iter().all(|(_, s)| *s == 0));
        assert_eq!(order(&r), sample().iter().map(|e| e.label.as_str()).collect::<Vec<_>>());
        assert_eq!(order(&filter_entries(&sample(), "   ")).len(), 8);
    }

    #[test]
    fn initials_find_release_version() {
        let r = filter_entries(&sample(), "rv");
        assert_eq!(order(&r), vec!["Release version"]);
        assert_eq!(order(&filter_entries(&sample(), "RV"))[0], "Release version");
        assert_eq!(order(&filter_entries(&sample(), "sp"))[0], "Story points");
        assert_eq!(order(&filter_entries(&sample(), "str"))[0], "Steps to reproduce");
    }

    #[test]
    fn prefix_beats_word_start_beats_substring_in_the_list() {
        let es: Vec<MenuEntry> = ["Fix version", "Version", "Conversion", "Revert"].iter().map(|l| entry(l, &[])).collect();
        assert_eq!(order(&filter_entries(&es, "ver")), vec!["Version", "Fix version", "Revert", "Conversion"]);
        // Exakter Treffer zuerst
        let es: Vec<MenuEntry> = ["Status update", "Status", "Statue"].iter().map(|l| entry(l, &[])).collect();
        assert_eq!(order(&filter_entries(&es, "status"))[0], "Status");
    }

    #[test]
    fn umlauts_are_found_with_plain_letters() {
        let es: Vec<MenuEntry> = ["Priorität", "Schärfung", "Größe", "Übersicht", "Meilenstein"].iter().map(|l| entry(l, &[])).collect();
        assert_eq!(order(&filter_entries(&es, "prioritat")), vec!["Priorität"]);
        assert_eq!(order(&filter_entries(&es, "prioritaet")), vec!["Priorität"]);
        assert_eq!(order(&filter_entries(&es, "scharf")), vec!["Schärfung"]);
        assert_eq!(order(&filter_entries(&es, "grosse")), vec!["Größe"]);
        assert_eq!(order(&filter_entries(&es, "ueber")), vec!["Übersicht"]);
        assert_eq!(order(&filter_entries(&es, "UBER")), vec!["Übersicht"]);
        assert_eq!(order(&filter_entries(&es, "meil")), vec!["Meilenstein"]);
    }

    #[test]
    fn partial_words_in_any_order() {
        let r = filter_entries(&sample(), "ver rel");
        assert_eq!(order(&r), vec!["Release version"]);
        let r = filter_entries(&sample(), "points sto");
        assert_eq!(order(&r), vec!["Story points"]);
    }

    #[test]
    fn ties_keep_the_original_order() {
        let es: Vec<MenuEntry> = ["Beta one", "Alpha one", "Gamma one"].iter().map(|l| entry(l, &[])).collect();
        let r = filter_entries(&es, "one");
        assert_eq!(order(&r), vec!["Beta one", "Alpha one", "Gamma one"]);
        // zweimal derselbe Aufruf = derselbe Ablauf
        assert_eq!(order(&r), order(&filter_entries(&es, "one")));
        // gleiche Bezeichnung: ursprüngliche Reihenfolge
        let same = vec![entry("Done", &[]), entry("Done", &[]), entry("Done", &[])];
        let r = filter_entries(&same, "done");
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn aliases_count_and_best_alias_wins() {
        let es = vec![entry("Priorität", &["Priority"]), entry("Prior notice", &[])];
        let r = filter_entries(&es, "priority");
        assert_eq!(order(&r), vec!["Priorität"]);
        let r = filter_entries(&es, "prior");
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn no_match_gives_an_empty_list() {
        assert!(filter_entries(&sample(), "zzzz").is_empty());
        assert!(filter_entries(&[], "x").is_empty());
        assert!(filter_entries(&sample(), "{{x}}").is_empty());
    }

    #[test]
    fn filter_over_real_menu_entries() {
        let f = fx();
        add(&f, f.story, "Create Dev task", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: None, defaults: vec![] }));
        let es = menu_entries(&cfg(&f, f.story), &ctx(Some(f.todo)), Lang::En);
        assert_eq!(filter_entries(&es, "rv")[0].0.label, "Release version");
        assert_eq!(filter_entries(&es, "dev")[0].0.label, "Create Dev task");
        assert_eq!(filter_entries(&es, "done")[0].0.label, "Done");
        let r = filter_entries(&es, "p");
        assert_eq!(r[0].0.label, "Priority", "{:?}", order(&r));
        assert_eq!(filter_entries(&es, "tit")[0].1 / 100, 9);
    }

    #[test]
    fn load_statuses_helper_lists_the_pool() {
        let f = fx();
        assert_eq!(load_statuses(&f.c, f.project).unwrap().len(), 3);
        assert!(MenuConfig::load(&f.c, 99_999).is_err());
    }
}
