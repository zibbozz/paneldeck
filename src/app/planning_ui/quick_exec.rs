//! Schnellmenü: Laden der Zeilen für Tickets und Ausführen einer Zeile – alles über die Datenschicht, mit der Quelle
//! `Source::quick()` im Verlauf. Kennt kein egui.
//!
//! * [`load`] liest Tickets, Konfiguration und Feldwerte und baut die Zeilen (`quick_logic::items_for`/`merge_multi`).
//! * [`choices`] liefert die Auswahl für Listen-Eingaben (Auswahlliste, Version, Meilenstein, Ticket-Verweis).
//! * [`run_transition`], [`run_action`], [`run_submit`] führen eine Zeile für **jedes** Ticket einzeln aus: ein Fehler bei einem
//!   Ticket stoppt die anderen nicht ([`BatchResult`] nennt Erfolge und Fehlschläge mit Grund). Die Operationen aller Erfolge
//!   werden zu **einer** Rückgängig-Operation zusammengefasst ([`BatchResult::undo_op`]).
//! * „Sitzung starten“ legt nichts in der Datenbank an: die fertigen Angaben stehen in [`SessionRequest`]; die Anwendung startet
//!   die Sitzung und verknüpft sie mit dem Ticket.

use super::quick_logic::{items_for, merge_multi, Item, ItemKey, Submit};
use crate::planning::comments::add_comment;
use crate::planning::fields::{list_fields, Field, FieldKind};
use crate::planning::history::Source;
use crate::planning::links::{add_link, Target};
use crate::planning::ops::TicketOp;
use crate::planning::projects::get_project;
use crate::planning::quick::actions::{get_quick_action, list_project_quick_actions, list_quick_actions, QuickAction};
use crate::planning::quick::input::InputKind;
use crate::planning::quick::menu::{MenuConfig, TicketContext};
use crate::planning::quick::params::{ActionParams, LinkDirection};
use crate::planning::quick::resolve::{resolve_create_defaults, resolve_session, ResolveContext, SourceContext};
use crate::planning::quick::QuickError;
use crate::planning::sprints::releases::{get_release, list_releases, ReleaseKind};
use crate::planning::summary::{project_summary, ProjectSummary};
use crate::planning::ticket_error::{atomic, TRes, TicketError};
use crate::planning::tickets::{create_ticket, get_ticket, set_field, update_ticket, NewTicket, Ticket, TicketUpdate};
use crate::planning::time_entries::log_time;
use crate::planning::transition::transition_ticket;
use crate::planning::types::get_type;
use crate::planning::values::load_values;
use crate::planning::work_kinds::list_work_kinds;
use crate::state::Lang;
use chrono::NaiveDateTime;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;

/// Wie viele Treffer eine Auswahl-Liste höchstens zeigt.
pub const MAX_CHOICES: usize = 12;

/// Alles, was das Menü für seine Tickets braucht.
pub struct Loaded {
    pub project_id: i64,
    pub items: Vec<Item>,
    /// Arten der Arbeit des Projekts (für „Zeit erfassen“).
    pub work_kinds: Vec<(i64, String)>,
    /// Überschrift: „PAN-12 Login“ bzw. „3 Tickets“.
    pub title: String,
}

fn ref_label(conn: &Connection, f: &Field, v: &Value, lang: Lang) -> Option<String> {
    match f.kind {
        FieldKind::Ticket => get_ticket(conn, v.as_i64()?).ok().map(|t| format!("{} {}", t.key(), t.title)),
        FieldKind::Version => get_release(conn, ReleaseKind::Version, v.as_i64()?).ok().map(|r| r.name),
        FieldKind::Milestone => get_release(conn, ReleaseKind::Milestone, v.as_i64()?).ok().map(|r| r.name),
        FieldKind::Terminal => v.as_str().map(str::to_string).or_else(|| Some(lang.t("Terminal", "Terminal").to_string())),
        _ => None,
    }
}

fn context_of(conn: &Connection, t: &Ticket, cfg: &MenuConfig, lang: Lang) -> TRes<TicketContext> {
    let values = load_values(conn, t.id)?;
    let mut labels = HashMap::new();
    for f in &cfg.fields {
        if let Some(v) = values.get(&f.id) {
            if let Some(l) = ref_label(conn, f, v, lang) {
                labels.insert(f.id, l);
            }
        }
    }
    Ok(TicketContext { title: t.title.clone(), status_id: t.status_id, values: values.into_iter().collect(), labels })
}

