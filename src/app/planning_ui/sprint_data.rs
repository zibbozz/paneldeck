//! Daten der Backlog-/Sprint-/Release-Oberfläche: aus der Datenschicht (`planning::sprints`) für die Anzeige aufbereitet
//! (Zeilen mit Typ und Status, Berichtsgruppen, Fortschritt), dazu die Schreibzugriffe der Oberfläche (Fallenlassen von Tickets,
//! Kalender-Einstellungen). Kein egui; alles mit `&Connection` und damit testbar.

use super::sprint_logic::{CalSettings, DropPlan, Section};
use crate::calc::burndown::{BurndownSeries, VelocityPoint};
use crate::calc::calendar::WorkCalendar;
use crate::calc::progress::Basis;
use crate::db::Db;
use crate::planning::sprints::releases::{list_releases, release_progress, Release, ReleaseKind, ReleaseProgress};
use crate::planning::ops::TicketOp;
use crate::planning::sprints::membership::{move_tickets_to_backlog_tracked, move_tickets_to_sprint_tracked};
use crate::planning::sprints::sprint::{backlog, get_sprint, list_sprints, sprint_content, Sprint, SprintContent, SprintState};
use crate::planning::sprints::{burndown_for_sprint, sprint_report, velocity_for_project, SRes, SprintError, SprintReport, SprintTotals};
use crate::planning::statuses::StatusCategory;
use crate::planning::summary::ProjectSummary;
use crate::planning::tickets::{get_ticket, move_rank, Ticket, POINTS_FIELD_KEY};
use crate::state::Lang;
use chrono::NaiveDate;
use rusqlite::{params, Connection};
use std::collections::HashMap;

