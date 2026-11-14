//! Reiter „Boards“ der Projektansicht: Kanban-Board mit Spalten und Karten, Board-Wechsler, Schnellfilter, Ziehen & Fallenlassen
//! (Spalte wechseln = Statuswechsel mit allen Regeln, Umsortieren = Backlog-Rang) und der Aufruf des Konfigurations-Dialogs.
//!
//! Einhängepunkte:
//! * `PlanUi::boards_tab` wird vom Reiter „Boards“ aufgerufen (`mod.rs`, volle Breite und Höhe, ohne die gemeinsame Rollfläche);
//! * `PlanUi::request_open_board_ticket` ist der (noch leere) Einsprung „Karte öffnen“ für die Ticket-Detailseite;
//! * `PlanUi::board_dialogs` (in `board_cfg.rs`) zeichnet die Dialoge.
//!
//! Die reine Logik steht in `board_logic.rs`; hier ist nur Zeichnen und Ereignisse einsammeln.

use super::board_cfg::{CfgDlg, DeleteDlg, NeedsDlg};
use super::board_logic::{self as logic, BoardData, Card, Pick, Quick};
use super::helpers::color_of;
use super::quick_menu::QuickTarget;
use super::widgets;
use super::{Cx, PlanUi};
use crate::planning::history::Source;
use crate::planning::projects::Project;
use crate::planning::summary::ProjectSummary;
use crate::planning::ticket_error::TicketError;
use crate::planning::tickets::{get_ticket, move_rank, set_flagged, set_starred};
use crate::planning::transition::{available_transitions, transition_ticket, TransitionOption};
use crate::planning::views::boards::{default_board_for, duplicate_board, ensure_default_board, list_boards, Board, BoardConfig};
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Align, Color32, Id, Key, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Breite einer Spalte in Pixeln.
const COL_W: f32 = 276.0;
const COL_PAD: f32 = 8.0;
/// Abstand zum Rand, ab dem beim Ziehen automatisch gerollt wird, und Rollgeschwindigkeit je Bild.
const EDGE: f32 = 44.0;
const SCROLL_STEP: f32 = 14.0;
/// So oft werden die Karten neu geladen, falls sich anderswo (Agent, anderer Reiter) etwas ändert.
const REFRESH: Duration = Duration::from_millis(2500);

/// Meldung unten in der Mitte (Fehler/Hinweis nach dem Fallenlassen).
struct Notice {
    text: String,
    error: bool,
    at: Instant,
}

/// Eine laufende Ziehaktion.
pub(super) struct Drag {
    pub ticket_id: i64,
    /// Abstand des Mauszeigers von der linken oberen Ecke der Karte beim Anfassen.
    grab: Vec2,
    /// Mögliche Zielstatus der Karte (einmal beim Anfassen gelesen).
    options: Vec<TransitionOption>,
    status_id: Option<i64>,
}

pub(super) struct State {
    project: i64,
    boards: Vec<Board>,
    pub(super) board_id: i64,
    pub(super) quick: Quick,
    data: Option<BoardData>,
    data_key: Option<(i64, BoardConfig, Quick, Lang)>,
    loaded_at: Instant,
    sum_seen: Option<Rc<ProjectSummary>>,
    /// Neu laden (nach eigener Änderung).
    pub(super) stale: bool,
    /// Sichtbare Seiten je (Board, Spalte).
    pages: HashMap<(i64, usize), usize>,
    drag: Option<Drag>,
    /// Karte unter dem Mauszeiger im letzten Bild (Ziel der Taste `.`).
    hover: Option<i64>,
    h_off: f32,
    col_off: HashMap<usize, f32>,
    pending_h: Option<f32>,
    pending_v: Option<(usize, f32)>,
    notice: Option<Notice>,
    pub(super) cfg: Option<CfgDlg>,
    pub(super) del: Option<DeleteDlg>,
    pub(super) needs: Option<NeedsDlg>,
}

impl Default for State {
    fn default() -> Self {
        State {
            project: 0,
            boards: Vec::new(),
            board_id: 0,
            quick: Quick::default(),
            data: None,
            data_key: None,
            loaded_at: Instant::now(),
            sum_seen: None,
            stale: true,
            pages: HashMap::new(),
            drag: None,
            hover: None,
            h_off: 0.0,
            col_off: HashMap::new(),
            pending_h: None,
            pending_v: None,
            notice: None,
            cfg: None,
            del: None,
            needs: None,
        }
    }
}

impl State {
    pub(super) fn current(&self) -> Option<&Board> {
        self.boards.iter().find(|b| b.id == self.board_id)
    }

    pub(super) fn board_names(&self) -> Vec<String> {
        self.boards.iter().map(|b| b.name.clone()).collect()
    }

