//! Die Hilfeseite der Abfragesprache. Sie öffnet sich wie ein Ticket über die aktuelle Ansicht (Vollfläche), hat aber statt
//! des Zurück-Pfeils ein „X“ oben rechts (Esc schließt ebenfalls). Der Inhalt ist `query_help::HelpDoc::builtin()` (Daten, zweisprachig,
//! nichts davon ist hier hartcodiert): links das Inhaltsverzeichnis (Abschnitte, Beispiele nach Stufe), rechts der Text im
//! Markdown-Stil, die Beispiele als Karten mit Abfrage, Kopieren, „In Suche übernehmen“, Erklärung und Screenshot (`help_shots`).
//! Oben die Volltextsuche (`query_help::search`).

use super::help_shots;
use super::widgets::{self, hint};
use crate::markdown::{self, Doc, RenderOpts};
use crate::planning::query_help::{Block, Example, HELP_TITLE, HelpDoc, Hit, Level, Text, plain, search};
use crate::state::Lang;
use crate::style::{self, Icon, S1, S2, S3};
use eframe::egui::{self, Align, Color32, Key, Rect, RichText, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Zustand der Hilfeseite.
#[derive(Default)]
pub struct HelpState {
    /// Die Seite ist offen.
    pub open: bool,
    search: String,
    /// Sprungmarke (Abschnitts- oder Beispiel-ID), zu der beim nächsten Zeichnen gescrollt wird.
    scroll_to: Option<String>,
    docs: HashMap<(usize, usize, u8, bool), Doc>,
    keys: HashMap<&'static str, Vec<String>>,
    textures: HashMap<&'static str, Option<TextureHandle>>,
    copied: Option<(&'static str, Instant)>,
}

impl HelpState {
    /// Die Seite öffnen (auf Wunsch gleich bei einem Beispiel oder Abschnitt).
    pub fn open_at(&mut self, anchor: Option<&str>) {
        self.open = true;
        self.scroll_to = anchor.map(str::to_string);
    }
}

/// Was die Seite gemeldet hat.
#[derive(Default, Debug)]
pub struct HelpOutput {
    /// „X“ oder Esc.
    pub close: bool,
    /// „In Suche übernehmen“ bei einem Beispiel.
    pub use_query: Option<String>,
}

// ------------------------------------------------------------------ Reine Hilfen (testbar)

/// Inhaltsverzeichnis: Abschnitte und die Beispiele je Stufe (ID, Titel).
#[derive(Debug, PartialEq, Eq)]
pub struct Toc {
    pub sections: Vec<(&'static str, &'static str)>,
    pub levels: Vec<(Level, Vec<(&'static str, &'static str)>)>,
}

pub fn toc(doc: &HelpDoc, lang: Lang) -> Toc {
    let sections = doc.sections.iter().map(|s| (s.id, s.title.get(lang))).collect();
    let all = doc.examples();
    let levels = [Level::Beginner, Level::Intermediate, Level::Advanced]
        .into_iter()
        .map(|l| (l, all.iter().filter(|e| e.level == l).map(|e| (e.id, e.title.get(lang))).collect::<Vec<_>>()))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    Toc { sections, levels }
}

/// Der Markdown-Text eines Blocks (ohne Beispiele und Code, die eigene Darstellung haben).
pub fn block_markdown(b: &Block, lang: Lang) -> Option<String> {
    match b {
        Block::Paragraph(t) => Some(t.get(lang).to_string()),
        Block::Note(t) => Some(format!("> **{}** {}", lang.t("Hinweis:", "Note:"), t.get(lang))),
        Block::Table { header, rows } => {
            let cell = |t: &Text| t.get(lang).replace('|', "\\|");
            let mut s = format!("| {} |\n", header.iter().map(cell).collect::<Vec<_>>().join(" | "));
            s.push_str(&format!("|{}\n", " --- |".repeat(header.len())));
            for r in rows {
                s.push_str(&format!("| {} |\n", r.iter().map(cell).collect::<Vec<_>>().join(" | ")));
            }
            Some(s)
        }
        Block::Code(_) | Block::Example(_) => None,
    }
}

/// Farbe der Stufe.
pub fn level_color(l: Level) -> Color32 {
    match l {
        Level::Beginner => Color32::from_rgb(0x7F, 0xBF, 0x6A),
        Level::Intermediate => Color32::from_rgb(0xE5, 0xA0, 0x4B),
        Level::Advanced => Color32::from_rgb(0xE0, 0x6C, 0x75),
    }
}

// ------------------------------------------------------------------ Die Seite

/// Die Hilfeseite zeichnen (füllt `ui`). `can_use`: „In Suche übernehmen“ anbieten.
pub fn help_page(ui: &mut Ui, st: &mut HelpState, lang: Lang, can_use: bool) -> HelpOutput {
    let mut out = HelpOutput::default();
    let ctx = ui.ctx().clone();
    // Esc schließt – es sei denn, ein Textfeld hat den Fokus (dann nimmt Esc nur den Fokus).
    let editing = ctx.memory(|m| m.focused().is_some());
    if !editing && ctx.input(|i| i.key_pressed(Key::Escape)) {
        out.close = true;
    }
    let doc = HelpDoc::builtin();
    let area = ui.available_rect_before_wrap();
    let pad = 18.0;
    let inner = area.shrink2(Vec2::new(pad, S3));

    // Kopfzeile
    let head_h = 34.0;
    let head = Rect::from_min_size(inner.min, Vec2::new(inner.width(), head_h));
    ui.scope_builder(UiBuilder::new().max_rect(head).layout(egui::Layout::left_to_right(Align::Center)), |ui| {
        ui.label(RichText::new(HELP_TITLE.get(lang)).heading());
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            if style::icon_btn(ui, Icon::Close, lang.t("Hilfe schließen (Esc)", "Close help (Esc)"), false).clicked() {
                out.close = true;
            }
            ui.add_space(S2);
            if !st.search.is_empty() && style::icon_btn_small(ui, Icon::Close, lang.t("Suche löschen", "Clear search")).clicked() {
                st.search.clear();
            }
            ui.add(egui::TextEdit::singleline(&mut st.search).desired_width(260.0).hint_text(lang.t("Anleitung durchsuchen …", "Search the guide …")).margin(egui::Margin::symmetric(8, 5)));
        });
    });
    let body = Rect::from_min_max(egui::pos2(inner.min.x, inner.min.y + head_h + S2), inner.max);
    ui.painter().hline(area.x_range(), body.min.y - S1, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(18)));

    let toc_w = if body.width() > 760.0 { 230.0 } else { 0.0 };
    if toc_w > 0.0 {
        let left = Rect::from_min_size(body.min, Vec2::new(toc_w, body.height()));
        ui.scope_builder(UiBuilder::new().max_rect(left), |ui| toc_panel(ui, st, doc, lang));
    }
    let right = Rect::from_min_max(egui::pos2(body.min.x + if toc_w > 0.0 { toc_w + 18.0 } else { 0.0 }, body.min.y), body.max);
    ui.scope_builder(UiBuilder::new().max_rect(right), |ui| {
        egui::ScrollArea::vertical().id_salt("help_content").auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width().min(880.0);
            ui.set_max_width(w);
            if st.search.trim().is_empty() {
                content(ui, st, doc, lang, can_use, &mut out);
            } else {
                results(ui, st, doc, lang, can_use, &mut out);
            }
            ui.add_space(40.0);
        });
    });
    out
}

