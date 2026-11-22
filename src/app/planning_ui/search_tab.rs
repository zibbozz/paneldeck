//! Reiter „Suche“ im Projekt: die Abfrage-Leiste (`query_bar`) samt Hilfeseite, gespeicherten Filtern und Schlüsselwort-Editor, darunter
//! die Ergebnisse der abgeschickten Abfrage (Tabelle, sortierbare Spalten, Seiten, Fehler mit Stelle).
//!
//! **Einhängepunkte (klein gehalten):**
//! * `Tab::Search` in `mod.rs` (Reiterliste `PlanUi::tabs`, Verteiler `PlanUi::body`) – der Reiter selbst ist [`PlanUi::search_tab`].
//! * [`PlanUi::run_search`] bekommt jede abgeschickte Abfrage (Leiste, gespeicherter Filter, Hilfeseite) und merkt sie; der Reiter führt sie
//!   im nächsten Bild über [`Results`] aus (Hintergrund-Faden mit eigener Verbindung, siehe `search_run`).
//! * [`PlanUi::open_search_query`]: Abfrage von außen in die Suche übernehmen **und ausführen** (Hilfeseite „In Suche übernehmen“, später
//!   „Filter öffnen“ der Dashboards über `open_search_in_project`).
//! * Ein Klick auf eine Ergebniszeile öffnet das Ticket über die zentrale Navigation (`PlanUi::open_ticket`, `ticket_nav.rs`).
//!
//! **Schema:** Die Leiste benutzt `planning::search::search_schema` (Felder des Bereichs, Sprints, Versionen, Schlüsselwörter). Der Bereich
//! ist das gewählte Projekt oder „alle Projekte“; beim Umschalten ändert sich das Schema und die Abfrage läuft neu. Gleichnamige Felder
//! verschiedener Art, die nicht abfragbar sind, nennt ein Hinweis unter der Leiste.

use super::query_bar::{QueryBarCtx, QueryBarState, query_bar};
use super::search_run::SchemaCache;
use super::search_table::{Results, ResultsCfg};
use super::widgets::{self, hint};
use super::{Cx, PlanUi, Tab};
use crate::planning::history::Source;
use crate::planning::search::{Conflict, Scope};
use crate::planning::tickets::{set_flagged, set_starred};
use crate::state::Lang;
use crate::style::{self, S2, S3};
use eframe::egui::{RichText, Ui};

/// Schlüssel in `meta`, unter dem der Verlauf der Abfragen liegt.
const HISTORY_META: &str = "query_history";

/// Zustand des Reiters „Suche“.
pub(super) struct State {
    pub bar: QueryBarState,
    /// Schema des Bereichs (neu gebaut bei anderem Bereich/anderer Sprache oder wenn `stale`)
    pub(super) schema: SchemaCache,
    pub stale: bool,
    history_loaded: bool,
    /// Die zuletzt abgeschickte Abfrage (so, wie sie ausgeführt wird; die Sortierung nach Spalten kommt dazu)
    pub last: Option<String>,
    /// Bereich: alle Projekte statt des gewählten
    pub scope_all: bool,
    pub res: Results,
}

impl Default for State {
    fn default() -> Self {
        State { bar: QueryBarState::new("search_tab"), schema: SchemaCache::default(), stale: true, history_loaded: false, last: None, scope_all: false, res: Results::new(None) }
    }
}

impl State {
    /// Bereich der Abfrage.
    pub(super) fn scope(&self, project: i64) -> Scope {
        if self.scope_all { Scope::All } else { Scope::Project(project) }
    }
}

// ------------------------------------------------------------------ Reiter

