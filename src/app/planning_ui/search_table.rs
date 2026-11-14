//! Die Ergebnisliste der Abfragen: Tabelle mit sortierbaren Spalten, Seitenwahl, Fehleranzeige und der Baustein [`Results`], der
//! Anforderung, Laden und Zeichnen zusammenhält. Benutzt vom Reiter „Suche“ (`search_tab`) und von der Ticket-Liste mit Abfrage
//! (`ticket_query`).
//!
//! Gezeichnet wird mit eigenen Zeilen (`TicketRow` aus `planning::search`): Typ-Symbol, Schlüssel, Titel, Status, Priorität, Punkte,
//! Version/Meilenstein/Terminal (Chips wie in der lokalen Ticket-Liste, bei genug Breite und nur wenn eine Zeile einen Wert hat),
//! Projekt (nur bei „alle Projekte“), Stern und Markierung. Klicks kommen als [`ResultsOut`] zurück; geändert wird hier nichts.
//!
//! **Scrollen zum geöffneten Ticket:** `Results::scroll_to` (vom Aufrufer gesetzt) bringt die Zeile des Tickets ins Bild – aber nur, wenn
//! sie nicht ohnehin ganz zu sehen ist, genau ein Mal und nur, wenn das Ticket im aktuellen Ergebnis steht (sonst wird der Wunsch
//! verworfen). Mit eigener Rollfläche rechnet [`scroll_target`] (wie die lokale Liste), sonst rollt die umgebende Seite zur Zeile.

use super::helpers::color_of;
use super::ref_display::{paint_chips, row_chips, row_tooltip, scroll_target, RowRefs};
use super::search_logic::{self as logic, Col, SortBy, PAGE_SIZE};
use super::search_run::{QueryRun, Request};
use super::ticket_tab::{columns_for, paint_mark, FLAG_RED, GOLD, REFS_MIN_WIDTH, W_REFS};
use super::widgets;
use crate::db::Db;
use crate::planning::search::{Problem, Scope, SearchError, TicketRow};
use crate::planning::statuses::StatusCategory;
use crate::query::Schema;
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, Align, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use std::collections::HashMap;

pub const ROW_H: f32 = 30.0;
const AMBER: Color32 = Color32::from_rgb(0xe5, 0xc0, 0x7b);
/// Rot der Fehler (wie in der Abfrage-Leiste).
const RED: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x75);

// ------------------------------------------------------------------ Baustein

/// Was [`Results::draw`] von außen braucht.
pub struct ResultsCfg<'a> {
    pub lang: Lang,
    /// Eigene Rollfläche für die Zeilen mit dieser Höhe (virtualisiert); `None` = die Zeilen stehen in der Seite des Aufrufers.
    pub scroll_height: Option<f32>,
    pub salt: &'static str,
    /// Das geöffnete Ticket (Hervorhebung).
    pub selected: Option<i64>,
    /// Das Schema des Bereichs (für die Frage, welche Spalten sortierbar sind).
    pub schema: &'a Schema,
    /// Drei Klicks (aufsteigend, absteigend, ohne) statt zwei.
    pub three_state: bool,
    /// Text für den leeren Zustand.
    pub empty_text: &'static str,
}

/// Ergebnis eines Bildes.
#[derive(Default, Debug, PartialEq)]
pub struct ResultsOut {
    /// Zeile angeklickt (oder „Öffnen“ im Kontextmenü).
    pub open: Option<i64>,
    /// Stern/Markierung gesetzt: (Ticket, Stern?, neuer Wert).
    pub mark: Option<(i64, bool, bool)>,
}

/// Anforderung, Ergebnis, Sortierung und Seite einer Ergebnisliste.
pub struct Results {
    pub run: QueryRun,
    pub sort: Option<SortBy>,
    /// Seite (0-basiert).
    pub page: i64,
    /// Hat die ausgeführte Abfrage ein eigenes ORDER BY? (Dann sind die Spalten nicht sortierbar.)
    own_order: bool,
    /// Rechteck der ersten gezeichneten Zeile (für Tests, die einen Klick einspeisen)
    pub first_row: Option<Rect>,
    /// Diese Zeile soll sichtbar werden (geöffnetes oder gerade verlassenes Ticket). Der Aufrufer setzt es; es wird erledigt, sobald
    /// die Zeile zu sehen ist, das Ergebnis sie nicht enthält oder Seite/Sortierung/Abfrage wechseln.
    pub scroll_to: Option<i64>,
    /// Neue Scroll-Position der eigenen Rollfläche für das nächste Bild (aus `scroll_to` berechnet).
    pub scroll_offset: Option<f32>,
    /// Scroll-Position der eigenen Rollfläche im letzten Bild (für Tests).
    pub list_offset: f32,
}

