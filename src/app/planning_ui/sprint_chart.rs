//! Diagramme der Sprint-Ansicht: Burndown (Ist-Linie als Treppe, Ideal-Linie, Wochenenden, Umfangsänderungen als Sprünge)
//! und Velocity (Balken geplant gegen erledigt, Durchschnitt). Die Geometrie (Skalierung, Achsen, Punkte) ist reine Rechnung
//! auf einem Rechteck und gut testbar; gezeichnet wird mit dem egui-Painter, ohne zusätzliche Abhängigkeit.

use crate::calc::burndown::{average_velocity, BurndownSeries, SprintResult, VelocityPoint};
use crate::calc::calendar::WorkCalendar;
use crate::state::Lang;
use crate::style;
use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, Ui, Vec2};

/// Rand um die Zeichenfläche für Achsenbeschriftungen.
pub const PAD_L: f32 = 40.0;
pub const PAD_R: f32 = 12.0;
pub const PAD_T: f32 = 10.0;
pub const PAD_B: f32 = 26.0;

/// Farbe für Umfangszuwachs (Sprünge nach oben).
pub const SCOPE_UP: Color32 = Color32::from_rgb(0xe5, 0xa0, 0x4b);
/// Farbe der erledigten Balken.
pub const DONE_GREEN: Color32 = Color32::from_rgb(0x7f, 0xbf, 0x6a);

// ------------------------------------------------------------------ Achsen

/// Senkrechte Achse: oberer Wert und Teilstrich-Abstand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub max: f64,
    pub step: f64,
}

impl Scale {
    pub fn ticks(&self) -> usize {
        ((self.max / self.step).round() as usize).max(1)
    }
}

/// „Schöne“ Achse für Werte von 0 bis `max_value`: höchstens etwa fünf Teilstriche, Schritte 1/2/5 × 10^k. Mit `integer` sind die Schritte
/// ganze Zahlen (Anzahl von Tickets). Leere oder ungültige Werte ergeben eine Achse von 0 bis 4.
pub fn nice_scale(max_value: f64, integer: bool) -> Scale {
    let m = if max_value.is_finite() && max_value > 0.0 { max_value } else { 0.0 };
    if m == 0.0 {
        return Scale { max: 4.0, step: 1.0 };
    }
    let raw = m / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let mut step = mag * if norm <= 1.0 { 1.0 } else if norm <= 2.0 { 2.0 } else if norm <= 5.0 { 5.0 } else { 10.0 };
    if integer {
        step = step.ceil().max(1.0);
    }
    let max = (m / step - 1e-9).ceil().max(1.0) * step;
    Scale { max, step }
}

/// Sind alle Werte (nahezu) ganze Zahlen?
pub fn all_whole(values: impl IntoIterator<Item = f64>) -> bool {
    values.into_iter().all(|v| (v - v.round()).abs() < 1e-9)
}

/// Beschriftung eines Teilstrichs („12“, „2.5“).
pub fn tick_label(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v:.1}") }
}

/// Wie viele Tage zwischen zwei Datumsbeschriftungen liegen, damit sie sich bei `px_per_day` Pixeln je Tag nicht überlappen.
pub fn label_every(px_per_day: f32, min_px: f32) -> usize {
    if px_per_day <= 0.0 {
        return 1;
    }
    ((min_px / px_per_day).ceil() as usize).max(1)
}

// ------------------------------------------------------------------ Burndown

/// Die Linien des Burndown-Diagramms in Bildschirmkoordinaten.
#[derive(Debug, Clone, PartialEq)]
pub struct BurnPlot {
    pub plot: Rect,
    pub scale: Scale,
    /// Ideal-Linie (am Wochenende waagerecht): Start und Tagesende jedes Tages.
    pub ideal: Vec<Pos2>,
    /// Ist-Linie als Treppe: am Ende jedes vergangenen Tages ein senkrechter Schritt auf den neuen Rest.
    pub actual: Vec<Pos2>,
    /// Senkrechte Schritte nach oben (Umfangszuwachs oder wieder geöffnet), als Strecken.
    pub ups: Vec<[Pos2; 2]>,
    /// Freie Tage (Wochenende, Feiertag): von/bis x.
    pub off_days: Vec<(f32, f32)>,
    /// Waagerechte Hilfslinien: Höhe und Wert.
    pub y_ticks: Vec<(f32, f64)>,
    /// Datumsbeschriftungen: x (Mitte des Tages) und Datum.
    pub x_labels: Vec<(f32, NaiveDate)>,
    /// x der Mitte des heutigen Tages, falls er im Sprint liegt.
    pub today_x: Option<f32>,
    /// Breite eines Tages.
    pub day_w: f32,
    pub days: usize,
}

