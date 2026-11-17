//! Demo-Datenbank für die README-Screenshots (nur Tests; wird nie mitgebaut).
//!
//! `cargo test --release demo_shots -- --ignored --nocapture` mit den Umgebungsvariablen
//!
//! * `PANELDECK_DEMO_DIR` – leerer Datenordner (Pflicht): dort entstehen `paneldeck.db` und `settings.json`,
//! * `PANELDECK_DEMO_LANG` – `en` (Standard) oder `de`,
//! * `PANELDECK_DEMO_TODAY` – optional `JJJJ-MM-TT` statt des heutigen Datums (der aktive Sprint liegt immer um „heute“),
//!
//! legt dort ein Projekt „Acme Shop“ (`SHOP`) mit Typen, Status, Feldern, ~45 Tickets, Kommentaren, Zeiten, Verknüpfungen,
//! vier Sprints (zwei abgeschlossen, einer aktiv mit Burndown-Verlauf, einer geplant), Versionen, Meilensteinen, gespeicherten
//! Filtern, zwei Boards und einem Dashboard an. Alles läuft über die echten Modul-Schnittstellen; die Uhr (`tasks::set_fixed_now`)
//! wird je Schritt gesetzt, damit Verlauf, Burndown und Zeiterfassung über mehrere Tage entstehen. Es werden keine echten Daten
//! gelesen und keine persönlichen Angaben verwendet.

mod data;

use crate::calc::calendar::WorkCalendar;
use crate::planning::fields::{create_field, list_fields, FieldKind, FieldOptions, NewField};
use crate::planning::history::Source;
use crate::planning::link_types::link_type_by_key;
use crate::planning::links::{add_link, Target};
use crate::planning::projects::{create_project, NewProject};
use crate::planning::sprints::releases::{create_milestone, create_version, set_milestone_reached, set_version_released, NewRelease};
use crate::planning::sprints::sprint::{complete_sprint, create_sprint, move_tickets_to_backlog, move_tickets_to_sprint, start_sprint, Leftover, NewSprint};
use crate::planning::statuses::{create_status, list_statuses, reorder_statuses, NewStatus, StatusCategory};
use crate::planning::tickets::{create_ticket, set_flagged, set_starred, NewTicket};
use crate::planning::time_entries::{log_time, StartInput};
use crate::planning::transition::transition_ticket;
use crate::planning::types::list_types;
use crate::planning::views::boards::{create_board, BoardColumn, BoardConfig, NewBoard};
use crate::planning::views::dashboards::{create_dashboard, set_default, ChartKind, CountGroup, Layout, NewDashboard, TileG, TileKind, TimeRange, LAYOUT_VERSION};
use crate::planning::views::filters::{create_filter, NewFilter};
use crate::planning::work_kinds::list_work_kinds;
use crate::planning::workflow::{replace_transitions, set_workflow_statuses, TransitionDef};
use crate::state::Lang;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Weekday};
use data::{Sp, St, Tk, Ty};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Debug;

type Res<T> = Result<T, String>;

trait Ex<T> {
    fn ex(self, what: &str) -> Res<T>;
}

impl<T, E: Debug> Ex<T> for Result<T, E> {
    fn ex(self, what: &str) -> Res<T> {
        self.map_err(|e| format!("{what}: {e:?}"))
    }
}

/// Was `build` angelegt hat.
#[derive(Debug)]
pub struct Built {
    pub project_id: i64,
    /// Kennung aus der Tabelle → Ticket-Schlüssel (`SHOP-12`)
    pub keys: HashMap<&'static str, String>,
    pub tickets: usize,
    pub s12_id: i64,
    /// Erster Tag des aktiven Sprints und „heute“ (Arbeitstag)
    pub s12_start: NaiveDate,
    pub today: NaiveDate,
}

/// Ein Ereignis der Geschichte; sie werden nach Zeit sortiert und der Reihe nach ausgeführt.
enum Ev {
    Create(usize),
    Sprint(usize),
    SprintAdd(usize, usize),
    SprintStart(usize),
    SprintComplete(usize),
    SprintDrop(usize),
    Move(usize, St, Source),
    Comment(usize, usize),
    Time(usize, i64, &'static str, usize),
    Star(usize),
    Flag(usize),
    Link(usize, &'static str, usize),
}

struct Builder<'a> {
    conn: &'a Connection,
    lang: Lang,
    de: bool,
    cal: WorkCalendar,
    s10: NaiveDate,
    today: NaiveDate,
    today_w: i64,
}

impl Builder<'_> {
    fn wd(&self, w: i64) -> NaiveDate {
        self.cal.add_work_days(self.s10, w).unwrap_or(self.s10)
    }

