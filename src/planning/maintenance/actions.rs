//! Aktionen der Wartung: Vorschau ([`preview`]) und Ausführen ([`apply`]). Die Wartung **löscht nie Tickets**; sie wechselt den
//! Status (in den Zielstatus „Won't Do“ des Projekts, mit Kommentar), archiviert, setzt Fälligkeiten, verknüpft Duplikate,
//! entfernt ungenutzte Konfiguration, führt Labels zusammen und räumt kaputte Verweise auf. Jede Änderung trägt die Quelle
//! „Wartung“ im Verlauf, jede Aktion steht im Wartungsprotokoll. Die Sicherung davor legt der Aufrufer an (Name → Protokoll).
//!
//! Der Statuswechsel der Wartung setzt den Zielstatus direkt (wie eine Massenänderung eines Administrators): Übergänge und
//! ihre Bedingungen des Workflows gelten hier nicht; Tickets, deren Typ den Zielstatus nicht im Workflow hat, werden
//! übersprungen und in der Vorschau genannt.

use super::{maint_project, write_log, MRes, MaintError, Target, TicketRef};
use crate::planning::fields::{list_fields, FieldKind};
use crate::planning::history::{self, HistoryField, Source};
use crate::planning::tickets::get_ticket;
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Ab so vielen Tickets (oder bei jeder Entfernung) verlangt die Oberfläche den 3-Sekunden-Knopf.
pub const CONFIRM_ABOVE: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    CloseWontDo { tickets: Vec<i64>, comment: String },
    Archive { tickets: Vec<i64> },
    Unarchive { tickets: Vec<i64> },
    /// Fälligkeit setzen (`JJJJ-MM-TT` oder `JJJJ-MM-TTTHH:MM`).
    SetDue { tickets: Vec<i64>, due: String },
    /// `ticket` als Duplikat von `original` verknüpfen; mit `close` zusätzlich in den Won't-Do-Status (mit Kommentar).
    LinkDuplicate { ticket: i64, original: i64, close: bool, comment: String },
    /// Ungenutzte Konfiguration entfernen.
    Remove { targets: Vec<Target> },
    MergeLabels { keep: String, merge: Vec<String> },
    DetachParent { tickets: Vec<i64> },
    ClearReference { tickets: Vec<i64> },
    RuleAction { rule: i64, tickets: Vec<i64> },
}

impl Action {
    /// Kennung im Protokoll.
    pub fn key(&self) -> &'static str {
        match self {
            Action::CloseWontDo { .. } => "close_wont_do",
            Action::Archive { .. } => "archive",
            Action::Unarchive { .. } => "unarchive",
            Action::SetDue { .. } => "set_due",
            Action::LinkDuplicate { .. } => "link_duplicate",
            Action::Remove { .. } => "remove_config",
            Action::MergeLabels { .. } => "merge_labels",
            Action::DetachParent { .. } => "detach_parent",
            Action::ClearReference { .. } => "clear_reference",
            Action::RuleAction { .. } => "rule_action",
        }
    }

    /// Entfernt die Aktion etwas (3-Sekunden-Knopf immer)?
    pub fn removes(&self) -> bool {
        matches!(self, Action::Remove { .. })
    }
}

/// Standard-Kommentar beim Schließen: „Closed by maintenance on 2026-11-20: unchanged for 94 days.“ (Sprache des Projekts,
/// d. h. der Oberfläche). In der Vorschau änderbar.
pub fn default_comment(lang: Lang, date: chrono::NaiveDate, reason: &str) -> String {
    let d = date.format("%Y-%m-%d");
    match lang {
        Lang::De => format!("Von der Wartung am {d} geschlossen: {reason}."),
        Lang::En => format!("Closed by maintenance on {d}: {reason}."),
    }
}

