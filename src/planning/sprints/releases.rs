//! Versionen und Meilensteine je Projekt: Verwaltung (Name, Beschreibung, Fälligkeit, erschienen/erreicht, Reihenfolge),
//! Fortschritt aus den Tickets und Löschen mit der Integritätsregel (`planning/integrity.rs`).
//!
//! Beide sind gleich gebaut und teilen sich den Code über [`ReleaseKind`]; für den Alltag gibt es dünne Namen (`create_version`,
//! `create_milestone` …). Felder vom Typ „Version“ bzw. „Meilenstein“ verweisen mit der Kennung auf einen Eintrag **desselben Projekts**
//! (geprüft in `values::normalize_value`, Hilfe: [`exists_in_project`]). Fortschritt: die Tickets, deren Feld auf den Eintrag zeigt,
//! nach Status-Kategorie (To Do / In Arbeit / Erledigt) und als Anteil nach [`Basis`].
//!
//! Beide hängen am Projektschalter `use_versions`.

use super::sprint::points_sql;
use super::{atomic, ensure_name_free, fmt_date, parse_date, release_project, SRes, SprintError, MAX_DESCRIPTION_CHARS};
use crate::calc::progress::{parent_progress, Basis, Progress};
use crate::planning::common;
use crate::planning::integrity::{self, DeleteMode};
use crate::planning::statuses::StatusCategory;
use crate::planning::tickets::{self, Ticket};
use crate::planning::PlanError;
use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};

/// Version oder Meilenstein.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseKind {
    Version,
    Milestone,
}

impl ReleaseKind {
    fn table(self) -> &'static str {
        match self {
            ReleaseKind::Version => "versions",
            ReleaseKind::Milestone => "milestones",
        }
    }

    fn done_col(self) -> &'static str {
        match self {
            ReleaseKind::Version => "released",
            ReleaseKind::Milestone => "reached",
        }
    }

    fn done_at_col(self) -> &'static str {
        match self {
            ReleaseKind::Version => "released_at",
            ReleaseKind::Milestone => "reached_at",
        }
    }

    /// Art der Felder (`fields.kind`), die auf diese Einträge zeigen.
    pub fn field_kind(self) -> &'static str {
        match self {
            ReleaseKind::Version => "version",
            ReleaseKind::Milestone => "milestone",
        }
    }

    /// Bezeichnung der Art in beiden Sprachen (Einzahl).
    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            ReleaseKind::Version => lang.t("Version", "Version"),
            ReleaseKind::Milestone => lang.t("Meilenstein", "Milestone"),
        }
    }

    /// Bezeichnung des Zustands „fertig“: erschienen bzw. erreicht (und das Gegenteil).
    pub fn done_label(self, done: bool, lang: crate::state::Lang) -> &'static str {
        match (self, done) {
            (ReleaseKind::Version, true) => lang.t("Erschienen", "Released"),
            (ReleaseKind::Version, false) => lang.t("Nicht erschienen", "Unreleased"),
            (ReleaseKind::Milestone, true) => lang.t("Erreicht", "Reached"),
            (ReleaseKind::Milestone, false) => lang.t("Offen", "Open"),
        }
    }
}

/// Eine Version oder ein Meilenstein.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub id: i64,
    pub project_id: i64,
    pub kind: ReleaseKind,
    pub name: String,
    pub description: String,
    pub due_date: Option<NaiveDate>,
    /// Version: erschienen; Meilenstein: erreicht.
    pub done: bool,
    /// Wann `done` gesetzt wurde.
    pub done_at: Option<String>,
    pub position: i64,
}

impl Release {
    /// Überfällig: fällig vor `today` und noch nicht erschienen/erreicht.
    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        !self.done && self.due_date.is_some_and(|d| d < today)
    }
}

pub type Version = Release;
pub type Milestone = Release;

fn cols(kind: ReleaseKind) -> String {
    format!("id, project_id, name, description, due_date, {}, {}, position", kind.done_col(), kind.done_at_col())
}

fn row_to(kind: ReleaseKind) -> impl Fn(&rusqlite::Row) -> rusqlite::Result<Release> {
    move |r| {
        Ok(Release {
            id: r.get(0)?,
            project_id: r.get(1)?,
            kind,
            name: r.get(2)?,
            description: r.get(3)?,
            due_date: r.get::<_, Option<String>>(4)?.and_then(|s| parse_date(&s).ok()),
            done: r.get::<_, i64>(5)? != 0,
            done_at: r.get(6)?,
            position: r.get(7)?,
        })
    }
}

