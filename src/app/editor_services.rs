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
    /// Zeilen der eigenen Sprach-Zuordnungen (Muster, Sprache), solange sie bearbeitet werden (#312).
    map_buf: Option<Vec<(String, String)>>,
    /// Fenster „In Dateien suchen“ offen und seine Eingaben.
    pub search_open: bool,
    pub search: super::editor_search_ui::SearchInput,
}

impl Default for EditorServices {
    fn default() -> Self {
        EditorServices { watch: DiskWatch::new(), find: find::Results::default(), open_queue: Vec::new(), edit_context: crate::integration::Status::Off, excludes_buf: None, map_buf: None, search_open: false, search: Default::default() }
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

/// Zeilen der eigenen Sprach-Zuordnungen bereinigt als Einstellung (leere/ungültige Muster fallen weg; bei doppelten
/// Mustern gilt die letzte Zeile).
fn clean_map(rows: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    use crate::editor_svc::settings::{clean_id, clean_pattern};
    rows.iter().filter_map(|(p, l)| Some((clean_pattern(p)?, clean_id(l)?))).collect()
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
        crate::editor_svc::settings::publish(&self.settings.editor, self.editor_probe().docs > 0);
        if self.wins.cur != 0 {
            return;
        }
        for path in std::mem::take(&mut self.editor_svc.open_queue) {
            // Linux-Pfad beim Programmstart (`--edit /home/u/x`): in der WSL-Standard-Distribution (#392).
            let path = match self.cli_path(&path.to_string_lossy(), None) {
                Ok(p) => PathBuf::from(p),
                Err(e) => {
                    self.notify_text(format!("{} {}: {}", state::t("Datei ließ sich nicht im Editor öffnen:", "Could not open the file in the editor:"), path.display(), e.message(self.settings.language)));
                    continue;
                }
            };
            // WSL-Dateien laden im Hintergrund (#392), lokale wie bisher.
            let r = if crate::wsl::is_wsl_unc(&path.to_string_lossy()) { self.editor_open_user(&path, None).map(|_| ()) } else { self.editor_bridge_open(ctx, &path) };
            if let Err(e) = r {
                self.notify_text(format!("{} {}: {e}", state::t("Datei ließ sich nicht im Editor öffnen:", "Could not open the file in the editor:"), path.display()));
            }
        }
        // Strg+Klick auf einen Pfad im Terminal (#392): Textdatei im Editor an der Zeile.
        for (path, line) in crate::links::take_editor_requests() {
            if let Err(e) = self.editor_open_user(Path::new(&path), line) {
                self.notify_text(format!("{} {path}: {e}", state::t("Datei ließ sich nicht im Editor öffnen:", "Could not open the file in the editor:")));
            }
        }
        self.ed.apply_pending_goto();
        if self.ed.pending_goto.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        // Speichern/Neu laden von WSL-Dateien im Hintergrund (#392).
        let lang = self.settings.language;
        let (done, busy) = self.ed.poll_disk_jobs(lang);
        for (id, title, r) in done {
            use super::editor_ui::DiskJobDone;
            match r {
                DiskJobDone::Saved => {
                    self.ed.set_hint(format!("{title}: {}", lang.t("gespeichert", "saved")));
                    // Aktionen beim Speichern (#383), Vergleiche und Marken (#385) erst jetzt – die Datei ist geschrieben.
                    self.sa_disk_job_done(id, true);
                    // Hot-Exit-Kopie erst jetzt löschen, anderer Reiter derselben Datei, Schließen fortsetzen.
                    self.editor_background_saved(id);
                }
                DiskJobDone::Reloaded(s) => {
                    self.ed.set_hint(format!("{title}: {}{}", lang.t("von der Platte neu geladen", "reloaded from disk"), if s.is_empty() { String::new() } else { format!(" ({s})") }));
                    self.ed.generation += 1;
                }
                DiskJobDone::Failed(e) => {
                    let extra = self.sa_disk_job_done(id, false);
                    let msg = match extra {
                        Some(x) => format!("{title}: {e} {x}"),
                        None => format!("{title}: {e}"),
                    };
                    // Hot-Exit-Kopie bleibt; eine wartende Schließen-Rückfrage zeigt den Fehler.
                    self.editor_background_save_failed(id, &msg);
                    self.notify_text(msg);
                }
                DiskJobDone::Conflict | DiskJobDone::Deleted | DiskJobDone::Unchanged => {}
            }
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(100));
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
        self.editor_search_window(ctx);
        if self.editor_svc.find.poll() || self.editor_svc.find.running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// `paneldeck --edit <datei>` aus einer zweiten Instanz (`hooks::IpcCmd::Edit`).
    pub(super) fn editor_open_from_ipc(&mut self, path: String) {
        self.editor_svc.open_queue.push(PathBuf::from(path));
    }

    /// MCP-Werkzeuge `editor_*` (Stufe prüft der Hub bzw. die Werkzeug-Regeln des Chats; die Schalter prüft `tools`).
    /// `hidden` = Ordner der für den Agenten gesperrten Projekte (#386, Review 3.2.0): solche Dokumente sieht er nicht,
    /// solche Dateien öffnet er nicht.
    pub(super) fn mcp_editor(&mut self, ctx: &egui::Context, tool: &str, args: &Value, hidden: Vec<String>) -> Result<Value, (String, String)> {
        let settings = self.settings.editor.clone();
        let mut host = BridgeHost { app: self, hidden };
        let r = tools::call(tool, &mut host, &settings, args);
        if r.is_ok() && matches!(tool, "editor_open" | "editor_edit" | "editor_save") {
            ctx.request_repaint();
        }
        r
    }

    /// MCP `format_file` (#381) starten: Regeln prüfen (`tools::plan_format`; die Stufe nach `op` prüft der Hub bzw. die
    /// Werkzeug-Regeln des Chats), dann den Lauf im Hilfsthread. Liefert Lauf, Dokument und Befehl.
    pub(super) fn mcp_format_start(&mut self, ctx: &egui::Context, args: &Value, hidden: &[String]) -> Result<(crate::editor_svc::format_job::Job, u64, crate::editor::format::Op), (String, String)> {
        let settings = self.settings.editor.clone();
        // Dokumente in Ordnern gesperrter Projekte gibt es für den Agenten nicht (Review 3.2.0).
        let docs: Vec<DocInfo> = self.editor_list().into_iter().map(doc_info).filter(|d| !doc_hidden(d, hidden)).collect();
        let (d, op) = tools::plan_format(&docs, &settings, args)?;
        let confirmed = args.get("remove_comments").and_then(Value::as_bool) == Some(true);
        let job = self.editor_start_agent_format(ctx, d.id, op, confirmed).map_err(|e| ("internal".to_string(), e))?;
        Ok((job, d.id, op))
    }

    /// Ergebnis von `format_file` anwenden (Herkunft Agent; ein inzwischen geändertes Dokument bleibt unverändert) und
    /// die Antwort bauen.
    pub(super) fn mcp_format_finish(&mut self, ctx: &egui::Context, doc: u64, op: crate::editor::format::Op, revision: u64, done: crate::editor_svc::format_job::Done) -> Result<Value, (String, String)> {
        use crate::editor_svc::format_job::Done;
        if let Done::Changed { edits, .. } = &done {
            match self.editor_apply_agent_format(doc, revision, edits.clone()) {
                Ok(()) => ctx.request_repaint(),
                Err(("conflict", _)) => {
                    return Err((
                        "conflict".into(),
                        state::t("Das Dokument wurde während des Formatierens geändert – nichts wurde übernommen. Bitte noch einmal aufrufen.", "The document changed while formatting – nothing was applied. Please call again.").into(),
                    ))
                }
                Err(("not_found", _)) => return Err(("not_found".into(), state::t("Das Dokument ist nicht mehr offen.", "The document is no longer open.").into())),
                Err(_) => return Err(("internal".into(), state::t("Die Änderung ließ sich nicht anwenden.", "The change could not be applied.").into())),
            }
        }
        let after = self.editor_list().into_iter().map(doc_info).find(|d| d.id == doc).unwrap_or_default();
        tools::format_reply(&after, op, &done)
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
            lang.t("Ohne Minimap, Umbruch und Syntaxfarben; die Suche in Dateien überspringt sie", "Without minimap, wrapping and syntax colours; search in files skips them"),
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
        row(ui, lang.t("Menüleiste", "Menu bar"), lang.t("Datei · Bearbeiten · Auswahl · Ansicht · Format über jeder Teilung", "File · Edit · Selection · View · Format above every split"), |ui| {
            style::toggle(ui, &mut self.settings.editor.menu_bar);
        });
        // Mehrere Cursor (#394): alles standardmäßig aus, Mausgesten mit eigenen Schaltern.
        let (next, all, soft) = (self.binding_text("ed_next_occurrence"), self.binding_text("ed_all_occurrences"), self.binding_text("ed_soft_undo"));
        let k = |s: &str| if s.is_empty() { lang.t("(kein Kürzel)", "(no shortcut)").to_string() } else { crate::keys::pretty(s, lang == crate::state::Lang::De) };
        let hint = if lang == crate::state::Lang::De {
            format!("Nächstes Vorkommen ({}), alle Vorkommen ({}), Cursor/Auswahl zurücknehmen ({}); Menü Auswahl und Palette", k(&next), k(&all), k(&soft))
        } else {
            format!("Next occurrence ({}), all occurrences ({}), undo cursor/selection change ({}); Selection menu and palette", k(&next), k(&all), k(&soft))
        };
        row(ui, lang.t("Mehrere Cursor", "Multiple cursors"), &hint, |ui| {
            style::toggle(ui, &mut self.settings.editor.multi_cursor);
        });
        row(ui, lang.t("Alt+Klick", "Alt+click"), lang.t("Setzt einen weiteren Cursor; auf einem Cursor nimmt es ihn weg", "Adds a cursor; on a cursor it removes it"), |ui| {
            style::toggle(ui, &mut self.settings.editor.multi_cursor_alt_click);
        });
        row(
            ui,
            lang.t("Mittlere Maustaste", "Middle mouse button"),
            lang.t("Ziehen: Spaltenauswahl, mit Strg dazu; Klick: Cursor setzen, mit Strg einen dazu", "Drag: column selection, with Ctrl added; click: place the cursor, with Ctrl add one"),
            |ui| {
                style::toggle(ui, &mut self.settings.editor.multi_cursor_middle);
            },
        );
        ui.add_space(S2);
        self.editor_format_settings_ui(ui);
        ui.add_space(S2);
        self.editor_languages_ui(ui);
        ui.add_space(S2);
        self.editor_smart_ui(ui);
        ui.add_space(S2);
        self.save_actions_settings_ui(ui);
        self.editor_compare_ui(ui);
        ui.add_space(S2);
        let top = ui.cursor().min.y;
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
        super::tutorial::spot_since(ui, Spot::EditorSearchSettings, top);
        ui.add_space(S2);
        self.editor_agents_ui(ui);
    }

    /// Abschnitt „Formatieren“ der Editor-Einstellungen (#381): nur ein Hinweis – es gibt keine Einstellung dafür, formatiert wird nie von allein.
    fn editor_format_settings_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        ui.label(RichText::new(lang.t("Formatieren (JSON, XML)", "Formatting (JSON, XML)")).strong());
        let (f, m, v) = (self.binding_text("ed_format"), self.binding_text("ed_minify"), self.binding_text("ed_validate"));
        let text = match lang {
            Lang::De => format!("Menü Format bzw. {f} (formatieren), {m} (minifizieren), {v} (prüfen); änderbar unter Tastenkürzel. Alles läuft lokal. Eingerückt wird nach .editorconfig bzw. der Einrückung der Datei."),
            Lang::En => format!("Format menu or {f} (format), {m} (minify), {v} (validate); changeable under Shortcuts. Everything runs locally. Indentation follows .editorconfig or the file's indentation."),
        };
        ui.label(RichText::new(text).small().color(style::dim(ui)));
        ui.label(RichText::new(lang.t("Formatiert wird nie von allein: nur auf Knopfdruck oder beim Speichern über eine Regel mit der Aktion Formatieren (Abschnitt „Aktionen beim Speichern“; Menü Format → Beim Speichern formatieren …).", "Nothing is ever formatted automatically: only on demand, or on save through a rule with the Format action (section “Actions on save”; Format menu → Format on save …).")).small().color(style::dim(ui)));
    }

    /// Abschnitt „Smart-Modus“ (#367); `initializationOptions` je Sprache nur über `settings.json` (`editor.smart.init_options`).: Vervollständigen, Grenzen, Server je Sprache (eingebaute abwählen, eigener Pfad),
    /// vertrauenswürdige Ordner (entziehen beendet die Server dort sofort).
    fn editor_smart_ui(&mut self, ui: &mut Ui) {
        use crate::lsp::{self, servers};
        let lang = self.settings.language;
        ui.label(RichText::new(lang.t("Smart-Modus (Sprachserver)", "Smart mode (language servers)")).strong());
        ui.label(RichText::new(lang.t(
            "Modus „Smart“ in der Statusleiste: Vervollständigen und Diagnosen von einem Sprachserver aus dem PATH oder einem eigenen Pfad – nur in vertrauenswürdigen Ordnern, nie aus dem Projektordner selbst.",
            "Mode “Smart” in the status bar: completion and diagnostics from a language server in PATH or an own path – only in trusted folders, never from the project folder itself.",
        )).small().color(style::dim(ui)));
        let s = &mut self.settings.editor.smart;
        row(ui, lang.t("Vervollständigen von selbst", "Complete automatically"), lang.t("Nach Auslösezeichen und 2 Wortzeichen; Strg+Leertaste geht immer", "After trigger characters and 2 word characters; Ctrl+Space always works"), |ui| {
            style::toggle(ui, &mut s.auto_complete);
        });
        row(ui, lang.t("Nachrichten mitschneiden", "Trace messages"), lang.t("Im Protokoll des Servers (nur im Speicher)", "In the server's log (in memory only)"), |ui| {
            style::toggle(ui, &mut s.trace);
        });
        let num = |ui: &mut Ui, v: &mut u32, (lo, hi): (u32, u32), suffix: &str| {
            ui.add(egui::DragValue::new(v).range(lo..=hi).suffix(suffix.to_string()));
        };
        row(ui, lang.t("Höchstens Server", "At most servers"), lang.t("Der am längsten unbenutzte endet", "The one unused for the longest ends"), |ui| num(ui, &mut s.max_servers, lsp::MAX_SERVERS_RANGE, ""));
        row(ui, lang.t("Zeitgrenze Vervollständigen", "Completion timeout"), "", |ui| num(ui, &mut s.completion_timeout_ms, lsp::COMPLETION_TIMEOUT_RANGE, " ms"));
        row(ui, lang.t("Diagnosen je Datei", "Diagnostics per file"), "", |ui| num(ui, &mut s.max_diagnostics, lsp::MAX_DIAGNOSTICS_RANGE, ""));
        row(ui, lang.t("Größte Nachricht", "Largest message"), "", |ui| num(ui, &mut s.max_message_mb, lsp::MAX_MESSAGE_MB_RANGE, " MB"));
        row(ui, lang.t("Smart-Modus bis", "Smart mode up to"), lang.t("Größere Dateien: Syntax-Modus", "Larger files: Syntax mode"), |ui| num(ui, &mut s.max_file_mb, lsp::MAX_FILE_MB_RANGE, " MB"));
        row(ui, lang.t("Leerlauf beendet Server nach", "Idle server ends after"), lang.t("Ohne offenes Dokument", "Without an open document"), |ui| num(ui, &mut s.idle_minutes, lsp::IDLE_MINUTES_RANGE, " min"));
        row(ui, lang.t("Tipp-Pause für Diagnosen", "Typing pause for diagnostics"), "", |ui| num(ui, &mut s.typing_pause_ms, lsp::TYPING_PAUSE_RANGE, " ms"));
        row(ui, lang.t("Abstürze bis „aus“", "Crashes until “off”"), lang.t("So viele Abstürze im Zeitraum: Smart für die Sprache bis zum Neustart aus", "This many crashes in the period: smart mode off for the language until restart"), |ui| {
            num(ui, &mut s.crash_max, lsp::CRASH_MAX_RANGE, "");
            ui.label(lang.t("in", "in"));
            num(ui, &mut s.crash_window_minutes, lsp::CRASH_WINDOW_RANGE, " min");
        });
        egui::CollapsingHeader::new(lang.t("Server je Sprache", "Servers per language")).id_salt("editor_smart_servers").show(ui, |ui| {
            ui.label(RichText::new(lang.t("Häkchen = eingebauter Server darf genutzt werden (der erste gefundene gewinnt); ein eigener Pfad (absolut, ohne Argumente) geht vor.", "Checked = the built-in server may be used (the first one found wins); an own path (absolute, without arguments) comes first.")).small().color(style::dim(ui)));
            egui::Grid::new("editor_smart_grid").num_columns(3).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
                for l in crate::editor::syntax::languages::LANGUAGES.iter().filter(|l| !servers::known(l.id).is_empty() || servers::OWN_PATH_ONLY.contains(&l.id)) {
                    ui.label(l.name);
                    ui.horizontal(|ui| {
                        for k in servers::known(l.id) {
                            let dis = s.disabled.entry(l.id.to_string()).or_default();
                            let mut on = !dis.iter().any(|d| d == k.id);
                            if ui.checkbox(&mut on, k.id).changed() {
                                if on {
                                    dis.retain(|d| d != k.id);
                                } else {
                                    dis.push(k.id.to_string());
                                }
                            }
                        }
                        if servers::known(l.id).is_empty() {
                            ui.label(RichText::new(lang.t("nur eigener Pfad", "own path only")).small().color(style::dim(ui)));
                        }
                    });
                    ui.horizontal(|ui| {
                        let mut txt = s.own_paths.get(l.id).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut txt).desired_width(200.0).hint_text(lang.t("eigener Pfad …", "own path …"))).changed() {
                            let p = std::path::PathBuf::from(txt.trim());
                            if txt.trim().is_empty() {
                                s.own_paths.remove(l.id);
                            } else if p.is_absolute() {
                                s.own_paths.insert(l.id.to_string(), p);
                            }
                        }
                        // Argumente durch Leerzeichen getrennt (ein Argument mit Leerzeichen: in "…").
                        let mut args = s.own_args.get(l.id).map(|a| a.iter().map(|x| if x.contains(' ') { format!("\"{x}\"") } else { x.clone() }).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut args).desired_width(140.0).hint_text(lang.t("Argumente", "arguments"))).changed() {
                            let v = split_args(&args);
                            if v.is_empty() {
                                s.own_args.remove(l.id);
                            } else {
                                s.own_args.insert(l.id.to_string(), v);
                            }
                        }
                    });
                    ui.end_row();
                }
            });
            s.disabled.retain(|_, v| !v.is_empty());
        });
        let dir = self.dir.clone();
        let folders: Vec<std::path::PathBuf> = self.ed.smart.trust(&dir).folders().to_vec();
        egui::CollapsingHeader::new(format!("{} ({})", lang.t("Vertrauenswürdige Ordner", "Trusted folders"), folders.len())).id_salt("editor_smart_trust").show(ui, |ui| {
            if folders.is_empty() {
                ui.label(RichText::new(lang.t("Noch keiner – die Hinweisleiste über einer Datei im Smart-Modus fragt.", "None yet – the bar above a file in smart mode asks.")).small().color(style::dim(ui)));
            }
            let mut revoke = None;
            for f in &folders {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(f.display().to_string()).small());
                    if ui.small_button(lang.t("Entziehen", "Revoke")).on_hover_text(lang.t("Server dort enden sofort", "Servers there stop at once")).clicked() {
                        revoke = Some(f.clone());
                    }
                });
            }
            if let Some(f) = revoke
                && let Err(e) = self.ed.smart_revoke(&f, &dir)
            {
                self.ed.set_hint(e);
            }
        });
    }

    /// Abschnitt „Sprachen“ der Editor-Einstellungen (#312): Standard-Modus, Modus je Sprache, eigene Zuordnungen.
    fn editor_languages_ui(&mut self, ui: &mut Ui) {
        use crate::editor::syntax::{self, Mode};
        let lang = self.settings.language;
        let mode_name = |m: Mode| match m {
            Mode::Text => lang.t("Text", "Text"),
            Mode::Syntax => lang.t("Syntax (Farben)", "Syntax (colours)"),
            Mode::Smart => lang.t("Smart (Sprachserver)", "Smart (language server)"),
        };
        ui.label(RichText::new(lang.t("Sprachen", "Languages")).strong());
        ui.label(
            RichText::new(lang.t(
                "Text = schlichter Editor, Syntax = Sprache erkennen und einfärben (Farben aus dem Theme). Die Statusleiste zeigt Modus und Sprache; beides lässt sich dort je Datei ändern. Smart = zusätzlich ein Sprachserver (Vervollständigen, Diagnosen) in vertrauenswürdigen Ordnern.",
                "Text = plain editor, Syntax = detect the language and colour it (colours from the theme). The status bar shows mode and language; both can be changed there per file. Smart = a language server on top (completion, diagnostics) in trusted folders.",
            ))
            .small()
            .color(style::dim(ui)),
        );
        row(ui, lang.t("Standard-Modus", "Default mode"), lang.t("Für Sprachen ohne eigene Einstellung; Standard: Text", "For languages without their own setting; default: Text"), |ui| {
            let ed = &mut self.settings.editor;
            egui::ComboBox::from_id_salt("editor_default_mode").selected_text(mode_name(ed.default_mode)).show_ui(ui, |ui| {
                for m in Mode::ALL {
                    ui.selectable_value(&mut ed.default_mode, m, mode_name(m));
                }
            });
        });
        egui::CollapsingHeader::new(lang.t("Modus je Sprache", "Mode per language")).id_salt("editor_lang_modes").show(ui, |ui| {
            let ed = &mut self.settings.editor;
            let dflt = format!("{} ({})", lang.t("Standard", "Default"), mode_name(ed.default_mode));
            egui::Grid::new("editor_lang_mode_grid").num_columns(3).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
                for l in syntax::languages::LANGUAGES.iter().filter(|l| l.id != "plaintext") {
                    ui.label(l.name);
                    let cur = ed.language_modes.get(l.id).copied();
                    let mut sel = cur;
                    egui::ComboBox::from_id_salt(("editor_lang_mode", l.id)).width(150.0).selected_text(cur.map(|m| mode_name(m).to_string()).unwrap_or_else(|| dflt.clone())).show_ui(ui, |ui| {
                        ui.selectable_value(&mut sel, None, dflt.as_str());
                        for m in Mode::ALL {
                            ui.selectable_value(&mut sel, Some(m), mode_name(m));
                        }
                    });
                    if sel != cur {
                        match sel {
                            Some(m) => {
                                ed.language_modes.insert(l.id.to_string(), m);
                            }
                            None => {
                                ed.language_modes.remove(l.id);
                            }
                        }
                    }
                    let exts: Vec<String> = l.exts.iter().take(6).map(|e| format!(".{e}")).chain(l.names.iter().take(2).map(|n| n.to_string())).collect();
                    ui.label(RichText::new(exts.join(" ")).small().color(style::dim(ui)));
                    ui.end_row();
                }
            });
            let others: Vec<String> = ed.language_modes.keys().filter(|k| syntax::languages::language(k).is_none()).cloned().collect();
            for k in others {
                ui.horizontal(|ui| {
                    let name = syntax::language_name(&k).unwrap_or(&k).to_string();
                    ui.label(format!("{name}: {}", ed.language_modes.get(&k).map(|m| mode_name(*m)).unwrap_or("")));
                    if ui.small_button("✕").on_hover_text(lang.t("Einstellung entfernen", "Remove setting")).clicked() {
                        ed.language_modes.remove(&k);
                    }
                });
            }
        });
        egui::CollapsingHeader::new(lang.t("Eigene Zuordnungen", "Own mappings")).id_salt("editor_lang_map").show(ui, |ui| {
            ui.label(
                RichText::new(lang.t(
                    "Gehen vor der eingebauten Erkennung. Muster: eine Endung (*.tpl oder .tpl) oder ein Dateiname (Jenkinsfile), ohne Groß/Klein.",
                    "Take precedence over the built-in detection. Pattern: an extension (*.tpl or .tpl) or a file name (Jenkinsfile), case-insensitive.",
                ))
                .small()
                .color(style::dim(ui)),
            );
            let settings_rows: Vec<(String, String)> = self.settings.editor.language_map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let buf = self.editor_svc.map_buf.get_or_insert_with(|| settings_rows.clone());
            if clean_map(buf) != self.settings.editor.language_map {
                *buf = settings_rows;
            }
            let mut changed = false;
            let mut remove = None;
            for (i, (pat, id)) in buf.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    changed |= ui.add(egui::TextEdit::singleline(pat).desired_width(170.0).hint_text("*.tpl")).changed();
                    let shown = syntax::language_name(id).map(|n| super::editor_ui::lang_label(id, n, lang)).unwrap_or_else(|| id.clone());
                    egui::ComboBox::from_id_salt(("editor_lang_map_lang", i)).width(220.0).height(320.0).selected_text(shown).show_ui(ui, |ui| {
                        for l in syntax::all_languages() {
                            if ui.selectable_label(*id == l.id, super::editor_ui::lang_label(&l.id, &l.name, lang)).clicked() {
                                *id = l.id.clone();
                                changed = true;
                            }
                        }
                    });
                    if !syntax::languages::is_known(id) {
                        ui.label(RichText::new(lang.t("unbekannte Sprache", "unknown language")).small().color(style::ui().warning));
                    }
                    if ui.small_button("✕").on_hover_text(lang.t("Zuordnung entfernen", "Remove mapping")).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                buf.remove(i);
                changed = true;
            }
            if buf.len() < crate::editor_svc::settings::MAX_LANGUAGE_ENTRIES && ui.button(lang.t("+ Zuordnung", "+ Mapping")).clicked() {
                buf.push((String::new(), "plaintext".to_string()));
            }
            if changed {
                self.settings.editor.language_map = clean_map(buf);
            }
        });
    }

    /// Abschnitt „Vergleich und Änderungsmarken“ (#385): Marken am Rand (Standard aus) und ihre Grundlage, Vorgaben
    /// neuer Vergleichs-Reiter.
    fn editor_compare_ui(&mut self, ui: &mut Ui) {
        use crate::editor_svc::settings::{DiffLayout, MarkerBase, DIFF_CONTEXT_RANGE};
        let lang = self.settings.language;
        ui.label(RichText::new(lang.t("Vergleich und Änderungsmarken", "Comparison and change markers")).strong());
        let (next, prev) = (self.binding_text("ed_next_change"), self.binding_text("ed_prev_change"));
        let ed = &mut self.settings.editor;
        row(
            ui,
            lang.t("Änderungsmarken am Rand", "Change markers in the gutter"),
            lang.t("Hinzugefügte, geänderte und entfernte Zeilen; nicht in großen Dateien", "Added, modified and removed lines; not in large files"),
            |ui| {
                style::toggle(ui, &mut ed.change_markers);
            },
        );
        row(
            ui,
            lang.t("Grundlage der Marken", "Markers compare with"),
            lang.t("git: nicht gestagt und gestagt in eigenen Farben; außerhalb eines Repositorys die gespeicherte Datei", "git: unstaged and staged in their own colours; outside a repository the saved file"),
            |ui| {
                let label = |b: MarkerBase| match b {
                    MarkerBase::Git => lang.t("git (Index und HEAD)", "git (index and HEAD)"),
                    MarkerBase::Disk => lang.t("Gespeicherte Datei", "Saved file"),
                };
                ui.add_enabled_ui(ed.change_markers, |ui| {
                    egui::ComboBox::from_id_salt("editor_marker_base").selected_text(label(ed.marker_base)).show_ui(ui, |ui| {
                        for b in [MarkerBase::Git, MarkerBase::Disk] {
                            ui.selectable_value(&mut ed.marker_base, b, label(b));
                        }
                    });
                });
            },
        );
        row(ui, lang.t("Darstellung im Vergleich", "Comparison layout"), lang.t("Automatisch: nebeneinander, unter 700 px untereinander", "Automatic: side by side, unified below 700 px"), |ui| {
            let label = |l: DiffLayout| match l {
                DiffLayout::Auto => lang.t("Automatisch", "Automatic"),
                DiffLayout::Split => lang.t("Nebeneinander", "Side by side"),
                DiffLayout::Unified => lang.t("Untereinander", "Unified"),
            };
            egui::ComboBox::from_id_salt("editor_diff_layout").selected_text(label(ed.diff_layout)).show_ui(ui, |ui| {
                for l in [DiffLayout::Auto, DiffLayout::Split, DiffLayout::Unified] {
                    ui.selectable_value(&mut ed.diff_layout, l, label(l));
                }
            });
        });
        row(ui, lang.t("Kontextzeilen", "Context lines"), lang.t("Unveränderte Zeilen um jede Änderung, der Rest ist eingeklappt", "Unchanged lines around each change, the rest is collapsed"), |ui| {
            ui.add(egui::DragValue::new(&mut ed.diff_context).range(DIFF_CONTEXT_RANGE.0..=DIFF_CONTEXT_RANGE.1).speed(0.2));
        });
        row(ui, lang.t("Leerraum ignorieren", "Ignore whitespace"), lang.t("Vorgabe für neue Vergleiche (umschaltbar im Vergleich)", "Default for new comparisons (switchable in the comparison)"), |ui| {
            style::toggle(ui, &mut ed.diff_ignore_whitespace);
        });
        ui.label(
            RichText::new(if lang == Lang::De {
                format!("Nächste/vorige Änderung: {next} / {prev} (Einstellungen → Tastenkürzel). Vergleichen: Kontextmenü eines Reiters oder einer Datei im Ordnerbaum, Befehlspalette.")
            } else {
                format!("Next/previous change: {next} / {prev} (Settings → Shortcuts). Compare: context menu of a tab or of a file in the folder tree, command palette.")
            })
            .small()
            .color(style::dim(ui)),
        );
    }

    /// Abschnitt „Agenten“ der Editor-Einstellungen: die drei Schalter (Standard aus, nur bei Stufe Steuern wirksam).
    fn editor_agents_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        let top = ui.cursor().min.y;
        ui.label(RichText::new(lang.t("Agenten", "Agents")).strong());
        let control = self.settings.mcp_level == crate::mcp::McpLevel::Control;
        ui.label(
            RichText::new(lang.t(
                "Lesen geht ab der Stufe „Nur lesen“ (editor_list, editor_read, format_file mit validate). Die Schalter gelten nur bei der Stufe „Steuern“ – für externe Agenten unter Einstellungen → Agenten, für Chats die Stufe des Modells. Änderungen eines Agenten bleiben ungespeichert, sind als „vom Agenten“ markiert und lassen sich rückgängig machen.",
                "Reading works from level “Read only” (editor_list, editor_read, format_file with validate). The switches only apply at level “Control” – for external agents under Settings → Agents, for chats the level of the model. Changes by an agent stay unsaved, are marked “by agent” and can be undone.",
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
        row(ui, lang.t("Agenten dürfen formatieren", "Agents may format"), lang.t("format_file: offene JSON/XML-Dateien; Prüfen geht ab „Nur lesen“ ohne Schalter", "format_file: open JSON/XML files; validating works from “Read only” without a switch"), |ui| {
            style::toggle(ui, &mut ed.agent_format);
        });
        // #394: ab Stufe „Nur lesen“ (editor_read), Standard aus.
        row(ui, lang.t("Agenten sehen die Auswahlen", "Agents see the selections"), lang.t("editor_read nennt deine Cursor und Auswahlen (ab „Nur lesen“)", "editor_read reports your cursors and selections (from “Read only”)"), |ui| {
            style::toggle(ui, &mut ed.agent_selections);
        });
        super::tutorial::spot_since(ui, Spot::EditorAgentSwitches, top);
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
    // Diese Funktionen verbinden die Dienste mit den Dokumenten des Editors (`app::editor_ui`, Schnittstelle dort).

    /// Datei in einem neuen Editor-Tab öffnen (offene Datei: nach vorn holen). Für `--edit` und das Explorer-Menü –
    /// eine Aktion des Nutzers, nie „von einem Agenten“.
    fn editor_bridge_open(&mut self, _ctx: &egui::Context, path: &Path) -> Result<(), String> {
        self.editor_open(path, true, false).map(|_| ())
    }

    /// Pfade aller offenen Dokumente mit Datei (für das Beobachten der Platte). Vorher den Vergleichsstand der Dateien
    /// neu setzen, die der Editor selbst gelesen oder gespeichert hat (eigenes Schreiben ist keine fremde Änderung).
    fn editor_bridge_paths(&mut self) -> Vec<PathBuf> {
        for p in self.editor_take_touched() {
            self.editor_svc.watch.baseline(&p);
        }
        self.editor_open_paths()
    }

    /// Eine Datei hat sich auf der Platte geändert, ist verschwunden oder wieder da: der Editor entscheidet (still neu
    /// laden mit Hinweis, Infoleiste *Meine behalten* / *Von der Platte laden* / *Unterschied anzeigen*).
    fn editor_bridge_disk_event(&mut self, _ctx: &egui::Context, ev: DiskEvent) {
        self.editor_disk_changed(ev.path());
    }

    /// Einstellungen für den Editor (ersetzt die Vorgaben von `editor_prefs`).
    pub(super) fn editor_prefs_from_settings(&self) -> super::editor_ui::EditorPrefs {
        let e = &self.settings.editor;
        super::editor_ui::EditorPrefs {
            font_size: self.settings.font_size,
            large_file_bytes: e.large_file_bytes(),
            autosave: e.autosave.then(|| Duration::from_millis(e.autosave_delay_ms as u64)),
            syntax: super::editor_ui::SyntaxPrefs::from_settings(e),
            menu_bar: e.menu_bar,
            keys: super::editor_ui::MENU_KEYS.iter().map(|id| (id.to_string(), self.binding_raw(id))).collect(),
            markers: e.change_markers,
            marker_base: e.marker_base,
            diff_context: e.diff_context as usize,
            diff_layout: e.diff_layout,
            diff_ignore_ws: e.diff_ignore_whitespace,
            multi: super::editor_ui::MultiPrefs { on: e.multi_cursor, alt_click: e.multi_cursor_alt_click, middle: e.multi_cursor_middle },
            smart_auto: e.smart.auto_complete,
        }
    }

    /// Fenster „In Dateien suchen“ (Strg+Umschalt+F im Editor-Reiter): Panel aus `editor_search_ui`, Treffer öffnen die
    /// Datei an der Fundstelle; F4 / Umschalt+F4 springen zum nächsten / vorigen Treffer.
    fn editor_search_window(&mut self, ctx: &egui::Context) {
        if self.mode != titlebar::Mode::Editor {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::F)) {
            self.editor_svc.search_open = true;
            self.editor_svc.search.focus = true;
        }
        let step = ctx.input_mut(|i| {
            if i.consume_key(Modifiers::SHIFT, Key::F4) {
                Some(false)
            } else if i.consume_key(Modifiers::NONE, Key::F4) {
                Some(true)
            } else {
                None
            }
        });
        let mut target = step.and_then(|f| self.editor_svc.find.step(f));
        if self.editor_svc.search_open {
            let lang = self.settings.language;
            let root = self.editor_root();
            let mut open = true;
            let settings = self.settings.editor.clone();
            let svc = &mut self.editor_svc;
            let mut action = None;
            let esc = !egui::Popup::is_any_open(ctx) && ctx.input(|i| i.key_pressed(Key::Escape));
            let win = find_files_window(ctx, lang).open(&mut open).show(ctx, |ui| {
                action = super::editor_search_ui::show(ui, lang, &mut svc.search, &mut svc.find, root.as_deref(), &settings);
            });
            if let Some(w) = win {
                // Esc schließt das Fenster, wenn es gemeint ist: im Suchfeld (siehe `PanelAction::Close`) oder mit dem
                // Zeiger über dem Fenster – nicht beim Tippen im Editor (dort hebt Esc die Auswahl auf).
                if esc && w.response.contains_pointer() {
                    open = false;
                }
                let _ = w.response.spot(Spot::EditorSearchFiles);
            }
            match action {
                Some(super::editor_search_ui::PanelAction::Open { path, line, col }) => target = Some((path, line, col)),
                Some(super::editor_search_ui::PanelAction::Close) => open = false,
                None => {}
            }
            if !open {
                self.editor_svc.search_open = false;
                self.editor_svc.find.cancel();
            }
        }
        if let Some((path, line, col)) = target {
            // Treffer: Zeile/Spalte ab 1, der Editor zählt ab 0.
            if let Err(e) = self.editor_open_at(&path, line.saturating_sub(1), col.saturating_sub(1)) {
                self.notify_text(format!("{}: {e}", path.display()));
            }
        }
    }
}

/// Fenster „In Dateien suchen“: nie größer als das Hauptfenster (Ränder, Suchfeld und Schalter bleiben sichtbar).
pub(super) fn find_files_window<'a>(ctx: &egui::Context, lang: Lang) -> egui::Window<'a> {
    style::fit_to_screen(ctx, style::movable(ctx, Id::new("editor_find_files"), egui::Window::new(lang.t("In Dateien suchen", "Search in files"))), [520.0, 420.0], [340.0, 200.0]).collapsible(false)
}

/// Gespeicherter Stand einer Datei für `editor_diff` (#385): `disk`, `head` oder `index` (lokales git, Zeitgrenze
/// [`crate::editor_svc::git_base::GIT_TIMEOUT`]). Blockiert – im Hilfsthread aufrufen.
fn read_base(path: &Path, enc: Option<crate::editor::TextEncoding>, base: &str, max: u64) -> Result<Option<String>, String> {
    use crate::editor_svc::git_base::{self, Blob, Rev};
    match base {
        "disk" => match std::fs::metadata(path) {
            Ok(m) if m.len() > max => Err(state::t("Die Datei auf der Platte ist zu groß.", "The file on disk is too large.").to_string()),
            Ok(_) => std::fs::read(path).map(|b| Some(crate::editor::encoding::decode(&b, enc).text)).map_err(|e| e.to_string()),
            Err(_) => Ok(None),
        },
        "head" | "index" => {
            let repo = git_base::locate(path).ok_or_else(|| state::t("Die Datei liegt in keinem git-Repository (oder git fehlt).", "The file is not in a git repository (or git is missing).").to_string())?;
            let rev = if base == "head" { Rev::Head } else { Rev::Index };
            match git_base::blobs(&repo, &[rev], max)?.into_iter().next() {
                Some(Blob::Bytes(b)) => Ok(Some(git_base::decode_like(&b, enc).text)),
                Some(Blob::TooLarge(_)) => Err(state::t("Der Stand in git ist zu groß.", "The state in git is too large.").to_string()),
                _ => Ok(None),
            }
        }
        _ => Err("disk, head, index".into()),
    }
}

impl App {
    /// `editor_diff` (#385) ohne die Oberfläche anzuhalten: Argumente und Texte jetzt, Lesen des Stands (git, Platte) und
    /// Vergleichen im Hilfsthread. Das Ergebnis kommt über den Kanal (der Hub wartet in `McpState::pending`).
    pub(super) fn mcp_editor_diff(&mut self, ctx: &egui::Context, args: &Value, hidden: Vec<String>) -> Result<std::sync::mpsc::Receiver<Result<Value, (String, String)>>, (String, String)> {
        let plan = {
            let mut host = BridgeHost { app: self, hidden };
            tools::diff_plan(&mut host, args)?
        };
        let path = plan.doc.path.clone();
        let enc = crate::editor::TextEncoding::from_id(&plan.doc.encoding);
        let max = self.settings.editor.large_file_bytes();
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let base = match (&plan.base, &path) {
                (tools::DiffBase::Read(a), Some(p)) => read_base(p, enc, a, max),
                (tools::DiffBase::Read(_), None) => Err(state::t("Ein unbenannter Tab hat keinen gespeicherten Stand.", "An untitled tab has no saved state.").to_string()),
                (tools::DiffBase::Text(_), _) => Ok(None),
            };
            let _ = tx.send(tools::diff_finish(plan, base));
            c.request_repaint();
        });
        Ok(rx)
    }
}

