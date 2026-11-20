//! Einstellungen → Agenten, Abschnitt „Systemprompt“: Vorlage ansehen und bearbeiten (`system-prompt.md` im Datenordner),
//! Sprache wählen, Vorschau für die aktuelle Stufe und den Modus, kopieren und als Datei speichern.
//!
//! Der Zustand des Editors liegt im egui-Speicher (nicht in `App`), die Logik in `crate::mcp::sysprompt`.

use super::App;
use crate::mcp::sysprompt::{self, Ctx, FILE_NAME, MAX_BYTES};
use crate::mcp::{HubInfo, McpLevel};
use crate::state::{self, Lang};
use crate::style::{self, S1, S2};
use eframe::egui::{self, Color32, RichText};
use std::path::Path;
use std::time::SystemTime;

#[allow(non_snake_case)]
fn WARN() -> Color32 {
    crate::style::ui().warning
}
#[allow(non_snake_case)]
fn ERR() -> Color32 {
    crate::style::ui().error
}

/// Editor-Zustand (im egui-Speicher).
#[derive(Clone, Default)]
pub(super) struct PromptEd {
    pub loaded: bool,
    /// Text im Editor.
    pub text: String,
    /// Stand der Datei bzw. der eingebauten Vorlage (zum Erkennen ungespeicherter Änderungen).
    pub baseline: String,
    /// `true` = eigene Datei vorhanden und genutzt.
    pub custom: bool,
    /// Warnung beim Laden (Datei zu groß, kein UTF-8 …), in der Sprache der Oberfläche.
    pub load_warning: Option<String>,
    /// Änderungszeit der Datei beim Laden (um Änderungen von außen zu bemerken).
    pub mtime: Option<SystemTime>,
    /// Letzte Meldung (Text, Fehler?).
    pub msg: Option<(String, bool)>,
    /// Die Datei wurde von außen geändert, während der Editor ungespeicherte Änderungen hat (Speichern überschreibt sie).
    pub changed_on_disk: bool,
}

fn mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir.join(FILE_NAME)).and_then(|m| m.modified()).ok()
}

impl PromptEd {
    pub fn dirty(&self) -> bool {
        self.text != self.baseline
    }

    /// Vorlage (neu) laden; ungespeicherte Änderungen gehen dabei verloren.
    pub fn load(&mut self, dir: &Path, prompt_lang: Lang, ui_lang: Lang) {
        let l = sysprompt::load(dir, prompt_lang);
        self.text = l.template.clone();
        self.baseline = l.template;
        self.custom = l.custom;
        self.load_warning = l.issue.map(|i| i.message(ui_lang));
        self.mtime = mtime(dir);
        self.loaded = true;
        self.changed_on_disk = false;
    }

    /// Einmal pro Frame: erstes Laden und Änderungen der Datei von außen übernehmen. Mit ungespeicherten Änderungen wird nur
    /// vermerkt, dass sich die Datei geändert hat (Warnung), damit nichts still verloren geht.
    pub fn sync(&mut self, dir: &Path, prompt_lang: Lang, ui_lang: Lang) {
        if !self.loaded {
            return self.load(dir, prompt_lang, ui_lang);
        }
        if mtime(dir) != self.mtime {
            if self.dirty() {
                self.changed_on_disk = true;
            } else {
                self.load(dir, prompt_lang, ui_lang);
            }
        }
    }

    /// Sprache des Prompts gewechselt: Ohne eigene Datei und ohne Änderungen zeigt der Editor die andere eingebaute Vorlage.
    pub fn lang_changed(&mut self, dir: &Path, prompt_lang: Lang, ui_lang: Lang) {
        if !self.custom && !self.dirty() {
            self.load(dir, prompt_lang, ui_lang);
        }
    }

    pub fn save(&mut self, dir: &Path, prompt_lang: Lang, ui_lang: Lang) {
        match sysprompt::save(dir, &self.text) {
            Ok(()) => {
                self.load(dir, prompt_lang, ui_lang);
                let m = if self.custom { ui_lang.t("Gespeichert in system-prompt.md.", "Saved to system-prompt.md.") } else { ui_lang.t("Leerer Text: eingebaute Vorlage wiederhergestellt.", "Empty text: built-in template restored.") };
                self.msg = Some((m.to_string(), false));
            }
            Err(e) => self.msg = Some((e.message(ui_lang), true)),
        }
    }

