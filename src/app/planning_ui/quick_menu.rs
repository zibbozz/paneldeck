//! Schnellmenü (Taste `.`): zentriertes Overlay mit Suchfeld, Liste der Aktionen für ein oder mehrere Tickets, kleinen
//! Eingabeformularen und einer Zusammenfassung bei Fehlern. Die Logik steht in `quick_logic.rs`, das Laden und Ausführen in
//! `quick_exec.rs`; hier ist nur Zeichnen und Tasten einsammeln.
//!
//! **Einhängepunkte** (damit Ticket-Liste, Board und Detailansicht das Menü öffnen können):
//! * [`PlanUi::open_quick_menu`] öffnet das Menü für die Tickets (Kennungen). Das Menü baut sich im nächsten Bild auf.
//! * [`PlanUi::quick_shortcut`] ist die Taste `.`: eine Ansicht ruft sie jedes Bild mit den markierten bzw. dem aktuellen Ticket
//!   auf (leer = nichts markiert); sie öffnet das Menü nur, wenn gerade kein Textfeld getippt wird.
//! * [`QuickTarget`] beschreibt ein Ticket für diese Aufrufe (`QuickTarget { ticket_id }`).
//! * Ergebnisse (Toast mit „Rückgängig“, Sitzungsstart) holt die Anwendung mit [`PlanUi::take_quick_out`] ab;
//!   [`PlanUi::quick_undo`] nimmt die Änderung hinter einem Toast-Knopf zurück.
//!
//! Tasten: Pfeile wählen, Enter führt aus bzw. bestätigt das Formular, Esc geht zurück bzw. schließt, 1–9 wählt bei leerem
//! Suchfeld die nummerierte Zeile. Die Liste ist zweisprachig durchsuchbar (auch Namen der anderen Sprache).

use super::helpers::color_of;
use super::quick_exec::{self as exec, BatchResult, SessionRequest};
use super::quick_fill::{self, Fill};
use super::quick_logic::{self as logic, FormState, Item, ItemKey, Row};
use super::widgets;
use super::{Cx, PlanUi};
use crate::planning::ops::TicketOp;
use crate::planning::quick::input::InputKind;
use crate::state::Lang;
use crate::style::{self, S1, S2};
use crate::undo::Op;
use eframe::egui::{self, Align2, Color32, Event, Id, Key, Modifiers, Pos2, Rect, RichText, Sense, Ui, Vec2};
use std::collections::VecDeque;

/// Ein Ticket als Ziel des Schnellmenüs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct QuickTarget {
    pub ticket_id: i64,
}

/// Was das Menü an die Anwendung zurückgibt.
pub(in crate::app) enum Out {
    Toast { text: String, error: bool, undo: Option<u64> },
    Session(SessionRequest),
}

/// Anzahl der Rückgängig-Schritte, die hinter Toasts bereitgehalten werden.
const MAX_UNDO: usize = 20;
const ROW_H: f32 = 42.0;

enum Stage {
    List,
    Form { item: usize, form: FormState },
    /// Statuswechsel, Schnellaktion „Status ändern“ oder „Ticket anlegen“ mit fehlenden Pflichtfeldern: Formular für genau
    /// diese Felder.
    Fill { item: usize, fill: Fill },
    /// Ergebnis mit Fehlschlägen (Zeilen „PAN-5: Grund“).
    Result { text: String, lines: Vec<String> },
}

struct Menu {
    ids: Vec<i64>,
    project_id: i64,
    title: String,
    items: Vec<Item>,
    work_kinds: Vec<(i64, String)>,
    query: String,
    sel: usize,
    stage: Stage,
    recent: Vec<ItemKey>,
    /// Hinweis unter der Liste (Grund einer ausgegrauten Zeile).
    note: Option<String>,
    /// Das nächste Bild setzt den Fokus ins erste Eingabefeld.
    focus: bool,
    scroll_to_sel: bool,
    /// Vom Knopf in der Ticket-Ansicht geöffnet (eine bestimmte Aktion): Zurück/Esc schließt das Menü, statt die Liste zu zeigen.
    direct: bool,
}

#[derive(Default)]
pub(super) struct State {
    /// Tickets und (bei einem Knopf der Ticket-Ansicht) die Schnellaktion, die sofort gestartet wird.
    pending: Option<(Vec<i64>, Option<i64>)>,
    menu: Option<Menu>,
    out: Vec<Out>,
    undo: VecDeque<(u64, TicketOp)>,
    next_token: u64,
}

impl State {
    pub(super) fn is_open(&self) -> bool {
        self.menu.is_some() || self.pending.is_some()
    }
}

/// Eingaben dieses Bildes (vor dem Zeichnen eingesammelt).
#[derive(Default)]
struct Keys {
    up: bool,
    down: bool,
    enter: bool,
    ctrl_enter: bool,
    esc: bool,
    digit: Option<usize>,
}

/// `keep_enter`: ein mehrzeiliges Feld ist offen, Enter bleibt für den Zeilenumbruch (bestätigt wird mit Strg+Enter).
/// Ist eine Auswahlliste (Dropdown, Version, Meilenstein, Terminal) aufgeklappt, gehören Esc, Enter und die Pfeile ihr: Esc
/// schließt nur die Liste, nicht das Formular, Enter wählt den Eintrag statt das Formular zu bestätigen.
fn collect_keys(ctx: &egui::Context, take_digits: bool, keep_enter: bool) -> Keys {
    let popup = egui::Popup::is_any_open(ctx);
    ctx.input_mut(|i| {
        let mut k = Keys::default();
        k.esc = !popup && i.consume_key(Modifiers::NONE, Key::Escape);
        k.up = !popup && i.consume_key(Modifiers::NONE, Key::ArrowUp);
        k.down = !popup && i.consume_key(Modifiers::NONE, Key::ArrowDown);
        k.ctrl_enter = i.consume_key(Modifiers::COMMAND, Key::Enter);
        k.enter = !popup && !keep_enter && i.consume_key(Modifiers::NONE, Key::Enter);
        if take_digits {
            let mut found = None;
            i.events.retain(|e| match e {
                Event::Text(t) if found.is_none() && logic::digit_of(t).is_some() => {
                    found = logic::digit_of(t);
                    false
                }
                _ => true,
            });
            k.digit = found;
        }
        k
    })
}

impl PlanUi {
    /// Öffnet das Schnellmenü für die Tickets. Das Menü wird im nächsten Bild geladen (Datenbank), leere Liste = nichts tun.
    pub(in crate::app) fn open_quick_menu(&mut self, target: Vec<i64>) {
        if !target.is_empty() {
            self.quick.pending = Some((target, None));
        }
    }

    /// Startet die Schnellaktion `action_id` für die Tickets, so als wäre sie im Schnellmenü gewählt worden (Knöpfe der
    /// Ticket-Ansicht): erst das Pflichtfeld-Formular bzw. die Titel-Eingabe, wenn nötig, sonst sofort ausführen; Ergebnis als
    /// Toast mit „Rückgängig“ bzw. Sitzungsstart über [`PlanUi::take_quick_out`]. Das Menü selbst bleibt unsichtbar, solange
    /// nichts einzugeben ist.
    pub(in crate::app) fn open_quick_action(&mut self, target: Vec<i64>, action_id: i64) {
        if !target.is_empty() {
            self.quick.pending = Some((target, Some(action_id)));
        }
    }

    /// Die Taste `.`: öffnet das Menü für `targets`, wenn kein Textfeld getippt wird und kein Menü offen ist. `true` = geöffnet.
    pub(in crate::app) fn quick_shortcut(&mut self, ctx: &egui::Context, targets: &[QuickTarget]) -> bool {
        if targets.is_empty() || self.quick.is_open() || ctx.wants_keyboard_input() {
            return false;
        }
        let pressed = ctx.input(|i| {
            i.key_pressed(Key::Period) && !i.modifiers.command && !i.modifiers.alt
        });
        if pressed {
            self.open_quick_menu(targets.iter().map(|t| t.ticket_id).collect());
        }
        pressed
    }

