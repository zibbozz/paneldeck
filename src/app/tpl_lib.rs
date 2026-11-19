//! Vorlagen-Bibliothek in der Seitenleiste (#244): Terminal- und Gruppen-Vorlagen aus `templates.json` anzeigen,
//! suchen, starten (Doppelklick, Ziehen in die Sitzungsliste/einen Ordner/einen Bereich, Kontextmenü, Befehlspalette,
//! Ordner-Kontextmenü), anlegen (aus einer Sitzung oder neu), bearbeiten, duplizieren und löschen.
//!
//! Die Daten stehen in `crate::templates`; hier liegen nur Oberfläche und Start. Wiederholte Starts einer
//! Terminal-Vorlage zählen den Namen hoch (`API`, `API #2` …, `templates::numbered_name`).

use super::dragdrop::drop_zone;
use super::session_settings::ConvChoice;
use super::*;

/// Wohin eine Vorlage startet.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TplAt {
    /// Mit den Einstellungen der Vorlage (Ordner im Baum der Vorlage).
    Default,
    /// In diesen Ordner des Sitzungsbaums (Gruppen-Vorlagen: als Unterordner davon).
    Folder(String),
    /// In einen Bereich des Layouts: (Bereich, Zone 0 ersetzen, 1 links, 2 rechts, 3 oben, 4 unten). Nur Terminal-Vorlagen.
    Pane(u64, u8),
}

/// Zustand der Bibliothek je Fenster (`multiwin::WinView`).
#[derive(Default)]
pub(super) struct TplView {
    /// Suchtext im Abschnitt der Seitenleiste.
    pub(super) search: String,
    /// Offener Editor (eine Vorlage bearbeiten oder neu anlegen).
    pub(super) form: Option<TplForm>,
    /// Gerade gezogene Vorlage (Index in `App::templates`).
    pub(super) dragging: Option<usize>,
    /// Sitzungsliste der Seitenleiste in diesem Bild (Ablageziel in der Listenansicht).
    pub(super) list_rect: Option<Rect>,
    /// Offene Rückfrage vor dem Start einer Vorlage mit erweiterten Claude-Optionen.
    pub(super) confirm: Option<TplConfirm>,
}

/// Rückfrage vor dem Start einer Vorlage, deren Claude-Optionen erweitern, worauf Claude zugreifen darf oder wie es
/// läuft (zusätzliche Ordner, erlaubte Werkzeuge, MCP-Konfiguration, `--safe-mode`, ergänzter System-Prompt):
/// `templates.json` kann von jemand anderem stammen.
#[derive(Clone, Debug)]
pub(super) struct TplConfirm {
    /// Index und Name der Vorlage beim Fragen (der Name findet sie wieder, falls sich die Liste geändert hat).
    pub(super) index: usize,
    pub(super) name: String,
    pub(super) at: TplAt,
    /// (Sitzungsname, Art, Wert)
    pub(super) rows: Vec<(String, templates::Risk, String)>,
    /// Gruppen-Vorlage (zeigt die Sitzung je Zeile).
    pub(super) group: bool,
    focus: bool,
}

impl TplConfirm {
    pub(super) fn new(index: usize, t: &Template, at: TplAt) -> Self {
        TplConfirm { index, name: t.name.clone(), at, rows: t.risky(), group: !t.is_terminal(), focus: true }
    }

    /// Index der gefragten Vorlage in `list` (über den Namen, falls sich die Liste inzwischen geändert hat).
    pub(super) fn live_index(&self, list: &[Template]) -> Option<usize> {
        if list.get(self.index).is_some_and(|t| t.name == self.name) {
            return Some(self.index);
        }
        list.iter().position(|t| t.name == self.name)
    }
}

/// Ergebnis eines Bilds der Rückfrage.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(super) enum ConfirmOut {
    Open,
    /// Mit allen Optionen starten.
    Start,
    /// Ohne die erfragten Optionen starten.
    StartWithout,
    Cancel,
}

/// Beschriftung einer erfragten Option.
pub(super) fn risk_label(k: templates::Risk, lang: Lang) -> &'static str {
    use templates::Risk::*;
    match k {
        AddDir => lang.t("Zusätzlicher Ordner (--add-dir)", "Extra folder (--add-dir)"),
        PluginDir => lang.t("Plugin-Ordner (--plugin-dir)", "Plugin folder (--plugin-dir)"),
        SystemPrompt => lang.t("Eigener System-Prompt (--system-prompt)", "System prompt (--system-prompt)"),
        AllowedTools => lang.t("Erlaubte Werkzeuge (--allowed-tools)", "Allowed tools (--allowed-tools)"),
        McpConfig => lang.t("MCP-Konfiguration (--mcp-config)", "MCP config (--mcp-config)"),
        SafeMode => lang.t("Hooks und MCP-Server aus (--safe-mode)", "Hooks and MCP servers off (--safe-mode)"),
        AppendSystemPrompt => lang.t("System-Prompt ergänzen (--append-system-prompt)", "Append system prompt (--append-system-prompt)"),
    }
}

/// Die Rückfrage als Fenster (rein, ohne App – headless testbar). Beim Öffnen bekommt „Abbrechen“ den Fokus (nicht das
/// Terminal dahinter, #255); Esc bricht ab.
pub(super) fn tpl_confirm_ui(ctx: &egui::Context, c: &mut TplConfirm, lang: Lang) -> ConfirmOut {
    let mut out = ConfirmOut::Open;
    let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let amber = Color32::from_rgb(0xe5, 0xc0, 0x7b);
    style::movable(ctx, Id::new("tpl_confirm"), egui::Window::new(lang.t("Vorlage mit erweiterten Claude-Optionen starten?", "Start template with extended Claude options?")))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            let intro = if lang == Lang::De {
                format!("Die Vorlage {} startet Claude Code mit Optionen, die erweitern, worauf Claude zugreifen darf oder wie es läuft. Vorlagen können von anderen stammen (templates.json lässt sich weitergeben) – prüfe die Werte:", lang.q(&c.name))
            } else {
                format!("The template {} starts Claude Code with options that widen what Claude may access or change how it runs. Templates can come from other people (templates.json can be shared) – check the values:", lang.q(&c.name))
            };
            dialogs::hint_label(ui, &intro, amber);
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("tpl_confirm_rows").max_height(260.0).show(ui, |ui| {
                egui::Grid::new("tpl_confirm_grid").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                    for (session, k, v) in &c.rows {
                        let label = if c.group && !session.is_empty() { format!("{session}: {}", risk_label(*k, lang)) } else { risk_label(*k, lang).to_string() };
                        ui.label(RichText::new(format!("⚠ {label}")).color(amber));
                        ui.add(egui::Label::new(RichText::new(v).monospace()).wrap());
                        ui.end_row();
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let s = ui.button(lang.t("Starten", "Start"));
                mark(ui, "confirm_start", &s);
                if s.clicked() {
                    out = ConfirmOut::Start;
                }
                let w = ui.button(lang.t("Ohne diese Optionen starten", "Start without these options"));
                mark(ui, "confirm_without", &w);
                if w.clicked() {
                    out = ConfirmOut::StartWithout;
                }
                let x = ui.button(lang.t("Abbrechen", "Cancel"));
                mark(ui, "confirm_cancel", &x);
                if c.focus {
                    x.request_focus();
                    c.focus = false;
                }
                if x.clicked() {
                    out = ConfirmOut::Cancel;
                }
            });
        });
    if esc && out == ConfirmOut::Open {
        out = ConfirmOut::Cancel;
    }
    out
}