/// Rechnet die Reihen des Burndowns auf das Rechteck `plot` um.
pub fn burn_plot(series: &BurndownSeries, cal: &WorkCalendar, plot: Rect, today: NaiveDate) -> BurnPlot {
    let n = series.days.len();
    let top_val = series.ideal.iter().copied().chain(series.actual.iter().flatten().copied()).chain([series.scope_total_start]).fold(0.0f64, f64::max);
    let integer = all_whole(series.ideal.iter().copied().chain(series.actual.iter().flatten().copied()).chain([series.scope_total_start]));
    let scale = nice_scale(top_val, integer);
    let day_w = if n == 0 { 0.0 } else { plot.width() / n as f32 };
    let x_edge = |edge: usize| plot.left() + day_w * edge as f32;
    let y_of = |v: f64| plot.bottom() - plot.height() * (v / scale.max).clamp(0.0, 1.0) as f32;
    let y_ticks = (0..=scale.ticks()).map(|i| (y_of(scale.step * i as f64), scale.step * i as f64)).collect();
    let mut p = BurnPlot { plot, scale, ideal: vec![], actual: vec![], ups: vec![], off_days: vec![], y_ticks, x_labels: vec![], today_x: None, day_w, days: n };
    if n == 0 {
        return p;
    }
    p.ideal.push(Pos2::new(x_edge(0), y_of(series.scope_total_start)));
    for (i, v) in series.ideal.iter().enumerate() {
        p.ideal.push(Pos2::new(x_edge(i + 1), y_of(*v)));
    }
    // Ist-Linie: Start beim Startumfang, dann je Tag waagerecht bis zum Tagesende und senkrecht auf den neuen Rest.
    let mut prev = series.scope_total_start;
    p.actual.push(Pos2::new(x_edge(0), y_of(prev)));
    for (i, v) in series.actual.iter().enumerate() {
        let Some(v) = v else { break };
        let x = x_edge(i + 1);
        p.actual.push(Pos2::new(x, y_of(prev)));
        p.actual.push(Pos2::new(x, y_of(*v)));
        if *v > prev + 1e-9 {
            p.ups.push([Pos2::new(x, y_of(prev)), Pos2::new(x, y_of(*v))]);
        }
        prev = *v;
    }
    for (i, day) in series.days.iter().enumerate() {
        if !cal.is_work_day(*day) {
            p.off_days.push((x_edge(i), x_edge(i + 1)));
        }
        if *day == today {
            p.today_x = Some((x_edge(i) + x_edge(i + 1)) / 2.0);
        }
    }
    let every = label_every(day_w, 46.0);
    for (i, day) in series.days.iter().enumerate() {
        if i % every == 0 {
            p.x_labels.push(((x_edge(i) + x_edge(i + 1)) / 2.0, *day));
        }
    }
    p
}

/// Welcher Tag (Index) liegt unter dem Mauszeiger?
pub fn day_at(plot: Rect, days: usize, x: f32) -> Option<usize> {
    if days == 0 || x < plot.left() || x > plot.right() {
        return None;
    }
    Some((((x - plot.left()) / plot.width() * days as f32) as usize).min(days - 1))
}

/// Das heutige Datum für die Ist-Linie: beim laufenden Sprint heute, beim abgeschlossenen das Sprintende (die ganze Linie).
pub fn chart_today(completed: bool, end: NaiveDate, today: NaiveDate) -> NaiveDate {
    if completed { end } else { today }
}

/// Kurzes Datum für die Achse: „2.11.“ / „11/02“.
pub fn axis_date(d: NaiveDate, lang: Lang) -> String {
    match lang {
        Lang::De => format!("{}.{}.", d.day(), d.month()),
        Lang::En => format!("{:02}-{:02}", d.month(), d.day()),
    }
}