/// Grund für den Standard-Kommentar.
pub fn reason_text(lang: Lang, check: super::Check, days: Option<i64>) -> String {
    use super::Check::*;
    match (check, days, lang) {
        (StaleInProgress | StaleNew, Some(d), Lang::De) => format!("seit {d} Tagen unverändert"),
        (StaleInProgress | StaleNew, Some(d), Lang::En) => format!("unchanged for {d} days"),
        (StaleInProgress | StaleNew, None, Lang::De) => "lange unverändert".into(),
        (StaleInProgress | StaleNew, None, Lang::En) => "unchanged for a long time".into(),
        (Overdue, Some(d), Lang::De) => format!("seit {d} Tagen überfällig"),
        (Overdue, Some(d), Lang::En) => format!("overdue for {d} days"),
        (OpenInFinished, _, Lang::De) => "offen in einem abgeschlossenen Sprint oder Release".into(),
        (OpenInFinished, _, Lang::En) => "open in a finished sprint or release".into(),
        (PossibleDuplicate, _, Lang::De) => "Duplikat".into(),
        (PossibleDuplicate, _, Lang::En) => "duplicate".into(),
        (_, _, Lang::De) => "von der Wartungsprüfung gefunden".into(),
        (_, _, Lang::En) => "found by the maintenance check".into(),
    }
}

/// Vorschau einer Aktion.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    /// Überschrift, z. B. „23 Tickets werden auf Won't Do gesetzt“.
    pub headline: String,
    /// Betroffene Tickets (anklickbar).
    pub tickets: Vec<TicketRef>,
    /// Weitere Zeilen (entfernte Konfiguration, übersprungene Tickets mit Grund …).
    pub lines: Vec<String>,
    /// So viele Dinge ändern sich.
    pub count: usize,
    /// Braucht den 3-Sekunden-Knopf (mehr als zehn Tickets oder Entfernen).
    pub confirm_slow: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub changed: usize,
    /// Übersprungen (Schlüssel/Name, Grund).
    pub skipped: Vec<(String, String)>,
    pub log_id: i64,
}

fn tref(conn: &Connection, id: i64) -> Option<TicketRef> {
    get_ticket(conn, id).ok().map(|t| TicketRef { id: t.id, key: t.key(), title: t.title })
}

/// Darf der Zielstatus für das Ticket gesetzt werden? `Err(Grund)` sonst.
fn status_allowed(conn: &Connection, ticket: i64, status: i64, lang: Lang) -> MRes<Result<(), String>> {
    let t = get_ticket(conn, ticket)?;
    let ty = crate::planning::types::get_type(conn, t.type_id)?;
    if !ty.workflow {
        return Ok(Err(lang.t("Typ ohne Workflow (kein Status)", "type without workflow (no status)").into()));
    }
    let wf = crate::planning::workflow::workflow_statuses(conn, t.type_id)?;
    if !wf.contains(&status) {
        return Ok(Err(lang.t("Zielstatus nicht im Workflow des Typs", "target status is not in the type's workflow").into()));
    }
    if t.status_id == Some(status) {
        return Ok(Err(lang.t("hat den Status schon", "already has the status").into()));
    }
    Ok(Ok(()))
}

fn wont_do(conn: &Connection, project_id: i64) -> MRes<i64> {
    super::load_settings(conn, project_id)?.wont_do.ok_or_else(|| {
        MaintError::refused(
            "Für dieses Projekt ist kein Zielstatus „Won't Do“ gewählt (Projekt-Einstellungen → Wartung).",
            "No “Won't Do” target status is chosen for this project (Project settings → Maintenance).",
        )
    })
}

/// Tickets des Projekts aus der Liste (fremde und unbekannte fallen weg).
fn own(conn: &Connection, project_id: i64, ids: &[i64]) -> MRes<Vec<i64>> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for id in ids {
        let p: Option<i64> = conn.query_row("SELECT project_id FROM tickets WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        if p == Some(project_id) && seen.insert(*id) {
            v.push(*id);
        }
    }
    Ok(v)
}

fn n_tickets(lang: Lang, n: usize) -> String {
    match lang {
        Lang::De => format!("{n} Ticket(s)"),
        Lang::En => format!("{n} ticket(s)"),
    }
}