/// Editor einer Vorlage. Terminal-Vorlagen haben alle Felder einer Sitzung, Gruppen-Vorlagen nur Name und Raster.
#[derive(Clone)]
pub(super) struct TplForm {
    /// Index der bearbeiteten Vorlage; `None` = neue Vorlage.
    pub(super) index: Option<usize>,
    /// Name der Vorlage beim Öffnen (bleibt der Index gültig, solange die Vorlage noch so heißt).
    pub(super) orig_name: Option<String>,
    pub(super) terminal: bool,
    pub(super) name: String,
    pub(super) grid: bool,
    /// Die Sitzung (Terminal-Vorlage) bzw. die erste Sitzung (Gruppe, nur angezeigt).
    pub(super) item: TplItem,
    /// Namen der Sitzungen einer Gruppen-Vorlage (nur angezeigt).
    pub(super) members: Vec<String>,
    pub(super) tags: String,
    pub(super) own_color: bool,
    pub(super) color: [u8; 3],
    pub(super) conv: ConvChoice,
    pub(super) opts: ClaudeOpts,
    /// Freie Argumente aus der alten Zeile `claude_args` (frühere Versionen), die keine Option kennt: bleiben erhalten.
    pub(super) legacy_rest: String,
    pub(super) error: Option<String>,
    focus: bool,
}

impl TplForm {
    /// Editor für eine vorhandene Vorlage (`index`) oder – mit `None` – für eine neue mit diesem Inhalt.
    pub(super) fn of(index: Option<usize>, t: &Template) -> Self {
        let item = t.items.first().cloned().unwrap_or_default();
        let conv = if item.continue_last {
            ConvChoice::ContinueLast
        } else if item.claude_continue == Some(false) {
            ConvChoice::AlwaysNew
        } else {
            ConvChoice::Resume
        };
        TplForm {
            index,
            orig_name: index.map(|_| t.name.clone()),
            terminal: t.is_terminal() || (t.terminal && t.items.is_empty()),
            name: t.name.clone(),
            grid: t.grid,
            tags: item.tags.join(", "),
            own_color: item.color.is_some(),
            color: item.color.unwrap_or([0x56, 0x9c, 0xd6]),
            conv,
            opts: item.opts(),
            legacy_rest: templates::legacy_rest(&item.claude_args),
            members: t.items.iter().map(|i| i.name.clone()).collect(),
            item,
            error: None,
            focus: true,
        }
    }

    /// Neue, leere Terminal-Vorlage.
    pub(super) fn blank() -> Self {
        TplForm::of(None, &Template::terminal("", TplItem::default()))
    }

    /// Vorlage aus dem Editor; `orig` ist die bearbeitete (deren übrige Sitzungen einer Gruppe bleiben).
    pub(super) fn to_template(&self, orig: Option<&Template>) -> Template {
        let name = self.name.trim().to_string();
        if !self.terminal {
            let mut t = orig.cloned().unwrap_or_default();
            t.name = name;
            t.grid = self.grid;
            return t;
        }
        let mut it = self.item.clone();
        it.name = it.name.trim().to_string();
        it.cwd = it.cwd.trim().to_string();
        it.group = state::clean_folder_path(it.group.trim());
        it.tags = TplItem::clean_tags(&session_settings::parse_tags(&self.tags));
        it.color = self.own_color.then_some(self.color);
        it.notes = it.notes.trim_end().to_string();
        if it.claude {
            it.claude_opts = self.opts.sanitized(false);
            it.continue_last = self.conv.continue_last();
            it.claude_continue = (!self.conv.resume_later()).then_some(false);
            it.prompt = it.prompt.trim().to_string();
        } else {
            it.claude_opts = ClaudeOpts::default();
            it.continue_last = false;
            it.claude_continue = None;
            it.prompt = String::new();
        }
        // Die alte Zeile für ältere Versionen (die kennen nur Modell, Effort, Permission-Mode, Agent und Schalter):
        // ohne `--safe-mode` (ältere Versionen fragen nicht nach), dahinter die erhaltenen freien Argumente.
        it.claude_args = if it.claude {
            let mut legacy = it.claude_opts.clone();
            legacy.extra.flags.retain(|f| crate::claude_opts::agent_switch(f));
            format!("{} {}", legacy.to_legacy(), self.legacy_rest.trim()).trim().to_string()
        } else {
            String::new()
        };
        Template::terminal(&name, it)
    }

    /// Fehler vor dem Speichern (Name leer oder schon vergeben).
    pub(super) fn check(&self, list: &[Template], lang: Lang) -> Result<(), String> {
        let n = self.name.trim();
        if n.is_empty() {
            return Err(lang.t("Bitte einen Namen eingeben.", "Please enter a name.").to_string());
        }
        let own = self.live_index(list);
        if list.iter().enumerate().any(|(i, t)| Some(i) != own && t.name.trim().eq_ignore_ascii_case(n)) {
            return Err(format!("{} {}", lang.t("Eine Vorlage heißt schon", "A template is already named"), lang.q(n)));
        }
        Ok(())
    }

    /// Index der bearbeiteten Vorlage, solange es sie unter ihrem Namen noch gibt (sonst wird neu angelegt). Hat sich
    /// die Liste inzwischen verschoben (Löschen, Duplizieren), wird sie über ihren Namen gefunden.
    pub(super) fn live_index(&self, list: &[Template]) -> Option<usize> {
        let (i, n) = (self.index?, self.orig_name.as_ref()?);
        if list.get(i).is_some_and(|t| &t.name == n) {
            return Some(i);
        }
        templates::find(list, n).map(|(i, _)| i)
    }
}

/// Ergebnis eines Bilds des Editors.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(super) enum FormOut {
    Open,
    Save,
    Cancel,
}

