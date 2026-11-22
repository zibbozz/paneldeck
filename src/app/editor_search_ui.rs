//! Ergebnis-Panel der Suche in Dateien (#54): Eingabezeile mit Schaltern (Groß-/Kleinschreibung, ganzes Wort,
//! regulärer Ausdruck, Ignorierte einbeziehen), Start/Abbrechen, Zusammenfassung und die Trefferliste je Datei
//! (aufklappbar, Treffer hervorgehoben). Das Modell und die Suche stehen in `editor_svc::find`; wo das Panel im
//! Editor-Reiter steht und was „öffnen“ heißt, entscheidet der Editor über die zurückgegebene [`PanelAction`].

use super::*;
use crate::editor_svc::find::{self, Query, Results};
use crate::editor_svc::settings::EditorSettings;
use eframe::egui::text::{LayoutJob, TextFormat};
use std::path::Path;

/// Eingaben des Panels (bleiben zwischen zwei Suchen stehen).
#[derive(Clone, Debug, Default)]
pub(super) struct SearchInput {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// `None` = Vorgabe aus *Einstellungen → Editor*.
    pub include_ignored: Option<bool>,
    /// Eingabefeld beim nächsten Bild fokussieren (z. B. nach Strg+Umschalt+F).
    pub focus: bool,
}

impl SearchInput {
    /// Anfrage aus Eingaben und Einstellungen.
    pub fn query(&self, s: &EditorSettings) -> Query {
        Query {
            pattern: self.pattern.clone(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            include_ignored: self.include_ignored.unwrap_or(s.search_include_ignored),
            excludes: s.search_exclude.clone(),
            max_file_bytes: s.large_file_bytes(),
        }
    }
}

/// Was der Editor tun soll.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PanelAction {
    /// Datei an Zeile/Spalte (ab 1) öffnen bzw. dorthin springen.
    Open { path: PathBuf, line: usize, col: usize },
    /// Esc im Suchfeld: Fenster schließen.
    Close,
}

/// Zusammenfassung als Text (für die Statuszeile des Panels).
pub(super) fn summary_text(r: &Results, lang: Lang) -> String {
    if let Some(e) = &r.error {
        return e.clone();
    }
    let files = r.files.len();
    let matches = r.total_matches();
    let base = match lang {
        Lang::De => format!("{matches} Treffer in {files} Dateien"),
        Lang::En => format!("{matches} {} in {files} {}", if matches == 1 { "result" } else { "results" }, if files == 1 { "file" } else { "files" }),
    };
    match &r.summary {
        None if r.running() => match lang {
            Lang::De => format!("{base} – suche … ({} Dateien durchsucht)", r.progress),
            Lang::En => format!("{base} – searching … ({} files scanned)", r.progress),
        },
        None => String::new(),
        Some(s) => {
            let mut extra = Vec::new();
            if s.skipped_binary > 0 {
                extra.push(match lang {
                    Lang::De => format!("{} binär übersprungen", s.skipped_binary),
                    Lang::En => format!("{} binary skipped", s.skipped_binary),
                });
            }
            if s.skipped_large > 0 {
                extra.push(match lang {
                    Lang::De => format!("{} große übersprungen", s.skipped_large),
                    Lang::En => format!("{} large skipped", s.skipped_large),
                });
            }
            if s.errors > 0 {
                extra.push(match lang {
                    Lang::De => format!("{} nicht lesbar", s.errors),
                    Lang::En => format!("{} unreadable", s.errors),
                });
            }
            let tail = if s.cancelled {
                lang.t(" – abgebrochen", " – cancelled")
            } else if s.truncated {
                lang.t(" – Obergrenze erreicht, Suche eingrenzen", " – limit reached, narrow the search")
            } else {
                ""
            };
            let scanned = match lang {
                Lang::De => format!("{} Dateien durchsucht", s.files_scanned),
                Lang::En => format!("{} files scanned", s.files_scanned),
            };
            extra.insert(0, scanned);
            format!("{base} ({}){tail}", extra.join(", "))
        }
    }
}