impl Results {
    pub fn new(sort: Option<SortBy>) -> Results {
        Results { run: QueryRun::default(), sort, page: 0, own_order: false, first_row: None, scroll_to: None, scroll_offset: None, list_offset: 0.0 }
    }

    /// Pro Bild: Ergebnisse einsammeln und die Anforderung zur Abfrage `query` (ohne Sortierung) im Bereich abgleichen.
    /// `query = None` = nichts anzeigen.
    pub fn drive(&mut self, ctx: &egui::Context, db: &Db, scope: Scope, lang: Lang, schema: &Schema, query: Option<&str>) {
        let desired = query.map(|q| {
            self.own_order = logic::has_order(q);
            Request { scope, jql: logic::with_sort(q, schema, self.sort), lang, page: self.page.max(0) }
        });
        if let Some(fix) = self.run.drive(ctx, db, desired) {
            self.page = fix;
        }
    }

    /// Neue Abfrage: auf Seite 1, neu laden.
    pub fn restart(&mut self) {
        self.page = 0;
        self.scroll_to = None;
        self.scroll_offset = None;
        self.run.mark_stale();
    }

    /// Statuszeile, Fehler, Tabelle. `show_project`: Spalte „Projekt“ (bei „alle Projekte“).
    pub fn draw(&mut self, ui: &mut Ui, cfg: &ResultsCfg, show_project: bool) -> ResultsOut {
        let lang = cfg.lang;
        let mut out = ResultsOut::default();
        self.first_row = None;
        let loading = self.run.loading();
        let total = self.run.total();
        let mut new_page: Option<i64> = None;
        // ---- Statuszeile: Anzahl, Zeit, Seitenwahl, Ladezustand
        let error = self.run.error().cloned();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if loading {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new(lang.t("Suche läuft …", "Searching …")).color(style::dim(ui)));
            }
            if error.is_none() && self.run.shown.is_some() {
                ui.label(RichText::new(count_text(total, self.page, lang)).color(style::dim(ui)));
                if let Some(f) = &self.run.shown {
                    ui.label(RichText::new(took_text(f.took)).small().color(style::dim(ui)));
                }
                if logic::page_count(total) > 1 {
                    new_page = pager(ui, lang, total, self.page);
                }
            }
            if self.own_order && error.is_none() && self.run.shown.is_some() {
                ui.label(RichText::new(lang.t("Reihenfolge laut Abfrage", "Order from the query")).small().color(style::dim(ui)))
                    .on_hover_text(lang.t("Die Abfrage hat ein ORDER BY; die Spalten sind darum nicht sortierbar. Entferne es, um nach Spalten zu sortieren.", "The query has an ORDER BY, so the columns cannot be sorted. Remove it to sort by columns."));
            }
        });
        ui.add_space(S1);
        if let Some(p) = new_page {
            self.page = logic::clamp_page(total, p);
            // auf einer anderen Seite steht das Ticket nicht mehr
            self.scroll_to = None;
            self.scroll_offset = None;
        }
        // ---- Fehler statt Tabelle
        if let (Some(err), Some(f)) = (&error, &self.run.shown) {
            if !loading {
                self.scroll_to = None;
            }
            draw_error(ui, lang, err, &f.req.jql);
            return out;
        }
        if self.run.shown.is_none() {
            if !loading {
                self.scroll_to = None;
            }
            return out;
        }
        if self.run.rows().is_empty() {
            if !loading {
                self.scroll_to = None;
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(cfg.empty_text).color(style::dim(ui)));
                });
            }
            return out;
        }
        // ---- Tabelle
        let sortable: Vec<Col> = [Col::Type, Col::Key, Col::Title, Col::Status, Col::Priority, Col::Points, Col::Project, Col::Marks]
            .into_iter()
            .filter(|c| logic::sortable(cfg.schema, *c))
            .collect();
        // Gescrollt wird erst, wenn das Ergebnis da ist (beim Nachladen stehen noch die alten Zeilen).
        let scroll_to = if loading { None } else { self.scroll_to };
        let scroll_offset = self.scroll_offset.take();
        let tcfg = TableCfg {
            lang,
            show_project,
            sort: if self.own_order { None } else { self.sort },
            sort_locked: self.own_order,
            sortable: &sortable,
            selected: cfg.selected,
            scroll_height: cfg.scroll_height,
            salt: cfg.salt,
            refs: self.run.refs(),
            scroll_to,
            scroll_offset,
        };
        let mut t = TableOut::default();
        draw_table(ui, &tcfg, self.run.rows(), &mut t);
        self.list_offset = t.list_offset;
        if t.scroll_done {
            self.scroll_to = None;
        }
        if let Some(off) = t.scroll_offset {
            self.scroll_offset = Some(off);
            ui.ctx().request_repaint();
        }
        if let Some(c) = t.sort_click {
            self.sort = if cfg.three_state { SortBy::next(self.sort, c) } else { Some(self.sort.unwrap_or(SortBy::NEWEST).toggled(c)) };
            self.page = 0;
            self.scroll_to = None;
            self.scroll_offset = None;
        }
        if let Some(k) = t.copy_key {
            ui.ctx().copy_text(k);
        }
        self.first_row = t.first_row;
        out.open = t.open;
        out.mark = t.mark;
        out
    }
}

