//! Zentrale Navigation „Ticket öffnen“: **ein** Einsprung für alle Stellen, die ein Ticket zeigen wollen (Board-Karte, Backlog-Zeile,
//! Dashboard-Kachel, Übersicht, Verweise in der Ticket-Ansicht, Chips/Palette/Mini-Suche im Terminal Hub).
//!
//! * [`PlanUi::open_ticket`] öffnet ein Ticket anhand der Kennung (Projektwechsel, Reiter „Tickets“, Seite oder Panel je nach
//!   Modus, Hilfe schließen) und merkt den Reiter, von dem man kam. [`PlanUi::open_ticket_anywhere`] ist dasselbe mit bekanntem Projekt.
//! * **Zurück:** [`PlanUi::ticket_back`] (Pfeil, Esc) geht erst durch die Verweiskette (vorheriges Ticket) und führt danach zum
//!   Reiter zurück, von dem aus das Ticket geöffnet wurde (Board, Backlog, Dashboard, Übersicht …); wurde es in der Liste
//!   geöffnet, zur Liste. [`PlanUi::ticket_close`] (Kreuz im Panel) verlässt die Ansicht ohne Verweiskette.
//!   Wer den Reiter von Hand wechselt, vergisst den Rückweg (`TicketUi::return_tab`).
//! * **Terminal Hub:** `App::open_ticket_from_anywhere` (`terminal_links.rs`) schaltet auf den Todo-Reiter und meldet das Ticket
//!   mit [`PlanUi::request_open_ticket`] an; `PlanUi::header` öffnet es im selben Bild (der Expertenmodus wird dafür eingeschaltet).
//! * **Dashboard → Suche:** [`PlanUi::open_search_query`] zeigt den Reiter „Suche“ des Projekts mit der Abfrage im Feld. Sie wird
//!   *nicht* ausgeführt, solange `run_search` (search_tab.rs) nur ein Platzhalter ist; mit der angeschlossenen Suche genügt dort ein
//!   Aufruf von `self.run_search(jql)` an der markierten Stelle.
//! * Der **Entwurf** „Neues Ticket“ (`TicketUi::create`) und alle anderen Dialoge bleiben bei der Navigation unberührt.

use super::ticket_state::ViewMode;
use super::{Cx, PlanUi, Tab};
use crate::planning::tickets::get_ticket;
use crate::state::Lang;

/// Wohin „Zurück“ führt, wenn ein Ticket vom Reiter `current` aus geöffnet wird. Im Reiter „Tickets“ selbst bleibt der bisherige
/// Rückweg (Verweis von Ticket zu Ticket) erhalten; ist dort kein Ticket offen, kommt man aus der Liste (`None`).
pub(super) fn return_tab_for(current: Tab, ticket_open: bool, existing: Option<Tab>) -> Option<Tab> {
    if current == Tab::Tickets {
        if ticket_open { existing } else { None }
    } else {
        Some(current)
    }
}

/// Auf welches Ticket die Taste „.“ im Reiter „Tickets“ wirkt: im Seitenmodus das geöffnete Ticket, sonst das Ticket unter dem
/// Mauszeiger in der Liste, danach das im Panel geöffnete.
pub(super) fn quick_target(open: Option<i64>, mode: ViewMode, hover: Option<i64>) -> Option<i64> {
    match mode {
        ViewMode::Page => open.or(hover),
        ViewMode::Panel => hover.or(open),
    }
}

/// Name eines Reiters für den Hinweistext des Zurück-Pfeils.
pub(super) fn tab_label(tab: Tab, lang: Lang) -> &'static str {
    match tab {
        Tab::Tickets => lang.t("Tickets", "Tickets"),
        Tab::Overview => lang.t("Übersicht", "Overview"),
        Tab::Search => lang.t("Suche", "Search"),
        Tab::Boards => lang.t("Boards", "Boards"),
        Tab::Dashboards => lang.t("Dashboards", "Dashboards"),
        Tab::Backlog => lang.t("Backlog", "Backlog"),
        Tab::Releases => lang.t("Releases", "Releases"),
        Tab::Types => lang.t("Typen", "Types"),
        Tab::Fields => lang.t("Felder", "Fields"),
        Tab::Statuses => lang.t("Status", "Statuses"),
        Tab::Workflows => lang.t("Workflows", "Workflows"),
        Tab::Links => lang.t("Verknüpfungstypen", "Link types"),
        Tab::WorkKinds => lang.t("Arten der Arbeit", "Kinds of work"),
        Tab::Templates => lang.t("Vorlagen & Sicherung", "Templates & backup"),
        Tab::Quick => lang.t("Schnellaktionen", "Quick actions"),
    }
}

impl PlanUi {
    /// Ticket anhand der Kennung öffnen (egal in welchem Projekt es liegt). `false` und eine Fehlermeldung, wenn es das Ticket
    /// nicht gibt.
    pub(super) fn open_ticket(&mut self, cx: &Cx, ticket_id: i64) -> bool {
        match get_ticket(cx.conn, ticket_id) {
            Ok(t) => {
                self.open_ticket_anywhere(cx, t.id, t.project_id);
                true
            }
            Err(e) => {
                self.t_fail(cx, &e);
                false
            }
        }
    }

