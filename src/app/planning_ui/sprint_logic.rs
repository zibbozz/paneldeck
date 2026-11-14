//! Reine Logik der Backlog-/Sprint-Oberfläche (ohne egui-Zustand, gut testbar): Mehrfachauswahl, Drop-Ziele beim Ziehen,
//! Datums- und Dauer-Eingabe der Sprint-Formulare, Kalender-Einstellungen, Ergebnis-Zusammenfassung beim Abschließen.
//! Die Datenbank-Aufrufe der Oberfläche stehen in `sprint_ops.rs`, das Zeichnen in `sprint_view.rs`.

use crate::calc::calendar::WorkCalendar;
use crate::calc::progress::Basis;
use crate::planning::sprints::sprint::{end_from_duration, Leftover};
use crate::planning::sprints::{Figure, SprintError, DATE_FORMAT, DEFAULT_SPRINT_DAYS, MAX_SPRINT_DAYS};
use crate::state::Lang;
use chrono::NaiveDate;

// ------------------------------------------------------------------ Bereiche und Drop-Ziele

/// Ein Bereich der Backlog-Seite, in den Tickets fallen können.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Backlog,
    Sprint(i64),
}

/// Lage eines Bereichs im Bild (in Pixeln; nur die senkrechte Reihenfolge zählt, die Breite prüft der Aufrufer).
#[derive(Debug, Clone, PartialEq)]
pub struct SectionGeom {
    pub section: Section,
    /// Oberkante und Unterkante des ganzen Bereichs (mit Kopf).
    pub top: f32,
    pub bottom: f32,
    /// Oberkante des Zeilenbereichs (für die Einfügemarke bei leerem Bereich).
    pub body_top: f32,
    /// Oberkante und Unterkante jeder gezeigten Zeile (ohne die gerade gezogenen), von oben nach unten.
    pub rows: Vec<(f32, f32)>,
    /// Zugeklappt: Fallenlassen hängt ans Ende an.
    pub collapsed: bool,
}

/// Wohin ein Ziehen gerade zeigt: Bereich und Einfüge-Index unter den gezeigten Zeilen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drop {
    pub section: Section,
    pub idx: usize,
}

/// Findet den Bereich unter dem Mauszeiger (`y`) und die Einfüge-Stelle: Zahl der Zeilen, deren Mitte über dem Zeiger liegt.
/// Zugeklappte Bereiche liefern `usize::MAX` (= ans Ende).
pub fn locate_drop(geoms: &[SectionGeom], y: f32) -> Option<Drop> {
    let g = geoms.iter().find(|g| y >= g.top && y <= g.bottom)?;
    let idx = if g.collapsed { usize::MAX } else { g.rows.iter().filter(|(t, b)| (t + b) / 2.0 < y).count() };
    Some(Drop { section: g.section, idx })
}

/// Höhe der Einfügemarke für `drop` im Bereich `g`.
pub fn insert_line_y(g: &SectionGeom, idx: usize) -> f32 {
    if g.rows.is_empty() {
        return g.body_top + 3.0;
    }
    let idx = idx.min(g.rows.len());
    if idx == 0 { g.rows[0].0 - 1.0 } else { g.rows[idx - 1].1 + 1.0 }
}

/// Wie ein Fallenlassen in der Datenbank ankommt: Zielbereich und das Ticket, vor das die gezogenen gehören (`None` = ans Ende).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropPlan {
    pub target: Section,
    pub before: Option<i64>,
    /// Die gezogenen Tickets in der Reihenfolge, in der sie landen sollen.
    pub ids: Vec<i64>,
}

/// Plan für `drop`: `section_ids` sind **alle** Tickets des Zielbereichs in Reihenfolge (auch die noch nicht gezeigten, auch die
/// gezogenen). `before` ist das Ticket an der Einfüge-Stelle, ohne die gezogenen mitzuzählen.
pub fn plan_drop(drop: Drop, section_ids: &[i64], dragged: &[i64]) -> DropPlan {
    let before = section_ids.iter().copied().filter(|i| !dragged.contains(i)).nth(drop.idx);
    DropPlan { target: drop.section, before, ids: dragged.to_vec() }
}

// ------------------------------------------------------------------ Mehrfachauswahl

/// Ausgewählte Tickets (Strg = einzeln dazu/weg, Umschalt = Bereich seit dem Anker).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    ids: Vec<i64>,
    anchor: Option<i64>,
}

