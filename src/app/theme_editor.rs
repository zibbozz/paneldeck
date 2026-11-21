//! Theme-Editor und Verwaltung eigener Themes (#309): *Einstellungen → Darstellung* (Bearbeiten, Als Kopie bearbeiten,
//! Neu, Umbenennen, Duplizieren, Löschen, Im Explorer zeigen, Import/Export) und das Editor-Fenster mit Farbliste links
//! (Gruppen, Suche, Markierung geänderter Werte, Zurücksetzen auf abgeleitet), Vorschau rechts (echte Bedienelemente)
//! und Design-Prüfung unten. Die App ändert sich erst nach „Speichern“.

use super::theme_preview::{self, Marks, Sample};
use super::toasts::ToastKind;
use super::App;
use crate::state::Lang;
use crate::style;
use crate::theme::checks::{self, Hint, Rule};
use crate::theme::color::{contrast, over, parse_hex, to_hex};
use crate::theme::file;
use crate::theme::generate::{self, Seed};
use crate::theme::tokens::{self, all_tokens, Group, Spec, Token};
use crate::theme::{self, Theme};
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Id, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::sync::Arc;

/// Zustand der Theme-Verwaltung in der App.
#[derive(Default)]
pub(super) struct ThemeUi {
    pub(super) editor: Option<Editor>,
    rename: Option<Rename>,
    delete_ask: Option<String>,
}

struct Rename {
    old: String,
    buf: String,
    err: Option<String>,
}

/// Ein offener Editor (Arbeitskopie und zuletzt gespeicherter Stand).
pub(super) struct Editor {
    theme: Theme,
    saved: Theme,
    selected: Option<&'static str>,
    search: String,
    hex: String,
    hex_for: Option<(&'static str, Color32)>,
    hex_err: bool,
    link_to: &'static str,
    link_alpha: u8,
    link_for: Option<&'static str>,
    undo: Vec<Theme>,
    redo: Vec<Theme>,
    last_edit: f64,
    generator: Option<Seed>,
    closing: bool,
    error: Option<String>,
    highlight: Vec<&'static str>,
    scroll_to: Option<&'static str>,
    sample: Sample,
    marks: Marks,
    hints: Vec<Hint>,
    hints_for: Option<(Theme, Vec<String>)>,
    /// Zeitpunkt der letzten Design-Prüfung (Entprellen beim Ziehen).
    hints_at: f64,
    style_for: Option<(Theme, Arc<egui::Style>)>,
}

/// Was der Editor nach dem Zeichnen von der App braucht.
enum EdAction {
    Save,
    Discard,
    ToggleRule(Rule),
}

const UNDO_MAX: usize = 100;
/// Bearbeitungen innerhalb dieser Zeit (Ziehen im Farbwähler) zählen als ein Rückgängig-Schritt.
const UNDO_MERGE_S: f64 = 0.6;
/// Design-Prüfung höchstens so oft neu rechnen (Ziehen im Farbwähler ändert das Theme jedes Bild).
const HINTS_EVERY_S: f64 = 0.15;

impl Editor {
    fn new(theme: Theme, saved: Theme) -> Editor {
        Editor {
            theme,
            saved,
            selected: None,
            search: String::new(),
            hex: String::new(),
            hex_for: None,
            hex_err: false,
            link_to: "text",
            link_alpha: 100,
            link_for: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: f64::NEG_INFINITY,
            generator: None,
            closing: false,
            error: None,
            highlight: Vec::new(),
            scroll_to: None,
            sample: Sample::default(),
            marks: Marks::new(),
            hints: Vec::new(),
            hints_for: None,
            hints_at: f64::NEG_INFINITY,
            style_for: None,
        }
    }

    /// Ungespeicherte Änderungen (ein neues, nie gespeichertes Theme zählt immer).
    fn dirty(&self) -> bool {
        self.theme.source.is_none() || self.theme != self.saved
    }

    /// Änderung mit Rückgängig-Schritt; schnelle Folgen (Ziehen) werden zusammengefasst.
    fn edit(&mut self, now: f64, merge: bool, f: impl FnOnce(&mut Theme)) {
        let before = self.theme.clone();
        f(&mut self.theme);
        if self.theme == before {
            return;
        }
        if !(merge && now - self.last_edit < UNDO_MERGE_S) {
            self.undo.push(before);
            if self.undo.len() > UNDO_MAX {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = if merge { now } else { f64::NEG_INFINITY };
        self.hex_for = None;
        self.error = None;
    }

    /// Wie [`Editor::edit`] (ohne Zusammenfassen), aber nur, wenn dadurch kein Kreis aus Verweisen entsteht – sonst
    /// bleibt alles, wie es ist, mit Meldung. Für Änderungen, die Ableitungen umstellen (hell/dunkel, Zurücksetzen,
    /// Generator). `false`, wenn abgelehnt.
    fn edit_checked(&mut self, now: f64, lang: Lang, f: impl FnOnce(&mut Theme)) -> bool {
        let mut test = self.theme.clone();
        f(&mut test);
        if let Some(cy) = tokens::find_cycle(&test) {
            // Ein schon vorhandener Kreis blockiert nichts (sonst ließe er sich nicht mehr beheben).
            if tokens::find_cycle(&self.theme).is_none() {
                self.error = Some(format!("{} {}", lang.t("Nicht übernommen – Verweise im Kreis:", "Not applied – links would form a loop:"), cy.join(" → ")));
                return false;
            }
        }
        self.edit(now, false, |t| *t = test);
        true
    }

    fn undo(&mut self) {
        if let Some(t) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.theme, t));
            self.hex_for = None;
            self.last_edit = f64::NEG_INFINITY;
            // Eine Meldung zu einer abgelehnten Änderung passt nach einem Schritt zurück nicht mehr.
            self.error = None;
        }
    }

    fn redo(&mut self) {
        if let Some(t) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.theme, t));
            self.hex_for = None;
            self.last_edit = f64::NEG_INFINITY;
            self.error = None;
        }
    }

    /// Nach erfolgreichem Speichern unter `path`: Stand merken; eine alte Meldung (z. B. ein abgelehnter Kreis)
    /// verschwindet.
    fn mark_saved(&mut self, path: std::path::PathBuf) {
        self.theme.source = Some(path);
        self.saved = self.theme.clone();
        self.error = None;
    }

    fn select(&mut self, name: &'static str) {
        self.selected = Some(name);
        self.hex_for = None;
        self.link_for = None;
    }

    /// Gesetzt (nicht abgeleitet)? Terminalfarben sind immer gesetzt.
    fn is_set(&self, name: &str) -> bool {
        !tokens::is_ui(name) || self.theme.ui.contains_key(name)
    }

    /// Seit dem letzten Speichern geändert?
    fn modified(&self, name: &str) -> bool {
        tokens::resolve(&self.theme, name) != tokens::resolve(&self.saved, name) || self.theme.ui.get(name) != self.saved.ui.get(name)
    }

    /// Design-Prüfung nach einer Änderung neu rechnen, höchstens alle [`HINTS_EVERY_S`]. `true`, wenn eine Prüfung
    /// noch aussteht (der Aufrufer zeichnet dann nach der Wartezeit erneut).
    fn refresh_hints(&mut self, off: &[String], now: f64) -> bool {
        if matches!(&self.hints_for, Some((t, o)) if *t == self.theme && o.as_slice() == off) {
            return false;
        }
        if self.hints_for.is_some() && now - self.hints_at < HINTS_EVERY_S {
            return true;
        }
        self.hints = checks::run(&self.theme, |r| !off.iter().any(|o| o == r.key()));
        self.hints_for = Some((self.theme.clone(), off.to_vec()));
        self.hints_at = now;
        false
    }

    fn preview_style(&mut self, base: &egui::Style) -> Arc<egui::Style> {
        if let Some((t, s)) = &self.style_for {
            if *t == self.theme {
                return s.clone();
            }
        }
        let mut st = base.clone();
        let accent = self.theme.ui_colors().accent;
        style::build_style(&mut st, &self.theme, 1.0, accent);
        let s = Arc::new(st);
        self.style_for = Some((self.theme.clone(), s.clone()));
        s
    }
}

