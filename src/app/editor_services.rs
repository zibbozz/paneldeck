//! Dienste des Editor-Reiters in der App (#54): *Einstellungen → Editor*, der Schalter für den Explorer-Eintrag unter
//! *Einstellungen → Fenster*, die MCP-Werkzeuge `editor_*` (Ausführung im Hauptdurchlauf), das Beobachten offener
//! Dateien auf der Platte und die Warteschlange für `--edit`.
//!
//! Die eigentliche Logik steht ohne egui in `crate::editor_svc`. Die Verbindung zum Editor (Dokumente, Tabs) läuft
//! über die Schnittstelle `editor_svc::tools::EditorHost` und die kleinen Brücken-Funktionen am Ende dieser Datei
//! (Abschnitt „Brücke zum Editor“), die die Oberfläche des Editors bedient.

use super::*;
use crate::editor_svc::settings::EditorSettings;
use crate::editor_svc::tools::{self, DocInfo, EditorHost, TextEdit};
use crate::editor_svc::watch::{DiskEvent, DiskWatch};
use crate::editor_svc::find;
use serde_json::Value;
use std::path::Path;

/// Index des Einstellungs-Tabs „Editor“ (am Ende angehängt, damit die übrigen Indizes gelten).
pub(super) const SETTINGS_TAB: usize = 13;

/// Zustand der Editor-Dienste in der App.
pub(super) struct EditorServices {
    /// Änderungen offener Dateien auf der Platte.
    pub watch: DiskWatch,
    /// Suche in Dateien (Modell des Ergebnis-Panels).
    pub find: find::Results,
    /// Dateien, die im Editor geöffnet werden sollen (`--edit` beim Start und von einer zweiten Instanz).
    pub open_queue: Vec<PathBuf>,
    /// Explorer-Eintrag „In Paneldeck-Editor öffnen“ (Stand der Registry, unter *Einstellungen → Fenster*).
    pub edit_context: crate::integration::Status,
    /// Text des Feldes „Ausschlussmuster“, solange der Tab offen ist (gespeichert wird bereinigt).
    excludes_buf: Option<String>,
}

impl Default for EditorServices {
    fn default() -> Self {
        EditorServices { watch: DiskWatch::new(), find: find::Results::default(), open_queue: Vec::new(), edit_context: crate::integration::Status::Off, excludes_buf: None }
    }
}

impl EditorServices {
    /// Beim Start: `--edit <datei>` vormerken.
    pub fn from_cli() -> EditorServices {
        let mut s = EditorServices::default();
        if let Some(f) = &crate::cli::get().edit {
            s.open_queue.push(PathBuf::from(f));
        }
        s
    }
}

/// Zeile wie in den übrigen Einstellungs-Tabs: Beschriftung (+ Erklärung) links, Steuerelement rechts.
fn row(ui: &mut Ui, label: &str, hint: &str, add: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(Vec2::new(w, 42.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let x0 = ui.cursor().min.x;
        ui.allocate_ui_with_layout(Vec2::new(240.0, 42.0), egui::Layout::top_down(egui::Align::LEFT).with_main_align(egui::Align::Center), |ui| {
            ui.label(label);
            if !hint.is_empty() {
                ui.label(RichText::new(hint).small().color(style::dim(ui)));
            }
        });
        let used = ui.cursor().min.x - x0;
        ui.add_space((250.0 - used).max(0.0));
        add(ui);
    });
    ui.add_space(4.0);
}

/// Schriftfamilie des Editors für egui (`FontFamily::Name("editor")`); immer vorhanden.
pub(super) const EDITOR_FAMILY: &str = "editor";

/// Familie „editor“ in die Schriften eintragen: leer oder gleich der Terminalschrift = die Liste der Terminalschrift;
/// sonst die gewählte Schrift (eingebaut oder installiert) vor den übrigen Festbreitenschriften.
pub(super) fn add_editor_family(fonts: &mut egui::FontDefinitions, choice: &str, terminal: &str) {
    let mut list = fonts.families.get(&egui::FontFamily::Monospace).cloned().unwrap_or_default();
    let key = match choice {
        "" => None,
        c if c == terminal => None,
        "Cascadia Mono" => fonts.font_data.keys().find(|k| k.to_lowercase().contains("cascadia")).cloned(),
        "Hack" => Some("Hack".to_string()).filter(|k| fonts.font_data.contains_key(k)),
        other => crate::sysfonts::load(other, false, false).filter(crate::sysfonts::is_monospace).map(|d| {
            fonts.font_data.insert("editor-regular".to_owned(), std::sync::Arc::new(egui::FontData { font: d.bytes.into(), index: d.index, tweak: Default::default() }));
            "editor-regular".to_string()
        }),
    };
    if let Some(k) = key {
        list.retain(|n| n != &k);
        list.insert(0, k);
    }
    fonts.families.insert(egui::FontFamily::Name(EDITOR_FAMILY.into()), list);
}

