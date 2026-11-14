//! Der Inhalt der Dashboard-Kacheln: aus den geladenen Daten ([`TileData`]) wird hier gezeichnet – Listen mit Spalten, Zahlen und
//! Diagramme, Sprint-Fortschritt, Burndown, Zeit-Säulen, Projektliste, Hinweise und Fehler. Gezeichnet wird nur; Klicks auf Zeilen
//! und Segmente kommen als [`Event`]s zurück und werden von `dash_view.rs` weitergegeben.

use super::dash_charts as charts;
use super::dash_data::{duration_text, BurnView, ProjectRow, SprintView, TileData};
use super::dash_logic::{self as logic, ColInfo, Group, CELL_GAP};
use super::helpers::color_of;
use super::widgets;
use crate::planning::search::TicketRow;
use crate::planning::statuses::StatusCategory;
use crate::planning::views::dashboards::{ChartKind, CountGroup, TileKind};
use crate::state::Lang;
use crate::style;
use chrono::NaiveDate;
use eframe::egui::{self, Align2, Color32, FontId, Id, Pos2, Rect, RichText, Sense, Ui, Vec2};

const ROW_PX: f32 = 24.0;

/// Was ein Klick in einer Kachel bewirken soll (wird von `dash_view.rs` ausgeführt).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Ein Ticket öffnen.
    OpenTicket(i64),
    /// Die Abfrage in der Suche öffnen („Filter öffnen“).
    OpenFilter { jql: String },
    /// Zum Projekt springen.
    JumpProject(i64),
}

/// Was die Kachel zum Zeichnen braucht.
pub struct BodyCx<'a> {
    pub lang: Lang,
    pub tile_id: u32,
    pub kind: &'a TileKind<i64>,
    /// Klicks gelten (im Bearbeiten-Modus nicht).
    pub interactive: bool,
}

/// Zeichnet den Inhalt der Kachel in den verfügbaren Platz von `ui`.
pub fn draw_body(ui: &mut Ui, cx: &BodyCx, data: Option<&TileData>, events: &mut Vec<Event>) {
    let lang = cx.lang;
    let Some(data) = data else {
        message(ui, lang.t("Lädt …", "Loading …"), false);
        return;
    };
    match data {
        TileData::Rows { rows, total, columns } => draw_rows(ui, cx, rows, *total, columns, events),
        TileData::Counts { groups, total, group_by } => draw_counts(ui, cx, groups, *total, *group_by, events),
        TileData::Sprint(s) => draw_sprint(ui, lang, s),
        TileData::Burndown(b) => draw_burndown(ui, lang, b),
        TileData::Time { buckets, total, project_key } => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}: {}", lang.t("Gesamt", "Total"), duration_text(*total, lang))).strong());
                if let Some(k) = project_key {
                    charts::dim_label(ui, k.clone());
                }
            });
            let h = ui.available_height();
            charts::columns(ui, h, buckets, lang);
        }
        TileData::Projects(list) => draw_projects(ui, cx, list, events),
        TileData::Info(t) => message(ui, t, false),
        TileData::Error(t) => message(ui, t, true),
    }
}

/// Hinweis oder Fehler in der Mitte der Kachel.
fn message(ui: &mut Ui, text: &str, error: bool) {
    ui.add_space(6.0);
    let color = if error { widgets::DANGER } else { style::dim(ui) };
    ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
}

// ------------------------------------------------------------------ Liste