fn matches(tok: &Token, q: &str, lang: Lang) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || tok.name.contains(&q) || lang.t(tok.de, tok.en).to_lowercase().contains(&q)
}

/// Kleines Farbfeld mit Schachbrett hinter durchscheinenden Farben.
fn swatch(p: &egui::Painter, r: Rect, c: Color32, border: Color32) {
    if c.a() < 255 {
        p.rect_filled(r, CornerRadius::same(3), Color32::from_gray(200));
        let h = r.width() / 2.0;
        for (dx, dy) in [(0.0, 0.0), (h, h)] {
            p.rect_filled(Rect::from_min_size(r.min + Vec2::new(dx, dy), Vec2::splat(h)), CornerRadius::ZERO, Color32::from_gray(120));
        }
    }
    p.rect_filled(r, CornerRadius::same(3), c);
    p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0_f32, border), StrokeKind::Inside);
}

/// Deckende Farbe eines Namens über ihrem üblichen Untergrund (für Kontrastangaben).
fn solid(t: &Theme, name: &str) -> Color32 {
    let c = tokens::resolve(t, name);
    if c.a() == 255 {
        c
    } else {
        let base = tokens::resolve(t, "window_bg");
        over(c, if base.a() == 255 { base } else { over(base, t.bg.to_opaque()) })
    }
}

impl App {
    /// Editor für das Theme `name` öffnen; eingebaute Themes als Kopie („One Dark (2)“), die erst beim Speichern als
    /// Datei entsteht.
    pub(super) fn theme_open_editor(&mut self, name: &str) {
        let Some(t) = self.themes.iter().find(|t| t.name == name).cloned() else { return };
        if t.builtin {
            let mut copy = t;
            copy.name = file::unique_name(&copy.name, &self.theme_names());
            copy.builtin = false;
            copy.source = None;
            self.theme_ui.editor = Some(Editor::new(copy.clone(), copy));
        } else {
            self.theme_ui.editor = Some(Editor::new(t.clone(), t));
        }
    }

    /// Neues Theme: aus Hintergrund und Akzent des aktuellen Themes erzeugt, mit offenem Generator.
    pub(super) fn theme_new(&mut self) {
        let lang = self.settings.language;
        let seed = Seed::from_theme(&self.theme());
        let name = file::unique_name(lang.t("Mein Theme", "My theme"), &self.theme_names());
        let t = generate::generate(&name, seed);
        let mut ed = Editor::new(t.clone(), t);
        ed.generator = Some(seed);
        self.theme_ui.editor = Some(ed);
    }

    fn theme_names(&self) -> Vec<String> {
        self.themes.iter().map(|t| t.name.clone()).chain(self.theme_ui.editor.as_ref().filter(|e| e.theme.source.is_none()).map(|e| e.theme.name.clone())).collect()
    }

    /// Name prüfen: nicht leer, höchstens 80 Zeichen, von keinem anderen Theme belegt (ohne `own`, das eigene Theme).
    /// `kept`: bisheriger Name der Datei – bleibt er unverändert, gilt nur, dass kein anderes Theme genau so heißt
    /// (alte Dateien mit langen Namen oder „one dark“ neben „One Dark“ lassen sich weiter speichern).
    fn theme_name_error(&self, name: &str, own: Option<&std::path::Path>, kept: Option<&str>) -> Option<String> {
        let lang = self.settings.language;
        let other = |t: &&Theme| own.is_none() || t.source.as_deref() != own;
        if kept == Some(name) {
            let taken = self.themes.iter().filter(other).any(|t| t.name == name);
            return taken.then(|| format!("{} {}", lang.t("Dieser Name ist schon vergeben:", "This name is already used:"), lang.q(&file::short(name))));
        }
        let n = name.trim();
        if n.is_empty() {
            return Some(lang.t("Der Name ist leer.", "The name is empty.").into());
        }
        if n.chars().count() > 80 {
            return Some(lang.t("Der Name ist länger als 80 Zeichen.", "The name is longer than 80 characters.").into());
        }
        let taken = self.themes.iter().filter(other).any(|t| t.name.eq_ignore_ascii_case(n));
        taken.then(|| format!("{} {}", lang.t("Dieser Name ist schon vergeben:", "This name is already used:"), lang.q(n)))
    }

    /// Verweise auf ein umbenanntes Theme (Auswahl, Profile) nachziehen.
    fn theme_renamed(&mut self, old: &str, new: &str) {
        if self.settings.theme == old {
            self.settings.theme = new.to_string();
        }
        for p in &mut self.settings.profiles {
            if p.theme == old {
                p.theme = new.to_string();
            }
        }
    }

    /// Arbeitskopie speichern (neue Datei oder bestehende), Themes neu laden und das Theme auswählen.
    fn theme_save(&mut self, ed: &mut Editor) -> Result<(), String> {
        let lang = self.settings.language;
        // Der Name einer bestehenden Datei bleibt, wie er ist, solange er nicht geändert wurde (auch mit Leerzeichen).
        let kept = ed.saved.source.as_ref().map(|_| ed.saved.name.clone());
        if kept.as_deref() != Some(ed.theme.name.as_str()) {
            ed.theme.name = ed.theme.name.trim().to_string();
        }
        if let Some(e) = self.theme_name_error(&ed.theme.name, ed.theme.source.as_deref(), kept.as_deref()) {
            return Err(e);
        }
        if let Some(c) = tokens::find_cycle(&ed.theme) {
            return Err(format!("{} {}", lang.t("Verweise im Kreis:", "Links form a loop:"), c.join(" → ")));
        }
        let path = match &ed.theme.source {
            Some(p) => p.clone(),
            None => file::file_for(&self.themes_dir(), &ed.theme.name),
        };
        file::write(&path, &ed.theme).map_err(|e| format!("{} {e}", lang.t("Speichern fehlgeschlagen:", "Saving failed:")))?;
        let old = ed.saved.source.as_ref().map(|_| ed.saved.name.clone());
        ed.mark_saved(path);
        self.reload_themes();
        if let Some(old) = old.filter(|o| *o != ed.theme.name) {
            let old: String = old;
            self.theme_renamed(&old, &ed.theme.name);
        }
        self.settings.theme = ed.theme.name.clone();
        Ok(())
    }

    /// Eigenes Theme duplizieren („… (2)“) und als Datei anlegen.
    fn theme_duplicate(&mut self, name: &str) {
        let lang = self.settings.language;
        let Some(mut t) = self.themes.iter().find(|t| t.name == name).cloned() else { return };
        t.name = file::unique_name(name, &self.theme_names());
        t.builtin = false;
        let path = file::file_for(&self.themes_dir(), &t.name);
        match file::write(&path, &t) {
            Ok(()) => {
                let msg = format!("{} {}", lang.t("Theme angelegt:", "Theme created:"), lang.q(&t.name));
                self.reload_themes();
                self.push_toast(msg, ToastKind::Info, None);
            }
            Err(e) => self.push_toast(format!("{} {e}", lang.t("Duplizieren fehlgeschlagen:", "Duplicating failed:")), ToastKind::Error, None),
        }
    }