/// Eine Trefferzeile mit hervorgehobenen Bereichen.
fn line_job(ui: &Ui, hit: &find::LineHit) -> LayoutJob {
    let c = crate::style::ui();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let normal = TextFormat { font_id: font.clone(), color: c.text, ..Default::default() };
    let marked = TextFormat { font_id: font.clone(), color: c.text_strong, background: c.selection_bg, ..Default::default() };
    let num = TextFormat { font_id: font, color: c.text_dim, ..Default::default() };
    let mut job = LayoutJob::default();
    job.append(&format!("{:>5}  ", hit.line), 0.0, num);
    let mut at = 0usize;
    for (a, b) in &hit.ranges {
        let (a, b) = ((*a).min(hit.preview.len()), (*b).min(hit.preview.len()));
        if a < at || !hit.preview.is_char_boundary(a) || !hit.preview.is_char_boundary(b) {
            continue;
        }
        job.append(&hit.preview[at..a], 0.0, normal.clone());
        job.append(&hit.preview[a..b], 0.0, marked.clone());
        at = b;
    }
    job.append(&hit.preview[at..], 0.0, normal);
    job
}

/// Das Panel zeichnen. `root` = geöffneter Ordner des Editors (`None`: kein Ordner offen – Suche nicht möglich).
/// Nichts darin ist breiter als der verfügbare Platz: Schalter rechts zuerst, das Suchfeld nimmt den Rest, lange Texte
/// werden gekürzt – sonst wuchs das Fenster mit jedem Bild über den Rand des Hauptfensters.
pub(super) fn show(ui: &mut Ui, lang: Lang, input: &mut SearchInput, results: &mut Results, root: Option<&Path>, settings: &EditorSettings) -> Option<PanelAction> {
    let mut action = None;
    let mut start = false;
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut inc = input.include_ignored.unwrap_or(settings.search_include_ignored);
            if ui
                .selectable_label(inc, lang.t("Ignorierte", "Ignored"))
                .on_hover_text(lang.t("Ignorierte einbeziehen: auch Dateien, die .gitignore ausschließt (eigene Ausschlussmuster gelten weiter)", "Include ignored: also files excluded by .gitignore (your own exclude patterns still apply)"))
                .clicked()
            {
                inc = !inc;
                input.include_ignored = Some(inc);
            }
            for (on, label, tip) in [
                (&mut input.regex, ".*", lang.t("Regulärer Ausdruck", "Regular expression")),
                (&mut input.whole_word, "W", lang.t("Nur ganze Wörter", "Whole word")),
                (&mut input.case_sensitive, "Aa", lang.t("Groß-/Kleinschreibung beachten", "Match case")),
            ] {
                if ui.selectable_label(*on, RichText::new(label).monospace()).on_hover_text(tip).clicked() {
                    *on = !*on;
                }
            }
            // Die Breite ist die des Textes, der Innenrand kommt dazu.
            let edit = egui::TextEdit::singleline(&mut input.pattern).hint_text(lang.t("In Dateien suchen", "Search in files")).desired_width((ui.available_width() - 12.0).max(40.0));
            let r = ui.add(edit);
            if std::mem::take(&mut input.focus) {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                start = true;
                // Fokus bleibt im Suchfeld: weitersuchen mit Enter, schließen mit Esc.
                r.request_focus();
            }
            // Esc im Suchfeld (egui nimmt ihm dabei den Fokus) schließt das Fenster.
            if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }
        });
    });
    ui.horizontal(|ui| {
        if results.running() {
            if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                results.cancel();
            }
        } else if ui.add_enabled(root.is_some() && !input.pattern.is_empty(), egui::Button::new(lang.t("Suchen", "Search"))).clicked() {
            start = true;
        }
        if !results.files.is_empty() && !results.running() && ui.small_button(lang.t("Alle zuklappen", "Collapse all")).clicked() {
            results.collapsed = (0..results.files.len()).collect();
        }
        let text = if root.is_none() { lang.t("Kein Ordner geöffnet.", "No folder is open.").to_string() } else { summary_text(results, lang) };
        let color = if results.error.is_some() { crate::style::ui().error } else { style::dim(ui) };
        ui.add(egui::Label::new(RichText::new(text).small().color(color)).truncate());
    });
    if start
        && let Some(root) = root.filter(|_| !input.pattern.is_empty())
    {
        let ctx = ui.ctx().clone();
        results.start(root.to_path_buf(), input.query(settings), move || ctx.request_repaint());
    }
    ui.separator();
    egui::ScrollArea::vertical().id_salt("editor_find_results").auto_shrink([false, false]).show(ui, |ui| {
        for fi in 0..results.files.len() {
            let open = !results.collapsed.contains(&fi);
            let f = &results.files[fi];
            let head = format!("{} {}  ({})", if open { "▾" } else { "▸" }, f.rel, f.matches);
            let r = ui.add(egui::Button::selectable(false, RichText::new(head).strong()).truncate()).on_hover_text(f.path.display().to_string());
            if r.clicked() {
                if open {
                    results.collapsed.insert(fi);
                } else {
                    results.collapsed.remove(&fi);
                }
            }
            if !open {
                continue;
            }
            let f = &results.files[fi];
            for (li, hit) in f.lines.iter().enumerate() {
                let sel = results.selected == Some((fi, li));
                let mut job = line_job(ui, hit);
                // Eine Zeile, am Rand gekürzt (die ganze Zeile steht in der Datei).
                job.wrap = egui::text::TextWrapping::truncate_at_width((ui.available_width() - ui.spacing().button_padding.x * 2.0).max(40.0));
                let r = ui.selectable_label(sel, job);
                if r.clicked() {
                    action = Some(PanelAction::Open { path: f.path.clone(), line: hit.line, col: hit.col });
                }
            }
            if f.more {
                ui.label(RichText::new(lang.t("… weitere Treffer in dieser Datei", "… more results in this file")).small().color(style::dim(ui)));
            }
        }
    });
    if let Some(PanelAction::Open { path, line, .. }) = &action {
        results.selected = results.files.iter().enumerate().find_map(|(fi, f)| (f.path == *path).then(|| f.lines.iter().position(|l| l.line == *line).map(|li| (fi, li))).flatten());
    }
    if close && action.is_none() {
        action = Some(PanelAction::Close);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_takes_the_settings_defaults() {
        let s = EditorSettings { search_include_ignored: true, search_exclude: vec!["dist/".into()], large_file_mb: 5, ..Default::default() };
        let mut i = SearchInput { pattern: "x".into(), ..Default::default() };
        let q = i.query(&s);
        assert!(q.include_ignored && q.excludes == ["dist/"] && q.max_file_bytes == 5 * 1024 * 1024);
        i.include_ignored = Some(false);
        assert!(!i.query(&s).include_ignored, "der Schalter im Panel geht vor");
    }

    #[test]
    fn summary_texts_in_both_languages() {
        let mut r = Results::default();
        r.summary = Some(find::Summary { files_scanned: 10, skipped_binary: 2, truncated: true, ..Default::default() });
        let en = summary_text(&r, Lang::En);
        let de = summary_text(&r, Lang::De);
        assert!(en.contains("0 results in 0 files") && en.contains("10 files scanned") && en.contains("2 binary") && en.contains("limit"), "{en}");
        assert!(de.contains("0 Treffer in 0 Dateien") && de.contains("10 Dateien durchsucht") && de.contains("binär") && de.contains("Obergrenze"), "{de}");
        r.error = Some("bad".into());
        assert_eq!(summary_text(&r, Lang::En), "bad");
    }

    #[test]
    fn the_panel_renders_and_reports_clicks_headless() {
        let ctx = egui::Context::default();
        let mut input = SearchInput::default();
        let mut results = Results::default();
        let s = EditorSettings::default();
        let mut out = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                out = show(ui, Lang::En, &mut input, &mut results, None, &s);
            });
        });
        assert!(out.is_none());
    }

    /// Klicktest 3.0.0: Mit langen Treffern, langem Suchtext und langer Zusammenfassung bleibt das Fenster im Hauptfenster
    /// (Suchfeld, „Suchen“ und „Ignorierte“ sichtbar), wächst nicht und schrumpft mit einem kleineren Hauptfenster.
    #[test]
    fn the_window_stays_inside_the_main_window() {
        let ctx = egui::Context::default();
        let mut input = SearchInput { pattern: "z".repeat(300), ..Default::default() };
        let mut results = Results::default();
        results.files = (0..30)
            .map(|i| find::FileHits {
                path: PathBuf::from(format!("/r/f{i}.rs")),
                rel: format!("{}f{i}.rs", "deep/".repeat(40)),
                lines: (0..10).map(|l| find::LineHit { line: l + 1, col: 1, preview: "y".repeat(400), ranges: vec![(0, 3)], count: 1 }).collect(),
                matches: 10,
                more: true,
            })
            .collect();
        results.summary = Some(find::Summary { files_scanned: 123_456, skipped_binary: 2, skipped_large: 3, errors: 4, truncated: true, ..Default::default() });
        let s = EditorSettings::default();
        let root = PathBuf::from("/r");
        let mut n = 0;
        for (w, h) in [(1500.0, 960.0), (1100.0, 700.0), (600.0, 420.0)] {
            let mut rects = Vec::new();
            let mut seen = None;
            for _ in 0..8 {
                n += 1;
                let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))), time: Some(n as f64 * 0.05), ..Default::default() };
                let _ = ctx.run(raw, |ctx| {
                    let mut open = true;
                    super::super::editor_services::find_files_window(ctx, Lang::En).open(&mut open).show(ctx, |ui| {
                        let _ = show(ui, Lang::En, &mut input, &mut results, Some(&root), &s);
                        seen = Some((ui.min_rect(), ui.max_rect()));
                    });
                });
                rects.push(ctx.memory(|m| m.area_rect(Id::new("editor_find_files"))).expect("Fenster"));
            }
            let win = *rects.last().unwrap();
            assert!(win.min.x >= -0.5 && win.min.y >= -0.5 && win.max.x <= w + 0.5 && win.max.y <= h + 0.5, "{w}×{h}: Fenster {win:?} ragt hinaus");
            assert!((rects[4].width() - win.width()).abs() < 0.5 && (rects[4].height() - win.height()).abs() < 0.5, "{w}×{h}: Fenster wächst: {rects:?}");
            assert!(win.width() <= 520.0 + 40.0, "{w}×{h}: breiter als vorgesehen: {win:?}");
            let (used, avail) = seen.expect("gezeichnet");
            assert!(used.max.x <= avail.max.x + 0.5 && used.min.x >= avail.min.x - 0.5, "{w}×{h}: Inhalt {used:?} breiter als das Fenster {avail:?}");
        }
    }

    /// Esc im Suchfeld schließt das Fenster.
    #[test]
    fn escape_in_the_search_field_closes() {
        let ctx = egui::Context::default();
        let mut input = SearchInput { focus: true, ..Default::default() };
        let mut results = Results::default();
        let s = EditorSettings::default();
        let mut last = None;
        for (i, ev) in [vec![], vec![], vec![egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]].into_iter().enumerate() {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), time: Some(i as f64 * 0.05), events: ev, ..Default::default() };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    last = show(ui, Lang::En, &mut input, &mut results, None, &s);
                });
            });
            if i < 2 {
                assert!(last.is_none());
            }
        }
        assert_eq!(last, Some(PanelAction::Close));
    }

    /// Nach Enter (Suche starten) bleibt der Fokus im Suchfeld: Esc schließt das Fenster danach weiterhin.
    #[test]
    fn escape_after_enter_still_closes() {
        let ctx = egui::Context::default();
        let mut input = SearchInput { focus: true, pattern: "needle".into(), ..Default::default() };
        let mut results = Results::default();
        let s = EditorSettings::default();
        let key = |key| vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }];
        let mut last = None;
        for (i, ev) in [vec![], vec![], key(Key::Enter), vec![], vec![], key(Key::Escape)].into_iter().enumerate() {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), time: Some(i as f64 * 0.05), events: ev, ..Default::default() };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    last = show(ui, Lang::En, &mut input, &mut results, None, &s);
                });
            });
            if i < 5 {
                assert_ne!(last, Some(PanelAction::Close), "Bild {i}");
            }
        }
        assert_eq!(last, Some(PanelAction::Close));
    }
}