fn draw_rows(ui: &mut Ui, cx: &BodyCx, rows: &[TicketRow], total: i64, columns: &[String], events: &mut Vec<Event>) {
    let lang = cx.lang;
    let cols = logic::known_columns(columns);
    let widths = logic::column_widths(&cols, ui.available_width());
    // Kopfzeile mit den Spaltennamen
    let (head, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
    let mut x = head.left();
    for (c, w) in cols.iter().zip(&widths) {
        let cell = Rect::from_min_size(Pos2::new(x, head.top()), Vec2::new(*w, head.height()));
        ui.painter().with_clip_rect(cell).text(Pos2::new(cell.left(), cell.center().y), Align2::LEFT_CENTER, c.label(lang), FontId::proportional(11.0), style::dim(ui));
        x += w + CELL_GAP;
    }
    let footer = if total > rows.len() as i64 { 18.0 } else { 0.0 };
    if rows.is_empty() {
        message(ui, lang.t("Keine Tickets.", "No tickets."), false);
        return;
    }
    let h = (ui.available_height() - footer).max(ROW_PX);
    egui::ScrollArea::vertical().id_salt(("dash_rows", cx.tile_id)).auto_shrink([false, false]).max_height(h).show_rows(ui, ROW_PX, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let row = &rows[i];
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_PX), Sense::click());
            if cx.interactive && resp.hovered() {
                ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.7));
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            paint_row(ui, rect, &cols, &widths, row, lang);
            if cx.interactive && resp.clicked() {
                events.push(Event::OpenTicket(row.id));
            }
        }
    });
    if footer > 0.0 {
        charts::dim_label(
            ui,
            match lang {
                Lang::De => format!("{} von {total} Tickets – alle über „Filter öffnen“.", rows.len()),
                Lang::En => format!("Showing {} of {total} tickets – open the filter for all.", rows.len()),
            },
        );
    }
}

/// Datum aus einem Zeitstempel `JJJJ-MM-TT…`, kurz geschrieben.
pub fn date_text(ts: &str, lang: Lang) -> String {
    ts.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map(|d| logic::short_date(d, lang)).unwrap_or_default()
}

fn paint_row(ui: &Ui, rect: Rect, cols: &[&ColInfo], widths: &[f32], row: &TicketRow, lang: Lang) {
    let base = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let dim = style::dim(ui);
    let done = row.status_category == Some(StatusCategory::Done);
    let text_col = if done { dim } else { ui.visuals().text_color() };
    let cy = rect.center().y;
    let mut x = rect.left();
    for (c, w) in cols.iter().zip(widths) {
        let cell = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(*w, rect.height()));
        let p = base.with_clip_rect(cell.intersect(base.clip_rect()));
        let left = Pos2::new(cell.left(), cy);
        match c.id {
            "key" => {
                p.text(left, Align2::LEFT_CENTER, &row.key, FontId::monospace(12.0), dim);
            }
            "type" => {
                widgets::paint_type_badge(&p, Rect::from_center_size(Pos2::new(cell.left() + 8.0, cy), Vec2::splat(15.0)), row.type_icon.as_deref(), color_of(&row.type_color));
                p.text(Pos2::new(cell.left() + 22.0, cy), Align2::LEFT_CENTER, &row.type_name, FontId::proportional(12.5), text_col);
            }
            "summary" => {
                p.text(left, Align2::LEFT_CENTER, &row.title, FontId::proportional(13.0), text_col);
            }
            "status" => {
                if let Some(name) = &row.status_name {
                    let col = row.status_color.as_deref().map(color_of).unwrap_or(dim);
                    p.circle_filled(Pos2::new(cell.left() + 5.0, cy), 3.5, col);
                    p.text(Pos2::new(cell.left() + 14.0, cy), Align2::LEFT_CENTER, name, FontId::proportional(12.5), text_col);
                }
            }
            "priority" => {
                if let Some(name) = &row.priority {
                    let col = row.priority_color.as_deref().map(color_of).unwrap_or(text_col);
                    p.circle_filled(Pos2::new(cell.left() + 4.0, cy), 3.0, col);
                    p.text(Pos2::new(cell.left() + 12.0, cy), Align2::LEFT_CENTER, name, FontId::proportional(12.5), text_col);
                }
            }
            "points" => {
                if let Some(v) = row.story_points {
                    p.text(left, Align2::LEFT_CENTER, logic::num_text(v, lang), FontId::proportional(12.5), text_col);
                }
            }
            "updated" => {
                p.text(left, Align2::LEFT_CENTER, date_text(&row.updated_at, lang), FontId::proportional(12.5), dim);
            }
            "parent" => {
                if let Some(k) = &row.parent_key {
                    p.text(left, Align2::LEFT_CENTER, k, FontId::monospace(12.0), dim);
                }
            }
            "project" => {
                p.text(left, Align2::LEFT_CENTER, &row.project_key, FontId::monospace(12.0), dim);
            }
            "marks" => {
                let mut cx = cell.left() + 7.0;
                if row.starred {
                    widgets::type_icon(&p, "star", Pos2::new(cx, cy), 14.0, Color32::from_rgb(0xe5, 0xc0, 0x4b));
                    cx += 16.0;
                }
                if row.flagged {
                    widgets::type_icon(&p, "flag", Pos2::new(cx, cy), 14.0, Color32::from_rgb(0xe0, 0x6c, 0x5b));
                }
            }
            _ => {}
        }
        x += w + CELL_GAP;
    }
}