    /// Ergebnisse seit dem letzten Aufruf (Toasts, Sitzungsstarts).
    pub(in crate::app) fn take_quick_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.quick.out)
    }

    /// Nimmt die Änderung hinter dem Toast-Knopf zurück. Liefert den Text für einen Toast.
    pub(in crate::app) fn quick_undo(&mut self, conn: &rusqlite::Connection, token: u64, lang: Lang) -> Result<String, String> {
        let pos = self.quick.undo.iter().position(|(t, _)| *t == token).ok_or_else(|| lang.t("Das lässt sich nicht mehr rückgängig machen.", "This can no longer be undone.").to_string())?;
        let Some((_, op)) = self.quick.undo.remove(pos) else { return Err(String::new()) };
        op.inverse().apply(conn)?;
        self.dirty = true;
        self.boards.stale = true;
        Ok(lang.t("Rückgängig gemacht.", "Undone.").to_string())
    }

    fn quick_open(&mut self, cx: &Cx, ids: Vec<i64>, action: Option<i64>) {
        match exec::load(cx.conn, &ids, cx.lang) {
            Ok(l) => {
                let recent = cx.db.meta(&format!("quick_recent_{}", l.project_id)).map(|s| logic::recent_from_text(&s)).unwrap_or_default();
                let mut m = Menu { ids, project_id: l.project_id, title: l.title, items: l.items, work_kinds: l.work_kinds, query: String::new(), sel: 0, stage: Stage::List, recent, note: None, focus: true, scroll_to_sel: false, direct: action.is_some() };
                if let Some(id) = action {
                    // Knopf in der Ticket-Ansicht: genau diese Zeile wie im Menü ausführen (Formular, Titel, Ausführen).
                    let Some(idx) = m.items.iter().position(|i| i.key == ItemKey::Action(id)) else {
                        let text = cx.lang.t("Die Aktion ist hier nicht verfügbar.", "The action is not available here.").to_string();
                        self.quick.out.push(Out::Toast { text, error: true, undo: None });
                        return;
                    };
                    self.quick_run_item(cx, &mut m, idx, chrono::Local::now().naive_local());
                    if m.ids.is_empty() {
                        return; // schon ausgeführt, nichts mehr zu zeigen
                    }
                    if let (Stage::List, Some(note)) = (&m.stage, m.note.take()) {
                        // nicht ausführbar (ausgegraut, ungültig): als Hinweis melden, die Liste würde hier nur verwirren
                        self.quick.out.push(Out::Toast { text: note, error: true, undo: None });
                        return;
                    }
                }
                self.quick.menu = Some(m);
            }
            Err(e) => self.quick.out.push(Out::Toast { text: e, error: true, undo: None }),
        }
    }

    /// Zeichnet das Menü (aus `PlanUi::dialogs`).
    pub(super) fn quick_dialogs(&mut self, ctx: &egui::Context, cx: &Cx) {
        if let Some((ids, action)) = self.quick.pending.take() {
            self.quick_open(cx, ids, action);
        }
        let Some(mut m) = self.quick.menu.take() else { return };
        let lang = cx.lang;
        let mut close = false;
        let now = chrono::Local::now().naive_local();

        // Tasten dieses Bildes. Ziffern nur in der Liste bei leerem Suchfeld.
        let in_list = matches!(m.stage, Stage::List);
        let keep_enter = match &m.stage {
            Stage::Form { item, .. } => matches!(m.items.get(*item).map(|i| (&i.key, &i.input)), Some((ItemKey::Comment, _)) | Some((_, Some(InputKind::RichTextEditor { .. })))),
            Stage::Fill { fill, .. } => !fill.enter_submits(),
            _ => false,
        };
        let keys = collect_keys(ctx, in_list && m.query.is_empty(), keep_enter);
        let prefixes: Vec<String> = self.projects.iter().map(|p| p.key.clone()).collect();
        let rows: Vec<Row> = if in_list { logic::layout(&m.items, &m.query, &m.recent, lang) } else { Vec::new() };
        let numbered = logic::numbered(&rows, &m.items);
        let mut run: Option<usize> = None;
        if in_list {
            if keys.down && !rows.is_empty() {
                m.sel = (m.sel + 1).min(rows.len() - 1);
                m.scroll_to_sel = true;
            }
            if keys.up {
                m.sel = m.sel.saturating_sub(1);
                m.scroll_to_sel = true;
            }
            m.sel = m.sel.min(rows.len().saturating_sub(1));
            if keys.enter || keys.ctrl_enter {
                run = rows.get(m.sel).map(|r| r.item);
            }
            if let Some(n) = keys.digit {
                run = numbered.get(n).copied().or(run);
            }
            if keys.esc {
                close = true;
            }
        } else if keys.esc {
            if m.direct {
                close = true;
            } else {
                m.stage = Stage::List;
                m.focus = true;
            }
        }
        let submit = match &m.stage {
            Stage::Form { item, .. } => {
                let multiline = matches!(m.items.get(*item).map(|i| (&i.key, &i.input)), Some((ItemKey::Comment, _)) | Some((_, Some(InputKind::RichTextEditor { .. }))));
                (keys.ctrl_enter || (keys.enter && !multiline)).then_some(())
            }
            Stage::Fill { fill, .. } => (keys.ctrl_enter || (keys.enter && fill.enter_submits())).then_some(()),
            _ => None,
        };

        // Zeichnen
        let mut back = false;
        let mut apply = false;
        let mut clicked: Option<usize> = None;
        let mut query_changed = false;
        let resp = egui::Modal::new(Self::modal_id("quick")).area(egui::Modal::default_area(Self::modal_id("quick")).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 90.0))).show(ctx, |ui| {
            ui.set_width(widgets::modal_width(ctx, 600.0));
            ui.label(RichText::new(&m.title).small().color(style::dim(ui)));
            ui.add_space(S1);
            match &mut m.stage {
                Stage::List => {
                    let te = egui::TextEdit::singleline(&mut m.query)
                        .id(Id::new("quick_query"))
                        .hint_text(lang.t("Aktion oder Feld suchen …", "Search an action or field …"))
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(10, 8));
                    let r = ui.add(te);
                    if m.focus {
                        r.request_focus();
                        m.focus = false;
                    }
                    query_changed = r.changed();
                    ui.add_space(S1);
                    let h = (ctx.content_rect().height() - 260.0).clamp(160.0, 420.0);
                    egui::ScrollArea::vertical().id_salt("quick_rows").max_height(h).auto_shrink([false, true]).show(ui, |ui| {
                        if rows.is_empty() {
                            ui.add_space(S2);
                            ui.label(RichText::new(lang.t("Nichts gefunden.", "Nothing found.")).color(style::dim(ui)));
                        }
                        for (n, row) in rows.iter().enumerate() {
                            if let Some(h) = &row.header {
                                ui.add_space(4.0);
                                ui.label(RichText::new(h).small().strong().color(style::dim(ui)));
                            }
                            let it = &m.items[row.item];
                            let number = numbered.iter().position(|i| *i == row.item).filter(|_| m.query.is_empty());
                            let (rect, r) = draw_row(ui, it, n == m.sel, number, lang);
                            if r.clicked() {
                                clicked = Some(row.item);
                            }
                            if r.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                                m.sel = n;
                            }
                            if n == m.sel && m.scroll_to_sel {
                                ui.scroll_to_rect(rect, None);
                            }
                        }
                        m.scroll_to_sel = false;
                    });
                    if let Some(note) = &m.note {
                        ui.add_space(S1);
                        ui.horizontal_wrapped(|ui| widgets::error(ui, note));
                    }
                    ui.add_space(S1);
                    widgets::hint(ui, lang.t("↑↓ wählen · Enter ausführen · 1–9 Kürzel · Esc schließen", "↑↓ choose · Enter run · 1–9 shortcut · Esc close"));
                }
                Stage::Form { item, form } => {
                    let it = &m.items[*item];
                    let focus = std::mem::take(&mut m.focus);
                    let (b, a) = draw_form(ui, ctx, cx, &m.ids, m.project_id, &m.work_kinds, it, form, focus);
                    back = b;
                    apply = a;
                }
                Stage::Fill { fill, .. } => {
                    m.focus = false;
                    let (b, a) = quick_fill::draw(ui, cx, &m.ids, &prefixes, fill);
                    back = b;
                    apply = a;
                }
                Stage::Result { text, lines } => {
                    ui.label(RichText::new(text.as_str()).strong());
                    ui.add_space(S1);
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([true, true]).show(ui, |ui| {
                        for l in lines.iter() {
                            ui.horizontal_wrapped(|ui| widgets::error(ui, l));
                        }
                    });
                    ui.add_space(S2);
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, lang.t("Zurück", "Back")).clicked() {
                            back = true;
                        }
                        if widgets::secondary(ui, lang.t("Schließen", "Close")).clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if resp.should_close() && in_list {
            close = true;
        }
        if query_changed {
            m.sel = 0;
            m.note = None;
            m.scroll_to_sel = true;
        }
        if back {
            if m.direct {
                close = true;
            } else {
                m.stage = Stage::List;
                m.focus = true;
            }
        }
        if clicked.is_some() {
            run = clicked;
        }
        if let Some(idx) = run {
            self.quick_run_item(cx, &mut m, idx, now);
        }
        if apply || submit.is_some() {
            self.quick_submit(cx, &mut m, now);
        }
        if close {
            return;
        }
        // Das Menü bleibt offen, solange es nicht geschlossen oder erfolgreich ausgeführt wurde (Marker: leere Id-Liste).
        if !m.ids.is_empty() {
            self.quick.menu = Some(m);
        }
        ctx.request_repaint();
    }

    fn quick_run_item(&mut self, cx: &Cx, m: &mut Menu, idx: usize, now: chrono::NaiveDateTime) {
        let Some(it) = m.items.get(idx).cloned() else { return };
        if let Some(reason) = &it.disabled {
            m.note = Some(format!("{}: {reason}", it.label));
            return;
        }
        m.note = None;
        // Statuswechsel und Schnellaktionen mit leeren Pflichtfeldern: erst das Formular für diese Felder. „Ticket anlegen“
        // ohne Titelvorgabe fragt zuerst den Titel (danach `quick_submit`).
        let prepared = match it.key {
            ItemKey::Transition(to) => Some(quick_fill::prepare(cx.conn, &m.ids, to, cx.lang)),
            ItemKey::Action(id) if !it.ask_title => Some(quick_fill::prepare_action(cx.conn, &m.ids, id, now, cx.lang)),
            _ => None,
        };
        match prepared {
            Some(Ok(Some(fill))) => {
                m.stage = Stage::Fill { item: idx, fill };
                return;
            }
            Some(Err(e)) => {
                m.note = Some(e);
                return;
            }
            Some(Ok(None)) | None => {}
        }
        if it.needs_form() {
            m.stage = Stage::Form { item: idx, form: FormState::for_item(&it) };
            m.focus = true;
            return;
        }
        if let Some(res) = exec::run_direct(cx.conn, &m.ids, &it.key, now, cx.lang) {
            self.quick_finish(cx, m, &it, res);
        }
    }

    /// „Verschieben“/„Anlegen“ im Pflichtfeld-Formular: Werte prüfen, Zeile mit den Werten ausführen (ein Rückgängig-Schritt).
    fn quick_submit_fill(&mut self, cx: &Cx, m: &mut Menu, now: chrono::NaiveDateTime) {
        let lang = cx.lang;
        let Stage::Fill { item, fill } = &mut m.stage else { return };
        let Some(it) = m.items.get(*item).cloned() else { return };
        let Some(values) = quick_fill::collect(fill, lang) else {
            fill.error = Some(lang.t("Bitte die markierten Felder prüfen.", "Please check the marked fields.").to_string());
            return;
        };
        fill.error = None;
        let res = match &fill.run {
            quick_fill::Run::Status { to } => exec::run_transition_with(cx.conn, &m.ids, *to, &values, lang),
            quick_fill::Run::Action { id, title } => exec::run_action_with(cx.conn, &m.ids, *id, title.as_deref(), &values, now, lang),
        };
        // Scheitert alles (z. B. ein noch leeres Pflichtfeld), bleibt das Formular mit den Eingaben offen und zeigt den Grund;
        // bei mehreren Tickets mit dem Schlüssel des ersten.
        if res.ok.is_empty() {
            if let Some((key, why)) = res.failed.first() {
                fill.error = Some(if m.ids.len() == 1 { why.clone() } else { format!("{key}: {why}") });
                return;
            }
        }
        self.quick_finish(cx, m, &it, res);
    }

    fn quick_submit(&mut self, cx: &Cx, m: &mut Menu, now: chrono::NaiveDateTime) {
        if matches!(m.stage, Stage::Fill { .. }) {
            self.quick_submit_fill(cx, m, now);
            return;
        }
        let lang = cx.lang;
        let Stage::Form { item, form } = &mut m.stage else { return };
        let Some(it) = m.items.get(*item).cloned() else { return };
        let sub = match logic::validate(&it, form, now, lang) {
            Ok(s) => s,
            Err(e) => {
                form.error = Some(e);
                return;
            }
        };
        // „Ticket anlegen“: fehlen dem neuen Ticket Pflichtfelder, folgt nach dem Titel das Formular dafür.
        if let (ItemKey::Action(id), logic::Submit::NewTitle(t)) = (&it.key, &sub) {
            match quick_fill::prepare_action(cx.conn, &m.ids, *id, now, lang) {
                Ok(Some(fill)) => {
                    m.stage = Stage::Fill { item: m.items.iter().position(|i| i.key == it.key).unwrap_or(0), fill: fill.with_title(t) };
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    form.error = Some(e);
                    return;
                }
            }
        }
        let res = match (&it.key, &sub) {
            (ItemKey::Action(id), logic::Submit::NewTitle(t)) => exec::run_action(cx.conn, &m.ids, *id, Some(t), now, lang),
            _ => exec::run_submit(cx.conn, &m.ids, &sub, now, lang),
        };
        // Fehler eines einzelnen Tickets bleiben im Formular sichtbar.
        if res.ok.is_empty() && m.ids.len() == 1 {
            if let (Some((_, why)), Stage::Form { form, .. }) = (res.failed.first(), &mut m.stage) {
                form.error = Some(why.clone());
                return;
            }
        }
        self.quick_finish(cx, m, &it, res);
    }

    /// Ergebnis auswerten: zuletzt benutzt, Rückgängig, Toast, Sitzungen; bei Fehlschlägen die Zusammenfassung zeigen.
    fn quick_finish(&mut self, cx: &Cx, m: &mut Menu, it: &Item, res: BatchResult) {
        let lang = cx.lang;
        if !res.ok.is_empty() {
            logic::push_recent(&mut m.recent, it.key.clone());
            cx.db.set_meta(&format!("quick_recent_{}", m.project_id), &logic::recent_to_text(&m.recent));
            self.dirty = true;
            self.boards.stale = true;
        }
        let mut text = logic::summary_text(&it.label, res.ok.len(), res.failed.len(), lang);
        if !res.created.is_empty() {
            text = format!("{text} {}", match lang {
                Lang::De => format!("Angelegt: {}.", res.created.join(", ")),
                Lang::En => format!("Created: {}.", res.created.join(", ")),
            });
        }
        if let Some(op) = res.undo_op() {
            self.quick.next_token += 1;
            let token = self.quick.next_token;
            self.quick.undo.push_back((token, op));
            while self.quick.undo.len() > MAX_UNDO {
                self.quick.undo.pop_front();
            }
            self.quick.out.push(Out::Toast { text: text.clone(), error: false, undo: Some(token) });
        } else if !res.ok.is_empty() {
            self.quick.out.push(Out::Toast { text: text.clone(), error: false, undo: None });
        }
        for s in res.sessions.iter().cloned() {
            self.quick.out.push(Out::Session(s));
        }
        if res.failed.is_empty() {
            m.ids.clear(); // Marker: erledigt, Menü schließt
        } else {
            m.stage = Stage::Result { text, lines: res.failure_lines() };
        }
    }
}

