//! Diagramme der Dashboard-Kacheln, selbst mit dem egui-Painter gezeichnet (keine zusätzliche Abhängigkeit): Fortschrittsbalken,
//! waagerechte Balken, Torte mit Legende, Burndown-Linien und Säulen für die Zeiterfassung. Die Rechnung (Skalierung, Winkel,
//! Treffer) steht in `dash_logic.rs`; hier wird nur gezeichnet und die Maus ausgewertet.

use super::dash_data::duration_text;
use super::dash_logic::{self as logic, Bucket, Group};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Align2, Color32, FontId, Id, Mesh, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2};
use std::hash::Hash;

const ROW: f32 = 24.0;
const TEXT: f32 = 12.5;

fn font() -> FontId {
    FontId::proportional(TEXT)
}

fn grid_color(ui: &Ui) -> Color32 {
    ui.visuals().widgets.noninteractive.bg_stroke.color
}

/// Waagerechter Fortschrittsbalken über die verfügbare Breite (`fraction` 0 bis 1).
pub fn progress_bar(ui: &mut Ui, fraction: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), Sense::hover());
    ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.inactive.bg_fill);
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    if f > 0.0 {
        let w = (rect.width() * f).max(6.0_f32.min(rect.width()));
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(w, rect.height())), 5.0, color);
    }
}

/// Waagerechte Balken, eine Zeile je Gruppe (Name, Balken, Anzahl). Liefert die angeklickte Gruppe.
pub fn bars(ui: &mut Ui, salt: impl Hash, groups: &[Group], lang: Lang) -> Option<usize> {
    let mut clicked = None;
    let total: u32 = groups.iter().map(|g| g.count).sum();
    let max = groups.iter().map(|g| g.count).max().unwrap_or(0) as f64;
    egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for (i, g) in groups.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
            let label_w = (rect.width() * 0.34).clamp(60.0, 170.0);
            let count_w = 34.0;
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.6));
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let color = logic::slice_color(i, g.color.as_deref());
            let label_rect = Rect::from_min_size(rect.min + Vec2::new(4.0, 0.0), Vec2::new(label_w - 8.0, rect.height()));
            p.with_clip_rect(label_rect).text(Pos2::new(label_rect.left(), rect.center().y), Align2::LEFT_CENTER, &g.name, font(), ui.visuals().text_color());
            let bar_x = rect.left() + label_w;
            let bar_w = (rect.right() - count_w - bar_x - 4.0).max(0.0);
            let len = logic::bar_len(g.count as f64, max, bar_w);
            if len > 0.0 {
                p.rect_filled(Rect::from_min_size(Pos2::new(bar_x, rect.center().y - 6.0), Vec2::new(len, 12.0)), 3.0, color);
            }
            p.text(Pos2::new(rect.right() - 4.0, rect.center().y), Align2::RIGHT_CENTER, g.count.to_string(), font(), style::dim(ui));
            let tip = format!("{}: {} ({})", g.name, g.count, logic::percent_text(g.count, total));
            if resp.on_hover_text(tip).clicked() {
                clicked = Some(i);
            }
        }
        let _ = lang;
    });
    clicked
}

/// Ein Kreisausschnitt als Dreiecksfächer (funktioniert auch über 180°).
fn slice_mesh(center: Pos2, radius: f32, a0: f32, a1: f32, color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    let steps = (((a1 - a0) / 0.07).ceil() as usize).clamp(1, 120);
    mesh.colored_vertex(center, color);
    for i in 0..=steps {
        let a = -std::f32::consts::FRAC_PI_2 + a0 + (a1 - a0) * i as f32 / steps as f32;
        mesh.colored_vertex(center + Vec2::new(a.cos(), a.sin()) * radius, color);
    }
    for i in 0..steps as u32 {
        mesh.add_triangle(0, i + 1, i + 2);
    }
    mesh
}

