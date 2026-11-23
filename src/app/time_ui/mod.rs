//! Reiter „Zeit“ (#164, ab 3.2.0): Unterreiter Wecker, Timer, Kalender, Stoppuhren, Weltuhren, Geburtstage – je eine
//! Liste mit Detailbereich rechts. Ein Zustand für alle Fenster (`App::time`, nicht in `view_fields!`); der Takt (Klingeln,
//! verpasste Wecker, abgelaufene Timer), die Benachrichtigungen und das Fenster „klingelt“ laufen nur im Hauptdurchlauf
//! (`app_glue.rs`).
//!
//! Die Oberfläche selbst ([`TimeUi::show`]) braucht nur eine [`Env`] (Datenbank, Sprache, Uhrzeit, Einstellungen) und lässt
//! sich so ohne Fenster testen (`tests.rs`).

mod alarms;
mod app_glue;
mod birthdays;
mod clocks;
mod stopwatches;
#[cfg(test)]
mod tests;
mod timers;

use crate::app::tutorial::{Spot, TutMark};
use crate::state::Lang;
use crate::style;
use crate::time::settings::TimeSettings;
use crate::time::sounds::{SoundRef, BUILTIN};
use crate::time::{store, TimeData};
use eframe::egui::{self, Align, Color32, FontId, Id, Layout, Pos2, Rect, RichText, Sense, Ui, UiBuilder, Vec2};
use rusqlite::Connection;

/// Unterreiter (Reihenfolge wie in der Leiste).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Sub {
    #[default]
    Alarms,
    Timers,
    Calendar,
    Stopwatches,
    Clocks,
    Birthdays,
}

impl Sub {
    pub const ALL: [Sub; 6] = [Sub::Alarms, Sub::Timers, Sub::Calendar, Sub::Stopwatches, Sub::Clocks, Sub::Birthdays];

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            Sub::Alarms => lang.t("Wecker", "Alarms"),
            Sub::Timers => lang.t("Timer", "Timers"),
            Sub::Calendar => lang.t("Kalender", "Calendar"),
            Sub::Stopwatches => lang.t("Stoppuhren", "Stopwatches"),
            Sub::Clocks => lang.t("Weltuhren", "Clocks"),
            Sub::Birthdays => lang.t("Geburtstage", "Birthdays"),
        }
    }
}

/// Was die Oberfläche zum Zeichnen braucht.
pub(crate) struct Env<'a> {
    pub conn: Option<&'a Connection>,
    pub lang: Lang,
    /// Jetzt (Unix-ms).
    pub now_ms: i64,
}

/// Aufträge der Oberfläche an die Anwendung.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Out {
    /// Kurzer Hinweis (Toast).
    Toast(String),
    /// Einstellungen geändert (speichern).
    SettingsChanged,
    /// Zeit wurde an einem Ticket gebucht (Ansichten der Todo-Liste neu laden).
    Booked,
}

/// Was gerade klingelt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ring {
    pub what: RingKind,
    /// Seit (Unix-ms).
    pub since: i64,
    pub title: String,
    pub body: String,
    /// Klang läuft noch (nach der Höchstdauer verstummt er, das Fenster bleibt).
    pub sounding: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RingKind {
    Alarm(i64),
    Timer(i64),
}

impl RingKind {
    pub fn key(self) -> String {
        match self {
            RingKind::Alarm(id) => format!("alarm:{id}"),
            RingKind::Timer(id) => format!("timer:{id}"),
        }
    }
}

/// Offene Rückfrage „47 min auf PAN-12 buchen?“.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BookAsk {
    pub ticket: String,
    pub minutes: u32,
    /// Beginn (Unix-ms).
    pub start_ms: i64,
    /// Kommentar des Zeiteintrags (Name des Timers/der Stoppuhr).
    pub comment: String,
    /// Stoppuhr, deren gebuchte Zeit nach dem Buchen steigt.
    pub stopwatch: Option<(i64, i64)>,
}

/// Bearbeitungspuffer für das Datum eines Geburtstags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BdayEdit {
    pub id: i64,
    pub day: u32,
    pub month: u32,
    pub has_year: bool,
    pub year: i32,
}

