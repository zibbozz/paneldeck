//! Sitzung neu laden (#169) und Einstellungen einer Sitzung nachträglich ändern (#170).
//!
//! * Neu laden: Knopf in der Kopfzeile des Bereichs (neben Schließen/Minimieren/Teilen), Eintrag in der Befehls-Palette
//!   und im Kontextmenü der Seitenleiste. Rückfrage ohne Wartezeit, solange ein Prozess läuft (ein beendeter Prozess
//!   startet gleich neu). Was neu startet, folgt den Einstellungen der Sitzung (`reload_plan`): ein Terminal startet
//!   seine Shell neu, eine Claude-Sitzung Claude – mit Fortsetzen der Konversation nur bei `claude_continue`, sonst neu.
//!   Chats haben keinen Prozess: dort gibt es kein Neu laden.
//! * Einstellungen: „Einstellungen …“ im Kontextmenü einer Sitzung. Alles, was beim Anlegen einstellbar ist (Profil,
//!   Shell, Arbeitsordner, Name, Farbe, Ordner im Baum, Tags, Notizen, Claude an/aus, „Konversation fortsetzen“).
//!   Startbefehl und Umgebungsvariablen gehören zum Profil. Chats bleiben Chats (nur Name, Farbe, Ordner, Tags, Notizen).
//!   Ändert sich etwas, das erst beim Start wirkt (Profil, Shell, Arbeitsordner, Claude an/aus), fragt der Dialog nach dem
//!   Speichern „Jetzt neu laden?“; sonst gilt die Änderung beim nächsten Neu laden.

use super::closing::{confirm_window, Outcome};
use super::*;

/// Wie Claude beim Start einer Sitzung beginnt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClaudeStart {
    /// Diese Konversation fortsetzen (`--resume <id>`).
    Resume(String),
    /// Letzte Konversation im Ordner fortsetzen (`--continue`).
    Continue,
    /// Neue Konversation mit eigener ID (`--session-id`).
    Fresh,
}

/// Soll beim Start eine vorhandene Konversation fortgesetzt werden? Neu laden/Neu starten (`restart`) nur, wenn die
/// Sitzung „Konversation fortsetzen“ an hat; beim Programmstart (`restore`) zusätzlich nach der globalen Einstellung.
pub(super) fn resume_wanted(restore: bool, restart: bool, session_continue: bool, global_resume: bool) -> bool {
    session_continue && (restart || (restore && global_resume))
}

/// Entscheidung, wie Claude startet. `resume_id` = ausdrücklich gewünschte Konversation (Verlauf), `continue_last` =
/// Häkchen „--continue“ beim Anlegen, `wsl` = die Sitzung läuft in WSL (dort gibt es keine Konversations-ID aus den Hooks).
pub(super) fn claude_start(resume_id: Option<&str>, resume_wanted: bool, continue_last: bool, meta: &SessionMeta, wsl: bool) -> ClaudeStart {
    if let Some(id) = resume_id.filter(|id| crate::quote::is_uuid(id)) {
        ClaudeStart::Resume(id.to_string())
    } else if resume_wanted && meta.claude_started && crate::quote::is_uuid(&meta.claude_session) {
        ClaudeStart::Resume(meta.claude_session.clone())
    } else if (resume_wanted && wsl) || continue_last {
        ClaudeStart::Continue
    } else {
        ClaudeStart::Fresh
    }
}

/// Was „Neu laden“ startet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReloadPlan {
    /// Terminal ohne Claude: die Shell startet neu.
    Shell,
    /// Claude startet neu und setzt die Konversation fort (falls es schon eine gibt).
    ClaudeContinue,
    /// Claude startet neu mit einer neuen Konversation.
    ClaudeFresh,
}

/// Plan für eine Sitzung; `None` bei Chats (kein Prozess).
pub(super) fn reload_plan(meta: &SessionMeta) -> Option<ReloadPlan> {
    if meta.kind == SessionKind::Chat {
        return None;
    }
    Some(if !meta.claude {
        ReloadPlan::Shell
    } else if meta.claude_continue {
        ReloadPlan::ClaudeContinue
    } else {
        ReloadPlan::ClaudeFresh
    })
}

impl ReloadPlan {
    pub(super) fn text(self, lang: Lang) -> &'static str {
        match self {
            ReloadPlan::Shell => lang.t("Die Shell wird neu gestartet.", "The shell restarts."),
            ReloadPlan::ClaudeContinue => lang.t("Claude wird neu gestartet und setzt die Konversation fort.", "Claude restarts and continues the conversation."),
            ReloadPlan::ClaudeFresh => lang.t("Claude wird mit einer neuen Konversation gestartet.", "Claude restarts with a new conversation."),
        }
    }

    /// Tooltip des Knopfs in der Kopfzeile.
    pub(super) fn tip(self, lang: Lang) -> &'static str {
        match self {
            ReloadPlan::Shell => lang.t("Neu laden – Shell neu starten", "Reload – restart the shell"),
            ReloadPlan::ClaudeContinue => lang.t("Neu laden – Claude neu starten und die Konversation fortsetzen", "Reload – restart Claude and continue the conversation"),
            ReloadPlan::ClaudeFresh => lang.t("Neu laden – Claude mit neuer Konversation starten", "Reload – start Claude with a new conversation"),
        }
    }
}

