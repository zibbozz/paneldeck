//! Nur mit dem Cargo-Feature `readme-shots` (nie im Release): Startansicht für die README-/Doku-Screenshots.
//!
//! Ist die Umgebungsvariable `PANELDECK_SHOT` auf eine Ansicht der Todo-Liste gesetzt ([`super::super::shot_start::PLAN_VIEWS`]),
//! springt die Anwendung beim Start in den Reiter „Todo-Liste“ (Expertenmodus, erstes echtes Projekt) und zeigt die
//! genannte Ansicht – ganz ohne Maus und Tastatur. Werte (optional mit `:`-Argument):
//! `tickets[:SCHLÜSSEL]` (Liste mit Seitenpanel des Tickets), `board`, `backlog`, `releases`, `dashboard`, `overview`, `search`,
//! `search-example:<id>` (Suche mit der Abfrage des Hilfe-Beispiels `<id>` ausgeführt; `linkedTerminal()` ist dann das Terminal der
//! Beispieldaten, siehe [`terminals`]), `maint-page` (Wartungsseite, Kategorie Tickets), `maint-fix` (Vorschau „Als Won't Do
//! schließen“ für die Befunde „lange nicht bearbeitet“), `maint-duplicate` (Vergleich eines möglichen Duplikats),
//! `maint-database` (Kategorie Datenbank mit Statistik).
//! Die übrigen Ansichten (Terminal Hub, Editor, Einstellungen …) stehen in `app/shot_start.rs`.
//! Siehe `tools/docs-shots.ps1` und `tools/query-help-shots.ps1`.

use super::maint_view::Pane;
use super::ticket_state::ViewMode;
use super::{Cx, PlanUi, Tab};
use crate::db::Db;
use crate::planning::maintenance::{Category, Check, Fix};
use crate::planning::tickets::ticket_by_key;
use crate::state::Lang;
use crate::planning::query_help::HelpDoc;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

static DONE: AtomicBool = AtomicBool::new(false);
/// Das „aktuelle Terminal“ der Suche bei `search-example:<id>` (die echte Suche der Oberfläche kennt keins).
static TERMINALS: OnceLock<Vec<String>> = OnceLock::new();
/// Was nach der vollen Prüfung der Wartung noch geöffnet wird (`maint-fix`, `maint-duplicate`, `maint-database`).
static AFTER_REPORT: Mutex<Option<&'static str>> = Mutex::new(None);

/// Terminals für `Ctx::with_terminals` der Suche (leer außer bei `search-example:<id>`).
pub(in crate::app) fn terminals() -> Vec<String> {
    TERMINALS.get().cloned().unwrap_or_default()
}

/// Der Wert von `PANELDECK_SHOT`, solange die Ansicht der Todo-Liste noch nicht angewendet wurde (andere Ansichten: `None`).
pub(in crate::app) fn pending_spec() -> Option<String> {
    if DONE.load(Ordering::Relaxed) {
        return None;
    }
    let s = std::env::var("PANELDECK_SHOT").ok().filter(|s| !s.trim().is_empty())?;
    let view = s.split_once(':').map_or(s.as_str(), |(v, _)| v);
    super::super::shot_start::is_plan_view(view).then_some(s)
}

impl PlanUi {
    /// Wendet die Startansicht an, sobald die Projekte geladen sind (einmal); danach je Bild die Schritte, die auf die
    /// Prüfung der Wartung warten.
    pub(in crate::app) fn dev_view(&mut self, db: &Db, lang: Lang) {
        let Some(spec) = pending_spec() else {
            self.dev_view_after(db, lang);
            return;
        };
        if self.projects.is_empty() {
            return;
        }
        DONE.store(true, Ordering::Relaxed);
        let cx = Cx { conn: &db.conn, db, lang };
        if !self.expert {
            self.set_expert(db, true);
        }
        let Some(id) = self.projects.iter().find(|p| !p.system).map(|p| p.id) else { return };
        self.select(&cx, id);
        // der Ticket-Bereich merkt sich sein Projekt; sonst räumt `ticket_refresh` das geöffnete Ticket beim ersten Bild wieder weg
        self.tickets.switch_project(id);
        let (view, arg) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        match view {
            "board" => self.tab = Tab::Boards,
            "backlog" => self.tab = Tab::Backlog,
            "releases" => self.tab = Tab::Releases,
            "dashboard" => self.tab = Tab::Dashboards,
            "overview" => self.tab = Tab::Overview,
            "search" => self.tab = Tab::Search,
            "search-example" => {
                if let Some(e) = HelpDoc::builtin().example(arg) {
                    let _ = TERMINALS.set(e.demo.current_terminal.iter().cloned().collect());
                    self.open_search_query(&e.jql, Some(false));
                } else {
                    self.tab = Tab::Search;
                }
            }
            "maint-page" | "maint-fix" | "maint-duplicate" | "maint-database" => {
                self.tab = Tab::Overview;
                let cat = if view == "maint-database" { Category::Database } else { Category::Tickets };
                self.open_maintenance(&cx, id, Some(Pane::Cat(cat)));
                if view != "maint-page" {
                    *AFTER_REPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(match view {
                        "maint-fix" => "fix",
                        "maint-duplicate" => "duplicate",
                        _ => "stats",
                    });
                }
            }
            _ => {
                self.tab = Tab::Tickets;
                if let Ok(t) = ticket_by_key(cx.conn, arg) {
                    self.tickets.mode = ViewMode::Panel;
                    self.tickets.mode_loaded = true;
                    self.open_ticket_anywhere(&cx, t.id, t.project_id);
                }
            }
        }
        self.dirty = true;
    }

    /// Nach der vollen Prüfung der Wartung: Vorschau, Vergleich oder Statistik öffnen (einmal).
    fn dev_view_after(&mut self, db: &Db, lang: Lang) {
        let mut want = AFTER_REPORT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(what) = *want else { return };
        if self.maint.job.is_some() || self.maint.report.is_none() {
            return;
        }
        *want = None;
        drop(want);
        let cx = Cx { conn: &db.conn, db, lang };
        let findings = self.maint.report.as_ref().map(|r| r.findings.clone()).unwrap_or_default();
        match what {
            "fix" => {
                // Befunde mit „Als Won't Do schließen“, am liebsten alle derselben Prüfung (eine Begründung im Kommentar).
                let all: Vec<_> = findings.iter().filter(|f| f.fixes.contains(&Fix::CloseWontDo)).collect();
                let check = [Check::StaleNew, Check::StaleInProgress].into_iter().find(|c| all.iter().any(|f| f.check == *c));
                let pick: Vec<_> = all.iter().copied().filter(|f| check.is_none_or(|c| f.check == c)).take(4).collect();
                for f in &pick {
                    self.maint.selected.insert(f.id.clone());
                }
                if !pick.is_empty() {
                    self.maint_open_fix(&cx, Fix::CloseWontDo, &pick);
                }
            }
            "duplicate" => {
                if let Some(f) = findings.iter().find(|f| f.check == Check::PossibleDuplicate) {
                    self.maint_open_fix(&cx, Fix::LinkDuplicate, &[f]);
                }
            }
            _ => self.maint_spawn_stats(&cx),
        }
        self.dirty = true;
    }
}
