//! Schnellmenü (Taste `.`): reine Logik ohne Oberfläche und ohne Datenbank.
//!
//! * [`Item`]/[`ItemKey`]: eine Zeile des Menüs. Sie entsteht aus den Einträgen der Datenschicht (`quick::menu`: Titel, Felder,
//!   erlaubte Statusübergänge, Schnellaktionen) und zwei eingebauten Einträgen (Kommentar, Zeit erfassen).
//!   Nicht anwendbare Einträge bleiben in der Liste, sind aber ausgegraut und nennen den Grund ([`Item::disabled`]).
//! * [`items_for`] baut die Liste für ein Ticket, [`merge_multi`] für mehrere Tickets (nur gemeinsam Mögliches ist aktiv).
//! * [`layout`] sortiert nach der Eingabe (tolerante Suche der Datenschicht), „zuletzt benutzt“ steht oben, dazu Überschriften
//!   und die Zahl-Kürzel 1–9 ([`numbered`], [`digit_of`]).
//! * [`FormState`]/[`validate`]: die kleinen Eingabeformulare im Menü und ihre Prüfung (Fehler zweisprachig).
//! * [`summary_text`]: Zusammenfassung eines Mehrfach-Laufs („3 erfolgreich, 1 fehlgeschlagen“).

use crate::calc::duration::parse_duration;
use crate::planning::quick::input::{parse_input_at, InputError, InputKind};
use crate::planning::quick::matcher;
use crate::planning::quick::menu::{allowed_status_targets, menu_entries, EntryKind, MenuConfig, MenuEntry, TicketContext};
use crate::planning::quick::params::{ActionParams, FieldRef};
use crate::planning::quick::{actions::default_of, QuickError};
use crate::planning::ticket_error::TicketError;
use crate::planning::time_entries::StartInput;
use crate::state::Lang;
use chrono::NaiveDateTime;
use serde_json::{json, Value};

/// Höchstzahl der „zuletzt benutzt“-Einträge je Projekt.
pub const MAX_RECENT: usize = 8;
/// So viele Zeilen bekommen ein Zahl-Kürzel.
pub const MAX_NUMBERED: usize = 9;

/// Kennung einer Menüzeile; bleibt über Sitzungen stabil (für „zuletzt benutzt“).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemKey {
    Title,
    Field(i64),
    /// Statuswechsel (Zielstatus).
    Transition(i64),
    Action(i64),
    Comment,
    LogTime,
}

impl ItemKey {
    pub fn to_text(&self) -> String {
        match self {
            ItemKey::Title => "title".into(),
            ItemKey::Field(i) => format!("field:{i}"),
            ItemKey::Transition(i) => format!("status:{i}"),
            ItemKey::Action(i) => format!("action:{i}"),
            ItemKey::Comment => "comment".into(),
            ItemKey::LogTime => "time".into(),
        }
    }

    pub fn parse(s: &str) -> Option<ItemKey> {
        let num = |p: &str| s.strip_prefix(p).and_then(|n| n.parse::<i64>().ok());
        match s {
            "title" => Some(ItemKey::Title),
            "comment" => Some(ItemKey::Comment),
            "time" => Some(ItemKey::LogTime),
            _ => num("field:").map(ItemKey::Field).or_else(|| num("status:").map(ItemKey::Transition)).or_else(|| num("action:").map(ItemKey::Action)),
        }
    }
}

/// Gespeicherte Liste („zuletzt benutzt“) als Text, mit Komma getrennt.
pub fn recent_to_text(recent: &[ItemKey]) -> String {
    recent.iter().map(ItemKey::to_text).collect::<Vec<_>>().join(",")
}

pub fn recent_from_text(s: &str) -> Vec<ItemKey> {
    let mut out: Vec<ItemKey> = Vec::new();
    for k in s.split(',').filter_map(|p| ItemKey::parse(p.trim())) {
        if !out.contains(&k) && out.len() < MAX_RECENT {
            out.push(k);
        }
    }
    out
}

/// Eintrag nach vorn schieben (neueste zuerst, ohne Doppelte, begrenzt).
pub fn push_recent(recent: &mut Vec<ItemKey>, key: ItemKey) {
    recent.retain(|k| *k != key);
    recent.insert(0, key);
    recent.truncate(MAX_RECENT);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Fields,
    Status,
    Actions,
    /// Eingebaute Erfassung (Kommentar, Zeit).
    Record,
}

impl Group {
    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Group::Fields => lang.t("Felder", "Fields"),
            Group::Status => lang.t("Status", "Status"),
            Group::Actions => lang.t("Aktionen", "Actions"),
            Group::Record => lang.t("Erfassen", "Record"),
        }
    }
}

/// Eine Zeile des Menüs.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: ItemKey,
    pub group: Group,
    pub label: String,
    /// Weitere Namen, die ebenfalls treffen (Name in der anderen Sprache).
    pub aliases: Vec<String>,
    pub detail: String,
    /// Eingabeform (Titel und Felder).
    pub input: Option<InputKind>,
    /// Aktueller Wert des Feldes (roh).
    pub current: Option<Value>,
    pub color: Option<String>,
    /// `Some(Grund)`: nicht anwendbar, ausgegraut.
    pub disabled: Option<String>,
    /// „Ticket anlegen“ ohne Titelvorgabe: das Menü fragt den Titel.
    pub ask_title: bool,
}

