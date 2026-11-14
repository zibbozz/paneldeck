//! Der Reiter „Tickets“ eines Projekts: Werkzeugleiste (Neu, Suche, Filter), Tabelle mit sortierbaren Spalten (virtualisiert,
//! auch für 10.000 Tickets flüssig), Datenladen, Tastenkürzel (Strg+N, Strg+Z, Strg+Y, Esc) und die gemeinsamen Hilfen
//! zum Auswerten von Änderungen der Datenschicht.

use super::helpers::color_of;
use super::ticket_data::{load_rows, Row};
use super::ticket_state::{sort_rows, Filters, SortCol, SortCtx, ViewMode};
use super::widgets;
use super::{Cx, Msg, PlanUi, Tab};
use crate::planning::fields::Field;
use crate::planning::history::Source;
use crate::planning::tickets::{set_flagged, set_starred};
use crate::planning::ops::TicketOp;
use crate::planning::projects::Project;
use crate::planning::statuses::StatusCategory;
use crate::planning::summary::ProjectSummary;
use crate::planning::ticket_error::{TRes, TicketError};
use crate::state::Lang;
use crate::style::{self, S1, S2, S3};
use eframe::egui::{self, Align, Color32, Key, Layout, Modifiers, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use rusqlite::Connection;
use std::time::Instant;

const ROW_H: f32 = 30.0;
const GOLD: Color32 = Color32::from_rgb(0xe5, 0xc0, 0x7b);
const FLAG_RED: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x75);

/// Alle Tickets des Projekts und davon erledigte (für die Übersicht).
pub fn project_counts(conn: &Connection, project_id: i64) -> (usize, usize) {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN s.category = 'done' THEN 1 ELSE 0 END), 0) FROM tickets t LEFT JOIN statuses s ON s.id = t.status_id WHERE t.project_id = ?1",
        [project_id],
        |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
    )
    .unwrap_or((0, 0))
}

/// Das Prioritäts-Feld des Typs (falls er eines hat).
fn priority_field<'a>(sum: &'a ProjectSummary, type_id: i64) -> Option<&'a Field> {
    sum.type_by_id(type_id)?.fields.iter().find(|f| super::ticket_data::is_priority(f))
}

/// Anzeigetext, Farbe und Rang der Priorität einer Zeile.
fn priority_of<'a>(sum: &'a ProjectSummary, row: &Row) -> Option<(usize, &'a crate::planning::fields::OptionValue)> {
    let id = row.prio?;
    let f = priority_field(sum, row.ticket.type_id)?;
    f.options.values.iter().enumerate().find(|(_, v)| v.id == id)
}

/// Spalten, die bei der gegebenen Breite Platz haben.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cols {
    pub status: bool,
    pub prio: bool,
    pub points: bool,
}

pub fn columns_for(width: f32) -> Cols {
    Cols { status: width >= 460.0, prio: width >= 640.0, points: width >= 720.0 }
}

impl PlanUi {
    // ------------------------------------------------------------------ Auswerten von Änderungen

    /// Fehlermeldung unter den Reitern (bis zur nächsten gelungenen Änderung).
    pub(super) fn msg_error(&mut self, _cx: &Cx, text: String) {
        self.msg = Some(Msg { text, ok: false, at: Instant::now() });
    }

    pub(super) fn t_fail(&mut self, cx: &Cx, e: &TicketError) {
        self.msg_error(cx, e.message(cx.lang));
    }

    /// Ergebnis einer Ticket-Änderung: Operation für Strg+Z merken oder Fehler melden. `true` = gelungen.
    pub(super) fn t_apply(&mut self, cx: &Cx, r: TRes<Option<TicketOp>>) -> bool {
        match r {
            Ok(op) => {
                self.tickets.commit(op);
                self.dirty = true;
                self.msg = None;
                true
            }
            Err(e) => {
                self.t_fail(cx, &e);
                false
            }
        }
    }

    // ------------------------------------------------------------------ Laden