/// Zustand des Reiters.
#[derive(Default)]
pub(crate) struct TimeUi {
    pub data: TimeData,
    pub loaded: bool,
    /// Fehler beim Laden/Speichern (wird oben angezeigt).
    pub error: Option<String>,
    /// Tabellen fehlen oder ließen sich nicht laden: kein Takt, Agenten bekommen `db_unavailable`.
    pub unavailable: bool,
    pub sub: Sub,
    pub sel_alarm: Option<i64>,
    pub sel_timer: Option<i64>,
    pub sel_stopwatch: Option<i64>,
    pub sel_clock: Option<i64>,
    pub sel_birthday: Option<i64>,
    /// Suchtext je Liste.
    pub filter: [String; 6],
    pub ringing: Vec<Ring>,
    /// Hinweise (verpasste Wecker, abgelaufene Timer), oben im Reiter bis „Ausblenden“.
    pub notes: Vec<String>,
    pub book: Vec<BookAsk>,
    pub options_open: bool,
    /// Rückfrage „Löschen?“ für diesen Eintrag.
    pub confirm_delete: Option<(Sub, i64)>,
    /// Suche in der Zonenliste der Weltuhr.
    pub zone_query: String,
    /// Name für „Als Vorlage speichern“.
    pub preset_name: String,
    /// Datum des gewählten Geburtstags in Bearbeitung (gespeichert wird nur ein gültiges).
    pub bday_edit: Option<BdayEdit>,
    pub out: Vec<Out>,
    /// Takt: zuletzt (Unix-ms) und Startprüfung erledigt.
    pub last_tick: i64,
    pub started: bool,
    /// Vorschau läuft (Klang-Schlüssel `preview`).
    pub previewing: bool,
    /// Erinnerungen an Kalendertermine (alle 30 s abgefragt, `calendar::remind`).
    pub cal_poller: crate::calendar::remind::Poller,
    pub last_cal_poll: i64,
}

impl TimeUi {
    /// Lädt die Daten einmal (beim ersten Öffnen bzw. ersten Takt).
    pub fn ensure_loaded(&mut self, conn: Option<&Connection>) {
        if self.loaded {
            return;
        }
        let Some(conn) = conn else { return };
        self.loaded = true;
        if !store::available(conn) {
            self.error = Some(crate::state::lang().t("Die Datenbank kennt den Reiter „Zeit“ noch nicht.", "The database does not know the Time tab yet.").into());
            self.unavailable = true;
            return;
        }
        match store::load(conn) {
            Ok(d) => self.data = d,
            Err(e) => {
                self.error = Some(e.to_string());
                self.unavailable = true;
            }
        }
    }

