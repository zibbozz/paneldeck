//! Oberfläche des Kalenders (#164, 3.2.0): Monats-, Wochen-, Tagesansicht und Liste, rechts der Detailbereich, Dialog
//! für eigene Termine (mit Baukasten für Wiederholungen), Optionen der Quellen, iCal-Import/-Export.
//!
//! Einstieg für den Zeit-Reiter: [`show`] (zeichnet den Kalender in `ui`). Die Logik liegt in `crate::calendar`; der
//! Zustand in [`CalUi`] arbeitet ohne `App` gegen [`CalEnv`] (Datenbank, Sprache, Uhr, Zeitzone) und lässt sich so ohne
//! Fenster testen (`tests.rs`).
//!
//! * `views`: die vier Ansichten (gemalt, Einträge erst im sichtbaren Zeitraum ausgerechnet).
//! * `dialog`: Termin anlegen/bearbeiten.

pub(crate) mod dialog;
mod views;

/// Formular für Tests anderer Module (Grenzwerte im Dialog).
#[cfg(test)]
pub(crate) fn test_form(t: NaiveDateTime) -> dialog::EventForm {
    dialog::EventForm::new_at(t, false)
}

#[cfg(test)]
mod tests;

use crate::calendar::builder;
use crate::calendar::cron::Cron;
use crate::calendar::ics;
use crate::calendar::sources::{self, Item, Kind};
use crate::calendar::store::{self, Options, Sources, ViewKind};
use crate::calendar::zone::{SystemZone, Zone};
use crate::calendar::{self as cal, Event};
use crate::state::Lang;
use crate::style;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};
use eframe::egui::{self, Rect, RichText, Ui, UiBuilder, Vec2};
use rusqlite::Connection;
use std::time::Instant;

/// Breite des Detailbereichs rechts.
const DETAIL_W: f32 = 300.0;
/// Wie lange die geladenen Einträge gelten, bevor sie neu gelesen werden (Änderungen in der Planung).
const CACHE_SECS: f32 = 5.0;

/// Kalender in `ui` zeichnen – der Einstieg für den Zeit-Reiter (Unterreiter „Kalender“).
pub(crate) fn show(app: &mut super::App, ui: &mut Ui) {
    let zone = SystemZone;
    let env = CalEnv { conn: app.db.as_ref().map(|d| &d.conn), lang: app.settings.language, now: chrono::Local::now().naive_local(), zone: &zone };
    app.cal.ui(ui, &env);
    if app.cal.take_changed() {
        app.wins.data_gen += 1;
    }
}

/// Was der Kalender von außen braucht.
pub struct CalEnv<'a> {
    pub conn: Option<&'a Connection>,
    pub lang: Lang,
    /// Lokale Zeit „jetzt“.
    pub now: NaiveDateTime,
    pub zone: &'a dyn Zone,
}

/// Ziel eines Klicks (für Tests: wo was gezeichnet wurde).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Hit {
    Item(Kind, i64, NaiveDateTime),
    /// Zelle eines Tages (Monat) bzw. Spalte eines Tages (Woche/Tag, Rechteck = 0–24 Uhr).
    Day(NaiveDate),
    More(NaiveDate),
    Button(&'static str),
}

/// Auswahl: Art, Kennung, Beginn des Vorkommens.
pub(crate) type SelKey = (Kind, i64, NaiveDateTime);

struct Cache {
    from: NaiveDate,
    to: NaiveDate,
    generation: u64,
    at: Instant,
    items: std::rc::Rc<Vec<Item>>,
}

/// Zustand der Kalender-Oberfläche.
#[derive(Default)]
pub struct CalUi {
    pub(crate) view: ViewKind,
    /// Tag, um den es geht (Monat/Woche/Tag/Liste ab hier); `None` = heute.
    pub(crate) anchor: Option<NaiveDate>,
    pub(crate) selected: Option<SelKey>,
    pub(crate) form: Option<dialog::EventForm>,
    pub(crate) options_open: bool,
    opts: Option<Options>,
    cache: Option<Cache>,
    generation: u64,
    /// Meldung in der Werkzeugleiste (Text, Fehler?).
    pub(crate) msg: Option<(String, bool)>,
    pub(crate) report: Option<ics::ImportReport>,
    pub(crate) confirm_delete: Option<i64>,
    /// Gezeichnete Ziele des letzten Bildes.
    pub(crate) hits: Vec<(Hit, Rect)>,
    /// Wochen-/Tagesansicht schon auf den Morgen gerollt (für diese Ansicht und Woche)?
    scrolled: Option<(ViewKind, NaiveDate)>,
    /// Daten geändert (andere Fenster neu laden).
    changed: bool,
}

