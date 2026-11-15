//! Darstellung eines [`Doc`] in egui (nur Anzeige, nichts ist editierbar).
//!
//! Absätze und Überschriften werden als ein umbrochenes `Galley` gezeichnet; Klicks auf Links und
//! Ticket-Verweise werden über die Glyphenpositionen ermittelt und als [`RenderOutput`] gemeldet.
//! Fett wird (die Oberflächenschrift hat keinen Fettschnitt) durch ein zweites, um 0,6 px versetztes
//! Galley nur der fetten Läufe nachgebildet.

use super::parse::{Align as TAlign, Block, Doc, Inline, ListItem, is_safe_url};
use crate::style::{ACCENT, RADIUS, dim};
use eframe::egui::{
    self, Color32, FontFamily, FontId, Galley, Margin, Pos2, Rect, Sense, Stroke, StrokeKind, TextStyle, Ui, UiBuilder, Vec2,
    text::{LayoutJob, TextFormat},
};
use std::ops::Range;
use std::sync::Arc;

/// Einstellungen der Darstellung.
#[derive(Clone, Debug, Default)]
pub struct RenderOpts {
    /// Zeilenenden im Quelltext als Zeilenumbruch zeichnen (statt als Leerzeichen, wie bei CommonMark).
    pub soft_breaks_as_newlines: bool,
    /// Codeblöcke bekommen oben rechts einen Knopf „Kopieren“ (legt den Code in die Zwischenablage).
    /// `Some(text)` mit dem (übersetzten) Knopftext; `None` = kein Knopf (Standard).
    pub copy_button: Option<&'static str>,
}

/// Ereignisse eines Zeichendurchlaufs. Öffnen von Adressen macht der Aufrufer (nur `http`, `https`,
/// `mailto` werden überhaupt klickbar; siehe [`is_safe_url`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderOutput {
    /// Angeklickter Ticket-Verweis, z. B. `PAN-12`.
    pub clicked_ticket: Option<String>,
    /// Angeklickte Adresse.
    pub clicked_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Url(String),
    Ticket(String),
}

/// Farben und Größen des aktuellen Kontexts (Zitate zeichnen z. B. gedämpft).
#[derive(Clone, Copy)]
struct Look {
    size: f32,
    color: Color32,
    strong: Color32,
    code_bg: Color32,
}

impl Look {
    fn from_ui(ui: &Ui) -> Look {
        let v = ui.visuals();
        let size = ui.style().text_styles.get(&TextStyle::Body).map(|f| f.size).unwrap_or(13.5);
        Look { size, color: v.text_color(), strong: v.strong_text_color(), code_bg: v.text_color().gamma_multiply(0.10) }
    }
}

/// Ein fertig umbrochener Textblock samt klickbaren Bereichen (in Zeichenindizes).
struct Laid {
    galley: Arc<Galley>,
    /// Nur die fetten Läufe sichtbar, zum Nachzeichnen.
    bold: Option<Arc<Galley>>,
    links: Vec<(Range<usize>, Target)>,
}

struct Run {
    text: String,
    fmt: TextFormat,
    bold: bool,
}

#[derive(Clone, Copy)]
struct St {
    bold: bool,
    italic: bool,
    link: bool,
}

struct Builder<'a> {
    look: &'a Look,
    size: f32,
    opts: &'a RenderOpts,
    runs: Vec<Run>,
    links: Vec<(Range<usize>, Target)>,
    /// Zeichenposition im zusammengesetzten Text.
    pos: usize,
}