/// Text beim Überfahren eines Tages.
pub fn burn_tooltip(series: &BurndownSeries, i: usize, basis_unit: &str, lang: Lang) -> Option<String> {
    let day = series.days.get(i)?;
    let ideal = tick_label(*series.ideal.get(i)?);
    let actual = match series.actual.get(i)? {
        Some(v) => tick_label(*v),
        None => "–".to_string(),
    };
    Some(match lang {
        Lang::De => format!("{}\nIdeal: {ideal} {basis_unit}\nIst: {actual} {basis_unit}", super::sprint_logic::fmt_date_ui(*day, lang)),
        Lang::En => format!("{}\nIdeal: {ideal} {basis_unit}\nActual: {actual} {basis_unit}", super::sprint_logic::fmt_date_ui(*day, lang)),
    })
}

fn grid_color(ui: &Ui) -> Color32 {
    ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7)
}

/// Zeichnet Achsen, Gitter und die Beschriftungen (gemeinsam für beide Diagramme).
fn paint_axes(ui: &Ui, plot: Rect, y_ticks: &[(f32, f64)]) {
    let painter = ui.painter();
    let dim = style::dim(ui);
    let grid = grid_color(ui);
    for (y, v) in y_ticks {
        painter.line_segment([Pos2::new(plot.left(), *y), Pos2::new(plot.right(), *y)], Stroke::new(1.0_f32, grid));
        painter.text(Pos2::new(plot.left() - 6.0, *y), Align2::RIGHT_CENTER, tick_label(*v), FontId::proportional(11.0), dim);
    }
    painter.line_segment([Pos2::new(plot.left(), plot.bottom()), Pos2::new(plot.right(), plot.bottom())], Stroke::new(1.0_f32, dim.gamma_multiply(0.8)));
}

/// Zeichnet das Burndown-Diagramm in `outer` (mit Rand für die Achsen).
pub fn paint_burndown(ui: &Ui, outer: Rect, series: &BurndownSeries, cal: &WorkCalendar, today: NaiveDate, lang: Lang) -> BurnPlot {
    let plot = Rect::from_min_max(Pos2::new(outer.left() + PAD_L, outer.top() + PAD_T), Pos2::new(outer.right() - PAD_R, outer.bottom() - PAD_B));
    let bp = burn_plot(series, cal, plot, today);
    let painter = ui.painter();
    let dim = style::dim(ui);
    // freie Tage
    for (a, b) in &bp.off_days {
        painter.rect_filled(Rect::from_min_max(Pos2::new(*a, plot.top()), Pos2::new(*b, plot.bottom())), 0.0, dim.gamma_multiply(0.10));
    }
    paint_axes(ui, plot, &bp.y_ticks);
    for (x, d) in &bp.x_labels {
        painter.text(Pos2::new(*x, plot.bottom() + 6.0), Align2::CENTER_TOP, axis_date(*d, lang), FontId::proportional(11.0), dim);
    }
    if let Some(x) = bp.today_x {
        painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0_f32, style::accent().gamma_multiply(0.5)));
    }
    if bp.ideal.len() > 1 {
        painter.add(egui::Shape::line(bp.ideal.clone(), Stroke::new(1.6_f32, dim.gamma_multiply(1.1))));
    }
    if bp.actual.len() > 1 {
        painter.add(egui::Shape::line(bp.actual.clone(), Stroke::new(2.2_f32, style::accent())));
    }
    for seg in &bp.ups {
        painter.line_segment(*seg, Stroke::new(2.6_f32, SCOPE_UP));
        painter.circle_filled(seg[1], 3.0, SCOPE_UP);
    }
    if let Some(last) = bp.actual.last() {
        painter.circle_filled(*last, 3.5, style::accent());
    }
    bp
}

// ------------------------------------------------------------------ Velocity