    fn theme_delete(&mut self, name: &str) {
        let lang = self.settings.language;
        if self.theme_ui.editor.is_some() {
            return;
        }
        let Some(path) = self.themes.iter().find(|t| t.name == name && !t.builtin).and_then(|t| t.source.clone()) else { return };
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.reload_themes();
                if self.settings.theme == name {
                    self.settings.theme = self.themes[0].name.clone();
                }
                self.push_toast(format!("{} {}", lang.t("Theme gelöscht:", "Theme deleted:"), lang.q(name)), ToastKind::Info, None);
            }
            Err(e) => self.push_toast(format!("{} {e}", lang.t("Löschen fehlgeschlagen:", "Deleting failed:")), ToastKind::Error, None),
        }
    }

    fn theme_rename(&mut self, old: &str, new: &str) -> Result<(), String> {
        let lang = self.settings.language;
        if self.theme_ui.editor.is_some() {
            return Err(lang.t("Erst den Theme-Editor schließen (speichern oder verwerfen).", "Close the theme editor first (save or discard).").into());
        }
        let Some(t) = self.themes.iter().find(|t| t.name == old && !t.builtin).cloned() else { return Err(lang.t("Theme nicht gefunden.", "Theme not found.").into()) };
        // Unverändert übernommen (auch ein alter Name mit Leerzeichen am Rand): nichts zu tun.
        if new == old || new.trim() == old {
            return Ok(());
        }
        let new = new.trim();
        if let Some(e) = self.theme_name_error(new, t.source.as_deref(), None) {
            return Err(e);
        }
        let mut t2 = t.clone();
        t2.name = new.to_string();
        let path = t.source.clone().unwrap_or_else(|| file::file_for(&self.themes_dir(), new));
        file::write(&path, &t2).map_err(|e| format!("{} {e}", lang.t("Speichern fehlgeschlagen:", "Saving failed:")))?;
        self.reload_themes();
        self.theme_renamed(old, new);
        Ok(())
    }

    /// Theme-Datei importieren (höchstens 1 MB, geprüft); bei Namensgleichheit „… (2)“. Nichts wird überschrieben.
    fn theme_import(&mut self) {
        let lang = self.settings.language;
        let Some(src) = crate::platform::notify::open_json() else { return };
        match file::read_file(&src) {
            Ok(mut t) => {
                t.name = file::unique_name(&t.name, &self.theme_names());
                t.source = None;
                let path = file::file_for(&self.themes_dir(), &t.name);
                match file::write(&path, &t) {
                    Ok(()) => {
                        self.reload_themes();
                        self.push_toast(format!("{} {}", lang.t("Theme importiert:", "Theme imported:"), lang.q(&t.name)), ToastKind::Info, None);
                    }
                    Err(e) => self.push_toast(format!("{} {e}", lang.t("Import fehlgeschlagen:", "Import failed:")), ToastKind::Error, None),
                }
            }
            Err(msg) => self.push_toast(format!("{} {}", lang.t("Import abgelehnt:", "Import refused:"), msg.get(lang)), ToastKind::Error, None),
        }
    }

    fn theme_export(&mut self, name: &str) {
        let lang = self.settings.language;
        let Some(t) = self.themes.iter().find(|t| t.name == name).cloned() else { return };
        let default = file::file_for(std::path::Path::new(""), &t.name).to_string_lossy().to_string();
        let Some(dest) = crate::platform::notify::save_json(&default) else { return };
        match std::fs::write(&dest, file::to_json(&t)) {
            Ok(()) => self.push_toast(format!("{} {}", lang.t("Theme exportiert:", "Theme exported:"), dest.display()), ToastKind::Info, None),
            Err(e) => self.push_toast(format!("{} {e}", lang.t("Export fehlgeschlagen:", "Export failed:")), ToastKind::Error, None),
        }
    }

    /// Zeile unter der Theme-Wahl (*Einstellungen → Darstellung*): Bearbeiten, Neu, weitere Aktionen; ungültige Dateien.
    pub(super) fn theme_manage_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        let cur = self.themes.iter().find(|t| t.name == self.settings.theme).or(self.themes.first()).cloned();
        let Some(cur) = cur else { return };
        let editing = self.theme_ui.editor.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.add_space(248.0);
            let (label, tip) = if cur.builtin {
                (lang.t("Als Kopie bearbeiten …", "Edit as copy …"), lang.t("Eingebaute Themes sind schreibgeschützt; bearbeitet wird eine Kopie.", "Built-in themes are read-only; you edit a copy."))
            } else {
                (lang.t("Theme bearbeiten …", "Edit theme …"), lang.t("Farben dieses Themes ändern", "Change the colours of this theme"))
            };
            if ui.add_enabled(!editing, egui::Button::new(label)).on_hover_text(tip).clicked() {
                self.theme_open_editor(&cur.name);
            }
            if ui.add_enabled(!editing, egui::Button::new(lang.t("Neues Theme …", "New theme …"))).on_hover_text(lang.t("Aus wenigen Farben erzeugen und anpassen", "Generate from a few colours and fine-tune")).clicked() {
                self.theme_new();
            }
            // Solange der Editor offen ist, keine Datei-Aktionen, die seinen Stand überholen würden (ein Umbenennen
            // würde beim Speichern still zurückgenommen, Profile zeigten dann ins Leere).
            let busy = lang.t("Erst den Theme-Editor schließen (speichern oder verwerfen).", "Close the theme editor first (save or discard).");
            ui.menu_button("⋯", |ui| {
                if ui.add_enabled(!cur.builtin && !editing, egui::Button::new(lang.t("Umbenennen …", "Rename …"))).on_disabled_hover_text(if editing { busy } else { lang.t("Eingebaute Themes lassen sich nicht umbenennen.", "Built-in themes cannot be renamed.") }).clicked() {
                    self.theme_ui.rename = Some(Rename { old: cur.name.clone(), buf: cur.name.clone(), err: None });
                    ui.close();
                }
                if ui.button(lang.t("Duplizieren", "Duplicate")).clicked() {
                    self.theme_duplicate(&cur.name);
                    ui.close();
                }
                if ui.add_enabled(!cur.builtin && !editing, egui::Button::new(lang.t("Löschen …", "Delete …"))).on_disabled_hover_text(if editing { busy } else { lang.t("Eingebaute Themes lassen sich nicht löschen.", "Built-in themes cannot be deleted.") }).clicked() {
                    self.theme_ui.delete_ask = Some(cur.name.clone());
                    ui.close();
                }
                if crate::platform::open::FILE_MANAGER {
                    if let Some(p) = &cur.source {
                        if ui.button(lang.t("Im Explorer zeigen", "Show in Explorer")).clicked() {
                            crate::platform::open::show_in_file_manager(&p.to_string_lossy(), false);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.add_enabled(!editing, egui::Button::new(lang.t("Importieren …", "Import …"))).on_disabled_hover_text(busy).clicked() {
                        ui.close();
                        self.theme_import();
                    }
                    if ui.button(lang.t("Exportieren …", "Export …")).clicked() {
                        ui.close();
                        self.theme_export(&cur.name);
                    }
                }
            });
        });
        if !self.theme_issues.is_empty() {
            ui.add_space(4.0);
            for is in &self.theme_issues {
                let file = is.path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                ui.label(RichText::new(format!("⚠ themes/{file}: {}", is.msg.get(lang))).small().color(style::ui().warning)).on_hover_text(lang.t(
                    "Diese Datei ist ungültig und kann erst gewählt werden, wenn sie korrigiert ist (danach „Neu laden“).",
                    "This file is invalid and cannot be selected until it is fixed (then “Reload”).",
                ));
            }
        }
    }

    /// Editor-Fenster und Rückfragen der Theme-Verwaltung (nur im Hauptfenster).
    pub(super) fn theme_ui_frame(&mut self, ctx: &egui::Context) {
        if self.wins.cur != 0 {
            return;
        }
        self.theme_rename_ui(ctx);
        self.theme_delete_ui(ctx);
        let Some(mut ed) = self.theme_ui.editor.take() else { return };
        let lang = self.settings.language;
        let now = ctx.input(|i| i.time);
        let off = self.settings.theme_checks_off.clone();
        if ed.refresh_hints(&off, now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(HINTS_EVERY_S));
        }
        let mut open = true;
        let mut action = None;
        let title = format!("{} – {}", lang.t("Theme-Editor", "Theme editor"), ed.theme.name);
        let base_style = (*ctx.style()).clone();
        editor_window(ctx, title).open(&mut open).show(ctx, |ui| body(&mut ed, ui, lang, now, &off, &base_style, &mut action));
        let mut close = false;
        if !open {
            if ed.dirty() {
                ed.closing = true;
            } else {
                close = true;
            }
        }
        if ed.generator.is_some() {
            generator_ui(&mut ed, ctx, lang, now);
        }
        if ed.closing {
            style::modal(Id::new("pd_theme_close")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(lang.t("Ungespeicherte Änderungen", "Unsaved changes")).heading());
                ui.label(format!("{} {}", lang.q(&ed.theme.name), lang.t("hat Änderungen, die noch nicht gespeichert sind.", "has changes that are not saved yet.")));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new(lang.t("Speichern", "Save")).strong()).fill(style::brand().gamma_multiply(0.7))).clicked() {
                        action = Some(EdAction::Save);
                        ed.closing = false;
                        close = true;
                    }
                    if ui.button(lang.t("Verwerfen", "Discard")).clicked() {
                        action = Some(EdAction::Discard);
                        ed.closing = false;
                    }
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        ed.closing = false;
                    }
                });
            });
        }
        match action {
            Some(EdAction::Save) => match self.theme_save(&mut ed) {
                Ok(()) => {
                    let msg = format!("{} {}", lang.t("Theme gespeichert:", "Theme saved:"), lang.q(&ed.theme.name));
                    self.push_toast(msg, ToastKind::Info, None);
                }
                Err(e) => {
                    ed.error = Some(e);
                    close = false;
                }
            },
            Some(EdAction::Discard) => close = true,
            Some(EdAction::ToggleRule(r)) => {
                let k = r.key().to_string();
                if let Some(i) = self.settings.theme_checks_off.iter().position(|o| *o == k) {
                    self.settings.theme_checks_off.remove(i);
                } else {
                    self.settings.theme_checks_off.push(k);
                }
            }
            None => {}
        }
        if !close {
            self.theme_ui.editor = Some(ed);
        }
    }

    fn theme_rename_ui(&mut self, ctx: &egui::Context) {
        let Some(mut r) = self.theme_ui.rename.take() else { return };
        let lang = self.settings.language;
        let (mut done, mut cancel) = (false, false);
        style::modal(Id::new("pd_theme_rename")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(lang.t("Theme umbenennen", "Rename theme")).heading());
            let resp = ui.add(egui::TextEdit::singleline(&mut r.buf).desired_width(f32::INFINITY));
            resp.request_focus();
            if let Some(e) = &r.err {
                ui.label(RichText::new(e).small().color(style::ui().error));
            }
            ui.horizontal(|ui| {
                if ui.button(lang.t("Umbenennen", "Rename")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
                if ui.button(lang.t("Abbrechen", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });
        });
        if done {
            match self.theme_rename(&r.old, &r.buf) {
                Ok(()) => return,
                Err(e) => r.err = Some(e),
            }
        }
        if !cancel {
            self.theme_ui.rename = Some(r);
        }
    }

    fn theme_delete_ui(&mut self, ctx: &egui::Context) {
        let Some(name) = self.theme_ui.delete_ask.take() else { return };
        let lang = self.settings.language;
        let (mut yes, mut no) = (false, false);
        style::modal(Id::new("pd_theme_delete")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(lang.t("Theme löschen?", "Delete theme?")).heading());
            ui.label(format!("{} {}", lang.q(&name), lang.t("wird gelöscht (die Datei im Ordner themes).", "will be deleted (its file in the themes folder).")));
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(lang.t("Löschen", "Delete")).color(Color32::WHITE)).fill(style::ui().danger_button)).clicked() {
                    yes = true;
                }
                if ui.button(lang.t("Abbrechen", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    no = true;
                }
            });
        });
        if yes {
            self.theme_delete(&name);
        } else if !no {
            self.theme_ui.delete_ask = Some(name);
        }
    }
}

