//! Rich-Text-Feld mit Umschalter „Text | Visuell“ (wie bei Jira).
//!
//! * Textmodus: Formatierungsleiste und mehrzeiliger Markdown-Editor – nur hier wird editiert.
//! * Visuell: nur das gerenderte Ergebnis, nicht editierbar.
//!
//! Tastenkürzel im Textmodus (nur bei Fokus im Feld): Strg+B fett, Strg+I kursiv. Die App-Kürzel in
//! `keys.rs` verwenden alle zusätzlich Umschalt (bzw. Alt/Tab), es gibt also keine Kollision.

use super::format::{Format, Words, apply_format_with};
use super::parse::parse;
use super::render::{RenderOpts, RenderOutput, render};
use crate::state::{Lang, lang, t};
use crate::style::{D_BASE, D_FAST, RADIUS, anim, dim, mix};
use eframe::egui::{
    self, CornerRadius, FontId, Id, Key, Margin, Modifiers, Pos2, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2,
    text::{CCursor, CCursorRange, LayoutJob, TextFormat},
};
use std::hash::Hash;
use std::ops::Range;

/// Anzeigemodus des Felds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FieldMode {
    /// Markdown-Quelltext bearbeiten.
    #[default]
    Text,
    /// Gerenderte Ansicht, nur lesbar.
    Visual,
}

/// Einstellungen des Felds.
pub struct FieldOpts<'a> {
    /// Starthöhe des Editors in Zeilen (der Editor wächst mit dem Inhalt).
    pub rows: usize,
    /// Entscheidet, welche Kürzel (z. B. `PAN`) als Ticket-Verweis gelten (`None`: keine Verweise).
    pub known_prefix: Option<&'a dyn Fn(&str) -> bool>,
    /// Zeilenenden als Umbruch zeichnen (Visuell).
    pub soft_breaks_as_newlines: bool,
    /// Höhe, ab der das Feld scrollt (`None`: wächst unbegrenzt).
    pub max_height: Option<f32>,
    /// Platzhaltertext im leeren Editor (`None`: Standardtext).
    pub hint: Option<&'a str>,
    /// Formatierungsleiste im Textmodus anzeigen.
    pub toolbar: bool,
}

impl Default for FieldOpts<'_> {
    fn default() -> Self {
        FieldOpts { rows: 6, known_prefix: None, soft_breaks_as_newlines: false, max_height: None, hint: None, toolbar: true }
    }
}

/// Ergebnis von [`rich_text_field_ex`].
pub struct FieldOutput {
    /// Antwort des Editors (Textmodus) bzw. der Anzeige (Visuell); `changed()` auch bei Leistenaktionen.
    pub response: Response,
    /// Klicks auf Verweise in der Visuell-Ansicht.
    pub render: RenderOutput,
}

/// Rich-Text-Feld; siehe Moduldoku. `id` muss je Feld eindeutig sein.
pub fn rich_text_field(ui: &mut Ui, id: impl Hash, text: &mut String, mode: &mut FieldMode, opts: &FieldOpts) -> Response {
    rich_text_field_ex(ui, id, text, mode, opts).response
}

