//! Zustand der Ticket-Oberfläche (Liste, geöffnetes Ticket, Dialoge, Rückgängig-Historie) und die reinen Hilfen dazu:
//! Sortierung der Liste, Filter → Datenschicht, Beschriftung von Rückgängig-Schritten, Sicherheitstimer.

use super::ticket_data::{Detail, Row};
use super::ticket_dialogs::{ChangeDlg, CloneDlg, ConfirmDlg, ConvertDlg, DeleteDlg, TimeDlg, TransitionDlg};
use super::ticket_draft::Draft;
use crate::markdown::FieldMode;
use crate::planning::ops::{MarkKind, TicketOp};
use crate::planning::ticket_list::{TicketFilter, TicketOrder};
use crate::state::Lang;
use crate::undo::History;
use std::collections::HashMap;
use std::time::Instant;

/// So lange (in Sekunden) bleibt „Löschen“ gesperrt.
pub const CONFIRM_SECS: f32 = 3.0;

/// Wie ein geöffnetes Ticket gezeigt wird.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    /// Eigene Seite mit Zurück-Pfeil (Standard).
    Page,
    /// Seitenpanel rechts neben der Liste.
    Panel,
}

impl ViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ViewMode::Page => "page",
            ViewMode::Panel => "panel",
        }
    }

    pub fn parse(s: &str) -> ViewMode {
        if s == "panel" { ViewMode::Panel } else { ViewMode::Page }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortCol {
    Type,
    Key,
    Title,
    Status,
    Priority,
    Points,
    Marks,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sort {
    pub col: SortCol,
    pub desc: bool,
}

impl Default for Sort {
    /// Neueste zuerst.
    fn default() -> Self {
        Sort { col: SortCol::Key, desc: true }
    }
}

impl Sort {
    /// Klick auf eine Spaltenüberschrift: dieselbe Spalte kehrt die Richtung um, eine neue beginnt aufsteigend
    /// (Markierungen und Punkte: absteigend, das Wichtige zuerst).
    pub fn toggled(self, col: SortCol) -> Sort {
        if self.col == col {
            Sort { col, desc: !self.desc }
        } else {
            Sort { col, desc: matches!(col, SortCol::Marks | SortCol::Points) }
        }
    }
}

/// Filter der Liste (Typ- und Status-Chips, Stern, Markierung, Text).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub types: Vec<i64>,
    pub statuses: Vec<i64>,
    pub starred: bool,
    pub flagged: bool,
    pub text: String,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.types.is_empty() && self.statuses.is_empty() && !self.starred && !self.flagged && self.text.trim().is_empty()
    }

    pub fn to_ticket_filter(&self) -> TicketFilter {
        TicketFilter {
            types: self.types.clone(),
            statuses: self.statuses.clone(),
            starred: self.starred.then_some(true),
            flagged: self.flagged.then_some(true),
            text: Some(self.text.trim().to_string()).filter(|t| !t.is_empty()),
            order: TicketOrder::Rank,
            ..Default::default()
        }
    }

    /// Chip an- oder ausschalten.
    pub fn toggle_type(&mut self, id: i64) {
        toggle(&mut self.types, id);
    }

    pub fn toggle_status(&mut self, id: i64) {
        toggle(&mut self.statuses, id);
    }

    /// Auswahlen entfernen, die es nicht mehr gibt (Typ/Status gelöscht).
    pub fn retain_known(&mut self, types: &[i64], statuses: &[i64]) {
        self.types.retain(|t| types.contains(t));
        self.statuses.retain(|s| statuses.contains(s));
    }
}

fn toggle(v: &mut Vec<i64>, id: i64) {
    if let Some(p) = v.iter().position(|x| *x == id) {
        v.remove(p);
    } else {
        v.push(id);
    }
}

/// Hilfen für die Sortierung (Positionen der Typen/Status, Rang der Priorität).
pub struct SortCtx<'a> {
    pub type_pos: &'a dyn Fn(i64) -> i64,
    pub status_pos: &'a dyn Fn(i64) -> i64,
    /// Rang der Priorität (0 = höchste); `None` = ohne.
    pub prio_rank: &'a dyn Fn(&Row) -> Option<usize>,
}