impl Item {
    pub fn search_terms(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.label.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    pub fn enabled(&self) -> bool {
        self.disabled.is_none()
    }

    /// Öffnet die Zeile ein Formular (sonst wird sie sofort ausgeführt)?
    pub fn needs_form(&self) -> bool {
        matches!(self.key, ItemKey::Title | ItemKey::Field(_) | ItemKey::Comment | ItemKey::LogTime) || self.ask_title
    }

    fn from_entry(e: MenuEntry) -> Item {
        let (key, group) = match &e.kind {
            EntryKind::Title => (ItemKey::Title, Group::Fields),
            EntryKind::Field { field_id, .. } => (ItemKey::Field(*field_id), Group::Fields),
            EntryKind::Transition { to_status_id } => (ItemKey::Transition(*to_status_id), Group::Status),
            EntryKind::Action { action_id } => (ItemKey::Action(*action_id), Group::Actions),
        };
        Item { key, group, label: e.label, aliases: e.aliases, detail: e.detail, input: e.input, current: e.current, color: e.color, disabled: None, ask_title: false }
    }
}

fn other(lang: Lang) -> Lang {
    match lang {
        Lang::De => Lang::En,
        Lang::En => Lang::De,
    }
}

/// Die eingebauten Einträge „Kommentar hinzufügen“ und „Zeit erfassen“.
fn builtin_items(lang: Lang) -> Vec<Item> {
    let mk = |key: ItemKey, de: &'static str, en: &'static str, alias: &[&str], detail: (&'static str, &'static str)| {
        let o = other(lang);
        let mut aliases = vec![o.t(de, en).to_string()];
        aliases.extend(alias.iter().map(|s| s.to_string()));
        Item { key, group: Group::Record, label: lang.t(de, en).to_string(), aliases, detail: lang.t(detail.0, detail.1).to_string(), input: None, current: None, color: None, disabled: None, ask_title: false }
    };
    vec![
        mk(ItemKey::Comment, "Kommentar hinzufügen", "Add comment", &["comment", "kommentar", "note"], ("Kommentar am Ticket", "Comment on the ticket")),
        mk(ItemKey::LogTime, "Zeit erfassen", "Log time", &["time", "zeit", "worklog", "buchen"], ("Arbeitszeit buchen", "Book working time")),
    ]
}

/// Alle Zeilen für ein Ticket: aktive Einträge der Datenschicht, die eingebauten, dann die ausgegrauten mit Grund.
pub fn items_for(cfg: &MenuConfig, ctx: &TicketContext, lang: Lang) -> Vec<Item> {
    let mut items: Vec<Item> = menu_entries(cfg, ctx, lang).into_iter().map(Item::from_entry).collect();
    // Terminal-Felder werden über die Terminal-Verknüpfung gesetzt, nicht im Menü.
    for it in items.iter_mut() {
        if matches!(it.input, Some(InputKind::TerminalPick)) {
            it.disabled = Some(lang.t("Terminals werden über die Verknüpfung in der Terminal-Liste gesetzt.", "Terminals are linked from the terminal list.").to_string());
        }
        if let ItemKey::Action(id) = it.key {
            if let Some(a) = cfg.actions.iter().find(|a| a.id == id) {
                if let ActionParams::CreateTicket(c) = &a.params {
                    it.ask_title = default_of(c, FieldRef::Title).and_then(Value::as_str).is_none_or(|s| s.trim().is_empty());
                }
            }
        }
    }
    items.extend(builtin_items(lang));
    items.extend(disabled_items(cfg, ctx, &items, lang));
    items
}

fn status_label(cfg: &MenuConfig, id: i64, lang: Lang) -> String {
    cfg.statuses.iter().find(|s| s.id == id).map(|s| s.display(lang)).unwrap_or_default()
}

/// Ausgegraute Zeilen: gesperrte Statusübergänge und nicht ausführbare Aktionen, jeweils mit Grund.
fn disabled_items(cfg: &MenuConfig, ctx: &TicketContext, active: &[Item], lang: Lang) -> Vec<Item> {
    let mut out = Vec::new();
    let from = ctx.status_id.filter(|s| cfg.workflow_statuses.contains(s));
    let targets = allowed_status_targets(cfg, ctx);
    let from_name = from.map(|f| status_label(cfg, f, lang)).unwrap_or_default();
    if cfg.ticket_type.workflow {
        for to in &cfg.workflow_statuses {
            if Some(*to) == from || targets.contains(to) {
                continue;
            }
            let Some(st) = cfg.statuses.iter().find(|s| s.id == *to) else { continue };
            let has_rule = cfg.transitions.iter().any(|t| Some(t.def.from_status_id) == from && t.def.to_status_id == *to);
            let reason = if has_rule || cfg.transitions.is_empty() {
                lang.t("Eine Bedingung des Übergangs sperrt den Wechsel.", "A condition of the transition blocks the change.").to_string()
            } else {
                match lang {
                    Lang::De => format!("Aus {} ist dieser Wechsel nicht erlaubt.", lang.q(&from_name)),
                    Lang::En => format!("This change is not allowed from {}.", lang.q(&from_name)),
                }
            };
            let label = st.display(lang);
            out.push(Item {
                key: ItemKey::Transition(*to),
                group: Group::Status,
                aliases: Vec::new(),
                detail: format!("{}: {from_name} → {label}", lang.t("Status", "Status")),
                label,
                input: None,
                current: None,
                color: Some(st.color.clone()),
                disabled: Some(reason),
                ask_title: false,
            });
        }
    }
    for a in &cfg.actions {
        if active.iter().any(|i| i.key == ItemKey::Action(a.id)) {
            continue;
        }
        let reason = if a.invalid {
            QuickError::ActionInvalid.message(lang)
        } else if let ActionParams::ChangeStatus(c) = &a.params {
            if Some(c.to_status_id) == from {
                lang.t("Das Ticket hat diesen Status bereits.", "The ticket already has this status.").to_string()
            } else {
                match lang {
                    Lang::De => format!("Der Wechsel nach {} ist von hier aus nicht möglich.", lang.q(&status_label(cfg, c.to_status_id, lang))),
                    Lang::En => format!("Changing to {} is not possible from here.", lang.q(&status_label(cfg, c.to_status_id, lang))),
                }
            }
        } else {
            continue;
        };
        let label = a.display(lang);
        out.push(Item { key: ItemKey::Action(a.id), group: Group::Actions, aliases: Vec::new(), detail: a.kind().label(lang).to_string(), label, input: None, current: None, color: None, disabled: Some(reason), ask_title: false });
    }
    out
}

/// Zeilen für mehrere Tickets: Reihenfolge und Beschriftung vom ersten, aktiv nur, was bei **allen** aktiv ist.
pub fn merge_multi(lists: &[Vec<Item>], lang: Lang) -> Vec<Item> {
    let n = lists.len();
    let Some(first) = lists.first() else { return Vec::new() };
    if n == 1 {
        return first.clone();
    }
    let mut out = Vec::new();
    for it in first {
        let mut m = it.clone();
        let mut active = 0;
        let mut first_reason: Option<String> = it.disabled.clone();
        let mut same_current = true;
        for l in lists {
            match l.iter().find(|x| x.key == it.key) {
                Some(x) if x.enabled() => {
                    active += 1;
                    if x.current != it.current {
                        same_current = false;
                    }
                }
                Some(x) => first_reason = first_reason.or_else(|| x.disabled.clone()),
                None => {}
            }
        }
        if active < n {
            let why = match lang {
                Lang::De => format!("Nur bei {active} von {n} Tickets möglich."),
                Lang::En => format!("Only possible for {active} of {n} tickets."),
            };
            m.disabled = Some(match first_reason {
                Some(r) if active == 0 => r,
                _ => why,
            });
        }
        if !same_current {
            m.current = None;
        }
        // Der aktuelle Wert des ersten Tickets steht nicht für alle.
        if let Some(i) = m.detail.find(" · ") {
            m.detail.truncate(i);
        }
        out.push(m);
    }
    out
}

/// Eine Zeile der angezeigten Liste.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub item: usize,
    /// Überschrift über dieser Zeile (Gruppenwechsel).
    pub header: Option<String>,
}