impl CalUi {
    /// Neu laden (nach Änderungen von außen, z. B. MCP).
    pub fn invalidate(&mut self) {
        self.generation += 1;
    }

    fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn touched(&mut self) {
        self.generation += 1;
        self.changed = true;
    }

    pub(crate) fn anchor(&self, env: &CalEnv) -> NaiveDate {
        self.anchor.filter(|d| cal::year_ok(*d)).unwrap_or(env.now.date())
    }

    fn options(&mut self, conn: &Connection) -> &mut Options {
        if self.opts.is_none() {
            let o = store::load_options(conn);
            self.view = o.view;
            self.opts = Some(o);
        }
        self.opts.as_mut().unwrap()
    }

    /// Sichtbarer Zeitraum `[from, to)` der Ansicht.
    pub(crate) fn range(&self, env: &CalEnv) -> (NaiveDate, NaiveDate) {
        let a = self.anchor(env);
        match self.view {
            ViewKind::Month => {
                let first = NaiveDate::from_ymd_opt(a.year(), a.month(), 1).unwrap_or(a);
                let s = cal::week_start(first);
                (s, cal::add_days(s, 42))
            }
            ViewKind::Week => {
                let s = cal::week_start(a);
                (s, cal::add_days(s, 7))
            }
            ViewKind::Day => (a, cal::add_days(a, 1)),
            ViewKind::List => (a, cal::add_days(a, 60)),
        }
    }

    /// Einträge des sichtbaren Zeitraums (zwischengespeichert, geteilt statt je Bild kopiert).
    pub(crate) fn items(&mut self, env: &CalEnv) -> std::rc::Rc<Vec<Item>> {
        let Some(conn) = env.conn else { return Default::default() };
        let (from, to) = self.range(env);
        let fresh = self.cache.as_ref().is_some_and(|c| c.from == from && c.to == to && c.generation == self.generation && c.at.elapsed().as_secs_f32() < CACHE_SECS);
        if !fresh {
            let opts = self.options(conn).clone();
            let items = sources::collect(conn, &opts, from, to, env.zone);
            self.cache = Some(Cache { from, to, generation: self.generation, at: Instant::now(), items: std::rc::Rc::new(items) });
        }
        self.cache.as_ref().map(|c| std::rc::Rc::clone(&c.items)).unwrap_or_default()
    }

    fn step(&mut self, env: &CalEnv, dir: i64) {
        let a = self.anchor(env);
        let next = match self.view {
            ViewKind::Month => {
                let idx = a.year() as i64 * 12 + a.month0() as i64 + dir;
                NaiveDate::from_ymd_opt(idx.div_euclid(12) as i32, idx.rem_euclid(12) as u32 + 1, 1).unwrap_or(a)
            }
            ViewKind::Week => cal::add_days(a, 7 * dir),
            ViewKind::Day => cal::add_days(a, dir),
            ViewKind::List => cal::add_days(a, 30 * dir),
        };
        // nie aus dem erlaubten Bereich blättern
        if cal::year_ok(next) {
            self.anchor = Some(next);
        }
    }

    pub(crate) fn set_view(&mut self, env: &CalEnv, v: ViewKind) {
        self.view = v;
        if let Some(conn) = env.conn {
            let o = self.options(conn);
            o.view = v;
            let o = o.clone();
            let _ = store::save_options(conn, &o);
        }
    }