/// Ein Ticket in Backlog oder Sprint, fertig für die Zeile.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub ticket: Ticket,
    pub type_name: String,
    pub type_color: String,
    pub type_icon: Option<String>,
    pub status_name: String,
    pub status_color: String,
    pub done: bool,
    pub points: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SprintBox {
    pub sprint: Sprint,
    pub rows: Vec<Row>,
    pub totals: SprintTotals,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BacklogData {
    /// Der aktive Sprint zuerst, dann die geplanten nach Start.
    pub sprints: Vec<SprintBox>,
    /// Tickets ohne offenen Sprint nach Rang (ohne die erledigten, wenn sie ausgeblendet sind).
    pub backlog: Vec<Row>,
    /// Erledigte Tickets im Backlog, die gerade ausgeblendet sind.
    pub hidden_done: usize,
    /// Anzahl der abgeschlossenen Sprints (für den Hinweis im Kopf).
    pub completed_sprints: usize,
}

impl BacklogData {
    /// Alle Tickets in Anzeigereihenfolge der Bereiche (Sprints, dann Backlog) – ohne zugeklappte Bereiche und mit dem Seitenlimit je Bereich.
    pub fn visible_ids(&self, collapsed: &dyn Fn(Section) -> bool, shown: &dyn Fn(Section, usize) -> usize) -> Vec<i64> {
        let mut out = Vec::new();
        for s in &self.sprints {
            let sec = Section::Sprint(s.sprint.id);
            if !collapsed(sec) {
                out.extend(s.rows.iter().take(shown(sec, s.rows.len())).map(|r| r.ticket.id));
            }
        }
        if !collapsed(Section::Backlog) {
            out.extend(self.backlog.iter().take(shown(Section::Backlog, self.backlog.len())).map(|r| r.ticket.id));
        }
        out
    }

    /// Alle Tickets eines Bereichs in Reihenfolge (auch die nicht gezeigten).
    pub fn section_ids(&self, section: Section) -> Vec<i64> {
        match section {
            Section::Backlog => self.backlog.iter().map(|r| r.ticket.id).collect(),
            Section::Sprint(id) => self.sprints.iter().find(|s| s.sprint.id == id).map(|s| s.rows.iter().map(|r| r.ticket.id).collect()).unwrap_or_default(),
        }
    }

    pub fn row(&self, ticket_id: i64) -> Option<&Row> {
        self.sprints.iter().flat_map(|s| s.rows.iter()).chain(self.backlog.iter()).find(|r| r.ticket.id == ticket_id)
    }

    pub fn sprint(&self, id: i64) -> Option<&SprintBox> {
        self.sprints.iter().find(|s| s.sprint.id == id)
    }
}

/// Offene Sprints in Anzeigereihenfolge: aktiv zuerst, dann geplant nach Start (ohne Start zuletzt), bei Gleichstand nach Kennung.
pub fn sort_open_sprints(v: &mut [Sprint]) {
    v.sort_by_key(|s| (s.state != SprintState::Active, s.start_date.is_none(), s.start_date, s.id));
}

/// Story Points aller Tickets des Projekts (`ticket id` → Punkte).
fn points_map(conn: &Connection, project_id: i64) -> SRes<HashMap<i64, f64>> {
    let mut st = conn.prepare(
        "SELECT v.ticket_id, v.num FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND f.key = ?2 AND f.kind = 'number' AND v.num IS NOT NULL",
    )?;
    let rows = st.query_map(params![project_id, POINTS_FIELD_KEY], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

/// Zeile für ein Ticket: Typ und Status aus der Projektübersicht.
pub fn make_row(t: Ticket, points: Option<f64>, sum: &ProjectSummary, lang: Lang) -> Row {
    let ty = sum.type_by_id(t.type_id).map(|x| &x.ticket_type);
    let st = t.status_id.and_then(|s| sum.status(s));
    Row {
        type_name: ty.map(|x| x.display(lang)).unwrap_or_default(),
        type_color: ty.map(|x| x.color.clone()).unwrap_or_else(|| "#888888".into()),
        type_icon: ty.and_then(|x| x.icon.clone()),
        status_name: st.map(|s| s.display(lang)).unwrap_or_default(),
        status_color: st.map(|s| s.color.clone()).unwrap_or_else(|| "#888888".into()),
        done: st.is_some_and(|s| s.category == StatusCategory::Done),
        points,
        ticket: t,
    }
}

/// Lädt Sprints und Backlog des Projekts.
pub fn load_backlog(conn: &Connection, sum: &ProjectSummary, show_done: bool, lang: Lang) -> SRes<BacklogData> {
    let pid = sum.project.id;
    let all = list_sprints(conn, pid)?;
    let completed_sprints = all.iter().filter(|s| s.state == SprintState::Completed).count();
    let mut open: Vec<Sprint> = all.into_iter().filter(|s| s.state.is_open()).collect();
    sort_open_sprints(&mut open);
    let mut sprints = Vec::with_capacity(open.len());
    for sp in open {
        let c: SprintContent = sprint_content(conn, sp.id)?;
        let rows = c.items.into_iter().map(|i| make_row(i.ticket, i.points, sum, lang)).collect();
        sprints.push(SprintBox { sprint: c.sprint, rows, totals: c.totals });
    }
    let points = points_map(conn, pid)?;
    let mut hidden_done = 0;
    let mut rows = Vec::new();
    for t in backlog(conn, pid, true)? {
        let p = points.get(&t.id).copied();
        let row = make_row(t, p, sum, lang);
        if row.done && !show_done {
            hidden_done += 1;
        } else {
            rows.push(row);
        }
    }
    Ok(BacklogData { sprints, backlog: rows, hidden_done, completed_sprints })
}

// ------------------------------------------------------------------ Fallenlassen und Verschieben

/// Führt einen Drop-Plan aus: erst die Zugehörigkeit (Sprint/Backlog), dann der Rang. Liefert, wie viele Tickets den Bereich gewechselt haben,
/// und die Operation für Strg+Z / Strg+Y: Zugehörigkeit (samt Sprint-Zeilen für Burndown und Bericht, siehe `sprints::membership`) und
/// Rang bilden **einen** Schritt (`None`, wenn sich nichts geändert hat).
pub fn apply_drop(conn: &Connection, plan: &DropPlan) -> Result<(usize, Option<TicketOp>), SprintError> {
    let (moved, change) = match plan.target {
        Section::Backlog => move_tickets_to_backlog_tracked(conn, &plan.ids)?,
        Section::Sprint(id) => move_tickets_to_sprint_tracked(conn, id, &plan.ids)?,
    };
    let mut ops: Vec<TicketOp> = change.map(TicketOp::Sprint).into_iter().collect();
    for id in &plan.ids {
        ops.extend(move_rank(conn, *id, plan.before)?);
    }
    Ok((moved, TicketOp::combine(ops)))
}

/// Verschiebt Tickets aus dem Kontextmenü (ohne Rang zu ändern); liefert dazu die Operation für Strg+Z.
pub fn move_to(conn: &Connection, target: Section, ids: &[i64]) -> Result<(usize, Option<TicketOp>), SprintError> {
    let (moved, change) = match target {
        Section::Backlog => move_tickets_to_backlog_tracked(conn, ids)?,
        Section::Sprint(id) => move_tickets_to_sprint_tracked(conn, id, ids)?,
    };
    Ok((moved, change.map(TicketOp::Sprint)))
}

// ------------------------------------------------------------------ Kalender

pub fn load_cal(db: &Db, project: i64) -> CalSettings {
    let (kw, kh) = CalSettings::meta_keys(project);
    CalSettings::from_meta(db.meta(&kw).as_deref(), db.meta(&kh).as_deref())
}

pub fn save_cal(db: &Db, project: i64, c: &CalSettings) {
    let (kw, kh) = CalSettings::meta_keys(project);
    db.set_meta(&kw, if c.exclude_weekends { "1" } else { "0" });
    db.set_meta(&kh, &c.holidays_text());
}

// ------------------------------------------------------------------ Sprint-Ansicht

/// Eintrag in einer Berichtsliste.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportItem {
    pub id: i64,
    pub key: String,
    pub title: String,
    pub points: Option<f64>,
}

/// Gruppen des Sprint-Berichts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Planned,
    Completed,
    Added,
    Removed,
    Open,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::Planned, Group::Completed, Group::Added, Group::Removed, Group::Open];

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Group::Planned => lang.t("Geplant", "Planned"),
            Group::Completed => lang.t("Erledigt", "Completed"),
            Group::Added => lang.t("Hinzugefügt", "Added"),
            Group::Removed => lang.t("Entfernt", "Removed"),
            Group::Open => lang.t("Offen", "Open"),
        }
    }

    pub fn hint(self, lang: Lang) -> &'static str {
        match self {
            Group::Planned => lang.t("Der zugesagte Umfang beim Start des Sprints.", "The scope committed when the sprint started."),
            Group::Completed => lang.t("In diesem Sprint erledigt.", "Completed in this sprint."),
            Group::Added => lang.t("Nach dem Start dazugekommen.", "Added after the start."),
            Group::Removed => lang.t("Nach dem Start unerledigt herausgenommen.", "Taken out unfinished after the start."),
            Group::Open => lang.t("Noch im Sprint und nicht erledigt.", "Still in the sprint and not done."),
        }
    }
}