/// Die App als `EditorHost` für die MCP-Werkzeuge (nur im Hauptdurchlauf).
struct BridgeHost<'a> {
    app: &'a mut App,
    /// Ordner der für den Agenten gesperrten Projekte (#386, Review 3.2.0).
    hidden: Vec<String>,
}

/// Liegt das Dokument im Ordner eines gesperrten Projekts (`hidden`)? Unbenannte Tabs nie.
fn doc_hidden(d: &DocInfo, hidden: &[String]) -> bool {
    d.path.as_deref().is_some_and(|p| super::mcp::access_gate::path_hidden(p, hidden))
}

fn doc_info(d: super::editor_ui::DocInfo) -> DocInfo {
    use crate::editor_svc::tools::DiskState;
    DocInfo {
        id: d.id,
        path: d.path,
        title: d.title,
        dirty: d.dirty,
        agent_edited: d.agent_edited,
        opened_by_agent: d.opened_by_agent,
        lines: d.lines,
        bytes: d.bytes as u64,
        encoding: d.encoding,
        eol: d.eol,
        large: d.large,
        read_only: d.read_only,
        disk: if d.disk_deleted {
            DiskState::Deleted
        } else if d.disk_changed {
            DiskState::Changed
        } else {
            DiskState::InSync
        },
        active: d.active,
    }
}

