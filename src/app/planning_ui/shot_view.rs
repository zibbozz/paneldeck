//! Nur mit dem Cargo-Feature `readme-shots` (nie im Release): Startansicht für die README-Screenshots.
//!
//! Ist die Umgebungsvariable `PANELDECK_SHOT` gesetzt, springt die Anwendung beim Start in den Reiter „Todo-Liste“ (Expertenmodus,
//! erstes echtes Projekt) und zeigt die genannte Ansicht – ganz ohne Maus und Tastatur. Werte (optional mit `:`-Argument):
//! `tickets[:SCHLÜSSEL]` (Liste mit Seitenpanel des Tickets), `board`, `backlog`, `releases`, `dashboard`, `overview`, `search`.
//! Siehe `tools/readme-shots.ps1`.

use super::ticket_state::ViewMode;
use super::{Cx, PlanUi, Tab};
use crate::db::Db;
use crate::planning::tickets::ticket_by_key;
use crate::state::Lang;
use std::sync::atomic::{AtomicBool, Ordering};

static DONE: AtomicBool = AtomicBool::new(false);

/// Der Wert von `PANELDECK_SHOT`, solange die Ansicht noch nicht angewendet wurde.
pub(in crate::app) fn pending_spec() -> Option<String> {
    if DONE.load(Ordering::Relaxed) {
        return None;
    }
    std::env::var("PANELDECK_SHOT").ok().filter(|s| !s.trim().is_empty())
}

impl PlanUi {
    /// Wendet die Startansicht an, sobald die Projekte geladen sind (einmal).
    pub(in crate::app) fn dev_view(&mut self, db: &Db, lang: Lang) {
        let Some(spec) = pending_spec() else { return };
        if self.projects.is_empty() {
            return;
        }
        DONE.store(true, Ordering::Relaxed);
        let cx = Cx { conn: &db.conn, db, lang };
        if !self.expert {
            self.set_expert(db, true);
        }
        if let Some(id) = self.projects.iter().find(|p| !p.system).map(|p| p.id) {
            self.select(&cx, id);
            // der Ticket-Bereich merkt sich sein Projekt; sonst räumt `ticket_refresh` das geöffnete Ticket beim ersten Bild wieder weg
            self.tickets.switch_project(id);
        }
        let (view, arg) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        match view {
            "board" => self.tab = Tab::Boards,
            "backlog" => self.tab = Tab::Backlog,
            "releases" => self.tab = Tab::Releases,
            "dashboard" => self.tab = Tab::Dashboards,
            "overview" => self.tab = Tab::Overview,
            "search" => self.tab = Tab::Search,
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
}
