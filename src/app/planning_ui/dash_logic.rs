//! Reine Logik des Reiters „Dashboards“ (ohne egui-Zustand, gut testbar): Rasterrechnung (Pixel ↔ Zelle, Ziehen, Größe ändern),
//! Titel und Beschriftungen, Spaltenkatalog der Such-Kachel, Gruppierung für Zahlen-/Diagramm-Kacheln, Skalierung der Diagramme,
//! Zeit-Behälter (Tage/Wochen), Regeln für den Zwischenspeicher der Kachel-Daten und die Vorlagen neuer Dashboards.

use crate::planning::search::TicketRow;
use crate::planning::views::dashboards::{ChartKind, CountGroup, Layout, Tile, TileKind, TimeRange, DEFAULT_SEARCH_COLUMNS, GRID_COLUMNS, LAYOUT_VERSION, MAX_ROWS, MAX_TILE_HEIGHT};
use crate::query::{parse, quote_string, Query};
use crate::state::Lang;
use chrono::{Datelike, Duration as Days, NaiveDate};
use eframe::egui::{Color32, Pos2, Rect, Vec2};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::time::Duration;

// ------------------------------------------------------------------ Raster

/// Höhe einer Rasterzeile und Abstand zwischen den Kacheln (Pixel).
pub const ROW_H: f32 = 34.0;
pub const GAP: f32 = 8.0;
/// Kleinste Kachel, die sich per Ziehen einstellen lässt (Spalten, Zeilen).
pub const MIN_TILE_W: u32 = 2;
pub const MIN_TILE_H: u32 = 3;
/// Freie Zeilen unter der letzten Kachel im Bearbeiten-Modus (Platz zum Hinunterziehen).
pub const SPARE_ROWS: u32 = 4;

/// Das Raster auf dem Bildschirm: linke obere Ecke und Spaltenbreite (aus der verfügbaren Breite).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub left: f32,
    pub top: f32,
    pub col_w: f32,
}

impl Grid {
    pub fn new(left: f32, top: f32, width: f32) -> Grid {
        let cols = GRID_COLUMNS as f32;
        Grid { left, top, col_w: ((width - GAP * (cols - 1.0)) / cols).max(4.0) }
    }

    pub fn pitch_x(&self) -> f32 {
        self.col_w + GAP
    }

    pub fn pitch_y(&self) -> f32 {
        ROW_H + GAP
    }

    /// Gesamthöhe von `rows` Zeilen.
    pub fn height(rows: u32) -> f32 {
        (rows as f32 * (ROW_H + GAP) - GAP).max(0.0)
    }

    pub fn tile_rect(&self, t: &Tile) -> Rect {
        let min = Pos2::new(self.left + t.x as f32 * self.pitch_x(), self.top + t.y as f32 * self.pitch_y());
        let size = Vec2::new(t.w as f32 * self.pitch_x() - GAP, t.h as f32 * self.pitch_y() - GAP);
        Rect::from_min_size(min, size)
    }

    /// Zelle (Spalte, Zeile) für die linke obere Ecke einer Kachel der Breite `w`, deren Ecke gerade bei `top_left` liegt.
    pub fn move_target(&self, w: u32, top_left: Pos2) -> (u32, u32) {
        let cx = ((top_left.x - self.left) / self.pitch_x()).round() as i64;
        let cy = ((top_left.y - self.top) / self.pitch_y()).round() as i64;
        let max_x = GRID_COLUMNS.saturating_sub(w) as i64;
        (cx.clamp(0, max_x) as u32, cy.clamp(0, MAX_ROWS as i64 - 1) as u32)
    }

    /// Neue Größe (Spalten, Zeilen), wenn die rechte untere Ecke der Kachel bei `pointer` liegt.
    pub fn resize_target(&self, x: u32, y: u32, pointer: Pos2) -> (u32, u32) {
        let w = ((pointer.x + GAP - self.left) / self.pitch_x()).round() as i64 - x as i64;
        let h = ((pointer.y + GAP - self.top) / self.pitch_y()).round() as i64 - y as i64;
        let max_w = GRID_COLUMNS.saturating_sub(x).max(MIN_TILE_W) as i64;
        (w.clamp(MIN_TILE_W as i64, max_w) as u32, h.clamp(MIN_TILE_H as i64, MAX_TILE_HEIGHT as i64) as u32)
    }
}

/// Wie viele Zeilen das Layout belegt.
pub fn layout_rows(l: &Layout) -> u32 {
    l.tiles.iter().map(|t| t.y + t.h).max().unwrap_or(0)
}

/// Verschiebt die Kachel um `dx`/`dy` Zellen (innerhalb des Rasters); die anderen weichen aus. `true`, wenn sich etwas geändert hat.
pub fn nudge(l: &mut Layout, id: u32, dx: i32, dy: i32) -> bool {
    let Some(t) = l.tile(id) else { return false };
    let x = (t.x as i64 + dx as i64).clamp(0, GRID_COLUMNS.saturating_sub(t.w) as i64) as u32;
    let y = (t.y as i64 + dy as i64).clamp(0, (MAX_ROWS.saturating_sub(t.h)) as i64) as u32;
    let before = l.clone();
    l.move_tile(id, x, y);
    *l != before
}

/// Was beim Ziehen gerade passiert.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragMode {
    Move,
    Resize,
}

/// Das Layout so, wie es beim Loslassen würde: Kachel `id` ist an der Stelle des Mauszeigers, die anderen weichen aus.
/// `grab` ist der Abstand des Mauszeigers zur linken oberen Ecke der Kachel beim Anfassen.
pub fn preview_layout(base: &Layout, id: u32, mode: DragMode, grab: Vec2, grid: &Grid, pointer: Pos2) -> Layout {
    let mut l = base.clone();
    let Some(t) = base.tile(id) else { return l };
    match mode {
        DragMode::Move => {
            let (x, y) = grid.move_target(t.w, pointer - grab);
            l.move_tile(id, x, y);
        }
        DragMode::Resize => {
            let (w, h) = grid.resize_target(t.x, t.y, pointer - grab);
            l.resize_tile(id, w, h);
        }
    }
    l
}

// ------------------------------------------------------------------ Kachel-Arten und Titel

/// Auswahl der Kachel-Art im Dialog (ohne die Einstellungen der Art).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindSel {
    Search,
    Counts,
    Sprint,
    Burndown,
    TimeTracking,
    Favorites,
    Flagged,
    Projects,
}

pub const KIND_SELS: [KindSel; 8] = [KindSel::Search, KindSel::Counts, KindSel::Sprint, KindSel::Burndown, KindSel::TimeTracking, KindSel::Favorites, KindSel::Flagged, KindSel::Projects];

impl KindSel {
    pub fn of<P>(k: &TileKind<P>) -> KindSel {
        match k {
            TileKind::Search { .. } => KindSel::Search,
            TileKind::Counts { .. } => KindSel::Counts,
            TileKind::Sprint { .. } => KindSel::Sprint,
            TileKind::Burndown { .. } => KindSel::Burndown,
            TileKind::TimeTracking { .. } => KindSel::TimeTracking,
            TileKind::Favorites => KindSel::Favorites,
            TileKind::Flagged => KindSel::Flagged,
            TileKind::Projects => KindSel::Projects,
        }
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            KindSel::Search => lang.t("Suche (Ticket-Liste)", "Search (ticket list)"),
            KindSel::Counts => lang.t("Zahlen / Diagramm", "Numbers / chart"),
            KindSel::Sprint => lang.t("Sprint-Fortschritt", "Sprint progress"),
            KindSel::Burndown => lang.t("Burndown", "Burndown"),
            KindSel::TimeTracking => lang.t("Zeiterfassung", "Time tracking"),
            KindSel::Favorites => lang.t("Favoriten", "Favorites"),
            KindSel::Flagged => lang.t("Markierte Tickets", "Flagged tickets"),
            KindSel::Projects => lang.t("Alle Projekte", "All projects"),
        }
    }

    pub fn explain(self, lang: Lang) -> &'static str {
        match self {
            KindSel::Search => lang.t("Die Treffer einer Abfrage (JQL) oder eines gespeicherten Filters als Liste.", "The hits of a query (JQL) or of a saved filter as a list."),
            KindSel::Counts => lang.t("Anzahl der Tickets einer Abfrage nach Status, Typ oder Priorität als Torte oder Balken.", "Number of tickets of a query by status, type or priority as a pie or bar chart."),
            KindSel::Sprint => lang.t("Fortschritt des aktiven Sprints eines Projekts.", "Progress of the active sprint of a project."),
            KindSel::Burndown => lang.t("Burndown-Diagramm des aktiven Sprints eines Projekts.", "Burndown chart of the active sprint of a project."),
            KindSel::TimeTracking => lang.t("Gebuchte Zeit je Tag oder Woche.", "Booked time per day or week."),
            KindSel::Favorites => lang.t("Tickets mit Stern.", "Starred tickets."),
            KindSel::Flagged => lang.t("Markierte Tickets (Flagge).", "Flagged tickets."),
            KindSel::Projects => lang.t("Alle Projekte mit Zugriff per Klick.", "All projects with quick access by click."),
        }
    }
}