/// Reihenfolge der Zeilen: aktive vor ausgegrauten, dann beste Punktzahl, dann „zuletzt benutzt“, dann ursprüngliche Reihenfolge.
/// Eine leere Eingabe zeigt die zuletzt benutzten zuerst (Überschrift „Zuletzt benutzt“), den Rest nach Gruppen.
pub fn layout(items: &[Item], query: &str, recent: &[ItemKey], lang: Lang) -> Vec<Row> {
    let empty = query.trim().is_empty();
    let mut hits: Vec<(usize, u32)> = items.iter().enumerate().filter_map(|(i, it)| matcher::best_score(query, it.search_terms()).map(|s| (i, s))).collect();
    let rpos = |i: usize| recent.iter().position(|k| *k == items[i].key).unwrap_or(usize::MAX);
    hits.sort_by(|a, b| {
        let (da, db) = (!items[a.0].enabled(), !items[b.0].enabled());
        da.cmp(&db).then(b.1.cmp(&a.1)).then(rpos(a.0).cmp(&rpos(b.0))).then(a.0.cmp(&b.0))
    });
    if !empty {
        return hits.into_iter().map(|(item, _)| Row { item, header: None }).collect();
    }
    // Leere Eingabe: Zuletzt benutzte (aktive) oben, danach nach Gruppen in Menü-Reihenfolge.
    let mut rows: Vec<Row> = Vec::new();
    let recent_idx: Vec<usize> = hits.iter().map(|h| h.0).filter(|i| items[*i].enabled() && rpos(*i) != usize::MAX).collect();
    for (n, i) in recent_idx.iter().enumerate() {
        rows.push(Row { item: *i, header: (n == 0).then(|| lang.t("Zuletzt benutzt", "Recently used").to_string()) });
    }
    let mut last: Option<Group> = None;
    let mut in_disabled = false;
    let mut rest: Vec<usize> = hits.iter().map(|h| h.0).filter(|i| !recent_idx.contains(i)).collect();
    rest.sort_by_key(|i| (!items[*i].enabled(), *i));
    for i in rest {
        let it = &items[i];
        let header = if !it.enabled() {
            if in_disabled {
                None
            } else {
                in_disabled = true;
                Some(lang.t("Nicht verfügbar", "Not available").to_string())
            }
        } else if last != Some(it.group) {
            last = Some(it.group);
            Some(it.group.label(lang).to_string())
        } else {
            None
        };
        rows.push(Row { item: i, header });
    }
    rows
}

