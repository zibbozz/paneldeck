//! Daten der Dashboard-Kacheln: jede Kachel wird hier aus der Datenbank gelesen (über `planning::search`, die Sprints-Schicht und die
//! Zeiterfassung) und in ein [`TileData`] gepackt, das nur noch gezeichnet werden muss. Fehler einer Kachel (ungültige Abfrage,
//! Projekt fehlt, Sprints aus …) werden **in die Daten** geschrieben ([`TileData::Error`], zweisprachig) und nie nach außen gereicht –
//! eine kaputte Kachel stört die anderen nicht. Reine Lesefunktionen auf `&Connection`, keine Oberfläche.

use super::dash_logic::{self as logic, Bucket, Group, FAVORITES_JQL, FLAGGED_JQL, LIST_COLUMNS, MAX_GROUPS};
use crate::calc::burndown::BurndownSeries;
use crate::calc::calendar::WorkCalendar;
use crate::calc::duration::format_duration;
use crate::calc::progress::Basis;
use crate::planning::projects::{get_project, list_projects};
use crate::planning::search::{count, run_query, Ctx, Page, Scope, TicketRow};
use crate::planning::sprints::sprint::{active_sprint, sprint_content};
use crate::planning::sprints::{burndown_for_sprint, velocity_for_project};
use crate::planning::time_entries::time_by_day_in;
use crate::planning::views::dashboards::{CountGroup, TileKind};
use crate::state::Lang;
use chrono::NaiveDate;
use rusqlite::Connection;
use std::time::Duration;

/// Höchstzahl der Zeilen einer Such-Kachel (die Gesamtzahl steht trotzdem dabei) und der festen Listen.
pub const SEARCH_LIMIT: i64 = 100;
pub const LIST_LIMIT: i64 = 50;
/// Zeitlimit einer Abfrage im Dashboard (kürzer als das Standard-Limit, damit eine teure Abfrage die Oberfläche nicht lange anhält).
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
/// Wie viele abgeschlossene Sprints die mittlere Velocity einbeziehen.
const VELOCITY_SPRINTS: usize = 5;
/// Arbeitstage-Rechnung des Dashboards: Wochenende frei.
fn calendar() -> WorkCalendar {
    WorkCalendar::with_weekend_switch(true)
}

/// Ein Projekt in der Projekt-Kachel.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectRow {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub color: String,
    pub tickets: i64,
}

/// Der Sprint-Fortschritt.
#[derive(Clone, Debug, PartialEq)]
pub struct SprintView {
    pub project_key: String,
    pub name: String,
    pub goal: String,
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
    /// Erledigt und insgesamt, gewichtet nach `unit` (Punkte, sonst Tickets).
    pub done: f64,
    pub total: f64,
    pub points: bool,
    pub done_tickets: u32,
    pub total_tickets: u32,
    /// Arbeitstage von heute bis zum Ende einschließlich; `None` ohne Enddatum, 0 nach dem Ende.
    pub work_days_left: Option<u32>,
    /// Mittlere Velocity der letzten abgeschlossenen Sprints (erledigtes Gewicht), falls es welche gibt.
    pub velocity: Option<f64>,
}

impl SprintView {
    /// Anteil erledigt (0 bis 1); ohne Umfang 0.
    pub fn fraction(&self) -> f32 {
        if self.total > 0.0 { (self.done / self.total).clamp(0.0, 1.0) as f32 } else { 0.0 }
    }
}

/// Die Linien des Burndowns (nur Arbeitstage) und die Kennzahlen.
#[derive(Clone, Debug, PartialEq)]
pub struct BurnView {
    pub project_key: String,
    pub sprint: String,
    pub labels: Vec<String>,
    pub ideal: Vec<f64>,
    pub actual: Vec<Option<f64>>,
    pub points: bool,
    /// Rest heute (letzter bekannter Ist-Wert).
    pub remaining: Option<f64>,
    pub added: f64,
    pub removed: f64,
    pub completed: f64,
}