/// Lädt Tickets und baut die Zeilen. Alle Tickets müssen zum selben Projekt gehören.
pub fn load(conn: &Connection, ids: &[i64], lang: Lang) -> Result<Loaded, String> {
    let err = |e: TicketError| e.message(lang);
    let mut tickets = Vec::new();
    for id in ids {
        tickets.push(get_ticket(conn, *id).map_err(err)?);
    }
    let first = tickets.first().ok_or_else(|| lang.t("Kein Ticket gewählt.", "No ticket selected.").to_string())?;
    let project_id = first.project_id;
    if tickets.iter().any(|t| t.project_id != project_id) {
        return Err(lang.t("Die Tickets gehören zu verschiedenen Projekten.", "The tickets belong to different projects.").to_string());
    }
    let sum: ProjectSummary = project_summary(conn, project_id).map_err(|e| e.message(lang))?;
    let actions = list_project_quick_actions(conn, project_id).map_err(|e| e.message(lang))?;
    let mut lists = Vec::new();
    for t in &tickets {
        let cfg = MenuConfig::from_summary(&sum, t.type_id, actions.clone()).ok_or_else(|| lang.t("Der Typ des Tickets fehlt.", "The ticket's type is missing.").to_string())?;
        let ctx = context_of(conn, t, &cfg, lang).map_err(err)?;
        lists.push(items_for(&cfg, &ctx, lang));
    }
    let items = merge_multi(&lists, lang);
    let work_kinds = list_work_kinds(conn, project_id).map_err(|e| e.message(lang))?.into_iter().map(|w| (w.id, w.display(lang))).collect();
    let title = if tickets.len() == 1 {
        format!("{} {}", tickets[0].key(), tickets[0].title)
    } else {
        match lang {
            Lang::De => format!("{} Tickets", tickets.len()),
            Lang::En => format!("{} tickets", tickets.len()),
        }
    };
    Ok(Loaded { project_id, items, work_kinds, title })
}

/// Eine Schnellaktion als Knopf in der Ticket-Ansicht (siehe [`buttons_for`]).
#[derive(Debug, Clone, PartialEq)]
pub struct QuickButton {
    pub action_id: i64,
    /// Beschriftung (Name der Aktion).
    pub label: String,
    /// Hinweistext: was die Aktion tut (Status setzen, Ticket anlegen, Sitzung starten).
    pub tip: String,
    /// `Some(Grund)`: nicht ausführbar (ausgegraut, der Grund steht im Hinweis).
    pub disabled: Option<String>,
}

/// Die Knöpfe für die Schnellaktionen des Typs von `t`, in der Reihenfolge der Aktionen des Typs. Dieselbe Logik wie das
/// Schnellmenü (`items_for`): ungültige Aktionen und Statuswechsel, die eine Bedingung sperrt, sind ausgegraut; ein
/// Statuswechsel in den aktuellen Status entfällt (nichts zu tun). Liest nur; wird beim Laden der Ticket-Ansicht aufgerufen.
pub fn buttons_for(conn: &Connection, sum: &ProjectSummary, t: &Ticket, lang: Lang) -> Vec<QuickButton> {
    let Ok(actions) = list_quick_actions(conn, t.type_id) else { return Vec::new() };
    if actions.is_empty() {
        return Vec::new();
    }
    let Some(cfg) = MenuConfig::from_summary(sum, t.type_id, actions) else { return Vec::new() };
    let Ok(ctx) = context_of(conn, t, &cfg, lang) else { return Vec::new() };
    let items = items_for(&cfg, &ctx, lang);
    cfg.actions
        .iter()
        .filter_map(|a| {
            let it = items.iter().find(|i| i.key == ItemKey::Action(a.id))?;
            if let ActionParams::ChangeStatus(c) = &a.params {
                if !a.invalid && ctx.status_id == Some(c.to_status_id) {
                    return None;
                }
            }
            let mut tip = it.detail.clone();
            if it.ask_title {
                tip = format!("{tip} · {}", lang.t("fragt zuerst den Titel", "asks for the title first"));
            }
            if let Some(why) = &it.disabled {
                tip = format!("{}: {why}", lang.t("Nicht verfügbar", "Not available"));
            }
            Some(QuickButton { action_id: a.id, label: it.label.clone(), tip, disabled: it.disabled.clone() })
        })
        .collect()
}