// ------------------------------------------------------------------ Zahlen und Diagramm

fn draw_counts(ui: &mut Ui, cx: &BodyCx, groups: &[Group], total: u32, group_by: CountGroup, events: &mut Vec<Event>) {
    let lang = cx.lang;
    let (base_jql, chart) = match cx.kind {
        TileKind::Counts { jql, chart, .. } => (jql.clone(), *chart),
        _ => (String::new(), ChartKind::Bar),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(total.to_string()).strong().size(18.0));
        charts::dim_label(ui, lang.t("Tickets", "tickets"));
    });
    if groups.is_empty() {
        message(ui, lang.t("Keine Tickets.", "No tickets."), false);
        return;
    }
    ui.add_space(2.0);
    let salt = ("dash_counts", cx.tile_id);
    let hit = match chart {
        ChartKind::Pie => charts::pie(ui, salt, groups, lang),
        ChartKind::Bar => charts::bars(ui, salt, groups, lang),
    };
    if let (true, Some(i)) = (cx.interactive, hit) {
        let g = &groups[i];
        let jql = if g.other { base_jql } else { logic::segment_jql(&base_jql, group_by, (!g.none).then_some(g.name.as_str())) };
        events.push(Event::OpenFilter { jql });
    }
}

// ------------------------------------------------------------------ Sprint

fn unit(points: bool, lang: Lang) -> &'static str {
    if points { lang.t("Punkte", "points") } else { lang.t("Tickets", "tickets") }
}

fn draw_sprint(ui: &mut Ui, lang: Lang, s: &SprintView) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&s.name).strong().size(16.0));
        charts::dim_label(ui, s.project_key.clone());
    });
    if !s.goal.trim().is_empty() {
        ui.add(egui::Label::new(RichText::new(&s.goal).color(style::dim(ui))).truncate());
    }
    let dates = match (s.start, s.end) {
        (Some(a), Some(b)) => format!("{} – {}", logic::short_date(a, lang), logic::short_date(b, lang)),
        (None, Some(b)) => format!("{} {}", lang.t("bis", "until"), logic::short_date(b, lang)),
        _ => String::new(),
    };
    let left = match s.work_days_left {
        Some(0) => format!(" · {}", lang.t("Zeit abgelaufen", "time is up")),
        Some(1) => format!(" · {}", lang.t("noch 1 Arbeitstag", "1 work day left")),
        Some(n) => format!(" · {}", match lang {
            Lang::De => format!("noch {n} Arbeitstage"),
            Lang::En => format!("{n} work days left"),
        }),
        None => String::new(),
    };
    if !dates.is_empty() {
        charts::dim_label(ui, format!("{dates}{left}"));
    }
    ui.add_space(8.0);
    charts::progress_bar(ui, s.fraction(), style::accent());
    ui.add_space(4.0);
    let pct = (s.fraction() * 100.0).round() as u32;
    ui.label(format!("{} / {} {} ({pct}%)", logic::num_text(s.done, lang), logic::num_text(s.total, lang), unit(s.points, lang)));
    if s.points {
        charts::dim_label(ui, format!("{} / {} {}", s.done_tickets, s.total_tickets, lang.t("Tickets erledigt", "tickets done")));
    }
    if let Some(v) = s.velocity {
        charts::dim_label(
            ui,
            match lang {
                Lang::De => format!("Mittlere Velocity: {} {} pro Sprint", logic::num_text(v, lang), unit(s.points, lang)),
                Lang::En => format!("Average velocity: {} {} per sprint", logic::num_text(v, lang), unit(s.points, lang)),
            },
        );
    }
}