/// Der Editor als Fenster (rein, ohne App – headless testbar).
#[allow(clippy::too_many_arguments)]
pub(super) fn tpl_form_ui(ctx: &egui::Context, f: &mut TplForm, shells: &[ShellSpec], profiles: &[Profile], folders: &[String], claude_mode: bool, lang: Lang) -> FormOut {
    let mut out = FormOut::Open;
    let mut opts_ok = true;
    let esc = !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let body_h = dialogs::dialog_body_height(ctx);
    let title = match (f.index.is_some(), f.terminal) {
        (true, true) => lang.t("Terminal-Vorlage bearbeiten", "Edit terminal template"),
        (false, true) => lang.t("Neue Terminal-Vorlage", "New terminal template"),
        _ => lang.t("Gruppen-Vorlage bearbeiten", "Edit group template"),
    };
    style::movable(ctx, Id::new("tpl_editor"), egui::Window::new(title)).collapsible(false).resizable(false).show(ctx, |ui| {
        egui::ScrollArea::vertical().id_salt("tpl_editor_scroll").max_height(body_h).show(ui, |ui| {
            egui::Grid::new("tpl_editor_grid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                ui.label(lang.t("Name der Vorlage", "Template name"));
                let te = ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(320.0).hint_text(lang.t("z. B. API-Server", "e.g. API server")));
                if f.focus {
                    te.request_focus();
                    f.focus = false;
                }
                mark(ui, "name", &te);
                ui.end_row();
                if !f.terminal {
                    ui.label(lang.t("Sitzungen", "Sessions"));
                    ui.label(RichText::new(f.members.join(", ")).color(style::dim(ui)));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut f.grid, lang.t("Als Raster öffnen", "Open as grid"));
                    ui.end_row();
                    return;
                }
                ui.label(lang.t("Sitzungsname", "Session name"));
                ui.add(egui::TextEdit::singleline(&mut f.item.name).desired_width(320.0).hint_text(lang.t("leer = Name der Vorlage", "empty = template name")));
                ui.end_row();
                ui.label("");
                dialogs::hint_label(ui, lang.t("Weitere Starts zählen hoch: API, API #2, API #3 …", "Further starts count up: API, API #2, API #3 …"), style::dim(ui));
                ui.end_row();
                ui.label(lang.t("Profil", "Profile"));
                let cur = profiles.iter().find(|p| p.id == f.item.profile).map(|p| p.name.clone()).unwrap_or_else(|| lang.t("Keins", "None").to_string());
                egui::ComboBox::from_id_salt("tpl_prof").selected_text(cur).show_ui(ui, |ui| {
                    if ui.selectable_label(f.item.profile == 0, lang.t("Keins", "None")).clicked() {
                        f.item.profile = 0;
                    }
                    for p in profiles {
                        let label = if p.admin { format!("{} ({})", p.name, lang.t("Administrator", "administrator")) } else { p.name.clone() };
                        if ui.selectable_label(f.item.profile == p.id, label).clicked() {
                            f.item.profile = p.id;
                            if !p.shell_id.is_empty() {
                                f.item.shell_id = p.shell_id.clone();
                            }
                        }
                    }
                });
                ui.end_row();
                let custom_prof = profiles.iter().find(|p| p.id == f.item.profile).filter(|p| !p.program.trim().is_empty());
                let custom = custom_prof.is_some();
                // Shell, mit der die Sitzung startet: ein eigenes wsl.exe-Programm zählt als WSL (wie beim Start).
                let eff_shell = custom_prof.map(super::custom_program_shell_id).unwrap_or_else(|| f.item.shell_id.clone());
                ui.label("Shell");
                if custom {
                    ui.label(RichText::new(lang.t("Programm des Profils", "Program of the profile")).color(style::dim(ui)));
                } else {
                    let cur = shells.iter().find(|s| s.id == f.item.shell_id).map(|s| s.label.clone()).unwrap_or_else(|| lang.t("Standard-Shell", "Default shell").to_string());
                    egui::ComboBox::from_id_salt("tpl_shell").selected_text(cur).show_ui(ui, |ui| {
                        ui.selectable_value(&mut f.item.shell_id, String::new(), lang.t("Standard-Shell", "Default shell"));
                        for s in shells {
                            ui.selectable_value(&mut f.item.shell_id, s.id.clone(), &s.label);
                        }
                    });
                }
                ui.end_row();
                ui.label(lang.t("Startordner", "Start folder"));
                session_settings::folder_field(ui, &mut f.item.cwd, &eff_shell, false, lang);
                ui.end_row();
                if eff_shell.starts_with("wsl") {
                    #[cfg(test)]
                    ui.data_mut(|d| d.insert_temp(Id::new(("tpl_mark", "wsl_hint")), true));
                    ui.label("");
                    dialogs::hint_label(ui, lang.t("WSL: auch ein Linux-Pfad wie /home/ich/projekt oder ~", "WSL: also a Linux path such as /home/me/project or ~"), style::dim(ui));
                    ui.end_row();
                }
                ui.label(lang.t("Ordner im Baum", "Tree folder"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut f.item.group).hint_text(lang.t("z. B. Backend/API (leer = keiner)", "e.g. Backend/API (empty = none)")).desired_width(290.0));
                    if !folders.is_empty() {
                        ui.menu_button("▾", |ui| {
                            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                                for g in folders {
                                    if ui.button(g).clicked() {
                                        f.item.group = g.clone();
                                        ui.close();
                                    }
                                }
                            });
                        });
                    }
                });
                ui.end_row();
                ui.label(lang.t("Farbe", "Color"));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut f.own_color, lang.t("Eigene Farbe", "Own color"));
                    if f.own_color {
                        ui.color_edit_button_srgb(&mut f.color);
                    }
                });
                ui.end_row();
                ui.label("Tags");
                ui.add(egui::TextEdit::singleline(&mut f.tags).hint_text(lang.t("durch Komma getrennt", "comma-separated")).desired_width(320.0));
                ui.end_row();
                ui.label(lang.t("Notizen", "Notes"));
                ui.add(egui::TextEdit::multiline(&mut f.item.notes).desired_rows(2).desired_width(320.0));
                ui.end_row();
                if claude_mode || f.item.claude {
                    ui.label("");
                    let r = ui.checkbox(&mut f.item.claude, lang.t("Claude Code starten", "Start Claude Code"));
                    mark(ui, "claude", &r);
                    ui.end_row();
                    if f.item.claude {
                        ui.label(lang.t("Konversation", "Conversation"));
                        session_settings::conv_combo(ui, "tpl_conv", &mut f.conv, lang);
                        ui.end_row();
                        // Wie die Standards: kein bypassPermissions in einer Vorlage (Datei kann weitergegeben werden).
                        // Shell der Vorlage (auch aus einem eigenen Profil): WSL-Ordner für WSL-Vorlagen.
                        let target = match shells.iter().find(|s| s.id == eff_shell) {
                            Some(s) => super::folder_picker::Target::for_shell(Some(s), false),
                            None if eff_shell.starts_with("wsl") => super::folder_picker::Target::Wsl { distro: eff_shell.strip_prefix("wsl:").filter(|d| !d.is_empty()).map(str::to_string), choose_distro: false },
                            None => super::folder_picker::Target::Windows,
                        };
                        opts_ok = dialogs::claude_opts_rows(ui, lang, &mut f.opts, "tpled", dialogs::OptsPlace::Template, &target);
                        if !f.legacy_rest.trim().is_empty() {
                            ui.label(lang.t("Weitere Argumente", "Further arguments"));
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(f.legacy_rest.trim()).monospace());
                                if ui.small_button(lang.t("Entfernen", "Remove")).on_hover_text(lang.t("Diese Argumente aus einer früheren Version nicht mehr übergeben", "Stop passing these arguments from an earlier version")).clicked() {
                                    f.legacy_rest.clear();
                                }
                            });
                            ui.end_row();
                            ui.label("");
                            dialogs::hint_label(ui, lang.t("Aus einer früheren Version der Vorlage; werden beim Start unverändert (bereinigt) an Claude Code übergeben.", "From an earlier version of the template; passed to Claude Code at start as they are (cleaned)."), style::dim(ui));
                            ui.end_row();
                        }
                        ui.label(lang.t("Start-Prompt", "Start prompt"));
                        ui.add(egui::TextEdit::multiline(&mut f.item.prompt).desired_rows(2).desired_width(320.0));
                        ui.end_row();
                        ui.label("");
                        dialogs::hint_label(
                            ui,
                            lang.t(
                                "Startet ein Agent die Vorlage (MCP), gelten seine Regeln: nie Administrator-Profile, keine zusätzlichen Ordner, erlaubten Werkzeuge, MCP-Konfiguration oder --safe-mode aus der Vorlage.",
                                "When an agent starts the template (MCP), its rules apply: never administrator profiles, no extra folders, allowed tools, MCP config or --safe-mode from the template.",
                            ),
                            style::dim(ui),
                        );
                        ui.end_row();
                    }
                }
            });
        });
        if let Some(e) = &f.error {
            ui.add_space(4.0);
            dialogs::hint_label(ui, e, Color32::from_rgb(0xe0, 0x6c, 0x75));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let b = ui.add_enabled(opts_ok, egui::Button::new(lang.t("Speichern", "Save")));
            mark(ui, "save", &b);
            if b.clicked() {
                out = FormOut::Save;
            }
            let c = ui.button(lang.t("Abbrechen", "Cancel"));
            mark(ui, "cancel", &c);
            if c.clicked() {
                out = FormOut::Cancel;
            }
        });
    });
    if esc && out == FormOut::Open {
        out = FormOut::Cancel;
    }
    out
}

/// Rechteck eines Elements für Tests merken.
fn mark(ui: &Ui, key: &'static str, r: &egui::Response) {
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(Id::new(("tpl_mark", key)), r.rect));
    let _ = (ui, key, r);
}