/// Wie [`rich_text_field`], meldet zusätzlich Klicks auf Verweise.
pub fn rich_text_field_ex(ui: &mut Ui, id: impl Hash, text: &mut String, mode: &mut FieldMode, opts: &FieldOpts) -> FieldOutput {
    let id = ui.make_persistent_id(id);
    let te_id = id.with("edit");
    let ctx = ui.ctx().clone();
    let words = if matches!(lang(), Lang::De) { Words::DE } else { Words::EN };
    let mut render_out = RenderOutput::default();
    let mut tool_changed = false;
    let mut focus_edit = false;

    let response = ui
        .vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                let before = *mode;
                mode_switch(ui, id, mode);
                if before == FieldMode::Visual && *mode == FieldMode::Text {
                    focus_edit = true;
                }
                if *mode == FieldMode::Text && opts.toolbar {
                    ui.add_space(8.0);
                    for spec in TOOLS {
                        if tool_button(ui, spec).clicked() {
                            tool_changed |= apply(&ctx, te_id, text, spec.format, words);
                            focus_edit = true;
                        }
                    }
                }
            });
            ui.add_space(4.0);
            match *mode {
                FieldMode::Text => {
                    // Strg+B / Strg+I, nur wenn das Feld den Fokus hat
                    if ctx.memory(|m| m.has_focus(te_id)) {
                        let mut fmts: Vec<Format> = Vec::new();
                        ctx.input_mut(|i| {
                            if !i.modifiers.shift && !i.modifiers.alt {
                                if i.consume_key(Modifiers::COMMAND, Key::B) {
                                    fmts.push(Format::Bold);
                                }
                                if i.consume_key(Modifiers::COMMAND, Key::I) {
                                    fmts.push(Format::Italic);
                                }
                            }
                        });
                        for f in fmts {
                            tool_changed |= apply(&ctx, te_id, text, f, words);
                        }
                    }
                    if focus_edit {
                        ctx.memory_mut(|m| m.request_focus(te_id));
                    }
                    let hint = opts.hint.unwrap_or_else(|| t("Markdown schreiben …", "Write Markdown …"));
                    let edit = |ui: &mut Ui, text: &mut String| {
                        ui.add(
                            egui::TextEdit::multiline(text)
                                .id(te_id)
                                .font(TextStyle::Monospace)
                                .desired_rows(opts.rows.max(1))
                                .desired_width(f32::INFINITY)
                                .hint_text(hint),
                        )
                    };
                    match opts.max_height {
                        Some(h) => egui::ScrollArea::vertical().max_height(h).auto_shrink([false, true]).show(ui, |ui| edit(ui, text)).inner,
                        None => edit(ui, text),
                    }
                }
                FieldMode::Visual => {
                    let line = ui.text_style_height(&TextStyle::Monospace);
                    let min_h = opts.rows.max(1) as f32 * line + 4.0;
                    let known = |p: &str| opts.known_prefix.map(|f| f(p)).unwrap_or(false);
                    let ropts = RenderOpts { soft_breaks_as_newlines: opts.soft_breaks_as_newlines, ..RenderOpts::default() };
                    let body = |ui: &mut Ui, out: &mut RenderOutput| {
                        egui::Frame::new()
                            .fill(ui.visuals().extreme_bg_color)
                            .stroke(Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7)))
                            .corner_radius(RADIUS)
                            .inner_margin(Margin::same(6))
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.set_min_height(min_h);
                                if text.trim().is_empty() {
                                    ui.label(egui::RichText::new(t("Nichts anzuzeigen.", "Nothing to show.")).color(dim(ui)));
                                } else {
                                    let doc = parse(text, &known);
                                    *out = render(ui, &doc, &ropts);
                                }
                            })
                            .response
                    };
                    match opts.max_height {
                        Some(h) => egui::ScrollArea::vertical().max_height(h).auto_shrink([false, true]).show(ui, |ui| body(ui, &mut render_out)).inner,
                        None => body(ui, &mut render_out),
                    }
                }
            }
        })
        .inner;
    let mut response = response;
    if tool_changed {
        response.mark_changed();
    }
    FieldOutput { response, render: render_out }
}

// ───────────────────────────── Auswahl und Formatierung ─────────────────────────────

fn read_selection(ctx: &egui::Context, id: Id, text: &str) -> Range<usize> {
    match egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range()) {
        Some(r) => {
            let (a, b) = (r.primary.index, r.secondary.index);
            a.min(b)..a.max(b)
        }
        None => {
            let n = text.chars().count();
            n..n
        }
    }
}

fn write_selection(ctx: &egui::Context, id: Id, sel: Range<usize>) {
    let mut st = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(sel.start), CCursor::new(sel.end))));
    st.store(ctx, id);
}

/// Wendet eine Formatierung auf den Text des Editors an; `true`, wenn sich der Text geändert hat.
fn apply(ctx: &egui::Context, te_id: Id, text: &mut String, fmt: Format, words: Words) -> bool {
    let sel = read_selection(ctx, te_id, text);
    let (new_text, new_sel) = apply_format_with(text, sel, fmt, words);
    let changed = new_text != *text;
    if changed {
        *text = new_text;
    }
    write_selection(ctx, te_id, new_sel);
    changed
}