/// Die Zeilen mit Zahl-Kürzel: die ersten neun **aktiven** Zeilen in Anzeigereihenfolge (Index in `items`).
pub fn numbered(rows: &[Row], items: &[Item]) -> Vec<usize> {
    rows.iter().map(|r| r.item).filter(|i| items[*i].enabled()).take(MAX_NUMBERED).collect()
}

/// Zahl-Kürzel aus einem getippten Zeichen: „1“ → 0 … „9“ → 8.
pub fn digit_of(text: &str) -> Option<usize> {
    let mut chars = text.chars();
    let (c, rest) = (chars.next()?, chars.next());
    if rest.is_some() {
        return None;
    }
    c.to_digit(10).filter(|d| (1..=9).contains(d)).map(|d| d as usize - 1)
}

// ------------------------------------------------------------------ Formular

/// Eingaben der kleinen Formulare im Menü.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormState {
    /// Text (Titel, Text-/Zahl-/Datumsfelder, Labels, Kommentar, Dauer) bzw. Suchtext bei Auswahllisten.
    pub text: String,
    /// Gewählte Kennungen (Auswahlliste, Mehrfachauswahl, Version, Meilenstein, Ticket).
    pub picked: Vec<i64>,
    /// Antwort der Checkbox (`None` = noch keine).
    pub flag: Option<bool>,
    /// Zeit erfassen: Beginn (leer = automatisch), Kommentar, Art der Arbeit.
    pub start: String,
    pub comment: String,
    pub work_kind: Option<i64>,
    pub error: Option<String>,
}

impl FormState {
    /// Vorbelegung mit dem aktuellen Wert des Eintrags.
    pub fn for_item(it: &Item) -> FormState {
        let mut f = FormState::default();
        let cur = it.current.as_ref().filter(|v| !v.is_null());
        match (&it.key, &it.input, cur) {
            (ItemKey::Title, _, Some(v)) => f.text = v.as_str().unwrap_or_default().to_string(),
            (_, Some(InputKind::Dropdown { .. } | InputKind::VersionPick | InputKind::MilestonePick | InputKind::TicketSearch { .. }), Some(v)) => f.picked = v.as_i64().into_iter().collect(),
            (_, Some(InputKind::MultiSelect { .. }), Some(v)) => f.picked = v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(),
            (_, Some(InputKind::Checkbox), Some(v)) => f.flag = v.as_bool(),
            (_, Some(InputKind::Labels), Some(v)) => f.text = v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
            (_, Some(InputKind::Number { .. }), Some(v)) => f.text = v.as_f64().map(|x| if x.fract() == 0.0 { format!("{}", x as i64) } else { x.to_string() }).unwrap_or_default(),
            (_, Some(InputKind::Date | InputKind::DateTime { .. } | InputKind::Text { .. } | InputKind::RichTextEditor { .. }), Some(v)) => f.text = v.as_str().unwrap_or_default().to_string(),
            _ => {}
        }
        f
    }
}

/// Geprüfte Eingabe, bereit zum Ausführen.
#[derive(Debug, Clone, PartialEq)]
pub enum Submit {
    Title(String),
    Field { field_id: i64, value: Value },
    Comment(String),
    LogTime { duration: String, start: Option<StartInput>, work_kind: Option<i64>, comment: String },
    /// Titel für „Ticket anlegen“ ohne Titelvorgabe.
    NewTitle(String),
}

fn title_checked(text: &str, lang: Lang) -> Result<String, String> {
    match parse_input_at(&InputKind::Text { max_length: Some(crate::planning::tickets::MAX_TITLE_CHARS as u32) }, text, chrono::DateTime::UNIX_EPOCH.naive_utc()) {
        Ok(Value::String(s)) => Ok(s),
        Ok(_) => Err(TicketError::TitleEmpty.message(lang)),
        Err(e) => Err(e.message(lang)),
    }
}