    /// Pro Bild: Zustand zum gewählten Projekt abgleichen, Liste und geöffnetes Ticket bei Bedarf neu laden.
    pub(super) fn ticket_refresh(&mut self, cx: &Cx, undo_steps: usize) {
        self.tickets.undo_cap = undo_steps.max(1);
        self.tickets.history.set_capacity(undo_steps.max(1));
        if !self.tickets.mode_loaded {
            self.tickets.mode_loaded = true;
            self.tickets.mode = cx.db.meta("ticket_view_mode").map(|v| ViewMode::parse(&v)).unwrap_or(ViewMode::Page);
        }
        let Some(sum) = self.sum.clone() else { return };
        if sum.project.system {
            self.tickets.switch_project(sum.project.id);
            return;
        }
        // ein Ticket aus einem anderen Projekt ist geöffnet: der Projektwechsel gehört dazu
        self.tickets.switch_project(sum.project.id);
        if self.tickets.counts_dirty {
            self.tickets.counts_dirty = false;
            self.tickets.counts = project_counts(cx.conn, sum.project.id);
        }
        if self.tab == Tab::Tickets && self.tickets.list_dirty {
            self.tickets.list_dirty = false;
            let types: Vec<i64> = sum.types.iter().map(|t| t.ticket_type.id).collect();
            let statuses: Vec<i64> = sum.statuses.iter().map(|s| s.id).collect();
            self.tickets.filters.retain_known(&types, &statuses);
            match load_rows(cx.conn, sum.project.id, &self.tickets.filters) {
                Ok((rows, total)) => {
                    self.tickets.rows = rows;
                    self.tickets.project_total = total;
                    self.resort(&sum);
                }
                Err(e) => self.t_fail(cx, &e),
            }
        }
        if self.tickets.detail_dirty && self.tickets.open.is_some() {
            self.tickets.detail_dirty = false;
            let id = self.tickets.open.unwrap_or(0);
            match super::ticket_data::Detail::load(cx.conn, Some(&sum), id, cx.lang) {
                Ok(d) => {
                    d.merge_drafts(&mut self.tickets.view.drafts);
                    self.tickets.detail = Some(d);
                }
                Err(e) if e.is_not_found() => {
                    // das Ticket gibt es nicht mehr (gelöscht, Undo des Anlegens …)
                    self.tickets.back.retain(|b| *b != id);
                    self.tickets.go_back();
                }
                Err(e) => self.t_fail(cx, &e),
            }
        }
    }

    pub(super) fn resort(&mut self, sum: &ProjectSummary) {
        let type_pos = |id: i64| sum.types.iter().position(|t| t.ticket_type.id == id).unwrap_or(usize::MAX) as i64;
        let status_pos = |id: i64| sum.statuses.iter().position(|s| s.id == id).unwrap_or(usize::MAX) as i64;
        let prio = |r: &Row| priority_of(sum, r).map(|(i, _)| i);
        let cx = SortCtx { type_pos: &type_pos, status_pos: &status_pos, prio_rank: &prio };
        sort_rows(&mut self.tickets.rows, self.tickets.sort, &cx);
    }

    // ------------------------------------------------------------------ Tastenkürzel