    pub fn reset(&mut self, dir: &Path, prompt_lang: Lang, ui_lang: Lang) {
        match sysprompt::reset(dir) {
            Ok(backup) => {
                self.load(dir, prompt_lang, ui_lang);
                let m = if backup {
                    ui_lang.t("Eingebaute Vorlage wiederhergestellt; deine Vorlage liegt als system-prompt.md.bak im Datenordner.", "Built-in template restored; your template was kept as system-prompt.md.bak in the data folder.")
                } else {
                    ui_lang.t("Eingebaute Vorlage wiederhergestellt.", "Built-in template restored.")
                };
                self.msg = Some((m.to_string(), false));
            }
            Err(e) => self.msg = Some((format!("{}: {e}", ui_lang.t("Zurücksetzen fehlgeschlagen", "Reset failed")), true)),
        }
    }
}

/// Was der Abschnitt von außen braucht.
pub(super) struct PromptEnv<'a> {
    /// Sprache der Oberfläche.
    pub lang: Lang,
    pub dir: &'a Path,
    pub level: McpLevel,
    pub expert: bool,
    /// Sprache des Prompts (Einstellung).
    pub prompt_lang: &'a mut Lang,
}

/// Aktion, die erst nach dem Zeichnen ausgeführt wird (Dateidialog, Zwischenablage).
enum Act {
    Copy(String),
    SaveAs(String),
}

fn ed_id() -> egui::Id {
    egui::Id::new("sysprompt_editor_state")
}