    pub(super) fn notify(&mut self, text: String, error: bool) {
        self.notice = Some(Notice { text, error, at: Instant::now() });
    }
}

/// Was beim Zeichnen angestoßen wurde; wird nach dem Zeichnen ausgeführt.
enum Act {
    Open(i64),
    Star(i64, bool),
    Flag(i64, bool),
    More(usize),
    Start { ticket: i64, grab: Vec2 },
    /// Mauszeiger über der Karte.
    Hover(i64),
    /// Schnellmenü für die Karte öffnen (Kontextmenü).
    Quick(i64),
}

impl PlanUi {
    /// Einsprung „Karte öffnen“ (Klick auf eine Karte). Bleibt leer, bis die Ticket-Detailseite angebunden ist.
    pub(super) fn request_open_board_ticket(&mut self, _ticket_id: i64) {
        // TODO tickets-ui: hier die Detailseite des Tickets öffnen.
    }

    // ------------------------------------------------------------------ Laden

    /// Liste der Boards, Auswahl und Standard-Board bereitstellen.
    fn boards_sync(&mut self, cx: &Cx, sum: &Rc<ProjectSummary>, proj: &Project) {
        let st = &mut self.boards;
        let changed_sum = !st.sum_seen.as_ref().is_some_and(|s| Rc::ptr_eq(s, sum));
        if st.project != proj.id {
            *st = State { project: proj.id, ..State::default() };
        }
        if !(st.stale || changed_sum) {
            return;
        }
        st.stale = false;
        st.sum_seen = Some(sum.clone());
        let mut list = list_boards(cx.conn, proj.id).unwrap_or_default();
        // Standard-Board beim ersten Öffnen eines Projekts mit Workflow-Typen (nur einmal, auch wenn später alle gelöscht werden)
        let seeded_key = format!("boards_seeded_{}", proj.id);
        if list.is_empty() && cx.db.meta(&seeded_key).is_none() && !logic::workflow_status_ids(sum).is_empty() {
            cx.db.set_meta(&seeded_key, "1");
            if ensure_default_board(cx.conn, proj.id, cx.lang).is_ok() {
                list = list_boards(cx.conn, proj.id).unwrap_or_default();
            }
        }
        if !list.iter().any(|b| b.id == st.board_id) {
            let saved = cx.db.meta(&format!("expert_board_{}", proj.id)).and_then(|v| v.parse::<i64>().ok());
            st.board_id = saved.filter(|id| list.iter().any(|b| b.id == *id)).or_else(|| list.first().map(|b| b.id)).unwrap_or(0);
        }
        st.boards = list;
        st.data_key = None;
    }

    pub(super) fn select_board(&mut self, cx: &Cx, id: i64) {
        if self.boards.board_id != id {
            self.boards.board_id = id;
            self.boards.data_key = None;
            self.boards.drag = None;
            self.boards.h_off = 0.0;
            self.boards.col_off.clear();
            cx.db.set_meta(&format!("expert_board_{}", self.boards.project), &id.to_string());
        }
    }

    /// Karten neu laden, wenn sich Board, Konfiguration, Filter oder Sprache geändert haben oder die Zeit abgelaufen ist.
    fn boards_data(&mut self, cx: &Cx, sum: &ProjectSummary) {
        let st = &mut self.boards;
        let Some(board) = st.boards.iter().find(|b| b.id == st.board_id) else {
            st.data = None;
            return;
        };
        let key = (board.id, board.config.clone(), st.quick.clone(), cx.lang);
        let fresh = st.data_key.as_ref() == Some(&key) && st.loaded_at.elapsed() < REFRESH;
        if fresh || (st.drag.is_some() && st.data.is_some()) {
            return;
        }
        match logic::load_board(cx.conn, sum, &board.config, &st.quick, cx.lang) {
            Ok(d) => st.data = Some(d),
            Err(e) => {
                st.data = None;
                st.notice = Some(Notice { text: e.message(cx.lang), error: true, at: Instant::now() });
            }
        }
        st.data_key = Some(key);
        st.loaded_at = Instant::now();
    }

    // ------------------------------------------------------------------ Reiter