/// Prüft das Formular zu einer Zeile. `now` für Schlüsselwörter wie „heute“.
pub fn validate(it: &Item, f: &FormState, now: NaiveDateTime, lang: Lang) -> Result<Submit, String> {
    match &it.key {
        ItemKey::Title => title_checked(&f.text, lang).map(Submit::Title),
        ItemKey::Comment => {
            let body = f.text.trim();
            if body.is_empty() { Err(TicketError::CommentEmpty.message(lang)) } else { Ok(Submit::Comment(body.to_string())) }
        }
        ItemKey::LogTime => {
            let d = f.text.trim();
            if d.is_empty() {
                return Err(lang.t("Bitte eine Dauer eingeben, z. B. 1h 30m oder 2d.", "Please enter a duration, e.g. 1h 30m or 2d.").to_string());
            }
            match parse_duration(d) {
                Ok(0) => return Err(TicketError::DurationZero.message(lang)),
                Ok(_) => {}
                Err(e) => return Err(TicketError::DurationInvalid(e).message(lang)),
            }
            let start = if f.start.trim().is_empty() {
                None
            } else {
                Some(StartInput::parse(&f.start).ok_or_else(|| lang.t("Der Beginn ist ungültig (JJJJ-MM-TT oder JJJJ-MM-TT HH:MM).", "The start is not valid (YYYY-MM-DD or YYYY-MM-DD HH:MM).").to_string())?)
            };
            Ok(Submit::LogTime { duration: d.to_string(), start, work_kind: f.work_kind, comment: f.comment.trim().to_string() })
        }
        ItemKey::Field(id) => {
            let Some(kind) = &it.input else { return Err(InputError::PickOnly.message(lang)) };
            let one = || f.picked.first().map(|i| json!(i)).unwrap_or(Value::Null);
            let value = match kind {
                InputKind::Dropdown { .. } | InputKind::VersionPick | InputKind::MilestonePick | InputKind::TicketSearch { .. } => one(),
                InputKind::MultiSelect { .. } => {
                    if f.picked.is_empty() { Value::Null } else { json!(f.picked) }
                }
                InputKind::Checkbox => json!(f.flag.ok_or_else(|| InputError::Empty.message(lang))?),
                InputKind::TerminalPick => return Err(InputError::PickOnly.message(lang)),
                other => parse_input_at(other, &f.text, now).map_err(|e| e.message(lang))?,
            };
            Ok(Submit::Field { field_id: *id, value })
        }
        ItemKey::Action(_) if it.ask_title => title_checked(&f.text, lang).map(Submit::NewTitle),
        ItemKey::Action(_) | ItemKey::Transition(_) => Err(lang.t("Diese Zeile hat kein Formular.", "This row has no form.").to_string()),
    }
}

/// Feldart des Formulars (für Überschrift und Aufbau).
pub fn is_list_input(k: &InputKind) -> bool {
    matches!(k, InputKind::Dropdown { .. } | InputKind::MultiSelect { .. } | InputKind::VersionPick | InputKind::MilestonePick | InputKind::TicketSearch { .. })
}

