//! Dialog „Termin“ (#164): Titel, ganztägig, Beginn, Dauer bzw. Tage, Farbe, Erinnerung, Notiz und Wiederholung – ein
//! Baukasten (täglich, werktags, wöchentlich, monatlich am n-ten Tag oder Wochentag, jährlich, alle X Minuten/Stunden),
//! der einen Cron-Ausdruck erzeugt; Fachleute schreiben den Ausdruck direkt („Eigener Ausdruck“). Darunter Ende (nie,
//! bis, nach N Mal), Ausnahmen und die Vorschau der nächsten 5 Termine mit der Warnung zum ODER-Fall.

use super::CalEnv;
use crate::calendar::builder::{self, Pattern, HOUR_STEPS, MINUTE_STEPS};
use crate::calendar::cron::Cron;
use crate::calendar::store::{self, EventInput};
use crate::calendar::{self as cal, Event, Recurrence};
use crate::palette::Area;
use crate::state::Lang;
use crate::style;
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use eframe::egui::{self, RichText, Ui};

/// Art der Wiederholung im Baukasten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RKind {
    None,
    Daily,
    Weekdays,
    Weekly,
    MonthlyDay,
    MonthlyNth,
    Yearly,
    EveryMinutes,
    EveryHours,
    Custom,
}

impl RKind {
    const ALL: [RKind; 10] = [RKind::None, RKind::Daily, RKind::Weekdays, RKind::Weekly, RKind::MonthlyDay, RKind::MonthlyNth, RKind::Yearly, RKind::EveryMinutes, RKind::EveryHours, RKind::Custom];

    fn label(self, lang: Lang) -> &'static str {
        match self {
            RKind::None => lang.t("Keine Wiederholung", "Does not repeat"),
            RKind::Daily => lang.t("Täglich", "Daily"),
            RKind::Weekdays => lang.t("Werktags (Mo–Fr)", "Weekdays (Mon–Fri)"),
            RKind::Weekly => lang.t("Wöchentlich an …", "Weekly on …"),
            RKind::MonthlyDay => lang.t("Monatlich am Tag …", "Monthly on day …"),
            RKind::MonthlyNth => lang.t("Monatlich am n-ten Wochentag", "Monthly on the n-th weekday"),
            RKind::Yearly => lang.t("Jährlich", "Yearly"),
            RKind::EveryMinutes => lang.t("Alle X Minuten", "Every X minutes"),
            RKind::EveryHours => lang.t("Alle X Stunden", "Every X hours"),
            RKind::Custom => lang.t("Eigener Ausdruck (Cron)", "Custom expression (cron)"),
        }
    }
}

/// Ende der Wiederholung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndKind {
    Never,
    Until,
    Count,
}

/// Auswahl der Erinnerung (Minuten vorher).
pub(crate) const REMINDERS: [Option<i64>; 9] = [None, Some(0), Some(5), Some(10), Some(15), Some(30), Some(60), Some(120), Some(1440)];

pub(crate) fn reminder_label(m: Option<i64>, lang: Lang) -> String {
    match (m, lang) {
        (None, l) => l.t("Keine Erinnerung", "No reminder").into(),
        (Some(0), l) => l.t("Zu Beginn", "At the start").into(),
        (Some(m), Lang::De) if m % 1440 == 0 => format!("{} Tag(e) vorher", m / 1440),
        (Some(m), Lang::En) if m % 1440 == 0 => format!("{} day(s) before", m / 1440),
        (Some(m), Lang::De) if m % 60 == 0 => format!("{} Std. vorher", m / 60),
        (Some(m), Lang::En) if m % 60 == 0 => format!("{} h before", m / 60),
        (Some(m), Lang::De) => format!("{m} Min. vorher"),
        (Some(m), Lang::En) => format!("{m} min before"),
    }
}

/// Ergebnis eines Bildes des Dialogs.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Outcome {
    Open,
    Cancel,
    Saved(Event),
}