    /// Zeitpunkt am Arbeitstag `w`. Ab „heute“ werden alle Zeitpunkte unter Beibehaltung der Reihenfolge in den Vormittag von heute
    /// gestaucht (Arbeitstag und Stunde ergeben die Reihenfolge); so liegt nichts in der Zukunft und kein Status-Verlauf gerät durcheinander.
    fn at(&self, w: i64, h: u32, m: u32) -> NaiveDateTime {
        if w < self.today_w {
            return self.wd(w).and_hms_opt(h, m, 0).unwrap_or_default();
        }
        let r = (w - self.today_w) * 10 + i64::from(h.saturating_sub(8)).min(9);
        let mins = 7 * 60 + 30 + r * 6 + i64::from(m) / 10;
        self.wd(self.today_w).and_hms_opt((mins / 60).min(23) as u32, (mins % 60) as u32, 0).unwrap_or_default()
    }
}

fn iso(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Der Sprint-Start so, dass „heute“ der 8. Arbeitstag des aktiven Sprints ist (Montag bis Donnerstag: 6.–9.).
fn sprint12_start(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let mut eff = today;
    while matches!(eff.weekday(), Weekday::Sat | Weekday::Sun) {
        eff = eff.pred_opt().unwrap_or(eff);
    }
    let monday = eff - Duration::days(eff.weekday().num_days_from_monday() as i64);
    let prev_monday = monday - Duration::days(7);
    let start = if eff.weekday().num_days_from_monday() <= 3 { prev_monday } else { prev_monday + Duration::days(2) };
    (start, eff)
}

/// Baut die Demo-Daten in der (leeren) Datenbank. `today` ist das Datum des Aufrufs.
pub fn build(conn: &Connection, lang: Lang, today: NaiveDate) -> Res<Built> {
    let de = lang == Lang::De;
    let tx = data::texts(de);
    let cal = WorkCalendar::default();
    let (s12_start, eff) = sprint12_start(today);
    let s10 = s12_start - Duration::days(28);
    let mut b = Builder { conn, lang, de, cal, s10, today: eff, today_w: 0 };
    b.today_w = (0..60).find(|w| b.wd(*w) >= eff).unwrap_or(27);
    let result = build_inner(&mut b, &tx, s12_start);
    crate::tasks::set_fixed_now(None);
    result
}

fn build_inner(b: &mut Builder, tx: &data::Texts, s12_start: NaiveDate) -> Res<Built> {
    let conn = b.conn;
    let lang = b.lang;
    crate::tasks::set_fixed_now(Some(&iso(b.at(-9, 9, 0))));

    // ---- Projekt mit Standard-Inhalten
    let mut np = NewProject::new("SHOP", tx.project_name, "#5B8DEF");
    np.description = tx.project_desc.to_string();
    np.with_defaults = true;
    np.use_sprints = true;
    np.use_versions = true;
    let pid = create_project(conn, &np).ex("Projekt")?;

    // ---- Status „In Review“ zwischen „In Progress“ und „Done“
    let base = list_statuses(conn, pid).ex("Status")?;
    let by_key = |k: &str| base.iter().find(|s| s.key.as_deref() == Some(k)).map(|s| s.id).unwrap_or(0);
    let (st_todo, st_progress, st_done) = (by_key("todo"), by_key("progress"), by_key("done"));
    let st_review = create_status(conn, &NewStatus { project_id: pid, key: None, name: Some(tx.review.to_string()), color: "#B77FE0".into(), category: StatusCategory::Progress }).ex("Status Review")?;
    reorder_statuses(conn, pid, &[st_todo, st_progress, st_review, st_done]).ex("Status ordnen")?;
    let types = list_types(conn, pid).ex("Typen")?;
    let type_id = |t: Ty| types.iter().find(|x| x.key.as_deref() == Some(t.key())).map(|x| x.id).unwrap_or(0);
    let def = |f: i64, t: i64| TransitionDef { from_status_id: f, to_status_id: t, ..Default::default() };
    for t in &types {
        set_workflow_statuses(conn, t.id, &[st_todo, st_progress, st_review, st_done]).ex("Workflow Status")?;
        replace_transitions(
            conn,
            t.id,
            &[
                def(st_todo, st_progress),
                def(st_progress, st_todo),
                def(st_progress, st_review),
                def(st_review, st_progress),
                def(st_review, st_done),
                def(st_progress, st_done),
                def(st_done, st_progress),
            ],
        )
        .ex("Workflow")?;
    }

    // ---- zusätzliche Felder
    let milestone_name = lang.t("Meilenstein", "Milestone");
    // Labels sind eingebaut (jeder Typ hat sie)
    for t in [Ty::Story, Ty::Task, Ty::Defect] {
        create_field(conn, &NewField::new(type_id(t), milestone_name, FieldKind::Milestone)).ex("Meilenstein")?;
    }
    for t in [Ty::Task, Ty::Defect] {
        let mut v = NewField::new(type_id(t), "Release version", FieldKind::Version);
        v.key = Some("release_version".into());
        v.name = None;
        v.builtin = true;
        create_field(conn, &v).ex("Version")?;
    }
    {
        let mut sp = NewField::new(type_id(Ty::Defect), "Story points", FieldKind::Number);
        sp.key = Some("story_points".into());
        sp.name = None;
        sp.builtin = true;
        sp.options = FieldOptions { min: Some(0.0), decimals: Some(0), ..Default::default() };
        sp.show_on_card = true;
        create_field(conn, &sp).ex("Story points")?;
    }
    let mut field_ids: HashMap<(Ty, &str), i64> = HashMap::new();
    for t in [Ty::Epic, Ty::Story, Ty::Task, Ty::Sub, Ty::Defect] {
        for f in list_fields(conn, type_id(t)).ex("Felder")? {
            let key = match (f.key.as_deref(), f.kind) {
                (Some(k), _) => k.to_string(),
                (None, FieldKind::Labels) => "labels".to_string(),
                (None, FieldKind::Milestone) => "milestone".to_string(),
                _ => continue,
            };
            let key: &'static str = Box::leak(key.into_boxed_str());
            field_ids.insert((t, key), f.id);
        }
    }

    // ---- Versionen, Meilensteine
    let today = b.today;
    let v14 = create_version(conn, &NewRelease { project_id: pid, name: "1.4".into(), description: String::new(), due_date: Some(today - Duration::days(30)) }).ex("Version")?;
    let v15 = create_version(conn, &NewRelease { project_id: pid, name: "1.5".into(), description: String::new(), due_date: Some(today + Duration::days(9)) }).ex("Version")?;
    let v20 = create_version(conn, &NewRelease { project_id: pid, name: "2.0".into(), description: String::new(), due_date: Some(today + Duration::days(37)) }).ex("Version")?;
    set_version_released(conn, v14, true).ex("Version erschienen")?;
    let versions = [0, v14, v15, v20];
    let m_beta = create_milestone(conn, &NewRelease { project_id: pid, name: tx.milestones[0].into(), description: String::new(), due_date: Some(today - Duration::days(14)) }).ex("Meilenstein")?;
    let m_pub = create_milestone(conn, &NewRelease { project_id: pid, name: tx.milestones[1].into(), description: String::new(), due_date: Some(today + Duration::days(37)) }).ex("Meilenstein")?;
    set_milestone_reached(conn, m_beta, true).ex("Meilenstein erreicht")?;
    let milestones = [0, m_beta, m_pub];

    // ---- Sprints (Anlage zu ihrer Zeit, siehe Ereignisse)
    let tickets = data::tickets();
    let idx: HashMap<&str, usize> = tickets.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let mut events: Vec<(NaiveDateTime, usize, Ev)> = Vec::new();
    let mut seq = 0usize;
    let mut push = |t: NaiveDateTime, e: Ev, events: &mut Vec<(NaiveDateTime, usize, Ev)>| {
        events.push((t, seq, e));
        seq += 1;
    };
    for k in 0..4usize {
        push(b.at(if k == 3 { 21 } else { 10 * k as i64 - 3 }, 10, 0), Ev::Sprint(k), &mut events);
        if k < 3 {
            push(b.at(10 * k as i64, 8, 0), Ev::SprintStart(k), &mut events);
        }
    }
    push(b.at(9, 17, 0), Ev::SprintComplete(0), &mut events);
    push(b.at(19, 17, 0), Ev::SprintComplete(1), &mut events);
    for (i, t) in tickets.iter().enumerate() {
        push(b.at(t.created, 8, (i as u32 * 2) % 50), Ev::Create(i), &mut events);
        let (sprint, when): (Option<usize>, Option<NaiveDateTime>) = match t.sprint {
            Sp::None => (None, None),
            Sp::S10 => (Some(0), Some(plan_time(b, t, 0))),
            Sp::S11 | Sp::S11Carry => (Some(1), Some(plan_time(b, t, 1))),
            Sp::S12 | Sp::S12Dropped(..) => (Some(2), Some(plan_time(b, t, 2))),
            Sp::S12Late(w, h) => (Some(2), Some(b.at(w, h, 0))),
            Sp::S13 => (Some(3), Some(b.at(t.created, 10, 30))),
        };
        if let (Some(k), Some(w)) = (sprint, when) {
            push(w, Ev::SprintAdd(k, i), &mut events);
        }
        if let Sp::S12Dropped(w, h) = t.sprint {
            push(b.at(w, h, 0), Ev::SprintDrop(i), &mut events);
        }
        for (n, (w, h, st)) in t.path.iter().enumerate() {
            let src = match (t.id, *st) {
                ("g", St::Progress) => Source::agent("worker"),
                ("h", St::Progress) | ("c", St::Progress) => Source::agent("api-server"),
                _ => Source::ui(),
            };
            push(b.at(*w, *h, (n as u32 * 7) % 40), Ev::Move(i, *st, src), &mut events);
        }
        if data::STARRED.contains(&t.id) {
            push(b.at(t.created, 12, 0), Ev::Star(i), &mut events);
        }
        if data::FLAGGED.contains(&t.id) {
            push(b.at(t.created + 1, 12, 0), Ev::Flag(i), &mut events);
        }
    }
    for (n, (id, w, h, _, _, _)) in data::COMMENTS.iter().enumerate() {
        let ticket = idx.get(id).copied().unwrap_or(0);
        push(b.at(*w, *h, 5 + n as u32), Ev::Comment(ticket, n), &mut events);
    }
    for (n, (id, w, dur, kind)) in data::TIME.iter().enumerate() {
        let i = idx.get(id).copied().unwrap_or(0);
        push(b.at(*w, 17, n as u32 % 50), Ev::Time(i, *w, dur, *kind), &mut events);
    }
    for (from, kind, to) in data::LINKS {
        let (f, t) = (idx.get(from).copied().unwrap_or(0), idx.get(to).copied().unwrap_or(0));
        let at = b.at(tickets[f].created.max(tickets[t].created) + 1, 14, 0);
        push(at, Ev::Link(f, kind, t), &mut events);
    }
    events.sort_by_key(|(t, s, _)| (*t, *s));

    // ---- Ereignisse ausführen
    let kinds = list_work_kinds(conn, pid).ex("Arten der Arbeit")?;
    let cal = WorkCalendar::default();
    let mut ids: Vec<i64> = vec![0; tickets.len()];
    let mut keys: HashMap<&'static str, String> = HashMap::new();
    let mut sprint_ids: [i64; 4] = [0; 4];
    let status_of = |s: St| match s {
        St::Progress => st_progress,
        St::Review => st_review,
        St::Done => st_done,
    };
    for (t, _, ev) in events {
        crate::tasks::set_fixed_now(Some(&iso(t)));
        match ev {
            Ev::Create(i) => {
                let tk = &tickets[i];
                let render = |s: &str| render_text(s, &keys);
                let mut values: Vec<(i64, Value)> = Vec::new();
                let fid = |k: &str| field_ids.get(&(tk.ty, k)).copied();
                if let Some(f) = fid("priority") {
                    values.push((f, json!(tk.prio)));
                }
                if tk.pts > 0 {
                    if let Some(f) = fid("story_points") {
                        values.push((f, json!(tk.pts)));
                    }
                }
                if let (Some(f), false) = (fid("labels"), tk.labels.is_empty()) {
                    values.push((f, json!(tk.labels)));
                }
                if tk.ver > 0 {
                    if let Some(f) = fid("release_version") {
                        values.push((f, json!(versions[tk.ver])));
                    }
                }
                if tk.ms > 0 {
                    if let Some(f) = fid("milestone") {
                        values.push((f, json!(milestones[tk.ms])));
                    }
                }
                if let Some((_, en, de)) = data::DESCRIPTIONS.iter().find(|d| d.0 == tk.id) {
                    if let Some(f) = fid("description") {
                        values.push((f, json!(render(if b.de { de } else { en }))));
                    }
                }
                // Fälligkeit: jedes dritte Ticket, verteilt von überfällig bis in zwei Wochen
                if i % 3 == 0 {
                    if let Some(f) = fid("due") {
                        values.push((f, json!((today + Duration::days((i % 17) as i64 - 3)).format("%Y-%m-%d").to_string())));
                    }
                }
                if let Some((_, en, de)) = data::STEPS.iter().find(|d| d.0 == tk.id) {
                    if let Some(f) = fid("steps") {
                        values.push((f, json!(render(if b.de { de } else { en }))));
                    }
                }
                let parent = if tk.parent.is_empty() { None } else { idx.get(tk.parent).map(|p| ids[*p]) };
                if tk.ty == Ty::Story {
                    if let (Some(f), Some(p)) = (fid("epic"), parent) {
                        values.push((f, json!(p)));
                    }
                }
                let title = if b.de { tk.de } else { tk.en };
                let (made, _) = create_ticket(conn, &NewTicket { project_id: pid, type_id: type_id(tk.ty), title: title.to_string(), parent_id: parent, values }, &Source::ui()).ex(&format!("Ticket {}", tk.id))?;
                ids[i] = made.id;
                keys.insert(tk.id, made.key());
            }
            Ev::Sprint(k) => {
                let n = NewSprint {
                    project_id: pid,
                    name: format!("Sprint {}", 10 + k),
                    goal: tx.sprint_goals[k].to_string(),
                    start: Some(b.wd(10 * k as i64)),
                    end: Some(b.wd(10 * k as i64 + 9)),
                    default_days: Some(10),
                };
                sprint_ids[k] = create_sprint(conn, &n, &cal).ex("Sprint")?;
            }
            Ev::SprintAdd(k, i) => {
                move_tickets_to_sprint(conn, sprint_ids[k], &[ids[i]]).ex(&format!("Sprint {k} <- {}", tickets[i].id))?;
            }
            Ev::SprintStart(k) => {
                start_sprint(conn, sprint_ids[k], &cal).ex("Sprint starten")?;
            }
            Ev::SprintComplete(k) => {
                let leftover = if k == 1 { Leftover::ToSprint(sprint_ids[2]) } else { Leftover::ToBacklog };
                complete_sprint(conn, sprint_ids[k], leftover).ex("Sprint abschließen")?;
            }
            Ev::SprintDrop(i) => {
                move_tickets_to_backlog(conn, &[ids[i]]).ex("Sprint verlassen")?;
            }
            Ev::Move(i, st, src) => {
                transition_ticket(conn, ids[i], status_of(st), &[], &src).ex(&format!("Status {}", tickets[i].id))?;
            }
            Ev::Comment(n, ci) => {
                let (_, _, _, agent, en, de) = data::COMMENTS[ci];
                let src = if agent.is_empty() { Source::ui() } else { Source::agent(agent) };
                crate::planning::comments::add_comment(conn, ids[n], &render_text(if b.de { de } else { en }, &keys), &src).ex("Kommentar")?;
            }
            Ev::Time(i, w, dur, kind) => {
                let start = b.at(w, 9, (i as u32 * 7) % 40);
                let kind_id = kinds.get(kind).map(|k| k.id);
                log_time(conn, ids[i], dur, Some(StartInput::DateTime(start)), kind_id, "", t, &Source::ui()).ex("Zeit")?;
            }
            Ev::Star(i) => {
                set_starred(conn, ids[i], true, &Source::ui()).ex("Stern")?;
            }
            Ev::Flag(i) => {
                set_flagged(conn, ids[i], true, &Source::ui()).ex("Markierung")?;
            }
            Ev::Link(f, kind, t2) => {
                let lt = link_type_by_key(conn, kind).ex("Verknüpfungstyp")?;
                add_link(conn, ids[f], lt.id, Target::Ticket(ids[t2]), &Source::ui()).ex("Verknüpfung")?;
            }
        }
    }

    // ---- Ansichten (mit der echten Uhr, sie sind nicht zeitabhängig)
    crate::tasks::set_fixed_now(Some(&iso(b.at(b.today_w, 8, 0))));
    let status_ids = [st_todo, st_progress, st_review, st_done];
    let mut card_fields: Vec<i64> = Vec::new();
    for key in ["priority", "story_points"] {
        for t in [Ty::Epic, Ty::Story, Ty::Task, Ty::Sub, Ty::Defect] {
            if let Some(f) = field_ids.get(&(t, key)) {
                card_fields.push(*f);
            }
        }
    }
    let columns: Vec<BoardColumn> = status_ids.iter().map(|s| BoardColumn { title: String::new(), status_ids: vec![*s] }).collect();
    let type_filter: Vec<i64> = [Ty::Story, Ty::Task, Ty::Sub, Ty::Defect].iter().map(|t| type_id(*t)).collect();
    create_board(
        conn,
        &NewBoard {
            project_id: pid,
            name: tx.board_sprint.to_string(),
            description: String::new(),
            config: BoardConfig { columns: columns.clone(), type_filter: type_filter.clone(), only_active_sprint: true, card_fields: card_fields.clone(), ..Default::default() },
        },
    )
    .ex("Board")?;
    create_board(
        conn,
        &NewBoard {
            project_id: pid,
            name: tx.board_main.to_string(),
            description: tx.board_main_desc.to_string(),
            config: BoardConfig { columns, type_filter, card_fields, ..Default::default() },
        },
    )
    .ex("Board")?;
    for (name, jql) in tx.filters {
        create_filter(conn, &NewFilter { project_id: Some(pid), name: name.to_string(), jql: jql.to_string() }).ex("Filter")?;
    }
    let tile = |id: u32, x: u32, y: u32, w: u32, h: u32, kind: TileKind<i64>| TileG { id, x, y, w, h, kind };
    let layout = Layout {
        version: LAYOUT_VERSION,
        tiles: vec![
            tile(1, 0, 0, 8, 7, TileKind::Burndown { project_id: None }),
            tile(2, 8, 0, 4, 7, TileKind::Sprint { project_id: None }),
            tile(3, 0, 7, 4, 7, TileKind::Counts { jql: String::new(), group_by: CountGroup::Status, chart: ChartKind::Pie }),
            tile(4, 4, 7, 4, 7, TileKind::Counts { jql: "type != Epic".into(), group_by: CountGroup::Priority, chart: ChartKind::Bar }),
            tile(5, 8, 7, 4, 7, TileKind::TimeTracking { range: TimeRange::Week, project_id: None }),
        ],
    };
    let dash = create_dashboard(conn, &NewDashboard { name: tx.dash_name.to_string(), project_id: Some(pid), layout: Some(layout) }).ex("Dashboard")?;
    set_default(conn, dash).ex("Dashboard Standard")?;
    Ok(Built { project_id: pid, keys, tickets: tickets.len(), s12_id: sprint_ids[2], s12_start, today: b.today })
}

/// Zeitpunkt, zu dem ein Ticket im geplanten Umfang des Sprints `k` landet: am Tag vor dem Start, oder (wenn es erst später angelegt wurde) kurz nach dem Anlegen.
fn plan_time(b: &Builder, t: &Tk, k: usize) -> NaiveDateTime {
    let before = 10 * k as i64 - 1;
    if t.created < before { b.at(before, 15, 0) } else { b.at(t.created, 10, 15) }
}

fn render_text(s: &str, keys: &HashMap<&'static str, String>) -> String {
    let mut out = s.to_string();
    for (id, key) in keys {
        out = out.replace(&format!("{{#{id}}}"), key);
    }
    out
}

/// Optionen fürs Schreiben der Datenordner-Dateien.
pub fn write_data_dir(dir: &std::path::Path, lang: Lang, today: NaiveDate) -> Res<Built> {
    std::fs::create_dir_all(dir).ex("Ordner")?;
    if dir.join(crate::db::FILE).exists() {
        return Err(format!("{} gibt es schon – bitte einen leeren Ordner angeben", crate::db::FILE));
    }
    let backups = dir.join("backups");
    let db = crate::db::open(dir, &backups).map_err(|e| format!("Datenbank: {e:?}"))?;
    let built = build(&db.conn, lang, today)?;
    db.set_meta("expert_mode", "1");
    db.set_meta("expert_project", &built.project_id.to_string());
    drop(db);
    let mut settings = crate::state::Settings::default();
    settings.language = lang;
    settings.backup_interval_hours = 0;
    crate::state::save_json(&dir.join("settings.json"), &settings);
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::search::{count, Ctx, Scope};
    use crate::planning::sprints::analysis::burndown_for_sprint;
    use crate::planning::sprints::sprint::sprint_totals;

    fn mem(lang: Lang, today: NaiveDate) -> (Connection, Built) {
        let c = crate::db::open_memory();
        let built = build(&c, lang, today).unwrap_or_else(|e| panic!("{e}"));
        (c, built)
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn builds_in_both_languages_on_every_weekday() {
        for lang in [Lang::En, Lang::De] {
            // Mo 2026-11-09 … So 2026-11-15
            for offset in 0..7 {
                let today = day(2026, 11, 9) + Duration::days(offset);
                let (c, built) = mem(lang, today);
                assert_eq!(built.tickets, data::tickets().len());
                let n: i64 = c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
                assert_eq!(n as usize, built.tickets, "{lang:?} {today}");
                // tools/readme-shots.ps1 öffnet dieses Ticket im Seitenpanel
                assert_eq!(built.keys.get("e").map(String::as_str), Some("SHOP-23"));
                // „heute“ liegt im aktiven Sprint, der Burndown hat Verlauf
                let s = crate::planning::sprints::sprint::get_sprint(&c, built.s12_id).unwrap();
                assert!(s.start_date.unwrap() <= built.today && built.today <= s.end_date.unwrap(), "{today}: {s:?}");
                let series = burndown_for_sprint(&c, built.s12_id, crate::calc::progress::Basis::Points, WorkCalendar::default(), built.today).unwrap();
                assert!(series.actual.len() >= 6, "{today}: Ist-Linie hat nur {} Punkte", series.actual.len());
                // die Abfragen der gespeicherten Filter sind gültig und treffen etwas
                let ctx = Ctx::new(&built.today.format("%Y-%m-%d").to_string(), lang);
                for f in crate::planning::views::filters::list_filters(&c, Some(built.project_id)).unwrap() {
                    let hits = count(&c, &ctx, Scope::Project(built.project_id), &f.jql).unwrap_or_else(|e| panic!("{}: {e:?}", f.jql));
                    assert!(hits > 0, "{}: keine Treffer", f.jql);
                }
                let totals = sprint_totals(&c, built.s12_id).unwrap();
                assert!(totals.total.tickets >= 10);
            }
        }
    }

    /// Erzeugt die Demo-Datenbank in `PANELDECK_DEMO_DIR` (siehe Moduldoku).
    #[test]
    #[ignore = "erzeugt Dateien für die Screenshots; mit --ignored und PANELDECK_DEMO_DIR starten"]
    fn demo_shots() {
        let dir = std::env::var("PANELDECK_DEMO_DIR").expect("PANELDECK_DEMO_DIR fehlt");
        let lang = match std::env::var("PANELDECK_DEMO_LANG").as_deref() {
            Ok("de") => Lang::De,
            _ => Lang::En,
        };
        let today = std::env::var("PANELDECK_DEMO_TODAY").ok().and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()).unwrap_or_else(|| chrono::Local::now().date_naive());
        let built = write_data_dir(std::path::Path::new(&dir), lang, today).unwrap_or_else(|e| panic!("{e}"));
        println!("Demo-Datenbank: {dir} ({} Tickets, Sprint 12 ab {}, heute {}, Beispiel-Ticket {})", built.tickets, built.s12_start, built.today, built.keys.get("e").cloned().unwrap_or_default());
    }
}