impl Builder<'_> {
    fn push(&mut self, text: &str, fmt: TextFormat, bold: bool) {
        if text.is_empty() {
            return;
        }
        self.pos += text.chars().count();
        self.runs.push(Run { text: text.to_string(), fmt, bold });
    }

    fn plain(&self, st: St) -> TextFormat {
        TextFormat {
            font_id: FontId::new(self.size, FontFamily::Proportional),
            color: if st.link {
                ACCENT
            } else if st.bold {
                self.look.strong
            } else {
                self.look.color
            },
            italics: st.italic,
            underline: if st.link { Stroke::new(1.0_f32, ACCENT) } else { Stroke::NONE },
            ..Default::default()
        }
    }

    fn walk(&mut self, inl: &[Inline], st: St) {
        for i in inl {
            match i {
                Inline::Text(t) => {
                    let f = self.plain(st);
                    self.push(t, f, st.bold);
                }
                Inline::Code(c) => {
                    let mut f = self.plain(St { bold: false, ..st });
                    f.font_id = FontId::new(self.size * 0.93, FontFamily::Monospace);
                    if !st.link {
                        f.color = self.look.color;
                    }
                    f.background = self.look.code_bg;
                    self.push(c, f, false);
                }
                Inline::SoftBreak => {
                    let f = self.plain(St { bold: false, italic: false, link: false });
                    self.push(if self.opts.soft_breaks_as_newlines { "\n" } else { " " }, f, false);
                }
                Inline::HardBreak => {
                    let f = self.plain(St { bold: false, italic: false, link: false });
                    self.push("\n", f, false);
                }
                Inline::TicketRef(id) => {
                    let mut f = self.plain(St { link: false, ..st });
                    f.color = ACCENT;
                    f.background = ACCENT.gamma_multiply(0.16);
                    let start = self.pos;
                    self.push(id, f, st.bold);
                    let end = self.pos;
                    self.links.push((start..end, Target::Ticket(id.clone())));
                }
                Inline::Bold(c) => self.walk(c, St { bold: true, ..st }),
                Inline::Italic(c) => self.walk(c, St { italic: true, ..st }),
                Inline::Link { text, url } => {
                    let safe = is_safe_url(url);
                    let start = self.pos;
                    self.walk(text, St { link: safe, ..st });
                    let end = self.pos;
                    if safe && end > start {
                        self.links.push((start..end, Target::Url(url.clone())));
                    }
                }
            }
        }
    }
}

/// Umbricht Inline-Elemente auf `wrap` Breite (unendlich = nicht umbrechen).
fn layout(ui: &Ui, look: &Look, opts: &RenderOpts, inl: &[Inline], size: f32, bold_all: bool, wrap: f32, break_anywhere: bool) -> Laid {
    let mut b = Builder { look, size, opts, runs: Vec::new(), links: Vec::new(), pos: 0 };
    b.walk(inl, St { bold: bold_all, italic: false, link: false });
    if b.runs.is_empty() {
        // leere Zeile soll trotzdem Höhe haben
        let f = b.plain(St { bold: false, italic: false, link: false });
        b.runs.push(Run { text: " ".into(), fmt: f, bold: false });
    }
    let mut job = LayoutJob::default();
    job.wrap.max_width = if wrap.is_finite() { wrap.max(8.0) } else { f32::INFINITY };
    job.wrap.break_anywhere = break_anywhere;
    for r in &b.runs {
        job.append(&r.text, 0.0, r.fmt.clone());
    }
    let bold = if b.runs.iter().any(|r| r.bold) {
        let mut j = LayoutJob::default();
        j.wrap = job.wrap.clone();
        for r in &b.runs {
            let mut f = r.fmt.clone();
            f.background = Color32::TRANSPARENT;
            f.underline = Stroke::NONE;
            if !r.bold {
                f.color = Color32::TRANSPARENT;
            }
            j.append(&r.text, 0.0, f);
        }
        Some(ui.painter().layout_job(j))
    } else {
        None
    };
    Laid { galley: ui.painter().layout_job(job), bold, links: b.links }
}

fn paint_laid(ui: &Ui, pos: Pos2, l: &Laid, color: Color32) {
    ui.painter().galley(pos, l.galley.clone(), color);
    if let Some(b) = &l.bold {
        ui.painter().galley(pos + Vec2::new(0.6, 0.0), b.clone(), color);
    }
}

