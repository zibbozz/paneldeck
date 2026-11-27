//! Menüeinträge, Palette und Revisionsauswahl des Vergleichs (#385, 3.2.0).
//!
//! **Für die Menüleiste je Teilung** (eigenes Modul `menu.rs`): Die Einträge kommen als Liste ([`file_entries`] für
//! *Datei → Vergleichen mit …*, [`view_entries`] für *Ansicht → Änderungsmarken / Grundlage der Marken / nächste/vorige
//! Änderung*) oder fertig gezeichnet ([`file_menu`], [`view_menu`]) – beides liefert Befehle ([`Cmd`]) für den
//! `cmds`-Vektor des Bilds. Der Untereintrag *Vergleichen mit …* im Kontextmenü eines Reiters ist [`compare_submenu`].

use super::diff_tab::Target;
use super::view::Cmd;
use super::EditorUi;
use crate::editor_svc::git_base::{self, Commit, RepoFile, Rev};
use crate::editor_svc::settings::MarkerBase;
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Id, Key, RichText, Ui};
use std::path::PathBuf;
use std::sync::mpsc;

/// Ein Menüeintrag.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct MenuEntry {
    pub label: String,
    /// Tastenkürzel zum Anzeigen (leer = keins).
    pub shortcut: String,
    pub enabled: bool,
    /// Häkchen (Schalter); `None` = normaler Eintrag.
    pub checked: Option<bool>,
    /// Trennlinie davor.
    pub separator: bool,
    pub cmd: Cmd,
}

fn entry(label: &str, enabled: bool, cmd: Cmd) -> MenuEntry {
    MenuEntry { label: label.to_string(), shortcut: String::new(), enabled, checked: None, separator: false, cmd }
}

/// *Datei → Vergleichen mit …* für das Dokument im aktiven Reiter.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn file_entries(ed: &EditorUi, lang: Lang) -> Vec<MenuEntry> {
    file_entries_for(ed, ed.active_file_doc(), lang)
}

/// Wie [`file_entries`] für ein bestimmtes Dokument (die Menüleiste je Teilung nimmt das ihrer Teilung).
pub(super) fn file_entries_for(ed: &EditorUi, doc: Option<u64>, lang: Lang) -> Vec<MenuEntry> {
    let path = doc.and_then(|d| ed.docs.get(&d)).and_then(|d| d.path.clone());
    let (d, has_doc, has_path) = (doc.unwrap_or(0), doc.is_some(), path.is_some());
    let mut v = vec![
        entry(lang.t("Vergleichen mit Datei …", "Compare with file …"), has_doc, Cmd::Compare(d, Target::PickFile)),
        entry(lang.t("Vergleichen mit gespeicherter Datei", "Compare with saved file"), has_path, Cmd::Compare(d, Target::Disk)),
        entry(lang.t("Vergleichen mit HEAD", "Compare with HEAD"), has_path, Cmd::Compare(d, Target::Head)),
        entry(lang.t("Vergleichen mit dem Index (gestagt)", "Compare with the index (staged)"), has_path, Cmd::Compare(d, Target::Index)),
        entry(lang.t("Vergleichen mit Revision …", "Compare with revision …"), has_path, Cmd::PickRevision(path.clone().unwrap_or_default(), None)),
        entry(lang.t("Vergleichen mit der Zwischenablage", "Compare with clipboard"), has_doc, Cmd::Compare(d, Target::Clipboard)),
    ];
    v[0].separator = true;
    v
}

/// *Ansicht → Änderungsmarken*, *Grundlage der Marken*, nächste/vorige Änderung. `next`/`prev` = Kürzel zum Anzeigen.
pub(super) fn view_entries(prefs: &super::EditorPrefs, lang: Lang, next: &str, prev: &str) -> Vec<MenuEntry> {
    let mut v = vec![
        MenuEntry { checked: Some(prefs.markers), separator: true, ..entry(lang.t("Änderungsmarken", "Change markers"), true, Cmd::SetMarkers(!prefs.markers)) },
        MenuEntry { checked: Some(prefs.marker_base == MarkerBase::Git), ..entry(lang.t("Marken gegen git (Index und HEAD)", "Markers against git (index and HEAD)"), prefs.markers, Cmd::SetMarkerBase(MarkerBase::Git)) },
        MenuEntry { checked: Some(prefs.marker_base == MarkerBase::Disk), ..entry(lang.t("Marken gegen die gespeicherte Datei", "Markers against the saved file"), prefs.markers, Cmd::SetMarkerBase(MarkerBase::Disk)) },
        MenuEntry { shortcut: next.to_string(), separator: true, ..entry(lang.t("Nächste Änderung", "Next change"), true, Cmd::StepChange(true)) },
        MenuEntry { shortcut: prev.to_string(), ..entry(lang.t("Vorige Änderung", "Previous change"), true, Cmd::StepChange(false)) },
    ];
    v[0].separator = true;
    v
}