impl App {
    /// Je Bild (Hauptfenster): `--edit`-Warteschlange abarbeiten, offene Dateien auf Änderungen prüfen, Suche weiterführen.
    pub(super) fn poll_editor_services(&mut self, ctx: &egui::Context) {
        // Schalter für die Werkzeugliste der Chats (wie `mcp::LEVEL`).
        crate::editor_svc::settings::publish(&self.settings.editor);
        if self.wins.cur != 0 {
            return;
        }
        for path in std::mem::take(&mut self.editor_svc.open_queue) {
            if let Err(e) = self.editor_bridge_open(ctx, &path) {
                self.notify_text(format!("{} {}: {e}", state::t("Datei ließ sich nicht im Editor öffnen:", "Could not open the file in the editor:"), path.display()));
            }
        }
        let open = self.editor_bridge_paths();
        let c = ctx.clone();
        let events = self.editor_svc.watch.tick(Instant::now(), &open, move || c.request_repaint());
        for ev in events {
            self.editor_bridge_disk_event(ctx, ev);
        }
        if let Some(d) = self.editor_svc.watch.next_due(Instant::now(), !open.is_empty()) {
            ctx.request_repaint_after(d.max(Duration::from_millis(50)));
        }
        if self.editor_svc.find.poll() || self.editor_svc.find.running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// `paneldeck --edit <datei>` aus einer zweiten Instanz (`hooks::IpcCmd::Edit`).
    pub(super) fn editor_open_from_ipc(&mut self, path: String) {
        self.editor_svc.open_queue.push(PathBuf::from(path));
    }

    /// MCP-Werkzeuge `editor_*` (Stufe prüft der Hub bzw. die Werkzeug-Regeln des Chats; die Schalter prüft `tools`).
    pub(super) fn mcp_editor(&mut self, ctx: &egui::Context, tool: &str, args: &Value) -> Result<Value, (String, String)> {
        let settings = self.settings.editor.clone();
        let mut host = BridgeHost { app: self, ctx };
        let r = tools::call(tool, &mut host, &settings, args);
        if r.is_ok() && matches!(tool, "editor_open" | "editor_edit" | "editor_save") {
            ctx.request_repaint();
        }
        r
    }

    /// *Einstellungen → Editor*.
    pub(super) fn editor_settings_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        ui.label(RichText::new(lang.t("Editor", "Editor")).strong());
        row(ui, lang.t("Schrift", "Font"), lang.t("Standard: die Terminalschrift", "Default: the terminal font"), |ui| {
            let ed = &mut self.settings.editor;
            let term = self.settings.terminal_font.clone();
            let shown = if ed.font.is_empty() { format!("{} ({term})", lang.t("Terminalschrift", "Terminal font")) } else { ed.font.clone() };
            egui::ComboBox::from_id_salt("editor_font").selected_text(shown).height(360.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut ed.font, String::new(), format!("{} ({term})", lang.t("Terminalschrift", "Terminal font")));
                ui.separator();
                for (k, l) in [("Cascadia Mono", lang.t("Cascadia Mono (eingebaut)", "Cascadia Mono (built-in)")), ("Hack", lang.t("Hack (eingebaut)", "Hack (built-in)"))] {
                    ui.selectable_value(&mut ed.font, k.to_string(), l);
                }
                let installed = crate::sysfonts::monospace_families_cached();
                if !installed.is_empty() {
                    ui.separator();
                }
                for f in installed.iter().filter(|f| f.as_str() != "Cascadia Mono" && f.as_str() != "Hack") {
                    ui.selectable_value(&mut ed.font, f.clone(), f.as_str());
                }
            });
        });
        row(
            ui,
            lang.t("Große Dateien ab", "Large files from"),
            lang.t("Ohne Minimap, Umbruch und (später) Syntaxfarben; die Suche in Dateien überspringt sie", "Without minimap, wrapping and (later) syntax colours; search in files skips them"),
            |ui| {
                ui.add(egui::DragValue::new(&mut self.settings.editor.large_file_mb).range(crate::editor_svc::settings::LARGE_FILE_MB_RANGE.0..=crate::editor_svc::settings::LARGE_FILE_MB_RANGE.1).speed(1.0).suffix(" MB"));
            },
        );
        row(ui, lang.t("Automatisch speichern", "Autosave"), lang.t("Nur Dateien, die es schon gibt; unbenannte Tabs nie", "Only files that already exist; never untitled tabs"), |ui| {
            style::toggle(ui, &mut self.settings.editor.autosave);
        });
        let (lo, hi) = crate::editor_svc::settings::AUTOSAVE_MS_RANGE;
        row(ui, lang.t("Verzögerung", "Delay"), lang.t("Nach der letzten Eingabe", "After the last change"), |ui| {
            let mut secs = self.settings.editor.autosave_delay_ms as f64 / 1000.0;
            let r = ui.add_enabled(self.settings.editor.autosave, egui::DragValue::new(&mut secs).range(lo as f64 / 1000.0..=hi as f64 / 1000.0).speed(0.1).max_decimals(1).suffix(" s"));
            if r.changed() {
                self.settings.editor.autosave_delay_ms = ((secs * 1000.0).round() as u32).clamp(lo, hi);
            }
        });
        ui.add_space(S2);
        ui.label(RichText::new(lang.t("Suche in Dateien", "Search in files")).strong());
        row(
            ui,
            lang.t("Ignorierte einbeziehen", "Include ignored"),
            lang.t("Vorgabe des Schalters in der Suche: auch Dateien, die .gitignore ausschließt", "Default of the switch in the search: also files excluded by .gitignore"),
            |ui| {
                style::toggle(ui, &mut self.settings.editor.search_include_ignored);
            },
        );
        ui.label(lang.t("Ausschlussmuster", "Exclude patterns"));
        ui.label(
            RichText::new(lang.t(
                "Eines je Zeile, Syntax wie .gitignore (z. B. node_modules, *.min.js, dist/). Gilt immer, auch mit „Ignorierte einbeziehen“. Die Ordner .git, .hg und .svn werden nie durchsucht.",
                "One per line, syntax as in .gitignore (e.g. node_modules, *.min.js, dist/). Always applies, also with “Include ignored”. The folders .git, .hg and .svn are never searched.",
            ))
            .small()
            .color(style::dim(ui)),
        );
        let buf = self.editor_svc.excludes_buf.get_or_insert_with(|| self.settings.editor.excludes_text());
        let r = ui.add(egui::TextEdit::multiline(buf).desired_rows(4).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace).hint_text("node_modules\n*.min.js"));
        if r.changed() {
            self.settings.editor.set_excludes_from_text(buf);
        }
        if r.lost_focus() {
            self.editor_svc.excludes_buf = Some(self.settings.editor.excludes_text());
        }
        ui.add_space(S2);
        self.editor_agents_ui(ui);
    }

    /// Abschnitt „Agenten“ der Editor-Einstellungen: die drei Schalter (Standard aus, nur bei Stufe Steuern wirksam).
    fn editor_agents_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        ui.label(RichText::new(lang.t("Agenten", "Agents")).strong());
        let control = self.settings.mcp_level == crate::mcp::McpLevel::Control;
        ui.label(
            RichText::new(lang.t(
                "Lesen geht ab der Stufe „Nur lesen“ (editor_list, editor_read). Die Schalter gelten nur bei der Stufe „Steuern“ – für externe Agenten unter Einstellungen → Agenten, für Chats die Stufe des Modells. Änderungen eines Agenten bleiben ungespeichert, sind als „vom Agenten“ markiert und lassen sich rückgängig machen.",
                "Reading works from level “Read only” (editor_list, editor_read). The switches only apply at level “Control” – for external agents under Settings → Agents, for chats the level of the model. Changes by an agent stay unsaved, are marked “by agent” and can be undone.",
            ))
            .small()
            .color(style::dim(ui)),
        );
        if !control {
            ui.label(RichText::new(lang.t("Externe Agenten haben gerade nicht die Stufe „Steuern“ – die Schalter wirken für sie erst dann.", "External agents are not at level “Control” right now – the switches only take effect for them at that level.")).small().color(crate::style::ui().warning));
        }
        let ed: &mut EditorSettings = &mut self.settings.editor;
        row(ui, lang.t("Agenten dürfen Dateien öffnen", "Agents may open files"), lang.t("Neuer Tab; nur lokale Pfade, nie Netzwerk-/UNC-Pfade oder Geräte", "New tab; local paths only, never network/UNC paths or devices"), |ui| {
            style::toggle(ui, &mut ed.agent_open);
        });
        row(ui, lang.t("Agenten dürfen offene Dateien bearbeiten", "Agents may edit open files"), lang.t("Ungespeichert, als „vom Agenten“ markiert, rückgängig machbar", "Unsaved, marked “by agent”, can be undone"), |ui| {
            style::toggle(ui, &mut ed.agent_edit);
        });
        row(ui, lang.t("Agenten dürfen speichern", "Agents may save"), lang.t("Nur offene Dateien, die es schon gibt; unbenannte Tabs speichert nur du", "Only open files that already exist; only you save untitled tabs"), |ui| {
            style::toggle(ui, &mut ed.agent_save);
        });
    }

    /// Zeile „In Paneldeck-Editor öffnen“ unter *Einstellungen → Fenster* (nur Windows). Fehler gehen an den Aufrufer.
    pub(super) fn edit_context_row(&mut self, ui: &mut Ui, exe: &Path) -> Option<String> {
        if !cfg!(windows) {
            return None;
        }
        let lang = self.settings.language;
        let mut err = None;
        row(
            ui,
            lang.t("Explorer: „In Paneldeck-Editor öffnen“", "Explorer: “Open in Paneldeck editor”"),
            lang.t("Rechtsklick auf eine Datei öffnet sie in einem neuen Editor-Tab (alle Dateiarten)", "Right-click a file to open it in a new editor tab (all file types)"),
            |ui| {
                let mut v = self.editor_svc.edit_context.is_mine();
                if style::toggle(ui, &mut v).changed() {
                    // Der Menütext landet in der Registry: in der gespeicherten Sprache, nicht in der von `--lang`.
                    let shown = state::lang();
                    state::set_lang(state::persisted_lang(self.settings.language));
                    let r = crate::integration::set_edit_context(v, exe);
                    state::set_lang(shown);
                    match r {
                        Ok(()) => self.editor_svc.edit_context = if v { crate::integration::Status::Mine } else { crate::integration::Status::Off },
                        Err(e) => err = Some(e),
                    }
                }
                if let Some(cmd) = self.editor_svc.edit_context.other() {
                    let text = match lang {
                        Lang::De => format!("Der Eintrag zeigt auf ein anderes Paneldeck:\n{cmd}\n\nEinschalten übernimmt ihn für diese Instanz."),
                        Lang::En => format!("The entry points to another Paneldeck:\n{cmd}\n\nTurning it on takes it over for this instance."),
                    };
                    ui.label(RichText::new(lang.t("→ anderes Paneldeck", "→ another Paneldeck")).small().color(crate::style::ui().warning)).on_hover_text(text);
                }
            },
        );
        err
    }

    // ───────────────────────── Brücke zum Editor (Oberfläche des Editor-Reiters) ─────────────────────────
    //
    // Diese Funktionen verbinden die Dienste mit den Dokumenten des Editors. Solange der Editor-Reiter nicht da ist,
    // gibt es keine offenen Dokumente; Öffnen meldet einen Fehler.

    /// Datei in einem neuen Editor-Tab öffnen (offene Datei: nach vorn holen). Für `--edit` und das Explorer-Menü.
    fn editor_bridge_open(&mut self, _ctx: &egui::Context, path: &Path) -> Result<(), String> {
        let _ = path;
        Err(state::t("Der Editor-Reiter ist in diesem Build nicht verfügbar.", "The Editor tab is not available in this build.").to_string())
    }

    /// Pfade aller offenen Dokumente mit Datei (für das Beobachten der Platte).
    fn editor_bridge_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Eine Datei hat sich auf der Platte geändert (Infoleiste bzw. still neu laden).
    fn editor_bridge_disk_event(&mut self, _ctx: &egui::Context, _ev: DiskEvent) {}
}

/// Die App als `EditorHost` für die MCP-Werkzeuge (nur im Hauptdurchlauf).
struct BridgeHost<'a> {
    app: &'a mut App,
    ctx: &'a egui::Context,
}

impl EditorHost for BridgeHost<'_> {
    fn docs(&self) -> Vec<DocInfo> {
        let _ = &self.app;
        Vec::new()
    }

    fn text(&self, _doc: u64) -> Option<String> {
        None
    }

    fn open_file(&mut self, path: &Path, show: bool) -> Result<u64, String> {
        let _ = (path, show, self.ctx);
        Err(state::t("Der Editor-Reiter ist in diesem Build nicht verfügbar.", "The Editor tab is not available in this build.").to_string())
    }

    fn open_untitled(&mut self, _name: Option<&str>, _text: &str, _show: bool) -> Result<u64, String> {
        Err(state::t("Der Editor-Reiter ist in diesem Build nicht verfügbar.", "The Editor tab is not available in this build.").to_string())
    }

    fn apply_agent_edits(&mut self, _doc: u64, _edits: &[TextEdit]) -> Result<(), String> {
        Err("no editor".into())
    }

    fn save(&mut self, _doc: u64) -> Result<(), String> {
        Err("no editor".into())
    }
}