impl EditorHost for BridgeHost<'_> {
    fn docs(&self) -> Vec<DocInfo> {
        self.app.editor_list().into_iter().map(doc_info).filter(|d| !doc_hidden(d, &self.hidden)).collect()
    }

    fn text(&mut self, doc: u64) -> Option<String> {
        self.app.editor_text(doc)
    }

    fn open_file(&mut self, path: &Path, show: bool) -> Result<u64, String> {
        self.app.editor_open(path, show, true)
    }

    fn open_untitled(&mut self, text: &str, show: bool) -> Result<u64, String> {
        Ok(self.app.editor_new(text, show, true))
    }

    fn apply_agent_edits(&mut self, doc: u64, edits: &[TextEdit]) -> Result<(), String> {
        self.app.editor_apply_agent_edit_bytes(doc, edits.iter().map(|e| (e.range.clone(), e.text.clone())).collect())
    }

    fn show(&mut self, doc: u64) {
        self.app.editor_focus(doc);
    }

    fn selections(&mut self, doc: u64) -> Option<Vec<((usize, usize), (usize, usize))>> {
        self.app.editor_selections(doc)
    }

    fn base_text(&mut self, doc: u64, base: &str) -> Result<Option<String>, String> {
        let info = self.app.editor_list().into_iter().find(|d| d.id == doc).ok_or_else(|| state::t("Das Dokument ist nicht mehr offen.", "The document is no longer open.").to_string())?;
        let path = info.path.ok_or_else(|| state::t("Ein unbenannter Tab hat keinen gespeicherten Stand.", "An untitled tab has no saved state.").to_string())?;
        read_base(&path, crate::editor::TextEncoding::from_id(&info.encoding), base, self.app.settings.editor.large_file_bytes())
    }

    fn save(&mut self, doc: u64) -> Result<(), String> {
        self.app.save_actions.last_report = None;
        let r = self.app.editor_save_doc(doc);
        // Eigenes Speichern: neuer Vergleichsstand, sonst meldete die Überwachung es als fremde Änderung.
        if r.is_ok()
            && let Some(p) = self.app.editor_list().into_iter().find(|d| d.id == doc).and_then(|d| d.path)
        {
            self.app.editor_svc.watch.baseline(&p);
        }
        r
    }

    fn save_actions_report(&mut self) -> Option<Value> {
        self.app.save_actions.last_report.take()
    }

    /// Datenordner (und der Ordner der laufenden Instanz bei Profilen) – nie für Agenten (#383).
    fn protected(&self, path: &Path) -> bool {
        crate::save_actions::is_inside(path, &self.app.dir) || crate::save_actions::is_inside(&crate::save_actions::copy::normalize(path), &self.app.dir)
    }

    fn hidden(&self, path: &Path) -> bool {
        super::mcp::access_gate::path_hidden(path, &self.hidden)
    }
}

/// Argumente aus einer Zeile (Leerzeichen trennen, `"…"` hält zusammen) – für den eigenen Server (#367).
fn split_args(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            ' ' | '\t' if !quoted => {
                if any || !cur.is_empty() {
                    v.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        v.push(cur);
    }
    v
}

#[cfg(test)]
mod smart_settings_tests {
    #[test]
    fn own_server_arguments_split_on_spaces_and_keep_quotes_together() {
        assert_eq!(super::split_args(r#"-NoLogo -File "C:\a b\x.ps1"  --stdio"#), vec!["-NoLogo", "-File", r"C:\a b\x.ps1", "--stdio"]);
        assert!(super::split_args("  ").is_empty());
        assert_eq!(super::split_args("\"\""), vec![String::new()]);
    }
}