impl App {
    /// Beschreibung einer Sitzung für eine Vorlage (Gruppen- und Terminal-Vorlagen).
    pub(super) fn tpl_item_of(&self, e: &Entry) -> TplItem {
        let (o, rest) = e.meta.claude_opts_eff();
        let o = o.sanitized(false);
        // Für ältere Versionen weiter auch als Zeile (die kennen nur Modell, Effort, Permission-Mode, Agent und
        // Schalter) – ohne `--safe-mode`: ältere Versionen fragen vor dem Start nicht nach.
        let mut legacy = o.clone();
        legacy.extra.flags.retain(|f| crate::claude_opts::agent_switch(f));
        TplItem {
            name: e.display_name(),
            shell_id: e.meta.shell_id.clone(),
            cwd: e.cwd_now(),
            claude: e.meta.claude,
            claude_args: format!("{} {rest}", legacy.to_legacy()).trim().to_string(),
            profile: e.meta.profile,
            group: e.meta.group.clone(),
            prompt: String::new(),
            claude_continue: (!e.meta.claude_continue).then_some(false),
            color: Some(e.meta.color),
            tags: e.meta.tags.clone(),
            notes: e.meta.notes.clone(),
            claude_opts: if e.meta.claude { o } else { ClaudeOpts::default() },
            continue_last: false,
        }
    }

    /// Farbe, Tags und Notizen der Vorlage auf eine neue Sitzung übertragen.
    pub(super) fn apply_tpl_extras(&mut self, id: u64, it: &TplItem) {
        let Some(e) = self.entries.get_mut(&id) else { return };
        if let Some(c) = it.color {
            e.meta.color = c;
        }
        let tags = TplItem::clean_tags(&it.tags);
        if !tags.is_empty() {
            e.meta.tags = tags;
        }
        if !it.notes.trim().is_empty() {
            e.meta.notes = it.notes.clone();
        }
    }

    /// Namen aller Sitzungen (für das Hochzählen).
    pub(super) fn session_names(&self) -> Vec<String> {
        self.entries.values().map(|e| e.display_name()).collect()
    }

    /// Vorlage `i` starten (Nutzer: Seitenleiste, Palette, Kontextmenü, Ordner-Menü, Ziehen, alter Vorlagen-Dialog).
    /// Bringt sie Claude-Optionen mit, die erweitern, worauf Claude zugreifen darf oder wie es läuft
    /// ([`TplItem::risky`]), wird zuerst nachgefragt ([`tpl_confirm_ui`]) und noch nichts gestartet. Gibt die neuen
    /// Sitzungen zurück.
    pub(super) fn start_template(&mut self, ctx: &egui::Context, i: usize, at: TplAt) -> Vec<u64> {
        let Some(t) = self.templates.get(i) else { return Vec::new() };
        if needs_confirm(t, self.settings.claude_mode) {
            self.tpl.confirm = Some(TplConfirm::new(i, t, at));
            self.tpl.dragging = None;
            self.templates_open = false;
            self.want_focus = None;
            return Vec::new();
        }
        self.start_template_with(ctx, i, at, false)
    }

    /// Vorlage `i` ohne Rückfrage starten; `drop_risky` = ohne die erfragten Optionen ([`templates::strip_risky`]).
    pub(super) fn start_template_with(&mut self, ctx: &egui::Context, i: usize, at: TplAt, drop_risky: bool) -> Vec<u64> {
        let Some(t) = self.templates.get(i).cloned() else { return Vec::new() };
        let terminal = t.is_terminal();
        // Gruppen-Vorlagen in einen Bereich: wie ein normaler Start (mit Raster).
        let at = if !terminal && matches!(at, TplAt::Pane(..)) { TplAt::Default } else { at };
        let pane = match at {
            TplAt::Pane(tid, z) => Some((tid, z)),
            _ => None,
        };
        let folder = match &at {
            TplAt::Folder(f) if !f.is_empty() => Some(f.clone()),
            _ => None,
        };
        let mut taken = self.session_names();
        let defaults = self.settings.claude_defaults.sanitized(false);
        let mut ids = Vec::new();
        for it in t.items.iter().take(24) {
            let name = if terminal {
                let n = templates::numbered_name(&t.session_name(), &taken);
                taken.push(n.clone());
                n
            } else {
                it.name.clone()
            };
            let group = match (&folder, terminal) {
                (Some(f), true) => f.clone(),
                (Some(f), false) if !it.group.is_empty() => format!("{f}/{}", it.group),
                (Some(f), false) => f.clone(),
                (None, _) => it.group.clone(),
            };
            let claude = it.claude && self.settings.claude_mode;
            // Optionen der Vorlage (strukturiert oder aus der alten Zeile) vor den Standards des Nutzers; die freien Reste
            // der alten Zeile bleiben dabei.
            let (mut own, rest) = it.opts_and_rest();
            if drop_risky {
                own = templates::strip_risky(&own);
            }
            let claude_opts = claude.then(|| own.or(&defaults));
            let id = self.new_session(
                ctx,
                NewOpts {
                    shell_id: Some(it.shell_id.clone()).filter(|x| !x.is_empty()),
                    cwd: Some(it.cwd.clone()),
                    name: Some(name),
                    claude,
                    claude_args: rest,
                    claude_opts,
                    continue_last: claude && it.continue_last,
                    prompt: Some(it.prompt.clone()).filter(|x| !x.trim().is_empty()),
                    profile: Some(it.profile),
                    group: Some(group),
                    claude_continue: it.claude_continue,
                    quiet: pane.is_some(),
                    ..NewOpts::default()
                },
            );
            self.apply_tpl_extras(id, it);
            ids.push(id);
        }
        if let (Some((tid, z)), Some(&id)) = (pane, ids.first()) {
            if self.layout.as_ref().is_some_and(|l| l.contains(tid)) {
                self.drop_into_pane(id, tid, z);
            } else {
                self.activate(id);
            }
        } else if t.grid && ids.len() > 1 {
            let shown: Vec<u64> = ids.iter().copied().take(12).collect();
            if let Some(l) = Node::grid(&shown, Dir::Horizontal) {
                self.layout = Some(l);
                self.focused = shown[0];
                self.want_focus = Some(shown[0]);
            }
        }
        self.templates_open = false;
        ids
    }

    /// Editor speichern: neue Vorlage anhängen oder die bearbeitete ersetzen. `false` bei Fehler (steht im Editor).
    pub(super) fn tpl_form_save(&mut self) -> bool {
        let lang = self.settings.language;
        let Some(f) = self.tpl.form.as_mut() else { return false };
        if let Err(e) = f.check(&self.templates, lang) {
            f.error = Some(e);
            return false;
        }
        let idx = f.live_index(&self.templates);
        let t = f.to_template(idx.and_then(|i| self.templates.get(i)));
        let name = t.name.clone();
        match idx {
            Some(i) => self.templates[i] = t,
            None => self.templates.push(t),
        }
        templates::save(&self.dir, &self.templates);
        self.tpl.form = None;
        self.notify_text(format!("{} {}", lang.t("Vorlage gespeichert:", "Template saved:"), lang.q(&name)));
        true
    }

    /// Ordner des aktiven Projekts (für Auswahllisten).
    fn project_folders(&self) -> Vec<String> {
        let mut v: Vec<String> = self.projects.iter().find(|p| p.id == self.active_project).map(|p| p.folders.iter().map(|f| f.path.clone()).collect()).unwrap_or_default();
        v.sort_by_key(|x| x.to_lowercase());
        v.dedup();
        v
    }