    /// Der Reiter „Boards“ (nimmt die ganze Breite und Höhe unter den Reitern).
    pub(super) fn boards_tab(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        self.boards_sync(cx, &sum, proj);
        self.boards_header(ui, cx, &sum);
        let Some(board) = self.boards.current().cloned() else {
            ui.add_space(S2);
            if logic::workflow_status_ids(&sum).is_empty() {
                widgets::hint(ui, lang.t("Dieses Projekt hat noch keinen Typ mit Workflow. Lege im Reiter „Workflows“ Status für einen Typ fest, dann erscheinen hier die Boards.", "This project has no type with a workflow yet. Set statuses for a type in the “Workflows” tab and the boards appear here."));
            } else {
                widgets::hint(ui, lang.t("Noch kein Board. Lege mit „Neues Board“ eines an.", "No board yet. Create one with “New board”."));
            }
            return;
        };
        self.boards_quick(ui, cx, &sum, &board);
        self.boards_data(cx, &sum);
        if let Some(d) = &self.boards.data {
            if let Some(p) = &d.source_problem {
                widgets::error(ui, &format!("{} {p}", lang.t("Die Quelle des Boards lässt sich nicht auswerten:", "The board's source cannot be evaluated:")));
            }
            if !d.unassigned.is_empty() {
                let names: Vec<String> = d.unassigned.iter().filter_map(|id| sum.status(*id)).map(|s| s.display(lang)).collect();
                widgets::hint(ui, &format!("{} {}", lang.t("Status ohne Spalte (nicht auf dem Board):", "Statuses without a column (not on the board):"), names.join(", ")));
            }
        }
        ui.add_space(S1);
        if board.config.columns.is_empty() {
            widgets::hint(ui, lang.t("Dieses Board hat noch keine Spalten. Mit „Bearbeiten“ legst du sie an.", "This board has no columns yet. Use “Edit” to add them."));
        } else {
            self.draw_board(ui, cx, &sum, &board);
        }
        self.boards_notice(ui.ctx());
        // Karten gelegentlich auffrischen (Agenten, andere Reiter); beim Ziehen laufend zeichnen
        if self.boards.drag.is_some() {
            ui.ctx().request_repaint();
        } else {
            ui.ctx().request_repaint_after(REFRESH);
        }
    }