/// Was eine Kachel zeigt.
#[derive(Clone, Debug)]
pub enum TileData {
    /// Ticket-Liste (Such-, Favoriten- und Markierte-Kachel): die Zeilen, die Gesamtzahl und die Spalten.
    Rows { rows: Vec<TicketRow>, total: i64, columns: Vec<String> },
    Counts { groups: Vec<Group>, total: u32, group_by: CountGroup },
    Sprint(SprintView),
    Burndown(BurnView),
    Time { buckets: Vec<Bucket>, total: u32, project_key: Option<String> },
    Projects(Vec<ProjectRow>),
    /// Kein Fehler, aber nichts zu zeigen (z. B. kein aktiver Sprint).
    Info(String),
    /// Die Kachel konnte nicht geladen werden.
    Error(String),
}

// ------------------------------------------------------------------ Hintergrund-Leser

/// Ein Auftrag an den Hintergrund-Leser: eine Kachel mit allem, was zum Laden nötig ist.
pub struct Job {
    pub tile: u32,
    /// Schlüssel der Einstellungen und Stand der Daten zum Zeitpunkt des Auftrags (kommen mit dem Ergebnis zurück).
    pub key: u64,
    pub generation: u64,
    pub dash_id: i64,
    pub dash_project: Option<i64>,
    pub kind: TileKind<i64>,
    pub lang: Lang,
    pub today: NaiveDate,
}

/// Das Ergebnis eines Auftrags.
pub struct Done {
    pub tile: u32,
    pub key: u64,
    pub generation: u64,
    pub dash_id: i64,
    pub data: TileData,
}

/// Ein Hintergrund-Faden mit eigener, nur lesender Verbindung zur Datenbank: Er arbeitet die Aufträge der Reihe nach ab und weckt die
/// Oberfläche, wenn ein Ergebnis da ist. Große Projekte lassen die Oberfläche so nie warten. Endet, wenn der `Worker` verworfen wird.
pub struct Worker {
    tx: std::sync::mpsc::Sender<Job>,
    rx: std::sync::mpsc::Receiver<Done>,
}

fn open_reader(path: &std::path::Path) -> Result<Connection, String> {
    let c = Connection::open(path).map_err(|e| e.to_string())?;
    c.busy_timeout(Duration::from_secs(3)).map_err(|e| e.to_string())?;
    // Der Leser schreibt nie.
    c.pragma_update(None, "query_only", true).map_err(|e| e.to_string())?;
    Ok(c)
}