    /// Rückfrage vor dem Start zeigen (falls offen).
    pub(super) fn tpl_confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut c) = self.tpl.confirm.take() else { return };
        let out = tpl_confirm_ui(ctx, &mut c, self.settings.language);
        let (idx, at) = (c.live_index(&self.templates), c.at.clone());
        match out {
            ConfirmOut::Open => self.tpl.confirm = Some(c),
            ConfirmOut::Cancel => {}
            ConfirmOut::Start | ConfirmOut::StartWithout => {
                if let Some(i) = idx {
                    self.start_template_with(ctx, i, at, out == ConfirmOut::StartWithout);
                }
            }
        }
    }

    /// Editor zeigen (falls offen).
    pub(super) fn tpl_editor_ui(&mut self, ctx: &egui::Context) {
        self.tpl_confirm_dialog(ctx);
        let Some(mut f) = self.tpl.form.take() else { return };
        let lang = self.settings.language;
        let folders = self.project_folders();
        let out = tpl_form_ui(ctx, &mut f, &self.shells, &self.settings.profiles, &folders, self.settings.claude_mode, lang);
        self.tpl.form = Some(f);
        match out {
            FormOut::Open => {}
            FormOut::Cancel => self.tpl.form = None,
            FormOut::Save => {
                self.tpl_form_save();
            }
        }
    }

    /// Abschnitt „Vorlagen“ unten in der Seitenleiste (einklappbar), über der Fußzeile.
    pub(super) fn templates_section(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let max_h = (ui.available_height() * 0.4).max(90.0);
        egui::TopBottomPanel::bottom("sidebar_templates").frame(egui::Frame::NONE).show_separator_line(false).resizable(false).show_inside(ui, |ui| {
            ui.add_space(S1);
            let id = ui.make_persistent_id("tpl_section");
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
            let open = state.is_open();
            ui.horizontal(|ui| {
                let (r, resp) = ui.allocate_exact_size(Vec2::new((ui.available_width() - 34.0).max(40.0), 22.0), Sense::click());
                let p = ui.painter();
                let weak = style::dim(ui);
                let cx = r.min.x + 8.0;
                let cy = r.center().y;
                let pts = if open {
                    vec![Pos2::new(cx - 4.0, cy - 2.5), Pos2::new(cx + 4.0, cy - 2.5), Pos2::new(cx, cy + 3.0)]
                } else {
                    vec![Pos2::new(cx - 2.5, cy - 4.0), Pos2::new(cx - 2.5, cy + 4.0), Pos2::new(cx + 3.0, cy)]
                };
                p.add(egui::Shape::convex_polygon(pts, weak, Stroke::NONE));
                let g = p.layout_no_wrap(format!("{}  {}", lang.t("VORLAGEN", "TEMPLATES"), self.templates.len()), FontId::proportional(11.5), weak);
                p.galley(Pos2::new(r.min.x + 18.0, cy - g.size().y / 2.0), g, weak);
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.on_hover_text(lang.t("Vorlagen ein-/ausklappen", "Show/hide templates")).clicked() {
                    state.toggle(ui);
                }
                ui.spacing_mut().item_spacing.x = 0.0;
                if style::icon_btn_small(ui, SIcon::Plus, lang.t("Neue Terminal-Vorlage …", "New terminal template …")).clicked() {
                    actions.push(Action::TemplateNew);
                }
            });
            state.store(ui.ctx());
            if !open {
                return;
            }
            if self.templates.is_empty() {
                ui.label(
                    RichText::new(lang.t(
                        "Noch keine Vorlagen. Rechtsklick auf eine Sitzung → „Als Terminal-Vorlage speichern …“ oder auf einen Ordner → „Als Vorlage speichern …“.",
                        "No templates yet. Right-click a session → “Save as terminal template …” or a folder → “Save as template …”.",
                    ))
                    .small()
                    .color(style::dim(ui)),
                );
                return;
            }
            if self.templates.len() > 4 || !self.tpl.search.is_empty() {
                ui.add(egui::TextEdit::singleline(&mut self.tpl.search).hint_text(lang.t("Vorlagen suchen", "Search templates")).desired_width(f32::INFINITY));
            }
            let q = self.tpl.search.trim().to_string();
            let shown: Vec<usize> = (0..self.templates.len()).filter(|i| self.templates[*i].matches(&q)).collect();
            egui::ScrollArea::vertical().id_salt("tpl_section_list").max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                if shown.is_empty() {
                    ui.label(RichText::new(lang.t("Keine Treffer.", "No matches.")).small().color(style::dim(ui)));
                }
                for i in shown {
                    self.template_row(ui, i, actions);
                }
            });
        });
    }

    /// Eine Zeile der Bibliothek: Doppelklick startet, Ziehen legt ab, Rechtsklick öffnet das Menü.
    fn template_row(&mut self, ui: &mut Ui, i: usize, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let Some(t) = self.templates.get(i) else { return };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            self.tpl.dragging = Some(i);
        }
        let (text_c, strong_c, hover_bg) = {
            let v = ui.visuals();
            (v.text_color(), v.strong_text_color(), v.widgets.hovered.bg_fill)
        };
        let weak = style::dim(ui);
        let p = ui.painter().clone();
        let hv = style::anim(ui.ctx(), resp.id.with("h"), resp.hovered(), style::D_FAST);
        p.rect_filled(rect, 6.0, hover_bg.gamma_multiply(hv));
        let cy = rect.center().y;
        let item = t.item();
        let (icon, icol) = match item {
            Some(it) if it.claude && self.settings.claude_mode => (SIcon::SparkSmall, style::ACCENT),
            Some(it) => (SIcon::Terminal, it.color.map(|c| Color32::from_rgb(c[0], c[1], c[2])).unwrap_or(weak)),
            None => (SIcon::Template, weak),
        };
        style::paint_icon(&p, icon, Pos2::new(rect.min.x + 12.0, cy), icol);
        let mut right = rect.max.x - 6.0;
        // Erweiterte Claude-Optionen: kleines Warnsymbol (Tooltip nennt sie; vor dem Start wird gefragt).
        if needs_confirm(t, self.settings.claude_mode) {
            let amber = Color32::from_rgb(0xe5, 0xc0, 0x7b);
            let g = p.layout_no_wrap("⚠".to_string(), FontId::proportional(12.0), amber);
            right -= g.size().x;
            #[cfg(test)]
            ui.data_mut(|d| d.insert_temp(Id::new(("tpl_mark_warn", i)), true));
            p.galley(Pos2::new(right, cy - g.size().y / 2.0), g, amber);
            right -= 6.0;
        }
        if !t.is_terminal() {
            let g = p.layout_no_wrap(t.items.len().to_string(), FontId::proportional(11.0), weak);
            right -= g.size().x;
            p.galley(Pos2::new(right, cy - g.size().y / 2.0), g, weak);
            right -= 6.0;
        }
        let mut job = egui::text::LayoutJob::simple_singleline(t.name.clone(), FontId::proportional(13.0), if resp.hovered() { strong_c } else { text_c });
        job.wrap = egui::text::TextWrapping { max_width: (right - rect.min.x - 28.0).max(20.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = p.layout_job(job);
        p.galley(Pos2::new(rect.min.x + 26.0, cy - g.size().y / 2.0), g, text_c);
        let tip = tpl_tip(t, &self.shells, &self.settings.profiles, lang);
        let resp = resp.on_hover_text(tip);
        if resp.double_clicked() {
            actions.push(Action::TemplateStartAt(i, TplAt::Default));
        }
        let terminal = t.is_terminal();
        let folders = self.project_folders();
        resp.context_menu(|ui| {
            if ui.button(lang.t("Starten", "Start")).clicked() {
                actions.push(Action::TemplateStartAt(i, TplAt::Default));
                ui.close();
            }
            if !folders.is_empty() {
                ui.menu_button(lang.t("Starten in Ordner", "Start in folder"), |ui| {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for f in &folders {
                            if ui.button(f).clicked() {
                                actions.push(Action::TemplateStartAt(i, TplAt::Folder(f.clone())));
                                ui.close();
                            }
                        }
                    });
                });
            }
            ui.separator();
            if ui.button(lang.t("Bearbeiten …", "Edit …")).clicked() {
                actions.push(Action::TemplateEdit(i));
                ui.close();
            }
            if ui.button(lang.t("Duplizieren", "Duplicate")).clicked() {
                actions.push(Action::TemplateDuplicate(i));
                ui.close();
            }
            // Rückfrage mit Wartezeit statt „zweimal klicken“ (#276, `closing`).
            if ui.button(lang.t("Löschen …", "Delete …")).clicked() {
                actions.push(Action::TemplateDeleteAsk(i));
                ui.close();
            }
            if !terminal {
                ui.separator();
                ui.label(RichText::new(lang.t("Gruppen-Vorlage: startet alle Sitzungen", "Group template: starts all sessions")).small().color(style::dim(ui)));
            }
        });
    }

    /// Eine Vorlage wird gezogen: Ziel anzeigen (Ordner im Baum, Sitzungsliste, Bereich mit Zone) und beim Loslassen starten.
    /// `list_rect` = sichtbare Sitzungsliste der Seitenleiste in diesem Bild (Ordner zählen nur darin, nicht
    /// herausgescrollte). Ohne Zeiger über dem Fenster gibt es kein Ziel (Loslassen außerhalb tut nichts); ist die Taste
    /// nicht mehr gedrückt, ohne dass ein Loslassen ankam (z. B. Moduswechsel während des Ziehens), endet das Ziehen.
    pub(super) fn handle_tpl_drag(&mut self, ctx: &egui::Context, list_rect: Option<Rect>) {
        let Some(i) = self.tpl.dragging else { return };
        let Some(t) = self.templates.get(i).cloned() else {
            self.tpl.dragging = None;
            return;
        };
        let lang = self.settings.language;
        let (pos, released, down, esc) = ctx.input(|inp| (inp.pointer.hover_pos(), inp.pointer.any_released(), inp.pointer.primary_down(), inp.key_pressed(Key::Escape)));
        if !down && !released {
            self.tpl.dragging = None;
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("tpl_drag_layer")));
        let accent = style::ACCENT;
        let fill = Color32::from_rgba_unmultiplied(0xd9, 0x77, 0x57, 40);
        let own_group = t.items.first().map(|it| it.group.clone()).unwrap_or_default();
        let target: Option<(TplAt, String)> = pos.and_then(|p| {
            let in_list = list_rect.is_some_and(|r| r.contains(p));
            let folder = in_list
                .then(|| self.folder_rects.iter().filter(|(_, r)| r.contains(p)).min_by(|a, b| (a.1.width() * a.1.height()).total_cmp(&(b.1.width() * b.1.height()))).map(|(f, r)| (f.clone(), list_rect.map_or(*r, |l| r.intersect(l)))))
                .flatten();
            if let Some((f, r)) = folder {
                if f.is_empty() {
                    let label = if own_group.is_empty() || !t.is_terminal() { lang.t("Kein Ordner", "No folder").to_string() } else { format!("→ {own_group}") };
                    return Some((TplAt::Default, label));
                }
                painter.rect_filled(r, 6.0, fill);
                painter.rect_stroke(r.shrink(1.0), 6.0, Stroke::new(1.5_f32, accent), StrokeKind::Inside);
                return Some((TplAt::Folder(f.clone()), format!("→ {f}")));
            }
            if in_list {
                let label = if own_group.is_empty() || !t.is_terminal() { lang.t("Starten", "Start").to_string() } else { format!("→ {own_group}") };
                return Some((TplAt::Default, label));
            }
            let (tid, r) = self.pane_rects.iter().find(|(_, r)| r.contains(p)).map(|(id, r)| (*id, *r))?;
            if !t.is_terminal() {
                return Some((TplAt::Default, lang.t("Starten", "Start").to_string()));
            }
            let z = drop_zone(r, p);
            painter.rect_filled(zone_rect(r, z), 0.0, Color32::from_rgba_unmultiplied(0xd9, 0x77, 0x57, 70));
            painter.rect_stroke(zone_rect(r, z).shrink(2.0), 4.0, Stroke::new(2.0_f32, accent), StrokeKind::Inside);
            Some((TplAt::Pane(tid, z), zone_label(z, lang).to_string()))
        });
        if let Some(p) = pos {
            let label = match &target {
                Some((_, l)) => format!("{}  {l}", t.name),
                None => t.name.clone(),
            };
            let g = painter.layout_no_wrap(label, FontId::proportional(13.0), Color32::WHITE);
            let mut lr = Rect::from_min_size(p + Vec2::new(12.0, 12.0), g.size() + Vec2::new(12.0, 6.0));
            let sr = ctx.content_rect();
            if lr.max.x > sr.max.x - 4.0 {
                lr = lr.translate(Vec2::new(sr.max.x - 4.0 - lr.max.x, 0.0));
            }
            painter.rect_filled(lr, 4.0, Color32::from_black_alpha(200));
            painter.galley(lr.min + Vec2::new(6.0, 3.0), g, Color32::WHITE);
            ctx.set_cursor_icon(if target.is_some() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::NoDrop });
        }
        if esc {
            self.tpl.dragging = None;
            return;
        }
        if released {
            self.tpl.dragging = None;
            if let Some((at, _)) = target {
                self.start_template(ctx, i, at);
            }
        }
    }
}