/// Was ein Klick auf „Neu laden“ tut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReloadRoute {
    /// Chat: nichts (kein Prozess).
    Nothing,
    /// Prozess beendet: gleich neu starten.
    Now,
    /// Prozess läuft: erst fragen (ohne Wartezeit).
    Ask,
}

pub(super) fn reload_route(chat: bool, exited: bool) -> ReloadRoute {
    if chat {
        ReloadRoute::Nothing
    } else if exited {
        ReloadRoute::Now
    } else {
        ReloadRoute::Ask
    }
}

/// Offene Rückfrage „Neu laden?“ (je Fenster).
pub(super) struct ReloadAsk {
    pub(super) id: u64,
    /// Nach dem Speichern der Einstellungen gefragt („Jetzt neu laden?“, sonst beim nächsten Mal).
    pub(super) after_settings: bool,
}

/// Fehler im Einstellungsdialog (Speichern bleibt gesperrt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Problem {
    UnknownProfile,
    UnknownShell,
    MissingFolder,
}

impl Problem {
    pub(super) fn text(self, lang: Lang) -> &'static str {
        match self {
            Problem::UnknownProfile => lang.t("Das Profil gibt es nicht mehr.", "The profile no longer exists."),
            Problem::UnknownShell => lang.t("Diese Shell wurde nicht gefunden.", "This shell was not found."),
            Problem::MissingFolder => lang.t("Den Arbeitsordner gibt es nicht.", "The working folder does not exist."),
        }
    }
}

/// Tags aus einer Eingabe „a, #b, c“ (wie „Tags …“ im Kontextmenü).
pub(super) fn parse_tags(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().trim_start_matches('#').trim().to_string()).filter(|t| !t.is_empty()).collect()
}

/// Shell-Kennung für eine Sitzung mit diesem Profil (wie beim Anlegen, `App::new_session`): ein Profil mit eigenem
/// Programm bestimmt sie (`custom:<id>`), sonst die gewählte Shell. Eine übrig gebliebene `custom:`-Kennung (Profil
/// gewechselt) fällt auf die Shell des Profils bzw. die Standard-Shell zurück.
pub(super) fn resolve_shell(profile: Option<&Profile>, shell_id: &str, default_shell: &str) -> String {
    match profile {
        Some(p) if !p.program.trim().is_empty() => format!("custom:{}", p.id),
        _ if shell_id.starts_with("custom:") => match profile {
            Some(p) if !p.shell_id.is_empty() => p.shell_id.clone(),
            _ => default_shell.to_string(),
        },
        _ => shell_id.to_string(),
    }
}

/// Inhalt des Dialogs „Einstellungen der Sitzung“.
pub(super) struct SessionForm {
    pub(super) id: u64,
    pub(super) chat: bool,
    /// Modell eines Chats (nur zur Anzeige).
    pub(super) chat_model: String,
    /// Eigener Name; leer = automatisch (folgt dem Terminal-Titel).
    pub(super) name: String,
    /// Aktueller automatischer Name (Platzhalter des Namensfelds).
    pub(super) auto_name: String,
    pub(super) color: [u8; 3],
    pub(super) group: String,
    pub(super) tags: String,
    pub(super) notes: String,
    pub(super) profile: u64,
    pub(super) shell_id: String,
    pub(super) cwd: String,
    pub(super) claude: bool,
    pub(super) claude_continue: bool,
    /// Arbeitsordner beim Öffnen (nur ein geänderter Ordner muss existieren).
    orig_cwd: String,
    pub(super) problems: Vec<Problem>,
    /// Beim ersten Bild den Fokus ins Namensfeld (sonst tippte man ins Terminal dahinter).
    pub(super) focus: bool,
}

impl SessionForm {
    pub(super) fn from_meta(meta: &SessionMeta, auto_name: String) -> Self {
        SessionForm {
            id: meta.id,
            chat: meta.kind == SessionKind::Chat,
            chat_model: meta.chat.as_ref().map(|c| c.model.clone()).unwrap_or_default(),
            name: if meta.custom_name { meta.name.clone() } else { String::new() },
            auto_name,
            color: meta.color,
            group: meta.group.clone(),
            tags: meta.tags.join(", "),
            notes: meta.notes.clone(),
            profile: meta.profile,
            shell_id: meta.shell_id.clone(),
            cwd: meta.cwd.clone(),
            claude: meta.claude,
            claude_continue: meta.claude_continue,
            orig_cwd: meta.cwd.clone(),
            problems: Vec::new(),
            focus: true,
        }
    }

    /// Profil gewählt: Shell und Ordner wie im Dialog „Neue Sitzung“ übernehmen.
    pub(super) fn pick_profile(&mut self, p: Option<&Profile>) {
        match p {
            Some(p) => {
                self.profile = p.id;
                if !p.program.trim().is_empty() {
                    self.shell_id = format!("custom:{}", p.id);
                } else if !p.shell_id.is_empty() {
                    self.shell_id = p.shell_id.clone();
                }
                if !p.cwd.is_empty() {
                    self.cwd = p.cwd.clone();
                }
            }
            None => self.profile = 0,
        }
    }

