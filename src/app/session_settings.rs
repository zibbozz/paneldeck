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

use super::closing::{confirm_window_keep, Outcome};
use crate::claude_opts::ClaudeOpts;
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

/// `lingering`: im Job der Sitzung laufen Hintergrundprozesse, die das Neu laden überleben würden (#168).
pub(super) fn reload_route(chat: bool, exited: bool, lingering: bool) -> ReloadRoute {
    if chat {
        ReloadRoute::Nothing
    } else if exited && !lingering {
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
    /// Aus dem Fenster einer abgedockten Sitzung: die Rückfrage erscheint dort.
    pub(super) detached: bool,
    /// Hintergrundprozesse im Job der Sitzung beim Öffnen (#168): dann „Weiterlaufen lassen“ oder „Alle beenden“.
    pub(super) lingering: Vec<super::closing::Lingering>,
}

/// Fehler im Einstellungsdialog (Speichern bleibt gesperrt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Problem {
    UnknownProfile,
    UnknownShell,
    MissingFolder,
    InvalidClaudeOption,
}

impl Problem {
    pub(super) fn text(self, lang: Lang) -> &'static str {
        match self {
            Problem::UnknownProfile => lang.t("Das Profil gibt es nicht mehr.", "The profile no longer exists."),
            Problem::UnknownShell => lang.t("Diese Shell wurde nicht gefunden.", "This shell was not found."),
            Problem::MissingFolder => lang.t("Den Arbeitsordner gibt es nicht.", "The working folder does not exist."),
            Problem::InvalidClaudeOption => lang.t("Ungültige Claude-Option (Modell oder Agent).", "Invalid Claude option (model or agent)."),
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

/// Was der Dialog ändern kann – einmal als Stand beim Öffnen, einmal als Eingabe.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FormValues {
    /// Eigener Name; leer = automatisch (folgt dem Terminal-Titel).
    pub(super) name: String,
    pub(super) color: [u8; 3],
    pub(super) group: String,
    pub(super) tags: String,
    pub(super) notes: String,
    pub(super) profile: u64,
    pub(super) shell_id: String,
    pub(super) cwd: String,
    pub(super) claude: bool,
    pub(super) claude_continue: bool,
    /// Claude-Optionen (#161): wirksame Werte der Sitzung (auch alte aus `claude_args`).
    pub(super) copts: ClaudeOpts,
}

impl FormValues {
    fn of(meta: &SessionMeta) -> Self {
        FormValues {
            name: if meta.custom_name { meta.name.clone() } else { String::new() },
            color: meta.color,
            group: meta.group.clone(),
            tags: meta.tags.join(", "),
            notes: meta.notes.clone(),
            profile: meta.profile,
            shell_id: meta.shell_id.clone(),
            cwd: meta.cwd.clone(),
            claude: meta.claude,
            claude_continue: meta.claude_continue,
            copts: meta.claude_opts_eff().0,
        }
    }
}

/// Inhalt des Dialogs „Einstellungen der Sitzung“.
pub(super) struct SessionForm {
    pub(super) id: u64,
    pub(super) chat: bool,
    /// Modell eines Chats (nur zur Anzeige).
    pub(super) chat_model: String,
    /// Aktueller automatischer Name (Platzhalter des Namensfelds).
    pub(super) auto_name: String,
    /// Ordner des eigenen Git-Worktrees: dann ist der Arbeitsordner fest.
    pub(super) worktree: Option<String>,
    /// Eingabe.
    pub(super) v: FormValues,
    /// Stand beim Öffnen: Gespeichert wird nur, was der Nutzer hier geändert hat (andere Änderungen in der
    /// Zwischenzeit – Notizen-Fenster, MCP `rename_session`, Farbe im Kontextmenü – bleiben erhalten).
    orig: FormValues,
    pub(super) problems: Vec<Problem>,
    /// Modell- und Agentenname gültig (Ergebnis von `claude_opts_rows` im letzten Bild).
    pub(super) opts_ok: bool,
    /// Beim ersten Bild den Fokus ins Namensfeld (sonst tippte man ins Terminal dahinter).
    pub(super) focus: bool,
    /// Wofür der Arbeitsordner gilt (Windows oder WSL der gewählten Shell); setzt die App vor jedem Bild (#216).
    pub(super) target: folder_picker::Target,
    /// Ordnerfeld mit WSL-Unterstützung (#172, #216), beim ersten Bild angelegt.
    picker: Option<folder_picker::FolderPicker>,
    /// Ausgangslage des Ordnerfelds: (angezeigter Wert, Arbeitsordner) beim Anlegen bzw. nach einer Änderung von außen.
    /// Erst wenn sich die Anzeige ändert (Eingabe, anderer Modus, andere Shell), wird sie zum Arbeitsordner – eine bloß
    /// umgerechnete Anzeige (`C:\x` als `/mnt/c/x`) ändert nichts.
    picker_base: (String, String),
    /// Arbeitsordner nach dem letzten Bild: Ändert er sich von außen (Profil gewählt), übernimmt das Feld ihn.
    picker_cwd: String,
    /// „Zuletzt benutzte“ WSL-Ordner (`Settings::wsl_recent`, beim Öffnen kopiert).
    pub(super) wsl_recent: std::collections::BTreeMap<String, Vec<String>>,
}

impl SessionForm {
    pub(super) fn from_meta(meta: &SessionMeta, auto_name: String) -> Self {
        let v = FormValues::of(meta);
        SessionForm {
            id: meta.id,
            chat: meta.kind == SessionKind::Chat,
            chat_model: meta.chat.as_ref().map(|c| c.model.clone()).unwrap_or_default(),
            auto_name,
            worktree: meta.worktree.as_ref().map(|w| w.path.clone()),
            orig: v.clone(),
            v,
            problems: Vec::new(),
            opts_ok: true,
            focus: true,
            target: folder_picker::Target::Windows,
            picker: None,
            picker_base: (String::new(), String::new()),
            picker_cwd: String::new(),
            wsl_recent: Default::default(),
        }
    }

    /// Grund, warum das Ordnerfeld den Wert nicht übernehmen lässt (ungültiger oder fehlender Linux-Ordner, andere
    /// Distribution; Prüfung in WSL im Hintergrund), sonst None.
    pub(super) fn folder_blocked(&self, lang: Lang) -> Option<String> {
        self.picker.as_ref().filter(|_| self.worktree.is_none()).and_then(|p| p.blocking_error(lang))
    }

    /// Profil gewählt: Shell und Ordner wie im Dialog „Neue Sitzung“ übernehmen (den Ordner nicht bei einem Worktree).
    pub(super) fn pick_profile(&mut self, p: Option<&Profile>) {
        match p {
            Some(p) => {
                self.v.profile = p.id;
                if !p.program.trim().is_empty() {
                    self.v.shell_id = format!("custom:{}", p.id);
                } else if !p.shell_id.is_empty() {
                    self.v.shell_id = p.shell_id.clone();
                }
                if !p.cwd.is_empty() && self.worktree.is_none() {
                    self.v.cwd = p.cwd.clone();
                }
            }
            None => self.v.profile = 0,
        }
    }