impl Selection {
    pub fn contains(&self, id: i64) -> bool {
        self.ids.contains(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// Klick auf ein Ticket. `order` sind alle sichtbaren Tickets in Anzeigereihenfolge (über alle Bereiche).
    pub fn click(&mut self, id: i64, ctrl: bool, shift: bool, order: &[i64]) {
        let anchor_pos = self.anchor.and_then(|a| order.iter().position(|x| *x == a));
        let pos = order.iter().position(|x| *x == id);
        match (shift, anchor_pos, pos) {
            (true, Some(a), Some(p)) => {
                let (lo, hi) = (a.min(p), a.max(p));
                let range = &order[lo..=hi];
                if ctrl {
                    for r in range {
                        if !self.ids.contains(r) {
                            self.ids.push(*r);
                        }
                    }
                } else {
                    self.ids = range.to_vec();
                }
                // Der Anker bleibt, damit sich der Bereich mit weiteren Umschalt-Klicks verändern lässt.
            }
            _ if ctrl => {
                if let Some(i) = self.ids.iter().position(|x| *x == id) {
                    self.ids.remove(i);
                } else {
                    self.ids.push(id);
                }
                self.anchor = Some(id);
            }
            _ => {
                self.ids = vec![id];
                self.anchor = Some(id);
            }
        }
    }

    /// Entfernt Tickets, die nicht mehr sichtbar sind (nach dem Neuladen).
    pub fn retain_visible(&mut self, order: &[i64]) {
        self.ids.retain(|i| order.contains(i));
        if self.anchor.is_some_and(|a| !order.contains(&a)) {
            self.anchor = None;
        }
    }

    /// Die ausgewählten Tickets in Anzeigereihenfolge.
    pub fn in_order(&self, order: &[i64]) -> Vec<i64> {
        order.iter().copied().filter(|i| self.ids.contains(i)).collect()
    }

    /// Was beim Anfassen von `grabbed` gezogen wird: die ganze Auswahl (in Anzeigereihenfolge), wenn das Ticket dazugehört,
    /// sonst nur das Ticket. Ein Ticket außerhalb der Auswahl ersetzt die Auswahl.
    pub fn drag_set(&self, grabbed: i64, order: &[i64]) -> Vec<i64> {
        if self.contains(grabbed) { self.in_order(order) } else { vec![grabbed] }
    }

    /// Worauf eine Aktion des Kontextmenüs wirkt (wie [`Selection::drag_set`]).
    pub fn targets(&self, id: i64, order: &[i64]) -> Vec<i64> {
        self.drag_set(id, order)
    }
}

// ------------------------------------------------------------------ Datum und Dauer

/// Datum so, wie es in den Formularen steht: Deutsch `TT.MM.JJJJ`, Englisch `JJJJ-MM-TT`.
pub fn fmt_date_ui(d: NaiveDate, lang: Lang) -> String {
    match lang {
        Lang::De => d.format("%d.%m.%Y").to_string(),
        Lang::En => d.format(DATE_FORMAT).to_string(),
    }
}

/// Datum aus einem Eingabefeld: `JJJJ-MM-TT` oder `TT.MM.JJJJ` (auch `T.M.JJJJ`). Leerraum außen wird ignoriert.
pub fn parse_date_input(s: &str) -> Result<NaiveDate, SprintError> {
    let t = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(t, DATE_FORMAT) {
        return Ok(d);
    }
    let parts: Vec<&str> = t.split('.').collect();
    if let [d, m, y] = parts[..] {
        if let (Ok(d), Ok(m), Ok(y)) = (d.trim().parse::<u32>(), m.trim().parse::<u32>(), y.trim().parse::<i32>()) {
            if (1000..=9999).contains(&y) {
                if let Some(date) = NaiveDate::from_ymd_opt(y, m, d) {
                    return Ok(date);
                }
            }
        }
    }
    Err(SprintError::DateInvalid)
}

/// Optionales Datum: leeres Feld = kein Datum.
pub fn parse_opt_date(s: &str) -> Result<Option<NaiveDate>, SprintError> {
    if s.trim().is_empty() { Ok(None) } else { parse_date_input(s).map(Some) }
}

/// Dauer in Arbeitstagen aus einem Eingabefeld: leer = Standard, sonst 1 bis [`MAX_SPRINT_DAYS`].
pub fn parse_days_input(s: &str) -> Result<u32, SprintError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(DEFAULT_SPRINT_DAYS);
    }
    match t.parse::<u32>() {
        Ok(n) if (1..=MAX_SPRINT_DAYS).contains(&n) => Ok(n),
        _ => Err(SprintError::DaysInvalid { max: MAX_SPRINT_DAYS }),
    }
}

/// Eingabefelder des Sprint-Formulars (Texte, wie sie im Dialog stehen).
#[derive(Debug, Clone, PartialEq)]
pub struct SprintFields {
    pub name: String,
    pub goal: String,
    pub start: String,
    pub end: String,
    pub days: String,
    /// Der Nutzer hat das Ende selbst gesetzt: Start/Dauer rechnen es nicht mehr neu.
    pub end_manual: bool,
}