/// Alles für die Ansicht eines Sprints: Bericht, Burndown, Summen.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub sprint: Sprint,
    pub report: SprintReport,
    pub totals: SprintTotals,
    pub series: Option<BurndownSeries>,
    pub groups: Vec<(Group, Vec<ReportItem>)>,
}

/// Burndown-Datum: beim abgeschlossenen Sprint das Ende, sonst heute.
pub fn burn_day(sprint: &Sprint, today: NaiveDate) -> NaiveDate {
    match (sprint.state == SprintState::Completed, sprint.end_date) {
        (true, Some(e)) => super::sprint_chart::chart_today(true, e, today),
        _ => today,
    }
}

pub fn load_detail(conn: &Connection, sprint_id: i64, basis: Basis, cal: &WorkCalendar, today: NaiveDate) -> SRes<Detail> {
    let sprint = get_sprint(conn, sprint_id)?;
    let report = sprint_report(conn, sprint_id)?;
    let totals = sprint_content(conn, sprint_id)?.totals;
    let series = match burndown_for_sprint(conn, sprint_id, basis, cal.clone(), burn_day(&sprint, today)) {
        Ok(s) => Some(s),
        Err(SprintError::DatesIncomplete) => None,
        Err(e) => return Err(e),
    };
    let mut groups = Vec::new();
    for g in Group::ALL {
        let mut items = Vec::new();
        for e in report.entries.iter().filter(|e| match g {
            Group::Planned => e.planned,
            Group::Completed => e.completed,
            Group::Added => e.added,
            Group::Removed => e.removed,
            Group::Open => e.open,
        }) {
            if let Ok(t) = get_ticket(conn, e.ticket_id) {
                items.push(ReportItem { id: t.id, key: t.key(), title: t.title, points: e.points });
            }
        }
        groups.push((g, items));
    }
    Ok(Detail { sprint, report, totals, series, groups })
}

