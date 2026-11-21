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
pub(super) fn show(ui: &mut Ui, lang: Lang, input: &mut SearchInput, results: &mut Results, root: Option<&Path>, settings: &EditorSettings) -> Option<PanelAction> {
    let mut action = None;
    let mut start = false;
    ui.horizontal(|ui| {
        let edit = egui::TextEdit::singleline(&mut input.pattern).hint_text(lang.t("In Dateien suchen", "Search in files")).desired_width((ui.available_width() - 150.0).max(80.0));
        let r = ui.add(edit);
        if std::mem::take(&mut input.focus) {
            r.request_focus();
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            start = true;
        }
        for (on, label, tip) in [
            (&mut input.case_sensitive, "Aa", lang.t("Groß-/Kleinschreibung beachten", "Match case")),
            (&mut input.whole_word, "W", lang.t("Nur ganze Wörter", "Whole word")),
            (&mut input.regex, ".*", lang.t("Regulärer Ausdruck", "Regular expression")),
        ] {
            if ui.selectable_label(*on, RichText::new(label).monospace()).on_hover_text(tip).clicked() {
                *on = !*on;
            }
        }
        let mut inc = input.include_ignored.unwrap_or(settings.search_include_ignored);
        if ui
            .selectable_label(inc, lang.t("Ignorierte", "Ignored"))
            .on_hover_text(lang.t("Ignorierte einbeziehen: auch Dateien, die .gitignore ausschließt (eigene Ausschlussmuster gelten weiter)", "Include ignored: also files excluded by .gitignore (your own exclude patterns still apply)"))
            .clicked()
        {
            inc = !inc;
            input.include_ignored = Some(inc);
        }
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
        ui.label(RichText::new(text).small().color(color));
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
            let r = ui.selectable_label(false, RichText::new(head).strong()).on_hover_text(f.path.display().to_string());
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
                let r = ui.selectable_label(sel, line_job(ui, hit));
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
}
