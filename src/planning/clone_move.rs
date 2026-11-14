//! Ticket klonen sowie Typ ändern bzw. in ein anderes Projekt verschieben.
//!
//! **Klonen** ([`clone_preview`], [`clone_ticket`]): pro Feld ein Haken (Standard: alle an), das Klon bekommt einen neuen
//! Schlüssel, den Startstatus des Workflows, das Eltern-Ticket des Originals und – auf Wunsch – Kopien aller Kinder
//! (rekursiv, mit allen Feldern). Kommentare, Verlauf, Zeiten, Verknüpfungen, Markierung und Stern werden nicht kopiert.
//!
//! **Typ ändern / verschieben** ([`change_type_preview`], [`change_type`]): Der Zieltyp kann zum selben oder zu einem anderen
//! Projekt gehören.
//! * Werte wandern gemäß der Zuordnung (Standard: [`suggest_field_map`]) in Felder des Zieltyps mit gleichem Feldtyp;
//!   Dropdown-/Mehrfachauswahl-Werte werden über ihre Bezeichnung übersetzt; passt ein Wert im Zielfeld nicht (Grenzen,
//!   Verweis), geht er verloren. Alles, was verloren geht, nennt die Vorschau mit dem Wert.
//! * Der Status wird zugeordnet: gleicher Status, sonst gleiche Bezeichnung, sonst gleiche Kategorie, sonst Startstatus
//!   (ohne Workflow des Zieltyps: kein Status).
//! * Bei einem Wechsel des Projekts bekommt das Ticket einen neuen Schlüssel im Zielprojekt; der Verlauf hält fest
//!   „Verschoben von PAN-12“. Die Kennung des Tickets bleibt, Verknüpfungen bleiben bestehen.
//! * **Hierarchie:** Das Eltern-Ticket bleibt, wenn die Regeln den neuen Typ darunter erlauben (und das Projekt gleich
//!   bleibt), sonst wird die Verbindung gelöst. Ebenso bleiben Kinder, wenn ihr Typ unter dem neuen Typ erlaubt ist, sonst
//!   werden sie gelöst. Beides steht in Vorschau und Ergebnis, damit die Oberfläche warnen kann.
//! * Pflichtfelder des Zieltyps müssen danach gefüllt sein (Zuordnung, `set_values` oder Standardwert), sonst
//!   `RequiredMissing` – die Oberfläche kann dann genau diese Felder abfragen.

use super::fields::{list_fields, Field};
use super::history::{self, json_text, HistoryField, Source};
use super::integrity::FieldMapEntry;
use super::ops::TicketOp;
use super::projects::get_project;
use super::statuses::{get_status, list_statuses};
use super::ticket_error::{atomic, FieldLabel, TRes, TicketError};
use super::tickets::{clean_title, clear_refs_to, create_ticket_noted, get_ticket, raw_apply_fields, raw_set_parent, touch, NewTicket, Ticket, MAX_TITLE_CHARS};
use super::types::{get_type, may_be_child, suggest_field_map, TicketType};
use super::values::{self, load_values, normalize_value, translate_value, RefCtx};
use super::work_kinds::list_work_kinds;
use super::workflow::{default_status, workflow_statuses};
use super::{common, PlanError};
use crate::state::Lang;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashSet;

// ------------------------------------------------------------------ Klonen

/// Eine Zeile der Klon-Vorschau: das Feld, sein aktueller Wert und ob er übernommen wird.
#[derive(Debug, Clone, PartialEq)]
pub struct ClonePreviewRow {
    pub field: Field,
    pub value: Option<Value>,
    /// Standard: ja.
    pub included: bool,
}

/// Alle Felder des Typs mit den Werten des Originals, damit die Oberfläche je Feld einen Haken anbieten kann.
pub fn clone_preview(conn: &Connection, source_id: i64) -> TRes<Vec<ClonePreviewRow>> {
    let t = get_ticket(conn, source_id)?;
    Ok(values::fields_with_values(conn, t.id, t.type_id)?.into_iter().map(|fv| ClonePreviewRow { field: fv.field, value: fv.value, included: true }).collect())
}

#[derive(Debug, Clone, Default)]
pub struct CloneOptions {
    /// (Feld-Id, übernehmen?). Felder ohne Eintrag werden übernommen; nicht übernommene bekommen ihren Standardwert.
    pub fields: Vec<(i64, bool)>,
    /// Auch alle Kinder (rekursiv) kopieren.
    pub include_children: bool,
    /// Wird an den Titel des Klons gehängt (z. B. „ (copy)“); leer = gleicher Titel.
    pub title_suffix: String,
}

impl CloneOptions {
    /// Alle Felder, ohne Kinder, mit dem üblichen Titelzusatz der Sprache.
    pub fn new(lang: Lang) -> Self {
        CloneOptions { fields: Vec::new(), include_children: false, title_suffix: lang.t(" (Kopie)", " (copy)").to_string() }
    }
}

fn title_with_suffix(title: &str, suffix: &str) -> String {
    let room = MAX_TITLE_CHARS.saturating_sub(suffix.trim_end().chars().count() + 1);
    let t: String = if title.chars().count() > room { title.chars().take(room).collect() } else { title.to_string() };
    format!("{t}{suffix}")
}

/// Klont ein Ticket (siehe Moduldoku). Liefert das neue Ticket und eine Operation, die alle neuen Tickets wieder entfernt.
pub fn clone_ticket(conn: &Connection, source_id: i64, opts: &CloneOptions, source: &Source) -> TRes<(Ticket, TicketOp)> {
    atomic(conn, || {
        let src = get_ticket(conn, source_id)?;
        let fields = list_fields(conn, src.type_id)?;
        let current = load_values(conn, src.id)?;
        let mut values: Vec<(i64, Value)> = Vec::new();
        for f in &fields {
            let wanted = opts.fields.iter().rev().find(|(i, _)| *i == f.id).map(|(_, on)| *on).unwrap_or(true);
            if wanted {
                if let Some(v) = current.get(&f.id) {
                    values.push((f.id, v.clone()));
                }
            }
        }
        let title = clean_title(&title_with_suffix(&src.title, &opts.title_suffix))?;
        let n = NewTicket { project_id: src.project_id, type_id: src.type_id, title, parent_id: src.parent_id, values };
        let (root, op) = create_ticket_noted(conn, &n, source, Some(format!("clone:{}", src.key())))?;
        let mut ops = vec![op];
        if opts.include_children {
            let mut seen: HashSet<i64> = HashSet::from([src.id]);
            clone_children(conn, src.id, root.id, source, &mut seen, &mut ops)?;
        }
        Ok((get_ticket(conn, root.id)?, TicketOp::combine(ops).expect("mindestens das Klon selbst")))
    })
}