fn count_text(total: i64, page: i64, lang: Lang) -> String {
    let (a, b) = logic::page_span(total, page);
    match (lang, total) {
        (Lang::De, 0) => "Keine Treffer".to_string(),
        (Lang::En, 0) => "No results".to_string(),
        (Lang::De, 1) => "1 Ticket".to_string(),
        (Lang::En, 1) => "1 ticket".to_string(),
        (Lang::De, _) if total > PAGE_SIZE => format!("{a}–{b} von {total} Tickets"),
        (Lang::En, _) if total > PAGE_SIZE => format!("{a}–{b} of {total} tickets"),
        (Lang::De, _) => format!("{total} Tickets"),
        (Lang::En, _) => format!("{total} tickets"),
    }
}

fn took_text(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 { format!("{ms} ms") } else { format!("{:.1} s", d.as_secs_f32()) }
}

/// Seitenwahl; gibt die gewählte Seite zurück.
fn pager(ui: &mut Ui, lang: Lang, total: i64, page: i64) -> Option<i64> {
    let pages = logic::page_count(total);
    let page = logic::clamp_page(total, page);
    let mut pick = None;
    if ui.add_enabled(page > 0, egui::Button::new("‹").small()).on_hover_text(lang.t("Vorherige Seite", "Previous page")).clicked() {
        pick = Some(page - 1);
    }
    ui.label(RichText::new(match lang {
        Lang::De => format!("Seite {} von {pages}", page + 1),
        Lang::En => format!("Page {} of {pages}", page + 1),
    }));
    if ui.add_enabled(page + 1 < pages, egui::Button::new("›").small()).on_hover_text(lang.t("Nächste Seite", "Next page")).clicked() {
        pick = Some(page + 1);
    }
    pick
}

// ------------------------------------------------------------------ Fehler

/// Fehler einer Ausführung unter der Leiste: die Meldungen mit Position, darunter die Abfrage mit hervorgehobener Stelle.
/// Das Zeitlimit hat einen eigenen Hinweis.
pub fn draw_error(ui: &mut Ui, lang: Lang, err: &SearchError, query: &str) {
    let problems: Vec<Problem> = err.problems(lang);
    let timeout = matches!(err, SearchError::Timeout);
    let color = if timeout { AMBER } else { RED };
    ui.add_space(S1);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(if timeout { "⏱" } else { "▲" }).color(color));
        if problems.is_empty() {
            ui.label(RichText::new(err.message(lang)).color(color));
        } else {
            ui.label(RichText::new(lang.t("Die Abfrage konnte nicht ausgeführt werden:", "The query could not be run:")).color(color));
        }
    });
    for p in &problems {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(18.0);
            ui.label(RichText::new(format!("{} {}: {}", lang.t("Position", "Position"), p.column, p.message)).color(color));
        });
    }
    if !problems.is_empty() && !query.is_empty() {
        ui.add_space(S2);
        let job = logic::highlight_job(query, &problems, ui.visuals().text_color(), color, egui::TextStyle::Monospace.resolve(ui.style()));
        egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.add(egui::Label::new(job).wrap());
        });
    }
    if timeout {
        ui.add_space(S1);
        ui.label(RichText::new(lang.t("Tipp: Schränke die Abfrage ein (Projekt, Typ, Status) oder lass die Datenbank kurz zur Ruhe kommen.", "Tip: narrow the query (project, type, status) or give the database a moment.")).small().color(style::dim(ui)));
    }
}