    /// Den Kalender zeichnen.
    pub fn ui(&mut self, ui: &mut Ui, env: &CalEnv) {
        self.hits.clear();
        let lang = env.lang;
        let Some(conn) = env.conn else {
            ui.label(RichText::new(lang.t("Die Datenbank ist nicht verfügbar – der Kalender braucht sie.", "The database is not available – the calendar needs it.")).color(style::danger_soft()));
            return;
        };
        let _ = self.options(conn);
        self.toolbar(ui, env);
        ui.add_space(style::S1);
        let items = self.items(env);
        let full = ui.available_rect_before_wrap();
        let detail = (self.view == ViewKind::List || self.selected.is_some()) && full.width() > 640.0;
        let (main, side) = if detail {
            let split = full.right() - DETAIL_W;
            (Rect::from_min_max(full.min, egui::pos2(split - style::S2, full.max.y)), Some(Rect::from_min_max(egui::pos2(split, full.min.y), full.max)))
        } else {
            (full, None)
        };
        ui.scope_builder(UiBuilder::new().max_rect(main), |ui| match self.view {
            ViewKind::Month => self.month(ui, env, &items),
            ViewKind::Week => self.week(ui, env, &items, 7),
            ViewKind::Day => self.week(ui, env, &items, 1),
            ViewKind::List => self.list(ui, env, &items),
        });
        if let Some(side) = side {
            ui.scope_builder(UiBuilder::new().max_rect(side), |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(side.size() - Vec2::splat(14.0));
                    self.detail(ui, env, &items);
                });
            });
        }
        ui.allocate_rect(full, egui::Sense::hover());
        self.dialogs(ui.ctx(), env);
    }

    fn toolbar(&mut self, ui: &mut Ui, env: &CalEnv) {
        let lang = env.lang;
        ui.horizontal(|ui| {
            let r = ui.button(lang.t("Heute", "Today"));
            self.hits.push((Hit::Button("today"), r.rect));
            if r.clicked() {
                self.anchor = None;
            }
            let r = ui.button("◀").on_hover_text(lang.t("Zurück", "Previous"));
            self.hits.push((Hit::Button("prev"), r.rect));
            if r.clicked() {
                self.step(env, -1);
            }
            let r = ui.button("▶").on_hover_text(lang.t("Weiter", "Next"));
            self.hits.push((Hit::Button("next"), r.rect));
            if r.clicked() {
                self.step(env, 1);
            }
            ui.add_space(style::S2);
            ui.label(RichText::new(self.title(env)).strong().size(15.0));
            ui.add_space(style::S3);
            for (v, de, en, key) in [
                (ViewKind::Month, "Monat", "Month", "view_month"),
                (ViewKind::Week, "Woche", "Week", "view_week"),
                (ViewKind::Day, "Tag", "Day", "view_day"),
                (ViewKind::List, "Liste", "List", "view_list"),
            ] {
                let r = style::tab(ui, lang.t(de, en), self.view == v);
                self.hits.push((Hit::Button(key), r.rect));
                if r.clicked() {
                    self.set_view(env, v);
                }
            }
            ui.add_space(style::S3);
            let r = ui.button(lang.t("+ Neuer Termin", "+ New event"));
            self.hits.push((Hit::Button("new"), r.rect));
            if r.clicked() {
                let a = self.anchor(env);
                self.form = Some(dialog::EventForm::new_at(a.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default()), false));
            }
            ui.menu_button(lang.t("iCal ▾", "iCal ▾"), |ui| {
                if ui.button(lang.t("Importieren …", "Import …")).clicked() {
                    ui.close();
                    if let Some(p) = crate::platform::notify::open_ics() {
                        self.import_path(env, &p);
                    }
                }
                if ui.button(lang.t("Exportieren …", "Export …")).clicked() {
                    ui.close();
                    if let Some(p) = crate::platform::notify::save_ics("paneldeck-calendar.ics") {
                        self.export_path(env, &p);
                    }
                }
            });
            let r = ui.button(lang.t("Optionen …", "Options …"));
            self.hits.push((Hit::Button("options"), r.rect));
            if r.clicked() {
                self.options_open = true;
            }
            if let Some((m, err)) = &self.msg {
                ui.add_space(style::S2);
                let c = if *err { style::danger_soft() } else { style::dim(ui) };
                ui.label(RichText::new(m).small().color(c));
            }
        });
    }

    /// Überschrift der Ansicht (Monat, Woche, Tag, Liste).
    pub(crate) fn title(&self, env: &CalEnv) -> String {
        let lang = env.lang;
        let a = self.anchor(env);
        match self.view {
            ViewKind::Month => format!("{} {}", builder::month_name(a.month(), lang), a.year()),
            ViewKind::Week => {
                let s = cal::week_start(a);
                let e = cal::add_days(s, 6);
                match lang {
                    Lang::De => format!("KW {} · {}–{}", s.iso_week().week(), s.format("%d.%m."), e.format("%d.%m.%Y")),
                    Lang::En => format!("Week {} · {} – {}", s.iso_week().week(), s.format("%b %d"), e.format("%b %d, %Y")),
                }
            }
            ViewKind::Day => day_title(a, lang),
            ViewKind::List => match lang {
                Lang::De => format!("Ab {}", a.format("%d.%m.%Y")),
                Lang::En => format!("From {}", a.format("%b %d, %Y")),
            },
        }
    }

    /// Termin aus der Datenbank (für Detail und Bearbeiten).
    fn event(&self, env: &CalEnv, id: i64) -> Option<Event> {
        env.conn.and_then(|c| store::get_event(c, id).ok())
    }

    /// Detailbereich rechts: Liste (Listenansicht) oder ausgewählter Eintrag.
    fn detail(&mut self, ui: &mut Ui, env: &CalEnv, items: &[Item]) {
        let lang = env.lang;
        let Some(sel) = self.selected else {
            ui.label(RichText::new(lang.t("Einen Eintrag auswählen, um Einzelheiten zu sehen.", "Select an entry to see its details.")).color(style::dim(ui)));
            return;
        };
        let item = items.iter().find(|i| i.sel_key() == sel).cloned();
        let Some(item) = item else {
            ui.label(RichText::new(lang.t("Der Eintrag liegt nicht im sichtbaren Zeitraum.", "The entry is not in the visible range.")).color(style::dim(ui)));
            if ui.button(lang.t("Auswahl aufheben", "Clear selection")).clicked() {
                self.selected = None;
            }
            return;
        };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 5.0, super::palette_ui::color32(&item.color));
            ui.label(RichText::new(&item.title).strong().size(15.0));
        });
        ui.label(RichText::new(item.kind.label(lang)).small().color(style::dim(ui)));
        ui.add_space(style::S1);
        ui.label(when_text(&item, lang));
        if !item.key.is_empty() {
            ui.label(RichText::new(&item.key).monospace());
        }
        if item.done {
            ui.label(RichText::new(match item.kind {
                Kind::Time => lang.t("Gebucht", "Booked"),
                Kind::Version => lang.t("Erschienen", "Released"),
                Kind::Milestone => lang.t("Erreicht", "Reached"),
                Kind::Sprint => lang.t("Abgeschlossen", "Completed"),
                _ => lang.t("Erledigt", "Done"),
            }).small().color(style::dim(ui)));
        }
        if item.kind != Kind::Event {
            return;
        }
        let Some(ev) = self.event(env, item.id) else { return };
        if let Some(r) = &ev.recur {
            ui.add_space(style::S1);
            if let Ok(c) = Cron::parse(&r.cron) {
                ui.label(format!("↻ {}", builder::describe(&c, lang)));
                if let Some(w) = c.warning(lang) {
                    ui.label(RichText::new(w).small().color(style::danger_soft()));
                }
            }
            let next = ev.next_occurrences(env.now.max(ev.start), 5, env.zone);
            if !next.is_empty() {
                ui.label(RichText::new(lang.t("Nächste Termine:", "Next occurrences:")).small().color(style::dim(ui)));
                for n in next {
                    ui.label(RichText::new(fmt_dt(n, ev.all_day, lang)).small());
                }
            }
        }
        if let Some(m) = ev.reminder {
            ui.label(RichText::new(format!("⏰ {}", dialog::reminder_label(Some(m), lang))).small());
        }
        if !ev.note.is_empty() {
            ui.add_space(style::S1);
            egui::ScrollArea::vertical().id_salt("cal_detail_note").max_height(160.0).show(ui, |ui| {
                ui.label(&ev.note);
            });
        }
        ui.add_space(style::S2);
        ui.horizontal_wrapped(|ui| {
            let r = ui.button(lang.t("Bearbeiten …", "Edit …"));
            self.hits.push((Hit::Button("edit"), r.rect));
            if r.clicked() {
                self.form = Some(dialog::EventForm::from_event(&ev));
            }
            if ev.recur.is_some() {
                let r = ui.button(lang.t("Dieses Vorkommen auslassen", "Skip this occurrence"));
                self.hits.push((Hit::Button("skip"), r.rect));
                if r.clicked()
                    && let Some(conn) = env.conn {
                        match store::skip_occurrence(conn, ev.id, item.start.date()) {
                            Ok(_) => {
                                self.selected = None;
                                self.touched();
                            }
                            Err(e) => self.msg = Some((e.message(lang), true)),
                        }
                    }
            }
            let r = ui.button(RichText::new(lang.t("Löschen …", "Delete …")).color(style::danger_soft()));
            self.hits.push((Hit::Button("delete"), r.rect));
            if r.clicked() {
                self.confirm_delete = Some(ev.id);
            }
        });
    }

    /// Eintrag anklicken: auswählen; doppelt (eigener Termin): bearbeiten.
    pub(crate) fn click_item(&mut self, env: &CalEnv, it: &Item, double: bool) {
        self.selected = Some(it.sel_key());
        if double && it.kind == Kind::Event
            && let Some(ev) = self.event(env, it.id) {
                self.form = Some(dialog::EventForm::from_event(&ev));
            }
    }

    pub(crate) fn import_path(&mut self, env: &CalEnv, path: &std::path::Path) {
        let Some(conn) = env.conn else { return };
        match ics::import_file(conn, path, env.zone) {
            Ok(rep) => {
                self.msg = Some((rep.summary(env.lang), false));
                self.report = Some(rep);
                self.touched();
            }
            Err(e) => self.msg = Some((e.message(env.lang), true)),
        }
    }

    pub(crate) fn export_path(&mut self, env: &CalEnv, path: &std::path::Path) {
        let Some(conn) = env.conn else { return };
        match ics::export_file(conn, path, env.zone) {
            Ok(rep) => {
                let text = match env.lang {
                    Lang::De if rep.without_rrule.is_empty() => format!("{} Termine exportiert.", rep.events),
                    Lang::De => format!("{} Termine exportiert ({} Wiederholungen nur als Cron, ohne RRULE).", rep.events, rep.without_rrule.len()),
                    Lang::En if rep.without_rrule.is_empty() => format!("{} events exported.", rep.events),
                    Lang::En => format!("{} events exported ({} recurrences only as cron, without RRULE).", rep.events, rep.without_rrule.len()),
                };
                self.msg = Some((text, false));
            }
            Err(e) => self.msg = Some((e.message(env.lang), true)),
        }
    }

    /// Dialoge: Termin, Optionen, Löschen, Import-Bericht.
    fn dialogs(&mut self, ctx: &egui::Context, env: &CalEnv) {
        let lang = env.lang;
        if let Some(mut f) = self.form.take() {
            match f.show(ctx, env) {
                dialog::Outcome::Open => self.form = Some(f),
                dialog::Outcome::Cancel => {}
                dialog::Outcome::Saved(ev) => {
                    self.selected = Some((Kind::Event, ev.id, ev.start));
                    self.anchor = Some(ev.start.date());
                    self.touched();
                }
            }
        }
        if self.options_open {
            self.options_window(ctx, env);
        }
        if let Some(id) = self.confirm_delete {
            let title = self.event(env, id).map(|e| e.title).unwrap_or_default();
            let mut close = false;
            let m = style::modal(egui::Id::new("cal_delete")).show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.label(RichText::new(lang.t("Termin löschen?", "Delete the event?")).strong());
                ui.label(match lang {
                    Lang::De => format!("„{title}“ wird mit allen Wiederholungen gelöscht."),
                    Lang::En => format!("“{title}” is deleted with all its recurrences."),
                });
                ui.add_space(style::S2);
                ui.horizontal(|ui| {
                    let r = ui.button(RichText::new(lang.t("Löschen", "Delete")).color(style::danger_soft()));
                    self.hits.push((Hit::Button("delete_confirm"), r.rect));
                    if r.clicked() {
                        if let Some(conn) = env.conn
                            && let Err(e) = store::delete_event(conn, id) {
                                self.msg = Some((e.message(lang), true));
                            }
                        self.selected = None;
                        self.touched();
                        close = true;
                    }
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        close = true;
                    }
                });
            });
            if close || m.should_close() {
                self.confirm_delete = None;
            }
        }
        if let Some(rep) = &self.report {
            let mut close = false;
            let m = style::modal(egui::Id::new("cal_import_report")).show(ctx, |ui| {
                ui.set_max_width(520.0);
                ui.label(RichText::new(lang.t("iCal-Import", "iCal import")).strong());
                ui.label(rep.summary(lang));
                egui::ScrollArea::vertical().id_salt("cal_report").max_height(300.0).show(ui, |ui| {
                    for s in &rep.skipped {
                        let why = if lang == Lang::De { &s.de } else { &s.en };
                        ui.label(RichText::new(format!("✗ {} {}: {} – {why}", lang.t("Zeile", "Line"), s.line, s.title)).small().color(style::danger_soft()));
                    }
                    for n in &rep.notes {
                        let why = if lang == Lang::De { &n.de } else { &n.en };
                        ui.label(RichText::new(format!("ℹ {}: {why}", n.title)).small());
                    }
                    for d in &rep.duplicates {
                        ui.label(RichText::new(format!("= {} ({})", d, lang.t("schon vorhanden", "already there"))).small().color(style::dim(ui)));
                    }
                });
                if ui.button("OK").clicked() {
                    close = true;
                }
            });
            if close || m.should_close() {
                self.report = None;
            }
        }
    }

    fn options_window(&mut self, ctx: &egui::Context, env: &CalEnv) {
        let lang = env.lang;
        let Some(conn) = env.conn else { return };
        let mut o = self.options(conn).clone();
        let before = o.clone();
        let projects = sources::projects(conn);
        let mut close = false;
        let m = style::modal(egui::Id::new("cal_options")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(RichText::new(lang.t("Kalender – Optionen", "Calendar – options")).strong());
            ui.label(RichText::new(lang.t("Was der Kalender außer eigenen Terminen zeigt, für alle Projekte und je Projekt.", "What the calendar shows besides own events, for all projects and per project.")).small().color(style::dim(ui)));
            ui.add_space(style::S1);
            egui::ScrollArea::vertical().id_salt("cal_opts").max_height(360.0).show(ui, |ui| {
                egui::Grid::new("cal_opts_grid").num_columns(6).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                    ui.label("");
                    ui.label(RichText::new(lang.t("Fälligkeiten", "Due dates")).small().strong());
                    ui.label(RichText::new(lang.t("Sprints", "Sprints")).small().strong());
                    ui.label(RichText::new(lang.t("Versionen", "Versions")).small().strong());
                    ui.label(RichText::new(lang.t("Gebuchte Zeit", "Booked time")).small().strong());
                    ui.label("");
                    ui.end_row();
                    ui.label(RichText::new(lang.t("Alle Projekte (Standard)", "All projects (default)")).strong());
                    sources_row(ui, &mut o.sources, true);
                    ui.label("");
                    ui.end_row();
                    let mut rows: Vec<(i64, String)> = vec![(0, lang.t("Einfache Todo-Liste", "Simple to-do list").to_string())];
                    rows.extend(projects.iter().map(|(id, k, n)| (*id, format!("{k} – {n}"))));
                    for (id, name) in rows {
                        ui.label(&name);
                        let mut s = o.for_project(id);
                        let own = o.per_project.contains_key(&id);
                        let changed = sources_row(ui, &mut s, id != 0);
                        if changed {
                            o.per_project.insert(id, s);
                        }
                        if own {
                            if ui.small_button(lang.t("Standard", "Default")).on_hover_text(lang.t("Wieder die Einstellung für alle Projekte nehmen", "Use the setting for all projects again")).clicked() {
                                o.per_project.remove(&id);
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(style::S2);
            ui.checkbox(&mut o.reminders, lang.t("Erinnerungen an Termine (Benachrichtigung zur eingestellten Zeit)", "Event reminders (notification at the set time)"));
            ui.add_space(style::S2);
            if ui.button(lang.t("Schließen", "Close")).clicked() {
                close = true;
            }
        });
        if o != before {
            let _ = store::save_options(conn, &o);
            self.opts = Some(o);
            self.generation += 1;
        }
        if close || m.should_close() {
            self.options_open = false;
        }
    }
}

/// Eine Zeile Quellen-Schalter; liefert, ob sich etwas geändert hat. Die Todo-Liste hat nur Fälligkeiten.
fn sources_row(ui: &mut Ui, s: &mut Sources, all: bool) -> bool {
    let mut ch = ui.checkbox(&mut s.due, "").changed();
    if all {
        ch |= ui.checkbox(&mut s.sprints, "").changed();
        ch |= ui.checkbox(&mut s.versions, "").changed();
        ch |= ui.checkbox(&mut s.time, "").changed();
    } else {
        ui.label("");
        ui.label("");
        ui.label("");
    }
    ch
}

// ───────────────────────── Doku-Bilder (`shot_start`, Feature `readme-shots`)

/// Beispieltermine für die Doku-Bilder rund um `today` (nur, wenn es noch keine Termine gibt): eine Serie werktags,
/// eine monatliche Serie am n-ten Wochentag, ein mehrtägiger Termin, Einzeltermine und zwei Todo-Fälligkeiten; Sprints
/// und Versionen in den Optionen an.
pub(crate) fn demo_seed(conn: &Connection, today: NaiveDate, lang: Lang) {
    if store::all_events(conn).map(|v| !v.is_empty()).unwrap_or(true) {
        return;
    }
    let monday = cal::week_start(today);
    let at = |d: i64, h: u32, m: u32| (monday + Duration::days(d)).and_hms_opt(h, m, 0).unwrap_or_default();
    let mk = |title: &str, start: NaiveDateTime, minutes: i64, cron: Option<&str>, all_day: bool, color: &str, reminder: Option<i64>| {
        let mut i = store::EventInput::new(title, start, minutes);
        i.all_day = all_day;
        i.color = color.into();
        i.reminder = reminder;
        i.recur = cron.map(|c| cal::Recurrence { cron: c.into(), ..Default::default() });
        let _ = store::create_event(conn, i, None, "local");
    };
    mk(lang.t("Standup", "Standup"), at(-7, 9, 30), 15, Some("30 9 * * MON-FRI"), false, "#61AFEF", Some(5));
    mk(lang.t("Sprint-Review", "Sprint review"), at(-28, 14, 0), 60, Some("0 14 * * FRI#2"), false, "#E5A04B", None);
    mk(lang.t("Konferenz", "Conference"), at(9, 0, 0), 3 * 1440, None, true, "#C678DD", None);
    mk(lang.t("Zahnarzt", "Dentist"), at(2, 16, 0), 45, None, false, "#98C379", Some(60));
    mk(lang.t("Mittagessen mit Ana", "Lunch with Ana"), at(1, 12, 30), 60, None, false, "#56B6C2", None);
    mk(lang.t("Release-Planung", "Release planning"), at(3, 10, 0), 90, None, false, "#E06C75", None);
    let now = cal::fmt_stored(today.and_time(NaiveTime::MIN));
    for (t, due) in [(lang.t("Domain verlängern", "Renew the domain"), (today + Duration::days(3)).format("%Y-%m-%d").to_string()), (lang.t("Rechnung bezahlen", "Pay the invoice"), format!("{}T17:00", (today + Duration::days(8)).format("%Y-%m-%d")))] {
        let _ = conn.execute(
            "INSERT INTO todo_items (title, body, done, position, created_at, updated_at, due) VALUES (?1, '', 0, (SELECT COALESCE(MAX(position), 0) + 1 FROM todo_items), ?2, ?2, ?3)",
            rusqlite::params![t, now, due],
        );
    }
    let mut o = store::load_options(conn);
    o.sources.sprints = true;
    o.sources.versions = true;
    let _ = store::save_options(conn, &o);
}

impl CalUi {
    /// Zustand für ein Doku-Bild: `month` (Standup ausgewählt, Details rechts), `week`, `dialog` (Standup bearbeiten).
    pub(crate) fn demo_prepare(&mut self, env: &CalEnv, which: &str) {
        let Some(conn) = env.conn else { return };
        let today = env.now.date();
        self.anchor = Some(today);
        self.view = if which == "week" { ViewKind::Week } else { ViewKind::Month };
        let standup = store::all_events(conn).ok().and_then(|v| v.into_iter().find(|e| e.recur.as_ref().is_some_and(|r| r.cron.contains("MON-FRI"))));
        if let Some(e) = standup {
            if let Some(s) = e.next_occurrences(env.now, 1, env.zone).first() {
                self.selected = Some((Kind::Event, e.id, *s));
            }
            if which == "dialog" {
                self.form = Some(dialog::EventForm::from_event(&e));
            }
        }
        self.invalidate();
    }
}

/// Doku-Bild: den Kalender über das ganze Fenster unter der Titelleiste zeichnen (bis der Zeit-Reiter da ist).
#[cfg(feature = "readme-shots")]
pub(crate) fn shot_draw(app: &mut super::App, ctx: &egui::Context) {
    let full = ctx.content_rect();
    let r = Rect::from_min_max(egui::pos2(full.left(), full.top() + 40.0), full.max);
    egui::Area::new(egui::Id::new("shot_calendar")).order(egui::Order::Middle).fixed_pos(r.min).show(ctx, |ui| {
        egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(12).show(ui, |ui| {
            let size = r.size() - Vec2::splat(24.0);
            ui.set_min_size(size);
            ui.set_max_size(size);
            show(app, ui);
        });
    });
}

/// „Montag, 23. November 2026“ / „Monday, November 23, 2026“.
pub(crate) fn day_title(d: NaiveDate, lang: Lang) -> String {
    let wd = builder::weekday_name(d.weekday().num_days_from_sunday() as u8, lang);
    let m = builder::month_name(d.month(), lang);
    match lang {
        Lang::De => format!("{wd}, {}. {m} {}", d.day(), d.year()),
        Lang::En => format!("{wd}, {m} {}, {}", d.day(), d.year()),
    }
}

/// Zeit eines Eintrags (mit Datum).
pub(crate) fn fmt_dt(t: NaiveDateTime, all_day: bool, lang: Lang) -> String {
    let f = match (lang, all_day) {
        (Lang::De, true) => "%a %d.%m.%Y",
        (Lang::De, false) => "%a %d.%m.%Y %H:%M",
        (Lang::En, true) => "%a %b %d, %Y",
        (Lang::En, false) => "%a %b %d, %Y %H:%M",
    };
    let s = t.format(f).to_string();
    if lang == Lang::De { de_weekday(&s) } else { s }
}

/// Englische Kurz-Wochentage aus `chrono` (ohne Locale) ins Deutsche.
fn de_weekday(s: &str) -> String {
    let map = [("Mon", "Mo"), ("Tue", "Di"), ("Wed", "Mi"), ("Thu", "Do"), ("Fri", "Fr"), ("Sat", "Sa"), ("Sun", "So")];
    for (en, de) in map {
        if let Some(rest) = s.strip_prefix(en) {
            return format!("{de}{rest}");
        }
    }
    s.to_string()
}

/// Beginn–Ende eines Eintrags in Worten.
pub(crate) fn when_text(it: &Item, lang: Lang) -> String {
    let (d0, d1) = it.day_span();
    if it.all_day {
        if d0 == d1 {
            format!("{} · {}", fmt_dt(it.start, true, lang), lang.t("ganztägig", "all day"))
        } else {
            format!("{} – {}", fmt_dt(d0.and_time(NaiveTime::MIN), true, lang), fmt_dt(d1.and_time(NaiveTime::MIN), true, lang))
        }
    } else if it.end <= it.start {
        fmt_dt(it.start, false, lang)
    } else if it.start.date() == it.end.date() || it.end == (it.start.date() + Duration::days(1)).and_time(NaiveTime::MIN) {
        format!("{} – {}", fmt_dt(it.start, false, lang), cal::fmt_time(it.end))
    } else {
        format!("{} – {}", fmt_dt(it.start, false, lang), fmt_dt(it.end, false, lang))
    }
}