/// Der Abschnitt „Systemprompt“. Gibt `true` zurück, wenn sich die Spracheinstellung geändert hat (dann speichern).
pub(super) fn system_prompt_section(ui: &mut egui::Ui, env: &mut PromptEnv) -> bool {
    let lang = env.lang;
    let mut ed: PromptEd = ui.data_mut(|d| d.get_temp(ed_id())).unwrap_or_default();
    ed.sync(env.dir, *env.prompt_lang, lang);
    let mut changed = false;
    let mut act = None;

    ui.add_space(S2);
    ui.separator();
    ui.add_space(S1);
    ui.label(RichText::new(lang.t("Systemprompt", "System prompt")).strong());
    ui.label(
        RichText::new(lang.t(
            "Anleitung für Agenten, die Paneldeck nicht kennen – z. B. lokale Modelle oder andere Programme mit MCP. Paneldeck schickt sie automatisch beim Verbinden (`initialize`, Feld `instructions`); du kannst sie auch kopieren oder als Datei speichern und dort als Systemprompt einsetzen.",
            "Instructions for agents that do not know Paneldeck – e.g. local models or other programs using MCP. Paneldeck sends them automatically when an agent connects (`initialize`, field `instructions`); you can also copy them or save them as a file and use them there as the system prompt.",
        ))
        .small()
        .color(style::dim(ui)),
    );
    ui.add_space(S1);
    ui.horizontal(|ui| {
        ui.label(RichText::new(lang.t("Sprache des Prompts:", "Prompt language:")).small());
        let before = *env.prompt_lang;
        crate::afford::choice_value(ui, env.prompt_lang, Lang::En, "English", 0.0);
        crate::afford::choice_value(ui, env.prompt_lang, Lang::De, "Deutsch", 0.0);
        if *env.prompt_lang != before {
            changed = true;
            ed.lang_changed(env.dir, *env.prompt_lang, lang);
        }
    });
    ui.horizontal_wrapped(|ui| {
        let (src, col) = if ed.custom {
            (lang.t("Eigene Vorlage (system-prompt.md im Datenordner, für beide Sprachen)", "Custom template (system-prompt.md in the data folder, used for both languages)"), crate::style::ui().diff_rename)
        } else {
            (lang.t("Eingebaute Vorlage", "Built-in template"), style::dim(ui))
        };
        ui.label(RichText::new(src).small().strong().color(col));
        if ed.dirty() {
            ui.label(RichText::new(lang.t("· ungespeicherte Änderungen", "· unsaved changes")).small().color(WARN()));
        }
    });
    if let Some(w) = &ed.load_warning {
        ui.label(RichText::new(w).small().color(WARN()));
    }
    if ed.changed_on_disk {
        ui.label(
            RichText::new(lang.t(
                "system-prompt.md wurde außerhalb von Paneldeck geändert. „Speichern“ überschreibt diese Änderungen, „Änderungen verwerfen“ lädt die Datei.",
                "system-prompt.md was changed outside Paneldeck. “Save” overwrites those changes, “Discard changes” loads the file.",
            ))
            .small()
            .color(WARN()),
        );
    }
    egui::ScrollArea::vertical().max_height(260.0).id_salt("sysprompt_edit").show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(&mut ed.text).code_editor().desired_rows(12).desired_width(f32::INFINITY));
    });
    let too_large = ed.text.len() > MAX_BYTES;
    if too_large {
        let msg = match lang {
            Lang::De => format!("Zu groß: {} KB, höchstens {} KB – so lässt sich die Vorlage nicht speichern.", ed.text.len().div_ceil(1024), MAX_BYTES / 1024),
            Lang::En => format!("Too large: {} KB, at most {} KB – the template cannot be saved like this.", ed.text.len().div_ceil(1024), MAX_BYTES / 1024),
        };
        ui.label(RichText::new(msg).small().color(ERR()));
    }
    let unknown = sysprompt::unknown_placeholders(&ed.text);
    if !unknown.is_empty() {
        let list = unknown.iter().map(|n| format!("{{{{{n}}}}}")).collect::<Vec<_>>().join(", ");
        ui.label(RichText::new(format!("{} {list}", lang.t("Unbekannte Platzhalter (bleiben unverändert stehen):", "Unknown placeholders (left as they are):"))).small().color(WARN()));
    }
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(ed.dirty() && !too_large, egui::Button::new(lang.t("Speichern", "Save"))).on_hover_text(lang.t("Schreibt system-prompt.md in den Datenordner", "Writes system-prompt.md to the data folder")).clicked() {
            ed.save(env.dir, *env.prompt_lang, lang);
        }
        if ui.add_enabled(ed.dirty(), egui::Button::new(lang.t("Änderungen verwerfen", "Discard changes"))).clicked() {
            ed.load(env.dir, *env.prompt_lang, lang);
            ed.msg = None;
        }
        if ui
            .add_enabled(ed.custom || ed.dirty(), egui::Button::new(lang.t("Auf Standard zurücksetzen", "Reset to default")))
            .on_hover_text(lang.t("Benennt system-prompt.md in system-prompt.md.bak um (ersetzt eine ältere Sicherung); danach gilt wieder die eingebaute Vorlage", "Renames system-prompt.md to system-prompt.md.bak (replacing an older backup); the built-in template applies again"))
            .clicked()
        {
            ed.reset(env.dir, *env.prompt_lang, lang);
        }
    });
    ui.add_space(S1);
    ui.label(RichText::new(lang.t("Platzhalter", "Placeholders")).small().strong());
    for (ph, de, en) in [
        ("{{tools}}", "Werkzeugliste – nur, was der Agent bei der aktuellen Stufe und im aktuellen Modus nutzen kann (bei „Aus“: alle, gekennzeichnet)", "Tool list – only what the agent can use at the current level and mode (at Off: all, marked)"),
        ("{{level}}", "Berechtigungsstufe (Aus / Nur lesen / Steuern)", "Permission level (Off / Read only / Control)"),
        ("{{mode}}", "Modus der Todo-Liste (einfach / Experte)", "To-do mode (simple / expert)"),
        ("{{version}}", "Paneldeck-Version", "Paneldeck version"),
        ("{{{{", "ein wörtliches {{", "a literal {{"),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(ph).small().monospace());
            ui.label(RichText::new(lang.t(de, en)).small().color(style::dim(ui)));
        });
    }
    ui.add_space(S2);
    let hub = HubInfo { level: env.level, expert: env.expert };
    let ctx = Ctx::new(*env.prompt_lang, Some(hub));
    let rendered = sysprompt::render(&ed.text, &ctx).text;
    ui.label(
        RichText::new(format!(
            "{}{} ({}: {}, {}: {})",
            lang.t("Vorschau", "Preview"),
            if ed.dirty() { lang.t(" – ungespeicherte Änderungen", " – unsaved changes") } else { "" },
            lang.t("Stufe", "level"),
            sysprompt::level_name(lang, Some(hub)),
            lang.t("Modus", "mode"),
            sysprompt::mode_name(lang, Some(hub))
        ))
        .small()
        .strong(),
    );
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical().max_height(220.0).id_salt("sysprompt_preview").show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&rendered).small().monospace());
        });
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button(lang.t("Kopieren", "Copy")).on_hover_text(lang.t("Kopiert den gerenderten Text", "Copies the rendered text")).clicked() {
            act = Some(Act::Copy(rendered.clone()));
        }
        if ui.button(lang.t("Als Datei speichern …", "Save as file…")).on_hover_text(lang.t("Speichert den gerenderten Text als .md-Datei", "Saves the rendered text as a .md file")).clicked() {
            act = Some(Act::SaveAs(rendered.clone()));
        }
    });
    match act {
        Some(Act::Copy(t)) => {
            ui.ctx().copy_text(t);
            ed.msg = Some((lang.t("In die Zwischenablage kopiert.", "Copied to the clipboard.").to_string(), false));
        }
        Some(Act::SaveAs(t)) => {
            if let Some(m) = save_as(env.dir, &t, lang) {
                ed.msg = Some(m);
            }
        }
        None => {}
    }
    if let Some((m, is_err)) = &ed.msg {
        ui.label(RichText::new(m).small().color(if *is_err { ERR() } else { style::dim(ui) }));
    }
    ui.data_mut(|d| d.insert_temp(ed_id(), ed));
    changed
}