/// Titel der Kachel: der eigene der Such-Kachel, sonst ein Standardtitel der Art.
pub fn tile_title(kind: &TileKind<i64>, lang: Lang) -> String {
    match kind {
        TileKind::Search { title, .. } if !title.trim().is_empty() => title.trim().to_string(),
        TileKind::Search { .. } => lang.t("Tickets", "Tickets").to_string(),
        TileKind::Sprint { .. } => "Sprint".to_string(),
        TileKind::Burndown { .. } => "Burndown".to_string(),
        TileKind::Counts { group_by, .. } => match group_by {
            CountGroup::Status => lang.t("Tickets nach Status", "Tickets by status"),
            CountGroup::Type => lang.t("Tickets nach Typ", "Tickets by type"),
            CountGroup::Priority => lang.t("Tickets nach Priorität", "Tickets by priority"),
        }
        .to_string(),
        TileKind::TimeTracking { range, .. } => match range {
            TimeRange::Day => lang.t("Zeit pro Tag", "Time per day"),
            TimeRange::Week => lang.t("Zeit pro Woche", "Time per week"),
        }
        .to_string(),
        TileKind::Favorites => lang.t("Favoriten", "Favorites").to_string(),
        TileKind::Flagged => lang.t("Markierte Tickets", "Flagged tickets").to_string(),
        TileKind::Projects => lang.t("Alle Projekte", "All projects").to_string(),
    }
}

/// JQL der Kachel, soweit sie eine hat (für „Filter öffnen“): Such-/Zahlen-Kachel die eigene, Favoriten/Markierte feste.
pub fn tile_jql(kind: &TileKind<i64>) -> Option<String> {
    match kind {
        TileKind::Search { jql, .. } | TileKind::Counts { jql, .. } => Some(jql.clone()),
        TileKind::Favorites => Some(FAVORITES_JQL.to_string()),
        TileKind::Flagged => Some(FLAGGED_JQL.to_string()),
        _ => None,
    }
}

/// Abfragen der festen Listen-Kacheln.
pub const FAVORITES_JQL: &str = "starred = true ORDER BY updated DESC";
pub const FLAGGED_JQL: &str = "flagged = true ORDER BY updated DESC";
/// Spalten der festen Listen-Kacheln.
pub const LIST_COLUMNS: [&str; 4] = ["key", "summary", "status", "updated"];

// ------------------------------------------------------------------ Spalten der Such-Kachel

/// Eine Spalte der Such-Kachel. `width` 0 = nimmt den Rest (nur „summary“).
#[derive(Debug, PartialEq)]
pub struct ColInfo {
    pub id: &'static str,
    pub de: &'static str,
    pub en: &'static str,
    pub width: f32,
}

impl ColInfo {
    pub fn label(&self, lang: Lang) -> &'static str {
        lang.t(self.de, self.en)
    }
}

pub const COLUMNS: [ColInfo; 10] = [
    ColInfo { id: "key", de: "Schlüssel", en: "Key", width: 66.0 },
    ColInfo { id: "type", de: "Typ", en: "Type", width: 104.0 },
    ColInfo { id: "summary", de: "Titel", en: "Summary", width: 0.0 },
    ColInfo { id: "status", de: "Status", en: "Status", width: 104.0 },
    ColInfo { id: "priority", de: "Priorität", en: "Priority", width: 84.0 },
    ColInfo { id: "points", de: "Punkte", en: "Points", width: 48.0 },
    ColInfo { id: "updated", de: "Geändert", en: "Updated", width: 78.0 },
    ColInfo { id: "parent", de: "Eltern", en: "Parent", width: 66.0 },
    ColInfo { id: "project", de: "Projekt", en: "Project", width: 56.0 },
    ColInfo { id: "marks", de: "Markierung", en: "Marks", width: 40.0 },
];

pub fn col_info(id: &str) -> Option<&'static ColInfo> {
    COLUMNS.iter().find(|c| c.id == id)
}

/// Die bekannten Spalten in der Reihenfolge der Einstellung (Doppelte und Unbekannte entfallen); ohne eine gültige Angabe die Standardspalten.
pub fn known_columns(ids: &[String]) -> Vec<&'static ColInfo> {
    let mut out: Vec<&'static ColInfo> = Vec::new();
    for id in ids {
        if let Some(c) = col_info(id.trim()) {
            if !out.iter().any(|o| o.id == c.id) {
                out.push(c);
            }
        }
    }
    if out.is_empty() {
        out = DEFAULT_SEARCH_COLUMNS.iter().filter_map(|id| col_info(id)).collect();
    }
    out
}

/// Abstand zwischen den Zellen einer Zeile.
pub const CELL_GAP: f32 = 8.0;
const MIN_FLEX: f32 = 80.0;

/// Breiten der Spalten in `avail` Pixeln: feste Breiten, der Titel bekommt den Rest. Passt es nicht, werden alle anteilig schmaler.
pub fn column_widths(cols: &[&ColInfo], avail: f32) -> Vec<f32> {
    if cols.is_empty() {
        return Vec::new();
    }
    let gaps = CELL_GAP * (cols.len() as f32 - 1.0);
    let fixed: f32 = cols.iter().map(|c| c.width).sum();
    let flex_count = cols.iter().filter(|c| c.width == 0.0).count();
    let flex = if flex_count > 0 { ((avail - gaps - fixed) / flex_count as f32).max(MIN_FLEX) } else { 0.0 };
    let mut w: Vec<f32> = cols.iter().map(|c| if c.width == 0.0 { flex } else { c.width }).collect();
    let total: f32 = w.iter().sum::<f32>() + gaps;
    if total > avail && avail > gaps {
        let k = (avail - gaps) / (total - gaps);
        for x in &mut w {
            *x = (*x * k).max(24.0);
        }
    }
    w
}

// ------------------------------------------------------------------ Gruppen (Zahlen-/Diagramm-Kachel)

/// Eine Gruppe (Segment/Balken): Name, Farbe (`#RRGGBB`, falls bekannt), Anzahl; `none` = „ohne Wert“, `other` = Sammelgruppe.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub color: Option<String>,
    pub count: u32,
    pub none: bool,
    pub other: bool,
}

/// Höchstzahl der Gruppen im Diagramm (der Rest wird zu „Sonstige“).
pub const MAX_GROUPS: usize = 9;

/// Name und Farbe der Gruppe, zu der die Zeile zählt (`None` = ohne Wert).
pub fn row_group(row: &TicketRow, g: CountGroup) -> (Option<String>, Option<String>) {
    let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    match g {
        CountGroup::Status => (row.status_name.clone(), row.status_color.clone()),
        CountGroup::Type => (non_empty(&row.type_name), non_empty(&row.type_color)),
        CountGroup::Priority => (row.priority.clone(), row.priority_color.clone()),
    }
}