fn draw_burndown(ui: &mut Ui, lang: Lang, b: &BurnView) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&b.sprint).strong());
        charts::dim_label(ui, b.project_key.clone());
    });
    let u = unit(b.points, lang);
    let rem = b.remaining.map(|r| logic::num_text(r, lang)).unwrap_or_else(|| "–".to_string());
    charts::dim_label(
        ui,
        match lang {
            Lang::De => format!("Rest {rem} {u} · erledigt {} · neu {} · entfernt {}", logic::num_text(b.completed, lang), logic::num_text(b.added, lang), logic::num_text(b.removed, lang)),
            Lang::En => format!("Remaining {rem} {u} · done {} · added {} · removed {}", logic::num_text(b.completed, lang), logic::num_text(b.added, lang), logic::num_text(b.removed, lang)),
        },
    );
    if b.labels.is_empty() {
        message(ui, lang.t("Keine Arbeitstage im Sprint.", "No work days in the sprint."), false);
        return;
    }
    let h = ui.available_height();
    charts::line_chart(ui, h, &b.labels, &b.ideal, &b.actual, lang);
}

// ------------------------------------------------------------------ Projekte

fn draw_projects(ui: &mut Ui, cx: &BodyCx, list: &[ProjectRow], events: &mut Vec<Event>) {
    let lang = cx.lang;
    if list.is_empty() {
        message(ui, lang.t("Noch kein Projekt.", "No project yet."), false);
        return;
    }
    egui::ScrollArea::vertical().id_salt(Id::new(("dash_projects", cx.tile_id))).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for p in list {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
            if cx.interactive && resp.hovered() {
                ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.7));
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
            painter.circle_filled(Pos2::new(rect.left() + 9.0, rect.center().y), 5.0, color_of(&p.color));
            painter.text(Pos2::new(rect.left() + 22.0, rect.center().y), Align2::LEFT_CENTER, &p.key, FontId::monospace(12.0), color_of(&p.color));
            painter.text(Pos2::new(rect.left() + 70.0, rect.center().y), Align2::LEFT_CENTER, &p.name, FontId::proportional(13.0), ui.visuals().text_color());
            painter.text(Pos2::new(rect.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER, p.tickets.to_string(), FontId::proportional(12.5), style::dim(ui));
            if cx.interactive && resp.on_hover_text(lang.t("Zum Projekt wechseln", "Go to the project")).clicked() {
                events.push(Event::JumpProject(p.id));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::views::dashboards::TimeRange;

    #[test]
    fn dates_from_timestamps() {
        assert_eq!(date_text("2026-11-09T10:20:30", Lang::En), "Nov 9");
        assert_eq!(date_text("2026-11-09", Lang::De), "9.11.");
        assert_eq!(date_text("", Lang::En), "");
        assert_eq!(date_text("kaputt", Lang::En), "");
        assert_eq!(date_text("2026-13-45T00:00:00", Lang::En), "");
    }

    fn row(i: i64, done: bool) -> TicketRow {
        TicketRow {
            id: i,
            project_id: 1,
            project_key: "PAN".into(),
            number: i,
            key: format!("PAN-{i}"),
            title: "Ein sehr langer Titel, der garantiert nicht in die Spalte passt und abgeschnitten werden muss".into(),
            type_id: 1,
            type_name: "Story".into(),
            type_color: "#7FBF6A".into(),
            type_icon: Some("bookmark".into()),
            status_id: Some(1),
            status_name: Some("In Progress".into()),
            status_color: Some("#58A6E6".into()),
            status_category: Some(if done { StatusCategory::Done } else { StatusCategory::Progress }),
            priority: Some("High".into()),
            priority_color: Some("#E06C75".into()),
            story_points: Some(2.5),
            parent_id: Some(9),
            parent_key: Some("PAN-9".into()),
            flagged: i % 2 == 0,
            starred: i % 3 == 0,
            rank: i,
            updated_at: "2026-11-09T10:00:00".into(),
        }
    }

    fn all_columns() -> Vec<String> {
        logic::COLUMNS.iter().map(|c| c.id.to_string()).collect()
    }

    fn group(name: &str, count: u32) -> Group {
        Group { name: name.into(), color: None, count, none: false, other: false }
    }

    /// Jede Art von Kachel-Inhalt lässt sich in jeder Größe zeichnen (auch winzig und leer), ohne Absturz und in beiden Sprachen.
    #[test]
    fn every_kind_of_content_draws_at_every_size() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
        let sprint = SprintView { project_key: "PAN".into(), name: "Sprint 1".into(), goal: "Ein Ziel".into(), start: Some(today), end: Some(today), done: 3.0, total: 5.0, points: true, done_tickets: 2, total_tickets: 4, work_days_left: Some(0), velocity: Some(7.5) };
        let burn = BurnView { project_key: "PAN".into(), sprint: "Sprint 1".into(), labels: vec!["Nov 9".into(), "Nov 10".into(), "Nov 11".into()], ideal: vec![5.0, 2.5, 0.0], actual: vec![Some(5.0), Some(4.0), None], points: false, remaining: Some(4.0), added: 1.0, removed: 0.0, completed: 1.0 };
        let buckets = logic::bucket_time(&[(today, 95u32)].into_iter().collect(), today, TimeRange::Day, Lang::En);
        let kinds_and_data: Vec<(TileKind<i64>, TileData)> = vec![
            (TileKind::Search { jql: String::new(), columns: all_columns(), title: String::new() }, TileData::Rows { rows: (1..40).map(|i| row(i, i % 4 == 0)).collect(), total: 500, columns: all_columns() }),
            (TileKind::Search { jql: String::new(), columns: vec![], title: String::new() }, TileData::Rows { rows: vec![], total: 0, columns: vec![] }),
            (TileKind::Counts { jql: String::new(), group_by: CountGroup::Status, chart: ChartKind::Pie }, TileData::Counts { groups: vec![group("A", 5), group("B", 3), group("C", 0)], total: 8, group_by: CountGroup::Status }),
            (TileKind::Counts { jql: String::new(), group_by: CountGroup::Type, chart: ChartKind::Bar }, TileData::Counts { groups: vec![group("A", 5), group("Ein sehr langer Gruppenname", 1)], total: 6, group_by: CountGroup::Type }),
            (TileKind::Counts { jql: String::new(), group_by: CountGroup::Priority, chart: ChartKind::Pie }, TileData::Counts { groups: vec![], total: 0, group_by: CountGroup::Priority }),
            (TileKind::Sprint { project_id: None }, TileData::Sprint(sprint.clone())),
            (TileKind::Sprint { project_id: None }, TileData::Sprint(SprintView { total: 0.0, done: 0.0, goal: String::new(), work_days_left: None, start: None, end: None, velocity: None, points: false, ..sprint })),
            (TileKind::Burndown { project_id: None }, TileData::Burndown(burn.clone())),
            (TileKind::Burndown { project_id: None }, TileData::Burndown(BurnView { labels: vec!["Nov 9".into()], ideal: vec![1.0], actual: vec![Some(1.0)], ..burn.clone() })),
            (TileKind::Burndown { project_id: None }, TileData::Burndown(BurnView { labels: vec![], ideal: vec![], actual: vec![], remaining: None, ..burn })),
            (TileKind::TimeTracking { range: TimeRange::Day, project_id: None }, TileData::Time { buckets, total: 95, project_key: Some("PAN".into()) }),
            (TileKind::TimeTracking { range: TimeRange::Week, project_id: None }, TileData::Time { buckets: vec![], total: 0, project_key: None }),
            (TileKind::Projects, TileData::Projects(vec![ProjectRow { id: 1, key: "PAN".into(), name: "Paneldeck".into(), color: "#112233".into(), tickets: 12 }])),
            (TileKind::Projects, TileData::Projects(vec![])),
            (TileKind::Favorites, TileData::Info("Kein aktiver Sprint in PAN.".into())),
            (TileKind::Flagged, TileData::Error("Position 8: kaputt".into())),
        ];
        let ctx = egui::Context::default();
        for lang in [Lang::De, Lang::En] {
            for (w, h) in [(420.0, 260.0), (120.0, 60.0), (30.0, 20.0), (900.0, 700.0)] {
                for (n, (kind, data)) in kinds_and_data.iter().enumerate() {
                    for loading in [false, true] {
                        let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))), ..Default::default() }, |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(w, h))), |ui| {
                                    let mut events = Vec::new();
                                    draw_body(ui, &BodyCx { lang, tile_id: n as u32 + 1, kind, interactive: true }, if loading { None } else { Some(data) }, &mut events);
                                    assert!(events.is_empty(), "ohne Klick kein Ereignis");
                                });
                            });
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn units_are_translated() {
        assert_eq!(unit(true, Lang::De), "Punkte");
        assert_eq!(unit(false, Lang::En), "tickets");
    }
}