    fn shell_of(v: &FormValues, profiles: &[Profile], default_shell: &str) -> String {
        resolve_shell(profiles.iter().find(|p| p.id == v.profile), &v.shell_id, default_shell)
    }

    /// Prüfen wie beim Anlegen, aber nur Geändertes (ein inzwischen fehlendes Profil oder ein gelöschter Ordner sperrt
    /// das Speichern anderer Änderungen nicht): Profil und Shell müssen bekannt sein, ein Arbeitsordner muss existieren
    /// (wie bei `new_session` über MCP). `is_dir(shell, path)` kennt die Shell, damit WSL-Pfade (#172) prüfbar sind.
    /// Chats haben nichts davon.
    pub(super) fn validate(&self, shells: &[ShellSpec], profiles: &[Profile], default_shell: &str, is_dir: impl Fn(&str, &str) -> bool) -> Vec<Problem> {
        let mut v = Vec::new();
        if self.chat {
            return v;
        }
        if self.v.profile != self.orig.profile && self.v.profile != 0 && !profiles.iter().any(|p| p.id == self.v.profile) {
            v.push(Problem::UnknownProfile);
        }
        let shell = Self::shell_of(&self.v, profiles, default_shell);
        let shell_changed = shell != Self::shell_of(&self.orig, profiles, default_shell);
        if shell_changed && !shell.starts_with("custom:") && !shell.is_empty() && !shells.is_empty() && !shells.iter().any(|s| s.id == shell) {
            v.push(Problem::UnknownShell);
        }
        let cwd = self.v.cwd.trim();
        if !cwd.is_empty() && cwd != self.orig.cwd.trim() && !is_dir(&shell, cwd) {
            v.push(Problem::MissingFolder);
        }
        if self.v.claude && self.v.copts != self.orig.copts && (!self.opts_ok || self.v.copts.sanitized(true) != self.v.copts.trimmed()) {
            v.push(Problem::InvalidClaudeOption);
        }
        v
    }

    /// Hat der Nutzer etwas geändert, das erst beim (Neu-)Start wirkt?
    pub(super) fn needs_reload(&self, profiles: &[Profile], default_shell: &str) -> bool {
        if self.chat {
            return false;
        }
        let (v, o) = (&self.v, &self.orig);
        v.profile != o.profile
            || Self::shell_of(v, profiles, default_shell) != Self::shell_of(o, profiles, default_shell)
            || v.cwd.trim() != o.cwd.trim()
            || v.claude != o.claude
            || (v.claude && v.copts != o.copts)
    }