/// Das Editor-Fenster (ohne Schließen-Knopf; den setzt der Aufrufer). Anfangs höchstens so groß wie das Hauptfenster
/// abzüglich eines Randes – egui begrenzt nur den Inhalt, der Rahmen ragte sonst über den Rand. Die Höchstgröße folgt
/// dem Hauptfenster in jedem Bild: egui merkt sich die Größe, ein verkleinertes Hauptfenster schnitte sonst ab.
fn editor_window<'a>(ctx: &egui::Context, title: String) -> egui::Window<'a> {
    const MIN: [f32; 2] = [860.0, 540.0];
    let screen = ctx.content_rect().size();
    egui::Window::new(title)
        .id(Id::new("pd_theme_editor"))
        .default_size([1240.0_f32.min(screen.x - 24.0), 800.0_f32.min(screen.y - 24.0)])
        .min_size(MIN)
        .max_size([(screen.x - 40.0).max(MIN[0]), (screen.y - 80.0).max(MIN[1])])
        .resizable(true)
        .collapsible(false)
        .constrain(true)
}

/// Inhalt des Editor-Fensters.
fn body(ed: &mut Editor, ui: &mut Ui, lang: Lang, now: f64, off: &[String], base_style: &egui::Style, action: &mut Option<EdAction>) {
    // Kopfzeile: Name, hell/dunkel, Generator, Rückgängig, Prüfregeln, Speichern/Verwerfen. Speichern/Verwerfen
    // stehen rechts und werden zuerst gesetzt; der Rest nimmt nur die übrige Breite und bricht in schmalen Fenstern
    // in eine zweite Zeile um, statt die Knöpfe aus dem Fenster zu schieben.
    let dirty = ed.dirty();
    let (_, right) = egui::Sides::new().shrink_left().show(
        ui,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(lang.t("Name", "Name"));
                let mut name = ed.theme.name.clone();
                if ui.add(egui::TextEdit::singleline(&mut name).desired_width(220.0)).changed() {
                    ed.edit(now, true, |t| t.name = name);
                }
                let mut dark = ed.theme.dark;
                if ui.checkbox(&mut dark, lang.t("Dunkel", "Dark")).on_hover_text(lang.t("Bestimmt, wie abgeleitete Farben berechnet werden", "Decides how derived colours are calculated")).changed() {
                    // Hell/dunkel stellt Ableitungen um – das darf mit eigenen Verweisen keinen Kreis ergeben.
                    ed.edit_checked(now, lang, |t| t.dark = dark);
                }
                ui.separator();
                if ui.button(lang.t("Aus Farben erzeugen …", "Generate from colours …")).on_hover_text(lang.t("Hintergrund, Akzent und hell/dunkel wählen – alles andere wird berechnet", "Pick background, accent and light/dark – everything else is calculated")).clicked() {
                    ed.generator = Some(Seed::from_theme(&ed.theme));
                }
                if ui.add_enabled(!ed.undo.is_empty(), egui::Button::new("↶")).on_hover_text(lang.t("Rückgängig (Strg+Z)", "Undo (Ctrl+Z)")).clicked() {
                    ed.undo();
                }
                if ui.add_enabled(!ed.redo.is_empty(), egui::Button::new("↷")).on_hover_text(lang.t("Wiederholen (Strg+Y)", "Redo (Ctrl+Y)")).clicked() {
                    ed.redo();
                }
                ui.menu_button(format!("⚙ {}", lang.t("Prüfungen", "Checks")), |ui| {
                    ui.label(RichText::new(lang.t("Regeln der Design-Prüfung (nur Hinweise)", "Design check rules (hints only)")).small().color(style::dim(ui)));
                    for r in Rule::ALL {
                        let mut on = !off.iter().any(|o| o == r.key());
                        if ui.checkbox(&mut on, r.label(lang)).on_hover_text(r.desc(lang)).changed() {
                            *action = Some(EdAction::ToggleRule(r));
                        }
                    }
                });
            });
        },
        |ui| {
            let mut act = None;
            if ui.button(lang.t("Verwerfen", "Discard")).on_hover_text(lang.t("Änderungen verwerfen und schließen", "Discard the changes and close")).clicked() {
                act = Some(EdAction::Discard);
            }
            let save = egui::Button::new(RichText::new(lang.t("Speichern", "Save")).strong()).fill(style::brand().gamma_multiply(0.7));
            if ui.add_enabled(dirty, save).on_hover_text(lang.t("Speichern und in der App verwenden", "Save and use it in the app")).clicked() {
                act = Some(EdAction::Save);
            }
            if dirty {
                ui.label(RichText::new(lang.t("● ungespeichert", "● unsaved")).small().color(style::ui().warning));
            }
            act
        },
    );
    if right.is_some() {
        *action = right;
    }
    // Strg+Z / Strg+Y, solange kein Textfeld den Fokus hat.
    if ui.ctx().memory(|m| m.focused().is_none()) {
        let (z, y) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z), i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)));
        if z {
            ed.undo();
        }
        if y {
            ed.redo();
        }
    }
    if let Some(e) = &ed.error {
        ui.label(RichText::new(e).color(style::ui().error));
    }
    ui.separator();
    let avail = ui.available_size();
    let list_w = 350.0_f32.min(avail.x * 0.4);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(list_w, avail.y), Layout::top_down(Align::Min), |ui| {
            ui.set_min_size(Vec2::new(list_w, avail.y));
            list(ed, ui, lang, now);
        });
        ui.separator();
        let right_w = ui.available_width();
        ui.allocate_ui_with_layout(Vec2::new(right_w, avail.y), Layout::top_down(Align::Min), |ui| {
            ui.set_min_size(Vec2::new(right_w, avail.y));
            let checks_h = (avail.y * 0.26).clamp(120.0, 240.0);
            let prev_h = (avail.y - checks_h - 16.0).max(160.0);
            preview(ed, ui, lang, base_style, prev_h);
            ui.separator();
            checks_panel(ed, ui, lang, now);
        });
    });
}