    fn boards_header(&mut self, ui: &mut Ui, cx: &Cx, sum: &Rc<ProjectSummary>) {
        let lang = cx.lang;
        let mut pick: Option<i64> = None;
        let (mut new, mut dup, mut edit, mut del) = (false, false, false, false);
        let cur = self.boards.current().map(|b| (b.id, b.name.clone()));
        let total = self.boards.data.as_ref().map(|d| d.total);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some((cur_id, name)) = &cur {
                egui::ComboBox::from_id_salt("board_pick").selected_text(name.as_str()).width(220.0).show_ui(ui, |ui| {
                    for b in &self.boards.boards {
                        if ui.selectable_label(b.id == *cur_id, &b.name).clicked() {
                            pick = Some(b.id);
                        }
                    }
                });
            }
            new = ui.button(format!("+ {}", lang.t("Neues Board", "New board"))).clicked();
            if cur.is_some() {
                dup = ui.button(lang.t("Duplizieren", "Duplicate")).clicked();
                edit = ui.button(lang.t("Bearbeiten", "Edit")).clicked();
                del = widgets::small_danger(ui, lang.t("Löschen", "Delete")).clicked();
            }
            if let Some(n) = total {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("{n} {}", lang.t("Tickets", "tickets"))).small().color(style::dim(ui)));
                });
            }
        });
        if let Some(id) = pick {
            self.select_board(cx, id);
        }
        if new {
            self.open_new_board(cx, sum);
        }
        if dup {
            if let Some((id, _)) = &cur {
                match duplicate_board(cx.conn, *id, lang) {
                    Ok(new_id) => {
                        self.boards.stale = true;
                        self.select_board(cx, new_id);
                        self.dirty = true;
                    }
                    Err(e) => self.boards.notify(e.message(lang), true),
                }
            }
        }
        if edit {
            self.open_edit_board(cx);
        }
        if del {
            if let Some((id, name)) = cur {
                self.boards.del = Some(DeleteDlg { id, name });
            }
        }
        if let Some(b) = self.boards.current() {
            if !b.description.trim().is_empty() {
                ui.label(RichText::new(&b.description).small().color(style::dim(ui)));
            }
        }
    }

    /// Dialog „Neues Board“ mit der Konfiguration des Standard-Boards (alle Status der Workflows) und freiem Namen.
    fn open_new_board(&mut self, cx: &Cx, sum: &ProjectSummary) {
        let lang = cx.lang;
        let mut nb = match default_board_for(cx.conn, sum.project.id, lang) {
            Ok(b) => b,
            Err(e) => {
                self.boards.notify(e.message(lang), true);
                return;
            }
        };
        nb.name = logic::free_name(&nb.name, &self.boards.board_names());
        self.boards.cfg = Some(CfgDlg::new(None, nb.name, String::new(), nb.config));
    }

    fn open_edit_board(&mut self, cx: &Cx) {
        let _ = cx;
        if let Some(b) = self.boards.current() {
            self.boards.cfg = Some(CfgDlg::new(Some(b.id), b.name.clone(), b.description.clone(), b.config.clone()));
        }
    }

    // ------------------------------------------------------------------ Schnellfilter

    fn boards_quick(&mut self, ui: &mut Ui, cx: &Cx, sum: &ProjectSummary, board: &Board) {
        let lang = cx.lang;
        let q = &mut self.boards.quick;
        // Chips nur für Typen, die das Board zeigt
        let types: Vec<(i64, String, Color32)> =
            sum.types.iter().filter(|t| t.ticket_type.workflow && (board.config.type_filter.is_empty() || board.config.type_filter.contains(&t.ticket_type.id))).map(|t| (t.ticket_type.id, t.ticket_type.display(lang), color_of(&t.ticket_type.color))).collect();
        q.types.retain(|t| types.iter().any(|x| x.0 == *t));
        ui.add_space(S1);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            ui.add(egui::TextEdit::singleline(&mut q.text).id(Id::new("board_quick_text")).desired_width(190.0).hint_text(lang.t("Suchen: Titel, Schlüssel, Text …", "Search: title, key, text …")).margin(egui::Margin::symmetric(8, 4)));
            for (id, name, col) in &types {
                let on = q.types.contains(id);
                if style::pill(ui, name, None, *col, on).on_hover_text(lang.t("Nur diesen Typ zeigen (mehrere wählbar)", "Show only this type (several can be picked)")).clicked() {
                    if on {
                        q.types.retain(|t| t != id);
                    } else {
                        q.types.push(*id);
                    }
                }
            }
            if icon_toggle(ui, "star", q.starred, Color32::from_rgb(0xe5, 0xc0, 0x4b)).on_hover_text(lang.t("Nur meine Favoriten (Stern)", "Only my favorites (star)")).clicked() {
                q.starred = !q.starred;
            }
            if icon_toggle(ui, "flag", q.flagged, Color32::from_rgb(0xe0, 0x6c, 0x5b)).on_hover_text(lang.t("Nur markierte Tickets (Flagge)", "Only flagged tickets")).clicked() {
                q.flagged = !q.flagged;
            }
            if q.active() && ui.add(egui::Button::new(RichText::new(lang.t("Zurücksetzen", "Clear")).small())).clicked() {
                *q = Quick::default();
            }
        });
    }

    // ------------------------------------------------------------------ Board zeichnen

    fn draw_board(&mut self, ui: &mut Ui, cx: &Cx, sum: &Rc<ProjectSummary>, board: &Board) {
        let lang = cx.lang;
        let Some(data) = self.boards.data.take() else { return };
        let ctx = ui.ctx().clone();
        let cfg = board.config.clone();
        let board_rect = ui.available_rect_before_wrap();
        let col_h = (board_rect.height() - 16.0).max(160.0);
        let mut acts: Vec<Act> = Vec::new();
        let dragging = self.boards.drag.as_ref().map(|d| d.ticket_id);
        let has_points = data.has_points;
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        // Zustand je Spalte beim Ziehen
        let picks: Vec<Option<Pick>> = data
            .columns
            .iter()
            .map(|c| {
                let d = self.boards.drag.as_ref()?;
                Some(logic::pick_target(&c.status_ids, d.status_id, &d.options))
            })
            .collect();
        let mut col_rects: Vec<Rect> = Vec::new();
        let mut body_rects: Vec<Rect> = Vec::new();
        let mut card_rects: Vec<Vec<(i64, Rect)>> = Vec::new();
        let mut new_cols: HashMap<usize, f32> = HashMap::new();
        let pending_h = self.boards.pending_h.take();
        let pages = self.boards.pages.clone();
        let pending_v = self.boards.pending_v.take();
        let panel = ui.visuals().faint_bg_color;
        let mut sa = egui::ScrollArea::horizontal().id_salt(("board_h", board.id)).auto_shrink([false, false]).max_height(col_h + 14.0);
        if let Some(x) = pending_h {
            sa = sa.horizontal_scroll_offset(x);
        }
        let out = sa.show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (ci, col) in data.columns.iter().enumerate() {
                    let shown = logic::shown_count(pages.get(&(board.id, ci)).copied().unwrap_or(1), col.cards.len());
                    let (rect, body, cards, off) = ui
                        .allocate_ui_with_layout(Vec2::new(COL_W, col_h), Layout::top_down(Align::Min), |ui| {
                            draw_column(ui, lang, &cfg, board.id, ci, col, shown, has_points, dragging, pending_v.filter(|(c, _)| *c == ci).map(|(_, v)| v), &mut acts, panel)
                        })
                        .inner;
                    col_rects.push(rect);
                    body_rects.push(body);
                    card_rects.push(cards);
                    new_cols.insert(ci, off);
                }
            });
        });
        self.boards.h_off = out.state.offset.x;
        self.boards.col_off = new_cols;
        // Dimmen und Markieren beim Ziehen
        if dragging.is_some() {
            let painter = ui.painter().clone();
            for (ci, r) in col_rects.iter().enumerate() {
                let Some(Some(p)) = picks.get(ci) else { continue };
                let over = pointer.is_some_and(|ptr| r.contains(ptr));
                if !p.allowed() {
                    painter.rect_filled(*r, 8.0, ui.visuals().window_fill.gamma_multiply(0.62));
                    let text = match p {
                        Pick::Blocked => lang.t("Durch eine Bedingung gesperrt", "Blocked by a condition"),
                        _ => lang.t("Nicht erlaubt", "Not allowed"),
                    };
                    if over {
                        painter.rect_stroke(*r, 8.0, Stroke::new(1.5_f32, widgets::DANGER), StrokeKind::Inside);
                    }
                    let g = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(12.5), widgets::DANGER);
                    painter.galley(Pos2::new(r.center().x - g.size().x / 2.0, r.top() + 40.0), g, widgets::DANGER);
                } else if over {
                    painter.rect_stroke(*r, 8.0, Stroke::new(2.0_f32, style::accent()), StrokeKind::Inside);
                }
            }
        }
        // Ziehen: Einfügemarke, Karte am Mauszeiger, Esc, Loslassen, Rollen am Rand
        if let Some(tid) = dragging {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let mut drop_at: Option<(usize, usize)> = None;
            if let Some(ptr) = pointer {
                if let Some(ci) = col_rects.iter().position(|r| r.x_range().contains(ptr.x) && r.y_range().contains(ptr.y)) {
                    let cards: Vec<&(i64, Rect)> = card_rects[ci].iter().filter(|(id, _)| *id != tid).collect();
                    let mids: Vec<f32> = cards.iter().map(|(_, r)| r.center().y).collect();
                    let idx = logic::insert_index(&mids, ptr.y);
                    drop_at = Some((ci, idx));
                    if picks.get(ci).and_then(|p| p.as_ref()).is_some_and(Pick::allowed) {
                        let y = if cards.is_empty() {
                            body_rects[ci].top() + 4.0
                        } else if idx == 0 {
                            cards[0].1.top() - 3.0
                        } else {
                            cards[idx - 1].1.bottom() + 3.0
                        };
                        let r = col_rects[ci];
                        let line = [Pos2::new(r.left() + 8.0, y), Pos2::new(r.right() - 8.0, y)];
                        ui.painter().line_segment(line, Stroke::new(3.0_f32, style::accent()));
                    }
                }
                // automatisches Rollen am Rand
                if board_rect.contains(ptr) || (ptr.y > board_rect.top() - 30.0 && ptr.y < board_rect.bottom() + 30.0) {
                    if ptr.x < board_rect.left() + EDGE {
                        self.boards.pending_h = Some((self.boards.h_off - SCROLL_STEP).max(0.0));
                    } else if ptr.x > board_rect.right() - EDGE {
                        self.boards.pending_h = Some(self.boards.h_off + SCROLL_STEP);
                    }
                }
                if let Some(ci) = col_rects.iter().position(|r| r.x_range().contains(ptr.x)) {
                    let b = body_rects[ci];
                    let cur = self.boards.col_off.get(&ci).copied().unwrap_or(0.0);
                    if ptr.y < b.top() + EDGE && ptr.y > b.top() - 30.0 {
                        self.boards.pending_v = Some((ci, (cur - SCROLL_STEP).max(0.0)));
                    } else if ptr.y > b.bottom() - EDGE && ptr.y < b.bottom() + 30.0 {
                        self.boards.pending_v = Some((ci, cur + SCROLL_STEP));
                    }
                }
            }
            // Karte am Mauszeiger
            if let (Some(ptr), Some(card)) = (pointer, data.card(tid)) {
                let grab = self.boards.drag.as_ref().map(|d| d.grab).unwrap_or(Vec2::ZERO);
                egui::Area::new(Id::new("board_ghost")).order(egui::Order::Tooltip).fixed_pos(ptr - grab).interactable(false).constrain(false).show(&ctx, |ui| {
                    ui.set_width(COL_W - 2.0 * COL_PAD - 4.0);
                    ui.set_opacity(0.92);
                    paint_card(ui, card, &cfg, lang, true);
                });
            }
            let esc = ctx.input(|i| i.key_pressed(Key::Escape));
            let down = ctx.input(|i| i.pointer.primary_down());
            if esc {
                self.boards.drag = None;
            } else if !down {
                let drag = self.boards.drag.take();
                if let (Some(drag), Some((ci, idx))) = (drag, drop_at) {
                    self.board_drop(cx, sum, &data, drag, ci, idx);
                }
            }
        }
        // Ereignisse der Karten
        self.boards.hover = None;
        for a in acts {
            match a {
                Act::Hover(id) => self.boards.hover = Some(id),
                Act::Quick(id) => self.open_quick_menu(vec![id]),
                Act::Open(id) => self.request_open_board_ticket(id),
                Act::Star(id, on) => self.board_mark(cx, id, true, on),
                Act::Flag(id, on) => self.board_mark(cx, id, false, on),
                Act::More(ci) => {
                    *self.boards.pages.entry((board.id, ci)).or_insert(1) += 1;
                }
                Act::Start { ticket, grab } => {
                    let t = get_ticket(cx.conn, ticket).ok();
                    let options = available_transitions(cx.conn, ticket).unwrap_or_default();
                    self.boards.drag = Some(Drag { ticket_id: ticket, grab, options, status_id: t.and_then(|t| t.status_id) });
                }
            }
        }
        if self.boards.data.is_none() {
            self.boards.data = Some(data);
        }
        // Taste „.“: Schnellmenü für die Karte unter dem Mauszeiger (nicht beim Ziehen).
        if let (Some(id), None) = (self.boards.hover, self.boards.drag.as_ref()) {
            self.quick_shortcut(&ctx, &[QuickTarget { ticket_id: id }]);
        }
    }

    // ------------------------------------------------------------------ Fallenlassen

    /// Karte in Spalte `ci` an Position `idx` fallen lassen: Statuswechsel (wenn die Spalte eine andere ist) und Rang.
    fn board_drop(&mut self, cx: &Cx, sum: &ProjectSummary, data: &BoardData, drag: Drag, ci: usize, idx: usize) {
        let lang = cx.lang;
        let Some(col) = data.columns.get(ci) else { return };
        let id = drag.ticket_id;
        let ticket = match get_ticket(cx.conn, id) {
            Ok(t) => t,
            Err(e) => {
                self.boards.notify(e.message(lang), true);
                return;
            }
        };
        let ids: Vec<i64> = col.cards.iter().map(|c| c.ticket.id).collect();
        let before = logic::rank_before(&ids, id, idx);
        let key = ticket.key();
        match logic::pick_target(&col.status_ids, ticket.status_id, &drag.options) {
            Pick::Stay => self.board_rank(cx, id, before),
            Pick::Go { status_id, .. } => match transition_ticket(cx.conn, id, status_id, &[], &Source::ui()) {
                Ok(_) => self.board_rank(cx, id, before),
                Err(TicketError::TransitionNeedsFields(missing)) => {
                    let to_name = sum.status(status_id).map(|s| s.display(lang)).unwrap_or_default();
                    self.boards.needs = Some(NeedsDlg::new(sum, &ticket, status_id, to_name, before, &missing));
                }
                Err(e) => self.boards.notify(refusal(lang, &key, &col.title, &e), true),
            },
            Pick::Blocked | Pick::NotAllowed => {
                let e = logic::reason_for_column(cx.conn, &ticket, &col.status_ids).unwrap_or(TicketError::TransitionNotAllowed);
                self.boards.notify(refusal(lang, &key, &col.title, &e), true);
            }
        }
    }

    /// Rang setzen (nach dem Statuswechsel oder beim Umsortieren).
    pub(super) fn board_rank(&mut self, cx: &Cx, id: i64, before: Option<i64>) {
        match move_rank(cx.conn, id, before) {
            Ok(_) => self.boards.stale = true,
            Err(e) => self.boards.notify(e.message(cx.lang), true),
        }
        self.boards.stale = true;
        self.boards.data_key = None;
    }

    fn board_mark(&mut self, cx: &Cx, id: i64, star: bool, on: bool) {
        let r = if star { set_starred(cx.conn, id, on, &Source::ui()) } else { set_flagged(cx.conn, id, on, &Source::ui()) };
        if let Err(e) = r {
            self.boards.notify(e.message(cx.lang), true);
        }
        self.boards.data_key = None;
    }

    // ------------------------------------------------------------------ Meldung

    fn boards_notice(&mut self, ctx: &egui::Context) {
        let Some(n) = &self.boards.notice else { return };
        if n.at.elapsed() > Duration::from_secs(6) {
            self.boards.notice = None;
            return;
        }
        let (text, error) = (n.text.clone(), n.error);
        egui::Area::new(Id::new("board_toast")).anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -18.0)).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
            let mut f = egui::Frame::popup(ui.style()).corner_radius(egui::CornerRadius::same(10));
            if error {
                f = f.stroke(Stroke::new(1.2_f32, widgets::DANGER));
            }
            f.show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.label(text);
            });
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