/// Arbeitstage von heute (oder vom Start, wenn der Sprint noch nicht begonnen hat) bis zum Ende, beide eingeschlossen. 0 nach dem Ende.
pub fn remaining_work_days(start: NaiveDate, end: NaiveDate, cal: &WorkCalendar, today: NaiveDate) -> u32 {
    if today > end {
        return 0;
    }
    cal.work_days_inclusive(today.max(start), end)
}

/// Velocity der letzten `n` abgeschlossenen Sprints.
pub fn load_velocity(conn: &Connection, project_id: i64, n: usize, basis: Basis) -> SRes<Vec<VelocityPoint>> {
    velocity_for_project(conn, project_id, n, basis)
}

/// Sprints für die Auswahl im Bericht: aktive und abgeschlossene, die neuesten zuerst.
pub fn report_sprints(conn: &Connection, project_id: i64) -> SRes<Vec<Sprint>> {
    let mut v: Vec<Sprint> = list_sprints(conn, project_id)?.into_iter().filter(|s| s.state != SprintState::Planned).collect();
    v.sort_by(|a, b| (b.state == SprintState::Active).cmp(&(a.state == SprintState::Active)).then(b.completed_at.cmp(&a.completed_at)).then(b.id.cmp(&a.id)));
    Ok(v)
}

// ------------------------------------------------------------------ Releases

#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseRow {
    pub release: Release,
    pub progress: ReleaseProgress,
}

pub fn load_releases(conn: &Connection, kind: ReleaseKind, project_id: i64, basis: Basis) -> SRes<Vec<ReleaseRow>> {
    let mut out = Vec::new();
    for r in list_releases(conn, kind, project_id)? {
        let progress = release_progress(conn, kind, r.id, basis)?;
        out.push(ReleaseRow { release: r, progress });
    }
    Ok(out)
}

/// Anzeige „offen · in Arbeit · erledigt“ eines Fortschritts.
pub fn progress_parts(p: &ReleaseProgress) -> [u32; 3] {
    [p.todo, p.in_progress, p.done]
}