/// Einträge in ein offenes Menü zeichnen.
// Die Menüleiste je Teilung (`menu.rs`) übernimmt die Listen in ihr Modell (`menu::diff_items`); gezeichnet wird hier
// nur in Tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn draw_entries(ui: &mut Ui, entries: Vec<MenuEntry>, cmds: &mut Vec<Cmd>) {
    for e in entries {
        if e.separator {
            ui.separator();
        }
        let label = match e.checked {
            Some(true) => format!("✓ {}", e.label),
            Some(false) => format!("   {}", e.label),
            None => e.label,
        };
        let mut b = egui::Button::new(label);
        if !e.shortcut.is_empty() {
            b = b.shortcut_text(e.shortcut);
        }
        if ui.add_enabled(e.enabled, b).clicked() {
            cmds.push(e.cmd);
            ui.close();
        }
    }
}

/// *Datei → Vergleichen mit …* fertig gezeichnet (für die Menüleiste).
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn file_menu(ui: &mut Ui, ed: &EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    draw_entries(ui, file_entries(ed, lang), cmds);
}

/// *Ansicht → Änderungsmarken …* fertig gezeichnet (für die Menüleiste).
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn view_menu(ui: &mut Ui, prefs: &super::EditorPrefs, lang: Lang, next: &str, prev: &str, cmds: &mut Vec<Cmd>) {
    draw_entries(ui, view_entries(prefs, lang, next, prev), cmds);
}

/// Untermenü *Vergleichen mit …* im Kontextmenü eines Datei-Reiters: gespeicherte Datei, HEAD, Index, Revision,
/// Zwischenablage, eine Datei, andere offene Dokumente.
pub(super) fn compare_submenu(ui: &mut Ui, ed: &EditorUi, doc: u64, lang: Lang, cmds: &mut Vec<Cmd>) {
    ui.menu_button(lang.t("Vergleichen mit …", "Compare with …"), |ui| {
        let path = ed.docs.get(&doc).and_then(|d| d.path.clone());
        let has_path = path.is_some();
        let mut push = |ui: &mut Ui, label: &str, ok: bool, c: Cmd| {
            if ui.add_enabled(ok, egui::Button::new(label)).clicked() {
                cmds.push(c);
                ui.close();
            }
        };
        push(ui, lang.t("Gespeicherter Datei", "Saved file"), has_path, Cmd::Compare(doc, Target::Disk));
        push(ui, "HEAD", has_path, Cmd::Compare(doc, Target::Head));
        push(ui, lang.t("Index (gestagt)", "Index (staged)"), has_path, Cmd::Compare(doc, Target::Index));
        push(ui, lang.t("Revision …", "Revision …"), has_path, Cmd::PickRevision(path.clone().unwrap_or_default(), None));
        push(ui, lang.t("Zwischenablage", "Clipboard"), true, Cmd::Compare(doc, Target::Clipboard));
        push(ui, lang.t("Datei …", "File …"), true, Cmd::Compare(doc, Target::PickFile));
        let others: Vec<(u64, String)> = ed.panes.docs().into_iter().filter(|d| *d != doc).filter_map(|d| ed.docs.get(&d).map(|e| (d, e.title(lang)))).collect();
        if !others.is_empty() {
            ui.separator();
            for (d, t) in others {
                push(ui, &lang.q(&t), true, Cmd::Compare(doc, Target::Doc(d)));
            }
        }
    });
}

/// Einträge für die Befehlspalette (nur im Editor-Reiter).
pub(super) fn palette_entries(ed: &EditorUi, prefs: &super::EditorPrefs, lang: Lang, next: &str, prev: &str) -> Vec<(String, String, Cmd)> {
    let mut v: Vec<(String, String, Cmd)> = file_entries(ed, lang).into_iter().filter(|e| e.enabled).map(|e| (e.label, e.shortcut, e.cmd)).collect();
    for e in view_entries(prefs, lang, next, prev).into_iter().filter(|e| e.enabled) {
        let label = match (&e.cmd, e.checked) {
            (Cmd::SetMarkers(true), _) => lang.t("Änderungsmarken einschalten", "Turn change markers on").to_string(),
            (Cmd::SetMarkers(false), _) => lang.t("Änderungsmarken ausschalten", "Turn change markers off").to_string(),
            (_, Some(true)) => continue,
            _ => e.label,
        };
        v.push((label, e.shortcut, e.cmd));
    }
    v
}