impl PlanUi {
    /// Der Reiter „Suche“: Abfrage-Leiste, Bereich und Ergebnisse.
    pub(super) fn search_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(proj) = self.project().cloned() else { return };
        if !self.search.history_loaded {
            self.search.history_loaded = true;
            if let Some(h) = cx.db.meta(HISTORY_META) {
                self.search.bar.history = super::query_logic::History::from_json(&h);
            }
        }
        if proj.system {
            // das System-Projekt (Todo-Liste) hat keine Tickets: gesucht wird in allen
            self.search.scope_all = true;
        }
        hint(ui, lang.t(
            "Schreibe eine Abfrage, z. B. type = Epic AND status != Done ORDER BY priority. Vorschläge erscheinen beim Tippen; das Fragezeichen öffnet die Anleitung.",
            "Write a query, e.g. type = Epic AND status != Done ORDER BY priority. Suggestions appear while you type; the question mark opens the guide.",
        ));
        ui.add_space(S2);
        // ---- Bereich: dieses Projekt oder alle
        let mut all = self.search.scope_all;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(RichText::new(lang.t("Suchen in", "Search in")).small().color(style::dim(ui)));
            ui.add_enabled_ui(!proj.system, |ui| {
                widgets::radio(ui, &mut all, false, format!("{} ({})", lang.t("Dieses Projekt", "This project"), proj.key)).on_hover_text(lang.t("Nur Tickets dieses Projekts; Felder, Typen und Status des Projekts", "Only tickets of this project; the project's fields, types and statuses"));
            });
            widgets::radio(ui, &mut all, true, lang.t("Alle Projekte", "All projects")).on_hover_text(lang.t("Tickets aller Projekte; gleichnamige Felder werden zusammengelegt", "Tickets of all projects; fields with the same name are merged"));
        });
        if all != self.search.scope_all {
            self.search.scope_all = all;
            self.search.res.page = 0;
        }
        let scope = self.search.scope(proj.id);
        if self.search.stale {
            self.search.stale = false;
            self.search.schema.invalidate();
            if self.search.last.is_some() {
                self.search.res.run.mark_stale();
            }
        }
        self.search.schema.ensure(cx.conn, scope, lang);
        ui.add_space(S2);
        let Some(loaded) = self.search.schema.loaded() else {
            if let Some(e) = self.search.schema.error() {
                widgets::error(ui, &e.message(lang));
            }
            return;
        };
        // ---- Leiste
        let key = proj.key.clone();
        let project_for_filters = (!self.search.scope_all).then_some(proj.id);
        let bar_ctx = QueryBarCtx::new(&loaded.schema, lang).with_filters(cx.conn, project_for_filters, if self.search.scope_all { "" } else { &key });
        let top = ui.cursor().min.y;
        let out = query_bar(ui, &mut self.search.bar, &bar_ctx);
        // Tutorial (#143): die Abfrage-Leiste als Ziel.
        crate::app::tutorial::spot_since(ui, crate::app::tutorial::Spot::QueryBar, top);
        if out.help_clicked {
            self.help.open_at(None);
        }
        if out.keywords_clicked {
            self.kw.open();
        }
        if out.history_changed {
            cx.db.set_meta(HISTORY_META, &self.search.bar.history.to_json());
        }
        if let Some(q) = out.submitted {
            self.run_search(&q);
        }
        if !loaded.conflicts.is_empty() {
            conflict_note(ui, lang, &loaded.conflicts);
        }
        ui.add_space(S3);
        // ---- Ergebnisse
        let query = self.search.last.clone();
        self.search.res.drive(ui.ctx(), cx.db, scope, lang, &loaded.schema, query.as_deref());
        if query.is_none() {
            widgets::card(ui, |ui| {
                ui.label(RichText::new(lang.t("Die Ergebnisse erscheinen hier, sobald du eine Abfrage abschickst (Enter oder „Suchen“).", "The results appear here once you run a query (Enter or “Search”).")).color(style::dim(ui)));
            });
            return;
        }
        let cfg = ResultsCfg {
            lang,
            scroll_height: None,
            salt: "search_res",
            selected: None,
            schema: &loaded.schema,
            three_state: true,
            empty_text: lang.t("Kein Ticket passt zur Abfrage.", "No ticket matches the query."),
        };
        // Kommt man vom geöffneten Ticket zurück (Zurück, Schließen), steht dessen Zeile in der Liste im Bild – wie in der Ticket-Liste.
        if let Some(id) = self.tickets.scroll_to.take() {
            self.search.res.scroll_to = Some(id);
        }
        let o = self.search.res.draw(ui, &cfg, self.search.scope_all);
        if let Some(id) = o.open {
            self.open_ticket(cx, id);
        }
        if let Some((id, is_star, on)) = o.mark {
            let r = if is_star { set_starred(cx.conn, id, on, &Source::ui()) } else { set_flagged(cx.conn, id, on, &Source::ui()) };
            self.t_apply(cx, r);
        }
    }

    /// **Anschlusspunkt der Suche:** jede abgeschickte Abfrage (gültig oder leer) landet hier. Sie wird gemerkt, die Ergebnisse beginnen
    /// wieder bei Seite 1 und der Reiter führt sie im nächsten Bild aus (auch dieselbe Abfrage noch einmal: „Aktualisieren“).
    pub(super) fn run_search(&mut self, query: &str) {
        self.search.last = Some(query.trim().to_string());
        self.search.res.restart();
    }

    /// Abfrage von außen in die Suche übernehmen und **ausführen**: Text in die Leiste, Reiter „Suche“, Lauf. `all_projects`:
    /// `Some(b)` setzt den Bereich (alle Projekte oder das gewählte), `None` lässt ihn.
    pub(super) fn open_search_query(&mut self, query: &str, all_projects: Option<bool>) {
        if let Some(a) = all_projects {
            self.search.scope_all = a;
        }
        self.search.bar.set_query(query);
        self.show_search_tab();
        self.run_search(query);
    }

    /// Den Schlüsselwort-Dialog zeichnen (aus `dialogs`); Änderungen machen das Schema der Leiste ungültig.
    pub(super) fn keywords_dialog_ui(&mut self, ctx: &eframe::egui::Context, cx: &Cx) {
        if super::keywords_ed::keywords_dialog(ctx, &mut self.kw, cx.conn, cx.lang) {
            self.search.stale = true;
            self.search.bar.filters.invalidate();
            self.dirty = true;
        }
    }

    /// Reiter „Suche“ wählen (z. B. nach „In Suche übernehmen“ in der Hilfe).
    pub(super) fn show_search_tab(&mut self) {
        self.tab = Tab::Search;
    }
}