fn list(ed: &mut Editor, ui: &mut Ui, lang: Lang, now: f64) {
    ui.add(egui::TextEdit::singleline(&mut ed.search).hint_text(lang.t("Farbe suchen …", "Search colours …")).desired_width(f32::INFINITY));
    ui.horizontal(|ui| {
        ui.label(RichText::new(lang.t("● gesetzt   ○ abgeleitet   * geändert", "● set   ○ derived   * changed")).small().color(style::dim(ui)));
    });
    // Beide Richtungen: Zu breiter Inhalt rollt, statt das Fenster über den Rand wachsen zu lassen.
    let out = egui::ScrollArea::both().id_salt("pd_theme_list").auto_shrink([false, false]).show(ui, |ui| {
        for g in Group::ALL {
            let toks: Vec<&'static Token> = all_tokens().filter(|t| t.group == g && matches(t, &ed.search, lang)).collect();
            if toks.is_empty() {
                continue;
            }
            ui.add_space(6.0);
            ui.label(RichText::new(g.label(lang)).strong());
            for tok in toks {
                row(ed, ui, tok, lang, now);
            }
        }
    });
    note_overflow(ui, &out);
}

/// Nur in Tests: merkt sich, wie weit der Inhalt einer Rollfläche breiter ist als sie (es soll nie seitlich gerollt
/// werden müssen; der Rollbalken ist nur die Sicherung gegen ein wachsendes Fenster).
fn note_overflow<R>(ui: &Ui, out: &egui::scroll_area::ScrollAreaOutput<R>) {
    if cfg!(test) {
        let over = out.content_size.x - out.inner_rect.width();
        ui.ctx().data_mut(|d| {
            let v = d.get_temp_mut_or(Id::new("pd_theme_overflow"), 0.0_f32);
            *v = v.max(over);
        });
    }
}

fn row(ed: &mut Editor, ui: &mut Ui, tok: &'static Token, lang: Lang, now: f64) {
    let name = tok.name;
    let c = tokens::resolve(&ed.theme, name);
    let sel = ed.selected == Some(name);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    if ed.scroll_to == Some(name) {
        resp.scroll_to_me(Some(Align::Center));
        ed.scroll_to = None;
    }
    let v = ui.visuals();
    let p = ui.painter();
    if sel {
        p.rect_filled(rect, CornerRadius::same(6), style::accent().gamma_multiply(0.22));
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(6), v.widgets.hovered.bg_fill);
    }
    swatch(p, Rect::from_min_size(rect.min + Vec2::new(6.0, 8.0), Vec2::splat(18.0)), c, v.widgets.noninteractive.bg_stroke.color);
    let x = rect.min.x + 32.0;
    let set = ed.is_set(name);
    let mark = format!("{}{}", if set { "●" } else { "○" }, if ed.modified(name) { " *" } else { "" });
    p.text(Pos2::new(x, rect.min.y + 3.0), egui::Align2::LEFT_TOP, name, FontId::monospace(12.0), v.text_color());
    p.text(Pos2::new(x, rect.min.y + 18.0), egui::Align2::LEFT_TOP, lang.t(tok.de, tok.en), FontId::proportional(11.0), v.weak_text_color());
    p.text(Pos2::new(rect.max.x - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, mark, FontId::proportional(12.0), if set { style::accent() } else { v.weak_text_color() });
    let hover = match ed.theme.ui.get(name) {
        Some(s) => s.to_text(),
        None if tokens::is_ui(name) => format!("{} {}", lang.t("abgeleitet:", "derived:"), tokens::describe_expr(tokens::default_expr(name, ed.theme.dark))),
        None => to_hex(c),
    };
    let resp = resp.on_hover_text(hover);
    if resp.clicked() {
        if sel {
            ed.selected = None;
        } else {
            ed.select(name);
            ed.highlight = vec![name];
        }
    }
    if sel {
        egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 4, top: 4, bottom: 8 }).show(ui, |ui| color_editor(ed, ui, name, lang, now));
    }
}