// ------------------------------------------------------------------ Zeichnen

fn draw_row(ui: &mut Ui, it: &Item, selected: bool, number: Option<usize>, lang: Lang) -> (Rect, egui::Response) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
    let enabled = it.enabled();
    let v = ui.visuals();
    if selected {
        ui.painter().rect_filled(rect, 6.0, style::accent().gamma_multiply(if enabled { 0.28 } else { 0.12 }));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.bg_fill.gamma_multiply(0.5));
    }
    let dim = style::dim(ui);
    let text_col = if enabled { v.text_color() } else { v.weak_text_color() };
    let mut x = rect.left() + 10.0;
    // Kürzel
    if let Some(n) = number {
        let b = Rect::from_center_size(Pos2::new(x + 8.0, rect.center().y), Vec2::splat(18.0));
        ui.painter().rect_stroke(b, 4.0, egui::Stroke::new(1.0_f32, dim), egui::StrokeKind::Inside);
        ui.painter().text(b.center(), Align2::CENTER_CENTER, (n + 1).to_string(), egui::FontId::proportional(11.0), dim);
    }
    x += 26.0;
    if let Some(c) = &it.color {
        ui.painter().circle_filled(Pos2::new(x + 4.0, rect.top() + 14.0), 4.0, color_of(c));
        x += 14.0;
    }
    ui.painter().text(Pos2::new(x, rect.top() + 14.0), Align2::LEFT_CENTER, &it.label, egui::FontId::proportional(14.0), text_col);
    // Zweite Zeile: Beschreibung bzw. Grund
    let (second, col) = match &it.disabled {
        Some(r) => (r.as_str(), Color32::from_rgb(0xe5, 0xa0, 0x4b)),
        None => (it.detail.as_str(), dim),
    };
    let mut job = egui::text::LayoutJob::simple_singleline(second.to_string(), egui::FontId::proportional(11.5), col);
    job.wrap.max_width = (rect.right() - x - 10.0).max(40.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(Pos2::new(x, rect.top() + 25.0), galley, col);
    let resp = if let Some(r) = &it.disabled { resp.on_hover_text(format!("{}: {r}", lang.t("Nicht verfügbar", "Not available"))) } else { resp };
    (rect, resp)
}