    /// Prüfen wie beim Anlegen: Profil und Shell müssen bekannt sein, ein geänderter Arbeitsordner muss existieren
    /// (wie bei `new_session` über MCP). Chats haben nichts davon.
    pub(super) fn validate(&self, shells: &[ShellSpec], profiles: &[Profile], default_shell: &str, is_dir: impl Fn(&str) -> bool) -> Vec<Problem> {
        let mut v = Vec::new();
        if self.chat {
            return v;
        }
        let prof = profiles.iter().find(|p| p.id == self.profile);
        if self.profile != 0 && prof.is_none() {
            v.push(Problem::UnknownProfile);
        }
        let shell = resolve_shell(prof, &self.shell_id, default_shell);
        if !shell.starts_with("custom:") && !shell.is_empty() && !shells.is_empty() && !shells.iter().any(|s| s.id == shell) {
            v.push(Problem::UnknownShell);
        }
        let cwd = self.cwd.trim();
        if !cwd.is_empty() && cwd != self.orig_cwd.trim() && !is_dir(cwd) {
            v.push(Problem::MissingFolder);
        }
        v
    }

    /// Ändern sich Dinge, die erst beim (Neu-)Start wirken?
    pub(super) fn needs_reload(&self, meta: &SessionMeta, profiles: &[Profile], default_shell: &str) -> bool {
        if self.chat {
            return false;
        }
        let shell = resolve_shell(profiles.iter().find(|p| p.id == self.profile), &self.shell_id, default_shell);
        shell != meta.shell_id || self.profile != meta.profile || self.cwd.trim() != meta.cwd || self.claude != meta.claude
    }

    /// Übernehmen. Der Ordner im Baum wird bereinigt (`clean_folder_path`); dass er im Projekt existiert, sorgt der
    /// Aufrufer (`App::set_group`). Gibt zurück, ob ein Neu laden nötig ist.
    pub(super) fn apply(&self, meta: &mut SessionMeta, profiles: &[Profile], default_shell: &str) -> bool {
        let reload = self.needs_reload(meta, profiles, default_shell);
        let name = self.name.trim();
        if name.is_empty() {
            meta.custom_name = false;
        } else {
            meta.name = name.to_string();
            meta.custom_name = true;
        }
        meta.color = self.color;
        meta.group = state::clean_folder_path(&self.group);
        meta.tags = parse_tags(&self.tags);
        meta.notes = self.notes.clone();
        if !self.chat {
            meta.shell_id = resolve_shell(profiles.iter().find(|p| p.id == self.profile), &self.shell_id, default_shell);
            meta.profile = self.profile;
            meta.cwd = self.cwd.trim().to_string();
            meta.claude = self.claude;
            meta.claude_continue = self.claude_continue;
        }
        reload
    }
}

/// Feld „Arbeitsordner“: Text und Auswahl-Knopf. Eigene Funktion, damit hier später die WSL-fähige Ordnerauswahl (#172)
/// eingesetzt werden kann. `shell_id` = gewählte Shell (für eine solche Auswahl).
pub(super) fn folder_field(ui: &mut Ui, cwd: &mut String, _shell_id: &str, lang: Lang) {
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(cwd).desired_width(290.0).hint_text(lang.t("leer = Benutzerordner", "empty = home folder")));
        if ui.button("…").on_hover_text(lang.t("Ordner wählen", "Choose folder")).clicked() {
            if let Some(p) = notify::pick_folder(cwd) {
                *cwd = p;
            }
        }
    });
}

/// Farbauswahl (die Farben des Kontextmenüs „Farbe“); die gewählte ist umrandet.
fn color_row(ui: &mut Ui, color: &mut [u8; 3]) {
    ui.horizontal(|ui| {
        for c in PALETTE {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
            ui.painter().rect_filled(r, 4.0, Color32::from_rgb(c[0], c[1], c[2]));
            if *color == c {
                ui.painter().rect_stroke(r.expand(2.0), 5.0, Stroke::new(1.5_f32, ui.visuals().strong_text_color()), StrokeKind::Outside);
            }
            if resp.clicked() {
                *color = c;
            }
        }
    });
}

/// Lage eines Elements für die Tests ohne Fenster merken (sonst nichts).
fn mark(ui: &Ui, key: &'static str, r: &egui::Response) {
    #[cfg(test)]
    ui.ctx().data_mut(|d| d.insert_temp(Id::new(("sset_mark", key)), r.rect));
    #[cfg(not(test))]
    let _ = (ui, key, r);
}

/// Ergebnis eines Bildes des Einstellungsdialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FormOutcome {
    Open,
    Save,
    Cancel,
}