/// Ein Eintrag (auch wenn das Projekt keine Versionen mehr nutzt – so lassen sich vorhandene Feldwerte weiter anzeigen).
pub fn get_release(conn: &Connection, kind: ReleaseKind, id: i64) -> SRes<Release> {
    conn.query_row(&format!("SELECT {} FROM {} WHERE id = ?1", cols(kind), kind.table()), [id], row_to(kind)).optional()?.ok_or_else(SprintError::not_found)
}

/// Alle Einträge des Projekts in der eingestellten Reihenfolge.
pub fn list_releases(conn: &Connection, kind: ReleaseKind, project_id: i64) -> SRes<Vec<Release>> {
    release_project(conn, project_id)?;
    let mut st = conn.prepare(&format!("SELECT {} FROM {} WHERE project_id = ?1 ORDER BY position, id", cols(kind), kind.table()))?;
    Ok(st.query_map([project_id], row_to(kind))?.collect::<Result<Vec<_>, _>>()?)
}

/// Gehört die Kennung zu einem Eintrag dieser Art im Projekt? (Prüfung der Feldwerte.)
pub(crate) fn exists_in_project(conn: &Connection, kind: ReleaseKind, id: i64, project_id: i64) -> rusqlite::Result<bool> {
    Ok(conn.query_row(&format!("SELECT 1 FROM {} WHERE id = ?1 AND project_id = ?2", kind.table()), [id, project_id], |_| Ok(())).optional()?.is_some())
}

#[derive(Debug, Clone)]
pub struct NewRelease {
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub due_date: Option<NaiveDate>,
}

impl NewRelease {
    pub fn new(project_id: i64, name: &str) -> Self {
        NewRelease { project_id, name: name.to_string(), description: String::new(), due_date: None }
    }
}

/// Änderungen; `None` = unverändert (`due_date: Some(None)` löscht die Fälligkeit).
#[derive(Debug, Clone, Default)]
pub struct UpdateRelease {
    pub name: Option<String>,
    pub description: Option<String>,
    pub due_date: Option<Option<NaiveDate>>,
}

fn clean_description(s: &str) -> SRes<String> {
    let d = s.trim().to_string();
    if d.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(SprintError::TextTooLong { max: MAX_DESCRIPTION_CHARS });
    }
    Ok(d)
}