/// Auswahl für eine Listen-Eingabe, gefiltert nach `query` (Teilwort, Groß-/Kleinschreibung egal): (Kennung, Anzeigetext).
pub fn choices(conn: &Connection, kind: &InputKind, project_id: i64, query: &str, exclude: &[i64], lang: Lang) -> Vec<(i64, String)> {
    let q = crate::planning::quick::matcher::fold(query.trim());
    let keep = |s: &str| q.is_empty() || crate::planning::quick::matcher::fold(s).contains(&q);
    let mut out: Vec<(i64, String)> = match kind {
        InputKind::Dropdown { values } | InputKind::MultiSelect { values } => {
            values.iter().filter(|v| keep(&v.label) || v.aliases.iter().any(|a| keep(a))).map(|v| (v.id, v.label.clone())).collect()
        }
        InputKind::VersionPick => list_releases(conn, ReleaseKind::Version, project_id).unwrap_or_default().into_iter().filter(|r| keep(&r.name)).map(|r| (r.id, r.name)).collect(),
        InputKind::MilestonePick => list_releases(conn, ReleaseKind::Milestone, project_id).unwrap_or_default().into_iter().filter(|r| keep(&r.name)).map(|r| (r.id, r.name)).collect(),
        InputKind::TicketSearch { target_types } => search_tickets(conn, project_id, query, target_types, exclude),
        _ => Vec::new(),
    };
    let _ = lang;
    out.truncate(MAX_CHOICES);
    out
}

/// Tickets des Projekts nach Schlüssel (`PAN-12` oder `12`) oder Titelteil, neueste zuerst.
fn search_tickets(conn: &Connection, project_id: i64, query: &str, target_types: &[i64], exclude: &[i64]) -> Vec<(i64, String)> {
    let q = query.trim();
    let num: Option<i64> = q.rsplit(['-', ' ']).next().and_then(|n| n.parse().ok());
    let like = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let Ok(mut st) = conn.prepare(
        "SELECT t.id, p.key, t.number, t.title, t.type_id FROM tickets t JOIN projects p ON p.id = t.project_id
         WHERE t.project_id = ?1 AND (?2 = '' OR t.title LIKE ?3 ESCAPE '\\' OR t.number = ?4) ORDER BY t.updated_at DESC, t.id DESC LIMIT 60",
    ) else {
        return Vec::new();
    };
    let rows = st.query_map(params![project_id, q, like, num.unwrap_or(-1)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?)));
    let Ok(rows) = rows else { return Vec::new() };
    rows.filter_map(Result::ok)
        .filter(|(id, _, _, _, ty)| !exclude.contains(id) && (target_types.is_empty() || target_types.contains(ty)))
        .map(|(id, key, n, title, _)| (id, format!("{key}-{n} {title}")))
        .take(MAX_CHOICES)
        .collect()
}

/// Angaben zum Start einer Sitzung (fertig aufgelöst).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRequest {
    pub ticket_id: i64,
    pub ticket_key: String,
    pub shell: Option<String>,
    pub claude: bool,
    pub prompt: Option<String>,
    pub cwd: Option<String>,
}

/// Ergebnis eines Laufs über mehrere Tickets.
#[derive(Debug, Default)]
pub struct BatchResult {
    /// Schlüssel der erfolgreichen Tickets.
    pub ok: Vec<String>,
    /// (Schlüssel, Grund) der fehlgeschlagenen.
    pub failed: Vec<(String, String)>,
    /// Neu angelegte Tickets (Schlüssel).
    pub created: Vec<String>,
    pub ops: Vec<TicketOp>,
    pub sessions: Vec<SessionRequest>,
}

impl BatchResult {
    /// Eine Operation für „Rückgängig“ (alle Erfolge zusammen).
    pub fn undo_op(&self) -> Option<TicketOp> {
        TicketOp::combine(self.ops.clone())
    }

    /// Die Fehlschläge als Zeilen „PAN-5: Grund“.
    pub fn failure_lines(&self) -> Vec<String> {
        self.failed.iter().map(|(k, r)| format!("{k}: {r}")).collect()
    }