/// Breiten der drei Fortschrittsabschnitte (To Do, In Arbeit, Erledigt) im Balken der Breite `w`; ohne Tickets alles 0.
pub fn bar_widths(parts: [u32; 3], w: f32) -> [f32; 3] {
    let total: u32 = parts.iter().sum();
    if total == 0 {
        return [0.0; 3];
    }
    parts.map(|p| w * p as f32 / total as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::sprint::{create_sprint, move_tickets_to_sprint, start_sprint, NewSprint};
    use crate::planning::summary::project_summary;
    use crate::planning::tickets::fixture::Fx;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn env() -> Fx {
        let f = Fx::new();
        update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
        f
    }

    fn sprint(f: &Fx, name: &str, start: Option<NaiveDate>) -> i64 {
        let mut n = NewSprint::new(f.project, name);
        n.start = start;
        create_sprint(&f.c, &n, &WorkCalendar::default()).unwrap()
    }

    #[test]
    fn open_sprints_are_ordered_active_first_then_by_start() {
        let f = env();
        let a = sprint(&f, "A", Some(d(2026, 12, 1)));
        let b = sprint(&f, "B", None);
        let c = sprint(&f, "C", Some(d(2026, 11, 2)));
        let e = sprint(&f, "E", Some(d(2026, 11, 20)));
        start_sprint(&f.c, e, &WorkCalendar::default()).unwrap();
        let mut v = list_sprints(&f.c, f.project).unwrap();
        v.reverse();
        sort_open_sprints(&mut v);
        assert_eq!(v.iter().map(|s| s.id).collect::<Vec<_>>(), vec![e, c, a, b]);
    }

    #[test]
    fn backlog_data_holds_sprints_and_rows_with_type_and_status() {
        let f = env();
        let t1 = f.mk(f.task, "Erste");
        let t2 = f.mk(f.task, "Zweite");
        let t3 = f.mk(f.task, "Dritte");
        let sp = sprint(&f, "Sprint 1", None);
        move_tickets_to_sprint(&f.c, sp, &[t2.id]).unwrap();
        crate::planning::transition::transition_ticket(&f.c, t3.id, f.done, &[], &crate::planning::history::Source::ui()).unwrap();
        let sum = project_summary(&f.c, f.project).unwrap();
        let d = load_backlog(&f.c, &sum, false, Lang::En).unwrap();
        assert_eq!(d.sprints.len(), 1);
        assert_eq!(d.sprints[0].rows.len(), 1);
        assert_eq!(d.sprints[0].rows[0].ticket.id, t2.id);
        assert!(!d.sprints[0].rows[0].type_name.is_empty());
        assert!(d.sprints[0].rows[0].type_color.starts_with('#'));
        assert_eq!(d.backlog.iter().map(|r| r.ticket.id).collect::<Vec<_>>(), vec![t1.id], "das erledigte Ticket ist ausgeblendet");
        assert_eq!(d.hidden_done, 1);
        let all = load_backlog(&f.c, &sum, true, Lang::En).unwrap();
        assert_eq!(all.backlog.len(), 2);
        assert!(all.backlog.iter().any(|r| r.done && r.ticket.id == t3.id));
        assert_eq!(all.hidden_done, 0);
        assert_eq!(all.section_ids(Section::Sprint(sp)), vec![t2.id]);
        assert_eq!(all.section_ids(Section::Sprint(9999)), Vec::<i64>::new());
        assert!(all.row(t1.id).is_some() && all.row(12345).is_none());
        assert!(all.sprint(sp).is_some());
    }

    #[test]
    fn visible_ids_respect_collapse_and_paging() {
        let f = env();
        let ts: Vec<_> = (0..4).map(|i| f.mk(f.task, &format!("T{i}"))).collect();
        let sp = sprint(&f, "S", None);
        move_tickets_to_sprint(&f.c, sp, &[ts[0].id, ts[1].id]).unwrap();
        let sum = project_summary(&f.c, f.project).unwrap();
        let d = load_backlog(&f.c, &sum, true, Lang::En).unwrap();
        let none = |_: Section| false;
        let all = |_: Section, n: usize| n;
        assert_eq!(d.visible_ids(&none, &all), vec![ts[0].id, ts[1].id, ts[2].id, ts[3].id]);
        let collapsed = |s: Section| s == Section::Sprint(sp);
        assert_eq!(d.visible_ids(&collapsed, &all), vec![ts[2].id, ts[3].id]);
        let one = |_: Section, n: usize| n.min(1);
        assert_eq!(d.visible_ids(&none, &one), vec![ts[0].id, ts[2].id]);
    }

    #[test]
    fn story_points_come_into_the_rows() {
        let f = env();
        let t = f.mk(f.task, "Mit Punkten");
        let sp = f.field(f.task, "story_points");
        crate::planning::tickets::set_field(&f.c, t.id, sp.id, serde_json::json!(5), &crate::planning::history::Source::ui()).unwrap();
        let sum = project_summary(&f.c, f.project).unwrap();
        let d = load_backlog(&f.c, &sum, true, Lang::En).unwrap();
        assert_eq!(d.backlog[0].points, Some(5.0));
    }

    #[test]
    fn dropping_moves_between_sections_and_sets_the_rank() {
        let f = env();
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let c = f.mk(f.task, "C");
        let sp = sprint(&f, "S", None);
        // B und C in den Sprint, C vor B
        let plan = DropPlan { target: Section::Sprint(sp), before: None, ids: vec![b.id, c.id] };
        assert_eq!(apply_drop(&f.c, &plan).unwrap().0, 2);
        let sum = project_summary(&f.c, f.project).unwrap();
        let data = load_backlog(&f.c, &sum, true, Lang::En).unwrap();
        assert_eq!(data.section_ids(Section::Sprint(sp)), vec![b.id, c.id]);
        assert_eq!(data.section_ids(Section::Backlog), vec![a.id]);
        // C vor B (innerhalb des Sprints): nur Rang, niemand wechselt den Bereich
        let plan = DropPlan { target: Section::Sprint(sp), before: Some(b.id), ids: vec![c.id] };
        assert_eq!(apply_drop(&f.c, &plan).unwrap().0, 0);
        let data = load_backlog(&f.c, &sum, true, Lang::En).unwrap();
        assert_eq!(data.section_ids(Section::Sprint(sp)), vec![c.id, b.id]);
        // beide ins Backlog vor A, in der gezogenen Reihenfolge
        let plan = DropPlan { target: Section::Backlog, before: Some(a.id), ids: vec![b.id, c.id] };
        assert_eq!(apply_drop(&f.c, &plan).unwrap().0, 2);
        let data = load_backlog(&f.c, &sum, true, Lang::En).unwrap();
        assert_eq!(data.section_ids(Section::Backlog), vec![b.id, c.id, a.id]);
        assert!(data.section_ids(Section::Sprint(sp)).is_empty());
    }

    #[test]
    fn dropping_into_a_completed_sprint_is_refused() {
        let f = env();
        let a = f.mk(f.task, "A");
        let sp = sprint(&f, "S", Some(d(2026, 11, 2)));
        start_sprint(&f.c, sp, &WorkCalendar::default()).unwrap();
        crate::planning::sprints::sprint::complete_sprint(&f.c, sp, crate::planning::sprints::Leftover::ToBacklog).unwrap();
        let plan = DropPlan { target: Section::Sprint(sp), before: None, ids: vec![a.id] };
        assert_eq!(apply_drop(&f.c, &plan).unwrap_err(), SprintError::SprintCompleted);
    }

    #[test]
    fn context_menu_moves_without_changing_the_rank() {
        let f = env();
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let sp = sprint(&f, "S", None);
        assert_eq!(move_to(&f.c, Section::Sprint(sp), &[b.id, a.id]).unwrap().0, 2);
        let sum = project_summary(&f.c, f.project).unwrap();
        let data = load_backlog(&f.c, &sum, true, Lang::En).unwrap();
        assert_eq!(data.section_ids(Section::Sprint(sp)), vec![a.id, b.id], "nach Rang, nicht nach Reihenfolge der Auswahl");
        assert_eq!(move_to(&f.c, Section::Backlog, &[a.id]).unwrap().0, 1);
    }

    #[test]
    fn calendar_settings_are_saved_per_project() {
        let f = env();
        let db = Db { conn: crate::db::open_memory(), path: std::path::PathBuf::new() };
        assert_eq!(load_cal(&db, 3), CalSettings::default());
        let c = CalSettings { exclude_weekends: false, holidays: vec![d(2026, 12, 25)] };
        save_cal(&db, 3, &c);
        assert_eq!(load_cal(&db, 3), c);
        assert_eq!(load_cal(&db, 4), CalSettings::default());
        drop(f);
    }

    #[test]
    fn remaining_days_follow_the_calendar() {
        let cal = WorkCalendar::default();
        let (s, e) = (d(2026, 11, 2), d(2026, 11, 13));
        assert_eq!(remaining_work_days(s, e, &cal, d(2026, 11, 2)), 10);
        assert_eq!(remaining_work_days(s, e, &cal, d(2026, 11, 11)), 3);
        assert_eq!(remaining_work_days(s, e, &cal, d(2026, 11, 14)), 0, "Sprint überfällig");
        assert_eq!(remaining_work_days(s, e, &cal, d(2026, 10, 1)), 10, "vor dem Start zählt der ganze Sprint");
        assert_eq!(remaining_work_days(s, e, &cal, d(2026, 11, 7)), 5, "Samstag: die letzte Woche bleibt");
    }

    #[test]
    fn progress_bar_widths_add_up() {
        assert_eq!(bar_widths([0, 0, 0], 100.0), [0.0; 3]);
        assert_eq!(bar_widths([1, 1, 2], 100.0), [25.0, 25.0, 50.0]);
        assert_eq!(bar_widths([0, 0, 5], 80.0), [0.0, 0.0, 80.0]);
    }

    #[test]
    fn report_sprint_list_has_active_first_then_newest_completed() {
        crate::planning::sprints::clock::set("2026-11-14T09:00:00");
        let f = env();
        let cal = WorkCalendar::default();
        let s1 = sprint(&f, "S1", Some(d(2026, 11, 2)));
        start_sprint(&f.c, s1, &cal).unwrap();
        crate::planning::sprints::sprint::complete_sprint(&f.c, s1, crate::planning::sprints::Leftover::ToBacklog).unwrap();
        crate::planning::sprints::clock::set("2026-11-20T10:00:00");
        let s2 = sprint(&f, "S2", Some(d(2026, 11, 16)));
        start_sprint(&f.c, s2, &cal).unwrap();
        crate::planning::sprints::sprint::complete_sprint(&f.c, s2, crate::planning::sprints::Leftover::ToBacklog).unwrap();
        let s3 = sprint(&f, "S3", Some(d(2026, 11, 23)));
        sprint(&f, "S4", None);
        start_sprint(&f.c, s3, &cal).unwrap();
        let ids: Vec<i64> = report_sprints(&f.c, f.project).unwrap().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![s3, s2, s1]);
    }

    #[test]
    fn detail_groups_list_the_tickets_of_the_report() {
        let f = env();
        let cal = WorkCalendar::default();
        crate::planning::sprints::clock::set("2026-11-02T09:00:00");
        let a = f.mk(f.task, "Fertig");
        let b = f.mk(f.task, "Offen");
        let sp = sprint(&f, "S", Some(d(2026, 11, 2)));
        move_tickets_to_sprint(&f.c, sp, &[a.id, b.id]).unwrap();
        start_sprint(&f.c, sp, &cal).unwrap();
        crate::planning::sprints::clock::set("2026-11-03T10:00:00");
        crate::planning::transition::transition_ticket(&f.c, a.id, f.done, &[], &crate::planning::history::Source::ui()).unwrap();
        f.c.execute("UPDATE history SET at = '2026-11-03T10:00:00' WHERE id = (SELECT MAX(id) FROM history WHERE ticket_id = ?1 AND field = 'status')", [a.id]).unwrap();
        let det = load_detail(&f.c, sp, Basis::TicketCount, &cal, d(2026, 11, 3)).unwrap();
        let g = |grp: Group| det.groups.iter().find(|(x, _)| *x == grp).map(|(_, v)| v.iter().map(|i| i.title.clone()).collect::<Vec<_>>()).unwrap();
        assert_eq!(g(Group::Planned), vec!["Fertig", "Offen"]);
        assert_eq!(g(Group::Completed), vec!["Fertig"]);
        assert_eq!(g(Group::Open), vec!["Offen"]);
        assert!(g(Group::Added).is_empty() && g(Group::Removed).is_empty());
        assert_eq!(det.totals.total.tickets, 2);
        assert_eq!(det.totals.done.tickets, 1);
        let s = det.series.unwrap();
        assert_eq!(s.scope_total_start, 2.0);
        assert_eq!(s.completed, 1.0);
        assert_eq!(det.report.planned.tickets, 2);
    }

    #[test]
    fn a_sprint_without_dates_has_no_burndown() {
        let f = env();
        let sp = sprint(&f, "S", None);
        let det = load_detail(&f.c, sp, Basis::Points, &WorkCalendar::default(), d(2026, 11, 3)).unwrap();
        assert!(det.series.is_none());
    }

    #[test]
    fn burndown_day_is_the_end_for_completed_sprints() {
        let f = env();
        let cal = WorkCalendar::default();
        crate::planning::sprints::clock::set("2026-11-14T09:00:00");
        let sp = sprint(&f, "S", Some(d(2026, 11, 2)));
        let s = start_sprint(&f.c, sp, &cal).unwrap();
        assert_eq!(burn_day(&s, d(2026, 11, 5)), d(2026, 11, 5));
        let done = crate::planning::sprints::sprint::complete_sprint(&f.c, sp, crate::planning::sprints::Leftover::ToBacklog).unwrap().sprint;
        assert_eq!(burn_day(&done, d(2027, 1, 1)), d(2026, 11, 13));
    }

    #[test]
    fn releases_come_with_progress() {
        let f = env();
        let v = crate::planning::sprints::releases::create_version(&f.c, &crate::planning::sprints::releases::NewRelease::new(f.project, "1.0")).unwrap();
        let rows = load_releases(&f.c, ReleaseKind::Version, f.project, Basis::TicketCount).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].release.id, v);
        assert_eq!(rows[0].progress.total(), 0);
        assert_eq!(progress_parts(&rows[0].progress), [0, 0, 0]);
        assert!(load_releases(&f.c, ReleaseKind::Milestone, f.project, Basis::TicketCount).unwrap().is_empty());
    }
}