/// Zählt Einträge je Name: absteigend nach Anzahl (bei Gleichstand nach Name); mehr als `max` Gruppen → die kleinsten werden zu „Sonstige“.
pub fn group_counts(items: impl IntoIterator<Item = (Option<String>, Option<String>)>, none_label: &str, other_label: &str, max: usize) -> Vec<Group> {
    let mut map: HashMap<Option<String>, (Option<String>, u32)> = HashMap::new();
    for (name, color) in items {
        let e = map.entry(name).or_insert((color.clone(), 0));
        if e.0.is_none() {
            e.0 = color;
        }
        e.1 += 1;
    }
    let mut groups: Vec<Group> = map
        .into_iter()
        .map(|(name, (color, count))| match name {
            Some(n) => Group { name: n, color, count, none: false, other: false },
            None => Group { name: none_label.to_string(), color: None, count, none: true, other: false },
        })
        .collect();
    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let max = max.max(2);
    if groups.len() > max {
        let rest: u32 = groups.split_off(max - 1).iter().map(|g| g.count).sum();
        groups.push(Group { name: other_label.to_string(), color: None, count: rest, none: false, other: true });
    }
    groups
}

/// Feldname der Gruppierung in der Abfragesprache.
pub fn group_field(g: CountGroup) -> &'static str {
    match g {
        CountGroup::Status => "status",
        CountGroup::Type => "type",
        CountGroup::Priority => "priority",
    }
}

/// Abfrage für „Segment öffnen“: die Abfrage der Kachel UND das Segment (`status = "Done"` bzw. `priority IS EMPTY`); die Sortierung
/// bleibt am Ende. Lässt sich die Abfrage der Kachel nicht lesen, steht nur das Segment da.
pub fn segment_jql(base: &str, g: CountGroup, value: Option<&str>) -> String {
    let field = group_field(g);
    let cond = match value {
        Some(v) => format!("{field} = {}", quote_string(v)),
        None => format!("{field} IS EMPTY"),
    };
    let Ok(q) = parse(base) else { return cond };
    let mut s = match &q.filter {
        Some(f) => format!("({f}) AND {cond}"),
        None => cond,
    };
    let order = Query { filter: None, order: q.order.clone() }.to_string();
    if !order.is_empty() {
        s.push(' ');
        s.push_str(&order);
    }
    s
}

/// Zahl für Beschriftungen: ganze Zahlen ohne Nachkommastelle, sonst eine (Komma im Deutschen).
pub fn num_text(v: f64, lang: Lang) -> String {
    let s = if (v - v.round()).abs() < 0.05 { format!("{}", v.round() as i64) } else { format!("{v:.1}") };
    if lang == Lang::De { s.replace('.', ",") } else { s }
}

pub fn percent_text(count: u32, total: u32) -> String {
    if total == 0 {
        return "0%".to_string();
    }
    format!("{}%", (count as f64 * 100.0 / total as f64).round() as u32)
}

// ------------------------------------------------------------------ Diagramm-Skalierung

/// Länge eines Balkens in Pixeln (höchstens `width`); ein Wert über 0 ist immer sichtbar (mindestens 2 Pixel).
pub fn bar_len(value: f64, max: f64, width: f32) -> f32 {
    if !(max > 0.0) || !(value > 0.0) || !(width > 0.0) {
        return 0.0;
    }
    ((value / max) as f32 * width).clamp(2.0_f32.min(width), width)
}

/// Die nächste „runde“ Zahl (1, 2, 5 × 10^k) ab `v`; für `v <= 0` die 1.
pub fn nice_max(v: f64) -> f64 {
    if !(v > 0.0) || !v.is_finite() {
        return 1.0;
    }
    let exp = v.log10().floor();
    let base = 10f64.powf(exp);
    let m = v / base;
    let nice = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * base
}

/// Beschriftungen der senkrechten Achse: `n + 1` Werte von 0 bis `max`.
pub fn axis_ticks(max: f64, n: usize) -> Vec<f64> {
    let n = n.max(1);
    (0..=n).map(|i| max * i as f64 / n as f64).collect()
}

/// Winkel (Bogenmaß, ab 12 Uhr im Uhrzeigersinn) von Anfang und Ende jedes Segments; ohne Treffer leer.
pub fn pie_angles(counts: &[u32]) -> Vec<(f32, f32)> {
    let total: u64 = counts.iter().map(|c| *c as u64).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut acc = 0u64;
    counts
        .iter()
        .map(|c| {
            let a0 = acc as f64 / total as f64 * std::f64::consts::TAU;
            acc += *c as u64;
            let a1 = acc as f64 / total as f64 * std::f64::consts::TAU;
            (a0 as f32, a1 as f32)
        })
        .collect()
}

/// Welches Segment liegt unter `p` (Mittelpunkt `center`, Radius `radius`)? `None` außerhalb des Kreises.
pub fn pie_hit(center: Pos2, radius: f32, p: Pos2, counts: &[u32]) -> Option<usize> {
    let d = p - center;
    if radius <= 0.0 || d.length() > radius {
        return None;
    }
    let a = (d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2).rem_euclid(std::f32::consts::TAU);
    pie_angles(counts).iter().position(|(a0, a1)| counts_nonzero(*a0, *a1) && a >= *a0 && a < *a1)
}

fn counts_nonzero(a0: f32, a1: f32) -> bool {
    a1 > a0
}

/// Farbe eines Segments: die der Gruppe, sonst die Palette der Anwendung der Reihe nach.
pub fn slice_color(index: usize, color: Option<&str>) -> Color32 {
    use super::helpers::{parse_hex, PALETTE};
    color.and_then(parse_hex).unwrap_or_else(|| parse_hex(PALETTE[index % PALETTE.len()]).unwrap_or(Color32::GRAY))
}

// ------------------------------------------------------------------ Zeit

/// Anzahl der Tage bzw. Wochen der Zeiterfassungs-Kachel.
pub const DAY_BUCKETS: i64 = 14;
pub const WEEK_BUCKETS: i64 = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub start: NaiveDate,
    pub label: String,
    pub minutes: u32,
}

/// Der abgefragte Zeitraum (erster und letzter Tag, einschließlich).
pub fn time_window(today: NaiveDate, range: TimeRange) -> (NaiveDate, NaiveDate) {
    match range {
        TimeRange::Day => (today - Days::days(DAY_BUCKETS - 1), today),
        TimeRange::Week => {
            let monday = today - Days::days(today.weekday().num_days_from_monday() as i64);
            (monday - Days::days(7 * (WEEK_BUCKETS - 1)), monday + Days::days(6))
        }
    }
}

fn weekday_short(d: NaiveDate, lang: Lang) -> &'static str {
    let i = d.weekday().num_days_from_monday() as usize;
    match lang {
        Lang::De => ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"][i],
        Lang::En => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i],
    }
}

/// Kurzes Datum: „9.11.“ bzw. „Nov 9“.
pub fn short_date(d: NaiveDate, lang: Lang) -> String {
    match lang {
        Lang::De => format!("{}.{}.", d.day(), d.month()),
        Lang::En => {
            let m = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][d.month0() as usize];
            format!("{m} {}", d.day())
        }
    }
}

/// Zeit je Tag zu Behältern (14 Tage bzw. 8 Wochen, alt → neu) zusammenfassen.
pub fn bucket_time(by_day: &BTreeMap<NaiveDate, u32>, today: NaiveDate, range: TimeRange, lang: Lang) -> Vec<Bucket> {
    let (from, _) = time_window(today, range);
    match range {
        TimeRange::Day => (0..DAY_BUCKETS)
            .map(|i| {
                let d = from + Days::days(i);
                Bucket { start: d, label: format!("{} {}", weekday_short(d, lang), d.day()), minutes: by_day.get(&d).copied().unwrap_or(0) }
            })
            .collect(),
        TimeRange::Week => (0..WEEK_BUCKETS)
            .map(|i| {
                let start = from + Days::days(7 * i);
                let minutes = (0..7).map(|k| by_day.get(&(start + Days::days(k))).copied().unwrap_or(0)).fold(0u32, |a, m| a.saturating_add(m));
                let wk = start.iso_week().week();
                Bucket { start, label: format!("{}{wk}", lang.t("KW", "W")), minutes }
            })
            .collect(),
    }
}

