//! Die Ansichten des Kalenders: Monat (Raster 6 × 7, ganztägige/mehrtägige Einträge als Balken, Uhrzeiten als Zeilen,
//! „+n weitere“), Woche/Tag (Balken für Ganztägiges oben, darunter das Stundenraster mit Blöcken nebeneinander bei
//! Überschneidung) und Liste (nach Tagen gruppiert; Details rechts). Gemalt mit dem Painter; nur die Einträge des
//! sichtbaren Zeitraums kommen herein (Wiederholungen sind schon auf ihn beschränkt).

use super::{dialog, CalEnv, CalUi, Hit};
use crate::app::palette_ui::color32;
use crate::calendar::builder;
use crate::calendar::sources::Item;
use crate::calendar::store::ViewKind;
use crate::state::Lang;
use crate::style;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2};

/// Höhe einer Stunde im Raster (Woche/Tag).
pub(crate) const HOUR_H: f32 = 40.0;
/// Breite der Stundenbeschriftung links.
pub(crate) const GUTTER: f32 = 46.0;
const LINE_H: f32 = 17.0;
const LANE_H: f32 = 19.0;
/// Höchstens so viele Balkenreihen oben in Woche/Tag (danach „+n“).
const MAX_LANES: usize = 4;

/// Gut lesbare Schrift auf einer Farbe.
fn text_on(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 150.0 { Color32::from_rgb(20, 20, 20) } else { Color32::WHITE }
}

fn chip_color(it: &Item) -> Color32 {
    let c = color32(&it.color);
    if it.done { c.gamma_multiply(0.45) } else { c }
}

/// Reihen für Balken (Spalten `c0..=c1`) gierig vergeben: die erste Reihe, in der die Spalten frei sind.
pub(crate) fn lanes(spans: &[(usize, usize)]) -> Vec<usize> {
    let mut used: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut out = Vec::with_capacity(spans.len());
    for &(a, b) in spans {
        let lane = used.iter().position(|l| l.iter().all(|&(x, y)| b < x || a > y)).unwrap_or_else(|| {
            used.push(Vec::new());
            used.len() - 1
        });
        used[lane].push((a, b));
        out.push(lane);
    }
    out
}

/// Spalten für überlappende Blöcke eines Tages: (Spalte, Spaltenzahl der Gruppe) je Block; `segs` nach Beginn sortiert.
pub(crate) fn columns(segs: &[(i64, i64)]) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 1); segs.len()];
    let mut i = 0;
    while i < segs.len() {
        // Gruppe: alle, die sich (auch über andere) überschneiden
        let mut end = segs[i].1;
        let mut j = i + 1;
        while j < segs.len() && segs[j].0 < end {
            end = end.max(segs[j].1);
            j += 1;
        }
        let mut col_end: Vec<i64> = Vec::new();
        for k in i..j {
            let (s, e) = segs[k];
            let c = col_end.iter().position(|&x| x <= s).unwrap_or_else(|| {
                col_end.push(0);
                col_end.len() - 1
            });
            col_end[c] = e;
            out[k].0 = c;
        }
        for o in out.iter_mut().take(j).skip(i) {
            o.1 = col_end.len();
        }
        i = j;
    }
    out
}

fn wd_short(d: NaiveDate, lang: Lang) -> &'static str {
    builder::weekday_short(d.weekday().num_days_from_sunday() as u8, lang)
}

impl CalUi {
    fn paint_bar(&mut self, ui: &Ui, env: &CalEnv, it: &Item, r: Rect, salt: impl std::hash::Hash) {
        let p = ui.painter();
        let fill = chip_color(it);
        p.rect_filled(r, CornerRadius::same(3), fill);
        if self.selected == Some(it.sel_key()) {
            p.rect_stroke(r.expand(1.0), CornerRadius::same(4), Stroke::new(1.5_f32, ui.visuals().strong_text_color()), StrokeKind::Outside);
        }
        let prefix = if it.recurring { "↻ " } else { "" };
        p.with_clip_rect(r.shrink(2.0)).text(r.left_center() + Vec2::new(4.0, 0.0), Align2::LEFT_CENTER, format!("{prefix}{}", it.title), FontId::proportional(12.0), text_on(fill));
        self.item_hit(ui, env, it, r, salt);
    }