/// Ein Balkenpaar des Velocity-Diagramms.
#[derive(Debug, Clone, PartialEq)]
pub struct VelBar {
    pub planned: Rect,
    pub completed: Rect,
    /// Mitte des Platzes (für die Beschriftung).
    pub center_x: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VelPlot {
    pub plot: Rect,
    pub scale: Scale,
    pub bars: Vec<VelBar>,
    pub y_ticks: Vec<(f32, f64)>,
    /// Höhe der Durchschnittslinie (Durchschnitt des Erledigten aller gezeigten Sprints).
    pub avg: Option<(f32, f64)>,
}

/// Durchschnitt des Erledigten über die gezeigten Sprints (`calc::burndown::average_velocity`).
pub fn velocity_average(points: &[VelocityPoint]) -> Option<f64> {
    let results: Vec<SprintResult> = points.iter().map(|p| SprintResult { label: p.label.clone(), planned: p.planned, completed: p.completed }).collect();
    average_velocity(&results, results.len())
}

/// Rechnet die Balken auf das Rechteck `plot` um: je Sprint ein Platz mit zwei Balken (geplant links, erledigt rechts).
pub fn velocity_plot(points: &[VelocityPoint], plot: Rect) -> VelPlot {
    let top_val = points.iter().fold(0.0f64, |m, p| m.max(p.planned).max(p.completed));
    let integer = all_whole(points.iter().flat_map(|p| [p.planned, p.completed]));
    let scale = nice_scale(top_val, integer);
    let y_of = |v: f64| plot.bottom() - plot.height() * (v / scale.max).clamp(0.0, 1.0) as f32;
    let y_ticks = (0..=scale.ticks()).map(|i| (y_of(scale.step * i as f64), scale.step * i as f64)).collect();
    let n = points.len();
    let slot = if n == 0 { 0.0 } else { plot.width() / n as f32 };
    let bar_w = (slot * 0.32).clamp(4.0, 34.0);
    let gap = 3.0;
    let bars = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let cx = plot.left() + slot * (i as f32 + 0.5);
            let mk = |x0: f32, v: f64| Rect::from_min_max(Pos2::new(x0, y_of(v)), Pos2::new(x0 + bar_w, plot.bottom()));
            VelBar { planned: mk(cx - gap / 2.0 - bar_w, p.planned), completed: mk(cx + gap / 2.0, p.completed), center_x: cx }
        })
        .collect();
    let avg = velocity_average(points).map(|a| (y_of(a), a));
    VelPlot { plot, scale, bars, y_ticks, avg }
}

/// Welcher Sprint (Index) liegt unter `x`?
pub fn bar_at(plot: Rect, n: usize, x: f32) -> Option<usize> {
    day_at(plot, n, x)
}

/// Text beim Überfahren eines Sprints im Velocity-Diagramm.
pub fn velocity_tooltip(p: &VelocityPoint, unit: &str, lang: Lang) -> String {
    let ratio = p.completion_ratio.map(|r| format!(" ({} %)", (r * 100.0).round() as i64)).unwrap_or_default();
    match lang {
        Lang::De => format!("{}\nGeplant: {} {unit}\nErledigt: {} {unit}{ratio}", p.label, tick_label(p.planned), tick_label(p.completed)),
        Lang::En => format!("{}\nPlanned: {} {unit}\nCompleted: {} {unit}{ratio}", p.label, tick_label(p.planned), tick_label(p.completed)),
    }
}

/// Zeichnet das Velocity-Diagramm in `outer`.
pub fn paint_velocity(ui: &Ui, outer: Rect, points: &[VelocityPoint], lang: Lang) -> VelPlot {
    let plot = Rect::from_min_max(Pos2::new(outer.left() + PAD_L, outer.top() + PAD_T), Pos2::new(outer.right() - PAD_R, outer.bottom() - PAD_B));
    let vp = velocity_plot(points, plot);
    let painter = ui.painter();
    let dim = style::dim(ui);
    paint_axes(ui, plot, &vp.y_ticks);
    for (bar, p) in vp.bars.iter().zip(points) {
        painter.rect_filled(bar.planned, 2.0, dim.gamma_multiply(0.55));
        painter.rect_filled(bar.completed, 2.0, DONE_GREEN);
        let label = if p.label.chars().count() > 12 { format!("{}…", p.label.chars().take(11).collect::<String>()) } else { p.label.clone() };
        painter.text(Pos2::new(bar.center_x, plot.bottom() + 6.0), Align2::CENTER_TOP, label, FontId::proportional(11.0), dim);
    }
    if let Some((y, a)) = vp.avg {
        painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.4_f32, style::accent()));
        let text = match lang {
            Lang::De => format!("Ø {}", tick_label((a * 10.0).round() / 10.0)),
            Lang::En => format!("avg {}", tick_label((a * 10.0).round() / 10.0)),
        };
        painter.text(Pos2::new(plot.right() - 2.0, y - 3.0), Align2::RIGHT_BOTTOM, text, FontId::proportional(11.0), style::accent());
    }
    vp
}