fn toc_panel(ui: &mut Ui, st: &mut HelpState, doc: &HelpDoc, lang: Lang) {
    let t = toc(doc, lang);
    egui::ScrollArea::vertical().id_salt("help_toc").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(lang.t("Abschnitte", "Sections")).small().color(style::dim(ui)));
        for (id, title) in &t.sections {
            if toc_link(ui, title, None).clicked() {
                st.scroll_to = Some((*id).to_string());
                st.search.clear();
            }
        }
        for (level, items) in &t.levels {
            ui.add_space(S2);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, level_color(*level));
                ui.label(RichText::new(format!("{} – {}", lang.t("Beispiele", "Examples"), level.label().get(lang))).small().color(style::dim(ui)));
            });
            for (id, title) in items {
                if toc_link(ui, title, Some(level_color(*level))).clicked() {
                    st.scroll_to = Some((*id).to_string());
                    st.search.clear();
                }
            }
        }
    });
}

/// Ein Eintrag im Inhaltsverzeichnis (Text, bei Hover hervorgehoben).
fn toc_link(ui: &mut Ui, text: &str, dot: Option<Color32>) -> egui::Response {
    let indent = if dot.is_some() { 12.0 } else { 0.0 };
    let w = ui.available_width();
    let galley = ui.painter().layout(text.to_string(), egui::FontId::proportional(13.0), ui.visuals().text_color(), (w - indent - 8.0).max(40.0));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, galley.size().y + 6.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let col = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color().gamma_multiply(0.85) };
    ui.painter().galley(egui::pos2(rect.min.x + 4.0 + indent, rect.min.y + 3.0), galley, col);
    resp
}