/// Die geprüften Werte des Formulars.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSprint {
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
    pub days: u32,
}

impl SprintFields {
    pub fn new(name: &str) -> Self {
        SprintFields { name: name.to_string(), goal: String::new(), start: String::new(), end: String::new(), days: DEFAULT_SPRINT_DAYS.to_string(), end_manual: false }
    }

    /// Start oder Dauer wurden geändert: das Ende folgt der Dauer-Vorlage, solange es nicht von Hand gesetzt ist.
    pub fn recompute_end(&mut self, cal: &WorkCalendar, lang: Lang) {
        if self.end_manual {
            return;
        }
        let (Ok(Some(start)), Ok(days)) = (parse_opt_date(&self.start), parse_days_input(&self.days)) else { return };
        if let Ok(end) = end_from_duration(start, days, cal) {
            self.end = fmt_date_ui(end, lang);
        }
    }

    /// Das Ende wurde von Hand geändert: Dauer = Arbeitstage zwischen Start und Ende (mindestens 1). Wird das Ende geleert, folgt es
    /// wieder der Dauer.
    pub fn after_end_edit(&mut self, cal: &WorkCalendar, lang: Lang) {
        if self.end.trim().is_empty() {
            self.end_manual = false;
            self.recompute_end(cal, lang);
            return;
        }
        self.end_manual = true;
        if let (Ok(Some(s)), Ok(Some(e))) = (parse_opt_date(&self.start), parse_opt_date(&self.end)) {
            let n = cal.work_days_inclusive(s, e);
            if (1..=MAX_SPRINT_DAYS).contains(&n) {
                self.days = n.to_string();
            }
        }
    }

    /// Arbeitstage zwischen Start und Ende (für die Anzeige neben dem Enddatum); `None`, wenn eines fehlt oder ungültig ist.
    pub fn work_days(&self, cal: &WorkCalendar) -> Option<u32> {
        let (Ok(Some(s)), Ok(Some(e))) = (parse_opt_date(&self.start), parse_opt_date(&self.end)) else { return None };
        (e >= s).then(|| cal.work_days_inclusive(s, e))
    }

    /// Prüft die Eingaben (die Datenschicht prüft Reihenfolge, Namen und Längen noch einmal).
    pub fn parse(&self) -> Result<ParsedSprint, SprintError> {
        let start = parse_opt_date(&self.start)?;
        let end = parse_opt_date(&self.end)?;
        let days = parse_days_input(&self.days)?;
        if end.is_some() && start.is_none() {
            return Err(SprintError::DatesIncomplete);
        }
        Ok(ParsedSprint { start, end, days })
    }
}

// ------------------------------------------------------------------ Kalender des Projekts

/// Arbeitskalender des Projekts für Sprints: Wochenenden ausschließen und Feiertage.
#[derive(Debug, Clone, PartialEq)]
pub struct CalSettings {
    pub exclude_weekends: bool,
    pub holidays: Vec<NaiveDate>,
}

impl Default for CalSettings {
    fn default() -> Self {
        CalSettings { exclude_weekends: true, holidays: Vec::new() }
    }
}

impl CalSettings {
    pub fn calendar(&self) -> WorkCalendar {
        WorkCalendar::with_weekend_switch(self.exclude_weekends).with_holidays(self.holidays.clone())
    }

    /// Schlüssel der Einstellungen in der `meta`-Tabelle.
    pub fn meta_keys(project: i64) -> (String, String) {
        (format!("sprint_weekends_{project}"), format!("sprint_holidays_{project}"))
    }

    /// Aus den gespeicherten Texten (fehlt etwas: Standard; kaputte Daten werden ignoriert).
    pub fn from_meta(weekends: Option<&str>, holidays: Option<&str>) -> Self {
        let exclude_weekends = weekends.map(|w| w != "0").unwrap_or(true);
        let holidays = holidays.map(|h| parse_holidays(h).unwrap_or_default()).unwrap_or_default();
        CalSettings { exclude_weekends, holidays }
    }

    /// Feiertage als Text zum Speichern (`JJJJ-MM-TT`, durch Komma getrennt).
    pub fn holidays_text(&self) -> String {
        self.holidays.iter().map(|d| d.format(DATE_FORMAT).to_string()).collect::<Vec<_>>().join(",")
    }
}