/// Legt einen Eintrag am Ende der Reihenfolge an. Der Name muss im Projekt (ohne Beachtung der Groß-/Kleinschreibung) eindeutig sein.
pub fn create_release(conn: &Connection, kind: ReleaseKind, n: &NewRelease) -> SRes<i64> {
    release_project(conn, n.project_id)?;
    let name = common::clean_name(&n.name)?;
    ensure_name_free(conn, kind.table(), n.project_id, &name, None)?;
    let description = clean_description(&n.description)?;
    let position = common::next_position(conn, kind.table(), "project_id = ?1", &[n.project_id])?;
    conn.execute(
        &format!("INSERT INTO {} (project_id, name, description, due_date, position) VALUES (?1, ?2, ?3, ?4, ?5)", kind.table()),
        params![n.project_id, name, description, n.due_date.map(fmt_date), position],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_release(conn: &Connection, kind: ReleaseKind, id: i64, u: &UpdateRelease) -> SRes<()> {
    let cur = get_release(conn, kind, id)?;
    release_project(conn, cur.project_id)?;
    let name = match &u.name {
        Some(n) => {
            let n = common::clean_name(n)?;
            ensure_name_free(conn, kind.table(), cur.project_id, &n, Some(id))?;
            n
        }
        None => cur.name,
    };
    let description = match &u.description {
        Some(d) => clean_description(d)?,
        None => cur.description,
    };
    let due = u.due_date.unwrap_or(cur.due_date);
    conn.execute(&format!("UPDATE {} SET name = ?1, description = ?2, due_date = ?3 WHERE id = ?4", kind.table()), params![name, description, due.map(fmt_date), id])?;
    Ok(())
}

/// Neue Reihenfolge; `ids` muss genau die Einträge des Projekts enthalten (jeder einmal).
pub fn reorder_releases(conn: &Connection, kind: ReleaseKind, project_id: i64, ids: &[i64]) -> SRes<()> {
    release_project(conn, project_id)?;
    Ok(common::reorder(conn, kind.table(), "project_id = ?1", &[project_id], ids)?)
}

/// Setzt „erschienen“ (Version) bzw. „erreicht“ (Meilenstein) oder nimmt es zurück. Ein unveränderter Zustand lässt den Zeitpunkt stehen.
pub fn set_release_done(conn: &Connection, kind: ReleaseKind, id: i64, done: bool) -> SRes<()> {
    let cur = get_release(conn, kind, id)?;
    release_project(conn, cur.project_id)?;
    if cur.done == done {
        return Ok(());
    }
    let at = if done { Some(super::clock::now()) } else { None };
    conn.execute(&format!("UPDATE {} SET {} = ?1, {} = ?2 WHERE id = ?3", kind.table(), kind.done_col(), kind.done_at_col()), params![done as i64, at, id])?;
    Ok(())
}

/// Wie viele Tickets mit ihrem Feld auf den Eintrag zeigen.
pub fn release_usage(conn: &Connection, kind: ReleaseKind, id: i64) -> SRes<i64> {
    get_release(conn, kind, id)?;
    Ok(match kind {
        ReleaseKind::Version => integrity::usage_count_version(conn, id)?,
        ReleaseKind::Milestone => integrity::usage_count_milestone(conn, id)?,
    })
}

/// Löscht einen Eintrag. Zeigen Tickets darauf, ist ein `mode` nötig (sonst `InUse`): `Migrate { to }` hängt die Feldwerte auf einen
/// anderen Eintrag **derselben Art im selben Projekt** um, `Discard` leert die Felder.
pub fn delete_release(conn: &Connection, kind: ReleaseKind, id: i64, mode: Option<DeleteMode>) -> SRes<()> {
    let cur = get_release(conn, kind, id)?;
    release_project(conn, cur.project_id)?;
    let count = release_usage(conn, kind, id)?;
    integrity::require_mode(count, mode.is_some())?;
    let to = match &mode {
        Some(DeleteMode::Migrate { to }) => {
            let t = get_release(conn, kind, *to).map_err(|_| SprintError::Plan(PlanError::ReplacementInvalid))?;
            if t.id == id || t.project_id != cur.project_id {
                return Err(PlanError::ReplacementInvalid.into());
            }
            Some(t.id)
        }
        _ => None,
    };
    atomic(conn, || {
        integrity::migrate_values_release(conn, kind.field_kind(), id, to)?;
        crate::planning::workflow::scrub_release_refs(conn, kind.field_kind(), id, to)?;
        conn.execute(&format!("DELETE FROM {} WHERE id = ?1", kind.table()), [id])?;
        Ok(())
    })
}

/// Die Tickets, deren Feld auf den Eintrag zeigt, nach `rank`.
pub fn release_tickets(conn: &Connection, kind: ReleaseKind, id: i64) -> SRes<Vec<Ticket>> {
    let cur = get_release(conn, kind, id)?;
    release_project(conn, cur.project_id)?;
    let sql = format!("SELECT {} {} WHERE t.project_id = ?1 AND t.id IN ({}) ORDER BY t.rank, t.id", tickets::COLS, tickets::FROM, referencing_sql());
    let mut st = conn.prepare(&sql)?;
    Ok(st.query_map(params![cur.project_id, kind.field_kind(), id], tickets::from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Unterabfrage: Kennungen der Tickets mit Feldern der Art `?2`, die auf `?3` zeigen.
fn referencing_sql() -> &'static str {
    "SELECT v.ticket_id FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE f.kind = ?2 AND v.text = CAST(?3 AS TEXT)"
}

/// Fortschritt eines Eintrags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReleaseProgress {
    /// Tickets der Status-Kategorie „To Do“ (auch ohne Status).
    pub todo: u32,
    /// … „In Arbeit“.
    pub in_progress: u32,
    /// … „Erledigt“.
    pub done: u32,
    /// Anteil der erledigten Tickets nach der gewählten [`Basis`] (Punkte oder Anzahl).
    pub progress: Progress,
}

impl ReleaseProgress {
    pub fn total(&self) -> u32 {
        self.todo + self.in_progress + self.done
    }
}

/// Fortschritt nach Status-Kategorie und als Anteil (`basis`: Punkte aus `story_points`, Anzahl oder Punkte mit „ohne = 1“).
pub fn release_progress(conn: &Connection, kind: ReleaseKind, id: i64, basis: Basis) -> SRes<ReleaseProgress> {
    let cur = get_release(conn, kind, id)?;
    release_project(conn, cur.project_id)?;
    let sql = format!("SELECT s.category, {} FROM tickets t LEFT JOIN statuses s ON s.id = t.status_id WHERE t.project_id = ?1 AND t.id IN ({})", points_sql(), referencing_sql());
    let mut st = conn.prepare(&sql)?;
    let rows = st
        .query_map(params![cur.project_id, kind.field_kind(), id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<f64>>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let (mut todo, mut in_progress, mut done) = (0, 0, 0);
    let mut children = Vec::with_capacity(rows.len());
    for (cat, points) in rows {
        let cat = cat.as_deref().map(StatusCategory::parse);
        match cat {
            Some(StatusCategory::Done) => done += 1,
            Some(StatusCategory::Progress) => in_progress += 1,
            _ => todo += 1,
        }
        children.push((cat == Some(StatusCategory::Done), points));
    }
    Ok(ReleaseProgress { todo, in_progress, done, progress: parent_progress(&children, basis) })
}

// ------------------------------------------------------------------ Versionen

pub fn get_version(conn: &Connection, id: i64) -> SRes<Version> {
    get_release(conn, ReleaseKind::Version, id)
}

pub fn list_versions(conn: &Connection, project_id: i64) -> SRes<Vec<Version>> {
    list_releases(conn, ReleaseKind::Version, project_id)
}

pub fn create_version(conn: &Connection, n: &NewRelease) -> SRes<i64> {
    create_release(conn, ReleaseKind::Version, n)
}

pub fn update_version(conn: &Connection, id: i64, u: &UpdateRelease) -> SRes<()> {
    update_release(conn, ReleaseKind::Version, id, u)
}

pub fn reorder_versions(conn: &Connection, project_id: i64, ids: &[i64]) -> SRes<()> {
    reorder_releases(conn, ReleaseKind::Version, project_id, ids)
}

/// Version als erschienen markieren (oder zurücknehmen).
pub fn set_version_released(conn: &Connection, id: i64, released: bool) -> SRes<()> {
    set_release_done(conn, ReleaseKind::Version, id, released)
}

pub fn version_usage(conn: &Connection, id: i64) -> SRes<i64> {
    release_usage(conn, ReleaseKind::Version, id)
}

pub fn delete_version(conn: &Connection, id: i64, mode: Option<DeleteMode>) -> SRes<()> {
    delete_release(conn, ReleaseKind::Version, id, mode)
}

pub fn version_tickets(conn: &Connection, id: i64) -> SRes<Vec<Ticket>> {
    release_tickets(conn, ReleaseKind::Version, id)
}

pub fn version_progress(conn: &Connection, id: i64, basis: Basis) -> SRes<ReleaseProgress> {
    release_progress(conn, ReleaseKind::Version, id, basis)
}

// ------------------------------------------------------------------ Meilensteine

pub fn get_milestone(conn: &Connection, id: i64) -> SRes<Milestone> {
    get_release(conn, ReleaseKind::Milestone, id)
}

pub fn list_milestones(conn: &Connection, project_id: i64) -> SRes<Vec<Milestone>> {
    list_releases(conn, ReleaseKind::Milestone, project_id)
}

pub fn create_milestone(conn: &Connection, n: &NewRelease) -> SRes<i64> {
    create_release(conn, ReleaseKind::Milestone, n)
}

pub fn update_milestone(conn: &Connection, id: i64, u: &UpdateRelease) -> SRes<()> {
    update_release(conn, ReleaseKind::Milestone, id, u)
}

pub fn reorder_milestones(conn: &Connection, project_id: i64, ids: &[i64]) -> SRes<()> {
    reorder_releases(conn, ReleaseKind::Milestone, project_id, ids)
}

/// Meilenstein als erreicht markieren (oder zurücknehmen).
pub fn set_milestone_reached(conn: &Connection, id: i64, reached: bool) -> SRes<()> {
    set_release_done(conn, ReleaseKind::Milestone, id, reached)
}

pub fn milestone_usage(conn: &Connection, id: i64) -> SRes<i64> {
    release_usage(conn, ReleaseKind::Milestone, id)
}

pub fn delete_milestone(conn: &Connection, id: i64, mode: Option<DeleteMode>) -> SRes<()> {
    delete_release(conn, ReleaseKind::Milestone, id, mode)
}

pub fn milestone_tickets(conn: &Connection, id: i64) -> SRes<Vec<Ticket>> {
    release_tickets(conn, ReleaseKind::Milestone, id)
}

pub fn milestone_progress(conn: &Connection, id: i64, basis: Basis) -> SRes<ReleaseProgress> {
    release_progress(conn, ReleaseKind::Milestone, id, basis)
}