/// Torte (links) mit Legende (rechts). Liefert die angeklickte Gruppe (Segment oder Legendenzeile).
pub fn pie(ui: &mut Ui, salt: impl Hash, groups: &[Group], lang: Lang) -> Option<usize> {
    let salt = Id::new(salt);
    let mut clicked = None;
    let area = ui.available_rect_before_wrap();
    let side = (area.height().min(area.width() * 0.55)).clamp(48.0, 280.0);
    let pie_rect = Rect::from_min_size(area.min, Vec2::new(side, side.min(area.height())));
    let center = pie_rect.center();
    let radius = (pie_rect.width().min(pie_rect.height()) / 2.0 - 6.0).max(8.0);
    let counts: Vec<u32> = groups.iter().map(|g| g.count).collect();
    let total: u32 = counts.iter().sum();
    let resp = ui.interact(pie_rect, salt.with("pie"), Sense::click());
    let hover = resp.hover_pos().and_then(|p| logic::pie_hit(center, radius, p, &counts));
    let angles = logic::pie_angles(&counts);
    if angles.is_empty() {
        ui.painter().circle_stroke(center, radius, Stroke::new(1.5_f32, grid_color(ui)));
    }
    for (i, ((a0, a1), g)) in angles.iter().zip(groups).enumerate() {
        if a1 <= a0 {
            continue;
        }
        let r = if hover == Some(i) { radius + 4.0 } else { radius };
        ui.painter().add(Shape::mesh(slice_mesh(center, r, *a0, *a1, logic::slice_color(i, g.color.as_deref()))));
    }
    // Trennlinien in der Farbe des Untergrunds
    let bg = ui.visuals().panel_fill;
    if angles.len() > 1 {
        for (a0, _) in &angles {
            let a = -std::f32::consts::FRAC_PI_2 + a0;
            ui.painter().line_segment([center, center + Vec2::new(a.cos(), a.sin()) * radius], Stroke::new(1.5_f32, bg));
        }
    }
    if let Some(i) = hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let g = &groups[i];
        let tip = format!("{}: {} ({})", g.name, g.count, logic::percent_text(g.count, total));
        if resp.clicked() {
            clicked = Some(i);
        }
        resp.on_hover_text(tip);
    }
    // Legende
    let legend = Rect::from_min_max(Pos2::new(pie_rect.right() + 12.0, area.top()), area.max);
    if legend.width() > 40.0 {
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(salt.with("legend_ui")).max_rect(legend));
        egui::ScrollArea::vertical().id_salt(salt.with("legend")).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, g) in groups.iter().enumerate() {
                let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                let p = ui.painter();
                if r.hovered() || hover == Some(i) {
                    p.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.6));
                }
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                p.circle_filled(Pos2::new(rect.left() + 8.0, rect.center().y), 4.5, logic::slice_color(i, g.color.as_deref()));
                let right = format!("{}  {}", g.count, logic::percent_text(g.count, total));
                let rg = p.layout_no_wrap(right, font(), style::dim(ui));
                let rw = rg.size().x;
                p.galley(Pos2::new(rect.right() - 4.0 - rw, rect.center().y - rg.size().y / 2.0), rg, style::dim(ui));
                let name_rect = Rect::from_min_max(Pos2::new(rect.left() + 18.0, rect.top()), Pos2::new(rect.right() - rw - 12.0, rect.bottom()));
                p.with_clip_rect(name_rect).text(Pos2::new(name_rect.left(), rect.center().y), Align2::LEFT_CENTER, &g.name, font(), ui.visuals().text_color());
                if r.clicked() {
                    clicked = Some(i);
                }
            }
        });
    }
    let _ = lang;
    ui.allocate_rect(area, Sense::hover());
    clicked
}

/// Wertebereich der Burndown-Achse: die nächste runde Zahl über dem größten Wert.
pub fn line_max(ideal: &[f64], actual: &[Option<f64>]) -> f64 {
    let m = ideal.iter().copied().chain(actual.iter().flatten().copied()).fold(0.0_f64, |a, v| if v.is_finite() { a.max(v) } else { a });
    logic::nice_max(m)
}