    fn record(&mut self, key: String, r: Result<Option<TicketOp>, String>) {
        match r {
            Ok(op) => {
                self.ok.push(key);
                self.ops.extend(op);
            }
            Err(e) => self.failed.push((key, e)),
        }
    }
}

fn key_of(conn: &Connection, id: i64) -> String {
    get_ticket(conn, id).map(|t| t.key()).unwrap_or_else(|_| format!("#{id}"))
}

/// Statuswechsel nach `to` für jedes Ticket (mit allen Regeln des Workflows).
pub fn run_transition(conn: &Connection, ids: &[i64], to: i64, lang: Lang) -> BatchResult {
    let mut out = BatchResult::default();
    for id in ids {
        let r = transition_ticket(conn, *id, to, &[], &Source::quick()).map_err(|e| e.message(lang));
        out.record(key_of(conn, *id), r);
    }
    out
}

/// Statuswechsel mit Feldwerten (Pflichtfelder des Übergangs). Jedes Ticket bekommt nur die Werte der Felder seines Typs, damit
/// ein Lauf über Tickets verschiedener Typen nicht an fremden Feldern scheitert.
pub fn run_transition_with(conn: &Connection, ids: &[i64], to: i64, values: &[(i64, Value)], lang: Lang) -> BatchResult {
    let mut out = BatchResult::default();
    for id in ids {
        let own = match get_ticket(conn, *id) {
            Ok(t) => values_of_type(conn, t.type_id, values),
            Err(_) => Vec::new(),
        };
        let r = transition_ticket(conn, *id, to, &own, &Source::quick()).map_err(|e| e.message(lang));
        out.record(key_of(conn, *id), r);
    }
    out
}

/// Die Werte, deren Felder zum Typ `type_id` gehören (ein Lauf über Tickets verschiedener Typen scheitert so nicht an
/// fremden Feldern).
fn values_of_type(conn: &Connection, type_id: i64, values: &[(i64, Value)]) -> Vec<(i64, Value)> {
    match list_fields(conn, type_id) {
        Ok(fields) => values.iter().filter(|(fid, _)| fields.iter().any(|f| f.id == *fid)).cloned().collect(),
        Err(_) => Vec::new(),
    }
}

/// Geprüfte Eingabe (Titel, Feld, Kommentar, Zeit) für jedes Ticket ausführen.
pub fn run_submit(conn: &Connection, ids: &[i64], s: &Submit, now: NaiveDateTime, lang: Lang) -> BatchResult {
    let mut out = BatchResult::default();
    for id in ids {
        let src = Source::quick();
        let r: TRes<Option<TicketOp>> = match s {
            Submit::Title(t) => update_ticket(conn, *id, &TicketUpdate { title: Some(t.clone()), values: vec![] }, &src),
            Submit::Field { field_id, value } => set_field(conn, *id, *field_id, value.clone(), &src),
            Submit::Comment(body) => add_comment(conn, *id, body, &src).map(|(_, op)| Some(op)),
            Submit::LogTime { duration, start, work_kind, comment } => log_time(conn, *id, duration, *start, *work_kind, comment, now, &src).map(|(_, op)| Some(op)),
            Submit::NewTitle(_) => Err(TicketError::Db("internal: NewTitle needs an action".into())),
        };
        out.record(key_of(conn, *id), r.map_err(|e| e.message(lang)));
    }
    out
}

fn source_context(conn: &Connection, t: &Ticket, lang: Lang) -> TRes<SourceContext> {
    let ty = get_type(conn, t.type_id)?;
    let status_name = match t.status_id {
        Some(s) => crate::planning::statuses::get_status(conn, s).map(|s| s.display(lang)).unwrap_or_default(),
        None => String::new(),
    };
    let fields = list_fields(conn, t.type_id)?;
    let values = load_values(conn, t.id)?;
    let description = fields.iter().find(|f| f.key.as_deref() == Some("description")).and_then(|f| values.get(&f.id)).and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(SourceContext { id: t.id, key: t.key(), number: t.number, title: t.title.clone(), type_name: ty.display(lang), status_name, description })
}

pub(super) fn resolve_context(conn: &Connection, t: &Ticket, lang: Lang, today: &str) -> TRes<ResolveContext> {
    let p = get_project(conn, t.project_id)?;
    Ok(ResolveContext { source: Some(source_context(conn, t, lang)?), project_key: p.key, project_name: p.name, today: today.to_string() })
}