/// Sortiert die Zeilen; Zeilen ohne Wert (Status, Priorität, Punkte) stehen immer hinten.
pub fn sort_rows(rows: &mut [Row], sort: Sort, cx: &SortCtx) {
    use std::cmp::Ordering;
    fn opt<T: PartialOrd>(a: Option<T>, b: Option<T>, desc: bool) -> Ordering {
        match (a, b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                let o = a.partial_cmp(&b).unwrap_or(Ordering::Equal);
                if desc { o.reverse() } else { o }
            }
        }
    }
    rows.sort_by(|a, b| {
        let (ta, tb) = (&a.ticket, &b.ticket);
        let main = match sort.col {
            SortCol::Key => {
                let o = ta.number.cmp(&tb.number);
                if sort.desc { o.reverse() } else { o }
            }
            SortCol::Title => {
                let o = ta.title.to_lowercase().cmp(&tb.title.to_lowercase());
                if sort.desc { o.reverse() } else { o }
            }
            SortCol::Type => {
                let o = (cx.type_pos)(ta.type_id).cmp(&(cx.type_pos)(tb.type_id));
                if sort.desc { o.reverse() } else { o }
            }
            SortCol::Status => opt(ta.status_id.map(|s| (cx.status_pos)(s)), tb.status_id.map(|s| (cx.status_pos)(s)), sort.desc),
            SortCol::Priority => opt((cx.prio_rank)(a), (cx.prio_rank)(b), sort.desc),
            SortCol::Points => opt(a.points, b.points, sort.desc),
            SortCol::Marks => {
                let ka = (ta.starred as u8) * 2 + ta.flagged as u8;
                let kb = (tb.starred as u8) * 2 + tb.flagged as u8;
                let o = ka.cmp(&kb);
                if sort.desc { o.reverse() } else { o }
            }
        };
        main.then(tb.number.cmp(&ta.number))
    });
}

/// Kurze Beschreibung einer rückgängig gemachten oder wiederholten Änderung („Undone: …“).
pub fn op_label(op: &TicketOp, lang: Lang) -> String {
    let s = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
    match op {
        TicketOp::Created { present, snapshot, .. } => {
            let key = snapshot.ticket.key();
            // `present` beschreibt den Zustand NACH der Operation
            if *present {
                format!("{} {key}", lang.t("Ticket angelegt:", "Ticket created:"))
            } else {
                format!("{} {key}", lang.t("Ticket entfernt:", "Ticket removed:"))
            }
        }
        TicketOp::Title { .. } => s("Titel geändert", "title changed"),
        TicketOp::Fields { changes, .. } => match lang {
            Lang::De => format!("{} Feld(er) geändert", changes.len()),
            Lang::En => format!("{} field(s) changed", changes.len()),
        },
        TicketOp::Status { .. } => s("Status geändert", "status changed"),
        TicketOp::Parent { .. } => s("Eltern-Ticket geändert", "parent changed"),
        TicketOp::Mark { kind: MarkKind::Flag, .. } => s("Markierung geändert", "flag changed"),
        TicketOp::Mark { kind: MarkKind::Star, .. } => s("Stern geändert", "star changed"),
        TicketOp::Rank { .. } => s("Reihenfolge geändert", "order changed"),
        TicketOp::Link { .. } => s("Verknüpfung geändert", "link changed"),
        TicketOp::Comment { .. } | TicketOp::CommentEdit { .. } => s("Kommentar geändert", "comment changed"),
        TicketOp::Time { .. } | TicketOp::TimeEdit { .. } => s("Zeiteintrag geändert", "time entry changed"),
        TicketOp::Sprint(_) => s("Sprint-Zuordnung geändert", "sprint assignment changed"),
        TicketOp::TodoConversion { .. } => s("Todo-Karte umgewandelt", "todo card converted"),
        TicketOp::Batch(ops) => ops.first().map(|o| op_label(o, lang)).unwrap_or_default(),
    }
}

/// Verbleibende Sekunden (aufgerundet) eines Sicherheitstimers.
pub fn countdown_secs(since: Instant, now: Instant) -> u32 {
    let left = CONFIRM_SECS - now.saturating_duration_since(since).as_secs_f32();
    left.ceil().max(0.0) as u32
}