/// Welche Beschriftungen der waagerechten Achse gezeigt werden (jede `k`-te, immer die erste und letzte, wenn Platz ist).
pub fn label_indices(n: usize, max_labels: usize) -> Vec<usize> {
    if n == 0 || max_labels == 0 {
        return Vec::new();
    }
    let step = n.div_ceil(max_labels).max(1);
    let mut v: Vec<usize> = (0..n).step_by(step).collect();
    if let Some(last) = v.last().copied() {
        // die letzte nur anhängen, wenn sie nicht an die vorletzte stößt
        if last != n - 1 && n - 1 - last >= step.div_ceil(2) {
            v.push(n - 1);
        }
    }
    v
}

/// Burndown: Ideal-Linie (gestrichelt, grau) und Ist-Linie (Akzentfarbe) über die Tage.
pub fn line_chart(ui: &mut Ui, height: f32, labels: &[String], ideal: &[f64], actual: &[Option<f64>], lang: Lang) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height.max(60.0)), Sense::hover());
    let n = labels.len().min(ideal.len());
    if n == 0 {
        return;
    }
    let plot = Rect::from_min_max(Pos2::new(rect.left() + 34.0, rect.top() + 6.0), Pos2::new(rect.right() - 10.0, rect.bottom() - 20.0));
    if plot.width() < 20.0 || plot.height() < 20.0 {
        return;
    }
    let max = line_max(&ideal[..n], actual);
    let grid = grid_color(ui);
    let dim = style::dim(ui);
    let p = ui.painter();
    for v in logic::axis_ticks(max, 4) {
        let y = plot.bottom() - (v / max) as f32 * plot.height();
        p.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0_f32, grid.gamma_multiply(0.6)));
        p.text(Pos2::new(plot.left() - 5.0, y), Align2::RIGHT_CENTER, logic::num_text(v, lang), FontId::proportional(11.0), dim);
    }
    let x_at = |i: usize| if n == 1 { plot.center().x } else { plot.left() + plot.width() * i as f32 / (n - 1) as f32 };
    let y_at = |v: f64| plot.bottom() - (v / max).clamp(0.0, 1.0) as f32 * plot.height();
    for i in label_indices(n, ((plot.width() / 52.0) as usize).max(2)) {
        p.text(Pos2::new(x_at(i), plot.bottom() + 4.0), Align2::CENTER_TOP, &labels[i], FontId::proportional(11.0), dim);
    }
    let ideal_pts: Vec<Pos2> = (0..n).map(|i| Pos2::new(x_at(i), y_at(ideal[i]))).collect();
    p.extend(Shape::dashed_line(&ideal_pts, Stroke::new(1.5_f32, dim), 6.0, 4.0));
    let actual_pts: Vec<Pos2> = (0..n).filter_map(|i| actual.get(i).copied().flatten().map(|v| Pos2::new(x_at(i), y_at(v)))).collect();
    let accent = style::accent();
    if actual_pts.len() > 1 {
        p.add(Shape::line(actual_pts.clone(), Stroke::new(2.2_f32, accent)));
    }
    for pt in &actual_pts {
        p.circle_filled(*pt, 3.0, accent);
    }
    // Maus: nächster Tag mit Hinweis
    if let Some(h) = resp.hover_pos().filter(|h| plot.expand2(Vec2::new(8.0, 0.0)).contains(*h)) {
        let i = if n == 1 { 0 } else { (((h.x - plot.left()) / plot.width() * (n - 1) as f32).round().max(0.0) as usize).min(n - 1) };
        ui.painter().line_segment([Pos2::new(x_at(i), plot.top()), Pos2::new(x_at(i), plot.bottom())], Stroke::new(1.0_f32, dim.gamma_multiply(0.7)));
        let act = actual.get(i).copied().flatten().map(|v| logic::num_text(v, lang)).unwrap_or_else(|| "–".to_string());
        let tip = format!("{}\n{}: {}\n{}: {}", labels[i], lang.t("Ideal", "Ideal"), logic::num_text(ideal[i], lang), lang.t("Ist", "Actual"), act);
        resp.on_hover_text(tip);
    }
}