/// Welches Ziel liegt unter `local` (Position relativ zum Galley)?
fn target_at(l: &Laid, local: Vec2) -> Option<&Target> {
    if l.links.is_empty() {
        return None;
    }
    let mut idx = 0usize;
    for pr in &l.galley.rows {
        let r = pr.rect();
        if local.y >= r.min.y && local.y < r.max.y {
            for (k, g) in pr.row.glyphs.iter().enumerate() {
                let x0 = pr.pos.x + g.pos.x;
                if local.x >= x0 && local.x < x0 + g.advance_width {
                    let ci = idx + k;
                    return l.links.iter().find(|(range, _)| range.contains(&ci)).map(|(_, t)| t);
                }
            }
            return None;
        }
        idx += pr.row.glyphs.len() + usize::from(pr.row.ends_with_newline);
    }
    None
}

struct Cx<'a> {
    opts: &'a RenderOpts,
    out: &'a mut RenderOutput,
    look: Look,
    /// Innerhalb von Listeneinträgen: Blöcke rücken enger zusammen.
    tight: bool,
}

/// Zeigerform, Tooltip und Klickmeldung für ein getroffenes Ziel.
fn handle_target(ui: &Ui, resp: &egui::Response, t: Option<&Target>, cx: &mut Cx) {
    let Some(t) = t else { return };
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    if let Target::Url(u) = t {
        let _ = resp.clone().on_hover_text(u.as_str());
    }
    if resp.clicked() {
        match t {
            Target::Url(u) => cx.out.clicked_url = Some(u.clone()),
            Target::Ticket(id) => cx.out.clicked_ticket = Some(id.clone()),
        }
    }
}

/// Zeichnet ein Dokument. Gibt Klicks auf Verweise zurück.
pub fn render(ui: &mut Ui, doc: &Doc, opts: &RenderOpts) -> RenderOutput {
    let mut out = RenderOutput::default();
    let look = Look::from_ui(ui);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut cx = Cx { opts, out: &mut out, look, tight: false };
        blocks(ui, &mut cx, &doc.blocks);
    });
    out
}

const BLOCK_GAP: f32 = 7.0;

fn blocks(ui: &mut Ui, cx: &mut Cx, list: &[Block]) {
    for (i, b) in list.iter().enumerate() {
        if i > 0 {
            ui.add_space(if cx.tight {
                2.0
            } else if matches!(b, Block::Heading { .. }) {
                BLOCK_GAP + 4.0
            } else {
                BLOCK_GAP
            });
        }
        block(ui, cx, b);
    }
}

fn inline_block(ui: &mut Ui, cx: &mut Cx, inl: &[Inline], size: f32, bold_all: bool) {
    let laid = layout(ui, &cx.look, cx.opts, inl, size, bold_all, ui.available_width(), false);
    let sense = if laid.links.is_empty() { Sense::hover() } else { Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(laid.galley.size(), sense);
    if ui.is_rect_visible(rect) {
        paint_laid(ui, rect.min, &laid, cx.look.color);
    }
    if let Some(p) = resp.hover_pos().or_else(|| resp.interact_pointer_pos()) {
        handle_target(ui, &resp, target_at(&laid, p - rect.min), cx);
    }
}

fn block(ui: &mut Ui, cx: &mut Cx, b: &Block) {
    match b {
        Block::Paragraph(inl) => inline_block(ui, cx, inl, cx.look.size, false),
        Block::Heading { level, inlines } => {
            let f = [1.55, 1.3, 1.15, 1.05, 1.0, 1.0][(*level as usize).clamp(1, 6) - 1];
            inline_block(ui, cx, inlines, cx.look.size * f, true);
            if *level <= 2 {
                ui.add_space(3.0);
                let r = ui.available_rect_before_wrap();
                ui.painter().hline(r.x_range(), r.min.y, Stroke::new(1.0_f32, cx.look.color.gamma_multiply(0.22)));
                ui.add_space(1.0);
            }
        }
        Block::Rule => {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 11.0), Sense::hover());
            ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0_f32, cx.look.color.gamma_multiply(0.3)));
        }
        Block::CodeBlock { lang, code } => code_block(ui, cx, lang, code),
        Block::Quote(inner) => quote(ui, cx, inner),
        Block::List { ordered, start, items } => list(ui, cx, *ordered, *start, items),
        Block::Table { align, header, rows } => table(ui, cx, align, header, rows),
    }
}