/// Das ganze Dokument.
fn content(ui: &mut Ui, st: &mut HelpState, doc: &HelpDoc, lang: Lang, can_use: bool, out: &mut HelpOutput) {
    for (si, s) in doc.sections.iter().enumerate() {
        anchor(ui, st, s.id);
        ui.add_space(S3);
        ui.label(RichText::new(s.title.get(lang)).size(22.0).strong());
        ui.add_space(S1);
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.x_range(), r.min.y, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(28)));
        ui.add_space(S2);
        for (bi, b) in s.blocks.iter().enumerate() {
            block(ui, st, (si, bi), b, lang, can_use, out);
            ui.add_space(S2);
        }
    }
}

/// Suchergebnisse: Beispiele als volle Karten, alles andere als Zeile mit Ausschnitt und Sprung.
fn results(ui: &mut Ui, st: &mut HelpState, doc: &HelpDoc, lang: Lang, can_use: bool, out: &mut HelpOutput) {
    let hits: Vec<Hit> = search(doc, lang, &st.search);
    ui.add_space(S2);
    ui.label(RichText::new(match lang {
        Lang::De => format!("{} Treffer für {}", hits.len(), lang.q(st.search.trim())),
        Lang::En => format!("{} result(s) for {}", hits.len(), lang.q(st.search.trim())),
    }).color(style::dim(ui)));
    if hits.is_empty() {
        ui.add_space(S2);
        hint(ui, lang.t("Nichts gefunden. Versuche ein anderes Wort, z. B. einen Feldnamen oder Operator (IN, ORDER BY, currentDate).", "Nothing found. Try another word, e.g. a field name or operator (IN, ORDER BY, currentDate)."));
    }
    ui.add_space(S2);
    for h in &hits {
        let sec_idx = doc.sections.iter().position(|s| s.id == h.section_id);
        let example = h.example_id.and_then(|id| doc.example(id));
        match (example, sec_idx, h.block) {
            (Some(e), Some(si), Some(bi)) => {
                ui.label(RichText::new(h.section_title).small().color(style::dim(ui)));
                example_card(ui, st, (si, bi), e, lang, can_use, out);
            }
            _ => {
                widgets::card(ui, |ui| {
                    ui.label(RichText::new(h.section_title).strong());
                    ui.label(RichText::new(&h.snippet).small().color(style::dim(ui)));
                });
                if ui.small_button(lang.t("Zum Abschnitt", "Go to section")).clicked() {
                    st.scroll_to = Some(h.section_id.to_string());
                    st.search.clear();
                }
            }
        }
        ui.add_space(S2);
    }
}

/// Zu einer Sprungmarke scrollen, wenn sie angefragt ist (am Cursor der Marke aufrufen).
fn anchor(ui: &mut Ui, st: &mut HelpState, id: &str) {
    if st.scroll_to.as_deref() == Some(id) {
        ui.scroll_to_cursor(Some(Align::TOP));
        st.scroll_to = None;
    }
}