/// Eine Spalte: Kopf (Titel, Anzahl, Summe Story Points) und die Karten. Liefert (Rahmen, Kartenbereich, Kartenrechtecke, Rollstand).
#[allow(clippy::too_many_arguments)]
fn draw_column(ui: &mut Ui, lang: Lang, cfg: &BoardConfig, board_id: i64, ci: usize, col: &logic::Column, shown: usize, has_points: bool, dragging: Option<i64>, v_off: Option<f32>, acts: &mut Vec<Act>, panel: Color32) -> (Rect, Rect, Vec<(i64, Rect)>, f32) {
    let col_h = ui.available_height();
    let mut cards_out: Vec<(i64, Rect)> = Vec::new();
    let mut body = Rect::NOTHING;
    let mut offset = 0.0;
    let frame = egui::Frame::NONE.fill(panel).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(COL_PAD as i8));
    let fr = frame.show(ui, |ui| {
        ui.set_width(COL_W - 2.0 * COL_PAD);
        ui.set_min_height(col_h - 2.0 * COL_PAD);
        // Kopf
        ui.horizontal(|ui| {
            ui.set_height(22.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(col.cards.len().to_string()).small().color(style::dim(ui)));
                if has_points {
                    ui.label(RichText::new(format!("Σ {}", logic::points_text(col.points))).small().color(style::dim(ui))).on_hover_text(lang.t("Summe der Story Points", "Sum of story points"));
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, color_of(&col.color));
                    ui.add(egui::Label::new(RichText::new(&col.title).strong()).truncate().selectable(false));
                });
            });
        });
        ui.add_space(S1);
        let avail = ui.available_height();
        let mut sa = egui::ScrollArea::vertical().id_salt(("board_col", board_id, ci)).auto_shrink([false, false]).max_height(avail);
        if let Some(y) = v_off {
            sa = sa.vertical_scroll_offset(y);
        }
        let out = sa.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.set_width(COL_W - 2.0 * COL_PAD - 6.0);
            if col.cards.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(lang.t("Keine Karten", "No cards")).small().color(style::dim(ui)));
            }
            for card in col.cards.iter().take(shown) {
                let is_dragged = dragging == Some(card.ticket.id);
                let rect = ui
                    .scope(|ui| {
                        if is_dragged {
                            ui.set_opacity(0.35);
                        }
                        paint_card(ui, card, cfg, lang, false)
                    })
                    .inner;
                let resp = ui.interact(rect, Id::new(("board_card", card.ticket.id)), Sense::click_and_drag());
                if resp.hovered() && dragging.is_none() {
                    acts.push(Act::Hover(card.ticket.id));
                    ui.painter().rect_stroke(rect, 7.0, Stroke::new(1.2_f32, style::accent().gamma_multiply(0.8)), StrokeKind::Inside);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.drag_started() {
                    let ptr = ui.ctx().input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.min);
                    acts.push(Act::Start { ticket: card.ticket.id, grab: ptr - rect.min });
                } else if resp.clicked() {
                    acts.push(Act::Open(card.ticket.id));
                }
                resp.context_menu(|ui| {
                    if ui.button(lang.t("Öffnen", "Open")).clicked() {
                        acts.push(Act::Open(card.ticket.id));
                        ui.close();
                    }
                    let star = if card.ticket.starred { lang.t("Stern entfernen", "Remove star") } else { lang.t("Mit Stern markieren", "Add star") };
                    if ui.button(star).clicked() {
                        acts.push(Act::Star(card.ticket.id, !card.ticket.starred));
                        ui.close();
                    }
                    let flag = if card.ticket.flagged { lang.t("Markierung entfernen", "Remove flag") } else { lang.t("Markieren (Flagge)", "Flag") };
                    if ui.button(flag).clicked() {
                        acts.push(Act::Flag(card.ticket.id, !card.ticket.flagged));
                        ui.close();
                    }
                    if ui.button(format!("{} (.)", lang.t("Schnellmenü …", "Quick menu …"))).clicked() {
                        acts.push(Act::Quick(card.ticket.id));
                        ui.close();
                    }
                });
                cards_out.push((card.ticket.id, rect));
            }
            if col.cards.len() > shown {
                let rest = col.cards.len() - shown;
                if ui.add_sized([ui.available_width(), 24.0], egui::Button::new(RichText::new(format!("{rest} {} …", lang.t("weitere", "more"))).small())).clicked() {
                    acts.push(Act::More(ci));
                }
            }
        });
        body = out.inner_rect;
        offset = out.state.offset.y;
    });
    (fr.response.rect, body, cards_out, offset)
}