/// Name eines Konfigurations-Ziels für Vorschau und Protokoll; `None`, wenn es das Ziel nicht mehr gibt.
fn target_name(conn: &Connection, t: &Target, lang: Lang) -> Option<String> {
    use crate::planning::*;
    Some(match t {
        Target::Field(id) => {
            let f = fields::get_field(conn, *id).ok()?;
            format!("{} {}", lang.t("Feld", "Field"), lang.q(&f.display(lang)))
        }
        Target::FieldOption { field, option } => {
            let f = fields::get_field(conn, *field).ok()?;
            let o = f.options.values.iter().find(|o| o.id == *option)?;
            format!("{} {} ({})", lang.t("Auswahlwert", "Option"), lang.q(&o.display(lang)), f.display(lang))
        }
        Target::Status(id) => format!("{} {}", lang.t("Status", "Status"), lang.q(&statuses::get_status(conn, *id).ok()?.display(lang))),
        Target::Type(id) => format!("{} {}", lang.t("Typ", "Type"), lang.q(&types::get_type(conn, *id).ok()?.display(lang))),
        Target::LinkType(id) => format!("{} {}", lang.t("Verknüpfungstyp", "Link type"), lang.q(&link_types::get_link_type(conn, *id).ok()?.names(lang).0)),
        Target::WorkKind(id) => format!("{} {}", lang.t("Art der Arbeit", "Kind of work"), lang.q(&work_kinds::get_work_kind(conn, *id).ok()?.display(lang))),
        _ => return None,
    })
}

/// Ist das Ziel ungenutzt (nur dann darf die Wartung es entfernen)?
fn target_unused(conn: &Connection, t: &Target) -> MRes<bool> {
    use crate::planning::integrity::*;
    Ok(match t {
        Target::Field(id) => conn.query_row("SELECT COUNT(*) FROM ticket_values WHERE field_id = ?1", [id], |r| r.get::<_, i64>(0))? == 0,
        Target::FieldOption { field, option } => usage_count_option(conn, *field, *option)? == 0,
        Target::Status(id) => usage_count_status(conn, *id)? == 0,
        Target::Type(id) => usage_count_type(conn, *id)? == 0,
        Target::LinkType(id) => usage_count_link_type(conn, *id)? == 0,
        Target::WorkKind(id) => usage_count_work_kind(conn, *id)? == 0,
        _ => false,
    })
}

/// Gehört das Konfigurations-Ziel zum Projekt (Verknüpfungstypen sind global)?
fn target_in_project(conn: &Connection, project_id: i64, t: &Target) -> MRes<bool> {
    let q = |sql: &str, id: i64| -> rusqlite::Result<Option<i64>> { conn.query_row(sql, [id], |r| r.get(0)).optional() };
    Ok(match t {
        Target::Field(id) | Target::FieldOption { field: id, .. } => q("SELECT tt.project_id FROM fields f JOIN ticket_types tt ON tt.id = f.type_id WHERE f.id = ?1", *id)? == Some(project_id),
        Target::Status(id) => q("SELECT project_id FROM statuses WHERE id = ?1", *id)? == Some(project_id),
        Target::Type(id) => q("SELECT project_id FROM ticket_types WHERE id = ?1", *id)? == Some(project_id),
        Target::WorkKind(id) => q("SELECT project_id FROM work_kinds WHERE id = ?1", *id)? == Some(project_id),
        Target::LinkType(id) => q("SELECT id FROM link_types WHERE id = ?1 AND system = 0", *id)?.is_some(),
        _ => false,
    })
}