// ───────────────────────────── Bedienelemente ─────────────────────────────

/// Kleines Segment „Text | Visuell“.
fn mode_switch(ui: &mut Ui, id: Id, mode: &mut FieldMode) {
    let labels = [t("Text", "Text"), t("Visuell", "Visual")];
    let tips = [
        t("Markdown-Quelltext bearbeiten", "Edit the Markdown source"),
        t("Gerendertes Ergebnis ansehen (nicht editierbar)", "View the rendered result (read-only)"),
    ];
    let modes = [FieldMode::Text, FieldMode::Visual];
    let font = FontId::proportional(12.5);
    let galleys: Vec<_> = labels.iter().map(|l| ui.painter().layout_no_wrap(l.to_string(), font.clone(), egui::Color32::PLACEHOLDER)).collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 22.0).collect();
    let h = 24.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(widths.iter().sum(), h), Sense::hover());
    let (text_c, hover_bg) = (ui.visuals().text_color(), ui.visuals().widgets.hovered.bg_fill);
    let r = (h / 2.0) as u8;
    let mut x = rect.min.x;
    for i in 0..2 {
        let seg = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(widths[i], h));
        let resp = ui.interact(seg, id.with(("seg", i)), Sense::click()).on_hover_text(tips[i]);
        let hv = anim(ui.ctx(), id.with(("h", i)), resp.hovered(), D_FAST);
        let on = anim(ui.ctx(), id.with(("on", i)), *mode == modes[i], D_BASE);
        let cr = if i == 0 { CornerRadius { nw: r, sw: r, ne: 0, se: 0 } } else { CornerRadius { nw: 0, sw: 0, ne: r, se: r } };
        ui.painter().rect_filled(seg, cr, hover_bg.gamma_multiply(hv * (1.0 - on)));
        ui.painter().rect_filled(seg, cr, crate::style::brand().gamma_multiply(0.28 * on));
        let tc = mix(text_c.gamma_multiply(0.72), text_c, on.max(hv * 0.6));
        let g = &galleys[i];
        ui.painter().galley(Pos2::new(seg.center().x - g.size().x / 2.0, seg.center().y - g.size().y / 2.0), g.clone(), tc);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            *mode = modes[i];
        }
        x += widths[i];
    }
    ui.painter().rect_stroke(rect, r, Stroke::new(1.0_f32, text_c.gamma_multiply(0.28)), StrokeKind::Inside);
    ui.painter().vline(rect.min.x + widths[0], rect.y_range(), Stroke::new(1.0_f32, text_c.gamma_multiply(0.28)));
}

#[derive(Clone, Copy, PartialEq)]
enum Face {
    Plain,
    Bold,
    Italic,
    Mono,
}

struct Tool {
    format: Format,
    label: fn() -> &'static str,
    face: Face,
    tip: fn() -> String,
}

fn ctrl_name(key: &str) -> String {
    crate::keys::pretty(&format!("Ctrl+{key}"), matches!(lang(), Lang::De))
}

const TOOLS: &[Tool] = &[
    Tool { format: Format::Bold, label: || "B", face: Face::Bold, tip: || format!("{} ({})", t("Fett", "Bold"), ctrl_name("B")) },
    Tool { format: Format::Italic, label: || "I", face: Face::Italic, tip: || format!("{} ({})", t("Kursiv", "Italic"), ctrl_name("I")) },
    Tool { format: Format::Code, label: || "<>", face: Face::Mono, tip: || t("Code (im Text oder als Block bei mehreren Zeilen)", "Code (inline, or a block for several lines)").to_string() },
    Tool { format: Format::Heading, label: || "H", face: Face::Bold, tip: || t("Überschrift (erneut klicken: nächste Stufe)", "Heading (click again for the next level)").to_string() },
    Tool { format: Format::List, label: || "•", face: Face::Plain, tip: || t("Aufzählung", "Bulleted list").to_string() },
    Tool { format: Format::NumberedList, label: || "1.", face: Face::Plain, tip: || t("Nummerierte Liste", "Numbered list").to_string() },
    Tool { format: Format::TaskList, label: || "[ ]", face: Face::Mono, tip: || t("Aufgabenliste", "Task list").to_string() },
    Tool { format: Format::Quote, label: || ">", face: Face::Mono, tip: || t("Zitat", "Quote").to_string() },
    Tool { format: Format::Link, label: || t("Link", "Link"), face: Face::Plain, tip: || t("Link einfügen oder entfernen", "Insert or remove a link").to_string() },
    Tool { format: Format::Table, label: || t("Tabelle", "Table"), face: Face::Plain, tip: || t("Tabellengerüst einfügen", "Insert a table scaffold").to_string() },
];