impl Worker {
    /// Startet den Faden für die Datenbank-Datei `path`; `wake` wird nach jedem Ergebnis aufgerufen. `None`, wenn der Faden nicht startet.
    pub fn spawn(path: std::path::PathBuf, wake: impl Fn() + Send + 'static) -> Option<Worker> {
        let (tx, jobs) = std::sync::mpsc::channel::<Job>();
        let (done_tx, rx) = std::sync::mpsc::channel::<Done>();
        std::thread::Builder::new()
            .name("dash-loader".into())
            .spawn(move || {
                let mut conn: Option<Result<Connection, String>> = None;
                for job in jobs {
                    let c = conn.get_or_insert_with(|| open_reader(&path));
                    let data = match c {
                        Ok(c) => Loader { conn: c, lang: job.lang, today: job.today, dash_project: job.dash_project }.load(&job.kind),
                        Err(e) => TileData::Error(format!("{}: {e}", job.lang.t("Datenbank konnte nicht geöffnet werden", "Could not open the database"))),
                    };
                    if done_tx.send(Done { tile: job.tile, key: job.key, generation: job.generation, dash_id: job.dash_id, data }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .ok()?;
        Some(Worker { tx, rx })
    }

    /// Gibt den Auftrag ab; `false`, wenn der Faden nicht mehr läuft.
    pub fn submit(&self, job: Job) -> bool {
        self.tx.send(job).is_ok()
    }

    /// Alle fertigen Ergebnisse.
    pub fn poll(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }
}

/// Womit geladen wird: Verbindung, Sprache, heutiges Datum und das Projekt des Dashboards (`None` = global).
pub struct Loader<'a> {
    pub conn: &'a Connection,
    pub lang: Lang,
    pub today: NaiveDate,
    pub dash_project: Option<i64>,
}

impl Loader<'_> {
    fn scope(&self) -> Scope {
        match self.dash_project {
            Some(p) => Scope::Project(p),
            None => Scope::All,
        }
    }

    fn ctx(&self) -> Ctx {
        Ctx::new(&self.today.format("%Y-%m-%d").to_string(), self.lang).with_timeout(QUERY_TIMEOUT)
    }

    /// Lädt die Daten einer Kachel; schlägt etwas fehl, steht der Fehler (zweisprachig) in [`TileData::Error`].
    pub fn load(&self, kind: &TileKind<i64>) -> TileData {
        match kind {
            TileKind::Search { jql, columns, .. } => self.rows(jql, columns.iter().map(String::as_str).collect(), SEARCH_LIMIT),
            TileKind::Favorites => self.rows(FAVORITES_JQL, LIST_COLUMNS.to_vec(), LIST_LIMIT),
            TileKind::Flagged => self.rows(FLAGGED_JQL, LIST_COLUMNS.to_vec(), LIST_LIMIT),
            TileKind::Counts { jql, group_by, .. } => self.counts(jql, *group_by),
            TileKind::Sprint { project_id } => self.sprint(project_id.or(self.dash_project)),
            TileKind::Burndown { project_id } => self.burndown(project_id.or(self.dash_project)),
            TileKind::TimeTracking { range, project_id } => self.time(*range, project_id.or(self.dash_project)),
            TileKind::Projects => self.projects(),
        }
    }

    fn rows(&self, jql: &str, columns: Vec<&str>, limit: i64) -> TileData {
        match run_query(self.conn, &self.ctx(), self.scope(), jql, Page::first(limit)) {
            Ok(p) => TileData::Rows { rows: p.rows, total: p.total, columns: columns.iter().map(|c| c.to_string()).collect() },
            Err(e) => TileData::Error(e.message(self.lang)),
        }
    }

    fn counts(&self, jql: &str, group_by: CountGroup) -> TileData {
        let lang = self.lang;
        match run_query(self.conn, &self.ctx(), self.scope(), jql, Page::ALL) {
            Ok(p) => {
                let total = p.rows.len() as u32;
                let groups = logic::group_counts(p.rows.iter().map(|r| logic::row_group(r, group_by)), lang.t("(ohne Wert)", "(no value)"), lang.t("Sonstige", "Other"), MAX_GROUPS);
                TileData::Counts { groups, total, group_by }
            }
            Err(e) => TileData::Error(e.message(lang)),
        }
    }

    fn projects(&self) -> TileData {
        let list = match list_projects(self.conn) {
            Ok(l) => l,
            Err(e) => return TileData::Error(e.message(self.lang)),
        };
        let ctx = self.ctx();
        let rows = list
            .into_iter()
            .filter(|p| !p.system)
            .map(|p| ProjectRow { tickets: count(self.conn, &ctx, Scope::Project(p.id), "").unwrap_or(0), id: p.id, key: p.key, name: p.name, color: p.color })
            .collect();
        TileData::Projects(rows)
    }

    /// Das Projekt der Sprint-/Burndown-Kachel samt aktivem Sprint; sonst die fertige Meldung.
    fn active(&self, project: Option<i64>) -> Result<(crate::planning::projects::Project, crate::planning::sprints::Sprint), TileData> {
        let lang = self.lang;
        let Some(pid) = project else { return Err(TileData::Info(lang.t("Wähle in den Einstellungen der Kachel ein Projekt.", "Choose a project in the tile settings.").to_string())) };
        let p = get_project(self.conn, pid).map_err(|e| TileData::Error(e.message(lang)))?;
        if !p.use_sprints {
            return Err(TileData::Info(match lang {
                Lang::De => format!("Das Projekt {} nutzt keine Sprints.", p.key),
                Lang::En => format!("Project {} does not use sprints.", p.key),
            }));
        }
        match active_sprint(self.conn, pid) {
            Some(s) => Ok((p, s)),
            None => Err(TileData::Info(match lang {
                Lang::De => format!("Kein aktiver Sprint in {}.", p.key),
                Lang::En => format!("No active sprint in {}.", p.key),
            })),
        }
    }

    fn sprint(&self, project: Option<i64>) -> TileData {
        let lang = self.lang;
        let (p, s) = match self.active(project) {
            Ok(x) => x,
            Err(d) => return d,
        };
        let content = match sprint_content(self.conn, s.id) {
            Ok(c) => c,
            Err(e) => return TileData::Error(e.message(lang)),
        };
        let t = content.totals;
        let points = t.total.points > 0.0;
        let basis = if points { Basis::Points } else { Basis::TicketCount };
        let work_days_left = s.end_date.map(|end| if self.today > end { 0 } else { calendar().work_days_inclusive(self.today, end) });
        let velocity = velocity_for_project(self.conn, p.id, VELOCITY_SPRINTS, if points { Basis::Points } else { Basis::TicketCount })
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| v.iter().map(|x| x.completed).sum::<f64>() / v.len() as f64);
        TileData::Sprint(SprintView {
            project_key: p.key,
            name: s.name,
            goal: s.goal,
            start: s.start_date,
            end: s.end_date,
            done: t.done.weight(basis),
            total: t.total.weight(basis),
            points,
            done_tickets: t.done.tickets,
            total_tickets: t.total.tickets,
            work_days_left,
            velocity,
        })
    }

    fn burndown(&self, project: Option<i64>) -> TileData {
        let lang = self.lang;
        let (p, s) = match self.active(project) {
            Ok(x) => x,
            Err(d) => return d,
        };
        // Punkte, wenn der Sprint welche hat, sonst Tickets (wie die Sprint-Kachel)
        let points = sprint_content(self.conn, s.id).map(|c| c.totals.total.points > 0.0).unwrap_or(false);
        let basis = if points { Basis::Points } else { Basis::TicketCount };
        let cal = calendar();
        match burndown_for_sprint(self.conn, s.id, basis, cal.clone(), self.today) {
            Ok(series) => {
                let (days, ideal, actual) = work_day_points(&series, &cal);
                TileData::Burndown(BurnView {
                    project_key: p.key,
                    sprint: s.name,
                    labels: days.iter().map(|d| logic::short_date(*d, lang)).collect(),
                    remaining: actual.iter().rev().find_map(|a| *a),
                    ideal,
                    actual,
                    points,
                    added: series.scope_added,
                    removed: series.scope_removed,
                    completed: series.completed,
                })
            }
            Err(e) => TileData::Error(e.message(lang)),
        }
    }

    fn time(&self, range: crate::planning::views::dashboards::TimeRange, project: Option<i64>) -> TileData {
        let lang = self.lang;
        let key = match project {
            Some(p) => match get_project(self.conn, p) {
                Ok(p) => Some(p.key),
                Err(e) => return TileData::Error(e.message(lang)),
            },
            None => None,
        };
        let (from, to) = logic::time_window(self.today, range);
        match time_by_day_in(self.conn, project, from, to) {
            Ok(map) => {
                let buckets = logic::bucket_time(&map, self.today, range, lang);
                let total = buckets.iter().fold(0u32, |a, b| a.saturating_add(b.minutes));
                TileData::Time { buckets, total, project_key: key }
            }
            Err(e) => TileData::Error(e.message(lang)),
        }
    }
}

/// Nur die Arbeitstage des Burndowns (die freien Tage bleiben auf der Achse weg): Tage, Ideal- und Ist-Werte.
pub fn work_day_points(series: &BurndownSeries, cal: &WorkCalendar) -> (Vec<NaiveDate>, Vec<f64>, Vec<Option<f64>>) {
    let mut days = Vec::new();
    let mut ideal = Vec::new();
    let mut actual = Vec::new();
    for (i, d) in series.days.iter().enumerate() {
        if cal.is_work_day(*d) {
            days.push(*d);
            ideal.push(series.ideal.get(i).copied().unwrap_or(0.0));
            actual.push(series.actual.get(i).copied().flatten());
        }
    }
    (days, ideal, actual)
}

/// Gebuchte Zeit kurz („1d 2h 30m“ bei 8-Stunden-Tag).
pub fn duration_text(minutes: u32, lang: Lang) -> String {
    format_duration(minutes, 8.0, lang)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::history::Source;
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::sprints::sprint::{create_sprint, move_tickets_to_sprint, start_sprint, NewSprint};
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::{set_flagged, set_starred};
    use crate::planning::transition::transition_ticket;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn loader<'a>(f: &'a Fx, project: Option<i64>) -> Loader<'a> {
        Loader { conn: &f.c, lang: Lang::En, today: day(2026, 11, 11), dash_project: project }
    }

    fn rows(d: &TileData) -> (&Vec<TicketRow>, i64) {
        match d {
            TileData::Rows { rows, total, .. } => (rows, *total),
            other => panic!("keine Liste: {other:?}"),
        }
    }

    /// Projekt PAN mit vier Tickets: Task (Done), Task (To Do, Stern), Story (In Progress, Markierung), Defect (To Do).
    fn fx() -> Fx {
        let f = Fx::new();
        let a = f.mk(f.task, "A done");
        transition_ticket(&f.c, a.id, f.done, &[], &Source::ui()).unwrap();
        let b = f.mk(f.task, "B starred");
        set_starred(&f.c, b.id, true, &Source::ui()).unwrap();
        let c = f.mk(f.story, "C flagged");
        transition_ticket(&f.c, c.id, f.progress, &[], &Source::ui()).unwrap();
        set_flagged(&f.c, c.id, true, &Source::ui()).unwrap();
        f.mk(f.defect, "D");
        f
    }

    #[test]
    fn search_tile_runs_the_query_and_keeps_the_total() {
        let f = fx();
        let l = loader(&f, Some(f.project));
        let d = l.load(&TileKind::Search { jql: String::new(), columns: vec!["key".into()], title: String::new() });
        let (r, total) = rows(&d);
        assert_eq!((r.len(), total), (4, 4));
        let d = l.load(&TileKind::Search { jql: r#"type = "Dev task" ORDER BY key"#.into(), columns: vec![], title: String::new() });
        let (r, total) = rows(&d);
        assert_eq!((r.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), total), (vec!["A done", "B starred"], 2));
        assert!(matches!(&d, TileData::Rows { columns, .. } if columns.is_empty()), "leere Spalten bleiben leer – die Oberfläche setzt die Standardspalten");
    }

    #[test]
    fn invalid_query_becomes_a_tile_error_in_both_languages() {
        let f = fx();
        let kind = TileKind::<i64>::Search { jql: "type = ".into(), columns: vec![], title: String::new() };
        let en = loader(&f, None).load(&kind);
        let de = Loader { lang: Lang::De, ..loader(&f, None) }.load(&kind);
        let (TileData::Error(a), TileData::Error(b)) = (&en, &de) else { panic!("{en:?} {de:?}") };
        assert!(!a.is_empty() && !b.is_empty());
        assert_ne!(a, b, "Fehlertexte sind übersetzt");
        // unbekanntes Feld, zu lange Abfrage: ebenfalls Fehler, kein Absturz
        for jql in ["nonsense = 1".to_string(), "x".repeat(5000)] {
            assert!(matches!(loader(&f, None).load(&TileKind::<i64>::Search { jql, columns: vec![], title: String::new() }), TileData::Error(_)));
        }
        assert!(matches!(loader(&f, None).load(&TileKind::<i64>::Counts { jql: "((".into(), group_by: CountGroup::Status, chart: crate::planning::views::dashboards::ChartKind::Pie }), TileData::Error(_)));
    }

    #[test]
    fn favorites_and_flagged_tiles() {
        let f = fx();
        let l = loader(&f, Some(f.project));
        let d = l.load(&TileKind::Favorites);
        assert_eq!(rows(&d).0.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), vec!["B starred"]);
        let d = l.load(&TileKind::Flagged);
        assert_eq!(rows(&d).0.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), vec!["C flagged"]);
        assert!(matches!(&d, TileData::Rows { columns, .. } if columns.len() == LIST_COLUMNS.len()));
    }

    #[test]
    fn counts_group_by_status_type_and_priority() {
        let f = fx();
        let l = loader(&f, Some(f.project));
        let counts = |g| match l.load(&TileKind::Counts { jql: String::new(), group_by: g, chart: crate::planning::views::dashboards::ChartKind::Bar }) {
            TileData::Counts { groups, total, group_by } => {
                assert_eq!(group_by, g);
                (groups, total)
            }
            other => panic!("{other:?}"),
        };
        let (g, total) = counts(CountGroup::Status);
        assert_eq!(total, 4);
        assert_eq!(g.iter().map(|x| x.count).sum::<u32>(), 4);
        assert!(g.iter().any(|x| x.name == "To Do" && x.count == 2), "{g:?}");
        assert!(g.iter().all(|x| x.color.is_some() || x.none));
        let (g, _) = counts(CountGroup::Type);
        assert!(g.iter().any(|x| x.count == 2), "zwei Tasks: {g:?}");
        assert_eq!(g.iter().map(|x| x.count).sum::<u32>(), 4);
        let (g, _) = counts(CountGroup::Priority);
        assert_eq!(g.iter().map(|x| x.count).sum::<u32>(), 4, "auch Tickets ohne Priorität zählen mit");
        // mit Abfrage eingeschränkt
        let d = l.load(&TileKind::Counts { jql: r#"type = "Dev task""#.into(), group_by: CountGroup::Status, chart: crate::planning::views::dashboards::ChartKind::Pie });
        assert!(matches!(d, TileData::Counts { total: 2, .. }));
    }

    #[test]
    fn global_dashboards_search_all_projects_and_projects_tile_lists_them() {
        let f = fx();
        let other = f.other_project("OTH");
        crate::planning::tickets::create_ticket(&f.c, &crate::planning::tickets::NewTicket::new(other, crate::planning::types::list_types(&f.c, other).unwrap()[0].id, "In other"), &Source::ui()).unwrap();
        let all = loader(&f, None).load(&TileKind::Search { jql: String::new(), columns: vec![], title: String::new() });
        assert_eq!(rows(&all).1, 5);
        let one = loader(&f, Some(f.project)).load(&TileKind::Search { jql: String::new(), columns: vec![], title: String::new() });
        assert_eq!(rows(&one).1, 4, "ein Projekt-Dashboard sucht nur in seinem Projekt");
        match loader(&f, None).load(&TileKind::Projects) {
            TileData::Projects(p) => {
                assert_eq!(p.iter().map(|x| (x.key.as_str(), x.tickets)).collect::<Vec<_>>(), vec![("PAN", 4), ("OTH", 1)], "ohne das System-Projekt der Todolist");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sprint_tiles_explain_why_they_are_empty() {
        let f = fx();
        let l = loader(&f, Some(f.project));
        let info = |d: TileData| match d {
            TileData::Info(s) => s,
            other => panic!("{other:?}"),
        };
        // Sprints aus
        assert!(info(l.load(&TileKind::Sprint { project_id: None })).contains("does not use sprints"));
        assert!(info(l.load(&TileKind::Burndown { project_id: None })).contains("does not use sprints"));
        // global ohne Projekt
        assert!(info(loader(&f, None).load(&TileKind::Sprint { project_id: None })).contains("Choose a project"));
        // an, aber kein aktiver Sprint
        update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
        assert!(info(l.load(&TileKind::Sprint { project_id: None })).contains("No active sprint"));
        assert_eq!(info(Loader { lang: Lang::De, ..loader(&f, Some(f.project)) }.load(&TileKind::Sprint { project_id: None })), "Kein aktiver Sprint in PAN.");
        // unbekanntes Projekt: Fehler in der Kachel
        assert!(matches!(l.load(&TileKind::Sprint { project_id: Some(9999) }), TileData::Error(_)));
        // ein globales Dashboard kann ein Projekt ausdrücklich wählen
        assert!(info(loader(&f, None).load(&TileKind::Sprint { project_id: Some(f.project) })).contains("No active sprint"));
    }

    #[test]
    fn sprint_and_burndown_tiles_show_the_active_sprint() {
        let f = fx();
        update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
        let cal = calendar();
        let mut n = NewSprint::new(f.project, "Sprint 1");
        n.start = Some(day(2026, 11, 9));
        n.end = Some(day(2026, 11, 20));
        n.goal = "Ship it".into();
        let sid = create_sprint(&f.c, &n, &cal).unwrap();
        let ids: Vec<i64> = crate::planning::search::run_query(&f.c, &Ctx::new("2026-11-11", Lang::En), Scope::Project(f.project), r#"type = "Dev task""#, Page::ALL).unwrap().rows.iter().map(|r| r.id).collect();
        move_tickets_to_sprint(&f.c, sid, &ids).unwrap();
        start_sprint(&f.c, sid, &cal).unwrap();
        let l = loader(&f, Some(f.project));
        match l.load(&TileKind::Sprint { project_id: None }) {
            TileData::Sprint(s) => {
                assert_eq!((s.project_key.as_str(), s.name.as_str(), s.goal.as_str()), ("PAN", "Sprint 1", "Ship it"));
                assert_eq!((s.total_tickets, s.done_tickets), (2, 1));
                assert!(!s.points, "keine Punkte → es zählen Tickets");
                assert_eq!((s.done, s.total), (1.0, 2.0));
                assert!((s.fraction() - 0.5).abs() < 1e-6);
                assert_eq!(s.work_days_left, Some(8), "Mi 11.11. bis Fr 20.11.: 8 Arbeitstage");
                assert_eq!(s.velocity, None, "noch kein abgeschlossener Sprint");
            }
            other => panic!("{other:?}"),
        }
        match l.load(&TileKind::Burndown { project_id: None }) {
            TileData::Burndown(b) => {
                assert_eq!((b.project_key.as_str(), b.sprint.as_str()), ("PAN", "Sprint 1"));
                assert_eq!(b.labels.len(), 10, "zwei Wochen ohne Wochenenden");
                assert_eq!(b.ideal.len(), 10);
                assert_eq!(b.actual.len(), 10);
                assert!(b.actual[0].is_some() && b.actual[9].is_none(), "Ist-Linie endet heute: {:?}", b.actual);
                assert_eq!(b.labels[0], "Nov 9");
                assert!(b.ideal[0] >= b.ideal[9] && b.ideal[9] == 0.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn time_tile_buckets_the_booked_time() {
        use crate::planning::time_entries::log_time;
        use crate::planning::views::dashboards::TimeRange;
        let f = fx();
        let t = f.mk(f.task, "Booked");
        let noon = |m: u32, d: u32| day(2026, m, d).and_hms_opt(12, 0, 0).unwrap();
        log_time(&f.c, t.id, "1h 30m", None, None, "", noon(11, 11), &Source::ui()).unwrap();
        log_time(&f.c, t.id, "30m", None, None, "", noon(11, 10), &Source::ui()).unwrap();
        log_time(&f.c, t.id, "2h", None, None, "", noon(10, 20), &Source::ui()).unwrap(); // älter als 14 Tage
        let l = loader(&f, None);
        match l.load(&TileKind::TimeTracking { range: TimeRange::Day, project_id: None }) {
            TileData::Time { buckets, total, project_key } => {
                assert_eq!((buckets.len(), total, project_key), (14, 120, None));
                assert_eq!((buckets[13].minutes, buckets[12].minutes), (90, 30));
            }
            other => panic!("{other:?}"),
        }
        match l.load(&TileKind::TimeTracking { range: TimeRange::Week, project_id: Some(f.project) }) {
            TileData::Time { buckets, total, project_key } => {
                assert_eq!((buckets.len(), total, project_key.as_deref()), (8, 240, Some("PAN")), "die Wochen reichen weiter zurück");
                assert_eq!(buckets[7].minutes, 120);
            }
            other => panic!("{other:?}"),
        }
        // Ein Projekt-Dashboard zählt nur sein Projekt
        let other = f.other_project("OTH");
        match loader(&f, Some(other)).load(&TileKind::TimeTracking { range: TimeRange::Day, project_id: None }) {
            TileData::Time { total, project_key, .. } => assert_eq!((total, project_key.as_deref()), (0, Some("OTH"))),
            other => panic!("{other:?}"),
        }
        assert!(matches!(l.load(&TileKind::TimeTracking { range: TimeRange::Day, project_id: Some(9999) }), TileData::Error(_)));
    }

    #[test]
    fn work_day_points_drop_the_weekend() {
        let start = day(2026, 11, 6); // Freitag
        let days: Vec<NaiveDate> = (0..5).map(|i| start + chrono::Duration::days(i)).collect(); // Fr Sa So Mo Di
        let series = BurndownSeries { days, ideal: vec![4.0, 3.0, 2.0, 1.0, 0.0], actual: vec![Some(4.0), Some(3.0), Some(3.0), None, None], scope_total_start: 4.0, scope_added: 0.0, scope_removed: 0.0, completed: 1.0 };
        let (d, i, a) = work_day_points(&series, &calendar());
        assert_eq!(d, vec![day(2026, 11, 6), day(2026, 11, 9), day(2026, 11, 10)]);
        assert_eq!(i, vec![4.0, 1.0, 0.0]);
        assert_eq!(a, vec![Some(4.0), None, None]);
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(duration_text(0, Lang::En), "0m");
        assert_eq!(duration_text(90, Lang::En), "1h 30m");
        assert_eq!(duration_text(8 * 60 + 15, Lang::De), "1d 15m");
    }
}