/// Gerenderten Text speichern: über den Dialog, ohne Dialog in `exports/` im Datenordner.
/// Unter Windows ist `None` vom Dialog ein Abbruch (keine Datei, keine Meldung); nur ohne Dialog (andere Systeme) geht
/// die Datei nach `exports/`.
fn save_as(dir: &Path, text: &str, lang: Lang) -> Option<(String, bool)> {
    let name = "paneldeck-system-prompt.md";
    let path = match crate::platform::notify::save_md(name) {
        Some(p) => p,
        None if crate::platform::notify::DIALOGS => return None,
        None => {
            let d = dir.join("exports");
            let _ = std::fs::create_dir_all(&d);
            d.join(name)
        }
    };
    Some(match std::fs::write(&path, text) {
        Ok(()) => (format!("{} {}", lang.t("Gespeichert:", "Saved:"), path.display()), false),
        Err(e) => (format!("{}: {e}", lang.t("Speichern fehlgeschlagen", "Saving failed")), true),
    })
}

impl App {
    /// Abschnitt „Systemprompt“ in Einstellungen → Agenten.
    pub(super) fn sysprompt_settings_ui(&mut self, ui: &mut egui::Ui) {
        let dir = self.dir.clone();
        let mut env = PromptEnv { lang: self.settings.language, dir: &dir, level: self.settings.mcp_level, expert: self.plan.expert, prompt_lang: &mut self.settings.mcp_prompt_lang };
        if system_prompt_section(ui, &mut env) {
            // Sofort sichern: die Brücke liest die Sprache beim nächsten `initialize` aus der Datei.
            state::save_json(&self.dir.join("settings.json"), &self.settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, RawInput, Rect, Vec2};

    fn temp_dir() -> std::path::PathBuf {
        static C: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("paneldeck-test-sysprompt-ui-{}-{}", std::process::id(), C.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    struct H {
        ctx: egui::Context,
        dir: std::path::PathBuf,
        lang: Lang,
        prompt_lang: Lang,
        level: McpLevel,
        expert: bool,
        time: f64,
    }

    impl H {
        fn frame(&mut self) -> bool {
            self.time += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 1600.0))), time: Some(self.time), ..Default::default() };
            let mut changed = false;
            let H { ctx, dir, lang, prompt_lang, level, expert, .. } = self;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut env = PromptEnv { lang: *lang, dir, level: *level, expert: *expert, prompt_lang };
                    changed |= system_prompt_section(ui, &mut env);
                });
            });
            changed
        }
        fn ed(&self) -> PromptEd {
            self.ctx.data(|d| d.get_temp::<PromptEd>(ed_id())).unwrap()
        }
        fn set_ed(&self, ed: PromptEd) {
            self.ctx.data_mut(|d| d.insert_temp(ed_id(), ed));
        }
    }

    #[test]
    fn section_loads_edits_saves_and_resets() {
        let dir = temp_dir();
        let mut h = H { ctx: egui::Context::default(), dir: dir.clone(), lang: Lang::De, prompt_lang: Lang::En, level: McpLevel::Read, expert: false, time: 0.0 };
        assert!(!h.frame());
        let ed = h.ed();
        assert!(ed.loaded && !ed.custom && !ed.dirty() && ed.text == sysprompt::builtin(Lang::En));
        // Sprache des Prompts wechseln: ohne Datei zeigt der Editor die deutsche Vorlage
        h.prompt_lang = Lang::De;
        let mut ed = h.ed();
        ed.lang_changed(&dir, Lang::De, Lang::De);
        h.set_ed(ed);
        h.frame();
        assert_eq!(h.ed().text, sysprompt::builtin(Lang::De));
        // bearbeiten (mit unbekanntem Platzhalter) und speichern
        let mut ed = h.ed();
        ed.text = "Eigener Prompt {{version}} {{gibtsnicht}}".into();
        h.set_ed(ed);
        h.frame();
        assert!(h.ed().dirty());
        let mut ed = h.ed();
        ed.save(&dir, Lang::De, Lang::De);
        h.set_ed(ed);
        h.frame();
        let ed = h.ed();
        assert!(ed.custom && !ed.dirty() && ed.msg.as_ref().is_some_and(|m| !m.1));
        assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), "Eigener Prompt {{version}} {{gibtsnicht}}");
        // Änderung von außen wird übernommen
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join(FILE_NAME), "\u{feff}Von außen {{level}} und mehr Text").unwrap();
        h.frame();
        assert_eq!(h.ed().text, "Von außen {{level}} und mehr Text");
        // ungespeicherte Änderungen + Änderung von außen: nichts wird überschrieben, nur gewarnt
        let mut ed = h.ed();
        ed.text = "Meine Änderung".into();
        h.set_ed(ed);
        h.frame();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join(FILE_NAME), "Zweite Änderung von außen").unwrap();
        h.frame();
        let ed = h.ed();
        assert!(ed.changed_on_disk && ed.text == "Meine Änderung" && ed.dirty());
        let mut ed = h.ed();
        ed.load(&dir, Lang::De, Lang::De);
        assert!(!ed.changed_on_disk && ed.text == "Zweite Änderung von außen");
        h.set_ed(ed);
        h.frame();
        // zu groß: Speichern wird abgelehnt, die Datei bleibt
        let mut ed = h.ed();
        ed.text = "x".repeat(MAX_BYTES + 10);
        ed.save(&dir, Lang::De, Lang::En);
        assert!(ed.msg.as_ref().is_some_and(|m| m.1 && m.0.contains("Too large")));
        h.set_ed(ed);
        h.frame();
        assert!(dir.join(FILE_NAME).exists());
        // zurücksetzen
        let mut ed = h.ed();
        ed.reset(&dir, Lang::De, Lang::De);
        h.set_ed(ed);
        h.frame();
        let ed = h.ed();
        assert!(!ed.custom && !dir.join(FILE_NAME).exists() && ed.text == sysprompt::builtin(Lang::De));
        assert!(dir.join(sysprompt::BACKUP_NAME).exists() && ed.msg.as_ref().is_some_and(|m| m.0.contains("system-prompt.md.bak")));
        // Englische Oberfläche, Expertenmodus, Steuern: zeichnet ohne Panik
        h.lang = Lang::En;
        h.level = McpLevel::Control;
        h.expert = true;
        h.frame();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_shows_a_warning_and_the_builtin_template() {
        let dir = temp_dir();
        std::fs::write(dir.join(FILE_NAME), [0xff, 0xfe, 0x00]).unwrap();
        let mut h = H { ctx: egui::Context::default(), dir: dir.clone(), lang: Lang::En, prompt_lang: Lang::En, level: McpLevel::Off, expert: false, time: 0.0 };
        h.frame();
        let ed = h.ed();
        assert!(!ed.custom && ed.text == sysprompt::builtin(Lang::En));
        assert!(ed.load_warning.as_deref().is_some_and(|w| w.contains("UTF-8")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