fn code_block(ui: &mut Ui, cx: &mut Cx, lang: &str, code: &str) {
    egui::Frame::new().fill(cx.look.code_bg).corner_radius(RADIUS).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        if let Some(label) = cx.opts.copy_button {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(lang).size(cx.look.size * 0.8).color(dim(ui)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(label).clicked() {
                        ui.ctx().copy_text(code.to_string());
                    }
                });
            });
            ui.add_space(2.0);
        } else if !lang.is_empty() {
            ui.label(egui::RichText::new(lang).size(cx.look.size * 0.8).color(dim(ui)));
            ui.add_space(2.0);
        }
        let text = egui::RichText::new(code).font(FontId::new(cx.look.size * 0.93, FontFamily::Monospace)).color(cx.look.color);
        ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Wrap));
    });
}

fn quote(ui: &mut Ui, cx: &mut Cx, inner: &[Block]) {
    let saved = cx.look;
    cx.look.color = saved.color.gamma_multiply(0.82);
    cx.look.strong = saved.strong.gamma_multiply(0.88);
    let r = egui::Frame::new().inner_margin(Margin { left: 13, right: 0, top: 1, bottom: 1 }).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        blocks(ui, cx, inner);
    });
    cx.look = saved;
    let rect = r.response.rect;
    let bar = Rect::from_min_max(Pos2::new(rect.min.x + 2.0, rect.min.y), Pos2::new(rect.min.x + 5.0, rect.max.y));
    ui.painter().rect_filled(bar, 1.5, ACCENT.gamma_multiply(0.75));
}

fn list(ui: &mut Ui, cx: &mut Cx, ordered: bool, start: u64, items: &[ListItem]) {
    let line_h = ui.text_style_height(&TextStyle::Body).max(cx.look.size * 1.2);
    let last_no = start.saturating_add(items.len() as u64);
    let num_w = if ordered {
        let g = ui.painter().layout_no_wrap(format!("{last_no}."), FontId::proportional(cx.look.size), cx.look.color);
        g.size().x
    } else {
        0.0
    };
    let marker_w = if ordered { (num_w + 12.0).max(26.0) } else { 24.0 };
    for (k, item) in items.iter().enumerate() {
        if k > 0 {
            ui.add_space(2.0);
        }
        let area = ui.available_rect_before_wrap();
        let content = Rect::from_min_max(Pos2::new(area.min.x + marker_w, area.min.y), area.max);
        ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
            ui.set_min_height(line_h);
            let was_tight = std::mem::replace(&mut cx.tight, true);
            blocks(ui, cx, &item.blocks);
            cx.tight = was_tight;
        });
        let cy = area.min.y + line_h / 2.0;
        let p = ui.painter();
        let col = cx.look.color;
        match item.task {
            Some(done) => {
                let b = Rect::from_center_size(Pos2::new(area.min.x + 10.0, cy), Vec2::splat(14.0));
                if done {
                    p.rect_filled(b, 3.5, ACCENT);
                    let w = Stroke::new(1.9_f32, Color32::WHITE);
                    let pt = |x: f32, y: f32| Pos2::new(b.min.x + b.width() * x, b.min.y + b.height() * y);
                    p.line_segment([pt(0.22, 0.52), pt(0.43, 0.74)], w);
                    p.line_segment([pt(0.43, 0.74), pt(0.80, 0.28)], w);
                } else {
                    p.rect_stroke(b, 3.5, Stroke::new(1.3_f32, col.gamma_multiply(0.6)), StrokeKind::Inside);
                }
            }
            None if ordered => {
                let n = start.saturating_add(k as u64);
                let g = p.layout_no_wrap(format!("{n}."), FontId::proportional(cx.look.size), col.gamma_multiply(0.85));
                p.galley(Pos2::new(area.min.x + marker_w - 7.0 - g.size().x, cy - g.size().y / 2.0), g, col);
            }
            None => {
                let c = Pos2::new(area.min.x + 9.0, cy);
                p.circle_filled(c, 2.8, col.gamma_multiply(0.85));
            }
        }
    }
}