/// Das Formular zu einer Zeile. Liefert (zurück, anwenden).
#[allow(clippy::too_many_arguments)]
fn draw_form(ui: &mut Ui, ctx: &egui::Context, cx: &Cx, ids: &[i64], project_id: i64, work_kinds: &[(i64, String)], it: &Item, f: &mut FormState, focus: bool) -> (bool, bool) {
    let lang = cx.lang;
    let mut back = false;
    let mut apply = false;
    let heading = match (ids.len(), lang) {
        (1, _) => it.label.clone(),
        (n, Lang::De) => format!("{} ({n} Tickets)", it.label),
        (n, Lang::En) => format!("{} ({n} tickets)", it.label),
    };
    ui.label(RichText::new(heading).heading());
    ui.add_space(S1);
    let first_id = Id::new("quick_form_first");
    let mut first_resp: Option<egui::Response> = None;
    let single = |ui: &mut Ui, text: &mut String, hint: &str, id: Id| ui.add(egui::TextEdit::singleline(text).id(id).hint_text(hint).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 6)));
    match (&it.key, &it.input) {
        (ItemKey::Comment, _) => {
            widgets::hint(ui, lang.t("Kommentar (Markdown) · Strg+Enter sendet", "Comment (Markdown) · Ctrl+Enter sends"));
            first_resp = Some(ui.add(egui::TextEdit::multiline(&mut f.text).id(first_id).desired_rows(5).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 6))));
        }
        (ItemKey::LogTime, _) => {
            widgets::hint(ui, lang.t("Dauer, z. B. 1h 30m, 2d oder 1,5h", "Duration, e.g. 1h 30m, 2d or 1.5h"));
            first_resp = Some(single(ui, &mut f.text, lang.t("Dauer", "Duration"), first_id));
            ui.add_space(S1);
            widgets::hint(ui, lang.t("Beginn (optional): JJJJ-MM-TT oder JJJJ-MM-TT HH:MM – sonst endet die Zeit jetzt", "Start (optional): YYYY-MM-DD or YYYY-MM-DD HH:MM – otherwise the time ends now"));
            single(ui, &mut f.start, lang.t("Beginn", "Start"), Id::new("quick_form_start"));
            if !work_kinds.is_empty() {
                ui.add_space(S1);
                let sel = f.work_kind.and_then(|k| work_kinds.iter().find(|w| w.0 == k)).map(|w| w.1.clone()).unwrap_or_else(|| lang.t("(keine Angabe)", "(not specified)").to_string());
                egui::ComboBox::from_id_salt("quick_work_kind").selected_text(sel).width(240.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.work_kind, None, lang.t("(keine Angabe)", "(not specified)"));
                    for (id, name) in work_kinds {
                        ui.selectable_value(&mut f.work_kind, Some(*id), name);
                    }
                });
            }
            ui.add_space(S1);
            single(ui, &mut f.comment, lang.t("Kommentar (optional)", "Comment (optional)"), Id::new("quick_form_comment"));
        }
        (ItemKey::Action(_), _) => {
            widgets::hint(ui, lang.t("Titel des neuen Tickets", "Title of the new ticket"));
            first_resp = Some(single(ui, &mut f.text, lang.t("Titel", "Title"), first_id));
        }
        (ItemKey::Title, _) => {
            first_resp = Some(single(ui, &mut f.text, lang.t("Titel", "Title"), first_id));
        }
        (_, Some(kind)) if logic::is_list_input(kind) => {
            let multi = matches!(kind, InputKind::MultiSelect { .. });
            let r = single(ui, &mut f.text, &kind.hint(lang), first_id);
            first_resp = Some(r);
            ui.add_space(S1);
            let list = exec::choices(cx.conn, kind, project_id, &f.text, if matches!(kind, InputKind::TicketSearch { .. }) { ids } else { &[] }, lang);
            if !multi {
                let mut none = f.picked.is_empty();
                if widgets::radio(ui, &mut none, true, lang.t("(leer)", "(empty)")).clicked() {
                    f.picked.clear();
                }
            }
            egui::ScrollArea::vertical().id_salt("quick_choices").max_height(200.0).auto_shrink([true, true]).show(ui, |ui| {
                if list.is_empty() {
                    widgets::hint(ui, lang.t("Keine Treffer.", "No matches."));
                }
                for (id, name) in &list {
                    let mut on = f.picked.contains(id);
                    let label = if multi { name.clone() } else { name.clone() };
                    if multi {
                        if widgets::check(ui, &mut on, label).changed() {
                            if on {
                                f.picked.push(*id);
                            } else {
                                f.picked.retain(|p| p != id);
                            }
                        }
                    } else if widgets::radio(ui, &mut on, true, label).clicked() {
                        f.picked = vec![*id];
                    }
                }
            });
        }
        (_, Some(InputKind::Checkbox)) => {
            ui.horizontal(|ui| {
                let mut yes = f.flag == Some(true);
                let mut no = f.flag == Some(false);
                if widgets::radio(ui, &mut yes, true, lang.t("Ja", "Yes")).clicked() {
                    f.flag = Some(true);
                }
                if widgets::radio(ui, &mut no, true, lang.t("Nein", "No")).clicked() {
                    f.flag = Some(false);
                }
            });
        }
        (_, Some(kind @ InputKind::RichTextEditor { rows })) => {
            widgets::hint(ui, &format!("{} · {}", kind.hint(lang), lang.t("Strg+Enter übernimmt", "Ctrl+Enter applies")));
            first_resp = Some(ui.add(egui::TextEdit::multiline(&mut f.text).id(first_id).desired_rows(rows.unwrap_or(6) as usize).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 6))));
        }
        (_, Some(kind)) => {
            widgets::hint(ui, &kind.hint(lang));
            first_resp = Some(single(ui, &mut f.text, "", first_id));
            if matches!(kind, InputKind::Number { .. } | InputKind::Text { .. } | InputKind::Labels | InputKind::Date | InputKind::DateTime { .. }) {
                widgets::hint(ui, lang.t("Leer lassen leert das Feld.", "Leave empty to clear the field."));
            }
        }
        _ => {}
    }
    if focus {
        if let Some(r) = &first_resp {
            r.request_focus();
        } else {
            ctx.memory_mut(|m| m.surrender_focus(first_id));
        }
    }
    if let Some(e) = &f.error {
        ui.add_space(S1);
        ui.horizontal_wrapped(|ui| widgets::error(ui, e));
    }
    ui.add_space(S2);
    ui.horizontal(|ui| {
        if widgets::primary(ui, lang.t("Übernehmen", "Apply")).clicked() {
            apply = true;
        }
        if widgets::secondary(ui, lang.t("Zurück", "Back")).clicked() {
            back = true;
        }
    });
    (back, apply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::planning::history::Source;
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::tickets::get_ticket;

    /// Ein Bild lang zeichnen, mit den Ereignissen `events`.
    fn frame(ui_state: &mut PlanUi, ctx: &egui::Context, cx: &Cx, events: Vec<Event>) {
        let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            ui_state.quick_dialogs(ctx, cx);
        });
    }

    fn key(k: Key) -> Vec<Event> {
        vec![
            Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: Modifiers::NONE },
            Event::Key { key: k, physical_key: Some(k), pressed: false, repeat: false, modifiers: Modifiers::NONE },
        ]
    }

    fn db_of(conn: rusqlite::Connection) -> Db {
        Db { conn, path: std::path::PathBuf::new() }
    }

    #[test]
    fn menu_opens_lists_and_runs_a_status_row_with_undo() {
        let f = Fx::new();
        let t = f.mk(f.story, "Login");
        let db = db_of(f.c);
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::En };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        assert!(!ui.quick.is_open());
        ui.open_quick_menu(vec![]);
        assert!(!ui.quick.is_open(), "leere Liste öffnet nichts");
        ui.open_quick_menu(vec![t.id]);
        assert!(ui.quick.is_open());
        frame(&mut ui, &ctx, &cx, vec![]);
        let m = ui.quick.menu.as_ref().expect("Menü geladen");
        assert_eq!(m.ids, vec![t.id]);
        assert!(m.items.iter().any(|i| i.label == "In Progress" && i.enabled()));
        // Zeile „In Progress“ direkt ausführen
        let idx = m.items.iter().position(|i| i.label == "In Progress").unwrap();
        let mut m = ui.quick.menu.take().unwrap();
        ui.quick_run_item(&cx, &mut m, idx, chrono::Local::now().naive_local());
        assert!(m.ids.is_empty(), "erfolgreich = Menü schließt");
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.progress));
        let out = ui.take_quick_out();
        let Some(Out::Toast { undo: Some(token), error: false, .. }) = out.into_iter().next() else { panic!("Toast mit Rückgängig erwartet") };
        // Rückgängig
        assert!(ui.quick_undo(&db.conn, token, Lang::En).is_ok());
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.todo));
        assert!(ui.quick_undo(&db.conn, token, Lang::En).is_err(), "nur einmal");
        // zuletzt benutzt wurde gespeichert
        assert!(db.meta(&format!("quick_recent_{}", f.project)).unwrap().contains("status:"));
    }

    #[test]
    fn form_rows_validate_and_apply_and_multi_failures_show_a_summary() {
        let f = Fx::new();
        let a = f.mk(f.story, "A");
        let b = f.mk(f.epic, "B");
        let db = db_of(f.c);
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::En };
        let mut ui = PlanUi::default();
        let now = chrono::Local::now().naive_local();
        // einzelnes Ticket: Titel ändern, leerer Titel zeigt Fehler im Formular
        ui.open_quick_menu(vec![a.id]);
        let ctx = egui::Context::default();
        frame(&mut ui, &ctx, &cx, vec![]);
        let mut m = ui.quick.menu.take().unwrap();
        let idx = m.items.iter().position(|i| i.key == ItemKey::Title).unwrap();
        ui.quick_run_item(&cx, &mut m, idx, now);
        assert!(matches!(m.stage, Stage::Form { .. }));
        if let Stage::Form { form, .. } = &mut m.stage {
            form.text.clear();
        }
        ui.quick_submit(&cx, &mut m, now);
        let Stage::Form { form, .. } = &m.stage else { panic!() };
        assert!(form.error.is_some());
        if let Stage::Form { form, .. } = &mut m.stage {
            form.text = "Renamed".into();
        }
        ui.quick_submit(&cx, &mut m, now);
        assert!(m.ids.is_empty());
        assert_eq!(get_ticket(&db.conn, a.id).unwrap().title, "Renamed");
        // zwei Tickets verschiedenen Typs: Kommentar geht, eine Statusaktion nur teilweise → Zusammenfassung
        ui.open_quick_menu(vec![a.id, b.id]);
        frame(&mut ui, &ctx, &cx, vec![]);
        let mut m = ui.quick.menu.take().unwrap();
        let idx = m.items.iter().position(|i| i.key == ItemKey::Comment).unwrap();
        assert!(m.items[idx].enabled());
        ui.quick_run_item(&cx, &mut m, idx, now);
        if let Stage::Form { form, .. } = &mut m.stage {
            form.text = "hi all".into();
        }
        ui.quick_submit(&cx, &mut m, now);
        assert!(m.ids.is_empty());
        assert_eq!(crate::planning::comments::comments_of(&db.conn, b.id).unwrap().len(), 1);
        let _ = ui.take_quick_out();
        // Mehrfach mit Fehlschlag: Ticket ohne Workflow-Typ
        let no_wf = crate::planning::types::create_type(&db.conn, &crate::planning::types::NewType { project_id: f.project, key: None, name: Some("Note".into()), icon: None, color: "#445566".into(), workflow: false }).unwrap();
        let note = crate::planning::tickets::create_ticket(&db.conn, &crate::planning::tickets::NewTicket::new(f.project, no_wf, "N"), &Source::ui()).unwrap().0;
        let res = exec::run_transition(&db.conn, &[a.id, note.id], f.done, Lang::En);
        let mut m2 = Menu { ids: vec![a.id, note.id], project_id: f.project, title: String::new(), items: vec![], work_kinds: vec![], query: String::new(), sel: 0, stage: Stage::List, recent: vec![], note: None, focus: false, scroll_to_sel: false, direct: false };
        let it = Item { key: ItemKey::Transition(f.done), group: logic::Group::Status, label: "Done".into(), aliases: vec![], detail: String::new(), input: None, current: None, color: None, disabled: None, ask_title: false };
        ui.quick_finish(&cx, &mut m2, &it, res);
        let Stage::Result { text, lines } = &m2.stage else { panic!("Zusammenfassung erwartet") };
        assert!(text.contains("1 succeeded, 1 failed"), "{text}");
        assert_eq!(lines.len(), 1);
        assert!(!m2.ids.is_empty(), "Menü bleibt offen");
    }

    #[test]
    fn disabled_row_gives_a_note_and_keys_move_the_selection() {
        let f = Fx::new();
        let t = f.mk(f.story, "Login");
        let db = db_of(f.c);
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::De };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        ui.open_quick_menu(vec![t.id]);
        frame(&mut ui, &ctx, &cx, vec![]);
        frame(&mut ui, &ctx, &cx, key(Key::ArrowDown));
        assert_eq!(ui.quick.menu.as_ref().unwrap().sel, 1);
        frame(&mut ui, &ctx, &cx, key(Key::ArrowUp));
        assert_eq!(ui.quick.menu.as_ref().unwrap().sel, 0);
        // Esc schließt
        frame(&mut ui, &ctx, &cx, key(Key::Escape));
        assert!(ui.quick.menu.is_none());
        // Ziffer führt die nummerierte Zeile aus (Zeile 1 = Titel → Formular), Esc geht zurück, Esc schließt
        ui.open_quick_menu(vec![t.id]);
        frame(&mut ui, &ctx, &cx, vec![]);
        frame(&mut ui, &ctx, &cx, vec![Event::Text("1".into())]);
        assert!(matches!(ui.quick.menu.as_ref().unwrap().stage, Stage::Form { .. }));
        frame(&mut ui, &ctx, &cx, key(Key::Escape));
        assert!(matches!(ui.quick.menu.as_ref().unwrap().stage, Stage::List));
        // ausgegraute Zeile: Hinweis statt Ausführung
        let mut m = ui.quick.menu.take().unwrap();
        let idx = m.items.iter().position(|i| !i.enabled());
        if let Some(idx) = idx {
            ui.quick_run_item(&cx, &mut m, idx, chrono::Local::now().naive_local());
            assert!(m.note.is_some());
        }
    }

    /// Menü für `ids` laden und die Zeile des Zielstatus `to` ausführen; liefert das Menü.
    fn run_status_row(ui: &mut PlanUi, ctx: &egui::Context, cx: &Cx, ids: Vec<i64>, to: i64) -> Menu {
        ui.open_quick_menu(ids);
        frame(ui, ctx, cx, vec![]);
        let mut m = ui.quick.menu.take().expect("Menü geladen");
        let idx = m.items.iter().position(|i| i.key == ItemKey::Transition(to)).expect("Zeile des Zielstatus");
        ui.quick_run_item(cx, &mut m, idx, chrono::Local::now().naive_local());
        m
    }

    fn require_on_transition(f: &Fx, ty: i64, field: i64, from: i64, to: i64) {
        use crate::planning::workflow::{set_transition, TransitionDef};
        set_transition(&f.c, ty, &TransitionDef { from_status_id: from, to_status_id: to, required_fields: vec![field], ..Default::default() }).unwrap();
    }

    fn draft_of(m: &mut Menu, n: usize) -> &mut crate::app::planning_ui::ticket_draft::Draft {
        let Stage::Fill { fill, .. } = &mut m.stage else { panic!("Pflichtfeld-Formular erwartet") };
        &mut fill.fields[n].1
    }

    #[test]
    fn status_row_with_required_fields_opens_a_form_and_moves_with_the_values() {
        use crate::planning::fields::{FieldKind, FieldOptions};
        let f = Fx::new();
        let result = f.add_field(f.task, "Result", FieldKind::Text, FieldOptions::default(), false);
        require_on_transition(&f, f.task, result.id, f.todo, f.progress);
        let t = f.mk(f.task, "Needs a result");
        let db = db_of(f.c);
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::En };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        let m = run_status_row(&mut ui, &ctx, &cx, vec![t.id], f.progress);
        let Stage::Fill { fill, .. } = &m.stage else { panic!("statt der Fehlermeldung kommt das Formular") };
        assert_eq!(fill.fields.len(), 1);
        assert_eq!(fill.fields[0].0.id, result.id);
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.todo), "noch nichts verschoben");
        // Das Formular wird gezeichnet; „Verschieben“ ohne Wert zeigt den Fehler im Formular
        ui.quick.menu = Some(m);
        frame(&mut ui, &ctx, &cx, vec![]);
        let mut m = ui.quick.menu.take().unwrap();
        ui.quick_submit(&cx, &mut m, chrono::Local::now().naive_local());
        let Stage::Fill { fill, .. } = &m.stage else { panic!("bleibt im Formular") };
        assert!(fill.error.as_deref().is_some_and(|e| e.contains("Result")), "{:?}", fill.error);
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.todo));
        // mit Wert: Wechsel, Wert gespeichert, Rückgängig stellt beides her
        draft_of(&mut m, 0).text = "done well".into();
        ui.quick_submit(&cx, &mut m, chrono::Local::now().naive_local());
        assert!(m.ids.is_empty(), "erfolgreich = Menü schließt");
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.progress));
        assert_eq!(crate::planning::values::load_values(&db.conn, t.id).unwrap().get(&result.id), Some(&serde_json::json!("done well")));
        let Some(Out::Toast { undo: Some(token), error: false, .. }) = ui.take_quick_out().into_iter().next() else { panic!("Toast mit Rückgängig erwartet") };
        assert!(ui.quick_undo(&db.conn, token, Lang::En).is_ok());
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.todo));
        assert!(crate::planning::values::load_values(&db.conn, t.id).unwrap().get(&result.id).is_none());
    }

    #[test]
    fn required_field_form_is_drawn_in_both_languages_and_keys_work() {
        use crate::planning::fields::{FieldKind, FieldOptions};
        use crate::planning::sprints::releases::{create_release, NewRelease, ReleaseKind};
        use crate::planning::workflow::{set_transition, TransitionDef};
        let f = Fx::new();
        let result = f.add_field(f.task, "Result", FieldKind::Text, FieldOptions::default(), false);
        let version = f.add_field(f.task, "Fix version", FieldKind::Version, FieldOptions::default(), false);
        let term = f.add_field(f.task, "Session", FieldKind::Terminal, FieldOptions::default(), false);
        set_transition(&f.c, f.task, &TransitionDef { from_status_id: f.todo, to_status_id: f.progress, required_fields: vec![result.id, version.id, term.id], ..Default::default() }).unwrap();
        crate::planning::projects::update_project(&f.c, f.project, &crate::planning::projects::UpdateProject { use_versions: Some(true), ..Default::default() }).unwrap();
        let rel = create_release(&f.c, ReleaseKind::Version, &NewRelease::new(f.project, "1.0")).unwrap();
        let t = f.mk(f.task, "Many fields");
        let db = db_of(f.c);
        for lang in [Lang::De, Lang::En] {
            let cx = Cx { conn: &db.conn, db: &db, lang };
            let ctx = egui::Context::default();
            let mut ui = PlanUi::default();
            let m = run_status_row(&mut ui, &ctx, &cx, vec![t.id], f.progress);
            let Stage::Fill { fill, .. } = &m.stage else { panic!("Formular erwartet") };
            assert_eq!(fill.fields.len(), 3);
            assert!(fill.enter_submits());
            ui.quick.menu = Some(m);
            frame(&mut ui, &ctx, &cx, vec![]);
            // Esc: zurück zur Liste, das Menü bleibt offen
            frame(&mut ui, &ctx, &cx, key(Key::Escape));
            assert!(matches!(ui.quick.menu.as_ref().unwrap().stage, Stage::List));
        }
        // Version aus der Liste wählen, Terminal tippen, Enter bestätigt
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::En };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        let mut m = run_status_row(&mut ui, &ctx, &cx, vec![t.id], f.progress);
        draft_of(&mut m, 0).text = "r".into();
        draft_of(&mut m, 1).value = serde_json::json!(rel);
        draft_of(&mut m, 2).text = "term A".into();
        ui.quick.menu = Some(m);
        frame(&mut ui, &ctx, &cx, vec![]);
        frame(&mut ui, &ctx, &cx, key(Key::Enter));
        assert!(ui.quick.menu.is_none(), "Enter führt den Wechsel aus und schließt");
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.progress));
        let vals = crate::planning::values::load_values(&db.conn, t.id).unwrap();
        assert_eq!(vals.get(&version.id), Some(&serde_json::json!(rel)));
        assert_eq!(vals.get(&term.id), Some(&serde_json::json!("term A")));
    }

    #[test]
    fn several_tickets_share_one_form_and_each_gets_only_its_own_fields() {
        use crate::planning::fields::{FieldKind, FieldOptions};
        let f = Fx::new();
        let result = f.add_field(f.task, "Result", FieldKind::Text, FieldOptions::default(), false);
        require_on_transition(&f, f.task, result.id, f.todo, f.progress);
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let s = f.mk(f.story, "S");
        let db = db_of(f.c);
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::En };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        let mut m = run_status_row(&mut ui, &ctx, &cx, vec![a.id, b.id], f.progress);
        assert!(matches!(m.stage, Stage::Fill { .. }));
        draft_of(&mut m, 0).text = "same for both".into();
        ui.quick_submit(&cx, &mut m, chrono::Local::now().naive_local());
        for id in [a.id, b.id] {
            assert_eq!(get_ticket(&db.conn, id).unwrap().status_id, Some(f.progress));
            assert_eq!(crate::planning::values::load_values(&db.conn, id).unwrap().get(&result.id), Some(&serde_json::json!("same for both")));
        }
        // Werte fremder Felder werden pro Ticket herausgefiltert: eine Story ohne dieses Feld bewegt sich trotzdem
        let res = exec::run_transition_with(&db.conn, &[s.id], f.progress, &[(result.id, serde_json::json!("x"))], Lang::En);
        assert_eq!(res.ok.len(), 1, "{:?}", res.failed);
    }

    #[test]
    fn status_row_without_required_fields_needs_no_form() {
        let f = Fx::new();
        let t = f.mk(f.task, "Plain");
        assert!(super::quick_fill::prepare(&f.c, &[t.id], f.progress, Lang::En).unwrap().is_none());
    }

    fn add_action(f: &Fx, ty: i64, name: &str, p: crate::planning::quick::params::ActionParams) -> i64 {
        use crate::planning::quick::actions::{create_quick_action, NewQuickAction};
        create_quick_action(&f.c, &NewQuickAction::new(ty, name, p)).unwrap()
    }

    /// Menü für `ids` laden und die Zeile der Aktion ausführen; liefert das Menü.
    fn run_action_row(ui: &mut PlanUi, ctx: &egui::Context, cx: &Cx, ids: Vec<i64>, action: i64) -> Menu {
        ui.open_quick_menu(ids);
        frame(ui, ctx, cx, vec![]);
        let mut m = ui.quick.menu.take().expect("Menü geladen");
        let idx = m.items.iter().position(|i| i.key == ItemKey::Action(action)).expect("Zeile der Aktion");
        ui.quick_run_item(cx, &mut m, idx, chrono::Local::now().naive_local());
        m
    }

    #[test]
    fn change_status_action_with_required_fields_opens_the_form_and_undoes_in_one_step() {
        use crate::planning::fields::{FieldKind, FieldOptions};
        use crate::planning::quick::params::{ActionParams, ChangeStatusParams};
        use crate::planning::workflow::SetValue;
        let f = Fx::new();
        let result = f.add_field(f.task, "Result", FieldKind::Text, FieldOptions::default(), false);
        require_on_transition(&f, f.task, result.id, f.todo, f.progress);
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let start = add_action(&f, f.task, "Start", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.progress, set_values: vec![] }));
        // Eine Aktion, deren Setze-Wert das Pflichtfeld schon füllt, braucht kein Formular.
        let filled = add_action(&f, f.task, "Start filled", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.progress, set_values: vec![SetValue { field_id: result.id, value: serde_json::json!("auto") }] }));
        let db = db_of(f.c);
        for lang in [Lang::De, Lang::En] {
            let cx = Cx { conn: &db.conn, db: &db, lang };
            let ctx = egui::Context::default();
            let mut ui = PlanUi::default();
            let m = run_action_row(&mut ui, &ctx, &cx, vec![a.id, b.id], start);
            let Stage::Fill { fill, .. } = &m.stage else { panic!("statt der Fehlermeldung kommt das Formular") };
            assert_eq!((fill.fields.len(), fill.create), (1, false));
            assert_eq!(get_ticket(&db.conn, a.id).unwrap().status_id, Some(f.todo), "noch nichts verschoben");
            ui.quick.menu = Some(m);
            frame(&mut ui, &ctx, &cx, vec![]);
            frame(&mut ui, &ctx, &cx, key(Key::Escape));
            assert!(matches!(ui.quick.menu.as_ref().unwrap().stage, Stage::List), "Esc: zurück zur Liste");
        }
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::En };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        let mut m = run_action_row(&mut ui, &ctx, &cx, vec![a.id, b.id], start);
        ui.quick_submit(&cx, &mut m, chrono::Local::now().naive_local());
        assert!(matches!(m.stage, Stage::Fill { .. }), "ohne Wert bleibt das Formular");
        assert_eq!(get_ticket(&db.conn, a.id).unwrap().status_id, Some(f.todo));
        draft_of(&mut m, 0).text = "for both".into();
        ui.quick_submit(&cx, &mut m, chrono::Local::now().naive_local());
        assert!(m.ids.is_empty(), "erfolgreich = Menü schließt");
        for id in [a.id, b.id] {
            assert_eq!(get_ticket(&db.conn, id).unwrap().status_id, Some(f.progress));
            assert_eq!(crate::planning::values::load_values(&db.conn, id).unwrap().get(&result.id), Some(&serde_json::json!("for both")));
        }
        // EIN Toast, EIN Rückgängig-Schritt für beide Tickets
        let outs = ui.take_quick_out();
        assert_eq!(outs.len(), 1);
        let Some(Out::Toast { undo: Some(token), error: false, .. }) = outs.into_iter().next() else { panic!("Toast mit Rückgängig erwartet") };
        assert!(ui.quick_undo(&db.conn, token, Lang::En).is_ok());
        for id in [a.id, b.id] {
            assert_eq!(get_ticket(&db.conn, id).unwrap().status_id, Some(f.todo));
            assert!(crate::planning::values::load_values(&db.conn, id).unwrap().get(&result.id).is_none());
        }
        // Setze-Wert der Aktion füllt das Pflichtfeld: direkt ausführen
        let m = run_action_row(&mut ui, &ctx, &cx, vec![a.id], filled);
        assert!(m.ids.is_empty(), "kein Formular nötig");
        assert_eq!(crate::planning::values::load_values(&db.conn, a.id).unwrap().get(&result.id), Some(&serde_json::json!("auto")));
    }

    #[test]
    fn create_action_with_required_fields_asks_for_them_after_the_title() {
        use crate::planning::fields::{create_field, FieldKind, NewField};
        use crate::planning::quick::params::{ActionParams, CreateTicketParams, DefaultValue, FieldRef};
        let f = Fx::new();
        let must = {
            let mut n = NewField::new(f.task, "Must", FieldKind::Text);
            n.required = true;
            create_field(&f.c, &n).unwrap()
        };
        let with_default = {
            let mut n = NewField::new(f.task, "Kind", FieldKind::Text);
            n.required = true;
            n.default_value = Some(serde_json::json!("std"));
            create_field(&f.c, &n).unwrap()
        };
        let story = f.mk(f.story, "Login");
        let titled = add_action(&f, f.story, "Task titled", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.task, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Title, value: serde_json::json!("Fixed title") }] }));
        let untitled = add_action(&f, f.story, "Task ask", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.task, relation: None, defaults: vec![] }));
        let prefilled = add_action(&f, f.story, "Task prefilled", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.task, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Title, value: serde_json::json!("P") }, DefaultValue { field_id: FieldRef::Field(must), value: serde_json::json!("given") }] }));
        let db = db_of(f.c);
        let count = || db.conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get::<_, i64>(0)).unwrap();
        for lang in [Lang::De, Lang::En] {
            let cx = Cx { conn: &db.conn, db: &db, lang };
            let ctx = egui::Context::default();
            let mut ui = PlanUi::default();
            // Titel vorgegeben: sofort das Formular, nur für das Feld ohne Vorgabe
            let m = run_action_row(&mut ui, &ctx, &cx, vec![story.id], titled);
            let Stage::Fill { fill, .. } = &m.stage else { panic!("Formular erwartet") };
            assert_eq!((fill.fields.len(), fill.fields[0].0.id, fill.create), (1, must, true));
            assert_eq!(count(), 1, "noch nichts angelegt");
            ui.quick.menu = Some(m);
            frame(&mut ui, &ctx, &cx, vec![]);
            frame(&mut ui, &ctx, &cx, key(Key::Escape));
            assert!(matches!(ui.quick.menu.as_ref().unwrap().stage, Stage::List));
        }
        let cx = Cx { conn: &db.conn, db: &db, lang: Lang::En };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        let now = chrono::Local::now().naive_local();
        let mut m = run_action_row(&mut ui, &ctx, &cx, vec![story.id], titled);
        ui.quick_submit(&cx, &mut m, now);
        assert!(matches!(m.stage, Stage::Fill { .. }) && count() == 1, "ohne Wert wird nichts angelegt");
        draft_of(&mut m, 0).text = "because".into();
        ui.quick_submit(&cx, &mut m, now);
        assert!(m.ids.is_empty());
        assert_eq!(count(), 2);
        let Some(Out::Toast { undo: Some(token), text, .. }) = ui.take_quick_out().into_iter().next() else { panic!("Toast erwartet") };
        let key_new = text.split("Created: ").nth(1).unwrap().trim_end_matches('.').to_string();
        let new = crate::planning::tickets::ticket_by_key(&db.conn, &key_new).unwrap();
        assert_eq!(new.title, "Fixed title");
        let vals = crate::planning::values::load_values(&db.conn, new.id).unwrap();
        assert_eq!(vals.get(&must), Some(&serde_json::json!("because")));
        assert_eq!(vals.get(&with_default), Some(&serde_json::json!("std")), "Standardwert bleibt, kein Formularfeld");
        assert!(ui.quick_undo(&db.conn, token, Lang::En).is_ok());
        assert_eq!(count(), 1, "Rückgängig entfernt das Ticket");
        // Ohne Titelvorgabe: erst der Titel, dann das Formular, danach wird angelegt
        let mut m = run_action_row(&mut ui, &ctx, &cx, vec![story.id], untitled);
        assert!(matches!(m.stage, Stage::Form { .. }));
        if let Stage::Form { form, .. } = &mut m.stage {
            form.text = "Typed".into();
        }
        ui.quick_submit(&cx, &mut m, now);
        let Stage::Fill { fill, .. } = &m.stage else { panic!("nach dem Titel folgt das Formular") };
        assert_eq!(fill.run, quick_fill::Run::Action { id: untitled, title: Some("Typed".into()) });
        draft_of(&mut m, 0).text = "why".into();
        ui.quick_submit(&cx, &mut m, now);
        assert!(m.ids.is_empty());
        let Some(Out::Toast { text, .. }) = ui.take_quick_out().into_iter().next() else { panic!("Toast erwartet") };
        let key_new = text.split("Created: ").nth(1).unwrap().trim_end_matches('.').to_string();
        assert_eq!(crate::planning::tickets::ticket_by_key(&db.conn, &key_new).unwrap().title, "Typed");
        // Pflichtfeld schon durch die Aktion gefüllt: kein Formular
        let m = run_action_row(&mut ui, &ctx, &cx, vec![story.id], prefilled);
        assert!(m.ids.is_empty(), "kein Formular");
    }

    #[test]
    fn session_actions_never_open_a_form() {
        use crate::planning::quick::params::{ActionParams, StartSessionParams};
        let f = Fx::new();
        let story = f.mk(f.story, "Login");
        let act = add_action(&f, f.story, "Shell", ActionParams::StartSession(StartSessionParams::default()));
        let now = chrono::Local::now().naive_local();
        assert!(quick_fill::prepare_action(&f.c, &[story.id], act, now, Lang::En).unwrap().is_none());
        assert!(quick_fill::prepare_action(&f.c, &[story.id], 99_999, now, Lang::En).unwrap().is_none(), "unbekannte Aktion: der Lauf meldet es");
    }

    /// Knopf in der Ticket-Ansicht (`open_quick_action`): dieselbe Logik wie die Zeile im Menü, aber ohne Liste dazwischen.
    fn direct_case(lang: Lang) {
        use crate::planning::fields::{FieldKind, FieldOptions};
        use crate::planning::quick::params::{ActionParams, ChangeStatusParams, CreateTicketParams, DefaultValue, FieldRef, StartSessionParams};
        let f = Fx::new();
        let result = f.add_field(f.task, "Result", FieldKind::Text, FieldOptions::default(), false);
        require_on_transition(&f, f.task, result.id, f.todo, f.progress);
        let t = f.mk(f.task, "Needs a result");
        let story = f.mk(f.story, "Login");
        let start = add_action(&f, f.task, "Start", ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: f.progress, set_values: vec![] }));
        let ask = add_action(&f, f.story, "Task ask", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.task, relation: None, defaults: vec![] }));
        let fixed = add_action(&f, f.story, "Task fixed", ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: f.task, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Title, value: serde_json::json!("Fixed") }] }));
        let shell = add_action(&f, f.story, "Shell", ActionParams::StartSession(StartSessionParams::default()));
        let broken = add_action(&f, f.story, "Broken", ActionParams::StartSession(StartSessionParams::default()));
        f.c.execute("UPDATE quick_actions SET invalid = 1 WHERE id = ?1", [broken]).unwrap();
        let db = db_of(f.c);
        let cx = Cx { conn: &db.conn, db: &db, lang };
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        let count = || db.conn.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get::<_, i64>(0)).unwrap();
        // Ticket anlegen mit Titelvorgabe: sofort angelegt, kein Menü, Toast mit Rückgängig
        let before = count();
        ui.open_quick_action(vec![story.id], fixed);
        assert!(ui.quick.is_open());
        frame(&mut ui, &ctx, &cx, vec![]);
        assert!(ui.quick.menu.is_none(), "nichts einzugeben: kein Menü");
        assert_eq!(count(), before + 1);
        let Some(Out::Toast { undo: Some(token), error: false, .. }) = ui.take_quick_out().into_iter().next() else { panic!("Toast mit Rückgängig erwartet") };
        assert!(ui.quick_undo(&db.conn, token, lang).is_ok());
        assert_eq!(count(), before);
        // Sitzungsaktion: wie im Menü eine Anfrage an die Anwendung, nichts in der Datenbank
        ui.open_quick_action(vec![story.id], shell);
        frame(&mut ui, &ctx, &cx, vec![]);
        assert!(ui.quick.menu.is_none());
        let outs = ui.take_quick_out();
        assert!(outs.iter().any(|o| matches!(o, Out::Session(s) if s.ticket_id == story.id)), "Sitzungsanfrage erwartet");
        assert_eq!(count(), before);
        // Pflichtfeld: das Formular öffnet sich direkt (ohne Liste); Esc schließt das ganze Menü
        ui.open_quick_action(vec![t.id], start);
        frame(&mut ui, &ctx, &cx, vec![]);
        assert!(matches!(ui.quick.menu.as_ref().map(|m| (&m.stage, m.direct)), Some((Stage::Fill { .. }, true))));
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.todo));
        frame(&mut ui, &ctx, &cx, key(Key::Escape));
        assert!(ui.quick.menu.is_none(), "Esc schließt, statt die Liste zu zeigen");
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.todo));
        // mit Wert: verschoben, ein Rückgängig-Schritt
        ui.open_quick_action(vec![t.id], start);
        frame(&mut ui, &ctx, &cx, vec![]);
        let mut m = ui.quick.menu.take().unwrap();
        draft_of(&mut m, 0).text = "ready".into();
        ui.quick_submit(&cx, &mut m, chrono::Local::now().naive_local());
        assert!(m.ids.is_empty());
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.progress));
        let Some(Out::Toast { undo: Some(token), .. }) = ui.take_quick_out().into_iter().next() else { panic!("Toast erwartet") };
        assert!(ui.quick_undo(&db.conn, token, lang).is_ok());
        assert_eq!(get_ticket(&db.conn, t.id).unwrap().status_id, Some(f.todo));
        // Ticket anlegen ohne Titelvorgabe: erst die Titel-Eingabe; Esc schließt
        ui.open_quick_action(vec![story.id], ask);
        frame(&mut ui, &ctx, &cx, vec![]);
        assert!(matches!(ui.quick.menu.as_ref().map(|m| &m.stage), Some(Stage::Form { .. })));
        assert_eq!(count(), before);
        frame(&mut ui, &ctx, &cx, key(Key::Escape));
        assert!(ui.quick.menu.is_none());
        // ungültige Aktion und Aktion eines anderen Typs: nicht ausführbar, als Fehler-Toast gemeldet
        for id in [broken, start] {
            ui.open_quick_action(vec![story.id], id);
            frame(&mut ui, &ctx, &cx, vec![]);
            assert!(ui.quick.menu.is_none());
            let outs = ui.take_quick_out();
            assert!(matches!(outs.as_slice(), [Out::Toast { error: true, undo: None, .. }]));
            assert_eq!(count(), before);
        }
        // leere Liste öffnet nichts
        ui.open_quick_action(vec![], fixed);
        assert!(!ui.quick.is_open());
    }

    #[test]
    fn open_quick_action_runs_the_action_like_a_menu_row_in_both_languages() {
        direct_case(Lang::En);
        direct_case(Lang::De);
    }

    #[test]
    fn an_open_dropdown_keeps_esc_and_enter_for_itself() {
        let ctx = egui::Context::default();
        let input = |events: Vec<Event>| egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))), ..Default::default() };
        let (mut esc, mut enter) = (false, false);
        let _ = ctx.run(input([key(Key::Escape), key(Key::Enter)].concat()), |ctx| {
            let k = collect_keys(ctx, false, false);
            (esc, enter) = (k.esc, k.enter);
        });
        assert!(esc && enter, "ohne offene Liste gehören die Tasten dem Menü");
        let (mut esc, mut enter) = (true, true);
        let _ = ctx.run(input([key(Key::Escape), key(Key::Enter)].concat()), |ctx| {
            egui::Popup::open_id(ctx, Id::new("dropdown"));
            let k = collect_keys(ctx, false, false);
            (esc, enter) = (k.esc, k.enter);
        });
        assert!(!esc && !enter, "bei offener Liste bleiben Esc und Enter bei der Liste");
        // Strg+Enter bestätigt dennoch
        let ev = vec![Event::Key { key: Key::Enter, physical_key: Some(Key::Enter), pressed: true, repeat: false, modifiers: Modifiers::COMMAND }];
        let mut ctrl = false;
        let _ = ctx.run(input(ev), |ctx| {
            egui::Popup::open_id(ctx, Id::new("dropdown"));
            ctrl = collect_keys(ctx, false, false).ctrl_enter;
        });
        assert!(ctrl);
    }

    #[test]
    fn shortcut_needs_targets_and_no_open_menu() {
        let ctx = egui::Context::default();
        let mut ui = PlanUi::default();
        let ev = vec![Event::Key { key: Key::Period, physical_key: Some(Key::Period), pressed: true, repeat: false, modifiers: Modifiers::NONE }];
        let input = |events: Vec<Event>| egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))), ..Default::default() };
        let mut opened = false;
        let _ = ctx.run(input(ev.clone()), |ctx| opened = ui.quick_shortcut(ctx, &[]));
        assert!(!opened, "ohne Ticket nichts");
        let _ = ctx.run(input(ev.clone()), |ctx| opened = ui.quick_shortcut(ctx, &[QuickTarget { ticket_id: 4 }]));
        assert!(opened);
        assert!(ui.quick.is_open());
        let _ = ctx.run(input(ev), |ctx| opened = ui.quick_shortcut(ctx, &[QuickTarget { ticket_id: 4 }]));
        assert!(!opened, "schon offen");
    }
}