/// Oberflächenzustand des gerade geöffneten Tickets (Eingabepuffer usw.); wird beim Wechsel des Tickets zurückgesetzt.
#[derive(Default)]
pub struct ViewState {
    pub ticket: i64,
    pub drafts: HashMap<i64, Draft>,
    pub comment: String,
    pub comment_mode: FieldMode,
    /// Kommentar in Bearbeitung: (Kennung, Text, Darstellung).
    pub comment_edit: Option<(i64, String, FieldMode)>,
    pub link: LinkDraft,
    pub parent_text: String,
    pub parent_open: bool,
}

/// Eingabe „Verknüpfung hinzufügen“.
#[derive(Default)]
pub struct LinkDraft {
    pub open: bool,
    /// Index in der Liste der Angebote (Verknüpfungstyp + Richtung).
    pub choice: usize,
    pub text: String,
    pub target_ticket: Option<i64>,
    pub repo: String,
    pub name: String,
}

pub struct TicketUi {
    /// Projekt, zu dem die geladene Liste gehört.
    pub project: i64,
    pub list_dirty: bool,
    pub detail_dirty: bool,
    pub filters: Filters,
    pub sort: Sort,
    pub rows: Vec<Row>,
    /// Alle Tickets des Projekts (ohne Filter).
    pub project_total: usize,
    pub open: Option<i64>,
    /// Vorherige Tickets beim Navigieren über Verweise (Zurück-Pfeil).
    pub back: Vec<i64>,
    pub mode: ViewMode,
    pub mode_loaded: bool,
    pub detail: Option<Detail>,
    pub view: ViewState,
    pub history: History<TicketOp>,
    pub labels: Vec<String>,
    pub create: Option<super::ticket_create::CreateDlg>,
    pub create_another: bool,
    pub transition: Option<TransitionDlg>,
    pub delete: Option<DeleteDlg>,
    pub clone: Option<CloneDlg>,
    pub change: Option<ChangeDlg>,
    pub time: Option<TimeDlg>,
    pub convert: Option<ConvertDlg>,
    pub confirm: Option<ConfirmDlg>,
    /// Hinweise für den Toast-Stapel der Anwendung.
    pub toasts: Vec<String>,
    /// Fokus auf das Suchfeld der Liste anfordern.
    pub focus_search: bool,
    /// (alle, erledigte) Tickets des Projekts für die Übersicht.
    pub counts: (usize, usize),
    pub counts_dirty: bool,
    /// Zuletzt beim Anlegen gewählter Typ.
    pub create_last_type: Option<i64>,
    /// Größe der Rückgängig-Historie (Einstellung).
    pub undo_cap: usize,
    /// Reiter, von dem aus das Ticket geöffnet wurde (Board, Backlog, Dashboard …): „Zurück“ und „Schließen“ führen dorthin
    /// zurück, sobald das Ticket verlassen wird (siehe `ticket_nav.rs`). `None` = zur Liste.
    pub return_tab: Option<super::Tab>,
    /// Ticket unter dem Mauszeiger in der Liste (für die Taste „.“ und das Kontextmenü).
    pub hover: Option<i64>,
    /// Von außerhalb (Terminal Hub) gewünschtes Ticket; wird im nächsten Bild von `PlanUi::header` geöffnet.
    pub requested_open: Option<i64>,
}

impl Default for TicketUi {
    fn default() -> Self {
        TicketUi {
            project: 0,
            list_dirty: true,
            detail_dirty: true,
            filters: Filters::default(),
            sort: Sort::default(),
            rows: Vec::new(),
            project_total: 0,
            open: None,
            back: Vec::new(),
            mode: ViewMode::Page,
            mode_loaded: false,
            detail: None,
            view: ViewState::default(),
            history: History::new(25),
            labels: Vec::new(),
            create: None,
            create_another: false,
            transition: None,
            delete: None,
            clone: None,
            change: None,
            time: None,
            convert: None,
            confirm: None,
            toasts: Vec::new(),
            focus_search: false,
            counts: (0, 0),
            counts_dirty: true,
            create_last_type: None,
            undo_cap: 25,
            return_tab: None,
            hover: None,
            requested_open: None,
        }
    }
}