/// Hinweistext nach einem abgelehnten Fallenlassen: „PAN-5 kann nicht nach „Done“: Grund“.
fn refusal(lang: Lang, key: &str, column: &str, e: &TicketError) -> String {
    match lang {
        Lang::De => format!("{key} kann nicht nach {}: {}", lang.q(column), e.message(lang)),
        Lang::En => format!("{key} cannot move to {}: {}", lang.q(column), e.message(lang)),
    }
}

// ------------------------------------------------------------------ Karte

/// Kleiner Ein/Aus-Knopf mit Icon (Stern, Flagge).
fn icon_toggle(ui: &mut Ui, icon: &str, on: bool, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(26.0, 24.0), Sense::click());
    let v = ui.visuals();
    if on {
        ui.painter().rect_filled(rect, 12.0, color.gamma_multiply(0.25));
        ui.painter().rect_stroke(rect, 12.0, Stroke::new(1.0_f32, color.gamma_multiply(0.7)), StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 12.0, v.widgets.hovered.bg_fill);
    }
    let c = if on { color } else { v.weak_text_color() };
    widgets::type_icon(ui.painter(), icon, rect.center(), 15.0, c);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Kleines Etikett (Kartenfeld, Label).
fn chip(ui: &mut Ui, text: &str, color: Option<Color32>) {
    let (fill, fg) = match color {
        Some(c) => (c.gamma_multiply(0.24), c),
        None => (ui.visuals().widgets.inactive.bg_fill.gamma_multiply(1.6), ui.visuals().text_color().gamma_multiply(0.85)),
    };
    egui::Frame::NONE.fill(fill).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(text).small().color(fg)).selectable(false));
    });
}