/// Legendeneintrag: Farbfeld (oder Linienstück) und Text.
pub fn legend_item(ui: &mut Ui, color: Color32, text: &str, line: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (r, _) = ui.allocate_exact_size(Vec2::new(16.0, 12.0), egui::Sense::hover());
        if line {
            ui.painter().line_segment([Pos2::new(r.left(), r.center().y), Pos2::new(r.right(), r.center().y)], Stroke::new(2.4_f32, color));
        } else {
            ui.painter().rect_filled(Rect::from_center_size(r.center(), Vec2::new(10.0, 10.0)), 2.0, color);
        }
        ui.label(egui::RichText::new(text).small().color(style::dim(ui)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::burndown::{compute_burndown, ScopeEvent, SprintSpec};
    use crate::calc::progress::Basis;
    use chrono::NaiveDateTime;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn at(m: u32, day: u32, h: u32) -> NaiveDateTime {
        d(2026, m, day).and_hms_opt(h, 0, 0).unwrap()
    }

    fn rect() -> Rect {
        Rect::from_min_max(Pos2::new(100.0, 20.0), Pos2::new(500.0, 220.0))
    }

    /// Zwei Wochen (Mo 2.11. bis Fr 13.11.), 10 Punkte Startumfang, bis Mittwoch der zweiten Woche gerechnet.
    fn series() -> BurndownSeries {
        let spec = SprintSpec { start: d(2026, 11, 2), end: d(2026, 11, 13), calendar: WorkCalendar::default() };
        let events = vec![
            ScopeEvent::Added { at: at(11, 1, 9), points: Some(10.0) },
            ScopeEvent::Completed { at: at(11, 3, 10), points: Some(4.0) },
            ScopeEvent::Added { at: at(11, 4, 10), points: Some(2.0) },
            ScopeEvent::Completed { at: at(11, 10, 10), points: Some(3.0) },
        ];
        compute_burndown(&spec, &events, Basis::Points, d(2026, 11, 11))
    }

    #[test]
    fn scale_picks_round_steps() {
        assert_eq!(nice_scale(0.0, true), Scale { max: 4.0, step: 1.0 });
        assert_eq!(nice_scale(f64::NAN, false), Scale { max: 4.0, step: 1.0 });
        assert_eq!(nice_scale(-3.0, false), Scale { max: 4.0, step: 1.0 });
        assert_eq!(nice_scale(34.0, true), Scale { max: 40.0, step: 10.0 });
        assert_eq!(nice_scale(8.0, true), Scale { max: 8.0, step: 2.0 });
        assert_eq!(nice_scale(3.0, true), Scale { max: 3.0, step: 1.0 });
        assert_eq!(nice_scale(1.0, true), Scale { max: 1.0, step: 1.0 }, "ganze Schritte bei Anzahlen");
        assert_eq!(nice_scale(100.0, true), Scale { max: 100.0, step: 50.0 });
        assert_eq!(nice_scale(1.0, false), Scale { max: 1.0, step: 0.5 });
        let s = nice_scale(7.5, false);
        assert!(s.max >= 7.5 && s.ticks() <= 6, "{s:?}");
    }

    #[test]
    fn scale_always_covers_the_value_with_few_ticks() {
        for v in [0.3, 1.0, 2.0, 5.0, 7.0, 9.0, 13.0, 21.0, 34.0, 55.0, 89.0, 144.0, 1000.0, 12345.0] {
            for integer in [false, true] {
                let s = nice_scale(v, integer);
                assert!(s.max >= v - 1e-9, "{v} {integer}: {s:?}");
                assert!(s.ticks() <= 8 && s.ticks() >= 1, "{v} {integer}: {s:?}");
                if integer {
                    assert_eq!(s.step.fract(), 0.0);
                }
            }
        }
    }

    #[test]
    fn whole_numbers_and_labels() {
        assert!(all_whole([1.0, 2.0, 3.0000000001]));
        assert!(!all_whole([1.0, 2.5]));
        assert!(all_whole(std::iter::empty()));
        assert_eq!(tick_label(12.0), "12");
        assert_eq!(tick_label(2.5), "2.5");
        assert_eq!(label_every(40.0, 46.0), 2);
        assert_eq!(label_every(100.0, 46.0), 1);
        assert_eq!(label_every(10.0, 46.0), 5);
        assert_eq!(label_every(0.0, 46.0), 1);
    }

    #[test]
    fn burndown_geometry_follows_the_series() {
        let s = series();
        assert_eq!(s.scope_total_start, 10.0);
        let p = burn_plot(&s, &WorkCalendar::default(), rect(), d(2026, 11, 11));
        assert_eq!(p.days, 12);
        let scale = nice_scale(10.0, true).max;
        assert_eq!(p.scale.max, scale);
        // Ideal: Startpunkt + ein Punkt je Tag; am letzten Tag unten
        assert_eq!(p.ideal.len(), 13);
        assert_eq!(p.ideal[0].x, 100.0);
        assert_eq!(p.ideal[12].x, 500.0);
        assert_eq!(p.ideal[12].y, 220.0, "am letzten Tag ist der Ideal-Rest 0");
        // Am Wochenende (Tag 5 und 6 = Sa/So) bleibt die Ideal-Linie waagerecht
        assert_eq!(p.ideal[5].y, p.ideal[6].y);
        assert_eq!(p.ideal[6].y, p.ideal[7].y);
        assert_ne!(p.ideal[4].y, p.ideal[5].y, "am Freitag fällt sie noch");
        // freie Tage: Samstag und Sonntag der ersten Woche
        assert_eq!(p.off_days.len(), 2);
        // heute liegt im Sprint
        assert!(p.today_x.is_some());
    }

    #[test]
    fn actual_line_is_a_staircase_with_visible_scope_jumps() {
        let s = series();
        let p = burn_plot(&s, &WorkCalendar::default(), rect(), d(2026, 11, 11));
        // Reste am Tagesende: Mo 10, Di 6 (3.11. erledigt 4), Mi 8 (+2), Do 8, Fr 8, Sa 8, So 8, Mo 8, Di 5 (10.11.), Mi 5
        let rest: Vec<f64> = s.actual.iter().flatten().copied().collect();
        assert_eq!(rest, vec![10.0, 6.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 5.0, 5.0]);
        // Start + 2 Punkte je Tag mit Wert
        assert_eq!(p.actual.len(), 1 + 2 * rest.len());
        // Treppe: je zwei aufeinanderfolgende Punkte haben gleiches x (senkrecht) oder gleiches y (waagerecht)
        for w in p.actual.windows(2) {
            assert!(w[0].x == w[1].x || w[0].y == w[1].y, "{w:?}");
        }
        // Umfangszuwachs: genau ein Sprung nach oben (Mittwoch, +2)
        assert_eq!(p.ups.len(), 1);
        let up = p.ups[0];
        assert_eq!(up[0].x, up[1].x);
        assert!(up[1].y < up[0].y, "nach oben = kleineres y");
        assert_eq!(up[0].x, 100.0 + 3.0 * p.day_w, "am Ende des dritten Tages");
        // nach heute gibt es keine Punkte mehr
        assert_eq!(p.actual.last().unwrap().x, 100.0 + 10.0 * p.day_w);
    }

    #[test]
    fn empty_series_gives_empty_geometry() {
        let spec = SprintSpec { start: d(2026, 11, 13), end: d(2026, 11, 2), calendar: WorkCalendar::default() };
        let s = compute_burndown(&spec, &[], Basis::Points, d(2026, 11, 5));
        let p = burn_plot(&s, &WorkCalendar::default(), rect(), d(2026, 11, 5));
        assert!(p.ideal.is_empty() && p.actual.is_empty() && p.x_labels.is_empty());
        assert_eq!(p.scale, Scale { max: 4.0, step: 1.0 });
        assert_eq!(day_at(rect(), 0, 200.0), None);
    }

    #[test]
    fn axis_labels_do_not_overlap() {
        let s = series();
        let p = burn_plot(&s, &WorkCalendar::default(), rect(), d(2026, 11, 11));
        assert!(p.x_labels.len() >= 2);
        for w in p.x_labels.windows(2) {
            assert!(w[1].0 - w[0].0 >= 45.0, "{w:?}");
        }
        // Y-Linien: aufsteigend von unten nach oben
        for w in p.y_ticks.windows(2) {
            assert!(w[1].0 < w[0].0);
        }
        assert_eq!(p.y_ticks[0].1, 0.0);
        assert_eq!(p.y_ticks[0].0, 220.0);
    }

    #[test]
    fn day_under_the_pointer() {
        let r = rect();
        assert_eq!(day_at(r, 10, 100.0), Some(0));
        assert_eq!(day_at(r, 10, 139.9), Some(0));
        assert_eq!(day_at(r, 10, 140.1), Some(1));
        assert_eq!(day_at(r, 10, 500.0), Some(9), "rechter Rand gehört noch zum letzten Tag");
        assert_eq!(day_at(r, 10, 99.0), None);
        assert_eq!(day_at(r, 10, 501.0), None);
        assert_eq!(bar_at(r, 4, 480.0), Some(3));
    }

    #[test]
    fn completed_sprints_draw_to_their_end() {
        assert_eq!(chart_today(true, d(2026, 11, 13), d(2026, 12, 1)), d(2026, 11, 13));
        assert_eq!(chart_today(false, d(2026, 11, 13), d(2026, 11, 5)), d(2026, 11, 5));
    }

    #[test]
    fn tooltips_name_the_values() {
        let s = series();
        let t = burn_tooltip(&s, 2, "P", Lang::En).unwrap();
        assert!(t.contains("2026-11-04") && t.contains("Ideal: ") && t.contains("Actual: 8 P"), "{t}");
        let t = burn_tooltip(&s, 11, "P", Lang::De).unwrap();
        assert!(t.contains("13.11.2026") && t.contains("Ist: –"), "{t}");
        assert_eq!(burn_tooltip(&s, 99, "P", Lang::En), None);
        assert_eq!(axis_date(d(2026, 11, 2), Lang::De), "2.11.");
        assert_eq!(axis_date(d(2026, 11, 2), Lang::En), "11-02");
    }

    fn vp(label: &str, planned: f64, completed: f64) -> VelocityPoint {
        VelocityPoint { label: label.into(), planned, completed, completion_ratio: if planned > 0.0 { Some(completed / planned) } else { None } }
    }

    #[test]
    fn velocity_bars_scale_with_the_values() {
        let pts = vec![vp("S1", 20.0, 15.0), vp("S2", 30.0, 30.0), vp("S3", 10.0, 5.0)];
        let plot = rect();
        let v = velocity_plot(&pts, plot);
        assert_eq!(v.scale, nice_scale(30.0, true));
        assert_eq!(v.bars.len(), 3);
        // der höchste Balken (30 von 30 Achsenmaximum) reicht bis oben, wenn die Achse bei 30 endet
        let top = v.bars[1].completed.top();
        assert!((top - (plot.bottom() - plot.height() * (30.0 / v.scale.max) as f32)).abs() < 1e-3);
        // geplant links, erledigt rechts, nebeneinander ohne Überlappung
        for b in &v.bars {
            assert!(b.planned.right() <= b.completed.left());
            assert_eq!(b.planned.bottom(), plot.bottom());
            assert!(b.planned.left() >= plot.left() && b.completed.right() <= plot.right());
        }
        // die Plätze folgen aufeinander
        assert!(v.bars[0].center_x < v.bars[1].center_x && v.bars[1].center_x < v.bars[2].center_x);
        // Durchschnitt des Erledigten: (15 + 30 + 5) / 3
        let (y, a) = v.avg.unwrap();
        assert!((a - 50.0 / 3.0).abs() < 1e-6);
        assert!(y > plot.top() && y < plot.bottom());
    }

    #[test]
    fn velocity_without_sprints_is_empty() {
        let v = velocity_plot(&[], rect());
        assert!(v.bars.is_empty());
        assert_eq!(v.avg, None);
        assert_eq!(v.scale, Scale { max: 4.0, step: 1.0 });
        assert_eq!(velocity_average(&[]), None);
    }

    #[test]
    fn velocity_tooltip_shows_the_ratio() {
        let t = velocity_tooltip(&vp("Sprint 1", 20.0, 15.0), "P", Lang::En);
        assert!(t.contains("Sprint 1") && t.contains("Planned: 20 P") && t.contains("Completed: 15 P (75 %)"), "{t}");
        let t = velocity_tooltip(&vp("S", 0.0, 3.0), "T", Lang::De);
        assert!(t.contains("Geplant: 0 T") && !t.contains('%'), "{t}");
    }
}