/// Formular eines Termins.
#[derive(Debug, Clone)]
pub(crate) struct EventForm {
    pub id: Option<i64>,
    pub title: String,
    pub note: String,
    pub color: String,
    pub all_day: bool,
    pub date: String,
    pub time: String,
    pub minutes: i64,
    pub days: i64,
    pub reminder: Option<i64>,
    pub project_id: Option<i64>,
    pub rkind: RKind,
    pub weekly: u8,
    pub month_day: u32,
    pub nth: u8,
    pub nth_wd: u8,
    pub step_min: u32,
    pub step_h: u32,
    pub cron: String,
    pub end: EndKind,
    pub until: String,
    pub count: u32,
    pub except: Vec<NaiveDate>,
    pub except_add: String,
    pub error: Option<String>,
    /// Fokus beim ersten Bild in den Titel.
    focus: bool,
}

impl EventForm {
    /// Neuer Termin ab `t` (eine Stunde, keine Wiederholung).
    pub(crate) fn new_at(t: NaiveDateTime, all_day: bool) -> EventForm {
        let wd = t.weekday().num_days_from_sunday() as u8;
        EventForm {
            id: None,
            title: String::new(),
            note: String::new(),
            color: String::new(),
            all_day,
            date: t.format("%Y-%m-%d").to_string(),
            time: t.format("%H:%M").to_string(),
            minutes: 60,
            days: 1,
            reminder: None,
            project_id: None,
            rkind: RKind::None,
            weekly: 1 << wd,
            month_day: t.day(),
            nth: ((t.day() - 1) / 7 + 1) as u8,
            nth_wd: wd,
            step_min: 15,
            step_h: 2,
            cron: String::new(),
            end: EndKind::Never,
            until: String::new(),
            count: 10,
            except: Vec::new(),
            except_add: String::new(),
            error: None,
            focus: true,
        }
    }

    /// Formular eines vorhandenen Termins (der Baukasten erkennt eigene Ausdrücke wieder).
    pub(crate) fn from_event(e: &Event) -> EventForm {
        let mut f = EventForm::new_at(e.start, e.all_day);
        f.id = Some(e.id);
        f.title = e.title.clone();
        f.note = e.note.clone();
        f.color = e.color.clone();
        f.minutes = if e.all_day { 60 } else { e.minutes };
        f.days = e.days();
        f.reminder = e.reminder;
        f.project_id = e.project_id;
        f.focus = false;
        if let Some(r) = &e.recur {
            f.cron = r.cron.clone();
            f.rkind = RKind::Custom;
            if let Ok(c) = Cron::parse(&r.cron)
                && let Some((p, h, m)) = builder::recognise(&c) {
                    let same_time = matches!(p, Pattern::EveryMinutes(_)) || (matches!(p, Pattern::EveryHours(_)) && m == e.start.minute()) || (h == e.start.hour() && m == e.start.minute()) || e.all_day;
                    if same_time {
                        f.set_pattern(p);
                    }
                }
            if let Some(u) = r.until {
                f.end = EndKind::Until;
                f.until = u.format("%Y-%m-%d").to_string();
            } else if let Some(n) = r.count {
                f.end = EndKind::Count;
                f.count = n;
            }
            f.except = r.except.clone();
        }
        f
    }

    fn set_pattern(&mut self, p: Pattern) {
        match p {
            Pattern::Daily => self.rkind = RKind::Daily,
            Pattern::Weekdays => self.rkind = RKind::Weekdays,
            Pattern::Weekly(b) => {
                self.rkind = RKind::Weekly;
                self.weekly = b;
            }
            Pattern::MonthlyDay(d) => {
                self.rkind = RKind::MonthlyDay;
                self.month_day = d;
            }
            Pattern::MonthlyNth(n, wd) => {
                self.rkind = RKind::MonthlyNth;
                self.nth = n;
                self.nth_wd = wd;
            }
            Pattern::Yearly { .. } => self.rkind = RKind::Yearly,
            Pattern::EveryMinutes(x) => {
                self.rkind = RKind::EveryMinutes;
                self.step_min = x;
            }
            Pattern::EveryHours(x) => {
                self.rkind = RKind::EveryHours;
                self.step_h = x;
            }
        }
    }