    pub fn take_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    fn db_result<T>(&mut self, r: rusqlite::Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    pub fn is_ringing(&self, k: RingKind) -> bool {
        self.ringing.iter().any(|r| r.what == k)
    }

    /// Klingeln beenden (Fenster weg, Klang aus).
    pub fn stop_ring(&mut self, k: RingKind) {
        if self.ringing.iter().any(|r| r.what == k) {
            self.ringing.retain(|r| r.what != k);
            crate::time::notify::stop(&k.key());
        }
    }

    /// Den ganzen Reiter zeichnen (Leiste, Liste, Details). `settings` für das Optionen-Fenster.
    pub fn show(&mut self, ctx: &egui::Context, env: &Env, settings: &mut TimeSettings) {
        self.ensure_loaded(env.conn);
        let lang = env.lang;
        egui::TopBottomPanel::top(style::salted(Id::new("time_tabs"))).frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let left = ui.cursor().min;
                for s in Sub::ALL {
                    if style::tab(ui, s.label(lang), self.sub == s).clicked() {
                        self.sub = s;
                        self.confirm_delete = None;
                    }
                }
                let tabs = egui::Rect::from_min_max(left, egui::pos2(ui.min_rect().right(), left.y + 26.0));
                crate::app::tutorial::spot_rect(ui, Spot::TimeSubTabs, tabs);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(lang.t("Optionen …", "Options …")).on_hover_text(lang.t("Benachrichtigungen, Klang, Zeiterfassung", "Notifications, sound, time tracking")).spot(Spot::TimeOptions).clicked() {
                        self.options_open = !self.options_open;
                    }
                    let ringing = self.ringing.len();
                    if ringing > 0 {
                        ui.label(RichText::new(format!("⏰ {ringing}")).color(style::accent())).on_hover_text(lang.t("Klingelt gerade", "Ringing now"));
                    }
                });
            });
            if let Some(e) = self.error.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(style::danger_soft(), e);
                    if ui.small_button("×").clicked() {
                        self.error = None;
                    }
                });
            }
            if !self.notes.is_empty() {
                ui.add_space(4.0);
                egui::Frame::new().fill(style::accent().gamma_multiply(0.12)).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                    let n = self.notes.len();
                    for t in self.notes.iter().rev().take(5) {
                        ui.label(t);
                    }
                    ui.horizontal(|ui| {
                        if n > 5 {
                            ui.label(RichText::new(format!("+{} {}", n - 5, lang.t("weitere", "more"))).color(style::dim(ui)));
                        }
                        if ui.small_button(lang.t("Ausblenden", "Dismiss")).clicked() {
                            self.notes.clear();
                        }
                    });
                });
            }
        });
        if self.sub == Sub::Calendar {
            // Der Kalender braucht die ganze Anwendung (`calendar_ui::show(app, ui)`): `App::time_view` zeichnet ihn im
            // Hauptbereich (eigener Detailbereich rechts), hier nur Leiste und Hinweise.
        } else {
            egui::SidePanel::right(style::salted(Id::new("time_detail")))
                .resizable(true)
                .default_width(400.0)
                .width_range(300.0..=700.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().id_salt("time_detail_scroll").show(ui, |ui| {
                        ui.add_space(8.0);
                        match self.sub {
                            Sub::Alarms => self.alarm_detail(ui, env, settings),
                            Sub::Timers => self.timer_detail(ui, env, settings),
                            Sub::Stopwatches => self.stopwatch_detail(ui, env, settings),
                            Sub::Clocks => self.clock_detail(ui, env, settings),
                            Sub::Birthdays => self.birthday_detail(ui, env),
                            Sub::Calendar => {}
                        }
                    });
                });
            egui::CentralPanel::default().show(ctx, |ui| match self.sub {
                Sub::Alarms => self.alarm_list(ui, env),
                Sub::Timers => self.timer_list(ui, env),
                Sub::Stopwatches => self.stopwatch_list(ui, env),
                Sub::Clocks => self.clock_list(ui, env, settings),
                Sub::Birthdays => self.birthday_list(ui, env),
                Sub::Calendar => {}
            });
        }
        if self.options_open {
            self.options_window(ctx, lang, settings);
        }
        // laufende Anzeigen (Timer, Stoppuhren, Uhren) ticken lassen
        let live = match self.sub {
            Sub::Timers => self.data.timers.iter().any(|t| t.is_running()),
            Sub::Stopwatches => self.data.stopwatches.iter().any(|s| s.running()),
            Sub::Clocks | Sub::Alarms => true,
            _ => false,
        };
        // Minimiert bzw. im Tray sieht niemand die Anzeige: kein schnelles Neuzeichnen (Wecker und Timer weckt der Takt).
        let hidden = ctx.input(|i| i.viewport().minimized == Some(true));
        if live && !hidden {
            let fast = self.sub == Sub::Stopwatches;
            ctx.request_repaint_after(std::time::Duration::from_millis(if fast { 100 } else { 500 }));
        }
    }

    /// Kopf einer Liste: „+ Neu“ und Suchfeld. Liefert `true` bei Klick auf „Neu“.
    fn list_head(&mut self, ui: &mut Ui, idx: usize, new_label: &str, count: usize, lang: Lang) -> bool {
        let mut new = false;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ui.button(format!("+ {new_label}"));
            // Ziel des Tutorials: „+ Neuer Wecker“ (Liste 0)
            new = if idx == 0 { r.spot(Spot::TimeNewAlarm).clicked() } else { r.clicked() };
            ui.add(egui::TextEdit::singleline(&mut self.filter[idx]).hint_text(lang.t("Suchen …", "Search …")).desired_width(180.0));
            ui.label(RichText::new(count.to_string()).color(style::dim(ui)));
        });
        ui.add_space(4.0);
        ui.separator();
        new
    }

    fn matches(&self, idx: usize, text: &str) -> bool {
        let f = self.filter[idx].trim();
        f.is_empty() || text.to_lowercase().contains(&f.to_lowercase())
    }

    /// „Löschen“ mit Rückfrage. `true`, wenn bestätigt.
    fn delete_button(&mut self, ui: &mut Ui, sub: Sub, id: i64, lang: Lang) -> bool {
        ui.add_space(12.0);
        if self.confirm_delete == Some((sub, id)) {
            let mut yes = false;
            ui.horizontal(|ui| {
                ui.label(lang.t("Wirklich löschen?", "Really delete?"));
                if ui.button(RichText::new(lang.t("Löschen", "Delete")).color(style::danger_soft())).clicked() {
                    yes = true;
                }
                if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                    self.confirm_delete = None;
                }
            });
            if yes {
                self.confirm_delete = None;
            }
            yes
        } else {
            if ui.button(RichText::new(lang.t("Löschen …", "Delete …")).color(style::danger_soft())).clicked() {
                self.confirm_delete = Some((sub, id));
            }
            false
        }
    }

    /// Optionen des Reiters (Benachrichtigungen usw.).
    fn options_window(&mut self, ctx: &egui::Context, lang: Lang, s: &mut TimeSettings) {
        let mut open = true;
        let before = s.clone();
        egui::Window::new(lang.t("Zeit – Optionen", "Time – options"))
            .id(style::salted(Id::new("time_options")))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label(RichText::new(lang.t("Benachrichtigungen", "Notifications")).strong());
                egui::Grid::new("time_opt_grid").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("");
                    ui.label(lang.t("Klang", "Sound"));
                    ui.label(lang.t("Toast", "Toast"));
                    ui.end_row();
                    for (label, snd, toast) in [
                        (lang.t("Wecker", "Alarms"), &mut s.alarm_sound, &mut s.alarm_toast),
                        (lang.t("Timer-Ende", "Timer end"), &mut s.timer_sound, &mut s.timer_toast),
                        (lang.t("Termin-Erinnerungen", "Appointment reminders"), &mut s.reminder_sound, &mut s.reminder_toast),
                        (lang.t("Geburtstage", "Birthdays"), &mut s.birthday_sound, &mut s.birthday_toast),
                    ] {
                        ui.label(label);
                        style::toggle(ui, snd);
                        style::toggle(ui, toast);
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    style::toggle(ui, &mut s.flash);
                    ui.label(lang.t("Taskleistenknopf blinken lassen", "Flash the taskbar button"));
                });
                ui.horizontal(|ui| {
                    ui.label(lang.t("Lautstärke", "Volume"));
                    ui.add(egui::Slider::new(&mut s.volume, 0..=100).suffix(" %"));
                });
                ui.horizontal(|ui| {
                    ui.label(lang.t("Wecker klingeln höchstens", "Alarms ring for at most"));
                    ui.add(egui::DragValue::new(&mut s.ring_minutes).range(crate::time::settings::RING_MINUTES_RANGE.0..=crate::time::settings::RING_MINUTES_RANGE.1).suffix(" min"));
                });
                ui.add_space(8.0);
                ui.label(RichText::new(lang.t("Weiteres", "More")).strong());
                ui.horizontal(|ui| {
                    style::toggle(ui, &mut s.ticket_booking);
                    ui.label(lang.t("Zeit von Timern und Stoppuhren an Tickets buchen", "Book time of timers and stopwatches on tickets"));
                });
                ui.horizontal(|ui| {
                    style::toggle(ui, &mut s.clock_seconds);
                    ui.label(lang.t("Sekunden in Weltuhren", "Seconds in world clocks"));
                });
                ui.add_space(4.0);
                ui.label(RichText::new(lang.t("Wecker und Timer klingeln nur, solange Paneldeck läuft (auch minimiert oder im Tray).", "Alarms and timers ring only while Paneldeck runs (also minimized or in the tray).")).small().color(style::dim(ui)));
            });
        if !open {
            self.options_open = false;
        }
        if *s != before {
            self.out.push(Out::SettingsChanged);
        }
    }
}