/// Bearbeiten der gewählten Farbe: Farbwähler (HSV), Hex, Verweis, Zurücksetzen, Kontrast.
fn color_editor(ed: &mut Editor, ui: &mut Ui, name: &'static str, lang: Lang, now: f64) {
    let is_ui = tokens::is_ui(name);
    let mut c = tokens::resolve(&ed.theme, name);
    let alpha = if is_ui { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
    if egui::color_picker::color_picker_color32(ui, &mut c, alpha) {
        ed.edit(now, true, |t| {
            theme::set_color(t, name, c);
        });
    }
    // Hex-Feld (folgt dem Farbwähler, übernimmt beim Verlassen/Enter).
    let cur = tokens::resolve(&ed.theme, name);
    if ed.hex_for != Some((name, cur)) {
        ed.hex = to_hex(cur);
        ed.hex_for = Some((name, cur));
        ed.hex_err = false;
    }
    ui.horizontal(|ui| {
        ui.label("Hex");
        let r = ui.add(egui::TextEdit::singleline(&mut ed.hex).desired_width(100.0).font(egui::TextStyle::Monospace)).on_hover_text("#RRGGBB / #RRGGBBAA");
        if r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            match parse_hex(&ed.hex) {
                Some(h) if is_ui || h.a() == 255 => {
                    let h2 = h;
                    ed.edit(now, false, |t| {
                        theme::set_color(t, name, h2);
                    });
                    ed.hex_err = false;
                }
                _ => ed.hex_err = true,
            }
        }
        if ed.hex_err {
            ui.label(RichText::new(if is_ui { lang.t("erwartet #RRGGBB oder #RRGGBBAA", "expected #RRGGBB or #RRGGBBAA") } else { lang.t("erwartet #RRGGBB", "expected #RRGGBB") }).small().color(style::ui().error));
        }
    });
    if is_ui {
        // Verweis auf eine andere Farbe mit Deckkraft („@text/20“): Änderungen der Zielfarbe schlagen durch.
        if ed.link_for != Some(name) {
            if let Some(Spec::Link { to, alpha }) = ed.theme.ui.get(name) {
                ed.link_to = tokens::token(to.as_str()).map(|t| t.name).unwrap_or("text");
                ed.link_alpha = alpha.unwrap_or(100);
            } else {
                ed.link_to = if name == "text" { "fg" } else { "text" };
                ed.link_alpha = 100;
            }
            ed.link_for = Some(name);
        }
        ui.horizontal(|ui| {
            ui.label(lang.t("Verweis", "Link"));
            egui::ComboBox::from_id_salt(("pd_theme_link", name)).selected_text(format!("@{}", ed.link_to)).height(320.0).show_ui(ui, |ui| {
                for t in all_tokens().filter(|t| t.name != name) {
                    ui.selectable_value(&mut ed.link_to, t.name, format!("@{}", t.name)).on_hover_text(lang.t(t.de, t.en));
                }
            });
            ui.add(egui::DragValue::new(&mut ed.link_alpha).range(0..=100).suffix(" %")).on_hover_text(lang.t("Deckkraft", "Opacity"));
            if ui.button(lang.t("Verknüpfen", "Link")).on_hover_text(lang.t("Farbe folgt der gewählten Farbe (z. B. Rahmen = Text zu 20 %)", "The colour follows the chosen colour (e.g. border = text at 20 %)")).clicked() {
                let spec = Spec::Link { to: ed.link_to.to_string(), alpha: (ed.link_alpha < 100).then_some(ed.link_alpha) };
                let mut test = ed.theme.clone();
                test.ui.insert(name.to_string(), spec.clone());
                match tokens::find_cycle(&test) {
                    Some(cy) => ed.error = Some(format!("{} {}", lang.t("Verweise im Kreis:", "Links form a loop:"), cy.join(" → "))),
                    None => ed.edit(now, false, |t| {
                        t.ui.insert(name.to_string(), spec);
                    }),
                }
            }
        });
    }
    ui.horizontal(|ui| {
        let (label, tip) = if is_ui {
            (lang.t("Auf abgeleitet zurücksetzen", "Reset to derived"), format!("{} {}", lang.t("Ableitung:", "Derivation:"), tokens::describe_expr(tokens::default_expr(name, ed.theme.dark))))
        } else if name == "cursor" || name == "selection" {
            (lang.t("Auf abgeleitet zurücksetzen", "Reset to derived"), (if name == "cursor" { "@fg" } else { "mix(bg, fg, 20%)" }).to_string())
        } else {
            (lang.t("Auf gespeicherten Wert", "Reset to saved value"), to_hex(tokens::resolve(&ed.saved, name)))
        };
        // Auch ein aufbewahrter Wert einer neueren Version (Verweis auf eine unbekannte Farbe) lässt sich zurücksetzen.
        let can = if is_ui { ed.theme.ui.contains_key(name) || ed.theme.extra.ui.contains_key(name) } else { true };
        if ui.add_enabled(can, egui::Button::new(label)).on_hover_text(tip).clicked() {
            let saved = tokens::resolve(&ed.saved, name);
            ed.edit_checked(now, lang, |t| {
                if is_ui {
                    t.ui.remove(name);
                    t.extra.ui.remove(name);
                } else if name == "cursor" {
                    t.cursor = t.fg;
                } else if name == "selection" {
                    t.selection = style::mix(t.bg.to_opaque(), t.fg, 0.2);
                } else {
                    theme::set_color(t, name, saved);
                }
            });
        }
        if let Some(Spec::Link { .. }) = ed.theme.ui.get(name) {
            ui.label(RichText::new(ed.theme.ui[name].to_text()).monospace().small().color(style::dim(ui)));
        }
    });
    // Kontrast gegen die üblichen Hintergründe (WCAG).
    let me = solid(&ed.theme, name);
    let pairs: &[&'static str] = if matches!(name, "bg" | "fg") || name.starts_with("ansi") || name == "cursor" || name == "selection" {
        &["bg", "selection"]
    } else {
        &["window_bg", "sidebar_bg", "input_bg"]
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(lang.t("Kontrast:", "Contrast:")).small().color(style::dim(ui)));
        for bg in pairs.iter().filter(|b| **b != name) {
            let k = contrast(me, solid(&ed.theme, bg));
            let (mark, col) = if k >= checks::TEXT_MIN {
                ("✓", style::ui().success)
            } else if k >= checks::LARGE_MIN {
                ("◐", style::ui().warning)
            } else {
                ("⚠", style::ui().error)
            };
            ui.label(RichText::new(format!("{bg} {k:.1}:1 {mark}")).small().color(col)).on_hover_text(lang.t(
                "WCAG: ab 4,5:1 für Text, ab 3:1 für große Schrift (ab 18 px), Symbole und Rahmen. Nur ein Hinweis.",
                "WCAG: 4.5:1 for text, 3:1 for large text (from 18 px), icons and borders. Only a hint.",
            ));
        }
    });
}

fn preview(ed: &mut Editor, ui: &mut Ui, lang: Lang, base_style: &egui::Style, h: f32) {
    let u = ed.theme.ui_colors();
    let st = ed.preview_style(base_style);
    let clip = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), h));
    ui.painter().rect_filled(clip, CornerRadius::same(6), u.window_bg);
    ed.marks.clear();
    let Editor { theme, sample, marks, .. } = ed;
    // Beide Richtungen: Eine nur senkrecht rollende Fläche wüchse mit zu breitem Inhalt mit – samt Fenster.
    let out = egui::ScrollArea::both().id_salt("pd_theme_preview").max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show(ui, |ui| {
        style::with_preview(u, || {
            ui.scope(|ui| {
                ui.set_style(st.clone());
                egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    theme_preview::page(ui, theme, &u, lang, sample, marks);
                });
            });
        });
    });
    note_overflow(ui, &out);
    // Klick in die Vorschau: Farbe des Elements in der Liste wählen (die Bedienelemente bewirken nichts).
    let click = ui.input(|i| if i.pointer.primary_clicked() { i.pointer.interact_pos() } else { None });
    if let Some(pos) = click.filter(|p| clip.contains(*p)) {
        if ui.ctx().layer_id_at(pos) == Some(ui.layer_id()) {
            if let Some(tok) = theme_preview::hit(&ed.marks, pos) {
                ed.select(tok);
                ed.highlight = vec![tok];
                ed.scroll_to = Some(tok);
            }
        }
    }
    if !ed.highlight.is_empty() {
        theme_preview::highlight(ui, &ed.marks, &ed.highlight, clip);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
}