fn clone_children(conn: &Connection, from: i64, to: i64, source: &Source, seen: &mut HashSet<i64>, ops: &mut Vec<TicketOp>) -> TRes<()> {
    for child in super::tickets::children_of(conn, from)? {
        if !seen.insert(child.id) {
            continue;
        }
        let vals: Vec<(i64, Value)> = load_values(conn, child.id)?.into_iter().collect();
        let n = NewTicket { project_id: child.project_id, type_id: child.type_id, title: child.title.clone(), parent_id: Some(to), values: vals };
        let (new_child, op) = create_ticket_noted(conn, &n, source, Some(format!("clone:{}", child.key())))?;
        ops.push(op);
        clone_children(conn, child.id, new_child.id, source, seen, ops)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ Typ ändern / verschieben

/// Ein Wert, der in ein Feld des Zieltyps wandert.
#[derive(Debug, Clone, PartialEq)]
pub struct KeptField {
    pub from: Field,
    pub to: Field,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeTypePreview {
    /// Felder des alten Typs mit Wert, die wegfallen (mit dem Wert).
    pub lost: Vec<(Field, Value)>,
    /// Felder des Zieltyps, die keinen übernommenen Wert bekommen (hier lässt sich „Wert direkt setzen“ anbieten).
    pub added: Vec<Field>,
    /// Werte, die übernommen werden.
    pub kept: Vec<KeptField>,
    /// Die verwendete Zuordnung.
    pub mapping: Vec<FieldMapEntry>,
    /// Der Status nach dem Wechsel (`None` = ohne Status).
    pub status: Option<i64>,
    /// Das Ticket wechselt das Projekt.
    pub project_change: bool,
    /// Der Schlüssel nach dem Wechsel des Projekts.
    pub new_key: Option<String>,
    /// Die Verbindung zum Eltern-Ticket wird gelöst.
    pub parent_removed: bool,
    /// Kinder, die ihr Eltern-Ticket verlieren.
    pub detached_children: Vec<i64>,
}

struct Plan {
    ticket: Ticket,
    to: TicketType,
    new_fields: Vec<Field>,
    project_change: bool,
    kept: Vec<KeptField>,
    lost: Vec<(Field, Value)>,
    status: Option<i64>,
    parent_removed: bool,
    detached: Vec<i64>,
    mapping: Vec<FieldMapEntry>,
}

/// Status im Workflow des Zieltyps: gleicher, sonst gleiche Bezeichnung, sonst gleiche Kategorie, sonst Startstatus.
fn map_status(conn: &Connection, t: &Ticket, to: &TicketType) -> TRes<Option<i64>> {
    if !to.workflow {
        return Ok(None);
    }
    let wf = workflow_statuses(conn, to.id)?;
    if wf.is_empty() {
        return Ok(None);
    }
    if let Some(old) = t.status_id {
        if wf.contains(&old) {
            return Ok(Some(old));
        }
        if let Ok(o) = get_status(conn, old) {
            let pool = list_statuses(conn, to.project_id)?;
            let in_wf = |s: &&super::statuses::Status| wf.contains(&s.id);
            if let Some(s) = pool.iter().filter(in_wf).find(|s| common::same_label(o.key.as_deref(), o.name.as_deref(), s.key.as_deref(), s.name.as_deref())) {
                return Ok(Some(s.id));
            }
            if let Some(s) = pool.iter().filter(in_wf).find(|s| s.category == o.category) {
                return Ok(Some(s.id));
            }
        }
    }
    Ok(default_status(conn, to.id)?)
}

fn plan_change(conn: &Connection, ticket_id: i64, to_type: i64, mapping: &[FieldMapEntry]) -> TRes<Plan> {
    let t = get_ticket(conn, ticket_id)?;
    let to = get_type(conn, to_type)?;
    if to.system || get_project(conn, to.project_id)?.system {
        return Err(PlanError::SystemProtected.into());
    }
    let project_change = to.project_id != t.project_id;
    let old_fields = list_fields(conn, t.type_id)?;
    let new_fields = list_fields(conn, to.id)?;
    // Zuordnung prüfen: Quellfeld im alten Typ, Zielfeld im neuen, gleicher Feldtyp, kein Ziel doppelt.
    let mut targets: HashSet<i64> = HashSet::new();
    let mut from_seen: HashSet<i64> = HashSet::new();
    for e in mapping {
        let from = old_fields.iter().find(|f| f.id == e.from_field).ok_or(TicketError::MappingInvalid)?;
        if !from_seen.insert(from.id) {
            return Err(TicketError::MappingInvalid);
        }
        if let Some(tf) = e.to_field {
            let tf = new_fields.iter().find(|f| f.id == tf).ok_or(TicketError::MappingInvalid)?;
            if tf.kind != from.kind || !targets.insert(tf.id) {
                return Err(TicketError::MappingInvalid);
            }
        }
    }
    let ctx = RefCtx { project_id: to.project_id, ticket_id: Some(t.id) };
    let values = load_values(conn, t.id)?;
    let mut kept = Vec::new();
    let mut lost = Vec::new();
    for from in &old_fields {
        let Some(v) = values.get(&from.id) else { continue };
        let target = mapping.iter().find(|e| e.from_field == from.id).and_then(|e| e.to_field).and_then(|id| new_fields.iter().find(|f| f.id == id));
        let moved = target.and_then(|tf| {
            let tv = translate_value(from, tf, v)?;
            let n = normalize_value(conn, &ctx, tf, &tv).ok().flatten()?;
            Some(KeptField { from: from.clone(), to: tf.clone(), value: n })
        });
        match moved {
            Some(k) => kept.push(k),
            None => lost.push((from.clone(), v.clone())),
        }
    }
    let status = map_status(conn, &t, &to)?;
    let parent_removed = match t.parent_id {
        Some(p) => project_change || !may_be_child(conn, to.id, get_ticket(conn, p)?.type_id)?,
        None => false,
    };
    let mut detached = Vec::new();
    for c in super::tickets::children_of(conn, t.id)? {
        if project_change || !may_be_child(conn, c.type_id, to.id)? {
            detached.push(c.id);
        }
    }
    Ok(Plan { ticket: t, to, new_fields, project_change, kept, lost, status, parent_removed, detached, mapping: mapping.to_vec() })
}

/// Vorschau des Typwechsels mit der vorgeschlagenen Zuordnung ([`suggest_field_map`]).
pub fn change_type_preview(conn: &Connection, ticket_id: i64, to_type: i64) -> TRes<ChangeTypePreview> {
    let t = get_ticket(conn, ticket_id)?;
    get_type(conn, to_type)?;
    let mapping = suggest_field_map(conn, t.type_id, to_type)?;
    change_type_preview_with(conn, ticket_id, to_type, &mapping)
}

/// Vorschau mit einer eigenen Zuordnung.
pub fn change_type_preview_with(conn: &Connection, ticket_id: i64, to_type: i64, mapping: &[FieldMapEntry]) -> TRes<ChangeTypePreview> {
    let p = plan_change(conn, ticket_id, to_type, mapping)?;
    let kept_targets: HashSet<i64> = p.kept.iter().map(|k| k.to.id).collect();
    let added = p.new_fields.iter().filter(|f| !kept_targets.contains(&f.id)).cloned().collect();
    let new_key = if p.project_change {
        let proj = get_project(conn, p.to.project_id)?;
        Some(format!("{}-{}", proj.key, proj.next_number))
    } else {
        None
    };
    Ok(ChangeTypePreview { lost: p.lost, added, kept: p.kept, mapping: p.mapping, status: p.status, project_change: p.project_change, new_key, parent_removed: p.parent_removed, detached_children: p.detached })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeTypeReport {
    /// Das Ticket nach dem Wechsel.
    pub ticket: Ticket,
    /// Der Schlüssel vorher (bei einem Wechsel des Projekts ein anderer als nachher).
    pub old_key: String,
    pub lost: Vec<(Field, Value)>,
    pub parent_removed: bool,
    pub detached_children: Vec<i64>,
}

/// Führt den Plan aus. `set_values` sind schon geprüft; mit `enforce_required` werden Pflichtfelder des Zieltyps erzwungen
/// (nicht bei der Migration beim Löschen eines Typs).
fn execute(conn: &Connection, p: Plan, set_values: Vec<(i64, Option<Value>)>, defaults_for_added: bool, enforce_required: bool, source: &Source) -> TRes<ChangeTypeReport> {
    let t = &p.ticket;
    let old_key = t.key();
    let ctx = RefCtx { project_id: p.to.project_id, ticket_id: Some(t.id) };
    // Endgültige Werte im Zieltyp: übernommene, dann gesetzte, dann Standardwerte für den Rest.
    let mut fin: Vec<(Field, Option<Value>)> = Vec::new();
    for f in &p.new_fields {
        let value = if let Some((_, v)) = set_values.iter().rev().find(|(i, _)| *i == f.id) {
            v.clone()
        } else if let Some(k) = p.kept.iter().find(|k| k.to.id == f.id) {
            Some(k.value.clone())
        } else if defaults_for_added {
            f.default_value.as_ref().and_then(|d| normalize_value(conn, &ctx, f, d).ok().flatten())
        } else {
            None
        };
        fin.push((f.clone(), value));
    }
    if enforce_required {
        let missing: Vec<FieldLabel> = fin.iter().filter(|(f, v)| f.required && values::is_empty(v.as_ref())).map(|(f, _)| FieldLabel::of(f)).collect();
        if !missing.is_empty() {
            return Err(TicketError::RequiredMissing(missing));
        }
    }
    let batch = history::next_batch(conn)?;
    // Lösen der Hierarchie (mit Verlauf), solange die alten Verhältnisse gelten.
    if p.parent_removed {
        raw_set_parent(conn, t.id, None, source, batch)?;
    }
    for c in &p.detached {
        raw_set_parent(conn, *c, None, source, batch)?;
    }
    // Verlorene Werte im Verlauf, dann alle Werte des alten Typs entfernen.
    for (f, v) in &p.lost {
        history::log(conn, t.id, source, batch, &HistoryField::Custom(f.id), json_text(Some(v)).as_deref(), None)?;
    }
    conn.execute("DELETE FROM ticket_values WHERE ticket_id = ?1 AND field_id IN (SELECT id FROM fields WHERE type_id = ?2)", [t.id, t.type_id])?;
    // Projekt wechseln: neuer Schlüssel, neuer Rang, Verweise anderer Projekte lösen, Arten der Arbeit zuordnen.
    let (number, rank) = if p.project_change {
        let proj = get_project(conn, p.to.project_id)?;
        conn.execute("UPDATE projects SET next_number = ?1 WHERE id = ?2", [proj.next_number + 1, proj.id])?;
        let rank: i64 = conn.query_row("SELECT COALESCE(MAX(rank) + 1, 0) FROM tickets WHERE project_id = ?1", [proj.id], |r| r.get(0))?;
        remap_work_kinds(conn, t.id, t.project_id, proj.id)?;
        (proj.next_number, rank)
    } else {
        (t.number, t.rank)
    };
    conn.execute(
        "UPDATE tickets SET project_id = ?1, number = ?2, type_id = ?3, status_id = ?4, rank = ?5 WHERE id = ?6",
        params![p.to.project_id, number, p.to.id, p.status, rank, t.id],
    )?;
    history::log(conn, t.id, source, batch, &HistoryField::Type, Some(&t.type_id.to_string()), Some(&p.to.id.to_string()))?;
    if t.status_id != p.status {
        history::log(conn, t.id, source, batch, &HistoryField::Status, t.status_id.map(|s| s.to_string()).as_deref(), p.status.map(|s| s.to_string()).as_deref())?;
    }
    if p.project_change {
        let proj = get_project(conn, p.to.project_id)?;
        history::log(conn, t.id, source, batch, &HistoryField::Moved, Some(&old_key), Some(&format!("{}-{}", proj.key, number)))?;
        clear_refs_to(conn, t.id, Some(p.to.project_id), source, batch)?;
    }
    // Neue Werte schreiben (mit Verlauf).
    let writes: Vec<(i64, Option<Value>)> = fin.into_iter().filter(|(_, v)| v.is_some()).map(|(f, v)| (f.id, v)).collect();
    raw_apply_fields(conn, t.id, &writes, source, batch)?;
    touch(conn, t.id)?;
    Ok(ChangeTypeReport { ticket: get_ticket(conn, t.id)?, old_key, lost: p.lost, parent_removed: p.parent_removed, detached_children: p.detached })
}

/// Zeiteinträge behalten ihre Art der Arbeit, soweit das Zielprojekt eine gleich benannte kennt, sonst entfällt sie.
fn remap_work_kinds(conn: &Connection, ticket_id: i64, from_project: i64, to_project: i64) -> TRes<()> {
    let old = list_work_kinds(conn, from_project)?;
    let new = list_work_kinds(conn, to_project)?;
    for k in old {
        let target = new.iter().find(|n| common::same_label(k.key.as_deref(), k.name.as_deref(), n.key.as_deref(), n.name.as_deref())).map(|n| n.id);
        conn.execute("UPDATE time_entries SET work_kind_id = ?1 WHERE ticket_id = ?2 AND work_kind_id = ?3", params![target, ticket_id, k.id])?;
    }
    Ok(())
}

/// Ändert den Typ eines Tickets bzw. verschiebt es in das Projekt des Zieltyps (siehe Moduldoku). `mapping` ordnet Felder
/// des alten Typs Feldern des Zieltyps zu (Felder ohne Eintrag oder mit `to_field: None` werden verworfen), `set_values`
/// setzt Werte direkt in Felder des Zieltyps. Gleicher Typ: nichts geschieht.
pub fn change_type(conn: &Connection, ticket_id: i64, to_type: i64, mapping: &[FieldMapEntry], set_values: &[(i64, Value)], source: &Source) -> TRes<ChangeTypeReport> {
    atomic(conn, || {
        let t = get_ticket(conn, ticket_id)?;
        if t.type_id == to_type {
            return Ok(ChangeTypeReport { old_key: t.key(), ticket: t, lost: Vec::new(), parent_removed: false, detached_children: Vec::new() });
        }
        let plan = plan_change(conn, ticket_id, to_type, mapping)?;
        let ctx = RefCtx { project_id: plan.to.project_id, ticket_id: Some(t.id) };
        let mut sets: Vec<(i64, Option<Value>)> = Vec::new();
        for (fid, v) in set_values {
            let f = plan.new_fields.iter().find(|f| f.id == *fid).ok_or(TicketError::FieldNotInType { field_id: *fid })?;
            sets.push((*fid, normalize_value(conn, &ctx, f, v)?));
        }
        execute(conn, plan, sets, true, true, source)
    })
}

/// Für das Löschen eines Typs mit Ersatz (`integrity::migrate_tickets_type`): setzt ein Ticket auf den Ersatz-Typ desselben
/// Projekts um – ohne Pflichtfeld-Prüfung (die Tickets sind schon da), mit Standardwerten für neue Felder.
pub(crate) fn migrate_ticket_type(conn: &Connection, ticket_id: i64, to_type: i64, mapping: &[FieldMapEntry], source: &Source) -> TRes<()> {
    let plan = plan_change(conn, ticket_id, to_type, mapping)?;
    execute(conn, plan, Vec::new(), true, false, source).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::comments::add_comment;
    use crate::planning::fields::FieldKind;
    use crate::planning::history::history_of;
    use crate::planning::links::{add_link, links_of, Target};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::{set_field, set_flagged, set_starred, set_parent};
    use crate::planning::time_entries::{entries_of, log_time, tests_now};
    use crate::planning::transition::transition_ticket;
    use crate::planning::types::{create_type, list_types, NewType};
    use serde_json::json;

    fn ui() -> Source {
        Source::ui()
    }

    // ---------------------------------------------------------------- Klonen

    #[test]
    fn preview_lists_all_fields_with_values_and_defaults_to_included() {
        let f = Fx::new();
        let t = f.mk(f.story, "S");
        let sp = f.field(f.story, "story_points");
        set_field(&f.c, t.id, sp.id, json!(5), &ui()).unwrap();
        let rows = clone_preview(&f.c, t.id).unwrap();
        assert_eq!(rows.len(), list_fields(&f.c, f.story).unwrap().len());
        assert!(rows.iter().all(|r| r.included));
        assert_eq!(rows.iter().find(|r| r.field.id == sp.id).unwrap().value, Some(json!(5)));
        let desc = rows.iter().find(|r| r.field.key.as_deref() == Some("description")).unwrap();
        assert_eq!(desc.value, None);
        assert!(clone_preview(&f.c, 9999).unwrap_err().is_not_found());
    }

    #[test]
    fn clone_copies_chosen_fields_gets_new_key_and_start_status() {
        let f = Fx::new();
        let t = f.mk(f.story, "Original");
        let sp = f.field(f.story, "story_points");
        let desc = f.field(f.story, "description");
        let prio = f.field(f.story, "priority");
        set_field(&f.c, t.id, sp.id, json!(5), &ui()).unwrap();
        set_field(&f.c, t.id, desc.id, json!("Text"), &ui()).unwrap();
        set_field(&f.c, t.id, prio.id, json!(1), &ui()).unwrap();
        transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
        set_flagged(&f.c, t.id, true, &ui()).unwrap();
        set_starred(&f.c, t.id, true, &ui()).unwrap();
        add_comment(&f.c, t.id, "Kommentar", &ui()).unwrap();
        log_time(&f.c, t.id, "1h", None, None, "", tests_now(), &ui()).unwrap();
        let mut opts = CloneOptions::new(Lang::En);
        opts.fields = vec![(desc.id, false), (prio.id, false)];
        let (c, op) = clone_ticket(&f.c, t.id, &opts, &Source::agent("A")).unwrap();
        assert_eq!(c.key(), "PAN-2");
        assert_eq!(c.title, "Original (copy)");
        assert_eq!(c.status_id, Some(f.todo), "Startstatus");
        assert!(!c.flagged && !c.starred);
        assert_eq!(values::get_value(&f.c, c.id, &sp).unwrap(), Some(json!(5)), "übernommen");
        assert_eq!(values::get_value(&f.c, c.id, &desc).unwrap(), None, "abgewählt");
        assert_eq!(values::get_value(&f.c, c.id, &prio).unwrap(), Some(json!(3)), "abgewählt = Standardwert");
        assert!(crate::planning::comments::comments_of(&f.c, c.id).unwrap().is_empty());
        assert!(entries_of(&f.c, c.id).unwrap().is_empty());
        let h = history_of(&f.c, c.id).unwrap();
        assert_eq!(h.len(), 1, "nur „angelegt“, kein übernommener Verlauf");
        assert_eq!(h[0].new.as_deref(), Some("clone:PAN-1"));
        assert_eq!(crate::planning::history::describe(&f.c, &h[0], Lang::De), "Als Kopie angelegt von PAN-1");
        assert_eq!(crate::planning::history::describe(&f.c, &h[0], Lang::En), "Created as a copy of PAN-1");
        assert!(matches!(op, TicketOp::Created { .. }));
        // Titelzusatz deutsch / leer / sehr lange Titel
        assert_eq!(clone_ticket(&f.c, t.id, &CloneOptions::new(Lang::De), &ui()).unwrap().0.title, "Original (Kopie)");
        assert_eq!(clone_ticket(&f.c, t.id, &CloneOptions::default(), &ui()).unwrap().0.title, "Original");
        let long = f.mk(f.task, &"x".repeat(255));
        let c = clone_ticket(&f.c, long.id, &CloneOptions::new(Lang::En), &ui()).unwrap().0;
        assert!(c.title.ends_with(" (copy)") && c.title.chars().count() <= 255);
    }

    #[test]
    fn clone_keeps_parent_and_can_include_children_recursively() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "E");
        let story = f.child(&epic, f.story, "S");
        let t1 = f.child(&story, f.task, "T1");
        let t2 = f.child(&story, f.task, "T2");
        f.child(&t1, f.subtask, "Sub");
        let sp = f.field(f.task, "story_points");
        set_field(&f.c, t1.id, sp.id, json!(3), &ui()).unwrap();
        transition_ticket(&f.c, t2.id, f.done, &[], &ui()).unwrap();
        // ohne Kinder
        let (c, _) = clone_ticket(&f.c, story.id, &CloneOptions::new(Lang::En), &ui()).unwrap();
        assert_eq!(c.parent_id, Some(epic.id));
        assert!(crate::planning::tickets::children_of(&f.c, c.id).unwrap().is_empty());
        // mit Kindern
        let mut o = CloneOptions::new(Lang::En);
        o.include_children = true;
        let (c2, op) = clone_ticket(&f.c, story.id, &o, &ui()).unwrap();
        let kids = crate::planning::tickets::children_of(&f.c, c2.id).unwrap();
        assert_eq!(kids.iter().map(|k| k.title.as_str()).collect::<Vec<_>>(), vec!["T1", "T2"], "Kinder behalten ihren Titel");
        assert!(kids.iter().all(|k| k.status_id == Some(f.todo)), "Startstatus auch bei den Kindern");
        assert_eq!(values::get_value(&f.c, kids[0].id, &sp).unwrap(), Some(json!(3)), "Kinder mit allen Feldern");
        let grandkids = crate::planning::tickets::children_of(&f.c, kids[0].id).unwrap();
        assert_eq!(grandkids.len(), 1);
        assert_eq!(grandkids[0].title, "Sub");
        // die Originale sind unverändert
        assert_eq!(crate::planning::tickets::children_of(&f.c, story.id).unwrap().len(), 2);
        // Undo-Operation entfernt alle neuen Tickets wieder
        let before: i64 = f.c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(before, 5 + 1 + 4);
        use crate::undo::Op;
        op.inverse().apply(&f.c).unwrap();
        let after: i64 = f.c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(after, 6);
        assert!(get_ticket(&f.c, c2.id).unwrap_err().is_not_found());
    }

    #[test]
    fn clone_respects_required_fields_when_unchecked() {
        let f = Fx::new();
        let must = f.add_field(f.task, "Ziel", FieldKind::Text, Default::default(), true);
        let t = create_ticket_noted(&f.c, &NewTicket { values: vec![(must.id, json!("x"))], ..NewTicket::new(f.project, f.task, "T") }, &ui(), None).unwrap().0;
        let mut o = CloneOptions::new(Lang::En);
        o.fields = vec![(must.id, false)];
        assert!(matches!(clone_ticket(&f.c, t.id, &o, &ui()), Err(TicketError::RequiredMissing(_))));
        assert_eq!(crate::planning::ticket_list::list_tickets(&f.c, None, &Default::default()).unwrap().total, 1, "nichts entstanden");
        assert!(clone_ticket(&f.c, t.id, &CloneOptions::new(Lang::En), &ui()).is_ok());
    }

    // ---------------------------------------------------------------- Typ ändern

    #[test]
    fn preview_names_lost_added_and_kept_fields() {
        let f = Fx::new();
        let t = f.mk(f.story, "S");
        let sp = f.field(f.story, "story_points");
        let desc = f.field(f.story, "description");
        let epic_field = f.field(f.story, "epic");
        let e = f.mk(f.epic, "E");
        set_field(&f.c, t.id, sp.id, json!(5), &ui()).unwrap();
        set_field(&f.c, t.id, desc.id, json!("Text"), &ui()).unwrap();
        set_field(&f.c, t.id, epic_field.id, json!(e.id), &ui()).unwrap();
        // Story -> Defect: Description und Priority bleiben, Story points und Epic fallen weg, Steps kommt neu
        let p = change_type_preview(&f.c, t.id, f.defect).unwrap();
        let mut lost: Vec<(String, Value)> = p.lost.iter().map(|(fl, v)| (fl.key.clone().unwrap(), v.clone())).collect();
        lost.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(lost, vec![("epic".to_string(), json!(e.id)), ("story_points".to_string(), json!(5))]);
        assert!(p.kept.iter().any(|k| k.from.key.as_deref() == Some("description") && k.value == json!("Text")));
        assert!(p.kept.iter().any(|k| k.to.key.as_deref() == Some("priority") && k.value == json!(3)));
        assert_eq!(p.added.iter().map(|f| f.key.clone().unwrap()).collect::<Vec<_>>(), vec!["steps"]);
        assert_eq!(p.status, Some(f.todo));
        assert!(!p.project_change && p.new_key.is_none());
        assert!(!p.parent_removed && p.detached_children.is_empty());
        assert_eq!(p.mapping.len(), list_fields(&f.c, f.story).unwrap().len());
        // die Vorschau ändert nichts
        assert_eq!(get_ticket(&f.c, t.id).unwrap().type_id, f.story);
    }

    #[test]
    fn change_type_moves_mapped_values_and_logs() {
        let f = Fx::new();
        let t = f.mk(f.story, "S");
        let desc = f.field(f.story, "description");
        let steps = f.field(f.defect, "steps");
        let prio_new = f.field(f.defect, "priority");
        let sp = f.field(f.story, "story_points");
        set_field(&f.c, t.id, desc.id, json!("Text"), &ui()).unwrap();
        set_field(&f.c, t.id, sp.id, json!(8), &ui()).unwrap();
        let mapping = suggest_field_map(&f.c, f.story, f.defect).unwrap();
        let r = change_type(&f.c, t.id, f.defect, &mapping, &[(steps.id, json!("1. Klick"))], &Source::agent("S")).unwrap();
        assert_eq!(r.ticket.type_id, f.defect);
        assert_eq!(r.ticket.key(), "PAN-1", "gleiches Projekt = gleicher Schlüssel");
        assert_eq!(r.lost.len(), 1);
        assert_eq!(r.lost[0].0.key.as_deref(), Some("story_points"));
        let desc_new = f.field(f.defect, "description");
        assert_eq!(values::get_value(&f.c, t.id, &desc_new).unwrap(), Some(json!("Text")));
        assert_eq!(values::get_value(&f.c, t.id, &steps).unwrap(), Some(json!("1. Klick")));
        assert_eq!(values::get_value(&f.c, t.id, &prio_new).unwrap(), Some(json!(3)));
        assert!(values::load_values(&f.c, t.id).unwrap().keys().all(|k| list_fields(&f.c, f.defect).unwrap().iter().any(|x| x.id == *k)), "keine Werte des alten Typs übrig");
        let h = history_of(&f.c, t.id).unwrap();
        let ty = h.iter().find(|e| e.field == HistoryField::Type).unwrap();
        assert_eq!((ty.old.clone(), ty.new.clone()), (Some(f.story.to_string()), Some(f.defect.to_string())));
        assert_eq!(ty.source.kind, crate::planning::history::SourceKind::Agent);
        assert!(h.iter().any(|e| e.field == HistoryField::Custom(sp.id) && e.old.as_deref() == Some("8") && e.new.is_none()), "verlorener Wert im Verlauf");
        assert!(h.iter().any(|e| e.field == HistoryField::Custom(steps.id)));
        // gleicher Typ: nichts passiert
        let again = change_type(&f.c, t.id, f.defect, &[], &[], &ui()).unwrap();
        assert!(again.lost.is_empty());
    }

    #[test]
    fn change_type_validates_mapping_set_values_and_required_fields() {
        let f = Fx::new();
        let t = f.mk(f.story, "S");
        let desc = f.field(f.story, "description");
        let sp = f.field(f.story, "story_points");
        let steps = f.field(f.defect, "steps");
        let prio_d = f.field(f.defect, "priority");
        let bad = |m: Vec<FieldMapEntry>| change_type(&f.c, t.id, f.defect, &m, &[], &ui()).unwrap_err();
        // Feldtyp passt nicht (Rich -> Dropdown)
        assert_eq!(bad(vec![FieldMapEntry { from_field: desc.id, to_field: Some(prio_d.id) }]), TicketError::MappingInvalid);
        // Zielfeld doppelt
        let prio = f.field(f.story, "priority");
        assert_eq!(bad(vec![FieldMapEntry { from_field: prio.id, to_field: Some(prio_d.id) }, FieldMapEntry { from_field: prio.id, to_field: Some(prio_d.id) }]), TicketError::MappingInvalid);
        // Quellfeld gehört nicht zum Typ / Zielfeld nicht zum Zieltyp
        assert_eq!(bad(vec![FieldMapEntry { from_field: steps.id, to_field: None }]), TicketError::MappingInvalid);
        assert_eq!(bad(vec![FieldMapEntry { from_field: desc.id, to_field: Some(sp.id) }]), TicketError::MappingInvalid);
        // set_values: nur Felder des Zieltyps, Werte werden geprüft
        assert_eq!(change_type(&f.c, t.id, f.defect, &[], &[(sp.id, json!(1))], &ui()).unwrap_err(), TicketError::FieldNotInType { field_id: sp.id });
        assert!(matches!(change_type(&f.c, t.id, f.defect, &[], &[(prio_d.id, json!(99))], &ui()), Err(TicketError::ValueInvalid(_))));
        // Pflichtfeld des Zieltyps
        let must = f.add_field(f.defect, "Schweregrad", FieldKind::Text, Default::default(), true);
        match change_type(&f.c, t.id, f.defect, &[], &[], &ui()) {
            Err(TicketError::RequiredMissing(v)) => assert_eq!(v[0].id, must.id),
            other => panic!("{other:?}"),
        }
        assert_eq!(get_ticket(&f.c, t.id).unwrap().type_id, f.story, "nichts geändert");
        assert!(change_type(&f.c, t.id, f.defect, &[], &[(must.id, json!("hoch"))], &ui()).is_ok());
        // ohne Zuordnung gehen Werte verloren
        // System-Typ / unbekannter Typ
        let todo_proj = crate::planning::projects::project_by_key(&f.c, "TODO").unwrap();
        let todo_type = list_types(&f.c, todo_proj.id).unwrap()[0].id;
        let t2 = f.mk(f.task, "T2");
        assert_eq!(change_type(&f.c, t2.id, todo_type, &[], &[], &ui()).unwrap_err(), TicketError::Plan(PlanError::SystemProtected));
        assert!(change_type(&f.c, t2.id, 9999, &[], &[], &ui()).unwrap_err().is_not_found());
    }

    #[test]
    fn dropdown_values_are_translated_by_label_and_unfit_values_are_lost() {
        let f = Fx::new();
        let a = create_type(&f.c, &NewType { project_id: f.project, key: None, name: Some("A".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let b = create_type(&f.c, &NewType { project_id: f.project, key: None, name: Some("B".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let da = f.add_field(a, "Farbe", FieldKind::Dropdown, Fx::dropdown(&["Rot", "Grün", "Blau"]), false);
        let db = f.add_field(b, "Farbe", FieldKind::Dropdown, Fx::dropdown(&["blau", "rot"]), false);
        let na = f.add_field(a, "Menge", FieldKind::Number, Default::default(), false);
        let nb = f.add_field(b, "Menge", FieldKind::Number, crate::planning::fields::FieldOptions { max: Some(10.0), ..Default::default() }, false);
        let t = f.mk(a, "T");
        set_field(&f.c, t.id, da.id, json!(3), &ui()).unwrap(); // Blau
        set_field(&f.c, t.id, na.id, json!(50), &ui()).unwrap(); // zu groß für das Ziel
        let p = change_type_preview(&f.c, t.id, b).unwrap();
        assert_eq!(p.kept.len(), 1);
        assert_eq!(p.kept[0].value, json!(1), "Blau = Kennung 1 im Ziel");
        assert_eq!(p.lost.len(), 1, "50 passt nicht zu max 10");
        assert_eq!(p.lost[0].1, json!(50));
        change_type(&f.c, t.id, b, &p.mapping, &[], &ui()).unwrap();
        assert_eq!(values::get_value(&f.c, t.id, &db).unwrap(), Some(json!(1)));
        assert_eq!(values::get_value(&f.c, t.id, &nb).unwrap(), None);
        // Wert ohne Entsprechung geht verloren
        let t2 = f.mk(a, "T2");
        set_field(&f.c, t2.id, da.id, json!(2), &ui()).unwrap(); // Grün
        let p = change_type_preview(&f.c, t2.id, b).unwrap();
        assert!(p.kept.is_empty() && p.lost.len() == 1 && p.added.len() == 2);
    }

    #[test]
    fn status_is_mapped_by_id_label_category_or_start() {
        let f = Fx::new();
        // gleicher Typ-Workflow: Status bleibt
        let t = f.mk(f.task, "T");
        transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
        change_type(&f.c, t.id, f.defect, &[], &[], &ui()).unwrap();
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, Some(f.progress));
        // Zieltyp ohne Workflow: kein Status
        let plain = create_type(&f.c, &NewType { project_id: f.project, key: None, name: Some("Notiz".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        change_type(&f.c, t.id, plain, &[], &[], &ui()).unwrap();
        assert_eq!(get_ticket(&f.c, t.id).unwrap().status_id, None);
        // Typ mit eigenem Workflow (andere Status): gleiche Kategorie, sonst Startstatus
        let other_wf = create_type(&f.c, &NewType { project_id: f.project, key: None, name: Some("Anders".into()), icon: None, color: "#445566".into(), workflow: true }).unwrap();
        let mk_status = |name: &str, cat| crate::planning::statuses::create_status(&f.c, &crate::planning::statuses::NewStatus { project_id: f.project, key: None, name: Some(name.into()), color: "#445566".into(), category: cat }).unwrap();
        use crate::planning::statuses::StatusCategory as C;
        let (backlog, doing, finished) = (mk_status("Backlog", C::Todo), mk_status("Doing", C::Progress), mk_status("Finished", C::Done));
        crate::planning::workflow::set_workflow_statuses(&f.c, other_wf, &[backlog, doing, finished]).unwrap();
        let u = f.mk(f.task, "U");
        transition_ticket(&f.c, u.id, f.done, &[], &ui()).unwrap();
        let p = change_type_preview(&f.c, u.id, other_wf).unwrap();
        assert_eq!(p.status, Some(finished), "Done -> Done (Kategorie)");
        let w = f.mk(f.task, "W");
        change_type(&f.c, w.id, other_wf, &[], &[], &ui()).unwrap();
        assert_eq!(get_ticket(&f.c, w.id).unwrap().status_id, Some(backlog), "To Do -> Kategorie To Do");
        // gleiche Bezeichnung hat Vorrang vor der Kategorie
        let same_name = create_type(&f.c, &NewType { project_id: f.project, key: None, name: Some("Gleich".into()), icon: None, color: "#445566".into(), workflow: true }).unwrap();
        let extra = mk_status("Extra", C::Progress);
        crate::planning::workflow::set_workflow_statuses(&f.c, same_name, &[extra, backlog, doing]).unwrap();
        let x = f.mk(f.task, "X");
        transition_ticket(&f.c, x.id, f.progress, &[], &ui()).unwrap();
        let p = change_type_preview(&f.c, x.id, same_name).unwrap();
        assert_eq!(p.status, Some(doing), "Kategorie Progress, erster passender");
        let status_h: Vec<_> = history_of(&f.c, w.id).unwrap().into_iter().filter(|e| e.field == HistoryField::Status).collect();
        assert_eq!(status_h.len(), 1);
    }

    #[test]
    fn parent_and_children_are_kept_when_rules_allow_and_detached_otherwise() {
        let f = Fx::new();
        let epic = f.mk(f.epic, "E");
        let story = f.child(&epic, f.story, "S");
        let task = f.child(&story, f.task, "T");
        let defect = f.child(&story, f.defect, "D");
        // Task -> Defect: Eltern-Regel (Story) passt, Kinder (Subtask) dürfen unter Defect
        let sub = f.child(&task, f.subtask, "Sub");
        let p = change_type_preview(&f.c, task.id, f.defect).unwrap();
        assert!(!p.parent_removed && p.detached_children.is_empty());
        let r = change_type(&f.c, task.id, f.defect, &[], &[], &ui()).unwrap();
        assert_eq!(r.ticket.parent_id, Some(story.id));
        assert_eq!(get_ticket(&f.c, sub.id).unwrap().parent_id, Some(task.id));
        // Defect -> Epic: Eltern-Regel verletzt (Epic hat keinen Elterntyp), Kinder (Subtask) nicht unter Epic
        let p = change_type_preview(&f.c, defect.id, f.epic).unwrap();
        assert!(p.parent_removed);
        let r = change_type(&f.c, defect.id, f.epic, &[], &[], &ui()).unwrap();
        assert!(r.parent_removed);
        assert_eq!(get_ticket(&f.c, defect.id).unwrap().parent_id, None);
        // Story -> Epic: die Kinder Task/Defect dürfen unter Epic, Eltern-Regel verletzt
        let p = change_type_preview(&f.c, story.id, f.epic).unwrap();
        assert!(p.parent_removed);
        assert!(p.detached_children.is_empty(), "Devtask darf unter Epic");
        // Story -> Subtask: Kinder (Defect-Typ jetzt Task) passen nicht
        let p = change_type_preview(&f.c, story.id, f.subtask).unwrap();
        assert_eq!(p.detached_children, vec![task.id]);
        let r = change_type(&f.c, story.id, f.subtask, &[], &[], &ui()).unwrap();
        assert_eq!(r.detached_children, vec![task.id]);
        assert_eq!(get_ticket(&f.c, task.id).unwrap().parent_id, None);
        let h = history_of(&f.c, task.id).unwrap();
        assert!(h.iter().any(|e| e.field == HistoryField::Parent && e.new.is_none()));
    }

    #[test]
    fn moving_to_another_project_gives_a_new_key_and_keeps_links_and_data() {
        let f = Fx::new();
        let other = f.other_project("OTH");
        let o_types = list_types(&f.c, other).unwrap();
        let o_task = o_types.iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
        let a = f.mk(f.story, "A");
        let t = f.mk(f.task, "T");
        let epic = f.mk(f.epic, "E");
        set_parent(&f.c, a.id, Some(epic.id), &ui()).unwrap();
        let desc = f.field(f.task, "description");
        set_field(&f.c, t.id, desc.id, json!("Beschreibung"), &ui()).unwrap();
        let sp = f.field(f.task, "story_points");
        set_field(&f.c, t.id, sp.id, json!(3), &ui()).unwrap();
        transition_ticket(&f.c, t.id, f.progress, &[], &ui()).unwrap();
        add_link(&f.c, t.id, crate::planning::link_types::link_type_by_key(&f.c, "blocks").unwrap().id, Target::Ticket(a.id), &ui()).unwrap();
        let kinds = list_work_kinds(&f.c, f.project).unwrap();
        log_time(&f.c, t.id, "1h", None, Some(kinds[1].id), "", tests_now(), &ui()).unwrap();
        // ein Epic-Feld eines anderen Tickets verweist auf T? (T ist ein Task, daher Story-Ticket-Feld nicht möglich) – Verweis-Test unten
        let prev = change_type_preview(&f.c, t.id, o_task).unwrap();
        assert!(prev.project_change);
        assert_eq!(prev.new_key.as_deref(), Some("OTH-1"));
        let mapping = prev.mapping.clone();
        let r = change_type(&f.c, t.id, o_task, &mapping, &[], &Source::agent("X")).unwrap();
        assert_eq!(r.old_key, "PAN-2");
        assert_eq!(r.ticket.key(), "OTH-1");
        assert_eq!(r.ticket.id, t.id, "gleiche Kennung");
        assert_eq!(r.ticket.project_id, other);
        let o_desc = list_fields(&f.c, o_task).unwrap().into_iter().find(|f| f.key.as_deref() == Some("description")).unwrap();
        assert_eq!(values::get_value(&f.c, t.id, &o_desc).unwrap(), Some(json!("Beschreibung")));
        let o_status = list_statuses(&f.c, other).unwrap();
        let st = get_status(&f.c, r.ticket.status_id.unwrap()).unwrap();
        assert_eq!(st.project_id, other);
        assert_eq!(st.key.as_deref(), Some("progress"), "gleicher Status im Zielprojekt");
        assert!(o_status.iter().any(|s| s.id == st.id));
        // Verknüpfung bleibt
        assert_eq!(links_of(&f.c, t.id, Lang::En).unwrap().len(), 1);
        // Verlauf: Verschoben von PAN-2
        let h = history_of(&f.c, t.id).unwrap();
        let mv = h.iter().find(|e| e.field == HistoryField::Moved).unwrap();
        assert_eq!((mv.old.as_deref(), mv.new.as_deref()), (Some("PAN-2"), Some("OTH-1")));
        assert_eq!(crate::planning::history::describe(&f.c, mv, Lang::En), "Moved from PAN-2");
        assert_eq!(crate::planning::history::describe(&f.c, mv, Lang::De), "Verschoben von PAN-2");
        // Zeiteintrag behält die Art der Arbeit gleichen Namens
        let e = &entries_of(&f.c, t.id).unwrap()[0];
        let o_kinds = list_work_kinds(&f.c, other).unwrap();
        assert_eq!(e.work_kind_id, Some(o_kinds[1].id));
        // der Zähler des Quellprojekts läuft weiter, der Schlüssel PAN-2 wird nie wieder vergeben
        assert_eq!(f.mk(f.task, "N").key(), "PAN-4");
        // Rang am Ende des Zielprojekts, nächstes Ticket im Zielprojekt bekommt OTH-2
        let n = create_ticket_noted(&f.c, &NewTicket::new(other, o_task, "N2"), &ui(), None).unwrap().0;
        assert_eq!((n.key().as_str(), n.rank), ("OTH-2", 1));
    }

    #[test]
    fn moving_detaches_parent_and_children_and_clears_foreign_references() {
        let f = Fx::new();
        let other = f.other_project("OTH");
        let o_story = list_types(&f.c, other).unwrap().iter().find(|t| t.key.as_deref() == Some("userstory")).unwrap().id;
        let epic = f.mk(f.epic, "E");
        let story = f.child(&epic, f.story, "S");
        let task = f.child(&story, f.task, "T");
        let epic_field = f.field(f.story, "epic");
        let other_story = f.mk(f.story, "S2");
        set_field(&f.c, other_story.id, epic_field.id, json!(epic.id), &ui()).unwrap();
        let r = change_type(&f.c, story.id, o_story, &[], &[], &ui()).unwrap();
        assert!(r.parent_removed);
        assert_eq!(r.detached_children, vec![task.id]);
        assert_eq!(get_ticket(&f.c, story.id).unwrap().parent_id, None);
        assert_eq!(get_ticket(&f.c, task.id).unwrap().parent_id, None);
        // Verweise von Tickets des alten Projekts auf das verschobene Ticket werden geleert
        let epic_moved = change_type(&f.c, epic.id, list_types(&f.c, other).unwrap().iter().find(|t| t.key.as_deref() == Some("epic")).unwrap().id, &[], &[], &ui()).unwrap();
        assert_eq!(epic_moved.ticket.project_id, other);
        assert_eq!(values::get_value(&f.c, other_story.id, &epic_field).unwrap(), None);
    }

    #[test]
    fn moving_needs_required_fields_of_the_target() {
        let f = Fx::new();
        let other = f.other_project("OTH");
        let o_task = list_types(&f.c, other).unwrap().iter().find(|t| t.key.as_deref() == Some("devtask")).unwrap().id;
        let must = f.add_field(o_task, "Kunde", FieldKind::Text, Default::default(), true);
        let t = f.mk(f.task, "T");
        assert!(matches!(change_type(&f.c, t.id, o_task, &[], &[], &ui()), Err(TicketError::RequiredMissing(v)) if v[0].id == must.id));
        assert_eq!(get_ticket(&f.c, t.id).unwrap().project_id, f.project);
        let n: i64 = f.c.query_row("SELECT next_number FROM projects WHERE id = ?1", [other], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "Fehlschlag verbraucht keine Nummer");
        change_type(&f.c, t.id, o_task, &[], &[(must.id, json!("ACME"))], &ui()).unwrap();
        assert_eq!(get_ticket(&f.c, t.id).unwrap().key(), "OTH-1");
    }
}