    fn item_hit(&mut self, ui: &Ui, env: &CalEnv, it: &Item, r: Rect, salt: impl std::hash::Hash) {
        let id = egui::Id::new(("cal_item", it.kind, it.id, it.start)).with(salt);
        let resp = ui.interact(r, id, Sense::click()).on_hover_text(format!("{}\n{}", it.title, super::when_text(it, env.lang)));
        self.hits.push((Hit::Item(it.kind, it.id, it.start), r));
        if resp.double_clicked() {
            self.click_item(env, it, true);
        } else if resp.clicked() {
            self.click_item(env, it, false);
        }
    }

    // ───────────────────────── Monat

    pub(super) fn month(&mut self, ui: &mut Ui, env: &CalEnv, items: &[Item]) {
        let lang = env.lang;
        let (from, _) = self.range(env);
        let anchor = self.anchor(env);
        let rect = ui.available_rect_before_wrap();
        let head_h = 20.0;
        let cw = rect.width() / 7.0;
        let ch = ((rect.height() - head_h) / 6.0).max(64.0);
        let vis = ui.visuals().clone();
        let today = env.now.date();
        for i in 0..7 {
            let d = from + Duration::days(i);
            let x = rect.left() + cw * (i as f32 + 0.5);
            ui.painter().text(Pos2::new(x, rect.top() + head_h / 2.0), Align2::CENTER_CENTER, wd_short(d, lang), FontId::proportional(12.0), vis.weak_text_color());
        }
        let cap = (((ch - 20.0) / LINE_H).floor() as usize).max(1);
        for w in 0..6i64 {
            let week0 = from + Duration::days(7 * w);
            let top = rect.top() + head_h + w as f32 * ch;
            let cell = |i: usize| Rect::from_min_size(Pos2::new(rect.left() + cw * i as f32, top), Vec2::new(cw, ch));
            // Zellen
            for i in 0..7usize {
                let d = week0 + Duration::days(i as i64);
                let r = cell(i);
                let other = d.month() != anchor.month();
                let bg = if d == today { style::accent().gamma_multiply(0.12) } else if other { vis.faint_bg_color.gamma_multiply(0.6) } else { Color32::TRANSPARENT };
                ui.painter().rect_filled(r.shrink(0.5), CornerRadius::ZERO, bg);
                ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(0.5_f32, vis.widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
                let num_col = if d == today { style::accent() } else if other { vis.weak_text_color() } else { vis.text_color() };
                ui.painter().text(r.left_top() + Vec2::new(5.0, 3.0), Align2::LEFT_TOP, d.day().to_string(), FontId::proportional(12.0), num_col);
                let resp = ui.interact(r, egui::Id::new(("cal_m_cell", d)), Sense::click());
                self.hits.push((Hit::Day(d), r));
                if resp.double_clicked() {
                    self.form = Some(dialog::EventForm::new_at(d.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default()), false));
                } else if resp.clicked() {
                    self.anchor = Some(d);
                    self.selected = None;
                }
            }
            // Balken (ganztägig) dieser Woche
            let week_end = week0 + Duration::days(6);
            let bars: Vec<&Item> = items.iter().filter(|it| it.all_day && it.day_span().1 >= week0 && it.day_span().0 <= week_end).collect();
            let spans: Vec<(usize, usize)> = bars
                .iter()
                .map(|it| {
                    let (a, b) = it.day_span();
                    ((a.max(week0) - week0).num_days() as usize, (b.min(week_end) - week0).num_days() as usize)
                })
                .collect();
            let lane = lanes(&spans);
            let timed: Vec<Vec<&Item>> = (0..7)
                .map(|i| {
                    let d = week0 + Duration::days(i);
                    items.iter().filter(|it| !it.all_day && it.start.date() <= d && (it.end > d.and_time(NaiveTime::MIN) || it.start.date() == d)).collect()
                })
                .collect();
            // Bedarf je Tag
            let top_lane: Vec<usize> = (0..7).map(|i| spans.iter().zip(&lane).filter(|((a, b), _)| *a <= i && i <= *b).map(|(_, l)| l + 1).max().unwrap_or(0)).collect();
            let overflow: Vec<bool> = (0..7).map(|i| top_lane[i] + timed[i].len() > cap).collect();
            let mut hidden = [0usize; 7];
            for (k, it) in bars.iter().enumerate() {
                let (a, b) = spans[k];
                let l = lane[k];
                let any_over = (a..=b).any(|i| overflow[i]);
                if l + 1 < cap || (l < cap && !any_over) {
                    let r = Rect::from_min_max(Pos2::new(cell(a).left() + 2.0, top + 18.0 + l as f32 * LINE_H), Pos2::new(cell(b).right() - 2.0, top + 18.0 + l as f32 * LINE_H + LINE_H - 2.0));
                    self.paint_bar(ui, env, it, r, w);
                } else {
                    for h in hidden.iter_mut().take(b + 1).skip(a) {
                        *h += 1;
                    }
                }
            }
            for i in 0..7usize {
                let d = week0 + Duration::days(i as i64);
                let room = cap.saturating_sub(top_lane[i]).saturating_sub(overflow[i] as usize);
                for (n, it) in timed[i].iter().enumerate() {
                    if n >= room {
                        hidden[i] += 1;
                        continue;
                    }
                    let y = top + 18.0 + (top_lane[i] + n) as f32 * LINE_H;
                    let r = Rect::from_min_size(Pos2::new(cell(i).left() + 2.0, y), Vec2::new(cw - 4.0, LINE_H - 2.0));
                    let p = ui.painter();
                    if self.selected == Some(it.sel_key()) {
                        p.rect_filled(r, CornerRadius::same(3), vis.selection.bg_fill.gamma_multiply(0.6));
                    }
                    p.circle_filled(r.left_center() + Vec2::new(5.0, 0.0), 3.5, chip_color(it));
                    let t = if it.start.date() == d { crate::calendar::fmt_time(it.start) } else { "…".into() };
                    let col = if it.done { vis.weak_text_color() } else { vis.text_color() };
                    p.with_clip_rect(r).text(r.left_center() + Vec2::new(12.0, 0.0), Align2::LEFT_CENTER, format!("{t} {}", it.title), FontId::proportional(12.0), col);
                    self.item_hit(ui, env, it, r, (w, i));
                }
                if hidden[i] > 0 {
                    let y = top + 18.0 + (cap - 1) as f32 * LINE_H;
                    let r = Rect::from_min_size(Pos2::new(cell(i).left() + 2.0, y), Vec2::new(cw - 4.0, LINE_H - 2.0));
                    let text = match lang {
                        Lang::De => format!("+{} weitere", hidden[i]),
                        Lang::En => format!("+{} more", hidden[i]),
                    };
                    ui.painter().text(r.left_center() + Vec2::new(4.0, 0.0), Align2::LEFT_CENTER, text, FontId::proportional(11.5), style::accent());
                    let resp = ui.interact(r, egui::Id::new(("cal_more", d)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    self.hits.push((Hit::More(d), r));
                    if resp.clicked() {
                        self.anchor = Some(d);
                        self.set_view(env, ViewKind::Day);
                    }
                }
            }
        }
        ui.allocate_rect(rect, Sense::hover());
    }

    // ───────────────────────── Woche / Tag

    pub(super) fn week(&mut self, ui: &mut Ui, env: &CalEnv, items: &[Item], n: i64) {
        let lang = env.lang;
        let (from, _) = self.range(env);
        let last = from + Duration::days(n - 1);
        let rect = ui.available_rect_before_wrap();
        let vis = ui.visuals().clone();
        let today = env.now.date();
        let head_h = 22.0;
        let col_w = ((rect.width() - GUTTER) / n as f32).max(20.0);
        let col_x = |i: i64| rect.left() + GUTTER + col_w * i as f32;
        for i in 0..n {
            let d = from + Duration::days(i);
            let r = Rect::from_min_size(Pos2::new(col_x(i), rect.top()), Vec2::new(col_w, head_h));
            let label = match lang {
                Lang::De => format!("{} {}.", wd_short(d, lang), d.day()),
                Lang::En => format!("{} {}", wd_short(d, lang), d.day()),
            };
            let col = if d == today { style::accent() } else { vis.text_color() };
            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.0), col);
            if n > 1 {
                let resp = ui.interact(r, egui::Id::new(("cal_w_head", d)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    self.anchor = Some(d);
                    self.set_view(env, ViewKind::Day);
                }
            }
        }
        // Balken oben
        let bars: Vec<&Item> = items.iter().filter(|it| it.all_day && it.day_span().1 >= from && it.day_span().0 <= last).collect();
        let spans: Vec<(usize, usize)> = bars
            .iter()
            .map(|it| {
                let (a, b) = it.day_span();
                ((a.max(from) - from).num_days() as usize, (b.min(last) - from).num_days() as usize)
            })
            .collect();
        let lane = lanes(&spans);
        let n_lanes = lane.iter().map(|l| l + 1).max().unwrap_or(0);
        let shown = n_lanes.min(MAX_LANES);
        let bar_top = rect.top() + head_h;
        let bar_h = shown.max(1) as f32 * LANE_H + 6.0 + if n_lanes > MAX_LANES { 14.0 } else { 0.0 };
        ui.painter().text(Pos2::new(rect.left() + GUTTER - 6.0, bar_top + 10.0), Align2::RIGHT_CENTER, lang.t("ganzt.", "all day"), FontId::proportional(10.5), vis.weak_text_color());
        let mut more = 0;
        for (k, it) in bars.iter().enumerate() {
            let (a, b) = spans[k];
            if lane[k] >= MAX_LANES {
                more += 1;
                continue;
            }
            let y = bar_top + 3.0 + lane[k] as f32 * LANE_H;
            let r = Rect::from_min_max(Pos2::new(col_x(a as i64) + 2.0, y), Pos2::new(col_x(b as i64 + 1) - 2.0, y + LANE_H - 3.0));
            self.paint_bar(ui, env, it, r, "bar");
        }
        if more > 0 {
            let text = match lang {
                Lang::De => format!("+{more} weitere ganztägige"),
                Lang::En => format!("+{more} more all-day"),
            };
            ui.painter().text(Pos2::new(col_x(0) + 4.0, bar_top + bar_h - 9.0), Align2::LEFT_CENTER, text, FontId::proportional(11.0), style::accent());
        }
        ui.painter().hline(rect.left()..=rect.right(), bar_top + bar_h, Stroke::new(1.0_f32, vis.widgets.noninteractive.bg_stroke.color));
        // Stundenraster
        let grid = Rect::from_min_max(Pos2::new(rect.left(), bar_top + bar_h + 1.0), rect.max);
        let view = self.view;
        ui.scope_builder(UiBuilder::new().max_rect(grid), |ui| {
            let mut sa = egui::ScrollArea::vertical().id_salt(("cal_grid", n)).auto_shrink([false, false]);
            if self.scrolled != Some((view, from)) {
                let hour = if today >= from && today <= last { (env.now.hour() as f32 - 1.0).max(0.0) } else { 7.0 };
                sa = sa.vertical_scroll_offset(hour * HOUR_H);
                self.scrolled = Some((view, from));
            }
            sa.show(ui, |ui| {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0 * HOUR_H), Sense::click());
                let col_x = |i: i64| r.left() + GUTTER + col_w * i as f32;
                for h in 0..24 {
                    let y = r.top() + h as f32 * HOUR_H;
                    ui.painter().hline(r.left() + GUTTER..=r.right(), y, Stroke::new(0.5_f32, vis.widgets.noninteractive.bg_stroke.color));
                    ui.painter().text(Pos2::new(r.left() + GUTTER - 6.0, y + 2.0), Align2::RIGHT_TOP, format!("{h:02}:00"), FontId::proportional(11.0), vis.weak_text_color());
                }
                for i in 0..n {
                    let d = from + Duration::days(i);
                    let c = Rect::from_min_size(Pos2::new(col_x(i), r.top()), Vec2::new(col_w, 24.0 * HOUR_H));
                    if d == today {
                        ui.painter().rect_filled(c, CornerRadius::ZERO, style::accent().gamma_multiply(0.06));
                    }
                    ui.painter().vline(c.left(), r.top()..=r.bottom(), Stroke::new(0.5_f32, vis.widgets.noninteractive.bg_stroke.color));
                    self.hits.push((Hit::Day(d), c));
                }
                if resp.double_clicked()
                    && let Some(p) = resp.interact_pointer_pos() {
                        let i = ((p.x - r.left() - GUTTER) / col_w).floor() as i64;
                        if (0..n).contains(&i) {
                            let minutes = (((p.y - r.top()) / HOUR_H * 60.0) as i64 / 30 * 30).clamp(0, 23 * 60 + 30);
                            let t = (from + Duration::days(i)).and_time(NaiveTime::MIN) + Duration::minutes(minutes);
                            self.form = Some(dialog::EventForm::new_at(t, false));
                        }
                    }
                // Blöcke je Tag
                for i in 0..n {
                    let d = from + Duration::days(i);
                    let d0 = d.and_time(NaiveTime::MIN);
                    let d1 = d0 + Duration::days(1);
                    let mut segs: Vec<(&Item, i64, i64)> = items
                        .iter()
                        .filter(|it| !it.all_day && it.start < d1 && (it.end > d0 || (it.end <= it.start && it.start >= d0)))
                        .map(|it| {
                            let s = (it.start.max(d0) - d0).num_minutes();
                            let e = if it.end <= it.start { s + 20 } else { (it.end.min(d1) - d0).num_minutes().max(s + 20) };
                            (it, s, e)
                        })
                        .collect();
                    segs.sort_by_key(|(it, s, e)| (*s, -(*e), it.id));
                    let cols = columns(&segs.iter().map(|(_, s, e)| (*s, *e)).collect::<Vec<_>>());
                    for (k, (it, s, e)) in segs.iter().enumerate() {
                        let (c, nc) = cols[k];
                        let w = (col_w - 4.0) / nc as f32;
                        let x = col_x(i) + 2.0 + w * c as f32;
                        let y0 = r.top() + *s as f32 / 60.0 * HOUR_H;
                        let y1 = r.top() + (*e).min(24 * 60) as f32 / 60.0 * HOUR_H;
                        let br = Rect::from_min_max(Pos2::new(x, y0 + 1.0), Pos2::new(x + w - 1.0, y1 - 1.0));
                        let fill = chip_color(it);
                        let p = ui.painter();
                        p.rect_filled(br, CornerRadius::same(4), fill.gamma_multiply(0.85));
                        if self.selected == Some(it.sel_key()) {
                            p.rect_stroke(br.expand(1.0), CornerRadius::same(5), Stroke::new(1.5_f32, vis.strong_text_color()), StrokeKind::Outside);
                        }
                        let clip = p.with_clip_rect(br.shrink(2.0));
                        let tc = text_on(fill);
                        let head = format!("{}{} {}", if it.recurring { "↻ " } else { "" }, crate::calendar::fmt_time(it.start), it.title);
                        clip.text(br.left_top() + Vec2::new(4.0, 2.0), Align2::LEFT_TOP, head, FontId::proportional(12.0), tc);
                        if br.height() > 32.0 && !it.key.is_empty() {
                            clip.text(br.left_top() + Vec2::new(4.0, 17.0), Align2::LEFT_TOP, &it.key, FontId::proportional(11.0), tc.gamma_multiply(0.85));
                        }
                        self.item_hit(ui, env, it, br, ("grid", i));
                    }
                    if d == today {
                        let y = r.top() + (env.now.hour() * 60 + env.now.minute()) as f32 / 60.0 * HOUR_H;
                        ui.painter().hline(col_x(i)..=col_x(i) + col_w, y, Stroke::new(1.5_f32, style::danger_soft()));
                    }
                }
            });
        });
        ui.allocate_rect(rect, Sense::hover());
    }

    // ───────────────────────── Liste

    pub(super) fn list(&mut self, ui: &mut Ui, env: &CalEnv, items: &[Item]) {
        let lang = env.lang;
        let (from, _) = self.range(env);
        egui::ScrollArea::vertical().id_salt("cal_list").auto_shrink([false, false]).show(ui, |ui| {
            if items.is_empty() {
                ui.label(RichText::new(lang.t("Keine Einträge in den nächsten 60 Tagen.", "No entries in the next 60 days.")).color(style::dim(ui)));
                return;
            }
            let mut day: Option<NaiveDate> = None;
            for (k, it) in items.iter().enumerate() {
                let d = it.start.date().max(from);
                if day != Some(d) {
                    day = Some(d);
                    ui.add_space(style::S1);
                    let mut t = RichText::new(super::day_title(d, lang)).strong();
                    if d == env.now.date() {
                        t = t.color(style::accent());
                    }
                    ui.label(t);
                }
                let time = if it.all_day {
                    if it.multi_day() {
                        let (_, b) = it.day_span();
                        match lang {
                            Lang::De => format!("bis {}", b.format("%d.%m.")),
                            Lang::En => format!("until {}", b.format("%b %d")),
                        }
                    } else {
                        lang.t("ganztägig", "all day").to_string()
                    }
                } else {
                    start_text(it.start, d)
                };
                let sel = self.selected == Some(it.sel_key());
                let r = ui
                    .horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.5, chip_color(it));
                        let mut label = format!("{time:>10}  {}{}", if it.recurring { "↻ " } else { "" }, it.title);
                        if !it.key.is_empty() {
                            label.push_str(&format!("  · {}", it.key));
                        }
                        let text = if it.done { RichText::new(label).weak() } else { RichText::new(label) };
                        let r = ui.selectable_label(sel, text);
                        ui.label(RichText::new(it.kind.label(lang)).small().color(style::dim(ui)));
                        r
                    })
                    .inner;
                self.hits.push((Hit::Item(it.kind, it.id, it.start), r.rect));
                if r.double_clicked() {
                    self.click_item(env, it, true);
                } else if r.clicked() {
                    self.click_item(env, it, false);
                }
                let _ = k;
            }
        });
    }
}

fn start_text(t: NaiveDateTime, day: NaiveDate) -> String {
    if t.date() < day { "…".into() } else { crate::calendar::fmt_time(t) }
}