// ------------------------------------------------------------------ Zwischenspeicher

/// Woran sich erkennen lässt, ob die gespeicherten Daten einer Kachel noch passen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheMeta {
    pub key: u64,
    pub generation: u64,
}

/// Neu laden, wenn es noch nichts gibt, die Einstellungen (`key`) oder der Stand der Daten (`generation`) sich geändert haben
/// oder die Daten älter als `ttl` sind.
pub fn needs_reload(have: Option<CacheMeta>, key: u64, generation: u64, age: Duration, ttl: Duration) -> bool {
    match have {
        None => true,
        Some(m) => m.key != key || m.generation != generation || age >= ttl,
    }
}

/// Schlüssel der Einstellungen einer Kachel (Art, Abfrage, Projekte, Sprache).
pub fn tile_key(kind: &TileKind<i64>, dash_project: Option<i64>, lang: Lang) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(kind).unwrap_or_default().hash(&mut h);
    dash_project.hash(&mut h);
    (lang == Lang::De).hash(&mut h);
    h.finish()
}

/// Welche Kacheln in diesem Bild geladen werden (höchstens `max`): zuerst die ohne Daten, dann die ältesten – jeweils in der gegebenen Reihenfolge.
/// Eintrag: (Kachel, hat schon Daten).
pub fn pick_loads(pending: &[(u32, bool)], max: usize) -> Vec<u32> {
    let mut v: Vec<u32> = pending.iter().filter(|(_, has)| !*has).map(|(id, _)| *id).collect();
    v.extend(pending.iter().filter(|(_, has)| *has).map(|(id, _)| *id));
    v.truncate(max);
    v
}

// ------------------------------------------------------------------ Kachel bearbeiten (Entwurf)

/// Arbeitskopie einer Kachel im Dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct TileDraft {
    pub sel: KindSel,
    pub title: String,
    pub jql: String,
    pub columns: Vec<String>,
    pub group_by: CountGroup,
    pub chart: ChartKind,
    pub range: TimeRange,
    pub project: Option<i64>,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl TileDraft {
    pub fn new(sel: KindSel) -> TileDraft {
        let mut d = TileDraft {
            sel,
            title: String::new(),
            jql: String::new(),
            columns: DEFAULT_SEARCH_COLUMNS.iter().map(|s| s.to_string()).collect(),
            group_by: CountGroup::Status,
            chart: ChartKind::Pie,
            range: TimeRange::Week,
            project: None,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        d.set_default_size();
        d
    }

    /// Setzt Breite und Höhe auf die Standardgröße der gewählten Art.
    pub fn set_default_size(&mut self) {
        let (w, h) = self.to_kind().default_size();
        self.w = w;
        self.h = h;
    }

    pub fn from_tile(t: &Tile) -> TileDraft {
        let mut d = TileDraft::new(KindSel::of(&t.kind));
        match &t.kind {
            TileKind::Search { jql, columns, title } => {
                d.jql = jql.clone();
                d.columns = columns.clone();
                d.title = title.clone();
            }
            TileKind::Counts { jql, group_by, chart } => {
                d.jql = jql.clone();
                d.group_by = *group_by;
                d.chart = *chart;
            }
            TileKind::Sprint { project_id } | TileKind::Burndown { project_id } => d.project = *project_id,
            TileKind::TimeTracking { range, project_id } => {
                d.range = *range;
                d.project = *project_id;
            }
            TileKind::Favorites | TileKind::Flagged | TileKind::Projects => {}
        }
        d.x = t.x;
        d.y = t.y;
        d.w = t.w;
        d.h = t.h;
        d
    }

    pub fn to_kind(&self) -> TileKind<i64> {
        match self.sel {
            KindSel::Search => TileKind::Search { jql: self.jql.trim().to_string(), columns: self.columns.clone(), title: self.title.trim().to_string() },
            KindSel::Counts => TileKind::Counts { jql: self.jql.trim().to_string(), group_by: self.group_by, chart: self.chart },
            KindSel::Sprint => TileKind::Sprint { project_id: self.project },
            KindSel::Burndown => TileKind::Burndown { project_id: self.project },
            KindSel::TimeTracking => TileKind::TimeTracking { range: self.range, project_id: self.project },
            KindSel::Favorites => TileKind::Favorites,
            KindSel::Flagged => TileKind::Flagged,
            KindSel::Projects => TileKind::Projects,
        }
    }

    /// Hat die Art eine Abfrage (JQL)?
    pub fn has_query(&self) -> bool {
        matches!(self.sel, KindSel::Search | KindSel::Counts)
    }

    /// Braucht die Art ein Projekt (im globalen Dashboard muss es ausdrücklich gewählt sein)?
    pub fn needs_project(&self) -> bool {
        matches!(self.sel, KindSel::Sprint | KindSel::Burndown)
    }

    /// Spalte ein- oder ausschalten (neue Spalten kommen ans Ende).
    pub fn toggle_column(&mut self, id: &str) {
        if let Some(i) = self.columns.iter().position(|c| c == id) {
            self.columns.remove(i);
        } else {
            self.columns.push(id.to_string());
        }
    }

    /// Spalte um eine Stelle verschieben (`dir` −1 = nach links/oben, 1 = nach rechts/unten).
    pub fn move_column(&mut self, id: &str, dir: i32) -> bool {
        let Some(i) = self.columns.iter().position(|c| c == id) else { return false };
        let j = i as i64 + dir as i64;
        if j < 0 || j >= self.columns.len() as i64 {
            return false;
        }
        self.columns.swap(i, j as usize);
        true
    }
}

/// Setzt den Entwurf ins Layout: neue Kachel (`tile = None`) an der ersten freien Stelle in der Größe des Entwurfs, sonst die vorhandene
/// ändern (Art, Einstellungen, Platz, Größe; die anderen weichen aus). Liefert die Nummer der Kachel; `None`, wenn kein Platz mehr ist
/// (zu viele Kacheln) oder die Kachel fehlt.
pub fn apply_draft(l: &mut Layout, tile: Option<u32>, d: &TileDraft) -> Option<u32> {
    let kind = d.to_kind();
    let (w, h) = (d.w.clamp(1, GRID_COLUMNS), d.h.clamp(1, MAX_TILE_HEIGHT));
    match tile {
        None => l.add_tile(kind, Some((w, h))),
        Some(id) => {
            let t = l.tiles.iter_mut().find(|t| t.id == id)?;
            t.kind = kind;
            l.move_tile(id, d.x, d.y);
            l.resize_tile(id, w, h);
            Some(id)
        }
    }
}

/// Dateiname für den Export: Buchstaben und Ziffern bleiben (klein), alles andere wird zu `-`; leer → `dashboard`.
pub fn file_slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "dashboard".to_string() } else { out }
}

// ------------------------------------------------------------------ Vorlagen neuer Dashboards

/// Was ein neues Dashboard enthält.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    /// Eine große Such-Kachel (wie das Standard-Dashboard).
    Search,
    /// Mehrere Kacheln: Zahlen nach Status/Typ, Markierte, zuletzt Geändertes, Favoriten, Sprint, Burndown, Zeit bzw. Projekte.
    Overview,
}