    /// Übernehmen – nur die Felder, die der Nutzer im Dialog geändert hat. Der Ordner im Baum wird bereinigt
    /// (`clean_folder_path`); dass er im Projekt existiert, sorgt der Aufrufer (`App::set_group`). Gibt zurück, ob ein
    /// Neu laden nötig ist.
    pub(super) fn apply(&self, meta: &mut SessionMeta, profiles: &[Profile], default_shell: &str) -> bool {
        let reload = self.needs_reload(profiles, default_shell);
        let (v, o) = (&self.v, &self.orig);
        if v.name.trim() != o.name.trim() {
            let name = v.name.trim();
            if name.is_empty() {
                meta.custom_name = false;
            } else {
                meta.name = name.to_string();
                meta.custom_name = true;
            }
        }
        if v.color != o.color {
            meta.color = v.color;
        }
        if v.group != o.group {
            meta.group = state::clean_folder_path(&v.group);
        }
        if v.tags != o.tags {
            meta.tags = parse_tags(&v.tags);
        }
        if v.notes != o.notes {
            meta.notes = v.notes.clone();
        }
        if !self.chat {
            if v.profile != o.profile || v.shell_id != o.shell_id {
                meta.profile = v.profile;
                meta.shell_id = Self::shell_of(v, profiles, default_shell);
            }
            if v.cwd.trim() != o.cwd.trim() && self.worktree.is_none() {
                meta.cwd = v.cwd.trim().to_string();
            }
            if v.claude != o.claude {
                meta.claude = v.claude;
            }
            if v.claude_continue != o.claude_continue {
                meta.claude_continue = v.claude_continue;
            }
            if v.copts != o.copts {
                // Nur für diese Sitzung (auch bypassPermissions als ausdrückliche Wahl); nie als Vorauswahl gemerkt.
                // Alte Angaben aus `claude_args` fallen weg, sonst kämen geleerte Felder von dort zurück.
                let (_, rest) = meta.claude_opts_eff();
                meta.claude_opts = v.copts.sanitized(true);
                meta.claude_args = rest;
            }
        }
        reload
    }
}

/// Feld „Arbeitsordner“: Text und Auswahl-Knopf. Eigene Funktion, damit hier die WSL-fähige Ordnerauswahl (#172)
/// eingesetzt werden kann; `shell_id` = gewählte Shell (bestimmt, welche Pfade gelten). `locked` = Ordner eines
/// Git-Worktrees (nicht änderbar).
pub(super) fn folder_field(ui: &mut Ui, cwd: &mut String, _shell_id: &str, locked: bool, lang: Lang) {
    ui.horizontal(|ui| {
        ui.add_enabled(!locked, egui::TextEdit::singleline(cwd).desired_width(290.0).hint_text(lang.t("leer = Benutzerordner", "empty = home folder")));
        if ui.add_enabled(!locked, egui::Button::new("…")).on_hover_text(lang.t("Ordner wählen", "Choose folder")).clicked() {
            if let Some(p) = notify::pick_folder(cwd) {
                *cwd = p;
            }
        }
    });
}

/// Gibt es den Arbeitsordner (`wsl` = die Shell startet in WSL)? Nie blockierend im UI-Thread (#216): Ein Linux-Pfad
/// einer WSL-Shell prüft das Ordnerfeld im Hintergrund (`SessionForm::folder_blocked`), ein `\\wsl$`-Pfad wird nie
/// geprüft, ein Netzwerkpfad nur mit kurzer Frist (keine Antwort = kein Einwand). Sonst der Windows-Ordner.
pub(super) fn folder_exists(wsl: bool, path: &str) -> bool {
    let p = path.trim();
    if wsl && crate::wsl::looks_linux(p) {
        return true;
    }
    crate::pathfmt::dir_exists_quick(p, std::time::Duration::from_millis(400)) != Some(false)
}

/// Farbauswahl: die geltende Palette des Terminal-Hubs wie im Kontextmenü „Farbe“ (die gewählte ist umrandet) und
/// „Eigene Farbe …“ (klappt im Dialog auf; die Farbe kommt nicht in die Palette).
fn color_row(ui: &mut Ui, color: &mut [u8; 3], lang: Lang) {
    ui.vertical(|ui| {
        let cur = crate::palette::hex_of(*color);
        if let Some(h) = palette_ui::pick(ui, "sset_color", crate::palette::Area::Hub, Some(&cur), palette_ui::PickOpts::HUB, lang) {
            *color = crate::palette::rgb(&h);
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

/// Der Dialog selbst (ohne App, für Tests ohne Fenster). `claude_mode` = Claude-Modus eingeschaltet, `esc_ok` = Esc
/// darf den Dialog schließen (nicht, solange darüber eine Rückfrage offen ist). Esc wird verbraucht (kein Terminal und
/// kein zweiter Dialog bekommt es) und gilt nicht, solange eine Auswahlliste offen ist (die schließt Esc zuerst).
#[allow(clippy::too_many_arguments)]
pub(super) fn session_form_ui(ctx: &egui::Context, f: &mut SessionForm, shells: &[ShellSpec], profiles: &[Profile], claude_mode: bool, reload_hint: bool, esc_ok: bool, lang: Lang) -> FormOutcome {
    let mut out = FormOutcome::Open;
    let esc = esc_ok && !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
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
                let te = ui.add(egui::TextEdit::singleline(&mut f.v.name).hint_text(hint).desired_width(320.0));
                if f.focus {
                    te.request_focus();
                    f.focus = false;
                }
                ui.end_row();
                ui.label(lang.t("Farbe", "Color"));
                color_row(ui, &mut f.v.color, lang);
                ui.end_row();
                ui.label(lang.t("Ordner im Baum", "Tree folder"));
                ui.add(egui::TextEdit::singleline(&mut f.v.group).hint_text(lang.t("z. B. Backend/API (leer = keiner)", "e.g. Backend/API (empty = none)")).desired_width(320.0));
                ui.end_row();
                ui.label(lang.t("Tags", "Tags"));
                ui.add(egui::TextEdit::singleline(&mut f.v.tags).hint_text(lang.t("mit Komma getrennt", "comma separated")).desired_width(320.0));
                ui.end_row();
                ui.label(lang.t("Notizen", "Notes"));
                ui.add(egui::TextEdit::multiline(&mut f.v.notes).desired_rows(3).desired_width(320.0));
                ui.end_row();
                if f.chat {
                    return;
                }
                ui.separator();
                ui.separator();
                ui.end_row();
                let prog_profile = profiles.iter().find(|p| p.id == f.v.profile).is_some_and(|p| !p.program.trim().is_empty());
                if !profiles.is_empty() || f.v.profile != 0 {
                    ui.label(lang.t("Profil", "Profile"));
                    let cur = profiles.iter().find(|p| p.id == f.v.profile).map(|p| p.name.clone()).unwrap_or_else(|| lang.t("Keins", "None").to_string());
                    let r = egui::ComboBox::from_id_salt("sset_prof").selected_text(cur).show_ui(ui, |ui| {
                        if ui.selectable_label(f.v.profile == 0, lang.t("Keins", "None")).clicked() {
                            f.pick_profile(None);
                        }
                        for p in profiles {
                            if ui.selectable_label(f.v.profile == p.id, &p.name).clicked() {
                                f.pick_profile(Some(p));
                            }
                        }
                    });
                    mark(ui, "profile", &r.response);
                    ui.end_row();
                }
                ui.label("Shell");
                if prog_profile {
                    ui.label(RichText::new(lang.t("eigenes Programm des Profils", "the profile's own program")).color(style::dim(ui)));
                } else {
                    let cur = shells.iter().find(|s| s.id == f.v.shell_id).map(|s| s.label.clone()).unwrap_or_else(|| f.v.shell_id.clone());
                    egui::ComboBox::from_id_salt("sset_shell").selected_text(cur).show_ui(ui, |ui| {
                        for s in shells {
                            ui.selectable_value(&mut f.v.shell_id, s.id.clone(), &s.label);
                        }
                    });
                }
                ui.end_row();
                ui.label(lang.t("Arbeitsordner", "Working folder"));
                if f.worktree.is_some() {
                    let shell = f.v.shell_id.clone();
                    folder_field(ui, &mut f.v.cwd, &shell, true, lang);
                } else {
                    // Wie im Dialog „Neue Sitzung“: bei einer WSL-Shell Linux-Pfade, Prüfung und Auswahl in WSL (#216).
                    let target = f.target.clone();
                    let (cwd, fresh) = (f.v.cwd.clone(), f.picker.is_none());
                    let picker = f.picker.get_or_insert_with(|| folder_picker::FolderPicker::new(("sset_cwd", f.id), &cwd, target.clone()));
                    picker.set_target(target);
                    if fresh || cwd != f.picker_cwd {
                        // Erstes Bild oder Ordner von außen geändert (Profil): Feld übernimmt ihn als neue Ausgangslage.
                        if !fresh {
                            picker.set_text(&cwd);
                        }
                        f.picker_base = (picker.value(), cwd.clone());
                    }
                    let r = picker.show(ui, lang, &f.wsl_recent);
                    if let Some(sid) = r.distro_changed.and_then(|d| shells.iter().find(|s| s.id.strip_prefix("wsl:").is_some_and(|x| x.eq_ignore_ascii_case(&d)))).map(|s| s.id.clone()) {
                        f.v.shell_id = sid;
                    }
                    let shown = picker.value();
                    f.v.cwd = if shown != f.picker_base.0 { shown } else { f.picker_base.1.clone() };
                    f.picker_cwd = f.v.cwd.clone();
                }
                ui.end_row();
                if f.worktree.is_some() {
                    ui.label("");
                    let r = ui.label(RichText::new(lang.t("Der Arbeitsordner ist der Git-Worktree dieser Sitzung und bleibt fest.", "The working folder is this session's Git worktree and stays fixed.")).small().color(style::dim(ui)));
                    mark(ui, "worktree", &r);
                    ui.end_row();
                }
                ui.label("");
                let prof = profiles.iter().find(|p| p.id == f.v.profile);
                let start_note = match prof {
                    Some(p) if !p.startup.trim().is_empty() || !p.env.trim().is_empty() => format!("{} {}", lang.t("Startbefehl und Umgebungsvariablen kommen aus dem Profil", "Start command and environment variables come from the profile"), lang.q(&p.name)),
                    _ => lang.t("Startbefehl und Umgebungsvariablen legt ein Profil fest (Einstellungen → Profile).", "A profile sets the start command and environment variables (Settings → Profiles).").to_string(),
                };
                ui.label(RichText::new(start_note).small().color(style::dim(ui)));
                ui.end_row();
                if claude_mode || f.v.claude {
                    ui.label("");
                    let r = ui.checkbox(&mut f.v.claude, lang.t("Claude Code starten", "Start Claude Code"));
                    mark(ui, "claude", &r);
                    ui.end_row();
                    if f.v.claude {
                        ui.label("");
                        let r = continue_checkbox(ui, &mut f.v.claude_continue, lang);
                        mark(ui, "continue", &r);
                        ui.end_row();
                        // Claude-Optionen (#161) wie im Dialog „Neue Sitzung“; bypassPermissions nur als Wahl für diese Sitzung.
                        f.opts_ok = super::dialogs::claude_opts_rows(ui, lang, &mut f.v.copts, "sset", super::dialogs::OptsPlace::Session);
                        ui.label("");
                        let r = ui.label(RichText::new(lang.t("Die Optionen gelten nur für diese Sitzung, ab dem nächsten Start.", "The options apply to this session only, from its next start.")).small().color(style::dim(ui)));
                        mark(ui, "copts", &r);
                        ui.end_row();
                    }
                }
            });
            if f.chat {
                ui.add_space(S1);
                ui.label(RichText::new(format!("{} {} – {}", lang.t("Chat mit", "Chat with"), f.chat_model, lang.t("lässt sich nicht in ein Terminal umwandeln.", "cannot be turned into a terminal."))).small().color(style::dim(ui)));
            }
            if reload_hint {
                ui.add_space(S1);
                ui.label(RichText::new(lang.t("Profil, Shell, Arbeitsordner, Claude und die Claude-Optionen wirken erst nach einem Neu laden.", "Profile, shell, working folder, Claude and the Claude options take effect after a reload.")).small().color(style::dim(ui)));
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

/// Häkchen „Konversation fortsetzen“ (Einstellungen der Sitzung und Dialog „Neue Sitzung“).
pub(super) fn continue_checkbox(ui: &mut Ui, on: &mut bool, lang: Lang) -> egui::Response {
    ui.checkbox(on, lang.t("Konversation fortsetzen", "Continue the conversation")).on_hover_text(lang.t(
        "Beim Neu laden, Neu starten und Programmstart setzt Claude die bisherige Konversation fort. Aus: jedes Mal eine neue Konversation.",
        "On reload, restart and program start Claude continues the previous conversation. Off: a new conversation every time.",
    ))
}

impl App {
    /// „Neu laden“: Chats nie; ein beendeter Prozess startet gleich neu, ein laufender erst nach der Rückfrage. Die
    /// Rückfrage einer abgedockten Sitzung erscheint in deren Fenster.
    pub(super) fn request_reload(&mut self, ctx: &egui::Context, id: u64) {
        let Some(e) = self.entries.get(&id) else { return };
        let (chat, exited) = (e.is_chat(), e.exited());
        let lingering = if chat { Vec::new() } else { self.lingering_of(&[id]) };
        match reload_route(chat, exited, !lingering.is_empty()) {
            ReloadRoute::Nothing => {}
            ReloadRoute::Now => self.apply(ctx, Action::Restart(id)),
            ReloadRoute::Ask => {
                // Einmal den Fokus abgeben, damit Enter nicht noch ins Terminal geht; danach hält „Abbrechen“ ihn.
                models_ui::release_focus(ctx);
                self.reload_ask = Some(ReloadAsk { id, after_settings: false, detached: self.detached.contains(&id), lingering });
            }
        }
    }

    /// Rückfrage „Neu laden?“ (ohne Wartezeit). `viewport` = Id der abgedockten Sitzung, deren Fenster gerade gezeichnet
    /// wird (`None` = das normale Fenster).
    pub(super) fn reload_ask_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>, viewport: Option<u64>) {
        let Some(ask) = self.reload_ask.take() else { return };
        let in_detached = ask.detached && self.detached.contains(&ask.id);
        let here = match viewport {
            Some(v) => in_detached && v == ask.id,
            None => !in_detached,
        };
        if !here {
            self.reload_ask = Some(ask);
            return;
        }
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
        // Hintergrundprozesse im Job (#168): „Neu laden, weiterlaufen lassen“ oder „Neu laden, alle beenden“.
        let jobs = !ask.lingering.is_empty();
        let (button, keep) = if jobs {
            (lang.t("Neu laden, alle beenden", "Reload and end all"), Some(lang.t("Neu laden, weiterlaufen lassen", "Reload, keep running")))
        } else {
            (button, None)
        };
        let o = confirm_window_keep(ctx, Id::new("reload_ask"), title, button, keep, 0, lang, 460.0, |ui| {
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
            super::closing::lingering_section(ui, &ask.lingering, lang, false);
        });
        match o {
            Outcome::Go | Outcome::Keep => {
                if o == Outcome::Go && jobs {
                    actions.push(Action::EndJobs(super::closing::job_refs(&ask.lingering)));
                }
                actions.push(Action::Restart(ask.id));
                // Den Fokus nur holen, wenn die Sitzung hier im Layout steht (nicht minimiert, nicht in einem anderen Fenster).
                let shown = self.layout.as_ref().is_some_and(|l| l.contains(ask.id)) && !self.detached.contains(&ask.id);
                self.want_focus = Some(if shown { ask.id } else { self.focused });
            }
            Outcome::Cancel => self.want_focus = Some(self.focused),
            Outcome::Open => self.reload_ask = Some(ask),
        }
    }

    pub(super) fn open_session_settings(&mut self, id: u64) {
        if let Some(e) = self.entries.get(&id) {
            let auto = if e.meta.custom_name { e.live.as_ref().and_then(|s| s.title()).unwrap_or_default() } else { e.display_name() };
            let mut f = SessionForm::from_meta(&e.meta, auto);
            f.wsl_recent = self.settings.wsl_recent.clone();
            self.session_settings = Some(f);
        }
    }

    /// Ziel des Ordnerfelds für die im Dialog gewählte Shell (samt Profil mit eigenem Programm, wie beim Start).
    fn session_form_target(&self, f: &SessionForm, default_shell: &str) -> folder_picker::Target {
        let sid = resolve_shell(self.settings.profiles.iter().find(|p| p.id == f.v.profile), &f.v.shell_id, default_shell);
        let choose = sid.starts_with("wsl:") && self.shells.iter().any(|s| s.id == sid);
        folder_picker::Target::for_shell(self.shell_by_id(&sid).as_ref(), choose)
    }

    /// Dialog „Einstellungen der Sitzung“ (je Fenster).
    pub(super) fn session_settings_ui(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.session_settings.take() else { return };
        if !self.entries.contains_key(&f.id) {
            return; // inzwischen geschlossen
        }
        let lang = self.settings.language;
        let default_shell = self.settings.default_shell.clone();
        let hint = f.needs_reload(&self.settings.profiles, &default_shell);
        let esc_ok = self.reload_ask.is_none();
        f.target = self.session_form_target(&f, &default_shell);
        match session_form_ui(ctx, &mut f, &self.shells, &self.settings.profiles, self.settings.claude_mode, hint, esc_ok, lang) {
            FormOutcome::Open => self.session_settings = Some(f),
            FormOutcome::Cancel => self.want_focus = Some(self.focused),
            FormOutcome::Save => {
                let is_wsl = |sh: &str| self.shell_by_id(sh).is_some_and(|s| crate::shells::is_wsl(&s));
                f.problems = f.validate(&self.shells, &self.settings.profiles, &default_shell, |sh, p| folder_exists(is_wsl(sh), p));
                if f.folder_blocked(lang).is_some() && !f.problems.contains(&Problem::MissingFolder) {
                    f.problems.push(Problem::MissingFolder);
                }
                if !f.problems.is_empty() {
                    self.session_settings = Some(f);
                    return;
                }
                let id = f.id;
                let Some(e) = self.entries.get_mut(&id) else { return };
                let reload = f.apply(&mut e.meta, &self.settings.profiles, &default_shell);
                if let Some(p) = f.picker.as_ref().filter(|_| f.v.cwd.trim() != f.orig.cwd.trim()) {
                    p.remember(&mut self.settings.wsl_recent);
                }
                let group = e.meta.group.clone();
                self.set_group(id, group); // legt den Ordner im Projekt an
                if reload {
                    models_ui::release_focus(ctx);
                    let lingering = self.lingering_of(&[id]);
                    self.reload_ask = Some(ReloadAsk { id, after_settings: true, detached: false, lingering });
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
        assert_eq!(reload_route(false, false, false), ReloadRoute::Ask);
        assert_eq!(reload_route(false, true, false), ReloadRoute::Now);
        assert_eq!(reload_route(true, false, false), ReloadRoute::Nothing);
        assert_eq!(reload_route(true, true, false), ReloadRoute::Nothing);
        // Hintergrundprozesse im Job (#168): auch bei beendeter Shell fragen (weiterlaufen lassen oder beenden)
        assert_eq!(reload_route(false, true, true), ReloadRoute::Ask);
        assert_eq!(reload_route(false, false, true), ReloadRoute::Ask);
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
                o = confirm_window_keep(ctx, Id::new("reload_ask"), "Reload session?", "Reload", None, 0, Lang::En, 420.0, |ui| {
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
        assert_eq!((f.v.profile, f.v.shell_id.as_str(), f.v.cwd.as_str()), (2, "bash", "C:\\work"));
        f.pick_profile(Some(&p[0]));
        assert_eq!(f.v.shell_id, "custom:1");
        f.pick_profile(None);
        assert_eq!(f.v.profile, 0);
        // Worktree: der Ordner des Profils ersetzt den Worktree-Ordner nicht
        let wt = SessionMeta { cwd: "C:\\wt".into(), worktree: Some(state::Worktree { repo: "C:\\r".into(), path: "C:\\wt".into(), branch: "b".into() }), ..SessionMeta::default() };
        let mut f = form(&wt);
        f.pick_profile(Some(&p[1]));
        assert_eq!(f.v.cwd, "C:\\wt");
    }

    #[test]
    fn validation_checks_only_what_changed() {
        let (p, s) = (profiles(), shells());
        let meta = SessionMeta { shell_id: "pwsh".into(), cwd: "C:\\gone".into(), ..SessionMeta::default() };
        let dir = |_shell: &str, d: &str| d == "C:\\ok";
        let mut f = form(&meta);
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty(), "unveränderter (auch fehlender) Ordner sperrt nicht");
        f.v.cwd = "C:\\missing".into();
        assert_eq!(f.validate(&s, &p, "pwsh", dir), vec![Problem::MissingFolder]);
        f.v.cwd = "C:\\ok".into();
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty());
        f.v.cwd = "  ".into();
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty(), "leer = Benutzerordner");
        f.v.shell_id = "fish".into();
        assert_eq!(f.validate(&s, &p, "pwsh", dir), vec![Problem::UnknownShell]);
        f.v.shell_id = "pwsh".into();
        f.v.profile = 9;
        assert_eq!(f.validate(&s, &p, "pwsh", dir), vec![Problem::UnknownProfile]);
        f.v.profile = 1; // Profil mit Programm: Shell egal
        f.v.shell_id = "fish".into();
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty());
        // Unverändert ungültig (Profil gelöscht, Shell deinstalliert): sperrt andere Änderungen nicht
        let stale = SessionMeta { shell_id: "fish".into(), profile: 9, ..SessionMeta::default() };
        let mut f = form(&stale);
        f.v.name = "x".into();
        assert!(f.validate(&s, &p, "pwsh", dir).is_empty());
        // Die Prüfung des Ordners bekommt die gewählte Shell (WSL-Pfade, #172)
        let mut f = form(&meta);
        f.v.shell_id = "bash".into();
        f.v.cwd = "/home/u".into();
        assert!(f.validate(&s, &p, "pwsh", |sh, d| sh == "bash" && d == "/home/u").is_empty());
        // Chat: nichts davon
        let mut c = form(&SessionMeta { kind: SessionKind::Chat, ..SessionMeta::default() });
        c.v.profile = 9;
        c.v.cwd = "C:\\missing".into();
        assert!(c.validate(&s, &p, "pwsh", dir).is_empty());
        for pr in [Problem::UnknownProfile, Problem::UnknownShell, Problem::MissingFolder, Problem::InvalidClaudeOption] {
            assert_ne!(pr.text(Lang::De), pr.text(Lang::En));
        }
    }

    #[test]
    fn apply_changes_and_tells_when_a_reload_is_needed() {
        let p = profiles();
        let mut meta = SessionMeta { id: 3, name: "x".into(), shell_id: "pwsh".into(), cwd: "C:\\a".into(), tags: vec!["t".into()], ..SessionMeta::default() };
        // Nur Name, Farbe, Ordner, Tags, Notizen: kein Neu laden
        let mut f = form(&meta);
        assert_eq!(f.v.name, "", "automatischer Name → leeres Feld");
        assert_eq!(f.v.tags, "t");
        f.v.name = "  Build ".into();
        f.v.color = crate::palette::rgb(crate::palette::HUB_DEFAULTS[2]);
        f.v.group = " a / b ".into();
        f.v.tags = "x, #y,, z ".into();
        f.v.notes = "n".into();
        assert!(!f.apply(&mut meta, &p, "pwsh"));
        assert_eq!((meta.name.as_str(), meta.custom_name), ("Build", true));
        assert_eq!(meta.color, crate::palette::rgb(crate::palette::HUB_DEFAULTS[2]));
        assert_eq!(meta.group, "a/b");
        assert_eq!(meta.tags, vec!["x", "y", "z"]);
        assert_eq!(meta.notes, "n");
        // „Konversation fortsetzen“ allein: wirkt beim nächsten Neu laden, keine Rückfrage
        let mut f = form(&meta);
        f.v.claude_continue = false;
        assert!(!f.apply(&mut meta, &p, "pwsh"));
        assert!(!meta.claude_continue);
        // Name leeren → wieder automatisch
        let mut f = form(&meta);
        f.v.name.clear();
        f.apply(&mut meta, &p, "pwsh");
        assert!(!meta.custom_name);
        // Claude an, Ordner, Shell, Profil: Neu laden nötig
        for change in 0..4 {
            let mut m = meta.clone();
            let mut f = form(&m);
            match change {
                0 => f.v.claude = true,
                1 => f.v.cwd = "C:\\b".into(),
                2 => f.v.shell_id = "bash".into(),
                _ => f.pick_profile(Some(&p[0])),
            }
            assert!(f.needs_reload(&p, "pwsh"), "Änderung {change}");
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
        f.v.claude = false;
        assert!(f.apply(&mut m, &p, "pwsh"));
        assert!(!m.claude);
        // Worktree: der Arbeitsordner bleibt
        let mut m = SessionMeta { cwd: "C:\\wt".into(), worktree: Some(state::Worktree { repo: "C:\\r".into(), path: "C:\\wt".into(), branch: "b".into() }), ..SessionMeta::default() };
        let mut f = form(&m);
        assert!(f.worktree.is_some());
        f.v.cwd = "C:\\elsewhere".into();
        f.apply(&mut m, &p, "pwsh");
        assert_eq!(m.cwd, "C:\\wt");
    }

    #[test]
    fn saving_keeps_changes_made_elsewhere_while_the_dialog_was_open() {
        let p = profiles();
        let mut meta = SessionMeta { id: 4, name: "old".into(), custom_name: true, shell_id: "pwsh".into(), notes: "a".into(), ..SessionMeta::default() };
        let mut f = form(&meta);
        // Inzwischen: Notizen-Fenster, MCP rename_session, Farbe im Kontextmenü, Tags-Eingabe
        meta.notes = "from notes panel".into();
        meta.name = "renamed by agent".into();
        meta.color = crate::palette::nth_rgb(crate::palette::Area::Hub, 5);
        meta.tags = vec!["t2".into()];
        // Im Dialog nur der Ordner im Baum geändert
        f.v.group = "G".into();
        assert!(!f.apply(&mut meta, &p, "pwsh"));
        assert_eq!(meta.group, "G");
        assert_eq!(meta.notes, "from notes panel");
        assert_eq!(meta.name, "renamed by agent");
        assert_eq!(meta.color, crate::palette::nth_rgb(crate::palette::Area::Hub, 5));
        assert_eq!(meta.tags, vec!["t2"]);
        assert_eq!(meta.shell_id, "pwsh");
    }

    #[test]
    fn claude_options_are_per_session_start_time_settings() {
        let p = profiles();
        let mut meta = SessionMeta { id: 6, claude: true, shell_id: "pwsh".into(), claude_args: "--model opus --permission-mode plan --verbose".into(), ..SessionMeta::default() };
        // Alte Sitzung: Optionen aus claude_args erscheinen im Dialog
        let f = form(&meta);
        assert_eq!((f.v.copts.model.as_str(), f.v.copts.permission_mode.as_str()), ("opus", "plan"));
        // Unverändert: nichts geschrieben, kein Neu laden
        assert!(!f.apply(&mut meta, &p, "pwsh"));
        assert!(meta.claude_opts.is_empty());
        assert_eq!(meta.claude_args, "--model opus --permission-mode plan --verbose");
        // Geändert (Modell geleert = Standard, Effort, bypassPermissions als ausdrückliche Wahl): Neu laden nötig
        let mut f = form(&meta);
        f.v.copts.model.clear();
        f.v.copts.effort = "high".into();
        f.v.copts.permission_mode = crate::claude_opts::BYPASS.into();
        assert!(f.needs_reload(&p, "pwsh"));
        assert!(f.validate(&shells(), &p, "pwsh", |_, _| true).is_empty());
        assert!(f.apply(&mut meta, &p, "pwsh"));
        assert_eq!(meta.claude_opts.effort, "high");
        assert_eq!(meta.claude_opts.permission_mode, "bypassPermissions", "nur für diese Sitzung");
        assert!(meta.claude_opts.model.is_empty());
        assert_eq!(meta.claude_args, "", "alte Angaben fallen weg, sonst käme opus zurück");
        assert_eq!(meta.claude_opts.extra.flags, ["verbose"], "der alte Schalter ist jetzt ein Schalter (#182)");
        assert_eq!(meta.claude_opts_eff().0.model, "", "geleertes Modell bleibt Standard");
        // Ungültiger Name sperrt das Speichern
        let mut f = form(&meta);
        f.v.copts.agent = "bad name!".into();
        f.opts_ok = false;
        assert_eq!(f.validate(&shells(), &p, "pwsh", |_, _| true), vec![Problem::InvalidClaudeOption]);
        // Ohne Claude zählen die Optionen nicht als Start-Änderung
        let mut t = SessionMeta { id: 7, shell_id: "pwsh".into(), ..SessionMeta::default() };
        let mut f = form(&t);
        f.v.copts.model = "haiku".into();
        assert!(!f.needs_reload(&p, "pwsh"));
        f.apply(&mut t, &p, "pwsh");
        assert_eq!(t.claude_opts.model, "haiku", "gespeichert für einen späteren Claude-Start");
    }

    #[test]
    fn more_claude_options_are_start_time_settings() {
        use crate::claude_opts::ExtraOpts;
        let p = profiles();
        let mut meta = SessionMeta { id: 8, claude: true, shell_id: "pwsh".into(), ..SessionMeta::default() };
        // Schalter ändern = Start-Änderung („Jetzt neu laden?“)
        let mut f = form(&meta);
        f.v.copts.extra.flags = vec!["chrome".into(), "ide".into()];
        assert!(f.needs_reload(&p, "pwsh"));
        assert!(f.validate(&shells(), &p, "pwsh", |_, _| true).is_empty());
        assert!(f.apply(&mut meta, &p, "pwsh"));
        assert_eq!(meta.claude_opts.extra.flags, ["chrome", "ide"]);
        // Wertoptionen des Nutzers (auch die Rechte erweiternden) für diese Sitzung; Text mit Leerzeichen am Rand ist gültig.
        let mut f = form(&meta);
        f.v.copts.extra.append_system_prompt = "Be brief. ".into();
        f.v.copts.extra.allowed_tools = "Read".into();
        assert!(f.needs_reload(&p, "pwsh"));
        assert!(f.validate(&shells(), &p, "pwsh", |_, _| true).is_empty());
        assert!(f.apply(&mut meta, &p, "pwsh"));
        assert_eq!(meta.claude_opts.extra, ExtraOpts { flags: vec!["chrome".into(), "ide".into()], allowed_tools: "Read".into(), append_system_prompt: "Be brief.".into(), ..Default::default() });
        // Ungültiger Wert sperrt das Speichern (auch wenn die Zeilen ihn nicht gemeldet hätten).
        let mut f = form(&meta);
        f.v.copts.extra.max_budget_usd = "lots".into();
        assert_eq!(f.validate(&shells(), &p, "pwsh", |_, _| true), vec![Problem::InvalidClaudeOption]);
        // Unverändert: kein Neu laden
        assert!(!form(&meta).needs_reload(&p, "pwsh"));
    }

    #[test]
    fn the_dialog_shows_more_claude_options() {
        let mut d = Dlg::new();
        let mut f = form(&SessionMeta { id: 9, claude: true, shell_id: "pwsh".into(), ..SessionMeta::default() });
        f.v.copts.extra.flags = vec!["verbose".into()];
        d.frame(&mut f, true, vec![]);
        d.frame(&mut f, true, vec![]);
        assert!(f.opts_ok);
        assert_eq!(f.v.copts.extra.flags, ["verbose"], "Abschnitt offen, Häkchen unverändert");
        // Ungültiger Wert im Abschnitt wird erkannt.
        f.v.copts.extra.append_system_prompt = "100% sure".into();
        d.frame(&mut f, true, vec![]);
        assert!(!f.opts_ok);
        assert_eq!(f.validate(&shells(), &profiles(), "pwsh", |_, _| true), vec![Problem::InvalidClaudeOption]);
    }

    #[test]
    fn the_dialog_shows_claude_options_with_the_bypass_warning() {
        let mut d = Dlg::new();
        let mut f = form(&SessionMeta { id: 1, claude: true, shell_id: "pwsh".into(), ..SessionMeta::default() });
        d.frame(&mut f, true, vec![]);
        d.frame(&mut f, true, vec![]);
        assert!(d.find("copts").is_some(), "Claude-Optionen sichtbar");
        f.v.copts.agent = "bad name!".into();
        d.frame(&mut f, true, vec![]);
        assert!(!f.opts_ok, "ungültiger Agentenname erkannt");
        // Ohne Claude: keine Optionen
        let mut d = Dlg::new();
        let mut t = form(&SessionMeta { id: 2, shell_id: "pwsh".into(), ..SessionMeta::default() });
        d.frame(&mut t, true, vec![]);
        d.frame(&mut t, true, vec![]);
        assert!(d.find("copts").is_none());
    }

    #[test]
    fn chats_stay_chats() {
        let p = profiles();
        let mut meta = SessionMeta { id: 5, kind: SessionKind::Chat, chat: Some(state::ChatMeta { server: "s".into(), model: "m".into() }), ..SessionMeta::default() };
        let mut f = form(&meta);
        assert!(f.chat);
        assert_eq!(f.chat_model, "m");
        f.v.claude = true;
        f.v.shell_id = "pwsh".into();
        f.v.cwd = "C:\\x".into();
        f.v.profile = 2;
        f.v.name = "Ask".into();
        assert!(!f.apply(&mut meta, &p, "pwsh"), "ein Chat wird nie neu geladen");
        assert_eq!(meta.kind, SessionKind::Chat);
        assert!(!meta.claude && meta.shell_id.is_empty() && meta.cwd.is_empty() && meta.profile == 0);
        assert_eq!(meta.name, "Ask");
    }

    #[test]
    fn folder_check_respects_wsl_and_never_probes_shares() {
        // WSL-Shell: Linux-Pfade prüft das Ordnerfeld im Hintergrund – hier kein Einwand (#216)
        assert!(folder_exists(true, "/home/u/projekt"));
        assert!(folder_exists(true, "~/x"));
        // WSL-Freigaben werden nie geprüft (könnte die WSL-VM starten)
        assert!(folder_exists(false, r"\\wsl.localhost\Ubuntu\home\u"));
        assert!(folder_exists(true, r"\\wsl$\Ubuntu\home\u"));
        // Windows-Ordner wie bisher
        let here = std::env::current_dir().unwrap().to_string_lossy().to_string();
        assert!(folder_exists(false, &here) && folder_exists(true, &here));
        assert!(!folder_exists(false, &format!("{here}\\gibt-es-nicht-xyz")));
        assert!(!folder_exists(false, "/home/u/projekt"), "Linux-Pfad ohne WSL-Shell");
    }

    #[test]
    fn the_folder_field_keeps_the_saved_folder_and_follows_profiles() {
        let mut d = Dlg::new();
        let mut f = form(&SessionMeta { id: 4, shell_id: "pwsh".into(), cwd: "C:\\a".into(), ..SessionMeta::default() });
        for _ in 0..3 {
            d.frame(&mut f, false, vec![]);
        }
        assert_eq!(f.v.cwd, "C:\\a", "unverändert");
        assert!(f.folder_blocked(Lang::En).is_none(), "Windows-Pfad: keine Sperre");
        // Profil mit Ordner gewählt: Feld und Wert folgen
        f.pick_profile(Some(&profiles()[1]));
        d.frame(&mut f, false, vec![]);
        d.frame(&mut f, false, vec![]);
        assert_eq!(f.v.cwd, "C:\\work");
        assert_eq!(f.picker.as_ref().map(|p| p.value()), Some("C:\\work".to_string()));
        // Worktree: kein Ordnerfeld, Wert fest
        let mut w = form(&SessionMeta { id: 5, cwd: "C:\\wt".into(), worktree: Some(state::Worktree { repo: "C:\\r".into(), path: "C:\\wt".into(), branch: "b".into() }), ..SessionMeta::default() });
        d.frame(&mut w, false, vec![]);
        assert!(w.picker.is_none() && w.folder_blocked(Lang::En).is_none());
        assert_eq!(w.v.cwd, "C:\\wt");
    }

    /// Dialog ohne Fenster: einige Bilder mit Ereignissen.
    struct Dlg {
        ctx: egui::Context,
        t: f64,
        esc_ok: bool,
    }

    impl Dlg {
        fn new() -> Dlg {
            Dlg { ctx: egui::Context::default(), t: 0.0, esc_ok: true }
        }
        fn frame(&mut self, f: &mut SessionForm, claude_mode: bool, events: Vec<Event>) -> FormOutcome {
            self.frame_left(f, claude_mode, events).0
        }
        /// Wie `frame`; dazu die Tasten, die danach noch übrig sind (die bekäme ein Terminal oder ein zweiter Dialog).
        fn frame_left(&mut self, f: &mut SessionForm, claude_mode: bool, events: Vec<Event>) -> (FormOutcome, Vec<Key>) {
            self.t += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))), time: Some(self.t), events, ..Default::default() };
            let (mut out, mut left) = (FormOutcome::Open, Vec::new());
            let (s, p) = (shells(), profiles());
            let esc_ok = self.esc_ok;
            let _ = self.ctx.run(input, |ctx| {
                out = session_form_ui(ctx, f, &s, &p, claude_mode, false, esc_ok, Lang::En);
                left = ctx.input(|i| i.events.iter().filter_map(|e| if let Event::Key { key, pressed: true, .. } = e { Some(*key) } else { None }).collect());
            });
            (out, left)
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

    fn esc() -> Event {
        Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    #[test]
    fn dialog_saves_cancels_and_shows_claude_rows_only_for_terminals() {
        let mut d = Dlg::new();
        let mut f = form(&SessionMeta { id: 1, claude: true, shell_id: "pwsh".into(), ..SessionMeta::default() });
        assert_eq!(d.frame(&mut f, true, vec![]), FormOutcome::Open);
        assert!(!f.focus, "Fokus ging beim ersten Bild ins Namensfeld");
        d.frame(&mut f, true, vec![]);
        assert!(d.find("continue").is_some(), "Claude-Sitzung: Option sichtbar");
        assert!(d.find("claude").is_some());
        assert!(d.find("worktree").is_none());
        // Häkchen klicken schaltet die Option
        let r = d.find("continue").unwrap();
        d.click(&mut f, true, r.center());
        assert!(!f.v.claude_continue);
        // Speichern
        let r = d.find("save").unwrap();
        assert_eq!(d.click(&mut f, true, r.center()), FormOutcome::Save);
        // Esc bricht ab und wird verbraucht (kein zweiter Dialog, kein Terminal bekommt es)
        let (o, left) = d.frame_left(&mut f, true, vec![esc()]);
        assert_eq!(o, FormOutcome::Cancel);
        assert!(left.is_empty(), "{left:?}");
        // Chat: keine Claude-, Shell- oder Ordnerzeilen
        let mut d = Dlg::new();
        let mut c = form(&SessionMeta { id: 2, kind: SessionKind::Chat, ..SessionMeta::default() });
        d.frame(&mut c, true, vec![]);
        d.frame(&mut c, true, vec![]);
        assert!(d.find("claude").is_none());
        assert!(d.find("continue").is_none());
        assert!(d.find("save").is_some());
        // Terminal ohne Claude-Modus: kein Claude-Häkchen
        let mut d = Dlg::new();
        let mut t = form(&SessionMeta { id: 3, shell_id: "pwsh".into(), ..SessionMeta::default() });
        d.frame(&mut t, false, vec![]);
        d.frame(&mut t, false, vec![]);
        assert!(d.find("claude").is_none());
        // Abbrechen-Knopf
        let r = d.find("cancel").unwrap();
        assert_eq!(d.click(&mut t, false, r.center()), FormOutcome::Cancel);
    }

    #[test]
    fn esc_waits_for_a_question_on_top_and_for_open_lists() {
        // Rückfrage darüber offen: Esc gehört ihr, der Dialog bleibt und lässt die Taste liegen
        let mut d = Dlg::new();
        d.esc_ok = false;
        let mut f = form(&SessionMeta { id: 1, shell_id: "pwsh".into(), ..SessionMeta::default() });
        d.frame(&mut f, true, vec![]);
        let (o, left) = d.frame_left(&mut f, true, vec![esc()]);
        assert_eq!(o, FormOutcome::Open);
        assert_eq!(left, vec![Key::Escape]);
        // Offene Auswahlliste: Esc schließt erst die Liste, nicht den Dialog
        let mut d = Dlg::new();
        let mut f = form(&SessionMeta { id: 1, shell_id: "pwsh".into(), profile: 2, ..SessionMeta::default() });
        d.frame(&mut f, true, vec![]);
        d.frame(&mut f, true, vec![]);
        let r = d.find("profile").unwrap();
        d.click(&mut f, true, r.center());
        assert!(egui::Popup::is_any_open(&d.ctx), "Profilliste offen");
        assert_eq!(d.frame(&mut f, true, vec![esc()]), FormOutcome::Open, "Esc schließt nur die Liste");
    }

    #[test]
    fn a_worktree_locks_the_folder_with_a_hint() {
        let mut d = Dlg::new();
        let mut f = form(&SessionMeta { id: 1, shell_id: "pwsh".into(), cwd: "C:\\wt".into(), worktree: Some(state::Worktree { repo: "C:\\r".into(), path: "C:\\wt".into(), branch: "b".into() }), ..SessionMeta::default() });
        d.frame(&mut f, true, vec![]);
        d.frame(&mut f, true, vec![]);
        assert!(d.find("worktree").is_some());
    }

    #[test]
    fn color_row_offers_the_palette_and_a_custom_color() {
        let mut d = Dlg::new();
        let mut f = form(&SessionMeta { id: 1, shell_id: "pwsh".into(), ..SessionMeta::default() });
        for _ in 0..4 {
            d.frame(&mut f, false, vec![]);
        }
        let r0 = d.ctx.read_response(Id::new("sset_color").with(("pal", 2usize))).map(|r| r.rect);
        d.frame(&mut f, false, vec![]);
        let r1 = d.ctx.read_response(Id::new("sset_color").with(("pal", 2usize))).map(|r| r.rect);
        assert!(r0.zip(r1).is_some_and(|(a, b)| (a.center() - b.center()).length() < 3.0), "Lage stabil");
        let sw = |d: &Dlg, i: usize| d.ctx.read_response(Id::new("sset_color").with(("pal", i))).expect("Farbfeld").rect;
        // Farbfeld 2 der geltenden Palette
        d.click(&mut f, false, sw(&d, 2).center());
        assert_eq!(f.v.color, crate::palette::rgb(&crate::palette::colors(crate::palette::Area::Hub)[2].hex));
        // „Eigene Farbe …“ unter den Farbfeldern aufklappen, Hex eintippen, übernehmen
        let last = sw(&d, 10);
        d.click(&mut f, false, Pos2::new(last.left() + 30.0, last.bottom() + 14.0));
        // Der Dialog wächst und rückt neu in die Mitte: Lage erst danach lesen.
        let hex = |d: &mut Dlg, f: &mut SessionForm| {
            for _ in 0..4 {
                d.frame(f, false, vec![]);
            }
            d.ctx.read_response(Id::new("sset_color").with("custom").with("hex")).expect("Eigene Farbe aufgeklappt").rect
        };
        let r = hex(&mut d, &mut f);
        d.click(&mut f, false, r.center());
        d.frame(&mut f, false, vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }]);
        d.frame(&mut f, false, vec![Event::Text("#2288FF".into())]);
        let hex = hex(&mut d, &mut f);
        assert_eq!(d.click(&mut f, false, Pos2::new(hex.right() + 8.0 + 20.0, hex.center().y)), FormOutcome::Open, "der Dialog bleibt offen");
        assert_eq!(f.v.color, [0x22, 0x88, 0xFF]);
        assert!(!crate::palette::colors(crate::palette::Area::Hub).iter().any(|c| c.hex == "#2288FF"), "nicht in der Palette");
    }
}
