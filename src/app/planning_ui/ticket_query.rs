//! Abfrage-Leiste über der Ticket-Liste (Reiter „Tickets“).
//!
//! * Im Normalzustand steht die Leiste unter der ersten Zeile (Neu, Suche, Stern/Markierung) und über den Typ-/Status-Chips. Schickt man
//!   eine **gültige, nicht leere** Abfrage ab, wird sie aktiv.
//! * Ist eine Abfrage aktiv, **ersetzt sie die lokalen Filter** (Chips, Stern, Markierung, Suchfeld): die Liste lädt dann über
//!   `planning::search::run_query` (im Hintergrund, mit Seiten zu 100 Tickets) und zeigt dieselben Spalten wie die lokale Liste, sortiert
//!   mit derselben Logik (Standard: neueste zuerst). „Abfrage löschen“ kehrt zur lokalen Liste zurück.
//! * Der Bereich ist immer das gewählte Projekt. Ein Wechsel des Projekts löscht die Abfrage.
//!
//! **Einhängepunkte in `ticket_tab.rs`:** am Anfang von `tickets_tab` (`tq_sync`, `tq_active` → `tq_tab`), nach der ersten Zeile
//! (`tq_bar_row`) und in `ticket_refresh` (`tq_data_changed`, wenn die Daten sich geändert haben und eine Abfrage aktiv ist).

use super::query_bar::{QueryBarCtx, QueryBarState, query_bar};
use super::search_logic::SortBy;
use super::search_run::SchemaCache;
use super::search_table::{Results, ResultsCfg};
use super::widgets;
use super::{Cx, PlanUi};
use crate::planning::history::Source;
use crate::planning::projects::Project;
use crate::planning::search::Scope;
use crate::planning::tickets::{set_flagged, set_starred};
use crate::style::{self, S1, S2};
use eframe::egui::{Align, Layout, RichText, Ui};

/// Schlüssel in `meta`, unter dem der Verlauf der Abfragen liegt (gemeinsam mit dem Reiter „Suche“).
const HISTORY_META: &str = "query_history";

/// Zustand der Abfrage der Ticket-Liste.
pub(super) struct State {
    pub bar: QueryBarState,
    schema: SchemaCache,
    /// Die aktive Abfrage (gültig abgeschickt, nicht leer); `None` = lokale Filter gelten.
    pub active: Option<String>,
    pub res: Results,
    /// Projekt, zu dem der Zustand gehört.
    project: i64,
    history_loaded: bool,
}

impl Default for State {
    fn default() -> Self {
        State { bar: QueryBarState::new("ticket_query"), schema: SchemaCache::default(), active: None, res: Results::new(Some(SortBy::NEWEST)), project: 0, history_loaded: false }
    }
}

impl State {
    /// Abfrage aktivieren (oder, bei leerem Text, abschalten); `true`, wenn sich der Zustand geändert hat.
    fn submit(&mut self, q: &str) -> bool {
        let q = q.trim();
        if q.is_empty() {
            return self.clear();
        }
        self.active = Some(q.to_string());
        self.res.restart();
        true
    }

    /// Abfrage abschalten; `true`, wenn eine aktiv war.
    fn clear(&mut self) -> bool {
        let was = self.active.take().is_some();
        self.res.run.clear();
        self.res.page = 0;
        was
    }
}

impl PlanUi {
    /// Eine Abfrage ist aktiv (die lokalen Filter ruhen).
    pub(super) fn tq_active(&self) -> bool {
        self.tq.active.is_some()
    }

    /// Beim Projektwechsel die Abfrage vergessen.
    pub(super) fn tq_sync(&mut self, project: i64) {
        if self.tq.project != project {
            self.tq.project = project;
            self.tq.active = None;
            self.tq.bar.text.clear();
            self.tq.res.run.clear();
            self.tq.res.page = 0;
            self.tq.schema.invalidate();
        }
    }

    /// Die Daten haben sich geändert: Schema und Ergebnis neu holen.
    pub(super) fn tq_data_changed(&mut self) {
        self.tq.schema.invalidate();
        self.tq.res.run.mark_stale();
    }