// ------------------------------------------------------------------ Tabelle

pub struct TableCfg<'a> {
    pub lang: Lang,
    pub show_project: bool,
    pub sort: Option<SortBy>,
    /// Die Abfrage hat ein eigenes ORDER BY: Köpfe sind nicht klickbar.
    pub sort_locked: bool,
    pub sortable: &'a [Col],
    pub selected: Option<i64>,
    pub scroll_height: Option<f32>,
    pub salt: &'static str,
    /// Version, Meilenstein und Terminal je Ticket (mit dem Ergebnis geladen); `None` = keine Spalte.
    pub refs: Option<&'a HashMap<i64, RowRefs>>,
    /// Ticket, dessen Zeile sichtbar werden soll (nur wenn nicht ohnehin zu sehen).
    pub scroll_to: Option<i64>,
    /// Scroll-Position, die in diesem Bild einmalig gesetzt wird (Ergebnis der Rechnung des vorigen Bildes).
    pub scroll_offset: Option<f32>,
}

#[derive(Default, Debug)]
pub struct TableOut {
    pub sort_click: Option<Col>,
    pub open: Option<i64>,
    pub mark: Option<(i64, bool, bool)>,
    pub copy_key: Option<String>,
    pub first_row: Option<Rect>,
    /// Der Wunsch `scroll_to` ist erledigt (Zeile zu sehen, gescrollt oder nicht im Ergebnis).
    pub scroll_done: bool,
    /// Neue Scroll-Position für das nächste Bild (eigene Rollfläche).
    pub scroll_offset: Option<f32>,
    /// Scroll-Position der eigenen Rollfläche in diesem Bild (für Tests).
    pub list_offset: f32,
}

struct Widths {
    badge: f32,
    key: f32,
    title: f32,
    status: f32,
    prio: f32,
    points: f32,
    refs: f32,
    project: f32,
    marks: f32,
}

fn widths(width: f32, show_project: bool, show_refs: bool) -> Widths {
    let c = columns_for(width);
    let badge = 30.0;
    let key = 96.0;
    let status = if c.status { 132.0 } else { 0.0 };
    let prio = if c.prio { 112.0 } else { 0.0 };
    let points = if c.points { 54.0 } else { 0.0 };
    let refs = if show_refs { W_REFS } else { 0.0 };
    let project = if show_project && width >= 560.0 { 72.0 } else { 0.0 };
    let marks = 56.0;
    let title = (width - 8.0 - badge - key - status - prio - points - refs - project - marks - 8.0).max(80.0);
    Widths { badge, key, title, status, prio, points, refs, project, marks }
}