/// Der Dialog selbst (ohne App, für Tests ohne Fenster). `claude_mode` = Claude-Modus eingeschaltet.
pub(super) fn session_form_ui(ctx: &egui::Context, f: &mut SessionForm, shells: &[ShellSpec], profiles: &[Profile], claude_mode: bool, reload_hint: bool, lang: Lang) -> FormOutcome {
    let mut out = FormOutcome::Open;
    let esc = ctx.input(|i| i.key_pressed(Key::Escape));
    egui::Window::new(lang.t("Einstellungen der Sitzung", "Session settings"))
        .id(Id::new("session_settings"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Grid::new("sset_grid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                ui.label(lang.t("Name", "Name"));
                let auto = lang.t("automatisch", "automatic");
                let hint = if f.auto_name.is_empty() { auto.to_string() } else { format!("{} ({auto})", f.auto_name) };
                let te = ui.add(egui::TextEdit::singleline(&mut f.name).hint_text(hint).desired_width(320.0));
                if f.focus {
                    te.request_focus();
                    f.focus = false;
                }
                ui.end_row();
                ui.label(lang.t("Farbe", "Color"));
                color_row(ui, &mut f.color);
                ui.end_row();
                ui.label(lang.t("Ordner im Baum", "Tree folder"));
                ui.add(egui::TextEdit::singleline(&mut f.group).hint_text(lang.t("z. B. Backend/API (leer = keiner)", "e.g. Backend/API (empty = none)")).desired_width(320.0));
                ui.end_row();
                ui.label(lang.t("Tags", "Tags"));
                ui.add(egui::TextEdit::singleline(&mut f.tags).hint_text(lang.t("mit Komma getrennt", "comma separated")).desired_width(320.0));
                ui.end_row();
                ui.label(lang.t("Notizen", "Notes"));
                ui.add(egui::TextEdit::multiline(&mut f.notes).desired_rows(3).desired_width(320.0));
                ui.end_row();
                if f.chat {
                    return;
                }
                ui.separator();
                ui.separator();
                ui.end_row();
                let prog_profile = profiles.iter().find(|p| p.id == f.profile).is_some_and(|p| !p.program.trim().is_empty());
                if !profiles.is_empty() || f.profile != 0 {
                    ui.label(lang.t("Profil", "Profile"));
                    let cur = profiles.iter().find(|p| p.id == f.profile).map(|p| p.name.clone()).unwrap_or_else(|| lang.t("Keins", "None").to_string());
                    egui::ComboBox::from_id_salt("sset_prof").selected_text(cur).show_ui(ui, |ui| {
                        if ui.selectable_label(f.profile == 0, lang.t("Keins", "None")).clicked() {
                            f.pick_profile(None);
                        }
                        for p in profiles {
                            if ui.selectable_label(f.profile == p.id, &p.name).clicked() {
                                f.pick_profile(Some(p));
                            }
                        }
                    });
                    ui.end_row();
                }
                ui.label("Shell");
                if prog_profile {
                    ui.label(RichText::new(lang.t("eigenes Programm des Profils", "the profile's own program")).color(style::dim(ui)));
                } else {
                    let cur = shells.iter().find(|s| s.id == f.shell_id).map(|s| s.label.clone()).unwrap_or_else(|| f.shell_id.clone());
                    egui::ComboBox::from_id_salt("sset_shell").selected_text(cur).show_ui(ui, |ui| {
                        for s in shells {
                            ui.selectable_value(&mut f.shell_id, s.id.clone(), &s.label);
                        }
                    });
                }
                ui.end_row();
                ui.label(lang.t("Arbeitsordner", "Working folder"));
                folder_field(ui, &mut f.cwd, &f.shell_id.clone(), lang);
                ui.end_row();
                ui.label("");
                let prof = profiles.iter().find(|p| p.id == f.profile);
                let start_note = match prof {
                    Some(p) if !p.startup.trim().is_empty() || !p.env.trim().is_empty() => format!("{} {}", lang.t("Startbefehl und Umgebungsvariablen kommen aus dem Profil", "Start command and environment variables come from the profile"), lang.q(&p.name)),
                    _ => lang.t("Startbefehl und Umgebungsvariablen legt ein Profil fest (Einstellungen → Profile).", "A profile sets the start command and environment variables (Settings → Profiles).").to_string(),
                };
                ui.label(RichText::new(start_note).small().color(style::dim(ui)));
                ui.end_row();
                if claude_mode || f.claude {
                    ui.label("");
                    let r = ui.checkbox(&mut f.claude, lang.t("Claude Code starten", "Start Claude Code"));
                    mark(ui, "claude", &r);
                    ui.end_row();
                    if f.claude {
                        ui.label("");
                        let r = ui.checkbox(&mut f.claude_continue, lang.t("Konversation fortsetzen", "Continue the conversation")).on_hover_text(lang.t(
                            "Beim Neu laden, Neu starten und Programmstart setzt Claude die bisherige Konversation fort. Aus: jedes Mal eine neue Konversation.",
                            "On reload, restart and program start Claude continues the previous conversation. Off: a new conversation every time.",
                        ));
                        mark(ui, "continue", &r);
                        ui.end_row();
                        // Hier kommen die Claude-Optionen (Modell, Aufwand, Permission-Mode, Agent; #161) hinzu.
                    }
                }
            });
            if f.chat {
                ui.add_space(S1);
                ui.label(RichText::new(format!("{} {} – {}", lang.t("Chat mit", "Chat with"), f.chat_model, lang.t("lässt sich nicht in ein Terminal umwandeln.", "cannot be turned into a terminal."))).small().color(style::dim(ui)));
            }
            if reload_hint {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Profil, Shell, Arbeitsordner und Claude wirken erst nach einem Neu laden.", "Profile, shell, working folder and Claude take effect after a reload.")).small().color(style::dim(ui)));
            }
            for p in &f.problems {
                ui.colored_label(Color32::from_rgb(0xe0, 0x6c, 0x75), p.text(lang));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let save = ui.button(lang.t("Speichern", "Save"));
                mark(ui, "save", &save);
                if save.clicked() {
                    out = FormOutcome::Save;
                }
                let cancel = ui.button(lang.t("Abbrechen", "Cancel"));
                mark(ui, "cancel", &cancel);
                if cancel.clicked() {
                    out = FormOutcome::Cancel;
                }
            });
        });
    if esc && out == FormOutcome::Open {
        out = FormOutcome::Cancel;
    }
    out
}