    /// Strg+N (neues Ticket), Strg+Z / Strg+Y (rückgängig/wiederholen), Esc (zurück) im Ticketbereich.
    pub(super) fn ticket_keys(&mut self, ctx: &egui::Context, cx: &Cx) {
        let Some(sum) = self.sum.clone() else { return };
        if sum.project.system || self.tab != Tab::Tickets || self.tickets.any_modal() || self.wizard.is_some() || self.settings.is_some() || self.delete_project.is_some() || self.del.is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::N)) {
            self.open_create(cx, None);
            return;
        }
        // Solange ein Textfeld den Fokus hat, gehört Strg+Z dem Feld.
        if ctx.wants_keyboard_input() {
            return;
        }
        let (redo, undo, esc) = ctx.input_mut(|i| {
            let redo = i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::CTRL, Key::Y);
            let undo = !redo && i.consume_key(Modifiers::CTRL, Key::Z);
            (redo, undo, i.consume_key(Modifiers::NONE, Key::Escape))
        });
        if undo {
            self.ticket_undo(cx, false);
        } else if redo {
            self.ticket_undo(cx, true);
        } else if esc && self.tickets.open.is_some() {
            self.tickets.go_back();
        }
    }

    pub(super) fn ticket_undo(&mut self, cx: &Cx, redo: bool) {
        let lang = cx.lang;
        let label = if redo { self.tickets.history.next_redo() } else { self.tickets.history.next_undo() }.map(|op| super::ticket_state::op_label(op, lang));
        let r = if redo { self.tickets.history.redo(cx.conn) } else { self.tickets.history.undo(cx.conn) };
        match r {
            Ok(true) => {
                let l = label.unwrap_or_default();
                self.tickets.toast(match (lang, redo) {
                    (Lang::De, false) => format!("Rückgängig: {l}"),
                    (Lang::En, false) => format!("Undone: {l}"),
                    (Lang::De, true) => format!("Wiederholt: {l}"),
                    (Lang::En, true) => format!("Redone: {l}"),
                });
            }
            Ok(false) => {}
            Err(e) => {
                let what = if redo { lang.t("Wiederholen nicht möglich", "Cannot redo") } else { lang.t("Rückgängig nicht möglich", "Cannot undo") };
                self.msg_error(cx, format!("{what}: {e}"));
            }
        }
        self.tickets.invalidate();
        self.dirty = true;
    }

    // ------------------------------------------------------------------ Reiter „Tickets“

    pub(super) fn tickets_tab(&mut self, ui: &mut Ui, cx: &Cx, proj: &Project) {
        let lang = cx.lang;
        let Some(sum) = self.sum.clone() else { return };
        // Fehler (nicht Erfolg) über der Liste
        if let Some(m) = self.msg.as_ref().filter(|m| !m.ok) {
            let t = m.text.clone();
            ui.horizontal_wrapped(|ui| widgets::error(ui, &t));
            ui.add_space(S1);
        }
        let mut changed = false;
        // ---- Zeile 1: Neu, Suche, Stern/Markierung, Zähler
        ui.horizontal(|ui| {
            if widgets::primary(ui, &format!("+ {}", lang.t("Neues Ticket", "New ticket"))).on_hover_text(lang.t("Neues Ticket anlegen (Strg+N)", "Create a new ticket (Ctrl+N)")).clicked() {
                self.open_create(cx, None);
            }
            ui.add_space(S2);
            let id = egui::Id::new("ticket_search_box");
            let w = (ui.available_width() - 330.0).clamp(160.0, 420.0);
            let r = ui.add_sized([w, 28.0], egui::TextEdit::singleline(&mut self.tickets.filters.text).id(id).hint_text(lang.t("Suchen: Titel, Schlüssel, Text …", "Search: title, key, text …")).margin(egui::Margin::symmetric(8, 5)));
            if self.tickets.focus_search {
                r.request_focus();
                self.tickets.focus_search = false;
            }
            changed |= r.changed();
            if !self.tickets.filters.text.is_empty() && style::icon_btn_small(ui, style::Icon::Close, lang.t("Suche leeren", "Clear the search")).clicked() {
                self.tickets.filters.text.clear();
                changed = true;
            }
            ui.add_space(S2);
            let mut star = self.tickets.filters.starred;
            if mark_toggle(ui, "star", &mut star, GOLD, lang.t("Nur Favoriten (Stern)", "Starred only")) {
                self.tickets.filters.starred = star;
                changed = true;
            }
            let mut flag = self.tickets.filters.flagged;
            if mark_toggle(ui, "flag", &mut flag, FLAG_RED, lang.t("Nur markierte Tickets (Flag)", "Flagged only")) {
                self.tickets.filters.flagged = flag;
                changed = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n = self.tickets.rows.len();
                let total = self.tickets.project_total;
                let text = if self.tickets.filters.is_empty() {
                    match lang {
                        Lang::De => format!("{total} Ticket(s)"),
                        Lang::En => format!("{total} ticket(s)"),
                    }
                } else {
                    match lang {
                        Lang::De => format!("{n} von {total}"),
                        Lang::En => format!("{n} of {total}"),
                    }
                };
                ui.label(RichText::new(text).small().color(style::dim(ui)));
            });
        });
        ui.add_space(S2);
        // ---- Zeile 2: Typ- und Status-Chips
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            ui.label(RichText::new(lang.t("Typ", "Type")).small().color(style::dim(ui)));
            for t in sum.types.iter().filter(|t| !t.ticket_type.system) {
                let on = self.tickets.filters.types.contains(&t.ticket_type.id);
                if style::pill(ui, &t.ticket_type.display(lang), None, color_of(&t.ticket_type.color), on).clicked() {
                    self.tickets.filters.toggle_type(t.ticket_type.id);
                    changed = true;
                }
            }
            if !sum.statuses.is_empty() {
                ui.add_space(S3);
                ui.label(RichText::new(lang.t("Status", "Status")).small().color(style::dim(ui)));
                for s in &sum.statuses {
                    let on = self.tickets.filters.statuses.contains(&s.id);
                    if style::pill(ui, &s.display(lang), None, color_of(&s.color), on).clicked() {
                        self.tickets.filters.toggle_status(s.id);
                        changed = true;
                    }
                }
            }
            if !self.tickets.filters.is_empty() && ui.small_button(lang.t("Filter zurücksetzen", "Clear filters")).clicked() {
                self.tickets.filters = Filters::default();
                changed = true;
            }
        });
        ui.add_space(S2);
        if changed {
            self.tickets.list_dirty = true;
        }
        if self.tickets.rows.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                if self.tickets.project_total == 0 && self.tickets.filters.is_empty() {
                    ui.label(RichText::new(lang.t("Noch keine Tickets in diesem Projekt.", "No tickets in this project yet.")).heading().color(style::dim(ui)));
                    ui.add_space(S2);
                    ui.label(RichText::new(lang.t("Lege das erste mit „Neues Ticket“ an (Strg+N).", "Create the first one with “New ticket” (Ctrl+N).")).color(style::dim(ui)));
                } else {
                    ui.label(RichText::new(lang.t("Kein Ticket passt zu den Filtern.", "No ticket matches the filters.")).color(style::dim(ui)));
                }
            });
            let _ = proj;
            return;
        }
        self.ticket_table(ui, cx, &sum);
    }

    /// Tabelle mit Kopfzeile und virtualisierten Zeilen.
    fn ticket_table(&mut self, ui: &mut Ui, cx: &Cx, sum: &ProjectSummary) {
        let lang = cx.lang;
        let width = ui.available_width();
        let cols = columns_for(width);
        let w_badge = 30.0;
        let w_key = 84.0;
        let w_status = if cols.status { 132.0 } else { 0.0 };
        let w_prio = if cols.prio { 112.0 } else { 0.0 };
        let w_points = if cols.points { 54.0 } else { 0.0 };
        let w_marks = 56.0;
        let w_title = (width - 8.0 - w_badge - w_key - w_status - w_prio - w_points - w_marks - 8.0).max(80.0);
        // Kopfzeile
        let mut new_sort: Option<SortCol> = None;
        let (hrect, _) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::hover());
        ui.painter().rect_filled(hrect, 6.0, ui.visuals().faint_bg_color);
        let mut x = hrect.left() + 8.0;
        let sort = self.tickets.sort;
        let mut head = |ui: &mut Ui, label: &str, col: SortCol, w: f32, align_right: bool| {
            let r = Rect::from_min_size(Pos2::new(x, hrect.top()), Vec2::new(w, 26.0));
            let resp = ui.interact(r, egui::Id::new(("tcol", col as u8)), Sense::click());
            let active = sort.col == col;
            let arrow = if active { if sort.desc { " ▼" } else { " ▲" } } else { "" };
            let color = if active { ui.visuals().strong_text_color() } else { style::dim(ui) };
            let g = ui.painter().layout_no_wrap(format!("{label}{arrow}"), egui::FontId::proportional(12.0), color);
            let px = if align_right { r.right() - g.size().x - 4.0 } else { r.left() };
            ui.painter().galley(Pos2::new(px, r.center().y - g.size().y / 2.0), g, color);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                new_sort = Some(col);
            }
            resp.on_hover_text(lang.t("Nach dieser Spalte sortieren", "Sort by this column"));
            x += w;
        };
        head(ui, "", SortCol::Type, w_badge, false);
        head(ui, lang.t("Schlüssel", "Key"), SortCol::Key, w_key, false);
        head(ui, lang.t("Titel", "Title"), SortCol::Title, w_title, false);
        if cols.status {
            head(ui, lang.t("Status", "Status"), SortCol::Status, w_status, false);
        }
        if cols.prio {
            head(ui, lang.t("Priorität", "Priority"), SortCol::Priority, w_prio, false);
        }
        if cols.points {
            head(ui, lang.t("Pkt.", "Pts"), SortCol::Points, w_points, true);
        }
        head(ui, "★ ⚑", SortCol::Marks, w_marks, true);
        if let Some(c) = new_sort {
            self.tickets.sort = self.tickets.sort.toggled(c);
            self.resort(sum);
        }
        ui.add_space(2.0);
        // Zeilen
        let n = self.tickets.rows.len();
        let open = self.tickets.open;
        let mut click: Option<i64> = None;
        let mut mark: Option<(i64, bool, bool)> = None; // (Ticket, Stern?, neuer Wert)
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().id_salt("ticket_rows").auto_shrink([false, false]).show_rows(ui, ROW_H, n, |ui, range| {
            for i in range {
                let row = &self.tickets.rows[i];
                let t = &row.ticket;
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width - 6.0, ROW_H), Sense::click());
                let p = ui.painter();
                if open == Some(t.id) {
                    p.rect_filled(rect, 5.0, style::accent().gamma_multiply(0.22));
                } else if resp.hovered() {
                    p.rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.7));
                } else if i % 2 == 1 {
                    p.rect_filled(rect, 5.0, ui.visuals().faint_bg_color.gamma_multiply(0.5));
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let ty = sum.type_by_id(t.type_id);
                let done = t.status_id.and_then(|s| sum.status(s)).is_some_and(|s| s.category == StatusCategory::Done);
                let text_col = if done { style::dim(ui) } else { ui.visuals().text_color() };
                let mut x = rect.left() + 8.0;
                // Typ
                if let Some(ty) = ty {
                    widgets::paint_type_badge(p, Rect::from_min_size(Pos2::new(x, rect.center().y - 10.0), Vec2::splat(20.0)), ty.ticket_type.icon.as_deref(), color_of(&ty.ticket_type.color));
                }
                x += w_badge;
                let g = p.layout_no_wrap(t.key(), egui::FontId::monospace(12.5), style::dim(ui));
                p.galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, style::dim(ui));
                x += w_key;
                let mut job = egui::text::LayoutJob::simple_singleline(t.title.clone(), egui::TextStyle::Body.resolve(ui.style()), text_col);
                if done {
                    job.sections[0].format.strikethrough = Stroke::new(1.0_f32, text_col);
                }
                job.wrap.max_width = w_title - 10.0;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                job.wrap.overflow_character = Some('…');
                let g = p.layout_job(job);
                p.galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, text_col);
                x += w_title;
                if cols.status {
                    if let Some(s) = t.status_id.and_then(|s| sum.status(s)) {
                        let c = color_of(&s.color);
                        let g = p.layout_no_wrap(s.display(lang), egui::FontId::proportional(11.5), c);
                        let pw = (g.size().x + 22.0).min(w_status - 6.0);
                        let pr = Rect::from_min_size(Pos2::new(x, rect.center().y - 10.0), Vec2::new(pw, 20.0));
                        p.rect_filled(pr, 10.0, c.gamma_multiply(0.18));
                        p.circle_filled(Pos2::new(pr.left() + 10.0, pr.center().y), 3.2, c);
                        p.galley(Pos2::new(pr.left() + 18.0, pr.center().y - g.size().y / 2.0), g, c);
                    }
                    x += w_status;
                }
                if cols.prio {
                    if let Some((_, v)) = priority_of(sum, row) {
                        let c = v.color.as_deref().map(color_of).unwrap_or(Color32::GRAY);
                        p.circle_filled(Pos2::new(x + 6.0, rect.center().y), 4.5, c);
                        let g = p.layout_no_wrap(v.display(lang), egui::FontId::proportional(12.5), text_col);
                        p.galley(Pos2::new(x + 16.0, rect.center().y - g.size().y / 2.0), g, text_col);
                    }
                    x += w_prio;
                }
                if cols.points {
                    if let Some(pt) = row.points {
                        let txt = if pt.fract() == 0.0 { format!("{}", pt as i64) } else { format!("{pt:.1}") };
                        let g = p.layout_no_wrap(txt, egui::FontId::proportional(12.5), text_col);
                        p.galley(Pos2::new(x + w_points - g.size().x - 6.0, rect.center().y - g.size().y / 2.0), g, text_col);
                    }
                    x += w_points;
                }
                // Stern und Markierung (klickbar, vor dem Zeilenklick ausgewertet)
                let star_r = Rect::from_center_size(Pos2::new(x + 14.0, rect.center().y), Vec2::splat(22.0));
                let flag_r = Rect::from_center_size(Pos2::new(x + 40.0, rect.center().y), Vec2::splat(22.0));
                let mut hit_mark = false;
                for (r, is_star, on, col) in [(star_r, true, t.starred, GOLD), (flag_r, false, t.flagged, FLAG_RED)] {
                    let rs = ui.interact(r, egui::Id::new(("rowmark", t.id, is_star)), Sense::click());
                    let c = if on { col } else if rs.hovered() { col.gamma_multiply(0.7) } else { style::dim(ui).gamma_multiply(0.45) };
                    paint_mark(ui.painter(), if is_star { "star" } else { "flag" }, r.center(), on, c);
                    if rs.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if rs.clicked() {
                        mark = Some((t.id, is_star, !on));
                        hit_mark = true;
                    }
                    rs.on_hover_text(match (is_star, on) {
                        (true, false) => lang.t("Zu den Favoriten", "Add to favorites"),
                        (true, true) => lang.t("Aus den Favoriten entfernen", "Remove from favorites"),
                        (false, false) => lang.t("Markieren (Flag)", "Flag"),
                        (false, true) => lang.t("Markierung entfernen", "Remove the flag"),
                    });
                }
                if resp.clicked() && !hit_mark {
                    click = Some(t.id);
                }
                resp.on_hover_text(&t.title);
            }
        });
        if let Some(id) = click {
            self.tickets.open_ticket(id, false);
        }
        if let Some((id, is_star, on)) = mark {
            let r = if is_star { set_starred(cx.conn, id, on, &Source::ui()) } else { set_flagged(cx.conn, id, on, &Source::ui()) };
            self.t_apply(cx, r);
        }
    }
}