/// Fragt Paneldeck vor dem Start dieser Vorlage nach (erweiterte Claude-Optionen, nur mit Claude-Modus)?
pub(super) fn needs_confirm(t: &Template, claude_mode: bool) -> bool {
    claude_mode && !t.risky().is_empty()
}

/// Text der Zone eines Bereichs (wie beim Ziehen von Sitzungen).
fn zone_label(z: u8, lang: Lang) -> &'static str {
    match z {
        1 => lang.t("Links teilen", "Split left"),
        2 => lang.t("Rechts teilen", "Split right"),
        3 => lang.t("Oben teilen", "Split above"),
        4 => lang.t("Unten teilen", "Split below"),
        _ => lang.t("Ersetzen", "Replace"),
    }
}

/// Hervorgehobene Fläche einer Zone.
fn zone_rect(r: Rect, z: u8) -> Rect {
    match z {
        1 => Rect::from_min_max(r.min, Pos2::new(r.center().x, r.max.y)),
        2 => Rect::from_min_max(Pos2::new(r.center().x, r.min.y), r.max),
        3 => Rect::from_min_max(r.min, Pos2::new(r.max.x, r.center().y)),
        4 => Rect::from_min_max(Pos2::new(r.min.x, r.center().y), r.max),
        _ => r,
    }
}