/// Feiertage aus einem Textfeld: Daten durch Komma, Semikolon, Leerzeichen oder Zeilenumbruch getrennt; sortiert, ohne Doppelte.
/// Fehler: das erste unlesbare Stück.
pub fn parse_holidays(text: &str) -> Result<Vec<NaiveDate>, String> {
    let mut out: Vec<NaiveDate> = Vec::new();
    for tok in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        match parse_date_input(tok) {
            Ok(d) => out.push(d),
            Err(_) => return Err(tok.to_string()),
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Feiertage zum Bearbeiten im Textfeld: je Zeile ein Datum in der Anzeigeform der Sprache.
pub fn holidays_edit_text(h: &[NaiveDate], lang: Lang) -> String {
    h.iter().map(|d| fmt_date_ui(*d, lang)).collect::<Vec<_>>().join("\n")
}

// ------------------------------------------------------------------ Abschließen

/// Auswahl im Dialog „Sprint abschließen“.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftoverChoice {
    Backlog,
    NextSprint,
}

/// Ziel der unerledigten Tickets aus der Auswahl; `None`, wenn „nächster Sprint“ gewählt, aber keiner vorhanden ist.
pub fn leftover_from(choice: LeftoverChoice, target: Option<i64>) -> Option<Leftover> {
    match choice {
        LeftoverChoice::Backlog => Some(Leftover::ToBacklog),
        LeftoverChoice::NextSprint => target.map(Leftover::ToSprint),
    }
}

/// Vorgabe für den Zielsprint: der geplante Sprint mit dem frühesten Start (ohne Start zuletzt), bei Gleichstand der ältere.
pub fn default_target(planned: &[(i64, Option<NaiveDate>)]) -> Option<i64> {
    planned.iter().min_by_key(|(id, start)| (start.is_none(), *start, *id)).map(|(id, _)| *id)
}

/// Satz für das Ergebnis des Abschließens.
pub fn complete_summary(done: usize, left: usize, leftover: Leftover, target_name: Option<&str>, lang: Lang) -> String {
    let dest = match (leftover, target_name) {
        (Leftover::ToSprint(_), Some(n)) => match lang {
            Lang::De => format!("in den Sprint {}", lang.q(n)),
            Lang::En => format!("to the sprint {}", lang.q(n)),
        },
        _ => lang.t("ins Backlog", "to the backlog").to_string(),
    };
    let done_t = tickets_text(done, lang);
    match (lang, left) {
        (Lang::De, 0) => format!("{done_t} erledigt – nichts blieb offen."),
        (Lang::En, 0) => format!("{done_t} done – nothing was left open."),
        (Lang::De, n) => format!("{done_t} erledigt; {} offen – verschoben {dest}.", tickets_text(n, lang)),
        (Lang::En, n) => format!("{done_t} done; {} open – moved {dest}.", tickets_text(n, lang)),
    }
}

// ------------------------------------------------------------------ Reihenfolge von Listen

/// Verschiebt das Element an `idx` um einen Platz (`dir` = -1 nach oben, +1 nach unten). `None`, wenn es am Rand steht.
pub fn move_in_list(ids: &[i64], idx: usize, dir: i32) -> Option<Vec<i64>> {
    let to = idx as i64 + dir as i64;
    if idx >= ids.len() || to < 0 || to as usize >= ids.len() {
        return None;
    }
    let mut v = ids.to_vec();
    v.swap(idx, to as usize);
    Some(v)
}

/// Text „3 Tickets“ / „1 ticket“.
pub fn tickets_text(n: usize, lang: Lang) -> String {
    match (lang, n) {
        (Lang::De, 1) => "1 Ticket".to_string(),
        (Lang::De, n) => format!("{n} Tickets"),
        (Lang::En, 1) => "1 ticket".to_string(),
        (Lang::En, n) => format!("{n} tickets"),
    }
}

/// Einheit der gewählten Zählweise für Achsen und Summen („P“ für Punkte, „Tickets“ für die Anzahl).
pub fn unit_label(basis: Basis, lang: Lang) -> &'static str {
    match basis {
        Basis::TicketCount => lang.t("Tickets", "tickets"),
        _ => lang.t("P", "pts"),
    }
}

/// Umfang einer Menge als Text: „5 Tickets · 21 P“ (die Punkte nur, wenn es welche gibt).
pub fn figure_text(f: &Figure, lang: Lang) -> String {
    let mut s = tickets_text(f.tickets as usize, lang);
    if f.points > 0.0 {
        s.push_str(&format!(" · {} P", points_text(f.points)));
    }
    s
}

/// Gewicht einer Menge nach der Zählweise: „34 P“ bzw. „12 Tickets“.
pub fn weight_text(f: &Figure, basis: Basis, lang: Lang) -> String {
    match basis {
        Basis::TicketCount => tickets_text(f.tickets as usize, lang),
        _ => format!("{} {}", points_text(f.weight(basis)), unit_label(basis, lang)),
    }
}

/// Anteil des Erledigten am Gesamtumfang (0 bis 1) nach der Zählweise; 0 bei leerem Umfang.
pub fn done_fraction(done: &Figure, total: &Figure, basis: Basis) -> f32 {
    let t = total.weight(basis);
    if t <= 0.0 { 0.0 } else { (done.weight(basis) / t).clamp(0.0, 1.0) as f32 }
}