    /// Ticket `id` des Projekts `project` öffnen: Projekt wechseln, Reiter „Tickets“ wählen, Ticket in der Seite oder im Panel zeigen
    /// (je nach Modus), Hilfe schließen; merkt den Reiter, von dem man kam.
    pub(super) fn open_ticket_anywhere(&mut self, cx: &Cx, id: i64, project: i64) {
        if !self.expert {
            self.set_expert(cx.db, true);
        }
        self.help.open = false;
        self.msg = None;
        let viewing = self.tab == Tab::Tickets && self.tickets.open.is_some();
        let ret = return_tab_for(self.tab, self.tickets.open.is_some(), self.tickets.return_tab);
        if project != self.selected {
            // der Projektwechsel räumt die Navigation auf; das bisherige Ticket bleibt für „Zurück“ nicht erhalten
            self.select(cx, project);
        }
        self.tab = Tab::Tickets;
        self.tickets.open_ticket(id, viewing);
        self.tickets.return_tab = ret;
        self.tickets.invalidate();
    }

    /// Das Ticket-Anzeigen verlassen: zurück zum Reiter, von dem aus es geöffnet wurde (falls es einen gibt).
    fn leave_ticket_view(&mut self) {
        if let Some(t) = self.tickets.return_tab.take() {
            self.tab = t;
        }
    }

    /// Zurück (Pfeil, Esc): zum vorherigen Ticket der Verweiskette, danach zum Ursprungs-Reiter bzw. zur Liste.
    pub(super) fn ticket_back(&mut self) {
        if self.tickets.go_back() {
            self.leave_ticket_view();
        }
    }

    /// Schließen (Kreuz im Panel): die Ansicht verlassen, auch wenn Verweise davor lagen.
    pub(super) fn ticket_close(&mut self) {
        self.tickets.close();
        self.leave_ticket_view();
    }

    /// Hinweistext des Zurück-Pfeils („Zurück zu Boards“ usw.), wenn der Weg nicht zur Liste führt und keine Verweiskette davor liegt.
    pub(super) fn back_target_label(&self, lang: Lang) -> Option<&'static str> {
        if !self.tickets.back.is_empty() {
            return None;
        }
        self.tickets.return_tab.map(|t| tab_label(t, lang))
    }

    /// Ticket aus dem Terminal Hub anfordern (Chips, Palette, Mini-Suche); geöffnet wird es im nächsten Bild von `header`.
    pub(in crate::app) fn request_open_ticket(&mut self, ticket_id: i64) {
        self.tickets.requested_open = Some(ticket_id);
    }

    /// Angefordertes Ticket jetzt öffnen (aus `header`, sobald Projekte und Datenbank bereit sind).
    pub(super) fn run_requested_open(&mut self, cx: &Cx) {
        if let Some(id) = self.tickets.requested_open.take() {
            self.open_ticket(cx, id);
        }
    }

    /// Reiter „Suche“ zeigen, die Abfrage ins Feld setzen (ohne sie auszuführen) und dafür bei Bedarf das Projekt wechseln
    /// (`None` = im aktuellen Projekt bleiben, z. B. Dashboards über alle Projekte).
    pub(super) fn open_search_query(&mut self, cx: &Cx, jql: &str, project: Option<i64>) {
        if let Some(p) = project.filter(|p| *p != self.selected && self.projects.iter().any(|x| x.id == *p)) {
            self.select(cx, p);
        }
        self.help.open = false;
        self.msg = None;
        self.search.bar.set_query(jql);
        self.tab = Tab::Search;
        // TODO query-ui: sobald `run_search` die Suche ausführt, hier `self.run_search(jql)` aufrufen.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_tab_is_the_origin_unless_the_ticket_view_itself_navigates() {
        assert_eq!(return_tab_for(Tab::Boards, false, None), Some(Tab::Boards));
        assert_eq!(return_tab_for(Tab::Dashboards, true, Some(Tab::Boards)), Some(Tab::Dashboards), "ein anderer Reiter gewinnt");
        assert_eq!(return_tab_for(Tab::Tickets, true, Some(Tab::Backlog)), Some(Tab::Backlog), "Verweis von Ticket zu Ticket behält den Rückweg");
        assert_eq!(return_tab_for(Tab::Tickets, false, Some(Tab::Backlog)), None, "aus der Liste: zurück zur Liste");
        assert_eq!(return_tab_for(Tab::Tickets, true, None), None);
    }

    #[test]
    fn quick_target_prefers_the_page_ticket_and_the_hovered_row() {
        assert_eq!(quick_target(Some(1), ViewMode::Page, Some(2)), Some(1));
        assert_eq!(quick_target(None, ViewMode::Page, Some(2)), Some(2));
        assert_eq!(quick_target(Some(1), ViewMode::Panel, Some(2)), Some(2));
        assert_eq!(quick_target(Some(1), ViewMode::Panel, None), Some(1));
        assert_eq!(quick_target(None, ViewMode::Panel, None), None);
    }

    #[test]
    fn every_tab_has_a_label_in_both_languages() {
        for t in [Tab::Tickets, Tab::Overview, Tab::Search, Tab::Boards, Tab::Dashboards, Tab::Backlog, Tab::Releases, Tab::Types, Tab::Fields, Tab::Statuses, Tab::Workflows, Tab::Links, Tab::WorkKinds, Tab::Templates, Tab::Quick] {
            assert!(!tab_label(t, Lang::De).is_empty() && !tab_label(t, Lang::En).is_empty());
        }
    }
}