/// Tooltip einer Vorlage: was sie startet und wie.
pub(super) fn tpl_tip(t: &Template, shells: &[ShellSpec], profiles: &[Profile], lang: Lang) -> String {
    let mut lines = vec![t.name.clone()];
    match t.item() {
        Some(it) => {
            if !it.name.trim().is_empty() && it.name.trim() != t.name.trim() {
                lines.push(format!("{}: {}", lang.t("Sitzung", "Session"), it.name.trim()));
            }
            if let Some(p) = profiles.iter().find(|p| p.id == it.profile) {
                lines.push(format!("{}: {}", lang.t("Profil", "Profile"), p.name));
            }
            if let Some(s) = shells.iter().find(|s| s.id == it.shell_id) {
                lines.push(format!("Shell: {}", s.label));
            }
            if !it.cwd.is_empty() {
                lines.push(format!("{}: {}", lang.t("Ordner", "Folder"), it.cwd));
            }
            if !it.group.is_empty() {
                lines.push(format!("{}: {}", lang.t("Ordner im Baum", "Tree folder"), it.group));
            }
            if !it.tags.is_empty() {
                lines.push(it.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
            }
            if it.claude {
                let s = it.opts().summary(lang.t("Agent", "Agent"));
                lines.push(if s.is_empty() { "Claude Code".to_string() } else { format!("Claude Code · {s}") });
            }
        }
        None => lines.push(format!("{} {}: {}", t.items.len(), lang.t("Sitzungen", "sessions"), t.items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
    let risky = t.risky();
    if !risky.is_empty() {
        lines.push(String::new());
        lines.push(format!("⚠ {}", lang.t("Erweiterte Claude-Optionen (vor dem Start wird gefragt):", "Extended Claude options (asks before starting):")));
        for (_, k, v) in risky.iter().take(8) {
            let v: String = v.chars().take(80).collect();
            lines.push(format!("  {}: {v}", risk_label(*k, lang)));
        }
    }
    lines.push(String::new());
    lines.push(lang.t("Doppelklick startet; ziehen in die Sitzungsliste, einen Ordner oder einen Bereich.", "Double-click starts; drag into the session list, a folder or a pane.").to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, RawInput};

    fn shells() -> Vec<ShellSpec> {
        vec![ShellSpec { id: "powershell".into(), label: "PowerShell".into(), program: String::new(), args: vec![] }, ShellSpec { id: "wsl:Ubuntu".into(), label: "WSL Ubuntu".into(), program: String::new(), args: vec![] }]
    }

    fn frame(ctx: &egui::Context, f: &mut TplForm, events: Vec<Event>) -> FormOut {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))), events, ..RawInput::default() };
        let mut out = FormOut::Open;
        let _ = ctx.run(input, |ctx| {
            out = tpl_form_ui(ctx, f, &shells(), &[], &["Work".into()], true, Lang::En);
        });
        out
    }

    fn rect(ctx: &egui::Context, key: &'static str) -> Option<Rect> {
        ctx.data(|d| d.get_temp::<Rect>(Id::new(("tpl_mark", key))))
    }

    fn click(ctx: &egui::Context, f: &mut TplForm, key: &'static str) -> FormOut {
        // Zeiger hinbewegen, bis das Element ruhig steht (das Fenster passt seine Größe über einige Bilder an).
        let mut p = rect(ctx, key).expect(key).center();
        for _ in 0..30 {
            frame(ctx, f, vec![Event::PointerMoved(p)]);
            let now = rect(ctx, key).expect(key).center();
            if now == p {
                break;
            }
            p = now;
        }
        frame(ctx, f, vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        frame(ctx, f, vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }])
    }

    #[test]
    fn form_round_trips_a_terminal_template() {
        let it = TplItem {
            name: "API".into(),
            shell_id: "wsl:Ubuntu".into(),
            cwd: "/home/me".into(),
            claude: true,
            profile: 0,
            group: "Work//API/".into(),
            prompt: " hi ".into(),
            tags: vec!["web".into(), "rust".into()],
            color: Some([9, 8, 7]),
            notes: "n\n".into(),
            claude_opts: ClaudeOpts { model: "opus".into(), ..ClaudeOpts::default() },
            continue_last: true,
            ..TplItem::default()
        };
        let t = Template::terminal("Backend", it);
        let f = TplForm::of(Some(0), &t);
        assert_eq!((f.conv, f.tags.as_str(), f.own_color, f.opts.model.as_str()), (ConvChoice::ContinueLast, "web, rust", true, "opus"));
        let back = f.to_template(Some(&t));
        let b = back.item().unwrap();
        assert_eq!((b.group.as_str(), b.prompt.as_str(), b.notes.as_str()), ("Work/API", "hi", "n"));
        assert!(b.continue_last && b.claude_continue.is_none());
        // Für ältere Versionen auch als Zeile (#244 Review).
        assert_eq!(b.claude_args, "--model opus");
        assert_eq!(b.claude_opts.model, "opus");
        assert_eq!(b.color, Some([9, 8, 7]));
        // Claude aus: Optionen, Prompt und Konversation entfallen.
        let mut f2 = f.clone();
        f2.item.claude = false;
        let b2 = f2.to_template(None);
        let b2 = b2.item().unwrap();
        assert!(b2.claude_opts.is_empty() && b2.prompt.is_empty() && !b2.continue_last);
        // Immer neu
        let mut f3 = f.clone();
        f3.conv = ConvChoice::AlwaysNew;
        assert_eq!(f3.to_template(None).items[0].claude_continue, Some(false));
    }

    #[test]
    fn group_templates_keep_their_sessions() {
        let g = Template { name: "G".into(), grid: true, items: vec![TplItem { name: "a".into(), ..TplItem::default() }, TplItem { name: "b".into(), ..TplItem::default() }], terminal: false };
        let mut f = TplForm::of(Some(0), &g);
        assert!(!f.terminal);
        f.name = "G2".into();
        f.grid = false;
        let t = f.to_template(Some(&g));
        assert_eq!((t.name.as_str(), t.grid, t.items.len()), ("G2", false, 2));
    }

    #[test]
    fn names_must_be_set_and_unique() {
        let list = vec![Template { name: "API".into(), ..Template::default() }, Template { name: "Web".into(), ..Template::default() }];
        let mut f = TplForm::blank();
        assert!(f.check(&list, Lang::En).is_err(), "leer");
        f.name = "api".into();
        assert!(f.check(&list, Lang::En).is_err(), "schon vergeben");
        f.name = "New".into();
        assert!(f.check(&list, Lang::En).is_ok());
        // Die eigene Vorlage darf ihren Namen behalten.
        let mut e = TplForm::of(Some(0), &list[0]);
        assert!(e.check(&list, Lang::En).is_ok());
        e.name = "Web".into();
        assert!(e.check(&list, Lang::En).is_err());
        // Hat sich die Liste verschoben (Löschen/Duplizieren davor), wird sie über ihren Namen gefunden (#244 Review);
        // gibt es sie nicht mehr, wird neu angelegt.
        assert_eq!(TplForm::of(Some(0), &list[0]).live_index(&list), Some(0));
        assert_eq!(TplForm::of(Some(1), &list[0]).live_index(&list), Some(0));
        assert_eq!(TplForm::of(Some(5), &list[0]).live_index(&list), Some(0));
        let gone = Template { name: "Gone".into(), ..Template::default() };
        assert_eq!(TplForm::of(Some(0), &gone).live_index(&list), None);
    }

    #[test]
    fn editing_keeps_free_legacy_arguments_and_writes_the_line_for_old_versions() {
        // Vorlage einer früheren Version: nur die Zeile.
        let old = Template::terminal("Old", TplItem { claude: true, claude_args: "--model opus --foo bar".into(), ..TplItem::default() });
        let mut f = TplForm::of(Some(0), &old);
        assert_eq!((f.opts.model.as_str(), f.legacy_rest.as_str()), ("opus", "--foo bar"));
        f.opts.extra.flags = vec!["ide".into(), "safe-mode".into()];
        f.opts.effort = "high".into();
        let t = f.to_template(Some(&old));
        let it = t.item().unwrap();
        assert_eq!(it.claude_opts.model, "opus");
        // Ältere Versionen: Modell, Effort und Schalter als Zeile (ohne --safe-mode), dahinter die freien Argumente.
        assert_eq!(it.claude_args, "--model opus --effort high --ide --foo bar");
        // Wieder geöffnet: dieselben freien Argumente, nicht doppelt.
        let f2 = TplForm::of(Some(0), &t);
        assert_eq!(f2.legacy_rest, "--foo bar");
        assert_eq!(f2.to_template(Some(&t)).items[0].claude_args, it.claude_args);
        // Entfernt der Nutzer sie, fallen sie weg.
        let mut f3 = f2.clone();
        f3.legacy_rest.clear();
        assert_eq!(f3.to_template(Some(&t)).items[0].claude_args, "--model opus --effort high --ide");
        // Ohne Claude keine Zeile.
        let mut f4 = f2.clone();
        f4.item.claude = false;
        assert!(f4.to_template(None).items[0].claude_args.is_empty());
    }

    // Nur Windows: WSL gibt es nur dort (`wsl.exe` aus System32); außerhalb von Windows ist ein Programm `C:\…\wsl.exe` kein
    // Pfad mit Ordnern (`\` trennt dort nicht), und ein eigenes Profil zählt dort absichtlich nie als WSL.
    #[cfg(windows)]
    #[test]
    fn custom_wsl_program_counts_as_wsl_in_the_editor() {
        let ctx = egui::Context::default();
        let prof = Profile { id: 7, name: "My WSL".into(), program: r"C:\Windows\System32\wsl.exe".into(), args: "-d Debian".into(), ..Profile::default() };
        let mut f = TplForm::blank();
        f.item.profile = 7;
        f.item.shell_id = "powershell".into();
        let hint = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<bool>(Id::new(("tpl_mark", "wsl_hint")))).unwrap_or(false);
        let run = |ctx: &egui::Context, f: &mut TplForm, profiles: &[Profile]| {
            ctx.data_mut(|d| d.remove::<bool>(Id::new(("tpl_mark", "wsl_hint"))));
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))), ..RawInput::default() };
            let _ = ctx.run(input, |ctx| {
                tpl_form_ui(ctx, f, &shells(), profiles, &[], true, Lang::En);
            });
        };
        run(&ctx, &mut f, std::slice::from_ref(&prof));
        assert!(hint(&ctx), "eigenes wsl.exe-Programm = WSL");
        run(&ctx, &mut f, &[Profile { program: r"C:\tools\nu.exe".into(), ..prof.clone() }]);
        assert!(!hint(&ctx), "anderes Programm: kein WSL");
        assert_eq!(super::super::custom_program_shell_id(&prof), "wsl:Debian");
    }

    /// Absoluter Ordner in der Form der Plattform: `--add-dir` nimmt nur solche an (`claude_opts::valid_path`).
    fn secret_dir() -> &'static str {
        if cfg!(windows) { r"C:\secret" } else { "/secret" }
    }

    fn risky_template() -> Template {
        let o = ClaudeOpts { extra: crate::claude_opts::ExtraOpts { add_dirs: vec![secret_dir().into()], flags: vec!["safe-mode".into()], ..Default::default() }, ..ClaudeOpts::default() };
        Template::terminal("Risky", TplItem { claude: true, claude_opts: o, ..TplItem::default() })
    }

    #[test]
    fn risky_templates_ask_before_starting() {
        let t = risky_template();
        assert!(needs_confirm(&t, true));
        assert!(!needs_confirm(&t, false), "ohne Claude-Modus startet kein Claude");
        assert!(!needs_confirm(&Template::terminal("Plain", TplItem { claude: true, ..TplItem::default() }), true));
        let tip = tpl_tip(&t, &[], &[], Lang::En);
        assert!(tip.contains("⚠") && tip.contains(secret_dir()) && tip.contains("--safe-mode"), "{tip}");
        assert!(tpl_tip(&t, &[], &[], Lang::De).contains("vor dem Start wird gefragt"));
        // Die Rückfrage findet ihre Vorlage über den Namen wieder.
        let c = TplConfirm::new(3, &t, TplAt::Default);
        assert_eq!(c.rows.len(), 2);
        let list = vec![Template::terminal("Other", TplItem::default()), t.clone()];
        assert_eq!(c.live_index(&list), Some(1));
        assert_eq!(c.live_index(&list[..1]), None);
    }

    fn confirm_frame(ctx: &egui::Context, c: &mut TplConfirm, events: Vec<Event>) -> ConfirmOut {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))), events, ..RawInput::default() };
        let mut out = ConfirmOut::Open;
        let _ = ctx.run(input, |ctx| {
            out = tpl_confirm_ui(ctx, c, Lang::En);
        });
        out
    }

    fn confirm_click(ctx: &egui::Context, c: &mut TplConfirm, key: &'static str) -> ConfirmOut {
        let mut p = rect(ctx, key).expect(key).center();
        for _ in 0..30 {
            confirm_frame(ctx, c, vec![Event::PointerMoved(p)]);
            let now = rect(ctx, key).expect(key).center();
            if now == p {
                break;
            }
            p = now;
        }
        confirm_frame(ctx, c, vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        confirm_frame(ctx, c, vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }])
    }

    #[test]
    fn confirm_dialog_buttons_focus_and_escape() {
        let t = risky_template();
        let ctx = egui::Context::default();
        let mut c = TplConfirm::new(0, &t, TplAt::Default);
        assert_eq!(confirm_frame(&ctx, &mut c, vec![]), ConfirmOut::Open);
        // Der Dialog nimmt die Tastatur (Fokus auf „Abbrechen“), nicht das Terminal dahinter.
        assert!(ctx.memory(|m| m.focused()).is_some());
        confirm_frame(&ctx, &mut c, vec![]);
        assert_eq!(confirm_click(&ctx, &mut c, "confirm_start"), ConfirmOut::Start);
        assert_eq!(confirm_click(&ctx, &mut c, "confirm_without"), ConfirmOut::StartWithout);
        assert_eq!(confirm_click(&ctx, &mut c, "confirm_cancel"), ConfirmOut::Cancel);
        let esc = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        assert_eq!(confirm_frame(&ctx, &mut c, vec![esc]), ConfirmOut::Cancel);
        for k in [templates::Risk::AddDir, templates::Risk::PluginDir, templates::Risk::AllowedTools, templates::Risk::McpConfig, templates::Risk::SafeMode, templates::Risk::SystemPrompt, templates::Risk::AppendSystemPrompt] {
            assert_ne!(risk_label(k, Lang::En), risk_label(k, Lang::De));
        }
    }

    #[test]
    fn editor_saves_cancels_and_shows_claude_rows_only_when_on() {
        let ctx = egui::Context::default();
        let mut f = TplForm::blank();
        f.name = "API".into();
        frame(&ctx, &mut f, vec![]);
        frame(&ctx, &mut f, vec![]);
        assert!(rect(&ctx, "name").is_some() && rect(&ctx, "claude").is_some());
        // Claude einschalten zeigt die Optionen (Konversation, Modell …) – die Zeilen kommen in den Editor.
        assert_eq!(click(&ctx, &mut f, "claude"), FormOut::Open);
        assert!(f.item.claude);
        // Das Fenster wächst mit den Claude-Zeilen erst über ein paar Bilder.
        for _ in 0..4 {
            frame(&ctx, &mut f, vec![]);
        }
        assert_eq!(click(&ctx, &mut f, "save"), FormOut::Save);
        assert_eq!(click(&ctx, &mut f, "cancel"), FormOut::Cancel);
        // Esc bricht ab.
        let esc = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        assert_eq!(frame(&ctx, &mut f, vec![esc]), FormOut::Cancel);
    }

    #[test]
    fn tooltip_names_what_starts() {
        let t = Template::terminal("Backend", TplItem { name: "API".into(), shell_id: "powershell".into(), cwd: r"C:\src".into(), group: "Work".into(), tags: vec!["web".into()], ..TplItem::default() });
        let tip = tpl_tip(&t, &shells(), &[], Lang::En);
        for s in ["Backend", "Session: API", "Shell: PowerShell", r"Folder: C:\src", "Tree folder: Work", "#web", "Double-click"] {
            assert!(tip.contains(s), "{s}: {tip}");
        }
        let tip = tpl_tip(&t, &shells(), &[], Lang::De);
        assert!(tip.contains("Doppelklick") && tip.contains("Ordner im Baum"), "{tip}");
        let g = Template { name: "G".into(), items: vec![TplItem { name: "a".into(), ..TplItem::default() }, TplItem { name: "b".into(), ..TplItem::default() }], ..Template::default() };
        assert!(tpl_tip(&g, &[], &[], Lang::En).contains("2 sessions: a, b"));
    }

    #[test]
    fn zones_and_labels() {
        let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0));
        assert_eq!(drop_zone(r, Pos2::new(50.0, 50.0)), 0);
        assert_eq!(drop_zone(r, Pos2::new(5.0, 50.0)), 1);
        assert_eq!(drop_zone(r, Pos2::new(95.0, 50.0)), 2);
        assert_eq!(drop_zone(r, Pos2::new(50.0, 5.0)), 3);
        assert_eq!(drop_zone(r, Pos2::new(50.0, 95.0)), 4);
        assert_eq!(zone_rect(r, 2).min.x, 50.0);
        for z in 0..5 {
            assert_ne!(zone_label(z, Lang::En), zone_label(z, Lang::De));
        }
    }
}