/// Zeichnet eine Karte und liefert ihr Rechteck. Der Inhalt hat keine eigene Bedienung (die Karte wird als Ganzes angefasst).
fn paint_card(ui: &mut Ui, card: &Card, cfg: &BoardConfig, lang: Lang, ghost: bool) -> Rect {
    let _ = lang;
    let v = ui.visuals();
    let fill = if ghost { v.window_fill } else { v.widgets.inactive.bg_fill };
    let stroke = if ghost { Stroke::new(1.5_f32, style::accent()) } else { Stroke::new(1.0_f32, v.widgets.noninteractive.bg_stroke.color) };
    let dimmed = card.done && !ghost;
    let type_col = color_of(&card.type_color);
    let fr = egui::Frame::NONE.fill(fill).stroke(stroke).corner_radius(egui::CornerRadius::same(7)).inner_margin(egui::Margin { left: 11, right: 8, top: 6, bottom: 7 });
    let r = fr.show(ui, |ui| {
        ui.set_width(ui.available_width());
        if dimmed {
            ui.set_opacity(0.55);
        }
        ui.spacing_mut().item_spacing = Vec2::new(5.0, 4.0);
        ui.horizontal(|ui| {
            if cfg.show_type_icon {
                widgets::type_badge(ui, card.type_icon.as_deref(), type_col, 16.0);
            }
            if cfg.show_key {
                ui.add(egui::Label::new(RichText::new(card.ticket.key()).small().monospace().color(style::dim(ui))).selectable(false));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if card.ticket.starred {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    widgets::type_icon(ui.painter(), "star", r.center(), 14.0, Color32::from_rgb(0xe5, 0xc0, 0x4b));
                }
                if card.ticket.flagged {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    widgets::type_icon(ui.painter(), "flag", r.center(), 14.0, Color32::from_rgb(0xe0, 0x6c, 0x5b));
                }
            });
        });
        let mut title = RichText::new(&card.ticket.title);
        if card.done {
            title = title.strikethrough();
        }
        ui.add(egui::Label::new(title).selectable(false));
        if !card.fields.is_empty() || !card.labels.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 3.0);
                for f in &card.fields {
                    let col = f.color.as_deref().map(color_of);
                    let text = if col.is_some() { f.text.clone() } else { format!("{}: {}", f.name, f.text) };
                    chip(ui, &text, col);
                }
                for l in &card.labels {
                    chip(ui, &format!("# {l}"), Some(style::accent()));
                }
            });
        }
    });
    let rect = r.response.rect;
    // Farbstreifen des Typs links
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.left() + 1.0, rect.top() + 5.0), Pos2::new(rect.left() + 4.0, rect.bottom() - 5.0)), 1.5, type_col.gamma_multiply(if dimmed { 0.5 } else { 1.0 }));
    rect
}