impl TicketUi {
    /// Beim Wechsel des Projekts: Liste, Filter und geöffnetes Ticket zurücksetzen (Historie bleibt, sie gilt für alle Projekte).
    pub fn switch_project(&mut self, project: i64) {
        if self.project != project {
            self.project = project;
            self.filters = Filters::default();
            self.rows.clear();
            self.open = None;
            self.back.clear();
            self.return_tab = None;
            self.hover = None;
            self.detail = None;
            self.view = ViewState::default();
            self.list_dirty = true;
            self.detail_dirty = true;
            self.counts_dirty = true;
            self.create_last_type = None;
        }
    }

    /// Daten (Liste und Detail) beim nächsten Bild neu laden.
    pub fn invalidate(&mut self) {
        self.list_dirty = true;
        self.detail_dirty = true;
    }

    /// Ticket öffnen; `from_link` merkt das bisherige Ticket für den Zurück-Pfeil.
    pub fn open_ticket(&mut self, id: i64, from_link: bool) {
        if from_link {
            if let Some(cur) = self.open.filter(|c| *c != id) {
                self.back.push(cur);
            }
        } else {
            self.back.clear();
        }
        if self.open != Some(id) {
            self.view = ViewState { ticket: id, ..Default::default() };
            self.detail = None;
        }
        self.open = Some(id);
        self.detail_dirty = true;
    }

    /// Zurück: zum vorherigen Ticket oder – ohne Vorgänger – zur Liste. `true`, wenn die Liste erreicht wurde.
    pub fn go_back(&mut self) -> bool {
        match self.back.pop() {
            Some(prev) => {
                self.open = Some(prev);
                self.view = ViewState { ticket: prev, ..Default::default() };
                self.detail = None;
                self.detail_dirty = true;
                false
            }
            None => {
                self.close();
                true
            }
        }
    }

    pub fn close(&mut self) {
        self.open = None;
        self.detail = None;
        self.back.clear();
        self.view = ViewState::default();
    }

    /// Eine abgeschlossene Änderung merken (rückgängig-fähig).
    pub fn commit(&mut self, op: Option<TicketOp>) {
        if let Some(op) = op {
            self.history.push(op);
        }
        self.invalidate();
    }

    /// Änderungen entfernen, die sich nicht mehr zurücknehmen lassen, weil sie diese Tickets betreffen
    /// (nach endgültigem Löschen oder nicht umkehrbaren Aktionen).
    pub fn forget_tickets(&mut self, ids: &[i64]) {
        self.history.retain(|op| !op.touches(ids));
    }

    /// Alles vergessen (z. B. wenn ein ganzes Projekt oder ein Typ gelöscht wurde).
    pub fn reset_history(&mut self) {
        self.history = History::new(self.undo_cap);
    }