/// Die Tabelle: Kopf mit sortierbaren Spalten, darunter die Zeilen (mit eigener Rollfläche, wenn `scroll_height` gesetzt ist).
pub fn draw_table(ui: &mut Ui, cfg: &TableCfg, rows: &[TicketRow], out: &mut TableOut) {
    let lang = cfg.lang;
    let width = ui.available_width();
    // Die Spalte erscheint (wie in der lokalen Liste) ab einer Mindestbreite und nur, wenn eine Zeile einen Wert hat.
    let show_refs = width >= REFS_MIN_WIDTH && cfg.refs.is_some_and(|m| !m.is_empty());
    let w = widths(width, cfg.show_project, show_refs);
    // ---- Kopf
    let (hrect, _) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::hover());
    ui.painter().rect_filled(hrect, 6.0, ui.visuals().faint_bg_color);
    let mut x = hrect.left() + 8.0;
    let mut head = |ui: &mut Ui, label: &str, col: Option<Col>, cw: f32, right: bool| {
        if cw <= 0.0 {
            return;
        }
        let r = Rect::from_min_size(Pos2::new(x, hrect.top()), Vec2::new(cw, 26.0));
        let can = col.is_some_and(|c| !cfg.sort_locked && cfg.sortable.contains(&c));
        let resp = ui.interact(r, egui::Id::new((cfg.salt, "col", col)), if can { Sense::click() } else { Sense::hover() });
        let active = col.is_some() && cfg.sort.is_some_and(|s| Some(s.col) == col);
        let arrow = match cfg.sort {
            Some(s) if Some(s.col) == col => {
                if s.desc { " ▼" } else { " ▲" }
            }
            _ => "",
        };
        let color = if active { ui.visuals().strong_text_color() } else { style::dim(ui) };
        let g = ui.painter().layout_no_wrap(format!("{label}{arrow}"), FontId::proportional(12.0), color);
        let px = if right { r.right() - g.size().x - 4.0 } else { r.left() };
        ui.painter().galley(Pos2::new(px, r.center().y - g.size().y / 2.0), g, color);
        if can && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if can && resp.clicked() {
            out.sort_click = col;
        }
        let tip = if col.is_none() {
            ""
        } else if cfg.sort_locked {
            lang.t("Die Abfrage hat ein ORDER BY – Reihenfolge laut Abfrage", "The query has an ORDER BY – order from the query")
        } else if can {
            lang.t("Nach dieser Spalte sortieren", "Sort by this column")
        } else {
            lang.t("Nach dieser Spalte lässt sich in diesem Bereich nicht sortieren", "This column cannot be sorted in this scope")
        };
        if !tip.is_empty() {
            resp.on_hover_text(tip);
        }
        x += cw;
    };
    head(ui, "", Some(Col::Type), w.badge, false);
    head(ui, lang.t("Schlüssel", "Key"), Some(Col::Key), w.key, false);
    head(ui, lang.t("Titel", "Title"), Some(Col::Title), w.title, false);
    head(ui, lang.t("Status", "Status"), Some(Col::Status), w.status, false);
    head(ui, lang.t("Priorität", "Priority"), Some(Col::Priority), w.prio, false);
    head(ui, lang.t("Pkt.", "Pts"), Some(Col::Points), w.points, true);
    head(ui, lang.t("Version · Meilenstein · Terminal", "Version · Milestone · Terminal"), None, w.refs, false);
    head(ui, lang.t("Projekt", "Project"), Some(Col::Project), w.project, false);
    head(ui, "★ ⚑", Some(Col::Marks), w.marks, true);
    ui.add_space(2.0);
    // ---- Zeilen
    ui.spacing_mut().item_spacing.y = 0.0;
    let row_w = width - 6.0;
    match cfg.scroll_height {
        Some(h) => {
            let mut area = egui::ScrollArea::vertical().id_salt((cfg.salt, "rows")).auto_shrink([false, false]).max_height(h.max(ROW_H * 2.0));
            // die berechnete Position genau ein Mal anwenden
            let applied = match cfg.scroll_offset {
                Some(off) => {
                    area = area.vertical_scroll_offset(off);
                    true
                }
                None => false,
            };
            let res = area.show_rows(ui, ROW_H, rows.len(), |ui, range| {
                for i in range {
                    draw_row(ui, cfg, &w, &rows[i], i, row_w, out);
                }
            });
            out.list_offset = res.state.offset.y;
            if let Some(id) = cfg.scroll_to {
                if applied {
                    out.scroll_done = true;
                } else if let Some(i) = rows.iter().position(|r| r.id == id) {
                    match scroll_target(i, ROW_H, res.state.offset.y, res.inner_rect.height(), res.content_size.y) {
                        None => out.scroll_done = true,
                        Some(off) => out.scroll_offset = Some(off),
                    }
                } else {
                    // das Ticket steht nicht im Ergebnis: nichts zu tun
                    out.scroll_done = true;
                }
            }
        }
        None => {
            for (i, row) in rows.iter().enumerate() {
                draw_row(ui, cfg, &w, row, i, row_w, out);
            }
            // ohne eigene Rollfläche: ist die gewünschte Zeile nicht im Ergebnis, ist der Wunsch erledigt
            if cfg.scroll_to.is_some_and(|id| !rows.iter().any(|r| r.id == id)) {
                out.scroll_done = true;
            }
        }
    }
}