fn table(ui: &mut Ui, cx: &mut Cx, align: &[TAlign], header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) {
    let cols = header.len();
    if cols == 0 {
        return;
    }
    let size = cx.look.size;
    let (padx, pady) = (8.0f32, 4.0f32);
    let avail = ui.available_width().max(40.0);
    // Zeilen: Kopf zuerst
    let mut all: Vec<(&[Vec<Inline>], bool)> = vec![(header, true)];
    all.extend(rows.iter().map(|r| (r.as_slice(), false)));
    let cell = |row: &[Vec<Inline>], c: usize| -> Vec<Inline> { row.get(c).cloned().unwrap_or_default() };
    // natürliche Breiten (ohne Umbruch)
    let mut laid: Vec<Vec<Laid>> = all
        .iter()
        .map(|(r, head)| (0..cols).map(|c| layout(ui, &cx.look, cx.opts, &cell(r, c), size, *head, f32::INFINITY, false)).collect())
        .collect();
    let col_nat: Vec<f32> = (0..cols).map(|c| laid.iter().map(|r| r[c].galley.size().x).fold(0.0, f32::max) + 2.0 * padx).map(|w| w.max(28.0)).collect();
    let total: f32 = col_nat.iter().sum();
    let mut widths = col_nat.clone();
    if total > avail {
        let min_w: Vec<f32> = col_nat.iter().map(|w| w.min(60.0)).collect();
        let sum_min: f32 = min_w.iter().sum();
        if sum_min >= avail {
            widths = min_w.iter().map(|w| (w * avail / sum_min).max(20.0)).collect();
        } else {
            let need: f32 = col_nat.iter().zip(&min_w).map(|(n, m)| n - m).sum();
            let extra = avail - sum_min;
            widths = col_nat.iter().zip(&min_w).map(|(n, m)| m + if need > 0.0 { (n - m) * extra / need } else { 0.0 }).collect();
        }
        laid = all
            .iter()
            .map(|(r, head)| (0..cols).map(|c| layout(ui, &cx.look, cx.opts, &cell(r, c), size, *head, (widths[c] - 2.0 * padx).max(8.0), true)).collect())
            .collect();
    }
    let row_h: Vec<f32> = laid.iter().map(|r| r.iter().map(|l| l.galley.size().y).fold(0.0, f32::max) + 2.0 * pady).collect();
    let (total_w, total_h) = (widths.iter().sum::<f32>(), row_h.iter().sum::<f32>());
    let has_links = laid.iter().flatten().any(|l| !l.links.is_empty());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, total_h), if has_links { Sense::click() } else { Sense::hover() });
    let border = cx.look.color.gamma_multiply(0.28);
    let stroke = Stroke::new(1.0_f32, border);
    let p = ui.painter();
    if ui.is_rect_visible(rect) {
        p.rect_filled(Rect::from_min_size(rect.min, Vec2::new(total_w, row_h[0])), 0.0, cx.look.color.gamma_multiply(0.09));
    }
    // Linien
    let mut y = rect.min.y;
    for h in &row_h {
        p.hline(rect.x_range(), y, stroke);
        y += h;
    }
    let mut x = rect.min.x;
    for w in &widths {
        p.vline(x, rect.y_range(), stroke);
        x += w;
    }
    p.hline(rect.x_range(), rect.max.y, stroke);
    p.vline(rect.max.x, rect.y_range(), stroke);
    // Zellen
    let hover = resp.hover_pos().or_else(|| resp.interact_pointer_pos());
    let mut hit: Option<&Target> = None;
    let mut y = rect.min.y;
    for (r, row) in laid.iter().enumerate() {
        let mut x = rect.min.x;
        for (c, l) in row.iter().enumerate() {
            let inner = widths[c] - 2.0 * padx;
            let gw = l.galley.size().x;
            let dx = match align.get(c).copied().unwrap_or_default() {
                TAlign::Center => ((inner - gw) / 2.0).max(0.0),
                TAlign::Right => (inner - gw).max(0.0),
                _ => 0.0,
            };
            let origin = Pos2::new(x + padx + dx, y + pady);
            paint_laid(ui, origin, l, cx.look.color);
            if hit.is_none() {
                if let Some(pp) = hover {
                    hit = target_at(l, pp - origin);
                }
            }
            x += widths[c];
        }
        y += row_h[r];
    }
    handle_target(ui, &resp, hit, cx);
}