/// Zusammenfassung eines Laufs: „Status gesetzt: 3 erfolgreich“ bzw. „… 3 erfolgreich, 1 fehlgeschlagen“.
pub fn summary_text(label: &str, ok: usize, failed: usize, lang: Lang) -> String {
    match (ok, failed, lang) {
        (_, 0, Lang::De) if ok == 1 => format!("{label}: erledigt."),
        (_, 0, Lang::En) if ok == 1 => format!("{label}: done."),
        (_, 0, Lang::De) => format!("{label}: {ok} erfolgreich."),
        (_, 0, Lang::En) => format!("{label}: {ok} succeeded."),
        (0, _, Lang::De) => format!("{label}: {failed} fehlgeschlagen."),
        (0, _, Lang::En) => format!("{label}: {failed} failed."),
        (_, _, Lang::De) => format!("{label}: {ok} erfolgreich, {failed} fehlgeschlagen."),
        (_, _, Lang::En) => format!("{label}: {ok} succeeded, {failed} failed."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::quick::actions::fixture::*;
    use crate::planning::quick::actions::{create_quick_action, NewQuickAction};
    use crate::planning::quick::params::{ChangeStatusParams, CreateTicketParams, DefaultValue, StartSessionParams};
    use crate::planning::workflow::{replace_transitions, TransitionDef};
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(10, 0, 0).unwrap()
    }

    fn ctx(status: Option<i64>) -> TicketContext {
        TicketContext { title: "Login page".into(), status_id: status, ..Default::default() }
    }

    fn add(f: &Fx, ty: i64, name: &str, p: ActionParams) -> i64 {
        create_quick_action(&f.c, &NewQuickAction::new(ty, name, p)).unwrap()
    }

    fn item(key: ItemKey, label: &str, group: Group) -> Item {
        Item { key, group, label: label.into(), aliases: vec![], detail: String::new(), input: None, current: None, color: None, disabled: None, ask_title: false }
    }

    fn labels(rows: &[Row], items: &[Item]) -> Vec<String> {
        rows.iter().map(|r| items[r.item].label.clone()).collect()
    }

    #[test]
    fn item_keys_roundtrip_and_recent_list_is_bounded() {
        for k in [ItemKey::Title, ItemKey::Field(7), ItemKey::Transition(3), ItemKey::Action(9), ItemKey::Comment, ItemKey::LogTime] {
            assert_eq!(ItemKey::parse(&k.to_text()), Some(k));
        }
        assert_eq!(ItemKey::parse("nope"), None);
        assert_eq!(ItemKey::parse("field:x"), None);
        let mut r = Vec::new();
        for i in 0..20 {
            push_recent(&mut r, ItemKey::Field(i));
        }
        assert_eq!(r.len(), MAX_RECENT);
        assert_eq!(r[0], ItemKey::Field(19));
        push_recent(&mut r, ItemKey::Field(15));
        assert_eq!(r[0], ItemKey::Field(15));
        assert_eq!(r.iter().filter(|k| **k == ItemKey::Field(15)).count(), 1);
        assert_eq!(recent_from_text(&recent_to_text(&r)), r);
        assert_eq!(recent_from_text("field:1,junk,field:1,status:2"), vec![ItemKey::Field(1), ItemKey::Transition(2)]);
    }

    #[test]
    fn digit_shortcuts() {
        assert_eq!(digit_of("1"), Some(0));
        assert_eq!(digit_of("9"), Some(8));
        assert_eq!(digit_of("0"), None);
        assert_eq!(digit_of("12"), None);
        assert_eq!(digit_of("a"), None);
        assert_eq!(digit_of(""), None);
    }

    #[test]
    fn story_menu_has_fields_status_actions_and_builtins() {
        let f = fx();
        add(&f, f.story, "Create Dev task", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: None, defaults: vec![] }));
        let cfg = MenuConfig::load(&f.c, f.story).unwrap();
        let items = items_for(&cfg, &ctx(Some(f.todo)), Lang::En);
        let rows = layout(&items, "", &[], Lang::En);
        let l = labels(&rows, &items);
        assert_eq!(l[0], "Title");
        assert!(l.contains(&"Priority".to_string()) && l.contains(&"In Progress".to_string()) && l.contains(&"Create Dev task".to_string()));
        assert!(l.contains(&"Add comment".to_string()) && l.contains(&"Log time".to_string()));
        // Überschriften je Gruppenwechsel
        let heads: Vec<&str> = rows.iter().filter_map(|r| r.header.as_deref()).collect();
        assert_eq!(heads, vec!["Fields", "Status", "Actions", "Record"]);
        // ohne Titelvorgabe fragt das Menü den Titel
        let a = items.iter().find(|i| i.label == "Create Dev task").unwrap();
        assert!(a.ask_title && a.needs_form());
    }

    #[test]
    fn create_action_with_title_default_runs_without_a_form() {
        let f = fx();
        add(&f, f.story, "With title", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.dev, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Title, value: json!("Task for {{source.key}}") }] }));
        let cfg = MenuConfig::load(&f.c, f.story).unwrap();
        let items = items_for(&cfg, &ctx(Some(f.todo)), Lang::En);
        let a = items.iter().find(|i| i.label == "With title").unwrap();
        assert!(!a.ask_title && !a.needs_form());
    }

    #[test]
    fn blocked_transitions_and_actions_are_greyed_out_with_a_reason() {
        let f = fx();
        replace_transitions(&f.c, f.story, &[TransitionDef { from_status_id: f.todo, to_status_id: f.progress, ..Default::default() }]).unwrap();
        add(&f, f.story, "Finish", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.done, set_values: vec![] }));
        add(&f, f.story, "Start", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.todo, set_values: vec![] }));
        let bad = add(&f, f.story, "Broken", ActionParams::StartSession(StartSessionParams::default()));
        f.c.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [bad]).unwrap();
        let cfg = MenuConfig::load(&f.c, f.story).unwrap();
        let items = items_for(&cfg, &ctx(Some(f.todo)), Lang::En);
        let by = |l: &str| items.iter().find(|i| i.label == l).unwrap();
        assert!(by("In Progress").enabled());
        assert!(by("Done").disabled.as_deref().unwrap().contains("not allowed from"), "{:?}", by("Done").disabled);
        assert!(by("Finish").disabled.is_some());
        assert!(by("Start").disabled.as_deref().unwrap().contains("already"));
        assert!(by("Broken").disabled.is_some());
        // ausgegraute stehen am Ende und unter „Nicht verfügbar“
        let rows = layout(&items, "", &[], Lang::En);
        let first_disabled = rows.iter().position(|r| !items[r.item].enabled()).unwrap();
        assert!(rows[first_disabled..].iter().all(|r| !items[r.item].enabled()));
        assert_eq!(rows[first_disabled].header.as_deref(), Some("Not available"));
        // deutscher Grund
        let de = items_for(&cfg, &ctx(Some(f.todo)), Lang::De);
        assert!(de.iter().find(|i| i.label == "Erledigt").unwrap().disabled.as_deref().unwrap().contains("nicht erlaubt"));
        // Ausgegraute werden nicht durchnummeriert
        let n = numbered(&rows, &items);
        assert!(n.iter().all(|i| items[*i].enabled()) && n.len() <= MAX_NUMBERED);
    }

    #[test]
    fn typing_filters_ranks_and_keeps_enabled_before_disabled() {
        let mut items = vec![item(ItemKey::Field(1), "Priority", Group::Fields), item(ItemKey::Field(2), "Story points", Group::Fields), item(ItemKey::Transition(5), "Done", Group::Status)];
        items.push(Item { disabled: Some("x".into()), ..item(ItemKey::Transition(6), "Done soon", Group::Status) });
        let r = layout(&items, "done", &[], Lang::En);
        assert_eq!(labels(&r, &items), vec!["Done", "Done soon"]);
        assert!(r.iter().all(|r| r.header.is_none()));
        assert_eq!(labels(&layout(&items, "sp", &[], Lang::En), &items)[0], "Story points");
        assert!(layout(&items, "zzz", &[], Lang::En).is_empty());
    }

    #[test]
    fn aliases_make_both_languages_findable() {
        let f = fx();
        let cfg = MenuConfig::load(&f.c, f.story).unwrap();
        let items = items_for(&cfg, &ctx(Some(f.todo)), Lang::De);
        let r = layout(&items, "comment", &[], Lang::De);
        assert_eq!(items[r[0].item].label, "Kommentar hinzufügen");
        let r = layout(&items, "log time", &[], Lang::De);
        assert_eq!(items[r[0].item].label, "Zeit erfassen");
        let r = layout(&items, "priority", &[], Lang::De);
        assert_eq!(items[r[0].item].label, "Priorität");
    }

    #[test]
    fn recently_used_rows_come_first_and_win_ties() {
        let items = vec![item(ItemKey::Field(1), "Alpha", Group::Fields), item(ItemKey::Field(2), "Beta", Group::Fields), item(ItemKey::Transition(3), "Gamma", Group::Status)];
        let recent = vec![ItemKey::Transition(3), ItemKey::Field(2)];
        let rows = layout(&items, "", &recent, Lang::En);
        assert_eq!(labels(&rows, &items), vec!["Gamma", "Beta", "Alpha"]);
        assert_eq!(rows[0].header.as_deref(), Some("Recently used"));
        assert_eq!(rows[1].header, None);
        assert_eq!(rows[2].header.as_deref(), Some("Fields"));
        // Gleichstand beim Tippen: das zuletzt benutzte zuerst
        let items = vec![item(ItemKey::Field(1), "Item one", Group::Fields), item(ItemKey::Field(2), "Item two", Group::Fields)];
        let rows = layout(&items, "item", &[ItemKey::Field(2)], Lang::En);
        assert_eq!(labels(&rows, &items), vec!["Item two", "Item one"]);
        // und die Zahl-Kürzel folgen der Anzeige
        assert_eq!(numbered(&rows, &items), vec![1, 0]);
    }

    #[test]
    fn numbering_stops_at_nine() {
        let items: Vec<Item> = (0..12).map(|i| item(ItemKey::Field(i), &format!("Row {i:02}"), Group::Fields)).collect();
        let rows = layout(&items, "", &[], Lang::En);
        assert_eq!(numbered(&rows, &items).len(), 9);
    }

    #[test]
    fn multi_ticket_menu_is_active_only_where_all_agree() {
        let f = fx();
        let cfg = MenuConfig::load(&f.c, f.story).unwrap();
        let a = items_for(&cfg, &ctx(Some(f.todo)), Lang::En);
        let b = items_for(&cfg, &ctx(Some(f.progress)), Lang::En);
        let m = merge_multi(&[a.clone(), b], Lang::En);
        let by = |l: &str| m.iter().find(|i| i.label == l).unwrap();
        // „Done“ ist aus beiden Status erlaubt, „In Progress“ nur für das erste Ticket (das zweite ist schon dort)
        assert!(by("Done").enabled());
        assert!(by("In Progress").disabled.as_deref().unwrap().contains("1 of 2"));
        assert!(by("Title").enabled());
        assert!(!by("Priority").detail.contains(" · "), "kein Wert eines einzelnen Tickets");
        assert_eq!(merge_multi(&[a.clone()], Lang::En), a);
        assert!(merge_multi(&[], Lang::En).is_empty());
    }

    fn field_item(kind: InputKind, current: Option<Value>) -> Item {
        Item { input: Some(kind), current, ..item(ItemKey::Field(4), "F", Group::Fields) }
    }

    #[test]
    fn form_validation_per_input_kind() {
        let n = now();
        let ok = |it: &Item, f: &FormState| validate(it, f, n, Lang::En).unwrap();
        let err = |it: &Item, f: &FormState| validate(it, f, n, Lang::En).unwrap_err();
        // Titel
        let t = item(ItemKey::Title, "Title", Group::Fields);
        assert_eq!(ok(&t, &FormState { text: "  New  ".into(), ..Default::default() }), Submit::Title("New".into()));
        assert!(err(&t, &FormState::default()).contains("empty"), "{}", err(&t, &FormState::default()));
        assert!(validate(&t, &FormState::default(), n, Lang::De).unwrap_err().contains("leer"));
        // Zahl
        let num = field_item(InputKind::Number { min: Some(0.0), max: Some(10.0), decimals: Some(0), unit: None }, None);
        assert_eq!(ok(&num, &FormState { text: "5".into(), ..Default::default() }), Submit::Field { field_id: 4, value: json!(5) });
        assert!(err(&num, &FormState { text: "50".into(), ..Default::default() }).contains("range"));
        assert!(err(&num, &FormState { text: "abc".into(), ..Default::default() }).contains("not a number"));
        // leer = Feld leeren
        assert_eq!(ok(&num, &FormState::default()), Submit::Field { field_id: 4, value: Value::Null });
        // Auswahl
        let dd = field_item(InputKind::Dropdown { values: vec![] }, None);
        assert_eq!(ok(&dd, &FormState { picked: vec![3], ..Default::default() }), Submit::Field { field_id: 4, value: json!(3) });
        assert_eq!(ok(&dd, &FormState::default()), Submit::Field { field_id: 4, value: Value::Null });
        let multi = field_item(InputKind::MultiSelect { values: vec![] }, None);
        assert_eq!(ok(&multi, &FormState { picked: vec![1, 2], ..Default::default() }), Submit::Field { field_id: 4, value: json!([1, 2]) });
        // Checkbox braucht eine Antwort
        let cb = field_item(InputKind::Checkbox, None);
        assert!(err(&cb, &FormState::default()).contains("yes or no"));
        assert_eq!(ok(&cb, &FormState { flag: Some(false), ..Default::default() }), Submit::Field { field_id: 4, value: json!(false) });
        // Datum
        let d = field_item(InputKind::Date, None);
        assert_eq!(ok(&d, &FormState { text: "today".into(), ..Default::default() }), Submit::Field { field_id: 4, value: json!("2026-11-14") });
        assert!(err(&d, &FormState { text: "31.02.2026".into(), ..Default::default() }).contains("valid date"));
        // Terminal nur aus der Liste
        assert!(validate(&field_item(InputKind::TerminalPick, None), &FormState::default(), n, Lang::De).unwrap_err().contains("Liste"));
    }

    #[test]
    fn comment_and_time_forms() {
        let n = now();
        let c = item(ItemKey::Comment, "Add comment", Group::Record);
        assert_eq!(validate(&c, &FormState { text: " hi ".into(), ..Default::default() }, n, Lang::En), Ok(Submit::Comment("hi".into())));
        assert!(validate(&c, &FormState::default(), n, Lang::En).is_err());
        let t = item(ItemKey::LogTime, "Log time", Group::Record);
        let f = FormState { text: "1h 30m".into(), comment: " work ".into(), work_kind: Some(2), ..Default::default() };
        assert_eq!(validate(&t, &f, n, Lang::En), Ok(Submit::LogTime { duration: "1h 30m".into(), start: None, work_kind: Some(2), comment: "work".into() }));
        let f = FormState { text: "2h".into(), start: "2026-11-13 09:00".into(), ..Default::default() };
        assert!(matches!(validate(&t, &f, n, Lang::En), Ok(Submit::LogTime { start: Some(_), .. })));
        assert!(validate(&t, &FormState { text: "2h".into(), start: "yesterday".into(), ..Default::default() }, n, Lang::En).unwrap_err().contains("start"));
        assert!(validate(&t, &FormState { text: "2h".into(), start: "gestern".into(), ..Default::default() }, n, Lang::De).unwrap_err().contains("Beginn"));
        assert!(validate(&t, &FormState::default(), n, Lang::En).unwrap_err().contains("duration"));
        assert!(validate(&t, &FormState { text: "abc".into(), ..Default::default() }, n, Lang::En).is_err());
        assert!(validate(&t, &FormState { text: "0m".into(), ..Default::default() }, n, Lang::En).is_err());
    }

    #[test]
    fn new_ticket_title_form_and_prefill() {
        let n = now();
        let a = Item { ask_title: true, ..item(ItemKey::Action(1), "Create", Group::Actions) };
        assert_eq!(validate(&a, &FormState { text: "Fix it".into(), ..Default::default() }, n, Lang::En), Ok(Submit::NewTitle("Fix it".into())));
        assert!(validate(&a, &FormState::default(), n, Lang::En).is_err());
        let plain = item(ItemKey::Action(1), "Go", Group::Actions);
        assert!(validate(&plain, &FormState::default(), n, Lang::En).is_err());
        // Vorbelegung
        let t = Item { current: Some(json!("Old")), ..item(ItemKey::Title, "Title", Group::Fields) };
        assert_eq!(FormState::for_item(&t).text, "Old");
        assert_eq!(FormState::for_item(&field_item(InputKind::Dropdown { values: vec![] }, Some(json!(7)))).picked, vec![7]);
        assert_eq!(FormState::for_item(&field_item(InputKind::MultiSelect { values: vec![] }, Some(json!([1, 2])))).picked, vec![1, 2]);
        assert_eq!(FormState::for_item(&field_item(InputKind::Checkbox, Some(json!(true)))).flag, Some(true));
        assert_eq!(FormState::for_item(&field_item(InputKind::Labels, Some(json!(["a", "b"])))).text, "a, b");
        assert_eq!(FormState::for_item(&field_item(InputKind::Number { min: None, max: None, decimals: None, unit: None }, Some(json!(2.5)))).text, "2.5");
        assert_eq!(FormState::for_item(&field_item(InputKind::Date, None)), FormState::default());
    }

    #[test]
    fn summaries_in_both_languages() {
        assert_eq!(summary_text("Done", 1, 0, Lang::En), "Done: done.");
        assert_eq!(summary_text("Done", 3, 0, Lang::En), "Done: 3 succeeded.");
        assert_eq!(summary_text("Done", 3, 1, Lang::En), "Done: 3 succeeded, 1 failed.");
        assert_eq!(summary_text("Done", 0, 2, Lang::En), "Done: 2 failed.");
        assert_eq!(summary_text("Erledigt", 3, 1, Lang::De), "Erledigt: 3 erfolgreich, 1 fehlgeschlagen.");
        assert_eq!(summary_text("Erledigt", 1, 0, Lang::De), "Erledigt: erledigt.");
        assert_ne!(summary_text("x", 2, 0, Lang::De), summary_text("x", 2, 0, Lang::En));
    }

    #[test]
    fn group_labels_are_bilingual() {
        for g in [Group::Fields, Group::Status, Group::Actions, Group::Record] {
            assert!(!g.label(Lang::De).is_empty() && !g.label(Lang::En).is_empty());
        }
        assert!(is_list_input(&InputKind::VersionPick) && !is_list_input(&InputKind::Date));
    }
}