/// Hinweis auf Feldnamen, die im Bereich nicht abfragbar sind (gleicher Name, verschiedene Art – oder von einem eingebauten Feld verdeckt).
pub(super) fn conflict_note(ui: &mut Ui, lang: Lang, conflicts: &[Conflict]) {
    let names: Vec<String> = conflicts.iter().map(|c| lang.q(&c.name)).collect();
    let text = match lang {
        Lang::De => format!("▲ Nicht abfragbar (gleicher Name, verschiedene Art): {}. Benenne eines der Felder um.", names.join(", ")),
        Lang::En => format!("▲ Not queryable (same name, different kind): {}. Rename one of the fields.", names.join(", ")),
    };
    ui.add_space(2.0);
    let r = ui.label(RichText::new(text).small().color(style::dim(ui)));
    r.on_hover_text(conflict_details(lang, conflicts));
}

/// Genaue Angaben je Feld (für den Tooltip des Hinweises).
pub(super) fn conflict_details(lang: Lang, conflicts: &[Conflict]) -> String {
    conflicts
        .iter()
        .map(|c| {
            let dropped: Vec<&str> = c.dropped.iter().map(|k| k.as_str()).collect();
            match (&c.kept, lang) {
                (None, Lang::De) => format!("{}: wird vom eingebauten Feld gleichen Namens verdeckt (Arten: {}).", lang.q(&c.name), dropped.join(", ")),
                (None, Lang::En) => format!("{}: hidden by the built-in field of the same name (kinds: {}).", lang.q(&c.name), dropped.join(", ")),
                (Some(k), Lang::De) => format!("{}: abfragbar ist die Art „{}“; die Arten {} sind es nicht.", lang.q(&c.name), k.as_str(), dropped.join(", ")),
                (Some(k), Lang::En) => format!("{}: the “{}” kind is queryable; the kinds {} are not.", lang.q(&c.name), k.as_str(), dropped.join(", ")),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