fn quick_err(e: QuickError, lang: Lang) -> String {
    e.message(lang)
}

/// Eine Schnellaktion für jedes Ticket ausführen. `new_title` ersetzt den Titel bei „Ticket anlegen“ ohne Titelvorgabe.
pub fn run_action(conn: &Connection, ids: &[i64], action_id: i64, new_title: Option<&str>, now: NaiveDateTime, lang: Lang) -> BatchResult {
    run_action_with(conn, ids, action_id, new_title, &[], now, lang)
}

/// Wie [`run_action`], mit zusätzlichen Feldwerten (die Eingaben des Pflichtfeld-Formulars): beim Statuswechsel gehen sie in
/// denselben Wechsel, beim Anlegen in das neue Ticket – jeweils nur die Werte, deren Feld zum Typ gehört. Sie ergänzen die
/// Vorgaben der Aktion und überschreiben gleiche Felder.
pub fn run_action_with(conn: &Connection, ids: &[i64], action_id: i64, new_title: Option<&str>, extra: &[(i64, Value)], now: NaiveDateTime, lang: Lang) -> BatchResult {
    let mut out = BatchResult::default();
    let action: QuickAction = match get_quick_action(conn, action_id) {
        Ok(a) => a,
        Err(e) => {
            for id in ids {
                out.failed.push((key_of(conn, *id), quick_err(e.clone(), lang)));
            }
            return out;
        }
    };
    let today = now.format("%Y-%m-%d").to_string();
    for id in ids {
        let key = key_of(conn, *id);
        if action.invalid {
            out.failed.push((key, QuickError::ActionInvalid.message(lang)));
            continue;
        }
        let r: Result<Option<TicketOp>, String> = (|| -> Result<Option<TicketOp>, String> {
            let t = get_ticket(conn, *id).map_err(|e| e.message(lang))?;
            if t.type_id != action.type_id {
                return Err(lang.t("Die Aktion gehört zu einem anderen Tickettyp.", "The action belongs to a different ticket type.").to_string());
            }
            match &action.params {
                ActionParams::ChangeStatus(c) => {
                    let mut vals: Vec<(i64, Value)> = c.set_values.iter().map(|s| (s.field_id, s.value.clone())).collect();
                    vals.extend(values_of_type(conn, t.type_id, extra));
                    transition_ticket(conn, *id, c.to_status_id, &vals, &Source::quick()).map_err(|e| e.message(lang))
                }
                ActionParams::CreateTicket(_) => {
                    let ctx = resolve_context(conn, &t, lang, &today).map_err(|e| e.message(lang))?;
                    let resolved = resolve_create_defaults(conn, &action, &ctx).map_err(|e| quick_err(e, lang))?;
                    let title = match new_title.map(str::trim).filter(|s| !s.is_empty()) {
                        Some(s) => s.to_string(),
                        None => resolved.title.clone(),
                    };
                    let (ticket, op) = atomic(conn, || {
                        let mut values = resolved.values.clone();
                        values.extend(values_of_type(conn, resolved.ticket_type_id, extra));
                        let n = NewTicket { project_id: t.project_id, type_id: resolved.ticket_type_id, title, parent_id: None, values };
                        let (new, created) = create_ticket(conn, &n, &Source::quick())?;
                        let mut ops = vec![created];
                        if let Some(rel) = &resolved.relation {
                            let (from, to) = match rel.direction {
                                LinkDirection::Out => (t.id, new.id),
                                LinkDirection::In => (new.id, t.id),
                            };
                            let (_, l) = add_link(conn, from, rel.link_type_id, Target::Ticket(to), &Source::quick())?;
                            ops.push(l);
                        }
                        Ok((new, TicketOp::combine(ops)))
                    })
                    .map_err(|e| e.message(lang))?;
                    out.created.push(ticket.key());
                    Ok(op)
                }
                ActionParams::StartSession(p) => {
                    let src = source_context(conn, &t, lang).map_err(|e| e.message(lang))?;
                    let wd = get_project(conn, t.project_id).map(|p| p.work_dir).unwrap_or_default();
                    let s = resolve_session(p, Some(&src), &wd, lang);
                    out.sessions.push(SessionRequest { ticket_id: t.id, ticket_key: t.key(), shell: s.shell, claude: s.claude, prompt: s.prompt, cwd: s.cwd });
                    Ok(None)
                }
            }
        })();
        out.record(key, r);
    }
    out
}