/// Punkte ohne unnötige Nachkommastellen („13“, „2.5“).
pub fn points_text(x: f64) -> String {
    if x.fract() == 0.0 { format!("{}", x as i64) } else { format!("{x:.1}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn geoms() -> Vec<SectionGeom> {
        vec![
            SectionGeom { section: Section::Sprint(1), top: 0.0, bottom: 100.0, body_top: 30.0, rows: vec![(30.0, 50.0), (52.0, 72.0)], collapsed: false },
            SectionGeom { section: Section::Sprint(2), top: 110.0, bottom: 150.0, body_top: 140.0, rows: vec![], collapsed: false },
            SectionGeom { section: Section::Backlog, top: 160.0, bottom: 300.0, body_top: 190.0, rows: vec![(190.0, 210.0), (212.0, 232.0), (234.0, 254.0)], collapsed: false },
        ]
    }

    #[test]
    fn drop_target_is_the_section_under_the_pointer() {
        let g = geoms();
        assert_eq!(locate_drop(&g, 10.0), Some(Drop { section: Section::Sprint(1), idx: 0 }));
        assert_eq!(locate_drop(&g, 45.0), Some(Drop { section: Section::Sprint(1), idx: 1 }), "unter der Mitte der ersten Zeile");
        assert_eq!(locate_drop(&g, 70.0), Some(Drop { section: Section::Sprint(1), idx: 2 }));
        assert_eq!(locate_drop(&g, 90.0), Some(Drop { section: Section::Sprint(1), idx: 2 }), "unter allen Zeilen = ans Ende");
        assert_eq!(locate_drop(&g, 130.0), Some(Drop { section: Section::Sprint(2), idx: 0 }), "leerer Sprint");
        assert_eq!(locate_drop(&g, 205.0), Some(Drop { section: Section::Backlog, idx: 1 }));
        assert_eq!(locate_drop(&g, 105.0), None, "zwischen den Bereichen");
        assert_eq!(locate_drop(&g, 400.0), None);
        assert_eq!(locate_drop(&[], 5.0), None);
    }

    #[test]
    fn a_collapsed_section_takes_drops_at_the_end() {
        let mut g = geoms();
        g[0].collapsed = true;
        g[0].rows.clear();
        let d = locate_drop(&g, 20.0).unwrap();
        assert_eq!(d.idx, usize::MAX);
        let plan = plan_drop(d, &[5, 6, 7], &[9]);
        assert_eq!(plan.before, None);
    }

    #[test]
    fn the_insert_line_sits_between_rows() {
        let g = &geoms()[0];
        assert_eq!(insert_line_y(g, 0), 29.0);
        assert_eq!(insert_line_y(g, 1), 51.0);
        assert_eq!(insert_line_y(g, 2), 73.0);
        assert_eq!(insert_line_y(g, 99), 73.0, "zu großer Index = ans Ende");
        assert_eq!(insert_line_y(&geoms()[1], 0), 143.0, "leerer Bereich");
    }

    #[test]
    fn plan_places_the_dragged_tickets_before_the_right_neighbour() {
        let ids = [10, 11, 12, 13];
        let at = |idx| Drop { section: Section::Backlog, idx };
        assert_eq!(plan_drop(at(0), &ids, &[99]).before, Some(10));
        assert_eq!(plan_drop(at(2), &ids, &[99]).before, Some(12));
        assert_eq!(plan_drop(at(4), &ids, &[99]).before, None, "ans Ende");
        // Die gezogenen Tickets zählen nicht mit: 11 und 12 stecken im Bereich und werden vor dem Index herausgenommen.
        assert_eq!(plan_drop(at(1), &ids, &[11, 12]).before, Some(13));
        assert_eq!(plan_drop(at(0), &ids, &[10, 11]).before, Some(12));
        assert_eq!(plan_drop(at(2), &ids, &[10, 11]).before, None);
        let p = plan_drop(Drop { section: Section::Sprint(3), idx: 0 }, &[], &[1, 2]);
        assert_eq!((p.target, p.before, p.ids), (Section::Sprint(3), None, vec![1, 2]));
    }

    #[test]
    fn plain_click_selects_one_ticket() {
        let order = [1, 2, 3, 4, 5];
        let mut s = Selection::default();
        s.click(2, false, false, &order);
        s.click(4, false, false, &order);
        assert_eq!(s.in_order(&order), vec![4]);
        assert!(s.contains(4) && !s.contains(2));
    }

    #[test]
    fn ctrl_click_toggles() {
        let order = [1, 2, 3, 4, 5];
        let mut s = Selection::default();
        s.click(2, true, false, &order);
        s.click(5, true, false, &order);
        s.click(3, true, false, &order);
        assert_eq!(s.in_order(&order), vec![2, 3, 5]);
        s.click(3, true, false, &order);
        assert_eq!(s.in_order(&order), vec![2, 5]);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn shift_click_selects_a_range_from_the_anchor() {
        let order = [1, 2, 3, 4, 5, 6];
        let mut s = Selection::default();
        s.click(2, false, false, &order);
        s.click(5, false, true, &order);
        assert_eq!(s.in_order(&order), vec![2, 3, 4, 5]);
        s.click(3, false, true, &order);
        assert_eq!(s.in_order(&order), vec![2, 3], "der Anker bleibt, der Bereich schrumpft");
        s.click(1, false, true, &order);
        assert_eq!(s.in_order(&order), vec![1, 2]);
        // Strg+Umschalt fügt den Bereich hinzu
        let mut s = Selection::default();
        s.click(1, false, false, &order);
        s.click(6, true, false, &order);
        s.click(3, true, true, &order);
        assert_eq!(s.in_order(&order), vec![1, 3, 4, 5, 6]);
    }

    #[test]
    fn shift_without_anchor_selects_only_the_clicked_ticket() {
        let order = [1, 2, 3];
        let mut s = Selection::default();
        s.click(3, false, true, &order);
        assert_eq!(s.in_order(&order), vec![3]);
    }

    #[test]
    fn selection_drops_tickets_that_vanish() {
        let mut s = Selection::default();
        let order = [1, 2, 3, 4];
        s.click(2, false, false, &order);
        s.click(4, true, false, &order);
        s.retain_visible(&[1, 4]);
        assert_eq!(s.in_order(&[1, 4]), vec![4]);
        s.retain_visible(&[1]);
        assert!(s.is_empty());
        s.click(1, false, true, &[1, 2]);
        assert_eq!(s.in_order(&[1, 2]), vec![1]);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn dragging_a_selected_ticket_takes_the_whole_selection_in_display_order() {
        let order = [1, 2, 3, 4, 5];
        let mut s = Selection::default();
        s.click(4, false, false, &order);
        s.click(2, true, false, &order);
        assert_eq!(s.drag_set(4, &order), vec![2, 4]);
        assert_eq!(s.drag_set(2, &order), vec![2, 4]);
        assert_eq!(s.drag_set(5, &order), vec![5], "ein Ticket außerhalb der Auswahl wird allein gezogen");
        assert_eq!(s.targets(3, &order), vec![3]);
        assert_eq!(s.targets(4, &order), vec![2, 4]);
    }

    #[test]
    fn dates_are_entered_in_both_forms() {
        assert_eq!(parse_date_input("2026-11-02"), Ok(d(2026, 11, 2)));
        assert_eq!(parse_date_input(" 02.11.2026 "), Ok(d(2026, 11, 2)));
        assert_eq!(parse_date_input("2.11.2026"), Ok(d(2026, 11, 2)));
        for bad in ["", "2026-13-01", "31.02.2026", "heute", "2.11.26", "2026/11/02", "1.2"] {
            assert_eq!(parse_date_input(bad), Err(SprintError::DateInvalid), "{bad}");
        }
        assert_eq!(parse_opt_date("  "), Ok(None));
        assert_eq!(parse_opt_date("2026-11-02"), Ok(Some(d(2026, 11, 2))));
        assert!(parse_opt_date("x").is_err());
        // Anzeige und Eingabe sind zueinander passend
        for lang in [Lang::De, Lang::En] {
            assert_eq!(parse_date_input(&fmt_date_ui(d(2026, 1, 5), lang)), Ok(d(2026, 1, 5)));
        }
        assert_eq!(fmt_date_ui(d(2026, 1, 5), Lang::De), "05.01.2026");
        assert_eq!(fmt_date_ui(d(2026, 1, 5), Lang::En), "2026-01-05");
    }

    #[test]
    fn duration_input_has_a_default_and_limits() {
        assert_eq!(parse_days_input(""), Ok(DEFAULT_SPRINT_DAYS));
        assert_eq!(parse_days_input(" 5 "), Ok(5));
        assert_eq!(parse_days_input("366"), Ok(366));
        for bad in ["0", "367", "-1", "x", "2.5"] {
            assert_eq!(parse_days_input(bad), Err(SprintError::DaysInvalid { max: MAX_SPRINT_DAYS }), "{bad}");
        }
    }

    #[test]
    fn the_end_follows_start_and_duration_until_it_is_set_by_hand() {
        let cal = WorkCalendar::default();
        let mut f = SprintFields::new("Sprint 1");
        f.start = "02.11.2026".into();
        f.recompute_end(&cal, Lang::De);
        assert_eq!(f.end, "13.11.2026", "zwei Wochen ab Montag");
        f.days = "5".into();
        f.recompute_end(&cal, Lang::De);
        assert_eq!(f.end, "06.11.2026");
        // Ende von Hand: die Dauer folgt dem Ende (Arbeitstage), Start/Dauer lassen das Ende dann in Ruhe
        f.end = "20.11.2026".into();
        f.after_end_edit(&cal, Lang::De);
        assert!(f.end_manual);
        assert_eq!(f.days, "15");
        f.start = "09.11.2026".into();
        f.recompute_end(&cal, Lang::De);
        assert_eq!(f.end, "20.11.2026");
        // Ende leeren: wieder automatisch
        f.end.clear();
        f.after_end_edit(&cal, Lang::De);
        assert!(!f.end_manual);
        assert_eq!(f.end, "27.11.2026", "15 Arbeitstage ab 09.11.");
    }

    #[test]
    fn recompute_ignores_unreadable_input() {
        let cal = WorkCalendar::default();
        let mut f = SprintFields::new("S");
        f.recompute_end(&cal, Lang::En);
        assert_eq!(f.end, "", "ohne Start kein Ende");
        f.start = "kaputt".into();
        f.recompute_end(&cal, Lang::En);
        assert_eq!(f.end, "");
        f.start = "2026-11-02".into();
        f.days = "0".into();
        f.recompute_end(&cal, Lang::En);
        assert_eq!(f.end, "");
    }

    #[test]
    fn holidays_and_weekend_switch_change_the_end() {
        let cal = CalSettings { exclude_weekends: true, holidays: vec![d(2026, 11, 4)] }.calendar();
        let mut f = SprintFields::new("S");
        f.start = "2026-11-02".into();
        f.recompute_end(&cal, Lang::En);
        assert_eq!(f.end, "2026-11-16", "ein Feiertag verschiebt das Ende um einen Arbeitstag");
        let every = CalSettings { exclude_weekends: false, holidays: vec![] }.calendar();
        f.recompute_end(&every, Lang::En);
        assert_eq!(f.end, "2026-11-11", "zehn Tage am Stück");
    }

    #[test]
    fn work_days_label_counts_the_calendar() {
        let cal = WorkCalendar::default();
        let mut f = SprintFields::new("S");
        assert_eq!(f.work_days(&cal), None);
        f.start = "2026-11-02".into();
        f.end = "2026-11-13".into();
        assert_eq!(f.work_days(&cal), Some(10));
        f.end = "2026-11-01".into();
        assert_eq!(f.work_days(&cal), None, "Ende vor Start");
    }

    #[test]
    fn form_parse_checks_each_field() {
        let mut f = SprintFields::new("S");
        assert_eq!(f.parse(), Ok(ParsedSprint { start: None, end: None, days: 10 }));
        f.start = "2026-11-02".into();
        f.end = "2026-11-13".into();
        f.days = "10".into();
        assert_eq!(f.parse(), Ok(ParsedSprint { start: Some(d(2026, 11, 2)), end: Some(d(2026, 11, 13)), days: 10 }));
        f.start.clear();
        assert_eq!(f.parse(), Err(SprintError::DatesIncomplete));
        f.start = "2026-11-02".into();
        f.end = "nope".into();
        assert_eq!(f.parse(), Err(SprintError::DateInvalid));
        f.end.clear();
        f.days = "400".into();
        assert_eq!(f.parse(), Err(SprintError::DaysInvalid { max: MAX_SPRINT_DAYS }));
    }

    #[test]
    fn holidays_parse_in_any_separator() {
        assert_eq!(parse_holidays("2026-12-25, 24.12.2026;2026-12-25\n01.01.2027"), Ok(vec![d(2026, 12, 24), d(2026, 12, 25), d(2027, 1, 1)]));
        assert_eq!(parse_holidays(""), Ok(vec![]));
        assert_eq!(parse_holidays("2026-12-25 morgen"), Err("morgen".to_string()));
    }

    #[test]
    fn calendar_settings_round_trip_through_meta() {
        let s = CalSettings { exclude_weekends: false, holidays: vec![d(2026, 12, 25), d(2026, 12, 24)] };
        let text = s.holidays_text();
        assert_eq!(text, "2026-12-25,2026-12-24");
        let back = CalSettings::from_meta(Some("0"), Some(&text));
        assert!(!back.exclude_weekends);
        assert_eq!(back.holidays, vec![d(2026, 12, 24), d(2026, 12, 25)]);
        assert_eq!(CalSettings::from_meta(None, None), CalSettings::default());
        assert!(CalSettings::from_meta(Some("1"), Some("kaputt")).holidays.is_empty());
        assert_eq!(CalSettings::meta_keys(7), ("sprint_weekends_7".to_string(), "sprint_holidays_7".to_string()));
        assert_eq!(CalSettings::default().calendar(), WorkCalendar::default());
        assert_eq!(holidays_edit_text(&[d(2026, 12, 24), d(2026, 12, 25)], Lang::De), "24.12.2026\n25.12.2026");
    }

    #[test]
    fn leftover_target_comes_from_the_choice() {
        assert_eq!(leftover_from(LeftoverChoice::Backlog, Some(3)), Some(Leftover::ToBacklog));
        assert_eq!(leftover_from(LeftoverChoice::NextSprint, Some(3)), Some(Leftover::ToSprint(3)));
        assert_eq!(leftover_from(LeftoverChoice::NextSprint, None), None);
    }

    #[test]
    fn default_target_is_the_earliest_planned_sprint() {
        assert_eq!(default_target(&[]), None);
        assert_eq!(default_target(&[(3, Some(d(2026, 12, 1))), (2, Some(d(2026, 11, 20))), (4, None)]), Some(2));
        assert_eq!(default_target(&[(5, None), (4, None)]), Some(4), "ohne Termine der ältere");
        assert_eq!(default_target(&[(5, None), (9, Some(d(2027, 1, 1)))]), Some(9));
    }

    #[test]
    fn complete_summary_names_the_destination() {
        assert_eq!(complete_summary(5, 0, Leftover::ToBacklog, None, Lang::En), "5 tickets done – nothing was left open.");
        assert_eq!(complete_summary(5, 2, Leftover::ToBacklog, None, Lang::En), "5 tickets done; 2 tickets open – moved to the backlog.");
        assert_eq!(complete_summary(5, 2, Leftover::ToSprint(2), Some("Sprint 2"), Lang::En), "5 tickets done; 2 tickets open – moved to the sprint “Sprint 2”.");
        assert_eq!(complete_summary(1, 3, Leftover::ToSprint(2), Some("Sprint 2"), Lang::De), "1 Ticket erledigt; 3 Tickets offen – verschoben in den Sprint „Sprint 2“.");
        assert_eq!(complete_summary(0, 1, Leftover::ToBacklog, None, Lang::De), "0 Tickets erledigt; 1 Ticket offen – verschoben ins Backlog.");
    }

    #[test]
    fn list_moves_stay_inside_the_list() {
        let ids = [1, 2, 3];
        assert_eq!(move_in_list(&ids, 1, -1), Some(vec![2, 1, 3]));
        assert_eq!(move_in_list(&ids, 1, 1), Some(vec![1, 3, 2]));
        assert_eq!(move_in_list(&ids, 0, -1), None);
        assert_eq!(move_in_list(&ids, 2, 1), None);
        assert_eq!(move_in_list(&ids, 9, 1), None);
        assert_eq!(move_in_list(&[], 0, 1), None);
    }

    #[test]
    fn small_texts() {
        assert_eq!(tickets_text(1, Lang::De), "1 Ticket");
        assert_eq!(tickets_text(0, Lang::De), "0 Tickets");
        assert_eq!(tickets_text(2, Lang::En), "2 tickets");
        assert_eq!(tickets_text(1, Lang::En), "1 ticket");
        assert_eq!(points_text(13.0), "13");
        assert_eq!(points_text(2.5), "2.5");
    }

    #[test]
    fn figures_read_as_text() {
        let mut f = Figure::default();
        f.add(Some(3.0));
        f.add(Some(2.5));
        f.add(None);
        assert_eq!(figure_text(&f, Lang::En), "3 tickets · 5.5 P");
        assert_eq!(figure_text(&Figure::default(), Lang::De), "0 Tickets");
        assert_eq!(weight_text(&f, Basis::Points, Lang::En), "5.5 pts");
        assert_eq!(weight_text(&f, Basis::PointsWithFallbackOne, Lang::De), "6.5 P");
        assert_eq!(weight_text(&f, Basis::TicketCount, Lang::De), "3 Tickets");
        assert_eq!(unit_label(Basis::TicketCount, Lang::En), "tickets");
    }

    #[test]
    fn done_fraction_follows_the_basis() {
        let mut total = Figure::default();
        let mut done = Figure::default();
        for (p, d) in [(Some(5.0), true), (Some(5.0), false), (None, false), (None, true)] {
            total.add(p);
            if d {
                done.add(p);
            }
        }
        assert_eq!(done_fraction(&done, &total, Basis::Points), 0.5);
        assert_eq!(done_fraction(&done, &total, Basis::TicketCount), 0.5);
        assert!((done_fraction(&done, &total, Basis::PointsWithFallbackOne) - 6.0 / 12.0).abs() < 1e-6);
        assert_eq!(done_fraction(&Figure::default(), &Figure::default(), Basis::Points), 0.0);
    }
}