    /// Zeichnet die Leiste; gibt die abgeschickte Abfrage zurück. `None`, wenn das Schema fehlt.
    fn tq_bar(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) -> Option<String> {
        if !self.tq.history_loaded {
            self.tq.history_loaded = true;
            if let Some(h) = cx.db.meta(HISTORY_META) {
                self.tq.bar.history = super::query_logic::History::from_json(&h);
            }
        }
        self.tq.schema.ensure(cx.conn, Scope::Project(proj.id), cx.lang);
        let Some(loaded) = self.tq.schema.loaded() else {
            if let Some(e) = self.tq.schema.error() {
                widgets::error(ui, &e.message(cx.lang));
            }
            return None;
        };
        let out = query_bar(ui, &mut self.tq.bar, &QueryBarCtx::new(&loaded.schema, cx.lang).with_filters(cx.conn, Some(proj.id), &proj.key));
        if out.help_clicked {
            self.help.open_at(None);
        }
        if out.keywords_clicked {
            self.kw.open();
        }
        if out.history_changed {
            cx.db.set_meta(HISTORY_META, &self.tq.bar.history.to_json());
        }
        out.submitted
    }

    /// Normalzustand: die Leiste unter der ersten Zeile. Gibt `true` zurück, wenn jetzt eine Abfrage aktiv ist (der Aufrufer zeichnet
    /// die lokale Liste dann nicht mehr).
    pub(super) fn tq_bar_row(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) -> bool {
        ui.add_space(S1);
        if let Some(q) = self.tq_bar(ui, cx, proj) {
            if self.tq.submit(&q) {
                ui.ctx().request_repaint();
            }
        }
        ui.add_space(S1);
        self.tq_active()
    }

    /// Zustand mit aktiver Abfrage: Kopfzeile mit Hinweis, Leiste, Ergebnisse.
    pub(super) fn tq_tab(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let mut clear = false;
        ui.horizontal(|ui| {
            if widgets::primary(ui, &format!("+ {}", lang.t("Neues Ticket", "New ticket"))).on_hover_text(lang.t("Neues Ticket anlegen (Strg+N)", "Create a new ticket (Ctrl+N)")).clicked() {
                self.open_create(cx, None);
            }
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Filter durch Abfrage ersetzt", "Filters replaced by the query")).small().color(style::dim(ui)))
                .on_hover_text(lang.t("Solange eine Abfrage aktiv ist, gelten Typ-/Status-Chips, Stern, Markierung und das Suchfeld nicht.", "While a query is active, the type/status chips, star, flag and search box do not apply."));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(lang.t("Abfrage löschen", "Clear query")).on_hover_text(lang.t("Zurück zur lokalen Liste mit den Schnellfiltern", "Back to the local list with the quick filters")).clicked() {
                    clear = true;
                }
            });
        });
        ui.add_space(S1);
        if let Some(q) = self.tq_bar(ui, cx, proj) {
            let was_active = self.tq.active.is_some();
            self.tq.submit(&q);
            if was_active && self.tq.active.is_none() {
                // leer abgeschickt: wie „Abfrage löschen“, die lokale Liste lädt neu
                self.tickets.list_dirty = true;
                return;
            }
        }
        if clear {
            self.tq.clear();
            self.tq.bar.text.clear();
            self.tickets.list_dirty = true;
            return;
        }
        let Some(query) = self.tq.active.clone() else { return };
        let Some(loaded) = self.tq.schema.loaded() else { return };
        if !loaded.conflicts.is_empty() {
            super::search_tab::conflict_note(ui, lang, &loaded.conflicts);
        }
        ui.add_space(S1);
        self.tq.res.drive(ui.ctx(), cx.db, Scope::Project(proj.id), lang, &loaded.schema, Some(&query));
        // Die Tabelle füllt den Rest der Höhe (abzüglich Statuszeile und Tabellenkopf).
        let height = (ui.available_height() - 70.0).max(120.0);
        let cfg = ResultsCfg {
            lang,
            scroll_height: Some(height),
            salt: "ticket_query_res",
            selected: self.tickets.open,
            schema: &loaded.schema,
            three_state: false,
            empty_text: lang.t("Kein Ticket passt zur Abfrage.", "No ticket matches the query."),
        };
        let o = self.tq.res.draw(ui, &cfg, false);
        if let Some(id) = o.open {
            self.tickets.open_ticket(id, false);
        }
        if let Some((id, is_star, on)) = o.mark {
            let r = if is_star { set_starred(cx.conn, id, on, &Source::ui()) } else { set_flagged(cx.conn, id, on, &Source::ui()) };
            self.t_apply(cx, r);
        }
    }
}