/// Flacher Werkzeugknopf mit Text.
fn tool_button(ui: &mut Ui, spec: &Tool) -> Response {
    let label = (spec.label)();
    let family = if spec.face == Face::Mono { egui::FontFamily::Monospace } else { egui::FontFamily::Proportional };
    let size = if spec.face == Face::Mono { 12.0 } else { 13.0 };
    let mut job = LayoutJob::default();
    job.append(
        label,
        0.0,
        TextFormat { font_id: FontId::new(size, family), color: egui::Color32::PLACEHOLDER, italics: spec.face == Face::Italic, ..Default::default() },
    );
    let g = ui.painter().layout_job(job);
    let w = (g.size().x + 14.0).max(26.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    let v = ui.visuals();
    let (text, strong, hover_bg) = (v.text_color(), v.strong_text_color(), v.widgets.hovered.bg_fill);
    let h = anim(ui.ctx(), resp.id.with("h"), resp.hovered(), D_FAST);
    ui.painter().rect_filled(rect, 5.0, hover_bg.gamma_multiply(h));
    let col = mix(text.gamma_multiply(0.8), strong, h);
    let pos = Pos2::new(rect.center().x - g.size().x / 2.0, rect.center().y - g.size().y / 2.0);
    ui.painter().galley(pos, g.clone(), col);
    if spec.face == Face::Bold {
        ui.painter().galley(pos + Vec2::new(0.6, 0.0), g, col);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text((spec.tip)())
}

// ───────────────────────────── Tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(f: impl FnOnce(&mut Ui)) {
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 700.0))), ..Default::default() };
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
    fn field_renders_in_both_modes() {
        let prefix = |p: &str| p == "PAN";
        for mode in [FieldMode::Text, FieldMode::Visual] {
            for text in ["", "# Titel\n\nText mit PAN-1 und **fett**.\n\n| a | b |\n|---|---|\n| 1 | 2 |"] {
                let mut t = text.to_string();
                let mut m = mode;
                let opts = FieldOpts { known_prefix: Some(&prefix), max_height: Some(200.0), ..Default::default() };
                frame(|ui| {
                    let out = rich_text_field_ex(ui, "f", &mut t, &mut m, &opts);
                    assert_eq!(out.render, RenderOutput::default());
                });
                assert_eq!(t, text, "ohne Eingabe bleibt der Text unverändert");
                assert_eq!(m, mode);
            }
        }
    }

    #[test]
    fn apply_updates_text_and_selection() {
        let ctx = egui::Context::default();
        let id = Id::new("x");
        let mut text = "hello world".to_string();
        write_selection(&ctx, id, 6..11);
        assert!(apply(&ctx, id, &mut text, Format::Bold, Words::EN));
        assert_eq!(text, "hello **world**");
        assert_eq!(read_selection(&ctx, id, &text), 8..13);
        // zweites Mal schaltet aus
        assert!(apply(&ctx, id, &mut text, Format::Bold, Words::EN));
        assert_eq!(text, "hello world");
        assert_eq!(read_selection(&ctx, id, &text), 6..11);
        // ohne gespeicherte Auswahl: Cursor am Ende
        let mut t2 = "abc".to_string();
        assert!(apply(&ctx, Id::new("none"), &mut t2, Format::List, Words::EN));
        assert_eq!(t2, "- abc");
    }

    #[test]
    fn tools_have_both_languages() {
        for t in TOOLS {
            assert!(!(t.tip)().is_empty());
            assert!(!(t.label)().is_empty());
        }
        assert_ne!(crate::state::Lang::De, crate::state::Lang::En);
    }
}