fn checks_panel(ed: &mut Editor, ui: &mut Ui, lang: Lang, now: f64) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(lang.t("Design-Prüfung", "Design check")).strong());
        let n = ed.hints.len();
        let (txt, col) = if n == 0 { (lang.t("keine Hinweise", "no hints").to_string(), style::ui().success) } else { (n.to_string(), style::ui().warning) };
        egui::Frame::new().fill(col.gamma_multiply(0.2)).corner_radius(CornerRadius::same(9)).inner_margin(egui::Margin::symmetric(7, 1)).show(ui, |ui| {
            ui.label(RichText::new(txt).small().color(col));
        });
        ui.label(RichText::new(lang.t("Hinweise blockieren nie das Speichern.", "Hints never block saving.")).small().color(style::dim(ui)));
    });
    let mut apply = None;
    let mut pick = None;
    // Beide Richtungen: Sollte doch etwas zu breit sein, erscheint ein Rollbalken, statt dass das Fenster wächst.
    let out = egui::ScrollArea::both().id_salt("pd_theme_checks").auto_shrink([false, false]).show(ui, |ui| {
        for (i, h) in ed.hints.iter().enumerate() {
            // „Übernehmen“ und Farbfeld zuerst rechts setzen; der Text bekommt nur die übrige Breite und bricht um.
            // (Vorher nahm der abgeschnittene Text die ganze Breite, und die Knöpfe ragten rechts aus dem Fenster.)
            let (left, ()) = egui::Sides::new().shrink_left().show(
                ui,
                |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(h.rule.label(lang)).small().color(style::dim(ui)));
                        let msg = ui.add(egui::Label::new(h.msg.get(lang)).sense(Sense::click()).wrap());
                        if msg.clicked() {
                            pick = Some(i);
                        }
                    })
                    .response
                },
                |ui| {
                    if let Some(f) = &h.fix {
                        if ui.button(lang.t("Übernehmen", "Apply")).on_hover_text(format!("{} ({})", checks::fix_text(f), lang.t("rückgängig mit Strg+Z", "undo with Ctrl+Z"))).clicked() {
                            apply = Some(i);
                        }
                        let (sr, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        swatch(ui.painter(), sr, f.color, ui.visuals().widgets.noninteractive.bg_stroke.color);
                    }
                },
            );
            if left.interact(Sense::click()).clicked() {
                pick = Some(i);
            }
        }
    });
    note_overflow(ui, &out);
    if let Some(i) = pick {
        let toks = ed.hints[i].tokens.clone();
        if let Some(&first) = toks.first() {
            ed.select(first);
            ed.scroll_to = Some(first);
        }
        ed.highlight = toks;
    }
    if let Some(i) = apply {
        if let Some(f) = ed.hints[i].fix.clone() {
            ed.edit(now, false, |t| {
                theme::set_color(t, f.token, f.color);
            });
            ed.select(f.token);
            ed.highlight = vec![f.token];
        }
    }
}