    pub fn any_modal(&self) -> bool {
        self.create.is_some() || self.transition.is_some() || self.delete.is_some() || self.clone.is_some() || self.change.is_some() || self.time.is_some() || self.convert.is_some() || self.confirm.is_some()
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(text.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::ops::TicketSnapshot;
    use crate::planning::history::Source;
    use crate::planning::tickets::Ticket;
    use std::time::Duration;

    fn ticket(id: i64, number: i64, title: &str) -> Ticket {
        Ticket {
            id,
            project_id: 1,
            project_key: "PAN".into(),
            number,
            type_id: 1,
            title: title.into(),
            status_id: None,
            parent_id: None,
            rank: number,
            flagged: false,
            starred: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn row(id: i64, title: &str) -> Row {
        Row { ticket: ticket(id, id, title), prio: None, points: None }
    }

    fn numbers(rows: &[Row]) -> Vec<i64> {
        rows.iter().map(|r| r.ticket.number).collect()
    }

    fn cx<'a>(type_pos: &'a dyn Fn(i64) -> i64, status_pos: &'a dyn Fn(i64) -> i64, prio: &'a dyn Fn(&Row) -> Option<usize>) -> SortCtx<'a> {
        SortCtx { type_pos, status_pos, prio_rank: prio }
    }

    #[test]
    fn sorting_by_key_and_title_in_both_directions() {
        let mut rows = vec![row(2, "banane"), row(1, "Zitrone"), row(3, "Apfel")];
        let (tp, sp, pr) = (|_t: i64| 0, |_s: i64| 0, |_r: &Row| None);
        let c = cx(&tp, &sp, &pr);
        sort_rows(&mut rows, Sort { col: SortCol::Key, desc: false }, &c);
        assert_eq!(numbers(&rows), vec![1, 2, 3]);
        sort_rows(&mut rows, Sort { col: SortCol::Key, desc: true }, &c);
        assert_eq!(numbers(&rows), vec![3, 2, 1]);
        sort_rows(&mut rows, Sort { col: SortCol::Title, desc: false }, &c);
        assert_eq!(numbers(&rows), vec![3, 2, 1], "Apfel, banane, Zitrone – ohne Beachtung der Groß-/Kleinschreibung");
        sort_rows(&mut rows, Sort { col: SortCol::Title, desc: true }, &c);
        assert_eq!(numbers(&rows), vec![1, 2, 3]);
    }

    #[test]
    fn rows_without_a_value_stay_last_in_both_directions() {
        let mut rows = vec![row(1, "a"), row(2, "b"), row(3, "c")];
        rows[0].points = Some(3.0);
        rows[2].points = Some(8.0);
        let (tp, sp, pr) = (|_t: i64| 0, |_s: i64| 0, |_r: &Row| None);
        let c = cx(&tp, &sp, &pr);
        sort_rows(&mut rows, Sort { col: SortCol::Points, desc: false }, &c);
        assert_eq!(numbers(&rows), vec![1, 3, 2]);
        sort_rows(&mut rows, Sort { col: SortCol::Points, desc: true }, &c);
        assert_eq!(numbers(&rows), vec![3, 1, 2], "ohne Punkte bleibt hinten");
    }

    #[test]
    fn priority_status_and_marks_sort() {
        let mut rows = vec![row(1, "a"), row(2, "b"), row(3, "c"), row(4, "d")];
        rows[0].prio = Some(2);
        rows[1].prio = Some(0);
        rows[3].ticket.status_id = Some(10);
        rows[1].ticket.status_id = Some(20);
        rows[2].ticket.starred = true;
        rows[0].ticket.flagged = true;
        let (tp, sp) = (|_t: i64| 0, |s: i64| if s == 10 { 1 } else { 2 });
        let pr = |r: &Row| r.prio.map(|p| p as usize);
        let c = cx(&tp, &sp, &pr);
        sort_rows(&mut rows, Sort { col: SortCol::Priority, desc: false }, &c);
        assert_eq!(numbers(&rows)[..2], [2, 1], "höchste Priorität (Rang 0) zuerst");
        sort_rows(&mut rows, Sort { col: SortCol::Status, desc: false }, &c);
        assert_eq!(numbers(&rows)[..2], [4, 2]);
        sort_rows(&mut rows, Sort { col: SortCol::Marks, desc: true }, &c);
        assert_eq!(numbers(&rows)[..2], [3, 1], "Stern vor Markierung");
    }

    #[test]
    fn clicking_a_header_toggles_or_switches() {
        let s = Sort::default();
        assert_eq!(s, Sort { col: SortCol::Key, desc: true });
        assert_eq!(s.toggled(SortCol::Key), Sort { col: SortCol::Key, desc: false });
        assert_eq!(s.toggled(SortCol::Title), Sort { col: SortCol::Title, desc: false });
        assert!(s.toggled(SortCol::Marks).desc && s.toggled(SortCol::Points).desc);
    }

    #[test]
    fn filters_map_to_the_data_layer() {
        let mut f = Filters::default();
        assert!(f.is_empty());
        assert!(f.to_ticket_filter().text.is_none() && f.to_ticket_filter().starred.is_none());
        f.toggle_type(3);
        f.toggle_status(5);
        f.starred = true;
        f.text = "  abc def ".into();
        let t = f.to_ticket_filter();
        assert_eq!((t.types, t.statuses, t.starred, t.flagged, t.text.as_deref()), (vec![3], vec![5], Some(true), None, Some("abc def")));
        assert!(!f.is_empty());
        f.toggle_type(3);
        assert!(f.types.is_empty());
        f.toggle_status(9);
        f.retain_known(&[1], &[5]);
        assert_eq!(f.statuses, vec![5], "unbekannter Status entfällt");
    }

    #[test]
    fn view_mode_roundtrips_and_defaults_to_page() {
        assert_eq!(ViewMode::parse("panel"), ViewMode::Panel);
        assert_eq!(ViewMode::parse("page"), ViewMode::Page);
        assert_eq!(ViewMode::parse("unsinn"), ViewMode::Page);
        assert_eq!(ViewMode::parse(ViewMode::Panel.as_str()), ViewMode::Panel);
    }

    #[test]
    fn countdown_counts_down_to_zero() {
        let t0 = Instant::now();
        assert_eq!(countdown_secs(t0, t0), 3);
        assert_eq!(countdown_secs(t0, t0 + Duration::from_millis(1100)), 2);
        assert_eq!(countdown_secs(t0, t0 + Duration::from_millis(2500)), 1);
        assert_eq!(countdown_secs(t0, t0 + Duration::from_secs(3)), 0);
        assert_eq!(countdown_secs(t0, t0 + Duration::from_secs(60)), 0);
    }

    #[test]
    fn navigation_keeps_a_back_stack() {
        let mut u = TicketUi::default();
        u.open_ticket(1, false);
        u.open_ticket(2, true);
        u.open_ticket(3, true);
        assert_eq!((u.open, u.back.clone()), (Some(3), vec![1, 2]));
        assert!(!u.go_back());
        assert_eq!(u.open, Some(2));
        assert!(!u.go_back());
        assert_eq!(u.open, Some(1));
        assert!(u.go_back(), "ohne Vorgänger zurück zur Liste");
        assert_eq!(u.open, None);
        // aus der Liste geöffnet: keine alten Vorgänger
        u.back = vec![9];
        u.open_ticket(5, false);
        assert!(u.back.is_empty());
        // dasselbe Ticket noch einmal über einen Verweis: kein Eintrag auf sich selbst
        u.open_ticket(5, true);
        assert!(u.back.is_empty());
    }

    #[test]
    fn history_forgets_deleted_tickets() {
        let mut u = TicketUi::default();
        let src = Source::ui();
        u.commit(Some(TicketOp::Mark { ticket: 1, kind: MarkKind::Star, before: false, after: true, source: src.clone() }));
        u.commit(Some(TicketOp::Mark { ticket: 2, kind: MarkKind::Flag, before: false, after: true, source: src.clone() }));
        u.commit(None);
        u.forget_tickets(&[1]);
        assert!(u.history.can_undo());
        u.forget_tickets(&[2]);
        assert!(!u.history.can_undo(), "nichts mehr übrig");
        u.commit(Some(TicketOp::Title { ticket: 7, before: "a".into(), after: "b".into(), source: src }));
        u.reset_history();
        assert!(!u.history.can_undo());
    }

    #[test]
    fn undo_labels_exist_in_both_languages() {
        let src = Source::ui();
        let ops = vec![
            TicketOp::Title { ticket: 1, before: "a".into(), after: "b".into(), source: src.clone() },
            TicketOp::Status { ticket: 1, before: None, after: Some(2), source: src.clone() },
            TicketOp::Mark { ticket: 1, kind: MarkKind::Flag, before: false, after: true, source: src.clone() },
            TicketOp::Mark { ticket: 1, kind: MarkKind::Star, before: false, after: true, source: src.clone() },
            TicketOp::Parent { ticket: 1, before: None, after: Some(2), source: src.clone() },
            TicketOp::Created { snapshot: TicketSnapshot { ticket: ticket(1, 1, "x"), values: vec![] }, present: true, source: src.clone() },
        ];
        for op in ops {
            let (de, en) = (op_label(&op, Lang::De), op_label(&op, Lang::En));
            assert!(!de.is_empty() && !en.is_empty());
            assert_ne!(de, en, "{op:?}");
        }
    }
}