/// Das Layout der Übersichts-Vorlage. `sprints`: das Projekt nutzt Sprints (Sprint- und Burndown-Kachel); `global`: Dashboard über alle
/// Projekte (statt Sprint/Burndown die Projekt-Kachel).
pub fn overview_layout(lang: Lang, sprints: bool, global: bool) -> Layout {
    let mut l = Layout { version: LAYOUT_VERSION, tiles: Vec::new() };
    let put = |l: &mut Layout, kind: TileKind<i64>, x: u32, y: u32, w: u32, h: u32| {
        let id = l.next_id();
        l.tiles.push(Tile { id, x, y, w, h, kind });
    };
    put(&mut l, TileKind::Counts { jql: String::new(), group_by: CountGroup::Status, chart: ChartKind::Pie }, 0, 0, 4, 8);
    put(&mut l, TileKind::Counts { jql: String::new(), group_by: CountGroup::Type, chart: ChartKind::Bar }, 4, 0, 4, 8);
    put(&mut l, TileKind::Flagged, 8, 0, 4, 8);
    put(
        &mut l,
        TileKind::Search { jql: "ORDER BY updated DESC".to_string(), columns: ["key", "type", "summary", "status", "updated"].iter().map(|s| s.to_string()).collect(), title: lang.t("Zuletzt geändert", "Recently changed").to_string() },
        0,
        8,
        8,
        9,
    );
    put(&mut l, TileKind::Favorites, 8, 8, 4, 9);
    if global {
        put(&mut l, TileKind::Projects, 0, 17, 4, 8);
        put(&mut l, TileKind::TimeTracking { range: TimeRange::Week, project_id: None }, 4, 17, 8, 8);
    } else if sprints {
        put(&mut l, TileKind::Sprint { project_id: None }, 0, 17, 4, 8);
        put(&mut l, TileKind::Burndown { project_id: None }, 4, 17, 4, 8);
        put(&mut l, TileKind::TimeTracking { range: TimeRange::Week, project_id: None }, 8, 17, 4, 8);
    } else {
        put(&mut l, TileKind::TimeTracking { range: TimeRange::Week, project_id: None }, 0, 17, 12, 8);
    }
    l.normalize();
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::views::dashboards::{default_dashboard_layout, validate_layout};
    use crate::planning::views::testutil::{db, project};

    fn tile(id: u32, x: u32, y: u32, w: u32, h: u32) -> Tile {
        Tile { id, x, y, w, h, kind: TileKind::Favorites }
    }

    fn layout(tiles: Vec<Tile>) -> Layout {
        Layout { version: LAYOUT_VERSION, tiles }
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // ---------------------------------------------------------------- Raster

    #[test]
    fn grid_places_tiles_by_cells() {
        let g = Grid::new(10.0, 20.0, 1200.0 - 0.0);
        let cw = (1200.0 - GAP * 11.0) / 12.0;
        assert!((g.col_w - cw).abs() < 0.001);
        let r = g.tile_rect(&tile(1, 0, 0, 12, 2));
        assert_eq!((r.left(), r.top()), (10.0, 20.0));
        assert!((r.width() - 1200.0).abs() < 0.01, "12 Spalten füllen die ganze Breite: {}", r.width());
        assert_eq!(r.height(), 2.0 * (ROW_H + GAP) - GAP);
        let r = g.tile_rect(&tile(2, 3, 4, 1, 1));
        assert!((r.left() - (10.0 + 3.0 * g.pitch_x())).abs() < 0.001);
        assert!((r.top() - (20.0 + 4.0 * g.pitch_y())).abs() < 0.001);
        assert_eq!(Grid::height(0), 0.0);
        assert_eq!(Grid::height(3), 3.0 * (ROW_H + GAP) - GAP);
        // sehr schmal: nie negativ oder 0
        assert!(Grid::new(0.0, 0.0, 10.0).col_w >= 4.0);
    }

    #[test]
    fn move_target_snaps_and_stays_in_the_grid() {
        let g = Grid::new(0.0, 0.0, 1200.0);
        let (px, py) = (g.pitch_x(), g.pitch_y());
        assert_eq!(g.move_target(4, Pos2::new(0.0, 0.0)), (0, 0));
        assert_eq!(g.move_target(4, Pos2::new(px * 2.4, py * 3.6)), (2, 4), "rundet zur nächsten Zelle");
        assert_eq!(g.move_target(4, Pos2::new(-500.0, -500.0)), (0, 0), "nie links/oben heraus");
        assert_eq!(g.move_target(4, Pos2::new(5000.0, 5000.0)).0, 8, "rechts bleibt Platz für die Breite");
        assert_eq!(g.move_target(12, Pos2::new(px * 5.0, 0.0)).0, 0);
        assert!(g.move_target(4, Pos2::new(0.0, 1.0e9)).1 < MAX_ROWS);
        assert_eq!(g.move_target(4, Pos2::new(f32::NAN, f32::NAN)), (0, 0), "NaN ist kein Absturz");
    }

    #[test]
    fn resize_target_respects_minimum_and_maximum() {
        let g = Grid::new(0.0, 0.0, 1200.0);
        let (px, py) = (g.pitch_x(), g.pitch_y());
        // rechte untere Ecke einer 4×3-Kachel bei (2,1): x + w = 6, y + h = 4
        let corner = Pos2::new(6.0 * px - GAP, 4.0 * py - GAP);
        assert_eq!(g.resize_target(2, 1, corner), (4, 3));
        assert_eq!(g.resize_target(2, 1, Pos2::new(0.0, 0.0)), (MIN_TILE_W, MIN_TILE_H), "nie kleiner als das Minimum");
        assert_eq!(g.resize_target(2, 1, Pos2::new(1.0e6, 1.0e6)), (10, MAX_TILE_HEIGHT), "nie über den Rand und nie höher als erlaubt");
        assert_eq!(g.resize_target(11, 0, Pos2::new(1.0e6, 100.0)).0, MIN_TILE_W, "am rechten Rand gilt das Minimum (die Kachel rückt dann nach links)");
    }

    #[test]
    fn preview_moves_and_resizes_and_pushes_the_others_down() {
        let g = Grid::new(0.0, 0.0, 1200.0);
        let base = layout(vec![tile(1, 0, 0, 6, 4), tile(2, 6, 0, 6, 4)]);
        // Kachel 2 über Kachel 1 ziehen: 1 weicht nach unten aus
        let p = preview_layout(&base, 2, DragMode::Move, Vec2::ZERO, &g, Pos2::new(0.0, 0.0));
        assert_eq!((p.tile(2).unwrap().x, p.tile(2).unwrap().y), (0, 0));
        assert_eq!(p.tile(1).unwrap().y, 4);
        validate_layout(&db(), None, &p).unwrap();
        // Größe: Kachel 1 breiter → Kachel 2 weicht aus
        let corner = Pos2::new(8.0 * g.pitch_x() - GAP, 4.0 * g.pitch_y() - GAP);
        let p = preview_layout(&base, 1, DragMode::Resize, Vec2::ZERO, &g, corner);
        assert_eq!(p.tile(1).unwrap().w, 8);
        assert!(p.tile(2).unwrap().y >= 4);
        validate_layout(&db(), None, &p).unwrap();
        // Unbekannte Kachel: unverändert
        assert_eq!(preview_layout(&base, 99, DragMode::Move, Vec2::ZERO, &g, Pos2::ZERO), base);
        // Der Greifpunkt zählt: am rechten Rand der Kachel angefasst bleibt die Kachel links bei der Maus
        let p = preview_layout(&base, 1, DragMode::Move, Vec2::new(3.0 * g.pitch_x(), 0.0), &g, Pos2::new(3.0 * g.pitch_x(), 0.0));
        assert_eq!(p.tile(1).unwrap().x, 0);
    }

    #[test]
    fn nudge_moves_by_cells_and_stops_at_the_edges() {
        let mut l = layout(vec![tile(1, 0, 0, 6, 4), tile(2, 6, 0, 6, 4)]);
        assert!(!nudge(&mut l, 1, -1, 0), "links ist Schluss");
        assert!(!nudge(&mut l, 1, 0, -1));
        assert!(!nudge(&mut l, 2, 1, 0), "rechts ist Schluss");
        assert!(nudge(&mut l, 1, 0, 1));
        assert_eq!(l.tile(1).unwrap().y, 1);
        assert!(nudge(&mut l, 2, 0, 2));
        assert_eq!(l.tile(2).unwrap().y, 2);
        // gegeneinander schieben: die andere weicht aus, es bleibt gültig
        assert!(nudge(&mut l, 1, 4, 0));
        validate_layout(&db(), None, &l).unwrap();
        assert!(!nudge(&mut l, 99, 1, 1));
    }

    #[test]
    fn rows_of_a_layout() {
        assert_eq!(layout_rows(&layout(vec![])), 0);
        assert_eq!(layout_rows(&layout(vec![tile(1, 0, 0, 6, 4), tile(2, 6, 3, 6, 9)])), 12);
    }

    // ---------------------------------------------------------------- Titel, Spalten

    #[test]
    fn titles_follow_kind_and_language() {
        let search = |t: &str| TileKind::<i64>::Search { jql: String::new(), columns: vec![], title: t.into() };
        assert_eq!(tile_title(&search("  Bugs "), Lang::En), "Bugs");
        assert_eq!(tile_title(&search(""), Lang::En), "Tickets");
        let c = |g| TileKind::<i64>::Counts { jql: String::new(), group_by: g, chart: ChartKind::Pie };
        assert_eq!(tile_title(&c(CountGroup::Status), Lang::De), "Tickets nach Status");
        assert_eq!(tile_title(&c(CountGroup::Priority), Lang::En), "Tickets by priority");
        assert_eq!(tile_title(&TileKind::<i64>::Projects, Lang::De), "Alle Projekte");
        // jede Art hat in beiden Sprachen einen nichtleeren Titel und eine Beschriftung
        for sel in KIND_SELS {
            for lang in [Lang::De, Lang::En] {
                assert!(!sel.label(lang).is_empty() && !sel.explain(lang).is_empty());
                assert!(!tile_title(&TileDraft::new(sel).to_kind(), lang).is_empty());
            }
        }
        assert_eq!(tile_jql(&TileKind::<i64>::Favorites).as_deref(), Some(FAVORITES_JQL));
        assert_eq!(tile_jql(&TileKind::<i64>::Projects), None);
    }

    #[test]
    fn columns_are_known_unique_and_default_when_empty() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let k = known_columns(&ids(&["status", "key", "status", "nonsense", " type "]));
        assert_eq!(k.iter().map(|c| c.id).collect::<Vec<_>>(), vec!["status", "key", "type"]);
        let k = known_columns(&ids(&["nonsense"]));
        assert_eq!(k.iter().map(|c| c.id).collect::<Vec<_>>(), DEFAULT_SEARCH_COLUMNS.to_vec());
        assert_eq!(known_columns(&[]).len(), DEFAULT_SEARCH_COLUMNS.len());
        // Der Katalog deckt die Standardspalten der Datenschicht ab
        for id in DEFAULT_SEARCH_COLUMNS {
            assert!(col_info(id).is_some(), "{id}");
        }
        for c in &COLUMNS {
            assert!(!c.label(Lang::De).is_empty() && !c.label(Lang::En).is_empty());
        }
    }

    #[test]
    fn column_widths_give_the_rest_to_the_summary() {
        let cols = known_columns(&["key".into(), "summary".into(), "status".into()]);
        let w = column_widths(&cols, 600.0);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0], 66.0);
        assert_eq!(w[2], 104.0);
        assert!((w.iter().sum::<f32>() + 2.0 * CELL_GAP - 600.0).abs() < 0.01, "füllt genau: {w:?}");
        // zu schmal: alle anteilig kleiner, nie unter 24, Titel nie unter dem Minimum vor dem Verkleinern
        let narrow = column_widths(&cols, 200.0);
        assert!(narrow.iter().sum::<f32>() + 2.0 * CELL_GAP <= 200.0 + 0.5, "{narrow:?}");
        assert!(narrow.iter().all(|x| *x >= 24.0));
        // ohne Titel-Spalte: feste Breiten
        let c2 = known_columns(&["key".into(), "status".into()]);
        assert_eq!(column_widths(&c2, 900.0), vec![66.0, 104.0]);
        assert!(column_widths(&[], 100.0).is_empty());
    }

    // ---------------------------------------------------------------- Gruppen

    fn named(n: &str, c: Option<&str>) -> (Option<String>, Option<String>) {
        (Some(n.to_string()), c.map(|s| s.to_string()))
    }

    #[test]
    fn groups_sort_by_count_then_name_and_keep_colors() {
        let items = vec![named("Done", Some("#00FF00")), named("To Do", None), named("To Do", Some("#AAAAAA")), named("Done", None), named("Alpha", None), (None, None)];
        let g = group_counts(items, "(none)", "Other", MAX_GROUPS);
        let names: Vec<&str> = g.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["Done", "To Do", "(none)", "Alpha"], "Gleichstand nach Name, ohne Wert zählt wie eine Gruppe: {g:?}");
        assert_eq!(g[0], Group { name: "Done".into(), color: Some("#00FF00".into()), count: 2, none: false, other: false });
        assert_eq!(g[1].color.as_deref(), Some("#AAAAAA"), "die erste bekannte Farbe gilt");
        assert!(g[2].none);
    }

    #[test]
    fn many_groups_collapse_into_other() {
        let items: Vec<_> = (0..20).flat_map(|i| (0..(20 - i)).map(move |_| named(&format!("G{i:02}"), None))).collect();
        let total = items.len() as u32;
        let g = group_counts(items, "-", "Other", 5);
        assert_eq!(g.len(), 5);
        assert!(g[4].other && g[4].name == "Other");
        assert_eq!(g.iter().map(|x| x.count).sum::<u32>(), total, "nichts geht verloren");
        assert_eq!(g[0].name, "G00");
        assert!(group_counts(Vec::new(), "-", "Other", 5).is_empty());
        // genau max Gruppen bleiben ungekürzt
        let g = group_counts((0..5).map(|i| named(&format!("g{i}"), None)), "-", "Other", 5);
        assert_eq!(g.len(), 5);
        assert!(!g.iter().any(|x| x.other));
    }

    #[test]
    fn segment_queries_combine_with_the_tile_query() {
        assert_eq!(segment_jql("", CountGroup::Status, Some("Done")), "status = \"Done\"");
        assert_eq!(segment_jql("type = Defect", CountGroup::Status, Some("In Progress")), "(type = Defect) AND status = \"In Progress\"");
        assert_eq!(segment_jql("type = Defect ORDER BY key", CountGroup::Priority, None), "(type = Defect) AND priority IS EMPTY ORDER BY key");
        assert_eq!(segment_jql("ORDER BY updated DESC", CountGroup::Type, Some("Story")), "type = \"Story\" ORDER BY updated DESC");
        assert_eq!(segment_jql("a = \"x\" OR b = 1", CountGroup::Type, Some("Q\"uote")), "(a = \"x\" OR b = 1) AND type = \"Q\\\"uote\"");
        assert_eq!(segment_jql("((( kaputt", CountGroup::Type, Some("Bug")), "type = \"Bug\"", "unlesbare Abfrage der Kachel: nur das Segment");
        // das Ergebnis ist selbst wieder lesbar
        for q in [segment_jql("type = Defect ORDER BY key", CountGroup::Status, Some("Done")), segment_jql("", CountGroup::Priority, None)] {
            assert!(parse(&q).is_ok(), "{q}");
        }
    }

    #[test]
    fn export_file_names() {
        assert_eq!(file_slug("Mein Dashboard"), "mein-dashboard");
        assert_eq!(file_slug("  Bugs / Sprint #3 "), "bugs-sprint-3");
        assert_eq!(file_slug("Übersicht"), "übersicht");
        assert_eq!(file_slug("???"), "dashboard");
        assert_eq!(file_slug(""), "dashboard");
        assert_eq!(file_slug(r"a\b:c*d"), "a-b-c-d", "keine Zeichen, die Windows in Dateinamen verbietet");
    }

    #[test]
    fn numbers_in_labels() {
        assert_eq!(num_text(5.0, Lang::En), "5");
        assert_eq!(num_text(2.5, Lang::En), "2.5");
        assert_eq!(num_text(2.5, Lang::De), "2,5");
        assert_eq!(num_text(0.0, Lang::En), "0");
        assert_eq!(num_text(4.98, Lang::En), "5");
        assert_eq!(num_text(-1.0, Lang::En), "-1");
    }

    #[test]
    fn percent_text_rounds() {
        assert_eq!(percent_text(1, 3), "33%");
        assert_eq!(percent_text(2, 3), "67%");
        assert_eq!(percent_text(5, 5), "100%");
        assert_eq!(percent_text(0, 0), "0%");
    }

    // ---------------------------------------------------------------- Diagramme

    #[test]
    fn bar_lengths_scale_and_stay_visible() {
        assert_eq!(bar_len(10.0, 10.0, 200.0), 200.0);
        assert_eq!(bar_len(5.0, 10.0, 200.0), 100.0);
        assert_eq!(bar_len(0.0, 10.0, 200.0), 0.0);
        assert_eq!(bar_len(0.001, 1000.0, 200.0), 2.0, "kleine Werte bleiben sichtbar");
        assert_eq!(bar_len(1.0, 0.0, 200.0), 0.0, "Maximum 0: keine Teilung durch null");
        assert_eq!(bar_len(20.0, 10.0, 200.0), 200.0, "nie länger als die Breite");
        assert_eq!(bar_len(f64::NAN, 10.0, 200.0), 0.0);
        assert_eq!(bar_len(1.0, 10.0, 1.0), 1.0);
    }

    #[test]
    fn axis_maximum_is_a_round_number() {
        for (v, want) in [(0.0, 1.0), (-3.0, 1.0), (0.7, 1.0), (1.0, 1.0), (1.2, 2.0), (3.0, 5.0), (5.0, 5.0), (7.0, 10.0), (11.0, 20.0), (42.0, 50.0), (101.0, 200.0), (999.0, 1000.0), (f64::NAN, 1.0), (f64::INFINITY, 1.0)] {
            assert_eq!(nice_max(v), want, "{v}");
        }
        assert_eq!(axis_ticks(10.0, 5), vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(axis_ticks(10.0, 0).len(), 2, "mindestens ein Schritt");
    }

    #[test]
    fn pie_angles_cover_the_circle_in_order() {
        assert!(pie_angles(&[]).is_empty());
        assert!(pie_angles(&[0, 0]).is_empty());
        let a = pie_angles(&[1, 1, 2]);
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].0, 0.0);
        assert!((a[0].1 - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!((a[2].1 - std::f32::consts::TAU).abs() < 1e-5);
        for w in a.windows(2) {
            assert!((w[0].1 - w[1].0).abs() < 1e-6, "lückenlos");
        }
    }

    #[test]
    fn pie_hit_finds_the_segment_under_the_pointer() {
        let c = Pos2::new(100.0, 100.0);
        // Segmente: 50 % (rechte Hälfte, ab 12 Uhr im Uhrzeigersinn), 25 % (unten links), 25 % (oben links)
        let counts = [2, 1, 1];
        assert_eq!(pie_hit(c, 50.0, Pos2::new(130.0, 100.0), &counts), Some(0), "rechts");
        assert_eq!(pie_hit(c, 50.0, Pos2::new(101.0, 70.0), &counts), Some(0), "kurz nach 12 Uhr");
        assert_eq!(pie_hit(c, 50.0, Pos2::new(80.0, 130.0), &counts), Some(1), "unten links");
        assert_eq!(pie_hit(c, 50.0, Pos2::new(70.0, 80.0), &counts), Some(2), "oben links");
        assert_eq!(pie_hit(c, 50.0, Pos2::new(200.0, 100.0), &counts), None, "außerhalb");
        assert_eq!(pie_hit(c, 50.0, c, &[0, 0]), None, "leer");
        // Segment mit 0 wird nie getroffen
        assert_eq!(pie_hit(c, 50.0, Pos2::new(130.0, 100.0), &[0, 3]), Some(1));
    }

    #[test]
    fn slice_colors_use_the_group_color_or_the_palette() {
        assert_eq!(slice_color(0, Some("#112233")), Color32::from_rgb(0x11, 0x22, 0x33));
        let a = slice_color(0, None);
        assert_eq!(slice_color(super::super::helpers::PALETTE.len(), None), a, "die Palette läuft im Kreis");
        assert_ne!(slice_color(1, None), a);
        assert_eq!(slice_color(2, Some("kaputt")), slice_color(2, None));
    }

    // ---------------------------------------------------------------- Zeit

    #[test]
    fn time_windows() {
        // Mittwoch 2026-11-11
        let today = d(2026, 11, 11);
        assert_eq!(time_window(today, TimeRange::Day), (d(2026, 10, 29), today));
        let (from, to) = time_window(today, TimeRange::Week);
        assert_eq!(to, d(2026, 11, 15), "Sonntag der laufenden Woche");
        assert_eq!(from, d(2026, 9, 21), "Montag, sieben Wochen davor");
        assert_eq!(from.weekday(), chrono::Weekday::Mon);
    }

    #[test]
    fn day_buckets_cover_fourteen_days_with_labels() {
        let today = d(2026, 11, 11);
        let mut m = BTreeMap::new();
        m.insert(today, 90);
        m.insert(d(2026, 11, 10), 30);
        m.insert(d(2026, 10, 1), 500); // außerhalb
        let b = bucket_time(&m, today, TimeRange::Day, Lang::En);
        assert_eq!(b.len(), 14);
        assert_eq!(b[13], Bucket { start: today, label: "Wed 11".into(), minutes: 90 });
        assert_eq!(b[12].minutes, 30);
        assert_eq!(b.iter().map(|x| x.minutes).sum::<u32>(), 120);
        assert_eq!(bucket_time(&m, today, TimeRange::Day, Lang::De)[13].label, "Mi 11");
    }

    #[test]
    fn week_buckets_sum_up_monday_to_sunday() {
        let today = d(2026, 11, 11);
        let mut m = BTreeMap::new();
        m.insert(d(2026, 11, 9), 60); // Montag dieser Woche
        m.insert(d(2026, 11, 15), 40); // Sonntag dieser Woche
        m.insert(d(2026, 11, 8), 20); // Sonntag davor
        let b = bucket_time(&m, today, TimeRange::Week, Lang::En);
        assert_eq!(b.len(), 8);
        assert_eq!(b[7].start, d(2026, 11, 9));
        assert_eq!(b[7].minutes, 100);
        assert_eq!(b[6].minutes, 20);
        assert_eq!(b[7].label, "W46");
        assert_eq!(bucket_time(&m, today, TimeRange::Week, Lang::De)[7].label, "KW46");
        m.insert(d(2026, 11, 10), u32::MAX);
        assert_eq!(bucket_time(&m, today, TimeRange::Week, Lang::En)[7].minutes, u32::MAX, "läuft nicht über");
    }

    #[test]
    fn short_dates_per_language() {
        assert_eq!(short_date(d(2026, 11, 9), Lang::De), "9.11.");
        assert_eq!(short_date(d(2026, 11, 9), Lang::En), "Nov 9");
        assert_eq!(short_date(d(2026, 1, 31), Lang::En), "Jan 31");
    }

    // ---------------------------------------------------------------- Zwischenspeicher

    #[test]
    fn cache_reloads_on_change_generation_or_age() {
        let ttl = Duration::from_secs(8);
        let m = CacheMeta { key: 7, generation: 3 };
        assert!(needs_reload(None, 7, 3, Duration::ZERO, ttl), "noch nichts da");
        assert!(!needs_reload(Some(m), 7, 3, Duration::from_secs(2), ttl), "frisch und unverändert: nicht neu fragen");
        assert!(needs_reload(Some(m), 8, 3, Duration::ZERO, ttl), "Einstellungen geändert");
        assert!(needs_reload(Some(m), 7, 4, Duration::ZERO, ttl), "Daten geändert (Stand erhöht)");
        assert!(needs_reload(Some(m), 7, 3, ttl, ttl), "abgelaufen");
        assert!(needs_reload(Some(m), 7, 3, Duration::from_secs(60), ttl));
    }

    #[test]
    fn tile_key_changes_with_every_setting() {
        let k = |kind: &TileKind<i64>, p: Option<i64>, l: Lang| tile_key(kind, p, l);
        let s = |jql: &str| TileKind::<i64>::Search { jql: jql.into(), columns: vec!["key".into()], title: String::new() };
        let base = k(&s("a"), None, Lang::En);
        assert_eq!(base, k(&s("a"), None, Lang::En), "stabil");
        assert_ne!(base, k(&s("b"), None, Lang::En));
        assert_ne!(base, k(&s("a"), Some(1), Lang::En));
        assert_ne!(base, k(&s("a"), None, Lang::De));
        assert_ne!(k(&TileKind::Sprint { project_id: Some(1) }, None, Lang::En), k(&TileKind::Sprint { project_id: Some(2) }, None, Lang::En));
        assert_ne!(k(&TileKind::Favorites, None, Lang::En), k(&TileKind::Flagged, None, Lang::En));
    }

    #[test]
    fn load_budget_prefers_tiles_without_data() {
        let pending = [(1, true), (2, false), (3, true), (4, false)];
        assert_eq!(pick_loads(&pending, 2), vec![2, 4]);
        assert_eq!(pick_loads(&pending, 3), vec![2, 4, 1]);
        assert_eq!(pick_loads(&pending, 10), vec![2, 4, 1, 3]);
        assert!(pick_loads(&pending, 0).is_empty());
        assert!(pick_loads(&[], 2).is_empty());
    }

    // ---------------------------------------------------------------- Entwurf

    #[test]
    fn drafts_round_trip_every_kind() {
        let kinds: Vec<TileKind<i64>> = vec![
            TileKind::Search { jql: "type = Defect".into(), columns: vec!["key".into(), "status".into()], title: "Bugs".into() },
            TileKind::Counts { jql: "x = 1".into(), group_by: CountGroup::Priority, chart: ChartKind::Bar },
            TileKind::Sprint { project_id: Some(3) },
            TileKind::Burndown { project_id: None },
            TileKind::TimeTracking { range: TimeRange::Day, project_id: Some(2) },
            TileKind::Favorites,
            TileKind::Flagged,
            TileKind::Projects,
        ];
        for (i, k) in kinds.into_iter().enumerate() {
            let t = Tile { id: i as u32 + 1, x: 1, y: 2, w: 5, h: 6, kind: k.clone() };
            let d = TileDraft::from_tile(&t);
            assert_eq!(d.to_kind(), k);
            assert_eq!((d.x, d.y, d.w, d.h), (1, 2, 5, 6));
            assert_eq!(d.sel, KindSel::of(&k));
        }
        // Eingaben werden zurechtgestutzt
        let mut d = TileDraft::new(KindSel::Search);
        d.title = "  Titel  ".into();
        d.jql = "  type = Task ".into();
        assert!(matches!(d.to_kind(), TileKind::Search { jql, title, .. } if jql == "type = Task" && title == "Titel"));
        assert!(d.has_query() && !d.needs_project());
        assert!(TileDraft::new(KindSel::Sprint).needs_project() && !TileDraft::new(KindSel::Sprint).has_query());
        // Standardgrößen kommen von der Datenschicht
        assert_eq!((TileDraft::new(KindSel::Search).w, TileDraft::new(KindSel::Search).h), (12, 12));
        assert_eq!((TileDraft::new(KindSel::Counts).w, TileDraft::new(KindSel::Counts).h), (4, 8));
    }

    #[test]
    fn draft_columns_toggle_and_move() {
        let mut d = TileDraft::new(KindSel::Search);
        d.columns = vec!["key".into(), "summary".into()];
        d.toggle_column("status");
        assert_eq!(d.columns, vec!["key", "summary", "status"]);
        d.toggle_column("key");
        assert_eq!(d.columns, vec!["summary", "status"]);
        assert!(d.move_column("status", -1));
        assert_eq!(d.columns, vec!["status", "summary"]);
        assert!(!d.move_column("status", -1), "am Anfang");
        assert!(!d.move_column("nope", 1));
    }

    #[test]
    fn applying_a_draft_adds_and_edits_tiles() {
        let c = db();
        let mut l = default_dashboard_layout();
        let mut d = TileDraft::new(KindSel::Counts);
        let id = apply_draft(&mut l, None, &d).unwrap();
        assert_eq!(l.tiles.len(), 2);
        validate_layout(&c, None, &l).unwrap();
        let t = l.tile(id).unwrap();
        assert_eq!((t.w, t.h), (4, 8));
        assert!(t.y >= 12, "unter der großen Such-Kachel: erste freie Stelle");
        // ändern: Art, Platz und Größe; die Such-Kachel weicht aus
        d = TileDraft::from_tile(t);
        d.sel = KindSel::Flagged;
        d.x = 0;
        d.y = 0;
        d.w = 6;
        d.h = 5;
        assert_eq!(apply_draft(&mut l, Some(id), &d), Some(id));
        let t = l.tile(id).unwrap();
        assert_eq!((t.kind.clone(), t.x, t.y, t.w, t.h), (TileKind::Flagged, 0, 0, 6, 5));
        validate_layout(&c, None, &l).unwrap();
        assert!(l.tile(1).unwrap().y >= 5, "die andere weicht nach unten aus");
        assert_eq!(apply_draft(&mut l, Some(99), &d), None, "unbekannte Kachel");
        // zu viele Kacheln
        for _ in 0..60 {
            if apply_draft(&mut l, None, &TileDraft::new(KindSel::Favorites)).is_none() {
                break;
            }
        }
        assert_eq!(l.tiles.len(), crate::planning::views::dashboards::MAX_TILES);
        assert_eq!(apply_draft(&mut l, None, &TileDraft::new(KindSel::Favorites)), None);
        // Größen außerhalb des Rasters werden zurechtgestutzt
        let mut l = layout(vec![]);
        let mut d = TileDraft::new(KindSel::Favorites);
        d.w = 99;
        d.h = 0;
        let id = apply_draft(&mut l, None, &d).unwrap();
        assert_eq!((l.tile(id).unwrap().w, l.tile(id).unwrap().h), (GRID_COLUMNS, 1));
    }

    // ---------------------------------------------------------------- Vorlagen

    #[test]
    fn overview_templates_are_valid_in_every_scope() {
        let c = db();
        let p = project(&c, "AA", false);
        for lang in [Lang::De, Lang::En] {
            for (sprints, global) in [(false, false), (true, false), (false, true), (true, true)] {
                let l = overview_layout(lang, sprints, global);
                let scope = if global { None } else { Some(p) };
                validate_layout(&c, scope, &l).unwrap_or_else(|e| panic!("{sprints} {global}: {e:?}"));
                assert!(l.tiles.len() >= 6);
                let has = |f: fn(&TileKind<i64>) -> bool| l.tiles.iter().any(|t| f(&t.kind));
                assert_eq!(has(|k| matches!(k, TileKind::Sprint { .. })), sprints && !global);
                assert_eq!(has(|k| matches!(k, TileKind::Burndown { .. })), sprints && !global);
                assert_eq!(has(|k| matches!(k, TileKind::Projects)), global);
                assert!(has(|k| matches!(k, TileKind::Counts { group_by: CountGroup::Status, .. })));
                assert!(has(|k| matches!(k, TileKind::Favorites)));
                assert!(has(|k| matches!(k, TileKind::Flagged)));
                // alle Spalten der Such-Kachel kennt die Oberfläche
                for t in &l.tiles {
                    if let TileKind::Search { columns, .. } = &t.kind {
                        assert!(columns.iter().all(|c| col_info(c).is_some()));
                    }
                }
            }
        }
        let l = overview_layout(Lang::De, false, false);
        assert!(l.tiles.iter().any(|t| matches!(&t.kind, TileKind::Search { title, .. } if title == "Zuletzt geändert")));
    }
}