/// Zeile einer Liste: Titel, Unterzeile, Wert rechts, Steuerelemente ganz rechts. Liefert „angeklickt“.
#[allow(clippy::too_many_arguments)]
pub(super) fn row(ui: &mut Ui, selected: bool, title: &str, sub: &str, value: &str, value_col: Option<Color32>, ctl_w: f32, controls: impl FnOnce(&mut Ui)) -> bool {
    let h = ROW_H;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 6.0, style::accent().gamma_multiply(0.22));
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, Color32::from_white_alpha(10));
    }
    let text = ui.visuals().text_color();
    let dim = style::dim(ui);
    let left = rect.left() + 10.0;
    let val_right = rect.right() - ctl_w - 10.0;
    let vg = p.layout_no_wrap(value.to_string(), FontId::monospace(16.0), value_col.unwrap_or(text));
    let vw = vg.size().x;
    p.galley(Pos2::new(val_right - vw, rect.center().y - vg.size().y / 2.0), vg, text);
    let max_text = (val_right - vw - 12.0 - left).max(20.0);
    p.galley(Pos2::new(left, rect.top() + 5.0), one_line(p, title, 14.0, text, max_text), text);
    if !sub.is_empty() {
        p.galley(Pos2::new(left, rect.top() + 24.0), one_line(p, sub, 11.5, dim, max_text), dim);
    }
    if ctl_w > 0.0 {
        let cr = Rect::from_min_max(Pos2::new(rect.right() - ctl_w - 4.0, rect.top()), Pos2::new(rect.right() - 4.0, rect.bottom()));
        let mut child = ui.new_child(UiBuilder::new().max_rect(cr).layout(Layout::right_to_left(Align::Center)));
        controls(&mut child);
    }
    resp.clicked()
}