// ------------------------------------------------------------------------------------------------ Revisionsauswahl

/// Daten der Auswahl (im Hilfsthread gelesen).
pub(super) struct PickerData {
    pub repo: Option<RepoFile>,
    pub branches: Vec<git_base::RefName>,
    pub tags: Vec<git_base::RefName>,
    pub commits: Vec<Commit>,
}

/// Fenster *Vergleichen mit Revision …*: HEAD, Index, Zweige, Tags, die letzten 50 Commits der Datei (mit Suche) und
/// freie Eingabe einer Revision (geprüft). Nur lokales git.
pub(crate) struct RevPicker {
    pub path: PathBuf,
    /// Linke Seite dieses Vergleichs ersetzen; `None` = neuer Vergleich mit dem Dokument der Datei.
    pub diff: Option<u64>,
    rx: Option<mpsc::Receiver<PickerData>>,
    pub(super) data: Option<PickerData>,
    pub filter: String,
    pub free: String,
    pub error: Option<String>,
    focus: bool,
}

/// Was im Fenster gewählt wurde.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Picked {
    Rev(Rev),
    Close,
}

impl RevPicker {
    pub(crate) fn new(path: PathBuf, diff: Option<u64>) -> RevPicker {
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let repo = git_base::locate(&p);
            let (branches, tags, commits) = match &repo {
                Some(r) => {
                    let (b, t) = git_base::refs(&r.root);
                    (b, t, git_base::file_log(r, 50))
                }
                None => (Vec::new(), Vec::new(), Vec::new()),
            };
            let _ = tx.send(PickerData { repo, branches, tags, commits });
        });
        RevPicker { path, diff, rx: Some(rx), data: None, filter: String::new(), free: String::new(), error: None, focus: true }
    }

    /// Freie Eingabe prüfen (Zeichen, dann `rev-parse --verify` im Repository).
    pub(super) fn check_free(&self) -> Result<Rev, String> {
        let s = self.free.trim();
        git_base::check_ref(s)?;
        let root = self.data.as_ref().and_then(|d| d.repo.as_ref()).map(|r| r.root.clone()).ok_or_else(|| crate::state::t("Die Datei liegt in keinem git-Repository.", "The file is not in a git repository.").to_string())?;
        // Repository in WSL (#392): kein git-Aufruf im UI-Thread über `\\wsl.localhost` – eine unbekannte Revision meldet
        // dann der Vergleich selbst (er liest im Hintergrund).
        if !crate::wsl::is_wsl_unc(&root.to_string_lossy()) {
            git_base::verify_ref(&root, s)?;
        }
        Ok(Rev::Commit(s.to_string()))
    }

    /// Fenster zeichnen; liefert die Wahl.
    pub(super) fn show(&mut self, ctx: &egui::Context, lang: Lang) -> Option<Picked> {
        if let Some(rx) = &self.rx
            && let Ok(d) = rx.try_recv()
        {
            self.data = Some(d);
            self.rx = None;
        }
        if self.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut picked = None;
        let mut open = true;
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let title = format!("{} – {name}", lang.t("Vergleichen mit Revision", "Compare with revision"));
        let win = style::fit_to_screen(ctx, style::movable(ctx, Id::new("ed_rev_picker"), egui::Window::new(title)), [460.0, 520.0], [320.0, 260.0]).collapsible(false).open(&mut open);
        win.show(ctx, |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(lang.t("Suchen (Zweig, Tag, Commit, Text) …", "Search (branch, tag, commit, text) …")).desired_width(f32::INFINITY).id(Id::new("ed_rev_filter")));
            if std::mem::take(&mut self.focus) {
                r.request_focus();
            }
            ui.horizontal(|ui| {
                ui.label(lang.t("Revision:", "Revision:"));
                let fr = ui.add(egui::TextEdit::singleline(&mut self.free).hint_text("main, v1.2, HEAD~2, a1b2c3d").desired_width(180.0).id(Id::new("ed_rev_free")));
                let go = ui.button(lang.t("Vergleichen", "Compare")).clicked() || (fr.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                if go {
                    match self.check_free() {
                        Ok(rv) => picked = Some(Picked::Rev(rv)),
                        Err(e) => self.error = Some(e),
                    }
                }
            });
            if let Some(e) = &self.error {
                ui.colored_label(style::ui().error, e);
            }
            ui.separator();
            let Some(data) = &self.data else {
                ui.weak(lang.t("git wird gelesen …", "reading git …"));
                return;
            };
            if data.repo.is_none() {
                ui.colored_label(style::ui().warning, lang.t("Die Datei liegt in keinem git-Repository (oder git fehlt).", "The file is not in a git repository (or git is missing)."));
                return;
            }
            let q = self.filter.trim().to_lowercase();
            let hit = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
            egui::ScrollArea::vertical().auto_shrink([false, false]).max_height((ui.available_height() - 4.0).max(120.0)).show(ui, |ui| {
                for (label, rev) in [("HEAD".to_string(), Rev::Head), (lang.t("Index (gestagt)", "Index (staged)").to_string(), Rev::Index)] {
                    if hit(&label) && ui.selectable_label(false, RichText::new(label).strong()).clicked() {
                        picked = Some(Picked::Rev(rev));
                    }
                }
                let mut section = |ui: &mut Ui, title: &str, items: Vec<(String, String, Rev)>| {
                    if items.is_empty() {
                        return;
                    }
                    ui.add_space(style::S2);
                    ui.label(RichText::new(title).small().color(style::dim(ui)));
                    for (label, hint, rev) in items {
                        let resp = ui.selectable_label(false, label);
                        let resp = if hint.is_empty() { resp } else { resp.on_hover_text(hint) };
                        if resp.clicked() {
                            picked = Some(Picked::Rev(rev));
                        }
                    }
                };
                // Volle Ref-Namen (`refs/heads/…`, `refs/tags/…`): eindeutig, auch wenn ein Zweig wie ein Tag heißt.
                section(ui, lang.t("Zweige", "Branches"), data.branches.iter().filter(|b| hit(&b.short)).map(|b| (b.short.clone(), b.full.clone(), Rev::Commit(b.full.clone()))).collect());
                section(ui, lang.t("Tags", "Tags"), data.tags.iter().filter(|t| hit(&t.short)).map(|t| (t.short.clone(), t.full.clone(), Rev::Commit(t.full.clone()))).collect());
                section(
                    ui,
                    lang.t("Letzte Commits der Datei", "Recent commits of the file"),
                    data.commits
                        .iter()
                        .filter(|c| hit(&c.short) || hit(&c.subject) || hit(&c.author) || hit(&c.hash))
                        .map(|c| (format!("{}  {}  {}", c.short, c.date, c.subject), format!("{} – {}", c.hash, c.author), Rev::Commit(c.hash.clone())))
                        .collect(),
                );
            });
        });
        if !open || (!egui::Popup::is_any_open(ctx) && ctx.input(|i| i.key_pressed(Key::Escape))) {
            picked = picked.or(Some(Picked::Close));
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_revisions_are_validated_before_use() {
        let mut p = RevPicker { path: PathBuf::from("x.txt"), diff: None, rx: None, data: None, filter: String::new(), free: "--all".into(), error: None, focus: false };
        assert!(p.check_free().is_err(), "Option statt Revision");
        p.free = "main".into();
        assert!(p.check_free().is_err(), "ohne Repository");
        p.data = Some(PickerData { repo: None, branches: vec![], tags: vec![], commits: vec![] });
        assert!(p.check_free().is_err());
    }

    #[test]
    fn menu_entries_have_commands_and_toggle_markers() {
        let prefs = super::super::EditorPrefs::default();
        let v = view_entries(&prefs, Lang::En, "F7", "Shift+F7");
        assert_eq!(v[0].cmd, Cmd::SetMarkers(true), "aus → einschalten");
        assert_eq!(v[0].checked, Some(false));
        assert!(!v[1].enabled, "Grundlage erst mit Marken");
        assert_eq!(v[3].shortcut, "F7");
        let ed = EditorUi::default();
        let f = file_entries(&ed, Lang::De);
        assert_eq!(f.len(), 6);
        assert!(f.iter().all(|e| !e.enabled), "ohne Dokument nichts zu vergleichen");
        assert!(f.iter().all(|e| !e.label.is_empty()));
        // Fertig gezeichnet (wie in der Menüleiste): ohne Panik, ohne Befehl ohne Klick.
        let ctx = egui::Context::default();
        let mut cmds = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                file_menu(ui, &ed, Lang::En, &mut cmds);
                view_menu(ui, &prefs, Lang::En, "F7", "Shift+F7", &mut cmds);
            });
        });
        assert!(cmds.is_empty());
        // Palette: Marken einschalten, F7 mit Kürzel
        let p = palette_entries(&ed, &prefs, Lang::En, "F7", "Shift+F7");
        assert!(p.iter().any(|(l, _, c)| l == "Turn change markers on" && *c == Cmd::SetMarkers(true)));
        assert!(p.iter().any(|(_, k, c)| k == "F7" && *c == Cmd::StepChange(true)));
    }
}