    /// Beginn aus den Feldern.
    pub(crate) fn start(&self) -> Option<NaiveDateTime> {
        let d = cal::parse_date(&self.date)?;
        if self.all_day {
            return Some(d.and_time(NaiveTime::MIN));
        }
        let t = NaiveTime::parse_from_str(self.time.trim(), "%H:%M").ok()?;
        Some(d.and_time(t))
    }

    /// Muster des Baukastens aus den Feldern (`None` bei „keine“ oder „eigener Ausdruck“).
    fn pattern(&self) -> Option<Pattern> {
        let s = self.start()?;
        Some(match self.rkind {
            RKind::None | RKind::Custom => return None,
            RKind::Daily => Pattern::Daily,
            RKind::Weekdays => Pattern::Weekdays,
            RKind::Weekly => Pattern::Weekly(self.weekly),
            RKind::MonthlyDay => Pattern::MonthlyDay(self.month_day),
            RKind::MonthlyNth => Pattern::MonthlyNth(self.nth, self.nth_wd),
            RKind::Yearly => Pattern::Yearly { month: s.month(), day: s.day() },
            RKind::EveryMinutes => Pattern::EveryMinutes(self.step_min),
            RKind::EveryHours => Pattern::EveryHours(self.step_h),
        })
    }

    /// Den Ausdruck aus dem Baukasten neu setzen (außer bei „eigener Ausdruck“).
    pub(crate) fn sync_cron(&mut self) {
        if let (Some(p), Some(s)) = (self.pattern(), self.start()) {
            let (h, m) = if self.all_day { (0, 0) } else { (s.hour(), s.minute()) };
            if let Ok(c) = builder::to_cron(p, h, m) {
                self.cron = c;
            }
        }
        if self.rkind == RKind::None {
            self.cron.clear();
        }
    }

    /// Eingaben prüfen und als `EventInput` liefern.
    pub(crate) fn input(&self, lang: Lang) -> Result<EventInput, String> {
        let start = self.start().ok_or_else(|| lang.t("Datum (JJJJ-MM-TT) oder Uhrzeit (HH:MM) ist ungültig.", "The date (YYYY-MM-DD) or time (HH:MM) is invalid.").to_string())?;
        let mut i = EventInput::new(&self.title, start, if self.all_day { self.days.max(1) * 1440 } else { self.minutes });
        i.all_day = self.all_day;
        i.note = self.note.clone();
        i.color = self.color.clone();
        i.reminder = self.reminder;
        i.project_id = self.project_id;
        if self.rkind != RKind::None {
            if let Some(p) = self.pattern() {
                let (h, m) = if self.all_day { (0, 0) } else { (start.hour(), start.minute()) };
                builder::to_cron(p, h, m).map_err(|e| lang.t(e.de, e.en).to_string())?;
            }
            let mut r = Recurrence { cron: self.cron.trim().to_string(), ..Default::default() };
            Cron::parse(&r.cron).map_err(|e| e.message(lang).to_string())?;
            match self.end {
                EndKind::Never => {}
                EndKind::Until => r.until = Some(cal::parse_date(&self.until).ok_or_else(|| lang.t("Das Ende (JJJJ-MM-TT) ist ungültig.", "The end date (YYYY-MM-DD) is invalid.").to_string())?),
                EndKind::Count => r.count = Some(self.count),
            }
            r.except = self.except.clone();
            i.recur = Some(r);
        }
        i.validate().map_err(|e| e.message(lang))
    }