/// Text auf eine Zeile mit „…“ am Ende, höchstens `max` breit.
fn one_line(p: &egui::Painter, text: &str, size: f32, col: Color32, max: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), FontId::proportional(size), col, max);
    job.wrap = egui::text::TextWrapping { max_width: max, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    p.layout_job(job)
}

pub(super) const ROW_H: f32 = 44.0;

/// Liste mit vielen Zeilen (nur die sichtbaren werden gezeichnet).
pub(super) fn rows(ui: &mut Ui, id: &str, n: usize, mut f: impl FnMut(&mut Ui, usize)) {
    egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show_rows(ui, ROW_H + ui.spacing().item_spacing.y, n, |ui, range| {
        for i in range {
            f(ui, i);
        }
    });
}

/// Klangwahl mit Vorschau. `current` ist der gespeicherte Klang (`builtin:…`/`file:…`); liefert den neuen bei Änderung.
pub(super) fn sound_picker(ui: &mut Ui, id: &str, current: &str, fallback: &'static str, previewing: &mut bool, volume: f32, lang: Lang) -> Option<String> {
    let cur = SoundRef::parse(current, fallback);
    let mut out = None;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(id).selected_text(cur.label(lang)).width(170.0).show_ui(ui, |ui| {
            for (name, de, en) in BUILTIN {
                let r = SoundRef::Builtin(name);
                if ui.selectable_label(cur == r, lang.t(de, en)).clicked() {
                    out = Some(r.key());
                }
            }
            ui.separator();
            if ui.selectable_label(matches!(cur, SoundRef::File(_)), lang.t("Eigene Datei (WAV/MP3) …", "Own file (WAV/MP3) …")).clicked()
                && let Some(p) = crate::platform::notify::pick_sound() {
                    out = Some(SoundRef::File(p).key());
                }
        });
        let label = if *previewing { "■" } else { "▶" };
        if ui.button(label).on_hover_text(lang.t("Vorschau", "Preview")).clicked() {
            if *previewing {
                crate::platform::sound::stop("preview");
                *previewing = false;
            } else {
                let r = SoundRef::parse(out.as_deref().unwrap_or(current), fallback);
                match r.clip(lang) {
                    Ok(c) => {
                        crate::platform::sound::play("preview", c, false, volume);
                        *previewing = true;
                    }
                    Err(e) => {
                        ui.ctx().data_mut(|d| d.insert_temp(Id::new(("sound_err", id)), e));
                    }
                }
            }
        }
    });
    if let Some(e) = ui.ctx().data(|d| d.get_temp::<String>(Id::new(("sound_err", id)))) {
        ui.colored_label(style::danger_soft(), e);
    }
    if let SoundRef::File(p) = &cur
        && !p.exists() {
            ui.colored_label(style::danger_soft(), lang.t("Datei fehlt – es klingt der Standardklang.", "File missing – the default sound plays."));
        }
    out
}

/// Ticketfeld für die Zeiterfassung: Schlüssel eingeben, gültig = Titel des Tickets. Liefert den neuen Text bei Änderung.
pub(super) fn ticket_field(ui: &mut Ui, id: &str, current: &str, conn: Option<&Connection>, lang: Lang) -> Option<String> {
    let mut text = current.to_string();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(lang.t("Ticket", "Ticket"));
        changed = ui.add(egui::TextEdit::singleline(&mut text).id_salt(id).hint_text(lang.t("z. B. PAN-12 (leer = keins)", "e.g. PAN-12 (empty = none)")).desired_width(140.0)).changed();
    });
    let key = text.trim();
    if !key.is_empty() {
        match conn.map(|c| crate::planning::tickets::ticket_by_key(c, key)) {
            Some(Ok(t)) => {
                ui.label(RichText::new(format!("{} – {}", t.key(), t.title)).small().color(style::dim(ui)));
            }
            _ => {
                ui.colored_label(style::danger_soft(), lang.t("Kein Ticket mit diesem Schlüssel.", "No ticket with this key."));
            }
        }
    }
    changed.then(|| text.trim().to_uppercase())
}