/// Säulen der Zeiterfassung (Minuten je Tag/Woche) mit Beschriftung unter jeder Säule.
pub fn columns(ui: &mut Ui, height: f32, buckets: &[Bucket], lang: Lang) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height.max(60.0)), Sense::hover());
    let n = buckets.len();
    if n == 0 {
        return;
    }
    let plot = Rect::from_min_max(Pos2::new(rect.left() + 4.0, rect.top() + 16.0), Pos2::new(rect.right() - 4.0, rect.bottom() - 18.0));
    if plot.width() < 20.0 || plot.height() < 16.0 {
        return;
    }
    let maxm = buckets.iter().map(|b| b.minutes).max().unwrap_or(0) as f64;
    let top = logic::nice_max(maxm / 60.0) * 60.0;
    let slot = plot.width() / n as f32;
    let bar_w = (slot * 0.62).clamp(4.0, 40.0);
    let dim = style::dim(ui);
    let grid = grid_color(ui);
    let accent = style::accent();
    let hover = resp.hover_pos().filter(|h| plot.contains(*h)).map(|h| (((h.x - plot.left()) / slot) as usize).min(n - 1));
    let p = ui.painter();
    p.line_segment([Pos2::new(plot.left(), plot.bottom()), Pos2::new(plot.right(), plot.bottom())], Stroke::new(1.0_f32, grid));
    p.text(Pos2::new(plot.left(), rect.top()), Align2::LEFT_TOP, duration_text(top as u32, lang), FontId::proportional(11.0), dim);
    for (i, b) in buckets.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let h = logic::bar_len(b.minutes as f64, top, plot.height());
        if h > 0.0 {
            let color = if hover == Some(i) { accent } else { accent.gamma_multiply(0.8) };
            p.rect_filled(Rect::from_min_size(Pos2::new(cx - bar_w / 2.0, plot.bottom() - h), Vec2::new(bar_w, h)), 3.0, color);
        }
    }
    for i in label_indices(n, ((plot.width() / 46.0) as usize).max(2)) {
        p.text(Pos2::new(plot.left() + slot * (i as f32 + 0.5), plot.bottom() + 3.0), Align2::CENTER_TOP, &buckets[i].label, FontId::proportional(11.0), dim);
    }
    if let Some(i) = hover {
        let b = &buckets[i];
        resp.on_hover_text(format!("{}: {}", b.label, duration_text(b.minutes, lang)));
    }
}

/// Kleine gedämpfte Hinweiszeile.
pub fn dim_label(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).small().color(style::dim(ui)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_maximum_covers_both_lines() {
        assert_eq!(line_max(&[10.0, 5.0, 0.0], &[Some(8.0), None]), 10.0);
        assert_eq!(line_max(&[10.0, 5.0, 0.0], &[Some(12.0)]), 20.0, "Ist über dem Ideal: Achse wächst mit");
        assert_eq!(line_max(&[0.0], &[None]), 1.0, "leer: Achse 0 bis 1");
        assert_eq!(line_max(&[f64::NAN, 3.0], &[]), 5.0, "unendlich/NaN stört nicht");
    }

    #[test]
    fn axis_labels_are_thinned_out() {
        assert_eq!(label_indices(0, 5), Vec::<usize>::new());
        assert_eq!(label_indices(5, 0), Vec::<usize>::new());
        assert_eq!(label_indices(5, 10), vec![0, 1, 2, 3, 4], "genug Platz: alle");
        let v = label_indices(14, 5);
        assert_eq!(v[0], 0);
        assert!(v.len() <= 6 && v.windows(2).all(|w| w[1] > w[0]), "{v:?}");
        assert_eq!(label_indices(1, 3), vec![0]);
    }
}