/// Umschalter „Stern“ / „Flag“ der Werkzeugleiste (aktiv = farbig); `true`, wenn geklickt wurde.
fn mark_toggle(ui: &mut Ui, icon: &str, on: &mut bool, col: Color32, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let hover = resp.hovered();
    if *on {
        ui.painter().rect_filled(rect, 6.0, col.gamma_multiply(0.22));
    } else if hover {
        ui.painter().rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
    }
    paint_mark(ui.painter(), icon, rect.center(), *on, if *on { col } else { style::dim(ui) });
    if hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = resp.on_hover_text(tip).clicked();
    if clicked {
        *on = !*on;
    }
    clicked
}

/// Stern oder Flag zeichnen; angeschaltet etwas kräftiger mit Füllpunkt.
pub fn paint_mark(p: &egui::Painter, icon: &str, c: Pos2, on: bool, col: Color32) {
    widgets::type_icon(p, icon, c, if on { 17.0 } else { 15.0 }, col);
    if on {
        widgets::type_icon(p, icon, c + Vec2::new(0.4, 0.0), 17.0, col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::transition::transition_ticket;

    #[test]
    fn narrow_lists_drop_columns_in_order() {
        assert_eq!(columns_for(900.0), Cols { status: true, prio: true, points: true });
        assert_eq!(columns_for(680.0), Cols { status: true, prio: true, points: false });
        assert_eq!(columns_for(500.0), Cols { status: true, prio: false, points: false });
        assert_eq!(columns_for(300.0), Cols { status: false, prio: false, points: false });
    }

    #[test]
    fn counts_include_done_tickets() {
        let f = Fx::new();
        assert_eq!(project_counts(&f.c, f.project), (0, 0));
        let a = f.mk(f.task, "A");
        f.mk(f.task, "B");
        transition_ticket(&f.c, a.id, f.done, &[], &Source::ui()).unwrap();
        assert_eq!(project_counts(&f.c, f.project), (2, 1));
        assert_eq!(project_counts(&f.c, 9999), (0, 0));
    }

    #[test]
    fn priority_resolves_through_the_type_field() {
        let f = Fx::new();
        let sum = crate::planning::summary::project_summary(&f.c, f.project).unwrap();
        let t = f.mk(f.task, "T");
        let medium = f.field(f.task, "priority").default_value.unwrap().as_i64().unwrap();
        let row = Row { ticket: t, prio: Some(medium), points: None };
        let (rank, v) = priority_of(&sum, &row).unwrap();
        assert_eq!(rank, 2, "Medium ist der dritte von fünf Werten");
        assert_eq!(v.display(Lang::De), "Normal");
        let none = Row { prio: None, ..row.clone() };
        assert!(priority_of(&sum, &none).is_none());
    }
}