// ───────────────────────────── Tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse::{parse, parse_plain, tests::{Rng, random_markdown}};

    fn frame(f: impl FnOnce(&mut Ui)) {
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 900.0))), ..Default::default() };
        let mut f = Some(f);
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(f) = f.take() {
                    f(ui);
                }
            });
        });
    }

    #[test]
    fn renders_sample_without_panic() {
        let doc = crate::markdown::sample_doc();
        frame(|ui| {
            let out = render(ui, &doc, &RenderOpts::default());
            assert_eq!(out, RenderOutput::default());
        });
        frame(|ui| {
            render(ui, &doc, &RenderOpts { soft_breaks_as_newlines: true, ..RenderOpts::default() });
        });
    }

    #[test]
    fn code_block_copy_button_is_optional_and_copies() {
        let doc = parse_plain("text\n\n```rust\nfn main() {}\n```\n");
        let run = |opts: &RenderOpts, events: Vec<egui::Event>, ctx: &egui::Context, t: f64| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 600.0))), time: Some(t), events, ..Default::default() };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render(ui, &doc, opts);
                });
            })
        };
        let find = |out: &egui::FullOutput, text: &str| {
            out.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect().center()),
                _ => None,
            })
        };
        // Standard: kein Knopf
        let ctx = egui::Context::default();
        let out = run(&RenderOpts::default(), vec![], &ctx, 0.0);
        assert!(find(&out, "Copy").is_none());
        // Mit Knopf: Klick legt den Code in die Zwischenablage
        let opts = RenderOpts { copy_button: Some("Copy"), ..RenderOpts::default() };
        let ctx = egui::Context::default();
        run(&opts, vec![], &ctx, 0.0);
        let out = run(&opts, vec![], &ctx, 0.1);
        let p = find(&out, "Copy").expect("Knopf gezeichnet");
        let btn = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        run(&opts, vec![egui::Event::PointerMoved(p)], &ctx, 0.2);
        run(&opts, vec![btn(true)], &ctx, 0.3);
        let out = run(&opts, vec![btn(false)], &ctx, 0.4);
        let copied = out.platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "fn main() {}"));
        assert!(copied, "{:?}", out.platform_output.commands);
    }

    #[test]
    fn renders_edge_cases() {
        for src in [
            "",
            "| a |\n|---|\n",
            "| a | b | c |\n|:-|:-:|-:|\n| a very long cell with lots of words that must wrap somewhere | x | y |\n| `c` | **b** | [l](https://x.y) |",
            "- [ ] a\n- [x] b\n  - 1. nested",
            "> > > deep\n> - x",
            "```\nlong line long line long line long line long line long line long line long line long line long line\n```",
            "[](x) [a]() **",
            "https://example.org/a/very/long/url/without/any/spaces/that/is/definitely/longer/than/the/width/of/the/panel/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            "# \n## \n#### x",
            "- \n-\n1.\n",
        ] {
            let doc = parse(src, &|_| true);
            frame(|ui| {
                ui.set_max_width(300.0);
                render(ui, &doc, &RenderOpts::default());
            });
        }
    }

    #[test]
    fn fuzz_render_never_panics() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        for _ in 0..120 {
            let len = rng.below(30);
            let src = random_markdown(&mut rng, len);
            let doc = parse(&src, &|x| x == "PAN");
            frame(|ui| {
                ui.set_max_width(160.0 + rng.below(300) as f32);
                render(ui, &doc, &RenderOpts::default());
            });
        }
    }

    /// Mitte des Glyphs mit Zeichenindex `ci` (Galley-Koordinaten) und sein Zeichen.
    fn glyph_center(l: &Laid, ci: usize) -> (Vec2, char) {
        let mut idx = 0;
        for pr in &l.galley.rows {
            if ci < idx + pr.row.glyphs.len() {
                let g = &pr.row.glyphs[ci - idx];
                return (Vec2::new(pr.pos.x + g.pos.x + g.advance_width / 2.0, pr.rect().center().y), g.chr);
            }
            idx += pr.row.glyphs.len() + usize::from(pr.row.ends_with_newline);
        }
        panic!("Zeichen {ci} nicht gefunden");
    }

    #[test]
    fn click_targets_follow_wrapped_text() {
        let doc = parse("Hallo **welt** PAN-12 und [Link](https://example.org) dort. Noch viel mehr Text damit es umbricht und umbricht PAN-3", &|x| x == "PAN");
        frame(|ui| {
            let look = Look::from_ui(ui);
            let opts = RenderOpts::default();
            let Block::Paragraph(inl) = &doc.blocks[0] else { panic!() };
            let laid = layout(ui, &look, &opts, inl, 14.0, false, 160.0, false);
            assert!(laid.galley.rows.len() > 1, "soll umbrechen");
            let text = laid.galley.text().to_string();
            assert!(laid.bold.is_some());
            assert_eq!(laid.links.len(), 3);
            for (range, target) in &laid.links {
                // Mitte des ersten Zeichens und des letzten Zeichens treffen das Ziel
                for ci in [range.start, range.end - 1] {
                    let (c, chr) = glyph_center(&laid, ci);
                    assert_eq!(Some(chr), text.chars().nth(ci), "Zeichen {ci}");
                    assert_eq!(target_at(&laid, c), Some(target), "Zeichen {ci}");
                }
                // das Zeichen davor (Leerzeichen) ist kein Treffer
                let (c, _) = glyph_center(&laid, range.start - 1);
                assert_eq!(target_at(&laid, c), None);
            }
            // unterhalb: nichts
            assert_eq!(target_at(&laid, Vec2::new(5.0, laid.galley.size().y + 30.0)), None);
        });
    }

    #[test]
    fn unsafe_urls_are_not_clickable() {
        let doc = parse_plain("[x](file:///C:/Windows/System32/calc.exe) [y](C:\\a.exe) [z](https://ok.org) [m](mailto:a@b.c)");
        frame(|ui| {
            let look = Look::from_ui(ui);
            let Block::Paragraph(inl) = &doc.blocks[0] else { panic!() };
            let laid = layout(ui, &look, &RenderOpts::default(), inl, 14.0, false, 600.0, false);
            let urls: Vec<_> = laid.links.iter().map(|(_, t)| t.clone()).collect();
            assert_eq!(urls, vec![Target::Url("https://ok.org".into()), Target::Url("mailto:a@b.c".into())]);
        });
    }
}