fn block(ui: &mut Ui, st: &mut HelpState, at: (usize, usize), b: &Block, lang: Lang, can_use: bool, out: &mut HelpOutput) {
    match b {
        Block::Code(c) => code_box(ui, c),
        Block::Example(e) => example_card(ui, st, at, e, lang, can_use, out),
        _ => {
            if let Some(src) = block_markdown(b, lang) {
                render_md(ui, st, (at.0, at.1, 0, lang == Lang::De), &src);
            }
        }
    }
}

/// Markdown rendern; der geparste Baum wird je Block gemerkt.
fn render_md(ui: &mut Ui, st: &mut HelpState, key: (usize, usize, u8, bool), src: &str) {
    let doc = st.docs.entry(key).or_insert_with(|| markdown::parse_plain(src));
    markdown::render(ui, doc, &RenderOpts::default());
}

/// Abfrage oder Grammatik als abgesetzter Code.
fn code_box(ui: &mut Ui, code: &str) {
    egui::Frame::NONE.fill(Color32::from_black_alpha(70)).corner_radius(6.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(code).monospace().size(13.0)).wrap());
    });
}

/// Eine Beispiel-Karte.
fn example_card(ui: &mut Ui, st: &mut HelpState, at: (usize, usize), e: &Example, lang: Lang, can_use: bool, out: &mut HelpOutput) {
    anchor(ui, st, e.id);
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(plain(e.title.get(lang))).strong().size(15.0));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let col = level_color(e.level);
                ui.label(RichText::new(e.level.label().get(lang)).small().color(col));
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, col);
            });
        });
        ui.add_space(S1);
        code_box(ui, &e.jql);
        ui.add_space(S1);
        ui.horizontal(|ui| {
            let copied = st.copied.is_some_and(|(id, t)| id == e.id && t.elapsed() < Duration::from_millis(1500));
            let label = if copied { lang.t("Kopiert", "Copied") } else { lang.t("Kopieren", "Copy") };
            if ui.button(label).on_hover_text(lang.t("Die Abfrage in die Zwischenablage kopieren", "Copy the query to the clipboard")).clicked() {
                ui.ctx().copy_text(e.jql.clone());
                st.copied = Some((e.id, Instant::now()));
            }
            if copied {
                ui.ctx().request_repaint_after(Duration::from_millis(1600));
            }
            if can_use && ui.button(lang.t("In Suche übernehmen", "Use in search")).on_hover_text(lang.t("Die Abfrage ins Suchfeld übernehmen und die Hilfe schließen", "Put the query into the search field and close the help")).clicked() {
                out.use_query = Some(e.jql.clone());
            }
        });
        ui.add_space(S1);
        render_md(ui, st, (at.0, at.1, 1, lang == Lang::De), e.explanation.get(lang));
        ui.add_space(S1);
        let keys = st.keys.entry(e.id).or_insert_with(|| e.expected_keys()).clone();
        let result = if keys.is_empty() { lang.t("keine Treffer", "no matches").to_string() } else { keys.join(", ") };
        ui.label(RichText::new(format!("{} {result}", lang.t("Ergebnis mit den Beispieldaten:", "Result with the example data:"))).small().color(style::dim(ui)));
        ui.add_space(S2);
        screenshot(ui, st, e, lang);
    });
}