/// „Aus wenigen Farben erzeugen“: Hintergrund, Akzent, hell/dunkel → komplettes Theme (Rückgängig möglich).
fn generator_ui(ed: &mut Editor, ctx: &egui::Context, lang: Lang, now: f64) {
    let Some(mut seed) = ed.generator else { return };
    let (mut go, mut cancel) = (false, false);
    style::modal(Id::new("pd_theme_gen")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(RichText::new(lang.t("Aus wenigen Farben erzeugen", "Generate from a few colours")).heading());
        ui.label(RichText::new(lang.t(
            "Erzeugt alle Terminal- und Oberflächenfarben aus Hintergrund, Akzent und hell/dunkel. Danach kannst du jede Farbe einzeln anpassen.",
            "Creates all terminal and interface colours from a background, an accent and light/dark. Afterwards you can fine-tune every colour.",
        )).small().color(style::dim(ui)));
        ui.add_space(6.0);
        egui::Grid::new("pd_theme_gen_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(lang.t("Hintergrund", "Background"));
            ui.horizontal(|ui| {
                if egui::color_picker::color_edit_button_srgba(ui, &mut seed.bg, egui::color_picker::Alpha::Opaque).changed() {
                    seed.dark = theme::luma(seed.bg) < 128.0;
                }
                ui.label(RichText::new(to_hex(seed.bg)).monospace());
            });
            ui.end_row();
            ui.label(lang.t("Akzent", "Accent"));
            ui.horizontal(|ui| {
                egui::color_picker::color_edit_button_srgba(ui, &mut seed.accent, egui::color_picker::Alpha::Opaque);
                ui.label(RichText::new(to_hex(seed.accent)).monospace());
            });
            ui.end_row();
            ui.label(lang.t("Art", "Kind"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut seed.dark, true, lang.t("Dunkel", "Dark"));
                ui.radio_value(&mut seed.dark, false, lang.t("Hell", "Light"));
            });
            ui.end_row();
        });
        // Vorschau der erzeugten Palette.
        let g = generate::generate("", seed);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), g.bg);
        let w = (rect.width() - 16.0) / 8.0;
        for i in 0..16 {
            let r = Rect::from_min_size(rect.min + Vec2::new(8.0 + (i % 8) as f32 * w, 6.0 + (i / 8) as f32 * 16.0), Vec2::new(w - 4.0, 13.0));
            ui.painter().rect_filled(r, CornerRadius::same(2), g.ansi[i]);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new(lang.t("Erzeugen", "Generate")).strong()).fill(style::brand().gamma_multiply(0.7))).clicked() {
                go = true;
            }
            if ui.button(lang.t("Abbrechen", "Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    });
    if go {
        // Bei Ablehnung (Kreis) steht die Meldung in der Kopfzeile des Editors.
        ed.edit_checked(now, lang, |t| {
            let mut g = generate::generate(&t.name, seed);
            g.source = t.source.clone();
            // Unbekannte Inhalte der Datei (neuere Versionen) bleiben erhalten.
            g.extra = std::mem::take(&mut t.extra);
            *t = g;
        });
        ed.generator = None;
    } else if cancel {
        ed.generator = None;
    } else {
        ed.generator = Some(seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed() -> Editor {
        let mut t = crate::theme::all()[0].clone();
        t.builtin = false;
        t.source = Some(std::path::PathBuf::from("x.json"));
        Editor::new(t.clone(), t)
    }

    #[test]
    fn edits_are_undoable_and_drags_merge() {
        let mut e = ed();
        assert!(!e.dirty());
        e.edit(1.0, true, |t| {
            theme::set_color(t, "accent", Color32::RED);
        });
        e.edit(1.2, true, |t| {
            theme::set_color(t, "accent", Color32::GREEN);
        });
        assert_eq!(e.undo.len(), 1, "Ziehen = ein Schritt");
        assert!(e.dirty() && e.modified("accent") && e.modified("link") && !e.modified("text"));
        e.edit(5.0, false, |t| {
            t.ui.remove("accent");
        });
        assert_eq!(e.undo.len(), 2);
        e.undo();
        assert_eq!(e.theme.ui["accent"], Spec::Hex(Color32::GREEN));
        e.undo();
        assert!(!e.dirty());
        e.redo();
        assert!(e.dirty());
        // Keine Änderung → kein Schritt.
        let n = e.undo.len();
        e.edit(9.0, false, |_| {});
        assert_eq!(e.undo.len(), n);
    }

    #[test]
    fn edits_that_would_form_a_loop_are_refused() {
        let mut e = ed();
        // Dunkel: surface → window_bg → sidebar_bg → bg (kein Kreis). Hell wäre window_bg = @surface → Kreis.
        e.edit(1.0, false, |t| {
            t.ui.insert("surface".into(), Spec::Link { to: "window_bg".into(), alpha: None });
        });
        assert!(tokens::find_cycle(&e.theme).is_none());
        let steps = e.undo.len();
        assert!(!e.edit_checked(2.0, Lang::En, |t| t.dark = false));
        assert!(e.theme.dark && e.undo.len() == steps, "nichts übernommen");
        assert!(e.error.as_deref().is_some_and(|m| m.contains("loop")), "{:?}", e.error);
        // Zurücksetzen auf abgeleitet und danach hell: geht, und nimmt auch aufbewahrte Werte mit.
        e.theme.extra.ui.insert("border".into(), serde_json::json!("@future/20"));
        assert!(e.edit_checked(3.0, Lang::En, |t| {
            t.ui.remove("surface");
            t.extra.ui.remove("border");
        }));
        assert!(e.edit_checked(4.0, Lang::En, |t| t.dark = false));
        assert!(!e.theme.dark && e.theme.extra.ui.is_empty() && e.error.is_none());
    }

    #[test]
    fn a_refused_edit_message_goes_away_after_the_next_change_undo_or_save() {
        let mut e = ed();
        e.edit(1.0, false, |t| {
            t.ui.insert("surface".into(), Spec::Link { to: "window_bg".into(), alpha: None });
        });
        let refuse = |e: &mut Editor, now: f64| {
            assert!(!e.edit_checked(now, Lang::En, |t| t.dark = false));
            assert!(e.error.is_some());
        };
        refuse(&mut e, 2.0);
        e.edit(3.0, false, |t| {
            theme::set_color(t, "accent", Color32::RED);
        });
        assert!(e.error.is_none(), "nach einer erfolgreichen Änderung");
        refuse(&mut e, 4.0);
        e.undo();
        assert!(e.error.is_none(), "nach Rückgängig");
        refuse(&mut e, 5.0);
        e.redo();
        assert!(e.error.is_none(), "nach Wiederholen");
        refuse(&mut e, 6.0);
        e.mark_saved(std::path::PathBuf::from("x.json"));
        assert!(e.error.is_none() && !e.dirty(), "nach dem Speichern");
    }

    /// Das Fenster wächst nicht über seine Größe hinaus (früher schob die abgeschnittene Zeile der Design-Prüfung
    /// „Übernehmen“ rechts hinaus, das Fenster wuchs jedes Bild bis über den Bildschirmrand), und der Inhalt passt auch
    /// in ein schmales Hauptfenster.
    #[test]
    fn editor_fits_the_window_and_does_not_grow() {
        for (w, h) in [(1500.0, 960.0), (1100.0, 760.0)] {
            for lang in [Lang::En, Lang::De] {
                let ctx = egui::Context::default();
                let mut e = ed();
                // Dunkler Text auf dunklem Grund: Hinweise mit Vorschlag und langen Texten.
                theme::set_color(&mut e.theme, "text", Color32::from_gray(40));
                e.select("accent");
                e.refresh_hints(&[], 0.0);
                assert!(e.hints.iter().any(|h| h.fix.is_some()), "Hinweis mit „Übernehmen“");
                let mut seen = None;
                for frame in 0..8 {
                    let mut action = None;
                    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))), time: Some(frame as f64 * 0.05), ..Default::default() };
                    let _ = ctx.run(input, |ctx| {
                        let st = (*ctx.style()).clone();
                        ctx.data_mut(|d| d.insert_temp(Id::new("pd_theme_overflow"), 0.0_f32));
                        editor_window(ctx, "t".into()).show(ctx, |ui| {
                            body(&mut e, ui, lang, 0.0, &[], &st, &mut action);
                            seen = Some((ui.min_rect(), ui.max_rect()));
                        });
                    });
                    assert!(action.is_none());
                }
                let (used, avail) = seen.expect("gezeichnet");
                assert!(used.max.x <= avail.max.x + 0.5, "{w}/{lang:?}: Inhalt {used:?} breiter als das Fenster {avail:?}");
                // Liste, Vorschau und Design-Prüfung passen in ihre Breite (kein seitliches Rollen nötig).
                let over = ctx.data(|d| d.get_temp::<f32>(Id::new("pd_theme_overflow"))).unwrap_or(0.0);
                assert!(over <= 0.5, "{w}/{lang:?}: eine Rollfläche ist um {over} px zu breit");
                let win = ctx.memory(|m| m.area_rect(Id::new("pd_theme_editor"))).expect("Fenster");
                assert!(win.min.x >= -0.5 && win.max.x <= w + 0.5, "{w}/{lang:?}: Fenster {win:?} ragt aus dem Bildschirm");
                assert!(win.width() <= 1240.0 + 40.0, "{w}/{lang:?}: Fenster gewachsen: {win:?}");
            }
        }
    }

    /// egui merkt sich die Fenstergröße: Wird das Hauptfenster bei offenem Editor (oder zwischen zwei Öffnungen)
    /// verkleinert, schrumpft der Editor mit, statt rechts abgeschnitten zu werden.
    #[test]
    fn editor_shrinks_with_a_smaller_main_window() {
        let ctx = egui::Context::default();
        let mut e = ed();
        let mut frame = 0;
        let mut run = |w: f32, h: f32, n: usize| {
            for _ in 0..n {
                let mut action = None;
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))), time: Some(frame as f64 * 0.05), ..Default::default() };
                frame += 1;
                let _ = ctx.run(input, |ctx| {
                    let st = (*ctx.style()).clone();
                    editor_window(ctx, "t".into()).show(ctx, |ui| body(&mut e, ui, Lang::De, 0.0, &[], &st, &mut action));
                });
            }
            ctx.memory(|m| m.area_rect(Id::new("pd_theme_editor"))).expect("Fenster")
        };
        let big = run(1500.0, 960.0, 6);
        assert!(big.width() > 1100.0, "Ausgangslage: breiter als das spätere Hauptfenster ({big:?})");
        let small = run(1100.0, 760.0, 8);
        assert!(small.min.x >= -0.5 && small.max.x <= 1100.5, "Fenster {small:?} ragt aus 1100 px");
        assert!(small.min.y >= -0.5 && small.max.y <= 760.5, "Fenster {small:?} ragt aus 760 px");
    }

    #[test]
    fn design_checks_are_debounced_while_dragging() {
        let mut e = ed();
        assert!(!e.refresh_hints(&[], 10.0));
        let first = e.hints_for.clone();
        e.edit(10.01, true, |t| {
            theme::set_color(t, "text", Color32::from_gray(40));
        });
        assert!(e.refresh_hints(&[], 10.05), "noch nicht neu geprüft");
        assert_eq!(e.hints_for, first);
        assert!(!e.refresh_hints(&[], 10.2));
        assert_eq!(e.hints_for.as_ref().map(|h| &h.0), Some(&e.theme));
        assert!(!e.hints.is_empty(), "dunkler Text auf dunklem Grund");
        // Unverändert: keine weitere Prüfung nötig.
        assert!(!e.refresh_hints(&[], 10.21));
    }

    #[test]
    fn new_themes_count_as_unsaved_and_search_matches_both_languages() {
        let mut e = ed();
        e.theme.source = None;
        assert!(e.dirty());
        let tok = tokens::token("border").unwrap();
        assert!(matches(tok, "BORD", Lang::En) && matches(tok, "Rahmen", Lang::De) && matches(tok, "separators", Lang::En) && !matches(tok, "xyz", Lang::En));
        assert!(e.is_set("ansi3") && !e.is_set("border"));
    }

    #[test]
    fn editor_window_draws_without_panic() {
        let ctx = egui::Context::default();
        let mut e = ed();
        e.select("accent");
        e.highlight = vec!["accent"];
        for lang in [Lang::En, Lang::De] {
            let mut action = None;
            let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))), ..Default::default() }, |ctx| {
                let st = (*ctx.style()).clone();
                e.refresh_hints(&[], 0.0);
                egui::Window::new("t").default_size([1240.0, 800.0]).show(ctx, |ui| body(&mut e, ui, lang, 0.0, &[], &st, &mut action));
                e.generator = Some(Seed::from_theme(&e.theme));
                generator_ui(&mut e, ctx, lang, 0.0);
            });
            assert!(action.is_none());
            assert!(!e.marks.is_empty(), "Vorschau gezeichnet");
        }
    }
}