    /// Vorschau: die nächsten 5 Termine ab dem Beginn (mit Ende und Ausnahmen).
    pub(crate) fn preview(&self, env: &CalEnv) -> Result<Vec<NaiveDateTime>, String> {
        let mut f = self.clone();
        if f.title.trim().is_empty() {
            f.title = "…".into();
        }
        let i = f.input(env.lang)?;
        let e = Event {
            id: 0,
            uid: String::new(),
            title: i.title,
            note: String::new(),
            color: String::new(),
            all_day: i.all_day,
            start: i.start,
            minutes: i.minutes,
            recur: i.recur,
            reminder: None,
            project_id: None,
            origin: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        Ok(e.next_occurrences(e.start, 5, env.zone))
    }

    /// Speichern (anlegen oder ändern).
    pub(crate) fn save(&mut self, env: &CalEnv) -> Option<Event> {
        let conn = env.conn?;
        let input = match self.input(env.lang) {
            Ok(i) => i,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        let r = match self.id {
            Some(id) => store::update_event(conn, id, input),
            None => store::create_event(conn, input, None, "local"),
        };
        match r {
            Ok(e) => Some(e),
            Err(e) => {
                self.error = Some(e.message(env.lang));
                None
            }
        }
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context, env: &CalEnv) -> Outcome {
        let lang = env.lang;
        let mut out = Outcome::Open;
        let m = style::modal(egui::Id::new("cal_event_form")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(if self.id.is_some() { lang.t("Termin bearbeiten", "Edit event") } else { lang.t("Neuer Termin", "New event") }).strong().size(15.0));
            ui.add_space(style::S1);
            let before = (self.rkind, self.all_day, self.date.clone(), self.time.clone(), self.weekly, self.month_day, self.nth, self.nth_wd, self.step_min, self.step_h);
            egui::Grid::new("cal_form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(lang.t("Titel", "Title"));
                let r = ui.add(egui::TextEdit::singleline(&mut self.title).id(egui::Id::new("cal_form_title")).desired_width(320.0).hint_text(lang.t("z. B. Standup", "e.g. Standup")));
                if std::mem::take(&mut self.focus) {
                    r.request_focus();
                }
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.all_day, lang.t("Ganztägig", "All day"));
                ui.end_row();
                ui.label(lang.t("Beginn", "Start"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.date).id(egui::Id::new("cal_form_date")).desired_width(100.0).hint_text("YYYY-MM-DD"));
                    if !self.all_day {
                        ui.add(egui::TextEdit::singleline(&mut self.time).id(egui::Id::new("cal_form_time")).desired_width(52.0).hint_text("HH:MM"));
                    }
                });
                ui.end_row();
                if self.all_day {
                    ui.label(lang.t("Tage", "Days"));
                    ui.add(egui::DragValue::new(&mut self.days).range(1..=366));
                } else {
                    ui.label(lang.t("Dauer", "Duration"));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.minutes).range(0..=cal::MAX_MINUTES).speed(5.0).suffix(lang.t(" Min.", " min")));
                        for (m, l) in [(15, "15m"), (30, "30m"), (60, "1h"), (120, "2h")] {
                            if ui.small_button(l).clicked() {
                                self.minutes = m;
                            }
                        }
                    });
                }
                ui.end_row();
                ui.label(lang.t("Erinnerung", "Reminder"));
                egui::ComboBox::from_id_salt("cal_form_rem").selected_text(reminder_label(self.reminder, lang)).show_ui(ui, |ui| {
                    for r in REMINDERS {
                        ui.selectable_value(&mut self.reminder, r, reminder_label(r, lang));
                    }
                });
                ui.end_row();
                ui.label(lang.t("Farbe", "Colour"));
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.color.is_empty(), lang.t("Standard", "Default")).clicked() {
                        self.color.clear();
                    }
                    ui.menu_button(lang.t("Farbe wählen …", "Pick colour …"), |ui| {
                        if let Some(c) = crate::app::palette_ui::pick(ui, "cal_form_color", Area::Expert, Some(&self.color), crate::app::palette_ui::PickOpts::TODO, lang) {
                            self.color = c;
                            ui.close();
                        }
                    });
                    if !self.color.is_empty() {
                        let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(14.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 6.0, crate::app::palette_ui::color32(&self.color));
                    }
                });
                ui.end_row();
                ui.label(lang.t("Notiz", "Note"));
                ui.add(egui::TextEdit::multiline(&mut self.note).id(egui::Id::new("cal_form_note")).desired_rows(3).desired_width(320.0));
                ui.end_row();
            });
            ui.separator();
            self.recurrence_ui(ui, env);
            let after = (self.rkind, self.all_day, self.date.clone(), self.time.clone(), self.weekly, self.month_day, self.nth, self.nth_wd, self.step_min, self.step_h);
            if before != after || (self.cron.is_empty() && !matches!(self.rkind, RKind::None | RKind::Custom)) {
                self.sync_cron();
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(style::danger_soft()));
            }
            ui.add_space(style::S2);
            ui.horizontal(|ui| {
                let r = ui.add_enabled(!self.title.trim().is_empty(), egui::Button::new(lang.t("Speichern", "Save")));
                if (r.clicked() || (ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl) && !self.title.trim().is_empty()))
                    && let Some(e) = self.save(env) {
                        out = Outcome::Saved(e);
                    }
                if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                    out = Outcome::Cancel;
                }
            });
        });
        if matches!(out, Outcome::Open) && m.should_close() {
            out = Outcome::Cancel;
        }
        out
    }

    fn recurrence_ui(&mut self, ui: &mut Ui, env: &CalEnv) {
        let lang = env.lang;
        ui.horizontal(|ui| {
            ui.label(lang.t("Wiederholung", "Repeat"));
            egui::ComboBox::from_id_salt("cal_form_rkind").width(240.0).selected_text(self.rkind.label(lang)).show_ui(ui, |ui| {
                for k in RKind::ALL {
                    ui.selectable_value(&mut self.rkind, k, k.label(lang));
                }
            });
        });
        if self.rkind == RKind::None {
            return;
        }
        match self.rkind {
            RKind::Weekly => {
                ui.horizontal(|ui| {
                    for d in [1u8, 2, 3, 4, 5, 6, 0] {
                        let mut on = self.weekly & (1 << d) != 0;
                        if ui.checkbox(&mut on, builder::weekday_short(d, lang)).changed() {
                            if on { self.weekly |= 1 << d } else { self.weekly &= !(1 << d) }
                        }
                    }
                });
            }
            RKind::MonthlyDay => {
                ui.horizontal(|ui| {
                    ui.label(lang.t("Tag", "Day"));
                    ui.add(egui::DragValue::new(&mut self.month_day).range(1..=31));
                });
            }
            RKind::MonthlyNth => {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("cal_form_nth").selected_text(format!("{}.", self.nth)).show_ui(ui, |ui| {
                        for n in 1..=5u8 {
                            ui.selectable_value(&mut self.nth, n, format!("{n}."));
                        }
                    });
                    egui::ComboBox::from_id_salt("cal_form_nth_wd").selected_text(builder::weekday_name(self.nth_wd, lang)).show_ui(ui, |ui| {
                        for d in [1u8, 2, 3, 4, 5, 6, 0] {
                            ui.selectable_value(&mut self.nth_wd, d, builder::weekday_name(d, lang));
                        }
                    });
                });
            }
            RKind::EveryMinutes => {
                ui.horizontal(|ui| {
                    ui.label(lang.t("Alle", "Every"));
                    egui::ComboBox::from_id_salt("cal_form_stepm").selected_text(self.step_min.to_string()).show_ui(ui, |ui| {
                        for s in MINUTE_STEPS {
                            ui.selectable_value(&mut self.step_min, s, s.to_string());
                        }
                    });
                    ui.label(lang.t("Minuten (zur vollen Stunde ausgerichtet)", "minutes (aligned to the full hour)"));
                });
            }
            RKind::EveryHours => {
                ui.horizontal(|ui| {
                    ui.label(lang.t("Alle", "Every"));
                    egui::ComboBox::from_id_salt("cal_form_steph").selected_text(self.step_h.to_string()).show_ui(ui, |ui| {
                        for s in HOUR_STEPS {
                            ui.selectable_value(&mut self.step_h, s, s.to_string());
                        }
                    });
                    ui.label(lang.t("Stunden (ab Mitternacht, zur Minute des Beginns)", "hours (from midnight, at the start's minute)"));
                });
            }
            _ => {}
        }
        if let Some(h) = self.pattern().and_then(|p| builder::hint(p, lang)) {
            ui.label(RichText::new(h).small().color(style::dim(ui)));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang.t("Cron", "Cron")).small());
            let r = ui.add(egui::TextEdit::singleline(&mut self.cron).id(egui::Id::new("cal_form_cron")).font(egui::TextStyle::Monospace).desired_width(220.0));
            if r.changed() {
                // eigener Ausdruck: der Baukasten hält still
                self.rkind = RKind::Custom;
            }
            ui.label(RichText::new("min h dom mon dow · @daily · MON#1").small().color(style::dim(ui)));
        });
        ui.horizontal(|ui| {
            ui.label(lang.t("Ende", "Ends"));
            ui.radio_value(&mut self.end, EndKind::Never, lang.t("nie", "never"));
            ui.radio_value(&mut self.end, EndKind::Until, lang.t("am", "on"));
            if self.end == EndKind::Until {
                ui.add(egui::TextEdit::singleline(&mut self.until).id(egui::Id::new("cal_form_until")).desired_width(90.0).hint_text("YYYY-MM-DD"));
            }
            ui.radio_value(&mut self.end, EndKind::Count, lang.t("nach", "after"));
            if self.end == EndKind::Count {
                ui.add(egui::DragValue::new(&mut self.count).range(1..=cal::MAX_COUNT));
                ui.label(lang.t("Mal", "times"));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(lang.t("Ausnahmen", "Exceptions"));
            let mut remove = None;
            for (i, d) in self.except.iter().enumerate() {
                if ui.small_button(format!("{} ✕", d.format("%Y-%m-%d"))).on_hover_text(lang.t("Ausnahme entfernen", "Remove the exception")).clicked() {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                self.except.remove(i);
            }
            ui.add(egui::TextEdit::singleline(&mut self.except_add).id(egui::Id::new("cal_form_exc")).desired_width(90.0).hint_text("YYYY-MM-DD"));
            if ui.small_button(lang.t("+ Tag auslassen", "+ Skip day")).clicked() {
                match cal::parse_date(&self.except_add) {
                    Some(d) => {
                        if !self.except.contains(&d) {
                            self.except.push(d);
                            self.except.sort();
                        }
                        self.except_add.clear();
                    }
                    None => self.error = Some(lang.t("Ausnahme: Datum JJJJ-MM-TT", "Exception: date YYYY-MM-DD").into()),
                }
            }
        });
        // Vorschau und Warnung
        match Cron::parse(&self.cron) {
            Ok(c) => {
                ui.label(RichText::new(format!("↻ {}", builder::describe(&c, lang))).small());
                if let Some(w) = c.warning(lang) {
                    ui.label(RichText::new(format!("⚠ {w}")).small().color(style::danger_soft()));
                }
                match self.preview(env) {
                    Ok(next) if next.is_empty() => {
                        ui.label(RichText::new(lang.t("Keine Termine (Ausdruck passt nie oder Ende liegt davor).", "No occurrences (the expression never matches or the end is earlier).")).small().color(style::danger_soft()));
                    }
                    Ok(next) => {
                        ui.label(RichText::new(lang.t("Nächste 5:", "Next 5:")).small().color(style::dim(ui)));
                        for n in next {
                            ui.label(RichText::new(format!("  {}", super::fmt_dt(n, self.all_day, lang))).small().monospace());
                        }
                    }
                    Err(_) => {}
                }
            }
            Err(e) => {
                ui.label(RichText::new(e.message(lang)).small().color(style::danger_soft()));
            }
        }
    }
}