/// Führt die Zeile `key` ohne Formular aus (Statuswechsel oder Schnellaktion).
pub fn run_direct(conn: &Connection, ids: &[i64], key: &ItemKey, now: NaiveDateTime, lang: Lang) -> Option<BatchResult> {
    match key {
        ItemKey::Transition(to) => Some(run_transition(conn, ids, *to, lang)),
        ItemKey::Action(id) => Some(run_action(conn, ids, *id, None, now, lang)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::comments::comments_of;
    use crate::planning::history::{history_of, SourceKind};
    use crate::planning::links::links_of;
    use crate::planning::quick::actions::{create_quick_action, NewQuickAction};
    use crate::planning::quick::params::{ChangeStatusParams, CreateTicketParams, DefaultValue, FieldRef, RelationSpec, StartSessionParams};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::time_entries::entries_of;
    use crate::planning::workflow::SetValue;
    use crate::undo::Op;
    use chrono::NaiveDate;
    use serde_json::json;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(10, 0, 0).unwrap()
    }

    fn add(f: &Fx, ty: i64, name: &str, p: ActionParams) -> i64 {
        create_quick_action(&f.c, &NewQuickAction::new(ty, name, p)).unwrap()
    }

    #[test]
    fn load_builds_items_with_reference_labels() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "Big plan");
        let story = f.mk(f.story, "Login");
        let epic_field = f.field(f.story, "epic");
        set_field(&f.c, story.id, epic_field.id, json!(epic.id), &Source::ui()).unwrap();
        let l = load(&f.c, &[story.id], Lang::En).unwrap();
        assert_eq!(l.title, format!("{} Login", story.key()));
        let it = l.items.iter().find(|i| i.label == "Epic").unwrap();
        assert!(it.detail.contains("Big plan"), "{}", it.detail);
        assert!(l.items.iter().any(|i| i.key == ItemKey::LogTime));
        assert!(l.work_kinds.len() >= 1);
        // mehrere Tickets, zwei Projekte, kein Ticket
        let l2 = load(&f.c, &[story.id, epic.id], Lang::De).unwrap();
        assert_eq!(l2.title, "2 Tickets");
        assert!(load(&f.c, &[], Lang::En).is_err());
        assert!(load(&f.c, &[9999], Lang::En).is_err());
    }

    #[test]
    fn transition_runs_with_quick_source_and_can_be_undone() {
        let f = Fx::new();
        let a = f.mk(f.story, "A");
        let b = f.mk(f.story, "B");
        let r = run_transition(&f.c, &[a.id, b.id], f.progress, Lang::En);
        assert_eq!((r.ok.len(), r.failed.len()), (2, 0));
        assert_eq!(get_ticket(&f.c, a.id).unwrap().status_id, Some(f.progress));
        let h = history_of(&f.c, a.id).unwrap();
        assert!(h.iter().any(|e| e.source.kind == SourceKind::Quick));
        let op = r.undo_op().unwrap();
        op.inverse().apply(&f.c).unwrap();
        assert_eq!(get_ticket(&f.c, a.id).unwrap().status_id, Some(f.todo));
        assert_eq!(get_ticket(&f.c, b.id).unwrap().status_id, Some(f.todo));
        // gleicher Status: kein Fehler, nichts zu tun
        let r = run_transition(&f.c, &[a.id], f.todo, Lang::En);
        assert_eq!((r.ok.len(), r.failed.len(), r.ops.len()), (1, 0, 0));
    }

    #[test]
    fn one_failure_does_not_stop_the_others_and_names_the_reason() {
        let f = Fx::new();
        let a = f.mk(f.story, "A");
        let no_wf = crate::planning::types::create_type(&f.c, &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Note".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let note = f.mk(no_wf, "N");
        let r = run_transition(&f.c, &[note.id, a.id], f.progress, Lang::En);
        assert_eq!(r.ok, vec![a.key()]);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].0, note.key());
        assert!(r.failure_lines()[0].starts_with(&format!("{}:", note.key())));
        assert!(r.undo_op().is_some());
    }

    #[test]
    fn submit_title_field_comment_and_time() {
        let f = Fx::new();
        let t = f.mk(f.story, "Old");
        let sp = f.field(f.story, "story_points");
        let r = run_submit(&f.c, &[t.id], &Submit::Title("New title".into()), now(), Lang::En);
        assert_eq!(r.ok.len(), 1);
        assert_eq!(get_ticket(&f.c, t.id).unwrap().title, "New title");
        let r = run_submit(&f.c, &[t.id], &Submit::Field { field_id: sp.id, value: json!(5) }, now(), Lang::En);
        assert_eq!(r.ok.len(), 1);
        assert_eq!(load_values(&f.c, t.id).unwrap().get(&sp.id), Some(&json!(5)));
        // leeren
        run_submit(&f.c, &[t.id], &Submit::Field { field_id: sp.id, value: Value::Null }, now(), Lang::En);
        assert!(load_values(&f.c, t.id).unwrap().get(&sp.id).is_none());
        let r = run_submit(&f.c, &[t.id], &Submit::Comment("hello".into()), now(), Lang::En);
        assert_eq!(comments_of(&f.c, t.id).unwrap().len(), 1);
        r.undo_op().unwrap().inverse().apply(&f.c).unwrap();
        assert!(comments_of(&f.c, t.id).unwrap().is_empty());
        let r = run_submit(&f.c, &[t.id], &Submit::LogTime { duration: "1h 30m".into(), start: None, work_kind: None, comment: "x".into() }, now(), Lang::En);
        assert_eq!(r.ok.len(), 1);
        let e = entries_of(&f.c, t.id).unwrap();
        assert_eq!((e.len(), e[0].minutes), (1, 90));
        // falsches Feld für ein Ticket anderen Typs → Fehler mit Grund, kein Abbruch
        let e2 = f.mk(f.epic, "E");
        let r = run_submit(&f.c, &[e2.id, t.id], &Submit::Field { field_id: sp.id, value: json!(3) }, now(), Lang::En);
        assert_eq!((r.ok.len(), r.failed.len()), (1, 1));
        let r = run_submit(&f.c, &[t.id], &Submit::Comment("  ".into()), now(), Lang::De);
        assert_eq!(r.failed.len(), 1);
    }

    #[test]
    fn change_status_action_writes_values_and_create_action_links_the_new_ticket() {
        let f = Fx::new();
        let story = f.mk(f.story, "Login");
        let sp = f.field(f.story, "story_points");
        let act = add(&f, f.story, "Finish", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.done, set_values: vec![SetValue { field_id: sp.id, value: json!(8) }] }));
        let r = run_action(&f.c, &[story.id], act, None, now(), Lang::En);
        assert_eq!(r.ok.len(), 1, "{:?}", r.failed);
        assert_eq!(get_ticket(&f.c, story.id).unwrap().status_id, Some(f.done));
        assert_eq!(load_values(&f.c, story.id).unwrap().get(&sp.id), Some(&json!(8)));
        // Ticket anlegen, Titel aus Platzhalter, verknüpft
        let lt = crate::planning::link_types::link_type_by_key(&f.c, "blocks").unwrap().id;
        let create = add(
            &f,
            f.story,
            "Task",
            ActionParams::CreateTicket(CreateTicketParams {
                ticket_type_id: f.task,
                relation: Some(RelationSpec { link_type_id: lt, direction: LinkDirection::Out }),
                defaults: vec![DefaultValue { field_id: FieldRef::Title, value: json!("Task for {{source.key}}") }],
            }),
        );
        let r = run_action(&f.c, &[story.id], create, None, now(), Lang::En);
        assert_eq!(r.ok.len(), 1, "{:?}", r.failed);
        assert_eq!(r.created.len(), 1);
        let new = crate::planning::tickets::ticket_by_key(&f.c, &r.created[0]).unwrap();
        assert_eq!(new.title, format!("Task for {}", story.key()));
        assert_eq!(links_of(&f.c, story.id, Lang::En).unwrap().len(), 1);
        // Rückgängig: Verknüpfung und Ticket verschwinden
        r.undo_op().unwrap().inverse().apply(&f.c).unwrap();
        assert!(crate::planning::tickets::ticket_by_key(&f.c, &r.created[0]).is_err());
        assert!(links_of(&f.c, story.id, Lang::En).unwrap().is_empty());
    }

    #[test]
    fn create_action_without_title_uses_the_given_one_or_fails_with_a_reason() {
        let f = Fx::new();
        let story = f.mk(f.story, "Login");
        let create = add(&f, f.story, "Task", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.task, relation: None, defaults: vec![] }));
        let r = run_action(&f.c, &[story.id], create, None, now(), Lang::En);
        assert_eq!(r.failed.len(), 1, "ohne Titel");
        let r = run_action(&f.c, &[story.id], create, Some("  Typed  "), now(), Lang::En);
        assert_eq!(r.ok.len(), 1);
        assert_eq!(crate::planning::tickets::ticket_by_key(&f.c, &r.created[0]).unwrap().title, "Typed");
    }

    #[test]
    fn session_action_returns_a_request_and_changes_nothing() {
        let f = Fx::new();
        let story = f.mk(f.story, "Login");
        let act = add(&f, f.story, "Claude", ActionParams::StartSession(StartSessionParams { claude: true, prompt_from_ticket: true, cwd_from_project: true, shell: None }));
        f.c.execute("UPDATE projects SET work_dir = 'C:\\work' WHERE id = ?1", [f.project]).unwrap();
        let r = run_action(&f.c, &[story.id], act, None, now(), Lang::En);
        assert_eq!(r.ok.len(), 1);
        assert!(r.ops.is_empty());
        let s = &r.sessions[0];
        assert!(s.claude && s.prompt.as_deref().unwrap().contains("Login") && s.cwd.as_deref() == Some("C:\\work"));
        assert_eq!(s.ticket_key, story.key());
    }

    #[test]
    fn invalid_or_foreign_actions_fail_cleanly() {
        let f = Fx::new();
        let story = f.mk(f.story, "Login");
        let epic = f.mk(f.epic, "E");
        let act = add(&f, f.story, "Session", ActionParams::StartSession(StartSessionParams::default()));
        let r = run_action(&f.c, &[epic.id], act, None, now(), Lang::En);
        assert_eq!(r.failed.len(), 1, "anderer Tickettyp");
        f.c.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [act]).unwrap();
        let r = run_action(&f.c, &[story.id], act, None, now(), Lang::De);
        assert!(r.failed[0].1.contains("ungültig"));
        let r = run_action(&f.c, &[story.id], 99_999, None, now(), Lang::En);
        assert_eq!(r.failed.len(), 1);
        assert!(run_direct(&f.c, &[story.id], &ItemKey::Title, now(), Lang::En).is_none());
    }

    #[test]
    fn choices_filter_lists_and_search_tickets() {
        let f = Fx::new();
        let a = f.mk(f.story, "Login page");
        let _b = f.mk(f.story, "Logout");
        let epic = f.mk(f.epic, "Big plan");
        let kind = InputKind::TicketSearch { target_types: vec![f.epic] };
        let c = choices(&f.c, &kind, f.project, "", &[], Lang::En);
        assert_eq!(c.len(), 1, "nur Epics");
        assert!(c[0].1.contains("Big plan"));
        let any = InputKind::TicketSearch { target_types: vec![] };
        assert_eq!(choices(&f.c, &any, f.project, "log", &[], Lang::En).len(), 2);
        assert_eq!(choices(&f.c, &any, f.project, "log", &[a.id], Lang::En).len(), 1);
        assert_eq!(choices(&f.c, &any, f.project, &epic.key(), &[], Lang::En)[0].0, epic.id);
        assert_eq!(choices(&f.c, &any, f.project, "100%_", &[], Lang::En).len(), 0, "Platzhalter werden nicht ausgewertet");
        let dd = InputKind::Dropdown { values: vec![crate::planning::quick::input::InputOption { id: 1, label: "High".into(), aliases: vec!["Hoch".into()], color: None }, crate::planning::quick::input::InputOption { id: 2, label: "Low".into(), aliases: vec![], color: None }] };
        assert_eq!(choices(&f.c, &dd, f.project, "hoch", &[], Lang::En), vec![(1, "High".to_string())]);
        assert_eq!(choices(&f.c, &dd, f.project, "", &[], Lang::En).len(), 2);
        assert!(choices(&f.c, &InputKind::Date, f.project, "", &[], Lang::En).is_empty());
    }
}