impl App {
    /// „Neu laden“: Chats nie; ein beendeter Prozess startet gleich neu, ein laufender erst nach der Rückfrage.
    pub(super) fn request_reload(&mut self, ctx: &egui::Context, id: u64) {
        let Some(e) = self.entries.get(&id) else { return };
        match reload_route(e.is_chat(), e.exited()) {
            ReloadRoute::Nothing => {}
            ReloadRoute::Now => self.apply(ctx, Action::Restart(id)),
            ReloadRoute::Ask => self.reload_ask = Some(ReloadAsk { id, after_settings: false }),
        }
    }

    /// Rückfrage „Neu laden?“ (ohne Wartezeit).
    pub(super) fn reload_ask_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(ask) = self.reload_ask.take() else { return };
        let lang = self.settings.language;
        let Some(e) = self.entries.get(&ask.id) else { return };
        let Some(plan) = reload_plan(&e.meta) else { return };
        let name = e.display_name();
        let (title, button) = if ask.after_settings {
            (lang.t("Jetzt neu laden?", "Reload now?"), lang.t("Jetzt neu laden", "Reload now"))
        } else {
            (lang.t("Sitzung neu laden?", "Reload session?"), lang.t("Neu laden", "Reload"))
        };
        let running = !e.exited();
        let o = confirm_window(ctx, Id::new("reload_ask"), title, button, 0, lang, 420.0, |ui| {
            ui.label(RichText::new(name).strong());
            ui.add_space(S1);
            ui.label(plan.text(lang));
            if running {
                ui.label(lang.t("Der laufende Prozess wird beendet.", "The running process is terminated."));
            }
            if ask.after_settings {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Sonst gelten die Änderungen beim nächsten Neu laden.", "Otherwise the changes apply on the next reload.")).small().color(style::dim(ui)));
            }
        });
        match o {
            Outcome::Go => {
                actions.push(Action::Restart(ask.id));
                self.want_focus = Some(ask.id);
            }
            Outcome::Cancel => self.want_focus = Some(self.focused),
            Outcome::Open => self.reload_ask = Some(ask),
        }
    }

    pub(super) fn open_session_settings(&mut self, id: u64) {
        if let Some(e) = self.entries.get(&id) {
            let auto = if e.meta.custom_name { e.live.as_ref().and_then(|s| s.title()).unwrap_or_default() } else { e.display_name() };
            self.session_settings = Some(SessionForm::from_meta(&e.meta, auto));
        }
    }

    /// Dialog „Einstellungen der Sitzung“ (je Fenster).
    pub(super) fn session_settings_ui(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.session_settings.take() else { return };
        let Some(meta) = self.entries.get(&f.id).map(|e| e.meta.clone()) else { return }; // inzwischen geschlossen
        let lang = self.settings.language;
        let default_shell = self.settings.default_shell.clone();
        let hint = f.needs_reload(&meta, &self.settings.profiles, &default_shell);
        match session_form_ui(ctx, &mut f, &self.shells, &self.settings.profiles, self.settings.claude_mode, hint, lang) {
            FormOutcome::Open => self.session_settings = Some(f),
            FormOutcome::Cancel => self.want_focus = Some(self.focused),
            FormOutcome::Save => {
                f.problems = f.validate(&self.shells, &self.settings.profiles, &default_shell, |p| std::path::Path::new(p).is_dir());
                if !f.problems.is_empty() {
                    self.session_settings = Some(f);
                    return;
                }
                let id = f.id;
                let Some(e) = self.entries.get_mut(&id) else { return };
                let reload = f.apply(&mut e.meta, &self.settings.profiles, &default_shell);
                let group = e.meta.group.clone();
                self.set_group(id, group); // legt den Ordner im Projekt an
                if reload {
                    self.reload_ask = Some(ReloadAsk { id, after_settings: true });
                } else {
                    self.want_focus = Some(self.focused);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    fn uuid() -> String {
        "0123abcd-0000-4000-8000-00000000abcd".to_string()
    }

    fn claude_meta(started: bool, cont: bool) -> SessionMeta {
        SessionMeta { id: 1, claude: true, claude_started: started, claude_session: uuid(), claude_continue: cont, ..SessionMeta::default() }
    }

    #[test]
    fn reload_follows_the_session_settings() {
        // Terminal: Shell
        assert_eq!(reload_plan(&SessionMeta::default()), Some(ReloadPlan::Shell));
        // Claude mit „fortsetzen“ (Standard) bzw. ohne
        assert_eq!(reload_plan(&claude_meta(true, true)), Some(ReloadPlan::ClaudeContinue));
        assert_eq!(reload_plan(&claude_meta(true, false)), Some(ReloadPlan::ClaudeFresh));
        // Chat: kein Neu laden
        assert_eq!(reload_plan(&SessionMeta { kind: SessionKind::Chat, ..SessionMeta::default() }), None);
        // Texte in beiden Sprachen vorhanden und verschieden
        for p in [ReloadPlan::Shell, ReloadPlan::ClaudeContinue, ReloadPlan::ClaudeFresh] {
            assert_ne!(p.text(Lang::De), p.text(Lang::En));
            assert_ne!(p.tip(Lang::De), p.tip(Lang::En));
        }
    }

    #[test]
    fn a_running_process_asks_first_an_ended_one_restarts_at_once() {
        assert_eq!(reload_route(false, false), ReloadRoute::Ask);
        assert_eq!(reload_route(false, true), ReloadRoute::Now);
        assert_eq!(reload_route(true, false), ReloadRoute::Nothing);
        assert_eq!(reload_route(true, true), ReloadRoute::Nothing);
    }

    #[test]
    fn the_reload_question_has_no_countdown() {
        // Rückfrage mit 0 s: Enter bestätigt sofort, Esc bricht ab (kein Terminal bekommt die Tasten).
        let ctx = egui::Context::default();
        let key = |k| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let run = |t: f64, ev: Vec<Event>| {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(t), events: ev, ..Default::default() };
            let mut o = Outcome::Open;
            let _ = ctx.run(input, |ctx| {
                o = confirm_window(ctx, Id::new("reload_ask"), "Reload session?", "Reload", 0, Lang::En, 420.0, |ui| {
                    ui.label(ReloadPlan::ClaudeFresh.text(Lang::En));
                });
            });
            o
        };
        assert_eq!(run(0.0, vec![]), Outcome::Open);
        assert_eq!(run(0.1, vec![key(Key::Enter)]), Outcome::Go, "ohne Wartezeit");
        assert_eq!(run(0.2, vec![key(Key::Escape)]), Outcome::Cancel);
    }

    #[test]
    fn restart_resumes_only_with_continue_on() {
        // Neu laden (restart): fortsetzen nur mit der Option der Sitzung
        let w = resume_wanted(false, true, true, false);
        assert!(w);
        assert_eq!(claude_start(None, w, false, &claude_meta(true, true), false), ClaudeStart::Resume(uuid()));
        let w = resume_wanted(false, true, false, true);
        assert!(!w);
        assert_eq!(claude_start(None, w, false, &claude_meta(true, false), false), ClaudeStart::Fresh);
        // Noch kein Prompt gesendet: nichts fortzusetzen → neu (Windows) bzw. --continue (WSL, keine ID aus den Hooks)
        assert_eq!(claude_start(None, true, false, &claude_meta(false, true), false), ClaudeStart::Fresh);
        assert_eq!(claude_start(None, true, false, &claude_meta(false, true), true), ClaudeStart::Continue);
        assert_eq!(claude_start(None, false, false, &claude_meta(false, false), true), ClaudeStart::Fresh, "WSL ohne fortsetzen: neu");
        // Programmstart (restore): globale Einstellung und Option der Sitzung
        assert!(resume_wanted(true, false, true, true));
        assert!(!resume_wanted(true, false, true, false));
        assert!(!resume_wanted(true, false, false, true));
        // Neue Sitzung: weder restore noch restart
        assert!(!resume_wanted(false, false, true, true));
        // Ausdrückliche Konversation (Verlauf) und --continue beim Anlegen gehen immer vor
        let other = "ffffffff-0000-4000-8000-000000000000";
        assert_eq!(claude_start(Some(other), false, false, &claude_meta(true, false), false), ClaudeStart::Resume(other.into()));
        assert_eq!(claude_start(Some("not-a-uuid"), false, true, &claude_meta(true, false), false), ClaudeStart::Continue);
        // Kaputte gespeicherte ID wird nicht übernommen
        let bad = SessionMeta { claude_session: "x; rm -rf".into(), ..claude_meta(true, true) };
        assert_eq!(claude_start(None, true, false, &bad, false), ClaudeStart::Fresh);
    }

    fn profiles() -> Vec<Profile> {
        vec![
            Profile { id: 1, name: "Prog".into(), program: "C:\\tools\\nu.exe".into(), ..Profile::default() },
            Profile { id: 2, name: "Bash".into(), shell_id: "bash".into(), cwd: "C:\\work".into(), startup: "echo hi".into(), ..Profile::default() },
        ]
    }

    fn shells() -> Vec<ShellSpec> {
        ["pwsh", "bash", "cmd"].iter().map(|s| ShellSpec { id: s.to_string(), label: s.to_string(), program: format!("{s}.exe"), args: vec![] }).collect()
    }

    fn form(meta: &SessionMeta) -> SessionForm {
        SessionForm::from_meta(meta, "auto".into())
    }

    #[test]
    fn shell_follows_the_profile_like_on_creation() {
        let p = profiles();
        assert_eq!(resolve_shell(Some(&p[0]), "pwsh", "cmd"), "custom:1", "eigenes Programm");
        assert_eq!(resolve_shell(Some(&p[1]), "bash", "cmd"), "bash");
        assert_eq!(resolve_shell(None, "pwsh", "cmd"), "pwsh");
        // Profil mit Programm abgewählt: übrig gebliebene custom-Kennung → Shell des Profils bzw. Standard
        assert_eq!(resolve_shell(Some(&p[1]), "custom:1", "cmd"), "bash");
        assert_eq!(resolve_shell(None, "custom:1", "cmd"), "cmd");
        // pick_profile übernimmt Shell und Ordner
        let mut f = form(&SessionMeta { shell_id: "pwsh".into(), ..SessionMeta::default() });
        f.pick_profile(Some(&p[1]));
        assert_eq!((f.profile, f.shell_id.as_str(), f.cwd.as_str()), (2, "bash", "C:\\work"));
        f.pick_profile(Some(&p[0]));
        assert_eq!(f.shell_id, "custom:1");
        f.pick_profile(None);
        assert_eq!(f.profile, 0);
    }

    #[test]
    fn validation_like_the_new_session_dialog() {
        let (p, s) = (profiles(), shells());
        let meta = SessionMeta { shell_id: "pwsh".into(), cwd: "C:\\gone".into(), ..SessionMeta::default() };
        let dir = |d: &str| d == "C:\\ok";
        let mut f = form(&meta);
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty(), "unveränderter (auch fehlender) Ordner sperrt nicht");
        f.cwd = "C:\\missing".into();
        assert_eq!(f.validate(&s, &p, "pwsh", dir), vec![Problem::MissingFolder]);
        f.cwd = "C:\\ok".into();
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty());
        f.cwd = "  ".into();
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty(), "leer = Benutzerordner");
        f.shell_id = "fish".into();
        assert_eq!(f.validate(&s, &p, "pwsh", dir), vec![Problem::UnknownShell]);
        f.shell_id = "pwsh".into();
        f.profile = 9;
        assert_eq!(f.validate(&s, &p, "pwsh", dir), vec![Problem::UnknownProfile]);
        f.profile = 1; // Profil mit Programm: Shell egal
        f.shell_id = "fish".into();
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty());
        // Chat: nichts davon
        let mut c = form(&SessionMeta { kind: SessionKind::Chat, ..SessionMeta::default() });
        c.profile = 9;
        c.cwd = "C:\\missing".into();
        assert!(c.validate(&s, &p, "pwsh", dir).is_empty());
        for pr in [Problem::UnknownProfile, Problem::UnknownShell, Problem::MissingFolder] {
            assert_ne!(pr.text(Lang::De), pr.text(Lang::En));
        }
    }

    #[test]
    fn apply_changes_and_tells_when_a_reload_is_needed() {
        let p = profiles();
        let mut meta = SessionMeta { id: 3, name: "x".into(), shell_id: "pwsh".into(), cwd: "C:\\a".into(), tags: vec!["t".into()], ..SessionMeta::default() };
        // Nur Name, Farbe, Ordner, Tags, Notizen: kein Neu laden
        let mut f = form(&meta);
        assert_eq!(f.name, "", "automatischer Name → leeres Feld");
        assert_eq!(f.tags, "t");
        f.name = "  Build ".into();
        f.color = PALETTE[2];
        f.group = " a / b ".into();
        f.tags = "x, #y,, z ".into();
        f.notes = "n".into();
        assert!(!f.apply(&mut meta, &p, "pwsh"));
        assert_eq!((meta.name.as_str(), meta.custom_name), ("Build", true));
        assert_eq!(meta.color, PALETTE[2]);
        assert_eq!(meta.group, "a/b");
        assert_eq!(meta.tags, vec!["x", "y", "z"]);
        assert_eq!(meta.notes, "n");
        // „Konversation fortsetzen“ allein: wirkt beim nächsten Neu laden, keine Rückfrage
        let mut f = form(&meta);
        f.claude_continue = false;
        assert!(!f.apply(&mut meta, &p, "pwsh"));
        assert!(!meta.claude_continue);
        // Name leeren → wieder automatisch
        let mut f = form(&meta);
        f.name.clear();
        f.apply(&mut meta, &p, "pwsh");
        assert!(!meta.custom_name);
        // Claude an, Ordner, Shell, Profil: Neu laden nötig
        for change in 0..4 {
            let mut m = meta.clone();
            let mut f = form(&m);
            match change {
                0 => f.claude = true,
                1 => f.cwd = "C:\\b".into(),
                2 => f.shell_id = "bash".into(),
                _ => f.pick_profile(Some(&p[0])),
            }
            assert!(f.needs_reload(&m, &p, "pwsh"), "Änderung {change}");
            assert!(f.apply(&mut m, &p, "pwsh"), "Änderung {change}");
        }
        let mut m = meta.clone();
        let mut f = form(&m);
        f.pick_profile(Some(&p[0]));
        f.apply(&mut m, &p, "pwsh");
        assert_eq!((m.profile, m.shell_id.as_str()), (1, "custom:1"));
        // Claude aus einer Claude-Sitzung → Terminal (und zurück)
        let mut m = claude_meta(true, true);
        let mut f = form(&m);
        f.claude = false;
        assert!(f.apply(&mut m, &p, "pwsh"));
        assert!(!m.claude);
    }

    #[test]
    fn chats_stay_chats() {
        let p = profiles();
        let mut meta = SessionMeta { id: 5, kind: SessionKind::Chat, chat: Some(state::ChatMeta { server: "s".into(), model: "m".into() }), ..SessionMeta::default() };
        let mut f = form(&meta);
        assert!(f.chat);
        assert_eq!(f.chat_model, "m");
        f.claude = true;
        f.shell_id = "pwsh".into();
        f.cwd = "C:\\x".into();
        f.profile = 2;
        f.name = "Ask".into();
        assert!(!f.apply(&mut meta, &p, "pwsh"), "ein Chat wird nie neu geladen");
        assert_eq!(meta.kind, SessionKind::Chat);
        assert!(!meta.claude && meta.shell_id.is_empty() && meta.cwd.is_empty() && meta.profile == 0);
        assert_eq!(meta.name, "Ask");
    }

    /// Dialog ohne Fenster: einige Bilder mit Ereignissen.
    struct Dlg {
        ctx: egui::Context,
        t: f64,
    }

    impl Dlg {
        fn frame(&mut self, f: &mut SessionForm, claude_mode: bool, events: Vec<Event>) -> FormOutcome {
            self.t += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))), time: Some(self.t), events, ..Default::default() };
            let mut out = FormOutcome::Open;
            let (s, p) = (shells(), profiles());
            let _ = self.ctx.run(input, |ctx| {
                out = session_form_ui(ctx, f, &s, &p, claude_mode, false, Lang::En);
            });
            out
        }
        fn click(&mut self, f: &mut SessionForm, claude_mode: bool, p: Pos2) -> FormOutcome {
            let b = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            self.frame(f, claude_mode, vec![Event::PointerMoved(p)]);
            self.frame(f, claude_mode, vec![b(true)]);
            self.frame(f, claude_mode, vec![b(false)])
        }
        /// Rechteck eines markierten Elements (`mark`) aus dem letzten Bild.
        fn find(&self, key: &'static str) -> Option<Rect> {
            self.ctx.data(|d| d.get_temp::<Rect>(Id::new(("sset_mark", key))))
        }
    }

    #[test]
    fn dialog_saves_cancels_and_shows_claude_rows_only_for_terminals() {
        let mut d = Dlg { ctx: egui::Context::default(), t: 0.0 };
        let mut f = form(&SessionMeta { id: 1, claude: true, shell_id: "pwsh".into(), ..SessionMeta::default() });
        assert_eq!(d.frame(&mut f, true, vec![]), FormOutcome::Open);
        assert!(!f.focus, "Fokus ging beim ersten Bild ins Namensfeld");
        d.frame(&mut f, true, vec![]);
        assert!(d.find("continue").is_some(), "Claude-Sitzung: Option sichtbar");
        assert!(d.find("claude").is_some());
        // Häkchen klicken schaltet die Option
        let r = d.find("continue").unwrap();
        d.click(&mut f, true, r.center());
        assert!(!f.claude_continue);
        // Speichern
        let r = d.find("save").unwrap();
        assert_eq!(d.click(&mut f, true, r.center()), FormOutcome::Save);
        // Esc bricht ab
        let esc = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        assert_eq!(d.frame(&mut f, true, vec![esc]), FormOutcome::Cancel);
        // Chat: keine Claude-, Shell- oder Ordnerzeilen
        let mut d = Dlg { ctx: egui::Context::default(), t: 0.0 };
        let mut c = form(&SessionMeta { id: 2, kind: SessionKind::Chat, ..SessionMeta::default() });
        d.frame(&mut c, true, vec![]);
        d.frame(&mut c, true, vec![]);
        assert!(d.find("claude").is_none());
        assert!(d.find("continue").is_none());
        assert!(d.find("save").is_some());
        // Terminal ohne Claude-Modus: kein Claude-Häkchen
        let mut d = Dlg { ctx: egui::Context::default(), t: 0.0 };
        let mut t = form(&SessionMeta { id: 3, shell_id: "pwsh".into(), ..SessionMeta::default() });
        d.frame(&mut t, false, vec![]);
        d.frame(&mut t, false, vec![]);
        assert!(d.find("claude").is_none());
        // Abbrechen-Knopf
        let r = d.find("cancel").unwrap();
        assert_eq!(d.click(&mut t, false, r.center()), FormOutcome::Cancel);
    }
}