fn draw_row(ui: &mut Ui, cfg: &TableCfg, w: &Widths, row: &TicketRow, i: usize, row_w: f32, out: &mut TableOut) {
    let lang = cfg.lang;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
    if i == 0 {
        out.first_row = Some(rect);
    }
    // Ohne eigene Rollfläche liegen alle Zeilen in der Seite des Aufrufers: die gewünschte Zeile in die Mitte rollen, wenn sie nicht
    // ganz zu sehen ist (einmal).
    if cfg.scroll_height.is_none() && cfg.scroll_to == Some(row.id) {
        if !ui.clip_rect().contains_rect(rect) {
            ui.scroll_to_rect(rect, Some(Align::Center));
        }
        out.scroll_done = true;
    }
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    if cfg.selected == Some(row.id) {
        p.rect_filled(rect, 5.0, style::accent().gamma_multiply(0.22));
    } else if resp.hovered() {
        p.rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.7));
    } else if i % 2 == 1 {
        p.rect_filled(rect, 5.0, ui.visuals().faint_bg_color.gamma_multiply(0.5));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let dim = style::dim(ui);
    let done = row.status_category == Some(StatusCategory::Done);
    let text_col = if done { dim } else { ui.visuals().text_color() };
    let cy = rect.center().y;
    let mut x = rect.left() + 8.0;
    widgets::paint_type_badge(p, Rect::from_min_size(Pos2::new(x, cy - 10.0), Vec2::splat(20.0)), row.type_icon.as_deref(), color_of(&row.type_color));
    x += w.badge;
    let g = p.layout_no_wrap(row.key.clone(), FontId::monospace(12.5), dim);
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, dim);
    x += w.key;
    let mut job = egui::text::LayoutJob::simple_singleline(row.title.clone(), egui::TextStyle::Body.resolve(ui.style()), text_col);
    if done {
        job.sections[0].format.strikethrough = Stroke::new(1.0_f32, text_col);
    }
    job.wrap.max_width = w.title - 10.0;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = p.layout_job(job);
    p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, text_col);
    x += w.title;
    if w.status > 0.0 {
        if let Some(name) = &row.status_name {
            let c = row.status_color.as_deref().map(color_of).unwrap_or(dim);
            let g = p.layout_no_wrap(name.clone(), FontId::proportional(11.5), c);
            let pw = (g.size().x + 22.0).min(w.status - 6.0);
            let pr = Rect::from_min_size(Pos2::new(x, cy - 10.0), Vec2::new(pw, 20.0));
            p.rect_filled(pr, 10.0, c.gamma_multiply(0.18));
            p.circle_filled(Pos2::new(pr.left() + 10.0, cy), 3.2, c);
            p.galley(Pos2::new(pr.left() + 18.0, cy - g.size().y / 2.0), g, c);
        }
        x += w.status;
    }
    if w.prio > 0.0 {
        if let Some(name) = &row.priority {
            let c = row.priority_color.as_deref().map(color_of).unwrap_or(Color32::GRAY);
            p.circle_filled(Pos2::new(x + 6.0, cy), 4.5, c);
            let g = p.layout_no_wrap(name.clone(), FontId::proportional(12.5), text_col);
            p.galley(Pos2::new(x + 16.0, cy - g.size().y / 2.0), g, text_col);
        }
        x += w.prio;
    }
    if w.points > 0.0 {
        if let Some(pt) = row.story_points {
            let txt = if pt.fract() == 0.0 { format!("{}", pt as i64) } else { format!("{pt:.1}") };
            let g = p.layout_no_wrap(txt, FontId::proportional(12.5), text_col);
            p.galley(Pos2::new(x + w.points - g.size().x - 6.0, cy - g.size().y / 2.0), g, text_col);
        }
        x += w.points;
    }
    if w.refs > 0.0 {
        if let Some(r) = cfg.refs.and_then(|m| m.get(&row.id)) {
            paint_chips(ui, ui.painter(), Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w.refs - 8.0, ROW_H)), &row_chips(r, lang));
        }
        x += w.refs;
    }
    if w.project > 0.0 {
        let g = p.layout_no_wrap(row.project_key.clone(), FontId::monospace(12.0), dim);
        p.galley(Pos2::new(x, cy - g.size().y / 2.0), g, dim);
        x += w.project;
    }
    // Stern und Markierung (klickbar, vor dem Zeilenklick ausgewertet)
    let star_r = Rect::from_center_size(Pos2::new(x + 14.0, cy), Vec2::splat(22.0));
    let flag_r = Rect::from_center_size(Pos2::new(x + 40.0, cy), Vec2::splat(22.0));
    let mut hit_mark = false;
    for (r, is_star, on, col) in [(star_r, true, row.starred, GOLD), (flag_r, false, row.flagged, FLAG_RED)] {
        let rs = ui.interact(r, egui::Id::new((cfg.salt, "mark", row.id, is_star)), Sense::click());
        let c = if on { col } else if rs.hovered() { col.gamma_multiply(0.7) } else { dim.gamma_multiply(0.45) };
        paint_mark(ui.painter(), if is_star { "star" } else { "flag" }, r.center(), on, c);
        if rs.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if rs.clicked() {
            out.mark = Some((row.id, is_star, !on));
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
        out.open = Some(row.id);
    }
    // Kontextmenü
    let tip = match cfg.refs.and_then(|m| m.get(&row.id)) {
        Some(r) => format!("{}\n{}", row.title, row_tooltip(r, lang)),
        None => row.title.clone(),
    };
    let resp = resp.on_hover_text(tip);
    resp.context_menu(|ui| {
        if ui.button(lang.t("Öffnen", "Open")).clicked() {
            out.open = Some(row.id);
            ui.close();
        }
        if ui.button(if row.starred { lang.t("Aus den Favoriten entfernen", "Remove from favorites") } else { lang.t("Zu den Favoriten (Stern)", "Add to favorites (star)") }).clicked() {
            out.mark = Some((row.id, true, !row.starred));
            ui.close();
        }
        if ui.button(if row.flagged { lang.t("Markierung entfernen", "Remove the flag") } else { lang.t("Markieren (Flag)", "Flag") }).clicked() {
            out.mark = Some((row.id, false, !row.flagged));
            ui.close();
        }
        ui.separator();
        if ui.button(lang.t("Schlüssel kopieren", "Copy key")).clicked() {
            out.copy_key = Some(row.key.clone());
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_text_is_translated_and_shows_the_page_range() {
        assert_eq!(count_text(0, 0, Lang::En), "No results");
        assert_eq!(count_text(0, 0, Lang::De), "Keine Treffer");
        assert_eq!(count_text(1, 0, Lang::En), "1 ticket");
        assert_eq!(count_text(42, 0, Lang::De), "42 Tickets");
        assert_eq!(count_text(250, 1, Lang::En), "101–200 of 250 tickets");
        assert_eq!(count_text(250, 2, Lang::De), "201–250 von 250 Tickets");
    }

    #[test]
    fn took_is_shown_in_ms_or_seconds() {
        assert_eq!(took_text(std::time::Duration::from_millis(12)), "12 ms");
        assert_eq!(took_text(std::time::Duration::from_millis(1500)), "1.5 s");
    }

    #[test]
    fn columns_drop_with_the_width_and_project_needs_room() {
        let wide = widths(900.0, true, false);
        assert!(wide.status > 0.0 && wide.prio > 0.0 && wide.points > 0.0 && wide.project > 0.0);
        assert_eq!(wide.refs, 0.0, "ohne Werte keine Spalte");
        let narrow = widths(300.0, true, false);
        assert_eq!((narrow.status, narrow.prio, narrow.points, narrow.project), (0.0, 0.0, 0.0, 0.0));
        assert!(narrow.title >= 80.0);
        assert_eq!(widths(900.0, false, false).project, 0.0, "ohne „alle Projekte“ keine Projekt-Spalte");
        // die Spalten füllen die Breite (Titel nimmt den Rest)
        for show_refs in [false, true] {
            let w = widths(1000.0, true, show_refs);
            let sum = 8.0 + w.badge + w.key + w.title + w.status + w.prio + w.points + w.refs + w.project + w.marks + 8.0;
            assert!((sum - 1000.0).abs() < 0.01, "{sum}");
            assert_eq!(w.refs > 0.0, show_refs);
        }
    }
}