/// Der Screenshot des Beispiels oder ein Platzhalter.
fn screenshot(ui: &mut Ui, st: &mut HelpState, e: &Example, lang: Lang) {
    let ctx = ui.ctx().clone();
    let tex = st
        .textures
        .entry(e.id)
        .or_insert_with(|| help_shots::shot_bytes(e.id).and_then(help_shots::decode).map(|img| ctx.load_texture(format!("query-help-{}", e.id), img, TextureOptions::LINEAR)))
        .clone();
    match tex {
        Some(t) => {
            let [w, h] = t.size();
            let max_w = ui.available_width().min(760.0);
            let scale = (max_w / w as f32).min(1.0);
            ui.add(egui::Image::new((t.id(), Vec2::new(w as f32 * scale, h as f32 * scale))).corner_radius(6.0))
                .on_hover_text(lang.t("Screenshot aus der Anwendung (englische Oberfläche)", "Screenshot from the application (English interface)"));
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), egui::Sense::hover());
            ui.painter().rect_stroke(rect, 6.0, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, lang.t("Screenshot folgt", "Screenshot coming"), egui::FontId::proportional(12.5), style::dim(ui));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toc_lists_every_section_and_example_once() {
        let doc = HelpDoc::builtin();
        for lang in [Lang::En, Lang::De] {
            let t = toc(doc, lang);
            assert_eq!(t.sections.len(), doc.sections.len());
            let listed: usize = t.levels.iter().map(|(_, v)| v.len()).sum();
            assert_eq!(listed, doc.examples().len());
            let mut ids: Vec<&str> = t.levels.iter().flat_map(|(_, v)| v.iter().map(|(i, _)| *i)).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), listed, "Beispiel doppelt im Inhaltsverzeichnis");
            assert!(t.sections.iter().all(|(_, title)| !title.is_empty()));
        }
        // Stufen in der Reihenfolge Einsteiger → Profi, nur nicht leere
        let t = toc(doc, Lang::En);
        let levels: Vec<Level> = t.levels.iter().map(|(l, _)| *l).collect();
        let mut sorted = levels.clone();
        sorted.sort();
        assert_eq!(levels, sorted);
        assert!(!levels.is_empty());
        assert_ne!(toc(doc, Lang::En), toc(doc, Lang::De), "Titel sind je Sprache übersetzt");
    }

    #[test]
    fn blocks_become_markdown_in_both_languages() {
        let doc = HelpDoc::builtin();
        let (mut paras, mut tables, mut notes) = (0, 0, 0);
        for lang in [Lang::En, Lang::De] {
            for s in &doc.sections {
                for b in &s.blocks {
                    match (b, block_markdown(b, lang)) {
                        (Block::Code(_) | Block::Example(_), md) => assert!(md.is_none()),
                        (b, Some(md)) => {
                            assert!(!md.trim().is_empty());
                            let parsed = markdown::parse_plain(&md);
                            assert!(!parsed.blocks.is_empty(), "{md}");
                            match b {
                                Block::Table { .. } => {
                                    tables += 1;
                                    assert!(parsed.blocks.iter().any(|x| matches!(x, markdown::Block::Table { .. })), "Tabelle nicht erkannt:\n{md}");
                                }
                                Block::Note(_) => {
                                    notes += 1;
                                    assert!(parsed.blocks.iter().any(|x| matches!(x, markdown::Block::Quote(_))));
                                }
                                _ => paras += 1,
                            }
                        }
                        (_, None) => panic!("Block ohne Markdown"),
                    }
                }
            }
        }
        assert!(paras > 0 && tables > 0 && notes > 0);
    }

    #[test]
    fn open_at_remembers_the_anchor() {
        let mut st = HelpState::default();
        assert!(!st.open);
        st.open_at(Some("filters"));
        assert!(st.open);
        assert_eq!(st.scroll_to.as_deref(), Some("filters"));
        st.open_at(None);
        assert_eq!(st.scroll_to, None);
    }

    #[test]
    fn search_finds_examples_by_query_text() {
        let doc = HelpDoc::builtin();
        let hits = search(doc, Lang::En, "ORDER BY");
        assert!(hits.iter().any(|h| h.example_id.is_some()));
        assert!(search(doc, Lang::De, "   ").is_empty());
        assert!(search(doc, Lang::En, "zzzzzqqq").is_empty());
    }

    #[test]
    fn every_level_has_a_color_and_label() {
        for l in [Level::Beginner, Level::Intermediate, Level::Advanced] {
            assert_ne!(level_color(l), Color32::TRANSPARENT);
            assert!(!l.label().en.is_empty() && !l.label().de.is_empty());
        }
    }
}