/// Große Zeitanzeige.
pub(super) fn big_time(ui: &mut Ui, text: &str, col: Option<Color32>) {
    let c = col.unwrap_or(ui.visuals().text_color());
    ui.label(RichText::new(text).font(FontId::monospace(34.0)).color(c));
}

/// Lokale Zeit (HH:MM) eines Zeitpunkts (Unix-ms).
pub(super) fn local_hhmm(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

/// Beispieldaten und Startansicht für die Doku-Bilder (`tools/docs-shots.ps1`, nur mit dem Feature `readme-shots`).
#[cfg(feature = "readme-shots")]
impl TimeUi {
    pub(in crate::app) fn shot_demo(&mut self, conn: &Connection, view: &str, lang: Lang) {
        use crate::time::{alarm, Alarm, Birthday, Preset, Stopwatch, Timer, WorldClock};
        let now = crate::time::now_ms();
        let now_s = now / 1000;
        if store::load(conn).map(|d| d.alarms.is_empty() && d.timers.is_empty() && d.clocks.is_empty()).unwrap_or(false) {
            let de = lang == Lang::De;
            let _ = store::in_tx(conn, || {
                let mut a = Alarm::new(if de { "Aufstehen" } else { "Wake up" }, 6, 45, alarm::WORKDAYS, now_s);
                a.sound = "builtin:bell".into();
                store::save_alarm(conn, &mut a)?;
                store::save_alarm(conn, &mut Alarm::new(if de { "Wochenende" } else { "Weekend" }, 9, 0, alarm::WEEKEND, now_s))?;
                let mut off = Alarm::new(if de { "Standup" } else { "Stand-up" }, 9, 30, alarm::WORKDAYS, now_s);
                off.enabled = false;
                store::save_alarm(conn, &mut off)?;
                store::save_preset(conn, &mut Preset { id: 0, name: if de { "Tee" } else { "Tea" }.into(), duration_ms: 4 * 60_000 })?;
                store::save_preset(conn, &mut Preset { id: 0, name: "Pomodoro".into(), duration_ms: 25 * 60_000 })?;
                let mut t = Timer::new("Pomodoro", 25 * 60_000);
                t.start(now - 7 * 60_000);
                store::save_timer(conn, &mut t)?;
                let mut t2 = Timer::new(if de { "Tee" } else { "Tea" }, 4 * 60_000);
                t2.state = crate::time::TimerState::Expired { at: now - 12 * 60_000 };
                store::save_timer(conn, &mut t2)?;
                store::save_timer(conn, &mut Timer::new(if de { "Build prüfen" } else { "Check the build" }, 15 * 60_000))?;
                let mut s = Stopwatch::new(if de { "Lauf" } else { "Run" });
                s.start(now - 754_300);
                s.laps = vec![241_800, 489_100];
                store::save_stopwatch(conn, &mut s)?;
                for (label, zone, analog) in [("", "America/New_York", false), ("", "Asia/Tokyo", true), ("", "Europe/London", false), (if de { "Team Bangalore" } else { "Team Bangalore" }, "Asia/Kolkata", false)] {
                    let mut c = WorldClock::new(label, zone);
                    c.analog = analog;
                    store::save_clock(conn, &mut c)?;
                }
                store::save_birthday(conn, &mut Birthday::new("Anna", 1, 20, Some(1990)))?;
                store::save_birthday(conn, &mut Birthday::new("Ben", 2, 29, Some(2000)))?;
                store::save_birthday(conn, &mut Birthday::new("Clara", 7, 3, None))?;
                Ok(())
            });
        }
        self.loaded = false;
        self.ensure_loaded(Some(conn));
        match view {
            "time-timers" => {
                self.sub = Sub::Timers;
                self.sel_timer = self.data.timers.first().map(|t| t.id);
            }
            "time-clocks" => {
                self.sub = Sub::Clocks;
                self.sel_clock = self.data.clocks.iter().find(|c| c.analog).map(|c| c.id);
            }
            _ => {
                self.sub = Sub::Alarms;
                self.sel_alarm = self.data.alarms.first().map(|a| a.id);
            }
        }
    }
}