/// Tickets mit Labels in einer der Schreibweisen von `merge`.
fn label_tickets(conn: &Connection, project_id: i64, merge: &[String]) -> MRes<Vec<i64>> {
    let mut st = conn.prepare(
        "SELECT DISTINCT v.ticket_id FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND f.kind = 'labels' AND v.text = ?2",
    )?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for m in merge {
        for id in st.query_map(params![project_id, m], |r| r.get::<_, i64>(0))? {
            let id = id?;
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// Was die Aktion tun würde (nur lesen). Ein Fehler heißt: die Aktion ist so nicht möglich (z. B. kein Won't-Do-Status).
pub fn preview(conn: &Connection, project_id: i64, a: &Action, lang: Lang) -> MRes<Preview> {
    maint_project(conn, project_id)?;
    let mut lines = Vec::new();
    let mut tickets = Vec::new();
    let add_all = |ids: &[i64], tickets: &mut Vec<TicketRef>| {
        for id in ids {
            if let Some(t) = tref(conn, *id) {
                tickets.push(t);
            }
        }
    };
    let headline = match a {
        Action::CloseWontDo { .. } | Action::LinkDuplicate { close: true, .. } => {
            let status = wont_do(conn, project_id)?;
            let sname = crate::planning::statuses::get_status(conn, status)?.display(lang);
            let ids = match a {
                Action::LinkDuplicate { ticket, .. } => own(conn, project_id, &[*ticket])?,
                Action::CloseWontDo { tickets: ids, .. } => own(conn, project_id, ids)?,
                _ => Vec::new(),
            };
            for id in ids {
                let Some(t) = tref(conn, id) else { continue };
                match status_allowed(conn, id, status, lang)? {
                    Ok(()) => tickets.push(t),
                    Err(why) => lines.push(format!("{} – {} ({why})", t.key, lang.t("übersprungen", "skipped"))),
                }
            }
            match a {
                Action::LinkDuplicate { original, .. } => {
                    let o = tref(conn, *original).map(|t| t.key).unwrap_or_default();
                    match lang {
                        Lang::De => format!("Als Duplikat von {o} verknüpfen und auf {} setzen", lang.q(&sname)),
                        Lang::En => format!("Link as duplicate of {o} and set to {}", lang.q(&sname)),
                    }
                }
                _ => match lang {
                    Lang::De => format!("{} werden auf {} gesetzt (mit Kommentar)", n_tickets(lang, tickets.len()), lang.q(&sname)),
                    Lang::En => format!("{} will be set to {} (with a comment)", n_tickets(lang, tickets.len()), lang.q(&sname)),
                },
            }
        }
        _ => String::new(),
    };
    let headline = match a {
        Action::CloseWontDo { .. } | Action::LinkDuplicate { close: true, .. } => headline,
        Action::LinkDuplicate { ticket, original, .. } => {
            add_all(&own(conn, project_id, &[*ticket])?, &mut tickets);
            let o = tref(conn, *original).map(|t| t.key).unwrap_or_default();
            match lang {
                Lang::De => format!("Als Duplikat von {o} verknüpfen"),
                Lang::En => format!("Link as duplicate of {o}"),
            }
        }
        Action::Archive { tickets: ids } => {
            add_all(&own(conn, project_id, ids)?, &mut tickets);
            match lang {
                Lang::De => format!("{} werden archiviert (aus Listen, Boards und Suche ausgeblendet, jederzeit wiederherstellbar)", n_tickets(lang, tickets.len())),
                Lang::En => format!("{} will be archived (hidden from lists, boards and search, restorable at any time)", n_tickets(lang, tickets.len())),
            }
        }
        Action::Unarchive { tickets: ids } => {
            add_all(&own(conn, project_id, ids)?, &mut tickets);
            match lang {
                Lang::De => format!("{} werden aus dem Archiv geholt", n_tickets(lang, tickets.len())),
                Lang::En => format!("{} will be restored from the archive", n_tickets(lang, tickets.len())),
            }
        }
        Action::SetDue { tickets: ids, due } => {
            if crate::due::parse_stored(due).is_none() {
                return Err(MaintError::refused("Das Datum ist ungültig (JJJJ-MM-TT).", "The date is invalid (YYYY-MM-DD)."));
            }
            add_all(&own(conn, project_id, ids)?, &mut tickets);
            match lang {
                Lang::De => format!("{} bekommen die Fälligkeit {}", n_tickets(lang, tickets.len()), crate::due::display(due, lang)),
                Lang::En => format!("{} get the due date {}", n_tickets(lang, tickets.len()), crate::due::display(due, lang)),
            }
        }
        Action::Remove { targets } => {
            let mut n = 0;
            for t in targets {
                let Some(name) = target_name(conn, t, lang) else { continue };
                if !target_in_project(conn, project_id, t)? {
                    lines.push(format!("{name} – {}", lang.t("gehört nicht zum Projekt", "does not belong to the project")));
                } else if target_unused(conn, t)? {
                    lines.push(name);
                    n += 1;
                } else {
                    lines.push(format!("{name} – {}", lang.t("in Benutzung, bleibt", "in use, kept")));
                }
            }
            match lang {
                Lang::De => format!("{n} ungenutzte Einträge werden entfernt"),
                Lang::En => format!("{n} unused entries will be removed"),
            }
        }
        Action::MergeLabels { keep, merge } => {
            add_all(&label_tickets(conn, project_id, merge)?, &mut tickets);
            let m: Vec<String> = merge.iter().map(|x| lang.q(x)).collect();
            match lang {
                Lang::De => format!("{} → {} in {}", m.join(", "), lang.q(keep), n_tickets(lang, tickets.len())),
                Lang::En => format!("{} → {} in {}", m.join(", "), lang.q(keep), n_tickets(lang, tickets.len())),
            }
        }
        Action::DetachParent { tickets: ids } => {
            add_all(&own(conn, project_id, ids)?, &mut tickets);
            match lang {
                Lang::De => format!("{}: Verweis auf das gelöschte Eltern-Ticket wird gelöst", n_tickets(lang, tickets.len())),
                Lang::En => format!("{}: the reference to the deleted parent is removed", n_tickets(lang, tickets.len())),
            }
        }
        Action::ClearReference { tickets: ids } => {
            add_all(&own(conn, project_id, ids)?, &mut tickets);
            match lang {
                Lang::De => format!("{}: Verweise auf Gelöschtes und ungültige Werte werden entfernt", n_tickets(lang, tickets.len())),
                Lang::En => format!("{}: references to deleted items and invalid values are removed", n_tickets(lang, tickets.len())),
            }
        }
        Action::RuleAction { rule, tickets: ids } => {
            let r = super::rules::get_rule(conn, *rule)?;
            add_all(&own(conn, project_id, ids)?, &mut tickets);
            if let super::rules::RuleAction::Status { status } = r.action {
                let keep: Vec<TicketRef> = std::mem::take(&mut tickets);
                for t in keep {
                    match status_allowed(conn, t.id, status, lang)? {
                        Ok(()) => tickets.push(t),
                        Err(why) => lines.push(format!("{} – {} ({why})", t.key, lang.t("übersprungen", "skipped"))),
                    }
                }
            }
            format!("{} {}: {} – {}", lang.t("Regel", "Rule"), lang.q(&r.name), super::rules::action_text(conn, &r.action, lang), n_tickets(lang, tickets.len()))
        }
    };
    let count = if a.removes() { lines.iter().filter(|l| !l.contains(" – ")).count() } else { tickets.len() };
    Ok(Preview { headline, confirm_slow: count > CONFIRM_ABOVE || a.removes(), tickets, lines, count })
}

/// Status direkt setzen (Wartung), mit Verlauf.
fn force_status(conn: &Connection, id: i64, status: i64, src: &Source) -> MRes<bool> {
    let batch = history::next_batch(conn)?;
    Ok(crate::planning::tickets::raw_set_status(conn, id, Some(status), src, batch)?.is_some())
}

fn set_archived(conn: &Connection, id: i64, on: bool, src: &Source) -> MRes<bool> {
    let t = get_ticket(conn, id)?;
    if t.archived == on {
        return Ok(false);
    }
    conn.execute("UPDATE tickets SET archived = ?1 WHERE id = ?2", params![on as i64, id])?;
    let batch = history::next_batch(conn)?;
    history::log(conn, id, src, batch, &HistoryField::Other("archived".into()), Some(if on { "0" } else { "1" }), Some(if on { "1" } else { "0" }))?;
    Ok(true)
}

/// Archivieren bzw. aus dem Archiv holen aus der Ticket-Ansicht (Menü „⋯“, „Wiederherstellen“): Quelle Oberfläche, mit Verlauf,
/// ohne Wartungsprotokoll.
pub fn set_archived_ui(conn: &Connection, id: i64, on: bool) -> MRes<bool> {
    set_archived(conn, id, on, &Source::ui())
}

/// Verknüpfungstyp „duplicates“; wird bei Bedarf angelegt (er ist ein löschbarer Standard-Typ).
pub fn duplicates_link_type(conn: &Connection) -> MRes<i64> {
    if let Some(id) = conn.query_row("SELECT id FROM link_types WHERE key = 'duplicates'", [], |r| r.get(0)).optional()? {
        return Ok(id);
    }
    let pos: i64 = conn.query_row("SELECT COALESCE(MAX(position) + 1, 0) FROM link_types", [], |r| r.get(0))?;
    conn.execute("INSERT INTO link_types (key, name_out, name_in, symmetric, system, target_kind, position) VALUES ('duplicates', NULL, NULL, 0, 0, 'ticket', ?1)", [pos])?;
    Ok(conn.last_insert_rowid())
}

/// Führt die Aktion aus (alles oder nichts) und schreibt das Protokoll. `backup` = Dateiname der Sicherung davor.
pub fn apply(conn: &Connection, project_id: i64, a: &Action, lang: Lang, backup: Option<&str>) -> MRes<Outcome> {
    let p = preview(conn, project_id, a, lang)?;
    let src = Source::maintenance();
    conn.execute_batch("SAVEPOINT maint_action")?;
    let r = run(conn, project_id, a, lang, &p, &src);
    match r {
        Ok((changed, skipped)) => {
            let detail = match a {
                Action::Remove { .. } => p.lines.iter().filter(|l| !l.contains(" – ")).cloned().collect::<Vec<_>>().join("; "),
                _ => p.tickets.iter().map(|t| t.key.clone()).collect::<Vec<_>>().join(", "),
            };
            let log_id = write_log(conn, Some(project_id), a.key(), changed as i64, backup, &detail.chars().take(4000).collect::<String>())?;
            conn.execute_batch("RELEASE maint_action")?;
            Ok(Outcome { changed, skipped, log_id })
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO maint_action; RELEASE maint_action");
            Err(e)
        }
    }
}

fn run(conn: &Connection, project_id: i64, a: &Action, lang: Lang, p: &Preview, src: &Source) -> MRes<(usize, Vec<(String, String)>)> {
    let mut changed = 0;
    let mut skipped = Vec::new();
    let ids: Vec<i64> = p.tickets.iter().map(|t| t.id).collect();
    match a {
        Action::CloseWontDo { comment, .. } => {
            let status = wont_do(conn, project_id)?;
            for id in &ids {
                if force_status(conn, *id, status, src)? {
                    changed += 1;
                    if !comment.trim().is_empty() {
                        crate::planning::comments::add_comment(conn, *id, comment, src)?;
                    }
                }
            }
        }
        Action::LinkDuplicate { ticket, original, close, comment } => {
            if ticket == original {
                return Err(MaintError::refused("Ein Ticket kann kein Duplikat von sich selbst sein.", "A ticket cannot duplicate itself."));
            }
            own(conn, project_id, &[*original])?.first().ok_or(MaintError::NotFound)?;
            let lt = duplicates_link_type(conn)?;
            match crate::planning::links::add_link(conn, *ticket, lt, crate::planning::links::Target::Ticket(*original), src) {
                Ok(_) => changed += 1,
                Err(crate::planning::ticket_error::TicketError::LinkDuplicate) => {}
                Err(e) => return Err(e.into()),
            }
            if *close && ids.contains(ticket) {
                let status = wont_do(conn, project_id)?;
                force_status(conn, *ticket, status, src)?;
                if !comment.trim().is_empty() {
                    crate::planning::comments::add_comment(conn, *ticket, comment, src)?;
                }
            }
        }
        Action::Archive { .. } | Action::Unarchive { .. } => {
            let on = matches!(a, Action::Archive { .. });
            for id in &ids {
                if set_archived(conn, *id, on, src)? {
                    changed += 1;
                }
            }
        }
        Action::SetDue { due, .. } => {
            for id in &ids {
                let t = get_ticket(conn, *id)?;
                let Some(f) = list_fields(conn, t.type_id)?.into_iter().find(|f| f.is_due()) else {
                    skipped.push((t.key(), lang.t("Typ ohne Fälligkeit", "type without due date").into()));
                    continue;
                };
                if crate::planning::tickets::set_field(conn, *id, f.id, Value::String(due.clone()), src)?.is_some() {
                    changed += 1;
                }
            }
        }
        Action::Remove { targets } => {
            for t in targets {
                if !target_in_project(conn, project_id, t)? || !target_unused(conn, t)? {
                    if let Some(n) = target_name(conn, t, lang) {
                        skipped.push((n, lang.t("in Benutzung", "in use").into()));
                    }
                    continue;
                }
                use crate::planning::*;
                match t {
                    Target::Field(id) => fields::delete_field(conn, *id, None)?,
                    Target::Status(id) => statuses::delete_status(conn, *id, None)?,
                    Target::Type(id) => types::delete_type(conn, *id, None)?,
                    Target::LinkType(id) => link_types::delete_link_type(conn, *id, None)?,
                    Target::WorkKind(id) => work_kinds::delete_work_kind(conn, *id, None)?,
                    Target::FieldOption { field, option } => {
                        let f = fields::get_field(conn, *field)?;
                        let mut o = f.options.clone();
                        o.values.retain(|v| v.id != *option);
                        fields::update_field(conn, *field, &fields::UpdateField { options: Some(o), ..Default::default() })?;
                    }
                    _ => continue,
                }
                changed += 1;
            }
        }
        Action::MergeLabels { keep, merge } => {
            let merge_set: HashSet<&String> = merge.iter().collect();
            for id in &ids {
                let t = get_ticket(conn, *id)?;
                for f in list_fields(conn, t.type_id)?.into_iter().filter(|f| f.kind == FieldKind::Labels) {
                    let Some(Value::Array(old)) = crate::planning::values::get_value(conn, *id, &f)? else { continue };
                    let mut new: Vec<Value> = Vec::new();
                    for v in &old {
                        let s = v.as_str().unwrap_or_default().to_string();
                        let s = if merge_set.contains(&s) { keep.clone() } else { s };
                        if !new.iter().any(|x| x.as_str() == Some(s.as_str())) {
                            new.push(Value::String(s));
                        }
                    }
                    if new != old && crate::planning::tickets::set_field(conn, *id, f.id, Value::Array(new), src)?.is_some() {
                        changed += 1;
                    }
                }
            }
        }
        Action::DetachParent { .. } => {
            for id in &ids {
                let t = get_ticket(conn, *id)?;
                let Some(pid) = t.parent_id else { continue };
                let exists: Option<i64> = conn.query_row("SELECT id FROM tickets WHERE id = ?1", [pid], |r| r.get(0)).optional()?;
                if exists.is_none() {
                    let batch = history::next_batch(conn)?;
                    crate::planning::tickets::raw_set_parent(conn, *id, None, src, batch)?;
                    changed += 1;
                }
            }
        }
        Action::ClearReference { .. } => {
            for id in &ids {
                changed += clear_refs(conn, project_id, *id, src)?;
            }
        }
        Action::RuleAction { rule, .. } => {
            let r = super::rules::get_rule(conn, *rule)?;
            for id in &ids {
                let did = match &r.action {
                    super::rules::RuleAction::None => false,
                    super::rules::RuleAction::Archive => set_archived(conn, *id, true, src)?,
                    super::rules::RuleAction::Status { status } => force_status(conn, *id, *status, src)?,
                    super::rules::RuleAction::Field { field, value } => {
                        let t = get_ticket(conn, *id)?;
                        if crate::planning::fields::get_field(conn, *field).map(|f| f.type_id) != Ok(t.type_id) {
                            skipped.push((t.key(), lang.t("Feld gehört nicht zum Typ", "field does not belong to the type").into()));
                            false
                        } else {
                            crate::planning::tickets::set_field(conn, *id, *field, value.clone(), src)?.is_some()
                        }
                    }
                };
                if did {
                    changed += 1;
                }
            }
        }
    }
    Ok((changed, skipped))
}

/// Entfernt kaputte Verweise eines Tickets: Verknüpfungen zu fehlenden Tickets, Ticket-Felder mit fehlendem Ziel,
/// gelöschte Auswahlwerte, fehlende Versionen/Meilensteine. Liefert die Zahl der Änderungen.
fn clear_refs(conn: &Connection, project_id: i64, id: i64, src: &Source) -> MRes<usize> {
    let mut n = 0;
    let mut st = conn.prepare("SELECT l.id, l.to_ticket_id FROM links l WHERE l.from_ticket_id = ?1 AND l.to_kind = 'ticket' AND (l.to_ticket_id IS NULL OR l.to_ticket_id NOT IN (SELECT id FROM tickets))")?;
    let broken: Vec<(i64, Option<i64>)> = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (lid, to) in broken {
        conn.execute("DELETE FROM links WHERE id = ?1", [lid])?;
        let batch = history::next_batch(conn)?;
        history::log(conn, id, src, batch, &HistoryField::Other("broken_link".into()), to.map(|t| t.to_string()).as_deref(), None)?;
        n += 1;
    }
    let t = get_ticket(conn, id)?;
    let versions: HashSet<i64> = conn.prepare("SELECT id FROM versions WHERE project_id = ?1")?.query_map([project_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let milestones: HashSet<i64> = conn.prepare("SELECT id FROM milestones WHERE project_id = ?1")?.query_map([project_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let exists = |tid: i64| -> bool { conn.query_row("SELECT 1 FROM tickets WHERE id = ?1", [tid], |_| Ok(())).is_ok() };
    let mut changes: Vec<(i64, Option<Value>)> = Vec::new();
    for f in list_fields(conn, t.type_id)? {
        let Some(v) = crate::planning::values::get_value(conn, id, &f)? else { continue };
        let ok_one = |x: &Value| -> bool {
            match f.kind {
                FieldKind::Dropdown | FieldKind::Multi => x.as_i64().is_some_and(|o| f.options.values.iter().any(|ov| ov.id == o)),
                FieldKind::Ticket => x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())).is_some_and(&exists),
                FieldKind::Version => x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())).is_some_and(|i| versions.contains(&i)),
                FieldKind::Milestone => x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())).is_some_and(|i| milestones.contains(&i)),
                _ => true,
            }
        };
        let new = match &v {
            Value::Array(items) => {
                let kept: Vec<Value> = items.iter().filter(|x| ok_one(x)).cloned().collect();
                if kept.len() == items.len() {
                    continue;
                }
                if kept.is_empty() { None } else { Some(Value::Array(kept)) }
            }
            one => {
                if ok_one(one) {
                    continue;
                }
                None
            }
        };
        changes.push((f.id, new));
    }
    if !changes.is_empty() {
        let batch = history::next_batch(conn)?;
        n += crate::planning::tickets::raw_apply_fields(conn, id, &changes, src, batch)?.len();
    }
    let _ = json!(null);
    Ok(n)
}
